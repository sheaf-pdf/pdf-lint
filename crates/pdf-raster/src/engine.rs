//! Core zero-I/O page, Form XObject, Type 3 glyph, soft-mask, tiling-pattern,
//! OCG, annotation, and AcroForm widget rasterization engine.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::marker::PhantomData;
use std::ops::{ControlFlow, Deref, DerefMut};
use std::sync::Arc;

#[cfg(feature = "text-layout")]
use pdf_annots::Annot;
use pdf_annots::{
    AppearanceMode, Appearances as AnnotAppearances, SynthesizedAp, TypedAnnot, page_annots_with,
    placement_matrix, stream_placement_matrix, synth_annot,
};
use pdf_content_streams::{Concat, FillRule, Operator, ResBody, ResourceNeeds, TextMode};
use pdf_fonts::{Font, FontKind};
use pdf_gfx::{
    ActiveColorSpace, BlendState, Event, Forms, ImageDrawn, ImageOf, Kind, Mark, MarkProps, Matrix,
    Opts as GfxOpts, PageGeom, PathDrawn, Point, Rect, Seg, SoftMask, SoftMaskKind, State, Summary,
    TextShow, TransparencyGroup, Wants, interpret_with,
};
use pdf_images::DecodeOptions;
use pdf_objects::fault::{Place, Sink};
use pdf_objects::{DecodedStream, Dict, Object, ObjectId, Resolve, Streams, Unreadable, Why};
use pdf_semantics::ocg::{OcState, read as read_oc_properties};
use pdf_semantics::pages::{Inheritable, Page};
use vello_cpu::Mask;
use vello_cpu::kurbo::BezPath;

use crate::builtin_glyphs::emit_builtin_glyph;
use crate::canvas2d::{Canvas2d, outline_verbs_to_bez_path, segs_to_bez_path};
use crate::error::Error;
use crate::fallback::{FontFallback, FontQuery, RenderCache};
use crate::fault::Fault;
use crate::limits::LimitKind;
use crate::opts::{Appearances, RenderMedium, RenderOpts, RenderSummary};
use crate::paint::{
    ResolvedPaint, dict_float, dict_float4, dict_int, dict_matrix, eval_solid_color,
    evaluate_shading_rect, lookup_resource_entry,
};
use crate::phases::phase;
use crate::pixmap::Pixmap;

/// Covariant lifetime-narrowing adapter over `O: Resolve<'o> + Streams + Copy`
/// that preserves `Streams::decoded_standard` while allowing local shorter-lived
/// `Dict` / `Object` resource trees to be interpreted alongside `'o` indirect
/// objects.
#[derive(Clone, Copy)]
pub(crate) struct Narrow<'o, O> {
    inner: O,
    _lt: PhantomData<&'o ()>,
}

impl<'o, O> Narrow<'o, O> {
    pub(crate) const fn new(inner: O) -> Self {
        Self {
            inner,
            _lt: PhantomData,
        }
    }
}

impl<'a, 'o: 'a, O: Resolve<'o>> Resolve<'a> for Narrow<'o, O> {
    fn resolve(&self, id: ObjectId) -> Option<&'a Object> {
        self.inner.resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'a Object>, Unreadable> {
        self.inner.try_resolve(id)
    }
}

impl<'o, O: Streams> Streams for Narrow<'o, O> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        self.inner.decoded(id, limit)
    }

    fn decoded_standard(
        &self,
        id: ObjectId,
        limit: usize,
    ) -> Result<DecodedStream<'_>, Unreadable> {
        self.inner.decoded_standard(id, limit)
    }
}

/// Overlay resolver/streams provider binding ephemeral synthesized indirect
/// objects (at high `0x7000_0000+` IDs) over `base` without mutating the
/// caller's document.
#[derive(Clone, Copy)]
struct EphemeralOverlay<'a, O> {
    base: O,
    extra: &'a BTreeMap<ObjectId, Object>,
}

impl<'o, 'a: 'o, O: Resolve<'o>> Resolve<'o> for EphemeralOverlay<'a, O> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.extra.get(&id).or_else(|| self.base.resolve(id))
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        if let Some(obj) = self.extra.get(&id) {
            return Ok(Some(obj));
        }
        self.base.try_resolve(id)
    }
}

impl<'a, O: Streams> Streams for EphemeralOverlay<'a, O> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        if let Some(obj) = self.extra.get(&id) {
            let bytes = obj
                .as_stream()
                .and_then(|s| s.data.bytes())
                .ok_or_else(|| Unreadable::new(id, Why::Damaged, "not an in-memory stream"))?;
            if bytes.len() > limit {
                return Err(Unreadable::new(
                    id,
                    Why::Limit,
                    "ephemeral stream exceeds limit",
                ));
            }
            return Ok(Cow::Borrowed(bytes));
        }
        self.base.decoded(id, limit)
    }

    fn decoded_standard(
        &self,
        id: ObjectId,
        limit: usize,
    ) -> Result<DecodedStream<'_>, Unreadable> {
        if let Some(obj) = self.extra.get(&id) {
            let bytes = obj
                .as_stream()
                .and_then(|s| s.data.bytes())
                .ok_or_else(|| Unreadable::new(id, Why::Damaged, "not an in-memory stream"))?;
            if bytes.len() > limit {
                return Err(Unreadable::new(
                    id,
                    Why::Limit,
                    "ephemeral stream exceeds limit",
                ));
            }
            return Ok(DecodedStream::new(Cow::Borrowed(bytes), None));
        }
        self.base.decoded_standard(id, limit)
    }
}

struct EraseSink<'a, S: ?Sized>(&'a mut S);

impl<S: Sink<Fault> + ?Sized> Sink<Fault> for EraseSink<'_, S> {
    fn wants(&self) -> bool {
        self.0.wants()
    }

    fn fault(&mut self, at: Place, fault: Fault) {
        self.0.fault(at, fault);
    }
}

struct SharedSink<'s> {
    wants: bool,
    inner: RefCell<&'s mut dyn Sink<Fault>>,
}

impl<'s> SharedSink<'s> {
    fn new(sink: &'s mut dyn Sink<Fault>) -> Self {
        Self {
            wants: sink.wants(),
            inner: RefCell::new(sink),
        }
    }

    fn emit(&self, at: Place, fault: Fault) {
        if self.wants
            && let Ok(mut guard) = self.inner.try_borrow_mut()
        {
            guard.fault(at, fault);
        }
    }
}

struct GfxSinkAdapter<'a, 's>(&'a SharedSink<'s>);

impl Sink<pdf_gfx::Fault> for GfxSinkAdapter<'_, '_> {
    fn wants(&self) -> bool {
        self.0.wants
    }

    fn fault(&mut self, at: Place, fault: pdf_gfx::Fault) {
        self.0.emit(at, Fault::Gfx(fault));
    }
}

struct LocalFaultSink<'a, 's>(&'a SharedSink<'s>);

impl Sink<Fault> for LocalFaultSink<'_, '_> {
    fn wants(&self) -> bool {
        self.0.wants
    }

    fn fault(&mut self, at: Place, fault: Fault) {
        self.0.emit(at, fault);
    }
}

struct ImageSinkAdapter<'a, S: ?Sized>(&'a mut S);

impl<S: Sink<Fault> + ?Sized> Sink<pdf_images::Fault> for ImageSinkAdapter<'_, S> {
    fn wants(&self) -> bool {
        self.0.wants()
    }

    fn fault(&mut self, at: Place, fault: pdf_images::Fault) {
        self.0.fault(at, Fault::Image(fault));
    }
}

struct SemSinkAdapter<'a, S: ?Sized>(&'a mut S);

impl<S: Sink<Fault> + ?Sized> Sink<pdf_semantics::Fault> for SemSinkAdapter<'_, S> {
    fn wants(&self) -> bool {
        self.0.wants()
    }

    fn fault(&mut self, at: Place, fault: pdf_semantics::Fault) {
        self.0.fault(at, Fault::Semantics(fault));
    }
}

struct AnnotSinkAdapter<'a, S: ?Sized>(&'a mut S);

impl<S: Sink<Fault> + ?Sized> Sink<pdf_annots::Fault> for AnnotSinkAdapter<'_, S> {
    fn wants(&self) -> bool {
        self.0.wants()
    }

    fn fault(&mut self, at: Place, fault: pdf_annots::Fault) {
        self.0.fault(at, Fault::Annot(fault));
    }
}

#[cfg(feature = "text-layout")]
struct FormSinkAdapter<'a, S: ?Sized>(&'a mut S);

#[cfg(feature = "text-layout")]
impl<S: Sink<Fault> + ?Sized> Sink<pdf_forms::Fault> for FormSinkAdapter<'_, S> {
    fn wants(&self) -> bool {
        self.0.wants()
    }

    fn fault(&mut self, at: Place, fault: pdf_forms::Fault) {
        self.0.fault(at, Fault::Form(fault));
    }
}

