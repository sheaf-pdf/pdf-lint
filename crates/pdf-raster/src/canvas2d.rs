//! Private 2D fine rasterizer & PDF §11 blend compositor wrapping `vello_cpu`.
//!
//! No `vello_cpu`, `kurbo`, or `peniko` type ever appears in `pdf-raster`'s
//! public API. Coordinates are sanitized and clamped before reaching `kurbo`
//! curve flatteners, and final scene rasterization runs inside `catch_unwind`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use pdf_content_streams::{Blend, FillRule, LineCap, LineJoin};
use pdf_fonts::OutlineVerb;
use pdf_gfx::{BlendState, DashPattern, Matrix, Point, Seg};
use vello_cpu::color::{AlphaColor, Srgb};
use vello_cpu::kurbo::{self, Affine, BezPath, Cap, Join, PathEl, Stroke};
use vello_cpu::peniko::{
    BlendMode, Compose, Extend, Fill, ImageAlphaType, ImageQuality, ImageSampler, Mix,
};
use vello_cpu::{
    Image, ImageSource, Level, Mask, Pixmap as VelloPixmap, RasterizerSettings, RenderContext,
    RenderMode, RenderSettings, Resources, TargetInit,
};

use crate::error::Error;
use crate::limits::Limits;
use crate::pixmap::{PixelFormat, Pixmap, unpremul_rgba};

/// Coordinate clamp (`±1,000,000` user/device units) guarding `kurbo` curve
/// subdivision and dash expansion from runaway work on extreme floats.
const MAX_COORD: f64 = 1_000_000.0;

/// Maximum dash-pattern expansion count per stroke to prevent CPU exhaustion on
/// `0.0001`-unit dashes over huge paths.
const MAX_DASH_SEGMENTS: f64 = 32_768.0;

pub(crate) fn simd_level() -> Level {
    #[cfg(feature = "simd")]
    {
        Level::try_detect().unwrap_or(Level::baseline())
    }
    #[cfg(not(feature = "simd"))]
    {
        Level::baseline()
    }
}

pub(crate) struct Canvas2d {
    ctx: RenderContext,
    width: u16,
    height: u16,
}

impl Canvas2d {
    pub(crate) fn new(width: u32, height: u32) -> Result<Self, Error> {
        let w = u16::try_from(width)
            .ok()
            .filter(|&v| v > 0)
            .ok_or(Error::InvalidViewport("width out of u16 range"))?;
        let h = u16::try_from(height)
            .ok()
            .filter(|&v| v > 0)
            .ok_or(Error::InvalidViewport("height out of u16 range"))?;
        let settings = RenderSettings {
            level: simd_level(),
            num_threads: 0,
        };
        Ok(Self {
            ctx: RenderContext::new_with(w, h, settings),
            width: w,
            height: h,
        })
    }

    pub(crate) const fn width(&self) -> u16 {
        self.width
    }

    pub(crate) const fn height(&self) -> u16 {
        self.height
    }

    pub(crate) fn set_anti_alias(&mut self, enabled: bool) {
        self.ctx
            .set_aliasing_threshold(if enabled { None } else { Some(128) });
    }

    pub(crate) fn set_transform(&mut self, m: Matrix) {
        self.ctx.set_transform(matrix_to_affine(m));
    }

    pub(crate) fn set_fill_rule(&mut self, rule: FillRule) {
        self.ctx.set_fill_rule(fill_rule_to_peniko(rule));
    }

    pub(crate) fn set_blend_mode(&mut self, mode: &BlendState) {
        self.ctx.set_blend_mode(blend_to_peniko(mode));
    }

    pub(crate) fn set_solid_rgba8(&mut self, [r, g, b, a]: [u8; 4]) {
        self.ctx.reset_paint_transform();
        self.ctx.reset_tint();
        self.ctx
            .set_paint(AlphaColor::<Srgb>::from_rgba8(r, g, b, a));
    }

