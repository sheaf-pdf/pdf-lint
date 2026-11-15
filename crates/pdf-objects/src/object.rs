//! Owned PDF objects, inline-optimized names/strings, span tracking, and exact dictionaries
//! (ISO 32000-2 §7.3).
//!
//! Every [`Object`] pairs a [`Value`] with an optional [`Span`] recording its byte range in the
//! source file or decoded object stream (`size_of::<Object>() == 64` on 64-bit targets).
//! Mutating an object via [`Object::value_mut`] or `as_*_mut` clears the span on the edited node
//! and its containers while preserving untouched siblings' spans.
//!
//! Recursive operations—`Drop`, `Clone`, `PartialEq`, [`Object::eq_value`],
//! [`Object::references`], [`Object::renumber`], and [`Object::unplace`]—use explicit heap stacks
//! so arbitrarily deep structures never overflow the call stack.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::String;
#[cfg(target_has_atomic = "ptr")]
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::borrow::Borrow;
use core::cmp::Ordering;
use core::fmt::{self, Write as _};
use core::hash::{Hash, Hasher};
use core::iter::{Enumerate, FusedIterator};
use core::ops::{Deref, Range};
use core::slice;

use crate::text;

/// Indirect object identifier (`number` and `generation`, ISO 32000-2 §7.3.10).
///
/// Ordered and hashed by `(number, generation)`. `Display` formats as `"{number} {generation}"`,
/// so `format!("{id} R")` produces an indirect reference and `format!("{id} obj")` an object
/// header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId {
    /// Positive object number (`0` is reserved for the free-list head in xref tables).
    pub number: u32,
    /// Generation number (`0..=65535`).
    pub generation: u16,
}

impl ObjectId {
    /// Creates an [`ObjectId`] for `number` and `generation`.
    #[must_use]
    pub const fn new(number: u32, generation: u16) -> ObjectId {
        ObjectId { number, generation }
    }
}

impl From<(u32, u16)> for ObjectId {
    fn from((number, generation): (u32, u16)) -> ObjectId {
        ObjectId { number, generation }
    }
}

impl From<ObjectId> for (u32, u16) {
    fn from(id: ObjectId) -> (u32, u16) {
        (id.number, id.generation)
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.number, self.generation)
    }
}

/// Source byte range of a parsed PDF token, value, or stream payload.
///
/// When [`Span::stream`] is `None`, [`Span::start`] and [`Span::end`] are byte offsets in the
/// outer PDF file. When [`Span::stream`] is `Some(id)`, they are byte offsets inside the decoded
/// payload of object stream `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    stream: Option<ObjectId>,
    start: usize,
    end: usize,
}

impl Span {
    /// Creates a [`Span`] covering `start..end` in the file (`stream: None`) or in object stream
    /// `stream`.
    ///
    /// Debug-asserts `start <= end`; use [`Span::try_new`] when offsets come from untrusted
    /// arithmetic.
    #[must_use]
    pub const fn new(stream: Option<ObjectId>, start: usize, end: usize) -> Span {
        debug_assert!(start <= end, "Span::new requires start <= end");
        Span { stream, start, end }
    }

    /// Creates a [`Span`] if `start <= end`, returning `None` otherwise.
    #[must_use]
    pub const fn try_new(stream: Option<ObjectId>, start: usize, end: usize) -> Option<Span> {
        if start <= end {
            Some(Span { stream, start, end })
        } else {
            None
        }
    }

    /// Enclosing object stream [`ObjectId`], or `None` for top-level file bytes.
    #[must_use]
    #[inline]
    pub const fn stream(&self) -> Option<ObjectId> {
        self.stream
    }

    /// Byte offset of the first byte in the span.
    #[must_use]
    #[inline]
    pub const fn start(&self) -> usize {
        self.start
    }

    /// Byte offset one past the last byte in the span.
    #[must_use]
    #[inline]
    pub const fn end(&self) -> usize {
        self.end
    }

    /// `self.start()..self.end()` as a [`Range`].
    #[must_use]
    #[inline]
    pub const fn range(&self) -> Range<usize> {
        self.start..self.end
    }

    /// Length of the span in bytes.
    #[must_use]
    #[inline]
    pub const fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Returns `true` if the span covers zero bytes.
    #[must_use]
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    /// Slices `file` at `self.range()` when this span refers to top-level file bytes
    /// (`self.stream().is_none()`).
    ///
    /// Returns `None` if `self.stream().is_some()` or if the range exceeds `file.len()`.
    #[must_use]
    #[inline]
    pub fn slice_in<'a>(&self, file: &'a [u8]) -> Option<&'a [u8]> {
        if self.stream.is_some() {
            return None;
        }
        file.get(self.start..self.end)
    }

    /// Slices `decoded` at `self.range()` when `self.stream() == Some(stream_id)`.
    #[must_use]
    #[inline]
    pub fn stream_slice_in<'a>(&self, stream_id: ObjectId, decoded: &'a [u8]) -> Option<&'a [u8]> {
        if self.stream != Some(stream_id) {
            return None;
        }
        decoded.get(self.start..self.end)
    }
}

/// Compact 16-byte immutable byte buffer optimized for PDF names and short strings.
///
/// Stores `0..=14` bytes inline with zero heap allocations, `15..=64` bytes in a single thin
/// 64-byte allocation, and larger buffers behind a reference-counted slice for $O(1)$ cloning.
#[derive(Clone)]
pub struct SmallBytes(BytesRepr);

#[derive(Clone)]
enum BytesRepr {
    Inline {
        len: u8,
        buf: [u8; 14],
    },
    Medium {
        len: u8,
        buf: Box<[u8; 64]>,
    },
    #[cfg(target_has_atomic = "ptr")]
    Large(Arc<Box<[u8]>>),
    #[cfg(not(target_has_atomic = "ptr"))]
    Large(Box<Box<[u8]>>),
}

impl SmallBytes {
    /// An empty [`SmallBytes`] buffer.
    pub const EMPTY: SmallBytes = SmallBytes(BytesRepr::Inline {
        len: 0,
        buf: [0; 14],
    });

    /// Creates an empty [`SmallBytes`] buffer without allocating.
    #[must_use]
    pub const fn new() -> SmallBytes {
        SmallBytes::EMPTY
    }

    /// Copies `bytes` into a [`SmallBytes`] buffer.
    #[must_use]
    pub fn from_slice(bytes: &[u8]) -> SmallBytes {
        if bytes.len() <= 14 {
            let mut buf = [0u8; 14];
            buf[..bytes.len()].copy_from_slice(bytes);
            SmallBytes(BytesRepr::Inline {
                len: bytes.len() as u8,
                buf,
            })
        } else if bytes.len() <= 64 {
            let mut buf = Box::new([0u8; 64]);
            buf[..bytes.len()].copy_from_slice(bytes);
            SmallBytes(BytesRepr::Medium {
                len: bytes.len() as u8,
                buf,
            })
        } else {
            let boxed = bytes.to_vec().into_boxed_slice();
            SmallBytes::from_large(boxed)
        }
    }

    /// Converts an owned `Vec<u8>` into [`SmallBytes`], reusing its heap buffer when longer than
    /// 64 bytes.
    #[must_use]
    pub fn from_vec(bytes: Vec<u8>) -> SmallBytes {
        if bytes.len() <= 64 {
            SmallBytes::from_slice(&bytes)
        } else {
            SmallBytes::from_large(bytes.into_boxed_slice())
        }
    }

    /// Collects an iterator of decoded bytes into [`SmallBytes`] using a 64-byte stack buffer
    /// before spilling to `Vec`.
    pub(crate) fn from_iter_bounded<I: IntoIterator<Item = u8>>(
        iter: I,
        cap_hint: usize,
    ) -> SmallBytes {
        let mut iter = iter.into_iter();
        let mut stack = [0u8; 64];
        let mut len = 0usize;
        while len < 64 {
            let Some(b) = iter.next() else {
                return SmallBytes::from_slice(&stack[..len]);
            };
            stack[len] = b;
            len += 1;
        }
        let Some(next) = iter.next() else {
            return SmallBytes(BytesRepr::Medium {
                len: 64,
                buf: Box::new(stack),
            });
        };
        let mut vec = Vec::with_capacity(cap_hint.max(128));
        vec.extend_from_slice(&stack);
        vec.push(next);
        vec.extend(iter);
        SmallBytes::from_large(vec.into_boxed_slice())
    }

    #[cfg(target_has_atomic = "ptr")]
    fn from_large(boxed: Box<[u8]>) -> SmallBytes {
        SmallBytes(BytesRepr::Large(Arc::new(boxed)))
    }

    #[cfg(not(target_has_atomic = "ptr"))]
    fn from_large(boxed: Box<[u8]>) -> SmallBytes {
        SmallBytes(BytesRepr::Large(Box::new(boxed)))
    }

    /// Views the buffer as a byte slice.
    #[must_use]
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        match &self.0 {
            BytesRepr::Inline { len, buf } => &buf[..usize::from(*len)],
            BytesRepr::Medium { len, buf } => &buf[..usize::from(*len)],
            BytesRepr::Large(buf) => buf,
        }
    }

    /// Length in bytes.
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// Returns `true` if empty.
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Converts into an owned `Vec<u8>`, reusing uniquely owned large allocations without copying.
    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        match self.0 {
            BytesRepr::Inline { len, buf } => buf[..usize::from(len)].to_vec(),
            BytesRepr::Medium { len, buf } => buf[..usize::from(len)].to_vec(),
            #[cfg(target_has_atomic = "ptr")]
            BytesRepr::Large(arc) => match Arc::try_unwrap(arc) {
                Ok(boxed) => boxed.into_vec(),
                Err(shared) => shared.as_ref().to_vec(),
            },
            #[cfg(not(target_has_atomic = "ptr"))]
            BytesRepr::Large(boxed) => (*boxed).into_vec(),
        }
    }
}

impl Default for SmallBytes {
    fn default() -> SmallBytes {
        SmallBytes::EMPTY
    }
}

impl Deref for SmallBytes {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl AsRef<[u8]> for SmallBytes {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Borrow<[u8]> for SmallBytes {
    #[inline]
    fn borrow(&self) -> &[u8] {
        self.as_slice()
    }
}

impl fmt::Debug for SmallBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_capped_bytes(f, self.as_slice())
    }
}

