//! Core zero-I/O page, Form XObject, Type 3 glyph, soft-mask, tiling-pattern,
//! OCG, annotation, and AcroForm widget rasterization engine.

pub(crate) mod annots;
pub(crate) mod mask;
pub(crate) mod ops;
pub(crate) mod sinks;
pub(crate) mod stream;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use pdf_content_streams::{Concat, FillRule};
use pdf_gfx::{BlendState, Mark, MarkProps, PageGeom, Point, Rect, Seg, TransparencyGroup};
use pdf_objects::Why;
use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Dict, Object, ObjectId, Resolve, Streams};
use pdf_semantics::ocg::{OcEvent, OcState, read as read_oc_properties};
use pdf_semantics::pages::{Inheritable, Page};

use crate::canvas2d::{Canvas2d, segs_to_bez_path};
use crate::engine::annots::draw_page_annotations;
use crate::engine::mask::SoftMaskCacheKey;
use crate::engine::sinks::{Narrow, SemSinkAdapter};
use crate::engine::stream::render_stream_into;
use crate::error::Error;
use crate::fallback::{FontFallback, RenderCache};
use crate::fault::Fault;
use crate::limits::LimitKind;
use crate::opts::{RenderMedium, RenderOpts, RenderSummary};
use crate::paint::{ActiveColorKey, resolve_opt};
use crate::phases::phase;
use crate::pixmap::Pixmap;

pub(crate) type TileCacheKey = (ObjectId, u32, u32, u8, Option<[u8; 4]>);
pub(crate) type ImageCacheKey = (ObjectId, Option<[u8; 4]>);
pub(crate) type ColorCacheKey = (usize, usize, ActiveColorKey);

#[inline]
pub(crate) fn check_cancelled(opts: &RenderOpts) -> Result<(), Error> {
    if opts
        .stop
        .as_ref()
        .is_some_and(|flag| flag.load(Ordering::Relaxed))
    {
        return Err(Error::Cancelled);
    }
    Ok(())
}

pub(crate) fn is_srgb_group_cs<'o, R: Resolve<'o> + Copy>(objects: R, cs_obj: &'o Object) -> bool {
    let Ok(Some(resolved)) = resolve_opt(objects, cs_obj) else {
        return true;
    };
    if let Some(name) = resolved.as_name() {
        return matches!(name, b"DeviceRGB");
    }
    if let Some(arr) = resolved.as_array()
        && let Some(first) = arr.first()
        && let Ok(Some(head)) = resolve_opt(objects, first)
    {
        return head.as_name() == Some(b"DeviceRGB");
    }
    false
}

/// Immutable shared context threaded through stream, soft-mask, pattern, text,
/// and annotation sub-renders.
pub(crate) struct RenderCtx<'o, 'a, O> {
    pub(crate) objects: Narrow<'o, O>,
    pub(crate) oc_state: Option<&'a OcState>,
    pub(crate) output_intent_icc: Option<&'a [u8]>,
    pub(crate) cache: &'a RenderCache,
    pub(crate) fallback: Option<&'a dyn FontFallback>,
    pub(crate) opts: &'a RenderOpts,
}

impl<'o, 'a, O: Copy> Clone for RenderCtx<'o, 'a, O> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'o, 'a, O: Copy> Copy for RenderCtx<'o, 'a, O> {}

impl<'o, 'a, O> RenderCtx<'o, 'a, O> {
    pub(crate) fn with_objects<'n, O2>(&self, objects: Narrow<'n, O2>) -> RenderCtx<'n, 'a, O2> {
        RenderCtx {
            objects,
            oc_state: self.oc_state,
            output_intent_icc: self.output_intent_icc,
            cache: self.cache,
            fallback: self.fallback,
            opts: self.opts,
        }
    }
}

pub(crate) struct RenderState {
    pub(crate) summary: RenderSummary,
    pub(crate) ops_left: usize,
    pub(crate) glyphs_left: usize,
    pub(crate) interpreted_left: usize,
    pub(crate) decompressed_left: usize,
    pub(crate) subrenders_left: usize,
    pub(crate) offscreen_pixels_left: u64,
    pub(crate) scene_bytes_used: usize,
    pub(crate) active_streams: Vec<ObjectId>,
    pub(crate) tile_cache: HashMap<TileCacheKey, Arc<vello_cpu::Pixmap>>,
    pub(crate) image_cache: HashMap<ImageCacheKey, Arc<pdf_images::RgbaImage>>,
    pub(crate) smask_cache: HashMap<SoftMaskCacheKey, Option<vello_cpu::Mask>>,
    pub(crate) type3_stream_cache: HashMap<ObjectId, Arc<[u8]>>,
    pub(crate) color_cache: HashMap<ColorCacheKey, Option<pdf_color::PreparedSpace>>,
    pub(crate) warned_builtin_fonts: HashSet<usize>,
}

