//! pdf-semantics' page tree written, as qpdf reads it: files of trees
//! [`write()`] wrote over a document's pages, through pdf-structure's writer,
//! which `qpdf --check` passes with nothing to say, and whose pages `qpdf
//! --show-pages` lists in the order they were given. qpdf is pdf-fixtures',
//! as pdf-structure's tests run it: `PDF_LINT_QPDF`, else `qpdf` on the
//! `PATH`. The test is ignored, so that `cargo test` needs no qpdf, and run
//! by `just test-qpdf` against the pinned one; its absence then fails it,
//! rather than passing it unrun.

use std::path::Path;

use pdf_fixtures::qpdf::Qpdf;
use pdf_objects::{Object, ObjectId, Version, dict};
use pdf_semantics::Limits;
use pdf_semantics::pages::{FANOUT, write};
use pdf_structure::source::Document;

/// A file of `n` pages under a tree [`write()`] wrote under `fanout`, numbered
/// by a document's ids, and its pages in order: the pages' ids handed out
/// first, then the nodes', then the catalog's. Each page is `/Type /Page`,
/// its `/Parent` and a `/MediaBox` of its own, its width its place; the root
/// holds the `/Resources` they inherit, which 7.7.3.3 has every page hold.
fn file_of(n: usize, fanout: usize) -> (Vec<u8>, Vec<ObjectId>) {
    let mut doc = Document::new(Version::new(1, 7), dict! {});
    let pages: Vec<ObjectId> = (0..n).map(|_| doc.new_id()).collect();
    let mut written = write(&pages, fanout, &Limits::DEFAULT, || doc.new_id()).unwrap();
    written.nodes[0].1.push("Resources", dict! {});
    for (id, node) in written.nodes {
        doc.set(id, node);
    }
    for (place, (&page, &parent)) in pages.iter().zip(&written.parents).enumerate() {
        let width = i64::try_from(place).unwrap() + 1;
        let media = vec![
            Object::from(0),
            Object::from(0),
            Object::from(width),
            Object::from(200),
        ];
        doc.set(
            page,
            dict! { "Type" => "Page", "Parent" => parent, "MediaBox" => media },
        );
    }
    let catalog = doc.add(dict! { "Type" => "Catalog", "Pages" => written.root });
    doc.trailer_mut().dict_mut().set("Root", catalog);
    let mut file = Vec::new();
    let options = pdf_structure::write::Options::DEFAULT;
    pdf_structure::write::document(&mut file, &doc, None, None, &options).unwrap();
    (file, pages)
}

/// What qpdf says, run with `args` and then the file at `path`: its exit
/// code and what it wrote, standard output then standard error, the file's
/// path as `FILE`.
fn asked(qpdf: &Qpdf, args: &[&str], path: &Path) -> (Option<i32>, String) {
    let mut args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    args.push(path.display().to_string());
    let ran = qpdf.exec(&args).expect("qpdf runs");
    let text =
        String::from_utf8_lossy(&ran.stdout).to_string() + &String::from_utf8_lossy(&ran.stderr);
    (ran.code, text.replace(&path.display().to_string(), "FILE"))
}

#[test]
#[ignore = "needs qpdf: `just test-qpdf`"]
fn qpdf_reads_a_written_tree_clean_and_in_order() {
    let folder = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(folder).unwrap();
    for (n, fanout) in [
        (1, FANOUT),
        (33, FANOUT),
        (1025, FANOUT),
        (7000, FANOUT),
        (1025, 2),
        (7000, usize::MAX),
    ] {
        let (bytes, pages) = file_of(n, fanout);
        let path = folder.join(format!("semantics-written-{n}-by-{fanout}.pdf"));
        std::fs::write(&path, bytes).unwrap();
        let qpdf = Qpdf::new(&path);
        if let Err(why) = qpdf.version() {
            panic!("this test needs qpdf ({why}): install it, or name it in PDF_LINT_QPDF");
        }
        let (code, said) = asked(&qpdf, &["--check"], &path);
        assert_eq!(code, Some(0), "{n} under {fanout}: {said}");
        let clean = "checking FILE\nPDF Version: 1.7\nFile is not encrypted\n\
                     File is not linearized\nNo syntax or stream encoding errors found; \
                     the file may still contain\nerrors that qpdf cannot detect\n";
        assert_eq!(said, clean, "{n} under {fanout}");
        let (code, said) = asked(&qpdf, &["--show-pages"], &path);
        assert_eq!(code, Some(0), "{n} under {fanout}: {said}");
        assert!(!said.contains("WARNING"), "{n} under {fanout}: {said}");
        let listed: Vec<&str> = (said.lines())
            .filter_map(|line| line.strip_prefix("page ")?.split_once(": "))
            .map(|(_, page)| page)
            .collect();
        let given: Vec<String> = pages.iter().map(|page| format!("{page} R")).collect();
        assert_eq!(listed, given, "{n} under {fanout}");
        std::fs::remove_file(&path).unwrap();
    }
}
