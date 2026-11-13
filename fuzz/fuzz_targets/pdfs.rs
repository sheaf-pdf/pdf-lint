//! A file read through the facade, on any bytes: opened from memory, its
//! version, catalog, document information dictionary, claims and page tree,
//! each page's content joined within its cap and its operations walked, and
//! its linearization's verdict, holding and judged whole only where it's
//! linearized; every answer the same with faults kept as with none looked
//! for, and under small caps the same promises. And where it isn't
//! encrypted, its document written in the clear and under a new encryption,
//! its key drawn from the input and its IVs derived: read with no password,
//! each page's content refused as locked or read as in the clear; unlocked by
//! the password, read as in the clear.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    pdfs::fuzz::read(data);
});
