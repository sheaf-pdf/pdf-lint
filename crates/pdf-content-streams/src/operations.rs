//! A content stream's operations (ISO 32000-2, 7.8.2): each operator, or
//! inline image, with the operands before it, their ranges held in the
//! item itself.

use std::fmt;
use std::iter::FusedIterator;
use std::ops::Range;

use tinyvec::TinyVec;

use crate::error::Error;
use crate::fault::{self, Fault, Sink};
use crate::inline::{Found, InlineImage, ResCx};
use crate::iter;
use crate::limits::Limits;
use crate::operand::Operand;
use crate::operator::Operator;
use crate::tokens::{Tok, Tokens, tokens};

/// Where an operation's operands are: the first sixteen held inline, as
/// many as PDFium keeps, so an operation of no more allocates nothing;
/// the rest, with them, on the heap.
type Ranges = TinyVec<[Range<usize>; 16]>;

/// The operations of `s`, in order: each operator, or inline image, with
/// the operands before it ([`Operation`]). They're its [`tokens`], read
/// under `limits`, grouped: operands at the end, which no operator takes,
/// are no operation. After an error there's nothing more.
///
/// Every operand is kept, up to [`Limits::operands`], past which is the
/// error [`tokens`] gives, where the first past it starts: PDFium keeps
/// the last sixteen, which are [`Operation::arg`]`(0)` to `arg(15)`, and
/// a reader that counts them, or reads a color with more components,
/// has them all.
///
/// Where ISO 32000-2 would read the bytes otherwise, a sink given by
/// [`Operations::with_sink`] is told ([`fault`]); the operations are the
/// same.
///
/// ```
/// use pdf_content_streams::{Limits, ResCx, operations};
///
/// let content = b"q 1 0 0 1 72 720 cm /F1 12 Tf Q 3";
/// let read: Vec<_> = operations(content, &ResCx::none(), &Limits::DEFAULT)
///     .collect::<Result<_, _>>()
///     .unwrap();
/// let names: Vec<_> = read.iter().map(|op| op.name()).collect();
/// // The `3` at the end is no operation's.
/// assert_eq!(names, [&b"q"[..], b"cm", b"Tf", b"Q"]);
/// let cm = &read[1];
/// assert_eq!((cm.at(), cm.range()), (17..19, 2..19));
/// assert_eq!(cm.operands().len(), 6);
/// assert_eq!(cm.arg(0).unwrap().bytes(), b"720");
/// ```
pub fn operations<'a, 'r>(s: &'a [u8], res: &'r ResCx, limits: &Limits) -> Operations<'a, 'r> {
    Operations::new(tokens(s, res, limits))
}

/// A content stream's operations, in order ([`operations`]), and where
/// ISO 32000-2 would read them otherwise told to `S`, a
/// [`Sink`]: none for `()`.
#[must_use = "operations are lazy: nothing is read until one is asked for"]
#[derive(Clone)]
pub struct Operations<'a, 'r, S = ()> {
    tokens: Tokens<'a, 'r, S>,
}

impl<'a, 'r, S> Operations<'a, 'r, S> {
    /// The operations of `tokens`.
    pub(crate) fn new(tokens: Tokens<'a, 'r, S>) -> Operations<'a, 'r, S> {
        Operations { tokens }
    }

    /// The same operations, from where these are, with where ISO 32000-2
    /// would read them otherwise told to `sink` ([`fault`]): what
    /// [`Tokens::with_sink`] tells, and operands at the end that no
    /// operator takes. A sink borrowed, `&mut S`, is one, so it can be read
    /// after.
    ///
    /// ```
    /// use pdf_content_streams::fault::Fault;
    /// use pdf_content_streams::{Limits, ResCx, operations};
    ///
    /// let content = b"q 0 0 m 9 9 l S Q 1 2";
    /// let mut kept = Vec::new();
    /// let res = ResCx::none();
    /// let read = operations(content, &res, &Limits::DEFAULT);
    /// let read = read.with_sink(&mut kept);
    /// assert_eq!(read.count(), 5);
    /// assert_eq!(kept, [(18..21, Fault::Trailing { count: 2 })]);
    /// assert_eq!(
    ///     kept[0].1.to_string(),
    ///     "2 operands at the end that no operator takes"
    /// );
    /// ```
    pub fn with_sink<T: Sink<'a>>(self, sink: T) -> Operations<'a, 'r, T> {
        Operations {
            tokens: self.tokens.with_sink(sink),
        }
    }
}

impl<S> fmt::Debug for Operations<'_, '_, S> {
    /// Its tokens', which print no content.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Operations")
            .field("tokens", &self.tokens)
            .finish()
    }
}

impl<'a, S: Sink<'a>> Iterator for Operations<'a, '_, S> {
    type Item = Result<Operation<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut operands = Ranges::new();
        loop {
            let Some(token) = self.tokens.step() else {
                self.trailing(&operands);
                self.tokens.finish();
                return None;
            };
            let (at, tok) = match token {
                Ok(token) => token,
                Err(e) => return Some(Err(e)),
            };
            let image = match tok {
                Tok::Operand(_) => {
                    operands.push(at);
                    continue;
                }
                Tok::Op(_) => None,
                Tok::InlineImage(data) => Some((data, self.tokens.found(), self.tokens.capped())),
            };
            return Some(Ok(Operation {
                content: self.tokens.content(),
                at,
                image,
                operands,
            }));
        }
    }
}

