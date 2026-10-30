//! Where ISO 32000-2 reads content otherwise. Content is read here by
//! PDFium's rules; where ISO's rules would read the same bytes another
//! way, or no way at all, that's a [`Fault`], told to a [`Sink`] with
//! where it is. Nothing told changes what's read: the tokens, the
//! operations and the values are the same whatever the sink, and a sink
//! that doesn't want faults, `()`, has none looked for.
//!
//! Who tells what:
//!
//! - **A token's own** ([`Number`](Fault::Number),
//!   [`OutOfRange`](Fault::OutOfRange), [`HexString`](Fault::HexString),
//!   [`Name`](Fault::Name), [`Closer`](Fault::Closer),
//!   [`Brace`](Fault::Brace)): [`Tokens`](crate::Tokens) and
//!   [`Operations`](crate::Operations) with a sink
//!   ([`Tokens::with_sink`](crate::Tokens::with_sink)), as each operand
//!   is read; an inline image's dictionary's tokens too.
//! - **An inline image's** ([`ImageLength`](Fault::ImageLength),
//!   [`AfterId`](Fault::AfterId), [`BeforeEi`](Fault::BeforeEi), and its
//!   dictionary's [`Key`](Fault::Key), [`Keyword`](Fault::Keyword) and
//!   [`NoValue`](Fault::NoValue)): the same, as the image is read.
//! - **A page's** ([`Across`](Fault::Across)): the same, over a page's
//!   streams joined ([`Concat::tokens`](crate::Concat::tokens),
//!   [`Concat::operations`](crate::Concat::operations)).
//! - **The operations'** ([`Trailing`](Fault::Trailing)):
//!   [`Operations`](crate::Operations), at the end.
//! - **What's inside an array or a dictionary**: each element's, key's
//!   and value's own, and [`Key`](Fault::Key),
//!   [`Keyword`](Fault::Keyword) and [`NoValue`](Fault::NoValue), when
//!   it's read ([`Operand::object_with`](crate::Operand::object_with)):
//!   reading it is what finds them.
//!
//! Each is told once, as reading passes its end: faults come in the order
//! of their ends.

use std::borrow::Cow;
use std::ops::Range;

use crate::scan::{is_number, is_space};

/// Where faults are told: a checker keeps them; `()` drops them, and
/// wants none, so none is looked for. A fault borrows from `'a`, the
/// content read, so a sink can keep it as it is. A sink borrowed, `&mut
/// S` or `&mut dyn Sink`, is a sink too.
pub trait Sink<'a> {
    /// A fault at `at`, offsets into the content read.
    fn fault(&mut self, at: Range<usize>, fault: Fault<'a>);

    /// Whether faults are wanted. A check that does work of its own, such
    /// as reading a number word as ISO 32000-2 writes one, or a hex
    /// string's bytes, runs only when they are, and a sink that doesn't
    /// want them is told none. True unless a sink says otherwise; asked as
    /// reading goes.
    fn wants(&self) -> bool {
        true
    }
}

/// Drops every fault, and wants none, so none is looked for.
impl Sink<'_> for () {
    #[inline]
    fn fault(&mut self, _: Range<usize>, _: Fault<'_>) {}

    #[inline]
    fn wants(&self) -> bool {
        false
    }
}

/// Tells the sink it borrows, and wants what it wants.
impl<'a, S: Sink<'a> + ?Sized> Sink<'a> for &mut S {
    #[inline]
    fn fault(&mut self, at: Range<usize>, fault: Fault<'a>) {
        (**self).fault(at, fault);
    }

    #[inline]
    fn wants(&self) -> bool {
        (**self).wants()
    }
}

