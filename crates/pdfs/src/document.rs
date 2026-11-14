//! A document being written: a new one, or a file read and edited, its pages
//! added, and saved whole, linearized or as an update.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::io;
use std::path::Path;

use pdf_content_streams::{ContentWriter, Res};
use pdf_crypt::Protection;
use pdf_filters::{Backend, Level};
use pdf_linearize::family::{self, Linearized};
use pdf_objects::version::Version;
use pdf_objects::{Object, ObjectId, dict};
use pdf_semantics::pages::{self, Rect};
use pdf_semantics::xmp::Claims;
use pdf_structure::XrefKind;
use pdf_structure::read::Identity;
use pdf_structure::write::{self, Base, Deflate, Encryptor, Pack, ReadFrom, Written};

use crate::build;
use crate::error::Error;
use crate::limits::Limits;
use crate::phases::phase;

/// The version a new document's header says unless it's told otherwise: 1.7,
/// which every reader reads, and pdf-structure's writer's default.
const NEW: Version = Version::new(1, 7);

/// The least version a file's header says where a cross-reference stream
/// lists it and object streams hold its objects (ISO 32000-2, 7.5.8).
const STREAMS: Version = Version::new(1, 5);

/// A document being written: a new one ([`Document::new`]), or a file read
/// and edited ([`Pdf::edit`](crate::Pdf::edit)), which borrows the file's
/// reading, `'p`, to copy what it doesn't change from. Pages are added to it
/// ([`Document::add_page`]), each drawn by a content writer, and written into
/// it at a save, each resource they draw with once a file; and it's saved
/// whole ([`Document::save`]), linearized ([`Document::save_linearized`]) or,
/// edited, as an update after the file read ([`Document::save_update`]). What
/// else it holds is pdf-structure's document ([`Document::structure`]),
/// whose objects a caller edits as it likes.
///
/// A save writes the file in memory first, so one refused leaves its path as
/// it was; each has a `_to` form that writes into memory alone. Saved twice in
/// the clear, nothing changed between, it writes the same bytes. It's `Send`
/// and `Sync`, and its `Debug` gives how many pages it has and how it's
/// written, not what it holds.
///
/// A page of text in Helvetica, saved and read back:
///
/// ```
/// use pdfs::pdf_fonts::Std14;
/// use pdfs::pdf_semantics::pages::Rect;
///
/// let mut doc = pdfs::Document::new();
/// let page = doc.add_page(Rect([0.0, 0.0, 612.0, 792.0]));
/// page.begin_text();
/// page.font(pdfs::standard_font(Std14::Helvetica), 24.0);
/// page.text_move(72.0, 700.0);
/// page.show(&Std14::Helvetica.codes("Hello, world").unwrap());
/// page.end_text();
/// let file = doc.to_bytes().unwrap();
///
/// let pdf = pdfs::from_bytes(file).unwrap();
/// let pages = pdf.pages().unwrap();
/// assert_eq!(pages.len(), 1);
/// assert_eq!(pages.get(0).unwrap().text().unwrap().text(), "Hello, world");
/// ```
pub struct Document<'p> {
    /// pdf-structure's document, which is written.
    doc: pdf_structure::Document,
    /// The pages added since a save wrote them in, in order.
    added: VecDeque<Added>,
    /// A new document's pages written in, in order, which its page tree lists.
    placed: Vec<ObjectId>,
    /// A new document's page tree's nodes, as it was last written: their
    /// numbers taken again when it's written anew.
    nodes: Vec<ObjectId>,
    /// Whether a new document has pages its page tree doesn't list yet.
    stale: bool,
    /// Each resource written into it, by what it is: each once.
    written: HashMap<Res, ObjectId>,
    /// What it's encrypted by: none for in the clear.
    encryptor: Option<Encryptor>,
    /// The file it was read from, where it was.
    read: Option<Read<'p>>,
    /// The caps the file it was read from was read under, which a linearized
    /// save walks its trees under; each crate's defaults for a new one.
    limits: Limits,
}

/// A page added: its box, and its content's writer.
struct Added {
    media_box: Rect,
    writer: ContentWriter,
}

