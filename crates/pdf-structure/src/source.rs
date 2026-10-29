//! A file as a reader read it: a [`Source`], which any reader can
//! implement, and [`Document`], an owned one that readers fill, tests
//! build, and a caller builds and edits in memory; and what the bytes
//! reader records of where it read each object ([`Record`]) and of the
//! file as written ([`Facts`]).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::RangeBounds;

use pdf_crypt::Encoding;
use pdf_objects::Resolve;
use pdf_objects::object::{Dict, Object, ObjectId, Span, Value};
use pdf_objects::parse;
use pdf_objects::version::Version;

use crate::bytes::{SourceId, Undecoded, Unindexed};

/// A file, as a reader read it.
pub trait Source {
    /// Every object its cross-references list.
    fn ids(&self) -> Vec<ObjectId>;
    /// Object `id`, as written: references kept as references, strings
    /// decoded and decrypted. The error says why there's none: the file
    /// hasn't it, or it doesn't read.
    fn get(&self, id: ObjectId) -> Result<&Object, Unread>;
    /// The trailer.
    fn trailer(&self) -> &Trailer;
    /// The version in the header.
    fn header_version(&self) -> Option<Version>;
    /// What the reader warned of while reading it: repairs, duplicate
    /// keys, and anything else it had to choose how to read.
    fn warnings(&self) -> &[Warning];
    /// The file's length in bytes, when the reader knows it.
    fn size(&self) -> Option<usize> {
        None
    }
    /// The data of the catalog's metadata stream (ISO 32000-2, 14.3.2), as
    /// written, when the reader has it in plain view.
    fn metadata(&self) -> Option<&[u8]> {
        None
    }
}

/// A file's trailer. A reader outside the crate starts from
/// [`Trailer::new`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Trailer {
    /// The cross-reference stream it is, when it is one.
    pub stream: Option<ObjectId>,
    /// The dictionary after `trailer`, or the cross-reference stream.
    pub object: Object,
    /// Why it isn't the trailer as the file's newest cross-reference
    /// section writes it, when it isn't: then it's what the reader made of
    /// the file instead.
    pub unwritten: Option<String>,
}

/// Something a reader had to choose how to read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Warning {
    /// The object it's in, when it's in one.
    pub object: Option<ObjectId>,
    /// Where, when the reader knows.
    pub at: Option<Span>,
    /// What it says.
    pub message: String,
}

impl Warning {
    /// `message`, of object `object` at `at`.
    pub fn new(object: Option<ObjectId>, at: Option<Span>, message: String) -> Warning {
        Warning {
            object,
            at,
            message,
        }
    }
}

impl Trailer {
    /// The trailer `object` is, as the file's newest cross-reference
    /// section writes it, and not a cross-reference stream: a reader that
    /// read one, or read the file otherwise, sets [`Trailer::stream`] or
    /// [`Trailer::unwritten`] on what this gives.
    pub fn new(object: Object) -> Trailer {
        Trailer {
            stream: None,
            object,
            unwritten: None,
        }
    }

    /// Its dictionary: the trailer's, or the cross-reference stream's.
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

/// A file read whole: its objects, trailer, header version and warnings.
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
#[derive(Debug, Clone, PartialEq)]
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
    pub trailer: Trailer,
    /// The version in the header.
    pub header: Option<Version>,
    /// What the reader had to choose how to read, in the order it chose.
    pub warnings: Vec<Warning>,
    /// The object streams' decoded data, by stream, which the objects
    /// read from them are placed in ([`Span::stream`]). The record of an
    /// object read from one names its bytes here, so it says where the
    /// object was read only while this is as read.
    pub decoded: BTreeMap<ObjectId, Vec<u8>>,
    /// The file's newest incremental update, when the reader read its
    /// cross-reference sections and the newest follows one written before
    /// it.
    pub update: Option<Update>,
    /// The file's length in bytes, when the reader knows it.
    pub size: Option<usize>,
    /// The data of the catalog's metadata stream, as written, when it's in
    /// plain view: unfiltered, and not encrypted ([`Source::metadata`]).
    pub metadata: Option<Vec<u8>>,
    /// Whether the file is encrypted, and if it is, whether it opened.
    pub encryption: Encryption,
    /// What the file's cross-reference sections and header say of it, as
    /// written, by the bytes reader; the defaults from another.
    pub facts: Facts,
}

/// Where an object was read from, which the bytes reader records when it
/// first reads it ([`crate::bytes::Reading::record`]), in the reading it
/// read it in; a [`Document`] keeps it with the object while the object
/// is as read ([`Document::record`]). Two records are equal when they say
/// the same of where their objects were read, whichever readings they're
/// of, so that two readings of one file are equal; [`Record::source`]
/// tells the readings apart.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Record {
    /// The reading it was read in, whose file its places are in.
    pub source: SourceId,
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
}

impl PartialEq for Record {
    fn eq(&self, other: &Record) -> bool {
        let Record {
            source: _,
            origin,
            at,
            endobj,
            data,
        } = self;
        (origin, at, endobj, data) == (&other.origin, &other.at, &other.endobj, &other.data)
    }
}

impl Eq for Record {}

/// Where an object was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

