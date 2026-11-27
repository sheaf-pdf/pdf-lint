//! Tier-1 & Tier-2 image decoding, color-space evaluation, and `/ImageMask`, `/Mask`, and
//! `/SMask` + `/Matte` compositing into straight-alpha `RGBA8`.

use std::fmt;

use pdf_color::{EvalOptions, Family, PreparedSpace, Spaces};
use pdf_content_streams::{ImageStream, InlineImage};
use pdf_filters::Reading as FilterReading;
use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Dict, Object, ObjectId, Resolve, Rules, Span, Streams};

use crate::codecs::ccitt::decode_ccitt;
use crate::codecs::dct::decode_dct;
use crate::codecs::jbig2::decode_jbig2;
use crate::codecs::jpx::{ColorFaultForwarder, decode_jpx};
use crate::error::Error;
use crate::fault::Fault;
use crate::header::{
    ImageCodec, ImageMeta, MaskSpec, RenderingIntent, SMaskInData, check_dimensions,
    check_sample_budget, lookup_color_space_object, parse_decode, parse_mask, read_bool, read_int,
    resolve_obj,
};
use crate::inline_chain::decode_inline_standard;
use crate::limits::{LimitKind, Limits};
use crate::phases::phase;
use crate::samples::{RawImage, decode_raw_samples};

/// Options controlling color-space evaluation, stencil coloring, and mask compositing.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct DecodeOptions<'o> {
    /// Color-space evaluation options (`Rules::Iso` or `Rules::Pdfium`, plus optional `/Default*`
    /// and `/OutputIntents` ICC profile).
    pub color: EvalOptions<'o>,
    /// Straight-alpha `[R, G, B, A]` color painted for marked pixels (`0.0` after `/Decode`) of a
    /// 1-bit `/ImageMask true` stencil (`[0, 0, 0, 255]` opaque black by default).
    pub stencil_color: [u8; 4],
    /// Whether to resolve and composite `/Mask` and `/SMask` (`true` by default).
    pub apply_masks: bool,
}

impl Default for DecodeOptions<'_> {
    fn default() -> Self {
        Self::PDFIUM
    }
}

impl<'o> DecodeOptions<'o> {
    /// Strict ISO 32000-2 decoding options (`Rules::Iso`, opaque black stencil color, masks on).
    pub const ISO: DecodeOptions<'static> = DecodeOptions {
        color: EvalOptions::ISO,
        stencil_color: [0, 0, 0, 255],
        apply_masks: true,
    };

    /// Viewer-compatible PDFium (`as of chromium/8152`) decoding options.
    pub const PDFIUM: DecodeOptions<'static> = DecodeOptions {
        color: EvalOptions::PDFIUM,
        stencil_color: [0, 0, 0, 255],
        apply_masks: true,
    };

    /// Builds [`DecodeOptions`] for `rules` using `resources` to populate `/DefaultGray`,
    /// `/DefaultRGB`, and `/DefaultCMYK` under `Rules::Iso`.
    ///
    /// # Errors
    ///
    /// Propagates [`pdf_objects::Unreadable`] if reading `/Resources/ColorSpace` fails.
    pub fn from_resources<R: Resolve<'o> + Copy>(
        objects: R,
        rules: Rules,
        resources: Option<&'o Object>,
    ) -> Result<DecodeOptions<'o>, pdf_objects::Unreadable> {
        let spaces = Spaces::read(objects, resources)?;
        Ok(DecodeOptions {
            color: EvalOptions::from_spaces(rules, spaces),
            stencil_color: [0, 0, 0, 255],
            apply_masks: true,
        })
    }
}

/// Tier-2 decoded image in straight-alpha `sRGB` `RGBA8` (`width * height * 4` bytes, top-to-bottom
/// row-major).
///
/// `Debug` prints dimensions and byte length only—never pixel data.
#[derive(Clone, PartialEq, Eq)]
pub struct RgbaImage {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl fmt::Debug for RgbaImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RgbaImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("byte_len", &self.pixels.len())
            .finish()
    }
}

impl RgbaImage {
    /// Constructs an [`RgbaImage`] from `width`, `height`, and `width * height * 4` `RGBA8` bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Limit`] if dimensions or buffer length exceed `limits`, or
    /// [`Error::NotAnImage`] if `pixels.len() != width * height * 4`.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>, limits: &Limits) -> Result<Self, Error> {
        let count = check_dimensions(width, height, limits)?;
        let expected = check_sample_budget(count.saturating_mul(4), limits)?;
        if pixels.len() != expected {
            return Err(Error::NotAnImage);
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    /// Image width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Image height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Row-major `RGBA8` byte slice (`width * height * 4` bytes).
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.pixels
    }

    /// Mutable row-major `RGBA8` byte slice (`width * height * 4` bytes).
    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    /// Consumes the image and returns its underlying `RGBA8` byte vector.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.pixels
    }

    /// Returns `[R, G, B, A]` at `(x, y)`, or `None` if out of bounds.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        Some([
            self.pixels[idx],
            self.pixels[idx + 1],
            self.pixels[idx + 2],
            self.pixels[idx + 3],
        ])
    }

    /// Fills `[x .. x + w) × [y .. y + h)` in-place with `rgba`, clamped to image bounds.
    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, rgba: [u8; 4]) {
        if w == 0 || h == 0 {
            return;
        }
        let x0 = x.min(self.width) as usize;
        let y0 = y.min(self.height) as usize;
        let x1 = x.saturating_add(w).min(self.width) as usize;
        let y1 = y.saturating_add(h).min(self.height) as usize;
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let stride = self.width as usize;
        for ry in y0..y1 {
            let row = &mut self.pixels[(ry * stride + x0) * 4..(ry * stride + x1) * 4];
            for px in row.as_chunks_mut::<4>().0 {
                *px = rgba;
            }
        }
    }
}

