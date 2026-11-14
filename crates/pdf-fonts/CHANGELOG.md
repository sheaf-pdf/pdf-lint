# Changelog

## 0.1.0-alpha.1 (unreleased)

First release. It's an alpha: any alpha may change the API, and each entry here says what changed and how to update. Depend on the exact version, `=0.1.0-alpha.1`; a plain `0.1.0-alpha.1` lets cargo take later alphas too. Its unit tests don't build from the package. They take their files from pdf-fixtures, which is never published. They read them through pdf-structure, not yet on crates.io.

### Added

- `Font::read`: a font dictionary read as PDFium (chromium/8125) reads it, over pdf-objects' `Resolve` and `Streams`. Type 1, MMType1, TrueType, Type 3 and Type 0 fonts, with their embedded CMaps and their `/ToUnicode`.
- `Font`'s readings: a string's codes (`Font::codes`, a `Codes` iterator), each code's CID, width, vertical metrics and text, how far the font's glyphs reach, and a glyph's box. `Font::approx` says which of them are guessed.
- `Fonts` and `FontScope`: each font of a source read once, kept for every page that draws in it and charged to each page's budget, and the fonts one interpretation of content picks, by name.
- `shown`: the strings a page's content shows, in order, each in its font, as PDFium reads them for its text, with no layout. It reads `Tf`, `gs` where its ExtGState's `/Font` picks a font, `q` and `Q`, `Tj`, `TJ`, `'` and `"`, and follows `Do` of a form in its own state, to `Limits::forms` deep and `Limits::form_uses` times a call. `Shown` holds each string (`ShownString`) and what wasn't read (`Unread`): a form too deep or drawn too often, content past a cap, a form or a font past the budget or behind a password, a graphics state behind one, and a stream whose data doesn't decode, which PDFium reads as written.
- The `programs` feature, on by default: a font's program read through skrifa, for `Font::char_box`, the font's heights and the text its encoding leaves to it. `Font::read_for_text` and `Fonts::for_text` read none.
- `Encoding`, `glyph_text`, `name_code_point` and `cp1252`: a simple font's encodings (D.2, D.5, D.6), and the text a glyph's name stands for.
- `Std14`: the standard 14 fonts, found by a `/BaseFont` as PDFium takes it, with Adobe's metrics. `Std14::NOTICE` and `Std14::afm_text` are for a binary's list of notices.
- `Std14::dict`, `Std14::codes`, `Encoding::code` and `Unencodable`: a standard 14 font's dictionary, and text in its codes, which read back as the text given. A character with no code is refused, never replaced.
- `Fault` and `Error`: where a font departs from the standard, told with its place to pdf-objects' `fault::Sink`, and what stops a reading. A cap of the reading's own and what the build hasn't are told apart from the file's faults.
- `Limits`: caps on a CMap's chain, a Type 1 program's scan, a program's glyphs and how deep and how often `shown` follows forms, beside pdf-content-streams' caps for what a font and `shown` lex as content.
- The `predefined-cmaps` feature, off by default: Adobe's predefined CMaps, from hayro-cmap's bundle. The `tracing` feature, off by default: a span over a font read, and within it over its program's opening, its CMap's and its `/ToUnicode`'s parsing, and a Type 3 font's CharProcs; and a span over a walk of what content shows.

### Notes

- It's built on pdf-objects `=0.1.0-alpha.2` and pdf-content-streams `=0.1.0-alpha.1`, both re-exported whole and none of their items by name. `Limits::content` is pdf-content-streams' type. It needs Rust 1.89.
- Where it reads a case otherwise than PDFium, the repository's `tests/pdfium/differ.tsv` lists it and why.
- `Fault::UseCMap`, `Fault::ToUnicode` and `Fault::ProgramMalformed` are told too of a stream whose filters the reader doesn't decode, which is the reader's and not the file's. A later alpha may tell that apart.
- A reading past the caller's budget fails with `Error::TooLarge`. A stream past a cap of what gives it fails it with `Error::Unreadable`, the caller's to know, as one behind a password does. Nothing is told to the sink after that. Either way the budget is as it was.
- Nothing in this crate panics on what a file holds. A font's program is parsed by skrifa 0.50 and read-fonts 0.47.
- It's licensed `(MIT OR Apache-2.0) AND FTL`, as `src/program.rs` translates some of FreeType's code, under `FTL.TXT`. The fonts' data keeps its own terms, which the README lists.
- `Std14`, `Rect` and `VerticalMetrics` are exhaustive on purpose. Every other enum and every struct with public fields is `#[non_exhaustive]`, so each can grow in a minor release. `Fault`'s variants aren't, so a fault can be built and compared.
