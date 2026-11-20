//! Non-fatal ISO 32000-2 departures reported while reading a form, a field's
//! font, or its default appearance string.
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
    /// Underlying departure reported by `pdf-gfx` while a default appearance
    /// string (`/DA`) was parsed, with its byte offset in the string where it
    /// has one.
    DefaultAppearance(pdf_gfx::Fault),
    /// A text field has no `/DA`, its own, inherited, or the form's, or one
    /// with no `Tf`: its text is drawn in Helvetica at an automatic size, as
    /// PDFium draws it (ISO 32000-2 §12.7.4.3 requires one with a `Tf`).
    MissingDefaultAppearance,
    /// The default resources (`/DR /Font`) have no font dictionary by the name
    /// a `/DA` string gives; a standard 14 font chosen by that name draws the
    /// text instead, and the caller adds it to `/DR`.
    FontNotInResources,
    /// Underlying departure reported by `pdf-fonts` while a field's font was
    /// read from the default resources.
    Font(pdf_fonts::Fault),
}

impl From<pdf_annots::Fault> for Fault {
    fn from(value: pdf_annots::Fault) -> Self {
        Self::Annot(value)
    }
}

impl From<pdf_gfx::Fault> for Fault {
    fn from(value: pdf_gfx::Fault) -> Self {
        Self::DefaultAppearance(value)
    }
}

impl From<pdf_fonts::Fault> for Fault {
    fn from(value: pdf_fonts::Fault) -> Self {
        Self::Font(value)
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
            Self::DefaultAppearance(inner) => {
                f.debug_tuple("DefaultAppearance").field(inner).finish()
            }
            Self::MissingDefaultAppearance => f.write_str("MissingDefaultAppearance"),
            Self::FontNotInResources => f.write_str("FontNotInResources"),
            Self::Font(inner) => f.debug_tuple("Font").field(inner).finish(),
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
            // `pdf_gfx::Fault` has no `Display`; its `Debug` holds no name.
            Self::DefaultAppearance(inner) => {
                write!(f, "a default appearance string departs: {inner:?}")
            }
            Self::MissingDefaultAppearance => {
                f.write_str("a text field has no default appearance with a font")
            }
            Self::FontNotInResources => {
                f.write_str("the default resources have no font by the name /DA gives")
            }
            Self::Font(inner) => write!(f, "{inner}"),
        }
    }
}
