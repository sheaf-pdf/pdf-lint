//! Each fault said once: a fault on an object that a read on the way to
//! another tells too (the open, of the encryption dictionary and what its
//! entries refer to, of a cross-reference stream that lists itself, and of
//! the objects the scan reads; a stream's read, of its indirect `/Length`)
//! is said where the object's own read says it, in its place in the whole
//! read's order; and by a reading that hasn't read the object, once, where
//! it was told. What the open reads of a stream whose `/Length` is a
//! reference it can't follow yet, its own read reads alike.

use pdf_fixtures::fixture;
use pdf_objects::dict;

use super::lengths::{
    a_cross_reference_streams_length_by_reference,
    a_cross_reference_streams_length_in_an_object_stream, alike,
    an_encryption_dictionarys_stream_with_its_length_in_an_object_stream, data, no_length, scanned,
    unfollowed,
};
use super::tests::{KINDS, encryption_updated, root, settings, to_encrypt};
use super::*;

/// `file` with object `n`'s `endobj` (its last, where the file has it more
/// than once, as an update's) written as spaces, so that it has none and
/// every offset is as it was.
fn unended(file: &[u8], n: u32) -> Vec<u8> {
    let head = format!("{n} 0 obj");
    let mut at = None;
    let mut from = 0;
    while let Some(found) = find(&file[from..], head.as_bytes()) {
        let start = from + found;
        if start == 0 || file[start - 1] == b'\n' {
            at = Some(start);
        }
        from = start + 1;
    }
    let start = at.expect("the object is written");
    let end = start + find(&file[start..], b"endobj").expect("it has an endobj");
    let mut file = file.to_vec();
    file[end..end + 6].copy_from_slice(b"      ");
    file
}

/// The fault object `n` without its `endobj` says.
fn unended_fault(n: u32) -> Fault {
    Fault::Parse {
        kind: ParseKind::NoEndobj,
        text: format!("{n} 0 obj has no endobj").into(),
    }
}

/// The objects `faults` are on, each with its words, in order.
fn said(faults: &[(Place, Fault)]) -> Vec<(Option<u32>, String)> {
    (faults.iter())
        .map(|(place, fault)| (place.object.map(|id| id.number), fault.to_string()))
        .collect()
}

/// What a file of broken objects says, read whole and on demand: `first`
/// and `last` each without its `endobj`, `first` numbered below `last`,
/// and `last` read on the way to another, once or more, as the file is
/// opened (`by` none) or as object `by`, numbered below `first`, is read.
/// Read whole, each is said once, at its own read's place: `first`'s, then
/// `last`'s. On demand, `last` is said once where the first read on the
/// way told it, until it's read itself: so with `first` read, before it;
/// with both read, as the whole read says them. Each fault said is the one
/// its object's own read tells.
fn said_once_at_its_own_place(file: &[u8], first: u32, last: u32, by: Option<u32>, what: &str) {
    let fault = |n: u32| (Some(n), unended_fault(n).to_string());
    let doc = document(file, &Options::DEFAULT);
    assert_eq!(said(doc.faults()), [fault(first), fault(last)], "{what}");
    let reading = Reading::open(file.to_vec(), &Options::DEFAULT).unwrap();
    if let Some(by) = by {
        assert!(
            reading.faults().is_empty(),
            "{what}: {:?}",
            reading.faults()
        );
        reading.get(ObjectId::new(by, 0)).unwrap();
    }
    assert_eq!(said(&reading.faults()), [fault(last)], "{what}: on the way");
    let told = reading.faults()[0].clone();
    reading.get(ObjectId::new(first, 0)).unwrap();
    assert_eq!(
        said(&reading.faults()),
        [fault(last), fault(first)],
        "{what}: {first} read"
    );
    reading.get(ObjectId::new(last, 0)).unwrap();
    assert_eq!(
        said(&reading.faults()),
        [fault(first), fault(last)],
        "{what}: both read"
    );
    assert_eq!(reading.faults(), doc.faults(), "{what}: read on demand");
    // The one said is its own read's, which the read on the way told too.
    let alone = Reading::open(file.to_vec(), &Options::DEFAULT).unwrap();
    alone.get(ObjectId::new(last, 0)).unwrap();
    assert_eq!(
        alone.faults(),
        std::slice::from_ref(&told),
        "{what}: {last} read alone"
    );
    assert_eq!(doc.faults()[1], told, "{what}");
}

