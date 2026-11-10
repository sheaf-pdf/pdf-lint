//! Resources by what they are (ISO 32000-2, 7.8.3): a resource's objects
//! and a digest of them, so that equal resources are written once and
//! named alike.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use pdf_objects::{Data, Dict, Object, ObjectId, Span, Str, Stream, Value};

/// A resource's name by what it is: a 128-bit digest of its objects. It
/// names a resource and finds it, but doesn't decide what it is: two
/// resources are one where they're equal ([`Res`]), whatever their ids.
///
/// The digest is XXH3-128, fast and the same on every build and platform,
/// so a resource is named alike wherever and whenever content naming it is
/// written. It isn't a security boundary, and needn't be: two resources
/// made to digest alike are two resources, named apart.
///
/// Each value is fed by its kind and what it holds, so that two give the
/// same bytes only when they're equal:
///
/// - **A dictionary** by its keys in order of their bytes, each key once
///   with its last value, as a lookup gives it: dictionaries equal key by
///   key digest alike however their entries are written.
/// - **A string** by its bytes, whether written literal or hex; a text
///   string a reader decoded ([`Str::Text`]) and ciphertext
///   ([`Str::Encrypted`]) each apart from bytes, and from each other.
/// - **A number** by its kind and its value: an integer as its 64 bits,
///   apart from a real, and a real as the bits of its `f64`, which is how
///   pdf-objects holds an `f32` given it (the `f64` of its shortest
///   decimal), so `0.3` digests alike made from either.
/// - **A stream** by its dictionary as held, `/Length` among its entries
///   where its builder set it ([`Stream::new`] and [`Stream::set_bytes`]
///   do), and by its data: bytes in memory as they are; data where a
///   reader found it ([`Data::At`]) by that place, an object stream's
///   number and two offsets, as reading the bytes is the caller's; data
///   neither in memory nor placed by that alone.
/// - **Names, references, booleans, null and arrays** as they are, an
///   array's elements in order.
///
/// Where a value was written (its span, a key's) isn't fed: a value's
/// digest is what it is, not where it was. A place tells apart the streams
/// of one reading, not of two, as two files laid out alike hold streams at
/// the same offsets. So a resource holds its streams' data in memory
/// ([`Res::of`] refuses any other).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResId(pub u128);

impl ResId {
    /// The digest of `objects`, a resource's objects as
    /// [`ResBody::Objects`] holds them, every byte of them read once.
    #[must_use]
    pub fn of(objects: &[Object]) -> ResId {
        let mut bytes = Vec::new();
        for object in objects {
            feed(&mut bytes, object);
        }
        ResId(twox_hash::XxHash3_128::oneshot(&bytes))
    }
}

/// A resource content draws with: a font, an ExtGState, a property list,
/// an image or a form, named by what it is ([`ResId`]).
///
/// It's made once and kept: [`Res::of`] reads every byte of its objects,
/// each stream's data in memory, and holds them shared, so a clone is
/// cheap and compares by pointer. Two resources are one where they're
/// equal: their ids equal and their objects the same by what they are, as
/// [`ResId`] feeds them, whoever made them. So a `HashMap<Res, _>`, hashed
/// by the id, holds each resource once. One already written
/// ([`Res::written`]) is that object alone, named by its number, never an
/// object equal to it.
///
/// Its fields are public. One built by hand with an id that isn't its
/// objects' digest is equal to no resource of other objects: an id names,
/// and the objects decide.
///
/// ```
/// use std::collections::HashMap;
///
/// use pdf_content_streams::Res;
/// use pdf_content_streams::pdf_objects::{ObjectId, dict};
///
/// let a = Res::dict(dict! { "Type" => "ExtGState", "ca" => 0.5 }).unwrap();
/// let b = Res::dict(dict! { "ca" => 0.5, "Type" => "ExtGState" }).unwrap();
/// assert_eq!(a, b);
/// assert_ne!(a, Res::written(ObjectId::new(7, 0)));
///
/// // Written once: a map of resources holds equal ones as one.
/// let mut written = HashMap::new();
/// written.insert(a, ObjectId::new(3, 0));
/// assert_eq!(written.get(&b), Some(&ObjectId::new(3, 0)));
/// ```
#[derive(Debug, Clone)]
pub struct Res {
    /// Its name by what it is: the digest of its objects, or of a
    /// reference to it, for one already written.
    pub id: ResId,
    /// What it is.
    pub body: ResBody,
}

/// What a resource is.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ResBody {
    /// Its objects: the resource first, then those it reaches. A reference
    /// `(i, 0)` among them is to the `i`th.
    Objects(Arc<[Object]>),
    /// An object the caller has already written, by its number in the
    /// file being written: a page made a form, say.
    Written(ObjectId),
}

/// A resource refused by [`Res::of`]: one of its objects holds a stream
/// whose data isn't in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[error("object {object} of the resource holds a stream whose data isn't in memory")]
#[non_exhaustive]
pub struct Unheld {
    /// Which of the resource's objects holds it, counted from 0.
    pub object: usize,
}

