//! A file as a reader read it: a [`Source`], which any reader can
//! implement, and [`Document`], an owned one that readers fill, tests
//! build, and a caller builds and edits in memory; and what the bytes
//! reader records of where it read each object ([`Record`]) and of the
//! file as written ([`Facts`]), its revisions among it ([`Revision`]).

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashSet, btree_map};
use std::fmt;
use std::hash::{BuildHasher, RandomState};
use std::iter::FusedIterator;
use std::ops::RangeBounds;
use std::sync::LazyLock;

use pdf_crypt::{Encoding, Permissions, Refused};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::object::{Data, Dict, Entry, Object, ObjectId, Span, Str, Value};
use pdf_objects::parse;
use pdf_objects::version::Version;
use pdf_objects::{Resolve, Streams, Unreadable, Why};

use crate::read::{Error, Fault, SourceId, Undecoded, Unfiltered, Unindexed, Unwritten};

/// A file, as a reader read it: its objects, and its streams' data as a
/// viewer takes it ([`Streams`]), which a reader without the file's bytes,
/// as a document alone is, gives only of what it holds ([`Why::Detached`]
/// of the rest). A [`Document`]
/// is one, and so are a [`Reading`](crate::Reading), which answers of what
/// it has read so far, and a document read beside its reading
/// ([`WithReading`](crate::read::WithReading)).
///
/// A `Reading`'s and a `Document`'s own methods of the trait's names give
/// the same answers in a plainer or richer form (an iterator for a `Vec`,
/// the reading's [`Error`] for an [`Unread`], no `Option` where it always
/// knows, a borrow for a `Cow`), and a call on one names them first; the
/// trait's are reached by its name (`Source::facts(&doc)`) or through a
/// `&dyn Source`.
pub trait Source: Streams {
    /// Every object its cross-references list.
    #[must_use]
    fn ids(&self) -> Vec<ObjectId>;
    /// Object `id`, as written: references kept as references, strings
    /// decoded and decrypted.
    ///
    /// # Errors
    ///
    /// [`Unread`], saying why there's none: the file hasn't it, it doesn't
    /// read, or the source of the file's bytes didn't give them
    /// ([`Unread::Source`]), which asked again may.
    fn get(&self, id: ObjectId) -> Result<&Object, Unread>;
    /// The trailer.
    #[must_use]
    fn trailer(&self) -> &Trailer;
    /// The version in the header.
    #[must_use]
    fn header_version(&self) -> Option<Version>;
    /// What the reader told of while reading it, each fault at its place:
    /// repairs, duplicate keys, and anything else it had to choose how to
    /// read, in the whole read's order. Lent where the reader holds them, as
    /// a document does, or made for the call, as a reading makes them of
    /// what it has read so far
    /// ([`Reading::faults`](crate::Reading::faults)); none where they
    /// weren't looked for ([`Options::faults`](crate::Options::faults)).
    #[must_use]
    fn faults(&self) -> Cow<'_, [(Place, Fault)]>;
    /// Tells `sink` each of [`Source::faults`], in their order, while it
    /// wants them, asked before each: none where it wants none, and none
    /// after it stops. So that one sink takes the faults of several crates'
    /// readers, each its own type.
    fn tell_faults(&self, sink: &mut dyn Sink<Fault>) {
        if !sink.wants() {
            return;
        }
        match self.faults() {
            Cow::Borrowed(faults) => {
                for (place, fault) in faults {
                    if !sink.wants() {
                        return;
                    }
                    sink.fault(*place, fault.clone());
                }
            }
            Cow::Owned(faults) => {
                for (place, fault) in faults {
                    if !sink.wants() {
                        return;
                    }
                    sink.fault(place, fault);
                }
            }
        }
    }
    /// The file's length in bytes, when the reader knows it.
    #[must_use]
    fn size(&self) -> Option<usize> {
        None
    }
    /// The data of the catalog's metadata stream (ISO 32000-2, 14.3.2), as
    /// written, when the reader has it in plain view: lent where the reader
    /// holds it, as a document does, or read for the call, as a reading
    /// reads it ([`Reading::metadata`](crate::Reading::metadata)), none
    /// where the source of the file's bytes fails on the way to it.
    #[must_use]
    fn metadata(&self) -> Option<Cow<'_, [u8]>> {
        None
    }
    /// Where object `id` was read from, when the reader recorded it.
    #[must_use]
    fn record(&self, id: ObjectId) -> Option<&Record> {
        let _ = id;
        None
    }
    /// Object stream `stream`'s decoded data, which the objects read from
    /// it are placed in ([`Span::stream`]), when the reader holds it: a
    /// document holds each whose header reads ([`Document::held`]); a
    /// reading, each it has decoded so far, as a read of an object in one
    /// decodes it, and none of one it hasn't. A stream's data decoded for
    /// the asking is [`Streams::decoded`]'s.
    #[must_use]
    fn held(&self, stream: ObjectId) -> Option<&[u8]> {
        let _ = stream;
        None
    }
    /// The file's revisions, the first first, when the reader read its
    /// cross-reference sections: the last is the newest, whose trailer is
    /// [`Source::trailer`]'s. A checker tells what a later revision wrote
    /// over of an earlier one's by its [`Revision::numbers`], and reads the
    /// file as an earlier one was written from its bytes up to its
    /// [`Revision::end`]
    /// ([`Reading::prefix`](crate::read::Reading::prefix)).
    #[must_use]
    fn revisions(&self) -> &[Revision] {
        &[]
    }
    /// What the file's header and sections say of it, as written, when the
    /// reader knows. A [`Document`] gives its own ([`Document::facts`]),
    /// the defaults where its reader recorded none, as in a document built:
    /// there, [`Facts::linearized`] false says only that it isn't known to
    /// be.
    #[must_use]
    fn facts(&self) -> Option<&Facts> {
        None
    }
    /// Whether object stream `stream` leaves the strings of the objects read
    /// from it in the clear, in a file that encrypts its strings: an object
    /// in one is encrypted with the stream's data, whole, not itself, and
    /// that isn't encrypted (ISO 32000-2, 7.5.7, 7.6.2, 7.6.6). Where the
    /// reader knows: a reading, of one it has decoded so far, and false of
    /// one it hasn't, as [`Source::held`] gives none of it.
    #[must_use]
    fn in_the_clear(&self, stream: ObjectId) -> bool {
        let _ = stream;
        false
    }
    /// Stream `id`'s data decoded through its filters, at most `limit`
    /// bytes, decrypted as the reader decrypts it, or why it isn't, as
    /// [`Reading::decoded_within`](crate::read::Reading::decoded_within)
    /// gives it: the reader's own account of the decode, every fault of
    /// it, for a checker, where [`Streams::decoded`] gives what a viewer
    /// takes of it. Data past `limit` is an [`Error::Decode`] holding what
    /// decoded up to it, [`Fault::Limit`](crate::pdf_filters::Fault::Limit)
    /// among its faults, never the data cut short as if it were whole; a
    /// stream the reader can't give the data of, as it has a filter the
    /// reader doesn't take, is an error saying why ([`Error::Filters`]).
    /// None where the reader doesn't give this account of streams' data, as
    /// a document alone doesn't, its streams' data being where it was
    /// written: then none of any stream, so that a caller asking for one
    /// learns it of all; so none of a stream a document holds as bytes
    /// either, whose data it gives a viewer ([`Streams::decoded`]). A
    /// reading gives it, and so does a document read beside its reading
    /// ([`WithReading`](crate::read::WithReading)).
    #[must_use]
    fn decoded_within(&self, id: ObjectId, limit: usize) -> Option<Result<Vec<u8>, Error>> {
        let _ = (id, limit);
        None
    }
}

/// A file's trailer. A reader outside the crate starts from
/// [`Trailer::new`].
/// Not exhaustive, so that it can come to say more of the trailer.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Trailer {
    /// The cross-reference stream it is, when it is one.
    pub stream: Option<ObjectId>,
    /// The dictionary after `trailer`, or the cross-reference stream.
    pub object: Object,
    /// Why it isn't the trailer as the file's newest cross-reference
    /// section writes it, when it isn't: then it's what the reader made of
    /// the file instead, as another reader says it in its words
    /// ([`Unwritten::Other`]).
    pub unwritten: Option<Unwritten>,
}

impl Trailer {
    /// The trailer `object` is, as the file's newest cross-reference
    /// section writes it, and not a cross-reference stream: a reader that
    /// read one, or read the file otherwise, sets [`Trailer::stream`] or
    /// [`Trailer::unwritten`] on what this gives.
    #[must_use]
    pub const fn new(object: Object) -> Trailer {
        Trailer {
            stream: None,
            object,
            unwritten: None,
        }
    }

    /// Its dictionary: the trailer's, or the cross-reference stream's.
    #[must_use]
    pub fn dict(&self) -> &Dict {
        static EMPTY: Dict = Dict::EMPTY;
        self.object.as_dict().unwrap_or(&EMPTY)
    }

    /// Its dictionary, to change: the trailer's, or the cross-reference
    /// stream's. A trailer that's neither, which [`Trailer::dict`] reads as
    /// empty, is made an empty dictionary first. An edit, as through
    /// [`Object::as_dict_mut`], so its span goes.
    pub fn dict_mut(&mut self) -> &mut Dict {
        let value = self.object.value_mut();
        if !matches!(value, Value::Dict(_) | Value::Stream(_)) {
            *value = Value::Dict(Dict::new());
        }
        match value {
            Value::Dict(dict) => dict,
            Value::Stream(stream) => &mut stream.dict,
            _ => unreachable!("made a dictionary"),
        }
    }
}

/// A file read whole: its objects, trailer, header version and faults.
/// A reader outside the crate starts from [`Document::new`], and so does a
/// document built in memory.
///
/// Each object is kept with the [`Record`] of where it was read, while
/// it's as read: an object put in ([`Document::set`]) has none, and one
/// taken to edit ([`Document::get_mut`]) loses it, so a record is always
/// of the object beside it. New objects are numbered past every number
/// the file's cross-references list ([`Document::new_id`],
/// [`Document::add`]); what the trailer reaches is walked
/// ([`Document::reachable`]), and what it doesn't removed
/// ([`Document::prune`]).
///
/// What else it holds is read by its accessors: [`Document::trailer`],
/// [`Document::header_version`] and the rest, each of a name of
/// [`Source`]'s, giving the same answer as the trait in a plainer form
/// where it always knows (no `Option` of its [`Facts`], and a borrow of its
/// faults and its metadata where the trait's are a `Cow`); and the
/// document's own beside, which the trait hasn't ([`Document::update`],
/// [`Document::encryption`]). What a reader
/// outside the crate knows and fills in has a setter
/// ([`Document::trailer_mut`], [`Document::set_update`] and the rest); what
/// the bytes reader keeps of its own reading, the object streams it
/// decoded, those left in the clear and its facts, has none, as its
/// meaning is what that reader read.
///
/// Its `Debug` gives its objects as pdf-objects' values print, but each
/// string and each stream's data held in memory by its length, and the
/// object streams' decoded data and the metadata by their length too, not
/// their bytes, which may be decrypted: a test compares two by `==`, not
/// by their `Debug`.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub struct Document {
    /// Each object its cross-references list, with where it was read while
    /// it's as read, or why it doesn't read.
    objects: BTreeMap<ObjectId, Held>,
    /// Where new ids start ([`Document::new_id`]): one past every number
    /// the file's cross-references list, the trailer's `/Size` less one
    /// and every number put in since, never below 1; wider than a `u32`,
    /// so that one past 4,294,967,295 is too. From there on, it's that,
    /// `PAST`, plus the last number handed out since (or, where an object
    /// put at the largest took it there, the number new ids would have
    /// started after), 0 for none, after which the next one no entry holds
    /// is looked for.
    next: u64,
    /// The trailer.
    pub(crate) trailer: Trailer,
    /// The version in the header.
    pub(crate) header: Option<Version>,
    /// What the bytes reader told of as it read, each fault at its place,
    /// in the whole read's order; none in a document built, or read with
    /// none looked for. No caller edits it.
    pub(crate) faults: Vec<(Place, Fault)>,
    /// The object streams' decoded data, by stream, which the objects
    /// read from them are placed in ([`Span::stream`]). The record of an
    /// object read from one names its bytes here, so it says where the
    /// object was read only while this is as read. No caller edits it.
    pub(crate) decoded: BTreeMap<ObjectId, Vec<u8>>,
    /// The object streams of `decoded` whose data is whole, as a reading
    /// of the stream decodes it, and whose object is as read: decoded by
    /// the reader, decrypted cleanly and nothing cut off, and the object
    /// not put, edited, set unread or removed since, which takes it out.
    /// What the reader cut off, at a fault, the stream's limit or the
    /// budget, or didn't decrypt cleanly, is what the objects read from it
    /// are read from, not the stream's data; and an object stream put or
    /// edited may no longer be what was decoded. What
    /// [`Streams::decoded`] lends.
    pub(crate) whole: BTreeSet<ObjectId>,
    /// The file's newest incremental update, when the reader read its
    /// cross-reference sections and the newest follows one written before
    /// it.
    pub(crate) update: Option<Update>,
    /// The file's length in bytes, when the reader knows it.
    pub(crate) size: Option<usize>,
    /// The data of the catalog's metadata stream, as written, when it's in
    /// plain view: unfiltered, and not encrypted ([`Source::metadata`]).
    pub(crate) metadata: Option<Vec<u8>>,
    /// The metadata stream the reader read `metadata` from, whose data it
    /// is while that's the catalog's metadata stream and as read
    /// ([`Document::metadata`]); none where it was set
    /// ([`Document::set_metadata`]).
    pub(crate) metadata_of: Option<ObjectId>,
    /// Whether the file is encrypted, and if it is, whether it opened.
    pub(crate) encryption: Encryption,
    /// What the file's cross-reference sections and header say of it, as
    /// written, by the bytes reader; the defaults from another.
    pub(crate) facts: Facts,
    /// The file's revisions, the first first, when the reader read its
    /// cross-reference sections: the last's trailer is
    /// [`Document::trailer`] as read. Empty where the file was scanned, or
    /// its later revisions list more numbers in all than the reader reads
    /// entries ([`Limits::entries`](crate::Limits::entries)), and in a
    /// document built.
    pub(crate) revisions: Vec<Revision>,
    /// The object streams whose data an encrypted file leaves in the clear,
    /// by `/StmF` or their own `/Crypt` filter `/Identity`, while it
    /// encrypts its strings, by number: the strings of the objects read
    /// from them are in the clear (ISO 32000-2, 7.5.7, 7.6.6). The reader
    /// records each whose data it decoded, or a decoder did.
    pub(crate) in_the_clear: BTreeSet<ObjectId>,
}

impl fmt::Debug for Document {
    /// What it holds, its objects' strings and data in memory, the object
    /// streams' decoded data and the metadata by their length, not their
    /// bytes, which may be decrypted.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let objects: BTreeMap<_, _> = (self.objects.iter())
            .map(|(id, held)| {
                let held = (held.as_ref()).map(|(object, record)| (Unprinted(object), record));
                (id, held)
            })
            .collect();
        let decoded: BTreeMap<_, _> = (self.decoded.iter())
            .map(|(id, data)| (id, Len(data.len())))
            .collect();
        f.debug_struct("Document")
            .field("objects", &objects)
            .field("next", &self.next)
            .field("trailer", &self.trailer)
            .field("header", &self.header)
            .field("faults", &self.faults)
            .field("decoded", &decoded)
            .field("whole", &self.whole)
            .field("update", &self.update)
            .field("size", &self.size)
            .field(
                "metadata",
                &self.metadata.as_ref().map(|data| Len(data.len())),
            )
            .field("metadata_of", &self.metadata_of)
            .field("encryption", &self.encryption)
            .field("facts", &self.facts)
            .field("revisions", &self.revisions)
            .field("in_the_clear", &self.in_the_clear)
            .finish()
    }
}

/// Data's length, as a `Debug` gives data it doesn't print.
struct Len(usize);

impl fmt::Debug for Len {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} bytes", self.0)
    }
}

/// A value as a document's `Debug` gives it: as pdf-objects' `Debug`
/// writes it, but each string and a stream's data held in memory by its
/// length ([`Len`]), as they may be decrypted.
struct Unprinted<'v, T: ?Sized>(&'v T);

impl fmt::Debug for Unprinted<'_, Object> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Object")
            .field("value", &Unprinted(self.0.value()))
            .field("at", &self.0.at)
            .finish()
    }
}

impl fmt::Debug for Unprinted<'_, Value> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Value::String(text, form) => {
                let (kind, len) = match text {
                    Str::Bytes(bytes) => ("Bytes", bytes.len()),
                    Str::Text(text) => ("Text", text.len()),
                    Str::Encrypted(bytes) => ("Encrypted", bytes.len()),
                };
                let text = DebugTuple(kind, Len(len));
                f.debug_tuple("String").field(&text).field(form).finish()
            }
            Value::Array(items) => {
                let items: Vec<_> = items.iter().map(Unprinted).collect();
                f.debug_tuple("Array").field(&items).finish()
            }
            Value::Dict(dict) => f.debug_tuple("Dict").field(&Unprinted(dict)).finish(),
            Value::Stream(stream) => {
                let data: &dyn fmt::Debug = match &stream.data {
                    Data::Bytes(bytes) => &DebugTuple("Bytes", Len(bytes.len())),
                    data => data,
                };
                let shown = DebugStream(Unprinted(&stream.dict), data);
                f.debug_tuple("Stream").field(&shown).finish()
            }
            value => fmt::Debug::fmt(value, f),
        }
    }
}

impl fmt::Debug for Unprinted<'_, Dict> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let entries: Vec<_> = (self.0.entries().iter())
            .map(|entry| DebugEntry(entry, Unprinted(&entry.value)))
            .collect();
        f.debug_struct("Dict").field("entries", &entries).finish()
    }
}