impl RenderState {
    pub(crate) fn new(opts: &RenderOpts) -> Self {
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
            scene_bytes_used: 0,
            active_streams: Vec::new(),
            tile_cache: HashMap::new(),
            image_cache: HashMap::new(),
            smask_cache: HashMap::new(),
            type3_stream_cache: HashMap::new(),
            color_cache: HashMap::new(),
            warned_builtin_fonts: HashSet::new(),
        }
    }

    pub(crate) fn snapshot_gfx_limits(&self, base: &pdf_gfx::Limits) -> pdf_gfx::Limits {
        let mut lim = *base;
        lim.operators = self.ops_left;
        lim.glyphs = self.glyphs_left;
        lim.interpreted = self.interpreted_left;
        lim.decompressed = self.decompressed_left;
        lim
    }

    pub(crate) fn charge_offscreen_pixels(
        &mut self,
        width: u32,
        height: u32,
        max_scene_bytes: usize,
    ) -> Result<(), Error> {
        let px = u64::from(width) * u64::from(height);
        if px > self.offscreen_pixels_left {
            return Err(Error::Limit(LimitKind::Pixels));
        }
        let bytes = px
            .checked_mul(4)
            .and_then(|b| usize::try_from(b).ok())
            .ok_or(Error::Limit(LimitKind::SceneBytes))?;
        if bytes > max_scene_bytes {
            return Err(Error::Limit(LimitKind::SceneBytes));
        }
        self.offscreen_pixels_left -= px;
        Ok(())
    }

    pub(crate) fn note_cached_bytes(&mut self, bytes: usize, max_scene_bytes: usize) -> bool {
        if bytes > max_scene_bytes {
            return false;
        }
        if self.scene_bytes_used.saturating_add(bytes) > max_scene_bytes {
            self.tile_cache.clear();
            self.image_cache.clear();
            self.smask_cache.clear();
            self.type3_stream_cache.clear();
            self.scene_bytes_used = 0;
        }
        self.scene_bytes_used += bytes;
        true
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

/// Builder-style zero-I/O rasterizer binding [`RenderOpts`] with an optional
/// shared [`RenderCache`] and [`FontFallback`] provider.
#[derive(Clone, Copy)]
pub struct Renderer<'a> {
    opts: &'a RenderOpts,
    cache: Option<&'a RenderCache>,
    fallback: Option<&'a dyn FontFallback>,
}

impl fmt::Debug for Renderer<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Renderer")
            .field("opts", &self.opts)
            .field("has_cache", &self.cache.is_some())
            .field("has_fallback", &self.fallback.is_some())
            .finish()
    }
}

impl<'a> Renderer<'a> {
    /// Creates a [`Renderer`] configured with `opts`.
    #[must_use]
    pub const fn new(opts: &'a RenderOpts) -> Self {
        Self {
            opts,
            cache: None,
            fallback: None,
        }
    }

    /// Reuses a cross-page [`RenderCache`] across renders.
    #[must_use]
    pub const fn with_cache(mut self, cache: &'a RenderCache) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Attaches a [`FontFallback`] provider for unembedded fonts.
    #[must_use]
    pub const fn with_fallback(mut self, fallback: &'a dyn FontFallback) -> Self {
        self.fallback = Some(fallback);
        self
    }

    /// Attaches an optional [`FontFallback`] provider.
    #[must_use]
    pub const fn with_fallback_opt(mut self, fallback: Option<&'a dyn FontFallback>) -> Self {
        self.fallback = fallback;
        self
    }

    /// Rasterizes `page` without collecting non-fatal [`Fault`]s.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] on invalid viewports, cancellation, exceeded
    /// [`crate::Limits`], or unreadable objects/streams.
    pub fn render_page<'o, O>(
        &self,
        objects: O,
        catalog: Option<&'o Dict>,
        page: &Page<'o>,
    ) -> Result<(Pixmap, RenderSummary), Error>
    where
        O: Resolve<'o> + Streams + Copy,
    {
        self.render_page_with(objects, catalog, page, &mut ())
    }

