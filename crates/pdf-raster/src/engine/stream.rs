//! Content-stream event state machine (`StreamState`, `q`/`Q`, Form XObject
//! stacks, clip unwinding, and optional-content suppression).

use std::ops::ControlFlow;

use pdf_content_streams::{Concat, FillRule, Operator};
use pdf_gfx::{
    BlendState, Event, Forms, Kind, Matrix, Opts as GfxOpts, Summary, Wants, interpret_with,
};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Object, Resolve, Streams};
use vello_cpu::Mask;
use vello_cpu::kurbo::BezPath;

use crate::canvas2d::{Canvas2d, segs_to_bez_path};
use crate::engine::mask::{SoftMaskKey, sync_soft_mask};
use crate::engine::ops::{
    accum_text_clip_only, paint_image, paint_path, paint_shading_operator, paint_text,
};
use crate::engine::sinks::{EraseSink, GfxSinkAdapter, LocalFaultSink, SharedSink};
use crate::engine::{
    RenderCtx, RenderState, check_cancelled, is_mark_hidden_by_oc, is_srgb_group_cs, rect_to_segs,
};
use crate::error::Error;
use crate::fault::Fault;
use crate::limits::LimitKind;

#[derive(Clone)]
struct SavedStateEntry {
    clips_pushed: usize,
    mask: Option<Mask>,
    smask_key: SoftMaskKey,
}

struct FormStackEntry {
    q_floor: usize,
    clips_at_entry: usize,
    pushed_bbox_clip: bool,
    pushed_layer: bool,
    prev_mask: Option<Mask>,
    prev_smask_key: SoftMaskKey,
    prev_base_ctm: Matrix,
    suppressed_oc: bool,
}

pub(crate) struct StreamState {
    q_stack: Vec<SavedStateEntry>,
    form_stack: Vec<FormStackEntry>,
    current_clips: usize,
    current_mask: Option<Mask>,
    current_smask_key: SoftMaskKey,
    stream_base_ctm: Matrix,
    oc_hidden_marks: Vec<bool>,
    oc_suppress_count: usize,
    bt_clip_bez: BezPath,
    in_bt: bool,
    page_to_device: Matrix,
    layer_depth: usize,
    uncolored_override: Option<[u8; 4]>,
}

impl StreamState {
    fn new(
        page_to_device: Matrix,
        layer_depth: usize,
        uncolored_override: Option<[u8; 4]>,
    ) -> Self {
        Self {
            q_stack: Vec::new(),
            form_stack: Vec::new(),
            current_clips: 0,
            current_mask: None,
            current_smask_key: SoftMaskKey::None,
            stream_base_ctm: Matrix::IDENTITY,
            oc_hidden_marks: Vec::new(),
            oc_suppress_count: 0,
            bt_clip_bez: BezPath::new(),
            in_bt: false,
            page_to_device,
            layer_depth,
            uncolored_override,
        }
    }

    /// Unwinds any unbalanced `q` frames above `q_floor` and any clips pushed
    /// above `clips_floor` in the current frame.
    fn unwind_clips_in_frame(&mut self, canvas: &mut Canvas2d, q_floor: usize, clips_floor: usize) {
        while self.q_stack.len() > q_floor {
            if let Some(saved) = self.q_stack.pop() {
                for _ in 0..self.current_clips {
                    canvas.pop_clip();
                }
                self.current_clips = saved.clips_pushed;
            }
        }
        while self.current_clips > clips_floor {
            canvas.pop_clip();
            self.current_clips -= 1;
        }
    }

    fn cleanup_all(&mut self, canvas: &mut Canvas2d) {
        while let Some(form) = self.form_stack.pop() {
            self.unwind_clips_in_frame(canvas, form.q_floor, form.clips_at_entry);
            if form.pushed_bbox_clip {
                canvas.pop_clip();
            }
            if form.pushed_layer {
                canvas.pop_layer();
            }
            self.current_mask = form.prev_mask;
            canvas.set_mask(self.current_mask.clone());
        }
        self.unwind_clips_in_frame(canvas, 0, 0);
        canvas.set_mask(None);
    }

