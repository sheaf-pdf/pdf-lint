//! What a token stands for: a name's or a string's bytes, or a number, as
//! ISO 32000-2 reads them; and in [`pdfium`], as PDFium does.
//!
//! Decoding hides nothing. Every byte of a name or string is in exactly one
//! [`Piece`], which says what value byte it stands for and how, so each
//! escape, and each place where readers or PDF versions differ, is there to
//! see, with where it was written. A decoder still gives a byte at those
//! places, and [`How`] says which it chose.

use core::fmt;
use core::iter::FusedIterator;
use core::ops::{BitOr, BitOrAssign, Range};

use memchr::memchr2;

pub mod pdfium;

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
///
/// A quirk is found in files, not designed, so one may come in a minor
/// release: a match on `How` outside this crate needs a wildcard.
#[non_exhaustive]
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
    /// string, or anything after a string's close: no byte. A token read by
    /// ISO 32000-2's rules never has one; a hex string read by PDFium's may,
    /// a byte in it that's neither a hex digit nor white-space.
    Invalid,
}

impl How {
    /// Every `How`, in order: the ordinary ones first, the quirks after.
    pub const ALL: &[How] = &[
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

/// A set of quirks: the [`How`]s out of the ordinary among some pieces
/// ([`Pieces::quirks`]), or built, by [`insert`](Self::insert), from an
/// iterator of `How`s (`collect`, `extend`), or from two sets (`|`, `|=`).
/// A `How` that isn't a quirk ([`How::is_quirk`]) is never in one: putting
/// it in does nothing. [`iter`](Self::iter), or `for how in quirks`, gives
/// the quirks in [`How::ALL`]'s order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Quirks(u16);

// A bit a `How`, by its discriminant; `How::ALL` lists them in that order.
const _: () = assert!(How::ALL.len() <= u16::BITS as usize);

impl Quirks {
    /// Whether `how` is among them.
    pub const fn contains(self, how: How) -> bool {
        self.0 & (1 << how as u16) != 0
    }

    /// Whether there are none.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// How many there are.
    pub const fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// Puts `how` among them, if it's a quirk; a `How` that isn't one
    /// ([`How::is_quirk`]) is ignored.
    pub const fn insert(&mut self, how: How) {
        if how.is_quirk() {
            self.0 |= 1 << how as u16;
        }
    }

    /// The quirks, in [`How::ALL`]'s order.
    pub fn iter(self) -> QuirksIter {
        QuirksIter(self.0)
    }
}

impl fmt::Debug for Quirks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl BitOr for Quirks {
    type Output = Quirks;

    /// The quirks in either.
    fn bitor(self, other: Quirks) -> Quirks {
        Quirks(self.0 | other.0)
    }
}

impl BitOrAssign for Quirks {
    /// Puts `other`'s quirks among them.
    fn bitor_assign(&mut self, other: Quirks) {
        self.0 |= other.0;
    }
}

impl Extend<How> for Quirks {
    /// Puts each quirk among them; a `How` that isn't one is ignored.
    fn extend<I: IntoIterator<Item = How>>(&mut self, hows: I) {
        for how in hows {
            self.insert(how);
        }
    }
}

impl FromIterator<How> for Quirks {
    /// The quirks among `hows`; a `How` that isn't one is ignored.
    fn from_iter<I: IntoIterator<Item = How>>(hows: I) -> Quirks {
        let mut quirks = Quirks::default();
        quirks.extend(hows);
        quirks
    }
}

impl IntoIterator for Quirks {
    type Item = How;
    type IntoIter = QuirksIter;

    /// The quirks, in [`How::ALL`]'s order.
    fn into_iter(self) -> QuirksIter {
        self.iter()
    }
}

/// The quirks of a [`Quirks`] set, in [`How::ALL`]'s order: from
/// [`Quirks::iter`], or `for how in quirks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuirksIter(u16);

impl Iterator for QuirksIter {
    type Item = How;

    fn next(&mut self) -> Option<How> {
        if self.0 == 0 {
            return None;
        }
        let bit = self.0.trailing_zeros() as usize;
        self.0 &= self.0 - 1;
        // A bit is set only by `insert`, from a `How`, whose discriminant
        // indexes `How::ALL`.
        Some(How::ALL[bit])
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.0.count_ones() as usize;
        (left, Some(left))
    }
}

impl ExactSizeIterator for QuirksIter {}

impl FusedIterator for QuirksIter {}

/// The [`Piece`]s of a name, a literal string or a hex string, in order.
/// They join into the bytes given, with no gap and no overlap. Each is a
/// byte or more and stands for one byte at most, so there are never more
/// pieces, or value bytes, than bytes: `size_hint` gives a piece at least
/// while bytes are left, and at most as many as the bytes left.
#[derive(Debug, Clone)]
pub struct Pieces<'a> {
    bytes: &'a [u8],
    base: usize,
    pos: usize,
    syntax: Syntax,
    /// A literal string's parentheses still open, its first included: one
    /// from the start when the bytes don't begin with it, the inside of a
    /// string already open.
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
/// Without it, they're the inside of a name: `name(b"abc")` is three plain
/// bytes, and a `/` among them is invalid, as it is in any name's body.
pub fn name(bytes: &[u8]) -> Pieces<'_> {
    Pieces::new(bytes, 0, Syntax::Name)
}

/// The pieces of a literal string's bytes, parentheses included (ISO
/// 32000-2, 7.3.4.2). Without the opening one, they're the inside of a
/// string already open: a `(` among them opens a nested pair as it would
/// inside any string, a `)` that balances no `(` closes the string, and
/// anything after that is invalid. `literal(b"a(b)c")` is five plain
/// bytes; `literal(b"a)b")` is `a`, a closing delimiter and an invalid
/// `b`.
pub fn literal(bytes: &[u8]) -> Pieces<'_> {
    Pieces::new(bytes, 0, Syntax::Literal)
}

