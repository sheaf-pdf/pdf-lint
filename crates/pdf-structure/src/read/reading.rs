//! A reading of a file: opened, then each object read when it's asked for,
//! on any thread; or read whole, into a [`Document`].

use std::borrow::Cow;
use std::cell::Cell;
use std::fmt;
use std::io;
use std::iter::FusedIterator;
use std::mem::discriminant;
use std::ops::Range;
use std::slice;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use pdf_crypt::{Encoded, Handler, Refused};
use pdf_objects::fault::Place;
use pdf_objects::object::{Object, ObjectId, Span, Stream, Value};
use pdf_objects::version::Version;
use pdf_objects::{Resolve, Streams, Unreadable, Why};

use super::window::{ByPosition, Bytes, InMemory};
use super::{Crypt, Decode, Fault, Limits, Options, Reader, Stored, Unfiltered, Whole, XrefEntry};
use crate::crypt::{List, is_xref};
use crate::filter;
use crate::source::{
    Document, Encryption, Facts, Record, Revision, Source, Trailer, Unread, Update,
};
use crate::write::as_written;

/// A file's bytes, which a [`Reading`] reads. Bytes in memory give all of
/// them ([`ByteSource::whole`]): the slices, vectors and shared slices of
/// bytes do. A reference to a source, a box of one and one shared
/// ([`Arc`]) read as the source does, so that a source can be shared with
/// a reading, or chosen as the program runs (`Arc<dyn ByteSource>`). A
/// source that doesn't hold them in memory is read by position, a window
/// at a time from where a read starts, at first
/// [`ByteSource::first_window`] bytes, twice as many each time what's read
/// runs to the window's end, where more of the file could change it:
/// [`FileBytes`](super::FileBytes) reads a file so, and says whether the
/// file changed since ([`ByteSource::unchanged`]).
pub trait ByteSource: Send + Sync {
    /// How many bytes it has.
    fn len(&self) -> usize;

    /// Whether it has none.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every byte, when it holds them in memory: then nothing is read by
    /// [`ByteSource::read_at`]. None, by default, for a source that reads
    /// them from elsewhere.
    fn whole(&self) -> Option<&[u8]> {
        None
    }

    /// The bytes in `range`, within `0..len()`: all of them, or an error.
    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>>;

    /// How many bytes a read at an offset reads first, when the source
    /// doesn't hold its bytes in memory: 4096, by default. It changes only
    /// what a read costs, never what it reads.
    fn first_window(&self) -> usize {
        4096
    }

    /// Whether its bytes are still those it had when it was made, as far as
    /// it can tell without reading them: what the writer asks before it
    /// reads anything of a reading's to write ([`write`](crate::write)),
    /// refusing to write where they aren't. True, by default: bytes in
    /// memory are the caller's, which change only as the caller changes
    /// them. [`FileBytes`](super::FileBytes) asks its file
    /// ([`FileBytes::unchanged`](super::FileBytes::unchanged)). Whatever it
    /// says, an object the writer copies as the bytes it was read from is
    /// checked against a hash of them taken as it was read.
    fn unchanged(&self) -> io::Result<bool> {
        Ok(true)
    }
}

/// `bytes` in `range`, or an error when it runs past them.
fn within(bytes: &[u8], range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
    let (start, end, len) = (range.start, range.end, bytes.len());
    bytes
        .get(range)
        .map(Cow::Borrowed)
        .ok_or_else(|| past(start, end, len))
}

/// What a read of bytes `start..end` of `len` is, which runs past them.
fn past(start: usize, end: usize, len: usize) -> io::Error {
    let message = format!("bytes {start}..{end} aren't within the {len} there are");
    io::Error::new(io::ErrorKind::UnexpectedEof, message)
}

impl ByteSource for [u8] {
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn whole(&self) -> Option<&[u8]> {
        Some(self)
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        within(self, range)
    }
}

impl ByteSource for Vec<u8> {
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn whole(&self) -> Option<&[u8]> {
        Some(self)
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        within(self, range)
    }
}

/// A source behind a pointer, `$t<S>`, reads as `S` does.
macro_rules! forward {
    ($($t:ty),*) => {$(
        impl<S: ByteSource + ?Sized> ByteSource for $t {
            fn len(&self) -> usize {
                (**self).len()
            }

            fn whole(&self) -> Option<&[u8]> {
                (**self).whole()
            }

            fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
                (**self).read_at(range)
            }

            fn first_window(&self) -> usize {
                (**self).first_window()
            }

            fn unchanged(&self) -> io::Result<bool> {
                (**self).unchanged()
            }
        }
    )*};
}

forward!(&S, Box<S>, Arc<S>);

/// A source's first `len` bytes, as a source of their own: at a revision's
/// end ([`Revision::end`]), the file as that revision was written, which
/// [`Reading::prefix`] reads. They're read as the source reads them, in
/// memory where it holds them, else by position, its first window and
/// whether it's unchanged the source's; a read past them is an error, as
/// one past a source's own bytes is.
#[derive(Debug, Clone)]
pub struct Prefix<S> {
    source: S,
    len: usize,
}

impl<S: ByteSource> Prefix<S> {
    /// `source`'s first `len` bytes: all of them, where it has no more.
    pub fn new(source: S, len: usize) -> Prefix<S> {
        let len = len.min(source.len());
        Prefix { source, len }
    }

    /// The source they're the first bytes of.
    pub fn source(&self) -> &S {
        &self.source
    }
}

impl<S: ByteSource> ByteSource for Prefix<S> {
    fn len(&self) -> usize {
        self.len
    }

    fn whole(&self) -> Option<&[u8]> {
        self.source.whole()?.get(..self.len)
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        match range.end <= self.len {
            true => self.source.read_at(range),
            false => Err(past(range.start, range.end, self.len)),
        }
    }

    fn first_window(&self) -> usize {
        self.source.first_window()
    }

    fn unchanged(&self) -> io::Result<bool> {
        self.source.unchanged()
    }
}

/// Which reading a [`Record`] is of ([`Record::source`]): each reading has
/// its own, unique in the process, which each record it makes holds, so
/// that the places a record gives are known for the file they're in. Two
/// readings of one file have two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceId(u64);

