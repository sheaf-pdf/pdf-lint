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

pub use image::{MAX_PIXELS, RgbaImage, RgbaImageExt};
pub use score::{
    BaselineRow, CaseEvaluation, CaseOutcome, DriftReport, Grade, ParseError, RubricCategories,
    RubricConfig, RubricProfile, VisualScore, evaluate_case, format_baseline_tsv,
    parse_baseline_tsv, render_diff_strip, score_images, score_non_uniform_images,
};

/// Errors produced by [`RgbaImageExt`], [`score_images`],
/// [`score_non_uniform_images`], [`score_pixmaps`], [`compare_backends`],
/// [`evaluate_case`], or [`render_diff_strip`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
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
    /// Reference image was completely uniform (all pixels identical), guarding
    /// against vacuous `100.0 Exact` scores on blank renders.
    #[error("reference image is uniform (blank render guard)")]
    UniformReference,
    /// Visual comparison score fell below the required floor.
    #[error("visual score {score:.2} ({grade}) fell below floor {floor:.2}")]
    BelowFloor {
        /// Measured visual score (`0.0..=100.0`).
        score: f32,
        /// Qualitative grade band.
        grade: Grade,
        /// Required minimum score.
        floor: f32,
    },
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

/// Renders `page_index` of `bytes` once through `candidate` (e.g.
/// `pdfs::RustBackend`) and once through `oracle` (e.g. `PdfiumBackend`) under
/// `opts`, scores the pair directly via [`score_images`], and requires
/// `score.score >= visual_floor`.
///
/// Unlike [`evaluate_case`], this function evaluates only direct visual parity
/// between the two [`RenderBackend`]s (since `RenderBackend::render_page` does
/// not run a separate `Rules::Iso` pass or collect ISO read faults).
///
/// # Errors
///
/// Returns [`BackendError`] if either backend fails to open or render the page,
/// if dimensions mismatch, or if `score.score < visual_floor`
/// ([`Error::BelowFloor`]).
pub fn compare_backends(
    candidate: &dyn RenderBackend,
    oracle: &dyn RenderBackend,
    bytes: Arc<[u8]>,
    password: Option<&str>,
    page_index: usize,
    opts: &RenderOpts,
    visual_floor: f32,
) -> Result<VisualScore, BackendError> {
    let cand_doc = candidate.open_bytes(bytes.clone(), password)?;
    let orac_doc = oracle.open_bytes(bytes, password)?;
    let (cand_pix, _) = cand_doc.render_page(page_index, opts)?;
    let (orac_pix, _) = orac_doc.render_page(page_index, opts)?;

    let cand_rgba = RgbaImage::from_pixmap(&cand_pix).map_err(BackendError::custom)?;
    let orac_rgba = RgbaImage::from_pixmap(&orac_pix).map_err(BackendError::custom)?;

    let score = score_images(&cand_rgba, &orac_rgba).map_err(BackendError::custom)?;
    if score.score < visual_floor {
        return Err(BackendError::custom(Error::BelowFloor {
            score: score.score,
            grade: score.grade,
            floor: visual_floor,
        }));
    }
    Ok(score)
}