/// The pieces of a hexadecimal string's bytes, `<` and `>` included (ISO
/// 32000-2, 7.3.4.3). Without the `<`, they're the inside of a string
/// already open: `hex(b"4142")` is two escapes, `A` and `B`, and
/// `hex(b">")` a closing delimiter.
pub fn hex(bytes: &[u8]) -> Pieces<'_> {
    Pieces::new(bytes, 0, Syntax::Hex)
}

impl<'a> Pieces<'a> {
    fn new(bytes: &'a [u8], base: usize, syntax: Syntax) -> Self {
        // Bytes without their opening parenthesis are the inside of a
        // string that's open already.
        let depth = match syntax {
            Syntax::Literal if bytes.first() != Some(&b'(') => 1,
            _ => 0,
        };
        Pieces {
            bytes,
            base,
            pos: 0,
            syntax,
            depth,
            closed: false,
        }
    }

    /// The value's bytes, from the pieces left: never more than the bytes
    /// left, which the iterator's `size_hint` gives as its upper bound, so
    /// a buffer of that length holds them without growing. Cloning it is
    /// cheap, as cloning the pieces is.
    pub fn bytes(self) -> impl FusedIterator<Item = u8> + Clone + 'a {
        self.filter_map(|piece| piece.byte)
    }

    /// The bytes from the next piece on.
    fn rest(&self) -> &'a [u8] {
        self.bytes.get(self.pos..).unwrap_or_default()
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
        let (&first, after) = self.rest().split_first()?;
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

    /// A piece at least while bytes are left, each piece being a byte or
    /// more; at most one a byte.
    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.rest().len();
        (usize::from(left > 0), Some(left))
    }
}

// `pos` only grows, and `next` gives nothing once it reaches the end.
impl FusedIterator for Pieces<'_> {}

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

/// A literal string's value as written, when nothing in it needs decoding:
/// what's inside its parentheses (the first, when the bytes begin with it,
/// and the one that balances it) holds no backslash and no CR, and nothing
/// comes after the one that balances it. Bytes without their opening
/// parenthesis are the inside of a string already open, as [`literal`]
/// reads them.
fn literal_verbatim(bytes: &[u8]) -> Option<&[u8]> {
    if memchr2(b'\\', b'\r', bytes).is_some() {
        return None;
    }
    let start = usize::from(bytes.first() == Some(&b'('));
    let mut depth = 1usize;
    let mut at = start;
    while let Some(i) = bytes.get(at..).and_then(|rest| memchr2(b'(', b')', rest)) {
        at += i;
        match bytes[at] {
            b'(' => depth += 1,
            _ if depth == 1 => return (at + 1 == bytes.len()).then(|| &bytes[start..at]),
            _ => depth = depth.saturating_sub(1),
        }
        at += 1;
    }
    bytes.get(start..)
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
///
/// Exhaustive on purpose, so that a match on every error is checked by the
/// compiler: a new variant would be a breaking release.
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
    /// It's written as one, but too small: a real whose digits aren't all
    /// zero, which an `f64` reads as zero (`0.` followed by 400 zeros and a
    /// `1`). An integer can't be.
    #[error("a number too small to hold")]
    Underflow,
}

