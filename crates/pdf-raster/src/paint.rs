//! Color evaluation, `PatternType 1` tiling patterns, `PatternType 2` shading
//! patterns, and ISO 32000-2 §8.7.4 shadings (`ShadingType 1..=5`).

use std::sync::Arc;

use pdf_color::{EvalOptions, Function, PreparedSpace, Space};
use pdf_content_streams::{Concat, FillRule};
use pdf_gfx::{ActiveColorSpace, Event, Matrix, Point, Rect};
use pdf_objects::Why;
use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Dict, Object, ObjectId, Resolve, Streams, Unreadable};
use vello_cpu::Pixmap as VelloPixmap;

use crate::canvas2d::{Canvas2d, build_vello_pixmap_straight, segs_to_bez_path};
use crate::engine::stream::render_stream_into;
use crate::engine::{RenderCtx, RenderState, check_cancelled, rect_to_segs};
use crate::error::Error;
use crate::fault::Fault;
use crate::limits::LimitKind;
use crate::opts::RenderOpts;
use crate::phases::phase;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum ActiveColorKey {
    DeviceGray,
    DeviceRgb,
    DeviceCmyk,
    Named(Box<[u8]>),
}

/// Evaluated paint ready to configure on [`Canvas2d`].
pub(crate) enum ResolvedPaint {
    /// Solid straight-alpha `[R, G, B, A]`.
    Solid([u8; 4]),
    /// Repeated tiling bitmap (`tile`, `paint_transform` in device space).
    Tiled {
        pixmap: Arc<VelloPixmap>,
        transform: Matrix,
    },
    /// Clamped shading bitmap positioned in device space (`Extend::Pad`).
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

#[inline]
pub(crate) fn resolve_opt<'o, R: Resolve<'o>>(
    objects: R,
    obj: &'o Object,
) -> Result<Option<&'o Object>, Unreadable> {
    match obj.as_reference() {
        None => Ok(Some(obj)),
        Some(id) => match objects.try_resolve(id) {
            Ok(v) => Ok(v),
            Err(u) if u.why == Why::Damaged => Ok(None),
            Err(u) => Err(u),
        },
    }
}

/// Resolves an entry inside `/Resources/<category>/<name>`, falling back to
/// `page_resources` when `resources` omits `<category>`, matching PDFium's
/// resource inheritance while propagating hard `Unreadable` errors (`Locked`,
/// `Capped`, `Source`).
pub(crate) fn lookup_resource_entry<'o, R: Resolve<'o> + Copy>(
    objects: R,
    resources: Option<&'o Object>,
    page_resources: Option<&'o Object>,
    category: &[u8],
    name: &[u8],
) -> Result<Option<&'o Object>, Unreadable> {
    let cat_in = |res: &'o Object| -> Result<Option<&'o Dict>, Unreadable> {
        let Some(res_dict) = resolve_opt(objects, res)?.and_then(Object::as_dict) else {
            return Ok(None);
        };
        let Some(cat_val) = res_dict.get(category) else {
            return Ok(None);
        };
        Ok(resolve_opt(objects, cat_val)?.and_then(Object::as_dict))
    };
    if let Some(res) = resources
        && let Some(cat) = cat_in(res)?
    {
        return Ok(cat.get(name));
    }
    if let Some(page) = page_resources
        && let Some(cat) = cat_in(page)?
    {
        return Ok(cat.get(name));
    }
    Ok(None)
}

/// Evaluates `active_space` + `components` + `alpha` into straight-alpha
/// `[R, G, B, A]`, returning `None` for `/Separation /None` (transparent no-op
/// paint).
#[allow(clippy::too_many_arguments)]
pub(crate) fn eval_solid_color<'o, 'a, 'r, O, S>(
    ctx: &RenderCtx<'o, 'a, O>,
    active_space: &ActiveColorSpace,
    components: &[f32],
    alpha: f32,
    resources: Option<&'r Object>,
    page_resources: Option<&'r Object>,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<Option<[u8; 4]>, Error>
where
    'o: 'r,
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let objects = ctx.objects;
    let alpha_u8 = (alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;

    let key_kind = match active_space {
        ActiveColorSpace::DeviceGray => Some(ActiveColorKey::DeviceGray),
        ActiveColorSpace::DeviceRgb => Some(ActiveColorKey::DeviceRgb),
        ActiveColorSpace::DeviceCmyk => Some(ActiveColorKey::DeviceCmyk),
        ActiveColorSpace::Named(name) => Some(ActiveColorKey::Named((&**name).into())),
        ActiveColorSpace::Pattern | _ => None,
    };
    let Some(color_key) = key_kind else {
        return Ok(Some([0, 0, 0, alpha_u8]));
    };

    let res_ptr = resources.map_or(0, |o| std::ptr::from_ref(o) as usize);
    let page_res_ptr = page_resources.map_or(0, |o| std::ptr::from_ref(o) as usize);
    let full_key = (res_ptr, page_res_ptr, color_key);

    if let Some(cached) = state.color_cache.get(&full_key) {
        let Some(prep) = cached.as_ref() else {
            return Ok(Some([0, 0, 0, alpha_u8]));
        };
        return Ok(prep
            .eval_with_rules(components, ctx.opts.rules)
            .and_then(pdf_color::Paint::srgb8)
            .map(|[r, g, b]| [r, g, b, alpha_u8]));
    }

    let spaces = pdf_color::Spaces::read(objects, resources.or(page_resources))?;
    let mut eval_opts = EvalOptions::from_spaces(ctx.opts.rules, spaces);
    eval_opts.output_intent_profile = ctx.output_intent_icc;

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
            lookup_resource_entry(objects, resources, page_resources, b"ColorSpace", name)?
        }
        ActiveColorSpace::Pattern | _ => None,
    };

    let prepared = if let Some(obj) = space_obj {
        let mut csink = ColorSink(sink);
        if let Some(space) = Space::read(objects, obj, &ctx.opts.limits.color, &mut csink)? {
            // Uncolored `/Pattern` space `[/Pattern /BaseSpace]` evaluates its underlying
            // base space when painting an uncolored tiling pattern.
            let target_space = match &space {
                Space::Pattern(Some(base)) => base.as_ref(),
                other => other,
            };
            PreparedSpace::from_space(
                objects,
                target_space,
                &eval_opts,
                &ctx.opts.limits.color,
                &mut csink,
            )?
        } else {
            None
        }
    } else {
        None
    };

    let result = match &prepared {
        Some(prep) => prep
            .eval_with_rules(components, ctx.opts.rules)
            .and_then(pdf_color::Paint::srgb8)
            .map(|[r, g, b]| [r, g, b, alpha_u8]),
        None => Some([0, 0, 0, alpha_u8]),
    };

    if state.color_cache.len() >= 64 {
        state.color_cache.clear();
    }
    state.color_cache.insert(full_key, prepared);
    Ok(result)
}

