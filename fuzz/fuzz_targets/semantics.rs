//! A file's common structures, on any bytes: read as a series of objects,
//! the name and number trees its catalog holds, and each of its first objects
//! as a tree's root of either kind, walked: the same whatever hears their
//! faults, what a plain walk gives under small depths too, what's cut under a
//! few nodes only what's there, and written, read back; its page tree by both
//! rules, and each of its first dictionaries as a catalog: the same whatever
//! hears their faults, within the caps, and alike where ISO's rules say
//! nothing's wrong; the same bytes read as a date, written and read back, and
//! as a count of seconds; and read as a file, its trees and pages over a
//! reading and the document it makes the same.

#![no_main]

use pdf_semantics::fuzz::{dates, over, walks};
use pdf_structure::bytes::Reading;
use pdf_structure::fuzz::options;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    walks(data);
    dates(data);
    if let Ok(reading) = Reading::open(data, &options()) {
        let objects = &reading;
        let read = over(&objects, reading.trailer().dict());
        let document = reading.to_document();
        let document = document.expect("bytes in memory make a document");
        let objects = &document;
        assert_eq!(over(&objects, document.trailer.dict()), read);
    }
});
