//! Bounded access to decrypted, filter-decoded PDF stream data ([`Streams`], [`WithStreams`]).
//!
//! [`Resolve`] looks up indirect objects—including stream dictionaries and raw [`Data`]
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

use crate::object::{Data, Dict, Entry, Name, Object, ObjectId, Str, Value};
use crate::{Resolve, Unreadable, Why};

/// Stream payload decoded through all leading standard filters, paired with any trailing
/// non-standard filters left for a caller's codec ([`Streams::decoded_standard`]).
///
/// `Debug` prints `data` by byte length rather than raw contents so stream payloads never leak into
/// logs or diagnostics.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub struct DecodedStream<'a> {
    /// Decrypted stream bytes after every leading standard filter (`data.len() <= limit`).
    pub data: Cow<'a, [u8]>,
    /// First non-standard filter (for example `DCTDecode`, `CCITTFaxDecode`, `JBIG2Decode`, or
    /// `JPXDecode`) and any filters listed after it, or `None` when the stream used only standard
    /// filters (or none).
    pub tail: Option<FilterTail>,
}

impl<'a> DecodedStream<'a> {
    /// Creates a [`DecodedStream`] from `data` and optional remaining `tail`.
    #[must_use]
    pub const fn new(data: Cow<'a, [u8]>, tail: Option<FilterTail>) -> DecodedStream<'a> {
        DecodedStream { data, tail }
    }

    /// Converts borrowed `data` into an owned `'static` [`DecodedStream`].
    #[must_use]
    pub fn into_owned(self) -> DecodedStream<'static> {
        DecodedStream {
            data: Cow::Owned(self.data.into_owned()),
            tail: self.tail,
        }
    }
}

impl fmt::Debug for DecodedStream<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecodedStream")
            .field("data", &ByteLen(self.data.len()))
            .field("tail", &self.tail)
            .finish()
    }
}

/// Non-standard filter where standard stream decoding stopped, together with any filters listed
/// after it in `/Filter`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FilterTail {
    /// First non-standard filter (such as `DCTDecode`, `CCITTFaxDecode`, `JBIG2Decode`, or
    /// `JPXDecode`) and its resolved `/DecodeParms` dictionary.
    pub first: FilterStep,
    /// Any filters listed after [`Self::first`] in pipeline order.
    ///
    /// Single-codec image streams have an empty `after`; callers decoding a trailing image codec
    /// should reject a non-empty `after` unless they explicitly handle the remaining steps.
    pub after: Vec<FilterStep>,
}

impl FilterTail {
    /// Creates a [`FilterTail`] starting at `first` followed by `after`.
    #[must_use]
    pub const fn new(first: FilterStep, after: Vec<FilterStep>) -> FilterTail {
        FilterTail { first, after }
    }
}

/// A single stream filter and its resolved `/DecodeParms` dictionary.
///
/// `Debug` redacts string and stream byte payloads inside `parms` to byte lengths.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub struct FilterStep {
    /// Filter name, with ISO 32000-2 §8.9.7 Table 92 inline abbreviations (`AHx`, `A85`, `LZW`,
    /// `Fl`, `RL`, `CCF`, `DCT`) expanded to their full names by stream chain readers.
    pub name: Name,
    /// Resolved `/DecodeParms` dictionary for this filter, when present and a dictionary.
    ///
    /// Indirect references *to* the `/DecodeParms` entry are resolved by the reader, while
    /// indirect references *inside* the dictionary (such as `/JBIG2Globals`) remain [`ObjectId`]
    /// references for the caller to resolve via [`Resolve`] and [`Streams`].
    pub parms: Option<Dict>,
}

impl FilterStep {
    /// Creates a [`FilterStep`] for filter `name` and optional parameter dictionary `parms`.
    #[must_use]
    pub fn new(name: impl Into<Name>, parms: Option<Dict>) -> FilterStep {
        FilterStep {
            name: name.into(),
            parms,
        }
    }
}

impl fmt::Debug for FilterStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parms = self.parms.as_ref().map(|d| RedactedDict(d, 0));
        f.debug_struct("FilterStep")
            .field("name", &self.name)
            .field("parms", &parms)
            .finish()
    }
}

struct ByteLen(usize);

impl fmt::Debug for ByteLen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} bytes", self.0)
    }
}