    pub(crate) fn set_tiling_paint(&mut self, tile: Arc<VelloPixmap>, paint_transform: Matrix) {
        self.ctx.reset_tint();
        let brush = Image {
            image: ImageSource::Pixmap(tile),
            sampler: ImageSampler {
                x_extend: Extend::Repeat,
                y_extend: Extend::Repeat,
                quality: ImageQuality::Medium,
                alpha: 1.0,
            },
        };
        self.ctx.set_paint(brush);
        self.ctx
            .set_paint_transform(matrix_to_affine(paint_transform));
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn set_stroke_style(
        &mut self,
        line_width: f32,
        cap: LineCap,
        join: LineJoin,
        miter_limit: f32,
        dash: &DashPattern,
        ctm_to_device: Matrix,
        path_extent: f64,
    ) {
        // ISO 32000-2 §8.4.3.2: a line width of 0 denotes the thinnest line
        // (~1 device pixel) that can be rendered at device resolution.
        let effective_width = if line_width.abs() <= 1e-6 {
            let sx = f64::from(ctm_to_device.a).hypot(f64::from(ctm_to_device.b));
            let sy = f64::from(ctm_to_device.c).hypot(f64::from(ctm_to_device.d));
            let scale = sx.max(sy);
            if scale > 1e-6 {
                (1.0 / scale).clamp(1e-4, MAX_COORD)
            } else {
                1.0
            }
        } else {
            f64::from(line_width.abs()).min(MAX_COORD)
        };

        let mut stroke = Stroke::new(effective_width)
            .with_caps(match cap {
                LineCap::Butt => Cap::Butt,
                LineCap::Round => Cap::Round,
                LineCap::Square => Cap::Square,
            })
            .with_join(match join {
                LineJoin::Miter => Join::Miter,
                LineJoin::Round => Join::Round,
                LineJoin::Bevel => Join::Bevel,
            })
            .with_miter_limit(f64::from(miter_limit.max(1.0)).min(1_000.0));

        if !dash.is_solid() {
            let cycle_sum: f64 = dash.array.iter().map(|&v| f64::from(v.max(0.0))).sum();
            if cycle_sum > 1e-5 && path_extent / cycle_sum <= MAX_DASH_SEGMENTS {
                let mut pattern: Vec<f64> = dash
                    .array
                    .iter()
                    .take(64)
                    .map(|&v| f64::from(v.max(0.0)).min(MAX_COORD))
                    .collect();
                // Odd-length dash arrays repeat twice per cycle in PDF (§8.4.3.6).
                if pattern.len() % 2 == 1 {
                    let clone = pattern.clone();
                    pattern.extend_from_slice(&clone);
                }
                let phase = f64::from(dash.phase).rem_euclid(cycle_sum * 2.0);
                stroke = stroke.with_dashes(phase, pattern);
            }
        }
        self.ctx.set_stroke(stroke);
    }

    pub(crate) fn fill_bez_path(&mut self, path: &BezPath) {
        if !path.is_empty() {
            self.ctx.fill_path(path);
        }
    }

    pub(crate) fn stroke_bez_path(&mut self, path: &BezPath) {
        if !path.is_empty() {
            self.ctx.stroke_path(path);
        }
    }

    pub(crate) fn push_clip(&mut self, path: &BezPath, rule: FillRule, transform: Matrix) {
        self.ctx.set_transform(matrix_to_affine(transform));
        self.ctx.set_fill_rule(fill_rule_to_peniko(rule));
        self.ctx.push_clip_path(path);
    }

    pub(crate) fn pop_clip(&mut self) {
        self.ctx.pop_clip();
    }

    pub(crate) fn set_mask(&mut self, mask: Option<Mask>) {
        match mask {
            Some(m) => self.ctx.set_mask(m),
            None => self.ctx.reset_mask(),
        }
    }

    pub(crate) fn push_group_layer(
        &mut self,
        clip_path: Option<(&BezPath, FillRule, Matrix)>,
        blend_mode: &BlendState,
        opacity: f32,
        mask: Option<Mask>,
    ) {
        let clip_bez = if let Some((path, rule, transform)) = clip_path {
            self.ctx.set_transform(matrix_to_affine(transform));
            self.ctx.set_fill_rule(fill_rule_to_peniko(rule));
            Some(path)
        } else {
            self.ctx.set_transform(Affine::IDENTITY);
            None
        };
        self.ctx.push_layer(
            clip_bez,
            Some(blend_to_peniko(blend_mode)),
            Some(opacity.clamp(0.0, 1.0)),
            mask,
            None,
        );
    }

    pub(crate) fn pop_layer(&mut self) {
        self.ctx.pop_layer();
    }

    /// Draws an `RGBA8` image (`width × height`) placed onto PDF's `[0, 1] × [0, 1]`
    /// unit square (`y = 0` bottom, `y = 1` top) under `unit_to_device`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_rgba_image(
        &mut self,
        rgba: Vec<u8>,
        img_w: u32,
        img_h: u32,
        unit_to_device: Matrix,
        interpolate: bool,
        opacity: f32,
        blend: &BlendState,
    ) {
        let (Ok(w), Ok(h)) = (u16::try_from(img_w), u16::try_from(img_h)) else {
            return;
        };
        if w == 0 || h == 0 {
            return;
        }
        let Some(vello_pix) = build_vello_pixmap_straight(rgba, w, h) else {
            return;
        };

        // Image pixel space `[0..w] × [0..h]` (`y` down, row 0 = top) -> PDF unit
        // square `[0..1] × [0..1]` (`y` up, `y = 1` = top):
        // x_u = x_px / w, y_u = 1.0 - y_px / h.
        let img_to_unit = Matrix::new(
            1.0 / (img_w as f32),
            0.0,
            0.0,
            -1.0 / (img_h as f32),
            0.0,
            1.0,
        );
        let img_to_device = img_to_unit.then(unit_to_device);
        if !img_to_device.is_finite() || img_to_device.determinant() == 0.0 {
            return;
        }

        let brush = Image {
            image: ImageSource::Pixmap(Arc::new(vello_pix)),
            sampler: ImageSampler {
                x_extend: Extend::Pad,
                y_extend: Extend::Pad,
                quality: if interpolate {
                    ImageQuality::Medium
                } else {
                    ImageQuality::Low
                },
                alpha: opacity.clamp(0.0, 1.0),
            },
        };
        self.ctx.set_blend_mode(blend_to_peniko(blend));
        self.ctx.reset_paint_transform();
        self.ctx.reset_tint();
        self.ctx.set_transform(matrix_to_affine(img_to_device));
        self.ctx.set_paint(brush);
        let rect = kurbo::Rect::new(0.0, 0.0, f64::from(w), f64::from(h));
        self.ctx.fill_rect(&rect);
    }

