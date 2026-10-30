//! What the fuzz target checks, which a unit test checks too: on any
//! bytes, a file is read without a panic, in the same way twice, with
//! every place the reading gives inside what was read; read on demand, in
//! an order the input chooses, it's the whole read, so long as no stream
//! either decodes reaches the budget, and made a document with the reading
//! kept, at a point the input chooses, it's the reading's whole read
//! after; read by position, a window at a time from a first window the
//! input chooses, it's the whole read, and each stream's data and decoded
//! data are as in memory; pruned, it reaches what it did, and an object
//! added to it is at a number none held; and so are its revision before an
//! incremental update, and the file opened by an owner's password; and
//! opened by a wrong password, then unlocked by the owner's, it's the file
//! opened by the owner's. It reads under small limits, so that a run tries
//! more inputs. It's compiled only for tests and under cargo-fuzz's
//! `--cfg fuzzing`.
//!
//! And what the `write` target checks ([`write_file`]): a file read is
//! written whole and as an update, under a renumbering the input picks or
//! none, each way the input picks, without a panic, and where it read
//! soundly, written and read again it's the same objects, each at the
//! number the renumbering gives it, those copied as the bytes they were
//! read from among them, their headers and the references that move
//! written anew in place; as the input picks, the document written is the
//! file read whole and taken by the reading, which takes it where it read
//! soundly, and leaves it as it was where it doesn't; and where a byte of
//! it changed after it was read, a copy of an object whose bytes hold it is
//! refused.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::convert::Infallible;
use std::io;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};

use pdf_crypt::Password;
use pdf_crypt::rand_core::{TryCryptoRng, TryRng};
use pdf_objects::object::{Object, ObjectId, Span, Value};
use pdf_objects::serialize;

use crate::bytes::{ByteSource, Error, Options, Reading, Whole, XrefEntry};
use crate::source::{Document, Encryption, Origin, Source, XrefKind};
use crate::write::{self, Base, Body, Encryptor, Pack};

/// The owner's password the fuzz target opens a file by, when its input
/// says to: the one the unit test's encrypted files are written with.
const OWNER: Password<'static> = Password("owner");

/// Passwords that open none of the unit test's encrypted files.
const WRONG: [Password<'static>; 2] = [Password("wrong"), Password("also wrong")];

/// The limits the fuzz target reads under: a stream decodes to at most
/// 1 MiB, and a reading's streams to 4 MiB, so that a run tries more
/// inputs.
pub fn options() -> Options {
    let mut options = Options::DEFAULT;
    options.limits.stream = 1 << 20;
    options.limits.budget = 4 << 20;
    options
}

/// Bytes in memory, read a window at a time as a file is, by position,
/// the first window `first` bytes: counting the bytes read.
#[derive(Debug)]
pub struct Counted<'a> {
    bytes: &'a [u8],
    first: usize,
    read: AtomicUsize,
}

impl<'a> Counted<'a> {
    /// `bytes`, read by position, `first` bytes first.
    pub fn new(bytes: &'a [u8], first: usize) -> Counted<'a> {
        Counted {
            bytes,
            first,
            read: AtomicUsize::new(0),
        }
    }

    /// How many bytes have been read.
    pub fn read(&self) -> usize {
        self.read.load(Ordering::Relaxed)
    }
}

impl ByteSource for Counted<'_> {
    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        self.read.fetch_add(range.len(), Ordering::Relaxed);
        let (start, end) = (range.start, range.end);
        self.bytes.get(range).map(Cow::Borrowed).ok_or_else(|| {
            let message = format!("bytes {start}..{end} aren't within the file");
            io::Error::new(io::ErrorKind::UnexpectedEof, message)
        })
    }

    fn first_window(&self) -> usize {
        self.first
    }
}

/// Bytes in memory, read a window at a time as a file is, by position,
/// the first window `first` bytes, one of which, once it's flipped
/// ([`Flipped::flip`]), reads as its complement: a file changed under a
/// reading that its source doesn't tell ([`ByteSource::unchanged`]).
#[derive(Debug)]
pub struct Flipped<'a> {
    bytes: &'a [u8],
    first: usize,
    at: AtomicUsize,
}

impl<'a> Flipped<'a> {
    /// `bytes`, read by position, `first` bytes first, none flipped.
    pub fn new(bytes: &'a [u8], first: usize) -> Flipped<'a> {
        Flipped {
            bytes,
            first,
            at: AtomicUsize::new(usize::MAX),
        }
    }

    /// Byte `at` read as its complement from now on, in place of any
    /// flipped before.
    pub fn flip(&self, at: usize) {
        self.at.store(at, Ordering::Relaxed);
    }
}

impl ByteSource for Flipped<'_> {
    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        let (start, end) = (range.start, range.end);
        let Some(bytes) = self.bytes.get(range) else {
            let message = format!("bytes {start}..{end} aren't within the file");
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, message));
        };
        let at = self.at.load(Ordering::Relaxed);
        if !(start..end).contains(&at) {
            return Ok(Cow::Borrowed(bytes));
        }
        let mut bytes = bytes.to_vec();
        bytes[at - start] ^= 0xFF;
        Ok(Cow::Owned(bytes))
    }

    fn first_window(&self) -> usize {
        self.first
    }
}

/// `source` opened as [`Reading::open_with`] opens it, but that an object
/// read by position is first given a window to a byte past the next offset
/// the cross-references list only when `bound`: without, a small first
/// window reaches every read's windows.
pub fn open<S: ByteSource>(
    source: S,
    passwords: &[Password],
    options: &Options,
    bound: bool,
) -> Result<Reading<S>, Error> {
    Reading::open_by(source, passwords, options, bound)
}

/// The fuzz target's body: `data`'s first two bytes choose how the file
/// after them is read, and the rest is the file. Read whole under
/// [`options`], it's the same twice, and every place the reading gives is
/// inside what was read: each object's, each value's and key's in it, each
/// stream's data's, the trailer's and each warning's, in the file or in
/// the decoded data of the object stream it names, which the reading
/// keeps. Read on demand, its objects asked for in an order the choice's
/// next bits give, some read unkept first, each the object and record it's
/// kept as, it's the whole read, when neither reading's decoding reached
/// the budget, and made a document with the reading kept, at a point the
/// choice's bits from the eleventh give, it's the reading's whole read
/// after. Read by position, through [`Counted`], its first window and
/// whether an object's reaches the next listed offset as the choice's
/// bits from the fourth give, it's the whole read, and each stream's data
/// and decoded data are as in memory. Pruned, what its trailer reaches is
/// the same, and every object it keeps is among it, and an object added
/// is at a number no entry held (`pruned`). When the file has an
/// incremental update, so is the revision before it ([`Options::section`]);
/// and with the choice's lowest bit set, so is the file opened by the
/// owner's password `owner`, and its revision before an update, and the
/// file opened by a wrong password, then unlocked by the owner's
/// (`unlocked`). Panics on a broken promise.
pub fn read_file(data: &[u8]) {
    let (choice, file) = match data {
        [a, b, file @ ..] => (u16::from_le_bytes([*a, *b]), file),
        _ => (0, data),
    };
    alike(file, &[], choice);
    if choice & 1 != 0 {
        alike(file, &[OWNER], choice);
        unlocked(file, choice);
    }
}