impl SourceId {
    /// One no reading has had.
    pub(super) fn next() -> SourceId {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        SourceId(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// Why a [`Reading`]'s call gives nothing.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The object doesn't read, and why, which the reading keeps with it.
    #[error("{0}")]
    Unread(Unread),
    /// The source didn't give the bytes asked of it: nothing of what the
    /// call was reading is kept, so it reads again when it's asked again.
    /// An object it read whole on the way is kept, as [`Reading::get`]
    /// keeps it: the object stream an object is in, and the objects its
    /// filters refer to; and so is an object stream it decoded whole on the
    /// way to a document ([`Reading::to_document`]).
    #[error("the file doesn't read: {0}")]
    Io(io::Error),
    /// The file is encrypted, and its bytes aren't handed out as written,
    /// where they could be ciphertext: at a span in the file
    /// ([`Reading::bytes`]), or a stream's data that the reader doesn't
    /// decrypt ([`Reading::data`]): a stream among the objects the
    /// encryption dictionary refers to, whose strings it doesn't decrypt
    /// either, or an embedded file's without a `/Crypt` filter of its own
    /// where the encryption dictionary's `/EFF` isn't a name, which doesn't
    /// say how it's encrypted.
    #[error("the file is encrypted, and its bytes aren't handed out as written")]
    Encrypted,
    /// No bytes the reading has are at `span`: it runs past the file, or
    /// past the decoded data of the object stream it names, or names one
    /// that no object the reading lists is in.
    #[error("{}", outside(.span))]
    Outside {
        /// Where the bytes were asked for.
        span: Span,
    },
    /// Object `id`, whose data was asked for, isn't a stream.
    #[error("{id} R isn't a stream")]
    NotAStream {
        /// The object.
        id: ObjectId,
    },
    /// The file is encrypted, and no password the reader was given opens
    /// it, so a stream's data, which may be ciphertext, isn't handed out:
    /// its password is needed. In a file whose streams aren't encrypted
    /// ([`Encryption::Plain`]), only a stream's whose own `/Crypt` filter
    /// encrypts it, or an embedded file's without one of its own under an
    /// `/EFF` that isn't `/Identity`.
    #[error("the file is encrypted, and no password the reader was given opens it")]
    Locked,
    /// The file is encrypted by a security handler the reader can't open
    /// ([`Encryption::Unsupported`]), so a stream's data, which may be
    /// ciphertext, isn't handed out.
    #[error("the file is encrypted by a security handler the reader can't open")]
    Unsupported,
    /// The stream's `/Filter` or `/DecodeParms` are ones the reader doesn't
    /// take.
    #[error("{0}")]
    Filters(Unfiltered),
    /// The stream's data doesn't decrypt cleanly: pdf-crypt's error, which
    /// holds what decrypted, if anything did.
    #[error("its data doesn't decrypt cleanly: {0}")]
    Decrypt(pdf_crypt::Error),
    /// The stream's data doesn't decode cleanly through its filters:
    /// pdf-filters' error, which holds what decoded, however far that went,
    /// and each fault. A stream that decodes past
    /// [`Limits::stream`](super::Limits::stream) is cut off there, with
    /// [`Fault::Limit`](pdf_filters::Fault::Limit) among them; and so is
    /// a stream with no filters past the limit it's read to
    /// ([`Reading::decoded_within`]), its fault the first filter's, the one
    /// fault of pdf-filters' the reader makes itself, so that past a limit
    /// is one answer, filtered or not. Its `Debug` gives the data's length,
    /// not its bytes, as pdf-filters' does.
    #[error("its data doesn't decode cleanly: {0}")]
    Decode(pdf_filters::Error),
    /// A document isn't taken as the reading's ([`Reading::adopt`]), and
    /// is left as it was: its object `id` isn't read here as it was read
    /// (another file's, or the file changed since, or a revision of it with
    /// another object there), or doesn't read here, or reads here where the
    /// document holds it unread.
    #[error("{id} R isn't read here as the document read it")]
    Unlike {
        /// The object.
        id: ObjectId,
    },
    /// A document isn't taken as the reading's ([`Reading::adopt`]), and
    /// is left as it was: it isn't encrypted as the reading's file is, or
    /// wasn't opened as the reading opened it ([`Encryption`]: one is
    /// encrypted and the other isn't, or a password opened one and not the
    /// other), or its encryption dictionary, as stored and where its
    /// trailer names it, or its first `/ID` element, which a key is made
    /// of, isn't the file's.
    #[error("the document isn't encrypted, or opened, as this reading's file is")]
    UnlikeEncryption,
}

/// A stream, and its data as [`Reading::data`] hands it out; or why it
/// doesn't.
type Handed<'f> = Result<(&'f Stream, Cow<'f, [u8]>), Error>;

/// [`Error::Outside`]'s text.
fn outside(span: &Span) -> String {
    let (start, end) = (span.start, span.end);
    match span.stream {
        None => format!("bytes {start}..{end} aren't in the file"),
        Some(stream) => {
            format!("bytes {start}..{end} aren't in object stream {stream} R's decoded data")
        }
    }
}

/// A file being read: opened, then each object read when it's first asked
/// for, kept or not, on any thread; or read whole, into a [`Document`], as
/// [`document`](super::document) reads it, the reading taken
/// ([`Reading::into_document`]) or kept beside it
/// ([`Reading::to_document`]).
///
/// [`Reading::open`] reads the header, the cross-reference sections and
/// the trailer (or, where they don't read, finds the objects by scanning
/// the file), and opens an encrypted file's security handler by the
/// passwords its [`Options`] give, or the empty password; no object yet.
/// A reading no password opened is opened by another with
/// [`Reading::unlock`]. [`Reading::get`] reads an object
/// the first time it's asked for and keeps it, with what the reader told
/// of as it read it, decrypting its strings in an encrypted file it opened;
/// [`Reading::object_unkept`] reads one without keeping it. An object in an
/// object stream is read from the stream's decoded data, the stream decoded
/// and kept the first time an object in it is read, once, whichever thread
/// asks. [`Reading::data`] gives a stream's data, decrypted in an encrypted
/// file it opened, and [`Reading::decoded_within`] that through its
/// filters, each read when it's asked for, and not kept.
///
/// Read in any order, on any threads, then made a [`Document`], a reading
/// is the whole read, the same objects, faults in the same order and the
/// same decoded data, so long as no stream it decodes reaches its
/// [`budget`](super::Limits::budget): that's spent in the order streams
/// are decoded, so which stream it cuts depends on the order objects are
/// asked for, where the whole read decodes them by number.
///
/// A source that holds its bytes in memory ([`ByteSource::whole`]) is read
/// as the whole read reads a file; one that doesn't is read by position, a
/// window at a time, each read reading what it would of the whole file. An
/// object's first window reaches a byte past the next offset the
/// cross-references list, so that an object as it should be is one read.
/// The header is read from the file's start, `startxref` and the last
/// `%%EOF` back from its end, and each section at its offset, with the
/// `startxref` and `%%EOF` after it, where its revision ends; a file whose
/// sections don't read is read whole, to be scanned. Where the source
/// fails ([`Error::Io`]), the call keeps nothing of what it was reading,
/// but an object it read whole on the way, and asked again it reads again.
///
/// A reading is a [`Source`], as a document is, answering of what it has
/// read so far, and pdf-objects' [`Streams`]. Its own methods of the
/// traits' names say more than the traits' shapes do, and a call on a
/// `Reading` names them first, each giving the same answer in a plainer or
/// a richer form: [`Reading::ids`] is an iterator, [`Reading::get`] and
/// [`Reading::decoded_within`] give an [`Error`], [`Reading::faults`] a
/// `Vec`, [`Reading::metadata`] a `Result`, and [`Reading::size`] and
/// [`Reading::facts`] no `Option`. The traits' answers are reached through
/// a `&dyn Source`, or by the trait's name: `Source::get(&reading, id)`,
/// and a viewer's account of a decode, `Streams::decoded(&reading, id,
/// limit)`, which no method of a reading's own shadows.
///
/// ```
/// use pdf_structure::read::{Options, Reading};
/// use pdf_structure::pdf_objects::ObjectId;
///
/// let file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n\
///     xref\n0 2\n0000000000 65535 f \n0000000009 00000 n \n\
///     trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n45\n%%EOF\n";
/// let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
/// assert_eq!(reading.ids().collect::<Vec<_>>(), [ObjectId::new(1, 0)]);
/// let catalog = reading.get(ObjectId::new(1, 0)).unwrap();
/// assert!(catalog.as_dict().is_some_and(|d| d.has(b"Type")));
/// assert!(reading.faults().is_empty());
/// ```
pub struct Reading<S> {
    source: S,
    pub(super) reader: Reader,
    /// Whether an object read by position is first given a window to a
    /// byte past the next listed offset: always, but where the tests and
    /// the fuzz target read without, so that a small first window reaches
    /// every read's windows.
    bound: bool,
    /// How many times a resolver's read stopped at the source
    /// ([`Reading::failed_resolves`]).
    failed_resolves: AtomicU64,
}

impl<S> fmt::Debug for Reading<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reading")
            .field("id", &self.reader.id)
            .field("trailer", &self.reader.trailer)
            .field("encryption", &self.reader.encryption)
            .finish_non_exhaustive()
    }
}

/// Every object a [`Reading`]'s cross-references list, by number: what
/// [`Reading::ids`] gives.
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Debug, Clone)]
pub struct Ids<'r> {
    entries: slice::Iter<'r, (u32, (u16, XrefEntry))>,
}

