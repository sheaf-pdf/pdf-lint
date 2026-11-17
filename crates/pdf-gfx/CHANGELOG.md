# Changelog

## 0.1.0-alpha.1 (unreleased)

First release. Until `0.1.0` stabilizes, alpha releases may include breaking API changes documented here; depend on the exact version (`=0.1.0-alpha.1`).

### Added

- `Point`, `Rect`, `Quad`, `Matrix`, `Rotation`, `PageSize`, and `PageGeom`: 2D PDF geometry, affine matrix composition and inversion, axis-aligned rectangle placement (`Matrix::onto`), and closed-form transforms between PDF PageSpace and viewer ViewSpace (`PageGeom::to_view`, `PageGeom::from_view`, `PageGeom::from_page`).
- `interpret` and `interpret_with`: bounded zero-I/O graphics and text state interpreter over `Concat` and `Resolve + Streams`, matching PDFium's content interpretation rules as of chromium/8152 (`Forms::Descend` and `Forms::Skip`, `Opts`, `Wants`, `Summary`).
- `State` and `TextState`: thread-safe (`Send + Sync`) graphics state tracking across `q`/`Q`/`cm`, line style, `DashPattern`, `RenderingIntent`, `ActiveColorSpace`, `ColorValues`, `BlendState`, `SoftMask`, and `ExtGState` (`gs`) dictionaries.
- `PathDrawn`, `Seg`, `Clip`, `ClipPart`, and `ClipParts`: path construction, fill/stroke bounds, `W`/`W*` path clips (including PDFium's single-point clip and auto-merge rules), text rendering modes `4..=7` (`Tr`) clipping, and Form XObject `/BBox` clips.
- `TextShow`, `Glyphs`, and `Glyph`: lazy glyph placement over `pdf-fonts` across `Tj`, `TJ`, `'`, and `"`, with horizontal and vertical writing modes, `char_box` outline union, pen anchors, glyph matrices (`Glyph::tm`), and decoded Unicode text (`Glyph::text`).
- `FormUse`, `UseId`, `Refused`, `ImageDrawn`, `ImageOf`, and `ShadingDrawn`: nested Form XObject entry/exit/refusal events, placed XObject and inline image metadata with effective horizontal/vertical DPI (`ImageDrawn::dpi`, `ImageDrawn::dpi_with_unit`), and `sh` shading bounds.
- `Mark`, `MarkProps`, `Marks`, and `MarksIter`: marked-content sequence (`BMC`/`BDC`/`EMC`) and point (`MP`/`DP`) tracking across Form XObject boundaries with `/MCID`, `/ActualText`, `/OC`, and `/Artifact` helpers.
- `encode`, `encode_with`, and `EncodedPage`: content-stream and Form XObject re-encoder that rewrites relative line-moving text operators (`Td`, `TD`, `T*`, `'`, `"`) into position-independent `Tm` + `Td` pairs while preserving exact glyph, path, clip, and mark geometry.
- `Canvas` and `TextPiece`: high-level vector, inset border, rounded-rectangle, circle/ellipse, clip, placed image/Form XObject, marked-content, and `TJ`-kerned text builder over `ContentWriter`.
- `TextPage`, `TextPageOpts`, `PageChar`, `TextLine`, `SearchOpts`, and `TextMatch` (`text-layout` feature): `CPDF_TextPage`-compatible reading-order assembly with synthetic spaces/newlines, faux-bold deduplication, `/ActualText` replacement, `unicode-bidi` RTL reordering, soft-hyphen wrap detection, hit-testing, rectangular text extraction, merged selection quads, and search.
- `layout_text`, `Canvas::draw_layout`, `Canvas::text_box`, `LayoutFont`, `FontEncoder`, `TextBoxStyle`, `TextAlign`, `VerticalAlign`, `WrapMode`, `LaidOutText`, `LaidOutLine`, and `UnencodableChar` (`text-layout` feature): word/character wrapping, alignment, justification, automatic font sizing, and comb-cell layout over `Std14` and `Font`.
- `Limits`, `Cap`, `Error`, `Unwritable`, and `Fault`: per-page interpretation budgets and non-fatal diagnostics reported via `pdf-objects`' `fault::Sink`.
- Cargo features `programs` (default, forwarding `pdf-fonts/programs`), `text-layout` (default, enabling `TextPage` and multiline `Canvas` layout), `predefined-cmaps` (forwarding `pdf-fonts/predefined-cmaps`), and `tracing` (numeric phase spans).

### Notes

- Re-exports `pdf-objects`, `pdf-color`, `pdf-content-streams`, `pdf-semantics`, and `pdf-fonts` at the crate root. Requires Rust 1.92.
- Never panics on untrusted input, and `Debug` implementations and `tracing` spans never include file plaintext, resource names, tag names, or stream bytes.
- `Point`, `Rect`, `Quad`, `Matrix`, `Rotation`, `Seg`, `Forms`, `MarkProps`, and `TextPiece` are exhaustive on purpose; other public enums and configuration/metadata structs are `#[non_exhaustive]` so new variants and fields can be added without breaking callers.
