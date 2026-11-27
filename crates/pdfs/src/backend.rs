//! Pure-Rust [`RenderBackend`] implementation over [`crate::Pdf`].

use std::borrow::Cow;
use std::fmt;
use std::io;
use std::ops::Range;
use std::sync::Arc;

use pdf_crypt::Password;
use pdf_objects::Why;
use pdf_raster::{BackendError, FontFallback, Refusal, RenderBackend, RenderSource, RenderedDoc};
use pdf_semantics::pages;
use pdf_structure::Encryption;
use pdf_structure::read::{self, ByteSource};

use crate::error::Error;
use crate::open::Options;

/// Pure-Rust [`RenderBackend`] powered by `pdfs` and `pdf-raster`.
///
/// Opens an in-memory [`Pdf<Arc<[u8]>>`](crate::Pdf) (which directly implements
/// [`RenderedDoc`]), unlocks password-protected files via `pdf-crypt`, walks and
/// caches the page tree upfront during `open_bytes` / `open_bytes_with_passwords`,
/// and shares parsed font programs across pages. The returned [`RenderedDoc`] is
/// `Send + Sync` (`#![forbid(unsafe_code)]`) so caller threads can rasterize
/// pages or tiles in parallel.
///
/// [`RenderBackend::open_source`] opens a [`RenderSource`] where it is, as
/// the reader's own byte source: one buffer in memory is read as
/// [`open_bytes`](RenderBackend::open_bytes) reads it, and any other source
/// (a file, [`Parts`](pdf_raster::Parts)) by position, an object at a time,
/// with no copy of the whole.
///
/// What an open fails with says whose trouble it is:
/// [`BackendError::Locked`] where no password opened the file;
/// [`BackendError::Refused`] where the file can't be drawn whatever the
/// password, with [`Refusal::Security`] for an encryption the reader doesn't
/// open, [`Refusal::Source`] for bytes that didn't read, and
/// [`Refusal::Format`] for a file with no catalog or no page tree, its
/// source the [`Error`](crate::Error); and [`BackendError::Backend`] for the
/// rest, a cap the reader reads under among them. An unsupported
/// encryption's source is pdf-structure's
/// [`Unsupported`](pdf_structure::source::Unsupported).
///
/// ```
/// use std::sync::Arc;
/// use pdfs::prelude::*;
///
/// let mut doc = Document::new();
/// let page = doc.add_page(Rect::new(0.0, 0.0, 72.0, 72.0));
/// page.fill_color(Color::rgb8([20, 120, 220]));
/// page.rect(0.0, 0.0, 72.0, 72.0);
/// page.paint(Paint::Fill);
/// let bytes: Arc<[u8]> = Arc::from(doc.to_bytes().unwrap());
///
/// let backend: &dyn RenderBackend = &RustBackend::new();
/// let rendered = backend.open_bytes(bytes, None).unwrap();
/// let (pixmap, _) = rendered.render_page(0, &RenderOpts::dpi(72.0)).unwrap();
/// assert_eq!(pixmap.pixel_rgba8(36, 36), Some([20, 120, 220, 255]));
/// assert!(pixmap.to_png().unwrap().starts_with(b"\x89PNG\r\n\x1a\n"));
/// ```
#[derive(Clone)]
pub struct RustBackend {
    options: Options<'static>,
    fallback: Option<Arc<dyn FontFallback>>,
}

impl Default for RustBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for RustBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RustBackend")
            .field("options", &self.options)
            .field("has_fallback", &self.fallback.is_some())
            .finish()
    }
}

impl RustBackend {
    /// Creates a [`RustBackend`] with viewer-compatible [`Options::DEFAULT`]
    /// and built-in fallback glyphs.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            options: Options::DEFAULT,
            fallback: None,
        }
    }

    /// Configures reader [`Options`] (limits, recovery, rules, Flate backend).
    #[must_use]
    pub const fn with_options(mut self, options: Options<'static>) -> Self {
        self.options = options;
        self
    }

    /// Attaches a shared [`FontFallback`] provider for unembedded fonts.
    #[must_use]
    pub fn with_font_fallback(mut self, fallback: Arc<dyn FontFallback>) -> Self {
        self.fallback = Some(fallback);
        self
    }
}

