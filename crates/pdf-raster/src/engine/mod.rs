//! Core zero-I/O page, Form XObject, Type 3 glyph, soft-mask, tiling-pattern,
//! OCG, annotation, and AcroForm widget rasterization engine.

pub(crate) mod annots;
pub(crate) mod mask;
pub(crate) mod ops;
pub(crate) mod sinks;
pub(crate) mod stream;

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use pdf_content_streams::{Concat, FillRule};
use pdf_gfx::{BlendState, Mark, MarkProps, PageGeom, Point, Rect, Seg, TransparencyGroup};
use pdf_objects::Why;
use pdf_objects::fault::Sink;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Streams};
use pdf_semantics::ocg::{OcState, read as read_oc_properties};
use pdf_semantics::pages::{Inheritable, Page};

use crate::canvas2d::{Canvas2d, segs_to_bez_path};
use crate::engine::annots::draw_page_annotations;
use crate::engine::sinks::{Narrow, SemSinkAdapter};
use crate::engine::stream::render_stream_into;
use crate::error::Error;
use crate::fallback::{FontFallback, RenderCache};
use crate::fault::Fault;
use crate::limits::LimitKind;
use crate::opts::{RenderMedium, RenderOpts, RenderSummary};
use crate::phases::phase;
use crate::pixmap::Pixmap;

pub(crate) type TileCacheKey = (ObjectId, u32, u32, u8, Option<[u8; 4]>);

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
    pub(crate) active_streams: Vec<ObjectId>,
    pub(crate) tile_cache: HashMap<TileCacheKey, Arc<vello_cpu::Pixmap>>,
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
            active_streams: Vec::new(),
            tile_cache: HashMap::new(),
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

    pub(crate) fn charge_offscreen_pixels(&mut self, width: u32, height: u32) -> Result<(), Error> {
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
    let pushed_page_group = if page_group.is_some_and(|g| g.isolated) {
        canvas.push_group_layer(None, &BlendState::default(), 1.0, None);
        true
    } else {
        false
    };

    let contents_slices = decode_page_contents(objects, page.dict, &mut state.decompressed_left)?;
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
    let resolved_vp = opts
        .viewport
        .resolve_with_rules(geom, opts.rules, &opts.limits)?;
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

    canvas.pop_clip();
    let pixmap = canvas.finish(opts.background, opts.format, &opts.limits)?;
    Ok((pixmap, state.summary))
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

    let total = stream_ids.len();
    let mut slices = Vec::with_capacity(total);
    for (idx, sid) in stream_ids.into_iter().enumerate() {
        match objects.decoded(sid, *decompressed_budget) {
            Ok(bytes) => {
                *decompressed_budget = decompressed_budget.saturating_sub(bytes.len());
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
