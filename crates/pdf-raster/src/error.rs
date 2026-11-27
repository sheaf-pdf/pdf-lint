//! Hard errors returned when rasterization cannot proceed.

use pdf_objects::Unreadable;

use crate::limits::LimitKind;

/// Hard failure returned by [`crate::Renderer`], [`crate::render_page`],
/// [`crate::render_page_with`], [`crate::render_content`], [`crate::Pixmap`], or
/// [`crate::Viewport::resolve`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Underlying object/stream provider failed (`Locked`, `Capped`, `Source`,
    /// or `Detached`).
    #[error("unreadable PDF object or stream: {0}")]
    Unreadable(#[from] Unreadable),
    /// Requested viewport dimensions, DPI, or tile bounds were invalid (e.g.
    /// zero width/height, non-finite DPI, or singular matrix).
    #[error("invalid viewport: {0}")]
    InvalidViewport(&'static str),
    /// Caller-supplied pixel buffer length did not match `width * height * 4`.
    #[error("pixmap buffer length {actual} does not match expected {expected} bytes")]
    BufferLength {
        /// Actual byte slice length supplied.
        actual: usize,
        /// Expected `width * height * 4` byte length.
        expected: usize,
    },
    /// A configured [`crate::Limits`] resource cap was exceeded.
    #[error("rasterization exceeded resource limit: {0}")]
    Limit(LimitKind),
    /// Rasterization was cooperatively aborted via [`crate::RenderOpts::stop`].
    #[error("rasterization cancelled")]
    Cancelled,
    /// Page-tree inspection failed.
    #[error("PDF page-tree error: {0}")]
    Pages(#[from] pdf_semantics::pages::Error),
    /// Content-stream or Form XObject interpretation failed.
    #[error("PDF graphics interpretation error: {0}")]
    Gfx(#[from] pdf_gfx::Error),
    /// Annotation reading failed due to an unreadable object or hard cap.
    #[error("PDF annotation error: {0}")]
    Annot(#[from] pdf_annots::Error),
    /// Form tree reading failed due to an unreadable object or hard cap.
    #[error("PDF form error: {0}")]
    Form(#[from] pdf_forms::Error),
    /// Internal software rasterizer panicked inside a `catch_unwind` guard.
    #[error("internal 2D rasterizer error")]
    RasterizerPanic,
}
