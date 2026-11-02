//! The caps content is read under, and how far it decodes to find where
//! inline images end: one value for all of them.

/// The caps on reading content: past one, it can't be read, and reading
/// stops with an [`Error`](crate::Error) saying which and where, but for
/// [`Limits::values`] and [`Limits::color`], whose values read otherwise,
/// and no error says so; and how an inline image's DCT and CCITTFax data's
/// end is found ([`Limits::coded_ends`]). A caller sets them on a copy of
/// [`Limits::DEFAULT`] (`limits.operands = 64`, say), as more may come.
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
    /// as pdf-objects' parser has it. An inline image's color space written
    /// as an array is read so, and past it counts as one that doesn't load,
    /// 3 components.
    pub values: usize,
    /// An inline image's data, in bytes: 64 MiB by default.
    pub inline_image: usize,
    /// What finding inline images' data's ends may cost in one reading of
    /// content ([`tokens`](crate::tokens)), in bytes, in all: 256 MiB by
    /// default. Where PDFium decodes an image's data to find where it ends
    /// (Flate and LZW), what decoding gives counts; where DCT and CCITTFax
    /// data is decoded too ([`CodedEnds`]' `Decoded`, with the
    /// `image-ends` feature), what decoding a JPEG holds, its output and,
    /// decoding several scans, its coefficients, counted from its header
    /// before it's decoded (fax data is decoded to no output); and, where
    /// the decoder can't decode the data, what it was given to read: fax
    /// data is given a window of what follows, from 4 KiB, doubled while
    /// its decoder reads to the window's end, as it says nothing of how far
    /// it read where it can't decode the data. And where that end comes to
    /// nothing, the decoder refusing the data or no `EI` coming after it,
    /// PDFium makes no stream of the data and reads on from its start, so
    /// the data, read on as content, may hold further images, each decoded
    /// again from inside it: the bytes such a decode read count too, and,
    /// for any filter, those read past an end in vain, looking for an `EI`
    /// (a place from which none was found is remembered, and not read from
    /// again). So no content can make finding its images' ends cost more
    /// than this past its own length, whether by a deflate bomb or by many
    /// images read again; content whose images' ends each have an `EI`
    /// after them spends only what decoding gives, and a window for each
    /// image's data its decoder can't decode. Past it, reading stops with an
    /// error at the image's `BI`
    /// ([`ErrorKind::ImageEnds`](crate::ErrorKind::ImageEnds)), where
    /// PDFium, which has no such cap, reads on.
    pub image_ends: usize,
    /// A page's streams joined ([`Join`](crate::Join)), in bytes, the
    /// space between each among them: 256 MiB by default. A place in them
    /// is counted in 32 bits ([`Pos`](crate::Pos)), so `Join` refuses past
    /// 4 GiB, whatever this says.
    pub content: usize,
    /// How an inline image's DCT and CCITTFax data's end is found, which
    /// PDFium finds by decoding it: by the stand-in, by default, or, with
    /// the `image-ends` feature, by decoding it as PDFium does
    /// ([`CodedEnds`]). Turning the feature on changes nothing until a
    /// reading asks for it here.
    pub coded_ends: CodedEnds,
    /// The caps a color space is counted under, in an inline image's array
    /// and by [`ResCx::read`](crate::ResCx::read): past one, a branch of it
    /// doesn't load, as [`pdf_color::Limits`] says, and that's no error, so
    /// the image is sized by that count and may end where PDFium's doesn't.
    /// [`ResCx::capped`](crate::ResCx::capped) names the spaces it was met
    /// for; of an array in an image, [`pdf_color::counted`] says.
    pub color: pdf_color::Limits,
}

/// How an inline image's DCT and CCITTFax data's end is found
/// ([`Limits::coded_ends`]): PDFium decodes the data to find it, every row,
/// and ends it where its decoder then stands. Flate's and LZW's ends are
/// found by decoding them always.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum CodedEnds {
    /// Found in the data's bytes, standing in for PDFium's decoders: a
    /// JPEG's past its first end of image (FF D9) from its start, and fax
    /// data's, or a JPEG's with none, at the first `EI` with white-space
    /// before it and white-space, a delimiter or the end after it. The
    /// default, and, without the `image-ends` feature, the only way.
    #[default]
    StandIn,
    /// Found by decoding the data as PDFium does, DCT data on zune-jpeg
    /// and CCITTFax data on hayro-ccitt, what's decoded not kept
    /// ([`tokens`](crate::tokens) says where each ends). Only with the
    /// `image-ends` feature.
    #[cfg(feature = "image-ends")]
    Decoded,
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
        coded_ends: CodedEnds::StandIn,
        color: pdf_color::Limits::DEFAULT,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}
