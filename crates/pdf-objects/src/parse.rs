//! Stack-safe PDF object and stream parser (ISO 32000-2 §7.3 and PDFium `chromium/8152`).
//!
//! [`Parser`] reads direct values ([`Parser::value`]) and indirect objects ([`Parser::header`],
//! [`Parser::body`], [`Parser::stream`]) from full byte slices, decoded object streams
//! ([`Parser::in_stream`]), or positional file windows ([`Parser::window`]).
//!
//! - **Spans and faults:** Every parsed [`Object`] and dictionary key carries its source [`Span`].
//!   Recoverable syntax departures are reported as [`Fault`]s to a [`Sink`] (pass `&mut ()` to
//!   disable fault work at zero cost, or `&mut Vec<(Span, Fault)>` to collect them).
//! - **Two-step streams:** Because `/Length` on a stream may be an indirect reference or wrong in
//!   damaged files, [`Parser::body`] stops at the first byte of stream data and returns a
//!   [`StreamHead`] offering both [`StreamHead::declared`] and [`StreamHead::scanned`] extents for
//!   [`Parser::stream`].
//! - **Windowed reads:** On non-final windows (`last == false`), any parse whose lookahead touches
//!   the end of the window returns [`ErrorKind::Short`] without advancing [`Parser::offset`], so
//!   retrying with a larger window produces the exact same result as reading the entire file.
//!
//! ```
//! use pdf_objects::parse::{Body, Fault, Parser};
//! use pdf_objects::{Span, Value};
//!
//! let file = b"4 0 obj\n<< /Length 5 >>\nstream\nhello\nendstream\nendobj\n";
//! let mut faults: Vec<(Span, Fault<'_>)> = Vec::new();
//! let mut parser = Parser::at(file, 0);
//! let id = parser.header(&mut faults).unwrap();
//! let Body::Stream(head) = parser.body(id, &mut faults).unwrap() else { panic!() };
//! let data = head.declared(5).unwrap_or_else(|| head.scanned());
//! let stream = parser.stream(head, data, &mut faults).unwrap();
//!
//! assert_eq!(data.slice_in(file), Some(&b"hello"[..]));
//! assert!(matches!(stream.value(), Value::Stream(_)));
//! assert_eq!(parser.offset(), file.len() - 1);
//! assert!(faults.is_empty());
//! ```

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Write as _};
use core::ops::{Deref, Range};
use core::sync::atomic::{AtomicBool, Ordering};

use memchr::memmem;
use memchr::{memchr2, memchr2_iter};
use pdf_tokenizer::decode::pdfium::{self, Number};
use pdf_tokenizer::decode::{self, How, NumberError, Pieces, Quirks};
use pdf_tokenizer::{Iso, Kind, Pdfium, RuleSet, Rules, Token, Tokenizer};

use crate::object::{
    Data, Dict, Entry, Form, Name, Object, ObjectId, SmallBytes, Span, Str, Stream, Value,
};

/// Default maximum container nesting depth ([`Limits::DEFAULT`]).
pub const NESTING: usize = 256;

/// Resource caps enforced by [`Parser`] so untrusted inputs cannot exhaust memory.
///
/// [`Parser`] uses an explicit heap stack, so arbitrary `nesting` values never overflow the
/// call stack. [`serialize`](crate::serialize) enforces [`NESTING`] on output so serialized
/// objects always parse back under [`Limits::DEFAULT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Limits {
    /// Maximum array/dictionary nesting depth (`256` by default).
    pub nesting: usize,
    /// Maximum values (including array elements and dictionary keys + values) read in one call
    /// (`2^25` by default).
    ///
    /// On 64-bit targets (`Object` = 64 B, `Entry` = 112 B, minimum non-empty `Vec` capacity = 4),
    /// worst-case structural overhead is `256` bytes per value (`8 GiB` at `2^25`).
    pub values: usize,
}

impl Limits {
    /// Default parser limits (`nesting = 256`, `values = 2^25`).
    pub const DEFAULT: Limits = Limits {
        nesting: NESTING,
        values: 1 << 25,
    };

    /// Creates [`Limits`] with explicit `nesting` and `values` caps.
    #[must_use]
    pub const fn new(nesting: usize, values: usize) -> Limits {
        Limits { nesting, values }
    }

    /// Returns a copy with `nesting` updated.
    #[must_use]
    pub const fn with_nesting(mut self, nesting: usize) -> Limits {
        self.nesting = nesting;
        self
    }

    /// Returns a copy with `values` updated.
    #[must_use]
    pub const fn with_values(mut self, values: usize) -> Limits {
        self.values = values;
        self
    }
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}

/// Receiver for non-fatal syntax [`Fault`]s encountered by [`Parser`].
///
/// Pass `&mut ()` to discard faults and skip fault-only checks at zero runtime cost, or pass
/// `&mut Vec<(Span, Fault<'a>)>` / `&mut Vec<(Span, OwnedFault)>` to collect them. Use
/// [`FaultSink`] to forward parser faults into a [`crate::fault::Sink`].
pub trait Sink<'a> {
    /// Records `fault` at byte span `at`.
    fn fault(&mut self, at: Span, fault: Fault<'a>);

    /// Returns `true` if fault detection is enabled (`false` for `()`).
    #[must_use]
    fn wants(&self) -> bool {
        true
    }
}

impl Sink<'_> for () {
    #[inline]
    fn fault(&mut self, _: Span, _: Fault<'_>) {}

    #[inline]
    fn wants(&self) -> bool {
        false
    }
}

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

impl<'a> Sink<'a> for Vec<(Span, Fault<'a>)> {
    #[inline]
    fn fault(&mut self, at: Span, fault: Fault<'a>) {
        self.push((at, fault));
    }
}

impl Sink<'_> for Vec<(Span, OwnedFault)> {
    #[inline]
    fn fault(&mut self, at: Span, fault: Fault<'_>) {
        self.push((at, fault.into_owned()));
    }
}

/// Bridges a [`crate::fault::Sink<OwnedFault>`] into a [`Sink`], optionally tagging every
/// [`crate::fault::Place`] with an enclosing [`ObjectId`].
#[derive(Debug)]
pub struct FaultSink<'s, S: ?Sized> {
    object: Option<ObjectId>,
    sink: &'s mut S,
}

impl<'s, S: crate::fault::Sink<OwnedFault> + ?Sized> FaultSink<'s, S> {
    /// Wraps `sink`, tagging emitted places with `object` when `Some`.
    #[must_use]
    pub fn new(object: Option<ObjectId>, sink: &'s mut S) -> Self {
        FaultSink { object, sink }
    }
}

impl<S: crate::fault::Sink<OwnedFault> + ?Sized> Sink<'_> for FaultSink<'_, S> {
    #[inline]
    fn fault(&mut self, at: Span, fault: Fault<'_>) {
        let mut place = crate::fault::Place::from(at);
        if let Some(id) = self.object {
            place = place.in_object(id);
        }
        self.sink.fault(place, fault.into_owned());
    }

    #[inline]
    fn wants(&self) -> bool {
        self.sink.wants()
    }
}

/// Recoverable syntax departure observed and recovered from by [`Parser`].
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Fault<'a> {
    /// Integer literal outside 64-bit signed range; clamped to `i64::MIN` / `i64::MAX` (`Rules::Iso`).
    #[error("an integer outside 64 bits, read as the nearest that fits")]
    IntegerOverflow,
    /// Real literal magnitude exceeds `f64::MAX`; clamped to `f64::MIN` / `f64::MAX` (`Rules::Iso`).
    #[error("a real too large for 64 bits, read as the largest")]
    RealOverflow,
    /// Non-zero real literal underflows `f64` to signed `0.0` (`Rules::Iso`).
    #[error("a real too small for 64 bits, read as 0")]
    RealUnderflow,
    /// Name containing `#` without two hex digits or `#00` (NUL, ISO 32000-2 §7.3.5).
    #[error(
        "the name {written} has {what}, which readers read differently",
        written = first_40(.written),
        what = name_fault(*.stray, *.nul)
    )]
    Name {
        /// Raw name token bytes including leading `/`.
        written: &'a [u8],
        /// `true` if `#` appears without two valid hex digits.
        stray: bool,
        /// `true` if `#00` (escaped NUL) appears.
        nul: bool,
    },
    /// Indirect object header or reference uses object number `0` (ISO 32000-2 §7.3.10 requires
    /// positive object numbers).
    #[error("object number 0 is reserved for the free list and numbers no indirect object")]
    ZeroObjectNumber,
    /// Indirect object header or reference number/generation carries a leading `+` sign.
    #[error(
        "{} in an object identifier has a `+` sign, where ISO 32000-2 requires unsigned digits",
        first_40(.written)
    )]
    SignedObjectId {
        /// Raw signed integer token.
        written: &'a [u8],
    },
    /// Missing end-of-line marker after `stream`.
    #[error("no end of line after `stream`")]
    NoEol,
    /// Horizontal space or tab between `stream` and its end-of-line marker (ISO 32000-2 §7.3.8.1).
    #[error("spaces or tabs between `stream` and its end of line")]
    StreamSpace,
    /// Lone `\r` after `stream` instead of `\r\n` or `\n` (ISO 32000-2 §7.3.8.1).
    #[error("a lone CR after `stream`, where ISO 32000-2 wants CR LF or LF")]
    LoneCr,
    /// Comment or NUL byte between stream data and `endstream`.
    #[error("a comment or NUL byte between a stream's data and `endstream`")]
    BeforeEndstream,
    /// Missing `endobj` keyword after an indirect object or stream.
    #[error("{id} obj has no endobj")]
    NoEndobj {
        /// Enclosing indirect object identifier.
        id: ObjectId,
    },
    /// Token accepted as a number by PDFium (`--5`, `1.2.3`, `+`, `.`) that is a keyword under
    /// ISO 32000-2.
    #[error(
        "{} is a number by PDFium's rules and a keyword to ISO 32000-2",
        first_40(.written)
    )]
    Number {
        /// Raw token bytes.
        written: &'a [u8],
    },
    /// Integer outside PDFium's 32-bit range, read as `0` under [`Rules::Pdfium`].
    #[error(
        "{} is outside the integers PDFium's rules read, and read as 0",
        first_40(.written)
    )]
    OutOfRange {
        /// Raw token bytes.
        written: &'a [u8],
    },
    /// Hex string containing non-hex, non-whitespace bytes skipped under [`Rules::Pdfium`].
    #[error("the hex string {} has a byte that's no hex digit", first_40(.written))]
    HexString {
        /// Raw `<...>` token bytes.
        written: &'a [u8],
    },
    /// Unescaped `\r` inside a literal string preserved as `\r` under [`Rules::Pdfium`] (where
    /// ISO 32000-2 §7.3.4.2 normalizes to `\n`).
    #[error(
        "the string {} has a CR written in it, read as written, where ISO 32000-2 reads LF",
        first_40(.written)
    )]
    StringCr {
        /// Raw `(...)` token bytes.
        written: &'a [u8],
    },
    /// Number or name exceeding PDFium's 256-byte word buffer, truncated to 256 bytes under
    /// [`Rules::Pdfium`].
    #[error(
        "{} is longer than the 256 bytes PDFium's rules read of a word, and read as its first 256",
        first_40(.written)
    )]
    Long {
        /// Full raw token bytes.
        written: &'a [u8],
    },
    /// `0x80` or `0xFF` treated as whitespace inside an object under [`Rules::Pdfium`].
    #[error("{} is white-space here and a regular character to ISO 32000-2", byte_hex(.written))]
    HighByte {
        /// Single-byte slice containing `0x80` or `0xFF`.
        written: &'a [u8],
    },
}

