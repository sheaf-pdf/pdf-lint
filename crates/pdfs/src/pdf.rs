//! Open PDF handle (`Pdf`) and page inspection (`Pages`, `Page`).

use std::borrow::Cow;
use std::fmt;
use std::iter::FusedIterator;
use std::ops::ControlFlow;
use std::sync::OnceLock;

use pdf_crypt::{Encoded, Refused};
use pdf_fonts::{Fonts, Shown};
use pdf_linearize::check::{self, Verdict};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::version::Version;
use pdf_objects::{Dict, ObjectId, Streams};
use pdf_semantics::info::Info;
use pdf_semantics::pages;
use pdf_semantics::xmp::Claims;
use pdf_structure::Encryption;
use pdf_structure::read::{self, ByteSource, Identity, Reading};
use pdf_structure::write::{Base, Encryptor, ReadFrom};

use crate::document::Document;
use crate::error::Error;
use crate::file;
use crate::limits::Limits;
use crate::open::Options;
use crate::page::{self, Content, NamedFont};
use crate::phases::phase;

/// An open PDF file with lazy, thread-safe object and font caching.
///
/// Created via [`open`](crate::open), [`from_bytes`](crate::from_bytes), or
/// [`Options`]. Indirect objects and fonts are parsed on first access and cached
/// across threads on top of [`pdf_structure::Reading`]. Use [`Pdf::pages`] to
/// inspect pages and extract text, [`Pdf::edit`] to modify the file, or
/// [`Pdf::reading`] to access the underlying low-level reader directly.
///
/// `Pdf` is `Send + Sync`, and its `Debug` output reports only metadata (byte
/// length, encryption status, and reader configuration), never document content.
pub struct Pdf<S: ByteSource = Vec<u8>> {
    reading: Reading<S>,
    limits: Limits,
    rules: Rules,
    #[cfg(not(feature = "raster"))]
    fonts: Fonts,
    #[cfg(feature = "raster")]
    cache: pdf_raster::RenderCache,
    #[cfg(feature = "raster")]
    fallback: Option<std::sync::Arc<dyn pdf_raster::FontFallback>>,
    pages_cache: OnceLock<pages::PageTable>,
    identity: Option<Identity>,
}

impl<S: ByteSource> fmt::Debug for Pdf<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pdf")
            .field("bytes", &self.reading.size())
            .field("encryption", self.reading.encryption())
            .field("limits", &self.limits)
            .field("rules", &self.rules)
            .finish_non_exhaustive()
    }
}

impl<S: ByteSource> Pdf<S> {
    /// `reading`, opened under `options`, the file `identity` names.
    pub(crate) fn new(
        reading: Reading<S>,
        options: &Options<'_>,
        identity: Option<Identity>,
    ) -> Pdf<S> {
        Pdf {
            reading,
            limits: options.limits,
            rules: options.rules,
            #[cfg(not(feature = "raster"))]
            fonts: Fonts::default(),
            #[cfg(feature = "raster")]
            cache: pdf_raster::RenderCache::default(),
            #[cfg(feature = "raster")]
            fallback: options.font_fallback.cloned(),
            pages_cache: OnceLock::new(),
            identity,
        }
    }

    /// Attaches a shared [`FontFallback`](pdf_raster::FontFallback) provider
    /// (`raster` feature) used by [`Page::render`] and [`pdf_raster::RenderedDoc`].
    #[cfg(feature = "raster")]
    #[must_use]
    pub fn with_font_fallback(
        mut self,
        fallback: std::sync::Arc<dyn pdf_raster::FontFallback>,
    ) -> Self {
        self.fallback = Some(fallback);
        self
    }

    /// The shared [`FontFallback`](pdf_raster::FontFallback) provider attached
    /// to this [`Pdf`], if any (`raster` feature).
    #[cfg(feature = "raster")]
    #[must_use]
    pub fn font_fallback(&self) -> Option<&dyn pdf_raster::FontFallback> {
        self.fallback.as_deref()
    }

    /// The reading, for what the facade doesn't do: each object, a stream's
    /// data as written or decoded every way the reader says it, the file's
    /// revisions, and its document read whole.
    #[must_use]
    pub const fn reading(&self) -> &Reading<S> {
        &self.reading
    }

    /// The reading, taken.
    #[must_use]
    pub fn into_reading(self) -> Reading<S> {
        self.reading
    }

    /// The caps it's read under.
    #[must_use]
    pub const fn limits(&self) -> &Limits {
        &self.limits
    }

    /// The rules its page tree is walked by and its text decoded by.
    #[must_use]
    pub const fn rules(&self) -> Rules {
        self.rules
    }

    /// Its fonts, each read once and kept for every page that draws in it
    /// ([`Page::fonts`], [`Page::text`]), its program read where the build has
    /// the `programs` feature, as PDFium reads one, for the text its encoding
    /// leaves to it; for pdf-fonts' calls over [`Pdf::reading`] too.
    #[must_use]
    pub const fn fonts(&self) -> &Fonts {
        #[cfg(feature = "raster")]
        {
            self.cache.fonts()
        }
        #[cfg(not(feature = "raster"))]
        {
            &self.fonts
        }
    }

    /// Its shared cross-page [`RenderCache`](pdf_raster::RenderCache) (`raster`
    /// feature), wrapping [`Self::fonts`] alongside cached fallback font
    /// programs for [`Page::render`] and [`pdf_raster`] calls.
    #[cfg(feature = "raster")]
    #[must_use]
    pub const fn render_cache(&self) -> &pdf_raster::RenderCache {
        &self.cache
    }

    /// The file it reads, where [`open`](crate::open) opened it by its path:
    /// what tells, before writing to a path, whether it's this file
    /// (pdf-structure's [`Identity::of`]). None for one opened from a source
    /// ([`from_bytes`](crate::from_bytes)).
    #[must_use]
    pub const fn identity(&self) -> Option<&Identity> {
        self.identity.as_ref()
    }

    /// Whether the file is encrypted, and if it is, whether a password
    /// opened it, which, and what the file grants
    /// ([`Encryption::Opened`]'s `permissions`).
    #[must_use]
    pub const fn encryption(&self) -> &Encryption {
        self.reading.encryption()
    }