/// The encryption dictionary itself without its `endobj`: the open reads it
/// to open the file, and the whole read reads it again as an object.
#[test]
fn an_encryption_dictionarys_fault_is_said_once_at_its_own_place() {
    for kind in KINDS {
        let file = fixture::encrypted(&settings(kind, b""), &to_encrypt(&[]), root());
        let file = unended(&unended(&file, 5), 6);
        let doc = document(&file, &Options::DEFAULT);
        assert!(
            matches!(doc.encryption(), Encryption::Opened { .. }),
            "{kind:?}"
        );
        said_once_at_its_own_place(&file, 5, 6, None, &format!("{kind:?}"));
    }
}

/// `/StmF` by reference to an object without its `endobj`, which the open
/// reads for the dictionary's value.
#[test]
fn a_fault_in_what_stmf_refers_to_is_said_once_at_its_own_place() {
    let mut tried = 0;
    for kind in KINDS {
        let file = fixture::encrypted(&settings(kind, b""), &to_encrypt(&[]), root());
        let own = &file[find(&file, b"6 0 obj").unwrap()..];
        let own = &own[..find(own, b"endobj").unwrap()];
        if find(own, b"/StmF /StdCF").is_none() {
            continue;
        }
        let file = encryption_updated(&file, "/StmF /StdCF", "/StmF 9 0 R", &[(9, "/StdCF")]);
        let file = unended(&unended(&file, 5), 9);
        let doc = document(&file, &Options::DEFAULT);
        assert!(
            matches!(doc.encryption(), Encryption::Opened { .. }),
            "{kind:?}"
        );
        said_once_at_its_own_place(&file, 5, 9, None, &format!("{kind:?}"));
        tried += 1;
    }
    // The three kinds from /V 4, which have crypt filters.
    assert_eq!(tried, 3);
}

/// `/EFF` by reference to an object without its `endobj`: from `/V` 4,
/// where the open reads it for the dictionary's value, said once at its
/// own place; below, where it isn't read, said once all the same.
#[test]
fn a_fault_in_what_eff_refers_to_is_said_once_at_its_own_place() {
    for kind in KINDS {
        let file = fixture::encrypted(&settings(kind, b""), &to_encrypt(&[]), root());
        let file = encryption_updated(&file, "/P -4", "/EFF 9 0 R /P -4", &[(9, "/StdCF")]);
        let file = unended(&unended(&file, 5), 9);
        let v = pdf_crypt::writer::dictionary(&settings(kind, b"")).v;
        let what = format!("{kind:?}");
        match v >= 4 {
            true => said_once_at_its_own_place(&file, 5, 9, None, &what),
            false => {
                let fault = |n: u32| (Some(n), unended_fault(n).to_string());
                let doc = document(&file, &Options::DEFAULT);
                assert_eq!(said(doc.faults()), [fault(5), fault(9)], "{what}");
                let reading = Reading::open(file.clone(), &Options::DEFAULT).unwrap();
                assert!(
                    reading.faults().is_empty(),
                    "{what}: {:?}",
                    reading.faults()
                );
            }
        }
    }
}

/// `/StmF` and `/EFF` both by reference to one object without its
/// `endobj`, from `/V` 4: the open reads it for each entry's value, so two
/// reads on the way tell its fault, and it's said once, the first time,
/// until its own read says it.
#[test]
fn a_fault_in_what_two_entries_refer_to_is_said_once_at_its_own_place() {
    let mut tried = 0;
    for kind in KINDS {
        let file = fixture::encrypted(&settings(kind, b""), &to_encrypt(&[]), root());
        let own = &file[find(&file, b"6 0 obj").unwrap()..];
        let own = &own[..find(own, b"endobj").unwrap()];
        if find(own, b"/StmF /StdCF").is_none() {
            continue;
        }
        let to = "/StmF 9 0 R /EFF 9 0 R";
        let file = encryption_updated(&file, "/StmF /StdCF", to, &[(9, "/StdCF")]);
        let file = unended(&unended(&file, 5), 9);
        let doc = document(&file, &Options::DEFAULT);
        assert!(
            matches!(doc.encryption(), Encryption::Opened { .. }),
            "{kind:?}"
        );
        said_once_at_its_own_place(&file, 5, 9, None, &format!("{kind:?}"));
        tried += 1;
    }
    // The three kinds from /V 4, which have crypt filters.
    assert_eq!(tried, 3);
}