type TileCacheKey = (ObjectId, u32, u32, u8, Option<[u8; 4]>);

struct RenderState {
    summary: RenderSummary,
    ops_left: usize,
    glyphs_left: usize,
    interpreted_left: usize,
    decompressed_left: usize,
    subrenders_left: usize,
    offscreen_pixels_left: u64,
    active_streams: Vec<ObjectId>,
    tile_cache: HashMap<TileCacheKey, Arc<vello_cpu::Pixmap>>,
}

impl RenderState {
    fn new(opts: &RenderOpts) -> Self {
        Self {
            summary: RenderSummary {
                reported: true,
                ..RenderSummary::default()
            },
            ops_left: opts.limits.gfx.operators,
            glyphs_left: opts.limits.gfx.glyphs,
            interpreted_left: opts.limits.gfx.interpreted,
            decompressed_left: opts.limits.gfx.decompressed,
            subrenders_left: opts.limits.gfx.operators.max(1),
            offscreen_pixels_left: opts.limits.max_pixels.saturating_mul(4),
            active_streams: Vec::new(),
            tile_cache: HashMap::new(),
        }
    }

    fn snapshot_gfx_limits(&self, base: &pdf_gfx::Limits) -> pdf_gfx::Limits {
        let mut lim = *base;
        lim.operators = self.ops_left;
        lim.glyphs = self.glyphs_left;
        lim.interpreted = self.interpreted_left;
        lim.decompressed = self.decompressed_left;
        lim
    }

    fn charge_offscreen_pixels(&mut self, width: u32, height: u32) -> Result<(), Error> {
        let px = u64::from(width) * u64::from(height);
        if px > self.offscreen_pixels_left {
            return Err(Error::Limit(LimitKind::Pixels));
        }
        self.offscreen_pixels_left -= px;
        Ok(())
    }
}

impl Deref for RenderState {
    type Target = RenderSummary;

    fn deref(&self) -> &Self::Target {
        &self.summary
    }
}

impl DerefMut for RenderState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.summary
    }
}

/// Rasterizes `page` without collecting non-fatal [`Fault`]s.
///
/// # Errors
///
/// Returns [`Error`] on invalid viewports, exceeded [`crate::Limits`], or
/// unreadable objects/streams.
pub fn render_page<'o, O>(
    objects: O,
    catalog: Option<&'o Dict>,
    page: &Page<'o>,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
) -> Result<(Pixmap, RenderSummary), Error>
where
    O: Resolve<'o> + Streams + Copy,
{
    render_page_with(objects, catalog, page, cache, fallback, opts, &mut ())
}

/// Rasterizes `page` over `objects` with zero file I/O, reporting recoverable
/// ISO 32000-2 departures to `sink`.
///
/// Passing `&mut ()` disables fault collection and produces the exact same
/// [`Pixmap`], [`RenderSummary`], and [`Error`]s as any active sink.
///
/// # Errors
///
/// Returns [`Error`] on invalid viewports, exceeded [`crate::Limits`], or
/// unreadable objects/streams.
pub fn render_page_with<'o, O, S>(
    objects: O,
    catalog: Option<&'o Dict>,
    page: &Page<'o>,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
    sink: &mut S,
) -> Result<(Pixmap, RenderSummary), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let mut geom = PageGeom::from_page(page);
    if let Some(dict) = page.dict {
        geom = geom.with_page_boxes(objects, dict);
    }
    let resolved_vp = opts.viewport.resolve(&geom, &opts.limits)?;
    phase!(
        DEBUG,
        "render_page",
        width = resolved_vp.width,
        height = resolved_vp.height
    );

    let oc_state = load_oc_state(objects, catalog, opts, sink)?;
    let output_intent_icc = load_output_intent_icc(objects, catalog, opts)?;

    let mut canvas = Canvas2d::new(resolved_vp.width, resolved_vp.height)?;
    let mut state = RenderState::new(opts);

    // Clip to the visible `/CropBox` in PageSpace (essential for `Viewport::Tile`
    // and custom matrices, and matches PDFium's page clip).
    let crop_segs = rect_to_segs(geom.crop_box);
    let (crop_bez, _) = segs_to_bez_path(&crop_segs);
    canvas.set_anti_alias(opts.anti_alias.paths);
    canvas.push_clip(&crop_bez, FillRule::NonZero, resolved_vp.page_to_device);

    // Optional page-level `/Group << /S /Transparency /I true >>`.
    let page_group = page
        .dict
        .and_then(|d| TransparencyGroup::from_dict(objects, d));
    let pushed_page_group = if page_group.is_some_and(|g| g.isolated) {
        canvas.push_group_layer(None, &BlendState::default(), 1.0, None);
        true
    } else {
        false
    };

    // 1. Decode and draw the page's `/Contents` stream(s).
    let contents_slices = decode_page_contents(objects, page.dict, &mut state.decompressed_left)?;
    if !contents_slices.is_empty() {
        let refs: Vec<&[u8]> = contents_slices.iter().map(Vec::as_slice).collect();
        let concat = Concat::new(&refs);
        let page_resources = page.attribute(Inheritable::Resources).map(|a| a.value);
        let stream_summary = render_stream_into(
            Narrow::new(objects),
            &concat,
            page_resources,
            Matrix::IDENTITY,
            resolved_vp.page_to_device,
            0,
            None,
            oc_state.as_ref(),
            output_intent_icc.as_deref(),
            cache,
            fallback,
            opts,
            &mut canvas,
            &mut state,
            sink,
        );
        if let Err(err) = stream_summary {
            if pushed_page_group {
                canvas.pop_layer();
            }
            return Err(err);
        }
        state.gfx = stream_summary?;
    }

    if pushed_page_group {
        canvas.pop_layer();
    }

    // 2. Draw visible `/Annots` and interactive `/Subtype /Widget` overlays.
    if (opts.draw.annots || opts.draw.forms)
        && let Some(page_dict) = page.dict
    {
        draw_page_annotations(
            objects,
            catalog,
            page_dict,
            page.id,
            resolved_vp.page_to_device,
            oc_state.as_ref(),
            output_intent_icc.as_deref(),
            cache,
            fallback,
            opts,
            &mut canvas,
            &mut state,
            sink,
        )?;
    }

    canvas.pop_clip();
    let pixmap = canvas.finish(opts.background, opts.format, &opts.limits)?;
    Ok((pixmap, state.summary))
}

/// Rasterizes a standalone decoded [`Concat`] content stream (such as a Form
/// XObject, synthesized appearance stream, or `Canvas` output) without
/// collecting non-fatal [`Fault`]s.
///
/// # Errors
///
/// Returns [`Error`] on invalid viewports, exceeded [`crate::Limits`], or
/// unreadable objects/streams.
pub fn render_content<'o, O>(
    objects: O,
    content: &Concat<'_>,
    resources: Option<&'o Object>,
    geom: &PageGeom,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
) -> Result<(Pixmap, RenderSummary), Error>
where
    O: Resolve<'o> + Streams + Copy,
{
    render_content_with(
        objects,
        content,
        resources,
        geom,
        cache,
        fallback,
        opts,
        &mut (),
    )
}

/// Rasterizes a standalone decoded [`Concat`] content stream, reporting
/// recoverable ISO 32000-2 departures to `sink`.
///
/// # Errors
///
/// Same error conditions as [`render_content`].
#[allow(clippy::too_many_arguments)]
pub fn render_content_with<'o, O, S>(
    objects: O,
    content: &Concat<'_>,
    resources: Option<&'o Object>,
    geom: &PageGeom,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
    sink: &mut S,
) -> Result<(Pixmap, RenderSummary), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let resolved_vp = opts.viewport.resolve(geom, &opts.limits)?;
    let mut canvas = Canvas2d::new(resolved_vp.width, resolved_vp.height)?;
    let mut state = RenderState::new(opts);

    let crop_segs = rect_to_segs(geom.crop_box);
    let (crop_bez, _) = segs_to_bez_path(&crop_segs);
    canvas.set_anti_alias(opts.anti_alias.paths);
    canvas.push_clip(&crop_bez, FillRule::NonZero, resolved_vp.page_to_device);

    state.gfx = render_stream_into(
        Narrow::new(objects),
        content,
        resources,
        Matrix::IDENTITY,
        resolved_vp.page_to_device,
        0,
        None,
        None,
        None,
        cache,
        fallback,
        opts,
        &mut canvas,
        &mut state,
        sink,
    )?;

    canvas.pop_clip();
    let pixmap = canvas.finish(opts.background, opts.format, &opts.limits)?;
    Ok((pixmap, state.summary))
}

#[derive(Clone)]
struct SavedStateEntry {
    clips_pushed: usize,
    mask: Option<Mask>,
    smask_spec: SoftMask,
    group_overrides: GroupOverrides,
}

