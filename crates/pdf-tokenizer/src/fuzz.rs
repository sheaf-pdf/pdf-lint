//! What the fuzz target checks, which the unit tests check too: every
//! promise the tokenizer and the decoders make, on any input, under each
//! set of rules, the two sets side by side, and each chosen as the
//! tokenizer runs against the same fixed in its type. It's compiled only
//! for tests and under cargo-fuzz's `--cfg fuzzing`.

use core::str::from_utf8;

use crate::decode::pdfium::{self, Number};
use crate::decode::{How, NumberError};
use crate::{Iso, Kind, Pdfium, RuleSet, Rules, Token, Tokenizer, is_white_space};

/// The fuzz target's body: [`walk`]s the input after `data`'s first two
/// bytes, from an offset those bytes choose, and checks it cut short
/// ([`prefix`]), from that offset, where the same bytes read the other way
/// round choose: each under both sets of rules, and the two side by side.
pub fn tokenize(data: &[u8]) {
    let (offset, cut, input) = match data {
        [a, b, input @ ..] => {
            let seed = usize::from(u16::from_le_bytes([*a, *b]));
            let offset = seed % (input.len() + 1);
            let seed = usize::from(u16::from_be_bytes([*a, *b]));
            (offset, offset + seed % (input.len() - offset + 1), input)
        }
        _ => (0, data.len(), data),
    };
    walk(input, offset);
    prefix(input, offset, cut);
}

/// [`prefix_with`] under each set of rules, fixed in the tokenizer's type.
pub fn prefix(input: &[u8], offset: usize, cut: usize) {
    prefix_with(input, offset, cut, Iso);
    prefix_with(input, offset, cut, Pdfium);
}

/// Checks the tokens of `input[..cut]` from `offset` against the whole
/// input's, in lockstep, under `rules`, taking each stream's data as
/// [`walk`] does: each that ends before the cut is the whole input's token
/// at the same place, its kind and bytes; the last reaches the cut,
/// starting where the whole input's token does; and none comes after it.
/// Only a token that reaches the end of a slice can come out otherwise with
/// more bytes after it. The cut is taken no nearer than `offset`, and no
/// further than the input's end. Panics on a broken promise.
pub fn prefix_with<R: RuleSet>(input: &[u8], offset: usize, cut: usize, rules: R) {
    let offset = offset.min(input.len());
    let cut = cut.clamp(offset, input.len());
    let mut part = Tokenizer::at(&input[..cut], offset).with_rules(rules);
    let mut whole = Tokenizer::at(input, offset).with_rules(rules);
    // Whether the slice goes on after `token`, checked against `full`, the
    // whole input's token at the same point.
    let settled = |token: Token<'_>, full: Option<Token<'_>>| {
        if token.end() < cut {
            assert_eq!(Some(token), full, "settled by the bytes before the cut");
            return true;
        }
        assert_eq!(token.end(), cut, "the last token reaches the cut");
        assert_eq!(full.map(|f| f.start), Some(token.start));
        false
    };
    while let Some(token) = part.next() {
        if !settled(token, whole.next()) {
            break;
        }
        if token.kind == Kind::Keyword && token.bytes == b"stream" {
            let (eol, full) = (part.eol(), whole.eol());
            match eol {
                Some(eol) if !settled(eol, full) => break,
                Some(_) => {}
                None => assert_eq!(full, None, "no end of line before the cut"),
            }
            let data = part.data_until(b"endstream");
            if !settled(data, Some(whole.data_until(b"endstream"))) {
                break;
            }
        }
    }
    assert_eq!(part.offset(), cut, "the tokens reach the cut");
    assert_eq!(part.next(), None, "nothing after the last");
}

/// [`walk_with`] under each set of rules, fixed in the tokenizer's type,
/// then [`side_by_side`] and [`chosen_as_fixed`]. Returns how many tokens
/// there were by ISO 32000-2's rules.
pub fn walk(input: &[u8], offset: usize) -> usize {
    let count = walk_with(input, offset, Iso);
    walk_with(input, offset, Pdfium);
    side_by_side(input, offset);
    chosen_as_fixed(input, offset);
    count
}