/// The object an entry lists: one in use, of its generation, and one in an
/// object stream, of generation 0; none for a free one, nor for number 0.
fn listed(&(n, (generation, entry)): &(u32, (u16, XrefEntry))) -> Option<ObjectId> {
    match entry {
        _ if n == 0 => None,
        XrefEntry::InUse { .. } => Some(ObjectId::new(n, generation)),
        XrefEntry::Compressed { .. } => Some(ObjectId::new(n, 0)),
        _ => None,
    }
}

impl Iterator for Ids<'_> {
    type Item = ObjectId;

    #[inline]
    fn next(&mut self) -> Option<ObjectId> {
        self.entries.find_map(listed)
    }

    /// At most one an entry left.
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, self.entries.size_hint().1)
    }
}

impl FusedIterator for Ids<'_> {}

/// The newest entry a [`Reading`]'s cross-reference sections give each
/// object number, by number: what [`Reading::entries`] gives.
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Debug, Clone)]
pub struct Entries<'r> {
    entries: slice::Iter<'r, (u32, (u16, XrefEntry))>,
}

impl Iterator for Entries<'_> {
    type Item = (u32, u16, XrefEntry);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        (self.entries.next()).map(|&(n, (generation, entry))| (n, generation, entry))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.entries.size_hint()
    }
}

impl FusedIterator for Entries<'_> {}

impl<'a> Reading<&'a [u8]> {
    /// The file `file` is, opened as [`Reading::open`] opens it, which
    /// can't fail in memory; `native` but in the tests that make it ask
    /// `decoder` for every stream.
    pub(crate) fn in_memory(
        file: &'a [u8],
        options: &Options<'_>,
        native: bool,
        decoder: Option<&dyn Decode>,
    ) -> Reading<&'a [u8]> {
        let Ok(reader) = Reader::new(InMemory(file), options, native, decoder);
        Reading {
            reader,
            source: file,
            bound: true,
            failed_resolves: AtomicU64::new(0),
        }
    }

    /// The reading read whole, with `decoder` asked for what the reader
    /// doesn't decode or decrypt itself, which can't fail in memory.
    pub(crate) fn whole(self, decoder: Option<&dyn Decode>) -> Whole {
        let Ok(whole) = self.reader.whole(InMemory(self.source), decoder);
        whole
    }
}

impl<S: ByteSource> Reading<S> {
    /// The file `source` holds, opened under `options`: its header, its
    /// cross-reference sections and trailer, and, when it's encrypted, its
    /// security handler, opened by the passwords they give
    /// ([`Options::passwords`]), or the empty password, as most files' is,
    /// given none. One no password given opens is left locked, which
    /// [`Reading::unlock`] opens by the bytes a prompt gives once it's
    /// open, without reading the file again. The passwords aren't kept. It
    /// fails only when the source doesn't give its bytes ([`Error::Io`]).
    ///
    /// The one way in: [`document`](super::document) is a reading of a
    /// file in memory, opened so and read whole, and
    /// [`WithReading::new`](super::WithReading::new) one read whole beside
    /// its reading.
    pub fn open(source: S, options: &Options<'_>) -> Result<Reading<S>, Error> {
        Reading::open_by(source, options, true)
    }

    /// [`Reading::open`], an object read by position first given a
    /// window to a byte past the next listed offset when `bound`.
    pub(crate) fn open_by(
        source: S,
        options: &Options<'_>,
        bound: bool,
    ) -> Result<Reading<S>, Error> {
        let reader = match source.whole() {
            Some(file) => {
                let Ok(reader) = Reader::new(InMemory(file), options, true, None);
                reader
            }
            None => {
                let file = ByPosition::new(&source, bound);
                Reader::new(file, options, true, None).map_err(Error::Io)?
            }
        };
        Ok(Reading {
            source,
            reader,
            bound,
            failed_resolves: AtomicU64::new(0),
        })
    }

    /// The reading, an encrypted file no password opened, opened by
    /// `passwords`, each a password's bytes in an encoding, as a prompt
    /// gives them (pdf-crypt's `Encoded`), as [`Reading::open`] would have
    /// opened it by passwords as typed ([`Options::passwords`]): those
    /// that fit the file's revision first, in the order given, then the
    /// rest, each as the owner's and then the user's (see pdf-crypt's
    /// `Handler::open`), and [`Encryption::Opened`]'s `by` the index among
    /// `passwords` of the one that opened it. Nothing of the file is read again: the
    /// reading kept its encryption dictionary's values and its first `/ID`
    /// element while no password had opened it. What it read and decoded
    /// is dropped, to be read again decrypted, and what it told of is what
    /// it told of opening the file, with what's wrong with `/Perms` among
    /// it, where faults are looked for; its budget is what opening the file
    /// left. [`Encryption::Opened`] says, as an open by the password does,
    /// what the file grants.
    ///
    /// Given back as it was, with why: where none of `passwords` opens it
    /// ([`Refused::Password`]), or its security handler is one the reader
    /// can't open: another handler ([`Refused::OtherHandler`]), a `/V` and
    /// `/R` the standard handler isn't ([`Refused::Version`]), or a
    /// dictionary it can't take, or an `/Encrypt` that isn't one
    /// ([`Refused::Malformed`]), each in the words
    /// [`Encryption::Unsupported`] says where its strings or streams are
    /// encrypted. One whose strings and streams aren't
    /// ([`Encryption::Plain`]: `/StmF` and `/StrF` `/Identity`, or absent,
    /// from `/V` 4) is refused alike, by the same variant, and stays plain.
    /// A file that isn't encrypted, or that a password opened, is given
    /// back as it is.
    // The refusal holds the reading, as the reading opened is one: a box
    // would make neither side smaller.
    #[allow(clippy::result_large_err)]
    pub fn unlock(mut self, passwords: &[Encoded]) -> Result<Reading<S>, (Reading<S>, Refused)> {
        match self.reader.unlock(passwords) {
            Ok(()) => Ok(self),
            Err(refused) => Err((self, refused)),
        }
    }

