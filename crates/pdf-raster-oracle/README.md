# pdf-raster-oracle

Internal (`publish = false`) workspace crate housing:

- **`rubric`:** Pure-Rust `0–100` perceptual visual and ISO 32000-2 conformance scoring (`score_images`, `score_pixmaps`, `evaluate_case`, `compare_backends`, TSV baseline & drift reporting).
- **PDFium (`as of chromium/8152`) test discovery & oracle helpers:** Locates `target/pdfium-8152` or `$PDF_RASTER_PDFIUM` / `$PDF_LINT_PDFIUM` / `$PDFIUM_LIB_DIR` (`require_pdfium_backend`, `require_pdfium`), keeping working-directory library probing and upward dev-dependencies out of published crates.

Run the live PDFium comparison suites across color spaces, image codecs, page rendering, and backend parity via:

```sh
just test-pdfium
```
