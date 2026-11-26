//! Color evaluation, `PatternType 1` tiling patterns, `PatternType 2` shading
//! patterns, and ISO 32000-2 §8.7.4 shadings (`ShadingType 1..=5`).

use std::sync::Arc;

use pdf_color::{EvalOptions, Function, PreparedSpace, Space};
use pdf_gfx::{ActiveColorSpace, Matrix, Point, Rect};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Dict, Object, ObjectId, Resolve, Streams};
use vello_cpu::Pixmap as VelloPixmap;

use crate::canvas2d::build_vello_pixmap_straight;
use crate::error::Error;
use crate::fault::Fault;
use crate::opts::RenderOpts;

/// Evaluated paint ready to configure on [`crate::canvas2d::Canvas2d`].
pub(crate) enum ResolvedPaint {
    /// Solid straight-alpha `[R, G, B, A]`.
    Solid([u8; 4]),
    /// Repeated or positioned bitmap (`tile`, `paint_transform` in device space).
    Tiled {
        pixmap: Arc<VelloPixmap>,
        transform: Matrix,
    },
    /// Clipped bitmap positioned in device space (`x0`, `y0` offset).
    Shading {
        pixmap: Arc<VelloPixmap>,
        transform: Matrix,
    },
    /// Invisible paint (`/Separation /None` or empty/invalid pattern).
    None,
}

struct ColorSink<'a, S: ?Sized>(&'a mut S);

impl<S: Sink<Fault> + ?Sized> Sink<pdf_color::Fault> for ColorSink<'_, S> {
    fn wants(&self) -> bool {
        self.0.wants()
    }

    fn fault(&mut self, at: Place, fault: pdf_color::Fault) {
        self.0.fault(at, Fault::Color(fault));
    }
}

/// Resolves an entry inside `/Resources/<category>/<name>`, falling back to
/// `page_resources` when `resources` omits `<category>`, matching PDFium's
/// resource inheritance.
pub(crate) fn lookup_resource_entry<'o, R: Resolve<'o> + Copy>(
    objects: R,
    resources: Option<&'o Object>,
    page_resources: Option<&'o Object>,
    category: &[u8],
    name: &[u8],
) -> Option<&'o Object> {
    let cat_in = |res: &'o Object| -> Option<&'o Dict> {
        let res_dict = match res.as_reference() {
            Some(id) => objects.resolve(id)?.as_dict()?,
            None => res.as_dict()?,
        };
        let cat_val = res_dict.get(category)?;
        match cat_val.as_reference() {
            Some(id) => objects.resolve(id)?.as_dict(),
            None => cat_val.as_dict(),
        }
    };
    if let Some(res) = resources
        && let Some(cat) = cat_in(res)
    {
        return cat.get(name);
    }
    if let Some(page) = page_resources
        && let Some(cat) = cat_in(page)
    {
        return cat.get(name);
    }
    None
}

/// Evaluates `space` + `components` + `alpha` into straight-alpha `[R, G, B, A]`,
/// returning `None` for `/Separation /None` (transparent no-op paint).
#[allow(clippy::too_many_arguments)]
pub(crate) fn eval_solid_color<'o, 'r, O, S>(
    objects: O,
    active_space: &ActiveColorSpace,
    components: &[f32],
    alpha: f32,
    resources: Option<&'r Object>,
    page_resources: Option<&'r Object>,
    output_intent_icc: Option<&[u8]>,
    opts: &RenderOpts,
    sink: &mut S,
) -> Result<Option<[u8; 4]>, Error>
where
    'o: 'r,
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let objects = crate::engine::Narrow::new(objects);
    let alpha_u8 = (alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let spaces = pdf_color::Spaces::read(objects, resources.or(page_resources))?;
    let mut eval_opts = EvalOptions::from_spaces(opts.rules, spaces);
    eval_opts.output_intent_profile = output_intent_icc;

    // Fast path for device spaces when `Rules::Pdfium` is active or no
    // `/Default*` override applies.
    let device_obj;
    let space_obj: Option<&Object> = match active_space {
        ActiveColorSpace::DeviceGray => {
            device_obj = Object::name("DeviceGray");
            Some(&device_obj)
        }
        ActiveColorSpace::DeviceRgb => {
            device_obj = Object::name("DeviceRGB");
            Some(&device_obj)
        }
        ActiveColorSpace::DeviceCmyk => {
            device_obj = Object::name("DeviceCMYK");
            Some(&device_obj)
        }
        ActiveColorSpace::Named(name) => {
            lookup_resource_entry(objects, resources, page_resources, b"ColorSpace", name)
        }
        ActiveColorSpace::Pattern | _ => None,
    };

    let Some(obj) = space_obj else {
        return Ok(Some([0, 0, 0, alpha_u8]));
    };

    let mut csink = ColorSink(sink);
    let Some(space) = Space::read(objects, obj, &opts.limits.color, &mut csink)? else {
        return Ok(Some([0, 0, 0, alpha_u8]));
    };

    // Uncolored `/Pattern` space `[/Pattern /BaseSpace]` evaluates its underlying
    // base space when painting an uncolored tiling pattern.
    let target_space = match &space {
        Space::Pattern(Some(base)) => base.as_ref(),
        other => other,
    };

    let Some(prepared) = PreparedSpace::from_space(
        objects,
        target_space,
        &eval_opts,
        &opts.limits.color,
        &mut csink,
    )?
    else {
        return Ok(Some([0, 0, 0, alpha_u8]));
    };

    Ok(prepared
        .eval_with_rules(components, opts.rules, &opts.limits.color)
        .and_then(pdf_color::Paint::srgb8)
        .map(|[r, g, b]| [r, g, b, alpha_u8]))
}