    /// It, an encrypted file no password opened, opened by `passwords`, each
    /// a password's bytes in an encoding, as a prompt gives them (pdf-crypt's
    /// [`Encoded`]; `Password(text).encodings()` gives a password's), as the
    /// reading opens it ([`Reading::unlock`]):
    /// [`Encryption::Opened`]'s `by` is then the index among `passwords` of
    /// the one that opened it. Nothing keeps them. What was read before is
    /// read again, decrypted, the fonts among it.
    ///
    /// ```
    /// use pdfs::prelude::*;
    ///
    /// // A file of one page, encrypted by a password.
    /// let password = Password("secret").prepared().unwrap();
    /// let mut doc = Document::new();
    /// doc.add_page(Rect::LETTER);
    /// doc.encrypt(&Protection::new(&password)).unwrap();
    /// let file = doc.to_bytes().unwrap();
    ///
    /// let pdf = pdfs::from_bytes(file).unwrap();
    /// assert_eq!(*pdf.encryption(), Encryption::Locked);
    /// let pdf = pdf.unlock(&Password("secret").encodings()).unwrap();
    /// assert!(matches!(pdf.encryption(), Encryption::Opened { .. }));
    /// assert_eq!(pdf.pages().unwrap().len(), 1);
    /// ```
    ///
    /// # Errors
    ///
    /// Given back as it was, with why, where none of `passwords` opens it or
    /// its security handler is one the reader can't open: as
    /// [`Reading::unlock`] refuses. A file that isn't encrypted, or that a
    /// password opened, is given back as it is.
    // The refusal holds the file, as the file opened is one: a box would
    // make neither side smaller.
    #[allow(clippy::result_large_err)]
    pub fn unlock(self, passwords: &[Encoded]) -> Result<Pdf<S>, (Pdf<S>, Refused)> {
        let Pdf {
            reading,
            limits,
            rules,
            #[cfg(not(feature = "raster"))]
            fonts,
            #[cfg(feature = "raster")]
            cache,
            #[cfg(feature = "raster")]
            fallback,
            pages_cache,
            identity,
        } = self;
        match reading.unlock(passwords) {
            Ok(reading) => Ok(Pdf {
                reading,
                limits,
                rules,
                #[cfg(not(feature = "raster"))]
                fonts: Fonts::default(),
                #[cfg(feature = "raster")]
                cache: pdf_raster::RenderCache::default(),
                #[cfg(feature = "raster")]
                fallback,
                pages_cache: OnceLock::new(),
                identity,
            }),
            Err((reading, refused)) => Err((
                Pdf {
                    reading,
                    limits,
                    rules,
                    #[cfg(not(feature = "raster"))]
                    fonts,
                    #[cfg(feature = "raster")]
                    cache,
                    #[cfg(feature = "raster")]
                    fallback,
                    pages_cache,
                    identity,
                },
                refused,
            )),
        }
    }

    /// Its version: the later, by number, of its header's and its catalog's
    /// `/Version` (ISO 32000-2, 7.7.2), as [`file::version`] reads it.
    ///
    /// # Errors
    ///
    /// [`Error::Unreadable`] where the catalog doesn't read for the reader's
    /// own reason: past a cap, behind a password, or not given by the
    /// source.
    pub fn version(&self) -> Result<Option<Version>, Error> {
        Ok(file::version(&self.reading)?)
    }

    /// Its catalog, the dictionary its trailer's `/Root` names (ISO 32000-2,
    /// 7.7.2).
    ///
    /// # Errors
    ///
    /// [`Error::NoCatalog`] where `/Root` names no dictionary;
    /// [`Error::Unreadable`] where the catalog doesn't read.
    pub fn catalog(&self) -> Result<&Dict, Error> {
        let trailer = self.reading.trailer().dict();
        pdf_semantics::catalog(&self.reading, trailer)?.ok_or(Error::NoCatalog)
    }