/// The integer `bytes` are written as: digits, with an optional sign (ISO
/// 32000-2, 7.3.3).
pub fn integer(bytes: &[u8]) -> Result<i64, NumberError> {
    let (negative, digits) = match bytes {
        [b'-', digits @ ..] => (true, digits),
        [b'+', digits @ ..] => (false, digits),
        digits => (false, digits),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err(NumberError::NotANumber);
    }
    // Toward the sign, so that `i64::MIN`, one further from 0 than
    // `i64::MAX`, is read; past either end, an overflow.
    let mut value: i64 = 0;
    for &b in digits {
        let digit = i64::from(b - b'0');
        value = (value.checked_mul(10))
            .and_then(|v| match negative {
                true => v.checked_sub(digit),
                false => v.checked_add(digit),
            })
            .ok_or(NumberError::Overflow)?;
    }
    Ok(value)
}

/// The number `bytes` are written as, an integer or a real, as an `f64`
/// (ISO 32000-2, 7.3.3): too large past a finite `f64`, too small when a
/// real with a digit other than `0` reads as zero. Forms Rust reads but PDF
/// doesn't write, such as `1e5` or `inf`, aren't numbers.
pub fn number(bytes: &[u8]) -> Result<f64, NumberError> {
    if let Some(value) = short_decimal(bytes) {
        return Ok(value);
    }
    if !matches!(word(bytes), Kind::Integer | Kind::Real) {
        return Err(NumberError::NotANumber);
    }
    let text = core::str::from_utf8(bytes).map_err(|_| NumberError::NotANumber)?;
    let value: f64 = text.parse().map_err(|_| NumberError::NotANumber)?;
    if !value.is_finite() {
        Err(NumberError::Overflow)
    } else if value == 0.0 && bytes.iter().any(|b| matches!(b, b'1'..=b'9')) {
        // Zero from a digit that isn't: a decimal below the least `f64`.
        // The short reading above never gives one (its least is 1e-22).
        Err(NumberError::Underflow)
    } else {
        Ok(value)
    }
}

/// The powers of ten an `f64` holds exactly, 10^0 to 10^22.
const TENS: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
];

/// The number `bytes` are written as, where it's short enough to read
/// exactly by one division: digits, at most one point among them and an
/// optional sign, the digits read as an integer no more than 2^53 and at
/// most 22 of them after the point. Both that integer and the power of ten
/// it's divided by are then `f64`s exactly, and the quotient is rounded
/// once, to the nearest, as a decimal's reading is: the `f64` the decimal
/// is read as. None otherwise, for [`number`]'s whole reading; and None
/// always on x86 without SSE2, whose x87 division rounds to a 64-bit
/// significand and then again to an `f64`'s, so that a quotient may be
/// rounded twice, where the whole reading sets the x87's precision for
/// its own division.
fn short_decimal(bytes: &[u8]) -> Option<f64> {
    if cfg!(all(target_arch = "x86", not(target_feature = "sse2"))) {
        return None;
    }
    let (negative, rest) = match bytes {
        [b'-', rest @ ..] => (true, rest),
        [b'+', rest @ ..] => (false, rest),
        rest => (false, rest),
    };
    let (mut digits, mut value, mut point) = (0, 0u64, None);
    for &b in rest {
        match b {
            b'0'..=b'9' if digits < 19 => {
                value = value * 10 + u64::from(b - b'0');
                digits += 1;
            }
            b'.' if point.is_none() => point = Some(digits),
            _ => return None,
        }
    }
    let after = digits - point.unwrap_or(digits);
    if digits == 0 || value > 1 << 53 || after >= TENS.len() {
        return None;
    }
    let value = value as f64 / TENS[after];
    Some(if negative { -value } else { value })
}

