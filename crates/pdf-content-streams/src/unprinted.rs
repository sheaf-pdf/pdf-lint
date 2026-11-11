//! pdf-objects' values as this crate's `Debug`s give them: in pdf-objects'
//! shape, names, numbers and structure as they are, but each string and a
//! stream's data held in memory by its length, as they're read from a
//! page's content or a file, which may be decrypted, and a string may be
//! a page's text.

use std::fmt;

use pdf_objects::{Data, Dict, Entry, Object, Str, Value};

/// Data's length, as a `Debug` gives data it doesn't print.
pub(crate) struct Len(pub(crate) usize);

impl fmt::Debug for Len {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} bytes", self.0)
    }
}

/// A value as this crate's `Debug`s give it: as pdf-objects' `Debug`
/// writes it, but each string and a stream's data held in memory by its
/// length ([`Len`]), as they may be a page's text, or decrypted.
pub(crate) struct Unprinted<'v, T: ?Sized>(pub(crate) &'v T);

impl fmt::Debug for Unprinted<'_, Object> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Object")
            .field("value", &Unprinted(self.0.value()))
            .field("at", &self.0.at)
            .finish()
    }
}

impl fmt::Debug for Unprinted<'_, Value> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Value::String(text, form) => {
                let (kind, len) = match text {
                    Str::Bytes(bytes) => ("Bytes", bytes.len()),
                    Str::Text(text) => ("Text", text.len()),
                    Str::Encrypted(bytes) => ("Encrypted", bytes.len()),
                };
                let text = DebugTuple(kind, Len(len));
                f.debug_tuple("String").field(&text).field(form).finish()
            }
            Value::Array(items) => {
                let items: Vec<_> = items.iter().map(Unprinted).collect();
                f.debug_tuple("Array").field(&items).finish()
            }
            Value::Dict(dict) => f.debug_tuple("Dict").field(&Unprinted(dict)).finish(),
            Value::Stream(stream) => {
                let data: &dyn fmt::Debug = match &stream.data {
                    Data::Bytes(bytes) => &DebugTuple("Bytes", Len(bytes.len())),
                    data => data,
                };
                let shown = DebugStream(Unprinted(&stream.dict), data);
                f.debug_tuple("Stream").field(&shown).finish()
            }
            value => fmt::Debug::fmt(value, f),
        }
    }
}

impl fmt::Debug for Unprinted<'_, Dict> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let entries: Vec<_> = (self.0.entries().iter())
            .map(|entry| DebugEntry(entry, Unprinted(&entry.value)))
            .collect();
        f.debug_struct("Dict").field("entries", &entries).finish()
    }
}

/// A tuple variant of one field, `kind(field)`.
pub(crate) struct DebugTuple<T>(pub(crate) &'static str, pub(crate) T);

impl<T: fmt::Debug> fmt::Debug for DebugTuple<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple(self.0).field(&self.1).finish()
    }
}

/// A stream, its dictionary and its data as given.
struct DebugStream<'v>(Unprinted<'v, Dict>, &'v dyn fmt::Debug);

impl fmt::Debug for DebugStream<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stream")
            .field("dict", &self.0)
            .field("data", self.1)
            .finish()
    }
}

/// A dictionary's entry, its value as given.
struct DebugEntry<'v>(&'v Entry, Unprinted<'v, Object>);

impl fmt::Debug for DebugEntry<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("key", &self.0.key)
            .field("key_at", &self.0.key_at)
            .field("value", &self.1)
            .finish()
    }
}
