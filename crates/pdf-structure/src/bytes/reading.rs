//! A reading of a file: opened, then each object read when it's asked for,
//! on any thread; or read whole, into a [`Document`].

use std::borrow::Cow;
use std::fmt;
use std::io;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use pdf_objects::object::{Object, ObjectId, Span};
use pdf_objects::version::Version;

use super::{Decode, Options, Reader, Whole, XrefEntry};
use crate::source::{Document, Encryption, Facts, Record, Trailer, Unread, Update, Warning};

/// A file's bytes, which a [`Reading`] reads. Bytes in memory give all of
/// them ([`ByteSource::whole`]): the slices, vectors and shared slices of
/// bytes do, and so does a reference to a source.
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

    /// The bytes in `range`, within `0..len()`.
    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>>;
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

impl ByteSource for Arc<[u8]> {
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

impl<S: ByteSource + ?Sized> ByteSource for &S {
    fn len(&self) -> usize {
        (**self).len()
    }

    fn whole(&self) -> Option<&[u8]> {
        (**self).whole()
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        (**self).read_at(range)
    }
}

/// Which reading a [`Record`] is of: each reading has its own, unique in
/// the process, so a span is known for the file it's in. Two readings of
/// one file have two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceId(u64);

impl SourceId {
    /// One no reading has had.
    fn next() -> SourceId {
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
    /// The source didn't give the bytes asked of it: nothing read is kept,
    /// so the call reads again when it's asked again.
    #[error("the file doesn't read: {0}")]
    Io(io::Error),
    /// The file is encrypted, and its bytes aren't handed out as written,
    /// where they could be ciphertext.
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
}

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
/// for, kept or not, on any thread; or read whole, into a [`Document`]
/// ([`Reading::into_document`]), as [`read`](super::read) reads it.
///
/// [`Reading::open`] reads the header, the cross-reference sections and
/// the trailer (or, where they don't read, finds the objects by scanning
/// the file), and opens an encrypted file's security handler with the
/// passwords it's given; no object yet. [`Reading::get`] reads an object
/// the first time it's asked for and keeps it, with what the reader warned
/// of as it read it, decrypting its strings in an encrypted file it opened;
/// [`Reading::object_unkept`] reads one without keeping it. An object in an
/// object stream is read from the stream's decoded data, the stream decoded
/// and kept the first time an object in it is read, once, whichever thread
/// asks.
///
/// Read in any order, on any threads, then made a [`Document`], a reading
/// is the whole read, the same objects, warnings in the same order and the
/// same decoded data, so long as no stream it decodes reaches its
/// [`budget`](super::Limits::budget): that's spent in the order streams
/// are decoded, so which stream it cuts depends on the order objects are
/// asked for, where the whole read decodes them by number.
///
/// ```
/// use pdf_structure::bytes::{Options, Reading};
///
/// let file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n\
///     xref\n0 2\n0000000000 65535 f \n0000000009 00000 n \n\
///     trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n45\n%%EOF\n";
/// let reading = Reading::open(&file[..], &[], &Options::DEFAULT).unwrap();
/// assert_eq!(reading.ids().collect::<Vec<_>>(), [(1, 0)]);
/// let catalog = reading.get((1, 0)).unwrap();
/// assert!(catalog.as_dict().is_some_and(|d| d.has(b"Type")));
/// assert!(reading.warnings().is_empty());
/// ```
pub struct Reading<S> {
    source: S,
    /// A source's bytes when it doesn't hold them in memory, read whole as
    /// it's opened.
    held: Option<Vec<u8>>,
    pub(super) reader: Reader,
    id: SourceId,
}

impl<S> fmt::Debug for Reading<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reading")
            .field("source", &self.id)
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
        passwords: &[&[u8]],
        options: &Options,
        native: bool,
        decoder: Option<&dyn Decode>,
    ) -> Reading<&'a [u8]> {
        Reading {
            reader: Reader::new(file, passwords, options, native, decoder),
            source: file,
            held: None,
            id: SourceId::next(),
        }
    }
}

