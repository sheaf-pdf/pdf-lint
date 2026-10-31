//! The tokenizer: a position in the input, and the rules for where each
//! kind of token ends.

use core::iter::FusedIterator;

use memchr::{memchr, memchr2, memchr3, memmem};

use crate::chars::{SKIMMED, class, is_white_space};
use crate::rules::{Iso, RuleSet, Rules};
use crate::token::{Kind, Token};

/// Splits bytes into [`Token`]s, from any offset.
///
/// As an iterator, it yields every token to the end of the input. Each is
/// at least one byte, and together they are the input from where it
/// started, with no gap and no overlap. It carries nothing from one token
/// to the next, so started at any token's first byte it yields that token
/// first. Cloning it is cheap, so a clone can look ahead.
///
/// It follows ISO 32000-2's lexical rules unless it's given PDFium's
/// ([`with_rules`](Self::with_rules)), and keeps every promise under
/// either. `R` holds them: [`Iso`], the default, or
/// [`Pdfium`](crate::Pdfium), fixed in the type, or a [`Rules`] value,
/// chosen as it runs ([`RuleSet`]).
///
/// After a `stream` keyword, or an inline image's `ID`, come bytes whose
/// length the tokenizer can't know. Iterating on reads them as tokens like
/// any others. To take them as data instead, take the white-space before
/// them with [`eol`](Self::eol) or [`space_byte`](Self::space_byte), then
/// the data with [`data`](Self::data) or [`data_until`](Self::data_until).
#[derive(Debug, Clone)]
pub struct Tokenizer<'a, R: RuleSet = Iso> {
    bytes: &'a [u8],
    /// Never past the end of `bytes`.
    pos: usize,
    rules: R,
}

impl<'a> Tokenizer<'a> {
    /// The tokens of `bytes`, from the start, by ISO 32000-2's rules, fixed
    /// in its type.
    pub fn new(bytes: &'a [u8]) -> Self {
        Self::at(bytes, 0)
    }

    /// The tokens of `bytes`, from `offset`, or none when `offset` is past
    /// the end, by ISO 32000-2's rules, fixed in its type.
    pub fn at(bytes: &'a [u8], offset: usize) -> Self {
        Tokenizer {
            bytes,
            pos: offset.min(bytes.len()),
            rules: Iso,
        }
    }
}

