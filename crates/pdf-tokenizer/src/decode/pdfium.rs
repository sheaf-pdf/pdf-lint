//! What a token stands for as PDFium reads it, where that's otherwise than
//! ISO 32000-2: a number word's value, a name's bytes, a literal string's
//! and a hex string's.
//!
//! These readings go with tokens read by
//! [`Rules::Pdfium`](crate::Rules::Pdfium), and give values only: unlike
//! [`Pieces`](super::Pieces), they don't say how each byte was written,
//! since the [`How`](super::How)s are ISO 32000-2's. Where PDFium and ISO
//! 32000-2 read the same bytes alike, so do these and the pieces.
//!
//! Each reads all the bytes it's given. PDFium's parsers keep only the
//! first bytes of a long word or string (its content parser 255 of a word
//! and 32,767 of a string, its object parser 256 of a word), and a reader
//! that follows them cuts the bytes before it reads them. None of these
//! fails, panics or allocates.
//!
//! ```
//! use pdf_tokenizer::decode::pdfium::{self, Number};
//!
//! assert_eq!(pdfium::number(b"--5"), Number::Signed(0));
//! assert_eq!(pdfium::number(b"1.2.3"), Number::Real(1.2));
//! assert!(pdfium::name(b"/F#3G").eq(*b"F0"));
//! assert!(pdfium::literal(b"(a\rb)").eq(*b"a\rb"));
//! assert!(pdfium::hex(b"<4G1>").eq(*b"A"));
//! ```

use core::iter::FusedIterator;

/// A number as PDFium holds one (its `FX_Number`), from a number word by
/// [`number`]: one of three, by how the word is written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    /// A word without a point that doesn't start with a sign: the digits
    /// it starts with, as an unsigned 32-bit integer, or 0 when they're
    /// past `u32` or there are none.
    Unsigned(u32),
    /// A word without a point that starts with `+` or `-`: the digits
    /// after it, negated after a minus, or 0 when they're past `i32`'s
    /// range or there are none.
    Signed(i32),
    /// A word with a point: the decimal it starts with, past the signs
    /// before it but for the last of them when it's a minus, as an `f32`
    /// rounded to the nearest. Infinite past `f32`'s range, and 0 when
    /// there's no digit.
    Real(f32),
}

impl Number {
    /// The number as PDFium reads a number, an `f32` (`GetNumber`): an
    /// integer rounded to the nearest one.
    pub fn float(self) -> f32 {
        match self {
            Number::Unsigned(n) => n as f32,
            Number::Signed(n) => n as f32,
            Number::Real(n) => n,
        }
    }

    /// The number as PDFium reads an integer (`GetInteger`): an unsigned
    /// one past `i32::MAX` wraps to a negative one, and a real is cut to
    /// its whole part, saturating at `i32`'s ends.
    pub fn integer(self) -> i32 {
        match self {
            Number::Unsigned(n) => n as i32,
            Number::Signed(n) => n,
            Number::Real(n) => n as i32,
        }
    }
}

/// The number a number word is, as PDFium reads it (`FX_Number`): a
/// word of digits, signs and points, an [`Integer`](crate::Kind::Integer)
/// or a [`Real`](crate::Kind::Real) by PDFium's rules. Other bytes are
/// read without failing, as far as they read as one.
///
/// ISO 32000-2 (7.3.3) has no reading of `--5` or `1.2.3`, and holds
/// integers past 32 bits; PDFium reads `--5` as 0, `1.2.3` as 1.2 and
/// `4294967296` as 0.
pub fn number(bytes: &[u8]) -> Number {
    if bytes.contains(&b'.') {
        return Number::Real(decimal(bytes).parse().unwrap_or(0.0));
    }
    let (sign, digits) = match bytes {
        [b'-', rest @ ..] => (Some(true), rest),
        [b'+', rest @ ..] => (Some(false), rest),
        rest => (None, rest),
    };
    let mut value = Some(0u32);
    for &d in digits.iter().take_while(|d| d.is_ascii_digit()) {
        value = value.and_then(|v| v.checked_mul(10)?.checked_add(u32::from(d - b'0')));
    }
    let value = value.unwrap_or(0);
    match sign {
        None => Number::Unsigned(value),
        Some(false) => Number::Signed(i32::try_from(value).unwrap_or(0)),
        Some(true) if value == i32::MIN.unsigned_abs() => Number::Signed(i32::MIN),
        Some(true) => Number::Signed(i32::try_from(value).map_or(0, |v| -v)),
    }
}

