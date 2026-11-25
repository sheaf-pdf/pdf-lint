//! Runtime-loaded PDFium (`chromium/8152`) rendering oracle and category-selectable `0–100`
//! visual + ISO 32000-2 conformance rubric for workspace test suites.
//!
//! This crate is internal to the workspace (`publish = false`). Published crates (`pdf-color`,
//! and future `pdf-images` / `pdf-raster`) depend on it by `path` alone in `[dev-dependencies]`
//! so published packages remain 100% pure Rust with `#![forbid(unsafe_code)]`.
//!
//! # Architecture
//!
//! - **[`Pdfium`] (`ffi`):** Loads `libpdfium` on demand via `libloading` (`$PDF_LINT_PDFIUM`,
//!   `$PDFIUM_LIB_DIR`, or `<workspace>/target/pdfium-8152/lib/libpdfium.so`) and serializes every
//!   C FFI call behind a single process-wide `Mutex`, making [`Pdfium`] safely `Send + Sync` under
//!   parallel `cargo-nextest` / `cargo test` execution.
//! - **[`RgbaImage`] (`image`):** Safe, unpremultiplied sRGB `RGBA8` buffer with redacted `Debug`
//!   formatting, rectangle drawing for swatch-strip synthesis, and deterministic backdrop
//!   compositing.
//! - **[`rubric`]:** Decomposed `0–100` perceptual visual scorer in Oklab (`struct_pen`,
//!   `color_pen`, `edge_pen`) with $3 \times 3$ shift-aware edge tolerance and $16 \times 16$
//!   worst-tile pooling, paired with toggleable [`RubricCategories`] and [`RubricProfile`] presets
//!   (`ConformingCore`, `PdfiumWild`, `IsoNormative`, `WriteAndSanitize`), TSV baseline drift
//!   tracking, and 4-up diagnostic diff rendering.

pub mod ffi;
pub mod image;
pub mod rubric;

pub use ffi::{Error, Pdfium, RenderFlags, RenderOptions};
pub use image::RgbaImage;
pub use rubric::{
    BaselineRow, CaseEvaluation, CaseOutcome, DriftReport, Grade, RubricCategories, RubricConfig,
    RubricProfile, VisualScore, evaluate_case, format_baseline_tsv, parse_baseline_tsv,
    render_diff_strip, score_images,
};

#[cfg(test)]
#[forbid(unsafe_code)]
mod tests {
    use super::*;

    #[test]
    fn public_types_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Pdfium>();
        assert_send_sync::<RenderFlags>();
        assert_send_sync::<RenderOptions>();
        assert_send_sync::<Error>();
        assert_send_sync::<RgbaImage>();
        assert_send_sync::<VisualScore>();
        assert_send_sync::<Grade>();
        assert_send_sync::<RubricCategories>();
        assert_send_sync::<RubricProfile>();
        assert_send_sync::<RubricConfig>();
        assert_send_sync::<BaselineRow>();
    }
}
