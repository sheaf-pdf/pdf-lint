//! Document-level rendering traits ([`RenderBackend`], [`RenderedDoc`]) for
//! swappable multi-engine execution (`pdfs::Pdf`, `pdfs::RustBackend`, and
//! `pdf_raster::PdfiumBackend`).

use std::sync::Arc;

use pdf_gfx::PageGeom;

use crate::error::Error;
use crate::opts::{RenderOpts, RenderSummary};
use crate::pixmap::Pixmap;
use crate::source::RenderSource;

/// Why an engine refused a document it was given to open, as
/// [`BackendError::Refused`] says it: the file's own trouble, apart from a
/// password it lacks ([`BackendError::Locked`]) and from an engine's or an
/// adapter's own failure ([`BackendError::Backend`]).
///
/// A host acts on the difference: it asks for a password on `Locked`, and on
/// a refusal says the file can't be drawn by this engine, which no password
/// changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Refusal {
    /// The bytes aren't a PDF the engine can read: no header, cross-reference
    /// or trailer it can follow, or no catalog or page tree under them.
    /// PDFium's `FPDF_ERR_FORMAT`.
    Format,
    /// The document is encrypted by a security handler, or with an encryption
    /// dictionary, the engine doesn't open, whatever the password. PDFium's
    /// `FPDF_ERR_SECURITY`.
    Security,
    /// The source didn't give the bytes asked of it: a read of a
    /// [`RenderSource`] that failed, or PDFium's `FPDF_ERR_FILE`.
    Source,
    /// The engine refused the document and named no reason this enum has.
    /// PDFium's `FPDF_ERR_UNKNOWN` and `FPDF_ERR_PAGE`.
    Unknown,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Refusal::Format => "it isn't a PDF the engine can read",
            Refusal::Security => "its encryption isn't one the engine opens",
            Refusal::Source => "its bytes didn't read",
            Refusal::Unknown => "the engine gave no reason",
        })
    }
}

