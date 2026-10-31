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
    /// How deep arrays and dictionaries nest, and marked content: 512 by
    /// default. PDFium reads arrays and dictionaries 64 levels deep, the
    /// outermost at 0, and what's nested deeper as no object, which is an
    /// error here too ([`tokens`](crate::tokens)), whatever this says; an
    /// array inside an array, outside any dictionary, opens none, so nests
    /// no deeper. What's open is kept on the heap, however deep.
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
    /// What finding inline images' data's ends may cost in one reading of
    /// content ([`tokens`](crate::tokens)), in bytes, in all: 256 MiB by
    /// default. Where PDFium decodes an image's data to find where it ends
    /// (Flate and LZW), what decoding gives counts. And where that end
    /// comes to nothing, the decoder refusing the data or no `EI` coming
    /// after it, PDFium makes no stream of the data and reads on from its
    /// start, so the data, read on as content, may hold further images,
    /// each decoded again from inside it: the bytes such a decode read
    /// count too, and, for any filter, those read past an end in vain,
    /// looking for an `EI` (a place from which none was found is
    /// remembered, and not read from again). So no content can make
    /// finding its images' ends cost more than this past its own length,
    /// whether by a deflate bomb or by many images read again; content
    /// whose images' ends each have an `EI` after them spends only what
    /// decoding gives. Past it, reading stops with an error at the image's
    /// `BI` ([`ErrorKind::ImageEnds`](crate::ErrorKind::ImageEnds)), where
    /// PDFium, which has no such cap, reads on.
    pub image_ends: usize,
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
        image_ends: 256 << 20,
        content: 256 << 20,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}
