//! Copies: an object as it was read ([`Body::Read`]) written as the bytes
//! it was read from, where those are still the bytes it was read from and
//! still mean what they meant: in the reading the write is given, under the
//! key its strings and data are encrypted with, or none; or, read from an
//! object stream, in one of the write's. Under a renumbering, its header
//! and each reference the renumbering names otherwise are written anew in
//! place, by the reference's own place, the rest as it was; at another
//! number only where its key isn't made from its number.

use std::borrow::Cow;
use std::ops::Range;

use pdf_objects::object::{Object, ObjectId, Value};
use pdf_objects::parse::Parser;
use pdf_objects::serialize;
use pdf_tokenizer::{Kind, Tokenizer, is_regular};

use super::{Body, Encrypting, Encryptor, Error, Numbering, ReadFrom};
use crate::bytes::{SourceId, XrefEntry};
use crate::source::{Origin, Record};

/// What a write copies by: the reading its objects were read in, and
/// whether, encrypted as it is, it copies those read at offsets.
pub(super) struct Copying<'w> {
    from: &'w dyn ReadFrom,
    offsets: Offsets<'w>,
}

/// Whether a write copies the objects read at offsets, by how it's
/// encrypted.
enum Offsets<'w> {
    /// No: it's encrypted otherwise than the reading's file is.
    No,
    /// Yes: neither it nor the reading's file is encrypted.
    Plain,
    /// Each that the list says the same of in both: it's encrypted by an
    /// encryptor of the reading, `read`, as that was made of it, with the
    /// key the reading's file is encrypted with; at its own number, or at
    /// any where no method of `read`'s makes keys per object.
    Same {
        read: &'w Encryptor,
        /// The reading's document metadata stream, where `/EncryptMetadata
        /// false` leaves its data in the clear.
        metadata: Option<ObjectId>,
    },
}

/// An object in an object stream of the write's: its value, written anew;
/// or an object as read, given at a number, copied as its bytes were read
/// from another where the rules for a copy hold ([`Copying::member`]), as
/// the object stream is built, else written anew.
#[derive(Debug)]
pub(super) enum Member<'a> {
    /// Its value, written anew.
    Object(&'a Object),
    /// An object as read, at the number it's given at.
    Read(ObjectId, &'a Body<'a>),
}

impl<'a> From<&'a Object> for Member<'a> {
    fn from(object: &'a Object) -> Member<'a> {
        Member::Object(object)
    }
}

/// A copy: the bytes an object was read from, and the runs of them written
/// anew, in order and apart, each with what's written in its place.
pub(super) struct Spliced<'w> {
    bytes: Cow<'w, [u8]>,
    anew: Vec<(Range<usize>, Anew)>,
}

/// What's written in place of a run of a copy's bytes.
#[derive(Clone, Copy)]
enum Anew {
    /// The header, `n g obj`, of the number it's written at.
    Header(ObjectId),
    /// A reference, `n g R`, to the number it names now, or `null` where
    /// the renumbering names none.
    Reference(Option<ObjectId>),
}

impl<'w> Spliced<'w> {
    /// Its bytes, each run written anew as what's in its place names it now,
    /// at the end of `out`.
    pub(super) fn write(&self, out: &mut Vec<u8>) {
        out.reserve(self.bytes.len() + 16 * self.anew.len());
        let mut from = 0;
        for (run, anew) in &self.anew {
            out.extend_from_slice(&self.bytes[from..run.start]);
            let (id, word): (_, &[u8]) = match *anew {
                Anew::Header(id) => (id, b" obj"),
                Anew::Reference(Some(id)) => (id, b" R"),
                Anew::Reference(None) => {
                    out.extend_from_slice(b"null");
                    from = run.end;
                    continue;
                }
            };
            serialize::decimal(out, id.0.into());
            out.push(b' ');
            serialize::decimal(out, id.1.into());
            out.extend_from_slice(word);
            from = run.end;
        }
        out.extend_from_slice(&self.bytes[from..]);
    }

    /// Its bytes, as [`Spliced::write`] writes them: borrowed where nothing
    /// is written anew.
    fn into_bytes(self) -> Cow<'w, [u8]> {
        match self.anew.is_empty() {
            true => self.bytes,
            false => {
                let mut out = Vec::new();
                self.write(&mut out);
                Cow::Owned(out)
            }
        }
    }
}

