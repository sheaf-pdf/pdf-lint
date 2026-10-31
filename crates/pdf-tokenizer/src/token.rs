//! A token: what kind it is, where it starts, and the bytes it was written
//! as.

use core::ops::Range;

/// What a token is. The kinds are lexical only: `obj`, `R`, `true` and
/// `stream` are all [`Kind::Keyword`], and `3 0 R` is three tokens with
/// spaces between them.
///
/// A kind is only a tag. Every token's bytes are in [`Token::bytes`],
/// whatever its kind, [`Kind::Junk`] included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A run of white-space bytes: by PDFium's rules, 0x80 and 0xFF among
    /// them ([`Rules`](crate::Rules)).
    Space,
    /// `%` and what follows it, up to but not including an end of line.
    Comment,
    /// A run of regular bytes that is an integer: digits, with an optional
    /// sign (`17`, `+17`, `-98`). It may be too large for any integer type.
    /// By PDFium's rules, any run of digits and signs (`--5`, `5-3`, `+`).
    Integer,
    /// A run of regular bytes that is a real: digits and one point, with an
    /// optional sign (`34.5`, `-.002`, `4.`). By PDFium's rules, any run of
    /// digits, signs and points with a point among them (`1.2.3`, `.`).
    Real,
    /// `/` and the regular bytes after it, `#xx` escapes as written. `/`
    /// alone is the empty name.
    Name,
    /// A literal string, `(` to the `)` that balances it, escapes as
    /// written. It's unclosed when the input ends first, and then runs to
    /// the end of the input.
    String {
        /// Whether its `)` came.
        closed: bool,
    },
    /// A hexadecimal string, `<` to `>`. It's unclosed when a byte that is
    /// neither a hex digit nor white-space comes before `>`, and then ends
    /// before that byte, or when the input ends first. By PDFium's rules it
    /// runs to its `>` over any byte, and is unclosed only when the input
    /// ends first.
    HexString {
        /// Whether its `>` came.
        closed: bool,
    },
    /// `[`.
    ArrayOpen,
    /// `]`.
    ArrayClose,
    /// `<<`.
    DictOpen,
    /// `>>`.
    DictClose,
    /// `{`, which PostScript calculator functions use.
    BraceOpen,
    /// `}`.
    BraceClose,
    /// Any other run of regular bytes: `obj`, `R`, `true`, `null`,
    /// `stream`, a content stream's operators, `1e5`, and bytes that are no
    /// syntax at all.
    Keyword,
    /// A `)` with no string open, or a `>` that isn't part of `>>`.
    Junk,
    /// Bytes a caller measured as a stream's or an inline image's data, with
    /// [`Tokenizer::data`](crate::Tokenizer::data) or
    /// [`Tokenizer::data_until`](crate::Tokenizer::data_until). The iterator
    /// never yields it.
    Data,
}

/// A token: its kind, where it starts, and its bytes as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Token<'a> {
    /// What it is.
    pub kind: Kind,
    /// The offset of its first byte in the input.
    pub start: usize,
    /// Its bytes as written: a slice of the input.
    pub bytes: &'a [u8],
}

impl Token<'_> {
    /// The offset just past its last byte.
    pub fn end(&self) -> usize {
        self.start + self.bytes.len()
    }

    /// Where it is in the input.
    pub fn range(&self) -> Range<usize> {
        self.start..self.end()
    }
}