/// A tuple variant of one field, `kind(field)`.
struct DebugTuple<T>(&'static str, T);

impl<T: fmt::Debug> fmt::Debug for DebugTuple<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple(self.0).field(&self.1).finish()
    }
}

/// A stream, its dictionary and its data as given.
struct DebugStream<'v>(Unprinted<'v, Dict>, &'v dyn fmt::Debug);

impl fmt::Debug for DebugStream<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stream")
            .field("dict", &self.0)
            .field("data", self.1)
            .finish()
    }
}

/// A dictionary's entry, its value as given.
struct DebugEntry<'v>(&'v Entry, Unprinted<'v, Object>);

impl fmt::Debug for DebugEntry<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("key", &self.0.key)
            .field("key_at", &self.0.key_at)
            .field("value", &self.1)
            .finish()
    }
}

/// Where an object was read from, which the bytes reader records when it
/// first reads it ([`crate::read::Reading::record`]), in the reading it
/// read it in; a [`Document`] keeps it with the object while the object
/// is as read ([`Document::record`]). Two records are equal when they say
/// the same of where their objects were read, whichever readings they're
/// of, so that two readings of one file are equal; [`Record::source`]
/// tells the readings apart.
///
/// It holds a hash of the bytes it names, as they were when the object was
/// read. The writer checks a copy of them by it
/// ([`write::Body::Read`](crate::write::Body::Read)), and refuses bytes
/// that hash otherwise. The hash is keyed by one random key for the
/// process, std's [`RandomState`]'s, which is never shown, so bytes can't
/// be written to hash alike without it. Two records of the same bytes in
/// one process have the same hash, and it's in their equality; in another
/// process it's another. It isn't public, so that how it's made can
/// change, and the record's `Debug` leaves it out.
///
/// Where std has no randomness from the operating system
/// (`wasm32-unknown-unknown`), it makes the key from where memory was
/// allocated, which can be guessed. There the hash tells bytes changed by
/// accident or by another program, not by one who means them to hash
/// alike.
#[derive(Clone)]
#[non_exhaustive]
pub struct Record {
    /// The reading it was read in, whose file its places are in.
    pub source: SourceId,
    /// The object it was read as: the number and generation its `n g obj`
    /// says, or, in an object stream, the number its header gives the
    /// place it was read from, at the generation it was asked for.
    pub id: ObjectId,
    /// Where it was found.
    pub origin: Origin,
    /// Its bytes: from its `n g obj` to past its `endobj`, or to where it
    /// ends without one; or, for an object in an object stream, its value
    /// in the stream's decoded data.
    pub at: Span,
    /// Whether `endobj` follows it. An object in an object stream has none.
    pub endobj: bool,
    /// A stream's data, as it was read: none for an object that isn't a
    /// stream.
    pub data: Option<DataExtent>,
    /// The hash of its bytes, those [`Record::at`] names, as they were read
    /// ([`Record::hash_of`]).
    pub(crate) hash: u64,
}

/// The key every record's hash is made under: one for the process, made
/// the first time a hash is, from the randomness std keys a `HashMap` by.
static RECORD_KEY: LazyLock<RandomState> = LazyLock::new(RandomState::new);

impl Record {
    /// What a record's hash is of `bytes`: std's keyed hash under the
    /// process's one key ([`RECORD_KEY`]), so that two readings in one
    /// process hash the same bytes alike.
    pub(crate) fn hash_of(bytes: &[u8]) -> u64 {
        RECORD_KEY.hash_one(bytes)
    }
}

impl fmt::Debug for Record {
    /// Its fields but the hash, which is another in each process.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Record")
            .field("source", &self.source)
            .field("id", &self.id)
            .field("origin", &self.origin)
            .field("at", &self.at)
            .field("endobj", &self.endobj)
            .field("data", &self.data)
            .finish_non_exhaustive()
    }
}

impl PartialEq for Record {
    fn eq(&self, other: &Record) -> bool {
        let Record {
            source: _,
            id,
            origin,
            at,
            endobj,
            data,
            hash,
        } = self;
        (id, origin, at, endobj, data, hash)
            == (
                &other.id,
                &other.origin,
                &other.at,
                &other.endobj,
                &other.data,
                &other.hash,
            )
    }
}

impl Eq for Record {}

/// Where an object was found.
/// Not exhaustive, as a reader may come to find an object another way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Origin {
    /// At the offset a cross-reference section gives.
    File,
    /// At an offset found by scanning the file, as its sections don't read
    /// ([`Trailer::unwritten`]).
    Recovered,
    /// In object stream `stream`'s decoded data, the `index`th of its
    /// objects (ISO 32000-2, 7.5.7).
    Stream {
        /// The object stream's number: its generation is 0 (7.5.8.3).
        stream: u32,
        /// Which of the stream's objects it is, from 0.
        index: u32,
    },
    /// `by` bytes from the offset its cross-reference section gives, exactly
    /// at its own `n g obj`, where the section's entries are shifted
    /// ([`Fault::Shifted`], [`Fault::Moved`]): before it where `by` is
    /// less than 0.
    Shifted {
        /// How far.
        by: i64,
    },
}

/// Which bytes a stream's data is, and how the reader chose them.
/// Not exhaustive, so that it can come to say more of how the data was found;
/// made only by the reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct DataExtent {
    /// Where the data is, as the stream's [`Data::At`] has it.
    pub at: Span,
    /// Whether it's as long as the stream's `/Length` says, which ended at
    /// `endstream`; else it runs up to `endstream`, found by scanning.
    pub declared: bool,
    /// Whether the stream's `/Length` is a reference to another object.
    pub indirect_length: bool,
}

/// What a file's header and cross-reference sections say of it, as
/// written. Whether its sections read is [`Trailer::unwritten`]'s being
/// none.
/// Not exhaustive, so that it can come to say more of the file as written; a
/// reader outside the crate starts from its `Default`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Facts {
    /// Where `%PDF-` starts, when the file has it within its first 1024
    /// bytes: what's before it is junk some writers put first. The offsets
    /// the file writes count from there (ISO 32000-2, 7.5.2), so every
    /// position a reading gives is this many bytes past what's written.
    pub header_offset: Option<usize>,
    /// The newest cross-reference section read: where it starts, and how
    /// it's written; none when the file was scanned.
    pub newest: Option<(usize, XrefKind)>,
    /// How many cross-reference sections were read along `/Prev`: 0 when
    /// the file was scanned.
    pub sections: usize,
    /// Whether a section is a hybrid file's: a table whose trailer gives a
    /// cross-reference stream's offset by `/XRefStm` (ISO 32000-2, 7.5.8.4).
    pub hybrid: bool,
    /// The largest number of a cross-reference stream read: an update
    /// written after the file numbers its objects above it, as well as
    /// above the trailer's `/Size`.
    pub max_stream: Option<u32>,
    /// Whether a linearization holds (ISO 32000-2, Annex F): the file's
    /// first object, whole within the 1024 bytes after where `%PDF-`
    /// starts, is a dictionary whose `/Linearized` is 1 (or a real whose
    /// floor is 1, as qpdf reads it) and whose `/L`, a direct integer, is
    /// the file's length counted from there. An update appended after it,
    /// which changes the length, voids it. Its hint tables aren't read.
    pub linearized: bool,
    /// Whether the reader read a section or an object away from where the
    /// file's sections put it, or found the objects by scanning: what its
    /// faults say ([`Options::recovery`](crate::Options::recovery)), told
    /// with faults off too.
    pub recovered: bool,
}

/// One of a file's revisions (ISO 32000-2, 7.5.6): the file as first
/// written, or what an incremental update added after it. A linearized
/// file's first-page cross-reference section and the main one its `/Prev`
/// gives, later in the file, are one revision (Annex F). A reader outside
/// the crate makes one by [`Revision::new`], and says where it ends by
/// [`Revision::ending`].
/// Not exhaustive, so that it can come to say more of a revision.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Revision {
    /// Where its newest cross-reference section starts: what its
    /// `startxref` gave, past the header ([`Facts::header_offset`]).
    pub at: usize,
    /// Its trailer: its newest section's, the dictionary after `trailer`
    /// or the cross-reference stream ([`Trailer::stream`]).
    pub trailer: Trailer,
    /// The numbers its sections list, in use or free, but 0, ascending,
    /// each once: what it may have written over of an earlier revision's.
    /// None for the first, before which there's none. After the first, none
    /// where the reader doesn't know them: what it wrote over of the
    /// revisions before it can't then be told, so a checker checks none of
    /// their trailers.
    pub numbers: Option<Box<[u32]>>,
    /// Where its bytes end, when the reader found it: past the `%%EOF`
    /// after the `startxref` that follows its last-written section, the one
    /// furthest into the file (a linearized revision's main section, whose
    /// first-page section's own `startxref` and `%%EOF` may be left out:
    /// Annex F), and past a CR LF, CR or LF right after that `%%EOF`, the
    /// same where none follows (7.5.5, 7.5.6). The file as the revision was
    /// written is its bytes up to the line's end ([`RevisionEnd::line`],
    /// [`Reading::prefix`](crate::read::Reading::prefix)). None where no
    /// `startxref` follows that section, or anything but ASCII white-space
    /// (space, HT, LF, FF and CR, not the NUL PDF counts too) comes between
    /// `startxref` and its offset, digits alone, or between that and a
    /// `%%EOF`, a comment among it.
    pub end: Option<RevisionEnd>,
}

/// Where a revision's bytes end ([`Revision::end`]): past its `%%EOF`, and
/// past the end of that line, never before the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RevisionEnd {
    eof: usize,
    line: usize,
}

impl RevisionEnd {
    /// The end of a revision whose `%%EOF` ends at `eof`, on a line that
    /// ends at `line`: none where `line` is before `eof`.
    #[must_use]
    pub fn new(eof: usize, line: usize) -> Option<RevisionEnd> {
        (eof <= line).then_some(RevisionEnd { eof, line })
    }

    /// Where its `%%EOF` ends: past those five bytes.
    #[must_use]
    pub const fn eof(self) -> usize {
        self.eof
    }

    /// Where the `%%EOF`'s line ends: past a CR LF, CR or LF right after
    /// it, or at it where none follows. The file as the revision was
    /// written is its bytes up to here.
    #[must_use]
    pub const fn line(self) -> usize {
        self.line
    }
}

/// How a cross-reference section is written.
/// Not exhaustive, so that a section written another way can be told apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum XrefKind {
    /// A table, after `xref` (ISO 32000-2, 7.5.4).
    Table,
    /// A cross-reference stream (7.5.8).
    Stream,
}

/// Whether a file is encrypted (its trailer's `/Encrypt`), and if it is,
/// whether the reader opened it.
/// Not exhaustive, as the public-key handler's answers may join it, and
/// `Opened` may come to say more of how the file opened.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Encryption {
    /// It isn't.
    #[default]
    None,
    /// It is, and a password it was given opened it, the owner's or the
    /// user's: its strings and object streams are decrypted. A reader
    /// outside the crate makes it by [`Encryption::opened`], and says which
    /// password by [`Encryption::by`], and what the file grants by
    /// [`Encryption::permitting`].
    #[non_exhaustive]
    Opened {
        /// Whether by the owner's password.
        owner: bool,
        /// Which of the passwords the reader was given opened it: its index
        /// among them, and the encoding it opened in. None when it was
        /// given none, and the empty password opened it.
        by: Option<(usize, Encoding)>,
        /// What the file's `/P` grants one who opens it by the user's
        /// password, by its revision's meaning of the bits (pdf-crypt's
        /// `Handler::permissions`). Opened by the owner's, every permission
        /// is the owner's whatever `/P` says, as `owner` tells: these are
        /// still what `/P` grants a user. The reader enforces none of
        /// them, as it decrypts all of a file it opens.
        permissions: Permissions,
    },
    /// It is, and no password it was given opens it.
    Locked,
    /// It is, and no password it was given opens it (or its handler is
    /// one the reader can't open), but its strings and streams aren't
    /// encrypted (`/StrF` and `/StmF` `/Identity`, as when only embedded
    /// files are, `/AuthEvent /EFOpen`): it reads whole, but for an object
    /// stream whose own `/Crypt` filter encrypts it.
    Plain,
    /// It is, by a security handler the reader can't open, and why: its
    /// strings stay ciphertext, and its object streams aren't read.
    Unsupported(Unsupported),
}

/// Why the reader can't open an encrypted file's security handler
/// ([`Encryption::Unsupported`]): its trailer's `/Encrypt` isn't an
/// encryption dictionary it reads, or the handler refuses the dictionary.
/// Each in its words. Not exhaustive, as the public-key handler's answers
/// may join it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Unsupported {
    /// The trailer's `/Encrypt` is no dictionary the reader reads.
    #[error("{0}")]
    Encrypt(Uncrypt),
    /// pdf-crypt's standard security handler refuses the dictionary:
    /// another handler's ([`Refused::OtherHandler`]), a `/V` and `/R` it
    /// isn't ([`Refused::Version`]), or a dictionary that's wrong
    /// ([`Refused::Malformed`]). Never a password that doesn't open it
    /// ([`Refused::Password`]): that file is
    /// [`Encryption::Locked`].
    #[error("{0}")]
    Refused(Refused),
}

/// Why a trailer's `/Encrypt` isn't an encryption dictionary the reader
/// reads ([`Unsupported::Encrypt`]).
/// Not exhaustive, as reasons are found as the reader learns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Uncrypt {
    /// It isn't a dictionary, nor a reference to one.
    #[error("its /Encrypt isn't a dictionary")]
    NotADictionary,
    /// It's a reference to an object that doesn't read.
    #[error("its /Encrypt doesn't read")]
    Unread,
    /// It's a reference to an object in an object stream, which an
    /// encryption dictionary can't be (ISO 32000-2, 7.5.7).
    #[error("its /Encrypt is in an object stream, which it can't be (ISO 32000-2, 7.5.7)")]
    InObjectStream,
}

/// A file's newest incremental update. A reader outside the crate makes
/// one by [`Update::new`].
/// Not exhaustive, so that it can come to say more of the update.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Update {
    /// The objects its cross-reference section lists in use.
    pub objects: BTreeSet<ObjectId>,
    /// Where the section it follows starts: the file as of that section is
    /// its previous revision ([`Options::section`](crate::Options::section)).
    pub previous: usize,
}

/// Why an object doesn't read: what [`Source::get`] gives in its place,
/// and [`Document::unread`] gives. Each is one of the bytes reader's
/// reasons ([`crate::read`]), or another reader's words.
/// Not exhaustive, as reasons are found as the reader learns.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Unread {
    /// The file hasn't it: no cross-reference section lists it.
    #[error("{id} R isn't in the file")]
    Missing {
        /// The object.
        id: ObjectId,
    },
    /// It doesn't parse at the offset its cross-references give.
    #[error("{0}")]
    Parse(parse::Error),
    /// The offset its cross-references give holds another object.
    #[error("byte {offset} holds {found} obj, where the cross-references put {expected} R")]
    Elsewhere {
        /// The offset.
        offset: usize,
        /// The object written there.
        found: ObjectId,
        /// The object the cross-references put there.
        expected: ObjectId,
    },
    /// It's in object stream `stream` (ISO 32000-2, 7.5.7), whose data
    /// doesn't decode, and why.
    #[error("its object stream {stream} 0 R doesn't decode: {why}")]
    Container {
        /// The object stream's number.
        stream: u32,
        /// Why its data doesn't decode.
        why: Undecoded,
    },
    /// It's in object stream `stream`, in an encrypted file that no
    /// password the reader was given opens, and nothing else decodes it:
    /// its password is needed.
    #[error("it's in object stream {stream} 0 R, and nothing decodes that")]
    Undecodable {
        /// The object stream's number.
        stream: u32,
    },
    /// It's in object stream `stream`, which doesn't read itself, and why.
    #[error("its object stream {stream} 0 R doesn't read: {why}")]
    ContainerUnread {
        /// The object stream's number.
        stream: u32,
        /// Why the object stream doesn't read.
        why: Box<Unread>,
    },
    /// It's in object stream `stream`, which isn't in the file.
    #[error("its object stream {stream} 0 R isn't in the file")]
    ContainerMissing {
        /// The object stream's number.
        stream: u32,
    },
    /// It's in object stream `stream`, whose header doesn't say where its
    /// objects are, and why: said as the object's own.
    #[error("{why}")]
    ContainerHeader {
        /// The object stream's number.
        stream: u32,
        /// Why its header doesn't say.
        why: Unindexed,
    },
    /// Its cross-references put it at `index` in object stream `stream`,
    /// whose header lists fewer objects.
    #[error("object stream {stream} 0 R has no object {index}")]
    NoIndex {
        /// The object stream's number.
        stream: u32,
        /// Which of the stream's objects the cross-references say it is,
        /// from 0.
        index: u32,
    },
    /// Its cross-references put it at `index` in object stream `stream`,
    /// whose header numbers that object `number`.
    #[error("object {index} of object stream {stream} 0 R is {number}, not {expected}")]
    Numbered {
        /// The object stream's number.
        stream: u32,
        /// Which of the stream's objects the cross-references say it is,
        /// from 0.
        index: u32,
        /// The number the header gives it.
        number: u32,
        /// Its own number.
        expected: u32,
    },
    /// It's in object stream `stream`, whose decoded data the reader
    /// hasn't.
    #[error("object stream {stream} 0 R has no data")]
    NoData {
        /// The object stream's number.
        stream: u32,
    },
    /// It's a value nothing closes (a number, a name, a keyword) that runs
    /// to where a fault cut object stream `stream`'s data off, so it may be
    /// cut short.
    #[error(
        "it runs to where a fault cut object stream {stream} 0 R's data off, so it may be cut short"
    )]
    Cut {
        /// The object stream's number.
        stream: u32,
    },
    /// The source of the file's bytes didn't give those the read asked for
    /// ([`Error::Io`]), in its words: a [`Reading`](crate::Reading)'s
    /// [`Source::get`] gives it where the reading's own `get` gives that
    /// error. Nothing of the read is kept, so asked again it may read.
    #[error("{message}")]
    Source {
        /// What the source said.
        message: String,
    },
    /// Another reader's words: a [`Source`] outside the crate says why in
    /// its own.
    #[error("{0}")]
    Other(String),
}

