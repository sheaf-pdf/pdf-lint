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
//!   [`Name`](Fault::Name), [`StringCr`](Fault::StringCr),
//!   [`Long`](Fault::Long), [`Closer`](Fault::Closer),
//!   [`Brace`](Fault::Brace), and an array's or a dictionary's
//!   [`NestedArray`](Fault::NestedArray) and [`NoDict`](Fault::NoDict),
//!   where PDFium ends what it holds before ISO 32000-2 would):
//!   [`Tokens`](crate::Tokens) and [`Operations`](crate::Operations) with
//!   a sink ([`Tokens::with_sink`](crate::Tokens::with_sink)), as each
//!   operand is read; an inline image's dictionary's tokens too.
//! - **An inline image's** ([`EscapedKey`](Fault::EscapedKey),
//!   [`Abbreviation`](Fault::Abbreviation), [`DictEnd`](Fault::DictEnd),
//!   [`ImageLength`](Fault::ImageLength), [`AfterId`](Fault::AfterId),
//!   [`BeforeEi`](Fault::BeforeEi), and its dictionary's
//!   [`Keyword`](Fault::Keyword) and [`NoValue`](Fault::NoValue)): the
//!   same, as the image is read; and a `BI` that begins none,
//!   [`LoneBi`](Fault::LoneBi), as reading passes the keyword that makes
//!   it one.
//! - **White-space's** ([`HighByte`](Fault::HighByte), 0x80 and 0xFF, which
//!   are white-space to PDFium and regular characters to ISO 32000-2,
//!   7.2.3): the same, as reading passes the white-space between tokens,
//!   inside an array or a dictionary among it, and in an inline image's
//!   dictionary, as far as ISO's runs, and between its data and its `EI`.
//! - **A page's** ([`Across`](Fault::Across)): the same, over a page's
//!   streams joined ([`Concat::tokens`](crate::Concat::tokens),
//!   [`Concat::operations`](crate::Concat::operations)).
//! - **The operations'** ([`Trailing`](Fault::Trailing)):
//!   [`Operations`](crate::Operations), at the end.
//! - **What's inside an array or a dictionary**: each element's, key's
//!   and value's own, and [`Key`](Fault::Key),
//!   [`Keyword`](Fault::Keyword) and [`NoValue`](Fault::NoValue), when
//!   it's read ([`Operand::object_with`](crate::Operand::object_with), or
//!   [`Operand::faults_with`](crate::Operand::faults_with) with no object
//!   made): reading it is what finds them. A dictionary one of them makes
//!   none is the token's to tell ([`NoDict`](Fault::NoDict)), with where
//!   PDFium's reading of it stops.
//!
//! Each is told once, as reading passes its end: faults come in the order
//! of their ends.
//!
//! Where 0x80 or 0xFF is outside a string, a comment or an inline image's
//! data, ISO reads a word or a name on over it, or a word of its own: a
//! [`HighByte`](Fault::HighByte) each. As the byte after an inline image's
//! `ID`, where ISO asks white-space, it's an [`AfterId`](Fault::AfterId)
//! alone, not both.

use std::borrow::Cow;
use std::ops::Range;

use memchr::memchr2;
use pdf_tokenizer::decode::{self, NumberError, pdfium};
use pdf_tokenizer::{Kind, is_white_space};