/// `file` read whole under [`options`], opened by `passwords`, as of the
/// section at `section` when there's one.
fn whole(file: &[u8], passwords: &[Password], section: Option<usize>) -> Whole {
    let mut options = options();
    options.section = section;
    Reading::in_memory(file, passwords, &options, true, None).whole(None)
}

/// The file read whole twice, the same each time, every place it gives
/// inside `file`'s bytes or what it decoded, its newest revision's trailer
/// the trailer, and read on demand in the order `choice` gives, the same;
/// and when the file has an update, the same of the revision before it.
fn alike(file: &[u8], passwords: &[Password], choice: u16) {
    let read = whole(file, passwords, None);
    let doc = &read.document;
    assert_eq!(
        *doc,
        whole(file, passwords, None).document,
        "a reading is the same twice"
    );
    within(file, doc);
    if let Some(newest) = doc.revisions.last() {
        assert_eq!(
            newest.trailer, doc.trailer,
            "the newest revision's trailer is the trailer"
        );
    }
    pruned(doc);
    on_demand(file, passwords, None, &read, choice);
    by_position(file, passwords, None, doc, choice);
    streams(file, passwords, choice);
    if let Some(update) = &doc.update {
        let section = Some(update.previous);
        let previous = whole(file, passwords, section);
        assert_eq!(
            previous.document,
            whole(file, passwords, section).document,
            "the revision before an update reads the same twice"
        );
        within(file, &previous.document);
        pruned(&previous.document);
        on_demand(file, passwords, section, &previous, choice);
        by_position(file, passwords, section, &previous.document, choice);
    }
}

/// The file, as of `section`, opened by `passwords`, read by position
/// through [`Counted`], its first window 1 to 64 bytes as `choice`'s bits
/// from the fifth give, and an object's first window to the next listed
/// offset when its fourth is set: read whole, it's `doc`, the whole read,
/// which decodes its streams in the same order, so whether or not the
/// budget is reached.
fn by_position(
    file: &[u8],
    passwords: &[Password],
    section: Option<usize>,
    doc: &Document,
    choice: u16,
) {
    let mut options = options();
    options.section = section;
    let first = 1 + usize::from((choice >> 4) & 63);
    let source = Counted::new(file, first);
    let read = open(&source, passwords, &options, choice & 8 != 0)
        .and_then(|reading| reading.into_document());
    let read = read.expect("bytes in memory are read by position without fail");
    assert_eq!(read, *doc, "a reading by position is the whole read");
}

/// The file, opened by `passwords`, in memory and read by position through
/// [`Counted`] as [`by_position`] reads it: each object's data, and its
/// data decoded, the same each way, or the same error; the data, where
/// it's borrowed, a borrow of the file's bytes.
fn streams(file: &[u8], passwords: &[Password], choice: u16) {
    let options = options();
    let reading = Reading::in_memory(file, passwords, &options, true, None);
    let first = 1 + usize::from((choice >> 4) & 63);
    let source = Counted::new(file, first);
    let by_position = open(&source, passwords, &options, choice & 8 != 0)
        .expect("bytes in memory are read by position without fail");
    for id in reading.ids() {
        let data = reading.data(id);
        if let Ok(Cow::Borrowed(data)) = &data {
            let within = file.as_ptr_range();
            assert!(
                data.is_empty() || within.contains(&data.as_ptr()),
                "{id:?}'s data is borrowed from elsewhere"
            );
        }
        let read = by_position.data(id);
        assert_eq!(
            format!("{data:?}"),
            format!("{read:?}"),
            "{id:?}'s data by position"
        );
        let decoded = format!("{:?}", reading.decoded(id));
        let read = format!("{:?}", by_position.decoded(id));
        assert_eq!(decoded, read, "{id:?} decoded by position");
    }
}

/// The file, as of `section`, opened by `passwords`, read on demand: its
/// objects asked for in the order `choice`'s bits from the second give (by
/// number, from the last, from the middle out, or the objects in object
/// streams first), every third read unkept first, which gives the object
/// and record it's then kept as, and made a document with the reading kept
/// once none, a third, two thirds or all of them are, as its bits from the
/// eleventh give; then read whole, it's that document, and it's `whole`'s,
/// when neither reached the budget.
fn on_demand(
    file: &[u8],
    passwords: &[Password],
    section: Option<usize>,
    whole: &Whole,
    choice: u16,
) {
    let mut options = options();
    options.section = section;
    let reading = Reading::in_memory(file, passwords, &options, true, None);
    let ids = ordered(&reading, choice);
    let made = ids.len() * usize::from((choice >> 10) & 3) / 3;
    let mut document = None;
    for (i, &id) in ids.iter().enumerate() {
        if i == made {
            document = Some(reading.to_document());
        }
        let unkept = (i % 3 == 0).then(|| reading.object_unkept(id));
        let got = reading.get(id);
        if let Some(unkept) = unkept {
            match (unkept, got) {
                (Ok((object, record)), Ok(kept)) => {
                    assert_eq!(object, *kept, "{id:?} read unkept is the object kept");
                    assert_eq!(Some(&record), reading.record(id), "{id:?}'s record");
                }
                (Err(unkept), Err(kept)) => {
                    assert_eq!(unkept.to_string(), kept.to_string(), "{id:?}'s failure");
                }
                (unkept, got) => panic!("{id:?} read unkept as {unkept:?}, and kept as {got:?}"),
            }
        }
    }
    let document = document.unwrap_or_else(|| reading.to_document());
    let document = document.expect("bytes in memory are made a document without fail");
    let lazily = reading.whole(None);
    assert_eq!(
        document, lazily.document,
        "a document made with its reading kept is the reading's whole read"
    );
    if !whole.reached && !lazily.reached {
        assert_eq!(
            lazily.document, whole.document,
            "a reading on demand is the whole read"
        );
    }
}

