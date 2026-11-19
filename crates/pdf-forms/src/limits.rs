//! Resource caps for the bounded field tree walk.

/// Per-call resource caps enforced while reading an interactive form.
///
/// The walk over `/Fields` and each field's `/Kids`, the indirect references it
/// follows, and the items of a choice field's value all end within these
/// bounds, whatever the file says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Maximum indirect reference hops followed when resolving a value (`32` by
    /// default).
    pub hops: usize,
    /// Maximum fields and widgets read from one form, counted together
    /// (`131_072` by default).
    pub fields: usize,
    /// Maximum depth of the field tree; deeper fields are left out with
    /// [`Fault::TooDeep`](crate::Fault::TooDeep) (`64` by default).
    pub depth: usize,
    /// Maximum items read from one field's array value or default value
    /// (`16_384` by default).
    pub items: usize,
    /// Caps forwarded to `pdf-annots` when a widget is read as an annotation.
    pub annots: pdf_annots::Limits,
}

impl Limits {
    /// Default caps suitable for untrusted real-world PDFs.
    pub const DEFAULT: Limits = Limits {
        hops: 32,
        fields: 131_072,
        depth: 64,
        items: 16_384,
        annots: pdf_annots::Limits::DEFAULT,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}

/// Which [`Limits`] cap ended an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Cap {
    /// Fields and widgets in the form exceeded [`Limits::fields`].
    Fields,
    /// Items in a field's array value exceeded [`Limits::items`].
    Items,
    /// A cap of `pdf-annots` was exceeded while a widget was read.
    Annots(pdf_annots::Cap),
}

impl std::fmt::Display for Cap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cap::Fields => f.write_str("fields and widgets in a form"),
            Cap::Items => f.write_str("items in a field's value"),
            Cap::Annots(cap) => write!(f, "{cap}"),
        }
    }
}