impl<S: ByteSource> Reading<S> {
    /// The file `source` holds, opened: its header, its cross-reference
    /// sections and trailer, and, when it's encrypted, its security
    /// handler, opened by the first of `passwords` that opens it, each
    /// tried as the owner's and then the user's (none, the empty
    /// password), as [`read_opened`](super::read_opened) tries them. The
    /// passwords aren't kept. It fails only when the source doesn't give
    /// its bytes ([`Error::Io`]).
    pub fn open(source: S, passwords: &[&[u8]], options: &Options) -> Result<Reading<S>, Error> {
        let held = match source.whole() {
            Some(_) => None,
            None => {
                let bytes = source.read_at(0..source.len()).map_err(Error::Io)?;
                Some(bytes.into_owned())
            }
        };
        let file = (held.as_deref())
            .or_else(|| source.whole())
            .unwrap_or_default();
        let reader = Reader::new(file, passwords, options, true, None);
        Ok(Reading {
            source,
            held,
            reader,
            id: SourceId::next(),
        })
    }

    /// The file's bytes.
    fn file(&self) -> &[u8] {
        (self.held.as_deref())
            .or_else(|| self.source.whole())
            .unwrap_or_default()
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

    /// Object `id`, as written: read the first time it's asked for, and
    /// kept, as a failure to read it is ([`Error::Unread`]), and so is the
    /// object stream it's in, decoded. Its references are kept as
    /// references, and its strings decrypted in an encrypted file that
    /// opened.
    pub fn get(&self, id: ObjectId) -> Result<&Object, Error> {
        self.reader.get(self.file(), id).map_err(Error::Unread)
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
        let object = match self.reader.slots[i].get() {
            Some(read) => read.object.clone(),
            None => self.reader.read(self.file(), i).object,
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
        self.file().len()
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
        let (file, reader) = (self.file(), &self.reader);
        let plain = !reader.encrypts || reader.plain_metadata;
        let get = |id| reader.get(file, id).ok();
        let metadata = super::metadata(file, &reader.trailer, &get, !plain);
        Ok(metadata.map(Cow::Borrowed))
    }

    /// The bytes at `span`: in the file, or in the decoded data of the
    /// object stream it names, which is decoded and kept when it isn't yet.
    /// An encrypted file's are refused ([`Error::Encrypted`]), as they may
    /// be ciphertext, but for an object stream's decoded data, which is
    /// decrypted.
    pub fn bytes(&self, span: Span) -> Result<Cow<'_, [u8]>, Error> {
        let file = self.file();
        let range = span.start..span.end;
        let outside = Error::Outside { span };
        match span.stream {
            None if self.reader.encrypts => Err(Error::Encrypted),
            None => file.get(range).map(Cow::Borrowed).ok_or(outside),
            Some((stream, 0)) => {
                let container = (self.reader.container(file, stream)).ok_or(outside)?;
                if let Err(why) = &container.header {
                    return Err(Error::Unread(why.clone()));
                }
                let bytes = container.data.get(range);
                bytes.map(Cow::Borrowed).ok_or(Error::Outside { span })
            }
            Some(_) => Err(outside),
        }
    }

    /// Which reading this is, which its records are of.
    pub fn source(&self) -> SourceId {
        self.id
    }

    /// Every object its cross-references list read, those not read yet as
    /// the whole read reads them, into a [`Document`]: what it read, its
    /// warnings in the order [`Reading::warnings`] says them. It fails
    /// only when the source doesn't give the bytes asked of it
    /// ([`Error::Io`]).
    pub fn into_document(self) -> Result<Document, Error> {
        Ok(self.whole(None).document)
    }

    /// The reading read whole, with `decoder` asked for what the reader
    /// doesn't decode or decrypt itself.
    pub(crate) fn whole(self, decoder: Option<&dyn Decode>) -> Whole {
        let Reading {
            source,
            held,
            reader,
            ..
        } = self;
        let file = (held.as_deref())
            .or_else(|| source.whole())
            .unwrap_or_default();
        reader.whole(file, decoder)
    }
}
