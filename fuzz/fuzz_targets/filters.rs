//! The filters' promises, on any input, with any filters the input's
//! first bytes choose, on each backend, the first of them on data that may
//! run on too, each under qpdf's reading and PDFium's, PDFium's held to
//! qpdf's where neither of its own faults is said; and the encoders', with
//! the one or two encoders they choose.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    pdf_filters::fuzz::filters(data);
    pdf_filters::fuzz::encoders(data);
});
