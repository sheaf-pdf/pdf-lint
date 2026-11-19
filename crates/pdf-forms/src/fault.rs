//! Non-fatal ISO 32000-2 departures reported while reading a form.
//!
//! `Fault`'s `Debug` implementation reports static key names and object IDs,
//! never field names or field values.

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
    /// A dictionary entry did not have the type required by ISO 32000-2.
    WrongType {
        /// Dictionary key name (e.g. `"Fields"`, `"Kids"`, `"Ff"`, `"Q"`).
        key: &'static str,
    },
    /// An entry of `/Fields` or `/Kids` is not a dictionary; it is left out.
    FieldNotDict,
    /// A field in `/Fields` or `/Kids` is written as a direct dictionary rather
    /// than an indirect reference.
    DirectField,
    /// The same field is reached a second time (a cycle through `/Kids`, or one
    /// field listed twice); the second is left out.
    FieldLoop(ObjectId),
    /// A field lies deeper than [`Limits::depth`](crate::Limits::depth); it and
    /// its descendants are left out.
    TooDeep,
    /// A terminal field has no `/FT`, its own or inherited, or one that names no
    /// field type.
    MissingFieldType,
    /// A field's `/Parent` is absent or doesn't name the field whose `/Kids`
    /// lists it.
    ParentMismatch,
    /// A field without `/Kids` at the top of the tree has no partial name
    /// (`/T`).
    MissingName,
    /// Underlying departure reported by `pdf-annots` while a widget was read.
    Annot(pdf_annots::Fault),
}

impl From<pdf_annots::Fault> for Fault {
    fn from(value: pdf_annots::Fault) -> Self {
        Self::Annot(value)
    }
}

impl fmt::Debug for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReferenceLoop => f.write_str("ReferenceLoop"),
            Self::DamagedObject(id) => f.debug_tuple("DamagedObject").field(id).finish(),
            Self::WrongType { key } => f.debug_struct("WrongType").field("key", key).finish(),
            Self::FieldNotDict => f.write_str("FieldNotDict"),
            Self::DirectField => f.write_str("DirectField"),
            Self::FieldLoop(id) => f.debug_tuple("FieldLoop").field(id).finish(),
            Self::TooDeep => f.write_str("TooDeep"),
            Self::MissingFieldType => f.write_str("MissingFieldType"),
            Self::ParentMismatch => f.write_str("ParentMismatch"),
            Self::MissingName => f.write_str("MissingName"),
            Self::Annot(inner) => f.debug_tuple("Annot").field(inner).finish(),
        }
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReferenceLoop => f.write_str("indirect reference loop or hop limit exceeded"),
            Self::DamagedObject(id) => write!(f, "object {id} is damaged"),
            Self::WrongType { key } => write!(f, "form /{key} has an unexpected value type"),
            Self::FieldNotDict => f.write_str("a field entry is not a dictionary"),
            Self::DirectField => f.write_str("a field is a direct dictionary"),
            Self::FieldLoop(id) => write!(f, "field {id} is reached more than once"),
            Self::TooDeep => f.write_str("the field tree is deeper than the depth limit"),
            Self::MissingFieldType => f.write_str("a terminal field has no field type"),
            Self::ParentMismatch => f.write_str("a field's /Parent doesn't name its parent"),
            Self::MissingName => f.write_str("a top-level field has no partial name"),
            Self::Annot(inner) => write!(f, "{inner}"),
        }
    }
}