    /// The file read by position.
    pub(super) fn by_position(&self) -> ByPosition<'_> {
        ByPosition::new(&self.source, self.bound)
    }

    /// Every object its cross-references list, by number: each in use, of
    /// its generation, and each in an object stream, of generation 0.
    pub fn ids(&self) -> Ids<'_> {
        Ids {
            entries: self.reader.entries.iter(),
        }
    }

    /// The newest entry its cross-reference sections give each object
    /// number (or, when the file was scanned, the scan), by number: the
    /// number, its generation and what the entry says.
    pub fn entries(&self) -> Entries<'_> {
        Entries {
            entries: self.reader.entries.iter(),
        }
    }

    /// The newest entry its sections give number `n`, as
    /// [`Reading::entries`] gives it, for the writer, which copies an
    /// object in an object stream only as this entry has it.
    pub(crate) fn entry(&self, n: u32) -> Option<(u16, XrefEntry)> {
        self.reader.entry(n).copied()
    }

    /// Object `id`, as written: read the first time it's asked for, and
    /// kept, as a failure to read it is ([`Error::Unread`]), and so is the
    /// object stream it's in, decoded. Its references are kept as
    /// references, and its strings decrypted in an encrypted file that
    /// opened.
    pub fn get(&self, id: ObjectId) -> Result<&Object, Error> {
        let got = match self.source.whole() {
            Some(file) => {
                let Ok(got) = self.reader.get(InMemory(file), id);
                got
            }
            None => self.reader.get(self.by_position(), id).map_err(Error::Io)?,
        };
        got.map_err(Error::Unread)
    }

    /// Object `id`, as [`Reading::get`] gives it, and where it was read
    /// from: read without keeping it, its faults or a failure to read it,
    /// when it isn't kept already, so that a pass over many objects, each
    /// once, doesn't hold them all. The object stream it's in is decoded
    /// and kept, as it is for [`Reading::get`], with the objects the
    /// stream and its filters are, which are read and kept.
    pub fn object_unkept(&self, id: ObjectId) -> Result<(Object, Record), Error> {
        let Some(i) = self.reader.listed(id) else {
            return Err(Error::Unread(Unread::Missing { id }));
        };
        let object = match (self.reader.slots[i].get(), self.source.whole()) {
            (Some(read), _) => read.object.clone(),
            (None, Some(file)) => {
                let Ok(read) = self.reader.read(InMemory(file), i);
                read.object
            }
            (None, None) => {
                (self.reader.read(self.by_position(), i))
                    .map_err(Error::Io)?
                    .object
            }
        };
        object.map_err(Error::Unread)
    }

    /// Where object `id` was read from, once a read of it is kept, and it
    /// read; none before.
    pub fn record(&self, id: ObjectId) -> Option<&Record> {
        let i = self.reader.listed(id)?;
        let read = self.reader.slots[i].get()?;
        read.object.as_ref().ok().map(|(_, record)| record)
    }

    /// How many times a resolve has found that the source didn't give the
    /// bytes asked of it: through [`Resolve::resolve`], which gives none, as
    /// a reference to an object the file hasn't, and through
    /// [`Resolve::try_resolve`], which says so ([`Why::Source`]). What follows
    /// references through `resolve` reads such an object as null: the count
    /// is how a caller learns that something may have read so, where nothing
    /// after it would say, and it may ask again. A read by [`Reading::get`]
    /// says so to its caller, and isn't counted. A stream's data as a viewer
    /// takes it ([`Streams::decoded`]) reads the stream's object as
    /// `try_resolve` does, counted so, and its data as [`Reading::data`]
    /// does, not counted, both said to its caller ([`Why::Source`]). In
    /// memory, always 0.
    pub fn failed_resolves(&self) -> u64 {
        self.failed_resolves.load(Ordering::Relaxed)
    }

    /// The trailer.
    pub fn trailer(&self) -> &Trailer {
        &self.reader.trailer
    }

    /// The version in the header.
    pub fn header_version(&self) -> Option<Version> {
        self.reader.version
    }

    /// Whether the file is encrypted, and if it is, whether it opened.
    pub fn encryption(&self) -> &Encryption {
        &self.reader.encryption
    }

    /// The security handler of an encrypted file a password opened, for
    /// the writer, which encrypts by it as the reading decrypts by it.
    pub(crate) fn handler(&self) -> Option<&Handler> {
        self.reader.handler.as_ref()
    }

    /// What the file leaves in the clear, which the reading decrypts by,
    /// for the writer, which encrypts by it.
    pub(crate) fn list(&self) -> &List {
        &self.reader.list
    }

    /// An encrypted file's encryption dictionary as the file stores it,
    /// where, and the first `/ID` element its handler opens by; none where
    /// it isn't encrypted, or its `/Encrypt` isn't a dictionary.
    pub(crate) fn stored(&self) -> Option<&Stored> {
        self.reader.stored.as_deref()
    }

    /// The section it was opened as of ([`Options::section`]), when it
    /// was, for the writer, which goes after the file's last.
    pub(crate) fn as_of(&self) -> Option<usize> {
        self.reader.as_of
    }

    /// The file's newest incremental update, when its cross-reference
    /// sections were read and the newest follows one written before it.
    pub fn update(&self) -> Option<&Update> {
        self.reader.update.as_ref()
    }

    /// What the file's header and cross-reference sections say of it, as
    /// written.
    pub fn facts(&self) -> &Facts {
        &self.reader.facts
    }

    /// The file's revisions, the first first, as [`Document::revisions`]
    /// gives them: none where its sections didn't read, nor where its later
    /// revisions list more numbers in all than it reads entries
    /// ([`Limits::entries`](crate::Limits::entries)).
    pub fn revisions(&self) -> &[Revision] {
        &self.reader.revisions
    }

    /// The file's length in bytes.
    pub fn size(&self) -> usize {
        self.source.len()
    }

    /// The caps it reads under, as it was opened with them
    /// ([`Options::limits`]).
    pub fn limits(&self) -> &Limits {
        &self.reader.limits
    }

    /// The file's first `len` bytes, read as a file of their own, from
    /// their last `startxref`, under `limits` (this reading's,
    /// [`Reading::limits`], or less) and this reading's Flate backend: at a
    /// revision's end ([`Revision::end`]), the file as that revision was
    /// written, opened lazily as any reading is, with a budget of its own.
    /// Read by position as this reading is, or in memory. Encrypted by an
    /// encryption dictionary whose values and first `/ID` element are those
    /// this reading opened its file by, where the empty password doesn't
    /// open it, it's opened by this reading's security handler, its
    /// [`Encryption`] this reading's, so that no password is asked for or
    /// kept: the key a password makes is made from those and the password
    /// (ISO 32000-2, 7.6.4.3). Otherwise it's opened as [`Reading::open`]
    /// opens a file, by the empty password, and [`Reading::unlock`] opens it
    /// by another. It fails only where the source doesn't give the bytes
    /// asked of it ([`Error::Io`]).
    ///
    /// ```
    /// use pdf_structure::RevisionEnd;
    /// use pdf_structure::read::{Options, Reading};
    /// use pdf_structure::pdf_objects::ObjectId;
    ///
    /// let first = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n\
    ///     xref\n0 2\n0000000000 65535 f \n0000000009 00000 n \n\
    ///     trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n45\n%%EOF\n";
    /// let update = b"1 0 obj\n<< /Type /Catalog /Lang (en) >>\nendobj\n\
    ///     xref\n1 1\n0000000147 00000 n \n\
    ///     trailer\n<< /Size 2 /Root 1 0 R /Prev 45 >>\nstartxref\n194\n%%EOF\n";
    /// let file = [&first[..], &update[..]].concat();
    /// let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    /// let ends: Vec<_> = reading.revisions().iter().map(|r| r.end).collect();
    /// let end = |eof, line| RevisionEnd::new(eof, line);
    /// assert_eq!(ends, [end(146, 147), end(285, 286)]);
    /// let before = reading.prefix(147, reading.limits()).unwrap();
    /// assert_eq!(before.revisions(), &reading.revisions()[..1]);
    /// let catalog = before.get(ObjectId::new(1, 0)).unwrap().as_dict().unwrap();
    /// assert!(!catalog.has(b"Lang"));
    /// ```
    pub fn prefix(&self, len: usize, limits: &Limits) -> Result<Reading<Prefix<&S>>, Error> {
        // Every option named, so that one more has its say here.
        // Opened by the empty password, its handler's key given it after;
        // its faults looked for where this reading's are.
        let options = Options {
            limits: *limits,
            section: None,
            flate: self.reader.flate,
            passwords: &[],
            faults: self.reader.wants,
        };
        let source = Prefix::new(&self.source, len);
        let mut prefix = Reading::open_by(source, &options, self.bound)?;
        let reader = &self.reader;
        if let (Some(handler), Some(stored)) = (&reader.handler, &reader.stored) {
            let (values, id) = (&stored.values, &stored.id);
            prefix.reader.key(handler, values, id, &reader.encryption);
        }
        Ok(prefix)
    }

    /// What the reader told of in what it has read so far, each fault at
    /// its place, in the order the whole read says it: as it opened the
    /// file; as it read each object in use, by number; as it decoded each
    /// object stream, by number; as it read each object in an object
    /// stream, by number; each of those read that starts inside the bytes
    /// of another read, by stream, then where it starts; and as it
    /// decrypted each object's strings, by number. Each once: what it told
    /// as it opened the file of the encryption dictionary, or an object its
    /// entries refer to, which it reads to open the file, is said where that
    /// object's own read says it too, there, and where it hasn't read the
    /// object, as it opened the file. What was read without keeping it isn't
    /// here; nor is any where faults aren't looked for ([`Options::faults`]).
    pub fn faults(&self) -> Vec<(Place, Fault)> {
        self.reader.faults()
    }

    /// The data of the catalog's metadata stream (ISO 32000-2, 14.3.2), as
    /// written, when it's in plain view: unfiltered, and not encrypted, as
    /// [`Document::metadata`] has it. The objects on the way to it are read
    /// and kept.
    pub fn metadata(&self) -> Result<Option<Cow<'_, [u8]>>, Error> {
        match self.source.whole() {
            Some(file) => {
                let Ok(metadata) = self.metadata_in(InMemory(file));
                Ok(metadata)
            }
            None => self.metadata_in(self.by_position()).map_err(Error::Io),
        }
    }

    /// [`Reading::metadata`], of `file`.
    fn metadata_in<'f, F: Bytes<'f>>(&'f self, file: F) -> Result<Option<Cow<'f, [u8]>>, F::Error> {
        let reader = &self.reader;
        let get = |id| Ok(reader.get(file, id)?.ok());
        super::metadata(file, &reader.trailer, &get, !reader.list.plain_metadata())
    }

    /// The bytes at `span`: in the file, or in the decoded data of the
    /// object stream it names, which is decoded and kept when it isn't yet.
    /// An encrypted file's are refused ([`Error::Encrypted`]), as they may
    /// be ciphertext, but for an object stream's decoded data, which is
    /// decrypted.
    pub fn bytes(&self, span: Span) -> Result<Cow<'_, [u8]>, Error> {
        match self.source.whole() {
            Some(file) => {
                let Ok(bytes) = self.bytes_in(InMemory(file), span);
                bytes
            }
            None => self.bytes_in(self.by_position(), span).map_err(Error::Io)?,
        }
    }

    /// [`Reading::bytes`], of `file`.
    fn bytes_in<'f, F: Bytes<'f>>(
        &'f self,
        file: F,
        span: Span,
    ) -> Result<Result<Cow<'f, [u8]>, Error>, F::Error> {
        match span.stream {
            None if self.reader.list.encrypts => Ok(Err(Error::Encrypted)),
            _ => self.written_in(file, span),
        }
    }

    /// The bytes at `span` as written: in the file, an encrypted file's
    /// too, which [`Reading::bytes`] refuses, as they may be ciphertext; or
    /// in the decoded data of the object stream it names, decrypted, which
    /// is decoded and kept when it isn't yet. For the writer, which copies
    /// an object as the bytes it was read from only where they're right to
    /// copy: under the key they're encrypted by, at their number.
    pub(crate) fn written(&self, span: Span) -> Result<Cow<'_, [u8]>, Error> {
        match self.source.whole() {
            Some(file) => {
                let Ok(bytes) = self.written_in(InMemory(file), span);
                bytes
            }
            None => self
                .written_in(self.by_position(), span)
                .map_err(Error::Io)?,
        }
    }

    /// The bytes at `span` as written: in the file, or in the decoded data
    /// of the object stream it names, which is decoded and kept when it
    /// isn't yet; of `file`.
    fn written_in<'f, F: Bytes<'f>>(
        &'f self,
        file: F,
        span: Span,
    ) -> Result<Result<Cow<'f, [u8]>, Error>, F::Error> {
        let range = span.start..span.end;
        let outside = Error::Outside { span };
        Ok(match span.stream {
            None => file.get(range)?.ok_or(outside),
            Some(ObjectId {
                number: stream,
                generation: 0,
            }) => match self.reader.container(file, stream)? {
                None => Err(outside),
                Some(container) => match &container.header {
                    Err(why) => Err(Error::Unread(why.clone())),
                    Ok(_) => container.data.get(range).map(Cow::Borrowed).ok_or(outside),
                },
            },
            Some(_) => Err(outside),
        })
    }

    /// The data of stream `id`, as written, filters and all; in an
    /// encrypted file, decrypted as the reader decrypts an object stream's:
    /// by the stream's own `/Crypt` filter when it has one, else as the
    /// encryption dictionary's `/StmF` says (ISO 32000-2, 7.6.6); an
    /// embedded file's (`/Type /EmbeddedFile`) without one of its own, by
    /// the crypt filter the encryption dictionary's `/EFF` names, when it
    /// has one (7.6.6 and Table 20). The stream is read and kept, as
    /// [`Reading::get`] keeps it, but not its data, which is read each time
    /// it's asked for, borrowed from a source in memory where nothing's
    /// decrypted.
    ///
    /// Left as written, as the reader leaves them: a cross-reference
    /// stream's, which is never encrypted (7.5.8.2), and the catalog's
    /// metadata stream's under `/EncryptMetadata false`, whether the file
    /// opens or not, as [`Document::metadata`] has it, but where its own
    /// crypt filter encrypts it, which then decrypts it (7.6.6). Refused,
    /// where it may be ciphertext the reader doesn't decrypt: in a file no
    /// password opened ([`Error::Locked`]), but one whose streams aren't
    /// encrypted ([`Encryption::Plain`]), whose data is as written but a
    /// stream's that its own crypt filter encrypts, or an embedded file's
    /// without one of its own under an `/EFF` that isn't `/Identity`; in a
    /// file whose security handler the reader can't open
    /// ([`Error::Unsupported`]); and a stream's that the encryption
    /// dictionary refers to, or an embedded file's without a crypt filter of
    /// its own where `/EFF` isn't a name ([`Error::Encrypted`]). Data that
    /// doesn't decrypt cleanly is an error holding what did
    /// ([`Error::Decrypt`]).
    pub fn data(&self, id: ObjectId) -> Result<Cow<'_, [u8]>, Error> {
        let data = match self.source.whole() {
            Some(file) => {
                let Ok(data) = self.data_in(InMemory(file), id);
                data
            }
            None => self.data_in(self.by_position(), id).map_err(Error::Io)?,
        };
        data.map(|(_, data)| data)
    }

    /// The data of stream `id` decoded: [`Reading::data`] through the
    /// stream's filters, by pdf-filters, its `/Filter` and `/DecodeParms`
    /// read as the reader reads an object stream's (references followed
    /// through what's read and kept, but in a cross-reference stream's,
    /// whose values are direct), up to `limit` bytes or
    /// [`Limits::stream`](super::Limits::stream), whichever is less, so a
    /// caller with a budget of its own has a stream decoded no further than
    /// it could take; `usize::MAX` for the reading's own cap alone. What's
    /// decoded isn't kept, nor counted against the reading's budget, which
    /// is for what the reading keeps. Data that doesn't decode cleanly is
    /// an error holding what did, a stream cut off at the limit among them
    /// ([`Error::Decode`]). A stream without filters is its data, held to
    /// the limit too: past it, its data is cut off there, an error as a
    /// stream decoded past it is ([`Error::Decode`], with
    /// [`Fault::Limit`](pdf_filters::Fault::Limit) at filter 0, the one
    /// fault of pdf-filters' the reader makes itself).
    ///
    /// This is the reader's own account of the decode, every fault of it,
    /// for a checker; what a viewer takes of it is [`Streams::decoded`]'s,
    /// which the reading answers by this.
    ///
    /// ```
    /// use pdf_structure::read::{Error, Options, Reading};
    /// use pdf_structure::pdf_filters::Fault;
    /// use pdf_structure::pdf_objects::ObjectId;
    ///
    /// let file = b"%PDF-1.7\n1 0 obj\n<< /Length 6 >>\nstream\nabcdef\nendstream\nendobj\n\
    ///     trailer\n<< /Size 2 >>\n%%EOF\n";
    /// let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    /// assert_eq!(reading.decoded_within(ObjectId::new(1, 0), 6).unwrap(), b"abcdef");
    /// let Err(Error::Decode(cut)) = reading.decoded_within(ObjectId::new(1, 0), 5) else {
    ///     panic!("past its limit");
    /// };
    /// assert_eq!(cut.data, b"abcde");
    /// assert_eq!(cut.faults, [(0, Fault::Limit)]);
    /// ```
    pub fn decoded_within(&self, id: ObjectId, limit: usize) -> Result<Vec<u8>, Error> {
        self.decoded_within_as(id, None, limit)
    }

    /// Whether a decode asked for within `limit` is held to the reader's own
    /// cap on a stream's data instead
    /// ([`Limits::stream`](super::Limits::stream)), that being the less:
    /// past it is then [`Why::Capped`], not the caller's [`Why::Limit`]. At
    /// equal it's the caller's, the data being past what was asked for
    /// either way.
    pub(super) fn capped_at(&self, limit: usize) -> bool {
        self.reader.limits.stream < limit
    }

    /// [`Reading::decoded_within`], the stream `object` where it's given:
    /// what [`Reading::get`] gives of `id`, as a document read whole from
    /// this reading holds it; where it isn't, the stream as
    /// [`Reading::get`] reads it.
    pub(super) fn decoded_within_as<'f>(
        &'f self,
        id: ObjectId,
        object: Option<Result<&'f Object, &Unread>>,
        limit: usize,
    ) -> Result<Vec<u8>, Error> {
        let limit = limit.min(self.reader.limits.stream);
        match self.decoded_as(id, object, limit) {
            // Only a stream with no filters comes back longer than its limit.
            Ok(mut data) if data.len() > limit => {
                data.truncate(limit);
                data.shrink_to_fit();
                Err(Error::Decode(pdf_filters::Error {
                    data,
                    faults: vec![(0, pdf_filters::Fault::Limit)],
                }))
            }
            decoded => decoded,
        }
    }

    /// The data of stream `id` decoded, each filter's output up to `limit`
    /// bytes and a stream without filters whole, the stream `object` where
    /// it's given.
    fn decoded_as<'f>(
        &'f self,
        id: ObjectId,
        object: Option<Result<&'f Object, &Unread>>,
        limit: usize,
    ) -> Result<Vec<u8>, Error> {
        match self.source.whole() {
            Some(file) => {
                let Ok(decoded) = self.decoded_in(InMemory(file), id, object, limit);
                decoded
            }
            None => (self.decoded_in(self.by_position(), id, object, limit)).map_err(Error::Io)?,
        }
    }

    /// Stream `id`, and its data, as [`Reading::data`] gives it, of `file`.
    fn data_in<'f, F: Bytes<'f>>(&'f self, file: F, id: ObjectId) -> Result<Handed<'f>, F::Error> {
        let object = self.reader.get(file, id)?;
        self.data_of(file, id, object.as_ref().map(|o| *o))
    }

    /// [`Reading::data_in`], the stream `object`, what reading `id` gives.
    fn data_of<'f, F: Bytes<'f>>(
        &'f self,
        file: F,
        id: ObjectId,
        object: Result<&'f Object, &Unread>,
    ) -> Result<Handed<'f>, F::Error> {
        let reader = &self.reader;
        let stream = match object {
            Ok(object) => match object.value() {
                Value::Stream(stream) => stream,
                _ => return Ok(Err(Error::NotAStream { id })),
            },
            Err(why) => return Ok(Err(Error::Unread(why.clone()))),
        };
        let crypt = match reader.crypt(file, id, &stream.dict)? {
            Ok(crypt) => crypt,
            Err(error) => return Ok(Err(error)),
        };
        let written = match stream.data.at() {
            Some(at) => match self.written_in(file, at)? {
                Ok(written) => written,
                Err(error) => return Ok(Err(error)),
            },
            // The bytes it holds, where it isn't placed: never a stream the
            // reader read.
            None => Cow::Borrowed(stream.data.bytes().unwrap_or_default()),
        };
        Ok(match crypt {
            Crypt::Written => Ok((stream, written)),
            Crypt::Decrypted(handler, method) => {
                match handler.decrypt_stream(id.into(), &written, method.as_ref()) {
                    Ok(plain) => Ok((stream, Cow::Owned(plain))),
                    Err(error) => Err(Error::Decrypt(error)),
                }
            }
        })
    }

    /// [`Reading::decoded_as`], of `file`.
    fn decoded_in<'f, F: Bytes<'f>>(
        &'f self,
        file: F,
        id: ObjectId,
        object: Option<Result<&'f Object, &Unread>>,
        limit: usize,
    ) -> Result<Result<Vec<u8>, Error>, F::Error> {
        let handed = match object {
            Some(object) => self.data_of(file, id, object)?,
            None => self.data_in(file, id)?,
        };
        let (stream, data) = match handed {
            Ok(read) => read,
            Err(error) => return Ok(Err(error)),
        };
        let reader = &self.reader;
        let failed = Cell::new(None);
        let resolve = |id| match reader.get(file, id) {
            Ok(object) => object.ok(),
            Err(error) => {
                failed.set(Some(error));
                None
            }
        };
        // A cross-reference stream's values are direct (ISO 32000-2,
        // 7.5.8.2), so its references aren't followed, as the file is read.
        let filters = match is_xref(&stream.dict) {
            true => filter::filters(&stream.dict, None),
            false => filter::filters(&stream.dict, Some(&resolve)),
        };
        if let Some(error) = failed.into_inner() {
            return Err(error);
        }
        Ok(match filters {
            Ok(filters) => {
                pdf_filters::decode(&data, &filters, limit, reader.flate).map_err(Error::Decode)
            }
            Err(why) => Err(Error::Filters(why)),
        })
    }

    /// Which reading this is, which each of its records holds.
    pub fn id(&self) -> SourceId {
        self.reader.id
    }

    /// The source it reads: to ask it, say, whether its file has changed
    /// ([`FileBytes::unchanged`](super::FileBytes::unchanged)).
    pub fn source(&self) -> &S {
        &self.source
    }

    /// The source it read, the reading dropped.
    pub fn into_source(self) -> S {
        self.source
    }

    /// Every object its cross-references list read, those not read yet as
    /// the whole read reads them, into a [`Document`]: what it read, its
    /// faults in the order [`Reading::faults`] says them. It fails
    /// only when the source doesn't give the bytes asked of it
    /// ([`Error::Io`]). The reading is gone after, and its records with it,
    /// so the writer can't read the data of the document's streams, nor copy
    /// an object as read: a document to write is made by
    /// [`Reading::to_document`], the reading kept and given to the write, or
    /// taken by a reading of the file opened again ([`Reading::adopt`]).
    pub fn into_document(self) -> Result<Document, Error> {
        let Reading {
            source,
            reader,
            bound,
            ..
        } = self;
        let whole = match source.whole() {
            Some(file) => {
                let Ok(whole) = reader.whole(InMemory(file), None);
                whole
            }
            None => {
                let file = ByPosition::new(&source, bound);
                reader.whole(file, None).map_err(Error::Io)?
            }
        };
        Ok(whole.document)
    }

    /// Every object its cross-references list read, as
    /// [`Reading::into_document`] reads them, into a [`Document`], the
    /// reading kept: what it has kept cloned, and what it hasn't read
    /// without keeping it, as [`Reading::object_unkept`] reads it, so that
    /// the document holds the one copy of it. Each object stream is
    /// decoded and kept, as a read of an object in it keeps it, with the
    /// objects the stream and its filters are. Its faults are in the order
    /// [`Reading::faults`] says them, and made after it,
    /// [`Reading::into_document`]'s document is the same; whatever the
    /// reading read before, it's the whole read, so long as no stream the
    /// reading decodes, before it or in it, reaches its budget.
    ///
    /// Kept beside the document, the reading hands out what the document
    /// doesn't hold: a stream's data, which stays where it was written
    /// ([`Data::At`](pdf_objects::object::Data::At)), through
    /// [`Reading::data`] and [`Reading::decoded_within`], and the bytes at a span
    /// ([`Reading::bytes`]); each record names the reading
    /// ([`Record::source`]). It fails only when the source doesn't give the
    /// bytes asked of it ([`Error::Io`]), keeping none of what it read
    /// unkept, so asked again it reads them again.
    pub fn to_document(&self) -> Result<Document, Error> {
        match self.source.whole() {
            Some(file) => {
                let Ok(document) = self.reader.document(InMemory(file));
                Ok(document)
            }
            None => (self.reader.document(self.by_position())).map_err(Error::Io),
        }
    }

    /// `doc`'s objects as read taken as this reading's, where it reads each
    /// alike and `doc` is encrypted, and opened, as its file is: each record
    /// then names it ([`Record::source`]). So a document read whole
    /// ([`document`](super::document), [`Reading::into_document`]), whose reading is
    /// gone, is written from a reading of its file opened again as one made
    /// by [`Reading::to_document`] is: its streams' data, where it was
    /// written ([`Data::At`](pdf_objects::object::Data::At)), read from it,
    /// and its objects copied as the bytes they were read from where the
    /// rules for a copy hold ([`write::Body::Read`](crate::write::Body::Read)),
    /// or passed over by an update after the file
    /// ([`write::Base::of`](crate::write::Base::of)).
    ///
    /// Each object `doc` holds with a record ([`Document::record`]) is
    /// compared with the reading's own record of its number: alike where
    /// it's the same record, but the reading's ([`Record`]'s equality: read
    /// as that number, where and how, and its bytes' hash). One the reading
    /// has kept is compared as it is; the rest are read without keeping
    /// them, as [`Reading::object_unkept`] reads them, a parse each, the
    /// object streams they're in decoded and kept: about what reading the
    /// file whole takes again. Each object `doc` holds unread
    /// ([`Document::unread`]) is one the reading doesn't read either: one it
    /// reads, as under larger [`Limits`](super::Limits), would be left out
    /// of what `doc` writes and not out of what the reading's own document
    /// writes. One `doc` doesn't list at all, removed or never listed (an
    /// object stream's members past where its data was cut off), isn't
    /// asked of, and is left out of what it writes. An object without a
    /// record, built or edited, is left as it is, to be written anew.
    ///
    /// `doc` was opened as the reading was ([`Document::encryption`] and
    /// [`Reading::encryption`] alike: neither encrypted, both opened by a
    /// password, both locked, and so on), so that its strings are plaintext
    /// where the reading's are, and its object streams read where the
    /// reading's do: a document read without the password the reading was
    /// given, whose strings are ciphertext and whose object streams'
    /// members don't read, isn't taken. And an encrypted file's encryption
    /// is `doc`'s where the encryption dictionary it was opened by, as
    /// stored, is the one `doc`'s trailer names, at the same number or
    /// alike in the trailer, and its first `/ID` element is `doc`'s: what a
    /// password's key is made of (ISO 32000-2, 7.6.4.3), so that the bytes
    /// copied are encrypted as `doc`'s objects were read. That's read from
    /// `doc`'s trailer as it was read: a trailer whose `/Encrypt` or `/ID`
    /// was edited first is refused, so a document is taken before it's
    /// edited so (a write writes `/Encrypt` and `/ID` from its encryptor,
    /// whatever the trailer says). The key itself isn't compared: the
    /// owner's password and the user's open a file with one key (ISO
    /// 32000-2, 7.6.4.3 and 7.6.4.4), so a reading by the owner's password
    /// takes a document read by the user's, and the other way.
    ///
    /// All or none: where an object isn't alike, `doc` is left as it was,
    /// and it's refused ([`Error::Unlike`]): another file, the file changed
    /// since `doc` was read, a reading as of another revision
    /// ([`Options::section`]) whose object at a number isn't `doc`'s, or
    /// under other limits. So it is where the encryption isn't alike
    /// ([`Error::UnlikeEncryption`]): the same objects' bytes under another
    /// `/ID`, whose key is another, or `doc` read by no password that opens
    /// the file, or by one where the reading wasn't. And so it is where the
    /// source doesn't give the bytes asked of it ([`Error::Io`]), and asked
    /// again it reads them again.
    pub fn adopt(&self, doc: &mut Document) -> Result<(), Error> {
        if !self.encrypts_as(doc) {
            return Err(Error::UnlikeEncryption);
        }
        for (id, record) in doc.records() {
            let alike = match self.record(id) {
                Some(own) => own == record,
                None => match self.object_unkept(id) {
                    Ok((_, own)) => own == *record,
                    Err(Error::Io(error)) => return Err(Error::Io(error)),
                    Err(_) => false,
                },
            };
            if !alike {
                return Err(Error::Unlike { id });
            }
        }
        // What `doc` holds unread, the reading doesn't read either.
        for (id, _) in doc.unread() {
            let reads = match self.record(id) {
                Some(_) => true,
                None => match self.object_unkept(id) {
                    Ok(_) => true,
                    Err(Error::Io(error)) => return Err(Error::Io(error)),
                    Err(_) => false,
                },
            };
            if reads {
                return Err(Error::Unlike { id });
            }
        }
        doc.adopted(self.id());
        Ok(())
    }

    /// Whether `doc` is encrypted, and opened, as the reading's file is:
    /// its [`Encryption`] of the reading's kind (opened by a password,
    /// whichever, locked, and so on); and neither encrypted, as the reader
    /// reads a trailer's `/Encrypt`, or both, by the encryption dictionary
    /// the reading opened the file by, as stored ([`Reading::stored`]), at
    /// the number `doc`'s trailer names, where the object `doc` holds is
    /// alike, or in `doc`'s trailer alike, and by the same first `/ID`
    /// element, an absent one empty. Dictionaries are compared as
    /// pdf-objects' writer writes them, as an object's places in its file
    /// aren't the dictionary.
    fn encrypts_as(&self, doc: &Document) -> bool {
        if discriminant(&doc.encryption) != discriminant(self.encryption()) {
            return false;
        }
        let encrypt = match doc.trailer.dict().get(b"Encrypt") {
            // A null is no entry, and so is a reference to no object (ISO
            // 32000-2, 7.3.9 and 7.3.10).
            Some(e) if e.is_null() => None,
            Some(e) if e.as_reference().is_some_and(|id| !doc.contains(id)) => None,
            encrypt => encrypt,
        };
        let (encrypt, stored) = match (encrypt, self.stored()) {
            (None, _) => return !self.reader.list.encrypts,
            (Some(encrypt), Some(stored)) => (encrypt, stored),
            (Some(_), None) => return false,
        };
        let dict = match encrypt.value() {
            Value::Ref(id) if stored.at == Some(*id) => doc.resolve(*id).and_then(Object::as_dict),
            Value::Dict(dict) if stored.at.is_none() => Some(dict),
            _ => None,
        };
        let theirs = as_written(&stored.dict);
        let alike = dict.is_some_and(|d| theirs.is_some() && as_written(d) == theirs);
        alike && first_id(doc) == stored.id
    }
}