impl Res {
    /// The resource `objects` are ([`ResBody::Objects`]), named by them.
    ///
    /// # Errors
    ///
    /// [`Unheld`] where a stream in one of `objects`, at any depth, holds
    /// its data anywhere but in memory ([`Data::Bytes`]). Data where a
    /// reader found it ([`Data::At`]) can't be told from another reading's
    /// at the same place, and data that's neither can't be told at all. A
    /// caller holds such data as bytes ([`Stream::set_bytes`]), or names
    /// an object already written ([`Res::written`]).
    pub fn of(objects: impl Into<Arc<[Object]>>) -> Result<Res, Unheld> {
        let objects = objects.into();
        if let Some(object) = objects.iter().position(|o| !in_memory(o)) {
            return Err(Unheld { object });
        }
        Ok(Res::held(objects))
    }

    /// A dictionary that reaches nothing else: an ExtGState or a property
    /// list.
    ///
    /// # Errors
    ///
    /// [`Unheld`] as [`Res::of`] gives it: PDF holds a stream by reference
    /// alone (7.3.8.1), but a [`Dict`] can hold one as a value.
    pub fn dict(dict: Dict) -> Result<Res, Unheld> {
        Res::of([Object::from(dict)])
    }

    /// Object `id`, already written ([`ResBody::Written`]): named by its
    /// number, the digest of a reference to it.
    #[must_use]
    pub fn written(id: ObjectId) -> Res {
        Res {
            id: ResId::of(&[Object::from(id)]),
            body: ResBody::Written(id),
        }
    }

    /// The resource `objects` are, each stream in them holding its data in
    /// memory, as the caller knows: an ExtGState the writer builds, or a
    /// copy of what a resources dictionary binds, told by what it is.
    pub(crate) fn held(objects: impl Into<Arc<[Object]>>) -> Res {
        let objects = objects.into();
        debug_assert!(objects.iter().all(in_memory), "held in memory");
        Res {
            id: ResId::of(&objects),
            body: ResBody::Objects(objects),
        }
    }
}

/// Equal where the ids are equal and the bodies the same ([`ResBody`]):
/// an id names a resource, and its objects decide it.
impl PartialEq for Res {
    fn eq(&self, other: &Res) -> bool {
        self.id == other.id && self.body == other.body
    }
}

impl Eq for Res {}

