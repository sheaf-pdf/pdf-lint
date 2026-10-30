//! A page's content, on any bytes: read whole and as two streams, joined
//! alike, its tokens tiling it, each operand read as an object, its
//! operations its tokens grouped, and read again under small caps, the
//! same up to the cap's error; each read through a sink that keeps its
//! faults the same, the faults inside it in the order of their ends; and
//! each operation typed and stepped through the graphics objects' states.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_content_streams::fuzz::content(data));
