//! Thin `#![forbid(unsafe_code)]` [`RenderBackend`](crate::RenderBackend)
//! adapter over [`pdfium_render`] (`=0.9.6`, `thread_safe`, `pdfium_8125`),
//! enabled by the additive `pdfium` Cargo feature.
//!
//! Lets callers share one [`pdfium_render::prelude::Pdfium`] instance across
//! the process and swap between `pdfs::Pdf` / `pdfs::RustBackend` and
//! [`PdfiumBackend`] behind `&dyn RenderBackend`.
//!
//! # Dynamic Linking & Semver Notes
//!
//! - `pdf-raster/pdfium` binds `libpdfium` dynamically at runtime (it is not
//!   compatible with `pdfium-render/static`).
//! - `pdfium-render` `=0.9.6` binds PDFium's `8125` C ABI (`pdfium_8125`, the
//!   latest ABI feature shipped in `0.9.6`), which is ABI-compatible with the
//!   workspace's pinned `chromium/8152` `libpdfium` build; pinning `=0.9.6` and
//!   naming `pdfium_8125` explicitly prevents a patch upgrade from advancing
//!   eager symbol bindings past `8152`.
//! - This module re-exports [`pdfium_render`], so upgrades to `pdfium_render`
//!   are semver-significant when the `pdfium` feature is enabled.

mod backend;

pub use backend::{Error, PdfiumBackend};
pub use pdfium_render;