use crate::operand::{STRING, WORD};
use crate::scan::{Event, TOP, UNCAPPED, Visit, is_number, tokenizer, walk};

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
    /// integer outside the 32 bits PDFium reads, unsigned when it's written
    /// without a sign and signed when with one, which
    /// [`Operand::number`](crate::Operand::number) and
    /// [`object`](crate::object) read as 0, as PDFium reads it; or a real
    /// past an `f32`'s range, which `Operand::number` reads as infinite, as
    /// PDFium reads it, and `object` as the decimal it is, at 64 bits; or
    /// one too large for a 64-bit float, longer than the 255 bytes of a word
    /// read here ([`Long`](Fault::Long)). ISO converts the number written to
    /// its value ("shall"), the range left to the processor (7.3.3; Annex
    /// C, which is advice).
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
    /// A name with a `#` without two hex digits after it, or with `#00`,
    /// read as NUL. Read as PDFium reads it ([`name`](crate::name)), such
    /// a `#` with two bytes after it is an escape, a byte that's no hex
    /// digit read as 0 (`/F#3G` is `F0`), and with fewer it's itself; ISO
    /// 32000-2 reads a `#` as an escape only before two hex digits, writes
    /// a `#` in a name as `#23` ("shall"), and a name holds any byte but
    /// NUL (7.3.5).
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
    /// A literal string with a carriage return written in it, not
    /// escaped: read as written, a CR or a CR LF, as PDFium reads it,
    /// where ISO 32000-2 reads an end of line in a literal string as a line
    /// feed alone (7.3.4.2, "shall").
    #[error(
        "the string {} has a CR written in it, read as written, where ISO 32000-2 reads LF",
        shown(.written)
    )]
    StringCr {
        /// The string, `(` to `)`.
        written: &'a [u8],
    },
    /// A number or a name longer than the 255 bytes PDFium's content parser
    /// keeps of a word, its `/` counted, or a string whose bytes, read, are
    /// more than the 32,767 it keeps of one: read here, as PDFium reads it,
    /// as its first 255 or its first 32,767, the rest dropped, where ISO
    /// 32000-2 reads all of it, and sets no length (Annex C's are advice).
    /// [`object`](crate::object) reads a number whole, at 64 bits.
    #[error("{} is longer than PDFium reads, and read as its first part", shown(.written))]
    Long {
        /// The word or the string, whole.
        written: &'a [u8],
    },
    /// A `)`, `>`, `]` or `>>` with nothing open to close: passed over
    /// here, as PDFium passes it over, an operand that's no object; to ISO
    /// 32000-2 it's no token at all, where content is objects and operators
    /// ("shall": 7.8.2, 7.2.3). Or one that closes nothing here, but what
    /// ISO reads open, where PDFium has ended it sooner
    /// ([`NestedArray`](Fault::NestedArray), [`NoDict`](Fault::NoDict)).
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
    /// A byte 0x80 or 0xFF where content's white-space is: white-space
    /// here, as PDFium reads it, ending a word or a name, and passed over
    /// between tokens; to ISO 32000-2 a regular character, part of the word
    /// or the name beside it, or a word of its own (7.2.3, a definition,
    /// neither a "shall" nor a "should"). Told for each such byte between
    /// tokens, inside an array or a dictionary, in an inline image's
    /// dictionary as far as ISO's runs, to its first `ID`, and between its
    /// data, where ISO ends it, and its `EI`, which ISO has after optional
    /// white-space (8.9.7), but among tokens there, which
    /// [`BeforeEi`](Fault::BeforeEi) tells whole. Not in a string or a
    /// comment, where it's a byte like any other, nor in an image's data.
    /// Nor as the white-space byte before an image's data, after its `ID`
    /// or the object that ends its dictionary: that's
    /// [`AfterId`](Fault::AfterId) where ISO asks white-space there, the
    /// data's first byte where ISO asks none, and after an object,
    /// [`DictEnd`](Fault::DictEnd)'s, where ISO reads the dictionary on.
    #[error("{} is white-space here and a regular character to ISO 32000-2", byte(.written))]
    HighByte {
        /// The byte.
        written: &'a [u8],
    },
    /// A word that's no object where an object is written: an element of
    /// an array, a key or value of a dictionary, or a value of an inline
    /// image's. Passed over here in an array, as PDFium passes it over;
    /// a dictionary's key or value makes the dictionary none, as PDFium
    /// reads it ([`NoDict`](Fault::NoDict)); and an inline image's value
    /// takes its key out, as PDFium takes out a key it reads no value for.
    /// ISO 32000-2's arrays and dictionaries hold objects ("shall": 7.3.6,
    /// 7.3.7), and so does an inline image's dictionary, its key-value
    /// pairs (8.9.7). Where an inline image's key is written, a keyword
    /// makes its `BI` a [`LoneBi`](Fault::LoneBi).
    #[error("{} is no object, where one is written", shown(.written))]
    Keyword {
        /// The word.
        written: &'a [u8],
    },
    /// A dictionary's key that's an object but not a name: the dictionary
    /// is none here, as PDFium reads it, its reading stopped at that key
    /// ([`NoDict`](Fault::NoDict)), of which a string's `(` or a hex
    /// string's `<` is the first byte alone; ISO 32000-2's keys are names
    /// (7.3.7, "shall"). In an inline image's dictionary, such an object
    /// ends it: [`DictEnd`](Fault::DictEnd).
    #[error("{} is a dictionary's key and not a name", shown(.written))]
    Key {
        /// The key.
        written: &'a [u8],
    },
    /// A dictionary's key with no value after it, at its `>>`: the
    /// dictionary is none here, as PDFium reads it
    /// ([`NoDict`](Fault::NoDict)). Or an inline image's, at an `ID`, which
    /// PDFium reads as its value, none, taking the key out and reading the
    /// dictionary on past it, where ISO 32000-2's ends at that `ID`
    /// (8.9.7). ISO's dictionaries are key-value pairs (7.3.7, "shall").
    #[error("the key {} has no value", shown(.written))]
    NoValue {
        /// The key.
        written: &'a [u8],
    },
    /// A `[` inside an array, with no dictionary open around it: it opens
    /// no array here, as PDFium reads it, which drops it and reads on the
    /// array it's in, so the next `]` ends that array, and what ISO
    /// 32000-2 reads after the inner array's `]` is read as content, or as
    /// that array's elements; ISO's arrays may hold arrays, however deep
    /// (7.3.6). So `[[1] 2]` is the array of 1 here, then `2` and `]`,
    /// which closes nothing.
    #[error("{} opens no array here, inside an array, and one to ISO 32000-2", shown(.written))]
    NestedArray {
        /// The `[`.
        written: &'a [u8],
    },
    /// A dictionary that's none here, as PDFium reads it, from its `<<` to
    /// the word its reading stopped at: a key that's no name
    /// ([`Key`](Fault::Key)), its first byte alone where it's a string, or
    /// a value that's no object (a keyword, a closer that closes nothing,
    /// a brace, a `>>` where a key has no value, or a dictionary that's
    /// none itself), what's after it read as content, or as the elements
    /// of an array it's in, which a `]` read for it ends. ISO 32000-2's
    /// dictionary runs to its `>>`, its keys names and its values objects
    /// (7.3.7, "shall").
    #[error("{} is no dictionary here, where ISO 32000-2 reads one", shown(.written))]
    NoDict {
        /// The dictionary, `<<` to where its reading stopped.
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
    /// An inline image's key written with a `#` escape, decoded one of
    /// the keys 8.9.7 reads (Table 91's, either spelling): passed over
    /// here, as PDFium matches a key as written; ISO 32000-2 reads a name
    /// by its escapes decoded (7.3.5), so the key is the one it names.
    #[error("{} is an inline image's key to ISO 32000-2, and no key here", shown(.written))]
    EscapedKey {
        /// The key.
        written: &'a [u8],
    },
    /// An inline image's key written both abbreviated and in full (8.9.7,
    /// Table 91), where the full name's value is an abbreviated name
    /// (Table 92) that means another than the abbreviation's: read here
    /// by the full name's, as PDFium sets an abbreviated value over the
    /// abbreviation's; ISO 32000-2 gives the abbreviated key precedence
    /// (8.9.7, "shall", as its errata add).
    #[error(
        "an inline image's {key} is read from its full name, and by ISO 32000-2 from its abbreviation"
    )]
    Abbreviation {
        /// The image's dictionary, from after `BI` to `ID`.
        written: &'a [u8],
        /// The key's full name.
        key: &'static str,
    },
    /// An inline image whose `/L` or `/Length` gives its data a length
    /// that ends it elsewhere than it's read here, where PDFium reads
    /// neither: it finds an unfiltered image's data by its size, a
    /// filter's by its end, and otherwise reads on to the next `EI`; or,
    /// where a decoder ends it short of its filter's own end of data (where
    /// DCT and CCITTFax data is decoded, fax data's end-of-block pattern
    /// or a JPEG's end of image), elsewhere than that end, where ISO ends
    /// it.
    /// ISO 32000-2's `/L` is the data's length (8.9.7, "shall"), after the
    /// white-space byte after `ID`, and any more white-space where the
    /// last of its filters is one of the two ASCII ones, ASCIIHexDecode and
    /// ASCII85Decode. The length and the filters are read from the
    /// dictionary as ISO reads it, to its first `ID`, a key's value too
    /// ([`InlineImage::length`](crate::InlineImage::length)), and counted
    /// from where the data is read here.
    #[error("an inline image's data is {read} bytes, where its length says {given}")]
    ImageLength {
        /// The length its dictionary gives.
        given: u64,
        /// The data's bytes as ISO 32000-2 counts them, to where it ends
        /// them: where the data is read here, or, where a decoder ends it
        /// short of its filter's own end of data, that end.
        read: usize,
    },
    /// An inline image's `ID` with a delimiter after it, not white-space,
    /// the data read from straight after it, as PDFium reads it: ISO
    /// 32000-2 asks one white-space byte after `ID`, unless the last of the
    /// image's filters is one of the two ASCII ones, ASCIIHexDecode and
    /// ASCII85Decode (8.9.7, "shall", as its errata correct it). The `ID`
    /// is the first, where ISO's dictionary ends: one written as a key's
    /// value ([`NoValue`](Fault::NoValue)) too, after which PDFium reads
    /// on, its filters those before it.
    #[error("an inline image's ID has no white-space after it")]
    AfterId,
    /// Tokens between an inline image's data and its `EI`: passed over
    /// here, as PDFium reads on to the next `EI`; ISO 32000-2's `EI`
    /// follows the data (8.9.7, "shall"). The data runs on to its filter's
    /// own end of data where a decoder ends it sooner, as for
    /// [`ImageLength`](Fault::ImageLength).
    #[error("{} is between an inline image's data and its EI", shown(.written))]
    BeforeEi {
        /// What's between, the first token to the last.
        written: &'a [u8],
    },
    /// An object that's no name where an inline image's key is written:
    /// a number, a string, an array, a dictionary, `true`, `false`,
    /// `null`, a closer or a brace. Its dictionary ends there, as PDFium
    /// ends it, and its data is read from after it, as from after `ID`;
    /// ISO 32000-2's keys are names (7.3.7, "shall"), and an inline
    /// image's key-value pairs run from `BI` to `ID` (8.9.7, "shall"), so
    /// ISO reads its dictionary on. In an operand's dictionary, such a key
    /// is a [`Key`](Fault::Key), which makes the dictionary none.
    #[error("{} ends an inline image's dictionary, where a key is written", shown(.written))]
    DictEnd {
        /// The object.
        written: &'a [u8],
    },
    /// A `BI` with a keyword other than `ID` where its dictionary's key is
    /// written, from `BI` to that keyword: no inline image here, as
    /// PDFium reads it, the `BI` an operator of its own, and what's after
    /// it read as content, the dictionary's tokens and the keyword among
    /// it. To ISO 32000-2, `BI` begins an inline image, its key-value
    /// pairs running to `ID` (8.9.7, "shall"), and a keyword is no key
    /// (7.3.7). Told as reading passes the keyword.
    #[error("{} begins no inline image, a keyword where a key is written", shown(.written))]
    LoneBi {
        /// `BI` to the keyword.
        written: &'a [u8],
    },
}

