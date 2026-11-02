//! The caps a color space is read under: one value for all of them.

/// The caps on reading a color space: past one, what's left of it doesn't
/// load, as what doesn't load in the file wouldn't. A caller sets them on a
/// copy of [`Limits::DEFAULT`] (`limits.space_depth = 16`, say), as more may
/// come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// How deep a space nests: an array of one, a base, an alternate, a tint
    /// function or a stitching function's subfunction, each a level below
    /// what holds it. 64 by default, as deep as PDFium's parser nests arrays
    /// written in place; PDFium goes as deep as its stack does by reference,
    /// and here that's where they part. Each level is a few calls on the
    /// stack, so a caller that sets it far higher gives them room.
    pub space_depth: usize,
    /// How many objects are looked up for one space, and how many
    /// subfunctions written in place are read: 4,096 by default. A stitching
    /// function whose subfunctions share subfunctions costs PDFium its
    /// exponent; here, past this, it doesn't load. It's per space, not per
    /// dictionary of them, so a large dictionary of honest spaces is never
    /// cut.
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
