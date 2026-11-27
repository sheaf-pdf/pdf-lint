//! `/ExtGState` `/SMask` transparency group evaluation, caching, backdrop
//! (`/BC`), transfer function (`/TR`), and CTM placement (ISO 32000-2 §11.6.5).

use std::panic::{AssertUnwindSafe, catch_unwind};

use pdf_content_streams::{Concat, FillRule};
use pdf_gfx::{Matrix, Rect, SoftMask, SoftMaskKind, SoftMaskParams, State, TransparencyGroup};
use pdf_objects::Why;
use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Object, ObjectId, Resolve, Streams};
use vello_cpu::Mask;

use crate::canvas2d::{Canvas2d, segs_to_bez_path};
use crate::engine::stream::render_stream_into;
use crate::engine::{RenderCtx, RenderState, check_cancelled, rect_to_segs};
use crate::error::Error;
use crate::fault::Fault;
use crate::paint::{dict_float4, dict_matrix};

#[derive(Clone, PartialEq, Eq, Hash)]
enum TransferKey {
    None,
    Ref(ObjectId),
    InlinePtr(usize),
}

/// Complete identity of an active soft mask, including target canvas size,
/// `/BC`, `/TR`, and the CTM in effect when `gs` established the soft mask
/// (`ISO 32000-2`, §11.6.5.1).
#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub(crate) enum SoftMaskKey {
    #[default]
    None,
    Invalid,
    Mask(SoftMaskCacheKey),
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct SoftMaskCacheKey {
    group_id: ObjectId,
    kind_tag: u8,
    vp_size: (u32, u32),
    ctm_bits: [u32; 6],
    backdrop_bits: Vec<u32>,
    transfer: TransferKey,
}

fn canonical_f32_bits(v: f32) -> u32 {
    (v + 0.0).to_bits()
}

fn matrix_bits(m: Matrix) -> [u32; 6] {
    [
        canonical_f32_bits(m.a),
        canonical_f32_bits(m.b),
        canonical_f32_bits(m.c),
        canonical_f32_bits(m.d),
        canonical_f32_bits(m.e),
        canonical_f32_bits(m.f),
    ]
}

