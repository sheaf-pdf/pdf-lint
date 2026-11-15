//! Deterministic PDF object serializer (ISO 32000-2 §7.3).
//!
//! Writes direct values ([`write()`], [`write_renumbered`]) and indirect objects ([`indirect`],
//! [`indirect_with`], [`indirect_renumbered`], [`indirect_stream`], [`indirect_stream_with`],
//! [`indirect_stream_renumbered`]) as canonical PDF syntax:
//!
//! - **Reals:** Shortest fixed-point decimal representation (always containing `.` and never an
//!   exponent) that round-trips to the same `f64`, up to 255 bytes. Non-finite values return
//!   [`Error::NonFinite`]; finite values whose fixed-point token exceeds 255 bytes return
//!   [`Error::RealOutOfRange`].
//! - **Names and strings:** Written byte-for-byte. Names containing NUL (`0x00`) are rejected
//!   ([`Error::NulName`]). Strings preserve their [`Form`] (defaulting to [`Form::Literal`]), and
//!   unencrypted [`Str::Encrypted`] values are rejected ([`Error::Encrypted`]).
//! - **Dictionaries:** Preserve entry order and duplicate keys; use [`Dict::remove_duplicates`]
//!   before serializing if strict single-key output is desired.
//! - **Streams:** Written only as top-level indirect objects (`n g obj ... endobj`), updating or
//!   appending `/Length` to match the serialized byte count. Direct or nested [`Value::Stream`]
//!   values are rejected ([`Error::InlineStream`]).
//! - **Object IDs:** Object number `0` is reserved for the cross-reference free list; indirect
//!   headers or emitted references with `number == 0` are rejected ([`Error::ZeroObjectId`]).
//!
//! On any [`Error`] or [`Refused`], the output buffer is rolled back to its starting length.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::convert::Infallible;
use core::fmt::{self, Write as _};
use core::marker::PhantomData;
use core::slice;

use crate::object::{Dict, Entry, Form, Object, ObjectId, Str, Value};
use crate::parse::Limits;

/// Maximum fixed-point byte length emitted by [`real`].
const MAX_REAL_LEN: usize = 255;

/// Lookup table of ASCII graphic bytes written verbatim in PDF names (`!`..=`~` minus `#` and
/// PDF delimiters). Note that `REGULAR[0]` is `false`.
const REGULAR: [bool; 256] = {
    let mut regular = [false; 256];
    let mut b = b'!';
    while b <= b'~' {
        regular[b as usize] = true;
        b += 1;
    }
    let special = b"#()<>[]{}/%";
    let mut i = 0;
    while i < special.len() {
        regular[special[i] as usize] = false;
        i += 1;
    }
    regular
};

/// Reason serialization refused an object and wrote nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A real number is `NaN` or infinite.
    #[error("non-finite real number{}", in_object(.id))]
    NonFinite {
        /// Indirect object ID being written, if any.
        id: Option<ObjectId>,
    },
    /// A finite real number requires more than 255 bytes in fixed-point decimal notation.
    #[error("real number exceeds 255-byte fixed-point limit{}", in_object(.id))]
    RealOutOfRange {
        /// Indirect object ID being written, if any.
        id: Option<ObjectId>,
    },
    /// Containers are nested deeper than [`Limits::DEFAULT`]'s nesting cap.
    #[error("containers nested deeper than {limit}{}", in_object(.id))]
    Nested {
        /// Indirect object ID being written, if any.
        id: Option<ObjectId>,
        /// Maximum permitted container nesting depth.
        limit: usize,
    },
    /// A string is still marked as ciphertext ([`Str::Encrypted`]).
    #[error("string is still ciphertext{}", in_object(.id))]
    Encrypted {
        /// Indirect object ID being written, if any.
        id: Option<ObjectId>,
    },
    /// A stream's payload is not held in memory as [`Data::Bytes`](crate::Data::Bytes).
    ///
    /// Use [`indirect_stream`] to serialize stream bytes supplied out-of-line.
    #[error("stream data is not in memory{}", in_object(.id))]
    Data {
        /// Indirect object ID being written, if any.
        id: Option<ObjectId>,
    },
    /// A [`Value::Stream`] was passed to [`write()`] / [`write_renumbered`] or nested inside an
    /// array or dictionary.
    #[error("stream cannot be written as a direct or nested value{}", in_object(.id))]
    InlineStream {
        /// Indirect object ID being written, if any.
        id: Option<ObjectId>,
    },
    /// A name or dictionary key contains a NUL (`0x00`) byte (forbidden by ISO 32000-2 §7.3.5).
    #[error("name contains a NUL byte{}", in_object(.id))]
    NulName {
        /// Indirect object ID being written, if any.
        id: Option<ObjectId>,
    },
    /// An indirect object header or emitted indirect reference has `number == 0`.
    #[error("object number 0 is reserved for the free list{}", in_object(.id))]
    ZeroObjectId {
        /// Indirect object ID being written, if any.
        id: Option<ObjectId>,
    },
}

impl Error {
    /// Attaches indirect object `id` to an error that has none yet.
    fn within(self, id: ObjectId) -> Error {
        match self {
            Error::NonFinite { id: None } => Error::NonFinite { id: Some(id) },
            Error::RealOutOfRange { id: None } => Error::RealOutOfRange { id: Some(id) },
            Error::Nested { id: None, limit } => Error::Nested {
                id: Some(id),
                limit,
            },
            Error::Encrypted { id: None } => Error::Encrypted { id: Some(id) },
            Error::Data { id: None } => Error::Data { id: Some(id) },
            Error::InlineStream { id: None } => Error::InlineStream { id: Some(id) },
            Error::NulName { id: None } => Error::NulName { id: Some(id) },
            Error::ZeroObjectId { id: None } => Error::ZeroObjectId { id: Some(id) },
            other => other,
        }
    }
}

fn in_object(id: &Option<ObjectId>) -> String {
    id.map(|id| format!(" in object {id}")).unwrap_or_default()
}

/// Per-object encryption hook for [`indirect_with`], [`indirect_renumbered`],
/// [`indirect_stream_with`], and [`indirect_stream_renumbered`] (ISO 32000-2 §7.6.2).
///
/// `()` implements `Encrypt` as a no-op cleartext pass-through.
pub trait Encrypt {
    /// Error returned when encrypting a string or stream fails.
    type Error;

    /// Returns `true` if the entire indirect object `id` should be written in the clear (for
    /// example, an `/Encrypt` dictionary or `/XRef` stream).
    #[must_use]
    fn clear(&self, id: ObjectId, dict: Option<&Dict>, stream: bool) -> bool {
        let _ = (id, dict, stream);
        false
    }

    /// Returns `true` if `key` inside `dict` (in object `id`) should have its value written in
    /// the clear (for example, `/Contents` in a signature dictionary).
    #[must_use]
    fn is_clear_key(&self, id: ObjectId, dict: &Dict, key: &[u8]) -> bool {
        let _ = (id, dict, key);
        false
    }

    /// Encrypts string `text` belonging to indirect object `id`.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if encryption fails; the serializer rolls back the entire object and
    /// returns [`Refused::Encrypt`].
    fn string<'t>(&self, id: ObjectId, text: &'t [u8]) -> Result<Cow<'t, [u8]>, Self::Error>;

    /// Encrypts filter-encoded stream payload `data` belonging to stream `id` with `dict`.
    ///
    /// Called before serializing `dict` so `/Length` reflects the encrypted byte count.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if encryption fails; the serializer rolls back the entire object and
    /// returns [`Refused::Encrypt`].
    fn stream<'d>(
        &self,
        id: ObjectId,
        dict: &Dict,
        data: &'d [u8],
    ) -> Result<Cow<'d, [u8]>, Self::Error>;
}

impl Encrypt for () {
    type Error = Infallible;

    #[inline]
    fn clear(&self, _: ObjectId, _: Option<&Dict>, _: bool) -> bool {
        true
    }

    #[inline]
    fn is_clear_key(&self, _: ObjectId, _: &Dict, _: &[u8]) -> bool {
        false
    }

    #[inline]
    fn string<'t>(&self, _: ObjectId, text: &'t [u8]) -> Result<Cow<'t, [u8]>, Infallible> {
        Ok(Cow::Borrowed(text))
    }

    #[inline]
    fn stream<'d>(
        &self,
        _: ObjectId,
        _: &Dict,
        data: &'d [u8],
    ) -> Result<Cow<'d, [u8]>, Infallible> {
        Ok(Cow::Borrowed(data))
    }
}

impl<E: Encrypt + ?Sized> Encrypt for &E {
    type Error = E::Error;

    #[inline]
    fn clear(&self, id: ObjectId, dict: Option<&Dict>, stream: bool) -> bool {
        (**self).clear(id, dict, stream)
    }

    #[inline]
    fn is_clear_key(&self, id: ObjectId, dict: &Dict, key: &[u8]) -> bool {
        (**self).is_clear_key(id, dict, key)
    }

    #[inline]
    fn string<'t>(&self, id: ObjectId, text: &'t [u8]) -> Result<Cow<'t, [u8]>, Self::Error> {
        (**self).string(id, text)
    }

    #[inline]
    fn stream<'d>(
        &self,
        id: ObjectId,
        dict: &Dict,
        data: &'d [u8],
    ) -> Result<Cow<'d, [u8]>, Self::Error> {
        (**self).stream(id, dict, data)
    }
}

impl<E: Encrypt + ?Sized> Encrypt for Box<E> {
    type Error = E::Error;

    #[inline]
    fn clear(&self, id: ObjectId, dict: Option<&Dict>, stream: bool) -> bool {
        (**self).clear(id, dict, stream)
    }

