//! Where a reader of a file's objects tells what it reads past: each fault
//! told to a [`Sink`] with its [`Place`], the object it's in and where it
//! was written. Each reader has a fault type of its own, `F`, and every
//! reader the one sink and the one place, so that one sink type can take
//! several readers' faults, a `Sink<A>` and a `Sink<B>`. What's read goes
//! on past each fault, and nothing told changes it: what a reader gives is
//! the same whatever the sink, and a sink that doesn't want faults, `()`,
//! has none looked for ([`Sink::wants`]), so a reader asked for none pays
//! for no check. A reader takes its sink as a type parameter, so that `()`
//! costs nothing; a `&mut dyn Sink<F>` can be given as well.
//!
//! Three readers keep a sink of their own, as what they tell isn't placed
//! by an object: this crate's parser
//! ([`parse::Sink`](crate::parse::Sink)), whose faults borrow the data
//! parsed, each at a [`Span`] alone, the object it's in being the
//! caller's to know; a content stream's reader, whose faults borrow the
//! content and are placed by offsets into it, which only a page's streams,
//! joined, map back to a stream and its bytes; and a linearized file's
//! check, whose findings each carry a place of their own, a key, or a hint
//! table's byte or bit, rather than an object's.

use crate::object::{ObjectId, Span};

/// Where faults of type `F` are told: a checker keeps them; `()` drops
/// them, and wants none, so none is looked for. A fault owns nothing of
/// what's read, so a sink can keep it as it is. A sink borrowed, `&mut S`
/// or `&mut dyn Sink<F>`, is a sink too, and a `Vec` keeps each with its
/// place.
pub trait Sink<F> {
    /// A fault at `at`.
    fn fault(&mut self, at: Place, fault: F);

    /// Whether faults are wanted. A check that does work of its own, such
    /// as following a name tree's `/Limits` or reading a `/BlackPoint` by
    /// reference, runs only when they are, and a sink that doesn't want
    /// them is told none. True unless a sink says otherwise; asked as a
    /// reading goes.
    #[must_use]
    fn wants(&self) -> bool {
        true
    }
}

/// Drops every fault, and wants none, so none is looked for.
impl<F> Sink<F> for () {
    #[inline]
    fn fault(&mut self, _: Place, _: F) {}

    #[inline]
    fn wants(&self) -> bool {
        false
    }
}

/// Tells the sink it borrows, and wants what it wants.
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

/// Keeps each fault with its place, in the order they were told.
impl<F> Sink<F> for Vec<(Place, F)> {
    fn fault(&mut self, at: Place, fault: F) {
        self.push((at, fault));
    }
}

/// Where a fault is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Place {
    /// The numbered object it's in; none where a reading began in a value
    /// written in place and hasn't reached one yet. Which object that is,
    /// where a reading reads more than one, each reader says.
    pub object: Option<ObjectId>,
    /// Where what's at fault was written, when the reader knows.
    pub at: Option<Span>,
}

impl Place {
    /// A fault's place: a reading makes one for each fault it tells, and a
    /// sink is tried on one.
    #[must_use]
    pub const fn new(object: Option<ObjectId>, at: Option<Span>) -> Place {
        Place { object, at }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fault of one reader's.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Mark {
        One,
        Two,
    }

    /// A fault of another's.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Word(&'static str);

    /// A place in object `number`, at bytes `start..start + 1` of the file.
    fn at(number: u32, start: usize) -> Place {
        Place::new(
            Some(ObjectId::new(number, 0)),
            Some(Span::new(None, start, start + 1)),
        )
    }

    /// Keeps each fault of either kind, as one sink, and says whether it
    /// wants them as it's told.
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

    /// What a reading tells `sink`, as one asks first whether it's wanted:
    /// two marks, where it is.
    fn read<S: Sink<Mark> + ?Sized>(sink: &mut S) -> bool {
        let wanted = sink.wants();
        if wanted {
            sink.fault(at(1, 0), Mark::One);
            sink.fault(at(2, 5), Mark::Two);
        }
        wanted
    }

    /// `()` wants none, so a reading tells it none; told one all the same,
    /// it drops it.
    #[test]
    fn nothing_wants_none_and_is_told_none() {
        assert!(!Sink::<Mark>::wants(&()));
        assert!(!read(&mut ()));
        Sink::fault(&mut (), at(1, 0), Mark::One);
        Sink::fault(&mut (), at(1, 0), Word("dropped"));
    }

    /// A sink borrowed, `&mut S` or `&mut dyn Sink<F>`, wants what it wraps
    /// and passes on each fault it's told, as it's told.
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

    /// A `Vec` wants every fault, and keeps each with its place, in the
    /// order they were told.
    #[test]
    fn a_vec_keeps_each_with_its_place_in_order() {
        let mut kept: Vec<(Place, Mark)> = Vec::new();
        assert!(read(&mut kept));
        kept.fault(Place::new(None, None), Mark::One);
        assert_eq!(
            kept,
            [
                (at(1, 0), Mark::One),
                (at(2, 5), Mark::Two),
                (Place::new(None, None), Mark::One),
            ]
        );
        let mut words = Vec::new();
        Sink::fault(&mut words, at(3, 9), Word("three"));
        assert_eq!(words, [(at(3, 9), Word("three"))]);
    }

    /// One sink takes two readers' faults, each through its own `Sink<F>`,
    /// by value, borrowed or as a `dyn`, and wants each as it says.
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
            [
                (at(1, 0), Mark::Two),
                (at(1, 0), Mark::One),
                (at(2, 5), Mark::Two),
            ]
        );
        assert_eq!(
            both.words,
            [(at(2, 0), Word("two")), (at(3, 0), Word("three"))]
        );
    }

    /// A place is made by `new`, a `const fn`, and holds what it's given.
    #[test]
    fn a_place_holds_its_object_and_bytes() {
        const NOWHERE: Place = Place::new(None, None);
        assert_eq!((NOWHERE.object, NOWHERE.at), (None, None));
        let place = at(7, 3);
        assert_eq!(place.object, Some(ObjectId::new(7, 0)));
        assert_eq!(place.at, Some(Span::new(None, 3, 4)));
    }
}
