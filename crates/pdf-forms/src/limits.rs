//! Resource caps for the bounded field tree walk, and for reading a field's
//! font and making its appearance.

/// Per-call resource caps enforced while reading an interactive form.
///
/// The walk over `/Fields` and each field's `/Kids`, the indirect references it
/// follows, and the items of a choice field's value all end within these
/// bounds, whatever the file says; so do reading a field's font from the
/// default resources and making an appearance (`text-layout`).
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
    /// Maximum bytes of display text an appearance is made of,
    /// `text-layout` (`262_144` by default); past it, [`Cap::Text`].
    pub text: usize,
    /// Maximum cells a comb field's appearance divides its box into
    /// (`/MaxLen`), `text-layout` (`1_024` by default); past it,
    /// [`Cap::CombCells`]. A divider may be drawn for each cell whatever
    /// the text, so the cells bound what an empty comb field's appearance
    /// holds.
    pub comb_cells: usize,
    /// Caps forwarded to `pdf-fonts` when a field's font is read from the
    /// default resources (`FieldFont::find`, `text-layout`).
    pub fonts: pdf_fonts::Limits,
    /// Maximum bytes one such font's streams decompress to: its `/ToUnicode`,
    /// an embedded CMap's chain, and with `programs` its program (`128 MiB` by
    /// default); past it, [`Cap::FontBudget`].
    pub font_budget: usize,
}

impl Limits {
    /// Default caps suitable for untrusted real-world PDFs.
    pub const DEFAULT: Limits = Limits {
        hops: 32,
        fields: 131_072,
        depth: 64,
        items: 16_384,
        annots: pdf_annots::Limits::DEFAULT,
        text: 262_144,
        comb_cells: 1_024,
        fonts: pdf_fonts::Limits::DEFAULT,
        font_budget: 128 << 20,
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
    /// The display text of an appearance exceeded [`Limits::text`].
    Text,
    /// A comb field's cells exceeded [`Limits::comb_cells`].
    CombCells,
    /// A field's font's streams decompressed past [`Limits::font_budget`].
    FontBudget,
}

impl std::fmt::Display for Cap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cap::Fields => f.write_str("fields and widgets in a form"),
            Cap::Items => f.write_str("items in a field's value"),
            Cap::Annots(cap) => write!(f, "{cap}"),
            Cap::Text => f.write_str("bytes of an appearance's text"),
            Cap::CombCells => f.write_str("cells of a comb field"),
            Cap::FontBudget => f.write_str("bytes a field's font decompresses to"),
        }
    }
}