impl<'a, R: RuleSet> Tokenizer<'a, R> {
    /// The same tokenizer, where it is, following `rules` from its next
    /// token on: `Tokenizer::at(bytes, offset).with_rules(Pdfium)` reads by
    /// PDFium's rules from any offset, fixed in its type, and
    /// `with_rules(Rules::Pdfium)` by the same rules, chosen as it runs.
    pub fn with_rules<S: RuleSet>(self, rules: S) -> Tokenizer<'a, S> {
        Tokenizer {
            bytes: self.bytes,
            pos: self.pos,
            rules,
        }
    }

    /// The rules it follows.
    pub fn rules(&self) -> Rules {
        self.rules.rules()
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
    /// what separates an inline image's `ID` from its data. White-space by
    /// the tokenizer's rules, so 0x80 and 0xFF too by PDFium's.
    pub fn space_byte(&mut self) -> Option<Token<'a>> {
        match self.rest() {
            [b, ..] if self.rules().is_white_space(*b) => Some(self.take(Kind::Space, 1)),
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
        let len = memmem::find(rest, marker).unwrap_or(rest.len());
        self.take(Kind::Data, len)
    }

    /// The next token that's neither white-space nor a comment, the
    /// white-space and comments before it passed over; `None` at the end,
    /// every token passed. It's the token iterating on to the first such
    /// one finds, and leaves the tokenizer where that leaves it, but it
    /// makes no token of the white-space (ISO 32000-2, 7.2.4, reads a
    /// comment as white-space too).
    ///
    /// ```
    /// use pdf_tokenizer::Tokenizer;
    ///
    /// let mut tokens = Tokenizer::new(b"  % a comment\n /Type");
    /// assert_eq!(tokens.next_significant().map(|t| t.bytes), Some(&b"/Type"[..]));
    /// assert_eq!(tokens.next_significant(), None);
    /// ```
    pub fn next_significant(&mut self) -> Option<Token<'a>> {
        let rules = self.rules();
        loop {
            let space = run(self.rest(), |b| rules.is_white_space(b));
            self.pos += space;
            let rest = self.rest();
            // The token function itself, inlined here as in `next`: a call
            // a token fewer.
            let (kind, len) = match rules {
                Rules::Iso => next_token::<false>(rest)?,
                Rules::Pdfium => next_token::<true>(rest)?,
            };
            if kind != Kind::Comment {
                return Some(self.token(rest, kind, len));
            }
            self.pos += len;
        }
    }

    /// The next token that opens or closes an array or a dictionary
    /// ([`Kind::ArrayOpen`], [`ArrayClose`](Kind::ArrayClose),
    /// [`DictOpen`](Kind::DictOpen), [`DictClose`](Kind::DictClose)), or a
    /// string that doesn't close, the tokens before it passed over; `None`
    /// at the end, every token passed. It's the token iterating on to the
    /// first such one finds, and leaves the tokenizer where that leaves it,
    /// but it looks only at the bytes that can start one, or a string or a
    /// comment that holds such bytes, and makes no token of the rest.
    ///
    /// From an array's or a dictionary's opening token, skimming on to the
    /// token that closes it passes it without reading what it holds. Which
    /// closes which, how deep they may nest, and what a string that doesn't
    /// close means are the caller's to say: the skim judges nothing.
    ///
    /// ```
    /// use pdf_tokenizer::{Kind, Tokenizer};
    ///
    /// let input = b"[1 (a]) <</A [2]>>] 3";
    /// let mut tokens = Tokenizer::new(input);
    /// let kinds: Vec<_> = core::iter::from_fn(|| tokens.skim()).map(|t| t.kind).collect();
    /// assert_eq!(
    ///     kinds,
    ///     [Kind::ArrayOpen, Kind::DictOpen, Kind::ArrayOpen, Kind::ArrayClose, Kind::DictClose, Kind::ArrayClose]
    /// );
    /// assert_eq!(tokens.offset(), input.len());
    /// ```
    pub fn skim(&mut self) -> Option<Token<'a>> {
        loop {
            let Some(i) = self.rest().iter().position(|&b| class(b) & SKIMMED != 0) else {
                self.pos = self.bytes.len();
                return None;
            };
            // No token before it runs past it: white-space, words and names
            // end at any delimiter, and `)`, `{` and `}` are a byte each. So
            // a token starts here.
            self.pos += i;
            let token = self.next()?;
            match token.kind {
                Kind::ArrayOpen
                | Kind::ArrayClose
                | Kind::DictOpen
                | Kind::DictClose
                | Kind::String { closed: false }
                | Kind::HexString { closed: false } => return Some(token),
                _ => {}
            }
        }
    }

    fn rest(&self) -> &'a [u8] {
        self.bytes.get(self.pos..).unwrap_or_default()
    }

    /// The next `len` bytes as a token of `kind`. `len` is never more than
    /// what's left.
    fn take(&mut self, kind: Kind, len: usize) -> Token<'a> {
        self.token(self.rest(), kind, len)
    }

    /// The first `len` bytes of `rest`, which is what's left, as a token of
    /// `kind`: `rest` taken once a token, by its caller. `len` is never more
    /// than its length, as each token function measures within what it's
    /// given (which the fuzz target checks on every token). A token function
    /// that measured past it would be a bug: a build with debug assertions
    /// stops at it, and one without reads the token to the input's end, so
    /// the tokenizer still never panics and still moves on.
    fn token(&mut self, rest: &'a [u8], kind: Kind, len: usize) -> Token<'a> {
        debug_assert!(len <= rest.len(), "a token measured past what's left");
        let start = self.pos;
        let bytes = &rest[..len.min(rest.len())];
        self.pos += bytes.len();
        Token { kind, start, bytes }
    }
}

impl<'a, R: RuleSet> Iterator for Tokenizer<'a, R> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        // Each set of rules is a loop of its own. Fixed in the type, the
        // rules are settled when this is compiled; chosen as it runs, they
        // cost a branch a token.
        let rest = self.rest();
        let (kind, len) = match self.rules() {
            Rules::Iso => next_token::<false>(rest)?,
            Rules::Pdfium => next_token::<true>(rest)?,
        };
        Some(self.token(rest, kind, len))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.rest().len();
        (usize::from(left > 0), Some(left))
    }
}

impl<R: RuleSet> FusedIterator for Tokenizer<'_, R> {}

