//! The parser: values, and indirect objects, read from a file's bytes or
//! an object stream's decoded data (ISO 32000-2, 7.3), on
//! [`pdf_tokenizer`]. Each value is placed where it was written, in a
//! [`Span`]; what the parser reads past is a [`Fault`], told to a [`Sink`],
//! and what stops a value is an [`Error`].
//!
//! An indirect object (7.3.10) is read in steps, so that which bytes are a
//! stream's data stays the caller's to choose: [`Parser::header`] reads its
//! `n g obj`; [`Parser::body`] its value, or a stream's dictionary to where
//! its data starts, with both extents the data may have, by a `/Length`
//! the caller resolves ([`StreamHead::declared`]) and up to `endstream`
//! ([`StreamHead::scanned`]); and [`Parser::stream`] the rest of the
//! stream, after the extent the caller chose.
//!
//! A reader that reads a file by position gives the parser a window of it
//! ([`Parser::window`]): the bytes it has read from an offset on, where
//! they start in the file, and whether they end it. Spans, offsets and
//! errors are then the file's. In a window that ends before the file
//! does, a call whose read reaches the window's end is [`Error::Short`],
//! as more of the file could change what it reads (pdf-tokenizer's "A
//! slice of a file"), and the caller reads again with more; read so, it
//! reads as it would with the whole file. A call that comes back short
//! may have told its sink of faults that more of the file shows aren't,
//! an `endobj` cut short among them, and the call that reads tells it
//! again of those that are: a caller reading in windows keeps a call's
//! faults only once it doesn't come back short.
//!
//! It reads by ISO 32000-2's lexical rules, or by PDFium's
//! ([`Parser::rules`]): its tokens then cut where PDFium cuts them, and
//! each read as PDFium's object parser reads it, a fault said where ISO
//! 32000-2 reads it otherwise. What it makes of the tokens, a reference or
//! a dictionary, is its own under either.

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use memchr::{memchr2, memchr2_iter};
use pdf_tokenizer::decode::pdfium::{self, Number};
use pdf_tokenizer::decode::{self, How, NumberError, Pieces, Quirks};
use pdf_tokenizer::{Iso, Kind, Pdfium, RuleSet, Rules, Token, Tokenizer};

use crate::object::{Data, Dict, Entry, Form, Object, ObjectId, Span, Str, Stream, Value};

/// How deeply arrays and dictionaries may nest in an object, unless a
/// parser is given other [`Limits`]: [`Limits::DEFAULT`]'s `nesting`.
pub const NESTING: usize = 256;

/// The caps a parser reads under, so that what it reads can't make it
/// recurse, or hold in memory, without end. A caller sets them on a copy
/// of [`Limits::DEFAULT`] (`limits.nesting = 64`, say) and gives it to
/// [`Parser::limits`]. Only a value reaches them: a token where a value
/// should be that can't start one is the error it is, under any limits.
///
/// The writer ([`serialize`](crate::serialize)) keeps to the default's
/// nesting, so that a parser reads what it writes, but not to its count
/// of values: it writes a value of more than 2^25, which a parser reads
/// back only under limits that take them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// How deeply arrays and dictionaries may nest: a value inside this
    /// many of them reads, and one inside more doesn't. [`NESTING`], 256,
    /// by default.
    pub nesting: usize,
    /// How many values one call may read, in all: the value, an array's
    /// elements, a dictionary's keys and values, and theirs. What a value
    /// holds in memory grows with its count of values more than with its
    /// bytes (an empty array is two bytes written, and tens in memory), so
    /// this caps that. 2^25 by default.
    ///
    /// At worst, a call's values take 288 bytes each in memory, on a 64-bit
    /// target, besides their names' and strings' bytes: an element is held
    /// in its array as an [`Object`], at most 72 bytes, and a key and its
    /// value in their dictionary as an [`Entry`], at most 128, and an
    /// array's or a dictionary's room grows as a `Vec`'s does, to at most
    /// twice what it holds and at least four, so an array of one value
    /// holds four `Object`s' room. Under the default, one call may then
    /// hold 9 GiB, read from 64 MiB of arrays of one value nested in each
    /// other.
    pub values: usize,
}

impl Limits {
    /// The limits a parser reads under unless it's given others: values
    /// nested at most [`NESTING`] deep, and at most 2^25 of them in one
    /// call.
    pub const DEFAULT: Limits = Limits {
        nesting: NESTING,
        values: 1 << 25,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}

/// Where a parser says what it read past: a checker keeps it; `()` drops
/// it, and wants none, so none is looked for. A fault borrows from `'a`,
/// the data the parser reads, so a sink can keep it as it is. A sink
/// borrowed, `&mut S` or `&mut dyn Sink`, is a sink too.
///
/// It's the parser's own, not the [`fault::Sink`](crate::fault::Sink) a
/// reader of a file's objects tells: a fault borrows the data parsed, and
/// is placed by a [`Span`] alone, as the object it's in, where it's in one,
/// is the caller's to know. A reader that calls the parser tells its own
/// faults of what it's told, at their places.
pub trait Sink<'a> {
    /// A fault at `at`, which the parser read past.
    fn fault(&mut self, at: Span, fault: Fault<'a>);

    /// Whether faults are wanted. A check that does work of its own, such
    /// as reading a token as ISO 32000-2 reads it beside the reading
    /// PDFium's rules take ([`Parser::rules`]), runs only when they are,
    /// and a sink that doesn't want them is told none. True unless a sink
    /// says otherwise; asked as the parser reads.
    #[must_use]
    fn wants(&self) -> bool {
        true
    }
}

/// Drops every fault, and wants none, so none is looked for.
impl Sink<'_> for () {
    #[inline]
    fn fault(&mut self, _: Span, _: Fault<'_>) {}

    #[inline]
    fn wants(&self) -> bool {
        false
    }
}

/// Tells the sink it borrows, and wants what it wants.
impl<'a, S: Sink<'a> + ?Sized> Sink<'a> for &mut S {
    #[inline]
    fn fault(&mut self, at: Span, fault: Fault<'a>) {
        (**self).fault(at, fault);
    }

    #[inline]
    fn wants(&self) -> bool {
        (**self).wants()
    }
}

/// What the parser read past: the value is read, as readers read it, and
/// the fault said.
///
/// By PDFium's rules ([`Parser::rules`]), a token is read as PDFium's
/// object parser reads it, and where ISO 32000-2 reads it otherwise,
/// that's a fault: [`Number`](Fault::Number),
/// [`OutOfRange`](Fault::OutOfRange), [`HexString`](Fault::HexString),
/// [`StringCr`](Fault::StringCr), [`Long`](Fault::Long), and
/// [`Name`](Fault::Name) as by ISO 32000-2's rules; and a byte that's
/// white-space to PDFium and a regular character to ISO 32000-2,
/// [`HighByte`](Fault::HighByte).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Fault<'a> {
    /// An integer outside 64 bits, read as the nearest that fits, by ISO
    /// 32000-2's rules.
    #[error("an integer outside 64 bits, read as the nearest that fits")]
    IntegerOverflow,
    /// A real too large for 64 bits, read as the largest, by ISO 32000-2's
    /// rules.
    #[error("a real too large for 64 bits, read as the largest")]
    RealOverflow,
    /// A real too small for 64 bits, its digits not all zero, read as 0
    /// with its sign, by ISO 32000-2's rules, which read the decimal
    /// written (7.3.3), its range and precision left to the processor
    /// (Annex C, which is advice). Not by PDFium's, which read a number
    /// from its first 256 bytes ([`Long`](Fault::Long)), none of which is
    /// too small for 64 bits: the least, `.` and 254 zeros and a `1`, is
    /// 1e-255.
    #[error("a real too small for 64 bits, read as 0")]
    RealUnderflow,
    /// A name with a `#` without two hex digits after it, or with `#00`,
    /// NUL, which a name can't hold (ISO 32000-2, 7.3.5), readers reading
    /// each their own way. By ISO 32000-2's rules, read with the `#` as
    /// itself; by PDFium's, as an escape when two bytes follow it, a byte
    /// that's no hex digit read as 0, and as itself when fewer do; `#00`
    /// as NUL by both.
    #[error(
        "the name {written} has {what}, which readers read differently",
        written = first_40(.written),
        what = name_fault(*.stray, *.nul)
    )]
    Name {
        /// The name as written, its `/` first.
        written: &'a [u8],
        /// Whether it has a `#` without two hex digits after it.
        stray: bool,
        /// Whether it has `#00`.
        nul: bool,
    },
    /// No end of line after a stream's `stream` keyword.
    #[error("no end of line after `stream`")]
    NoEol,
    /// A lone CR after a stream's `stream` keyword, where ISO 32000-2 wants
    /// CR LF or LF (7.3.8.1).
    #[error("a lone CR after `stream`, where ISO 32000-2 wants CR LF or LF")]
    LoneCr,
    /// No `endobj` after an indirect object.
    #[error("{id} obj has no endobj")]
    NoEndobj {
        /// The object.
        id: ObjectId,
    },
    /// A word of digits, signs and points that ISO 32000-2 doesn't write
    /// as a number, such as `--5`, `1.2.3`, `+` or `.`: a number by
    /// PDFium's rules, read as PDFium reads it (`--5` the integer 0,
    /// `1.2.3` the real 1.2), and a keyword to ISO, whose integer is
    /// digits, a sign before them or not, and whose real has one point
    /// among its digits too (7.3.3, "shall").
    #[error(
        "{} is a number by PDFium's rules and a keyword to ISO 32000-2",
        first_40(.written)
    )]
    Number {
        /// The word.
        written: &'a [u8],
    },
    /// An integer outside the 32 bits PDFium's rules read, unsigned when
    /// it's written without a sign and signed when with one: read as 0,
    /// as PDFium reads it, where ISO 32000-2 reads the integer written
    /// ("shall"), its range left to the processor (7.3.3; Annex C, which
    /// is advice).
    #[error(
        "{} is outside the integers PDFium's rules read, and read as 0",
        first_40(.written)
    )]
    OutOfRange {
        /// The integer.
        written: &'a [u8],
    },
    /// A hex string with a byte that's neither a hex digit nor
    /// white-space: read by PDFium's rules to its `>`, the byte passed
    /// over, where ISO 32000-2's hex string holds hex digits and
    /// white-space alone (7.3.4.3, "shall"), and ends before any other
    /// byte.
    #[error("the hex string {} has a byte that's no hex digit", first_40(.written))]
    HexString {
        /// The string, `<` to `>`.
        written: &'a [u8],
    },
    /// A literal string with a carriage return written in it, not
    /// escaped: read as written by PDFium's rules, a CR or a CR LF, where
    /// ISO 32000-2 reads an end of line in a literal string as a line
    /// feed alone (7.3.4.2, "shall").
    #[error(
        "the string {} has a CR written in it, read as written, where ISO 32000-2 reads LF",
        first_40(.written)
    )]
    StringCr {
        /// The string, `(` to `)`.
        written: &'a [u8],
    },
    /// A number or a name longer than the 256 bytes PDFium's object parser
    /// keeps of a word: read by PDFium's rules as its first 256, a name's
    /// `/` among them, the rest dropped, where ISO 32000-2 reads every
    /// byte, and sets no length (Annex C's lengths are advice).
    #[error(
        "{} is longer than the 256 bytes PDFium's rules read of a word, and read as its first 256",
        first_40(.written)
    )]
    Long {
        /// The word, whole.
        written: &'a [u8],
    },
    /// A byte 0x80 or 0xFF where an object's white-space is, by PDFium's
    /// rules: white-space to PDFium, ending a word or a name and passed
    /// over between tokens, where ISO 32000-2 reads it as a regular
    /// character, part of the word or the name beside it, or a word of its
    /// own (7.2.3). Told for each such byte in the object a call reads,
    /// from its first token to the end of its last, once the call has read
    /// it: a value; a stream's dictionary through its `stream` keyword and
    /// end of line ([`Parser::body`]); and from the end of the stream's
    /// data through `endstream` ([`Parser::stream`]). Not in a string or a
    /// comment, where it's a byte like any other, nor in a stream's data;
    /// nor around an object, before its first token or after its last,
    /// which is no object's, nor by a call that fails ([`Parser::rules`]).
    #[error("{} is white-space here and a regular character to ISO 32000-2", byte(.written))]
    HighByte {
        /// The byte.
        written: &'a [u8],
    },
}

/// A word's or a string's first 40 bytes, for a [`Fault`]'s text.
fn first_40(written: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(&written[..written.len().min(40)])
}

/// A byte in hex, `0x80`, for [`Fault::HighByte`]'s text.
fn byte(written: &[u8]) -> String {
    written
        .first()
        .map_or_else(String::new, |b| format!("0x{b:02X}"))
}

/// What's wrong with a name, for [`Fault::Name`]'s text.
fn name_fault(stray: bool, nul: bool) -> &'static str {
    match (stray, nul) {
        (true, true) => "a # without two hex digits after it, and #00, NUL",
        (true, false) => "a # without two hex digits after it",
        _ => "#00, NUL, which a name can't hold",
    }
}

/// What stops the parser reading a value. Each offset it names counts
/// where its spans do, in the file, a window's base and all
/// ([`Parser::window`]), or in an object stream's data: [`Error::Header`]'s
/// where the parser was, as it was given, and [`Error::Extent`]'s as its
/// caller gave them.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A value nested deeper in arrays and dictionaries than the parser
    /// allows, [`Limits::nesting`] ([`NESTING`] by default).
    #[error("arrays and dictionaries nested deeper than {limit}")]
    Nested {
        /// How deeply they may nest.
        limit: usize,
        /// Where the value nested deeper starts.
        at: usize,
    },
    /// More values in one call than the parser allows, [`Limits::values`]:
    /// the value, an array's elements, a dictionary's keys and values, and
    /// theirs.
    #[error("more than {limit} values in one object, one more at byte {at}")]
    Values {
        /// How many one call may read.
        limit: usize,
        /// Where the value past them starts.
        at: usize,
    },
    /// The data ends where a value should be.
    #[error("the data ends where a value should be")]
    End,
    /// A reference, `n g R`, whose number doesn't fit 32 bits or whose
    /// generation doesn't fit 16, which numbers no object.
    #[error(
        "a reference at byte {at}, {} {} R, numbers no object",
        lossy(.number),
        lossy(.generation)
    )]
    Reference {
        /// Where it starts.
        at: usize,
        /// Its number, as written.
        number: Vec<u8>,
        /// Its generation, as written.
        generation: Vec<u8>,
    },
    /// A string, literal or hex, that doesn't end.
    #[error("a string at byte {at} that doesn't end")]
    String {
        /// Where it starts.
        at: usize,
    },
    /// An array that doesn't end.
    #[error("an array at byte {at} that doesn't end")]
    Array {
        /// Where it starts.
        at: usize,
    },
    /// A dictionary that doesn't end.
    #[error("a dictionary at byte {at} that doesn't end")]
    Dict {
        /// Where it starts.
        at: usize,
    },
    /// A dictionary's key that isn't a name.
    #[error("a dictionary's key at byte {at} isn't a name")]
    Key {
        /// Where the key is.
        at: usize,
    },
    /// A keyword other than `true`, `false` and `null` where a value
    /// should be.
    #[error("`{}` at byte {at}, where a value should be", lossy(.word))]
    Keyword {
        /// Where it is.
        at: usize,
        /// Its first 32 bytes.
        word: Vec<u8>,
    },
    /// Another token where a value should be: a `]`, `>>`, `{` or `}`, or a
    /// `)` or `>` that closes nothing.
    #[error("{:?} at byte {at}, where a value should be", lossy(.bytes))]
    Token {
        /// Where it is.
        at: usize,
        /// Its bytes.
        bytes: Vec<u8>,
    },
    /// No `n g obj`, an indirect object's header.
    #[error("no `n g obj` at byte {at}")]
    Header {
        /// Where it was looked for: where the parser was, as it was given
        /// when it had read nothing, past the data's end too, where a
        /// cross-reference can point.
        at: usize,
    },
    /// A stream with no `endstream` after its data.
    #[error("{id} obj's stream has no endstream")]
    Endstream {
        /// The object.
        id: ObjectId,
    },
    /// A stream given data it can't have: bytes that don't start where its
    /// data does, end before they start, run past what the parser reads,
    /// or are in other data.
    #[error(
        "{id} obj's stream can't have bytes {start}..{end} as its data: they don't start where its data does, end before they start, run past what's read, or are in other data"
    )]
    Extent {
        /// The object.
        id: ObjectId,
        /// Where the data given starts.
        start: usize,
        /// Where the data given ends.
        end: usize,
    },
    /// The read reached the end of a window that isn't the file's last
    /// ([`Parser::window`]), where more of the file could change what it
    /// reads: a token reaching it, none before it where one was looked
    /// for, or a stream's data running to it. The caller reads again with
    /// more.
    #[error("the data ends at byte {at}, where more of the file could change what's read")]
    Short {
        /// Where the window ends, in the file.
        at: usize,
    },
}

