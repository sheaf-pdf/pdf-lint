//! Opening PDF files and configuring reader behavior.

use std::fmt;
#[cfg(any(unix, windows))]
use std::path::Path;

use pdf_crypt::Password;
use pdf_filters::Backend;
use pdf_objects::pdf_tokenizer::Rules;
#[cfg(any(unix, windows))]
use pdf_structure::read::FileBytes;
use pdf_structure::read::{self, ByteSource, Faults, Identity, Reading, Recovery};

use crate::error::Error;
use crate::limits::Limits;
use crate::pdf::Pdf;
use crate::phases::phase;

/// Options controlling how a PDF file is opened and read.
///
/// Customize options by mutating a copy of [`Options::DEFAULT`] (for example,
/// `options.passwords = &passwords;`).
///
/// By default, `Options::DEFAULT` uses viewer-compatible (PDFium) behavior:
/// [`Recovery::Pdfium`] for damaged files, [`Rules::Pdfium`] for page trees and
/// text decoding, [`Backend::ZlibRs`] for Flate decompression, and
/// [`Faults::Off`] for zero-overhead silent parsing. For strict spec validation,
/// set `Recovery::Verified`, `Rules::Iso`, `Faults::Kept`, and
/// `Backend::MinizOxide`.
///
/// `Debug` prints only the password count, never the passwords themselves.
///
/// ```
/// use pdfs::Options;
/// use pdfs::pdf_filters::Backend;
/// use pdfs::pdf_structure::read::{Faults, Recovery};
/// use pdfs::pdf_tokenizer::Rules;
///
/// let file = b"%PDF-1.7\n\
///     1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
///     2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>\nendobj\n\
///     3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n\
///     4 0 obj\n<< /Length 27 >>\nstream\n0 0 1 rg 72 72 144 144 re f\nendstream\nendobj\n\
///     xref\n0 5\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n\
///     0000000139 00000 n \n0000000202 00000 n \n\
///     trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n279\n%%EOF\n";
///
/// let mut options = Options::DEFAULT;
/// options.rules = Rules::Iso;
/// options.recovery = Recovery::Verified;
/// options.faults = Faults::Kept;
/// options.flate = Backend::MinizOxide;
/// let pdf = options.from_bytes(&file[..]).unwrap();
/// let mut told = Vec::new();
/// assert_eq!(pdf.pages_with(&mut told).unwrap().len(), 1);
/// assert!(told.is_empty() && pdf.faults().is_empty());
/// ```
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct Options<'a> {
    /// The passwords an encrypted file is opened by, in the empty
    /// password's place, each as typed, tried in each of its encodings, as
    /// pdf-structure's reading tries them
    /// ([`read::Options::passwords`]); which opened it is
    /// [`Encryption::Opened`](pdf_structure::Encryption::Opened)'s `by`, its
    /// index here. None by default, for the empty password. They're
    /// borrowed for the open alone: nothing keeps them.
    pub passwords: &'a [Password<'a>],
    /// The caps it's read under: [`Limits::DEFAULT`] by default.
    pub limits: Limits,
    /// The rules its page tree is walked by and its text strings decoded
    /// by: PDFium's by default, `Rules::Iso` for ISO 32000-2's.
    pub rules: Rules,
    /// How it's read where its sections or objects aren't where it says:
    /// as PDFium reads it ([`Recovery::Pdfium`]) by default, or only where
    /// the file proves it ([`Recovery::Verified`]).
    pub recovery: Recovery,
    /// The backend its Flate streams inflate on: zlib-rs
    /// ([`Backend::ZlibRs`]) by default, or miniz_oxide. On clean data the
    /// two decode alike; on damaged data, how much decodes can differ.
    pub flate: Backend,
    /// Whether its faults are looked for and kept: none by default
    /// ([`Faults::Off`]), at no cost; with [`Faults::Kept`], the reading's
    /// own are [`Pdf::faults`]. Each crate's faults are told to the sink a
    /// call's `_with` form is given, whatever this says.
    pub faults: Faults,
    /// Optional shared [`FontFallback`](pdf_raster::FontFallback) provider
    /// (`raster` feature) used by `Page::render` and `RenderedDoc` on the
    /// opened [`Pdf`].
    #[cfg(feature = "raster")]
    pub font_fallback: Option<&'a std::sync::Arc<dyn pdf_raster::FontFallback>>,
}

impl Options<'static> {
    /// The options a file is opened with unless it's given others: no
    /// password but the empty one, [`Limits::DEFAULT`], and a viewer's
    /// reading, PDFium's rules and recovery, zlib-rs, no fault looked for.
    pub const DEFAULT: Options<'static> = Options {
        passwords: &[],
        limits: Limits::DEFAULT,
        rules: Rules::Pdfium,
        recovery: Recovery::Pdfium,
        flate: Backend::ZlibRs,
        faults: Faults::Off,
        #[cfg(feature = "raster")]
        font_fallback: None,
    };
}