/// Checks that each set of rules, chosen as the tokenizer runs (a
/// [`Rules`] value), gives the tokens of `input` from `offset` that the
/// same rules fixed in its type give.
pub fn chosen_as_fixed(input: &[u8], offset: usize) {
    let iso = Tokenizer::at(input, offset);
    let chosen = iso.clone().with_rules(Rules::Iso);
    assert_eq!((iso.rules(), chosen.rules()), (Rules::Iso, Rules::Iso));
    assert!(iso.eq(chosen), "ISO 32000-2's rules, fixed or chosen");
    let pdfium = Tokenizer::at(input, offset).with_rules(Pdfium);
    let chosen = pdfium.clone().with_rules(Rules::Pdfium);
    assert_eq!(
        (pdfium.rules(), chosen.rules()),
        (Rules::Pdfium, Rules::Pdfium)
    );
    assert!(pdfium.eq(chosen), "PDFium's rules, fixed or chosen");
}

/// Tokenizes `input` from `offset` to its end under the rules `set`,
/// taking each stream's data as a reader might (an end of line, then up
/// to `endstream`), and checks every token. Returns how many tokens there
/// were. Panics on a broken promise.
pub fn walk_with<R: RuleSet>(input: &[u8], offset: usize, set: R) -> usize {
    let mut tokenizer = Tokenizer::at(input, offset).with_rules(set);
    let rules = set.rules();
    assert_eq!(tokenizer.rules(), rules);
    let mut at = offset.min(input.len());
    let mut count = 0;
    while let Some(token) = tokenizer.next() {
        check_with(input, &token, at, rules);
        let again = Tokenizer::at(input, token.start).with_rules(set).next();
        assert_eq!(again, Some(token), "started at a token, it yields it");
        if token.kind == Kind::Space {
            let next = input.get(token.end()).copied();
            assert!(
                !next.is_some_and(|b| rules.is_white_space(b)),
                "a run of white-space is one token"
            );
        }
        check_decoding_with(input, &token, rules);
        at = token.end();
        count += 1;
        if token.kind == Kind::Keyword && token.bytes == b"stream" {
            if let Some(eol) = tokenizer.eol() {
                check_with(input, &eol, at, rules);
                at = eol.end();
                count += 1;
            }
            let data = tokenizer.data_until(b"endstream");
            check_with(input, &data, at, rules);
            let marker = &b"endstream"[..];
            assert!(!data.bytes.windows(marker.len()).any(|w| w == marker));
            let after = input.get(data.end()..).unwrap_or_default();
            assert!(after.is_empty() || after.starts_with(marker));
            at = data.end();
            count += 1;
        }
    }
    assert_eq!(at, input.len(), "the tokens reach the end of the input");
    count
}

/// [`check_with`] under ISO 32000-2's rules.
pub fn check(input: &[u8], token: &Token<'_>, at: usize) {
    check_with(input, token, at, Rules::Iso);
}

/// Checks that `token`, read under `rules`, starts at `at`, is the input's
/// bytes there, and keeps its kind's promises.
pub fn check_with(input: &[u8], token: &Token<'_>, at: usize, rules: Rules) {
    assert_eq!(token.start, at, "tokens join with no gap or overlap");
    assert_eq!(input.get(token.range()), Some(token.bytes));
    let bytes = token.bytes;
    let next = input.get(token.end()).copied();
    if token.kind != Kind::Data {
        assert!(!bytes.is_empty(), "only data can be empty");
    }
    let regular = |b| rules.is_regular(b);
    let maximal = !next.is_some_and(regular);
    match token.kind {
        Kind::Space => assert!(bytes.iter().all(|&b| rules.is_white_space(b))),
        Kind::Comment => {
            assert_eq!(bytes[0], b'%');
            assert!(!bytes.iter().any(|&b| b == b'\r' || b == b'\n'));
            assert!(matches!(next, None | Some(b'\r' | b'\n')));
        }
        Kind::Name => {
            assert_eq!(bytes[0], b'/');
            assert!(bytes[1..].iter().all(|&b| regular(b)));
            assert!(maximal);
        }
        Kind::Integer | Kind::Real | Kind::Keyword => {
            assert!(bytes.iter().all(|&b| regular(b)));
            assert!(maximal);
            let kind = match rules {
                Rules::Iso => number(bytes),
                Rules::Pdfium => pdfium_word(bytes),
            };
            assert_eq!(token.kind, kind.unwrap_or(Kind::Keyword));
        }
        Kind::String { closed } => {
            assert_eq!(bytes[0], b'(');
            assert_eq!(balanced_at(bytes), closed.then_some(bytes.len()));
            if !closed {
                assert_eq!(token.end(), input.len());
            }
        }
        Kind::HexString { closed } => {
            assert_eq!(bytes[0], b'<');
            assert_ne!(input.get(token.start + 1), Some(&b'<'));
            let inside = if closed {
                assert_eq!(bytes.last(), Some(&b'>'));
                &bytes[1..bytes.len() - 1]
            } else if rules == Rules::Pdfium {
                assert_eq!(token.end(), input.len(), "unclosed only at the end");
                &bytes[1..]
            } else {
                assert!(!next.is_some_and(|b| hex_or_space(b) || b == b'>'));
                &bytes[1..]
            };
            match rules {
                Rules::Iso => assert!(inside.iter().all(|&b| hex_or_space(b))),
                Rules::Pdfium => assert!(!inside.contains(&b'>'), "to its first `>`"),
            }
        }
        Kind::ArrayOpen => assert_eq!(bytes, b"["),
        Kind::ArrayClose => assert_eq!(bytes, b"]"),
        Kind::DictOpen => assert_eq!(bytes, b"<<"),
        Kind::DictClose => assert_eq!(bytes, b">>"),
        Kind::BraceOpen => assert_eq!(bytes, b"{"),
        Kind::BraceClose => assert_eq!(bytes, b"}"),
        Kind::Junk => assert!(bytes == b")" || (bytes == b">" && next != Some(b'>'))),
        Kind::Data => {}
    }
}

