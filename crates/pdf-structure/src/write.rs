//! A writer of a file's structure (ISO 32000-2, 7.5): a whole file
//! ([`full`], [`document`]) or an incremental update after one
//! ([`update`]), given numbered objects; and the parts either is laid out
//! in, for a caller that lays a file out itself: its cross-reference
//! sections, as a table or a stream, and object streams of the objects
//! that may go in one, deflated through [`pdf_filters`]. The objects are
//! written by pdf-objects' writer ([`pdf_objects::serialize`]), each at the
//! position a section lists it at.
//!
//! - **A whole file** ([`full`]): the header, each object at its number,
//!   in the order given, the encryption dictionary, the object streams,
//!   and one cross-reference section, a table or a stream, with no
//!   `/Prev`; the trailer the caller's but for the keys a section writes
//!   itself, and an `/ID` whose second string is a hash of what's
//!   written, so the same objects write the same bytes. Packed
//!   ([`Options::pack`]), what may go in an object stream goes in one,
//!   built on threads. [`document`] writes a [`Document`]'s
//!   objects so, but those a whole file leaves out.
//! - **An update** ([`update`]): after a file, each object at its number,
//!   and a section in the file's kind listing only those, its `/Prev` the
//!   file's last section (7.5.6); the file's trailer carried, and what it
//!   writes said ([`Written`]), so a second update goes after it without
//!   reading it again. What it goes after is a [`Base`]: a reading's
//!   ([`Base::of`]), or one a caller describes ([`Base::new`]).
//! - **Copies:** an object as it was read ([`Body::Read`], a document's
//!   by [`bodies`]) is written as the bytes it was read from, read again
//!   from the reading given ([`ReadFrom`]), where they're right to copy: at
//!   its number, under the key it was encrypted with, or none; an object
//!   stream's member into an object stream of the write's. Its bytes are
//!   checked first against a hash of them taken as it was read, and the
//!   reading's source asked whether it changed ([`ByteSource::unchanged`]);
//!   where they changed, the write is refused. An update passes over what
//!   its file holds as read, so a document's bodies write what changed.
//! - **Renumbered** ([`Options::renumber`]): a whole file's or an update's
//!   objects written at the numbers a caller's renumbering gives them, each
//!   reference named as it names it, `null` where it names none, the
//!   trailer's too. What's given stays at its numbers, which a stream's
//!   data, a copy's record and what the encryptor resolves are found by; an
//!   encrypted file's objects are encrypted under the numbers they're
//!   written at, a whole file's by the encryptor in those numbers
//!   ([`Encryptor::renumbered`]), an update's by its file's, whose
//!   encryption dictionary and what that refers to it doesn't move. An
//!   object as read is copied as its bytes, its header and each reference
//!   that moves written anew where its bytes had it; at another number only
//!   where its key isn't made of its number. An update passes over only
//!   what doesn't move, nor names what does.
//! - **Cross-reference sections** ([`Section`]): a whole file's one
//!   subsection, `[0, /Size)`, every number it doesn't list written as
//!   free; or an update's, runs of the numbers it lists, which are only
//!   those it writes (7.5.6). Free entries are linked in a list from entry
//!   0, each at the generation its number is to be used at next (7.5.4).
//!   An object in an object stream is listed as its container and its
//!   place there, which only a stream can say (7.5.8.3). A whole file's
//!   stream is deflated under PNG's Up predictor, each row less the row
//!   above it, as qpdf writes its own; an update's is written as it is.
//! - **Positions counted from the header,** where `%PDF-` starts, which a
//!   whole file's section has at its start: PDFium, pdf.js and qpdf count
//!   them so, past any bytes a writer put before the header, where ISO
//!   32000-2 counts them from the file's start (7.5.4), as this crate's
//!   reader does. The two differ only for an update of a file with bytes
//!   before its header ([`Section::append`]), whose sections this crate's
//!   reader reads only by scanning.
//! - **Object streams** (7.5.7): which objects may go in one
//!   ([`packable`]), and a container of them ([`container`]), its members
//!   direct after a header of where each starts, deflated, no bigger than
//!   the reader decodes a stream to ([`MAX_DATA`]).
//! - **Deflated** by the backend and at the level a [`Deflate`] says.
//! - **Encrypted as it's written** (7.6): an encrypted file's every string
//!   and stream's data, as pdf-objects' writer writes each, by an
//!   [`Encryptor`], a reading's ([`Encryptor::of`]) or a new file's
//!   ([`Encryptor::create`]), through what it gives for the file's objects
//!   ([`Encrypting`]): with the object's key, AES's IV derived from `n g
//!   obj` and the data, so the same object encrypts to the same bytes on
//!   every save. What it leaves in the clear is the list the reader
//!   decrypts by, one list both ways (see [`bytes`](crate::bytes), "What's
//!   encrypted"). Under RC4 a changed object written again at its number
//!   and generation is encrypted with the keystream its earlier version
//!   was, which leaks what changed to one who holds both (pdf-crypt's
//!   `Method::Rc4`), and so is an object a renumbering writes at a number
//!   another object had, which leaks the XOR of the two; a file written
//!   whole under a new AES-256 encryption ([`Encryptor::create`]) doesn't.
//!
//! What it can't write is refused ([`Error`]), and then nothing is
//! written.

mod append;
mod copy;
mod crypt;
mod full;
mod objstm;
#[cfg(test)]
pub(crate) mod tests;
mod xref;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use pdf_filters::{Backend, Encoder, Level, Prediction, Unencodable};
use pdf_objects::Resolve;
use pdf_objects::object::{Dict, Object, ObjectId, Span, Value};
use pdf_objects::serialize::{self, Refused, Renumber};
use pdf_objects::version::Version;

use crate::bytes::{ByteSource, Reading, SourceId, XrefEntry};
use crate::crypt::Get;
use crate::source::{Document, Record, XrefKind};

pub use append::{Base, update};
pub use crypt::{Encrypting, Encryptor, Unencryptable};
pub use full::{document, full};
pub use objstm::{MAX_DATA, MEMBERS, container, fits, packable};
pub use xref::{Entry, Section};

// A reading compares a document's encryption dictionary as written too
// (`Reading::adopt`).
pub(crate) use append::as_written;

// The tests' file writer writes its encrypted files' dictionaries so too.
#[cfg(any(test, feature = "fixtures"))]
pub(crate) use crypt::encryption;

/// The object numbers a written file may use are below it: the
/// 8,388,607 indirect objects a file may hold (ISO 32000-1, Annex C), so
/// that a whole file's cross-reference section, which lists every number
/// from 0, has at most that many rows. It's below 2^24 too: RC4's and
/// AES-128's key for an object is made from the low three bytes of its
/// number (ISO 32000-2, 7.6.3, Algorithm 1), so no two numbers below it
/// share a key.
pub const NUMBERS: u32 = 8_388_607;