/// The decimal `bytes` start with, read as [`Number::Real`] reads it but
/// as an `f64` (PDFium's `StringToDouble`): for a reader that holds a real
/// at 64 bits. 0 when there's no digit, and infinite past `f64`'s range.
pub fn real(bytes: &[u8]) -> f64 {
    decimal(bytes).parse().unwrap_or(0.0)
}

/// The decimal `bytes` start with, as text: past the spaces and signs
/// before it but for the last of them when it's a minus, then digits, a
/// point and digits, each part optional. Rust parses it as PDFium does,
/// rounding to the nearest; one without a digit doesn't parse.
fn decimal(bytes: &[u8]) -> &str {
    let lead = bytes
        .iter()
        .take_while(|&&b| matches!(b, b' ' | b'+' | b'-'))
        .count();
    let from = lead - usize::from(lead > 0 && bytes[lead - 1] == b'-');
    let rest = &bytes[from..];
    let digits = |at: usize| {
        let after = rest.get(at..).unwrap_or_default();
        at + after.iter().take_while(|b| b.is_ascii_digit()).count()
    };
    let mut end = digits(usize::from(rest.first() == Some(&b'-')));
    if rest.get(end) == Some(&b'.') {
        end = digits(end + 1);
    }
    // Only a minus, digits and a point.
    core::str::from_utf8(&rest[..end]).unwrap_or_default()
}

/// The bytes of a name's or a string's value as PDFium reads them, in
/// order: from [`name`], [`literal`] or [`hex`].
#[derive(Debug, Clone)]
pub struct Bytes<'a> {
    bytes: &'a [u8],
    pos: usize,
    syntax: Syntax,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Syntax {
    Name,
    /// A literal string, and the parentheses open inside it.
    Literal(usize),
    Hex,
    /// A string's close has come.
    Done,
}

/// A name's bytes, `/` included, as PDFium reads them (`PDF_NameDecode`):
/// a `#` with two bytes after it stands for the byte they make as hex
/// digits, a byte that isn't one read as 0, and every other byte for
/// itself. ISO 32000-2 (7.3.5) reads a `#` as an escape only before two
/// hex digits, and [`super::name`]'s pieces say what it does otherwise.
pub fn name(bytes: &[u8]) -> Bytes<'_> {
    let pos = usize::from(bytes.first() == Some(&b'/'));
    Bytes {
        bytes,
        pos,
        syntax: Syntax::Name,
    }
}

/// A literal string's bytes, parentheses included, as PDFium reads them:
/// an unescaped CR, or CR LF, as written, where ISO 32000-2 (7.3.4.2)
/// reads LF; and an octal escape of one or two digits that the bytes end
/// on, in a string that doesn't close, as nothing. Every other escape, and
/// every other byte, as ISO 32000-2 reads it.
pub fn literal(bytes: &[u8]) -> Bytes<'_> {
    let pos = usize::from(bytes.first() == Some(&b'('));
    Bytes {
        bytes,
        pos,
        syntax: Syntax::Literal(0),
    }
}

/// A hexadecimal string's bytes, `<` and `>` included, as PDFium reads
/// them: its hex digits in pairs, up to its `>`, every other byte passed
/// over, and an odd last digit as if `0` followed it. ISO 32000-2
/// (7.3.4.3) allows only white-space between the digits.
pub fn hex(bytes: &[u8]) -> Bytes<'_> {
    let pos = usize::from(bytes.first() == Some(&b'<'));
    Bytes {
        bytes,
        pos,
        syntax: Syntax::Hex,
    }
}

