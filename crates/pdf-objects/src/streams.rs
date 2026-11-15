//! Bounded access to decrypted, filter-decoded PDF stream data ([`Streams`], [`WithStreams`]).
//!
//! [`Resolve`] looks up indirect objects—including stream dictionaries and raw [`Data`](crate::Data)
//! locations—without running stream filters or ciphers. Domain crates that read decoded stream
//! payloads (such as content streams, `/ToUnicode` CMaps, or ICC profiles) accept [`Streams`] so
//! any file reader, cache, or custom closure ([`WithStreams`]) can supply decoded bytes under a
//! caller-enforced byte limit.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
#[cfg(target_has_atomic = "ptr")]
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;

use crate::object::{Object, ObjectId};
use crate::{Resolve, Unreadable, Why};

/// Decodes and decrypts indirect PDF stream payloads up to a caller-supplied byte limit.
///
/// Implementations return `Cow::Borrowed` when decoded bytes are already cached in memory and
/// `Cow::Owned` when decoded on demand. Returned slices are guaranteed to have length at most
/// `limit`; outputs exceeding `limit` fail with [`Why::Limit`].
pub trait Streams {
    /// Returns at most `limit` decrypted, filter-decoded bytes for stream `id`.
    ///
    /// `id` must be the stream object's own [`ObjectId`] (after resolving any indirect reference).
    ///
    /// # Errors
    ///
    /// Returns [`Unreadable`] with:
    /// - [`Why::Damaged`] if `id` is absent, is not a stream, or fails to decode any bytes;
    /// - [`Why::Limit`] if the decoded output exceeds `limit` bytes;
    /// - [`Why::Capped`] if decoding exceeds a reader-configured resource cap;
    /// - [`Why::Locked`] if decryption requires a missing password or fails;
    /// - [`Why::Filter`] if `/Filter` or `/DecodeParms` is unsupported or malformed;
    /// - [`Why::Source`] if reading from the underlying byte source fails;
    /// - [`Why::Detached`] if the backing stream bytes are no longer attached.
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable>;
}

impl<S: Streams + ?Sized> Streams for &S {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        (**self).decoded(id, limit)
    }
}

impl<S: Streams + ?Sized> Streams for Box<S> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        (**self).decoded(id, limit)
    }
}

#[cfg(target_has_atomic = "ptr")]
impl<S: Streams + ?Sized> Streams for Arc<S> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        (**self).decoded(id, limit)
    }
}

/// Pairs an object resolver `R` with a stream-decoding function `F` to implement both [`Resolve`]
/// and [`Streams`].
///
/// [`WithStreams::decoded`] enforces the requested `limit`: if `F` returns more than `limit`
/// bytes, [`WithStreams`] returns `Err(Unreadable)` with [`Why::Limit`].
///
/// ```
/// use pdf_objects::{
///     Object, ObjectId, Resolve, Streams, Unreadable, Why, WithStreams, dict, resolve_fn,
/// };
///
/// let font = Object::from(dict! {
///     "Subtype" => Object::name("Type1"),
///     "BaseFont" => Object::name("Helvetica"),
/// });
/// let objects = resolve_fn(|id: ObjectId| (id == ObjectId::new(1, 0)).then_some(&font));
/// let reader = WithStreams::new(objects, |id: ObjectId, _limit: usize| match id.number {
///     2 => Ok(b"1 beginbfchar <41> <0041> endbfchar".to_vec()),
///     _ => Err(Unreadable::new(id, Why::Damaged, format!("{id} R is missing"))),
/// });
///
/// assert_eq!(reader.resolve(ObjectId::new(1, 0)), Some(&font));
/// let cmap = reader.decoded(ObjectId::new(2, 0), 100).unwrap();
/// assert!(cmap.starts_with(b"1 beginbfchar"));
/// assert_eq!(reader.decoded(ObjectId::new(2, 0), 5).unwrap_err().why, Why::Limit);
/// assert_eq!(reader.decoded(ObjectId::new(3, 0), 100).unwrap_err().why, Why::Damaged);
/// ```
#[derive(Clone, Copy)]
pub struct WithStreams<R, F> {
    objects: R,
    streams: F,
}

