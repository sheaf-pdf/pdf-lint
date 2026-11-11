//! PDF's standard filters for data of any kind (ISO 32000-2, 7.4), decoded:
//! `ASCIIHexDecode`, `ASCII85Decode`, `LZWDecode` and `FlateDecode` with
//! their predictors, and `RunLengthDecode`; and `FlateDecode` encoded, with
//! PNG's predictors, for a writer. The filters for images (DCT, JPX, JBIG2,
//! CCITT fax) and `Crypt` aren't here: decrypting is a step before the
//! filters, the caller's.
//!
//! - **Every fault said, what decoded kept.** Data that breaks a rule
//!   decodes as far as a reader can take it, and [`Error`] has both what
//!   decoded and each [`Fault`], so a checker can read the data and still
//!   report what was wrong with it. Where the spec leaves a fault's reading
//!   open, it's read as qpdf reads it, but for an LZW table that fills,
//!   read on past as PDFium reads it ([`Fault::TableFull`]); where a fault
//!   stops a filter, what decoded before it is kept, which qpdf, giving the
//!   data undecoded, doesn't keep.
//! - **Or as PDFium reads it, on request.** [`decode_as`],
//!   [`decode_into_as`] and [`decode_prefix_into_as`] take a [`Reading`].
//!   Under [`Reading::Pdfium`], an LZW code past the table, and what PDFium
//!   refuses, read as PDFium reads them. So a reader can decode as PDFium
//!   does, or find where PDFium ends the data.
//! - **A limit on what comes out** of each filter, so what a small stream
//!   decodes into is the caller's to bound: a filter holds its input and
//!   its output, whose buffer grows by doubling.
//! - **Parameters, not dictionaries.** A [`Filter`] says what its
//!   `/DecodeParms` do; reading them from a dictionary is the caller's, so
//!   it works with any object model.
//! - **Either Flate backend, named at each call.** `FlateDecode` inflates
//!   and deflates through [miniz_oxide](https://crates.io/crates/miniz_oxide)
//!   (the `miniz_oxide` feature, on by default) or
//!   [zlib-rs](https://crates.io/crates/zlib-rs) (the `zlib-rs` feature),
//!   each compiled in by its feature, both if both are on. Every call that
//!   decodes or encodes names its [`Backend`], so a feature another crate
//!   turns on changes nothing a call gives.
//! - **Into the caller's buffer.** [`decode_into`] and [`encode`] append to
//!   a `Vec` the caller gives, so that streams join in one buffer, and one
//!   buffer serves many; [`decode`] gives a `Vec` of its own.
//! - **What a filter read, where its data runs on.** [`decode_prefix_into`]
//!   decodes data that may run on into other bytes, an inline image's in a
//!   content stream, by one filter, and says how many bytes of it the
//!   filter read and whether its data ended there ([`Prefix`]): Flate's and
//!   LZW's counted as zlib and PDFium count them, so a reader can find
//!   where PDFium takes such data to end.
//!
//! ```
//! use pdf_filters::{Filter, decode};
//! # #[cfg(feature = "miniz_oxide")]
//! # let backend = pdf_filters::Backend::MinizOxide;
//! # #[cfg(not(feature = "miniz_oxide"))]
//! # let backend = pdf_filters::Backend::ZlibRs;
//!
//! let data = decode(b"48 65 6C 6C 6F>", &[Filter::AsciiHex], 1 << 20, backend);
//! assert_eq!(data.unwrap(), b"Hello");
//!
//! // No `>` at the end: the data decodes, and the fault is said.
//! let error = decode(b"48 69", &[Filter::AsciiHex], 1 << 20, backend).unwrap_err();
//! assert_eq!(error.data, b"Hi");
//! assert_eq!(error.faults, [(0, pdf_filters::Fault::NoEnd)]);
//! ```

#[cfg(not(any(feature = "miniz_oxide", feature = "zlib-rs")))]
compile_error!(
    "pdf-filters needs a Flate backend: turn on its `miniz_oxide` feature (the default), its `zlib-rs`, or both"
);

mod ascii85;
mod flate;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod hex;
mod lzw;
mod predictor;
mod run_length;

use std::fmt;

use crate::flate::Room;

/// Which deflate implementation a Flate filter inflates and deflates
/// with. Each exists only with its feature, `miniz_oxide` (the default)
/// or `zlib-rs`; with both on, both do, and every call names the one it
/// takes, so that a feature another crate turns on changes no call's
/// output.
///
/// On data that decodes cleanly, the two give the same. On damaged data,
/// where each stops, how much it decoded before and the place a fault
/// says can differ. What each encodes differs, byte for byte, at a level:
/// each is zlib data any inflater reads, deflated as
/// [flate2](https://crates.io/crates/flate2) deflates on that backend
/// ([`encode`] says how).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Backend {
    /// miniz_oxide, miniz's deflate in Rust, with no `unsafe` code: with
    /// the `miniz_oxide` feature, on by default.
    #[cfg(feature = "miniz_oxide")]
    MinizOxide,
    /// zlib-rs, zlib in Rust, faster to inflate and to deflate at the
    /// default level, its `unsafe` code behind the safe API this calls:
    /// with the `zlib-rs` feature.
    #[cfg(feature = "zlib-rs")]
    ZlibRs,
}

/// How filters read data where ISO 32000-2 leaves a reading open, which
/// [`decode_as`], [`decode_into_as`] and [`decode_prefix_into_as`] take:
/// qpdf's, the default, or PDFium's. Only LZW's decoder and the predictors read
/// otherwise under them; the other filters read alike.
///
/// `#[non_exhaustive]`, so another reader's reading can join it in a minor
/// release.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Reading {
    /// qpdf's, every fault said, which [`decode`], [`decode_into`] and
    /// [`decode_prefix_into`] read by, but where it parts from qpdf on
    /// purpose: an LZW table that fills is read on past, as PDFium, pdf.js
    /// and poppler read it, where qpdf stops ([`Fault::TableFull`]); and
    /// where a fault stops a filter, what decoded before it is kept, where
    /// qpdf gives the data undecoded. An LZW code that isn't in the table
    /// stops the filter ([`Fault::Code`]).
    #[default]
    Qpdf,
    /// PDFium's, as its LZW decoder and its predictors read data as of
    /// chromium/8125 (its `FlateOrLZWDecode`), every fault said:
    /// - an LZW code past the table's next entry that isn't the first
    ///   since a clear code or the data's start is read as that entry, the
    ///   last code's bytes and their first byte again ([`Fault::AsNext`]),
    ///   and the codes after it as PDFium reads them: the entry the next
    ///   code makes holds that code, read through the table each time
    ///   it's read, as the table has it then (its low byte where it's
    ///   still past the table, and of a run longer than 4,000 bytes that
    ///   this makes, the last 4,000); and a code from 258 on right after it
    ///   is the last the filter reads;
    /// - what PDFium refuses is refused ([`Fault::Refused`]), nothing of
    ///   the filter's output given: an LZW code past the table that's the
    ///   first since a clear code or the data's start ([`Fault::Code`]),
    ///   LZW data that decodes to nothing, a TIFF or PNG predictor whose
    ///   rows would be no bits or more than 2³² − 1, and a PNG predictor on
    ///   nothing decoded, empty data among it;
    /// - where it isn't refused, a predictor is undone as under
    ///   [`Reading::Qpdf`], so its output is this crate's: a layout
    ///   PDFium's predictor undoes and this crate's doesn't, of bits a
    ///   sample other than 1, 2, 4, 8 or 16, is [`Fault::Parameters`], with
    ///   nothing decoded, though where the data ends and what was read are
    ///   PDFium's.
    Pdfium,
}

/// A standard filter for data of any kind, with what its parameters say.
///
/// Exhaustive on purpose: these are ISO 32000-2's filters for data of any
/// kind (7.4), each with every parameter it reads, so a match on every
/// filter is checked by the compiler, as a reader that names or decodes
/// them wants. A new standard filter would be a breaking release, as every
/// such reader must learn it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Filter {
    /// `ASCIIHexDecode`: hex digits, white-space between them ignored, up
    /// to `>`.
    AsciiHex,
    /// `ASCII85Decode`: base-85 groups of five characters, up to `~>`.
    Ascii85,
    /// `LZWDecode`, with its predictor.
    Lzw {
        /// `/EarlyChange`: whether codes widen one entry early, as they do
        /// unless it's 0.
        early_change: bool,
        /// What `/Predictor`, `/Colors`, `/BitsPerComponent` and
        /// `/Columns` say.
        predictor: Predictor,
    },
    /// `FlateDecode`: zlib data (RFC 1950), with its predictor.
    Flate {
        /// What `/Predictor`, `/Colors`, `/BitsPerComponent` and
        /// `/Columns` say.
        predictor: Predictor,
    },
    /// `RunLengthDecode`: runs of bytes copied or repeated, up to a length
    /// of 128.
    RunLength,
}

/// What an LZW or Flate filter's `/Predictor` says was done to the data
/// before it was compressed, undone after it's decompressed (ISO 32000-2,
/// 7.4.4.4).
///
/// Exhaustive on purpose: `/Predictor` is read one of these three ways, so
/// a match on every one is checked by the compiler; another would be a
/// breaking release.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Predictor {
    /// `/Predictor 1`, the default: nothing.
    #[default]
    None,
    /// `/Predictor 2`: TIFF's, each sample less the one before it in its
    /// row.
    Tiff(Layout),
    /// `/Predictor 10` to `15`: PNG's, each row with a first byte saying
    /// which of PNG's filters it's predicted by, whichever of 10 to 15 it
    /// is.
    Png(Layout),
}