    /// Rasterizes into an internal `VelloPixmap` (premultiplied `RGBA8`), used
    /// for offscreen soft-mask groups and tiling pattern cells.
    pub(crate) fn render_to_vello_pixmap(
        mut self,
        background: Option<[u8; 4]>,
    ) -> Result<VelloPixmap, Error> {
        let init = match background {
            Some([r, g, b, a]) => TargetInit::Clear(AlphaColor::<Srgb>::from_rgba8(r, g, b, a)),
            None => TargetInit::Clear(AlphaColor::TRANSPARENT),
        };
        let settings = RasterizerSettings {
            render_mode: RenderMode::OptimizeSpeed,
            target_init: init,
            ..RasterizerSettings::default()
        };
        let (w, h) = (self.width, self.height);
        catch_unwind(AssertUnwindSafe(move || {
            let mut resources = Resources::new();
            let mut target = VelloPixmap::new(w, h);
            self.ctx.flush();
            self.ctx.render_with(&mut target, &mut resources, settings);
            target
        }))
        .map_err(|_| Error::RasterizerPanic)
    }

    /// Finishes rasterization and converts the output into a [`Pixmap`] in
    /// `format`.
    pub(crate) fn finish(
        self,
        background: Option<[u8; 4]>,
        format: PixelFormat,
        limits: &Limits,
    ) -> Result<Pixmap, Error> {
        let (w, h) = (u32::from(self.width), u32::from(self.height));
        let vello_pix = self.render_to_vello_pixmap(background)?;
        let premul = vello_pix.data_as_u8_slice();
        let mut out = Vec::with_capacity(premul.len());
        match format {
            PixelFormat::Rgba8Premul => out.extend_from_slice(premul),
            PixelFormat::Rgba8 => {
                for px in premul.as_chunks::<4>().0 {
                    out.extend_from_slice(&unpremul_rgba(*px));
                }
            }
            PixelFormat::Bgra8 => {
                let opaque_bg = background.is_some_and(|[_, _, _, a]| a == 255);
                if opaque_bg {
                    for &[r, g, b, a] in premul.as_chunks::<4>().0 {
                        out.extend_from_slice(&[b, g, r, a]);
                    }
                } else {
                    for px in premul.as_chunks::<4>().0 {
                        let [r, g, b, a] = unpremul_rgba(*px);
                        out.extend_from_slice(&[b, g, r, a]);
                    }
                }
            }
        }
        Pixmap::from_bytes(w, h, format, out, limits)
    }
}

pub(crate) fn build_vello_pixmap_straight(
    mut rgba: Vec<u8>,
    width: u16,
    height: u16,
) -> Option<VelloPixmap> {
    let expected = usize::from(width) * usize::from(height) * 4;
    if rgba.len() != expected {
        return None;
    }
    // `VelloPixmap::from_parts` asserts `rgba.capacity() % 4 == 0` via `bytemuck`.
    if !rgba.capacity().is_multiple_of(4) {
        rgba.shrink_to_fit();
    }
    Some(VelloPixmap::from_parts(
        rgba,
        width,
        height,
        vello_cpu::PixelMetadata::new(ImageAlphaType::Alpha, true),
    ))
}

pub(crate) fn matrix_to_affine(m: Matrix) -> Affine {
    let clamp = |v: f32| {
        if v.is_finite() {
            f64::from(v).clamp(-MAX_COORD, MAX_COORD)
        } else {
            0.0
        }
    };
    Affine::new([
        clamp(m.a),
        clamp(m.b),
        clamp(m.c),
        clamp(m.d),
        clamp(m.e),
        clamp(m.f),
    ])
}

