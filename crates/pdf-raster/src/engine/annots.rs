//! Page `/Annots` and interactive `/AcroForm` `/Subtype /Widget` overlay
//! rendering (written `/AP /N` streams and stateless synthesized appearances).

use std::collections::BTreeMap;

#[cfg(feature = "text-layout")]
use pdf_annots::Annot;
use pdf_annots::{
    AppearanceMode, Appearances as AnnotAppearances, TypedAnnot, page_annots_with,
    placement_matrix, stream_placement_matrix, synth_annot,
};
use pdf_content_streams::{Concat, FillRule, ResBody, ResourceNeeds};
use pdf_gfx::{BlendState, Matrix, Rect, TransparencyGroup};
use pdf_objects::Why;
use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Dict, Object, ObjectId, Resolve, Streams};

use crate::canvas2d::{Canvas2d, segs_to_bez_path};
#[cfg(feature = "text-layout")]
use crate::engine::sinks::FormSinkAdapter;
use crate::engine::sinks::{
    AnnotSinkAdapter, EPHEMERAL_GEN, EphemeralOverlay, Narrow, alloc_ephemeral_block,
};
use crate::engine::stream::render_stream_into;
use crate::engine::{RenderCtx, RenderState, check_cancelled, is_srgb_group_cs, rect_to_segs};
use crate::error::Error;
use crate::fault::Fault;
use crate::limits::LimitKind;
use crate::opts::{Appearances, RenderMedium};
use crate::phases::phase;

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_page_annotations<'o, 'a, O, S>(
    ctx: &RenderCtx<'o, 'a, O>,
    #[cfg_attr(not(feature = "text-layout"), allow(unused_variables))] catalog: Option<&'o Dict>,
    page_dict: &'o Dict,
    page_id: Option<ObjectId>,
    page_to_device: Matrix,
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
    check_cancelled(opts)?;
    let annots = {
        let mut asink = AnnotSinkAdapter(sink);
        page_annots_with(
            objects,
            page_dict,
            page_id,
            opts.rules,
            &opts.limits.annots,
            &mut asink,
        )?
    };
    if annots.len() > opts.limits.max_annots {
        return Err(Error::Limit(LimitKind::Annotations));
    }
    phase!(DEBUG, "render_annots", count = annots.len());

    #[cfg(feature = "text-layout")]
    let mut lazy_form: Option<Option<pdf_forms::Form<'o>>> = None;

    for annot in &annots {
        check_cancelled(opts)?;
        let visible = match opts.medium {
            RenderMedium::Display => annot.is_visible_on_screen(),
            RenderMedium::Print => annot.is_printable(),
        };
        if !visible {
            continue;
        }
        if let (Some(oc), Some(oc_entry)) = (ctx.oc_state, annot.dict().get(b"OC"))
            && !oc.is_visible(objects, oc_entry)?
        {
            continue;
        }

        let is_widget = matches!(annot.typed(), TypedAnnot::Widget(_));
        if is_widget && !opts.draw.forms {
            continue;
        }
        if !is_widget && !opts.draw.annots {
            continue;
        }

        let rect = annot.rect();
        if rect.is_empty() || !rect.is_finite() {
            continue;
        }

        let strict_state = opts.appearances == Appearances::WrittenOnly && is_widget;
        let written_ap = AnnotAppearances::read_with(
            objects,
            annot.dict(),
            annot.id(),
            opts.rules,
            &opts.limits.annots,
            &mut AnnotSinkAdapter(sink),
        )?
        .and_then(|ap| ap.stream(AppearanceMode::Normal, strict_state, false));

        if let Some(stream_id) = written_ap {
            if draw_written_appearance_stream(
                ctx,
                stream_id,
                rect,
                page_to_device,
                canvas,
                state,
                sink,
            )? {
                if is_widget {
                    state.forms_drawn += 1;
                } else {
                    state.annots_drawn += 1;
                }
            }
            continue;
        }

        if opts.appearances == Appearances::WrittenOnly {
            continue;
        }

        if matches!(annot.typed(), TypedAnnot::Redact(_)) {
            continue;
        }

        if is_widget {
            #[cfg(feature = "text-layout")]
            {
                if lazy_form.is_none() {
                    let loaded = match catalog {
                        Some(cat) => {
                            let mut fsink = FormSinkAdapter(sink);
                            pdf_forms::Form::read_with(
                                objects,
                                cat,
                                opts.rules,
                                &opts.limits.forms,
                                &mut fsink,
                            )?
                        }
                        None => None,
                    };
                    lazy_form = Some(loaded);
                }
                if let Some(Some(ref form)) = lazy_form
                    && draw_synthesized_widget(
                        ctx,
                        form,
                        annot,
                        page_to_device,
                        canvas,
                        state,
                        sink,
                    )?
                {
                    state.forms_drawn += 1;
                }
            }
        } else if let Some(synth) = synth_annot(annot)
            && draw_ephemeral_appearance(
                ctx,
                &synth.content,
                &synth.needs,
                synth.bbox,
                synth.matrix,
                rect,
                page_to_device,
                canvas,
                state,
                sink,
            )?
        {
            state.annots_drawn += 1;
        }
    }

    Ok(())
}

