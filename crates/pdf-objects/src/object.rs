//! The objects a checked file holds, as a reader read them: owned, each
//! value with where it was written when the reader knows, and each
//! dictionary as written, in order and with any key written twice.

use std::collections::HashSet;

use crate::text;

/// An indirect object's number and generation.
pub type ObjectId = (u32, u16);

/// Where a value was written: a run of the file's bytes, or of an object
/// stream's decoded data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct Span {
    /// The object stream whose decoded data it's in; none for the file.
    pub stream: Option<ObjectId>,
    /// The offset of its first byte.
    pub start: usize,
    /// The offset just past its last byte.
    pub end: usize,
}

impl Span {
    /// Bytes `start..end` of the file, or of object stream `stream`'s
    /// decoded data.
    pub const fn new(stream: Option<ObjectId>, start: usize, end: usize) -> Span {
        Span { stream, start, end }
    }
}

/// A value, and where it was written.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    /// The value itself.
    pub value: Value,
    /// Where it was written, when the reader knows: a reader of the file's
    /// bytes always does, and one through another tool's JSON, as qpdf's,
    /// may not.
    pub at: Option<Span>,
}

/// PDF's objects (ISO 32000-2, 7.3).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// The null object.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// An integer.
    Integer(i64),
    /// A real number.
    Real(f64),
    /// A name's bytes, its `#xx` escapes decoded.
    Name(Vec<u8>),
    /// A string, literal or hex.
    String(Str),
    /// An array: its elements, in order.
    Array(Vec<Object>),
    /// A dictionary.
    Dict(Dict),
    /// A stream: its dictionary, and where its data is.
    Stream(Stream),
    /// A reference to an indirect object, `n g R`.
    Ref(ObjectId),
}

/// A string's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Str {
    /// Its bytes, its escapes decoded (and decrypted, in an encrypted
    /// file).
    Bytes(Vec<u8>),
    /// Text a reader decoded without saying from which encoding, as a
    /// reader through another tool's JSON may give a string it reads as
    /// text (one through qpdf's does): what depends on the bytes, such as
    /// whether they're ASCII, can't be told. A reader of the file's bytes
    /// never gives it.
    Text(String),
    /// Ciphertext as written, in an encrypted file its reader doesn't
    /// decrypt: what depends on its value can't be told.
    Encrypted(Vec<u8>),
}

/// A dictionary: its entries as written, in order. Lookups take a key's
/// last entry, as readers do; [`Dict::duplicates`] gives the others.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dict {
    entries: Vec<Entry>,
}

/// A dictionary's entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The key's bytes, its `#xx` escapes decoded.
    pub key: Vec<u8>,
    /// Where the key was written, when the reader knows.
    pub key_at: Option<Span>,
    /// The key's value.
    pub value: Object,
}

/// A stream: its dictionary, and where its data is.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Stream {
    /// Its dictionary.
    pub dict: Dict,
    /// Where its data was written, when the reader knows.
    pub data: Option<Span>,
}

impl Object {
    /// `value`, not placed.
    pub fn new(value: Value) -> Object {
        Object { value, at: None }
    }

    /// The null object, not placed.
    pub fn null() -> Object {
        Object::new(Value::Null)
    }

    /// A reference to object `n`, generation 0.
    pub fn reference(n: u32) -> Object {
        Object::new(Value::Ref((n, 0)))
    }

    /// A string of `text`'s bytes.
    pub fn string(text: &str) -> Object {
        Object::new(Value::String(Str::Bytes(text.as_bytes().to_vec())))
    }

    /// A string of `bytes`.
    pub fn bytes(bytes: impl Into<Vec<u8>>) -> Object {
        Object::new(Value::String(Str::Bytes(bytes.into())))
    }

    /// A stream of `dict`, whose data isn't placed.
    pub fn stream(dict: Dict) -> Object {
        Object::new(Value::Stream(Stream { dict, data: None }))
    }

    /// The same value, written at `at`.
    pub fn at(self, at: Span) -> Object {
        Object {
            at: Some(at),
            ..self
        }
    }

    /// A name's bytes.
    pub fn as_name(&self) -> Option<&[u8]> {
        match &self.value {
            Value::Name(name) => Some(name),
            _ => None,
        }
    }

    /// An integer's value.
    pub fn as_integer(&self) -> Option<i64> {
        match self.value {
            Value::Integer(i) => Some(i),
            _ => None,
        }
    }

    /// An integer's or a real's value.
    pub fn as_number(&self) -> Option<f64> {
        match self.value {
            Value::Integer(i) => Some(i as f64),
            Value::Real(r) => Some(r),
            _ => None,
        }
    }

    /// A dictionary, or a stream's dictionary.
    pub fn as_dict(&self) -> Option<&Dict> {
        match &self.value {
            Value::Dict(dict) | Value::Stream(Stream { dict, .. }) => Some(dict),
            _ => None,
        }
    }

    /// The object a reference names.
    pub fn as_reference(&self) -> Option<ObjectId> {
        match self.value {
            Value::Ref(id) => Some(id),
            _ => None,
        }
    }

    /// Whether it's the null object.
    pub fn is_null(&self) -> bool {
        matches!(self.value, Value::Null)
    }
}

impl Str {
    /// The string as text (ISO 32000-2, 7.9.2.2), however it's encoded;
    /// none for ciphertext.
    pub fn text(&self) -> Option<String> {
        match self {
            Str::Bytes(bytes) => Some(text::decode(bytes).text),
            Str::Text(text) => Some(text.clone()),
            Str::Encrypted(_) => None,
        }
    }

    /// Its bytes, when the reader gave them.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Str::Bytes(bytes) => Some(bytes),
            Str::Text(_) | Str::Encrypted(_) => None,
        }
    }
}

