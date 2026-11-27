# Changelog

## 0.1.0-alpha.1 (unreleased)

First release. It's an alpha: any alpha may change the API, and each entry here says what changed and how to update. Depend on the exact version, `=0.1.0-alpha.1`; a plain `0.1.0-alpha.1` lets cargo take later alphas too. Its integration tests don't build from the package: they take their test files from `pdf-fixtures`, which is never published.

### Added

- `Renderer`, `render_page`, `render_page_with`, `render_content`, and `render_content_with`: zero-I/O pure-Rust 2D PDF page, Form XObject, annotation, and AcroForm widget rasterizer over any `Resolve + Streams` provider, supporting both strict ISO 32000-2 (`Rules::Iso`) and viewer (`Rules::Pdfium`, as of chromium/8152) policies.
- `Viewport` (`Page`, `Tile`, `Dpi`, `Custom`), `ResolvedViewport`, `RenderOpts` (`dpi`, `page_width`, `with_viewport`, `with_stop`), `Draw`, `Appearances`, `RenderMedium`, `AntiAlias`, and `RenderSummary`: closed-form viewport resolution, overlay selection, cooperative cancellation (`Error::Cancelled`), and honest approximation reporting (`RenderSummary::approximated`).
- `Pixmap` and `PixelFormat` (`Rgba8`, `Bgra8`, `Rgba8Premul`): upfront-budgeted pixel buffers with redacted `Debug` formatting, straight-alpha pixel access (`pixel_rgba8`, `to_rgba8_vec`, `to_rgba_image`, `into_rgba_image`), and zero-dependency PNG encoding (`to_png`).
- `FontFallback`, `FontQuery` (`std14`, `style`, `base_font`, `family_name`, `cid_ordering`, `sample_char`), and `RenderCache`: thread-safe cross-page font and fallback-program cache with buffer-pointer deduplication, plus built-in Latin/ZapfDingbats legibility contours.
- `RenderBackend`, `RenderedDoc`, and `BackendError`: object-safe (`Send + Sync`) document rendering traits (including `open_bytes_with_passwords`) for swappable multi-engine execution.
- `Limits`, `LimitKind`, `Error`, and `Fault`: upfront resource caps (`max_width`, `max_height`, `max_pixels`, `max_pixmap_bytes`, `max_layer_depth`, `max_tile_side`, `max_tile_pixels`, `max_shading_samples`, `max_scene_bytes`, `max_annots`) and typed non-fatal `Sink<Fault>` diagnostics.
- Optional `pdfium` feature: `pdfium` module and `PdfiumBackend`, a `#![forbid(unsafe_code)]` dynamic-library `RenderBackend` adapter over `pdfium-render` `=0.9.6` (`thread_safe`, `pdfium_8125` ABI).
- Optional `programs` (default), `text-layout` (default), `icc` (default), `predefined-cmaps`, `simd`, `zlib-rs`, and `tracing` features.