/// `doc`'s first `/ID` element, as the reader takes a file's: its trailer's
/// `/ID`, followed where it's a reference, its first string; empty where
/// it hasn't one.
fn first_id(doc: &Document) -> Vec<u8> {
    let id = doc.trailer.dict().get(b"ID");
    let id = match id.map(Object::value) {
        Some(Value::Ref(at)) => doc.resolve(*at),
        _ => id,
    };
    let first = match id.map(Object::value) {
        Some(Value::Array(id)) => match id.first().map(Object::value) {
            Some(Value::String(s, _)) => s.as_bytes().map(<[u8]>::to_vec),
            _ => None,
        },
        _ => None,
    };
    first.unwrap_or_default()
}

/// Its objects, as a reference resolves them: each read and kept the first
/// time it's asked for, as [`Reading::get`] keeps it; none for one the file
/// hasn't, one that doesn't read, or one whose bytes the source doesn't
/// give, without why, which [`Resolve::try_resolve`] and [`Reading::get`]
/// say. A read the source stops keeps nothing, so asked again it reads, and
/// it's counted ([`Reading::failed_resolves`]).
impl<'o, S: ByteSource> Resolve<'o> for &'o Reading<S> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        let reading: &'o Reading<S> = self;
        match reading.source.whole() {
            Some(file) => {
                let Ok(object) = reading.reader.resolve(InMemory(file), id);
                object
            }
            None => match reading.reader.resolve(reading.by_position(), id) {
                Ok(object) => object,
                Err(_) => {
                    reading.failed_resolves.fetch_add(1, Ordering::Relaxed);
                    None
                }
            },
        }
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        let reading: &'o Reading<S> = self;
        let got = match reading.source.whole() {
            Some(file) => {
                let Ok(got) = reading.reader.get(InMemory(file), id);
                got
            }
            None => match reading.reader.get(reading.by_position(), id) {
                Ok(got) => got,
                Err(error) => {
                    reading.failed_resolves.fetch_add(1, Ordering::Relaxed);
                    let message = format!("{id} R doesn't read: {error}");
                    return Err(Unreadable::new(id, Why::Source, message));
                }
            },
        };
        got.map_or_else(
            |why| why.unreadable(id).map_or(Ok(None), Err),
            |object| Ok(Some(object)),
        )
    }
}