impl Predictor {
    /// Whether PDFium refuses data its filter decoded to `decoded` bytes,
    /// judged before any of it is undone (its `ApplyPredictor`). TIFF's and
    /// PNG's are refused where a row would be no bits, or more than
    /// 2³² − 1. PNG's is refused on nothing decoded.
    ///
    /// Under [`Reading::Pdfium`], decoding with the predictor says this as
    /// [`Fault::Refused`]. This tells it without undoing the predictor, for
    /// a reader that wants only where the data ends, which a predictor
    /// doesn't change.
    ///
    /// ```
    /// use pdf_filters::{Layout, Predictor};
    ///
    /// let png = Predictor::Png(Layout::default());
    /// assert!(png.pdfium_refuses(0));
    /// assert!(!png.pdfium_refuses(1));
    ///
    /// // 16 bits a sample, 2^28 a row: 2^32 bits, which PDFium can't count.
    /// let wide = Layout { colors: 1, bits_per_component: 16, columns: 1 << 28 };
    /// assert!(Predictor::Tiff(wide).pdfium_refuses(1));
    /// ```
    #[must_use]
    pub fn pdfium_refuses(self, decoded: usize) -> bool {
        predictor::refuses(self, decoded == 0)
    }
}

/// How the data a predictor works on is laid out in rows.
///
/// Exhaustive on purpose: these are the three parameters a predictor reads
/// its rows by, each field public, so a layout is built by a literal and
/// taken apart by its fields; a field more would be a breaking release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Layout {
    /// `/Colors`: samples a pixel, 1 or more.
    pub colors: u32,
    /// `/BitsPerComponent`: bits a sample, 1, 2, 4, 8 or 16.
    pub bits_per_component: u32,
    /// `/Columns`: pixels a row, 1 or more.
    pub columns: u32,
}

impl Default for Layout {
    /// The parameters' defaults: one 8-bit sample, one pixel a row.
    fn default() -> Layout {
        Layout {
            colors: 1,
            bits_per_component: 8,
            columns: 1,
        }
    }
}

/// What's wrong with a filter's data, or its parameters. Each says
/// whether the filter stopped there: when it didn't, what it gave is the
/// whole of what a reader reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Fault {
    /// A byte the filter doesn't take, at `at` in its input: in
    /// `ASCIIHexDecode` one that's neither a hex digit, white-space nor
    /// `>`, and in `ASCII85Decode` one that's neither `!` to `u`, `z`,
    /// white-space nor `~`. It stopped there.
    #[error("byte {byte:#04x} at {at} isn't one it takes")]
    Byte {
        /// Where, in the filter's input.
        at: usize,
        /// The byte.
        byte: u8,
    },
    /// The data ends without its end-of-data marker: `>`, `~>`, an LZW
    /// code of 257, or a run-length of 128. It didn't stop: everything was
    /// decoded.
    #[error("no end-of-data marker")]
    NoEnd,
    /// `~` at `at`, in `ASCII85Decode`, not followed by `>`. It stopped
    /// there: followed by something else, without what was decoded of the
    /// group it's in; at the data's end, with it.
    #[error("`~` at {at} isn't followed by `>`")]
    BrokenEnd {
        /// Where, in the filter's input.
        at: usize,
    },
    /// `z` at `at`, in `ASCII85Decode`, inside a group. It stopped there.
    #[error("`z` at {at} is inside a group")]
    InGroup {
        /// Where, in the filter's input.
        at: usize,
    },
    /// A group at `at`, in `ASCII85Decode`, whose value is over 2³² − 1.
    /// It didn't stop: the group is read as its value's low 32 bits.
    #[error("the group at {at} is over 2^32 - 1")]
    Overflow {
        /// Where the group begins, in the filter's input.
        at: usize,
    },
    /// A last group at `at`, in `ASCII85Decode`, of one character, which
    /// is no bytes. It didn't stop.
    #[error("the last group, at {at}, is one character")]
    LoneByte {
        /// Where, in the filter's input.
        at: usize,
    },
    /// An LZW code at byte `at` that isn't in the table yet. It stopped
    /// there. Under [`Reading::Pdfium`], only one that's the first since a
    /// clear code or the data's start, and the data is then refused
    /// ([`Fault::Refused`]); a later one is read on past
    /// ([`Fault::AsNext`]).
    #[error("the code at byte {at} isn't in the table")]
    Code {
        /// The byte the code begins in, in the filter's input.
        at: usize,
    },
    /// The LZW table full, 4,096 entries, at byte `at`, with no clear code
    /// to empty it. It didn't stop: the codes after are read against the
    /// table as it is, 12 bits wide, until a clear code empties it, as
    /// PDFium and pdf.js read them (qpdf stops).
    #[error("the table is full at byte {at}")]
    TableFull {
        /// The byte the code that would add to it begins in, in the
        /// filter's input.
        at: usize,
    },
    /// The data doesn't begin with a zlib header for deflate data without a
    /// preset dictionary. It stopped there, with nothing decoded.
    #[error("no zlib header")]
    Header,
    /// Deflate data that isn't, at byte `at`. It stopped there.
    #[error("the deflate data is corrupt at byte {at}")]
    Corrupt {
        /// Where, in the filter's input, as near as can be told: the last
        /// byte the decompressor took.
        at: usize,
    },
    /// The data ends inside what it's decoding: a deflate stream before
    /// its end, a run-length's run. It stopped there, with what was
    /// decoded before.
    #[error("the data ends early")]
    Truncated,
    /// The zlib data's Adler-32 checksum isn't its decompressed data's. It
    /// didn't stop: everything was decoded, and readers read it so.
    #[error("the Adler-32 checksum doesn't match")]
    Checksum,
    /// A PNG-predicted row, the `row`th from 0, whose first byte, `tag`,
    /// isn't 0 to 4. It didn't stop: the row is read as predicted by
    /// nothing.
    #[error("row {row}'s first byte is {tag}, not 0 to 4")]
    RowTag {
        /// Which row, from 0.
        row: usize,
        /// Its first byte.
        tag: u8,
    },
    /// The predicted data's last row short by `missing` bytes, when it
    /// ends there, or ends early. It didn't stop: the row is read with
    /// zeros for them. A row a fault cut off before is left out.
    #[error("the last row is {missing} bytes short")]
    PartialRow {
        /// How many bytes the row lacks.
        missing: usize,
    },
    /// A predictor's [`Layout`] that isn't one: no colors or columns, or
    /// bits a sample other than 1, 2, 4, 8 or 16. Nothing was decoded.
    #[error("its predictor's parameters aren't ones it takes")]
    Parameters,
    /// The filter's output over the limit the call was given: before a
    /// PNG predictor, that limit and a tag a row ([`decode`] says so). It
    /// stopped there, with as much as the limit allows: behind a predictor,
    /// the whole rows that fit under it, none where a row is over it. The
    /// crate has no cap of its own, so the limit this says is always the
    /// caller's.
    #[error("its output is over the limit")]
    Limit,
    /// An LZW code at byte `at` past the table's next entry, read as that
    /// entry, the last code's bytes and their first byte again, as PDFium's
    /// decoder reads it, under [`Reading::Pdfium`] alone ([`Reading`] says
    /// how the codes after it are read). It didn't stop, but where the
    /// next code is from 258 on, that's the last read.
    #[error("the code at byte {at} is past the table's next entry, read as it")]
    AsNext {
        /// The byte the code begins in, in the filter's input.
        at: usize,
    },
    /// What the filter decoded is refused, as PDFium refuses it, under
    /// [`Reading::Pdfium`] alone ([`Reading`] says what it refuses), after
    /// the fault that says why where there's one: [`Fault::Code`] for an
    /// LZW code past the table that's the first since a clear code or the
    /// data's start; [`Fault::Parameters`] for a predictor whose rows would
    /// be no bits. Nothing of the filter's output is given: what it decoded
    /// is cut away, the next filter given nothing. What it read is counted
    /// all the same ([`Prefix`]).
    #[error("it's refused, as PDFium refuses it")]
    Refused,
}

/// Data that didn't decode cleanly: what decoded, and why it's not clean.
///
/// Its `Debug` gives the length of what decoded, not its bytes, which may
/// be a whole stream's, decrypted before it was decoded:
/// `Error { data: 23 bytes, faults: [(0, Checksum)] }`. So two errors whose
/// data are the same length have the same `Debug`, whatever their bytes: a
/// test compares errors by `==`, never by their `Debug` text, and shows
/// `data` itself where it needs the bytes.
///
/// Exhaustive on purpose: what decoded and its faults, each field public,
/// so an error is built by a literal and taken apart by its fields; a
/// field more would be a breaking release.
#[derive(Clone, PartialEq, Eq, thiserror::Error)]
pub struct Error {
    /// What decoded, through every filter: each given what the one before
    /// it gave, however far that went.
    pub data: Vec<u8>,
    /// Each fault, with the index of its filter in the list, in the list's
    /// order: at least one, and the first of each kind for a filter.
    pub faults: Vec<(usize, Fault)>,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        said(&self.faults, f)
    }
}

impl fmt::Debug for Error {
    /// What decoded by its length, not its bytes, and each fault.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error")
            .field("data", &format_args!("{} bytes", self.data.len()))
            .field("faults", &self.faults)
            .finish()
    }
}

/// Why data decoded into a caller's buffer didn't decode cleanly: each
/// fault, with the index of its filter in the list, in the list's order,
/// as an [`Error`] has them; what decoded is in the buffer.
///
/// Exhaustive on purpose: the faults alone, its field public, as an
/// [`Error`]'s are.
#[derive(Clone, Debug, PartialEq, Eq, Hash, thiserror::Error)]
pub struct Faults(pub Vec<(usize, Fault)>);