/// [`check_decoding_with`] under ISO 32000-2's rules.
pub fn check_decoding(input: &[u8], token: &Token<'_>) {
    check_decoding_with(input, token, Rules::Iso);
}

/// Checks what a token read under `rules` decodes to, read apart from the
/// decoder: a name's or string's pieces join into it, and each stands for
/// what its own bytes say; an integer is the one its digits say. The
/// pieces and numbers are ISO 32000-2's reading, so a number word only
/// PDFium's rules make one, such as `--5`, isn't a number to them. And
/// PDFium's readings, on every token whatever its rules: a number's as
/// PDFium's digits say, a name's and a string's bytes as their pieces or
/// their own bytes say.
pub fn check_decoding_with(input: &[u8], token: &Token<'_>, rules: Rules) {
    let iso_number = number(token.bytes) == Some(token.kind);
    match token.kind {
        Kind::Integer | Kind::Real if rules == Rules::Pdfium && !iso_number => {
            assert_eq!(token.integer(), Err(NumberError::NotANumber));
            assert_eq!(token.number(), Err(NumberError::NotANumber));
        }
        Kind::Integer => check_integer(token),
        Kind::Real => check_number(token),
        Kind::Name | Kind::String { .. } | Kind::HexString { .. } => {
            check_pieces(input, token, rules);
            check_pdfium_bytes(input, token);
        }
        _ => {
            assert!(token.pieces().is_none());
            assert_eq!(token.number(), Err(NumberError::NotANumber));
        }
    }
    check_pdfium_number(token.bytes);
}

fn check_integer(token: &Token<'_>) {
    let (negative, digits) = match token.bytes {
        [b'-', digits @ ..] => (true, digits),
        [b'+', digits @ ..] => (false, digits),
        digits => (false, digits),
    };
    let mut value = Some(0i128);
    for &d in digits {
        value = value.and_then(|v| v.checked_mul(10)?.checked_add(i128::from(d - b'0')));
    }
    let value = value.map(|v| if negative { -v } else { v });
    let expected = value.and_then(|v| i64::try_from(v).ok());
    assert_eq!(token.integer(), expected.ok_or(NumberError::Overflow));
    check_number(token);
}

/// Checks that a number is a finite `f64` when its integer part has 308
/// digits or fewer, and too large when it has 310 or more: `f64::MAX` is
/// about 1.8e308.
fn check_number(token: &Token<'_>) {
    let unsigned = token
        .bytes
        .strip_prefix(b"-")
        .or(token.bytes.strip_prefix(b"+"));
    let unsigned = unsigned.unwrap_or(token.bytes);
    let whole = unsigned.split(|&b| b == b'.').next().unwrap_or_default();
    let digits = whole.iter().skip_while(|&&b| b == b'0').count();
    match token.number() {
        Ok(value) => assert!(
            value.is_finite() && digits <= 309,
            "{value} from {digits} digits"
        ),
        Err(NumberError::Overflow) => assert!(digits >= 309, "too large at {digits} digits"),
        Err(NumberError::NotANumber) => panic!("a number token is a number"),
    }
}