    #[cfg(any(feature = "images", feature = "raster"))]
    pub(crate) fn optional_catalog(&self) -> Result<Option<&Dict>, Error> {
        match self.catalog() {
            Ok(dict) => Ok(Some(dict)),
            Err(Error::NoCatalog) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// What it says of itself, its document information dictionary
    /// (ISO 32000-2, 14.3.3), read typed, its text decoded by its rules;
    /// none where its trailer has none. Nothing departing from the clause is
    /// told: [`Pdf::info_with`] tells it.
    ///
    /// ```
    /// use pdfs::pdf_objects::dict;
    /// use pdfs::prelude::*;
    ///
    /// // A file of one page whose trailer names a title.
    /// let mut doc = Document::new();
    /// doc.add_page(Rect::LETTER);
    /// let info = doc.structure_mut().add(dict! { "Title" => Object::text("Minutes") });
    /// doc.structure_mut().trailer_mut().dict_mut().set("Info", info);
    /// let pdf = pdfs::from_bytes(doc.to_bytes().unwrap()).unwrap();
    ///
    /// let info = pdf.info().unwrap().unwrap();
    /// assert_eq!(info.title.as_deref(), Some("Minutes"));
    /// assert_eq!(info.author, None);
    /// ```
    ///
    /// # Errors
    ///
    /// As [`file::info`] fails, an [`Error::Unreadable`].
    pub fn info(&self) -> Result<Option<Info>, Error> {
        self.info_with(&mut ())
    }

    /// [`Pdf::info`], what departs from the clause told to `sink`
    /// (pdf-semantics' [`Fault`](pdf_semantics::Fault)s).
    ///
    /// # Errors
    ///
    /// As [`Pdf::info`] fails.
    pub fn info_with<K: Sink<pdf_semantics::Fault> + ?Sized>(
        &self,
        sink: &mut K,
    ) -> Result<Option<Info>, Error> {
        Ok(file::info(&self.reading, self.rules, sink)?)
    }

    /// What it claims to conform to, PDF/A, PDF/UA or PDF/VT, by its
    /// metadata stream's XMP packet, as [`file::claims`] reads it; none
    /// where its catalog has no metadata stream. A packet that isn't UTF-8 is
    /// read as one that claims nothing: [`Pdf::claims_with`] is told it
    /// isn't.
    ///
    /// ```
    /// use pdfs::prelude::*;
    ///
    /// // A file of one page that claims PDF/A-2b.
    /// let mut claims = Claims::default();
    /// claims.pdfa_part = Some("2".into());
    /// claims.pdfa_conformance = Some("B".into());
    /// let mut doc = Document::new();
    /// doc.add_page(Rect::LETTER);
    /// doc.claim(&claims).unwrap();
    /// let pdf = pdfs::from_bytes(doc.to_bytes().unwrap()).unwrap();
    ///
    /// assert_eq!(pdf.claims().unwrap(), Some(claims));
    /// ```
    ///
    /// # Errors
    ///
    /// As [`file::claims`] fails.
    pub fn claims(&self) -> Result<Option<Claims>, Error> {
        self.claims_with(&mut ())
    }

    /// [`Pdf::claims`], what departs from XMP told to `sink`.
    ///
    /// # Errors
    ///
    /// As [`Pdf::claims`] fails.
    pub fn claims_with<K: Sink<pdf_semantics::Fault> + ?Sized>(
        &self,
        sink: &mut K,
    ) -> Result<Option<Claims>, Error> {
        file::claims(&self.reading, &self.limits.semantics, sink)
    }

    /// Reads the catalog's `/OpenAction` entry (ISO 32000-2, 7.7.2, Table 29),
    /// if present, without collecting non-fatal faults.
    ///
    /// # Errors
    ///
    /// As [`file::open_action`] fails.
    pub fn open_action(&self) -> Result<Option<pdf_semantics::action::OpenAction>, Error> {
        self.open_action_with(&mut ())
    }

    /// [`Pdf::open_action`], reporting recoverable ISO 32000-2 departures to
    /// `sink`.
    ///
    /// # Errors
    ///
    /// As [`Pdf::open_action`] fails.
    pub fn open_action_with<K: Sink<pdf_semantics::Fault> + ?Sized>(
        &self,
        sink: &mut K,
    ) -> Result<Option<pdf_semantics::action::OpenAction>, Error> {
        file::open_action(&self.reading, self.rules, &self.limits.semantics, sink)
    }

    /// Reads standard Dublin Core, XMP Basic, Adobe PDF, and conformance
    /// metadata from the catalog's `/Metadata` stream ([`file::metadata`]).
    ///
    /// # Errors
    ///
    /// As [`file::metadata`] fails.
    pub fn metadata(&self) -> Result<Option<pdf_semantics::xmp::Metadata>, Error> {
        self.metadata_with(&mut ())
    }

    /// [`Pdf::metadata`], reporting recoverable XMP departures to `sink`.
    ///
    /// # Errors
    ///
    /// As [`Pdf::metadata`] fails.
    pub fn metadata_with<K: Sink<pdf_semantics::Fault> + ?Sized>(
        &self,
        sink: &mut K,
    ) -> Result<Option<pdf_semantics::xmp::Metadata>, Error> {
        file::metadata(&self.reading, &self.limits.semantics, sink)
    }

    /// Reads the document outline (`/Outlines`) hierarchy ([`file::outlines`]).
    ///
    /// # Errors
    ///
    /// As [`file::outlines`] fails.
    pub fn outlines(&self) -> Result<Option<Vec<pdf_semantics::outlines::OutlineItem>>, Error> {
        self.outlines_with(&mut ())
    }

    /// [`Pdf::outlines`], reporting recoverable ISO 32000-2 departures to `sink`.
    ///
    /// # Errors
    ///
    /// As [`Pdf::outlines`] fails.
    pub fn outlines_with<K: Sink<pdf_semantics::Fault> + ?Sized>(
        &self,
        sink: &mut K,
    ) -> Result<Option<Vec<pdf_semantics::outlines::OutlineItem>>, Error> {
        file::outlines(&self.reading, self.rules, &self.limits.semantics, sink)
    }

    /// Reads the catalog's `/PageLabels` table ([`file::page_labels`]).
    ///
    /// # Errors
    ///
    /// As [`file::page_labels`] fails.
    pub fn page_labels(&self) -> Result<Option<pdf_semantics::labels::PageLabels>, Error> {
        self.page_labels_with(&mut ())
    }

    /// [`Pdf::page_labels`], reporting recoverable ISO 32000-2 departures to `sink`.
    ///
    /// # Errors
    ///
    /// As [`Pdf::page_labels`] fails.
    pub fn page_labels_with<K: Sink<pdf_semantics::Fault> + ?Sized>(
        &self,
        sink: &mut K,
    ) -> Result<Option<pdf_semantics::labels::PageLabels>, Error> {
        file::page_labels(&self.reading, self.rules, &self.limits.semantics, sink)
    }

    /// Reads embedded file attachments from `/Names /EmbeddedFiles`
    /// ([`file::embedded_files`]).
    ///
    /// # Errors
    ///
    /// As [`file::embedded_files`] fails.
    pub fn embedded_files(&self) -> Result<Vec<pdf_semantics::files::FileSpec>, Error> {
        self.embedded_files_with(&mut ())
    }

    /// [`Pdf::embedded_files`], reporting recoverable ISO 32000-2 departures to `sink`.
    ///
    /// # Errors
    ///
    /// As [`Pdf::embedded_files`] fails.
    pub fn embedded_files_with<K: Sink<pdf_semantics::Fault> + ?Sized>(
        &self,
        sink: &mut K,
    ) -> Result<Vec<pdf_semantics::files::FileSpec>, Error> {
        file::embedded_files(&self.reading, self.rules, &self.limits.semantics, sink)
    }

    /// Reads catalog-level `/AF` Associated Files ([`file::associated_files`]).
    ///
    /// # Errors
    ///
    /// As [`file::associated_files`] fails.
    pub fn associated_files(&self) -> Result<Vec<pdf_semantics::files::FileSpec>, Error> {
        self.associated_files_with(&mut ())
    }

    /// [`Pdf::associated_files`], reporting recoverable ISO 32000-2 departures to `sink`.
    ///
    /// # Errors
    ///
    /// As [`Pdf::associated_files`] fails.
    pub fn associated_files_with<K: Sink<pdf_semantics::Fault> + ?Sized>(
        &self,
        sink: &mut K,
    ) -> Result<Vec<pdf_semantics::files::FileSpec>, Error> {
        file::associated_files(&self.reading, self.rules, sink)
    }

    /// Reads optional content configuration (`/OCProperties`, [`file::optional_content`]).
    ///
    /// # Errors
    ///
    /// As [`file::optional_content`] fails.
    pub fn optional_content(&self) -> Result<Option<pdf_semantics::ocg::OcProperties>, Error> {
        self.optional_content_with(&mut ())
    }

    /// [`Pdf::optional_content`], reporting recoverable ISO 32000-2 departures to `sink`.
    ///
    /// # Errors
    ///
    /// As [`Pdf::optional_content`] fails.
    pub fn optional_content_with<K: Sink<pdf_semantics::Fault> + ?Sized>(
        &self,
        sink: &mut K,
    ) -> Result<Option<pdf_semantics::ocg::OcProperties>, Error> {
        file::optional_content(&self.reading, self.rules, &self.limits.semantics, sink)
    }

    /// Reads catalog display settings (`/PageLayout`, `/PageMode`, `/Lang`, and
    /// `/ViewerPreferences`, [`file::catalog_view`]).
    ///
    /// # Errors
    ///
    /// As [`file::catalog_view`] fails.
    pub fn catalog_view(&self) -> Result<pdf_semantics::viewer::CatalogView, Error> {
        self.catalog_view_with(&mut ())
    }

    /// [`Pdf::catalog_view`], reporting recoverable ISO 32000-2 departures to `sink`.
    ///
    /// # Errors
    ///
    /// As [`Pdf::catalog_view`] fails.
    pub fn catalog_view_with<K: Sink<pdf_semantics::Fault> + ?Sized>(
        &self,
        sink: &mut K,
    ) -> Result<pdf_semantics::viewer::CatalogView, Error> {
        file::catalog_view(&self.reading, self.rules, sink)
    }

    pub(crate) fn cached_page_table(&self) -> Result<&pages::PageTable, Error> {
        if let Some(tbl) = self.pages_cache.get() {
            return Ok(tbl);
        }
        phase!(DEBUG, "pages", pdfium = matches!(self.rules, Rules::Pdfium));
        let catalog = self.catalog()?;
        let walked = pages::pages(
            &self.reading,
            catalog,
            self.rules,
            &self.limits.semantics,
            &mut (),
        )?;
        let tbl = pages::PageTable::from_pages(&self.reading, &walked);
        let _ = self.pages_cache.set(tbl);
        Ok(self.pages_cache.get().expect("just initialized"))
    }

    #[cfg(feature = "raster")]
    pub(crate) fn prime_pages(&self) -> Result<(), Error> {
        let _ = self.cached_page_table()?;
        Ok(())
    }

    /// Its pages, in order, as its page tree lists them by its rules, each
    /// with what it inherits and from where (pdf-semantics'
    /// [`pages::pages`]). Cached on first read; nothing departing from the
    /// clause is told: [`Pdf::pages_with`] tells it.
    ///
    /// # Errors
    ///
    /// [`Error::NoCatalog`] and [`Error::Unreadable`] as [`Pdf::catalog`]
    /// fails; [`Error::Pages`] where the tree doesn't read: past
    /// [`Limits::semantics`]' caps, by PDFium's rules a tree with no page or
    /// a loop, and an object on the way that doesn't read for the reader's
    /// own reason, never read as no pages.
    pub fn pages(&self) -> Result<Pages<'_, S>, Error> {
        let tbl = self.cached_page_table()?;
        Ok(Pages {
            table: tbl.bind(&self.reading),
            pdf: self,
        })
    }

    /// [`Pdf::pages`], what departs from the clause told to `sink`: by ISO's
    /// rules, everything; by PDFium's, only what doesn't read.
    ///
    /// # Errors
    ///
    /// As [`Pdf::pages`] fails.
    pub fn pages_with<K: Sink<pdf_semantics::Fault> + ?Sized>(
        &self,
        sink: &mut K,
    ) -> Result<Pages<'_, S>, Error> {
        if !sink.wants() {
            return self.pages();
        }
        phase!(DEBUG, "pages", pdfium = matches!(self.rules, Rules::Pdfium));
        let catalog = self.catalog()?;
        let walked = pages::pages(
            &self.reading,
            catalog,
            self.rules,
            &self.limits.semantics,
            sink,
        )?;
        if self.pages_cache.get().is_none() {
            let _ = self
                .pages_cache
                .set(pages::PageTable::from_pages(&self.reading, &walked));
        }
        Ok(Pages {
            table: walked,
            pdf: self,
        })
    }

