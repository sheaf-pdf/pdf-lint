//! Category-selectable `0–100` visual and ISO 32000-2 conformance rubric,
//! straight-alpha [`RgbaImage`] helpers, and [`RenderBackend`] differential
//! comparison ([`compare_backends`]).
//!
//! All items in this module are 100% pure safe Rust (`#![forbid(unsafe_code)]`)
//! with no external C or FFI dependencies, so callers can score [`Pixmap`] pairs
//! or run shadow-mode comparisons between any two [`RenderBackend`]s even when
//! the `pdfium` feature is disabled.

use std::sync::Arc;

use pdf_raster::{BackendError, Pixmap, RenderBackend, RenderOpts};

mod image;
mod score;

pub use image::{MAX_PIXELS, RgbaImage};
pub use score::{
    BaselineRow, CaseEvaluation, CaseOutcome, DriftReport, Grade, RubricCategories, RubricConfig,
    RubricProfile, VisualScore, evaluate_case, format_baseline_tsv, parse_baseline_tsv,
    render_diff_strip, score_images,
};

/// Errors produced by [`RgbaImage`], [`score_images`], [`score_pixmaps`],
/// [`evaluate_case`], or [`render_diff_strip`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Requested image dimensions were zero or exceeded [`MAX_PIXELS`].
    #[error("invalid or unsupported image dimensions {width}x{height}")]
    InvalidDimensions {
        /// Requested width in pixels.
        width: u32,
        /// Requested height in pixels.
        height: u32,
    },
    /// Two images passed to [`score_images`] or [`render_diff_strip`] had
    /// different pixel dimensions.
    #[error(
        "image dimensions differ: candidate is {cand_w}x{cand_h}, reference is {ref_w}x{ref_h}"
    )]
    DimensionMismatch {
        /// Candidate width.
        cand_w: u32,
        /// Candidate height.
        cand_h: u32,
        /// Reference width.
        ref_w: u32,
        /// Reference height.
        ref_h: u32,
    },
}

impl RgbaImage {
    /// Converts any [`Pixmap`] (`Rgba8`, `Bgra8`, or `Rgba8Premul`) into an
    /// unpremultiplied straight-alpha sRGB [`RgbaImage`] for rubric scoring.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidDimensions`] if `pixmap` exceeds [`MAX_PIXELS`].
    pub fn from_pixmap(pixmap: &Pixmap) -> Result<Self, Error> {
        Self::from_rgba(pixmap.width(), pixmap.height(), pixmap.to_rgba8_vec())
    }
}

/// Evaluates the `0.0..=100.0` perceptual [`VisualScore`] directly between two
/// [`Pixmap`]s.
///
/// # Errors
///
/// Returns [`Error`] if dimensions mismatch or exceed [`MAX_PIXELS`].
pub fn score_pixmaps(candidate: &Pixmap, reference: &Pixmap) -> Result<VisualScore, Error> {
    let cand = RgbaImage::from_pixmap(candidate)?;
    let refr = RgbaImage::from_pixmap(reference)?;
    score_images(&cand, &refr)
}

/// Renders `page_index` of `bytes` through both `candidate` (e.g.
/// `pdfs::RustBackend`) and `oracle` (e.g. `PdfiumBackend`) under `opts`, and
/// evaluates the pair against `rubric`.
///
/// # Errors
///
/// Returns [`BackendError`] if either backend fails to open or render the page,
/// or if the resulting bitmaps cannot be scored.
pub fn compare_backends(
    candidate: &dyn RenderBackend,
    oracle: &dyn RenderBackend,
    bytes: Arc<[u8]>,
    password: Option<&str>,
    page_index: usize,
    opts: &RenderOpts,
    rubric: &RubricConfig,
) -> Result<CaseOutcome, BackendError> {
    let cand_doc = candidate.open_bytes(bytes.clone(), password)?;
    let orac_doc = oracle.open_bytes(bytes, password)?;
    let (cand_pix, _) = cand_doc.render_page(page_index, opts)?;
    let (orac_pix, _) = orac_doc.render_page(page_index, opts)?;

    let cand_rgba = RgbaImage::from_pixmap(&cand_pix).map_err(BackendError::custom)?;
    let orac_rgba = RgbaImage::from_pixmap(&orac_pix).map_err(BackendError::custom)?;

    evaluate_case(
        rubric,
        &CaseEvaluation {
            candidate_pdfium: Some(&cand_rgba),
            candidate_iso: Some(&cand_rgba),
            pdfium_reference: Some(&orac_rgba),
            iso_reference: Some(&orac_rgba),
            iso_fault_count: 0,
            write_lint_errors: 0,
        },
    )
    .map_err(BackendError::custom)
}
