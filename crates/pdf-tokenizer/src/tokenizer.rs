//! The tokenizer: a position in the input, and the rules for where each
//! kind of token ends.

use core::iter::FusedIterator;

use crate::chars::{is_regular, is_white_space};
use crate::token::{Kind, Token};

/// Splits bytes into [`Token`]s, from any offset.
///
/// As an iterator, it yields every token to the end of the input. Each is
/// at least one byte, and together they are the input from where it
/// started, with no gap and no overlap. It carries nothing from one token
/// to the next, so started at any token's first byte it yields that token
/// first. Cloning it is cheap, so a clone can look ahead.
///
/// After a `stream` keyword, or an inline image's `ID`, come bytes whose
/// length the tokenizer can't know. Iterating on reads them as tokens like
/// any others. To take them as data instead, take the white-space before
/// them with [`eol`](Self::eol) or [`space_byte`](Self::space_byte), then
/// the data with [`data`](Self::data) or [`data_until`](Self::data_until).
#[derive(Debug, Clone)]
pub struct Tokenizer<'a> {
    bytes: &'a [u8],
    /// Never past the end of `bytes`.
    pos: usize,
}

impl<'a> Tokenizer<'a> {
    /// The tokens of `bytes`, from the start.
    pub fn new(bytes: &'a [u8]) -> Self {
        Self::at(bytes, 0)
    }

    /// The tokens of `bytes`, from `offset`, or none when `offset` is past
    /// the end.
    pub fn at(bytes: &'a [u8], offset: usize) -> Self {
        Tokenizer {
            bytes,
            pos: offset.min(bytes.len()),
        }
    }

    /// The offset of the next token. (Not `position`: on a `&mut
    /// Tokenizer`, that would be [`Iterator::position`].)
    pub fn offset(&self) -> usize {
        self.pos
    }

    /// One end of line, CR LF, LF or CR, as a [`Kind::Space`] token, if one
    /// is next: what ends the line a `stream` keyword is on. ISO 32000-2
    /// allows CR LF or LF there, and the token's bytes say which came.
    pub fn eol(&mut self) -> Option<Token<'a>> {
        let len = match self.rest() {
            [b'\r', b'\n', ..] => 2,
            [b'\n' | b'\r', ..] => 1,
            _ => return None,
        };
        Some(self.take(Kind::Space, len))
    }

    /// One white-space byte, as a [`Kind::Space`] token, if one is next:
    /// what separates an inline image's `ID` from its data.
    pub fn space_byte(&mut self) -> Option<Token<'a>> {
        match self.rest() {
            [b, ..] if is_white_space(*b) => Some(self.take(Kind::Space, 1)),
            _ => None,
        }
    }

    /// The next `len` bytes, or as many as are left, as one [`Kind::Data`]
    /// token: a stream's data, when the caller trusts its `/Length`.
    pub fn data(&mut self, len: usize) -> Token<'a> {
        let len = len.min(self.rest().len());
        self.take(Kind::Data, len)
    }

    /// The bytes before the next `marker`, or all that are left when it
    /// never comes, as one [`Kind::Data`] token: a stream's data up to
    /// `endstream`, say. The marker itself is left to tokenize. An empty
    /// marker comes at once, and the data is empty.
    ///
    /// The search is for the bytes alone, wherever they are. A caller that
    /// wants a marker only after white-space, or only before a delimiter,
    /// finds it itself and calls [`data`](Self::data).
    pub fn data_until(&mut self, marker: &[u8]) -> Token<'a> {
        let rest = self.rest();
        let len = if marker.is_empty() {
            0
        } else {
            rest.windows(marker.len())
                .position(|w| w == marker)
                .unwrap_or(rest.len())
        };
        self.take(Kind::Data, len)
    }

    fn rest(&self) -> &'a [u8] {
        self.bytes.get(self.pos..).unwrap_or_default()
    }

    /// The next `len` bytes as a token of `kind`. `len` is never more than
    /// what's left.
    fn take(&mut self, kind: Kind, len: usize) -> Token<'a> {
        let start = self.pos;
        let bytes = self.rest().get(..len).unwrap_or_default();
        self.pos += bytes.len();
        Token { kind, start, bytes }
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        let rest = self.rest();
        let (&first, after) = rest.split_first()?;
        let (kind, len) = match first {
            b if is_white_space(b) => (Kind::Space, run(rest, is_white_space)),
            b'%' => (Kind::Comment, 1 + run(after, |b| b != b'\r' && b != b'\n')),
            b'/' => (Kind::Name, 1 + run(after, is_regular)),
            b'(' => literal_string(rest),
            b'<' if after.first() == Some(&b'<') => (Kind::DictOpen, 2),
            b'<' => hex_string(rest),
            b'>' if after.first() == Some(&b'>') => (Kind::DictClose, 2),
            b'>' | b')' => (Kind::Junk, 1),
            b'[' => (Kind::ArrayOpen, 1),
            b']' => (Kind::ArrayClose, 1),
            b'{' => (Kind::BraceOpen, 1),
            b'}' => (Kind::BraceClose, 1),
            // Every delimiter is above, so `first` is regular.
            _ => {
                let len = run(rest, is_regular);
                (word(rest.get(..len).unwrap_or_default()), len)
            }
        };
        Some(self.take(kind, len))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.rest().len();
        (usize::from(left > 0), Some(left))
    }
}

