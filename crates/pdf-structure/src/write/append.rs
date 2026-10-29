//! An update after a file's last section (ISO 32000-2, 7.5.6): the objects
//! it writes, each encrypted on its own when the file is, and a section in
//! the file's cross-reference kind whose runs list only those, its `/Prev`
//! at the last section's start. Positions count from the file's header,
//! `/ID[0]` is kept, `/ID[1]` is new, and `/Encrypt` and the rest of the
//! trailer are carried. An update frees nothing: an object it no longer
//! needs stays in use, its number never given again.

use std::io;

use pdf_objects::object::{Dict, Object, ObjectId, Value};

use super::crypt::Place;
use super::{
    Bodies, Body, Encryptor, Entry, Error, NUMBERS, Options, ReadFrom, SECTION_KEYS, Section,
    Written, checked, dangling, encrypting, file_id, first_id, fresh, id_value, indirect, stopped,
};
use crate::bytes::{ByteSource, Reading, SourceId};
use crate::source::XrefKind;

/// The file an update goes after, as its last section describes it. Made
/// of a reading ([`Base::of`]), or of what a caller read otherwise
/// ([`Base::new`]), its fields set after; and given back by each write
/// ([`Written::base`]), for an update after it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Base {
    /// Its length: where the update starts.
    pub len: usize,
    /// Whether its last line is ended; if not, the update ends it.
    pub ends_line: bool,
    /// Where its header, `%PDF-`, starts: the update's positions count from
    /// there, as PDFium, pdf.js and qpdf read them. This crate's reader
    /// counts them from the file's start (ISO 32000-2, 7.5.4), so it reads
    /// an update after a file with bytes before its header only by
    /// scanning.
    pub header: usize,
    /// Where its last section starts, from the header: what its last
    /// `startxref` says, which the update's `/Prev` gives.
    pub prev: usize,
    /// Past every object number it uses, its cross-reference streams'
    /// included: where its new numbers start, below which an update's
    /// cross-reference stream isn't numbered.
    pub size: u32,
    /// Its last section's kind, which the update's is.
    pub kind: XrefKind,
    /// Its `/ID[0]`, which an RC4 or AES-128 key is made from.
    pub first_id: Option<Vec<u8>>,
    /// Its newest trailer, whose entries the update's carries.
    pub trailer: Dict,
    /// The reading its bytes are the bytes of, when it was made of one:
    /// kept by an update after it, which appends, and none after a whole
    /// file.
    pub source: Option<SourceId>,
}

impl Base {
    /// `reading`'s file, as its last section describes it: its length, its
    /// last byte, its newest section's start and kind, its trailer and its
    /// `/ID[0]`, its header at 0, as the reader counts positions from the
    /// file's start (ISO 32000-2, 7.5.4), and the reading's
    /// [`SourceId`]. New numbers start past its trailer's `/Size` (below
    /// 2³²), every number its sections list, in use or free, and its
    /// cross-reference streams' numbers.
    ///
    /// Refused: a file with bytes before its header ([`Error::Prefixed`]),
    /// whose positions this crate's reader counts from the file's start
    /// (ISO 32000-2, 7.5.4) and PDFium, pdf.js and qpdf from the header, so
    /// that an update's would be misread by one or the other; a file whose
    /// sections didn't read, read by scanning ([`Error::Unsound`]); a
    /// trailer without `/Root`
    /// ([`Error::Rootless`]); a reading as of an earlier revision
    /// ([`Options::section`](crate::Options::section)), whose last section
    /// isn't the file's ([`Error::Earlier`]), as an update after it would
    /// leave every later revision unreachable; and a source whose bytes
    /// don't read ([`Error::Io`]). A caller that means any of those, or
    /// counts positions from the header, describes the file by
    /// [`Base::new`].
    pub fn of<S: ByteSource>(reading: &Reading<S>) -> Result<Base, Error> {
        if let Some(header @ 1..) = reading.facts().header_offset {
            return Err(Error::Prefixed(header));
        }
        let trailer = reading.trailer();
        let newest = reading.facts().newest;
        let Some((prev, kind)) = newest.filter(|_| trailer.unwritten.is_none()) else {
            return Err(Error::Unsound);
        };
        let dict = trailer.dict();
        if !dict.has(b"Root") {
            return Err(Error::Rootless);
        }
        let source = reading.source();
        let io = |e: io::Error| Error::Io(e.to_string());
        if let Some(section) = reading.as_of() {
            if startxref(source).map_err(io)? != Some(section) {
                return Err(Error::Earlier(section));
            }
        }
        let len = source.len();
        let ends_line = match len {
            0 => false,
            len => {
                let last = source.read_at(len - 1..len).map_err(io)?;
                matches!(last.first(), Some(b'\n' | b'\r'))
            }
        };
        let size = (dict.get(b"Size").and_then(Object::as_integer))
            .and_then(|size| u32::try_from(size).ok());
        let listed = reading.entries().map(|(n, ..)| n.saturating_add(1)).max();
        let streams = (reading.facts().max_stream).map(|n| n.saturating_add(1));
        let size = [size, listed, streams]
            .into_iter()
            .flatten()
            .fold(1, u32::max);
        Ok(Base {
            len,
            ends_line,
            header: 0,
            prev,
            size,
            kind,
            first_id: first_id(dict),
            trailer: dict.clone(),
            source: Some(reading.id()),
        })
    }