impl<'w> Copying<'w> {
    /// What a write given `from`, encrypted by `encryptor` as it's given
    /// (a whole file's before [`Encryptor::renumbered`] and
    /// [`Encryptor::whole`]), copies by. Refused where `from`'s source says
    /// its bytes aren't those it was read from.
    pub(super) fn new(
        from: &'w dyn ReadFrom,
        encryptor: Option<&'w Encryptor>,
    ) -> Result<Copying<'w>, Error> {
        from.unchanged()?;
        let offsets = match encryptor {
            None if !from.encrypts() => Offsets::Plain,
            Some(read) if read.read_in() == Some(from.id()) => Offsets::Same {
                read,
                metadata: match read.plain_metadata() {
                    true => from.metadata(),
                    false => None,
                },
            },
            _ => Offsets::No,
        };
        Ok(Copying { from, offsets })
    }

    /// Object `id`, `body`, written on its own at `at` with its references
    /// as `to` names them: the bytes it was read from, its header and the
    /// references `to` names otherwise spliced, where it's copied; none
    /// where it's written anew. Refused where its record is another
    /// reading's or another object's, or under a renumbering its references
    /// aren't where its bytes have them ([`Error::Elsewhere`]), or its
    /// bytes aren't those it was read from ([`Error::Changed`]).
    pub(super) fn at_offset(
        &self,
        id: ObjectId,
        at: ObjectId,
        body: &Body<'_>,
        encrypting: Option<&Encrypting<'_>>,
        to: Numbering<'_>,
    ) -> Result<Option<Spliced<'w>>, Error> {
        let Body::Read { object, record } = body else {
            return Ok(None);
        };
        if !as_read(id, object, record, self.from.id())? {
            return Ok(None);
        }
        // Read at an offset its sections give, `endobj` after it.
        if record.origin != Origin::File || !record.endobj {
            return Ok(None);
        }
        // A stream's /Length a number, its data as long.
        if record
            .data
            .is_some_and(|d| !d.declared || d.indirect_length)
        {
            return Ok(None);
        }
        // Encrypted, its bytes are right under the same key alone: at its
        // own number, or where no key is made of the number, at any.
        let keeps = match &self.offsets {
            Offsets::No => false,
            Offsets::Plain => true,
            Offsets::Same { read, metadata } => {
                (at == id || !read.per_object())
                    && encrypting.is_some_and(|e| {
                        let there = |id| -> Option<&Object> { self.from.object(id) };
                        e.keeps(read, &there, *metadata, (id, at), object)
                    })
            }
        };
        if !keeps {
            return Ok(None);
        }
        let bytes = self.checked(id, record)?;
        let mut header = Parser::at(&bytes, 0);
        if header.header().ok() != Some(id) {
            return Err(Error::Elsewhere(id));
        }
        let header = header.offset();
        let mut anew = Vec::new();
        if at != id {
            anew.push((0..header, Anew::Header(at)));
        }
        if to.0.is_some() {
            // A stream's references are in its dictionary, before its data.
            let end = match record.data {
                Some(data) => data.at.start.saturating_sub(record.at.start),
                None => bytes.len(),
            };
            references(id, object, record, &bytes, header..end, to, &mut anew)?;
        }
        Ok(Some(Spliced { bytes, anew }))
    }

    /// Object `id`, `body`, which goes in an object stream of the write's,
    /// its references as `to` names them: the bytes it was read from in
    /// another, those `to` names otherwise spliced, where they're copied;
    /// none where it's written anew. The number it's written at is its
    /// container's to say, not its bytes'. Refused as
    /// [`Copying::at_offset`] refuses.
    pub(super) fn member(
        &self,
        id: ObjectId,
        body: &Body<'_>,
        to: Numbering<'_>,
    ) -> Result<Option<Cow<'w, [u8]>>, Error> {
        let Body::Read { object, record } = body else {
            return Ok(None);
        };
        if !as_read(id, object, record, self.from.id())? {
            return Ok(None);
        }
        let Origin::Stream { stream, index } = record.origin else {
            return Ok(None);
        };
        // The place the reading's sections give its number.
        let listed = match self.from.entry(id.0) {
            Some(XrefEntry::Compressed {
                stream: s,
                index: i,
            }) => (s, i) == (stream, index),
            _ => false,
        };
        if !listed || id.1 != 0 || record.at.stream != Some((stream, 0)) {
            return Err(Error::Elsewhere(id));
        }
        let bytes = self.checked(id, record)?;
        if to.0.is_none() {
            return Ok(Some(bytes));
        }
        let mut anew = Vec::new();
        references(id, object, record, &bytes, 0..bytes.len(), to, &mut anew)?;
        Ok(Some(Spliced { bytes, anew }.into_bytes()))
    }

    /// The bytes `record` names, object `id`'s, read again: refused where
    /// they don't hash as they did when it was read ([`Error::Changed`]).
    fn checked(&self, id: ObjectId, record: &Record) -> Result<Cow<'w, [u8]>, Error> {
        let bytes = self.from.written(id, record.at)?;
        match Record::hash_of(&bytes) == record.hash {
            true => Ok(bytes),
            false => Err(Error::Changed(Some(id))),
        }
    }
}