/// Error returned when opening a document or rendering a page through a
/// [`RenderBackend`] / [`RenderedDoc`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BackendError {
    /// Underlying rasterization, viewport, cancellation, or resource-limit
    /// failure.
    #[error(transparent)]
    Raster(#[from] Error),
    /// Document is encrypted and none of the supplied passwords opened it.
    #[error("PDF document is encrypted and locked")]
    Locked,
    /// The engine refused the document for a reason of the file's own that
    /// no password changes, on open (or, from a `pdfs::Pdf` opened without a
    /// backend, when a page is first asked of it): `reason` names it, and
    /// `error` is the engine's own error, for a downcast. From the pure-Rust
    /// engine that is `pdfs::Error`, or pdf-structure's
    /// `source::Unsupported` where the reason is [`Refusal::Security`]; from
    /// PDFium, `pdf_raster::pdfium::Error`; from the default
    /// [`RenderBackend::open_source`], the source's [`std::io::Error`].
    ///
    /// The message is whole, the engine's words after the reason, so the
    /// engine's error isn't this error's `source()` too.
    #[error("the document was refused: {reason}: {error}")]
    Refused {
        /// Why, in the terms every engine shares.
        reason: Refusal,
        /// The engine's own error.
        error: Box<dyn std::error::Error + Send + Sync>,
    },
    /// Requested zero-based `page_index` is out of range (`>= page_count`).
    #[error("page index {page_index} out of bounds (page count: {page_count})")]
    PageOutOfBounds {
        /// Requested zero-based page index.
        page_index: usize,
        /// Total number of pages in the document.
        page_count: usize,
    },
    /// Requested option combination cannot be honored by this backend (for
    /// example, `Viewport::Custom` with `draw.forms == true` on PDFium).
    #[error("unsupported render option for backend: {0}")]
    Unsupported(&'static str),
    /// An error of the engine's or its adapter's own that isn't one of the
    /// above: a page that doesn't load, a library call that failed, a cap the
    /// engine reads under.
    #[error(transparent)]
    Backend(Box<dyn std::error::Error + Send + Sync>),
}

impl BackendError {
    /// Wraps an arbitrary `Send + Sync + 'static` backend error.
    #[must_use]
    pub fn custom(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Backend(Box::new(err))
    }

    /// A document refused on open for `reason`, `err` the engine's own error.
    #[must_use]
    pub fn refused(reason: Refusal, err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Refused {
            reason,
            error: Box::new(err),
        }
    }

    /// Why the document was refused, where this is [`BackendError::Refused`].
    #[must_use]
    pub const fn refusal(&self) -> Option<Refusal> {
        match self {
            Self::Refused { reason, .. } => Some(*reason),
            _ => None,
        }
    }
}

/// An opened PDF document handle bound to a [`RenderBackend`], safe to share
/// and query concurrently across threads (`Send + Sync`).
pub trait RenderedDoc: Send + Sync {
    /// Number of pages in the document.
    fn page_count(&self) -> usize;

    /// Returns the 2D geometry (`MediaBox`, `CropBox`, `Rotation`, and where
    /// available `UserUnit` and auxiliary boxes) of zero-based `page_index`.
    ///
    /// Note that external engines such as `PdfiumBackend` populate `media_box`,
    /// `crop_box`, `rotation`, and `user_unit`, leaving `bleed_box`, `trim_box`,
    /// and `art_box` as `None`.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError::PageOutOfBounds`] if `page_index >= self.page_count()`,
    /// or [`BackendError`] if page metadata cannot be read.
    fn page_geom(&self, page_index: usize) -> Result<PageGeom, BackendError>;

    /// Rasterizes zero-based `page_index` under `opts`.
    ///
    /// # Per-Engine Option Support
    ///
    /// - **Pure-Rust (`pdfs::Pdf` / `pdfs::RustBackend`):** Honors every field
    ///   of [`RenderOpts`] (`viewport`, `medium`, `background`, `format`, `draw`,
    ///   `appearances`, `anti_alias`, `hint_fonts`, `stop`), using the opened
    ///   `Pdf`'s `rules` and the element-wise narrower of `opts.limits` and the
    ///   `Pdf`'s `limits`, and returns a populated [`RenderSummary`] with
    ///   `reported == true`.
    /// - **`PdfiumBackend` (`pdfium` feature):** Honors `viewport`, `medium`
    ///   (`FPDF_PRINTING`), `background`, `format`, `draw`, `anti_alias`, `stop`
    ///   (checked before entering FFI), and the pixmap dimensions/byte caps in
    ///   `limits`; ignores `appearances`, `hint_fonts`, `rules`, and
    ///   interpreter/codec sub-limits; returns [`BackendError::Unsupported`] for
    ///   `Viewport::Custom` when `draw.forms == true` (PDFium's matrix path
    ///   cannot render form-fill widgets), and returns
    ///   [`RenderSummary::unreported`] (`reported == false`).
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] on invalid viewports, cooperative cancellation
    /// (`BackendError::Raster(Error::Cancelled)`), unsupported backend options,
    /// out-of-bounds `page_index`, exceeded [`crate::Limits`], or unreadable
    /// content.
    fn render_page(
        &self,
        page_index: usize,
        opts: &RenderOpts,
    ) -> Result<(Pixmap, RenderSummary), BackendError>;
}

/// Swappable PDF rendering engine (`pdfs::RustBackend`, `PdfiumBackend`, etc.).
pub trait RenderBackend: Send + Sync {
    /// Stable human-readable backend identifier (e.g. `"pdf-raster"` or
    /// `"pdfium"`).
    fn name(&self) -> &'static str;

    /// Opens `bytes` (shared zero-copy via `Arc<[u8]>`) with an optional UTF-8
    /// `password`.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError::Locked`] if `bytes` is encrypted and `password`
    /// does not unlock it; [`BackendError::Refused`] if the engine refuses the
    /// document for a reason of the file's own (its format, its security
    /// handler, bytes that didn't read); or another [`BackendError`] where the
    /// engine itself failed.
    fn open_bytes(
        &self,
        bytes: Arc<[u8]>,
        password: Option<&str>,
    ) -> Result<Box<dyn RenderedDoc>, BackendError>;

    /// Opens `bytes` trying the empty password first and then each candidate in
    /// `passwords` in order until one unlocks the document.
    ///
    /// Backends that natively accept a slice of candidate passwords (such as
    /// `pdfs::RustBackend`) override this to derive keys in a single open pass.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError::Locked`] if `bytes` is encrypted and none of
    /// `passwords` unlocks it, or [`BackendError`] on a non-password failure.
    fn open_bytes_with_passwords(
        &self,
        bytes: Arc<[u8]>,
        passwords: &[&str],
    ) -> Result<Box<dyn RenderedDoc>, BackendError> {
        each_password(passwords, |password| {
            self.open_bytes(bytes.clone(), password)
        })
    }