impl Fault<'_> {
    /// Converts a borrowed [`Fault`] into an owned `'static` [`OwnedFault`].
    #[must_use]
    pub fn into_owned(self) -> OwnedFault {
        match self {
            Fault::IntegerOverflow => OwnedFault::IntegerOverflow,
            Fault::RealOverflow => OwnedFault::RealOverflow,
            Fault::RealUnderflow => OwnedFault::RealUnderflow,
            Fault::Name {
                written,
                stray,
                nul,
            } => OwnedFault::Name {
                written: SmallBytes::from_slice(written),
                stray,
                nul,
            },
            Fault::ZeroObjectNumber => OwnedFault::ZeroObjectNumber,
            Fault::SignedObjectId { written } => OwnedFault::SignedObjectId {
                written: SmallBytes::from_slice(written),
            },
            Fault::NoEol => OwnedFault::NoEol,
            Fault::StreamSpace => OwnedFault::StreamSpace,
            Fault::LoneCr => OwnedFault::LoneCr,
            Fault::BeforeEndstream => OwnedFault::BeforeEndstream,
            Fault::NoEndobj { id } => OwnedFault::NoEndobj { id },
            Fault::Number { written } => OwnedFault::Number {
                written: SmallBytes::from_slice(written),
            },
            Fault::OutOfRange { written } => OwnedFault::OutOfRange {
                written: SmallBytes::from_slice(written),
            },
            Fault::HexString { written } => OwnedFault::HexString {
                written: SmallBytes::from_slice(written),
            },
            Fault::StringCr { written } => OwnedFault::StringCr {
                written: SmallBytes::from_slice(written),
            },
            Fault::Long { written } => OwnedFault::Long {
                written: SmallBytes::from_slice(written),
            },
            Fault::HighByte { written } => OwnedFault::HighByte {
                written: SmallBytes::from_slice(written),
            },
        }
    }
}

impl fmt::Debug for Fault<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fault::IntegerOverflow => f.write_str("IntegerOverflow"),
            Fault::RealOverflow => f.write_str("RealOverflow"),
            Fault::RealUnderflow => f.write_str("RealUnderflow"),
            Fault::Name {
                written,
                stray,
                nul,
            } => f
                .debug_struct("Name")
                .field("written", &CappedBytes(written))
                .field("stray", stray)
                .field("nul", nul)
                .finish(),
            Fault::ZeroObjectNumber => f.write_str("ZeroObjectNumber"),
            Fault::SignedObjectId { written } => f
                .debug_struct("SignedObjectId")
                .field("written", &CappedBytes(written))
                .finish(),
            Fault::NoEol => f.write_str("NoEol"),
            Fault::StreamSpace => f.write_str("StreamSpace"),
            Fault::LoneCr => f.write_str("LoneCr"),
            Fault::BeforeEndstream => f.write_str("BeforeEndstream"),
            Fault::NoEndobj { id } => f.debug_struct("NoEndobj").field("id", id).finish(),
            Fault::Number { written } => f
                .debug_struct("Number")
                .field("written", &CappedBytes(written))
                .finish(),
            Fault::OutOfRange { written } => f
                .debug_struct("OutOfRange")
                .field("written", &CappedBytes(written))
                .finish(),
            Fault::HexString { written } => f
                .debug_struct("HexString")
                .field("written", &CappedBytes(written))
                .finish(),
            Fault::StringCr { written } => f
                .debug_struct("StringCr")
                .field("written", &CappedBytes(written))
                .finish(),
            Fault::Long { written } => f
                .debug_struct("Long")
                .field("written", &CappedBytes(written))
                .finish(),
            Fault::HighByte { written } => f
                .debug_struct("HighByte")
                .field("written", &CappedBytes(written))
                .finish(),
        }
    }
}

/// Owned `'static` counterpart of [`Fault`] for retaining diagnostics after dropping input buffers.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum OwnedFault {
    /// Owned [`Fault::IntegerOverflow`].
    #[error("an integer outside 64 bits, read as the nearest that fits")]
    IntegerOverflow,
    /// Owned [`Fault::RealOverflow`].
    #[error("a real too large for 64 bits, read as the largest")]
    RealOverflow,
    /// Owned [`Fault::RealUnderflow`].
    #[error("a real too small for 64 bits, read as 0")]
    RealUnderflow,
    /// Owned [`Fault::Name`].
    #[error(
        "the name {written} has {what}, which readers read differently",
        written = first_40(.written),
        what = name_fault(*.stray, *.nul)
    )]
    Name {
        /// Raw name bytes.
        written: SmallBytes,
        /// `#` without two hex digits.
        stray: bool,
        /// `#00` escape.
        nul: bool,
    },
    /// Owned [`Fault::ZeroObjectNumber`].
    #[error("object number 0 is reserved for the free list and numbers no indirect object")]
    ZeroObjectNumber,
    /// Owned [`Fault::SignedObjectId`].
    #[error(
        "{} in an object identifier has a `+` sign, where ISO 32000-2 requires unsigned digits",
        first_40(.written)
    )]
    SignedObjectId {
        /// Raw signed integer token.
        written: SmallBytes,
    },
    /// Owned [`Fault::NoEol`].
    #[error("no end of line after `stream`")]
    NoEol,
    /// Owned [`Fault::StreamSpace`].
    #[error("spaces or tabs between `stream` and its end of line")]
    StreamSpace,
    /// Owned [`Fault::LoneCr`].
    #[error("a lone CR after `stream`, where ISO 32000-2 wants CR LF or LF")]
    LoneCr,
    /// Owned [`Fault::BeforeEndstream`].
    #[error("a comment or NUL byte between a stream's data and `endstream`")]
    BeforeEndstream,
    /// Owned [`Fault::NoEndobj`].
    #[error("{id} obj has no endobj")]
    NoEndobj {
        /// Enclosing indirect object identifier.
        id: ObjectId,
    },
    /// Owned [`Fault::Number`].
    #[error(
        "{} is a number by PDFium's rules and a keyword to ISO 32000-2",
        first_40(.written)
    )]
    Number {
        /// Raw token bytes.
        written: SmallBytes,
    },
    /// Owned [`Fault::OutOfRange`].
    #[error(
        "{} is outside the integers PDFium's rules read, and read as 0",
        first_40(.written)
    )]
    OutOfRange {
        /// Raw token bytes.
        written: SmallBytes,
    },
    /// Owned [`Fault::HexString`].
    #[error("the hex string {} has a byte that's no hex digit", first_40(.written))]
    HexString {
        /// Raw token bytes.
        written: SmallBytes,
    },
    /// Owned [`Fault::StringCr`].
    #[error(
        "the string {} has a CR written in it, read as written, where ISO 32000-2 reads LF",
        first_40(.written)
    )]
    StringCr {
        /// Raw token bytes.
        written: SmallBytes,
    },
    /// Owned [`Fault::Long`].
    #[error(
        "{} is longer than the 256 bytes PDFium's rules read of a word, and read as its first 256",
        first_40(.written)
    )]
    Long {
        /// Raw token bytes.
        written: SmallBytes,
    },
    /// Owned [`Fault::HighByte`].
    #[error("{} is white-space here and a regular character to ISO 32000-2", byte_hex(.written))]
    HighByte {
        /// Single-byte buffer.
        written: SmallBytes,
    },
}