/// Hashed by its id alone, which equal resources share.
impl Hash for Res {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// The same objects, as [`ResId`] feeds them: a clone of one by pointer,
/// else object by object, a dictionary's entries in any order, a string
/// literal or hex, where a value was written aside. Or the same object
/// written already. Never objects beside one written, even where they're
/// a reference to it.
impl PartialEq for ResBody {
    fn eq(&self, other: &ResBody) -> bool {
        match (self, other) {
            (ResBody::Objects(a), ResBody::Objects(b)) => {
                Arc::ptr_eq(a, b)
                    || (a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| same(a, b)))
            }
            (ResBody::Written(a), ResBody::Written(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for ResBody {}

/// Whether every stream in `object`, at any depth [`ResId`] reads, holds
/// its data in memory.
fn in_memory(object: &Object) -> bool {
    match object.value() {
        Value::Array(items) => items.iter().all(in_memory),
        Value::Dict(dict) => dict.iter().all(|(_, o)| in_memory(o)),
        Value::Stream(stream) => {
            matches!(stream.data, Data::Bytes(_)) && stream.dict.iter().all(|(_, o)| in_memory(o))
        }
        _ => true,
    }
}

/// `object` written into `out` so that two objects write the same bytes
/// only when they're equal, as [`ResId`] says: each part tagged with its
/// kind, and each run of bytes and each list with its length.
pub(crate) fn feed(out: &mut Vec<u8>, object: &Object) {
    match object.value() {
        Value::Null => out.push(b'n'),
        Value::Bool(b) => out.extend([b'b', u8::from(*b)]),
        Value::Integer(i) => {
            out.push(b'i');
            out.extend(i.to_le_bytes());
        }
        Value::Real(r) => {
            out.push(b'r');
            out.extend(r.to_bits().to_le_bytes());
        }
        Value::Name(name) => {
            out.push(b'/');
            run(out, name);
        }
        Value::String(Str::Bytes(bytes), _) => {
            out.push(b'(');
            run(out, bytes);
        }
        Value::String(Str::Text(text), _) => {
            out.push(b't');
            run(out, text.as_bytes());
        }
        Value::String(Str::Encrypted(bytes), _) => {
            out.push(b'e');
            run(out, bytes);
        }
        Value::Array(items) => {
            out.push(b'[');
            count(out, items.len());
            for item in items {
                feed(out, item);
            }
        }
        Value::Dict(d) => {
            out.push(b'<');
            dict(out, d);
        }
        Value::Stream(s) => stream(out, s),
        Value::Ref(ObjectId { number, generation }) => {
            out.push(b'R');
            out.extend(number.to_le_bytes());
            out.extend(generation.to_le_bytes());
        }
    }
}

/// A stream: a tag for what its data is, its dictionary, and the data.
/// The tag comes first, as data in memory is a run of bytes, whose length
/// may start with any byte, so a tag after the dictionary couldn't tell
/// data of another kind from it.
fn stream(out: &mut Vec<u8>, stream: &Stream) {
    match &stream.data {
        Data::Bytes(bytes) => {
            out.push(b's');
            dict(out, &stream.dict);
            run(out, bytes);
        }
        Data::At(at) => {
            out.push(b'@');
            dict(out, &stream.dict);
            place(out, *at);
        }
        Data::Unplaced => {
            out.push(b'u');
            dict(out, &stream.dict);
        }
        // Data of a kind this doesn't know: by that alone, never as bytes
        // it may not have.
        _ => {
            out.push(b'?');
            dict(out, &stream.dict);
        }
    }
}

/// Where a stream's data was written: the object stream it's in, if any,
/// and its two offsets.
fn place(out: &mut Vec<u8>, at: Span) {
    match at.stream {
        None => out.push(0),
        Some(ObjectId { number, generation }) => {
            out.push(1);
            out.extend(number.to_le_bytes());
            out.extend(generation.to_le_bytes());
        }
    }
    out.extend((at.start as u64).to_le_bytes());
    out.extend((at.end as u64).to_le_bytes());
}

fn count(out: &mut Vec<u8>, n: usize) {
    out.extend((n as u64).to_le_bytes());
}

fn run(out: &mut Vec<u8>, bytes: &[u8]) {
    count(out, bytes.len());
    out.extend(bytes);
}

/// A dictionary's keys in order of their bytes, each once with its last
/// value, counted first.
fn dict(out: &mut Vec<u8>, dict: &Dict) {
    let entries = sorted(dict);
    count(out, entries.len());
    for (key, value) in entries {
        run(out, key);
        feed(out, value);
    }
}

/// A dictionary's entries in order of their keys' bytes, each key once
/// with its last value.
fn sorted(dict: &Dict) -> Vec<(&[u8], &Object)> {
    let mut entries: Vec<_> = dict.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
}

/// Whether `a` and `b` are the same by what they are: whether [`feed`]
/// writes them the same bytes, told without writing them.
pub(crate) fn same(a: &Object, b: &Object) -> bool {
    match (a.value(), b.value()) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Integer(a), Value::Integer(b)) => a == b,
        (Value::Real(a), Value::Real(b)) => a.to_bits() == b.to_bits(),
        (Value::Name(a), Value::Name(b)) => a == b,
        (Value::String(a, _), Value::String(b, _)) => match (a, b) {
            (Str::Bytes(a), Str::Bytes(b)) | (Str::Encrypted(a), Str::Encrypted(b)) => a == b,
            (Str::Text(a), Str::Text(b)) => a == b,
            _ => false,
        },
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
        }
        (Value::Dict(a), Value::Dict(b)) => same_dict(a, b),
        (Value::Stream(a), Value::Stream(b)) => {
            same_data(&a.data, &b.data) && same_dict(&a.dict, &b.dict)
        }
        (Value::Ref(a), Value::Ref(b)) => a == b,
        _ => false,
    }
}

/// Whether two dictionaries are the same, key by key, as [`dict`] feeds
/// them. Compared in the order they're written while their keys keep in
/// step, as most compared are written alike, and sorted where they part.
fn same_dict(a: &Dict, b: &Dict) -> bool {
    let (mut x, mut y) = (a.iter(), b.iter());
    loop {
        match (x.next(), y.next()) {
            (None, None) => return true,
            (Some((k, v)), Some((l, w))) if k == l => {
                if !same(v, w) {
                    return false;
                }
            }
            _ => break,
        }
    }
    let (a, b) = (sorted(a), sorted(b));
    a.len() == b.len()
        && a.iter()
            .zip(&b)
            .all(|((k, v), (l, w))| k == l && same(v, w))
}

/// Whether two streams' data is the same as [`stream`] feeds it: bytes in
/// memory by their bytes, data placed by its place, and data neither, or
/// of a kind this doesn't know, by its kind alone.
fn same_data(a: &Data, b: &Data) -> bool {
    match (a, b) {
        (Data::Bytes(a), Data::Bytes(b)) => a == b,
        (Data::At(a), Data::At(b)) => (a.stream, a.start, a.end) == (b.stream, b.start, b.end),
        (Data::Unplaced, Data::Unplaced) => true,
        (Data::Bytes(_) | Data::At(_) | Data::Unplaced, _) => false,
        (_, b) => !matches!(b, Data::Bytes(_) | Data::At(_) | Data::Unplaced),
    }
}

#[cfg(test)]
mod tests;
