//! A reading of a file: opened, then each object read when it's asked for,
//! on any thread; or read whole, into a [`Document`].

use std::borrow::Cow;
use std::cell::Cell;
use std::fmt;
use std::io;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use pdf_crypt::{Encoded, Handler, Password, Refused};
use pdf_objects::Resolve;
use pdf_objects::object::{Object, ObjectId, Span, Stream, Value};
use pdf_objects::version::Version;

use super::window::{ByPosition, Bytes, InMemory};
use super::{Crypt, Decode, Options, Reader, Stored, Unfiltered, Whole, XrefEntry};
use crate::crypt::{List, is_xref};
use crate::filter;
use crate::source::{Document, Encryption, Facts, Record, Trailer, Unread, Update, Warning};

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
    bytes.get(range).map(Cow::Borrowed).ok_or_else(|| {
        let message = format!("bytes {start}..{end} aren't within the {len} there are");
        io::Error::new(io::ErrorKind::UnexpectedEof, message)
    })
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
    #[error("{} {} R isn't a stream", .id.0, .id.1)]
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
    /// [`Fault::Limit`](pdf_filters::Fault::Limit) among them.
    #[error("its data doesn't decode cleanly: {0}")]
    Decode(pdf_filters::Error),
}

/// A stream, and its data as [`Reading::data`] hands it out; or why it
/// doesn't.
type Handed<'f> = Result<(&'f Stream, Cow<'f, [u8]>), Error>;

/// [`Error::Outside`]'s text.
fn outside(span: &Span) -> String {
    let (start, end) = (span.start, span.end);
    match span.stream {
        None => format!("bytes {start}..{end} aren't in the file"),
        Some((n, g)) => {
            format!("bytes {start}..{end} aren't in object stream {n} {g} R's decoded data")
        }
    }
}