/// The objects of a small file: a catalog, its page tree, and an object
/// of its own, 3.
fn objects() -> Vec<(u32, fixture::Body)> {
    vec![
        (
            1,
            dict! { "Type" => "Catalog", "Pages" => Object::reference(2) }.into(),
        ),
        (
            2,
            dict! { "Type" => "Pages", "Kids" => Vec::<Object>::new(), "Count" => 0 }.into(),
        ),
        (3, dict! { "A" => 1 }.into()),
    ]
}

/// A cross-reference stream that lists itself, without its `endobj`: the
/// open reads it for its rows, and the whole read reads it again as the
/// object its own row lists.
#[test]
fn a_fault_in_a_cross_reference_stream_that_lists_itself_is_said_once_at_its_own_place() {
    let trailer = dict! { "Root" => Object::reference(1) };
    let file = fixture::file("1.7", &objects(), trailer, fixture::Xref::Stream);
    let file = unended(&unended(&file, 2), 4);
    let reading = Reading::open(file.clone(), &Options::DEFAULT).unwrap();
    assert_eq!(reading.trailer().stream, Some(ObjectId::new(4, 0)));
    assert!(reading.ids().any(|id| id == ObjectId::new(4, 0)));
    said_once_at_its_own_place(&file, 2, 4, None, "the stream");
}

/// A scanned file whose last object is without its `endobj`: the scan
/// reads the objects from the file's end until a cross-reference stream,
/// and the whole read reads each again as an object.
#[test]
fn a_fault_in_an_object_the_scan_reads_is_said_once_at_its_own_place() {
    let trailer = dict! { "Root" => Object::reference(1) };
    let file = fixture::file("1.7", &objects(), trailer, fixture::Xref::Stream);
    // An object after the cross-reference stream, its last, and a
    // `startxref` to no section, so the file is scanned.
    let at = find(&file, b"startxref").unwrap();
    let file = [
        &file[..at],
        b"5 0 obj\n<< /B 2 >>\nendobj\nstartxref\n9\n%%EOF\n",
    ]
    .concat();
    let file = unended(&unended(&file, 2), 5);
    let reading = Reading::open(file.clone(), &Options::DEFAULT).unwrap();
    assert!(reading.trailer().unwritten.is_some());
    assert_eq!(reading.trailer().stream, Some(ObjectId::new(4, 0)));
    said_once_at_its_own_place(&file, 2, 5, None, "scanned");
}

/// A stream whose `/Length` is an object without its `endobj`: the
/// stream's read reads it for its length, and the whole read reads it again
/// as an object.
#[test]
fn a_fault_in_a_streams_indirect_length_is_said_once_at_its_own_place() {
    let file = fixture::assembled(
        "1.7",
        &[
            (1, b"<< /Type /Catalog /Pages 5 0 R >>"),
            (2, b"<< /Length 4 0 R >>\nstream\nabcd\nendstream"),
            (3, b"<< /A 1 >>"),
            (4, b"4"),
            (5, b"<< /Type /Pages /Kids [] /Count 0 >>"),
        ],
        "<< /Size 6 /Root 1 0 R >>",
    );
    let file = unended(&unended(&file, 3), 4);
    let reading = Reading::open(file.clone(), &Options::DEFAULT).unwrap();
    let stream = reading
        .get(ObjectId::new(2, 0))
        .unwrap()
        .as_stream()
        .unwrap();
    assert!(
        reading
            .record(ObjectId::new(2, 0))
            .unwrap()
            .data
            .unwrap()
            .declared
    );
    assert_eq!(stream.data.at().map(|at| at.end - at.start), Some(4));
    said_once_at_its_own_place(&file, 3, 4, Some(2), "/Length");
}

