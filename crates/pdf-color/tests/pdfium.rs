//! pdf-color's count of a color space's components held to PDFium's own, case by
//! case. Each case is in `tests/pdfium` (`cases.tsv`, its bytes in `cases/`,
//! written by `cases.py`), with PDFium's answer for it recorded beside it at
//! the pinned build (`chromium-N.txt`, by the repository's `oracle/pdfium.py`,
//! which `oracle/README.md` describes); here the same answer is made from
//! [`components`] over [`Spaces`], and the two must agree, but for the cases
//! `differ.tsv` names, which must still differ.
//!
//! A case is a file (`.pdf`) of the kind `spaces`, whose page draws inline
//! images, each in a space its resources name. PDFium's answer is a word for
//! each image: how many components it sized the image by, or `none` where it has
//! no space for it, so an image of 1 component of 1 bit. Here each is the name
//! the content asks for, looked up as a reader of content looks it up: in a form's
//! own `/ColorSpace` dictionary, if it's drawn in a form that has one, else its
//! page's.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use pdf_color::{Limits, Object, Resolve, Spaces, components};
use pdf_objects::parse::Parser;
use pdf_structure::bytes::{Options, Reading};

/// How a case's image begins, up to the name of its color space.
const IMAGE: &[u8] = b"BI /W 8 /H 1 /BPC 8 /CS ";

/// The set's folder.
fn set() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pdfium")
}

/// A file's lines that aren't comments, each cut at its tabs.
fn rows(path: &Path) -> Vec<Vec<String>> {
    let text = fs::read_to_string(path).unwrap_or_default();
    (text.lines())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').map(str::to_string).collect())
        .collect()
}

/// The answers recorded at the pinned build: the set's one `chromium-N.txt`.
fn recorded() -> HashMap<String, String> {
    let files: Vec<PathBuf> = (fs::read_dir(set()).unwrap())
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            name.starts_with("chromium-") && name.ends_with(".txt")
        })
        .collect();
    assert_eq!(files.len(), 1, "one recorded answer file: {files:?}");
    (rows(&files[0]).into_iter())
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect()
}

/// The name of each image in `file`, as written, in order.
fn asked(file: &[u8]) -> Vec<&[u8]> {
    let mut names = Vec::new();
    let mut from = 0;
    while let Some(at) = (file[from..].windows(IMAGE.len())).position(|w| w == IMAGE) {
        let start = from + at + IMAGE.len();
        let length = (file[start..].iter()).position(|b| *b == b' ').unwrap();
        names.push(&file[start..start + length]);
        from = start + length;
    }
    names
}

/// The word for the image drawn in `name`, as written.
fn word<'o, R: Resolve<'o> + Copy>(objects: R, spaces: Option<Spaces<'o>>, name: &[u8]) -> String {
    let name = Parser::at(name, 0).value(&mut ()).expect("a name");
    let name = name.as_name().expect("a name");
    let Some(named) = spaces.and_then(|spaces| spaces.get(name)) else {
        return "none".to_string();
    };
    match components(objects, named.value, &Limits::DEFAULT) {
        Ok(Some(count)) => count.to_string(),
        Ok(None) => "none".to_string(),
        Err(error) => format!("error: {error}"),
    }
}

/// The answer for the images of `file`, over `objects`: object 3 is the page, by
/// `cases.py`, and one that doesn't read, as a file's bytes can't under this
/// reader's rules, has no resources; object 5 a form, if it's one.
fn answer<'o, R: Resolve<'o> + Copy>(objects: R, file: &[u8]) -> String {
    let page: Option<&'o Object> = objects.resolve((3, 0));
    let resources = page
        .and_then(Object::as_dict)
        .and_then(|d| d.get(b"Resources"));
    let page_spaces = Spaces::read(objects, resources).expect("the case's objects read");
    let form = (objects.resolve((5, 0)))
        .and_then(Object::as_stream)
        .filter(|stream| stream.dict.get(b"Subtype").and_then(Object::as_name) == Some(b"Form"));
    let form_spaces = form.and_then(|form| {
        Spaces::read(objects, form.dict.get(b"Resources")).expect("the case's objects read")
    });
    let spaces = form_spaces.or(page_spaces);
    let words: Vec<String> = (asked(file).into_iter())
        .map(|name| word(objects, spaces, name))
        .collect();
    words.join(" ")
}

/// The answer for the file `bytes`, over a reading of it.
fn ours(bytes: &[u8]) -> String {
    let reading = Reading::open(bytes, &Options::DEFAULT).expect("a case opens");
    answer(&reading, bytes)
}

#[test]
fn every_case_counts_as_pdfium_counts_it() {
    let recorded = recorded();
    let differ: HashMap<String, String> = (rows(&set().join("differ.tsv")).into_iter())
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    let mut wrong = Vec::new();
    let cases = rows(&set().join("cases.tsv"));
    assert!(!cases.is_empty());
    for case in &cases {
        let (name, kind) = (&case[0], &case[1]);
        assert_eq!(kind, "spaces", "{name}: a kind this test doesn't read");
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let ours = ours(&bytes);
        let pdfium = recorded
            .get(name)
            .unwrap_or_else(|| panic!("{name}: no answer recorded"));
        match (pdfium == &ours, differ.contains_key(name)) {
            (true, false) | (false, true) => {}
            (false, false) => wrong.push(format!("{name}: PDFium {pdfium}, here {ours}")),
            (true, true) => wrong.push(format!(
                "{name}: counted as PDFium counts it, but in differ.tsv"
            )),
        }
    }
    for name in differ.keys() {
        if !cases.iter().any(|c| &c[0] == name) {
            wrong.push(format!("{name}: in differ.tsv but no case"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The answers are the same over a document made from the reading.
#[test]
fn every_case_counts_alike_over_a_document() {
    let cases = rows(&set().join("cases.tsv"));
    for case in &cases {
        let name = &case[0];
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let reading = Reading::open(&bytes, &Options::DEFAULT).expect("a case opens");
        let document = reading.to_document().expect("a case makes a document");
        assert_eq!(answer(&document, &bytes), ours(&bytes), "{name}");
    }
}