    #[inline]
    fn is_clear_key(&self, id: ObjectId, dict: &Dict, key: &[u8]) -> bool {
        (**self).is_clear_key(id, dict, key)
    }

    #[inline]
    fn string<'t>(&self, id: ObjectId, text: &'t [u8]) -> Result<Cow<'t, [u8]>, Self::Error> {
        (**self).string(id, text)
    }

    #[inline]
    fn stream<'d>(
        &self,
        id: ObjectId,
        dict: &Dict,
        data: &'d [u8],
    ) -> Result<Cow<'d, [u8]>, Self::Error> {
        (**self).stream(id, dict, data)
    }
}

impl<E: Encrypt + ?Sized> Encrypt for Arc<E> {
    type Error = E::Error;

    #[inline]
    fn clear(&self, id: ObjectId, dict: Option<&Dict>, stream: bool) -> bool {
        (**self).clear(id, dict, stream)
    }

    #[inline]
    fn is_clear_key(&self, id: ObjectId, dict: &Dict, key: &[u8]) -> bool {
        (**self).is_clear_key(id, dict, key)
    }

    #[inline]
    fn string<'t>(&self, id: ObjectId, text: &'t [u8]) -> Result<Cow<'t, [u8]>, Self::Error> {
        (**self).string(id, text)
    }

    #[inline]
    fn stream<'d>(
        &self,
        id: ObjectId,
        dict: &Dict,
        data: &'d [u8],
    ) -> Result<Cow<'d, [u8]>, Self::Error> {
        (**self).stream(id, dict, data)
    }
}

/// Combined serialization or encryption error returned by `*_with` and `*_renumbered` writers.
///
/// Exhaustive on purpose: each variant delegates extensibility to its inner error type.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refused<E> {
    /// Serialization error ([`Error`]).
    #[error("{0}")]
    Write(Error),
    /// Encryption error returned by [`Encrypt`].
    #[error("object {id} can't be encrypted: {why}")]
    Encrypt {
        /// Indirect object ID whose string or stream failed encryption.
        id: ObjectId,
        /// Underlying encryptor error.
        why: E,
    },
}

impl<E> From<Error> for Refused<E> {
    fn from(error: Error) -> Refused<E> {
        Refused::Write(error)
    }
}

impl<E> Refused<E> {
    fn within(self, id: ObjectId) -> Refused<E> {
        match self {
            Refused::Write(e) => Refused::Write(e.within(id)),
            other => other,
        }
    }
}

/// On-the-fly indirect reference remapping during serialization.
///
/// Returning `Some(new_id)` writes `new_id` as `n g R` (unless `new_id.number == 0`, which fails
/// with [`Error::ZeroObjectId`]); returning `None` writes `null` (ISO 32000-2 §7.3.10).
///
/// Implemented for `()` (identity), any `Fn(ObjectId) -> Option<ObjectId>` closure or function,
/// and `&dyn Renumber` / `Box<dyn Renumber>`.
///
/// ```
/// use std::collections::BTreeMap;
/// use pdf_objects::{Object, ObjectId, dict, serialize};
///
/// let page = Object::from(dict! {
///     "Parent" => ObjectId::new(2, 0),
///     "Annots" => ObjectId::new(9, 0),
/// });
/// let map = BTreeMap::from([(ObjectId::new(2, 0), ObjectId::new(1, 0))]);
/// let mut out = Vec::new();
/// serialize::indirect_renumbered(
///     &mut out,
///     ObjectId::new(3, 0),
///     &page,
///     &(),
///     &|id| map.get(&id).copied(),
/// )
/// .unwrap();
/// assert_eq!(out, b"3 0 obj\n<</Parent 1 0 R/Annots null>>\nendobj\n");
/// ```
pub trait Renumber {
    /// Maps `id` to its serialized target ID, or `None` to emit `null`.
    #[must_use]
    fn renumber(&self, id: ObjectId) -> Option<ObjectId>;
}

impl Renumber for () {
    #[inline]
    fn renumber(&self, id: ObjectId) -> Option<ObjectId> {
        Some(id)
    }
}

impl<F: Fn(ObjectId) -> Option<ObjectId> + ?Sized> Renumber for F {
    #[inline]
    fn renumber(&self, id: ObjectId) -> Option<ObjectId> {
        self(id)
    }
}

impl Renumber for &(dyn Renumber + '_) {
    #[inline]
    fn renumber(&self, id: ObjectId) -> Option<ObjectId> {
        (**self).renumber(id)
    }
}

impl Renumber for &(dyn Renumber + Send + '_) {
    #[inline]
    fn renumber(&self, id: ObjectId) -> Option<ObjectId> {
        (**self).renumber(id)
    }
}

impl Renumber for &(dyn Renumber + Send + Sync + '_) {
    #[inline]
    fn renumber(&self, id: ObjectId) -> Option<ObjectId> {
        (**self).renumber(id)
    }
}

impl Renumber for Box<dyn Renumber + '_> {
    #[inline]
    fn renumber(&self, id: ObjectId) -> Option<ObjectId> {
        (**self).renumber(id)
    }
}

impl Renumber for Box<dyn Renumber + Send + '_> {
    #[inline]
    fn renumber(&self, id: ObjectId) -> Option<ObjectId> {
        (**self).renumber(id)
    }
}

impl Renumber for Box<dyn Renumber + Send + Sync + '_> {
    #[inline]
    fn renumber(&self, id: ObjectId) -> Option<ObjectId> {
        (**self).renumber(id)
    }
}

trait Through: Copy {
    type Error: From<Error>;
    fn clear(self) -> Self;
    fn is_clear_key(self, dict: &Dict, key: &[u8]) -> bool;
    fn string<'t>(self, text: &'t [u8]) -> Result<Cow<'t, [u8]>, Self::Error>;
    fn stream<'d>(self, dict: &Dict, data: &'d [u8]) -> Result<Cow<'d, [u8]>, Self::Error>;
}

struct Plain<E>(PhantomData<fn() -> E>);

impl<E> Plain<E> {
    const NEW: Plain<E> = Plain(PhantomData);
}

impl<E> Clone for Plain<E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<E> Copy for Plain<E> {}

impl<E: From<Error>> Through for Plain<E> {
    type Error = E;

    #[inline]
    fn clear(self) -> Self {
        self
    }

    #[inline]
    fn is_clear_key(self, _: &Dict, _: &[u8]) -> bool {
        false
    }

    #[inline]
    fn string<'t>(self, text: &'t [u8]) -> Result<Cow<'t, [u8]>, E> {
        Ok(Cow::Borrowed(text))
    }

    #[inline]
    fn stream<'d>(self, _: &Dict, data: &'d [u8]) -> Result<Cow<'d, [u8]>, E> {
        Ok(Cow::Borrowed(data))
    }
}

struct Crypt<'e, E: ?Sized> {
    encrypt: Option<&'e E>,
    id: ObjectId,
}

impl<E: ?Sized> Clone for Crypt<'_, E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<E: ?Sized> Copy for Crypt<'_, E> {}

impl<E: Encrypt + ?Sized> Through for Crypt<'_, E> {
    type Error = Refused<E::Error>;

    #[inline]
    fn clear(self) -> Self {
        Crypt {
            encrypt: None,
            id: self.id,
        }
    }

    #[inline]
    fn is_clear_key(self, dict: &Dict, key: &[u8]) -> bool {
        self.encrypt
            .is_some_and(|enc| enc.is_clear_key(self.id, dict, key))
    }

    #[inline]
    fn string<'t>(self, text: &'t [u8]) -> Result<Cow<'t, [u8]>, Self::Error> {
        match self.encrypt {
            Some(enc) => enc
                .string(self.id, text)
                .map_err(|why| Refused::Encrypt { id: self.id, why }),
            None => Ok(Cow::Borrowed(text)),
        }
    }

    #[inline]
    fn stream<'d>(self, dict: &Dict, data: &'d [u8]) -> Result<Cow<'d, [u8]>, Self::Error> {
        match self.encrypt {
            Some(enc) => enc
                .stream(self.id, dict, data)
                .map_err(|why| Refused::Encrypt { id: self.id, why }),
            None => Ok(Cow::Borrowed(data)),
        }
    }
}

fn object_frame<R: From<Error>>(
    out: &mut Vec<u8>,
    id: ObjectId,
    body: impl FnOnce(&mut Vec<u8>) -> Result<(), R>,
) -> Result<usize, R> {
    if id.number == 0 {
        return Err(Error::ZeroObjectId { id: Some(id) }.into());
    }
    let start = out.len();
    reference(out, id);
    out.extend_from_slice(b" obj\n");
    match body(out) {
        Ok(()) => {
            out.extend_from_slice(b"\nendobj\n");
            Ok(start)
        }
        Err(e) => {
            out.truncate(start);
            Err(e)
        }
    }
}

/// Writes `obj` as a direct PDF object into `out`.
///
/// # Errors
///
/// Returns [`Error`] and leaves `out` unchanged if `obj` contains a non-finite or out-of-range
/// real, exceeds [`Limits::DEFAULT`] nesting, contains [`Str::Encrypted`], contains a NUL byte in
/// a name, emits reference `0`, or contains a [`Value::Stream`].
pub fn write(out: &mut Vec<u8>, obj: &Object) -> Result<(), Error> {
    write_renumbered(out, obj, &())
}

/// Writes `obj` as a direct PDF object into `out`, remapping references through `to`.
///
/// # Errors
///
/// Same as [`write()`]; leaves `out` unchanged on error.
pub fn write_renumbered<N: Renumber + ?Sized>(
    out: &mut Vec<u8>,
    obj: &Object,
    to: &N,
) -> Result<(), Error> {
    let start = out.len();
    value(out, obj, false, Plain::<Error>::NEW, to).inspect_err(|_| out.truncate(start))
}