/// Bytes, lossily as text, for [`Error`]'s text.
fn lossy(bytes: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

/// Reads values, or an indirect object, from data, a file's bytes, a
/// window of them ([`Parser::window`]), or an object stream's decoded
/// data, from an offset on. Its strings are read as their bytes, or as
/// ciphertext ([`Parser::ciphertext`]), and its spans are in the file, or
/// in an object stream's data ([`Parser::in_stream`]). It reads under
/// [`Limits`] ([`Parser::limits`]), by ISO 32000-2's lexical rules unless
/// it's given PDFium's ([`Parser::rules`]). A call that fails leaves the
/// parser where it was, though it has said the faults it read past before
/// the error, all but [`Fault::HighByte`], which only a call that reads
/// its object tells.
///
/// A parser reads nothing until a call asks it to, so one made and
/// dropped is a mistake, which the compiler says:
///
/// ```compile_fail
/// #![deny(unused_must_use)]
/// use pdf_objects::parse::Parser;
///
/// Parser::at(b"1 0 obj 5 endobj", 0);
/// ```
#[must_use = "a parser is lazy: it reads nothing until a value is asked for"]
#[derive(Clone)]
pub struct Parser<'a> {
    data: &'a [u8],
    /// Where it reads next, in `data`: as it was given until a call reads,
    /// past the data's end too.
    offset: usize,
    /// Where `data` starts in the file, which its spans count from.
    base: usize,
    /// Whether `data` ends the file.
    last: bool,
    /// The object stream whose decoded data `data` is, for its spans.
    stream: Option<ObjectId>,
    /// Whether its strings are read as ciphertext.
    ciphertext: bool,
    /// How deeply values may nest, and how many one call may read.
    limits: Limits,
    /// The lexical rules it reads by. Each call reads by a tokenizer with
    /// them fixed in its type, so that it has nothing to choose at each
    /// token.
    rules: Rules,
}

impl fmt::Debug for Parser<'_> {
    /// Where it is, the data's length and how it reads it; not the data,
    /// which may be a whole file, or decrypted.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Parser")
            .field("offset", &self.offset)
            .field("data", &format_args!("{} bytes", self.data.len()))
            .field("base", &self.base)
            .field("last", &self.last)
            .field("stream", &self.stream)
            .field("ciphertext", &self.ciphertext)
            .field("limits", &self.limits)
            .field("rules", &self.rules)
            .finish()
    }
}

impl<'a> Parser<'a> {
    /// A parser of `data`, reading from `offset`. Past the data's end
    /// there's nothing to read, and the offset is kept as it's given, which
    /// [`Error::Header`] names: a cross-reference can point there. The data
    /// is read whole, as a window that starts the file and ends it.
    pub const fn at(data: &'a [u8], offset: usize) -> Parser<'a> {
        Parser {
            data,
            offset,
            base: 0,
            last: true,
            stream: None,
            ciphertext: false,
            limits: Limits::DEFAULT,
            rules: Rules::Iso,
        }
    }

    /// The parser, its data being a window of a file: bytes `base..base +
    /// data.len()`, which its spans, [`offset`](Self::offset) and every
    /// [`Error`]'s place then count in. The offset it was made at counts
    /// in the data, so `Parser::at(window, i).window(base, last)` reads
    /// from byte `base + i` of the file. `last` when the window ends the
    /// file.
    ///
    /// In a window that isn't the file's last, a call that reads a token
    /// reaching the window's end, or finds none where it looks for one, is
    /// [`Error::Short`], whatever else it would have said, since more of
    /// the file could change what it reads; so is [`Parser::stream`] when
    /// its data, or the extents the caller was given
    /// ([`StreamHead::declared`], [`StreamHead::scanned`]), ran to it. Read
    /// again in a window with more, or the file's last, a call reads as it
    /// would with the whole file.
    pub const fn window(self, base: usize, last: bool) -> Parser<'a> {
        Parser { base, last, ..self }
    }