impl<'a> Token<'a> {
    /// The pieces of a name, a literal string or a hex string, at their
    /// offsets in the input, as ISO 32000-2 reads them, whatever rules read
    /// the token. `None` for any other kind. [`pdfium`] reads the same
    /// bytes as PDFium does.
    pub fn pieces(&self) -> Option<Pieces<'a>> {
        let syntax = match self.kind {
            Kind::Name => Syntax::Name,
            Kind::LiteralString => Syntax::Literal,
            Kind::HexString => Syntax::Hex,
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
        // The bytes are searched for what the pieces would say isn't plain,
        // not walked piece by piece: the same answer, from any bytes, a
        // token's or not.
        let bytes = self.bytes;
        let value = match self.kind {
            Kind::Name => {
                let body = bytes.strip_prefix(b"/").unwrap_or(bytes);
                body.iter()
                    .all(|&b| is_regular(b) && b != b'#')
                    .then_some(body)?
            }
            Kind::LiteralString => literal_verbatim(bytes)?,
            // No piece of a hex string is plain (each between its
            // delimiters is digits, white-space or a byte it can't hold),
            // so it's verbatim only with nothing but them.
            Kind::HexString => match bytes {
                [] | [b'<' | b'>'] | [b'<', b'>'] => &[],
                _ => return None,
            },
            _ => return None,
        };
        // Empty, it's where the token starts, as the pieces would place it.
        Some(if value.is_empty() {
            bytes.get(..0).unwrap_or_default()
        } else {
            value
        })
    }

    /// The integer an [`Integer`](Kind::Integer) token is written as, read
    /// as ISO 32000-2 writes one: a word only PDFium's rules make an
    /// integer, such as `--5`, isn't one here, and [`pdfium::number`] reads
    /// it.
    pub fn integer(&self) -> Result<i64, NumberError> {
        match self.kind {
            Kind::Integer => integer(self.bytes),
            _ => Err(NumberError::NotANumber),
        }
    }

    /// The number an [`Integer`](Kind::Integer) or [`Real`](Kind::Real)
    /// token is written as, as an `f64`, read as ISO 32000-2 writes one: a
    /// word only PDFium's rules make a number, such as `1.2.3`, isn't one
    /// here, and [`pdfium::number`] reads it.
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

    use std::format;
    use std::vec;
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

    /// A real whose digits aren't all zero but reads as zero is too small
    /// to hold; one whose digits are all zero is zero, with its sign, to
    /// the bit, however many digits it has.
    #[test]
    fn a_real_too_small_to_hold_is_an_underflow() {
        // `0.`, 400 zeros, `1`.
        let mut tiny = vec![b'0'; 403];
        tiny[1] = b'.';
        tiny[402] = b'1';
        assert_eq!(token(&tiny).number(), Err(NumberError::Underflow));
        let signed = |sign: &[u8]| [sign, &tiny[..]].concat();
        assert_eq!(decode::number(&signed(b"-")), Err(NumberError::Underflow));
        assert_eq!(decode::number(&signed(b"+")), Err(NumberError::Underflow));
        // The least `f64`, 5e-324, is held; a digit further isn't.
        let least = format!("0.{}5", "0".repeat(323));
        assert_eq!(decode::number(least.as_bytes()), Ok(5e-324));
        let below = format!("0.{}1", "0".repeat(324));
        assert_eq!(
            decode::number(below.as_bytes()),
            Err(NumberError::Underflow)
        );
        let bits = |n: Result<f64, NumberError>| n.map(f64::to_bits);
        for zero in [&b"0"[..], b"0.0", b"0.000", b".0", b"0."] {
            assert_eq!(bits(decode::number(zero)), Ok(0.0f64.to_bits()), "{zero:?}");
            let negative = [b"-", zero].concat();
            assert_eq!(
                bits(decode::number(&negative)),
                Ok((-0.0f64).to_bits()),
                "{negative:?}"
            );
        }
        let zeros = format!("0.{}", "0".repeat(400));
        assert_eq!(bits(decode::number(zeros.as_bytes())), Ok(0.0f64.to_bits()));
        let zeros = format!("-{zeros}");
        assert_eq!(
            bits(decode::number(zeros.as_bytes())),
            Ok((-0.0f64).to_bits())
        );
        // An integer can't underflow: zero from zeros, or at least one.
        assert_eq!(decode::integer(b"0000"), Ok(0));
        assert_eq!(decode::number(b"0000"), Ok(0.0));
        let ones = format!("1{}", "0".repeat(400));
        assert_eq!(decode::integer(ones.as_bytes()), Err(NumberError::Overflow));
        assert_eq!(decode::number(ones.as_bytes()), Err(NumberError::Overflow));
    }