fn draw_written_appearance_stream<'o, 'a, O, S>(
    ctx: &RenderCtx<'o, 'a, O>,
    stream_id: ObjectId,
    annot_rect: Rect,
    page_to_device: Matrix,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<bool, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let objects = ctx.objects;
    let opts = ctx.opts;
    if state.active_streams.contains(&stream_id) {
        return Ok(false);
    }
    let Some((bbox, form_matrix, place)) =
        stream_placement_matrix(objects, stream_id, annot_rect, &opts.limits.annots)?
    else {
        return Ok(false);
    };
    let Some(stream_obj) = objects.try_resolve(stream_id)? else {
        return Ok(false);
    };
    let Some(stream) = stream_obj.as_stream() else {
        return Ok(false);
    };
    let data = match objects.decoded(stream_id, state.decompressed_left) {
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
            sink.fault(Place::new(Some(stream_id), None), Fault::SkippedStream);
            state.approximated = true;
            return Ok(false);
        }
        Err(e) => return Err(Error::Unreadable(e)),
    };

    let form_to_page = form_matrix.then(place);
    let form_to_device = form_to_page.then(page_to_device);
    if !form_to_device.is_finite() || form_to_device.determinant() == 0.0 {
        return Ok(false);
    }

    let group = TransparencyGroup::from_dict(objects, &stream.dict);
    if let Some(ref g) = group
        && (!g.isolated
            || g.knockout
            || g.color_space
                .is_some_and(|cs| !is_srgb_group_cs(objects, cs)))
    {
        sink.fault(Place::new(Some(stream_id), None), Fault::ApproximatedGroup);
        state.approximated = true;
    }
    let clip_segs = rect_to_segs(bbox);
    let (clip_bez, _) = segs_to_bez_path(&clip_segs);
    canvas.set_anti_alias(opts.anti_alias.paths);
    canvas.push_clip(&clip_bez, FillRule::NonZero, form_to_device);
    if group.is_some() {
        canvas.push_group_layer(None, &BlendState::default(), 1.0, None);
    }

    let slice: &[u8] = &data;
    let concat = Concat::new(&[slice]);
    let res = stream.dict.get(b"Resources");
    state.active_streams.push(stream_id);
    let res_run = render_stream_into(
        ctx,
        &concat,
        res,
        form_to_device,
        1,
        None,
        canvas,
        state,
        sink,
    );
    state.active_streams.pop();

    if group.is_some() {
        canvas.pop_layer();
    }
    canvas.pop_clip();
    let _ = res_run?;
    Ok(true)
}

#[cfg(feature = "text-layout")]
fn draw_synthesized_widget<'o, 'a, O, S>(
    ctx: &RenderCtx<'o, 'a, O>,
    form: &pdf_forms::Form<'o>,
    annot: &Annot<'o>,
    page_to_device: Matrix,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<bool, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    use pdf_forms::FieldKind;
    use pdf_forms::appearance::{
        Unmade, choice_for_widget, push_for_widget, text_for_widget, toggle_for_widget,
    };

    let objects = ctx.objects;
    let opts = ctx.opts;
    let matched = form.fields().iter().find_map(|field| {
        field
            .widgets()
            .iter()
            .find(|w| {
                if let (Some(wid), Some(aid)) = (w.id(), annot.id()) {
                    wid == aid
                } else {
                    std::ptr::eq(w.dict(), annot.dict())
                }
            })
            .map(|w| (field, w))
    });
    let Some((field, widget)) = matched else {
        return Ok(false);
    };

    let map_unmade = |u: Unmade| -> Result<(), Error> {
        match u {
            Unmade::Unreadable(unr) => Err(Error::Unreadable(unr)),
            _ => Ok(()),
        }
    };

    let chosen_ap: Option<pdf_forms::appearance::Appearance> = match field.kind() {
        Some(FieldKind::Text) => {
            let mut fsink = FormSinkAdapter(sink);
            match text_for_widget(
                form,
                field,
                widget,
                objects,
                opts.rules,
                None,
                &opts.limits.forms,
                &mut fsink,
            ) {
                Ok(made) => Some(made.appearance),
                Err(u) => {
                    map_unmade(u)?;
                    None
                }
            }
        }
        Some(FieldKind::CheckBox | FieldKind::Radio) => {
            let mut fsink = FormSinkAdapter(sink);
            match toggle_for_widget(
                form,
                field,
                widget,
                objects,
                opts.rules,
                &opts.limits.forms,
                &mut fsink,
            ) {
                Ok(made) => {
                    let states =
                        widget.states_with(objects, opts.rules, &opts.limits.forms, &mut fsink)?;
                    let active: Option<&[u8]> =
                        states.state.as_deref().or_else(|| match field.value() {
                            Some(pdf_forms::FieldValue::State(s)) => Some(*s),
                            _ => None,
                        });
                    if active.is_some_and(|s| s == made.on_state.as_slice()) {
                        Some(made.appearances.on)
                    } else {
                        made.appearances.off
                    }
                }
                Err(u) => {
                    map_unmade(u)?;
                    None
                }
            }
        }
        Some(FieldKind::PushButton) => {
            let mut fsink = FormSinkAdapter(sink);
            match push_for_widget(
                form,
                field,
                widget,
                objects,
                opts.rules,
                &opts.limits.forms,
                &mut fsink,
            ) {
                Ok(made) => Some(made.appearances.normal),
                Err(u) => {
                    map_unmade(u)?;
                    None
                }
            }
        }
        Some(FieldKind::Combo | FieldKind::List) => {
            let mut fsink = FormSinkAdapter(sink);
            match choice_for_widget(
                form,
                field,
                widget,
                objects,
                opts.rules,
                &opts.limits.forms,
                &mut fsink,
            ) {
                Ok(made) => Some(made.appearance),
                Err(u) => {
                    map_unmade(u)?;
                    None
                }
            }
        }
        _ => None,
    };

    let Some(ap) = chosen_ap else {
        return Ok(false);
    };
    draw_ephemeral_appearance(
        ctx,
        &ap.content,
        &ap.needs,
        ap.bbox,
        ap.matrix,
        annot.rect(),
        page_to_device,
        canvas,
        state,
        sink,
    )
}