/// How the writer deflates what it compresses, a whole file's
/// cross-reference stream and each object stream: on which of
/// pdf-filters' backends, and how hard. A caller sets them on a copy of
/// [`Deflate::DEFAULT`], as it does a reading's
/// [`Options`](crate::Options).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Deflate {
    /// The backend that deflates: by default pdf-filters'
    /// `Backend::MinizOxide`, which every build of this crate has;
    /// `Backend::ZlibRs` with its `zlib-rs` feature. The two deflate to
    /// different bytes, each zlib data any inflater reads.
    pub backend: Backend,
    /// How hard it works to make the data small: pdf-filters'
    /// `Level::DEFAULT`, 6, by default.
    pub level: Level,
}

impl Deflate {
    /// How the writer deflates unless it's told otherwise: miniz_oxide, at
    /// level 6.
    pub const DEFAULT: Deflate = Deflate {
        backend: Backend::MinizOxide,
        level: Level::DEFAULT,
    };

    /// `data`, first predicted as `predictor` says, deflated at the end of
    /// `out`.
    fn encode(self, data: &[u8], predictor: Prediction, out: &mut Vec<u8>) -> Result<(), Error> {
        let flate = Encoder::Flate {
            level: self.level,
            predictor,
        };
        pdf_filters::encode(data, &[flate], self.backend, out).map_err(Error::Unencodable)
    }
}

impl Default for Deflate {
    fn default() -> Deflate {
        Deflate::DEFAULT
    }
}

/// What the writer writes of an object, at the number it's given with.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Body<'a> {
    /// An object written anew, from its value: built, or changed since it
    /// was read. A stream's data is its [`Data::Bytes`](pdf_objects::object::Data::Bytes),
    /// or, where it's [`Data::At`](pdf_objects::object::Data::At), the
    /// data of the stream the reading the write is given read at that
    /// number there, as the reading hands it out.
    New(Cow<'a, Object>),
    /// An object as it was read, with where it was read from
    /// ([`Reading::record`], [`Document::record`]; [`bodies`] gives a
    /// document's so): copied as the bytes it was read from where those
    /// are right to copy, else written anew from `object`, as
    /// [`Body::New`] is. An update after the reading's own file passes
    /// over it ([`Base::source`]), its bytes there already, where it's as
    /// read at the number it's given, and a renumbering moves neither it
    /// nor a reference in it: given at another, it refuses the update
    /// ([`Error::Elsewhere`]), as rule 3 says.
    ///
    /// Copied, an object read at an offset is written as its bytes from
    /// `n g obj` to past `endobj`, ciphertext and all; one read from an
    /// object stream, when it goes in one of the write's
    /// ([`Options::pack`]), as its bytes there, which are plaintext in any
    /// reading. A packed write puts what may go in an object stream
    /// ([`packable`]) in one whether it was read at an offset or not, so
    /// such an object isn't copied at its offset there but written anew in
    /// a container: a caller that would keep it copied, as a writer that
    /// packs only what it writes anew does, leaves its number out of every
    /// group ([`Pack::groups`]), which writes it on its own. Which is the
    /// caller's. Under a renumbering ([`Options::renumber`]), its bytes
    /// are copied but its header, `n g obj`, where it's written at another
    /// number, and each reference the renumbering names otherwise, `n g R`
    /// or `null`, each written anew where its bytes had it (rule 9). A copy
    /// is made only from the reading the write is given ([`ReadFrom`]), so
    /// with none it's written anew; and only:
    ///
    /// 1. where `record` is that reading's ([`Record::source`]): else the
    ///    write is refused ([`Error::Elsewhere`]);
    /// 2. for an object at an offset, where the write is encrypted as the
    ///    reading's file is, by an encryptor of that reading
    ///    ([`Encryptor::of`]), and the list of what's left in the clear
    ///    says the same of the object in both (see
    ///    [`bytes`](crate::bytes), "What's encrypted"), or where neither is
    ///    encrypted: its strings and data are encrypted with its key. Under
    ///    a renumbering, at another number than it was read as only where
    ///    neither is encrypted, or no method of that encryptor makes a key
    ///    per object (RC4 and AES-128 make one of the object's number and
    ///    generation, ISO 32000-2, 7.6.3, Algorithm 1; AES-256's is the
    ///    file's, Algorithm 1.A, its IV beside its ciphertext): else it's
    ///    written anew, encrypted under the number it's written at. At its
    ///    own number, its references are rewritten under any encryption, as
    ///    a reference isn't encrypted. An object read from an object stream
    ///    is plaintext there, and an object stream is encrypted whole, so it
    ///    goes in one under any encryption, or none, at any number;
    /// 3. where `record` was read as the id it's given at
    ///    ([`Record::id`]), and `object` is `record`'s, as read: else the
    ///    write is refused ([`Error::Elsewhere`]), however it's encrypted,
    ///    whether or not it would copy; and where its bytes are its
    ///    number's, an object at an offset's starting with `n g obj` for
    ///    its id, one in an object stream at the place the reading's
    ///    sections give its number, else refused so. One an edit changed,
    ///    which dropped its place ([`Object::at`]), is written anew;
    /// 4. where it wasn't found by scanning
    ///    ([`Origin::Recovered`](crate::Origin::Recovered)), as the file's
    ///    sections didn't read;
    /// 5. for an object at an offset, where `endobj` follows it;
    /// 6. for a stream, where its `/Length` is no reference, and its data is
    ///    as long as that says ([`DataExtent`](crate::DataExtent));
    /// 7. for an object read from an object stream, where it goes in one of
    ///    the write's: on its own, it's written anew, as its strings would
    ///    otherwise be in the clear in an encrypted file;
    /// 8. where its bytes, read again, hash as they did when it was read:
    ///    else the write is refused ([`Error::Changed`]), as the file
    ///    changed under the reading;
    /// 9. under a renumbering, where its references are where its bytes
    ///    have them: each, as [`Object::references`] walks them, placed
    ///    ([`Object::at`]) among the bytes `record` names, past its header
    ///    and before a stream's data, in the order walked and apart, and
    ///    each the renumbering names otherwise reading there as itself,
    ///    `n g R`, three tokens of their own: else `object` isn't what its
    ///    bytes say, and the write is refused ([`Error::Elsewhere`]),
    ///    however it's encrypted. A comment between a changed reference's
    ///    tokens goes with them; every other byte, strings and data among
    ///    them, is copied as it was. Without a renumbering, nothing of it is
    ///    asked.
    Read {
        /// The object as read: written anew from it where it isn't copied.
        object: Cow<'a, Object>,
        /// Where it was read from.
        record: Cow<'a, Record>,
    },
}

