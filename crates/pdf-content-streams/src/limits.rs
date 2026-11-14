//! The caps content is read under, how far it decodes to find where
//! inline images end, what a splice's checks may read, and how far the
//! writer follows references: one value for all of them.

/// The caps on reading content, and how far its edits and its writer go.
///
/// Past most of the caps on reading, content can't be read, and reading
/// stops with an [`Error`](crate::Error) saying which and where. Past
/// [`Limits::values`], [`Limits::color`] and [`Limits::space_values`],
/// values read otherwise and no error says so, but an image sized by such
/// a count is told ([`InlineImage::capped`](crate::InlineImage::capped)).
/// Past [`Limits::content`], [`Join`](crate::Join) refuses a stream
/// ([`TooLarge`](crate::TooLarge)). Past [`Limits::splice_reads`],
/// [`splice_each`](crate::splice_each) is refused, and past
/// [`Limits::hops`] the writer binds nothing. [`Limits::coded_ends`] says
/// how an inline image's DCT and CCITTFax data's end is found.
///
/// A caller sets them on a copy of [`Limits::DEFAULT`]
/// (`limits.operands = 64`, say), as more may come.
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
    /// error here too ([`tokens`](fn@crate::tokens)), whatever this says; an
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
    /// as an array is read so, under [`Limits::space_values`] too.
    pub values: usize,
    /// An inline image's data, in bytes: 64 MiB by default.
    pub inline_image: usize,
    /// What finding inline images' data's ends may cost in one reading of
    /// content ([`tokens`](fn@crate::tokens)), in bytes, in all: 256 MiB by
    /// default. Where PDFium decodes an image's data to find where it ends
    /// (Flate and LZW), what the decoder gives counts, its predictor not
    /// undone; where DCT and CCITTFax
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
    /// for, and an image sized by one says so
    /// ([`InlineImage::capped`](crate::InlineImage::capped)), as edits
    /// refuse content that holds one
    /// ([`SpliceError::Capped`](crate::SpliceError::Capped)).
    pub color: pdf_color::Limits,
    /// How many values an inline image's color space written as an array
    /// may hold, as [`Limits::values`] counts them: 4,096 by default.
    /// They're counted before the array is read into an object, nothing
    /// kept; past it, or past [`Limits::values`], the space counts as one
    /// that doesn't load, 3 components, and the image is capped, as past a
    /// cap of [`Limits::color`]. PDFium opens no array in an array outside a
    /// dictionary, and the dictionaries such a space holds are CalGray's,
    /// CalRGB's or Lab's, so one written honestly holds a few dozen.
    pub space_values: usize,
    /// How many values one reading's marked content may hold
    /// ([`marked`](fn@crate::marked), [`MarkedNest`](crate::MarkedNest)): each
    /// range one, and each property list written in the content, kept with
    /// its range, the values reading it built, as [`Limits::values`] counts
    /// them: 2^22 by default, some 400 to 470 MB. Each list is read under
    /// what's left, so one past it stops there, never built whole first,
    /// and past it reading stops with an error at the `BMC` or `BDC`
    /// ([`ErrorKind::MarkedSize`](crate::ErrorKind::MarkedSize)), never a
    /// list dropped, as a range whose list is dropped loses what it marks
    /// (its `/MCID`, say). A list past [`Limits::values`] alone, where
    /// that's the lesser, reads as none, as an operand does.
    pub marked: usize,
    /// How many bytes one [`splice_each`](crate::splice_each)'s checks may
    /// read in all: 16 GiB by default. Each check reads the content, and,
    /// where it takes something out, what that keeps, and is counted before
    /// it's made; past it, `splice_each` is refused
    /// ([`SpliceError::Reads`](crate::SpliceError::Reads)), never a group
    /// left in unchecked. A `u64`, as it sums many readings, and holds 16
    /// GiB on a 32-bit target too. [`splice`](fn@crate::splice) and
    /// [`without_keys`](crate::without_keys) read the content two and three
    /// times a call, whatever this says: a caller calling them in a loop
    /// bounds the loop.
    pub splice_reads: u64,
    /// How many references in a row [`Reserved::read`](crate::Reserved::read)
    /// follows to find what a resources dictionary, a category of it or a
    /// name binds: 16 by default. Past it, a dictionary or a category isn't
    /// read, and a name binds nothing that can be told by what it is, so
    /// the writer keeps clear of it. These are the only references this
    /// crate follows in a row: [`Join::push_contents`](crate::Join::push_contents)
    /// follows one, as PDFium does, and a color space's are pdf-color's to
    /// follow, under [`Limits::color`].
    pub hops: usize,
}

/// How an inline image's DCT and CCITTFax data's end is found
/// ([`Limits::coded_ends`]): PDFium decodes the data to find it, every row,
/// and ends it where its decoder then stands. Flate's and LZW's ends are
/// found by decoding them always.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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
    /// ([`tokens`](fn@crate::tokens) says where each ends). Only with the
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
        space_values: 4096,
        marked: 1 << 22,
        splice_reads: 16 << 30,
        hops: 16,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}