const MAX_DEBUG_DEPTH: usize = 16;

struct RedactedDict<'a>(&'a Dict, usize);

impl fmt::Debug for RedactedDict<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.1 >= MAX_DEBUG_DEPTH {
            return write!(f, "Dict({} entries, ...)", self.0.entries().len());
        }
        let mut map = f.debug_struct("Dict");
        let entries: Vec<_> = self
            .0
            .entries()
            .iter()
            .map(|entry| RedactedEntry(entry, self.1 + 1))
            .collect();
        map.field("entries", &entries).finish()
    }
}

struct RedactedEntry<'a>(&'a Entry, usize);

impl fmt::Debug for RedactedEntry<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("key", &self.0.key_name())
            .field("key_at", &self.0.key_at())
            .field("value", &RedactedObject(self.0.value(), self.1))
            .finish()
    }
}

struct RedactedObject<'a>(&'a Object, usize);

impl fmt::Debug for RedactedObject<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Object")
            .field("value", &RedactedValue(self.0.value(), self.1))
            .field("at", &self.0.at())
            .finish()
    }
}

struct RedactedValue<'a>(&'a Value, usize);

impl fmt::Debug for RedactedValue<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Value::String(Str::Bytes(bytes), form) => f
                .debug_tuple("String")
                .field(&ByteLen(bytes.len()))
                .field(form)
                .finish(),
            Value::String(Str::Encrypted(bytes), form) => f
                .debug_tuple("EncryptedString")
                .field(&ByteLen(bytes.len()))
                .field(form)
                .finish(),
            Value::Array(items) if self.1 >= MAX_DEBUG_DEPTH => {
                write!(f, "Array({} items, ...)", items.len())
            }
            Value::Array(items) => {
                let redacted: Vec<_> = items
                    .iter()
                    .map(|o| RedactedObject(o, self.1 + 1))
                    .collect();
                f.debug_tuple("Array").field(&redacted).finish()
            }
            Value::Dict(dict) => f
                .debug_tuple("Dict")
                .field(&RedactedDict(dict, self.1))
                .finish(),
            Value::Stream(stream) => {
                let data: &dyn fmt::Debug = match &stream.data {
                    Data::Bytes(bytes) => &ByteLen(bytes.len()),
                    other => other,
                };
                f.debug_struct("Stream")
                    .field("dict", &RedactedDict(&stream.dict, self.1 + 1))
                    .field("data", data)
                    .finish()
            }
            other => fmt::Debug::fmt(other, f),
        }
    }
}

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

    /// Decodes stream `id` through all leading standard filters up to `limit` bytes, stopping at
    /// the first non-standard filter (such as `DCTDecode`, `CCITTFaxDecode`, `JBIG2Decode`, or
    /// `JPXDecode`) and returning the remaining [`FilterTail`] alongside the decoded bytes.
    ///
    /// When the stream uses only standard filters (or none), [`DecodedStream::tail`] is `None` and
    /// [`DecodedStream::data`] matches [`Streams::decoded`].
    ///
    /// The default implementation delegates to [`Streams::decoded`], returning `tail: None` on
    /// success and preserving [`Why::Filter`] for streams with non-standard filters on existing
    /// implementations.
    ///
    /// Resource limits and panic freedom apply only through the standard filters run by the
    /// implementation; callers running third-party image codecs on the returned bytes must enforce
    /// their own bounds and error handling on those codecs.
    ///
    /// # Errors
    ///
    /// Returns [`Unreadable`] for the same reasons as [`Streams::decoded`], except that
    /// implementations supporting partial filter chains return [`DecodedStream`] with `tail: Some`
    /// instead of [`Why::Filter`] when decoding stops cleanly at a non-standard filter within
    /// `limit`.
    fn decoded_standard(
        &self,
        id: ObjectId,
        limit: usize,
    ) -> Result<DecodedStream<'_>, Unreadable> {
        self.decoded(id, limit)
            .map(|data| DecodedStream::new(data, None))
    }
}

impl<S: Streams + ?Sized> Streams for &S {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        (**self).decoded(id, limit)
    }

    fn decoded_standard(
        &self,
        id: ObjectId,
        limit: usize,
    ) -> Result<DecodedStream<'_>, Unreadable> {
        (**self).decoded_standard(id, limit)
    }
}