/// Evaluates a shading dictionary or stream (`sh` or `PatternType 2`) over the
/// integer device-pixel rectangle `[px0..px1] × [py0..py1]`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn evaluate_shading_rect<'o, 'r, O, S>(
    objects: O,
    shading_obj: &'r Object,
    shading_to_device: Matrix,
    device_bounds: (u32, u32, u32, u32),
    include_background: bool,
    alpha: f32,
    resources: Option<&'r Object>,
    page_resources: Option<&'r Object>,
    output_intent_icc: Option<&[u8]>,
    opts: &RenderOpts,
    sink: &mut S,
) -> Result<Option<(Arc<VelloPixmap>, Matrix)>, Error>
where
    'o: 'r,
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let objects = crate::engine::Narrow::new(objects);
    let (x0, y0, x1, y1) = device_bounds;
    if x1 <= x0 || y1 <= y0 {
        return Ok(None);
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let (Ok(w16), Ok(h16)) = (u16::try_from(w), u16::try_from(h)) else {
        return Ok(None);
    };
    let total_px = u64::from(w) * u64::from(h);
    if total_px > u64::from(opts.limits.max_shading_samples) {
        return Err(Error::Limit(crate::limits::LimitKind::ShadingSamples));
    }

    let resolved = match shading_obj.as_reference() {
        Some(id) => objects.try_resolve(id)?,
        None => Some(shading_obj),
    };
    let (dict, stream_id) = match resolved {
        Some(obj) if obj.as_dict().is_some() => {
            (obj.as_dict().unwrap(), shading_obj.as_reference())
        }
        Some(obj) if obj.as_stream().is_some() => {
            (&obj.as_stream().unwrap().dict, shading_obj.as_reference())
        }
        _ => {
            sink.fault(
                Place::new(shading_obj.as_reference(), shading_obj.at()),
                Fault::InvalidShading,
            );
            return Ok(None);
        }
    };

    let Some(device_to_shading) = shading_to_device.inverse() else {
        return Ok(None);
    };

    let shading_type = dict_int(objects, dict, b"ShadingType").unwrap_or(0);
    let Some(cs_raw) = dict.get(b"ColorSpace") else {
        sink.fault(Place::new(stream_id, None), Fault::InvalidShading);
        return Ok(None);
    };
    let cs_resolved = match cs_raw.as_reference() {
        Some(id) => objects.try_resolve(id)?,
        None => Some(cs_raw),
    };
    let cs_obj = match cs_resolved.and_then(Object::as_name) {
        Some(name) if !matches!(name, b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK") => {
            lookup_resource_entry(objects, resources, page_resources, b"ColorSpace", name)
                .unwrap_or(cs_raw)
        }
        _ => cs_raw,
    };

    let spaces = pdf_color::Spaces::read(objects, resources.or(page_resources))?;
    let mut eval_opts = EvalOptions::from_spaces(opts.rules, spaces);
    eval_opts.output_intent_profile = output_intent_icc;

    let mut csink = ColorSink(sink);
    let Some(space) = Space::read(objects, cs_obj, &opts.limits.color, &mut csink)? else {
        return Ok(None);
    };
    let Some(prepared) =
        PreparedSpace::from_space(objects, &space, &eval_opts, &opts.limits.color, &mut csink)?
    else {
        return Ok(None);
    };

    let alpha_u8 = (alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let bg_rgba = if include_background {
        dict_float_vec(objects, dict, b"Background")
            .and_then(|vals| {
                prepared
                    .eval_with_rules(&vals, opts.rules, &opts.limits.color)
                    .and_then(pdf_color::Paint::srgb8)
            })
            .map(|[r, g, b]| [r, g, b, alpha_u8])
    } else {
        None
    };

    let shading_bbox =
        dict_float4(objects, dict, b"BBox").map(|[a, b, c, d]| Rect::new(a, b, c, d));

    let mut rgba = vec![0u8; (w as usize) * (h as usize) * 4];
    if let Some(bg) = bg_rgba {
        for px in rgba.as_chunks_mut::<4>().0 {
            *px = bg;
        }
    }

    match shading_type {
        1 => eval_type1_shading(
            objects,
            dict,
            &prepared,
            device_to_shading,
            shading_bbox,
            (x0, y0, w, h),
            alpha_u8,
            opts,
            &mut csink,
            &mut rgba,
        )?,
        2 => eval_type2_axial(
            objects,
            dict,
            &prepared,
            device_to_shading,
            shading_bbox,
            (x0, y0, w, h),
            alpha_u8,
            opts,
            &mut csink,
            &mut rgba,
        )?,
        3 => eval_type3_radial(
            objects,
            dict,
            &prepared,
            device_to_shading,
            shading_bbox,
            (x0, y0, w, h),
            alpha_u8,
            opts,
            &mut csink,
            &mut rgba,
        )?,
        4 | 5 => {
            if let Some(sid) = stream_id {
                eval_mesh_shading(
                    objects,
                    sid,
                    dict,
                    shading_type,
                    &prepared,
                    shading_to_device,
                    shading_bbox,
                    (x0, y0, w, h),
                    alpha_u8,
                    opts,
                    &mut csink,
                    &mut rgba,
                )?;
            }
        }
        _ => {
            sink.fault(Place::new(stream_id, None), Fault::InvalidShading);
            return Ok(None);
        }
    }

    let Some(pix) = build_vello_pixmap_straight(rgba, w16, h16) else {
        return Ok(None);
    };
    let transform = Matrix::translate(x0 as f32, y0 as f32);
    Ok(Some((Arc::new(pix), transform)))
}

/// Reads `/Function` on a shading dictionary: either a single function (`m -> n`)
/// or an array of `n` 1-output functions (`m -> 1` each).
fn read_shading_functions<'o, O, S>(
    objects: O,
    dict: &'o Dict,
    rules: pdf_objects::Rules,
    limits: &pdf_color::Limits,
    sink: &mut ColorSink<'_, S>,
) -> Result<Option<Vec<Function>>, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let Some(fn_raw) = dict.get(b"Function") else {
        return Ok(None);
    };
    let resolved = match fn_raw.as_reference() {
        Some(id) => objects.try_resolve(id)?,
        None => Some(fn_raw),
    };
    if let Some(arr) = resolved.and_then(Object::as_array) {
        let mut funcs = Vec::with_capacity(arr.len().min(16));
        for item in arr.iter().take(16) {
            let Some(f) = Function::read(objects, item, rules, limits, sink)? else {
                return Ok(None);
            };
            funcs.push(f);
        }
        Ok((!funcs.is_empty()).then_some(funcs))
    } else {
        Ok(Function::read(objects, fn_raw, rules, limits, sink)?.map(|f| vec![f]))
    }
}

