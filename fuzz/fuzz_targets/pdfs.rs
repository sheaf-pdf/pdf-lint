//! A file read through the facade, on any bytes: opened from memory, its
//! version, catalog, document information dictionary, claims and page tree,
//! each page's content joined within its cap and its operations walked, its
//! fonts and its text, which fails only as its content does, and its
//! linearization's verdict, holding and judged whole only where it's
//! linearized; every answer the same with faults kept as with none looked
//! for, and under small caps the same promises; its file edited, a page
//! added, and saved whole, written or refused. And where it isn't
//! encrypted, its document written in the clear and under a new encryption,
//! its key drawn from the input and its IVs derived: read with no password,
//! each page's content refused as locked or read as in the clear; unlocked by
//! the password, read as in the clear. And a new document of its first pages,
//! their content written again operation by operation, saved whole, with a
//! table, linearized and under a new encryption, each read back as written,
//! the same in every kind; one of no page refused.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    pdfs::fuzz::read(data);
    pdfs::fuzz::write(data);
});