impl FusedIterator for Tokenizer<'_> {}

/// How many of `bytes`' first bytes `keep` holds for.
fn run(bytes: &[u8], keep: impl Fn(u8) -> bool) -> usize {
    bytes.iter().position(|&b| !keep(b)).unwrap_or(bytes.len())
}

/// What a run of regular bytes is: a number, when it's written as one
/// (ISO 32000-2, 7.3.3), and otherwise a keyword.
pub(crate) fn word(run: &[u8]) -> Kind {
    let digits = match run {
        [b'+' | b'-', digits @ ..] => digits,
        digits => digits,
    };
    let mut any_digit = false;
    let mut point = false;
    for &b in digits {
        match b {
            b'0'..=b'9' => any_digit = true,
            b'.' if !point => point = true,
            _ => return Kind::Keyword,
        }
    }
    match (any_digit, point) {
        (false, _) => Kind::Keyword,
        (true, false) => Kind::Integer,
        (true, true) => Kind::Real,
    }
}

/// A literal string at the start of `rest`, which starts with `(`: to the
/// `)` that balances it, past any byte a backslash escapes (ISO 32000-2,
/// 7.3.4.2).
fn literal_string(rest: &[u8]) -> (Kind, usize) {
    let mut depth = 0usize;
    let mut escaped = false;
    for (i, &b) in rest.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match b {
            b'\\' => escaped = true,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return (Kind::String { closed: true }, i + 1);
                }
            }
            _ => {}
        }
    }
    (Kind::String { closed: false }, rest.len())
}

