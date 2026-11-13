//! A file being read, its pages, and a page.

use std::borrow::Cow;
use std::fmt;
use std::iter::FusedIterator;

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

/// A file being read: opened under [`Options`], then each object read when
/// it's first asked for, kept, from any thread (pdf-structure's
/// [`Reading`], lazy); its pages listed, each page's content joined, what it
/// says of itself and claims, and whether it's linearized, each read under
/// the [`Limits`] and by the rules it was opened with. Every recipe here is
/// a function too ([`page`](mod@crate::page), [`file`](mod@crate::file)),
/// which this calls with them. What it doesn't do, the reading does
/// ([`Pdf::reading`]).
///
/// It's `Send` and `Sync`, so one file can be read on many threads. Its
/// `Debug` gives its length, its encryption and how it's read, not what it
/// holds.
pub struct Pdf<S: ByteSource = Vec<u8>> {
    reading: Reading<S>,
    limits: Limits,
    rules: Rules,
    fonts: Fonts,
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
            fonts: Fonts::default(),
            identity,
        }
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
        &self.fonts
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
            fonts,
            identity,
        } = self;
        let back = |reading, fonts| Pdf {
            reading,
            limits,
            rules,
            fonts,
            identity: identity.clone(),
        };
        match reading.unlock(passwords) {
            Ok(reading) => Ok(back(reading, Fonts::default())),
            Err((reading, refused)) => Err((back(reading, fonts), refused)),
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

    /// What it says of itself, its document information dictionary
    /// (ISO 32000-2, 14.3.3), read typed, its text decoded by its rules;
    /// none where its trailer has none. Nothing departing from the clause is
    /// told: [`Pdf::info_with`] tells it.
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

    /// Its pages, in order, as its page tree lists them by its rules, each
    /// with what it inherits and from where (pdf-semantics'
    /// [`pages::pages`]). Nothing departing from the clause is told:
    /// [`Pdf::pages_with`] tells it.
    ///
    /// # Errors
    ///
    /// [`Error::NoCatalog`] and [`Error::Unreadable`] as [`Pdf::catalog`]
    /// fails; [`Error::Pages`] where the tree doesn't read: past
    /// [`Limits::semantics`]' caps, by PDFium's rules a tree with no page or
    /// a loop, and an object on the way that doesn't read for the reader's
    /// own reason, never read as no pages.
    pub fn pages(&self) -> Result<Pages<'_, S>, Error> {
        self.pages_with(&mut ())
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
        phase!(DEBUG, "pages", pdfium = matches!(self.rules, Rules::Pdfium));
        let catalog = self.catalog()?;
        let walked = pages::pages(
            &self.reading,
            catalog,
            self.rules,
            &self.limits.semantics,
            sink,
        );
        Ok(Pages {
            table: walked?,
            pdf: self,
        })
    }

    /// Stream `id`'s data, decrypted and decoded through its filters, as a
    /// viewer takes it (pdf-objects' [`Streams`]): what decodes of a stream
    /// that decodes only in part, up to [`Limits::read`]'s cap on a stream.
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
    /// ([`Document::save_update`]). It borrows the reading, which what's as
    /// read is copied from.
    ///
    /// # Errors
    ///
    /// [`Error::Read`] where the source doesn't give its bytes, and
    /// [`Error::Write`] where it's encrypted and no password opened it
    /// ([`Locked`](pdf_structure::write::Error::Locked)), or its security handler is one the reader
    /// can't open: there's nothing to write its strings and streams from.
    pub fn edit(&self) -> Result<Document<'_>, Error> {
        phase!(DEBUG, "edit", bytes = self.reading.size());
        let doc = self.reading.to_document()?;
        let encryptor = Encryptor::of(&self.reading)?;
        let base = Base::of(&self.reading);
        let from: &dyn ReadFrom = &self.reading;
        let identity = self.identity.clone();
        Ok(Document::edited(doc, encryptor, from, base, identity))
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

    /// Its content: its streams decoded and joined under
    /// [`Limits::content`], as [`page::content`](fn@page::content) joins
    /// them, and the color spaces its resources name.
    ///
    /// # Errors
    ///
    /// As [`page::content`](fn@page::content) fails: [`Error::Content`]
    /// where its streams are past the cap or one doesn't read for the
    /// reader's own reason, its [`Why`](pdf_objects::Why) kept, and
    /// [`Error::Unreadable`] where its resources don't read.
    pub fn content(&self) -> Result<Content, Error> {
        page::content(&self.pdf.reading, self.page, &self.pdf.limits.content)
    }

    /// The fonts its resources name, each by its name, read once a file and
    /// kept, as [`page::fonts`](fn@page::fonts) reads them, within
    /// [`Limits::font_budget`].
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
            &pdf.fonts,
            self.page,
            &pdf.limits,
            &mut budget,
        )
    }

    /// The strings its content shows, in order, each in its font, the forms
    /// it draws followed, as PDFium reads them for its text, with no layout,
    /// as [`page::text`](fn@page::text) reads them, within
    /// [`Limits::font_budget`]: [`Shown::text`] joins them. What isn't read
    /// is said in the answer ([`Shown::unread`]).
    ///
    /// # Errors
    ///
    /// As [`Page::content`] fails.
    pub fn text(&self) -> Result<Shown, Error> {
        let pdf = self.pdf;
        let mut budget = pdf.limits.font_budget;
        page::text(
            &pdf.reading,
            &pdf.fonts,
            self.page,
            &pdf.limits,
            &mut budget,
        )
    }
}
