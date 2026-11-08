//! A file with bytes before its header, whose offsets count from the header
//! (ISO 32000-2, 7.5.2): read as the file without them is, every way, each
//! object at its place past them, every position a reading gives in the
//! file's bytes, and nothing told but where the header is
//! ([`Fault::HeaderAt`]). A position a caller gives, an earlier section's
//! ([`Options::section`]), is taken as it is.

use pdf_fixtures::fixture::{self, Xref};
use pdf_objects::Span;

use super::lengths::every_way;
use super::tests::{KINDS, clean, compressed, root, settings, to_encrypt, updated_with_padding};
use super::*;
use crate::fuzz::{Counted, open};
use crate::source::Source;

/// Bytes some writers put before `%PDF-`: one, a line, and a hundred.
fn prefixes() -> [Vec<u8>; 3] {
    [b" ".to_vec(), b"junk\r\n".to_vec(), vec![b'~'; 100]]
}

/// A hybrid file (7.5.8.4): a table whose trailer's `/XRefStm` gives a
/// cross-reference stream listing what the table leaves free, object 3.
fn hybrid() -> Vec<u8> {
    let mut file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
    let three = file.len();
    file.extend_from_slice(b"3 0 obj\n(three)\nendobj\n");
    let stream = file.len();
    let rows: Vec<u8> = [three, stream]
        .iter()
        .flat_map(|&at| {
            [
                vec![1],
                u16::try_from(at).unwrap().to_be_bytes().to_vec(),
                vec![0],
            ]
        })
        .flatten()
        .collect();
    let head = format!(
        "2 0 obj\n<< /Type /XRef /Size 4 /W [1 2 1] /Index [3 1 2 1] /Length {} >>\nstream\n",
        rows.len()
    );
    file.extend_from_slice(head.as_bytes());
    file.extend_from_slice(&rows);
    file.extend_from_slice(b"\nendstream\nendobj\n");
    let table = file.len();
    file.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n0000000009 00000 n \n");
    file.extend_from_slice(b"0000000000 00000 f \n0000000000 00000 f \n");
    let trailer = format!("trailer\n<< /Size 4 /Root 1 0 R /XRefStm {stream} >>\n");
    file.extend_from_slice(trailer.as_bytes());
    file.extend_from_slice(format!("startxref\n{table}\n%%EOF\n").as_bytes());
    file
}

/// The files read here, each its name, its bytes, and the passwords it's
/// read by: a table, a stream, an update along `/Prev`, object streams, a
/// hybrid table, and one encrypted under each method, every one read
/// without a fault.
fn files() -> Vec<(String, Vec<u8>, Vec<Password<'static>>)> {
    let mut files = vec![
        (
            "a table".to_string(),
            fixture::file("1.7", &clean(), root(), Xref::Table),
            vec![],
        ),
        (
            "a stream".to_string(),
            fixture::file("1.7", &clean(), root(), Xref::Stream),
            vec![],
        ),
        ("an update".to_string(), updated_with_padding().0, vec![]),
        ("object streams".to_string(), compressed(false).0, vec![]),
        ("a hybrid table".to_string(), hybrid(), vec![]),
    ];
    for kind in KINDS {
        let file = fixture::encrypted(&settings(kind, b"user"), &to_encrypt(&[]), root());
        files.push((format!("{kind:?}"), file, vec![Password("user")]));
    }
    files
}

/// `span` in a file `by` bytes longer before its header: the same where
/// it's in an object stream's decoded data.
fn shifted(span: Span, by: usize) -> Span {
    match span.stream {
        Some(_) => span,
        None => Span::new(None, span.start + by, span.end + by),
    }
}

/// Where the bytes before the header are said: their count, at `%PDF-`.
fn header_at(by: usize) -> (Place, Fault) {
    let at = Place::new(None, Some(Span::new(None, by, by + 5)));
    (at, Fault::HeaderAt { at: by })
}

