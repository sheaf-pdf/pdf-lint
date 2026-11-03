//! The common structures, read through pdf-semantics: the page tree (ISO
//! 32000-2, 7.7.3), by ISO's rules and by PDFium's; each name and number
//! tree the walk reaches (7.9.6, 7.9.7); and the catalog's XMP packet
//! (14.3.2), which the extensions a file declares are read from
//! ([`extension::read`](crate::extension::read)).
//!
//! The page tree is walked before the walk from the trailer, from the
//! catalog the trailer as written leads to. The trees are read after the
//! content: each root the walk reached as a name or number tree in the
//! newest revision, once, in the order it reached them, a root with a
//! number given by it, so that what's told of the root is in its object.
//! Each reads the file's objects as the predicates resolve them
//! ([`Objects`]), which says why one doesn't read as a document's own
//! resolver does, so that an object that takes a password or is past a cap
//! ends a walk, where one that doesn't parse is the file's fault. What each
//! tells is kept ([`Kept`]), each walked to its end; nothing it tells is
//! found yet.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::mem::{self, Discriminant};

use pdf_semantics::pages::pages;
use pdf_semantics::tree::{names, numbers};
use pdf_semantics::{Fault, Limits, Place, Sink};

use super::{Objects, Walk};
use crate::Rules;
use crate::object::{Dict, Object, ObjectId};
use crate::source::Source;

/// What the structures are read under: pdf-semantics' caps, which ISO
/// 32000-2 doesn't set.
const LIMITS: Limits = Limits::DEFAULT;

/// Keeps, of each kind of fault in each object, the first told, where it
/// was, and how many were told, in the order each was first told: what a
/// finding needs, so that what's kept grows with the objects and kinds of
/// fault, never with the faults, which a file can make millions of (a
/// `/Names` of a million keys out of order, a `/Kids` listing one page a
/// million times).
#[derive(Default)]
pub(crate) struct Kept {
    /// Each object's kind: its first fault, where, and how many.
    told: Vec<(Place, Fault, usize)>,
    /// Where each object's kind is in `told`, only looked up.
    index: HashMap<(Option<ObjectId>, Discriminant<Fault>), usize>,
}

impl Sink for Kept {
    fn fault(&mut self, at: Place, fault: Fault) {
        match self.index.entry((at.object, mem::discriminant(&fault))) {
            Entry::Occupied(kind) => self.told[*kind.get()].2 += 1,
            Entry::Vacant(kind) => {
                kind.insert(self.told.len());
                self.told.push((at, fault, 1));
            }
        }
    }
}

#[cfg(test)]
impl Kept {
    /// What's kept, in the order each was first told.
    pub(crate) fn told(&self) -> &[(Place, Fault, usize)] {
        &self.told
    }
}

/// Notes whether a walk told of an object that doesn't read
/// ([`Fault::Damaged`]), the one fault a walk by PDFium's rules tells, and
/// keeps nothing else.
#[derive(Default)]
struct Damage {
    /// Whether one was told.
    told: bool,
}

impl Sink for Damage {
    fn fault(&mut self, _: Place, fault: Fault) {
        self.told |= matches!(fault, Fault::Damaged { .. });
    }
}

/// A name or number tree's root the walk reached ([`Walk::tree`]), which
/// [`Walk::structures`] reads again.
pub(super) struct Root<'a> {
    /// The root as written.
    object: &'a Object,
    /// Its number, where it's an indirect object.
    id: Option<ObjectId>,
    /// Whether it's a name tree's, not a number tree's.
    names: bool,
}

/// Walks the page tree that `trailer`'s catalog leads to, by ISO's rules,
/// what it tells kept, and by PDFium's, noting whether an object didn't
/// read; each to its end.
pub(super) fn page_trees(source: &dyn Source, trailer: &Dict) {
    let Ok(Some(catalog)) = pdf_semantics::catalog(Objects(source), trailer) else {
        return;
    };
    let mut kept = Kept::default();
    let _ = pages(Objects(source), catalog, Rules::Iso, &LIMITS, &mut kept);
    let mut damage = Damage::default();
    let _ = pages(
        Objects(source),
        catalog,
        Rules::Pdfium,
        &LIMITS,
        &mut damage,
    );
}