    /// Rasterizes `page` over `objects` with zero file I/O, reporting
    /// recoverable ISO 32000-2 departures and visual approximations to `sink`.
    ///
    /// # Errors
    ///
    /// Same error conditions as [`Renderer::render_page`].
    pub fn render_page_with<'o, O, S>(
        &self,
        objects: O,
        catalog: Option<&'o Dict>,
        page: &Page<'o>,
        sink: &mut S,
    ) -> Result<(Pixmap, RenderSummary), Error>
    where
        O: Resolve<'o> + Streams + Copy,
        S: Sink<Fault> + ?Sized,
    {
        let owned_cache;
        let cache = match self.cache {
            Some(c) => c,
            None => {
                owned_cache = RenderCache::new();
                &owned_cache
            }
        };
        render_page_with(
            objects,
            catalog,
            page,
            cache,
            self.fallback,
            self.opts,
            sink,
        )
    }

    /// Rasterizes a standalone decoded [`Concat`] content stream without
    /// collecting non-fatal [`Fault`]s.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] on invalid viewports, cancellation, exceeded
    /// [`crate::Limits`], or unreadable objects/streams.
    pub fn render_content<'o, O>(
        &self,
        objects: O,
        content: &Concat<'_>,
        resources: Option<&'o Object>,
        geom: &PageGeom,
    ) -> Result<(Pixmap, RenderSummary), Error>
    where
        O: Resolve<'o> + Streams + Copy,
    {
        self.render_content_with(objects, content, resources, geom, &mut ())
    }

    /// Rasterizes a standalone decoded [`Concat`] content stream, reporting
    /// recoverable ISO 32000-2 departures and visual approximations to `sink`.
    ///
    /// # Errors
    ///
    /// Same error conditions as [`Renderer::render_content`].
    pub fn render_content_with<'o, O, S>(
        &self,
        objects: O,
        content: &Concat<'_>,
        resources: Option<&'o Object>,
        geom: &PageGeom,
        sink: &mut S,
    ) -> Result<(Pixmap, RenderSummary), Error>
    where
        O: Resolve<'o> + Streams + Copy,
        S: Sink<Fault> + ?Sized,
    {
        let owned_cache;
        let cache = match self.cache {
            Some(c) => c,
            None => {
                owned_cache = RenderCache::new();
                &owned_cache
            }
        };
        render_content_with(
            objects,
            content,
            resources,
            geom,
            cache,
            self.fallback,
            self.opts,
            sink,
        )
    }
}

/// Rasterizes `page` without collecting non-fatal [`Fault`]s.
///
/// # Errors
///
/// Returns [`Error`] on invalid viewports, cancellation, exceeded
/// [`crate::Limits`], or unreadable objects/streams.
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
/// ISO 32000-2 departures and visual approximations to `sink`.
///
/// Passing `&mut ()` disables fault collection and produces the exact same
/// [`Pixmap`], [`RenderSummary`], and [`Error`]s as any active sink.
///
/// # Errors
///
/// Returns [`Error`] on invalid viewports, cancellation, exceeded
/// [`crate::Limits`], or unreadable objects/streams.
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
    check_cancelled(opts)?;
    let mut geom = PageGeom::from_page(page);
    if let Some(dict) = page.dict {
        geom = geom.with_page_boxes(objects, dict);
    }
    let resolved_vp = opts
        .viewport
        .resolve_with_rules(&geom, opts.rules, &opts.limits)?;
    phase!(
        DEBUG,
        "render_page",
        width = resolved_vp.width,
        height = resolved_vp.height
    );

    let oc_state = load_oc_state(objects, catalog, opts, sink)?;
    let output_intent_icc = load_output_intent_icc(objects, catalog, opts)?;
    let ctx = RenderCtx {
        objects: Narrow::new(objects),
        oc_state: oc_state.as_ref(),
        output_intent_icc: output_intent_icc.as_deref(),
        cache,
        fallback,
        opts,
    };

    let mut canvas = Canvas2d::new(resolved_vp.width, resolved_vp.height)?;
    let mut state = RenderState::new(opts);

    let crop_segs = rect_to_segs(geom.crop_box);
    let (crop_bez, _) = segs_to_bez_path(&crop_segs);
    canvas.set_anti_alias(opts.anti_alias.paths);
    canvas.push_clip(&crop_bez, FillRule::NonZero, resolved_vp.page_to_device);

    let page_group = page
        .dict
        .and_then(|d| TransparencyGroup::from_dict(objects, d));
    if let Some(ref g) = page_group
        && (!g.isolated
            || g.knockout
            || g.color_space
                .is_some_and(|cs| !is_srgb_group_cs(ctx.objects, cs)))
    {
        sink.fault(Place::new(page.id, None), Fault::ApproximatedGroup);
        state.approximated = true;
    }
    let pushed_page_group = if page_group.is_some_and(|g| g.isolated) {
        canvas.push_group_layer(None, &BlendState::default(), 1.0, None);
        true
    } else {
        false
    };

    let contents_slices = decode_page_contents(objects, page.dict, opts, &mut state, sink)?;
    if !contents_slices.is_empty() {
        let refs: Vec<&[u8]> = contents_slices.iter().map(Vec::as_slice).collect();
        let concat = Concat::new(&refs);
        let page_resources = page.attribute(Inheritable::Resources).map(|a| a.value);
        let stream_summary = render_stream_into(
            &ctx,
            &concat,
            page_resources,
            resolved_vp.page_to_device,
            0,
            None,
            &mut canvas,
            &mut state,
            sink,
        );
        if let Err(err) = stream_summary {
            if pushed_page_group {
                canvas.pop_layer();
            }
            canvas.pop_clip();
            return Err(err);
        }
        state.gfx = stream_summary?;
    }

    if pushed_page_group {
        canvas.pop_layer();
    }

    if (opts.draw.annots || opts.draw.forms)
        && let Some(page_dict) = page.dict
    {
        draw_page_annotations(
            &ctx,
            catalog,
            page_dict,
            page.id,
            resolved_vp.page_to_device,
            &mut canvas,
            &mut state,
            sink,
        )?;
    }

    check_cancelled(opts)?;
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
/// Returns [`Error`] on invalid viewports, cancellation, exceeded
/// [`crate::Limits`], or unreadable objects/streams.
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
/// recoverable ISO 32000-2 departures and visual approximations to `sink`.
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
    check_cancelled(opts)?;
    let resolved_vp = opts
        .viewport
        .resolve_with_rules(geom, opts.rules, &opts.limits)?;
    phase!(
        DEBUG,
        "render_content",
        width = resolved_vp.width,
        height = resolved_vp.height
    );
    let ctx = RenderCtx {
        objects: Narrow::new(objects),
        oc_state: None,
        output_intent_icc: None,
        cache,
        fallback,
        opts,
    };
    let mut canvas = Canvas2d::new(resolved_vp.width, resolved_vp.height)?;
    let mut state = RenderState::new(opts);

    let crop_segs = rect_to_segs(geom.crop_box);
    let (crop_bez, _) = segs_to_bez_path(&crop_segs);
    canvas.set_anti_alias(opts.anti_alias.paths);
    canvas.push_clip(&crop_bez, FillRule::NonZero, resolved_vp.page_to_device);

    state.gfx = render_stream_into(
        &ctx,
        content,
        resources,
        resolved_vp.page_to_device,
        0,
        None,
        &mut canvas,
        &mut state,
        sink,
    )?;

    check_cancelled(opts)?;
    canvas.pop_clip();
    let pixmap = canvas.finish(opts.background, opts.format, &opts.limits)?;
    Ok((pixmap, state.summary))
}