/// What an edited document keeps of the file it was read from.
struct Read<'p> {
    /// The reading, which what's as read is copied from.
    from: &'p dyn ReadFrom,
    /// The file as its last section describes it, which an update goes
    /// after; or why there's none.
    base: Result<Base, write::Error>,
    /// The file the reading reads by position, where it was opened by its
    /// path.
    identity: Option<Identity>,
}

impl fmt::Debug for Document<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Document")
            .field("added", &self.added.len())
            .field("pages", &self.placed.len())
            .field("resources", &self.written.len())
            .field("encrypted", &self.encryptor.is_some())
            .field("edited", &self.read.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for Document<'static> {
    fn default() -> Self {
        Document::new()
    }
}

impl Document<'static> {
    /// A new document of no pages yet: its catalog object 1, its header 1.7,
    /// nothing else. A page tree is written over its pages at each save that
    /// adds one, its nodes of 32 kids at most (pdf-semantics'
    /// [`pages::FANOUT`]), each page carrying its own attributes; no `/Info`
    /// is written unless a caller puts one in its trailer, and nothing is
    /// stamped on it.
    #[must_use]
    pub fn new() -> Document<'static> {
        let trailer = dict! { "Root" => ObjectId::new(1, 0) };
        let mut doc = pdf_structure::Document::new(NEW, trailer);
        doc.insert(1, dict! { "Type" => "Catalog" });
        Document::of(doc, None, None, Limits::DEFAULT)
    }
}