impl fmt::Display for Faults {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        said(&self.0, f)
    }
}

/// Each fault after its filter's index, `; ` between them.
fn said(faults: &[(usize, Fault)], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    for (i, (filter, fault)) in faults.iter().enumerate() {
        if i > 0 {
            write!(f, "; ")?;
        }
        write!(f, "filter {filter}: {fault}")?;
    }
    Ok(())
}

/// `data` decoded by `filters` in their order, as a stream's `/Filter`
/// lists them, no filter's output over `limit` bytes; an LZW or Flate
/// filter's before its predictor is allowed a PNG predictor's tag a row
/// besides; Flate inflated by `backend`. With no filters, it's `data`
/// whole, past the limit or not, as the limit bounds what filters give.
/// The limit bounds what's allocated, so it should be one that can be: a
/// short last row is read with zeros up to it. It's [`decode_into`] an
/// empty `Vec`, which is then cut to its length.
///
/// # Errors
///
/// An [`Error`] when the data doesn't decode cleanly: what decoded,
/// through every filter, however far that went, and each [`Fault`] with
/// its filter's index, a filter's output over the limit among them
/// ([`Fault::Limit`]).
pub fn decode(
    data: &[u8],
    filters: &[Filter],
    limit: usize,
    backend: Backend,
) -> Result<Vec<u8>, Error> {
    decode_as(data, filters, limit, backend, Reading::Qpdf)
}

/// `data` decoded as [`decode`] decodes it, read as `reading` reads it.
/// It's [`decode_into_as`] an empty `Vec`, which is then cut to its length.
///
/// ```
/// use pdf_filters::{Fault, Filter, Predictor, Reading, decode_as};
/// # #[cfg(feature = "miniz_oxide")]
/// # let backend = pdf_filters::Backend::MinizOxide;
/// # #[cfg(not(feature = "miniz_oxide"))]
/// # let backend = pdf_filters::Backend::ZlibRs;
///
/// // LZW codes 256 65 66 300 257: 300 is past the table, read as its next
/// // entry under PDFium's reading, where qpdf's stops.
/// let data = [0x80, 0x10, 0x48, 0x52, 0xc8, 0x08];
/// let lzw = Filter::Lzw { early_change: true, predictor: Predictor::None };
/// let pdfium = decode_as(&data, &[lzw], 1 << 20, backend, Reading::Pdfium).unwrap_err();
/// assert_eq!(pdfium.data, b"ABBB");
/// assert_eq!(pdfium.faults, [(0, Fault::AsNext { at: 3 })]);
/// let qpdf = decode_as(&data, &[lzw], 1 << 20, backend, Reading::Qpdf).unwrap_err();
/// assert_eq!(qpdf.data, b"AB");
/// ```
///
/// # Errors
///
/// An [`Error`] when the data doesn't decode cleanly, as [`decode`] gives
/// it, and, under [`Reading::Pdfium`], where PDFium reads it otherwise
/// ([`Fault::AsNext`]) or refuses it ([`Fault::Refused`]).
pub fn decode_as(
    data: &[u8],
    filters: &[Filter],
    limit: usize,
    backend: Backend,
    reading: Reading,
) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let decoded = decode_into_as(data, filters, limit, backend, reading, &mut out);
    out.shrink_to_fit();
    match decoded {
        Ok(()) => Ok(out),
        Err(Faults(faults)) => Err(Error { data: out, faults }),
    }
}

/// `data` decoded as [`decode`] decodes it, appended to `out`: what `out`
/// held is kept, before it, and `limit` bounds what's added, the last
/// filter's output, not what was there; with no filters, `data` is added
/// whole, as [`decode`] gives it. With faults, `out` has what decoded, as
/// an [`Error`]'s `data` does. The last filter decodes straight into
/// `out`, grown with zeros as it fills and cut to what was written; those
/// before it each into a buffer of its own.
///
/// # Errors
///
/// [`Faults`] when the data doesn't decode cleanly: each fault with its
/// filter's index, as an [`Error`] has them, `out` having what decoded.
pub fn decode_into(
    data: &[u8],
    filters: &[Filter],
    limit: usize,
    backend: Backend,
    out: &mut Vec<u8>,
) -> Result<(), Faults> {
    decode_into_as(data, filters, limit, backend, Reading::Qpdf, out)
}

/// `data` decoded as [`decode_into`] decodes it, appended to `out`, read
/// as `reading` reads it: [`decode_into`] is this under [`Reading::Qpdf`].
/// Under [`Reading::Pdfium`], a filter whose output PDFium refuses gives
/// none, and the next is given nothing, which, LZW's or behind a PNG
/// predictor, it refuses too; nothing decodes to nothing but where PDFium
/// refuses it.
///
/// ```
/// use pdf_filters::{Fault, Faults, Filter, Predictor, Reading, decode_into_as};
/// # #[cfg(feature = "miniz_oxide")]
/// # let backend = pdf_filters::Backend::MinizOxide;
/// # #[cfg(not(feature = "miniz_oxide"))]
/// # let backend = pdf_filters::Backend::ZlibRs;
///
/// // LZW codes 256 65 66 300 257, nine bits each: 300 is past the table,
/// // whose next entry is 259, and read as it, as PDFium reads it.
/// let data = [0x80, 0x10, 0x48, 0x52, 0xc8, 0x08];
/// let lzw = Filter::Lzw { early_change: true, predictor: Predictor::None };
/// let mut out = Vec::new();
/// let decoded = decode_into_as(&data, &[lzw], 1 << 20, backend, Reading::Pdfium, &mut out);
/// assert_eq!(out, b"ABBB");
/// assert_eq!(decoded, Err(Faults(vec![(0, Fault::AsNext { at: 3 })])));
///
/// // qpdf's reading stops there, what decoded before it kept.
/// let mut out = Vec::new();
/// let decoded = decode_into_as(&data, &[lzw], 1 << 20, backend, Reading::Qpdf, &mut out);
/// assert_eq!(out, b"AB");
/// assert_eq!(decoded, Err(Faults(vec![(0, Fault::Code { at: 3 })])));
/// ```
///
/// # Errors
///
/// [`Faults`] when the data doesn't decode cleanly, as [`decode_into`]
/// gives them, and, under [`Reading::Pdfium`], where PDFium reads it
/// otherwise ([`Fault::AsNext`]) or refuses it ([`Fault::Refused`]).
pub fn decode_into_as(
    data: &[u8],
    filters: &[Filter],
    limit: usize,
    backend: Backend,
    reading: Reading,
    out: &mut Vec<u8>,
) -> Result<(), Faults> {
    let Some(last) = filters.len().checked_sub(1) else {
        out.extend_from_slice(data);
        return Ok(());
    };
    let mut faults = Vec::new();
    let mut decoded = None;
    for (i, filter) in filters.iter().enumerate() {
        let input = decoded.as_deref().unwrap_or(data);
        let into = match i == last {
            true => std::mem::take(out),
            false => Vec::new(),
        };
        let mut into = Out::new(into, predictor::before(limit, filter));
        // Nothing decodes to nothing, as in qpdf, whatever the filter, but
        // where PDFium refuses it.
        if !input.is_empty() {
            one(
                input,
                filter,
                limit,
                backend,
                reading,
                Room::Given,
                &mut into,
            );
        } else if reading == Reading::Pdfium {
            refused(filter, 0, &mut into);
        }
        faults.extend(into.faults.into_iter().map(|fault| (i, fault)));
        match i == last {
            true => *out = into.data,
            false => decoded = Some(into.data),
        }
    }
    match faults.is_empty() {
        true => Ok(()),
        false => Err(Faults(faults)),
    }
}

/// What a filter read of data that may run on into other bytes, as
/// [`decode_prefix_into`] gives it: how many bytes from the data's start,
/// and whether its data ended within them. A reader of such data, an
/// inline image's in a content stream (which ISO 32000-2, 8.9.7, has end
/// before its `EI`), can take it to end where its filter stopped reading,
/// as PDFium does where it decodes Flate and LZW data to find that end.
///
/// A filter reads what it takes to decode, and to judge the byte it stops
/// at: up to its end-of-data marker; up to the byte a fault stopped it at;
/// up to the byte whose output was over the limit; or, with no marker, to
/// the data's end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Prefix {
    /// How many bytes of the data the filter read, from its start:
    /// - `ASCIIHexDecode`'s, `ASCII85Decode`'s and `RunLengthDecode`'s, to
    ///   the last byte this crate's decoder took: its `>`, the `>` of its
    ///   `~>`, its run-length of 128, or the byte it stopped at (after a
    ///   `~` that isn't followed by `>`, the byte that follows it). PDFium
    ///   reads these filters' data otherwise.
    /// - `LZWDecode`'s, to the last bit of the last code it read, rounded
    ///   up to a byte, never the bytes read ahead of it, as PDFium's LZW
    ///   decoder counts what it consumed: to its code of 257, to the code
    ///   it stopped at, or to the last whole code before the data's end,
    ///   the bits after it too few for one. At a code that isn't in the
    ///   table, which stops it under [`Reading::Qpdf`] ([`Fault::Code`]),
    ///   PDFium's reads on, as if it were the table's next entry, so it
    ///   counts more; under [`Reading::Pdfium`] this does too, to the code
    ///   PDFium's decoder reads last.
    /// - `FlateDecode`'s, the zlib data's bytes the backend took, its
    ///   checksum among them, as zlib's `total_in` counts them: each byte
    ///   any of whose bits it used, those it read ahead and didn't use
    ///   given back. On damaged data the backends can part, where each
    ///   finds the damage: miniz_oxide refuses at once a dynamic block
    ///   whose code-length code has no lengths, which zlib and zlib-rs read
    ///   on into. A header that isn't one for deflate data without a preset
    ///   dictionary isn't given to the backend, and is counted as zlib
    ///   counts it: its two bytes (or as many as there are), but none where
    ///   it asks for a preset dictionary and four bytes after it name one,
    ///   as zlib then stops for the dictionary without counting them.
    pub read: usize,
    /// Whether the filter's data ended within those bytes, at its
    /// end-of-data marker: `>`; `~>`; an LZW code of 257; a run-length of
    /// 128; for Flate, the zlib data's last deflate block and the checksum
    /// after it, matching its data or not. When it did, bytes after them
    /// change nothing the filter gives; when it didn't, a fault or the
    /// limit stopped it, or the data ended without its marker.
    pub ended: bool,
    /// How many bytes the filter's decoder gave, under the limit, before
    /// its predictor: what decoding gave, whatever the predictor then made
    /// of it, and where it's refused ([`Fault::Refused`]) and cut away, so
    /// a caller can count what decoding cost by it. Without a predictor,
    /// and not refused, it's what the filter added.
    pub decoded: usize,
}