impl PartialEq for SmallBytes {
    #[inline]
    fn eq(&self, other: &SmallBytes) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Eq for SmallBytes {}

impl PartialOrd for SmallBytes {
    #[inline]
    fn partial_cmp(&self, other: &SmallBytes) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SmallBytes {
    #[inline]
    fn cmp(&self, other: &SmallBytes) -> Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

impl Hash for SmallBytes {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

impl PartialEq<[u8]> for SmallBytes {
    #[inline]
    fn eq(&self, other: &[u8]) -> bool {
        self.as_slice() == other
    }
}

impl PartialEq<&[u8]> for SmallBytes {
    #[inline]
    fn eq(&self, other: &&[u8]) -> bool {
        self.as_slice() == *other
    }
}

impl<const N: usize> PartialEq<[u8; N]> for SmallBytes {
    #[inline]
    fn eq(&self, other: &[u8; N]) -> bool {
        self.as_slice() == other
    }
}

impl<const N: usize> PartialEq<&[u8; N]> for SmallBytes {
    #[inline]
    fn eq(&self, other: &&[u8; N]) -> bool {
        self.as_slice() == *other
    }
}

impl PartialEq<Vec<u8>> for SmallBytes {
    #[inline]
    fn eq(&self, other: &Vec<u8>) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl PartialEq<str> for SmallBytes {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_slice() == other.as_bytes()
    }
}

impl PartialEq<&str> for SmallBytes {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.as_slice() == other.as_bytes()
    }
}

impl PartialEq<SmallBytes> for [u8] {
    #[inline]
    fn eq(&self, other: &SmallBytes) -> bool {
        self == other.as_slice()
    }
}

impl PartialEq<SmallBytes> for &[u8] {
    #[inline]
    fn eq(&self, other: &SmallBytes) -> bool {
        *self == other.as_slice()
    }
}

impl From<&[u8]> for SmallBytes {
    fn from(bytes: &[u8]) -> SmallBytes {
        SmallBytes::from_slice(bytes)
    }
}

impl<const N: usize> From<&[u8; N]> for SmallBytes {
    fn from(bytes: &[u8; N]) -> SmallBytes {
        SmallBytes::from_slice(bytes)
    }
}

impl<const N: usize> From<[u8; N]> for SmallBytes {
    fn from(bytes: [u8; N]) -> SmallBytes {
        SmallBytes::from_slice(&bytes)
    }
}

impl From<Vec<u8>> for SmallBytes {
    fn from(bytes: Vec<u8>) -> SmallBytes {
        SmallBytes::from_vec(bytes)
    }
}

impl From<&Vec<u8>> for SmallBytes {
    fn from(bytes: &Vec<u8>) -> SmallBytes {
        SmallBytes::from_slice(bytes)
    }
}

impl From<Box<[u8]>> for SmallBytes {
    fn from(bytes: Box<[u8]>) -> SmallBytes {
        if bytes.len() <= 64 {
            SmallBytes::from_slice(&bytes)
        } else {
            SmallBytes::from_large(bytes)
        }
    }
}

impl From<&str> for SmallBytes {
    fn from(s: &str) -> SmallBytes {
        SmallBytes::from_slice(s.as_bytes())
    }
}

impl From<String> for SmallBytes {
    fn from(s: String) -> SmallBytes {
        SmallBytes::from_vec(s.into_bytes())
    }
}

impl From<&String> for SmallBytes {
    fn from(s: &String) -> SmallBytes {
        SmallBytes::from_slice(s.as_bytes())
    }
}

impl From<Cow<'_, [u8]>> for SmallBytes {
    fn from(cow: Cow<'_, [u8]>) -> SmallBytes {
        match cow {
            Cow::Borrowed(b) => SmallBytes::from_slice(b),
            Cow::Owned(v) => SmallBytes::from_vec(v),
        }
    }
}

impl FromIterator<u8> for SmallBytes {
    fn from_iter<T: IntoIterator<Item = u8>>(iter: T) -> SmallBytes {
        let iter = iter.into_iter();
        let (low, _) = iter.size_hint();
        SmallBytes::from_iter_bounded(iter, low)
    }
}

impl From<SmallBytes> for Vec<u8> {
    fn from(bytes: SmallBytes) -> Vec<u8> {
        bytes.into_vec()
    }
}

/// Decoded PDF name object bytes without the leading `/` (ISO 32000-2 §7.3.5).
///
/// Stores standard PDF keys (`Type`, `Subtype`, `BaseFont`, `FontDescriptor`, `Resources`,
/// `MediaBox`, `Length`, `Filter`, `FlateDecode`, etc.) inline in 16 bytes with zero heap
/// allocations.
///
/// Unlike strings, [`Name`] converts directly into [`Object`] via `Object::from(Name::new("Page"))`
/// or [`Object::name`]; plain `&str` and `String` do not implement `Into<Object>` so text strings
/// ([`Object::text`]) and names ([`Object::name`]) cannot be confused.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(SmallBytes);

impl Name {
    /// The empty name (`/`).
    pub const EMPTY: Name = Name(SmallBytes::EMPTY);

    /// Creates a [`Name`] from raw decoded bytes (without the leading `/`).
    #[must_use]
    pub fn new(bytes: impl Into<SmallBytes>) -> Name {
        Name(bytes.into())
    }

    /// Wraps an existing [`SmallBytes`] as a [`Name`].
    #[must_use]
    pub const fn from_small_bytes(bytes: SmallBytes) -> Name {
        Name(bytes)
    }

    /// Returns the decoded name bytes.
    #[must_use]
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }

    /// Views the name as UTF-8 if its decoded bytes are valid UTF-8.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        core::str::from_utf8(self.as_bytes()).ok()
    }

    /// Unwraps the inner [`SmallBytes`].
    #[must_use]
    #[inline]
    pub fn into_bytes(self) -> SmallBytes {
        self.0
    }

    /// Converts the name into an owned `Vec<u8>`.
    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        self.0.into_vec()
    }
}

impl Deref for Name {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl AsRef<[u8]> for Name {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl Borrow<[u8]> for Name {
    #[inline]
    fn borrow(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("/")?;
        let bytes = self.as_bytes();
        let shown = bytes.len().min(64);
        for &b in &bytes[..shown] {
            if (0x21..=0x7E).contains(&b) && b != b'#' && b != b'/' {
                f.write_char(char::from(b))?;
            } else {
                write!(f, "#{b:02X}")?;
            }
        }
        if bytes.len() > shown {
            write!(f, "...({} bytes)", bytes.len())?;
        }
        Ok(())
    }
}

impl PartialEq<[u8]> for Name {
    #[inline]
    fn eq(&self, other: &[u8]) -> bool {
        self.as_bytes() == other
    }
}

impl PartialEq<&[u8]> for Name {
    #[inline]
    fn eq(&self, other: &&[u8]) -> bool {
        self.as_bytes() == *other
    }
}

impl<const N: usize> PartialEq<[u8; N]> for Name {
    #[inline]
    fn eq(&self, other: &[u8; N]) -> bool {
        self.as_bytes() == other
    }
}

impl<const N: usize> PartialEq<&[u8; N]> for Name {
    #[inline]
    fn eq(&self, other: &&[u8; N]) -> bool {
        self.as_bytes() == *other
    }
}

impl PartialEq<Vec<u8>> for Name {
    #[inline]
    fn eq(&self, other: &Vec<u8>) -> bool {
        self.as_bytes() == other.as_slice()
    }
}

impl PartialEq<SmallBytes> for Name {
    #[inline]
    fn eq(&self, other: &SmallBytes) -> bool {
        self.as_bytes() == other.as_slice()
    }
}

impl PartialEq<str> for Name {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<&str> for Name {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<Name> for [u8] {
    #[inline]
    fn eq(&self, other: &Name) -> bool {
        self == other.as_bytes()
    }
}

impl PartialEq<Name> for &[u8] {
    #[inline]
    fn eq(&self, other: &Name) -> bool {
        *self == other.as_bytes()
    }
}

impl From<SmallBytes> for Name {
    fn from(bytes: SmallBytes) -> Name {
        Name(bytes)
    }
}

impl From<&[u8]> for Name {
    fn from(bytes: &[u8]) -> Name {
        Name(SmallBytes::from_slice(bytes))
    }
}

impl<const N: usize> From<&[u8; N]> for Name {
    fn from(bytes: &[u8; N]) -> Name {
        Name(SmallBytes::from_slice(bytes))
    }
}

impl<const N: usize> From<[u8; N]> for Name {
    fn from(bytes: [u8; N]) -> Name {
        Name(SmallBytes::from_slice(&bytes))
    }
}

impl From<Vec<u8>> for Name {
    fn from(bytes: Vec<u8>) -> Name {
        Name(SmallBytes::from_vec(bytes))
    }
}

impl From<&Vec<u8>> for Name {
    fn from(bytes: &Vec<u8>) -> Name {
        Name(SmallBytes::from_slice(bytes))
    }
}

impl From<&str> for Name {
    fn from(s: &str) -> Name {
        Name(SmallBytes::from_slice(s.as_bytes()))
    }
}

impl From<String> for Name {
    fn from(s: String) -> Name {
        Name(SmallBytes::from_vec(s.into_bytes()))
    }
}

impl From<&String> for Name {
    fn from(s: &String) -> Name {
        Name(SmallBytes::from_slice(s.as_bytes()))
    }
}

impl From<&Name> for Name {
    fn from(name: &Name) -> Name {
        name.clone()
    }
}

impl From<Name> for SmallBytes {
    fn from(name: Name) -> SmallBytes {
        name.0
    }
}

impl From<Name> for Vec<u8> {
    fn from(name: Name) -> Vec<u8> {
        name.into_vec()
    }
}

fn write_capped_bytes(f: &mut fmt::Formatter<'_>, bytes: &[u8]) -> fmt::Result {
    f.write_str("b\"")?;
    let shown = bytes.len().min(64);
    for &b in &bytes[..shown] {
        for esc in core::ascii::escape_default(b) {
            f.write_char(char::from(esc))?;
        }
    }
    f.write_str("\"")?;
    if bytes.len() > shown {
        write!(f, "...({} bytes)", bytes.len())?;
    }
    Ok(())
}

/// Owned PDF object pairing a [`Value`] with an optional source [`Span`].
///
/// Two [`Object`]s are `==` when both their [`Value`]s and their [`Span`]s (including string
/// [`Form`]s, dictionary entry order, duplicate keys, and key spans) match. Use
/// [`Object::eq_value`] to compare semantic PDF values while ignoring spans, string forms, key
/// order, and shadowed dictionary duplicates.
#[derive(Debug)]
pub struct Object {
    value: Value,
    at: Option<Span>,
}

/// PDF object variants (ISO 32000-2 §7.3).
#[derive(Debug)]
pub enum Value {
    /// The `null` object.
    Null,
    /// Boolean (`true` or `false`).
    Bool(bool),
    /// 64-bit signed integer.
    Integer(i64),
    /// Floating-point real number (original token spelling is preserved via [`Object::at`]).
    Real(f64),
    /// Name object (`#xx` hex escapes decoded).
    Name(Name),
    /// Byte/text string paired with its original or preferred lexical [`Form`].
    String(Str, Option<Form>),
    /// Ordered array of objects.
    Array(Vec<Object>),
    /// Dictionary of key-value entries.
    Dict(Dict),
    /// Indirect stream dictionary and data location/payload.
    Stream(Box<Stream>),
    /// Indirect object reference (`n g R`).
    Ref(ObjectId),
}

/// Decoded or still-encrypted PDF string payload.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Str {
    /// Decoded (and, in encrypted files, decrypted) string bytes.
    Bytes(SmallBytes),
    /// Raw ciphertext bytes from an encrypted file that has not yet been decrypted.
    ///
    /// Redacted in `Debug` output and rejected by [`serialize`](crate::serialize) unless caller
    /// encryption/copying handles it first.
    Encrypted(SmallBytes),
}

impl fmt::Debug for Str {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Str::Bytes(bytes) => f.debug_tuple("Bytes").field(bytes).finish(),
            Str::Encrypted(bytes) => write!(f, "Encrypted({} bytes)", bytes.len()),
        }
    }
}

/// Lexical form of a PDF string token (ISO 32000-2 §7.3.4.2–7.3.4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Form {
    /// Parenthesized literal string `(...)`.
    Literal,
    /// Angle-bracketed hexadecimal string `<...>`.
    Hex,
}

/// PDF dictionary preserving entry order, key spans, and duplicate keys (ISO 32000-2 §7.3.7).
///
/// Lookups ([`Dict::get`], [`Dict::iter`]) return each key's last entry—matching PDF viewers—while
/// [`Dict::duplicates`] yields earlier shadowed entries for linting. Dictionaries up to 64 entries
/// perform duplicate-aware iteration with zero heap allocations.
///
/// ```
/// use pdf_objects::{Object, dict};
///
/// let mut d = dict! {
///     "Type" => Object::name("Page"),
///     "Rotate" => 90,
///     "Rotate" => 180,
/// };
/// assert_eq!(d.type_name(), Some(&b"Page"[..]));
/// assert_eq!(d.get("Rotate").and_then(Object::as_integer), Some(180));
/// assert_eq!(d.duplicates().len(), 1);
///
/// // `set` updates the key's effective position and removes earlier duplicates.
/// d.set("Rotate", 270);
/// assert_eq!(d.duplicates().len(), 0);
/// assert_eq!(d.get("Rotate").and_then(Object::as_integer), Some(270));
/// ```
#[derive(Debug, Default)]
pub struct Dict {
    pub(crate) entries: Vec<Entry>,
}