/// A file being read: opened, then each object read when it's first asked
/// for, kept or not, on any thread; or read whole, into a [`Document`], as
/// [`read`](super::read) reads it, the reading taken
/// ([`Reading::into_document`]) or kept beside it
/// ([`Reading::to_document`]).
///
/// [`Reading::open`] reads the header, the cross-reference sections and
/// the trailer (or, where they don't read, finds the objects by scanning
/// the file), and opens an encrypted file's security handler by the empty
/// password, [`Reading::open_with`] by the passwords it's given; no object
/// yet. A reading no password opened is opened by another with
/// [`Reading::unlock`]. [`Reading::get`] reads an object
/// the first time it's asked for and keeps it, with what the reader warned
/// of as it read it, decrypting its strings in an encrypted file it opened;
/// [`Reading::object_unkept`] reads one without keeping it. An object in an
/// object stream is read from the stream's decoded data, the stream decoded
/// and kept the first time an object in it is read, once, whichever thread
/// asks. [`Reading::data`] gives a stream's data, decrypted in an encrypted
/// file it opened, and [`Reading::decoded`] that through its filters, each
/// read when it's asked for, and not kept.
///
/// Read in any order, on any threads, then made a [`Document`], a reading
/// is the whole read, the same objects, warnings in the same order and the
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
/// `%%EOF` back from its end, and each section at its offset; a file whose
/// sections don't read is read whole, to be scanned. Where the source
/// fails ([`Error::Io`]), the call keeps nothing of what it was reading,
/// but an object it read whole on the way, and asked again it reads again.
///
/// ```
/// use pdf_structure::bytes::{Options, Reading};
///
/// let file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n\
///     xref\n0 2\n0000000000 65535 f \n0000000009 00000 n \n\
///     trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n45\n%%EOF\n";
/// let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
/// assert_eq!(reading.ids().collect::<Vec<_>>(), [(1, 0)]);
/// let catalog = reading.get((1, 0)).unwrap();
/// assert!(catalog.as_dict().is_some_and(|d| d.has(b"Type")));
/// assert!(reading.warnings().is_empty());
/// ```
pub struct Reading<S> {
    source: S,
    pub(super) reader: Reader,
    /// Whether an object read by position is first given a window to a
    /// byte past the next listed offset: always, but where the tests and
    /// the fuzz target read without, so that a small first window reaches
    /// every read's windows.
    bound: bool,
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

impl<'a> Reading<&'a [u8]> {
    /// The file `file` is, opened as [`Reading::open`] opens it, which
    /// can't fail in memory; `native` but in the tests that make it ask
    /// `decoder` for every stream.
    pub(crate) fn in_memory(
        file: &'a [u8],
        passwords: &[Password],
        options: &Options,
        native: bool,
        decoder: Option<&dyn Decode>,
    ) -> Reading<&'a [u8]> {
        let Ok(reader) = Reader::new(InMemory(file), passwords, options, native, decoder);
        Reading {
            reader,
            source: file,
            bound: true,
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
    /// The file `source` holds, opened: its header, its cross-reference
    /// sections and trailer, and, when it's encrypted, its security
    /// handler, opened by the empty password, as most files' is, which
    /// [`Reading::unlock`] opens by another when it doesn't. It fails only
    /// when the source doesn't give its bytes ([`Error::Io`]).
    pub fn open(source: S, options: &Options) -> Result<Reading<S>, Error> {
        Reading::open_by(source, &[], options, true)
    }

    /// The file `source` holds, opened as [`Reading::open`] opens it, but
    /// its security handler by `passwords`, in the empty password's place:
    /// each in each of its encodings ([`Password::encodings`]), tried as the
    /// owner's and then the user's, every password's that fit the file's
    /// revision first, in the order given, then every password's others
    /// (pdf-crypt's `Handler::open`, given them all), as
    /// [`read_opened`](super::read_opened) tries them. So a later password
    /// can open it before an earlier one that would open it only in an
    /// encoding that doesn't fit. Given none, the empty password. A wrong one leaves it
    /// [`Encryption::Locked`], or [`Encryption::Plain`] where its strings
    /// and streams aren't encrypted, as no password would: the empty one
    /// isn't tried after it. Which opened it is
    /// [`Encryption::Opened`]'s `by`, its index among `passwords`. The
    /// passwords aren't kept. It fails only when the source doesn't give
    /// its bytes ([`Error::Io`]).
    pub fn open_with(
        source: S,
        passwords: &[Password],
        options: &Options,
    ) -> Result<Reading<S>, Error> {
        Reading::open_by(source, passwords, options, true)
    }

    /// [`Reading::open_with`], an object read by position first given a
    /// window to a byte past the next listed offset when `bound`.
    pub(crate) fn open_by(
        source: S,
        passwords: &[Password],
        options: &Options,
        bound: bool,
    ) -> Result<Reading<S>, Error> {
        let reader = match source.whole() {
            Some(file) => {
                let Ok(reader) = Reader::new(InMemory(file), passwords, options, true, None);
                reader
            }
            None => {
                let file = ByPosition::new(&source, bound);
                Reader::new(file, passwords, options, true, None).map_err(Error::Io)?
            }
        };
        Ok(Reading {
            source,
            reader,
            bound,
        })
    }

    /// The reading, an encrypted file no password opened, opened by
    /// `passwords`, as [`Reading::open_with`] would have opened it: those
    /// that fit the file's revision first, in the order given, then the
    /// rest, each as the owner's and then the user's (see pdf-crypt's
    /// `Handler::open`), and [`Encryption::Opened`]'s `by` the index among
    /// `passwords` of the one that opened it. Nothing of the file is read again: the
    /// reading kept its encryption dictionary's values and its first `/ID`
    /// element while no password had opened it. What it read and decoded
    /// is dropped, to be read again decrypted, and what it warned of is
    /// what it warned of opening the file, with what's wrong with `/Perms`
    /// among it; its budget is what opening the file left.
    ///
    /// Given back as it was, with why: where none of `passwords` opens it
    /// ([`Refused::Password`]), or its security handler is one the reader
    /// can't open ([`Refused::Unsupported`]). A file that isn't encrypted,
    /// or that a password opened, is given back as it is.
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
    fn by_position(&self) -> ByPosition<'_> {
        ByPosition::new(&self.source, self.bound)
    }

    /// Every object its cross-references list, by number: each in use, of
    /// its generation, and each in an object stream, of generation 0.
    pub fn ids(&self) -> impl Iterator<Item = ObjectId> + '_ {
        (self.reader.entries.iter()).filter_map(|&(n, (generation, entry))| match entry {
            _ if n == 0 => None,
            XrefEntry::InUse { .. } => Some((n, generation)),
            XrefEntry::Compressed { .. } => Some((n, 0)),
            _ => None,
        })
    }

    /// The newest entry its cross-reference sections give each object
    /// number (or, when the file was scanned, the scan), by number: the
    /// number, its generation and what the entry says.
    pub fn entries(&self) -> impl Iterator<Item = (u32, u16, XrefEntry)> + '_ {
        (self.reader.entries.iter()).map(|&(n, (generation, entry))| (n, generation, entry))
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
    /// from: read without keeping it, its warnings or a failure to read it,
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

