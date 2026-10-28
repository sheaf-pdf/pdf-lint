//! What a token stands for: a name's or a string's bytes, or a number.
//!
//! Decoding hides nothing. Every byte of a name or string is in exactly one
//! [`Piece`], which says what value byte it stands for and how, so each
//! escape, and each place where readers or PDF versions differ, is there to
//! see, with where it was written. A decoder still gives a byte at those
//! places, and [`How`] says which it chose.

use core::fmt;
use core::ops::Range;

use crate::chars::{is_regular, is_white_space};
use crate::token::{Kind, Token};
use crate::tokenizer::word;

/// A run of a name's or string's bytes, and the value byte it stands for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Piece {
    /// Where it is: offsets in the input, or in the bytes given to
    /// [`name`], [`literal`] or [`hex`].
    pub at: Range<usize>,
    /// The value byte it stands for, if any.
    pub byte: Option<u8>,
    /// How it stands for it.
    pub how: How,
}

/// How a [`Piece`] stands for its byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum How {
    /// `/`, `(`, `)`, `<` or `>`, which delimits the value: no byte.
    Delimiter,
    /// A byte as written.
    Plain,
    /// An escape: a name's `#20`, a string's `\n`, `\(` or `\101`, or a
    /// hex string's pair of digits, which white-space may split.
    Escape,
    /// A string's `\` and the end of line after it, which together stand
    /// for nothing.
    LineContinuation,
    /// White-space between a hex string's digits: no byte.
    HexSpace,
    /// A `#` in a name without two hex digits after it: `#`, as before
    /// PDF 1.2 (qpdf reads NUL).
    StrayHash,
    /// `#00` in a name: NUL, which a name may not hold (qpdf drops the
    /// value).
    NulEscape,
    /// A string's octal escape over 255, such as `\777`: its low eight
    /// bits, as ISO 32000-2 says.
    OctalOverflow,
    /// A string's `\` before a byte that has no escape, such as `\q`: the
    /// byte, with the backslash ignored, as ISO 32000-2 says.
    IgnoredBackslash,
    /// An unescaped CR, or CR LF, in a string: LF, as ISO 32000-2 says. An
    /// unescaped LF is [`Plain`](How::Plain).
    EolAsLf,
    /// A hex string's last digit, with no digit to pair with: as if `0`
    /// followed it.
    OddDigit,
    /// A `\` that an unclosed string ends on: no byte.
    Dangling,
    /// A byte the syntax doesn't allow there, such as a letter in a hex
    /// string, or anything after a string's close: no byte. A token from
    /// the tokenizer never has one.
    Invalid,
}

impl How {
    const ALL: [How; 13] = [
        How::Delimiter,
        How::Plain,
        How::Escape,
        How::LineContinuation,
        How::HexSpace,
        How::StrayHash,
        How::NulEscape,
        How::OctalOverflow,
        How::IgnoredBackslash,
        How::EolAsLf,
        How::OddDigit,
        How::Dangling,
        How::Invalid,
    ];

    /// Whether it's out of the ordinary: a place where readers, PDF
    /// versions or a writer's intent may differ, or an error. Delimiters,
    /// plain bytes, escapes, line continuations and a hex string's
    /// white-space aren't.
    pub const fn is_quirk(self) -> bool {
        !matches!(
            self,
            How::Delimiter | How::Plain | How::Escape | How::LineContinuation | How::HexSpace
        )
    }
}

/// A set of quirks: the [`How`]s out of the ordinary among some pieces.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Quirks(u16);

impl Quirks {
    /// Whether `how` is among them.
    pub const fn contains(self, how: How) -> bool {
        self.0 & (1 << how as u16) != 0
    }

    /// Whether there are none.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn insert(&mut self, how: How) {
        if how.is_quirk() {
            self.0 |= 1 << how as u16;
        }
    }
}

impl fmt::Debug for Quirks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let quirks = How::ALL.into_iter().filter(|&how| self.contains(how));
        f.debug_set().entries(quirks).finish()
    }
}

/// The [`Piece`]s of a name, a literal string or a hex string, in order.
/// They join into the bytes given, with no gap and no overlap.
#[derive(Debug, Clone)]
pub struct Pieces<'a> {
    bytes: &'a [u8],
    base: usize,
    pos: usize,
    syntax: Syntax,
    /// A literal string's parentheses still open, its first included.
    depth: usize,
    /// Whether the value's closing delimiter has come.
    closed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Syntax {
    Name,
    Literal,
    Hex,
}