    /// [`decode::integer`] as it was: by the tokenizer's word, then Rust's
    /// reading of the text.
    fn integer_by_text(bytes: &[u8]) -> Result<i64, NumberError> {
        if crate::tokenizer::word(bytes) != crate::Kind::Integer {
            return Err(NumberError::NotANumber);
        }
        let text = core::str::from_utf8(bytes).map_err(|_| NumberError::NotANumber)?;
        text.parse().map_err(|_| NumberError::Overflow)
    }

    /// [`decode::number`] as it was, every number by Rust's reading, zero
    /// from a digit that isn't an underflow.
    fn number_by_text(bytes: &[u8]) -> Result<f64, NumberError> {
        if !matches!(
            crate::tokenizer::word(bytes),
            crate::Kind::Integer | crate::Kind::Real
        ) {
            return Err(NumberError::NotANumber);
        }
        let text = core::str::from_utf8(bytes).map_err(|_| NumberError::NotANumber)?;
        let value: f64 = text.parse().map_err(|_| NumberError::NotANumber)?;
        if !value.is_finite() {
            Err(NumberError::Overflow)
        } else if value == 0.0 && bytes.iter().any(|b| matches!(b, b'1'..=b'9')) {
            Err(NumberError::Underflow)
        } else {
            Ok(value)
        }
    }

    /// `bytes` read both ways, the same: the integer, and the number to the
    /// bit (so `-0` is told from `0`).
    fn read_alike(bytes: &[u8]) {
        assert_eq!(decode::integer(bytes), integer_by_text(bytes), "{bytes:?}");
        let bits = |n: Result<f64, NumberError>| n.map(f64::to_bits);
        assert_eq!(
            bits(decode::number(bytes)),
            bits(number_by_text(bytes)),
            "{bytes:?}"
        );
    }

    /// Every word of up to six bytes of digits, signs, points, `e`, a
    /// letter and a space reads as Rust read its text; and the edges:
    /// `i64`'s ends and past them, 2^53 and either side of it, 19 and 20
    /// digits, 22 and 23 after the point, with each sign, long words, and
    /// 20,000 words of up to 30 bytes made by xorshift from a seed.
    #[test]
    fn a_number_read_by_its_bytes_is_what_its_text_reads_as() {
        const ALPHABET: &[u8] = b"0159.+-ex ";
        let mut word = Vec::new();
        let mut count = 0;
        fn each(word: &mut Vec<u8>, left: usize, count: &mut usize) {
            read_alike(word);
            *count += 1;
            if left == 0 {
                return;
            }
            for &b in ALPHABET {
                word.push(b);
                each(word, left - 1, count);
                word.pop();
            }
        }
        each(&mut word, 6, &mut count);
        assert_eq!(count, 1_111_111);
        let edges = [
            "9223372036854775807",
            "9223372036854775808",
            "-9223372036854775808",
            "-9223372036854775809",
            "00000000000000000000009",
            "9007199254740992",
            "9007199254740993",
            "9007199254740991",
            "900719925474099.2",
            "900719925474099.3",
            ".9007199254740993",
            "1234567890123456789",
            "12345678901234567890",
            "1.234567890123456789",
            "0.0000000000000000000001",
            "0.00000000000000000000001",
            ".1234567890123456789012",
            "1.2345678901234567890123",
            "0.1",
            "0.3",
            "2.675",
            "0",
            "-0",
            "-0.",
            "-.0",
            "+0.0",
            "1.",
            ".",
            "-",
            "+.",
            "--1",
            "1..",
            "1.2.3",
        ];
        for edge in edges {
            for sign in ["", "-", "+"] {
                read_alike(format!("{sign}{edge}").as_bytes());
            }
        }
        for len in [300, 308, 309, 310, 400] {
            let mut long = vec![b'9'; len];
            read_alike(&long);
            long[len / 2] = b'.';
            read_alike(&long);
            // Too small to hold, and zero.
            let mut tiny = vec![b'0'; len];
            tiny[1] = b'.';
            read_alike(&tiny);
            tiny[len - 1] = b'1';
            read_alike(&tiny);
        }
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..20_000 {
            let len = (next() % 31) as usize;
            let word: Vec<u8> = (0..len)
                .map(|_| match next() % 16 {
                    0 => b'.',
                    1 => b'-',
                    2 => b'+',
                    3 => b'e',
                    _ => b'0' + (next() % 10) as u8,
                })
                .collect();
            read_alike(&word);
        }
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
        assert_eq!(
            NumberError::Underflow.to_string(),
            "a number too small to hold"
        );
    }

