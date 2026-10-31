//! Where an inline image's data ends, where that's found in its bytes and
//! not in tokens: an end-of-data marker its filter writes, a JPEG's end of
//! image, or an `EI` between white-space.

use memchr::memmem;
use pdf_tokenizer::{is_delimiter, is_white_space};

/// Where RunLength data ends in one content stream, from any place in it:
/// past its end-of-data byte (128). Each place a walk that found no end
/// has passed is remembered, as a walk from it finds none either: content
/// of many inline images whose data doesn't end walks each byte once, not
/// once for each image.
#[derive(Debug, Clone, Default)]
pub(crate) struct RunLength {
    /// A bit for each place in the content, set where a walk from it
    /// finds no end; none until one doesn't.
    endless: Vec<u64>,
}

impl RunLength {
    /// Where RunLength data from `at` in `s`, the content stream it's
    /// always given, ends: past its end-of-data byte. None when the end of
    /// `s` comes first.
    pub(crate) fn end(&mut self, s: &[u8], at: usize) -> Option<usize> {
        let mut to = at;
        while let Some(&n) = s.get(to) {
            if n == 128 {
                return Some(to + 1);
            }
            if self.is_endless(to) {
                break;
            }
            to += run(n);
        }
        // No end from `at`, nor from any place the walk passed.
        if self.endless.is_empty() {
            self.endless = vec![0; s.len().div_ceil(64)];
        }
        let mut at = at;
        while at < to.min(s.len()) {
            self.endless[at / 64] |= 1 << (at % 64);
            at += run(s[at]);
        }
        None
    }

    fn is_endless(&self, at: usize) -> bool {
        self.endless
            .get(at / 64)
            .is_some_and(|w| w >> (at % 64) & 1 != 0)
    }
}

/// The bytes a RunLength run that starts with length byte `n`, not 128,
/// takes: `n` + 1 bytes copied after it, or one byte repeated.
fn run(n: u8) -> usize {
    match n {
        0..=127 => usize::from(n) + 2,
        _ => 2,
    }
}

/// Where ASCII85 data from `at` stops, as PDFium's decoder reads it: at
/// the first byte that's neither one of its digits (`!` to `u`), `z`, nor
/// white-space it passes over (CR, LF, space and tab), `~` among them.
/// None when no such byte comes before the end.
pub(crate) fn a85_stop(s: &[u8], at: usize) -> Option<usize> {
    let passed = |c: &u8| matches!(c, b'!'..=b'u' | b'z' | b'\r' | b'\n' | b' ' | b'\t');
    Some(at + s.get(at..)?.iter().position(|c| !passed(c))?)
}

/// Where ASCII85 data from `at` ends, as PDFium's decoder reads it, the
/// byte it stops at `stop` ([`a85_stop`]): past that byte, and past a `>`
/// straight after; where it starts when that byte is its first.
pub(crate) fn a85_end(s: &[u8], at: usize, stop: usize) -> usize {
    if stop == at {
        return at;
    }
    stop + 1 + usize::from(s.get(stop + 1) == Some(&b'>'))
}

/// The last search of one content stream for the first place from where
/// it began that holds what's searched for: where it began, and where it
/// found it. Content of many inline images whose data doesn't end is
/// searched once, not once for each image.
#[derive(Debug, Clone, Default)]
pub(crate) struct Searched(Option<(usize, Option<usize>)>);

impl Searched {
    /// The first place from `at` that `search` finds, where whether a
    /// place is one is told by what's at it alone: the last search's
    /// answer, where it began at or before `at` and found none, or found
    /// one at `at` or after.
    pub(crate) fn from(
        &mut self,
        at: usize,
        search: impl FnOnce(usize) -> Option<usize>,
    ) -> Option<usize> {
        if let Some((from, found)) = self.0 {
            if at >= from && found.is_none_or(|f| f >= at) {
                return found;
            }
        }
        let found = search(at);
        self.0 = Some((at, found));
        found
    }
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