    /// Stream `id`'s data, decrypted and decoded through its filters, as a
    /// viewer takes it (pdf-objects' [`Streams`]): what decodes of a stream
    /// that decodes only in part, up to [`Limits::read`]'s cap on a stream.
    ///
    /// ```
    /// use pdfs::prelude::*;
    ///
    /// // A page that fills a square, its content written deflated.
    /// let mut doc = Document::new();
    /// let page = doc.add_page(Rect::LETTER);
    /// page.rect(0.0, 0.0, 10.0, 10.0);
    /// page.paint(Paint::Fill);
    /// let pdf = pdfs::from_bytes(doc.to_bytes().unwrap()).unwrap();
    ///
    /// let pages = pdf.pages().unwrap();
    /// let page = pages.get(0).unwrap().attributes().dict.unwrap();
    /// let contents = page.get(b"Contents").and_then(Object::as_reference).unwrap();
    /// assert!(pdf.stream(contents).unwrap().starts_with(b"0 0 10 10 re"));
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::Unreadable`], its [`Why`](pdf_objects::Why) saying why: past
    /// the cap ([`Why::Capped`](pdf_objects::Why::Capped)), behind a
    /// password, through a filter the reader doesn't decode, not given by
    /// the source, and damaged, or not a stream.
    pub fn stream(&self, id: ObjectId) -> Result<Cow<'_, [u8]>, Error> {
        Ok(Streams::decoded(&self.reading, id, usize::MAX)?)
    }

    /// Whether it's linearized and its linearization holds, as
    /// [`file::linearized`] checks it. Nothing wrong with it is told:
    /// [`Pdf::linearized_with`] tells it.
    ///
    /// # Errors
    ///
    /// [`Error::Linearize`], only where the source doesn't give its bytes.
    pub fn linearized(&self) -> Result<Verdict, Error> {
        self.linearized_with(&mut ())
    }

    /// [`Pdf::linearized`], each finding told to `sink`, what wasn't judged
    /// among them.
    ///
    /// # Errors
    ///
    /// As [`Pdf::linearized`] fails.
    pub fn linearized_with<K: check::Sink + ?Sized>(&self, sink: &mut K) -> Result<Verdict, Error> {
        Ok(file::linearized(&self.reading, &self.limits, sink)?)
    }

    /// Its document, to edit and save ([`Document`]): every object its
    /// cross-references list read (pdf-structure's
    /// [`Reading::to_document`]), each as read, to be copied from the file
    /// where it isn't changed; under the file's own encryption, which the
    /// password that opened it opened ([`Encryptor::of`]), so an encrypted
    /// file is saved encrypted as it was unless [`Document::decrypt`] or
    /// [`Document::encrypt`] says otherwise; and after the file as its last
    /// section describes it ([`Base::of`]), for an update
    /// ([`Document::save_update`]); and under the caps it's read under, which
    /// a linearized save's walks of its trees are made under
    /// ([`Document::linearize_options`]). It borrows the reading, which what's
    /// as read is copied from.
    ///
    /// What the file permits isn't held to here: opened by its user's
    /// password, a file that withholds changes is edited and saved all the
    /// same, and [`Document::decrypt`] saves it in the clear. The encryption
    /// itself can't stop that, and ISO 32000-2 asks what processes a file to
    /// honour its permissions (7.6.4.1), which here is the caller, which
    /// reads [`Pdf::encryption`]'s `permissions`, and which password opened
    /// it (`by`), first.
    ///
    /// # Errors
    ///
    /// [`Error::Read`] where the source doesn't give its bytes, and
    /// [`Error::Write`] where it's encrypted and no password opened it
    /// ([`Locked`](pdf_structure::write::Error::Locked)), or its security
    /// handler is one the reader can't open: there's nothing to write its
    /// strings and streams from.
    pub fn edit(&self) -> Result<Document<'_>, Error> {
        phase!(DEBUG, "edit", bytes = self.reading.size());
        let doc = self.reading.to_document()?;
        let encryptor = Encryptor::of(&self.reading)?;
        let base = Base::of(&self.reading);
        let from: &dyn ReadFrom = &self.reading;
        let identity = self.identity.clone();
        let limits = self.limits;
        Ok(Document::edited(
            doc, encryptor, from, base, identity, limits,
        ))
    }

    /// What the reading found wrong as it read, each at its place, in the
    /// whole read's order (pdf-structure's [`Reading::faults`]): only of
    /// what it has read so far, and only where they're looked for
    /// ([`Options::faults`]); none by default.
    #[must_use]
    pub fn faults(&self) -> Vec<(Place, read::Fault)> {
        self.reading.faults()
    }
}

/// A file's pages, in order, as [`Pdf::pages`] lists them: pdf-semantics'
/// table ([`Pages::table`]), each page given with its file. Its `Debug` gives
/// how many there are.
pub struct Pages<'p, S: ByteSource> {
    table: pages::Pages<'p>,
    pdf: &'p Pdf<S>,
}

impl<S: ByteSource> fmt::Debug for Pages<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pages")
            .field("len", &self.table.len())
            .field("counted", &self.table.counted())
            .finish()
    }
}

impl<'p, S: ByteSource> Pages<'p, S> {
    /// How many pages the tree lists.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.table.len()
    }

    /// Whether it lists none, which only ISO's rules give: PDFium's make
    /// that [`Error::Pages`].
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// The count PDFium gives under its rules, which may be more than the
    /// tree lists ([`pages::Pages::counted`]).
    #[must_use]
    pub const fn counted(&self) -> usize {
        self.table.counted()
    }

    /// Page `index`, from 0; none past the last.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<Page<'_, S>> {
        let page = self.table.get(index)?;
        Some(Page {
            page,
            pdf: self.pdf,
        })
    }

    /// Each page, in order.
    pub fn iter(&self) -> PagesIter<'_, S> {
        PagesIter {
            pages: self.table.iter(),
            pdf: self.pdf,
        }
    }

    /// pdf-semantics' table of them.
    #[must_use]
    pub const fn table(&self) -> &pages::Pages<'p> {
        &self.table
    }

    /// pdf-semantics' table of them, taken.
    #[must_use]
    pub fn into_table(self) -> pages::Pages<'p> {
        self.table
    }

    /// Builds a [`DestTable`](pdf_semantics::dest::DestTable) indexing named and
    /// page-reference destinations across the document without collecting
    /// non-fatal faults.
    ///
    /// # Errors
    ///
    /// As [`file::dests`] fails.
    pub fn dests(&self) -> Result<pdf_semantics::dest::DestTable, Error> {
        self.dests_with(&mut ())
    }

    /// [`Pages::dests`], reporting recoverable ISO 32000-2 departures to
    /// `sink`.
    ///
    /// # Errors
    ///
    /// As [`Pages::dests`] fails.
    pub fn dests_with<K: Sink<pdf_semantics::Fault> + ?Sized>(
        &self,
        sink: &mut K,
    ) -> Result<pdf_semantics::dest::DestTable, Error> {
        file::dests(
            &self.pdf.reading,
            &self.table,
            self.pdf.rules,
            &self.pdf.limits.semantics,
            sink,
        )
    }
}

impl<'a, S: ByteSource> IntoIterator for &'a Pages<'_, S> {
    type Item = Page<'a, S>;
    type IntoIter = PagesIter<'a, S>;

    fn into_iter(self) -> PagesIter<'a, S> {
        self.iter()
    }
}

/// Each page of a file, in order: what [`Pages::iter`] gives. It knows how
/// many are left, and a `count`, `for_each` or `fold` over it goes through
/// them in one pass.
#[must_use = "an iterator is lazy: it gives nothing until asked"]
pub struct PagesIter<'a, S: ByteSource> {
    pages: std::slice::Iter<'a, pages::Page<'a>>,
    pdf: &'a Pdf<S>,
}

impl<S: ByteSource> fmt::Debug for PagesIter<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PagesIter")
            .field("left", &self.pages.len())
            .finish()
    }
}

impl<S: ByteSource> Clone for PagesIter<'_, S> {
    fn clone(&self) -> Self {
        PagesIter {
            pages: self.pages.clone(),
            pdf: self.pdf,
        }
    }
}

impl<'a, S: ByteSource> Iterator for PagesIter<'a, S> {
    type Item = Page<'a, S>;

    #[inline]
    fn next(&mut self) -> Option<Page<'a, S>> {
        let page = self.pages.next()?;
        Some(Page {
            page,
            pdf: self.pdf,
        })
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.pages.size_hint()
    }

    #[inline]
    fn nth(&mut self, n: usize) -> Option<Page<'a, S>> {
        let page = self.pages.nth(n)?;
        Some(Page {
            page,
            pdf: self.pdf,
        })
    }

    /// One pass over the table, as `count`, `for_each` and `last` fold.
    #[inline]
    fn fold<B, F: FnMut(B, Page<'a, S>) -> B>(self, init: B, mut f: F) -> B {
        let pdf = self.pdf;
        self.pages
            .fold(init, |acc, page| f(acc, Page { page, pdf }))
    }
}

impl<S: ByteSource> DoubleEndedIterator for PagesIter<'_, S> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        let page = self.pages.next_back()?;
        Some(Page {
            page,
            pdf: self.pdf,
        })
    }
}

impl<S: ByteSource> ExactSizeIterator for PagesIter<'_, S> {}

impl<S: ByteSource> FusedIterator for PagesIter<'_, S> {}

/// A page of a file: what its page tree says of it, each inheritable
/// attribute with where it came from ([`Page::attributes`]), its content
/// ([`Page::content`]), its fonts ([`Page::fonts`]) and its text
/// ([`Page::text`]). Its `Debug` gives its object, how often it was
/// listed before and its boxes, not its dictionary.
pub struct Page<'a, S: ByteSource> {
    page: &'a pages::Page<'a>,
    pdf: &'a Pdf<S>,
}

impl<S: ByteSource> Clone for Page<'_, S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S: ByteSource> Copy for Page<'_, S> {}

impl<S: ByteSource> fmt::Debug for Page<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Page")
            .field("id", &self.page.id)
            .field("repeat", &self.page.repeat)
            .field("media_box", &self.page.media_box())
            .field("crop_box", &self.page.crop_box())
            .field("rotate", &self.page.rotate())
            .finish_non_exhaustive()
    }
}

impl<'a, S: ByteSource> Page<'a, S> {
    /// What the page tree says of it (pdf-semantics' [`pages::Page`]): its
    /// object and dictionary, its boxes, rotation and resources, each its
    /// own or inherited, and from where.
    #[must_use]
    pub const fn attributes(&self) -> &'a pages::Page<'a> {
        self.page
    }

    /// Its 2D page geometry ([`pdf_gfx::PageGeom`]): normalized `/MediaBox`,
    /// visible `/CropBox`, `/Rotate`, `/UserUnit`, optional `/BleedBox` /
    /// `/TrimBox` / `/ArtBox`, and closed-form transforms between PageSpace and
    /// ViewSpace.
    #[must_use]
    pub fn geom(&self) -> pdf_gfx::PageGeom {
        page::geom(&self.pdf.reading, self.page)
    }

    /// Reads the page's `/Annots` array under the file's rules and
    /// [`Limits::annots`] without collecting non-fatal faults.
    ///
    /// # Errors
    ///
    /// As [`page::annots`](fn@page::annots) fails.
    pub fn annots(&self) -> Result<Vec<pdf_annots::Annot<'a>>, Error> {
        self.annots_with(&mut ())
    }

    /// [`Page::annots`], reporting recoverable ISO 32000-2 departures to `sink`
    /// ([`pdf_annots::Fault`]).
    ///
    /// # Errors
    ///
    /// As [`Page::annots`] fails.
    pub fn annots_with<K: Sink<pdf_annots::Fault> + ?Sized>(
        &self,
        sink: &mut K,
    ) -> Result<Vec<pdf_annots::Annot<'a>>, Error> {
        let pdf = self.pdf;
        page::annots_with(&pdf.reading, self.page, pdf.rules, &pdf.limits, sink)
    }

    /// Plans flattening of eligible annotations on this page according to
    /// `opts` ([`page::plan_flatten`](fn@page::plan_flatten)).
    ///
    /// `/Redact` marks stay on the page, undrawn, unless `opts` sets
    /// [`include_redactions`](pdf_annots::FlattenOpts::include_redactions):
    /// flattening never removes the content under a mark, so drawing one would
    /// only make the page look redacted.
    ///
    /// # Errors
    ///
    /// As [`page::plan_flatten`](fn@page::plan_flatten) fails.
    pub fn plan_flatten(
        &self,
        opts: pdf_annots::FlattenOpts,
    ) -> Result<pdf_annots::FlattenPlan, Error> {
        page::plan_flatten(&self.pdf.reading, self.page, opts, &self.pdf.limits)
    }

    /// Its content: its streams decoded and joined under
    /// [`Limits::content`], as [`page::content`](fn@page::content) joins
    /// them, and the color spaces its resources name. A stream whose data
    /// doesn't decode for the file's own reason is joined as none and said
    /// ([`Concat::undecoded`](pdf_content_streams::Concat::undecoded)), where
    /// PDFium reads its data as written.
    ///
    /// # Errors
    ///
    /// As [`page::content`](fn@page::content) fails: [`Error::Content`]
    /// where its streams are past the cap or one doesn't read for the
    /// reader's own reason (past a cap, behind a password or not given), its
    /// [`Why`](pdf_objects::Why) kept, never for the file's own damage; and
    /// [`Error::Unreadable`] where its resources don't read for it.
    pub fn content(&self) -> Result<Content, Error> {
        page::content(&self.pdf.reading, self.page, &self.pdf.limits.content)
    }

    /// The fonts its resources name, each by its name, read once a file and
    /// kept, as [`page::fonts`](fn@page::fonts) reads them, within
    /// [`Limits::font_budget`].
    ///
    /// ```
    /// use pdfs::prelude::*;
    ///
    /// // A page that shows a string in Courier.
    /// let mut doc = Document::new();
    /// let page = doc.add_page(Rect::LETTER);
    /// page.begin_text();
    /// page.font(standard_font(Std14::Courier), 12.0);
    /// page.show(&Std14::Courier.codes("Hi").unwrap());
    /// page.end_text();
    /// let pdf = pdfs::from_bytes(doc.to_bytes().unwrap()).unwrap();
    ///
    /// let pages = pdf.pages().unwrap();
    /// let fonts = pages.get(0).unwrap().fonts().unwrap();
    /// assert_eq!(fonts.len(), 1);
    /// assert!(fonts[0].font.as_ref().is_ok_and(Option::is_some));
    /// ```
    ///
    /// # Errors
    ///
    /// As [`page::fonts`](fn@page::fonts) fails: [`Error::Unreadable`] where
    /// its resources, or their `/Font`, don't read for the reader's own
    /// reason.
    pub fn fonts(&self) -> Result<Vec<NamedFont>, Error> {
        let pdf = self.pdf;
        let mut budget = pdf.limits.font_budget;
        page::fonts(
            &pdf.reading,
            pdf.fonts(),
            self.page,
            &pdf.limits,
            &mut budget,
        )
    }

