//! Every promise the tokenizer makes, on any input, from any offset.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_tokenizer::fuzz::tokenize(data));
