//! What's written from hand-written bodies, as qpdf reads it: `qpdf
//! --check-linearization`'s whole answer, exit and text, written out per file.
//! ISO 32000-2's Annex F and qpdf's check differ on some things, as qpdf's
//! comments say, so "qpdf is happy" isn't the test: each answer is the one that
//! follows from what the file does, read against what qpdf computes.

use std::path::Path;
use std::process::Command;

mod cases;
use cases::Doc;

/// What qpdf says of `doc`'s file: its exit code and all it wrote, its path as
/// `FILE`.
fn answer(name: &str, doc: &Doc) -> (i32, String) {
    let qpdf = std::env::var_os("PDF_LINT_QPDF").unwrap_or_else(|| "qpdf".into());
    let folder = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(folder).unwrap();
    let path = folder.join(format!("linearize-{name}.pdf"));
    std::fs::write(&path, doc.written().bytes).unwrap();
    let output = Command::new(qpdf)
        .arg("--check-linearization")
        .arg(&path)
        .output()
        .expect("these tests need qpdf: install it, or name it in PDF_LINT_QPDF");
    let text = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    std::fs::remove_file(&path).unwrap();
    let text = text.replace(path.to_str().unwrap(), "FILE");
    (output.status.code().expect("it exited"), text)
}

/// F.3.6 has the hint stream's number after every other in the first group, those
/// of what's in its object streams too, so where the group has such members it
/// follows them, which qpdf's check flags, on any file written to that rule.
const HINT_LAST: &str = "linearized file contains an uncompressed object after a compressed one in a cross-reference stream";

/// What qpdf says of each case. What's not plain has a reason:
///
/// - A packed file with members in its first group, [`HINT_LAST`].
/// - A form. F.3.5 has only the form's dictionary in part 4, with its fields where
///   they fall, so the widgets are their pages'. qpdf's categories have all an
///   `/AcroForm` reaches open-document, in part 4, so it computes those pages as
///   having a widget fewer, and part 6 as ending before it.
/// - `/P` other than 0. F.3.7 has the page an `/OpenAction` names first, and qpdf
///   takes page 0 to be first whatever `/P` says, so the object it expects first
///   isn't, and what follows from that: where part 6 ends, its first entry's length,
///   its first object's place. Entries are by page number, so a page's length in the
///   table is what it computes.
/// - An outline under packing. qpdf's check fails its own file there; this one names
///   the object stream that holds the root, and it computes the same, so no warning.
///   A file is clean of that but for its members in part 6 ([`HINT_LAST`]).
///
/// Nothing else: a warning that isn't here is a fault in the file or a change in
/// qpdf, to be read.
const ANSWERS: &[(&str, &[&str])] = &[
    ("two-table", &[]),
    ("two-stream", &[]),
    ("two-hex", &[]),
    ("two-packed", &[]),
    ("shared-table", &[]),
    ("shared-stream", &[]),
    ("shared-packed", &[HINT_LAST]),
    ("shared-packed-hex", &[HINT_LAST]),
    (
        "first-table",
        &[
            "first page object (/O) mismatch",
            "end of first page section (/E) mismatch: /E = 865; computed = 1035..1036",
            "shared object 0 length mismatch: hint table = 259; computed = 171",
            "first page object offset mismatch",
        ],
    ),
    (
        "first-stream",
        &[
            "first page object (/O) mismatch",
            "end of first page section (/E) mismatch: /E = 915; computed = 1085..1086",
            "shared object 0 length mismatch: hint table = 262; computed = 171",
            "first page object offset mismatch",
        ],
    ),
    (
        "first-packed",
        &[
            "first page object (/O) mismatch",
            "end of first page section (/E) mismatch: /E = 918; computed = 1088..1089",
            "shared object 0 length mismatch: hint table = 262; computed = 171",
            "first page object offset mismatch",
        ],
    ),
    (
        "form-table",
        &[
            "end of first page section (/E) mismatch: /E = 965; computed = 887..888",
            "object count mismatch for page 0: hint table = 3; computed = 2",
            "object count mismatch for page 1: hint table = 3; computed = 2",
        ],
    ),
    (
        "form-stream",
        &[
            "end of first page section (/E) mismatch: /E = 992; computed = 913..914",
            "object count mismatch for page 0: hint table = 3; computed = 2",
            "object count mismatch for page 1: hint table = 3; computed = 2",
        ],
    ),
    (
        "form-packed",
        &[
            HINT_LAST,
            "end of first page section (/E) mismatch: /E = 1153; computed = 1005..1006",
            "object count mismatch for page 0: hint table = 3; computed = 2",
            "object count mismatch for page 1: hint table = 3; computed = 2",
        ],
    ),
    ("outline-table", &[]),
    ("outline-first", &[]),
    ("outline-stream", &[]),
    ("outline-packed", &[]),
    ("outline-first-packed", &[HINT_LAST]),
    ("thumbs-table", &[]),
    ("thumbs-stream", &[]),
    ("thumbs-packed", &[]),
    ("all-table", &[]),
    ("all-stream", &[]),
    ("all-packed", &[HINT_LAST]),
    ("uniform-table", &[]),
    ("uniform-stream", &[]),
    ("long-table", &[]),
    ("long-stream", &[]),
    ("long-packed", &[HINT_LAST]),
];

/// `(code, text)` as qpdf gives it, for `warnings`.
fn expected(warnings: &[&str]) -> (i32, String) {
    if warnings.is_empty() {
        return (0, "FILE: no linearization errors\n".to_string());
    }
    let mut text = String::new();
    for warning in warnings {
        text.push_str(&format!("WARNING: FILE: {warning}\n"));
    }
    text.push_str("qpdf: operation succeeded with warnings\n");
    (3, text)
}

#[test]
#[ignore = "needs qpdf: just test-qpdf"]
fn each_files_answer_is_what_it_should_be() {
    for (name, doc) in cases::all() {
        if name.starts_with("long-first") {
            continue;
        }
        let at = ANSWERS.iter().find(|(case, _)| *case == name);
        let (_, warnings) = at.unwrap_or_else(|| panic!("{name} has no answer written"));
        assert_eq!(answer(name, &doc), expected(warnings), "{name}");
    }
    assert_eq!(ANSWERS.len(), cases::all().len() - 2);
}

/// Many pages under another first page: what follows from qpdf taking page 0 as the
/// first runs on through the pages, each a shape of the short file's, with what
/// that first page had shared and nothing else.
#[test]
#[ignore = "needs qpdf: just test-qpdf"]
fn many_pages_under_another_first_page_warn_only_of_that() {
    for (name, doc) in cases::all() {
        if !name.starts_with("long-first") {
            continue;
        }
        let (code, text) = answer(name, &doc);
        assert_eq!(code, 3, "{name}");
        let expected = [
            "first page object (/O) mismatch",
            HINT_LAST,
            "end of first page section (/E) mismatch:",
            "shared object ",
            "first shared object number mismatch:",
            "first page object offset mismatch",
            "object count mismatch for page ",
            "page 0 has shared identifier entries",
            "page ",
        ];
        let warnings = text.lines().filter(|line| line.starts_with("WARNING"));
        for line in warnings {
            let line = line.strip_prefix("WARNING: FILE: ").unwrap();
            assert!(
                expected.iter().any(|start| line.starts_with(start)),
                "{name}: {line}"
            );
        }
        assert!(text.contains("first page object (/O) mismatch"), "{name}");
    }
}