    /// The strings its content shows, in order, each in its font, the forms
    /// it draws followed, as PDFium reads them for its text, with no layout,
    /// as [`page::text`](fn@page::text) reads them, within
    /// [`Limits::font_budget`]: [`Shown::text`] joins them. What isn't read
    /// is said in the answer ([`Shown::unread`]), a stream whose data doesn't
    /// decode among it.
    ///
    /// # Errors
    ///
    /// As [`Page::content`] fails.
    pub fn text(&self) -> Result<Shown, Error> {
        let pdf = self.pdf;
        let mut budget = pdf.limits.font_budget;
        page::text(
            &pdf.reading,
            pdf.fonts(),
            self.page,
            &pdf.limits,
            &mut budget,
        )
    }

    /// Interprets its content stream and drawn Form XObjects (`Do`) under
    /// [`Limits::gfx`], streaming placed paths, glyph origins/quads, images,
    /// shadings, clips, and marked content to `visit`
    /// ([`page::interpret`](fn@page::interpret)).
    ///
    /// # Errors
    ///
    /// As [`page::interpret`](fn@page::interpret) fails.
    pub fn interpret(
        &self,
        opts: pdf_gfx::Opts,
        visit: impl FnMut(pdf_gfx::Event<'_>) -> ControlFlow<()>,
    ) -> Result<pdf_gfx::Summary, Error> {
        self.interpret_with(opts, &mut (), visit)
    }

    /// [`Page::interpret`], reporting recoverable ISO 32000-2 departures to
    /// `sink` ([`pdf_gfx::Fault`]).
    ///
    /// # Errors
    ///
    /// As [`Page::interpret`] fails.
    pub fn interpret_with<K: Sink<pdf_gfx::Fault> + ?Sized>(
        &self,
        opts: pdf_gfx::Opts,
        sink: &mut K,
        visit: impl FnMut(pdf_gfx::Event<'_>) -> ControlFlow<()>,
    ) -> Result<pdf_gfx::Summary, Error> {
        let pdf = self.pdf;
        page::interpret_with(
            &pdf.reading,
            pdf.fonts(),
            self.page,
            &pdf.limits,
            opts,
            sink,
            visit,
        )
    }

    /// Re-encodes its content stream and every Form XObject drawn on it into
    /// position-independent `Tm` + `Td` text operations
    /// ([`page::encode`](fn@page::encode)).
    ///
    /// # Errors
    ///
    /// As [`page::encode`](fn@page::encode) fails.
    pub fn encode(&self) -> Result<pdf_gfx::EncodedPage, Error> {
        self.encode_with(&mut ())
    }

    /// [`Page::encode`], reporting recoverable ISO 32000-2 departures to `sink`
    /// ([`pdf_gfx::Fault`]).
    ///
    /// # Errors
    ///
    /// As [`Page::encode`] fails.
    pub fn encode_with<K: Sink<pdf_gfx::Fault> + ?Sized>(
        &self,
        sink: &mut K,
    ) -> Result<pdf_gfx::EncodedPage, Error> {
        let pdf = self.pdf;
        page::encode_with(&pdf.reading, pdf.fonts(), self.page, &pdf.limits, sink)
    }

    /// Extracts reading-order text, line geometry, and search index
    /// ([`pdf_gfx::TextPage`]) from the page (`text-layout` feature).
    ///
    /// # Errors
    ///
    /// As [`page::text_page`](fn@page::text_page) fails.
    #[cfg(feature = "text-layout")]
    pub fn text_page(&self, opts: pdf_gfx::TextPageOpts) -> Result<pdf_gfx::TextPage, Error> {
        self.text_page_with(opts, &mut ())
    }

    /// [`Page::text_page`], reporting recoverable ISO 32000-2 departures to
    /// `sink` ([`pdf_gfx::Fault`]).
    ///
    /// # Errors
    ///
    /// As [`Page::text_page`] fails.
    #[cfg(feature = "text-layout")]
    pub fn text_page_with<K: Sink<pdf_gfx::Fault> + ?Sized>(
        &self,
        opts: pdf_gfx::TextPageOpts,
        sink: &mut K,
    ) -> Result<pdf_gfx::TextPage, Error> {
        let pdf = self.pdf;
        page::text_page_with(
            &pdf.reading,
            pdf.fonts(),
            self.page,
            &pdf.limits,
            opts,
            sink,
        )
    }

    /// Searches reading-order text on the page for `query` under `opts`
    /// (`text-layout` feature), returning every match with its character/byte
    /// ranges and highlight [`Quad`](pdf_gfx::Quad)s.
    ///
    /// # Errors
    ///
    /// As [`Page::text_page`] fails.
    #[cfg(feature = "text-layout")]
    pub fn search(
        &self,
        query: &str,
        opts: pdf_gfx::SearchOpts,
    ) -> Result<Vec<pdf_gfx::TextMatch>, Error> {
        Ok(self
            .text_page(pdf_gfx::TextPageOpts::default())?
            .search(query, opts))
    }

    /// Lists every Image XObject and inline image drawn on the page (including
    /// inside Form XObjects) in content-stream drawing order, without decoding
    /// pixel samples ([`page::images`](fn@page::images)).
    ///
    /// Pass any `ImageOf::XObject(id)` entry to `Page::decode_image` or
    /// `Page::decode_image_raw` (`images` feature) to decode its pixels.
    ///
    /// # Errors
    ///
    /// As [`Page::interpret`] fails.
    pub fn images(&self) -> Result<Vec<pdf_gfx::ImageDrawn>, Error> {
        self.images_with(&mut ())
    }

    /// [`Page::images`], reporting recoverable ISO 32000-2 departures to `sink`
    /// ([`pdf_gfx::Fault`]).
    ///
    /// # Errors
    ///
    /// As [`Page::images`] fails.
    pub fn images_with<K: Sink<pdf_gfx::Fault> + ?Sized>(
        &self,
        sink: &mut K,
    ) -> Result<Vec<pdf_gfx::ImageDrawn>, Error> {
        let pdf = self.pdf;
        page::images_with(&pdf.reading, pdf.fonts(), self.page, &pdf.limits, sink)
    }

    /// Decodes any indirect Image XObject stream `image_id` in the document
    /// into straight `sRGB` `RGBA8` ([`pdf_images::RgbaImage`], `images`
    /// feature), using this page's `/Resources` for `/Default*` color spaces
    /// and the file's catalog for `/OutputIntents` (under [`Rules::Iso`]),
    /// without collecting non-fatal faults.
    ///
    /// Discover image `ObjectId`s drawn on the page via [`Page::images`], and
    /// call [`RgbaImage::to_png`](pdf_images::RgbaImage::to_png) to encode PNG
    /// bytes.
    ///
    /// ```
    /// use pdfs::pdf_images::{JpegColorSpace, encode_jpeg};
    /// use pdfs::prelude::*;
    ///
    /// let mut doc = Document::new();
    /// let jpg = encode_jpeg(&[240, 60, 40], 1, 1, JpegColorSpace::Rgb, 95).unwrap();
    /// let img_id = doc.structure_mut().add(Object::from(jpg));
    /// let mut canvas = Canvas::new();
    /// canvas.draw_xobject(Res::written(img_id), Matrix::new(50.0, 0.0, 0.0, 50.0, 10.0, 10.0));
    /// *doc.add_page(Rect::new(0.0, 0.0, 100.0, 100.0)) = canvas.into_writer();
    ///
    /// let pdf = pdfs::from_bytes(doc.to_bytes().unwrap()).unwrap();
    /// let pages = pdf.pages().unwrap();
    /// let page = pages.get(0).unwrap();
    /// let drawn = page.images().unwrap();
    /// let ImageOf::XObject(id) = drawn[0].of else { unreachable!() };
    /// let rgba = page.decode_image(id).unwrap();
    /// assert_eq!((rgba.width(), rgba.height()), (1, 1));
    /// assert!(rgba.to_png().unwrap().starts_with(b"\x89PNG\r\n\x1a\n"));
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::Unreadable`] if the catalog or page `/Resources` cannot be read
    /// (locked, capped, or I/O failure), or [`Error::Images`] if `image_id` is
    /// missing, malformed, exceeds [`Limits::images`], or fails to decode.
    #[cfg(feature = "images")]
    pub fn decode_image(&self, image_id: ObjectId) -> Result<pdf_images::RgbaImage, Error> {
        self.decode_image_with(image_id, &mut ())
    }

    /// [`Page::decode_image`], reporting recoverable ISO 32000-2 departures to
    /// `sink` ([`pdf_images::Fault`], `images` feature).
    ///
    /// # Errors
    ///
    /// As [`Page::decode_image`] fails.
    #[cfg(feature = "images")]
    pub fn decode_image_with<K: Sink<pdf_images::Fault> + ?Sized>(
        &self,
        image_id: ObjectId,
        sink: &mut K,
    ) -> Result<pdf_images::RgbaImage, Error> {
        let pdf = self.pdf;
        let catalog = pdf.optional_catalog()?;
        page::decode_image_with(
            &pdf.reading,
            catalog,
            self.page,
            image_id,
            pdf.rules,
            &pdf.limits,
            sink,
        )
    }

    /// Decodes any indirect Image XObject stream `image_id` into native
    /// unpacked samples ([`pdf_images::RawImage`], `images` feature) before
    /// color conversion or mask compositing, without collecting non-fatal
    /// faults.
    ///
    /// # Errors
    ///
    /// [`Error::Images`] if `image_id` is missing, malformed, exceeds
    /// [`Limits::images`], or fails to decode.
    #[cfg(feature = "images")]
    pub fn decode_image_raw(&self, image_id: ObjectId) -> Result<pdf_images::RawImage, Error> {
        self.decode_image_raw_with(image_id, &mut ())
    }

    /// [`Page::decode_image_raw`], reporting recoverable ISO 32000-2 departures
    /// to `sink` ([`pdf_images::Fault`], `images` feature).
    ///
    /// # Errors
    ///
    /// As [`Page::decode_image_raw`] fails.
    #[cfg(feature = "images")]
    pub fn decode_image_raw_with<K: Sink<pdf_images::Fault> + ?Sized>(
        &self,
        image_id: ObjectId,
        sink: &mut K,
    ) -> Result<pdf_images::RawImage, Error> {
        let pdf = self.pdf;
        page::decode_image_raw_with(
            &pdf.reading,
            self.page,
            image_id,
            pdf.rules,
            &pdf.limits,
            sink,
        )
    }

    /// Rasterizes this page into a [`Pixmap`](pdf_raster::Pixmap) (`raster`
    /// feature), discarding [`RenderSummary`](pdf_raster::RenderSummary) for
    /// concise call sites (e.g. `page.render_pixmap(&RenderOpts::dpi(150.0))?.to_png()`).
    ///
    /// As with [`Page::render`], [`Pdf::rules`] overrides `opts.rules` and
    /// resource caps take the element-wise narrower of [`Pdf::limits`] and
    /// `opts.limits`.
    ///
    /// # Errors
    ///
    /// As [`Page::render`] fails.
    #[cfg(feature = "raster")]
    pub fn render_pixmap(
        &self,
        opts: &pdf_raster::RenderOpts,
    ) -> Result<pdf_raster::Pixmap, Error> {
        Ok(self.render(opts)?.0)
    }

    /// Rasterizes this page into `(Pixmap, RenderSummary)` (`raster` feature)
    /// under `opts`, reusing [`Pdf::render_cache`] and [`Pdf::font_fallback`]
    /// without collecting non-fatal faults.
    ///
    /// **Policy & limits override:** [`Pdf::rules`] always overrides
    /// `opts.rules`, and resource limits take the element-wise narrower of
    /// [`Pdf::limits`] and `opts.limits` so customizing a single cap on `opts`
    /// never widens the caps the [`Pdf`] was opened with. Call
    /// [`Pixmap::to_png`](pdf_raster::Pixmap::to_png) to encode PNG bytes, or
    /// [`Page::render_pixmap`] when [`RenderSummary`](pdf_raster::RenderSummary)
    /// is not needed.
    ///
    /// ```
    /// use pdfs::prelude::*;
    ///
    /// let mut doc = Document::new();
    /// let p = doc.add_page(Rect::new(0.0, 0.0, 72.0, 72.0));
    /// p.fill_color(Color::rgb8([220, 40, 30]));
    /// p.rect(0.0, 0.0, 72.0, 72.0);
    /// p.paint(Paint::Fill);
    ///
    /// let pdf = pdfs::from_bytes(doc.to_bytes().unwrap()).unwrap();
    /// let pages = pdf.pages().unwrap();
    /// let page = pages.get(0).unwrap();
    /// let (pixmap, summary) = page.render(&RenderOpts::dpi(72.0)).unwrap();
    /// assert_eq!((pixmap.width(), pixmap.height()), (72, 72));
    /// assert_eq!(pixmap.pixel_rgba8(36, 36), Some([220, 40, 30, 255]));
    /// assert!(!summary.approximated);
    /// assert!(pixmap.to_png().unwrap().starts_with(b"\x89PNG\r\n\x1a\n"));
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::Unreadable`] if the catalog cannot be read, or
    /// [`Error::Raster`] as [`page::render`](fn@page::render) fails.
    #[cfg(feature = "raster")]
    pub fn render(
        &self,
        opts: &pdf_raster::RenderOpts,
    ) -> Result<(pdf_raster::Pixmap, pdf_raster::RenderSummary), Error> {
        self.render_with(opts, &mut ())
    }

    /// [`Page::render`], reporting recoverable ISO 32000-2 departures to
    /// `sink` ([`pdf_raster::Fault`], `raster` feature).
    ///
    /// # Errors
    ///
    /// As [`Page::render`] fails.
    #[cfg(feature = "raster")]
    pub fn render_with<K: Sink<pdf_raster::Fault> + ?Sized>(
        &self,
        opts: &pdf_raster::RenderOpts,
        sink: &mut K,
    ) -> Result<(pdf_raster::Pixmap, pdf_raster::RenderSummary), Error> {
        let pdf = self.pdf;
        let catalog = pdf.optional_catalog()?;
        page::render_with(
            &pdf.reading,
            catalog,
            self.page,
            &pdf.cache,
            pdf.font_fallback(),
            opts,
            pdf.rules,
            &pdf.limits,
            sink,
        )
    }
}

#[cfg(feature = "raster")]
impl<S: ByteSource> pdf_raster::RenderedDoc for Pdf<S> {
    fn page_count(&self) -> usize {
        self.cached_page_table().map_or(0, pages::PageTable::len)
    }

    fn page_geom(&self, page_index: usize) -> Result<pdf_gfx::PageGeom, pdf_raster::BackendError> {
        if *self.encryption() == Encryption::Locked {
            return Err(pdf_raster::BackendError::Locked);
        }
        let tbl = self
            .cached_page_table()
            .map_err(pdf_raster::BackendError::custom)?;
        let Some(p) = tbl.get(&self.reading, page_index) else {
            return Err(pdf_raster::BackendError::PageOutOfBounds {
                page_index,
                page_count: tbl.len(),
            });
        };
        Ok(page::geom(&self.reading, &p))
    }

    fn render_page(
        &self,
        page_index: usize,
        opts: &pdf_raster::RenderOpts,
    ) -> Result<(pdf_raster::Pixmap, pdf_raster::RenderSummary), pdf_raster::BackendError> {
        if *self.encryption() == Encryption::Locked {
            return Err(pdf_raster::BackendError::Locked);
        }
        let tbl = self
            .cached_page_table()
            .map_err(pdf_raster::BackendError::custom)?;
        let Some(p) = tbl.get(&self.reading, page_index) else {
            return Err(pdf_raster::BackendError::PageOutOfBounds {
                page_index,
                page_count: tbl.len(),
            });
        };
        let catalog = self.optional_catalog().map_err(|err| match err {
            Error::Unreadable(u) if u.why == pdf_objects::Why::Locked => {
                pdf_raster::BackendError::Locked
            }
            other => pdf_raster::BackendError::custom(other),
        })?;
        page::render_with(
            &self.reading,
            catalog,
            &p,
            &self.cache,
            self.font_fallback(),
            opts,
            self.rules,
            &self.limits,
            &mut (),
        )
        .map_err(|err| match err {
            Error::Raster(r) => pdf_raster::BackendError::Raster(r),
            Error::Unreadable(u) if u.why == pdf_objects::Why::Locked => {
                pdf_raster::BackendError::Locked
            }
            other => pdf_raster::BackendError::custom(other),
        })
    }
}