    /// Opens the document `source` holds, read by position, with an optional
    /// UTF-8 `password`: for a document that isn't one buffer in memory (a
    /// file read where it is, or [`Parts`](crate::Parts), several shared
    /// buffers one after another), opened with no copy of the whole. The
    /// document keeps `source` while it lives and reads from it as it needs.
    ///
    /// Both of the family's engines open a source where it is
    /// (`pdfs::RustBackend` through its reader's byte source, `PdfiumBackend`
    /// through PDFium's custom file access). The default, for a backend that
    /// opens one buffer alone, copies the whole source into a buffer of its
    /// own and opens that by [`RenderBackend::open_bytes`]: always a copy,
    /// one buffer or not, since a source can't hand over the `Arc` it's
    /// held by. A caller with an `Arc<[u8]>` calls `open_bytes`.
    ///
    /// # Errors
    ///
    /// As [`RenderBackend::open_bytes`] fails; [`BackendError::Refused`] with
    /// [`Refusal::Source`] where the source doesn't give its bytes.
    fn open_source(
        &self,
        source: Arc<dyn RenderSource>,
        password: Option<&str>,
    ) -> Result<Box<dyn RenderedDoc>, BackendError> {
        self.open_bytes(joined(&*source)?, password)
    }

    /// Opens `source` trying the empty password first and then each candidate
    /// in `passwords` in order until one unlocks the document, as
    /// [`RenderBackend::open_bytes_with_passwords`] does bytes.
    ///
    /// The default is for a backend that opens one buffer alone, as
    /// [`RenderBackend::open_source`]'s is: it copies the source into one
    /// buffer, once, and tries the passwords on that by
    /// [`RenderBackend::open_bytes_with_passwords`]. It doesn't call
    /// `open_source`, so a backend with an `open_source` of its own, which
    /// reads a source where it is, writes this one too (both of the
    /// family's engines do), or its passwords are tried on a copy.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError::Locked`] if the document is encrypted and none
    /// of `passwords` unlocks it, or [`BackendError`] on a non-password
    /// failure.
    fn open_source_with_passwords(
        &self,
        source: Arc<dyn RenderSource>,
        passwords: &[&str],
    ) -> Result<Box<dyn RenderedDoc>, BackendError> {
        self.open_bytes_with_passwords(joined(&*source)?, passwords)
    }
}

/// Every byte of `source`, copied into one buffer.
fn joined(source: &dyn RenderSource) -> Result<Arc<[u8]>, BackendError> {
    match source.whole() {
        Some(whole) => Ok(Arc::from(whole)),
        None => match source.read_at(0..source.len()) {
            Ok(read) => Ok(Arc::from(read)),
            Err(err) => Err(BackendError::refused(Refusal::Source, err)),
        },
    }
}