/// What's written, its first 40 bytes, for a fault's text.
fn shown(written: &[u8]) -> Cow<'_, str> {
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
/// string's byte that's no hex digit, a name's `#`, a literal string's CR,
/// a word or a string longer than PDFium reads, a closer or a brace. What
/// it is where an object is written.
pub(crate) fn token<'a>(written: &'a [u8], at: usize, sink: &mut (impl Sink<'a> + ?Sized)) -> Read {
    let (fault, long) = match written {
        [b'[', ..] | [b'<', b'<', ..] => (None, false),
        [b'(', ..] => (string_cr(written), long_string(written, pdfium::literal)),
        [b'/', ..] => (name(written), written.len() > WORD),
        [b'<', ..] => (hex_string(written), long_string(written, pdfium::hex)),
        b")" | b">" | b"]" | b">>" => (Some(Fault::Closer { written }), false),
        b"{" | b"}" => (Some(Fault::Brace { written }), false),
        b"true" | b"false" | b"null" => (None, false),
        _ if is_number(written) => (number(written), written.len() > WORD),
        _ => return Read::Keyword,
    };
    let read = match fault {
        Some(Fault::Number { .. } | Fault::Closer { .. } | Fault::Brace { .. }) => Read::Told,
        _ => Read::Object,
    };
    if let Some(fault) = fault {
        sink.fault(at..at + written.len(), fault);
    }
    if long {
        sink.fault(at..at + written.len(), Fault::Long { written });
    }
    read
}

