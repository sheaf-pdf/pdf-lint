//! Pure-Rust [`RenderBackend`] implementation over [`crate::Pdf`].

use std::fmt;
use std::sync::Arc;

use pdf_crypt::Password;
use pdf_raster::{BackendError, FontFallback, RenderBackend, RenderedDoc};
use pdf_structure::Encryption;

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
        let pws: Vec<Password<'_>> = passwords.iter().copied().map(Password).collect();
        let mut opts = self.options;
        if !pws.is_empty() {
            opts.passwords = &pws;
        }
        if let Some(ref fb) = self.fallback {
            opts.font_fallback = Some(fb);
        }
        let pdf = opts.from_bytes(bytes).map_err(BackendError::custom)?;
        if *pdf.encryption() == Encryption::Locked {
            return Err(BackendError::Locked);
        }
        pdf.prime_pages().map_err(BackendError::custom)?;
        Ok(Box::new(pdf))
    }
}
