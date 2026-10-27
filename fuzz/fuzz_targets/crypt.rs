//! The security handler's promises, on any dictionary and ciphertext.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pdf_crypt::fuzz::handler(data));
