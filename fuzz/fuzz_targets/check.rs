//! Reading a file from its bytes, beside its reading, and checking it, its
//! content among it, on any bytes: no panic, the same twice, every
//! finding's bytes inside what was read, and its content moving nothing a
//! check of the document alone finds.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_lint::fuzz::check_file(data));
