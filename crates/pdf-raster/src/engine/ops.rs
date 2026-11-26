//! Path, text, Type 3 `/CharProcs`, Image XObject / inline image, and `sh`
//! painting operations.

use std::panic::{AssertUnwindSafe, catch_unwind};

use pdf_content_streams::{Concat, FillRule, TextMode};
use pdf_fonts::{Font, FontKind};
use pdf_gfx::{
    BlendState, Event, ImageDrawn, ImageOf, Matrix, PathDrawn, Point, Rect, Seg, TextShow,
};
use pdf_images::DecodeOptions;
use pdf_objects::Why;
use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Object, Resolve, Streams};
use vello_cpu::Mask;
use vello_cpu::kurbo::BezPath;

use crate::builtin_glyphs::emit_builtin_glyph;
use crate::canvas2d::{Canvas2d, outline_verbs_to_bez_path, segs_to_bez_path};
use crate::engine::sinks::ImageSinkAdapter;
use crate::engine::stream::{
    GroupOverrides, effective_blend, effective_fill_alpha, render_stream_into,
};
use crate::engine::{RenderCtx, RenderState, clamp_device_rect, rect_to_segs};
use crate::error::Error;
use crate::fallback::FontQuery;
use crate::fault::Fault;
use crate::paint::{
    ResolvedPaint, configure_paint_on_canvas, eval_solid_color, evaluate_shading_rect,
    lookup_resource_entry, resolve_active_paint,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_path<'o, 'a, 'e, O, S>(
    ctx: &RenderCtx<'o, 'a, O>,
    event: &Event<'e>,
    drawn: &PathDrawn<'e>,
    stream_base_ctm: Matrix,
    page_to_device: Matrix,
    layer_depth: usize,
    uncolored_override: Option<[u8; 4]>,
    ov: GroupOverrides,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let fills = drawn.paint.fills();
    let strokes = drawn.paint.strokes();
    if fills.is_none() && !strokes {
        return Ok(());
    }

    let user_to_device = event.state.ctm.then(page_to_device);
    if !user_to_device.is_finite() {
        return Ok(());
    }

    let dev_rect = page_to_device.apply_rect(drawn.bounds);
    let dev_bbox = clamp_device_rect(
        dev_rect,
        u32::from(canvas.width()),
        u32::from(canvas.height()),
    );
    let eff_blend = effective_blend(event.state, ov);

    canvas.set_anti_alias(ctx.opts.anti_alias.paths);
    canvas.set_blend_mode(&eff_blend);

    if let Some(rule) = fills {
        let paint = resolve_active_paint(
            ctx,
            event,
            false,
            dev_bbox,
            stream_base_ctm,
            page_to_device,
            layer_depth,
            uncolored_override,
            ov,
            state,
            sink,
        )?;
        canvas.set_transform(page_to_device);
        if configure_paint_on_canvas(canvas, &paint, page_to_device) {
            let (fill_bez, _) = segs_to_bez_path(drawn.segments);
            canvas.set_fill_rule(rule);
            canvas.fill_bez_path(&fill_bez);
        }
    }

    if strokes && user_to_device.determinant() != 0.0 {
        let paint = resolve_active_paint(
            ctx,
            event,
            true,
            dev_bbox,
            stream_base_ctm,
            page_to_device,
            layer_depth,
            uncolored_override,
            ov,
            state,
            sink,
        )?;
        canvas.set_transform(user_to_device);
        if configure_paint_on_canvas(canvas, &paint, user_to_device) {
            let (stroke_bez, extent) = segs_to_bez_path(drawn.user_segments);
            canvas.set_stroke_style(
                event.state.line_width,
                event.state.line_cap,
                event.state.line_join,
                event.state.miter_limit,
                &event.state.dash,
                user_to_device,
                extent,
            );
            canvas.stroke_bez_path(&stroke_bez);
        }
    }

    Ok(())
}

