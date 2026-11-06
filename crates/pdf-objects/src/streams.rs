//! A stream's data, which [`Resolve`] doesn't give: what gives it
//! ([`Streams`]), decrypted and decoded through the stream's filters, and a
//! resolver and a function as one ([`WithStreams`]), for what has its
//! streams some other way.

use std::borrow::Cow;

use crate::object::{Object, ObjectId};
use crate::{Resolve, Unreadable};

/// What gives a stream's data, decrypted and decoded through its filters:
/// what reads a file implements this, a reference to it being one too, and
/// [`Resolve`], so that what reads a stream's data, a font's `/ToUnicode` or
/// a page's content, reads it through any of them. [`WithStreams`] makes one
/// of a resolver and a function.
///
/// It's the data as a viewer takes it: a stream that decodes in part is what
/// decoded, as PDFium takes it, unless what stopped it is the limit. A
/// checker that wants every fault of a decode reads the reader's own account
/// of it instead, which pdf-structure gives as `Reading::decoded_within` and
/// `Source::decoded_within`. On a pdf-structure `Reading`, whose own
/// `decoded` takes no limit, this one is `Streams::decoded(&reading, id,
/// limit)`.
///
/// What's given is borrowed where the reader holds it decoded, as a document
/// holds its object streams', and owned where it's decoded for the call. A
/// caller's cache of what's decoded goes behind this, a hit lent.
///
/// The data is at most the limit asked for, or the answer is
/// [`Why::Limit`](crate::Why::Limit). A caller doesn't trust what it's
/// given: data longer than it asked for is past the limit, whatever gave it.
pub trait Streams {
    /// Stream `id`'s data, at most `limit` bytes of it. `id` is the stream's
    /// own number: a caller has followed any reference to it.
    ///
    /// # Errors
    ///
    /// [`Unreadable`], saying why there's none:
    /// [`Why::Damaged`](crate::Why::Damaged) where it isn't a stream, doesn't
    /// read, or its decode fails before it gives a byte (a stream that
    /// decodes cleanly to nothing is empty data, not this);
    /// [`Why::Limit`](crate::Why::Limit) past `limit`;
    /// [`Why::Capped`](crate::Why::Capped) past a cap the reader reads
    /// under; [`Why::Locked`](crate::Why::Locked) where it takes
    /// a password the reader wasn't given, or doesn't decrypt;
    /// [`Why::Filter`](crate::Why::Filter) through a filter the reader doesn't
    /// decode, or with a `/Filter` or `/DecodeParms` it can't take as written;
    /// [`Why::Source`](crate::Why::Source) where the source of the file's
    /// bytes didn't give them, which asked again may;
    /// [`Why::Detached`](crate::Why::Detached) where the reader holds no
    /// stream's data and has nothing to read it from, as a document alone
    /// hasn't, which asked again won't.
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable>;
}

/// A reference to what gives a stream's data gives it, as what reads a
/// file's streams is read over a reference for its objects' lifetime.
impl<S: Streams + ?Sized> Streams for &S {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        (**self).decoded(id, limit)
    }
}

/// A resolver and a function from a stream's number and a limit to its data,
/// as one: objects, with their streams' data by the function, for what reads a
/// file and has its streams some other way. The function gives the data owned:
/// a closure can't lend what outlives the call, so a reader that holds what it
/// decodes, and would lend it, implements [`Streams`] on a type of its own.
/// What the function gives is given as it is, longer than asked for or not:
/// the caller's to refuse.
///
/// ```
/// use pdf_objects::{Object, ObjectId, Resolve, Streams, Unreadable, WithStreams, Why};
///
/// let font = Object::from(pdf_objects::dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica" });
/// let objects = |id: ObjectId| (id == ObjectId::new(1, 0)).then_some(&font);
/// let read = WithStreams::new(&objects, |id: ObjectId, _: usize| match id.number {
///     2 => Ok(b"1 beginbfchar <41> <0041> endbfchar".to_vec()),
///     _ => Err(Unreadable::new(id, Why::Damaged, format!("{id} R isn't in the file"))),
/// });
/// assert_eq!(read.resolve(ObjectId::new(1, 0)), Some(&font));
/// assert!(read.resolve(ObjectId::new(2, 0)).is_none());
/// let data = read.decoded(ObjectId::new(2, 0), 100).unwrap();
/// assert!(data.starts_with(b"1 beginbfchar"));
/// let none = read.decoded(ObjectId::new(3, 0), 100).unwrap_err();
/// assert_eq!((none.why, none.to_string()), (Why::Damaged, "3 0 R isn't in the file".into()));
/// ```
#[derive(Debug, Clone, Copy)]
pub struct WithStreams<R, F> {
    objects: R,
    streams: F,
}