impl Prefix {
    /// Read to `read`, its data `ended` there or not; what its decoder gave
    /// is counted once it's decoded ([`one`]).
    fn new(read: usize, ended: bool) -> Prefix {
        Prefix {
            read,
            ended,
            decoded: 0,
        }
    }

    /// Read to `read`, its data ended there.
    fn ended(read: usize) -> Prefix {
        Prefix::new(read, true)
    }

    /// Read to `read`, its data not ended: stopped there, or at the data's
    /// end.
    fn unended(read: usize) -> Prefix {
        Prefix::new(read, false)
    }
}

/// `data` decoded by one `filter`, appended to `out` as [`decode_into`]
/// appends it decoded by a list of that filter alone, the same bytes and
/// the same faults, for data that may run on into other bytes, as an
/// inline image's does in a content stream: and the [`Prefix`] of `data`
/// the filter read, and whether its data ended there. Bytes after those
/// it read change nothing it gives, and cost nothing: what decoding costs,
/// in time and memory, follows what the filter reads and gives, not
/// `data`'s length, so `data` may be all that follows, however long.
/// Nothing is read of nothing, which decodes to nothing.
///
/// ```
/// use pdf_filters::{Filter, decode_prefix_into};
/// # #[cfg(feature = "miniz_oxide")]
/// # let backend = pdf_filters::Backend::MinizOxide;
/// # #[cfg(not(feature = "miniz_oxide"))]
/// # let backend = pdf_filters::Backend::ZlibRs;
///
/// // Hex digits, their `>`, and what follows: an inline image's data, then
/// // the content stream's `EI`.
/// let mut out = Vec::new();
/// let (prefix, decoded) =
///     decode_prefix_into(b"48 69> EI Q", Filter::AsciiHex, 1 << 20, backend, &mut out);
/// assert_eq!((out, decoded), (b"Hi".to_vec(), Ok(())));
/// assert_eq!((prefix.read, prefix.ended), (6, true));
/// ```
///
/// # Errors
///
/// The second of what it gives is [`Faults`] when the data doesn't decode
/// cleanly, each fault the one filter's, at index 0, `out` having what
/// decoded; the [`Prefix`] is given either way.
pub fn decode_prefix_into(
    data: &[u8],
    filter: Filter,
    limit: usize,
    backend: Backend,
    out: &mut Vec<u8>,
) -> (Prefix, Result<(), Faults>) {
    decode_prefix_into_as(data, filter, limit, backend, Reading::Qpdf, out)
}

/// `data` decoded by one `filter` as [`decode_prefix_into`] decodes it,
/// read as `reading` reads it, and the [`Prefix`] of it the filter read:
/// [`decode_prefix_into`] is this under [`Reading::Qpdf`]. Under
/// [`Reading::Pdfium`], what it read is where PDFium's decoder takes the
/// data to end, and a refusal is said ([`Fault::Refused`]), what decoded
/// cut away but counted ([`Prefix::decoded`]), as a reader that finds
/// where PDFium ends an inline image's data wants.
///
/// ```
/// use pdf_filters::{Fault, Filter, Predictor, Reading, decode_prefix_into_as};
/// # #[cfg(feature = "miniz_oxide")]
/// # let backend = pdf_filters::Backend::MinizOxide;
/// # #[cfg(not(feature = "miniz_oxide"))]
/// # let backend = pdf_filters::Backend::ZlibRs;
///
/// // LZW codes 256 300 65 257, then an inline image's `EI`: 300 is the
/// // first code since the clear code, and the table has no entry yet, so
/// // PDFium refuses the data.
/// let data = b"\x80\x4b\x08\x30\x10 EI";
/// let lzw = Filter::Lzw { early_change: true, predictor: Predictor::None };
/// let mut out = Vec::new();
/// let (prefix, decoded) =
///     decode_prefix_into_as(data, lzw, 1 << 20, backend, Reading::Pdfium, &mut out);
/// assert_eq!((prefix.read, prefix.ended, prefix.decoded), (3, false, 0));
/// let faults = decoded.unwrap_err().0;
/// assert_eq!(faults, [(0, Fault::Code { at: 1 }), (0, Fault::Refused)]);
/// assert!(out.is_empty());
/// ```
///
/// # Errors
///
/// The second of what it gives is [`Faults`] when the data doesn't decode
/// cleanly, as [`decode_prefix_into`] gives them, and, under
/// [`Reading::Pdfium`], where PDFium reads it otherwise ([`Fault::AsNext`])
/// or refuses it ([`Fault::Refused`]); the [`Prefix`] is given either way.
pub fn decode_prefix_into_as(
    data: &[u8],
    filter: Filter,
    limit: usize,
    backend: Backend,
    reading: Reading,
    out: &mut Vec<u8>,
) -> (Prefix, Result<(), Faults>) {
    let mut into = Out::new(std::mem::take(out), predictor::before(limit, &filter));
    // Nothing decodes to nothing, as `decode_into_as` has it.
    let prefix = match data.is_empty() {
        true => {
            if reading == Reading::Pdfium {
                refused(&filter, 0, &mut into);
            }
            Prefix::unended(0)
        }
        false => one(
            data,
            &filter,
            limit,
            backend,
            reading,
            Room::RunsOn,
            &mut into,
        ),
    };
    *out = into.data;
    let decoded = match into.faults.is_empty() {
        true => Ok(()),
        false => Err(Faults(into.faults.into_iter().map(|f| (0, f)).collect())),
    };
    (prefix, decoded)
}

/// `input` decoded by `filter` into `out`, read as `reading` reads it, its
/// predictor undone, the predictor's output no more than `limit` bytes,
/// Flate's output first given `room`: what of `input` the filter read, and
/// how much its decoder gave.
fn one(
    input: &[u8],
    filter: &Filter,
    limit: usize,
    backend: Backend,
    reading: Reading,
    room: Room,
    out: &mut Out,
) -> Prefix {
    let (mut prefix, predictor) = match *filter {
        Filter::AsciiHex => (hex::decode(input, out), Predictor::None),
        Filter::Ascii85 => (ascii85::decode(input, out), Predictor::None),
        Filter::Lzw {
            early_change,
            predictor,
        } => (lzw::decode(input, early_change, reading, out), predictor),
        Filter::Flate { predictor } => (flate::decode(input, backend, room, out), predictor),
        Filter::RunLength => (run_length::decode(input, out), Predictor::None),
    };
    prefix.decoded = out.data.len() - out.start;
    if reading == Reading::Pdfium && refused(filter, prefix.decoded, out) {
        return prefix;
    }
    if predictor != Predictor::None {
        // Data a fault stopped mid-row ends in a row cut off, not short:
        // it isn't read.
        let cut = (out.faults.iter())
            .any(|f| matches!(f, Fault::Limit | Fault::Corrupt { .. } | Fault::Code { .. }));
        out.limit = limit;
        predictor::undo(predictor, cut, out);
    }
    prefix
}

/// Whether PDFium refuses what `filter`'s decoder gave into `out`,
/// `decoded` bytes (its `FlateOrLZWDecode`, and its predictors'
/// `ApplyPredictor`, judged on the decoder's output, before any is undone):
/// LZW data whose first code since a clear code or the data's start is
/// past the table ([`Fault::Code`]), or that decodes to nothing; a
/// predictor's, as [`predictor::pdfium_refuses`] says, after
/// [`Fault::Parameters`] where this crate's predictor doesn't take its
/// layout either. A decoder the limit stopped decoded something. Where
/// PDFium refuses it, said, and `out` cut back to what it held.
fn refused(filter: &Filter, decoded: usize, out: &mut Out) -> bool {
    let nothing = decoded == 0 && !out.faults.contains(&Fault::Limit);
    let (lzw, predictor) = match *filter {
        Filter::Lzw { predictor, .. } => {
            let first = (out.faults.iter()).any(|f| matches!(f, Fault::Code { .. }));
            (first || nothing, predictor)
        }
        Filter::Flate { predictor } => (false, predictor),
        _ => (false, Predictor::None),
    };
    if !lzw && !predictor::pdfium_refuses(predictor, nothing, out) {
        return false;
    }
    out.data.truncate(out.start);
    out.fault(Fault::Refused);
    true
}

/// What a filter does to data as a writer encodes it: the filter, and
/// what the writer chooses that decoding doesn't need to know.
///
/// `#[non_exhaustive]`, as other filters' encoders may join it. A
/// variant's fields are each public, so an encoder is built by a literal,
/// and a field more (a deflater's strategy, say) would be a breaking
/// release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Encoder {
    /// `FlateDecode`'s zlib data (RFC 1950), its data first predicted as
    /// `predictor` says.
    Flate {
        /// How hard the deflater works to make the data small.
        level: Level,
        /// What's done to the data before it's compressed.
        predictor: Prediction,
    },
}

