//! Pure-Rust 2D rasterization on arbitrary bytes (`pdf_raster::fuzz::check`),
//! splitting inputs into multi-object/multi-stream scenes (Form XObjects, Image
//! XObjects, tiling patterns, shadings, soft masks, and Type 3 fonts) to check
//! fault-sink invariance, `Debug` redaction, and viewport/format determinism;
//! and read as a PDF file (`pdf_raster::fuzz::over`) over its first pages.

#![no_main]

use pdf_raster::fuzz::{check, over};
use pdf_structure::fuzz::options;
use pdf_structure::read::Reading;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    check(data);
    if let Ok(reading) = Reading::open(data, &options()) {
        over(&reading, reading.trailer().dict());
    }
});
