//! A page's content as one: the logical concatenation of its `/Contents`
//! streams. A marked range or a `q` may open in one stream and close in
//! the next, so a page's streams are read as PDFium reads them, joined
//! with a space between each ([`Concat`]), and what's found is mapped back
//! to a stream and a place in it ([`Pos`], [`SpanAcross`]). ISO 32000-2
//! reads an array of streams as one stream with white-space between each
//! (7.7.3.3, Table 31's `/Contents`).
//!
//! The streams are the caller's to read and decode: [`Join`] joins them
//! as they're decoded, under [`Limits::content`], each decoded straight
//! into the joined bytes when the decoder appends to a buffer.

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;

use crate::inline::ResCx;
use crate::limits::Limits;
use crate::operations::Operations;
use crate::tokens::{Tokens, tokens};

/// A place in a page's content: a byte in one of its streams. The end of
/// a stream is the place after its last byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    /// The stream, by its index among the page's: the order they were
    /// given in, an empty one counted.
    pub stream: u32,
    /// The byte in that stream, from its start.
    pub at: u32,
}

/// Part of a page's content, which may start in one stream and end in
/// another: from `start` up to `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanAcross {
    /// Where it starts.
    pub start: Pos,
    /// Where it ends: the place after its last byte.
    pub end: Pos,
}

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "stream {} byte {}", self.stream, self.at)
    }
}

impl fmt::Display for SpanAcross {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} to {}", self.start, self.end)
    }
}

/// Streams as one: their bytes joined, a space between each, as PDFium
/// joins a page's. A token can still run from one into the next, as in
/// PDFium, when a string or an inline image does. Offsets are into the
/// joined bytes. A place in them ([`Pos`]) counts in 32 bits, which a
/// page's content under [`Limits::content`] keeps within; streams of more
/// than 4 GiB together, joined by [`Concat::new`], which has no cap, give
/// places past it as `u32::MAX`.
#[derive(Debug, Clone)]
pub struct Concat<'a> {
    bytes: Cow<'a, [u8]>,
    /// Where each stream starts.
    starts: Box<[usize]>,
}