/// Which bytes a stream's data is, and how the reader chose them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct DataExtent {
    /// Where the data is, as the stream's
    /// [`Data::At`](pdf_objects::object::Data::At) has it.
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Facts {
    /// Where `%PDF-` starts, when the file has it within its first 1024
    /// bytes: what's before it is junk some writers put first.
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
}

/// How a cross-reference section is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum XrefKind {
    /// A table, after `xref` (ISO 32000-2, 7.5.4).
    Table,
    /// A cross-reference stream (7.5.8).
    Stream,
}

/// Whether a file is encrypted (its trailer's `/Encrypt`), and if it is,
/// whether the reader opened it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Encryption {
    /// It isn't.
    #[default]
    None,
    /// It is, and a password it was given opened it, the owner's or the
    /// user's: its strings and object streams are decrypted. A reader
    /// outside the crate makes it by [`Encryption::opened`], and says which
    /// password by [`Encryption::by`].
    #[non_exhaustive]
    Opened {
        /// Whether by the owner's password.
        owner: bool,
        /// Which of the passwords the reader was given opened it: its index
        /// among them, and the encoding it opened in. None when it was
        /// given none, and the empty password opened it.
        by: Option<(usize, Encoding)>,
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
    Unsupported(String),
}

/// A file's newest incremental update. A reader outside the crate makes
/// one by [`Update::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Update {
    /// The objects its cross-reference section lists in use.
    pub objects: BTreeSet<ObjectId>,
    /// Where the section it follows starts: the file as of that section
    /// is its previous revision ([`crate::bytes::read_from`]).
    pub previous: usize,
}

/// Why an object doesn't read: what [`Source::get`] gives in its place,
/// and [`Document::unread`] gives. Each is one of the bytes reader's
/// reasons ([`crate::bytes`]), or another reader's words.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Unread {
    /// The file hasn't it: no cross-reference section lists it.
    #[error("{n} {g} R isn't in the file", n = .id.0, g = .id.1)]
    Missing {
        /// The object.
        id: ObjectId,
    },
    /// It doesn't parse at the offset its cross-references give.
    #[error("{0}")]
    Parse(parse::Error),
    /// The offset its cross-references give holds another object.
    #[error(
        "byte {offset} holds {} {} obj, where the cross-references put {} {} R",
        .found.0,
        .found.1,
        .expected.0,
        .expected.1
    )]
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
    /// Another reader's words: a [`Source`] outside the crate says why in
    /// its own.
    #[error("{0}")]
    Other(String),
}

impl Encryption {
    /// [`Encryption::Opened`]: by the owner's password when `owner`, else
    /// by the user's; by the empty password, given none, until
    /// [`Encryption::by`] says which of those given. Each field `Opened`
    /// gains has a setter of its own, so this stays as it is.
    pub fn opened(owner: bool) -> Encryption {
        Encryption::Opened { owner, by: None }
    }

    /// It, opened by the password given at index `by`'s, in its encoding,
    /// or none, by the empty password, given none: `Opened`'s `by`. Any
    /// other is given back as it is.
    #[must_use = "it gives the Encryption back, with which password opened it"]
    pub fn by(self, by: Option<(usize, Encoding)>) -> Encryption {
        match self {
            Encryption::Opened { owner, .. } => Encryption::Opened { owner, by },
            other => other,
        }
    }
}

impl Update {
    /// The update whose cross-reference section lists `objects` in use,
    /// after the section that starts at `previous`.
    pub fn new(objects: BTreeSet<ObjectId>, previous: usize) -> Update {
        Update { objects, previous }
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
    /// use pdf_structure::pdf_objects::{dict, object::Object, version::Version};
    /// use pdf_structure::{Document, Encryption, Source, Update};
    ///
    /// let trailer = dict! { "Root" => Object::reference(1) };
    /// let mut doc = Document::new(Version::new(1, 7), trailer);
    /// doc.insert(1, dict! { "Type" => "Catalog" });
    /// doc.update = Some(Update::new(BTreeSet::from([(1, 0)]), 9));
    /// doc.encryption = Encryption::opened(false);
    /// assert!(doc.get((1, 0)).is_ok());
    /// ```
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
            warnings: Vec::new(),
            decoded: BTreeMap::new(),
            update: None,
            size: None,
            metadata: None,
            encryption: Encryption::None,
            facts: Facts::default(),
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
        let held = objects.last_key_value().map(|(&(n, _), _)| n);
        Document {
            next: first_new(trailer.dict(), listed.max(held)),
            objects,
            trailer,
            header: None,
            warnings: Vec::new(),
            decoded: BTreeMap::new(),
            update: None,
            size: None,
            metadata: None,
            encryption: Encryption::None,
            facts: Facts::default(),
        }
    }

    /// Puts object `n`, generation 0, with no record: [`Document::set`]
    /// at `(n, 0)`.
    pub fn insert(&mut self, n: u32, object: impl Into<Object>) {
        self.set((n, 0), object);
    }

    /// Whether it lists object `id`, whether it reads or not.
    pub fn contains(&self, id: ObjectId) -> bool {
        self.objects.contains_key(&id)
    }

    /// Each object that reads, by number.
    pub fn objects(&self) -> impl DoubleEndedIterator<Item = (ObjectId, &Object)> {
        (self.objects.iter()).filter_map(|(&id, held)| Some((id, &held.as_ref().ok()?.0)))
    }

