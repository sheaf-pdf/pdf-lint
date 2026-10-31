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
    /// marked content: 512 by default. The tokens keep what's open on the
    /// heap, but reading an operand as an object
    /// ([`Operand::object`](crate::Operand::object)) nests on the stack a
    /// level for each, some 0.4 KiB a level optimized and twice that
    /// unoptimized, so a cap past a few thousand needs a thread with a
    /// stack to match, or content can overflow it.
    pub depth: usize,
    /// How many values reading one operand as an object may build
    /// ([`Operand::object`](crate::Operand::object)): it, an array's
    /// elements, a dictionary's keys and values, and theirs; past it, it
    /// reads as none. What a value holds in memory grows with its count
    /// of values more than with its bytes (an empty array is two bytes
    /// written, and tens in memory), so this caps that. 2^25 by default,
    /// as pdf-objects' parser has it.
    pub values: usize,
    /// An inline image's data, in bytes: 64 MiB by default.
    pub inline_image: usize,
    /// A page's streams joined ([`Join`](crate::Join)), in bytes, the
    /// space between each among them: 256 MiB by default. A place in them
    /// is counted in 32 bits ([`Pos`](crate::Pos)), so `Join` refuses past
    /// 4 GiB, whatever this says.
    pub content: usize,
}

impl Limits {
    /// The caps content is read under unless it's given others.
    pub const DEFAULT: Limits = Limits {
        operands: 1 << 16,
        depth: 512,
        values: 1 << 25,
        inline_image: 64 << 20,
        content: 256 << 20,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}