/// A stream's data as a viewer takes it ([`Streams`]), by
/// [`Reading::decoded_within`], owned: what decoded of a stream that decodes
/// in part, as PDFium takes it, unless what stopped it is the limit, at most
/// `limit` bytes or [`Limits::stream`](super::Limits::stream), whichever is
/// less. Where it gives none, it says why in a resolver's words ([`Why`]):
/// first what its resolver says of the object ([`Resolve::try_resolve`]), so
/// that an object that doesn't read is said as a reference to it is; then,
/// past the limit, [`Why::Limit`]; past the reader's own cap on a stream's
/// data, where that's less than the limit, or through more filters than it
/// takes ([`Unfiltered::Filters`]), [`Why::Capped`]; in a file no password
/// the reader was given opens, one whose security handler it can't open, a
/// stream the encryption dictionary refers to or one that doesn't decrypt
/// cleanly, [`Why::Locked`]; a source that didn't give the bytes,
/// [`Why::Source`]; through a filter the reader doesn't decode, or with a
/// `/Filter` or `/DecodeParms` it doesn't take as written ([`Unfiltered`]'s
/// other kinds), [`Why::Filter`]; and what isn't a stream, isn't there, or
/// whose decode fails before it gives a byte, [`Why::Damaged`]. A stream
/// that decodes cleanly to nothing is given, empty.
///
/// A reading has no method of its own of this name: `reading.decoded(id,
/// limit)` is this one, where the trait is in scope, and so is
/// `Streams::decoded(&reading, id, limit)`.
impl<S: ByteSource> Streams for Reading<S> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        match Resolve::try_resolve(&self, id) {
            Ok(Some(_)) => {}
            Ok(None) => return Err(missing(id)),
            Err(unreadable) => return Err(unreadable),
        }
        lenient(id, self.decoded_within(id, limit), self.capped_at(limit)).map(Cow::Owned)
    }
}

