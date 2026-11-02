//! Fonts' data, on any bytes: scanned as a Type 1 program, holding no more than it
//! was given, and under other caps the same or refused; and its words read as
//! glyphs' and fonts' names, and its first bytes as codes in each encoding.

#![no_main]

use pdf_fonts::fuzz::{names, type1};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    type1(data);
    names(data);
});
