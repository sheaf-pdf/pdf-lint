//! Fonts, on any bytes: scanned as a Type 1 program, holding no more than it was
//! given, and under other caps the same or refused; its words read as glyphs' and
//! fonts' names, and its first bytes as codes in each encoding; split into a font's
//! parts and read as one, a program among them, every accessor of every code, the
//! budget never rising, the same whatever the sink, the same twice, doing no more under smaller caps, and
//! failing only as it's made to; and read as a file, each font in it the same.

#![no_main]

use pdf_fonts::fuzz::{fonts, names, over, type1};
use pdf_structure::bytes::Reading;
use pdf_structure::fuzz::options;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    type1(data);
    names(data);
    fonts(data);
    if let Ok(reading) = Reading::open(data, &options()) {
        over(&reading, reading.ids());
    }
});