impl Dict {
    /// A dictionary with no entries.
    pub const EMPTY: Dict = Dict {
        entries: Vec::new(),
    };

    /// A dictionary with no entries.
    pub fn new() -> Dict {
        Dict::default()
    }

    /// `key`'s value: its last entry's.
    pub fn get(&self, key: &[u8]) -> Option<&Object> {
        self.entry(key).map(|entry| &entry.value)
    }

    /// `key`'s last entry.
    pub fn entry(&self, key: &[u8]) -> Option<&Entry> {
        self.entries.iter().rev().find(|entry| entry.key == key)
    }

    /// Whether it has `key`.
    pub fn has(&self, key: &[u8]) -> bool {
        self.entry(key).is_some()
    }

    /// Each key and its value, in order, a key written twice at its last.
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &Object)> {
        self.last_entries()
            .map(|entry| (entry.key.as_slice(), &entry.value))
    }

    /// Each key's last entry, in order.
    pub fn last_entries(&self) -> impl Iterator<Item = &Entry> {
        let shadowed = self.shadowed();
        (self.entries.iter().enumerate())
            .filter(move |(i, _)| !shadowed.contains(i))
            .map(|(_, entry)| entry)
    }

    /// The entries a later one with the same key overrides, in order.
    pub fn duplicates(&self) -> impl Iterator<Item = &Entry> {
        let shadowed = self.shadowed();
        (self.entries.iter().enumerate())
            .filter(move |(i, _)| shadowed.contains(i))
            .map(|(_, entry)| entry)
    }

    /// Every entry as written, duplicates and all.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Every entry as written, to change.
    pub fn entries_mut(&mut self) -> impl Iterator<Item = &mut Entry> {
        self.entries.iter_mut()
    }

    /// How many keys it has.
    pub fn len(&self) -> usize {
        self.entries.len() - self.shadowed().len()
    }

    /// Whether it has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Adds an entry after the others, whether or not `key` has one.
    pub fn push(&mut self, key: impl AsRef<[u8]>, value: impl Into<Object>) {
        self.push_entry(Entry {
            key: key.as_ref().to_vec(),
            key_at: None,
            value: value.into(),
        });
    }

    /// Adds `entry` after the others, whether or not its key has one.
    pub fn push_entry(&mut self, entry: Entry) {
        self.entries.push(entry);
    }

    /// Sets `key` to `value`: in its last entry, or a new one.
    pub fn set(&mut self, key: impl AsRef<[u8]>, value: impl Into<Object>) {
        let key = key.as_ref();
        match self.entries.iter_mut().rev().find(|entry| entry.key == key) {
            Some(entry) => entry.value = value.into(),
            None => self.push(key, value),
        }
    }

    /// Removes every entry with `key`.
    pub fn remove(&mut self, key: &[u8]) {
        self.entries.retain(|entry| entry.key != key);
    }

    /// The indices of entries a later one with the same key overrides.
    fn shadowed(&self) -> HashSet<usize> {
        let mut seen = HashSet::new();
        let mut shadowed = HashSet::new();
        for (i, entry) in self.entries.iter().enumerate().rev() {
            if !seen.insert(entry.key.as_slice()) {
                shadowed.insert(i);
            }
        }
        shadowed
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

impl From<f64> for Object {
    fn from(r: f64) -> Object {
        Object::new(Value::Real(r))
    }
}

/// A name.
impl From<&str> for Object {
    fn from(name: &str) -> Object {
        Object::new(Value::Name(name.as_bytes().to_vec()))
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
        Object::new(Value::Stream(stream))
    }
}

/// A dictionary of these keys and values, each value anything that's
/// [`Into<Object>`]: `dict! { "Type" => "Catalog", "Pages" =>
/// Object::reference(2) }`, a `&str` value being a name.
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
    use super::*;

    #[test]
    fn a_dictionary_keeps_what_was_written_and_reads_a_keys_last_entry() {
        let mut d = dict! { "A" => 1, "B" => 2, "A" => 3 };
        assert_eq!(d.get(b"A").and_then(Object::as_integer), Some(3));
        assert_eq!(d.len(), 2);
        let keys: Vec<_> = d.iter().map(|(k, v)| (k, v.as_integer())).collect();
        assert_eq!(keys, [(&b"B"[..], Some(2)), (&b"A"[..], Some(3))]);
        let duplicates: Vec<_> = d.duplicates().map(|e| e.value.as_integer()).collect();
        assert_eq!(duplicates, [Some(1)]);
        assert_eq!(d.entries().len(), 3);
        d.set("A", 4);
        assert_eq!(d.get(b"A").and_then(Object::as_integer), Some(4));
        assert_eq!(
            d.entries()[0].value.as_integer(),
            Some(1),
            "set changes the last"
        );
        d.set("C", 5);
        assert_eq!(d.len(), 3);
        assert!(!d.has(b"D"));
    }

    #[test]
    fn values_say_what_they_are() {
        assert_eq!(Object::from("Page").as_name(), Some(&b"Page"[..]));
        assert_eq!(Object::from(4.5).as_number(), Some(4.5));
        assert_eq!(Object::from(7).as_number(), Some(7.0));
        assert_eq!(Object::reference(3).as_reference(), Some((3, 0)));
        let stream = Object::stream(dict! { "Length" => 0 });
        assert!(stream.as_dict().is_some_and(|d| d.has(b"Length")));
        assert!(Object::null().is_null());
        let span = Span {
            stream: None,
            start: 3,
            end: 9,
        };
        assert_eq!(Object::from(1).at(span).at, Some(span));
        assert_eq!(Str::Text("x".to_string()).as_bytes(), None);
    }
}