/// Single key-value entry in a [`Dict`].
#[derive(Debug)]
pub struct Entry {
    pub(crate) key: Name,
    pub(crate) key_at: Option<Span>,
    pub(crate) value: Object,
}

impl Entry {
    /// Creates an unplaced [`Entry`] for `key` and `value`.
    #[must_use]
    pub fn new(key: impl Into<Name>, value: impl Into<Object>) -> Entry {
        Entry {
            key: key.into(),
            key_at: None,
            value: value.into(),
        }
    }

    /// Attaches source span `at` to this entry's key.
    #[must_use]
    pub const fn with_key_span(mut self, at: Span) -> Entry {
        self.key_at = Some(at);
        self
    }

    /// Decoded key bytes.
    #[must_use]
    #[inline]
    pub fn key(&self) -> &[u8] {
        self.key.as_bytes()
    }

    /// Key as a [`Name`].
    #[must_use]
    #[inline]
    pub const fn key_name(&self) -> &Name {
        &self.key
    }

    /// Source span of the key token, if known and unedited.
    #[must_use]
    #[inline]
    pub const fn key_at(&self) -> Option<Span> {
        self.key_at
    }

    /// Entry value.
    #[must_use]
    #[inline]
    pub const fn value(&self) -> &Object {
        &self.value
    }

    /// Mutable reference to the entry's value.
    #[must_use]
    #[inline]
    pub fn value_mut(&mut self) -> &mut Object {
        &mut self.value
    }

    /// Consumes the entry and returns its [`Object`] value.
    #[must_use]
    #[inline]
    pub fn into_value(self) -> Object {
        self.value
    }

    /// Decomposes the entry into `(key, key_at, value)`.
    #[must_use]
    #[inline]
    pub fn into_parts(self) -> (Name, Option<Span>, Object) {
        (self.key, self.key_at, self.value)
    }

    /// Replaces the entry's key and clears its [`Entry::key_at`] span.
    pub fn set_key(&mut self, key: impl Into<Name>) {
        self.key = key.into();
        self.key_at = None;
    }

    /// Clears the key's [`Span`].
    pub fn clear_key_span(&mut self) {
        self.key_at = None;
    }
}

/// PDF stream dictionary paired with its raw (filter-encoded) payload [`Data`].
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Stream {
    /// Stream dictionary (`/Length`, `/Filter`, `/DecodeParms`, etc.).
    pub dict: Dict,
    /// Stream payload location or in-memory bytes.
    pub data: Data,
}

/// Backing storage for a [`Stream`]'s raw (filter-encoded) bytes.
#[derive(Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Data {
    /// Byte span in the source file (`Span::stream() == None`) as originally written.
    At(Span),
    /// Owned raw stream bytes in memory (redacted to byte length in `Debug`).
    Bytes(Vec<u8>),
    /// Unplaced stream with no attached byte slice.
    #[default]
    Unplaced,
}

impl fmt::Debug for Data {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Data::At(span) => f.debug_tuple("At").field(span).finish(),
            Data::Bytes(bytes) => write!(f, "Bytes({} bytes)", bytes.len()),
            Data::Unplaced => f.write_str("Unplaced"),
        }
    }
}

#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(size_of::<ObjectId>() == 8);
    assert!(size_of::<SmallBytes>() == 16);
    assert!(size_of::<Name>() == 16);
    assert!(size_of::<Str>() == 24);
    assert!(size_of::<Dict>() == 24);
    assert!(size_of::<Span>() == 32);
    assert!(size_of::<Option<Span>>() == 32);
    assert!(size_of::<Value>() == 32);
    assert!(size_of::<Object>() == 64);
    assert!(size_of::<Entry>() == 112);
};

impl Object {
    /// Wraps `value` with no source [`Span`].
    #[must_use]
    pub const fn new(value: Value) -> Object {
        Object { value, at: None }
    }

    /// Creates an unplaced `null` object.
    #[must_use]
    pub const fn null() -> Object {
        Object::new(Value::Null)
    }

    /// Creates an indirect reference `n 0 R`.
    #[must_use]
    pub const fn reference(n: u32) -> Object {
        Object::new(Value::Ref(ObjectId::new(n, 0)))
    }

    /// Creates a [`Value::Name`] object from decoded name bytes (without leading `/`).
    #[must_use]
    pub fn name(name: impl Into<Name>) -> Object {
        Object::new(Value::Name(name.into()))
    }

    /// Encodes `text` as an ISO 32000-2 §7.9.2.2 PDF text string.
    ///
    /// Uses [`Form::Literal`] when `text` consists of printable ASCII / `\t` / `\n` / `\r`
    /// (`PDFDocEncoding` verbatim subset) and [`Form::Hex`] (`UTF-16BE` with BOM) otherwise.
    /// Note that `U+001B` is reserved as the §7.9.2.2.2 language-escape delimiter in PDF text
    /// strings and will be stripped as an escape on decode.
    #[must_use]
    pub fn text(text: &str) -> Object {
        let form = if text::is_verbatim_pdf_doc(text) {
            Form::Literal
        } else {
            Form::Hex
        };
        Object::new(Value::String(
            Str::Bytes(SmallBytes::from_vec(text::encode(text))),
            Some(form),
        ))
    }

    /// Creates a byte string with no preferred lexical [`Form`].
    #[must_use]
    pub fn bytes(bytes: impl Into<SmallBytes>) -> Object {
        Object::new(Value::String(Str::Bytes(bytes.into()), None))
    }

    /// Creates a byte string marked for hexadecimal `<...>` serialization.
    #[must_use]
    pub fn hex(bytes: impl Into<SmallBytes>) -> Object {
        Object::new(Value::String(Str::Bytes(bytes.into()), Some(Form::Hex)))
    }

    /// Creates an unplaced stream object with `dict` and [`Data::Unplaced`].
    #[must_use]
    pub fn stream(dict: Dict) -> Object {
        Object::new(Value::Stream(Box::new(Stream {
            dict,
            data: Data::Unplaced,
        })))
    }

    /// Returns the source [`Span`] of this object, if unedited since parsing.
    #[must_use]
    #[inline]
    pub const fn at(&self) -> Option<Span> {
        self.at
    }

    /// Attaches source `span` to `self`.
    #[must_use]
    #[inline]
    pub const fn with_span(mut self, span: Span) -> Object {
        self.at = Some(span);
        self
    }

    /// Clears this object's top-level [`Span`] without walking nested children.
    #[inline]
    pub fn clear_span(&mut self) {
        self.at = None;
    }

    /// Borrows the underlying [`Value`].
    #[must_use]
    #[inline]
    pub const fn value(&self) -> &Value {
        &self.value
    }

    /// Extracts the inner [`Value`], discarding any top-level [`Span`].
    #[must_use]
    #[inline]
    pub fn into_value(mut self) -> Value {
        core::mem::replace(&mut self.value, Value::Null)
    }

    /// Mutably borrows the inner [`Value`] and clears `self.at()`.
    #[inline]
    pub fn value_mut(&mut self) -> &mut Value {
        self.at = None;
        &mut self.value
    }

    /// Mutably borrows the inner [`Value`] **without** clearing `self.at()`.
    ///
    /// Intended only for representations of the same source bytes (such as decrypting a parsed
    /// [`Str::Encrypted`] into [`Str::Bytes`]); copy-on-write writers treat intact spans as
    /// unchanged source bytes.
    #[inline]
    pub fn as_written_mut(&mut self) -> &mut Value {
        &mut self.value
    }