#[allow(clippy::too_many_arguments)]
fn draw_ephemeral_appearance<'o, 'a, O, S>(
    ctx: &RenderCtx<'o, 'a, O>,
    content_bytes: &[u8],
    needs: &ResourceNeeds,
    bbox: Rect,
    form_matrix: Matrix,
    annot_rect: Rect,
    page_to_device: Matrix,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<bool, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let Some(place) = placement_matrix(bbox, form_matrix, annot_rect) else {
        return Ok(false);
    };
    let form_to_page = form_matrix.then(place);
    let form_to_device = form_to_page.then(page_to_device);
    if !form_to_device.is_finite() || form_to_device.determinant() == 0.0 {
        return Ok(false);
    }

    let mut extra = BTreeMap::<ObjectId, Object>::new();
    let mut next_ephemeral: u32 = 1;
    let mut res_dict = Dict::new();
    for need in &needs.0 {
        let entry_obj = match need.res.body() {
            ResBody::Written(id) => Object::from(*id),
            ResBody::Objects(objs) => {
                let Ok(len_u32) = u32::try_from(objs.len()) else {
                    continue;
                };
                let Some(base_num) =
                    alloc_ephemeral_block(ctx.objects, &extra, &mut next_ephemeral, len_u32)
                else {
                    continue;
                };
                for (idx, raw_obj) in objs.iter().enumerate() {
                    let mut cloned = raw_obj.clone();
                    cloned.renumber(|to| {
                        (to.generation == 0 && (to.number as usize) < objs.len())
                            .then(|| ObjectId::new(base_num + to.number, EPHEMERAL_GEN))
                    });
                    extra.insert(
                        ObjectId::new(base_num + (idx as u32), EPHEMERAL_GEN),
                        cloned,
                    );
                }
                Object::from(ObjectId::new(base_num, EPHEMERAL_GEN))
            }
            _ => continue,
        };
        let cat_key = need.kind.key();
        if let Some(cat) = res_dict.get_mut(cat_key).and_then(Object::as_dict_mut) {
            cat.set(need.name.clone(), entry_obj);
        } else {
            let mut cat = Dict::new();
            cat.set(need.name.clone(), entry_obj);
            res_dict.set(cat_key.to_vec(), cat);
        }
    }
    let res_obj = Object::from(res_dict);

    let clip_segs = rect_to_segs(bbox);
    let (clip_bez, _) = segs_to_bez_path(&clip_segs);
    canvas.set_anti_alias(ctx.opts.anti_alias.paths);
    canvas.push_clip(&clip_bez, FillRule::NonZero, form_to_device);

    let concat = Concat::new(&[content_bytes]);
    let overlay = EphemeralOverlay::new(ctx.objects, &extra);
    let overlay_ctx = ctx.with_objects(Narrow::new(overlay));
    let run_res = render_stream_into(
        &overlay_ctx,
        &concat,
        Some(&res_obj),
        form_to_device,
        1,
        None,
        canvas,
        state,
        sink,
    );

    canvas.pop_clip();
    let _ = run_res?;
    Ok(true)
}
