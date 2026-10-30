//! A page's content, on any bytes: read whole and as two streams, joined
//! alike, its tokens tiling it, each operand read as an object, its
//! operations its tokens grouped, and read again under small caps, the
//! same up to the cap's error; each read through a sink that keeps its
//! faults the same, the faults inside it in the order of their ends; each
//! operation typed and stepped through the graphics objects' states; and
//! its `q`/`Q` balance, marked content and resource names what their
//! steppers give, an operation at a time, and what it keeps with its
//! ranges at the top taken out.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_content_streams::fuzz::content(data));
