//! Reading a file from its bytes, on any bytes: no panic, the same twice,
//! every place it gives inside what was read; and its revision before an
//! update, and the file opened by the owner's password, as the input's
//! first bytes choose, the same.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_structure::fuzz::read_file(data));