/// `open` with no password, then with each of `passwords` in order, until one
/// opens the document or it fails for another reason than its lock.
pub(crate) fn each_password(
    passwords: &[&str],
    mut open: impl FnMut(Option<&str>) -> Result<Box<dyn RenderedDoc>, BackendError>,
) -> Result<Box<dyn RenderedDoc>, BackendError> {
    match open(None) {
        Err(BackendError::Locked) if !passwords.is_empty() => {}
        opened => return opened,
    }
    for &password in passwords {
        match open(Some(password)) {
            Err(BackendError::Locked) => {}
            opened => return opened,
        }
    }
    Err(BackendError::Locked)
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Mutex;

    use super::*;
    use crate::source::Parts;

    /// A backend that opens one buffer alone, and records what it was given.
    #[derive(Default)]
    struct OneBuffer {
        opened: Mutex<Vec<(Vec<u8>, Option<String>)>>,
        password: Option<&'static str>,
    }

    struct NoPages;

    impl RenderedDoc for NoPages {
        fn page_count(&self) -> usize {
            0
        }

        fn page_geom(&self, page_index: usize) -> Result<PageGeom, BackendError> {
            Err(BackendError::PageOutOfBounds {
                page_index,
                page_count: 0,
            })
        }

        fn render_page(
            &self,
            page_index: usize,
            _: &RenderOpts,
        ) -> Result<(Pixmap, RenderSummary), BackendError> {
            Err(BackendError::PageOutOfBounds {
                page_index,
                page_count: 0,
            })
        }
    }

    impl RenderBackend for OneBuffer {
        fn name(&self) -> &'static str {
            "one-buffer"
        }

        fn open_bytes(
            &self,
            bytes: Arc<[u8]>,
            password: Option<&str>,
        ) -> Result<Box<dyn RenderedDoc>, BackendError> {
            self.opened
                .lock()
                .unwrap()
                .push((bytes.to_vec(), password.map(str::to_owned)));
            if self.password.is_some() && self.password != password {
                return Err(BackendError::Locked);
            }
            Ok(Box::new(NoPages))
        }
    }

    struct Failing;

    impl RenderSource for Failing {
        fn len(&self) -> usize {
            8
        }

        fn read_at(&self, _: std::ops::Range<usize>) -> io::Result<std::borrow::Cow<'_, [u8]>> {
            Err(io::Error::other("the disk went away"))
        }
    }

    #[test]
    fn the_default_open_source_opens_the_sources_bytes_joined() {
        let backend = OneBuffer::default();
        let parts = Parts::new(vec![Arc::from(&b"%PDF"[..]), Arc::from(&b"-1.7"[..])]);
        backend.open_source(Arc::new(parts), Some("pw")).unwrap();
        let whole: Arc<[u8]> = Arc::from(&b"whole"[..]);
        backend.open_source(Arc::new(whole), None).unwrap();
        assert_eq!(
            *backend.opened.lock().unwrap(),
            [
                (b"%PDF-1.7".to_vec(), Some("pw".to_owned())),
                (b"whole".to_vec(), None)
            ]
        );
    }

    #[test]
    fn a_source_that_doesnt_read_is_refused_as_its_source() {
        let backend = OneBuffer::default();
        let err = backend.open_source(Arc::new(Failing), None).err().unwrap();
        assert_eq!(err.refusal(), Some(Refusal::Source));
        // The message is whole, and the source's error is there to downcast.
        assert_eq!(
            err.to_string(),
            "the document was refused: its bytes didn't read: the disk went away"
        );
        assert!(std::error::Error::source(&err).is_none());
        let BackendError::Refused { error, .. } = &err else {
            unreachable!()
        };
        assert!(error.downcast_ref::<io::Error>().is_some());
        assert!(backend.opened.lock().unwrap().is_empty());
        let with_passwords = backend.open_source_with_passwords(Arc::new(Failing), &["a"]);
        assert_eq!(
            with_passwords.err().unwrap().refusal(),
            Some(Refusal::Source)
        );
        assert_eq!(BackendError::Locked.refusal(), None);
    }

    #[test]
    fn passwords_are_tried_in_order_after_none_for_bytes_and_a_source() {
        let tried = |backend: &OneBuffer| -> Vec<Option<String>> {
            let mut opened = backend.opened.lock().unwrap();
            opened.drain(..).map(|(_, password)| password).collect()
        };
        let some = |s: &str| Some(s.to_owned());
        let backend = OneBuffer {
            password: Some("two"),
            ..OneBuffer::default()
        };
        let bytes: Arc<[u8]> = Arc::from(&b"x"[..]);

        backend
            .open_bytes_with_passwords(bytes.clone(), &["one", "two", "three"])
            .unwrap();
        assert_eq!(tried(&backend), [None, some("one"), some("two")]);

        let source: Arc<dyn RenderSource> = Arc::new(bytes.clone());
        backend
            .open_source_with_passwords(Arc::clone(&source), &["two"])
            .unwrap();
        assert_eq!(tried(&backend), [None, some("two")]);

        // None opens it: locked, each tried once; with none given, one open.
        let locked = backend.open_source_with_passwords(source, &["a", "b"]);
        assert!(matches!(locked, Err(BackendError::Locked)));
        assert_eq!(tried(&backend), [None, some("a"), some("b")]);
        let locked = backend.open_bytes_with_passwords(bytes, &[]);
        assert!(matches!(locked, Err(BackendError::Locked)));
        assert_eq!(tried(&backend), [None]);
    }
}