/// The kind and length of the token `rest` starts with, by PDFium's rules
/// when `PDFIUM` and otherwise by ISO 32000-2's; `None` when it's empty.
// Inlined, with `literal_string`, into `next`: so each set of rules runs
// the instructions the tokenizer ran before there were two, where called
// they run about a tenth more on a file whose strings are long.
#[inline(always)]
fn next_token<const PDFIUM: bool>(rest: &[u8]) -> Option<(Kind, usize)> {
    let rules = if PDFIUM { Rules::Pdfium } else { Rules::Iso };
    let space = |b| rules.is_white_space(b);
    let regular = |b| rules.is_regular(b);
    let (&first, after) = rest.split_first()?;
    Some(match first {
        b if space(b) => (Kind::Space, run(rest, space)),
        b'%' => (Kind::Comment, 1 + line_end(after)),
        b'/' => (Kind::Name, 1 + run(after, regular)),
        b'(' => literal_string(rest),
        b'<' if after.first() == Some(&b'<') => (Kind::DictOpen, 2),
        b'<' if PDFIUM => pdfium_hex_string(rest),
        b'<' => hex_string(rest),
        b'>' if after.first() == Some(&b'>') => (Kind::DictClose, 2),
        b'>' | b')' => (Kind::Junk, 1),
        b'[' => (Kind::ArrayOpen, 1),
        b']' => (Kind::ArrayClose, 1),
        b'{' => (Kind::BraceOpen, 1),
        b'}' => (Kind::BraceClose, 1),
        // Every delimiter is above, so `first` is regular.
        _ => {
            let len = run(rest, regular);
            let run = rest.get(..len).unwrap_or_default();
            let kind = if PDFIUM { pdfium_word(run) } else { word(run) };
            (kind, len)
        }
    })
}

/// How many of `bytes`' first bytes `keep` holds for.
fn run(bytes: &[u8], keep: impl Fn(u8) -> bool) -> usize {
    bytes.iter().position(|&b| !keep(b)).unwrap_or(bytes.len())
}

