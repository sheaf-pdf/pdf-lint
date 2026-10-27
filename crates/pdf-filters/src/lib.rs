//! PDF's standard filters for data of any kind (ISO 32000-2, 7.4), decoded:
//! `ASCIIHexDecode`, `ASCII85Decode`, `LZWDecode` and `FlateDecode` with
//! their predictors, and `RunLengthDecode`. The filters for images (DCT,
//! JPX, JBIG2, CCITT fax) and `Crypt` aren't here: decrypting is a step
//! before the filters, the caller's.
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
//!   its output, and nothing the size of either besides.
//! - **Parameters, not dictionaries.** A [`Filter`] says what its
//!   `/DecodeParms` do; reading them from a dictionary is the caller's, so
//!   it works with any object model.
//!
//! `FlateDecode` decompresses through
//! [miniz_oxide](https://crates.io/crates/miniz_oxide).
//!
//! ```
//! use pdf_filters::{Filter, decode};
//!
//! let data = decode(b"48 65 6C 6C 6F>", &[Filter::AsciiHex], 1 << 20);
//! assert_eq!(data.unwrap(), b"Hello");
//!
//! // No `>` at the end: the data decodes, and the fault is said.
//! let error = decode(b"48 69", &[Filter::AsciiHex], 1 << 20).unwrap_err();
//! assert_eq!(error.data, b"Hi");
//! assert_eq!(error.faults, [(0, pdf_filters::Fault::NoEnd)]);
//! ```

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Fault {
    /// A byte the filter doesn't take, at `at` in its input: in
    /// `ASCIIHexDecode` one that's neither a hex digit, white-space nor
    /// `>`, and in `ASCII85Decode` one that's neither `!` to `u`, `z`,
    /// white-space nor `~`. It stopped there.
    Byte {
        /// Where, in the filter's input.
        at: usize,
        /// The byte.
        byte: u8,
    },
    /// The data ends without its end-of-data marker: `>`, `~>`, an LZW
    /// code of 257, or a run-length of 128. It didn't stop: everything was
    /// decoded.
    NoEnd,
    /// `~` at `at`, in `ASCII85Decode`, not followed by `>`. It stopped
    /// there, without what was decoded of the group it's in.
    BrokenEnd {
        /// Where, in the filter's input.
        at: usize,
    },
    /// `z` at `at`, in `ASCII85Decode`, inside a group. It stopped there.
    InGroup {
        /// Where, in the filter's input.
        at: usize,
    },
    /// A group at `at`, in `ASCII85Decode`, whose value is over 2³² − 1.
    /// It didn't stop: the group is read as its value's low 32 bits.
    Overflow {
        /// Where the group begins, in the filter's input.
        at: usize,
    },
    /// A last group at `at`, in `ASCII85Decode`, of one character, which
    /// is no bytes. It didn't stop.
    LoneByte {
        /// Where, in the filter's input.
        at: usize,
    },
    /// An LZW code at byte `at` that isn't in the table yet. It stopped
    /// there.
    Code {
        /// The byte the code begins in, in the filter's input.
        at: usize,
    },
    /// The LZW table full, 4,096 entries, at byte `at`, with no clear code
    /// to empty it. It stopped there.
    TableFull {
        /// The byte the code that would add to it begins in, in the
        /// filter's input.
        at: usize,
    },
    /// The data doesn't begin with a zlib header for deflate data without a
    /// preset dictionary. It stopped there, with nothing decoded.
    Header,
    /// Deflate data that isn't, at byte `at`. It stopped there.
    Corrupt {
        /// Where, in the filter's input, as near as can be told.
        at: usize,
    },
    /// The data ends inside what it's decoding: a deflate stream before
    /// its end, a run-length's run. It stopped there, with what was
    /// decoded before.
    Truncated,
    /// The zlib data's Adler-32 checksum isn't its decompressed data's. It
    /// didn't stop: everything was decoded, and readers read it so.
    Checksum,
    /// A PNG-predicted row, the `row`th from 0, whose first byte, `tag`,
    /// isn't 0 to 4. It didn't stop: the row is read as predicted by
    /// nothing.
    RowTag {
        /// Which row, from 0.
        row: usize,
        /// Its first byte.
        tag: u8,
    },
    /// The predicted data's last row short by `missing` bytes. It didn't
    /// stop: the row is read with zeros for them.
    PartialRow {
        /// How many bytes the row lacks.
        missing: usize,
    },
    /// A predictor's [`Layout`] that isn't one: no colors or columns, or
    /// bits a sample other than 1, 2, 4, 8 or 16. Nothing was decoded.
    Parameters,
    /// The filter's output over the limit. It stopped there, with as much
    /// as the limit allows.
    Limit,
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Fault::Byte { at, byte } => write!(f, "byte {byte:#04x} at {at} isn't one it takes"),
            Fault::NoEnd => write!(f, "no end-of-data marker"),
            Fault::BrokenEnd { at } => write!(f, "`~` at {at} isn't followed by `>`"),
            Fault::InGroup { at } => write!(f, "`z` at {at} is inside a group"),
            Fault::Overflow { at } => write!(f, "the group at {at} is over 2^32 - 1"),
            Fault::LoneByte { at } => write!(f, "the last group, at {at}, is one character"),
            Fault::Code { at } => write!(f, "the code at byte {at} isn't in the table"),
            Fault::TableFull { at } => write!(f, "the table is full at byte {at}"),
            Fault::Header => write!(f, "no zlib header"),
            Fault::Corrupt { at } => write!(f, "the deflate data is corrupt at byte {at}"),
            Fault::Truncated => write!(f, "the data ends early"),
            Fault::Checksum => write!(f, "the Adler-32 checksum doesn't match"),
            Fault::RowTag { row, tag } => write!(f, "row {row}'s first byte is {tag}, not 0 to 4"),
            Fault::PartialRow { missing } => write!(f, "the last row is {missing} bytes short"),
            Fault::Parameters => write!(f, "its predictor's parameters aren't ones it takes"),
            Fault::Limit => write!(f, "its output is over the limit"),
        }
    }
}

