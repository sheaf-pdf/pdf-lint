//! pdf-fonts' reading of a font held to PDFium's own, case by case. Each case is in
//! `tests/pdfium` (`cases.tsv`, its bytes in `cases/`, written by `cases.py`), with
//! PDFium's answer for it recorded beside it at the pinned build (`chromium-N.txt`,
//! by the repository's `oracle/pdfium.py`, which `oracle/README.md` describes); here
//! the same answer is made from [`Font`] over a reading of the file, and the two must
//! agree, but for the cases `differ.tsv` names, which must still differ.
//!
//! A case is a file (`.pdf`) whose page shows one string in one font, object 10. Its
//! answer is what PDFium's text page says of each character: its text and its origin
//! from where the string starts. A kind's `-heights` adds how far the first
//! character's loose box reaches from its origin, for a font whose heights aren't left
//! to a face PDFium substitutes, and its `-boxes` each character's box and loose box,
//! for one whose glyphs are a program it embeds. Here the characters are made as PDFium's
//! text page makes them, which isn't [`Font`]'s to say: a code with no text is its own
//! text, one the same as a code just before it at the same place isn't kept, nor a space
//! after a space, and nothing's kept of a string that takes up no room. A case whose name
//! begins `predefined-` or `program-` wants the feature that reads it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use pdf_fonts::pdf_content_streams::{Limits as ContentLimits, ResCx, operations};
use pdf_fonts::{Fault, Font, FontKind, Limits, Rect, Resolve};
use pdf_structure::bytes::{Options, Reading};

/// The most characters back a text page looks for the same code at the same place
/// (`CPDF_TextPage::ProcessTextObjectItems`).
const SAME_WITHIN: usize = 7;
/// How near that place is at the cases' font size of 1000: 0.07 of it.
const SAME_NEAR: f32 = 70.0;

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

/// A character a text page keeps: its code, its text, and its origin.
struct Character {
    code: u32,
    unicode: u32,
    x: f32,
    y: f32,
}

/// The characters PDFium's text page makes of `shown` in `font`, with word spacing
/// `tw`.
fn characters(font: &Font, shown: &[u8], tw: f32) -> Vec<Character> {
    let mut out: Vec<Character> = Vec::new();
    let mut at = 0.0f32;
    for (range, code) in font.codes(shown) {
        let (x, y) = match font.vertical(code) {
            Some(v) => (-v.vx, at - v.vy),
            None => (at, 0.0),
        };
        at += match font.vertical(code) {
            Some(v) => v.w1,
            None => font.width(code),
        };
        if font.word_spaced(code, range.len()) {
            at += tw;
        }
        let same = (out.iter().rev().take(SAME_WITHIN))
            .any(|c| c.code == code && (c.x - x).abs() < SAME_NEAR && (c.y - y).abs() < SAME_NEAR);
        if same {
            continue;
        }
        let units: Vec<u32> = match font.code_text(code) {
            Some(text) => text.chars().map(u32::from).collect(),
            None => vec![code],
        };
        for unicode in units {
            let after_space = out.last().is_some_and(|c| c.unicode == 32) && unicode == 32;
            if !after_space {
                out.push(Character {
                    code,
                    unicode,
                    x,
                    y,
                });
            }
        }
    }
    out
}

/// The box PDFium has for `code`, its glyph's own, where it's known: a Type 3 font's, or
/// that of a program it read.
fn own_box(font: &Font, code: u32) -> Option<Rect> {
    match font.outlined() {
        true => font.char_box(code),
        false => font.glyph_box(code),
    }
}

/// Whether a text page keeps nothing of `shown` in a Type 3 font or one whose program
/// was read, whose glyphs' boxes are all it knows of the room a string takes up
/// (`CPDF_TextObject::CalcPositionDataInternal`).
fn takes_no_room(font: &Font, shown: &[u8]) -> bool {
    if font.kind() != FontKind::Type3 && !font.outlined() {
        return false;
    }
    let (mut low, mut high, mut at) = (f32::INFINITY, f32::NEG_INFINITY, 0.0f32);
    for (_, code) in font.codes(shown) {
        let b = own_box(font, code).unwrap_or_default();
        match font.vertical(code) {
            Some(v) => {
                low = low.min(b.x0 - v.vx);
                high = high.max(b.x1 - v.vx);
            }
            None => {
                low = low.min(at + b.x0);
                high = high.max(at + b.x1);
                at += font.width(code);
            }
        }
    }
    (high - low).abs() < 0.0001
}