/// Checks PDFium's reading of a number against its digits, read apart:
/// a word without a point is an integer of 32 bits, unsigned unless it
/// starts with a sign, and 0 past its range; one with a point is the
/// decimal it starts with, negative after a minus, a finite `f32` when
/// that has 38 digits or fewer before its point and too large for one
/// when it has 40 or more (`f32::MAX` is about 3.4e38), and so at 64 bits
/// for 308 and 310. Any bytes are read, never panicking.
fn check_pdfium_number(bytes: &[u8]) {
    let read = pdfium::number(bytes);
    let (float, integer, real) = (read.float(), read.integer(), pdfium::real(bytes));
    assert!(!float.is_nan() && !real.is_nan());
    if bytes.contains(&b'.') {
        let Number::Real(value) = read else {
            panic!("a word with a point is a real: {read:?}");
        };
        // The decimal: past the signs and spaces, but for a minus right
        // before the rest; then a minus, digits, a point and digits.
        let signs = bytes.iter().take_while(|b| b" +-".contains(b)).count();
        let minus = signs > 0 && bytes[signs - 1] == b'-';
        let rest = &bytes[signs..];
        let whole = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        let fraction = match rest.get(whole) {
            Some(b'.') => rest[whole + 1..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count(),
            _ => 0,
        };
        if whole + fraction == 0 {
            assert_eq!((value, real), (0.0, 0.0), "no digit, no number");
            return;
        }
        assert_eq!(
            value < 0.0 || real < 0.0,
            minus && (value != 0.0 || real != 0.0)
        );
        let significant = rest[..whole].iter().skip_while(|&&b| b == b'0').count();
        if significant <= 38 {
            assert!(value.is_finite(), "{value} from {significant} digits");
        } else if significant >= 40 {
            assert!(value.is_infinite(), "{value} from {significant} digits");
        }
        if significant <= 308 {
            assert!(real.is_finite());
        } else if significant >= 310 {
            assert!(real.is_infinite());
        }
        assert_eq!(float, value);
        assert_eq!(integer, value as i32);
        return;
    }
    let (sign, digits) = match bytes {
        [b'-', digits @ ..] => (Some(-1i128), digits),
        [b'+', digits @ ..] => (Some(1), digits),
        digits => (None, digits),
    };
    let mut value = Some(0i128);
    for &d in digits.iter().take_while(|b| b.is_ascii_digit()) {
        value = value
            .map(|v| v * 10 + i128::from(d - b'0'))
            .filter(|&v| v <= i128::from(u32::MAX));
    }
    let value = value.unwrap_or(0);
    let meant = match sign {
        None => Number::Unsigned(u32::try_from(value).unwrap()),
        Some(sign) => Number::Signed(i32::try_from(sign * value).unwrap_or(0)),
    };
    assert_eq!(read, meant, "{bytes:?}");
    match read {
        Number::Unsigned(n) => assert_eq!((float, integer), (n as f32, n as i32)),
        Number::Signed(n) => assert_eq!((float, integer), (n as f32, n)),
        Number::Real(_) => unreachable!(),
    }
}

/// Checks PDFium's reading of a name's or a string's bytes. A literal
/// string's is its pieces' bytes, but each unescaped CR or CR LF as
/// written, and an octal escape of one or two digits that an unclosed
/// string ends on as nothing. A name's and a hex string's are read apart
/// from their bytes: a name's `#` with two bytes after it stands for them
/// as hex digits, and a hex string's digits pair up to its `>`. None is
/// ever longer than its token.
fn check_pdfium_bytes(input: &[u8], token: &Token<'_>) {
    let bytes = token.bytes;
    let digit = |b: u8| match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    };
    match token.kind {
        Kind::Name => {
            let body = &bytes[1..];
            let mut i = 0;
            let meant = core::iter::from_fn(|| {
                let &b = body.get(i)?;
                if b == b'#' && i + 2 < body.len() {
                    i += 3;
                    return Some(digit(body[i - 2]) * 16 + digit(body[i - 1]));
                }
                i += 1;
                Some(b)
            });
            assert!(pdfium::name(bytes).eq(meant), "{bytes:?}");
        }
        Kind::String { closed } => {
            let pieces = token.pieces().expect("strings have pieces");
            let meant = pieces.flat_map(|piece| {
                let written = &input[piece.at.clone()];
                let short_octal = written.len() <= 3
                    && written[0] == b'\\'
                    && written[1..].iter().all(|b| (b'0'..=b'7').contains(b))
                    && written.len() > 1;
                let dropped = !closed && piece.at.end == token.end() && short_octal;
                let (run, byte) = match piece.how {
                    How::EolAsLf => (written, None),
                    _ if dropped => (&[][..], None),
                    _ => (&[][..], piece.byte),
                };
                run.iter().copied().chain(byte)
            });
            assert!(pdfium::literal(bytes).eq(meant), "{bytes:?}");
        }
        Kind::HexString { .. } => {
            let inside = bytes[1..].split(|&b| b == b'>').next().unwrap_or_default();
            let mut digits = inside.iter().copied().filter(u8::is_ascii_hexdigit);
            let meant = core::iter::from_fn(|| {
                let high = digit(digits.next()?);
                Some(high * 16 + digits.next().map_or(0, digit))
            });
            assert!(pdfium::hex(bytes).eq(meant), "{bytes:?}");
        }
        _ => unreachable!("only names and strings"),
    }
    let longest = match token.kind {
        Kind::Name => pdfium::name(bytes).count(),
        Kind::String { .. } => pdfium::literal(bytes).count(),
        _ => pdfium::hex(bytes).count(),
    };
    assert!(longest < bytes.len(), "never longer than its token");
}

