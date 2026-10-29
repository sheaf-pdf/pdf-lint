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
//!   open, it's read as qpdf reads it; where a fault stops a filter, what
//!   decoded before it is kept, which qpdf, giving the data undecoded,
//!   doesn't keep.
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

/// Which deflate implementation a Flate filter inflates and deflates
/// with. Each exists only with its feature, `miniz_oxide` (the default)
/// or `zlib-rs`; with both on, both do, and every call names the one it
/// takes, so that a feature another crate turns on changes no call's
/// output.
///
/// On data that decodes cleanly, the two give the same. On damaged data,
/// where each stops, how much it decoded before and the place a fault
/// says can differ. What each encodes differs, byte for byte, at a level:
/// each is zlib data any inflater reads, the same as
/// [flate2](https://crates.io/crates/flate2) on that backend gives.
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

/// A standard filter for data of any kind, with what its parameters say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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

/// How the data a predictor works on is laid out in rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
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
    /// there.
    #[error("the code at byte {at} isn't in the table")]
    Code {
        /// The byte the code begins in, in the filter's input.
        at: usize,
    },
    /// The LZW table full, 4,096 entries, at byte `at`, with no clear code
    /// to empty it. It stopped there.
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
    /// The filter's output over the limit. It stopped there, with as much
    /// as the limit allows.
    #[error("its output is over the limit")]
    Limit,
}

/// Data that didn't decode cleanly: what decoded, and why it's not clean.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
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

/// Why data decoded into a caller's buffer didn't decode cleanly: each
/// fault, with the index of its filter in the list, in the list's order,
/// as an [`Error`] has them; what decoded is in the buffer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
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
/// besides; Flate inflated by `backend`. With no filters, it's `data`. The
/// limit bounds what's allocated, so it should be one that can be: a short
/// last row is read with zeros up to it. It's [`decode_into`] an empty
/// `Vec`, which is then cut to its length.
pub fn decode(
    data: &[u8],
    filters: &[Filter],
    limit: usize,
    backend: Backend,
) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let decoded = decode_into(data, filters, limit, backend, &mut out);
    out.shrink_to_fit();
    match decoded {
        Ok(()) => Ok(out),
        Err(Faults(faults)) => Err(Error { data: out, faults }),
    }
}

/// `data` decoded as [`decode`] decodes it, appended to `out`: what `out`
/// held is kept, before it, and `limit` bounds what's added, the last
/// filter's output, not what was there. With faults, `out` has what
/// decoded, as an [`Error`]'s `data` does. The last filter decodes
/// straight into `out`, grown with zeros as it fills and cut to what was
/// written; those before it each into a buffer of its own.
pub fn decode_into(
    data: &[u8],
    filters: &[Filter],
    limit: usize,
    backend: Backend,
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
        // Nothing decodes to nothing, as in qpdf, whatever the filter.
        if !input.is_empty() {
            one(input, filter, limit, backend, &mut into);
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

/// `input` decoded by `filter` into `out`, its predictor undone, the
/// predictor's output no more than `limit` bytes.
fn one(input: &[u8], filter: &Filter, limit: usize, backend: Backend, out: &mut Out) {
    let predictor = match *filter {
        Filter::AsciiHex => {
            hex::decode(input, out);
            Predictor::None
        }
        Filter::Ascii85 => {
            ascii85::decode(input, out);
            Predictor::None
        }
        Filter::Lzw {
            early_change,
            predictor,
        } => {
            lzw::decode(input, early_change, out);
            predictor
        }
        Filter::Flate { predictor } => {
            flate::decode(input, backend, out);
            predictor
        }
        Filter::RunLength => {
            run_length::decode(input, out);
            Predictor::None
        }
    };
    if predictor != Predictor::None {
        // Data a fault stopped mid-row ends in a row cut off, not short:
        // it isn't read.
        let cut = (out.faults.iter()).any(|f| {
            matches!(
                f,
                Fault::Limit | Fault::Corrupt { .. } | Fault::Code { .. } | Fault::TableFull { .. }
            )
        });
        out.limit = limit;
        predictor::undo(predictor, cut, out);
    }
}

/// What a filter does to data as a writer encodes it: the filter, and
/// what the writer chooses that decoding doesn't need to know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    pub fn filter(&self) -> Filter {
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
    pub const fn new(level: u8) -> Option<Level> {
        match level {
            0..=9 => Some(Level(level)),
            _ => None,
        }
    }

    /// Its number, 0 to 9.
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
    pub const fn predictor(self) -> u8 {
        match self {
            Prediction::None => 1,
            Prediction::Png(_, row) => 10 + row.tag(),
        }
    }
}

/// Which of PNG's filters predicts each row from the bytes before it in
/// the row and the row above it, read as zeros for the first row.
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
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
/// Flate's zlib data is what [flate2](https://crates.io/crates/flate2)'s
/// `ZlibEncoder` gives on the same backend at the same level: a window of
/// 32 KiB, zlib's memory level 8 and its default strategy, and zlib-rs
/// called as flate2 calls it, 32 KiB to write into at a call, as what it
/// writes depends on where that room runs out.
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

#[cfg(test)]
mod tests {
    use super::{
        Encoder, Error, Fault, Faults, Filter, Layout, Level, Prediction, Predictor, Row,
        Unencodable, decode, decode_into, encode,
    };
    use crate::fuzz::{backends, zlib};

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
            // No filters: the data, whole.
            assert_eq!(into(b"plain", &[], 1), (b"heldplain".to_vec(), Ok(())));
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
        ];
        for (fault, said) in cases {
            assert_eq!(fault.to_string(), said, "{fault:?}");
        }
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
}

/// The README's examples, run as doctests where miniz_oxide, which they
/// name, is compiled in.
#[cfg(all(doctest, feature = "miniz_oxide"))]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