/// The objects of `reading` in the order `choice`'s bits from the second
/// give: by number, from the last, from the middle out, or the objects in
/// object streams first.
fn ordered<S: ByteSource>(reading: &Reading<S>, choice: u16) -> Vec<ObjectId> {
    let mut ids: Vec<ObjectId> = reading.ids().collect();
    match (choice >> 1) & 3 {
        0 => {}
        1 => ids.reverse(),
        2 => {
            let half = ids.len() / 2;
            ids.rotate_left(half);
        }
        _ => {
            let members: Vec<u32> = (reading.entries())
                .filter(|(_, _, entry)| matches!(entry, XrefEntry::Compressed { .. }))
                .map(|(n, _, _)| n)
                .collect();
            ids.sort_by_key(|id| members.binary_search(&id.0).is_err());
        }
    }
    ids
}

/// The file opened by a wrong password, which leaves it locked where no
/// password but its own opens it, the empty one too; then, after none, a
/// third, two thirds or all of its objects are asked for, in the order
/// `choice` gives, as its bits from the eleventh give, unlocked by another
/// wrong password, which gives it back as it was, keeping what it had read
/// and said; then by the owner's password `owner`. Read on demand in that
/// order, it's then the file opened by the owner's password
/// ([`Reading::open_with`]): each object, its record and the failure to
/// read one, what it warned of, and made a document, the whole read; but
/// where the wrong password opened it, which unlocking leaves as it was.
fn unlocked(file: &[u8], choice: u16) {
    let options = options();
    let in_memory = "bytes in memory are read without fail";
    let locked = Reading::open_with(file, &WRONG[..1], &options).expect(in_memory);
    let ids = ordered(&locked, choice);
    let asked = ids.len() * usize::from((choice >> 10) & 3) / 3;
    for &id in &ids[..asked] {
        drop(locked.get(id));
    }
    let opened = matches!(locked.encryption(), Encryption::Opened { .. });
    let before = (kept(&locked), locked.encryption().clone());
    let locked = match locked.unlock(&WRONG[1].encodings()) {
        Ok(reading) | Err((reading, _)) => reading,
    };
    let after = (kept(&locked), locked.encryption().clone());
    assert_eq!(
        after, before,
        "a reading a wrong password doesn't unlock is as it was"
    );
    let unlocked = match locked.unlock(&OWNER.encodings()) {
        Ok(reading) | Err((reading, _)) => reading,
    };
    if opened {
        let after = (kept(&unlocked), unlocked.encryption().clone());
        assert_eq!(after, before, "a reading opened already is as it was");
        return;
    }
    // The owner's password has one encoding, so its index is the
    // password's.
    let by_owner = Reading::open_with(file, &[OWNER], &options).expect(in_memory);
    for &id in &ids {
        let got = |reading: &Reading<&[u8]>| match reading.get(id) {
            Ok(object) => Ok((object.clone(), reading.record(id).cloned())),
            Err(why) => Err(why.to_string()),
        };
        assert_eq!(got(&unlocked), got(&by_owner), "{id:?} unlocked");
    }
    assert_eq!(unlocked.encryption(), by_owner.encryption(), "unlocked");
    assert_eq!(
        unlocked.warnings(),
        by_owner.warnings(),
        "what's said unlocked"
    );
    let documents = [unlocked, by_owner].map(|reading| reading.into_document().expect(in_memory));
    assert_eq!(documents[0], documents[1], "a reading unlocked, read whole");
}

/// What `reading` keeps: each object's record, by number, and its
/// warnings.
fn kept<S: ByteSource>(
    reading: &Reading<S>,
) -> (
    Vec<(ObjectId, crate::source::Record)>,
    Vec<crate::source::Warning>,
) {
    let records = (reading.ids())
        .filter_map(|id| Some((id, reading.record(id)?.clone())))
        .collect();
    (records, reading.warnings())
}

/// Checks that `doc`, pruned, reaches what it reached from its trailer, in
/// the same order, every id it keeps among it and none it took; and that
/// two objects added to it, or to it pruned, are each at a number no entry
/// held, neither 0, and not the same.
fn pruned(doc: &Document) {
    let reached = doc.reachable();
    let mut pruned = doc.clone();
    let gone = pruned.prune();
    assert_eq!(
        pruned.reachable(),
        reached,
        "what's reached is the same pruned"
    );
    let reached: HashSet<ObjectId> = reached.into_iter().collect();
    let ids = pruned.ids();
    assert!(
        ids.iter().all(|id| reached.contains(id)),
        "each id kept is reached"
    );
    assert_eq!(
        ids.len() + gone.len(),
        doc.ids().len(),
        "each id kept or taken"
    );
    assert!(
        gone.iter().all(|id| !reached.contains(id)),
        "each id taken unreached"
    );
    for mut doc in [doc.clone(), pruned] {
        let held: HashSet<u32> = doc.ids().into_iter().map(|(n, _)| n).collect();
        let (one, two) = (doc.add(Object::null()), doc.add(Object::null()));
        for id in [one, two] {
            assert!(
                id.0 != 0 && id.1 == 0 && !held.contains(&id.0),
                "{id:?} added"
            );
        }
        assert_ne!(one, two, "two objects added at one id");
    }
}

/// Checks that every place `doc` gives is inside what was read, the file
/// or the decoded data of the object stream it names: each object's, and
/// each value's and key's in it, each stream's data's, the trailer's, each
/// revision's trailer's, and each warning's.
fn within(file: &[u8], doc: &Document) {
    let inside = |at: Span, of: Option<ObjectId>| {
        let len = match at.stream {
            None => Some(file.len()),
            Some(stream) => doc.decoded.get(&stream).map(Vec::len),
        };
        assert!(
            len.is_some_and(|len| at.start <= at.end && at.end <= len),
            "{at:?}, in {of:?}, is out of what was read"
        );
    };
    let objects = doc.objects().map(|(id, o)| (Some(id), o));
    let trailer = (doc.trailer.stream, &doc.trailer.object);
    let revisions = (doc.revisions.iter()).map(|r| (r.trailer.stream, &r.trailer.object));
    for (of, object) in objects.chain([trailer]).chain(revisions) {
        let mut stack = vec![object];
        while let Some(o) = stack.pop() {
            if let Some(at) = o.at {
                inside(at, of);
            }
            let dict = match o.value() {
                Value::Array(a) => {
                    stack.extend(a);
                    continue;
                }
                Value::Dict(d) => d,
                Value::Stream(s) => {
                    if let Some(at) = s.data.at() {
                        inside(at, of);
                    }
                    &s.dict
                }
                _ => continue,
            };
            for entry in dict.entries() {
                if let Some(at) = entry.key_at {
                    inside(at, of);
                }
                stack.push(&entry.value);
            }
        }
    }
    for warning in &doc.warnings {
        if let Some(at) = warning.at {
            inside(at, warning.object);
        }
    }
}

/// The password a file the `write` target writes under a new encryption
/// opens by.
const NEW: Password<'static> = Password("new");

/// The most object numbers a file the `write` target writes whole lists:
/// a whole file's section lists every number up to its last, so one past
/// it would take a run its time.
const LISTED: u32 = 1 << 16;

