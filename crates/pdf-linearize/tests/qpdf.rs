//! What's written from hand-written bodies, as qpdf reads it: `qpdf
//! --check-linearization`'s whole answer, exit and text, written out per file.
//! ISO 32000-2's Annex F and qpdf's check differ on some things, as qpdf's
//! comments say, so "qpdf is happy" isn't the test: each answer is the one that
//! follows from what the file does, read against what qpdf computes.

use std::path::Path;
use std::process::Command;

mod cases;
use cases::Doc;
#[cfg(feature = "family")]
mod written;

/// What qpdf says of `bytes` by `check`: its exit code and all it wrote, the file's
/// path as `FILE`.
fn asked(name: &str, bytes: &[u8], check: &str) -> (i32, String) {
    asked_by(name, bytes, check, "")
}

/// [`asked`], the file opened by `password`.
fn asked_by(name: &str, bytes: &[u8], check: &str, password: &str) -> (i32, String) {
    let qpdf = std::env::var_os("PDF_LINT_QPDF").unwrap_or_else(|| "qpdf".into());
    let folder = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(folder).unwrap();
    let path = folder.join(format!("linearize-{name}.pdf"));
    std::fs::write(&path, bytes).unwrap();
    let output = Command::new(qpdf)
        .arg(format!("--password={password}"))
        .arg(check)
        .arg(&path)
        .output()
        .expect("these tests need qpdf: install it, or name it in PDF_LINT_QPDF");
    let text = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    std::fs::remove_file(&path).unwrap();
    let text = text.replace(path.to_str().unwrap(), "FILE");
    (output.status.code().expect("it exited"), text)
}

/// What qpdf's `--check-linearization` says of `doc`'s file.
fn answer(name: &str, doc: &Doc) -> (i32, String) {
    asked(name, &doc.written().bytes, "--check-linearization")
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
        ];
        let warnings = text.lines().filter(|line| line.starts_with("WARNING"));
        for line in warnings {
            let line = line.strip_prefix("WARNING: FILE: ").unwrap();
            let shared_list = line.starts_with("page ") && line.contains(": shared object ");
            assert!(
                shared_list || expected.iter().any(|start| line.starts_with(start)),
                "{name}: {line}"
            );
        }
        assert!(text.contains("first page object (/O) mismatch"), "{name}");
    }
}

/// What the family adapter writes, as qpdf reads it, each warning its class, its
/// numbers taken out, as they move with any body: each kind of a file, and what
/// follows from where F.3.5 and qpdf's categories part, as above.
#[cfg(feature = "family")]
mod family {
    use std::collections::BTreeSet;

    use chacha20::ChaCha20Rng;
    use pdf_linearize::family::{Options, linearize, linearize_bytes};
    use pdf_structure::XrefKind;
    use pdf_structure::pdf_crypt::Password;
    use pdf_structure::pdf_crypt::Protection;
    use pdf_structure::pdf_crypt::rand_core::SeedableRng;
    use pdf_structure::write::Encryptor;

    use super::written::{KEYS, built, encrypted_as, kinds, opened, stream, two_pages};
    use super::{HINT_LAST, asked_by};

    /// `text`'s warnings, each once, its numbers `N`.
    pub(super) fn classes(text: &str) -> BTreeSet<String> {
        let warnings = text
            .lines()
            .filter_map(|line| line.strip_prefix("WARNING: FILE: "));
        let class = |line: &str| {
            let mut out = String::new();
            for c in line.chars() {
                match c.is_ascii_digit() {
                    true if out.ends_with('N') => {}
                    true => out.push('N'),
                    false => out.push(c),
                }
            }
            out
        };
        warnings.map(class).collect()
    }

    pub(super) const OBJECT_COUNT: &str =
        "object count mismatch for page N: hint table = N; computed = N";
    const FIRST_END: &str = "end of first page section (/E) mismatch: /E = N; computed = N..N";

    /// Both of qpdf's checks on each of `kinds`' file of `objects`, each kind's
    /// classes as `table`, `stream` or `packed` says, and no other.
    fn holds(name: &str, bytes: &[u8], table: &[&str], packed: &[&str]) {
        let out = |options: &Options<'_>| linearize_bytes(bytes, &[], options).unwrap();
        holds_as(name, out, "", table, packed);
    }