impl Body<'_> {
    /// The object it writes.
    fn object(&self) -> &Object {
        match self {
            Body::New(object) | Body::Read { object, .. } => object,
        }
    }
}

/// `doc`'s objects as a write's bodies, by number: each as it was read,
/// with its record, while it is ([`Body::Read`]), which a write given the
/// reading it was read in copies, and an update after that reading's file
/// passes over; the rest, built or changed, written anew ([`Body::New`]).
/// The reading is the one `doc` was made of by [`Reading::to_document`]: a
/// document read whole ([`bytes::read`](crate::bytes::read),
/// [`Reading::into_document`]) has records of a reading that's gone, which
/// a write given another reading refuses ([`Error::Elsewhere`]), and a
/// stream whose data no reading given holds refuses one given none
/// ([`Error::Placed`]), until a reading of its file opened again takes them
/// as its own ([`Reading::adopt`]).
pub fn bodies(doc: &Document) -> impl DoubleEndedIterator<Item = (ObjectId, Body<'_>)> {
    doc.objects_read().map(|(id, object, record)| {
        let object = Cow::Borrowed(object);
        let body = match record {
            Some(record) => Body::Read {
                object,
                record: Cow::Borrowed(record),
            },
            None => Body::New(object),
        };
        (id, body)
    })
}

impl From<Object> for Body<'_> {
    fn from(object: Object) -> Self {
        Body::New(Cow::Owned(object))
    }
}

impl<'a> From<&'a Object> for Body<'a> {
    fn from(object: &'a Object) -> Self {
        Body::New(Cow::Borrowed(object))
    }
}

/// How a whole file or an update is written. A caller sets the fields on
/// a copy of [`Options::DEFAULT`] (`options.kind = XrefKind::Stream`, say),
/// as it does a reading's [`Options`](crate::Options).
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct Options<'a> {
    /// A whole file's cross-reference section: a table, by default, or a
    /// stream. An update's is its base's ([`Base::kind`]).
    pub kind: XrefKind,
    /// The version a whole file's header says: 1.7 by default. It's
    /// written as given, a stream section under 1.5 too.
    pub version: Version,
    /// Whether a whole file puts what may go in an object stream in one,
    /// and how: none by default. Only a cross-reference stream lists an
    /// object stream's members, so with a table it's refused
    /// ([`Error::PackedTable`]); an update writes each object on its own.
    pub pack: Option<Pack<'a>>,
    /// How what's compressed is deflated: a whole file's cross-reference
    /// stream and each object stream.
    pub deflate: Deflate,
    /// How many threads object streams are built on, at most: 0, the
    /// default, for as many as the machine runs at once, up to 8, or one
    /// where that isn't known. Built on any number, they're the same bytes.
    pub threads: usize,
    /// Where a whole file's encryption dictionary goes, when it's written
    /// as an object, a number the file uses ([`Options::renumber`]): by
    /// default at its number in the file its [`Encryptor`] was read from,
    /// as the renumbering names it, and a new file's past every object
    /// written, as is one whose number the renumbering names none of. One
    /// that file had in its trailer stays there.
    pub encrypt_at: Option<ObjectId>,
    /// The least number a whole file's cross-reference stream and its new
    /// object streams take, and an update's cross-reference stream: past
    /// every number written, by default (0), as they always are; each
    /// passes over a number what's written refers to and doesn't hold.
    pub xref_from: u32,
    /// A flag that, set while the write runs, stops it: it's checked
    /// before each object and each object stream, and the write then
    /// writes nothing ([`Error::Stopped`]).
    pub stop: Option<&'a AtomicBool>,
    /// Where each object of a whole file ([`full`], [`document`]) or an
    /// update ([`update`]) goes: given the number and generation an object
    /// is given at, or a reference in what's written names, those it's
    /// written at, or none for an object the file doesn't hold, a reference
    /// to it then written `null`, as a reference to no object reads (ISO
    /// 32000-2, 7.3.10). Any function from an [`ObjectId`] to an
    /// `Option<ObjectId>` is one (pdf-objects' `serialize::Renumber`), and
    /// it is a function: asked of the same id many times, in any order and
    /// on several threads at once, it gives the same answer each time, as
    /// what's written by number in one place (an object, the list of what's
    /// left in the clear, a reference) must name what's there in another; a
    /// numbering made as it's asked is made first and given as a map. None,
    /// the default, writes each at the number it's given at. Any renumbering
    /// given counts as one, the identity among them (`&()`, or a function
    /// that names each as it is): its copies are held to their references'
    /// places, and what's written says it was renumbered, so an update
    /// after it refuses what's read ([`Base::renumbered`]); a write with
    /// nothing to renumber is given none. What an update is given is in its
    /// file's numbers, and it leaves the encryption dictionary and what that
    /// refers to where they are ([`update`], "Renumbered").
    ///
    /// - **What's given keeps its numbers:** a document's ids, an object as
    ///   read at the number it was read as ([`Record::id`]); a stream's
    ///   data where it was written, a copy's record and what the encryptor
    ///   resolves are found by them. Each object is written at the number
    ///   the renumbering gives it, refused where it gives none
    ///   ([`Error::Unnumbered`]), and each reference in it and in the
    ///   trailer is written as the renumbering names it. Two numbers may be
    ///   given one, a reference to either then naming the object there, but
    ///   one object is written at a number ([`Error::WrittenTwice`]): a
    ///   caller whose renumbering shares a number gives one object at it,
    ///   which [`document`], giving every object of the document, doesn't.
    ///   Under an encryption, the number of an encryption dictionary or
    ///   what one refers to isn't shared by another object given, which
    ///   would be written there in the clear ([`Error::Moved`]).
    /// - **The numbers the file uses are those written:** 0 and
    ///   [`NUMBERS`], a number two objects are written at, the encryption
    ///   dictionary's ([`Options::encrypt_at`]), one the encryptor's file
    ///   lists free under keys made per object ([`Error::Free`]), which a
    ///   renumbering passes over ([`Encryptor::free_numbers`]),
    ///   [`Pack::reuse`], those the writer gives its own objects, and an
    ///   update's file's where it passes over an object as read;
    ///   [`Pack::groups`] is given the numbers packed objects are given at.
    ///   An object given past [`NUMBERS`] and written below it is written.
    /// - **Errors name** an object by the number it's given at
    ///   ([`Error::Unnumbered`], [`Error::Elsewhere`], [`Error::Changed`],
    ///   [`Error::Placed`], [`Error::Data`], [`Error::Group`],
    ///   [`Error::Twice`], and [`Error::Moved`] the encryption's object); a
    ///   number the file uses by the number written ([`Error::Number`],
    ///   [`Error::WrittenTwice`], [`Error::EncryptAt`], [`Error::Free`]);
    ///   and what pdf-objects' writer or the encryptor refuses by the
    ///   number it was writing at ([`Error::Serialize`],
    ///   [`Error::Encrypt`]).
    /// - **Encrypted**, each object is encrypted under the number it's
    ///   written at: a whole file's by the encryptor in the numbers written
    ///   ([`Encryptor::renumbered`]: what it leaves in the clear, its
    ///   dictionary's place and the references in it), the document's
    ///   metadata stream found in the numbers given and kept as its number
    ///   written ([`Encryptor::encrypting_renumbered`]); an update's by its
    ///   encryptor as it is, in its file's numbers, the metadata stream
    ///   told by the number it's written at: it doesn't write the
    ///   dictionary again, and moves neither it nor what it refers to
    ///   ([`Error::Moved`]). **Under RC4**, an object written at a number
    ///   another object had in the file the encryptor was read from is
    ///   encrypted with that object's keystream, the key and `/ID[0]` being
    ///   the file's: where the earlier file is kept, or the update's file
    ///   holds both, the XOR of the two ciphertexts is the XOR of the two
    ///   plaintexts, for every object moved onto another's number
    ///   (pdf-crypt's `Method::Rc4`). A caller who doesn't want that writes
    ///   the file under a new AES-256 encryption ([`Encryptor::create`])
    ///   instead.
    /// - **Copied**, an object as read ([`Body::Read`]) is its bytes, its
    ///   header and each reference the renumbering names otherwise written
    ///   anew where its bytes had them, where the rules for a copy hold:
    ///   at another number than it was read as only where neither file is
    ///   encrypted, or no method of the encryptor makes keys per object
    ///   (AES-256); else it's written anew. Its references are checked to be
    ///   where its bytes have them first ([`Error::Elsewhere`]).
    /// - **What's written says so** ([`Written::base`],
    ///   [`Base::renumbered`]): an update after it is given objects at the
    ///   numbers written, built or edited, and refuses an object as read
    ///   and a reading given ([`Error::Renumbered`]), whose numbers aren't
    ///   the file's any more.
    pub renumber: Option<&'a (dyn Renumber + Sync + 'a)>,
}

