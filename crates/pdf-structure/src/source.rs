//! A file as a reader read it: a [`Source`], which any reader can
//! implement, and [`Document`], an owned one that readers fill and tests
//! build; and what the bytes reader records of where it read each object
//! ([`Record`]) and of the file as written ([`Facts`]).

use std::collections::{BTreeMap, BTreeSet};

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
}

/// A file read whole: its objects, trailer, header version and warnings.
/// A reader outside the crate starts from [`Document::new`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Document {
    /// Each object its cross-references list, or why it doesn't read.
    pub objects: BTreeMap<ObjectId, Result<Object, Unread>>,
    /// The trailer.
    pub trailer: Trailer,
    /// The version in the header.
    pub header: Option<Version>,
    /// What the reader had to choose how to read, in the order it chose.
    pub warnings: Vec<Warning>,
    /// The object streams' decoded data, by stream, which the objects
    /// read from them are placed in ([`Span::stream`]).
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
    /// Where each object that reads was read from, by the bytes reader;
    /// none from another.
    pub records: BTreeMap<ObjectId, Record>,
    /// What the file's cross-reference sections and header say of it, as
    /// written, by the bytes reader; the defaults from another.
    pub facts: Facts,
}

/// Where an object was read from, which the bytes reader records when it
/// first reads it ([`crate::bytes::Reading::record`]), in the reading it
/// read it in. Two records are equal when they say the same of where
/// their objects were read, whichever readings they're of, so that two
/// readings of one file are equal; [`Record::source`] tells the readings
/// apart.
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
    /// outside the crate makes it by [`Encryption::opened`].
    #[non_exhaustive]
    Opened {
        /// Whether by the owner's password.
        owner: bool,
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
/// and [`Document::objects`] holds. Each is one of the bytes reader's
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
    /// by the user's.
    pub fn opened(owner: bool) -> Encryption {
        Encryption::Opened { owner }
    }
}

impl Update {
    /// The update whose cross-reference section lists `objects` in use,
    /// after the section that starts at `previous`.
    pub fn new(objects: BTreeSet<ObjectId>, previous: usize) -> Update {
        Update { objects, previous }
    }
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
            records: BTreeMap::new(),
            facts: Facts::default(),
        }
    }

    /// Adds object `n`, generation 0.
    pub fn insert(&mut self, n: u32, object: impl Into<Object>) {
        self.objects.insert((n, 0), Ok(object.into()));
    }
}

impl Source for Document {
    fn ids(&self) -> Vec<ObjectId> {
        self.objects.keys().copied().collect()
    }

    fn get(&self, id: ObjectId) -> Result<&Object, Unread> {
        match self.objects.get(&id) {
            Some(Ok(object)) => Ok(object),
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

#[cfg(test)]
mod tests {
    use pdf_objects::dict;

    use super::*;

    #[test]
    fn a_document_gives_what_it_holds() {
        let mut doc = Document::new(Version::new(1, 7), dict! { "Root" => Object::reference(1) });
        doc.insert(1, dict! { "Type" => "Catalog" });
        let why = Unread::Other("it doesn't parse".to_string());
        doc.objects.insert((2, 0), Err(why.clone()));
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
