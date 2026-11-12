//! What the lopdf adapter writes, as qpdf reads it: both of qpdf's checks,
//! `--check-linearization` and `--check`, their exit and text, each warning by
//! its class, its numbers taken out, as pdf-linearize's own tests hold what the
//! family adapter writes, and of the same files.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

/// The files pdf-linearize's tests write by hand, as one source.
#[path = "../../pdf-linearize/tests/written/mod.rs"]
mod written;

/// What qpdf says of `bytes` by `check`, the file opened by `password`: its exit
/// code and all it wrote, the file's path as `FILE`.
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

/// `text`'s warnings, each once, its numbers `N`.
fn classes(text: &str) -> BTreeSet<String> {
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

/// A class qpdf warns of where a page's count of objects in the hint table
/// isn't what it computes.
const OBJECT_COUNT: &str = "object count mismatch for page N: hint table = N; computed = N";

/// What the lopdf adapter writes, as qpdf reads it: a table, whatever the file was, each
/// warning the class the family's table has of the same file, and for the same reason.
mod lopdf {
    use std::collections::BTreeSet;

    use lopdf::Document;
    use pdf_linearize_lopdf::linearize;

    use super::written::{built, stream, two_pages};
    use super::{OBJECT_COUNT, asked_by, classes};

    /// Both of qpdf's checks on what's written of `bytes`: each warns of `wanted`'s
    /// classes alone.
    fn holds(name: &str, bytes: &[u8], wanted: &[&str]) {
        let doc = Document::load_mem(bytes).unwrap();
        let mut out = Vec::new();
        linearize(&mut out, &doc, &pdf_linearize_lopdf::Options::DEFAULT).unwrap();
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
