//! Every promise the tokenizer makes, on any input, from any offset, under
//! ISO 32000-2's rules and PDFium's, and the two side by side.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_tokenizer::fuzz::tokenize(data));