struct CappedBytes<'a>(&'a [u8]);

impl fmt::Debug for CappedBytes<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("b\"")?;
        let shown = self.0.len().min(64);
        for &b in &self.0[..shown] {
            for esc in core::ascii::escape_default(b) {
                f.write_char(char::from(esc))?;
            }
        }
        f.write_str("\"")?;
        if self.0.len() > shown {
            write!(f, "...({} bytes)", self.0.len())?;
        }
        Ok(())
    }
}

fn first_40(written: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(&written[..written.len().min(40)])
}

fn byte_hex(written: &[u8]) -> String {
    written
        .first()
        .map_or_else(String::new, |b| format!("0x{b:02X}"))
}

fn name_fault(stray: bool, nul: bool) -> &'static str {
    match (stray, nul) {
        (true, true) => "a # without two hex digits after it, and #00, NUL",
        (true, false) => "a # without two hex digits after it",
        _ => "#00, NUL, which a name can't hold",
    }
}

/// Compact 8-byte boxed parser error. Dereferences to [`ErrorKind`].
#[derive(Clone, PartialEq, Eq, thiserror::Error)]
#[error(transparent)]
pub struct Error(Box<ErrorKind>);

impl Error {
    /// Creates an [`Error`] from an [`ErrorKind`].
    #[must_use]
    pub fn new(kind: ErrorKind) -> Error {
        Error(Box::new(kind))
    }

    /// Borrows the underlying [`ErrorKind`].
    #[must_use]
    #[inline]
    pub fn kind(&self) -> &ErrorKind {
        &self.0
    }

    /// Unboxes and returns the [`ErrorKind`].
    #[must_use]
    #[inline]
    pub fn into_kind(self) -> ErrorKind {
        *self.0
    }
}

impl Deref for Error {
    type Target = ErrorKind;

    #[inline]
    fn deref(&self) -> &ErrorKind {
        &self.0
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.0, f)
    }
}

impl From<ErrorKind> for Error {
    #[cold]
    fn from(kind: ErrorKind) -> Error {
        Error::new(kind)
    }
}

impl PartialEq<ErrorKind> for Error {
    fn eq(&self, other: &ErrorKind) -> bool {
        &*self.0 == other
    }
}

impl PartialEq<Error> for ErrorKind {
    fn eq(&self, other: &Error) -> bool {
        self == &*other.0
    }
}

#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(size_of::<Error>() == 8);
};

/// Specific failure reason inside [`Error`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ErrorKind {
    /// Container nesting exceeded [`Limits::nesting`].
    #[error("arrays and dictionaries nested deeper than {limit}")]
    Nested {
        /// Configured nesting cap.
        limit: usize,
        /// Byte offset of the value exceeding the cap.
        at: usize,
    },
    /// Value count in a single call exceeded [`Limits::values`].
    #[error("more than {limit} values in one object, one more at byte {at}")]
    Values {
        /// Configured value cap.
        limit: usize,
        /// Byte offset of the value exceeding the cap.
        at: usize,
    },
    /// Input ended where a value was expected.
    #[error("the data ends where a value should be")]
    End,
    /// Indirect reference `n g R` has `n` outside `u32` or `g` outside `u16`.
    #[error(
        "a reference at byte {at}, {} {} R, numbers no object",
        lossy(.number),
        lossy(.generation)
    )]
    Reference {
        /// Byte offset of the reference.
        at: usize,
        /// Raw object number token.
        number: SmallBytes,
        /// Raw generation token.
        generation: SmallBytes,
    },
    /// Unclosed literal `(...)` or hexadecimal `<...>` string.
    #[error("a string at byte {at} that doesn't end")]
    String {
        /// Byte offset of the opening delimiter.
        at: usize,
    },
    /// Unclosed `[` array.
    #[error("an array at byte {at} that doesn't end")]
    Array {
        /// Byte offset of `[`.
        at: usize,
    },
    /// Unclosed `<<` dictionary.
    #[error("a dictionary at byte {at} that doesn't end")]
    Dict {
        /// Byte offset of `<<`.
        at: usize,
    },
    /// Non-name token where a dictionary `/Key` or `>>` was expected.
    #[error("a dictionary's key at byte {at} isn't a name")]
    Key {
        /// Byte offset of the offending token.
        at: usize,
    },
    /// Unexpected keyword where a value was expected (anything other than `true`, `false`, `null`).
    #[error("`{}` at byte {at}, where a value should be", lossy(.word))]
    Keyword {
        /// Byte offset of the keyword.
        at: usize,
        /// Up to the first 32 bytes of the keyword.
        word: SmallBytes,
    },
    /// Unexpected delimiter token (`]`, `>>`, `{`, `}`, `)`, `>`) where a value was expected.
    #[error("{:?} at byte {at}, where a value should be", lossy(.bytes))]
    Token {
        /// Byte offset of the token.
        at: usize,
        /// Raw token bytes.
        bytes: SmallBytes,
    },
    /// Missing valid `n g obj` indirect object header.
    #[error("no `n g obj` at byte {at}")]
    Header {
        /// Byte offset where the header was sought.
        at: usize,
    },
    /// Missing `endstream` keyword after stream payload.
    #[error("{id} obj's stream has no endstream")]
    Endstream {
        /// Stream object identifier.
        id: ObjectId,
    },
    /// Stream payload [`Span`] passed to [`Parser::stream`] does not match the stream's start,
    /// stream context, or available buffer bounds.
    #[error(
        "{id} obj's stream can't have bytes {start}..{end} as its data: they don't start where its data does, end before they start, run past what's read, or are in other data"
    )]
    Extent {
        /// Stream object identifier.
        id: ObjectId,
        /// Supplied start offset.
        start: usize,
        /// Supplied end offset.
        end: usize,
    },
    /// Parsing reached the end of a non-final window (`last == false`), where reading further
    /// bytes could change the parsed result.
    #[error("the data ends at byte {at}, where more of the file could change what's read")]
    Short {
        /// File byte offset at the end of the window.
        at: usize,
    },
}

fn lossy(bytes: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

/// Stateful, zero-recursion PDF object parser over a byte slice or file window.
///
/// Failed calls (`Err`) leave [`Parser::offset`] unchanged.
///
/// ```compile_fail
/// #![deny(unused_must_use)]
/// use pdf_objects::parse::Parser;
///
/// Parser::at(b"1 0 obj 5 endobj", 0);
/// ```
///
/// Windowed reading example:
///
/// ```
/// use pdf_objects::parse::{ErrorKind, Parser};
///
/// let full = b"100 bytes_before... [12 0 R]";
/// // Window ending right after `12 0` (before ` R]`): returns `Short`.
/// let partial = &full[20..25];
/// let mut p = Parser::at(partial, 0).window(20, false);
/// assert_eq!(p.value(&mut ()).unwrap_err(), ErrorKind::Short { at: 25 });
/// assert_eq!(p.offset(), 20);
///
/// // Retry with the rest of the file (`last = true`).
/// let mut p = Parser::at(&full[20..], 0).window(20, true);
/// let arr = p.value(&mut ()).unwrap();
/// assert_eq!(arr.at().unwrap().range(), 20..28);
/// ```
#[must_use = "a parser is lazy and reads nothing until a method is called"]
#[derive(Clone)]
pub struct Parser<'a> {
    data: &'a [u8],
    offset: usize,
    base: usize,
    last: bool,
    stream: Option<ObjectId>,
    ciphertext: bool,
    limits: Limits,
    rules: Rules,
}