fn check_pieces(input: &[u8], token: &Token<'_>, rules: Rules) {
    let pieces = token.pieces().expect("names and strings have pieces");
    let total = pieces.clone().count();
    let closed = matches!(
        token.kind,
        Kind::String { closed: true } | Kind::HexString { closed: true }
    );
    let octal = |b: &u8| (b'0'..=b'7').contains(b);
    let mut at = token.start;
    for (i, piece) in pieces.enumerate() {
        assert_eq!(piece.at.start, at, "pieces join with no gap or overlap");
        assert!(piece.at.end > at, "every piece is a byte or more");
        at = piece.at.end;
        let written = &input[piece.at.clone()];
        let after = input.get(piece.at.end..token.end()).unwrap_or_default();
        let byte = piece.byte;
        match (token.kind, piece.how) {
            // A hex string by PDFium's rules runs over bytes ISO 32000-2's
            // can't hold.
            (Kind::HexString { .. }, How::Invalid) if rules == Rules::Pdfium => {
                assert!(written.len() == 1 && !hex_or_space(written[0]));
                assert_ne!(written[0], b'>');
                assert_eq!(byte, None);
            }
            (_, How::Invalid) => panic!("a token has no invalid bytes: {written:?}"),
            (_, How::Delimiter) => {
                let opening = i == 0 && matches!(written, b"/" | b"(" | b"<");
                let closing = i + 1 == total && closed && matches!(written, b")" | b">");
                assert!(
                    opening || closing,
                    "a delimiter only at the ends: {written:?}"
                );
                assert_eq!(byte, None);
            }
            (kind, How::Plain) => {
                assert_eq!(written.len(), 1);
                assert_eq!(byte, Some(written[0]));
                match kind {
                    Kind::Name => assert_ne!(written[0], b'#'),
                    Kind::String { .. } => assert!(!matches!(written[0], b'\\' | b'\r')),
                    _ => panic!("a hex string has no plain bytes"),
                }
            }
            (Kind::Name, How::Escape | How::NulEscape) => {
                assert!(matches!(written, [b'#', _, _]));
                let value = u8::from_str_radix(from_utf8(&written[1..]).unwrap(), 16).unwrap();
                assert_eq!(byte, Some(value));
                assert_eq!(value == 0, piece.how == How::NulEscape);
            }
            (Kind::Name, How::StrayHash) => {
                assert_eq!((written, byte), (&b"#"[..], Some(b'#')));
                let pair = after.len() >= 2 && after[..2].iter().all(u8::is_ascii_hexdigit);
                assert!(!pair, "a # with two hex digits is an escape");
            }
            (Kind::String { .. }, How::Escape | How::OctalOverflow | How::IgnoredBackslash) => {
                let [b'\\', escaped @ ..] = written else {
                    panic!("an escape starts with a backslash: {written:?}");
                };
                if !escaped.is_empty() && escaped.iter().all(octal) {
                    assert!(escaped.len() <= 3);
                    if escaped.len() < 3 {
                        assert!(!after.first().is_some_and(octal), "up to three digits");
                    }
                    let value = u16::from_str_radix(from_utf8(escaped).unwrap(), 8).unwrap();
                    assert_eq!(byte, Some((value % 256) as u8));
                    assert_eq!(value > 255, piece.how == How::OctalOverflow);
                } else {
                    let &[b] = escaped else {
                        panic!("one byte after a backslash: {written:?}");
                    };
                    let meant = match b {
                        b'n' => Some(b'\n'),
                        b'r' => Some(b'\r'),
                        b't' => Some(b'\t'),
                        b'b' => Some(0x08),
                        b'f' => Some(0x0C),
                        b'(' | b')' | b'\\' => Some(b),
                        _ => None,
                    };
                    assert_eq!(byte, Some(meant.unwrap_or(b)));
                    assert_eq!(meant.is_none(), piece.how == How::IgnoredBackslash);
                    assert!(b != b'\r' && b != b'\n');
                }
            }
            (Kind::String { .. }, How::LineContinuation) => {
                let lone_cr = written == b"\\\r" && after.first() != Some(&b'\n');
                assert!(written == b"\\\r\n" || written == b"\\\n" || lone_cr);
                assert_eq!(byte, None);
            }
            (Kind::String { .. }, How::EolAsLf) => {
                let lone_cr = written == b"\r" && after.first() != Some(&b'\n');
                assert!(written == b"\r\n" || lone_cr);
                assert_eq!(byte, Some(b'\n'));
            }
            (Kind::String { .. }, How::Dangling) => {
                assert_eq!(written, b"\\");
                assert!(i + 1 == total && !closed);
                assert_eq!(byte, None);
            }
            (Kind::HexString { .. }, How::HexSpace) => {
                assert!(written.len() == 1 && is_white_space(written[0]));
                assert_eq!(byte, None);
            }
            (Kind::HexString { .. }, How::Escape) => {
                let (high, low) = (written[0], written[written.len() - 1]);
                assert!(written.len() >= 2 && high.is_ascii_hexdigit() && low.is_ascii_hexdigit());
                assert!(
                    written[1..written.len() - 1]
                        .iter()
                        .all(|&b| is_white_space(b))
                );
                let value = u8::from_str_radix(from_utf8(&[high, low]).unwrap(), 16).unwrap();
                assert_eq!(byte, Some(value));
            }
            (Kind::HexString { .. }, How::OddDigit) => {
                assert!(written.len() == 1 && written[0].is_ascii_hexdigit());
                let next = after.iter().find(|&&b| !is_white_space(b));
                // By PDFium's rules, a byte ISO 32000-2's can't hold may
                // come before the next digit, as in `<4G1>`.
                let invalid =
                    rules == Rules::Pdfium && next.is_some_and(|b| !b.is_ascii_hexdigit());
                assert!(
                    matches!(next, None | Some(b'>')) || invalid,
                    "only the last digit is odd"
                );
                let value = u8::from_str_radix(from_utf8(written).unwrap(), 16).unwrap();
                assert_eq!(byte, Some(value * 16));
            }
            (kind, how) => panic!("{how:?} in a {kind:?}"),
        }
    }
    assert_eq!(at, token.end(), "the pieces reach the end of the token");
    check_verbatim(token);
}