/// Decodes an indirect Image XObject stream `id` into native-sample [`RawImage`] (Tier 1).
///
/// # Errors
///
/// Returns [`Error`] if `id` is unreadable, is not an Image XObject, exceeds `limits`, or fails
/// codec decoding.
pub fn decode_xobject_raw<'o, O>(
    objects: O,
    id: ObjectId,
    resources: Option<&'o Object>,
    options: &DecodeOptions<'o>,
    limits: &Limits,
) -> Result<RawImage, Error>
where
    O: Resolve<'o> + Streams + Copy,
{
    decode_xobject_raw_with(objects, id, resources, options, limits, &mut ())
}

/// Decodes an indirect Image XObject stream `id` into native-sample [`RawImage`] (Tier 1),
/// reporting recoverable departures to `sink`.
///
/// # Errors
///
/// Returns [`Error`] on hard failures or resource cap violations.
pub fn decode_xobject_raw_with<'o, O, S>(
    objects: O,
    id: ObjectId,
    resources: Option<&'o Object>,
    options: &DecodeOptions<'o>,
    limits: &Limits,
    sink: &mut S,
) -> Result<RawImage, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    phase!(
        "decode_xobject_raw",
        object = id.number,
        generation = id.generation
    );
    let Some(obj) = objects.try_resolve(id)? else {
        return Err(Error::NotAnImage);
    };
    let Some(stream) = obj.as_stream() else {
        return Err(Error::NotAnImage);
    };
    let dict = &stream.dict;
    if let Some(subtype) = dict
        .get(b"Subtype")
        .map(|o| resolve_obj(objects, o))
        .transpose()?
        .flatten()
        .and_then(Object::as_name)
        && subtype != b"Image"
    {
        return Err(Error::NotAnImage);
    }

    let place = Place::new(Some(id), obj.at());
    let (stream_cap, limit_kind) = standard_filter_cap(objects, dict, limits)?;
    let decoded_stream = match objects.decoded_standard(id, stream_cap) {
        Ok(ds) => ds,
        Err(u) if u.why == pdf_objects::Why::Limit => {
            return Err(Error::Limit {
                kind: limit_kind,
                value: (stream_cap as u64).saturating_add(1),
                max: stream_cap as u64,
            });
        }
        Err(u) => return Err(Error::Unreadable(u)),
    };
    decode_from_dict_and_stream(
        objects,
        dict,
        &decoded_stream,
        resources,
        options,
        limits,
        Some(place),
        sink,
    )
}

/// Decodes an in-memory [`Stream`](pdf_objects::Stream) (`Data::Bytes`) into native-sample
/// [`RawImage`] (Tier 1), running any leading standard filters declared on `stream.dict` before the
/// image codec.
///
/// # Errors
///
/// Returns [`Error::NotAnImage`] if `stream.data` is not in-memory bytes, or [`Error`] if filter or
/// codec decoding fails or exceeds `limits`.
pub fn decode_stream_raw<'o, O>(
    objects: O,
    stream: &'o pdf_objects::Stream,
    resources: Option<&'o Object>,
    options: &DecodeOptions<'o>,
    limits: &Limits,
) -> Result<RawImage, Error>
where
    O: Resolve<'o> + Streams + Copy,
{
    decode_stream_raw_with(objects, stream, resources, options, limits, &mut ())
}

/// Decodes an in-memory [`Stream`](pdf_objects::Stream) (`Data::Bytes`) into native-sample
/// [`RawImage`] (Tier 1), reporting recoverable departures to `sink`.
///
/// # Errors
///
/// Returns [`Error`] on hard failures or resource cap violations.
pub fn decode_stream_raw_with<'o, O, S>(
    objects: O,
    stream: &'o pdf_objects::Stream,
    resources: Option<&'o Object>,
    options: &DecodeOptions<'o>,
    limits: &Limits,
    sink: &mut S,
) -> Result<RawImage, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let Some(raw_bytes) = stream.data.bytes() else {
        return Err(Error::NotAnImage);
    };
    if let Some(subtype) = stream
        .dict
        .get(b"Subtype")
        .map(|o| resolve_obj(objects, o))
        .transpose()?
        .flatten()
        .and_then(Object::as_name)
        && subtype != b"Image"
    {
        return Err(Error::NotAnImage);
    }
    let filter_reading = match options.color.rules {
        Rules::Iso => FilterReading::Qpdf,
        _ => FilterReading::Pdfium,
    };
    let (stream_cap, limit_kind) = standard_filter_cap(objects, &stream.dict, limits)?;
    let decoded_stream = decode_inline_standard(
        objects,
        &stream.dict,
        raw_bytes,
        stream_cap,
        filter_reading,
        sink,
    )
    .map_err(|e| remap_inline_limit(e, limit_kind, stream_cap))?;
    decode_from_dict_and_stream(
        objects,
        &stream.dict,
        &decoded_stream,
        resources,
        options,
        limits,
        None,
        sink,
    )
}

/// Decodes an in-memory [`Stream`](pdf_objects::Stream) (`Data::Bytes`) into `RGBA8`
/// ([`RgbaImage`], Tier 2).
///
/// # Errors
///
/// Returns [`Error`] on hard failures or resource cap violations.
pub fn decode_stream<'o, O>(
    objects: O,
    stream: &'o pdf_objects::Stream,
    resources: Option<&'o Object>,
    options: &DecodeOptions<'o>,
    limits: &Limits,
) -> Result<RgbaImage, Error>
where
    O: Resolve<'o> + Streams + Copy,
{
    decode_stream_with(objects, stream, resources, options, limits, &mut ())
}

/// Decodes an in-memory [`Stream`](pdf_objects::Stream) (`Data::Bytes`) into `RGBA8`
/// ([`RgbaImage`], Tier 2), reporting recoverable departures to `sink`.
///
/// # Errors
///
/// Returns [`Error`] on hard failures or resource cap violations.
pub fn decode_stream_with<'o, O, S>(
    objects: O,
    stream: &'o pdf_objects::Stream,
    resources: Option<&'o Object>,
    options: &DecodeOptions<'o>,
    limits: &Limits,
    sink: &mut S,
) -> Result<RgbaImage, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let raw = decode_stream_raw_with(objects, stream, resources, options, limits, sink)?;
    raw.to_rgba8_inner(objects, options, limits, 0, sink)
}

