//! Thin `#![forbid(unsafe_code)]` adapter over [`pdfium_render`] (`0.9.6`,
//! `thread_safe`), enabled by the additive `pdfium` Cargo feature.
//!
//! Lets applications such as `sheaf` (as well as callers migrating incrementally
//! or comparing outputs in shadow mode) share one [`pdfium_render::prelude::Pdfium`]
//! instance across the process and swap between `pdfs::RustBackend` and
//! [`PdfiumBackend`] behind `&dyn RenderBackend`.

mod backend;

pub use backend::{Error, Pdfium, PdfiumBackend, RenderFlags, RenderOptions};
pub use pdfium_render;
