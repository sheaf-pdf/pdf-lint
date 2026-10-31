//! A page's content, on any bytes: read whole and as two streams, joined
//! alike, its tokens tiling it, each operand read as an object and its
//! number as PDFium reads it, its operations its tokens grouped, and read
//! again under small caps, what finding inline images' ends may cost among
//! them, the same up to the cap's error; each read through a sink that
//! keeps its faults the same, the faults inside it in the order of their
//! ends, where an object ends an inline image's dictionary and each lone
//! `BI` told; each operation typed and stepped through the graphics
//! objects' states; its `q`/`Q` balance, marked content and resource names
//! what their steppers give, an operation at a time, and what it keeps with
//! its ranges taken out, those at the top and every one, in no order and
//! overlapping; and its searches for an inline image's end and its walks
//! past one to `EI`, remembered or not, and its skims of arrays and
//! dictionaries, what plain ones find. With the `image-ends` feature, which
//! the fuzz crate turns on, it's read with an inline image's DCT and
//! CCITTFax data decoded to find where it ends, and its tokens tile it read
//! by the stand-in too.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_content_streams::fuzz::content(data));