/// A file as it's read so far ([`Source`]): each answer its own method's
/// ([`Reading::ids`], [`Reading::get`] and the rest), in the trait's shape,
/// of what it has read when it's asked. So its faults are those of what
/// it has read, in the whole read's order, made for the call; an object
/// stream's decoded data, and whether it leaves its objects' strings in
/// the clear, are given of one it has decoded, whose header read, and of
/// one it hasn't decoded yet, none and false; its metadata is read on the
/// way, none where the source fails; and an object whose bytes the source
/// didn't give is [`Unread::Source`], which asked again may read. Read
/// whole, every answer is its document's ([`Reading::to_document`]).
impl<S: ByteSource> Source for Reading<S> {
    fn ids(&self) -> Vec<ObjectId> {
        Reading::ids(self).collect()
    }

    fn get(&self, id: ObjectId) -> Result<&Object, Unread> {
        Reading::get(self, id).map_err(|error| match error {
            Error::Unread(unread) => unread,
            Error::Io(error) => Unread::Source {
                message: error.to_string(),
            },
            // None that `get` gives; said as it is, if one ever is.
            error => Unread::Other(error.to_string()),
        })
    }

    fn trailer(&self) -> &Trailer {
        Reading::trailer(self)
    }

    fn header_version(&self) -> Option<Version> {
        Reading::header_version(self)
    }