/// Decodes an inline image (`BI..ID..EI`) from a content stream into native-sample [`RawImage`]
/// (Tier 1).
///
/// # Errors
///
/// Returns [`Error`] if the inline image dictionary or stream is malformed or exceeds `limits`.
pub fn decode_inline_raw<'o, O>(
    objects: O,
    inline: &InlineImage<'_>,
    content: &[u8],
    resources: Option<&'o Object>,
    options: &DecodeOptions<'o>,
    limits: &Limits,
) -> Result<RawImage, Error>
where
    O: Resolve<'o> + Streams + Copy,
{
    decode_inline_raw_with(
        objects,
        inline,
        content,
        resources,
        options,
        limits,
        &mut (),
    )
}

/// Decodes an inline image (`BI..ID..EI`) into native-sample [`RawImage`] (Tier 1), reporting
/// recoverable departures to `sink`.
///
/// # Errors
///
/// Returns [`Error`] on hard failures or resource cap violations.
pub fn decode_inline_raw_with<'o, O, S>(
    objects: O,
    inline: &InlineImage<'_>,
    content: &[u8],
    resources: Option<&'o Object>,
    options: &DecodeOptions<'o>,
    limits: &Limits,
    sink: &mut S,
) -> Result<RawImage, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let at = inline.at();
    phase!("decode_inline_raw", start = at.start, end = at.end);
    let Some(dict) = inline.to_dict(&limits.content) else {
        return Err(Error::NotAnImage);
    };
    let raw_slice: &[u8] = match inline.stream() {
        ImageStream::Data(range) => content.get(range).ok_or(Error::NotAnImage)?,
        ImageStream::Empty => &[],
        ImageStream::Missing => return Err(Error::NotAnImage),
        _ => return Err(Error::NotAnImage),
    };
    let filter_reading = match options.color.rules {
        Rules::Iso => FilterReading::Qpdf,
        _ => FilterReading::Pdfium,
    };
    let narrowed = NarrowResolve {
        inner: objects,
        _marker: core::marker::PhantomData,
    };
    let (stream_cap, limit_kind) = standard_filter_cap(narrowed, &dict, limits)?;
    let decoded_stream =
        decode_inline_standard(narrowed, &dict, raw_slice, stream_cap, filter_reading, sink)
            .map_err(|e| remap_inline_limit(e, limit_kind, stream_cap))?;
    let place = Place::from(Span::new(None, at.start, at.end));
    let mut local_opts = DecodeOptions {
        color: EvalOptions::from_spaces(options.color.rules, None),
        stencil_color: options.stencil_color,
        apply_masks: options.apply_masks,
    };
    local_opts.color.default_gray = options.color.default_gray;
    local_opts.color.default_rgb = options.color.default_rgb;
    local_opts.color.default_cmyk = options.color.default_cmyk;
    local_opts.color.output_intent_profile = options.color.output_intent_profile;
    decode_from_dict_and_stream(
        narrowed,
        &dict,
        &decoded_stream,
        resources,
        &local_opts,
        limits,
        Some(place),
        sink,
    )
}

#[derive(Clone, Copy)]
struct NarrowResolve<'o, O> {
    inner: O,
    _marker: core::marker::PhantomData<fn() -> &'o ()>,
}

impl<'d, 'o: 'd, O: Resolve<'o>> Resolve<'d> for NarrowResolve<'o, O> {
    fn resolve(&self, id: ObjectId) -> Option<&'d Object> {
        self.inner.resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'d Object>, pdf_objects::Unreadable> {
        self.inner.try_resolve(id)
    }
}

impl<'o, O: Streams> Streams for NarrowResolve<'o, O> {
    fn decoded(
        &self,
        id: ObjectId,
        limit: usize,
    ) -> Result<std::borrow::Cow<'_, [u8]>, pdf_objects::Unreadable> {
        self.inner.decoded(id, limit)
    }

    fn decoded_standard(
        &self,
        id: ObjectId,
        limit: usize,
    ) -> Result<pdf_objects::DecodedStream<'_>, pdf_objects::Unreadable> {
        self.inner.decoded_standard(id, limit)
    }
}

/// Decodes an indirect Image XObject stream `id` into color-evaluated, mask-composited `RGBA8`
/// ([`RgbaImage`], Tier 2).
///
/// # Errors
///
/// Returns [`Error`] on hard failures or resource cap violations.
pub fn decode_xobject<'o, O>(
    objects: O,
    id: ObjectId,
    resources: Option<&'o Object>,
    options: &DecodeOptions<'o>,
    limits: &Limits,
) -> Result<RgbaImage, Error>
where
    O: Resolve<'o> + Streams + Copy,
{
    decode_xobject_with(objects, id, resources, options, limits, &mut ())
}

/// Decodes an indirect Image XObject stream `id` into `RGBA8` ([`RgbaImage`], Tier 2), reporting
/// recoverable departures to `sink`.
///
/// # Errors
///
/// Returns [`Error`] on hard failures or resource cap violations.
pub fn decode_xobject_with<'o, O, S>(
    objects: O,
    id: ObjectId,
    resources: Option<&'o Object>,
    options: &DecodeOptions<'o>,
    limits: &Limits,
    sink: &mut S,
) -> Result<RgbaImage, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let raw = decode_xobject_raw_with(objects, id, resources, options, limits, sink)?;
    raw.to_rgba8_inner(objects, options, limits, 0, sink)
}

/// Decodes an inline image (`BI..ID..EI`) into color-evaluated, mask-composited `RGBA8`
/// ([`RgbaImage`], Tier 2).
///
/// # Errors
///
/// Returns [`Error`] on hard failures or resource cap violations.
pub fn decode_inline<'o, O>(
    objects: O,
    inline: &InlineImage<'_>,
    content: &[u8],
    resources: Option<&'o Object>,
    options: &DecodeOptions<'o>,
    limits: &Limits,
) -> Result<RgbaImage, Error>
where
    O: Resolve<'o> + Streams + Copy,
{
    decode_inline_with(
        objects,
        inline,
        content,
        resources,
        options,
        limits,
        &mut (),
    )
}