impl Bytes<'_> {
    fn take(&mut self) -> Option<u8> {
        let b = *self.bytes.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn name(&mut self) -> Option<u8> {
        let b = self.take()?;
        match self.bytes.get(self.pos..self.pos + 2) {
            Some(&[high, low]) if b == b'#' => {
                self.pos += 2;
                Some((digit(high) << 4) | digit(low))
            }
            _ => Some(b),
        }
    }

    fn literal(&mut self, depth: usize) -> Option<u8> {
        loop {
            let b = self.take()?;
            let (depth, byte) = match b {
                b')' if depth == 0 => {
                    self.syntax = Syntax::Done;
                    return None;
                }
                b')' => (depth - 1, b')'),
                b'(' => (depth + 1, b'('),
                b'\\' => match self.escape() {
                    Escape::Byte(byte) => (depth, byte),
                    Escape::Nothing => continue,
                    Escape::End => return None,
                },
                b => (depth, b),
            };
            self.syntax = Syntax::Literal(depth);
            return Some(byte);
        }
    }

    /// The escape after a literal string's backslash.
    fn escape(&mut self) -> Escape {
        let Some(b) = self.take() else {
            return Escape::End;
        };
        Escape::Byte(match b {
            b'0'..=b'7' => {
                let mut code = u16::from(b - b'0');
                for _ in 0..2 {
                    match self.bytes.get(self.pos) {
                        Some(&d @ b'0'..=b'7') => {
                            code = code * 8 + u16::from(d - b'0');
                            self.pos += 1;
                        }
                        Some(_) => break,
                        // PDFium drops an escape the input ends inside.
                        None => return Escape::End,
                    }
                }
                code as u8
            }
            b'\r' => {
                if self.bytes.get(self.pos) == Some(&b'\n') {
                    self.pos += 1;
                }
                return Escape::Nothing;
            }
            b'\n' => return Escape::Nothing,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'b' => 0x08,
            b'f' => 0x0C,
            b => b,
        })
    }

    fn hex(&mut self) -> Option<u8> {
        // A digit read, waiting for its pair.
        let mut high = None;
        loop {
            let Some(b) = self.take().filter(|&b| b != b'>') else {
                self.syntax = Syntax::Done;
                return high;
            };
            if !b.is_ascii_hexdigit() {
                continue;
            }
            match high {
                Some(high) => return Some(high | digit(b)),
                None => high = Some(digit(b) << 4),
            }
        }
    }
}

/// What a literal string's escape stands for.
enum Escape {
    Byte(u8),
    /// A line continuation.
    Nothing,
    /// The bytes end inside it.
    End,
}

impl Iterator for Bytes<'_> {
    type Item = u8;

    fn next(&mut self) -> Option<u8> {
        match self.syntax {
            Syntax::Name => self.name(),
            Syntax::Literal(depth) => self.literal(depth),
            Syntax::Hex => self.hex(),
            Syntax::Done => None,
        }
    }
}

impl FusedIterator for Bytes<'_> {}