    /// Each object that reads whose id is in `range`, by number: a
    /// number's every generation, `n`'s, is `(n, 0)..=(n, u16::MAX)`.
    /// Panics where [`BTreeMap::range`] does: a range that starts after it
    /// ends, or starts and ends at one id it leaves out.
    pub fn range(
        &self,
        range: impl RangeBounds<ObjectId>,
    ) -> impl DoubleEndedIterator<Item = (ObjectId, &Object)> {
        (self.objects.range(range)).filter_map(|(&id, held)| Some((id, &held.as_ref().ok()?.0)))
    }

    /// Each object it lists that doesn't read, by number, with why.
    pub fn unread(&self) -> impl DoubleEndedIterator<Item = (ObjectId, &Unread)> {
        (self.objects.iter()).filter_map(|(&id, held)| Some((id, held.as_ref().err()?)))
    }

    /// Where object `id` was read from, while it's as read: none for an
    /// object put in by [`Document::set`] or taken to edit by
    /// [`Document::get_mut`], one a reader other than the bytes reader
    /// read, or one that doesn't read.
    pub fn record(&self, id: ObjectId) -> Option<&Record> {
        self.objects.get(&id)?.as_ref().ok()?.1.as_ref()
    }

    /// Each object's record, by number: each [`Document::record`] gives.
    pub fn records(&self) -> impl DoubleEndedIterator<Item = (ObjectId, &Record)> {
        (self.objects.iter()).filter_map(|(&id, held)| Some((id, held.as_ref().ok()?.1.as_ref()?)))
    }

    /// Puts `object` at `id`, with no record, as it wasn't read: gives the
    /// object it replaces, when one that read was there. New ids start
    /// past `id`'s number, if they didn't. The object keeps the spans it
    /// has, its own and its values', of whatever file it was read from,
    /// with nothing to say which: where spans are read as places in this
    /// document's file, as a check reads them, a copy from another file is
    /// told apart by its number alone.
    pub fn set(&mut self, id: ObjectId, object: impl Into<Object>) -> Option<Object> {
        self.past(id.0);
        let was = self.objects.insert(id, Ok((object.into(), None)));
        was?.ok().map(|(object, _)| object)
    }

    /// Puts why object `id` doesn't read at `id`, as a reader outside the
    /// crate does for an object it lists that doesn't read: gives the
    /// object it replaces, when one that read was there. New ids start
    /// past `id`'s number, if they didn't.
    pub fn set_unread(&mut self, id: ObjectId, why: Unread) -> Option<Object> {
        self.past(id.0);
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
    /// gives the object, when it read.
    pub fn remove(&mut self, id: ObjectId) -> Option<Object> {
        self.objects.remove(&id)?.ok().map(|(object, _)| object)
    }

    /// Object `id`, to edit, when it reads. Its record is dropped, as what
    /// the object is then is the caller's: a record says the object is as
    /// it was read, which a `&mut` can replace whole without an edit that
    /// drops its span. It's the object at `id` itself, a reference as it
    /// is, not followed: [`Document::dereference`] gives the id a chain of
    /// them ends at, and [`Document::get_dict_mut`] follows one to a
    /// dictionary.
    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
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
            return (n, 0);
        }
        let last = u32::try_from(self.next - PAST).unwrap_or(0);
        let n = (self.free_after(last))
            .or_else(|| self.free_after(0))
            .unwrap_or(1);
        self.next = PAST + u64::from(n);
        (n, 0)
    }

    /// The first number above `last` that no entry holds, at any
    /// generation; none when every one to 4,294,967,295 is held.
    fn free_after(&self, last: u32) -> Option<u32> {
        let mut n = last.checked_add(1)?;
        for &(held, _) in self.objects.range((n, 0)..).map(|(id, _)| id) {
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
    pub fn max_id(&self) -> u32 {
        u32::try_from(self.next - 1).unwrap_or(u32::MAX)
    }

    /// Has new ids start after `n`, or after the largest number an entry
    /// holds, where that's larger: never at a number held. Lowered, they
    /// may be numbers the file lists free, or numbers handed out before
    /// and not set. After 4,294,967,295, the largest, they're looked for
    /// from 1 again, as [`Document::new_id`]'s are once they've gone round.
    pub fn set_max_id(&mut self, n: u32) {
        let held = self.objects.last_key_value().map(|(&(held, _), _)| held);
        self.next = u64::from(n.max(held.unwrap_or(0))) + 1;
    }

    /// The object a chain of references ends at, from `object`, and the
    /// last id on the way: `object` itself, and none, when it isn't a
    /// reference. None where one names an object it hasn't, or one that
    /// doesn't read, or the chain runs past 128 references, as a loop
    /// does.
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
    /// [`Trailer::stream`], [`Document::update`]'s objects and a warning's
    /// object may then name one it no longer holds; the trailer refers to
    /// the encryption dictionary, so it stays.
    /// [`Document::decoded`] is left as it is, as the objects read from an
    /// object stream are placed in its data still. New ids start where
    /// they did.
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

impl Source for Document {
    fn ids(&self) -> Vec<ObjectId> {
        self.objects.keys().copied().collect()
    }

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

    fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    fn size(&self) -> Option<usize> {
        self.size
    }

    fn metadata(&self) -> Option<&[u8]> {
        self.metadata.as_deref()
    }
}

/// Its objects, as a reference resolves them: none for one it hasn't, or
/// one that doesn't read, without why ([`Source::get`] says).
impl<'o> Resolve<'o> for &'o Document {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        let doc: &'o Document = self;
        doc.object(id)
    }
}