    /// [`holds`], of what `written` writes in each kind, opened by `password`.
    fn holds_as(
        name: &str,
        written: impl Fn(&Options<'_>) -> Vec<u8>,
        password: &str,
        table: &[&str],
        packed: &[&str],
    ) {
        for (at, options) in kinds().iter().enumerate() {
            let out = written(options);
            let name = format!("{name}-{at}");
            let wanted: BTreeSet<String> = (if options.pack { packed } else { table })
                .iter()
                .map(|class| class.to_string())
                .collect();
            for check in ["--check-linearization", "--check"] {
                let (code, text) = asked_by(&name, &out, check, password);
                assert_eq!(classes(&text), wanted, "{name} {check}: {text}");
                assert_eq!(
                    code,
                    if wanted.is_empty() { 0 } else { 3 },
                    "{name} {check}: {text}"
                );
                assert!(!text.contains("ERROR"), "{name} {check}: {text}");
            }
        }
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn plain_files_are_clean_in_each_kind() {
        holds("plain", &built(&two_pages("")), &[], &[HINT_LAST]);
        // Its second page's own and what it shares, its outline, its thumbnail.
        let mut objects = two_pages("/Outlines 9 0 R /Dests 13 0 R /Names << /Dests 14 0 R >>");
        objects[2] = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Thumb 12 0 R >>".into();
        objects[3] =
            "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Rotate 90 /Own 15 0 R >>".into();
        objects.push("<< /Type /Outlines /First 10 0 R /Last 11 0 R /Count 2 >>".into());
        objects.push("<< /Title (a) /Parent 9 0 R /Next 11 0 R /Dest [3 0 R /Fit] >>".into());
        objects.push("<< /Title (b) /Parent 9 0 R /Prev 10 0 R /Dest [4 0 R /Fit] >>".into());
        objects.push(stream(
            "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
            "z",
        ));
        objects.push("<< /a [3 0 R /Fit] >>".into());
        objects.push("<< /Names [(a) [4 0 R /Fit]] >>".into());
        objects.push("<< /Type /XObject /Mine true >>".into());
        holds("outline", &built(&objects), &[], &[HINT_LAST]);
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn encrypted_files_are_as_clean_under_each_handler_and_a_new_key() {
        let plain = built(&two_pages(""));
        for (name, key) in KEYS {
            let name = name.replace(' ', "-");
            holds(&name, &encrypted_as(key, &plain), &[], &[HINT_LAST]);
        }
        // A new encryption, by its password: over a plain reading, and an RC4 one.
        let prepared = Password("user").prepared().unwrap();
        let new = Encryptor::create_with(
            &Protection::new(&prepared),
            &mut ChaCha20Rng::seed_from_u64(15),
        )
        .unwrap();
        let sources = [plain.clone(), encrypted_as(KEYS[1].1, &plain)];
        for (at, source) in sources.iter().enumerate() {
            let written = |options: &Options<'_>| {
                let reading = opened(source, "");
                let doc = reading.to_document().unwrap();
                let mut out = Vec::new();
                linearize(&mut out, &doc, Some(&new), Some(&reading), options).unwrap();
                out
            };
            holds_as(&format!("new-{at}"), written, "user", &[], &[HINT_LAST]);
        }
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn a_form_warns_of_what_qpdf_puts_in_part_four() {
        // A widget on each page, which qpdf's categories have with the form's
        // dictionary, where F.3.5 has them on their pages.
        let mut objects = two_pages("/AcroForm 9 0 R");
        objects[2] = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Annots [10 0 R] >>".into();
        objects[3] = "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Annots [11 0 R] >>".into();
        objects.push("<< /Fields [10 0 R 11 0 R] >>".into());
        objects.push("<< /FT /Tx /Subtype /Widget /T (a) /Rect [0 0 9 9] >>".into());
        objects.push("<< /FT /Tx /Subtype /Widget /T (b) /Rect [0 0 9 9] >>".into());
        let form = [OBJECT_COUNT];
        // The object stream of what's shared, which qpdf has none of.
        let shared = "page N: shared object N: in hint table but not computed list";
        let packed = [OBJECT_COUNT, HINT_LAST, shared];
        holds("form", &built(&objects), &form, &packed);
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn another_first_page_warns_as_the_engine_s_does() {
        let bytes = built(&two_pages("/OpenAction [4 0 R /Fit]"));
        let first = [
            "first page object (/O) mismatch",
            FIRST_END,
            "first page object offset mismatch",
            "shared object N length mismatch: hint table = N; computed = N",
            OBJECT_COUNT,
            "page N has shared identifier entries",
            "page N: shared object N: in hint table but not computed list",
            "page N: shared object N: in computed list but not hint table",
        ];
        let mut packed = first.to_vec();
        packed.push(HINT_LAST);
        holds("first", &bytes, &first, &packed);
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn the_same_file_again_is_as_clean() {
        let once = {
            let mut options = Options::DEFAULT;
            options.kind = XrefKind::Stream;
            options.pack = true;
            linearize_bytes(&built(&two_pages("")), &[], &options).unwrap()
        };
        holds("again", &once, &[], &[HINT_LAST]);
    }
}

/// What the lopdf adapter writes, as qpdf reads it: a table, whatever the file was, each
/// warning the class the family's table has of the same file, and for the same reason.
#[cfg(all(feature = "family", feature = "lopdf"))]
mod lopdf {
    use std::collections::BTreeSet;

    use lopdf::Document;
    use pdf_linearize::lopdf::linearize;

    use super::asked_by;
    use super::family::{OBJECT_COUNT, classes};
    use super::written::{built, stream, two_pages};

    /// Both of qpdf's checks on what's written of `bytes`: each warns of `wanted`'s
    /// classes alone.
    fn holds(name: &str, bytes: &[u8], wanted: &[&str]) {
        let doc = Document::load_mem(bytes).unwrap();
        let mut out = Vec::new();
        linearize(&mut out, &doc, &pdf_linearize::lopdf::Options::DEFAULT).unwrap();
        let wanted: BTreeSet<String> = wanted.iter().map(|class| class.to_string()).collect();
        for check in ["--check-linearization", "--check"] {
            let (code, text) = asked_by(name, &out, check, "");
            assert_eq!(classes(&text), wanted, "{name} {check}: {text}");
            let clean = if wanted.is_empty() { 0 } else { 3 };
            assert_eq!(code, clean, "{name} {check}: {text}");
            assert!(!text.contains("ERROR"), "{name} {check}: {text}");
        }
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn plain_files_are_clean() {
        holds("lopdf-plain", &built(&two_pages("")), &[]);
        let mut objects = two_pages("/Outlines 9 0 R /Dests 13 0 R /Names << /Dests 14 0 R >>");
        objects[2] = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Thumb 12 0 R >>".into();
        objects[3] =
            "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Rotate 90 /Own 15 0 R >>".into();
        objects.push("<< /Type /Outlines /First 10 0 R /Last 11 0 R /Count 2 >>".into());
        objects.push("<< /Title (a) /Parent 9 0 R /Next 11 0 R /Dest [3 0 R /Fit] >>".into());
        objects.push("<< /Title (b) /Parent 9 0 R /Prev 10 0 R /Dest [4 0 R /Fit] >>".into());
        objects.push(stream(
            "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
            "z",
        ));
        objects.push("<< /a [3 0 R /Fit] >>".into());
        objects.push("<< /Names [(a) [4 0 R /Fit]] >>".into());
        objects.push("<< /Type /XObject /Mine true >>".into());
        holds("lopdf-outline", &built(&objects), &[]);
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn a_form_warns_of_what_qpdf_puts_in_part_four() {
        let mut objects = two_pages("/AcroForm 9 0 R");
        objects[2] = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Annots [10 0 R] >>".into();
        objects[3] = "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Annots [11 0 R] >>".into();
        objects.push("<< /Fields [10 0 R 11 0 R] >>".into());
        objects.push("<< /FT /Tx /Subtype /Widget /T (a) /Rect [0 0 9 9] >>".into());
        objects.push("<< /FT /Tx /Subtype /Widget /T (b) /Rect [0 0 9 9] >>".into());
        holds("lopdf-form", &built(&objects), &[OBJECT_COUNT]);
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn another_first_page_warns_as_the_family_s_does() {
        let first = [
            "first page object (/O) mismatch",
            "end of first page section (/E) mismatch: /E = N; computed = N..N",
            "first page object offset mismatch",
            "shared object N length mismatch: hint table = N; computed = N",
            OBJECT_COUNT,
            "page N has shared identifier entries",
            "page N: shared object N: in hint table but not computed list",
            "page N: shared object N: in computed list but not hint table",
        ];
        holds(
            "lopdf-first",
            &built(&two_pages("/OpenAction [4 0 R /Fit]")),
            &first,
        );
    }
}
