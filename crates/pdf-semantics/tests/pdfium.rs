//! pdf-semantics' page tree by PDFium's rules held to PDFium's own, case by
//! case. Each case is in `tests/pdfium` (`cases.tsv`, its bytes in `cases/`,
//! written by `cases.py`), with PDFium's answer for it recorded beside it at
//! the pinned build (`chromium-N.txt`, by the repository's `oracle/pdfium.py`,
//! which `oracle/README.md` describes); here the same answer is made from the
//! table [`pages`] gives, and the two must agree, but for the cases `differ.tsv`
//! names, which must still differ.
//!
//! A case is a file (`.pdf`), of the kind `pages`: PDFium's answer is its count
//! of pages, then each page from the first to two past the last, its width,
//! height and quarter turns, or `null`. Here the count is [`Pages::counted`],
//! and a page's size what PDFium makes of its boxes.
//!
//! Three cases are trees as [`write()`] writes them, `cases.py` laying them out
//! in its own code: so PDFium's answer for each is what it makes of the
//! writer's tree, and a test here holds the writer's nodes to the case's.
//! PDFium answers for the first 66 places of a tree at most, so of 1,025
//! pages, it's their count and those.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Object, ObjectId};
use pdf_semantics::action::Action;
use pdf_semantics::dest::DestTable;
use pdf_semantics::outlines::OutlineItem;
use pdf_semantics::pages::{Error, FANOUT, Origin, Page, Pages, PdfBox, counted, pages, write};
use pdf_semantics::{Limits, catalog, files, labels, outlines, viewer};
use pdf_structure::read::{self, Options, Reading};

/// How many pages of a file the oracle asks for, as `oracle/pdfium.py` has it.
const PAGES: usize = 64;

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

/// A box, as PDFium holds one: its corners in order, in `f32`s.
fn ordered(rect: PdfBox) -> [f32; 4] {
    let [a, b, c, d] = rect.0.map(|n| n as f32);
    [a.min(c), b.min(d), a.max(c), b.max(d)]
}

fn is_empty(rect: [f32; 4]) -> bool {
    rect[0] >= rect[2] || rect[1] >= rect[3]
}