impl Options<'static> {
    /// How a file is written unless it's told otherwise: a whole file with
    /// a table, its header 1.7, nothing in object streams, deflated as
    /// [`Deflate::DEFAULT`] deflates, on as many threads as the machine
    /// runs, its encryption dictionary where its encryptor had it, nothing
    /// to stop it.
    pub const DEFAULT: Options<'static> = Options {
        kind: XrefKind::Table,
        version: Version::new(1, 7),
        pack: None,
        deflate: Deflate::DEFAULT,
        threads: 0,
        encrypt_at: None,
        xref_from: 0,
        stop: None,
        renumber: None,
    };
}

impl Default for Options<'_> {
    fn default() -> Self {
        Options::DEFAULT
    }
}

impl fmt::Debug for Options<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Options")
            .field("kind", &self.kind)
            .field("version", &self.version)
            .field("pack", &self.pack)
            .field("deflate", &self.deflate)
            .field("threads", &self.threads)
            .field("encrypt_at", &self.encrypt_at)
            .field("xref_from", &self.xref_from)
            .field("stop", &self.stop)
            .field("renumber", &self.renumber.map(|_| "a caller's"))
            .finish()
    }
}

/// Which object streams a whole file's packed objects go in: given the
/// numbers packed, ascending, each object stream's members, in order. A
/// number not packed, or named twice, is refused ([`Error::Group`]); one
/// left out of every group is written on its own; an empty group makes no
/// object stream. Under a renumbering ([`Options::renumber`]), the numbers
/// are those the packed objects are given at, as a caller groups its own
/// objects by what it knows of them.
pub type Groups<'a> = dyn Fn(&[u32]) -> Vec<Vec<u32>> + Sync + 'a;

/// How a whole file puts objects in object streams (ISO 32000-2, 7.5.7):
/// what it writes anew that may go in one ([`packable`]), but an
/// encryption dictionary and what one refers to, which a reader reads
/// before it has a key. A caller sets the fields on a copy of
/// [`Pack::DEFAULT`]. Under an encryption that leaves streams in the
/// clear and strings not, nothing is packed ([`Encryptor::packs`]).
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct Pack<'a> {
    /// The object streams' members ([`Groups`]); none, the default, for
    /// [`MEMBERS`] each, by number.
    pub groups: Option<&'a Groups<'a>>,
    /// Numbers the object streams take first, in order: a source's own
    /// object streams', say, whose numbers are free once their members are
    /// written anew. 0, a number past what a file may use ([`NUMBERS`]),
    /// one an object written has, one given already and one the file the
    /// encryptor was read from lists free under keys made per object are
    /// passed over; then they're numbered past every object written and
    /// [`Options::xref_from`]. They're numbers the file uses, so under a
    /// renumbering ([`Options::renumber`]) they're numbers written.
    pub reuse: &'a [u32],
}

impl Pack<'static> {
    /// Objects in object streams of [`MEMBERS`] each, by number, numbered
    /// past every object written.
    pub const DEFAULT: Pack<'static> = Pack {
        groups: None,
        reuse: &[],
    };
}

impl Default for Pack<'_> {
    fn default() -> Self {
        Pack::DEFAULT
    }
}

impl fmt::Debug for Pack<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pack")
            .field("groups", &self.groups.map(|_| "a caller's"))
            .field("reuse", &self.reuse)
            .finish()
    }
}

/// What a write wrote: where each object landed, and the file it's now,
/// which an update after it goes after without reading it again.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Written {
    /// Its cross-reference section: where each object it wrote is, and the
    /// cross-reference stream's own, when it's one, as positions in the
    /// file: a whole file's counted from where it starts in `out`, its
    /// header; an update's from its file's first byte, as [`Base::len`]
    /// counts, past any bytes before the header, which the section's rows
    /// are counted from as it writes them.
    pub section: Section,
    /// Where its cross-reference section starts, counted from the header,
    /// as its `startxref` says.
    pub start: usize,
    /// Its trailer's `/Size`.
    pub size: u32,
    /// Its trailer's `/ID`, when it has one.
    pub id: Option<[Vec<u8>; 2]>,
    /// The file as it is now, for an update after it. After a whole file
    /// it's of no reading ([`Base::source`] none), so an update after it
    /// passes over nothing as read: a document's [`bodies`] given again,
    /// their records still of the reading the document was read in, are
    /// each written again, copied where they copy. After a file written
    /// under a renumbering ([`Options::renumber`]), whole or as an update,
    /// it says so ([`Base::renumbered`]): the document's numbers aren't
    /// the file's, so an update after it is given objects at the numbers
    /// written, built or edited, under the encryptor in those numbers
    /// (after a whole file, [`Encryptor::renumbered`]'s), and refuses an
    /// object as read and a reading ([`Error::Renumbered`]); one that
    /// copies opens the file written and goes after [`Base::of`] that
    /// reading. With no reading to resolve it, an update after a
    /// renumbered write is given the catalog where `/EncryptMetadata false`
    /// leaves the metadata stream it names in the clear ([`Error::Catalog`]).
    /// After an update it's of the reading its base was, and renumbered
    /// where its base was or the update was.
    pub base: Base,
}

