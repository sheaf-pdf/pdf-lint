//! What a token is and where it ends, as PDFium's `CPDF_StreamParser`
//! ends them: at white-space and delimiters, a word of digits, signs and
//! points a number, strings nested, an array or a dictionary one token
//! with all it holds, and a closing delimiter with nothing to close a
//! token of its own. Only this module knows how a token ends; the rest of
//! the crate takes the ranges it cuts.

use std::ops::Range;

use crate::error::{Error, ErrorKind};

/// No cap on how deep arrays and dictionaries nest: for reading again
/// what was read under one, which holds none deeper. What's open is kept
/// in a list on the heap, not on the stack, so any depth is safe.
pub(crate) const UNCAPPED: usize = usize::MAX;

/// What a raw token is, before an inline image is made one.
pub(crate) enum Raw {
    Op,
    Operand,
}

/// The token from `from` on, past white-space and comments; None at the
/// end.
pub(crate) fn next_raw(
    s: &[u8],
    from: usize,
    depth: usize,
) -> Option<Result<(Range<usize>, Raw), Error>> {
    let start = skip_space(s, from);
    let end = match *s.get(start)? {
        b'(' => literal_string(s, start),
        b'[' => nested(s, start, depth),
        b'<' if s.get(start + 1) == Some(&b'<') => nested(s, start, depth),
        b'<' => hex_string(s, start),
        b'>' if s.get(start + 1) == Some(&b'>') => Ok(start + 2),
        b')' | b'>' | b']' | b'{' | b'}' => Ok(start + 1),
        b'/' => Ok(word_end(s, start + 1)),
        _ => {
            let end = word_end(s, start);
            let word = &s[start..end];
            let kind = if is_number(word) || matches!(word, b"true" | b"false" | b"null") {
                Raw::Operand
            } else {
                Raw::Op
            };
            return Some(Ok((start..end, kind)));
        }
    };
    Some(end.map(|end| (start..end, Raw::Operand)))
}

/// An array or dictionary from `start`, with all it holds: where it
/// ends. A closing delimiter that doesn't close what's open is passed
/// over.
fn nested(s: &[u8], start: usize, cap: usize) -> Result<usize, Error> {
    // Where each array or dictionary still open starts, and whether it's
    // a dictionary.
    let mut open: Vec<(usize, bool)> = Vec::new();
    let mut at = start;
    loop {
        let Some(&c) = s.get(at) else {
            let (at, dict) = open.last().copied().unwrap_or((start, false));
            let kind = if dict {
                ErrorKind::Dict
            } else {
                ErrorKind::Array
            };
            return Err(Error { at, kind });
        };
        let pair = |second: u8| c == second && s.get(at + 1) == Some(&second);
        let closes = match c {
            b'[' | b'<' if c == b'[' || pair(b'<') => {
                if open.len() >= cap {
                    return Err(Error {
                        at,
                        kind: ErrorKind::Depth,
                    });
                }
                open.push((at, c == b'<'));
                at += 1 + usize::from(c == b'<');
                continue;
            }
            b']' => Some(false),
            b'>' if pair(b'>') => Some(true),
            _ => None,
        };
        at = match (c, closes) {
            (_, Some(dict)) => {
                let end = at + 1 + usize::from(dict);
                if open.last().is_some_and(|&(_, d)| d == dict) {
                    open.pop();
                    if open.is_empty() {
                        return Ok(end);
                    }
                }
                end
            }
            (b'(', _) => literal_string(s, at)?,
            (b'<', _) => hex_string(s, at)?,
            (b'%', _) => line_end(s, at),
            _ => at + 1,
        };
    }
}

/// A literal string from its `(`: where it ends, after the `)` that
/// balances it.
pub(crate) fn literal_string(s: &[u8], start: usize) -> Result<usize, Error> {
    let mut depth = 0usize;
    let mut at = start;
    while let Some(&c) = s.get(at) {
        match c {
            b'\\' => at += 1,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(at + 1);
                }
            }
            _ => {}
        }
        at += 1;
    }
    Err(Error {
        at: start,
        kind: ErrorKind::String,
    })
}

/// A hex string from its `<`: where it ends, after its `>`.
pub(crate) fn hex_string(s: &[u8], start: usize) -> Result<usize, Error> {
    find(s, start + 1, b">").map(|end| end + 1).ok_or(Error {
        at: start,
        kind: ErrorKind::HexString,
    })
}

/// The elements of an array token, as written.
pub(crate) fn elements(array: &[u8], depth: usize) -> Vec<&[u8]> {
    let inner = array.strip_prefix(b"[").unwrap_or(array);
    items(inner.strip_suffix(b"]").unwrap_or(inner), depth)
}

/// The tokens of what's inside an array or a dictionary, as written.
fn items(inner: &[u8], depth: usize) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(Ok((r, _))) = next_raw(inner, at, depth) {
        at = r.end;
        out.push(&inner[r]);
    }
    out
}

/// Where RunLength data from `at` ends, past its end-of-data byte (128).
pub(crate) fn run_length_end(s: &[u8], mut at: usize) -> Option<usize> {
    while let Some(&n) = s.get(at) {
        at += match n {
            0..=127 => usize::from(n) + 2,
            128 => return Some(at + 1),
            _ => 2,
        };
    }
    None
}

/// The first `EI` from `start` with white-space before it and white-space,
/// a delimiter or the end after it: where it is.
pub(crate) fn ei_scan(s: &[u8], start: usize) -> Option<usize> {
    let mut at = start;
    loop {
        let i = find(s, at, b"EI")?;
        let before = i > 0 && is_space(s[i - 1]);
        let after = s.get(i + 2).is_none_or(|&c| is_space(c) || is_delimiter(c));
        if before && after {
            return Some(i);
        }
        at = i + 1;
    }
}

pub(crate) fn skip_space(s: &[u8], mut at: usize) -> usize {
    while let Some(&c) = s.get(at) {
        if c == b'%' {
            at = line_end(s, at);
        } else if is_space(c) {
            at += 1;
        } else {
            break;
        }
    }
    at
}

/// Where the line from `at` ends: its end-of-line byte, or the end.
pub(crate) fn line_end(s: &[u8], at: usize) -> usize {
    s[at..]
        .iter()
        .position(|&c| c == b'\r' || c == b'\n')
        .map_or(s.len(), |n| at + n)
}

pub(crate) fn word_end(s: &[u8], at: usize) -> usize {
    s[at..]
        .iter()
        .position(|&c| is_space(c) || is_delimiter(c))
        .map_or(s.len(), |n| at + n)
}

/// Where `needle` first is in `s` from `at`.
pub(crate) fn find(s: &[u8], at: usize, needle: &[u8]) -> Option<usize> {
    s.get(at..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|n| at + n)
}

/// A non-negative number, as PDFium reads an integer from one: a real's
/// whole part.
pub(crate) fn int(number: &[u8]) -> Option<u64> {
    if !is_number(number) {
        return None;
    }
    let value: f64 = std::str::from_utf8(number).ok()?.parse().ok()?;
    (0.0..4_294_967_296.0)
        .contains(&value)
        .then_some(value as u64)
}

/// Whether a word is a number, as PDFium tells one: digits, signs and
/// points only.
pub(crate) fn is_number(word: &[u8]) -> bool {
    word.iter()
        .all(|c| matches!(c, b'0'..=b'9' | b'+' | b'-' | b'.'))
}

pub(crate) fn is_space(c: u8) -> bool {
    matches!(c, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

fn is_delimiter(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}