/// Checks that [`Token::verbatim`] is the pieces' bytes, borrowed from the
/// token, exactly when every piece is plain or a delimiter; and that a value
/// is never longer than its token, each piece a byte or more standing for
/// one byte at most.
fn check_verbatim(token: &Token<'_>) {
    let pieces = token.pieces().expect("names and strings have pieces");
    assert!(pieces.clone().bytes().count() <= token.bytes.len());
    let plain = (pieces.clone()).all(|p| matches!(p.how, How::Delimiter | How::Plain));
    match token.verbatim() {
        Some(value) => {
            assert!(plain, "as written only when there's nothing to decode");
            assert!(value.iter().copied().eq(pieces.bytes()));
            let within = token.bytes.as_ptr_range();
            let inside = value.is_empty() || within.contains(&value.as_ptr());
            assert!(inside, "borrowed from the token");
        }
        None => assert!(!plain, "decoded only when there's something to decode"),
    }
}

/// Checks PDFium's tokens against ISO 32000-2's from the same places: at
/// each token PDFium's rules read from `offset`, ISO's rules give the same
/// token, but where the two part. A number word ISO's rules don't write as
/// one is a keyword to them, the same bytes. A hex string with a byte
/// that's neither a hex digit nor white-space before its `>` ends, to ISO's
/// rules, before that byte, unclosed. And 0x80 and 0xFF, white-space to
/// PDFium alone: ISO's white-space stops before one, which starts a
/// keyword, and a name or a word before one runs on over it.
pub fn side_by_side(input: &[u8], offset: usize) {
    let high = |b: u8| matches!(b, 0x80 | 0xFF);
    for token in Tokenizer::at(input, offset).with_rules(Pdfium) {
        let iso = Tokenizer::at(input, token.start).next();
        let iso = iso.expect("a token where PDFium's starts");
        assert_eq!(iso.start, token.start);
        let after = input.get(token.end()).copied();
        match token.kind {
            Kind::HexString { .. } if iso.bytes != token.bytes => {
                assert_eq!(iso.kind, Kind::HexString { closed: false });
                assert!(token.bytes.starts_with(iso.bytes));
                let stop = token.bytes[iso.bytes.len()];
                assert!(!hex_or_space(stop) && stop != b'>', "{stop:#x}");
            }
            Kind::Space if token.bytes.iter().any(|&b| high(b)) => {
                let lead = token.bytes.iter().take_while(|&&b| is_white_space(b));
                match lead.count() {
                    0 => assert_eq!((iso.kind, iso.bytes[0]), (Kind::Keyword, token.bytes[0])),
                    lead => assert_eq!((iso.kind, iso.bytes), (Kind::Space, &token.bytes[..lead])),
                }
            }
            Kind::Name | Kind::Integer | Kind::Real | Kind::Keyword if after.is_some_and(high) => {
                assert!(iso.bytes.len() > token.bytes.len());
                assert!(iso.bytes.starts_with(token.bytes));
                assert_eq!(iso.kind == Kind::Name, token.kind == Kind::Name);
            }
            _ => {
                assert_eq!(
                    iso.bytes, token.bytes,
                    "the same token but where the rules part"
                );
                let number = matches!(token.kind, Kind::Integer | Kind::Real);
                assert!(
                    iso.kind == token.kind || (number && iso.kind == Kind::Keyword),
                    "{:?} by PDFium's rules, {:?} by ISO's",
                    token.kind,
                    iso.kind
                );
            }
        }
    }
}