fn eval_functions(
    funcs: &[Function],
    inputs: &[f32],
    limits: &pdf_color::Limits,
    out: &mut [f32; 16],
) -> usize {
    if funcs.len() == 1 {
        let n = (funcs[0].outputs() as usize).min(16);
        funcs[0].eval(inputs, &mut out[..n], limits);
        n
    } else {
        let n = funcs.len().min(16);
        for (i, f) in funcs.iter().take(n).enumerate() {
            let mut single = [0.0f32; 1];
            f.eval(inputs, &mut single, limits);
            out[i] = single[0];
        }
        n
    }
}

#[allow(clippy::too_many_arguments)]
fn eval_type1_shading<'o, O, S>(
    objects: O,
    dict: &'o Dict,
    prepared: &PreparedSpace,
    device_to_shading: Matrix,
    shading_bbox: Option<Rect>,
    (x0, y0, w, h): (u32, u32, u32, u32),
    alpha_u8: u8,
    opts: &RenderOpts,
    sink: &mut ColorSink<'_, S>,
    rgba: &mut [u8],
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let Some(funcs) = read_shading_functions(objects, dict, opts.rules, &opts.limits.color, sink)?
    else {
        return Ok(());
    };
    let domain = dict_float4(objects, dict, b"Domain").unwrap_or([0.0, 1.0, 0.0, 1.0]);
    let dom_to_sh = dict_matrix(objects, dict, b"Matrix").unwrap_or(Matrix::IDENTITY);
    let Some(sh_to_dom) = dom_to_sh.inverse() else {
        return Ok(());
    };
    let (xmin, xmax) = (domain[0].min(domain[1]), domain[0].max(domain[1]));
    let (ymin, ymax) = (domain[2].min(domain[3]), domain[2].max(domain[3]));

    let mut comps = [0.0f32; 16];
    for row in 0..h {
        let py = (y0 + row) as f32 + 0.5;
        for col in 0..w {
            let px = (x0 + col) as f32 + 0.5;
            let p_sh = device_to_shading.apply(Point::new(px, py));
            if let Some(bb) = shading_bbox
                && !bb.contains(p_sh)
            {
                continue;
            }
            let p_dom = sh_to_dom.apply(p_sh);
            if p_dom.x < xmin || p_dom.x > xmax || p_dom.y < ymin || p_dom.y > ymax {
                continue;
            }
            let n = eval_functions(&funcs, &[p_dom.x, p_dom.y], &opts.limits.color, &mut comps);
            if let Some([r, g, b]) = prepared
                .eval_with_rules(&comps[..n], opts.rules, &opts.limits.color)
                .and_then(pdf_color::Paint::srgb8)
            {
                let idx = ((row as usize) * (w as usize) + (col as usize)) * 4;
                rgba[idx..idx + 4].copy_from_slice(&[r, g, b, alpha_u8]);
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn eval_type2_axial<'o, O, S>(
    objects: O,
    dict: &'o Dict,
    prepared: &PreparedSpace,
    device_to_shading: Matrix,
    shading_bbox: Option<Rect>,
    (x0, y0, w, h): (u32, u32, u32, u32),
    alpha_u8: u8,
    opts: &RenderOpts,
    sink: &mut ColorSink<'_, S>,
    rgba: &mut [u8],
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let Some([ax0, ay0, ax1, ay1]) = dict_float4(objects, dict, b"Coords") else {
        return Ok(());
    };
    let Some(funcs) = read_shading_functions(objects, dict, opts.rules, &opts.limits.color, sink)?
    else {
        return Ok(());
    };
    let [t0, t1] = dict_float2(objects, dict, b"Domain").unwrap_or([0.0, 1.0]);
    let [ext0, ext1] = dict_bool2(objects, dict, b"Extend").unwrap_or([false, false]);

    let (dx, dy) = (ax1 - ax0, ay1 - ay0);
    let denom = dx * dx + dy * dy;
    if denom <= 1e-12 {
        return Ok(());
    }

    // Precompute a 512-entry 1D LUT over `s in [0, 1]` so per-pixel shading is O(1)
    // even when `/Function` is a PostScript Type 4 calculator or ICC space!
    const LUT_LEN: usize = 512;
    let mut lut = [[0u8; 4]; LUT_LEN];
    let mut lut_valid = [false; LUT_LEN];
    let mut comps = [0.0f32; 16];
    for i in 0..LUT_LEN {
        let s = (i as f32) / ((LUT_LEN - 1) as f32);
        let t = t0 + (t1 - t0) * s;
        let n = eval_functions(&funcs, &[t], &opts.limits.color, &mut comps);
        if let Some([r, g, b]) = prepared
            .eval_with_rules(&comps[..n], opts.rules, &opts.limits.color)
            .and_then(pdf_color::Paint::srgb8)
        {
            lut[i] = [r, g, b, alpha_u8];
            lut_valid[i] = true;
        }
    }

    for row in 0..h {
        let py = (y0 + row) as f32 + 0.5;
        for col in 0..w {
            let px = (x0 + col) as f32 + 0.5;
            let p_sh = device_to_shading.apply(Point::new(px, py));
            if let Some(bb) = shading_bbox
                && !bb.contains(p_sh)
            {
                continue;
            }
            let raw_s = ((p_sh.x - ax0) * dx + (p_sh.y - ay0) * dy) / denom;
            let s = if raw_s < 0.0 {
                if !ext0 {
                    continue;
                }
                0.0
            } else if raw_s > 1.0 {
                if !ext1 {
                    continue;
                }
                1.0
            } else {
                raw_s
            };
            let idx_lut = ((s * ((LUT_LEN - 1) as f32)) + 0.5) as usize;
            let idx_lut = idx_lut.min(LUT_LEN - 1);
            if lut_valid[idx_lut] {
                let idx = ((row as usize) * (w as usize) + (col as usize)) * 4;
                rgba[idx..idx + 4].copy_from_slice(&lut[idx_lut]);
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn eval_type3_radial<'o, O, S>(
    objects: O,
    dict: &'o Dict,
    prepared: &PreparedSpace,
    device_to_shading: Matrix,
    shading_bbox: Option<Rect>,
    (x0, y0, w, h): (u32, u32, u32, u32),
    alpha_u8: u8,
    opts: &RenderOpts,
    sink: &mut ColorSink<'_, S>,
    rgba: &mut [u8],
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let Some([cx0, cy0, r0, cx1, cy1, r1]) = dict_float6(objects, dict, b"Coords") else {
        return Ok(());
    };
    if r0 < 0.0 || r1 < 0.0 {
        return Ok(());
    }
    let Some(funcs) = read_shading_functions(objects, dict, opts.rules, &opts.limits.color, sink)?
    else {
        return Ok(());
    };
    let [t0, t1] = dict_float2(objects, dict, b"Domain").unwrap_or([0.0, 1.0]);
    let [ext0, ext1] = dict_bool2(objects, dict, b"Extend").unwrap_or([false, false]);

    const LUT_LEN: usize = 512;
    let mut lut = [[0u8; 4]; LUT_LEN];
    let mut lut_valid = [false; LUT_LEN];
    let mut comps = [0.0f32; 16];
    for i in 0..LUT_LEN {
        let s = (i as f32) / ((LUT_LEN - 1) as f32);
        let t = t0 + (t1 - t0) * s;
        let n = eval_functions(&funcs, &[t], &opts.limits.color, &mut comps);
        if let Some([r, g, b]) = prepared
            .eval_with_rules(&comps[..n], opts.rules, &opts.limits.color)
            .and_then(pdf_color::Paint::srgb8)
        {
            lut[i] = [r, g, b, alpha_u8];
            lut_valid[i] = true;
        }
    }

    let (dx, dy, dr) = (
        f64::from(cx1 - cx0),
        f64::from(cy1 - cy0),
        f64::from(r1 - r0),
    );
    let a_coef = dx * dx + dy * dy - dr * dr;
    let (cx0_d, cy0_d, r0_d) = (f64::from(cx0), f64::from(cy0), f64::from(r0));

    let valid_s = |s: f64| -> Option<f32> {
        if !s.is_finite() || r0_d + s * dr < 0.0 {
            return None;
        }
        if s < 0.0 {
            ext0.then_some(0.0)
        } else if s > 1.0 {
            ext1.then_some(1.0)
        } else {
            Some(s as f32)
        }
    };

    for row in 0..h {
        let py = (y0 + row) as f32 + 0.5;
        for col in 0..w {
            let px = (x0 + col) as f32 + 0.5;
            let p_sh = device_to_shading.apply(Point::new(px, py));
            if let Some(bb) = shading_bbox
                && !bb.contains(p_sh)
            {
                continue;
            }
            let (vx, vy) = (f64::from(p_sh.x) - cx0_d, f64::from(p_sh.y) - cy0_d);
            let b_half = vx * dx + vy * dy + r0_d * dr;
            let c_coef = vx * vx + vy * vy - r0_d * r0_d;

            // ISO 32000-2 §8.7.4.5.4: solve `a * s^2 - 2 * b_half * s + c = 0`,
            // choosing the larger valid root `s` (`s` painted front-to-back).
            let chosen = if a_coef.abs() < 1e-11 {
                if b_half.abs() < 1e-11 {
                    None
                } else {
                    valid_s(c_coef / (2.0 * b_half))
                }
            } else {
                let disc = b_half * b_half - a_coef * c_coef;
                if disc < 0.0 {
                    None
                } else {
                    let root = disc.sqrt();
                    let s_hi = (b_half + root) / a_coef;
                    let s_lo = (b_half - root) / a_coef;
                    let (first, second) = if s_hi >= s_lo {
                        (s_hi, s_lo)
                    } else {
                        (s_lo, s_hi)
                    };
                    valid_s(first).or_else(|| valid_s(second))
                }
            };

            if let Some(s) = chosen {
                let idx_lut = ((s * ((LUT_LEN - 1) as f32)) + 0.5) as usize;
                let idx_lut = idx_lut.min(LUT_LEN - 1);
                if lut_valid[idx_lut] {
                    let idx = ((row as usize) * (w as usize) + (col as usize)) * 4;
                    rgba[idx..idx + 4].copy_from_slice(&lut[idx_lut]);
                }
            }
        }
    }
    Ok(())
}

/// Evaluates `ShadingType 4` (Free-form Gouraud triangle mesh) and
/// `ShadingType 5` (Lattice-form Gouraud triangle mesh).
#[allow(clippy::too_many_arguments)]
fn eval_mesh_shading<'o, O, S>(
    objects: O,
    stream_id: ObjectId,
    dict: &'o Dict,
    shading_type: i64,
    prepared: &PreparedSpace,
    shading_to_device: Matrix,
    shading_bbox: Option<Rect>,
    (x0, y0, w, h): (u32, u32, u32, u32),
    alpha_u8: u8,
    opts: &RenderOpts,
    sink: &mut ColorSink<'_, S>,
    rgba: &mut [u8],
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let bpc_coord = dict_int(objects, dict, b"BitsPerCoordinate").unwrap_or(0);
    let bpc_comp = dict_int(objects, dict, b"BitsPerComponent").unwrap_or(0);
    if !matches!(bpc_coord, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32)
        || !matches!(bpc_comp, 1 | 2 | 4 | 8 | 12 | 16)
    {
        return Ok(());
    }
    let funcs = read_shading_functions(objects, dict, opts.rules, &opts.limits.color, sink)?;
    let n_vals = if funcs.is_some() {
        1usize
    } else {
        (prepared.components() as usize).clamp(1, 16)
    };
    let Some(decode_arr) = dict_float_vec(objects, dict, b"Decode") else {
        return Ok(());
    };
    if decode_arr.len() < 4 + n_vals * 2 {
        return Ok(());
    }
    let data = match objects.decoded(stream_id, opts.limits.color.stream_bytes) {
        Ok(b) => b,
        Err(e)
            if matches!(
                e.why,
                pdf_objects::Why::Damaged | pdf_objects::Why::Filter | pdf_objects::Why::Limit
            ) =>
        {
            return Ok(());
        }
        Err(e) => return Err(Error::Unreadable(e)),
    };

    let mut reader = BitReader::new(&data);
    let eval_vertex_rgb = |vals: &[f32]| -> [f32; 3] {
        let mut comps = [0.0f32; 16];
        let slice = if let Some(ref fs) = funcs {
            let n = eval_functions(fs, &vals[..1], &opts.limits.color, &mut comps);
            &comps[..n]
        } else {
            vals
        };
        prepared
            .eval_with_rules(slice, opts.rules, &opts.limits.color)
            .and_then(pdf_color::Paint::srgb8)
            .map(|[r, g, b]| [f32::from(r), f32::from(g), f32::from(b)])
            .unwrap_or([0.0, 0.0, 0.0])
    };

    #[derive(Clone, Copy)]
    struct MeshVertex {
        dev_pt: Point,
        rgb: [f32; 3],
    }

    let read_vertex = |r: &mut BitReader<'_>| -> Option<MeshVertex> {
        let ux = r.read_bits(bpc_coord as u8)?;
        let uy = r.read_bits(bpc_coord as u8)?;
        let x = map_bits(ux, bpc_coord as u8, decode_arr[0], decode_arr[1]);
        let y = map_bits(uy, bpc_coord as u8, decode_arr[2], decode_arr[3]);
        let mut vals = [0.0f32; 16];
        for (i, slot) in vals.iter_mut().take(n_vals).enumerate() {
            let uc = r.read_bits(bpc_comp as u8)?;
            *slot = map_bits(
                uc,
                bpc_comp as u8,
                decode_arr[4 + i * 2],
                decode_arr[4 + i * 2 + 1],
            );
        }
        let sh_pt = Point::new(x, y);
        Some(MeshVertex {
            dev_pt: shading_to_device.apply(sh_pt),
            rgb: eval_vertex_rgb(&vals[..n_vals]),
        })
    };

    let mut triangles: Vec<[MeshVertex; 3]> = Vec::new();
    let max_tris = (opts.limits.max_shading_samples as usize).min(65_536);

    if shading_type == 4 {
        let bp_flag = dict_int(objects, dict, b"BitsPerFlag").unwrap_or(0);
        if !matches!(bp_flag, 2 | 4 | 8) {
            return Ok(());
        }
        let mut prev_tri: Option<[MeshVertex; 3]> = None;
        while triangles.len() < max_tris {
            let Some(flag) = reader.read_bits(bp_flag as u8) else {
                break;
            };
            let tri = match flag & 3 {
                0 => {
                    let (Some(v0), Some(v1), Some(v2)) = (
                        read_vertex(&mut reader),
                        read_vertex(&mut reader),
                        read_vertex(&mut reader),
                    ) else {
                        break;
                    };
                    [v0, v1, v2]
                }
                1 => {
                    let (Some([_, vb, vc]), Some(vn)) = (prev_tri, read_vertex(&mut reader)) else {
                        break;
                    };
                    [vb, vc, vn]
                }
                2 => {
                    let (Some([va, _, vc]), Some(vn)) = (prev_tri, read_vertex(&mut reader)) else {
                        break;
                    };
                    [va, vc, vn]
                }
                _ => break,
            };
            prev_tri = Some(tri);
            triangles.push(tri);
        }
    } else {
        let per_row = dict_int(objects, dict, b"VerticesPerRow").unwrap_or(0);
        if !(2..=4096).contains(&per_row) {
            return Ok(());
        }
        let cols = per_row as usize;
        let mut prev_row: Vec<MeshVertex> = Vec::with_capacity(cols);
        while triangles.len() < max_tris {
            let mut row = Vec::with_capacity(cols);
            for _ in 0..cols {
                if let Some(v) = read_vertex(&mut reader) {
                    row.push(v);
                } else {
                    break;
                }
            }
            if row.len() < cols {
                break;
            }
            if prev_row.len() == cols {
                for i in 0..(cols - 1) {
                    if triangles.len() + 2 > max_tris {
                        break;
                    }
                    triangles.push([prev_row[i], prev_row[i + 1], row[i]]);
                    triangles.push([prev_row[i + 1], row[i + 1], row[i]]);
                }
            }
            prev_row = row;
        }
    }

    let device_to_sh = shading_to_device.inverse();
    for [v0, v1, v2] in triangles {
        let min_x = v0
            .dev_pt
            .x
            .min(v1.dev_pt.x)
            .min(v2.dev_pt.x)
            .floor()
            .max(x0 as f32) as u32;
        let max_x = v0
            .dev_pt
            .x
            .max(v1.dev_pt.x)
            .max(v2.dev_pt.x)
            .ceil()
            .min((x0 + w) as f32) as u32;
        let min_y = v0
            .dev_pt
            .y
            .min(v1.dev_pt.y)
            .min(v2.dev_pt.y)
            .floor()
            .max(y0 as f32) as u32;
        let max_y = v0
            .dev_pt
            .y
            .max(v1.dev_pt.y)
            .max(v2.dev_pt.y)
            .ceil()
            .min((y0 + h) as f32) as u32;
        if min_x >= max_x || min_y >= max_y {
            continue;
        }
        let area = edge_fn(v0.dev_pt, v1.dev_pt, v2.dev_pt);
        if area.abs() < 1e-6 {
            continue;
        }
        let inv_area = 1.0 / area;
        for py_u in min_y..max_y {
            let py = py_u as f32 + 0.5;
            for px_u in min_x..max_x {
                let px = px_u as f32 + 0.5;
                let p = Point::new(px, py);
                let w0 = edge_fn(v1.dev_pt, v2.dev_pt, p) * inv_area;
                let w1 = edge_fn(v2.dev_pt, v0.dev_pt, p) * inv_area;
                let w2 = 1.0 - w0 - w1;
                if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                    continue;
                }
                if let (Some(bb), Some(inv)) = (shading_bbox, device_to_sh)
                    && !bb.contains(inv.apply(p))
                {
                    continue;
                }
                let r = (w0 * v0.rgb[0] + w1 * v1.rgb[0] + w2 * v2.rgb[0])
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let g = (w0 * v0.rgb[1] + w1 * v1.rgb[1] + w2 * v2.rgb[1])
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let b = (w0 * v0.rgb[2] + w1 * v1.rgb[2] + w2 * v2.rgb[2])
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let idx = (((py_u - y0) as usize) * (w as usize) + ((px_u - x0) as usize)) * 4;
                rgba[idx..idx + 4].copy_from_slice(&[r, g, b, alpha_u8]);
            }
        }
    }
    Ok(())
}

fn edge_fn(a: Point, b: Point, c: Point) -> f32 {
    (c.x - a.x) * (b.y - a.y) - (c.y - a.y) * (b.x - a.x)
}

fn map_bits(raw: u32, bits: u8, dmin: f32, dmax: f32) -> f32 {
    let max_val = if bits >= 32 {
        f64::from(u32::MAX)
    } else {
        f64::from((1u32 << bits) - 1)
    };
    let t = (f64::from(raw) / max_val) as f32;
    dmin + t * (dmax - dmin)
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    const fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn read_bits(&mut self, count: u8) -> Option<u32> {
        let n = usize::from(count);
        if self.bit_pos.checked_add(n)? > self.data.len().checked_mul(8)? {
            return None;
        }
        let mut out = 0u32;
        for _ in 0..n {
            let byte = self.data[self.bit_pos >> 3];
            let bit = (byte >> (7 - (self.bit_pos & 7))) & 1;
            out = (out << 1) | u32::from(bit);
            self.bit_pos += 1;
        }
        Some(out)
    }
}

pub(crate) fn dict_int<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
) -> Option<i64> {
    let v = dict.get(key)?;
    let resolved = match v.as_reference() {
        Some(id) => objects.resolve(id)?,
        None => v,
    };
    resolved.as_integer()
}

pub(crate) fn dict_float<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
) -> Option<f32> {
    let v = dict.get(key)?;
    let resolved = match v.as_reference() {
        Some(id) => objects.resolve(id)?,
        None => v,
    };
    let n = resolved.as_number()? as f32;
    n.is_finite().then_some(n)
}