    /// The file's length in bytes.
    pub fn size(&self) -> usize {
        self.source.len()
    }

    /// What the reader warned of in what it has read so far, in the order
    /// the whole read says it: as it opened the file; as it read each
    /// object in use, by number; as it decoded each object stream, by
    /// number; as it read each object in an object stream, by number; and
    /// as it decrypted each object's strings, by number. What was read
    /// without keeping it isn't here.
    pub fn warnings(&self) -> Vec<Warning> {
        self.reader.warnings()
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
            Some((stream, 0)) => match self.reader.container(file, stream)? {
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
    /// whose values are direct), up to [`Limits::stream`](super::Limits::stream)
    /// bytes; a stream without filters is its data. What's decoded isn't
    /// kept, nor counted against the reading's budget, which is for what
    /// the reading keeps. Data that doesn't decode cleanly is an error
    /// holding what did, a stream cut off at the limit among them
    /// ([`Error::Decode`]).
    pub fn decoded(&self, id: ObjectId) -> Result<Vec<u8>, Error> {
        match self.source.whole() {
            Some(file) => {
                let Ok(decoded) = self.decoded_in(InMemory(file), id);
                decoded
            }
            None => self.decoded_in(self.by_position(), id).map_err(Error::Io)?,
        }
    }

    /// Stream `id`, and its data, as [`Reading::data`] gives it, of `file`.
    fn data_in<'f, F: Bytes<'f>>(&'f self, file: F, id: ObjectId) -> Result<Handed<'f>, F::Error> {
        let reader = &self.reader;
        let stream = match reader.get(file, id)? {
            Ok(object) => match object.value() {
                Value::Stream(stream) => stream,
                _ => return Ok(Err(Error::NotAStream { id })),
            },
            Err(why) => return Ok(Err(Error::Unread(why))),
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
                match handler.decrypt_stream(id, &written, method.as_ref()) {
                    Ok(plain) => Ok((stream, Cow::Owned(plain))),
                    Err(error) => Err(Error::Decrypt(error)),
                }
            }
        })
    }

    /// [`Reading::decoded`], of `file`.
    fn decoded_in<'f, F: Bytes<'f>>(
        &'f self,
        file: F,
        id: ObjectId,
    ) -> Result<Result<Vec<u8>, Error>, F::Error> {
        let (stream, data) = match self.data_in(file, id)? {
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
                let (limit, flate) = (reader.limits.stream, reader.flate);
                pdf_filters::decode(&data, &filters, limit, flate).map_err(Error::Decode)
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
    /// warnings in the order [`Reading::warnings`] says them. It fails
    /// only when the source doesn't give the bytes asked of it
    /// ([`Error::Io`]). The reading is gone after, and its records with it,
    /// so the writer can't read the data of the document's streams, nor copy
    /// an object as read: a document to write is made by
    /// [`Reading::to_document`], the reading kept and given to the write.
    pub fn into_document(self) -> Result<Document, Error> {
        let Reading {
            source,
            reader,
            bound,
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
    /// objects the stream and its filters are. Its warnings are in the order
    /// [`Reading::warnings`] says them, and made after it,
    /// [`Reading::into_document`]'s document is the same; whatever the
    /// reading read before, it's the whole read, so long as no stream the
    /// reading decodes, before it or in it, reaches its budget.
    ///
    /// Kept beside the document, the reading hands out what the document
    /// doesn't hold: a stream's data, which stays where it was written
    /// ([`Data::At`](pdf_objects::object::Data::At)), through
    /// [`Reading::data`] and [`Reading::decoded`], and the bytes at a span
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
}

/// Its objects, as a reference resolves them: each read and kept the first
/// time it's asked for, as [`Reading::get`] keeps it; none for one the file
/// hasn't, one that doesn't read, or one whose bytes the source doesn't
/// give, without why ([`Reading::get`] says). A read the source stops keeps
/// nothing, so asked again it reads.
impl<'o, S: ByteSource> Resolve<'o> for &'o Reading<S> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        let reading: &'o Reading<S> = self;
        match reading.source.whole() {
            Some(file) => {
                let Ok(object) = reading.reader.resolve(InMemory(file), id);
                object
            }
            None => (reading.reader.resolve(reading.by_position(), id)).unwrap_or(None),
        }
    }
}