/// Each 0x80 and 0xFF in `gap`'s white-space, told to `tell` in order
/// ([`Fault::HighByte`]): `gap` is white-space and comments, as between
/// tokens, read from its start by PDFium's rules, where 0x80 and 0xFF are
/// white-space; a comment's bytes are its own. Nothing is read where `gap`
/// holds neither byte.
pub(crate) fn spaces<'a>(
    s: &'a [u8],
    gap: Range<usize>,
    mut tell: impl FnMut(Range<usize>, Fault<'a>),
) {
    if s.get(gap.clone())
        .is_none_or(|g| memchr2(0x80, 0xFF, g).is_none())
    {
        return;
    }
    let mut tokens = tokenizer(s, gap.start);
    while tokens.offset() < gap.end {
        let Some(token) = tokens.next() else {
            return;
        };
        if token.kind != Kind::Space {
            continue;
        }
        for (i, &c) in token.bytes.iter().enumerate() {
            let at = token.start + i;
            if at >= gap.end {
                return;
            }
            if c == 0x80 || c == 0xFF {
                let written = &s[at..at + 1];
                tell(at..at + 1, Fault::HighByte { written });
            }
        }
    }
}

/// Where PDFium ends what an array or a dictionary token holds before ISO
/// 32000-2 would, `token` where it is in `s`, each told to `tell` as
/// reading passes its end: a `[` it drops ([`Fault::NestedArray`]), a
/// dictionary it reads as none ([`Fault::NoDict`]), and 0x80 or 0xFF in
/// the white-space between what it holds ([`Fault::HighByte`]). Nothing for
/// any other token. The token was read under a cap on depth, so nothing in
/// it is deeper. Where it holds neither byte, its arrays' elements are
/// skimmed, as reading them first did.
pub(crate) fn nesting<'a>(
    s: &'a [u8],
    token: Range<usize>,
    tell: impl FnMut(Range<usize>, Fault<'a>),
) {
    let at = token.start;
    let rest = s.get(at..).unwrap_or_default();
    if !(rest.starts_with(b"[") || rest.starts_with(b"<<")) {
        return;
    }
    let high = s
        .get(token)
        .is_some_and(|t| memchr2(0x80, 0xFF, t).is_some());
    let mut nesting = Nesting {
        s,
        tell,
        open: Vec::new(),
        last: at,
        high,
        gap: at,
    };
    walk(s, at, TOP, UNCAPPED, !high, &mut nesting);
}