impl<'a> Walk<'a> {
    /// Keeps `root`, a name tree's root when `names`, else a number
    /// tree's, with its number where it's an indirect object, for
    /// [`Walk::structures`]. Not in an earlier revision's trailer, whose
    /// objects are the file's where no later revision wrote them, as the
    /// walk from the trailer reaches them.
    pub(super) fn note_root(&mut self, root: &'a Object, id: Option<ObjectId>, names: bool) {
        if self.revision.is_none() {
            self.roots.push(Root {
                object: root,
                id,
                names,
            });
        }
    }

    /// Reads each name and number tree the walk reached through
    /// pdf-semantics' trees, from its root, in the order the walk reached
    /// them: see the module's documentation.
    pub(super) fn structures(&mut self) {
        let roots = mem::take(&mut self.roots);
        for root in &roots {
            // Given by its number, the walk starts in the root's object.
            let reference;
            let given = match root.id {
                Some(id) => {
                    reference = Object::from(id);
                    &reference
                }
                None => root.object,
            };
            let objects = Objects(self.source);
            let mut kept = Kept::default();
            match root.names {
                true => names(objects, given, &LIMITS, &mut kept).for_each(drop),
                false => numbers(objects, given, &LIMITS, &mut kept).for_each(drop),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fault's place in `object`, at no bytes in particular.
    fn at(object: Option<ObjectId>) -> Place {
        Place::new(object, None)
    }

    /// Of each kind in each object, the first told and its place, and how
    /// many, in the order each kind was first told there: a kind told in
    /// another object is another, and a kind's fields don't part it.
    #[test]
    fn kept_is_each_kind_s_first_in_each_object_with_how_many() {
        let mut kept = Kept::default();
        let first = Place::new(Some((3, 0)), Some(crate::object::Span::new(None, 5, 9)));
        kept.fault(first, Fault::PageTreeRepeated { id: (5, 0) });
        kept.fault(at(Some((3, 0))), Fault::TreeKeyOrder);
        kept.fault(at(Some((3, 0))), Fault::PageTreeRepeated { id: (6, 0) });
        kept.fault(at(Some((4, 0))), Fault::PageTreeRepeated { id: (5, 0) });
        kept.fault(at(None), Fault::TreeKeyOrder);
        for _ in 0..1000 {
            kept.fault(at(Some((3, 0))), Fault::TreeKeyOrder);
        }
        kept.fault(at(None), Fault::TreeKeyOrder);
        assert_eq!(
            kept.told(),
            [
                (first, Fault::PageTreeRepeated { id: (5, 0) }, 2),
                (at(Some((3, 0))), Fault::TreeKeyOrder, 1001),
                (at(Some((4, 0))), Fault::PageTreeRepeated { id: (5, 0) }, 1),
                (at(None), Fault::TreeKeyOrder, 2),
            ]
        );
        assert_eq!(kept.index.len(), 4);
    }

    /// Only an object that doesn't read is noted: the one fault PDFium's
    /// rules tell, and the walk's others, which they don't, aren't.
    #[test]
    fn damage_notes_an_object_that_doesn_t_read_alone() {
        let mut damage = Damage::default();
        damage.fault(at(Some((2, 0))), Fault::PageTreeEmpty);
        damage.fault(at(None), Fault::PageNoMediaBox);
        assert!(!damage.told);
        damage.fault(at(Some((2, 0))), Fault::Damaged { id: (7, 0) });
        damage.fault(at(Some((2, 0))), Fault::PageTreeEmpty);
        assert!(damage.told);
    }
}