/// A place where ISO 32000-2 reads content otherwise than it's read here,
/// by PDFium's rules: what's read is read as before, and the fault says
/// how ISO's rules part from it. Each names its clause, and whether the
/// clause's rule is a "shall" or a "should".
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Fault<'a> {
    /// A word of digits, signs and points that isn't a number as ISO
    /// 32000-2 writes one, such as `--5`, `1.2.3`, `+`, `.` or `5-`: an
    /// operand here, as PDFium reads any such word, and a keyword to ISO,
    /// whose integer is digits, a sign before them or not, and whose real
    /// has one point among its digits too (7.3.3, "shall"). As an operand
    /// it reads as no object.
    #[error("{} is a number here and a keyword to ISO 32000-2", shown(.written))]
    Number {
        /// The word.
        written: &'a [u8],
    },
    /// A number ISO 32000-2 reads as its value, past what's read here: an
    /// integer outside 64 bits, read as no object, or a real too large for
    /// a 64-bit float, read as infinite. ISO converts the number written to
    /// its value ("shall"), the range left to the processor (7.3.3, Annex
    /// C).
    #[error("{} is past the numbers read here", shown(.written))]
    OutOfRange {
        /// The number.
        written: &'a [u8],
    },
    /// A hex string with a byte that's neither a hex digit nor
    /// white-space: read here to its `>`, the byte passed over, as PDFium
    /// reads it, where ISO 32000-2's hex string is hex digits and
    /// white-space alone, comments not among it (7.3.4.3, "shall").
    #[error("the hex string {} has a byte that's no hex digit", shown(.written))]
    HexString {
        /// The string, `<` to `>`.
        written: &'a [u8],
    },
    /// A name with a `#` without two hex digits after it, read with the
    /// `#` as itself, or with `#00`, read as NUL: ISO 32000-2 writes a `#`
    /// in a name as `#23` ("shall"), and a name holds any byte but NUL
    /// (7.3.5).
    #[error(
        "the name {} has {}, which readers read differently",
        shown(.written),
        name_fault(*.stray, *.nul)
    )]
    Name {
        /// The name as written, its `/` first.
        written: &'a [u8],
        /// Whether it has a `#` without two hex digits after it.
        stray: bool,
        /// Whether it has `#00`.
        nul: bool,
    },
    /// A `)`, `>`, `]` or `>>` with nothing open to close: passed over
    /// here, as PDFium passes it over, an operand that's no object; to ISO
    /// 32000-2 it's no token at all, where content is objects and operators
    /// ("shall": 7.8.2, 7.2.3).
    #[error("{} closes nothing", shown(.written))]
    Closer {
        /// The delimiter.
        written: &'a [u8],
    },
    /// A `{` or a `}`: a delimiter here, as in PDFium, an operand that's
    /// no object; to ISO 32000-2 a regular character, part of a keyword,
    /// as braces delimit only in a type 4 function (7.2.3, as its errata
    /// correct it: a definition, neither a "shall" nor a "should").
    #[error("{} is a delimiter here and a regular character to ISO 32000-2", shown(.written))]
    Brace {
        /// The brace.
        written: &'a [u8],
    },
    /// A word that's no object where an object is written: an element of
    /// an array, a key or value of a dictionary, an inline image's too.
    /// Passed over here, as PDFium passes it over; ISO 32000-2's arrays and
    /// dictionaries hold objects ("shall": 7.3.6, 7.3.7), and so does an
    /// inline image's dictionary, its key-value pairs (8.9.7).
    #[error("{} is no object, where one is written", shown(.written))]
    Keyword {
        /// The word.
        written: &'a [u8],
    },
    /// A dictionary's key that's an object but not a name, an inline
    /// image's too: passed over here, as PDFium passes it over, what's
    /// next read as the key; ISO 32000-2's keys are names (7.3.7,
    /// "shall").
    #[error("{} is a dictionary's key and not a name", shown(.written))]
    Key {
        /// The key.
        written: &'a [u8],
    },
    /// A dictionary's key with no value after it, at its `>>`, or an
    /// inline image's at its `ID`: passed over here; ISO 32000-2's
    /// dictionaries are key-value pairs (7.3.7, "shall").
    #[error("the key {} has no value", shown(.written))]
    NoValue {
        /// The key.
        written: &'a [u8],
    },
    /// A string, a comment or an inline image running from one of a page's
    /// streams into the next ([`Concat`](crate::Concat)). ISO 32000-2
    /// reads a page's streams as one with white-space between, and divides
    /// them only between its lexical tokens (7.7.3.3, Table 31's
    /// `/Contents`: "may" divide "only" there; 7.2), so an array, a
    /// dictionary or an inline image's dictionary divided between the
    /// tokens it's made of reads alike, and isn't told. Joined with a
    /// space, as PDFium joins them, a string, at any depth in an array or a
    /// dictionary, or an inline image's data, runs on into the next
    /// stream; and a comment at a stream's end, with no end of line, reads
    /// the next stream's first line as comment, which white-space that
    /// ends a line, as ISO's may be, wouldn't: told when that line has a
    /// token. A string or a comment is told where it is; an inline image
    /// whole, where its data, or a string or a comment in it, runs on.
    #[error("{} runs from one stream into the next", shown(.written))]
    Across {
        /// The string, the comment or the inline image, in the streams
        /// joined.
        written: &'a [u8],
    },
    /// Operands at the content's end that no operator takes: no operation
    /// here, as PDFium leaves them; ISO 32000-2's operands come before the
    /// operator that takes them (7.8.2, "shall").
    #[error("{count} operands at the end that no operator takes")]
    Trailing {
        /// How many.
        count: usize,
    },
    /// An inline image whose `/L` or `/Length` gives its data a length
    /// that ends it elsewhere than it's read here, where PDFium finds an
    /// unfiltered image's data by its size, or a filter's by its end, and
    /// counts `/L` only when it finds neither. ISO 32000-2's `/L` is the
    /// data's length (8.9.7, "shall"), after the white-space byte after
    /// `ID`, and any more white-space where the last of its filters is
    /// one of the two ASCII ones, ASCIIHexDecode and ASCII85Decode.
    #[error("an inline image's data is read as {read} bytes, where its length says {given}")]
    ImageLength {
        /// The length its dictionary gives.
        given: u64,
        /// The data's bytes as ISO counts them, to where it's read.
        read: usize,
    },
    /// An inline image's `ID` with a delimiter after it, not white-space,
    /// the data read from straight after it, as PDFium reads it: ISO
    /// 32000-2 asks one white-space byte after `ID`, unless the last of the
    /// image's filters is one of the two ASCII ones, ASCIIHexDecode and
    /// ASCII85Decode (8.9.7, "shall", as its errata correct it).
    #[error("an inline image's ID has no white-space after it")]
    AfterId,
    /// Tokens between an inline image's data and its `EI`: passed over
    /// here, as PDFium reads on to the next `EI`; ISO 32000-2's `EI`
    /// follows the data (8.9.7, "shall").
    #[error("{} is between an inline image's data and its EI", shown(.written))]
    BeforeEi {
        /// What's between, the first token to the last.
        written: &'a [u8],
    },
}