impl Unread {
    /// Why object `id`, which doesn't read for this, doesn't, as
    /// [`Resolve::try_resolve`] gives it: none where the file hasn't it
    /// ([`Unread::Missing`]), which `try_resolve` gives as no object, and
    /// else as a [`Document`]'s and a [`Reading`](crate::Reading)'s say it,
    /// so that a resolver over any [`Source`] can. Its [`Why`] is
    /// [`Why::Damaged`], [`Why::Capped`] (past a cap the reader parses or
    /// decodes under), [`Why::Locked`] (in an object stream that takes a
    /// password, or doesn't decrypt), or [`Why::Source`] where the source of
    /// the file's bytes didn't give them ([`Unread::Source`]), its message
    /// then the one a reading's own resolver gives.
    #[must_use]
    pub fn unreadable(&self, id: ObjectId) -> Option<Unreadable> {
        if let Unread::Missing { .. } = self {
            return None;
        }
        let message = format!("{id} R doesn't read: {self}");
        Some(Unreadable::new(id, self.why(), message))
    }

    /// What kind of failure it is. Every kind of failure is named here, so
    /// that a new one is put with its like on purpose.
    fn why(&self) -> Why {
        match self {
            Unread::Parse(parse::Error::Nested { .. } | parse::Error::Values { .. }) => Why::Capped,
            Unread::Container { why, .. } => match why {
                Undecoded::Filters(Unfiltered::Filters { .. }) => Why::Capped,
                Undecoded::Encrypted { .. } | Undecoded::Decrypt(_) => Why::Locked,
                Undecoded::NotAStream | Undecoded::Filters(_) | Undecoded::Other(_) => Why::Damaged,
            },
            Unread::Undecodable { .. } => Why::Locked,
            Unread::ContainerUnread { why, .. } => why.why(),
            Unread::Source { .. } => Why::Source,
            Unread::Missing { .. }
            | Unread::Parse(_)
            | Unread::Elsewhere { .. }
            | Unread::ContainerMissing { .. }
            | Unread::ContainerHeader { .. }
            | Unread::NoIndex { .. }
            | Unread::Numbered { .. }
            | Unread::NoData { .. }
            | Unread::Cut { .. }
            | Unread::Other(_) => Why::Damaged,
        }
    }
}

impl Encryption {
    /// [`Encryption::Opened`]: by the owner's password when `owner`, else
    /// by the user's; by the empty password, given none, until
    /// [`Encryption::by`] says which of those given; and granting none,
    /// until [`Encryption::permitting`] says what. Each field `Opened`
    /// gains has a setter of its own, so this stays as it is.
    #[must_use]
    pub const fn opened(owner: bool) -> Encryption {
        Encryption::Opened {
            owner,
            by: None,
            permissions: Permissions::NONE,
        }
    }

    /// It, opened by the password given at index `by`'s, in its encoding,
    /// or none, by the empty password, given none: `Opened`'s `by`. Any
    /// other is given back as it is.
    #[must_use = "it gives the Encryption back, with which password opened it"]
    pub fn by(self, by: Option<(usize, Encoding)>) -> Encryption {
        match self {
            Encryption::Opened {
                owner, permissions, ..
            } => Encryption::Opened {
                owner,
                by,
                permissions,
            },
            other => other,
        }
    }

    /// It, opened, granting `permissions` to one who opens it by the
    /// user's password: `Opened`'s `permissions`. Any other is given back
    /// as it is.
    #[must_use = "it gives the Encryption back, with what it grants"]
    pub fn permitting(self, permissions: Permissions) -> Encryption {
        match self {
            Encryption::Opened { owner, by, .. } => Encryption::Opened {
                owner,
                by,
                permissions,
            },
            other => other,
        }
    }
}

impl Update {
    /// The update whose cross-reference section lists `objects` in use,
    /// after the section that starts at `previous`.
    #[must_use]
    pub const fn new(objects: BTreeSet<ObjectId>, previous: usize) -> Update {
        Update { objects, previous }
    }
}

impl Revision {
    /// The revision whose newest cross-reference section starts at `at`,
    /// under `trailer`, its sections listing `numbers` (none for the
    /// first), where it ends not known. Each field `Revision` gains has a
    /// setter of its own, so this stays as it is.
    #[must_use]
    pub const fn new(at: usize, trailer: Trailer, numbers: Option<Box<[u32]>>) -> Revision {
        Revision {
            at,
            trailer,
            numbers,
            end: None,
        }
    }

    /// It, ending at `end` ([`Revision::end`]): past its `%%EOF`, and past
    /// the end of that line.
    #[must_use = "it gives the Revision back, with where it ends"]
    pub fn ending(self, end: RevisionEnd) -> Revision {
        Revision {
            end: Some(end),
            ..self
        }
    }
}

impl Unread {
    /// The object stream an object that doesn't read is in, where why is
    /// that stream's or the object's place in it: the stream doesn't
    /// decode, or nothing decodes it, it doesn't read or isn't in the file,
    /// its header doesn't say where the object is, or has no such object,
    /// or numbers it another, it has no data, or the object runs to where a
    /// fault cut it off. None for the rest, an object in one that doesn't
    /// parse among them, which isn't placed.
    #[must_use]
    pub const fn container(&self) -> Option<u32> {
        match *self {
            Unread::Container { stream, .. }
            | Unread::Undecodable { stream }
            | Unread::ContainerUnread { stream, .. }
            | Unread::ContainerMissing { stream }
            | Unread::ContainerHeader { stream, .. }
            | Unread::NoIndex { stream, .. }
            | Unread::Numbered { stream, .. }
            | Unread::NoData { stream }
            | Unread::Cut { stream } => Some(stream),
            Unread::Missing { .. }
            | Unread::Parse(_)
            | Unread::Elsewhere { .. }
            | Unread::Source { .. }
            | Unread::Other(_) => None,
        }
    }
}

/// What a [`Document`] holds of an object its cross-references list: the
/// object, with the record of where it was read while it's as read, none
/// for one put in or taken to edit; or why it doesn't read.
pub(crate) type Held = Result<(Object, Option<Record>), Unread>;

/// One past the largest object number, 4,294,967,295: where a document's
/// new ids are once they've gone past every number ([`Document::new_id`]).
const PAST: u64 = 1 << 32;

/// How many references [`Document::dereference`] follows, at the most.
const DEREFERENCES: usize = 128;

/// Where new ids start in a document under `trailer` whose largest number
/// listed is `listed`: one past it, and past the trailer's `/Size` less
/// one, where that's a number below the largest (`/Size` is one past the
/// largest number a file's cross-references list, ISO 32000-2, 7.5.5);
/// never below 1, as object 0 is always free. A `/Size` of 2^32 or more
/// is passed over, so a `/Size` alone never takes new ids past every
/// number, where they'd be numbers no entry holds, such as one a file
/// lists free: only an object at the largest number does.
fn first_new(trailer: &Dict, listed: Option<u32>) -> u64 {
    let size = (trailer.get(b"Size").and_then(Object::as_integer))
        .and_then(|size| u64::try_from(size).ok())
        .filter(|&size| size < PAST);
    let listed = listed.map(|n| u64::from(n) + 1);
    size.into_iter().chain(listed).fold(1, u64::max)
}

impl Document {
    /// A document with no objects yet, under `trailer`, its header at
    /// `version`: where a reader outside the crate starts, filling in what
    /// it read.
    ///
    /// ```
    /// use std::collections::BTreeSet;
    ///
    /// use pdf_structure::pdf_objects::{ObjectId, dict, object::Object, version::Version};
    /// use pdf_structure::{Document, Encryption, Source, Update};
    ///
    /// let trailer = dict! { "Root" => Object::reference(1) };
    /// let mut doc = Document::new(Version::new(1, 7), trailer);
    /// doc.insert(1, dict! { "Type" => "Catalog" });
    /// doc.set_update(Some(Update::new(BTreeSet::from([ObjectId::new(1, 0)]), 9)));
    /// doc.set_encryption(Encryption::opened(false));
    /// assert!(doc.get(ObjectId::new(1, 0)).is_ok());
    /// assert_eq!(*doc.encryption(), Encryption::opened(false));
    /// ```
    #[must_use]
    pub fn new(version: Version, trailer: Dict) -> Document {
        Document {
            objects: BTreeMap::new(),
            next: first_new(&trailer, None),
            trailer: Trailer {
                stream: None,
                object: Object::new(Value::Dict(trailer)),
                unwritten: None,
            },
            header: Some(version),
            faults: Vec::new(),
            decoded: BTreeMap::new(),
            whole: BTreeSet::new(),
            update: None,
            size: None,
            metadata: None,
            metadata_of: None,
            encryption: Encryption::None,
            facts: Facts::default(),
            revisions: Vec::new(),
            in_the_clear: BTreeSet::new(),
        }
    }

    /// What a reader read: `objects`, each with the record of where it was
    /// read, under `trailer`, new ids starting past them, past `listed`,
    /// the largest number the file's cross-references list beside them
    /// (a free one, or a cross-reference stream's), and past the
    /// trailer's `/Size` less one; the rest as [`Document::new`] leaves it,
    /// and no header, for the reader to fill in.
    pub(crate) fn read(
        objects: BTreeMap<ObjectId, Held>,
        trailer: Trailer,
        listed: Option<u32>,
    ) -> Document {
        let held = objects.last_key_value().map(|(id, _)| id.number);
        Document {
            next: first_new(trailer.dict(), listed.max(held)),
            objects,
            trailer,
            header: None,
            faults: Vec::new(),
            decoded: BTreeMap::new(),
            whole: BTreeSet::new(),
            update: None,
            size: None,
            metadata: None,
            metadata_of: None,
            encryption: Encryption::None,
            facts: Facts::default(),
            revisions: Vec::new(),
            in_the_clear: BTreeSet::new(),
        }
    }

    /// Puts object `n`, generation 0, with no record: [`Document::set`]
    /// at `ObjectId::new(n, 0)`.
    pub fn insert(&mut self, n: u32, object: impl Into<Object>) {
        self.set(ObjectId::new(n, 0), object);
    }

    /// Whether it lists object `id`, whether it reads or not.
    #[must_use]
    #[inline]
    pub fn contains(&self, id: ObjectId) -> bool {
        self.objects.contains_key(&id)
    }

    /// Each object that reads, by number.
    pub fn objects(&self) -> Objects<'_> {
        Objects {
            held: self.objects.iter(),
        }
    }

    /// Each object that reads whose id is in `range`, by number: a
    /// number's every generation, `n`'s, is
    /// `ObjectId::new(n, 0)..=ObjectId::new(n, u16::MAX)`. Panics where
    /// [`BTreeMap::range`] does: a range that starts after it ends, or
    /// starts and ends at one id it leaves out.
    pub fn range(&self, range: impl RangeBounds<ObjectId>) -> Range<'_> {
        Range {
            held: self.objects.range(range),
        }
    }

    /// Each object it lists that doesn't read, by number, with why.
    pub fn unread(&self) -> Unreads<'_> {
        Unreads {
            held: self.objects.iter(),
        }
    }

    /// Where object `id` was read from, while it's as read: none for an
    /// object put in by [`Document::set`] or taken to edit by
    /// [`Document::get_mut`], one a reader other than the bytes reader
    /// read, or one that doesn't read.
    #[must_use]
    #[inline]
    pub fn record(&self, id: ObjectId) -> Option<&Record> {
        self.objects.get(&id)?.as_ref().ok()?.1.as_ref()
    }

    /// Each object's record, by number: each [`Document::record`] gives.
    pub fn records(&self) -> Records<'_> {
        Records {
            held: self.objects.iter(),
        }
    }

    /// What it holds of each object it lists, by number, for the writer,
    /// which takes each that reads with its record while it's as read
    /// ([`write::Bodies`](crate::write::Bodies)).
    pub(crate) fn held_objects(&self) -> btree_map::Iter<'_, ObjectId, Held> {
        self.objects.iter()
    }

    /// Each record it holds made reading `source`'s ([`Record::source`]),
    /// for [`Reading::adopt`](crate::Reading::adopt), which has found that
    /// reading's own record of each object equal to it: so each is that
    /// reading's record.
    pub(crate) fn adopted(&mut self, source: SourceId) {
        let held = (self.objects.values_mut()).filter_map(|held| held.as_mut().ok());
        for record in held.filter_map(|(_, record)| record.as_mut()) {
            record.source = source;
        }
    }

    /// Puts `object` at `id`, with no record, as it wasn't read: gives the
    /// object it replaces, when one that read was there. New ids start
    /// past `id`'s number, if they didn't. The object keeps the spans it
    /// has, its own and its values', of whatever file it was read from,
    /// with nothing to say which, so an object of another file is put in
    /// unplaced ([`Object::unplace`]), a stream's data given first as its
    /// bytes, as that file's reading hands them out: a check reads spans as
    /// places in this document's file, and a writer would read a stream's
    /// data where its span says in the reading of this one.
    ///
    /// An object stream the reader decoded, at `id`, is no longer lent
    /// decoded ([`Streams::decoded`]), as the object there isn't the one
    /// decoded; the objects read from it keep their places in its data
    /// ([`Document::held`]), as their records do. Nor is the metadata read
    /// from a stream at `id` the document's ([`Document::metadata`]).
    pub fn set(&mut self, id: ObjectId, object: impl Into<Object>) -> Option<Object> {
        self.past(id.number);
        self.edited(id);
        let was = self.objects.insert(id, Ok((object.into(), None)));
        was?.ok().map(|(object, _)| object)
    }

    /// Puts why object `id` doesn't read at `id`, as a reader outside the
    /// crate does for an object it lists that doesn't read: gives the
    /// object it replaces, when one that read was there. New ids start
    /// past `id`'s number, if they didn't. An object stream there is no
    /// longer lent decoded, nor metadata read from it the document's, as
    /// [`Document::set`] says.
    pub fn set_unread(&mut self, id: ObjectId, why: Unread) -> Option<Object> {
        self.past(id.number);
        self.edited(id);
        let was = self.objects.insert(id, Err(why));
        was?.ok().map(|(object, _)| object)
    }

    /// New ids start past `n`, an entry's number, where they didn't. An
    /// entry at 4,294,967,295, the largest, takes them past every number,
    /// to be looked for above the number they'd have started after, so
    /// that none handed out before is handed out again.
    fn past(&mut self, n: u32) {
        self.next = match u32::try_from(self.next) {
            Ok(next) if n == u32::MAX => PAST + u64::from(next - 1),
            _ => self.next.max(u64::from(n) + 1),
        };
    }

    /// Removes object `id`, whether it reads or not, its record with it:
    /// gives the object, when it read. An object stream there is no longer
    /// lent decoded, nor metadata read from it the document's, as
    /// [`Document::set`] says.
    pub fn remove(&mut self, id: ObjectId) -> Option<Object> {
        self.edited(id);
        self.objects.remove(&id)?.ok().map(|(object, _)| object)
    }

    /// Object `id` is no longer as read: an object stream there no longer
    /// lent decoded, and metadata read from a stream there no longer the
    /// document's.
    fn edited(&mut self, id: ObjectId) {
        self.whole.remove(&id);
        if self.metadata_of == Some(id) {
            (self.metadata, self.metadata_of) = (None, None);
        }
    }

    /// Object `id`, to edit, when it reads. Its record is dropped, as what
    /// the object is then is the caller's: a record says the object is as
    /// it was read, which a `&mut` can replace whole without an edit that
    /// drops its span. It's the object at `id` itself, a reference as it
    /// is, not followed: [`Document::dereference`] gives the id a chain of
    /// them ends at, and [`Document::get_dict_mut`] follows one to a
    /// dictionary. An object stream taken so is no longer lent decoded,
    /// nor metadata read from it the document's, as [`Document::set`] says.
    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.objects.get(&id)?.as_ref().ok()?;
        self.edited(id);
        let (object, record) = self.objects.get_mut(&id)?.as_mut().ok()?;
        *record = None;
        Some(object)
    }

    /// Object `id`, when it reads.
    fn object(&self, id: ObjectId) -> Option<&Object> {
        Some(&self.objects.get(&id)?.as_ref().ok()?.0)
    }

    /// An id for a new object, generation 0, not set yet: past every
    /// number the file's cross-references list (in use, in an object
    /// stream or free), its cross-reference streams' too, the trailer's
    /// `/Size` less one, and every number put in or handed out since, so
    /// that each call gives another until they go round past the largest
    /// (below). Never object 0, which is always free.
    ///
    /// Once those reach 4,294,967,295, the largest number, as a file that
    /// lists it can, a new id is the next number above the last one handed
    /// out that no entry holds (or, where an object put at the largest took
    /// them there, above the number they'd have started after), from 1
    /// again after the largest. Then it may be one the file lists free,
    /// which an update writing it there should know, as its object's
    /// generation is 0; and once they've gone round from 1, one handed out
    /// before and not set since. There's always one, as an object at every
    /// number would take more than 700 GB; were a document to hold them,
    /// the new id would be 1, which it holds.
    pub fn new_id(&mut self) -> ObjectId {
        if let Ok(n) = u32::try_from(self.next) {
            self.next += 1;
            return ObjectId::new(n, 0);
        }
        let last = u32::try_from(self.next - PAST).unwrap_or(0);
        let n = (self.free_after(last))
            .or_else(|| self.free_after(0))
            .unwrap_or(1);
        self.next = PAST + u64::from(n);
        ObjectId::new(n, 0)
    }

    /// The first number above `last` that no entry holds, at any
    /// generation; none when every one to 4,294,967,295 is held.
    fn free_after(&self, last: u32) -> Option<u32> {
        let mut n = last.checked_add(1)?;
        for &ObjectId { number: held, .. } in
            self.objects.range(ObjectId::new(n, 0)..).map(|(id, _)| id)
        {
            if held > n {
                break;
            }
            if held == n {
                n = n.checked_add(1)?;
            }
        }
        Some(n)
    }

    /// Puts `object` at a new id ([`Document::new_id`]), with no record,
    /// and gives the id.
    pub fn add(&mut self, object: impl Into<Object>) -> ObjectId {
        let id = self.new_id();
        self.set(id, object);
        id
    }

    /// The number new ids start after: one less than the next
    /// [`Document::new_id`] gives, which can be a number the file lists
    /// free, as the trailer's `/Size` less one often is; 4,294,967,295 once
    /// new ids have gone past it.
    #[must_use]
    pub fn max_id(&self) -> u32 {
        u32::try_from(self.next - 1).unwrap_or(u32::MAX)
    }

    /// Has new ids start after `n`, or after the largest number an entry
    /// holds, where that's larger: never at a number held. Lowered, they
    /// may be numbers the file lists free, or numbers handed out before
    /// and not set. After 4,294,967,295, the largest, they're looked for
    /// from 1 again, as [`Document::new_id`]'s are once they've gone round.
    pub fn set_max_id(&mut self, n: u32) {
        let held = self.objects.last_key_value().map(|(id, _)| id.number);
        self.next = u64::from(n.max(held.unwrap_or(0))) + 1;
    }

    /// The object a chain of references ends at, from `object`, and the
    /// last id on the way: `object` itself, and none, when it isn't a
    /// reference. None where one names an object it hasn't, or one that
    /// doesn't read, or the chain runs past 128 references, as a loop
    /// does.
    #[must_use]
    pub fn dereference<'a>(&'a self, object: &'a Object) -> Option<(Option<ObjectId>, &'a Object)> {
        let (mut last, mut object) = (None, object);
        let mut followed = 0;
        while let Some(id) = object.as_reference() {
            if followed == DEREFERENCES {
                return None;
            }
            followed += 1;
            last = Some(id);
            object = self.object(id)?;
        }
        Some((last, object))
    }

    /// Object `id`'s dictionary, or its stream's, through the references
    /// it is ([`Document::dereference`]), when it reads.
    #[must_use]
    pub fn get_dict(&self, id: ObjectId) -> Option<&Dict> {
        self.dereference(self.object(id)?)?.1.as_dict()
    }

    /// [`Document::get_dict`]'s dictionary, to change: an edit of the
    /// object it's in, as through [`Object::as_dict_mut`], which drops its
    /// record as [`Document::get_mut`] does. Nothing's changed where it
    /// gives none.
    pub fn get_dict_mut(&mut self, id: ObjectId) -> Option<&mut Dict> {
        let (last, object) = self.dereference(self.object(id)?)?;
        object.as_dict()?;
        self.get_mut(last.unwrap_or(id))?.as_dict_mut()
    }

    /// The catalog: the dictionary the trailer's `/Root` refers to (ISO
    /// 32000-2, 7.7.2), as [`Document::get_dict`] gives it.
    #[must_use]
    pub fn catalog(&self) -> Option<&Dict> {
        self.get_dict(self.trailer.dict().get(b"Root")?.as_reference()?)
    }

    /// The catalog, to change, as [`Document::get_dict_mut`] gives it.
    pub fn catalog_mut(&mut self) -> Option<&mut Dict> {
        self.get_dict_mut(self.trailer.dict().get(b"Root")?.as_reference()?)
    }

    /// Every id the trailer reaches, through the objects that read, each
    /// once, in the order found: the trailer's references, then each found
    /// object's in turn, each value's in the order written
    /// ([`Object::references`]), every entry of a key written twice among
    /// them. An id of an object it hasn't, or of one that doesn't read, is
    /// found, and goes no further.
    #[must_use]
    pub fn reachable(&self) -> Vec<ObjectId> {
        let mut found = Vec::new();
        let mut seen = HashSet::new();
        let trailer = self.trailer.dict().entries().iter();
        for id in trailer.flat_map(|entry| entry.value.references()) {
            if seen.insert(id) {
                found.push(id);
            }
        }
        let mut i = 0;
        while let Some(&id) = found.get(i) {
            if let Some(object) = self.object(id) {
                for id in object.references() {
                    if seen.insert(id) {
                        found.push(id);
                    }
                }
            }
            i += 1;
        }
        found
    }

    /// Removes each object the trailer doesn't reach
    /// ([`Document::reachable`]), whether it reads or not, its record with
    /// it, and gives their ids by number. Nothing refers to an object
    /// stream or a cross-reference stream, so they go, and
    /// [`Trailer::stream`], [`Document::update`]'s objects and a fault's
    /// object may then name one it no longer holds; the trailer refers to
    /// the encryption dictionary, so it stays.
    /// What it holds of the object streams it decoded ([`Document::held`])
    /// is left as it is, as the objects read from an object stream are
    /// placed in its data still. New ids start where they did.
    pub fn prune(&mut self) -> Vec<ObjectId> {
        let reached: HashSet<ObjectId> = self.reachable().into_iter().collect();
        let mut pruned = Vec::new();
        self.objects.retain(|&id, _| {
            let kept = reached.contains(&id);
            if !kept {
                pruned.push(id);
            }
            kept
        });
        pruned
    }
}