/// The reading the objects a write is given were read in, which a stream's
/// data where it was written ([`Data::At`](pdf_objects::object::Data::At))
/// is read from, an object as read is copied from ([`Body::Read`]), and
/// what their references reach that they don't hold. Implemented by
/// [`Reading`] alone, as `&dyn ReadFrom`, so that a write that reads
/// nothing doesn't name a source's type; its methods are the writer's
/// own. A write given one asks its source first whether its bytes are
/// those it was read from ([`ByteSource::unchanged`]), and is refused
/// where they aren't ([`Error::Changed`]).
pub trait ReadFrom: sealed::Reads + Sync {}

impl<S: ByteSource> ReadFrom for Reading<S> {}

/// What the writer reads of a [`ReadFrom`]: its methods, which nothing
/// outside the crate calls or implements.
mod sealed {
    use std::borrow::Cow;

    use pdf_objects::object::{Object, ObjectId, Span};

    use super::Error;
    use crate::bytes::{SourceId, XrefEntry};

    /// What the writer reads of the reading its objects were read in.
    pub trait Reads {
        /// Stream `id`'s data, as the reading hands it out, where the
        /// reading's stream `id` has its data at `span`: refused
        /// otherwise ([`Error::Placed`]).
        fn data(&self, id: ObjectId, span: Span) -> Result<Cow<'_, [u8]>, Error>;

        /// Object `id`, as the reading reads and keeps it.
        fn object(&self, id: ObjectId) -> Option<&Object>;

        /// Which reading it is, which its records name.
        fn id(&self) -> SourceId;

        /// Nothing, where its source's bytes are those it had as the
        /// reading opened it, as far as the source tells: refused where
        /// they aren't ([`Error::Changed`]), or it can't tell
        /// ([`Error::Io`]).
        fn unchanged(&self) -> Result<(), Error>;

        /// The bytes at `span`, object `id`'s, as written, an encrypted
        /// file's as they are: refused where they aren't there to read any
        /// more ([`Error::Changed`]), or don't read ([`Error::Io`]).
        fn written(&self, id: ObjectId, span: Span) -> Result<Cow<'_, [u8]>, Error>;

        /// The newest entry its sections give number `n`.
        fn entry(&self, n: u32) -> Option<XrefEntry>;

        /// Whether its file is encrypted: whether what it reads as written
        /// is, all of it, as written.
        fn encrypts(&self) -> bool;

        /// Which stream is the document's metadata, as the reading finds it
        /// (its catalog's `/Metadata`): what `/EncryptMetadata false`
        /// leaves in the clear.
        fn metadata(&self) -> Option<ObjectId>;
    }
}

impl<S: ByteSource> sealed::Reads for Reading<S> {
    fn data(&self, id: ObjectId, span: Span) -> Result<Cow<'_, [u8]>, Error> {
        match self.get(id).map(Object::value) {
            Ok(Value::Stream(stream)) if stream.data.at() == Some(span) => {}
            _ => return Err(Error::Placed(id)),
        }
        Reading::data(self, id).map_err(|why| Error::Data {
            id,
            why: why.to_string(),
        })
    }

    fn object(&self, id: ObjectId) -> Option<&Object> {
        let reading: &Reading<S> = self;
        reading.resolve(id)
    }

    fn id(&self) -> SourceId {
        Reading::id(self)
    }

    fn unchanged(&self) -> Result<(), Error> {
        match self.source().unchanged() {
            Ok(true) => Ok(()),
            Ok(false) => Err(Error::Changed(None)),
            Err(e) => Err(Error::io(&e)),
        }
    }

    fn written(&self, id: ObjectId, span: Span) -> Result<Cow<'_, [u8]>, Error> {
        Reading::written(self, span).map_err(|why| match why {
            crate::bytes::Error::Io(e) => Error::io(&e),
            _ => Error::Changed(Some(id)),
        })
    }

    fn entry(&self, n: u32) -> Option<XrefEntry> {
        Reading::entry(self, n).map(|(_, entry)| entry)
    }

    fn encrypts(&self) -> bool {
        self.list().encrypts
    }

    fn metadata(&self) -> Option<ObjectId> {
        let reading: &Reading<S> = self;
        let get: Get<'_, '_, std::convert::Infallible> = &|id| Ok(reading.resolve(id));
        let Ok(metadata) = crate::crypt::metadata_stream(self.trailer().dict(), get);
        metadata
    }
}