/// The pieces of a name's bytes, `/` included (ISO 32000-2, 7.3.5).
pub fn name(bytes: &[u8]) -> Pieces<'_> {
    Pieces::new(bytes, 0, Syntax::Name)
}

/// The pieces of a literal string's bytes, parentheses included (ISO
/// 32000-2, 7.3.4.2).
pub fn literal(bytes: &[u8]) -> Pieces<'_> {
    Pieces::new(bytes, 0, Syntax::Literal)
}

/// The pieces of a hexadecimal string's bytes, `<` and `>` included (ISO
/// 32000-2, 7.3.4.3).
pub fn hex(bytes: &[u8]) -> Pieces<'_> {
    Pieces::new(bytes, 0, Syntax::Hex)
}

impl<'a> Pieces<'a> {
    fn new(bytes: &'a [u8], base: usize, syntax: Syntax) -> Self {
        Pieces {
            bytes,
            base,
            pos: 0,
            syntax,
            depth: 0,
            closed: false,
        }
    }

    /// The value's bytes, from the pieces left.
    pub fn bytes(self) -> impl Iterator<Item = u8> + 'a {
        self.filter_map(|piece| piece.byte)
    }

    /// The quirks among the pieces left.
    pub fn quirks(self) -> Quirks {
        self.fold(Quirks::default(), |mut quirks, piece| {
            quirks.insert(piece.how);
            quirks
        })
    }

    fn name(&self, first: u8, after: &[u8]) -> (usize, Option<u8>, How) {
        match (first, after) {
            (b'/', _) if self.pos == 0 => (1, None, How::Delimiter),
            (b'#', [high, low, ..]) if high.is_ascii_hexdigit() && low.is_ascii_hexdigit() => {
                let byte = (digit(*high) << 4) | digit(*low);
                let how = if byte == 0 {
                    How::NulEscape
                } else {
                    How::Escape
                };
                (3, Some(byte), how)
            }
            (b'#', _) => (1, Some(b'#'), How::StrayHash),
            (b, _) if is_regular(b) => (1, Some(b), How::Plain),
            _ => (1, None, How::Invalid),
        }
    }

    fn literal(&mut self, first: u8, after: &[u8]) -> (usize, Option<u8>, How) {
        match (first, after) {
            (b'(', _) if self.pos == 0 => {
                self.depth = 1;
                (1, None, How::Delimiter)
            }
            (b'\\', after) => escape(after),
            (b'\r', [b'\n', ..]) => (2, Some(b'\n'), How::EolAsLf),
            (b'\r', _) => (1, Some(b'\n'), How::EolAsLf),
            (b'(', _) => {
                self.depth += 1;
                (1, Some(b'('), How::Plain)
            }
            (b')', _) if self.depth == 1 => {
                self.closed = true;
                (1, None, How::Delimiter)
            }
            (b')', _) => {
                self.depth = self.depth.saturating_sub(1);
                (1, Some(b')'), How::Plain)
            }
            (b, _) => (1, Some(b), How::Plain),
        }
    }

    fn hex(&mut self, first: u8, after: &[u8]) -> (usize, Option<u8>, How) {
        match first {
            b'<' if self.pos == 0 => (1, None, How::Delimiter),
            b'>' => {
                self.closed = true;
                (1, None, How::Delimiter)
            }
            b if is_white_space(b) => (1, None, How::HexSpace),
            high if high.is_ascii_hexdigit() => {
                let gap = after.iter().take_while(|&&b| is_white_space(b)).count();
                match after.get(gap) {
                    Some(&low) if low.is_ascii_hexdigit() => {
                        (gap + 2, Some((digit(high) << 4) | digit(low)), How::Escape)
                    }
                    _ => (1, Some(digit(high) << 4), How::OddDigit),
                }
            }
            _ => (1, None, How::Invalid),
        }
    }
}

impl Iterator for Pieces<'_> {
    type Item = Piece;

    fn next(&mut self) -> Option<Piece> {
        let rest = self.bytes.get(self.pos..).unwrap_or_default();
        let (&first, after) = rest.split_first()?;
        let (len, byte, how) = if self.closed {
            (1, None, How::Invalid)
        } else {
            match self.syntax {
                Syntax::Name => self.name(first, after),
                Syntax::Literal => self.literal(first, after),
                Syntax::Hex => self.hex(first, after),
            }
        };
        let start = self.base + self.pos;
        self.pos += len;
        Some(Piece {
            at: start..start + len,
            byte,
            how,
        })
    }
}

