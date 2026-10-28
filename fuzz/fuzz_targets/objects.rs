//! The parser's promises, on any input, read as a value or an indirect
//! object as the input's first bytes choose, and the writer's on what it
//! read: written, it reads back as the same value.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_objects::fuzz::parse(data));
