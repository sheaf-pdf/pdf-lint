//! Form XObjects, placed Image XObjects / inline images, and painted shadings
//! (`CPDF_StreamContentParser::Handle_ExecuteXObject`, `Handle_ShadeFill`).
//!
//! Images are inspected from their stream dictionary or inline `BI..ID` header
//! alone—pixel samples are never decoded.

use std::ops::Range;
use std::sync::Arc;

use pdf_content_streams::{InlineImage, Limits as ContentLimits};
use pdf_objects::fault::Sink;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Streams, Why};

use crate::error::Error;
use crate::fault::Fault;
use crate::geom::{Matrix, Quad, Rect};
use crate::reader::ObjReader;

/// Stable path of `Do` byte offsets from the page's content stream down to a
/// specific Form XObject invocation.
///
/// Repeated identical runs over the same content produce equal [`UseId`]s, and
/// two invocations of the same Form XObject at different `Do` call sites have
/// distinct [`UseId`]s.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UseId(Arc<[u32]>);

impl UseId {
    pub(crate) fn new(path: &[u32]) -> Self {
        Self(path.into())
    }

    /// Byte offset of each `Do` operator from outermost (in the page's content
    /// stream) to innermost.
    #[must_use]
    pub fn path(&self) -> &[u32] {
        &self.0
    }

    /// Form nesting depth (`1` for a Form XObject drawn directly by the page).
    #[must_use]
    pub fn depth(&self) -> usize {
        self.0.len()
    }
}

/// Metadata for an active Form XObject invocation (`Do`), emitted with
/// [`Kind::FormBegin`](crate::Kind::FormBegin) and
/// [`Kind::FormEnd`](crate::Kind::FormEnd).
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct FormUse<'e> {
    /// Indirect object ID of the Form XObject stream.
    pub form: ObjectId,
    /// Call-site path identifying this specific invocation.
    pub id: &'e UseId,
    /// Form space onto PageSpace (`form_matrix.then(caller_ctm)`).
    pub matrix: Matrix,
    /// Form `/BBox` in form space, if present as an array (`None` when absent
    /// or not an array).
    pub bbox: Option<Rect>,
    /// Optional content group or membership dictionary (`/OC`) referenced by
    /// the Form XObject dictionary, if any.
    pub oc: Option<ObjectId>,
}

/// Reason a Form XObject invocation (`Do`) was not entered
/// ([`Kind::FormRefused`](crate::Kind::FormRefused)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Refused {
    /// Recursive `Do` cycle: `form` is already active on the current Form
    /// XObject call chain.
    Cycle,
    /// Nesting depth reached [`Limits::form_depth`](crate::Limits::form_depth)
    /// (PDFium's `kMaxFormLevel = 40` by default).
    Depth,
}

/// Where a placed image's encoded data lives.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImageOf {
    /// An indirect Image XObject stream invoked via `Do`.
    XObject(ObjectId),
    /// An inline image (`BI` .. `ID` .. `EI`) in the active content stream.
    Inline {
        /// Byte range of the inline image's data within [`At::content`](crate::At::content).
        data: Range<usize>,
    },
}

/// A placed Image XObject (`Do`) or inline image (`BI..EI`), without decoding
/// pixel data.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ImageDrawn {
    /// Source of the image (`XObject` stream or `Inline` byte range).
    pub of: ImageOf,
    /// Placed unit square (`[0, 1] × [0, 1]`) transformed into PageSpace by the
    /// active CTM (`FPDFPageObj_GetRotatedBounds`).
    pub quad: Quad,
    /// Intrinsic pixel width (`/Width` or `/W`), when a positive integer.
    pub width: Option<u32>,
    /// Intrinsic pixel height (`/Height` or `/H`), when a positive integer.
    pub height: Option<u32>,
    /// Bits per color component (`/BitsPerComponent` or `/BPC`), when `1..=32`.
    pub bits_per_component: Option<u8>,
    /// Interpolation hint (`/Interpolate` or `/I`; default `false`).
    pub interpolate: bool,
    /// `true` when `/ImageMask` (or `/IM`) is `true` (a 1-bit stencil mask
    /// painted in the current fill color).
    pub stencil: bool,
    /// Indirect `/SMask` soft-mask image reference, if any.
    pub smask: Option<ObjectId>,
    /// Indirect `/Mask` stencil image stream reference, if any (color-key array
    /// masks are inline on the image dictionary and leave `mask: None`).
    pub mask: Option<ObjectId>,
    /// Optional content reference (`/OC`) on an Image XObject, if any.
    pub oc: Option<ObjectId>,
}

