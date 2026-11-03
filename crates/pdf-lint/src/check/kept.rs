//! What a reading of a file's parts told, kept as a finding needs it: the
//! common structures' through pdf-semantics, a color space's through
//! pdf-color and a font's through pdf-fonts, each crate's faults by a sink
//! of its own ([`Kept`]), each place as pdf-lint holds it ([`Where`]).

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::mem::{self, Discriminant};

use crate::object::{ObjectId, Span};

/// Where a kept fault is: the numbered object it's in, and where what's at
/// fault was written, when the reader knows, as each crate's place has
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Where {
    /// The numbered object it's in.
    pub(crate) object: Option<ObjectId>,
    /// Where it was written.
    pub(crate) at: Option<Span>,
}

impl From<pdf_semantics::Place> for Where {
    fn from(place: pdf_semantics::Place) -> Where {
        Where {
            object: place.object,
            at: place.at,
        }
    }
}

impl From<pdf_color::Place> for Where {
    fn from(place: pdf_color::Place) -> Where {
        Where {
            object: place.object,
            at: place.at,
        }
    }
}

impl From<pdf_fonts::Place> for Where {
    fn from(place: pdf_fonts::Place) -> Where {
        Where {
            object: place.object,
            at: place.at,
        }
    }
}

/// Keeps, of each kind of fault in each object, the first told, where it
/// was, and how many were told, in the order each was first told: what a
/// finding needs, so that what's kept grows with the objects and kinds of
/// fault, never with the faults, which a file can make millions of (a
/// `/Names` of a million keys out of order, a `/Kids` listing one page a
/// million times). One told again at the bytes its kind was last told at
/// in the object is the same fault read again, as the page tree's walk
/// reads a page's `/Parent` for the node it names and again for what it
/// inherits, and isn't counted again; one at no bytes known always is. A
/// kind is a variant of `F`, the fault of the crate whose sink it is.
pub(crate) struct Kept<F> {
    /// Each object's kind: its first fault, where, and how many.
    told: Vec<(Where, F, usize)>,
    /// The bytes each of `told` was last told at.
    last: Vec<Option<Span>>,
    /// Where each object's kind is in `told`, only looked up.
    index: HashMap<(Option<ObjectId>, Discriminant<F>), usize>,
}

/// Nothing kept. Written out, as a derive would ask the fault to have a
/// default, which none has.
impl<F> Default for Kept<F> {
    fn default() -> Kept<F> {
        Kept {
            told: Vec::new(),
            last: Vec::new(),
            index: HashMap::new(),
        }
    }
}

impl<F> Kept<F> {
    /// Keeps `fault`, told at `at`, as the type's documentation says.
    fn keep(&mut self, at: Where, fault: F) {
        match self.index.entry((at.object, mem::discriminant(&fault))) {
            Entry::Occupied(kind) => {
                let i = *kind.get();
                if at.at.is_none() || at.at != self.last[i] {
                    self.told[i].2 += 1;
                    self.last[i] = at.at;
                }
            }
            Entry::Vacant(kind) => {
                kind.insert(self.told.len());
                self.told.push((at, fault, 1));
                self.last.push(at.at);
            }
        }
    }

    /// What's kept, in the order each was first told.
    pub(crate) fn told(&self) -> &[(Where, F, usize)] {
        &self.told
    }
}

impl pdf_semantics::Sink for Kept<pdf_semantics::Fault> {
    fn fault(&mut self, at: pdf_semantics::Place, fault: pdf_semantics::Fault) {
        self.keep(at.into(), fault);
    }
}

impl pdf_color::Sink for Kept<pdf_color::Fault> {
    fn fault(&mut self, at: pdf_color::Place, fault: pdf_color::Fault) {
        self.keep(at.into(), fault);
    }
}

impl pdf_fonts::Sink for Kept<pdf_fonts::Fault> {
    fn fault(&mut self, at: pdf_fonts::Place, fault: pdf_fonts::Fault) {
        self.keep(at.into(), fault);
    }
}

#[cfg(test)]
mod tests {
    //! What's kept of what each crate's reading tells: the same, whichever
    //! crate's it is.

