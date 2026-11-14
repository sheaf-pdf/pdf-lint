# pdf-fonts

Read PDF's fonts (ISO 32000-2, 9) as PDFium reads them: a font's codes, CIDs, widths, heights and text, a simple font's encodings and the text of its glyphs' names, the standard 14's metrics, and its program's glyph boxes.

It's made for what reads a page's text and for what checks a file. It reads no file itself. A font is read through what gives its objects and its streams' data, pdf-objects' `Resolve` and `Streams`: a pdf-structure `&Reading` is both, and pdf-objects' `WithStreams` makes both of a resolver and a function. It's built on [pdf-objects](https://crates.io/crates/pdf-objects) and [pdf-content-streams](https://crates.io/crates/pdf-content-streams) and re-exports both whole, so `pdf_fonts::pdf_objects` and `pdf_fonts::pdf_content_streams` name their types at the versions this crate is built with. Nothing here embeds a font or draws a glyph: what it writes is a standard 14 font's dictionary, and text in its codes.

## What it does

- **Read a file's font.** `Font::read` reads a font dictionary as PDFium (chromium/8125) does: Type 1, MMType1, TrueType, Type 3 and Type 0 fonts, their widths, encodings and `/Differences`, their CMaps and their `/ToUnicode`. Each value is read as PDFium's accessor for it reads it, a hostile one too.
- **Split a string into codes, and read each.** `Font::codes` gives the codes a string is in the font, as a `Codes` iterator. `cid`, `width`, `vertical` and `code_text` give a code's CID, width, vertical metrics and text, and `text` a string's. `metrics` says how far the font's glyphs reach, and `glyph_box` and `char_box` give a glyph's box.
- **Know what's guessed.** What can't be known is guessed, and `Font::approx` says which of its codes, widths, heights and boxes are guessed (`FontApprox`). `Font::loaded` says whether PDFium loads the font at all.
- **Read each font once.** `Fonts` keeps a source's fonts by number for every page that draws in them. What each cost is charged to each page's budget, so a page costs the same whichever page read a font first. `FontScope` keeps the fonts one interpretation of content picks, by name. `Fonts::stock` is the font PDFium draws text in where the resources name none.
- **Read the strings a page's content shows.** `shown` walks a page's content, its streams joined, as PDFium reads it for its text: `Tf`, `gs` where its ExtGState's `/Font` picks a font, `q` and `Q`, the four operators that show strings, and `Do` of a form, followed in its own state, its resources its own or else what draws it. Each string comes in order, with its bytes, its font, the form it's in and where it's written (`Shown`, `ShownString`), and `Shown::text` joins their text. Forms are followed to `Limits::forms` deep, 40 by default, as PDFium draws none deeper, and drawn `Limits::form_uses` times a call at most. There's no layout: strings come as the content shows them, not where they are on the page. A Type 3 font's glyphs and an annotation's appearance aren't walked. What isn't read is said, never guessed (`Unread`): a form too deep or drawn too often, content past a cap, a form or a font past the budget or behind a password, a graphics state behind one, and a stream whose data doesn't decode, damaged or through a filter the reader doesn't take, which PDFium reads as written.
- **Measure a glyph by its font's program.** With `programs`, a font's embedded program is read through skrifa. `Font::char_box` is a glyph's outline box as PDFium's FreeType gives it. The font's heights come from the program where its descriptor lacks them, and so does the text of a code PDFium's encoding leaves to it. `Font::program` says what was read (`FontProgram`). `Font::read_for_text` and `Fonts::for_text` read no program, whatever the build's features.
- **Look up encodings and glyph names.** `Encoding` is a simple font's encoding, code to glyph name: StandardEncoding, WinAnsiEncoding and MacRomanEncoding (D.2), and Symbol's and ZapfDingbats' own (D.5, D.6). `glyph_text` gives the text a glyph's name stands for, by the Adobe Glyph List's rules, and `name_code_point` reads a name as FreeType's glyph name list does. `cp1252` gives Windows-1252's character for a code.
- **Find a standard 14 font.** `Std14::named` takes a `/BaseFont` as PDFium does: after a subset's tag, one of the 14 names or of 75 alternates (`Arial,Bold`), in any case. `metrics`, `glyph_width` and `glyph_box` are Adobe's. `drawn_otherwise` says where PDFium's own faces draw a glyph at another width.
- **Write text in a standard 14 font.** `Std14::dict` is its font dictionary: the twelve Latin ones in WinAnsiEncoding, and Symbol and ZapfDingbats in their own, with no widths, which a file of PDF 1.0 to 1.7 may leave out. `Std14::codes` puts text in its codes, a byte a character, which read back as the text given through the font, as PDFium's font reads them, and `Encoding::code` gives one character's code. PDFium's text page writes two of WinAnsiEncoding's codes otherwise, a no-break space as a space and a soft hyphen as a hyphen. A character the encoding has no code for is refused where it is (`Unencodable`), never written as `?` or as another.
- **See where a font departs from the standard.** Each departure is a `Fault`, told with its place to pdf-objects' `fault::Sink`, and the reading goes on past it. A cap of the reading's own (`Fault::TooManyCMaps`, `Fault::ProgramCapped`) and what the build hasn't (`Fault::PredefinedNotBuilt`) are told apart, as they say nothing of the file. With `()` as the sink, no fault is looked for. What's read is the same through any sink. One sink can take pdf-semantics' and pdf-color's faults too.
- **Read hostile files.** `Limits` caps a CMap's chain, a Type 1 program's scan, a program's glyphs and how deep and how often forms are followed, and holds pdf-content-streams' caps, which a CMap, a CharProc and the content `shown` walks are lexed under. Past a content cap, a CMap or a CharProc is read as far as it lexes. Past a chain's cap, what it would give is guessed and told. Past the scan's, the program reads as none (`Fault::ProgramCapped`). Past a glyph's, its box is none. What a font's streams decompress to is charged to the caller's budget, and so is what drawing its glyphs costs, a CIDFont's as they're shown (`Font::charge_outlines`), and a form's content each time `shown` follows it. Past the budget, a reading fails with `Error::TooLarge`.
- **Know when a read failed.** An object or a stream may not read for a reason that's the caller's: past a cap of what gives it (`Why::Capped`), behind a password or from a source that failed. That fails the reading with `Error::Unreadable`, never a font with no widths, and nothing is told to the sink after it. Either way, `Font::read` leaves the budget as it was.
- **Share a font across threads.** A `Font` owns what it holds. `Font`, `Fonts`, `Codes` and `Shown` are `Send` and `Sync`. A `Font`'s `Debug` says what it is and how much it holds, never a file's text, and neither does `Shown`'s.

Nothing in this crate panics on what a file holds. A program is parsed by skrifa and read-fonts, which forbid `unsafe` and are fuzzed upstream.

## Features

- **`programs`**, on by default. A font's program is read through [skrifa](https://crates.io/crates/skrifa), as above. Without it, a glyph has no box from a program, the font's heights are guessed, and the text a program would give is guessed by the font's name.
- **`predefined-cmaps`**, off by default. Adobe's predefined CMaps come from [hayro-cmap](https://crates.io/crates/hayro-cmap)'s bundle. Without it, a predefined CMap the bundle has reads as one the build hasn't: its codes are guessed, `FontApprox::codes` says so, and `Fault::PredefinedNotBuilt` is told. With `()` as the sink, `approx` alone says it. CNS-EUC-H and -V, which PDFium doesn't know, read the same in every build (`Fault::PredefinedUnknown`).
- **`tracing`**, off by default. A font read (`Font::read`, `Font::read_for_text`, and `Fonts` and `FontScope` where they read one) runs in a span at `debug`, through [tracing](https://crates.io/crates/tracing). Within it, so do its program's opening, its embedded CMap's and its `/ToUnicode`'s parsing once each is decompressed, and a Type 3 font's CharProcs. A walk of the strings content shows (`shown`) runs in a span too, the fonts it reads in theirs within it. A span's fields are numbers, never a byte of a file. A code, a width, a string's text and a glyph's box have none, as their loop is the caller's. Nor has a font looked up where it was read before.

## An example

Helvetica-Bold by a name PDFium takes for it, its `/ToUnicode` read through its streams, and the same font past a cap of what gives them:

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

## How it's held to PDFium

The reading follows PDFium's behaviour as of chromium/8125, written in this crate's own code but for what the licence below says is FreeType's. Cases in the repository's `tests/pdfium` hold it there, each with PDFium's answer recorded from that build. Where this crate reads a case otherwise, `tests/pdfium/differ.tsv` lists it with why. Those files aren't in the package.

- **Its text is the better text, on purpose,** as text isn't layout. A glyph name of joined parts is the Adobe Glyph List's text (`f_i` is `fi`), and a Dingbats name is text only in a ZapfDingbats font. WinAnsiEncoding's 0xA0 and 0xAD are Windows-1252's. A Type 3 font's names are read for text. A CID's text is Adobe's UCS2 CMap's. A `/ToUnicode` range is read to its last code, and a surrogate pair is the one character it means.
- **A CMap is read as ISO 32000-2 has it.** An embedded CMap's `/UseCMap` and `usecmap` are read, where PDFium reads neither, and `FontApprox::codes` says what's taken from them. A predefined CMap's CIDs are Adobe's current ones. A CMap is lexed as content, not by PDFium's word parser, so a crafted one can read otherwise.
- **PDFDocEncoding and MacExpertEncoding have no table here.** Where a font names PDFDocEncoding, or MacExpertEncoding as the `/BaseEncoding` of a font not drawn in a TrueType program, the font's own encoding stands. A program's glyph by it has no box. Named otherwise, MacExpertEncoding reads as WinAnsiEncoding, as PDFium reads it.
- **A program is read by skrifa and read-fonts, not FreeType.** Where they're known to read one otherwise, a glyph's box is none, and `FontApprox::boxes` says so. They're less strict than FreeType, so a crafted program it refuses can read here. Its boxes are then ones PDFium hasn't, and nothing says so. A composite of more than 2,048 components and a charstring of more than a million operations are past their own bounds, so such a glyph's box is none where PDFium draws it.

## Exhaustive on purpose, or not

`Std14`, `Rect` and `VerticalMetrics` are exhaustive: the standard's 14 are fixed, and a rectangle and a glyph's vertical metrics are their parts. `Std14::ALL` lists the 14. Every other enum and every struct with public fields is `#[non_exhaustive]`, so each can grow in a minor release. `Fault`'s variants aren't, so a fault can be built and compared.

## What isn't here

- **Writing a font, or drawing a glyph.** A glyph's box is given, never its outline.
- **Placing text on a page.** A string's text is its codes' text in order. Where each glyph goes, under the text state, is the caller's.
- **The faces PDFium draws in.** A font without a program is drawn by PDFium in a face of its own or the system's, which isn't read here: a standard 14 font's widths are Adobe's, and its heights are guessed.
- **Kerning.** The standard 14's metrics are kept without their kerning pairs.
- **A font dictionary typed key by key, by ISO 32000-2's tables.** `Font` is a reading, as PDFium's is, and a `Fault` says where a font departs from the standard.

## Stability

This is an alpha: any alpha may change the API, and the CHANGELOG says how to follow. Depend on the exact version, `pdf-fonts = "=0.1.0-alpha.1"`. It needs Rust 1.89 or later.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.

The fonts' data isn't this crate's, and each part of it has its own terms, in the file that holds it:

- **`assets/afm`:** Adobe's Core 14 font metrics. They may be used and distributed for any purpose, with or without modification, on the terms of Adobe's paragraph. [`assets/afm/MustRead.html`](https://github.com/sheaf-pdf/pdf-lint/blob/main/crates/pdf-fonts/assets/afm/MustRead.html) holds it unmodified, as Adobe distributes it. Each file carries it too, in a `Comment` on its third line. Each file's second line says what's modified, and its copyright notices are as Adobe wrote them. They're compiled in whole, so a binary holds them with their notices. `Std14::NOTICE` is the paragraph, and `Std14::afm_text` a font's whole file, for a binary's list of notices.
- **`assets/glyphlist.txt` and `assets/zapfdingbats.txt`:** the Adobe Glyph List and the ITC Zapf Dingbats Glyph List, unchanged. Each is under the BSD 3-Clause license, given whole in its header. A binary's documentation must reproduce it, as for any dependency under that license.
- **With `predefined-cmaps`,** Adobe's CMaps are hayro-cmap's, under the BSD 3-Clause license (its package's `assets/LICENSE.txt`), not in this package. hayro-cmap decompresses them with brotli, under the BSD 3-Clause and MIT licenses. brotli pulls in brotli-decompressor (BSD 3-Clause or MIT), alloc-no-stdlib and alloc-stdlib (BSD 3-Clause). A binary that turns the feature on carries each of their notices too.
- **With `programs`,** on by default, skrifa and what it pulls in carry their own terms, in their packages: MIT OR Apache-2.0 for the most part, Unicode-3.0 for yoke and zerofrom, whose notice a binary keeps, and Zlib for foldhash. read-fonts compiles in the Adobe Glyph List too, on the terms above. A binary's list of notices takes them from there.

Some of `src/program.rs` comes from FreeType's code, copyright David Turner, Robert Wilhelm, and Werner Lemberg. It's under the FreeType Project License, not the licenses above, and [`FTL.TXT`](https://github.com/sheaf-pdf/pdf-lint/blob/main/crates/pdf-fonts/FTL.TXT) holds it unaltered. The package's `license` says so. It's written in Rust and changed to fit this crate:

- its reading of format 4 and format 2 `cmap` subtables translates `tt_cmap4_validate`, `tt_cmap4_char_index`, `tt_cmap4_char_map_linear`, `tt_cmap4_char_map_binary`, `tt_cmap2_validate`, `tt_cmap2_get_subheader` and `tt_cmap2_char_index` (`ttcmap.c`)
- its reading of a bare CFF font's encoding translates `cff_encoding_load` (`cffload.c`)
- its lists of tricky fonts and of the glyph names it adds to Unicode's are FreeType's (`trick_names`, `ttobjs.c`; `ft_extra_glyph_names` and `ft_extra_glyph_unicodes`, `psmodule.c`)

With `programs`, a binary's documentation must say that it's based in part on the work of the FreeType Team.
