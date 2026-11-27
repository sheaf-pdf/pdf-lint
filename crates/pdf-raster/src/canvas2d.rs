//! Private 2D fine rasterizer & PDF §11 blend compositor wrapping `vello_cpu`.
//!
//! No `vello_cpu`, `kurbo`, or `peniko` type ever appears in `pdf-raster`'s
//! public API. Coordinates are sanitized and clamped before reaching `kurbo`
//! curve flatteners, clip/layer stacks are depth-tracked so pops cannot
//! underflow, and eager strip/flattening calls as well as final scene
//! rasterization run inside `catch_unwind` (note: under `panic = "abort"`
//! profiles, upstream dependency panics abort rather than unwinding into
//! [`Error::RasterizerPanic`]).

use std::borrow::Cow;
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
use crate::limits::{LimitKind, Limits};
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
    clip_depth: usize,
    layer_depth: usize,
    panicked: bool,
}

impl Canvas2d {
    pub(crate) fn new(width: u32, height: u32) -> Result<Self, Error> {
        if width == 0 || height == 0 {
            return Err(Error::InvalidViewport(
                "canvas width and height must be >= 1",
            ));
        }
        let w = u16::try_from(width).map_err(|_| Error::Limit(LimitKind::Dimensions))?;
        let h = u16::try_from(height).map_err(|_| Error::Limit(LimitKind::Dimensions))?;
        let settings = RenderSettings {
            level: simd_level(),
            num_threads: 0,
        };
        Ok(Self {
            ctx: RenderContext::new_with(w, h, settings),
            width: w,
            height: h,
            clip_depth: 0,
            layer_depth: 0,
            panicked: false,
        })
    }

    fn run_guarded(&mut self, f: impl FnOnce(&mut RenderContext)) -> bool {
        if self.panicked {
            return false;
        }
        let ctx = &mut self.ctx;
        if catch_unwind(AssertUnwindSafe(|| f(ctx))).is_err() {
            self.panicked = true;
            return false;
        }
        true
    }

    pub(crate) const fn width(&self) -> u16 {
        self.width
    }

    pub(crate) const fn height(&self) -> u16 {
        self.height
    }

    pub(crate) fn set_anti_alias(&mut self, enabled: bool) {
        if !self.panicked {
            self.ctx
                .set_aliasing_threshold(if enabled { None } else { Some(128) });
        }
    }

    pub(crate) fn set_transform(&mut self, m: Matrix) {
        if !self.panicked {
            self.ctx.set_transform(matrix_to_affine(m));
        }
    }

    pub(crate) fn set_fill_rule(&mut self, rule: FillRule) {
        if !self.panicked {
            self.ctx.set_fill_rule(fill_rule_to_peniko(rule));
        }
    }

    pub(crate) fn set_blend_mode(&mut self, mode: &BlendState) {
        if !self.panicked {
            self.ctx.set_blend_mode(blend_to_peniko(mode));
        }
    }

    pub(crate) fn set_solid_rgba8(&mut self, [r, g, b, a]: [u8; 4]) {
        if !self.panicked {
            self.ctx.reset_paint_transform();
            self.ctx.reset_tint();
            self.ctx
                .set_paint(AlphaColor::<Srgb>::from_rgba8(r, g, b, a));
        }
    }

    pub(crate) fn set_tiling_paint(&mut self, tile: Arc<VelloPixmap>, paint_transform: Matrix) {
        if !self.panicked {
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
    }

    pub(crate) fn set_shading_paint(&mut self, shading: Arc<VelloPixmap>, paint_transform: Matrix) {
        if !self.panicked {
            self.ctx.reset_tint();
            let brush = Image {
                image: ImageSource::Pixmap(shading),
                sampler: ImageSampler {
                    x_extend: Extend::Pad,
                    y_extend: Extend::Pad,
                    quality: ImageQuality::Medium,
                    alpha: 1.0,
                },
            };
            self.ctx.set_paint(brush);
            self.ctx
                .set_paint_transform(matrix_to_affine(paint_transform));
        }
    }

    /// Configures stroke geometry and dash array on `self`, returning `true`
    /// when a non-degenerate dash pattern exceeded `MAX_DASH_SEGMENTS` and was
    /// approximated as a solid stroke.
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
    ) -> bool {
        if self.panicked {
            return false;
        }
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

        let mut dense_dash = false;
        if !dash.is_solid() {
            let mut pattern: Vec<f64> = dash
                .array
                .iter()
                .take(64)
                .map(|&v| f64::from(v.max(0.0)).min(MAX_COORD))
                .collect();
            let cycle_sum: f64 = pattern.iter().sum();
            if cycle_sum > 1e-5 {
                if path_extent / cycle_sum <= MAX_DASH_SEGMENTS {
                    // Odd-length dash arrays repeat twice per cycle in PDF (§8.4.3.6).
                    let full_cycle = if pattern.len() % 2 == 1 {
                        let clone = pattern.clone();
                        pattern.extend_from_slice(&clone);
                        cycle_sum * 2.0
                    } else {
                        cycle_sum
                    };
                    let phase = f64::from(dash.phase).rem_euclid(full_cycle);
                    stroke = stroke.with_dashes(phase, pattern);
                } else {
                    dense_dash = true;
                }
            }
        }
        self.ctx.set_stroke(stroke);
        dense_dash
    }

