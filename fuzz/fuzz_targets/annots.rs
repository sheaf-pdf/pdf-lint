//! Annotation, destination, action, reply-thread, appearance synthesis, and
//! flattening checks on arbitrary bytes (`pdf_annots::fuzz::check`), checking
//! fault-sink invariance and `Debug` hygiene; and read as a PDF file
//! (`pdf_annots::fuzz::over`) over its first pages.

#![no_main]

use pdf_annots::fuzz::{check, over};
use pdf_structure::fuzz::options;
use pdf_structure::read::Reading;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    check(data);
    if let Ok(reading) = Reading::open(data, &options()) {
        over(&reading, reading.trailer().dict());
    }
});