impl<'p> Document<'p> {
    /// `doc`, encrypted by `encryptor`, edited from the file `read` is of,
    /// which was read under `limits`.
    fn of(
        doc: pdf_structure::Document,
        encryptor: Option<Encryptor>,
        read: Option<Read<'p>>,
        limits: Limits,
    ) -> Document<'p> {
        Document {
            doc,
            added: VecDeque::new(),
            placed: Vec::new(),
            nodes: Vec::new(),
            stale: false,
            written: HashMap::new(),
            encryptor,
            read,
            limits,
        }
    }

    /// `doc`, read under `limits` from the file `from` reads, to write under
    /// `encryptor`, its encryption, after `base`, the file read, which
    /// `identity` names where that reading reads it by position.
    pub(crate) fn edited(
        doc: pdf_structure::Document,
        encryptor: Option<Encryptor>,
        from: &'p dyn ReadFrom,
        base: Result<Base, write::Error>,
        identity: Option<Identity>,
        limits: Limits,
    ) -> Document<'p> {
        let read = Read {
            from,
            base,
            identity,
        };
        Document::of(doc, encryptor, Some(read), limits)
    }

    /// pdf-structure's document, which is written: its objects, its trailer
    /// and its header's version. Pages added are in it once a save has
    /// written them in.
    #[must_use]
    pub const fn structure(&self) -> &pdf_structure::Document {
        &self.doc
    }

    /// pdf-structure's document, to edit: an object set, changed or taken
    /// out is written so at the next save.
    pub const fn structure_mut(&mut self) -> &mut pdf_structure::Document {
        &mut self.doc
    }

    /// pdf-structure's document, taken, without the pages added since the
    /// last save, which a save writes in.
    #[must_use]
    pub fn into_structure(self) -> pdf_structure::Document {
        self.doc
    }

    /// A page of `media_box` added after its pages, and the writer of its
    /// content, which a caller draws it with until the document is saved.
    /// What the content draws with is written into the document at a save,
    /// each resource once a file however many pages draw with it, and bound
    /// in the page's own `/Resources`. To a new document, it's a page of its
    /// page tree; to an edited one, a kid of its page tree's root, with every
    /// attribute a page inherits its own (`/MediaBox`, `/CropBox` the same
    /// box, `/Rotate 0`, `/Resources`), so nothing above it changes it.
    ///
    /// The box is written as the content writer writes a number: a whole one
    /// within 32 bits as an integer, else a real, and one that isn't finite,
    /// NaN or an infinity, as 0, which no file can hold. Its numbers are
    /// pdf-semantics' `f64`, where the writer's are `f32`
    /// ([`ContentWriter::rect`], say), so a layout reckoned in one is cast to
    /// the other.
    pub fn add_page(&mut self, media_box: Rect) -> &mut ContentWriter {
        let at = self.added.len();
        self.added.push_back(Added {
            media_box,
            writer: ContentWriter::new(),
        });
        &mut self.added[at].writer
    }

    /// What `claims` says written as its catalog's metadata stream, an XMP
    /// packet, as [`build::claim`] writes it.
    ///
    /// ```
    /// use pdfs::prelude::*;
    ///
    /// let mut claims = Claims::default();
    /// claims.pdfua_part = Some("1".into());
    /// let mut doc = Document::new();
    /// doc.add_page(Rect::LETTER);
    /// doc.claim(&claims).unwrap();
    /// // A second packet would replace the first: refused.
    /// assert!(matches!(doc.claim(&claims), Err(pdfs::Error::HasMetadata)));
    /// let pdf = pdfs::from_bytes(doc.to_bytes().unwrap()).unwrap();
    /// assert_eq!(pdf.claims().unwrap(), Some(claims));
    /// ```
    ///
    /// # Errors
    ///
    /// As [`build::claim`] refuses, with nothing changed: no catalog, a
    /// catalog that names a `/Metadata` already ([`Error::HasMetadata`]),
    /// whose packet the claims' alone would replace, and a claim that
    /// wouldn't read back as it's given.
    pub fn claim(&mut self, claims: &Claims) -> Result<(), Error> {
        build::claim(&mut self.doc, claims).map(|_| ())
    }

    /// Each save after this encrypted as `protection` says, under a new
    /// encryption of revision 6, AES-256, its key and salts from the operating
    /// system (pdf-structure's [`Encryptor::create`]), whatever it was
    /// encrypted by before. An update can't change a file's encryption, so
    /// [`Document::save_update`] of an edited document is then refused.
    ///
    /// Revision 6 is PDF 2.0's (ISO 32000-2, 7.6.4.2). A save under it keeps
    /// the document's own version all the same, 1.7 for a new one, as PDFium
    /// and qpdf read it. So a checker that holds a file to its version finds
    /// four entries too new for 1.7: `/V 5`, `/R 6` and `/Length 256` in the
    /// encryption dictionary, and `/CFM /AESV3` in its crypt filter. Adobe's
    /// extension level 8 to PDF 1.7 defines them. A caller may declare it in
    /// the catalog's `/Extensions` (ISO 32000-2, 7.12), as
    /// `/ADBE << /BaseVersion /1.7 /ExtensionLevel 8 >>`; nothing here does.
    /// Or a caller saves the file as PDF 2.0 ([`Document::save_with`]), which
    /// then asks more ([`Document::save_with_to`]).
    ///
    /// # Errors
    ///
    /// [`Error::Protection`], where pdf-crypt makes no encryption of it: a
    /// password that isn't prepared, or is empty or too long, no password,
    /// permissions withheld where the owner's password is the user's, and the
    /// operating system's random number generator failing. Nothing changes.
    pub fn encrypt(&mut self, protection: &Protection<'_>) -> Result<(), Error> {
        self.encryptor = Some(Encryptor::create(protection).map_err(Error::Protection)?);
        Ok(())
    }

    /// Each save after this in the clear, whatever it was encrypted by: an
    /// edited document is saved under its file's own encryption unless this
    /// says otherwise.
    ///
    /// Nothing here holds a caller to what the file permits: an encrypted
    /// file opened by its user's password alone, which withholds changes or
    /// copying ([`Encryption::Opened`](pdf_structure::Encryption::Opened)'s
    /// `permissions`), is edited, saved, and saved in the clear by this: the
    /// encryption itself can't stop it, and ISO 32000-2 asks what processes a
    /// file to honour them (7.6.4.1), which here is the caller, which reads
    /// them, and which password opened the file (`by`), before it edits it.
    ///
    /// ```
    /// use pdfs::prelude::*;
    ///
    /// // A file encrypted by a password, opened by it and saved in the clear.
    /// let password = Password("secret").prepared().unwrap();
    /// let mut doc = Document::new();
    /// doc.add_page(Rect::LETTER);
    /// doc.encrypt(&Protection::new(&password)).unwrap();
    /// let file = doc.to_bytes().unwrap();
    /// let mut options = Options::DEFAULT;
    /// options.passwords = &[Password("secret")];
    /// let pdf = options.from_bytes(file).unwrap();
    /// let mut edited = pdf.edit().unwrap();
    /// edited.decrypt();
    /// let clear = pdfs::from_bytes(edited.to_bytes().unwrap()).unwrap();
    /// assert_eq!(*clear.encryption(), Encryption::None);
    /// ```
    pub fn decrypt(&mut self) {
        self.encryptor = None;
    }

    /// Each save after this under an edited file's own RC4 encryption writing
    /// what would put new plaintext under a keystream the file used, which is
    /// refused unless the caller says so: an update's object whose strings or
    /// data are new where the file's revisions had others, which leaks their
    /// XOR to whoever holds the file; what's so written is listed
    /// ([`Written::keystreams_reused`]). pdf-structure's
    /// [`Encryptor::reusing_rc4_keystreams`] says what that means. Under AES,
    /// or in the clear, it changes nothing. A whole file needs none of it, as
    /// it writes each object it writes anew at a fresh generation, but of a
    /// file whose later sections don't read, which this lets it write too,
    /// listed. It's how [`Document::save_linearized`] is told to reuse them
    /// too: it writes under the document's encryptor, as pdf-linearize's
    /// [`family::linearize`] writes under the one it's given, so neither
    /// reads [`family::Options::rc4`].
    pub fn reuse_rc4_keystreams(&mut self) {
        self.encryptor = self.encryptor.take().map(Encryptor::reusing_rc4_keystreams);
    }

    /// How [`Document::save`] writes it, for a caller to change one field and
    /// save with ([`Document::save_with`]): a cross-reference stream, what may
    /// go in an object stream in one, deflated by zlib-rs at level 6, on as
    /// many threads as the machine runs, and its header the document's own
    /// version, 1.7 for a new one, at least 1.5, which a cross-reference
    /// stream and object streams need (ISO 32000-2, 7.5.8); so a file of PDF
    /// 2.0 is written 2.0. A new encryption doesn't raise it
    /// ([`Document::encrypt`] says what that means). The rest as
    /// pdf-structure's [`write::Options::DEFAULT`].
    ///
    /// ```
    /// use pdfs::prelude::*;
    ///
    /// let mut doc = Document::new();
    /// doc.add_page(Rect::LETTER);
    /// // Saved as PDF 2.0, with every object at the top level.
    /// let mut options = doc.save_options();
    /// options.version = Version::new(2, 0);
    /// options.pack = None;
    /// let mut file = Vec::new();
    /// doc.save_with_to(&mut file, &options).unwrap();
    /// assert!(file.starts_with(b"%PDF-2.0"));
    /// assert_eq!(pdfs::from_bytes(file).unwrap().pages().unwrap().len(), 1);
    /// ```
    #[must_use]
    pub fn save_options(&self) -> write::Options<'static> {
        let mut options = write::Options::DEFAULT;
        options.kind = XrefKind::Stream;
        options.pack = Some(Pack::DEFAULT);
        options.deflate = deflate();
        options.version = self.doc.header_version().map_or(NEW, |v| v.max(STREAMS));
        options
    }

    /// How [`Document::save_linearized`] lays it out, for a caller to change:
    /// as [`Document::save_options`] writes it, cross-reference streams,
    /// object streams and zlib-rs, its header the document's version, at
    /// least 1.5; the first page where `/OpenAction` says; and under the
    /// caps an edited document's file was read under
    /// ([`Pdf::limits`](crate::Pdf::limits)), its page tree and name trees
    /// walked as [`Limits::linearize`] says, or each crate's defaults for a
    /// new one. The rest as pdf-linearize's [`family::Options::DEFAULT`].
    /// Its `rc4` isn't read: [`Document::reuse_rc4_keystreams`] says it.
    #[must_use]
    pub fn linearize_options(&self) -> family::Options<'static> {
        let mut options = family::Options::DEFAULT;
        options.kind = XrefKind::Stream;
        options.pack = true;
        options.deflate = deflate();
        options.limits = self.limits.linearizing();
        options
    }

    /// It, saved whole as [`Document::save_options`] says, at `path`:
    /// [`Document::save_with`].
    ///
    /// # Errors
    ///
    /// As [`Document::save_with`] fails.
    pub fn save(&mut self, path: impl AsRef<Path>) -> Result<Written, Error> {
        let options = self.save_options();
        self.save_with(path, &options)
    }

    /// It, saved whole as `options` say, at `path`, as
    /// [`Document::save_with_to`] writes it: in memory first, then over what
    /// the path holds, so a save refused leaves it as it was. It isn't a
    /// durable write: a failure as the path is written leaves it in part. The
    /// file a [`Pdf`](crate::Pdf) opened by its path reads by position isn't
    /// written over: a document edited from it, saved at its path, is refused,
    /// as that reading's later reads would read the new bytes at the old
    /// places. To change a file in place, read it into memory first,
    /// `pdfs::from_bytes(std::fs::read(path)?)`, whose `Pdf` holds its own
    /// bytes, or append an update to it ([`Document::save_update`]).
    ///
    /// ```
    /// use pdfs::prelude::*;
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path().join("table.pdf");
    /// let mut doc = Document::new();
    /// doc.add_page(Rect::LETTER);
    /// // Listed by a cross-reference table, nothing in an object stream.
    /// let mut options = doc.save_options();
    /// options.kind = pdfs::pdf_structure::XrefKind::Table;
    /// options.pack = None;
    /// doc.save_with(&path, &options).unwrap();
    /// let pdf = pdfs::open(&path).unwrap();
    /// assert!(pdf.reading().trailer().stream.is_none());
    /// ```
    ///
    /// # Errors
    ///
    /// As [`Document::save_with_to`] refuses, with nothing written; and
    /// [`Error::SameFile`] where `path` is the file the document's reading
    /// reads by position, as it is, and [`Error::Io`] where what's at `path`
    /// doesn't say whether it is, or doesn't take what's written.
    pub fn save_with(
        &mut self,
        path: impl AsRef<Path>,
        options: &write::Options<'_>,
    ) -> Result<Written, Error> {
        let mut out = Vec::new();
        let written = self.save_with_to(&mut out, options)?;
        self.write_over(path.as_ref(), &out)?;
        Ok(written)
    }

    /// It, saved whole as [`Document::save_options`] says, at the end of
    /// `out`: [`Document::save_with_to`].
    ///
    /// # Errors
    ///
    /// As [`Document::save_with_to`] refuses.
    pub fn save_to(&mut self, out: &mut Vec<u8>) -> Result<Written, Error> {
        let options = self.save_options();
        self.save_with_to(out, &options)
    }

    /// It, saved whole as [`Document::save_options`] says: the file's bytes.
    ///
    /// # Errors
    ///
    /// As [`Document::save_with_to`] refuses.
    pub fn to_bytes(&mut self) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        self.save_to(&mut out)?;
        Ok(out)
    }

    /// It, saved whole as `options` say, at the end of `out`: the pages added
    /// written in first, then every object through pdf-structure's
    /// [`write::document`], encrypted as the document is, an edited one's
    /// objects as read copied from its file where they copy. What it wrote
    /// is said ([`Written`]). Saved under a version of PDF 2.0, it can go
    /// without two things PDF 2.0 asks for and earlier versions don't: a
    /// standard 14 font's widths, where a page added draws in one
    /// ([`Std14::dict`](pdf_fonts::Std14::dict) has none), and the trailer's
    /// `/ID` (ISO 32000-2, 7.5.5), which pdf-structure's writer gives a file
    /// only where it's encrypted or had one. Under a new encryption
    /// ([`Document::encrypt`]), its encryption dictionary has `/Length 256`,
    /// which PDF 2.0 deprecates and gives only for `/V` 2 and 3 (ISO 32000-2,
    /// 7.6.2).
    ///
    /// # Errors
    ///
    /// Refused, with nothing written: [`Error::PageTree`] where a new
    /// document has no page, or more than pdf-semantics' default cap on a
    /// tree; [`Error::NoCatalog`] and [`Error::NoPageTree`] where an edited
    /// one has no page tree to add a page to, or one whose `/Count` can't
    /// count the pages added; and [`Error::Write`] where
    /// pdf-structure's writer refuses, its file changed since it was read
    /// among it ([`write::Error::Changed`]).
    pub fn save_with_to(
        &mut self,
        out: &mut Vec<u8>,
        options: &write::Options<'_>,
    ) -> Result<Written, Error> {
        phase!(DEBUG, "save", pages = self.added.len());
        self.place(options.deflate)?;
        let from = self.from();
        Ok(write::document(
            out,
            &self.doc,
            self.encryptor.as_ref(),
            from,
            options,
        )?)
    }

    /// It, linearized as `options` say, at `path`, as
    /// [`Document::save_linearized_to`] writes it: in memory first, as
    /// [`Document::save_with`] writes a file.
    ///
    /// # Errors
    ///
    /// As [`Document::save_linearized_to`] refuses, with nothing written, and
    /// as [`Document::save_with`] fails at its path.
    pub fn save_linearized(
        &mut self,
        path: impl AsRef<Path>,
        options: &family::Options<'_>,
    ) -> Result<Linearized, Error> {
        let mut out = Vec::new();
        let linearized = self.save_linearized_to(&mut out, options)?;
        self.write_over(path.as_ref(), &out)?;
        Ok(linearized)
    }

    /// It, linearized as `options` say, at the end of `out`, through
    /// pdf-linearize's [`family::linearize`]: the pages added written in
    /// first, then every object it reaches laid out for reading over a
    /// network, encrypted as the document is. An edited RC4 file's own
    /// encryption puts no new plaintext under a keystream the file used
    /// unless [`Document::reuse_rc4_keystreams`] said so, and then lists
    /// what it so wrote ([`Linearized::keystreams_reused`]); as
    /// [`family::linearize`] writes under the encryptor it's given, it
    /// doesn't read `options.rc4`, which is [`family::linearize_bytes`]'.
    ///
    /// # Errors
    ///
    /// Refused, with nothing written, as a whole save is
    /// ([`Document::save_with_to`]) for its pages, and as
    /// [`family::linearize`] refuses ([`Error::Linearize`]), a string or
    /// stream at a keystream an RC4 file used among it.
    pub fn save_linearized_to(
        &mut self,
        out: &mut Vec<u8>,
        options: &family::Options<'_>,
    ) -> Result<Linearized, Error> {
        phase!(DEBUG, "save_linearized", pages = self.added.len());
        self.place(options.deflate)?;
        let from = self.from();
        let encryptor = self.encryptor.as_ref();
        Ok(family::linearize(out, &self.doc, encryptor, from, options)?)
    }

    /// What it changed since it was read, appended as an update to the file
    /// at `path` (ISO 32000-2, 7.5.6), as [`Document::save_update_to`] writes
    /// it: only where that file holds the file read, as long as it was and
    /// its last `startxref` the same (pdf-structure's [`Base::holds`]), so
    /// the file read itself, or a copy of it made first
    /// (`std::fs::copy(read, path)`), and never a file that changed since.
    /// None, and nothing written, where nothing changed. It isn't a durable
    /// write: a failure as it's appended leaves part of an update after the
    /// file's last `%%EOF`, which a reader reads past; and what keeps another
    /// writer off the file between its check and the append is the caller's.
    /// The file read then holds the update, so the document's base isn't it:
    /// an update after this one is made by reading the file again. Appended to
    /// the file a [`Pdf`](crate::Pdf) opened by its path reads by position, it
    /// changes that file under its reading, which reads on as it was where it
    /// was, but which no save copies from after
    /// ([`write::Error::Changed`]). Only on Unix and Windows, where a file is
    /// read by position.
    ///
    /// # Errors
    ///
    /// Refused with nothing written, as [`Document::save_update_to`] refuses,
    /// and [`Error::NotTheBase`] where the file at `path` doesn't hold the
    /// file read; [`Error::Io`] where it doesn't open, or doesn't take what's
    /// appended.
    #[cfg(any(unix, windows))]
    pub fn save_update(&mut self, path: impl AsRef<Path>) -> Result<Option<Written>, Error> {
        use std::io::Write as _;

        let base = self.base()?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .append(true)
            .open(path)?;
        let there = pdf_structure::read::FileBytes::new(file.try_clone()?)?;
        if !base.holds(&there)? {
            return Err(Error::NotTheBase);
        }
        let mut out = Vec::new();
        let Some(written) = self.save_update_to(&mut out)? else {
            return Ok(None);
        };
        (&file).write_all(&out)?;
        Ok(Some(written))
    }

    /// What it changed since it was read, as an update after the file read,
    /// at the end of `out`, through pdf-structure's [`write::update`]: the
    /// pages added written in first, then each object built or changed since,
    /// what's as read passed over as the file holds it, encrypted as the file
    /// is, a section listing only those whose `/Prev` is the file's last; so
    /// `out` is the update alone, which goes after the file read, or a copy of
    /// it. None, and nothing written, where nothing changed. Its positions
    /// count from the file's start, as an update's do, wherever it starts in
    /// `out`.
    ///
    /// # Errors
    ///
    /// Refused, with nothing written: [`Error::NotRead`] for a document no
    /// file was read for; [`Error::Write`] where its file has no base an
    /// update goes after (one read by scanning, [`write::Error::Unsound`]),
    /// and where pdf-structure's writer refuses the update: under an
    /// encryption that isn't the file's ([`write::Error::Unmatched`]), as
    /// after [`Document::encrypt`], and under the file's own RC4 encryption a
    /// string or stream that would be new at a keystream the file used,
    /// unless [`Document::reuse_rc4_keystreams`] said so; and as a whole save
    /// is ([`Document::save_with_to`]) for its pages.
    pub fn save_update_to(&mut self, out: &mut Vec<u8>) -> Result<Option<Written>, Error> {
        phase!(DEBUG, "save_update", pages = self.added.len());
        let base = self.base()?;
        let options = self.save_options();
        self.place(options.deflate)?;
        let bodies = write::bodies(&self.doc);
        let from = self.from();
        let encryptor = self.encryptor.as_ref();
        Ok(write::update(
            out, &base, bodies, encryptor, from, &options,
        )?)
    }

    /// It, each save after this under `encryptor`: the fuzz body's new
    /// encryption, which the test writer draws from its input.
    #[cfg(any(test, fuzzing))]
    pub(crate) fn encrypted_by(&mut self, encryptor: Encryptor) {
        self.encryptor = Some(encryptor);
    }

    /// What its objects as read are copied from: the reading of an edited
    /// document's file.
    fn from(&self) -> Option<&'p dyn ReadFrom> {
        self.read.as_ref().map(|read| read.from)
    }

    /// The file an update goes after.
    fn base(&self) -> Result<Base, Error> {
        let read = self.read.as_ref().ok_or(Error::NotRead)?;
        read.base.clone().map_err(Error::Write)
    }

    /// `out` written over what `path` holds, unless that's the file the
    /// document's reading reads by position. A file changed in place since
    /// it was read isn't that file by its [`Identity`], but a save of the
    /// document has refused it before ([`write::Error::Changed`]), as its
    /// reading asks its source whether it changed.
    fn write_over(&self, path: &Path, out: &[u8]) -> Result<(), Error> {
        let read = self.read.as_ref().and_then(|read| read.identity.as_ref());
        if let Some(identity) = read {
            match Identity::of(path) {
                Ok(there) if there == *identity => return Err(Error::SameFile),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(Error::Io(error)),
            }
        }
        std::fs::write(path, out)?;
        Ok(())
    }

    /// The pages added written in, as their content deflated by `deflate`:
    /// a new document's page tree written anew over every page, or an edited
    /// one's added as kids of its page tree's root. Each page is written in
    /// alone, so one refused leaves those before it in and the rest added.
    fn place(&mut self, deflate: Deflate) -> Result<(), Error> {
        match self.read {
            None => self.place_new(deflate),
            Some(_) => self.place_edited(deflate),
        }
    }

    /// A new document's pages written in, and its page tree written anew
    /// over them all where one was added.
    fn place_new(&mut self, deflate: Deflate) -> Result<(), Error> {
        if self.placed.is_empty() && self.added.is_empty() {
            return Err(Error::PageTree(pages::WriteError::NoPages));
        }
        while let Some(added) = self.added.front() {
            let (doc, written) = (&mut self.doc, &mut self.written);
            let page = build::page(doc, written, added.media_box, &added.writer, deflate)?;
            self.added.pop_front();
            self.placed.push(self.doc.add(page));
            self.stale = true;
        }
        if !self.stale {
            return Ok(());
        }
        let doc = &mut self.doc;
        let mut old = self.nodes.iter().copied();
        let limits = pdf_semantics::Limits::DEFAULT;
        let ids = || old.next().unwrap_or_else(|| doc.new_id());
        let tree = pages::write(&self.placed, pages::FANOUT, &limits, ids)?;
        let numbered: Vec<ObjectId> = tree.nodes.iter().map(|(id, _)| *id).collect();
        for &id in &self.nodes {
            if !numbered.contains(&id) {
                self.doc.remove(id);
            }
        }
        for (id, node) in tree.nodes {
            self.doc.set(id, node);
        }
        for (&page, &parent) in self.placed.iter().zip(&tree.parents) {
            if let Some(page) = self.doc.get_dict_mut(page) {
                page.set("Parent", parent);
            }
        }
        if let Some(catalog) = self.doc.catalog_mut() {
            catalog.set("Pages", tree.root);
        }
        self.nodes = numbered;
        self.stale = false;
        Ok(())
    }

    /// An edited document's pages written in, each a kid of its page tree's
    /// root after those it has, its `/Count` one more each, and with every
    /// attribute a page inherits its own. A `/Count` that can't count them
    /// all is refused before any is written in.
    fn place_edited(&mut self, deflate: Deflate) -> Result<(), Error> {
        if self.added.is_empty() {
            return Ok(());
        }
        let (root, mut count, kids) = root(&self.doc)?;
        let added = i64::try_from(self.added.len()).ok();
        if added.and_then(|added| count.checked_add(added)).is_none() {
            return Err(Error::NoPageTree);
        }
        let node = self.doc.get_dict_mut(root).ok_or(Error::NoPageTree)?;
        node.set("Kids", kids);
        while let Some(added) = self.added.front() {
            let (doc, written) = (&mut self.doc, &mut self.written);
            let mut page = build::page(doc, written, added.media_box, &added.writer, deflate)?;
            page.set("Parent", root);
            page.set("CropBox", build::rect(added.media_box));
            page.set("Rotate", 0);
            self.added.pop_front();
            let page = self.doc.add(page);
            count += 1;
            let node = self.doc.get_dict_mut(root).ok_or(Error::NoPageTree)?;
            if let Some(kids) = node.get_mut(b"Kids").and_then(Object::as_array_mut) {
                kids.push(Object::from(page));
            }
            node.set("Count", count);
        }
        Ok(())
    }
}