impl fmt::Debug for Parser<'_> {
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
    /// Creates a parser over `data` starting at `offset` (treated as a complete buffer with
    /// `base = 0` and `last = true`).
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

    /// Configures `data` as a window starting at file offset `base` (`last = true` if `data`
    /// reaches EOF).
    pub const fn window(self, base: usize, last: bool) -> Parser<'a> {
        Parser { base, last, ..self }
    }

    /// Marks parsed spans as belonging to decoded object stream `stream`.
    pub const fn in_stream(self, stream: ObjectId) -> Parser<'a> {
        Parser {
            stream: Some(stream),
            ..self
        }
    }

    /// Marks parsed strings as [`Str::Encrypted`] when `ciphertext` is `true`.
    pub const fn ciphertext(self, ciphertext: bool) -> Parser<'a> {
        Parser { ciphertext, ..self }
    }

    /// Sets nesting and value-count [`Limits`].
    pub const fn limits(self, limits: Limits) -> Parser<'a> {
        Parser { limits, ..self }
    }

    /// Selects lexical and token-decoding [`Rules`] ([`Rules::Iso`] by default, or
    /// [`Rules::Pdfium`] as of chromium/8152).
    pub const fn rules(self, rules: Rules) -> Parser<'a> {
        Parser { rules, ..self }
    }

    /// Current read position in file coordinates (`base + slice_offset`).
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.place(self.offset)
    }

    /// Parses the next direct PDF value.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] on syntax errors, resource cap violations, or non-final window
    /// boundaries ([`ErrorKind::Short`]), leaving [`Parser::offset`] unchanged.
    pub fn value(&mut self, sink: &mut (impl Sink<'a> + ?Sized)) -> Result<Object, Error> {
        match self.rules {
            Rules::Pdfium => self.value_by(Pdfium, sink),
            _ => self.value_by(Iso, sink),
        }
    }

    fn value_by<R: RuleSet>(
        &mut self,
        rules: R,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<Object, Error> {
        let mut call = Call::new();
        let mut cursor = Cursor::new(self.data, self.offset, rules);
        let res = self.read_whole(&mut cursor, &mut call, sink);
        self.settled(cursor.touched)?;
        let (value, written) = res?;
        self.told_high(rules, written, sink);
        self.offset = cursor.offset();
        Ok(value)
    }

    /// Parses an indirect object header `n g obj` (ISO 32000-2 §7.3.10).
    ///
    /// Reports [`Fault::ZeroObjectNumber`], [`Fault::SignedObjectId`], and (under
    /// [`Rules::Pdfium`]) number/whitespace faults once a complete header is matched.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Header`] if `n g obj` is absent or [`ErrorKind::Short`] if truncated
    /// by a non-final window, leaving [`Parser::offset`] unchanged.
    pub fn header(&mut self, sink: &mut (impl Sink<'a> + ?Sized)) -> Result<ObjectId, Error> {
        match self.rules {
            Rules::Pdfium => self.header_by(Pdfium, sink),
            _ => self.header_by(Iso, sink),
        }
    }

    fn header_by<R: RuleSet>(
        &mut self,
        rules: R,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<ObjectId, Error> {
        let offset = self.offset;
        let mut cursor = Cursor::new(self.data, offset, rules);
        let parsed = (|| {
            let n = cursor.next_token().filter(|n| n.kind == Kind::Integer)?;
            let g = cursor.next_token().filter(|g| g.kind == Kind::Integer)?;
            let obj = cursor.next_token()?;
            let number = u32::try_from(integer(&n, rules.rules())?).ok()?;
            let generation = u16::try_from(integer(&g, rules.rules())?).ok()?;
            is_keyword(Some(&obj), b"obj").then_some((ObjectId::new(number, generation), n, g, obj))
        })();
        self.settled(cursor.touched)?;
        let Some((id, n, g, obj)) = parsed else {
            return Err(ErrorKind::Header {
                at: self.place(offset),
            }
            .into());
        };
        if sink.wants() {
            let pdfium = rules.rules() == Rules::Pdfium;
            self.emit_id_faults(&n, &g, id, pdfium, sink);
            self.told_high(rules, n.start..obj.end(), sink);
        }
        self.offset = cursor.offset();
        Ok(id)
    }

    /// Parses indirect object `id`'s body after [`Parser::header`].
    ///
    /// For non-stream objects, consumes trailing `endobj` when present (advancing
    /// [`Parser::offset`] past `endobj`, or reporting [`Fault::NoEndobj`] if absent). For streams,
    /// stops at the first byte of stream data and returns [`Body::Stream`].
    ///
    /// # Errors
    ///
    /// Returns [`Error`] on malformed syntax, cap violations, or [`ErrorKind::Short`].
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

    fn body_by<R: RuleSet>(
        &mut self,
        rules: R,
        id: ObjectId,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<Body<'a>, Error> {
        let mut call = Call::new();
        let mut cursor = Cursor::new(self.data, self.offset, rules);
        let body = self.read_body(rules, &mut cursor, &mut call, id, sink);
        self.settled(cursor.touched)?;
        let (body, written, final_offset) = body?;
        self.told_high(rules, written, sink);
        self.offset = final_offset;
        Ok(body)
    }

    /// Finishes reading a stream begun by `head` using the caller-chosen `data` [`Span`]
    /// ([`StreamHead::declared`] or [`StreamHead::scanned`]), consuming `endstream` and (when
    /// present) `endobj`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Extent`] for invalid `data` spans, [`ErrorKind::Endstream`] when
    /// `endstream` does not follow `data`, or [`ErrorKind::Short`] on truncated windows.
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

    fn stream_by<R: RuleSet>(
        &mut self,
        rules: R,
        head: StreamHead<'a>,
        data: Span,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<Object, Error> {
        let mut touched = head.touched.load(Ordering::Relaxed);
        let stream = self.read_stream(rules, head, data, &mut touched, sink);
        self.settled(touched)?;
        let (object, written, final_offset) = stream?;
        self.told_high(rules, written, sink);
        self.offset = final_offset;
        Ok(object)
    }

    const fn place(&self, at: usize) -> usize {
        self.base.saturating_add(at)
    }

    fn span(&self, start: usize, end: usize) -> Span {
        Span::new(self.stream, self.place(start), self.place(end))
    }

    fn settled(&self, touched: bool) -> Result<(), Error> {
        if touched && !self.last {
            Err(ErrorKind::Short {
                at: self.place(self.data.len()),
            }
            .into())
        } else {
            Ok(())
        }
    }

    fn read_stream<R: RuleSet>(
        &self,
        rules: R,
        head: StreamHead<'a>,
        data: Span,
        touched: &mut bool,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<(Object, Range<usize>, usize), Error> {
        let StreamHead {
            id,
            dict,
            dict_at,
            start,
            ..
        } = head;
        let extent = || ErrorKind::Extent {
            id,
            start: data.start(),
            end: data.end(),
        };
        let Some(expected_start) = self.base.checked_add(start) else {
            return Err(extent().into());
        };
        if data.stream() != self.stream
            || data.start() != expected_start
            || data.start() > data.end()
        {
            return Err(extent().into());
        }
        let Some(end) = data.end().checked_sub(self.base) else {
            return Err(extent().into());
        };
        if end > self.data.len() {
            *touched = true;
            return Err(extent().into());
        }

        let mut cursor = Cursor::new(self.data, end, rules);
        let endstream = cursor.next_token();
        *touched |= cursor.touched;
        let endstream = endstream
            .filter(|tok| is_keyword(Some(tok), b"endstream"))
            .ok_or_else(|| Error::from(ErrorKind::Endstream { id }))?;

        if sink.wants() {
            let gap = &self.data[end..endstream.start];
            if memchr2(b'%', 0, gap).is_some() {
                sink.fault(self.span(end, endstream.start), Fault::BeforeEndstream);
            }
        }

        let from = dict_at.map_or(expected_start, |at| at.start());
        let at = Span::new(self.stream, from, self.place(endstream.end()));
        let object = Object::from(Stream {
            dict,
            data: Data::At(data),
        })
        .with_span(at);

        let after_endstream = cursor.offset();
        let final_offset = self.consume_endobj(&mut cursor, id, &object, sink);
        *touched |= cursor.touched;
        Ok((object, end..after_endstream, final_offset))
    }

    fn read_body<R: RuleSet>(
        &self,
        rules: R,
        cursor: &mut Cursor<'a, R>,
        call: &mut Call,
        id: ObjectId,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<(Body<'a>, Range<usize>, usize), Error> {
        let (value, written) = self.read_whole(cursor, call, sink)?;
        let at = value.at();
        if let Value::Dict(_) = value.value()
            && is_keyword(cursor.peek0().as_ref(), b"stream")
        {
            let keyword_tok = cursor.next_token().expect("peeked stream keyword");
            let Value::Dict(dict) = value.into_value() else {
                unreachable!()
            };
            let keyword = self.span(keyword_tok.start, keyword_tok.end());
            let after_kw = cursor.offset();
            let stream_start = self.read_stream_eol(rules, after_kw, keyword, cursor, sink);
            let head = StreamHead {
                id,
                dict,
                dict_at: at,
                keyword,
                start: stream_start,
                data: self.data,
                base: self.base,
                stream: self.stream,
                rules: self.rules,
                touched: AtomicBool::new(false),
            };
            return Ok((
                Body::Stream(head),
                written.start..stream_start,
                stream_start,
            ));
        }

        let final_offset = self.consume_endobj(cursor, id, &value, sink);
        Ok((Body::Object(value), written, final_offset))
    }

    fn read_stream_eol<R: RuleSet>(
        &self,
        rules: R,
        after_kw: usize,
        keyword: Span,
        cursor: &mut Cursor<'a, R>,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> usize {
        let tail = self.data.get(after_kw..).unwrap_or_default();
        let spaces = tail
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();

        if spaces > 0 {
            let after_spaces = after_kw + spaces;
            if after_spaces >= self.data.len() {
                cursor.touched = true;
                if sink.wants() {
                    sink.fault(keyword, Fault::NoEol);
                }
                return after_kw;
            }
            if matches!(self.data[after_spaces], b'\n' | b'\r') {
                if sink.wants() {
                    sink.fault(self.span(after_kw, after_spaces), Fault::StreamSpace);
                }
                let mut t = Tokenizer::at(self.data, after_spaces).with_rules(rules);
                let eol = t.eol();
                cursor.touched |= t.offset() >= self.data.len();
                if let Some(eol) = eol
                    && eol.bytes == b"\r"
                    && sink.wants()
                {
                    sink.fault(self.span(eol.start, eol.end()), Fault::LoneCr);
                }
                return t.offset();
            }
            // Non-EOL byte after space(s): spaces are stream payload, and EOL is missing.
            if sink.wants() {
                sink.fault(keyword, Fault::NoEol);
            }
            return after_kw;
        }

        let mut t = Tokenizer::at(self.data, after_kw).with_rules(rules);
        let eol = t.eol();
        cursor.touched |= t.offset() >= self.data.len();
        match eol {
            _ if !sink.wants() => {}
            None => sink.fault(keyword, Fault::NoEol),
            Some(eol) if eol.bytes == b"\r" => {
                sink.fault(self.span(eol.start, eol.end()), Fault::LoneCr);
            }
            Some(_) => {}
        }
        t.offset()
    }

    fn consume_endobj<R: RuleSet>(
        &self,
        cursor: &mut Cursor<'a, R>,
        id: ObjectId,
        object: &Object,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> usize {
        if is_keyword(cursor.peek0().as_ref(), b"endobj") {
            let _ = cursor.next_token();
            cursor.offset()
        } else {
            if let Some(at) = object.at().filter(|_| sink.wants()) {
                sink.fault(at, Fault::NoEndobj { id });
            }
            cursor.offset()
        }
    }

    fn read_whole<R: RuleSet>(
        &self,
        cursor: &mut Cursor<'a, R>,
        call: &mut Call,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<(Object, Range<usize>), Error> {
        let mut token = cursor
            .next_token()
            .ok_or_else(|| Error::from(ErrorKind::End))?;
        let first_start = token.start;
        let rules = cursor.rules();
        let pdfium = rules == Rules::Pdfium;
        let mut stack: Vec<Frame> = Vec::new();

        'parse_token: loop {
            let depth = stack.len();
            if starts_value(&token) {
                if depth > self.limits.nesting {
                    return Err(ErrorKind::Nested {
                        limit: self.limits.nesting,
                        at: self.place(token.start),
                    }
                    .into());
                }
                self.count(call, token.start)?;
            }

            let mut completed = match token.kind {
                Kind::ArrayOpen => {
                    let open_start = token.start;
                    match cursor.peek0() {
                        Some(close) if close.kind == Kind::ArrayClose => {
                            let close = cursor.next_token().expect("peeked ]");
                            Object::new(Value::Array(Vec::new()))
                                .with_span(self.span(open_start, close.end()))
                        }
                        Some(_) => {
                            stack.push(Frame::Array {
                                open_start,
                                elements: Vec::new(),
                            });
                            token = cursor.next_token().expect("peeked element");
                            continue 'parse_token;
                        }
                        None => {
                            return Err(ErrorKind::Array {
                                at: self.place(open_start),
                            }
                            .into());
                        }
                    }
                }
                Kind::DictOpen => {
                    let open_start = token.start;
                    let dict = Dict::new();
                    let key = cursor.next_token().ok_or_else(|| {
                        Error::from(ErrorKind::Dict {
                            at: self.place(open_start),
                        })
                    })?;
                    match key.kind {
                        Kind::DictClose => Object::new(Value::Dict(dict))
                            .with_span(self.span(open_start, key.end())),
                        Kind::Name => {
                            self.count(call, key.start)?;
                            let pending_key = self.name(&key, pdfium, sink);
                            let pending_key_at = self.span(key.start, key.end());
                            let val_tok = cursor
                                .next_token()
                                .ok_or_else(|| Error::from(ErrorKind::End))?;
                            stack.push(Frame::Dict {
                                open_start,
                                dict,
                                pending_key,
                                pending_key_at,
                            });
                            token = val_tok;
                            continue 'parse_token;
                        }
                        _ => {
                            return Err(ErrorKind::Key {
                                at: self.place(key.start),
                            }
                            .into());
                        }
                    }
                }
                _ => self.parse_leaf(token, cursor, rules, pdfium, sink)?,
            };

            loop {
                match stack.last_mut() {
                    None => return Ok((completed, first_start..cursor.offset())),
                    Some(Frame::Array {
                        open_start,
                        elements,
                    }) => {
                        elements.push(completed);
                        match cursor.peek0() {
                            Some(close) if close.kind == Kind::ArrayClose => {
                                let close = cursor.next_token().expect("peeked ]");
                                let Some(Frame::Array {
                                    open_start,
                                    elements,
                                }) = stack.pop()
                                else {
                                    unreachable!()
                                };
                                completed = Object::new(Value::Array(elements))
                                    .with_span(self.span(open_start, close.end()));
                            }
                            Some(_) => {
                                token = cursor.next_token().expect("peeked element");
                                continue 'parse_token;
                            }
                            None => {
                                let open = *open_start;
                                return Err(ErrorKind::Array {
                                    at: self.place(open),
                                }
                                .into());
                            }
                        }
                    }
                    Some(Frame::Dict { .. }) => {
                        let Some(Frame::Dict {
                            open_start,
                            mut dict,
                            pending_key,
                            pending_key_at,
                        }) = stack.pop()
                        else {
                            unreachable!()
                        };
                        dict.push_entry(Entry {
                            key: pending_key,
                            key_at: Some(pending_key_at),
                            value: completed,
                        });
                        let key = cursor.next_token().ok_or_else(|| {
                            Error::from(ErrorKind::Dict {
                                at: self.place(open_start),
                            })
                        })?;
                        match key.kind {
                            Kind::DictClose => {
                                completed = Object::new(Value::Dict(dict))
                                    .with_span(self.span(open_start, key.end()));
                            }
                            Kind::Name => {
                                self.count(call, key.start)?;
                                let next_key = self.name(&key, pdfium, sink);
                                let next_key_at = self.span(key.start, key.end());
                                let val_tok = cursor
                                    .next_token()
                                    .ok_or_else(|| Error::from(ErrorKind::End))?;
                                stack.push(Frame::Dict {
                                    open_start,
                                    dict,
                                    pending_key: next_key,
                                    pending_key_at: next_key_at,
                                });
                                token = val_tok;
                                continue 'parse_token;
                            }
                            _ => {
                                return Err(ErrorKind::Key {
                                    at: self.place(key.start),
                                }
                                .into());
                            }
                        }
                    }
                }
            }
        }
    }

    fn parse_leaf<R: RuleSet>(
        &self,
        token: Token<'a>,
        cursor: &mut Cursor<'a, R>,
        rules: Rules,
        pdfium: bool,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<Object, Error> {
        let at = self.span(token.start, token.end());
        let value = match token.kind {
            Kind::Integer => {
                if let Some(g) = cursor.peek0().filter(|g| g.kind == Kind::Integer)
                    && let Some(r) = cursor.peek1().filter(|r| is_keyword(Some(r), b"R"))
                {
                    let number = integer(&token, rules).and_then(|n| u32::try_from(n).ok());
                    let generation = integer(&g, rules).and_then(|gv| u16::try_from(gv).ok());
                    let (Some(number), Some(generation)) = (number, generation) else {
                        return Err(ErrorKind::Reference {
                            at: self.place(token.start),
                            number: SmallBytes::from_slice(token.bytes),
                            generation: SmallBytes::from_slice(g.bytes),
                        }
                        .into());
                    };
                    let _ = cursor.next_token();
                    let _ = cursor.next_token();
                    let id = ObjectId::new(number, generation);
                    if sink.wants() {
                        self.emit_id_faults(&token, &g, id, pdfium, sink);
                    }
                    let ref_at = self.span(token.start, r.end());
                    return Ok(Object::new(Value::Ref(id)).with_span(ref_at));
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
                return Err(ErrorKind::String {
                    at: self.place(token.start),
                }
                .into());
            }
            Kind::LiteralString | Kind::HexString => {
                let form = match token.kind {
                    Kind::HexString => Form::Hex,
                    _ => Form::Literal,
                };
                let bytes = if pdfium {
                    self.pdfium_string(&token, sink)
                } else {
                    decoded(&token)
                };
                let s = if self.ciphertext {
                    Str::Encrypted(bytes)
                } else {
                    Str::Bytes(bytes)
                };
                Value::String(s, Some(form))
            }
            Kind::Keyword => match token.bytes {
                b"true" => Value::Bool(true),
                b"false" => Value::Bool(false),
                b"null" => Value::Null,
                word => {
                    return Err(ErrorKind::Keyword {
                        at: self.place(token.start),
                        word: SmallBytes::from_slice(&word[..word.len().min(32)]),
                    }
                    .into());
                }
            },
            _ => {
                return Err(ErrorKind::Token {
                    at: self.place(token.start),
                    bytes: SmallBytes::from_slice(token.bytes),
                }
                .into());
            }
        };
        Ok(Object::new(value).with_span(at))
    }

    fn emit_id_faults(
        &self,
        n: &Token<'a>,
        g: &Token<'a>,
        id: ObjectId,
        pdfium: bool,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) {
        if pdfium {
            self.told_number(n, sink);
            self.told_number(g, sink);
        }
        let n_iso = decode::integer(n.bytes).ok();
        let g_iso = decode::integer(g.bytes).ok();
        if n.bytes.starts_with(b"+") && n_iso == Some(i64::from(id.number)) {
            sink.fault(
                self.span(n.start, n.end()),
                Fault::SignedObjectId { written: n.bytes },
            );
        }
        if id.number == 0 && n_iso == Some(0) {
            sink.fault(self.span(n.start, n.end()), Fault::ZeroObjectNumber);
        }
        if g.bytes.starts_with(b"+") && g_iso == Some(i64::from(id.generation)) {
            sink.fault(
                self.span(g.start, g.end()),
                Fault::SignedObjectId { written: g.bytes },
            );
        }
    }

    #[inline]
    fn count(&self, call: &mut Call, at: usize) -> Result<(), Error> {
        call.values += 1;
        if call.values > self.limits.values {
            Err(ErrorKind::Values {
                limit: self.limits.values,
                at: self.place(at),
            }
            .into())
        } else {
            Ok(())
        }
    }

    fn told_high<R: RuleSet>(
        &self,
        rules: R,
        written: Range<usize>,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) {
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

    fn name(&self, token: &Token<'a>, pdfium: bool, sink: &mut (impl Sink<'a> + ?Sized)) -> Name {
        if pdfium {
            return self.pdfium_name(token, sink);
        }
        if !token.bytes.contains(&b'#') {
            return Name::from(token.bytes.get(1..).unwrap_or_default());
        }
        if sink.wants() {
            self.told_name(token, sink);
        }
        Name::from_small_bytes(decoded(token))
    }

    fn told_name(&self, token: &Token<'a>, sink: &mut (impl Sink<'a> + ?Sized)) {
        let quirks = token.pieces().map_or(Quirks::default(), Pieces::quirks);
        let (stray, nul) = (
            quirks.contains(How::StrayHash),
            quirks.contains(How::NulEscape),
        );
        if stray || nul {
            sink.fault(
                self.span(token.start, token.end()),
                Fault::Name {
                    written: token.bytes,
                    stray,
                    nul,
                },
            );
        }
    }

    fn pdfium_name(&self, token: &Token<'a>, sink: &mut (impl Sink<'a> + ?Sized)) -> Name {
        if sink.wants() {
            if token.bytes.contains(&b'#') {
                self.told_name(token, sink);
            }
            self.told_long(token, sink);
        }
        let word = kept(token.bytes);
        if !word.contains(&b'#') {
            return Name::from(word.get(1..).unwrap_or_default());
        }
        let bytes = pdfium::name(word);
        let cap = bytes.size_hint().1.unwrap_or(0);
        Name::from_small_bytes(SmallBytes::from_iter_bounded(bytes, cap))
    }

    fn pdfium_number(&self, token: &Token<'a>, sink: &mut (impl Sink<'a> + ?Sized)) -> Value {
        if sink.wants() {
            self.told_number(token, sink);
        }
        pdfium_number(token.bytes)
    }

    fn told_number(&self, token: &Token<'a>, sink: &mut (impl Sink<'a> + ?Sized)) {
        let written = token.bytes;
        let fault = match decode::integer(written) {
            Ok(n) => {
                (wide(pdfium::number(written)) != Some(n)).then_some(Fault::OutOfRange { written })
            }
            Err(NumberError::Overflow) => Some(Fault::OutOfRange { written }),
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

    fn told_long(&self, token: &Token<'a>, sink: &mut (impl Sink<'a> + ?Sized)) {
        if token.bytes.len() > WORD {
            sink.fault(
                self.span(token.start, token.end()),
                Fault::Long {
                    written: token.bytes,
                },
            );
        }
    }

    fn pdfium_string(&self, token: &Token<'a>, sink: &mut (impl Sink<'a> + ?Sized)) -> SmallBytes {
        if let Some(value) = token.verbatim() {
            return SmallBytes::from_slice(value);
        }
        let bytes = match token.kind {
            Kind::HexString => pdfium::hex(token.bytes),
            _ => pdfium::literal(token.bytes),
        };
        if sink.wants() {
            self.told_string(token, bytes.clone(), sink);
        }
        let cap = match token.kind {
            Kind::HexString => token.bytes.len() / 2,
            _ => bytes.size_hint().1.unwrap_or(0),
        };
        SmallBytes::from_iter_bounded(bytes, cap)
    }

    fn told_string(
        &self,
        token: &Token<'a>,
        bytes: pdfium::Bytes<'_>,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) {
        let written = token.bytes;
        let fault = match token.kind {
            Kind::HexString => {
                let inner = written.get(1..written.len().saturating_sub(1));
                let stray = |b: &u8| !b.is_ascii_hexdigit() && !pdf_tokenizer::is_white_space(*b);
                inner
                    .unwrap_or_default()
                    .iter()
                    .any(stray)
                    .then_some(Fault::HexString { written })
            }
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

enum Frame {
    Array {
        open_start: usize,
        elements: Vec<Object>,
    },
    Dict {
        open_start: usize,
        dict: Dict,
        pending_key: Name,
        pending_key_at: Span,
    },
}

const WORD: usize = 256;

fn kept(word: &[u8]) -> &[u8] {
    &word[..word.len().min(WORD)]
}

fn pdfium_number(word: &[u8]) -> Value {
    let word = kept(word);
    match pdfium::number(word) {
        Number::Unsigned(n) => Value::Integer(n.into()),
        Number::Signed(n) => Value::Integer(n.into()),
        Number::Real(_) => Value::Real(pdfium::real(word)),
    }
}

fn wide(number: Number) -> Option<i64> {
    match number {
        Number::Unsigned(n) => Some(n.into()),
        Number::Signed(n) => Some(n.into()),
        Number::Real(_) => None,
    }
}

fn integer(token: &Token<'_>, rules: Rules) -> Option<i64> {
    match rules {
        Rules::Pdfium => match pdfium_number(token.bytes) {
            Value::Integer(n) => Some(n),
            _ => None,
        },
        _ => token.integer().ok(),
    }
}

/// Result of [`Parser::body`]: either a finished non-stream [`Object`] or a [`StreamHead`] ready
/// for [`Parser::stream`].
#[derive(Debug, Clone)]
pub enum Body<'a> {
    /// Non-stream indirect object.
    Object(Object),
    /// Stream dictionary and start offset awaiting extent selection.
    Stream(StreamHead<'a>),
}

/// Partially parsed indirect stream object positioned at the first byte of stream data.
#[must_use = "pass StreamHead to Parser::stream with StreamHead::declared or StreamHead::scanned"]
pub struct StreamHead<'a> {
    id: ObjectId,
    dict: Dict,
    dict_at: Option<Span>,
    keyword: Span,
    start: usize,
    data: &'a [u8],
    base: usize,
    stream: Option<ObjectId>,
    rules: Rules,
    touched: AtomicBool,
}

impl fmt::Debug for StreamHead<'_> {
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
    /// Borrows the parsed stream dictionary.
    #[must_use]
    pub const fn dict(&self) -> &Dict {
        &self.dict
    }

    /// Source [`Span`] of the `stream` keyword.
    #[must_use]
    pub const fn keyword(&self) -> Span {
        self.keyword
    }

    /// File (or object-stream) byte offset where stream payload data starts.
    #[must_use]
    pub const fn start(&self) -> usize {
        self.base.saturating_add(self.start)
    }

    /// Returns the `length`-byte payload [`Span`] if `endstream` immediately follows (past
    /// whitespace/comments under the parser's [`Rules`]).
    #[must_use]
    pub fn declared(&self, length: usize) -> Option<Span> {
        let end = self.start.checked_add(length)?;
        let file_start = self.base.checked_add(self.start)?;
        let file_end = self.base.checked_add(end)?;
        if end > self.data.len() {
            self.touched.store(true, Ordering::Relaxed);
            return None;
        }
        let (after, touched) = match self.rules {
            Rules::Pdfium => {
                let mut t = Tokenizer::at(self.data, end).with_rules(Pdfium);
                let tok = t.next_significant();
                (tok, t.offset() >= self.data.len())
            }
            _ => {
                let mut t = Tokenizer::at(self.data, end);
                let tok = t.next_significant();
                (tok, t.offset() >= self.data.len())
            }
        };
        if touched {
            self.touched.store(true, Ordering::Relaxed);
        }
        is_keyword(after.as_ref(), b"endstream")
            .then(|| Span::new(self.stream, file_start, file_end))
    }

    /// Scans forward from [`StreamHead::start`] for the next standalone `endstream` keyword,
    /// skipping false matches followed by a regular character (such as `endstreamX`) and stripping
    /// at most one preceding `\r\n` or `\n` stream-framing EOL.
    #[must_use]
    pub fn scanned(&self) -> Span {
        let mut search_from = self.start.min(self.data.len());
        while let Some(rel) = memmem::find(&self.data[search_from..], b"endstream") {
            let pos = search_from + rel;
            let after = pos + b"endstream".len();
            if after >= self.data.len() {
                self.touched.store(true, Ordering::Relaxed);
                return self.make_span(self.start, self.strip_preceding_eol(pos));
            }
            let next = self.data[after];
            if self.rules.is_regular(next) {
                search_from = pos + 1;
                continue;
            }
            return self.make_span(self.start, self.strip_preceding_eol(pos));
        }
        self.touched.store(true, Ordering::Relaxed);
        self.make_span(self.start, self.data.len())
    }

    fn strip_preceding_eol(&self, endstream_pos: usize) -> usize {
        if endstream_pos > self.start && self.data[endstream_pos - 1] == b'\n' {
            if endstream_pos - 1 > self.start && self.data[endstream_pos - 2] == b'\r' {
                endstream_pos - 2
            } else {
                endstream_pos - 1
            }
        } else if endstream_pos > self.start && self.data[endstream_pos - 1] == b'\r' {
            endstream_pos - 1
        } else {
            endstream_pos
        }
    }

    fn make_span(&self, start: usize, end: usize) -> Span {
        let s = self.base.saturating_add(start);
        let e = self.base.saturating_add(end).max(s);
        Span::new(self.stream, s, e)
    }
}

struct Call {
    values: usize,
}

impl Call {
    const fn new() -> Call {
        Call { values: 0 }
    }
}

struct Buffered<'a> {
    token: Option<Token<'a>>,
    after_offset: usize,
    touched: bool,
}

struct Cursor<'a, R: RuleSet> {
    t: Tokenizer<'a, R>,
    end: usize,
    logical_offset: usize,
    touched: bool,
    peek0: Option<Buffered<'a>>,
    peek1: Option<Buffered<'a>>,
}

impl<'a, R: RuleSet> Cursor<'a, R> {
    fn new(data: &'a [u8], offset: usize, rules: R) -> Self {
        Cursor {
            t: Tokenizer::at(data, offset).with_rules(rules),
            end: data.len(),
            logical_offset: offset,
            touched: false,
            peek0: None,
            peek1: None,
        }
    }

    #[inline]
    fn rules(&self) -> Rules {
        self.t.rules()
    }

    #[inline]
    fn offset(&self) -> usize {
        self.logical_offset
    }

    #[inline]
    fn pull_raw(&mut self) -> Buffered<'a> {
        let token = self.t.next_significant();
        let after_offset = self.t.offset();
        let touched = after_offset >= self.end;
        Buffered {
            token,
            after_offset,
            touched,
        }
    }

    #[inline]
    fn next_token(&mut self) -> Option<Token<'a>> {
        let b = match self.peek0.take() {
            Some(b) => {
                self.peek0 = self.peek1.take();
                b
            }
            None => self.pull_raw(),
        };
        self.logical_offset = b.after_offset;
        self.touched |= b.touched;
        b.token
    }

    #[inline]
    fn peek0(&mut self) -> Option<Token<'a>> {
        if self.peek0.is_none() {
            let b = self.pull_raw();
            self.peek0 = Some(b);
        }
        let b = self.peek0.as_ref().expect("populated peek0");
        self.touched |= b.touched;
        b.token
    }

    #[inline]
    fn peek1(&mut self) -> Option<Token<'a>> {
        let _ = self.peek0();
        if self.peek1.is_none() {
            let b = self.pull_raw();
            self.peek1 = Some(b);
        }
        let b = self.peek1.as_ref().expect("populated peek1");
        self.touched |= b.touched;
        b.token
    }
}