pub(crate) fn dict_float_vec<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
) -> Option<Vec<f32>> {
    let v = dict.get(key)?;
    let resolved = match v.as_reference() {
        Some(id) => objects.resolve(id)?,
        None => v,
    };
    let arr = resolved.as_array()?;
    let mut out = Vec::with_capacity(arr.len().min(64));
    for item in arr.iter().take(64) {
        let r = match item.as_reference() {
            Some(id) => objects.resolve(id)?,
            None => item,
        };
        let n = r.as_number()? as f32;
        if !n.is_finite() {
            return None;
        }
        out.push(n);
    }
    Some(out)
}

pub(crate) fn dict_float2<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
) -> Option<[f32; 2]> {
    let v = dict_float_vec(objects, dict, key)?;
    v.get(..2)?.try_into().ok()
}

pub(crate) fn dict_float4<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
) -> Option<[f32; 4]> {
    let v = dict_float_vec(objects, dict, key)?;
    v.get(..4)?.try_into().ok()
}

pub(crate) fn dict_float6<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
) -> Option<[f32; 6]> {
    let v = dict_float_vec(objects, dict, key)?;
    v.get(..6)?.try_into().ok()
}

pub(crate) fn dict_bool2<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
) -> Option<[bool; 2]> {
    let v = dict.get(key)?;
    let resolved = match v.as_reference() {
        Some(id) => objects.resolve(id)?,
        None => v,
    };
    let arr = resolved.as_array()?;
    if arr.len() < 2 {
        return None;
    }
    let b = |i: usize| -> Option<bool> {
        let item = &arr[i];
        let r = match item.as_reference() {
            Some(id) => objects.resolve(id)?,
            None => item,
        };
        r.as_bool()
    };
    Some([b(0)?, b(1)?])
}

pub(crate) fn dict_matrix<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
) -> Option<Matrix> {
    dict_float6(objects, dict, key).map(Matrix::from_array)
}
