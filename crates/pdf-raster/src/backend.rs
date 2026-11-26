//! Layer 2 document-level rendering traits ([`RenderBackend`], [`RenderedDoc`])
//! for swappable multi-engine execution (`pdfs::RustBackend`,
//! `pdf-raster-pdfium::PdfiumBackend`, and viewer/CLI engine pools).

use std::sync::Arc;

use pdf_gfx::PageGeom;

use crate::error::Error;
use crate::opts::{RenderOpts, RenderSummary};
use crate::pixmap::Pixmap;

/// Error returned when opening a document or rendering a page through a
/// [`RenderBackend`] / [`RenderedDoc`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BackendError {
    /// Underlying rasterization, viewport, or resource-limit failure.
    #[error(transparent)]
    Raster(#[from] Error),
    /// Document is encrypted and none of the supplied passwords opened it.
    #[error("PDF document is encrypted and locked")]
    Locked,
    /// Requested zero-based `page_index` is out of range (`>= page_count`).
    #[error("page index {page_index} out of bounds (page count: {page_count})")]
    PageOutOfBounds {
        /// Requested zero-based page index.
        page_index: usize,
        /// Total number of pages in the document.
        page_count: usize,
    },
    /// Backend-specific document open, I/O, or external engine error.
    #[error("{0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl BackendError {
    /// Wraps an arbitrary `Send + Sync + 'static` backend error.
    #[must_use]
    pub fn custom(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Backend(Box::new(err))
    }
}

/// An opened PDF document handle bound to a [`RenderBackend`], safe to share
/// and query concurrently across threads (`Send + Sync`).
pub trait RenderedDoc: Send + Sync {
    /// Number of pages in the document.
    fn page_count(&self) -> usize;

    /// Returns the 2D geometry (`MediaBox`, `CropBox`, `Rotation`, `UserUnit`)
    /// of zero-based `page_index`.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError::PageOutOfBounds`] if `page_index >= self.page_count()`,
    /// or [`BackendError`] if page metadata cannot be read.
    fn page_geom(&self, page_index: usize) -> Result<PageGeom, BackendError>;

    /// Rasterizes zero-based `page_index` under `opts`.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError`] on invalid viewports, out-of-bounds `page_index`,
    /// exceeded [`crate::Limits`], or unreadable content.
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
    /// does not unlock it, or [`BackendError`] if the document cannot be opened.
    fn open_bytes(
        &self,
        bytes: Arc<[u8]>,
        password: Option<&str>,
    ) -> Result<Box<dyn RenderedDoc>, BackendError>;
}
