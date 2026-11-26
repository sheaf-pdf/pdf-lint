//! Pure-Rust [`RenderBackend`] implementation over [`Pdf`].

use std::fmt;
use std::sync::Arc;

use pdf_crypt::Password;
use pdf_gfx::PageGeom;
use pdf_raster::{
    BackendError, FontFallback, Pixmap, RenderBackend, RenderOpts, RenderSummary, RenderedDoc,
};
use pdf_structure::Encryption;

use crate::open::Options;
use crate::pdf::Pdf;

/// Pure-Rust [`RenderBackend`] powered by `pdfs` and `pdf-raster`.
///
/// Opens documents lazily over `Arc<[u8]>`, unlocks password-protected files
/// via `pdf-crypt`, caches parsed fonts and fallback faces across pages on
/// [`Pdf::render_cache`], and rasterizes pages or tiles concurrently across
/// threads (`Send + Sync`, `#![forbid(unsafe_code)]`).
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

struct RustRenderedDoc {
    pdf: Pdf<Arc<[u8]>>,
    page_count: usize,
    fallback: Option<Arc<dyn FontFallback>>,
}

impl RenderedDoc for RustRenderedDoc {
    fn page_count(&self) -> usize {
        self.page_count
    }

    fn page_geom(&self, page_index: usize) -> Result<PageGeom, BackendError> {
        let pages = self.pdf.pages().map_err(BackendError::custom)?;
        let Some(page) = pages.get(page_index) else {
            return Err(BackendError::PageOutOfBounds {
                page_index,
                page_count: pages.len(),
            });
        };
        Ok(page.geom())
    }

    fn render_page(
        &self,
        page_index: usize,
        opts: &RenderOpts,
    ) -> Result<(Pixmap, RenderSummary), BackendError> {
        let pages = self.pdf.pages().map_err(BackendError::custom)?;
        let Some(page) = pages.get(page_index) else {
            return Err(BackendError::PageOutOfBounds {
                page_index,
                page_count: pages.len(),
            });
        };
        let mut effective = opts.clone();
        // Enforce the narrower of the backend's reader caps and `opts.limits`
        // unless the caller explicitly customized `opts.limits` tighter.
        if opts.limits == pdf_raster::Limits::DEFAULT {
            effective.limits = self.pdf.limits().reading_raster();
        }
        let catalog = self.pdf.catalog().ok();
        pdf_raster::render_page(
            self.pdf.reading(),
            catalog,
            page.attributes(),
            self.pdf.render_cache(),
            self.fallback.as_deref(),
            &effective,
        )
        .map_err(BackendError::Raster)
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
        let pws = password.map(|s| [Password(s)]);
        let mut opts = self.options;
        if let Some(ref arr) = pws {
            opts.passwords = arr;
        }
        let pdf = opts.from_bytes(bytes).map_err(BackendError::custom)?;
        if *pdf.encryption() == Encryption::Locked {
            return Err(BackendError::Locked);
        }
        let page_count = pdf.pages().map_err(BackendError::custom)?.len();
        Ok(Box::new(RustRenderedDoc {
            pdf,
            page_count,
            fallback: self.fallback.clone(),
        }))
    }
}
