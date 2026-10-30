//! What a token is and where it ends, as PDFium's `CPDF_StreamParser`
//! ends them: pdf-tokenizer's tokens, read by PDFium's rules
//! ([`Pdfium`]), white-space and comments passed over, a word of digits,
//! signs and points a number, strings nested, an array or a dictionary one
//! token with all it holds, and a closing delimiter with nothing to close a
//! token of its own. Content's tokens come from here: the rest of the
//! crate takes the ranges it cuts, or the tokenizer it makes.

use std::ops::Range;

use pdf_tokenizer::{Kind, Pdfium, Token, Tokenizer};

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

/// The tokens of `s` from `at`, by PDFium's rules.
pub(crate) fn tokenizer(s: &[u8], at: usize) -> Tokenizer<'_, Pdfium> {
    Tokenizer::at(s, at).with_rules(Pdfium)
}

/// The first token from `at` that isn't white-space or a comment; None at
/// the end.
pub(crate) fn significant(s: &[u8], at: usize) -> Option<Token<'_>> {
    tokenizer(s, at).find(|t| !matches!(t.kind, Kind::Space | Kind::Comment))
}

/// Where the first token from `at` that isn't white-space or a comment
/// starts, or the end.
pub(crate) fn skip_space(s: &[u8], at: usize) -> usize {
    significant(s, at).map_or(s.len().max(at), |t| t.start)
}

/// The token from `from` on, past white-space and comments; None at the
/// end.
pub(crate) fn next_raw(
    s: &[u8],
    from: usize,
    depth: usize,
) -> Option<Result<(Range<usize>, Raw), Error>> {
    let mut tokens = tokenizer(s, from);
    let token = tokens.find(|t| !matches!(t.kind, Kind::Space | Kind::Comment))?;
    let error = |kind| {
        Some(Err(Error {
            at: token.start,
            kind,
        }))
    };
    let end = match token.kind {
        Kind::Keyword if !matches!(token.bytes, b"true" | b"false" | b"null") => {
            return Some(Ok((token.range(), Raw::Op)));
        }
        Kind::String { closed: false } => return error(ErrorKind::String),
        Kind::HexString { closed: false } => return error(ErrorKind::HexString),
        Kind::ArrayOpen | Kind::DictOpen => match nested(tokens, token, depth) {
            Ok(end) => end,
            Err(e) => return Some(Err(e)),
        },
        // A number, `true`, `false` or `null`, a name, a string, a closer
        // with nothing to close or a brace.
        _ => token.end(),
    };
    Some(Ok((token.start..end, Raw::Operand)))
}

/// An array or dictionary from `open`, its `[` or `<<`, with all it holds,
/// `tokens` reading on after it: where it ends. A closing delimiter that
/// doesn't close what's open is passed over.
fn nested(mut tokens: Tokenizer<'_, Pdfium>, open: Token<'_>, cap: usize) -> Result<usize, Error> {
    // Where each array or dictionary still open starts, and whether it's
    // a dictionary.
    let mut open_at: Vec<(usize, bool)> = Vec::new();
    let mut token = open;
    loop {
        match token.kind {
            Kind::ArrayOpen | Kind::DictOpen => {
                if open_at.len() >= cap {
                    return Err(Error {
                        at: token.start,
                        kind: ErrorKind::Depth,
                    });
                }
                open_at.push((token.start, token.kind == Kind::DictOpen));
            }
            Kind::ArrayClose | Kind::DictClose => {
                let dict = token.kind == Kind::DictClose;
                if open_at.last().is_some_and(|&(_, d)| d == dict) {
                    open_at.pop();
                    if open_at.is_empty() {
                        return Ok(token.end());
                    }
                }
            }
            Kind::String { closed: false } => {
                return Err(Error {
                    at: token.start,
                    kind: ErrorKind::String,
                });
            }
            Kind::HexString { closed: false } => {
                return Err(Error {
                    at: token.start,
                    kind: ErrorKind::HexString,
                });
            }
            _ => {}
        }
        let Some(next) = tokens.next() else {
            let (at, dict) = open_at.last().copied().unwrap_or((open.start, false));
            let kind = if dict {
                ErrorKind::Dict
            } else {
                ErrorKind::Array
            };
            return Err(Error { at, kind });
        };
        token = next;
    }
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

/// Whether a word is a number, as PDFium tells one: digits, signs and
/// points only, an integer or a real by PDFium's rules.
pub(crate) fn is_number(word: &[u8]) -> bool {
    tokenizer(word, 0).next().is_some_and(|t| {
        matches!(t.kind, Kind::Integer | Kind::Real) && t.bytes.len() == word.len()
    })
}

/// Whether `c` is white-space by PDFium's rules: ISO 32000-2's six bytes
/// (7.2.3), and 0x80 and 0xFF.
pub(crate) fn is_space(c: u8) -> bool {
    pdf_tokenizer::Rules::Pdfium.is_white_space(c)
}
