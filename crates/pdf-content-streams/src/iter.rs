//! The iterators the crate's methods give, each a type of its own, so a
//! caller can hold one in a field or name it in a signature: an
//! operation's operands ([`Operands`]), a set's operators ([`Operators`]),
//! the names a cap was met for ([`Capped`]), resource names
//! ([`NamesOf`], [`Names`], [`ReservedNames`]), property lists
//! ([`Lists`]) and marked ranges still open ([`Open`]).
//!
//! Each is fused, and gives the hints of what it walks, so a loop over
//! one, or a `count`, `for_each` or `fold`, walks that alone, in one pass.
//! None prints a byte of content in its `Debug`: an operation's operands
//! print where they are, and property lists how many are left.

use std::collections::{btree_map, btree_set, hash_map};
use std::fmt;
use std::iter::FusedIterator;
use std::ops::Range;
use std::slice;

use pdf_objects::Dict;

use crate::names::ResKind;
use crate::operand::Operand;
use crate::operator::Operator;
use crate::res::Res;

/// An operation's operands, in order, as written: what
/// [`Operation::operands`](crate::Operation::operands) gives. It knows
/// how many are left, and walks from either end.
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Clone)]
pub struct Operands<'a, 'o> {
    content: &'a [u8],
    ranges: slice::Iter<'o, Range<usize>>,
}

impl<'a, 'o> Operands<'a, 'o> {
    /// The operands at `ranges` in `content`.
    pub(crate) fn new(content: &'a [u8], ranges: &'o [Range<usize>]) -> Operands<'a, 'o> {
        Operands {
            content,
            ranges: ranges.iter(),
        }
    }
}

impl<'a> Iterator for Operands<'a, '_> {
    type Item = Operand<'a>;

    #[inline]
    fn next(&mut self) -> Option<Operand<'a>> {
        let content = self.content;
        (self.ranges.next()).map(|r| Operand::new(content, r.clone()))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.ranges.size_hint()
    }

    /// By the ranges' own, as `map`'s is.
    #[inline]
    fn fold<B, F: FnMut(B, Operand<'a>) -> B>(self, init: B, mut f: F) -> B {
        let content = self.content;
        (self.ranges).fold(init, |acc, r| f(acc, Operand::new(content, r.clone())))
    }
}

impl DoubleEndedIterator for Operands<'_, '_> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        let content = self.content;
        (self.ranges.next_back()).map(|r| Operand::new(content, r.clone()))
    }

    /// By the ranges' own, as `map`'s is.
    #[inline]
    fn rfold<B, F: FnMut(B, Self::Item) -> B>(self, init: B, mut f: F) -> B {
        let content = self.content;
        (self.ranges).rfold(init, |acc, r| f(acc, Operand::new(content, r.clone())))
    }
}

impl ExactSizeIterator for Operands<'_, '_> {}

impl FusedIterator for Operands<'_, '_> {}

impl fmt::Debug for Operands<'_, '_> {
    /// Where the operands left are; not their bytes, which are content.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Operands")
            .field("left", &self.ranges.as_slice())
            .finish()
    }
}

/// The operators of a set, in [`Operator::ALL`]'s order,
/// [`Other`](Operator::Other) last: what
/// [`Operators::iter`](crate::Operators::iter) gives. It knows how many
/// are left.
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Clone)]
pub struct Operators {
    /// The set's operators not given yet, each by its bit
    /// ([`crate::Operators`]).
    left: u128,
}

impl Operators {
    /// The operators of `set`.
    pub(crate) const fn new(set: crate::Operators) -> Operators {
        Operators { left: set.bits() }
    }
}

/// The operator whose bit is `i`: its place in [`Operator::ALL`],
/// [`Other`](Operator::Other) past it.
#[inline]
fn operator(i: u32) -> Operator {
    (Operator::ALL.get(i as usize).copied()).unwrap_or(Operator::Other)
}

impl Iterator for Operators {
    type Item = Operator;

    #[inline]
    fn next(&mut self) -> Option<Operator> {
        if self.left == 0 {
            return None;
        }
        let i = self.left.trailing_zeros();
        self.left &= self.left - 1;
        Some(operator(i))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.left.count_ones() as usize;
        (n, Some(n))
    }

    /// One pass over the bits left, lowest first.
    #[inline]
    fn fold<B, F: FnMut(B, Operator) -> B>(self, init: B, mut f: F) -> B {
        let (mut left, mut acc) = (self.left, init);
        while left != 0 {
            acc = f(acc, operator(left.trailing_zeros()));
            left &= left - 1;
        }
        acc
    }
}

impl ExactSizeIterator for Operators {}

impl FusedIterator for Operators {}

impl fmt::Debug for Operators {
    /// The operators left, as a set of them prints.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let left = crate::Operators::from_bits(self.left);
        f.debug_tuple("Operators").field(&left).finish()
    }
}

/// The names a cap was met for, in the order of their bytes: what
/// [`ResCx::capped`](crate::ResCx::capped) gives.
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Debug, Clone)]
pub struct Capped<'r> {
    names: btree_set::Iter<'r, Vec<u8>>,
}

impl<'r> Capped<'r> {
    /// The names of `names`.
    pub(crate) const fn new(names: btree_set::Iter<'r, Vec<u8>>) -> Capped<'r> {
        Capped { names }
    }
}

impl<'r> Iterator for Capped<'r> {
    type Item = &'r [u8];

    #[inline]
    fn next(&mut self) -> Option<&'r [u8]> {
        self.names.next().map(Vec::as_slice)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.names.size_hint()
    }

    /// By the set's own, as `map`'s is.
    #[inline]
    fn fold<B, F: FnMut(B, &'r [u8]) -> B>(self, init: B, mut f: F) -> B {
        (self.names).fold(init, |acc, name| f(acc, name.as_slice()))
    }
}

