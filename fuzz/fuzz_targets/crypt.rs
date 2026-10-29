//! The security handler's promises, on any dictionary and ciphertext; and
//! a password's forms', on any text.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    pdf_crypt::fuzz::handler(data);
    pdf_crypt::fuzz::password(data);
});