    pub(crate) fn fill_bez_path(&mut self, path: &BezPath) {
        if !path.is_empty() {
            self.run_guarded(|ctx| ctx.fill_path(path));
        }
    }

    pub(crate) fn stroke_bez_path(&mut self, path: &BezPath) {
        if !path.is_empty() {
            self.run_guarded(|ctx| ctx.stroke_path(path));
        }
    }

    pub(crate) fn push_clip(&mut self, path: &BezPath, rule: FillRule, transform: Matrix) {
        if self.run_guarded(|ctx| {
            ctx.set_transform(matrix_to_affine(transform));
            ctx.set_fill_rule(fill_rule_to_peniko(rule));
            ctx.push_clip_path(path);
        }) {
            self.clip_depth += 1;
        }
    }

    pub(crate) fn pop_clip(&mut self) {
        if self.clip_depth > 0 {
            self.clip_depth -= 1;
            self.run_guarded(RenderContext::pop_clip);
        }
    }

    pub(crate) fn set_mask(&mut self, mask: Option<Mask>) {
        if !self.panicked {
            match mask {
                Some(m) => self.ctx.set_mask(m),
                None => self.ctx.reset_mask(),
            }
        }
    }

    pub(crate) fn push_group_layer(
        &mut self,
        clip_path: Option<(&BezPath, FillRule, Matrix)>,
        blend_mode: &BlendState,
        opacity: f32,
        mask: Option<Mask>,
    ) {
        let bm = blend_to_peniko(blend_mode);
        let op = opacity.clamp(0.0, 1.0);
        if self.run_guarded(|ctx| {
            let clip_bez = if let Some((path, rule, transform)) = clip_path {
                ctx.set_transform(matrix_to_affine(transform));
                ctx.set_fill_rule(fill_rule_to_peniko(rule));
                Some(path)
            } else {
                ctx.set_transform(Affine::IDENTITY);
                None
            };
            ctx.push_layer(clip_bez, Some(bm), Some(op), mask, None);
        }) {
            self.layer_depth += 1;
        }
    }

    pub(crate) fn pop_layer(&mut self) {
        if self.layer_depth > 0 {
            self.layer_depth -= 1;
            self.run_guarded(RenderContext::pop_layer);
        }
    }

    /// Draws an `RGBA8` image (`img_w × img_h`) placed onto PDF's `[0, 1] × [0, 1]`
    /// unit square (`y = 0` bottom, `y = 1` top) under `unit_to_device`.
    ///
    /// When `smooth_downscale` is `true` and the target device extent along X or
    /// Y is smaller than half the source dimension, iteratively box-filters the
    /// image by `2×` in premultiplied alpha so high-DPI scanned text and line
    /// art remain crisp at thumbnail scales instead of aliasing.
    ///
    /// Returns `false` if the final texture dimensions exceed `u16::MAX`
    /// (`65,535` px) and could not be drawn.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_rgba_image(
        &mut self,
        rgba: Cow<'_, [u8]>,
        img_w: u32,
        img_h: u32,
        unit_to_device: Matrix,
        interpolate: bool,
        smooth_downscale: bool,
        opacity: f32,
        blend: &BlendState,
    ) -> bool {
        if img_w == 0 || img_h == 0 {
            return true;
        }
        if !unit_to_device.is_finite() || unit_to_device.determinant() == 0.0 {
            return true;
        }

        let dev_w = unit_to_device.a.hypot(unit_to_device.b);
        let dev_h = unit_to_device.c.hypot(unit_to_device.d);

        let mut cur_w = img_w;
        let mut cur_h = img_h;
        let mut cur_bytes = rgba;
        let mut downsampled = false;

        if smooth_downscale && dev_w > 0.0 && dev_h > 0.0 {
            while ((cur_w as f32) > dev_w * 2.0 && cur_w >= 2)
                || ((cur_h as f32) > dev_h * 2.0 && cur_h >= 2)
            {
                let half_x = (cur_w as f32) > dev_w * 2.0 && cur_w >= 2;
                let half_y = (cur_h as f32) > dev_h * 2.0 && cur_h >= 2;
                let (next_w, next_h, next_buf) =
                    downsample_rgba_box(&cur_bytes, cur_w, cur_h, half_x, half_y);
                cur_w = next_w;
                cur_h = next_h;
                cur_bytes = Cow::Owned(next_buf);
                downsampled = true;
            }
        }

        let (Ok(w), Ok(h)) = (u16::try_from(cur_w), u16::try_from(cur_h)) else {
            return false;
        };
        if self.panicked {
            return true;
        }
        let Some(vello_pix) = build_vello_pixmap_straight(cur_bytes.into_owned(), w, h) else {
            return true;
        };

        // Image pixel space `[0..w] × [0..h]` (`y` down, row 0 = top) -> PDF unit
        // square `[0..1] × [0..1]` (`y` up, `y = 1` = top):
        // x_u = x_px / w, y_u = 1.0 - y_px / h.
        let img_to_unit = Matrix::new(
            1.0 / (cur_w as f32),
            0.0,
            0.0,
            -1.0 / (cur_h as f32),
            0.0,
            1.0,
        );
        let img_to_device = img_to_unit.then(unit_to_device);
        if !img_to_device.is_finite() || img_to_device.determinant() == 0.0 {
            return true;
        }

        let use_bilinear = interpolate || (downsampled && smooth_downscale);
        let brush = Image {
            image: ImageSource::Pixmap(Arc::new(vello_pix)),
            sampler: ImageSampler {
                x_extend: Extend::Pad,
                y_extend: Extend::Pad,
                quality: if use_bilinear {
                    ImageQuality::Medium
                } else {
                    ImageQuality::Low
                },
                alpha: opacity.clamp(0.0, 1.0),
            },
        };
        let bm = blend_to_peniko(blend);
        let xf = matrix_to_affine(img_to_device);
        let rect = kurbo::Rect::new(0.0, 0.0, f64::from(w), f64::from(h));
        self.run_guarded(move |ctx| {
            ctx.set_blend_mode(bm);
            ctx.reset_paint_transform();
            ctx.reset_tint();
            ctx.set_transform(xf);
            ctx.set_paint(brush);
            ctx.fill_rect(&rect);
        });
        true
    }

    /// Rasterizes into an internal `VelloPixmap` (premultiplied `RGBA8`), used
    /// for offscreen soft-mask groups and tiling pattern cells.
    pub(crate) fn render_to_vello_pixmap(
        mut self,
        background: Option<[u8; 4]>,
    ) -> Result<VelloPixmap, Error> {
        if self.panicked {
            return Err(Error::RasterizerPanic);
        }
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
                for px in premul.as_chunks::<4>().0 {
                    let [r, g, b, a] = unpremul_rgba(*px);
                    out.extend_from_slice(&[b, g, r, a]);
                }
            }
        }
        Pixmap::from_bytes(w, h, format, out, limits)
    }
}

