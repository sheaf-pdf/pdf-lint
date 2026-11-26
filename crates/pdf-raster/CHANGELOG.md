# Changelog

All notable changes to `pdf-raster` are documented in this file.

## [Unreleased]

### Added

- Initial `0.1.0-alpha.1` release: zero-I/O pure-Rust 2D PDF page, Form XObject,
  annotation, and AcroForm widget rasterizer (`render_page`, `render_page_with`,
  `render_content`, `render_content_with`) supporting paths, clips, embedded
  TrueType/CFF/Type 1 & Type 3 fonts, fallback fonts, Image XObjects & inline
  images, transparency groups, blend modes, soft masks, tiling patterns,
  shadings, and optional content groups under both `Rules::Iso` and
  `Rules::Pdfium` (as of chromium/8152), object-safe `RenderBackend`,
  `RenderedDoc`, and `BackendError` traits, the `rubric` visual/conformance
  scorer (`compare_backends`), and an optional `pdfium` feature providing a thin
  `PdfiumBackend` adapter over `pdfium-render` `0.9.6`.