impl Default for Options<'_> {
    fn default() -> Self {
        Options::DEFAULT
    }
}

impl fmt::Debug for Options<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("Options");
        s.field("passwords", &self.passwords.len())
            .field("limits", &self.limits)
            .field("rules", &self.rules)
            .field("recovery", &self.recovery)
            .field("flate", &self.flate)
            .field("faults", &self.faults);
        #[cfg(feature = "raster")]
        s.field("has_font_fallback", &self.font_fallback.is_some());
        s.finish()
    }
}

impl<'a> Options<'a> {
    /// The file at `path`, opened under these options: read by position, a
    /// window at a time, never whole but where a damaged one is scanned
    /// (pdf-structure's [`FileBytes`]), so a large file isn't held in
    /// memory; each object read when it's asked for. The file is held open
    /// while the [`Pdf`] lives, which knows it by its [`Identity`]
    /// ([`Pdf::identity`]). A file changed while it's read is read as it
    /// then is: a caller that must change it reads it into memory first,
    /// `from_bytes(std::fs::read(path)?)`, whose `Pdf` holds its own bytes.
    /// Only on Unix and Windows, where a file is read by position.
    ///
    /// ```
    /// use pdfs::pdf_structure::read::{Faults, Recovery};
    /// use pdfs::prelude::*;
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path().join("one.pdf");
    /// let mut doc = Document::new();
    /// doc.add_page(Rect::LETTER);
    /// doc.save(&path).unwrap();
    ///
    /// // Read as a checker reads it: no recovery past what's written, every
    /// // fault kept.
    /// let mut options = Options::DEFAULT;
    /// options.recovery = Recovery::Verified;
    /// options.faults = Faults::Kept;
    /// let pdf = options.open(&path).unwrap();
    /// assert_eq!(pdf.pages().unwrap().len(), 1);
    /// assert!(pdf.faults().is_empty());
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::Io`] where the file doesn't open, or what identifies it
    /// doesn't read; [`Error::Read`] where it doesn't give its bytes.
    #[cfg(any(unix, windows))]
    pub fn open(&self, path: impl AsRef<Path>) -> Result<Pdf<FileBytes>, Error> {
        let file = std::fs::File::open(path)?;
        let bytes = FileBytes::new(file)?;
        let identity = bytes.identity().clone();
        self.opened(bytes, Some(identity))
    }

    /// The file `source` holds, opened under these options: its header,
    /// cross-reference sections and trailer read, and an encrypted one's
    /// security handler opened by the passwords given; each object read when
    /// it's asked for. Bytes in memory (`Vec<u8>`, `&[u8]`, `Arc<[u8]>`) are
    /// read as they are, and any other source by position.
    ///
    /// # Errors
    ///
    /// [`Error::Read`], only where the source doesn't give its bytes, which
    /// bytes in memory always do.
    pub fn from_bytes<S: ByteSource>(&self, source: S) -> Result<Pdf<S>, Error> {
        self.opened(source, None)
    }

    /// `source` opened, the file `identity` names where it's given.
    fn opened<S: ByteSource>(
        &self,
        source: S,
        identity: Option<Identity>,
    ) -> Result<Pdf<S>, Error> {
        phase!(DEBUG, "open", bytes = source.len());
        let reading = Reading::open(source, &self.reading())?;
        Ok(Pdf::new(reading, self, identity))
    }

    /// The reading's options: these, the rest pdf-structure's defaults.
    const fn reading(&self) -> read::Options<'a> {
        let mut options = read::Options::DEFAULT;
        options.limits = self.limits.read;
        options.flate = self.flate;
        options.passwords = self.passwords;
        options.faults = self.faults;
        options.recovery = self.recovery;
        options
    }
}

/// The file at `path`, opened under [`Options::DEFAULT`]: [`Options::open`].
///
/// ```
/// let dir = tempfile::tempdir().unwrap();
/// let path = dir.path().join("one.pdf");
/// let mut doc = pdfs::Document::new();
/// doc.add_page(pdfs::prelude::Rect::LETTER);
/// doc.save(&path).unwrap();
///
/// let pdf = pdfs::open(&path).unwrap();
/// assert_eq!(pdf.pages().unwrap().len(), 1);
/// // It knows the file it reads, so a save over it is refused.
/// assert!(pdf.identity().is_some());
/// ```
///
/// # Errors
///
/// As [`Options::open`] fails.
#[cfg(any(unix, windows))]
pub fn open(path: impl AsRef<Path>) -> Result<Pdf<FileBytes>, Error> {
    Options::DEFAULT.open(path)
}

/// The file `source` holds, opened under [`Options::DEFAULT`]:
/// [`Options::from_bytes`].
///
/// # Errors
///
/// As [`Options::from_bytes`] fails: only where the source doesn't give its
/// bytes.
pub fn from_bytes<S: ByteSource>(source: S) -> Result<Pdf<S>, Error> {
    Options::DEFAULT.from_bytes(source)
}