    fn faults(&self) -> Cow<'_, [(Place, Fault)]> {
        Cow::Owned(Reading::faults(self))
    }

    fn size(&self) -> Option<usize> {
        Some(Reading::size(self))
    }

    fn metadata(&self) -> Option<Cow<'_, [u8]>> {
        Reading::metadata(self).ok().flatten()
    }

    fn record(&self, id: ObjectId) -> Option<&Record> {
        Reading::record(self, id)
    }

    fn held(&self, stream: ObjectId) -> Option<&[u8]> {
        let container = self.container(stream)?;
        container.header.is_ok().then_some(&container.data[..])
    }

    fn revisions(&self) -> &[Revision] {
        Reading::revisions(self)
    }

    fn facts(&self) -> Option<&Facts> {
        Some(Reading::facts(self))
    }

    fn in_the_clear(&self, stream: ObjectId) -> bool {
        self.container(stream)
            .is_some_and(|container| container.clear)
    }

    fn decoded_within(&self, id: ObjectId, limit: usize) -> Option<Result<Vec<u8>, Error>> {
        Some(Reading::decoded_within(self, id, limit))
    }
}

impl<S> Reading<S> {
    /// Object stream `stream`, once it's decoded: none before, and none of
    /// an id that isn't an object stream's, whose generation is 0.
    fn container(&self, stream: ObjectId) -> Option<&super::Container> {
        match stream.generation {
            0 => (self.reader.containers.get(&stream.number)?.get()).map(Box::as_ref),
            _ => None,
        }
    }
}

/// Why object `id`, which the file hasn't, gives no stream's data
/// ([`Streams`]): it isn't there.
pub(crate) fn missing(id: ObjectId) -> Unreadable {
    Unreadable::new(id, Why::Damaged, Unread::Missing { id }.to_string())
}

/// What stream `id`'s data, `decoded` as [`Reading::decoded_within`] gives
/// it, is as a viewer takes it ([`Streams`]): what decoded of a stream that
/// decodes in part, unless what stopped it is the limit; or why there's
/// none, as `Streams` for a [`Reading`] says, an object that doesn't read
/// as [`Unread::unreadable`] says. A decode the limit stopped is past the
/// caller's limit, or past the reader's own cap where `capped` says that
/// was the bound ([`Reading::capped_at`]).
pub(crate) fn lenient(
    id: ObjectId,
    decoded: Result<Vec<u8>, Error>,
    capped: bool,
) -> Result<Vec<u8>, Unreadable> {
    let unreadable =
        |why: Why, error: &Error| Unreadable::new(id, why, format!("{id} R doesn't read: {error}"));
    match decoded {
        Ok(data) => Ok(data),
        Err(Error::Decode(error)) => {
            let limited = (error.faults.iter()).any(|(_, f)| *f == pdf_filters::Fault::Limit);
            match (limited, error.data.is_empty()) {
                (false, false) => Ok(error.data),
                (true, _) => {
                    let why = if capped { Why::Capped } else { Why::Limit };
                    Err(unreadable(why, &Error::Decode(error)))
                }
                (false, true) => Err(unreadable(Why::Damaged, &Error::Decode(error))),
            }
        }
        Err(error @ Error::Io(_)) => Err(unreadable(Why::Source, &error)),
        Err(
            error @ (Error::Locked | Error::Encrypted | Error::Unsupported | Error::Decrypt(_)),
        ) => Err(unreadable(Why::Locked, &error)),
        Err(error @ Error::Filters(Unfiltered::Filters { .. })) => {
            Err(unreadable(Why::Capped, &error))
        }
        Err(error @ Error::Filters(_)) => Err(unreadable(Why::Filter, &error)),
        Err(Error::Unread(why)) => match why.unreadable(id) {
            Some(unreadable) => Err(unreadable),
            None => Err(missing(id)),
        },
        Err(error) => Err(unreadable(Why::Damaged, &error)),
    }
}