/// A hexadecimal string at the start of `rest`, which starts with `<` and
/// not `<<`: to its `>`, or up to the first byte it can't hold (ISO
/// 32000-2, 7.3.4.3).
fn hex_string(rest: &[u8]) -> (Kind, usize) {
    for (i, &b) in rest.iter().enumerate().skip(1) {
        if b == b'>' {
            return (Kind::HexString { closed: true }, i + 1);
        }
        if !b.is_ascii_hexdigit() && !is_white_space(b) {
            return (Kind::HexString { closed: false }, i);
        }
    }
    (Kind::HexString { closed: false }, rest.len())
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use crate::{Kind, Token, Tokenizer, fuzz};

    const STRING: Kind = Kind::String { closed: true };
    const UNCLOSED_STRING: Kind = Kind::String { closed: false };
    const HEX: Kind = Kind::HexString { closed: true };
    const UNCLOSED_HEX: Kind = Kind::HexString { closed: false };

    /// `input`'s tokens, each kind and its bytes, checked by the fuzz
    /// target's body, whole and cut at every byte.
    fn tokens(input: &[u8]) -> Vec<(Kind, &[u8])> {
        let tokens: Vec<_> = Tokenizer::new(input).map(|t| (t.kind, t.bytes)).collect();
        fuzz::walk(input, 0);
        for cut in 0..=input.len() {
            fuzz::prefix(input, 0, cut);
        }
        tokens
    }

    fn kind_of(word: &[u8]) -> Kind {
        match tokens(word)[..] {
            [(kind, bytes)] if bytes == word => kind,
            ref other => panic!("{word:?} isn't one token: {other:?}"),
        }
    }

    #[test]
    fn white_space_is_one_token_per_run() {
        assert_eq!(
            tokens(b" \t\r\n\x0c\x00x"),
            [
                (Kind::Space, &b" \t\r\n\x0c\x00"[..]),
                (Kind::Keyword, b"x")
            ]
        );
    }

    #[test]
    fn a_comment_ends_before_its_end_of_line() {
        assert_eq!(
            tokens(b"%PDF-1.7\r\n%%EOF"),
            [
                (Kind::Comment, &b"%PDF-1.7"[..]),
                (Kind::Space, b"\r\n"),
                (Kind::Comment, b"%%EOF"),
            ]
        );
        assert_eq!(
            tokens(b"% (not a string) /nor a name\rx"),
            [
                (Kind::Comment, &b"% (not a string) /nor a name"[..]),
                (Kind::Space, b"\r"),
                (Kind::Keyword, b"x"),
            ]
        );
    }

    #[test]
    fn names_keep_their_escapes() {
        assert_eq!(
            tokens(b"/Type/A#20B/ /"),
            [
                (Kind::Name, &b"/Type"[..]),
                (Kind::Name, b"/A#20B"),
                (Kind::Name, b"/"),
                (Kind::Space, b" "),
                (Kind::Name, b"/"),
            ]
        );
    }

    #[test]
    fn numbers_are_written_as_iso_32000_writes_them() {
        for int in ["0", "+17", "-98", "0042", "123456789012345678901234567890"] {
            assert_eq!(kind_of(int.as_bytes()), Kind::Integer, "{int}");
        }
        for real in ["34.5", "-3.62", "+123.6", "4.", "-.002", "0.0", ".5", "+.5"] {
            assert_eq!(kind_of(real.as_bytes()), Kind::Real, "{real}");
        }
        for word in [
            "+", "-", ".", "-.", "1.2.3", "--5", "+-1", "1e5", "12abc", "0x1F",
        ] {
            assert_eq!(kind_of(word.as_bytes()), Kind::Keyword, "{word}");
        }
    }

    #[test]
    fn keywords_are_every_other_run_of_regular_bytes() {
        for word in [
            &b"obj"[..],
            b"R",
            b"true",
            b"null",
            b"stream",
            b"T*",
            b"'",
            b"\x80\xff",
        ] {
            assert_eq!(kind_of(word), Kind::Keyword, "{word:?}");
        }
        // A delimiter ends a keyword, as white-space does.
        assert_eq!(
            tokens(b"endobj%"),
            [(Kind::Keyword, &b"endobj"[..]), (Kind::Comment, b"%")]
        );
    }

    #[test]
    fn literal_strings_balance_their_parentheses() {
        assert_eq!(kind_of(b"(a (nested) string)"), STRING);
        assert_eq!(kind_of(b"(an escaped \\) one)"), STRING);
        assert_eq!(kind_of(b"(\\\\)"), STRING);
        assert_eq!(kind_of(b"(a line \\\r\ncontinued)"), STRING);
        assert_eq!(kind_of(b"(\xfe\xff\x00(\x00))"), STRING);
        assert_eq!(tokens(b"(a)(b)"), [(STRING, &b"(a)"[..]), (STRING, b"(b)")]);
    }

    #[test]
    fn an_unclosed_string_runs_to_the_end() {
        assert_eq!(kind_of(b"(open (and more) "), UNCLOSED_STRING);
        assert_eq!(kind_of(b"(ends in a backslash \\"), UNCLOSED_STRING);
        assert_eq!(kind_of(b"(escaped close \\)"), UNCLOSED_STRING);
    }

    #[test]
    fn hex_strings_end_at_their_close_or_before_a_byte_they_cant_hold() {
        assert_eq!(kind_of(b"<48 65\n6C>"), HEX);
        assert_eq!(kind_of(b"<>"), HEX);
        assert_eq!(kind_of(b"<4865"), UNCLOSED_HEX);
        assert_eq!(
            tokens(b"<48(x)"),
            [(UNCLOSED_HEX, &b"<48"[..]), (STRING, b"(x)")]
        );
        assert_eq!(
            tokens(b"<<<"),
            [(Kind::DictOpen, &b"<<"[..]), (UNCLOSED_HEX, b"<")]
        );
        assert_eq!(
            tokens(b"<g>"),
            [
                (UNCLOSED_HEX, &b"<"[..]),
                (Kind::Keyword, b"g"),
                (Kind::Junk, b">")
            ]
        );
    }

    #[test]
    fn delimiters_are_tokens_of_their_own() {
        assert_eq!(
            tokens(b"[]{}<<>>"),
            [
                (Kind::ArrayOpen, &b"["[..]),
                (Kind::ArrayClose, b"]"),
                (Kind::BraceOpen, b"{"),
                (Kind::BraceClose, b"}"),
                (Kind::DictOpen, b"<<"),
                (Kind::DictClose, b">>"),
            ]
        );
    }

    #[test]
    fn a_close_with_nothing_open_is_junk() {
        assert_eq!(
            tokens(b") > >>>"),
            [
                (Kind::Junk, &b")"[..]),
                (Kind::Space, b" "),
                (Kind::Junk, b">"),
                (Kind::Space, b" "),
                (Kind::DictClose, b">>"),
                (Kind::Junk, b">"),
            ]
        );
    }

    #[test]
    fn tokens_say_where_they_are() {
        let input = b"1 0 obj";
        let mut tokens = Tokenizer::at(input, 2);
        assert_eq!(tokens.offset(), 2);
        let zero = tokens.next().unwrap();
        assert_eq!((zero.kind, zero.start, zero.end()), (Kind::Integer, 2, 3));
        assert_eq!(tokens.offset(), 3);
        assert_eq!(Tokenizer::at(input, 99).next(), None);
        assert_eq!(Tokenizer::new(b"").next(), None);
    }

    #[test]
    fn a_streams_data_is_the_callers_to_measure() {
        let input = b"4 0 obj\n<< /Length 5 >>\nstream\r\nBT ET\nendstream\nendobj";
        let mut tokens = Tokenizer::new(input);
        let keyword = tokens
            .by_ref()
            .find(|t| t.kind == Kind::Keyword && t.start > 4);
        assert_eq!(keyword.map(|t| t.bytes), Some(&b"stream"[..]));
        assert_eq!(
            tokens.eol().map(|t| (t.kind, t.bytes)),
            Some((Kind::Space, &b"\r\n"[..]))
        );
        let data = tokens.data(5);
        assert_eq!((data.kind, data.bytes), (Kind::Data, &b"BT ET"[..]));
        let rest: Vec<_> = tokens.map(|t| (t.kind, t.bytes)).collect();
        assert_eq!(
            rest,
            [
                (Kind::Space, &b"\n"[..]),
                (Kind::Keyword, b"endstream"),
                (Kind::Space, b"\n"),
                (Kind::Keyword, b"endobj"),
            ]
        );
    }

    #[test]
    fn eol_takes_one_end_of_line() {
        let eol = |input: &'static [u8]| Tokenizer::new(input).eol().map(|t| t.bytes);
        assert_eq!(eol(b"\r\n\r\n"), Some(&b"\r\n"[..]));
        assert_eq!(eol(b"\n\n"), Some(&b"\n"[..]));
        assert_eq!(eol(b"\rx"), Some(&b"\r"[..]));
        assert_eq!(eol(b" \n"), None);
        assert_eq!(eol(b""), None);
    }

    #[test]
    fn space_byte_takes_one_white_space_byte() {
        let mut tokens = Tokenizer::new(b"ID \x00\x01EI");
        tokens.next();
        let space = tokens.space_byte().map(|t| t.bytes);
        assert_eq!(space, Some(&b" "[..]));
        assert_eq!(tokens.data_until(b"EI").bytes, b"\x00\x01");
        assert_eq!(tokens.space_byte(), None);
    }

    #[test]
    fn data_until_stops_before_its_marker_or_at_the_end() {
        let mut tokens = Tokenizer::new(b"abcendstream");
        assert_eq!(tokens.data_until(b"").bytes, b"");
        assert_eq!(tokens.data_until(b"endstream").bytes, b"abc");
        assert_eq!(tokens.data_until(b"endstream").bytes, b"");
        assert_eq!(tokens.next().map(|t| t.bytes), Some(&b"endstream"[..]));
        let mut tokens = Tokenizer::new(b"no marker");
        assert_eq!(tokens.data_until(b"endstream").bytes, b"no marker");
        assert_eq!(tokens.next(), None);
    }

    /// A slice of a file tokenizes as the whole file does but for its last
    /// token, which reaches the slice's end: `12` of `123`, `<` of `<<`,
    /// `>` of `>>`, a string cut short, white-space and a comment that go
    /// on, `\r` of the `\r\n` after `stream`, and data cut before its
    /// marker, or inside it.
    #[test]
    fn only_the_last_token_of_a_slice_can_come_out_otherwise() {
        let file = b"[123] << /A (a b) >>  % c\nstream\r\nxy endstream";
        let cases: [(usize, Kind, &[u8]); 9] = [
            (3, Kind::Integer, b"12"),
            (7, UNCLOSED_HEX, b"<"),
            (15, UNCLOSED_STRING, b"(a "),
            (19, Kind::Junk, b">"),
            (21, Kind::Space, b" "),
            (24, Kind::Comment, b"% "),
            (33, Kind::Space, b"\r"),
            (36, Kind::Data, b"xy"),
            (42, Kind::Data, b"xy endst"),
        ];
        // The tokens, each stream's data taken as a reader might: an end
        // of line, then up to `endstream`.
        let read = |input: &'static [u8]| {
            let mut tokens = Tokenizer::new(input);
            let mut read = Vec::new();
            while let Some(token) = tokens.next() {
                read.push(token);
                if token.bytes == b"stream" {
                    read.extend(tokens.eol());
                    if tokens.offset() < input.len() {
                        read.push(tokens.data_until(b"endstream"));
                    }
                }
            }
            read
        };
        let whole = read(file);
        for (cut, kind, bytes) in cases {
            let part = read(&file[..cut]);
            let (last, before) = part.split_last().unwrap();
            assert_eq!(before, &whole[..before.len()], "{cut}");
            assert_eq!((last.kind, last.bytes, last.end()), (kind, bytes, cut));
            assert_ne!(whole[before.len()], *last, "{cut}");
            fuzz::prefix(file, 0, cut);
        }
    }

    #[test]
    fn data_stops_at_the_end() {
        let mut tokens = Tokenizer::new(b"abc");
        let data = tokens.data(100);
        assert_eq!((data.start, data.bytes), (0, &b"abc"[..]));
        let empty: Token<'_> = tokens.data(1);
        assert_eq!((empty.start, empty.bytes), (3, &b""[..]));
    }
}
