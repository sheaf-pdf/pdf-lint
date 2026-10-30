//! Copies: an object as it was read ([`Body::Read`]) written as the bytes
//! it was read from, where those are still the bytes it was read from and
//! still mean what they meant: in the reading the write is given, at its
//! own number, under the key its strings and data are encrypted with, or
//! none; or, read from an object stream, in one of the write's. Under a
//! renumbering, only where nothing in it moves: its number, and every
//! reference in it.

use std::borrow::Cow;

use pdf_objects::object::{Object, ObjectId, Value};
use pdf_objects::parse::Parser;

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
    /// key the reading's file is encrypted with.
    Same {
        read: &'w Encryptor,
        /// The reading's document metadata stream, where `/EncryptMetadata
        /// false` leaves its data in the clear.
        metadata: Option<ObjectId>,
    },
}

/// An object in an object stream of the write's: its value, written anew,
/// or its bytes as they were read from another, copied.
#[derive(Debug)]
pub(super) enum Member<'a> {
    /// Its value, written anew.
    Object(&'a Object),
    /// Its bytes, as read.
    Copied(Cow<'a, [u8]>),
}

impl<'a> From<&'a Object> for Member<'a> {
    fn from(object: &'a Object) -> Member<'a> {
        Member::Object(object)
    }
}

impl<'w> Copying<'w> {
    /// What a write given `from`, encrypted by `encryptor` as it's given
    /// (a whole file's before [`Encryptor::whole`]), copies by. Refused
    /// where `from`'s source says its bytes aren't those it was read from.
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
    /// as `to` names them: the bytes it was read from, where they're
    /// copied; none where it's written anew. Refused where its record is
    /// another reading's or another object's ([`Error::Elsewhere`]), or its
    /// bytes aren't those it was read from ([`Error::Changed`]).
    pub(super) fn at_offset(
        &self,
        id: ObjectId,
        at: ObjectId,
        body: &Body<'_>,
        encrypting: Option<&Encrypting<'_>>,
        to: Numbering<'_>,
    ) -> Result<Option<Cow<'w, [u8]>>, Error> {
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
        // Its bytes say its number and its references: copied where
        // neither moves.
        if at != id || !to.still(object) {
            return Ok(None);
        }
        // Encrypted, its bytes are right under the same key alone.
        let keeps = match &self.offsets {
            Offsets::No => false,
            Offsets::Plain => true,
            Offsets::Same { read, metadata } => encrypting.is_some_and(|e| {
                let there = |id| -> Option<&Object> { self.from.object(id) };
                e.keeps(read, &there, *metadata, (id, at), object)
            }),
        };
        if !keeps {
            return Ok(None);
        }
        let bytes = self.checked(id, record)?;
        if Parser::at(&bytes, 0).header().ok() != Some(id) {
            return Err(Error::Elsewhere(id));
        }
        Ok(Some(bytes))
    }

    /// Object `id`, `body`, which goes in an object stream of the write's,
    /// its references as `to` names them: the bytes it was read from in
    /// another, where they're copied; none where it's written anew. The
    /// number it's written at is its container's to say, not its bytes'.
    /// Refused as [`Copying::at_offset`] refuses.
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
        // Its bytes say its references: copied where they don't move.
        if !to.still(object) {
            return Ok(None);
        }
        self.checked(id, record).map(Some)
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
        if let Some(bytes) = copying.at_offset(id, at, body, encrypting, to)? {
            let pos = out.len();
            out.extend_from_slice(&bytes);
            out.push(b'\n');
            return Ok(pos);
        }
    }
    super::indirect(out, id, at, body.object(), encrypting, from, to)
}
