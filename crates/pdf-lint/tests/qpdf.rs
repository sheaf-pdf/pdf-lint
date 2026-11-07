//! Files read from their bytes and through qpdf alone, a second reader,
//! checked by pdf-lint: the findings alike but for where, and where they
//! aren't, why. qpdf is pdf-fixtures', as pdf-structure's tests run it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Instant;

use pdf_fixtures::qpdf::Qpdf;
use pdf_lint::bytes::{self, WithReading};
use pdf_lint::{
    Document, Finding, Hold, Linearized, Made, Options, Rule, check, check_with_reading,
};
use pdf_structure::write;

/// qpdf, which these tests need: `PDF_LINT_QPDF`, else `qpdf` on the
/// `PATH`. They're ignored, so that `cargo test` needs no qpdf, and run by
/// `just test-qpdf` against the pinned one; its absence then fails them,
/// rather than passing them unrun.
fn qpdf(file: &Path) -> Qpdf {
    let qpdf = Qpdf::new(file);
    if let Err(why) = qpdf.version() {
        panic!("these tests need qpdf ({why}): install it, or name it in PDF_LINT_QPDF");
    }
    qpdf
}

/// A committed fixture's path, by name: the files qpdf wrote, in
/// pdf-structure's `tests/files`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../pdf-structure/tests/files")
        .join(format!("{name}.pdf"))
}

/// Every PDF under `PDF_LINT_CORPUS`, in order.
fn corpus() -> Vec<PathBuf> {
    let dir = std::env::var_os("PDF_LINT_CORPUS").expect("PDF_LINT_CORPUS names a folder");
    let mut folders = vec![PathBuf::from(dir)];
    let mut files = Vec::new();
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
            } else if (path.extension()).is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// `doc`'s findings, but for where: qpdf says nowhere.
fn findings(doc: &Document) -> Vec<Finding> {
    let report = check(doc, &Hold::All(Made::Merged), &Options::default());
    (report.findings.into_iter())
        .map(|mut f| {
            f.at = None;
            f
        })
        .collect()
}

#[test]
#[ignore = "needs qpdf: just test-qpdf"]
fn a_file_checks_through_qpdf_alone_as_through_its_bytes() {
    for test in [
        "qpdf-plain",
        "qpdf-compressed",
        "qpdf-aes-256",
        "qpdf-aes-256-unpacked",
    ] {
        let path = fixture(test);
        let data = std::fs::read(&path).unwrap();
        let qpdf = qpdf(&path);
        let bytes = bytes::read(&data);
        let alone = qpdf.read().unwrap();
        // Checked, the two read alike, but for where.
        assert_eq!(findings(&bytes), findings(&alone), "{test}");
    }
}

/// What qpdf linearizes, with object streams and without, holds by the
/// check pdf-lint makes of it beside its reading: it's linearized, and no
/// error of its linearization is found.
#[test]
#[ignore = "needs qpdf: just test-qpdf"]
fn what_qpdf_linearizes_holds() {
    let dir = tempfile::tempdir().unwrap();
    let program = std::env::var_os("PDF_LINT_QPDF").unwrap_or_else(|| "qpdf".into());
    for test in ["qpdf-plain", "qpdf-compressed"] {
        let path = fixture(test);
        qpdf(&path);
        for streams in ["disable", "generate"] {
            let out = dir.path().join(format!("{test}-{streams}.pdf"));
            let status = std::process::Command::new(&program)
                .arg("--linearize")
                .arg(format!("--object-streams={streams}"))
                .args([&path, &out])
                .status()
                .unwrap();
            assert!(status.success(), "{test}, {streams}: qpdf {status}");
            let data = std::fs::read(&out).unwrap();
            let read = WithReading::new(&data[..], &[], &bytes::Options::DEFAULT).unwrap();
            let report = check_with_reading(&read, &Hold::All(Made::Merged), &Options::default());
            let found: Vec<_> = (report.findings.iter())
                .filter(|f| f.rule == Rule::Linearization)
                .collect();
            assert!(found.is_empty(), "{test}, {streams}: {report}");
            assert_eq!(
                report.linearized,
                Some(Linearized::Holds),
                "{test}, {streams}"
            );
        }
    }
}