impl Encoder {
    /// The filter that decodes what this encodes: what a writer's
    /// `/Filter` and `/DecodeParms` say, but for which of PNG's predictors
    /// `/Predictor` names, 10 to 15, any of which decodes alike, so the
    /// filter doesn't hold it: that's [`Prediction::predictor`].
    #[must_use]
    pub const fn filter(&self) -> Filter {
        match *self {
            Encoder::Flate { predictor, .. } => Filter::Flate {
                predictor: match predictor {
                    Prediction::None => Predictor::None,
                    Prediction::Png(layout, _) => Predictor::Png(layout),
                },
            },
        }
    }
}

/// How hard a Flate encoder's deflater works to make data small, from 0,
/// the data stored as it is, to 9, the smallest it makes and the slowest,
/// as zlib numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Level(u8);

impl Level {
    /// 0: the data stored as it is, in blocks of its own, not compressed.
    pub const NONE: Level = Level(0);
    /// 6, zlib's default: nearly as small as 9, in much less time.
    pub const DEFAULT: Level = Level(6);
    /// 9: the smallest, the slowest.
    pub const BEST: Level = Level(9);

    /// Level `level`, unless it's over 9.
    #[must_use]
    pub const fn new(level: u8) -> Option<Level> {
        match level {
            0..=9 => Some(Level(level)),
            _ => None,
        }
    }

    /// Its number, 0 to 9.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl Default for Level {
    /// [`Level::DEFAULT`], 6.
    fn default() -> Level {
        Level::DEFAULT
    }
}

/// What an encoder does to data before it's compressed, which its
/// filter's [`Predictor`] undoes (ISO 32000-2, 7.4.4.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Prediction {
    /// Nothing, `/Predictor 1`.
    #[default]
    None,
    /// PNG's, `/Predictor 10` to `15`: the data in rows as the [`Layout`]
    /// has them, each predicted by the [`Row`] filter, its first byte the
    /// filter's tag.
    Png(Layout, Row),
}

impl Prediction {
    /// The `/Predictor` a writer writes for it: 1 for none, and for PNG's
    /// 10 plus its row filter's tag, so 12 for Up, as cross-reference
    /// streams are often written.
    #[must_use]
    pub const fn predictor(self) -> u8 {
        match self {
            Prediction::None => 1,
            Prediction::Png(_, row) => 10 + row.tag(),
        }
    }
}

/// Which of PNG's filters predicts each row from the bytes before it in
/// the row and the row above it, read as zeros for the first row.
///
/// `#[non_exhaustive]`, though these are all five of PNG's filters:
/// making it exhaustive later is a minor release, where the other way
/// round would be a breaking one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Row {
    /// Tag 0: nothing, the row as it is.
    None,
    /// Tag 1, Sub: each byte less the one a pixel before it.
    Sub,
    /// Tag 2, Up: each byte less the one above it, as cross-reference
    /// streams' rows are often written (`/Predictor 12`).
    Up,
    /// Tag 3, Average: each byte less the mean of the one a pixel before
    /// it and the one above it, rounded down.
    Average,
    /// Tag 4, Paeth: each byte less whichever of the bytes a pixel before
    /// it, above it, and above that is nearest the first plus the second
    /// less the third.
    Paeth,
}

impl Row {
    /// Its tag, the first byte of each row it predicts: 0 to 4.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Row::None => 0,
            Row::Sub => 1,
            Row::Up => 2,
            Row::Average => 3,
            Row::Paeth => 4,
        }
    }
}

/// Why data can't be encoded as its encoders say. Nothing was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Unencodable {
    /// A predictor's [`Layout`] that isn't one: no colors or columns, or
    /// bits a sample other than 1, 2, 4, 8 or 16.
    #[error("its predictor's parameters aren't ones it takes")]
    Parameters,
    /// Data a predictor takes that isn't whole rows: the last short by
    /// `missing` bytes.
    #[error("the last row is {missing} bytes short")]
    PartialRow {
        /// How many bytes the last row lacks.
        missing: usize,
    },
}

/// `data` encoded by `encoders`, listed as a stream's `/Filter` lists
/// them, so that the last is applied first and decoding by their filters
/// ([`Encoder::filter`]) in their order gives `data`; Flate deflated by
/// `backend`; appended to `out`. With no encoders, `data` itself. The
/// first encoder writes straight into `out`, grown with zeros as it fills
/// and cut to what was written. When it can't be, `out` is as it was.
///
/// Flate deflates with a window of 32 KiB, zlib's memory level 8 and its
/// default strategy, zlib-rs given 32 KiB to write into at a call, as what
/// it writes depends on where that room runs out: as
/// [flate2](https://crates.io/crates/flate2)'s `ZlibEncoder` deflates on
/// the same backend. With flate2 1.1 on the same versions of miniz_oxide
/// and zlib-rs, the bytes were found the same at levels 6 and 9; that's
/// measured, not promised, as a release of either can change them.
///
/// # Errors
///
/// [`Unencodable`] when an encoder's PNG predictor has a [`Layout`] that
/// isn't one, or the data it's given isn't whole rows of it, whichever
/// encoder in the list that is; `out` is then as it was.
pub fn encode(
    data: &[u8],
    encoders: &[Encoder],
    backend: Backend,
    out: &mut Vec<u8>,
) -> Result<(), Unencodable> {
    if encoders.is_empty() {
        out.extend_from_slice(data);
        return Ok(());
    }
    let mut encoded: Option<Vec<u8>> = None;
    for (i, encoder) in encoders.iter().enumerate().rev() {
        let input = encoded.as_deref().unwrap_or(data);
        match i {
            0 => encode_one(input, encoder, backend, out)?,
            _ => {
                let mut next = Vec::new();
                encode_one(input, encoder, backend, &mut next)?;
                encoded = Some(next);
            }
        }
    }
    Ok(())
}

/// `data` encoded by `encoder`, appended to `out`, which is left as it was
/// when it can't be.
fn encode_one(
    data: &[u8],
    encoder: &Encoder,
    backend: Backend,
    out: &mut Vec<u8>,
) -> Result<(), Unencodable> {
    match *encoder {
        Encoder::Flate { level, predictor } => match predictor {
            Prediction::None => flate::encode(data, level, backend, out),
            Prediction::Png(layout, row) => {
                let predicted = predictor::png(data, layout, row)?;
                flate::encode(&predicted, level, backend, out);
            }
        },
    }
    Ok(())
}

/// A filter's output, kept to the limit, and its faults: what it decodes
/// is added to `data` past `start`, what was there before being the
/// caller's.
struct Out {
    data: Vec<u8>,
    start: usize,
    limit: usize,
    faults: Vec<Fault>,
}

impl Out {
    fn new(data: Vec<u8>, limit: usize) -> Out {
        Out {
            start: data.len(),
            data,
            limit,
            faults: Vec::new(),
        }
    }

    /// Notes `fault`, unless one of its kind is noted already.
    fn fault(&mut self, fault: Fault) {
        let kind = std::mem::discriminant(&fault);
        if !self
            .faults
            .iter()
            .any(|f| std::mem::discriminant(f) == kind)
        {
            self.faults.push(fault);
        }
    }

    /// How many of `n` more bytes the limit allows: all of them, or what
    /// room there is, with the limit noted as a fault.
    fn take(&mut self, n: usize) -> usize {
        let room = self.limit - (self.data.len() - self.start);
        if n > room {
            self.fault(Fault::Limit);
        }
        n.min(room)
    }

    /// `bytes` added, or as many as the limit allows and `Err`.
    fn extend(&mut self, bytes: &[u8]) -> Result<(), ()> {
        let take = self.take(bytes.len());
        self.data.extend_from_slice(&bytes[..take]);
        whole(take, bytes.len())
    }

    /// `byte` added `n` times, or as many as the limit allows and `Err`.
    fn repeat(&mut self, byte: u8, n: usize) -> Result<(), ()> {
        let take = self.take(n);
        self.data.resize(self.data.len() + take, byte);
        whole(take, n)
    }
}

/// `Ok` when all `n` bytes were taken.
fn whole(taken: usize, n: usize) -> Result<(), ()> {
    match taken == n {
        true => Ok(()),
        false => Err(()),
    }
}

