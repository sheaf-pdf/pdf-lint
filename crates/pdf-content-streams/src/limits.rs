//! The caps content is read under: one value for all of them.

/// The caps on reading content: past one, it can't be read, and reading
/// stops with an [`Error`](crate::Error) saying which and where. A caller
/// sets them on a copy of [`Limits::DEFAULT`] (`limits.operands = 64`,
/// say), as more may come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Operands before an operator: 65,536 by default. PDFium keeps the
    /// last 16, and no operator of ISO 32000-2 takes more than six but
    /// `SCN` and `scn`, which take a component for each of a DeviceN
    /// space's colorants, to which PDF 2.0 sets no limit, and a pattern's
    /// name; and a stream may hold runs of numbers no operator ends.
    pub operands: usize,
    /// How deep arrays and dictionaries nest, as PDFium caps them, and
    /// marked content: 512 by default.
    pub depth: usize,
    /// An inline image's data, in bytes: 64 MiB by default.
    pub inline_image: usize,
    /// A page's streams joined ([`Join`](crate::Join)), in bytes, the
    /// space between each among them: 256 MiB by default.
    pub content: usize,
}

impl Limits {
    /// The caps content is read under unless it's given others.
    pub const DEFAULT: Limits = Limits {
        operands: 1 << 16,
        depth: 512,
        inline_image: 64 << 20,
        content: 256 << 20,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}