/// Why the writer wrote nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An offset past a cross-reference table's ten digits.
    #[error("a cross-reference table can't list offset {0}")]
    TooFar(u64),
    /// An object in an object stream, by number: only a cross-reference
    /// stream can list one.
    #[error("a cross-reference table can't list object {0}, which is in an object stream")]
    Packed(u32),
    /// A position before the header, which a section's positions are
    /// counted from ([`Section::append`]).
    #[error("position {pos} is before the header, at {header}, which positions are counted from")]
    BeforeHeader {
        /// The position.
        pos: usize,
        /// Where the header starts.
        header: usize,
    },
    /// A value that pdf-objects' writer refuses: its error, which says
    /// why and in which object.
    #[error("{0}")]
    Serialize(serialize::Error),
    /// Data that pdf-filters doesn't encode as it was asked to: its error.
    #[error("data that doesn't encode: {0}")]
    Unencodable(Unencodable),
    /// A string or a stream's data of object `id`, in a file written
    /// encrypted, that isn't encrypted, and why: nothing is written in the
    /// clear in its place.
    #[error("object {} {} can't be encrypted: {why}", id.0, id.1)]
    Encrypt {
        /// The object.
        id: ObjectId,
        /// Why.
        why: Unencryptable,
    },
    /// An encrypted file that no password opened, whether its strings and
    /// streams are encrypted or not: there's no key to encrypt what's
    /// written into it by ([`Encryptor::of`]).
    #[error("it's encrypted, and no password opened it, so there's no key to encrypt by")]
    Locked,
    /// An encrypted file whose security handler the reader can't open, and
    /// why ([`Encryptor::of`]).
    #[error("it's encrypted by a security handler the reader can't open: {0}")]
    Unsupported(String),
    /// An object number a written file may not use: 0, which heads the
    /// free list (ISO 32000-2, 7.5.4), or one at or past [`NUMBERS`], which
    /// a whole file's section would list every number up to.
    #[error("{}", number(*.0))]
    Number(u32),
    /// An object number given twice, at any generation: under a
    /// renumbering ([`Options::renumber`]), an update's among them where one
    /// of the two is an object as read it passes over, which is in its file
    /// at that number already ([`Body::Read`]).
    #[error("object {0} is given twice")]
    Twice(u32),
    /// Under a renumbering ([`Options::renumber`]), the number written that
    /// it gives two objects given at two numbers, or that an update gives
    /// an object where its file holds one it passes over: one object is
    /// written at a number. Two numbers given one name the object there,
    /// and a caller gives one object at it.
    #[error("two objects would be written at object number {0}")]
    WrittenTwice(u32),
    /// An object at the number a whole file writes its encryption
    /// dictionary at, or the number an update's file has it at.
    #[error("object {0} is the encryption dictionary's")]
    EncryptAt(u32),
    /// A whole file with a cross-reference table asked to put objects in
    /// object streams, whose members only a stream lists.
    #[error("a cross-reference table can't list objects in object streams")]
    PackedTable,
    /// An update whose cross-reference stream would be numbered at or past
    /// [`NUMBERS`], past every number its file and it use: its number.
    #[error("no object number left for an update's cross-reference stream: it would be {0}")]
    NoNumber(u32),
    /// A file with this many bytes before its header ([`Base::of`]): this
    /// crate's reader counts its positions from the file's start, and
    /// PDFium, pdf.js and qpdf from the header, so an update's would be
    /// misread by one or the other; [`Base::new`] says which.
    #[error(
        "it has {0} bytes before its header, so an update's positions would count from where one reader or another doesn't"
    )]
    Prefixed(usize),
    /// A file whose cross-reference sections didn't read, which was read
    /// by scanning: there's no section for an update's `/Prev` to give
    /// ([`Base::of`]).
    #[error("its cross-reference sections didn't read, so an update has no /Prev to give")]
    Unsound,
    /// A trailer that names no catalog: no `/Root`.
    #[error("its trailer names no catalog (/Root)")]
    Rootless,
    /// A reading as of the section at this byte, not the file's last
    /// ([`Base::of`]): an update after it would leave every later revision
    /// unreachable.
    #[error("it's read as of the section at byte {0}, not its last, which an update goes after")]
    Earlier(usize),
    /// An update whose encryption isn't its file's: none for an encrypted
    /// file, or one for a file that isn't, or another file's, whose
    /// dictionary is elsewhere or isn't the one the file stores
    /// ([`Base::encryption`]), or whose `/ID[0]` isn't the file's. An
    /// update can't change how the revisions before it are encrypted, and
    /// under another file's key what it writes wouldn't decrypt.
    #[error("an update is encrypted as its file is, and this one's encryption isn't its file's")]
    Unmatched,
    /// An object at a number the file its [`Encryptor`] was read from
    /// lists free, under keys made per object (RC4, AES-128): it would be
    /// encrypted with a deleted object's key ([`Encryptor::of`]).
    #[error(
        "object {} {} is at a number its file lists free, so its key would be a deleted object's",
        .0.0, .0.1
    )]
    Free(ObjectId),
    /// A stream whose data is where it was written
    /// ([`Data::At`](pdf_objects::object::Data::At)), which isn't where
    /// the reading the write was given read that number's stream's data,
    /// or with no reading given: its data is given as bytes instead, or the
    /// reading it was read in given, which a document read whole
    /// ([`bytes::read`](crate::bytes::read)) has dropped
    /// ([`Reading::to_document`] keeps it), or a reading of its file opened
    /// again that took the document ([`Reading::adopt`]).
    #[error(
        "stream {} {}'s data is where no stream of the reading given read it at that number",
        .0.0, .0.1
    )]
    Placed(ObjectId),
    /// A stream's data that the reading given doesn't hand out, and why:
    /// the reading's error as words, which a caller can't match on, as the
    /// reading's error isn't `Clone` or `Eq` and this one is; the reading
    /// gives it typed ([`Reading::data`]).
    #[error("stream {} {}'s data doesn't read: {why}", id.0, id.1)]
    Data {
        /// The stream.
        id: ObjectId,
        /// Why, the reading's error's words.
        why: String,
    },
    /// A catalog that isn't among what's written, nor in the reading
    /// given, under `/EncryptMetadata false`, which leaves the document's
    /// metadata stream in the clear: its `/Metadata` says which that is.
    #[error(
        "object {} {}, the catalog, isn't written or read, so which stream is the metadata /EncryptMetadata false leaves in the clear isn't known",
        .0.0, .0.1
    )]
    Catalog(ObjectId),
    /// A group of object streams' members ([`Groups`]) that names this
    /// number, which isn't one the write packs, or names it twice.
    #[error("a group of object streams names object {0}, which isn't packed, or names it twice")]
    Group(u32),
    /// The write was stopped ([`Options::stop`]).
    #[error("stopped")]
    Stopped,
    /// A source whose bytes don't read: the I/O error's kind, which a
    /// caller can match (to try again where it's `Interrupted`, say), and
    /// its words.
    #[error("its bytes don't read: {why}")]
    Io {
        /// The error's kind.
        kind: std::io::ErrorKind,
        /// Its words.
        why: String,
    },
    /// The file the reading given reads isn't as it was when it was read:
    /// its source says so ([`ByteSource::unchanged`]), before anything's
    /// written; or the bytes object `id` was read from, read again to copy
    /// them, aren't those it was read from ([`Body::Read`]).
    #[error("{}", changed(*.0))]
    Changed(Option<ObjectId>),
    /// An object as read ([`Body::Read`]) given with a record of another
    /// reading than the one given, or of another object: read as another
    /// number or generation ([`Record::id`]), or not the object given, nor,
    /// under a renumbering, with its references where its bytes have them.
    /// A document's objects copy from the reading it was made of by
    /// [`Reading::to_document`], not another of the same bytes, unless that
    /// one took them as its own ([`Reading::adopt`]).
    #[error(
        "object {} {} is given with a record of another reading than the one given, or of another object, or, renumbered, with references that aren't where its bytes have them",
        .0.0, .0.1
    )]
    Elsewhere(ObjectId),
    /// An object given at this number and generation that the renumbering
    /// ([`Options::renumber`]) gives no number to be written at: an object
    /// given is one the caller means written.
    #[error(
        "object {} {} is given, and the renumbering gives it no number to be written at",
        .0.0, .0.1
    )]
    Unnumbered(ObjectId),
    /// An update after a file written under a renumbering, whole or as an
    /// update ([`Base::renumbered`]), given an object as read
    /// ([`Body::Read`]), or a reading: the reading's records and objects
    /// are at the numbers it read, which aren't the file's any more, so one
    /// given again would be written over another object of the file. An
    /// update after it is given objects at the numbers written, built or
    /// edited.
    #[error(
        "the file was written under a renumbering, so an update after it is given objects at its numbers, built or edited, and no reading"
    )]
    Renumbered,
    /// A renumbering ([`Options::renumber`]) that writes another object at
    /// the number of object `id` of the file the encryptor was read from,
    /// an encryption dictionary or what one refers to, by the number it's
    /// given at: at the number it gives `id`, in a whole file, which writes
    /// the encryption dictionary again in the numbers written
    /// ([`Encryptor::renumbered`]); at `id`'s own, in an update, which
    /// doesn't, so the file's `/Encrypt` names them where they are, and
    /// whose renumbering then moves none of them, nor names one none. A
    /// reader reads what's at their numbers in the clear (ISO 32000-2,
    /// 7.6.2), which another object's strings would then be.
    #[error(
        "object {} {} is an encryption dictionary or what one refers to, whose number a reader reads in the clear: the renumbering writes another object there, or moves it in an update, which leaves it where it is",
        .0.0, .0.1
    )]
    Moved(ObjectId),
}

