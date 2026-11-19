//! The field tree read from arbitrary bytes (`pdf_forms::fuzz::check`),
//! checking fault-sink invariance, the tree's shape, its caps, and `Debug`
//! hygiene; and read from a PDF file's catalog (`pdf_forms::fuzz::over`).

#![no_main]

use pdf_forms::fuzz::{check, over};
use pdf_structure::fuzz::options;
use pdf_structure::read::Reading;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    check(data);
    if let Ok(reading) = Reading::open(data, &options()) {
        over(&reading, reading.trailer().dict());
    }
});
