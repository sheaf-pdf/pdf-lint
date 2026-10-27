//! What the checker reads a file through: a [`Source`], which any reader
//! can implement, and [`Document`], an owned one that readers can fill
//! and tests can build.

use std::collections::{BTreeMap, BTreeSet};

use crate::object::{Dict, Object, ObjectId, Span, Value};
use crate::version::Version;

/// A file, as a reader read it.
pub trait Source {
    /// Every object its cross-references list.
    fn ids(&self) -> Vec<ObjectId>;
    /// Object `id`, as written: references kept as references, strings
    /// decoded and decrypted. The error says why there's none: the file
    /// hasn't it, or it doesn't read.
    fn get(&self, id: ObjectId) -> Result<&Object, String>;
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
    /// Whether the string written at `at` was written as a hex string,
    /// `<...>`, when the reader knows.
    fn hex(&self, at: Span) -> Option<bool> {
        let _ = at;
        None
    }
    /// The data of the catalog's metadata stream (ISO 32000-2, 14.3.2), as
    /// written, when the reader has it in plain view.
    fn metadata(&self) -> Option<&[u8]> {
        None
    }
}

/// A file's trailer.
#[derive(Debug, Clone, PartialEq)]
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
pub struct Warning {
    /// The object it's in, when it's in one.
    pub object: Option<ObjectId>,
    /// Where, when the reader knows.
    pub at: Option<Span>,
    pub message: String,
}

impl Trailer {
    /// Its dictionary: the trailer's, or the cross-reference stream's.
    pub fn dict(&self) -> &Dict {
        static EMPTY: Dict = Dict::EMPTY;
        self.object.as_dict().unwrap_or(&EMPTY)
    }
}

/// A file read whole: its objects, trailer, header version and warnings.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// Each object its cross-references list, or why it doesn't read.
    pub objects: BTreeMap<ObjectId, Result<Object, String>>,
    pub trailer: Trailer,
    pub header: Option<Version>,
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
    /// Where the strings written as hex strings are, when the reader
    /// knows: a reader of the bytes does; one through another tool's JSON
    /// may not.
    pub hex: Option<BTreeSet<Span>>,
    /// The data of the catalog's metadata stream, as written, when it's in
    /// plain view: unfiltered, and not encrypted ([`Source::metadata`]).
    pub metadata: Option<Vec<u8>>,
    /// Whether the file is encrypted, and if it is, whether it opened.
    pub encryption: Encryption,
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
    /// user's: its strings and object streams are decrypted.
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

/// A file's newest incremental update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    /// The objects its cross-reference section lists in use.
    pub objects: BTreeSet<ObjectId>,
    /// Where the section it follows starts: the file as of that section
    /// is its previous revision ([`crate::bytes::read_from`]).
    pub previous: usize,
}

impl Document {
    /// A document with no objects yet, under `trailer`, its header at
    /// `version`.
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
            hex: None,
            metadata: None,
            encryption: Encryption::None,
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

    fn get(&self, id: ObjectId) -> Result<&Object, String> {
        match self.objects.get(&id) {
            Some(Ok(object)) => Ok(object),
            Some(Err(why)) => Err(why.clone()),
            None => Err(format!("{} {} R isn't in the file", id.0, id.1)),
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

    fn hex(&self, at: Span) -> Option<bool> {
        self.hex.as_ref().map(|hex| hex.contains(&at))
    }

    fn metadata(&self) -> Option<&[u8]> {
        self.metadata.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict;

    #[test]
    fn a_document_gives_what_it_holds() {
        let mut doc = Document::new(Version::new(1, 7), dict! { "Root" => Object::reference(1) });
        doc.insert(1, dict! { "Type" => "Catalog" });
        doc.objects
            .insert((2, 0), Err("it doesn't parse".to_string()));
        assert_eq!(doc.ids(), [(1, 0), (2, 0)]);
        assert!(doc.get((1, 0)).is_ok_and(|o| o.as_dict().is_some()));
        assert_eq!(doc.get((2, 0)), Err("it doesn't parse".to_string()));
        assert_eq!(doc.get((3, 0)), Err("3 0 R isn't in the file".to_string()));
        assert!(doc.trailer().dict().has(b"Root"));
        assert_eq!(doc.header_version(), Some(Version::new(1, 7)));
    }
}