/// The `write` target's body: `data`'s first two bytes choose how the
/// file after them is read and written, its third how a whole file of it
/// and an update after it are renumbered ([`Renumbering`]) and, by its top
/// bit, whether the document is read whole and taken by the reading
/// ([`adopted`]), and the rest is the file. Read under
/// [`options`], by the owner's password `owner` when the choice's lowest
/// bit is set, made a document with its reading kept (or read whole and
/// taken by that reading, which reads it alike), then written whole,
/// but where it would list a number past [`LISTED`]: with a table, a
/// stream, or a stream packed, by number or in sevens on one thread, as
/// the choice's next two bits give; encrypted as it was read, decrypted,
/// or under a new encryption of revision 6 for `new` whose randomness the
/// file's bytes give ([`Drawn`]), as its two after give; under the
/// renumbering; each object as read ([`write::bodies`]), so copied where
/// the rules for a copy hold.
/// And, when its sixth bit is set, an update after it, of the document's
/// objects, the one its bits from the seventh pick changed, a key more,
/// under its own encryption and the renumbering, kept off what an update
/// leaves where it is ([`Renumbering::of_update`]): it writes that one and
/// what the renumbering moves or names otherwise alone. Else, a reading of
/// the file by position ([`Flipped`]), made a document, a byte of an
/// object read at an offset flipped after, the one those bits pick among
/// their bytes but their streams' data, written whole as before. Neither
/// panics. Where the reading read soundly, with nothing to warn of, and a
/// write wrote, its file read again under the same limits, by its
/// password, is sound and holds every object the document did, at the
/// number the renumbering gives it, its references as it names them, as
/// pdf-objects' writer writes each (reals as written and strings' forms
/// aside), a stream's data as handed out, but the encryption dictionary
/// and what it refers to, which the writer writes as its encryption is;
/// and the update's changed object with its key, at its number; and the
/// write after the byte flipped is refused as changed, of an object whose
/// bytes hold it, or it wrote what was read. Panics on a broken promise.
pub fn write_file(data: &[u8]) {
    compared(data);
}

/// How the `write` target renumbers a whole file, and an update after it
/// ([`Renumbering::of_update`]), as its input's third byte, but its top
/// bit (which says whether the document written is read whole and taken by
/// the reading, [`adopted`]), gives it, taken by five: none; each object at its
/// own number, given; the document's numbers reversed, each object at the
/// number of the one as far from the end as it is from the start; each
/// past every number the file has; or numbered from 1 in order, passing
/// over the numbers the encryptor keeps free (a dense numbering, two
/// objects never sharing one, as `document` gives every object). A
/// reference to a number the document doesn't hold is written null.
struct Renumbering(Option<BTreeMap<ObjectId, ObjectId>>);

impl Renumbering {
    fn of(byte: u8, doc: &Document, encryptor: Option<&Encryptor>) -> Renumbering {
        let ids: Vec<ObjectId> = doc.objects().map(|(id, _)| id).collect();
        let free: BTreeSet<u32> = (encryptor.into_iter())
            .flat_map(|e| e.free_numbers().map(|(n, _)| n))
            .collect();
        let map = match (byte & 0x7f) % 5 {
            1 => ids.iter().map(|&id| (id, id)).collect(),
            2 => {
                let numbers = ids.iter().rev().map(|id| id.0);
                (ids.iter().zip(numbers))
                    .map(|(&id, n)| (id, (n, id.1)))
                    .collect()
            }
            3 => {
                let top = (ids.iter().map(|id| id.0))
                    .chain(free.iter().copied())
                    .max();
                let past = top.unwrap_or(0).saturating_add(1);
                (ids.iter())
                    .map(|&(n, g)| ((n, g), (n.saturating_add(past), g)))
                    .collect()
            }
            4 => {
                let mut next = 1u32;
                let mut map = BTreeMap::new();
                for &id in &ids {
                    while free.contains(&next) {
                        next += 1;
                    }
                    map.insert(id, (next, 0));
                    next += 1;
                }
                map
            }
            _ => return Renumbering(None),
        };
        Renumbering(Some(map))
    }

    /// Where an object given at `id` is written, and what a reference to it
    /// names.
    fn at(&self, id: ObjectId) -> Option<ObjectId> {
        match &self.0 {
            None => Some(id),
            Some(map) => map.get(&id).copied(),
        }
    }

    /// How the `write` target renumbers an update after `doc`'s file, as
    /// `byte` gives a whole file's ([`Renumbering::of`]), but what an update
    /// leaves where it is: what `encryptor` leaves in the clear by number,
    /// an encryption dictionary and what one refers to, and what a whole
    /// file leaves out (object streams, whose members the file may still
    /// read through them, cross-reference streams, a linearization
    /// dictionary), each at its own number; another object the whole
    /// file's would put at one of their numbers goes past every number the
    /// file and the rest take, from `size`, the file's.
    fn of_update(
        byte: u8,
        doc: &Document,
        encryptor: Option<&Encryptor>,
        size: u32,
    ) -> Renumbering {
        let Renumbering(Some(mut map)) = Renumbering::of(byte, doc, encryptor) else {
            return Renumbering(None);
        };
        let left_out = |o: &Object| match o.value() {
            Value::Stream(s) => s.dict.has_type(b"ObjStm") || s.dict.has_type(b"XRef"),
            Value::Dict(d) => d.has(b"Linearized"),
            _ => false,
        };
        let mut stays: BTreeSet<ObjectId> = (doc.objects())
            .filter(|&(id, o)| left_out(o) || encryptor.is_some_and(|e| e.dictionary_object(id)))
            .map(|(id, _)| id)
            .collect();
        if let Some(e) = encryptor {
            stays.extend(e.at());
            stays.extend(e.dictionary().iter().flat_map(|(_, v)| v.references()));
        }
        let numbers: BTreeSet<u32> = stays.iter().map(|id| id.0).collect();
        let mut next = (map.values().map(|at| at.0.saturating_add(1)))
            .chain([size])
            .max()
            .unwrap_or(size);
        for (id, at) in &mut map {
            if stays.contains(id) {
                *at = *id;
            } else if numbers.contains(&at.0) {
                *at = (next, at.1);
                next = next.saturating_add(1);
            }
        }
        for &id in &stays {
            map.insert(id, id);
        }
        Renumbering(Some(map))
    }

    /// The last number a whole file of `doc` writes an object at, under it.
    fn last(&self, doc: &Document) -> u32 {
        (doc.objects().filter_map(|(id, _)| self.at(id)))
            .map(|id| id.0)
            .max()
            .unwrap_or(0)
    }
}