#[inline]
fn starts_value(token: &Token<'_>) -> bool {
    match token.kind {
        Kind::Integer | Kind::Real | Kind::Name | Kind::ArrayOpen | Kind::DictOpen => true,
        Kind::LiteralString | Kind::HexString => token.closed,
        Kind::Keyword => matches!(token.bytes, b"true" | b"false" | b"null"),
        _ => false,
    }
}

fn is_keyword(token: Option<&Token<'_>>, word: &[u8]) -> bool {
    token.is_some_and(|t| t.is_keyword(word))
}

fn decoded(token: &Token<'_>) -> SmallBytes {
    if let Some(value) = token.verbatim() {
        return SmallBytes::from_slice(value);
    }
    let Some(pieces) = token.pieces() else {
        return SmallBytes::EMPTY;
    };
    let bytes = pieces.bytes();
    let cap = match token.kind {
        Kind::HexString => token.bytes.len() / 2,
        _ => bytes.size_hint().1.unwrap_or(0),
    };
    SmallBytes::from_iter_bounded(bytes, cap)
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::vec;

    use super::*;
    use crate::fuzz::Kept;

    fn value(input: &[u8]) -> (Result<Object, Error>, Kept) {
        let mut kept = Kept::default();
        let value = Parser::at(input, 0).value(&mut kept);
        (value, kept)
    }

    fn indirect<'a>(
        input: &'a [u8],
        choose: impl FnOnce(&StreamHead<'a>) -> Span,
    ) -> (Result<Object, Error>, Kept) {
        let mut kept = Kept::default();
        let mut parser = Parser::at(input, 0);
        let object = parser
            .header(&mut kept)
            .and_then(|id| match parser.body(id, &mut kept)? {
                Body::Object(object) => Ok(object),
                Body::Stream(head) => {
                    let data = choose(&head);
                    parser.stream(head, data, &mut kept)
                }
            });
        (object, kept)
    }

    fn bytes(input: &[u8], at: Span) -> &[u8] {
        &input[at.start()..at.end()]
    }

    fn found(input: &[u8], needle: &[u8]) -> Span {
        let start = input
            .windows(needle.len())
            .position(|w| w == needle)
            .unwrap();
        Span::new(None, start, start + needle.len())
    }

    #[test]
    fn debug_redacts_input_data_and_caps_fault_bytes() {
        let mut file = b"4 0 obj\n<< /Length 5 >>\nstream\nhello\nendstream\nendobj\n".to_vec();
        file.resize(4000, b' ');
        let mut parser = Parser::at(&file, 0);
        let limits = Limits::DEFAULT;
        let shown = format!(
            "Parser {{ offset: 0, data: 4000 bytes, base: 0, last: true, stream: None, \
             ciphertext: false, limits: {limits:?}, rules: Iso }}"
        );
        assert_eq!(format!("{parser:?}"), shown);
        let id = parser.header(&mut ()).unwrap();
        let Ok(Body::Stream(head)) = parser.body(id, &mut ()) else {
            panic!("expected stream");
        };
        let head_dbg = format!("{head:?}");
        assert_eq!(
            head_dbg,
            "StreamHead { id: ObjectId { number: 4, generation: 0 }, start: 31, data: 4000 bytes, .. }"
        );
        assert!(!head_dbg.contains("hello"));

        let huge = vec![b'a'; 5000];
        let f = Fault::Long { written: &huge };
        let f_dbg = format!("{f:?}");
        assert!(f_dbg.len() < 150, "{f_dbg}");
        assert!(f_dbg.contains("...(5000 bytes)"));
    }

    #[test]
    fn sink_vec_and_fault_sink_bridge() {
        let input = b"<< /Bad#zz 99999999999999999999 >>";
        let mut borrowed: Vec<(Span, Fault<'_>)> = Vec::new();
        Parser::at(input, 0).value(&mut borrowed).unwrap();
        assert_eq!(borrowed.len(), 2);
        // Key fault is reported in monotonic byte order before value fault!
        assert!(borrowed[0].0.start() < borrowed[1].0.start());
        assert!(matches!(borrowed[0].1, Fault::Name { stray: true, .. }));
        assert_eq!(borrowed[1].1, Fault::IntegerOverflow);

        let mut owned: Vec<(Span, OwnedFault)> = Vec::new();
        Parser::at(input, 0).value(&mut owned).unwrap();
        assert_eq!(owned.len(), 2);
        assert_eq!(owned[1].1, OwnedFault::IntegerOverflow);

        // Forward into crate::fault::Sink via FaultSink.
        struct DomainSink(Vec<(crate::fault::Place, OwnedFault)>);
        impl crate::fault::Sink<OwnedFault> for DomainSink {
            fn fault(&mut self, place: crate::fault::Place, fault: OwnedFault) {
                self.0.push((place, fault));
            }
        }
        let mut ds = DomainSink(Vec::new());
        let mut bridge = FaultSink::new(Some(ObjectId::new(9, 0)), &mut ds);
        Parser::at(input, 0).value(&mut bridge).unwrap();
        assert_eq!(ds.0.len(), 2);
        assert_eq!(ds.0[0].0.object, Some(ObjectId::new(9, 0)));

        // Disabled crate::fault::Sink (`()`) disables wants().
        assert!(!FaultSink::new(None, &mut ()).wants());
    }

    #[test]
    fn faults_and_header_faults_reported_accurately() {
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
            let (val, kept) = value(input);
            assert!(val.is_ok());
            let want = match said {
                "" => Vec::new(),
                s => vec![(found(input, written.as_bytes()), s.to_string())],
            };
            assert_eq!(kept.faults, want);
        }

        // 0 0 obj / 0 0 R and +1 +0 obj / +1 +0 R faults.
        let (_, k_zero_ref) = value(b"0 0 R");
        assert_eq!(k_zero_ref.faults.len(), 1);
        assert!(k_zero_ref.faults[0].1.contains("object number 0"));

        let (_, k_plus_ref) = value(b"+3 +0 R");
        assert_eq!(k_plus_ref.faults.len(), 2);

        let (_, k_hdr) = indirect(b"+0 +1 obj 5 endobj", StreamHead::scanned);
        assert_eq!(k_hdr.faults.len(), 3); // +0, 0, +1

        // Horizontal spaces before stream EOL vs missing EOL vs comment before endstream.
        let (s_space, k_space) = indirect(
            b"1 0 obj << /Length 2 >> stream  \r\nab\nendstream endobj",
            |h| h.declared(2).unwrap(),
        );
        assert_eq!(
            s_space
                .unwrap()
                .as_stream()
                .unwrap()
                .data
                .at()
                .unwrap()
                .range(),
            34..36
        );
        assert_eq!(k_space.faults.len(), 1);
        assert_eq!(
            k_space.faults[0].1,
            "spaces or tabs between `stream` and its end of line"
        );

        let (_, k_comment) = indirect(
            b"1 0 obj << /Length 2 >> stream\nab\n%comment\nendstream endobj",
            |h| h.declared(2).unwrap(),
        );
        assert_eq!(k_comment.faults.len(), 1);
        assert_eq!(
            k_comment.faults[0].1,
            "a comment or NUL byte between a stream's data and `endstream`"
        );

        // `stream xy` without EOL preserves `' '` in stream data and emits NoEol.
        let (s_no_eol, k_no_eol) = indirect(
            b"1 0 obj << >>stream xy\nendstream\nendobj",
            StreamHead::scanned,
        );
        assert_eq!(
            bytes(
                b"1 0 obj << >>stream xy\nendstream\nendobj",
                s_no_eol.unwrap().as_stream().unwrap().data.at().unwrap()
            ),
            b" xy"
        );
        assert_eq!(k_no_eol.faults[0].1, "no end of line after `stream`");
    }

    #[test]
    fn body_and_stream_consume_endobj_when_present() {
        let seq = b"1 0 obj [1 2] endobj\n2 0 obj << /Length 3 >> stream\nabc\nendstream endobj\n3 0 obj 99";
        let mut p = Parser::at(seq, 0);
        let mut faults: Vec<(Span, Fault<'_>)> = Vec::new();

        let id1 = p.header(&mut faults).unwrap();
        assert_eq!(id1, ObjectId::new(1, 0));
        let Body::Object(o1) = p.body(id1, &mut faults).unwrap() else {
            panic!()
        };
        assert_eq!(bytes(seq, o1.at().unwrap()), b"[1 2]");
        assert_eq!(p.offset(), 20); // right after first `endobj`

        let id2 = p.header(&mut faults).unwrap();
        assert_eq!(id2, ObjectId::new(2, 0));
        let Body::Stream(head) = p.body(id2, &mut faults).unwrap() else {
            panic!()
        };
        let data = head.declared(3).unwrap();
        let o2 = p.stream(head, data, &mut faults).unwrap();
        assert_eq!(
            bytes(seq, o2.at().unwrap()),
            b"<< /Length 3 >> stream\nabc\nendstream"
        );
        assert!(faults.is_empty());

        // Missing endobj on object 3: leaves offset right after `99` and reports NoEndobj.
        let id3 = p.header(&mut faults).unwrap();
        assert_eq!(id3, ObjectId::new(3, 0));
        let Body::Object(o3) = p.body(id3, &mut faults).unwrap() else {
            panic!()
        };
        assert_eq!(o3.as_integer(), Some(99));
        assert_eq!(faults.len(), 1);
        assert!(matches!(faults[0].1, Fault::NoEndobj { id } if id == id3));
    }

    #[test]
    fn scanned_skips_endstream_prefix_and_strips_single_framing_eol() {
        // `endstreamX` inside stream payload is NOT a standalone `endstream` keyword!
        let input = b"1 0 obj << /Length 999 >> stream\nfoo endstreamX bar\r\nendstream\nendobj";
        let (obj, faults) = indirect(input, StreamHead::scanned);
        assert!(faults.faults.is_empty());
        let data_span = obj.unwrap().as_stream().unwrap().data.at().unwrap();
        assert_eq!(bytes(input, data_span), b"foo endstreamX bar");

        // Double newline before `endstream`: only the framing newline is stripped!
        let two_nl = b"1 0 obj << >> stream\nhi\n\nendstream endobj";
        let (obj2, _) = indirect(two_nl, StreamHead::scanned);
        assert_eq!(
            bytes(
                two_nl,
                obj2.unwrap().as_stream().unwrap().data.at().unwrap()
            ),
            b"hi\n"
        );

        // Empty stream `stream\n\nendstream`: clamped to >= start, yielding b"".
        let empty = b"1 0 obj << >> stream\n\nendstream endobj";
        let (obj3, _) = indirect(empty, StreamHead::scanned);
        assert_eq!(
            bytes(empty, obj3.unwrap().as_stream().unwrap().data.at().unwrap()),
            b""
        );
    }

    #[test]
    fn error_messages_and_extents() {
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
            assert_eq!(error.to_string(), said);
        }

        let input = b"7 2 obj << >> stream\nabc endstream";
        let astray: [fn(&StreamHead<'_>) -> Span; 3] = [
            |head| Span::new(None, head.start() + 1, head.start() + 2),
            |head| Span::new(None, head.start(), head.start() + 99),
            |head| Span::new(Some(ObjectId::new(9, 0)), head.start(), head.start() + 3),
        ];
        for choose in astray {
            let err = indirect(input, choose).0.unwrap_err();
            assert!(matches!(err.kind(), ErrorKind::Extent { .. }));
        }
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn memory_bounds_match_documented_limits() {
        assert_eq!(size_of::<Object>(), 64);
        assert_eq!(size_of::<Entry>(), 112);
        let most = 4 * 64;
        assert!(4 * size_of::<Object>() <= most);
        assert!(4 * size_of::<Entry>() / 2 <= most);
        assert_eq!(most * Limits::DEFAULT.values, 8 << 30);
    }

    #[test]
    fn iterative_parser_handles_5000_deep_nesting_on_64kib_stack() {
        const DEPTH: usize = 5_000;
        let mut input = Vec::with_capacity(DEPTH * 2 + 1);
        input.resize(DEPTH, b'[');
        input.push(b'7');
        input.resize(DEPTH * 2 + 1, b']');

        let handle = std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(move || {
                let limits = Limits::DEFAULT.with_nesting(DEPTH);
                let obj = Parser::at(&input, 0).limits(limits).value(&mut ()).unwrap();
                drop(obj);
            })
            .unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn lookahead_respects_non_final_windows() {
        const BASE: usize = 1_000_000;
        let file = b"1 /A 2345 ";
        let whole = Parser::at(file, 0).value(&mut ()).unwrap();
        for cut in [6, 7, 8] {
            let mut window = Parser::at(&file[..cut], 0).window(BASE, false);
            let val = window.value(&mut ()).unwrap();
            assert_eq!(val.value(), whole.value(), "{cut}");
            assert_eq!(window.offset(), BASE + 1);
        }
        let file = b"1 0 2345 ";
        for cut in [6, 7, 8] {
            let mut window = Parser::at(&file[..cut], 0).window(BASE, false);
            assert!(
                matches!(window.value(&mut ()), Err(ref e) if matches!(e.kind(), ErrorKind::Short { .. })),
                "{cut}"
            );
        }
    }

    #[test]
    fn every_window_matches_whole_file_across_iso_and_pdfium() {
        let inputs: [(&[u8], u16); 16] = [
            (b"%x\n<< /A [1 (x) /N] /B 2 0 R /C << /D true >> >> 9", 0),
            (b"[12 0 R] (ab) <41 42> [1 2] << /A 1 >> 1 ", 0),
            (b"12 0 RG ", 0),
            (b"[1 endobj 2 3", 0),
            (b"1 0 obj 7 endobj\n", 0),
            (b"5 0 obj << >> stray", 0),
            (b"1 0 obj << >> stream\r\nxy\nendstream endobj\n", 2),
            (b"1 0 obj << >>stream\rxy\nendstream\nendobj", 2),
            (
                b"1 0 obj << /Length 2 >> stream  \r\nxy\nendstream endobj",
                2,
            ),
            (
                b"4 0 obj\n<< /Length 6 >>\nstream\r\nabcdef\nendstream\nendobj ",
                6,
            ),
            (
                b"1 0 obj << /Length 16 >> stream\nab endstream cd\nendstream endobj\n",
                16,
            ),
            (
                b"1 0 obj << /Length 18 >> stream\nab endstreamX cd \nendstream endobj\n",
                18,
            ),
            (b"[/A#zz 99999999999999999999 -1.5e3]", 0),
            (
                b"[--5 1.2.3 4294967296 <41G42> (a\rb\r\nc) /#5Gx 3 --1 R]",
                0,
            ),
            (b"[(a)\x80(b) /A\xFF 1 0\x80R --1\x80 %\x80\n2]\x80", 0),
            (
                b"1 0 obj << /Length 2 >>\x80stream\nab\xFFendstream\x80endobj",
                2,
            ),
        ];
        for (input, length) in inputs {
            let read = crate::fuzz::every_window(input, length);
            assert!(read > 0, "{}", String::from_utf8_lossy(input));
        }
    }

    fn pdfium(input: &[u8]) -> (Result<Object, Error>, Kept) {
        let mut kept = Kept::default();
        let value = Parser::at(input, 0).rules(Rules::Pdfium).value(&mut kept);
        (value, kept)
    }

    #[test]
    fn pdfium_numbers_names_strings_and_high_bytes() {
        assert_eq!(pdfium(b"--5").0.unwrap().as_integer(), Some(0));
        assert_eq!(pdfium(b"1.2.3").0.unwrap().as_number(), Some(1.2));
        assert_eq!(pdfium(b"4294967296").0.unwrap().as_integer(), Some(0));
        assert_eq!(
            pdfium(b"4294967295").0.unwrap().as_integer(),
            Some(4_294_967_295)
        );

        for (length, expected) in [(255, 5), (256, 5), (257, 0), (258, 0)] {
            let word = [vec![b'0'; length - 1], b"5".to_vec()].concat();
            let (val, kept) = pdfium(&word);
            assert_eq!(val.unwrap().as_integer(), Some(expected));
            assert_eq!(kept.faults.len(), usize::from(length > 256));
        }

        let (hex_val, hex_kept) = pdfium(b"<41G42>");
        assert_eq!(hex_val.unwrap().as_bytes(), Some(&b"AB"[..]));
        assert_eq!(hex_kept.faults.len(), 1);

        let (cr_val, cr_kept) = pdfium(b"(a\rb\r\nc)");
        assert_eq!(cr_val.unwrap().as_bytes(), Some(&b"a\rb\r\nc"[..]));
        assert_eq!(cr_kept.faults.len(), 1);

        let (name_val, name_kept) = pdfium(b"/#5Groducer");
        assert_eq!(name_val.unwrap().as_name(), Some(&b"Producer"[..]));
        assert_eq!(name_kept.faults.len(), 1);

        // High bytes in header vs body vs outside object.
        let mut h_kept = Kept::default();
        let mut p = Parser::at(b"1\x800 obj\x805\x80endobj", 0).rules(Rules::Pdfium);
        let id = p.header(&mut h_kept).unwrap();
        assert_eq!(id, ObjectId::new(1, 0));
        assert_eq!(h_kept.faults.len(), 1); // 0x80 inside header `1\x800 obj`!
        let mut b_kept = Kept::default();
        let _ = p.body(id, &mut b_kept).unwrap();
        assert!(b_kept.faults.is_empty()); // 0x80 around `5` is outside value!
    }
}