/// Whether `byte` is PDF's white-space (ISO 32000-2, 7.2.3), which
/// `ASCIIHexDecode` and `ASCII85Decode` ignore: NUL, tab, line feed, form
/// feed, carriage return or space. Vertical tab isn't, though it's C's.
const fn is_white_space(byte: u8) -> bool {
    matches!(byte, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

#[cfg(test)]
mod tests {
    use super::{
        Encoder, Error, Fault, Faults, Filter, Layout, Level, Prediction, Predictor, Reading, Row,
        Unencodable, decode, decode_into, decode_into_as, decode_prefix_into,
        decode_prefix_into_as, encode,
    };
    use crate::fuzz::{backends, zlib};

    fn layout(colors: u32, bits_per_component: u32, columns: u32) -> Layout {
        Layout {
            colors,
            bits_per_component,
            columns,
        }
    }

    /// LZW codes 256 65 66 300 257, nine bits each: 300 is past the
    /// table, whose next entry is 259.
    const PAST: [u8; 6] = [0x80, 0x10, 0x48, 0x52, 0xc8, 0x08];

    /// LZW codes of 3,839 bytes of no pattern that fill the table, then 65
    /// 66 258 257: the table full, read on, its entry 258 the first two
    /// bytes, 1 and 38.
    fn full() -> Vec<u8> {
        // 3,839 codes of bytes fill the table to 4,095, nine bits wide to
        // entry 510, ten to 1,022, eleven to 2,046, then twelve.
        let mut bits: Vec<bool> = Vec::new();
        let mut put =
            |code: u16, width: u32| bits.extend((0..width).rev().map(|i| code >> i & 1 == 1));
        let width = |k: u32| match k + 258 {
            ..=510 => 9,
            511..=1022 => 10,
            1023..=2046 => 11,
            _ => 12,
        };
        for k in 0..3839u32 {
            // Code k makes entry 257 + k (none for the first), as wide as
            // the table then is, early change on.
            put(1 + (k * 37 % 255) as u16, width(k.saturating_sub(1)));
        }
        for code in [65, 66, 258, 257] {
            put(code, 12);
        }
        (bits.chunks(8))
            .map(|b| (0..8).fold(0, |byte, i| byte << 1 | u8::from(b.get(i) == Some(&true))))
            .collect()
    }

    /// `data` decoded by `filters` under each reading, into a buffer that
    /// holds something: what was added and the faults.
    fn read_as(
        data: &[u8],
        filters: &[Filter],
        limit: usize,
        reading: Reading,
    ) -> (Vec<u8>, Result<(), Faults>) {
        let mut out = b"held".to_vec();
        let decoded = decode_into_as(data, filters, limit, backends()[0], reading, &mut out);
        assert_eq!(&out[..4], b"held", "what the buffer held, kept");
        (out.split_off(4), decoded)
    }

    /// qpdf's reading is what `decode_into` and `decode_prefix_into` read
    /// by, on clean data and damaged, by every filter.
    #[test]
    fn qpdfs_reading_is_the_functions_without_one() {
        let png = Predictor::Png(layout(1, 8, 2));
        let lzw = Filter::Lzw {
            early_change: true,
            predictor: Predictor::None,
        };
        for backend in backends() {
            let hello = zlib(b"hello", Level::DEFAULT, backend);
            let rows = zlib(&[1, 1, 1, 2, 1, 1, 0], Level::DEFAULT, backend);
            let cases: [(&[u8], Filter); 7] = [
                (
                    &hello,
                    Filter::Flate {
                        predictor: Predictor::None,
                    },
                ),
                (
                    &hello[..7],
                    Filter::Flate {
                        predictor: Predictor::None,
                    },
                ),
                (&rows, Filter::Flate { predictor: png }),
                (&PAST, lzw),
                (&[0x80, 0x4b, 0x08, 0x30, 0x10], lzw),
                (b"48 69", Filter::AsciiHex),
                (b"", lzw),
            ];
            for (data, filter) in cases {
                for limit in [3, 100] {
                    let mut a = b"held".to_vec();
                    let mut b = a.clone();
                    let into = decode_into(data, &[filter], limit, backend, &mut a);
                    let qpdf =
                        decode_into_as(data, &[filter], limit, backend, Reading::Qpdf, &mut b);
                    assert_eq!((a, into), (b, qpdf), "{filter:?}, {limit}");
                    let mut a = b"held".to_vec();
                    let mut b = a.clone();
                    let prefix = decode_prefix_into(data, filter, limit, backend, &mut a);
                    let qpdf =
                        decode_prefix_into_as(data, filter, limit, backend, Reading::Qpdf, &mut b);
                    assert_eq!((a, prefix), (b, qpdf), "{filter:?}, {limit}");
                }
            }
            assert_eq!(Reading::default(), Reading::Qpdf);
        }
    }

    /// Where qpdf's reading parts from qpdf, as `Reading::Qpdf` says: a
    /// full LZW table read on past, where qpdf stops; and what decoded
    /// before a fault that stops a filter kept, where qpdf gives the data
    /// undecoded.
    #[test]
    fn qpdfs_reading_parts_from_qpdf_where_it_says() {
        let lzw = [Filter::Lzw {
            early_change: true,
            predictor: Predictor::None,
        }];
        let (out, decoded) = read_as(&full(), &lzw, 1 << 20, Reading::Qpdf);
        assert_eq!(out.len(), 3839 + 4, "read on past the full table");
        assert_eq!(out[3839..], [65, 66, 1, 38]);
        assert!(matches!(
            &decoded.unwrap_err().0[..],
            [(0, Fault::TableFull { .. })]
        ));
        // A code not in the table stops it, what decoded before kept.
        let (out, decoded) = read_as(&PAST, &lzw, 1 << 20, Reading::Qpdf);
        assert_eq!(
            (out, decoded),
            (
                b"AB".to_vec(),
                Err(Faults(vec![(0, Fault::Code { at: 3 })]))
            )
        );
        for backend in backends() {
            let hello = zlib(b"hello, hello", Level::DEFAULT, backend);
            let cut = decode(
                &hello[..hello.len() - 4],
                &[Filter::Flate {
                    predictor: Predictor::None,
                }],
                100,
                backend,
            );
            assert_eq!(cut.unwrap_err().data, b"hello, hello", "{backend:?}");
        }
    }

    /// `decoded` is what the filter's decoder gave, before its predictor:
    /// a PNG predictor's tags among it, the rows a layout this crate's
    /// predictor doesn't take never undone, and what's refused cut away.
    #[test]
    fn decoded_is_what_the_decoder_gave_before_its_predictor() {
        for backend in backends() {
            let rows = zlib(&[1, 1, 1, 2, 1, 1, 0], Level::DEFAULT, backend);
            for (predictor, reading, added, decoded) in [
                (Predictor::None, Reading::Qpdf, 7, 7),
                (Predictor::Png(layout(1, 8, 2)), Reading::Qpdf, 6, 7),
                (Predictor::Tiff(layout(1, 8, 2)), Reading::Pdfium, 8, 7),
                (Predictor::Png(layout(1, 3, 2)), Reading::Pdfium, 0, 7),
                (Predictor::Tiff(layout(0, 8, 2)), Reading::Pdfium, 0, 7),
            ] {
                let mut out = b"held".to_vec();
                let flate = Filter::Flate { predictor };
                let (prefix, _) =
                    decode_prefix_into_as(&rows, flate, 100, backend, reading, &mut out);
                assert_eq!(
                    (out.len() - 4, prefix.decoded),
                    (added, decoded),
                    "{predictor:?}, {reading:?}"
                );
            }
            // Nothing read, nothing decoded.
            let mut out = Vec::new();
            let flate = Filter::Flate {
                predictor: Predictor::None,
            };
            let (prefix, _) =
                decode_prefix_into_as(b"", flate, 100, backend, Reading::Pdfium, &mut out);
            assert_eq!(prefix.decoded, 0);
        }
    }

    /// PDFium's predictors refuse what its filter decoded, under its
    /// reading, judged on the decoder's output: TIFF's and PNG's whose rows
    /// would be no bits (this crate's predictor doesn't take the layout
    /// either, which is said first) or more than 2³² − 1, PNG's on nothing.
    /// What's refused is cut away, the buffer as it was, and the next
    /// filter is given nothing. A layout PDFium's predictor takes and this
    /// crate's doesn't isn't refused: its parameters are said, as under
    /// qpdf's reading, with nothing decoded. `Predictor::pdfium_refuses`
    /// tells each the same from what the decoder gives alone.
    #[test]
    fn under_pdfiums_reading_its_predictors_refusals_are_said() {
        let refused = |why: &[Fault]| {
            let mut faults: Vec<(usize, Fault)> = why.iter().map(|&f| (0, f)).collect();
            faults.push((0, Fault::Refused));
            Err(Faults(faults))
        };
        let parameters = Err(Faults(vec![(0, Fault::Parameters)]));
        for backend in backends() {
            let six = zlib(b"abcdef", Level::DEFAULT, backend);
            let nothing = zlib(b"", Level::DEFAULT, backend);
            let huge = layout(u32::MAX, 16, 2);
            for (data, predictor, answer) in [
                (
                    &six,
                    Predictor::Png(layout(1, 8, 0)),
                    refused(&[Fault::Parameters]),
                ),
                (
                    &six,
                    Predictor::Tiff(layout(0, 8, 1)),
                    refused(&[Fault::Parameters]),
                ),
                (
                    &six,
                    Predictor::Tiff(layout(1, 0, 1)),
                    refused(&[Fault::Parameters]),
                ),
                (&six, Predictor::Png(huge), refused(&[])),
                (&six, Predictor::Tiff(huge), refused(&[])),
                (&nothing, Predictor::Png(layout(1, 8, 1)), refused(&[])),
                (
                    &nothing,
                    Predictor::Png(layout(1, 3, 1)),
                    refused(&[Fault::Parameters]),
                ),
                (&nothing, Predictor::Tiff(layout(1, 8, 1)), Ok(())),
                (&six, Predictor::Tiff(layout(1, 3, 1)), parameters.clone()),
            ] {
                let flate = [Filter::Flate { predictor }];
                let alone = Filter::Flate {
                    predictor: Predictor::None,
                };
                let mut out = Vec::new();
                let (alone, _) =
                    decode_prefix_into_as(data, alone, 100, backend, Reading::Pdfium, &mut out);
                let is_refused = matches!(&answer, Err(Faults(f)) if f.iter().any(|(_, f)| *f == Fault::Refused));
                assert_eq!(
                    predictor.pdfium_refuses(alone.decoded),
                    is_refused,
                    "{predictor:?}"
                );
                let mut out = b"held".to_vec();
                let decoded = decode_into_as(data, &flate, 100, backend, Reading::Pdfium, &mut out);
                let added = if answer.is_ok() {
                    b"held".to_vec()
                } else {
                    out.clone()
                };
                assert_eq!((decoded, &out[..]), (answer, &added[..]), "{predictor:?}");
                assert_eq!(&out[..4], b"held");
                assert_eq!(out.len(), 4, "{predictor:?}: nothing given");
                // Under qpdf's, none is refused.
                let mut out = Vec::new();
                let qpdf = decode_into_as(data, &flate, 100, backend, Reading::Qpdf, &mut out);
                assert!(
                    !matches!(&qpdf, Err(Faults(f)) if f.iter().any(|(_, f)| *f == Fault::Refused))
                );
            }
            // The next filter given nothing: LZW's refuses it too.
            let lzw = Filter::Lzw {
                early_change: true,
                predictor: Predictor::None,
            };
            let chain = [
                Filter::Flate {
                    predictor: Predictor::Png(layout(1, 8, 0)),
                },
                lzw,
            ];
            let mut out = Vec::new();
            let decoded = decode_into_as(&six, &chain, 100, backend, Reading::Pdfium, &mut out);
            let faults = vec![
                (0, Fault::Parameters),
                (0, Fault::Refused),
                (1, Fault::Refused),
            ];
            assert_eq!((out, decoded), (Vec::new(), Err(Faults(faults))));
        }
    }

    /// Under PDFium's reading, nothing decodes to nothing but where PDFium
    /// refuses it: LZW's, and behind a PNG predictor, or one whose rows
    /// would be no bits; Flate's alone, and the other filters', give
    /// nothing, as under qpdf's.
    #[test]
    fn under_pdfiums_reading_nothing_is_refused_where_pdfium_refuses_it() {
        let lzw = |predictor| Filter::Lzw {
            early_change: true,
            predictor,
        };
        for backend in backends() {
            for (filter, refused) in [
                (lzw(Predictor::None), vec![Fault::Refused]),
                (
                    Filter::Flate {
                        predictor: Predictor::Png(layout(1, 8, 1)),
                    },
                    vec![Fault::Refused],
                ),
                (
                    Filter::Flate {
                        predictor: Predictor::Tiff(layout(1, 8, 0)),
                    },
                    vec![Fault::Parameters, Fault::Refused],
                ),
                (
                    Filter::Flate {
                        predictor: Predictor::Tiff(layout(1, 8, 1)),
                    },
                    vec![],
                ),
                (
                    Filter::Flate {
                        predictor: Predictor::None,
                    },
                    vec![],
                ),
                (Filter::AsciiHex, vec![]),
                (Filter::RunLength, vec![]),
            ] {
                let mut out = b"held".to_vec();
                let (prefix, decoded) =
                    decode_prefix_into_as(b"", filter, 100, backend, Reading::Pdfium, &mut out);
                let faults = decoded
                    .err()
                    .map_or_else(Vec::new, |f| f.0.into_iter().map(|(_, f)| f).collect());
                assert_eq!(
                    (out, faults),
                    (b"held".to_vec(), refused.clone()),
                    "{filter:?}"
                );
                assert_eq!((prefix.read, prefix.ended, prefix.decoded), (0, false, 0));
                let mut out = Vec::new();
                let listed =
                    decode_into_as(b"", &[filter], 100, backend, Reading::Pdfium, &mut out);
                let faults = listed
                    .err()
                    .map_or_else(Vec::new, |f| f.0.into_iter().map(|(_, f)| f).collect());
                assert_eq!(faults, refused, "{filter:?}, listed");
            }
        }
    }

    /// Under PDFium's reading, LZW data PDFium refuses is cut away, what it
    /// read counted: a code past the table that's the first since a clear
    /// code, and data that decodes to nothing, its end or not.
    #[test]
    fn under_pdfiums_reading_lzw_it_refuses_is_cut_away() {
        let lzw = Filter::Lzw {
            early_change: true,
            predictor: Predictor::Png(layout(1, 8, 1)),
        };
        for backend in backends() {
            for (data, read, why) in [
                // 256 300 65 257: 300 first since the clear code.
                (
                    &[0x80, 0x4b, 0x08, 0x30, 0x10][..],
                    3,
                    vec![Fault::Code { at: 1 }],
                ),
                // 300 65 257: first at the start.
                (
                    &[0x96, 0x10, 0x60, 0x20][..],
                    2,
                    vec![Fault::Code { at: 0 }],
                ),
                // 256 257: nothing.
                (&[0x80, 0x40, 0x40][..], 3, vec![]),
                // 256, and no end: nothing.
                (&[0x80, 0x00][..], 2, vec![Fault::NoEnd]),
            ] {
                let mut out = b"held".to_vec();
                let (prefix, decoded) =
                    decode_prefix_into_as(data, lzw, 100, backend, Reading::Pdfium, &mut out);
                let mut faults = why.clone();
                faults.push(Fault::Refused);
                let faults: Vec<_> = faults.into_iter().map(|f| (0, f)).collect();
                assert_eq!(
                    (out, decoded),
                    (b"held".to_vec(), Err(Faults(faults))),
                    "{data:02x?}"
                );
                assert_eq!((prefix.read, prefix.decoded), (read, 0), "{data:02x?}");
            }
        }
    }

    #[test]
    fn nothing_decodes_to_nothing_whatever_the_filters() {
        let png = Predictor::Png(Layout::default());
        let filters = [
            Filter::AsciiHex,
            Filter::Ascii85,
            Filter::Lzw {
                early_change: true,
                predictor: png,
            },
            Filter::Flate { predictor: png },
            Filter::RunLength,
        ];
        for backend in backends() {
            for filter in filters {
                let decoded = decode(b"", &[filter], 100, backend);
                assert_eq!(decoded, Ok(Vec::new()), "{filter:?}");
            }
            // And what a filter decodes to nothing, the next takes as nothing.
            let chain = [Filter::AsciiHex, Filter::Flate { predictor: png }];
            assert_eq!(decode(b">", &chain, 100, backend), Ok(Vec::new()));
        }
    }

    #[test]
    fn each_filter_takes_what_the_one_before_gave() {
        let flate = Filter::Flate {
            predictor: Predictor::None,
        };
        for backend in backends() {
            let compressed = zlib(b"chained", Level::DEFAULT, backend);
            let hex: String = compressed.iter().map(|b| format!("{b:02x}")).collect();
            let chain = [Filter::AsciiHex, flate];
            let data = decode(format!("{hex}>").as_bytes(), &chain, 100, backend);
            assert_eq!(data.unwrap(), b"chained");
            // A fault in the first, and the second given what it gave: the
            // checksum's last byte gone.
            let cut = &hex.as_bytes()[..hex.len() - 2];
            let error = decode(cut, &chain, 100, backend).unwrap_err();
            assert_eq!(error.data, b"chained", "{backend:?}");
            assert_eq!(error.faults, [(0, Fault::NoEnd), (1, Fault::Truncated)]);
        }
    }

    /// Decoded into a buffer, what it held is kept before what's added,
    /// the limit bounds only what's added, and what decoded is kept with
    /// the faults.
    #[test]
    fn decoding_into_a_buffer_keeps_what_it_held_and_limits_what_it_adds() {
        let flate = Filter::Flate {
            predictor: Predictor::None,
        };
        let png = Filter::Flate {
            predictor: Predictor::Png(Layout {
                colors: 1,
                bits_per_component: 8,
                columns: 2,
            }),
        };
        for backend in backends() {
            let into = |data: &[u8], filters: &[Filter], limit| {
                let mut out = b"held".to_vec();
                let decoded = decode_into(data, filters, limit, backend, &mut out);
                (out, decoded)
            };
            let hello = zlib(b"hello", Level::DEFAULT, backend);
            assert_eq!(into(&hello, &[flate], 5), (b"heldhello".to_vec(), Ok(())));
            let limited = Err(Faults(vec![(0, Fault::Limit)]));
            assert_eq!(into(&hello, &[flate], 3), (b"heldhel".to_vec(), limited));
            let rows = zlib(&[1, 1, 1, 2, 1, 1], Level::DEFAULT, backend);
            assert_eq!(
                into(&rows, &[png], 4),
                (b"held\x01\x02\x02\x03".to_vec(), Ok(()))
            );
            let no_end = Err(Faults(vec![(0, Fault::NoEnd)]));
            assert_eq!(
                into(b"6869", &[Filter::AsciiHex], 2),
                (b"heldhi".to_vec(), no_end)
            );
            // Through a chain, the last filter's output added.
            let hex: String = hello.iter().map(|b| format!("{b:02x}")).collect();
            let chain = [Filter::AsciiHex, flate];
            let chained = into(format!("{hex}>").as_bytes(), &chain, 100);
            assert_eq!(chained, (b"heldhello".to_vec(), Ok(())));
            // No filters: the data, whole, past the limit, and so by decode.
            assert_eq!(into(b"plain", &[], 1), (b"heldplain".to_vec(), Ok(())));
            assert_eq!(decode(b"plain", &[], 1, backend), Ok(b"plain".to_vec()));
            // What decode gives is what's added.
            let mut out = b"held".to_vec();
            let decoded = decode_into(&hello[..6], &[flate], 100, backend, &mut out);
            let error = decode(&hello[..6], &[flate], 100, backend).unwrap_err();
            assert_eq!(
                (&out[4..], decoded),
                (&error.data[..], Err(Faults(error.faults)))
            );
        }
    }

    /// Data that may run on decodes into a buffer as `decode_into` decodes
    /// it by a list of its one filter, what the buffer held kept, under the
    /// limit; and nothing is read of nothing, which decodes to nothing.
    #[test]
    fn data_that_may_run_on_decodes_as_decode_into_decodes_it() {
        let flate = Filter::Flate {
            predictor: Predictor::None,
        };
        let lzw = Filter::Lzw {
            early_change: true,
            predictor: Predictor::None,
        };
        for backend in backends() {
            let hello = [&zlib(b"hello", Level::DEFAULT, backend)[..], b" EI"].concat();
            for (data, filter) in [
                (&hello[..], flate),
                (&hello[..9], flate),
                (b"48 69> EI", Filter::AsciiHex),
                (b"87cUR~> EI", Filter::Ascii85),
                (b"\x01ab\x80 EI", Filter::RunLength),
                (b"\x80\x0B\x60\x50\x22\x0C\x0C\x85\x01 EI", lzw),
                (b"\x80\x0B\x60", lzw),
            ] {
                for limit in [2, 100] {
                    let mut out = b"held".to_vec();
                    let (_, decoded) = decode_prefix_into(data, filter, limit, backend, &mut out);
                    let mut whole = b"held".to_vec();
                    let listed = decode_into(data, &[filter], limit, backend, &mut whole);
                    assert_eq!((out, decoded), (whole, listed), "{filter:?}, {limit}");
                }
            }
            let mut out = b"held".to_vec();
            let (prefix, decoded) = decode_prefix_into(b"", flate, 100, backend, &mut out);
            assert_eq!((prefix.read, prefix.ended), (0, false));
            assert_eq!((out, decoded), (b"held".to_vec(), Ok(())));
        }
    }

    /// Faults say what an error says.
    #[test]
    fn faults_say_each_fault_with_its_filter() {
        let faults = vec![(0, Fault::NoEnd), (1, Fault::Truncated)];
        let error = Error {
            data: b"data".to_vec(),
            faults: faults.clone(),
        };
        assert_eq!(Faults(faults).to_string(), error.to_string());
    }

    /// Encoded by Flate, written out as hex digits by hand, and decoded by
    /// both; and by Flate twice, decoded by the two in their order.
    #[test]
    fn what_flate_encodes_decodes_behind_another_filter() {
        let data = b"chained, and chained again".repeat(20);
        let flate = Encoder::Flate {
            level: Level::DEFAULT,
            predictor: Prediction::None,
        };
        let twice = [
            flate,
            Encoder::Flate {
                level: Level::BEST,
                predictor: Prediction::Png(
                    Layout {
                        colors: 1,
                        bits_per_component: 8,
                        columns: 26,
                    },
                    Row::Paeth,
                ),
            },
        ];
        for backend in backends() {
            let mut out = Vec::new();
            encode(&data, &[flate], backend, &mut out).unwrap();
            let hex: String = out.iter().map(|b| format!("{b:02x}")).collect();
            let filters = [Filter::AsciiHex, flate.filter()];
            let decoded = decode(format!("{hex}>").as_bytes(), &filters, 1 << 20, backend);
            assert_eq!(decoded.unwrap(), data, "{backend:?}");
            let mut out = Vec::new();
            encode(&data, &twice, backend, &mut out).unwrap();
            let filters = twice.map(|encoder| encoder.filter());
            for inflater in backends() {
                let decoded = decode(&out, &filters, 1 << 20, inflater);
                assert_eq!(decoded.unwrap(), data, "{backend:?}, {inflater:?}");
            }
        }
    }

    /// Encoded into a buffer, what it held is kept before what's added;
    /// when it can't be, it's as it was, wherever in the list the encoder
    /// that can't is.
    #[test]
    fn encoding_into_a_buffer_keeps_what_it_held_or_leaves_it_be() {
        let png = |columns, row| Encoder::Flate {
            level: Level::DEFAULT,
            predictor: Prediction::Png(
                Layout {
                    colors: 1,
                    bits_per_component: 8,
                    columns,
                },
                row,
            ),
        };
        let none = Layout {
            colors: 0,
            bits_per_component: 8,
            columns: 1,
        };
        let flate = Encoder::Flate {
            level: Level::DEFAULT,
            predictor: Prediction::None,
        };
        for backend in backends() {
            let mut out = b"held".to_vec();
            encode(b"hello", &[flate], backend, &mut out).unwrap();
            assert_eq!(out[..4], *b"held");
            assert_eq!(out[4..], zlib(b"hello", Level::DEFAULT, backend));
            let cases = [
                (
                    vec![png(3, Row::Up)],
                    Unencodable::PartialRow { missing: 1 },
                ),
                (
                    vec![flate, png(4, Row::Sub)],
                    Unencodable::PartialRow { missing: 3 },
                ),
                (
                    vec![png(2, Row::Up), flate],
                    Unencodable::PartialRow { missing: 1 },
                ),
                (
                    vec![Encoder::Flate {
                        level: Level::BEST,
                        predictor: Prediction::Png(none, Row::Up),
                    }],
                    Unencodable::Parameters,
                ),
            ];
            for (encoders, why) in cases {
                let mut out = b"held".to_vec();
                let encoded = encode(b"hello", &encoders, backend, &mut out);
                assert_eq!(
                    (encoded, &out[..]),
                    (Err(why), &b"held"[..]),
                    "{encoders:?}"
                );
            }
            let mut out = b"held".to_vec();
            encode(b"plain", &[], backend, &mut out).unwrap();
            assert_eq!(out, b"heldplain");
        }
    }

    /// Each reason data can't be encoded says what's wrong, word for word.
    #[test]
    fn each_reason_it_cant_encode_says_whats_wrong() {
        assert_eq!(
            Unencodable::Parameters.to_string(),
            "its predictor's parameters aren't ones it takes"
        );
        assert_eq!(
            Unencodable::PartialRow { missing: 2 }.to_string(),
            "the last row is 2 bytes short"
        );
    }

    /// Each fault says what's wrong, word for word.
    #[test]
    fn each_fault_says_whats_wrong() {
        let cases = [
            (
                Fault::Byte { at: 3, byte: b'z' },
                "byte 0x7a at 3 isn't one it takes",
            ),
            (Fault::NoEnd, "no end-of-data marker"),
            (Fault::BrokenEnd { at: 4 }, "`~` at 4 isn't followed by `>`"),
            (Fault::InGroup { at: 5 }, "`z` at 5 is inside a group"),
            (Fault::Overflow { at: 6 }, "the group at 6 is over 2^32 - 1"),
            (
                Fault::LoneByte { at: 7 },
                "the last group, at 7, is one character",
            ),
            (
                Fault::Code { at: 8 },
                "the code at byte 8 isn't in the table",
            ),
            (Fault::TableFull { at: 9 }, "the table is full at byte 9"),
            (Fault::Header, "no zlib header"),
            (
                Fault::Corrupt { at: 10 },
                "the deflate data is corrupt at byte 10",
            ),
            (Fault::Truncated, "the data ends early"),
            (Fault::Checksum, "the Adler-32 checksum doesn't match"),
            (
                Fault::RowTag { row: 2, tag: 9 },
                "row 2's first byte is 9, not 0 to 4",
            ),
            (
                Fault::PartialRow { missing: 3 },
                "the last row is 3 bytes short",
            ),
            (
                Fault::Parameters,
                "its predictor's parameters aren't ones it takes",
            ),
            (Fault::Limit, "its output is over the limit"),
            (
                Fault::AsNext { at: 11 },
                "the code at byte 11 is past the table's next entry, read as it",
            ),
            (Fault::Refused, "it's refused, as PDFium refuses it"),
        ];
        for (fault, said) in cases {
            assert_eq!(fault.to_string(), said, "{fault:?}");
        }
    }

    /// An error's `Debug` gives what decoded by its length, not its bytes,
    /// which may be a whole stream's, decrypted; and each fault.
    #[test]
    fn an_errors_debug_gives_its_datas_length_not_its_bytes() {
        let error = Error {
            data: b"a stream's decoded data".to_vec(),
            faults: vec![(0, Fault::Checksum)],
        };
        assert_eq!(
            format!("{error:?}"),
            "Error { data: 23 bytes, faults: [(0, Checksum)] }"
        );
    }

    /// An error says each fault after its filter's index, in the list's
    /// order.
    #[test]
    fn an_error_says_each_fault_with_its_filter() {
        let error = |faults| Error {
            data: Vec::new(),
            faults,
        };
        assert_eq!(
            error(vec![(0, Fault::NoEnd)]).to_string(),
            "filter 0: no end-of-data marker"
        );
        assert_eq!(
            error(vec![(0, Fault::NoEnd), (1, Fault::Truncated)]).to_string(),
            "filter 0: no end-of-data marker; filter 1: the data ends early"
        );
    }

    /// White-space is the six bytes of ISO 32000-2's table, and no other
    /// byte of the 256.
    #[test]
    fn white_space_is_the_six_bytes_and_no_other() {
        let six = [0x00, 0x09, 0x0a, 0x0c, 0x0d, 0x20];
        for byte in 0..=u8::MAX {
            let expected = six.contains(&byte);
            assert_eq!(super::is_white_space(byte), expected, "{byte:#04x}");
        }
    }

    /// The README is shown on crates.io and docs.rs, where a path beside
    /// the crate reaches nothing: its licence links are the repository's
    /// URLs, the manifest's `repository` and the file, and no link of it
    /// climbs out of the crate.
    #[test]
    fn the_readmes_licence_links_reach_the_files() {
        let readme = include_str!("../README.md");
        let repository = env!("CARGO_PKG_REPOSITORY");
        for file in ["LICENSE-APACHE", "LICENSE-MIT"] {
            let link = format!("[{file}]({repository}/blob/main/{file})");
            assert!(
                readme.contains(&link),
                "{file}'s link isn't the repository's URL"
            );
        }
        assert!(!readme.contains("](../"), "a link climbs out of the crate");
    }
}

/// The README's examples, run as doctests where miniz_oxide, which they
/// name, is compiled in.
#[cfg(all(doctest, feature = "miniz_oxide"))]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