/// A literal string's escape: `after` is what follows its backslash, and
/// the length counts the backslash.
fn escape(after: &[u8]) -> (usize, Option<u8>, How) {
    match after {
        [] => (1, None, How::Dangling),
        [b'n', ..] => (2, Some(b'\n'), How::Escape),
        [b'r', ..] => (2, Some(b'\r'), How::Escape),
        [b't', ..] => (2, Some(b'\t'), How::Escape),
        [b'b', ..] => (2, Some(0x08), How::Escape),
        [b'f', ..] => (2, Some(0x0C), How::Escape),
        [b @ (b'(' | b')' | b'\\'), ..] => (2, Some(*b), How::Escape),
        [b'\r', b'\n', ..] => (3, None, How::LineContinuation),
        [b'\r' | b'\n', ..] => (2, None, How::LineContinuation),
        [b'0'..=b'7', ..] => {
            let mut digits = 0;
            let mut value = 0u16;
            for &b in after.iter().take(3) {
                if !matches!(b, b'0'..=b'7') {
                    break;
                }
                value = value * 8 + u16::from(b - b'0');
                digits += 1;
            }
            let how = if value > 0xFF {
                How::OctalOverflow
            } else {
                How::Escape
            };
            (1 + digits, Some((value & 0xFF) as u8), how)
        }
        [b, ..] => (2, Some(*b), How::IgnoredBackslash),
    }
}

/// A hex digit's value.
fn digit(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

/// Why a token isn't the number asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum NumberError {
    /// It isn't written as one.
    #[error("not a number")]
    NotANumber,
    /// It's written as one, but too large: an integer outside `i64`, or a
    /// number too large for a finite `f64`. Readers differ here: qpdf
    /// reads the whole object holding it as null.
    #[error("a number too large to hold")]
    Overflow,
}

/// The integer `bytes` are written as: digits, with an optional sign (ISO
/// 32000-2, 7.3.3).
pub fn integer(bytes: &[u8]) -> Result<i64, NumberError> {
    if word(bytes) != Kind::Integer {
        return Err(NumberError::NotANumber);
    }
    let text = core::str::from_utf8(bytes).map_err(|_| NumberError::NotANumber)?;
    // Only digits and a sign are left, so a failure is an overflow.
    text.parse().map_err(|_| NumberError::Overflow)
}

/// The number `bytes` are written as, an integer or a real, as an `f64`
/// (ISO 32000-2, 7.3.3). Forms Rust reads but PDF doesn't write, such as
/// `1e5` or `inf`, aren't numbers.
pub fn number(bytes: &[u8]) -> Result<f64, NumberError> {
    if !matches!(word(bytes), Kind::Integer | Kind::Real) {
        return Err(NumberError::NotANumber);
    }
    let text = core::str::from_utf8(bytes).map_err(|_| NumberError::NotANumber)?;
    let value: f64 = text.parse().map_err(|_| NumberError::NotANumber)?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(NumberError::Overflow)
    }
}

