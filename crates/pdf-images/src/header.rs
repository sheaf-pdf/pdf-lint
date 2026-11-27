//! Image dictionary metadata, codec kinds, mask specifications, and rendering intents.

pub use pdf_color::RenderingIntent;
use pdf_color::{Family, PreparedSpace, Spaces};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Dict, Object, ObjectId, Resolve, Span};

use crate::error::Error;
use crate::fault::Fault;
use crate::limits::{LimitKind, Limits};

/// Codec used to compress the image's sample stream after any leading standard filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImageCodec {
    /// Uncompressed or standard-filtered (`FlateDecode`, `LZWDecode`, `ASCII85Decode`,
    /// `ASCIIHexDecode`, `RunLengthDecode`) packed sample bits.
    Raw,
    /// ISO/IEC 10918-1 JPEG (`DCTDecode` / `DCT`).
    Dct,
    /// ITU-T T.4 / T.6 bi-level facsimile (`CCITTFaxDecode` / `CCF`).
    CcittFax,
    /// ITU-T T.88 / ISO/IEC 14492 bi-level (`JBIG2Decode`).
    Jbig2,
    /// ISO/IEC 15444 JPEG 2000 (`JPXDecode`).
    Jpx,
}

/// Hard-mask specification from `/Mask` on an image dictionary (ISO 32000-2, §8.9.6.3–§8.9.6.4).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MaskSpec {
    /// No `/Mask` entry.
    None,
    /// Explicit 1-bit `/ImageMask true` stream XObject.
    Stencil(ObjectId),
    /// Color-key mask `[min_0 max_0 ... min_{n-1} max_{n-1}]` in raw pre-`/Decode` sample integers.
    ColorKey(Vec<(u32, u32)>),
}

/// Embedded soft-mask mode `/SMaskInData` for `JPXDecode` streams (ISO 32000-2, Table 87).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SMaskInData {
    /// `0` (default): ignore any alpha channel in the image codestream.
    #[default]
    None,
    /// `1`: codestream includes a straight (un-premultiplied) opacity channel.
    Separate,
    /// `2`: codestream includes a premultiplied opacity channel (matte black `[0, ...]`).
    Premultiplied,
}

/// Parsed metadata for an Image XObject or inline image (`BI..ID..EI`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ImageMeta {
    /// Intrinsic pixel width (`/Width` or `/W`, `>= 1`).
    pub width: u32,
    /// Intrinsic pixel height (`/Height` or `/H`, `>= 1`).
    pub height: u32,
    /// Bits per sample component (`1, 2, 4, 8, 16`).
    pub bits_per_component: u8,
    /// Number of color components per pixel (`1` for `/ImageMask true` stencils).
    pub components: u8,
    /// Resolved color space family (`None` for `/ImageMask true` stencils).
    pub color_space: Option<Family>,
    /// `true` when `/ImageMask` (`/IM`) is `true`.
    pub stencil: bool,
    /// Interpolation hint (`/Interpolate` or `/I`; default `false`).
    pub interpolate: bool,
    /// Resolved `/Decode` intervals `(d_min, d_max)` of length `components`.
    pub decode: Vec<(f64, f64)>,
    /// Optional `/Mask` specification (explicit stencil stream or pre-`/Decode` color-key array).
    pub mask: MaskSpec,
    /// Optional indirect `/SMask` soft-mask stream ID (ignored when `smask_in_data != None` per
    /// ISO 32000-2 Table 87).
    pub smask: Option<ObjectId>,
    /// `/SMaskInData` mode (`0, 1, 2`) for `JPXDecode` images.
    pub smask_in_data: SMaskInData,
    /// Optional `/Intent` rendering intent.
    pub intent: Option<RenderingIntent>,
    /// Optional `/OC` optional-content reference.
    pub oc: Option<ObjectId>,
}

impl ImageMeta {
    /// Constructs [`ImageMeta`] with standard defaults (`interpolate = false`,
    /// [`MaskSpec::None`], `smask = None`, [`SMaskInData::None`], `intent = None`, `oc = None`,
    /// and standard Table 90 `/Decode` intervals for `color_space` and `bits_per_component`).
    ///
    /// All fields on [`ImageMeta`] are `pub`, so callers can customize `decode`, `mask`, `smask`,
    /// `interpolate`, `intent`, or `oc` on the returned struct before calling
    /// [`RawImage::from_parts`](crate::RawImage::from_parts).
    #[must_use]
    pub fn new(
        width: u32,
        height: u32,
        bits_per_component: u8,
        components: u8,
        color_space: Option<Family>,
        stencil: bool,
    ) -> Self {
        let decode = if stencil {
            vec![(0.0, 1.0)]
        } else {
            match color_space {
                Some(Family::Indexed) => {
                    let hi = if (1..=32).contains(&bits_per_component) {
                        ((1_u64 << bits_per_component) - 1) as f64
                    } else {
                        255.0
                    };
                    vec![(0.0, hi)]
                }
                Some(Family::Lab) => vec![(0.0, 100.0), (-100.0, 100.0), (-100.0, 100.0)],
                _ => vec![(0.0, 1.0); usize::from(components)],
            }
        };
        Self {
            width,
            height,
            bits_per_component,
            components,
            color_space,
            stencil,
            interpolate: false,
            decode,
            mask: MaskSpec::None,
            smask: None,
            smask_in_data: SMaskInData::None,
            intent: None,
            oc: None,
        }
    }