pub(crate) fn segs_to_bez_path(segs: &[Seg]) -> (BezPath, f64) {
    let mut path = BezPath::with_capacity(segs.len());
    let mut prev = kurbo::Point::ZERO;
    let mut extent = 0.0f64;
    let pt = |p: Point| {
        kurbo::Point::new(
            f64::from(p.x).clamp(-MAX_COORD, MAX_COORD),
            f64::from(p.y).clamp(-MAX_COORD, MAX_COORD),
        )
    };
    for &seg in segs {
        match seg {
            Seg::Move(p) => {
                let q = pt(p);
                prev = q;
                path.push(PathEl::MoveTo(q));
            }
            Seg::Line(p) => {
                let q = pt(p);
                extent += (q.x - prev.x).hypot(q.y - prev.y);
                prev = q;
                path.push(PathEl::LineTo(q));
            }
            Seg::Curve(a, b, p) => {
                let (qa, qb, qp) = (pt(a), pt(b), pt(p));
                extent += (qa.x - prev.x).hypot(qa.y - prev.y)
                    + (qb.x - qa.x).hypot(qb.y - qa.y)
                    + (qp.x - qb.x).hypot(qp.y - qb.y);
                prev = qp;
                path.push(PathEl::CurveTo(qa, qb, qp));
            }
            Seg::Close => path.push(PathEl::ClosePath),
        }
    }
    (path, extent)
}

pub(crate) fn outline_verbs_to_bez_path(
    verbs: &[OutlineVerb],
    glyph_to_target: Matrix,
    out: &mut BezPath,
) -> f64 {
    let mut prev = kurbo::Point::ZERO;
    let mut extent = 0.0f64;
    let map = |x: f32, y: f32| {
        let p = glyph_to_target.apply(Point::new(x, y));
        kurbo::Point::new(
            f64::from(p.x).clamp(-MAX_COORD, MAX_COORD),
            f64::from(p.y).clamp(-MAX_COORD, MAX_COORD),
        )
    };
    for &verb in verbs {
        match verb {
            OutlineVerb::MoveTo(x, y) => {
                let q = map(x, y);
                prev = q;
                out.push(PathEl::MoveTo(q));
            }
            OutlineVerb::LineTo(x, y) => {
                let q = map(x, y);
                extent += (q.x - prev.x).hypot(q.y - prev.y);
                prev = q;
                out.push(PathEl::LineTo(q));
            }
            OutlineVerb::QuadTo(cx, cy, x, y) => {
                let (qc, qp) = (map(cx, cy), map(x, y));
                extent += (qc.x - prev.x).hypot(qc.y - prev.y) + (qp.x - qc.x).hypot(qp.y - qc.y);
                prev = qp;
                out.push(PathEl::QuadTo(qc, qp));
            }
            OutlineVerb::CurveTo(cx0, cy0, cx1, cy1, x, y) => {
                let (q0, q1, qp) = (map(cx0, cy0), map(cx1, cy1), map(x, y));
                extent += (q0.x - prev.x).hypot(q0.y - prev.y)
                    + (q1.x - q0.x).hypot(q1.y - q0.y)
                    + (qp.x - q1.x).hypot(qp.y - q1.y);
                prev = qp;
                out.push(PathEl::CurveTo(q0, q1, qp));
            }
            OutlineVerb::Close => out.push(PathEl::ClosePath),
        }
    }
    extent
}

fn fill_rule_to_peniko(rule: FillRule) -> Fill {
    match rule {
        FillRule::NonZero => Fill::NonZero,
        FillRule::EvenOdd => Fill::EvenOdd,
    }
}

pub(crate) fn blend_to_peniko(mode: &BlendState) -> BlendMode {
    let mix = match mode {
        BlendState::Mode(b) => match b {
            Blend::Normal => Mix::Normal,
            Blend::Multiply => Mix::Multiply,
            Blend::Screen => Mix::Screen,
            Blend::Overlay => Mix::Overlay,
            Blend::Darken => Mix::Darken,
            Blend::Lighten => Mix::Lighten,
            Blend::ColorDodge => Mix::ColorDodge,
            Blend::ColorBurn => Mix::ColorBurn,
            Blend::HardLight => Mix::HardLight,
            Blend::SoftLight => Mix::SoftLight,
            Blend::Difference => Mix::Difference,
            Blend::Exclusion => Mix::Exclusion,
            Blend::Hue => Mix::Hue,
            Blend::Saturation => Mix::Saturation,
            Blend::Color => Mix::Color,
            Blend::Luminosity => Mix::Luminosity,
        },
        BlendState::Compatible | BlendState::Other(_) | _ => Mix::Normal,
    };
    BlendMode {
        mix,
        compose: Compose::SrcOver,
    }
}