/// Decodes an inline image (`BI..ID..EI`) into `RGBA8` ([`RgbaImage`], Tier 2), reporting
/// recoverable departures to `sink`.
///
/// # Errors
///
/// Returns [`Error`] on hard failures or resource cap violations.
pub fn decode_inline_with<'o, O, S>(
    objects: O,
    inline: &InlineImage<'_>,
    content: &[u8],
    resources: Option<&'o Object>,
    options: &DecodeOptions<'o>,
    limits: &Limits,
    sink: &mut S,
) -> Result<RgbaImage, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let raw = decode_inline_raw_with(objects, inline, content, resources, options, limits, sink)?;
    raw.to_rgba8_inner(objects, options, limits, 0, sink)
}

impl RawImage {
    /// Evaluates this [`RawImage`] into straight-alpha `sRGB` `RGBA8` ([`RgbaImage`]), applying
    /// `/Decode`, color-space conversion, `/ImageMask` stencil coloring, `/Mask`, and `/SMask` +
    /// `/Matte`.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if RGBA buffer allocation exceeds `limits` or a subsidiary stream fails
    /// with a hard reader error.
    pub fn to_rgba8<'o, O>(
        &self,
        objects: O,
        options: &DecodeOptions<'o>,
        limits: &Limits,
    ) -> Result<RgbaImage, Error>
    where
        O: Resolve<'o> + Streams + Copy,
    {
        self.to_rgba8_with(objects, options, limits, &mut ())
    }

    /// Evaluates this [`RawImage`] into `RGBA8` ([`RgbaImage`]), reporting recoverable mask
    /// departures to `sink`.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if RGBA buffer allocation exceeds `limits` or a subsidiary stream fails
    /// with a hard reader error.
    pub fn to_rgba8_with<'o, O, S>(
        &self,
        objects: O,
        options: &DecodeOptions<'o>,
        limits: &Limits,
        sink: &mut S,
    ) -> Result<RgbaImage, Error>
    where
        O: Resolve<'o> + Streams + Copy,
        S: Sink<Fault> + ?Sized,
    {
        self.to_rgba8_inner(objects, options, limits, 0, sink)
    }

    fn to_rgba8_inner<'o, O, S>(
        &self,
        objects: O,
        options: &DecodeOptions<'o>,
        limits: &Limits,
        depth: u8,
        sink: &mut S,
    ) -> Result<RgbaImage, Error>
    where
        O: Resolve<'o> + Streams + Copy,
        S: Sink<Fault> + ?Sized,
    {
        let width = self.meta.width;
        let height = self.meta.height;
        phase!(
            "to_rgba8",
            width = width,
            height = height,
            components = self.meta.components
        );
        let pixels = check_dimensions(width, height, limits)?;
        let rgba_len = check_sample_budget(pixels.saturating_mul(4), limits)?;

        // 1. Stencil mask (`/ImageMask true`).
        if self.meta.stencil {
            let mut out = vec![0_u8; rgba_len];
            let mut comp = [0.0_f32; 1];
            for y in 0..height {
                for x in 0..width {
                    let _ = self.pixel_decoded(x, y, &mut comp);
                    // ISO 32000-2 §8.9.6.2: mapped value < 0.5 (`0`) paints stencil_color;
                    // mapped value >= 0.5 (`1`) leaves backdrop transparent.
                    if comp[0] < 0.5 {
                        let idx = ((y as usize) * (width as usize) + (x as usize)) * 4;
                        out[idx..idx + 4].copy_from_slice(&options.stencil_color);
                    }
                }
            }
            return Ok(RgbaImage {
                width,
                height,
                pixels: out,
            });
        }

        let Some(space) = self.prepared_space.as_ref() else {
            return Err(Error::MissingColorSpace);
        };

        // 2. Resolve optional `/SMask` + `/Matte` and explicit `/Mask` when `apply_masks` is on.
        //    Per ISO 32000-2 §8.9.5.1 (Table 87) and §11.6.5.2 (Table 144):
        //    - `/Matte` has as many components as the parent image's `/ColorSpace`—or, when the
        //      parent space is `Indexed`, its underlying base color space.
        //    - When `/SMask` (or embedded `/SMaskInData` alpha) is present, `/Mask` is ignored.
        let matte_space = space.base_space().unwrap_or(space);
        let matte_comps = u8::try_from(matte_space.components()).unwrap_or(self.meta.components);
        let resolved_smask = if options.apply_masks {
            match self.meta.smask {
                Some(smask_id) => {
                    load_smask(objects, smask_id, matte_comps, options, limits, depth, sink)?
                }
                None => None,
            }
        } else {
            None
        };

        let has_soft_alpha = resolved_smask.is_some() || self.embedded_alpha.is_some();

        let resolved_stencil_mask = if options.apply_masks && !has_soft_alpha {
            match &self.meta.mask {
                MaskSpec::Stencil(mask_id) => {
                    load_stencil_mask(objects, *mask_id, options, limits, depth, sink)?
                }
                _ => None,
            }
        } else {
            None
        };

        let color_key = if options.apply_masks && !has_soft_alpha {
            match &self.meta.mask {
                MaskSpec::ColorKey(ranges) => Some(ranges.as_slice()),
                _ => None,
            }
        } else {
            None
        };

        // Pre-evaluate `/Matte` into sRGB using `matte_space` (the base space for `Indexed`).
        let matte_srgb: Option<[f32; 3]> = resolved_smask
            .as_ref()
            .and_then(|sm| sm.matte.as_deref())
            .and_then(|m| matte_space.eval_with_rules(m, options.color.rules, &limits.color))
            .and_then(|p| p.srgb());

        let comps = usize::from(self.meta.components);
        let mut out = vec![0_u8; rgba_len];
        let mut raw_buf = vec![0_u16; comps];
        let mut dec_buf = vec![0.0_f32; comps];
        let denom = ((1_u32 << self.meta.bits_per_component) - 1) as f64;

        for y in 0..height {
            for x in 0..width {
                let px_offset = ((y as usize) * (width as usize) + (x as usize)) * 4;

                // Read raw integer samples once for both color-key check and `/Decode` mapping.
                let _ = self.pixel_raw(x, y, &mut raw_buf[..comps]);

                if let Some(ranges) = color_key {
                    let all_match =
                        ranges
                            .iter()
                            .zip(&raw_buf[..comps])
                            .all(|(&(lo, hi), &sample)| {
                                let s = u32::from(sample);
                                lo <= s && s <= hi
                            });
                    if all_match {
                        // Transparent `[0, 0, 0, 0]`.
                        continue;
                    }
                }

                if let Some(mask_img) = &resolved_stencil_mask {
                    let mx = sample_coord(x, width, mask_img.width());
                    let my = sample_coord(y, height, mask_img.height());
                    let mut m_val = [0.0_f32; 1];
                    let _ = mask_img.pixel_decoded(mx, my, &mut m_val);
                    // In an explicit `/Mask` stencil, `1` leaves backdrop unchanged (masked out).
                    if m_val[0] >= 0.5 {
                        continue;
                    }
                }

                // Determine alpha from `/SMask` or JPX `/SMaskInData`.
                let alpha_u8 = if let Some(sm) = &resolved_smask {
                    let sx = sample_coord(x, width, sm.image.width());
                    let sy = sample_coord(y, height, sm.image.height());
                    let mut a_val = [0.0_f32; 1];
                    let _ = sm.image.pixel_decoded(sx, sy, &mut a_val);
                    (a_val[0].clamp(0.0, 1.0) * 255.0).round() as u8
                } else if let Some(emb_a) = &self.embedded_alpha {
                    emb_a[(y as usize) * (width as usize) + (x as usize)]
                } else {
                    255
                };

                if alpha_u8 == 0 {
                    continue;
                }

                for c in 0..comps {
                    let (d_min, d_max) = self.meta.decode[c];
                    dec_buf[c] = (d_min + (f64::from(raw_buf[c]) / denom) * (d_max - d_min)) as f32;
                }

                // Apply ISO §11.6.5.3 component-domain `/Matte` un-premultiplication when in ISO
                // mode and not Indexed.
                let use_iso_matte =
                    matches!(options.color.rules, Rules::Iso) && space.family() != Family::Indexed;
                if use_iso_matte
                    && let Some(sm) = &resolved_smask
                    && let Some(matte) = &sm.matte
                    && alpha_u8 < 255
                {
                    let a = f32::from(alpha_u8) / 255.0;
                    for c in 0..comps {
                        let (lo, hi) =
                            sorted_pair(self.meta.decode[c].0 as f32, self.meta.decode[c].1 as f32);
                        dec_buf[c] = (matte[c] + (dec_buf[c] - matte[c]) / a).clamp(lo, hi);
                    }
                }

                let Some(paint) =
                    space.eval_with_rules(&dec_buf[..comps], options.color.rules, &limits.color)
                else {
                    continue;
                };
                let Some(mut rgb) = paint.srgb() else {
                    // `Separation /None` marks nothing!
                    continue;
                };

                if !use_iso_matte
                    && let Some(mr) = matte_srgb
                    && alpha_u8 < 255
                {
                    let a = f32::from(alpha_u8) / 255.0;
                    rgb[0] = (mr[0] + (rgb[0] - mr[0]) / a).clamp(0.0, 1.0);
                    rgb[1] = (mr[1] + (rgb[1] - mr[1]) / a).clamp(0.0, 1.0);
                    rgb[2] = (mr[2] + (rgb[2] - mr[2]) / a).clamp(0.0, 1.0);
                }

                out[px_offset] = quantize_u8(rgb[0]);
                out[px_offset + 1] = quantize_u8(rgb[1]);
                out[px_offset + 2] = quantize_u8(rgb[2]);
                out[px_offset + 3] = alpha_u8;
            }
        }

        Ok(RgbaImage {
            width,
            height,
            pixels: out,
        })
    }
}