impl<R, F> WithStreams<R, F> {
    /// Combines resolver `objects` with stream-decoding function `streams`.
    #[must_use]
    pub const fn new(objects: R, streams: F) -> WithStreams<R, F> {
        WithStreams { objects, streams }
    }

    /// Returns a reference to the underlying object resolver.
    #[must_use]
    pub const fn objects(&self) -> &R {
        &self.objects
    }
}

impl<R: fmt::Debug, F> fmt::Debug for WithStreams<R, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WithStreams")
            .field("objects", &self.objects)
            .finish_non_exhaustive()
    }
}

impl<'o, R: Resolve<'o>, F> Resolve<'o> for WithStreams<R, F> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.objects.resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        self.objects.try_resolve(id)
    }
}

impl<'o, R: Resolve<'o>, F> Resolve<'o> for &WithStreams<R, F> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.objects.resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        self.objects.try_resolve(id)
    }
}

impl<R, F: Fn(ObjectId, usize) -> Result<Vec<u8>, Unreadable>> Streams for WithStreams<R, F> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        let bytes = (self.streams)(id, limit)?;
        if bytes.len() > limit {
            return Err(Unreadable::new(
                id,
                Why::Limit,
                format!(
                    "decoded stream {id} R length ({}) exceeds limit ({limit})",
                    bytes.len()
                ),
            ));
        }
        Ok(Cow::Owned(bytes))
    }
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;
    use alloc::string::ToString;
    use alloc::vec;
    use core::cell::Cell;
    use core::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::object::Stream;
    use crate::{ResolveFn, resolve_fn};

    struct Map {
        objects: BTreeMap<ObjectId, Object>,
        asked: AtomicUsize,
    }

    impl Streams for Map {
        fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
            self.asked.fetch_add(1, Ordering::Relaxed);
            let stream = self.objects.get(&id).and_then(Object::as_stream);
            let Some(data) = stream.and_then(|s| s.data.bytes()) else {
                return Err(Unreadable::new(id, Why::Damaged, "no stream"));
            };
            if data.len() > limit {
                Err(Unreadable::new(id, Why::Limit, "past the limit"))
            } else {
                Ok(Cow::Borrowed(data))
            }
        }
    }

    fn map() -> Map {
        let stream = Stream::new(crate::dict! {}, b"abc".to_vec());
        let objects = BTreeMap::from([
            (ObjectId::new(1, 0), Object::from(stream)),
            (ObjectId::new(2, 0), Object::from(7)),
        ]);
        Map {
            objects,
            asked: AtomicUsize::new(0),
        }
    }

    fn by_resolve<'o>(r: impl Resolve<'o>, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        r.try_resolve(id)
    }

    fn gives<S: Streams + ?Sized>(streams: &S) -> Vec<Result<Vec<u8>, Why>> {
        let asked = [(1, 3), (1, 2), (2, 10), (3, 10)];
        asked
            .into_iter()
            .map(|(n, limit)| {
                streams
                    .decoded(ObjectId::new(n, 0), limit)
                    .map(Cow::into_owned)
                    .map_err(|u| u.why)
            })
            .collect()
    }

    fn expected() -> Vec<Result<Vec<u8>, Why>> {
        vec![
            Ok(b"abc".to_vec()),
            Err(Why::Limit),
            Err(Why::Damaged),
            Err(Why::Damaged),
        ]
    }

    #[test]
    fn references_and_smart_pointers_forward_streams() {
        let map = map();
        assert_eq!(gives(&map), expected());
        assert_eq!(gives(&&map), expected());
        let dynamic: &dyn Streams = &map;
        assert_eq!(gives(dynamic), expected());
        assert_eq!(gives(&dynamic), expected());
        assert_eq!(map.asked.load(Ordering::Relaxed), 16);

        let by_ref = &map;
        let lent = Streams::decoded(&by_ref, ObjectId::new(1, 0), 3).unwrap();
        assert!(matches!(lent, Cow::Borrowed(b"abc")));

        let boxed: Box<dyn Streams> = Box::new(self::map());
        assert_eq!(gives(&boxed), expected());

        let shared: Arc<dyn Streams + Send + Sync> = Arc::new(self::map());
        assert_eq!(gives(&shared), expected());
    }

    #[test]
    fn with_streams_resolves_formats_debug_and_enforces_limit() {
        let map = map();
        let asked = Cell::new(Vec::new());
        let with = WithStreams::new(&map.objects, |id: ObjectId, limit: usize| {
            let mut was = asked.take();
            was.push((id, limit));
            asked.set(was);
            Streams::decoded(&map, id, limit).map(Cow::into_owned)
        });

        let dbg = format!("{with:?}");
        assert!(dbg.starts_with("WithStreams { objects:"));
        assert_eq!(with.objects().len(), 2);

        assert_eq!(
            with.resolve(ObjectId::new(2, 0))
                .and_then(Object::as_integer),
            Some(7)
        );
        let with_ref = &with;
        assert_eq!(
            by_resolve(with_ref, ObjectId::new(2, 0))
                .unwrap()
                .and_then(Object::as_integer),
            Some(7)
        );
        assert_eq!(by_resolve(with_ref, ObjectId::new(3, 0)), Ok(None));
        assert_eq!(gives(&with), expected());
        let given = with.decoded(ObjectId::new(1, 0), 3).unwrap();
        assert!(matches!(given, Cow::Owned(ref data) if data == b"abc"));

        // Over-limit closure output is rejected with `Why::Limit`.
        let none = resolve_fn(|_: ObjectId| -> Option<&Object> { None });
        let over = WithStreams::new(none, |_: ObjectId, limit: usize| Ok(vec![0; limit + 1]));
        let copied = over;
        assert_eq!(
            over.decoded(ObjectId::new(1, 0), 4).unwrap_err().why,
            Why::Limit
        );
        assert_eq!(
            copied.decoded(ObjectId::new(1, 0), 0).unwrap_err().why,
            Why::Limit
        );

        // Underlying `try_resolve` and stream errors propagate unchanged.
        struct Locked;
        impl<'o> Resolve<'o> for Locked {
            fn resolve(&self, _: ObjectId) -> Option<&'o Object> {
                None
            }

            fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
                Err(Unreadable::new(id, Why::Locked, "it wants a password"))
            }
        }

        for reason in [Why::Source, Why::Detached] {
            let locked = WithStreams::new(Locked, |id: ObjectId, _: usize| {
                Err::<Vec<u8>, _>(Unreadable::new(id, reason, "no bytes"))
            });
            let why = by_resolve(&locked, ObjectId::new(4, 0)).unwrap_err();
            assert_eq!((why.id, why.why), (ObjectId::new(4, 0), Why::Locked));
            let why = locked.decoded(ObjectId::new(4, 0), 1).unwrap_err();
            assert_eq!((why.why, why.to_string()), (reason, "no bytes".to_string()));
        }
    }

    #[test]
    fn with_streams_const_construction() {
        type Objects = ResolveFn<fn(ObjectId) -> Option<&'static Object>>;
        type Streamed = fn(ObjectId, usize) -> Result<Vec<u8>, Unreadable>;
        fn none(_: ObjectId) -> Option<&'static Object> {
            None
        }
        fn empty(_: ObjectId, _: usize) -> Result<Vec<u8>, Unreadable> {
            Ok(Vec::new())
        }
        const WITH: WithStreams<Objects, Streamed> = WithStreams::new(resolve_fn(none), empty);
        assert!(WITH.resolve(ObjectId::new(1, 0)).is_none());
        assert_eq!(WITH.decoded(ObjectId::new(1, 0), 0).unwrap().as_ref(), b"");
    }
}