/// [`write_file`]'s body: whether the whole file, the update and the whole
/// file after a byte was flipped were written, or refused, and compared.
fn compared(data: &[u8]) -> [bool; 3] {
    let (choice, byte, file) = match data {
        [a, b, c, file @ ..] => (u16::from_le_bytes([*a, *b]), *c, file),
        _ => (0, 0, data),
    };
    let password = match choice & 1 {
        0 => Password(""),
        _ => OWNER,
    };
    let Ok(reading) = Reading::open_with(file, &[password], &options()) else {
        return [false; 3];
    };
    let Ok(mut doc) = reading.to_document() else {
        return [false; 3];
    };
    let sound = reading.trailer().unwritten.is_none() && reading.warnings().is_empty();
    if byte & 0x80 != 0 {
        doc = adopted(file, password, &reading, doc, sound);
    }
    let mut options = write::Options::DEFAULT;
    let sevens = |numbers: &[u32]| numbers.chunks(7).map(<[u32]>::to_vec).collect();
    let mut sevens_on_one = Pack::DEFAULT;
    sevens_on_one.groups = Some(&sevens);
    (options.kind, options.pack) = match (choice >> 1) & 3 {
        0 => (XrefKind::Table, None),
        1 => (XrefKind::Stream, None),
        2 => (XrefKind::Stream, Some(Pack::DEFAULT)),
        _ => {
            options.threads = 1;
            (XrefKind::Stream, Some(sevens_on_one))
        }
    };
    let own = Encryptor::of(&reading);
    let way = Way::of(file, choice);
    let (encryptor, opens) = way.encryptor(own.as_ref().ok().and_then(Option::as_ref), password);
    let renumbering = Renumbering::of(byte, &doc, encryptor);
    let to = |id| renumbering.at(id);
    if renumbering.0.is_some() {
        options.renumber = Some(&to);
    }
    let last = doc.objects().next_back().map_or(0, |(id, _)| id.0);
    let listed = last < LISTED && renumbering.last(&doc) < LISTED;
    let mut compared = [false; 3];
    if listed {
        let mut out = Vec::new();
        let written = write::document(&mut out, &doc, encryptor, Some(&reading), &options);
        if let (true, Ok(written)) = (sound, written) {
            assert_eq!(written.base.renumbered, renumbering.0.is_some());
            let back = read_back(&out, opens);
            same_objects(&doc, &reading, &back, None, &to);
            compared[0] = true;
        }
    }
    if choice & 32 != 0 {
        let own = own.ok().flatten();
        let how = (choice, byte);
        compared[1] = updated(file, &doc, &reading, own.as_ref(), password, sound, how);
    } else if listed && sound {
        compared[2] = flipped(file, password, &options, &way, choice >> 7, &to);
    }
    compared
}

/// `file` read whole by `password`, by a reading of its own that's gone
/// after, then taken by `reading` ([`Reading::adopt`]), a reading of it
/// kept, of which `doc` was made, so that its objects as read are written
/// through `reading` as `doc`'s are. Where `reading` read soundly, it's
/// taken, each record `reading`'s, and it's `doc`; where it isn't taken,
/// it's left as it was, its records their own reading's. The document
/// taken, or else `doc`.
fn adopted(
    file: &[u8],
    password: Password<'_>,
    reading: &Reading<&[u8]>,
    doc: Document,
    sound: bool,
) -> Document {
    let whole = Reading::open_with(file, &[password], &options()).and_then(Reading::into_document);
    let Ok(mut whole) = whole else {
        return doc;
    };
    let sources = |d: &Document| -> Vec<_> { d.records().map(|(id, r)| (id, r.source)).collect() };
    let (was, by) = (whole.clone(), sources(&whole));
    match reading.adopt(&mut whole) {
        Ok(()) => {
            assert!(
                whole.records().all(|(_, r)| r.source == reading.id()),
                "each record taken is the reading's"
            );
            if sound {
                assert_eq!(whole, doc, "the document taken is the reading's own");
            }
            whole
        }
        Err(error) => {
            assert!(
                !sound,
                "a reading read soundly takes its file's document: {error}"
            );
            assert_eq!(whole, was, "a document not taken is as it was");
            assert_eq!(sources(&whole), by, "its records their own reading's");
            doc
        }
    }
}

/// How the `write` target encrypts a whole file, as the choice's bits
/// from the fourth say: as it was read, decrypted, or under a new
/// encryption of revision 6 for `new`, its randomness the file's bytes,
/// the document's metadata encrypted as the seventh bit says.
struct Way {
    /// 0 and 1 as read, 2 decrypted, 3 a new encryption.
    choice: u16,
    new: Option<Encryptor>,
}

impl Way {
    fn of(file: &[u8], choice: u16) -> Way {
        let new = NEW.prepared().ok().and_then(|password| {
            Encryptor::create(&password, choice & 64 == 0, &mut Drawn::over(file)).ok()
        });
        Way {
            choice: (choice >> 3) & 3,
            new,
        }
    }

    /// Its encryptor, `own` as read, and the password the file written
    /// opens by, `password` where it's encrypted as read.
    fn encryptor<'e, 'p>(
        &'e self,
        own: Option<&'e Encryptor>,
        password: Password<'p>,
    ) -> (Option<&'e Encryptor>, Password<'p>) {
        match self.choice {
            2 => (None, Password("")),
            3 => (self.new.as_ref(), NEW),
            _ => (own, password),
        }
    }
}

/// `file`, read again by position by `password` through [`Flipped`], read
/// soundly and made a document; then a byte of an object it read at an
/// offset flipped, the one `pick` picks among those objects' bytes but
/// their streams' data, and the document written whole as `options` and
/// `way` say: refused as changed, of an object whose bytes hold the byte
/// flipped; or, where it wrote, read again the same objects as the
/// document, renumbered as `to` names them, which is `options`'
/// renumbering. Whether the write was compared so.
fn flipped(
    file: &[u8],
    password: Password<'_>,
    options: &write::Options<'_>,
    way: &Way,
    pick: u16,
    to: &dyn Fn(ObjectId) -> Option<ObjectId>,
) -> bool {
    let source = Flipped::new(file, 64);
    let Ok(reading) = Reading::open_with(&source, &[password], &self::options()) else {
        return false;
    };
    let Ok(doc) = reading.to_document() else {
        return false;
    };
    if reading.trailer().unwritten.is_some() || !reading.warnings().is_empty() {
        return false;
    }
    // Each object's bytes read at an offset, but its stream's data.
    let mut spans: Vec<Range<usize>> = Vec::new();
    for (_, record) in doc.records().filter(|(_, r)| r.origin == Origin::File) {
        let at = record.at;
        match record.data {
            Some(data) => spans.extend([at.start..data.at.start, data.at.end..at.end]),
            None => spans.push(at.start..at.end),
        }
    }
    let total: usize = spans.iter().map(Range::len).sum();
    if total == 0 {
        return false;
    }
    let mut left = usize::from(pick) % total;
    let Some(at) = spans.iter().find_map(|span| match left < span.len() {
        true => Some(span.start + left),
        false => {
            left -= span.len();
            None
        }
    }) else {
        return false;
    };
    source.flip(at);
    let own = Encryptor::of(&reading).ok().flatten();
    let (encryptor, opens) = way.encryptor(own.as_ref(), password);
    let mut out = Vec::new();
    match write::document(&mut out, &doc, encryptor, Some(&reading), options) {
        Ok(_) => {
            let back = read_back(&out, opens);
            same_objects(&doc, &reading, &back, None, to);
        }
        Err(write::Error::Changed(Some(id))) => {
            let record = doc.record(id).expect("a copy's record");
            let held = (record.at.start..record.at.end).contains(&at);
            assert!(held, "{id:?}'s bytes hold the byte flipped, {at}");
        }
        Err(_) => {}
    }
    true
}

