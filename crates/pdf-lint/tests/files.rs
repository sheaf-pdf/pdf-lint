//! Files another writer made, qpdf, read and checked by pdf-lint alone:
//! each as committed in `tests/files`, written once from a clean file
//! (qpdf.rs's `the_fixtures_are_written`).

use std::path::{Path, PathBuf};
use std::process::Command;

use pdf_lint::bytes::read_opened;
use pdf_lint::{Document, Encryption, Hold, Made, Object, Options, Str, Value, check};

/// Each fixture's name, whether its dictionaries are in object streams,
/// and whether it's encrypted, with the empty user password and `owner`
/// the owner's.
const FIXTURES: [(&str, bool, bool); 8] = [
    ("qpdf-plain", false, false),
    ("qpdf-compressed", true, false),
    ("qpdf-rc4-40", true, true),
    ("qpdf-rc4-128", true, true),
    ("qpdf-aes-128", true, true),
    ("qpdf-aes-256-r5", true, true),
    ("qpdf-aes-256", true, true),
    ("qpdf-aes-256-unpacked", false, true),
];

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/files")
        .join(format!("{name}.pdf"))
}

/// The text annotation, which every fixture has, with its `/Contents`.
fn annotation(doc: &Document) -> &Object {
    (doc.objects.values().flatten())
        .find(|o| {
            o.as_dict()
                .is_some_and(|d| d.get(b"Subtype").and_then(Object::as_name) == Some(b"Text"))
        })
        .expect("the annotation reads")
}

#[test]
fn each_file_qpdf_wrote_reads_whole_by_the_reader_alone() {
    for (name, packed, encrypted) in FIXTURES {
        let data = std::fs::read(fixture(name)).unwrap();
        let doc = read_opened(&data, &[]);
        let opened = match encrypted {
            true => Encryption::Opened { owner: false },
            false => Encryption::None,
        };
        assert_eq!(doc.encryption, opened, "{name}");
        assert_eq!(doc.trailer.unwritten, None, "{name}");
        assert!(doc.warnings.is_empty(), "{name}: {:?}", doc.warnings);
        assert!(doc.objects.values().all(Result::is_ok), "{name}");
        let annotation = annotation(&doc);
        assert_eq!(
            annotation.at.unwrap().stream.is_some(),
            packed,
            "{name}: in an object stream or not"
        );
        let contents = annotation.as_dict().unwrap().get(b"Contents").unwrap();
        assert_eq!(
            contents.value,
            Value::String(Str::Bytes(b"A note".to_vec())),
            "{name}"
        );
        // Clean, but for what the model finds of the encryption itself
        // (qpdf writes revision 6 in a PDF 1.7 file).
        let report = check(&doc, &Hold::All(Made::Merged), &Options::default());
        let others: Vec<_> = (report.findings.iter())
            .filter(|f| !f.table.starts_with("Encryption") && !f.table.starts_with("CryptFilter"))
            .collect();
        assert!(others.is_empty(), "{name}: {report}");
        // The owner's password opens it too, to the same.
        if encrypted {
            let owned = read_opened(&data, &[b"owner"]);
            assert_eq!(
                owned.encryption,
                Encryption::Opened { owner: true },
                "{name}"
            );
            assert_eq!(owned.objects, doc.objects, "{name}");
        }
    }
}

#[test]
fn each_file_qpdf_wrote_checks_through_the_command_line() {
    for (name, _, encrypted) in FIXTURES {
        let path = fixture(name);
        let out = Command::new(env!("CARGO_BIN_EXE_pdf-lint"))
            .arg(&path)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        // Encrypted, a finding or more on the encryption dictionary, by
        // the version qpdf writes; else clean.
        let code = if encrypted { [0, 1] } else { [0, 0] };
        assert!(
            out.status.code().is_some_and(|c| code.contains(&c)),
            "{name}: {stdout}"
        );
        assert!(!stdout.contains("unresolved"), "{name}: {stdout}");
    }
}