/// What qpdf linearizes, with object streams and without, then updated by
/// pdf-structure's writer, its catalog written again: that the update voids
/// the linearization is found, once, and the file as first written holds,
/// so nothing of its linearization is.
#[test]
#[ignore = "needs qpdf: just test-qpdf"]
fn what_qpdf_linearizes_then_updated_is_found_so() {
    let dir = tempfile::tempdir().unwrap();
    let program = std::env::var_os("PDF_LINT_QPDF").unwrap_or_else(|| "qpdf".into());
    for test in ["qpdf-plain", "qpdf-compressed"] {
        let path = fixture(test);
        qpdf(&path);
        for streams in ["disable", "generate"] {
            let out = dir.path().join(format!("{test}-{streams}.pdf"));
            let status = std::process::Command::new(&program)
                .arg("--linearize")
                .arg(format!("--object-streams={streams}"))
                .args([&path, &out])
                .status()
                .unwrap();
            assert!(status.success(), "{test}, {streams}: qpdf {status}");
            let data = std::fs::read(&out).unwrap();
            let reading = bytes::Reading::open(&data[..], &bytes::Options::DEFAULT).unwrap();
            let root = (reading.trailer().dict().get(b"Root"))
                .and_then(|r| r.as_reference())
                .unwrap();
            let mut catalog = reading.get(root).unwrap().clone();
            catalog
                .as_dict_mut()
                .unwrap()
                .set("PageLayout", "SinglePage");
            let base = write::Base::of(&reading).unwrap();
            let bodies = [(root, write::Body::from(&catalog))];
            let options = write::Options::DEFAULT;
            let mut updated = data.clone();
            write::update(&mut updated, &base, bodies, None, None, &options).unwrap();
            let read = WithReading::new(&updated[..], &[], &bytes::Options::DEFAULT).unwrap();
            let report = check_with_reading(&read, &Hold::All(Made::Merged), &Options::default());
            let found: Vec<_> = (report.findings.iter())
                .filter(|f| matches!(f.rule, Rule::Linearization | Rule::Updated))
                .map(|f| f.rule)
                .collect();
            assert_eq!(found, [Rule::Updated], "{test}, {streams}: {report}");
            assert_eq!(
                report.linearized,
                Some(Linearized::Updated { held: true }),
                "{test}, {streams}"
            );
        }
    }
}

/// Every PDF under `PDF_LINT_CORPUS` read from its bytes and through qpdf
/// alone, and the findings one reading gives and the other doesn't counted
/// by rule and side, with examples: but for where and for what only the
/// bytes can show (`duplicate` and `reader`; what's in an earlier
/// revision's trailer, as qpdf reads the newest alone; and `packed`, which
/// needs where each object was read). A file encrypted so that the
/// reader doesn't open it by the empty password, which qpdf may, differs
/// by its object streams' objects and its strings, which only qpdf then
/// reads.
#[test]
#[ignore = "needs PDF_LINT_CORPUS, a folder of PDF files, and qpdf"]
fn a_corpus_checks_alike_through_qpdf_alone() {
    let files = corpus();
    /// Each kind's count, and its first examples.
    type Kinds = BTreeMap<String, (usize, Vec<String>)>;
    let add = |kinds: &mut Kinds, kind: String, example: String| {
        let (n, examples) = kinds.entry(kind).or_default();
        *n += 1;
        if examples.len() < 5 {
            examples.push(example);
        }
    };
    #[derive(Default)]
    struct Tally {
        findings_differing: usize,
        findings: Kinds,
    }
    let tally = Mutex::new(Tally::default());
    let next = AtomicUsize::new(0);
    let start = Instant::now();
    let like = |f: &Finding| {
        let object = f
            .object
            .map_or("the trailer".to_string(), |id| format!("{id} R"));
        format!("{} {}/{} in {object}", f.rule, f.table, f.key)
    };
    let findings = |doc: &Document| {
        (findings(doc).into_iter())
            .filter(|f| {
                !matches!(f.rule, Rule::Duplicate | Rule::Reader | Rule::Packed)
                    && f.revision.is_none()
            })
            .collect::<Vec<_>>()
    };
    thread::scope(|scope| {
        for _ in 0..thread::available_parallelism().map_or(1, |n| n.get()) {
            scope.spawn(|| {
                while let Some(path) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let name = path.file_name().unwrap().to_string_lossy().into_owned();
                    let data = std::fs::read(path).unwrap();
                    let qpdf = Qpdf::new(path);
                    let bytes = bytes::read(&data);
                    let alone = qpdf.read();
                    // The findings one reading gives and the other doesn't,
                    // as many times as it gives them: by object, table, key
                    // and rule, as the path an object is first reached by
                    // follows the order of keys, which qpdf sorts, and a
                    // value prints as its reader gives it.
                    let mut only = Vec::new();
                    if let Ok(a) = &alone {
                        let mut count: BTreeMap<String, (isize, Rule)> = BTreeMap::new();
                        for f in findings(&bytes) {
                            count.entry(like(&f)).or_insert((0, f.rule)).0 += 1;
                        }
                        for f in findings(a) {
                            count.entry(like(&f)).or_insert((0, f.rule)).0 -= 1;
                        }
                        for (f, (n, rule)) in count {
                            let side = if n > 0 { "the bytes" } else { "qpdf" };
                            for _ in 0..n.unsigned_abs() {
                                only.push((format!("{rule} only through {side}"), f.clone()));
                            }
                        }
                    }
                    let mut tally = tally.lock().unwrap();
                    tally.findings_differing += usize::from(!only.is_empty());
                    for (kind, f) in only {
                        add(&mut tally.findings, kind, format!("{name}: {f}"));
                    }
                }
            });
        }
    });
    let tally = tally.into_inner().unwrap();
    println!(
        "{} files in {:.1} s: {} with other findings",
        files.len(),
        start.elapsed().as_secs_f64(),
        tally.findings_differing,
    );
    for (kind, (n, examples)) in &tally.findings {
        println!("{kind}: {n}");
        for example in examples {
            let example: String = example.chars().take(300).collect();
            println!("    {example}");
        }
    }
}
