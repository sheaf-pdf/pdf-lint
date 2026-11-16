//! Content interpretation and re-encoding on arbitrary bytes: split into page
//! content, resources, and indirect objects/streams (`pdf_gfx::fuzz::check`),
//! checking fault-sink invariance, `Wants::NONE` invariance, and `encode`
//! round-trip invariance; and read as a PDF file (`pdf_gfx::fuzz::over`) over
//! its first pages. Built with `fuzz/`'s `predefined-cmaps`, predefined CJK
//! CMaps resolve from `pdf-fonts`' bundle.

#![no_main]

use pdf_gfx::fuzz::{check, over};
use pdf_structure::fuzz::options;
use pdf_structure::read::Reading;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    check(data);
    if let Ok(reading) = Reading::open(data, &options()) {
        over(&reading, reading.trailer().dict());
    }
});