/// Writes `obj` as indirect object `id` (`n g obj\n...\nendobj\n`), returning its start offset in
/// `out`.
///
/// # Errors
///
/// Returns [`Error`] (attributed to `id`) and leaves `out` unchanged if `id.number == 0` or `obj`
/// cannot be serialized.
pub fn indirect(out: &mut Vec<u8>, id: ObjectId, obj: &Object) -> Result<usize, Error> {
    let plain = Plain::<Error>::NEW;
    object_frame(out, id, |out| value(out, obj, true, plain, &())).map_err(|e| e.within(id))
}

/// Writes `obj` as indirect object `id`, encrypting strings and stream data through `encrypt`.
///
/// # Errors
///
/// Returns [`Refused::Write`] or [`Refused::Encrypt`] and leaves `out` unchanged on failure.
pub fn indirect_with<E: Encrypt + ?Sized>(
    out: &mut Vec<u8>,
    id: ObjectId,
    obj: &Object,
    encrypt: &E,
) -> Result<usize, Refused<E::Error>> {
    indirect_renumbered(out, id, obj, encrypt, &())
}

/// Writes `obj` as indirect object `id`, encrypting through `encrypt` and remapping references
/// through `to`.
///
/// `id` is the serialized object number (not passed through `to`) and is passed to `encrypt` along
/// with `obj`'s un-renumbered dictionary when present.
///
/// # Errors
///
/// Same as [`indirect_with`]; leaves `out` unchanged on failure.
pub fn indirect_renumbered<E: Encrypt + ?Sized, N: Renumber + ?Sized>(
    out: &mut Vec<u8>,
    id: ObjectId,
    obj: &Object,
    encrypt: &E,
    to: &N,
) -> Result<usize, Refused<E::Error>> {
    if id.number == 0 {
        return Err(Refused::Write(Error::ZeroObjectId { id: Some(id) }));
    }
    let (dict, is_stream) = match obj.value() {
        Value::Dict(d) => (Some(d), false),
        Value::Stream(s) => (Some(&s.dict), true),
        _ => (None, false),
    };
    let written = if encrypt.clear(id, dict, is_stream) {
        object_frame(out, id, |out| value(out, obj, true, Plain::NEW, to))
    } else {
        let crypt = Crypt {
            encrypt: Some(encrypt),
            id,
        };
        object_frame(out, id, |out| value(out, obj, true, crypt, to))
    };
    written.map_err(|e| e.within(id))
}

/// Writes a stream of `dict` and out-of-line `data` as indirect object `id`, returning its start
/// offset in `out`.
///
/// `data` is written verbatim (already filter-encoded) with `/Length` set to `data.len()`.
///
/// # Errors
///
/// Returns [`Error`] (attributed to `id`) and leaves `out` unchanged if `id.number == 0` or
/// `dict` cannot be serialized.
pub fn indirect_stream(
    out: &mut Vec<u8>,
    id: ObjectId,
    dict: &Dict,
    data: &[u8],
) -> Result<usize, Error> {
    let plain = Plain::<Error>::NEW;
    object_frame(out, id, |out| stream(out, dict, data, plain, &())).map_err(|e| e.within(id))
}

/// Writes a stream of `dict` and plaintext filter-encoded `data` as indirect object `id`,
/// encrypting through `encrypt`.
///
/// # Errors
///
/// Same as [`indirect_with`]; leaves `out` unchanged on failure.
pub fn indirect_stream_with<E: Encrypt + ?Sized>(
    out: &mut Vec<u8>,
    id: ObjectId,
    dict: &Dict,
    data: &[u8],
    encrypt: &E,
) -> Result<usize, Refused<E::Error>> {
    indirect_stream_renumbered(out, id, dict, data, encrypt, &())
}

/// Writes a stream of `dict` and `data` as indirect object `id`, encrypting through `encrypt` and
/// remapping references in `dict` through `to`.
///
/// # Errors
///
/// Same as [`indirect_with`]; leaves `out` unchanged on failure.
pub fn indirect_stream_renumbered<E: Encrypt + ?Sized, N: Renumber + ?Sized>(
    out: &mut Vec<u8>,
    id: ObjectId,
    dict: &Dict,
    data: &[u8],
    encrypt: &E,
    to: &N,
) -> Result<usize, Refused<E::Error>> {
    if id.number == 0 {
        return Err(Refused::Write(Error::ZeroObjectId { id: Some(id) }));
    }
    let written = if encrypt.clear(id, Some(dict), true) {
        object_frame(out, id, |out| stream(out, dict, data, Plain::NEW, to))
    } else {
        let crypt = Crypt {
            encrypt: Some(encrypt),
            id,
        };
        object_frame(out, id, |out| stream(out, dict, data, crypt, to))
    };
    written.map_err(|e| e.within(id))
}

struct StackBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> StackBuf<N> {
    const fn new() -> Self {
        StackBuf {
            buf: [0; N],
            len: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    fn extend(&mut self, bytes: &[u8]) -> Result<(), fmt::Error> {
        if self.len + bytes.len() > N {
            return Err(fmt::Error);
        }
        self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
}

impl<const N: usize> fmt::Write for StackBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.extend(s.as_bytes())
    }
}

/// Formats `x` in shortest fixed-point decimal notation (always containing `.` and never an
/// exponent), up to 255 bytes. `-0.0` normalizes to `0.0`.
///
/// # Errors
///
/// Returns [`Error::NonFinite`] for `NaN` or infinities, and [`Error::RealOutOfRange`] if the
/// fixed-point representation exceeds 255 bytes. Leaves `out` unchanged on error.
pub fn real(out: &mut Vec<u8>, x: f64) -> Result<(), Error> {
    if !x.is_finite() {
        return Err(Error::NonFinite { id: None });
    }
    if x == 0.0 {
        out.extend_from_slice(b"0.0");
        return Ok(());
    }
    // Fast path for exact integral reals within ±2^53.
    if x > -9_007_199_254_740_992.0 && x < 9_007_199_254_740_992.0 && (x as i64 as f64) == x {
        integer(out, x as i64);
        out.extend_from_slice(b".0");
        return Ok(());
    }
    let mut buf = StackBuf::<260>::new();
    if write!(&mut buf, "{x}").is_err() {
        return Err(Error::RealOutOfRange { id: None });
    }
    if !buf.as_bytes().contains(&b'.') && buf.extend(b".0").is_err() {
        return Err(Error::RealOutOfRange { id: None });
    }
    if buf.len > MAX_REAL_LEN {
        return Err(Error::RealOutOfRange { id: None });
    }
    out.extend_from_slice(buf.as_bytes());
    Ok(())
}

/// Formats `n` in unsigned base-10 ASCII digits.
pub fn decimal(out: &mut Vec<u8>, mut n: u64) {
    let mut digits = [0u8; 20];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[at..]);
}

fn integer(out: &mut Vec<u8>, n: i64) {
    if n < 0 {
        out.push(b'-');
    }
    decimal(out, n.unsigned_abs());
}

fn reference(out: &mut Vec<u8>, id: ObjectId) {
    decimal(out, id.number.into());
    out.push(b' ');
    decimal(out, id.generation.into());
}

#[inline]
fn hex(out: &mut Vec<u8>, b: u8) {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    out.extend_from_slice(&[DIGITS[usize::from(b >> 4)], DIGITS[usize::from(b & 15)]]);
}

/// Writes `/` followed by `bytes`, escaping `#`, whitespace, delimiters, and non-ASCII bytes as
/// `#XX`.
///
/// # Errors
///
/// Returns [`Error::NulName`] and leaves `out` unchanged if `bytes` contains `0x00`.
pub fn name(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), Error> {
    if bytes.iter().all(|&b| REGULAR[usize::from(b)]) {
        out.reserve(1 + bytes.len());
        out.push(b'/');
        out.extend_from_slice(bytes);
        return Ok(());
    }
    if memchr::memchr(0, bytes).is_some() {
        return Err(Error::NulName { id: None });
    }
    out.reserve(1 + bytes.len());
    out.push(b'/');
    for &b in bytes {
        if REGULAR[usize::from(b)] {
            out.push(b);
        } else {
            out.push(b'#');
            hex(out, b);
        }
    }
    Ok(())
}

/// Writes `text` as a PDF string in `form`, escaping `\`, `\r`, and any unbalanced parentheses in
/// [`Form::Literal`].
pub fn string(out: &mut Vec<u8>, text: &[u8], form: Form) {
    match form {
        Form::Literal => {
            let mut has_backslash_or_cr = false;
            let mut depth = 0usize;
            let mut unmatched_close = false;
            for &b in text {
                match b {
                    b'\\' | b'\r' => has_backslash_or_cr = true,
                    b'(' => depth += 1,
                    b')' if depth > 0 => depth -= 1,
                    b')' => unmatched_close = true,
                    _ => {}
                }
            }
            let balanced = depth == 0 && !unmatched_close;
            if !has_backslash_or_cr && balanced {
                out.reserve(2 + text.len());
                out.push(b'(');
                out.extend_from_slice(text);
                out.push(b')');
                return;
            }
            if balanced {
                out.reserve(2 + text.len() + 4);
                out.push(b'(');
                for &b in text {
                    match b {
                        b'\\' => out.extend_from_slice(b"\\\\"),
                        b'\r' => out.extend_from_slice(b"\\r"),
                        _ => out.push(b),
                    }
                }
                out.push(b')');
                return;
            }
            // Slow path for strings with unbalanced parentheses: every index in `lone` is pushed
            // when `open` is empty, so `lone` followed by remaining `open` is already sorted.
            let mut open = Vec::new();
            let mut lone = Vec::new();
            for (i, &b) in text.iter().enumerate() {
                match b {
                    b'(' => open.push(i),
                    b')' if open.pop().is_none() => lone.push(i),
                    _ => {}
                }
            }
            lone.extend(open);
            debug_assert!(lone.windows(2).all(|w| w[0] < w[1]));
            let mut lone = lone.into_iter().peekable();
            out.reserve(2 + text.len() + lone.len());
            out.push(b'(');
            for (i, &b) in text.iter().enumerate() {
                match b {
                    b'\\' => out.extend_from_slice(b"\\\\"),
                    b'\r' => out.extend_from_slice(b"\\r"),
                    b'(' | b')' if lone.next_if_eq(&i).is_some() => {
                        out.extend_from_slice(&[b'\\', b]);
                    }
                    _ => out.push(b),
                }
            }
            out.push(b')');
        }
        Form::Hex => {
            out.reserve(2 + text.len().saturating_mul(2));
            out.push(b'<');
            for &b in text {
                hex(out, b);
            }
            out.push(b'>');
        }
    }
}

