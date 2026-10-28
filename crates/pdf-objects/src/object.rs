//! The objects a checked file holds, as a reader read them: owned, each
//! value with where it was written when the reader knows and no edit has
//! changed it since, and each dictionary as written, in order and with any
//! key written twice.

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
///
/// Its value is read through [`Object::value`] and changed through
/// [`Object::value_mut`] or an `as_*_mut` accessor, each an edit, which
/// drops its span: reaching a value inside another goes through each
/// value that holds it, so their spans go too, and a value no edit
/// reached keeps its own.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    value: Value,
    /// Where it was written, when the reader knows and no edit has changed
    /// it since: a reader of the file's bytes always does, and one through
    /// another tool's JSON, as qpdf's, may not. A value changed in place
    /// has none, nor has anything that holds it, as they may no longer be
    /// what those bytes say, so a writer writes them anew; one no edit
    /// touched keeps its own. A real's spelling, `1.50` or `.5`, is in the
    /// bytes it names.
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
    /// A real number; its spelling as written is in the bytes its span
    /// names.
    Real(f64),
    /// A name's bytes, its `#xx` escapes decoded.
    Name(Vec<u8>),
    /// A string, and its form when it was written or chosen: none for one
    /// built without choosing, or read by a reader that doesn't say, which
    /// a writer writes as a literal string.
    String(Str, Option<Form>),
    /// An array: its elements, in order.
    Array(Vec<Object>),
    /// A dictionary.
    Dict(Dict),
    /// A stream: its dictionary, and its data.
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

/// How a string was written (ISO 32000-2, 7.3.4.2 and 7.3.4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Form {
    /// A literal string, `(...)`.
    Literal,
    /// A hex string, `<...>`.
    Hex,
}

/// A dictionary: its entries as written, in order. Lookups take a key's
/// last entry, as readers do; [`Dict::duplicates`] gives the others. A
/// lookup allocates nothing for a dictionary of up to 32 entries.
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

/// A stream: its dictionary, and its data. Outside this crate it's made
/// by [`Stream::new`], from its bytes, or [`Object::stream`], or from
/// `Stream::default()` with its fields set, as more may come.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct Stream {
    /// Its dictionary.
    pub dict: Dict,
    /// Its data: where it was written, or its bytes. An edit of the
    /// dictionary leaves it as it is, as the data didn't change.
    pub data: Data,
}

