//! A file's color spaces, on any bytes: read as a series of objects, the first
//! of them each counted, its count within what it could be, within every cap,
//! the same under larger and smaller caps where none was met and a failing
//! resolver's reason kept; each read as a typed space, the same whatever the
//! sink, a read that fails included, and counting as it was counted where no
//! fault was told and no cap met; and each read as resources, the same by name as in order; and
//! read as a file, what every object counts, says and names over a reading and
//! the document it makes the same.

#![no_main]

use pdf_color::fuzz::{counts, over};
use pdf_structure::bytes::Reading;
use pdf_structure::fuzz::options;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    counts(data);
    if let Ok(reading) = Reading::open(data, &options()) {
        let read = over(&reading, reading.trailer().dict());
        let document = reading.to_document();
        let document = document.expect("bytes in memory make a document");
        assert_eq!(over(&document, document.trailer.dict()), read);
    }
});