    use std::fmt::Debug;

    use pdf_semantics::{Fault, Place, Sink};

    use super::*;

    /// A fault's place in `object`, at no bytes in particular.
    fn at(object: Option<ObjectId>) -> Place {
        Place::new(object, None)
    }

    /// [`at`], as it's kept.
    fn kept_at(object: Option<ObjectId>) -> Where {
        at(object).into()
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
                (first.into(), Fault::PageTreeRepeated { id: (5, 0) }, 2),
                (kept_at(Some((3, 0))), Fault::TreeKeyOrder, 1001),
                (
                    kept_at(Some((4, 0))),
                    Fault::PageTreeRepeated { id: (5, 0) },
                    1
                ),
                (kept_at(None), Fault::TreeKeyOrder, 2),
            ]
        );
        assert_eq!(kept.index.len(), 4);

        // Told again at the bytes its kind was last told at in the object,
        // as a walk reads a value twice, it's once; at other bytes, or at
        // none known, again.
        let mut kept = Kept::default();
        let bytes = |start| Place::new(Some((2, 0)), Some(Span::new(None, start, start + 5)));
        for start in [10, 10, 20, 10, 10] {
            kept.fault(bytes(start), Fault::PageTreeChain);
        }
        kept.fault(at(Some((2, 0))), Fault::PageTreeChain);
        kept.fault(at(Some((2, 0))), Fault::PageTreeChain);
        assert_eq!(kept.told(), [(bytes(10).into(), Fault::PageTreeChain, 5)]);
    }

    /// What's kept of two kinds of a crate's faults, `a` and `b`, each told
    /// through its sink by `tell`: `a` in object 3 at bytes 10 to 15, `b`
    /// there at none, `a` again at those bytes, at bytes 20 to 25 and at
    /// none, `a` in object 4, and `b` in no object.
    fn kept_of<F: Copy>(a: F, b: F, tell: fn(&mut Kept<F>, Where, F)) -> Vec<(Where, F, usize)> {
        let bytes = |start| Where {
            object: Some((3, 0)),
            at: Some(Span::new(None, start, start + 5)),
        };
        let mut kept = Kept::default();
        tell(&mut kept, bytes(10), a);
        tell(&mut kept, kept_at(Some((3, 0))), b);
        tell(&mut kept, bytes(10), a);
        tell(&mut kept, bytes(20), a);
        tell(&mut kept, kept_at(Some((3, 0))), a);
        tell(&mut kept, kept_at(Some((4, 0))), a);
        tell(&mut kept, kept_at(None), b);
        kept.told().to_vec()
    }

    /// Each crate's faults are kept alike, through the sink of its own: of
    /// each kind in each object the first, where, and how many, one told
    /// again at its kind's last bytes there not counted again, in the order
    /// each was first told.
    #[test]
    fn each_crate_s_faults_are_kept_alike() {
        fn alike<F: Copy + PartialEq + Debug>(a: F, b: F, tell: fn(&mut Kept<F>, Where, F)) {
            let first = Where {
                object: Some((3, 0)),
                at: Some(Span::new(None, 10, 15)),
            };
            let expected = [
                (first, a, 3),
                (kept_at(Some((3, 0))), b, 1),
                (kept_at(Some((4, 0))), a, 1),
                (kept_at(None), b, 1),
            ];
            assert_eq!(kept_of(a, b, tell), expected);
        }
        alike(
            Fault::PageTreeChain,
            Fault::TreeKeyOrder,
            |kept, at, fault| {
                Sink::fault(kept, Place::new(at.object, at.at), fault);
            },
        );
        alike(
            pdf_color::Fault::LookupLength,
            pdf_color::Fault::Hival,
            |kept, at, fault| {
                let place = pdf_color::Place::new(at.object, at.at);
                pdf_color::Sink::fault(kept, place, fault);
            },
        );
        alike(
            pdf_fonts::Fault::NoWidths,
            pdf_fonts::Fault::W,
            |kept, at, fault| {
                let place = pdf_fonts::Place::new(at.object, at.at);
                pdf_fonts::Sink::fault(kept, place, fault);
            },
        );
    }
}