    /// Returns the boolean value if [`Value::Bool`].
    #[must_use]
    #[inline]
    pub const fn as_bool(&self) -> Option<bool> {
        match self.value {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }

    /// Returns the decoded name bytes if [`Value::Name`].
    #[must_use]
    #[inline]
    pub fn as_name(&self) -> Option<&[u8]> {
        match &self.value {
            Value::Name(name) => Some(name.as_bytes()),
            _ => None,
        }
    }

    /// Returns the [`Name`] if [`Value::Name`].
    #[must_use]
    #[inline]
    pub const fn as_name_value(&self) -> Option<&Name> {
        match &self.value {
            Value::Name(name) => Some(name),
            _ => None,
        }
    }

    /// Returns the integer value if [`Value::Integer`].
    #[must_use]
    #[inline]
    pub const fn as_integer(&self) -> Option<i64> {
        match self.value {
            Value::Integer(i) => Some(i),
            _ => None,
        }
    }

    /// Returns the numeric value as `f64` if [`Value::Integer`] or [`Value::Real`].
    #[must_use]
    #[inline]
    pub const fn as_number(&self) -> Option<f64> {
        match self.value {
            Value::Integer(i) => Some(i as f64),
            Value::Real(r) => Some(r),
            _ => None,
        }
    }

    /// Returns the [`Str`] if [`Value::String`].
    #[must_use]
    #[inline]
    pub const fn as_str(&self) -> Option<&Str> {
        match &self.value {
            Value::String(s, _) => Some(s),
            _ => None,
        }
    }

    /// Returns decoded string bytes if [`Value::String`] holds [`Str::Bytes`].
    #[must_use]
    #[inline]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match &self.value {
            Value::String(s, _) => s.as_bytes(),
            _ => None,
        }
    }

    /// Returns the element slice if [`Value::Array`].
    #[must_use]
    #[inline]
    pub fn as_array(&self) -> Option<&[Object]> {
        match &self.value {
            Value::Array(array) => Some(array),
            _ => None,
        }
    }

    /// Returns the dictionary if [`Value::Dict`] or [`Value::Stream`].
    #[must_use]
    #[inline]
    pub fn as_dict(&self) -> Option<&Dict> {
        match &self.value {
            Value::Dict(dict) => Some(dict),
            Value::Stream(stream) => Some(&stream.dict),
            _ => None,
        }
    }

    /// Returns the [`Stream`] if [`Value::Stream`].
    #[must_use]
    #[inline]
    pub fn as_stream(&self) -> Option<&Stream> {
        match &self.value {
            Value::Stream(stream) => Some(stream),
            _ => None,
        }
    }

    /// Mutably borrows the array and clears `self.at()` if [`Value::Array`].
    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Object>> {
        match &mut self.value {
            Value::Array(array) => {
                self.at = None;
                Some(array)
            }
            _ => None,
        }
    }

    /// Mutably borrows the dictionary (or stream dictionary) and clears `self.at()`.
    pub fn as_dict_mut(&mut self) -> Option<&mut Dict> {
        let dict = match &mut self.value {
            Value::Dict(dict) => dict,
            Value::Stream(stream) => &mut stream.dict,
            _ => return None,
        };
        self.at = None;
        Some(dict)
    }

    /// Mutably borrows the [`Stream`] and clears `self.at()` if [`Value::Stream`].
    pub fn as_stream_mut(&mut self) -> Option<&mut Stream> {
        match &mut self.value {
            Value::Stream(stream) => {
                self.at = None;
                Some(stream)
            }
            _ => None,
        }
    }

    /// Returns the referenced [`ObjectId`] if [`Value::Ref`].
    #[must_use]
    #[inline]
    pub const fn as_reference(&self) -> Option<ObjectId> {
        match self.value {
            Value::Ref(id) => Some(id),
            _ => None,
        }
    }

    /// Returns `true` if [`Value::Null`].
    #[must_use]
    #[inline]
    pub const fn is_null(&self) -> bool {
        matches!(self.value, Value::Null)
    }

    /// Compares semantic PDF values, ignoring source [`Span`]s, string [`Form`]s, dictionary key
    /// order, and shadowed duplicate dictionary entries.
    #[must_use]
    pub fn eq_value(&self, other: &Object) -> bool {
        if let Some(eq) = eq_scalar_values(&self.value, &other.value) {
            return eq;
        }
        let mut stack = alloc::vec![(&self.value, &other.value)];
        while let Some((va, vb)) = stack.pop() {
            if let Some(eq) = eq_scalar_values(va, vb) {
                if !eq {
                    return false;
                }
                continue;
            }
            match (va, vb) {
                (Value::Array(ax), Value::Array(ay)) => {
                    if ax.len() != ay.len() {
                        return false;
                    }
                    for (x, y) in ax.iter().zip(ay.iter()) {
                        if let Some(eq) = eq_scalar_values(&x.value, &y.value) {
                            if !eq {
                                return false;
                            }
                        } else {
                            stack.push((&x.value, &y.value));
                        }
                    }
                }
                (Value::Dict(dx), Value::Dict(dy)) => {
                    if !push_dict_eq_value(dx, dy, &mut stack) {
                        return false;
                    }
                }
                (Value::Stream(sx), Value::Stream(sy)) => {
                    if sx.data != sy.data || !push_dict_eq_value(&sx.dict, &sy.dict, &mut stack) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        true
    }

    /// Iterates over all indirect [`ObjectId`]s referenced inside `self` in written order
    /// (including shadowed dictionary entries and stream dictionaries, never stream data).
    pub fn references(&self) -> References<'_> {
        let stack = match &self.value {
            Value::Ref(_) | Value::Array(_) | Value::Dict(_) | Value::Stream(_) => {
                alloc::vec![self]
            }
            _ => Vec::new(),
        };
        References { stack }
    }

    /// Rewrites every indirect reference in `self` using `to`, replacing unmapped references
    /// (`None`) with `Value::Null` (ISO 32000-2 §7.3.10).
    ///
    /// Clears the [`Span`] on each modified reference and its ancestor containers while leaving
    /// untouched subtrees' spans intact. Returns `true` if at least one reference changed.
    pub fn renumber(&mut self, mut to: impl FnMut(ObjectId) -> Option<ObjectId>) -> bool {
        match &mut self.value {
            Value::Ref(id) => {
                let named = to(*id);
                if named != Some(*id) {
                    self.value = named.map_or(Value::Null, Value::Ref);
                    self.at = None;
                    return true;
                }
                return false;
            }
            Value::Array(a) if !a.is_empty() => {}
            Value::Dict(d) if !d.entries.is_empty() => {}
            Value::Stream(s) if !s.dict.entries.is_empty() => {}
            _ => return false,
        }

        let mut holders: Vec<&mut Option<Span>> = Vec::new();
        let mut dropped = 0;
        let mut changed = false;
        let mut stack = alloc::vec![(self, 0)];
        while let Some((Object { value, at }, depth)) = stack.pop() {
            holders.truncate(depth);
            dropped = dropped.min(depth);
            let entries = match value {
                Value::Ref(id) => {
                    let named = to(*id);
                    if named != Some(*id) {
                        *value = named.map_or(Value::Null, Value::Ref);
                        *at = None;
                        for held in &mut holders[dropped..] {
                            **held = None;
                        }
                        dropped = depth;
                        changed = true;
                    }
                    continue;
                }
                Value::Array(array) => {
                    holders.push(at);
                    stack.extend(array.iter_mut().rev().map(|o| (o, depth + 1)));
                    continue;
                }
                Value::Dict(dict) => &mut dict.entries,
                Value::Stream(stream) => &mut stream.dict.entries,
                _ => continue,
            };
            holders.push(at);
            let values = entries.iter_mut().rev().map(|entry| &mut entry.value);
            stack.extend(values.map(|o| (o, depth + 1)));
        }
        changed
    }

    /// Strips every [`Span`] in `self` (values, dictionary keys, and [`Data::At`] stream spans,
    /// which become [`Data::Unplaced`]). [`Data::Bytes`] payloads are preserved.
    pub fn unplace(&mut self) {
        self.at = None;
        match &mut self.value {
            Value::Array(a) if !a.is_empty() => {}
            Value::Dict(d) if !d.entries.is_empty() => {}
            Value::Stream(s) => {
                if matches!(s.data, Data::At(_)) {
                    s.data = Data::Unplaced;
                }
                if s.dict.entries.is_empty() {
                    return;
                }
            }
            _ => return,
        }

        let mut stack = alloc::vec![self];
        while let Some(Object { value, at }) = stack.pop() {
            *at = None;
            let entries = match value {
                Value::Array(array) => {
                    stack.extend(array.iter_mut());
                    continue;
                }
                Value::Dict(dict) => &mut dict.entries,
                Value::Stream(stream) => {
                    if matches!(stream.data, Data::At(_)) {
                        stream.data = Data::Unplaced;
                    }
                    &mut stream.dict.entries
                }
                _ => continue,
            };
            for entry in entries {
                entry.key_at = None;
                stack.push(&mut entry.value);
            }
        }
    }

    #[inline]
    fn is_container(&self) -> bool {
        matches!(
            self.value,
            Value::Array(_) | Value::Dict(_) | Value::Stream(_)
        )
    }

    #[inline]
    fn clone_scalar(&self) -> Object {
        let value = match &self.value {
            Value::Null => Value::Null,
            Value::Bool(b) => Value::Bool(*b),
            Value::Integer(i) => Value::Integer(*i),
            Value::Real(r) => Value::Real(*r),
            Value::Name(n) => Value::Name(n.clone()),
            Value::String(s, f) => Value::String(s.clone(), *f),
            Value::Ref(id) => Value::Ref(*id),
            Value::Array(_) | Value::Dict(_) | Value::Stream(_) => {
                unreachable!("clone_scalar called on container")
            }
        };
        Object { value, at: self.at }
    }

    fn drop_containers(&mut self) {
        let mut current = core::mem::replace(&mut self.value, Value::Null);
        let mut stack: Vec<Value> = Vec::new();
        loop {
            let mut next_container: Option<Value> = None;
            let mut push_child = |obj: &mut Object| {
                if obj.is_container() {
                    let child = core::mem::replace(&mut obj.value, Value::Null);
                    if let Some(prev) = next_container.replace(child) {
                        stack.push(prev);
                    }
                }
            };
            match current {
                Value::Array(mut elems) => {
                    for elem in &mut elems {
                        push_child(elem);
                    }
                }
                Value::Dict(mut dict) => {
                    for entry in &mut dict.entries {
                        push_child(&mut entry.value);
                    }
                }
                Value::Stream(mut stream) => {
                    for entry in &mut stream.dict.entries {
                        push_child(&mut entry.value);
                    }
                }
                _ => {}
            }
            current = match next_container.or_else(|| stack.pop()) {
                Some(v) => v,
                None => break,
            };
        }
    }
}

impl Drop for Object {
    #[inline]
    fn drop(&mut self) {
        if self.is_container() {
            self.drop_containers();
        }
    }
}

enum CloneFrame<'a> {
    Array {
        at: Option<Span>,
        out: Vec<Object>,
        rest: slice::Iter<'a, Object>,
    },
    Dict {
        at: Option<Span>,
        stream_data: Option<Data>,
        out: Vec<Entry>,
        next_key: Name,
        next_key_at: Option<Span>,
        rest: slice::Iter<'a, Entry>,
    },
}

fn clone_object_deep(root: &Object) -> Object {
    let mut frames: Vec<CloneFrame<'_>> = Vec::new();
    let mut current = root;

    'descend: loop {
        let mut finished = match &current.value {
            Value::Array(elems) => {
                let mut out = Vec::with_capacity(elems.len());
                let mut rest = elems.iter();
                while let Some(item) = rest.next() {
                    if !item.is_container() {
                        out.push(item.clone_scalar());
                    } else {
                        frames.push(CloneFrame::Array {
                            at: current.at,
                            out,
                            rest,
                        });
                        current = item;
                        continue 'descend;
                    }
                }
                Object {
                    value: Value::Array(out),
                    at: current.at,
                }
            }
            Value::Dict(dict) => {
                let mut out = Vec::with_capacity(dict.entries.len());
                let mut rest = dict.entries.iter();
                while let Some(entry) = rest.next() {
                    if !entry.value.is_container() {
                        out.push(Entry {
                            key: entry.key.clone(),
                            key_at: entry.key_at,
                            value: entry.value.clone_scalar(),
                        });
                    } else {
                        frames.push(CloneFrame::Dict {
                            at: current.at,
                            stream_data: None,
                            out,
                            next_key: entry.key.clone(),
                            next_key_at: entry.key_at,
                            rest,
                        });
                        current = &entry.value;
                        continue 'descend;
                    }
                }
                Object {
                    value: Value::Dict(Dict { entries: out }),
                    at: current.at,
                }
            }
            Value::Stream(stream) => {
                let mut out = Vec::with_capacity(stream.dict.entries.len());
                let mut rest = stream.dict.entries.iter();
                let stream_data = stream.data.clone();
                while let Some(entry) = rest.next() {
                    if !entry.value.is_container() {
                        out.push(Entry {
                            key: entry.key.clone(),
                            key_at: entry.key_at,
                            value: entry.value.clone_scalar(),
                        });
                    } else {
                        frames.push(CloneFrame::Dict {
                            at: current.at,
                            stream_data: Some(stream_data),
                            out,
                            next_key: entry.key.clone(),
                            next_key_at: entry.key_at,
                            rest,
                        });
                        current = &entry.value;
                        continue 'descend;
                    }
                }
                Object {
                    value: Value::Stream(Box::new(Stream {
                        dict: Dict { entries: out },
                        data: stream_data,
                    })),
                    at: current.at,
                }
            }
            _ => current.clone_scalar(),
        };

        while let Some(frame) = frames.pop() {
            match frame {
                CloneFrame::Array {
                    at,
                    mut out,
                    mut rest,
                } => {
                    out.push(finished);
                    while let Some(item) = rest.next() {
                        if !item.is_container() {
                            out.push(item.clone_scalar());
                        } else {
                            frames.push(CloneFrame::Array { at, out, rest });
                            current = item;
                            continue 'descend;
                        }
                    }
                    finished = Object {
                        value: Value::Array(out),
                        at,
                    };
                }
                CloneFrame::Dict {
                    at,
                    stream_data,
                    mut out,
                    next_key,
                    next_key_at,
                    mut rest,
                } => {
                    out.push(Entry {
                        key: next_key,
                        key_at: next_key_at,
                        value: finished,
                    });
                    while let Some(entry) = rest.next() {
                        if !entry.value.is_container() {
                            out.push(Entry {
                                key: entry.key.clone(),
                                key_at: entry.key_at,
                                value: entry.value.clone_scalar(),
                            });
                        } else {
                            frames.push(CloneFrame::Dict {
                                at,
                                stream_data,
                                out,
                                next_key: entry.key.clone(),
                                next_key_at: entry.key_at,
                                rest,
                            });
                            current = &entry.value;
                            continue 'descend;
                        }
                    }
                    let dict = Dict { entries: out };
                    let value = match stream_data {
                        Some(data) => Value::Stream(Box::new(Stream { dict, data })),
                        None => Value::Dict(dict),
                    };
                    finished = Object { value, at };
                }
            }
        }
        return finished;
    }
}

impl Clone for Object {
    #[inline]
    fn clone(&self) -> Object {
        if !self.is_container() {
            self.clone_scalar()
        } else {
            clone_object_deep(self)
        }
    }
}

impl Clone for Entry {
    #[inline]
    fn clone(&self) -> Entry {
        Entry {
            key: self.key.clone(),
            key_at: self.key_at,
            value: self.value.clone(),
        }
    }
}

impl Clone for Dict {
    fn clone(&self) -> Dict {
        let mut entries = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            entries.push(entry.clone());
        }
        Dict { entries }
    }
}

impl Clone for Stream {
    fn clone(&self) -> Stream {
        Stream {
            dict: self.dict.clone(),
            data: self.data.clone(),
        }
    }
}