    /// The parser, its data being object stream `stream`'s decoded data,
    /// which its spans are then in ([`Span::stream`]).
    pub const fn in_stream(self, stream: ObjectId) -> Parser<'a> {
        Parser {
            stream: Some(stream),
            ..self
        }
    }

    /// The parser, reading strings as ciphertext ([`Str::Encrypted`]) when
    /// `ciphertext`: an encrypted file's, which its reader doesn't decrypt
    /// as it reads.
    pub const fn ciphertext(self, ciphertext: bool) -> Parser<'a> {
        Parser { ciphertext, ..self }
    }

    /// The parser, reading under `limits`: values nested deeper than its
    /// `nesting`, or more of them in one call than its `values`, are an
    /// error ([`Error::Nested`], [`Error::Values`]).
    pub const fn limits(self, limits: Limits) -> Parser<'a> {
        Parser { limits, ..self }
    }

    /// The parser, reading by `rules`: ISO 32000-2's lexical rules, the
    /// default, or PDFium's, as of chromium/8125 ([`Rules::Pdfium`]). Any
    /// other rules are read as ISO 32000-2's.
    ///
    /// By PDFium's rules, its tokens are cut where PDFium cuts them (0x80
    /// and 0xFF are white-space, a word of digits, signs and points is a
    /// number, and a hex string runs to its `>`: pdf-tokenizer's
    /// [`Rules`]), and each is read as PDFium's object parser reads it
    /// ([`pdf_tokenizer::decode::pdfium`]), held as this parser holds a
    /// value: a number word as the integer or the real PDFium holds, at 64
    /// bits (`--5` the integer 0, `1.2.3` the real 1.2, `4294967296` 0,
    /// `4294967295` itself); a name's `#` before any two bytes as an
    /// escape, a byte that's no hex digit as 0; a literal string's CR as
    /// written; a hex string's digits paired over any other byte; and a
    /// number or a name longer than 256 bytes as its first 256, a name's
    /// `/` among them. Where ISO 32000-2 reads such a token otherwise,
    /// that's a fault ([`Fault::Number`], [`Fault::OutOfRange`],
    /// [`Fault::HexString`], [`Fault::Name`], [`Fault::StringCr`],
    /// [`Fault::Long`]), looked for only when the sink
    /// [wants](Sink::wants) faults; and so is each 0x80 and 0xFF read as
    /// white-space in the object a call reads ([`Fault::HighByte`]), from
    /// its first token to the end of its last, a stream's data aside, once
    /// the call has read it.
    ///
    /// White-space around an object is no object's, so a 0x80 or 0xFF
    /// there isn't told: before a value's first token, and after its last,
    /// which a call may read past looking for what follows (an integer's
    /// `n g R`, or `endobj`); between an indirect object's `obj` and its
    /// value, and before its `endobj`, or after a stream's `endstream`. Nor
    /// is one told by a call that fails, which reads no object; read again
    /// with more of the file ([`Error::Short`]), the call that reads the
    /// object tells it, once. [`Parser::header`] tells nothing, by either
    /// rules.
    ///
    /// A real is held at 64 bits by either rules, the `f64` nearest the
    /// decimal it starts with, so that `0.1` is written back as `0.1`,
    /// where PDFium holds the nearest `f32`, which widened would be written
    /// as `0.10000000149011612`. The `f64` as an `f32` is PDFium's value
    /// but where rounding twice parts from rounding once: where the decimal
    /// is just off a point halfway between two `f32`s and its `f64` lands
    /// on it, as a real just under the halfway point past the greatest
    /// `f32` does, which PDFium holds as the greatest and the `f64` rounds
    /// on to infinity. A caller that needs PDFium's own value reads the
    /// word as PDFium does,
    /// [`pdfium::number(word).float()`](pdf_tokenizer::decode::pdfium::number).
    ///
    /// What the parser makes of its tokens is its own under either: a
    /// reference is two integers and `R`, its number and generation the
    /// integers' values, fitting 32 and 16 bits, and a header's the same;
    /// a dictionary's keys are names; a value is a token that starts one;
    /// and the limits are the same. PDFium's object parser reads more
    /// loosely, a reference from any two number words, say, its generation
    /// unread, which isn't followed here.
    pub const fn rules(self, rules: Rules) -> Parser<'a> {
        Parser { rules, ..self }
    }

    /// Where it reads next, counted as its spans are, from a window's base:
    /// past what it has read, or where it was made at, as given, when it
    /// has read nothing. A call that fails leaves it where it was.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.place(self.offset)
    }

    /// The next value.
    ///
    /// # Errors
    ///
    /// [`Error`] where no value reads from where the parser is, or one
    /// passes its [`Limits`]; and [`Error::Short`] where more of the file
    /// could change what it reads ([`Parser::window`]). The parser is left
    /// where it was.
    pub fn value(&mut self, sink: &mut (impl Sink<'a> + ?Sized)) -> Result<Object, Error> {
        match self.rules {
            Rules::Pdfium => self.value_by(Pdfium, sink),
            _ => self.value_by(Iso, sink),
        }
    }

    /// [`Parser::value`], by `rules`.
    fn value_by<R: RuleSet>(
        &mut self,
        rules: R,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<Object, Error> {
        let mut call = Call::of(self.data);
        let mut t = Tokenizer::at(self.data, self.offset).with_rules(rules);
        let value = self.read_whole(&mut t, &mut call, sink);
        self.settled(&call)?;
        let (value, written) = value?;
        self.told_high(rules, written, sink);
        self.offset = t.offset();
        Ok(value)
    }

    /// An indirect object's header, `n g obj` (ISO 32000-2, 7.3.10): its
    /// number and generation. Its error names where the parser was, as it
    /// was given when it has read nothing.
    ///
    /// It's read by the parser's rules, and tells nothing, having no sink:
    /// by PDFium's, its integers are read as PDFium holds them, so `--5 0
    /// obj` is object 0, which ISO 32000-2 reads as no header, and a 0x80
    /// or 0xFF between its words is white-space, neither said.
    ///
    /// # Errors
    ///
    /// [`Error::Header`] where there's no `n g obj`, naming where the parser
    /// was; [`Error::Short`] where more of the file could change what it
    /// reads ([`Parser::window`]). The parser is left where it was.
    pub fn header(&mut self) -> Result<ObjectId, Error> {
        match self.rules {
            Rules::Pdfium => self.header_by(Pdfium),
            _ => self.header_by(Iso),
        }
    }

    /// [`Parser::header`], by `rules`.
    fn header_by<R: RuleSet>(&mut self, rules: R) -> Result<ObjectId, Error> {
        let offset = self.offset;
        let mut call = Call::of(self.data);
        let mut t = Tokenizer::at(self.data, offset).with_rules(rules);
        let mut header = |t: &mut Tokenizer<'a, R>| {
            let n = call.significant(t).filter(|n| n.kind == Kind::Integer)?;
            let g = call.significant(t).filter(|g| g.kind == Kind::Integer)?;
            let obj = call.significant(t)?;
            let number = u32::try_from(integer(&n, rules.rules())?).ok()?;
            let generation = u16::try_from(integer(&g, rules.rules())?).ok()?;
            is_keyword(Some(&obj), b"obj").then_some(ObjectId::new(number, generation))
        };
        let id = header(&mut t).ok_or(Error::Header {
            at: self.place(offset),
        });
        self.settled(&call)?;
        let id = id?;
        self.offset = t.offset();
        Ok(id)
    }

    /// Object `id`'s value, after its header: a value, said when `endobj`
    /// doesn't follow it; or a dictionary with `stream` after it, read
    /// through `stream`'s end of line, said when it isn't CR LF or LF, to
    /// where its data starts.
    ///
    /// # Errors
    ///
    /// As [`Parser::value`]'s: [`Error`] where no value reads, or one passes
    /// its [`Limits`]; and [`Error::Short`] where more of the file could
    /// change what it reads. The parser is left where it was.
    pub fn body(
        &mut self,
        id: ObjectId,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<Body<'a>, Error> {
        match self.rules {
            Rules::Pdfium => self.body_by(Pdfium, id, sink),
            _ => self.body_by(Iso, id, sink),
        }
    }

    /// [`Parser::body`], by `rules`.
    fn body_by<R: RuleSet>(
        &mut self,
        rules: R,
        id: ObjectId,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<Body<'a>, Error> {
        let mut call = Call::of(self.data);
        let mut t = Tokenizer::at(self.data, self.offset).with_rules(rules);
        let body = self.read_body(&mut t, &mut call, id, sink);
        self.settled(&call)?;
        let (body, written) = body?;
        self.told_high(rules, written, sink);
        self.offset = t.offset();
        Ok(body)
    }

    /// The stream `head` began, its data `data`, which its caller chose:
    /// one of [`StreamHead::declared`] and [`StreamHead::scanned`]; then
    /// `endstream`, and `endobj`, said when it doesn't follow.
    ///
    /// # Errors
    ///
    /// [`Error::Extent`] for data that doesn't start where the stream's
    /// does, ends before it starts, runs past what the parser reads, or is
    /// in other data; [`Error::Endstream`] where `endstream` doesn't follow
    /// it; and [`Error::Short`] where the data, or the look for
    /// `endstream`, reached the end of a window that isn't the file's last
    /// ([`Parser::window`]). The parser is left where it was.
    pub fn stream(
        &mut self,
        head: StreamHead<'a>,
        data: Span,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<Object, Error> {
        match self.rules {
            Rules::Pdfium => self.stream_by(Pdfium, head, data, sink),
            _ => self.stream_by(Iso, head, data, sink),
        }
    }

    /// [`Parser::stream`], by `rules`.
    fn stream_by<R: RuleSet>(
        &mut self,
        rules: R,
        head: StreamHead<'a>,
        data: Span,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<Object, Error> {
        let mut call = Call::of(self.data);
        call.touched = head.touched.load(Ordering::Relaxed);
        let stream = self.read_stream(rules, head, data, &mut call, sink);
        self.settled(&call)?;
        let (object, written) = stream?;
        let end = written.end;
        self.told_high(rules, written, sink);
        self.offset = end;
        Ok(object)
    }

    /// Offset `at` of the data, counted where spans are: from the window's
    /// base, in the file.
    const fn place(&self, at: usize) -> usize {
        self.base.saturating_add(at)
    }

    /// Bytes `start..end` of the data, as a span.
    fn span(&self, start: usize, end: usize) -> Span {
        Span::new(self.stream, self.place(start), self.place(end))
    }

    /// [`Error::Short`] when a call reached the end of a window that isn't
    /// the file's last, whatever it read.
    fn settled(&self, call: &Call) -> Result<(), Error> {
        match call.touched && !self.last {
            true => Err(Error::Short {
                at: self.place(self.data.len()),
            }),
            false => Ok(()),
        }
    }

    /// [`Parser::stream`], by `rules`, and the bytes of the data it read,
    /// from the stream's data's end through `endstream`, where it leaves
    /// the parser.
    fn read_stream<R: RuleSet>(
        &self,
        rules: R,
        head: StreamHead<'a>,
        data: Span,
        call: &mut Call,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<(Object, Range<usize>), Error> {
        let StreamHead {
            id,
            dict,
            dict_at,
            start,
            ..
        } = head;
        let start = self.place(start);
        let extent = Error::Extent {
            id,
            start: data.start,
            end: data.end,
        };
        if data.stream != self.stream || data.start != start || data.start > data.end {
            return Err(extent);
        }
        // Past the window's end, more of the file could hold it.
        let end = data.end - self.base;
        if end > self.data.len() {
            call.touched = true;
            return Err(extent);
        }
        let mut t = Tokenizer::at(self.data, end).with_rules(rules);
        let endstream = call.significant(&mut t);
        let endstream = (endstream.filter(|token| is_keyword(Some(token), b"endstream")))
            .ok_or(Error::Endstream { id })?;
        let from = dict_at.map_or(start, |at| at.start);
        let at = Span::new(self.stream, from, self.place(endstream.end()));
        let object = Object::from(Stream {
            dict,
            data: Data::At(data),
        })
        .at(at);
        self.endobj(&t, call, id, &object, sink);
        Ok((object, end..t.offset()))
    }

    /// [`Parser::body`], from `t`, and the bytes of the data it read, from
    /// the value's first token to the end of its last, or, for a stream,
    /// of its end of line after `stream`.
    fn read_body<R: RuleSet>(
        &self,
        t: &mut Tokenizer<'a, R>,
        call: &mut Call,
        id: ObjectId,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<(Body<'a>, Range<usize>), Error> {
        let (value, written) = self.read_whole(t, call, sink)?;
        let at = value.at;
        let mut after = t.clone();
        let next = call.significant(&mut after);
        let object = match (value.into_value(), next) {
            (Value::Dict(dict), Some(keyword)) if is_keyword(Some(&keyword), b"stream") => {
                *t = after;
                let keyword = self.span(keyword.start, keyword.end());
                let eol = t.eol();
                call.note(t);
                match eol {
                    _ if !sink.wants() => {}
                    None => sink.fault(keyword, Fault::NoEol),
                    Some(eol) if eol.bytes == b"\r" => {
                        sink.fault(self.span(eol.start, eol.end()), Fault::LoneCr);
                    }
                    Some(_) => {}
                }
                let head = StreamHead {
                    id,
                    dict,
                    dict_at: at,
                    keyword,
                    start: t.offset(),
                    data: self.data,
                    base: self.base,
                    stream: self.stream,
                    rules: self.rules,
                    touched: AtomicBool::new(false),
                };
                return Ok((Body::Stream(head), written.start..t.offset()));
            }
            (other, _) => {
                let mut object = Object::new(other);
                object.at = at;
                object
            }
        };
        self.endobj(t, call, id, &object, sink);
        Ok((Body::Object(object), written))
    }

    /// Says object `id`, `object`, has no `endobj` when `t` doesn't go on
    /// with it.
    fn endobj<R: RuleSet>(
        &self,
        t: &Tokenizer<'a, R>,
        call: &mut Call,
        id: ObjectId,
        object: &Object,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) {
        // Looked for whatever the sink, as it may reach a window's end.
        if !is_keyword(call.significant(&mut t.clone()).as_ref(), b"endobj") {
            // Every value the parser reads is placed.
            if let Some(at) = object.at.filter(|_| sink.wants()) {
                sink.fault(at, Fault::NoEndobj { id });
            }
        }
    }

    /// The value `t` goes on with, a call's own, in `call`, read by `t`'s
    /// rules, and the bytes of the data it's written in, from its first
    /// token to the end of its last.
    fn read_whole<R: RuleSet>(
        &self,
        t: &mut Tokenizer<'a, R>,
        call: &mut Call,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<(Object, Range<usize>), Error> {
        let token = call.significant(t).ok_or(Error::End)?;
        let start = token.start;
        let value = self.read_from(token, t, call, sink, 0)?;
        Ok((value, start..t.offset()))
    }

    /// The value `t` goes on with, nested `depth` deep, in `call`, read by
    /// `t`'s rules.
    fn read<R: RuleSet>(
        &self,
        t: &mut Tokenizer<'a, R>,
        call: &mut Call,
        sink: &mut (impl Sink<'a> + ?Sized),
        depth: usize,
    ) -> Result<Object, Error> {
        let token = call.significant(t).ok_or(Error::End)?;
        self.read_from(token, t, call, sink, depth)
    }

    /// The value `token` starts, `t` past it, as [`Parser::read`] reads it:
    /// for an array's element, whose first token the array has looked at
    /// already for its `]`.
    fn read_from<R: RuleSet>(
        &self,
        token: Token<'a>,
        t: &mut Tokenizer<'a, R>,
        call: &mut Call,
        sink: &mut (impl Sink<'a> + ?Sized),
        depth: usize,
    ) -> Result<Object, Error> {
        // Settled where the rules are fixed in `R`, as they are.
        let rules = t.rules();
        let pdfium = rules == Rules::Pdfium;
        // Only a value reaches a limit: a token that can't start one is
        // the error it is, below, as it is without them.
        if starts_value(&token) {
            if depth > self.limits.nesting {
                return Err(Error::Nested {
                    limit: self.limits.nesting,
                    at: self.place(token.start),
                });
            }
            self.count(call, token.start)?;
        }
        let at = self.span(token.start, token.end());
        let value = match token.kind {
            Kind::Integer => {
                let mut look = t.clone();
                let generation = call
                    .significant(&mut look)
                    .filter(|g| g.kind == Kind::Integer);
                // `R` is looked for only after an integer: after any other
                // token there's no reference, whatever follows it.
                let r = generation.and_then(|_| {
                    (call.significant(&mut look)).filter(|r| is_keyword(Some(r), b"R"))
                });
                if let (Some(g), Some(r)) = (generation, r) {
                    let number = integer(&token, rules).and_then(|n| u32::try_from(n).ok());
                    let generation = integer(&g, rules).and_then(|g| u16::try_from(g).ok());
                    let (Some(number), Some(generation)) = (number, generation) else {
                        return Err(Error::Reference {
                            at: self.place(token.start),
                            number: token.bytes.to_vec(),
                            generation: g.bytes.to_vec(),
                        });
                    };
                    if pdfium && sink.wants() {
                        self.told_number(&token, sink);
                        self.told_number(&g, sink);
                    }
                    *t = look;
                    let at = self.span(token.start, r.end());
                    return Ok(Object::new(Value::Ref(ObjectId::new(number, generation))).at(at));
                }
                if pdfium {
                    self.pdfium_number(&token, sink)
                } else {
                    match token.integer() {
                        Ok(n) => Value::Integer(n),
                        Err(_) => {
                            if sink.wants() {
                                sink.fault(at, Fault::IntegerOverflow);
                            }
                            Value::Integer(if token.bytes.starts_with(b"-") {
                                i64::MIN
                            } else {
                                i64::MAX
                            })
                        }
                    }
                }
            }
            Kind::Real if pdfium => self.pdfium_number(&token, sink),
            Kind::Real => match token.number() {
                Ok(r) => Value::Real(r),
                // Too small to hold: zero, with its sign.
                Err(NumberError::Underflow) => {
                    if sink.wants() {
                        sink.fault(at, Fault::RealUnderflow);
                    }
                    Value::Real(if token.bytes.starts_with(b"-") {
                        -0.0
                    } else {
                        0.0
                    })
                }
                Err(_) => {
                    if sink.wants() {
                        sink.fault(at, Fault::RealOverflow);
                    }
                    Value::Real(if token.bytes.starts_with(b"-") {
                        f64::MIN
                    } else {
                        f64::MAX
                    })
                }
            },
            Kind::Name => Value::Name(self.name(&token, pdfium, sink)),
            Kind::LiteralString | Kind::HexString if !token.closed => {
                return Err(Error::String {
                    at: self.place(token.start),
                });
            }
            Kind::LiteralString | Kind::HexString => {
                let form = match token.kind {
                    Kind::HexString => Form::Hex,
                    _ => Form::Literal,
                };
                let bytes = match pdfium {
                    true => self.pdfium_string(&token, sink),
                    false => decoded(&token),
                };
                let s = match self.ciphertext {
                    true => Str::Encrypted(bytes),
                    false => Str::Bytes(bytes),
                };
                Value::String(s, Some(form))
            }
            Kind::ArrayOpen => {
                let mut array = Vec::new();
                loop {
                    let mut look = t.clone();
                    match call.significant(&mut look) {
                        Some(close) if close.kind == Kind::ArrayClose => {
                            *t = look;
                            let at = self.span(token.start, close.end());
                            return Ok(Object::new(Value::Array(array)).at(at));
                        }
                        Some(element) => {
                            *t = look;
                            array.push(self.read_from(element, t, call, sink, depth + 1)?);
                        }
                        None => {
                            return Err(Error::Array {
                                at: self.place(token.start),
                            });
                        }
                    }
                }
            }
            Kind::DictOpen => {
                let mut dict = Dict::new();
                loop {
                    let key = call.significant(t).ok_or(Error::Dict {
                        at: self.place(token.start),
                    })?;
                    match key.kind {
                        Kind::DictClose => {
                            let at = self.span(token.start, key.end());
                            return Ok(Object::new(Value::Dict(dict)).at(at));
                        }
                        Kind::Name => {
                            self.count(call, key.start)?;
                            let value = self.read(t, call, sink, depth + 1)?;
                            dict.push_entry(Entry {
                                key: self.name(&key, pdfium, sink),
                                key_at: Some(self.span(key.start, key.end())),
                                value,
                            });
                        }
                        _ => {
                            return Err(Error::Key {
                                at: self.place(key.start),
                            });
                        }
                    }
                }
            }
            Kind::Keyword => match token.bytes {
                b"true" => Value::Bool(true),
                b"false" => Value::Bool(false),
                b"null" => Value::Null,
                word => {
                    return Err(Error::Keyword {
                        at: self.place(token.start),
                        word: word[..word.len().min(32)].to_vec(),
                    });
                }
            },
            _ => {
                return Err(Error::Token {
                    at: self.place(token.start),
                    bytes: token.bytes.to_vec(),
                });
            }
        };
        Ok(Object::new(value).at(at))
    }

    /// One more value read in `call`, starting at offset `at` of the data:
    /// an error past the limit.
    fn count(&self, call: &mut Call, at: usize) -> Result<(), Error> {
        call.values += 1;
        match call.values > self.limits.values {
            true => Err(Error::Values {
                limit: self.limits.values,
                at: self.place(at),
            }),
            false => Ok(()),
        }
    }

    /// Tells `sink` of each 0x80 and 0xFF in the white-space of bytes
    /// `written` of the data, the object a call has read, by `rules`: where
    /// they're PDFium's, which read both bytes as white-space where ISO
    /// 32000-2 reads them as regular characters ([`Fault::HighByte`]), and
    /// the sink wants faults. Its tokens are read again from its first only
    /// where `written` holds either byte; a string's bytes and a comment's
    /// are their own.
    fn told_high<R: RuleSet>(
        &self,
        rules: R,
        written: Range<usize>,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) {
        // Settled where the rules are fixed in `R`, as they are.
        if rules.rules() != Rules::Pdfium || !sink.wants() {
            return;
        }
        let data = self.data;
        let end = written.end.min(data.len());
        if data
            .get(written.start..end)
            .is_none_or(|bytes| memchr2(0x80, 0xFF, bytes).is_none())
        {
            return;
        }
        let mut t = Tokenizer::at(data, written.start).with_rules(rules);
        while t.offset() < end {
            let Some(token) = t.next() else {
                return;
            };
            if token.kind != Kind::Space {
                continue;
            }
            let space = &data[token.start..token.end().min(end)];
            for i in memchr2_iter(0x80, 0xFF, space) {
                let at = token.start + i;
                let fault = Fault::HighByte {
                    written: &data[at..=at],
                };
                sink.fault(self.span(at, at + 1), fault);
            }
        }
    }

    /// A name's bytes, its escapes decoded, by PDFium's rules when
    /// `pdfium`; and a fault where it is when it has a `#` without two hex
    /// digits after it, or `#00`, NUL, which a name can't hold (ISO
    /// 32000-2, 7.3.5), readers reading each their own way.
    fn name(
        &self,
        token: &Token<'a>,
        pdfium: bool,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Vec<u8> {
        if pdfium {
            return self.pdfium_name(token, sink);
        }
        // Only a `#` starts an escape or a fault: without one, a name is the
        // regular bytes after its `/`, as written.
        if !token.bytes.contains(&b'#') {
            return token.bytes.get(1..).unwrap_or_default().to_vec();
        }
        if sink.wants() {
            self.told_name(token, sink);
        }
        decoded(token)
    }

    /// Tells `sink` of name `token`'s fault, if it has one: a `#` without
    /// two hex digits after it, or `#00`.
    fn told_name(&self, token: &Token<'a>, sink: &mut (impl Sink<'a> + ?Sized)) {
        let quirks = token.pieces().map_or(Quirks::default(), Pieces::quirks);
        let (stray, nul) = (
            quirks.contains(How::StrayHash),
            quirks.contains(How::NulEscape),
        );
        if stray || nul {
            let fault = Fault::Name {
                written: token.bytes,
                stray,
                nul,
            };
            sink.fault(self.span(token.start, token.end()), fault);
        }
    }

    /// A name's bytes as PDFium's object parser reads them
    /// ([`pdfium::name`]), from its first 256 bytes, its `/` among them;
    /// and its faults: a `#` ISO 32000-2 reads otherwise, or `#00`
    /// ([`Fault::Name`]), and its length past 256 ([`Fault::Long`]).
    fn pdfium_name(&self, token: &Token<'a>, sink: &mut (impl Sink<'a> + ?Sized)) -> Vec<u8> {
        if sink.wants() {
            if token.bytes.contains(&b'#') {
                self.told_name(token, sink);
            }
            self.told_long(token, sink);
        }
        let word = kept(token.bytes);
        if !word.contains(&b'#') {
            return word.get(1..).unwrap_or_default().to_vec();
        }
        reserved(pdfium::name(word))
    }

    /// A number token's value as PDFium's object parser reads it
    /// ([`pdfium_number`]), and its faults where ISO 32000-2 reads it
    /// otherwise ([`told_number`](Self::told_number)).
    fn pdfium_number(&self, token: &Token<'a>, sink: &mut (impl Sink<'a> + ?Sized)) -> Value {
        if sink.wants() {
            self.told_number(token, sink);
        }
        pdfium_number(token.bytes)
    }

    /// Tells `sink` where ISO 32000-2 reads number token `token`, read by
    /// PDFium's rules, otherwise: as no number ([`Fault::Number`]); as an
    /// integer outside the 32 bits PDFium reads, which it reads as 0
    /// ([`Fault::OutOfRange`]); or as every byte of a word longer than the
    /// 256 PDFium reads ([`Fault::Long`]). A real ISO 32000-2 reads is
    /// the same decimal to both, read at 64 bits.
    fn told_number(&self, token: &Token<'a>, sink: &mut (impl Sink<'a> + ?Sized)) {
        let written = token.bytes;
        let fault = match decode::integer(written) {
            // Unsigned or signed, by PDFium's reading of the whole word.
            Ok(n) => {
                (wide(pdfium::number(written)) != Some(n)).then_some(Fault::OutOfRange { written })
            }
            Err(NumberError::Overflow) => Some(Fault::OutOfRange { written }),
            // An integer can't underflow.
            Err(NumberError::Underflow) => None,
            Err(NumberError::NotANumber) => {
                matches!(decode::number(written), Err(NumberError::NotANumber))
                    .then_some(Fault::Number { written })
            }
        };
        if let Some(fault) = fault {
            sink.fault(self.span(token.start, token.end()), fault);
        }
        self.told_long(token, sink);
    }

    /// Tells `sink` when word `token` is longer than the 256 bytes
    /// PDFium's object parser reads of one ([`Fault::Long`]).
    fn told_long(&self, token: &Token<'a>, sink: &mut (impl Sink<'a> + ?Sized)) {
        if token.bytes.len() > WORD {
            let fault = Fault::Long {
                written: token.bytes,
            };
            sink.fault(self.span(token.start, token.end()), fault);
        }
    }

    /// A closed string's bytes as PDFium's object parser reads them
    /// ([`pdfium::literal`], [`pdfium::hex`]), and its fault where ISO
    /// 32000-2 reads them otherwise: a hex string with a byte that's
    /// neither a hex digit nor white-space ([`Fault::HexString`]), a
    /// literal string with a CR written in it ([`Fault::StringCr`]).
    fn pdfium_string(&self, token: &Token<'a>, sink: &mut (impl Sink<'a> + ?Sized)) -> Vec<u8> {
        // Nothing to decode: read alike by both.
        if let Some(value) = token.verbatim() {
            return value.to_vec();
        }
        let bytes = match token.kind {
            Kind::HexString => pdfium::hex(token.bytes),
            _ => pdfium::literal(token.bytes),
        };
        if sink.wants() {
            self.told_string(token, bytes.clone(), sink);
        }
        reserved(bytes)
    }

    /// Tells `sink` where ISO 32000-2 reads closed string `token`, whose
    /// bytes PDFium's object parser reads as `bytes`, otherwise.
    fn told_string(
        &self,
        token: &Token<'a>,
        bytes: pdfium::Bytes<'_>,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) {
        let written = token.bytes;
        let fault = match token.kind {
            Kind::HexString => {
                // Between its `<` and its `>`.
                let inner = written.get(1..written.len().saturating_sub(1));
                let stray = |b: &u8| !b.is_ascii_hexdigit() && !pdf_tokenizer::is_white_space(*b);
                let stray = inner.unwrap_or_default().iter().any(stray);
                stray.then_some(Fault::HexString { written })
            }
            // Only a CR written in it reads otherwise, PDFium keeping it
            // where ISO 32000-2 reads LF; every escape reads alike.
            _ => {
                let cr = written.contains(&b'\r')
                    && token.pieces().is_some_and(|iso| !bytes.eq(iso.bytes()));
                cr.then_some(Fault::StringCr { written })
            }
        };
        if let Some(fault) = fault {
            sink.fault(self.span(token.start, token.end()), fault);
        }
    }
}

/// The most of a word PDFium's object parser keeps: a number or a name
/// longer than this, its `/` counted, is read as its first `WORD` bytes.
const WORD: usize = 256;

/// The bytes of `word` PDFium's object parser keeps: its first [`WORD`].
fn kept(word: &[u8]) -> &[u8] {
    &word[..word.len().min(WORD)]
}

/// A number word's value as PDFium's object parser reads it, held as this
/// parser holds a value, at 64 bits: from its first [`WORD`] bytes, the
/// integer PDFium holds (unsigned or signed in 32 bits, 0 outside them),
/// or with a point among them the real, the decimal they start with
/// ([`pdfium::real`]). 256 bytes hold no decimal past an `f64`'s range, so
/// the real is finite.
fn pdfium_number(word: &[u8]) -> Value {
    let word = kept(word);
    match pdfium::number(word) {
        Number::Unsigned(n) => Value::Integer(n.into()),
        Number::Signed(n) => Value::Integer(n.into()),
        Number::Real(_) => Value::Real(pdfium::real(word)),
    }
}

/// An integer PDFium holds, at 64 bits; none for a real.
fn wide(number: Number) -> Option<i64> {
    match number {
        Number::Unsigned(n) => Some(n.into()),
        Number::Signed(n) => Some(n.into()),
        Number::Real(_) => None,
    }
}

/// The value of [`Integer`](Kind::Integer) token `token`, a reference's
/// number or generation or a header's, read by `rules`: by ISO 32000-2's,
/// none past 64 bits; by PDFium's, its integer reading
/// ([`pdfium_number`]).
fn integer(token: &Token, rules: Rules) -> Option<i64> {
    match rules {
        Rules::Pdfium => match pdfium_number(token.bytes) {
            Value::Integer(n) => Some(n),
            _ => None,
        },
        _ => token.integer().ok(),
    }
}

/// A PDFium reading's bytes, in a buffer reserved once for as many as the
/// bytes left to read, which a value is never longer than, so it never
/// grows; its capacity may exceed its length by what the escapes took.
fn reserved(bytes: pdfium::Bytes<'_>) -> Vec<u8> {
    let mut value = Vec::with_capacity(bytes.size_hint().1.unwrap_or(0));
    value.extend(bytes);
    value
}

/// An indirect object's value, read after its header.
///
/// Exhaustive on purpose: an indirect object's value is a stream or it
/// isn't (ISO 32000-2, 7.3.8 and 7.3.10), so a match on both is checked by
/// the compiler.
#[derive(Debug, Clone)]
pub enum Body<'a> {
    /// A value that isn't a stream.
    Object(Object),
    /// A stream, read to where its data starts.
    Stream(StreamHead<'a>),
}

/// A stream read to where its data starts: its dictionary, and both
/// extents its data may have, neither resolved nor chosen.
///
/// It's one parse's, borrowing the data the parser reads, and it remembers
/// when an extent ran to the end of a window ([`Parser::window`]) for
/// [`Parser::stream`].
#[must_use = "a stream's head is read to its data: the rest is read by Parser::stream"]
pub struct StreamHead<'a> {
    id: ObjectId,
    dict: Dict,
    dict_at: Option<Span>,
    keyword: Span,
    /// Where its data starts, in `data`.
    start: usize,
    /// What the parser reads, for the extents.
    data: &'a [u8],
    /// Where `data` starts in the file.
    base: usize,
    stream: Option<ObjectId>,
    /// The rules its parser reads by, which find `endstream` after data
    /// by a `/Length`.
    rules: Rules,
    /// Whether an extent ran to `data`'s end, where more of the file could
    /// change it: an atomic, so that a head can be shared, though nothing
    /// orders it with anything else.
    touched: AtomicBool,
}

impl fmt::Debug for StreamHead<'_> {
    /// Its object, where its data starts and the length of what its parser
    /// reads; not what it reads, which may be a whole file, or decrypted.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamHead")
            .field("id", &self.id)
            .field("start", &self.start())
            .field("data", &format_args!("{} bytes", self.data.len()))
            .finish_non_exhaustive()
    }
}