    /// `How::ALL` holds every `How` in the order of their discriminants,
    /// which `Quirks` indexes its bits by, the ordinary ones first.
    #[test]
    fn all_lists_every_how_in_order() {
        for (i, &how) in How::ALL.iter().enumerate() {
            assert_eq!(how as usize, i, "{how:?}");
        }
        let (ordinary, quirks) = How::ALL.split_at(5);
        assert!(ordinary.iter().all(|how| !how.is_quirk()), "{ordinary:?}");
        assert!(quirks.iter().all(|how| how.is_quirk()), "{quirks:?}");
        assert_eq!(How::ALL.last(), Some(&How::Invalid));
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

    /// `verbatim` searches the bytes for what its pieces would decode: the
    /// same answer as walking them, from the same place, for every run of
    /// bytes of these inputs taken as a name, a literal string and a hex
    /// string, a token's or not, and for every token of them.
    #[test]
    fn verbatim_by_a_search_is_verbatim_by_the_pieces() {
        use crate::Kind;
        let kinds = [Kind::Name, Kind::LiteralString, Kind::HexString];
        for input in VERBATIM_INPUTS {
            for start in 0..=input.len() {
                for end in start..=input.len() {
                    for kind in kinds {
                        let bytes = &input[start..end];
                        let token = Token {
                            kind,
                            closed: true,
                            start,
                            bytes,
                        };
                        fuzz::verbatim_as_walked(&token);
                    }
                }
            }
            fuzz::walk(input, 0);
        }
        let scanned = |kind, bytes: &'static [u8]| {
            Token {
                kind,
                closed: true,
                start: 0,
                bytes,
            }
            .verbatim()
        };
        assert_eq!(
            scanned(Kind::Name, b"Type"),
            Some(&b"Type"[..]),
            "a name's `/` is a delimiter only first"
        );
        assert_eq!(scanned(Kind::Name, b"/A/B"), None);
        assert_eq!(
            scanned(Kind::LiteralString, b"(a)b"),
            None,
            "nothing after the close"
        );
        assert_eq!(
            scanned(Kind::LiteralString, b"a)b"),
            None,
            "without its `(`, a string is open already: the `)` closes it"
        );
        assert_eq!(scanned(Kind::LiteralString, b"a)"), Some(&b"a"[..]));
        assert_eq!(scanned(Kind::LiteralString, b"a(b)c"), Some(&b"a(b)c"[..]));
        assert_eq!(scanned(Kind::HexString, b">"), Some(&b""[..]));
        assert_eq!(scanned(Kind::HexString, b"<<"), None);
    }

