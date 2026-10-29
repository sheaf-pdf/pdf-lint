//! Object streams (ISO 32000-2, 7.5.7): which objects may go in one, and
//! a container of them as the writer packs it, its members as direct
//! objects after a header of where each starts, deflated.

use pdf_filters::Prediction;
use pdf_objects::dict;
use pdf_objects::object::{Dict, Object, ObjectId, Stream, Value};
use pdf_objects::serialize;

use super::{Deflate, Error};
use crate::bytes::Limits;

/// The most a container's data may hold, as the reader decodes a stream:
/// [`Limits::stream`]'s default, 256 MiB, so a container the writer makes
/// is one the reader reads whole.
pub const MAX_DATA: usize = Limits::DEFAULT.stream;

/// The most objects a container holds where a writer chooses: qpdf's
/// default.
pub const MEMBERS: usize = 100;

/// Whether a container whose data is `len` bytes is one the reader takes.
pub fn fits(len: usize) -> bool {
    len <= MAX_DATA
}

/// Whether object `id`, `obj`, may go in an object stream (7.5.7): not a
/// stream, an object whose generation isn't 0, an encryption dictionary or
/// the linearization dictionary; nor a signature dictionary, so a later
/// signing tool finds it where it expects, its `/Contents` unencrypted. A
/// cross-reference stream is a stream, and a container's own `/Length` is
/// direct ([`container`]). An encryption dictionary is told by its shape
/// alone, the standard security handler's (a `/Filter` name, `/O` and
/// `/U`): a caller leaves out what the file's `/Encrypt` names and what
/// that refers to by number
/// ([`Encryptor::dictionary_object`](super::Encryptor::dictionary_object)),
/// as a whole file does.
pub fn packable(id: ObjectId, obj: &Object) -> bool {
    if id.1 != 0 {
        return false;
    }
    match obj.value() {
        Value::Stream(_) => false,
        Value::Dict(d) => !(encryption(d) || signature(d) || d.has(b"Linearized")),
        _ => true,
    }
}

/// Whether `d` has an encryption dictionary's keys: a `/Filter` name, `/O`
/// and `/U`.
fn encryption(d: &Dict) -> bool {
    d.get(b"Filter").and_then(Object::as_name).is_some() && d.has(b"O") && d.has(b"U")
}

/// Whether `d` is a signature dictionary, told after PDFium's tell: a
/// `/Type` of `/Sig` or `/DocTimeStamp`, or with no `/Type`, that `/FT`;
/// with neither, a signature's `/ByteRange` and `/Contents`, since its
/// `/Type` is optional (ISO 32000-2, Table 255).
fn signature(d: &Dict) -> bool {
    match d.get(b"Type").or_else(|| d.get(b"FT")) {
        Some(kind) => {
            (kind.as_name()).is_some_and(|kind| kind == b"Sig" || kind == b"DocTimeStamp")
        }
        None => d.has(b"ByteRange") && d.has(b"Contents"),
    }
}

/// An object stream of `members`, each its number and its object as
/// direct syntax (pdf-objects' [`serialize::write`]), in order: a header
/// of each member's number and where it starts, counted from `/First`,
/// then each member followed by a newline, deflated as `deflate` says. Its
/// `/Length` is direct, as the writer writes every stream's. None when its
/// data wouldn't [`fit`](fits).
pub fn container(members: &[(u32, Vec<u8>)], deflate: Deflate) -> Result<Option<Stream>, Error> {
    let mut header = Vec::new();
    let mut offset = 0;
    for (i, (n, bytes)) in members.iter().enumerate() {
        if i > 0 {
            header.push(b' ');
        }
        serialize::decimal(&mut header, (*n).into());
        header.push(b' ');
        serialize::decimal(&mut header, offset as u64);
        offset += bytes.len() + 1;
    }
    header.push(b'\n');
    let first = header.len();
    if !fits(first + offset) {
        return Ok(None);
    }
    let mut data = header;
    data.reserve(offset);
    for (_, bytes) in members {
        data.extend_from_slice(bytes);
        data.push(b'\n');
    }
    let dict = dict! {
        "Type" => "ObjStm",
        "N" => members.len() as i64,
        "First" => first as i64,
        "Filter" => "FlateDecode",
    };
    let mut deflated = Vec::new();
    deflate.encode(&data, Prediction::None, &mut deflated)?;
    Ok(Some(Stream::new(dict, deflated)))
}