impl FusedIterator for Capped<'_> {}

/// The names of one category of a [`NameSet`](crate::NameSet), in order:
/// what [`NameSet::of`](crate::NameSet::of) gives.
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Debug, Clone)]
pub struct NamesOf<'s> {
    names: btree_set::Range<'s, (ResKind, Vec<u8>)>,
}

impl<'s> NamesOf<'s> {
    /// The names of `names`, all of one category.
    pub(crate) const fn new(names: btree_set::Range<'s, (ResKind, Vec<u8>)>) -> NamesOf<'s> {
        NamesOf { names }
    }
}

impl<'s> Iterator for NamesOf<'s> {
    type Item = &'s [u8];

    #[inline]
    fn next(&mut self) -> Option<&'s [u8]> {
        self.names.next().map(|(_, name)| name.as_slice())
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.names.size_hint()
    }

    /// By the set's own, as `map`'s is.
    #[inline]
    fn fold<B, F: FnMut(B, &'s [u8]) -> B>(self, init: B, mut f: F) -> B {
        (self.names).fold(init, |acc, (_, name)| f(acc, name.as_slice()))
    }
}

impl FusedIterator for NamesOf<'_> {}

/// Every name of a [`NameSet`](crate::NameSet), each in its category, in
/// order: what [`NameSet::iter`](crate::NameSet::iter) gives.
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Debug, Clone)]
pub struct Names<'s> {
    names: btree_set::Iter<'s, (ResKind, Vec<u8>)>,
}

impl<'s> Names<'s> {
    /// The names of `names`.
    pub(crate) const fn new(names: btree_set::Iter<'s, (ResKind, Vec<u8>)>) -> Names<'s> {
        Names { names }
    }
}

impl<'s> Iterator for Names<'s> {
    type Item = (ResKind, &'s [u8]);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.names
            .next()
            .map(|(kind, name)| (*kind, name.as_slice()))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.names.size_hint()
    }

    /// By the set's own, as `map`'s is.
    #[inline]
    fn fold<B, F: FnMut(B, Self::Item) -> B>(self, init: B, mut f: F) -> B {
        (self.names).fold(init, |acc, (kind, name)| f(acc, (*kind, name.as_slice())))
    }
}

impl FusedIterator for Names<'_> {}

/// Every property list of a [`Properties`](crate::Properties), in no
/// order: what [`Properties::lists`](crate::Properties::lists) gives.
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Clone)]
pub struct Lists<'p> {
    lists: hash_map::Values<'p, Vec<u8>, Dict>,
}

impl<'p> Lists<'p> {
    /// The lists of `lists`.
    pub(crate) const fn new(lists: hash_map::Values<'p, Vec<u8>, Dict>) -> Lists<'p> {
        Lists { lists }
    }
}

impl<'p> Iterator for Lists<'p> {
    type Item = &'p Dict;

    #[inline]
    fn next(&mut self) -> Option<&'p Dict> {
        self.lists.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.lists.size_hint()
    }

    /// By the map's own.
    #[inline]
    fn fold<B, F: FnMut(B, &'p Dict) -> B>(self, init: B, f: F) -> B {
        self.lists.fold(init, f)
    }
}

impl FusedIterator for Lists<'_> {}

impl fmt::Debug for Lists<'_> {
    /// How many lists are left; not the lists, whose strings may be a
    /// page's text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lists")
            .field("left", &self.lists.len())
            .finish()
    }
}

/// The marked ranges still open, by their index among those found, the
/// outermost first: what [`MarkedNest::open`](crate::MarkedNest::open)
/// gives. It knows how many are left.
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Debug, Clone)]
pub struct Open<'n> {
    /// Each range open: its index, and where it starts.
    open: slice::Iter<'n, (usize, usize)>,
}

impl<'n> Open<'n> {
    /// The ranges of `open`, each its index and where it starts.
    pub(crate) fn new(open: &'n [(usize, usize)]) -> Open<'n> {
        Open { open: open.iter() }
    }
}

impl Iterator for Open<'_> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        self.open.next().map(|&(index, _)| index)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.open.size_hint()
    }

    /// By the slice's own, as `map`'s is.
    #[inline]
    fn fold<B, F: FnMut(B, usize) -> B>(self, init: B, mut f: F) -> B {
        (self.open).fold(init, |acc, &(index, _)| f(acc, index))
    }
}

impl ExactSizeIterator for Open<'_> {}

impl FusedIterator for Open<'_> {}

/// Every name a [`Reserved`](crate::Reserved) keeps, each in its
/// category, in order: what
/// [`Reserved::names`](crate::Reserved::names) gives.
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Debug, Clone)]
pub struct ReservedNames<'r> {
    names: btree_map::Keys<'r, (ResKind, Vec<u8>), Vec<Res>>,
}

impl<'r> ReservedNames<'r> {
    /// The names of `names`.
    pub(crate) const fn new(
        names: btree_map::Keys<'r, (ResKind, Vec<u8>), Vec<Res>>,
    ) -> ReservedNames<'r> {
        ReservedNames { names }
    }
}

impl<'r> Iterator for ReservedNames<'r> {
    type Item = (ResKind, &'r [u8]);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.names
            .next()
            .map(|(kind, name)| (*kind, name.as_slice()))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.names.size_hint()
    }

    /// By the map's own, as `map`'s is.
    #[inline]
    fn fold<B, F: FnMut(B, Self::Item) -> B>(self, init: B, mut f: F) -> B {
        (self.names).fold(init, |acc, (kind, name)| f(acc, (*kind, name.as_slice())))
    }
}

impl FusedIterator for ReservedNames<'_> {}

#[cfg(test)]
mod tests;
