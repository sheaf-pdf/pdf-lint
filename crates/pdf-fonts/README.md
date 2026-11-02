# pdf-fonts

PDF's fonts (ISO 32000-2, 9), read. It's made for what reads a page's text and for what checks a file: a simple font's encodings and the text its glyphs' names stand for, the standard 14's names and metrics, and a Type 1 program's charstrings. Nothing here writes a font or draws a glyph.

## What it does

- **Encodings.** `Encoding` is a simple font's encoding, code to glyph name: StandardEncoding, WinAnsiEncoding and MacRomanEncoding (ISO 32000-2, D.2), and Symbol's and ZapfDingbats' own (D.5, D.6). `Encoding::named` takes a font's `/Encoding` or `/BaseEncoding`. MacExpertEncoding isn't read, and is none, as an unknown name is. `Encoding::text` is the text a code draws; WinAnsi's is Windows-1252's exactly, `cp1252`, so its second space and hyphen are U+00A0 and U+00AD.
- **Glyph names.** `glyph_text` reads a name by the Adobe Glyph List's rules: what follows a period is passed over, the parts an underscore joins are each read, and a part is read from the ITC Zapf Dingbats list, in a font of those, the Adobe Glyph List, `uniXXXX…` or `uXXXX` to `uXXXXXX`. `name_code_point` reads a name as FreeType's glyph name list does, which PDFium copies: `uni` and four digits, `u` and four to six, or the list's code point where it gives exactly one.
- **The standard 14.** `Std14::named` takes a font's `/BaseFont` as PDFium does, as of chromium/8125: after a subset's tag, the 14 names or an alternate of 75 more (`Arial,Bold`, `TimesNewRomanPSMT`), in any case. `metrics`, `glyph_width` and `glyph_box` are Adobe's. `drawn_otherwise` says where PDFium's own faces draw a glyph at another width.
- **Type 1 programs.** `type1::scan` reads a program as read-fonts reads it, as far as its charstrings by glyph name, each decrypted. Its procedures' depth, its integers in a row and its work are capped (`Limits`), past which it's refused, so what's scanned is safe to hand to a reader of programs that isn't so bounded, and a program's names can be read with no such reader in the build at all.
- **Predefined CMaps,** behind the `predefined-cmaps` feature, whose bundle, [hayro-cmap](https://crates.io/crates/hayro-cmap), needs Rust 1.87. The crate itself needs 1.85.
- **Under caps.** `Limits` is one value for all of them, and nothing panics on what it's given. An AFM is only ever this crate's own file, so a parser of them has no caps and takes no file's bytes.

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