/// `file`, read as `reading` and made `doc`, with an update after it of
/// `doc`'s objects as read ([`write::bodies`]), but the one the choice's
/// bits from the seventh pick, changed, with a key more, under
/// `encryptor`, its own, and under the renumbering the input's third byte
/// gives ([`Renumbering::of_update`]): it writes that one, and what the
/// renumbering moves or names otherwise, passing over the rest, which the
/// file holds. When the reading was `sound` and the update wrote, the file
/// read again by `password` is sound, and the same objects, each at the
/// number the renumbering gives it, but that one with its key: whether it
/// was.
fn updated(
    file: &[u8],
    doc: &Document,
    reading: &Reading<&[u8]>,
    encryptor: Option<&Encryptor>,
    password: Password,
    sound: bool,
    (choice, byte): (u16, u8),
) -> bool {
    let Ok(base) = Base::of(reading) else {
        return false;
    };
    let objects: Vec<(ObjectId, &Object)> = doc.objects().collect();
    let Some(&(id, object)) = objects.get(usize::from(choice >> 7) % objects.len().max(1)) else {
        return false;
    };
    let mut changed = object.clone();
    let Some(dict) = changed.as_dict_mut() else {
        return false;
    };
    dict.set("WriteFuzzed", true);
    let renumbering = Renumbering::of_update(byte, doc, encryptor, base.size);
    let to = |id| renumbering.at(id);
    let mut options = write::Options::DEFAULT;
    if renumbering.0.is_some() {
        options.renumber = Some(&to);
    }
    let mut out = file.to_vec();
    let given = write::bodies(doc).map(|(at, body)| match at == id {
        true => (at, Body::from(&changed)),
        false => (at, body),
    });
    let written = write::update(&mut out, &base, given, encryptor, Some(reading), &options);
    if let (true, Ok(Some(written))) = (sound, written) {
        assert!(
            written.base.holds(&out[..]).expect("in memory"),
            "the update's base holds"
        );
        assert_eq!(written.base.renumbered, renumbering.0.is_some());
        for (at, object) in doc.objects() {
            let moves = to(at) != Some(at) || object.references().any(|r| to(r) != Some(r));
            let listed = written.section.lists(to(at).expect("numbered").0);
            assert_eq!(
                listed,
                at == id || moves,
                "{at:?} written alone of what's read"
            );
        }
        let back = read_back(&out, password);
        same_objects(doc, reading, &back, Some(id), &to);
        let at = to(id).expect("numbered");
        let got = back.get(at).expect("the changed object reads");
        let key = got.as_dict().and_then(|d| d.get(b"WriteFuzzed"));
        assert_eq!(key.and_then(Object::as_bool), Some(true), "{id:?} changed");
        return true;
    }
    false
}

/// `file`, written, read again under [`options`] by `password`: sound.
fn read_back<'f>(file: &'f [u8], password: Password<'_>) -> Reading<&'f [u8]> {
    let back = Reading::open_with(file, &[password], &options()).expect("in memory");
    if back.trailer().unwritten.is_none() {
        let _ = back.to_document();
    }
    assert!(
        back.trailer().unwritten.is_none() && back.warnings().is_empty(),
        "a written file reads soundly: {:?} {:?}",
        back.trailer().unwritten,
        back.warnings()
    );
    back
}

/// `back` holds every object `doc`, read in `reading`, holds, but
/// `changed`, a whole file's left-out ones, and the encryption dictionary
/// and what it refers to, at the number `to` gives it, its references as
/// `to` names them: each as pdf-objects' writer writes it, a stream's
/// dictionary but its `/Length`, and its data as each reading hands it
/// out.
fn same_objects<S: ByteSource>(
    doc: &Document,
    reading: &Reading<S>,
    back: &Reading<&[u8]>,
    changed: Option<ObjectId>,
    to: &dyn Fn(ObjectId) -> Option<ObjectId>,
) {
    let encrypt = doc.trailer.dict().get(b"Encrypt");
    let mut encryption: HashSet<ObjectId> = encrypt
        .map(|e| e.references().collect())
        .unwrap_or_default();
    let mut walk: Vec<ObjectId> = encryption.iter().copied().collect();
    while let Some(id) = walk.pop() {
        for at in doc.get(id).ok().into_iter().flat_map(Object::references) {
            if encryption.insert(at) {
                walk.push(at);
            }
        }
    }
    for (id, object) in doc.objects() {
        let left_out = match object.value() {
            Value::Stream(s) => s.dict.has_type(b"ObjStm") || s.dict.has_type(b"XRef"),
            Value::Dict(d) => d.has(b"Linearized"),
            _ => false,
        };
        if left_out || changed == Some(id) || encryption.contains(&id) {
            continue;
        }
        let at = to(id).expect("a number for each object written");
        let got = back
            .get(at)
            .unwrap_or_else(|e| panic!("{id:?} reads back at {at:?}: {e}"));
        let mut renumbered = Cow::Borrowed(object);
        if at != id || object.references().any(|r| to(r) != Some(r)) {
            renumbered.to_mut().renumber(to);
        }
        assert_eq!(
            written(got),
            written(&renumbered),
            "{id:?} reads back at {at:?} as it was"
        );
        if object.as_stream().is_some() {
            let data =
                |r: Result<Cow<'_, [u8]>, Error>| r.map(Cow::into_owned).map_err(|e| e.to_string());
            assert_eq!(
                data(back.data(at)),
                data(reading.data(id)),
                "{id:?}'s data reads back at {at:?} as it was"
            );
        }
    }
}