/// Accumulates text-clip glyph outlines (`4..=7 Tr`) when inside hidden optional
/// content without painting glyphs.
pub(crate) fn accum_text_clip_only<'o, 'a, 'e, O>(
    ctx: &RenderCtx<'o, 'a, O>,
    event: &Event<'e>,
    show: &TextShow<'e>,
    page_to_device: Matrix,
    state: &mut RenderState,
    bt_clip_bez: &mut BezPath,
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
{
    let Some(font) = show.font else {
        return Ok(());
    };
    let clips = show.mode.clips() && font.kind() != FontKind::Type3;
    if !clips {
        let n = show.glyphs().count();
        if n > state.glyphs_left {
            return Err(Error::Gfx(pdf_gfx::Error::Limit(pdf_gfx::Cap::Glyphs)));
        }
        state.glyphs_left -= n;
        return Ok(());
    }

    let trm_dev = show.trm().then(page_to_device);
    let ppem_y = trm_dev.c.hypot(trm_dev.d);
    let hint_ppem =
        (ctx.opts.hint_fonts && ctx.opts.anti_alias.text && ppem_y >= 4.0).then_some(ppem_y);
    let size_scale = show.size / 1000.0;
    let text_scale = event.state.text.scale;
    let rise = event.state.text.rise;
    let mut verbs = Vec::new();

    for glyph in show.glyphs() {
        if state.glyphs_left == 0 {
            return Err(Error::Gfx(pdf_gfx::Error::Limit(pdf_gfx::Cap::Glyphs)));
        }
        state.glyphs_left -= 1;
        verbs.clear();
        let mut h_stretch = 1.0f32;
        if !font.outline(glyph.code, hint_ppem, &mut verbs) {
            state.substituted_glyphs += 1;
            state.approximated = true;
            let decoded_char = glyph.text().and_then(|s| s.chars().next());
            let target_w = font.width(glyph.code);
            let used_external = if let (Some(fb), Some(ch)) = (ctx.fallback, decoded_char) {
                let query = FontQuery::from_font(font);
                if let Some(prog) =
                    ctx.cache
                        .fallback_program(fb, query, &ctx.opts.limits.gfx.fonts)
                {
                    if let Some(fb_adv) = prog.outline_char(ch, hint_ppem, &mut verbs) {
                        if target_w > 10.0 && fb_adv > 10.0 {
                            h_stretch = (target_w / fb_adv).clamp(0.4, 2.5);
                        }
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };
            if !used_external {
                emit_builtin_glyph(
                    decoded_char,
                    glyph.code,
                    target_w,
                    font.std14(),
                    font.style(),
                    font.glyph_box(glyph.code),
                    &mut verbs,
                );
            }
        }
        if verbs.is_empty() {
            continue;
        }
        let v_shift = if font.is_vertical() {
            let v = font.vertical(glyph.code);
            let (vx, vy) = v.map_or((0.0, 0.0), |v| (v.vx, v.vy));
            Matrix::translate(-vx, -vy)
        } else {
            Matrix::IDENTITY
        };
        let glyph_to_page = v_shift
            .then(Matrix::scale(
                size_scale * text_scale * h_stretch,
                size_scale,
            ))
            .then(Matrix::translate(glyph.pen.x, glyph.pen.y + rise))
            .then(event.state.text.matrix)
            .then(event.state.ctm);
        let _ = outline_verbs_to_bez_path(&verbs, glyph_to_page, bt_clip_bez);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_text<'o, 'a, 'e, O, S>(
    ctx: &RenderCtx<'o, 'a, O>,
    event: &Event<'e>,
    show: &TextShow<'e>,
    stream_base_ctm: Matrix,
    page_to_device: Matrix,
    layer_depth: usize,
    uncolored_override: Option<[u8; 4]>,
    ov: GroupOverrides,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
    bt_clip_bez: &mut BezPath,
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let Some(font) = show.font else {
        return Ok(());
    };
    let mode = show.mode;
    let fills = matches!(
        mode,
        TextMode::Fill | TextMode::FillStroke | TextMode::FillClip | TextMode::FillStrokeClip
    );
    let strokes = matches!(
        mode,
        TextMode::Stroke | TextMode::FillStroke | TextMode::StrokeClip | TextMode::FillStrokeClip
    );
    let clips = mode.clips() && font.kind() != FontKind::Type3;
    if !fills && !strokes && !clips {
        let n = show.glyphs().count();
        if n > state.glyphs_left {
            return Err(Error::Gfx(pdf_gfx::Error::Limit(pdf_gfx::Cap::Glyphs)));
        }
        state.glyphs_left -= n;
        return Ok(());
    }

    if font.approx().any() {
        state.approximated = true;
    }

    if font.kind() == FontKind::Type3 {
        return paint_type3_text(
            ctx,
            event,
            show,
            font,
            page_to_device,
            layer_depth,
            uncolored_override,
            ov,
            canvas,
            state,
            sink,
        );
    }

    let trm_dev = show.trm().then(page_to_device);
    let ppem_y = trm_dev.c.hypot(trm_dev.d);
    let hint_ppem =
        (ctx.opts.hint_fonts && ctx.opts.anti_alias.text && ppem_y >= 4.0).then_some(ppem_y);

    let size_scale = show.size / 1000.0;
    let text_scale = event.state.text.scale;
    let rise = event.state.text.rise;

    let mut run_bez = BezPath::new();
    let mut user_bez = BezPath::new();
    let mut stroke_extent = 0.0f64;
    let mut verbs = Vec::new();
    let mut run_bounds: Option<Rect> = None;

    for glyph in show.glyphs() {
        if state.glyphs_left == 0 {
            return Err(Error::Gfx(pdf_gfx::Error::Limit(pdf_gfx::Cap::Glyphs)));
        }
        state.glyphs_left -= 1;
        verbs.clear();
        let mut h_stretch = 1.0f32;
        if !font.outline(glyph.code, hint_ppem, &mut verbs) {
            state.substituted_glyphs += 1;
            state.approximated = true;
            let decoded_char = glyph.text().and_then(|s| s.chars().next());
            let target_w = font.width(glyph.code);
            let used_external = if let (Some(fb), Some(ch)) = (ctx.fallback, decoded_char) {
                let query = FontQuery::from_font(font);
                if let Some(prog) =
                    ctx.cache
                        .fallback_program(fb, query, &ctx.opts.limits.gfx.fonts)
                {
                    if let Some(fb_adv) = prog.outline_char(ch, hint_ppem, &mut verbs) {
                        if target_w > 10.0 && fb_adv > 10.0 {
                            h_stretch = (target_w / fb_adv).clamp(0.4, 2.5);
                        }
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };

            if !used_external {
                emit_builtin_glyph(
                    decoded_char,
                    glyph.code,
                    target_w,
                    font.std14(),
                    font.style(),
                    font.glyph_box(glyph.code),
                    &mut verbs,
                );
            }
        }

        if verbs.is_empty() {
            continue;
        }

        let v_shift = if font.is_vertical() {
            let v = font.vertical(glyph.code);
            let (vx, vy) = v.map_or((0.0, 0.0), |v| (v.vx, v.vy));
            Matrix::translate(-vx, -vy)
        } else {
            Matrix::IDENTITY
        };
        let glyph_to_tm = v_shift
            .then(Matrix::scale(
                size_scale * text_scale * h_stretch,
                size_scale,
            ))
            .then(Matrix::translate(glyph.pen.x, glyph.pen.y + rise));
        let glyph_to_user = glyph_to_tm.then(event.state.text.matrix);
        let glyph_to_page = glyph_to_user.then(event.state.ctm);

        let g_rect = glyph.quad.bounds();
        run_bounds = Some(match run_bounds {
            Some(r) => r.union(g_rect),
            None => g_rect,
        });

        if fills || clips {
            let _ = outline_verbs_to_bez_path(&verbs, glyph_to_page, &mut run_bez);
        }
        if clips {
            let _ = outline_verbs_to_bez_path(&verbs, glyph_to_page, bt_clip_bez);
        }
        if strokes {
            stroke_extent += outline_verbs_to_bez_path(&verbs, glyph_to_user, &mut user_bez);
        }
    }

    if !fills && !strokes {
        return Ok(());
    }

    let dev_bbox = run_bounds.and_then(|b| {
        clamp_device_rect(
            page_to_device.apply_rect(b),
            u32::from(canvas.width()),
            u32::from(canvas.height()),
        )
    });
    let eff_blend = effective_blend(event.state, ov);
    canvas.set_anti_alias(ctx.opts.anti_alias.text);
    canvas.set_blend_mode(&eff_blend);

    if fills && !run_bez.is_empty() {
        let paint = resolve_active_paint(
            ctx,
            event,
            false,
            dev_bbox,
            stream_base_ctm,
            page_to_device,
            layer_depth,
            uncolored_override,
            ov,
            state,
            sink,
        )?;
        canvas.set_transform(page_to_device);
        if configure_paint_on_canvas(canvas, &paint, page_to_device) {
            canvas.set_fill_rule(FillRule::NonZero);
            canvas.fill_bez_path(&run_bez);
        }
    }

    let user_to_device = event.state.ctm.then(page_to_device);
    if strokes && !user_bez.is_empty() && user_to_device.determinant() != 0.0 {
        let paint = resolve_active_paint(
            ctx,
            event,
            true,
            dev_bbox,
            stream_base_ctm,
            page_to_device,
            layer_depth,
            uncolored_override,
            ov,
            state,
            sink,
        )?;
        canvas.set_transform(user_to_device);
        if configure_paint_on_canvas(canvas, &paint, user_to_device) {
            canvas.set_stroke_style(
                event.state.line_width,
                event.state.line_cap,
                event.state.line_join,
                event.state.miter_limit,
                &event.state.dash,
                user_to_device,
                stroke_extent,
            );
            canvas.stroke_bez_path(&user_bez);
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn paint_type3_text<'o, 'a, 'e, O, S>(
    ctx: &RenderCtx<'o, 'a, O>,
    event: &Event<'e>,
    show: &TextShow<'e>,
    font: &Font,
    page_to_device: Matrix,
    layer_depth: usize,
    uncolored_override: Option<[u8; 4]>,
    ov: GroupOverrides,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let objects = ctx.objects;
    if layer_depth + 1 > ctx.opts.limits.max_layer_depth {
        sink.fault(Place::new(font.id(), None), Fault::LayerDepthExceeded);
        state.approximated = true;
        return Ok(());
    }
    let Some(fm_arr) = font.type3_matrix() else {
        return Ok(());
    };
    let font_matrix = Matrix::from_array(fm_arr);
    let type3_res = font.type3_resources().or_else(|| event.at.resources());
    let text_scale = event.state.text.scale;
    let rise = event.state.text.rise;

    let d1_color = match uncolored_override {
        Some(c) => Some(c),
        None => eval_solid_color(
            ctx,
            &event.state.fill_color_space,
            event.state.fill_color.as_slice(),
            effective_fill_alpha(event.state, ov),
            event.at.resources(),
            event.at.page_resources(),
            sink,
        )?,
    };

    for glyph in show.glyphs() {
        if state.glyphs_left == 0 {
            return Err(Error::Gfx(pdf_gfx::Error::Limit(pdf_gfx::Cap::Glyphs)));
        }
        state.glyphs_left -= 1;
        let Some(t3) = font.type3_glyph(glyph.code) else {
            continue;
        };
        if state.active_streams.contains(&t3.stream) {
            continue;
        }
        let data = match objects.decoded(t3.stream, state.decompressed_left) {
            Ok(b) => {
                state.decompressed_left = state.decompressed_left.saturating_sub(b.len());
                b
            }
            Err(e) if e.why == Why::Limit => {
                return Err(Error::Gfx(pdf_gfx::Error::Limit(
                    pdf_gfx::Cap::Decompressed,
                )));
            }
            Err(e) if matches!(e.why, Why::Damaged | Why::Filter) => continue,
            Err(e) => return Err(Error::Unreadable(e)),
        };
        let glyph_to_page = font_matrix
            .then(Matrix::scale(show.size * text_scale, show.size))
            .then(Matrix::translate(glyph.pen.x, glyph.pen.y + rise))
            .then(event.state.text.matrix)
            .then(event.state.ctm);
        let glyph_to_device = glyph_to_page.then(page_to_device);
        if !glyph_to_device.is_finite() || glyph_to_device.determinant() == 0.0 {
            continue;
        }
        let override_color = if t3.uncolored {
            let Some(c) = d1_color else {
                continue;
            };
            Some(c)
        } else {
            uncolored_override
        };
        let slice: &[u8] = &data;
        let concat = Concat::new(&[slice]);
        state.active_streams.push(t3.stream);
        let glyph_res = render_stream_into(
            ctx,
            &concat,
            type3_res,
            glyph_to_device,
            layer_depth + 1,
            override_color,
            canvas,
            state,
            sink,
        );
        state.active_streams.pop();
        let _ = glyph_res?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_image<'o, 'a, 'e, O, S>(
    ctx: &RenderCtx<'o, 'a, O>,
    event: &Event<'e>,
    img: &ImageDrawn,
    stream_base_ctm: Matrix,
    page_to_device: Matrix,
    layer_depth: usize,
    uncolored_override: Option<[u8; 4]>,
    ov: GroupOverrides,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let objects = ctx.objects;
    let opts = ctx.opts;
    if let (Some(oc), Some(oid)) = (ctx.oc_state, img.oc) {
        let oc_obj = Object::from(oid);
        if !oc.is_visible(objects, &oc_obj)? {
            return Ok(());
        }
    }

    let unit_to_device = event.state.ctm.then(page_to_device);
    if !unit_to_device.is_finite() || unit_to_device.determinant() == 0.0 {
        return Ok(());
    }

    let fill_alpha = effective_fill_alpha(event.state, ov);
    // Only evaluate active fill paint when `/ImageMask` is `true`! Non-stencil
    // images carry their own `/ColorSpace` and ignore `state.fill_color`.
    let stencil_paint = if img.stencil {
        let dev_bbox = clamp_device_rect(
            page_to_device.apply_rect(img.quad.bounds()),
            u32::from(canvas.width()),
            u32::from(canvas.height()),
        );
        let paint = resolve_active_paint(
            ctx,
            event,
            false,
            dev_bbox,
            stream_base_ctm,
            page_to_device,
            layer_depth,
            uncolored_override,
            ov,
            state,
            sink,
        )?;
        if matches!(paint, ResolvedPaint::None) {
            return Ok(());
        }
        Some(paint)
    } else {
        None
    };

    let solid_stencil_rgba = match &stencil_paint {
        Some(ResolvedPaint::Solid(rgba)) => *rgba,
        _ => [255, 255, 255, 255],
    };

    let active_res = event.at.resources().or_else(|| event.at.page_resources());
    let mut dec_opts = DecodeOptions::from_resources(objects, opts.rules, active_res)?;
    dec_opts.color.output_intent_profile = ctx.output_intent_icc;
    dec_opts.stencil_color = [
        solid_stencil_rgba[0],
        solid_stencil_rgba[1],
        solid_stencil_rgba[2],
        255,
    ];

    let mut img_sink = ImageSinkAdapter(sink);
    let decoded = match &img.of {
        ImageOf::XObject(id) => pdf_images::decode_xobject_with(
            objects,
            *id,
            active_res,
            &dec_opts,
            &opts.limits.images,
            &mut img_sink,
        ),
        ImageOf::Inline { .. } => {
            let Some(inline) = event.at.inline_image() else {
                return Ok(());
            };
            pdf_images::decode_inline_with(
                objects,
                &inline,
                event.at.content(),
                active_res,
                &dec_opts,
                &opts.limits.images,
                &mut img_sink,
            )
        }
        _ => return Ok(()),
    };

    let rgba_img = match decoded {
        Ok(im) => im,
        Err(pdf_images::Error::Unreadable(u))
            if matches!(
                u.why,
                Why::Locked | Why::Capped | Why::Source | Why::Detached
            ) =>
        {
            return Err(Error::Unreadable(u));
        }
        Err(_) => {
            let obj_id = match img.of {
                ImageOf::XObject(id) => Some(id),
                _ => None,
            };
            sink.fault(Place::new(obj_id, None), Fault::SkippedImage);
            state.approximated = true;
            return Ok(());
        }
    };

    let (w, h) = (rgba_img.width(), rgba_img.height());
    let mut bytes = rgba_img.into_bytes();
    let eff_blend = effective_blend(event.state, ov);
    let interpolate = img.interpolate && opts.anti_alias.images;

    // Pattern- or shading-filled `/ImageMask true` stencil: rasterize the
    // stencil into an alpha mask and fill the unit quad with the pattern/shading!
    if let Some(paint @ (ResolvedPaint::Tiled { .. } | ResolvedPaint::Shading { .. })) =
        stencil_paint
    {
        let (vp_w, vp_h) = (u32::from(canvas.width()), u32::from(canvas.height()));
        state.charge_offscreen_pixels(vp_w, vp_h)?;
        let mut mask_canvas = Canvas2d::new(vp_w, vp_h)?;
        mask_canvas.draw_rgba_image(
            bytes,
            w,
            h,
            unit_to_device,
            interpolate,
            1.0,
            &BlendState::default(),
        );
        let mask_pix = mask_canvas.render_to_vello_pixmap(None)?;
        let stencil_mask = catch_unwind(AssertUnwindSafe(|| Mask::new_alpha(&mask_pix)))
            .map_err(|_| Error::RasterizerPanic)?;

        let unit_corners = [
            unit_to_device.apply(Point::new(0.0, 0.0)),
            unit_to_device.apply(Point::new(1.0, 0.0)),
            unit_to_device.apply(Point::new(1.0, 1.0)),
            unit_to_device.apply(Point::new(0.0, 1.0)),
        ];
        let quad_segs = [
            Seg::Move(unit_corners[0]),
            Seg::Line(unit_corners[1]),
            Seg::Line(unit_corners[2]),
            Seg::Line(unit_corners[3]),
            Seg::Line(unit_corners[0]),
            Seg::Close,
        ];
        let (quad_bez, _) = segs_to_bez_path(&quad_segs);

        canvas.set_anti_alias(opts.anti_alias.paths);
        canvas.push_group_layer(None, &eff_blend, 1.0, Some(stencil_mask));
        canvas.set_blend_mode(&BlendState::default());
        canvas.set_transform(Matrix::IDENTITY);
        if configure_paint_on_canvas(canvas, &paint, Matrix::IDENTITY) {
            canvas.set_fill_rule(FillRule::NonZero);
            canvas.fill_bez_path(&quad_bez);
        }
        canvas.pop_layer();
        state.images_drawn += 1;
        return Ok(());
    }

    let opacity = if img.stencil {
        f32::from(solid_stencil_rgba[3]) / 255.0
    } else if let Some([ur, ug, ub, ua]) = uncolored_override {
        for px in bytes.as_chunks_mut::<4>().0 {
            px[0] = ur;
            px[1] = ug;
            px[2] = ub;
            px[3] = ((u16::from(px[3]) * u16::from(ua) + 127) / 255) as u8;
        }
        fill_alpha
    } else {
        fill_alpha
    };

    canvas.set_anti_alias(opts.anti_alias.paths);
    canvas.draw_rgba_image(
        bytes,
        w,
        h,
        unit_to_device,
        interpolate,
        opacity,
        &eff_blend,
    );
    state.images_drawn += 1;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_shading_operator<'o, 'a, 'e, O, S>(
    ctx: &RenderCtx<'o, 'a, O>,
    event: &Event<'e>,
    bounds_in_page: Option<Rect>,
    page_to_device: Matrix,
    ov: GroupOverrides,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let Some(name) = event.at.operand(0).and_then(|o| o.name()) else {
        return Ok(());
    };
    let Some(sh_obj) = lookup_resource_entry(
        ctx.objects,
        event.at.resources(),
        event.at.page_resources(),
        b"Shading",
        &name,
    ) else {
        return Ok(());
    };

    let (vp_w, vp_h) = (u32::from(canvas.width()), u32::from(canvas.height()));
    let dev_bounds = match bounds_in_page {
        Some(b) => match clamp_device_rect(page_to_device.apply_rect(b), vp_w, vp_h) {
            Some(r) => r,
            None => return Ok(()),
        },
        None => (0, 0, vp_w, vp_h),
    };

    let shading_to_device = event.state.ctm.then(page_to_device);
    let alpha = effective_fill_alpha(event.state, ov);
    // ISO 32000-2 §8.7.4.3 Table 76: `/Background` is NOT applied when painting
    // directly with `sh` (`include_background = false`).
    let Some((pixmap, transform)) = evaluate_shading_rect(
        ctx,
        sh_obj,
        shading_to_device,
        dev_bounds,
        false,
        alpha,
        event.at.resources(),
        event.at.page_resources(),
        state,
        sink,
    )?
    else {
        return Ok(());
    };

    let eff_blend = effective_blend(event.state, ov);
    canvas.set_anti_alias(ctx.opts.anti_alias.paths);
    canvas.set_blend_mode(&eff_blend);
    canvas.set_transform(Matrix::IDENTITY);
    canvas.set_shading_paint(pixmap, transform);
    let (x0, y0, x1, y1) = dev_bounds;
    let rect_segs = rect_to_segs(Rect::new(x0 as f32, y0 as f32, x1 as f32, y1 as f32));
    let (bez, _) = segs_to_bez_path(&rect_segs);
    canvas.set_fill_rule(FillRule::NonZero);
    canvas.fill_bez_path(&bez);
    state.shadings_drawn += 1;
    Ok(())
}