impl Clone for Value {
    #[inline]
    fn clone(&self) -> Value {
        match self {
            Value::Null => Value::Null,
            Value::Bool(b) => Value::Bool(*b),
            Value::Integer(i) => Value::Integer(*i),
            Value::Real(r) => Value::Real(*r),
            Value::Name(n) => Value::Name(n.clone()),
            Value::String(s, f) => Value::String(s.clone(), *f),
            Value::Ref(id) => Value::Ref(*id),
            Value::Array(elems) => {
                let mut out = Vec::with_capacity(elems.len());
                for elem in elems {
                    out.push(elem.clone());
                }
                Value::Array(out)
            }
            Value::Dict(dict) => Value::Dict(dict.clone()),
            Value::Stream(stream) => Value::Stream(Box::new((**stream).clone())),
        }
    }
}

#[inline]
fn eq_scalar_exact(a: &Value, b: &Value) -> Option<bool> {
    match (a, b) {
        (Value::Null, Value::Null) => Some(true),
        (Value::Bool(x), Value::Bool(y)) => Some(x == y),
        (Value::Integer(x), Value::Integer(y)) => Some(x == y),
        (Value::Real(x), Value::Real(y)) => Some(x == y),
        (Value::Name(x), Value::Name(y)) => Some(x == y),
        (Value::String(sx, fx), Value::String(sy, fy)) => Some(fx == fy && sx == sy),
        (Value::Ref(x), Value::Ref(y)) => Some(x == y),
        (Value::Array(_), Value::Array(_))
        | (Value::Dict(_), Value::Dict(_))
        | (Value::Stream(_), Value::Stream(_)) => None,
        _ => Some(false),
    }
}

#[inline]
fn eq_scalar_values(a: &Value, b: &Value) -> Option<bool> {
    match (a, b) {
        (Value::Null, Value::Null) => Some(true),
        (Value::Bool(x), Value::Bool(y)) => Some(x == y),
        (Value::Integer(x), Value::Integer(y)) => Some(x == y),
        (Value::Real(x), Value::Real(y)) => Some(x == y),
        (Value::Name(x), Value::Name(y)) => Some(x == y),
        (Value::String(sx, _), Value::String(sy, _)) => Some(sx == sy),
        (Value::Ref(x), Value::Ref(y)) => Some(x == y),
        (Value::Array(_), Value::Array(_))
        | (Value::Dict(_), Value::Dict(_))
        | (Value::Stream(_), Value::Stream(_)) => None,
        _ => Some(false),
    }
}

fn push_dict_eq_value<'a>(
    dx: &'a Dict,
    dy: &'a Dict,
    stack: &mut Vec<(&'a Value, &'a Value)>,
) -> bool {
    let x_iter = dx.iter();
    let y_iter = dy.iter();
    if x_iter.len() != y_iter.len() {
        return false;
    }
    if x_iter.len() <= 32 {
        for (k, vx) in x_iter {
            let Some(vy) = dy.get(k) else {
                return false;
            };
            if let Some(eq) = eq_scalar_values(&vx.value, &vy.value) {
                if !eq {
                    return false;
                }
            } else {
                stack.push((&vx.value, &vy.value));
            }
        }
        true
    } else {
        let mut ys: Vec<(&[u8], &Object)> = y_iter.collect();
        ys.sort_unstable_by_key(|&(k, _)| k);
        for (k, vx) in x_iter {
            let Ok(idx) = ys.binary_search_by_key(&k, |&(ky, _)| ky) else {
                return false;
            };
            let vy = ys[idx].1;
            if let Some(eq) = eq_scalar_values(&vx.value, &vy.value) {
                if !eq {
                    return false;
                }
            } else {
                stack.push((&vx.value, &vy.value));
            }
        }
        true
    }
}

enum EqTask<'a> {
    Objects(&'a [Object], &'a [Object]),
    Entries(&'a [Entry], &'a [Entry]),
}

fn eq_task_deep(first: EqTask<'_>) -> bool {
    let mut current = first;
    let mut stack: Vec<EqTask<'_>> = Vec::new();

    'outer: loop {
        match current {
            EqTask::Objects(mut xs, mut ys) => {
                while let (Some((x, xs_rest)), Some((y, ys_rest))) =
                    (xs.split_first(), ys.split_first())
                {
                    if x.at != y.at {
                        return false;
                    }
                    if let Some(eq) = eq_scalar_exact(&x.value, &y.value) {
                        if !eq {
                            return false;
                        }
                        xs = xs_rest;
                        ys = ys_rest;
                    } else {
                        let Some(child) = container_eq_task(&x.value, &y.value) else {
                            return false;
                        };
                        if !xs_rest.is_empty() {
                            stack.push(EqTask::Objects(xs_rest, ys_rest));
                        }
                        current = child;
                        continue 'outer;
                    }
                }
            }
            EqTask::Entries(mut xs, mut ys) => {
                while let (Some((x, xs_rest)), Some((y, ys_rest))) =
                    (xs.split_first(), ys.split_first())
                {
                    if x.key != y.key || x.key_at != y.key_at || x.value.at != y.value.at {
                        return false;
                    }
                    if let Some(eq) = eq_scalar_exact(&x.value.value, &y.value.value) {
                        if !eq {
                            return false;
                        }
                        xs = xs_rest;
                        ys = ys_rest;
                    } else {
                        let Some(child) = container_eq_task(&x.value.value, &y.value.value) else {
                            return false;
                        };
                        if !xs_rest.is_empty() {
                            stack.push(EqTask::Entries(xs_rest, ys_rest));
                        }
                        current = child;
                        continue 'outer;
                    }
                }
            }
        }
        current = match stack.pop() {
            Some(next) => next,
            None => return true,
        };
    }
}

#[inline]
fn container_eq_task<'a>(a: &'a Value, b: &'a Value) -> Option<EqTask<'a>> {
    match (a, b) {
        (Value::Array(ax), Value::Array(ay)) if ax.len() == ay.len() => {
            Some(EqTask::Objects(ax, ay))
        }
        (Value::Dict(dx), Value::Dict(dy)) if dx.entries.len() == dy.entries.len() => {
            Some(EqTask::Entries(&dx.entries, &dy.entries))
        }
        (Value::Stream(sx), Value::Stream(sy))
            if sx.data == sy.data && sx.dict.entries.len() == sy.dict.entries.len() =>
        {
            Some(EqTask::Entries(&sx.dict.entries, &sy.dict.entries))
        }
        _ => None,
    }
}

impl PartialEq for Object {
    #[inline]
    fn eq(&self, other: &Object) -> bool {
        if self.at != other.at {
            return false;
        }
        self.value == other.value
    }
}

impl PartialEq for Value {
    #[inline]
    fn eq(&self, other: &Value) -> bool {
        if let Some(eq) = eq_scalar_exact(self, other) {
            return eq;
        }
        match container_eq_task(self, other) {
            Some(task) => eq_task_deep(task),
            None => false,
        }
    }
}

impl PartialEq for Entry {
    #[inline]
    fn eq(&self, other: &Entry) -> bool {
        self.key == other.key && self.key_at == other.key_at && self.value == other.value
    }
}

impl PartialEq for Dict {
    #[inline]
    fn eq(&self, other: &Dict) -> bool {
        self.entries.len() == other.entries.len()
            && eq_task_deep(EqTask::Entries(&self.entries, &other.entries))
    }
}

impl PartialEq for Stream {
    #[inline]
    fn eq(&self, other: &Stream) -> bool {
        self.data == other.data && self.dict == other.dict
    }
}

impl Str {
    /// Creates a [`Str::Bytes`] payload.
    #[must_use]
    pub fn bytes(bytes: impl Into<SmallBytes>) -> Str {
        Str::Bytes(bytes.into())
    }

    /// Creates a [`Str::Encrypted`] ciphertext payload.
    #[must_use]
    pub fn encrypted(bytes: impl Into<SmallBytes>) -> Str {
        Str::Encrypted(bytes.into())
    }

    /// Decodes the string under ISO 32000-2 §7.9.2.2 rules, returning `None` for
    /// [`Str::Encrypted`].
    #[must_use]
    pub fn decode(&self) -> Option<text::Text> {
        match self {
            Str::Bytes(bytes) => Some(text::decode(bytes)),
            Str::Encrypted(_) => None,
        }
    }

    /// Decodes the string under PDFium (`chromium/8152`) rules, returning `None` for
    /// [`Str::Encrypted`].
    #[must_use]
    pub fn decode_pdfium(&self) -> Option<text::pdfium::Text> {
        match self {
            Str::Bytes(bytes) => Some(text::pdfium::decode(bytes)),
            Str::Encrypted(_) => None,
        }
    }

    /// Decodes the string into a `String` using either ISO 32000-2 or PDFium rules according to
    /// `rules`, returning `None` for [`Str::Encrypted`].
    #[must_use]
    pub fn decode_str(&self, rules: pdf_tokenizer::Rules) -> Option<String> {
        match self {
            Str::Bytes(bytes) => Some(text::decode_str(bytes, rules)),
            Str::Encrypted(_) => None,
        }
    }

    /// Decodes the string as ISO 32000-2 text, returning `None` for [`Str::Encrypted`].
    #[must_use]
    pub fn text(&self) -> Option<String> {
        self.decode().map(|t| t.text)
    }

    /// Returns the raw decoded string bytes, or `None` for [`Str::Encrypted`].
    #[must_use]
    #[inline]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Str::Bytes(bytes) => Some(bytes.as_slice()),
            Str::Encrypted(_) => None,
        }
    }

    /// Returns `true` if the string is still [`Str::Encrypted`].
    #[must_use]
    #[inline]
    pub const fn is_encrypted(&self) -> bool {
        matches!(self, Str::Encrypted(_))
    }
}

impl Data {
    /// Returns the source [`Span`] if [`Data::At`].
    #[must_use]
    #[inline]
    pub const fn at(&self) -> Option<Span> {
        match self {
            Data::At(at) => Some(*at),
            _ => None,
        }
    }

    /// Returns the in-memory byte slice if [`Data::Bytes`].
    #[must_use]
    #[inline]
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Data::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }
}

impl Stream {
    /// Creates a [`Stream`] with `dict` and `bytes`, setting `/Length` to `bytes.len()`.
    #[must_use]
    pub fn new(dict: Dict, bytes: Vec<u8>) -> Stream {
        let mut stream = Stream {
            dict,
            data: Data::Unplaced,
        };
        stream.set_bytes(bytes);
        stream
    }

    /// Replaces the stream payload with `bytes` and sets `/Length` to `bytes.len()` (removing any
    /// shadowed `/Length` duplicates).
    pub fn set_bytes(&mut self, bytes: Vec<u8>) {
        self.dict.set("Length", bytes.len() as i64);
        self.data = Data::Bytes(bytes);
    }
}

impl Dict {
    /// Empty dictionary constant.
    pub const EMPTY: Dict = Dict {
        entries: Vec::new(),
    };

    /// Creates an empty [`Dict`].
    #[must_use]
    pub const fn new() -> Dict {
        Dict::EMPTY
    }

