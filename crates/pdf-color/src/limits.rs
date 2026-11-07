//! The caps a color space is read under: one value for all of them.

/// The caps on reading a color space: past one, what's left of it doesn't
/// load, as what doesn't load in the file wouldn't. A caller sets them on a
/// copy of [`Limits::DEFAULT`] (`limits.space_depth = 16`, say): it's
/// `#[non_exhaustive]`, as more may come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// How deep a space nests: an array of one, a base, an alternate, a tint
    /// function or a stitching function's subfunction, each a level below
    /// what holds it, and in a typed [`Space`](crate::Space) a profile's
    /// `/Alternate`, a process dictionary's `/ColorSpace` and an entry of
    /// `/Colorants` too, each reference on the way down taking a level of its
    /// own. 64 by default: a number of ours, to keep the stack small, and not
    /// PDFium's. PDFium has only its stack by reference, so past this they part.
    /// What's written in place PDFium's parser stops reading at 64 levels from
    /// the object's root, its page and dictionaries among them, a stitching
    /// function costing two, and this counts levels of a space from the value,
    /// so short of this they can part too (`tests/pdfium/differ.tsv`). Each level
    /// is a few calls on the stack, so a caller that sets it far higher gives
    /// them room.
    pub space_depth: usize,
    /// How many objects are looked up for one space, and how many
    /// subfunctions written in place are read: 4,096 by default. A stitching
    /// function whose subfunctions share subfunctions costs PDFium its
    /// exponent; here, past this, it doesn't load. It's per space, not per
    /// dictionary of them, so a large dictionary of honest spaces is never
    /// cut.
    ///
    /// That's also why no cap covers the whole: a dictionary's cost is its
    /// entries times this times what each visit scans, as long as the object's
    /// own dictionary, and a caller reading untrusted files bounds that by its
    /// own means for now.
    ///
    /// A typed [`Space`](crate::Space) also spends one on each name it keeps
    /// and each entry of `/Colorants` it reads, so what it holds is bounded
    /// too; past it the rest of them aren't kept. What only a check reads has
    /// as many again of its own, so a sink that wants faults reads the same
    /// space as `()`.
    pub space_visits: usize,
}

impl Limits {
    /// The caps a space is read under unless it's given others.
    pub const DEFAULT: Limits = Limits {
        space_depth: 64,
        space_visits: 4096,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}
