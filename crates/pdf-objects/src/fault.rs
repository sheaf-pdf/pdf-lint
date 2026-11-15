//! Zero-cost-when-disabled diagnostic sink for recoverable PDF specification departures.
//!
//! Domain readers report non-fatal anomalies to a [`Sink<F>`] alongside a [`Place`] identifying
//! the indirect object and byte [`Span`]. Passing `&mut ()` disables collection (`wants() == false`)
//! so callers that only want parsed values pay zero diagnostic overhead while receiving identical
//! output.
//!
//! Note: [`parse::Sink`](crate::parse::Sink) is distinct because parser faults borrow directly from
//! the input slice at a [`Span`]; use [`parse::FaultSink`](crate::parse::FaultSink) to forward
//! owned [`parse::OwnedFault`](crate::parse::OwnedFault)s into a `fault::Sink`.

use alloc::vec::Vec;

use crate::object::{ObjectId, Span};

/// Receives recoverable faults of type `F` at a [`Place`].
///
/// Implemented for `()` (drops all faults with `wants() == false`), `Vec<(Place, F)>`, and
/// `&mut S` (including `&mut dyn Sink<F>`). A single collector can implement `Sink<A>` and
/// `Sink<B>` to gather faults from multiple readers.
pub trait Sink<F> {
    /// Records `fault` at `at`.
    fn fault(&mut self, at: Place, fault: F);

    /// Returns `true` if the sink wants faults reported.
    ///
    /// Readers skip fault-only validation work when `wants()` returns `false`. Defaults to `true`.
    #[must_use]
    fn wants(&self) -> bool {
        true
    }
}

/// Discards all faults and disables fault-only checks (`wants() == false`).
impl<F> Sink<F> for () {
    #[inline]
    fn fault(&mut self, _: Place, _: F) {}

    #[inline]
    fn wants(&self) -> bool {
        false
    }
}

impl<F, S: Sink<F> + ?Sized> Sink<F> for &mut S {
    #[inline]
    fn fault(&mut self, at: Place, fault: F) {
        (**self).fault(at, fault);
    }

    #[inline]
    fn wants(&self) -> bool {
        (**self).wants()
    }
}

/// Appends each fault with its [`Place`] in emission order.
impl<F> Sink<F> for Vec<(Place, F)> {
    fn fault(&mut self, at: Place, fault: F) {
        self.push((at, fault));
    }
}

/// Location of a reported fault: an optional enclosing [`ObjectId`] and byte [`Span`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct Place {
    /// Enclosing indirect object, or `None` when reading an unnumbered direct value.
    pub object: Option<ObjectId>,
    /// Source byte span of the offending token or value, if known.
    pub at: Option<Span>,
}

impl Place {
    /// Creates a [`Place`] from an optional [`ObjectId`] and [`Span`].
    #[must_use]
    pub const fn new(object: Option<ObjectId>, at: Option<Span>) -> Place {
        Place { object, at }
    }

    /// Returns this [`Place`] attributed to indirect object `object`.
    #[must_use]
    pub const fn in_object(self, object: ObjectId) -> Place {
        Place {
            object: Some(object),
            at: self.at,
        }
    }
}

impl From<Span> for Place {
    fn from(at: Span) -> Place {
        Place {
            object: None,
            at: Some(at),
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;
    use alloc::vec;

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Mark {
        One,
        Two,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Word(&'static str);

    fn at(number: u32, start: usize) -> Place {
        Place::new(
            Some(ObjectId::new(number, 0)),
            Some(Span::new(None, start, start + 1)),
        )
    }

    #[derive(Default)]
    struct Both {
        marks: Vec<(Place, Mark)>,
        words: Vec<(Place, Word)>,
        wants: bool,
    }

    impl Sink<Mark> for Both {
        fn fault(&mut self, at: Place, fault: Mark) {
            self.marks.push((at, fault));
        }

        fn wants(&self) -> bool {
            self.wants
        }
    }

    impl Sink<Word> for Both {
        fn fault(&mut self, at: Place, fault: Word) {
            self.words.push((at, fault));
        }
    }

    fn read<S: Sink<Mark> + ?Sized>(sink: &mut S) -> bool {
        let wanted = sink.wants();
        if wanted {
            sink.fault(at(1, 0), Mark::One);
            sink.fault(at(2, 5), Mark::Two);
        }
        wanted
    }

    #[test]
    fn nothing_wants_none_and_is_told_none() {
        assert!(!Sink::<Mark>::wants(&()));
        assert!(!read(&mut ()));
        Sink::fault(&mut (), at(1, 0), Mark::One);
        Sink::fault(&mut (), at(1, 0), Word("dropped"));
    }

    #[test]
    fn a_borrowed_sink_wants_what_it_wraps_and_tells_it() {
        for wants in [true, false] {
            let mut both = Both {
                wants,
                ..Both::default()
            };
            let mut borrowed = &mut both;
            assert_eq!(read(&mut borrowed), wants);
            let dynamic: &mut dyn Sink<Mark> = &mut both;
            assert_eq!(dynamic.wants(), wants);
            assert_eq!(read(dynamic), wants);
            let mut dynamic: &mut dyn Sink<Mark> = &mut both;
            assert_eq!(read(&mut dynamic), wants);
            let told = if wants { 6 } else { 0 };
            assert_eq!(both.marks.len(), told);
        }
        let mut unwanted: &mut dyn Sink<Mark> = &mut ();
        assert!(!read(&mut unwanted));
    }

    #[test]
    fn a_vec_keeps_each_with_its_place_in_order() {
        let mut kept: Vec<(Place, Mark)> = Vec::new();
        assert!(read(&mut kept));
        kept.fault(Place::new(None, None), Mark::One);
        assert_eq!(
            kept,
            vec![
                (at(1, 0), Mark::One),
                (at(2, 5), Mark::Two),
                (Place::new(None, None), Mark::One),
            ]
        );
        let mut words = Vec::new();
        Sink::fault(&mut words, at(3, 9), Word("three"));
        assert_eq!(words, vec![(at(3, 9), Word("three"))]);
    }

    #[test]
    fn one_sink_takes_two_readers_faults() {
        let mut both = Both::default();
        assert!(!Sink::<Mark>::wants(&both));
        assert!(Sink::<Word>::wants(&both));
        both.fault(at(1, 0), Mark::Two);
        both.fault(at(2, 0), Word("two"));
        {
            let words: &mut dyn Sink<Word> = &mut both;
            words.fault(at(3, 0), Word("three"));
        }
        both.wants = true;
        assert!(read(&mut both));
        assert_eq!(
            both.marks,
            vec![
                (at(1, 0), Mark::Two),
                (at(1, 0), Mark::One),
                (at(2, 5), Mark::Two),
            ]
        );
        assert_eq!(
            both.words,
            vec![(at(2, 0), Word("two")), (at(3, 0), Word("three"))]
        );
    }

    #[test]
    fn a_place_holds_its_object_and_bytes_and_orders() {
        const NOWHERE: Place = Place::new(None, None);
        assert_eq!((NOWHERE.object, NOWHERE.at), (None, None));
        let place = at(7, 3);
        assert_eq!(place.object, Some(ObjectId::new(7, 0)));
        assert_eq!(place.at, Some(Span::new(None, 3, 4)));

        let set = BTreeSet::from([at(2, 5), NOWHERE, at(1, 0)]);
        assert_eq!(set.len(), 3);
        assert_eq!(set.first(), Some(&NOWHERE));
    }
}