/// `object` as pdf-objects' writer writes it, a stream's dictionary alone,
/// but its `/Length`.
fn written(object: &Object) -> Vec<u8> {
    let object = match object.value() {
        Value::Stream(stream) => {
            let mut dict = stream.dict.clone();
            dict.remove(b"Length");
            Object::from(dict)
        }
        _ => object.clone(),
    };
    let mut out = Vec::new();
    let _ = serialize::write(&mut out, &object);
    out
}

/// A random number generator for the `write` target's body alone: the
/// input's bytes, then a counter's. It's no cryptographic generator, but
/// says it is (`TryCryptoRng`) so that pdf-crypt's `Handler::create` takes
/// it, through `Encryptor::create`, which only a fuzz body and the tests
/// may do: it's compiled for nothing else.
pub struct Drawn<'a> {
    data: &'a [u8],
    counter: u8,
}

impl Drawn<'_> {
    /// `data`'s bytes, then a counter's.
    pub fn over(data: &[u8]) -> Drawn<'_> {
        Drawn { data, counter: 0 }
    }
}

impl TryRng for Drawn<'_> {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        let mut bytes = [0; 4];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        let mut bytes = [0; 8];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
        for byte in dst {
            *byte = match self.data.split_first() {
                Some((&first, rest)) => {
                    self.data = rest;
                    first
                }
                None => {
                    self.counter = self.counter.wrapping_add(1);
                    self.counter
                }
            };
        }
        Ok(())
    }
}

impl TryCryptoRng for Drawn<'_> {}

#[cfg(test)]
mod tests {
    use pdf_crypt::writer::{Kind, Settings};
    use pdf_objects::dict;
    use pdf_objects::object::{Dict, Object};

    use super::*;
    use crate::bytes;
    use crate::fixture::{self, Body, Xref};