enum Frame<'a, T> {
    Array {
        iter: slice::Iter<'a, Object>,
        first: bool,
        through: T,
    },
    Dict {
        dict: &'a Dict,
        iter: slice::Iter<'a, Entry>,
        stream_len: Option<usize>,
        wrote_len: bool,
        through: T,
    },
}

/// Iterative heap-stack serializer for an `Object`.
fn value<T: Through, N: Renumber + ?Sized>(
    out: &mut Vec<u8>,
    root: &Object,
    allow_top_stream: bool,
    root_through: T,
    to: &N,
) -> Result<(), T::Error> {
    if let Value::Stream(s) = root.value() {
        if !allow_top_stream {
            return Err(Error::InlineStream { id: None }.into());
        }
        let Some(data) = s.data.bytes() else {
            return Err(Error::Data { id: None }.into());
        };
        return stream(out, &s.dict, data, root_through, to);
    }

    let mut frames: Vec<Frame<'_, T>> = Vec::new();
    let mut next: Option<(&Object, T)> = Some((root, root_through));
    run_frames(out, &mut frames, &mut next, to)
}

fn stream<T: Through, N: Renumber + ?Sized>(
    out: &mut Vec<u8>,
    dict: &Dict,
    data: &[u8],
    through: T,
    to: &N,
) -> Result<(), T::Error> {
    let data = through.stream(dict, data)?;
    let payload_len = data.len();
    out.reserve(payload_len.saturating_add(48));
    out.extend_from_slice(b"<<");
    let mut frames = Vec::with_capacity(4);
    frames.push(Frame::Dict {
        dict,
        iter: dict.entries().iter(),
        stream_len: Some(payload_len),
        wrote_len: false,
        through,
    });
    let mut next = None;
    run_frames(out, &mut frames, &mut next, to)?;
    out.extend_from_slice(b"\nstream\n");
    out.extend_from_slice(&data);
    out.extend_from_slice(b"\nendstream");
    Ok(())
}

fn run_frames<'a, T: Through, N: Renumber + ?Sized>(
    out: &mut Vec<u8>,
    frames: &mut Vec<Frame<'a, T>>,
    next: &mut Option<(&'a Object, T)>,
    to: &N,
) -> Result<(), T::Error> {
    let limit = Limits::DEFAULT.nesting;

    loop {
        if let Some((obj, through)) = next.take() {
            if frames.len() > limit {
                return Err(Error::Nested { id: None, limit }.into());
            }
            match obj.value() {
                Value::Null => out.extend_from_slice(b"null"),
                Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
                Value::Integer(i) => integer(out, *i),
                Value::Real(x) => real(out, *x)?,
                Value::Name(n) => name(out, n.as_bytes())?,
                Value::String(Str::Bytes(bytes), form) => {
                    let encrypted = through.string(bytes.as_slice())?;
                    string(out, &encrypted, form.unwrap_or(Form::Literal));
                }
                Value::String(Str::Encrypted(_), _) => {
                    return Err(Error::Encrypted { id: None }.into());
                }
                Value::Array(items) => {
                    if items.is_empty() {
                        out.extend_from_slice(b"[]");
                    } else {
                        out.push(b'[');
                        frames.push(Frame::Array {
                            iter: items.iter(),
                            first: true,
                            through,
                        });
                    }
                }
                Value::Dict(d) => {
                    if d.is_empty() {
                        out.extend_from_slice(b"<<>>");
                    } else {
                        out.extend_from_slice(b"<<");
                        frames.push(Frame::Dict {
                            dict: d,
                            iter: d.entries().iter(),
                            stream_len: None,
                            wrote_len: false,
                            through,
                        });
                    }
                }
                Value::Stream(_) => return Err(Error::InlineStream { id: None }.into()),
                Value::Ref(id) => match to.renumber(*id) {
                    Some(mapped) => {
                        if mapped.number == 0 {
                            return Err(Error::ZeroObjectId { id: None }.into());
                        }
                        reference(out, mapped);
                        out.extend_from_slice(b" R");
                    }
                    None => out.extend_from_slice(b"null"),
                },
            }
            continue;
        }

        let Some(top) = frames.last_mut() else {
            return Ok(());
        };

        match top {
            Frame::Array {
                iter,
                first,
                through,
            } => match iter.next() {
                Some(item) => {
                    if !*first && spaced(item) {
                        out.push(b' ');
                    }
                    *first = false;
                    *next = Some((item, *through));
                }
                None => {
                    out.push(b']');
                    frames.pop();
                }
            },
            Frame::Dict {
                dict,
                iter,
                stream_len,
                wrote_len,
                through,
            } => match iter.next() {
                Some(entry) => {
                    let key = entry.key();
                    name(out, key)?;
                    if let Some(n) = *stream_len
                        && key == b"Length"
                    {
                        out.push(b' ');
                        decimal(out, n as u64);
                        *wrote_len = true;
                        continue;
                    }
                    let val = entry.value();
                    if spaced(val) {
                        out.push(b' ');
                    }
                    let child_through = if through.is_clear_key(dict, key) {
                        through.clear()
                    } else {
                        *through
                    };
                    *next = Some((val, child_through));
                }
                None => {
                    if let Some(n) = *stream_len
                        && !*wrote_len
                    {
                        out.extend_from_slice(b"/Length ");
                        decimal(out, n as u64);
                    }
                    out.extend_from_slice(b">>");
                    frames.pop();
                }
            },
        }
    }
}