/// What a document holds beside its objects, each answer [`Source`]'s of
/// the same name, in a plainer form where it always knows; and a setter for
/// what a reader outside the crate knows and fills in.
impl Document {
    /// The trailer.
    #[must_use]
    pub const fn trailer(&self) -> &Trailer {
        &self.trailer
    }

    /// The trailer, to change: its dictionary ([`Trailer::dict_mut`]), the
    /// cross-reference stream it is, or why it isn't the trailer the
    /// file's newest section writes, as a reader outside the crate fills
    /// them in.
    pub const fn trailer_mut(&mut self) -> &mut Trailer {
        &mut self.trailer
    }

    /// The version in the header, when it has one.
    #[must_use]
    pub const fn header_version(&self) -> Option<Version> {
        self.header
    }

    /// Has the header's version be `version`.
    pub const fn set_header_version(&mut self, version: Option<Version>) {
        self.header = version;
    }

    /// What the bytes reader told of as it read the file, each fault at its
    /// place, in the whole read's order: none in a document built, or read
    /// with none looked for ([`Options::faults`](crate::Options::faults)). A
    /// reader outside the crate keeps its own, as they're its own, not the
    /// bytes reader's.
    #[must_use]
    pub fn faults(&self) -> &[(Place, Fault)] {
        &self.faults
    }

    /// Object stream `stream`'s decoded data, which the objects read from
    /// it are placed in ([`Span::stream`]), when the reader decoded it and
    /// its header reads: the place the records of the objects read from it
    /// name, whether or not the stream is lent as its data decoded
    /// ([`Streams::decoded`]), which it is while it's whole and its object
    /// as read.
    #[must_use]
    pub fn held(&self, stream: ObjectId) -> Option<&[u8]> {
        self.decoded.get(&stream).map(Vec::as_slice)
    }

    /// Whether object stream `stream` leaves the strings of the objects
    /// read from it in the clear, in a file that encrypts its strings, by
    /// `/StmF` or its own `/Crypt` filter `/Identity` (ISO 32000-2, 7.5.7,
    /// 7.6.6): recorded by the reader of each it decoded, or a decoder did.
    #[must_use]
    pub fn in_the_clear(&self, stream: ObjectId) -> bool {
        self.in_the_clear.contains(&stream)
    }

    /// The file's newest incremental update, when the reader read its
    /// cross-reference sections and the newest follows one written before
    /// it.
    #[must_use]
    pub const fn update(&self) -> Option<&Update> {
        self.update.as_ref()
    }

    /// Has its newest incremental update be `update`.
    pub fn set_update(&mut self, update: Option<Update>) {
        self.update = update;
    }

    /// The file's length in bytes, when the reader knows it.
    #[must_use]
    pub const fn size(&self) -> Option<usize> {
        self.size
    }

    /// Has the file's length in bytes be `size`.
    pub const fn set_size(&mut self, size: Option<usize>) {
        self.size = size;
    }

    /// The data of the catalog's metadata stream (ISO 32000-2, 14.3.2), as
    /// written, when it's in plain view: unfiltered, and not encrypted.
    /// What the reader read of it is given while the catalog's metadata
    /// stream is the one it was read from and that's as read: none once
    /// that stream is put, edited, set unread or removed
    /// ([`Document::set`]), or the catalog or the trailer's `/Root` leads
    /// to another. What's set ([`Document::set_metadata`]) is given as set.
    #[must_use]
    pub fn metadata(&self) -> Option<&[u8]> {
        let data = self.metadata.as_deref()?;
        match self.metadata_of {
            None => Some(data),
            Some(of) => (self.metadata_stream() == Some(of)).then_some(data),
        }
    }

    /// Has the catalog's metadata stream's data in plain view be `data`,
    /// given as set whatever's edited after.
    pub fn set_metadata(&mut self, data: Option<Vec<u8>>) {
        (self.metadata, self.metadata_of) = (data, None);
    }

    /// Which object the catalog's metadata stream is, through the objects
    /// it holds.
    fn metadata_stream(&self) -> Option<ObjectId> {
        let get: crate::crypt::Get<'_, '_, std::convert::Infallible> = &|id| Ok(self.object(id));
        let Ok(stream) = crate::crypt::metadata_stream(self.trailer.dict(), get);
        stream
    }

    /// Whether the file is encrypted, and if it is, whether it opened.
    #[must_use]
    pub const fn encryption(&self) -> &Encryption {
        &self.encryption
    }

    /// Has the file's encryption be `encryption`.
    pub fn set_encryption(&mut self, encryption: Encryption) {
        self.encryption = encryption;
    }

    /// What the file's cross-reference sections and header say of it, as
    /// written, by the bytes reader; the defaults where another read it,
    /// and in a document built, where [`Facts::linearized`] false says only
    /// that it isn't known to be.
    #[must_use]
    pub const fn facts(&self) -> &Facts {
        &self.facts
    }

    /// The file's revisions, the first first, when the reader read its
    /// cross-reference sections: the last's trailer is
    /// [`Document::trailer`] as read. Empty where the file was scanned, or
    /// its later revisions list more numbers in all than the reader reads
    /// entries ([`Limits::entries`](crate::Limits::entries)), and in a
    /// document built.
    #[must_use]
    pub fn revisions(&self) -> &[Revision] {
        &self.revisions
    }

    /// Has the file's revisions, the first first, be `revisions`.
    pub fn set_revisions(&mut self, revisions: Vec<Revision>) {
        self.revisions = revisions;
    }
}

/// Each object a [`Document`] holds that reads, by number:
/// [`Document::objects`].
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Debug, Clone)]
pub struct Objects<'d> {
    held: btree_map::Iter<'d, ObjectId, Held>,
}

/// An object that reads, of what a document holds of `id`.
fn object_of<'d>((&id, held): (&ObjectId, &'d Held)) -> Option<(ObjectId, &'d Object)> {
    Some((id, &held.as_ref().ok()?.0))
}

/// Why an object doesn't read, of what a document holds of `id`.
fn unread_of<'d>((&id, held): (&ObjectId, &'d Held)) -> Option<(ObjectId, &'d Unread)> {
    Some((id, held.as_ref().err()?))
}

/// An object's record, of what a document holds of `id`.
fn record_of<'d>((&id, held): (&ObjectId, &'d Held)) -> Option<(ObjectId, &'d Record)> {
    Some((id, held.as_ref().ok()?.1.as_ref()?))
}

/// An iterator over what a document holds of each object: the objects
/// held that give one, by number, as `filter_map` walks them, both ways,
/// a walk to the end (`fold`, and so `count` and `for_each`) by the map's
/// own, as `filter_map`'s is.
macro_rules! held_iterator {
    ($name:ident, $item:ty, $of:ident) => {
        impl<'d> Iterator for $name<'d> {
            type Item = (ObjectId, $item);

            #[inline]
            fn next(&mut self) -> Option<Self::Item> {
                self.held.find_map($of)
            }

            /// At most one an object left.
            #[inline]
            fn size_hint(&self) -> (usize, Option<usize>) {
                (0, self.held.size_hint().1)
            }

            #[inline]
            fn fold<B, F>(self, init: B, mut f: F) -> B
            where
                F: FnMut(B, Self::Item) -> B,
            {
                (self.held).fold(init, |acc, held| match $of(held) {
                    Some(item) => f(acc, item),
                    None => acc,
                })
            }
        }

        impl DoubleEndedIterator for $name<'_> {
            #[inline]
            fn next_back(&mut self) -> Option<Self::Item> {
                self.held.by_ref().rev().find_map($of)
            }

            #[inline]
            fn rfold<B, F>(self, init: B, mut f: F) -> B
            where
                F: FnMut(B, Self::Item) -> B,
            {
                (self.held).rfold(init, |acc, held| match $of(held) {
                    Some(item) => f(acc, item),
                    None => acc,
                })
            }
        }

        impl FusedIterator for $name<'_> {}
    };
}

held_iterator!(Objects, &'d Object, object_of);

/// Each object a [`Document`] holds that reads whose id is in a range, by
/// number: [`Document::range`].
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Debug, Clone)]
pub struct Range<'d> {
    held: btree_map::Range<'d, ObjectId, Held>,
}

held_iterator!(Range, &'d Object, object_of);

/// Each object a [`Document`] lists that doesn't read, by number, with
/// why: [`Document::unread`].
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Debug, Clone)]
pub struct Unreads<'d> {
    held: btree_map::Iter<'d, ObjectId, Held>,
}

held_iterator!(Unreads, &'d Unread, unread_of);

/// Each object's record a [`Document`] holds, by number:
/// [`Document::records`].
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Debug, Clone)]
pub struct Records<'d> {
    held: btree_map::Iter<'d, ObjectId, Held>,
}

held_iterator!(Records, &'d Record, record_of);

impl Source for Document {
    fn ids(&self) -> Vec<ObjectId> {
        self.objects.keys().copied().collect()
    }

    #[inline]
    fn get(&self, id: ObjectId) -> Result<&Object, Unread> {
        match self.objects.get(&id) {
            Some(Ok((object, _))) => Ok(object),
            Some(Err(why)) => Err(why.clone()),
            None => Err(Unread::Missing { id }),
        }
    }

    fn trailer(&self) -> &Trailer {
        &self.trailer
    }

    fn header_version(&self) -> Option<Version> {
        self.header
    }

    fn faults(&self) -> Cow<'_, [(Place, Fault)]> {
        Cow::Borrowed(&self.faults)
    }

    fn size(&self) -> Option<usize> {
        self.size
    }

    fn metadata(&self) -> Option<Cow<'_, [u8]>> {
        Document::metadata(self).map(Cow::Borrowed)
    }

    fn record(&self, id: ObjectId) -> Option<&Record> {
        Document::record(self, id)
    }

    fn held(&self, stream: ObjectId) -> Option<&[u8]> {
        self.decoded.get(&stream).map(Vec::as_slice)
    }

    fn revisions(&self) -> &[Revision] {
        &self.revisions
    }

    fn facts(&self) -> Option<&Facts> {
        Some(&self.facts)
    }

    fn in_the_clear(&self, stream: ObjectId) -> bool {
        self.in_the_clear.contains(&stream)
    }
}

/// A stream's data as a viewer takes it ([`Streams`]), of what a document
/// holds: an object stream's decoded data ([`Document::held`]), lent,
/// within the limit, else [`Why::Limit`], where the reader decoded it
/// whole and its object is as read (not put, edited, set unread or
/// removed since: [`Document::set`]); and the data of a stream it holds as
/// bytes, built or edited in memory and so in plain view
/// ([`Data::Bytes`]), decoded as a reading decodes a stream, owned: through
/// the filters its `/Filter` and `/DecodeParms` name
/// ([`read::filters`](crate::read::filters)), their references followed
/// through the document, Flate on the default backend, up to the less of
/// the limit and the reader's own cap,
/// [`read::STREAM`](crate::read::STREAM), and given as a reading's
/// [`Streams`] gives it ([`read::viewed`](crate::read::viewed)): past the
/// limit [`Why::Limit`], past the cap [`Why::Capped`], through filters it
/// doesn't take [`Why::Filter`]. Its `/Crypt` filter, whatever it names,
/// is no filter, as `read::filters` reads one: bytes held in memory are
/// plaintext, which a writer encrypts as it writes them, by the filter
/// named ([`write`](crate::write)), so a document decrypts nothing. Any
/// other stream's data, a span of a file the document doesn't hold, is
/// [`Why::Detached`], which a document read beside its reading
/// ([`WithReading`](crate::read::WithReading)) gives, and so is an object
/// stream's that the reader cut off as it decoded it, at a fault, the
/// stream's limit or the budget, or that didn't decrypt cleanly, as what it
/// holds of that isn't the stream's data. What isn't a stream, isn't there
/// or doesn't read is as its resolver says ([`Resolve::try_resolve`]),
/// [`Why::Damaged`] where that's none.
impl Streams for Document {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        let object = match Resolve::try_resolve(&self, id) {
            Ok(Some(object)) => object,
            Ok(None) => return Err(crate::read::missing(id)),
            Err(unreadable) => return Err(unreadable),
        };
        let Some(stream) = object.as_stream() else {
            let message = format!("{id} R doesn't read: {}", Error::NotAStream { id });
            return Err(Unreadable::new(id, Why::Damaged, message));
        };
        if let Some(data) = (self.decoded.get(&id)).filter(|_| self.whole.contains(&id)) {
            return lent(id, data, limit);
        }
        match stream.data.bytes() {
            Some(data) => self
                .decoded_bytes(id, &stream.dict, data, limit)
                .map(Cow::Owned),
            None => Err(Unreadable::new(
                id,
                Why::Detached,
                format!(
                    "{id} R's data isn't held decoded: a document alone decodes only what it \
                     holds as bytes, and holds only the object streams its reader decoded whole"
                ),
            )),
        }
    }
}