/// Evaluates the active stroking or nonstroking paint (solid color, tiling
/// pattern, or shading pattern).
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_active_paint<'o, 'a, 'e, O, S>(
    ctx: &RenderCtx<'o, 'a, O>,
    event: &Event<'e>,
    stroke: bool,
    device_bbox: Option<(u32, u32, u32, u32)>,
    stream_base_ctm: Matrix,
    page_to_device: Matrix,
    layer_depth: usize,
    uncolored_override: Option<[u8; 4]>,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<ResolvedPaint, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let alpha = if stroke {
        event.state.stroke_alpha
    } else {
        event.state.fill_alpha
    };
    if let Some([ur, ug, ub, ua]) = uncolored_override {
        let combined_a = ((f32::from(ua) / 255.0) * alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        return Ok(ResolvedPaint::Solid([ur, ug, ub, combined_a]));
    }

    let (space, comps, pat_name) = if stroke {
        (
            &event.state.stroke_color_space,
            event.state.stroke_color.as_slice(),
            event.state.stroke_pattern.as_deref(),
        )
    } else {
        (
            &event.state.fill_color_space,
            event.state.fill_color.as_slice(),
            event.state.fill_pattern.as_deref(),
        )
    };

    if let Some(name) = pat_name {
        return resolve_pattern_paint(
            ctx,
            name,
            space,
            comps,
            alpha,
            device_bbox,
            event.at.resources(),
            event.at.page_resources(),
            stream_base_ctm,
            page_to_device,
            layer_depth,
            state,
            sink,
        );
    }

    match eval_solid_color(
        ctx,
        space,
        comps,
        alpha,
        event.at.resources(),
        event.at.page_resources(),
        state,
        sink,
    )? {
        Some(rgba) => Ok(ResolvedPaint::Solid(rgba)),
        None => Ok(ResolvedPaint::None),
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_pattern_paint<'o, 'a, 'r, O, S>(
    ctx: &RenderCtx<'o, 'a, O>,
    pat_name: &[u8],
    underlying_space: &ActiveColorSpace,
    underlying_comps: &[f32],
    alpha: f32,
    device_bbox: Option<(u32, u32, u32, u32)>,
    resources: Option<&'r Object>,
    page_resources: Option<&'r Object>,
    stream_base_ctm: Matrix,
    page_to_device: Matrix,
    layer_depth: usize,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<ResolvedPaint, Error>
where
    'o: 'r,
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let objects = ctx.objects;
    check_cancelled(ctx.opts)?;
    if layer_depth + 1 > ctx.opts.limits.max_layer_depth {
        sink.fault(Place::new(None, None), Fault::LayerDepthExceeded);
        state.approximated = true;
        return Ok(ResolvedPaint::None);
    }

    let Some(pat_entry) =
        lookup_resource_entry(objects, resources, page_resources, b"Pattern", pat_name)?
    else {
        sink.fault(Place::new(None, None), Fault::InvalidPattern);
        state.approximated = true;
        return Ok(ResolvedPaint::None);
    };
    let pat_id = pat_entry.as_reference();
    let pat_resolved = match pat_id {
        Some(id) => objects.try_resolve(id)?,
        None => Some(pat_entry),
    };
    let Some(pat_obj) = pat_resolved else {
        sink.fault(Place::new(pat_id, None), Fault::InvalidPattern);
        state.approximated = true;
        return Ok(ResolvedPaint::None);
    };
    let dict = match pat_obj
        .as_dict()
        .or_else(|| pat_obj.as_stream().map(|s| &s.dict))
    {
        Some(d) => d,
        None => {
            sink.fault(Place::new(pat_id, pat_obj.at()), Fault::InvalidPattern);
            state.approximated = true;
            return Ok(ResolvedPaint::None);
        }
    };

    let pat_type = dict_int(objects, dict, b"PatternType")?.unwrap_or(0);
    let pat_matrix = dict_matrix(objects, dict, b"Matrix")?.unwrap_or(Matrix::IDENTITY);
    // ISO 32000-2 §8.7.2: pattern space maps to the enclosing stream's initial
    // coordinate space (`stream_base_ctm`), then onto device pixels (`page_to_device`).
    let pat_to_device = pat_matrix.then(stream_base_ctm).then(page_to_device);
    if !pat_to_device.is_finite() || pat_to_device.determinant() == 0.0 {
        return Ok(ResolvedPaint::None);
    }

    match pat_type {
        1 => {
            let Some(sid) = pat_id else {
                sink.fault(Place::new(None, pat_obj.at()), Fault::InvalidPattern);
                state.approximated = true;
                return Ok(ResolvedPaint::None);
            };
            if state.active_streams.contains(&sid) {
                sink.fault(Place::new(Some(sid), None), Fault::InvalidPattern);
                state.approximated = true;
                return Ok(ResolvedPaint::None);
            }
            let paint_type = dict_int(objects, dict, b"PaintType")?.unwrap_or(1);
            let Some([bx0, by0, bx1, by1]) = dict_float4(objects, dict, b"BBox")? else {
                sink.fault(Place::new(Some(sid), None), Fault::InvalidPattern);
                state.approximated = true;
                return Ok(ResolvedPaint::None);
            };
            let bbox = Rect::new(bx0, by0, bx1, by1);
            let x_step = dict_float(objects, dict, b"XStep")?.unwrap_or(0.0).abs();
            let y_step = dict_float(objects, dict, b"YStep")?.unwrap_or(0.0).abs();
            if bbox.is_empty() || !(x_step > 1e-4 && y_step > 1e-4) {
                sink.fault(Place::new(Some(sid), None), Fault::InvalidPattern);
                state.approximated = true;
                return Ok(ResolvedPaint::None);
            }

            if ctx.opts.limits.max_tile_side == 0 || ctx.opts.limits.max_tile_pixels == 0 {
                return Err(Error::Limit(LimitKind::PatternCells));
            }
            let sx = pat_to_device.a.hypot(pat_to_device.b);
            let sy = pat_to_device.c.hypot(pat_to_device.d);
            let max_side = ctx.opts.limits.max_tile_side.min(u32::from(u16::MAX));
            let ideal_w = (x_step * sx).round().max(1.0);
            let ideal_h = (y_step * sy).round().max(1.0);
            if ideal_w > (max_side as f32) || ideal_h > (max_side as f32) {
                state.approximated = true;
            }
            let tile_w = (ideal_w as u32).clamp(1, max_side);
            let tile_h = (ideal_h as u32).clamp(1, max_side);
            if u64::from(tile_w) * u64::from(tile_h) > ctx.opts.limits.max_tile_pixels {
                return Err(Error::Limit(LimitKind::PatternCells));
            }

            let uncolored_rgba = if paint_type == 2 {
                match eval_solid_color(
                    ctx,
                    underlying_space,
                    underlying_comps,
                    alpha,
                    resources,
                    page_resources,
                    state,
                    sink,
                )? {
                    Some(c) => Some(c),
                    None => return Ok(ResolvedPaint::None),
                }
            } else {
                None
            };

            let pat_to_tile = Matrix::translate(-bbox.x0, -(bbox.y0 + y_step)).then(Matrix::scale(
                (tile_w as f32) / x_step,
                -(tile_h as f32) / y_step,
            ));
            let Some(tile_to_pat) = pat_to_tile.inverse() else {
                return Ok(ResolvedPaint::None);
            };
            let paint_transform = tile_to_pat.then(pat_to_device);

            let alpha_u8 = if paint_type != 2 && alpha < 1.0 {
                (alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
            } else {
                255
            };
            let tile_key = (sid, tile_w, tile_h, alpha_u8, uncolored_rgba);
            if let Some(cached_tile) = state.tile_cache.get(&tile_key).cloned() {
                state.patterns_drawn += 1;
                return Ok(ResolvedPaint::Tiled {
                    pixmap: cached_tile,
                    transform: paint_transform,
                });
            }

            let data = match objects.decoded(sid, state.decompressed_left) {
                Ok(b) => {
                    state.decompressed_left = state.decompressed_left.saturating_sub(b.len());
                    b
                }
                Err(e) if e.why == Why::Limit => {
                    return Err(Error::Gfx(pdf_gfx::Error::Limit(
                        pdf_gfx::Cap::Decompressed,
                    )));
                }
                Err(e) if matches!(e.why, Why::Damaged | Why::Filter) => {
                    sink.fault(Place::new(Some(sid), None), Fault::InvalidPattern);
                    state.approximated = true;
                    return Ok(ResolvedPaint::None);
                }
                Err(e) => return Err(Error::Unreadable(e)),
            };

            state.charge_offscreen_pixels(tile_w, tile_h, ctx.opts.limits.max_scene_bytes)?;
            let mut tile_canvas = Canvas2d::new(tile_w, tile_h)?;
            let clip_segs = rect_to_segs(bbox);
            let (clip_bez, _) = segs_to_bez_path(&clip_segs);
            tile_canvas.push_clip(&clip_bez, FillRule::NonZero, pat_to_tile);

            let slice: &[u8] = &data;
            let concat = Concat::new(&[slice]);
            let pat_res = dict.get(b"Resources");
            state.active_streams.push(sid);
            let tile_res = render_stream_into(
                ctx,
                &concat,
                pat_res,
                pat_to_tile,
                layer_depth + 1,
                uncolored_rgba,
                &mut tile_canvas,
                state,
                sink,
            );
            state.active_streams.pop();
            tile_canvas.pop_clip();
            let _ = tile_res?;

            let mut vello_tile = tile_canvas.render_to_vello_pixmap(None)?;
            if alpha_u8 < 255 {
                vello_tile.multiply_alpha(alpha_u8);
            }
            let arc_tile = Arc::new(vello_tile);
            let tile_bytes = (tile_w as usize) * (tile_h as usize) * 4;
            if state.note_cached_bytes(tile_bytes, ctx.opts.limits.max_scene_bytes) {
                state.tile_cache.insert(tile_key, arc_tile.clone());
            }
            state.patterns_drawn += 1;
            Ok(ResolvedPaint::Tiled {
                pixmap: arc_tile,
                transform: paint_transform,
            })
        }
        2 => {
            let Some(sh_obj) = dict.get(b"Shading") else {
                sink.fault(Place::new(pat_id, None), Fault::InvalidPattern);
                state.approximated = true;
                return Ok(ResolvedPaint::None);
            };
            let Some(bounds) = device_bbox else {
                return Ok(ResolvedPaint::None);
            };
            match evaluate_shading_rect(
                ctx,
                sh_obj,
                pat_to_device,
                bounds,
                true,
                alpha,
                resources,
                page_resources,
                state,
                sink,
            )? {
                Some((pixmap, transform)) => {
                    state.shadings_drawn += 1;
                    Ok(ResolvedPaint::Shading { pixmap, transform })
                }
                None => Ok(ResolvedPaint::None),
            }
        }
        _ => {
            sink.fault(Place::new(pat_id, None), Fault::InvalidPattern);
            state.approximated = true;
            Ok(ResolvedPaint::None)
        }
    }
}

/// Applies `paint` to `canvas` under `scene_transform`, dispatching tiling
/// patterns to `Extend::Repeat` and shadings to `Extend::Pad`.
pub(crate) fn configure_paint_on_canvas(
    canvas: &mut Canvas2d,
    paint: &ResolvedPaint,
    scene_transform: Matrix,
) -> bool {
    match paint {
        ResolvedPaint::None => false,
        ResolvedPaint::Solid(rgba) => {
            canvas.set_solid_rgba8(*rgba);
            true
        }
        ResolvedPaint::Tiled { pixmap, transform } => {
            let paint_xf = match scene_transform.inverse() {
                Some(inv) => (*transform).then(inv),
                None => *transform,
            };
            canvas.set_tiling_paint(pixmap.clone(), paint_xf);
            true
        }
        ResolvedPaint::Shading { pixmap, transform } => {
            let paint_xf = match scene_transform.inverse() {
                Some(inv) => (*transform).then(inv),
                None => *transform,
            };
            canvas.set_shading_paint(pixmap.clone(), paint_xf);
            true
        }
    }
}

/// Evaluates a shading dictionary or stream (`sh` or `PatternType 2`) over the
/// integer device-pixel rectangle `[x0..x1] × [y0..y1]`, proportionally
/// downsampling the evaluation grid when `w * h > max_shading_samples`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn evaluate_shading_rect<'o, 'a, 'r, O, S>(
    ctx: &RenderCtx<'o, 'a, O>,
    shading_obj: &'r Object,
    shading_to_device: Matrix,
    device_bounds: (u32, u32, u32, u32),
    include_background: bool,
    alpha: f32,
    resources: Option<&'r Object>,
    page_resources: Option<&'r Object>,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<Option<(Arc<VelloPixmap>, Matrix)>, Error>
where
    'o: 'r,
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let objects = ctx.objects;
    let opts = ctx.opts;
    check_cancelled(opts)?;
    let (x0, y0, x1, y1) = device_bounds;
    if x1 <= x0 || y1 <= y0 {
        return Ok(None);
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let total_px = u64::from(w) * u64::from(h);
    let max_samples = opts.limits.max_shading_samples.max(1);
    let (grid_w, grid_h, step_x, step_y) = if total_px > u64::from(max_samples) {
        state.approximated = true;
        let scale = (f64::from(max_samples) / (total_px as f64)).sqrt();
        let mut gw = ((f64::from(w) * scale).floor() as u32).clamp(1, w);
        let mut gh = ((f64::from(h) * scale).floor() as u32).clamp(1, h);
        while u64::from(gw) * u64::from(gh) > u64::from(max_samples) {
            if gw >= gh && gw > 1 {
                gw -= 1;
            } else if gh > 1 {
                gh -= 1;
            } else {
                break;
            }
        }
        (gw, gh, (w as f32) / (gw as f32), (h as f32) / (gh as f32))
    } else {
        (w, h, 1.0_f32, 1.0_f32)
    };
    let (Ok(w16), Ok(h16)) = (u16::try_from(grid_w), u16::try_from(grid_h)) else {
        return Ok(None);
    };

    let resolved = match shading_obj.as_reference() {
        Some(id) => objects.try_resolve(id)?,
        None => Some(shading_obj),
    };
    let (dict, stream_id, sh_span) = match resolved {
        Some(obj) if obj.as_dict().is_some() => {
            (obj.as_dict().unwrap(), shading_obj.as_reference(), obj.at())
        }
        Some(obj) if obj.as_stream().is_some() => (
            &obj.as_stream().unwrap().dict,
            shading_obj.as_reference(),
            obj.at(),
        ),
        _ => {
            sink.fault(
                Place::new(shading_obj.as_reference(), shading_obj.at()),
                Fault::InvalidShading,
            );
            state.approximated = true;
            return Ok(None);
        }
    };

    let grid_to_device =
        Matrix::scale(step_x, step_y).then(Matrix::translate(x0 as f32, y0 as f32));
    let Some(device_to_grid) = grid_to_device.inverse() else {
        return Ok(None);
    };
    let shading_to_grid = shading_to_device.then(device_to_grid);
    let Some(grid_to_shading) = shading_to_grid.inverse() else {
        return Ok(None);
    };

    let shading_type = dict_int(objects, dict, b"ShadingType")?.unwrap_or(0);
    phase!(
        DEBUG,
        "eval_shading",
        shading_type = shading_type.max(0) as u64,
        width = grid_w,
        height = grid_h
    );

    let Some(cs_raw) = dict.get(b"ColorSpace") else {
        sink.fault(Place::new(stream_id, sh_span), Fault::InvalidShading);
        state.approximated = true;
        return Ok(None);
    };
    let cs_resolved = resolve_opt(objects, cs_raw)?;
    let cs_obj = match cs_resolved.and_then(Object::as_name) {
        Some(name) if !matches!(name, b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK") => {
            lookup_resource_entry(objects, resources, page_resources, b"ColorSpace", name)?
                .unwrap_or(cs_raw)
        }
        _ => cs_raw,
    };

    let spaces = pdf_color::Spaces::read(objects, resources.or(page_resources))?;
    let mut eval_opts = EvalOptions::from_spaces(opts.rules, spaces);
    eval_opts.output_intent_profile = ctx.output_intent_icc;

    let mut csink = ColorSink(sink);
    let Some(space) = Space::read(objects, cs_obj, &opts.limits.color, &mut csink)? else {
        state.approximated = true;
        return Ok(None);
    };
    let Some(prepared) =
        PreparedSpace::from_space(objects, &space, &eval_opts, &opts.limits.color, &mut csink)?
    else {
        state.approximated = true;
        return Ok(None);
    };

    let alpha_u8 = (alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let bg_rgba = if include_background {
        dict_float_vec(objects, dict, b"Background")?
            .and_then(|vals| {
                prepared
                    .eval_with_rules(&vals, opts.rules)
                    .and_then(pdf_color::Paint::srgb8)
            })
            .map(|[r, g, b]| [r, g, b, alpha_u8])
    } else {
        None
    };

    if matches!(shading_type, 6 | 7) {
        sink.fault(
            Place::new(stream_id, sh_span),
            Fault::UnsupportedShading { shading_type },
        );
        state.approximated = true;
        if let Some(bg) = bg_rgba {
            state.charge_offscreen_pixels(grid_w, grid_h, opts.limits.max_scene_bytes)?;
            let mut rgba = vec![0u8; (grid_w as usize) * (grid_h as usize) * 4];
            for px in rgba.as_chunks_mut::<4>().0 {
                *px = bg;
            }
            let Some(pix) = build_vello_pixmap_straight(rgba, w16, h16) else {
                return Ok(None);
            };
            return Ok(Some((Arc::new(pix), grid_to_device)));
        }
        return Ok(None);
    }

    if !matches!(shading_type, 1..=5) {
        sink.fault(Place::new(stream_id, sh_span), Fault::InvalidShading);
        state.approximated = true;
        return Ok(None);
    }

    let shading_bbox =
        dict_float4(objects, dict, b"BBox")?.map(|[a, b, c, d]| Rect::new(a, b, c, d));

    state.charge_offscreen_pixels(grid_w, grid_h, opts.limits.max_scene_bytes)?;
    let mut rgba = vec![0u8; (grid_w as usize) * (grid_h as usize) * 4];
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
            grid_to_shading,
            shading_bbox,
            (grid_w, grid_h),
            alpha_u8,
            opts,
            &mut csink,
            &mut rgba,
        )?,
        2 => eval_type2_axial(
            objects,
            dict,
            &prepared,
            grid_to_shading,
            shading_bbox,
            (grid_w, grid_h),
            alpha_u8,
            opts,
            &mut csink,
            &mut rgba,
        )?,
        3 => eval_type3_radial(
            objects,
            dict,
            &prepared,
            grid_to_shading,
            shading_bbox,
            (grid_w, grid_h),
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
                    shading_to_grid,
                    shading_bbox,
                    (grid_w, grid_h),
                    alpha_u8,
                    opts,
                    state,
                    &mut csink,
                    &mut rgba,
                )?;
            } else {
                sink.fault(Place::new(None, sh_span), Fault::InvalidShading);
                state.approximated = true;
                return Ok(None);
            }
        }
        _ => unreachable!(),
    }

    let Some(pix) = build_vello_pixmap_straight(rgba, w16, h16) else {
        return Ok(None);
    };
    Ok(Some((Arc::new(pix), grid_to_device)))
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
    let resolved = resolve_opt(objects, fn_raw)?;
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