    /// Creates an empty [`Dict`] with pre-allocated capacity for `capacity` entries.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Dict {
        Dict {
            entries: Vec::with_capacity(capacity),
        }
    }

    /// Returns the effective (last) value for `key`.
    #[must_use]
    #[inline]
    pub fn get(&self, key: impl AsRef<[u8]>) -> Option<&Object> {
        self.entry(key).map(|entry| &entry.value)
    }

    /// Mutably borrows the effective (last) value for `key`.
    #[must_use]
    #[inline]
    pub fn get_mut(&mut self, key: impl AsRef<[u8]>) -> Option<&mut Object> {
        let key = key.as_ref();
        self.entries
            .iter_mut()
            .rev()
            .find(|entry| entry.key.as_bytes() == key)
            .map(|entry| &mut entry.value)
    }

    /// Returns the effective (last) [`Entry`] for `key`.
    #[must_use]
    #[inline]
    pub fn entry(&self, key: impl AsRef<[u8]>) -> Option<&Entry> {
        let key = key.as_ref();
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.key.as_bytes() == key)
    }

    /// Returns `true` if `key` is present.
    #[must_use]
    #[inline]
    pub fn has(&self, key: impl AsRef<[u8]>) -> bool {
        self.entry(key).is_some()
    }

    /// Returns `/Type` when it is a [`Value::Name`].
    #[must_use]
    #[inline]
    pub fn type_name(&self) -> Option<&[u8]> {
        self.get(b"Type").and_then(Object::as_name)
    }

    /// Returns `true` if `/Type` is the name `name`.
    #[must_use]
    #[inline]
    pub fn has_type(&self, name: impl AsRef<[u8]>) -> bool {
        self.type_name() == Some(name.as_ref())
    }

    /// Iterates over effective `(key, value)` pairs in entry order (skipping shadowed duplicates).
    pub fn iter(&self) -> Iter<'_> {
        let shadowed = Shadowed::of(&self.entries);
        let remaining = self.entries.len() - shadowed.len();
        Iter {
            entries: self.entries.iter().enumerate(),
            shadowed,
            remaining,
        }
    }

    /// Iterates mutably over effective `(key, value)` pairs in entry order.
    pub fn iter_mut(&mut self) -> IterMut<'_> {
        let shadowed = Shadowed::of(&self.entries);
        let remaining = self.entries.len() - shadowed.len();
        IterMut {
            entries: self.entries.iter_mut().enumerate(),
            shadowed,
            remaining,
        }
    }

    /// Iterates over each key's effective (last) [`Entry`] in entry order.
    pub fn last_entries(&self) -> LastEntries<'_> {
        let shadowed = Shadowed::of(&self.entries);
        let remaining = self.entries.len() - shadowed.len();
        LastEntries {
            entries: self.entries.iter().enumerate(),
            shadowed,
            remaining,
        }
    }

    /// Iterates over earlier [`Entry`] occurrences overridden by a later entry with the same key.
    pub fn duplicates(&self) -> Duplicates<'_> {
        let shadowed = Shadowed::of(&self.entries);
        let remaining = shadowed.len();
        Duplicates {
            entries: self.entries.iter().enumerate(),
            shadowed,
            remaining,
        }
    }

    /// Returns all entries as written, including shadowed duplicates.
    #[must_use]
    #[inline]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Mutably iterates over all raw entries as written (use [`Entry::set_key`] if renaming a key
    /// so its [`Entry::key_at`] span is cleared).
    pub fn entries_mut(&mut self) -> slice::IterMut<'_, Entry> {
        self.entries.iter_mut()
    }

    /// Number of distinct keys in the dictionary.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len() - Shadowed::of(&self.entries).len()
    }

    /// Returns `true` if the dictionary has no entries.
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Appends `(key, value)` at the end of the dictionary without checking for existing keys.
    pub fn push(&mut self, key: impl Into<Name>, value: impl Into<Object>) {
        self.push_entry(Entry::new(key, value));
    }

    /// Appends raw `entry` at the end of the dictionary.
    pub fn push_entry(&mut self, entry: Entry) {
        self.entries.push(entry);
    }

    /// Sets `key` to `value` in place at its effective (last) entry—or appends a new entry if
    /// absent—and removes any earlier shadowed duplicates of `key`.
    pub fn set(&mut self, key: impl Into<Name>, value: impl Into<Object>) {
        let key = key.into();
        let key_bytes = key.as_bytes();
        let Some(last) = self
            .entries
            .iter()
            .rposition(|entry| entry.key.as_bytes() == key_bytes)
        else {
            self.push_entry(Entry::new(key, value));
            return;
        };
        self.entries[last].value = value.into();
        if self.entries[..last]
            .iter()
            .any(|entry| entry.key.as_bytes() == key_bytes)
        {
            let mut idx = 0;
            self.entries.retain(|entry| {
                let keep = idx == last || entry.key.as_bytes() != key_bytes;
                idx += 1;
                keep
            });
        }
    }

    /// Removes every entry matching `key` and returns the effective (last) entry's value.
    pub fn remove(&mut self, key: impl AsRef<[u8]>) -> Option<Object> {
        let key = key.as_ref();
        let last = self
            .entries
            .iter()
            .rposition(|entry| entry.key.as_bytes() == key)?;
        let value = self.entries.remove(last).value;
        self.entries.retain(|entry| entry.key.as_bytes() != key);
        Some(value)
    }

    /// Removes all shadowed duplicate entries, keeping each key's last occurrence in order.
    pub fn remove_duplicates(&mut self) {
        let shadowed = Shadowed::of(&self.entries);
        if shadowed.len() == 0 {
            return;
        }
        let mut index = 0;
        self.entries.retain(|_| {
            let kept = !shadowed.contains(index);
            index += 1;
            kept
        });
    }
}

const FEW: usize = 64;

#[derive(Debug, Clone)]
enum Shadowed {
    None,
    Few(u64),
    Many { bits: Box<[u64]>, count: usize },
}

impl Shadowed {
    fn of(entries: &[Entry]) -> Shadowed {
        if entries.len() <= 1 {
            return Shadowed::None;
        }
        if entries.len() <= FEW {
            let mut mask = 0u64;
            for (i, entry) in entries.iter().enumerate() {
                let key = entry.key.as_bytes();
                if entries[i + 1..]
                    .iter()
                    .any(|later| later.key.as_bytes() == key)
                {
                    mask |= 1u64 << i;
                }
            }
            return if mask == 0 {
                Shadowed::None
            } else {
                Shadowed::Few(mask)
            };
        }

        // Guaranteed O(n log n) worst-case with a single temporary index buffer (no hash-DoS).
        let mut order: Vec<usize> = (0..entries.len()).collect();
        order.sort_unstable_by(|&a, &b| {
            entries[a]
                .key
                .as_bytes()
                .cmp(entries[b].key.as_bytes())
                .then(a.cmp(&b))
        });

        let has_dups = order
            .windows(2)
            .any(|w| entries[w[0]].key.as_bytes() == entries[w[1]].key.as_bytes());
        if !has_dups {
            return Shadowed::None;
        }

        let mut bits = alloc::vec![0u64; entries.len().div_ceil(64)].into_boxed_slice();
        let mut count = 0usize;
        for w in order.windows(2) {
            if entries[w[0]].key.as_bytes() == entries[w[1]].key.as_bytes() {
                let idx = w[0];
                bits[idx >> 6] |= 1u64 << (idx & 63);
                count += 1;
            }
        }
        Shadowed::Many { bits, count }
    }

    #[inline]
    fn contains(&self, i: usize) -> bool {
        match self {
            Shadowed::None => false,
            Shadowed::Few(mask) => i < FEW && (mask & (1u64 << i)) != 0,
            Shadowed::Many { bits, .. } => bits
                .get(i >> 6)
                .is_some_and(|w| (w & (1u64 << (i & 63))) != 0),
        }
    }

    #[inline]
    fn len(&self) -> usize {
        match self {
            Shadowed::None => 0,
            Shadowed::Few(mask) => mask.count_ones() as usize,
            Shadowed::Many { count, .. } => *count,
        }
    }
}

/// Iterator over effective `(key, value)` pairs in a [`Dict`].
#[must_use = "iterators are lazy and do nothing unless consumed"]
#[derive(Debug, Clone)]
pub struct Iter<'a> {
    entries: Enumerate<slice::Iter<'a, Entry>>,
    shadowed: Shadowed,
    remaining: usize,
}

impl<'a> Iterator for Iter<'a> {
    type Item = (&'a [u8], &'a Object);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let shadowed = &self.shadowed;
        let item = self
            .entries
            .by_ref()
            .find(|(i, _)| !shadowed.contains(*i))
            .map(|(_, entry)| (entry.key.as_bytes(), &entry.value));
        if item.is_some() {
            self.remaining -= 1;
        }
        item
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }

    #[inline]
    fn fold<B, F: FnMut(B, Self::Item) -> B>(self, init: B, mut f: F) -> B {
        let shadowed = self.shadowed;
        self.entries.fold(init, |acc, (i, entry)| {
            if shadowed.contains(i) {
                acc
            } else {
                f(acc, (entry.key.as_bytes(), &entry.value))
            }
        })
    }
}

impl DoubleEndedIterator for Iter<'_> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        let shadowed = &self.shadowed;
        let item = self
            .entries
            .by_ref()
            .rfind(|(i, _)| !shadowed.contains(*i))
            .map(|(_, entry)| (entry.key.as_bytes(), &entry.value));
        if item.is_some() {
            self.remaining -= 1;
        }
        item
    }
}

impl ExactSizeIterator for Iter<'_> {
    #[inline]
    fn len(&self) -> usize {
        self.remaining
    }
}

impl FusedIterator for Iter<'_> {}

/// Mutable iterator over effective `(key, value)` pairs in a [`Dict`].
#[must_use = "iterators are lazy and do nothing unless consumed"]
#[derive(Debug)]
pub struct IterMut<'a> {
    entries: Enumerate<slice::IterMut<'a, Entry>>,
    shadowed: Shadowed,
    remaining: usize,
}

impl<'a> Iterator for IterMut<'a> {
    type Item = (&'a [u8], &'a mut Object);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let shadowed = &self.shadowed;
        let item = self
            .entries
            .by_ref()
            .find(|(i, _)| !shadowed.contains(*i))
            .map(|(_, entry)| (entry.key.as_bytes(), &mut entry.value));
        if item.is_some() {
            self.remaining -= 1;
        }
        item
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }

    #[inline]
    fn fold<B, F: FnMut(B, Self::Item) -> B>(self, init: B, mut f: F) -> B {
        let shadowed = self.shadowed;
        self.entries.fold(init, |acc, (i, entry)| {
            if shadowed.contains(i) {
                acc
            } else {
                f(acc, (entry.key.as_bytes(), &mut entry.value))
            }
        })
    }
}

impl DoubleEndedIterator for IterMut<'_> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        let shadowed = &self.shadowed;
        let item = self
            .entries
            .by_ref()
            .rfind(|(i, _)| !shadowed.contains(*i))
            .map(|(_, entry)| (entry.key.as_bytes(), &mut entry.value));
        if item.is_some() {
            self.remaining -= 1;
        }
        item
    }
}

impl ExactSizeIterator for IterMut<'_> {
    #[inline]
    fn len(&self) -> usize {
        self.remaining
    }
}

impl FusedIterator for IterMut<'_> {}

/// Iterator over each key's effective (last) [`Entry`] in a [`Dict`].
#[must_use = "iterators are lazy and do nothing unless consumed"]
#[derive(Debug, Clone)]
pub struct LastEntries<'a> {
    entries: Enumerate<slice::Iter<'a, Entry>>,
    shadowed: Shadowed,
    remaining: usize,
}

impl<'a> Iterator for LastEntries<'a> {
    type Item = &'a Entry;

    #[inline]
    fn next(&mut self) -> Option<&'a Entry> {
        let shadowed = &self.shadowed;
        let item = self
            .entries
            .by_ref()
            .find(|(i, _)| !shadowed.contains(*i))
            .map(|(_, entry)| entry);
        if item.is_some() {
            self.remaining -= 1;
        }
        item
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }

    #[inline]
    fn fold<B, F: FnMut(B, &'a Entry) -> B>(self, init: B, mut f: F) -> B {
        let shadowed = self.shadowed;
        self.entries.fold(init, |acc, (i, entry)| {
            if shadowed.contains(i) {
                acc
            } else {
                f(acc, entry)
            }
        })
    }
}

impl DoubleEndedIterator for LastEntries<'_> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        let shadowed = &self.shadowed;
        let item = self
            .entries
            .by_ref()
            .rfind(|(i, _)| !shadowed.contains(*i))
            .map(|(_, entry)| entry);
        if item.is_some() {
            self.remaining -= 1;
        }
        item
    }
}

impl ExactSizeIterator for LastEntries<'_> {
    #[inline]
    fn len(&self) -> usize {
        self.remaining
    }
}

