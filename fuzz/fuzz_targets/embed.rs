//! A TrueType program embedded, on any bytes: read whole, a face of four, or as one
//! table of a fixture program; opened, or refused saying why; text drawn in it, a
//! fixed text, the input's last bytes and what its `cmap` maps first, each character
//! its glyph or the first with none refused where it is, nothing of it kept; the font
//! written under each choice of options, each twice the same, refused only for its
//! licence where the right isn't given, a `/CIDSet` one more object and nothing else;
//! each font written read back by pdf-fonts' reader without a fault, each glyph's
//! width the program's and its text the one drawn, its program read and each glyph's
//! box the same cut as whole, and as the program given draws it; and a glyph the input
//! names drawn, refused where it's past the program's glyphs.

#![no_main]

use pdf_fonts::fuzz::embed;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = embed(data);
});