    /// A small, fast generator of bytes, the same each run.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % n.max(1) as u64).unwrap_or(0)
        }
    }

    fn numbers(n: &[i64]) -> Object {
        n.iter()
            .map(|&n| Object::from(n))
            .collect::<Vec<_>>()
            .into()
    }

    fn root() -> Dict {
        dict! { "Root" => Object::reference(1) }
    }

    /// A catalog, its page tree, a page with an annotation, and the
    /// annotation's appearance stream.
    fn objects() -> Vec<(u32, Body)> {
        vec![
            (
                1,
                dict! {
                    "Type" => "Catalog",
                    "Pages" => Object::reference(2),
                    "Lang" => Object::string("en"),
                }
                .into(),
            ),
            (
                2,
                dict! { "Type" => "Pages", "Kids" => vec![Object::reference(3)], "Count" => 1 }
                    .into(),
            ),
            (
                3,
                dict! {
                    "Type" => "Page",
                    "Parent" => Object::reference(2),
                    "MediaBox" => numbers(&[0, 0, 612, 792]),
                    "Annots" => vec![Object::reference(4)],
                }
                .into(),
            ),
            (
                4,
                dict! {
                    "Type" => "Annot",
                    "Subtype" => "Text",
                    "Rect" => numbers(&[10, 10, 30, 30]),
                    "Contents" => Object::string("A note"),
                    "AP" => dict! { "N" => Object::reference(5) },
                }
                .into(),
            ),
            (
                5,
                Body::stream(
                    dict! {
                        "Type" => "XObject",
                        "Subtype" => "Form",
                        "BBox" => numbers(&[0, 0, 20, 20]),
                    },
                    b"0 0 m 20 20 l S",
                ),
            ),
        ]
    }

    /// [`objects`] with a table, then an incremental update that rewrites
    /// the page with a second annotation, and adds it.
    fn updated() -> Vec<u8> {
        let mut file = fixture::file("1.7", &objects(), root(), Xref::Table);
        let text = String::from_utf8_lossy(&file);
        let previous: usize = (text.rsplit("startxref\n").next())
            .and_then(|tail| tail.lines().next()?.parse().ok())
            .expect("the writer's startxref");
        let page = file.len();
        file.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
              /Annots [4 0 R 6 0 R] >>\nendobj\n",
        );
        let six = file.len();
        file.extend_from_slice(
            b"6 0 obj\n<< /Type /Annot /Subtype /Square /Rect [0 0 1 1] /IC [1 0 0] >>\nendobj\n",
        );
        let xref = file.len();
        file.extend_from_slice(
            format!(
                "xref\n3 1\n{page:010} 00000 n \n6 1\n{six:010} 00000 n \n\
                 trailer\n<< /Size 7 /Root 1 0 R /Prev {previous} >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        file
    }

    /// A file whose page tree and page are in object stream 4, compressed,
    /// which cross-reference stream 5 lists: objects read from, and placed
    /// in, the stream's decoded data.
    fn in_an_object_stream() -> Vec<u8> {
        let pages = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
        let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents (A page) >>";
        let header = format!("2 0 3 {} ", pages.len() + 1);
        let data = fixture::zlib(format!("{header}{pages} {page}").as_bytes());
        let mut file = b"%PDF-1.7\n".to_vec();
        let one = file.len();
        file.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let four = file.len();
        let dict = format!(
            "<< /Type /ObjStm /N 2 /First {} /Filter /FlateDecode /Length {} >>",
            header.len(),
            data.len()
        );
        file.extend_from_slice(format!("4 0 obj\n{dict}\nstream\n").as_bytes());
        file.extend_from_slice(&data);
        file.extend_from_slice(b"\nendstream\nendobj\n");
        let five = file.len();
        let row = |kind: u8, field: usize, index: u8| {
            let [high, low] = u16::try_from(field).unwrap_or(0).to_be_bytes();
            [kind, high, low, index]
        };
        let rows = [
            row(0, 0, 255),
            row(1, one, 0),
            row(2, 4, 0),
            row(2, 4, 1),
            row(1, four, 0),
            row(1, five, 0),
        ]
        .concat();
        let dict = format!(
            "<< /Type /XRef /Size 6 /W [1 2 1] /Root 1 0 R /Length {} >>",
            rows.len()
        );
        file.extend_from_slice(format!("5 0 obj\n{dict}\nstream\n").as_bytes());
        file.extend_from_slice(&rows);
        file.extend_from_slice(
            format!("\nendstream\nendobj\nstartxref\n{five}\n%%EOF\n").as_bytes(),
        );
        file
    }

    /// Files to mutate: [`objects`] written with a table and with a
    /// cross-reference stream, and encrypted by each kind of pdf-crypt's
    /// writer, the user's password empty and the owner's `owner`; the
    /// update; and the object stream.
    fn files() -> Vec<Vec<u8>> {
        let mut files = vec![
            fixture::file("1.7", &objects(), root(), Xref::Table),
            fixture::file("1.7", &objects(), root(), Xref::Stream),
        ];
        let kinds = [
            Kind::Rc4By40,
            Kind::Rc4By128,
            Kind::Aes128,
            Kind::Aes256R5,
            Kind::Aes256,
        ];
        for kind in kinds {
            let settings = Settings {
                kind,
                user: b"",
                owner: OWNER.0.as_bytes(),
                p: -4,
                id: b"0123456789abcdef",
                encrypt_metadata: true,
            };
            files.push(fixture::encrypted(&settings, &objects(), root()));
        }
        files.push(updated());
        files.push(in_an_object_stream());
        files
    }

    /// The `write` target's checks on the same files: as written, by either
    /// password, every way of writing each whole and as an update writes and
    /// reads back the same objects, its document made beside its reading or
    /// read whole and taken by it, and so does each way of writing it whole
    /// after a byte is flipped, or it's refused as changed, the byte
    /// flipped at places spread over its objects; then each mutated many
    /// times, as a fuzzer might, without a panic, the same where it still
    /// reads soundly.
    #[test]
    fn mutated_files_write_and_read_back_alike() {
        let files = files();
        for file in &files {
            for choice in 0..64u16 {
                // Each renumbering in turn, spread over the choices, and
                // the document taken by the reading in every other turn.
                let taken = |turn: u16| u8::try_from(turn / 5 % 2).unwrap_or(0) << 7;
                let renumbered = [u8::try_from(choice % 5).unwrap_or(0) | taken(choice)];
                let updated = choice | 32 | (choice & 1) << 7;
                let data = [&updated.to_le_bytes()[..], &renumbered, file].concat();
                assert_eq!(
                    compared(&data),
                    [true, true, false],
                    "{updated}, {renumbered:?}"
                );
                let turn = choice + 2;
                let renumbered = [u8::try_from(turn % 5).unwrap_or(0) | taken(turn)];
                let pick = choice.wrapping_mul(0x9e3) & 0x1ff;
                let flipped = choice & !32 | pick << 7;
                let data = [&flipped.to_le_bytes()[..], &renumbered, file].concat();
                assert_eq!(
                    compared(&data),
                    [true, false, true],
                    "{flipped}, {renumbered:?}"
                );
            }
        }
        let mut random = Random(0x6a09_e667_f3bc_c908);
        for n in 0..400 {
            let choice = u16::try_from(random.next() & 0xffff).unwrap_or(0);
            let renumbered = [random.next() as u8];
            let mut file = [
                &choice.to_le_bytes()[..],
                &renumbered,
                &files[n % files.len()],
            ]
            .concat();
            for _ in 0..1 + random.below(4) {
                let at = 3 + random.below(file.len() - 3);
                match random.below(3) {
                    0 => file[at] = random.next() as u8,
                    1 => {
                        let end = (at + random.below(16)).min(file.len());
                        file.drain(at..end);
                    }
                    _ => {
                        let end = (at + random.below(64)).min(file.len());
                        let piece = file[at..end].to_vec();
                        file.splice(at..at, piece);
                    }
                }
                if file.len() < 4 {
                    break;
                }
            }
            write_file(&file);
        }
    }

    /// The fuzz target's checks on files the tests' writer makes, each
    /// mutated many times as a fuzzer might: bytes changed, cut, repeated,
    /// and moved, each file read by the empty password and, in turn, by
    /// the owner's too, and made a document with its reading kept at a
    /// point chosen at random.
    #[test]
    fn mutated_files_read_within_their_bytes_the_same_twice() {
        // As written, each reads without a warning, an encrypted one opens
        // by either password, and the update and the object stream are
        // read as one.
        let files = files();
        let mut encrypted = 0;
        for file in &files {
            let doc = bytes::read(file);
            assert!(doc.trailer.unwritten.is_none(), "{:?}", doc.trailer);
            assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
            if doc.encryption != Encryption::None {
                encrypted += 1;
                assert_eq!(doc.encryption, Encryption::opened(false));
                let opened = bytes::read_opened(file, &[OWNER]).encryption;
                let by = Some((0, pdf_crypt::Encoding::Utf8));
                assert_eq!(opened, Encryption::opened(true).by(by));
                // A wrong password leaves it locked.
                let locked = bytes::read_opened(file, &WRONG[..1]).encryption;
                assert_eq!(locked, Encryption::Locked);
            }
            read_file(&[&[1, 0][..], file].concat());
        }
        assert_eq!(encrypted, 5, "a file of each kind");
        let doc = bytes::read(&updated());
        assert!(doc.update.is_some(), "the update is one");
        let doc = bytes::read(&in_an_object_stream());
        let at = doc.get((3, 0)).map(|o| o.at.and_then(|at| at.stream));
        assert_eq!(at, Ok(Some((4, 0))), "the page is in the object stream");

        let mut random = Random(0x9e37_79b9_7f4a_7c15);
        let fragments: &[&[u8]] = &[
            b"<<",
            b">>",
            b"[",
            b"]",
            b"(",
            b")",
            b"/",
            b" ",
            b"\n",
            b"\r",
            b"0",
            b"9",
            b"-1",
            b"R",
            b"obj",
            b"endobj",
            b"stream",
            b"endstream",
            b"null",
            b"/Kids",
            b"/Parent",
            b"/Type",
            b"/Subtype",
            b"/Length",
            b"/Prev",
            b"1 0 R",
            b"%%EOF",
            b"xref",
            b"trailer",
            b"startxref",
            b"\xff",
        ];
        // Where a document is made with its reading kept, apart from the
        // mutations, which are as they were.
        let mut points = Random(0x2545_f491_4f6c_dd1d);
        for n in 0..2000 {
            // Each file in turn, the choice's lowest bit set every other
            // round: by the empty password alone, then the owner's too.
            let choice = u16::try_from(n / files.len()).unwrap_or(0);
            let choice = choice | u16::try_from(points.below(4)).unwrap_or(0) << 10;
            let mut file = [&choice.to_le_bytes()[..], &files[n % files.len()]].concat();
            for _ in 0..1 + random.below(6) {
                let at = 2 + random.below(file.len() - 2);
                match random.below(5) {
                    0 => file[at] = random.next() as u8,
                    1 => {
                        let end = (at + random.below(16)).min(file.len());
                        file.drain(at..end);
                    }
                    2 => {
                        let fragment = fragments[random.below(fragments.len())];
                        file.splice(at..at, fragment.iter().copied());
                    }
                    3 => {
                        let end = (at + random.below(64)).min(file.len());
                        let piece = file[at..end].to_vec();
                        file.splice(at..at, piece);
                    }
                    _ => {
                        let to = 2 + random.below(file.len() - 2);
                        let end = (at + random.below(32)).min(file.len());
                        let piece: Vec<u8> = file.drain(at..end).collect();
                        let to = to.min(file.len());
                        file.splice(to..to, piece);
                    }
                }
                if file.len() < 3 {
                    break;
                }
            }
            read_file(&file);
        }
    }
}