impl Clone for StreamHead<'_> {
    fn clone(&self) -> Self {
        StreamHead {
            id: self.id,
            dict: self.dict.clone(),
            dict_at: self.dict_at,
            keyword: self.keyword,
            start: self.start,
            data: self.data,
            base: self.base,
            stream: self.stream,
            rules: self.rules,
            touched: AtomicBool::new(self.touched.load(Ordering::Relaxed)),
        }
    }
}

impl StreamHead<'_> {
    /// Its dictionary.
    #[must_use]
    pub const fn dict(&self) -> &Dict {
        &self.dict
    }

    /// Where its `stream` keyword was written.
    #[must_use]
    pub const fn keyword(&self) -> Span {
        self.keyword
    }

    /// Where its data starts: after `stream` and its end of line, counted
    /// as its spans are.
    #[must_use]
    pub const fn start(&self) -> usize {
        self.base.saturating_add(self.start)
    }

    /// Its data by a `/Length` of `length`: that many bytes from where it
    /// starts, when `endstream` follows them, past white-space (and
    /// comments) by its parser's rules; else none.
    #[must_use]
    pub fn declared(&self, length: usize) -> Option<Span> {
        let end = self.start.checked_add(length)?;
        if end > self.data.len() {
            self.touched.store(true, Ordering::Relaxed);
            return None;
        }
        let mut call = Call::of(self.data);
        let after = match self.rules {
            Rules::Pdfium => {
                call.significant(&mut Tokenizer::at(self.data, end).with_rules(Pdfium))
            }
            _ => call.significant(&mut Tokenizer::at(self.data, end)),
        };
        if call.touched {
            self.touched.store(true, Ordering::Relaxed);
        }
        is_keyword(after.as_ref(), b"endstream").then(|| self.span(self.start, end))
    }

    /// Its data up to the first `endstream`, found by scanning, wherever
    /// it is; or to the end of what the parser reads, when none comes.
    #[must_use]
    pub fn scanned(&self) -> Span {
        let data = Tokenizer::at(self.data, self.start).data_until(b"endstream");
        if data.end() == self.data.len() {
            self.touched.store(true, Ordering::Relaxed);
        }
        self.span(data.start, data.end())
    }

    /// Bytes `start..end` of the data, as a span.
    fn span(&self, start: usize, end: usize) -> Span {
        let place = |at: usize| self.base.saturating_add(at);
        Span::new(self.stream, place(start), place(end))
    }
}

/// What one call has read: how many values, for [`Limits::values`], and
/// whether it reached the end of its data, where in a window more of the
/// file could change what it reads (a token that reaches it, or none where
/// one was looked for). Each call makes its own, passed to what it calls,
/// so that a parser keeps nothing of it, and stays `Sync`.
struct Call {
    values: usize,
    /// The data's length.
    end: usize,
    touched: bool,
}

impl Call {
    /// None yet, in `data`.
    fn of(data: &[u8]) -> Call {
        Call {
            values: 0,
            end: data.len(),
            touched: false,
        }
    }

    /// Notes whether `t`, over the data, has reached its end.
    fn note<R: RuleSet>(&mut self, t: &Tokenizer<'_, R>) {
        self.touched |= t.offset() >= self.end;
    }

    /// The next token that isn't white-space or a comment, noting when it
    /// reaches the data's end, or none comes before it.
    fn significant<'t, R: RuleSet>(&mut self, t: &mut Tokenizer<'t, R>) -> Option<Token<'t>> {
        let token = t.next_significant();
        self.note(t);
        token
    }
}

/// Whether `token` starts a value: a number, a name, a string that ends,
/// `[`, `<<`, `true`, `false` or `null`.
// Asked of each value: inlined into `read`, which is compiled for each set
// of rules now, and left a call without it.
#[inline]
fn starts_value(token: &Token) -> bool {
    match token.kind {
        Kind::Integer | Kind::Real | Kind::Name | Kind::ArrayOpen | Kind::DictOpen => true,
        Kind::LiteralString | Kind::HexString => token.closed,
        Kind::Keyword => matches!(token.bytes, b"true" | b"false" | b"null"),
        _ => false,
    }
}

fn is_keyword(token: Option<&Token>, word: &[u8]) -> bool {
    token.is_some_and(|t| t.is_keyword(word))
}