#[inline]
fn spaced(obj: &Object) -> bool {
    matches!(
        obj.value(),
        Value::Null | Value::Bool(_) | Value::Integer(_) | Value::Real(_) | Value::Ref(_)
    )
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;
    use alloc::string::ToString;
    use alloc::vec;

    use super::*;
    use crate::dict;
    use crate::fuzz;
    use crate::object::{Data, Span, Stream};
    use crate::parse::{Body, Parser};

    struct Objects(BTreeMap<ObjectId, Object>);

    impl Objects {
        fn get(&self, id: ObjectId) -> Option<&Object> {
            self.0.get(&id)
        }
    }

    fn open(bytes: &[u8]) -> Objects {
        let mut objects = BTreeMap::new();
        let mut at = 0;
        while at < bytes.len() {
            let mut parser = Parser::at(bytes, at);
            let id = parser.header(&mut ()).unwrap();
            let mut obj = match parser.body(id, &mut ()).unwrap() {
                Body::Object(obj) => obj,
                Body::Stream(head) => {
                    let length = head.dict().get("Length").and_then(Object::as_integer);
                    let data = head.declared(length.unwrap() as usize).unwrap();
                    parser.stream(head, data, &mut ()).unwrap()
                }
            };
            fuzz::unplace(&mut obj, bytes);
            assert_eq!(bytes.get(parser.offset()), Some(&b'\n'));
            at = parser.offset() + 1;
            objects.insert(id, obj);
        }
        Objects(objects)
    }

    fn file(objects: &[(u32, Object)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (num, obj) in objects {
            indirect(&mut out, ObjectId::new(*num, 0), obj).unwrap();
        }
        out
    }

    fn round_trip(obj: &Object) -> Object {
        open(&file(&[(2, obj.clone())]))
            .get(ObjectId::new(2, 0))
            .unwrap()
            .clone()
    }

    fn written(obj: &Object) -> String {
        let mut out = Vec::new();
        write(&mut out, obj).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn reals_round_trip_without_clamping_across_f32_and_f64() {
        let kept = [
            0.1,
            1.0 / 3.0,
            595.32,
            1e-7,
            1e20,
            3.4e38,
            -3.4e38,
            1.2e-38,
            1e100,
            -1e100,
            1e-100,
            -1e-100,
            612.0,
            -612.0,
            0.0,
        ]
        .map(Object::from)
        .into_iter()
        .chain(
            [
                0.1f32,
                1.0 / 3.0,
                595.32,
                841.89,
                1e20,
                f32::MAX,
                f32::MIN,
                f32::MIN_POSITIVE,
                1e-45f32,
                -1e-45f32,
            ]
            .map(Object::from),
        );

        let wrote: Vec<Object> = kept.clone().chain([Object::from(-0.0)]).collect();
        let want: Vec<f64> = kept.map(|o| o.as_number().unwrap()).chain([0.0]).collect();

        let Value::Array(back) = round_trip(&Object::from(wrote)).into_value() else {
            panic!("not an array");
        };
        assert_eq!(back.len(), want.len());
        for (back, want) in back.iter().zip(want) {
            let Value::Real(x) = back.value() else {
                panic!("{want} read back as {back:?}");
            };
            assert_eq!(x.to_bits(), want.to_bits(), "{want} read back as {x}");
        }

        assert_eq!(written(&Object::from(612.0)), "612.0");
        assert_eq!(written(&Object::from(-0.0)), "0.0");
        assert_eq!(written(&Object::from(1e-7)), "0.0000001");
        assert_eq!(written(&Object::from(0.1f32)), "0.1");
        assert_eq!(written(&Object::from(1e20)), "100000000000000000000.0");
        assert_eq!(
            written(&Object::from(1e-45f32)),
            "0.000000000000000000000000000000000000000000001"
        );
    }

    #[test]
    fn non_finite_and_out_of_range_reals_are_refused() {
        for x in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut out = b"%PDF-1.7\n".to_vec();
            assert_eq!(
                write(&mut out, &Object::from(x)),
                Err(Error::NonFinite { id: None })
            );
            assert_eq!(
                indirect(&mut out, ObjectId::new(7, 0), &Object::from(x)),
                Err(Error::NonFinite {
                    id: Some(ObjectId::new(7, 0))
                })
            );
            assert_eq!(out, b"%PDF-1.7\n");
        }

        for extreme in [
            f64::MAX,
            f64::MIN,
            f64::MIN_POSITIVE,
            5e-324,
            1e260,
            -1e-260,
        ] {
            let mut out = b"%PDF-1.7\n".to_vec();
            assert_eq!(
                write(&mut out, &Object::from(extreme)),
                Err(Error::RealOutOfRange { id: None }),
                "{extreme:e}"
            );
            assert_eq!(
                indirect(&mut out, ObjectId::new(9, 0), &Object::from(extreme)),
                Err(Error::RealOutOfRange {
                    id: Some(ObjectId::new(9, 0))
                })
            );
            assert_eq!(out, b"%PDF-1.7\n");
        }
    }

    #[test]
    fn inline_streams_nul_names_and_zero_object_ids_are_refused() {
        let mut out = b"%PDF-1.7\n".to_vec();
        let stream_obj = Object::from(Stream::new(dict! {}, b"abc".to_vec()));
        assert_eq!(
            write(&mut out, &stream_obj),
            Err(Error::InlineStream { id: None })
        );
        let nested_stream = Object::from(vec![stream_obj]);
        assert_eq!(
            indirect(&mut out, ObjectId::new(3, 0), &nested_stream),
            Err(Error::InlineStream {
                id: Some(ObjectId::new(3, 0))
            })
        );

        let nul_name = Object::name(b"A\0B");
        assert_eq!(write(&mut out, &nul_name), Err(Error::NulName { id: None }));
        assert_eq!(name(&mut out, b"A\0B"), Err(Error::NulName { id: None }));
        let nul_key = Object::from(dict! { b"K\0".as_slice() => 1 });
        assert_eq!(
            indirect(&mut out, ObjectId::new(4, 0), &nul_key),
            Err(Error::NulName {
                id: Some(ObjectId::new(4, 0))
            })
        );

        assert_eq!(
            indirect(&mut out, ObjectId::new(0, 0), &Object::null()),
            Err(Error::ZeroObjectId {
                id: Some(ObjectId::new(0, 0))
            })
        );
        assert_eq!(
            indirect_stream(&mut out, ObjectId::new(0, 1), &dict! {}, b""),
            Err(Error::ZeroObjectId {
                id: Some(ObjectId::new(0, 1))
            })
        );
        let zero_ref = Object::from(ObjectId::new(0, 0));
        assert_eq!(
            write(&mut out, &zero_ref),
            Err(Error::ZeroObjectId { id: None })
        );
        assert_eq!(
            indirect(&mut out, ObjectId::new(5, 0), &zero_ref),
            Err(Error::ZeroObjectId {
                id: Some(ObjectId::new(5, 0))
            })
        );
        assert_eq!(out, b"%PDF-1.7\n");

        // Renumbering 0 -> None emits `null` cleanly.
        let mut cleaned = Vec::new();
        write_renumbered(&mut cleaned, &zero_ref, &|id: ObjectId| {
            (id.number != 0).then_some(id)
        })
        .unwrap();
        assert_eq!(cleaned, b"null");
    }

    #[test]
    fn strings_names_and_streams_round_trip() {
        let literal = |s: &[u8]| Object::new(Value::String(Str::bytes(s), Some(Form::Literal)));
        let hex = |s: &[u8]| Object::hex(s);
        let nm = |s: &[u8]| Object::name(s);
        let values = Object::from(vec![
            literal(b"balanced (parens (nest)) as they are"),
            literal(b"balanced (parens) with \\ and \r"),
            literal(b"unbalanced ) and ( and ((x)"),
            literal(b"a back\\slash, and \\) escaped"),
            literal(b"CR\r alone, CR LF\r\n and LF\n"),
            literal(b"\x00\x80\xff binary"),
            literal(b""),
            hex(b"\x00\x01()\\\r"),
            hex(b""),
            nm(b"A B"),
            nm(b"#23 and /slash"),
            nm(b"\t\x7f\x80\xff"),
            nm(b"()<>[]{}%"),
            nm(b"Plain-Name_1.0"),
            nm(b""),
            nm(b"After"),
            Object::from(dict! {
                "True" => true,
                "Null" => Object::null(),
                "Ref" => ObjectId::new(3, 0),
                "Neg" => -1,
                "Mixed" => vec![
                    nm(b"A"),
                    nm(b"B"),
                    Object::from(1),
                    Object::from(2.5),
                    Object::from(false),
                    Object::null(),
                    Object::from(ObjectId::new(3, 0)),
                    literal(b"s"),
                ],
            }),
        ]);
        let content = b"\x00binary\nendstream\nendobj\n\xff".to_vec();
        let mut long = Stream::new(dict! { "Kind" => Object::name("Test") }, content.clone());
        long.dict.set("Length", 999);
        let mut unsaid = Stream::new(dict! {}, content.clone());
        unsaid.dict.remove("Length");
        let mut ind_len = Stream::new(dict! {}, content.clone());
        ind_len
            .dict
            .set("Length", Object::from(ObjectId::new(9, 0)));
        let pdf = open(&file(&[
            (2, values.clone()),
            (3, Object::from(long)),
            (4, Object::from(unsaid)),
            (5, Object::from(ind_len)),
        ]));
        assert_eq!(pdf.get(ObjectId::new(2, 0)).unwrap(), &values);
        for id in [
            ObjectId::new(3, 0),
            ObjectId::new(4, 0),
            ObjectId::new(5, 0),
        ] {
            let s = pdf.get(id).unwrap().as_stream().unwrap();
            assert_eq!(s.data.bytes(), Some(content.as_slice()), "{id:?}");
            let length = s.dict.get("Length").unwrap();
            assert_eq!(length, &Object::from(content.len() as i64), "{id:?}");
        }
        assert_eq!(
            pdf.get(ObjectId::new(3, 0))
                .unwrap()
                .as_stream()
                .unwrap()
                .dict
                .get("Kind")
                .unwrap(),
            &nm(b"Test")
        );
    }

    #[test]
    fn numbers_names_and_strings_write_as_formatting_would() {
        let bytes = |obj: &Object| {
            let mut out = Vec::new();
            write(&mut out, obj).unwrap();
            out
        };

        let mut dec = Vec::new();
        decimal(&mut dec, u64::MAX);
        assert_eq!(dec, format!("{}", u64::MAX).into_bytes());

        let mut integers = vec![i64::MAX, i64::MIN, i64::MIN + 1, u32::MAX.into()];
        for power in 0..19 {
            let ten = 10i64.pow(power);
            integers.extend([ten - 1, ten, ten + 1, -ten, 1 - ten]);
        }
        for i in integers {
            assert_eq!(written(&Object::from(i)), format!("{i}"));
        }
        for id in [
            ObjectId::new(1, 0),
            ObjectId::new(7, 0),
            ObjectId::new(10, 1),
            ObjectId::new(u32::MAX, u16::MAX),
        ] {
            let (n, g) = (id.number, id.generation);
            assert_eq!(written(&Object::from(id)), format!("{n} {g} R"));
            let mut out = Vec::new();
            indirect(&mut out, id, &Object::null()).unwrap();
            assert_eq!(out, format!("{n} {g} obj\nnull\nendobj\n").into_bytes());
        }
        let non_nul: Vec<u8> = (1..=255).collect();
        let mut expected_name = b"/".to_vec();
        for &b in &non_nul {
            match b.is_ascii_graphic() && !b"#()<>[]{}/%".contains(&b) {
                true => expected_name.push(b),
                false => expected_name.extend(format!("#{b:02X}").bytes()),
            }
        }
        assert_eq!(bytes(&Object::name(non_nul.as_slice())), expected_name);
        assert_eq!(written(&Object::name(b"Plain-Name_1.0")), "/Plain-Name_1.0");
        let every: Vec<u8> = (0..=255).collect();
        let hex_str: String = every.iter().map(|b| format!("{b:02X}")).collect();
        let mk_str = |text: &[u8], form| Object::new(Value::String(Str::bytes(text), Some(form)));
        assert_eq!(written(&mk_str(&every, Form::Hex)), format!("<{hex_str}>"));
        let plain: Vec<u8> = every
            .iter()
            .copied()
            .filter(|b| !b"\\\r()".contains(b))
            .collect();
        let want = [b"(".as_slice(), &plain, b")"].concat();
        assert_eq!(bytes(&mk_str(&plain, Form::Literal)), want);
        assert_eq!(
            written(&mk_str(b"a\\b\rc(d) e)", Form::Literal)),
            "(a\\\\b\\rc(d) e\\))"
        );

        let mut stream = Stream::new(dict! {}, vec![0; 1234]);
        let head = |s: &Stream| {
            let mut out = Vec::new();
            indirect(&mut out, ObjectId::new(1, 0), &Object::from(s.clone())).unwrap();
            out[8..24].to_vec()
        };
        assert_eq!(head(&stream), b"<</Length 1234>>");
        stream.dict.remove("Length");
        assert_eq!(head(&stream), b"<</Length 1234>>");
    }

    #[test]
    fn a_string_with_no_form_is_written_literal() {
        assert_eq!(written(&Object::bytes(*b"a (b")), "(a \\(b)");
        assert_eq!(written(&Object::bytes(*b"\x01")), "(\x01)");
        let back = round_trip(&Object::bytes(*b"x"));
        assert_eq!(
            *back.value(),
            Value::String(Str::bytes(*b"x"), Some(Form::Literal))
        );
    }

    #[test]
    fn each_refusal_rolls_back_output() {
        let ciphertext = Object::new(Value::String(Str::encrypted(*b"x"), Some(Form::Hex)));
        let at = Object::from(Stream {
            dict: dict! { "Length" => 4 },
            data: Data::At(Span::new(None, 0, 4)),
        });
        let unplaced_stream = Object::stream(dict! {});
        let mut deep = Object::from(1);
        for _ in 0..=Limits::DEFAULT.nesting {
            deep = Object::from(vec![deep]);
        }

        let mut out = b"%PDF-1.7\n".to_vec();
        for s in [&at, &unplaced_stream] {
            assert_eq!(
                indirect(&mut out, ObjectId::new(7, 2), s),
                Err(Error::Data {
                    id: Some(ObjectId::new(7, 2))
                })
            );
            assert_eq!(write(&mut out, s), Err(Error::InlineStream { id: None }));
        }
        assert_eq!(out, b"%PDF-1.7\n");

        for (refused, error) in [
            (ciphertext, Error::Encrypted { id: None }),
            (
                deep,
                Error::Nested {
                    id: None,
                    limit: 256,
                },
            ),
        ] {
            let within = error.within(ObjectId::new(7, 2));
            let after = Object::from(vec![
                Object::from(1),
                Object::from(dict! { "A" => 2, "B" => refused.clone() }),
            ]);
            for obj in [&refused, &after] {
                assert_eq!(write(&mut out, obj), Err(error));
                assert_eq!(indirect(&mut out, ObjectId::new(7, 2), obj), Err(within));
            }
            let d = dict! { "B" => refused };
            assert_eq!(
                indirect_stream(&mut out, ObjectId::new(7, 2), &d, b""),
                Err(within)
            );
            assert_eq!(out, b"%PDF-1.7\n");
        }
    }

    #[test]
    fn a_streams_data_given_apart_is_written_as_it_would_be_held() {
        let data = b"\x00binary\nendstream\n".to_vec();
        let d = dict! {
            "Kind" => Object::name("Test"),
            "Length" => Object::from(ObjectId::new(9, 0)),
        };
        let held = Object::from(Stream {
            dict: d.clone(),
            data: Data::Bytes(data.clone()),
        });
        let mut written_buf = b"%PDF-1.7\n".to_vec();
        assert_eq!(
            indirect(&mut written_buf, ObjectId::new(4, 1), &held),
            Ok(9)
        );
        let mut apart = b"%PDF-1.7\n".to_vec();
        assert_eq!(
            indirect_stream(&mut apart, ObjectId::new(4, 1), &d, &data),
            Ok(9)
        );
        assert_eq!(apart, written_buf);
        assert!(
            apart.ends_with(b"/Length 18>>\nstream\n\x00binary\nendstream\n\nendstream\nendobj\n")
        );

        let mut parser = Parser::at(&apart, 9);
        let id = parser.header(&mut ()).unwrap();
        let Ok(Body::Stream(head)) = parser.body(id, &mut ()) else {
            panic!("a stream");
        };
        let declared = head.declared(data.len()).unwrap();
        let read = parser.stream(head, declared, &mut ()).unwrap();
        let s = read.as_stream().unwrap();
        assert_eq!(
            indirect(&mut Vec::new(), id, &read),
            Err(Error::Data { id: Some(id) })
        );
        let mut again = b"%PDF-1.7\n".to_vec();
        let slice = s.data.at().unwrap().slice_in(&apart).unwrap();
        indirect_stream(&mut again, id, &s.dict, slice).unwrap();
        assert_eq!(again, apart);
    }

    #[test]
    fn a_key_written_twice_is_written_twice() {
        let d = dict! { "A" => 1, "B" => 2, "A" => 3 };
        assert_eq!(written(&Object::from(d.clone())), "<</A 1/B 2/A 3>>");
        assert_eq!(round_trip(&Object::from(d.clone())), Object::from(d));
        let s = Stream {
            dict: dict! {
                "Length" => 1,
                "Filter" => Object::name("FlateDecode"),
                "Length" => 2,
            },
            data: Data::Bytes(b"data".to_vec()),
        };
        let mut out = Vec::new();
        indirect(&mut out, ObjectId::new(1, 0), &Object::from(s)).unwrap();
        assert_eq!(
            out,
            b"1 0 obj\n<</Length 4/Filter/FlateDecode/Length 4>>\nstream\ndata\nendstream\nendobj\n"
        );
    }

    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, below: usize) -> usize {
            (self.next() % below as u64) as usize
        }

        fn bytes(&mut self, most: usize, allow_nul: bool) -> Vec<u8> {
            const SPECIAL: &[u8] = b"\\()\r\n\t #/%<>[]{}\x00\x7f\x80\xff";
            (0..self.below(most + 1))
                .map(|_| {
                    let b = match self.below(2) {
                        0 => SPECIAL[self.below(SPECIAL.len())],
                        _ => self.next() as u8,
                    };
                    if !allow_nul && b == 0 { b'_' } else { b }
                })
                .collect()
        }

        fn integer(&mut self) -> i64 {
            match self.below(4) {
                0 => [0, 1, -1, i64::MAX, i64::MIN, i64::MIN + 1][self.below(6)],
                1 => self.below(2000) as i64 - 1000,
                _ => self.next() as i64,
            }
        }

        fn real(&mut self) -> f64 {
            let bounds = [3.4e38f64, 1.175e-38, 1e-45];
            match self.below(4) {
                0 => {
                    let x = bounds[self.below(3)];
                    let x = match self.below(3) {
                        0 => f64::from_bits(x.to_bits() - 1),
                        1 => x,
                        _ => f64::from_bits(x.to_bits() + 1),
                    };
                    [x, -x][self.below(2)]
                }
                1 => [
                    0.0,
                    -0.0,
                    f64::from(f32::MAX),
                    f64::from(f32::MIN),
                    f64::from(1e-45f32),
                    1e100,
                    1e-100,
                    0.1,
                ][self.below(8)],
                2 => self.integer() as f64 / 10u64.pow(self.below(8) as u32) as f64,
                _ => loop {
                    let x = f64::from_bits(self.next());
                    if x.is_finite() && super::real(&mut Vec::new(), x).is_ok() {
                        break x;
                    }
                },
            }
        }

        fn value(&mut self, budget: &mut usize, depth: usize) -> Object {
            *budget = budget.saturating_sub(1);
            let kinds = if *budget > 0 && depth > 0 { 9 } else { 7 };
            match self.below(kinds) {
                0 => match self.below(3) {
                    0 => Object::null(),
                    n => Object::from(n == 1),
                },
                1 => Object::from(self.integer()),
                2 => Object::from(self.real()),
                3 => Object::name(self.bytes(12, false)),
                4 => {
                    let form = [None, Some(Form::Literal), Some(Form::Hex)][self.below(3)];
                    Object::new(Value::String(Str::bytes(self.bytes(24, true)), form))
                }
                5 => {
                    let raw = (self.next() as u32).max(1);
                    let n = [1, 2, u32::MAX, raw][self.below(4)];
                    let g = [0, 1, u16::MAX, self.next() as u16][self.below(4)];
                    Object::from(ObjectId::new(n, g))
                }
                6 => Object::from(Vec::new()),
                7 => Object::from(self.array(budget, depth - 1)),
                _ => Object::from(self.dict(budget, depth - 1)),
            }
        }

        fn array(&mut self, budget: &mut usize, depth: usize) -> Vec<Object> {
            (0..self.below(8))
                .map(|_| self.value(budget, depth))
                .collect()
        }

        fn dict(&mut self, budget: &mut usize, depth: usize) -> Dict {
            let mut d = Dict::new();
            for _ in 0..self.below(8) {
                let key = match (self.below(6), d.entries().last()) {
                    (0, Some(entry)) => entry.key().to_vec(),
                    (1, _) => b"Length".to_vec(),
                    _ => self.bytes(8, false),
                };
                d.push(key, self.value(budget, depth));
            }
            d
        }

        fn deep(&mut self, depth: usize) -> Object {
            let mut inner = match self.below(3) {
                0 => Object::from(Vec::new()),
                1 => Object::from(Dict::new()),
                _ => self.value(&mut 1, 0),
            };
            for _ in depth..Limits::DEFAULT.nesting {
                let beside = self.value(&mut 1, 0);
                inner = match self.below(2) {
                    0 => Object::from(vec![beside, inner]),
                    _ => Object::from(dict! { "A" => beside, "B" => inner }),
                };
            }
            inner
        }

        fn stream(&mut self, budget: &mut usize, deep: bool) -> Object {
            let mut data = self.bytes(64, true);
            if self.below(4) == 0 {
                data.extend_from_slice(b"\nendstream\n");
            }
            let mut dict = self.dict(budget, 3);
            if deep {
                dict.push("Deep", self.deep(1));
            }
            Object::from(Stream {
                dict,
                data: Data::Bytes(data),
            })
        }

        fn object(&mut self) -> Object {
            let deep = self.below(100) == 0;
            let most = self.below(300);
            let mut budget = 1 + self.below(1 + most);
            match (self.below(8), deep) {
                (0, _) => self.stream(&mut budget, deep),
                (_, true) => self.deep(0),
                (1..=3, _) => Object::from(self.dict(&mut budget, 5)),
                (4 | 5, _) => Object::from(self.array(&mut budget, 5)),
                _ => self.value(&mut budget, 6),
            }
        }

        fn id(&mut self) -> ObjectId {
            ObjectId::new((self.next() as u32).max(1), self.next() as u16)
        }
    }

    #[test]
    fn values_written_read_back_as_written_and_write_the_same_bytes_again() {
        let mut random = Random(0x9e37_79b9_7f4a_7c15);
        for _ in 0..20_000 {
            let object = random.object();
            let id = random.id();
            fuzz::round_trip(&object, Some(id), &[], None, Limits::DEFAULT);
            if object.as_stream().is_none() {
                fuzz::round_trip(&object, None, &[], None, Limits::DEFAULT);
            }
        }
    }

    fn leaves(o: &Object) -> usize {
        match o.value() {
            Value::Array(a) if !a.is_empty() => a.iter().map(leaves).sum(),
            Value::Dict(d) if !d.is_empty() => d.entries().iter().map(|e| leaves(e.value())).sum(),
            Value::Stream(s) => s
                .dict
                .entries()
                .iter()
                .filter(|e| e.key() != b"Length")
                .map(|e| leaves(e.value()))
                .sum(),
            _ => 1,
        }
    }

    fn plant(o: &mut Object, n: &mut usize, replacement: &Value) {
        match o.value_mut() {
            Value::Array(a) if !a.is_empty() => a.iter_mut().for_each(|e| plant(e, n, replacement)),
            Value::Dict(d) if !d.is_empty() => {
                d.entries_mut()
                    .for_each(|e| plant(e.value_mut(), n, replacement));
            }
            Value::Stream(s) => s
                .dict
                .entries_mut()
                .filter(|e| e.key() != b"Length")
                .for_each(|e| plant(e.value_mut(), n, replacement)),
            leaf => {
                if *n == 0 {
                    *leaf = replacement.clone();
                }
                *n = n.wrapping_sub(1);
            }
        }
    }

    fn planted(random: &mut Random, object: &Object, string: Str) -> Object {
        let mut object = object.clone();
        let val = Value::String(string, [None, Some(Form::Hex)][random.below(2)]);
        match leaves(&object) {
            0 => match object.as_stream_mut() {
                Some(s) => s.dict.push("S", Object::new(val)),
                None => unreachable!("only a stream holds no leaf"),
            },
            n => plant(&mut object, &mut random.below(n), &val),
        }
        object
    }

    #[test]
    fn values_holding_ciphertext_are_refused_wherever_it_is() {
        let ciphertext = Str::encrypted(*b"\x8f\x02");
        let mut random = Random(0x2545_f491_4f6c_dd1d);
        for _ in 0..2_000 {
            let base = random.object();
            let object = planted(&mut random, &base, ciphertext.clone());
            let id = random.id();
            let mut out = b"%PDF-1.7\n".to_vec();
            if object.as_stream().is_none() {
                assert_eq!(write(&mut out, &object), Err(Error::Encrypted { id: None }));
            }
            assert_eq!(
                indirect(&mut out, id, &object),
                Err(Error::Encrypted { id: Some(id) })
            );
            if let Some(s) = object.as_stream() {
                let data = s.data.bytes().unwrap();
                let apart = indirect_stream(&mut out, id, &s.dict, data);
                assert_eq!(apart, Err(Error::Encrypted { id: Some(id) }));
            }
            assert_eq!(out, b"%PDF-1.7\n");
        }
    }

    #[test]
    fn an_encryptor_is_given_every_string_and_streams_data_once_as_its_objects() {
        let mut random = Random(0x6a09_e667_f3bc_c908);
        let mut given_count = 0;
        for _ in 0..5_000 {
            let object = random.object();
            let id = random.id();
            let mut plain = Vec::new();
            indirect(&mut plain, id, &object).unwrap();
            let mut unit = Vec::new();
            assert_eq!(indirect_with(&mut unit, id, &object, &()), Ok(0));
            assert_eq!(unit, plain);
            fuzz::toyed(&object, id, &[], Limits::DEFAULT, &plain);
            let toy = fuzz::Toy::default();
            indirect_with(&mut Vec::new(), id, &object, &toy).unwrap();
            given_count += toy.seen.borrow().len();
            if let Some(s) = object.as_stream() {
                let data = s.data.bytes().unwrap();
                let mut apart = Vec::new();
                indirect_stream(&mut apart, id, &s.dict, data).unwrap();
                assert_eq!(apart, plain);
                let mut unit = Vec::new();
                indirect_stream_with(&mut unit, id, &s.dict, data, &()).unwrap();
                assert_eq!(unit, plain);
                let (mut held, mut given) = (Vec::new(), Vec::new());
                let toy = fuzz::Toy::default();
                let boxed_toy: Box<dyn Encrypt<Error = &'static str>> =
                    Box::new(fuzz::Toy::default());
                let _ = indirect_with(&mut Vec::new(), id, &object, &boxed_toy).unwrap();
                indirect_with(&mut held, id, &object, &toy).unwrap();
                let toy_apart = fuzz::Toy::default();
                indirect_stream_with(&mut given, id, &s.dict, data, &toy_apart).unwrap();
                assert_eq!(given, held);
                assert_eq!(toy_apart.seen, toy.seen);
            }
        }
        assert!(given_count > 2_000, "{given_count} given");
    }

    #[test]
    fn what_an_encryptor_gives_is_written_and_a_streams_length_is_its() {
        let toy = fuzz::Toy::default();
        let id = ObjectId::new(7, 1);
        let hex = Object::hex(*b"ab");
        let d = dict! { "Length" => 1, "S" => Object::text("x(y"), "H" => hex, "Length" => 2 };
        let mut out = Vec::new();
        indirect_stream_with(&mut out, id, &d, b"data", &toy).unwrap();
        let seen = toy.seen.borrow();
        let order = [
            (id, b"data".to_vec(), true),
            (id, b"x(y".to_vec(), false),
            (id, b"ab".to_vec(), false),
        ];
        assert_eq!(*seen, order);
        let tag = "\0\0\0\x07\0\x01";
        let want = format!(
            "7 1 obj\n<</Length 10/S({tag}y\\(x)/H<0000000700016261>/Length 10>>\n\
             stream\n{tag}atad\nendstream\nendobj\n"
        );
        assert_eq!(String::from_utf8_lossy(&out), want);
    }

    #[test]
    fn what_an_encryptor_leaves_in_the_clear_is_written_as_it_is() {
        let signature = dict! {
            "Contents" => Object::hex(*b"\x30\x82"),
            "ByteRange" => vec![Object::from(0), Object::from(4)],
            "M" => Object::text("D:2026"),
            "Prop_Build" => Object::text("Acrobat"),
            "Contents" => vec![Object::text("in the clear")],
        };
        let plain_of = |id, obj: &Object| {
            let mut out = Vec::new();
            indirect(&mut out, id, obj).unwrap();
            out
        };
        let xref = dict! {
            "Type" => Object::name("XRef"),
            "ID" => vec![Object::text("an /ID")],
        };
        let xref_stream = Object::from(Stream::new(xref.clone(), vec![1, 0, 0]));
        for (id, obj) in [
            (ObjectId::new(10, 0), Object::from(signature.clone())),
            (ObjectId::new(3, 0), xref_stream),
        ] {
            let toy = fuzz::Toy::default();
            let mut out = Vec::new();
            indirect_with(&mut out, id, &obj, &toy).unwrap();
            assert_eq!(out, plain_of(id, &obj), "{id:?}");
            assert!(toy.seen.borrow().is_empty(), "{id:?}");
        }
        // Multiple clear keys (`Contents` and `Prop_Build`) via `is_clear_key`.
        let toy = fuzz::Toy::default();
        let id = ObjectId::new(3, 0);
        let mut out = Vec::new();
        indirect_with(&mut out, id, &Object::from(signature), &toy).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("/Contents<3082>"), "{text}");
        assert!(text.contains("/Contents[(in the clear)]"), "{text}");
        assert!(text.contains("/Prop_Build(Acrobat)"), "{text}");
        assert!(!text.contains("D:2026"), "{text}");
        assert_eq!(*toy.seen.borrow(), [(id, b"D:2026".to_vec(), false)]);

        let toy = fuzz::Toy::default();
        indirect_with(&mut Vec::new(), id, &Object::from(xref), &toy).unwrap();
        assert_eq!(*toy.seen.borrow(), [(id, b"an /ID".to_vec(), false)]);
    }

    #[test]
    fn an_encryptors_refusal_writes_nothing() {
        let toy = fuzz::Toy {
            refuse: Some(b"secret".to_vec()),
            ..fuzz::Toy::default()
        };
        let id = ObjectId::new(7, 2);
        let deep = Object::from(vec![
            Object::from(1),
            Object::from(dict! { "A" => Object::text("secret") }),
        ]);
        let refused = Refused::Encrypt {
            id,
            why: "the toy refuses it",
        };
        let mut out = b"%PDF-1.7\n".to_vec();
        assert_eq!(
            indirect_with(&mut out, id, &deep, &toy),
            Err(refused.clone())
        );
        let stream = dict! { "S" => Object::text("not refused") };
        assert_eq!(
            indirect_stream_with(&mut out, id, &stream, b"secret", &toy),
            Err(refused.clone())
        );
        let nan = dict! { "S" => Object::text("x"), "N" => f64::NAN };
        assert_eq!(
            indirect_with(&mut out, id, &Object::from(nan), &toy),
            Err(Refused::Write(Error::NonFinite { id: Some(id) }))
        );
        assert_eq!(out, b"%PDF-1.7\n");
        assert_eq!(
            refused.to_string(),
            "object 7 2 can't be encrypted: the toy refuses it"
        );
    }

    #[test]
    fn values_nest_as_deep_as_the_parser_reads_them_and_no_deeper() {
        let limit = Limits::DEFAULT.nesting;
        type Wrap = fn(Object) -> Object;
        let wraps: [(Wrap, &[u8], &[u8]); 2] = [
            (|o| Object::from(vec![o]), b"[", b"]"),
            (|o| Object::from(dict! { "K" => o }), b"<</K ", b">>"),
        ];
        for inner in [Object::from(1), Object::from(Vec::new())] {
            for (wrap, open, close) in wraps {
                let mut deepest = inner.clone();
                for _ in 0..limit {
                    deepest = wrap(deepest);
                }
                let mut out = Vec::new();
                write(&mut out, &deepest).unwrap();
                let mut read = Parser::at(&out, 0).value(&mut ()).unwrap();
                fuzz::unplace(&mut read, &out);
                assert_eq!(read, deepest);
                assert!(indirect(&mut Vec::new(), ObjectId::new(1, 0), &deepest).is_ok());

                let past = wrap(deepest);
                let nested = Error::Nested { id: None, limit };
                assert_eq!(write(&mut Vec::new(), &past), Err(nested));
                assert_eq!(
                    indirect(&mut Vec::new(), ObjectId::new(1, 0), &past),
                    Err(nested.within(ObjectId::new(1, 0)))
                );
                let text = [open, &out, close].concat();
                let mut parser = Parser::at(&text, 0);
                let refused = parser.value(&mut ());
                let Err(crate::parse::ErrorKind::Nested { limit: refused, .. }) =
                    refused.map_err(crate::parse::Error::into_kind)
                else {
                    panic!("expected Nested error");
                };
                assert_eq!(refused, limit);
                assert_eq!(parser.offset(), 0);
            }
        }
    }

    #[test]
    fn each_reference_is_written_as_the_renumbering_names_it() {
        let to = |id: ObjectId| match id.number {
            1 => Some(ObjectId::new(5, 2)),
            2 => None,
            _ => Some(id),
        };
        let refs = vec![
            Object::from(ObjectId::new(1, 0)),
            Object::from(ObjectId::new(2, 0)),
            Object::from(7),
        ];
        let obj = Object::from(dict! {
            "A" => ObjectId::new(1, 0),
            "B" => ObjectId::new(2, 0),
            "C" => ObjectId::new(3, 0),
            "K" => refs,
        });
        let renumbered = |obj: &Object| {
            let mut out = Vec::new();
            write_renumbered(&mut out, obj, &to).unwrap();
            String::from_utf8(out).unwrap()
        };
        assert_eq!(
            renumbered(&obj),
            "<</A 5 2 R/B null/C 3 0 R/K[5 2 R null 7]>>"
        );
        assert_eq!(renumbered(&Object::from(ObjectId::new(2, 0))), "null");
        assert_eq!(renumbered(&Object::from(ObjectId::new(1, 0))), "5 2 R");

        let dyn_fn: &dyn Fn(ObjectId) -> Option<ObjectId> = &to;
        let dyn_renumber: &dyn Renumber = &to;
        let boxed_renumber: Box<dyn Renumber> = Box::new(to);
        for mapper in [&dyn_fn as &dyn Renumber, &dyn_renumber, &boxed_renumber] {
            let mut out = Vec::new();
            write_renumbered(&mut out, &obj, mapper).unwrap();
            assert_eq!(String::from_utf8(out).unwrap(), renumbered(&obj));
        }

        let mut out = Vec::new();
        assert_eq!(
            indirect_renumbered(&mut out, ObjectId::new(1, 0), &obj, &(), &to),
            Ok(0)
        );
        let want = format!("1 0 obj\n{}\nendobj\n", renumbered(&obj));
        assert_eq!(String::from_utf8(out).unwrap(), want);

        let d = dict! {
            "Length" => ObjectId::new(1, 0),
            "F" => ObjectId::new(1, 0),
            "G" => vec![Object::from(ObjectId::new(2, 0))],
        };
        let mut out = Vec::new();
        indirect_stream_renumbered(&mut out, ObjectId::new(2, 0), &d, b"1 0 R", &(), &to).unwrap();
        let want = "2 0 obj\n<</Length 5/F 5 2 R/G[null]>>\nstream\n1 0 R\nendstream\nendobj\n";
        assert_eq!(String::from_utf8(out).unwrap(), want);
    }

    #[test]
    fn renumbering_nothing_writes_what_the_writers_without_one_write() {
        let mut random = Random(0x3c6e_f372_fe94_f82b);
        let same = |id| Some(id);
        for _ in 0..5_000 {
            let object = random.object();
            let id = random.id();
            if object.as_stream().is_none() {
                let mut plain = Vec::new();
                write(&mut plain, &object).unwrap();
                for to in [&() as &dyn Renumber, &same] {
                    let mut out = Vec::new();
                    write_renumbered(&mut out, &object, to).unwrap();
                    assert_eq!(out, plain);
                }
            }
            let mut plain = Vec::new();
            indirect(&mut plain, id, &object).unwrap();
            let toy = fuzz::Toy::default();
            let mut sealed = Vec::new();
            indirect_with(&mut sealed, id, &object, &toy).unwrap();
            for to in [&() as &dyn Renumber, &same] {
                let mut out = Vec::new();
                indirect_renumbered(&mut out, id, &object, &(), to).unwrap();
                assert_eq!(out, plain);
                let through = fuzz::Toy::default();
                let mut out = Vec::new();
                indirect_renumbered(&mut out, id, &object, &through, to).unwrap();
                assert_eq!(out, sealed);
                assert_eq!(through.seen, toy.seen);
            }
            let Some(s) = object.as_stream() else {
                continue;
            };
            let data = s.data.bytes().unwrap();
            let mut apart = Vec::new();
            indirect_stream(&mut apart, id, &s.dict, data).unwrap();
            let toy = fuzz::Toy::default();
            let mut sealed = Vec::new();
            indirect_stream_with(&mut sealed, id, &s.dict, data, &toy).unwrap();
            for to in [&() as &dyn Renumber, &same] {
                let mut out = Vec::new();
                indirect_stream_renumbered(&mut out, id, &s.dict, data, &(), to).unwrap();
                assert_eq!(out, apart);
                let through = fuzz::Toy::default();
                let mut out = Vec::new();
                indirect_stream_renumbered(&mut out, id, &s.dict, data, &through, to).unwrap();
                assert_eq!(out, sealed);
                assert_eq!(through.seen, toy.seen);
            }
        }
    }

    #[test]
    fn values_written_renumbered_read_back_as_renumbered_in_memory() {
        let mut random = Random(0xbb67_ae85_84ca_a73b);
        let mut changed = 0;
        for _ in 0..20_000 {
            let object = random.object();
            let id = random.id();
            let seed = match random.below(8) {
                0 => 0,
                _ => random.next() as u16,
            };
            let to = fuzz::renumbering(seed);
            let mut moved = object.clone();
            changed += usize::from(moved.renumber(&to));
            let plain = fuzz::round_trip(&object, Some(id), &[], None, Limits::DEFAULT);
            fuzz::renumbered(&object, Some(id), &[], None, Limits::DEFAULT, &to, &plain);
            if object.as_stream().is_none() {
                let plain = fuzz::round_trip(&object, None, &[], None, Limits::DEFAULT);
                fuzz::renumbered(&object, None, &[], None, Limits::DEFAULT, &to, &plain);
            }
        }
        assert!(changed > 5_000, "{changed} changed");
    }

    #[test]
    fn an_encryptor_is_given_the_number_an_object_is_written_at() {
        let to = |id: ObjectId| {
            Some(if id.number == 3 {
                ObjectId::new(8, 0)
            } else {
                id
            })
        };
        let page = Object::from(dict! { "Self" => ObjectId::new(3, 0), "S" => Object::text("ab") });
        let toy = fuzz::Toy::default();
        let mut out = Vec::new();
        indirect_renumbered(&mut out, ObjectId::new(8, 0), &page, &toy, &to).unwrap();
        let tag = "\0\0\0\x08\0\0";
        let want = format!("8 0 obj\n<</Self 8 0 R/S({tag}ba)>>\nendobj\n");
        assert_eq!(String::from_utf8_lossy(&out), want);
        assert_eq!(
            *toy.seen.borrow(),
            [(ObjectId::new(8, 0), b"ab".to_vec(), false)]
        );
        let toy = fuzz::Toy::default();
        let mut out = Vec::new();
        let d = dict! { "Self" => ObjectId::new(3, 0) };
        indirect_stream_renumbered(&mut out, ObjectId::new(8, 0), &d, b"cd", &toy, &to).unwrap();
        assert_eq!(
            *toy.seen.borrow(),
            [(ObjectId::new(8, 0), b"cd".to_vec(), true)]
        );
        let toy = fuzz::Toy::default();
        let mut out = Vec::new();
        indirect_renumbered(&mut out, ObjectId::new(10, 0), &page, &toy, &to).unwrap();
        let want = "10 0 obj\n<</Self 8 0 R/S(ab)>>\nendobj\n";
        assert_eq!(String::from_utf8_lossy(&out), want);
        assert!(toy.seen.borrow().is_empty());
    }
}