impl Error {
    /// [`Error::Io`], of `e`.
    fn io(e: &std::io::Error) -> Error {
        Error::Io {
            kind: e.kind(),
            why: e.to_string(),
        }
    }
}

/// What [`Error::Changed`] says of object `id`, or of the file.
fn changed(id: Option<ObjectId>) -> String {
    match id {
        None => "the file the reading given reads has changed since it was read".to_string(),
        Some((n, g)) => format!(
            "object {n} {g}'s bytes aren't those it was read from: the file has changed since it was read"
        ),
    }
}

/// What [`Error::Number`] says of number `n`.
fn number(n: u32) -> String {
    match n {
        0 => "object 0 heads the free list, so no object is written there".to_string(),
        n => format!("object {n} is past the {NUMBERS} numbers a PDF may use"),
    }
}

/// pdf-objects' writer's refusal, through an [`Encrypting`]: the value it
/// can't write ([`Error::Serialize`]), or the string or stream's data it
/// can't encrypt ([`Error::Encrypt`]).
impl From<Refused<Unencryptable>> for Error {
    fn from(refused: Refused<Unencryptable>) -> Error {
        match refused {
            Refused::Write(e) => Error::Serialize(e),
            Refused::Encrypt { id, why } => Error::Encrypt { id, why },
        }
    }
}

/// After the header, a comment of bytes past ASCII, so that a tool that
/// guesses from the first lines takes the file for binary (ISO 32000-2,
/// 7.5.2).
const BINARY: &[u8] = b"%\xE2\xE3\xCF\xD3\n";

/// The trailer keys a section writes anew, or doesn't have: its links to
/// other sections, a cross-reference stream's dictionary's own (its
/// dictionary is the trailer), and the encryption and `/ID`, which the
/// writer writes itself.
const SECTION_KEYS: [&[u8]; 15] = [
    b"Prev",
    b"XRefStm",
    b"Type",
    b"Size",
    b"W",
    b"Index",
    b"Length",
    b"Filter",
    b"DecodeParms",
    b"F",
    b"FFilter",
    b"FDecodeParms",
    b"DL",
    b"Encrypt",
    b"ID",
];

/// `trailer`'s first `/ID` element, which an RC4 or AES-128 key is made
/// from: the array's first element, when that's a string, however many
/// elements the array has.
fn first_id(trailer: &Dict) -> Option<Vec<u8>> {
    let id = trailer.get(b"ID")?.as_array()?.first()?;
    match id.value() {
        Value::String(text, _) => text.as_bytes().map(<[u8]>::to_vec),
        _ => None,
    }
}

/// A trailer's `/ID`: `kept`, the first string a file has already, and
/// XXH3-128 of `written`, 16 bytes, big-endian, so that the same bytes get
/// the same `/ID`. A file with no first string gets the hash for both
/// where it's `encrypted`, which needs an `/ID` (ISO 32000-2, 14.4, has a
/// new file's two the same), and none otherwise.
fn file_id(kept: Option<Vec<u8>>, encrypted: bool, written: &[u8]) -> Option<[Vec<u8>; 2]> {
    if kept.is_none() && !encrypted {
        return None;
    }
    let hash = twox_hash::XxHash3_128::oneshot(written)
        .to_be_bytes()
        .to_vec();
    let first = kept.unwrap_or_else(|| hash.clone());
    Some([first, hash])
}

/// `id`, as a trailer's `/ID` value: its two strings, in hex.
fn id_value([first, second]: &[Vec<u8>; 2]) -> Object {
    Object::from(vec![
        Object::hex(first.clone()),
        Object::hex(second.clone()),
    ])
}

/// Whether object number `n` is one a written file may use.
fn usable(n: u32) -> Result<(), Error> {
    match n {
        0 => Err(Error::Number(n)),
        n if n >= NUMBERS => Err(Error::Number(n)),
        _ => Ok(()),
    }
}

/// Whether the write is to stop ([`Options::stop`]).
fn stopped(stop: Option<&AtomicBool>) -> Result<(), Error> {
    match stop.is_some_and(|stop| stop.load(Ordering::Relaxed)) {
        true => Err(Error::Stopped),
        false => Ok(()),
    }
}