#[derive(Clone, Copy, Default)]
struct GroupOverrides {
    active: bool,
    ca_fill_set: bool,
    ca_stroke_set: bool,
    bm_set: bool,
    smask_set: bool,
}

struct FormStackEntry {
    q_floor: usize,
    clips_at_entry: usize,
    pushed_bbox_clip: bool,
    pushed_layer: bool,
    prev_mask: Option<Mask>,
    prev_smask_spec: SoftMask,
    prev_base_ctm: Matrix,
    prev_group_overrides: GroupOverrides,
    suppressed_oc: bool,
}

#[allow(clippy::too_many_arguments)]
fn render_stream_into<'o, 'r, O, S>(
    objects: Narrow<'o, O>,
    content: &Concat<'_>,
    resources: Option<&'r Object>,
    initial_base_ctm: Matrix,
    page_to_device: Matrix,
    layer_depth: usize,
    uncolored_override: Option<[u8; 4]>,
    oc_state: Option<&OcState>,
    output_intent_icc: Option<&[u8]>,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<Summary, Error>
where
    'o: 'r,
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    if layer_depth > opts.limits.max_layer_depth {
        return Err(Error::Limit(LimitKind::LayerDepth));
    }
    if layer_depth > 0 {
        if state.subrenders_left == 0 {
            return Err(Error::Gfx(pdf_gfx::Error::Limit(pdf_gfx::Cap::Operators)));
        }
        state.subrenders_left -= 1;
    }
    let local_gfx_limits = state.snapshot_gfx_limits(&opts.limits.gfx);

    let mut erased = EraseSink(sink);
    let shared_sink = SharedSink::new(&mut erased);
    let mut gfx_sink = GfxSinkAdapter(&shared_sink);

    let mut q_stack: Vec<SavedStateEntry> = Vec::new();
    let mut form_stack: Vec<FormStackEntry> = Vec::new();
    let mut current_clips: usize = 0;
    let mut current_mask: Option<Mask> = None;
    let mut current_smask_spec = SoftMask::None;
    let mut smask_cache: HashMap<(ObjectId, u8), Option<Mask>> = HashMap::new();
    let mut stream_base_ctm = initial_base_ctm;
    let mut group_overrides = GroupOverrides::default();

    let mut oc_hidden_marks: Vec<bool> = Vec::new();
    let mut oc_suppress_count: usize = 0;

    let mut bt_clip_bez = BezPath::new();
    let mut in_bt = false;
    let mut hard_error: Option<Error> = None;

    let gfx_res = interpret_with(
        objects,
        content,
        resources,
        &local_gfx_limits,
        cache.fonts(),
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
            if let Err(err) = handle_event(
                objects,
                &event,
                page_to_device,
                layer_depth,
                uncolored_override,
                oc_state,
                output_intent_icc,
                cache,
                fallback,
                opts,
                canvas,
                state,
                &mut local_sink,
                &mut q_stack,
                &mut form_stack,
                &mut current_clips,
                &mut current_mask,
                &mut current_smask_spec,
                &mut smask_cache,
                &mut stream_base_ctm,
                &mut group_overrides,
                &mut oc_hidden_marks,
                &mut oc_suppress_count,
                &mut bt_clip_bez,
                &mut in_bt,
            ) {
                hard_error = Some(err);
                return ControlFlow::Break(());
            }
            ControlFlow::Continue(())
        },
    );

    // Ensure every clip and group layer pushed during this stream is balanced
    // even if `interpret_with` returned an error mid-stream!
    while let Some(form) = form_stack.pop() {
        while q_stack.len() > form.q_floor {
            if let Some(saved) = q_stack.pop() {
                for _ in 0..saved.clips_pushed {
                    canvas.pop_clip();
                }
            }
        }
        while current_clips > form.clips_at_entry {
            canvas.pop_clip();
            current_clips -= 1;
        }
        if form.pushed_bbox_clip {
            canvas.pop_clip();
        }
        if form.pushed_layer {
            canvas.pop_layer();
        }
        current_mask = form.prev_mask;
        canvas.set_mask(current_mask.clone());
    }
    while let Some(saved) = q_stack.pop() {
        for _ in 0..saved.clips_pushed {
            canvas.pop_clip();
        }
    }
    for _ in 0..current_clips {
        canvas.pop_clip();
    }
    canvas.set_mask(None);

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

