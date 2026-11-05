//! The tokenizer's readings by PDFium's rules held to PDFium's own, case
//! by case. Each case is in `tests/pdfium` (`cases.tsv`, its bytes in
//! `cases/`, written by `cases.py`), with PDFium's answer for it recorded
//! beside it at the pinned build (`chromium-N.txt`, by the repository's
//! `oracle/pdfium.py`, which `oracle/README.md` describes); here the same
//! answer is made from the tokenizer's reading, and the two must agree,
//! but for the cases `differ.tsv` names, which must still differ.
//!
//! A case is a word (`.word`), asked of PDFium as its kind says, and so
//! read here:
//! - `number`: written as `re`'s second operand, PDFium's answer the value
//!   it holds, or `not-a-number` where it reads a keyword; here the word's
//!   one token by PDFium's rules, an integer or a real read by
//!   `decode::pdfium::number`, a keyword none, and anything else 0, as
//!   PDFium reads an operand that's no number.
//! - `hex` and `literal`: shown by `Tj` in Courier, PDFium's answer the
//!   text shown, or `none` where nothing is; here the string's bytes, read
//!   by `decode::pdfium`, each as the character it is in Latin-1 (the
//!   cases' strings are ASCII, which the font shows as itself), none for
//!   an empty string, which PDFium shows nothing of.
//! - `name`: as `Tf`'s font, PDFium's answer whether it found the font
//!   `/F0`; here whether the word is one name, which `decode::pdfium::name`
//!   reads as `F0`.
//!
//! A `number`, `hex` or `literal` word that isn't one token of its kind
//! here answers so, which PDFium never does.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use pdf_tokenizer::decode::pdfium;
use pdf_tokenizer::{Kind, Pdfium, Tokenizer};

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

/// The word's tokens by PDFium's rules, white-space left out.
fn tokens(word: &[u8]) -> Vec<(Kind, &[u8])> {
    (Tokenizer::new(word).with_rules(Pdfium))
        .filter(|t| t.kind != Kind::Space)
        .map(|t| (t.kind, t.bytes))
        .collect()
}

/// A string's bytes as the text a standard font shows of them.
fn text(bytes: impl Iterator<Item = u8>) -> String {
    let bytes: Vec<u8> = bytes.collect();
    if bytes.is_empty() {
        return "none".to_string();
    }
    let shown: String = bytes.iter().map(|&b| char::from(b)).collect();
    format!("text {}", esc(shown.as_bytes()))
}

/// The tokenizer's answer for a case.
fn answer(kind: &str, word: &[u8]) -> String {
    let tokens = tokens(word);
    let one = match tokens.as_slice() {
        [one] => Some(*one),
        _ => None,
    };
    match (kind, one) {
        ("number", Some((Kind::Integer | Kind::Real, bytes))) => {
            format!("{}", pdfium::number(bytes).float())
        }
        ("number", Some((Kind::Keyword, _))) => "not-a-number".to_string(),
        ("number", Some(_)) => "0".to_string(),
        ("hex", Some((Kind::HexString, bytes))) => text(pdfium::hex(bytes)),
        ("literal", Some((Kind::LiteralString, bytes))) => text(pdfium::literal(bytes)),
        ("name", Some((Kind::Name, bytes))) if pdfium::name(bytes).eq(*b"F0") => {
            "found".to_string()
        }
        ("name", _) => "not-found".to_string(),
        // Not one token of the case's kind: no answer PDFium gives.
        _ => format!("read-as-{}-tokens", tokens.len()),
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
        let word = fs::read(set().join("cases").join(format!("{name}.word"))).unwrap();
        let pdfium = recorded
            .get(name)
            .unwrap_or_else(|| panic!("{name}: no answer recorded"));
        let ours = answer(kind, &word);
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
