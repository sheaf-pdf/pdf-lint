//! Reading a file from its bytes, beside its reading, and checking it, its
//! content and its fonts among it, on any bytes: no panic, the same twice,
//! every finding's bytes inside what was read, and what only the reading
//! gives, its content, its fonts' streams, its catalog's XMP packet as the
//! reading decodes it, a stream whose only fault is its Flate checksum read
//! whole, an earlier revision's objects a later one wrote over or freed,
//! read as that revision was written, and its linearization, moving nothing
//! else a check of the document alone finds.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_lint::fuzz::check_file(data));
