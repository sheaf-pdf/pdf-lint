//! Reading a file from its bytes and checking it, on any bytes: no panic,
//! the same twice, every finding's bytes inside what was read.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_lint::fuzz::check_file(data));