/// Data that didn't decode cleanly: what decoded, and why it's not clean.
#[derive(Clone, Debug, PartialEq, Eq)]
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
        for (i, (filter, fault)) in self.faults.iter().enumerate() {
            if i > 0 {
                write!(f, "; ")?;
            }
            write!(f, "filter {filter}: {fault}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

/// `data` decoded by `filters` in their order, as a stream's `/Filter`
/// lists them, no filter's output over `limit` bytes: an LZW or Flate
/// filter's before its predictor too, PNG's tags and all. With no
/// filters, it's `data`.
pub fn decode(data: &[u8], filters: &[Filter], limit: usize) -> Result<Vec<u8>, Error> {
    let mut faults = Vec::new();
    let mut decoded = None;
    for (i, filter) in filters.iter().enumerate() {
        let input = decoded.as_deref().unwrap_or(data);
        // Nothing decodes to nothing, as in qpdf, whatever the filter.
        if input.is_empty() {
            decoded = Some(Vec::new());
            continue;
        }
        let mut out = Out::new(limit);
        let predictor = match *filter {
            Filter::AsciiHex => {
                hex::decode(input, &mut out);
                Predictor::None
            }
            Filter::Ascii85 => {
                ascii85::decode(input, &mut out);
                Predictor::None
            }
            Filter::Lzw {
                early_change,
                predictor,
            } => {
                lzw::decode(input, early_change, &mut out);
                predictor
            }
            Filter::Flate { predictor } => {
                flate::decode(input, &mut out);
                predictor
            }
            Filter::RunLength => {
                run_length::decode(input, &mut out);
                Predictor::None
            }
        };
        if predictor != Predictor::None {
            let predicted = std::mem::take(&mut out.data);
            predictor::undo(predicted, predictor, &mut out);
        }
        faults.extend(out.faults.into_iter().map(|fault| (i, fault)));
        decoded = Some(out.data);
    }
    let data = decoded.unwrap_or_else(|| data.to_vec());
    match faults.is_empty() {
        true => Ok(data),
        false => Err(Error { data, faults }),
    }
}

/// A filter's output, kept to the limit, and its faults.
struct Out {
    data: Vec<u8>,
    limit: usize,
    faults: Vec<Fault>,
}

impl Out {
    fn new(limit: usize) -> Out {
        Out {
            data: Vec::new(),
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
        let room = self.limit - self.data.len();
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
    use super::{Filter, Layout, Predictor, decode};

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
        for filter in filters {
            assert_eq!(decode(b"", &[filter], 100), Ok(Vec::new()), "{filter:?}");
        }
        // And what a filter decodes to nothing, the next takes as nothing.
        let chain = [Filter::AsciiHex, Filter::Flate { predictor: png }];
        assert_eq!(decode(b">", &chain, 100), Ok(Vec::new()));
    }

    #[test]
    fn each_filter_takes_what_the_one_before_gave() {
        let flate = Filter::Flate {
            predictor: Predictor::None,
        };
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(b"chained", 6);
        let hex: String = compressed.iter().map(|b| format!("{b:02x}")).collect();
        let chain = [Filter::AsciiHex, flate];
        let data = decode(format!("{hex}>").as_bytes(), &chain, 100);
        assert_eq!(data.unwrap(), b"chained");
        // A fault in the first, and the second given what it gave: the
        // checksum's last byte gone.
        let error = decode(&hex.as_bytes()[..hex.len() - 2], &chain, 100).unwrap_err();
        assert_eq!(error.data, b"chained");
        assert_eq!(
            error.faults,
            [(0, super::Fault::NoEnd), (1, super::Fault::Truncated)]
        );
    }
}

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
