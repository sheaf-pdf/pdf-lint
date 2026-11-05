//! Resources by what they are (ISO 32000-2, 7.8.3): a resource's objects
//! and a digest of them, so that equal resources are written once and
//! named alike.

use std::sync::Arc;

use pdf_objects::{Data, Dict, Object, ObjectId, Span, Str, Stream, Value};

/// A resource's name by what it is: a 128-bit digest of its objects.
///
/// The digest is XXH3-128, fast and the same on every build and platform,
/// so a resource is named alike wherever and whenever content naming it is
/// written. It isn't a security boundary: two resources made to digest
/// alike are taken as one.
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
/// of one reading, not of two: two files laid out alike hold streams at
/// the same offsets, so a writer given resources of several readings holds
/// their data as bytes ([`Stream::set_bytes`]), or two of them may be named
/// as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResId(pub u128);

impl ResId {
    /// The digest of `objects`, a resource's objects as
    /// [`ResBody::Objects`] holds them, every byte of them read once.
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
/// a stream's data in memory among them, and holds them shared, so a clone
/// is cheap. Two resources of equal objects are one resource, whoever made
/// them; one already written ([`Res::written`]) is that object alone,
/// named by its number, never an object equal to it.
///
/// ```
/// use pdf_content_streams::Res;
/// use pdf_content_streams::pdf_objects::{ObjectId, dict};
///
/// let a = Res::dict(dict! { "Type" => "ExtGState", "ca" => 0.5 });
/// let b = Res::dict(dict! { "ca" => 0.5, "Type" => "ExtGState" });
/// assert_eq!(a.id, b.id);
/// assert_ne!(a.id, Res::written(ObjectId::new(7, 0)).id);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Res {
    /// Its name by what it is: the digest of its objects, or of a
    /// reference to it, for one already written.
    pub id: ResId,
    /// What it is.
    pub body: ResBody,
}

/// What a resource is.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ResBody {
    /// Its objects: the resource first, then those it reaches. A reference
    /// `(i, 0)` among them is to the `i`th.
    Objects(Arc<[Object]>),
    /// An object the caller has already written, by its number in the
    /// file being written: a page made a form, say.
    Written(ObjectId),
}

impl Res {
    /// The resource `objects` are ([`ResBody::Objects`]), named by them.
    pub fn of(objects: impl Into<Arc<[Object]>>) -> Res {
        let objects = objects.into();
        Res {
            id: ResId::of(&objects),
            body: ResBody::Objects(objects),
        }
    }

    /// A dictionary that reaches nothing else: an ExtGState or a property
    /// list.
    pub fn dict(dict: Dict) -> Res {
        Res::of([Object::from(dict)])
    }

    /// Object `id`, already written ([`ResBody::Written`]): named by its
    /// number, the digest of a reference to it.
    pub fn written(id: ObjectId) -> Res {
        Res {
            id: ResId::of(&[Object::from(id)]),
            body: ResBody::Written(id),
        }
    }
}

/// `object` written into `out` so that two objects write the same bytes
/// only when they're equal, as [`ResId`] says: each part tagged with its
/// kind, and each run of bytes and each list with its length.
fn feed(out: &mut Vec<u8>, object: &Object) {
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
    let mut entries: Vec<_> = dict.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    count(out, entries.len());
    for (key, value) in entries {
        run(out, key);
        feed(out, value);
    }
}

#[cfg(test)]
mod tests;