impl<'a> Concat<'a> {
    /// `streams` as one; one stream is borrowed as it is. There's no cap
    /// here: a page's streams decoded under one are joined by [`Join`].
    pub fn new(streams: &[&'a [u8]]) -> Concat<'a> {
        if let [one] = streams {
            return Concat {
                bytes: Cow::Borrowed(one),
                starts: Box::new([0]),
            };
        }
        let mut join = Join::uncapped();
        for stream in streams {
            // Nothing is past no cap.
            let _ = join.push(stream);
        }
        join.finish()
    }

    /// The streams' bytes, joined.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// How many streams there are.
    pub fn stream_count(&self) -> usize {
        self.starts.len()
    }

    /// Stream `i`'s bytes. Panics if there's no stream `i`.
    pub fn stream(&self, i: usize) -> &[u8] {
        &self.bytes[self.range(i)]
    }

    /// Where stream `i` is in the joined bytes. Panics if there's no
    /// stream `i`.
    pub fn range(&self, i: usize) -> Range<usize> {
        let end = self
            .starts
            .get(i + 1)
            .map_or(self.bytes.len(), |next| next - 1);
        self.starts[i]..end
    }

    /// Where offset `at` is: past a stream's last byte, the space after it
    /// is that stream's end.
    pub fn pos(&self, at: usize) -> Pos {
        let stream = self
            .starts
            .partition_point(|&start| start <= at)
            .saturating_sub(1);
        let start = self.starts.get(stream).copied().unwrap_or(0);
        Pos {
            stream: small(stream),
            at: small(at - start),
        }
    }

    /// Where `pos` is in the joined bytes; None past its stream's end, or
    /// past the last stream.
    pub fn offset(&self, pos: Pos) -> Option<usize> {
        let stream = pos.stream as usize;
        let range = (stream < self.starts.len()).then(|| self.range(stream))?;
        let at = range.start + pos.at as usize;
        (at <= range.end).then_some(at)
    }

    /// Where `span` is in the joined bytes; None when either end is
    /// nowhere, or the end is before the start.
    pub fn offsets(&self, span: SpanAcross) -> Option<Range<usize>> {
        let (start, end) = (self.offset(span.start)?, self.offset(span.end)?);
        (start <= end).then_some(start..end)
    }

    /// Where `range` is.
    pub fn span(&self, range: Range<usize>) -> SpanAcross {
        SpanAcross {
            start: self.pos(range.start),
            end: self.pos(range.end),
        }
    }

    /// The joined bytes' tokens ([`tokens`](crate::tokens)), at offsets
    /// into them. With a sink ([`Tokens::with_sink`]), a string, a
    /// comment or an inline image that runs from one stream into the next
    /// is told too ([`Fault::Across`](crate::fault::Fault::Across)).
    pub fn tokens<'s, 'r>(&'s self, res: &'r ResCx, limits: &Limits) -> Tokens<'s, 'r> {
        // Where each stream after the first starts.
        let joins = self.starts.get(1..).unwrap_or_default();
        tokens(&self.bytes, res, limits).joined(joins)
    }

    /// The joined bytes' operations ([`operations`](crate::operations)),
    /// at offsets into them, which [`pos`](Concat::pos) and
    /// [`span`](Concat::span) map back to the streams. With a sink
    /// ([`Operations::with_sink`]), a string, a comment or an inline image
    /// that runs from one stream into the next is told too
    /// ([`Fault::Across`](crate::fault::Fault::Across)).
    pub fn operations<'s, 'r>(&'s self, res: &'r ResCx, limits: &Limits) -> Operations<'s, 'r> {
        Operations::new(self.tokens(res, limits))
    }
}

fn small(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Streams joined as they're read, a space between each, under
/// [`Limits::content`]: the joined bytes, the spaces among them, are never
/// more. Stream `i` is the `i`th pushed, an empty one counted, as PDFium
/// joins an empty stream too, its space and all, so a [`Pos`]'s stream is
/// the index of the stream the caller gave.
///
/// ```
/// use pdf_content_streams::{Join, Limits, Pos};
///
/// let mut join = Join::new(&Limits::DEFAULT);
/// join.push(b"q 1 0 0 rg").unwrap();
/// // A decoder that appends to a buffer decodes straight into the join.
/// join.push_with(|out, room| {
///     let decoded = b"0 0 9 9 re f Q";
///     if decoded.len() > room {
///         return Err("past the room");
///     }
///     out.extend_from_slice(decoded);
///     Ok(())
/// })
/// .unwrap();
/// let page = join.finish();
/// assert_eq!(page.bytes(), b"q 1 0 0 rg 0 0 9 9 re f Q");
/// assert_eq!(page.pos(13), Pos { stream: 1, at: 2 });
/// ```
#[derive(Debug, Clone)]
pub struct Join {
    bytes: Vec<u8>,
    starts: Vec<usize>,
    cap: usize,
}

impl Join {
    /// None yet, under `limits.content`.
    pub fn new(limits: &Limits) -> Join {
        Join {
            bytes: Vec::new(),
            starts: Vec::new(),
            cap: limits.content,
        }
    }

    fn uncapped() -> Join {
        Join {
            bytes: Vec::new(),
            starts: Vec::new(),
            cap: usize::MAX,
        }
    }

    /// The bytes another stream may add, after the space before it.
    pub fn room(&self) -> usize {
        self.cap.saturating_sub(self.with(0))
    }

    /// `stream` added, a space before it if it isn't the first; refused
    /// past the cap, with nothing added.
    pub fn push(&mut self, stream: &[u8]) -> Result<(), TooLarge> {
        if self.with(stream.len()) > self.cap {
            return Err(self.too_large());
        }
        if !self.starts.is_empty() {
            self.bytes.push(b' ');
        }
        self.starts.push(self.bytes.len());
        self.bytes.extend_from_slice(stream);
        Ok(())
    }

    /// A stream added by `decode`, which appends it to the joined bytes it's
    /// given, as pdf-filters' `decode_into` does, up to the room it's given
    /// ([`Join::room`]); the space before it written first. A stream that
    /// decodes to nothing is a stream, its space and its start kept, as
    /// [`Join::push`] keeps one. Refused past the cap, or when `decode`
    /// fails or writes past its room: the joined bytes then cut back to
    /// where they were, the space too, and no stream added.
    ///
    /// `decode` may only append: the bytes before its room are the
    /// streams joined so far. Panics if it leaves fewer than it was given.
    pub fn push_with<E>(
        &mut self,
        decode: impl FnOnce(&mut Vec<u8>, usize) -> Result<(), E>,
    ) -> Result<(), Pushed<E>> {
        if self.with(0) > self.cap {
            return Err(Pushed::TooLarge(self.too_large()));
        }
        let mark = self.bytes.len();
        if !self.starts.is_empty() {
            self.bytes.push(b' ');
        }
        let start = self.bytes.len();
        let decoded = decode(&mut self.bytes, self.cap - start);
        assert!(
            self.bytes.len() >= start,
            "a decoder given to Join::push_with cut into the streams joined before it"
        );
        let refused = match decoded {
            Err(e) => Pushed::Decode(e),
            Ok(()) if self.bytes.len() > self.cap => Pushed::TooLarge(self.too_large()),
            Ok(()) => {
                self.starts.push(start);
                return Ok(());
            }
        };
        self.bytes.truncate(mark);
        Err(refused)
    }

    /// The streams joined.
    pub fn finish(self) -> Concat<'static> {
        Concat {
            bytes: Cow::Owned(self.bytes),
            starts: self.starts.into(),
        }
    }

    /// What `len` more bytes would make the joined bytes, a space before
    /// them.
    fn with(&self, len: usize) -> usize {
        self.bytes
            .len()
            .saturating_add(usize::from(!self.starts.is_empty()))
            .saturating_add(len)
    }

    fn too_large(&self) -> TooLarge {
        TooLarge { cap: self.cap }
    }
}

/// Streams joined past [`Limits::content`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[error("its streams decode to more than {cap} bytes")]
pub struct TooLarge {
    /// The cap: the most the joined bytes may be.
    pub cap: usize,
}

/// Why [`Join::push_with`] added no stream.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Pushed<E> {
    /// The stream, or the space before it, past the cap.
    #[error("{0}")]
    TooLarge(TooLarge),
    /// The decoder's error.
    #[error("{0}")]
    Decode(E),
}