/// What [`nesting`] tells as a walk reads a token: each array or
/// dictionary open, a dictionary by where it starts, and where the last
/// word read ends; and, where the token holds 0x80 or 0xFF (`high`), where
/// the last token read ends, so that the white-space before the next is
/// read as it is.
struct Nesting<'a, T> {
    s: &'a [u8],
    tell: T,
    open: Vec<Option<usize>>,
    last: usize,
    high: bool,
    gap: usize,
}

impl<'a, T: FnMut(Range<usize>, Fault<'a>)> Visit<'a> for Nesting<'a, T> {
    fn visit(&mut self, event: Event<'a>) {
        if let (true, Some(token)) = (self.high, event.token()) {
            spaces(self.s, self.gap..token.start, &mut self.tell);
            self.gap = token.end();
        }
        match event {
            Event::Open(token) => {
                let dict = token.bytes.starts_with(b"<<");
                self.open.push(dict.then_some(token.start));
            }
            Event::Word(token) => {
                if token.bytes == b"[" {
                    let written = token.bytes;
                    (self.tell)(token.range(), Fault::NestedArray { written });
                }
                self.last = token.end();
            }
            Event::Close(token) => {
                self.open.pop();
                self.last = token.end();
            }
            Event::Leaf(token) | Event::Key(token) | Event::NotKey(token) => {
                self.last = token.end();
            }
            Event::Unmade => {
                if let Some(Some(start)) = self.open.pop() {
                    let written = &self.s[start..self.last];
                    (self.tell)(start..self.last, Fault::NoDict { written });
                }
            }
        }
    }
}

