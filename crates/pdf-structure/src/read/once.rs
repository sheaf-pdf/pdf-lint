//! Each fault said once: a fault on an object that a read on the way to
//! another tells too, as the open does of the encryption dictionary and
//! what its entries refer to, is said where the object's own read says it,
//! in its place in the whole read's order; and by a reading that hasn't
//! read the object, once, where it was told.

use pdf_fixtures::fixture;

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
/// and `last` read on the way to another as the file is opened. Read whole,
/// each is said once, at its own read's place: `first`'s, then `last`'s.
/// Opened and nothing read, `last` is said once, as the open told it; with
/// `first` read, before it; with both read, as the whole read says them.
/// Each fault said is the one its object's own read tells.
fn said_once_at_its_own_place(file: &[u8], first: u32, last: u32, what: &str) {
    let fault = |n: u32| (Some(n), unended_fault(n).to_string());
    let doc = document(file, &Options::DEFAULT);
    assert_eq!(said(doc.faults()), [fault(first), fault(last)], "{what}");
    let reading = Reading::open(file.to_vec(), &Options::DEFAULT).unwrap();
    assert_eq!(said(&reading.faults()), [fault(last)], "{what}: opened");
    let opened = reading.faults()[0].clone();
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
    // The one said is its own read's, which is the open's too.
    let alone = Reading::open(file.to_vec(), &Options::DEFAULT).unwrap();
    alone.get(ObjectId::new(last, 0)).unwrap();
    assert_eq!(
        alone.faults(),
        std::slice::from_ref(&opened),
        "{what}: {last} read alone"
    );
    assert_eq!(doc.faults()[1], opened, "{what}");
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
        said_once_at_its_own_place(&file, 5, 6, &format!("{kind:?}"));
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
        said_once_at_its_own_place(&file, 5, 9, &format!("{kind:?}"));
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
            true => said_once_at_its_own_place(&file, 5, 9, &what),
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