impl Document {
    /// Stream `id`'s data, `data`, which the document holds as bytes,
    /// decoded through the filters its dictionary `dict` names, as a
    /// reading decodes a stream ([`Streams`] for a document).
    fn decoded_bytes(
        &self,
        id: ObjectId,
        dict: &Dict,
        data: &[u8],
        limit: usize,
    ) -> Result<Vec<u8>, Unreadable> {
        let resolve = |id| self.object(id);
        let within = limit.min(crate::read::STREAM);
        let capped = crate::read::STREAM < limit;
        let decoded = match crate::read::filters(dict, Some(&resolve)) {
            Err(why) => Err(Error::Filters(why)),
            Ok(filters) => {
                let flate = crate::read::Options::DEFAULT.flate;
                match pdf_filters::decode(data, &filters, within, flate) {
                    // Only a stream with no filters comes back longer than
                    // its limit: cut there, as a reading cuts one.
                    Ok(mut data) if data.len() > within => {
                        data.truncate(within);
                        Err(Error::Decode(pdf_filters::Error {
                            data,
                            faults: vec![(0, pdf_filters::Fault::Limit)],
                        }))
                    }
                    decoded => decoded.map_err(Error::Decode),
                }
            }
        };
        crate::read::viewed(id, decoded, capped)
    }
}

/// `data`, stream `id`'s decoded data as a reader holds it, lent where it's
/// within `limit`, else [`Why::Limit`] ([`Streams`]).
fn lent(id: ObjectId, data: &[u8], limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
    match data.len() <= limit {
        true => Ok(Cow::Borrowed(data)),
        false => {
            let message = format!(
                "{id} R's data decodes to {} bytes, past {limit}",
                data.len()
            );
            Err(Unreadable::new(id, Why::Limit, message))
        }
    }
}

