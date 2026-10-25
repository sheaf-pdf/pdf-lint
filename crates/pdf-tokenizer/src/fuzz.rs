//! What the fuzz target checks, which the unit tests check too: every
//! promise the tokenizer and the decoders make, on any input. It's compiled
//! only for tests and under cargo-fuzz's `--cfg fuzzing`.

use core::str::from_utf8;

use crate::decode::{How, NumberError};
use crate::{Kind, Token, Tokenizer, is_regular, is_white_space};

/// The fuzz target's body: [`walk`]s the input after `data`'s first two
/// bytes, from an offset those bytes choose.
pub fn tokenize(data: &[u8]) {
    let (offset, input) = match data {
        [a, b, input @ ..] => {
            let seed = usize::from(u16::from_le_bytes([*a, *b]));
            (seed % (input.len() + 1), input)
        }
        _ => (0, data),
    };
    walk(input, offset);
}

/// Tokenizes `input` from `offset` to its end, taking each stream's data as
/// a reader might (an end of line, then up to `endstream`), and checks
/// every token. Returns how many tokens there were. Panics on a broken
/// promise.
pub fn walk(input: &[u8], offset: usize) -> usize {
    let mut tokenizer = Tokenizer::at(input, offset);
    let mut at = offset.min(input.len());
    let mut count = 0;
    while let Some(token) = tokenizer.next() {
        check(input, &token, at);
        let again = Tokenizer::at(input, token.start).next();
        assert_eq!(again, Some(token), "started at a token, it yields it");
        if token.kind == Kind::Space {
            let next = input.get(token.end()).copied();
            assert!(
                !next.is_some_and(is_white_space),
                "a run of white-space is one token"
            );
        }
        check_decoding(input, &token);
        at = token.end();
        count += 1;
        if token.kind == Kind::Keyword && token.bytes == b"stream" {
            if let Some(eol) = tokenizer.eol() {
                check(input, &eol, at);
                at = eol.end();
                count += 1;
            }
            let data = tokenizer.data_until(b"endstream");
            check(input, &data, at);
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

/// Checks that `token` starts at `at`, is the input's bytes there, and
/// keeps its kind's promises.
pub fn check(input: &[u8], token: &Token<'_>, at: usize) {
    assert_eq!(token.start, at, "tokens join with no gap or overlap");
    assert_eq!(input.get(token.range()), Some(token.bytes));
    let bytes = token.bytes;
    let next = input.get(token.end()).copied();
    if token.kind != Kind::Data {
        assert!(!bytes.is_empty(), "only data can be empty");
    }
    let maximal = !next.is_some_and(is_regular);
    match token.kind {
        Kind::Space => assert!(bytes.iter().all(|&b| is_white_space(b))),
        Kind::Comment => {
            assert_eq!(bytes[0], b'%');
            assert!(!bytes.iter().any(|&b| b == b'\r' || b == b'\n'));
            assert!(matches!(next, None | Some(b'\r' | b'\n')));
        }
        Kind::Name => {
            assert_eq!(bytes[0], b'/');
            assert!(bytes[1..].iter().all(|&b| is_regular(b)));
            assert!(maximal);
        }
        Kind::Integer | Kind::Real | Kind::Keyword => {
            assert!(bytes.iter().all(|&b| is_regular(b)));
            assert!(maximal);
            assert_eq!(token.kind, number(bytes).unwrap_or(Kind::Keyword));
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
            } else {
                assert!(!next.is_some_and(|b| hex_or_space(b) || b == b'>'));
                &bytes[1..]
            };
            assert!(inside.iter().all(|&b| hex_or_space(b)));
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

/// Checks what a token decodes to, read apart from the decoder: a name's
/// or string's pieces join into it, and each stands for what its own bytes
/// say; an integer is the one its digits say.
pub fn check_decoding(input: &[u8], token: &Token<'_>) {
    match token.kind {
        Kind::Integer => check_integer(token),
        Kind::Real => check_number(token),
        Kind::Name | Kind::String { .. } | Kind::HexString { .. } => check_pieces(input, token),
        _ => {
            assert!(token.pieces().is_none());
            assert_eq!(token.number(), Err(NumberError::NotANumber));
        }
    }
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

fn check_pieces(input: &[u8], token: &Token<'_>) {
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
                assert!(
                    matches!(next, None | Some(b'>')),
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
