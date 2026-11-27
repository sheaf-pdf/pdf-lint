//! A document's bytes as a [`RenderBackend`](crate::RenderBackend) opens
//! them without one whole-file buffer: read by position ([`RenderSource`]),
//! or held as several shared buffers read one after another ([`Parts`]).

use std::borrow::Cow;
use std::fmt;
use std::io;
use std::ops::Range;
use std::sync::Arc;

/// A document's bytes, read by position: what
/// [`RenderBackend::open_source`](crate::RenderBackend::open_source) opens.
///
/// An engine keeps the source for as long as the document it opened lives and
/// reads from it on any thread, so a source is `Send + Sync` and its bytes
/// must not change while it's held. Bytes in memory (`[u8]`, `Vec<u8>`,
/// `Arc<[u8]>`) and [`Parts`] are sources already; a file read by position,
/// or a host's own segmented buffer, implements the trait.
///
/// ```
/// use std::sync::Arc;
/// use pdf_raster::{Parts, RenderSource};
///
/// let head: Arc<[u8]> = Arc::from(&b"%PDF-1.7\n"[..]);
/// let tail: Arc<[u8]> = Arc::from(&b"%%EOF\n"[..]);
/// let parts = Parts::new(vec![head, tail]);
/// assert_eq!(parts.len(), 15);
/// // A read within one part borrows it; one across two is copied.
/// assert_eq!(&*parts.read_at(5..8).unwrap(), b"1.7");
/// assert_eq!(&*parts.read_at(7..11).unwrap(), b"7\n%%");
/// assert!(parts.read_at(10..16).is_err());
/// ```
pub trait RenderSource: Send + Sync {
    /// How many bytes it has.
    #[must_use]
    fn len(&self) -> usize;

    /// Whether it has none.
    #[must_use]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every byte, when it holds them in one buffer in memory: an engine
    /// then reads them where they are. `None`, by default, for a source that
    /// reads them from elsewhere or holds them in pieces.
    #[must_use]
    fn whole(&self) -> Option<&[u8]> {
        None
    }

    /// The bytes in `range`, within `0..len()`: all of them, or an error.
    ///
    /// # Errors
    ///
    /// The source's own error where it doesn't give them all, or
    /// [`io::ErrorKind::UnexpectedEof`] where `range` runs past its bytes.
    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>>;
}

/// `bytes[range]`, or the error a read past them is.
fn within(bytes: &[u8], range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
    let (start, end, len) = (range.start, range.end, bytes.len());
    bytes
        .get(range)
        .map(Cow::Borrowed)
        .ok_or_else(|| past(start, end, len))
}

/// What a read of bytes `start..end` of `len` is, which runs past them.
fn past(start: usize, end: usize, len: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        format!("bytes {start}..{end} aren't within the {len} there are"),
    )
}

impl RenderSource for [u8] {
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn whole(&self) -> Option<&[u8]> {
        Some(self)
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        within(self, range)
    }
}

impl RenderSource for Vec<u8> {
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn whole(&self) -> Option<&[u8]> {
        Some(self)
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        within(self, range)
    }
}

/// A source behind a pointer, `$t`, reads as the source does.
macro_rules! forward {
    ($($t:ty),*) => {$(
        impl<S: RenderSource + ?Sized> RenderSource for $t {
            fn len(&self) -> usize {
                (**self).len()
            }

            fn whole(&self) -> Option<&[u8]> {
                (**self).whole()
            }

            fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
                (**self).read_at(range)
            }
        }
    )*};
}

forward!(Box<S>, Arc<S>);

/// Shared buffers read one after another as one document, with no copy of
/// any: a file's bytes and what was written after them, say, each held by
/// its own `Arc<[u8]>`.
///
/// A read within one buffer borrows it; a read across a boundary copies just
/// the bytes asked for. Empty buffers are dropped. One buffer alone is read
/// as bytes in memory ([`RenderSource::whole`]).
///
/// `Debug` prints the number of buffers and their total length, never bytes.
#[derive(Clone, Default)]
pub struct Parts {
    parts: Vec<Arc<[u8]>>,
    /// Where each part starts in the whole.
    starts: Vec<usize>,
    len: usize,
}

impl fmt::Debug for Parts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Parts")
            .field("parts", &self.parts.len())
            .field("len", &self.len)
            .finish()
    }
}

impl Parts {
    /// `parts` in order, as one document's bytes.
    ///
    /// # Panics
    ///
    /// Where their lengths come to more than `usize::MAX`, which buffers
    /// held in memory can't.
    #[must_use]
    pub fn new(parts: Vec<Arc<[u8]>>) -> Self {
        let parts: Vec<Arc<[u8]>> = parts.into_iter().filter(|p| !p.is_empty()).collect();
        let mut starts = Vec::with_capacity(parts.len());
        let mut len = 0_usize;
        for part in &parts {
            starts.push(len);
            len = len
                .checked_add(part.len())
                .expect("buffers in memory come to no more than usize::MAX bytes");
        }
        Self { parts, starts, len }
    }