impl<'a, S: Sink<'a>> FusedIterator for Operations<'a, '_, S> {}

impl<'a, S: Sink<'a>> Operations<'a, '_, S> {
    /// `operands`, at the end, no operator's: told, if there are any.
    fn trailing(&mut self, operands: &Ranges) {
        let sink = self.tokens.sink();
        if !sink.wants() {
            return;
        }
        if let (Some(first), Some(last)) = (operands.first(), operands.last()) {
            let count = operands.len();
            sink.fault(first.start..last.end, Fault::Trailing { count });
        }
    }
}

/// An operator, or an inline image, and the operands before it: an
/// operation of a content stream ([`operations`]). Offsets are into the
/// content read.
///
/// Operands are as written, each read only when asked ([`Operand`]).
/// They're counted by [`operands`](Operation::operands)' `len`, and
/// taken from the last by [`arg`](Operation::arg), as PDFium takes them.
#[derive(Clone)]
pub struct Operation<'a> {
    content: &'a [u8],
    /// Where the operator is, or the inline image, `BI` to `EI`.
    at: Range<usize>,
    /// Where an inline image's data is, how its end was found, and
    /// whether its size was counted past a cap.
    image: Option<(Range<usize>, Found, bool)>,
    operands: Ranges,
}

impl<'a> Operation<'a> {
    /// The operator, as written: `BI` for an inline image, and for a `BI`
    /// that begins none ([`inline_image`](Operation::inline_image)).
    #[must_use]
    pub fn name(&self) -> &'a [u8] {
        let end = match self.image {
            Some(_) => self.at.start + 2,
            None => self.at.end,
        };
        &self.content[self.at.start..end]
    }

    /// Where the operator is, or the inline image, `BI` to `EI`.
    #[must_use]
    pub fn at(&self) -> Range<usize> {
        self.at.clone()
    }

    /// Where the operation is: from its first operand, or its operator
    /// when it has none, to its operator's end.
    #[must_use]
    pub fn range(&self) -> Range<usize> {
        self.operands.first().map_or(self.at.start, |r| r.start)..self.at.end
    }

    /// Its operands, in order, as written.
    pub fn operands(&self) -> iter::Operands<'a, '_> {
        iter::Operands::new(self.content, &self.operands)
    }

    /// Operand `i` from the last, as PDFium takes them: 0 is the last.
    /// None past the first. Operands before those an operator takes are
    /// passed over this way, as PDFium passes them over.
    #[must_use]
    pub fn arg(&self, i: usize) -> Option<Operand<'a>> {
        let last = self.operands.len().checked_sub(1)?;
        let r = self.operands.get(last.checked_sub(i)?)?;
        Some(Operand::new(self.content, r.clone()))
    }

    /// The inline image it is, when it's one: its dictionary's entries,
    /// its data and the length it gives it. None for a `BI` that begins no
    /// image, a keyword written where its dictionary's key should be, as
    /// PDFium reads it ([`tokens`](fn@crate::tokens)): an operation of its
    /// own, with the operands before it, as PDFium clears them after it.
    #[must_use]
    pub fn inline_image(&self) -> Option<InlineImage<'a>> {
        let (data, found, capped) = self.image.clone()?;
        Some(InlineImage::new(
            self.content,
            self.at.clone(),
            data,
            found,
            capped,
        ))
    }

    /// Its operator, by its name ([`Operator::of`]): an inline image's is
    /// `BI`'s, and a word ISO 32000-2's Annex A doesn't name is
    /// [`Operator::Other`]. Where it may be used is
    /// [`State`](crate::State)'s to say ([`State::step`](crate::State::step)).
    #[must_use]
    pub fn operator(&self) -> Operator {
        Operator::of(self.name())
    }

    /// Whether it's an inline image.
    pub(crate) fn is_inline_image(&self) -> bool {
        self.image.is_some()
    }

    /// Whether its operands are held in the item, not spilled to the heap.
    #[cfg(test)]
    pub(crate) fn inline(&self) -> bool {
        self.operands.is_inline()
    }
}

impl fmt::Debug for Operation<'_> {
    /// Its operator's name, as a fault's text shows a word, and where it
    /// is and its operands are; not its operands' bytes, which are
    /// content.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Operation")
            .field("name", &fault::shown(self.name()))
            .field("at", &self.at)
            .field("image", &self.image.as_ref().map(|(data, ..)| data))
            .field("operands", &self.operands.as_slice())
            .finish()
    }
}

/// An operation read, or the error reading it gave, as an operation's
/// `Debug` wrote it before it cut a long name to what a fault's text
/// shows: its name whole, where it is, its image's data and its operands'
/// places. What the tests and the fuzz body compare two readings'
/// operations by.
#[cfg(any(test, fuzzing))]
pub(crate) fn written(op: &Result<Operation<'_>, Error>) -> String {
    match op {
        Ok(op) => format!(
            "Ok(Operation {{ name: {:?}, at: {:?}, image: {:?}, operands: {:?} }})",
            String::from_utf8_lossy(op.name()),
            op.at,
            op.image.as_ref().map(|(data, ..)| data),
            op.operands.as_slice(),
        ),
        Err(e) => format!("Err({e:?})"),
    }
}