/// Its objects, as a reference resolves them: none for one it hasn't, or
/// one that doesn't read, without why, which [`Resolve::try_resolve`] and
/// [`Source::get`] say.
impl<'o> Resolve<'o> for &'o Document {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        let doc: &'o Document = self;
        doc.object(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        let doc: &'o Document = self;
        match doc.objects.get(&id) {
            Some(Ok((object, _))) => Ok(Some(object)),
            Some(Err(why)) => why.unreadable(id).map_or(Ok(None), Err),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fmt;

    use pdf_crypt::Password;
    use pdf_objects::dict;
    use pdf_objects::object::Stream;

    use super::*;
    use crate::read::lazy::fixture;
    use crate::read::{Options, Prefix};

    /// What a reader outside the crate builds, and a writer's sections, can
    /// be had in a constant, as each is made by a `const fn`.
    #[test]
    fn what_a_reader_builds_can_be_had_in_a_constant() {
        use crate::write::{Section, fits};
        const TRAILER: Trailer = Trailer::new(Object::null());
        const OPENED: Encryption = Encryption::opened(true);
        const REVISION: Revision = Revision::new(9, Trailer::new(Object::null()), None);
        const UPDATE: Update = Update::new(BTreeSet::new(), 9);
        const UNREAD: Option<u32> = Unread::Cut { stream: 4 }.container();
        const FITS: [bool; 2] = [fits(10), fits(usize::MAX)];
        const SECTIONS: [Section; 2] = [Section::full(3), Section::append(7, 0)];
        assert_eq!(TRAILER, Trailer::new(Object::null()));
        assert_eq!(OPENED, Encryption::opened(true));
        assert_eq!((REVISION.at, REVISION.end), (9, None));
        assert_eq!((UPDATE.objects.len(), UPDATE.previous), (0, 9));
        assert_eq!(UNREAD, Some(4));
        assert_eq!(FITS, [true, false]);
        assert_eq!(SECTIONS.map(|section| section.size()), [3, 7]);
    }

    /// The strings in `value`, at any depth, a stream's dictionary's among
    /// them.
    fn strings(value: &Value, out: &mut Vec<Vec<u8>>) {
        let entries = |dict: &Dict, out: &mut Vec<Vec<u8>>| {
            (dict.entries().iter()).for_each(|entry| strings(entry.value.value(), out));
        };
        match value {
            Value::String(Str::Bytes(bytes), _) => out.push(bytes.clone()),
            Value::Array(items) => items.iter().for_each(|item| strings(item.value(), out)),
            Value::Dict(dict) => entries(dict, out),
            Value::Stream(stream) => entries(&stream.dict, out),
            _ => {}
        }
    }

    /// A document's `Debug` gives its objects' strings and a stream's data
    /// held in memory by their length, never their bytes, which may be
    /// decrypted: each string an encrypted file's reading decrypted, and
    /// each stream's bytes put in, in either form. The trailer's, of which
    /// the reader decrypts none, print as written.
    #[test]
    fn a_debug_gives_no_string_or_data_of_an_object() {
        for name in ["qpdf-aes-256", "qpdf-rc4-128"] {
            let mut doc = crate::read::document(&fixture(name), &Options::DEFAULT);
            assert!(
                matches!(doc.encryption(), Encryption::Opened { .. }),
                "{name}"
            );
            let data = b"plaintext of a stream held as bytes".to_vec();
            let stream = doc.add(Stream::new(dict! { "Type" => "XObject" }, data.clone()));
            let mut trailer = Vec::new();
            strings(&Value::Dict(doc.trailer().dict().clone()), &mut trailer);
            let mut decrypted = Vec::new();
            for object in doc.objects().map(|(_, object)| object) {
                strings(object.value(), &mut decrypted);
            }
            decrypted.retain(|bytes| !trailer.contains(bytes));
            assert!(!decrypted.is_empty(), "{name}");
            for shown in [format!("{doc:?}"), format!("{doc:#?}")] {
                for bytes in decrypted.iter().chain([&data]) {
                    assert!(!shown.contains(&format!("{bytes:?}")), "{name}: {shown}");
                }
                assert!(shown.contains(" bytes"), "{name}: {shown}");
            }
            let shown = format!("{doc:?}");
            let held = format!("data: Bytes({} bytes)", data.len());
            assert!(shown.contains(&held), "{name}: {shown}");
            let text = format!("String(Bytes({} bytes), Some(", decrypted[0].len());
            assert!(shown.contains(&text), "{name}: {shown}");
            assert!(doc.contains(stream));
        }
    }

    /// A document's `Debug` gives the object streams' decoded data and the
    /// metadata by their length, never their bytes, which may be decrypted,
    /// and the rest as its fields print, its objects among it, an object
    /// without strings as pdf-objects' `Debug` gives it. A prefix's gives
    /// its length and its source's, never the source's bytes.
    #[test]
    fn a_debug_gives_data_by_its_length() {
        let (file, _) = crate::read::tests::compressed(false);
        let mut doc = crate::read::document(&file, &Options::DEFAULT);
        let metadata = b"<x:xmpmeta>plaintext</x:xmpmeta>".to_vec();
        doc.set_metadata(Some(metadata.clone()));
        let stream = ObjectId::new(4, 0);
        let decoded = doc.held(stream).unwrap().to_vec();
        // Bytes as a derived `Debug` writes them, a number each.
        let numbers = |bytes: &[u8]| format!("{:?}", &bytes[..8]).replace(['[', ']'], "");
        let shown = format!("{doc:?}");
        for bytes in [&decoded, &metadata] {
            assert!(!shown.contains(&numbers(bytes)), "{shown}");
        }
        let held = format!("decoded: {{{stream:?}: {} bytes}}", decoded.len());
        assert!(shown.contains(&held), "{shown}");
        let given = format!("metadata: Some({} bytes)", metadata.len());
        assert!(shown.contains(&given), "{shown}");
        assert!(shown.contains(&format!("{:?}", doc.trailer())), "{shown}");
        let plain = (doc.objects().map(|(_, object)| object))
            .find(|object| {
                let mut found = Vec::new();
                strings(object.value(), &mut found);
                found.is_empty()
            })
            .unwrap();
        assert!(shown.contains(&format!("{plain:?}")), "{shown}");
        let pretty = format!("{doc:#?}");
        for len in [decoded.len(), metadata.len()] {
            assert!(pretty.contains(&format!("{len} bytes")), "{pretty}");
        }
        let prefix = Prefix::new(file.clone(), 9);
        let shown = format!("{prefix:?}");
        assert_eq!(
            shown,
            format!("Prefix {{ source: {} bytes, len: 9 }}", file.len())
        );
        assert!(!shown.contains(&numbers(&file)));
    }

    /// A record's `Debug` gives where its object was read, each of its
    /// public fields, and not the hash of its bytes, which is another in
    /// each process: of an object at an offset and of one in an object
    /// stream alike.
    #[test]
    fn a_records_debug_gives_no_hash() {
        let (file, _) = crate::read::tests::compressed(false);
        let doc = crate::read::document(&file, &Options::DEFAULT);
        let (mut at_offsets, mut members) = (0, 0);
        for (id, record) in doc.records() {
            match record.origin {
                Origin::Stream { .. } => members += 1,
                _ => at_offsets += 1,
            }
            let shown = format!("{record:?}");
            let Record {
                source,
                id: read_as,
                origin,
                at,
                endobj,
                data,
                hash,
            } = record;
            assert_eq!(
                shown,
                format!(
                    "Record {{ source: {source:?}, id: {read_as:?}, origin: {origin:?}, \
                     at: {at:?}, endobj: {endobj:?}, data: {data:?}, .. }}"
                ),
                "{id}"
            );
            let pretty = format!("{record:#?}");
            for shown in [shown, pretty] {
                assert!(!shown.contains("hash"), "{id}: {shown}");
                assert!(!shown.contains(&hash.to_string()), "{id}: {shown}");
            }
        }
        assert!(at_offsets > 0 && members > 0, "{at_offsets} and {members}");
    }

    /// Each of `items`, in turn, by `fold`: what a walk to the end gives.
    fn folded<T>(items: impl Iterator<Item = T>) -> Vec<T> {
        items.fold(Vec::new(), |mut all, item| {
            all.push(item);
            all
        })
    }

    /// Each of `items`, from the back, by `rfold`.
    fn rfolded<T>(items: impl DoubleEndedIterator<Item = T>) -> Vec<T> {
        items.rfold(Vec::new(), |mut all, item| {
            all.push(item);
            all
        })
    }

    /// `got`, a named iterator, walked beside `old`, the adapter it was:
    /// from the front, from the back, and from both ends in turn, each
    /// step's item and `size_hint` the same; done, it stays done; a clone
    /// taken part-way walks on as the rest of the adapter does; and walked
    /// to the end at once (`fold`, `rfold`), whole and part-way, it gives
    /// what the adapter gives.
    pub(crate) fn walked_alike<T, G, O>(got: G, old: O)
    where
        T: PartialEq + fmt::Debug,
        G: DoubleEndedIterator<Item = T> + FusedIterator + Clone,
        O: DoubleEndedIterator<Item = T> + Clone,
    {
        assert_eq!(folded(got.clone()), folded(old.clone()), "fold");
        assert_eq!(rfolded(got.clone()), rfolded(old.clone()), "rfold");
        let (mut part, mut was) = (got.clone(), old.clone());
        assert_eq!(
            (part.next(), part.next_back()),
            (was.next(), was.next_back())
        );
        assert_eq!(folded(part.clone()), folded(was.clone()), "fold, part-way");
        assert_eq!(rfolded(part), rfolded(was), "rfold, part-way");
        for turns in [&[false][..], &[true], &[false, true]] {
            let (mut got, mut old) = (got.clone(), old.clone());
            for step in 0.. {
                assert_eq!(got.size_hint(), old.size_hint(), "{step} of {turns:?}");
                let (g, o) = match turns[step % turns.len()] {
                    false => (got.next(), old.next()),
                    true => (got.next_back(), old.next_back()),
                };
                assert_eq!(g, o, "{step} of {turns:?}");
                if g.is_none() {
                    break;
                }
                if step == 1 {
                    let rest: Vec<T> = got.clone().collect();
                    assert_eq!(rest, old.clone().collect::<Vec<T>>(), "a clone");
                }
            }
            assert_eq!((got.next(), got.next_back()), (None, None), "done");
        }
    }

    /// [`walked_alike`], for an iterator that walks one way.
    pub(crate) fn walked_alike_forward<T, G, O>(got: G, old: O)
    where
        T: PartialEq + fmt::Debug,
        G: Iterator<Item = T> + FusedIterator + Clone,
        O: Iterator<Item = T> + Clone,
    {
        assert_eq!(folded(got.clone()), folded(old.clone()), "fold");
        let (mut part, mut was) = (got.clone(), old.clone());
        assert_eq!(part.next(), was.next());
        assert_eq!(folded(part), folded(was), "fold, part-way");
        let (mut got, mut old) = (got, old);
        for step in 0.. {
            assert_eq!(got.size_hint(), old.size_hint(), "{step}");
            let (g, o) = (got.next(), old.next());
            assert_eq!(g, o, "{step}");
            if g.is_none() {
                break;
            }
            if step == 1 {
                let rest: Vec<T> = got.clone().collect();
                assert_eq!(rest, old.clone().collect::<Vec<T>>(), "a clone");
            }
        }
        assert_eq!(got.next(), None, "done");
    }

    /// A document's iterators walk as the adapters they were, over what it
    /// holds: objects that read, with and without records, and ones that
    /// don't; built, and read whole from a file. Its objects, each range of
    /// them a caller asks, those that don't read and the records.
    #[test]
    fn a_documents_iterators_walk_as_they_did() {
        let mut built = Document::new(Version::new(1, 7), dict! { "Root" => Object::reference(1) });
        built.insert(1, dict! { "Type" => "Catalog" });
        built.insert(2, 2);
        built.set(ObjectId::new(6, 2), "Six");
        built.set_unread(
            ObjectId::new(3, 0),
            Unread::Other("it doesn't parse".to_string()),
        );
        built.set_unread(ObjectId::new(8, 0), Unread::Other("nor this".to_string()));
        let file = pdf_fixtures::fixture::assembled(
            "1.7",
            &[
                (1, &b"<< /Type /Catalog >>"[..]),
                (2, b"<< /A 1 >>"),
                (3, b"<< /Unended ("),
                (4, b"42"),
                (5, b"(five)"),
            ],
            "<< /Size 6 /Root 1 0 R >>",
        );
        let mut read = crate::read::document(&file, &crate::read::Options::DEFAULT);
        read.get_mut(ObjectId::new(4, 0));
        let empty = Document::new(Version::new(1, 7), Dict::new());
        let ranges = [
            (ObjectId::new(2, 0)..ObjectId::new(6, 0)),
            (ObjectId::new(0, 0)..ObjectId::new(9, 0)),
            (ObjectId::new(6, 0)..ObjectId::new(6, u16::MAX)),
            (ObjectId::new(3, 0)..ObjectId::new(4, 0)),
        ];
        // The adapters as they were.
        fn objects<'d>((&id, held): (&ObjectId, &'d Held)) -> Option<(ObjectId, &'d Object)> {
            Some((id, &held.as_ref().ok()?.0))
        }
        fn unread<'d>((&id, held): (&ObjectId, &'d Held)) -> Option<(ObjectId, &'d Unread)> {
            Some((id, held.as_ref().err()?))
        }
        fn records<'d>((&id, held): (&ObjectId, &'d Held)) -> Option<(ObjectId, &'d Record)> {
            Some((id, held.as_ref().ok()?.1.as_ref()?))
        }
        for doc in [&built, &read, &empty] {
            let held = doc.objects.iter();
            walked_alike(doc.objects(), held.clone().filter_map(objects));
            for range in ranges.clone() {
                let old = doc.objects.range(range.clone()).filter_map(objects);
                walked_alike(doc.range(range), old);
            }
            walked_alike(doc.range(..), doc.objects.range(..).filter_map(objects));
            walked_alike(doc.unread(), held.clone().filter_map(unread));
            walked_alike(doc.records(), held.clone().filter_map(records));
        }
        assert_eq!(read.records().count(), 3, "4 edited, 3 unread");
        assert_eq!(read.unread().count(), 1);
    }

    #[test]
    fn a_document_gives_what_it_holds() {
        let mut doc = Document::new(Version::new(1, 7), dict! { "Root" => Object::reference(1) });
        doc.insert(1, dict! { "Type" => "Catalog" });
        let why = Unread::Other("it doesn't parse".to_string());
        doc.set_unread(ObjectId::new(2, 0), why.clone());
        assert_eq!(doc.ids(), [ObjectId::new(1, 0), ObjectId::new(2, 0)]);
        assert!(
            doc.get(ObjectId::new(1, 0))
                .is_ok_and(|o| o.as_dict().is_some())
        );
        assert_eq!(doc.get(ObjectId::new(2, 0)), Err(why));
        assert_eq!(
            doc.get(ObjectId::new(3, 0)),
            Err(Unread::Missing {
                id: ObjectId::new(3, 0)
            })
        );
        assert_eq!(
            doc.get(ObjectId::new(3, 0)).unwrap_err().to_string(),
            "3 0 R isn't in the file"
        );
        assert!(doc.trailer().dict().has(b"Root"));
        assert_eq!(doc.header_version(), Some(Version::new(1, 7)));
    }

    /// The metadata a document read is the catalog's metadata stream's
    /// while that stream is the one it was read from and as read: an edit
    /// of the catalog that leaves `/Metadata` where it was keeps it; the
    /// stream put, taken to edit, set unread or removed, the catalog's
    /// `/Metadata` removed or the trailer's `/Root` another drops it, for
    /// the document and through [`Source`] alike. What's set is given as
    /// set, whatever's edited after.
    #[test]
    fn a_documents_metadata_is_the_catalogs_while_its_stream_is_as_read() {
        let xmp = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">as read</x:xmpmeta>";
        let stream = format!(
            "<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{}\nendstream",
            xmp.len(),
            String::from_utf8_lossy(xmp)
        );
        let file = pdf_fixtures::fixture::assembled(
            "1.7",
            &[
                (1, b"<< /Type /Catalog /Pages 3 0 R /Metadata 2 0 R >>"),
                (2, stream.as_bytes()),
                (3, b"<< /Type /Pages /Kids [] /Count 0 >>"),
                (4, b"<< /Type /Catalog /Pages 3 0 R >>"),
            ],
            "<< /Size 5 /Root 1 0 R >>",
        );
        let read = crate::read::document(&file, &Options::DEFAULT);
        assert_eq!(read.metadata(), Some(&xmp[..]));
        let given = |doc: &Document| {
            let source = Source::metadata(doc).map(Cow::into_owned);
            assert_eq!(source.as_deref(), doc.metadata());
            source
        };
        let mut doc = read.clone();
        doc.catalog_mut().unwrap().set("Lang", Object::from("en"));
        assert_eq!(given(&doc).as_deref(), Some(&xmp[..]), "the catalog edited");
        let metadata = ObjectId::new(2, 0);
        let new = Stream::new(dict! { "Type" => "Metadata" }, b"new".to_vec());
        let edit = |doc: &mut Document, what: &str| match what {
            "put" => drop(doc.set(metadata, new.clone())),
            "taken to edit" => {
                doc.get_mut(metadata);
            }
            "set unread" => drop(doc.set_unread(metadata, Unread::Other("gone".into()))),
            "removed" => drop(doc.remove(metadata)),
            "its /Metadata removed" => drop(doc.catalog_mut().unwrap().remove(b"Metadata")),
            _ => {
                let root = Object::reference(4);
                doc.trailer_mut().dict_mut().set("Root", root);
            }
        };
        let edits = [
            "put",
            "taken to edit",
            "set unread",
            "removed",
            "its /Metadata removed",
            "another /Root",
        ];
        for what in edits {
            let mut doc = read.clone();
            edit(&mut doc, what);
            assert_eq!(given(&doc), None, "{what}");
            let mut doc = read.clone();
            doc.set_metadata(Some(b"set".to_vec()));
            edit(&mut doc, what);
            assert_eq!(given(&doc).as_deref(), Some(&b"set"[..]), "{what}, set");
        }
    }

    /// A reference resolves to an object that reads; to none where the
    /// document has none, or one that doesn't read, as either is null.
    #[test]
    fn a_document_resolves_the_objects_that_read() {
        let mut doc = Document::new(Version::new(1, 7), dict! { "Root" => Object::reference(1) });
        doc.insert(1, dict! { "Type" => "Catalog" });
        let why = Unread::Other("it doesn't parse".to_string());
        doc.set_unread(ObjectId::new(2, 0), why);
        let resolve = &doc;
        let catalog = resolve.resolve(ObjectId::new(1, 0)).unwrap();
        assert!(std::ptr::eq(catalog, doc.get(ObjectId::new(1, 0)).unwrap()));
        assert_eq!(resolve.resolve(ObjectId::new(2, 0)), None);
        assert_eq!(resolve.resolve(ObjectId::new(3, 0)), None);
    }

    /// A document's objects by number, in order and in a range of ids,
    /// beside those that don't read; what it lists, read or not, by `ids`
    /// and `contains`.
    #[test]
    fn a_documents_objects_are_read_by_number_in_order_and_in_a_range() {
        let mut doc = Document::new(Version::new(1, 7), dict! { "Root" => Object::reference(1) });
        assert!(doc.ids().is_empty());
        assert_eq!(doc.objects().next(), None);
        doc.insert(1, dict! { "Type" => "Catalog" });
        doc.insert(2, 2);
        doc.insert(5, 5);
        doc.set(ObjectId::new(6, 2), "Six");
        doc.insert(7, 7);
        let why = Unread::Other("it doesn't parse".to_string());
        doc.set_unread(ObjectId::new(3, 0), why.clone());
        fn ids<'o>(objects: impl Iterator<Item = (ObjectId, &'o Object)>) -> Vec<ObjectId> {
            objects.map(|(id, _)| id).collect()
        }

        assert_eq!(
            doc.ids(),
            [
                ObjectId::new(1, 0),
                ObjectId::new(2, 0),
                ObjectId::new(3, 0),
                ObjectId::new(5, 0),
                ObjectId::new(6, 2),
                ObjectId::new(7, 0)
            ]
        );
        assert_eq!(doc.ids().len(), 6);
        assert!(doc.contains(ObjectId::new(3, 0)) && doc.contains(ObjectId::new(6, 2)));
        assert!(!doc.contains(ObjectId::new(4, 0)) && !doc.contains(ObjectId::new(6, 0)));
        assert_eq!(
            ids(doc.objects()),
            [
                ObjectId::new(1, 0),
                ObjectId::new(2, 0),
                ObjectId::new(5, 0),
                ObjectId::new(6, 2),
                ObjectId::new(7, 0)
            ]
        );
        assert!(
            doc.objects()
                .all(|(id, o)| std::ptr::eq(o, doc.get(id).unwrap()))
        );
        assert_eq!(ids(doc.objects().rev()).first(), Some(&ObjectId::new(7, 0)));
        assert_eq!(
            doc.unread().collect::<Vec<_>>(),
            [(ObjectId::new(3, 0), &why)]
        );
        assert_eq!(doc.get(ObjectId::new(3, 0)), Err(why));
        assert_eq!(
            doc.get(ObjectId::new(4, 0)),
            Err(Unread::Missing {
                id: ObjectId::new(4, 0)
            })
        );

        // Ranges as a caller asks them: between two ids, up to one, from
        // one, and a number's every generation.
        assert_eq!(
            ids(doc.range(ObjectId::new(2, 0)..ObjectId::new(6, 0))),
            [ObjectId::new(2, 0), ObjectId::new(5, 0)]
        );
        assert_eq!(
            ids(doc.range(..ObjectId::new(5, 0))),
            [ObjectId::new(1, 0), ObjectId::new(2, 0)]
        );
        assert_eq!(
            ids(doc.range(ObjectId::new(5, 0)..)),
            [
                ObjectId::new(5, 0),
                ObjectId::new(6, 2),
                ObjectId::new(7, 0)
            ]
        );
        assert_eq!(
            ids(doc.range(ObjectId::new(6, 0)..=ObjectId::new(6, u16::MAX))),
            [ObjectId::new(6, 2)]
        );
        assert_eq!(
            ids(doc.range(ObjectId::new(3, 0)..=ObjectId::new(4, 0))),
            []
        );
        assert_eq!(
            ids(doc.range(..ObjectId::new(5, 0)).rev()).first(),
            Some(&ObjectId::new(2, 0))
        );
    }

    /// A record is always of the object beside it: an object put in has
    /// none, one replaced, removed or taken to edit loses its own, one
    /// that doesn't read has none, and the rest keep theirs, each still
    /// where its object was written; a clone keeps them all.
    #[test]
    fn each_write_keeps_the_records_true() {
        let file = pdf_fixtures::fixture::assembled(
            "1.7",
            &[
                (1, &b"<< /Type /Catalog >>"[..]),
                (2, b"<< /A 1 >>"),
                (3, b"<< /Length 3 >>\nstream\nabc\nendstream"),
                (4, b"42"),
                (5, b"(five)"),
                (6, b"<< /Six 6 >>"),
                (7, b"<< /A >> >>"),
            ],
            "<< /Size 8 /Root 1 0 R >>",
        );
        let read = crate::read::document(&file, &crate::read::Options::DEFAULT);
        // Each record says where its object was written.
        let true_to_the_file = |doc: &Document| {
            for (id, record) in doc.records() {
                let written = &file[record.at.start..record.at.end];
                let header = format!("{id} obj");
                assert!(written.starts_with(header.as_bytes()), "{id:?}");
                assert_eq!(
                    doc.get(id).unwrap().at.map(|at| at.end <= record.at.end),
                    Some(true)
                );
            }
        };
        let ids = |doc: &Document| -> Vec<ObjectId> { doc.records().map(|(id, _)| id).collect() };
        assert_eq!(
            read.unread().map(|(id, _)| id).collect::<Vec<_>>(),
            [ObjectId::new(7, 0)]
        );
        let reads: Vec<ObjectId> = read.objects().map(|(id, _)| id).collect();
        assert_eq!(ids(&read), reads);
        assert_eq!(read.record(ObjectId::new(7, 0)), None);
        true_to_the_file(&read);

        // A clone keeps every record, its reading's id too.
        let mut doc = read.clone();
        assert_eq!(doc, read);
        assert!(doc.records().eq(read.records()));
        assert!((doc.records().zip(read.records())).all(|((_, a), (_, b))| a.source == b.source));

        // Put in: no record, giving back what it replaces where that read.
        let catalog = read.get(ObjectId::new(1, 0)).unwrap().clone();
        assert_eq!(
            doc.set(ObjectId::new(1, 0), dict! { "Type" => "Catalog" }),
            Some(catalog)
        );
        assert_eq!(doc.record(ObjectId::new(1, 0)), None);
        assert_eq!(doc.set(ObjectId::new(8, 0), 8), None);
        doc.insert(9, 9);
        assert_eq!(
            (
                doc.record(ObjectId::new(8, 0)),
                doc.record(ObjectId::new(9, 0))
            ),
            (None, None)
        );
        assert_eq!(
            doc.set(ObjectId::new(7, 0), 7),
            None,
            "an object that didn't read"
        );
        assert_eq!(doc.record(ObjectId::new(7, 0)), None);

        // Why one doesn't read: no record, giving back what read there.
        let why = Unread::Other("set aside".to_string());
        let two = read.get(ObjectId::new(2, 0)).unwrap().clone();
        assert_eq!(doc.set_unread(ObjectId::new(2, 0), why.clone()), Some(two));
        assert_eq!(doc.record(ObjectId::new(2, 0)), None);
        assert_eq!(doc.get(ObjectId::new(2, 0)), Err(why.clone()));
        assert_eq!(doc.set_unread(ObjectId::new(10, 0), why.clone()), None);

        // Removed: the object, its record with it; one that doesn't read,
        // nothing, and it's gone too.
        let stream = read.get(ObjectId::new(3, 0)).unwrap().clone();
        assert_eq!(doc.remove(ObjectId::new(3, 0)), Some(stream));
        assert!(!doc.contains(ObjectId::new(3, 0)) && doc.record(ObjectId::new(3, 0)).is_none());
        assert_eq!(doc.remove(ObjectId::new(10, 0)), None);
        assert!(!doc.contains(ObjectId::new(10, 0)));
        assert_eq!(doc.remove(ObjectId::new(3, 0)), None);

        // Taken to edit, the record goes though nothing changes, as a
        // `&mut` could have replaced the object whole; one that doesn't
        // read, or isn't listed, isn't given.
        let four = read.get(ObjectId::new(4, 0)).unwrap().clone();
        assert_eq!(doc.get_mut(ObjectId::new(4, 0)), Some(&mut four.clone()));
        assert_eq!(doc.get(ObjectId::new(4, 0)), Ok(&four));
        assert_eq!(doc.record(ObjectId::new(4, 0)), None);
        assert_eq!(doc.get_mut(ObjectId::new(2, 0)), None);
        assert_eq!(doc.get(ObjectId::new(2, 0)), Err(why));
        assert_eq!(doc.get_mut(ObjectId::new(11, 0)), None);
        assert!(!doc.contains(ObjectId::new(11, 0)));

        // What wasn't written to keeps its record, true to the file; and
        // a document whose records went isn't the one read.
        assert_eq!(ids(&doc), [ObjectId::new(5, 0), ObjectId::new(6, 0)]);
        assert_eq!(
            doc.record(ObjectId::new(5, 0)),
            read.record(ObjectId::new(5, 0))
        );
        true_to_the_file(&doc);
        let mut edited = read.clone();
        assert!(edited.get_mut(ObjectId::new(5, 0)).is_some());
        assert!(edited.objects().eq(read.objects()));
        assert_ne!(edited, read);

        // Every object taken to edit, as a map's values would be: none
        // keeps a record.
        for id in edited.ids() {
            if let Some(object) = edited.get_mut(id) {
                object.at = None;
            }
        }
        assert_eq!(edited.records().next(), None);
        assert_eq!(edited.objects().count(), reads.len());
    }

    /// A document's header version, where the file's newest
    /// cross-reference section starts and how it's written, and whether
    /// the file is encrypted and opened.
    #[test]
    fn a_document_says_its_version_newest_section_and_encryption() {
        let startxref = |file: &[u8]| -> usize {
            let at = file.windows(9).rposition(|w| w == b"startxref").unwrap() + 9;
            let digits: Vec<u8> = (file[at..].iter())
                .skip_while(|b| b.is_ascii_whitespace())
                .take_while(|b| b.is_ascii_digit())
                .copied()
                .collect();
            String::from_utf8(digits).unwrap().parse().unwrap()
        };
        let file = pdf_fixtures::fixture::assembled(
            "1.6",
            &[(1, &b"<< /Type /Catalog >>"[..])],
            "<< /Size 2 /Root 1 0 R >>",
        );
        let doc = crate::read::document(&file, &crate::read::Options::DEFAULT);
        assert_eq!(doc.header, Some(Version::new(1, 6)));
        let newest = Some((startxref(&file), XrefKind::Table));
        assert_eq!(doc.facts.newest, newest);
        assert_eq!(doc.encryption, Encryption::None);

        // qpdf's file, its dictionaries in object streams, encrypted with
        // AES-256 and opened by its empty user password.
        let file = fixture("qpdf-aes-256");
        let doc = crate::read::document(&file, &crate::read::Options::DEFAULT);
        assert_eq!(doc.header, Some(Version::new(1, 7)));
        assert_eq!(doc.facts.newest, Some((startxref(&file), XrefKind::Stream)));
        assert_ne!(doc.encryption, Encryption::None);
        assert_eq!(
            doc.encryption,
            Encryption::opened(false).permitting(pdf_crypt::Permissions::ALL)
        );
    }

    /// Each reason an object doesn't read says it as the reader's text
    /// for it did, by the same format string.
    #[test]
    fn each_reason_an_object_doesnt_read_is_said_as_it_was() {
        let (stream, index, number, offset) = (4, 1, 9, 120);
        let id = ObjectId::new(3, 1);
        let parse = parse::Error::Header { at: 7 };
        let why = Undecoded::NotAStream;
        let inner = Unread::Parse(parse.clone());
        let cases = [
            (Unread::Missing { id }, format!("{id} R isn't in the file")),
            (Unread::Parse(parse.clone()), parse.to_string()),
            (
                Unread::Elsewhere {
                    offset,
                    found: ObjectId::new(5, 0),
                    expected: id,
                },
                format!("byte {offset} holds 5 0 obj, where the cross-references put {id} R"),
            ),
            (
                Unread::Container {
                    stream,
                    why: why.clone(),
                },
                format!("its object stream {stream} 0 R doesn't decode: {why}"),
            ),
            (
                Unread::Undecodable { stream },
                format!("it's in object stream {stream} 0 R, and nothing decodes that"),
            ),
            (
                Unread::ContainerUnread {
                    stream,
                    why: Box::new(inner.clone()),
                },
                format!("its object stream {stream} 0 R doesn't read: {inner}"),
            ),
            (
                Unread::ContainerMissing { stream },
                format!("its object stream {stream} 0 R isn't in the file"),
            ),
            (
                Unread::ContainerHeader {
                    stream,
                    why: Unindexed::Counts,
                },
                "its /N or /First isn't a count".to_string(),
            ),
            (
                Unread::NoIndex { stream, index },
                format!("object stream {} 0 R has no object {index}", stream),
            ),
            (
                Unread::Numbered {
                    stream,
                    index,
                    number,
                    expected: id.number,
                },
                format!(
                    "object {index} of object stream {} 0 R is {number}, not {}",
                    stream, id.number
                ),
            ),
            (
                Unread::NoData { stream },
                format!("object stream {} 0 R has no data", stream),
            ),
            (
                Unread::Cut { stream },
                format!(
                    "it runs to where a fault cut object stream {} 0 R's data off, so it may be cut short",
                    stream
                ),
            ),
            (
                Unread::Source {
                    message: "no byte 7".to_string(),
                },
                "no byte 7".to_string(),
            ),
            (
                Unread::Other("qpdf's words".to_string()),
                "qpdf's words".to_string(),
            ),
        ];
        for (unread, said) in cases {
            assert_eq!(unread.to_string(), said, "{unread:?}");
        }
    }

    /// Each reason an object doesn't read, as a document's resolver says it:
    /// by the kind a caller decides by, the reason's words after the object's
    /// number; and none, with no reason, where the file hasn't it.
    #[test]
    fn a_document_says_why_an_object_doesnt_resolve() {
        let stream = 4;
        let id = ObjectId::new(9, 1);
        let filters = |why| Unread::Container {
            stream,
            why: Undecoded::Filters(why),
        };
        let container = |why| Unread::Container { stream, why };
        let parse_error = |error| Unread::Parse(error);
        let boxed = |why: Unread| Unread::ContainerUnread {
            stream,
            why: Box::new(why),
        };
        let cases = [
            (parse_error(parse::Error::End), Why::Damaged),
            (parse_error(parse::Error::Header { at: 7 }), Why::Damaged),
            (
                parse_error(parse::Error::Nested { limit: 2, at: 7 }),
                Why::Capped,
            ),
            (
                parse_error(parse::Error::Values { limit: 2, at: 7 }),
                Why::Capped,
            ),
            (
                Unread::Elsewhere {
                    offset: 5,
                    found: ObjectId::new(1, 0),
                    expected: id,
                },
                Why::Damaged,
            ),
            (container(Undecoded::NotAStream), Why::Damaged),
            (filters(Unfiltered::Filter), Why::Damaged),
            (filters(Unfiltered::Filters { count: 26 }), Why::Capped),
            (
                container(Undecoded::Encrypted {
                    filter: b"StdCF".to_vec(),
                }),
                Why::Locked,
            ),
            (
                container(Undecoded::Decrypt(pdf_crypt::Fault::Short { len: 3 })),
                Why::Locked,
            ),
            (
                container(Undecoded::Other("words".to_string())),
                Why::Damaged,
            ),
            (Unread::Undecodable { stream }, Why::Locked),
            (boxed(Unread::Undecodable { stream }), Why::Locked),
            (
                boxed(parse_error(parse::Error::Nested { limit: 2, at: 7 })),
                Why::Capped,
            ),
            (boxed(parse_error(parse::Error::End)), Why::Damaged),
            // The stream isn't in the file, though the object is listed.
            (
                boxed(Unread::Missing {
                    id: ObjectId::new(stream, 0),
                }),
                Why::Damaged,
            ),
            (Unread::ContainerMissing { stream }, Why::Damaged),
            (
                Unread::ContainerHeader {
                    stream,
                    why: Unindexed::Counts,
                },
                Why::Damaged,
            ),
            (Unread::NoIndex { stream, index: 2 }, Why::Damaged),
            (
                Unread::Numbered {
                    stream,
                    index: 1,
                    number: 8,
                    expected: id.number,
                },
                Why::Damaged,
            ),
            (Unread::NoData { stream }, Why::Damaged),
            (Unread::Cut { stream }, Why::Damaged),
            (
                Unread::Source {
                    message: "no byte 7".to_string(),
                },
                Why::Source,
            ),
            (Unread::Other("words".to_string()), Why::Damaged),
        ];
        let mut doc = Document::new(Version::new(1, 7), dict! {});
        let object = doc.add(Object::from(7));
        for (unread, why) in cases {
            doc.set_unread(id, unread.clone());
            let resolve = &doc;
            assert_eq!(resolve.resolve(id), None, "{unread:?}");
            let said = resolve.try_resolve(id).unwrap_err();
            assert_eq!((said.id, said.why), (id, why), "{unread:?}");
            assert_eq!(said.message, format!("9 1 R doesn't read: {unread}"));
            assert_eq!(said.to_string(), said.message);
        }
        let resolve = &doc;
        assert_eq!(resolve.try_resolve(object), Ok(resolve.resolve(object)));
        assert!(resolve.try_resolve(object).unwrap().is_some());
        assert_eq!(resolve.try_resolve(ObjectId::new(77, 0)), Ok(None));
        doc.set_unread(id, Unread::Missing { id });
        assert_eq!((&doc).try_resolve(id), Ok(None));
    }

    /// The object stream an object that doesn't read is in, for each reason
    /// that's its stream's or its place in it; none for the rest, an object
    /// in one that doesn't parse among them.
    #[test]
    fn why_an_object_doesnt_read_says_the_object_stream_its_in_where_its_why() {
        let stream = 4;
        let inner = Unread::Parse(parse::Error::Header { at: 7 });
        let placed = [
            Unread::Container {
                stream,
                why: Undecoded::NotAStream,
            },
            Unread::Undecodable { stream },
            Unread::ContainerUnread {
                stream,
                why: Box::new(inner.clone()),
            },
            Unread::ContainerMissing { stream },
            Unread::ContainerHeader {
                stream,
                why: Unindexed::Counts,
            },
            Unread::NoIndex { stream, index: 2 },
            Unread::Numbered {
                stream,
                index: 1,
                number: 9,
                expected: 3,
            },
            Unread::NoData { stream },
            Unread::Cut { stream },
        ];
        for why in &placed {
            assert_eq!(why.container(), Some(stream), "{why:?}");
        }
        let unplaced = [
            Unread::Missing {
                id: ObjectId::new(3, 0),
            },
            inner,
            Unread::Elsewhere {
                offset: 120,
                found: ObjectId::new(5, 0),
                expected: ObjectId::new(3, 0),
            },
            Unread::Source {
                message: "no byte 7".to_string(),
            },
            Unread::Other("qpdf's words".to_string()),
        ];
        for why in &unplaced {
            assert_eq!(why.container(), None, "{why:?}");
        }
    }

    /// A reader outside the crate makes a revision, as it does an update,
    /// and says where it ends.
    #[test]
    fn a_revision_is_made_outside_the_crate_by_new() {
        let trailer = Trailer::new(dict! { "Size" => 3 }.into());
        let first = Revision::new(9, trailer.clone(), None);
        assert_eq!(
            (first.at, &first.trailer, &first.numbers, first.end),
            (9, &trailer, &None, None)
        );
        let numbers: Box<[u32]> = Box::new([2, 5]);
        let later = Revision::new(120, trailer, Some(numbers.clone()));
        assert_eq!(later.numbers, Some(numbers));
        assert_ne!(first, later);
        let end = RevisionEnd::new(100, 102).unwrap();
        assert_eq!((end.eof(), end.line()), (100, 102));
        let ended = first.clone().ending(end);
        assert_eq!(ended.end, Some(end));
        assert_eq!((ended.at, &ended.trailer), (first.at, &first.trailer));
        assert_ne!(ended, first);
        // A line's end before its `%%EOF`'s is none; at it, an end.
        assert_eq!(RevisionEnd::new(102, 100), None);
        assert_eq!(RevisionEnd::new(100, 100).map(RevisionEnd::line), Some(100));
    }

    /// A reader of its own, implementing `Source`'s five methods alone,
    /// and `Streams` with none of the file's bytes, gives none of what a
    /// reader may know besides: no record, metadata, object stream's data,
    /// revisions or facts, and no object stream in the clear. A document
    /// gives what it holds, lent, through the trait as through itself.
    #[test]
    fn a_source_says_what_a_reader_knows_besides_where_it_knows_it() {
        struct Own(Trailer);
        impl Streams for Own {
            fn decoded(&self, id: ObjectId, _: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
                Err(Unreadable::new(id, Why::Detached, "it has no bytes"))
            }
        }
        impl Source for Own {
            fn ids(&self) -> Vec<ObjectId> {
                Vec::new()
            }
            fn get(&self, id: ObjectId) -> Result<&Object, Unread> {
                Err(Unread::Missing { id })
            }
            fn trailer(&self) -> &Trailer {
                &self.0
            }
            fn header_version(&self) -> Option<Version> {
                None
            }
            fn faults(&self) -> Cow<'_, [(Place, Fault)]> {
                Cow::Borrowed(&[])
            }
        }
        let own = Own(Trailer::new(dict! {}.into()));
        assert_eq!(own.record(ObjectId::new(1, 0)), None);
        assert_eq!(own.metadata(), None);
        assert_eq!(own.held(ObjectId::new(4, 0)), None);
        assert_eq!(own.decoded_within(ObjectId::new(4, 0), 10).map(drop), None);
        let source: &dyn Source = &own;
        let ungiven = Streams::decoded(source, ObjectId::new(4, 0), 10).unwrap_err();
        assert_eq!(
            (ungiven.id, ungiven.why),
            (ObjectId::new(4, 0), Why::Detached)
        );
        assert!(own.revisions().is_empty());
        assert_eq!(own.facts(), None);
        assert!(!own.in_the_clear(ObjectId::new(4, 0)));

        // qpdf's file of object streams, read whole.
        let file = fixture("qpdf-compressed");
        let mut doc = crate::read::document(&file, &crate::read::Options::DEFAULT);
        let source: &dyn Source = &doc;
        let member = (doc.objects())
            .find_map(|(id, o)| Some((id, o.at?.stream?)))
            .expect("a member");
        let (id, stream) = member;
        assert_eq!(source.record(id), doc.record(id));
        assert!(source.record(id).is_some());
        assert_eq!(source.held(stream), Some(&doc.decoded[&stream][..]));
        assert_eq!(source.held(ObjectId::new(1_000_000, 0)), None);
        assert!(matches!(source.faults(), Cow::Borrowed(w) if w == doc.faults()));
        assert_eq!(source.revisions(), &doc.revisions[..]);
        assert_eq!(source.revisions().len(), 1);
        assert_eq!(source.facts(), Some(&doc.facts));
        assert!(!source.in_the_clear(stream));
        doc.in_the_clear.insert(stream);
        assert!(Source::in_the_clear(&doc, stream));

        // One built has the defaults, and no revisions.
        let built = Document::new(Version::new(1, 7), dict! {});
        assert!(Source::revisions(&built).is_empty());
        assert_eq!(Source::facts(&built), Some(&Facts::default()));
        assert!(built.in_the_clear.is_empty());
    }

    /// Where `needle` first is in `file`.
    fn find(file: &[u8], needle: &[u8]) -> usize {
        file.windows(needle.len())
            .position(|w| w == needle)
            .unwrap()
    }

    /// New ids, one at a time, each generation 0: past every number a
    /// file's cross-references list, in use or free, past its
    /// cross-reference stream's, and past its trailer's `/Size` less one;
    /// for a document built in memory, its trailer's `/Size` or 1; past
    /// each number put in since, never 0; and after where `set_max_id`
    /// puts them, but never at a number held.
    #[test]
    fn new_ids_start_past_every_number_the_file_lists() {
        // Built in memory, under a trailer with no /Size.
        let mut doc = Document::new(Version::new(1, 7), Dict::new());
        assert_eq!(doc.max_id(), 0);
        assert_eq!(
            (doc.new_id(), doc.new_id()),
            (ObjectId::new(1, 0), ObjectId::new(2, 0))
        );
        assert_eq!(doc.max_id(), 2);
        assert_eq!(doc.add(dict! { "Type" => "Catalog" }), ObjectId::new(3, 0));
        assert!(
            doc.get_dict(ObjectId::new(3, 0))
                .is_some_and(|d| d.has_type(b"Catalog"))
        );
        assert_eq!(doc.record(ObjectId::new(3, 0)), None);
        assert_eq!(
            doc.ids(),
            [ObjectId::new(3, 0)],
            "an id handed out isn't held"
        );

        // Past a /Size, where that's a number of objects; else from 1.
        let sized = |size: Object| Document::new(Version::new(1, 7), dict! { "Size" => size });
        assert_eq!(sized(8.into()).new_id(), ObjectId::new(8, 0));
        assert_eq!(sized(8.into()).max_id(), 7);
        for size in [
            0.into(),
            (-3).into(),
            8.5.into(),
            (1i64 << 32).into(),
            (1i64 << 33).into(),
            Object::reference(8),
        ] {
            assert_eq!(
                sized(size.clone()).new_id(),
                ObjectId::new(1, 0),
                "{size:?}"
            );
        }

        // Past what's put in, read or not; one put in below leaves them.
        let mut doc = sized(8.into());
        doc.set(ObjectId::new(20, 3), "Twenty");
        assert_eq!(doc.new_id(), ObjectId::new(21, 0));
        doc.set_unread(
            ObjectId::new(30, 0),
            Unread::Other("it doesn't parse".to_string()),
        );
        assert_eq!(doc.add(31), ObjectId::new(31, 0));
        doc.insert(40, 40);
        doc.insert(5, 5);
        assert_eq!(doc.max_id(), 40);
        assert_eq!(doc.new_id(), ObjectId::new(41, 0));

        // Lowered, but never below a number held, read or not.
        doc.set_max_id(9);
        assert_eq!(doc.max_id(), 40);
        assert_eq!(doc.remove(ObjectId::new(40, 0)), Some(40.into()));
        assert_eq!(doc.max_id(), 40, "removing leaves where they start");
        doc.set_max_id(9);
        assert_eq!(doc.max_id(), 31);
        doc.remove(ObjectId::new(31, 0));
        doc.remove(ObjectId::new(30, 0));
        doc.set_max_id(9);
        assert_eq!(doc.new_id(), ObjectId::new(21, 0));
        doc.set_max_id(100);
        assert_eq!(doc.new_id(), ObjectId::new(101, 0));
        // Object 0, always free, is never one, even held.
        let mut doc = sized(0.into());
        doc.set(ObjectId::new(0, 0), 0);
        doc.set_max_id(0);
        assert_eq!((doc.max_id(), doc.new_id()), (0, ObjectId::new(1, 0)));

        // Read: past the trailer's /Size less one, which is past what the
        // table lists.
        let file = pdf_fixtures::fixture::assembled(
            "1.7",
            &[(1, &b"<< /Type /Catalog >>"[..]), (2, b"2")],
            "<< /Size 20 /Root 1 0 R >>",
        );
        let mut doc = crate::read::document(&file, &crate::read::Options::DEFAULT);
        assert_eq!(doc.max_id(), 19);
        assert_eq!(doc.new_id(), ObjectId::new(20, 0));

        // Past every number the table lists, though /Size says less: one
        // in use, and one free that nothing holds.
        let mut file = pdf_fixtures::fixture::assembled(
            "1.7",
            &[(1, &b"<< /Type /Catalog >>"[..]), (2, b"2"), (7, b"7")],
            "<< /Size 3 /Root 1 0 R >>",
        );
        let mut doc = crate::read::document(&file, &crate::read::Options::DEFAULT);
        assert_eq!(doc.new_id(), ObjectId::new(8, 0));
        let row = format!("{:010} 00000 n", find(&file, b"7 0 obj"));
        let at = find(&file, row.as_bytes());
        file[at..at + row.len()].copy_from_slice(b"0000000000 00001 f");
        let mut doc = crate::read::document(&file, &crate::read::Options::DEFAULT);
        assert!(!doc.contains(ObjectId::new(7, 0)) && doc.contains(ObjectId::new(2, 0)));
        assert_eq!(doc.max_id(), 7);
        assert_eq!(doc.new_id(), ObjectId::new(8, 0));

        // Past a cross-reference stream's number, which neither its rows
        // nor /Size reach.
        let objects = [
            (1, dict! { "Type" => "Catalog" }.into()),
            (2, Object::from(2).into()),
        ];
        let root = dict! { "Root" => Object::reference(1) };
        let mut file =
            pdf_fixtures::fixture::file("1.7", &objects, root, pdf_fixtures::fixture::Xref::Stream);
        let at = find(&file, b"3 0 obj");
        file.splice(at..at + 1, *b"90");
        let mut doc = crate::read::document(&file, &crate::read::Options::DEFAULT);
        assert_eq!(doc.facts.max_stream, Some(90));
        assert_eq!(
            doc.trailer.dict().get(b"Size").and_then(Object::as_integer),
            Some(4)
        );
        assert_eq!(doc.max_id(), 90);
        assert_eq!(doc.new_id(), ObjectId::new(91, 0));
    }

    /// Once new ids reach 4,294,967,295, the largest number, as a file that
    /// lists it can, each is the next number above the last handed out that
    /// no entry holds, from 1 again after the largest, never 0: two in a
    /// row differ, though neither is set; and an object put at the largest
    /// takes them there from where they'd have started.
    #[test]
    fn new_ids_past_the_largest_number_are_numbers_no_entry_holds() {
        // Up to the largest, as any other; then from 1, round to one
        // handed out before and not set.
        let mut doc = Document::new(Version::new(1, 7), Dict::new());
        let first = doc.new_id();
        doc.set_max_id(u32::MAX - 2);
        assert_eq!(doc.new_id(), ObjectId::new(u32::MAX - 1, 0));
        assert_eq!(doc.new_id(), ObjectId::new(u32::MAX, 0));
        assert_eq!(doc.max_id(), u32::MAX);
        assert_eq!(
            (doc.new_id(), doc.new_id()),
            (ObjectId::new(1, 0), ObjectId::new(2, 0))
        );
        assert_eq!(first, ObjectId::new(1, 0));
        assert_eq!(doc.max_id(), u32::MAX);

        // An object put at the largest, read or not, takes them past every
        // number, from where they'd have started: none handed out before is
        // again, so one set at an id handed out before an object is added
        // doesn't replace it; and none is a number a file lists free.
        for unread in [false, true] {
            let mut doc = Document::new(Version::new(1, 7), Dict::new());
            let page = doc.new_id();
            if unread {
                doc.set_unread(
                    ObjectId::new(u32::MAX, 0),
                    Unread::Other("copied".to_string()),
                );
            } else {
                doc.set(ObjectId::new(u32::MAX, 0), "Copied");
            }
            assert_eq!(doc.max_id(), u32::MAX);
            let font = doc.add("Font");
            assert_eq!(
                (page, font),
                (ObjectId::new(1, 0), ObjectId::new(2, 0)),
                "{unread}"
            );
            assert_eq!(doc.set(page, "Page"), None, "{unread}");
            let name = doc.get(font).ok().and_then(Object::as_name);
            assert_eq!(name, Some(&b"Font"[..]), "{unread}");
            assert_eq!(doc.new_id(), ObjectId::new(3, 0), "{unread}");
        }
        let mut file = pdf_fixtures::fixture::assembled(
            "1.7",
            &[(1, &b"<< /Type /Catalog >>"[..]), (2, b"2"), (3, b"3")],
            "<< /Size 4 /Root 1 0 R >>",
        );
        let row = format!("{:010} 00000 n", find(&file, b"2 0 obj"));
        let at = find(&file, row.as_bytes());
        file[at..at + row.len()].copy_from_slice(b"0000000000 00001 f");
        let mut doc = crate::read::document(&file, &crate::read::Options::DEFAULT);
        assert!(!doc.contains(ObjectId::new(2, 0)) && doc.contains(ObjectId::new(3, 0)));
        doc.set(ObjectId::new(u32::MAX, 0), "Copied");
        assert_eq!(
            (doc.new_id(), doc.new_id()),
            (ObjectId::new(4, 0), ObjectId::new(5, 0))
        );

        // A /Size whose number less one is the largest is passed over, as
        // any larger: alone, it takes new ids nowhere near a free entry.
        let size = dict! { "Size" => 1i64 << 32 };
        let mut doc = Document::new(Version::new(1, 7), size);
        assert_eq!(doc.max_id(), 0);
        assert_eq!(
            (doc.new_id(), doc.new_id()),
            (ObjectId::new(1, 0), ObjectId::new(2, 0))
        );
        file = pdf_fixtures::fixture::assembled(
            "1.7",
            &[(1, &b"<< /Type /Catalog >>"[..]), (2, b"2"), (3, b"3")],
            "<< /Size 4294967296 /Root 1 0 R >>",
        );
        let row = format!("{:010} 00000 n", find(&file, b"2 0 obj"));
        let at = find(&file, row.as_bytes());
        file[at..at + row.len()].copy_from_slice(b"0000000000 00001 f");
        let mut doc = crate::read::document(&file, &crate::read::Options::DEFAULT);
        assert_eq!(doc.max_id(), 3);
        assert_eq!(
            (doc.new_id(), doc.new_id()),
            (ObjectId::new(4, 0), ObjectId::new(5, 0))
        );

        // A document holding the largest: numbers no entry holds, at any
        // generation, read or not, in turn from the last handed out.
        let mut doc = Document::new(Version::new(1, 7), Dict::new());
        doc.insert(u32::MAX, "Last");
        for n in [1, 2, 6] {
            doc.insert(n, n);
        }
        doc.set(ObjectId::new(4, 2), "Four");
        doc.set_unread(
            ObjectId::new(5, 0),
            Unread::Other("it doesn't parse".to_string()),
        );
        assert_eq!(doc.max_id(), u32::MAX);
        let (three, seven) = (doc.new_id(), doc.new_id());
        assert_eq!((three, seven), (ObjectId::new(3, 0), ObjectId::new(7, 0)));
        assert_eq!(doc.add("Eight"), ObjectId::new(8, 0));
        doc.insert(9, 9);
        assert_eq!(doc.new_id(), ObjectId::new(10, 0));
        assert!(!doc.contains(three) && !doc.contains(seven));

        // Past the last held, from 1 again, to the first no entry holds.
        doc.insert(u32::MAX - 1, "Next to last");
        doc.next = PAST + u64::from(u32::MAX - 2);
        assert_eq!(doc.free_after(u32::MAX - 2), None);
        assert_eq!(
            (doc.new_id(), doc.new_id()),
            (ObjectId::new(3, 0), ObjectId::new(7, 0))
        );
        assert_eq!(doc.free_after(u32::MAX), None);

        // Lowered below the largest held, they're where they were again.
        doc.set_max_id(0);
        assert_eq!(doc.max_id(), u32::MAX);
        assert_eq!(doc.new_id(), ObjectId::new(3, 0));
    }

    /// A reference followed to the object its chain ends at, as many as
    /// 128 of them, and none where one names an object that isn't there or
    /// doesn't read, or the chain loops; a dictionary through references,
    /// a stream's too, to read and to change; and the catalog.
    #[test]
    fn references_are_followed_to_the_object_a_chain_ends_at() {
        let file = pdf_fixtures::fixture::assembled(
            "1.7",
            &[
                (1, &b"<< /Type /Catalog /Pages 2 0 R >>"[..]),
                (2, b"3 0 R"),
                (3, b"<< /Type /Pages /Kids [] /Count 0 >>"),
                (4, b"<< /Length 3 >>\nstream\nabc\nendstream"),
                (5, b"5"),
                (6, b"<< /A >> >>"),
                (7, b"9 0 R"),
                (8, b"10 0 R"),
                (10, b"8 0 R"),
                (11, b"6 0 R"),
            ],
            "<< /Size 12 /Root 1 0 R >>",
        );
        let read = crate::read::document(&file, &crate::read::Options::DEFAULT);
        let mut doc = read.clone();
        let pages = read.get(ObjectId::new(3, 0)).unwrap();
        let reference = |n| Object::reference(n);

        // Not a reference: itself. A chain: where it ends, and its last id.
        assert_eq!(doc.dereference(&5.into()), Some((None, &5.into())));
        assert_eq!(
            doc.dereference(&reference(2)),
            Some((Some(ObjectId::new(3, 0)), pages))
        );
        assert_eq!(
            doc.dereference(&reference(3)),
            Some((Some(ObjectId::new(3, 0)), pages))
        );
        // To nothing: not there, there and not read, or round in a loop.
        for n in [9, 6, 7, 11, 8, 12] {
            assert_eq!(doc.dereference(&reference(n)), None, "{n}");
        }
        // An object a reference is, stored as written, and followed.
        assert_eq!(
            doc.get(ObjectId::new(2, 0)).unwrap().as_reference(),
            Some(ObjectId::new(3, 0))
        );
        let stored = doc.get(ObjectId::new(2, 0)).unwrap();
        assert_eq!(doc.dereference(stored).unwrap().1, pages);
        // A dictionary's value followed.
        let catalog = doc.get_dict(ObjectId::new(1, 0)).unwrap();
        let kids = doc.dereference(catalog.get(b"Pages").unwrap()).unwrap().1;
        assert_eq!(kids, pages);

        // Dictionaries through references, a stream's too; none for what
        // isn't one, or doesn't read.
        assert!(
            doc.get_dict(ObjectId::new(1, 0))
                .is_some_and(|d| d.has_type(b"Catalog"))
        );
        assert_eq!(doc.get_dict(ObjectId::new(2, 0)), pages.as_dict());
        assert!(
            doc.get_dict(ObjectId::new(4, 0))
                .is_some_and(|d| d.has(b"Length"))
        );
        for n in [5, 6, 7, 8, 9, 11] {
            assert_eq!(doc.get_dict(ObjectId::new(n, 0)), None, "{n}");
        }
        assert_eq!(doc.catalog(), doc.get_dict(ObjectId::new(1, 0)));

        // To change: the object the chain ends at is edited, its record
        // and span dropped, and nothing else; nothing where none is given.
        doc.get_dict_mut(ObjectId::new(2, 0))
            .unwrap()
            .set("Count", 1);
        assert_eq!(doc.record(ObjectId::new(3, 0)), None);
        assert_eq!(doc.get(ObjectId::new(3, 0)).unwrap().at, None);
        assert_eq!(
            doc.record(ObjectId::new(2, 0)),
            read.record(ObjectId::new(2, 0))
        );
        assert_eq!(doc.get(ObjectId::new(2, 0)), read.get(ObjectId::new(2, 0)));
        let count = doc
            .get_dict(ObjectId::new(2, 0))
            .and_then(|d| d.get(b"Count"));
        assert_eq!(count.and_then(Object::as_integer), Some(1));
        for n in [5, 6, 7, 8] {
            assert_eq!(doc.get_dict_mut(ObjectId::new(n, 0)), None, "{n}");
            assert_eq!(
                doc.record(ObjectId::new(n, 0)),
                read.record(ObjectId::new(n, 0)),
                "{n}"
            );
        }
        doc.get_dict_mut(ObjectId::new(4, 0))
            .unwrap()
            .set("Filter", "FlateDecode");
        let stream = doc.get(ObjectId::new(4, 0)).unwrap().as_stream().unwrap();
        assert_eq!(
            stream.data,
            read.get(ObjectId::new(4, 0))
                .unwrap()
                .as_stream()
                .unwrap()
                .data
        );
        doc.catalog_mut().unwrap().set("Lang", Object::text("en"));
        assert!(doc.catalog().is_some_and(|d| d.has(b"Lang")));
        assert_eq!(doc.record(ObjectId::new(1, 0)), None);

        // No catalog: no /Root, or one that isn't a reference to one.
        for root in [
            None,
            Some(Object::from(1)),
            Some(reference(5)),
            Some(reference(9)),
        ] {
            let mut trailer = Dict::new();
            if let Some(root) = root.clone() {
                trailer.set("Root", root);
            }
            doc.trailer = Trailer::new(trailer.into());
            assert_eq!(doc.catalog(), None, "{root:?}");
            assert_eq!(doc.catalog_mut(), None, "{root:?}");
        }

        // As many as 128 references, and no more; an object's own number
        // isn't one of them.
        let mut doc = Document::new(Version::new(1, 7), Dict::new());
        for n in 99..228 {
            doc.insert(n, reference(n + 1));
        }
        doc.insert(228, dict! { "End" => true });
        let end = doc.get(ObjectId::new(228, 0)).unwrap();
        assert_eq!(
            doc.dereference(&reference(101)),
            Some((Some(ObjectId::new(228, 0)), end))
        );
        assert_eq!(doc.dereference(&reference(100)), None);
        assert_eq!(doc.get_dict(ObjectId::new(100, 0)), end.as_dict());
        assert_eq!(doc.get_dict(ObjectId::new(99, 0)), None);
    }

    /// A trailer's dictionary changed where it is: the trailer's, or the
    /// cross-reference stream's; a trailer that's neither made one.
    #[test]
    fn a_trailers_dictionary_is_changed_where_it_is() {
        let objects = [(1, dict! { "Type" => "Catalog" }.into())];
        let root = || dict! { "Root" => Object::reference(1) };
        for kind in [
            pdf_fixtures::fixture::Xref::Table,
            pdf_fixtures::fixture::Xref::Stream,
        ] {
            let file = pdf_fixtures::fixture::file("1.7", &objects, root(), kind);
            let mut doc = crate::read::document(&file, &crate::read::Options::DEFAULT);
            assert!(doc.trailer.object.at.is_some());
            let stream = doc.trailer.stream;
            doc.trailer.dict_mut().set("Info", Object::reference(2));
            assert_eq!(doc.trailer.object.at, None, "{kind:?}: an edit");
            assert!(doc.trailer.dict().has(b"Info") && doc.trailer.dict().has(b"Root"));
            let streamed = doc.trailer.object.as_stream().is_some();
            assert_eq!(streamed, kind == pdf_fixtures::fixture::Xref::Stream);
            assert_eq!(doc.trailer.stream, stream);
        }
        let mut trailer = Trailer::new(Object::null());
        assert!(trailer.dict().is_empty());
        trailer.dict_mut().set("Root", Object::reference(1));
        assert_eq!(trailer.object, Object::from(root()));
    }

    /// A document built in memory: objects that read, one that doesn't,
    /// one a reference names that isn't there, a key written twice, a
    /// stream naming one in its data, and objects nothing reaches.
    fn walked() -> Document {
        let trailer = dict! {
            "Size" => 20,
            "Root" => Object::reference(1),
            "Info" => Object::reference(2),
        };
        let mut doc = Document::new(Version::new(1, 7), trailer);
        let r = Object::reference;
        doc.insert(
            1,
            dict! {
                "Type" => "Catalog",
                "Pages" => r(3),
                "Outlines" => r(4),
                "Names" => dict! { "Dests" => r(5) },
            },
        );
        doc.insert(2, dict! { "Extra" => vec![r(6), r(1)] });
        doc.insert(
            3,
            dict! { "Kids" => vec![r(7), r(8)], "A" => r(10), "Count" => 2, "A" => r(11) },
        );
        doc.set_unread(
            ObjectId::new(4, 0),
            Unread::Other("it doesn't parse".to_string()),
        );
        let mut data = Stream::new(Dict::new(), b"13 0 R".to_vec());
        data.dict.set("Length", r(12));
        doc.insert(5, data);
        doc.insert(6, vec![r(99)]);
        doc.insert(7, dict! { "Parent" => r(3) });
        doc.insert(8, dict! { "Parent" => r(3), "Annot" => r(14) });
        for n in [10, 11, 12, 13, 14] {
            doc.insert(n, n);
        }
        doc.insert(15, dict! { "X" => r(16) });
        doc.insert(16, 16);
        doc.set_unread(
            ObjectId::new(17, 0),
            Unread::Other("it doesn't parse".to_string()),
        );
        doc
    }

    /// What the trailer reaches, each once, in the order found: the
    /// trailer's references, then each found object's in turn, in the
    /// order written, every entry of a key written twice; an object that
    /// isn't there, or doesn't read, found and not followed; a stream's
    /// data not read.
    #[test]
    fn reachable_finds_what_the_trailer_reaches_in_order() {
        let doc = walked();
        let reached: Vec<u32> = doc.reachable().into_iter().map(|id| id.number).collect();
        assert_eq!(reached, [1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 99, 14]);
        let mut doc = doc;
        doc.trailer = Trailer::new(Object::null());
        assert_eq!(doc.reachable(), []);
    }

    /// Pruned: each object the trailer doesn't reach, read or not, by
    /// number, an object stream and a cross-reference stream among them,
    /// the trailer naming the last still; what it reaches kept as it was,
    /// one that doesn't read too, and the decoded data, and where new ids
    /// start.
    #[test]
    fn prune_removes_what_the_trailer_doesnt_reach() {
        let mut doc = walked();
        doc.insert(18, Stream::new(dict! { "Type" => "XRef" }, Vec::new()));
        doc.trailer.stream = Some(ObjectId::new(18, 0));
        doc.insert(19, Stream::new(dict! { "Type" => "ObjStm" }, Vec::new()));
        doc.decoded.insert(ObjectId::new(19, 0), b"20 0".to_vec());
        let before = doc.clone();
        assert_eq!(
            doc.prune(),
            [
                ObjectId::new(13, 0),
                ObjectId::new(15, 0),
                ObjectId::new(16, 0),
                ObjectId::new(17, 0),
                ObjectId::new(18, 0),
                ObjectId::new(19, 0)
            ]
        );
        assert_eq!(
            doc.ids(),
            [1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 14].map(|n| ObjectId::new(n, 0))
        );
        assert!(doc.objects().all(|(id, o)| before.get(id) == Ok(o)));
        assert_eq!(
            doc.get(ObjectId::new(4, 0)),
            before.get(ObjectId::new(4, 0))
        );
        assert_eq!(doc.trailer, before.trailer);
        assert!(!doc.contains(doc.trailer.stream.unwrap()));
        assert_eq!(doc.decoded, before.decoded);
        assert_eq!(doc.max_id(), before.max_id());
        assert_eq!(doc.reachable(), before.reachable());
        assert_eq!(doc.prune(), [], "nothing more");
    }

    /// Each encrypted file qpdf wrote, opened by its user's password and by
    /// its owner's and pruned: its object streams and cross-reference
    /// stream go, nothing refers to them; the encryption dictionary stays,
    /// the trailer refers to it; and what stays is as read, its strings
    /// decrypted where they are, with its records, beside the decoded data.
    #[test]
    fn prune_keeps_an_encrypted_files_encryption_dictionary_and_strings() {
        let names = [
            "rc4-40",
            "rc4-128",
            "aes-128",
            "aes-256-r5",
            "aes-256",
            "aes-256-unpacked",
        ];
        for name in names {
            let file = fixture(&format!("qpdf-{name}"));
            let owner = [Password("owner")];
            for (passwords, by) in [(&[][..], None), (&owner, Some((0, Encoding::Utf8)))] {
                let owner = by.is_some();
                let mut doc = crate::read::document(
                    &file,
                    &crate::read::Options {
                        passwords,
                        ..crate::read::Options::DEFAULT
                    },
                );
                assert_eq!(
                    doc.encryption,
                    Encryption::opened(owner)
                        .by(by)
                        .permitting(pdf_crypt::Permissions::ALL),
                    "{name}"
                );
                let before = doc.clone();
                let reached: BTreeSet<ObjectId> = before.reachable().into_iter().collect();
                let pruned = doc.prune();
                assert!(pruned.is_sorted(), "{name}");
                assert!(
                    pruned
                        .iter()
                        .all(|id| !reached.contains(id) && !doc.contains(*id))
                );
                assert_eq!(pruned.len() + doc.ids().len(), before.ids().len(), "{name}");
                assert!(doc.ids().iter().all(|id| reached.contains(id)), "{name}");
                // What nothing refers to: the object streams and the
                // cross-reference stream.
                let containers = before.decoded.keys().chain(&before.trailer.stream);
                for id in containers {
                    assert!(pruned.contains(id), "{name}: {id:?}");
                }
                let unpacked = name.ends_with("unpacked");
                assert_eq!(before.decoded.is_empty(), unpacked, "{name}");
                // The encryption dictionary, kept as read.
                let encrypt = doc
                    .trailer
                    .dict()
                    .get(b"Encrypt")
                    .and_then(Object::as_reference);
                let encrypt = encrypt.unwrap();
                assert!(doc.get_dict(encrypt).is_some_and(|d| d.has(b"Filter")));
                // What stays as read, its records with it.
                for (id, object) in doc.objects() {
                    assert_eq!(Ok(object), before.get(id), "{name}: {id:?}");
                    assert_eq!(doc.record(id), before.record(id), "{name}: {id:?}");
                    assert!(doc.record(id).is_some(), "{name}: {id:?}");
                }
                let strings = |doc: &Document| -> usize {
                    let mut strings = 0;
                    for (_, object) in doc.objects() {
                        let mut stack = vec![object];
                        while let Some(o) = stack.pop() {
                            match o.value() {
                                Value::String(..) => strings += 1,
                                Value::Array(a) => stack.extend(a),
                                Value::Dict(d) => {
                                    stack.extend(d.entries().iter().map(|e| &e.value))
                                }
                                Value::Stream(s) => {
                                    stack.extend(s.dict.entries().iter().map(|e| &e.value))
                                }
                                _ => {}
                            }
                        }
                    }
                    strings
                };
                assert!(strings(&doc) > 0, "{name}");
                assert_eq!(doc.records().count(), doc.objects().count(), "{name}");
                assert_eq!(
                    (&doc.trailer, &doc.decoded, &doc.faults(), doc.max_id()),
                    (
                        &before.trailer,
                        &before.decoded,
                        &before.faults(),
                        before.max_id()
                    )
                );
                assert_eq!(doc.reachable(), before.reachable(), "{name}");
            }
        }
    }

    /// What a reading gives can be shared across threads.
    #[test]
    fn a_reading_is_send_and_sync() {
        fn shared<T: Send + Sync>() {}
        shared::<Document>();
        shared::<Trailer>();
        shared::<Fault>();
        shared::<Place>();
        shared::<Unsupported>();
        shared::<Update>();
        shared::<Encryption>();
        shared::<Unread>();
        shared::<Record>();
        shared::<Facts>();
        shared::<Revision>();
    }
}