fn eval_functions(funcs: &[Function], inputs: &[f32], out: &mut [f32; 16]) -> Option<usize> {
    if funcs.len() == 1 {
        let n = (funcs[0].outputs() as usize).min(16);
        funcs[0].eval(inputs, &mut out[..n]).then_some(n)
    } else {
        let n = funcs.len().min(16);
        for (i, f) in funcs.iter().take(n).enumerate() {
            let mut single = [0.0f32; 1];
            if !f.eval(inputs, &mut single) {
                return None;
            }
            out[i] = single[0];
        }
        Some(n)
    }
}

#[allow(clippy::too_many_arguments)]
fn eval_type1_shading<'o, O, S>(
    objects: O,
    dict: &'o Dict,
    prepared: &PreparedSpace,
    grid_to_shading: Matrix,
    shading_bbox: Option<Rect>,
    (w, h): (u32, u32),
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
    let domain = dict_float4(objects, dict, b"Domain")?.unwrap_or([0.0, 1.0, 0.0, 1.0]);
    let dom_to_sh = dict_matrix(objects, dict, b"Matrix")?.unwrap_or(Matrix::IDENTITY);
    let Some(sh_to_dom) = dom_to_sh.inverse() else {
        return Ok(());
    };
    let grid_to_dom = grid_to_shading.then(sh_to_dom);
    let (xmin, xmax) = (domain[0].min(domain[1]), domain[0].max(domain[1]));
    let (ymin, ymax) = (domain[2].min(domain[3]), domain[2].max(domain[3]));

    let mut comps = [0.0f32; 16];
    for row in 0..h {
        if row & 15 == 0 {
            check_cancelled(opts)?;
        }
        let py = (row as f32) + 0.5;
        let mut p_sh = grid_to_shading.apply(Point::new(0.5, py));
        let mut p_dom = grid_to_dom.apply(Point::new(0.5, py));
        for col in 0..w {
            let cur_sh = p_sh;
            let cur_dom = p_dom;
            p_sh.x += grid_to_shading.a;
            p_sh.y += grid_to_shading.b;
            p_dom.x += grid_to_dom.a;
            p_dom.y += grid_to_dom.b;

            if let Some(bb) = shading_bbox
                && !bb.contains(cur_sh)
            {
                continue;
            }
            if cur_dom.x < xmin || cur_dom.x > xmax || cur_dom.y < ymin || cur_dom.y > ymax {
                continue;
            }
            let Some(n) = eval_functions(&funcs, &[cur_dom.x, cur_dom.y], &mut comps) else {
                continue;
            };
            if let Some([r, g, b]) = prepared
                .eval_with_rules(&comps[..n], opts.rules)
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
    grid_to_shading: Matrix,
    shading_bbox: Option<Rect>,
    (w, h): (u32, u32),
    alpha_u8: u8,
    opts: &RenderOpts,
    sink: &mut ColorSink<'_, S>,
    rgba: &mut [u8],
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let Some([ax0, ay0, ax1, ay1]) = dict_float4(objects, dict, b"Coords")? else {
        return Ok(());
    };
    let Some(funcs) = read_shading_functions(objects, dict, opts.rules, &opts.limits.color, sink)?
    else {
        return Ok(());
    };
    let [t0, t1] = dict_float2(objects, dict, b"Domain")?.unwrap_or([0.0, 1.0]);
    let [ext0, ext1] = dict_bool2(objects, dict, b"Extend")?.unwrap_or([false, false]);

    let (dx, dy) = (ax1 - ax0, ay1 - ay0);
    let denom = dx * dx + dy * dy;
    if denom <= 1e-12 {
        return Ok(());
    }

    const LUT_LEN: usize = 512;
    let mut lut = [[0u8; 4]; LUT_LEN];
    let mut lut_valid = [false; LUT_LEN];
    let mut comps = [0.0f32; 16];
    for i in 0..LUT_LEN {
        let s = (i as f32) / ((LUT_LEN - 1) as f32);
        let t = t0 + (t1 - t0) * s;
        if let Some(n) = eval_functions(&funcs, &[t], &mut comps)
            && let Some([r, g, b]) = prepared
                .eval_with_rules(&comps[..n], opts.rules)
                .and_then(pdf_color::Paint::srgb8)
        {
            lut[i] = [r, g, b, alpha_u8];
            lut_valid[i] = true;
        }
    }

    let step_s = (grid_to_shading.a * dx + grid_to_shading.b * dy) / denom;
    for row in 0..h {
        if row & 15 == 0 {
            check_cancelled(opts)?;
        }
        let py = (row as f32) + 0.5;
        let mut p_sh = grid_to_shading.apply(Point::new(0.5, py));
        let mut raw_s = ((p_sh.x - ax0) * dx + (p_sh.y - ay0) * dy) / denom;
        for col in 0..w {
            let cur_sh = p_sh;
            let cur_s = raw_s;
            p_sh.x += grid_to_shading.a;
            p_sh.y += grid_to_shading.b;
            raw_s += step_s;

            if let Some(bb) = shading_bbox
                && !bb.contains(cur_sh)
            {
                continue;
            }
            let s = if cur_s < 0.0 {
                if !ext0 {
                    continue;
                }
                0.0
            } else if cur_s > 1.0 {
                if !ext1 {
                    continue;
                }
                1.0
            } else {
                cur_s
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
    grid_to_shading: Matrix,
    shading_bbox: Option<Rect>,
    (w, h): (u32, u32),
    alpha_u8: u8,
    opts: &RenderOpts,
    sink: &mut ColorSink<'_, S>,
    rgba: &mut [u8],
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let Some([cx0, cy0, r0, cx1, cy1, r1]) = dict_float6(objects, dict, b"Coords")? else {
        return Ok(());
    };
    if r0 < 0.0 || r1 < 0.0 {
        return Ok(());
    }
    let Some(funcs) = read_shading_functions(objects, dict, opts.rules, &opts.limits.color, sink)?
    else {
        return Ok(());
    };
    let [t0, t1] = dict_float2(objects, dict, b"Domain")?.unwrap_or([0.0, 1.0]);
    let [ext0, ext1] = dict_bool2(objects, dict, b"Extend")?.unwrap_or([false, false]);

    const LUT_LEN: usize = 512;
    let mut lut = [[0u8; 4]; LUT_LEN];
    let mut lut_valid = [false; LUT_LEN];
    let mut comps = [0.0f32; 16];
    for i in 0..LUT_LEN {
        let s = (i as f32) / ((LUT_LEN - 1) as f32);
        let t = t0 + (t1 - t0) * s;
        if let Some(n) = eval_functions(&funcs, &[t], &mut comps)
            && let Some([r, g, b]) = prepared
                .eval_with_rules(&comps[..n], opts.rules)
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
        if row & 15 == 0 {
            check_cancelled(opts)?;
        }
        let py = (row as f32) + 0.5;
        let mut p_sh = grid_to_shading.apply(Point::new(0.5, py));
        for col in 0..w {
            let cur_sh = p_sh;
            p_sh.x += grid_to_shading.a;
            p_sh.y += grid_to_shading.b;

            if let Some(bb) = shading_bbox
                && !bb.contains(cur_sh)
            {
                continue;
            }
            let (vx, vy) = (f64::from(cur_sh.x) - cx0_d, f64::from(cur_sh.y) - cy0_d);
            let b_half = vx * dx + vy * dy + r0_d * dr;
            let c_coef = vx * vx + vy * vy - r0_d * r0_d;

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

/// Evaluates `ShadingType 4` (Free-form Gouraud triangle mesh, ISO 32000-2
/// §8.7.4.5.5) and `ShadingType 5` (Lattice-form Gouraud triangle mesh,
/// §8.7.4.5.6).
#[allow(clippy::too_many_arguments)]
fn eval_mesh_shading<'o, O, S>(
    objects: O,
    stream_id: ObjectId,
    dict: &'o Dict,
    shading_type: i64,
    prepared: &PreparedSpace,
    shading_to_grid: Matrix,
    shading_bbox: Option<Rect>,
    (w, h): (u32, u32),
    alpha_u8: u8,
    opts: &RenderOpts,
    state: &mut RenderState,
    sink: &mut ColorSink<'_, S>,
    rgba: &mut [u8],
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let bpc_coord = dict_int(objects, dict, b"BitsPerCoordinate")?.unwrap_or(0);
    let bpc_comp = dict_int(objects, dict, b"BitsPerComponent")?.unwrap_or(0);
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
    let Some(decode_arr) = dict_float_vec(objects, dict, b"Decode")? else {
        return Ok(());
    };
    if decode_arr.len() < 4 + n_vals * 2 {
        return Ok(());
    }
    let data = match objects.decoded(stream_id, opts.limits.color.stream_bytes) {
        Ok(b) => b,
        Err(e) if matches!(e.why, Why::Damaged | Why::Filter | Why::Limit) => {
            sink.0
                .fault(Place::new(Some(stream_id), None), Fault::InvalidShading);
            state.approximated = true;
            return Ok(());
        }
        Err(e) => return Err(Error::Unreadable(e)),
    };

    #[derive(Clone, Copy)]
    struct MeshVertex {
        grid_pt: Point,
        vals: [f32; 16],
    }

    let mut reader = BitReader::new(&data);
    let read_coords_and_vals = |r: &mut BitReader<'_>| -> Option<MeshVertex> {
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
        // ISO 32000-2 §8.7.4.5.5 & §8.7.4.5.6: each vertex record is padded to a byte boundary.
        r.align_to_byte();
        Some(MeshVertex {
            grid_pt: shading_to_grid.apply(Point::new(x, y)),
            vals,
        })
    };

    let max_tris = (opts.limits.max_shading_samples as usize).clamp(1, 65_536);
    let mut triangles: Vec<[MeshVertex; 3]> = Vec::new();
    let mut truncated = false;

    if shading_type == 4 {
        let bp_flag = dict_int(objects, dict, b"BitsPerFlag")?.unwrap_or(0);
        if !matches!(bp_flag, 2 | 4 | 8) {
            return Ok(());
        }
        let read_flagged = |r: &mut BitReader<'_>| -> Option<(u32, MeshVertex)> {
            let flag = r.read_bits(bp_flag as u8)?;
            let v = read_coords_and_vals(r)?;
            Some((flag & 3, v))
        };
        let mut prev_tri: Option<[MeshVertex; 3]> = None;
        loop {
            if triangles.len() >= max_tris {
                if reader.has_more_bytes() {
                    truncated = true;
                }
                break;
            }
            let Some((flag, v_first)) = read_flagged(&mut reader) else {
                break;
            };
            let tri = match flag {
                0 => {
                    // ISO 32000-2 §8.7.4.5.5: a new triangle begins with a vertex whose edge flag
                    // is `0`, followed by two more complete vertex records (including their edge
                    // flags, which are ignored).
                    let (Some((_, v1)), Some((_, v2))) =
                        (read_flagged(&mut reader), read_flagged(&mut reader))
                    else {
                        break;
                    };
                    [v_first, v1, v2]
                }
                1 => {
                    let Some([_, vb, vc]) = prev_tri else {
                        break;
                    };
                    [vb, vc, v_first]
                }
                2 => {
                    let Some([va, _, vc]) = prev_tri else {
                        break;
                    };
                    [va, vc, v_first]
                }
                _ => break,
            };
            prev_tri = Some(tri);
            triangles.push(tri);
        }
    } else {
        let per_row = dict_int(objects, dict, b"VerticesPerRow")?.unwrap_or(0);
        if !(2..=4096).contains(&per_row) {
            return Ok(());
        }
        let cols = per_row as usize;
        let mut prev_row: Vec<MeshVertex> = Vec::with_capacity(cols);
        loop {
            if triangles.len() >= max_tris {
                if reader.has_more_bytes() {
                    truncated = true;
                }
                break;
            }
            let mut row = Vec::with_capacity(cols);
            for _ in 0..cols {
                if let Some(v) = read_coords_and_vals(&mut reader) {
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
                        truncated = true;
                        break;
                    }
                    triangles.push([prev_row[i], prev_row[i + 1], row[i]]);
                    triangles.push([prev_row[i + 1], row[i + 1], row[i]]);
                }
            }
            prev_row = row;
        }
    }

    // Precompute a 1D LUT for large parametric meshes when pixel steps exceed 65,536,
    // while keeping exact per-pixel `/Function` evaluation on normal meshes!
    const PARAM_LUT_LEN: usize = 1024;
    type ParamLut = [Option<[u8; 4]>; PARAM_LUT_LEN];
    let t_min = decode_arr[4].min(decode_arr[5]);
    let t_max = decode_arr[4].max(decode_arr[5]);
    let mut param_lut: Option<Box<ParamLut>> = None;

    let grid_to_sh = shading_to_grid.inverse();
    let mut steps_left = u64::from(opts.limits.max_shading_samples.max(1)).saturating_mul(4);
    let mut steps_used = 0u64;
    let mut comps = [0.0f32; 16];
    let mut interp_vals = [0.0f32; 16];

    'tris: for [v0, v1, v2] in triangles {
        check_cancelled(opts)?;
        let min_x = v0
            .grid_pt
            .x
            .min(v1.grid_pt.x)
            .min(v2.grid_pt.x)
            .floor()
            .max(0.0) as u32;
        let max_x = v0
            .grid_pt
            .x
            .max(v1.grid_pt.x)
            .max(v2.grid_pt.x)
            .ceil()
            .min(w as f32) as u32;
        let min_y = v0
            .grid_pt
            .y
            .min(v1.grid_pt.y)
            .min(v2.grid_pt.y)
            .floor()
            .max(0.0) as u32;
        let max_y = v0
            .grid_pt
            .y
            .max(v1.grid_pt.y)
            .max(v2.grid_pt.y)
            .ceil()
            .min(h as f32) as u32;
        if min_x >= max_x || min_y >= max_y {
            continue;
        }
        let area = edge_fn(v0.grid_pt, v1.grid_pt, v2.grid_pt);
        if area.abs() < 1e-6 {
            continue;
        }
        let inv_area = 1.0 / area;
        for py_u in min_y..max_y {
            let py = (py_u as f32) + 0.5;
            for px_u in min_x..max_x {
                if steps_left == 0 {
                    truncated = true;
                    break 'tris;
                }
                steps_left -= 1;
                let px = (px_u as f32) + 0.5;
                let p = Point::new(px, py);
                let w0 = edge_fn(v1.grid_pt, v2.grid_pt, p) * inv_area;
                let w1 = edge_fn(v2.grid_pt, v0.grid_pt, p) * inv_area;
                let w2 = 1.0 - w0 - w1;
                if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                    continue;
                }
                if let (Some(bb), Some(inv)) = (shading_bbox, grid_to_sh)
                    && !bb.contains(inv.apply(p))
                {
                    continue;
                }
                steps_used += 1;

                let pixel = if let Some(ref fs) = funcs {
                    let t = w0 * v0.vals[0] + w1 * v1.vals[0] + w2 * v2.vals[0];
                    if steps_used <= 65_536 || (t_max - t_min).abs() <= 1e-12 {
                        eval_functions(fs, &[t], &mut comps)
                            .and_then(|n| prepared.eval_with_rules(&comps[..n], opts.rules))
                            .and_then(pdf_color::Paint::srgb8)
                            .map(|[r, g, b]| [r, g, b, alpha_u8])
                    } else {
                        let lut = param_lut.get_or_insert_with(|| {
                            let mut tbl = Box::new([None; PARAM_LUT_LEN]);
                            for (i, slot) in tbl.iter_mut().enumerate() {
                                let s = (i as f32) / ((PARAM_LUT_LEN - 1) as f32);
                                let tv = t_min + (t_max - t_min) * s;
                                if let Some(n) = eval_functions(fs, &[tv], &mut comps)
                                    && let Some([r, g, b]) = prepared
                                        .eval_with_rules(&comps[..n], opts.rules)
                                        .and_then(pdf_color::Paint::srgb8)
                                {
                                    *slot = Some([r, g, b, alpha_u8]);
                                }
                            }
                            tbl
                        });
                        let norm = ((t - t_min) / (t_max - t_min)).clamp(0.0, 1.0);
                        let li = ((norm * ((PARAM_LUT_LEN - 1) as f32)) + 0.5) as usize;
                        lut[li.min(PARAM_LUT_LEN - 1)]
                    }
                } else {
                    for (c, slot) in interp_vals.iter_mut().enumerate().take(n_vals) {
                        *slot = w0 * v0.vals[c] + w1 * v1.vals[c] + w2 * v2.vals[c];
                    }
                    prepared
                        .eval_with_rules(&interp_vals[..n_vals], opts.rules)
                        .and_then(pdf_color::Paint::srgb8)
                        .map(|[r, g, b]| [r, g, b, alpha_u8])
                };

                if let Some(c) = pixel {
                    let idx = ((py_u as usize) * (w as usize) + (px_u as usize)) * 4;
                    rgba[idx..idx + 4].copy_from_slice(&c);
                }
            }
        }
    }

    if truncated {
        state.approximated = true;
        sink.0
            .fault(Place::new(Some(stream_id), None), Fault::InvalidShading);
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

    fn align_to_byte(&mut self) {
        self.bit_pos = (self.bit_pos + 7) & !7;
    }

    fn has_more_bytes(&self) -> bool {
        (self.bit_pos >> 3) < self.data.len()
    }
}

pub(crate) fn dict_int<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
) -> Result<Option<i64>, Unreadable> {
    let Some(v) = dict.get(key) else {
        return Ok(None);
    };
    Ok(resolve_opt(objects, v)?.and_then(Object::as_integer))
}

pub(crate) fn dict_float<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
) -> Result<Option<f32>, Unreadable> {
    let Some(v) = dict.get(key) else {
        return Ok(None);
    };
    let Some(resolved) = resolve_opt(objects, v)? else {
        return Ok(None);
    };
    let Some(raw) = resolved.as_number() else {
        return Ok(None);
    };
    let n = raw as f32;
    Ok(n.is_finite().then_some(n))
}