/// What's written, its first 40 bytes, for a fault's text.
fn shown(written: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(&written[..written.len().min(40)])
}

/// What's wrong with a name, for [`Fault::Name`]'s text.
fn name_fault(stray: bool, nul: bool) -> &'static str {
    match (stray, nul) {
        (true, true) => "a # without two hex digits after it, and #00, NUL",
        (true, false) => "a # without two hex digits after it",
        _ => "#00, NUL, which a name can't hold",
    }
}

/// What a token is to ISO 32000-2 where an object is written.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Read {
    /// An object: a number, a name, a string, an array, a dictionary,
    /// `true`, `false` or `null`.
    Object,
    /// No object, a fault of its own told.
    Told,
    /// A word that isn't a number, `true`, `false` or `null`: an operator
    /// where one may be, and no object where one is written.
    Keyword,
}

/// A token's own faults, told to `sink` at `at`, where it is: a number
/// ISO 32000-2 doesn't write so or reads past what's read here, a hex
/// string's byte that's no hex digit, a name's `#`, a closer or a brace.
/// What it is where an object is written.
pub(crate) fn token<'a>(written: &'a [u8], at: usize, sink: &mut (impl Sink<'a> + ?Sized)) -> Read {
    let fault = match written {
        [b'[', ..] | [b'(', ..] | [b'<', b'<', ..] => None,
        [b'/', ..] => name(written),
        [b'<', ..] => hex_string(written),
        b")" | b">" | b"]" | b">>" => Some(Fault::Closer { written }),
        b"{" | b"}" => Some(Fault::Brace { written }),
        b"true" | b"false" | b"null" => None,
        _ if is_number(written) => number(written),
        _ => return Read::Keyword,
    };
    let Some(fault) = fault else {
        return Read::Object;
    };
    let read = match fault {
        Fault::Number { .. } | Fault::Closer { .. } | Fault::Brace { .. } => Read::Told,
        _ => Read::Object,
    };
    sink.fault(at..at + written.len(), fault);
    read
}

/// A number word's fault: one ISO 32000-2 doesn't write so, or one past
/// what's read here.
fn number(written: &[u8]) -> Option<Fault<'_>> {
    if !iso_number(written) {
        return Some(Fault::Number { written });
    }
    // Digits, signs and points: ASCII, so text.
    let text = std::str::from_utf8(written).ok()?;
    let fits = if written.contains(&b'.') {
        text.parse::<f64>().is_ok_and(f64::is_finite)
    } else {
        text.parse::<i64>().is_ok()
    };
    (!fits).then_some(Fault::OutOfRange { written })
}

/// Whether a word is a number as ISO 32000-2 writes one (7.3.3): digits,
/// a sign before them or not, with at most one point among, before or
/// after them.
pub(crate) fn iso_number(word: &[u8]) -> bool {
    let unsigned = match word {
        [b'+' | b'-', rest @ ..] => rest,
        _ => word,
    };
    unsigned.iter().any(u8::is_ascii_digit)
        && unsigned.iter().all(|&c| c.is_ascii_digit() || c == b'.')
        && unsigned.iter().filter(|&&c| c == b'.').count() <= 1
}

/// A hex string's fault: a byte between its `<` and `>` that's neither a
/// hex digit nor white-space.
fn hex_string(written: &[u8]) -> Option<Fault<'_>> {
    let inner = written.get(1..written.len().saturating_sub(1))?;
    inner
        .iter()
        .any(|&c| !c.is_ascii_hexdigit() && !is_space(c))
        .then_some(Fault::HexString { written })
}

/// A name's fault: a `#` without two hex digits after it, or `#00`. Read
/// as [`name`](crate::name) reads one.
fn name(written: &[u8]) -> Option<Fault<'_>> {
    if !written.contains(&b'#') {
        return None;
    }
    let hex = |i: usize| written.get(i).is_some_and(u8::is_ascii_hexdigit);
    let (mut stray, mut nul) = (false, false);
    let mut i = 1;
    while i < written.len() {
        if written[i] == b'#' {
            if hex(i + 1) && hex(i + 2) {
                nul |= &written[i + 1..i + 3] == b"00";
                i += 3;
                continue;
            }
            stray = true;
        }
        i += 1;
    }
    (stray || nul).then_some(Fault::Name {
        written,
        stray,
        nul,
    })
}