fn decode_page_contents<'o, O, S>(
    objects: O,
    page_dict: Option<&'o Dict>,
    opts: &RenderOpts,
    state: &mut RenderState,
    sink: &mut S,
) -> Result<Vec<Vec<u8>>, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
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

    let total = stream_ids.len();
    let mut slices = Vec::with_capacity(total);
    for (idx, sid) in stream_ids.into_iter().enumerate() {
        check_cancelled(opts)?;
        match objects.decoded(sid, state.decompressed_left) {
            Ok(bytes) => {
                state.decompressed_left = state.decompressed_left.saturating_sub(bytes.len());
                let mut owned = bytes.into_owned();
                if idx + 1 < total && !owned.ends_with(b"\n") && !owned.ends_with(b" ") {
                    owned.push(b'\n');
                }
                slices.push(owned);
            }
            Err(u) if u.why == Why::Limit => {
                return Err(Error::Gfx(pdf_gfx::Error::Limit(
                    pdf_gfx::Cap::Decompressed,
                )));
            }
            Err(u) if matches!(u.why, Why::Damaged | Why::Filter) => {
                sink.fault(Place::new(Some(sid), None), Fault::SkippedStream);
                state.approximated = true;
            }
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
    let event = match opts.medium {
        RenderMedium::Display => OcEvent::View,
        RenderMedium::Print => OcEvent::Print,
    };
    Ok(Some(props.default_state_for_event(event, opts.rules)))
}

pub(crate) fn is_mark_hidden_by_oc<'a, R: Resolve<'a> + Copy>(
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
    let oi_arr = resolve_opt(objects, oi_raw)?.and_then(Object::as_array);
    let Some(arr) = oi_arr else {
        return Ok(None);
    };
    for item in arr.iter().take(16) {
        let Some(d) = resolve_opt(objects, item)?.and_then(Object::as_dict) else {
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

pub(crate) fn rect_to_segs(r: Rect) -> [Seg; 6] {
    [
        Seg::Move(Point::new(r.x0, r.y0)),
        Seg::Line(Point::new(r.x1, r.y0)),
        Seg::Line(Point::new(r.x1, r.y1)),
        Seg::Line(Point::new(r.x0, r.y1)),
        Seg::Line(Point::new(r.x0, r.y0)),
        Seg::Close,
    ]
}

pub(crate) fn clamp_device_rect(r: Rect, vp_w: u32, vp_h: u32) -> Option<(u32, u32, u32, u32)> {
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
