//! Where an inline image's data ends, where that's found in its bytes and
//! not in tokens: an end-of-data marker its filter writes, a JPEG's end of
//! image, or an `EI` between white-space.

use memchr::memmem;
use pdf_tokenizer::{is_delimiter, is_white_space};

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

/// Where ASCII85 data from `at` ends, as PDFium's decoder reads it: past
/// the first byte that's neither one of its digits (`!` to `u`), `z`, nor
/// white-space it passes over (CR, LF, space and tab), `~` among them, and
/// past a `>` straight after; where it starts when that byte is its first.
/// None when no such byte comes before the end.
pub(crate) fn a85_end(s: &[u8], at: usize) -> Option<usize> {
    let passed = |c: &u8| matches!(c, b'!'..=b'u' | b'z' | b'\r' | b'\n' | b' ' | b'\t');
    let first = at + s.get(at..)?.iter().position(|c| !passed(c))?;
    if first == at {
        return Some(at);
    }
    Some(first + 1 + usize::from(s.get(first + 1) == Some(&b'>')))
}

/// The first `EI` from `start` with white-space before it and white-space,
/// a delimiter or the end after it: where it is. White-space is ISO
/// 32000-2's six bytes (7.2.3).
pub(crate) fn ei_scan(s: &[u8], start: usize) -> Option<usize> {
    let mut at = start;
    loop {
        let i = find(s, at, b"EI")?;
        let before = i > 0 && is_white_space(s[i - 1]);
        let after = s
            .get(i + 2)
            .is_none_or(|&c| is_white_space(c) || is_delimiter(c));
        if before && after {
            return Some(i);
        }
        at = i + 1;
    }
}

/// Where `needle` first is in `s` from `at`.
pub(crate) fn find(s: &[u8], at: usize, needle: &[u8]) -> Option<usize> {
    memmem::find(s.get(at..)?, needle).map(|n| at + n)
}