fn is_standard_or_crypt_filter(name: &[u8]) -> bool {
    matches!(
        name,
        b"FlateDecode"
            | b"Fl"
            | b"LZWDecode"
            | b"LZW"
            | b"ASCIIHexDecode"
            | b"AHx"
            | b"ASCII85Decode"
            | b"A85"
            | b"RunLengthDecode"
            | b"RL"
            | b"Crypt"
    )
}

fn standard_filter_cap<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    limits: &Limits,
) -> Result<(usize, LimitKind), Error> {
    let has_codec_tail = match dict
        .get(b"Filter")
        .or_else(|| dict.get(b"F"))
        .map(|o| resolve_obj(objects, o))
        .transpose()?
        .flatten()
    {
        None => false,
        Some(obj) if obj.is_null() => false,
        Some(obj) => {
            if let Some(name) = obj.as_name() {
                !is_standard_or_crypt_filter(name)
            } else if let Some(arr) = obj.as_array() {
                let mut non_std = false;
                for item in arr {
                    if let Some(name) = resolve_obj(objects, item)?.and_then(Object::as_name)
                        && !is_standard_or_crypt_filter(name)
                    {
                        non_std = true;
                        break;
                    }
                }
                non_std
            } else {
                false
            }
        }
    };
    if has_codec_tail {
        Ok((limits.max_encoded_bytes, LimitKind::EncodedBytes))
    } else {
        Ok((limits.max_sample_bytes, LimitKind::SampleBytes))
    }
}

fn remap_inline_limit(err: Error, limit_kind: LimitKind, cap: usize) -> Error {
    match err {
        Error::Limit {
            kind: LimitKind::EncodedBytes,
            ..
        } => Error::Limit {
            kind: limit_kind,
            value: (cap as u64).saturating_add(1),
            max: cap as u64,
        },
        other => other,
    }
}

