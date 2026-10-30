//! A whole file: the header, every object at its number, the encryption
//! dictionary, the object streams, and one cross-reference section in the
//! kind asked for, with no `/Prev` or `/XRefStm`. Encrypted, each object
//! is encrypted under the number it's written at, an object stream's
//! members with it, and the encryptor's `/Encrypt` and `/ID[0]` are kept.
//! Asked to pack, what may go in an object stream goes in one, grouped as
//! the caller says or by number, which are built on threads, each
//! encrypted once, as a stream, under its own number. An object as read is
//! copied where the rules for a copy hold ([`Body::Read`]). Under a
//! renumbering, each object is written at the number it gives, and each
//! reference as it names it.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;

use pdf_objects::object::{Dict, Object, ObjectId, Value};
use pdf_objects::serialize;

use super::copy::{Copying, Member, written};
use super::crypt::Place;
use super::{
    Base, Bodies, Body, Deflate, Encrypting, Encryptor, Entry, Error, MEMBERS, NUMBERS, Numbering,
    Options, Pack, ReadFrom, SECTION_KEYS, Section, Written, bodies as read, checked,
    container as objstm, dangling, file_id, first_id, fresh, id_value, packable, stopped, usable,
};
use crate::source::{Document, XrefKind};

/// A whole file of `bodies`, at the end of `out`: the header, at
/// `options.version`, a comment of bytes past ASCII; each body at its
/// number, in the order given; the encryption dictionary; the object
/// streams, when `options.pack` says; and one cross-reference section of
/// `options.kind`, with no `/Prev`. Its positions count from where it
/// starts in `out`, its `%PDF-`, and so do [`Written`]'s, so `out` may
/// hold other bytes before it.
///
/// - **The trailer** is `trailer`'s entries but those a section writes
///   anew or doesn't have (`/Prev`, `/XRefStm`, a cross-reference
///   stream's dictionary's own, `/Encrypt` and `/ID`), then `/Encrypt`
///   and `/ID`. Its values are written as they are, never encrypted, as a
///   trailer isn't an object (ISO 32000-2, 7.6.2), and a reader reads
///   them as written: a direct `/Info` dictionary's strings are in the
///   clear in an encrypted file.
/// - **The `/ID`** keeps its first string, the encryptor's, else
///   `trailer`'s; the second is XXH3-128 of the bytes written before the
///   section, so the same bodies write the same bytes. A file that isn't
///   encrypted and has no first string has none; an encrypted one with
///   none gets the hash for both.
/// - **Encrypted** by `encryptor`, every string and stream's data is
///   encrypted as [`Encrypting`] says, but a stream's whose data is where
///   it was written, which is read from `from` as the reading hands it out,
///   decrypted, and encrypted again under `encryptor`. Its encryption
///   dictionary is written as the encryptor has it, as an object at
///   [`Options::encrypt_at`], else at its number in the file it was read
///   from, else past every body; or direct in the trailer where that file
///   had it so. Where its keys are made per object, each number its file
///   lists free is listed free at the generation that file's entry gives
///   it ([`Encryptor::free`]), `/Size` past it, and no object of the
///   writer's own is numbered there.
/// - **Copied**, an object as read ([`Body::Read`]) is written as the
///   bytes it was read from, read from `from`, where the rules for a copy
///   hold; one read from an object stream, which goes in one of the
///   write's, as its bytes there. Packed, one read at an offset that may go
///   in an object stream goes in one, written anew, unless the caller's
///   groups leave its number out ([`Pack::groups`]).
/// - **Packed**, what may go in an object stream ([`packable`]), but an
///   encryption dictionary and what one refers to,
///   goes in one after the rest, grouped by [`Pack::groups`], else by
///   number, [`MEMBERS`] each; numbered from [`Pack::reuse`], then past
///   every body and [`Options::xref_from`], the cross-reference stream
///   after them, each passing over a number what's written refers to and
///   doesn't hold, so that a reference to no object, which a reader reads
///   as null (ISO 32000-2, 7.3.10), doesn't come to name one of the
///   writer's own (a new encryption dictionary's number passes over them
///   too); built on up to [`Options::threads`] threads, the same
///   bytes on any number; each encrypted once, whole, as a stream, under
///   its number. An object stream whose data would be more than the reader
///   decodes ([`MAX_DATA`](super::MAX_DATA)) is written as its members on
///   their own, and so are all of them where no number is left for the
///   cross-reference stream, or under an encryption that leaves streams in
///   the clear and strings not ([`Encryptor::packs`]).
/// - **Renumbered** by [`Options::renumber`], each body is written at the
///   number it gives, each reference in it and in `trailer` named as it
///   names it, `null` where it names none, `trailer`'s before the writer's
///   own entries go in; a body it gives none refused. The encryption
///   dictionary goes at the number it gives the encryptor's, else past
///   every object, its references renumbered, a direct one's too, and each
///   object is encrypted under the number it's written at, by the encryptor
///   in the numbers written ([`Encryptor::renumbered`]); under RC4, an
///   object written at a number another had in the file the encryptor was
///   read from leaks the XOR of the two plaintexts to one who keeps both
///   files. An object as read is copied only where nothing in it moves. The
///   numbers checked, listed and taken by the writer's own objects are those
///   written, but [`Pack::groups`]', which are those given (see
///   [`Options::renumber`]); what's written says it was renumbered
///   ([`Base::renumbered`]).
///
/// A whole file's section lists every number from 0 to the last, so
/// objects numbered sparsely make many rows: as a stream, deflated, more
/// than the reader takes of a file its size (as many as it has bytes, or
/// 2²⁰ if that's more, [`Limits::entries`](crate::Limits::entries)), so
/// that the objects past them read back missing, but by a reading told to
/// take as many.
///
/// Refused, with nothing written ([`Error`]): a number 0 or at or past
/// [`NUMBERS`], a body's, the encryption dictionary's or the
/// cross-reference stream's; one given twice; a body the renumbering gives
/// no number, and two it writes at one; a body at the encryption
/// dictionary's; a body at a number `encryptor`'s file lists free, where
/// its keys are made per object ([`Encryptor::of`]); a table asked to pack;
/// a stream's data where it was written that isn't the reading's own for
/// its number, or with no `from`; a group naming a number that isn't
/// packed, or one twice; a `from` whose source says it changed, and a copy
/// whose bytes aren't those it was read from ([`Error::Changed`]), or
/// whose record is another reading's or another object's
/// ([`Error::Elsewhere`]); what pdf-objects' writer refuses, or the
/// encryptor; and a write stopped.
pub fn full<'a>(
    out: &mut Vec<u8>,
    bodies: impl IntoIterator<Item = (ObjectId, Body<'a>)>,
    trailer: &Dict,
    encryptor: Option<&Encryptor>,
    from: Option<&dyn ReadFrom>,
    options: &Options<'_>,
) -> Result<Written, Error> {
    let start = out.len();
    let bodies: Vec<(ObjectId, Body<'a>)> = bodies.into_iter().collect();
    let written = file(out, start, &bodies, trailer, encryptor, from, options);
    if written.is_err() {
        out.truncate(start);
    }
    written
}

/// `doc`'s objects as a whole file, through [`full`], its trailer
/// `doc`'s, each as read where it is ([`bodies`](super::bodies)), so that
/// given the reading `doc` was read in, it's copied where the rules for a
/// copy hold (a document made by
/// [`Reading::to_document`](crate::Reading::to_document), that reading
/// kept: one read whole has dropped it, so a stream of it is refused,
/// [`Error::Placed`], or [`Error::Elsewhere`] given another reading): but object streams and cross-reference streams, whose
/// members and entries are written anew, and a linearization dictionary,
/// which would no longer hold; and its encryption dictionary, which the
/// writer writes from `encryptor`, if any. Written under another
/// encryption than the one `doc`'s file was read with (none, or a new
/// one), the objects its encryption dictionary refers to are left out too.
/// What's left out is told by the numbers of `doc`. It gives every other
/// object of `doc`, so a renumbering ([`Options::renumber`]) that puts two
/// at one number is refused ([`Error::Twice`]): a caller whose renumbering
/// shares a number gives one of them through [`full`].
pub fn document(
    out: &mut Vec<u8>,
    doc: &Document,
    encryptor: Option<&Encryptor>,
    from: Option<&dyn ReadFrom>,
    options: &Options<'_>,
) -> Result<Written, Error> {
    let trailer = doc.trailer.dict();
    let left = encryption_objects(doc, encryptor);
    let bodies = read(doc).filter(|(id, body)| !left_out(body.object()) && !left.contains(id));
    full(out, bodies, trailer, encryptor, from, options)
}

/// Whether `obj` is one a whole file leaves out: an object stream (its
/// members are written anew, on their own or in the write's object
/// streams), a cross-reference stream or a linearization dictionary.
fn left_out(obj: &Object) -> bool {
    match obj.value() {
        Value::Stream(s) => s.dict.has_type(b"ObjStm") || s.dict.has_type(b"XRef"),
        Value::Dict(d) => d.has(b"Linearized"),
        _ => false,
    }
}

/// The objects of `doc`'s encryption dictionary that a whole file under
/// `encryptor` leaves out: the dictionary, when its trailer's `/Encrypt`
/// names one; and, but where `encryptor` is how `doc`'s file was
/// encrypted, whose dictionary refers to them as it did, the objects its
/// values refer to, as the reader reads them before it has a key (not
/// followed further).
fn encryption_objects(doc: &Document, encryptor: Option<&Encryptor>) -> HashSet<ObjectId> {
    let Some(encrypt) = doc.trailer.dict().get(b"Encrypt") else {
        return HashSet::new();
    };
    let named = encrypt.as_reference();
    let same = encryptor.is_some_and(|e| match e.place() {
        Place::At(at) => named == Some(at),
        Place::Direct => encrypt.as_dict().is_some(),
        Place::New => false,
    });
    let mut left: HashSet<ObjectId> = named.into_iter().collect();
    if !same {
        let dict = match named {
            Some(id) => doc.objects().find(|&(at, _)| at == id).map(|(_, obj)| obj),
            None => Some(encrypt),
        };
        left.extend(dict.into_iter().flat_map(Object::references));
    }
    left
}

/// [`full`]'s file, `out` from `start` on.
fn file(
    out: &mut Vec<u8>,
    start: usize,
    bodies: &[(ObjectId, Body<'_>)],
    trailer: &Dict,
    encryptor: Option<&Encryptor>,
    from: Option<&dyn ReadFrom>,
    options: &Options<'_>,
) -> Result<Written, Error> {
    if options.pack.is_some() && options.kind == XrefKind::Table {
        return Err(Error::PackedTable);
    }
    let to = Numbering(options.renumber);
    let pack = (options.pack).filter(|_| encryptor.is_none_or(Encryptor::packs));
    // The encryptor as given, whose list and place are of the numbers its
    // file has, which are what's given; and in the numbers written.
    let given = encryptor;
    let renumbered = match (encryptor, to.0) {
        (Some(e), Some(to)) => Some(e.renumbered(to)),
        _ => None,
    };
    let encryptor = renumbered.as_deref().or(encryptor);
    // Past every number written, which a body given and not written is
    // refused at below.
    let past = (bodies.iter())
        .filter_map(|(id, _)| to.at(*id))
        .map(|at| at.0.saturating_add(1))
        .fold(1, u32::max);
    let reuse = options.pack.map_or(&[][..], |pack| pack.reuse);
    // The writer numbers objects of its own only for a stream section,
    // object streams and a new encryption dictionary.
    let numbers = options.kind == XrefKind::Stream
        || pack.is_some()
        || encryptor.is_some_and(|e| e.place() == Place::New && options.encrypt_at.is_none());
    let dangling = match numbers {
        true => dangling(bodies, to, trailer, past, reuse),
        false => HashSet::new(),
    };
    let encrypt_at = match encryptor.map(Encryptor::place) {
        None | Some(Place::Direct) => None,
        Some(Place::At(at)) => Some(options.encrypt_at.unwrap_or(at)),
        Some(Place::New) => {
            let at = fresh(past, |n| dangling.contains(&n));
            Some(options.encrypt_at.unwrap_or((at, 0)))
        }
    };
    let at = checked(bodies, to, encrypt_at.map(|at| at.0), given)?;
    if let Some(at) = encrypt_at {
        usable(at.0)?;
    }
    // What's copied, by the encryption as given.
    let copying = from.map(|from| Copying::new(from, given)).transpose()?;
    let copying = copying.as_ref();
    // One section, whose /Encrypt is the only one: what's at a number the
    // file read named so, and this one doesn't, is encrypted.
    let whole = encryptor.map(|e| e.whole(encrypt_at));
    let encryptor = whole.as_deref().or(encryptor);
    // What the encryptor resolves references by, where there's one: the
    // bodies and the reading, by the numbers they're given at.
    let resolve = encryptor.map(|_| Bodies::new(bodies, from));
    let encrypting = match (encryptor, &resolve) {
        (Some(e), Some(resolve)) => Some(e.encrypting_at(trailer, resolve, to)?),
        _ => None,
    };
    let encrypting = encrypting.as_ref();

    out.extend_from_slice(format!("%PDF-{}\n", options.version).as_bytes());
    out.extend_from_slice(super::BINARY);
    let mut section = Section::full(1);
    let mut next = 1;
    // What goes in object streams, written once they're numbered: each
    // given at, written at, and its body.
    let mut members: Vec<(ObjectId, ObjectId, &Body<'_>)> = Vec::new();
    for ((id, body), &at) in bodies.iter().zip(&at) {
        stopped(options.stop)?;
        let (id, obj) = (*id, body.object());
        next = next.max(at.0 + 1);
        let dictionary = encryptor.is_some_and(|e| e.dictionary_object(at));
        if pack.is_some() && packable(at, obj) && !dictionary {
            members.push((id, at, body));
            continue;
        }
        let pos = written(out, id, at, body, encrypting, from, copying, to)? - start;
        let generation = at.1;
        section.insert(
            at.0,
            Entry::InUse {
                offset: pos,
                generation,
            },
        );
    }
    let given_id = first_id(trailer);
    let mut trailer = trailer.clone();
    for key in SECTION_KEYS {
        trailer.remove(key);
    }
    // The caller's entries in the numbers written, before the writer's own,
    // which are.
    if let Some(to) = to.0 {
        let mut renumbered = Object::from(trailer);
        renumbered.renumber(|id| to.renumber(id));
        trailer = match renumbered.into_value() {
            Value::Dict(dict) => dict,
            _ => unreachable!("a dictionary renumbered is one"),
        };
    }
    if let Some(e) = encryptor {
        match encrypt_at {
            None => trailer.set("Encrypt", e.dictionary().clone()),
            Some(at) => {
                let dict = Object::from(e.dictionary().clone());
                let pos = serialize::indirect(out, at, &dict).map_err(Error::Serialize)? - start;
                let generation = at.1;
                section.insert(
                    at.0,
                    Entry::InUse {
                        offset: pos,
                        generation,
                    },
                );
                trailer.set("Encrypt", Object::from(at));
                next = next.max(at.0 + 1);
            }
        }
    }
    if let Some(pack) = pack {
        let packing = Packing {
            start,
            encryptor,
            encrypting,
            from,
            copying,
            options,
            dangling: &dangling,
            to,
        };
        next = packing.packed(out, &mut section, &members, pack, next)?;
    }
    // The numbers the file read lists free under keys made per object,
    // free at the generation each is to be used at next, so that a tool
    // that follows this file's free list doesn't take one at a deleted
    // object's key.
    for (n, generation) in encryptor.into_iter().flat_map(Encryptor::free_numbers) {
        if !section.lists(n) {
            section.insert(n, Entry::Free { generation });
        }
    }
    let kept = (encryptor.and_then(Encryptor::first_id).map(<[u8]>::to_vec)).or(given_id);
    let id = file_id(kept, encryptor.is_some(), &out[start..]);
    if let Some(id) = &id {
        trailer.set("ID", id_value(id));
    }
    let pos = out.len() - start;
    let mut kept = trailer.clone();
    match options.kind {
        XrefKind::Table => section.table(out, trailer, pos)?,
        XrefKind::Stream => {
            let at = fresh(next.max(options.xref_from), |n| {
                dangling.contains(&n) || section.lists(n)
            });
            usable(at)?;
            section.stream(out, at, trailer, pos, options.deflate)?;
        }
    }
    let size = section.size();
    kept.set("Size", i64::from(size));
    let base = Base {
        len: out.len() - start,
        ends_line: true,
        header: 0,
        prev: pos,
        size,
        kind: options.kind,
        first_id: id.as_ref().map(|[first, _]| first.clone()),
        trailer: kept,
        encryption: encryptor.map(|e| e.dictionary().clone()),
        source: None,
        renumbered: to.0.is_some(),
    };
    Ok(Written {
        section,
        start: pos,
        size,
        id,
        base,
    })
}

/// What a whole file's object streams are written with.
struct Packing<'w> {
    /// Where the file starts in `out`.
    start: usize,
    encryptor: Option<&'w Encryptor>,
    encrypting: Option<&'w Encrypting<'w>>,
    from: Option<&'w dyn ReadFrom>,
    /// What's copied by, where `from` is given.
    copying: Option<&'w Copying<'w>>,
    options: &'w Options<'w>,
    /// The numbers what's written refers to and doesn't hold, which no
    /// object stream takes.
    dangling: &'w HashSet<u32>,
    /// The renumbering.
    to: Numbering<'w>,
}

impl Packing<'_> {
    /// `members`, each given at, written at, and its body, written after
    /// the rest of `out`, grouped as `pack` says by the numbers they're
    /// given at, numbered from its `reuse` first, and listed in `section`,
    /// whose numbers are below `next`: each object stream, which lists its
    /// members, and on their own those no object stream takes. What's
    /// next past the object streams.
    fn packed(
        &self,
        out: &mut Vec<u8>,
        section: &mut Section,
        members: &[(ObjectId, ObjectId, &Body<'_>)],
        pack: Pack<'_>,
        mut next: u32,
    ) -> Result<u32, Error> {
        type Packed<'m, 'b> = (ObjectId, ObjectId, &'m Body<'b>);
        let by_number: HashMap<u32, Packed<'_, '_>> = (members.iter())
            .map(|&member| (member.0.0, member))
            .collect();
        let mut numbers: Vec<u32> = by_number.keys().copied().collect();
        numbers.sort_unstable();
        let grouped = match pack.groups {
            Some(groups) => groups(&numbers),
            None => (numbers.chunks(MEMBERS).map(<[u32]>::to_vec)).collect(),
        };
        let mut placed = HashSet::with_capacity(members.len());
        for &n in grouped.iter().flatten() {
            if !by_number.contains_key(&n) || !placed.insert(n) {
                return Err(Error::Group(n));
            }
        }
        let grouped: Vec<Vec<u32>> = grouped.into_iter().filter(|g| !g.is_empty()).collect();
        // Those no group takes, on their own.
        for &member in members.iter().filter(|(id, ..)| !placed.contains(&id.0)) {
            self.plain(out, section, member)?;
        }
        // The numbers the members are written at, where they aren't those
        // they're given at.
        let written: Option<HashSet<u32>> =
            (self.to.0.is_some()).then(|| members.iter().map(|(_, at, _)| at.0).collect());
        let member_at = |n: u32| match &written {
            Some(written) => written.contains(&n),
            None => by_number.contains_key(&n),
        };
        let encryptor = self.encryptor;
        let taken = |n| {
            section.lists(n)
                || member_at(n)
                || self.dangling.contains(&n)
                || encryptor.is_some_and(|e| e.free(n).is_some())
        };
        let from = next.max(self.options.xref_from);
        let Some(given) = container_numbers(grouped.len(), pack.reuse, from, taken) else {
            for group in &grouped {
                for n in group {
                    self.plain(out, section, by_number[n])?;
                }
            }
            return Ok(next);
        };
        // Each member's bytes as read, where it's copied, else its value,
        // at the number it's written at.
        let mut groups: Vec<(u32, Vec<(ObjectId, Member<'_>)>)> = Vec::with_capacity(given.len());
        for (n, group) in given.into_iter().zip(&grouped) {
            let mut members = Vec::with_capacity(group.len());
            for m in group {
                let (id, at, body) = by_number[m];
                let copied = match self.copying {
                    Some(copying) => copying.member(id, body, self.to)?,
                    None => None,
                };
                let member = copied.map_or(Member::Object(body.object()), Member::Copied);
                members.push((at, member));
            }
            groups.push((n, members));
        }
        let threads = match self.options.threads {
            0 => std::thread::available_parallelism().map_or(1, |n| n.get().min(8)),
            n => n,
        };
        let options = self.options;
        let built = containers(
            &groups,
            self.encrypting,
            self.to,
            options.deflate,
            threads,
            options.stop,
        )?;
        for ((n, group), (container, numbers)) in groups.iter().zip(built.into_iter().zip(&grouped))
        {
            let Some(container) = container else {
                for m in numbers {
                    self.plain(out, section, by_number[m])?;
                }
                continue;
            };
            let pos = out.len() - self.start;
            out.extend_from_slice(&container);
            section.insert(
                *n,
                Entry::InUse {
                    offset: pos,
                    generation: 0,
                },
            );
            for (index, (id, _)) in (0..=u16::MAX).zip(group) {
                let entry = Entry::Compressed { stream: *n, index };
                section.insert(id.0, entry);
            }
            next = next.max(n + 1);
        }
        Ok(next)
    }

    /// `member`, given at, written at, and its body, which was to be
    /// packed, written on its own after the rest of `out`, copied where
    /// it's copied, and listed in `section`.
    fn plain(
        &self,
        out: &mut Vec<u8>,
        section: &mut Section,
        (id, at, body): (ObjectId, ObjectId, &Body<'_>),
    ) -> Result<(), Error> {
        stopped(self.options.stop)?;
        let (encrypting, from, copying, to) = (self.encrypting, self.from, self.copying, self.to);
        let pos = written(out, id, at, body, encrypting, from, copying, to)? - self.start;
        let generation = at.1;
        section.insert(
            at.0,
            Entry::InUse {
                offset: pos,
                generation,
            },
        );
        Ok(())
    }
}

/// The numbers `count` object streams take: `reuse`'s first, in order, but
/// 0, one a file may not use, one `taken` and one given already; then from
/// `from` up. None when there aren't so many, or the cross-reference
/// stream after them would take a number a file may not use.
fn container_numbers(
    count: usize,
    reuse: &[u32],
    from: u32,
    taken: impl Fn(u32) -> bool,
) -> Option<Vec<u32>> {
    let mut given = Vec::with_capacity(count);
    let mut gave = HashSet::new();
    for n in reuse.iter().copied().chain(from..NUMBERS) {
        if given.len() == count {
            break;
        }
        if n != 0 && n < NUMBERS && !taken(n) && gave.insert(n) {
            given.push(n);
        }
    }
    let xref = given.iter().map(|&n| n + 1).fold(from, u32::max);
    (given.len() == count && xref < NUMBERS).then_some(given)
}

/// Each of `groups`, an object stream's number and its members, each at
/// the number it's written at, as that object stream written, a member
/// written anew with its references as `to` names them, encrypted by
/// `encrypting` when given, under its number: none for one whose data
/// would be more than the reader takes,
/// or that has more members than a place in it can say. Built on up to
/// `threads` threads, 4 object streams to one at least, `stop` checked
/// before each, and joined in order, so that the same groups build the
/// same bytes however many threads build them; a part no thread can be
/// had for is built on the calling thread.
pub(super) fn containers(
    groups: &[(u32, Vec<(ObjectId, Member<'_>)>)],
    encrypting: Option<&Encrypting<'_>>,
    to: Numbering<'_>,
    deflate: Deflate,
    threads: usize,
    stop: Option<&AtomicBool>,
) -> Result<Vec<Option<Vec<u8>>>, Error> {
    let build = |part: &[(u32, Vec<(ObjectId, Member<'_>)>)]| -> Result<Vec<_>, Error> {
        (part.iter())
            .map(|(n, members)| {
                stopped(stop)?;
                container(*n, members, encrypting, to, deflate)
            })
            .collect()
    };
    let chunk = groups.len().div_ceil(threads.max(1)).max(4);
    if groups.len() <= chunk {
        return build(groups);
    }
    let build = &build;
    std::thread::scope(|s| {
        let parts: Vec<_> = (groups.chunks(chunk))
            .map(|part| {
                let spawned = std::thread::Builder::new().spawn_scoped(s, move || build(part));
                (part, spawned.ok())
            })
            .collect();
        let mut built = Vec::with_capacity(groups.len());
        for (part, spawned) in parts {
            let part = match spawned {
                Some(thread) => match thread.join() {
                    Ok(part) => part,
                    Err(panic) => std::panic::resume_unwind(panic),
                },
                // No thread to be had: here, the same bytes.
                None => build(part),
            };
            built.extend(part?);
        }
        Ok(built)
    })
}

/// `members`, each at the number it's written at, as object stream `n`,
/// written as an indirect object, encrypted by `encrypting` when given,
/// each member's value written anew, its references as `to` names them, or
/// its bytes as read, borrowed: none when its data wouldn't
/// [`fit`](super::fits), or it has more members than a place in it can
/// say ([`objstm`]).
fn container(
    n: u32,
    members: &[(ObjectId, Member<'_>)],
    encrypting: Option<&Encrypting<'_>>,
    to: Numbering<'_>,
    deflate: Deflate,
) -> Result<Option<Vec<u8>>, Error> {
    let mut written: Vec<(u32, Cow<'_, [u8]>)> = Vec::with_capacity(members.len());
    for (id, member) in members {
        let obj = match member {
            Member::Object(obj) => obj,
            Member::Copied(bytes) => {
                written.push((id.0, Cow::Borrowed(bytes)));
                continue;
            }
        };
        let mut bytes = Vec::new();
        let refused = match to.0 {
            None => serialize::write(&mut bytes, obj),
            Some(to) => serialize::write_renumbered(&mut bytes, obj, to),
        };
        if let Err(e) = refused {
            // The refusal, said of the object.
            let named = serialize::indirect(&mut Vec::new(), *id, obj).err();
            return Err(Error::Serialize(named.unwrap_or(e)));
        }
        written.push((id.0, Cow::Owned(bytes)));
    }
    let Some(stream) = objstm(&written, deflate)? else {
        return Ok(None);
    };
    let stream = Object::from(stream);
    let mut out = Vec::new();
    match encrypting {
        Some(e) => serialize::indirect_with(&mut out, (n, 0), &stream, e)?,
        None => serialize::indirect(&mut out, (n, 0), &stream).map_err(Error::Serialize)?,
    };
    Ok(Some(out))
}