/// An encryption dictionary of many faults, each a name with an escape of
/// no hexadecimal digits, at a place of its own: the open reads it to open
/// the file, and the whole read reads it again as an object. Each is said
/// once, at its own read's place, and the one said is the one the open
/// told; each told on the way is looked up, not found by a scan of what
/// the object's own read told (`faults/twice`, the benchmarks', times it).
#[test]
fn an_object_of_many_faults_read_on_the_way_is_said_once_each() {
    const NAMES: usize = 5_000;
    let dict = format!(
        "<< /Filter /Unknown /V 1 /R 2{} >>",
        " /A#zz 1".repeat(NAMES)
    );
    let file = fixture::assembled(
        "1.7",
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (2, b"<< /Type /Pages /Kids [] /Count 0 >>"),
            (3, dict.as_bytes()),
        ],
        "<< /Size 4 /Root 1 0 R /Encrypt 3 0 R /ID [<00> <00>] >>",
    );
    let reading = Reading::open(file.clone(), &Options::DEFAULT).unwrap();
    assert!(matches!(reading.encryption(), Encryption::Unsupported(_)));
    // The open's own fault, the file's, then the dictionary's, each at its
    // own read's place.
    let on = Some(ObjectId::new(3, 0));
    let (told, open): (Vec<_>, Vec<_>) =
        (reading.faults().into_iter()).partition(|(place, _)| place.object == on);
    assert!(told.len() >= NAMES, "{}", told.len());
    let whole: Vec<_> = open.into_iter().chain(told).collect();
    let doc = document(&file, &Options::DEFAULT);
    assert_eq!(doc.faults(), whole, "read whole");
    reading.get(ObjectId::new(3, 0)).unwrap();
    assert_eq!(reading.faults(), whole, "read on demand");
}

/// What the open reads before it lists an object, or has the handler that
/// decrypts an object stream, a stream whose `/Length` is a reference
/// among it, is read alike by the object's own read: a cross-reference
/// stream's rows, its `/Length` in an object stream or at an offset, and
/// what an encryption dictionary refers to, its `/Length` in an object
/// stream. So the fault that its `/Length` doesn't read, or in a
/// cross-reference stream that it's a reference, which isn't followed, is
/// said once, where its own read says it: told as the file opens, and by
/// the object's read alike, its data found by `endstream` either way.
#[test]
fn what_the_open_reads_of_a_length_in_an_object_stream_its_own_read_reads_alike() {
    let cases = [
        (
            "a cross-reference stream",
            a_cross_reference_streams_length_in_an_object_stream(),
            7,
        ),
        (
            "a cross-reference stream, its /Length at an offset",
            a_cross_reference_streams_length_by_reference(),
            6,
        ),
        (
            "what an encryption dictionary refers to",
            an_encryption_dictionarys_stream_with_its_length_in_an_object_stream(),
            9,
        ),
    ];
    for (what, file, n) in cases {
        let doc = alike(&file, &[]);
        let id = ObjectId::new(n, 0);
        // A cross-reference stream's reference is followed by no read, and
        // its data holds no `endstream`; the other's data is [`CONTENT`].
        let (fault, expected) = match n {
            6 | 7 => (unfollowed(&doc, n), vec![unfollowed(&doc, n)]),
            _ => (no_length(&doc, n), scanned(&doc, n).to_vec()),
        };
        let told: Vec<_> = (doc.faults().iter())
            .filter(|(place, _)| place.object == Some(id))
            .cloned()
            .collect();
        assert_eq!(told, expected, "{what}");
        assert!(!data(&doc, &file, n).1, "{what}: by endstream");
        // The open tells it; the object's own read, read alone, reads as
        // the open did, and tells it too, so it's said once.
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        let opened = reading.faults();
        assert!(opened.contains(&fault), "{what}: {opened:?}");
        reading.get(id).unwrap();
        assert!(
            !reading.record(id).unwrap().data.unwrap().declared,
            "{what}"
        );
        let said = reading.faults().into_iter().filter(|told| *told == fault);
        assert_eq!(said.count(), 1, "{what}");
        assert_eq!(reading.into_document().unwrap(), doc, "{what}");
    }
}
