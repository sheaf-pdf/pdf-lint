//! Fuzz target for `pdf-images`: exercises all image codecs, standard filter chains, `/Mask`,
//! `/SMask` + `/Matte`, inline images, scrubbing, re-encoders, and full-document XObject walks.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdf_structure::{Options, Reading};

fuzz_target!(|data: &[u8]| {
    pdf_images::fuzz::images(data);
    if let Ok(reading) = Reading::open(data, &Options::default()) {
        pdf_images::fuzz::over(&reading, reading.trailer().dict());
    }
});
