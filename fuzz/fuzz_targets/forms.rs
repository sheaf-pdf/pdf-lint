//! The field tree read from arbitrary bytes (`pdf_forms::fuzz::check`),
//! checking fault-sink invariance, the tree's shape, its caps, and `Debug`
//! hygiene, and making the appearance of each of its text fields' widgets;
//! the same bytes cut into a text field's text, flags, box, border, colors,
//! rotation, and `/DA`, its appearance made in each standard 14 font
//! (`pdf_forms::fuzz::check_appearance`, which `check` calls); and the form
//! read from a PDF file's catalog (`pdf_forms::fuzz::over`).

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
