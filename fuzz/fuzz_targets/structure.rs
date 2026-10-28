//! Reading a file from its bytes, on any bytes, under small limits: no
//! panic, the same twice, every place it gives inside what was read; read
//! on demand, in an order the input chooses, the whole read, where neither
//! decodes as far as the budget; read by position, from a first window the
//! input chooses, the whole read, each stream's data and decoded data as
//! in memory; and its revision before an update, and the file opened by
//! the owner's password, as the input's first bytes choose, the same.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_structure::fuzz::read_file(data));