#[inline]
fn sample_coord(coord: u32, src_extent: u32, mask_extent: u32) -> u32 {
    if src_extent == mask_extent {
        return coord;
    }
    let mapped =
        ((u64::from(coord) * 2 + 1) * u64::from(mask_extent)) / (u64::from(src_extent) * 2);
    (mapped as u32).min(mask_extent.saturating_sub(1))
}

#[inline]
fn sorted_pair(a: f32, b: f32) -> (f32, f32) {
    if a <= b { (a, b) } else { (b, a) }
}

#[inline]
fn quantize_u8(v: f32) -> u8 {
    if v.is_finite() {
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        0
    }
}

struct LoadedSMask {
    image: RawImage,
    matte: Option<Vec<f32>>,
}

fn load_smask<'o, O, S>(
    objects: O,
    smask_id: ObjectId,
    parent_components: u8,
    options: &DecodeOptions<'o>,
    limits: &Limits,
    depth: u8,
    sink: &mut S,
) -> Result<Option<LoadedSMask>, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let place = Place::new(Some(smask_id), None);
    if depth >= limits.mask_depth {
        sink.fault(place, Fault::InvalidSMask);
        return Ok(None);
    }
    let Some(smask_obj) = objects.try_resolve(smask_id)? else {
        sink.fault(place, Fault::InvalidSMask);
        return Ok(None);
    };
    let Some(smask_stream) = smask_obj.as_stream() else {
        sink.fault(place, Fault::InvalidSMask);
        return Ok(None);
    };

    let matte = parse_matte(objects, &smask_stream.dict, parent_components, place, sink)?;

    let mut sub_sink = ();
    let sub_opts = DecodeOptions {
        color: EvalOptions::from_spaces(options.color.rules, None),
        stencil_color: [0, 0, 0, 255],
        apply_masks: false,
    };
    match decode_xobject_raw_with(objects, smask_id, None, &sub_opts, limits, &mut sub_sink) {
        Ok(raw) if !raw.meta.stencil && raw.meta.components == 1 => {
            Ok(Some(LoadedSMask { image: raw, matte }))
        }
        Ok(_) => {
            sink.fault(place, Fault::InvalidSMask);
            Ok(None)
        }
        Err(Error::Unreadable(u))
            if matches!(
                u.why,
                pdf_objects::Why::Locked
                    | pdf_objects::Why::Capped
                    | pdf_objects::Why::Source
                    | pdf_objects::Why::Detached
            ) =>
        {
            Err(Error::Unreadable(u))
        }
        Err(Error::Limit {
            kind: LimitKind::SampleBytes | LimitKind::Pixels | LimitKind::Width | LimitKind::Height,
            ..
        }) => {
            sink.fault(place, Fault::InvalidSMask);
            Ok(None)
        }
        Err(_) => {
            sink.fault(place, Fault::InvalidSMask);
            Ok(None)
        }
    }
}

fn parse_matte<'o, R, S>(
    objects: R,
    dict: &'o Dict,
    parent_components: u8,
    place: Place,
    sink: &mut S,
) -> Result<Option<Vec<f32>>, Error>
where
    R: Resolve<'o> + Copy,
    S: Sink<Fault> + ?Sized,
{
    let Some(raw) = dict.get(b"Matte") else {
        return Ok(None);
    };
    let Some(obj) = resolve_obj(objects, raw)? else {
        sink.fault(place, Fault::InvalidMatte);
        return Ok(None);
    };
    let Some(arr) = obj.as_array() else {
        sink.fault(place, Fault::InvalidMatte);
        return Ok(None);
    };
    if arr.len() != usize::from(parent_components) {
        sink.fault(place, Fault::InvalidMatte);
        return Ok(None);
    }
    let mut vals = Vec::with_capacity(arr.len());
    for item in arr {
        match resolve_obj(objects, item)?.and_then(Object::as_number) {
            Some(v) if v.is_finite() => vals.push(v as f32),
            _ => {
                sink.fault(place, Fault::InvalidMatte);
                return Ok(None);
            }
        }
    }
    Ok(Some(vals))
}