/// How many of `bytes`' first bytes come before an end of line, CR or LF.
fn line_end(bytes: &[u8]) -> usize {
    memchr2(b'\r', b'\n', bytes).unwrap_or(bytes.len())
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

/// What a run of regular bytes is by PDFium's rules: a number when it's
/// all digits, signs and points, a real if a point is among them and an
/// integer if not; otherwise a keyword.
pub(crate) fn pdfium_word(run: &[u8]) -> Kind {
    let mut point = false;
    for &b in run {
        match b {
            b'0'..=b'9' | b'+' | b'-' => {}
            b'.' => point = true,
            _ => return Kind::Keyword,
        }
    }
    match (run.is_empty(), point) {
        (true, _) => Kind::Keyword,
        (false, false) => Kind::Integer,
        (false, true) => Kind::Real,
    }
}

/// A literal string at the start of `rest`, which starts with `(`: to the
/// `)` that balances it, past any byte a backslash escapes (ISO 32000-2,
/// 7.3.4.2).
// Inlined: see `next_token`.
#[inline(always)]
fn literal_string(rest: &[u8]) -> (Kind, usize) {
    let mut depth = 0usize;
    let mut at = 0;
    // From one parenthesis or backslash to the next: the bytes between
    // change nothing.
    while let Some(i) = rest.get(at..).and_then(|r| memchr3(b'(', b')', b'\\', r)) {
        at += i;
        match rest[at] {
            // The byte it escapes, whatever it is, is passed over too.
            b'\\' => at += 2,
            b'(' => {
                depth += 1;
                at += 1;
            }
            _ => {
                depth -= 1;
                at += 1;
                if depth == 0 {
                    return (Kind::String { closed: true }, at);
                }
            }
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

/// A hexadecimal string at the start of `rest`, which starts with `<` and
/// not `<<`, by PDFium's rules: to its `>`, over any byte.
fn pdfium_hex_string(rest: &[u8]) -> (Kind, usize) {
    match memchr(b'>', rest.get(1..).unwrap_or_default()) {
        Some(i) => (Kind::HexString { closed: true }, i + 2),
        None => (Kind::HexString { closed: false }, rest.len()),
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use crate::{Iso, Kind, Pdfium, Rules, Token, Tokenizer, fuzz};

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

    /// The inputs of the tests above, and more where a skim or a search
    /// could go wrong: closers with nothing open, strings and comments
    /// holding delimiters, strings that don't close, hex strings each set
    /// of rules ends otherwise, and markers at the start, at the end and
    /// cut short.
    const INPUTS: &[&[u8]] = &[
        b" \t\r\n\x0c\x00x",
        b"%PDF-1.7\r\n%%EOF",
        b"/Type/A#20B/ /",
        b"[1 (a]) <</A [2]>>] 3",
        b"[[1] 2] <<>> [<<]>>] >>> <<< ]]",
        b"(a (nested) string) (an escaped \\) one) (\\\\)",
        b"[(open (and more) ",
        b"[<48 65\n6C> <4G1> <48(x)> <g>]",
        b"<< /K [1 %] not a closer\r 2] >>\n% [\n]",
        b"4 0 obj\n<< /Length 5 >>\nstream\r\nBT ET\nendstream\nendobj",
        b"endstreamendstrea",
        b"ID \x00\x01EI",
        b"BI /W 1 ID xEI EI EI",
        b"[123] << /A (a b) >>  % c\nstream\r\nxy endstream",
        b"[--5.0 <4G 1>]\x80\xff x",
        b"[(\x80]\xff) /N\x80[ 1\xff]",
    ];

    /// A skim gives the next token that opens or closes an array or a
    /// dictionary, or a string that doesn't close, as iterating on to it
    /// does, under each set of rules: from every offset of the inputs.
    #[test]
    fn a_skim_gives_what_opens_or_closes_as_the_tokens_do() {
        let skims = |input: &'static [u8], rules| {
            let mut tokens = Tokenizer::new(input).with_rules(rules);
            core::iter::from_fn(move || tokens.skim().map(|t| (t.kind, t.bytes)))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            skims(b"[1 (a]) <</A [2]>>] 3", Rules::Iso),
            [
                (Kind::ArrayOpen, &b"["[..]),
                (Kind::DictOpen, b"<<"),
                (Kind::ArrayOpen, b"["),
                (Kind::ArrayClose, b"]"),
                (Kind::DictClose, b">>"),
                (Kind::ArrayClose, b"]"),
            ]
        );
        // A closer with nothing open is given too: which closes which is
        // the caller's to say. `>>>` is `>>` and `>`, `<<<` `<<` and `<`.
        assert_eq!(
            skims(b"] >>> <<<", Rules::Iso),
            [
                (Kind::ArrayClose, &b"]"[..]),
                (Kind::DictClose, b">>"),
                (Kind::DictOpen, b"<<"),
                (UNCLOSED_HEX, b"<"),
            ]
        );
        // A hex string ISO 32000-2's rules end before a byte they can't
        // hold doesn't close, and the skim goes on after it.
        assert_eq!(
            skims(b"[<4G1>]", Rules::Iso),
            [
                (Kind::ArrayOpen, &b"["[..]),
                (UNCLOSED_HEX, b"<4"),
                (Kind::ArrayClose, b"]"),
            ]
        );
        assert_eq!(
            skims(b"[<4G1>] [(a", Rules::Pdfium),
            [
                (Kind::ArrayOpen, &b"["[..]),
                (Kind::ArrayClose, b"]"),
                (Kind::ArrayOpen, b"["),
                (UNCLOSED_STRING, b"(a"),
            ]
        );
        // A comment hides what it holds to its end of line.
        assert_eq!(
            skims(b"%[]\r]", Rules::Iso),
            [(Kind::ArrayClose, &b"]"[..])]
        );
        for input in INPUTS {
            for at in 0..=input.len() {
                fuzz::skimmed(input, at, Iso);
                fuzz::skimmed(input, at, Pdfium);
            }
        }
    }

    /// The next significant token is the next that's neither white-space
    /// nor a comment, as iterating on to it finds it: from every offset of
    /// the inputs, under each set of rules, here chosen as the tokenizer
    /// runs (the skim's test has them fixed in its type).
    #[test]
    fn next_significant_passes_white_space_and_comments() {
        let input = b" %a\r\n%b\n\x80/N %c";
        let mut tokens = Tokenizer::new(input);
        let word = tokens.next_significant().unwrap();
        assert_eq!(
            (word.kind, word.start, word.bytes),
            (Kind::Keyword, 8, &b"\x80"[..])
        );
        let mut tokens = Tokenizer::new(input).with_rules(Pdfium);
        let name = tokens.next_significant().unwrap();
        assert_eq!(
            (name.kind, name.start, name.bytes),
            (Kind::Name, 9, &b"/N"[..])
        );
        assert_eq!(tokens.next_significant(), None);
        assert_eq!(tokens.offset(), input.len(), "every token passed");
        for input in INPUTS {
            for at in 0..=input.len() {
                fuzz::skimmed(input, at, Rules::Iso);
                fuzz::skimmed(input, at, Rules::Pdfium);
            }
        }
    }

    /// `data_until` stops where a look at every window finds its marker
    /// first, or at the end: from every offset of the inputs.
    #[test]
    fn data_until_stops_where_its_marker_first_is() {
        let input = b"endstreamendstrea";
        assert_eq!(Tokenizer::new(input).data_until(b"endstream").bytes, b"");
        let cut = Tokenizer::at(input, 1).data_until(b"endstream");
        assert_eq!(
            (cut.start, cut.bytes),
            (1, &input[1..]),
            "a marker cut short is none"
        );
        let mut tokens = Tokenizer::at(b"BI /W 1 ID xEI EI EI", 11);
        assert_eq!(tokens.data_until(b" EI").bytes, b"xEI");
        for input in INPUTS {
            for at in 0..=input.len() + 1 {
                fuzz::searched(input, at);
            }
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

    /// A token measured past what's left, which no token function does, is
    /// a bug: a build with debug assertions panics at it, and one without
    /// reads the token to the input's end and moves on.
    #[test]
    fn a_token_measured_past_the_input_stops_only_a_debug_build() {
        let read = std::panic::catch_unwind(|| {
            let mut tokens = Tokenizer::new(b"abc");
            tokens.pos = 1;
            let rest = tokens.rest();
            let token = tokens.token(rest, Kind::Keyword, 5);
            (token.start, token.bytes, tokens.offset(), tokens.next())
        });
        if cfg!(debug_assertions) {
            assert!(read.is_err(), "a debug build stops at the bug");
        } else {
            assert_eq!(read.ok(), Some((1, &b"bc"[..], 3, None)));
        }
    }

    /// The tokenizer under PDFium's rules, where they part from ISO
    /// 32000-2's: what PDFium 7881 and 8076 read the same bytes as, one-page
    /// files through their page objects (2026-10-30).
    mod pdfiums_rules {
        extern crate std;

        use std::vec::Vec;

        use super::{HEX, STRING, UNCLOSED_HEX, kind_of, tokens};
        use crate::{Iso, Kind, Pdfium, Rules, Tokenizer, fuzz};

        /// `input`'s tokens by PDFium's rules, each kind and its bytes,
        /// checked by the fuzz target's body, whole and cut at every byte,
        /// under both sets of rules and side by side.
        fn pdfium(input: &[u8]) -> Vec<(Kind, &[u8])> {
            let tokenizer = Tokenizer::new(input).with_rules(Rules::Pdfium);
            let tokens = tokenizer.map(|t| (t.kind, t.bytes)).collect();
            fuzz::walk(input, 0);
            for cut in 0..=input.len() {
                fuzz::prefix(input, 0, cut);
            }
            tokens
        }

        fn pdfium_kind_of(word: &[u8]) -> Kind {
            match pdfium(word)[..] {
                [(kind, bytes)] if bytes == word => kind,
                ref other => panic!("{word:?} isn't one token: {other:?}"),
            }
        }

        #[test]
        fn new_and_at_follow_iso_32000s_rules() {
            assert_eq!(Tokenizer::new(b"--5").rules(), Rules::Iso);
            assert_eq!(Tokenizer::at(b"--5", 1).rules(), Rules::Iso);
            assert_eq!(Tokenizer::new(b"--5").next().unwrap().kind, Kind::Keyword);
        }

        /// A number word is digits, signs and points, a real when there's a
        /// point: in `cm`, `re` and a `/MediaBox`, PDFium reads each of these
        /// as a number, and `1e5` as an operator.
        #[test]
        fn a_number_is_any_word_of_digits_signs_and_points() {
            for int in [
                "--5",
                "5-3",
                "+",
                "-",
                "+-1",
                "007",
                "4294967296",
                "-0",
                "+5",
            ] {
                assert_eq!(pdfium_kind_of(int.as_bytes()), Kind::Integer, "{int}");
                assert_eq!(kind_of(int.as_bytes()), iso(int), "{int}");
            }
            for real in [
                "1.2.3", "--5.0", "+-.5", ".", "5.", "-.", "+.", ".5.", "1..2", "34.5",
            ] {
                assert_eq!(pdfium_kind_of(real.as_bytes()), Kind::Real, "{real}");
                assert_eq!(kind_of(real.as_bytes()), iso(real), "{real}");
            }
            for word in ["1e5", "12abc", "0x1F", "3.4e38", "inf", "--5x"] {
                assert_eq!(pdfium_kind_of(word.as_bytes()), Kind::Keyword, "{word}");
                assert_eq!(kind_of(word.as_bytes()), Kind::Keyword, "{word}");
            }
        }

        /// What ISO 32000-2's rules make of the same word.
        fn iso(word: &str) -> Kind {
            match word {
                "007" | "4294967296" | "-0" | "+5" => Kind::Integer,
                "5." | "34.5" => Kind::Real,
                _ => Kind::Keyword,
            }
        }

        #[test]
        fn a_hex_string_runs_to_its_close_over_any_byte() {
            for hex in [
                &b"<41G42>"[..],
                b"<4G1>",
                b"<g>",
                b"<41(42>",
                b"<41/42>",
                b"<41\x80%]>",
            ] {
                assert_eq!(pdfium_kind_of(hex), HEX, "{hex:?}");
                assert_ne!(kind_of_iso(hex), HEX, "{hex:?}");
            }
            assert_eq!(
                pdfium(b"<48(x)>)"),
                [(HEX, &b"<48(x)>"[..]), (Kind::Junk, b")")]
            );
            assert_eq!(
                pdfium(b"<<<>"),
                [(Kind::DictOpen, &b"<<"[..]), (HEX, b"<>")]
            );
            assert_eq!(pdfium(b"<41><42>"), [(HEX, &b"<41>"[..]), (HEX, b"<42>")]);
        }

        /// ISO 32000-2's first token of `input`'s kind.
        fn kind_of_iso(input: &[u8]) -> Kind {
            tokens(input)[0].0
        }

        #[test]
        fn a_hex_string_is_unclosed_only_at_the_end() {
            for hex in [&b"<4865"[..], b"<48(x) Tj", b"<", b"<48 >"] {
                let kind = if hex.ends_with(b">") {
                    HEX
                } else {
                    UNCLOSED_HEX
                };
                assert_eq!(pdfium_kind_of(hex), kind, "{hex:?}");
            }
            assert_eq!(
                pdfium(b"(a)<"),
                [(STRING, &b"(a)"[..]), (UNCLOSED_HEX, b"<")]
            );
        }

        /// 0x80 and 0xFF are white-space to PDFium: in content, `1 2 3
        /// 4<0x80>re` is a rectangle and `/F0<0x80>10 Tf` names `F0`; in a
        /// `/MediaBox`, `[0<0x80>0 100 100]` is four numbers. No other byte is.
        #[test]
        fn white_space_takes_0x80_and_0xff() {
            assert_eq!(
                pdfium(b"4\x80re /F0\xff\x80 \xff10"),
                [
                    (Kind::Integer, &b"4"[..]),
                    (Kind::Space, b"\x80"),
                    (Kind::Keyword, b"re"),
                    (Kind::Space, b" "),
                    (Kind::Name, b"/F0"),
                    (Kind::Space, b"\xff\x80 \xff"),
                    (Kind::Integer, b"10"),
                ]
            );
            assert_eq!(
                tokens(b"4\x80re"),
                [(Kind::Keyword, &b"4\x80re"[..])],
                "regular to ISO 32000-2"
            );
            for b in [0x0B, 0x7F, 0x85, 0xA0] {
                assert_eq!(
                    pdfium(&[b'4', b, b'x']),
                    [(Kind::Keyword, &[b'4', b, b'x'][..])]
                );
            }
            // A comment and strings hold them by either rules.
            assert_eq!(
                pdfium(b"%\x80\n(\xff)<\x80>"),
                [
                    (Kind::Comment, &b"%\x80"[..]),
                    (Kind::Space, b"\n"),
                    (STRING, b"(\xff)"),
                    (HEX, b"<\x80>"),
                ]
            );
        }

        /// The one white-space byte PDFium takes after an inline image's
        /// `ID` may be 0x80 or 0xFF.
        #[test]
        fn space_byte_follows_the_rules() {
            let after_id = |rules| {
                let mut tokens = Tokenizer::at(b"ID\x80a EI", 2).with_rules(rules);
                tokens.space_byte().map(|t| t.bytes)
            };
            assert_eq!(after_id(Rules::Iso), None);
            assert_eq!(after_id(Rules::Pdfium), Some(&b"\x80"[..]));
        }

        /// Fixed in the type or chosen as it runs, the rules read the same
        /// tokens, and `new` and `at` give ISO 32000-2's, fixed:
        /// `Tokenizer<'a>` is `Tokenizer<'a, Iso>`.
        #[test]
        fn rules_fixed_in_the_type_read_as_rules_chosen() {
            let input = b"1 --5 <4G> 2\x80ID\xffx";
            let iso: Tokenizer<'_, Iso> = Tokenizer::new(input);
            assert!(iso.clone().eq(Tokenizer::new(input).with_rules(Rules::Iso)));
            let fixed: Tokenizer<'_, Pdfium> = iso.with_rules(Pdfium);
            assert_eq!(fixed.rules(), Rules::Pdfium);
            let chosen = Tokenizer::new(input).with_rules(Rules::Pdfium);
            assert!(fixed.eq(chosen));
            let mut after_id = Tokenizer::at(input, 15).with_rules(Pdfium);
            assert_eq!(after_id.space_byte().map(|t| t.bytes), Some(&b"\xff"[..]));
            assert_eq!(Tokenizer::at(input, 15).space_byte(), None);
            pdfium(input);
        }

        /// From any offset, and from where a tokenizer is: `with_rules`
        /// keeps its place and changes how the next token reads.
        #[test]
        fn with_rules_reads_from_any_offset() {
            let input = b"1 --5 <4G> 2";
            let mut tokens = Tokenizer::at(input, 2).with_rules(Rules::Pdfium);
            assert_eq!((tokens.offset(), tokens.rules()), (2, Rules::Pdfium));
            let word = tokens.next().unwrap();
            assert_eq!(
                (word.kind, word.start, word.bytes),
                (Kind::Integer, 2, &b"--5"[..])
            );
            let mut tokens = tokens.with_rules(Rules::Iso);
            assert_eq!(tokens.offset(), 5);
            tokens.next();
            let hex = tokens.next().unwrap();
            assert_eq!((hex.kind, hex.bytes), (UNCLOSED_HEX, &b"<4"[..]));
            let hex = Tokenizer::at(input, 6).with_rules(Rules::Pdfium).next();
            assert_eq!(hex.map(|t| (t.kind, t.bytes)), Some((HEX, &b"<4G>"[..])));
            assert_eq!(
                Tokenizer::at(input, 99).with_rules(Rules::Pdfium).next(),
                None
            );
        }

        /// A slice by PDFium's rules tokenizes as the whole does but for its
        /// last token: a hex string cut before its `>`, a number word cut
        /// short, white-space that goes on over 0x80.
        #[test]
        fn only_the_last_token_of_a_slice_can_come_out_otherwise() {
            let file = b"[--5.0 <4G 1>]\x80\xff x";
            let cases: [(usize, Kind, &[u8]); 4] = [
                (4, Kind::Integer, b"--5"),
                (11, UNCLOSED_HEX, b"<4G "),
                (15, Kind::Space, b"\x80"),
                (16, Kind::Space, b"\x80\xff"),
            ];
            let whole = pdfium(file);
            for (cut, kind, bytes) in cases {
                let part = pdfium(&file[..cut]);
                let (last, before) = part.split_last().unwrap();
                assert_eq!(before, &whole[..before.len()], "{cut}");
                assert_eq!(*last, (kind, bytes), "{cut}");
                assert_ne!(whole[before.len()], *last, "{cut}");
            }
        }

        /// Side by side, the rules give the same tokens but where they part.
        #[test]
        fn the_rules_part_only_where_they_say() {
            let input = b"q 1 0 0 1 --5 1.2.3 cm /F#3G 1 Tf (a\rb) Tj <4G1> Tj [1e5 2]\x80re";
            let iso = tokens(input);
            let pdfium = pdfium(input);
            let differ: Vec<_> = pdfium.iter().filter(|t| !iso.contains(t)).collect();
            assert_eq!(
                differ,
                [
                    &(Kind::Integer, &b"--5"[..]),
                    &(Kind::Real, b"1.2.3"),
                    &(HEX, b"<4G1>"),
                    &(Kind::Space, b"\x80"),
                    &(Kind::Keyword, b"re"),
                ]
            );
        }
    }
}