/// A stream's data.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Data {
    /// Where it was written, as written: encoded by the stream's filters,
    /// and encrypted in an encrypted file. A reader's.
    At(Span),
    /// Its bytes, in memory: encoded by the stream's filters, if it has
    /// any, and written as they are
    /// ([`serialize`](crate::serialize)).
    Bytes(Vec<u8>),
    /// Neither: a reader that doesn't say where, as one through another
    /// tool's JSON may not, or a stream built without its data.
    #[default]
    Unplaced,
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

    /// A string of `text`'s bytes, its form not chosen.
    pub fn string(text: &str) -> Object {
        Object::new(Value::String(Str::Bytes(text.as_bytes().to_vec()), None))
    }

    /// A string of `bytes`, its form not chosen.
    pub fn bytes(bytes: impl Into<Vec<u8>>) -> Object {
        Object::new(Value::String(Str::Bytes(bytes.into()), None))
    }

    /// A string of `bytes`, written as a hex string.
    pub fn hex(bytes: impl Into<Vec<u8>>) -> Object {
        Object::new(Value::String(Str::Bytes(bytes.into()), Some(Form::Hex)))
    }

    /// A stream of `dict`, whose data isn't placed.
    pub fn stream(dict: Dict) -> Object {
        Object::new(Value::Stream(Stream {
            dict,
            data: Data::Unplaced,
        }))
    }

    /// The same value, written at `at`.
    pub fn at(self, at: Span) -> Object {
        Object {
            at: Some(at),
            ..self
        }
    }

    /// Its value.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Its value, without where it was written.
    pub fn into_value(self) -> Value {
        self.value
    }

    /// Its value, to change: an edit, so its span goes, as what it holds
    /// may no longer be what the bytes it names stand for.
    pub fn value_mut(&mut self) -> &mut Value {
        self.at = None;
        &mut self.value
    }

    /// Its value, to change into what the same bytes stand for, as a
    /// reader does when it decrypts a string it read: its span is kept.
    /// Any other change is an edit, through [`Object::value_mut`].
    pub fn as_written_mut(&mut self) -> &mut Value {
        &mut self.value
    }

    /// A boolean's value.
    pub fn as_bool(&self) -> Option<bool> {
        match self.value {
            Value::Bool(b) => Some(b),
            _ => None,
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

    /// A string's bytes, when the reader gave them ([`Str::as_bytes`]).
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match &self.value {
            Value::String(s, _) => s.as_bytes(),
            _ => None,
        }
    }

    /// An array's elements.
    pub fn as_array(&self) -> Option<&[Object]> {
        match &self.value {
            Value::Array(array) => Some(array),
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

    /// A stream.
    pub fn as_stream(&self) -> Option<&Stream> {
        match &self.value {
            Value::Stream(stream) => Some(stream),
            _ => None,
        }
    }

    /// An array's elements, to change: an edit, as through
    /// [`Object::value_mut`], when it's an array.
    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Object>> {
        match &mut self.value {
            Value::Array(array) => {
                self.at = None;
                Some(array)
            }
            _ => None,
        }
    }

    /// A dictionary, or a stream's dictionary, to change: an edit, as
    /// through [`Object::value_mut`], when it's either. A stream's data
    /// keeps where it was written.
    pub fn as_dict_mut(&mut self) -> Option<&mut Dict> {
        match &mut self.value {
            Value::Dict(dict) | Value::Stream(Stream { dict, .. }) => {
                self.at = None;
                Some(dict)
            }
            _ => None,
        }
    }

    /// A stream, to change: an edit, as through [`Object::value_mut`],
    /// when it's a stream.
    pub fn as_stream_mut(&mut self) -> Option<&mut Stream> {
        match &mut self.value {
            Value::Stream(stream) => {
                self.at = None;
                Some(stream)
            }
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

impl Data {
    /// Where it was written, when it's [`Data::At`].
    pub fn at(&self) -> Option<Span> {
        match self {
            Data::At(at) => Some(*at),
            _ => None,
        }
    }

    /// Its bytes, when it's [`Data::Bytes`].
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Data::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }
}

impl Stream {
    /// A stream of `dict` and `bytes`, its `/Length` set to their length.
    pub fn new(dict: Dict, bytes: Vec<u8>) -> Stream {
        let mut stream = Stream {
            dict,
            data: Data::Unplaced,
        };
        stream.set_bytes(bytes);
        stream
    }

    /// Its data replaced by `bytes`, and its `/Length` set to their length,
    /// in its last entry or a new one.
    pub fn set_bytes(&mut self, bytes: Vec<u8>) {
        self.dict.set("Length", bytes.len() as i64);
        self.data = Data::Bytes(bytes);
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

    /// `key`'s value, its last entry's, to change.
    pub fn get_mut(&mut self, key: &[u8]) -> Option<&mut Object> {
        (self.entries.iter_mut().rev())
            .find(|entry| entry.key == key)
            .map(|entry| &mut entry.value)
    }

    /// `key`'s last entry.
    pub fn entry(&self, key: &[u8]) -> Option<&Entry> {
        self.entries.iter().rev().find(|entry| entry.key == key)
    }

    /// Whether it has `key`.
    pub fn has(&self, key: &[u8]) -> bool {
        self.entry(key).is_some()
    }

    /// Its `/Type`, when that's a name.
    pub fn type_name(&self) -> Option<&[u8]> {
        self.get(b"Type").and_then(Object::as_name)
    }

    /// Whether its `/Type` is the name `name`.
    pub fn has_type(&self, name: &[u8]) -> bool {
        self.type_name() == Some(name)
    }

    /// Each key and its value, in order, a key written twice at its last.
    pub fn iter(&self) -> Iter<'_> {
        Iter {
            entries: self.entries.iter().enumerate(),
            shadowed: Shadowed::of(&self.entries),
        }
    }

    /// Each key and its value, as [`Dict::iter`] gives them, the values to
    /// change.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&[u8], &mut Object)> {
        let shadowed = Shadowed::of(&self.entries);
        (self.entries.iter_mut().enumerate())
            .filter(move |(i, _)| !shadowed.contains(*i))
            .map(|(_, entry)| (entry.key.as_slice(), &mut entry.value))
    }

    /// Each key's last entry, in order.
    pub fn last_entries(&self) -> impl Iterator<Item = &Entry> {
        let shadowed = Shadowed::of(&self.entries);
        (self.entries.iter().enumerate())
            .filter(move |(i, _)| !shadowed.contains(*i))
            .map(|(_, entry)| entry)
    }

    /// The entries a later one with the same key overrides, in order.
    pub fn duplicates(&self) -> impl Iterator<Item = &Entry> {
        let shadowed = Shadowed::of(&self.entries);
        (self.entries.iter().enumerate())
            .filter(move |(i, _)| shadowed.contains(*i))
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
        self.entries.len() - Shadowed::of(&self.entries).len()
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

    /// Removes every entry with `key`, the others kept in order, and gives
    /// the value of the last it removed, `key`'s value as [`Dict::get`]
    /// gave it.
    pub fn remove(&mut self, key: &[u8]) -> Option<Object> {
        let last = self.entries.iter().rposition(|entry| entry.key == key)?;
        let value = self.entries.remove(last).value;
        self.entries.retain(|entry| entry.key != key);
        Some(value)
    }

    /// Removes the entries a later one with the same key overrides
    /// ([`Dict::duplicates`]), the others kept in order: each key's last
    /// is left, as ISO 32000-2 has a dictionary hold one entry for a key
    /// (7.3.7).
    pub fn remove_duplicates(&mut self) {
        let shadowed = Shadowed::of(&self.entries);
        let mut index = 0;
        self.entries.retain(|_| {
            let kept = !shadowed.contains(index);
            index += 1;
            kept
        });
    }
}

/// How many entries a dictionary may have for its lookups to find the ones
/// a later entry overrides without allocating.
const FEW: usize = 32;

/// The indices of a dictionary's entries a later one with the same key
/// overrides: for at most [`FEW`] entries, found by comparing each key with
/// the later ones, in a mask, so that a lookup allocates nothing; past that,
/// in a set.
#[derive(Debug, Clone)]
enum Shadowed {
    Few(u32),
    Many(HashSet<usize>),
}

impl Shadowed {
    fn of(entries: &[Entry]) -> Shadowed {
        if entries.len() <= FEW {
            let mut mask = 0;
            for (i, entry) in entries.iter().enumerate() {
                if entries[i + 1..].iter().any(|later| later.key == entry.key) {
                    mask |= 1 << i;
                }
            }
            return Shadowed::Few(mask);
        }
        let mut seen = HashSet::new();
        let mut shadowed = HashSet::new();
        for (i, entry) in entries.iter().enumerate().rev() {
            if !seen.insert(entry.key.as_slice()) {
                shadowed.insert(i);
            }
        }
        Shadowed::Many(shadowed)
    }

    fn contains(&self, i: usize) -> bool {
        match self {
            Shadowed::Few(mask) => i < FEW && (mask & (1 << i)) != 0,
            Shadowed::Many(set) => set.contains(&i),
        }
    }

    fn len(&self) -> usize {
        match self {
            Shadowed::Few(mask) => mask.count_ones() as usize,
            Shadowed::Many(set) => set.len(),
        }
    }
}

/// A dictionary's keys and their values, in order, a key written twice at
/// its last entry: what [`Dict::iter`] gives, and a `for` over a `&Dict`.
#[derive(Debug, Clone)]
pub struct Iter<'a> {
    entries: std::iter::Enumerate<std::slice::Iter<'a, Entry>>,
    shadowed: Shadowed,
}

impl<'a> Iterator for Iter<'a> {
    type Item = (&'a [u8], &'a Object);

    fn next(&mut self) -> Option<Self::Item> {
        let shadowed = &self.shadowed;
        (self.entries.by_ref())
            .find(|(i, _)| !shadowed.contains(*i))
            .map(|(_, entry)| (entry.key.as_slice(), &entry.value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, self.entries.size_hint().1)
    }
}

impl DoubleEndedIterator for Iter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let shadowed = &self.shadowed;
        (self.entries.by_ref())
            .rfind(|(i, _)| !shadowed.contains(*i))
            .map(|(_, entry)| (entry.key.as_slice(), &entry.value))
    }
}

impl<'a> IntoIterator for &'a Dict {
    type Item = (&'a [u8], &'a Object);
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

/// A dictionary of these keys and values, each value anything that's
/// [`Into<Object>`], each pushed: a key given twice is kept twice, as
/// [`dict!`](crate::dict) keeps it.
impl<K: AsRef<[u8]>, V: Into<Object>> FromIterator<(K, V)> for Dict {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Dict {
        let mut dict = Dict::new();
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

/// The f64 nearest the f32's shortest decimal, which is what an f32 is
/// written as: `0.1f32` is 0.1, where `f64::from` would give
/// 0.10000000149011612. Every f32's `Display` (`inf` and `NaN` too) parses
/// as an f64. Finding the shortest decimal is most of the cost, so a whole
/// number below 2^24, which is its own shortest decimal, skips it.
impl From<f32> for Object {
    fn from(r: f32) -> Object {
        if r.fract() == 0.0 && r.abs() < 16_777_216.0 {
            return Object::new(Value::Real(r.into()));
        }
        let r = r
            .to_string()
            .parse()
            .expect("an f32's Display parses as an f64");
        Object::new(Value::Real(r))
    }
}

/// A name.
impl From<&str> for Object {
    fn from(name: &str) -> Object {
        Object::new(Value::Name(name.as_bytes().to_vec()))
    }
}

/// A name.
impl From<String> for Object {
    fn from(name: String) -> Object {
        Object::new(Value::Name(name.into_bytes()))
    }
}

/// A reference.
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

    /// A dictionary without its overridden entries keeps each key's last,
    /// in order, and is written with each key once; with few entries or
    /// many.
    #[test]
    fn remove_duplicates_keeps_each_keys_last_entry() {
        let mut d = dict! { "A" => 1, "B" => 2, "A" => 3, "C" => 4, "B" => 5 };
        d.remove_duplicates();
        let entries: Vec<_> = (d.entries().iter())
            .map(|e| (e.key.as_slice(), e.value.as_integer()))
            .collect();
        let last = [(&b"A"[..], Some(3)), (b"C", Some(4)), (b"B", Some(5))];
        assert_eq!(entries, last);
        let mut out = Vec::new();
        crate::serialize::write(&mut out, &Object::from(d)).unwrap();
        assert_eq!(out, b"<</A 3/C 4/B 5>>");
        let mut many: Dict = (0..40).map(|i| (format!("K{}", i % 20), i)).collect();
        many.remove_duplicates();
        assert_eq!((many.entries().len(), many.len()), (20, 20));
        assert_eq!(many.duplicates().count(), 0);
        assert_eq!(many.get(b"K0").and_then(Object::as_integer), Some(20));
        assert_eq!(many.entries()[0].key, b"K0");
    }

    /// The objects, and what reads and writes them, can be shared across
    /// threads.
    #[test]
    fn the_types_are_send_and_sync() {
        fn shared<T: Send + Sync>() {}
        shared::<Object>();
        shared::<Value>();
        shared::<Str>();
        shared::<Dict>();
        shared::<Entry>();
        shared::<Iter<'_>>();
        shared::<Stream>();
        shared::<Data>();
        shared::<Span>();
        shared::<crate::parse::Limits>();
        shared::<crate::parse::Error>();
        shared::<crate::serialize::Error>();
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
        let hex = Value::String(Str::Bytes(b"x".to_vec()), Some(Form::Hex));
        assert_eq!(Object::hex(*b"x").value, hex);
        let unchosen = Value::String(Str::Bytes(b"x".to_vec()), None);
        assert_eq!(Object::bytes(*b"x").value, unchosen);
        assert_eq!(Object::string("x").value, unchosen);
    }

    /// `text`'s value, as the parser reads it: each value placed.
    fn read(text: &[u8]) -> Object {
        crate::parse::Parser::at(text, 0).value(&mut ()).unwrap()
    }

    /// The bytes of `text` that `o`'s span names, when it has one.
    fn written<'a>(text: &'a [u8], o: &Object) -> Option<&'a [u8]> {
        o.at.map(|at| &text[at.start..at.end])
    }

    /// Stream 4 0 of `file`, read by the parser, its data by `/Length`.
    fn stream(file: &[u8]) -> Object {
        use crate::parse::{Body, Parser};
        let mut parser = Parser::at(file, 0);
        let id = parser.header().unwrap();
        let Ok(Body::Stream(head)) = parser.body(id, &mut ()) else {
            panic!("a stream");
        };
        let length = head.dict().get(b"Length").and_then(Object::as_integer);
        let data = head.declared(length.unwrap() as usize).unwrap();
        parser.stream(head, data, &mut ()).unwrap()
    }

    #[test]
    fn an_edit_drops_the_span_of_what_it_changed_and_of_what_holds_it() {
        let text = b"<< /A [1 [2 3]] /B 4 >>";
        let mut o = read(text);
        let a = o.as_dict_mut().unwrap().get_mut(b"A").unwrap();
        let inner = &mut a.as_array_mut().unwrap()[1];
        *inner.as_array_mut().unwrap()[0].value_mut() = Value::Integer(5);

        let d = o.as_dict().unwrap();
        let a = d.get(b"A").unwrap();
        let [one, inner] = a.as_array().unwrap() else {
            panic!("two elements");
        };
        let [five, three] = inner.as_array().unwrap() else {
            panic!("two elements");
        };
        // What changed, and each value that holds it.
        assert_eq!(five.as_integer(), Some(5));
        for changed in [&o, a, inner, five] {
            assert_eq!(changed.at, None, "{changed:?}");
        }
        // Its siblings, at each depth, and the keys.
        assert_eq!(written(text, three), Some(&b"3"[..]));
        assert_eq!(written(text, one), Some(&b"1"[..]));
        assert_eq!(written(text, d.get(b"B").unwrap()), Some(&b"4"[..]));
        assert!(d.entries().iter().all(|e| e.key_at.is_some()));
    }

    #[test]
    fn each_call_that_changes_a_value_is_an_edit() {
        let mut o = read(b"[1]");
        o.value_mut();
        assert_eq!(o.at, None, "value_mut");
        let mut o = read(b"[1]");
        assert!(o.as_array_mut().is_some());
        assert_eq!(o.at, None, "as_array_mut");
        let mut o = read(b"<< /A 1 >>");
        assert!(o.as_dict_mut().is_some());
        assert_eq!(o.at, None, "as_dict_mut");
        let mut o = stream(b"4 0 obj << /Length 1 >> stream\nx\nendstream endobj");
        assert!(o.as_stream_mut().is_some());
        assert_eq!(o.at, None, "as_stream_mut");
        let mut o = stream(b"4 0 obj << /Length 1 >> stream\nx\nendstream endobj");
        assert!(o.as_dict_mut().is_some());
        assert_eq!(o.at, None, "as_dict_mut of a stream");
        // One that reaches nothing changes nothing.
        let mut o = read(b"7");
        assert!(o.as_array_mut().is_none());
        assert!(o.as_dict_mut().is_none());
        assert!(o.as_stream_mut().is_none());
        assert_eq!(written(b"7", &o), Some(&b"7"[..]));
    }

    #[test]
    fn a_dictionarys_changes_leave_its_other_entries_placed() {
        let text = b"<< /A 1 /B 2 /C 3 >>";
        let mut o = read(text);
        let d = o.as_dict_mut().unwrap();
        d.set("B", 5);
        d.push("D", 6);
        d.remove(b"C");
        assert_eq!(o.at, None);
        let d = o.as_dict().unwrap();
        assert_eq!(written(text, d.get(b"A").unwrap()), Some(&b"1"[..]));
        assert_eq!(d.get(b"B").unwrap().at, None);
        assert_eq!(d.get(b"D").unwrap().at, None);
        let key = d.entries()[1].key_at.expect("a key placed");
        assert_eq!(
            &text[key.start..key.end],
            b"/B",
            "an entry set keeps its key's"
        );
    }

    #[test]
    fn as_written_mut_keeps_the_span() {
        let text = b"<< /S (secret) >>";
        let mut o = read(text);
        let Value::Dict(d) = o.as_written_mut() else {
            panic!("a dictionary");
        };
        let s = d.get_mut(b"S").unwrap();
        let Value::String(s, _) = s.as_written_mut() else {
            panic!("a string");
        };
        *s = Str::Bytes(b"plain".to_vec());
        assert_eq!(written(text, &o), Some(&text[..]));
        let s = o.as_dict().unwrap().get(b"S").unwrap();
        assert_eq!(s.as_bytes(), Some(&b"plain"[..]));
        assert_eq!(written(text, s), Some(&b"(secret)"[..]));
    }

    #[test]
    fn a_streams_dictionary_edited_keeps_its_datas_span() {
        let file = b"4 0 obj\n<< /Length 5 >>\nstream\nhello\nendstream\nendobj\n";
        let mut o = stream(file);
        let data = o.as_stream().unwrap().data.at().unwrap();
        assert_eq!(&file[data.start..data.end], b"hello");
        o.as_dict_mut().unwrap().set("Filter", "FlateDecode");
        assert_eq!(o.at, None);
        assert_eq!(o.as_stream().unwrap().data, Data::At(data));
        // Its data replaced is its bytes, and /Length with them.
        o.as_stream_mut().unwrap().set_bytes(b"bye".to_vec());
        let s = o.as_stream().unwrap();
        assert_eq!(s.data.bytes(), Some(&b"bye"[..]));
        assert_eq!(s.data.at(), None);
        assert_eq!(s.dict.get(b"Length").and_then(Object::as_integer), Some(3));
    }

    #[test]
    fn remove_keeps_the_order_and_gives_the_last() {
        let mut d = dict! { "A" => 1, "B" => 2, "A" => 3, "C" => 4 };
        assert_eq!(d.remove(b"A").and_then(|o| o.as_integer()), Some(3));
        let keys: Vec<&[u8]> = d.entries().iter().map(|e| &e.key[..]).collect();
        assert_eq!(keys, [&b"B"[..], b"C"]);
        assert_eq!(d.remove(b"A"), None);
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn get_mut_and_iter_mut_take_a_keys_last_entry() {
        let mut d = dict! { "A" => 1, "B" => 2, "A" => 3 };
        *d.get_mut(b"A").unwrap() = Object::from(5);
        assert!(d.get_mut(b"C").is_none());
        let mut keys = Vec::new();
        for (key, value) in d.iter_mut() {
            keys.push(key.to_vec());
            *value = Object::from(value.as_integer().unwrap() * 10);
        }
        assert_eq!(keys, [b"B".to_vec(), b"A".to_vec()]);
        let values: Vec<_> = d.entries().iter().map(|e| e.value.as_integer()).collect();
        assert_eq!(values, [Some(1), Some(20), Some(50)]);
    }

    #[test]
    fn set_on_a_key_written_twice_changes_its_last_entry() {
        let mut d = dict! { "A" => 1, "A" => 2 };
        d.set("A", 3);
        let values: Vec<_> = d.entries().iter().map(|e| e.value.as_integer()).collect();
        assert_eq!(values, [Some(1), Some(3)]);
        assert_eq!(d.get(b"A").and_then(Object::as_integer), Some(3));
    }

    /// The lookups that find the entries a later one overrides, on a
    /// dictionary of 32 entries, which they do without allocating, and of
    /// 33, which they do through a set: each with a key written twice at
    /// its start and one at its end.
    #[test]
    fn lookups_read_a_keys_last_entry_with_few_entries_or_many() {
        for n in [32, 33] {
            let mut d: Dict = (0..n - 3).map(|i| (format!("K{i}"), i)).collect();
            d.push("K0", -1);
            d.push("X", 1);
            d.push("X", 2);
            assert_eq!(d.entries().len(), n as usize);
            assert_eq!(d.len(), n as usize - 2);
            let mut keys: Vec<Vec<u8>> = (1..n - 3).map(|i| format!("K{i}").into_bytes()).collect();
            keys.extend([b"K0".to_vec(), b"X".to_vec()]);
            let read: Vec<Vec<u8>> = d.iter().map(|(k, _)| k.to_vec()).collect();
            assert_eq!(read, keys, "{n}");
            let back: Vec<Vec<u8>> = d.iter().rev().map(|(k, _)| k.to_vec()).collect();
            assert_eq!(back, keys.iter().rev().cloned().collect::<Vec<_>>(), "{n}");
            let last: Vec<Vec<u8>> = d.last_entries().map(|e| e.key.clone()).collect();
            assert_eq!(last, keys, "{n}");
            let twice: Vec<_> = d.duplicates().map(|e| e.value.as_integer()).collect();
            assert_eq!(twice, [Some(0), Some(1)], "{n}");
            let values: Vec<_> = (&d).into_iter().map(|(_, v)| v.as_integer()).collect();
            assert_eq!(values[values.len() - 2..], [Some(-1), Some(2)], "{n}");
            assert_eq!(d.iter_mut().count(), n as usize - 2, "{n}");
        }
    }

    #[test]
    fn a_reals_spelling_is_the_bytes_its_span_names() {
        let text = b"[1.50 +17. .5 -0.0]";
        let o = read(text);
        let reals = o.as_array().unwrap();
        let spelled = [
            (1.5, &b"1.50"[..]),
            (17.0, b"+17."),
            (0.5, b".5"),
            (-0.0, b"-0.0"),
        ];
        for (o, (value, spelling)) in reals.iter().zip(spelled) {
            assert_eq!(*o.value(), Value::Real(value));
            assert_eq!(written(text, o), Some(spelling));
        }
    }

    #[test]
    fn values_come_from_their_rust_types() {
        let integer = |o: Object| o.as_integer();
        assert_eq!(integer(Object::from(u8::MAX)), Some(255));
        assert_eq!(integer(Object::from(u16::MAX)), Some(65_535));
        assert_eq!(integer(Object::from(u32::MAX)), Some(4_294_967_295));
        assert_eq!(integer(Object::from(i8::MIN)), Some(-128));
        assert_eq!(integer(Object::from(i16::MIN)), Some(-32_768));
        assert_eq!(integer(Object::from(i32::MIN)), Some(-2_147_483_648));
        assert_eq!(integer(Object::from(i64::MIN)), Some(i64::MIN));
        assert_eq!(Object::from((3, 1)).as_reference(), Some((3, 1)));
        assert_eq!(Object::from(true).as_bool(), Some(true));
        assert_eq!(Object::from(0.25).as_number(), Some(0.25));
        // An f32 as its shortest decimal: 0.1, not 0.10000000149011612.
        assert_eq!(Object::from(0.1f32).value, Value::Real(0.1));
        assert_eq!(Object::from(-3.0f32).value, Value::Real(-3.0));
        assert_eq!(Object::from(1e30f32).value, Value::Real(1e30));
        assert_eq!(Object::from(16_777_216f32).value, Value::Real(16_777_216.0));
        assert_eq!(
            Object::from(f32::INFINITY).value,
            Value::Real(f64::INFINITY)
        );
        assert!(Object::from(f32::NAN).as_number().is_some_and(f64::is_nan));
        // A string, borrowed or owned, is a name.
        assert_eq!(Object::from("Page").as_name(), Some(&b"Page"[..]));
        assert_eq!(
            Object::from("Page".to_string()).as_name(),
            Some(&b"Page"[..])
        );
        let array = Object::from(vec![Object::from(1)]);
        assert_eq!(array.as_array().map(<[Object]>::len), Some(1));
        assert!(Object::from(Dict::new()).as_dict().is_some());
        assert!(Object::from(Stream::default()).as_stream().is_some());
        assert_eq!(Object::from(Value::Null), Object::null());
    }

    #[test]
    fn objects_are_built_and_read_by_kind() {
        // Each kind's variant, and its accessor.
        let null = Object::new(Value::Null);
        assert!(null.is_null());
        assert_eq!(Object::new(Value::Bool(false)).as_bool(), Some(false));
        assert_eq!(Object::new(Value::Integer(-2)).as_integer(), Some(-2));
        assert_eq!(Object::new(Value::Real(2.5)).as_number(), Some(2.5));
        assert_eq!(Object::new(Value::Integer(2)).as_number(), Some(2.0));
        assert_eq!(
            Object::new(Value::Name(b"N".to_vec())).as_name(),
            Some(&b"N"[..])
        );
        assert_eq!(Object::new(Value::Ref((5, 0))).as_reference(), Some((5, 0)));
        let array = Object::new(Value::Array(vec![Object::from(1), Object::from(2)]));
        assert_eq!(array.as_array().map(<[Object]>::len), Some(2));
        let dict = Object::new(Value::Dict(dict! { "Type" => "Page" }));
        assert!(dict.as_dict().is_some_and(|d| d.has_type(b"Page")));
        let stream = Object::new(Value::Stream(Stream::new(Dict::new(), b"x".to_vec())));
        assert_eq!(
            stream.as_stream().and_then(|s| s.data.bytes()),
            Some(&b"x"[..])
        );
        // A stream's dictionary is a dictionary to `as_dict`.
        assert!(stream.as_dict().is_some_and(|d| d.has(b"Length")));
        // A string's bytes, and each form.
        let literal = Object::bytes(*b"(a)");
        assert_eq!(literal.as_bytes(), Some(&b"(a)"[..]));
        let forms = [Object::bytes(*b"a"), Object::hex(*b"a")].map(|o| match o.into_value() {
            Value::String(_, form) => form,
            _ => panic!("a string"),
        });
        assert_eq!(forms, [None, Some(Form::Hex)]);
        let text = Object::new(Value::String(
            Str::Text("a".to_string()),
            Some(Form::Literal),
        ));
        assert_eq!(text.as_bytes(), None);
        // Each accessor refuses the other kinds.
        let one = Object::from(1);
        assert_eq!(one.as_bool(), None);
        assert_eq!(one.as_name(), None);
        assert_eq!(one.as_bytes(), None);
        assert_eq!(one.as_array(), None);
        assert_eq!(one.as_dict(), None);
        assert_eq!(one.as_stream(), None);
        assert_eq!(one.as_reference(), None);
        assert_eq!(Object::from("N").as_number(), None);
        assert_eq!(Object::from(1.5).as_integer(), None);
        // The changing accessors, each its kind's.
        let mut array = Object::from(vec![]);
        array.as_array_mut().unwrap().push(Object::from(3));
        assert_eq!(array.as_array().map(<[Object]>::len), Some(1));
        let mut stream = Object::stream(Dict::new());
        stream.as_stream_mut().unwrap().set_bytes(Vec::new());
        stream.as_dict_mut().unwrap().set("Type", "XObject");
        let dict = stream.as_dict().unwrap();
        assert_eq!(dict.get(b"Length").and_then(Object::as_integer), Some(0));
        assert_eq!(dict.type_name(), Some(&b"XObject"[..]));
        // Values compare by value and place; a value says what it is.
        let id: ObjectId = (1, 0);
        assert_eq!(Object::from(id), Object::reference(1));
        assert_ne!(Object::from(1), Object::from(1).at(Span::new(None, 0, 1)));
        assert_eq!(format!("{:?}", Value::Integer(1)), "Integer(1)");
        assert_eq!(Object::from(7).into_value(), Value::Integer(7));
    }

    #[test]
    fn dictionaries_are_built_read_and_changed() {
        let mut d = Dict::new();
        assert!(d.is_empty() && d == Dict::default());
        assert_eq!(d.len(), 0);
        d.set("Type", "Annot");
        d.set("Subtype", "Text");
        assert!(d.has(b"Type") && !d.has(b"Rect"));
        assert!(d.has_type(b"Annot") && !d.has_type(b"Page"));
        assert_eq!(d.type_name(), Some(&b"Annot"[..]));
        assert_eq!(
            d.get(b"Subtype").and_then(Object::as_name),
            Some(&b"Text"[..])
        );
        assert_eq!(d.len(), 2);
        // A /Type that isn't a name names no type.
        assert_eq!(dict! { "Type" => 1 }.type_name(), None);
        // `dict!` and `collect` push, so a key given twice is kept twice.
        let pushed: Dict = [("A", 1), ("A", 2)].into_iter().collect();
        assert_eq!(pushed, dict! { "A" => 1, "A" => 2 });
        assert_eq!(pushed.entries().len(), 2);
        // A `for` over a dictionary, as `iter`, and equality in order.
        let mut keys = Vec::new();
        for (key, _) in &d {
            keys.push(key);
        }
        assert_eq!(keys, [&b"Type"[..], b"Subtype"]);
        assert_ne!(d, dict! { "Subtype" => "Text", "Type" => "Annot" });
        assert!(d.iter().any(|(_, v)| v.as_name() == Some(b"Text")));
        // Changed in place, and removed.
        if let Some(subtype) = d.get_mut(b"Subtype") {
            *subtype = Object::from("FreeText");
        }
        for (_, value) in d.iter_mut() {
            assert!(value.as_name().is_some());
        }
        assert_eq!(
            d.remove(b"Type")
                .and_then(|t| t.as_name().map(<[u8]>::to_vec)),
            Some(b"Annot".to_vec())
        );
        assert_eq!(d, dict! { "Subtype" => "FreeText" });
        assert_eq!(format!("{:?}", Dict::new()), "Dict { entries: [] }");
    }

    #[test]
    fn streams_are_built_with_their_bytes() {
        // Its /Length set to its bytes' length, in place of the one given.
        let s = Stream::new(
            dict! { "Length" => 99, "Filter" => "FlateDecode" },
            b"abc".to_vec(),
        );
        assert_eq!(s.data, Data::Bytes(b"abc".to_vec()));
        assert_eq!(s.dict.get(b"Length").and_then(Object::as_integer), Some(3));
        assert_eq!(s.dict.entries().len(), 2);
        let mut s = s;
        // Its data replaced; dropping /Filter for plain bytes is the
        // caller's.
        s.set_bytes(b"plain".to_vec());
        s.dict.remove(b"Filter");
        assert_eq!(s.data.bytes(), Some(&b"plain"[..]));
        assert_eq!(s.dict, dict! { "Length" => 5 });
        // Built without its data, or as a reader placed it.
        assert_eq!(Stream::default().data, Data::Unplaced);
        assert_eq!(Data::default().bytes(), None);
        let at = Span::new(None, 10, 15);
        assert_eq!(Data::At(at).at(), Some(at));
        assert_eq!(Data::At(at).bytes(), None);
        assert_eq!(Data::Bytes(Vec::new()).at(), None);
    }
}