/// Each reference in `object`, object `id` as read, as `to` names it, in
/// `bytes`, the bytes `record` names: the run of each that `to` names
/// otherwise added to `anew`, after what's there. Every reference, as
/// [`Object::references`] walks them, has its place in `bytes`, within
/// `within` (past the header, before a stream's data), in the order walked
/// and apart; and each one `to` names otherwise reads there as that
/// reference, `n g R` of its number and generation, tokens of their own:
/// else `object` isn't what its bytes say, its places a caller's, and the
/// write is refused ([`Error::Elsewhere`]). Walked with a stack of its own,
/// as `references` is.
fn references(
    id: ObjectId,
    object: &Object,
    record: &Record,
    bytes: &[u8],
    within: Range<usize>,
    to: Numbering<'_>,
    anew: &mut Vec<(Range<usize>, Anew)>,
) -> Result<(), Error> {
    let elsewhere = || Error::Elsewhere(id);
    let mut past = within.start;
    let mut stack = vec![object];
    while let Some(object) = stack.pop() {
        let entries = match object.value() {
            Value::Ref(named) => {
                let at = object.at.ok_or_else(elsewhere)?;
                let start = at.start.checked_sub(record.at.start);
                let end = at.end.checked_sub(record.at.start);
                let run = match (start, end) {
                    (Some(start), Some(end))
                        if at.stream == record.at.stream
                            && past <= start
                            && start < end
                            && end <= within.end =>
                    {
                        start..end
                    }
                    _ => return Err(elsewhere()),
                };
                past = run.end;
                let now = to.at(*named);
                if now != Some(*named) {
                    if !reads_as(bytes, run.clone(), *named) {
                        return Err(elsewhere());
                    }
                    anew.push((run, Anew::Reference(now)));
                }
                continue;
            }
            Value::Array(array) => {
                stack.extend(array.iter().rev());
                continue;
            }
            Value::Dict(dict) => dict.entries(),
            Value::Stream(stream) => stream.dict.entries(),
            _ => continue,
        };
        stack.extend(entries.iter().rev().map(|entry| &entry.value));
    }
    Ok(())
}

/// Whether `run` of `bytes` reads as a reference to `id`, as the parser
/// reads one: three tokens of their own, the first starting the run and
/// the last ending it, `n g R` of its number and generation, white-space
/// or comments between them.
fn reads_as(bytes: &[u8], run: Range<usize>, (n, g): ObjectId) -> bool {
    // A token starts the run, not a byte inside one.
    if run.start > 0 && is_regular(bytes[run.start - 1]) {
        return false;
    }
    let mut tokens = Tokenizer::at(bytes, run.start)
        .filter(|token| !matches!(token.kind, Kind::Space | Kind::Comment));
    let (Some(number), Some(generation), Some(r)) = (tokens.next(), tokens.next(), tokens.next())
    else {
        return false;
    };
    let integer = |token: &pdf_tokenizer::Token<'_>, want: i64| {
        token.kind == Kind::Integer && token.integer().ok() == Some(want)
    };
    number.start == run.start
        && integer(&number, n.into())
        && integer(&generation, g.into())
        && (r.kind, r.bytes) == (Kind::Keyword, &b"R"[..])
        && r.end() == run.end
}

/// Whether `object`, given with `record` as object `id`, is as reading
/// `source` read it: not where an edit dropped its place. Refused, however
/// the write is encrypted, where `record` is another reading's, or another
/// object's: read as another number or generation than `id`'s, or its
/// place isn't among the bytes `record` names, or one is a stream and the
/// other's not.
pub(super) fn as_read(
    id: ObjectId,
    object: &Object,
    record: &Record,
    source: SourceId,
) -> Result<bool, Error> {
    if record.source != source || record.id != id {
        return Err(Error::Elsewhere(id));
    }
    let Some(at) = object.at else {
        return Ok(false);
    };
    let within =
        at.stream == record.at.stream && record.at.start <= at.start && at.end <= record.at.end;
    let stream = match object.value() {
        Value::Stream(stream) => Some(stream),
        _ => None,
    };
    if !within || stream.is_some() != record.data.is_some() {
        return Err(Error::Elsewhere(id));
    }
    // A stream's data as it was read, not data given in its place.
    Ok(stream.is_none_or(|s| s.data.at() == record.data.map(|d| d.at)))
}

/// Object `id`, `body`, written on its own at the end of `out` at `at`,
/// its references as `to` names them, copied where `copying` copies it,
/// else anew, encrypted by `encrypting` when there's one, a stream's data
/// where it was written read from `from`: where it starts.
#[allow(clippy::too_many_arguments)]
pub(super) fn written(
    out: &mut Vec<u8>,
    id: ObjectId,
    at: ObjectId,
    body: &Body<'_>,
    encrypting: Option<&Encrypting<'_>>,
    from: Option<&dyn ReadFrom>,
    copying: Option<&Copying<'_>>,
    to: Numbering<'_>,
) -> Result<usize, Error> {
    if let Some(copying) = copying {
        if let Some(spliced) = copying.at_offset(id, at, body, encrypting, to)? {
            let pos = out.len();
            spliced.write(out);
            out.push(b'\n');
            return Ok(pos);
        }
    }
    super::indirect(out, id, at, body.object(), encrypting, from, to)
}