/// PDFium's numbers, written out apart from the tokenizer's own reading of
/// them: digits, signs and points, and a point makes a real.
fn pdfium_word(bytes: &[u8]) -> Option<Kind> {
    let numeric = |b: &u8| b.is_ascii_digit() || b"+-.".contains(b);
    if bytes.is_empty() || !bytes.iter().all(numeric) {
        return None;
    }
    Some(if bytes.contains(&b'.') {
        Kind::Real
    } else {
        Kind::Integer
    })
}

/// ISO 32000-2's numbers, written out apart from the tokenizer's own
/// reading of them: an optional sign, then digits with at most one point.
fn number(bytes: &[u8]) -> Option<Kind> {
    let body = match bytes {
        [b'+' | b'-', body @ ..] => body,
        body => body,
    };
    let digits = body.iter().filter(|b| b.is_ascii_digit()).count();
    let points = body.iter().filter(|&&b| b == b'.').count();
    if digits == 0 || points > 1 || digits + points != body.len() {
        return None;
    }
    Some(if points == 1 {
        Kind::Real
    } else {
        Kind::Integer
    })
}

/// Where a literal string's parentheses first balance, counted apart from
/// the tokenizer's own reading: drop each backslash and the byte it
/// escapes, then count what's left.
fn balanced_at(bytes: &[u8]) -> Option<usize> {
    let mut depth = 0i64;
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        i += 1;
        match b {
            b'\\' => i += 1,
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return Some(i.min(bytes.len()));
        }
    }
    None
}

fn hex_or_space(b: u8) -> bool {
    b.is_ascii_hexdigit() || is_white_space(b)
}