/// What PDFium makes of a page: an empty media box is Letter; an empty crop box
/// is the media box, any other is clipped to it, and one outside it leaves
/// nothing; whole quarter turns, `(degrees / 90) % 4`, turn what's shown.
fn shown(page: &Page) -> String {
    let media = page.media_box();
    let media = if media.origin == Origin::Default || is_empty(ordered(media.value)) {
        ordered(PdfBox::LETTER)
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

/// Encodes bytes as one word matching `oracle/pdfium.py`'s `esc(data)`.
fn esc(data: &[u8]) -> String {
    if data.is_empty() {
        return "%".to_string();
    }
    if data.len() > 64 {
        let mut h: u64 = 0xCBF2_9CE4_8422_2325;
        for &b in data {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3);
        }
        return format!("#{}:{h:016x}", data.len());
    }
    let mut out = String::new();
    for &b in data {
        if (0x21..=0x7E).contains(&b) && b != b'%' {
            out.push(char::from(b));
        } else {
            use std::fmt::Write as _;
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// The answer for the `pages` case `bytes`.
fn ours_pages(bytes: &[u8]) -> String {
    let Ok(reading) = Reading::open(bytes, &Options::DEFAULT) else {
        return "no-document".to_string();
    };
    let objects = &reading;
    let Ok(Some(catalog)) = catalog(objects, reading.trailer().dict()) else {
        return "count 0 : null ; null".to_string();
    };
    match pages(objects, catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ()) {
        Ok(table) => answer(&table),
        Err(Error::NoPages) => "count 0 : null ; null".to_string(),
        Err(error) => format!("error: {error}"),
    }
}

fn ours_outlines(bytes: &[u8]) -> String {
    let Ok(reading) = Reading::open(bytes, &Options::DEFAULT) else {
        return "no-document".to_string();
    };
    let Ok(Some(cat)) = catalog(&reading, reading.trailer().dict()) else {
        return "none".to_string();
    };
    let Ok(page_table) = pages(&reading, cat, Rules::Pdfium, &Limits::DEFAULT, &mut ()) else {
        return "none".to_string();
    };
    let Ok(dests) = DestTable::read(
        &reading,
        Some(cat),
        &page_table,
        Rules::Pdfium,
        &Limits::DEFAULT,
        &mut (),
    ) else {
        return "none".to_string();
    };
    let Ok(Some(items)) = outlines::read(&reading, cat, Rules::Pdfium, &Limits::DEFAULT, &mut ())
    else {
        return "none".to_string();
    };
    let mut rows = Vec::new();
    fn emit(items: &[OutlineItem], depth: usize, dests: &DestTable, rows: &mut Vec<String>) {
        for item in items {
            let count = item.raw_count.unwrap_or(0);
            let style = item.flags.bits();
            let mut words = vec![
                "bm".to_string(),
                "depth".to_string(),
                depth.to_string(),
                "title".to_string(),
                esc(item.title.as_bytes()),
                "count".to_string(),
                count.to_string(),
                "style".to_string(),
                style.to_string(),
            ];
            if let Some([r, g, b]) = item.color {
                words.push("color".to_string());
                words.push(format!("{r}"));
                words.push(format!("{g}"));
                words.push(format!("{b}"));
            }
            // `FPDFBookmark_GetDest` checks `/Dest`, or `/A` when `/S /GoTo`.
            let effective_dest = item.dest.as_ref().or_else(|| {
                item.actions.first().and_then(|a| match a {
                    Action::GoTo { dest } => dest.as_ref(),
                    _ => None,
                })
            });
            if let Some(d) = effective_dest {
                let page_idx = dests
                    .resolve(d, Rules::Pdfium, &mut ())
                    .map_or(-1i64, |rd| i64::try_from(rd.page_index).unwrap_or(-1));
                words.push("dest_page".to_string());
                words.push(page_idx.to_string());
            }
            if let Some(act) = item.actions.first() {
                let code = match act {
                    Action::GoTo { .. } => 1,
                    Action::GoToR { .. } => 2,
                    Action::Uri { .. } => 3,
                    Action::Launch { .. } => 4,
                    Action::GoToE { .. } => 5,
                    _ => 0,
                };
                words.push("act".to_string());
                words.push(code.to_string());
                if let Action::Uri { uri, .. } = act
                    && !uri.is_empty()
                {
                    words.push("uri".to_string());
                    words.push(esc(uri));
                }
            }
            rows.push(words.join(" "));
            emit(&item.children, depth + 1, dests, rows);
        }
    }
    emit(&items, 0, &dests, &mut rows);
    if rows.is_empty() {
        "none".to_string()
    } else {
        rows.join(" ; ")
    }
}

fn ours_labels(bytes: &[u8]) -> String {
    let Ok(reading) = Reading::open(bytes, &Options::DEFAULT) else {
        return "no-document".to_string();
    };
    let Ok(Some(cat)) = catalog(&reading, reading.trailer().dict()) else {
        return "count 0 : null".to_string();
    };
    let count = counted(&reading, cat, &Limits::DEFAULT).unwrap_or(0);
    let tbl = labels::read(&reading, cat, Rules::Pdfium, &Limits::DEFAULT, &mut ())
        .ok()
        .flatten();
    let each: Vec<String> = (0..count.min(PAGES) + 1)
        .map(|idx| {
            if idx >= count {
                "null".to_string()
            } else {
                match &tbl {
                    None => "null".to_string(),
                    Some(lbls) => esc(lbls.format(idx).as_bytes()),
                }
            }
        })
        .collect();
    format!("count {count} : {}", each.join(" ; "))
}

fn ours_attachments(bytes: &[u8]) -> String {
    let Ok(reading) = Reading::open(bytes, &Options::DEFAULT) else {
        return "no-document".to_string();
    };
    let Ok(Some(cat)) = catalog(&reading, reading.trailer().dict()) else {
        return "none".to_string();
    };
    let Ok(specs) = files::embedded_files(&reading, cat, Rules::Pdfium, &Limits::DEFAULT, &mut ())
    else {
        return "none".to_string();
    };
    if specs.is_empty() {
        return "none".to_string();
    }
    specs
        .iter()
        .enumerate()
        .map(|(i, sp)| {
            let desc = sp.description.as_deref().unwrap_or("");
            let sub = sp
                .embedded
                .as_ref()
                .and_then(|e| e.subtype.as_deref())
                .unwrap_or("");
            let rel = sp
                .relationship
                .as_ref()
                .map_or(&b""[..], files::AfRelationship::as_bytes);
            let data_word = match &sp.embedded {
                Some(emb) => match emb.bytes(&reading, Limits::DEFAULT.embedded_bytes) {
                    Ok(b) => esc(&b),
                    Err(_) => "unreadable".to_string(),
                },
                None => "unreadable".to_string(),
            };
            format!(
                "att {i} name {} desc {} sub {} rel {} data {data_word}",
                esc(sp.name.as_bytes()),
                esc(desc.as_bytes()),
                esc(sub.as_bytes()),
                esc(rel),
            )
        })
        .collect::<Vec<_>>()
        .join(" ; ")
}

fn ours_viewer(bytes: &[u8]) -> String {
    let Ok(reading) = Reading::open(bytes, &Options::DEFAULT) else {
        return "no-document".to_string();
    };
    let Ok(Some(cat)) = catalog(&reading, reading.trailer().dict()) else {
        return "no-document".to_string();
    };
    let Ok(view) = viewer::read(&reading, cat, Rules::Pdfium, &mut ()) else {
        return "no-document".to_string();
    };
    let vp = view.viewer_prefs.unwrap_or_default();
    let scaling = i32::from(vp.print_scaling_enabled());
    let copies = vp.effective_num_copies();
    let duplex = vp.duplex.map_or(0, viewer::Duplex::pdfium_code);
    let range = if vp.print_page_range.is_empty() {
        "none".to_string()
    } else {
        vp.print_page_range
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    };
    let mut words = vec![
        "scaling".to_string(),
        scaling.to_string(),
        "copies".to_string(),
        copies.to_string(),
        "duplex".to_string(),
        duplex.to_string(),
        "range".to_string(),
        range,
    ];
    if let Some(d) = vp.direction {
        words.push("Direction".to_string());
        words.push(esc(d.as_name().as_bytes()));
    }
    if let Some(m) = vp.non_full_screen_page_mode {
        words.push("NonFullScreenPageMode".to_string());
        words.push(esc(m.as_name().as_bytes()));
    }
    if let Some(b) = vp.view_area {
        words.push("ViewArea".to_string());
        words.push(esc(b.as_name().as_bytes()));
    }
    if let Some(b) = vp.print_area {
        words.push("PrintArea".to_string());
        words.push(esc(b.as_name().as_bytes()));
    }
    // `FPDFCatalog_GetLanguage` returns an empty UTF-16 string (`2` bytes -> `esc(b"") == "%"`)
    // when `/Lang` is absent or not a string.
    let lang = view.lang.as_deref().unwrap_or("");
    words.push("lang".to_string());
    words.push(esc(lang.as_bytes()));
    words.join(" ")
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
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let ours = match kind.as_str() {
            "pages" => ours_pages(&bytes),
            "outlines" => ours_outlines(&bytes),
            "labels" => ours_labels(&bytes),
            "attachments" => ours_attachments(&bytes),
            "viewer" => ours_viewer(&bytes),
            other => panic!("{name}: unknown kind {other}"),
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

/// The count PDFium's answer gives.
fn count_of(answer: &str) -> Option<usize> {
    answer
        .strip_prefix("count ")?
        .split(' ')
        .next()?
        .parse()
        .ok()
}

#[test]
fn where_no_page_is_under_the_root_the_count_is_pdfiums() {
    let recorded = recorded();
    let mut wrong = Vec::new();
    let mut none_under = 0;
    for case in rows(&set().join("cases.tsv")) {
        if case[1] != "pages" {
            continue;
        }
        let name = &case[0];
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let Ok(reading) = Reading::open(&bytes[..], &Options::DEFAULT) else {
            continue;
        };
        let objects = &reading;
        let Ok(Some(catalog)) = catalog(objects, reading.trailer().dict()) else {
            continue;
        };
        let walked = pages(objects, catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ());
        let ours = counted(objects, catalog, &Limits::DEFAULT);
        match walked {
            Err(Error::NoPages) => {
                none_under += 1;
                let theirs = recorded.get(name).and_then(|answer| count_of(answer));
                if ours.as_ref().ok() != theirs.as_ref() {
                    wrong.push(format!("{name}: PDFium counts {theirs:?}, here {ours:?}"));
                }
            }
            Ok(table) if ours != Ok(table.counted()) => {
                wrong.push(format!(
                    "{name}: the table counts {}, here {ours:?}",
                    table.counted()
                ));
            }
            _ => {}
        }
    }
    assert!(none_under > 0, "a case with no page under its root");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The cases of trees [`write()`] writes (`cases.py`'s `written`): each its
/// name, its count of pages, objects 2 on, and the fanout it's written under,
/// its nodes numbered from past the last page.
const WRITTEN: [(&str, u32, usize); 3] = [
    ("pages-written-33", 33, FANOUT),
    ("pages-written-64-by-3", 64, 3),
    ("pages-written-1025", 1025, FANOUT),
];

/// `o` as built: every place it was written left out, a dictionary's keys'
/// too.
fn unplaced(o: &Object) -> Object {
    let mut o = o.clone();
    o.unplace();
    o
}

#[test]
fn the_writer_writes_the_trees_pdfium_is_asked_of() {
    let cases = rows(&set().join("cases.tsv"));
    for (name, count, fanout) in WRITTEN {
        assert!(cases.iter().any(|case| case[0] == name), "{name}: no case");
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let document = read::document(&bytes, &read::Options::DEFAULT);
        let pages: Vec<ObjectId> = (2..count + 2).map(|n| ObjectId::new(n, 0)).collect();
        let mut next = count + 2;
        let ids = || {
            next += 1;
            ObjectId::new(next - 1, 0)
        };
        let written = write(&pages, fanout, &Limits::DEFAULT, ids).unwrap();
        let catalog = document.catalog().expect("a catalog");
        let root = catalog.get(b"Pages").and_then(Object::as_reference);
        assert_eq!(root, Some(written.root), "{name}");
        // The catalog, the pages and the nodes, and nothing else.
        let held: std::collections::BTreeMap<ObjectId, &Object> = document.objects().collect();
        assert_eq!(held.len(), 1 + pages.len() + written.nodes.len(), "{name}");
        for (id, node) in &written.nodes {
            let case = held.get(id).unwrap_or_else(|| panic!("{name}: no {id}"));
            assert_eq!(unplaced(case), Object::from(node.clone()), "{name}: {id}");
        }
        for (page, parent) in pages.iter().zip(&written.parents) {
            let case = held.get(page).and_then(|page| page.as_dict()).unwrap();
            let up = case.get(b"Parent").and_then(Object::as_reference);
            assert_eq!(up, Some(*parent), "{name}: {page}");
        }
    }
}