/// A number word's fault: one ISO 32000-2 doesn't write so (7.3.3), or
/// one past what's read here, each judged on the whole word as pdf-tokenizer
/// reads a number by ISO's rules, an integer, or a real when it has a
/// point: an integer PDFium reads otherwise, outside its 32 bits, or a
/// real it reads as infinite, past an `f32`; or one past 64 bits. Told
/// from the word's bytes alone ([`number_by_bytes`]) where they say, and
/// otherwise by reading it both ways ([`number_read`]).
pub(crate) fn number(written: &[u8]) -> Option<Fault<'_>> {
    match number_by_bytes(written) {
        Some(fault) => fault,
        None => number_read(written),
    }
}

/// A number word's fault ([`number`]), `written` digits, signs and points
/// alone, where its bytes say it without reading its value; None where
/// they don't. A word ISO 32000-2 doesn't write as a number, a sign
/// after its first byte, a second point or no digit, is one
/// ([`Fault::Number`]); an integer of nine digits at most is none, as
/// below 2^31 it's the integer PDFium reads, with a sign or without; and a
/// real of 38 digits at most before its point is none, as below 10^38 it's
/// within an `f32`'s range and a 64-bit float's. Past those, it's read.
pub(crate) fn number_by_bytes(written: &[u8]) -> Option<Option<Fault<'_>>> {
    let body = match written {
        [b'+' | b'-', body @ ..] => body,
        body => body,
    };
    let (mut point, mut digits) = (None, 0usize);
    for (i, &c) in body.iter().enumerate() {
        match c {
            b'0'..=b'9' => digits += 1,
            b'.' if point.is_none() => point = Some(i),
            _ => return Some(Some(Fault::Number { written })),
        }
    }
    match point {
        _ if digits == 0 => Some(Some(Fault::Number { written })),
        None if digits <= 9 => Some(None),
        Some(whole) if whole <= 38 => Some(None),
        _ => None,
    }
}

/// A number word's fault ([`number`]), read both ways: by ISO 32000-2's
/// rules, as pdf-tokenizer reads a number, and by PDFium's.
pub(crate) fn number_read(written: &[u8]) -> Option<Fault<'_>> {
    let past = Fault::OutOfRange { written };
    if written.contains(&b'.') {
        return match decode::number(written) {
            Ok(_) => pdfium::number(written)
                .float()
                .is_infinite()
                .then_some(past),
            Err(NumberError::NotANumber) => Some(Fault::Number { written }),
            Err(NumberError::Overflow) => Some(past),
        };
    }
    match decode::integer(written) {
        Ok(n) => {
            let pdfium = match pdfium::number(written) {
                pdfium::Number::Unsigned(u) => i64::from(u),
                pdfium::Number::Signed(i) => i64::from(i),
                pdfium::Number::Real(_) => return Some(past),
            };
            (pdfium != n).then_some(past)
        }
        Err(NumberError::NotANumber) => Some(Fault::Number { written }),
        Err(NumberError::Overflow) => Some(past),
    }
}

/// A literal string's fault: a CR written in it, not escaped, which PDFium
/// reads as written and ISO 32000-2 as LF (7.3.4.2). Every escape reads
/// alike, so the readings differ only there.
fn string_cr(written: &[u8]) -> Option<Fault<'_>> {
    let cr =
        written.contains(&b'\r') && !pdfium::literal(written).eq(decode::literal(written).bytes());
    cr.then_some(Fault::StringCr { written })
}

/// Whether a string's bytes, read by `read`, are more than the
/// [`STRING`] PDFium's content parser keeps. A string's bytes are never
/// more than it's written in, so only a long one is read.
fn long_string(written: &[u8], read: fn(&[u8]) -> pdfium::Bytes<'_>) -> bool {
    written.len() > STRING && read(written).nth(STRING).is_some()
}

/// A hex string's fault: a byte between its `<` and `>` that's neither a
/// hex digit nor white-space, ISO 32000-2's (7.2.3).
fn hex_string(written: &[u8]) -> Option<Fault<'_>> {
    let inner = written.get(1..written.len().saturating_sub(1))?;
    inner
        .iter()
        .any(|&c| !c.is_ascii_hexdigit() && !is_white_space(c))
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