impl RenderBackend for RustBackend {
    fn name(&self) -> &'static str {
        "pdf-raster"
    }

    fn open_bytes(
        &self,
        bytes: Arc<[u8]>,
        password: Option<&str>,
    ) -> Result<Box<dyn RenderedDoc>, BackendError> {
        self.open_bytes_with_passwords(bytes, password.as_slice())
    }

    fn open_bytes_with_passwords(
        &self,
        bytes: Arc<[u8]>,
        passwords: &[&str],
    ) -> Result<Box<dyn RenderedDoc>, BackendError> {
        self.open_with(bytes, passwords)
    }

    fn open_source(
        &self,
        source: Arc<dyn RenderSource>,
        password: Option<&str>,
    ) -> Result<Box<dyn RenderedDoc>, BackendError> {
        self.open_source_with_passwords(source, password.as_slice())
    }

    fn open_source_with_passwords(
        &self,
        source: Arc<dyn RenderSource>,
        passwords: &[&str],
    ) -> Result<Box<dyn RenderedDoc>, BackendError> {
        self.open_with(SourceBytes(source), passwords)
    }
}

impl RustBackend {
    /// `source` opened by the empty password or one of `passwords`, its page
    /// tree walked.
    fn open_with<S: ByteSource + 'static>(
        &self,
        source: S,
        passwords: &[&str],
    ) -> Result<Box<dyn RenderedDoc>, BackendError> {
        let pws: Vec<Password<'_>> = passwords.iter().copied().map(Password).collect();
        let mut opts = self.options;
        if !pws.is_empty() {
            opts.passwords = &pws;
        }
        if let Some(ref fb) = self.fallback {
            opts.font_fallback = Some(fb);
        }
        let pdf = opts.from_bytes(source).map_err(open_err)?;
        match pdf.encryption() {
            Encryption::Locked => return Err(BackendError::Locked),
            Encryption::Unsupported(unsupported) => {
                return Err(BackendError::refused(
                    Refusal::Security,
                    unsupported.clone(),
                ));
            }
            _ => {}
        }
        pdf.prime_pages().map_err(open_err)?;
        Ok(Box::new(pdf))
    }
}

/// A failed open, as a host matches on it: the file locked, the file refused
/// and why, or the reader's own failure (a cap among them).
fn open_err(err: Error) -> BackendError {
    // A cap or a limit is the reader's, not the file's refusal.
    let why = |why: Why| match why {
        Why::Source => Some(Refusal::Source),
        Why::Damaged => Some(Refusal::Format),
        _ => None,
    };
    let refusal = match &err {
        Error::Unreadable(u) | Error::Pages(pages::Error::Unreadable(u))
            if u.why == Why::Locked =>
        {
            return BackendError::Locked;
        }
        Error::Io(_) | Error::Read(read::Error::Io(_)) => Some(Refusal::Source),
        Error::Unreadable(u) | Error::Pages(pages::Error::Unreadable(u)) => why(u.why),
        Error::NoCatalog
        | Error::Pages(pages::Error::NoPages | pages::Error::Cycle | pages::Error::ParentLoop) => {
            Some(Refusal::Format)
        }
        _ => None,
    };
    match refusal {
        Some(reason) => BackendError::refused(reason, err),
        None => BackendError::custom(err),
    }
}

/// A [`RenderSource`] as the reader's byte source.
struct SourceBytes(Arc<dyn RenderSource>);

impl ByteSource for SourceBytes {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn whole(&self) -> Option<&[u8]> {
        self.0.whole()
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        self.0.read_at(range)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use pdf_fixtures::fixture::{Body, Xref, file};
    use pdf_objects::{Object, Stream, dict};
    use pdf_raster::{Parts, RenderOpts};