pub(crate) fn dict_float_vec<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
) -> Result<Option<Vec<f32>>, Unreadable> {
    let Some(v) = dict.get(key) else {
        return Ok(None);
    };
    let Some(arr) = resolve_opt(objects, v)?.and_then(Object::as_array) else {
        return Ok(None);
    };
    let mut out = Vec::with_capacity(arr.len().min(64));
    for item in arr.iter().take(64) {
        let Some(r) = resolve_opt(objects, item)? else {
            return Ok(None);
        };
        let Some(raw) = r.as_number() else {
            return Ok(None);
        };
        let n = raw as f32;
        if !n.is_finite() {
            return Ok(None);
        }
        out.push(n);
    }
    Ok(Some(out))
}

pub(crate) fn dict_float2<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
) -> Result<Option<[f32; 2]>, Unreadable> {
    let Some(v) = dict_float_vec(objects, dict, key)? else {
        return Ok(None);
    };
    Ok(v.get(..2).and_then(|s| s.try_into().ok()))
}

pub(crate) fn dict_float4<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
) -> Result<Option<[f32; 4]>, Unreadable> {
    let Some(v) = dict_float_vec(objects, dict, key)? else {
        return Ok(None);
    };
    Ok(v.get(..4).and_then(|s| s.try_into().ok()))
}

pub(crate) fn dict_float6<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
) -> Result<Option<[f32; 6]>, Unreadable> {
    let Some(v) = dict_float_vec(objects, dict, key)? else {
        return Ok(None);
    };
    Ok(v.get(..6).and_then(|s| s.try_into().ok()))
}

pub(crate) fn dict_bool2<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
) -> Result<Option<[bool; 2]>, Unreadable> {
    let Some(v) = dict.get(key) else {
        return Ok(None);
    };
    let Some(arr) = resolve_opt(objects, v)?.and_then(Object::as_array) else {
        return Ok(None);
    };
    if arr.len() < 2 {
        return Ok(None);
    }
    let Some(b0) = resolve_opt(objects, &arr[0])?.and_then(Object::as_bool) else {
        return Ok(None);
    };
    let Some(b1) = resolve_opt(objects, &arr[1])?.and_then(Object::as_bool) else {
        return Ok(None);
    };
    Ok(Some([b0, b1]))
}

pub(crate) fn dict_matrix<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    key: &[u8],
) -> Result<Option<Matrix>, Unreadable> {
    Ok(dict_float6(objects, dict, key)?.map(Matrix::from_array))
}