impl ImageDrawn {
    pub(crate) fn new(of: ImageOf, facts: &ImageFacts, ctm: Matrix) -> Self {
        Self {
            of,
            quad: ctm.apply_quad(Rect::UNIT),
            width: facts.width,
            height: facts.height,
            bits_per_component: facts.bits_per_component,
            interpolate: facts.interpolate,
            stencil: facts.stencil,
            smask: facts.smask,
            mask: facts.mask,
            oc: facts.oc,
        }
    }

    /// Effective horizontal and vertical resolution in dots per inch (`DPI`)
    /// assuming default user unit `1.0` (`1/72` inch).
    ///
    /// Returns `None` if `width` or `height` is missing/zero or if the placed
    /// `quad` has zero width or height.
    #[must_use]
    pub fn dpi(&self) -> Option<(f32, f32)> {
        self.dpi_with_unit(1.0)
    }

    /// Effective horizontal and vertical resolution in dots per inch (`DPI`)
    /// accounting for page `/UserUnit` (`user_unit = 1.0` -> `1/72` inch).
    #[must_use]
    pub fn dpi_with_unit(&self, user_unit: f32) -> Option<(f32, f32)> {
        if !(user_unit > 0.0 && user_unit.is_finite()) {
            return None;
        }
        let (w, h) = (self.width? as f32, self.height? as f32);
        let inches_x = self.quad.baseline_len() * user_unit / 72.0;
        let inches_y = self.quad.side_len() * user_unit / 72.0;
        if !(inches_x > 0.0 && inches_y > 0.0) {
            return None;
        }
        let (dx, dy) = (w / inches_x, h / inches_y);
        (dx.is_finite() && dy.is_finite()).then_some((dx, dy))
    }
}

/// A shading painted by `sh` (`ISO 32000-2`, §8.7.4.2).
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct ShadingDrawn {
    /// Indirect object ID of the shading dictionary/stream when bound by
    /// reference in `/Resources/Shading`.
    pub shading: Option<ObjectId>,
    /// Conservative axis-aligned bounding box in PageSpace: the intersection of
    /// `State::clip_bounds` and the shading's `/BBox` transformed by the CTM,
    /// or `None` when neither bounds it (the shading fills the entire page).
    pub bounds: Option<Rect>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ImageFacts {
    pub(crate) width: Option<u32>,
    pub(crate) height: Option<u32>,
    pub(crate) bits_per_component: Option<u8>,
    pub(crate) interpolate: bool,
    pub(crate) stencil: bool,
    pub(crate) smask: Option<ObjectId>,
    pub(crate) mask: Option<ObjectId>,
    pub(crate) oc: Option<ObjectId>,
}

impl ImageFacts {
    pub(crate) fn from_inline(image: &InlineImage<'_>, lim: &ContentLimits) -> Self {
        let pos_u32 = |full: &[u8]| -> Option<u32> {
            let obj = image.entry(full)?.object(lim)?;
            let n = obj.as_integer()?;
            u32::try_from(n).ok().filter(|&v| v > 0)
        };
        let bpc = image
            .entry(b"BitsPerComponent")
            .and_then(|o| o.object(lim))
            .and_then(|o| o.as_integer())
            .and_then(|n| u8::try_from(n).ok())
            .filter(|&v| (1..=32).contains(&v));
        let bool_entry = |full: &[u8]| -> bool {
            image
                .entry(full)
                .and_then(|o| o.object(lim))
                .and_then(|o| o.as_bool())
                == Some(true)
        };
        Self {
            width: pos_u32(b"Width"),
            height: pos_u32(b"Height"),
            bits_per_component: bpc,
            interpolate: bool_entry(b"Interpolate"),
            stencil: bool_entry(b"ImageMask"),
            smask: None,
            mask: None,
            oc: None,
        }
    }
}

pub(crate) struct Form<'o> {
    pub(crate) data: Vec<u8>,
    pub(crate) matrix: Matrix,
    pub(crate) bbox: Option<Rect>,
    pub(crate) resources: Option<&'o Object>,
    pub(crate) oc: Option<ObjectId>,
}

pub(crate) enum XObject<'o> {
    Form(Form<'o>),
    SkippedForm,
    Image(ImageFacts),
    Other,
    None,
    TooLarge,
}

