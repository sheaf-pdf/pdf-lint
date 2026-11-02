# Changelog

## 0.1.0 (unreleased)

- **A file's font, as read.** `Font::read`, over a `Resolve` and `Streams`, and what it gives: `id`, `kind` (`FontKind`, `Descendant`), `is_vertical`, `approx` (`FontApprox`), `program` (`FontProgram`, none yet), `cost`, `codes`, `word_spaced`, `cid`, `width`, `width_guessed`, `width_guessed_in`, `byte_widths`, `vertical` (`VerticalMetrics`), `metrics`, `glyph_box`, `char_box`, `charge_outlines`, `outlined`, `code_text`, `text`, `extent`. Type 1, MMType1, TrueType, Type 3 and Type 0 fonts, each as PDFium reads it; a standard 14 font by Adobe's metrics; embedded CMaps over those `/UseCMap` names; `/ToUnicode`; predefined CMaps with `predefined-cmaps`.
- **A stream's data.** `Streams`, `Decoded`, `With`, a resolver and a function as one, and, with the `structure` feature, `Streams` for pdf-structure's `&Reading`. A font refuses data past what it asked for.
- **Errors and faults.** `Error`, `TooLarge` or `Unreadable`; `Sink`, `Place` and `Fault`, as what's passed over is told, the same read whatever the sink.
- **Each font read once.** `Fonts` (`for_text`, `get`, `stock`, `scope`), `FontScope` (`font`, `stock`, `get`), `FontId`.
- **`Limits` whole:** `content`, `cmaps`, and for what's to read programs, `font_fuel`, `glyph_fuel`, `composite_glyphs`, `composite_depth`.
- **Encodings.** `Encoding` (`#[non_exhaustive]`): StandardEncoding, WinAnsiEncoding, MacRomanEncoding, and Symbol's and ZapfDingbats' own, each a table of its own, not read from the metrics, with `named`, `glyph` and `text`. `cp1252`, Windows-1252's character for a code.
- **Glyph names.** `glyph_text`, by the Adobe Glyph List's rules, and `name_code_point`, as FreeType's glyph name list reads a name, which PDFium copies. The two lists are in `assets`, unchanged.
- **The standard 14.** `Std14`, with `ALL`, `name`, `named`, by PDFium's names as of chromium/8125, `metrics`, `glyph_width`, `glyph_box`, `drawn_otherwise`, `afm_text` and `NOTICE`. The metrics are Adobe's, in `assets/afm`, with their kerning data and blank lines removed, each file saying so and carrying Adobe's notice. `FontMetrics` and `Rect`, what they're given in.
- **Type 1 programs.** `type1::scan` and `Charstrings`: a program's charstrings by glyph name, read as read-fonts reads it, under `Limits::proc_depth`, `ints_ahead` and `type1_work`.
- **Predefined CMaps.** `predefined-cmaps`, a feature on hayro-cmap, which needs Rust 1.87.
- `Dict`, `Object`, `ObjectId`, `Resolve`, `Span`, `Unreadable` and `Why` are pdf-objects', re-exported.