fn make_smask_key(
    smask: SoftMask,
    params: Option<&SoftMaskParams>,
    vp_size: (u32, u32),
) -> SoftMaskKey {
    match smask {
        SoftMask::None => SoftMaskKey::None,
        SoftMask::Mask { kind, group } => {
            let (Some(kind), Some(group_id)) = (kind, group) else {
                return SoftMaskKey::Invalid;
            };
            let kind_tag = u8::from(kind == SoftMaskKind::Luminosity);
            let ctm_bits = matrix_bits(params.map_or(Matrix::IDENTITY, |p| p.ctm));
            let backdrop_bits = params
                .and_then(|p| p.backdrop.as_ref())
                .map(|c| {
                    c.as_slice()
                        .iter()
                        .copied()
                        .map(canonical_f32_bits)
                        .collect()
                })
                .unwrap_or_default();
            let transfer = match params.and_then(|p| p.transfer.as_ref()) {
                None => TransferKey::None,
                Some(arc_obj) => match arc_obj.as_reference() {
                    Some(id) => TransferKey::Ref(id),
                    None => TransferKey::InlinePtr(std::sync::Arc::as_ptr(arc_obj) as usize),
                },
            };
            SoftMaskKey::Mask(SoftMaskCacheKey {
                group_id,
                kind_tag,
                vp_size,
                ctm_bits,
                backdrop_bits,
                transfer,
            })
        }
        _ => SoftMaskKey::None,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn sync_soft_mask<'o, 'a, O, S>(
    ctx: &RenderCtx<'o, 'a, O>,
    gstate: &State,
    page_to_device: Matrix,
    layer_depth: usize,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
    current_mask: &mut Option<Mask>,
    current_smask_key: &mut SoftMaskKey,
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let desired = gstate.soft_mask;
    let params = match desired {
        SoftMask::None => None,
        _ => gstate.soft_mask_params.as_ref(),
    };
    let vp_size = (u32::from(canvas.width()), u32::from(canvas.height()));
    let desired_key = make_smask_key(desired, params, vp_size);
    if desired_key == *current_smask_key {
        return Ok(());
    }
    *current_smask_key = desired_key.clone();

    match (desired, desired_key) {
        (SoftMask::None, _) => {
            *current_mask = None;
            canvas.set_mask(None);
        }
        (SoftMask::Mask { group, .. }, SoftMaskKey::Invalid) => {
            sink.fault(Place::new(group, None), Fault::InvalidSoftMask);
            state.approximated = true;
            *current_mask = None;
            canvas.set_mask(None);
        }
        (
            SoftMask::Mask {
                kind: Some(kind),
                group: Some(group_id),
            },
            SoftMaskKey::Mask(cache_key),
        ) => {
            if layer_depth + 1 > ctx.opts.limits.max_layer_depth {
                sink.fault(Place::new(Some(group_id), None), Fault::LayerDepthExceeded);
                state.approximated = true;
                *current_mask = None;
                canvas.set_mask(None);
                return Ok(());
            }
            if let Some(cached) = state.smask_cache.get(&cache_key) {
                *current_mask = cached.clone();
                canvas.set_mask(current_mask.clone());
                return Ok(());
            }

            let built = build_soft_mask(
                ctx,
                group_id,
                kind,
                params,
                vp_size,
                page_to_device,
                layer_depth + 1,
                state,
                sink,
            )?;
            let mask_bytes = (vp_size.0 as usize) * (vp_size.1 as usize);
            if state.note_cached_bytes(mask_bytes, ctx.opts.limits.max_scene_bytes) {
                state.smask_cache.insert(cache_key, built.clone());
            }
            *current_mask = built;
            canvas.set_mask(current_mask.clone());
        }
        _ => {
            *current_mask = None;
            canvas.set_mask(None);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_soft_mask<'o, 'a, O, S>(
    ctx: &RenderCtx<'o, 'a, O>,
    group_id: ObjectId,
    kind: SoftMaskKind,
    params: Option<&SoftMaskParams>,
    (vp_w, vp_h): (u32, u32),
    page_to_device: Matrix,
    layer_depth: usize,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<Option<Mask>, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let objects = ctx.objects;
    let opts = ctx.opts;
    check_cancelled(opts)?;
    if state.active_streams.contains(&group_id) {
        sink.fault(Place::new(Some(group_id), None), Fault::InvalidSoftMask);
        state.approximated = true;
        return Ok(None);
    }
    let Some(group_obj) = objects.try_resolve(group_id)? else {
        sink.fault(Place::new(Some(group_id), None), Fault::InvalidSoftMask);
        state.approximated = true;
        return Ok(None);
    };
    let Some(stream) = group_obj.as_stream() else {
        sink.fault(
            Place::new(Some(group_id), group_obj.at()),
            Fault::InvalidSoftMask,
        );
        state.approximated = true;
        return Ok(None);
    };
    let dict = &stream.dict;
    let data = match objects.decoded(group_id, state.decompressed_left) {
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
            sink.fault(Place::new(Some(group_id), None), Fault::InvalidSoftMask);
            state.approximated = true;
            return Ok(None);
        }
        Err(e) => return Err(Error::Unreadable(e)),
    };

    let form_matrix = dict_matrix(objects, dict, b"Matrix")?.unwrap_or(Matrix::IDENTITY);
    let smask_ctm = params.map_or(Matrix::IDENTITY, |p| p.ctm);
    // ISO 32000-2 §11.6.5.1: the soft-mask transparency group's coordinate
    // space maps via its `/Matrix` into the user space active when `gs` set the
    // soft mask (`smask_ctm`), then onto device pixels (`page_to_device`).
    let mask_transform = form_matrix.then(smask_ctm).then(page_to_device);
    if !mask_transform.is_finite() || mask_transform.determinant() == 0.0 {
        return Ok(None);
    }

    let form_bbox = dict_float4(objects, dict, b"BBox")?.map(|[a, b, c, d]| Rect::new(a, b, c, d));
    let form_res = dict.get(b"Resources");
    let group_meta = TransparencyGroup::from_dict(objects, dict);
    let backdrop = params
        .and_then(|p| p.backdrop.as_ref())
        .map(|c| c.as_slice());
    let transfer_obj: Option<&Object> = params.and_then(|p| p.transfer.as_deref());

    let bg = match kind {
        SoftMaskKind::Alpha => None,
        SoftMaskKind::Luminosity => {
            if let Some(bc_vals) = backdrop {
                let cs_obj = group_meta.and_then(|g| g.color_space);
                if let Some(cs) = cs_obj {
                    let spaces = pdf_color::Spaces::read(objects, form_res)?;
                    let mut eval_opts = pdf_color::EvalOptions::from_spaces(opts.rules, spaces);
                    eval_opts.output_intent_profile = ctx.output_intent_icc;
                    let rgb = pdf_color::Space::read(objects, cs, &opts.limits.color, &mut ())?
                        .and_then(|sp| {
                            pdf_color::PreparedSpace::from_space(
                                objects,
                                &sp,
                                &eval_opts,
                                &opts.limits.color,
                                &mut (),
                            )
                            .ok()
                            .flatten()
                        })
                        .and_then(|prep| {
                            prep.eval_with_rules(bc_vals, opts.rules)
                                .and_then(pdf_color::Paint::srgb8)
                        })
                        .unwrap_or([0, 0, 0]);
                    Some([rgb[0], rgb[1], rgb[2], 255])
                } else {
                    match bc_vals.len() {
                        1 => {
                            let g = (bc_vals[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                            Some([g, g, g, 255])
                        }
                        3 => Some([
                            (bc_vals[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                            (bc_vals[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                            (bc_vals[2].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                            255,
                        ]),
                        _ => Some([0, 0, 0, 255]),
                    }
                }
            } else {
                Some([0, 0, 0, 255])
            }
        }
        _ => None,
    };

    state.charge_offscreen_pixels(vp_w, vp_h, opts.limits.max_scene_bytes)?;
    let mut mask_canvas = Canvas2d::new(vp_w, vp_h)?;
    if let Some(bbox) = form_bbox {
        let segs = rect_to_segs(bbox);
        let (bez, _) = segs_to_bez_path(&segs);
        mask_canvas.push_clip(&bez, FillRule::NonZero, mask_transform);
    }

    let slice: &[u8] = &data;
    let concat = Concat::new(&[slice]);
    state.active_streams.push(group_id);
    let mask_res = render_stream_into(
        ctx,
        &concat,
        form_res,
        mask_transform,
        layer_depth,
        None,
        &mut mask_canvas,
        state,
        sink,
    );
    state.active_streams.pop();

    if form_bbox.is_some() {
        mask_canvas.pop_clip();
    }
    let _ = mask_res?;

    let vello_pix = mask_canvas.render_to_vello_pixmap(bg)?;
    let (w16, h16) = (vello_pix.width(), vello_pix.height());

    let tr_fn = if let Some(tr) = transfer_obj {
        pdf_color::Function::read(objects, tr, opts.rules, &opts.limits.color, &mut ())?
    } else {
        None
    };

    if let Some(func) = tr_fn {
        let mut lut = [0u8; 256];
        for (i, slot) in lut.iter_mut().enumerate() {
            let x = (i as f32) / 255.0;
            let mut y = [x];
            let _ = func.eval(&[x], &mut y);
            *slot = (y[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
        let alphas: Vec<u8> = vello_pix
            .data()
            .iter()
            .map(|px| {
                let raw = match kind {
                    SoftMaskKind::Alpha => px.a,
                    SoftMaskKind::Luminosity | _ => {
                        let r = f32::from(px.r) / 255.0;
                        let g = f32::from(px.g) / 255.0;
                        let b = f32::from(px.b) / 255.0;
                        ((r * 0.2126 + g * 0.7152 + b * 0.0722).clamp(0.0, 1.0) * 255.0 + 0.5) as u8
                    }
                };
                lut[usize::from(raw)]
            })
            .collect();
        if alphas.len() != usize::from(w16) * usize::from(h16) {
            return Ok(None);
        }
        catch_unwind(AssertUnwindSafe(move || {
            Some(Mask::from_parts(alphas, w16, h16))
        }))
        .map_err(|_| Error::RasterizerPanic)
    } else {
        catch_unwind(AssertUnwindSafe(|| {
            Some(match kind {
                SoftMaskKind::Alpha => Mask::new_alpha(&vello_pix),
                SoftMaskKind::Luminosity | _ => Mask::new_luminance(&vello_pix),
            })
        }))
        .map_err(|_| Error::RasterizerPanic)
    }
}
