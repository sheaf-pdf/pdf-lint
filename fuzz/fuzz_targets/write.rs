//! Writing a file read from its bytes whole, each way the input picks (a
//! table, a stream, packed; encrypted as it was read, decrypted, or under
//! a new encryption; renumbered or not), each object as read copied where
//! the rules for a copy hold, renumbered in place; then, as the input
//! picks, an update of its objects, one changed, renumbered as the whole
//! file was but what an update leaves where it is, or the file read again
//! with a byte flipped after it was read and written whole, refused as
//! changed or the same objects. Under small limits: no panic, and where it
//! read soundly, written and read again the same objects, each at the
//! number it was written at.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_structure::fuzz::write_file(data));