    /// The buffers, in order, the empty ones dropped.
    #[must_use]
    pub fn parts(&self) -> &[Arc<[u8]>] {
        &self.parts
    }
}

impl From<Vec<Arc<[u8]>>> for Parts {
    fn from(parts: Vec<Arc<[u8]>>) -> Self {
        Self::new(parts)
    }
}

impl RenderSource for Parts {
    fn len(&self) -> usize {
        self.len
    }

    fn whole(&self) -> Option<&[u8]> {
        match self.parts.as_slice() {
            [] => Some(&[]),
            [one] => Some(one),
            _ => None,
        }
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        let Range { start, end } = range;
        if start > end || end > self.len {
            return Err(past(start, end, self.len));
        }
        if start == end {
            return Ok(Cow::Borrowed(&[]));
        }
        // The part `start` is in: the last that starts at or before it.
        let mut i = self.starts.partition_point(|&s| s <= start) - 1;
        let first = &self.parts[i][start - self.starts[i]..];
        if first.len() >= end - start {
            return Ok(Cow::Borrowed(&first[..end - start]));
        }
        let mut out = Vec::with_capacity(end - start);
        out.extend_from_slice(first);
        while out.len() < end - start {
            i += 1;
            let part = &self.parts[i];
            let take = part.len().min(end - start - out.len());
            out.extend_from_slice(&part[..take]);
        }
        Ok(Cow::Owned(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arc(bytes: &[u8]) -> Arc<[u8]> {
        Arc::from(bytes)
    }

    #[test]
    fn parts_read_as_their_bytes_joined() {
        let whole: Vec<u8> = (0..=255).collect();
        // Every split into three (empty parts among them) reads every range
        // as the joined bytes do.
        for a in [0, 1, 7, 128, 255, 256] {
            for b in [a, a + 1, 200, 256] {
                let b = b.clamp(a, 256);
                let parts = Parts::new(vec![arc(&whole[..a]), arc(&whole[a..b]), arc(&whole[b..])]);
                assert_eq!(RenderSource::len(&parts), 256);
                for start in [
                    0,
                    1,
                    a.saturating_sub(1),
                    a,
                    b.saturating_sub(1),
                    b,
                    255,
                    256,
                ] {
                    for end in [start, start + 1, a, b, a + 3, 256] {
                        let end = end.clamp(start, 256);
                        let got = parts.read_at(start..end).unwrap();
                        assert_eq!(&*got, &whole[start..end], "{a}/{b}: {start}..{end}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_read_within_one_part_borrows_and_across_two_copies() {
        let parts = Parts::new(vec![arc(b"abcd"), arc(b""), arc(b"efgh")]);
        assert_eq!(parts.parts().len(), 2);
        assert!(matches!(
            parts.read_at(1..4).unwrap(),
            Cow::Borrowed(b"bcd")
        ));
        assert!(matches!(
            parts.read_at(4..8).unwrap(),
            Cow::Borrowed(b"efgh")
        ));
        assert!(matches!(parts.read_at(3..5).unwrap(), Cow::Owned(v) if v == b"de"));
        assert!(matches!(parts.read_at(8..8).unwrap(), Cow::Borrowed(b"")));
        assert!(parts.whole().is_none());
    }

    #[test]
    fn a_read_past_the_end_is_an_error_not_a_short_read() {
        let parts = Parts::new(vec![arc(b"abcd"), arc(b"efgh")]);
        for range in [0..9, 8..9, 9..9, 7..usize::MAX] {
            let err = parts.read_at(range).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        }
        #[allow(clippy::reversed_empty_ranges)]
        let backwards = parts.read_at(5..2);
        assert!(backwards.is_err());
        assert!(b"abcd"[..].read_at(2..5).is_err());
        assert!(vec![1_u8, 2].read_at(0..3).is_err());
    }

    #[test]
    fn one_part_or_none_is_bytes_in_memory() {
        assert_eq!(
            Parts::new(vec![arc(b"abcd"), arc(b"")]).whole(),
            Some(&b"abcd"[..])
        );
        let none = Parts::default();
        assert!(none.is_empty());
        assert_eq!(none.whole(), Some(&[][..]));
        assert_eq!(&*none.read_at(0..0).unwrap(), b"");
        // Behind a pointer a source reads as itself.
        let boxed: Box<dyn RenderSource> = Box::new(arc(b"abcd"));
        assert_eq!(boxed.whole(), Some(&b"abcd"[..]));
        assert_eq!(&*boxed.read_at(1..3).unwrap(), b"bc");
    }

    #[test]
    fn debug_gives_counts_not_bytes() {
        let parts = Parts::new(vec![arc(b"secret-bytes"), arc(b"more")]);
        let words = format!("{parts:?}");
        assert_eq!(words, "Parts { parts: 2, len: 16 }");
    }
}