impl<R, F> WithStreams<R, F> {
    /// `objects`, its streams' data by `streams`.
    #[must_use]
    pub const fn new(objects: R, streams: F) -> WithStreams<R, F> {
        WithStreams { objects, streams }
    }
}

/// Its objects, as its resolver gives them, and why not, as it says.
impl<'o, R: Resolve<'o>, F> Resolve<'o> for WithStreams<R, F> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.objects.resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        self.objects.try_resolve(id)
    }
}

/// Its streams' data, as its function gives it, owned.
impl<R, F: Fn(ObjectId, usize) -> Result<Vec<u8>, Unreadable>> Streams for WithStreams<R, F> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        (self.streams)(id, limit).map(Cow::Owned)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeMap;

    use super::*;
    use crate::Why;
    use crate::object::Stream;

    /// A file's objects in a map, which lends each stream's bytes as its
    /// data, and counts what it's asked.
    struct Map {
        objects: BTreeMap<ObjectId, Object>,
        asked: Cell<usize>,
    }

    impl Streams for Map {
        fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
            self.asked.set(self.asked.get() + 1);
            let stream = self.objects.get(&id).and_then(Object::as_stream);
            let Some(data) = stream.and_then(|stream| stream.data.bytes()) else {
                return Err(Unreadable::new(id, Why::Damaged, "no stream"));
            };
            match data.len() > limit {
                true => Err(Unreadable::new(id, Why::Limit, "past the limit")),
                false => Ok(Cow::Borrowed(data)),
            }
        }
    }

    /// A map of one stream, object 1, and one object that's no stream, 2.
    fn map() -> Map {
        let stream = Stream::new(crate::dict! {}, b"abc".to_vec());
        let objects = BTreeMap::from([
            (ObjectId::new(1, 0), Object::from(stream)),
            (ObjectId::new(2, 0), Object::from(7)),
        ]);
        Map {
            objects,
            asked: Cell::new(0),
        }
    }

    /// What `streams` gives of objects 1, within its length and a byte
    /// short, 2 and 3: the data, or why there's none.
    fn gives<S: Streams + ?Sized>(streams: &S) -> Vec<Result<Vec<u8>, Why>> {
        let asked = [(1, 3), (1, 2), (2, 10), (3, 10)];
        let given = asked.map(|(n, limit)| streams.decoded(ObjectId::new(n, 0), limit));
        (given.into_iter())
            .map(|given| {
                given
                    .map(Cow::into_owned)
                    .map_err(|unreadable| unreadable.why)
            })
            .collect()
    }

    /// What a map gives: its stream's data within the limit, and past it,
    /// or of what's no stream or isn't there, why not.
    fn given() -> Vec<Result<Vec<u8>, Why>> {
        vec![
            Ok(b"abc".to_vec()),
            Err(Why::Limit),
            Err(Why::Damaged),
            Err(Why::Damaged),
        ]
    }

    /// A reference to what gives a stream's data, and a `dyn Streams`, give
    /// what it gives, and ask it each time: nothing's kept between.
    #[test]
    fn a_reference_gives_what_it_refers_to() {
        let map = map();
        assert_eq!(gives(&map), given());
        assert_eq!(gives(&&map), given());
        let dynamic: &dyn Streams = &map;
        assert_eq!(gives(dynamic), given());
        assert_eq!(gives(&dynamic), given());
        assert_eq!(map.asked.get(), 16);
        // What it holds, it lends, through a reference too.
        let by_reference = &map;
        let lent = Streams::decoded(&by_reference, ObjectId::new(1, 0), 3).unwrap();
        assert!(matches!(lent, Cow::Borrowed(b"abc")));
    }

    /// `WithStreams` resolves by its resolver, says why as it says, and gives
    /// its function's data owned, or why there's none, as the function says.
    #[test]
    fn with_resolves_by_its_resolver_and_gives_its_function_s_data_owned() {
        let map = map();
        let objects = |id: ObjectId| map.objects.get(&id);
        let asked = Cell::new(Vec::new());
        let with = WithStreams::new(&objects, |id: ObjectId, limit: usize| {
            let mut was = asked.take();
            was.push((id, limit));
            asked.set(was);
            Streams::decoded(&map, id, limit).map(Cow::into_owned)
        });
        assert_eq!(
            with.resolve(ObjectId::new(2, 0))
                .and_then(Object::as_integer),
            Some(7)
        );
        assert_eq!(with.try_resolve(ObjectId::new(3, 0)), Ok(None));
        assert_eq!(gives(&with), given());
        let given = with.decoded(ObjectId::new(1, 0), 3).unwrap();
        assert!(matches!(given, Cow::Owned(ref data) if data == b"abc"));
        let asked: Vec<(u32, usize)> = (asked.take().iter())
            .map(|(id, limit)| (id.number, *limit))
            .collect();
        assert_eq!(asked, [(1, 3), (1, 2), (2, 10), (3, 10), (1, 3)]);

        // A resolver that says why is heard through it.
        struct Locked;
        impl<'o> Resolve<'o> for Locked {
            fn resolve(&self, _: ObjectId) -> Option<&'o Object> {
                None
            }
            fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
                Err(Unreadable::new(id, Why::Locked, "it wants a password"))
            }
        }
        // And a function that gives no data, for either reason a source
        // may have, is heard as it says.
        for given in [Why::Source, Why::Detached] {
            let locked = WithStreams::new(Locked, |id: ObjectId, _: usize| {
                Err::<Vec<u8>, _>(Unreadable::new(id, given, "no bytes"))
            });
            let why = locked.try_resolve(ObjectId::new(4, 0)).unwrap_err();
            assert_eq!((why.id, why.why), (ObjectId::new(4, 0), Why::Locked));
            let why = locked.decoded(ObjectId::new(4, 0), 1).unwrap_err();
            assert_eq!((why.why, why.to_string()), (given, "no bytes".into()));
        }
    }

    /// A `WithStreams` of functions is made at compile time.
    #[test]
    fn with_streams_is_made_at_compile_time() {
        type Objects = fn(ObjectId) -> Option<&'static Object>;
        type Streamed = fn(ObjectId, usize) -> Result<Vec<u8>, Unreadable>;
        fn none(_: ObjectId) -> Option<&'static Object> {
            None
        }
        fn empty(_: ObjectId, _: usize) -> Result<Vec<u8>, Unreadable> {
            Ok(Vec::new())
        }
        const WITH: WithStreams<Objects, Streamed> = WithStreams::new(none, empty);
        assert!(WITH.resolve(ObjectId::new(1, 0)).is_none());
        assert_eq!(
            WITH.decoded(ObjectId::new(1, 0), 1),
            Ok(Cow::Borrowed(&[][..]))
        );
    }

    /// What a function gives past the limit, `WithStreams` gives as it is:
    /// holding what's given to the limit is the caller's, which refuses it.
    #[test]
    fn data_past_the_limit_is_given_as_it_is_for_the_caller_to_refuse() {
        let none = |_: ObjectId| -> Option<&Object> { None };
        let over = WithStreams::new(&none, |_: ObjectId, limit: usize| Ok(vec![0; limit + 1]));
        let given = over.decoded(ObjectId::new(1, 0), 4).unwrap();
        assert_eq!(given.len(), 5);
        let held = |limit: usize| match over.decoded(ObjectId::new(1, 0), limit) {
            Ok(data) if data.len() <= limit => Ok(data.len()),
            Ok(_) => Err(Why::Limit),
            Err(unreadable) => Err(unreadable.why),
        };
        assert_eq!(held(0), Err(Why::Limit));
        assert_eq!(held(4), Err(Why::Limit));
        // A `WithStreams` is `Copy` where what it holds is, as a font's
        // reading wants.
        let copied = over;
        assert_eq!(copied.decoded(ObjectId::new(1, 0), 0).unwrap().len(), 1);
        assert_eq!(over.decoded(ObjectId::new(1, 0), 0).unwrap().len(), 1);
    }
}
