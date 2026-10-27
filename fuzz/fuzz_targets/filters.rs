//! The filters' promises, on any input, with any filters the input's
//! first bytes choose.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_filters::fuzz::filters(data));