/// A character's box on a text page, from its origin (`ProcessTextObjectItems`), and its
/// loose box (`GetLooseBounds`), as left, bottom, right and top; at a font size of 1000 a
/// glyph unit is a unit. A box of no height is one unit high, and one of no width its
/// character's width wide.
fn boxes(font: &Font, code: u32) -> ([f32; 4], [f32; 4]) {
    let glyph = own_box(font, code).unwrap_or_default();
    let width = match font.vertical(code) {
        Some(v) => v.w1,
        None => font.width(code),
    };
    let [mut left, mut bottom, mut right, mut top] = [glyph.x0, glyph.y0, glyph.x1, glyph.y1];
    if (top - bottom).abs() < 0.0001 {
        top = bottom + 1.0;
    }
    if (right - left).abs() < 0.0001 {
        right = left + width;
    }
    // `TransformRect` normalizes it: a vertical advance is below 0.
    if right < left {
        (left, right) = (right, left);
    }
    if top < bottom {
        (bottom, top) = (top, bottom);
    }
    let tight = [left, bottom, right, top];
    if left >= right || bottom >= top {
        return (tight, tight);
    }
    let metrics = font.metrics();
    let spanned = match font.vertical(code) {
        Some(v) => Some([v.vx - 500.0, v.vy + v.w1, v.vx + 500.0, v.vy]),
        // Normalized too: an `/Ascent` below `/Descent` is only the other way up.
        None if metrics.ascent != metrics.descent => Some([
            0.0,
            metrics.descent.min(metrics.ascent),
            width,
            metrics.descent.max(metrics.ascent),
        ]),
        None => None,
    };
    let loose = spanned.map_or(tight, |[l, b, r, t]| {
        [l.min(left), b.min(bottom), r.max(right), t.max(top)]
    });
    (tight, loose)
}

/// The answer for the file `bytes`, of the kind `kind`, over a reading of it.
fn ours(bytes: &[u8], kind: &str) -> String {
    let reading = Reading::open(bytes, &Options::DEFAULT).expect("a case opens");
    let dict = (&reading).resolve((10, 0)).and_then(|o| o.as_dict());
    let Some(dict) = dict else {
        return "no font".to_string();
    };
    let content = reading.decoded((4, 0)).expect("a case has its content");
    let mut shown = Vec::new();
    let mut tw = 0.0;
    let none = ResCx::none();
    for operation in operations(&content, &none, &ContentLimits::DEFAULT) {
        let operation = operation.expect("a case's content reads");
        match operation.name() {
            b"Tw" => tw = operation.arg(0).expect("its operand").number(),
            b"Tj" => {
                let string = operation.arg(0).expect("its operand");
                let string = string.object(&ContentLimits::DEFAULT).expect("a string");
                shown = string.as_bytes().expect("a string").to_vec();
            }
            _ => {}
        }
    }
    let mut budget = 1 << 20;
    let mut faults = Vec::new();
    let font = Font::read(&reading, dict, &Limits::DEFAULT, &mut budget, &mut faults)
        .expect("the case's objects read");
    // PDFium doesn't load such a font, and draws nothing in it. Each is told.
    let told =
        (faults.iter()).any(|(_, fault)| matches!(fault, Fault::DescendantFonts | Fault::CMap));
    assert_eq!(font.loaded(), !told, "whether it's loaded, by its faults");
    if !font.loaded() || takes_no_room(&font, &shown) {
        return "none".to_string();
    }
    let characters = characters(&font, &shown, tw);
    let mut words: Vec<String> = (characters.iter())
        .map(|c| {
            let word = format!("{:04x} {} {}", c.unicode, c.x, c.y);
            if kind != "fonts-boxes" {
                return word;
            }
            let (tight, loose) = boxes(&font, c.code);
            let [a, b, c, d] = tight;
            let [e, f, g, h] = loose;
            format!("{word} box {a} {b} {c} {d} loose {e} {f} {g} {h}")
        })
        .collect();
    if words.is_empty() {
        return "none".to_string();
    }
    if kind == "fonts-heights" {
        let metrics = font.metrics();
        if font.approx().heights && metrics.ascent != metrics.descent {
            return "heights guessed".to_string();
        }
        // A character's loose box is the font's heights, or its own box where they're
        // one, over its own box, a line high where that has no height.
        let glyph = own_box(&font, characters[0].code).unwrap_or_default();
        let (bottom, top) = match glyph.y0 == glyph.y1 {
            true => (glyph.y0, glyph.y0 + 1.0),
            false => (glyph.y0, glyph.y1),
        };
        let (bottom, top) = match metrics.ascent != metrics.descent {
            true => (metrics.descent.min(bottom), metrics.ascent.max(top)),
            false => (bottom, top),
        };
        words.push(format!("top {top} bottom {bottom}"));
    }
    words.join(" ; ")
}

/// Two answers alike, word by word, numbers as 32-bit floats.
fn agree(a: &str, b: &str) -> bool {
    let (a, b): (Vec<&str>, Vec<&str>) = (a.split(' ').collect(), b.split(' ').collect());
    a.len() == b.len()
        && a.iter().zip(&b).all(|(a, b)| {
            a == b
                || match (a.parse::<f32>(), b.parse::<f32>()) {
                    (Ok(a), Ok(b)) => a == b,
                    _ => false,
                }
        })
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
        assert!(
            kind.starts_with("fonts"),
            "{name}: a kind this test doesn't read"
        );
        if name.starts_with("predefined-") && !cfg!(feature = "predefined-cmaps") {
            continue;
        }
        if name.starts_with("program-") && !cfg!(feature = "programs") {
            continue;
        }
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let ours = ours(&bytes, kind);
        let pdfium = recorded
            .get(name)
            .unwrap_or_else(|| panic!("{name}: no answer recorded"));
        match (agree(pdfium, &ours), differ.contains_key(name)) {
            (true, false) | (false, true) => {}
            (false, false) => wrong.push(format!("{name}:\n  PDFium {pdfium}\n  here   {ours}")),
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
    assert!(
        wrong.is_empty(),
        "{} wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}
