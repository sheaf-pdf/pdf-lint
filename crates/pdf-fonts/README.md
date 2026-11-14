# pdf-fonts

PDF font reader, page text extractor, Standard 14 font metrics, CMap interpreter, and glyph bounding-box reader (ISO 32000-2, §9), matching PDFium's behavior as of chromium/8125.

`pdf-fonts` reads fonts through any [pdf-objects](https://crates.io/crates/pdf-objects) `Resolve` and `Streams` provider (such as a `pdf-structure` `&Reading` or `WithStreams`) without performing file I/O or rasterizing glyphs. Both [pdf-objects](https://crates.io/crates/pdf-objects) and [pdf-content-streams](https://crates.io/crates/pdf-content-streams) are re-exported at the crate root.

## Core Concepts

- **Font parsing & character decoding (`Font`, `Fonts`, `FontScope`):**
  - `Font::read` parses Type 1, MMType1, TrueType, Type 3, and Type 0 (CIDFont) dictionaries—including widths, encodings, `/Differences`, CMaps, `/ToUnicode` maps, and embedded font programs—using PDFium's rules.
  - `Font::codes` splits byte strings into character codes (`Codes`), exposing each code's CID (`cid`), advance width (`width`), vertical metrics (`vertical`), glyph bounding box (`glyph_box`, `char_box`), and Unicode text (`code_text`, `Font::text`). `Font::approx` (`FontApprox`) records whenever widths, heights, codes, or boxes had to be estimated.
  - `Fonts` caches parsed fonts across pages while charging stream/outline costs fairly against each page's budget; `FontScope` resolves named font resources within a content stream.
- **Page text extraction (`shown`, `Shown`):** Walks joined page content streams (`Tf`, `gs` `/Font`, `q`/`Q`, text-showing operators, and nested Form XObjects up to `Limits::forms` deep), yielding `ShownString` runs in content order (`Shown::text`) and reporting skipped or undecodable regions via `Unread`.
- **Standard 14 fonts & encodings (`Std14`, `Encoding`):**
  - `Std14::named` matches the 14 standard PDF fonts and PDFium's 75 alias names (such as `Arial,Bold`), providing Adobe AFM widths/boxes and building Standard 14 font dictionaries (`Std14::dict`) and encoded byte strings (`Std14::codes`, rejecting unencodable characters via `Unencodable`).
  - `Encoding`, `glyph_text`, `name_code_point`, and `cp1252` map character codes and glyph names to Unicode per the Adobe Glyph List and Windows-1252.
- **Diagnostics & resource budgets (`Fault`, `Limits`):** Spec departures emit `Fault`s to a `pdf-objects` `fault::Sink<Fault>` (skipped at zero cost when `()` is passed). `Limits` and caller byte budgets bound CMap chains, Type 1 scans, glyph counts, form recursion, and decompressed stream sizes (`Error::TooLarge`, `Error::Unreadable`) without panicking on untrusted files.

## Quickstart

Read `Arial,Bold` (mapped to `Helvetica-Bold`) with a `/ToUnicode` CMap stream and verify budget accounting on capped streams:

```rust
use std::collections::BTreeMap;

use pdf_fonts::pdf_objects::{
    Object, ObjectId, Stream, Unreadable, Why, WithStreams, dict,
};
use pdf_fonts::{Error, Font, Limits};

// Helvetica-Bold, by a name PDFium takes for it, its /ToUnicode object 1.
let font = dict! {
    "Subtype" => "Type1", "BaseFont" => "Arial,Bold", "Encoding" => "WinAnsiEncoding",
    "ToUnicode" => Object::reference(1),
};
let cmap = b"1 begincodespacerange <00> <FF> endcodespacerange\n\
    1 beginbfchar <01> <FB01> endbfchar";
let stream = Stream::new(dict! {}, cmap.to_vec());
let objects = BTreeMap::from([(ObjectId::new(1, 0), Object::from(stream))]);

// Its objects, and its streams' data as they hold it.
let resolve = |id: ObjectId| objects.get(&id);
let streams = WithStreams::new(&resolve, |id, _| {
    let data = objects.get(&id).and_then(Object::as_stream);
    let data = data.and_then(|stream| stream.data.bytes()).map(<[u8]>::to_vec);
    data.ok_or_else(|| Unreadable::new(id, Why::Damaged, "no such stream"))
});
let limits = Limits::DEFAULT;
let mut budget = 1 << 20;
let mut faults = Vec::new();
let read = Font::read(streams, &font, &limits, &mut budget, &mut faults).unwrap();
assert!(faults.is_empty());
assert_eq!(budget, (1 << 20) - cmap.len());

// Adobe's widths; the /ToUnicode's text, then WinAnsiEncoding's; heights guessed.
assert_eq!(read.width(u32::from(b'A')), 722.0);
assert_eq!(read.codes(b"\x01A\x80").count(), 3);
assert_eq!(read.text(b"\x01A\x80").as_deref(), Some("\u{FB01}A€"));
assert!(read.approx().heights);

// A stream past a cap of what gives it is the caller's to know, the budget kept.
let capped = WithStreams::new(&resolve, |id, _| {
    Err(Unreadable::new(id, Why::Capped, "past the reader's cap"))
});
let mut budget = 1 << 20;
let error = Font::read(capped, &font, &limits, &mut budget, &mut ()).unwrap_err();
assert!(matches!(error, Error::Unreadable(u) if u.why == Why::Capped));
assert_eq!(budget, 1 << 20);
```

## Cargo Features

- **`programs`** *(default)*: Parses embedded TrueType, CFF, and Type 1 font programs via [skrifa](https://crates.io/crates/skrifa) for glyph outline boxes (`Font::char_box`), fallback heights, and program-derived character mappings (`Font::read_for_text` and `Fonts::for_text` skip program parsing even when enabled).
- **`predefined-cmaps`** *(optional)*: Bundles Adobe's predefined CJK CMaps via [hayro-cmap](https://crates.io/crates/hayro-cmap). Without it, predefined CMaps fall back to approximate code widths and report `Fault::PredefinedNotBuilt`.
- **`tracing`** *(optional)*: Emits numeric [tracing](https://crates.io/crates/tracing) spans around font reads, program/CMap/CharProc parsing, and `shown` walks.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.

Bundled font data and derived components carry their own license terms, which downstream binaries must preserve where applicable:

- **`assets/afm`:** Adobe Core 14 font metrics, distributed under Adobe's notice in [`assets/afm/MustRead.html`](https://github.com/sheaf-pdf/pdf-lint/blob/main/crates/pdf-fonts/assets/afm/MustRead.html) (also embedded in each AFM file and exposed via `Std14::NOTICE` and `Std14::afm_text`).
- **`assets/glyphlist.txt` and `assets/zapfdingbats.txt`:** Adobe Glyph List and ITC Zapf Dingbats Glyph List under the BSD 3-Clause license (reproduced in each file's header).
- **With `predefined-cmaps`:** Adobe CMaps from `hayro-cmap` (BSD 3-Clause) and transitive `brotli` / `brotli-decompressor` / `alloc-no-stdlib` / `alloc-stdlib` notices (BSD 3-Clause / MIT).
- **With `programs` (default):** `skrifa` and its dependencies (`MIT OR Apache-2.0`, `Unicode-3.0` for `yoke`/`zerofrom`, `Zlib` for `foldhash`, and BSD 3-Clause Adobe Glyph List in `read-fonts`).

Portions of `src/program.rs` are translated into Rust from FreeType (copyright David Turner, Robert Wilhelm, and Werner Lemberg) under the FreeType Project License in [`FTL.TXT`](https://github.com/sheaf-pdf/pdf-lint/blob/main/crates/pdf-fonts/FTL.TXT) (reflected in the package's `(MIT OR Apache-2.0) AND FTL` license expression):

- Format 4 and format 2 `cmap` subtable parsing (`tt_cmap4_validate`, `tt_cmap4_char_index`, `tt_cmap4_char_map_linear`, `tt_cmap4_char_map_binary`, `tt_cmap2_validate`, `tt_cmap2_get_subheader`, `tt_cmap2_char_index` from `ttcmap.c`).
- Bare CFF encoding loading (`cff_encoding_load` from `cffload.c`).
- Tricky-font and extra PostScript glyph-name tables (`trick_names` from `ttobjs.c`; `ft_extra_glyph_names` and `ft_extra_glyph_unicodes` from `psmodule.c`).

Binaries built with the `programs` feature must include an acknowledgment in their documentation stating that the software is based in part on the work of the FreeType Team.
