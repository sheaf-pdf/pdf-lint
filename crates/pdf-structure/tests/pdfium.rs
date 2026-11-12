//! The reader under [`Recovery::Pdfium`] held to PDFium's own reading, case
//! by case, and under [`Recovery::Verified`] held to differ from it only
//! where it's said to. Each case is in `tests/pdfium` (`cases.tsv`, its
//! bytes in `cases/`, written by `cases.py`), with PDFium's answer for it
//! recorded beside it at the pinned build (`chromium-N.txt`, by the
//! repository's `oracle/pdfium.py`, which `oracle/README.md` describes);
//! here the same answer is made from the file read under each recovery and
//! its page tree walked by pdf-semantics' PDFium rules, and the two must
//! agree, but for the cases `differ.tsv` names under a recovery, which must
//! still differ under it.
//!
//! A case is a file (`.pdf`), of the kind `pages`: PDFium's answer is its
//! count of pages, then each page from the first to two past the last, its
//! width, height and quarter turns, or `null`; `no-document` for a file it
//! doesn't open. Here a reading with no catalog after a scan is a file
//! PDFium doesn't open, as PDFium opens none whose scan gives no catalog.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use pdf_objects::pdf_tokenizer::Rules;
use pdf_semantics::pages::{Error, Origin, Page, Pages, Rect, pages};
use pdf_semantics::{Limits, catalog};
use pdf_structure::read::{Options, Reading, Recovery};

/// How many pages of a file the oracle asks for, as `oracle/pdfium.py` has it.
const PAGES: usize = 64;

/// Each recovery the cases are read under, by the name `differ.tsv` gives it.
const RECOVERIES: [(&str, Recovery); 2] = [
    ("verified", Recovery::Verified),
    ("pdfium", Recovery::Pdfium),
];

/// The set's folder.
fn set() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pdfium")
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
                    (Ok(x), Ok(y)) => x == y,
                    _ => false,
                }
        })
}

/// A file's lines that aren't comments, each cut at its tabs: read by
/// lines, so that a checkout that writes them with CR LF reads alike.
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

/// A box, as PDFium holds one: its corners in order, in `f32`s.
fn ordered(rect: Rect) -> [f32; 4] {
    let [a, b, c, d] = rect.0.map(|n| n as f32);
    [a.min(c), b.min(d), a.max(c), b.max(d)]
}

fn is_empty(rect: [f32; 4]) -> bool {
    rect[0] >= rect[2] || rect[1] >= rect[3]
}

/// What PDFium makes of a page, as pdf-semantics' own cases have it: an
/// empty media box is Letter; an empty crop box is the media box, any
/// other is clipped to it, and one outside it leaves nothing; whole quarter
/// turns, `(degrees / 90) % 4`, turn what's shown.
fn shown(page: &Page) -> String {
    let media = page.media_box();
    let media = if media.origin == Origin::Default || is_empty(ordered(media.value)) {
        ordered(Rect::LETTER)
    } else {
        ordered(media.value)
    };
    let crop = page.crop_box();
    let crop = (crop.origin != Origin::Default)
        .then(|| ordered(crop.value))
        .filter(|crop| !is_empty(*crop));
    let visible = match crop {
        None => media,
        Some(crop) => {
            let clipped = [
                crop[0].max(media[0]),
                crop[1].max(media[1]),
                crop[2].min(media[2]),
                crop[3].min(media[3]),
            ];
            if clipped[0] > clipped[2] || clipped[1] > clipped[3] {
                [0.0; 4]
            } else {
                clipped
            }
        }
    };
    let (width, height) = (visible[2] - visible[0], visible[3] - visible[1]);
    let turns = (page.rotate().value / 90).rem_euclid(4);
    let (width, height) = if turns % 2 == 1 {
        (height, width)
    } else {
        (width, height)
    };
    format!("{width} {height} {turns}")
}

/// The answer the table gives, as the oracle writes PDFium's.
fn answer(table: &Pages) -> String {
    let count = table.counted();
    let each: Vec<String> = (0..count.min(PAGES) + 2)
        .map(|index| match table.get(index) {
            Some(page) if page.dict.is_some() => shown(page),
            _ => "null".to_string(),
        })
        .collect();
    format!("count {count} : {}", each.join(" ; "))
}

/// The answer for the file `bytes`, read under `recovery`.
fn ours(bytes: &[u8], recovery: Recovery) -> String {
    let mut options = Options::DEFAULT;
    options.recovery = recovery;
    let Ok(reading) = Reading::open(bytes, &options) else {
        return "no-document".to_string();
    };
    let objects = &reading;
    let Ok(Some(catalog)) = catalog(objects, reading.trailer().dict()) else {
        return match reading.trailer().unwritten {
            Some(_) => "no-document".to_string(),
            None => "count 0 : null ; null".to_string(),
        };
    };
    match pages(objects, catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ()) {
        Ok(table) => answer(&table),
        Err(Error::NoPages) => "count 0 : null ; null".to_string(),
        Err(error) => format!("error: {error}"),
    }
}

#[test]
fn every_case_reads_as_pdfium_reads_it_under_its_recovery() {
    let recorded = recorded();
    let listed = rows(&set().join("differ.tsv"));
    for r in &listed {
        assert!(
            r.len() == 3 && RECOVERIES.iter().any(|(name, _)| *name == r[1]),
            "differ.tsv: {r:?} isn't a name, the recovery it's read otherwise under and why"
        );
    }
    let differ: HashSet<(String, String)> = (listed.iter())
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    let mut wrong = Vec::new();
    let cases = rows(&set().join("cases.tsv"));
    assert!(!cases.is_empty());
    for case in &cases {
        let (name, kind) = (&case[0], &case[1]);
        assert_eq!(kind, "pages", "{name}: a kind this test doesn't read");
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let pdfium = recorded
            .get(name)
            .unwrap_or_else(|| panic!("{name}: no answer recorded"));
        for (recovery, how) in RECOVERIES {
            let ours = ours(&bytes, how);
            let listed = differ.contains(&(name.clone(), recovery.to_string()));
            match (agree(pdfium, &ours), listed) {
                (true, false) | (false, true) => {}
                (false, false) => {
                    wrong.push(format!("{name}, {recovery}: PDFium {pdfium}, here {ours}"));
                }
                (true, true) => wrong.push(format!(
                    "{name}, {recovery}: read as PDFium reads it, but in differ.tsv"
                )),
            }
        }
    }
    for (name, _) in &differ {
        if !cases.iter().any(|c| &c[0] == name) {
            wrong.push(format!("{name}: in differ.tsv but no case"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