    /// A file read otherwise: `len` bytes, its last section at `prev`, of
    /// `kind`, new numbers from `size`, `trailer` its newest trailer, its
    /// `/ID[0]` the trailer's; its header at 0, its last line taken as not
    /// ended (an update then starts with a newline), and of no reading.
    /// Each is a field, to set after.
    pub fn new(len: usize, prev: usize, size: u32, kind: XrefKind, trailer: Dict) -> Base {
        Base {
            len,
            ends_line: false,
            header: 0,
            prev,
            size,
            kind,
            first_id: first_id(&trailer),
            trailer,
            source: None,
        }
    }

    /// Whether `source` is still the file it describes, read again: as
    /// long, its last `startxref` its last section's. What keeps another
    /// writer from changing the file between this and an append is the
    /// caller's.
    pub fn holds<S: ByteSource + ?Sized>(&self, source: &S) -> io::Result<bool> {
        Ok(source.len() == self.len && startxref(source)? == Some(self.prev))
    }
}

/// An update after `base` that writes `bodies`, at the end of `out`: none
/// when there are none. After a newline, where `base`'s last line isn't
/// ended, each body at its number, in the order given, then a section of
/// `base`'s kind listing only those, in runs, positions counted from
/// `base`'s header, a stream's not deflated, numbered at
/// [`Options::xref_from`] or past `base.size` and every number written,
/// whichever is higher, passing over a number the objects written or the
/// trailer refer to and no object written holds. Its positions are `base.len` and its offset from
/// where it starts in `out`, so `out` may be empty or hold the file. Of
/// the [`Options`], it takes `deflate`, `xref_from` and `stop`; it writes
/// each object on its own, never in an object stream.
///
/// - **The trailer** carries `base`'s newest trailer's entries but those
///   a section writes anew (ISO 32000-2, 7.5.6): `/Root`, `/Prev`, at
///   `base.prev`, `/Info`, `/Encrypt` and `/ID` first, then the rest in
///   `base`'s order, then `/Size`. Its values are written as they are,
///   never encrypted, as a trailer isn't an object (7.6.2).
/// - **The `/ID`** keeps its first string, the encryptor's, else `base`'s;
///   the second is XXH3-128 of what the update wrote before its section,
///   so the same update writes the same bytes. A file that isn't
///   encrypted and has no first string gets none.
/// - **Encrypted** as `base`'s file is, by `encryptor`, which is that
///   file's ([`Encryptor::of`]): every string and stream's data as
///   [`Encrypting`](super::Encrypting) says; a stream's whose data is
///   where it was written read from `from`, decrypted, and encrypted again.
///   Its `/Encrypt` is repeated as the file has it, a reference or the
///   dictionary.
///
/// **Under RC4**, an object written again at its number and generation is
/// encrypted with the keystream its earlier version, still in the file,
/// was: each string and stream of an object from the start of one
/// keystream, made from the file's key and the object's number and
/// generation (Algorithm 1). So where a string's or stream's plaintext
/// changed, the XOR of the two ciphertexts is the XOR of the two
/// plaintexts, and one who holds the file, which holds both, has what
/// changed. AES-128 and AES-256 don't leak so, as each string and stream
/// has an IV of its own, derived from its data. A caller who doesn't want
/// that writes the file whole under a new AES-256 encryption
/// ([`Encryptor::create`]) instead.
///
/// Refused, with nothing written ([`Error`]): a `base` without `/Root`; an
/// encryptor that isn't `base`'s file's, or none for an encrypted file;
/// a number 0 or at or past [`NUMBERS`], or given twice; a body at the
/// encryption dictionary's number; one at a number the file lists free
/// where its keys are made per object ([`Encryptor::of`]); a
/// cross-reference stream with no number left below [`NUMBERS`]; a
/// stream's data where it was written that isn't the reading's own for its
/// number, or with no `from`; what pdf-objects' writer refuses, or the
/// encryptor; and a write stopped.
pub fn update<'a>(
    out: &mut Vec<u8>,
    base: &Base,
    bodies: impl IntoIterator<Item = (ObjectId, Body<'a>)>,
    encryptor: Option<&Encryptor>,
    from: Option<&dyn ReadFrom>,
    options: &Options<'_>,
) -> Result<Option<Written>, Error> {
    let start = out.len();
    let bodies: Vec<(ObjectId, Body<'a>)> = bodies.into_iter().collect();
    if bodies.is_empty() {
        return Ok(None);
    }
    let written = appended(out, start, base, &bodies, encryptor, from, options);
    if written.is_err() {
        out.truncate(start);
    }
    written.map(Some)
}

/// [`update`]'s section, `out` from `start` on.
fn appended(
    out: &mut Vec<u8>,
    start: usize,
    base: &Base,
    bodies: &[(ObjectId, Body<'_>)],
    encryptor: Option<&Encryptor>,
    from: Option<&dyn ReadFrom>,
    options: &Options<'_>,
) -> Result<Written, Error> {
    let Some(root) = base.trailer.get(b"Root") else {
        return Err(Error::Rootless);
    };
    // A null is no entry (ISO 32000-2, 7.3.9), so no encryption.
    let encrypt = (base.trailer.get(b"Encrypt")).filter(|e| !e.is_null());
    let matched = match (encryptor.map(Encryptor::place), encrypt.map(Object::value)) {
        (None, None) => true,
        (Some(Place::At(at)), Some(Value::Ref(id))) => at == *id,
        (Some(Place::Direct), Some(Value::Dict(_))) => true,
        _ => false,
    };
    if !matched {
        return Err(Error::Unmatched);
    }
    let encrypt_at = encryptor.and_then(Encryptor::at).map(|at| at.0);
    checked(bodies, encrypt_at, encryptor)?;
    // What the encryptor resolves references by, where there's one.
    let resolve = encryptor.map(|_| Bodies::new(bodies, from));
    let encrypting = match (encryptor, &resolve) {
        (Some(e), Some(resolve)) => Some(encrypting(e, &base.trailer, resolve)?),
        _ => None,
    };

    let at = |out: &Vec<u8>| base.len + (out.len() - start);
    if !base.ends_line {
        out.push(b'\n');
    }
    let mut section = Section::append(base.size, base.header);
    for (id, body) in bodies {
        stopped(options.stop)?;
        let pos = at(out);
        indirect(out, *id, body.object(), encrypting.as_ref(), from)?;
        let generation = id.1;
        section.insert(id.0, Entry::InUse { pos, generation });
    }
    let mut trailer = Dict::new();
    trailer.set("Root", root.clone());
    trailer.set("Prev", base.prev as i64);
    if let Some(info) = base.trailer.get(b"Info") {
        trailer.set("Info", info.clone());
    }
    if let Some(e) = encryptor {
        match e.place() {
            Place::At(at) => trailer.set("Encrypt", Object::from(at)),
            Place::Direct | Place::New => trailer.set("Encrypt", e.dictionary().clone()),
        }
    }
    let kept = (encryptor.and_then(Encryptor::first_id).map(<[u8]>::to_vec))
        .or_else(|| base.first_id.clone());
    let id = file_id(kept, encryptor.is_some(), &out[start..]);
    if let Some(id) = &id {
        trailer.set("ID", id_value(id));
    }
    let carried = (base.trailer.iter())
        .filter(|(key, _)| !SECTION_KEYS.contains(key) && !matches!(*key, b"Root" | b"Info"));
    for (key, value) in carried {
        trailer.set(key, value.clone());
    }
    let pos = at(out);
    let mut kept = trailer.clone();
    match base.kind {
        XrefKind::Table => section.table(out, trailer, pos)?,
        XrefKind::Stream => {
            let past = [options.xref_from, base.size, section.size()]
                .into_iter()
                .fold(0, u32::max);
            let num = fresh(past, &dangling(bodies, &base.trailer, past, &[]));
            if num >= NUMBERS {
                return Err(Error::NoNumber(num));
            }
            section.stream(out, num, trailer, pos, options.deflate)?;
        }
    }
    let size = section.size();
    kept.set("Size", i64::from(size));
    let prev = pos - base.header;
    let base = Base {
        len: at(out),
        ends_line: true,
        header: base.header,
        prev,
        size,
        kind: base.kind,
        first_id: (id.as_ref().map(|[first, _]| first.clone())).or_else(|| base.first_id.clone()),
        trailer: kept,
        source: base.source,
    };
    Ok(Written {
        section,
        start: prev,
        size,
        id,
        base,
    })
}

/// What the last `startxref` in `source` says: read back from its end, a
/// window at a time, twice as many bytes each time it isn't found; none
/// where there's none, or no offset after it.
fn startxref<S: ByteSource + ?Sized>(source: &S) -> io::Result<Option<usize>> {
    const KEYWORD: &[u8] = b"startxref";
    let len = source.len();
    let mut window = len.min(1024);
    loop {
        let bytes = source.read_at(len - window..len)?;
        let found = (bytes.windows(KEYWORD.len())).rposition(|w| w == KEYWORD);
        if let Some(at) = found {
            let after = &bytes[at + KEYWORD.len()..];
            let white = |b: &u8| matches!(b, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ');
            let digits: Vec<u8> = (after.iter().skip_while(|b| white(b)))
                .take_while(|b| b.is_ascii_digit())
                .copied()
                .collect();
            let text = std::str::from_utf8(&digits).unwrap_or_default();
            return Ok(text.parse().ok());
        }
        if window == len {
            return Ok(None);
        }
        window = (window * 2).min(len);
    }
}
