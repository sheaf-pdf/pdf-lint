//! A content stream's operations (ISO 32000-2, 7.8.2): each operator, or
//! inline image, with the operands before it, their ranges held in the
//! item itself.

use std::fmt;
use std::iter::FusedIterator;
use std::ops::Range;

use tinyvec::TinyVec;

use crate::error::Error;
use crate::inline::{InlineImage, ResCx};
use crate::limits::Limits;
use crate::operand::Operand;
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
    Operations {
        tokens: tokens(s, res, limits),
    }
}

/// A content stream's operations, in order ([`operations`]).
#[derive(Debug, Clone)]
pub struct Operations<'a, 'r> {
    tokens: Tokens<'a, 'r>,
}

impl<'a> Iterator for Operations<'a, '_> {
    type Item = Result<Operation<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut operands = Ranges::new();
        loop {
            let (at, tok) = match self.tokens.next()? {
                Ok(token) => token,
                Err(e) => return Some(Err(e)),
            };
            let image = match tok {
                Tok::Operand(_) => {
                    operands.push(at);
                    continue;
                }
                Tok::Op(_) => None,
                Tok::InlineImage(data) => Some(data),
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

impl FusedIterator for Operations<'_, '_> {}

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
    /// Where an inline image's data is.
    image: Option<Range<usize>>,
    operands: Ranges,
}

impl<'a> Operation<'a> {
    /// The operator, as written: `BI` for an inline image.
    pub fn name(&self) -> &'a [u8] {
        let end = match self.image {
            Some(_) => self.at.start + 2,
            None => self.at.end,
        };
        &self.content[self.at.start..end]
    }

    /// Where the operator is, or the inline image, `BI` to `EI`.
    pub fn at(&self) -> Range<usize> {
        self.at.clone()
    }

    /// Where the operation is: from its first operand, or its operator
    /// when it has none, to its operator's end.
    pub fn range(&self) -> Range<usize> {
        self.operands.first().map_or(self.at.start, |r| r.start)..self.at.end
    }

    /// Its operands, in order, as written.
    pub fn operands(&self) -> impl ExactSizeIterator<Item = Operand<'a>> + DoubleEndedIterator {
        let content = self.content;
        self.operands
            .iter()
            .map(move |r| Operand::new(content, r.clone()))
    }

    /// Operand `i` from the last, as PDFium takes them: 0 is the last.
    /// None past the first. Operands before those an operator takes are
    /// passed over this way, as PDFium passes them over.
    pub fn arg(&self, i: usize) -> Option<Operand<'a>> {
        let last = self.operands.len().checked_sub(1)?;
        let r = self.operands.get(last.checked_sub(i)?)?;
        Some(Operand::new(self.content, r.clone()))
    }

    /// The inline image it is, when it's one: its dictionary's entries,
    /// its data and the length it gives it.
    pub fn inline_image(&self) -> Option<InlineImage<'a>> {
        let data = self.image.clone()?;
        Some(InlineImage::new(self.content, self.at.clone(), data))
    }

    /// Whether its operands are held in the item, not spilled to the heap.
    #[cfg(test)]
    pub(crate) fn inline(&self) -> bool {
        self.operands.is_inline()
    }
}

impl fmt::Debug for Operation<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Operation")
            .field("name", &String::from_utf8_lossy(self.name()))
            .field("at", &self.at)
            .field("image", &self.image)
            .field("operands", &self.operands.as_slice())
            .finish()
    }
}