/// A name's or string's value, its escapes decoded, in a buffer reserved
/// once for as many bytes as the token has, which a value is never longer
/// than, so it never grows; its capacity may exceed its length by what the
/// escapes took. Nothing for any other token.
fn decoded(token: &Token) -> Vec<u8> {
    if let Some(value) = token.verbatim() {
        return value.to_vec();
    }
    let Some(pieces) = token.pieces() else {
        return Vec::new();
    };
    let bytes = pieces.bytes();
    let mut value = Vec::with_capacity(bytes.size_hint().1.unwrap_or(0));
    value.extend(bytes);
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuzz::Kept;

    /// `input` read as a value from its start: the value or the error, and
    /// what the sink was told.
    fn value(input: &[u8]) -> (Result<Object, Error>, Kept) {
        let mut kept = Kept::default();
        let value = Parser::at(input, 0).value(&mut kept);
        (value, kept)
    }

    /// The indirect object `input` begins with, its stream's data the
    /// extent `choose` picks: the object or the error, and what the sink
    /// was told.
    fn indirect<'a>(
        input: &'a [u8],
        choose: impl FnOnce(&StreamHead<'a>) -> Span,
    ) -> (Result<Object, Error>, Kept) {
        let mut kept = Kept::default();
        let mut parser = Parser::at(input, 0);
        let object = parser
            .header()
            .and_then(|id| match parser.body(id, &mut kept)? {
                Body::Object(object) => Ok(object),
                Body::Stream(head) => {
                    let data = choose(&head);
                    parser.stream(head, data, &mut kept)
                }
            });
        (object, kept)
    }

    /// `Debug` of a parser, and of a stream read to where its data starts,
    /// says where it is and how much it reads, never the data, which may be
    /// a whole file, or decrypted.
    #[test]
    fn debug_says_where_it_is_not_the_data() {
        let mut file = b"4 0 obj\n<< /Length 5 >>\nstream\nhello\nendstream\nendobj\n".to_vec();
        file.resize(4000, b' ');
        let mut parser = Parser::at(&file, 0);
        let limits = Limits::DEFAULT;
        let shown = format!(
            "Parser {{ offset: 0, data: 4000 bytes, base: 0, last: true, stream: None, \
             ciphertext: false, limits: {limits:?}, rules: Iso }}"
        );
        assert_eq!(format!("{parser:?}"), shown);
        let id = parser.header().unwrap();
        let Ok(Body::Stream(head)) = parser.body(id, &mut ()) else {
            panic!("a stream");
        };
        let shown = "StreamHead { id: ObjectId { number: 4, generation: 0 }, start: 31, \
                     data: 4000 bytes, .. }";
        assert_eq!(format!("{head:?}"), shown);
        let shown = format!("{:?}", Body::Stream(head));
        assert!(shown.starts_with("Stream(StreamHead { id: ") && !shown.contains("hello"));

        // In a window of an object stream's data, by PDFium's rules.
        let parser = Parser::at(b"<< /A (secret) >>", 3)
            .window(100, false)
            .in_stream(ObjectId::new(7, 0))
            .ciphertext(true)
            .rules(Rules::Pdfium);
        let shown = format!(
            "Parser {{ offset: 3, data: 17 bytes, base: 100, last: false, \
             stream: Some(ObjectId {{ number: 7, generation: 0 }}), ciphertext: true, \
             limits: {limits:?}, rules: Pdfium }}"
        );
        assert_eq!(format!("{parser:?}"), shown);
    }

    /// The bytes `at` is in `input`.
    fn bytes(input: &[u8], at: Span) -> &[u8] {
        &input[at.start..at.end]
    }

    /// The span of `needle`'s first occurrence in `input`.
    fn found(input: &[u8], needle: &[u8]) -> Span {
        let start = (input.windows(needle.len()))
            .position(|w| w == needle)
            .unwrap();
        Span::new(None, start, start + needle.len())
    }

    /// Each fault says what pdf-lint's reader said, word for word, where
    /// the parser read past it.
    #[test]
    fn each_fault_says_what_it_read_past() {
        let long = format!("/{}#z", "a".repeat(50));
        let cases: [(&[u8], &str, &str); 6] = [
            (
                b"99999999999999999999",
                "99999999999999999999",
                "an integer outside 64 bits, read as the nearest that fits",
            ),
            (
                b"/Ca#zt",
                "/Ca#zt",
                "the name /Ca#zt has a # without two hex digits after it, which readers read differently",
            ),
            (
                b"/N#00",
                "/N#00",
                "the name /N#00 has #00, NUL, which a name can't hold, which readers read differently",
            ),
            (
                b"/A#zz#00",
                "/A#zz#00",
                "the name /A#zz#00 has a # without two hex digits after it, and #00, NUL, which readers read differently",
            ),
            (
                long.as_bytes(),
                &long,
                "the name /aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa has a # without two hex digits after it, which readers read differently",
            ),
            (b"[ 1 ]", "", ""),
        ];
        for (input, written, said) in cases {
            let (value, kept) = value(input);
            assert!(value.is_ok(), "{value:?}");
            let want: Vec<(Span, String)> = match said {
                "" => Vec::new(),
                said => vec![(found(input, written.as_bytes()), said.to_string())],
            };
            assert_eq!(kept.faults, want);
        }
        let real = format!("-{}.5", "9".repeat(320));
        let (value, kept) = self::value(real.as_bytes());
        assert_eq!(*value.unwrap().value(), Value::Real(f64::MIN));
        assert_eq!(
            kept.faults,
            [(
                Span::new(None, 0, real.len()),
                "a real too large for 64 bits, read as the largest".to_string()
            )]
        );
        let (value, _) = self::value(b"-99999999999999999999");
        assert_eq!(*value.unwrap().value(), Value::Integer(i64::MIN));
        // A real too small to hold reads as zero, with its sign, to the
        // bit, and is said.
        for (sign, zero) in [("", 0.0f64), ("-", -0.0), ("+", 0.0)] {
            let tiny = format!("{sign}0.{}1", "0".repeat(400));
            let (value, kept) = self::value(tiny.as_bytes());
            let value = value.unwrap();
            let read = match value.value() {
                Value::Real(read) => *read,
                other => panic!("{other:?}"),
            };
            assert_eq!(read.to_bits(), zero.to_bits(), "{tiny}");
            assert_eq!(
                kept.faults,
                [(
                    Span::new(None, 0, tiny.len()),
                    "a real too small for 64 bits, read as 0".to_string()
                )],
                "{tiny}"
            );
        }
        // The name reads as written but for its well-formed escapes.
        let (value, _) = self::value(b"/A#zz#00#41");
        assert_eq!(*value.unwrap().value(), Value::Name(b"A#zz\0A".to_vec()));

        let faults = |input: &[u8]| {
            let (object, kept) = indirect(input, StreamHead::scanned);
            assert!(object.is_ok(), "{object:?}");
            let said: Vec<String> = kept.faults.into_iter().map(|(_, s)| s).collect();
            said
        };
        assert_eq!(faults(b"5 0 obj << >> stray"), ["5 0 obj has no endobj"]);
        assert_eq!(
            faults(b"1 0 obj << >>stream xy\nendstream\nendobj"),
            ["no end of line after `stream`"]
        );
        assert_eq!(
            faults(b"1 0 obj << >>stream\rxy\nendstream\nendobj"),
            ["a lone CR after `stream`, where ISO 32000-2 wants CR LF or LF"]
        );
        assert_eq!(
            faults(b"1 0 obj << >>stream\nxy\nendstream"),
            ["1 0 obj has no endobj"]
        );
        assert!(faults(b"1 0 obj << >>stream\r\nxy\nendstream endobj").is_empty());
    }

    /// A name without `#` reads as its pieces decode it, its bytes after
    /// the `/`, with no fault: each regular byte but `#` alone, all of them
    /// together, and the empty name, as a value and as a key.
    #[test]
    fn a_name_without_a_hash_is_its_bytes_as_written() {
        let regular: Vec<u8> = (0..=u8::MAX)
            .filter(|&b| pdf_tokenizer::is_regular(b) && b != b'#')
            .collect();
        let mut names: Vec<Vec<u8>> = regular.iter().map(|&b| vec![b'/', b]).collect();
        names.push(b"/".to_vec());
        names.push([&b"/"[..], &regular].concat());
        for written in names {
            let pieces = pdf_tokenizer::decode::name(&written);
            assert!(pieces.clone().quirks().is_empty(), "{written:?}");
            let meant: Vec<u8> = pieces.bytes().collect();
            assert_eq!(meant, written[1..], "{written:?}");
            let (value, kept) = self::value(&written);
            assert_eq!(*value.unwrap().value(), Value::Name(meant.clone()));
            assert!(kept.faults.is_empty(), "{written:?}");
            let (dict, kept) = self::value(&[&b"<<"[..], &written, b" 1>>"].concat());
            let dict = dict.unwrap();
            let keys: Vec<&[u8]> = dict.as_dict().unwrap().iter().map(|(k, _)| k).collect();
            assert_eq!(keys, [&meant[..]], "{written:?}");
            assert!(kept.faults.is_empty(), "{written:?}");
        }
    }

    /// Each error says what pdf-lint's reader said, word for word.
    #[test]
    fn each_error_says_what_stopped_it() {
        let deep = format!("{}{}", "[".repeat(300), "]".repeat(300));
        let keyword = format!("x{}", "y".repeat(40));
        let cases: [(&[u8], &str); 13] = [
            (
                deep.as_bytes(),
                "arrays and dictionaries nested deeper than 256",
            ),
            (b"", "the data ends where a value should be"),
            (b" % a comment", "the data ends where a value should be"),
            (
                b"<< /Pg 18446744073284681740 0 R >>",
                "a reference at byte 7, 18446744073284681740 0 R, numbers no object",
            ),
            (
                b"[1 70000 R]",
                "a reference at byte 1, 1 70000 R, numbers no object",
            ),
            (b"  (open", "a string at byte 2 that doesn't end"),
            (b"<41 42", "a string at byte 0 that doesn't end"),
            (b"[1 2", "an array at byte 0 that doesn't end"),
            (
                b"<< /Type /Catalog",
                "a dictionary at byte 0 that doesn't end",
            ),
            (b"<< 3 /X >>", "a dictionary's key at byte 3 isn't a name"),
            (b"[endobj]", "`endobj` at byte 1, where a value should be"),
            (
                keyword.as_bytes(),
                "`xyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy` at byte 0, where a value should be",
            ),
            (b">>", "\">>\" at byte 0, where a value should be"),
        ];
        for (input, said) in cases {
            let error = value(input).0.unwrap_err();
            assert_eq!(
                error.to_string(),
                said,
                "{}",
                String::from_utf8_lossy(input)
            );
        }
        let error = |input: &[u8]| indirect(input, StreamHead::scanned).0.unwrap_err();
        assert_eq!(error(b"1 0 R").to_string(), "no `n g obj` at byte 0");
        assert_eq!(
            error(b"1 0 obj << >> stream\nabc").to_string(),
            "1 0 obj's stream has no endstream"
        );
        // Data a stream can't have: after where its data starts, ending
        // before it starts, past the input, and in another stream's data.
        let input = b"7 2 obj << >> stream\nabc endstream";
        let astray: [fn(&StreamHead) -> Span; 4] = [
            |head| Span::new(None, head.start() + 1, head.start() + 2),
            |head| Span::new(None, head.start(), head.start() - 1),
            |head| Span::new(None, head.start(), head.start() + 99),
            |head| Span::new(Some(ObjectId::new(9, 0)), head.start(), head.start() + 3),
        ];
        let said: Vec<String> = (astray.into_iter())
            .map(|astray| indirect(input, astray).0.unwrap_err().to_string())
            .collect();
        let why = "they don't start where its data does, end before they start, run past what's read, or are in other data";
        assert_eq!(
            said,
            ["22..23", "21..20", "21..120", "21..24"].map(|bytes| format!(
                "7 2 obj's stream can't have bytes {bytes} as its data: {why}"
            ))
        );
    }

    /// A header the parser was given an offset for past the data's end, as
    /// a cross-reference can give, names that offset, not the end.
    #[test]
    fn a_header_past_the_end_names_the_offset_it_was_given() {
        let data = b"1 0 obj 7 endobj";
        let mut parser = Parser::at(data, 10_000);
        assert_eq!(parser.offset(), 10_000);
        let error = parser.header().unwrap_err();
        assert_eq!(error, Error::Header { at: 10_000 });
        assert_eq!(error.to_string(), "no `n g obj` at byte 10000");
        assert_eq!(Parser::at(data, 10_000).value(&mut ()), Err(Error::End));
        // A call that fails leaves the offset where it was, here as given,
        // so the next error names it too.
        assert_eq!(parser.offset(), 10_000);
        assert_eq!(parser.header(), Err(Error::Header { at: 10_000 }));
        assert_eq!(parser.value(&mut ()), Err(Error::End));
        assert_eq!(parser.offset(), 10_000);
        // From its start, it reads.
        let mut parser = Parser::at(data, 0);
        assert_eq!(parser.header(), Ok(ObjectId::new(1, 0)));
        assert_eq!(parser.offset(), 7);
    }

    /// A call that fails, having read some of the data, leaves the parser
    /// where it was, and one that reads moves it on.
    #[test]
    fn a_call_that_fails_leaves_the_parser_where_it_was() {
        let data = b"1 0 R [1 2 3 /x";
        let mut parser = Parser::at(data, 0);
        assert_eq!(parser.header(), Err(Error::Header { at: 0 }));
        assert_eq!(parser.offset(), 0);
        assert_eq!(
            *parser.value(&mut ()).unwrap().value(),
            Value::Ref(ObjectId::new(1, 0))
        );
        assert_eq!(parser.offset(), 5);
        assert_eq!(parser.value(&mut ()), Err(Error::Array { at: 6 }));
        assert_eq!(parser.offset(), 5);
        // A stream with no `endstream`, after its body read.
        let data = b"1 0 obj << >> stream\nabc";
        let mut parser = Parser::at(data, 0);
        let id = parser.header().unwrap();
        let Ok(Body::Stream(head)) = parser.body(id, &mut ()) else {
            panic!("a stream");
        };
        let start = parser.offset();
        let data = head.scanned();
        assert_eq!(
            parser.stream(head, data, &mut ()),
            Err(Error::Endstream { id })
        );
        assert_eq!(parser.offset(), start);
    }

    /// A sink can keep each fault as the parser gave it, borrowing the
    /// data, and a sink borrowed, `dyn` too, is one.
    #[test]
    fn a_sink_keeps_faults_as_they_are_or_through_a_borrow() {
        struct Typed<'a>(Vec<Fault<'a>>);
        impl<'a> Sink<'a> for Typed<'a> {
            fn fault(&mut self, _: Span, fault: Fault<'a>) {
                self.0.push(fault);
            }
        }
        let data = b"[/A#zz 99999999999999999999]".to_vec();
        let mut typed = Typed(Vec::new());
        Parser::at(&data, 0).value(&mut typed).unwrap();
        let written = &data[1..6];
        let name = Fault::Name {
            written,
            stray: true,
            nul: false,
        };
        assert_eq!(typed.0, [name, Fault::IntegerOverflow]);
        let mut kept = Kept::default();
        let sink: &mut dyn Sink = &mut kept;
        Parser::at(&data, 0).value(sink).unwrap();
        Parser::at(&data, 0).value(&mut &mut *sink).unwrap();
        assert_eq!(kept.faults.len(), 4);
    }

    /// Every value, at every depth, and every key, is placed in the file,
    /// or in an object stream's decoded data.
    #[test]
    fn values_are_placed_in_the_file_or_an_object_streams_data() {
        let input = b"%x\n<< /A [1 (x) /N] /B 2 0 R /C << /D true >> >> 9";
        for stream in [None, Some(ObjectId::new(12, 0))] {
            let mut parser = Parser::at(input, 3);
            if let Some(stream) = stream {
                parser = parser.in_stream(stream);
            }
            let o = parser.value(&mut ()).unwrap();
            assert_eq!(parser.offset(), input.len() - 2);
            let at = o.at.unwrap();
            assert_eq!(at.stream, stream);
            assert_eq!(bytes(input, at), &input[3..input.len() - 2]);
            let d = o.as_dict().unwrap();
            let placed = |key: &[u8]| {
                let entry = d.entry(key).unwrap();
                let (key_at, at) = (entry.key_at.unwrap(), entry.value.at.unwrap());
                assert_eq!((key_at.stream, at.stream), (stream, stream));
                (bytes(input, key_at), bytes(input, at))
            };
            assert_eq!(placed(b"A"), (&b"/A"[..], &b"[1 (x) /N]"[..]));
            assert_eq!(placed(b"B"), (&b"/B"[..], &b"2 0 R"[..]));
            assert_eq!(placed(b"C"), (&b"/C"[..], &b"<< /D true >>"[..]));
            let Value::Array(a) = d.get(b"A").unwrap().value() else {
                panic!("an array");
            };
            let each: Vec<&[u8]> = a.iter().map(|e| bytes(input, e.at.unwrap())).collect();
            assert_eq!(each, [&b"1"[..], b"(x)", b"/N"]);
            // The value after it, from where it ended.
            assert_eq!(*parser.value(&mut ()).unwrap().value(), Value::Integer(9));
        }
    }

    /// Each string keeps the form it was written in, hex or literal, at any
    /// depth.
    #[test]
    fn strings_keep_their_form() {
        let (value, kept) = value(b"[<4142> (AB) < 41 42 > [<>]]");
        let Value::Array(a) = value.unwrap().into_value() else {
            panic!("an array");
        };
        let string = |bytes: &[u8], form| Value::String(Str::Bytes(bytes.to_vec()), Some(form));
        let ab = |form| string(b"AB", form);
        let values: Vec<&Value> = a.iter().map(|o| o.value()).collect();
        assert_eq!(
            values[..3],
            [&ab(Form::Hex), &ab(Form::Literal), &ab(Form::Hex)]
        );
        let Value::Array(inner) = a[3].value() else {
            panic!("an array");
        };
        assert_eq!(*inner[0].value(), string(b"", Form::Hex));
        assert!(kept.faults.is_empty());
    }

    /// Strings, hex or literal, at any depth, are ciphertext exactly when
    /// the parser is told they are.
    #[test]
    fn strings_are_ciphertext_when_asked() {
        let input = b"<< /A (x\\)) /B [<78>] /N /x >>";
        for ciphertext in [false, true] {
            let o = Parser::at(input, 0)
                .ciphertext(ciphertext)
                .value(&mut ())
                .unwrap();
            let d = o.as_dict().unwrap();
            let string = |bytes: &[u8], form| match ciphertext {
                true => Value::String(Str::Encrypted(bytes.to_vec()), Some(form)),
                false => Value::String(Str::Bytes(bytes.to_vec()), Some(form)),
            };
            assert_eq!(*d.get(b"A").unwrap().value(), string(b"x)", Form::Literal));
            let Value::Array(b) = d.get(b"B").unwrap().value() else {
                panic!("an array");
            };
            assert_eq!(*b[0].value(), string(b"x", Form::Hex));
            assert_eq!(*d.get(b"N").unwrap().value(), Value::Name(b"x".to_vec()));
        }
    }

    /// Values nest as deep as 256 inside the outermost, and no deeper.
    #[test]
    fn nesting_is_capped_at_256() {
        let nested = |depth: usize, open: &str, inner: &str, close: &str| {
            format!("{}{inner}{}", open.repeat(depth), close.repeat(depth))
        };
        for (open, inner, close) in [("[", "1", "]"), ("<< /K ", "1", " >>"), ("[", "", "]")] {
            // The inner value at depth 256 reads, and at 257 doesn't; an empty
            // array is one deeper than what's in it.
            let reads = match inner {
                "" => 257,
                _ => 256,
            };
            let input = nested(reads, open, inner, close);
            assert!(value(input.as_bytes()).0.is_ok(), "{reads}");
            // Refused where the value nested deeper starts.
            let input = nested(reads + 1, open, inner, close);
            assert_eq!(
                value(input.as_bytes()).0,
                Err(Error::Nested {
                    limit: NESTING,
                    at: open.len() * (NESTING + 1)
                })
            );
        }
    }

    /// The default limits are what a parser reads under unless it's given
    /// others: values nested at most `NESTING` deep, and 2^25 of them.
    #[test]
    fn the_default_limits_nest_values_at_most_nesting_deep() {
        assert_eq!(Limits::DEFAULT.nesting, NESTING);
        assert_eq!(Limits::DEFAULT.values, 1 << 25);
        assert_eq!(Limits::default(), Limits::DEFAULT);
        assert_eq!(Parser::at(b"", 0).limits, Limits::DEFAULT);
    }

    /// A parser is made, and set, at compile time, and says where it is
    /// there; what a stream's head says of itself is read by `const fn`s.
    #[test]
    fn a_parser_is_made_at_compile_time() {
        const PARSER: Parser<'static> = Parser::at(b"%x\n[1 0 R]", 3)
            .window(10, true)
            .in_stream(ObjectId::new(4, 0))
            .ciphertext(true)
            .limits(Limits::DEFAULT)
            .rules(Rules::Pdfium);
        const OFFSET: usize = PARSER.offset();
        assert_eq!(OFFSET, 13);
        let mut parser = PARSER;
        let value = parser.value(&mut ()).unwrap();
        assert_eq!(value.at, Some(Span::new(Some(ObjectId::new(4, 0)), 13, 20)));
        assert_eq!(parser.offset(), 20);

        const fn parts<'h>(head: &'h StreamHead<'_>) -> (&'h Dict, Span, usize) {
            (head.dict(), head.keyword(), head.start())
        }
        let data = b"1 0 obj << /Length 2 >> stream\nab\nendstream endobj";
        let mut parser = Parser::at(data, 0);
        let id = parser.header().unwrap();
        let Ok(Body::Stream(head)) = parser.body(id, &mut ()) else {
            panic!("a stream");
        };
        let (dict, keyword, start) = parts(&head);
        assert_eq!(dict.get(b"Length").and_then(Object::as_integer), Some(2));
        assert_eq!((keyword, start), (Span::new(None, 24, 30), 31));
    }

    /// What a call's values take in memory at worst, as `Limits::values`
    /// says it, on a 64-bit target: an `Object` at most 72 bytes and an
    /// `Entry`, a key and its value, at most 128, so an array of one value,
    /// four `Object`s' room, takes the most, 288 bytes a value, and the
    /// default lets a call hold 9 GiB. An upper bound, as the layout is the
    /// compiler's to choose.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn a_value_holds_at_most_what_the_limits_say() {
        assert!(size_of::<Object>() <= 72, "{}", size_of::<Object>());
        assert!(size_of::<Entry>() <= 128, "{}", size_of::<Entry>());
        let most = 4 * 72;
        assert!(4 * size_of::<Object>() <= most);
        assert!(4 * size_of::<Entry>() / 2 <= most);
        assert_eq!(most * Limits::DEFAULT.values, 9 << 30);
    }

    /// A parser reads as many values in one call as its limits allow, its
    /// keys among them, and refuses one more, where it starts, leaving the
    /// parser where it was; each call counts from none.
    #[test]
    fn values_are_capped_by_the_limits() {
        // Each input, its count of values, and where its last starts.
        let cases: [(&[u8], usize, usize); 6] = [
            (b"7", 1, 0),
            (b"1 0 R", 1, 0),
            (b"[1 2 3]", 4, 5),
            (b"[[] << >> ()]", 4, 10),
            (b"<< /A 1 /B [2] >>", 6, 12),
            (b"<< /A << /B 1 /B 2 >> >>", 7, 17),
        ];
        for (input, values, last) in cases {
            let data = [b"%x\n", input, b" 9"].concat();
            let under = |values| Limits {
                values,
                ..Limits::DEFAULT
            };
            let mut parser = Parser::at(&data, 3).limits(under(values));
            assert!(parser.value(&mut ()).is_ok(), "{values}");
            assert_eq!(parser.offset(), data.len() - 2);
            let mut parser = Parser::at(&data, 3).limits(under(values - 1));
            let refused = parser.value(&mut ());
            let past = Error::Values {
                limit: values - 1,
                at: 3 + last,
            };
            assert_eq!(refused, Err(past));
            assert_eq!(parser.offset(), 3);
            // An indirect object's body, as a value.
            let data = [b"1 0 obj ", input, b" endobj"].concat();
            for (values, reads) in [(values, true), (values - 1, false)] {
                let mut parser = Parser::at(&data, 0).limits(under(values));
                let id = parser.header().unwrap();
                let after = parser.offset();
                let body = parser.body(id, &mut ());
                assert_eq!(body.is_ok(), reads, "{values}");
                if !reads {
                    assert!(matches!(body, Err(Error::Values { .. })));
                    assert_eq!(parser.offset(), after);
                }
            }
        }
        // Each call counts from none.
        let mut limits = Limits::DEFAULT;
        limits.values = 2;
        let mut parser = Parser::at(b"[1] [2]", 0).limits(limits);
        assert!(parser.value(&mut ()).is_ok() && parser.value(&mut ()).is_ok());
        assert_eq!(
            Error::Values { limit: 2, at: 9 }.to_string(),
            "more than 2 values in one object, one more at byte 9"
        );
    }

    /// Only a value reaches a limit: a token where a value should be that
    /// can't start one, or none, is the error it is without them; a value
    /// past either limit reaches it where it starts.
    #[test]
    fn only_a_value_reaches_a_limit() {
        let mut limits = Limits::DEFAULT;
        limits.nesting = 0;
        limits.values = 2;
        for input in [&b"<< /A >>"[..], b"<< /A", b"[>>]", b"[endobj]", b"[(open"] {
            let without = Parser::at(input, 0).value(&mut ());
            let reached = matches!(without, Err(Error::Nested { .. } | Error::Values { .. }));
            assert!(without.is_err() && !reached, "{without:?}");
            let mut parser = Parser::at(input, 0).limits(limits);
            assert_eq!(parser.value(&mut ()), without, "{input:?}");
        }
        let input = b"<< /A 1 >>";
        let mut parser = Parser::at(input, 0).limits(limits);
        let nested = Error::Nested { limit: 0, at: 6 };
        assert_eq!(parser.value(&mut ()), Err(nested));
        limits.nesting = 1;
        let mut parser = Parser::at(input, 0).limits(limits);
        assert_eq!(
            parser.value(&mut ()),
            Err(Error::Values { limit: 2, at: 6 })
        );
    }

    /// A parser reads values nested as deep as its limits allow, and
    /// refuses one deeper, leaving the parser where it was.
    #[test]
    fn nesting_is_capped_by_the_limits() {
        for nesting in [0, 1, 7] {
            let mut limits = Limits::DEFAULT;
            limits.nesting = nesting;
            let nested = |depth| format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
            let input = nested(nesting);
            let mut parser = Parser::at(input.as_bytes(), 0).limits(limits);
            assert!(parser.value(&mut ()).is_ok(), "{nesting}");
            let input = nested(nesting + 1);
            let mut parser = Parser::at(input.as_bytes(), 0).limits(limits);
            let nested = Error::Nested {
                limit: nesting,
                at: nesting + 1,
            };
            assert_eq!(parser.value(&mut ()), Err(nested));
            assert_eq!(parser.offset(), 0);
        }
    }

    /// An indirect object that isn't a stream, in its three calls' first
    /// two.
    #[test]
    fn an_indirect_object_reads_after_its_header() {
        let input = b"12 3 obj\n[1 2]\nendobj";
        let mut parser = Parser::at(input, 0);
        assert_eq!(parser.header(), Ok(ObjectId::new(12, 3)));
        let Ok(Body::Object(o)) = parser.body(ObjectId::new(12, 3), &mut ()) else {
            panic!("an object");
        };
        assert_eq!(bytes(input, o.at.unwrap()), b"[1 2]");
        assert_eq!(parser.offset(), 14);
    }

    /// A stream's two extents: by a `/Length` that holds, one that doesn't,
    /// and none; each passed to `stream`, and nothing else.
    #[test]
    fn a_streams_data_is_the_extent_its_caller_chose() {
        let input = b"4 0 obj\n<< /Length 6 >>\nstream\r\nabcdef\nendstream\nendobj";
        let mut parser = Parser::at(input, 0);
        let id = parser.header().unwrap();
        let mut kept = Kept::default();
        let Ok(Body::Stream(head)) = parser.body(id, &mut kept) else {
            panic!("a stream");
        };
        assert!(kept.faults.is_empty());
        assert_eq!(bytes(input, head.keyword()), b"stream");
        assert_eq!(
            head.dict().get(b"Length").and_then(Object::as_integer),
            Some(6)
        );
        let start = found(input, b"abcdef").start;
        assert_eq!((head.start(), parser.offset()), (start, start));
        // A length that holds, and with white-space before `endstream`, one
        // a byte shorter or longer than the data that doesn't.
        assert_eq!(head.declared(6), Some(Span::new(None, start, start + 6)));
        assert_eq!(head.declared(7), Some(Span::new(None, start, start + 7)));
        for length in [5, 8, 99, usize::MAX] {
            assert_eq!(head.declared(length), None, "{length}");
        }
        // None: up to `endstream`.
        assert_eq!(bytes(input, head.scanned()), b"abcdef\n");
        for data in [head.declared(6).unwrap(), head.scanned()] {
            let mut parser = parser.clone();
            let o = parser.stream(head.clone(), data, &mut kept).unwrap();
            assert!(kept.faults.is_empty());
            let Value::Stream(s) = o.value() else {
                panic!("a stream");
            };
            assert_eq!(s.data, Data::At(data));
            assert_eq!(s.dict, *head.dict());
            assert_eq!(
                bytes(input, o.at.unwrap()),
                &b"<< /Length 6 >>\nstream\r\nabcdef\nendstream"[..]
            );
            assert_eq!(parser.offset(), input.len() - 7);
        }
        // Data that doesn't start where the stream's does, runs past the
        // input, ends before it starts, or is in another stream's data.
        for data in [
            Span::new(None, start + 1, start + 6),
            Span::new(None, start - 1, start + 6),
            Span::new(None, start, input.len() + 1),
            Span::new(None, start, start - 1),
            Span::new(Some(ObjectId::new(9, 0)), start, start + 6),
        ] {
            let refused = parser.clone().stream(head.clone(), data, &mut ());
            let want = Error::Extent {
                id: ObjectId::new(4, 0),
                start: data.start,
                end: data.end,
            };
            assert_eq!(refused, Err(want), "{data:?}");
        }
    }

    /// A stream whose data holds `endstream`: its `/Length` reaches past
    /// it, scanning stops at it, and either reads, the scanned one with
    /// what follows it read as what follows the stream.
    #[test]
    fn a_stream_whose_data_holds_endstream_has_two_extents() {
        let input = b"1 0 obj << /Length 16 >> stream\nab endstream cd\nendstream endobj";
        let start = found(input, b"ab").start;
        let declared = Span::new(None, start, start + 16);
        assert_eq!(bytes(input, declared), b"ab endstream cd\n");
        let (o, kept) = indirect(input, |head| {
            assert_eq!(head.declared(16), Some(declared));
            assert_eq!(bytes(input, head.scanned()), b"ab ");
            declared
        });
        assert!(kept.faults.is_empty());
        let o = o.unwrap();
        assert_eq!(o.at.unwrap().end, input.len() - 7);
        let (o, kept) = indirect(input, StreamHead::scanned);
        let Value::Stream(s) = o.as_ref().unwrap().value() else {
            panic!("a stream");
        };
        assert_eq!(bytes(input, s.data.at().unwrap()), b"ab ");
        let at = o.unwrap().at.unwrap();
        assert_eq!(bytes(input, at), b"<< /Length 16 >> stream\nab endstream");
        assert_eq!(kept.faults, [(at, "1 0 obj has no endobj".to_string())]);
    }

    /// A window of a file reads as the file does from where the window
    /// starts, each place in the file: its values' spans, the parser's
    /// offset, its errors' places and a stream's extents. The offset the
    /// parser is made at counts in the window. `Parser::at` reads its data
    /// as a whole file, a window that starts it and ends it.
    #[test]
    fn a_window_places_what_it_reads_in_the_file() {
        let file = b"%x\n<< /A [1 (x)] /B 2 0 R >> 9 [1 2";
        let mut whole = Parser::at(file, 3);
        let mut window = Parser::at(&file[3..], 0).window(3, true);
        assert_eq!(window.offset(), 3);
        for _ in 0..3 {
            let mut kept = (Kept::default(), Kept::default());
            assert_eq!(window.value(&mut kept.0), whole.value(&mut kept.1));
            assert_eq!(window.offset(), whole.offset());
            assert_eq!(kept.0, kept.1);
        }
        let array = found(file, b"[1 2").start;
        assert_eq!(window.value(&mut ()), Err(Error::Array { at: array }));
        let mut at = Parser::at(file, 3).window(0, true);
        assert_eq!(at.value(&mut ()), Parser::at(file, 3).value(&mut ()));
        // A header looked for past the window's end, where it was given.
        let error = Parser::at(b"", 5).window(100, true).header();
        assert_eq!(error, Err(Error::Header { at: 105 }));
        // A stream's extents, and the stream, in the file.
        let file = b"xx\n4 0 obj << /Length 2 >> stream\nab\nendstream endobj";
        let mut parser = Parser::at(&file[3..], 0).window(3, true);
        let id = parser.header().unwrap();
        let Ok(Body::Stream(head)) = parser.body(id, &mut ()) else {
            panic!("a stream");
        };
        let ab = found(file, b"ab");
        assert_eq!((head.start(), parser.offset()), (ab.start, ab.start));
        assert_eq!(bytes(file, head.keyword()), b"stream");
        assert_eq!(head.declared(2), Some(ab));
        assert_eq!(bytes(file, head.scanned()), b"ab\n");
        let o = parser.stream(head, ab, &mut ()).unwrap();
        assert_eq!(bytes(file, o.at.unwrap()), &file[11..file.len() - 7]);
        assert_eq!(o.as_stream().unwrap().data, Data::At(ab));
        assert_eq!(parser.offset(), file.len() - 7);
    }

    /// In a window that isn't the file's last, a call whose read reaches
    /// its end is short, where more of the file could change what it
    /// reads: `12 0 R` cut after `0`, and after `R` where `RG` follows, a
    /// string, an array and a dictionary cut, `endobj` cut, the CR of the
    /// CR LF after `stream`, and a stream whose `/Length` runs past the
    /// window, though `endstream` is scanned inside it; and a read that
    /// stops before the end says what the whole file does.
    #[test]
    fn a_window_short_of_the_files_end_is_short_where_more_could_change_it() {
        const BASE: usize = 1_000_000;
        let cut = |file: &'static [u8], before: &[u8]| {
            let end = found(file, before).start;
            (Parser::at(&file[..end], 0).window(BASE, false), BASE + end)
        };
        let values: [(&[u8], &[u8]); 6] = [
            (b"[12 0 R]", b" R]"),
            (b"12 0 RG", b"G"),
            (b"(ab) ", b") "),
            (b"[1 2] ", b"] "),
            (b"<< /A 1 >> ", b"> "),
            (b"1 ", b" "),
        ];
        for (file, before) in values {
            let (mut window, end) = cut(file, before);
            assert_eq!(window.value(&mut ()), Err(Error::Short { at: end }));
            assert_eq!(window.offset(), BASE, "left where it was");
            assert!(Parser::at(file, 0).value(&mut ()).is_ok());
        }
        assert_eq!(
            Error::Short { at: 7 }.to_string(),
            "the data ends at byte 7, where more of the file could change what's read"
        );
        // An error before the end is the whole file's.
        let (mut window, _) = cut(b"[1 endobj 2 3", b"3");
        let error = Error::Keyword {
            at: BASE + 3,
            word: b"endobj".to_vec(),
        };
        assert_eq!(window.value(&mut ()), Err(error));
        // `endobj` cut: short, though it said there was none.
        let (mut window, _) = cut(b"1 0 obj 7 endobj\n", b"bj\n");
        let id = window.header().unwrap();
        let mut kept = Kept::default();
        assert!(matches!(
            window.body(id, &mut kept),
            Err(Error::Short { .. })
        ));
        assert_eq!(kept.faults.len(), 1);
        // The CR of a CR LF: short, though it said a lone CR.
        let file = b"1 0 obj << >> stream\r\nxy\nendstream endobj";
        let (mut window, _) = cut(file, b"\nxy");
        let id = window.header().unwrap();
        let mut kept = Kept::default();
        assert!(matches!(
            window.body(id, &mut kept),
            Err(Error::Short { .. })
        ));
        assert_eq!(kept.faults.len(), 1);
        let (_, kept) = indirect(file, StreamHead::scanned);
        assert!(kept.faults.is_empty());
        // A `/Length` past the window: none by it, and a shorter extent
        // scanned, which the stream after would take, `endstream` and what
        // follows it read; but it's short, as the head remembers.
        let file = b"1 0 obj << /Length 17 >> stream\nab endstream cd \nendstream endobj";
        let (mut window, end) = cut(file, b"\nendstream endobj");
        let id = window.header().unwrap();
        let Ok(Body::Stream(head)) = window.body(id, &mut ()) else {
            panic!("a stream");
        };
        assert_eq!(head.declared(17), None);
        let scanned = head.scanned();
        assert_eq!(scanned.end - scanned.start, 3);
        let stream = window.stream(head, scanned, &mut ());
        assert_eq!(stream, Err(Error::Short { at: end }));
        // The same window as the file's last reads as a file that ends
        // there: by the scanned extent, `endobj` said to be missing.
        let (window, _) = cut(file, b"\nendstream endobj");
        let mut last = window.window(BASE, true);
        let id = last.header().unwrap();
        let Ok(Body::Stream(head)) = last.body(id, &mut ()) else {
            panic!("a stream");
        };
        assert_eq!(head.declared(17), None);
        let scanned = head.scanned();
        let mut kept = Kept::default();
        assert!(last.stream(head, scanned, &mut kept).is_ok());
        assert_eq!(kept.faults.len(), 1);
        // Whole, it has its `/Length`'s.
        let (o, kept) = indirect(file, |head| head.declared(17).unwrap());
        assert!(o.is_ok() && kept.faults.is_empty());
    }

    /// A number with no integer after it is no reference, whatever comes
    /// next, so the parser looks no further for `R`: a window that ends in
    /// the token after the next isn't short, and reads the number as the
    /// whole input does. After an integer, it looks, and a window cut in
    /// what follows is short, as `R` may be there.
    #[test]
    fn a_number_with_no_integer_after_it_is_read_without_looking_for_r() {
        const BASE: usize = 1_000_000;
        let file = b"1 /A 2345 ";
        let whole = Parser::at(file, 0).value(&mut ()).unwrap();
        for cut in [6, 7, 8] {
            let mut window = Parser::at(&file[..cut], 0).window(BASE, false);
            let value = window.value(&mut ()).unwrap();
            assert_eq!(value.value(), whole.value(), "{cut}");
            assert_eq!(window.offset(), BASE + 1);
        }
        let file = b"1 0 2345 ";
        for cut in [6, 7, 8] {
            let mut window = Parser::at(&file[..cut], 0).window(BASE, false);
            assert!(
                matches!(window.value(&mut ()), Err(Error::Short { .. })),
                "{cut}"
            );
        }
        // An array's elements, each read once from where it looked for `]`.
        let (array, _) = value(b"[1 /A 2 0 R 3 [4] (5)]");
        let elements = array.unwrap().into_value();
        let Value::Array(elements) = elements else {
            panic!("an array");
        };
        let kinds: Vec<_> = elements.iter().map(|e| e.value().clone()).collect();
        assert_eq!(
            kinds,
            [
                Value::Integer(1),
                Value::Name(b"A".to_vec()),
                Value::Ref(ObjectId::new(2, 0)),
                Value::Integer(3),
                Value::Array(vec![Object::from(4).at(Span::new(None, 15, 16))]),
                Value::String(Str::Bytes(b"5".to_vec()), Some(Form::Literal)),
            ]
        );
    }

    /// Each of the tests' inputs read in every window, cut at every byte,
    /// from bytes 0 and 1,000,000 of a file: short, or as the whole input
    /// reads, every place moved by where the window starts.
    #[test]
    fn every_window_of_the_tests_inputs_is_short_or_reads_as_the_whole() {
        let inputs: [(&[u8], u16); 12] = [
            (b"%x\n<< /A [1 (x) /N] /B 2 0 R /C << /D true >> >> 9", 0),
            (b"[12 0 R] (ab) <41 42> [1 2] << /A 1 >> 1 ", 0),
            (b"12 0 RG ", 0),
            (b"[1 endobj 2 3", 0),
            (b"1 0 obj 7 endobj\n", 0),
            (b"5 0 obj << >> stray", 0),
            (b"1 0 obj << >> stream\r\nxy\nendstream endobj\n", 2),
            (b"1 0 obj << >>stream\rxy\nendstream\nendobj", 2),
            (
                b"4 0 obj\n<< /Length 6 >>\nstream\r\nabcdef\nendstream\nendobj ",
                6,
            ),
            (
                b"1 0 obj << /Length 16 >> stream\nab endstream cd\nendstream endobj\n",
                16,
            ),
            (
                b"1 0 obj << /Length 17 >> stream\nab endstream cd \nendstream endobj\n",
                17,
            ),
            (b"[/A#zz 99999999999999999999 -1.5e3]", 0),
        ];
        for (input, length) in inputs {
            let read = crate::fuzz::every_window(input, length);
            assert!(read > 0, "{}", String::from_utf8_lossy(input));
        }
    }

    /// `input` read as a value from its start by PDFium's rules: the value
    /// or the error, and what the sink was told.
    fn pdfium(input: &[u8]) -> (Result<Object, Error>, Kept) {
        let mut kept = Kept::default();
        let value = Parser::at(input, 0).rules(Rules::Pdfium).value(&mut kept);
        (value, kept)
    }

    /// The text of each fault `kept` was told, where it was told it.
    fn said(input: &[u8], kept: &Kept) -> Vec<(String, String)> {
        let at = |at: Span| String::from_utf8_lossy(&input[at.start..at.end]).into_owned();
        (kept.faults.iter())
            .map(|(span, said)| (at(*span), said.clone()))
            .collect()
    }

    /// ISO 32000-2's rules are the default, and any rules but PDFium's
    /// read as they do.
    #[test]
    fn iso_32000s_rules_are_the_default() {
        assert_eq!(Parser::at(b"", 0).rules, Rules::Iso);
        let input = b"[--5 1.2.3]";
        let iso = Err(Error::Keyword {
            at: 1,
            word: b"--5".to_vec(),
        });
        assert_eq!(value(input).0, iso);
        let mut parser = Parser::at(input, 0).rules(Rules::default());
        assert_eq!(parser.value(&mut ()), iso);
        assert!(pdfium(input).0.is_ok());
    }

    /// By PDFium's rules a number word is the number PDFium holds, at 64
    /// bits, as PDFium's chromium/7881, 8076 and 8125 read the first
    /// corner of a `/MediaBox` (one-page files, 2026-10-30), a fault said
    /// where ISO 32000-2 reads it otherwise: not as a number, or as an
    /// integer PDFium doesn't hold.
    #[test]
    fn pdfiums_rules_read_numbers_as_pdfium_holds_them() {
        let keyword = "is a number by PDFium's rules and a keyword to ISO 32000-2";
        let range = "is outside the integers PDFium's rules read, and read as 0";
        let cases: [(&[u8], Value, &str); 22] = [
            (b"--5", Value::Integer(0), keyword),
            (b"1.2.3", Value::Real(1.2), keyword),
            (b"--5.0", Value::Real(-5.0), keyword),
            (b"+-.5", Value::Real(-0.5), keyword),
            (b"5-3", Value::Integer(5), keyword),
            (b"+", Value::Integer(0), keyword),
            (b"-", Value::Integer(0), keyword),
            (b".", Value::Real(0.0), keyword),
            (b"1..2", Value::Real(1.0), keyword),
            (b"4294967296", Value::Integer(0), range),
            (b"+2147483648", Value::Integer(0), range),
            (b"-2147483649", Value::Integer(0), range),
            (b"99999999999999999999", Value::Integer(0), range),
            // Read alike by both.
            (b"-5", Value::Integer(-5), ""),
            (b"007", Value::Integer(7), ""),
            (b"+5", Value::Integer(5), ""),
            (b"4294967295", Value::Integer(4_294_967_295), ""),
            (b"2147483648", Value::Integer(2_147_483_648), ""),
            (b"-2147483648", Value::Integer(-2_147_483_648), ""),
            (b"-.5", Value::Real(-0.5), ""),
            (b"0.1", Value::Real(0.1), ""),
            (b"5.", Value::Real(5.0), ""),
        ];
        for (word, read, fault) in cases {
            let (value, kept) = pdfium(word);
            assert_eq!(*value.unwrap().value(), read, "{word:?}");
            let text = String::from_utf8_lossy(word);
            let want: Vec<(String, String)> = match fault {
                "" => Vec::new(),
                fault => vec![(text.to_string(), format!("{text} {fault}"))],
            };
            assert_eq!(said(word, &kept), want, "{word:?}");
            // In a /MediaBox's first corner, as PDFium's width says.
            let input = [b"[", word, b" 0 100 100]"].concat();
            let (boxed, kept) = pdfium(&input);
            let corner = boxed.unwrap().as_array().unwrap()[0].as_number();
            assert_eq!(corner, read_number(&read), "{word:?}");
            assert_eq!(kept.faults.len(), usize::from(!fault.is_empty()));
        }
        // ISO 32000-2's rules read none of them otherwise.
        let (value, kept) = self::value(b"4294967296");
        assert_eq!(*value.unwrap().value(), Value::Integer(4_294_967_296));
        assert!(kept.faults.is_empty());
        // A keyword to both.
        for word in [&b"1e5"[..], b"3.4028236e38"] {
            assert!(matches!(pdfium(word).0, Err(Error::Keyword { .. })));
        }
    }

    /// A number value as an `f64`.
    fn read_number(value: &Value) -> Option<f64> {
        match *value {
            Value::Integer(n) => Some(n as f64),
            Value::Real(r) => Some(r),
            _ => None,
        }
    }

    /// By PDFium's rules a number or a name is read by its first 256
    /// bytes, a name's `/` among them, as PDFium's chromium/7881, 8076
    /// and 8125 read a `/MediaBox`'s corner and an `/Info` key
    /// (2026-10-30): longer, a fault, as ISO 32000-2 reads every byte.
    #[test]
    fn pdfiums_rules_read_a_word_by_its_first_256_bytes() {
        let long =
            "is longer than the 256 bytes PDFium's rules read of a word, and read as its first 256";
        // `000...05`, read as 5 whole and 0 cut.
        for (length, read) in [(255, 5), (256, 5), (257, 0), (258, 0)] {
            let word = [vec![b'0'; length - 1], b"5".to_vec()].concat();
            let (value, kept) = pdfium(&word);
            assert_eq!(value.unwrap().as_integer(), Some(read), "{length}");
            assert_eq!(kept.faults.len(), usize::from(length > 256), "{length}");
            assert_eq!(self::value(&word).0.unwrap().as_integer(), Some(5));
        }
        // A real, an integer where its point is cut away, as PDFium holds
        // the bytes it keeps.
        let real = [b"0.".as_slice(), &[b'0'; 254], b"5"].concat();
        let (value, kept) = pdfium(&real);
        assert_eq!(*value.unwrap().value(), Value::Real(0.0));
        let text = String::from_utf8_lossy(&real[..40]);
        assert_eq!(
            kept.faults,
            [(Span::new(None, 0, 257), format!("{text} {long}"))]
        );
        let real = [&[b'0'; 255][..], b"7.5"].concat();
        assert_eq!(*pdfium(&real).0.unwrap().value(), Value::Integer(7));
        let real = [&[b'0'; 256][..], b".5"].concat();
        assert_eq!(*pdfium(&real).0.unwrap().value(), Value::Integer(0));
        assert_eq!(*self::value(&real).0.unwrap().value(), Value::Real(0.5));
        // A name, as a key and as a value: `/` and 255 bytes kept.
        for length in [255, 256, 257] {
            let name = [b"/".as_slice(), &vec![b'P'; length - 1]].concat();
            let input = [b"<<".as_slice(), &name, b" ", &name, b">>"].concat();
            let (dict, kept) = pdfium(&input);
            let dict = dict.unwrap();
            let kept_bytes = vec![b'P'; (length - 1).min(255)];
            let (key, value) = dict.as_dict().unwrap().iter().next().unwrap();
            assert_eq!(
                (key, value.as_name()),
                (&kept_bytes[..], Some(&kept_bytes[..]))
            );
            assert_eq!(kept.faults.len(), 2 * usize::from(length > 256), "{length}");
        }
        // A keyword isn't cut: it's no value either way.
        let keyword = vec![b'x'; 300];
        let (error, _) = pdfium(&keyword);
        assert_eq!(
            error,
            Err(Error::Keyword {
                at: 0,
                word: vec![b'x'; 32]
            })
        );
    }

    /// By PDFium's rules a hex string's digits pair to its `>` over any
    /// other byte, as PDFium's chromium/7881, 8076 and 8125 read a
    /// `/Title`, a fault said where ISO 32000-2 ends the string before
    /// such a byte.
    #[test]
    fn pdfiums_rules_read_a_hex_string_to_its_close() {
        for (input, meant, fault) in [
            (&b"<41G42>"[..], &b"AB"[..], true),
            (b"<4G1>", b"A", true),
            (b"<4G>", b"@", true),
            (b"<41(42>", b"AB", true),
            (b"<41/42>", b"AB", true),
            (b"<41\x8042>", b"AB", true),
            (b"<41%42>", b"AB", true),
            (b"<41 4>", b"A@", false),
            (b"<414>", b"A@", false),
            (b"<41\n42>", b"AB", false),
            (b"<>", b"", false),
        ] {
            let (value, kept) = pdfium(input);
            let value = value.unwrap();
            assert_eq!(value.as_bytes(), Some(meant), "{input:?}");
            assert_eq!(
                *value.value(),
                Value::String(Str::Bytes(meant.to_vec()), Some(Form::Hex))
            );
            let text = String::from_utf8_lossy(input);
            let want = format!("the hex string {text} has a byte that's no hex digit");
            let want = match fault {
                true => vec![(text.to_string(), want)],
                false => Vec::new(),
            };
            assert_eq!(said(input, &kept), want, "{input:?}");
            let iso = self::value(input).0;
            assert_eq!(iso.is_err(), fault, "{input:?}");
        }
        // Unclosed only where the data ends first.
        assert_eq!(pdfium(b"<41G").0, Err(Error::String { at: 0 }));
        // As ciphertext, the same bytes.
        let mut parser = Parser::at(b"<4G1>", 0)
            .rules(Rules::Pdfium)
            .ciphertext(true);
        let value = parser.value(&mut ()).unwrap();
        assert_eq!(
            *value.value(),
            Value::String(Str::Encrypted(b"A".to_vec()), Some(Form::Hex))
        );
    }

    /// By PDFium's rules a name's `#` before any two bytes is an escape, a
    /// byte that's no hex digit 0, as PDFium's chromium/7881, 8076 and 8125
    /// read an `/Info` key (2026-10-30); with fewer after it, itself. ISO
    /// 32000-2's rules read such a `#` as itself, and the fault is theirs.
    #[test]
    fn pdfiums_rules_read_a_names_hash_before_any_two_bytes() {
        let stray = "has a # without two hex digits after it, which readers read differently";
        let nul = "has #00, NUL, which a name can't hold, which readers read differently";
        for (input, meant, iso, fault) in [
            (
                &b"/#5Groducer"[..],
                &b"Producer"[..],
                &b"#5Groducer"[..],
                stray,
            ),
            (b"/P#g4", b"P\x04", b"P#g4", stray),
            (b"/##41", b"\x041", b"#A", stray),
            (b"/P#7z", b"Pp", b"P#7z", stray),
            (b"/F#3", b"F#3", b"F#3", stray),
            (b"/Producer#", b"Producer#", b"Producer#", stray),
            (b"/N#00", b"N\0", b"N\0", nul),
            (b"/A#20B", b"A B", b"A B", ""),
            (b"/Producer", b"Producer", b"Producer", ""),
        ] {
            let (value, kept) = pdfium(input);
            assert_eq!(value.unwrap().as_name(), Some(meant), "{input:?}");
            let text = String::from_utf8_lossy(input);
            let want = match fault {
                "" => Vec::new(),
                fault => vec![(text.to_string(), format!("the name {text} {fault}"))],
            };
            assert_eq!(said(input, &kept), want, "{input:?}");
            let (value, iso_kept) = self::value(input);
            assert_eq!(value.unwrap().as_name(), Some(iso), "{input:?}");
            assert_eq!(iso_kept, kept, "told alike, {input:?}");
            // As a key.
            let dict = [b"<<".as_slice(), input, b" (x)>>"].concat();
            let (dict, _) = pdfium(&dict);
            let dict = dict.unwrap();
            assert!(dict.as_dict().unwrap().has(meant), "{input:?}");
        }
    }

    /// By PDFium's rules a literal string's CR, alone or before LF, is read
    /// as written, as PDFium's chromium/7881, 8076 and 8125 read a `/Title`
    /// (2026-10-30), a fault said where ISO 32000-2 reads LF; every escape
    /// reads alike.
    #[test]
    fn pdfiums_rules_read_a_literal_strings_cr_as_written() {
        let cr = |text: &str| {
            format!(
                "the string {text} has a CR written in it, read as written, where ISO 32000-2 reads LF"
            )
        };
        for (input, meant, iso, fault) in [
            (
                &b"(a\rb\r\nc)"[..],
                &b"a\rb\r\nc"[..],
                &b"a\nb\nc"[..],
                true,
            ),
            (b"(a\r\rb)", b"a\r\rb", b"a\n\nb", true),
            (b"(a\n\rb)", b"a\n\rb", b"a\n\nb", true),
            (b"(a\\\rb)", b"ab", b"ab", false),
            (b"(a\\\r\nb)", b"ab", b"ab", false),
            (b"(a\\\nb)", b"ab", b"ab", false),
            (b"(a\\015b)", b"a\rb", b"a\rb", false),
            (b"(a\nb)", b"a\nb", b"a\nb", false),
            (b"(a\\)b)", b"a)b", b"a)b", false),
        ] {
            let (value, kept) = pdfium(input);
            let value = value.unwrap();
            assert_eq!(value.as_bytes(), Some(meant), "{input:?}");
            assert_eq!(
                value.value(),
                &Value::String(Str::Bytes(meant.to_vec()), Some(Form::Literal))
            );
            let text = String::from_utf8_lossy(input);
            let want = match fault {
                true => vec![(text.to_string(), cr(&text))],
                false => Vec::new(),
            };
            assert_eq!(said(input, &kept), want, "{input:?}");
            assert_eq!(self::value(input).0.unwrap().as_bytes(), Some(iso));
        }
    }

    /// By PDFium's rules 0x80 and 0xFF are white-space, as PDFium's
    /// chromium/7881, 8076 and 8125 read a `/MediaBox` and an `/Info` key
    /// (2026-10-30), each told where it is.
    #[test]
    fn pdfiums_rules_read_0x80_and_0xff_as_white_space() {
        for space in [0x80, 0xFF] {
            let input = [b"[0".as_slice(), &[space], b"0 100 100]"].concat();
            let (value, kept) = pdfium(&input);
            let each: Vec<i64> = (value.unwrap().as_array().unwrap().iter())
                .map(|o| o.as_integer().unwrap())
                .collect();
            assert_eq!(each, [0, 0, 100, 100]);
            assert_eq!(kept.faults, high_bytes(b"[0~0 100 100]", space, 0));
            assert!(matches!(
                self::value(&input).0,
                Err(Error::Keyword { at: 1, .. })
            ));
            let input = [b"<< /Title".as_slice(), &[space], b"(x) >>"].concat();
            let (dict, kept) = pdfium(&input);
            assert_eq!(
                dict.unwrap()
                    .as_dict()
                    .unwrap()
                    .get(b"Title")
                    .and_then(Object::as_bytes),
                Some(&b"x"[..])
            );
            assert_eq!(kept.faults, high_bytes(b"<< /Title~(x) >>", space, 0));
        }
    }

    /// By PDFium's rules a reference is still two integers and `R`, each
    /// integer as PDFium holds it, a fault said where ISO 32000-2 reads it
    /// otherwise; it numbers no object where the number doesn't fit 32 bits
    /// or the generation 16. PDFium's chromium/7881, 8076 and 8125 name
    /// object 3 from `3.9 0 R`, `3 -1 R`, `3 99999999999 R`, `3 --1 R`, `3
    /// 0.5 R`, `3 65536 R` and `3 + R` in a page tree's `/Kids`, and nothing
    /// from `--3 0 R`, `-3 0 R` or `4294967299 0 R` (2026-10-30): a
    /// reference is any two number words and `R` to its object parser, its
    /// number read from the first and its generation not read, which isn't
    /// followed here.
    #[test]
    fn pdfiums_rules_read_a_reference_by_its_integers() {
        let keyword = "is a number by PDFium's rules and a keyword to ISO 32000-2";
        let range = "is outside the integers PDFium's rules read, and read as 0";
        for (input, id, told) in [
            (&b"3 0 R"[..], ObjectId::new(3, 0), vec![]),
            (b"+3 0 R", ObjectId::new(3, 0), vec![]),
            (b"--3 0 R", ObjectId::new(0, 0), vec![("--3", keyword)]),
            (b"3 --1 R", ObjectId::new(3, 0), vec![("--1", keyword)]),
            (b"3 + R", ObjectId::new(3, 0), vec![("+", keyword)]),
            (
                b"--3 + R",
                ObjectId::new(0, 0),
                vec![("--3", keyword), ("+", keyword)],
            ),
            (
                b"3 99999999999 R",
                ObjectId::new(3, 0),
                vec![("99999999999", range)],
            ),
            (
                b"4294967299 0 R",
                ObjectId::new(0, 0),
                vec![("4294967299", range)],
            ),
            (
                b"4294967295 65535 R",
                ObjectId::new(u32::MAX, u16::MAX),
                vec![],
            ),
        ] {
            let (value, kept) = pdfium(input);
            let value = value.unwrap();
            assert_eq!(value.as_reference(), Some(id), "{input:?}");
            assert_eq!(value.at, Some(Span::new(None, 0, input.len())));
            let want: Vec<(String, String)> = (told.iter())
                .map(|(at, said)| (at.to_string(), format!("{at} {said}")))
                .collect();
            assert_eq!(said(input, &kept), want, "{input:?}");
        }
        // Numbering no object.
        for (input, number, generation) in [
            (&b"3 -1 R"[..], &b"3"[..], &b"-1"[..]),
            (b"-3 0 R", b"-3", b"0"),
            (b"3 65536 R", b"3", b"65536"),
        ] {
            let error = Error::Reference {
                at: 0,
                number: number.to_vec(),
                generation: generation.to_vec(),
            };
            assert_eq!(pdfium(input), (Err(error), Kept::default()), "{input:?}");
        }
        // A real is no reference's number or generation: three values.
        for input in [&b"[3.9 0 R]"[..], b"[3 0.5 R]"] {
            let at = input.len() - 2;
            let error = Error::Keyword {
                at,
                word: b"R".to_vec(),
            };
            assert_eq!(pdfium(input).0, Err(error), "{input:?}");
        }
        // A header's integers are read the same way.
        let mut parser = Parser::at(b"--1 4294967296 obj 7 endobj", 0).rules(Rules::Pdfium);
        assert_eq!(parser.header(), Ok(ObjectId::new(0, 0)));
        let mut parser = Parser::at(b"1\x800 obj", 0).rules(Rules::Pdfium);
        assert_eq!(parser.header(), Ok(ObjectId::new(1, 0)));
        assert_eq!(
            Parser::at(b"1\x800 obj", 0).header(),
            Err(Error::Header { at: 0 })
        );
    }

    /// By PDFium's rules a stream's `endstream` after the data a `/Length`
    /// gives is found past their white-space, 0x80 among it, and the
    /// stream read so: the 0x80 before `endstream` told, as the stream's,
    /// and the one after it not, as no object's.
    #[test]
    fn pdfiums_rules_find_endstream_past_their_white_space() {
        let input = b"1 0 obj << /Length 2 >> stream\nab\x80endstream\x80endobj";
        for (rules, declared) in [(Rules::Iso, false), (Rules::Pdfium, true)] {
            let mut parser = Parser::at(input, 0).rules(rules);
            let id = parser.header().unwrap();
            let mut kept = Kept::default();
            let Ok(Body::Stream(head)) = parser.body(id, &mut kept) else {
                panic!("a stream");
            };
            let ab = found(input, b"ab");
            assert_eq!(head.declared(2), declared.then_some(ab), "{rules:?}");
            let stream = parser.stream(head, ab, &mut kept);
            assert_eq!(stream.is_ok(), declared, "{rules:?}");
            let told = match declared {
                true => high_bytes(b"1 0 obj << /Length 2 >> stream\nab~endstream", 0x80, 0),
                false => Vec::new(),
            };
            assert_eq!(kept.faults, told, "{rules:?}");
        }
    }

    /// `text` with each `~` in it the byte `space`.
    fn with_byte(text: &[u8], space: u8) -> Vec<u8> {
        (text.iter())
            .map(|&b| if b == b'~' { space } else { b })
            .collect()
    }

    /// What [`Fault::HighByte`] tells of each `~` in `text` as
    /// [`with_byte`] makes it, `space` there, at its byte, in a window from
    /// byte `base`.
    fn high_bytes(text: &[u8], space: u8, base: usize) -> Vec<(Span, String)> {
        let said = Fault::HighByte { written: &[space] }.to_string();
        (text.iter().enumerate())
            .filter(|&(_, &b)| b == b'~')
            .map(|(at, _)| (Span::new(None, base + at, base + at + 1), said.clone()))
            .collect()
    }

    /// How many of the faults `kept` was told are [`Fault::HighByte`].
    fn high_told(kept: &Kept) -> usize {
        let high = [0x80, 0xFF].map(|b| Fault::HighByte { written: &[b] }.to_string());
        (kept.faults.iter())
            .filter(|(_, said)| high.contains(said))
            .count()
    }

    /// The indirect object `input` begins with, read by `rules`, its
    /// stream's data by a `/Length` of `length` where `endstream` follows
    /// it, else scanned: what the sink was told.
    fn indirect_told(input: &[u8], rules: Rules, length: usize) -> Kept {
        let mut kept = Kept::default();
        let mut parser = Parser::at(input, 0).rules(rules);
        let id = parser.header().unwrap();
        match parser.body(id, &mut kept).unwrap() {
            Body::Object(_) => {}
            Body::Stream(head) => {
                let data = head.declared(length).unwrap_or_else(|| head.scanned());
                parser.stream(head, data, &mut kept).unwrap();
            }
        }
        kept
    }

    /// A real too small for 64 bits, its digits not all zero, reads as 0
    /// with its sign, to the bit, by either rules; ISO 32000-2's say so,
    /// and PDFium's, which read it by its first 256 bytes, say only that
    /// it's longer: no 256-byte decimal is too small, the least, `.` and
    /// 254 zeros and a `1`, reading as 1e-255 by both.
    #[test]
    fn a_real_too_small_for_64_bits_is_said_by_isos_rules_alone() {
        let long =
            "is longer than the 256 bytes PDFium's rules read of a word, and read as its first 256";
        for (sign, zero) in [("", 0.0f64), ("-", -0.0)] {
            let tiny = format!("{sign}0.{}1", "0".repeat(400));
            let whole = Span::new(None, 0, tiny.len());
            let first = String::from_utf8_lossy(&tiny.as_bytes()[..40]);
            for (rules, said) in [
                (
                    Rules::Iso,
                    "a real too small for 64 bits, read as 0".to_string(),
                ),
                (Rules::Pdfium, format!("{first} {long}")),
            ] {
                let mut kept = Kept::default();
                let value = Parser::at(tiny.as_bytes(), 0).rules(rules).value(&mut kept);
                let Value::Real(read) = *value.unwrap().value() else {
                    panic!("a real");
                };
                assert_eq!(read.to_bits(), zero.to_bits(), "{rules:?}");
                assert_eq!(kept.faults, [(whole, said)], "{rules:?}");
            }
        }
        let least = [b".".as_slice(), &[b'0'; 254], b"1"].concat();
        assert_eq!(least.len(), 256);
        for rules in [Rules::Iso, Rules::Pdfium] {
            let mut kept = Kept::default();
            let value = Parser::at(&least, 0).rules(rules).value(&mut kept);
            assert_eq!(*value.unwrap().value(), Value::Real(1e-255), "{rules:?}");
            assert!(kept.faults.is_empty(), "{rules:?}");
        }
        assert_eq!(
            Fault::RealUnderflow.to_string(),
            "a real too small for 64 bits, read as 0"
        );
    }

    /// By PDFium's rules each 0x80 and 0xFF in the white-space of the
    /// object a call reads is told, once, at its byte, after the call's
    /// other faults: between a value's tokens, in an array and in a
    /// dictionary; at a word's end inside one; past an integer that `R` was
    /// looked for after, and inside a reference; between a stream's
    /// dictionary and `stream`, and between its data and `endstream`. By
    /// ISO 32000-2's rules, none: each is a regular character there.
    #[test]
    fn pdfiums_rules_tell_each_0x80_and_0xff_in_an_objects_white_space() {
        let values: [&[u8]; 8] = [
            b"[(a)~(b)]",
            b"<< /A~1 /B [2]~>>",
            b"[/A~]",
            b"[1~ 2]",
            b"[1 0~R]",
            b"<</A [1~~2]~>>",
            b"[1\n~\r\n2 %c\n~]",
            b"<< /A << /B [~] >> >>",
        ];
        let indirect: [&[u8]; 3] = [
            b"1 0 obj << /Length 2 >>~stream\nab\nendstream endobj",
            b"1 0 obj << /Length 2 >> stream\nab~endstream endobj",
            b"1 0 obj\n<< /A [1~2] >>\nendobj",
        ];
        for space in [0x80, 0xFF] {
            for text in values {
                let input = with_byte(text, space);
                let (value, kept) = pdfium(&input);
                assert!(value.is_ok(), "{text:?}: {value:?}");
                assert_eq!(kept.faults, high_bytes(text, space, 0), "{text:?}");
            }
            for text in indirect {
                let input = with_byte(text, space);
                let kept = indirect_told(&input, Rules::Pdfium, 2);
                assert_eq!(kept.faults, high_bytes(text, space, 0), "{text:?}");
            }
            assert_eq!(
                Fault::HighByte { written: &[space] }.to_string(),
                format!("0x{space:02X} is white-space here and a regular character to ISO 32000-2")
            );
        }
        // Each byte as itself.
        let input = b"[1\x80\xFF2]";
        let (_, kept) = pdfium(input);
        let mut told = high_bytes(b"[1~ 2]", 0x80, 0);
        told.extend(high_bytes(b"[1 ~2]", 0xFF, 0));
        assert_eq!(kept.faults, told);
        // After the call's other faults, though an integer's look for `R`
        // passed it before them, and `endobj`'s.
        let input = b"[--1\x80 /A]";
        let (_, kept) = pdfium(input);
        let mut told = vec![(
            Span::new(None, 1, 4),
            "--1 is a number by PDFium's rules and a keyword to ISO 32000-2".to_string(),
        )];
        told.extend(high_bytes(b"[--1~ /A]", 0x80, 0));
        assert_eq!(kept.faults, told);
        let text = b"1 0 obj [1~2] stray";
        let kept = indirect_told(&with_byte(text, 0x80), Rules::Pdfium, 0);
        let mut told = vec![(Span::new(None, 8, 13), "1 0 obj has no endobj".to_string())];
        told.extend(high_bytes(text, 0x80, 0));
        assert_eq!(kept.faults, told);
        // By ISO 32000-2's rules, part of a name, or a stream's data.
        let iso: [&[u8]; 3] = [
            b"[/A\x80]",
            b"<< /A\xFF 1 >>",
            b"1 0 obj << /Length 2 >> stream\nab\x80endstream endobj",
        ];
        for input in iso {
            let kept = match input.starts_with(b"1 0 obj") {
                true => indirect_told(input, Rules::Iso, 2),
                false => value(input).1,
            };
            assert!(kept.faults.is_empty(), "{input:?}");
        }
    }

    /// A 0x80 or 0xFF is told only where it's white-space in the object a
    /// call reads, by PDFium's rules: not in a comment, a string or a
    /// stream's data; not around an object, before its first token or
    /// after its last, though the call read past it looking for `R` or
    /// `endobj`, nor between `obj` and the value, before `endobj` or after
    /// `endstream`; and not by a call that fails, though it tells the
    /// faults it read past before the error. A window cut just after one
    /// is short, and tells none; the call with more tells it, once.
    #[test]
    fn a_high_byte_outside_the_object_read_or_in_a_call_that_fails_is_not_told() {
        let values: [&[u8]; 8] = [
            b"[1 %\x80\n 2]",
            b"[(a\x80b) (\xFF)]",
            b"\x80 5",
            b"5\x80",
            b"\xFF[1]\x80",
            b"<< /A 1 >>\x80",
            b"1 0 R\x80",
            b"(x)\xFF\xFF",
        ];
        for input in values {
            let (value, kept) = pdfium(input);
            assert!(value.is_ok(), "{input:?}: {value:?}");
            assert!(kept.faults.is_empty(), "{input:?}");
        }
        // A hex string runs to its `>` over any byte, which it says, as a
        // string's own: no 0x80 is told.
        let (_, kept) = pdfium(b"<4\x801>");
        assert_eq!(kept.faults.len(), 1);
        assert_eq!(high_told(&kept), 0);
        // Two values, the second read from where the first left the
        // parser, before the 0x80 between them.
        let input = b"5\x80 6";
        let mut parser = Parser::at(input, 0).rules(Rules::Pdfium);
        let mut kept = Kept::default();
        let five = parser.value(&mut kept).unwrap();
        assert_eq!((five.at, parser.offset()), (Some(Span::new(None, 0, 1)), 1));
        let six = parser.value(&mut kept).unwrap();
        assert_eq!(six.at, Some(Span::new(None, 3, 4)));
        assert!(kept.faults.is_empty());
        // Around an indirect object's value, and a stream's data.
        let indirect: [&[u8]; 4] = [
            b"1 0 obj\x805 endobj",
            b"1 0 obj 5\x80endobj",
            b"1 0 obj << >>stream\n\x80abc\xFF\nendstream\x80endobj",
            b"1 0 obj << /Length 3 >> stream\r\n\x80ab\nendstream\xFFendobj",
        ];
        for input in indirect {
            let kept = indirect_told(input, Rules::Pdfium, 3);
            assert!(kept.faults.is_empty(), "{input:?}");
        }
        // Calls that fail: a value, a body and a stream's rest.
        let (value, kept) = pdfium(b"[1\x80 2");
        assert_eq!(value, Err(Error::Array { at: 0 }));
        assert!(kept.faults.is_empty());
        let (value, kept) = pdfium(b"[--1\x80 2 endobj]");
        assert!(matches!(value, Err(Error::Keyword { at: 8, .. })));
        assert_eq!(
            kept.faults,
            [(
                Span::new(None, 1, 4),
                "--1 is a number by PDFium's rules and a keyword to ISO 32000-2".to_string()
            )]
        );
        let mut parser = Parser::at(b"1 0 obj [1\x80 2 endobj", 0).rules(Rules::Pdfium);
        let id = parser.header().unwrap();
        let mut kept = Kept::default();
        assert!(parser.body(id, &mut kept).is_err());
        assert!(kept.faults.is_empty());
        let input = b"1 0 obj << >> stream\nab\x80endstreamX endobj";
        let mut parser = Parser::at(input, 0).rules(Rules::Pdfium);
        let id = parser.header().unwrap();
        let mut kept = Kept::default();
        let Ok(Body::Stream(head)) = parser.body(id, &mut kept) else {
            panic!("a stream");
        };
        let stream = parser.stream(head, found(input, b"ab"), &mut kept);
        assert_eq!(stream, Err(Error::Endstream { id }));
        assert!(kept.faults.is_empty());
        // A window cut just after one: short, telling none; with the rest
        // of the file, it's told once, at its place in the file.
        const BASE: usize = 1_000_000;
        let file = b"[1\x80 2]";
        let mut kept = Kept::default();
        let mut window = (Parser::at(&file[..3], 0).window(BASE, false)).rules(Rules::Pdfium);
        assert_eq!(window.value(&mut kept), Err(Error::Short { at: BASE + 3 }));
        assert!(kept.faults.is_empty());
        let mut window = (Parser::at(file, 0).window(BASE, true)).rules(Rules::Pdfium);
        assert!(window.value(&mut kept).is_ok());
        assert_eq!(kept.faults, high_bytes(b"[1~ 2]", 0x80, BASE));
        // A body read whole, short where it looked for `endobj` past it:
        // it says there's none, as a call may short, but not the 0x80.
        let file = b"1 0 obj [1\x80 2] endobj";
        let mut kept = Kept::default();
        let mut window = (Parser::at(&file[..14], 0).window(BASE, false)).rules(Rules::Pdfium);
        let id = window.header().unwrap();
        let body = window.body(id, &mut kept);
        assert!(matches!(body, Err(Error::Short { at }) if at == BASE + 14));
        assert_eq!((kept.faults.len(), high_told(&kept)), (1, 0));
        let mut window = (Parser::at(file, 0).window(BASE, true)).rules(Rules::Pdfium);
        let id = window.header().unwrap();
        let mut kept = Kept::default();
        assert!(window.body(id, &mut kept).is_ok());
        assert_eq!(kept.faults, high_bytes(b"1 0 obj [1~ 2]", 0x80, BASE));
    }

    /// `()` wants no fault, and a sink borrowed wants what it does; a sink
    /// that wants none is told none, by either rules.
    #[test]
    fn a_sink_that_wants_no_faults_is_told_none() {
        struct Unwanted;
        impl Sink<'_> for Unwanted {
            fn fault(&mut self, at: Span, fault: Fault<'_>) {
                panic!("{fault} at {at:?}, told a sink that wants none");
            }
            fn wants(&self) -> bool {
                false
            }
        }
        // Through the impl for a borrow.
        fn wants<'a>(sink: impl Sink<'a>) -> bool {
            sink.wants()
        }
        assert!(!().wants() && !wants(&mut ()));
        assert!(Kept::default().wants() && wants(&mut Kept::default()));
        let sink: &mut dyn Sink = &mut Unwanted;
        assert!(!sink.wants() && !wants(&mut *sink));
        let tiny = [b"0.".as_slice(), &[b'0'; 400], b"1"].concat();
        let inputs: [&[u8]; 8] = [
            b"[/A#zz 99999999999999999999 -1.5e3]",
            b"1 0 obj << >>stream\rxy\nendstream\nendobj",
            b"1 0 obj << >>stream xy\nendstream",
            b"1 0 obj [--5 1.2.3 4294967296 <4G1> (a\rb) /#5Gx 3 --1 R] stray",
            b"1 0 obj << /N#00 1 >> endobj",
            &[b"1 0 obj [/".as_slice(), &[b'x'; 300], b"] endobj"].concat(),
            &[b"1 0 obj [".as_slice(), &tiny, b" -", &tiny, b"] endobj"].concat(),
            b"1 0 obj << /A [1\x80 2\xFF] >>\x80stream\nab\nendstream endobj",
        ];
        for rules in [Rules::Iso, Rules::Pdfium] {
            for input in inputs {
                let wanted = indirect_by(input, rules, &mut Kept::default());
                let unwanted = indirect_by(input, rules, &mut Unwanted);
                assert_eq!(wanted, unwanted, "{rules:?}");
            }
        }
    }

    /// The indirect object `input` begins with, read by `rules`, its
    /// stream's data the scanned extent, telling `sink`.
    fn indirect_by<'a>(
        input: &'a [u8],
        rules: Rules,
        sink: &mut impl Sink<'a>,
    ) -> Result<Object, Error> {
        let mut parser = Parser::at(input, 0).rules(rules);
        let id = parser.header()?;
        match parser.body(id, sink)? {
            Body::Object(object) => Ok(object),
            Body::Stream(head) => {
                let data = head.scanned();
                parser.stream(head, data, sink)
            }
        }
    }

    /// The limits hold by PDFium's rules as by ISO 32000-2's: tokens only
    /// PDFium's rules read as values count as values, and nest, where ISO
    /// 32000-2's read values of their lengths.
    #[test]
    fn the_limits_hold_by_either_rules() {
        let pdfiums = b"<< /A [--5 1.2.3] /B << /C <4G> >> >>";
        let isos = b"<< /A [-55 1.234] /B << /C <4F> >> >>";
        for (nesting, values) in [(2, 9), (1, 9), (0, 9), (2, 8), (2, 4), (2, 1)] {
            let limits = Limits { nesting, values };
            let mut pdfium = Parser::at(pdfiums, 0).rules(Rules::Pdfium).limits(limits);
            let mut iso = Parser::at(isos, 0).limits(limits);
            let (pdfium_read, iso_read) = (pdfium.value(&mut ()), iso.value(&mut ()));
            match (&pdfium_read, &iso_read) {
                (Ok(_), Ok(_)) => assert_eq!((nesting, values), (2, 9)),
                _ => assert_eq!(pdfium_read, iso_read, "{limits:?}"),
            }
            assert_eq!(pdfium.offset(), iso.offset(), "{limits:?}");
        }
        let limits = Limits {
            nesting: 1,
            values: 9,
        };
        let mut parser = Parser::at(pdfiums, 0).rules(Rules::Pdfium).limits(limits);
        assert_eq!(
            parser.value(&mut ()),
            Err(Error::Nested { limit: 1, at: 7 })
        );
    }

    /// The tests' inputs where the rules part, read in every window, cut
    /// at every byte, from bytes 0 and 1,000,000 of a file, by both rules:
    /// short, or as the whole input reads.
    #[test]
    fn every_window_where_the_rules_part_is_short_or_reads_as_the_whole() {
        let inputs: [(&[u8], u16); 8] = [
            (
                b"[--5 1.2.3 4294967296 <41G42> (a\rb\r\nc) /#5Gx 3 --1 R]",
                0,
            ),
            (b"[(a)\x80(b) /A\xFF 1 0\x80R --1\x80 %\x80\n2]\x80", 0),
            (
                b"1 0 obj << /Length 2 >>\x80stream\nab\xFFendstream\x80endobj",
                2,
            ),
            (b"<< /A\x80/B 1\xff2 >> 7", 0),
            (b"--3 0 R 4294967299 0 R", 0),
            (
                b"1 0 obj << /Length 2 >> stream\nab\x80endstream\x80endobj",
                2,
            ),
            (b"--1 0 obj 1.2.3 endobj", 0),
            (b"1 0 obj <4%1> endobj", 0),
        ];
        for (input, length) in inputs {
            let read = crate::fuzz::every_window(input, length);
            assert!(read > 0, "{}", String::from_utf8_lossy(input));
        }
    }
}