/// Box-filter `2×` step in premultiplied straight-alpha `RGBA8` along `half_x`
/// and/or `half_y`, handling odd trailing columns/rows cleanly without haloing
/// transparent pixels.
fn downsample_rgba_box(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    half_x: bool,
    half_y: bool,
) -> (u32, u32, Vec<u8>) {
    let dst_w = if half_x { (src_w / 2).max(1) } else { src_w };
    let dst_h = if half_y { (src_h / 2).max(1) } else { src_h };
    let sw = src_w as usize;
    let sh = src_h as usize;
    let dw = dst_w as usize;
    let dh = dst_h as usize;
    let mut out = vec![0_u8; dw * dh * 4];

    for dy in 0..dh {
        let sy0 = if half_y { dy * 2 } else { dy };
        let sy1 = if half_y {
            if dy + 1 == dh {
                sh
            } else {
                (dy * 2 + 2).min(sh)
            }
        } else {
            sy0 + 1
        };
        for dx in 0..dw {
            let sx0 = if half_x { dx * 2 } else { dx };
            let sx1 = if half_x {
                if dx + 1 == dw {
                    sw
                } else {
                    (dx * 2 + 2).min(sw)
                }
            } else {
                sx0 + 1
            };

            let mut sum_pr = 0_u32;
            let mut sum_pg = 0_u32;
            let mut sum_pb = 0_u32;
            let mut sum_a = 0_u32;
            let mut count = 0_u32;

            for sy in sy0..sy1 {
                let row_base = sy * sw * 4;
                for sx in sx0..sx1 {
                    let idx = row_base + sx * 4;
                    let r = u32::from(src[idx]);
                    let g = u32::from(src[idx + 1]);
                    let b = u32::from(src[idx + 2]);
                    let a = u32::from(src[idx + 3]);
                    sum_pr += r * a;
                    sum_pg += g * a;
                    sum_pb += b * a;
                    sum_a += a;
                    count += 1;
                }
            }

            let dst_idx = (dy * dw + dx) * 4;
            if sum_a == 0 || count == 0 {
                out[dst_idx..dst_idx + 4].copy_from_slice(&[0, 0, 0, 0]);
            } else {
                let out_a = ((sum_a + count / 2) / count).min(255) as u8;
                let un = |sum_pc: u32| -> u8 { ((sum_pc + sum_a / 2) / sum_a).min(255) as u8 };
                out[dst_idx] = un(sum_pr);
                out[dst_idx + 1] = un(sum_pg);
                out[dst_idx + 2] = un(sum_pb);
                out[dst_idx + 3] = out_a;
            }
        }
    }
    (dst_w, dst_h, out)
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
    catch_unwind(AssertUnwindSafe(move || {
        VelloPixmap::from_parts(
            rgba,
            width,
            height,
            vello_cpu::PixelMetadata::new(ImageAlphaType::Alpha, true),
        )
    }))
    .ok()
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