impl<S: Streams + ?Sized> Streams for Box<S> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        (**self).decoded(id, limit)
    }

    fn decoded_standard(
        &self,
        id: ObjectId,
        limit: usize,
    ) -> Result<DecodedStream<'_>, Unreadable> {
        (**self).decoded_standard(id, limit)
    }
}

#[cfg(target_has_atomic = "ptr")]
impl<S: Streams + ?Sized> Streams for Arc<S> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        (**self).decoded(id, limit)
    }

    fn decoded_standard(
        &self,
        id: ObjectId,
        limit: usize,
    ) -> Result<DecodedStream<'_>, Unreadable> {
        (**self).decoded_standard(id, limit)
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

    #[test]
    fn decoded_standard_default_forwarding_and_debug_hygiene() {
        // Default `decoded_standard` keeps `Why::Filter` on existing `Streams` impls and returns
        // `tail: None` when `decoded` succeeds.
        let none = resolve_fn(|_: ObjectId| -> Option<&Object> { None });
        let legacy = WithStreams::new(none, |id: ObjectId, _: usize| match id.number {
            1 => Ok(b"plain".to_vec()),
            _ => Err(Unreadable::new(id, Why::Filter, "unsupported /DCTDecode")),
        });
        let plain = legacy.decoded_standard(ObjectId::new(1, 0), 10).unwrap();
        assert_eq!(plain.data.as_ref(), b"plain");
        assert_eq!(plain.tail, None);
        assert_eq!(
            legacy
                .decoded_standard(ObjectId::new(2, 0), 10)
                .unwrap_err()
                .why,
            Why::Filter
        );

        // Custom `decoded_standard` forwards through `&S`, `&dyn Streams`, `Box`, and `Arc`.
        struct CodecSource(Vec<u8>);
        impl Streams for CodecSource {
            fn decoded(&self, id: ObjectId, _: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
                Err(Unreadable::new(id, Why::Filter, "codec stream"))
            }

            fn decoded_standard(
                &self,
                id: ObjectId,
                limit: usize,
            ) -> Result<DecodedStream<'_>, Unreadable> {
                if self.0.len() > limit {
                    return Err(Unreadable::new(id, Why::Limit, "past limit"));
                }
                let step = FilterStep::new(
                    "DCTDecode",
                    Some(crate::dict! {
                        "ColorTransform" => 0,
                        "SecretNote" => Object::text("do-not-print-me"),
                    }),
                );
                let tail = FilterTail::new(step, vec![FilterStep::new("FlateDecode", None)]);
                Ok(DecodedStream::new(Cow::Borrowed(&self.0), Some(tail)))
            }
        }

        let src = CodecSource(b"SECRET_JPEG_PAYLOAD".to_vec());
        let check = |s: &dyn Streams| {
            let got = s.decoded_standard(ObjectId::new(5, 0), 64).unwrap();
            assert_eq!(got.data.as_ref(), b"SECRET_JPEG_PAYLOAD");
            let tail = got.tail.as_ref().unwrap();
            assert_eq!(tail.first.name.as_bytes(), b"DCTDecode");
            assert_eq!(
                tail.first
                    .parms
                    .as_ref()
                    .and_then(|d| d.get("ColorTransform"))
                    .and_then(Object::as_integer),
                Some(0)
            );
            assert_eq!(tail.after.len(), 1);
            assert_eq!(tail.after[0].name.as_bytes(), b"FlateDecode");

            // `Debug` must never print stream payload bytes or string plaintext inside `parms`.
            let dbg = format!("{got:?}");
            assert!(!dbg.contains("SECRET_JPEG_PAYLOAD"), "{dbg}");
            assert!(!dbg.contains("do-not-print-me"), "{dbg}");
            assert!(dbg.contains("19 bytes"), "{dbg}");

            let owned = got.into_owned();
            assert_eq!(owned.data.as_ref(), b"SECRET_JPEG_PAYLOAD");
        };

        check(&src);
        let src_ref = &src;
        check(&src_ref);
        let boxed: Box<dyn Streams> = Box::new(CodecSource(b"SECRET_JPEG_PAYLOAD".to_vec()));
        check(&boxed);
        let shared: Arc<dyn Streams + Send + Sync> =
            Arc::new(CodecSource(b"SECRET_JPEG_PAYLOAD".to_vec()));
        check(&shared);
    }
}
