# pdf-fonts

PDF's fonts (ISO 32000-2, 9), read. It's made for what reads a page's text and for what checks a file: a font dictionary's codes, CIDs, widths, heights and text, each as PDFium reads them; a simple font's encodings and the text its glyphs' names stand for, the standard 14's names and metrics, a Type 1 program's charstrings, and what a font's program says of its glyphs' boxes, its heights and its text. Nothing here writes a font or paints a glyph.

## What it does

- **A file's font, as read.** `Font::read` takes a font dictionary, a resolver of its objects and what gives its streams' data (`Streams`): a pdf-structure `&Reading` with the `structure` feature, or `With`, a resolver and a function, for anything else. It gives the codes a string is in the font (`codes`), each code's CID (`cid`), width (`width`), vertical metrics (`vertical`) and text (`code_text`, `text`), how far the font's glyphs reach (`metrics`), a glyph's box where Adobe's metrics or a Type 3 font's CharProc say it (`glyph_box`) and where its program does (`char_box`), each read as PDFium reads it: Type 1, MMType1 and TrueType fonts' `/Widths`, `/FirstChar`, `/MissingWidth`, encodings and `/Differences`, a standard 14 font by its name with no `/Widths`, Type 3 fonts through their `/FontMatrix`, Type 0 fonts' `/W`, `/DW`, `/W2`, `/DW2`, an embedded CMap over those its `/UseCMap` names (the standard's reading: PDFium reads none of them and gives a code past the CMap's own ranges CID 0, so `approx` says what's taken from them), and a `/ToUnicode` CMap.
- **As PDFium reads it, on what's hostile too.** Each value of a font's dictionary is read by what PDFium's accessor for it does, as of chromium/8125, each held to PDFium's own answers (`tests/pdfium`, by the repository's oracle). A whole number is `true` as 1, what's no number 0, a real cut from its 32-bit float, and past 32 bits 0, and it's held as PDFium holds it: a simple font's widths in 16 bits, a CID font's in 32 and its `/DW` and vertical metrics in 16. A reference is followed once, for a number, an array or a stream, and one to a reference is then none; a dictionary has a reference more, and a stream's will do. `/Differences` has that one reading, for text and for glyphs. A font PDFium doesn't load, a Type 0 font without one CIDFont or an `/Encoding` it can read, is told (`Fault::DescendantFonts`, `Fault::CMap`) and read as far as it reads, as PDFium draws nothing in it.
- **A font's program,** with the `programs` feature, on by default, through skrifa. `/FontFile`, `/FontFile2` or `/FontFile3`, told apart by its bytes as FreeType tells it: TrueType, OpenType over CFF, bare CFF, Type 1. `Font::program` says whether it was read, and `char_box` is each glyph's outline box, as PDFium's FreeType gives it: a code is mapped to its glyph as `LoadGlyphMap` maps it, through what's read of the encoding, the program's charmaps and its glyphs' names. A font's heights where its descriptor lacks them are the program's face's, and the text of a code PDFium's encoding leaves to the program is the program's: a symbolic Type 1 font's by its glyph's name, a symbolic TrueType font's by its charmap. A program FreeType wouldn't load reads as PDFium reads it, as no program at all, and is told (`Fault::ProgramMalformed`). Its work is bounded: a TrueType glyph's composites and points are counted before it's drawn, a CFF or Type 1 glyph's charstrings evaluated within fuel, each of these in `Limits`, and what it decompresses to and what drawing its glyphs costs charged to the budget (a CIDFont's as they're shown, `charge_outlines`). Past a bound, a glyph's box is none. Without the feature, and in `Fonts::for_text`, no program is read: boxes are none, heights guessed, and that text guessed by the font's name.
- **Where its text parts from PDFium's, on purpose.** Text isn't layout, and these are the better text: a name of joined parts is the Adobe Glyph List's (`f_i` is `fi`); a Dingbats name is text only in a ZapfDingbats font; WinAnsiEncoding's 0xA0 and 0xAD are Windows-1252's; a Type 3 font's names are read for text as a simple font's are; a CID's text is Adobe's UCS2 CMap's, where PDFium's own table is older; a `/ToUnicode` range is read to its last code, where PDFium stops at its low byte or loses the block; a glyph name there is the list's text and a value of one byte that byte's character, where PDFium has none; a surrogate pair is the one character it means and half of one U+FFFD, where PDFium's are 16-bit units; 0xC1 by a program's Mac charmap is the inverted exclamation mark, where PDFium's table has the pound sign. A predefined CMap's CIDs are Adobe's current ones, so where PDFium's own table has no such code, the width differs too. A CMap is lexed as content, not by PDFium's word parser, so a crafted one can read otherwise. MacExpertEncoding and PDFDocEncoding in a font's dictionary aren't read, so a program's glyph by one has no box. `tests/pdfium/differ.tsv` lists every case of these, and why.
- **What's guessed is said, and what fails is the caller's.** What can't be known is guessed, and `approx` says which of codes, widths, heights and boxes (`FontApprox`). What's wrong with the dictionary never fails a reading, and is told to a `Sink` with where it is (`Fault`, `Place`): a `Vec` keeps them, and `()` looks for none, what's read being the same whatever the sink. A reading fails only for what its streams decompress to, past the budget it's read within, which is the caller's (`Error::TooLarge`), or for what doesn't read for a reason that's not the font's, a locked file, a source that failed, a cap met (`Error::Unreadable`). The budget is left as it was. A `Streams` is held to what it's asked for: data past the limit is too large, whatever gave it.
- **Each font read once.** `Fonts` keeps a source's fonts by number, for every page that draws in them, charging what each cost to each page's budget, so that what a page costs is the same whichever page read it first; `FontScope`, one interpretation's picks by name. `Fonts::stock` is what PDFium draws text in where the resources have no font by the name given.
- **Encodings.** `Encoding` is a simple font's encoding, code to glyph name: StandardEncoding, WinAnsiEncoding and MacRomanEncoding (ISO 32000-2, D.2), and Symbol's and ZapfDingbats' own (D.5, D.6). `Encoding::named` takes a font's `/Encoding` or `/BaseEncoding`. MacExpertEncoding isn't read, and is none, as an unknown name is. `Encoding::text` is the text a code draws; WinAnsi's is Windows-1252's where that has one, `cp1252`, so its second space and hyphen are U+00A0 and U+00AD, and the codes above the space it leaves empty draw the bullet. MacRoman's 0xF0 is the Apple logo, as PDFium's table has it.
- **Glyph names.** `glyph_text` reads a name by the Adobe Glyph List's rules: what follows a period is passed over, the parts an underscore joins are each read, and a part is read from the ITC Zapf Dingbats list, in a font of those, the Adobe Glyph List, `uniXXXX…` or `uXXXX` to `uXXXXXX`. `name_code_point` reads a name as FreeType's glyph name list does, which PDFium copies: `uni` and four digits, `u` and four to six, or the list's code point where it gives exactly one.
- **The standard 14.** `Std14::named` takes a font's `/BaseFont` as PDFium draws it, as of chromium/8125: after a subset's tag, the 14 names or an alternate of 75 more (`Arial,Bold`, `TimesNewRomanPSMT`), in any case. `metrics`, `glyph_width` and `glyph_box` are Adobe's. `drawn_otherwise` says where PDFium's own faces draw a glyph at another width.
- **Type 1 programs.** `type1::scan` reads a program as read-fonts reads it, as far as its charstrings by glyph name, each decrypted. Its procedures' depth, its integers in a row and its work are capped (`Limits`), past which it's refused, so what's scanned is safe to hand to a reader of programs that isn't so bounded, as `programs` hands read-fonts, and a program's names can be read with no such reader in the build at all.
- **Predefined CMaps,** behind the `predefined-cmaps` feature, whose bundle, [hayro-cmap](https://crates.io/crates/hayro-cmap), needs Rust 1.87. The crate itself needs 1.85.
- **Under caps.** `Limits` is one value for all of them, and nothing panics on what it's given, a program's parsers being the one thing that rests on others' promises: skrifa and read-fonts forbid `unsafe`, and are fuzzed upstream. What a font's streams decompress to is the budget's; what it holds beyond that is bounded by what it reads, a `/W` range of 2³¹ CIDs being one range. A CMap and a CharProc are lexed by pdf-content-streams, under its caps. An AFM is only ever this crate's own file, so a parser of them has no caps and takes no file's bytes.

```rust
use pdf_fonts::{Decoded, Font, Limits, Object, ObjectId, With};

// A font with no streams, over a resolver of nothing.
let none = |_: ObjectId| -> Option<&Object> { None };
let objects = With::new(&none, |_, _| Decoded::None);
let font = pdf_objects::dict! {
    "Subtype" => "TrueType", "BaseFont" => "Foo", "FirstChar" => 65, "Widths" => vec![Object::from(600)],
};
let mut budget = 1 << 20;
let mut faults = Vec::new();
let read = Font::read(objects, &font, &Limits::DEFAULT, &mut budget, &mut faults).unwrap();
assert_eq!(read.width(65), 600.0);
assert_eq!(read.codes(b"AB").count(), 2);
assert!(read.approx().heights);
assert!(faults.is_empty());
```

```rust
use pdf_fonts::{Encoding, Std14, glyph_text};

let font = Std14::named(b"ABCDEF+Arial,Bold").unwrap();
assert_eq!(font, Std14::HelveticaBold);
let glyph = Encoding::named(b"StandardEncoding").unwrap().glyph(b'A').unwrap();
assert_eq!(font.glyph_width(glyph), Some(722.0));
assert_eq!(glyph_text("f_i.alt", false).as_deref(), Some("fi"));
```

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.

The data isn't this crate's, and each part of it has its own terms, in the file that holds it:

- **`assets/afm`:** Adobe's Core 14 font metrics. They may be used and distributed for any purpose, with or without modification, on the terms of Adobe's paragraph, which each file carries unmodified, in a `Comment` on its third line, in place of the separate file (the set's MustRead.html) it was written in. Each file's second line says what's modified, and all its copyright notices are as Adobe wrote them. They're compiled in whole, so a binary holds them with their notices. `Std14::NOTICE` is the paragraph, and `Std14::afm_text` a font's whole file, for a binary's list of notices.
- **`assets/glyphlist.txt` and `assets/zapfdingbats.txt`:** the Adobe Glyph List and the ITC Zapf Dingbats Glyph List, unchanged, under the BSD 3-Clause license, each with the whole of it in its header. A binary's documentation must reproduce it, as for any dependency under that license.
- **With `predefined-cmaps`,** Adobe's CMaps are hayro-cmap's, under the BSD 3-Clause license (its package's `assets/LICENSE.txt`), not in this package. A binary that turns the feature on carries that notice too.