fn load_stencil_mask<'o, O, S>(
    objects: O,
    mask_id: ObjectId,
    options: &DecodeOptions<'o>,
    limits: &Limits,
    depth: u8,
    sink: &mut S,
) -> Result<Option<RawImage>, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let place = Place::new(Some(mask_id), None);
    if depth >= limits.mask_depth {
        sink.fault(place, Fault::InvalidMask);
        return Ok(None);
    }
    let mut sub_sink = ();
    let sub_opts = DecodeOptions {
        color: options.color,
        stencil_color: [0, 0, 0, 255],
        apply_masks: false,
    };
    match decode_xobject_raw_with(objects, mask_id, None, &sub_opts, limits, &mut sub_sink) {
        Ok(raw) if raw.meta.stencil => Ok(Some(raw)),
        Ok(_) => {
            sink.fault(place, Fault::InvalidMask);
            Ok(None)
        }
        Err(Error::Unreadable(u))
            if matches!(
                u.why,
                pdf_objects::Why::Locked
                    | pdf_objects::Why::Capped
                    | pdf_objects::Why::Source
                    | pdf_objects::Why::Detached
            ) =>
        {
            Err(Error::Unreadable(u))
        }
        Err(_) => {
            sink.fault(place, Fault::InvalidMask);
            Ok(None)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_from_dict_and_stream<'o, O, S>(
    objects: O,
    dict: &'o Dict,
    decoded_stream: &pdf_objects::DecodedStream<'_>,
    resources: Option<&'o Object>,
    options: &DecodeOptions<'o>,
    limits: &Limits,
    place: Option<Place>,
    sink: &mut S,
) -> Result<RawImage, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let place_or_nowhere = place.unwrap_or_else(|| Place::new(None, None));

    let width_i64 = read_int(objects, dict, b"Width")?.ok_or(Error::NotAnImage)?;
    let height_i64 = read_int(objects, dict, b"Height")?.ok_or(Error::NotAnImage)?;
    if width_i64 <= 0 || height_i64 <= 0 {
        return Err(Error::NotAnImage);
    }
    let width = u32::try_from(width_i64).map_err(|_| Error::Limit {
        kind: LimitKind::Width,
        value: width_i64 as u64,
        max: u64::from(limits.max_width),
    })?;
    let height = u32::try_from(height_i64).map_err(|_| Error::Limit {
        kind: LimitKind::Height,
        value: height_i64 as u64,
        max: u64::from(limits.max_height),
    })?;
    let _ = check_dimensions(width, height, limits)?;

    // Determine trailing codec filter (if any) and reject unsupported post-codec chains.
    let (codec, codec_parms) = match &decoded_stream.tail {
        None => (ImageCodec::Raw, None),
        Some(tail) => {
            if !tail.after.is_empty() {
                return Err(Error::UnsupportedFilter {
                    filter: tail.after[0].name.to_vec(),
                });
            }
            let c = match &tail.first.name[..] {
                b"DCTDecode" | b"DCT" => ImageCodec::Dct,
                b"CCITTFaxDecode" | b"CCF" => ImageCodec::CcittFax,
                b"JBIG2Decode" => ImageCodec::Jbig2,
                b"JPXDecode" => ImageCodec::Jpx,
                other => {
                    return Err(Error::UnsupportedFilter {
                        filter: other.to_vec(),
                    });
                }
            };
            (c, tail.first.parms.as_ref())
        }
    };

    let stencil = read_bool(objects, dict, b"ImageMask")?.unwrap_or(false);
    let interpolate = read_bool(objects, dict, b"Interpolate")?.unwrap_or(false);

    let smask_in_data = if codec == ImageCodec::Jpx {
        match read_int(objects, dict, b"SMaskInData")?.unwrap_or(0) {
            1 => SMaskInData::Separate,
            2 => SMaskInData::Premultiplied,
            _ => SMaskInData::None,
        }
    } else {
        SMaskInData::None
    };

    // Per ISO 32000-2 Table 87, `/SMask` shall not be specified when `/SMaskInData` is non-zero;
    // when `/SMaskInData` is non-zero, embedded JPX alpha takes precedence.
    let smask = if smask_in_data != SMaskInData::None {
        None
    } else {
        dict.get(b"SMask").and_then(Object::as_reference)
    };

    let intent = dict
        .get(b"Intent")
        .map(|o| resolve_obj(objects, o))
        .transpose()?
        .flatten()
        .and_then(Object::as_name)
        .map(RenderingIntent::from_name);

    let oc = dict.get(b"OC").and_then(Object::as_reference);

    let dict_bpc = read_int(objects, dict, b"BitsPerComponent")?;

    // Resolve `/ColorSpace` when not a stencil.
    let spaces = Spaces::read(objects, resources)?;
    let mut eval_opts = options.color;
    if eval_opts.rules == Rules::Iso {
        if eval_opts.default_gray.is_none() {
            eval_opts.default_gray = spaces.and_then(|s| s.get(b"DefaultGray")).map(|n| n.value);
        }
        if eval_opts.default_rgb.is_none() {
            eval_opts.default_rgb = spaces.and_then(|s| s.get(b"DefaultRGB")).map(|n| n.value);
        }
        if eval_opts.default_cmyk.is_none() {
            eval_opts.default_cmyk = spaces.and_then(|s| s.get(b"DefaultCMYK")).map(|n| n.value);
        }
    }

    let (prepared_space, had_pdf_color_space) = if stencil {
        if dict.get(b"ColorSpace").is_some() {
            sink.fault(place_or_nowhere, Fault::StencilHasColorSpace);
        }
        (None, false)
    } else if let Some(cs_obj) = lookup_color_space_object(objects, dict, spaces)? {
        let mut forwarder = ColorFaultForwarder { inner: sink };
        let Some(prep) =
            PreparedSpace::prepare(objects, cs_obj, &eval_opts, &limits.color, &mut forwarder)?
        else {
            return Err(Error::MissingColorSpace);
        };
        (Some(prep), true)
    } else if codec == ImageCodec::Jpx {
        // JPXDecode allows omitting `/ColorSpace`; `decode_jpx` supplies the JP2 container's space.
        (None, false)
    } else {
        return Err(Error::MissingColorSpace);
    };

    // Validate initial `/BitsPerComponent` before running codecs.
    // Per ISO 32000-2 §7.4.9, `/BitsPerComponent` is optional and ignored for `JPXDecode`.
    // Under `Rules::Pdfium`, PDFium (`CPDF_DIB::CreateDecoder`) also forces `bpc = 1` on
    // `CCITTFaxDecode` and `JBIG2Decode` regardless of `/BitsPerComponent`.
    let initial_bpc: u8 = if codec == ImageCodec::Jpx {
        if stencil { 1 } else { 8 }
    } else if matches!(codec, ImageCodec::CcittFax | ImageCodec::Jbig2) {
        match dict_bpc {
            None | Some(1) => 1,
            Some(other) if eval_opts.rules == Rules::Iso => {
                return Err(Error::InvalidBitsPerComponent { bpc: other });
            }
            Some(_) => {
                sink.fault(place_or_nowhere, Fault::InvalidBitsPerComponent);
                1
            }
        }
    } else if stencil {
        match dict_bpc {
            None | Some(1) => 1,
            Some(other) => return Err(Error::InvalidBitsPerComponent { bpc: other }),
        }
    } else {
        match (codec, dict_bpc) {
            (ImageCodec::Dct, None) => 8,
            (_, Some(v @ (1 | 2 | 4 | 8 | 16))) => v as u8,
            (_, Some(other)) => return Err(Error::InvalidBitsPerComponent { bpc: other }),
            (_, None) => return Err(Error::InvalidBitsPerComponent { bpc: 0 }),
        }
    };

    // Run codec or unpack raw stream bytes.
    let (final_w, final_h, final_bpc, final_comps, samples, embedded_alpha, final_space) =
        match codec {
            ImageCodec::Raw => {
                let comps = if stencil {
                    1_u8
                } else {
                    u8::try_from(prepared_space.as_ref().unwrap().components())
                        .map_err(|_| Error::MissingColorSpace)?
                };
                let s = decode_raw_samples(
                    &decoded_stream.data,
                    width,
                    height,
                    comps,
                    initial_bpc,
                    limits,
                    place_or_nowhere,
                    sink,
                )?;
                (width, height, initial_bpc, comps, s, None, prepared_space)
            }
            ImageCodec::CcittFax => {
                let comps = if stencil {
                    1_u8
                } else {
                    let c = prepared_space.as_ref().unwrap().components();
                    if c != 1 {
                        return Err(Error::Codec {
                            codec: ImageCodec::CcittFax,
                            detail: format!("CCITTFaxDecode requires 1-component space, got {c}"),
                        });
                    }
                    1_u8
                };
                if initial_bpc != 1 {
                    return Err(Error::InvalidBitsPerComponent {
                        bpc: i64::from(initial_bpc),
                    });
                }
                let s = decode_ccitt(
                    objects,
                    &decoded_stream.data,
                    codec_parms,
                    width,
                    height,
                    limits,
                    place_or_nowhere,
                    sink,
                )?;
                (width, height, 1, comps, s, None, prepared_space)
            }
            ImageCodec::Jbig2 => {
                let comps = if stencil {
                    1_u8
                } else {
                    let c = prepared_space.as_ref().unwrap().components();
                    if c != 1 {
                        return Err(Error::Codec {
                            codec: ImageCodec::Jbig2,
                            detail: format!("JBIG2Decode requires 1-component space, got {c}"),
                        });
                    }
                    1_u8
                };
                if initial_bpc != 1 {
                    return Err(Error::InvalidBitsPerComponent {
                        bpc: i64::from(initial_bpc),
                    });
                }
                let jb = decode_jbig2(
                    objects,
                    &decoded_stream.data,
                    codec_parms,
                    width,
                    height,
                    limits,
                    place_or_nowhere,
                    sink,
                )?;
                (
                    jb.width,
                    jb.height,
                    1,
                    comps,
                    jb.samples,
                    None,
                    prepared_space,
                )
            }
            ImageCodec::Dct => {
                if stencil {
                    return Err(Error::Codec {
                        codec: ImageCodec::Dct,
                        detail: "DCTDecode cannot be used with /ImageMask true".to_string(),
                    });
                }
                let expected_comps = u8::try_from(prepared_space.as_ref().unwrap().components())
                    .map_err(|_| Error::MissingColorSpace)?;
                let dct = decode_dct(
                    objects,
                    &decoded_stream.data,
                    codec_parms,
                    width,
                    height,
                    expected_comps,
                    eval_opts.rules,
                    limits,
                    place_or_nowhere,
                    sink,
                )?;
                if dct.components != expected_comps {
                    return Err(Error::Codec {
                        codec: ImageCodec::Dct,
                        detail: format!(
                            "JPEG component count ({}) does not match /ColorSpace ({expected_comps})",
                            dct.components
                        ),
                    });
                }
                (
                    dct.width,
                    dct.height,
                    8,
                    dct.components,
                    dct.samples,
                    None,
                    prepared_space,
                )
            }
            ImageCodec::Jpx => {
                let jpx = decode_jpx(
                    &decoded_stream.data,
                    width,
                    height,
                    prepared_space.as_ref(),
                    stencil,
                    smask_in_data,
                    &eval_opts,
                    limits,
                    place_or_nowhere,
                    sink,
                )?;
                if stencil {
                    if jpx.components != 1 || jpx.raw_depth != 1 {
                        return Err(Error::Codec {
                            codec: ImageCodec::Jpx,
                            detail: format!(
                                "JPXDecode with /ImageMask true requires a single 1-bit channel, got {} channels at {} bpc",
                                jpx.components, jpx.raw_depth
                            ),
                        });
                    }
                    (jpx.width, jpx.height, 1, 1, jpx.samples, None, None)
                } else {
                    let space = prepared_space.or(jpx.embedded_space);
                    let Some(sp) = space else {
                        return Err(Error::MissingColorSpace);
                    };
                    if u32::from(jpx.components) != sp.components() {
                        return Err(Error::Codec {
                            codec: ImageCodec::Jpx,
                            detail: format!(
                                "JPX component count ({}) does not match /ColorSpace ({})",
                                jpx.components,
                                sp.components()
                            ),
                        });
                    }
                    (
                        jpx.width,
                        jpx.height,
                        jpx.bpc,
                        jpx.components,
                        jpx.samples,
                        jpx.embedded_alpha,
                        Some(sp),
                    )
                }
            }
        };

    let default_decode = if stencil {
        vec![(0.0, 1.0)]
    } else {
        final_space
            .as_ref()
            .and_then(|s| s.default_decode(final_bpc))
            .unwrap_or_else(|| vec![(0.0, 1.0); usize::from(final_comps)])
    };

    // Per ISO 32000-2 §7.4.9 / Table 87, `/Decode` is ignored on `JPXDecode` streams that omit
    // `/ColorSpace` unless `/ImageMask` is `true`.
    let decode = if codec == ImageCodec::Jpx && !stencil && !had_pdf_color_space {
        default_decode
    } else {
        parse_decode(objects, dict, final_comps, default_decode, place, sink)?
    };

    let mask = parse_mask(objects, dict, final_comps, final_bpc, stencil, place, sink)?;

    let meta = ImageMeta {
        width: final_w,
        height: final_h,
        bits_per_component: final_bpc,
        components: final_comps,
        color_space: final_space.as_ref().map(PreparedSpace::family),
        stencil,
        interpolate,
        decode,
        mask,
        smask,
        smask_in_data,
        intent,
        oc,
    };

    Ok(RawImage {
        meta,
        codec,
        samples,
        embedded_alpha,
        prepared_space: final_space,
    })
}