#[cfg(test)]
mod tests {
    use pdf_crypt::Password;
    use pdf_objects::dict;
    use pdf_objects::object::Stream;

    use super::*;
    use crate::bytes::lazy::fixture;

    #[test]
    fn a_document_gives_what_it_holds() {
        let mut doc = Document::new(Version::new(1, 7), dict! { "Root" => Object::reference(1) });
        doc.insert(1, dict! { "Type" => "Catalog" });
        let why = Unread::Other("it doesn't parse".to_string());
        doc.set_unread((2, 0), why.clone());
        assert_eq!(doc.ids(), [(1, 0), (2, 0)]);
        assert!(doc.get((1, 0)).is_ok_and(|o| o.as_dict().is_some()));
        assert_eq!(doc.get((2, 0)), Err(why));
        assert_eq!(doc.get((3, 0)), Err(Unread::Missing { id: (3, 0) }));
        assert_eq!(
            doc.get((3, 0)).unwrap_err().to_string(),
            "3 0 R isn't in the file"
        );
        assert!(doc.trailer().dict().has(b"Root"));
        assert_eq!(doc.header_version(), Some(Version::new(1, 7)));
    }

    /// A reference resolves to an object that reads; to none where the
    /// document has none, or one that doesn't read, as either is null.
    #[test]
    fn a_document_resolves_the_objects_that_read() {
        let mut doc = Document::new(Version::new(1, 7), dict! { "Root" => Object::reference(1) });
        doc.insert(1, dict! { "Type" => "Catalog" });
        let why = Unread::Other("it doesn't parse".to_string());
        doc.set_unread((2, 0), why);
        let resolve = &doc;
        let catalog = resolve.resolve((1, 0)).unwrap();
        assert!(std::ptr::eq(catalog, doc.get((1, 0)).unwrap()));
        assert_eq!(resolve.resolve((2, 0)), None);
        assert_eq!(resolve.resolve((3, 0)), None);
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
        doc.set((6, 2), "Six");
        doc.insert(7, 7);
        let why = Unread::Other("it doesn't parse".to_string());
        doc.set_unread((3, 0), why.clone());
        fn ids<'o>(objects: impl Iterator<Item = (ObjectId, &'o Object)>) -> Vec<ObjectId> {
            objects.map(|(id, _)| id).collect()
        }

        assert_eq!(doc.ids(), [(1, 0), (2, 0), (3, 0), (5, 0), (6, 2), (7, 0)]);
        assert_eq!(doc.ids().len(), 6);
        assert!(doc.contains((3, 0)) && doc.contains((6, 2)));
        assert!(!doc.contains((4, 0)) && !doc.contains((6, 0)));
        assert_eq!(ids(doc.objects()), [(1, 0), (2, 0), (5, 0), (6, 2), (7, 0)]);
        assert!(
            doc.objects()
                .all(|(id, o)| std::ptr::eq(o, doc.get(id).unwrap()))
        );
        assert_eq!(ids(doc.objects().rev()).first(), Some(&(7, 0)));
        assert_eq!(doc.unread().collect::<Vec<_>>(), [((3, 0), &why)]);
        assert_eq!(doc.get((3, 0)), Err(why));
        assert_eq!(doc.get((4, 0)), Err(Unread::Missing { id: (4, 0) }));

        // Ranges as a caller asks them: between two ids, up to one, from
        // one, and a number's every generation.
        assert_eq!(ids(doc.range((2, 0)..(6, 0))), [(2, 0), (5, 0)]);
        assert_eq!(ids(doc.range(..(5, 0))), [(1, 0), (2, 0)]);
        assert_eq!(ids(doc.range((5, 0)..)), [(5, 0), (6, 2), (7, 0)]);
        assert_eq!(ids(doc.range((6, 0)..=(6, u16::MAX))), [(6, 2)]);
        assert_eq!(ids(doc.range((3, 0)..=(4, 0))), []);
        assert_eq!(ids(doc.range(..(5, 0)).rev()).first(), Some(&(2, 0)));
    }

    /// A record is always of the object beside it: an object put in has
    /// none, one replaced, removed or taken to edit loses its own, one
    /// that doesn't read has none, and the rest keep theirs, each still
    /// where its object was written; a clone keeps them all.
    #[test]
    fn each_write_keeps_the_records_true() {
        let file = crate::fixture::assembled(
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
        let read = crate::bytes::read(&file);
        // Each record says where its object was written.
        let true_to_the_file = |doc: &Document| {
            for (id, record) in doc.records() {
                let written = &file[record.at.start..record.at.end];
                let header = format!("{} {} obj", id.0, id.1);
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
            [(7, 0)]
        );
        let reads: Vec<ObjectId> = read.objects().map(|(id, _)| id).collect();
        assert_eq!(ids(&read), reads);
        assert_eq!(read.record((7, 0)), None);
        true_to_the_file(&read);

        // A clone keeps every record, its reading's id too.
        let mut doc = read.clone();
        assert_eq!(doc, read);
        assert!(doc.records().eq(read.records()));
        assert!((doc.records().zip(read.records())).all(|((_, a), (_, b))| a.source == b.source));

        // Put in: no record, giving back what it replaces where that read.
        let catalog = read.get((1, 0)).unwrap().clone();
        assert_eq!(
            doc.set((1, 0), dict! { "Type" => "Catalog" }),
            Some(catalog)
        );
        assert_eq!(doc.record((1, 0)), None);
        assert_eq!(doc.set((8, 0), 8), None);
        doc.insert(9, 9);
        assert_eq!((doc.record((8, 0)), doc.record((9, 0))), (None, None));
        assert_eq!(doc.set((7, 0), 7), None, "an object that didn't read");
        assert_eq!(doc.record((7, 0)), None);

        // Why one doesn't read: no record, giving back what read there.
        let why = Unread::Other("set aside".to_string());
        let two = read.get((2, 0)).unwrap().clone();
        assert_eq!(doc.set_unread((2, 0), why.clone()), Some(two));
        assert_eq!(doc.record((2, 0)), None);
        assert_eq!(doc.get((2, 0)), Err(why.clone()));
        assert_eq!(doc.set_unread((10, 0), why.clone()), None);

        // Removed: the object, its record with it; one that doesn't read,
        // nothing, and it's gone too.
        let stream = read.get((3, 0)).unwrap().clone();
        assert_eq!(doc.remove((3, 0)), Some(stream));
        assert!(!doc.contains((3, 0)) && doc.record((3, 0)).is_none());
        assert_eq!(doc.remove((10, 0)), None);
        assert!(!doc.contains((10, 0)));
        assert_eq!(doc.remove((3, 0)), None);

        // Taken to edit, the record goes though nothing changes, as a
        // `&mut` could have replaced the object whole; one that doesn't
        // read, or isn't listed, isn't given.
        let four = read.get((4, 0)).unwrap().clone();
        assert_eq!(doc.get_mut((4, 0)), Some(&mut four.clone()));
        assert_eq!(doc.get((4, 0)), Ok(&four));
        assert_eq!(doc.record((4, 0)), None);
        assert_eq!(doc.get_mut((2, 0)), None);
        assert_eq!(doc.get((2, 0)), Err(why));
        assert_eq!(doc.get_mut((11, 0)), None);
        assert!(!doc.contains((11, 0)));

        // What wasn't written to keeps its record, true to the file; and
        // a document whose records went isn't the one read.
        assert_eq!(ids(&doc), [(5, 0), (6, 0)]);
        assert_eq!(doc.record((5, 0)), read.record((5, 0)));
        true_to_the_file(&doc);
        let mut edited = read.clone();
        assert!(edited.get_mut((5, 0)).is_some());
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
        let file = crate::fixture::assembled(
            "1.6",
            &[(1, &b"<< /Type /Catalog >>"[..])],
            "<< /Size 2 /Root 1 0 R >>",
        );
        let doc = crate::bytes::read(&file);
        assert_eq!(doc.header, Some(Version::new(1, 6)));
        let newest = Some((startxref(&file), XrefKind::Table));
        assert_eq!(doc.facts.newest, newest);
        assert_eq!(doc.encryption, Encryption::None);

        // qpdf's file, its dictionaries in object streams, encrypted with
        // AES-256 and opened by its empty user password, where the crate
        // has it.
        let Some(file) = fixture("qpdf-aes-256") else {
            return;
        };
        let doc = crate::bytes::read(&file);
        assert_eq!(doc.header, Some(Version::new(1, 7)));
        assert_eq!(doc.facts.newest, Some((startxref(&file), XrefKind::Stream)));
        assert_ne!(doc.encryption, Encryption::None);
        assert_eq!(doc.encryption, Encryption::opened(false));
    }

    /// Each reason an object doesn't read says it as the reader's text
    /// for it did, by the same format string.
    #[test]
    fn each_reason_an_object_doesnt_read_is_said_as_it_was() {
        let (stream, index, number, offset) = (4, 1, 9, 120);
        let id = (3, 1);
        let parse = parse::Error::Header { at: 7 };
        let why = Undecoded::NotAStream;
        let inner = Unread::Parse(parse.clone());
        let cases = [
            (
                Unread::Missing { id },
                format!("{} {} R isn't in the file", id.0, id.1),
            ),
            (Unread::Parse(parse.clone()), parse.to_string()),
            (
                Unread::Elsewhere {
                    offset,
                    found: (5, 0),
                    expected: id,
                },
                format!(
                    "byte {offset} holds {} {} obj, where the cross-references put {} {} R",
                    5, 0, id.0, id.1
                ),
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
                    expected: id.0,
                },
                format!(
                    "object {index} of object stream {} 0 R is {number}, not {}",
                    stream, id.0
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
                Unread::Other("qpdf's words".to_string()),
                "qpdf's words".to_string(),
            ),
        ];
        for (unread, said) in cases {
            assert_eq!(unread.to_string(), said, "{unread:?}");
        }
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
        assert_eq!((doc.new_id(), doc.new_id()), ((1, 0), (2, 0)));
        assert_eq!(doc.max_id(), 2);
        assert_eq!(doc.add(dict! { "Type" => "Catalog" }), (3, 0));
        assert!(doc.get_dict((3, 0)).is_some_and(|d| d.has_type(b"Catalog")));
        assert_eq!(doc.record((3, 0)), None);
        assert_eq!(doc.ids(), [(3, 0)], "an id handed out isn't held");

        // Past a /Size, where that's a number of objects; else from 1.
        let sized = |size: Object| Document::new(Version::new(1, 7), dict! { "Size" => size });
        assert_eq!(sized(8.into()).new_id(), (8, 0));
        assert_eq!(sized(8.into()).max_id(), 7);
        for size in [
            0.into(),
            (-3).into(),
            8.5.into(),
            (1i64 << 32).into(),
            (1i64 << 33).into(),
            Object::reference(8),
        ] {
            assert_eq!(sized(size.clone()).new_id(), (1, 0), "{size:?}");
        }

        // Past what's put in, read or not; one put in below leaves them.
        let mut doc = sized(8.into());
        doc.set((20, 3), "Twenty");
        assert_eq!(doc.new_id(), (21, 0));
        doc.set_unread((30, 0), Unread::Other("it doesn't parse".to_string()));
        assert_eq!(doc.add(31), (31, 0));
        doc.insert(40, 40);
        doc.insert(5, 5);
        assert_eq!(doc.max_id(), 40);
        assert_eq!(doc.new_id(), (41, 0));

        // Lowered, but never below a number held, read or not.
        doc.set_max_id(9);
        assert_eq!(doc.max_id(), 40);
        assert_eq!(doc.remove((40, 0)), Some(40.into()));
        assert_eq!(doc.max_id(), 40, "removing leaves where they start");
        doc.set_max_id(9);
        assert_eq!(doc.max_id(), 31);
        doc.remove((31, 0));
        doc.remove((30, 0));
        doc.set_max_id(9);
        assert_eq!(doc.new_id(), (21, 0));
        doc.set_max_id(100);
        assert_eq!(doc.new_id(), (101, 0));
        // Object 0, always free, is never one, even held.
        let mut doc = sized(0.into());
        doc.set((0, 0), 0);
        doc.set_max_id(0);
        assert_eq!((doc.max_id(), doc.new_id()), (0, (1, 0)));

        // Read: past the trailer's /Size less one, which is past what the
        // table lists.
        let file = crate::fixture::assembled(
            "1.7",
            &[(1, &b"<< /Type /Catalog >>"[..]), (2, b"2")],
            "<< /Size 20 /Root 1 0 R >>",
        );
        let mut doc = crate::bytes::read(&file);
        assert_eq!(doc.max_id(), 19);
        assert_eq!(doc.new_id(), (20, 0));

        // Past every number the table lists, though /Size says less: one
        // in use, and one free that nothing holds.
        let mut file = crate::fixture::assembled(
            "1.7",
            &[(1, &b"<< /Type /Catalog >>"[..]), (2, b"2"), (7, b"7")],
            "<< /Size 3 /Root 1 0 R >>",
        );
        let mut doc = crate::bytes::read(&file);
        assert_eq!(doc.new_id(), (8, 0));
        let row = format!("{:010} 00000 n", find(&file, b"7 0 obj"));
        let at = find(&file, row.as_bytes());
        file[at..at + row.len()].copy_from_slice(b"0000000000 00001 f");
        let mut doc = crate::bytes::read(&file);
        assert!(!doc.contains((7, 0)) && doc.contains((2, 0)));
        assert_eq!(doc.max_id(), 7);
        assert_eq!(doc.new_id(), (8, 0));

        // Past a cross-reference stream's number, which neither its rows
        // nor /Size reach.
        let objects = [
            (1, dict! { "Type" => "Catalog" }.into()),
            (2, Object::from(2).into()),
        ];
        let root = dict! { "Root" => Object::reference(1) };
        let mut file = crate::fixture::file("1.7", &objects, root, crate::fixture::Xref::Stream);
        let at = find(&file, b"3 0 obj");
        file.splice(at..at + 1, *b"90");
        let mut doc = crate::bytes::read(&file);
        assert_eq!(doc.facts.max_stream, Some(90));
        assert_eq!(
            doc.trailer.dict().get(b"Size").and_then(Object::as_integer),
            Some(4)
        );
        assert_eq!(doc.max_id(), 90);
        assert_eq!(doc.new_id(), (91, 0));
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
        assert_eq!(doc.new_id(), (u32::MAX - 1, 0));
        assert_eq!(doc.new_id(), (u32::MAX, 0));
        assert_eq!(doc.max_id(), u32::MAX);
        assert_eq!((doc.new_id(), doc.new_id()), ((1, 0), (2, 0)));
        assert_eq!(first, (1, 0));
        assert_eq!(doc.max_id(), u32::MAX);

        // An object put at the largest, read or not, takes them past every
        // number, from where they'd have started: none handed out before is
        // again, so one set at an id handed out before an object is added
        // doesn't replace it; and none is a number a file lists free.
        for unread in [false, true] {
            let mut doc = Document::new(Version::new(1, 7), Dict::new());
            let page = doc.new_id();
            if unread {
                doc.set_unread((u32::MAX, 0), Unread::Other("copied".to_string()));
            } else {
                doc.set((u32::MAX, 0), "Copied");
            }
            assert_eq!(doc.max_id(), u32::MAX);
            let font = doc.add("Font");
            assert_eq!((page, font), ((1, 0), (2, 0)), "{unread}");
            assert_eq!(doc.set(page, "Page"), None, "{unread}");
            let name = doc.get(font).ok().and_then(Object::as_name);
            assert_eq!(name, Some(&b"Font"[..]), "{unread}");
            assert_eq!(doc.new_id(), (3, 0), "{unread}");
        }
        let mut file = crate::fixture::assembled(
            "1.7",
            &[(1, &b"<< /Type /Catalog >>"[..]), (2, b"2"), (3, b"3")],
            "<< /Size 4 /Root 1 0 R >>",
        );
        let row = format!("{:010} 00000 n", find(&file, b"2 0 obj"));
        let at = find(&file, row.as_bytes());
        file[at..at + row.len()].copy_from_slice(b"0000000000 00001 f");
        let mut doc = crate::bytes::read(&file);
        assert!(!doc.contains((2, 0)) && doc.contains((3, 0)));
        doc.set((u32::MAX, 0), "Copied");
        assert_eq!((doc.new_id(), doc.new_id()), ((4, 0), (5, 0)));

        // A /Size whose number less one is the largest is passed over, as
        // any larger: alone, it takes new ids nowhere near a free entry.
        let size = dict! { "Size" => 1i64 << 32 };
        let mut doc = Document::new(Version::new(1, 7), size);
        assert_eq!(doc.max_id(), 0);
        assert_eq!((doc.new_id(), doc.new_id()), ((1, 0), (2, 0)));
        file = crate::fixture::assembled(
            "1.7",
            &[(1, &b"<< /Type /Catalog >>"[..]), (2, b"2"), (3, b"3")],
            "<< /Size 4294967296 /Root 1 0 R >>",
        );
        let row = format!("{:010} 00000 n", find(&file, b"2 0 obj"));
        let at = find(&file, row.as_bytes());
        file[at..at + row.len()].copy_from_slice(b"0000000000 00001 f");
        let mut doc = crate::bytes::read(&file);
        assert_eq!(doc.max_id(), 3);
        assert_eq!((doc.new_id(), doc.new_id()), ((4, 0), (5, 0)));

        // A document holding the largest: numbers no entry holds, at any
        // generation, read or not, in turn from the last handed out.
        let mut doc = Document::new(Version::new(1, 7), Dict::new());
        doc.insert(u32::MAX, "Last");
        for n in [1, 2, 6] {
            doc.insert(n, n);
        }
        doc.set((4, 2), "Four");
        doc.set_unread((5, 0), Unread::Other("it doesn't parse".to_string()));
        assert_eq!(doc.max_id(), u32::MAX);
        let (three, seven) = (doc.new_id(), doc.new_id());
        assert_eq!((three, seven), ((3, 0), (7, 0)));
        assert_eq!(doc.add("Eight"), (8, 0));
        doc.insert(9, 9);
        assert_eq!(doc.new_id(), (10, 0));
        assert!(!doc.contains(three) && !doc.contains(seven));

        // Past the last held, from 1 again, to the first no entry holds.
        doc.insert(u32::MAX - 1, "Next to last");
        doc.next = PAST + u64::from(u32::MAX - 2);
        assert_eq!(doc.free_after(u32::MAX - 2), None);
        assert_eq!((doc.new_id(), doc.new_id()), ((3, 0), (7, 0)));
        assert_eq!(doc.free_after(u32::MAX), None);

        // Lowered below the largest held, they're where they were again.
        doc.set_max_id(0);
        assert_eq!(doc.max_id(), u32::MAX);
        assert_eq!(doc.new_id(), (3, 0));
    }

    /// A reference followed to the object its chain ends at, as many as
    /// 128 of them, and none where one names an object that isn't there or
    /// doesn't read, or the chain loops; a dictionary through references,
    /// a stream's too, to read and to change; and the catalog.
    #[test]
    fn references_are_followed_to_the_object_a_chain_ends_at() {
        let file = crate::fixture::assembled(
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
        let read = crate::bytes::read(&file);
        let mut doc = read.clone();
        let pages = read.get((3, 0)).unwrap();
        let reference = |n| Object::reference(n);

        // Not a reference: itself. A chain: where it ends, and its last id.
        assert_eq!(doc.dereference(&5.into()), Some((None, &5.into())));
        assert_eq!(doc.dereference(&reference(2)), Some((Some((3, 0)), pages)));
        assert_eq!(doc.dereference(&reference(3)), Some((Some((3, 0)), pages)));
        // To nothing: not there, there and not read, or round in a loop.
        for n in [9, 6, 7, 11, 8, 12] {
            assert_eq!(doc.dereference(&reference(n)), None, "{n}");
        }
        // An object a reference is, stored as written, and followed.
        assert_eq!(doc.get((2, 0)).unwrap().as_reference(), Some((3, 0)));
        let stored = doc.get((2, 0)).unwrap();
        assert_eq!(doc.dereference(stored).unwrap().1, pages);
        // A dictionary's value followed.
        let catalog = doc.get_dict((1, 0)).unwrap();
        let kids = doc.dereference(catalog.get(b"Pages").unwrap()).unwrap().1;
        assert_eq!(kids, pages);

        // Dictionaries through references, a stream's too; none for what
        // isn't one, or doesn't read.
        assert!(doc.get_dict((1, 0)).is_some_and(|d| d.has_type(b"Catalog")));
        assert_eq!(doc.get_dict((2, 0)), pages.as_dict());
        assert!(doc.get_dict((4, 0)).is_some_and(|d| d.has(b"Length")));
        for n in [5, 6, 7, 8, 9, 11] {
            assert_eq!(doc.get_dict((n, 0)), None, "{n}");
        }
        assert_eq!(doc.catalog(), doc.get_dict((1, 0)));

        // To change: the object the chain ends at is edited, its record
        // and span dropped, and nothing else; nothing where none is given.
        doc.get_dict_mut((2, 0)).unwrap().set("Count", 1);
        assert_eq!(doc.record((3, 0)), None);
        assert_eq!(doc.get((3, 0)).unwrap().at, None);
        assert_eq!(doc.record((2, 0)), read.record((2, 0)));
        assert_eq!(doc.get((2, 0)), read.get((2, 0)));
        let count = doc.get_dict((2, 0)).and_then(|d| d.get(b"Count"));
        assert_eq!(count.and_then(Object::as_integer), Some(1));
        for n in [5, 6, 7, 8] {
            assert_eq!(doc.get_dict_mut((n, 0)), None, "{n}");
            assert_eq!(doc.record((n, 0)), read.record((n, 0)), "{n}");
        }
        doc.get_dict_mut((4, 0))
            .unwrap()
            .set("Filter", "FlateDecode");
        let stream = doc.get((4, 0)).unwrap().as_stream().unwrap();
        assert_eq!(
            stream.data,
            read.get((4, 0)).unwrap().as_stream().unwrap().data
        );
        doc.catalog_mut().unwrap().set("Lang", Object::string("en"));
        assert!(doc.catalog().is_some_and(|d| d.has(b"Lang")));
        assert_eq!(doc.record((1, 0)), None);

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
        let end = doc.get((228, 0)).unwrap();
        assert_eq!(
            doc.dereference(&reference(101)),
            Some((Some((228, 0)), end))
        );
        assert_eq!(doc.dereference(&reference(100)), None);
        assert_eq!(doc.get_dict((100, 0)), end.as_dict());
        assert_eq!(doc.get_dict((99, 0)), None);
    }

    /// A trailer's dictionary changed where it is: the trailer's, or the
    /// cross-reference stream's; a trailer that's neither made one.
    #[test]
    fn a_trailers_dictionary_is_changed_where_it_is() {
        let objects = [(1, dict! { "Type" => "Catalog" }.into())];
        let root = || dict! { "Root" => Object::reference(1) };
        for kind in [crate::fixture::Xref::Table, crate::fixture::Xref::Stream] {
            let file = crate::fixture::file("1.7", &objects, root(), kind);
            let mut doc = crate::bytes::read(&file);
            assert!(doc.trailer.object.at.is_some());
            let stream = doc.trailer.stream;
            doc.trailer.dict_mut().set("Info", Object::reference(2));
            assert_eq!(doc.trailer.object.at, None, "{kind:?}: an edit");
            assert!(doc.trailer.dict().has(b"Info") && doc.trailer.dict().has(b"Root"));
            let streamed = doc.trailer.object.as_stream().is_some();
            assert_eq!(streamed, kind == crate::fixture::Xref::Stream);
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
        doc.set_unread((4, 0), Unread::Other("it doesn't parse".to_string()));
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
        doc.set_unread((17, 0), Unread::Other("it doesn't parse".to_string()));
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
        let reached: Vec<u32> = doc.reachable().into_iter().map(|(n, _)| n).collect();
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
        doc.trailer.stream = Some((18, 0));
        doc.insert(19, Stream::new(dict! { "Type" => "ObjStm" }, Vec::new()));
        doc.decoded.insert((19, 0), b"20 0".to_vec());
        let before = doc.clone();
        assert_eq!(
            doc.prune(),
            [(13, 0), (15, 0), (16, 0), (17, 0), (18, 0), (19, 0)]
        );
        assert_eq!(
            doc.ids(),
            [1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 14].map(|n| (n, 0))
        );
        assert!(doc.objects().all(|(id, o)| before.get(id) == Ok(o)));
        assert_eq!(doc.get((4, 0)), before.get((4, 0)));
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
            let Some(file) = fixture(&format!("qpdf-{name}")) else {
                return;
            };
            let owner = [Password("owner")];
            for (passwords, by) in [(&[][..], None), (&owner, Some((0, Encoding::Utf8)))] {
                let owner = by.is_some();
                let mut doc = crate::bytes::read_opened(&file, passwords);
                assert_eq!(doc.encryption, Encryption::opened(owner).by(by), "{name}");
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
                    (&doc.trailer, &doc.decoded, &doc.warnings, doc.max_id()),
                    (
                        &before.trailer,
                        &before.decoded,
                        &before.warnings,
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
        shared::<Warning>();
        shared::<Update>();
        shared::<Encryption>();
        shared::<Unread>();
        shared::<Record>();
        shared::<Facts>();
    }
}