impl<'o> XObject<'o> {
    pub(crate) fn read<O: Resolve<'o> + Streams, S: Sink<Fault> + ?Sized>(
        reader: &ObjReader<'_, O, S>,
        id: ObjectId,
        budget: usize,
        decode_forms: bool,
    ) -> Result<XObject<'o>, Error> {
        let Some(obj) = reader.resolve(id) else {
            return match reader.take_failure() {
                Some(unreadable) => Err(Error::Unreadable(unreadable)),
                None => Ok(XObject::None),
            };
        };
        let Some(stream) = obj.as_stream() else {
            return Ok(XObject::None);
        };
        let dict = &stream.dict;
        let subtype = reader.name_or_bytes_for(dict, b"Subtype");
        if let Some(unreadable) = reader.take_failure() {
            return Err(Error::Unreadable(unreadable));
        }
        match subtype {
            Some(b"Form") if !decode_forms => return Ok(XObject::SkippedForm),
            Some(b"Form") => {}
            Some(b"Image") => {
                let facts = read_image_xobject(reader, dict);
                if let Some(unreadable) = reader.take_failure() {
                    return Err(Error::Unreadable(unreadable));
                }
                return Ok(XObject::Image(facts));
            }
            _ => return Ok(XObject::Other),
        }

        let decoded = match reader.objects.decoded(id, budget) {
            Ok(d) => d.into_owned(),
            Err(unreadable) if unreadable.why == Why::Limit => return Ok(XObject::TooLarge),
            Err(unreadable) if matches!(unreadable.why, Why::Damaged | Why::Filter) => {
                return Err(Error::FormStream {
                    form: id,
                    source: unreadable,
                });
            }
            Err(unreadable) => return Err(Error::Unreadable(unreadable)),
        };

        let matrix = form_matrix(reader, dict);
        let bbox = dict_array4(reader, dict, b"BBox").map(|[a, b, c, d]| Rect::new(a, b, c, d));
        let resources = dict
            .get(b"Resources")
            .filter(|r| reader.plain_dict_of(r).is_some());
        let oc = dict.get(b"OC").and_then(Object::as_reference);
        if let Some(unreadable) = reader.take_failure() {
            return Err(Error::Unreadable(unreadable));
        }
        Ok(XObject::Form(Form {
            data: decoded,
            matrix,
            bbox,
            resources,
            oc,
        }))
    }
}

fn read_image_xobject<'o, O: Resolve<'o> + Streams, S: Sink<Fault> + ?Sized>(
    reader: &ObjReader<'_, O, S>,
    dict: &'o Dict,
) -> ImageFacts {
    let pos_u32 = |key: &[u8]| -> Option<u32> {
        let val = dict.get(key)?;
        let n = reader.integer_of(val)?;
        u32::try_from(n).ok().filter(|&v| v > 0)
    };
    let bpc = dict
        .get(b"BitsPerComponent")
        .and_then(|v| reader.integer_of(v))
        .and_then(|n| u8::try_from(n).ok())
        .filter(|&v| (1..=32).contains(&v));
    let stencil = reader.bool_for(dict, b"ImageMask") == Some(true);
    let interpolate = reader.bool_for(dict, b"Interpolate") == Some(true);
    let smask = dict.get(b"SMask").and_then(Object::as_reference);
    let mask = dict
        .get(b"Mask")
        .and_then(Object::as_reference)
        .filter(|&id| reader.stream(id).is_some());
    let oc = dict.get(b"OC").and_then(Object::as_reference);
    ImageFacts {
        width: pos_u32(b"Width"),
        height: pos_u32(b"Height"),
        bits_per_component: bpc,
        interpolate,
        stencil,
        smask,
        mask,
        oc,
    }
}

fn form_matrix<'o, O: Resolve<'o> + Streams, S: Sink<Fault> + ?Sized>(
    reader: &ObjReader<'_, O, S>,
    dict: &'o Dict,
) -> Matrix {
    let Some(arr) = reader.array_for(dict, b"Matrix") else {
        return Matrix::IDENTITY;
    };
    if arr.len() != 6 {
        return Matrix::IDENTITY;
    }
    let finite_at = |i: usize| {
        let v = reader.float_at(arr, i);
        if v.is_finite() { v } else { 0.0 }
    };
    Matrix {
        a: finite_at(0),
        b: finite_at(1),
        c: finite_at(2),
        d: finite_at(3),
        e: finite_at(4),
        f: finite_at(5),
    }
}

pub(crate) fn dict_array4<'o, O: Resolve<'o> + Streams, S: Sink<Fault> + ?Sized>(
    reader: &ObjReader<'_, O, S>,
    dict: &'o Dict,
    key: &[u8],
) -> Option<[f32; 4]> {
    let arr = reader.array_for(dict, key)?;
    if arr.len() != 4 {
        return Some([0.0; 4]);
    }
    let finite_at = |i: usize| {
        let v = reader.float_at(arr, i);
        if v.is_finite() { v } else { 0.0 }
    };
    Some([finite_at(0), finite_at(1), finite_at(2), finite_at(3)])
}