#[allow(clippy::too_many_arguments)]
fn handle_event<'o, 'e, O, S>(
    objects: Narrow<'o, O>,
    event: &Event<'e>,
    page_to_device: Matrix,
    layer_depth: usize,
    uncolored_override: Option<[u8; 4]>,
    oc_state: Option<&OcState>,
    output_intent_icc: Option<&[u8]>,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
    q_stack: &mut Vec<SavedStateEntry>,
    form_stack: &mut Vec<FormStackEntry>,
    current_clips: &mut usize,
    current_mask: &mut Option<Mask>,
    current_smask_spec: &mut SoftMask,
    smask_cache: &mut HashMap<(ObjectId, u8), Option<Mask>>,
    stream_base_ctm: &mut Matrix,
    group_overrides: &mut GroupOverrides,
    oc_hidden_marks: &mut Vec<bool>,
    oc_suppress_count: &mut usize,
    bt_clip_bez: &mut BezPath,
    in_bt: &mut bool,
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let q_floor = form_stack.last().map_or(0, |f| f.q_floor);

    match &event.kind {
        Kind::Op => {
            if state.ops_left == 0 {
                return Err(Error::Gfx(pdf_gfx::Error::Limit(pdf_gfx::Cap::Operators)));
            }
            state.ops_left -= 1;
            match event.at.operator() {
                Operator::Save => {
                    q_stack.push(SavedStateEntry {
                        clips_pushed: *current_clips,
                        mask: current_mask.clone(),
                        smask_spec: *current_smask_spec,
                        group_overrides: *group_overrides,
                    });
                    *current_clips = 0;
                }
                Operator::Restore => {
                    if q_stack.len() > q_floor
                        && let Some(saved) = q_stack.pop()
                    {
                        for _ in 0..*current_clips {
                            canvas.pop_clip();
                        }
                        *current_clips = saved.clips_pushed;
                        *current_mask = saved.mask;
                        *current_smask_spec = saved.smask_spec;
                        *group_overrides = saved.group_overrides;
                        canvas.set_mask(current_mask.clone());
                    }
                }
                Operator::BeginText => {
                    *in_bt = true;
                    bt_clip_bez.truncate(0);
                }
                Operator::EndText => {
                    if *in_bt && !bt_clip_bez.is_empty() && *oc_suppress_count == 0 {
                        canvas.set_anti_alias(opts.anti_alias.text);
                        canvas.push_clip(bt_clip_bez, FillRule::NonZero, page_to_device);
                        *current_clips += 1;
                    }
                    bt_clip_bez.truncate(0);
                    *in_bt = false;
                }
                Operator::ExtGState if group_overrides.active => {
                    if let Some(op_name) = event.at.operand(0).and_then(|o| o.name())
                        && let Some(gs_obj) = lookup_resource_entry(
                            objects,
                            event.at.resources(),
                            event.at.page_resources(),
                            b"ExtGState",
                            &op_name,
                        )
                    {
                        let gs_dict = match gs_obj.as_reference() {
                            Some(id) => objects.try_resolve(id)?.and_then(Object::as_dict),
                            None => gs_obj.as_dict(),
                        };
                        if let Some(gs) = gs_dict {
                            if gs.has(b"ca") {
                                group_overrides.ca_fill_set = true;
                            }
                            if gs.has(b"CA") {
                                group_overrides.ca_stroke_set = true;
                            }
                            if gs.has(b"BM") {
                                group_overrides.bm_set = true;
                            }
                            if gs.has(b"SMask") {
                                group_overrides.smask_set = true;
                            }
                        }
                    }
                }
                _ => {}
            }
            return Ok(());
        }

        Kind::MarkedBegin(mark) => {
            let hidden = is_mark_hidden_by_oc(objects, mark, oc_state)?;
            oc_hidden_marks.push(hidden);
            if hidden {
                *oc_suppress_count += 1;
            }
            return Ok(());
        }

        Kind::MarkedEnd(_) => {
            if let Some(was_hidden) = oc_hidden_marks.pop()
                && was_hidden
            {
                *oc_suppress_count = oc_suppress_count.saturating_sub(1);
            }
            return Ok(());
        }

        Kind::FormBegin(form_use) => {
            let hidden = if let (Some(oc), Some(oid)) = (oc_state, form_use.oc) {
                let oc_obj = Object::from(oid);
                !oc.is_visible(objects, &oc_obj)?
            } else {
                false
            };
            if hidden || *oc_suppress_count > 0 {
                *oc_suppress_count += 1;
                form_stack.push(FormStackEntry {
                    q_floor: q_stack.len(),
                    clips_at_entry: *current_clips,
                    pushed_bbox_clip: false,
                    pushed_layer: false,
                    prev_mask: current_mask.clone(),
                    prev_smask_spec: *current_smask_spec,
                    prev_base_ctm: *stream_base_ctm,
                    prev_group_overrides: *group_overrides,
                    suppressed_oc: true,
                });
                return Ok(());
            }

            sync_soft_mask(
                objects,
                event.state,
                *group_overrides,
                page_to_device,
                layer_depth,
                oc_state,
                output_intent_icc,
                cache,
                fallback,
                opts,
                canvas,
                state,
                sink,
                current_mask,
                current_smask_spec,
                smask_cache,
            )?;

            let eff_blend = effective_blend(event.state, *group_overrides);
            let eff_fill_alpha = effective_fill_alpha(event.state, *group_overrides);

            let mut pushed_bbox_clip = false;
            if let Some(bbox) = form_use.bbox {
                let segs = rect_to_segs(bbox);
                let (bez, _) = segs_to_bez_path(&segs);
                canvas.set_anti_alias(opts.anti_alias.paths);
                canvas.push_clip(
                    &bez,
                    FillRule::NonZero,
                    form_use.matrix.then(page_to_device),
                );
                pushed_bbox_clip = true;
            }

            let is_group = form_use.group.is_some();
            let pushed_layer = if is_group {
                if form_stack.len() + layer_depth >= opts.limits.max_layer_depth {
                    if pushed_bbox_clip {
                        canvas.pop_clip();
                    }
                    return Err(Error::Limit(LimitKind::LayerDepth));
                }
                canvas.push_group_layer(None, &eff_blend, eff_fill_alpha, current_mask.clone());
                canvas.set_mask(None);
                true
            } else {
                false
            };

            let prev_mask = current_mask.clone();
            let prev_smask = *current_smask_spec;
            let prev_base = *stream_base_ctm;
            let prev_overrides = *group_overrides;

            if is_group {
                *current_mask = None;
                *current_smask_spec = SoftMask::None;
                *group_overrides = GroupOverrides {
                    active: true,
                    ca_fill_set: false,
                    ca_stroke_set: false,
                    bm_set: false,
                    smask_set: false,
                };
            }
            *stream_base_ctm = form_use.matrix.then(*stream_base_ctm);

            form_stack.push(FormStackEntry {
                q_floor: q_stack.len(),
                clips_at_entry: *current_clips,
                pushed_bbox_clip,
                pushed_layer,
                prev_mask,
                prev_smask_spec: prev_smask,
                prev_base_ctm: prev_base,
                prev_group_overrides: prev_overrides,
                suppressed_oc: false,
            });
            return Ok(());
        }

        Kind::FormEnd(_) => {
            if let Some(entry) = form_stack.pop() {
                if entry.suppressed_oc {
                    *oc_suppress_count = oc_suppress_count.saturating_sub(1);
                    return Ok(());
                }
                while q_stack.len() > entry.q_floor {
                    if let Some(saved) = q_stack.pop() {
                        for _ in 0..*current_clips {
                            canvas.pop_clip();
                        }
                        *current_clips = saved.clips_pushed;
                    }
                }
                while *current_clips > entry.clips_at_entry {
                    canvas.pop_clip();
                    *current_clips -= 1;
                }
                if entry.pushed_bbox_clip {
                    canvas.pop_clip();
                }
                if entry.pushed_layer {
                    canvas.pop_layer();
                }
                *current_mask = entry.prev_mask;
                *current_smask_spec = entry.prev_smask_spec;
                *stream_base_ctm = entry.prev_base_ctm;
                *group_overrides = entry.prev_group_overrides;
                canvas.set_mask(current_mask.clone());
            }
            return Ok(());
        }

        _ => {}
    }

    if *oc_suppress_count > 0 {
        return Ok(());
    }

    // Synchronize `/ExtGState` `/SMask` before painting any path, glyph, image,
    // or shading.
    sync_soft_mask(
        objects,
        event.state,
        *group_overrides,
        page_to_device,
        layer_depth,
        oc_state,
        output_intent_icc,
        cache,
        fallback,
        opts,
        canvas,
        state,
        sink,
        current_mask,
        current_smask_spec,
        smask_cache,
    )?;

    match &event.kind {
        Kind::Path(drawn) => {
            paint_path(
                objects,
                event,
                drawn,
                *stream_base_ctm,
                page_to_device,
                layer_depth,
                uncolored_override,
                *group_overrides,
                oc_state,
                output_intent_icc,
                cache,
                fallback,
                opts,
                canvas,
                state,
                sink,
            )?;
            if let Some(rule) = drawn.clip {
                let (clip_bez, _) = segs_to_bez_path(drawn.segments);
                canvas.set_anti_alias(opts.anti_alias.paths);
                canvas.push_clip(&clip_bez, rule, page_to_device);
                *current_clips += 1;
            }
        }
        Kind::Text(show) => {
            paint_text(
                objects,
                event,
                show,
                *stream_base_ctm,
                page_to_device,
                layer_depth,
                uncolored_override,
                *group_overrides,
                oc_state,
                output_intent_icc,
                cache,
                fallback,
                opts,
                canvas,
                state,
                sink,
                bt_clip_bez,
            )?;
        }
        Kind::Image(img) => {
            paint_image(
                objects,
                event,
                img,
                page_to_device,
                uncolored_override,
                *group_overrides,
                oc_state,
                output_intent_icc,
                opts,
                canvas,
                state,
                sink,
            )?;
        }
        Kind::Shading(sh) => {
            paint_shading_operator(
                objects,
                event,
                sh.bounds,
                page_to_device,
                *group_overrides,
                output_intent_icc,
                opts,
                canvas,
                state,
                sink,
            )?;
        }
        _ => {}
    }

    Ok(())
}

fn effective_fill_alpha(state: &State, ov: GroupOverrides) -> f32 {
    if ov.active && !ov.ca_fill_set {
        1.0
    } else {
        state.fill_alpha
    }
}

fn effective_stroke_alpha(state: &State, ov: GroupOverrides) -> f32 {
    if ov.active && !ov.ca_stroke_set {
        1.0
    } else {
        state.stroke_alpha
    }
}

fn effective_blend(state: &State, ov: GroupOverrides) -> BlendState {
    if ov.active && !ov.bm_set {
        BlendState::default()
    } else {
        state.blend_mode.clone()
    }
}

fn effective_smask(state: &State, ov: GroupOverrides) -> SoftMask {
    if ov.active && !ov.smask_set {
        SoftMask::None
    } else {
        state.soft_mask
    }
}