    fn handle_event<'o, 'a, 'e, O, S>(
        &mut self,
        ctx: &RenderCtx<'o, 'a, O>,
        event: &Event<'e>,
        canvas: &mut Canvas2d,
        state: &mut RenderState,
        sink: &mut S,
    ) -> Result<(), Error>
    where
        O: Resolve<'o> + Streams + Copy,
        S: Sink<Fault> + ?Sized,
    {
        let objects = ctx.objects;
        let q_floor = self.form_stack.last().map_or(0, |f| f.q_floor);

        match &event.kind {
            Kind::Op => {
                if state.ops_left == 0 {
                    return Err(Error::Gfx(pdf_gfx::Error::Limit(pdf_gfx::Cap::Operators)));
                }
                state.ops_left -= 1;
                if state.ops_left & 63 == 0 {
                    check_cancelled(ctx.opts)?;
                }
                match event.at.operator() {
                    Operator::Save => {
                        self.q_stack.push(SavedStateEntry {
                            clips_pushed: self.current_clips,
                            mask: self.current_mask.clone(),
                            smask_key: self.current_smask_key.clone(),
                        });
                        self.current_clips = 0;
                    }
                    Operator::Restore => {
                        if self.q_stack.len() > q_floor
                            && let Some(saved) = self.q_stack.pop()
                        {
                            for _ in 0..self.current_clips {
                                canvas.pop_clip();
                            }
                            self.current_clips = saved.clips_pushed;
                            self.current_mask = saved.mask;
                            self.current_smask_key = saved.smask_key;
                            canvas.set_mask(self.current_mask.clone());
                        }
                    }
                    Operator::BeginText => {
                        self.in_bt = true;
                        self.bt_clip_bez.truncate(0);
                    }
                    Operator::EndText => {
                        // ISO 32000-2 §8.11.3.1: clipping operations inside hidden optional
                        // content still update the graphics state for subsequent visible content!
                        if self.in_bt && !self.bt_clip_bez.is_empty() {
                            canvas.set_anti_alias(ctx.opts.anti_alias.text);
                            canvas.push_clip(
                                &self.bt_clip_bez,
                                FillRule::NonZero,
                                self.page_to_device,
                            );
                            self.current_clips += 1;
                        }
                        self.bt_clip_bez.truncate(0);
                        self.in_bt = false;
                    }
                    _ => {}
                }
                return Ok(());
            }

            Kind::MarkedBegin(mark) => {
                let hidden = is_mark_hidden_by_oc(objects, mark, ctx.oc_state)?;
                self.oc_hidden_marks.push(hidden);
                if hidden {
                    self.oc_suppress_count += 1;
                }
                return Ok(());
            }

            Kind::MarkedEnd(_) => {
                if let Some(was_hidden) = self.oc_hidden_marks.pop()
                    && was_hidden
                {
                    self.oc_suppress_count = self.oc_suppress_count.saturating_sub(1);
                }
                return Ok(());
            }

            Kind::FormBegin(form_use) => {
                check_cancelled(ctx.opts)?;
                let hidden = if let (Some(oc), Some(oid)) = (ctx.oc_state, form_use.oc) {
                    let oc_obj = Object::from(oid);
                    !oc.is_visible(objects, &oc_obj)?
                } else {
                    false
                };
                if hidden || self.oc_suppress_count > 0 {
                    self.oc_suppress_count += 1;
                    self.form_stack.push(FormStackEntry {
                        q_floor: self.q_stack.len(),
                        clips_at_entry: self.current_clips,
                        pushed_bbox_clip: false,
                        pushed_layer: false,
                        prev_mask: self.current_mask.clone(),
                        prev_smask_key: self.current_smask_key.clone(),
                        prev_base_ctm: self.stream_base_ctm,
                        suppressed_oc: true,
                    });
                    return Ok(());
                }

                sync_soft_mask(
                    ctx,
                    event.state,
                    self.page_to_device,
                    self.layer_depth,
                    canvas,
                    state,
                    sink,
                    &mut self.current_mask,
                    &mut self.current_smask_key,
                )?;

                if matches!(event.state.blend_mode, BlendState::Other(_)) {
                    sink.fault(
                        Place::new(Some(form_use.form), None),
                        Fault::UnsupportedBlendMode,
                    );
                    state.approximated = true;
                }

                let mut pushed_bbox_clip = false;
                if let Some(bbox) = form_use.bbox {
                    let segs = rect_to_segs(bbox);
                    let (bez, _) = segs_to_bez_path(&segs);
                    canvas.set_anti_alias(ctx.opts.anti_alias.paths);
                    canvas.push_clip(
                        &bez,
                        FillRule::NonZero,
                        form_use.matrix.then(self.page_to_device),
                    );
                    pushed_bbox_clip = true;
                }

                let is_group = form_use.group.is_some();
                if let Some(ref grp) = form_use.group
                    && (!grp.isolated
                        || grp.knockout
                        || grp
                            .color_space
                            .is_some_and(|cs| !is_srgb_group_cs(objects, cs)))
                {
                    sink.fault(
                        Place::new(Some(form_use.form), None),
                        Fault::ApproximatedGroup,
                    );
                    state.approximated = true;
                }
                let pushed_layer = if is_group {
                    if self.form_stack.len() + self.layer_depth >= ctx.opts.limits.max_layer_depth {
                        if pushed_bbox_clip {
                            canvas.pop_clip();
                        }
                        return Err(Error::Limit(LimitKind::LayerDepth));
                    }
                    canvas.push_group_layer(
                        None,
                        &event.state.blend_mode,
                        event.state.fill_alpha,
                        self.current_mask.clone(),
                    );
                    canvas.set_mask(None);
                    true
                } else {
                    false
                };

                let prev_mask = self.current_mask.clone();
                let prev_smask_key = self.current_smask_key.clone();
                let prev_base = self.stream_base_ctm;

                if is_group {
                    self.current_mask = None;
                    self.current_smask_key = SoftMaskKey::None;
                }
                // `form_use.matrix` from `pdf-gfx` already maps FormSpace -> enclosing
                // stream's coordinate space (`PageSpace`), so never multiply by
                // `self.stream_base_ctm` a second time!
                self.stream_base_ctm = form_use.matrix;

                self.form_stack.push(FormStackEntry {
                    q_floor: self.q_stack.len(),
                    clips_at_entry: self.current_clips,
                    pushed_bbox_clip,
                    pushed_layer,
                    prev_mask,
                    prev_smask_key,
                    prev_base_ctm: prev_base,
                    suppressed_oc: false,
                });
                return Ok(());
            }

            Kind::FormEnd(_) => {
                if let Some(entry) = self.form_stack.pop() {
                    self.unwind_clips_in_frame(canvas, entry.q_floor, entry.clips_at_entry);
                    if entry.suppressed_oc {
                        self.oc_suppress_count = self.oc_suppress_count.saturating_sub(1);
                        self.current_mask = entry.prev_mask;
                        self.current_smask_key = entry.prev_smask_key;
                        self.stream_base_ctm = entry.prev_base_ctm;
                        canvas.set_mask(self.current_mask.clone());
                        return Ok(());
                    }
                    if entry.pushed_bbox_clip {
                        canvas.pop_clip();
                    }
                    if entry.pushed_layer {
                        canvas.pop_layer();
                    }
                    self.current_mask = entry.prev_mask;
                    self.current_smask_key = entry.prev_smask_key;
                    self.stream_base_ctm = entry.prev_base_ctm;
                    canvas.set_mask(self.current_mask.clone());
                }
                return Ok(());
            }

            _ => {}
        }

        // ISO 32000-2 §8.11.3.1: when optional content is suppressed, painting is
        // skipped, while clipping operators (`W`/`W*` and `4..=7 Tr`) still update
        // the active clipping path!
        if self.oc_suppress_count > 0 {
            match &event.kind {
                Kind::Path(drawn) => {
                    if let Some(rule) = drawn.clip {
                        let (clip_bez, _) = segs_to_bez_path(drawn.segments);
                        canvas.set_anti_alias(ctx.opts.anti_alias.paths);
                        canvas.push_clip(&clip_bez, rule, self.page_to_device);
                        self.current_clips += 1;
                    }
                }
                Kind::Text(show) => {
                    accum_text_clip_only(
                        ctx,
                        event,
                        show,
                        self.page_to_device,
                        state,
                        &mut self.bt_clip_bez,
                    )?;
                }
                _ => {}
            }
            return Ok(());
        }

        sync_soft_mask(
            ctx,
            event.state,
            self.page_to_device,
            self.layer_depth,
            canvas,
            state,
            sink,
            &mut self.current_mask,
            &mut self.current_smask_key,
        )?;

        match &event.kind {
            Kind::Path(drawn) => {
                paint_path(
                    ctx,
                    event,
                    drawn,
                    self.stream_base_ctm,
                    self.page_to_device,
                    self.layer_depth,
                    self.uncolored_override,
                    canvas,
                    state,
                    sink,
                )?;
                if let Some(rule) = drawn.clip {
                    let (clip_bez, _) = segs_to_bez_path(drawn.segments);
                    canvas.set_anti_alias(ctx.opts.anti_alias.paths);
                    canvas.push_clip(&clip_bez, rule, self.page_to_device);
                    self.current_clips += 1;
                }
            }
            Kind::Text(show) => {
                paint_text(
                    ctx,
                    event,
                    show,
                    self.stream_base_ctm,
                    self.page_to_device,
                    self.layer_depth,
                    self.uncolored_override,
                    canvas,
                    state,
                    sink,
                    &mut self.bt_clip_bez,
                )?;
            }
            Kind::Image(img) => {
                paint_image(
                    ctx,
                    event,
                    img,
                    self.stream_base_ctm,
                    self.page_to_device,
                    self.layer_depth,
                    self.uncolored_override,
                    canvas,
                    state,
                    sink,
                )?;
            }
            Kind::Shading(sh) => {
                paint_shading_operator(
                    ctx,
                    event,
                    sh.bounds,
                    self.page_to_device,
                    canvas,
                    state,
                    sink,
                )?;
            }
            _ => {}
        }

        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_stream_into<'o, 'a, 'r, O, S>(
    ctx: &RenderCtx<'o, 'a, O>,
    content: &Concat<'_>,
    resources: Option<&'r Object>,
    page_to_device: Matrix,
    layer_depth: usize,
    uncolored_override: Option<[u8; 4]>,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<Summary, Error>
where
    'o: 'r,
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    check_cancelled(ctx.opts)?;
    if layer_depth > ctx.opts.limits.max_layer_depth {
        return Err(Error::Limit(LimitKind::LayerDepth));
    }
    if layer_depth > 0 {
        if state.subrenders_left == 0 {
            return Err(Error::Gfx(pdf_gfx::Error::Limit(pdf_gfx::Cap::Operators)));
        }
        state.subrenders_left -= 1;
    }
    let local_gfx_limits = state.snapshot_gfx_limits(&ctx.opts.limits.gfx);

    let mut erased = EraseSink(sink);
    let shared_sink = SharedSink::new(&mut erased);
    let mut gfx_sink = GfxSinkAdapter(&shared_sink);

    let mut stream_state = StreamState::new(page_to_device, layer_depth, uncolored_override);
    let mut hard_error: Option<Error> = None;

    let gfx_res = interpret_with(
        ctx.objects,
        content,
        resources,
        &local_gfx_limits,
        ctx.cache.fonts(),
        GfxOpts {
            forms: Forms::Descend,
            wants: Wants::ALL,
        },
        &mut gfx_sink,
        |event: Event<'_>| {
            if hard_error.is_some() {
                return ControlFlow::Break(());
            }
            let mut local_sink = LocalFaultSink(&shared_sink);
            if let Err(err) = stream_state.handle_event(ctx, &event, canvas, state, &mut local_sink)
            {
                hard_error = Some(err);
                return ControlFlow::Break(());
            }
            ControlFlow::Continue(())
        },
    );

    stream_state.cleanup_all(canvas);

    if let Some(err) = hard_error {
        return Err(err);
    }
    let sub_summary = gfx_res?;
    state.interpreted_left = state
        .interpreted_left
        .saturating_sub(sub_summary.interpreted);
    state.decompressed_left = state
        .decompressed_left
        .saturating_sub(sub_summary.decompressed);
    Ok(sub_summary)
}
