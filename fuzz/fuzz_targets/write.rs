//! Writing a file read from its bytes, whole and as an update, each way
//! the input picks (a table, a stream, packed; encrypted as it was read,
//! decrypted, or under a new encryption), under small limits: no panic,
//! and where it read soundly, written and read again the same objects.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_structure::fuzz::write_file(data));