    /// Whether `/Decode` equals the default intervals for this image's color space and bit depth.
    #[must_use]
    pub fn has_default_decode(&self, space: Option<&PreparedSpace>) -> bool {
        if self.stencil {
            return self.decode == [(0.0, 1.0)];
        }
        match space.and_then(|s| s.default_decode(self.bits_per_component)) {
            Some(def) => self.decode == def,
            None => false,
        }
    }
}

/// Resolves an indirect reference one step (up to bounded hops) using `objects`.
pub(crate) fn resolve_obj<'o, R: Resolve<'o>>(
    objects: R,
    obj: &'o Object,
) -> Result<Option<&'o Object>, Error> {
    let mut cur = obj;
    for _ in 0..16 {
        match cur.as_reference() {
            Some(id) => match objects.try_resolve(id)? {
                Some(target) => cur = target,
                None => return Ok(None),
            },
            None => return Ok(Some(cur)),
        }
    }
    Ok(None)
}

pub(crate) fn read_int<'o, R: Resolve<'o>>(
    objects: R,
    dict: &Dict,
    key: &[u8],
) -> Result<Option<i64>, Error> {
    let Some(raw) = dict.get(key) else {
        return Ok(None);
    };
    if let Some(id) = raw.as_reference() {
        let Some(target) = objects.try_resolve(id)? else {
            return Ok(None);
        };
        let Some(obj) = resolve_obj(objects, target)? else {
            return Ok(None);
        };
        return Ok(obj_as_int(obj));
    }
    Ok(obj_as_int(raw))
}

fn obj_as_int(obj: &Object) -> Option<i64> {
    obj.as_integer().or_else(|| {
        obj.as_number()
            .filter(|v| v.is_finite() && v.fract() == 0.0)
            .and_then(|v| {
                if v >= i64::MIN as f64 && v < i64::MAX as f64 {
                    Some(v as i64)
                } else {
                    None
                }
            })
    })
}

pub(crate) fn read_bool<'o, R: Resolve<'o>>(
    objects: R,
    dict: &Dict,
    key: &[u8],
) -> Result<Option<bool>, Error> {
    let Some(raw) = dict.get(key) else {
        return Ok(None);
    };
    if let Some(id) = raw.as_reference() {
        let Some(target) = objects.try_resolve(id)? else {
            return Ok(None);
        };
        let Some(obj) = resolve_obj(objects, target)? else {
            return Ok(None);
        };
        return Ok(obj.as_bool());
    }
    Ok(raw.as_bool())
}

/// Checks `width` and `height` against `limits` before any pixel allocation.
pub(crate) fn check_dimensions(width: u32, height: u32, limits: &Limits) -> Result<u64, Error> {
    if width == 0 || height == 0 {
        return Err(Error::NotAnImage);
    }
    if width > limits.max_width {
        return Err(Error::Limit {
            kind: LimitKind::Width,
            value: u64::from(width),
            max: u64::from(limits.max_width),
        });
    }
    if height > limits.max_height {
        return Err(Error::Limit {
            kind: LimitKind::Height,
            value: u64::from(height),
            max: u64::from(limits.max_height),
        });
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > limits.max_pixels {
        return Err(Error::Limit {
            kind: LimitKind::Pixels,
            value: pixels,
            max: limits.max_pixels,
        });
    }
    Ok(pixels)
}

/// Checks that `bytes` fits within `limits.max_sample_bytes`.
pub(crate) fn check_sample_budget(bytes: u64, limits: &Limits) -> Result<usize, Error> {
    let max = limits.max_sample_bytes as u64;
    if bytes > max {
        return Err(Error::Limit {
            kind: LimitKind::SampleBytes,
            value: bytes,
            max,
        });
    }
    usize::try_from(bytes).map_err(|_| Error::Limit {
        kind: LimitKind::SampleBytes,
        value: bytes,
        max,
    })
}

/// Resolves a `/ColorSpace` entry from an image dictionary or `/Resources/ColorSpace`.
pub(crate) fn lookup_color_space_object<'o, R: Resolve<'o> + Copy>(
    objects: R,
    dict: &'o Dict,
    spaces: Option<Spaces<'o>>,
) -> Result<Option<&'o Object>, Error> {
    let Some(raw) = dict.get(b"ColorSpace") else {
        return Ok(None);
    };
    let Some(cs_obj) = resolve_obj(objects, raw)? else {
        return Ok(None);
    };
    if let Some(name) = cs_obj.as_name()
        && !matches!(
            name,
            b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK" | b"G" | b"RGB" | b"CMYK" | b"Pattern"
        )
        && let Some(named) = spaces.and_then(|s| s.get(name))
    {
        return resolve_obj(objects, named.value);
    }
    Ok(Some(cs_obj))
}

