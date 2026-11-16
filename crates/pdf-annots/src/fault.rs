//! Non-fatal ISO 32000-2 departures reported while inspecting annotations,
//! actions, and destinations.
//!
//! `Fault`'s `Debug` implementation reports static key names, counts, and
//! object IDs—never decrypted strings, URIs, destination names, or author text.

use std::fmt;

use pdf_objects::ObjectId;

/// Recoverable specification departure reported to a
/// [`pdf_objects::fault::Sink`].
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Fault {
    /// An indirect reference chain exceeded [`Limits::hops`](crate::Limits::hops)
    /// or formed a cycle.
    ReferenceLoop,
    /// An indirect object could not be parsed (`Why::Damaged`).
    DamagedObject(ObjectId),
    /// A page's `/Annots` entry is not an array.
    AnnotsNotArray,
    /// An entry in `/Annots` is a dangling indirect reference (`null`).
    DanglingAnnot(ObjectId),
    /// An entry in `/Annots` is neither an indirect dictionary nor a direct
    /// dictionary.
    AnnotNotDict,
    /// An annotation in `/Annots` is written as a direct dictionary rather than
    /// an indirect reference (ISO 32000-2 §12.5.2 / §7.7.3.3).
    DirectAnnot,
    /// The same indirect annotation `ObjectId` appears more than once in a
    /// page's `/Annots` array.
    DuplicateAnnot(ObjectId),
    /// An annotation dictionary is missing its required `/Subtype` entry or its
    /// value is not a name.
    MissingSubtype,
    /// An annotation dictionary is missing its required `/Rect` entry or `/Rect`
    /// is not an array of four finite numbers.
    InvalidRect,
    /// A dictionary entry did not have the type required by ISO 32000-2.
    WrongType {
        /// Dictionary key name (e.g. `"F"`, `"Contents"`, `"C"`, `"Border"`).
        key: &'static str,
    },
    /// A date string (`/M` or `/CreationDate`) was not a valid PDF `D:...`
    /// date.
    MalformedDate {
        /// Dictionary key name (`"M"` or `"CreationDate"`).
        key: &'static str,
    },
    /// `/C` or `/IC` had an element count other than `0`, `1`, `3`, or `4`, or
    /// non-numeric/out-of-range components.
    InvalidColor {
        /// Dictionary key name (`"C"` or `"IC"`).
        key: &'static str,
    },
    /// `/QuadPoints` was not an array of a non-zero multiple of 8 finite
    /// numbers.
    InvalidQuadPoints,
    /// `/Vertices`, `/L`, `/CL`, or `/InkList` contained malformed coordinates
    /// or an odd coordinate count.
    InvalidPoints {
        /// Dictionary key name (`"L"`, `"CL"`, `"Vertices"`, or `"InkList"`).
        key: &'static str,
    },
    /// A `/DA` default appearance string failed to lex or lacked a valid `Tf`
    /// font operator.
    InvalidDefaultAppearance,
    /// Both `/Dest` and `/A` are present on a `/Link` annotation (forbidden by
    /// ISO 32000-2 §12.5.6.5 Table 176).
    LinkHasBothDestAndAction,
    /// An `/IRT` reply chain formed a cycle or pointed to a missing/non-dict
    /// object.
    InvalidReplyChain,
    /// Underlying departure reported by `pdf-semantics` while inspecting
    /// destinations, actions, or `/Names/Dests`.
    Semantics(pdf_semantics::Fault),
}

impl From<pdf_semantics::Fault> for Fault {
    fn from(value: pdf_semantics::Fault) -> Self {
        Self::Semantics(value)
    }
}

impl fmt::Debug for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReferenceLoop => f.write_str("ReferenceLoop"),
            Self::DamagedObject(id) => f.debug_tuple("DamagedObject").field(id).finish(),
            Self::AnnotsNotArray => f.write_str("AnnotsNotArray"),
            Self::DanglingAnnot(id) => f.debug_tuple("DanglingAnnot").field(id).finish(),
            Self::AnnotNotDict => f.write_str("AnnotNotDict"),
            Self::DirectAnnot => f.write_str("DirectAnnot"),
            Self::DuplicateAnnot(id) => f.debug_tuple("DuplicateAnnot").field(id).finish(),
            Self::MissingSubtype => f.write_str("MissingSubtype"),
            Self::InvalidRect => f.write_str("InvalidRect"),
            Self::WrongType { key } => f.debug_struct("WrongType").field("key", key).finish(),
            Self::MalformedDate { key } => {
                f.debug_struct("MalformedDate").field("key", key).finish()
            }
            Self::InvalidColor { key } => f.debug_struct("InvalidColor").field("key", key).finish(),
            Self::InvalidQuadPoints => f.write_str("InvalidQuadPoints"),
            Self::InvalidPoints { key } => {
                f.debug_struct("InvalidPoints").field("key", key).finish()
            }
            Self::InvalidDefaultAppearance => f.write_str("InvalidDefaultAppearance"),
            Self::LinkHasBothDestAndAction => f.write_str("LinkHasBothDestAndAction"),
            Self::InvalidReplyChain => f.write_str("InvalidReplyChain"),
            Self::Semantics(inner) => f.debug_tuple("Semantics").field(inner).finish(),
        }
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReferenceLoop => f.write_str("indirect reference loop or hop limit exceeded"),
            Self::DamagedObject(id) => write!(f, "object {id} is damaged"),
            Self::AnnotsNotArray => f.write_str("page /Annots is not an array"),
            Self::DanglingAnnot(id) => write!(f, "/Annots entry {id} is a dangling reference"),
            Self::AnnotNotDict => f.write_str("/Annots entry is not a dictionary"),
            Self::DirectAnnot => f.write_str("/Annots entry is a direct dictionary"),
            Self::DuplicateAnnot(id) => write!(f, "duplicate annotation {id} in /Annots"),
            Self::MissingSubtype => f.write_str("annotation is missing a /Subtype name"),
            Self::InvalidRect => f.write_str("annotation /Rect is missing or not 4 finite numbers"),
            Self::WrongType { key } => write!(f, "annotation /{key} has an unexpected value type"),
            Self::MalformedDate { key } => write!(f, "annotation /{key} is not a valid PDF date"),
            Self::InvalidColor { key } => write!(f, "annotation /{key} color array is invalid"),
            Self::InvalidQuadPoints => {
                f.write_str("annotation /QuadPoints is not a non-empty multiple of 8 numbers")
            }
            Self::InvalidPoints { key } => {
                write!(f, "annotation /{key} point array is invalid")
            }
            Self::InvalidDefaultAppearance => {
                f.write_str("annotation /DA default appearance string is invalid")
            }
            Self::LinkHasBothDestAndAction => {
                f.write_str("Link annotation specifies both /Dest and /A")
            }
            Self::InvalidReplyChain => {
                f.write_str("annotation /IRT reply chain is invalid or cyclic")
            }
            Self::Semantics(inner) => write!(f, "{inner}"),
        }
    }
}
