//! pdf-objects' reading by PDFium's rules held to PDFium's own object
//! parser, case by case. Each case is in `tests/pdfium` (`cases.tsv`, its
//! bytes in `cases/`, written by `cases.py`), with PDFium's answer for it
//! recorded beside it at the pinned build (`chromium-N.txt`, by the
//! repository's `oracle/pdfium.py`, which `oracle/README.md` describes);
//! here the same answer is made from what the parser reads by PDFium's
//! rules (`Parser::rules`), and the two must agree, but for the cases
//! `differ.tsv` names, which must still differ.
//!
//! A case is an object as written (`.obj`), asked of PDFium as its kind
//! says, and so read here:
//! - `mediabox`: a page's `/MediaBox`, PDFium's answer the page's width and
//!   height; here an array of four values, each a number as an `f32` or 0
//!   for what's none, its corners put in order, and where it's empty, or
//!   no such array, the 612 by 792 PDFium gives a page then.
//! - `info`: the document's `/Info` dictionary, PDFium's answer the text of
//!   its entry under the key `.tag` holds; here that entry's string decoded
//!   by PDFium's rules (`text::decode_with`), and none where there's no such
//!   entry, or no dictionary.
//! - `kids`: what a page tree's `/Kids` holds, the page object 3, of 50 by
//!   60, PDFium's answer the page's size, or `no-page`; here whether the
//!   array's first element is a reference to object 3.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use pdf_objects::parse::Parser;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Object, ObjectId, Value, text};

/// A case's text or data longer than this is written by its length and
/// FNV-1a 64 hash, as the oracle writes it.
const LONG: usize = 64;

/// The set's folder.
fn set() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pdfium")
}

/// Bytes as the oracle writes them: printable ASCII as it is but `%`, any
/// other byte `%XX`; none, `%` alone; past [`LONG`] bytes, `#`, its
/// length, `:` and its hash.
fn esc(data: &[u8]) -> String {
    if data.is_empty() {
        return "%".to_string();
    }
    if data.len() > LONG {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in data {
            h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
        }
        return format!("#{}:{h:016x}", data.len());
    }
    let mut out = String::new();
    for &b in data {
        if (0x21..=0x7e).contains(&b) && b != b'%' {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Whether two answers are alike, word by word, numbers as `f32`s.
fn agree(a: &str, b: &str) -> bool {
    let (a, b): (Vec<&str>, Vec<&str>) = (
        a.split_whitespace().collect(),
        b.split_whitespace().collect(),
    );
    a.len() == b.len()
        && a.iter().zip(&b).all(|(x, y)| {
            x == y
                || match (x.parse::<f32>(), y.parse::<f32>()) {
                    (Ok(x), Ok(y)) => x == y || (x.is_nan() && y.is_nan()),
                    _ => false,
                }
        })
}

/// A file's lines that aren't comments, each cut at its tabs.
fn rows(path: &Path) -> Vec<Vec<String>> {
    let text = fs::read_to_string(path).unwrap_or_default();
    (text.lines())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').map(str::to_string).collect())
        .collect()
}

/// The answers recorded at the pinned build: the set's one
/// `chromium-N.txt`.
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

/// A value as PDFium's rules read it, alone.
fn value(bytes: &[u8]) -> Option<Object> {
    Parser::at(bytes, 0)
        .rules(Rules::Pdfium)
        .value(&mut ())
        .ok()
}

/// A number as PDFium's object parser holds it, an `f32`; 0 for what's no
/// number.
fn float(object: &Object) -> f32 {
    match object.value() {
        Value::Integer(i) => *i as f32,
        Value::Real(r) => *r as f32,
        _ => 0.0,
    }
}

/// The page's size from its `/MediaBox`.
fn mediabox(bytes: &[u8]) -> String {
    let corners = value(bytes)
        .and_then(|v| {
            v.as_array()
                .map(|a| a.iter().map(float).collect::<Vec<f32>>())
        })
        .filter(|a| a.len() == 4);
    let (w, h) = match corners.as_deref() {
        Some(&[l, b, r, t]) => {
            let (l, r) = if l > r { (r, l) } else { (l, r) };
            let (b, t) = if b > t { (t, b) } else { (b, t) };
            if l >= r || b >= t {
                (612.0, 792.0)
            } else {
                (r - l, t - b)
            }
        }
        _ => (612.0, 792.0),
    };
    format!("{w} {h}")
}

/// The text of the `/Info` dictionary's entry under `tag`.
fn info(bytes: &[u8], tag: &[u8]) -> String {
    let text = value(bytes)
        .and_then(|v| v.as_dict().and_then(|d| d.get(tag)).cloned())
        .and_then(|e| match e.into_value() {
            Value::String(s, _) => s
                .as_bytes()
                .map(|bytes| text::decode_with(bytes, Rules::Pdfium).text),
            _ => None,
        })
        .unwrap_or_default();
    format!("text {}", esc(text.as_bytes()))
}

/// The page's size, where `/Kids` names it.
fn kids(bytes: &[u8]) -> String {
    let array = value(&[&b"["[..], bytes, b"]"].concat());
    let first = array
        .as_ref()
        .and_then(|a| a.as_array())
        .and_then(|a| a.first());
    match first.and_then(Object::as_reference) {
        Some(ObjectId { number: 3, .. }) => "50 60".to_string(),
        _ => "no-page".to_string(),
    }
}

#[test]
fn every_case_reads_as_pdfium_reads_it() {
    let recorded = recorded();
    let differ: HashMap<String, String> = (rows(&set().join("differ.tsv")).into_iter())
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    let mut wrong = Vec::new();
    let cases = rows(&set().join("cases.tsv"));
    assert!(!cases.is_empty());
    for case in &cases {
        let (name, kind) = (&case[0], &case[1]);
        let read = |ext: &str| fs::read(set().join("cases").join(format!("{name}.{ext}"))).unwrap();
        let ours = match kind.as_str() {
            "mediabox" => mediabox(&read("obj")),
            "info" => info(&read("obj"), &read("tag")),
            "kids" => kids(&read("obj")),
            _ => panic!("{name}: a kind this test doesn't read, {kind}"),
        };
        let pdfium = recorded
            .get(name)
            .unwrap_or_else(|| panic!("{name}: no answer recorded"));
        match (agree(pdfium, &ours), differ.contains_key(name)) {
            (true, false) | (false, true) => {}
            (false, false) => wrong.push(format!("{name}: PDFium {pdfium}, here {ours}")),
            (true, true) => wrong.push(format!(
                "{name}: read as PDFium reads it, but in differ.tsv"
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