/// Parses `/Decode` into `components` `(d_min, d_max)` pairs, falling back to `default_decode` and
/// reporting [`Fault::InvalidDecode`] if `/Decode` is present and malformed.
pub(crate) fn parse_decode<'o, R, S>(
    objects: R,
    dict: &'o Dict,
    components: u8,
    default_decode: Vec<(f64, f64)>,
    place: Option<Place>,
    sink: &mut S,
) -> Result<Vec<(f64, f64)>, Error>
where
    R: Resolve<'o> + Copy,
    S: Sink<Fault> + ?Sized,
{
    let Some(raw) = dict.get(b"Decode") else {
        return Ok(default_decode);
    };
    let at: Option<Span> = raw.at().or_else(|| place.and_then(|p| p.at));
    let fault_place = Place::new(place.and_then(|p| p.object), at);
    let emit_fault = |sink: &mut S| {
        sink.fault(fault_place, Fault::InvalidDecode);
    };
    let Some(obj) = resolve_obj(objects, raw)? else {
        emit_fault(sink);
        return Ok(default_decode);
    };
    let Some(arr) = obj.as_array() else {
        emit_fault(sink);
        return Ok(default_decode);
    };
    let needed = usize::from(components) * 2;
    if arr.len() != needed {
        emit_fault(sink);
        return Ok(default_decode);
    }
    let mut pairs = Vec::with_capacity(usize::from(components));
    for [lo_obj, hi_obj] in arr.as_chunks::<2>().0 {
        let lo = resolve_obj(objects, lo_obj)?.and_then(Object::as_number);
        let hi = resolve_obj(objects, hi_obj)?.and_then(Object::as_number);
        match (lo, hi) {
            (Some(a), Some(b)) if a.is_finite() && b.is_finite() => pairs.push((a, b)),
            _ => {
                emit_fault(sink);
                return Ok(default_decode);
            }
        }
    }
    Ok(pairs)
}

/// Parses `/Mask` on an image dictionary into [`MaskSpec`].
pub(crate) fn parse_mask<'o, R, S>(
    objects: R,
    dict: &'o Dict,
    components: u8,
    bpc: u8,
    stencil: bool,
    place: Option<Place>,
    sink: &mut S,
) -> Result<MaskSpec, Error>
where
    R: Resolve<'o> + Copy,
    S: Sink<Fault> + ?Sized,
{
    let Some(raw) = dict.get(b"Mask") else {
        return Ok(MaskSpec::None);
    };
    let fault_place = Place::new(
        place.and_then(|p| p.object),
        raw.at().or_else(|| place.and_then(|p| p.at)),
    );
    if stencil {
        sink.fault(fault_place, Fault::InvalidMask);
        return Ok(MaskSpec::None);
    }
    if let Some(id) = raw.as_reference() {
        let Some(target) = objects.try_resolve(id)? else {
            sink.fault(fault_place, Fault::InvalidMask);
            return Ok(MaskSpec::None);
        };
        if target.as_stream().is_some() {
            return Ok(MaskSpec::Stencil(id));
        }
        if let Some(arr) = target.as_array() {
            return parse_color_key(objects, arr, components, bpc, fault_place, sink);
        }
        sink.fault(fault_place, Fault::InvalidMask);
        return Ok(MaskSpec::None);
    }
    if let Some(arr) = raw.as_array() {
        return parse_color_key(objects, arr, components, bpc, fault_place, sink);
    }
    sink.fault(fault_place, Fault::InvalidMask);
    Ok(MaskSpec::None)
}

fn parse_color_key<'o, R, S>(
    objects: R,
    arr: &'o [Object],
    components: u8,
    bpc: u8,
    place: Place,
    sink: &mut S,
) -> Result<MaskSpec, Error>
where
    R: Resolve<'o> + Copy,
    S: Sink<Fault> + ?Sized,
{
    let needed = usize::from(components) * 2;
    if needed == 0 || arr.len() < needed || !arr.len().is_multiple_of(2) {
        sink.fault(place, Fault::InvalidMask);
        return Ok(MaskSpec::None);
    }
    let max_sample = if bpc >= 32 {
        u32::MAX
    } else {
        (1_u32 << bpc) - 1
    };
    let mut ranges = Vec::with_capacity(usize::from(components));
    for [lo_obj, hi_obj] in arr[..needed].as_chunks::<2>().0 {
        let lo = resolve_obj(objects, lo_obj)?.and_then(Object::as_integer);
        let hi = resolve_obj(objects, hi_obj)?.and_then(Object::as_integer);
        let (Some(lo), Some(hi)) = (lo, hi) else {
            sink.fault(place, Fault::InvalidMask);
            return Ok(MaskSpec::None);
        };
        let lo_clamped = lo.clamp(0, i64::from(max_sample)) as u32;
        let hi_clamped = if hi < 0 {
            // A negative upper bound can never match a non-negative sample; store an inverted range
            // `(1, 0)` so `lo <= sample <= hi` evaluates false.
            ranges.push((1, 0));
            continue;
        } else {
            hi.min(i64::from(max_sample)) as u32
        };
        ranges.push((lo_clamped, hi_clamped));
    }
    Ok(MaskSpec::ColorKey(ranges))
}