impl<'a> Token<'a> {
    /// The pieces of a name, a literal string or a hex string, at their
    /// offsets in the input. `None` for any other kind.
    pub fn pieces(&self) -> Option<Pieces<'a>> {
        let syntax = match self.kind {
            Kind::Name => Syntax::Name,
            Kind::String { .. } => Syntax::Literal,
            Kind::HexString { .. } => Syntax::Hex,
            _ => return None,
        };
        Some(Pieces::new(self.bytes, self.start, syntax))
    }

    /// A name's or string's value as written, borrowed from the input,
    /// when nothing in it needs decoding: every piece but its delimiters is
    /// plain (a name without `#`, a literal string without `\` or CR, an
    /// empty hex string). `None` when something does, its value then being
    /// its pieces' [`bytes`](Pieces::bytes), and for any other kind.
    pub fn verbatim(&self) -> Option<&'a [u8]> {
        let mut plain: Option<Range<usize>> = None;
        for piece in self.pieces()? {
            match piece.how {
                How::Delimiter => {}
                How::Plain => {
                    let start = plain.map_or(piece.at.start, |run| run.start);
                    plain = Some(start..piece.at.end);
                }
                _ => return None,
            }
        }
        let run = plain.map_or(0..0, |run| run.start - self.start..run.end - self.start);
        Some(self.bytes.get(run).unwrap_or_default())
    }

    /// The integer an [`Integer`](Kind::Integer) token is written as.
    pub fn integer(&self) -> Result<i64, NumberError> {
        match self.kind {
            Kind::Integer => integer(self.bytes),
            _ => Err(NumberError::NotANumber),
        }
    }

    /// The number an [`Integer`](Kind::Integer) or [`Real`](Kind::Real)
    /// token is written as, as an `f64`.
    pub fn number(&self) -> Result<f64, NumberError> {
        match self.kind {
            Kind::Integer | Kind::Real => number(self.bytes),
            _ => Err(NumberError::NotANumber),
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use crate::decode::{self, How, NumberError, Piece, Pieces};
    use crate::{Token, Tokenizer, fuzz};

    /// The one token `input` is, checked as the fuzz target checks it.
    fn token(input: &[u8]) -> Token<'_> {
        let mut tokens = Tokenizer::new(input);
        let token = tokens.next().expect("a token");
        assert_eq!(tokens.next(), None, "{input:?} is one token");
        fuzz::walk(input, 0);
        token
    }

    fn value(pieces: Pieces<'_>) -> Vec<u8> {
        pieces.bytes().collect()
    }

    /// What `input`, one token, decodes to, and the quirks along the way.
    fn decoded(input: &[u8]) -> (Vec<u8>, Vec<How>) {
        let pieces = token(input).pieces().expect("a name or string");
        let quirks = pieces.clone().map(|p| p.how).filter(|h| h.is_quirk());
        (value(pieces.clone()), quirks.collect())
    }

    fn piece(at: core::ops::Range<usize>, byte: Option<u8>, how: How) -> Piece {
        Piece { at, byte, how }
    }

    #[test]
    fn a_names_pieces_say_where_each_byte_came_from() {
        let pieces: Vec<_> = token(b"/A#20B").pieces().unwrap().collect();
        assert_eq!(
            pieces,
            [
                piece(0..1, None, How::Delimiter),
                piece(1..2, Some(b'A'), How::Plain),
                piece(2..5, Some(b' '), How::Escape),
                piece(5..6, Some(b'B'), How::Plain),
            ]
        );
        // From a token, offsets are the input's.
        let input = b"<< /A#20B >>";
        let name = Tokenizer::new(input).nth(2).unwrap();
        assert_eq!(name.pieces().unwrap().nth(2).unwrap().at, 5..8);
    }

    #[test]
    fn a_names_hex_digits_are_either_case() {
        assert_eq!(decoded(b"/#4a#4A#41"), (b"JJA".to_vec(), Vec::new()));
    }

    #[test]
    fn a_stray_hash_stays_a_hash() {
        for (name, meant) in [
            (&b"/A#"[..], &b"A#"[..]),
            (b"/A#G1", b"A#G1"),
            (b"/A#2", b"A#2"),
        ] {
            assert_eq!(decoded(name), (meant.to_vec(), [How::StrayHash].to_vec()));
        }
    }

    #[test]
    fn a_nul_escape_is_nul_but_said_so() {
        assert_eq!(
            decoded(b"/A#00B"),
            (b"A\0B".to_vec(), [How::NulEscape].to_vec())
        );
    }

    #[test]
    fn a_strings_escapes_stand_for_their_bytes() {
        let (meant, quirks) = decoded(b"(\\n\\r\\t\\b\\f\\(\\)\\\\)");
        assert_eq!(meant, b"\n\r\t\x08\x0c()\\");
        assert!(quirks.is_empty());
    }

    #[test]
    fn octal_escapes_take_up_to_three_digits() {
        assert_eq!(decoded(b"(\\101)").0, b"A");
        assert_eq!(decoded(b"(\\0053)").0, b"\x053");
        assert_eq!(decoded(b"(\\5x)").0, b"\x05x");
        assert_eq!(decoded(b"(\\53)").0, b"+");
        assert_eq!(
            decoded(b"(\\777)"),
            (b"\xff".to_vec(), [How::OctalOverflow].to_vec())
        );
    }

    #[test]
    fn an_unescaped_end_of_line_is_lf() {
        assert_eq!(
            decoded(b"(a\r\nb\rc\nd)"),
            (b"a\nb\nc\nd".to_vec(), [How::EolAsLf; 2].to_vec())
        );
    }

    #[test]
    fn a_backslash_before_an_end_of_line_is_nothing() {
        for string in [&b"(a\\\r\nb)"[..], b"(a\\\rb)", b"(a\\\nb)"] {
            assert_eq!(decoded(string), (b"ab".to_vec(), Vec::new()));
        }
        // Only one end of line goes with the backslash.
        assert_eq!(decoded(b"(a\\\n\nb)").0, b"a\nb");
    }

    #[test]
    fn a_backslash_before_anything_else_is_ignored() {
        assert_eq!(
            decoded(b"(\\q\\8)"),
            (b"q8".to_vec(), [How::IgnoredBackslash; 2].to_vec())
        );
    }

    #[test]
    fn balanced_parentheses_are_part_of_the_string() {
        assert_eq!(decoded(b"(a (b) c)").0, b"a (b) c");
    }

    #[test]
    fn an_unclosed_string_decodes_what_there_is() {
        assert_eq!(decoded(b"(abc"), (b"abc".to_vec(), Vec::new()));
        assert_eq!(
            decoded(b"(abc\\"),
            (b"abc".to_vec(), [How::Dangling].to_vec())
        );
    }

    #[test]
    fn hex_digits_pair_across_white_space() {
        assert_eq!(decoded(b"<41 42\n43>"), (b"ABC".to_vec(), Vec::new()));
        let pieces: Vec<_> = token(b"<4 1>").pieces().unwrap().collect();
        assert_eq!(pieces[1], piece(1..4, Some(b'A'), How::Escape));
        assert_eq!(decoded(b"<6a6B>").0, b"jk");
    }

    #[test]
    fn an_odd_last_digit_is_followed_by_zero() {
        assert_eq!(
            decoded(b"<414>"),
            (b"A@".to_vec(), [How::OddDigit].to_vec())
        );
        assert_eq!(
            decoded(b"<41 4 >"),
            (b"A@".to_vec(), [How::OddDigit].to_vec())
        );
        assert_eq!(decoded(b"<4"), (b"@".to_vec(), [How::OddDigit].to_vec()));
        assert_eq!(decoded(b"<>"), (Vec::new(), Vec::new()));
    }

    #[test]
    fn bytes_no_token_holds_are_invalid() {
        let hows = |pieces: Pieces<'_>| pieces.map(|p| p.how).collect::<Vec<_>>();
        assert!(hows(decode::hex(b"<4g>")).contains(&How::Invalid));
        assert_eq!(hows(decode::literal(b"(a)b")).last(), Some(&How::Invalid));
        assert_eq!(hows(decode::name(b"/a b"))[2], How::Invalid);
        assert!(decode::hex(b"<4g>").quirks().contains(How::Invalid));
    }

    /// What qpdf 12.3.2 read the same bytes as, through its JSON (2026-10-24).
    #[test]
    fn decoding_agrees_with_qpdf_but_where_qpdf_departs_from_the_spec() {
        let agree: &[(&[u8], &[u8])] = &[
            (b"/A#20B", b"A B"),
            (b"/#41", b"A"),
            (b"(a\\nb\\tc)", b"a\nb\tc"),
            (b"(l1\r\nl2)", b"l1\nl2"),
            (b"(l1\rl2)", b"l1\nl2"),
            (b"(\\q)", b"q"),
            (b"(\\777)", b"\xff"),
            (b"(\\0053)", b"\x053"),
            (b"(\\5x)", b"\x05x"),
            (b"(a\\\r\nb)", b"ab"),
            (b"(a\\\rb)", b"ab"),
            (b"(\\8)", b"8"),
            (b"(a\\\nb)", b"ab"),
            (b"<414>", b"A@"),
            (b"<41 42\n43>", b"ABC"),
            (b"<>", b""),
        ];
        for (input, qpdf) in agree {
            assert_eq!(decoded(input).0, *qpdf, "{input:?}");
        }
        // qpdf reads a stray `#` as NUL, and drops a name holding `#00`.
        let departs: &[(&[u8], &[u8], &[u8])] = &[
            (b"/A#", b"A#", b"A\0"),
            (b"/A#G1", b"A#G1", b"A\0G1"),
            (b"/A#2", b"A#2", b"A\x002"),
            (b"/A#00B", b"A\0B", b"(no value)"),
        ];
        for (input, ours, qpdf) in departs {
            assert_eq!(decoded(input).0, *ours, "{input:?} (qpdf: {qpdf:?})");
        }
    }

    #[test]
    fn integers_are_i64s() {
        for (input, meant) in [
            (&b"0"[..], 0),
            (b"+17", 17),
            (b"-98", -98),
            (b"0042", 42),
            (b"-2147483649", -2_147_483_649),
            (b"9223372036854775807", i64::MAX),
            (b"-9223372036854775808", i64::MIN),
        ] {
            assert_eq!(token(input).integer(), Ok(meant), "{input:?}");
        }
        // qpdf reads the whole object holding one of these as null.
        assert_eq!(
            token(b"9223372036854775808").integer(),
            Err(NumberError::Overflow)
        );
        assert_eq!(
            token(b"99999999999999999999").integer(),
            Err(NumberError::Overflow)
        );
        assert_eq!(token(b"4.0").integer(), Err(NumberError::NotANumber));
        assert_eq!(decode::integer(b"x"), Err(NumberError::NotANumber));
    }

    #[test]
    fn numbers_are_f64s() {
        for (input, meant) in [
            (&b"34.5"[..], 34.5),
            (b"-3.62", -3.62),
            (b"+123.6", 123.6),
            (b"4.", 4.0),
            (b"-.002", -0.002),
            (b".5", 0.5),
            (b"+.5", 0.5),
            (b"17", 17.0),
            (b"99999999999999999999", 1e20),
        ] {
            assert_eq!(token(input).number(), Ok(meant), "{input:?}");
        }
        // Forms Rust reads that PDF doesn't write.
        for word in [&b"1e5"[..], b"inf", b"NaN", b"infinity"] {
            assert_eq!(
                decode::number(word),
                Err(NumberError::NotANumber),
                "{word:?}"
            );
        }
        let mut huge = [b'9'; 400];
        huge[399] = b'.';
        assert_eq!(token(&huge).number(), Err(NumberError::Overflow));
        assert_eq!(token(b"/Name").number(), Err(NumberError::NotANumber));
    }

    /// Each error says what the bytes aren't, word for word.
    #[test]
    fn each_number_error_says_what_it_is() {
        use std::string::ToString;
        assert_eq!(NumberError::NotANumber.to_string(), "not a number");
        assert_eq!(
            NumberError::Overflow.to_string(),
            "a number too large to hold"
        );
    }

    #[test]
    fn only_names_and_strings_have_pieces() {
        assert!(token(b"obj").pieces().is_none());
        assert!(token(b"17").pieces().is_none());
        assert!(token(b"(x)").pieces().is_some());
    }

    #[test]
    fn quirks_are_a_set() {
        let quirks = token(b"(\\q\r)").pieces().unwrap().quirks();
        assert!(quirks.contains(How::IgnoredBackslash) && quirks.contains(How::EolAsLf));
        assert!(!quirks.contains(How::Escape) && !quirks.contains(How::Plain));
        assert_eq!(std::format!("{quirks:?}"), "{IgnoredBackslash, EolAsLf}");
        assert!(token(b"(plain)").pieces().unwrap().quirks().is_empty());
    }

    #[test]
    fn verbatim_is_the_value_as_written_when_theres_nothing_to_decode() {
        let verbatim = |input: &'static [u8], meant: &[u8]| {
            let token = token(input);
            assert_eq!(token.verbatim(), Some(meant), "{input:?} is as written");
            let inside = token.bytes.as_ptr_range();
            assert!(meant.is_empty() || inside.contains(&token.verbatim().unwrap().as_ptr()));
        };
        let decoded = |input: &'static [u8], meant: &[u8]| {
            let token = token(input);
            assert_eq!(token.verbatim(), None, "{input:?} is decoded");
            assert!(token.pieces().unwrap().bytes().eq(meant.iter().copied()));
        };
        verbatim(b"/Type", b"Type");
        verbatim(b"/", b"");
        verbatim(b"(a (b)\nc)", b"a (b)\nc");
        verbatim(b"(unclosed", b"unclosed");
        verbatim(b"<>", b"");
        decoded(b"/A#20B", b"A B");
        decoded(b"(a\rb)", b"a\nb");
        decoded(b"(\\))", b")");
        decoded(b"<41>", b"A");
        assert_eq!(token(b"17").verbatim(), None);
    }
}