/// The renumbering a write is given ([`Options::renumber`]), or none: where
/// each object it's given is written, and what each reference names.
#[derive(Clone, Copy)]
struct Numbering<'a>(Option<&'a (dyn Renumber + Sync + 'a)>);

impl<'a> Numbering<'a> {
    /// None: each object at the number it's given at.
    const NONE: Numbering<'static> = Numbering(None);

    /// Where an object given at `id` is written, and what a reference to
    /// `id` names: `id` itself with no renumbering; none for an object the
    /// file doesn't hold.
    #[inline]
    fn at(self, id: ObjectId) -> Option<ObjectId> {
        match self.0 {
            None => Some(id),
            Some(to) => to.renumber(id),
        }
    }
}

/// Where each of `bodies` is written, by `to`, in order: refused where `to`
/// gives one none ([`Error::Unnumbered`]); each a number a written file may
/// use, given once and written once, not `encrypt_at`, the encryption
/// dictionary's, and not one `encryptor`'s file lists free under keys made
/// per object: the first that isn't, refused. `held` are the numbers of
/// objects an update under a renumbering passes over, ascending, given and
/// in its file at those numbers already, which no body is given or written
/// at again.
fn checked(
    bodies: &[(ObjectId, Body<'_>)],
    to: Numbering<'_>,
    encrypt_at: Option<u32>,
    encryptor: Option<&Encryptor>,
    held: &[u32],
) -> Result<Vec<ObjectId>, Error> {
    let held = |n: u32| held.binary_search(&n).is_ok();
    let mut written = Vec::with_capacity(bodies.len());
    let mut taken = HashSet::with_capacity(bodies.len());
    // Under a renumbering, the numbers given too, each once.
    let mut given = HashSet::with_capacity(if to.0.is_some() { bodies.len() } else { 0 });
    for &(id, _) in bodies {
        if (to.0.is_some() && !given.insert(id.0)) || held(id.0) {
            return Err(Error::Twice(id.0));
        }
        let at = to.at(id).ok_or(Error::Unnumbered(id))?;
        usable(at.0)?;
        if !taken.insert(at.0) || held(at.0) {
            return Err(match to.0 {
                None => Error::Twice(at.0),
                Some(_) => Error::WrittenTwice(at.0),
            });
        }
        if encrypt_at == Some(at.0) {
            return Err(Error::EncryptAt(at.0));
        }
        if encryptor.is_some_and(|e| e.free(at.0).is_some()) {
            return Err(Error::Free(at));
        }
        written.push(at);
    }
    Ok(written)
}

/// The numbers `bodies` and `trailer` refer to, as `to` names them, that no
/// body is written at, from `from` on and among `also`: a number the writer
/// gives an object of its own (an object stream, a cross-reference stream,
/// a new encryption dictionary), which is one of those, passes over them,
/// so that a reference to an object that isn't there, which a reader reads
/// as null (ISO 32000-2, 7.3.10), doesn't come to name one. Walked with one
/// stack for every object, as a value built in memory may nest however
/// deeply.
fn dangling(
    bodies: &[(ObjectId, Body<'_>)],
    to: Numbering<'_>,
    trailer: &Dict,
    from: u32,
    also: &[u32],
) -> HashSet<u32> {
    let also: HashSet<u32> = also.iter().copied().collect();
    let mut named = HashSet::new();
    // Each value seen once: a reference noted, as it's written, an array or
    // a dictionary put on the stack, the rest passed over.
    let mut see = |value: &Object| match value.value() {
        Value::Ref(id) => {
            match to.at(*id) {
                Some((n, _)) if n >= from || also.contains(&n) => {
                    named.insert(n);
                }
                _ => {}
            }
            None
        }
        Value::Array(_) | Value::Dict(_) | Value::Stream(_) => Some(()),
        _ => None,
    };
    let mut stack: Vec<&Object> = Vec::new();
    let objects = bodies.iter().map(|(_, body)| body.object());
    for object in objects.chain(trailer.entries().iter().map(|entry| &entry.value)) {
        if see(object).is_some() {
            stack.push(object);
        }
        while let Some(object) = stack.pop() {
            let entries = match object.value() {
                Value::Array(array) => {
                    for value in array {
                        if see(value).is_some() {
                            stack.push(value);
                        }
                    }
                    continue;
                }
                Value::Dict(dict) => dict.entries(),
                Value::Stream(stream) => stream.dict.entries(),
                _ => continue,
            };
            for entry in entries {
                if see(&entry.value).is_some() {
                    stack.push(&entry.value);
                }
            }
        }
    }
    if !named.is_empty() {
        for at in bodies.iter().filter_map(|(id, _)| to.at(*id)) {
            named.remove(&at.0);
        }
    }
    named
}

/// The first number from `from` on that isn't `passed`.
fn fresh(from: u32, passed: impl Fn(u32) -> bool) -> u32 {
    (from..=u32::MAX).find(|&n| !passed(n)).unwrap_or(u32::MAX)
}

/// What resolves a reference in what a write writes: the objects it's
/// given, then the reading they were read in.
struct Bodies<'b> {
    bodies: &'b [(ObjectId, Body<'b>)],
    index: HashMap<ObjectId, usize>,
    from: Option<&'b dyn ReadFrom>,
}

impl<'b> Bodies<'b> {
    fn new(bodies: &'b [(ObjectId, Body<'b>)], from: Option<&'b dyn ReadFrom>) -> Bodies<'b> {
        let index = (bodies.iter().enumerate()).map(|(i, (id, _))| (*id, i));
        Bodies {
            bodies,
            index: index.collect(),
            from,
        }
    }
}

impl<'b> Resolve<'b> for Bodies<'b> {
    fn resolve(&self, id: ObjectId) -> Option<&'b Object> {
        match self.index.get(&id) {
            Some(&i) => Some(self.bodies[i].1.object()),
            None => self.from?.object(id),
        }
    }
}

/// pdf-objects' writer's refusal, through no encryptor: the value it can't
/// write.
fn plain(refused: Refused<std::convert::Infallible>) -> Error {
    match refused {
        Refused::Write(e) => Error::Serialize(e),
        Refused::Encrypt { why, .. } => match why {},
    }
}

/// Object `id`, `obj`, written on its own at the end of `out` at `at`, its
/// references as `to` names them, encrypted by `encrypting` when there's
/// one, under `at`: where it starts. A stream whose data is where it was
/// written takes it from `from`, as the reading hands it out, by `id`
/// ([`Error::Placed`] with none).
fn indirect(
    out: &mut Vec<u8>,
    id: ObjectId,
    at: ObjectId,
    obj: &Object,
    encrypting: Option<&Encrypting<'_>>,
    from: Option<&dyn ReadFrom>,
    to: Numbering<'_>,
) -> Result<usize, Error> {
    if let Value::Stream(stream) = obj.value() {
        if let Some(span) = stream.data.at() {
            let data = from.ok_or(Error::Placed(id))?.data(id, span)?;
            let dict = &stream.dict;
            return match (encrypting, to.0) {
                (Some(e), None) => Ok(serialize::indirect_stream_with(out, at, dict, &data, e)?),
                (None, None) => {
                    serialize::indirect_stream(out, at, dict, &data).map_err(Error::Serialize)
                }
                (Some(e), Some(to)) => Ok(serialize::indirect_stream_renumbered(
                    out, at, dict, &data, e, to,
                )?),
                (None, Some(to)) => {
                    serialize::indirect_stream_renumbered(out, at, dict, &data, &(), to)
                        .map_err(plain)
                }
            };
        }
    }
    match (encrypting, to.0) {
        (Some(e), None) => Ok(serialize::indirect_with(out, at, obj, e)?),
        (None, None) => serialize::indirect(out, at, obj).map_err(Error::Serialize),
        (Some(e), Some(to)) => Ok(serialize::indirect_renumbered(out, at, obj, e, to)?),
        (None, Some(to)) => serialize::indirect_renumbered(out, at, obj, &(), to).map_err(plain),
    }
}
