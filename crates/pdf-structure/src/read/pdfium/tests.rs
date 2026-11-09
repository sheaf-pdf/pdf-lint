//! Files read as PDFium reads them ([`Recovery::Pdfium`]), the cases
//! `tests/pdfium` asks PDFium of among them: where the file is scanned and
//! why, what the scan lists, and what of the sections it keeps, each read
//! alike every way a reading reads; and where a file whose sections read
//! and give a page reads as [`Recovery::Verified`] reads it.

use pdf_crypt::Password;
use pdf_objects::object::{ObjectId, Value};

use super::super::{Fault, Faults, Options, Reading, Recovery, Unwritten, document};
use crate::fuzz::{Counted, open};
use crate::source::{Document, Origin, Source};
use crate::write::{self, Base};

/// A case of `tests/pdfium`, by its name past `pages-`.
macro_rules! case {
    ($name:literal) => {
        &include_bytes!(concat!("../../../tests/pdfium/cases/pages-", $name, ".pdf"))[..]
    };
}

/// The options a file is opened with under `recovery`, by `passwords`.
fn under<'a>(recovery: Recovery, passwords: &'a [Password<'a>]) -> Options<'a> {
    let mut options = Options::DEFAULT;
    options.recovery = recovery;
    options.passwords = passwords;
    options
}

/// `file` read under `options` every way the tests read: whole, on demand by
/// number and from the last back, and by position in first windows of 1, 7,
/// 64 and 4096 bytes, an object's reaching the next listed offset and not;
/// each the whole read; and with faults off the same objects, records and
/// facts, and no fault.
fn read(file: &[u8], options: &Options) -> Document {
    let whole = document(file, options);
    for back in [false, true] {
        let reading = Reading::open(file, options).unwrap();
        let mut ids: Vec<ObjectId> = reading.ids().collect();
        if back {
            ids.reverse();
        }
        ids.iter().for_each(|&id| drop(reading.get(id)));
        assert_eq!(
            reading.into_document().unwrap(),
            whole,
            "on demand, back {back}"
        );
    }
    for first in [1, 7, 64, 4096] {
        for bound in [true, false] {
            let source = Counted::new(file, first);
            let reading = open(&source, options.passwords, options, bound).unwrap();
            let ids: Vec<ObjectId> = reading.ids().collect();
            ids.iter().rev().for_each(|&id| drop(reading.get(id)));
            let doc = reading.into_document().unwrap();
            assert_eq!(doc, whole, "by position, {first} bytes, {bound}");
        }
    }
    let mut quiet = *options;
    quiet.faults = Faults::Off;
    let off = document(file, &quiet);
    assert_eq!(off.faults(), [], "faults off");
    assert_eq!(off.facts, whole.facts, "faults off");
    assert_eq!(off.trailer, whole.trailer, "faults off");
    let ids: Vec<ObjectId> = whole.objects().map(|(id, _)| id).collect();
    assert_eq!(off.objects().map(|(id, _)| id).collect::<Vec<_>>(), ids);
    for (id, object) in whole.objects() {
        assert_eq!(off.get(id), Ok(object), "faults off: {id}");
        assert_eq!(off.record(id), whole.record(id), "faults off: {id}");
    }
    whole
}

/// `file` read under [`Recovery::Pdfium`] every way ([`read`]).
fn as_pdfium(file: &[u8]) -> Document {
    read(file, &under(Recovery::Pdfium, &[]))
}

/// Where `file` has `text`, the first time.
fn place(file: &[u8], text: &[u8]) -> usize {
    file.windows(text.len()).position(|w| w == text).unwrap()
}

/// Where `file` has `text`, the last time.
fn last(file: &[u8], text: &[u8]) -> usize {
    file.windows(text.len()).rposition(|w| w == text).unwrap()
}

/// Where `file`'s last table starts: its `xref`, a line of its own.
fn table(file: &[u8]) -> usize {
    last(file, b"\nxref\n") + 1
}

/// Each of `numbers` read in `doc`, at its own `n 0 obj` in `file` (the
/// last), its origin `Recovered`, as a scan finds it.
fn each_found_by_the_scan(doc: &Document, file: &[u8], numbers: &[u32]) {
    for &n in numbers {
        let id = ObjectId::new(n, 0);
        assert!(doc.get(id).is_ok(), "{id}");
        let record = doc.record(id).unwrap();
        assert_eq!(record.origin, Origin::Recovered, "{id}");
        assert_eq!(
            record.at.start,
            last(file, format!("{n} 0 obj").as_bytes()),
            "{id}"
        );
    }
}

/// The widths of the pages the page tree's root, object `root`, lists
/// directly, each its number: none for a kid that doesn't read.
fn kids(doc: &Document, root: u32) -> Vec<Option<u32>> {
    let node = doc.get(ObjectId::new(root, 0)).unwrap().as_dict().unwrap();
    let kids = node.get(b"Kids").unwrap().as_array().unwrap();
    (kids.iter())
        .map(|kid| match kid.value() {
            Value::Ref(id) => doc.get(*id).ok().map(|_| id.number),
            _ => None,
        })
        .collect()
}

/// An update whose `/Prev` is past the file's end: under the verified
/// recovery the update's section is kept and said; as PDFium reads it, the
/// file is scanned for that, the trailer saying the section's reason, its
/// fault told as well, every object read where the scan finds it, the
/// update's page tree over the first's; and the scan read whole, the
/// facts saying it.
#[test]
fn a_later_section_that_doesnt_read_has_the_file_scanned() {
    let file = case!("update-prev-past-the-end");
    let verified = read(file, &under(Recovery::Verified, &[]));
    assert_eq!(verified.trailer.unwritten, None);
    assert!(verified.get(ObjectId::new(1, 0)).is_err());
    let doc = as_pdfium(file);
    let said: Vec<&Fault> = (doc.faults().iter())
        .filter_map(|(_, f)| matches!(f, Fault::Section { .. }).then_some(f))
        .collect();
    let [Fault::Section { at: 999_999, why }] = &said[..] else {
        panic!("{said:?}");
    };
    assert_eq!(doc.trailer.unwritten.as_ref(), Some(&**why));
    each_found_by_the_scan(&doc, file, &[1, 2, 3, 4, 6]);
    assert_eq!(kids(&doc, 2), [Some(3), Some(4), Some(6)]);
    assert!(doc.facts.recovered);
    assert_eq!(doc.trailer.stream, None);
    assert_eq!(
        doc.trailer.dict().get(b"Size").and_then(|s| s.as_integer()),
        Some(7)
    );
}

/// The first section's `/Prev` the update's own: both read, and the loop
/// said; as PDFium reads it, the file scanned for that, the trailer saying
/// where the chain comes back to.
#[test]
fn a_chain_that_comes_back_has_the_file_scanned() {
    let file = case!("update-first-prev-back-to-it");
    let update = table(file);
    let verified = read(file, &under(Recovery::Verified, &[]));
    assert_eq!(verified.trailer.unwritten, None);
    let doc = as_pdfium(file);
    let loops: Vec<&Fault> = (doc.faults().iter())
        .filter_map(|(_, f)| matches!(f, Fault::Loop { .. }).then_some(f))
        .collect();
    assert_eq!(loops, [&Fault::Loop { at: update }]);
    assert_eq!(doc.trailer.unwritten, Some(Unwritten::Loop { at: update }));
    each_found_by_the_scan(&doc, file, &[1, 2, 3, 4, 6]);
    assert_eq!(kids(&doc, 2), [Some(3), Some(4), Some(6)]);
}

/// The first object a file's only table lists, by number, not at its row:
/// as PDFium reads it, the file scanned for that, the trailer saying which
/// object and where the table put it; and a misnumbered subsection, whose
/// first object in use is 2 at 1's place, likewise.
#[test]
fn a_first_object_not_where_its_table_puts_it_has_the_file_scanned() {
    let file = case!("first-object-elsewhere");
    let doc = as_pdfium(file);
    let why = Unwritten::Misplaced {
        table: table(file),
        id: ObjectId::new(1, 0),
        offset: place(file, b"2 0 obj"),
    };
    assert_eq!(doc.trailer.unwritten, Some(why));
    each_found_by_the_scan(&doc, file, &[1, 2, 3, 4, 5]);
    assert_eq!(kids(&doc, 4), [Some(1), Some(2), Some(3)]);
    let file = case!("misnumbered-subsection");
    let doc = as_pdfium(file);
    let why = Unwritten::Misplaced {
        table: table(file),
        id: ObjectId::new(2, 0),
        offset: place(file, b"1 0 obj"),
    };
    assert_eq!(doc.trailer.unwritten, Some(why));
    each_found_by_the_scan(&doc, file, &[1, 2, 3, 4]);
}

/// The first-written table's first object not at its row, and a later
/// section's `/XRefStm` that doesn't read: as PDFium reads it, the file
/// scanned for the first, which PDFium checks before it reads any later
/// section's rows, so an update's table, which lists page 6 where a scan
/// doesn't find it, isn't read; the `/XRefStm` said all the same. Under
/// the verified recovery, read by the sections, page 6 among them, the
/// first table's entries at its first object's shift.
#[test]
fn a_first_object_not_at_its_row_is_found_before_a_later_xrefstm() {
    let file = case!("first-object-elsewhere-and-xrefstm-unread");
    let doc = as_pdfium(file);
    let why = Unwritten::Misplaced {
        table: place(file, b"\nxref\n") + 1,
        id: ObjectId::new(1, 0),
        offset: place(file, b"2 0 obj"),
    };
    assert_eq!(doc.trailer.unwritten, Some(why));
    let stms: Vec<&Fault> = (doc.faults().iter())
        .filter_map(|(_, f)| matches!(f, Fault::XrefStm { .. }).then_some(f))
        .collect();
    let [Fault::XrefStm { at: 9_999_999, .. }] = &stms[..] else {
        panic!("{stms:?}");
    };
    assert!(doc.record(ObjectId::new(6, 0)).is_none());
    assert_eq!(kids(&doc, 2), [Some(3), None]);
    let verified = read(file, &under(Recovery::Verified, &[]));
    assert_eq!(verified.trailer.unwritten, None);
    assert_eq!(kids(&verified, 2), [Some(3), Some(6)]);
    let one = verified.record(ObjectId::new(1, 0)).unwrap();
    let by = place(file, b"1 0 obj") as i64 - place(file, b"2 0 obj") as i64;
    assert_eq!(one.origin, Origin::Shifted { by });
    let six = verified.record(ObjectId::new(6, 0)).unwrap();
    assert_eq!(six.origin, Origin::File);
    assert_eq!(six.at.start, place(file, b"6 0 obj"));
}

/// A `/Prev` of 0, and an update's `/XRefStm` of 0, the header's place,
/// where no section is: as PDFium reads them, none, so the file is read by
/// its sections, not scanned, and page 6, which a scan wouldn't find, is
/// read where they put it; that none reads there is said, as the verified
/// recovery says it, which reads each file alike.
#[test]
fn a_prev_or_xrefstm_of_0_is_none_as_pdfium_reads_it() {
    for file in [case!("prev-zero"), case!("update-xrefstm-zero")] {
        let doc = as_pdfium(file);
        assert_eq!(doc.trailer.unwritten, None);
        let six = doc.record(ObjectId::new(6, 0)).unwrap();
        assert_eq!(six.origin, Origin::File);
        assert_eq!(six.at.start, place(file, b"6 0 obj"));
        let said = (doc.faults().iter())
            .filter(|(_, f)| {
                matches!(
                    f,
                    Fault::Section { at: 0, .. } | Fault::XrefStm { at: 0, .. }
                )
            })
            .count();
        assert_eq!(said, 1, "{:?}", doc.faults());
        let verified = read(file, &under(Recovery::Verified, &[]));
        assert_eq!(doc, verified);
    }
}

/// A catalog, or a page tree's root, its row at another object, where the
/// table's first object is where it's put: the sections read, and, as
/// PDFium reads it, the file scanned after them for want of a page, the
/// trailer saying so; under the verified recovery, read as written.
#[test]
fn a_trailer_that_gives_no_page_has_the_file_scanned_after_its_sections() {
    for (file, root, pages) in [
        (case!("catalog-elsewhere"), 1, vec![Some(2), Some(3)]),
        (case!("pages-elsewhere"), 2, vec![Some(4), Some(5)]),
    ] {
        let verified = read(file, &under(Recovery::Verified, &[]));
        assert_eq!(verified.trailer.unwritten, None);
        assert!(!verified.facts.recovered);
        let doc = as_pdfium(file);
        assert_eq!(doc.trailer.unwritten, Some(Unwritten::NoPages));
        assert!(doc.facts.recovered);
        each_found_by_the_scan(&doc, file, &[1, 2, 3, 4, 5, 6]);
        assert_eq!(kids(&doc, root), pages);
        // Scanned, as a file whose sections don't read is: no revision,
        // and no section said.
        assert_eq!(doc.revisions, []);
        assert_eq!((doc.facts.sections, doc.facts.newest), (0, None));
        assert_eq!(verified.revisions.len(), 1);
    }
}

/// An object a scan doesn't find, written inside another stream's data,
/// where its table lists it: where PDFium had read the table, at its first
/// object, the object read where the table puts it, as written; where it
/// hadn't, at an update's `/Prev` that doesn't read, not listed.
#[test]
fn what_the_scan_doesnt_find_is_read_where_the_sections_put_it_once_pdfium_read_them() {
    let file = case!("hidden-in-a-stream");
    let doc = as_pdfium(file);
    let hidden = doc.record(ObjectId::new(6, 0)).unwrap();
    assert_eq!(hidden.origin, Origin::File);
    assert_eq!(hidden.at.start, place(file, b"6 0 obj"));
    each_found_by_the_scan(&doc, file, &[1, 2, 3, 4]);
    assert_eq!(kids(&doc, 2), [Some(1), Some(6)]);
    let file = case!("update-prev-past-the-end-hidden");
    let doc = as_pdfium(file);
    assert!(doc.record(ObjectId::new(6, 0)).is_none());
    assert_eq!(kids(&doc, 2), [Some(3), Some(4), None]);
}

/// A scanned file's object stream's members listed where the stream is in
/// the file: over an object of their number before it, and under one after
/// it. The member over an object the scan found isn't among those the
/// fault counts, which it found no offset of.
#[test]
fn a_scans_members_are_listed_where_their_stream_is_in_the_file() {
    let file = case!("member-after-an-object");
    let doc = as_pdfium(file);
    let three = doc.record(ObjectId::new(3, 0)).unwrap();
    assert_eq!(
        three.origin,
        Origin::Stream {
            stream: 9,
            index: 0
        }
    );
    let members: Vec<&Fault> = (doc.faults().iter())
        .filter_map(|(_, f)| matches!(f, Fault::Members { .. }).then_some(f))
        .collect();
    assert_eq!(members, [] as [&Fault; 0]);
    let verified = read(file, &under(Recovery::Verified, &[]));
    let three = verified.record(ObjectId::new(3, 0)).unwrap();
    assert_eq!(three.origin, Origin::Recovered);
    let file = case!("object-after-a-member");
    let doc = as_pdfium(file);
    each_found_by_the_scan(&doc, file, &[1, 2, 3, 4, 9]);
}

/// A scanned file whose objects are in object streams, under AES-256: as
/// PDFium scans it, reading its streams as written, none of their members
/// is listed, by the password or not, and the streams are said unread;
/// opened by a wrong password and unlocked later, read as opened by it.
#[test]
fn an_encrypted_files_object_streams_list_no_member_by_the_scan() {
    let file = crate::read::recovery::tests::scanned_encrypted_with_object_streams();
    let user = [Password("user")];
    let opened = read(&file, &under(Recovery::Pdfium, &user));
    assert!(opened.trailer.unwritten.is_some());
    for n in 1..=3 {
        assert!(opened.record(ObjectId::new(n, 0)).is_none(), "{n}");
    }
    let members: Vec<&Fault> = (opened.faults().iter())
        .filter_map(|(_, f)| matches!(f, Fault::Members { .. }).then_some(f))
        .collect();
    let [
        Fault::Members {
            objects: 0,
            streams: 0,
            unread: 1..,
        },
    ] = &members[..]
    else {
        panic!("{members:?}");
    };
    let wrong = [Password("wrong")];
    let reading = Reading::open(&file[..], &under(Recovery::Pdfium, &wrong)).unwrap();
    let unlocked = reading.unlock(&user[0].encodings()).ok().unwrap();
    assert_eq!(unlocked.into_document().unwrap(), opened);
}

/// An encrypted file whose page tree's root has its row at another
/// object: as PDFium reads it, by its password, scanned after its sections
/// for want of a page; by a wrong one, which PDFium wouldn't open, read by
/// its sections; and unlocked later by the password, in memory and by
/// position, read as the file opened by it is, scanned so.
#[test]
fn a_trailer_that_gives_no_page_once_a_password_opens_the_file_has_it_scanned_alike() {
    use crate::write::tests::methods;

    let method = (methods().into_iter())
        .find(|m| m.name == "AES-256 created")
        .unwrap();
    // Its objects, every one: as PDFium's reading reads the case.
    let plain = case!("pages-elsewhere");
    let reading = Reading::open(plain, &under(Recovery::Pdfium, &[])).unwrap();
    let doc = reading.to_document().unwrap();
    let bodies = write::bodies(&doc);
    let mut file = Vec::new();
    let options = write::Options::DEFAULT;
    write::full(
        &mut file,
        bodies,
        doc.trailer.dict(),
        Some(&method.encryptor),
        None,
        &options,
    )
    .unwrap();
    // Object 2's row given object 3's place.
    let start = last(&file, b"xref\n0 ") + b"xref\n0 ".len();
    let rows = start + file[start..].iter().position(|&b| b == b'\n').unwrap() + 1;
    let three = file[rows + 60..rows + 70].to_vec();
    file[rows + 40..rows + 50].copy_from_slice(&three);
    let user = [Password("user")];
    let opened = read(&file, &under(Recovery::Pdfium, &user));
    assert_eq!(opened.trailer.unwritten, Some(Unwritten::NoPages));
    assert_eq!(kids(&opened, 2), [Some(4), Some(5)]);
    let wrong = [Password("wrong")];
    let locked = document(&file, &under(Recovery::Pdfium, &wrong));
    assert_eq!(locked.trailer.unwritten, None);
    for first in [None, Some(7)] {
        let unlocked = match first {
            None => Reading::open(&file[..], &under(Recovery::Pdfium, &wrong))
                .unwrap()
                .unlock(&user[0].encodings())
                .ok()
                .unwrap()
                .into_document(),
            Some(first) => {
                let source = Counted::new(&file, first);
                let options = under(Recovery::Pdfium, &wrong);
                (open(&source, &wrong, &options, true).unwrap())
                    .unlock(&user[0].encodings())
                    .ok()
                    .unwrap()
                    .into_document()
            }
        };
        assert_eq!(unlocked.unwrap(), opened, "{first:?}");
    }
}

/// A reading scanned as PDFium scans a file has no base to update it
/// after, as any scanned reading hasn't; the verified one of the same file,
/// read by its sections, has.
#[test]
fn a_reading_pdfium_would_scan_has_no_base() {
    let file = case!("catalog-elsewhere");
    let reading = Reading::open(file, &under(Recovery::Pdfium, &[])).unwrap();
    assert!(matches!(Base::of(&reading), Err(write::Error::Unsound)));
    let reading = Reading::open(file, &under(Recovery::Verified, &[])).unwrap();
    assert!(Base::of(&reading).is_ok());
}

/// Every test file that PDFium's reading doesn't scan reads as the verified
/// recovery reads it, by its passwords, faults and all; and a file opened
/// as of a section PDFium's reading would scan isn't scanned for it.
#[test]
fn a_file_whose_sections_read_and_give_a_page_reads_as_verified() {
    let mut read_so = 0;
    for (name, file, passwords) in crate::read::lazy::files() {
        let ours = document(&file, &under(Recovery::Pdfium, &passwords));
        let verified = document(&file, &under(Recovery::Verified, &passwords));
        if ours.trailer.unwritten.is_some() || verified.facts.recovered {
            continue;
        }
        assert_eq!(ours, verified, "{name}");
        read_so += 1;
    }
    assert!(read_so > 10, "{read_so}");
    let file = case!("update-prev-past-the-end");
    let mut options = under(Recovery::Pdfium, &[]);
    options.section = Some(table(file));
    let doc = document(file, &options);
    assert_eq!(doc.trailer.unwritten, None);
    assert!(doc.get(ObjectId::new(1, 0)).is_err());
}