    /// Bytes given to the free decoders without their opener are the inside
    /// of a value already open: a `)` that balances no `(` closes a literal
    /// string, and a `(` nests as inside any string.
    #[test]
    fn bytes_without_their_opener_are_the_inside_of_a_value() {
        let pieces = |pieces: Pieces<'_>| pieces.map(|p| (p.byte, p.how)).collect::<Vec<_>>();
        let plain = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|&b| (Some(b), How::Plain))
                .collect::<Vec<_>>()
        };
        assert_eq!(pieces(decode::literal(b"a(b)c")), plain(b"a(b)c"));
        assert_eq!(
            pieces(decode::literal(b"a)b")),
            [
                (Some(b'a'), How::Plain),
                (None, How::Delimiter),
                (None, How::Invalid),
            ]
        );
        assert_eq!(pieces(decode::literal(b")")), [(None, How::Delimiter)]);
        assert!(pieces(decode::literal(b"")).is_empty());
        // With its opener, as before: the `)` that balances it closes it.
        let mut whole = vec![(None, How::Delimiter)];
        whole.extend(plain(b"a(b)c"));
        whole.push((None, How::Delimiter));
        assert_eq!(pieces(decode::literal(b"(a(b)c)")), whole);
        assert_eq!(
            pieces(decode::hex(b"4142")),
            [(Some(b'A'), How::Escape), (Some(b'B'), How::Escape)]
        );
        assert_eq!(pieces(decode::hex(b">")), [(None, How::Delimiter)]);
        assert_eq!(
            pieces(decode::hex(b"41>")),
            [(Some(b'A'), How::Escape), (None, How::Delimiter)]
        );
        assert!(pieces(decode::name(b"")).is_empty());
        assert_eq!(pieces(decode::name(b"abc")), plain(b"abc"));
        assert_eq!(pieces(decode::name(b"a/b"))[1], (None, How::Invalid));
        // The offsets are the bytes', as for any bytes given.
        let at: Vec<_> = decode::literal(b"a)b").map(|p| p.at).collect();
        assert_eq!(at, [0..1, 1..2, 2..3]);
    }

    /// Every run of bytes of the verbatim test's inputs, read as a name, a
    /// literal string and a hex string by the pieces and by PDFium's
    /// readings, from the start and after each piece or byte.
    fn each_decoding(mut check: impl FnMut(Pieces<'_>, decode::pdfium::Bytes<'_>)) {
        for input in VERBATIM_INPUTS {
            for start in 0..=input.len() {
                for end in start..=input.len() {
                    let bytes = &input[start..end];
                    check(decode::name(bytes), decode::pdfium::name(bytes));
                    check(decode::literal(bytes), decode::pdfium::literal(bytes));
                    check(decode::hex(bytes), decode::pdfium::hex(bytes));
                }
            }
        }
    }

    /// `size_hint` bounds what's left: a piece at least while bytes are
    /// left and at most one a byte, and of a value's bytes at most one a
    /// byte, by the pieces and by PDFium's readings, so a buffer of the
    /// upper bound holds the value without growing; and each stays true
    /// after each piece or byte.
    #[test]
    fn size_hint_bounds_the_pieces_and_the_bytes_left() {
        /// `it` yields within its hint at every step, the hint's upper
        /// bound falls by one at least a step, and once it has ended the
        /// hint is (0, Some(0)).
        fn bounded<I: Iterator + Clone>(mut it: I) {
            loop {
                let (lower, upper) = it.size_hint();
                let left = it.clone().count();
                assert!(
                    lower <= left && left <= upper.unwrap(),
                    "{lower} <= {left} <= {upper:?}"
                );
                if it.next().is_none() {
                    assert_eq!(lower, 0, "nothing promised where nothing came");
                    assert_eq!(it.size_hint(), (0, Some(0)));
                    return;
                }
                assert!(
                    it.size_hint().1.unwrap() < upper.unwrap(),
                    "a step costs a byte"
                );
            }
        }
        let mut checked = 0;
        each_decoding(|pieces, pdfium| {
            let bytes = pieces.rest().len();
            assert_eq!(pieces.size_hint(), (usize::from(bytes > 0), Some(bytes)));
            assert_eq!(pieces.clone().bytes().size_hint(), (0, Some(bytes)));
            assert_eq!(pdfium.size_hint().0, 0);
            assert!(pdfium.size_hint().1.unwrap() <= bytes);
            bounded(pieces.clone());
            bounded(pieces.clone().bytes());
            bounded(pdfium.clone());
            // Reserved from the upper bound, a buffer never grows.
            let value = pieces.bytes();
            let upper = value.size_hint().1.unwrap();
            let mut held = Vec::with_capacity(upper);
            held.extend(value);
            assert!(held.len() <= upper && held.capacity() == upper);
            let upper = pdfium.size_hint().1.unwrap();
            let mut held = Vec::with_capacity(upper);
            held.extend(pdfium);
            assert!(held.len() <= upper && held.capacity() == upper);
            checked += 1;
        });
        assert!(checked > 1000, "{checked}");
        // A read past a string's close has nothing left.
        let mut read = decode::pdfium::literal(b"(a)bc");
        assert_eq!(read.size_hint(), (0, Some(4)));
        assert_eq!(read.by_ref().count(), 1);
        assert_eq!(read.size_hint(), (0, Some(0)));
    }

    /// A `Quirks` set is built from `How`s, one by one or collected, or
    /// from two sets, holds quirks alone, and iterates them in `How::ALL`'s
    /// order, exactly as many as `len` says.
    #[test]
    fn quirks_are_built_from_hows_and_iterate_in_order() {
        let quirks = token(b"(\\q\r)").pieces().unwrap().quirks();
        assert_eq!(quirks.len(), 2);
        let listed: Vec<How> = quirks.iter().collect();
        assert_eq!(listed, [How::IgnoredBackslash, How::EolAsLf]);
        let mut by_for = Vec::new();
        for how in quirks {
            by_for.push(how);
        }
        assert_eq!(by_for, listed);
        assert_eq!(quirks.iter().len(), 2);
        assert_eq!(quirks.iter().size_hint(), (2, Some(2)));
        // The same set, collected from each piece's `How`, the ordinary
        // ones ignored.
        let collected: decode::Quirks = (token(b"(\\q\r)").pieces().unwrap())
            .map(|p| p.how)
            .collect();
        assert_eq!(collected, quirks);
        let hows = [How::EolAsLf, How::Plain, How::IgnoredBackslash, How::Escape];
        assert_eq!(hows.into_iter().collect::<decode::Quirks>(), quirks);
        // Built one by one: a `How` that isn't a quirk does nothing.
        let mut built = decode::Quirks::default();
        assert_eq!((built.is_empty(), built.len()), (true, 0));
        for how in [
            How::Delimiter,
            How::Plain,
            How::Escape,
            How::LineContinuation,
            How::HexSpace,
        ] {
            built.insert(how);
            assert!(built.is_empty(), "{how:?} isn't a quirk");
        }
        built.insert(How::EolAsLf);
        built.insert(How::EolAsLf);
        assert_eq!((built.len(), built.contains(How::EolAsLf)), (1, true));
        built.extend([How::Plain, How::IgnoredBackslash]);
        assert_eq!(built, quirks);
        // Joined.
        let stray: decode::Quirks = [How::StrayHash].into_iter().collect();
        let joined = stray | quirks;
        assert_eq!(joined.len(), 3);
        assert_eq!(
            joined.iter().collect::<Vec<_>>(),
            [How::StrayHash, How::IgnoredBackslash, How::EolAsLf]
        );
        let mut joined_in_place = quirks;
        joined_in_place |= stray;
        assert_eq!(joined_in_place, joined);
        assert_eq!(quirks | decode::Quirks::default(), quirks);
        // Every quirk at once: `How::ALL`'s quirks, in its order.
        let all: decode::Quirks = How::ALL.iter().copied().collect();
        let every_quirk: Vec<How> = How::ALL.iter().copied().filter(|h| h.is_quirk()).collect();
        assert_eq!(all.iter().collect::<Vec<_>>(), every_quirk);
        assert_eq!(all.len(), every_quirk.len());
        assert!(every_quirk.iter().all(|&h| all.contains(h)));
        assert_eq!(
            format!("{all:?}"),
            format!("{every_quirk:?}")
                .replace('[', "{")
                .replace(']', "}")
        );
        let mut rest = all.iter();
        assert_eq!(rest.len(), every_quirk.len());
        rest.next();
        assert_eq!(rest.len(), every_quirk.len() - 1);
        assert_eq!(rest.clone().count(), rest.len());
    }

    /// The inputs the verbatim and size tests take every run of bytes of.
    const VERBATIM_INPUTS: [&[u8]; 9] = [
        b"/Type/A#20B/#/ x\x80",
        b"(a (b) c)(a)b)()(",
        b"(a\\) (a\r\nb) (\\(\\)) ((a)) (a)) )(",
        b"<41 4G> <> < > <<>> >",
        b"/A)(B(C) (D)E",
        b"(\x80\xff/[]{}%) [<a>]",
        b"/a#2 /a#20 /#00",
        b"",
        b")))(((",
    ];
}
