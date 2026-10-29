//! The parser's promises, on any input, read as a value or an indirect
//! object as the input's first bytes choose, and as a window of a file
//! cut where they choose, which is short or reads as the whole input does;
//! what it read's references, walked with a stack of their own, as a
//! recursive walk gives them; and the writer's on what it read: written,
//! it reads back as the same value, and written through a toy encryptor,
//! undone, it's what it wrote plainly.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_objects::fuzz::parse(data));
