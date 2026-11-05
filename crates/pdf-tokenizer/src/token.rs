//! A token: what kind it is, whether it closed, where it starts, and the
//! bytes it was written as.

use core::ops::Range;

/// What a token is. The kinds are lexical only: `obj`, `R`, `true` and
/// `stream` are all [`Kind::Keyword`], and `3 0 R` is three tokens with
/// spaces between them.
///
/// A kind is only a tag, one byte (`#[repr(u8)]`), so a table can be
/// indexed by it (`kind as usize`). Every token's bytes are in
/// [`Token::bytes`], whatever its kind, [`Kind::Junk`] included; whether a
/// string closed is in [`Token::closed`].
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A run of white-space bytes: by PDFium's rules, 0x80 and 0xFF among
    /// them ([`Rules`](crate::Rules)). From the iterator, a maximal run: the
    /// byte after it isn't white-space. From [`eol`](crate::Tokenizer::eol)
    /// one end of line, and from [`space_byte`](crate::Tokenizer::space_byte)
    /// one byte, whatever comes after.
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
    /// written (ISO 32000-2, 7.3.4.2). It's unclosed ([`Token::closed`]
    /// false) when the input ends first, and then runs to the end of the
    /// input.
    LiteralString,
    /// A hexadecimal string, `<` to `>` (ISO 32000-2, 7.3.4.3). It's
    /// unclosed ([`Token::closed`] false) when a byte that is neither a hex
    /// digit nor white-space comes before `>`, and then ends before that
    /// byte, or when the input ends first. By PDFium's rules it runs to its
    /// `>` over any byte, and is unclosed only when the input ends first.
    HexString,
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
    /// never yields it: it's a kind so that a `Vec<Token>` can hold a whole
    /// file, its streams' data included, and the tokens still join back
    /// into it.
    Data,
}

/// A token: its kind, whether it closed, where it starts, and its bytes as
/// written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Token<'a> {
    /// What it is.
    pub kind: Kind,
    /// Whether it's whole: false only for a literal or hexadecimal string
    /// whose closing delimiter didn't come, which then runs to where its
    /// kind says ([`Kind::LiteralString`], [`Kind::HexString`]). Every
    /// other token is closed.
    pub closed: bool,
    /// The offset of its first byte in the input.
    pub start: usize,
    /// Its bytes as written: a slice of the input.
    pub bytes: &'a [u8],
}

impl Token<'_> {
    /// The offset just past its last byte.
    #[must_use]
    pub const fn end(&self) -> usize {
        self.start + self.bytes.len()
    }

    /// Where it is in the input.
    #[must_use]
    pub const fn range(&self) -> Range<usize> {
        self.start..self.end()
    }

    /// Whether it's the keyword `word`: a [`Kind::Keyword`] whose bytes are
    /// `word`, as `obj`, `R`, `endstream` and a content stream's operators
    /// are. A token of any other kind isn't, whatever its bytes.
    #[must_use]
    pub fn is_keyword(&self, word: &[u8]) -> bool {
        self.kind == Kind::Keyword && self.bytes == word
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use core::mem::size_of;
    use core::ops::Range;
    use std::vec::Vec;

    use super::{Kind, Token};
    use crate::Tokenizer;

    /// A kind is one byte, so a table can be indexed by it, and a token is
    /// four words: the kind and `closed` share the first.
    #[test]
    fn a_kind_is_one_byte_and_a_token_four_words() {
        assert_eq!(size_of::<Kind>(), 1);
        assert_eq!(size_of::<Token<'_>>(), 4 * size_of::<usize>());
        assert_eq!(Kind::Space as usize, 0);
        assert_eq!(Kind::Data as usize, 15);
    }

    /// Where a token ends, and its range, are known at compile time.
    #[test]
    fn a_tokens_end_and_range_are_known_at_compile_time() {
        const TOKEN: Token<'static> = Token {
            kind: Kind::Keyword,
            closed: true,
            start: 2,
            bytes: b"obj",
        };
        const END: usize = TOKEN.end();
        const RANGE: Range<usize> = TOKEN.range();
        assert_eq!((END, RANGE), (5, 2..5));
    }

    /// A token is the keyword `word` by its kind and its bytes together: not
    /// a token of any other kind with the same bytes, and not a keyword of
    /// other bytes, a prefix or an extension of `word` included.
    #[test]
    fn a_keyword_is_its_kind_and_its_bytes_together() {
        const KINDS: [Kind; 16] = [
            Kind::Space,
            Kind::Comment,
            Kind::Integer,
            Kind::Real,
            Kind::Name,
            Kind::LiteralString,
            Kind::HexString,
            Kind::ArrayOpen,
            Kind::ArrayClose,
            Kind::DictOpen,
            Kind::DictClose,
            Kind::BraceOpen,
            Kind::BraceClose,
            Kind::Keyword,
            Kind::Junk,
            Kind::Data,
        ];
        for (i, kind) in KINDS.into_iter().enumerate() {
            assert_eq!(kind as usize, i, "every kind, once");
            let token = Token {
                kind,
                closed: true,
                start: 0,
                bytes: b"obj",
            };
            assert_eq!(token.is_keyword(b"obj"), kind == Kind::Keyword, "{kind:?}");
            assert!(!token.is_keyword(b"ob"), "{kind:?}");
            assert!(!token.is_keyword(b"objx"), "{kind:?}");
            assert!(!token.is_keyword(b""), "{kind:?}");
        }
        // From the tokenizer: the keyword among a name, a string and junk of
        // the same bytes.
        let input = b"1 0 obj /obj (obj) obj) objx";
        let words: Vec<_> = Tokenizer::new(input)
            .filter(|t| t.is_keyword(b"obj"))
            .map(|t| t.start)
            .collect();
        assert_eq!(words, [4, 19]);
        assert_eq!(
            Tokenizer::new(b"endobj")
                .next()
                .map(|t| t.is_keyword(b"obj")),
            Some(false)
        );
    }
}
