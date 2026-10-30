//! Object streams (ISO 32000-2, 7.5.7): which objects may go in one, by
//! the clause's list of what mustn't ([`KeptOut`]), which a reader checks
//! what it reads by too; and a container of them as the writer packs it,
//! its members as direct objects after a header of where each starts,
//! deflated.

use std::fmt;

use pdf_filters::Prediction;
use pdf_objects::dict;
use pdf_objects::object::{Dict, Object, ObjectId, Stream, Value};
use pdf_objects::serialize;

use super::{Deflate, Error, usable};
use crate::bytes::Limits;

/// The most a container's data may hold, as the reader decodes a stream:
/// [`Limits::stream`]'s default, 256 MiB, so a container the writer makes
/// is one the reader reads whole.
pub const MAX_DATA: usize = Limits::DEFAULT.stream;

/// The most objects a container holds where a writer chooses: qpdf's
/// default.
pub const MEMBERS: usize = 100;

/// The most members a container has: as many as a cross-reference
/// stream's two bytes give a member's place in it, as the writer writes
/// its entries.
pub(super) const PLACES: usize = 1 << 16;

/// Whether a container whose data is `len` bytes is one the reader takes.
pub fn fits(len: usize) -> bool {
    len <= MAX_DATA
}

/// What ISO 32000-2, 7.5.7, keeps out of an object stream: one list,
/// which the writer keeps out of those it packs ([`kept_out`]) and a
/// reader finds in those it reads. Each says how either tells it. Its
/// `Display` says what it is in a few words, as a finding names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeptOut {
    /// A stream. The writer tells it by its value ([`kept_out`]); a reader,
    /// which reads an object in an object stream as a value, by the
    /// keyword `stream` after it in the stream's data, as its dictionary is
    /// read.
    Stream,
    /// An object whose generation number isn't 0. The writer tells it by
    /// its id ([`kept_out`]); a reader can't, as an object in an object
    /// stream is read at generation 0, its entry having none (7.5.8.3).
    Generation,
    /// The file's encryption dictionary. The writer tells one by its
    /// shape ([`kept_out`]: the standard security handler's, a `/Filter`
    /// name, `/O` and `/U`), its caller leaving out the one the file it was
    /// read from names by number
    /// ([`Encryptor::dictionary_object`](super::Encryptor::dictionary_object));
    /// a reader, as the object a trailer's `/Encrypt` names.
    Encryption,
    /// The object an object stream's `/Length` refers to. The writer
    /// writes a container's `/Length` direct ([`container`]); a reader tells
    /// it by a container's `/Length`, and may name it by that container.
    Length,
    /// The linearization dictionary (Annex F), told by its `/Linearized`
    /// both ways ([`kept_out`]): one in an object stream can't be the
    /// file's first object, so it can't make the file linearized.
    Linearization,
    /// In a linearized file, the document catalog: the object the
    /// trailer's `/Root` names. A writer that lays a file out linearized
    /// keeps it out; a reader tells it where the file's linearization
    /// holds ([`Facts::linearized`](crate::Facts::linearized)).
    Catalog,
    /// In a linearized file, a page object. A writer that lays a file out
    /// linearized keeps them out; a reader tells one where the file's
    /// linearization holds, by what reaches it as a page: the page tree,
    /// or the name dictionary's templates.
    Page,
}

impl fmt::Display for KeptOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            KeptOut::Stream => "a stream",
            KeptOut::Generation => "an object at a generation other than 0",
            KeptOut::Encryption => "the file's encryption dictionary",
            KeptOut::Length => "what an object stream's /Length refers to",
            KeptOut::Linearization => "a linearization dictionary",
            KeptOut::Catalog => "a linearized file's catalog",
            KeptOut::Page => "a linearized file's page",
        })
    }
}

/// Why object `id`, `obj`, mustn't go in an object stream (7.5.7), as the
/// object alone tells: a stream; a generation other than 0; a dictionary
/// shaped as the standard security handler's encryption dictionary (a
/// `/Filter` name, `/O` and `/U`); a linearization dictionary
/// (`/Linearized`). None for any other: what only the file tells, an
/// object stream's `/Length`, or in a linearized file the catalog and its
/// pages, is the caller's to keep out.
pub fn kept_out(id: ObjectId, obj: &Object) -> Option<KeptOut> {
    match obj.value() {
        Value::Stream(_) => Some(KeptOut::Stream),
        _ if id.1 != 0 => Some(KeptOut::Generation),
        Value::Dict(d) if encryption(d) => Some(KeptOut::Encryption),
        Value::Dict(d) if d.has(b"Linearized") => Some(KeptOut::Linearization),
        _ => None,
    }
}

/// Whether object `id`, `obj`, may go in an object stream (7.5.7): not
/// what [`kept_out`] keeps out (a stream, an object whose generation isn't
/// 0, an encryption dictionary or the linearization dictionary); nor a
/// signature dictionary, so a later signing tool finds it where it
/// expects, its `/Contents` unencrypted. A cross-reference stream is a
/// stream, and a container's own `/Length` is direct ([`container`]). An
/// encryption dictionary is told by its shape alone, the standard security
/// handler's (a `/Filter` name, `/O` and `/U`): a caller leaves out what
/// the file's `/Encrypt` names and what that refers to by number
/// ([`Encryptor::dictionary_object`](super::Encryptor::dictionary_object)),
/// as a whole file does.
pub fn packable(id: ObjectId, obj: &Object) -> bool {
    if kept_out(id, obj).is_some() {
        return false;
    }
    match obj.value() {
        Value::Dict(d) => !signature(d),
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
/// direct syntax (pdf-objects' [`serialize::write`]), borrowed or owned, in
/// order: a header of each member's number and where it starts, counted
/// from `/First`, then each member followed by a newline, deflated as
/// `deflate` says. Its `/Length` is direct, as the writer writes every
/// stream's. None when its data wouldn't [`fit`](fits), or it has more
/// than 65,536 members, more than a cross-reference stream's place in it
/// says as the writer writes one. Refused: a member numbered 0 or at or
/// past [`NUMBERS`](super::NUMBERS) ([`Error::Number`]), and a number given
/// twice ([`Error::Twice`]).
pub fn container<B: AsRef<[u8]>>(
    members: &[(u32, B)],
    deflate: Deflate,
) -> Result<Option<Stream>, Error> {
    if members.len() > PLACES {
        return Ok(None);
    }
    let mut numbers = Vec::with_capacity(members.len());
    for &(n, _) in members {
        usable(n)?;
        numbers.push(n);
    }
    numbers.sort_unstable();
    if let Some(twice) = numbers.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(Error::Twice(twice[0]));
    }
    let mut header = Vec::new();
    let mut offset = 0;
    for (i, (n, bytes)) in members.iter().enumerate() {
        if i > 0 {
            header.push(b' ');
        }
        serialize::decimal(&mut header, (*n).into());
        header.push(b' ');
        serialize::decimal(&mut header, offset as u64);
        offset += bytes.as_ref().len() + 1;
    }
    header.push(b'\n');
    let first = header.len();
    if !fits(first + offset) {
        return Ok(None);
    }
    let mut data = header;
    data.reserve(offset);
    for (_, bytes) in members {
        data.extend_from_slice(bytes.as_ref());
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
