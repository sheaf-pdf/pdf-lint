# Changelog

## 0.1.0 (unreleased)

- **Encodings.** `Encoding` (`#[non_exhaustive]`): StandardEncoding, WinAnsiEncoding, MacRomanEncoding, and Symbol's and ZapfDingbats' own, each a table of its own, not read from the metrics, with `named`, `glyph` and `text`. `cp1252`, Windows-1252's character for a code.
- **Glyph names.** `glyph_text`, by the Adobe Glyph List's rules, and `name_code_point`, as FreeType's glyph name list reads a name, which PDFium copies. The two lists are in `assets`, unchanged.
- **The standard 14.** `Std14`, with `ALL`, `name`, `named`, by PDFium's names as of chromium/8125, `metrics`, `glyph_width`, `glyph_box`, `drawn_otherwise`, `afm_text` and `NOTICE`. The metrics are Adobe's, in `assets/afm`, with their kerning data and blank lines removed, each file saying so and carrying Adobe's notice. `FontMetrics` and `Rect`, what they're given in.
- **Type 1 programs.** `type1::scan` and `Charstrings`: a program's charstrings by glyph name, read as read-fonts reads it, under `Limits::proc_depth`, `ints_ahead` and `type1_work`.
- **Predefined CMaps.** `predefined-cmaps`, a feature on hayro-cmap, which needs Rust 1.87.
- `Dict`, `Object`, `ObjectId`, `Resolve`, `Span`, `Unreadable` and `Why` are pdf-objects', re-exported.
