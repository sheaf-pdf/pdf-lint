//! Thin `#![forbid(unsafe_code)]` [`RenderBackend`](crate::RenderBackend)
//! adapter over [`pdfium_render`] (`0.9`, `thread_safe`), enabled by the
//! additive `pdfium` Cargo feature.
//!
//! Lets callers share one [`pdfium_render::prelude::Pdfium`] instance across
//! the process and swap between `pdfs::Pdf` / `pdfs::RustBackend` and
//! [`PdfiumBackend`] behind `&dyn RenderBackend`.
//!
//! # Dynamic Linking & Semver Notes
//!
//! - `pdf-raster/pdfium` binds `libpdfium` dynamically at runtime (it is not
//!   compatible with `pdfium-render/static`).
//! - This module re-exports [`pdfium_render`] (`0.9`), so breaking upgrades to
//!   `pdfium_render` are semver-significant when the `pdfium` feature is
//!   enabled.

mod backend;

pub use backend::{Error, PdfiumBackend};
pub use pdfium_render;