/// The deflation a save does unless it's told otherwise: zlib-rs at level 6,
/// as PDFium's zlib inflates what it reads.
const fn deflate() -> Deflate {
    let mut deflate = Deflate::DEFAULT;
    deflate.backend = Backend::ZlibRs;
    deflate.level = Level::DEFAULT;
    deflate
}

/// `doc`'s page tree's root, to add a page to: its number, its `/Count` and
/// its kids, each read through a reference where it's one. Its catalog's
/// `/Pages` names it by reference, as a page's `/Parent` must (ISO 32000-2,
/// 7.7.3.3), a dictionary that isn't a page, whose `/Count` is an integer and
/// whose `/Kids`, where it has one, an array. Its `/Count` is any integer the
/// file holds, so the pages added are counted onto it checked.
fn root(doc: &pdf_structure::Document) -> Result<(ObjectId, i64, Vec<Object>), Error> {
    let catalog = doc.catalog().ok_or(Error::NoCatalog)?;
    let named = catalog
        .get(b"Pages")
        .and_then(|pages| doc.dereference(pages));
    let Some((Some(root), node)) = named else {
        return Err(Error::NoPageTree);
    };
    let node = node.as_dict().ok_or(Error::NoPageTree)?;
    if node.type_name() == Some(b"Page") {
        return Err(Error::NoPageTree);
    }
    let direct = |key: &[u8]| node.get(key).and_then(|value| doc.dereference(value));
    let count = direct(b"Count").and_then(|(_, count)| count.as_integer());
    let count = count.ok_or(Error::NoPageTree)?;
    let kids = match direct(b"Kids") {
        None if !node.has(b"Kids") => Vec::new(),
        kids => kids
            .and_then(|(_, kids)| kids.as_array())
            .ok_or(Error::NoPageTree)?
            .to_vec(),
    };
    Ok((root, count, kids))
}