/// A hex digit's value, and 0 for a byte that isn't one, as PDFium reads it.
fn digit(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::vec::Vec;

    use super::{Number, hex, literal, name, number, real};
    use crate::{Rules, Tokenizer, decode, fuzz};

    /// The one token `input` is by PDFium's rules, checked as the fuzz
    /// target checks it.
    fn token(input: &[u8]) -> &[u8] {
        let mut tokens = Tokenizer::new(input).with_rules(Rules::Pdfium);
        let token = tokens.next().expect("a token");
        assert_eq!(tokens.next(), None, "{input:?} is one token");
        fuzz::walk(input, 0);
        token.bytes
    }

    /// Operands as PDFium reads them, an `f32` each, what isn't a number
    /// 0, and past an `f32` infinite.
    #[test]
    fn numbers_are_read_as_pdfium_reads_operands() {
        for (word, n) in [
            (&b"1.2.3"[..], 1.2),
            (b"--5.0", -5.0),
            (b"+-.5", -0.5),
            (b"5.", 5.0),
            (b".", 0.0),
            (b"-", 0.0),
            (b"007", 7.0),
            (b"4294967295", 4_294_967_295.0),
            (b"4294967296", 0.0),
            (b"+2147483648", 0.0),
            (b"-2147483648", -2_147_483_648.0),
            (b"5-3", 5.0),
            (b"--5", 0.0),
            (b"/Name", 0.0),
            (b"(5)", 0.0),
        ] {
            assert_eq!(number(word).float(), n, "{word:?}");
        }
        assert!(
            number(&[[b'9'; 40].as_slice(), b".0"].concat())
                .float()
                .is_infinite()
        );
    }

    /// What PDFium 7881 and 8076 read each word as, in `cm`, `re` and a
    /// `/MediaBox` alike (one-page files, 2026-10-30), and how it holds it.
    #[test]
    fn numbers_are_held_as_pdfium_holds_them() {
        let max = f32::MAX;
        for (word, held) in [
            (&b"1.2.3"[..], Number::Real(1.2)),
            (b"--5.0", Number::Real(-5.0)),
            (b"+-.5", Number::Real(-0.5)),
            (b"-+5.", Number::Real(5.0)),
            (b".", Number::Real(0.0)),
            (b"+.", Number::Real(0.0)),
            (b"-.", Number::Real(0.0)),
            (b".5.", Number::Real(0.5)),
            (b"1..2", Number::Real(1.0)),
            (b"0.1", Number::Real(0.1)),
            (b"16777217.0", Number::Real(16_777_216.0)),
            // Halfway between two `f32`s, to the even one.
            (b"2.00000011920928955078125", Number::Real(2.0)),
            (b"2.000000119209289550781251", Number::Real(2.000_000_2)),
            (
                b"340282346638528859811704183484516925440.0",
                Number::Real(max),
            ),
            (
                b"340282356779733661637539395458142568447.9",
                Number::Real(max),
            ),
            (
                b"340282356779733661637539395458142568448.0",
                Number::Real(f32::INFINITY),
            ),
            (
                b"-340282356779733661637539395458142568448.0",
                Number::Real(f32::NEG_INFINITY),
            ),
            (
                b"0.000000000000000000000000000000000000000000001",
                Number::Real(1e-45),
            ),
            (b"-", Number::Signed(0)),
            (b"+", Number::Signed(0)),
            (b"--5", Number::Signed(0)),
            (b"-0", Number::Signed(0)),
            (b"+5", Number::Signed(5)),
            (b"+16777217", Number::Signed(16_777_217)),
            (b"-2147483648", Number::Signed(i32::MIN)),
            (b"-2147483649", Number::Signed(0)),
            (b"+2147483647", Number::Signed(i32::MAX)),
            (b"+2147483648", Number::Signed(0)),
            (b"007", Number::Unsigned(7)),
            (b"5-3", Number::Unsigned(5)),
            (b"2147483648", Number::Unsigned(2_147_483_648)),
            (b"4294967295", Number::Unsigned(u32::MAX)),
            (b"4294967296", Number::Unsigned(0)),
            (&[b'9'; 50], Number::Unsigned(0)),
        ] {
            assert_eq!(number(token(word)), held, "{word:?}");
        }
        assert_eq!(number(b""), Number::Unsigned(0));
        assert_eq!(number(b"16777217").float(), 16_777_216.0);
    }

    /// As PDFium reads an integer: an unsigned one past `i32` wraps, and a
    /// real is cut to its whole part, saturating.
    #[test]
    fn an_integer_is_read_as_pdfium_reads_one() {
        for (held, int) in [
            (Number::Unsigned(u32::MAX), -1),
            (Number::Unsigned(2_147_483_648), i32::MIN),
            (Number::Unsigned(7), 7),
            (Number::Signed(-5), -5),
            (Number::Real(1.9), 1),
            (Number::Real(-1.9), -1),
            (Number::Real(3e9), i32::MAX),
            (Number::Real(f32::NEG_INFINITY), i32::MIN),
        ] {
            assert_eq!(held.integer(), int, "{held:?}");
        }
    }

    /// At 64 bits, a reader's own width: the same decimal, read as an `f64`.
    #[test]
    fn a_real_reads_at_64_bits_too() {
        for (word, value) in [
            (&b"1.2.3"[..], 1.2),
            (b"--5.0", -5.0),
            (b"+-.5", -0.5),
            (b"0.1", 0.1),
            (b".", 0.0),
            (
                b"340282356779733661637539395458142568448.0",
                3.402_823_567_797_336_6e38,
            ),
        ] {
            assert_eq!(real(token(word)), value, "{word:?}");
        }
        let huge = [[b'9'; 400].as_slice(), b"."].concat();
        assert!(real(&huge).is_infinite());
    }

    fn value(bytes: impl Iterator<Item = u8>) -> Vec<u8> {
        bytes.collect()
    }

    /// A `#` with two bytes after it is an escape, a byte that's no hex
    /// digit 0: as PDFium 7881 and 8076 read a font's name in `Tf` and an
    /// `/Info` key (2026-10-30). ISO 32000-2 keeps such a `#`.
    #[test]
    fn a_names_hash_takes_any_two_bytes() {
        for (input, meant) in [
            (&b"/F#30"[..], &b"F0"[..]),
            (b"/F#3G", b"F0"),
            (b"/F#3g", b"F0"),
            (b"/F#G0", b"F\0"),
            (b"/#460", b"F0"),
            (b"/F#300", b"F00"),
            (b"/#F#30", b"\xf030"),
            (b"/##41", b"\x041"),
            (b"/#5Groducer", b"Producer"),
            (b"/Produce#72", b"Producer"),
            (b"/P#7z", b"Pp"),
            (b"/P#z7", b"P\x07"),
            (b"/A#20B", b"A B"),
            (b"/", b""),
        ] {
            assert_eq!(value(name(token(input))), meant, "{input:?}");
        }
        // With fewer than two bytes after it, a `#` is itself.
        for (input, meant) in [
            (&b"/F#3"[..], &b"F#3"[..]),
            (b"/F0#", b"F0#"),
            (b"/Producer#7", b"Producer#7"),
        ] {
            assert_eq!(value(name(token(input))), meant, "{input:?}");
        }
        assert_eq!(value(decode::name(b"/F#3G").bytes()), b"F#3G");
    }

    /// An unescaped CR or CR LF is kept as written, in `Tj` and in
    /// `/Title` (PDFium 7881 and 8076, 2026-10-30), where ISO 32000-2 reads
    /// LF; every escape is as ISO 32000-2's.
    #[test]
    fn a_literal_strings_end_of_line_is_as_written() {
        for (input, meant) in [
            (&b"(a\rb\r\nc)"[..], &b"a\rb\r\nc"[..]),
            (b"(a\r\rb)", b"a\r\rb"),
            (b"(a\n\rb)", b"a\n\rb"),
            (b"(a\\\rb)", b"ab"),
            (b"(a\\\r\nb)", b"ab"),
            (b"(a\\\nb)", b"ab"),
            (b"(a\\\n\nb)", b"a\nb"),
            (b"(a\\015b)", b"a\rb"),
        ] {
            assert_eq!(value(literal(token(input))), meant, "{input:?}");
        }
        assert_eq!(value(decode::literal(b"(a\rb)").bytes()), b"a\nb");
    }

    #[test]
    fn a_literal_strings_escapes_are_as_iso_32000s() {
        for (input, meant) in [
            (
                &b"(\\n\\r\\t\\b\\f\\(\\)\\\\)"[..],
                &b"\n\r\t\x08\x0c()\\"[..],
            ),
            (b"(\\101)", b"A"),
            (b"(\\0053)", b"\x053"),
            (b"(\\5x)", b"\x05x"),
            (b"(\\53)", b"+"),
            (b"(\\5)", b"\x05"),
            (b"(\\777)", b"\xff"),
            (b"(\\q\\8)", b"q8"),
            (b"(a (b) c)", b"a (b) c"),
            (b"()", b""),
        ] {
            assert_eq!(value(literal(token(input))), meant, "{input:?}");
            assert_eq!(value(decode::literal(input).bytes()), meant, "{input:?}");
        }
    }

    /// A string the bytes end inside: what there is, but an escape it
    /// ends inside, which PDFium drops, an octal one of one or two digits
    /// among them.
    #[test]
    fn an_unclosed_string_reads_what_there_is() {
        for (input, meant) in [
            (&b"(abc"[..], &b"abc"[..]),
            (b"(abc\\", b"abc"),
            (b"(a\\5", b"a"),
            (b"(a\\53", b"a"),
            (b"(a\\531", b"aY"),
            (b"(a\\\r", b"a"),
            (b"(a (b)", b"a (b)"),
        ] {
            assert_eq!(value(literal(token(input))), meant, "{input:?}");
        }
        // Nothing after its close.
        assert_eq!(value(literal(b"(a)b")), b"a");
    }

    /// Hex digits pair up to the `>`, over any other byte, in `Tj` and in
    /// `/Title` (PDFium 7881 and 8076, 2026-10-30).
    #[test]
    fn a_hex_strings_digits_pair_over_any_byte() {
        for (input, meant) in [
            (&b"<41 42>"[..], &b"AB"[..]),
            (b"<41G42>", b"AB"),
            (b"<4G1>", b"A"),
            (b"<4G>", b"@"),
            (b"<41(42>", b"AB"),
            (b"<41/42>", b"AB"),
            (b"<41%42>", b"AB"),
            (b"<41]42>", b"AB"),
            (b"<41\x8042>", b"AB"),
            (b"<41}42>", b"AB"),
            (b"<41{42>", b"AB"),
            (b"<4 1>", b"A"),
            (b"<41 4>", b"A@"),
            (b"<414>", b"A@"),
            (b"<6a6B>", b"jk"),
            (b"<>", b""),
            (b"<41", b"A"),
            (b"<4", b"@"),
        ] {
            assert_eq!(value(hex(token(input))), meant, "{input:?}");
        }
        assert_eq!(value(hex(b"<41>42")), b"A");
    }

    /// The readings take the bytes they're given, whole, and a value is
    /// never longer than its bytes.
    #[test]
    fn the_readings_take_the_bytes_given() {
        let long = [b"/".as_slice(), &[b'F'; 300]].concat();
        assert_eq!(name(&long).count(), 300);
        let string = [b"(".as_slice(), &[b'a'; 40_000], b")"].concat();
        assert_eq!(literal(&string).count(), 40_000);
        for bytes in [&b"name"[..], b"#41", b"a#"] {
            assert!(value(name(bytes)).len() <= bytes.len());
        }
        assert_eq!(value(name(b"#41")), b"A", "no `/`");
        assert_eq!(value(literal(b"a)b")), b"a", "no `(`");
        assert_eq!(value(hex(b"41>")), b"A", "no `<`");
    }
}
