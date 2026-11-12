//! A file's common structures, on any bytes: read as a series of objects, the
//! name and number trees its catalog holds, and each of its first objects as a
//! tree's root of either kind, walked: the same whatever hears their faults,
//! what a plain walk gives under small depths too, a node with neither `/Kids`
//! nor entries, or both, said as often as it finds one, what's cut under a few
//! nodes only what's there, and written, read back; its page tree by both
//! rules, and each of its first dictionaries as a catalog: the same whatever
//! hears their faults, within the caps, and alike where ISO's rules say
//! nothing's wrong; its first bytes as a page tree to write, balanced, read
//! back by both rules with nothing told, or refused for what wouldn't, no id
//! asked for but one refused; the same bytes read as a date, written and read
//! back, and as a count of seconds; read as an XMP packet, the same whatever
//! hears its faults and with its empty elements written open and closed, an
//! error holding what it read, and what smaller caps read what the default ones
//! read, as a claim's text beside an element, never taken, and as a file's
//! metadata stream through its streams' data, the same, and past a cap, nothing
//! read; what it claims, and claims made of its text, written and read back as
//! they are, or refused for a value that wouldn't; and read as a file, its
//! trees, pages and metadata stream over a reading and the document it makes
//! the same.

#![no_main]

use pdf_semantics::fuzz::{dates, over, walks, xmp};
use pdf_structure::read::Reading;
use pdf_structure::fuzz::options;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    walks(data);
    dates(data);
    xmp(data);
    if let Ok(reading) = Reading::open(data, &options()) {
        let read = over(&reading, reading.trailer().dict());
        let document = reading.to_document();
        let document = document.expect("bytes in memory make a document");
        assert_eq!(over(&document, document.trailer().dict()), read);
    }
});