/// That `doc`, of a file `by` bytes longer before its header, is `plain`,
/// the file without them, read alike: each object the same, at its place
/// past them, its data's too, and nothing told but where the header is,
/// where faults are `kept`.
fn read_past(doc: &Document, plain: &Document, by: usize, kept: bool, name: &str) {
    let ids: Vec<ObjectId> = doc.objects().map(|(id, _)| id).collect();
    let plain_ids: Vec<ObjectId> = plain.objects().map(|(id, _)| id).collect();
    assert_eq!(ids, plain_ids, "{name}");
    for (id, object) in doc.objects() {
        let mut object = object.clone();
        let mut want = plain.get(id).unwrap().clone();
        object.unplace();
        want.unplace();
        assert_eq!(object, want, "{name}: {id}");
        let (record, want) = (doc.record(id).unwrap(), plain.record(id).unwrap());
        assert_eq!(record.origin, want.origin, "{name}: {id}");
        assert_eq!(record.at, shifted(want.at, by), "{name}: {id}");
        let extent = |e: &DataExtent| (shifted(e.at, by), e.declared, e.indirect_length);
        let data = want.data.as_ref().map(extent);
        let got = record
            .data
            .as_ref()
            .map(|e| (e.at, e.declared, e.indirect_length));
        assert_eq!(got, data, "{name}: {id}");
    }
    let want: Vec<(Place, Fault)> = match kept {
        true => vec![header_at(by)],
        false => vec![],
    };
    assert_eq!(doc.faults(), want, "{name}");
    assert_eq!(doc.facts.header_offset, Some(by), "{name}");
    let newest = plain.facts.newest.map(|(at, kind)| (at + by, kind));
    assert_eq!(doc.facts.newest, newest, "{name}");
    assert_eq!(doc.trailer.unwritten, None, "{name}");
}

/// Each file, its bytes past one byte, a line and a hundred before its
/// header, read whole, on demand by number and from the last back, and by
/// position in windows of 1, 7, 64 and 4096 bytes, an object's reaching
/// the next listed offset and not: each object at its place from the
/// header, nothing told but where the header is; and with faults off,
/// nothing told.
#[test]
fn bytes_before_the_header_are_read_past_every_way() {
    for (name, plain, passwords) in files() {
        let options = Options {
            passwords: &passwords,
            ..Options::DEFAULT
        };
        let unprefixed = document(&plain, &options);
        assert_eq!(unprefixed.faults(), [], "{name}");
        assert_eq!(unprefixed.trailer.unwritten, None, "{name}");
        for prefix in prefixes() {
            let by = prefix.len();
            let file = [&prefix[..], &plain].concat();
            for (way, doc) in every_way(&file, &passwords) {
                read_past(&doc, &unprefixed, by, true, &format!("{name}, {by}, {way}"));
            }
            let quiet = Options {
                faults: Faults::Off,
                ..options
            };
            let doc = document(&file, &quiet);
            read_past(
                &doc,
                &unprefixed,
                by,
                false,
                &format!("{name}, {by}, no faults"),
            );
        }
    }
}

/// `rows` as a file `by` bytes longer before its header has them: each
/// place past them, its trailer's values as written.
fn rows_past(rows: &SectionRows, by: usize) -> SectionRows {
    let mut rows = rows.clone();
    rows.at += by;
    for row in &mut rows.rows {
        if let Some(XrefEntry::InUse { offset, .. }) = &mut row.entry {
            *offset += by;
        }
    }
    rows.first_row = rows
        .first_row
        .map(|(space, first)| (space + by, first + by));
    rows.prev = rows.prev.map(|at| at + by);
    rows.startxref = rows.startxref.map(|at| at + by);
    rows.trailer.object.unplace();
    rows.hybrid = rows.hybrid.map(|hybrid| Box::new(rows_past(&hybrid, by)));
    rows
}