    use super::*;
    use crate::prelude::{Color, Document, Paint, Password, Protection, Rect};

    /// A two-page file, a blue page and a red one, with a stream no page
    /// draws of `padding` bytes that don't compress.
    fn two_pages(padding: usize) -> Vec<u8> {
        let mut doc = Document::new();
        let page = doc.add_page(Rect::new(0.0, 0.0, 72.0, 72.0));
        page.fill_color(Color::rgb8([20, 120, 220]));
        page.rect(0.0, 0.0, 72.0, 72.0);
        page.paint(Paint::Fill);
        let page = doc.add_page(Rect::new(0.0, 0.0, 36.0, 36.0));
        page.fill_color(Color::rgb8([220, 40, 20]));
        page.rect(0.0, 0.0, 36.0, 36.0);
        page.paint(Paint::Fill);
        let mut seed = 0x9E37_79B9_u32;
        let noise: Vec<u8> = (0..padding)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            })
            .collect();
        let unused = Stream::new(dict! { "Type" => Object::name("Unused") }, noise);
        doc.structure_mut().add(Object::from(unused));
        doc.to_bytes().unwrap()
    }

    fn split(bytes: &[u8], at: &[usize]) -> Parts {
        let mut parts = Vec::new();
        let mut from = 0;
        for &to in at {
            parts.push(Arc::from(&bytes[from..to]));
            from = to;
        }
        parts.push(Arc::from(&bytes[from..]));
        Parts::new(parts)
    }

    /// A source that counts what's read of it, and can fail every read.
    struct Counted {
        parts: Parts,
        longest: AtomicUsize,
        fail: bool,
    }

    impl RenderSource for Counted {
        fn len(&self) -> usize {
            self.parts.len()
        }

        fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
            if self.fail {
                return Err(io::Error::other("the disk went away"));
            }
            self.longest.fetch_max(range.len(), Ordering::SeqCst);
            self.parts.read_at(range)
        }
    }

    #[test]
    fn a_source_in_parts_opens_and_draws_as_its_bytes_joined() {
        let bytes = two_pages(64);
        let backend = RustBackend::new();
        let whole = backend
            .open_bytes(Arc::from(bytes.as_slice()), None)
            .unwrap();
        let opts = RenderOpts::dpi(72.0);
        // Split inside the header, inside an object and inside the trailer.
        let cuts = [
            [3, 9],
            [bytes.len() / 2, bytes.len() / 2 + 1],
            [0, bytes.len() - 7],
        ];
        for at in cuts {
            let doc = backend
                .open_source(Arc::new(split(&bytes, &at)), None)
                .unwrap();
            assert_eq!(doc.page_count(), 2);
            for page in 0..2 {
                assert_eq!(doc.page_geom(page).unwrap(), whole.page_geom(page).unwrap());
                let (drawn, _) = doc.render_page(page, &opts).unwrap();
                assert_eq!(
                    drawn,
                    whole.render_page(page, &opts).unwrap().0,
                    "{at:?}/{page}"
                );
            }
            let first = doc.render_page(0, &opts).unwrap().0;
            assert_eq!(first.pixel_rgba8(36, 36), Some([20, 120, 220, 255]));
        }
        // One part alone is bytes in memory, read as `open_bytes` reads them.
        let one = backend
            .open_source(Arc::new(split(&bytes, &[])), None)
            .unwrap();
        assert_eq!(
            one.render_page(1, &opts).unwrap(),
            whole.render_page(1, &opts).unwrap()
        );
    }

    #[test]
    fn a_source_read_by_position_is_never_read_whole() {
        // A stream no page draws is most of the file.
        let bytes = two_pages(1 << 20);
        let source = Arc::new(Counted {
            parts: split(&bytes, &[bytes.len() / 3]),
            longest: AtomicUsize::new(0),
            fail: false,
        });
        let dyn_source: Arc<dyn RenderSource> = source.clone();
        let doc = RustBackend::new().open_source(dyn_source, None).unwrap();
        let (drawn, _) = doc.render_page(0, &RenderOpts::dpi(72.0)).unwrap();
        assert_eq!(drawn.pixel_rgba8(36, 36), Some([20, 120, 220, 255]));
        let longest = source.longest.load(Ordering::SeqCst);
        assert!(longest < bytes.len() / 8, "{longest} of {}", bytes.len());
    }

    #[test]
    fn a_source_takes_passwords_as_bytes_do() {
        let mut doc = Document::new();
        doc.add_page(Rect::new(0.0, 0.0, 40.0, 40.0));
        let pw = Password("correct-pass").prepared().unwrap();
        doc.encrypt(&Protection::new(&pw)).unwrap();
        let bytes = doc.to_bytes().unwrap();
        let source = || -> Arc<dyn RenderSource> { Arc::new(split(&bytes, &[bytes.len() / 2])) };
        let backend = RustBackend::new();
        assert!(matches!(
            backend.open_source(source(), None),
            Err(BackendError::Locked)
        ));
        assert!(matches!(
            backend.open_source(source(), Some("wrong")),
            Err(BackendError::Locked)
        ));
        let opened = backend.open_source(source(), Some("correct-pass")).unwrap();
        assert_eq!(opened.page_count(), 1);
        let opened = backend
            .open_source_with_passwords(source(), &["wrong", "correct-pass"])
            .unwrap();
        assert_eq!(opened.page_count(), 1);
    }

    #[test]
    fn a_refused_file_says_why_apart_from_a_locked_one() {
        let backend = RustBackend::new();
        let refusal = |bytes: &[u8]| {
            let err = backend.open_bytes(Arc::from(bytes), None).err().unwrap();
            let BackendError::Refused { reason, source } = &err else {
                panic!("not refused: {err}");
            };
            (*reason, source.to_string())
        };

        // Not a PDF, and a PDF with a catalog and no pages: its format.
        assert_eq!(refusal(b"not a pdf at all").0, Refusal::Format);
        let no_pages = file(
            "1.7",
            &[(
                1,
                Body::Object(Object::from(dict! { "Type" => Object::name("Catalog") })),
            )],
            dict! { "Root" => Object::reference(1) },
            Xref::Table,
        );
        let (reason, words) = refusal(&no_pages);
        assert_eq!(
            (reason, words.as_str()),
            (Refusal::Format, "the file has no pages")
        );

        // Another security handler's: its security, whatever the password.
        let other_handler = file(
            "1.7",
            &[
                (
                    1,
                    Body::Object(Object::from(dict! { "Type" => Object::name("Catalog") })),
                ),
                (
                    2,
                    Body::Object(Object::from(dict! {
                        "Filter" => Object::name("Vendor.Handler"),
                        "V" => 1,
                    })),
                ),
            ],
            dict! { "Root" => Object::reference(1), "Encrypt" => Object::reference(2) },
            Xref::Table,
        );
        assert_eq!(refusal(&other_handler).0, Refusal::Security);
        let with_password = backend
            .open_bytes(Arc::from(other_handler.as_slice()), Some("any"))
            .err()
            .unwrap();
        assert_eq!(with_password.refusal(), Some(Refusal::Security));
        let BackendError::Refused { source, .. } = &with_password else {
            unreachable!()
        };
        assert!(
            source
                .downcast_ref::<pdf_structure::source::Unsupported>()
                .is_some()
        );

        // A source that doesn't read: its source, the reader's error kept.
        let bytes = two_pages(64);
        let failing = Arc::new(Counted {
            parts: split(&bytes, &[bytes.len() / 2]),
            longest: AtomicUsize::new(0),
            fail: true,
        });
        let err = backend.open_source(failing, None).err().unwrap();
        assert_eq!(err.refusal(), Some(Refusal::Source));
        let BackendError::Refused { source, .. } = &err else {
            unreachable!()
        };
        assert!(matches!(
            source.downcast_ref::<Error>(),
            Some(Error::Read(read::Error::Io(_)))
        ));
    }
}