impl FusedIterator for LastEntries<'_> {}

/// Iterator over shadowed duplicate [`Entry`] items in a [`Dict`].
#[must_use = "iterators are lazy and do nothing unless consumed"]
#[derive(Debug, Clone)]
pub struct Duplicates<'a> {
    entries: Enumerate<slice::Iter<'a, Entry>>,
    shadowed: Shadowed,
    remaining: usize,
}

impl<'a> Iterator for Duplicates<'a> {
    type Item = &'a Entry;

    #[inline]
    fn next(&mut self) -> Option<&'a Entry> {
        if self.remaining == 0 {
            self.entries = [].iter().enumerate();
            return None;
        }
        let shadowed = &self.shadowed;
        let item = self
            .entries
            .by_ref()
            .find(|(i, _)| shadowed.contains(*i))
            .map(|(_, entry)| entry);
        if item.is_some() {
            self.remaining -= 1;
        }
        item
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }

    #[inline]
    fn fold<B, F: FnMut(B, &'a Entry) -> B>(self, init: B, mut f: F) -> B {
        if self.remaining == 0 {
            return init;
        }
        let shadowed = self.shadowed;
        self.entries.fold(init, |acc, (i, entry)| {
            if shadowed.contains(i) {
                f(acc, entry)
            } else {
                acc
            }
        })
    }
}

impl DoubleEndedIterator for Duplicates<'_> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            self.entries = [].iter().enumerate();
            return None;
        }
        let shadowed = &self.shadowed;
        let item = self
            .entries
            .by_ref()
            .rfind(|(i, _)| shadowed.contains(*i))
            .map(|(_, entry)| entry);
        if item.is_some() {
            self.remaining -= 1;
        }
        item
    }
}

impl ExactSizeIterator for Duplicates<'_> {
    #[inline]
    fn len(&self) -> usize {
        self.remaining
    }
}

impl FusedIterator for Duplicates<'_> {}

/// Stack-safe iterator over indirect [`ObjectId`]s referenced inside an [`Object`].
#[must_use = "iterators are lazy and do nothing unless consumed"]
#[derive(Debug, Clone)]
pub struct References<'a> {
    stack: Vec<&'a Object>,
}

impl Iterator for References<'_> {
    type Item = ObjectId;

    #[inline]
    fn next(&mut self) -> Option<ObjectId> {
        while let Some(object) = self.stack.pop() {
            let entries = match &object.value {
                Value::Ref(id) => return Some(*id),
                Value::Array(array) => {
                    self.stack.extend(array.iter().rev());
                    continue;
                }
                Value::Dict(dict) => &dict.entries,
                Value::Stream(stream) => &stream.dict.entries,
                _ => continue,
            };
            self.stack
                .extend(entries.iter().rev().map(|entry| &entry.value));
        }
        None
    }
}

impl FusedIterator for References<'_> {}

impl<'a> IntoIterator for &'a Dict {
    type Item = (&'a [u8], &'a Object);
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

impl<K: Into<Name>, V: Into<Object>> FromIterator<(K, V)> for Dict {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Dict {
        let iter = iter.into_iter();
        let mut dict = Dict::with_capacity(iter.size_hint().0);
        for (key, value) in iter {
            dict.push(key, value);
        }
        dict
    }
}

impl From<Value> for Object {
    fn from(value: Value) -> Object {
        Object::new(value)
    }
}

impl From<bool> for Object {
    fn from(b: bool) -> Object {
        Object::new(Value::Bool(b))
    }
}

impl From<i64> for Object {
    fn from(i: i64) -> Object {
        Object::new(Value::Integer(i))
    }
}

impl From<i32> for Object {
    fn from(i: i32) -> Object {
        Object::new(Value::Integer(i.into()))
    }
}

impl From<i16> for Object {
    fn from(i: i16) -> Object {
        Object::new(Value::Integer(i.into()))
    }
}

impl From<i8> for Object {
    fn from(i: i8) -> Object {
        Object::new(Value::Integer(i.into()))
    }
}

impl From<u32> for Object {
    fn from(i: u32) -> Object {
        Object::new(Value::Integer(i.into()))
    }
}

impl From<u16> for Object {
    fn from(i: u16) -> Object {
        Object::new(Value::Integer(i.into()))
    }
}

impl From<u8> for Object {
    fn from(i: u8) -> Object {
        Object::new(Value::Integer(i.into()))
    }
}

impl From<f64> for Object {
    fn from(r: f64) -> Object {
        Object::new(Value::Real(r))
    }
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

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).expect("ASCII decimal formatting")
    }
}

impl<const N: usize> fmt::Write for StackBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        if self.len + bytes.len() > N {
            return Err(fmt::Error);
        }
        self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
}

/// Converts `f32` to the `f64` nearest its shortest decimal representation (`0.1f32` becomes
/// `0.1`, not `0.10000000149011612`), formatting onto a stack buffer with zero heap allocations.
impl From<f32> for Object {
    fn from(r: f32) -> Object {
        if !r.is_finite() || r == 0.0 {
            return Object::new(Value::Real(f64::from(r)));
        }
        if r > -16_777_216.0 && r < 16_777_216.0 && (r as i32 as f32) == r {
            return Object::new(Value::Real(f64::from(r)));
        }
        let mut buf = StackBuf::<64>::new();
        write!(&mut buf, "{r}").expect("f32 Display fits in 64 bytes");
        let parsed = buf
            .as_str()
            .parse::<f64>()
            .expect("finite f32 Display parses as f64");
        Object::new(Value::Real(parsed))
    }
}

impl From<Name> for Object {
    fn from(name: Name) -> Object {
        Object::new(Value::Name(name))
    }
}

impl From<ObjectId> for Object {
    fn from(id: ObjectId) -> Object {
        Object::new(Value::Ref(id))
    }
}

impl From<Vec<Object>> for Object {
    fn from(array: Vec<Object>) -> Object {
        Object::new(Value::Array(array))
    }
}

impl From<Dict> for Object {
    fn from(dict: Dict) -> Object {
        Object::new(Value::Dict(dict))
    }
}

impl From<Stream> for Object {
    fn from(stream: Stream) -> Object {
        Object::new(Value::Stream(Box::new(stream)))
    }
}