/// A file with bytes before its header, its sections each on its own
/// ([`Reading::sections`]): each place in the file's bytes, every row's
/// offset, its first row's, its `/Prev`'s, its `startxref`'s and a hybrid
/// stream's, as the reading's; and its first object at its place.
#[test]
fn a_prefixed_files_sections_are_at_their_places_in_its_bytes() {
    for (name, plain, passwords) in files() {
        let options = Options {
            passwords: &passwords,
            ..Options::DEFAULT
        };
        let unprefixed = Reading::open(&plain[..], &options).unwrap();
        let want = unprefixed.sections().unwrap();
        assert_eq!(want.stopped, None, "{name}");
        let first = unprefixed.first_object().unwrap().unwrap();
        for prefix in prefixes() {
            let by = prefix.len();
            let file = [&prefix[..], &plain].concat();
            for bound in [false, true] {
                let source = Counted::new(&file, 7);
                let reading = open(&source, &passwords, &Options::DEFAULT, bound).unwrap();
                let sections = reading.sections().unwrap();
                assert_eq!(sections.stopped, None, "{name}, {by}");
                let read: Vec<SectionRows> = (sections.sections.iter())
                    .map(|rows| rows_past(rows, 0))
                    .collect();
                let past: Vec<SectionRows> = (want.sections.iter())
                    .map(|rows| rows_past(rows, by))
                    .collect();
                assert_eq!(read, past, "{name}, {by}");
                // The reading's entries are the newest section's rows.
                let newest = (reading.facts().newest).map(|(at, _)| at);
                assert_eq!(newest, Some(past[0].at), "{name}, {by}");
                let got = reading.first_object().unwrap().unwrap();
                assert_eq!((got.at, got.end), (first.at + by, first.end + by));
                assert_eq!(got.id, first.id);
            }
        }
    }
}

/// An update to a file with bytes before its header: its revisions and the
/// section its update follows at their places in the file's bytes, and
/// the reading as of its first revision opened at that place, taken as it
/// is: the section's own offset as written, which counts from the header,
/// isn't where it is, and isn't read there.
#[test]
fn an_earlier_revision_of_a_prefixed_file_is_read_as_of_its_place() {
    let (plain, first) = updated_with_padding();
    fn as_of(file: &[u8], section: usize) -> Reading<&[u8]> {
        let mut options = Options::DEFAULT;
        options.section = Some(section);
        Reading::open(file, &options).unwrap()
    }
    let earlier = as_of(&plain, first);
    let unprefixed = Reading::open(&plain[..], &Options::DEFAULT).unwrap();
    for prefix in prefixes() {
        let by = prefix.len();
        let file = [&prefix[..], &plain].concat();
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        let at: Vec<usize> = reading.revisions().iter().map(|r| r.at).collect();
        let want: Vec<usize> = (unprefixed.revisions().iter()).map(|r| r.at + by).collect();
        assert_eq!(at, want, "{by}");
        let ends = |r: &Revision| r.end.map(|end| (end.eof(), end.line()));
        let ended: Vec<_> = reading.revisions().iter().map(ends).collect();
        let want_ended: Vec<_> = (unprefixed.revisions().iter())
            .map(|r| ends(r).map(|(eof, line)| (eof + by, line + by)))
            .collect();
        assert_eq!(ended, want_ended, "{by}");
        assert_eq!(reading.update().map(|u| u.previous), Some(first + by));
        // As of its first revision, at its place: that revision.
        let as_first = as_of(&file, first + by);
        assert_eq!(as_first.trailer().unwritten, None, "{by}");
        assert_eq!(as_first.facts().newest.map(|(at, _)| at), Some(first + by));
        let ids: Vec<ObjectId> = as_first.ids().collect();
        assert_eq!(ids, earlier.ids().collect::<Vec<_>>(), "{by}");
        let sections = as_first.sections().unwrap();
        assert_eq!(sections.sections.len(), 1, "{by}");
        assert_eq!(sections.sections[0].at, first + by, "{by}");
        // As of the offset as written, a hundred bytes before the section:
        // not read there, and the file scanned.
        if by == 100 {
            let written = as_of(&file, first);
            assert!(written.trailer().unwritten.is_some(), "{by}");
            assert_eq!(written.facts().newest, None, "{by}");
        }
    }
}