#[allow(clippy::too_many_arguments)]
fn sync_soft_mask<'o, O, S>(
    objects: Narrow<'o, O>,
    gstate: &State,
    ov: GroupOverrides,
    page_to_device: Matrix,
    layer_depth: usize,
    oc_state: Option<&OcState>,
    output_intent_icc: Option<&[u8]>,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
    current_mask: &mut Option<Mask>,
    current_smask_spec: &mut SoftMask,
    smask_cache: &mut HashMap<(ObjectId, u8), Option<Mask>>,
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let desired = effective_smask(gstate, ov);
    if desired == *current_smask_spec {
        return Ok(());
    }
    *current_smask_spec = desired;
    match desired {
        SoftMask::None => {
            *current_mask = None;
            canvas.set_mask(None);
        }
        SoftMask::Mask { kind, group } => {
            let (Some(kind), Some(group_id)) = (kind, group) else {
                sink.fault(Place::new(group, None), Fault::InvalidSoftMask);
                *current_mask = None;
                canvas.set_mask(None);
                return Ok(());
            };
            if layer_depth + 1 > opts.limits.max_layer_depth {
                sink.fault(Place::new(Some(group_id), None), Fault::LayerDepthExceeded);
                state.approximated = true;
                *current_mask = None;
                canvas.set_mask(None);
                return Ok(());
            }
            let kind_tag = u8::from(kind == SoftMaskKind::Luminosity);
            let cache_usable =
                gstate.soft_mask_backdrop.is_none() && gstate.soft_mask_transfer.is_none();
            if cache_usable && let Some(cached) = smask_cache.get(&(group_id, kind_tag)) {
                *current_mask = cached.clone();
                canvas.set_mask(current_mask.clone());
                return Ok(());
            }

            let built = build_soft_mask(
                objects,
                group_id,
                kind,
                gstate.soft_mask_backdrop.as_ref().map(|c| c.as_slice()),
                gstate.soft_mask_transfer.as_deref(),
                (u32::from(canvas.width()), u32::from(canvas.height())),
                page_to_device,
                layer_depth + 1,
                oc_state,
                output_intent_icc,
                cache,
                fallback,
                opts,
                state,
                sink,
            )?;
            if cache_usable {
                smask_cache.insert((group_id, kind_tag), built.clone());
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
fn build_soft_mask<'o, O, S>(
    objects: Narrow<'o, O>,
    group_id: ObjectId,
    kind: SoftMaskKind,
    backdrop: Option<&[f32]>,
    transfer_obj: Option<&Object>,
    (vp_w, vp_h): (u32, u32),
    page_to_device: Matrix,
    layer_depth: usize,
    oc_state: Option<&OcState>,
    output_intent_icc: Option<&[u8]>,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<Option<Mask>, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    if state.active_streams.contains(&group_id) {
        sink.fault(Place::new(Some(group_id), None), Fault::InvalidSoftMask);
        return Ok(None);
    }
    let Some(group_obj) = objects.try_resolve(group_id)? else {
        sink.fault(Place::new(Some(group_id), None), Fault::InvalidSoftMask);
        return Ok(None);
    };
    let Some(stream) = group_obj.as_stream() else {
        sink.fault(
            Place::new(Some(group_id), group_obj.at()),
            Fault::InvalidSoftMask,
        );
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
            return Ok(None);
        }
        Err(e) => return Err(Error::Unreadable(e)),
    };

    let form_matrix = dict_matrix(objects, dict, b"Matrix").unwrap_or(Matrix::IDENTITY);
    let form_bbox = dict_float4(objects, dict, b"BBox").map(|[a, b, c, d]| Rect::new(a, b, c, d));
    let form_res = dict.get(b"Resources");
    let group_meta = TransparencyGroup::from_dict(objects, dict);

    // Backdrop color (`/BC`) applies to `/Luminosity` soft masks (defaulting to
    // opaque black `[0, 0, 0, 255]`, whereas `/Alpha` masks initialize to
    // transparent `[0, 0, 0, 0]`, ISO 32000-2 §11.6.5.2).
    let bg = match kind {
        SoftMaskKind::Alpha => None,
        SoftMaskKind::Luminosity => {
            if let Some(bc_vals) = backdrop {
                let cs_obj = group_meta.and_then(|g| g.color_space);
                if let Some(cs) = cs_obj {
                    let spaces = pdf_color::Spaces::read(objects, form_res)?;
                    let mut eval_opts = pdf_color::EvalOptions::from_spaces(opts.rules, spaces);
                    eval_opts.output_intent_profile = output_intent_icc;
                    let rgb = pdf_color::Space::read(objects, cs, &opts.limits.color, &mut ())?
                        .and_then(|sp| {
                            pdf_color::PreparedSpace::from_space(
                                Narrow::new(objects),
                                &sp,
                                &eval_opts,
                                &opts.limits.color,
                                &mut (),
                            )
                            .ok()
                            .flatten()
                        })
                        .and_then(|prep| {
                            prep.eval_with_rules(bc_vals, opts.rules, &opts.limits.color)
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

    state.charge_offscreen_pixels(vp_w, vp_h)?;
    let mut mask_canvas = Canvas2d::new(vp_w, vp_h)?;
    let mask_transform = form_matrix.then(page_to_device);
    if let Some(bbox) = form_bbox {
        let segs = rect_to_segs(bbox);
        let (bez, _) = segs_to_bez_path(&segs);
        mask_canvas.push_clip(&bez, FillRule::NonZero, mask_transform);
    }

    let slice: &[u8] = &data;
    let concat = Concat::new(&[slice]);
    state.active_streams.push(group_id);
    let mask_res = render_stream_into(
        objects,
        &concat,
        form_res,
        form_matrix,
        mask_transform,
        layer_depth,
        None,
        oc_state,
        output_intent_icc,
        cache,
        fallback,
        opts,
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

    // Optional `/TR` 1-in -> 1-out transfer function.
    let tr_fn = if let Some(tr) = transfer_obj {
        pdf_color::Function::read(objects, tr, opts.rules, &opts.limits.color, &mut ())?
    } else {
        None
    };

    if let Some(func) = tr_fn {
        let mut lut = [0u8; 256];
        for (i, slot) in lut.iter_mut().enumerate() {
            let x = (i as f32) / 255.0;
            let mut y = [0.0f32; 1];
            func.eval(&[x], &mut y, &opts.limits.color);
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
        Ok(Some(Mask::from_parts(alphas, w16, h16)))
    } else {
        Ok(Some(match kind {
            SoftMaskKind::Alpha => Mask::new_alpha(&vello_pix),
            SoftMaskKind::Luminosity | _ => Mask::new_luminance(&vello_pix),
        }))
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_active_paint<'o, 'e, O, S>(
    objects: Narrow<'o, O>,
    event: &Event<'e>,
    stroke: bool,
    device_bbox: Option<(u32, u32, u32, u32)>,
    stream_base_ctm: Matrix,
    page_to_device: Matrix,
    layer_depth: usize,
    uncolored_override: Option<[u8; 4]>,
    ov: GroupOverrides,
    oc_state: Option<&OcState>,
    output_intent_icc: Option<&[u8]>,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<ResolvedPaint, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let alpha = if stroke {
        effective_stroke_alpha(event.state, ov)
    } else {
        effective_fill_alpha(event.state, ov)
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
            objects,
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
            oc_state,
            output_intent_icc,
            cache,
            fallback,
            opts,
            state,
            sink,
        );
    }

    match eval_solid_color(
        objects,
        space,
        comps,
        alpha,
        event.at.resources(),
        event.at.page_resources(),
        output_intent_icc,
        opts,
        sink,
    )? {
        Some(rgba) => Ok(ResolvedPaint::Solid(rgba)),
        None => Ok(ResolvedPaint::None),
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_pattern_paint<'o, 'a, O, S>(
    objects: Narrow<'o, O>,
    pat_name: &[u8],
    underlying_space: &ActiveColorSpace,
    underlying_comps: &[f32],
    alpha: f32,
    device_bbox: Option<(u32, u32, u32, u32)>,
    resources: Option<&'a Object>,
    page_resources: Option<&'a Object>,
    stream_base_ctm: Matrix,
    page_to_device: Matrix,
    layer_depth: usize,
    oc_state: Option<&OcState>,
    output_intent_icc: Option<&[u8]>,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<ResolvedPaint, Error>
where
    'o: 'a,
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    if layer_depth + 1 > opts.limits.max_layer_depth {
        sink.fault(Place::new(None, None), Fault::LayerDepthExceeded);
        state.approximated = true;
        return Ok(ResolvedPaint::None);
    }

    let Some(pat_entry) =
        lookup_resource_entry(objects, resources, page_resources, b"Pattern", pat_name)
    else {
        sink.fault(Place::new(None, None), Fault::InvalidPattern);
        return Ok(ResolvedPaint::None);
    };
    let pat_id = pat_entry.as_reference();
    let pat_resolved = match pat_id {
        Some(id) => objects.try_resolve(id)?,
        None => Some(pat_entry),
    };
    let Some(pat_obj) = pat_resolved else {
        sink.fault(Place::new(pat_id, None), Fault::InvalidPattern);
        return Ok(ResolvedPaint::None);
    };
    let dict = match pat_obj
        .as_dict()
        .or_else(|| pat_obj.as_stream().map(|s| &s.dict))
    {
        Some(d) => d,
        None => {
            sink.fault(Place::new(pat_id, pat_obj.at()), Fault::InvalidPattern);
            return Ok(ResolvedPaint::None);
        }
    };

    let pat_type = dict_int(objects, dict, b"PatternType").unwrap_or(0);
    let pat_matrix = dict_matrix(objects, dict, b"Matrix").unwrap_or(Matrix::IDENTITY);
    // ISO 32000-2 §8.7.2: pattern space maps to the stream's initial default user
    // space (`stream_base_ctm`), then onto device pixels (`page_to_device`).
    let pat_to_device = pat_matrix.then(stream_base_ctm).then(page_to_device);
    if !pat_to_device.is_finite() || pat_to_device.determinant() == 0.0 {
        return Ok(ResolvedPaint::None);
    }

    match pat_type {
        1 => {
            let Some(sid) = pat_id else {
                sink.fault(Place::new(None, pat_obj.at()), Fault::InvalidPattern);
                return Ok(ResolvedPaint::None);
            };
            if state.active_streams.contains(&sid) {
                sink.fault(Place::new(Some(sid), None), Fault::InvalidPattern);
                return Ok(ResolvedPaint::None);
            }
            let paint_type = dict_int(objects, dict, b"PaintType").unwrap_or(1);
            let Some([bx0, by0, bx1, by1]) = dict_float4(objects, dict, b"BBox") else {
                sink.fault(Place::new(Some(sid), None), Fault::InvalidPattern);
                return Ok(ResolvedPaint::None);
            };
            let bbox = Rect::new(bx0, by0, bx1, by1);
            let x_step = dict_float(objects, dict, b"XStep").unwrap_or(0.0).abs();
            let y_step = dict_float(objects, dict, b"YStep").unwrap_or(0.0).abs();
            if bbox.is_empty() || !(x_step > 1e-4 && y_step > 1e-4) {
                sink.fault(Place::new(Some(sid), None), Fault::InvalidPattern);
                return Ok(ResolvedPaint::None);
            }

            let sx = pat_to_device.a.hypot(pat_to_device.b);
            let sy = pat_to_device.c.hypot(pat_to_device.d);
            let max_side = opts.limits.max_pattern_cells.clamp(1, 2048);
            let tile_w = ((x_step * sx).round().max(1.0) as u32).min(max_side);
            let tile_h = ((y_step * sy).round().max(1.0) as u32).min(max_side);
            if u64::from(tile_w) * u64::from(tile_h)
                > u64::from(opts.limits.max_pattern_cells) * 256
            {
                return Err(Error::Limit(LimitKind::PatternCells));
            }

            let uncolored_rgba = if paint_type == 2 {
                match eval_solid_color(
                    objects,
                    underlying_space,
                    underlying_comps,
                    alpha,
                    resources,
                    page_resources,
                    output_intent_icc,
                    opts,
                    sink,
                )? {
                    Some(c) => Some(c),
                    None => return Ok(ResolvedPaint::None),
                }
            } else {
                None
            };

            // Map pattern cell `[bbox.x0 .. bbox.x0 + x_step] × [bbox.y0 .. bbox.y0 + y_step]`
            // onto `[0..tile_w] × [0..tile_h]` (`y` flipped so upright pattern content stays upright).
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
                    return Ok(ResolvedPaint::None);
                }
                Err(e) => return Err(Error::Unreadable(e)),
            };

            state.charge_offscreen_pixels(tile_w, tile_h)?;
            let mut tile_canvas = Canvas2d::new(tile_w, tile_h)?;
            let clip_segs = rect_to_segs(bbox);
            let (clip_bez, _) = segs_to_bez_path(&clip_segs);
            tile_canvas.push_clip(&clip_bez, FillRule::NonZero, pat_to_tile);

            let slice: &[u8] = &data;
            let concat = Concat::new(&[slice]);
            let pat_res = dict.get(b"Resources");
            state.active_streams.push(sid);
            let tile_res = render_stream_into(
                objects,
                &concat,
                pat_res,
                Matrix::IDENTITY,
                pat_to_tile,
                layer_depth + 1,
                uncolored_rgba,
                oc_state,
                output_intent_icc,
                cache,
                fallback,
                opts,
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
            state.tile_cache.insert(tile_key, arc_tile.clone());
            state.patterns_drawn += 1;
            Ok(ResolvedPaint::Tiled {
                pixmap: arc_tile,
                transform: paint_transform,
            })
        }
        2 => {
            let Some(sh_obj) = dict.get(b"Shading") else {
                sink.fault(Place::new(pat_id, None), Fault::InvalidPattern);
                return Ok(ResolvedPaint::None);
            };
            let Some(bounds) = device_bbox else {
                return Ok(ResolvedPaint::None);
            };
            match evaluate_shading_rect(
                objects,
                sh_obj,
                pat_to_device,
                bounds,
                true,
                alpha,
                resources,
                page_resources,
                output_intent_icc,
                opts,
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
            Ok(ResolvedPaint::None)
        }
    }
}

fn configure_paint_on_canvas(
    canvas: &mut Canvas2d,
    paint: &ResolvedPaint,
    user_to_device: Matrix,
) -> bool {
    match paint {
        ResolvedPaint::None => false,
        ResolvedPaint::Solid(rgba) => {
            canvas.set_solid_rgba8(*rgba);
            true
        }
        ResolvedPaint::Tiled { pixmap, transform }
        | ResolvedPaint::Shading { pixmap, transform } => {
            // `vello_cpu` composes `effective_paint_transform = paint_transform.then(scene_transform)`
            // (`scene_transform * paint_transform` in kurbo column-vector notation!).
            // Let's double check `vello_cpu`'s `effective_paint_transform`!
            // When `scene_transform` is `user_to_device`, if we draw in device space (`IDENTITY`),
            // `scene_transform == IDENTITY`, so `paint_transform` is simply `*transform`!
            let paint_xf = match user_to_device.inverse() {
                Some(dev_to_user) => (*transform).then(dev_to_user),
                None => *transform,
            };
            canvas.set_tiling_paint(pixmap.clone(), paint_xf);
            true
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_path<'o, 'e, O, S>(
    objects: Narrow<'o, O>,
    event: &Event<'e>,
    drawn: &PathDrawn<'e>,
    stream_base_ctm: Matrix,
    page_to_device: Matrix,
    layer_depth: usize,
    uncolored_override: Option<[u8; 4]>,
    ov: GroupOverrides,
    oc_state: Option<&OcState>,
    output_intent_icc: Option<&[u8]>,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
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

    canvas.set_anti_alias(opts.anti_alias.paths);
    canvas.set_blend_mode(&eff_blend);

    if let Some(rule) = fills {
        let paint = resolve_active_paint(
            objects,
            event,
            false,
            dev_bbox,
            stream_base_ctm,
            page_to_device,
            layer_depth,
            uncolored_override,
            ov,
            oc_state,
            output_intent_icc,
            cache,
            fallback,
            opts,
            state,
            sink,
        )?;
        // Fill directly in device space (`IDENTITY` transform) so pattern/shading
        // `paint_transform` maps tile pixels straight to device pixels with zero
        // singular-CTM inversion edge cases!
        canvas.set_transform(page_to_device);
        if configure_paint_on_canvas(canvas, &paint, page_to_device) {
            let (fill_bez, _) = segs_to_bez_path(drawn.segments);
            canvas.set_fill_rule(rule);
            canvas.fill_bez_path(&fill_bez);
        }
    }

    if strokes && user_to_device.determinant() != 0.0 {
        let paint = resolve_active_paint(
            objects,
            event,
            true,
            dev_bbox,
            stream_base_ctm,
            page_to_device,
            layer_depth,
            uncolored_override,
            ov,
            oc_state,
            output_intent_icc,
            cache,
            fallback,
            opts,
            state,
            sink,
        )?;
        // Stroke `user_segments` under `user_to_device` (`ctm * page_to_device`)
        // so `line_width` and `DashPattern` transform anisotropically with CTM!
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

#[allow(clippy::too_many_arguments)]
fn paint_text<'o, 'e, O, S>(
    objects: Narrow<'o, O>,
    event: &Event<'e>,
    show: &TextShow<'e>,
    stream_base_ctm: Matrix,
    page_to_device: Matrix,
    layer_depth: usize,
    uncolored_override: Option<[u8; 4]>,
    ov: GroupOverrides,
    oc_state: Option<&OcState>,
    output_intent_icc: Option<&[u8]>,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
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

    // Type 3 fonts execute per-glyph content streams (`d0` colored vs `d1`
    // uncolored stencil).
    if font.kind() == FontKind::Type3 {
        return paint_type3_text(
            objects,
            event,
            show,
            font,
            stream_base_ctm,
            page_to_device,
            layer_depth,
            uncolored_override,
            ov,
            oc_state,
            output_intent_icc,
            cache,
            fallback,
            opts,
            canvas,
            state,
            sink,
        );
    }

    // Compute effective device pixel-per-em (`ppem`) for TrueType hinting.
    let trm_dev = show.trm().then(page_to_device);
    let ppem_y = trm_dev.c.hypot(trm_dev.d);
    let hint_ppem = (opts.hint_fonts && opts.anti_alias.text && ppem_y >= 4.0).then_some(ppem_y);

    // Base glyph-unit (`1000.0` = 1 em) -> text-matrix space scale.
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
            let used_external = if let (Some(fb), Some(ch)) = (fallback, decoded_char) {
                let query = FontQuery::from_font(font);
                if let Some(prog) = cache.fallback_program(fb, query, &opts.limits.gfx.fonts) {
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

        // Map `1000`-unit glyph space -> user space (`glyph_to_user`) and
        // PageSpace (`glyph_to_page`).
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
    canvas.set_anti_alias(opts.anti_alias.text);
    canvas.set_blend_mode(&eff_blend);

    if fills && !run_bez.is_empty() {
        let paint = resolve_active_paint(
            objects,
            event,
            false,
            dev_bbox,
            stream_base_ctm,
            page_to_device,
            layer_depth,
            uncolored_override,
            ov,
            oc_state,
            output_intent_icc,
            cache,
            fallback,
            opts,
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
            objects,
            event,
            true,
            dev_bbox,
            stream_base_ctm,
            page_to_device,
            layer_depth,
            uncolored_override,
            ov,
            oc_state,
            output_intent_icc,
            cache,
            fallback,
            opts,
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
fn paint_type3_text<'o, 'e, O, S>(
    objects: Narrow<'o, O>,
    event: &Event<'e>,
    show: &TextShow<'e>,
    font: &Font,
    stream_base_ctm: Matrix,
    page_to_device: Matrix,
    layer_depth: usize,
    uncolored_override: Option<[u8; 4]>,
    ov: GroupOverrides,
    oc_state: Option<&OcState>,
    output_intent_icc: Option<&[u8]>,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    if layer_depth + 1 > opts.limits.max_layer_depth {
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

    // Pre-evaluate active fill color once for `d1` uncolored Type 3 glyphs.
    let d1_color = match uncolored_override {
        Some(c) => Some(c),
        None => eval_solid_color(
            objects,
            &event.state.fill_color_space,
            event.state.fill_color.as_slice(),
            effective_fill_alpha(event.state, ov),
            event.at.resources(),
            event.at.page_resources(),
            output_intent_icc,
            opts,
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
            objects,
            &concat,
            type3_res,
            stream_base_ctm,
            glyph_to_device,
            layer_depth + 1,
            override_color,
            oc_state,
            output_intent_icc,
            cache,
            fallback,
            opts,
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
fn paint_image<'o, 'e, O, S>(
    objects: Narrow<'o, O>,
    event: &Event<'e>,
    img: &ImageDrawn,
    page_to_device: Matrix,
    uncolored_override: Option<[u8; 4]>,
    ov: GroupOverrides,
    oc_state: Option<&OcState>,
    output_intent_icc: Option<&[u8]>,
    opts: &RenderOpts,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    if let (Some(oc), Some(oid)) = (oc_state, img.oc) {
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
    let stencil_rgba = match uncolored_override {
        Some([r, g, b, a]) => [
            r,
            g,
            b,
            ((f32::from(a) / 255.0) * fill_alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        ],
        None => match eval_solid_color(
            objects,
            &event.state.fill_color_space,
            event.state.fill_color.as_slice(),
            fill_alpha,
            event.at.resources(),
            event.at.page_resources(),
            output_intent_icc,
            opts,
            sink,
        )? {
            Some(c) => c,
            None if img.stencil => return Ok(()),
            None => [0, 0, 0, 255],
        },
    };

    let active_res = event.at.resources().or_else(|| event.at.page_resources());
    let mut dec_opts = DecodeOptions::from_resources(objects, opts.rules, active_res)?;
    dec_opts.color.output_intent_profile = output_intent_icc;
    dec_opts.stencil_color = [stencil_rgba[0], stencil_rgba[1], stencil_rgba[2], 255];

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
    let opacity = if img.stencil {
        f32::from(stencil_rgba[3]) / 255.0
    } else if let Some([ur, ug, ub, ua]) = uncolored_override {
        // Inside an uncolored Type 3 (`d1`) or uncolored tiling pattern, tint
        // image pixels by the override color.
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

    let eff_blend = effective_blend(event.state, ov);
    let interpolate = img.interpolate && opts.anti_alias.images;
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
fn paint_shading_operator<'o, 'e, O, S>(
    objects: Narrow<'o, O>,
    event: &Event<'e>,
    bounds_in_page: Option<Rect>,
    page_to_device: Matrix,
    ov: GroupOverrides,
    output_intent_icc: Option<&[u8]>,
    opts: &RenderOpts,
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
        objects,
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
    // Note: ISO 32000-2 §8.7.4.3 Table 76 specifies that `/Background` is NOT
    // applied when painting with `sh` (`include_background = false`), only when
    // used inside a `PatternType 2` shading pattern!
    let Some((pixmap, transform)) = evaluate_shading_rect(
        objects,
        sh_obj,
        shading_to_device,
        dev_bounds,
        false,
        alpha,
        event.at.resources(),
        event.at.page_resources(),
        output_intent_icc,
        opts,
        sink,
    )?
    else {
        return Ok(());
    };

    let eff_blend = effective_blend(event.state, ov);
    canvas.set_anti_alias(opts.anti_alias.paths);
    canvas.set_blend_mode(&eff_blend);
    canvas.set_transform(Matrix::IDENTITY);
    canvas.set_tiling_paint(pixmap, transform);
    let (x0, y0, x1, y1) = dev_bounds;
    let rect_segs = rect_to_segs(Rect::new(x0 as f32, y0 as f32, x1 as f32, y1 as f32));
    let (bez, _) = segs_to_bez_path(&rect_segs);
    canvas.set_fill_rule(FillRule::NonZero);
    canvas.fill_bez_path(&bez);
    state.shadings_drawn += 1;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_page_annotations<'o, O, S>(
    objects: O,
    #[cfg_attr(not(feature = "text-layout"), allow(unused_variables))] catalog: Option<&'o Dict>,
    page_dict: &'o Dict,
    page_id: Option<ObjectId>,
    page_to_device: Matrix,
    oc_state: Option<&OcState>,
    output_intent_icc: Option<&[u8]>,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<(), Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
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

    #[cfg(feature = "text-layout")]
    let mut lazy_form: Option<Option<pdf_forms::Form<'o>>> = None;

    for annot in &annots {
        let visible = match opts.medium {
            RenderMedium::Display => annot.is_visible_on_screen(),
            RenderMedium::Print => annot.is_printable(),
        };
        if !visible {
            continue;
        }
        // Optional `/OC` membership on the annotation dictionary (ISO 32000-2 §12.5.2).
        if let (Some(oc), Some(oc_entry)) = (oc_state, annot.dict().get(b"OC"))
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

        // Check written `/AP /N` first.
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
                objects,
                stream_id,
                rect,
                page_to_device,
                oc_state,
                output_intent_icc,
                cache,
                fallback,
                opts,
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

        // Never synthesize unapplied `/Subtype /Redact` proposal overlays onto a
        // normal page render unless they had an explicit `/AP /N`.
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
                        objects,
                        form,
                        annot,
                        page_to_device,
                        oc_state,
                        output_intent_icc,
                        cache,
                        fallback,
                        opts,
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
                objects,
                &synth.content,
                &synth.needs,
                synth.bbox,
                synth.matrix,
                rect,
                page_to_device,
                oc_state,
                output_intent_icc,
                cache,
                fallback,
                opts,
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

#[allow(clippy::too_many_arguments)]
fn draw_written_appearance_stream<'o, O, S>(
    objects: O,
    stream_id: ObjectId,
    annot_rect: Rect,
    page_to_device: Matrix,
    oc_state: Option<&OcState>,
    output_intent_icc: Option<&[u8]>,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
    canvas: &mut Canvas2d,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<bool, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
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
        Err(e) if matches!(e.why, Why::Damaged | Why::Filter) => return Ok(false),
        Err(e) => return Err(Error::Unreadable(e)),
    };

    let form_to_page = form_matrix.then(place);
    let form_to_device = form_to_page.then(page_to_device);
    if !form_to_device.is_finite() || form_to_device.determinant() == 0.0 {
        return Ok(false);
    }

    let group = TransparencyGroup::from_dict(objects, &stream.dict);
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
        Narrow::new(objects),
        &concat,
        res,
        form_to_page,
        form_to_device,
        1,
        None,
        oc_state,
        output_intent_icc,
        cache,
        fallback,
        opts,
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
#[allow(clippy::too_many_arguments)]
fn draw_synthesized_widget<'o, O, S>(
    objects: O,
    form: &pdf_forms::Form<'o>,
    annot: &Annot<'o>,
    page_to_device: Matrix,
    oc_state: Option<&OcState>,
    output_intent_icc: Option<&[u8]>,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
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
        objects,
        &ap.content,
        &ap.needs,
        ap.bbox,
        ap.matrix,
        annot.rect(),
        page_to_device,
        oc_state,
        output_intent_icc,
        cache,
        fallback,
        opts,
        canvas,
        state,
        sink,
    )
}

#[allow(clippy::too_many_arguments)]
fn draw_ephemeral_appearance<'o, O, S>(
    objects: O,
    content_bytes: &[u8],
    needs: &ResourceNeeds,
    bbox: Rect,
    form_matrix: Matrix,
    annot_rect: Rect,
    page_to_device: Matrix,
    oc_state: Option<&OcState>,
    output_intent_icc: Option<&[u8]>,
    cache: &RenderCache,
    fallback: Option<&dyn FontFallback>,
    opts: &RenderOpts,
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

    // Materialize `needs` into an ephemeral `/Resources` dictionary + temporary
    // high-numbered object map (`0x7000_0000+`) without touching `objects`.
    let mut extra = BTreeMap::<ObjectId, Object>::new();
    let mut next_ephemeral: u32 = 0x7000_0000;
    let synth = SynthesizedAp::drawn(bbox, form_matrix, pdf_gfx::Canvas::new());
    let _ = synth;
    let mut res_dict = Dict::new();
    for need in &needs.0 {
        let entry_obj = match need.res.body() {
            ResBody::Written(id) => Object::from(*id),
            ResBody::Objects(objs) => {
                if objs.is_empty() {
                    continue;
                }
                let base_num = next_ephemeral;
                next_ephemeral = next_ephemeral.saturating_add(objs.len() as u32 + 1);
                for (idx, raw_obj) in objs.iter().enumerate() {
                    let mut cloned = raw_obj.clone();
                    cloned.renumber(|to| {
                        (to.generation == 0 && (to.number as usize) < objs.len())
                            .then(|| ObjectId::new(base_num + to.number, 0))
                    });
                    extra.insert(ObjectId::new(base_num + (idx as u32), 0), cloned);
                }
                Object::from(ObjectId::new(base_num, 0))
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
    canvas.set_anti_alias(opts.anti_alias.paths);
    canvas.push_clip(&clip_bez, FillRule::NonZero, form_to_device);

    let concat = Concat::new(&[content_bytes]);
    let overlay = EphemeralOverlay {
        base: Narrow::new(objects),
        extra: &extra,
    };
    let run_res = render_stream_into(
        Narrow::new(overlay),
        &concat,
        Some(&res_obj),
        form_to_page,
        form_to_device,
        1,
        None,
        oc_state,
        output_intent_icc,
        cache,
        fallback,
        opts,
        canvas,
        state,
        sink,
    );

    canvas.pop_clip();
    let _ = run_res?;
    Ok(true)
}

fn decode_page_contents<'o, O: Resolve<'o> + Streams + Copy>(
    objects: O,
    page_dict: Option<&'o Dict>,
    decompressed_budget: &mut usize,
) -> Result<Vec<Vec<u8>>, Error> {
    let Some(dict) = page_dict else {
        return Ok(Vec::new());
    };
    let Some(raw_contents) = dict.get(b"Contents") else {
        return Ok(Vec::new());
    };

    let mut stream_ids = Vec::new();
    if let Some(id) = raw_contents.as_reference() {
        let Some(resolved) = objects.try_resolve(id)? else {
            return Ok(Vec::new());
        };
        if resolved.as_stream().is_some() {
            stream_ids.push(id);
        } else if let Some(arr) = resolved.as_array() {
            for item in arr.iter().take(4096) {
                if let Some(sid) = item.as_reference() {
                    stream_ids.push(sid);
                }
            }
        }
    } else if let Some(arr) = raw_contents.as_array() {
        for item in arr.iter().take(4096) {
            if let Some(sid) = item.as_reference() {
                stream_ids.push(sid);
            }
        }
    }

    let mut slices = Vec::with_capacity(stream_ids.len());
    for (idx, sid) in stream_ids.into_iter().enumerate() {
        match objects.decoded(sid, *decompressed_budget) {
            Ok(bytes) => {
                *decompressed_budget = decompressed_budget.saturating_sub(bytes.len());
                let mut owned = bytes.into_owned();
                if idx + 1 < slices.capacity() && !owned.ends_with(b"\n") && !owned.ends_with(b" ")
                {
                    owned.push(b'\n');
                }
                slices.push(owned);
            }
            Err(u) if u.why == Why::Limit => {
                return Err(Error::Gfx(pdf_gfx::Error::Limit(
                    pdf_gfx::Cap::Decompressed,
                )));
            }
            Err(u) if matches!(u.why, Why::Damaged | Why::Filter) => continue,
            Err(u) => return Err(Error::Unreadable(u)),
        }
    }
    Ok(slices)
}

fn load_oc_state<'o, O, S>(
    objects: O,
    catalog: Option<&'o Dict>,
    opts: &RenderOpts,
    sink: &mut S,
) -> Result<Option<OcState>, Error>
where
    O: Resolve<'o> + Copy,
    S: Sink<Fault> + ?Sized,
{
    let Some(cat) = catalog else {
        return Ok(None);
    };
    let mut ssink = SemSinkAdapter(sink);
    let Some(props) =
        read_oc_properties(objects, cat, opts.rules, &opts.limits.semantics, &mut ssink)?
    else {
        return Ok(None);
    };
    let mut state = props.default_state();

    // Honor `/Usage` `/View` / `/Print` overrides when present on `/OCGs`
    // (ISO 32000-2 §8.11.4.4 Table 101).
    for grp in &props.groups {
        let Some(grp_dict) = objects.try_resolve(grp.id)?.and_then(Object::as_dict) else {
            continue;
        };
        let Some(usage) = grp_dict
            .get(b"Usage")
            .and_then(|u| match u.as_reference() {
                Some(id) => objects.resolve(id),
                None => Some(u),
            })
            .and_then(Object::as_dict)
        else {
            continue;
        };
        let (sub_key, state_key): (&[u8], &[u8]) = match opts.medium {
            RenderMedium::Display => (b"View", b"ViewState"),
            RenderMedium::Print => (b"Print", b"PrintState"),
        };
        if let Some(sub) = usage
            .get(sub_key)
            .and_then(|v| match v.as_reference() {
                Some(id) => objects.resolve(id),
                None => Some(v),
            })
            .and_then(Object::as_dict)
            && let Some(st_name) = sub.get(state_key).and_then(Object::as_name)
        {
            match st_name {
                b"OFF" => state.set_ocg(grp.id, false),
                b"ON" => state.set_ocg(grp.id, true),
                _ => {}
            }
        }
    }

    Ok(Some(state))
}

fn is_mark_hidden_by_oc<'a, R: Resolve<'a> + Copy>(
    objects: R,
    mark: &Mark,
    oc_state: Option<&OcState>,
) -> Result<bool, Error> {
    let Some(oc) = oc_state else {
        return Ok(false);
    };
    if mark.tag != b"OC" {
        return Ok(false);
    }
    let narrow = Narrow::new(objects);
    if let Some(oid) = mark.oc {
        let oc_obj = Object::from(oid);
        return Ok(!oc.is_visible(narrow, &oc_obj)?);
    }
    if let Some(dict) = mark.props.as_ref().and_then(MarkProps::dict) {
        let inline_obj = Object::from(dict.clone());
        return Ok(!oc.is_visible(narrow, &inline_obj)?);
    }
    Ok(false)
}

fn load_output_intent_icc<'o, O: Resolve<'o> + Streams + Copy>(
    objects: O,
    catalog: Option<&'o Dict>,
    opts: &RenderOpts,
) -> Result<Option<Vec<u8>>, Error> {
    if opts.rules != pdf_objects::pdf_tokenizer::Rules::Iso {
        return Ok(None);
    }
    let Some(cat) = catalog else {
        return Ok(None);
    };
    let Some(oi_raw) = cat.get(b"OutputIntents") else {
        return Ok(None);
    };
    let oi_arr = match oi_raw.as_reference() {
        Some(id) => objects.try_resolve(id)?.and_then(Object::as_array),
        None => oi_raw.as_array(),
    };
    let Some(arr) = oi_arr else {
        return Ok(None);
    };
    for item in arr.iter().take(16) {
        let dict = match item.as_reference() {
            Some(id) => objects.try_resolve(id)?.and_then(Object::as_dict),
            None => item.as_dict(),
        };
        let Some(d) = dict else {
            continue;
        };
        let Some(prof_id) = d.get(b"DestOutputProfile").and_then(Object::as_reference) else {
            continue;
        };
        match objects.decoded(prof_id, opts.limits.color.stream_bytes) {
            Ok(bytes) => return Ok(Some(bytes.into_owned())),
            Err(e) if matches!(e.why, Why::Damaged | Why::Filter | Why::Limit) => continue,
            Err(e) => return Err(Error::Unreadable(e)),
        }
    }
    Ok(None)
}

fn rect_to_segs(r: Rect) -> [Seg; 6] {
    [
        Seg::Move(Point::new(r.x0, r.y0)),
        Seg::Line(Point::new(r.x1, r.y0)),
        Seg::Line(Point::new(r.x1, r.y1)),
        Seg::Line(Point::new(r.x0, r.y1)),
        Seg::Line(Point::new(r.x0, r.y0)),
        Seg::Close,
    ]
}

fn clamp_device_rect(r: Rect, vp_w: u32, vp_h: u32) -> Option<(u32, u32, u32, u32)> {
    if !r.is_finite() {
        return None;
    }
    let x0 = r.x0.floor().max(0.0);
    let y0 = r.y0.floor().max(0.0);
    let x1 = r.x1.ceil().min(vp_w as f32);
    let y1 = r.y1.ceil().min(vp_h as f32);
    if !(x0 < x1 && y0 < y1) {
        return None;
    }
    Some((x0 as u32, y0 as u32, x1 as u32, y1 as u32))
}