/// Constructs a [`Dict`] from `key => value` pairs in written order.
///
/// Keys accept anything implementing `Into<Name>` (`&str`, `&[u8]`, `Name`, etc.), and values
/// accept anything implementing `Into<Object>`. Use [`Object::name`] for PDF name values and
/// [`Object::text`] for PDF text strings:
///
/// ```
/// use pdf_objects::{Object, dict};
///
/// let catalog = dict! {
///     "Type" => Object::name("Catalog"),
///     "Pages" => Object::reference(2),
/// };
/// assert!(catalog.has_type("Catalog"));
/// ```
#[macro_export]
macro_rules! dict {
    () => { $crate::object::Dict::new() };
    ($($key:expr => $value:expr),+ $(,)?) => {{
        let mut dict = $crate::object::Dict::new();
        $( dict.push($key, $value); )+
        dict
    }};
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;
    use alloc::format;
    use alloc::string::ToString;
    use alloc::vec;

    use super::*;

    #[test]
    fn object_id_layout_and_conversions() {
        const ID: ObjectId = ObjectId::new(12, 3);
        assert_eq!(size_of::<ObjectId>(), 8);
        assert_eq!((ID.number, ID.generation), (12, 3));
        assert_eq!(ID.to_string(), "12 0".replace("0", "3"));
        assert_eq!(format!("{} obj", ObjectId::new(7, 65535)), "7 65535 obj");

        for pair in [(0, 0), (1, 0), (12, 3), (u32::MAX, u16::MAX), (65536, 1)] {
            let id = ObjectId::from(pair);
            assert_eq!(<(u32, u16)>::from(id), pair);
        }

        let pairs = [
            (3, 0),
            (1, 2),
            (1, 0),
            (0, 5),
            (u32::MAX, u16::MAX),
            (3, 1),
            (2, 0),
            (1, 1),
        ];
        let ids: Vec<ObjectId> = pairs.iter().map(|&(n, g)| ObjectId::new(n, g)).collect();
        let sorted: Vec<(u32, u16)> = ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(Into::into)
            .collect();
        let mut by_pair = pairs.to_vec();
        by_pair.sort();
        assert_eq!(sorted, by_pair);
    }

    #[test]
    fn span_accessors_and_safe_slicing() {
        let file = b"0123456789";
        let top = Span::new(None, 2, 6);
        assert_eq!(
            (top.stream(), top.start(), top.end(), top.len()),
            (None, 2, 6, 4)
        );
        assert_eq!(top.range(), 2..6);
        assert!(!top.is_empty());
        assert_eq!(top.slice_in(file), Some(&b"2345"[..]));
        assert_eq!(top.stream_slice_in(ObjectId::new(1, 0), file), None);

        let in_stream = Span::new(Some(ObjectId::new(9, 0)), 1, 4);
        assert_eq!(in_stream.slice_in(file), None);
        assert_eq!(
            in_stream.stream_slice_in(ObjectId::new(9, 0), file),
            Some(&b"123"[..])
        );
        assert_eq!(in_stream.stream_slice_in(ObjectId::new(8, 0), file), None);

        assert_eq!(Span::try_new(None, 5, 4), None);
        assert_eq!(Span::try_new(None, 5, 5).unwrap().len(), 0);
        assert!(Span::new(None, 5, 5).is_empty());
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "Span::new requires start <= end")]
    fn span_new_panics_in_debug_when_inverted() {
        let _ = Span::new(None, 10, 9);
    }

    #[test]
    fn small_bytes_and_name_cover_inline_medium_and_large_tiers() {
        for len in [0, 1, 14, 15, 64, 65, 300] {
            let raw: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let sb = SmallBytes::from_slice(&raw);
            assert_eq!(sb.len(), len);
            assert_eq!(sb.is_empty(), len == 0);
            assert_eq!(sb, raw.as_slice());
            assert_eq!(sb, raw);
            let cloned = sb.clone();
            assert_eq!(cloned, sb);
            assert_eq!(sb.into_vec(), raw);

            let from_iter: SmallBytes = raw.iter().copied().collect();
            assert_eq!(from_iter, raw.as_slice());

            let name = Name::from(raw.clone());
            assert_eq!(name.as_bytes(), raw.as_slice());
            assert_eq!(name.into_vec(), raw);
        }

        let n = Name::new("Type");
        assert_eq!(n.as_str(), Some("Type"));
        assert_eq!(n, "Type");
        assert_eq!(n, *b"Type");
        assert_eq!(format!("{n:?}"), "/Type");
        assert_eq!(format!("{:?}", Name::new(b"A#/\0")), "/A#23#2F#00");
    }

    #[test]
    fn debug_redacts_encrypted_strings_and_stream_bytes() {
        let secret = Str::encrypted(*b"top-secret-plaintext");
        let dbg = format!("{secret:?}");
        assert_eq!(dbg, "Encrypted(20 bytes)");
        assert!(!dbg.contains("secret"));

        let data = Data::Bytes(b"raw-stream-payload".to_vec());
        let data_dbg = format!("{data:?}");
        assert_eq!(data_dbg, "Bytes(18 bytes)");
        assert!(!data_dbg.contains("payload"));
    }

    #[test]
    fn const_construction_and_accessors() {
        const AT: Span = Span::new(None, 3, 8);
        const PLACED: &Object = &Object::reference(7).with_span(AT);
        const REAL: &Object = &Object::new(Value::Real(5.5));
        const NULL: &Object = &Object::null();
        const EMPTY: &Dict = &Dict::new();
        assert_eq!(PLACED.at(), Some(AT));
        assert_eq!(REAL.as_number(), Some(5.5));
        assert!(NULL.is_null());
        assert_eq!(EMPTY, &Dict::EMPTY);
    }

    #[test]
    fn str_decode_helpers_respect_rules_and_encryption() {
        let utf16_le = Str::bytes(*b"\xFF\xFEA\x00");
        assert_eq!(utf16_le.decode().unwrap().text, "ÿþA\u{FFFD}");
        assert_eq!(utf16_le.decode_pdfium().unwrap().text, "A");
        assert_eq!(
            utf16_le.decode_str(pdf_tokenizer::Rules::Iso).unwrap(),
            "ÿþA\u{FFFD}"
        );
        assert_eq!(
            utf16_le.decode_str(pdf_tokenizer::Rules::Pdfium).unwrap(),
            "A"
        );
        assert_eq!(utf16_le.text().unwrap(), "ÿþA\u{FFFD}");
        assert!(!utf16_le.is_encrypted());

        let enc = Str::encrypted(*b"\xFF\xFEA\x00");
        assert!(enc.is_encrypted());
        assert!(enc.decode().is_none());
        assert!(enc.decode_pdfium().is_none());
        assert!(enc.decode_str(pdf_tokenizer::Rules::Iso).is_none());
        assert!(enc.text().is_none());
        assert!(enc.as_bytes().is_none());
    }

    #[test]
    fn text_objects_choose_form_and_round_trip() {
        let plain = Object::text("plain");
        assert_eq!(
            plain.value(),
            &Value::String(Str::bytes(*b"plain"), Some(Form::Literal))
        );
        let marked = Object::text("é");
        assert_eq!(
            marked.value(),
            &Value::String(Str::bytes(*b"\xFE\xFF\x00\xE9"), Some(Form::Hex))
        );
        // NUL and DEL are outside verbatim PDFDocEncoding, so they use UTF-16BE Hex.
        let nul = Object::text("\t\0");
        assert_eq!(nul.as_bytes(), Some(&b"\xFE\xFF\x00\t\x00\0"[..]));

        for sample in [
            "", "a\rb", "a\r\nb", "a\nb", "a(b", "a)b", "(a)", "a\\b", "\\", "\t\0", "é\r",
            "a\x7F", "\x01\x17",
        ] {
            let mut written = Vec::new();
            crate::serialize::write(&mut written, &Object::text(sample)).unwrap();
            for rules in [pdf_tokenizer::Rules::Iso, pdf_tokenizer::Rules::Pdfium] {
                let parsed = crate::parse::Parser::at(&written, 0)
                    .rules(rules)
                    .value(&mut ())
                    .unwrap();
                assert_eq!(
                    parsed.as_str().and_then(|s| s.decode_str(rules)),
                    Some(sample.to_string()),
                    "{written:?}"
                );
            }
        }
    }

    #[test]
    fn dict_set_overwrites_last_and_removes_shadowed_duplicates() {
        let mut d = dict! { "A" => 1, "B" => 2, "A" => 3, "C" => 4 };
        assert_eq!(d.get("A").and_then(Object::as_integer), Some(3));
        assert_eq!(d.len(), 3);
        assert_eq!(d.duplicates().len(), 1);

        d.set("A", 99);
        assert_eq!(d.entries().len(), 3);
        assert_eq!(d.duplicates().len(), 0);
        let items: Vec<_> = d.iter().map(|(k, v)| (k, v.as_integer())).collect();
        assert_eq!(
            items,
            [
                (&b"B"[..], Some(2)),
                (&b"A"[..], Some(99)),
                (&b"C"[..], Some(4))
            ]
        );

        // Stream::set_bytes cleans up any shadowed indirect /Length reference.
        let mut s = Stream {
            dict: dict! {
                "Length" => Object::reference(10),
                "Filter" => Object::name("FlateDecode"),
                "Length" => 50,
            },
            data: Data::Unplaced,
        };
        s.set_bytes(b"hello".to_vec());
        assert_eq!(Object::from(s).references().collect::<Vec<_>>(), []);
    }

    #[test]
    fn dict_exact_size_iterators_few_and_many() {
        for n in [0usize, 1, 16, 64, 65, 130] {
            let mut d = Dict::new();
            if n >= 3 {
                for i in 0..(n - 3) {
                    d.push(format!("K{i}"), i as i64);
                }
                d.push("K0", -1);
                d.push("X", 1);
                d.push("X", 2);
            } else {
                for i in 0..n {
                    d.push(format!("K{i}"), i as i64);
                }
            }

            let expected_dups = if n >= 4 {
                2
            } else if n == 3 {
                1
            } else {
                0
            };
            let expected_len = n - expected_dups;
            assert_eq!(d.len(), expected_len, "n={n}");
            assert_eq!(d.iter().len(), expected_len, "n={n}");
            assert_eq!(d.last_entries().len(), expected_len, "n={n}");
            assert_eq!(d.duplicates().len(), expected_dups, "n={n}");

            let mut it = d.iter();
            let mut rem = expected_len;
            while it.next().is_some() {
                rem -= 1;
                assert_eq!(it.len(), rem);
            }
            assert_eq!(it.next(), None);

            let mut rev_it = d.iter();
            let mut rem = expected_len;
            while rev_it.next_back().is_some() {
                rem -= 1;
                assert_eq!(rev_it.len(), rem);
            }
            assert_eq!(rev_it.next_back(), None);

            let fwd_dups: Vec<_> = d.duplicates().map(|e| e.key().to_vec()).collect();
            let mut bwd_dups: Vec<_> = d.duplicates().rev().map(|e| e.key().to_vec()).collect();
            bwd_dups.reverse();
            assert_eq!(fwd_dups, bwd_dups);
        }
    }

    #[test]
    fn eq_value_ignores_spans_forms_key_order_and_duplicates() {
        let a = crate::parse::Parser::at(b"<< /B (hi) /A 1 /A [2 3] >>", 0)
            .value(&mut ())
            .unwrap();
        let b = Object::from(dict! {
            "A" => vec![Object::from(2), Object::from(3)],
            "B" => Object::hex(*b"hi"),
        });
        assert_ne!(a, b);
        assert!(a.eq_value(&b));
        assert!(b.eq_value(&a));

        // Large (>32 entry) dictionaries compare order-independently as well.
        let d1: Object = (0..50)
            .map(|i| (format!("K{i}"), i))
            .collect::<Dict>()
            .into();
        let d2: Object = (0..50)
            .rev()
            .map(|i| (format!("K{i}"), i))
            .collect::<Dict>()
            .into();
        assert!(d1.eq_value(&d2));
    }

    fn read(text: &[u8]) -> Object {
        crate::parse::Parser::at(text, 0).value(&mut ()).unwrap()
    }

    fn stream_obj(file: &[u8]) -> Object {
        use crate::parse::{Body, Parser};
        let mut parser = Parser::at(file, 0);
        let id = parser.header(&mut ()).unwrap();
        let Ok(Body::Stream(head)) = parser.body(id, &mut ()) else {
            panic!("expected stream");
        };
        let length = head.dict().get("Length").and_then(Object::as_integer);
        let data = head.declared(length.unwrap() as usize).unwrap();
        parser.stream(head, data, &mut ()).unwrap()
    }

    #[test]
    fn edit_tracking_clears_ancestor_spans_only() {
        let text = b"<< /A [1 [2 3]] /B 4 >>";
        let mut o = read(text);
        let a = o.as_dict_mut().unwrap().get_mut("A").unwrap();
        let inner = &mut a.as_array_mut().unwrap()[1];
        *inner.as_array_mut().unwrap()[0].value_mut() = Value::Integer(5);

        let d = o.as_dict().unwrap();
        let a = d.get("A").unwrap();
        let [one, inner] = a.as_array().unwrap() else {
            panic!()
        };
        let [five, three] = inner.as_array().unwrap() else {
            panic!()
        };
        for changed in [&o, a, inner, five] {
            assert_eq!(changed.at(), None);
        }
        assert_eq!(three.at().unwrap().slice_in(text), Some(&b"3"[..]));
        assert_eq!(one.at().unwrap().slice_in(text), Some(&b"1"[..]));
        assert_eq!(
            d.get("B").unwrap().at().unwrap().slice_in(text),
            Some(&b"4"[..])
        );
        assert!(d.entries().iter().all(|e| e.key_at().is_some()));

        // Entry::set_key clears key_at.
        let mut o2 = read(b"<< /Old 1 >>");
        let entry = o2.as_dict_mut().unwrap().entries_mut().next().unwrap();
        assert!(entry.key_at().is_some());
        entry.set_key("New");
        assert!(entry.key_at().is_none());
        assert_eq!(entry.key(), b"New");
    }

    #[test]
    fn renumber_and_unplace_preserve_invariants() {
        let file = b"4 0 obj\n<< /Length 5 /F 1 0 R /G 2 0 R >>\nstream\nhello\nendstream\nendobj";
        let mut s = stream_obj(file);
        assert!(s.renumber(|id: ObjectId| (id.number != 1).then_some(id)));
        assert_eq!(s.at(), None);
        let st = s.as_stream().unwrap();
        assert_eq!(st.data.at().unwrap().slice_in(file), Some(&b"hello"[..]));
        assert_eq!(st.dict.get("F"), Some(&Object::null()));
        assert_eq!(
            st.dict.get("G").unwrap().at().unwrap().slice_in(file),
            Some(&b"2 0 R"[..])
        );

        s.unplace();
        assert_eq!(s.as_stream().unwrap().data, Data::Unplaced);
    }

    #[test]
    fn f32_conversion_is_stack_buffered_and_shortest_decimal() {
        assert_eq!(Object::from(0.1f32).value, Value::Real(0.1));
        assert_eq!(Object::from(-3.0f32).value, Value::Real(-3.0));
        assert!(
            Object::from(-0.0f32)
                .as_number()
                .unwrap()
                .is_sign_negative()
        );
        assert_eq!(Object::from(1e30f32).value, Value::Real(1e30));
        assert_eq!(Object::from(1e-45f32).value, Value::Real(1e-45));
        assert_eq!(Object::from(16_777_216f32).value, Value::Real(16_777_216.0));
        assert_eq!(
            Object::from(f32::INFINITY).value,
            Value::Real(f64::INFINITY)
        );
        assert!(Object::from(f32::NAN).as_number().is_some_and(f64::is_nan));
    }

    fn nested(depth: u32) -> Object {
        let at = |n: u32| Span::new(None, n as usize, n as usize + 1);
        let mut value = Object::reference(depth).with_span(at(depth));
        for n in (0..depth).rev() {
            value = match n % 2 {
                0 => Object::from(vec![Object::reference(n).with_span(at(n)), value]),
                _ => {
                    let mut dict = Dict::new();
                    dict.push_entry(
                        Entry::new("N", Object::reference(n).with_span(at(n))).with_key_span(at(n)),
                    );
                    dict.push_entry(Entry::new("Kid", value).with_key_span(at(n)));
                    Object::from(dict)
                }
            };
            value = value.with_span(at(n));
        }
        value
    }

    #[test]
    fn deep_objects_clone_compare_walk_and_drop_on_64kib_stack() {
        const DEPTH: u32 = 10_000;
        let built = std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(|| {
                let mut a = nested(DEPTH);
                let b = a.clone();
                assert_eq!(a, b);
                assert!(a.eq_value(&b));
                assert_eq!(a.references().count(), (DEPTH + 1) as usize);
                assert!(a.renumber(|id| Some(ObjectId::new(id.number + 1, id.generation))));
                assert_ne!(a, b);
                assert!(!a.eq_value(&b));
                a.unplace();
                drop(a);
                drop(b);
            })
            .unwrap();
        built.join().unwrap();
    }

    #[test]
    fn types_are_send_and_sync() {
        fn shared<T: Send + Sync>() {}
        shared::<Object>();
        shared::<Value>();
        shared::<SmallBytes>();
        shared::<Name>();
        shared::<Str>();
        shared::<Dict>();
        shared::<Entry>();
        shared::<Iter<'_>>();
        shared::<LastEntries<'_>>();
        shared::<Duplicates<'_>>();
        shared::<Stream>();
        shared::<Data>();
        shared::<Span>();
    }
}
