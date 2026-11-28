//! Holds `pdf-forms`'s `Rules::Pdfium` reading of the field tree to PDFium's
//! answers recorded at `chromium/8152` in `tests/pdfium/chromium-8152.txt`:
//! for each widget of a page, its field's type, flags, fully qualified name,
//! and a text field's value (the `forms` cases). The `forms-ap` cases record
//! the appearance PDFium makes for a text field that has none; their answers
//! are read here into what each widget's appearance paints, clips and shows,
//! and the heights of the faces PDFium draws the standard 14 in are held to
//! them.
//!
//! The `forms-ap-all` cases record every appearance PDFium makes for a
//! button or a choice field that has none, each state's and each slot's, and
//! the `forms-press` cases a button group's states and value after a press;
//! their answers are read here part by part. Each button's and choice's is
//! held to what `appearance`'s makers make for the same widget
//! (`buttons_here`, `choices_here`), and a case of theirs that nothing
//! compares fails `every_case_is_compared`. The presses are made here by
//! `pdf_forms::group`'s rules over what the read layer reads of the group,
//! and held to PDFium's states and values.
//!
//! The `forms-kind` cases record `FPDF_GetFormType`'s number for a document,
//! held to `FormKind::read`'s by PDFium's rules.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;

use pdf_forms::group::FieldGroup;
use pdf_forms::pdf_objects::pdf_tokenizer::Rules;
use pdf_forms::pdf_objects::{Object, Resolve, Str, Value};
use pdf_forms::pdf_semantics::pages;
use pdf_forms::{Field, FieldKind, FieldValue, Form, FormKind, Limits};
use pdf_structure::read::Recovery;
use pdf_structure::{Reading, read};

fn set() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/pdfium")
}

fn read_tsv<const N: usize>(name: &str) -> Vec<[String; N]> {
    fs::read_to_string(set().join(name))
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let cols: Vec<String> = l.splitn(N, '\t').map(str::to_owned).collect();
            cols.try_into().unwrap()
        })
        .collect()
}

/// Bytes as one word, as `oracle/pdfium.py`'s `esc` writes them: printable
/// ASCII as it is but `%`, any other byte `%XX`, none `%` alone, and past 64
/// bytes (its `LONG`) `#`, the length, `:` and the bytes' FNV-1a 64 hash.
fn esc(data: &[u8]) -> String {
    if data.is_empty() {
        return "%".to_string();
    }
    if data.len() > 64 {
        let hash = data.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
        });
        return format!("#{}:{hash:016x}", data.len());
    }
    let mut out = String::with_capacity(data.len());
    for &b in data {
        if (0x21..=0x7E).contains(&b) && b != b'%' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// PDFium's `FPDF_FORMFIELD_*` number for a field.
fn type_number(field: &Field<'_>) -> u32 {
    match field.kind() {
        None => 0,
        Some(FieldKind::PushButton) => 1,
        Some(FieldKind::CheckBox) => 2,
        Some(FieldKind::Radio) => 3,
        Some(FieldKind::Combo) => 4,
        Some(FieldKind::List) => 5,
        Some(FieldKind::Text) => 6,
        Some(FieldKind::Signature) => 7,
        Some(_) => unreachable!("a kind PDFium has no number for"),
    }
}

fn forms_answer(bytes: &[u8]) -> String {
    let mut opts = read::Options::DEFAULT;
    opts.recovery = Recovery::Pdfium;
    let Ok(reading) = Reading::open(bytes, &opts) else {
        return "no-page".to_string();
    };
    let objects = &reading;
    let trailer = reading.trailer().dict();
    let Ok(Some(catalog)) = pdf_forms::pdf_semantics::catalog(objects, trailer) else {
        return "no-page".to_string();
    };
    let Ok(table) = pages::pages(
        objects,
        catalog,
        Rules::Pdfium,
        &pdf_forms::pdf_semantics::Limits::DEFAULT,
        &mut (),
    ) else {
        return "no-page".to_string();
    };
    let Some(page) = table.iter().next().and_then(|page| page.dict) else {
        return "no-page".to_string();
    };
    let form = Form::read(objects, catalog, Rules::Pdfium, &Limits::DEFAULT).unwrap();

    let follow = |object: &Object| match object.as_reference() {
        Some(id) => objects.resolve(id).cloned(),
        None => Some(object.clone()),
    };
    let annots = page.get("Annots").and_then(follow);
    let annots = annots.as_ref().and_then(Object::as_array).unwrap_or(&[]);

    let mut out = Vec::new();
    for (index, entry) in annots.iter().enumerate() {
        if follow(entry).is_none_or(|annot| annot.as_dict().is_none()) {
            continue;
        }
        // The field whose widget this annotation is, by its object number.
        let found = form.as_ref().and_then(|form| {
            let id = entry.as_reference()?;
            form.fields()
                .iter()
                .enumerate()
                .find(|(_, field)| field.widgets().iter().any(|widget| widget.id() == Some(id)))
        });
        let Some((at, field)) = found else {
            out.push(format!("idx {index} none"));
            continue;
        };
        let name = form.as_ref().unwrap().full_name(at).unwrap();
        let mut words = format!(
            "idx {index} type {} flags {} name {}",
            type_number(field),
            field.flags().bits(),
            esc(name.as_bytes())
        );
        if field.kind() == Some(FieldKind::Text) {
            let value = match field.value() {
                Some(FieldValue::Text(text)) => text.as_str(),
                _ => "",
            };
            words.push_str(&format!(" value {}", esc(value.as_bytes())));
        }
        out.push(words);
    }
    if out.is_empty() {
        "none".to_string()
    } else {
        out.join(" ; ")
    }
}

/// `differ.tsv`'s lines, each cut at its tabs: a `forms` case's name and
/// why; a `forms-ap` case's name, the part of its record that differs, and
/// why, or the kind's name for a difference that is a rule.
fn differ_lines() -> Vec<Vec<String>> {
    fs::read_to_string(set().join("differ.tsv"))
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').map(str::to_owned).collect())
        .collect()
}

#[test]
fn pdfium_recorded_cases_agree() {
    let recorded: HashMap<String, String> = read_tsv::<2>("chromium-8152.txt")
        .into_iter()
        .map(|[k, v]| (k, v))
        .collect();
    let cases = read_tsv::<3>("cases.tsv");
    assert!(!cases.is_empty(), "cases.tsv must not be empty");
    let lines = differ_lines();
    for line in &lines {
        assert!(
            RULE_KINDS.contains(&line[0].as_str()) || recorded.contains_key(&line[0]),
            "differ.tsv names unknown case {}",
            line[0]
        );
    }
    let differs: HashMap<String, String> = lines
        .into_iter()
        .filter(|line| {
            cases
                .iter()
                .any(|[name, kind, _]| *name == line[0] && kind == "forms")
        })
        .map(|line| {
            assert_eq!(line.len(), 2, "a forms case's differ.tsv line: {line:?}");
            (line[0].clone(), line[1].clone())
        })
        .collect();

    let mut wrong = Vec::new();
    for [name, kind, _files] in &cases {
        assert!(
            kind == "forms"
                || kind == "forms-kind"
                || RULE_KINDS.contains(&kind.as_str())
                || (kind == "fonts" && name.starts_with("back-")),
            "{name}: unexpected case kind {kind}"
        );
        if kind != "forms" {
            continue;
        }
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let ours = forms_answer(&bytes);
        let pdfium = recorded
            .get(name)
            .unwrap_or_else(|| panic!("{name}: no recorded answer"));
        let matches = ours.split_whitespace().eq(pdfium.split_whitespace());
        if differs.contains_key(name) {
            if matches {
                wrong.push(format!(
                    "{name} is listed in differ.tsv but now matches PDFium: {ours}"
                ));
            }
        } else if !matches {
            wrong.push(format!("{name}:\n  pdfium: {pdfium}\n  ours:   {ours}"));
        }
    }

    assert!(
        wrong.is_empty(),
        "{} case(s) disagreed with chromium/8152:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// The kind of form a case's document holds, as a `forms-kind` record gives
/// it: `FPDF_GetFormType`'s number for what `FormKind::read` reads by `rules`.
fn kind_answer(bytes: &[u8], rules: Rules) -> String {
    let mut opts = read::Options::DEFAULT;
    opts.recovery = Recovery::Pdfium;
    let Ok(reading) = Reading::open(bytes, &opts) else {
        return "no-document".to_string();
    };
    let trailer = reading.trailer().dict();
    let Ok(Some(catalog)) = pdf_forms::pdf_semantics::catalog(&reading, trailer) else {
        return "no-document".to_string();
    };
    let number = match FormKind::read(&reading, catalog, rules, &Limits::DEFAULT).unwrap() {
        FormKind::None => 0,
        FormKind::AcroForm => 1,
        FormKind::XfaFull => 2,
        FormKind::XfaForeground => 3,
        other => unreachable!("{other:?}: a kind PDFium has no number for"),
    };
    format!("kind {number}")
}

/// Every `forms-kind` case reads, by PDFium's rules, as the kind PDFium says;
/// and by ISO 32000-2's the same, but the cases named here, where an `/XFA`
/// that is `null` or neither a stream nor an array is no entry, and
/// `/NeedsRendering` is read through a reference.
#[test]
fn pdfium_recorded_form_kinds_agree() {
    const ISO_DIFFERS: &[(&str, &str)] = &[
        ("kind-xfa-a-number", "kind 1"),
        ("kind-xfa-null", "kind 1"),
        ("kind-xfa-null-needs-rendering", "kind 1"),
        ("kind-xfa-missing", "kind 1"),
        ("kind-xfa-missing-needs-rendering", "kind 1"),
        ("kind-xfa-by-reference-to-null", "kind 1"),
        ("kind-xfa-needs-rendering-by-reference", "kind 2"),
    ];
    let recorded: HashMap<String, String> = read_tsv::<2>("chromium-8152.txt")
        .into_iter()
        .map(|[k, v]| (k, v))
        .collect();
    let mut wrong = Vec::new();
    let mut seen = BTreeSet::new();
    let mut differed = 0;
    for [name, kind, _] in &read_tsv::<3>("cases.tsv") {
        if kind != "forms-kind" {
            continue;
        }
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let pdfium = recorded
            .get(name)
            .unwrap_or_else(|| panic!("{name}: no recorded answer"));
        seen.insert(pdfium.clone());
        let ours = kind_answer(&bytes, Rules::Pdfium);
        if ours != *pdfium {
            wrong.push(format!("{name}:\n  pdfium: {pdfium}\n  ours:   {ours}"));
        }
        let iso = kind_answer(&bytes, Rules::Iso);
        match ISO_DIFFERS.iter().find(|(case, _)| case == name) {
            Some((_, expected)) => {
                differed += 1;
                if iso != *expected || iso == *pdfium {
                    wrong.push(format!("{name} by ISO's rules: {iso}, not {expected}"));
                }
            }
            None if iso != *pdfium => {
                wrong.push(format!("{name} by ISO's rules: {iso}, PDFium's {pdfium}"));
            }
            None => {}
        }
    }
    assert!(
        wrong.is_empty(),
        "{} case(s) disagreed with chromium/8152:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
    // Each kind PDFium names is among the cases, and each listed difference a case.
    let kinds: Vec<&str> = seen.iter().map(String::as_str).collect();
    assert_eq!(kinds, ["kind 0", "kind 1", "kind 2", "kind 3"]);
    assert_eq!(differed, ISO_DIFFERS.len());
}

/// A color as a `forms-ap` record gives it: the operator that set it (`g`,
/// `rg`, `k`, or `G`, `RG`, `K` for a stroke) and its components.
#[derive(Debug, Clone, PartialEq)]
struct Color {
    op: String,
    components: Vec<f32>,
}

/// How a path of a made appearance is painted.
#[derive(Debug, Clone, PartialEq)]
enum Paint {
    Fill,
    EvenOddFill,
    /// Its line width, cap, join, dash array and phase.
    Stroke {
        width: f32,
        cap: u8,
        join: u8,
        dash: Vec<f32>,
        phase: f32,
    },
}

/// A path's segment: `re` with its four numbers, `m`, `l`, `c`, `h`.
#[derive(Debug, Clone, PartialEq)]
enum Segment {
    Rect([f32; 4]),
    Move([f32; 2]),
    Line([f32; 2]),
    Curve([f32; 6]),
    Close,
}

/// A path painted before the appearance's `/Tx` sequence.
#[derive(Debug, Clone, PartialEq)]
struct Painted {
    paint: Paint,
    color: Color,
    segments: Vec<Segment>,
}

/// A `Tj` in the `/Tx` sequence: its font's resource name (`%` where no `Tf`
/// came first), size, origin, and codes as `esc` writes them.
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    font: String,
    size: f32,
    at: [f32; 2],
    codes: String,
}

/// What the `/Tx` sequence holds: its clip rectangle (none where it has none),
/// and the color of its text with each string shown (none where it shows none).
#[derive(Debug, Clone, PartialEq)]
struct Tx {
    clip: Option<[f32; 4]>,
    text: Option<(Color, Vec<Shown>)>,
}

/// A made appearance, in the record's five parts.
#[derive(Debug, Clone, PartialEq)]
struct Made {
    bbox: [f32; 4],
    matrix: [f32; 6],
    paint: Vec<Painted>,
    /// None where the appearance has no `/Tx` sequence (`tx none`).
    tx: Option<Tx>,
}

/// A record's words, read in the order `oracle/pdfium.py`'s `ap_record` and
/// `forms_ap` write them; a word out of place panics, naming the record.
struct Words<'a> {
    words: std::iter::Peekable<std::str::SplitWhitespace<'a>>,
    record: &'a str,
}

impl<'a> Words<'a> {
    fn new(record: &'a str) -> Self {
        Self {
            words: record.split_whitespace().peekable(),
            record,
        }
    }

    fn word(&mut self) -> &'a str {
        self.words
            .next()
            .unwrap_or_else(|| panic!("a record ends early: {}", self.record))
    }

    fn expect(&mut self, keyword: &str) {
        let word = self.word();
        assert_eq!(word, keyword, "in {}", self.record);
    }

    fn eat(&mut self, keyword: &str) -> bool {
        let found = self.words.peek() == Some(&keyword);
        if found {
            self.words.next();
        }
        found
    }

    fn peek(&mut self) -> Option<&'a str> {
        self.words.peek().copied()
    }

    fn num(&mut self) -> f32 {
        let word = self.word();
        word.parse()
            .unwrap_or_else(|_| panic!("{word} isn't a number in {}", self.record))
    }

    fn nums<const N: usize>(&mut self) -> [f32; N] {
        std::array::from_fn(|_| self.num())
    }

    fn color(&mut self) -> Color {
        let op = self.word().to_string();
        let count = match op.as_str() {
            "g" | "G" => 1,
            "rg" | "RG" => 3,
            "k" | "K" => 4,
            _ => panic!("{op} isn't a color operator in {}", self.record),
        };
        let components = (0..count).map(|_| self.num()).collect();
        Color { op, components }
    }

    fn painted(&mut self, paint: &str) -> Painted {
        let color = self.color();
        let paint = match paint {
            "fill" => Paint::Fill,
            "eofill" => Paint::EvenOddFill,
            _ => {
                self.expect("w");
                let width = self.num();
                self.expect("cap");
                let cap = self.word().parse().unwrap();
                self.expect("join");
                let join = self.word().parse().unwrap();
                self.expect("dash");
                let mut dash = Vec::new();
                if !self.eat("none") {
                    while self.peek() != Some("phase") {
                        dash.push(self.num());
                    }
                }
                self.expect("phase");
                Paint::Stroke {
                    width,
                    cap,
                    join,
                    dash,
                    phase: self.num(),
                }
            }
        };
        let mut segments = Vec::new();
        loop {
            let segment = match self.peek() {
                Some("re") => Segment::Rect(self.after_word()),
                Some("m") => Segment::Move(self.after_word()),
                Some("l") => Segment::Line(self.after_word()),
                Some("c") => Segment::Curve(self.after_word()),
                Some("h") => {
                    self.word();
                    Segment::Close
                }
                _ => break,
            };
            segments.push(segment);
        }
        Painted {
            paint,
            color,
            segments,
        }
    }

    fn after_word<const N: usize>(&mut self) -> [f32; N] {
        self.word();
        self.nums()
    }

    fn tx(&mut self) -> Option<Tx> {
        if self.eat("tx") {
            self.expect("none");
            return None;
        }
        self.expect("clip");
        let clip = if self.eat("none") {
            None
        } else {
            Some(self.nums())
        };
        if self.eat("text") {
            self.expect("none");
            return Some(Tx { clip, text: None });
        }
        self.expect("color");
        let color = self.color();
        let mut shown = Vec::new();
        while self.eat("font") {
            let font = self.word().to_string();
            self.expect("size");
            let size = self.num();
            self.expect("at");
            let at = self.nums();
            self.expect("codes");
            let codes = self.word().to_string();
            shown.push(Shown {
                font,
                size,
                at,
                codes,
            });
        }
        assert!(
            !shown.is_empty(),
            "text with nothing shown: {}",
            self.record
        );
        Some(Tx {
            clip,
            text: Some((color, shown)),
        })
    }
}

/// A `forms-ap` answer: for each annotation of the page in order, the
/// appearance PDFium made for it, or none.
fn made_appearances(answer: &str) -> Vec<Option<Made>> {
    answer
        .split(" ; ")
        .enumerate()
        .map(|(index, record)| {
            let mut words = Words::new(record);
            words.expect("idx");
            assert_eq!(words.word(), index.to_string(), "in {record}");
            if words.eat("none") {
                assert!(words.peek().is_none(), "in {record}");
                return None;
            }
            words.expect("bbox");
            let bbox = words.nums();
            words.expect("matrix");
            let matrix = words.nums();
            words.expect("paint");
            let mut paint = Vec::new();
            if !words.eat("none") {
                while let Some(kind @ ("fill" | "eofill" | "stroke")) = words.peek() {
                    words.word();
                    paint.push(words.painted(kind));
                }
            }
            let tx = words.tx();
            assert!(
                words.peek().is_none(),
                "words past the record's end: {record}"
            );
            Some(Made {
                bbox,
                matrix,
                paint,
                tx,
            })
        })
        .collect()
}

/// Each `forms-ap` case's answer reads as a record for every widget of its
/// file, and in each made appearance what the recorder holds holds: the box
/// at the origin, a quarter turn or none as its matrix, colors with their
/// components, every path with a segment, a stroke's style in range, text
/// at a size with codes. Each case has one appearance at least, with its
/// box, its matrix and its `/Tx` part.
#[test]
fn forms_ap_answers_read() {
    let recorded: HashMap<String, String> = read_tsv::<2>("chromium-8152.txt")
        .into_iter()
        .map(|[k, v]| (k, v))
        .collect();
    let cases = read_tsv::<3>("cases.tsv");
    let mut read = 0;
    for [name, kind, _files] in cases.iter().filter(|[_, kind, _]| kind == "forms-ap") {
        assert!(name.starts_with("ap-"), "{name}: a {kind} case's name");
        let answer = recorded
            .get(name)
            .unwrap_or_else(|| panic!("{name}: no recorded answer"));
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let widgets = bytes
            .windows(16)
            .filter(|w| w == b"/Subtype /Widget")
            .count();
        let made = made_appearances(answer);
        assert_eq!(made.len(), widgets, "{name}: a record for each widget");
        assert!(made.iter().any(Option::is_some), "{name}: nothing made");
        for appearance in made.iter().flatten() {
            let [x0, y0, x1, y1] = appearance.bbox;
            assert!(x0 == 0.0 && y0 == 0.0 && x1 >= 0.0 && y1 >= 0.0, "{name}");
            let [a, b, c, d, e, f] = appearance.matrix;
            let turns = [
                [1.0, 0.0, 0.0, 1.0],
                [0.0, 1.0, -1.0, 0.0],
                [-1.0, 0.0, 0.0, -1.0],
                [0.0, -1.0, 1.0, 0.0],
            ];
            assert!(turns.contains(&[a, b, c, d]), "{name}: matrix");
            assert!(
                [e, f].iter().all(|v| [0.0, x1, y1].contains(v)),
                "{name}: matrix"
            );
            for painted in &appearance.paint {
                let stroke = matches!(painted.paint, Paint::Stroke { .. });
                assert_eq!(
                    painted.color.op.chars().all(char::is_uppercase),
                    stroke,
                    "{name}"
                );
                assert!(!painted.segments.is_empty(), "{name}");
                if let Paint::Stroke {
                    width,
                    cap,
                    join,
                    ref dash,
                    phase,
                } = painted.paint
                {
                    assert!(
                        width >= 0.0 && cap <= 2 && join <= 2 && phase >= 0.0,
                        "{name}"
                    );
                    assert!(dash.iter().all(|v| *v >= 0.0), "{name}");
                }
                let corners = painted.segments.iter().map(|segment| match *segment {
                    Segment::Rect([x, y, w, h]) => {
                        x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()
                    }
                    Segment::Move(p) | Segment::Line(p) => p.iter().all(|v| v.is_finite()),
                    Segment::Curve(p) => p.iter().all(|v| v.is_finite()),
                    Segment::Close => true,
                });
                assert!(corners.into_iter().all(|ok| ok), "{name}");
            }
            let Some(tx) = &appearance.tx else { continue };
            if let Some(clip) = tx.clip {
                assert!(clip.iter().all(|v| v.is_finite()), "{name}: clip");
            }
            if let Some((color, shown)) = &tx.text {
                assert!(color.op.chars().all(char::is_lowercase), "{name}");
                assert!(
                    color.components.iter().all(|v| (0.0..=1.0).contains(v)),
                    "{name}"
                );
                for run in shown {
                    assert!(
                        run.size >= 0.0 && run.at.iter().all(|v| v.is_finite()),
                        "{name}"
                    );
                    assert!(
                        !run.font.is_empty() && run.codes != "%",
                        "{name}: {}",
                        run.codes
                    );
                }
            }
        }
        read += 1;
    }
    assert!(read > 0, "no forms-ap case");
}

/// The reader takes each part as the recorder writes it, and nothing else.
#[test]
fn forms_ap_records_are_read_by_part() {
    let made = made_appearances(
        "idx 0 bbox 0 0 100 20 matrix 0 1 -1 0 20 0 paint fill rg 1 1 0 re 0 0 100 20 stroke RG 0 0 1 \
         w 2 cap 0 join 0 dash 3 3 phase 0 m 1 1 l 1 19 h clip 1 1 98 18 color k 0 0 0 1 font Helv \
         size 12 at 2 5.5 codes Hi font % size 0 at 0 0 codes %2A ; idx 1 none ; idx 2 bbox 0 0 4 4 \
         matrix 1 0 0 1 0 0 paint none tx none",
    );
    assert_eq!(made.len(), 3);
    let first = made[0].as_ref().unwrap();
    assert_eq!(first.matrix, [0.0, 1.0, -1.0, 0.0, 20.0, 0.0]);
    assert_eq!(first.paint.len(), 2);
    assert_eq!(
        first.paint[0].segments,
        [Segment::Rect([0.0, 0.0, 100.0, 20.0])]
    );
    assert_eq!(
        first.paint[1].paint,
        Paint::Stroke {
            width: 2.0,
            cap: 0,
            join: 0,
            dash: vec![3.0, 3.0],
            phase: 0.0
        }
    );
    assert_eq!(
        first.paint[1].segments,
        [
            Segment::Move([1.0, 1.0]),
            Segment::Line([1.0, 19.0]),
            Segment::Close
        ]
    );
    let tx = first.tx.as_ref().unwrap();
    assert_eq!(tx.clip, Some([1.0, 1.0, 98.0, 18.0]));
    let (color, shown) = tx.text.as_ref().unwrap();
    assert_eq!(color.components, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(shown[1].font, "%");
    assert!(made[1].is_none());
    assert_eq!(made[2].as_ref().unwrap().tx, None);
    for wrong in [
        "idx 0 bbox 0 0 1 1 matrix 1 0 0 1 0 0 paint none",
        "idx 0 bbox 0 0 1 1 matrix 1 0 0 1 0 0 paint none tx none extra",
        "idx 1 none",
        "idx 0 bbox 0 0 1 1 matrix 1 0 0 1 0 0 paint fill q 1 re 0 0 1 1 tx none",
        "idx 0 bbox 0 0 1 1 matrix 1 0 0 1 0 0 paint none clip none color g 0",
    ] {
        assert!(
            std::panic::catch_unwind(|| made_appearances(wrong)).is_err(),
            "read: {wrong}"
        );
    }
}

/// `esc` writes a long string as the oracle does, by its length and hash.
#[test]
fn esc_writes_long_strings_by_their_hash() {
    assert_eq!(esc(b""), "%");
    assert_eq!(esc(b"a b%"), "a%20b%25");
    assert_eq!(esc(&[b'a'; 64]), "a".repeat(64));
    // FNV-1a 64 of 65 bytes of `a`, as `oracle/pdfium.py`'s `fnv` gives it.
    assert_eq!(esc(&[b'a'; 65]), "#65:8087e9a889f8a14c");
}

/// Each standard 14 face's heights, as a field's `/DR` font gives them to its
/// layout, are the ones PDFium's appearances at 100 points show (the
/// `ap-face-*` cases: boxes 400 by 300 with no border). A single line's
/// baseline is the box's middle less half their sum, a multiline field's
/// first its top less the ascent.
#[cfg(feature = "text-layout")]
#[test]
fn standard_faces_heights_are_the_recorded_ones() {
    use pdf_forms::pdf_fonts::Std14;
    use pdf_forms::pdf_gfx::{DefaultAppearance, LayoutFont};

    let recorded: HashMap<String, String> = read_tsv::<2>("chromium-8152.txt")
        .into_iter()
        .map(|[k, v]| (k, v))
        .collect();
    let mut held = Vec::new();
    for [name, _, _] in read_tsv::<3>("cases.tsv")
        .iter()
        .filter(|[name, _, _]| name.starts_with("ap-face-"))
    {
        let face = Std14::ALL
            .into_iter()
            .find(|face| *name == format!("ap-face-{}", face.name().to_ascii_lowercase()))
            .unwrap_or_else(|| panic!("{name}: no standard face of that name"));
        let made = made_appearances(&recorded[name]);
        let baseline = |widget: usize| {
            let tx = made[widget].as_ref().and_then(|made| made.tx.as_ref());
            let (_, shown) = tx.and_then(|tx| tx.text.as_ref()).unwrap();
            assert_eq!((shown.len(), shown[0].size), (1, 100.0), "{name}");
            shown[0].at[1]
        };
        let ascent = (300.0 - baseline(1)) * 10.0;
        let sum = (150.0 - baseline(0)) * 20.0;

        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let mut opts = read::Options::DEFAULT;
        opts.recovery = Recovery::Pdfium;
        let reading = Reading::open(&bytes, &opts).unwrap();
        let catalog = pdf_forms::pdf_semantics::catalog(&reading, reading.trailer().dict())
            .unwrap()
            .unwrap();
        let form = Form::read(&reading, catalog, Rules::Pdfium, &Limits::DEFAULT)
            .unwrap()
            .unwrap();
        let da = form.fields()[0].default_appearance().unwrap();
        let da = DefaultAppearance::parse(da, &pdf_forms::pdf_content_streams::Limits::DEFAULT);
        let font_name = da.and_then(|da| da.font_name).unwrap();
        let mut told: Vec<(pdf_forms::pdf_objects::fault::Place, pdf_forms::Fault)> = Vec::new();
        let font = form
            .field_font(&reading, &font_name, &Limits::DEFAULT, &mut told)
            .unwrap();
        assert!(!font.is_fallback() && told.is_empty(), "{name}: {told:?}");
        let metrics = font.layout().font_metrics();
        assert!(
            (metrics.ascent - ascent).abs() < 0.5
                && (metrics.ascent + metrics.descent - sum).abs() < 0.5,
            "{name}: ours {} and {}, PDFium's {ascent} and {}",
            metrics.ascent,
            metrics.descent,
            sum - ascent
        );
        held.push(face);
    }
    assert_eq!(held.len(), Std14::ALL.len(), "a case for each face");
}

/// The kinds of case besides `forms`: `forms-ap`, a text field's appearance;
/// `forms-ap-all`, every appearance of a button or a choice field;
/// `forms-press`, a button group's states after a press. A `differ.tsv` line
/// under one's name is a rule held on every case of it that it applies to.
const RULE_KINDS: &[&str] = &["forms-ap", "forms-ap-all", "forms-press"];

/// The cases of a kind that holds more than one maker's that a test
/// compares to what the crate makes, by kind and the start of their names:
/// `buttons_here`'s and `choices_here`'s.
const COMPARED: &[(&str, &str)] = &[
    ("forms-ap-all", "btn-"),
    ("forms-ap-all", "push-"),
    ("forms-ap-all", "combo-"),
    ("forms-ap-all", "list-"),
];

/// What a `forms-ap-all` record gives of an appearance's content, in the
/// order the content has it: a path painted, a clip set and where it ends,
/// where the `/Tx` sequence begins and ends, and each string shown with its
/// own color.
#[derive(Debug, Clone, PartialEq)]
enum Item {
    Painted(Painted),
    Clip(Vec<Segment>),
    EndClip,
    TxBegin,
    TxEnd,
    Text { color: Color, shown: Shown },
}

/// An appearance stream: its box, its matrix and its items.
#[derive(Debug, Clone, PartialEq)]
struct Drawn {
    bbox: [f32; 4],
    matrix: [f32; 6],
    items: Vec<Item>,
}

/// One appearance of a widget: its slot in `/AP` (`N`, `D` or `R`), its
/// state's name as `esc` writes it where the slot is a state dictionary,
/// and the stream, none where the entry is no stream.
#[derive(Debug, Clone, PartialEq)]
struct Appearance {
    slot: String,
    state: Option<String>,
    drawn: Option<Drawn>,
}

/// A widget's `forms-ap-all` record: its `/AS` as the oracle writes a value
/// (`/` and a name, `(` a string `)`, `none`, `other`), and its appearances,
/// none where it has no `/AP`.
#[derive(Debug, Clone, PartialEq)]
struct WidgetRecord {
    state: String,
    appearances: Option<Vec<Appearance>>,
}

impl Words<'_> {
    fn segments(&mut self) -> Vec<Segment> {
        let mut segments = Vec::new();
        loop {
            let segment = match self.peek() {
                Some("re") => Segment::Rect(self.after_word()),
                Some("m") => Segment::Move(self.after_word()),
                Some("l") => Segment::Line(self.after_word()),
                Some("c") => Segment::Curve(self.after_word()),
                Some("h") => {
                    self.word();
                    Segment::Close
                }
                _ => return segments,
            };
            segments.push(segment);
        }
    }

    /// The items of a stream, up to the record's next appearance or its end.
    fn items(&mut self) -> Vec<Item> {
        let mut items = Vec::new();
        if self.eat("none") {
            return items;
        }
        while let Some(word) = self.peek().filter(|word| *word != "ap") {
            self.word();
            let item = match word {
                "fill" | "eofill" | "stroke" => Item::Painted(self.painted(word)),
                "clip" => Item::Clip(self.segments()),
                "]clip" => Item::EndClip,
                "tx[" => Item::TxBegin,
                "]tx" => Item::TxEnd,
                "text" => {
                    let color = self.color();
                    self.expect("font");
                    let font = self.word().to_string();
                    self.expect("size");
                    let size = self.num();
                    self.expect("at");
                    let at = self.nums();
                    self.expect("codes");
                    let codes = self.word().to_string();
                    Item::Text {
                        color,
                        shown: Shown {
                            font,
                            size,
                            at,
                            codes,
                        },
                    }
                }
                _ => panic!("{word} isn't an item in {}", self.record),
            };
            items.push(item);
        }
        assert!(!items.is_empty(), "items with none: {}", self.record);
        items
    }
}

/// A `forms-ap-all` answer: for each annotation of the page in order, its
/// `/AS` and every appearance PDFium left it with.
fn widget_records(answer: &str) -> Vec<WidgetRecord> {
    answer
        .split(" ; ")
        .enumerate()
        .map(|(index, record)| {
            let mut words = Words::new(record);
            words.expect("idx");
            assert_eq!(words.word(), index.to_string(), "in {record}");
            words.expect("as");
            let state = words.word().to_string();
            words.expect("ap");
            if words.eat("none") {
                assert!(words.peek().is_none(), "in {record}");
                return WidgetRecord {
                    state,
                    appearances: None,
                };
            }
            let mut appearances = Vec::new();
            loop {
                let key = words.word();
                let (slot, state) = match key.split_once('/') {
                    Some((slot, state)) => (slot, Some(state.to_string())),
                    None => (key, None),
                };
                assert!(["N", "D", "R"].contains(&slot), "{key} in {record}");
                let drawn = if words.eat("none") {
                    None
                } else {
                    words.expect("bbox");
                    let bbox = words.nums();
                    words.expect("matrix");
                    let matrix = words.nums();
                    words.expect("items");
                    let items = words.items();
                    Some(Drawn {
                        bbox,
                        matrix,
                        items,
                    })
                };
                appearances.push(Appearance {
                    slot: slot.to_string(),
                    state,
                    drawn,
                });
                if !words.eat("ap") {
                    break;
                }
            }
            assert!(
                words.peek().is_none(),
                "words past the record's end: {record}"
            );
            WidgetRecord {
                state,
                appearances: Some(appearances),
            }
        })
        .collect()
}

/// A `forms-press` answer: each widget's `/AS`, then each field's full name
/// and `/V`, as the oracle writes a value.
fn pressed_records(answer: &str) -> (Vec<String>, Vec<(String, String)>) {
    let (mut states, mut fields) = (Vec::new(), Vec::new());
    for record in answer.split(" ; ") {
        let mut words = Words::new(record);
        if words.eat("idx") {
            assert!(fields.is_empty(), "a widget after the fields: {answer}");
            assert_eq!(words.word(), states.len().to_string(), "in {record}");
            words.expect("as");
            states.push(words.word().to_string());
        } else {
            words.expect("field");
            let name = words.word().to_string();
            words.expect("v");
            fields.push((name, words.word().to_string()));
        }
        assert!(
            words.peek().is_none(),
            "words past the record's end: {record}"
        );
    }
    (states, fields)
}

/// A value as the oracle writes `/AS` and `/V`: a name, a string, none or
/// another kind of value.
fn is_value_word(word: &str) -> bool {
    word == "none"
        || word == "other"
        || word.len() > 1 && word.starts_with('/')
        || word.len() > 2 && word.starts_with('(') && word.ends_with(')')
}

/// Each `forms-ap-all` case's answer reads as a record for every widget of
/// its file, a value for its `/AS` and every appearance with a stream: the
/// box at the origin, a quarter turn or none as its matrix, colors with
/// their components, every path with a segment, a stroke's style in range,
/// each clip ended and the `/Tx` sequence closed, text at a size with codes.
/// A button's normal appearance is a state dictionary with an on state and
/// `Off`; a push button's and a choice's is a stream.
#[test]
fn forms_ap_all_answers_read() {
    let recorded: HashMap<String, String> = read_tsv::<2>("chromium-8152.txt")
        .into_iter()
        .map(|[k, v]| (k, v))
        .collect();
    let cases = read_tsv::<3>("cases.tsv");
    let mut read = 0;
    for [name, kind, _files] in cases.iter().filter(|[_, kind, _]| kind == "forms-ap-all") {
        let toggle = name.starts_with("btn-");
        assert!(
            ["btn-", "push-", "combo-", "list-"]
                .iter()
                .any(|start| name.starts_with(start)),
            "{name}: a {kind} case's name"
        );
        let answer = recorded
            .get(name)
            .unwrap_or_else(|| panic!("{name}: no recorded answer"));
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let widgets = bytes
            .windows(16)
            .filter(|w| w == b"/Subtype /Widget")
            .count();
        let records = widget_records(answer);
        assert_eq!(records.len(), widgets, "{name}: a record for each widget");
        for record in &records {
            assert!(is_value_word(&record.state), "{name}: {}", record.state);
            let appearances = record
                .appearances
                .as_ref()
                .unwrap_or_else(|| panic!("{name}: nothing made"));
            let normal: Vec<_> = appearances.iter().filter(|a| a.slot == "N").collect();
            if toggle {
                assert!(
                    normal.iter().all(|a| a.state.is_some())
                        && normal.iter().any(|a| a.state.as_deref() == Some("Off"))
                        && normal.iter().any(|a| a.state.as_deref() != Some("Off")),
                    "{name}: a button's normal appearances"
                );
            } else {
                assert!(
                    normal.len() == 1 && normal[0].state.is_none(),
                    "{name}: one normal appearance"
                );
            }
            for drawn in appearances.iter().filter_map(|a| a.drawn.as_ref()) {
                let [x0, y0, x1, y1] = drawn.bbox;
                assert!(x0 == 0.0 && y0 == 0.0 && x1 >= 0.0 && y1 >= 0.0, "{name}");
                let [a, b, c, d, _, _] = drawn.matrix;
                assert!(
                    [a, b, c, d].iter().all(|v| [0.0, 1.0, -1.0].contains(v)),
                    "{name}: matrix"
                );
                let (mut clips, mut tx) = (0, 0);
                for item in &drawn.items {
                    match item {
                        Item::Painted(painted) => {
                            let stroke = matches!(painted.paint, Paint::Stroke { .. });
                            assert_eq!(
                                painted.color.op.chars().all(char::is_uppercase),
                                stroke,
                                "{name}"
                            );
                            assert!(!painted.segments.is_empty(), "{name}");
                            if let Paint::Stroke {
                                width, cap, join, ..
                            } = painted.paint
                            {
                                assert!(width >= 0.0 && cap <= 2 && join <= 2, "{name}");
                            }
                        }
                        Item::Clip(segments) => {
                            assert!(!segments.is_empty(), "{name}: a clip of nothing");
                            clips += 1;
                        }
                        Item::EndClip => {
                            assert!(clips > 0, "{name}: a clip ended unset");
                            clips -= 1;
                        }
                        Item::TxBegin => {
                            assert_eq!(tx, 0, "{name}: /Tx within /Tx");
                            tx += 1;
                        }
                        Item::TxEnd => {
                            assert_eq!(tx, 1, "{name}: /Tx ended unbegun");
                            tx -= 1;
                        }
                        Item::Text { color, shown } => {
                            assert!(color.op.chars().all(char::is_lowercase), "{name}");
                            assert!(
                                shown.size >= 0.0 && shown.codes != "%" && !shown.font.is_empty(),
                                "{name}"
                            );
                        }
                    }
                }
                assert_eq!(tx, 0, "{name}: /Tx left open");
            }
        }
        read += 1;
    }
    assert!(read > 0, "no forms-ap-all case");
}

/// Each `forms-press` case's answer reads as each widget's `/AS` and each
/// field's `/V`, and the widget its `.press` file names is one of the page's.
#[test]
fn forms_press_answers_read() {
    let recorded: HashMap<String, String> = read_tsv::<2>("chromium-8152.txt")
        .into_iter()
        .map(|[k, v]| (k, v))
        .collect();
    let cases = read_tsv::<3>("cases.tsv");
    let mut read = 0;
    for [name, kind, files] in cases.iter().filter(|[_, kind, _]| kind == "forms-press") {
        assert!(name.starts_with("press-"), "{name}: a {kind} case's name");
        assert_eq!(
            *files,
            format!("{name}.pdf,{name}.press"),
            "{name}: its files"
        );
        let (states, fields) = pressed_records(&recorded[name]);
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let widgets = bytes
            .windows(16)
            .filter(|w| w == b"/Subtype /Widget")
            .count();
        assert_eq!(states.len(), widgets, "{name}: a state for each widget");
        assert!(!fields.is_empty(), "{name}: no field");
        assert!(
            states
                .iter()
                .chain(fields.iter().map(|(_, v)| v))
                .all(|v| is_value_word(v)),
            "{name}"
        );
        let presses =
            fs::read_to_string(set().join("cases").join(format!("{name}.press"))).unwrap();
        let presses: Vec<usize> = presses
            .split_whitespace()
            .map(|p| p.parse().unwrap())
            .collect();
        assert!(
            !presses.is_empty() && presses.iter().all(|p| *p < widgets),
            "{name}: its presses"
        );
        read += 1;
    }
    assert!(read > 0, "no forms-press case");
}

/// Every case is of a kind a test compares to what the crate does (`fonts`,
/// the read-back cases, `written::read_back_cases_agree`'s), or of a kind and
/// a start of its name in `COMPARED`; and each line there has a case.
#[test]
fn every_case_is_compared() {
    let cases = read_tsv::<3>("cases.tsv");
    let mut compared = vec![0; COMPARED.len()];
    for [name, kind, _] in &cases {
        if kind == "forms"
            || kind == "forms-kind"
            || kind == "forms-ap"
            || kind == "forms-press"
            || kind == "fonts"
        {
            continue;
        }
        let line = COMPARED
            .iter()
            .position(|(k, start)| k == kind && name.starts_with(start))
            .unwrap_or_else(|| panic!("{name}: a {kind} case nothing compares"));
        compared[line] += 1;
    }
    for ((kind, start), count) in COMPARED.iter().zip(compared) {
        assert!(count > 0, "no {kind} case named {start}*");
    }
}

/// The readers take each part as the recorder writes it, and nothing else.
#[test]
fn forms_ap_all_and_press_records_are_read_by_part() {
    let records = widget_records(
        "idx 0 as (Off) ap N/Off bbox 0 0 12 12 matrix 1 0 0 1 0 0 items none ap N/Yes bbox 0 0 12 12 \
         matrix 0 1 -1 0 12 0 items fill g 0 m 1 1 c 1 2 3 4 5 6 h stroke RG 1 0 0 w 2 cap 0 join 0 \
         dash none phase 0 m 0 0 l 1 1 ap D/Off none ; idx 1 as none ap none ; idx 2 as /On ap N \
         bbox 0 0 50 20 matrix 1 0 0 1 0 0 items tx[ clip re 1 1 48 18 text g 1 font Helv size 10 at \
         1 6.5 codes Hi ]clip ]tx eofill k 0 0 0 1 re 0 0 50 20 re 1 1 48 18",
    );
    assert_eq!(records.len(), 3);
    let first = records[0].appearances.as_ref().unwrap();
    assert_eq!(records[0].state, "(Off)");
    assert_eq!(first.len(), 3);
    assert_eq!(
        (first[0].slot.as_str(), first[0].state.as_deref()),
        ("N", Some("Off"))
    );
    assert_eq!(first[0].drawn.as_ref().unwrap().items, []);
    let on = first[1].drawn.as_ref().unwrap();
    assert_eq!(on.matrix, [0.0, 1.0, -1.0, 0.0, 12.0, 0.0]);
    assert_eq!(on.items.len(), 2);
    let Item::Painted(stroke) = &on.items[1] else {
        panic!("{:?}", on.items[1])
    };
    assert_eq!(
        stroke.segments,
        [Segment::Move([0.0, 0.0]), Segment::Line([1.0, 1.0])]
    );
    assert_eq!(first[2].drawn, None);
    assert_eq!(records[1].appearances, None);
    let third = records[2].appearances.as_ref().unwrap();
    assert_eq!(
        (third[0].slot.as_str(), third[0].state.as_deref()),
        ("N", None)
    );
    let items = &third[0].drawn.as_ref().unwrap().items;
    assert_eq!(items[0], Item::TxBegin);
    assert_eq!(
        items[1],
        Item::Clip(vec![Segment::Rect([1.0, 1.0, 48.0, 18.0])])
    );
    let Item::Text { color, shown } = &items[2] else {
        panic!("{:?}", items[2])
    };
    assert_eq!(
        (color.components.as_slice(), shown.at),
        ([1.0].as_slice(), [1.0, 6.5])
    );
    assert_eq!(&items[3..5], [Item::EndClip, Item::TxEnd]);
    for wrong in [
        "idx 0 as (Off) ap N bbox 0 0 1 1 matrix 1 0 0 1 0 0",
        "idx 0 as (Off) ap N bbox 0 0 1 1 matrix 1 0 0 1 0 0 items",
        "idx 0 as (Off) ap N bbox 0 0 1 1 matrix 1 0 0 1 0 0 items none extra",
        "idx 0 as (Off) ap X bbox 0 0 1 1 matrix 1 0 0 1 0 0 items none",
        "idx 0 as (Off) ap N bbox 0 0 1 1 matrix 1 0 0 1 0 0 items q",
        "idx 0 as (Off)",
        "idx 1 as none ap none",
    ] {
        assert!(
            std::panic::catch_unwind(|| widget_records(wrong)).is_err(),
            "read: {wrong}"
        );
    }

    let (states, fields) = pressed_records("idx 0 as /Off ; idx 1 as /2 ; field g.kid v /2");
    assert_eq!(states, ["/Off", "/2"]);
    assert_eq!(fields, [("g.kid".to_string(), "/2".to_string())]);
    for wrong in [
        "idx 1 as /Off",
        "field g v /2 ; idx 0 as /Off",
        "idx 0 as /Off extra",
        "field g /2",
    ] {
        assert!(
            std::panic::catch_unwind(|| pressed_records(wrong)).is_err(),
            "read: {wrong}"
        );
    }
    assert!(
        ["/Off", "(Off)", "none", "other"]
            .iter()
            .all(|w| is_value_word(w))
    );
    assert!(!["/", "()", "Off", "%"].iter().any(|w| is_value_word(w)));
}

/// The parts of a `forms-press` record a `differ.tsv` line may name: a
/// widget's `/AS` (`as`, or `as@N` for widget N alone), and a field's `/V`
/// (`v`).
const PRESS_PARTS: [&str; 2] = ["as", "v"];

/// A value as the oracle writes `/AS` and `/V` (`is_value_word`), `objects`
/// resolving a reference: a name as `/` and its bytes as `esc` writes them,
/// a string in parentheses, `none`, or `other`.
fn value_word<'o>(objects: impl Resolve<'o>, object: Option<&'o Object>) -> String {
    let object = match object.map(|o| (o, o.as_reference())) {
        Some((_, Some(id))) => objects.resolve(id),
        Some((object, None)) => Some(object),
        None => None,
    };
    match object {
        None => "none".into(),
        Some(object) => match (object.as_name(), object.value()) {
            (Some(name), _) => format!("/{}", esc(name)),
            (_, Value::String(Str::Bytes(bytes), _)) => format!("({})", esc(bytes)),
            _ => "other".into(),
        },
    }
}

/// The presses of the `forms-press` case `name` made here: each widget of
/// its page's `/AS` and each field's full name and `/V` after them, as the
/// oracle writes them. A pressed field's group is read once
/// (`Field::group`), its widgets' states from their `/AS`, and pressed in
/// turn (`Group::press`); then, as `pdf_forms::group` has a caller save
/// them, its `/V` is the group's value and each of its widgets' `/AS` the
/// group's name for that widget's state. Every other `/AS` and `/V` is the
/// file's.
fn pressed_here(name: &str) -> (Vec<String>, Vec<(String, String)>) {
    let cases = set().join("cases");
    let bytes = fs::read(cases.join(format!("{name}.pdf"))).unwrap();
    let presses: Vec<usize> = fs::read_to_string(cases.join(format!("{name}.press")))
        .unwrap()
        .split_whitespace()
        .map(|p| p.parse().unwrap())
        .collect();
    let mut opts = read::Options::DEFAULT;
    opts.recovery = Recovery::Pdfium;
    let reading = Reading::open(&bytes[..], &opts).unwrap();
    let objects = &reading;
    let catalog = pdf_forms::pdf_semantics::catalog(objects, reading.trailer().dict())
        .unwrap()
        .unwrap();
    let table = pages::pages(
        objects,
        catalog,
        Rules::Pdfium,
        &pdf_forms::pdf_semantics::Limits::DEFAULT,
        &mut (),
    )
    .unwrap();
    let page = table.iter().next().and_then(|page| page.dict).unwrap();
    let annots = page
        .get("Annots")
        .and_then(|object| match object.as_reference() {
            Some(id) => objects.resolve(id),
            None => Some(object),
        })
        .and_then(Object::as_array)
        .unwrap();
    let form = Form::read(objects, catalog, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap()
        .unwrap();
    // The field and the index among its widgets of a page's annotation.
    let widget_of = |entry: &Object| {
        let id = entry.as_reference()?;
        form.fields().iter().enumerate().find_map(|(index, field)| {
            let at = field.widgets().iter().position(|w| w.id() == Some(id))?;
            Some((index, at))
        })
    };

    let mut groups: HashMap<usize, (FieldGroup, Vec<bool>)> = HashMap::new();
    for press in presses {
        let (field, at) = widget_of(&annots[press]).unwrap();
        let (group, states) = groups.entry(field).or_insert_with(|| {
            let group = form.fields()[field]
                .group(objects, Rules::Pdfium, &Limits::DEFAULT)
                .unwrap()
                .unwrap();
            let states = group.states().to_vec();
            (group, states)
        });
        *states = group.group().press(states, at);
    }

    let mut states = Vec::new();
    for entry in annots {
        let annot = match entry.as_reference() {
            Some(id) => objects.resolve(id),
            None => Some(entry),
        };
        let mut word = value_word(
            objects,
            annot.and_then(Object::as_dict).and_then(|d| d.get("AS")),
        );
        if let Some((field, at)) = widget_of(entry)
            && let Some((group, after)) = groups.get(&field)
        {
            word = format!("/{}", esc(&group.group().state_name(at, after[at])));
        }
        states.push(word);
    }
    let mut fields = Vec::new();
    for (index, field) in form.fields().iter().enumerate() {
        if field.partial_name().is_none() {
            continue;
        }
        let word = match groups.get(&index) {
            Some((group, after)) => format!("/{}", esc(&group.group().value(after))),
            None => value_word(objects, field.dict().get("V")),
        };
        fields.push((esc(form.full_name(index).unwrap().as_bytes()), word));
    }
    (states, fields)
}

/// Each `forms-press` case's presses, made here by `pdf_forms::group`'s
/// rules over the case's file read with pdf-structure ([`pressed_here`]),
/// leave each widget's `/AS` and each field's `/V` as PDFium's record has
/// them, word for word, but for the parts `differ.tsv` lists for the case,
/// which must still differ.
#[test]
fn presses_agree_with_pdfiums() {
    let recorded: HashMap<String, String> = read_tsv::<2>("chromium-8152.txt")
        .into_iter()
        .map(|[k, v]| (k, v))
        .collect();
    let cases: Vec<String> = read_tsv::<3>("cases.tsv")
        .into_iter()
        .filter(|[_, kind, _]| kind == "forms-press")
        .map(|[name, _, _]| name)
        .collect();

    // Each case's listed parts, each for one widget (`as@N`) or any.
    let mut listed: HashMap<String, BTreeSet<(String, Option<usize>)>> = HashMap::new();
    for line in differ_lines() {
        assert_ne!(line[0], "forms-press", "a rule for every press: {line:?}");
        if !cases.contains(&line[0]) {
            continue;
        }
        assert_eq!(
            line.len(),
            3,
            "a forms-press case's differ.tsv line: {line:?}"
        );
        let (part, at) = match line[1].split_once('@') {
            Some((part, at)) => (part, Some(at.parse().unwrap())),
            None => (line[1].as_str(), None),
        };
        assert!(PRESS_PARTS.contains(&part), "{line:?}");
        assert!(part == "as" || at.is_none(), "{line:?}");
        assert!(
            listed
                .entry(line[0].clone())
                .or_default()
                .insert((part.to_owned(), at)),
            "a part listed twice: {line:?}"
        );
    }

    let mut wrong = Vec::new();
    for name in &cases {
        let (states, fields) = pressed_records(&recorded[name]);
        let (our_states, our_fields) = pressed_here(name);
        assert_eq!(
            states.len(),
            our_states.len(),
            "{name}: a state for each widget"
        );
        let mut found = BTreeSet::new();
        for (index, (pdfium, ours)) in states.iter().zip(&our_states).enumerate() {
            if pdfium != ours {
                found.insert(("as", Some(index)));
            }
        }
        if fields != our_fields {
            found.insert(("v", None));
        }
        let expected = listed.remove(name).unwrap_or_default();
        let covers = |(part, at): &(String, Option<usize>),
                      (differs, index): &(&str, Option<usize>)| {
            part == differs && at.is_none_or(|at| Some(at) == *index)
        };
        let unlisted: Vec<_> = found
            .iter()
            .filter(|f| !expected.iter().any(|e| covers(e, f)))
            .collect();
        let agreeing: Vec<_> = expected
            .iter()
            .filter(|e| !found.iter().any(|f| covers(e, f)))
            .collect();
        if !unlisted.is_empty() || !agreeing.is_empty() {
            wrong.push(format!(
                "{name}: differs in {unlisted:?}, listed but agrees in {agreeing:?}\n  pdfium: \
                 {states:?} {fields:?}\n  ours:   {our_states:?} {our_fields:?}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} press case(s) disagreed with chromium/8152:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// The appearances `pdf_forms::appearance::toggle_for_widget` and
/// `push_for_widget` make for each widget of a `forms-ap-all` case's check
/// boxes, radio buttons and push buttons (`btn-` and `push-` cases),
/// interpreted with `pdf_gfx::interpret` and recorded item by item as the
/// oracle records PDFium's, and held to them part by part, under the
/// features `made_here` needs, whose comparisons of a path and a color it
/// takes.
#[cfg(all(feature = "text-layout", feature = "programs"))]
mod buttons_here {
    use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
    use std::fs;
    use std::ops::ControlFlow;

    use pdf_forms::appearance::{self, Appearance as Made};
    use pdf_forms::pdf_annots::{AppearanceEntry, Appearances};
    use pdf_forms::pdf_content_streams::{Concat, ResBody};
    use pdf_forms::pdf_fonts::{Encoding, Fonts, Std14};
    use pdf_forms::pdf_gfx::{self, Kind, Opts};
    use pdf_forms::pdf_objects::pdf_tokenizer::Rules;
    use pdf_forms::pdf_objects::{Dict, Object, Resolve};
    use pdf_forms::pdf_semantics::pages;
    use pdf_forms::{FieldKind, Form, Limits};
    use pdf_structure::read::Recovery;
    use pdf_structure::{Reading, read};

    use super::made_here::{
        ADDED, EXACT, PLACE, client_box, color, near, painted, placed, rectangle, regions,
        same_color, same_paint, segments,
    };
    use super::{
        Color, Drawn, Item, Painted, Segment, Shown, WidgetRecord, differ_lines, esc, read_tsv,
        set, value_word, widget_records,
    };

    /// A record's parts a `differ.tsv` line for a button case may name:
    /// `made`, made on one side and not the other; `states`, the states
    /// made (a toggle's on state's name and its `Off`, a push button's
    /// slots); `as`, the widget's `/AS`; and each made appearance's `bbox`,
    /// `matrix`, `paint` (the frame), `mark` (a toggle's), `clip` and
    /// `text` (a push button's caption).
    const PARTS: [&str; 9] = [
        "made", "states", "as", "bbox", "matrix", "paint", "mark", "clip", "text",
    ];

    /// The differences that are rules, each a line under `forms-ap-all`:
    /// `/AS` set as a name where PDFium sets a string; the mark a glyph,
    /// held by its box; and an `Off` that paints nothing not made.
    const RULES: [&str; 3] = ["as", "mark", "off"];

    /// How far a mark's box may lie from where PDFium's shape reaches, as a
    /// share of the side of the centre square it's drawn in: its centre on
    /// each axis, and its width and its height; a cross's width, whose glyph
    /// is narrower than tall where PDFium's cross is square, apart.
    const MARK: f32 = 0.04;
    const CROSS_WIDTH: f32 = 0.16;

    /// The appearances of one slot (`N`, `D` or `R`), by state (none for a
    /// stream).
    type Slots = BTreeMap<(String, Option<String>), Drawn>;

    /// One annotation of a case's page, as made here.
    struct Annotation {
        /// What's made, or why nothing is.
        made: Result<Slots, String>,
        /// Whether it's a check box's or a radio button's.
        toggle: bool,
        /// The on state's name a toggle's are made under here, and the
        /// one PDFium makes them under, as `esc` writes them.
        on_state: Option<(String, String)>,
        /// Its `/AS` as the oracle writes it, as a caller binds what's made:
        /// the widget's own, else `Off`, a name.
        state: String,
        /// Whether the widget had no `/AS`.
        no_state: bool,
        /// Each slot and state its `/AP` had in the case's file.
        had: BTreeSet<(String, String)>,
        /// The box less the border's reach, `[x y w h]`.
        client: [f32; 4],
        /// The name its font is recorded by.
        font: String,
    }

    /// `made`'s content's items as `forms-ap-all` records them: each path
    /// painted, each clip and where the `Q` that ends it comes, `/Tx`'s
    /// bounds, and each string shown with its color, its font (`font` where
    /// the content names the one resource it needs), its size, origin and
    /// codes.
    pub(super) fn items(
        objects: &Reading<&[u8]>,
        made: &Made,
        font: &str,
    ) -> Result<Vec<Item>, String> {
        let mut fonts = Dict::new();
        for need in &made.needs.0 {
            let bound = match need.res.body() {
                ResBody::Written(id) => Object::from(*id),
                ResBody::Objects(objects) => objects[0].clone(),
                other => panic!("a resource the maker doesn't name: {other:?}"),
            };
            fonts.set(need.name.clone(), bound);
        }
        let mut resources = Dict::new();
        resources.set("Font", fonts);
        let resources = Object::from(resources);
        let font_need = made.needs.0.first().map(|need| need.name.clone());

        let (mut items, mut saved, mut clips) = (Vec::new(), Vec::new(), 0usize);
        let mut tf: Option<Vec<u8>> = None;
        pdf_gfx::interpret(
            objects,
            &Concat::new(&[&made.content]),
            Some(&resources),
            &pdf_gfx::Limits::DEFAULT,
            &Fonts::default(),
            Opts::default(),
            |event| {
                match event.kind {
                    Kind::Op if event.at.name() == b"q" => saved.push(clips),
                    Kind::Op if event.at.name() == b"Q" => {
                        let before = saved.pop().unwrap_or(0);
                        items.extend((before..clips).map(|_| Item::EndClip));
                        clips = before;
                    }
                    Kind::Op if event.at.name() == b"Tf" => {
                        tf = event
                            .at
                            .operand(1)
                            .and_then(|o| o.name())
                            .map(|n| n.into_owned());
                    }
                    Kind::MarkedBegin(mark) if mark.tag == b"Tx" => items.push(Item::TxBegin),
                    Kind::MarkedEnd(mark) if mark.tag == b"Tx" => items.push(Item::TxEnd),
                    Kind::Path(path) if path.clip.is_some() => {
                        items.push(Item::Clip(segments(&path)));
                        clips += 1;
                    }
                    Kind::Path(path) => items.push(Item::Painted(painted(event.state, &path))),
                    Kind::Text(show) => {
                        let named = tf.is_some() && tf == font_need;
                        let state = event.state;
                        items.push(Item::Text {
                            color: color(&state.fill_color_space, &state.fill_color, false),
                            shown: Shown {
                                font: if named { font.into() } else { "?".into() },
                                size: show.size,
                                at: [show.start.e, show.start.f],
                                codes: esc(show.string),
                            },
                        });
                    }
                    _ => {}
                }
                ControlFlow::Continue(())
            },
        )
        .map_err(|e| format!("our content doesn't interpret: {e}"))?;
        Ok(items)
    }

    /// `made` as the oracle records a stream.
    pub(super) fn drawn(
        objects: &Reading<&[u8]>,
        made: &Made,
        font: &str,
    ) -> Result<Drawn, String> {
        Ok(Drawn {
            bbox: [made.bbox.x0, made.bbox.y0, made.bbox.x1, made.bbox.y1],
            matrix: made.matrix.to_array(),
            items: items(objects, made, font)?,
        })
    }

    /// The slots and states of `appearances`, as names `esc` writes.
    fn had(appearances: Option<&Appearances>) -> BTreeSet<(String, String)> {
        let mut had = BTreeSet::new();
        let Some(appearances) = appearances else {
            return had;
        };
        for (slot, entry) in [
            ("N", &appearances.normal),
            ("D", &appearances.down),
            ("R", &appearances.rollover),
        ] {
            if let Some(AppearanceEntry::States(states)) = entry {
                for (name, _) in states.iter() {
                    had.insert((slot.to_string(), esc(name)));
                }
            }
        }
        had
    }

    /// Each annotation of `name`'s page as made here, and the names of the
    /// default resources' fonts.
    fn made_for(name: &str) -> (Vec<Option<Annotation>>, HashSet<String>) {
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let mut opts = read::Options::DEFAULT;
        opts.recovery = Recovery::Pdfium;
        let reading = Reading::open(&bytes[..], &opts).unwrap();
        let objects = &reading;
        let catalog = pdf_forms::pdf_semantics::catalog(objects, reading.trailer().dict())
            .unwrap()
            .unwrap();
        let table = pages::pages(
            objects,
            catalog,
            Rules::Pdfium,
            &pdf_forms::pdf_semantics::Limits::DEFAULT,
            &mut (),
        )
        .unwrap();
        let page = table.iter().next().and_then(|page| page.dict).unwrap();
        let form = Form::read(objects, catalog, Rules::Pdfium, &Limits::DEFAULT)
            .unwrap()
            .unwrap();
        let fonts = form
            .font_names()
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .collect();
        let annots = page
            .get("Annots")
            .and_then(|object| match object.as_reference() {
                Some(id) => objects.resolve(id),
                None => Some(object),
            });
        let limits = Limits::DEFAULT;
        let mut made = Vec::new();
        for entry in annots.and_then(Object::as_array).unwrap_or(&[]) {
            let id = entry.as_reference();
            let found = form.fields().iter().find_map(|field| {
                let index = field
                    .widgets()
                    .iter()
                    .position(|w| w.id().is_some() && w.id() == id)?;
                Some((field, index))
            });
            let Some((field, index)) = found else {
                made.push(None);
                continue;
            };
            let widget = &field.widgets()[index];
            let annot = widget.annot(objects, Rules::Pdfium, &limits).unwrap();
            let own_state = widget.dict().get("AS");
            let toggle = field.kind() != Some(FieldKind::PushButton);
            let mut annotation = Annotation {
                made: Err(String::new()),
                toggle,
                on_state: None,
                state: match own_state {
                    Some(_) => value_word(objects, own_state),
                    None if toggle => "/Off".into(),
                    None => "none".into(),
                },
                no_state: own_state.is_none(),
                had: had(annot.appearances()),
                client: client_box(&annot),
                font: String::new(),
            };
            let mut slots = Slots::new();
            if toggle {
                // PDFium names the on state the widget's index where the
                // field has `/Opt`, else by the least name its normal
                // appearance has for on, else `Yes`.
                let options = field.options(objects, Rules::Pdfium, &limits).unwrap();
                let states = widget.states(objects, Rules::Pdfium, &limits).unwrap();
                let pdfium_on = match (options.is_empty(), &states.on_state) {
                    (false, _) => index.to_string().into_bytes(),
                    (true, Some(own)) => own.to_vec(),
                    (true, None) => b"Yes".to_vec(),
                };
                let made = appearance::toggle_for_widget(
                    &form,
                    field,
                    widget,
                    objects,
                    Rules::Pdfium,
                    &limits,
                    &mut (),
                );
                annotation.made = made.map_err(|e| e.to_string()).and_then(|made| {
                    let on = esc(&made.on_state);
                    annotation.on_state = Some((on.clone(), esc(&pdfium_on)));
                    let ap = &made.appearances;
                    let font = "ZapfDingbats";
                    for (slot, state, made) in [
                        ("N", &on, Some(&ap.on)),
                        ("N", &"Off".to_string(), ap.off.as_ref()),
                        ("D", &on, Some(&ap.down_on)),
                        ("D", &"Off".to_string(), ap.down_off.as_ref()),
                    ] {
                        if let Some(made) = made {
                            let key = (slot.to_string(), Some(state.clone()));
                            slots.insert(key, drawn(objects, made, font)?);
                        }
                    }
                    Ok(slots)
                });
            } else {
                let read = appearance::widget_push(
                    field,
                    widget,
                    objects,
                    Rules::Pdfium,
                    &limits,
                    &mut (),
                );
                let made = appearance::push_for_widget(
                    &form,
                    field,
                    widget,
                    objects,
                    Rules::Pdfium,
                    &limits,
                    &mut (),
                );
                annotation.made = match (read, made) {
                    (Ok(read), Ok(made)) => {
                        let font = match made.font.is_fallback() {
                            true => ADDED.to_string(),
                            false => String::from_utf8_lossy(read.font_name().unwrap()).into(),
                        };
                        annotation.font = font.clone();
                        let ap = &made.appearances;
                        (|| {
                            for (slot, made) in [
                                ("N", Some(&ap.normal)),
                                ("D", ap.down.as_ref()),
                                ("R", ap.rollover.as_ref()),
                            ] {
                                if let Some(made) = made {
                                    let key = (slot.to_string(), None);
                                    slots.insert(key, drawn(objects, made, &font)?);
                                }
                            }
                            Ok(slots)
                        })()
                    }
                    (Err(e), _) | (_, Err(e)) => Err(e.to_string()),
                };
            }
            made.push(Some(annotation));
        }
        (made, fonts)
    }

    /// The box where `segments` reach, each curve by its extremes rather
    /// than its control points: `[x0 y0 x1 y1]`.
    fn reach(segments: &[Segment]) -> [f32; 4] {
        let mut b = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        let mut take = |x: f32, y: f32| {
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        };
        let mut at = [0.0f32; 2];
        for segment in segments {
            match *segment {
                Segment::Rect([x, y, w, h]) => {
                    take(x, y);
                    take(x + w, y + h);
                }
                Segment::Move(p) | Segment::Line(p) => {
                    take(p[0], p[1]);
                    at = p;
                }
                Segment::Curve([x1, y1, x2, y2, x3, y3]) => {
                    // A cubic's coordinate at t, and where its derivative,
                    // a quadratic, is 0 within (0, 1).
                    let on = |a: f32, b: f32, c: f32, d: f32, t: f32| {
                        let u = 1.0 - t;
                        u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
                    };
                    let turns = |a: f32, b: f32, c: f32, d: f32| {
                        let (qa, qb, qc) =
                            (-a + 3.0 * b - 3.0 * c + d, 2.0 * (a - 2.0 * b + c), b - a);
                        let mut ts = Vec::new();
                        if qa.abs() < 1e-9 {
                            if qb.abs() > 1e-9 {
                                ts.push(-qc / qb);
                            }
                        } else {
                            let disc = qb * qb - 4.0 * qa * qc;
                            if disc >= 0.0 {
                                let root = disc.sqrt();
                                ts.push((-qb + root) / (2.0 * qa));
                                ts.push((-qb - root) / (2.0 * qa));
                            }
                        }
                        ts.into_iter()
                            .filter(|t| *t > 0.0 && *t < 1.0)
                            .collect::<Vec<_>>()
                    };
                    let mut ts = turns(at[0], x1, x2, x3);
                    ts.extend(turns(at[1], y1, y2, y3));
                    ts.extend([0.0, 1.0]);
                    for t in ts {
                        take(on(at[0], x1, x2, x3, t), on(at[1], y1, y2, y3, t));
                    }
                    at = [x3, y3];
                }
                Segment::Close => {}
            }
        }
        b
    }

    /// Where our mark's glyph reaches, from the string shown: its box in
    /// ZapfDingbats at its size from its origin, `[x0 y0 x1 y1]`.
    fn glyph_reach(shown: &Shown) -> Option<[f32; 4]> {
        let [code] = shown.codes.as_bytes() else {
            return None;
        };
        let name = Encoding::ZapfDingbats.glyph(*code)?;
        let b = Std14::ZapfDingbats.glyph_box(name)?;
        let k = shown.size / 1000.0;
        let [x, y] = shown.at;
        Some([x + b.x0 * k, y + b.y0 * k, x + b.x1 * k, y + b.y1 * k])
    }

    /// Whether our mark, `shown` in `ours`, is PDFium's last path,
    /// `pdfium`, within the mark's tolerance of the centre square's `side`,
    /// in the same color.
    fn same_mark(pdfium: &Painted, ours: (&Color, &Shown), side: f32) -> bool {
        let (color, shown) = ours;
        let Some(glyph) = glyph_reach(shown) else {
            return false;
        };
        let path = reach(&pdfium.segments);
        let centre = |b: [f32; 4]| [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0];
        let size = |b: [f32; 4]| [b[2] - b[0], b[3] - b[1]];
        let wide = match shown.codes.as_str() {
            "8" => CROSS_WIDTH,
            _ => MARK,
        };
        let [pw, ph] = size(path);
        let [gw, gh] = size(glyph);
        let same_color = pdfium.color.op.to_lowercase() == color.op
            && pdfium.color.components.len() == color.components.len()
            && (pdfium.color.components.iter().zip(&color.components))
                .all(|(a, b)| (a - b).abs() <= EXACT);
        near(centre(path), centre(glyph), MARK * side)
            && (pw - gw).abs() <= wide * side
            && (ph - gh).abs() <= MARK * side
            && same_color
    }

    /// How often each rule applied, and held.
    #[derive(Default)]
    struct Held {
        off: usize,
        as_: usize,
        mark: usize,
    }

    /// The parts in which one made appearance differs from PDFium's: the
    /// box, the matrix, the frame painted under the rest, a toggle's mark
    /// (its last path, PDFium's, against our one string), and a push
    /// button's clip and caption.
    fn differing_drawn(
        pdfium: &Drawn,
        ours: &Drawn,
        toggle: bool,
        annotation: &Annotation,
        fonts: &HashSet<String>,
        parts: &mut BTreeSet<&'static str>,
        held: &mut Held,
    ) {
        if !near(pdfium.bbox, ours.bbox, EXACT) {
            parts.insert("bbox");
        }
        if !near(pdfium.matrix, ours.matrix, EXACT) {
            parts.insert("matrix");
        }
        let paints = |items: &[Item]| -> Vec<Painted> {
            (items.iter())
                .filter_map(|item| match item {
                    Item::Painted(p) => Some(p.clone()),
                    _ => None,
                })
                .collect()
        };
        let texts = |items: &[Item]| -> Vec<(Color, Shown)> {
            (items.iter())
                .filter_map(|item| match item {
                    Item::Text { color, shown } => Some((color.clone(), shown.clone())),
                    _ => None,
                })
                .collect()
        };
        let (mut pdfium_paint, ours_paint) = (paints(&pdfium.items), paints(&ours.items));
        let (pdfium_text, ours_text) = (texts(&pdfium.items), texts(&ours.items));
        if toggle {
            // A mark in ours is a string; in PDFium's, its last path.
            match (ours_text.as_slice(), ours_paint.len() < pdfium_paint.len()) {
                ([(color, shown)], true) => {
                    let mark = pdfium_paint.pop().unwrap();
                    let side = annotation.client[2].min(annotation.client[3]);
                    match same_mark(&mark, (color, shown), side) {
                        true => held.mark += 1,
                        false => {
                            parts.insert("mark");
                        }
                    }
                }
                ([], _) => {}
                _ => {
                    parts.insert("mark");
                }
            }
            if !pdfium_text.is_empty() {
                parts.insert("mark");
            }
        } else {
            let clip = |items: &[Item]| -> Option<[f32; 4]> {
                (items.iter()).find_map(|item| match item {
                    Item::Clip(segments) => match segments.as_slice() {
                        [Segment::Rect([x, y, w, h])] => Some([*x, *y, x + w, y + h]),
                        other => rectangle(other),
                    },
                    _ => None,
                })
            };
            let ended = |items: &[Item]| items.iter().filter(|i| **i == Item::EndClip).count();
            let same_clip = match (clip(&pdfium.items), clip(&ours.items)) {
                (Some(p), Some(o)) => near(p, o, PLACE),
                (None, None) => true,
                _ => false,
            };
            if !same_clip || ended(&pdfium.items) != ended(&ours.items) {
                parts.insert("clip");
            }
            let same_text = pdfium_text.len() == ours_text.len()
                && pdfium_text
                    .iter()
                    .zip(&ours_text)
                    .all(|((pc, p), (oc, o))| {
                        let font = match fonts.contains(&p.font) {
                            true => p.font.as_str(),
                            false => ADDED,
                        };
                        same_color(pc, oc)
                            && font == o.font
                            && (p.size - o.size).abs() <= EXACT
                            && near(p.at, o.at, PLACE)
                            && p.codes == o.codes
                    });
            if !same_text {
                parts.insert("text");
            }
        }
        if !same_paint(&placed(&pdfium_paint), &regions(ours_paint)) {
            parts.insert("paint");
        }
    }

    /// The parts in which what's made here for one annotation differs from
    /// PDFium's record of it.
    fn differing(
        pdfium: &WidgetRecord,
        ours: &Option<Annotation>,
        fonts: &HashSet<String>,
        held: &mut Held,
    ) -> BTreeSet<&'static str> {
        let mut parts = BTreeSet::new();
        let Some(ours) = ours else {
            if pdfium.appearances.is_some() {
                parts.insert("made");
            }
            return parts;
        };
        let (Some(appearances), Ok(made)) = (&pdfium.appearances, &ours.made) else {
            if pdfium.appearances.is_some() || ours.made.is_ok() {
                parts.insert("made");
            }
            return parts;
        };
        // `/AS`: a name here, where PDFium sets `(Off)` where there was none.
        if pdfium.state != ours.state {
            match ours.no_state && pdfium.state == "(Off)" && ours.state == "/Off" {
                true => held.as_ += 1,
                false => {
                    parts.insert("as");
                }
            }
        }
        // PDFium's: the states it made, and those the file had beside them.
        let mut theirs = Slots::new();
        for appearance in appearances {
            let key = (appearance.slot.clone(), appearance.state.clone());
            let made_here = match (&ours.on_state, &appearance.state) {
                (Some((_, on)), Some(state)) => state == on || state == "Off",
                _ => true,
            };
            if !made_here {
                let state = appearance.state.clone().unwrap_or_default();
                if !ours.had.contains(&(appearance.slot.clone(), state)) {
                    parts.insert("states");
                }
                continue;
            }
            match &appearance.drawn {
                Some(drawn) => {
                    theirs.insert(key, drawn.clone());
                }
                None => {
                    parts.insert("states");
                }
            }
        }
        // Ours by the states PDFium made: the on state by role, its name
        // compared apart.
        let rename = |(slot, state): &(String, Option<String>)| {
            let state = match (&ours.on_state, state) {
                (Some((here, there)), Some(state)) if state == here => Some(there.clone()),
                _ => state.clone(),
            };
            (slot.clone(), state)
        };
        let mine: Slots = made.iter().map(|(k, v)| (rename(k), v.clone())).collect();
        if let Some((here, there)) = &ours.on_state
            && here != there
        {
            parts.insert("states");
        }
        for (key, pdfium) in &theirs {
            match mine.get(key) {
                Some(ours_drawn) => differing_drawn(
                    pdfium,
                    ours_drawn,
                    ours.toggle,
                    ours,
                    fonts,
                    &mut parts,
                    held,
                ),
                // An `Off` that paints nothing, not made here.
                None if key.1.as_deref() == Some("Off") && pdfium.items.is_empty() => {
                    held.off += 1;
                }
                None => {
                    parts.insert("states");
                }
            }
        }
        if mine.keys().any(|key| !theirs.contains_key(key)) {
            parts.insert("states");
        }
        parts
    }

    /// Each `btn-` and `push-` case's appearances, made by
    /// `toggle_for_widget` and `push_for_widget` from the case's file read
    /// with pdf-structure, agree with PDFium's recorded ones in every part,
    /// or in every part but those `differ.tsv` lists for it, which must
    /// still differ: the states made (an on state by the name PDFium makes
    /// it under, its stale states left aside where the file had them), the
    /// `/AS` a caller sets, the box and the matrix, each path of the frame,
    /// in order, as the region it covers within 0.01 pt in its color, a push
    /// button's clip and every caption's origin within 0.05 pt, its size,
    /// font and codes; and a check box's or radio button's mark, our
    /// glyph's box against where PDFium's last path reaches, within 0.04 of
    /// the centre square's side for its centre on each axis, its width and
    /// its height, a cross's width within 0.16. Where PDFium's `Off` paints
    /// nothing none is made here, and `/AS` is a name where PDFium sets
    /// `(Off)`, each held on every such case; the mark by its tolerance on
    /// every case it agrees on.
    #[test]
    fn made_buttons_agree_with_pdfiums() {
        let recorded: HashMap<String, String> = read_tsv::<2>("chromium-8152.txt")
            .into_iter()
            .map(|[k, v]| (k, v))
            .collect();
        let cases: Vec<String> = read_tsv::<3>("cases.tsv")
            .into_iter()
            .filter(|[name, kind, _]| {
                kind == "forms-ap-all" && (name.starts_with("btn-") || name.starts_with("push-"))
            })
            .map(|[name, _, _]| name)
            .collect();

        let mut listed: HashMap<String, BTreeSet<(String, Option<usize>)>> = HashMap::new();
        let mut rules = BTreeSet::new();
        for line in differ_lines() {
            if line[0] == "forms-ap-all" {
                assert_eq!(line.len(), 3, "{line:?}");
                if RULES.contains(&line[1].as_str()) {
                    assert!(
                        rules.insert(line[1].clone()),
                        "a rule listed twice: {line:?}"
                    );
                }
            } else if cases.contains(&line[0]) {
                assert_eq!(line.len(), 3, "a button case's differ.tsv line: {line:?}");
                let (part, at) = match line[1].split_once('@') {
                    Some((part, at)) => (part, Some(at.parse().unwrap())),
                    None => (line[1].as_str(), None),
                };
                assert!(PARTS.contains(&part), "{line:?}");
                assert!(
                    listed
                        .entry(line[0].clone())
                        .or_default()
                        .insert((part.to_owned(), at)),
                    "a part listed twice: {line:?}"
                );
            }
        }
        assert_eq!(rules.len(), RULES.len(), "a differ.tsv line for each rule");

        let mut held = Held::default();
        let mut wrong = Vec::new();
        for name in &cases {
            let pdfium = widget_records(&recorded[name]);
            let (ours, fonts) = made_for(name);
            assert_eq!(
                pdfium.len(),
                ours.len(),
                "{name}: a record for each annotation"
            );
            let mut found = BTreeSet::new();
            let mut said = Vec::new();
            for (index, (pdfium, ours)) in pdfium.iter().zip(&ours).enumerate() {
                let here = differing(pdfium, ours, &fonts, &mut held);
                if !here.is_empty() {
                    let made = ours.as_ref().map(|a| &a.made);
                    said.push(format!(
                        "  idx {index}: {here:?}\n    pdfium: {pdfium:?}\n    ours:   {made:?}"
                    ));
                }
                found.extend(here.into_iter().map(|part| (index, part)));
            }
            let expected = listed.remove(name).unwrap_or_default();
            let covers = |(part, at): &(String, Option<usize>),
                          (index, differs): &(usize, &str)| {
                part == differs && at.is_none_or(|at| at == *index)
            };
            let unlisted: Vec<_> = found
                .iter()
                .filter(|f| !expected.iter().any(|e| covers(e, f)))
                .collect();
            let agreeing: Vec<_> = expected
                .iter()
                .filter(|e| !found.iter().any(|f| covers(e, f)))
                .collect();
            if !unlisted.is_empty() || !agreeing.is_empty() {
                wrong.push(format!(
                    "{name}: differs unlisted in {unlisted:?}; listed but agrees: {agreeing:?}\n{}",
                    said.join("\n")
                ));
            }
        }
        assert!(
            listed.is_empty(),
            "differ.tsv lists cases that aren't button cases: {listed:?}"
        );
        for (rule, count) in [("off", held.off), ("as", held.as_), ("mark", held.mark)] {
            if count == 0 {
                wrong.push(format!("the {rule} rule held on no case"));
            }
        }
        assert!(
            wrong.is_empty(),
            "{} button case(s) disagreed with chromium/8152:\n{}",
            wrong.len(),
            wrong.join("\n")
        );
    }
}

/// The appearances `pdf_forms::appearance::choice_for_widget` makes for each
/// widget of a `forms-ap-all` case's combo boxes and list boxes (`combo-`
/// and `list-` cases), interpreted with `pdf_gfx::interpret` and recorded
/// item by item as the oracle records PDFium's (`buttons_here::drawn`), and
/// held to them part by part, under the features `made_here` needs, whose
/// comparisons of a path and a color it takes.
#[cfg(all(feature = "text-layout", feature = "programs"))]
mod choices_here {
    use std::collections::{BTreeSet, HashMap, HashSet};
    use std::fs;

    use pdf_forms::appearance;
    use pdf_forms::pdf_gfx::LayoutFont;
    use pdf_forms::pdf_objects::pdf_tokenizer::Rules;
    use pdf_forms::pdf_objects::{Object, Resolve};
    use pdf_forms::pdf_semantics::pages;
    use pdf_forms::{FieldKind, Form, Limits};
    use pdf_structure::read::Recovery;
    use pdf_structure::{Reading, read};

    use super::buttons_here::drawn;
    use super::made_here::{
        ADDED, EXACT, PLACE, client_box, near, placed, rectangle, regions, same_color, same_paint,
    };
    use super::{Drawn, Item, Painted, Segment, WidgetRecord, differ_lines, read_tsv, set};

    /// A record's parts a `differ.tsv` line for a choice case may name:
    /// `made`, made on one side and not the other; the appearance's `bbox`
    /// and `matrix`; `paint`, the frame and a combo box's drop button;
    /// `clip`, the `/Tx` sequence's clip; and `text`, what it shows, each
    /// string and a list box's highlights in order.
    const PARTS: [&str; 6] = ["made", "bbox", "matrix", "paint", "clip", "text"];

    /// The differences that are rules, each a line under `forms-ap-all`:
    /// where PDFium writes no clip, a combo box's clips to its text box;
    /// where PDFium writes no `/Tx` sequence, the one made here is empty;
    /// and a list box's lines PDFium writes below the box aren't written.
    const RULES: [&str; 3] = ["clip", "tx", "below"];

    /// The width PDFium gives a combo box's drop button.
    const DROP_BUTTON: f32 = 13.0;

    /// One annotation of a case's page, as made here.
    struct Annotation {
        /// What's made, or why nothing is.
        made: Result<Drawn, String>,
        /// The box text is set in, `[x0 y0 x1 y1]`: the box less the
        /// border's reach, and a combo box's drop button.
        text_box: [f32; 4],
        /// The bottom of the box less the border's reach.
        bottom: f32,
        /// The font's ascent at the size made, in points.
        ascent: f32,
    }

    /// Each annotation of `name`'s page as made here, and the names of the
    /// default resources' fonts.
    fn made_for(name: &str) -> (Vec<Option<Annotation>>, HashSet<String>) {
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let mut opts = read::Options::DEFAULT;
        opts.recovery = Recovery::Pdfium;
        let reading = Reading::open(&bytes[..], &opts).unwrap();
        let objects = &reading;
        let catalog = pdf_forms::pdf_semantics::catalog(objects, reading.trailer().dict())
            .unwrap()
            .unwrap();
        let table = pages::pages(
            objects,
            catalog,
            Rules::Pdfium,
            &pdf_forms::pdf_semantics::Limits::DEFAULT,
            &mut (),
        )
        .unwrap();
        let page = table.iter().next().and_then(|page| page.dict).unwrap();
        let form = Form::read(objects, catalog, Rules::Pdfium, &Limits::DEFAULT)
            .unwrap()
            .unwrap();
        let fonts = form
            .font_names()
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .collect();
        let annots = page
            .get("Annots")
            .and_then(|object| match object.as_reference() {
                Some(id) => objects.resolve(id),
                None => Some(object),
            });
        let limits = Limits::DEFAULT;
        let mut made = Vec::new();
        for entry in annots.and_then(Object::as_array).unwrap_or(&[]) {
            let id = entry.as_reference();
            let found = form.fields().iter().find_map(|field| {
                let widget = field
                    .widgets()
                    .iter()
                    .find(|w| w.id().is_some() && w.id() == id)?;
                Some((field, *widget))
            });
            let Some((field, widget)) = found else {
                made.push(None);
                continue;
            };
            let annot = widget.annot(objects, Rules::Pdfium, &limits).unwrap();
            let [x, y, w, h] = client_box(&annot);
            let text_box = match field.kind() {
                Some(FieldKind::Combo) => {
                    let left = x + w - DROP_BUTTON;
                    [x.min(left), y, x.max(left), y + h]
                }
                _ => [x, y, x + w, y + h],
            };
            let mut annotation = Annotation {
                made: Err(String::new()),
                text_box,
                bottom: y,
                ascent: 0.0,
            };
            let made_here = appearance::choice_for_widget(
                &form,
                field,
                &widget,
                objects,
                Rules::Pdfium,
                &limits,
                &mut (),
            );
            annotation.made = made_here.map_err(|e| e.to_string()).and_then(|made| {
                let font = match (made.font.is_fallback(), &made.font_name) {
                    (false, Some(name)) => String::from_utf8_lossy(name).into_owned(),
                    _ => ADDED.to_string(),
                };
                let ascent = made.font.layout().font_metrics().ascent;
                annotation.ascent = ascent * made.appearance.font_size / 1000.0;
                drawn(objects, &made.appearance, &font)
            });
            made.push(Some(annotation));
        }
        (made, fonts)
    }

    /// `items` cut at the `/Tx` sequence: the paths painted before and after
    /// it, in order, and what's between its bounds, none where it has none.
    fn cut(items: &[Item]) -> (Vec<Painted>, Option<Vec<Item>>) {
        let (mut paint, mut tx, mut inside) = (Vec::new(), None::<Vec<Item>>, false);
        for item in items {
            match item {
                Item::TxBegin => {
                    inside = true;
                    tx = Some(Vec::new());
                }
                Item::TxEnd => inside = false,
                Item::Painted(painted) if !inside => paint.push(painted.clone()),
                other if inside => tx.as_mut().unwrap().push(other.clone()),
                other => panic!("{other:?} outside /Tx"),
            }
        }
        (paint, tx)
    }

    /// A `/Tx` sequence's clip, `[x0 y0 x1 y1]`, and what it shows: each
    /// path painted (a list box's highlight) and string shown, in order.
    fn shown(tx: &[Item]) -> (Option<[f32; 4]>, Vec<Item>) {
        let mut clip = None;
        let mut shown = Vec::new();
        for item in tx {
            match item {
                Item::Clip(segments) => {
                    clip = match segments.as_slice() {
                        [Segment::Rect([x, y, w, h])] => Some([*x, *y, x + w, y + h]),
                        other => rectangle(other),
                    };
                }
                Item::EndClip => {}
                other => shown.push(other.clone()),
            }
        }
        (clip, shown)
    }

    /// Whether two items shown are the same: a highlight's rectangle within
    /// 0.05 pt in the same color, or a string at its origin within 0.05 pt
    /// in the same color, font, size, and codes.
    fn same_item(pdfium: &Item, ours: &Item, fonts: &HashSet<String>) -> bool {
        match (pdfium, ours) {
            (Item::Painted(p), Item::Painted(o)) => {
                let p = placed(std::slice::from_ref(p));
                let (p, o) = (&p[0], o);
                let (Some(pr), Some(or)) = (rectangle(&p.segments), rectangle(&o.segments)) else {
                    return false;
                };
                p.paint == o.paint && same_color(&p.color, &o.color) && near(pr, or, PLACE)
            }
            (
                Item::Text {
                    color: pc,
                    shown: p,
                },
                Item::Text {
                    color: oc,
                    shown: o,
                },
            ) => {
                let font = match fonts.contains(&p.font) {
                    true => p.font.as_str(),
                    false => ADDED,
                };
                same_color(pc, oc)
                    && font == o.font
                    && (p.size - o.size).abs() <= EXACT
                    && near(p.at, o.at, PLACE)
                    && p.codes == o.codes
            }
            _ => false,
        }
    }

    /// Whether `item`, one PDFium shows past the lines written here, is
    /// wholly below the box less the border's reach: a highlight's top, or
    /// a string's top (its baseline and the font's ascent), at or below
    /// its bottom.
    fn below(item: &Item, ours: &Annotation) -> bool {
        match item {
            Item::Painted(p) => rectangle(&placed(std::slice::from_ref(p))[0].segments)
                .is_some_and(|[_, _, _, top]| top <= ours.bottom + PLACE),
            Item::Text { shown, .. } => shown.at[1] + ours.ascent <= ours.bottom + PLACE,
            _ => false,
        }
    }

    /// How often each rule applied, and held.
    #[derive(Default)]
    struct Held {
        clip: usize,
        tx: usize,
        below: usize,
    }

    /// The parts in which what's made here for one annotation differs from
    /// PDFium's record of it.
    fn differing(
        pdfium: &WidgetRecord,
        ours: &Option<Annotation>,
        fonts: &HashSet<String>,
        held: &mut Held,
    ) -> BTreeSet<&'static str> {
        let mut parts = BTreeSet::new();
        let theirs = (pdfium.appearances.iter().flatten())
            .find(|a| a.slot == "N" && a.state.is_none())
            .and_then(|a| a.drawn.as_ref());
        let (Some(pdfium), Some(Ok(made))) = (theirs, ours.as_ref().map(|a| &a.made)) else {
            if theirs.is_some() || ours.as_ref().is_some_and(|a| a.made.is_ok()) {
                parts.insert("made");
            }
            return parts;
        };
        let ours = ours.as_ref().unwrap();
        if !near(pdfium.bbox, made.bbox, EXACT) {
            parts.insert("bbox");
        }
        if !near(pdfium.matrix, made.matrix, EXACT) {
            parts.insert("matrix");
        }
        let (pdfium_paint, pdfium_tx) = cut(&pdfium.items);
        let (ours_paint, ours_tx) = cut(&made.items);
        if !same_paint(&placed(&pdfium_paint), &regions(ours_paint)) {
            parts.insert("paint");
        }
        match (pdfium_tx, ours_tx) {
            // No `/Tx` in PDFium's: an empty one here.
            (None, Some(tx)) if tx.is_empty() => held.tx += 1,
            (None, None) => {}
            (None, Some(_)) | (Some(_), None) => {
                parts.insert("clip");
                parts.insert("text");
            }
            (Some(p), Some(o)) => {
                let ((p_clip, p_shown), (o_clip, o_shown)) = (shown(&p), shown(&o));
                let clip = match (p_clip, o_clip) {
                    // No clip in PDFium's: the text box.
                    (None, Some(o)) => {
                        let held_here = near(o, ours.text_box, PLACE);
                        held.clip += usize::from(held_here);
                        held_here
                    }
                    (Some(p), Some(o)) => near(p, o, PLACE),
                    (_, None) => false,
                };
                if !clip {
                    parts.insert("clip");
                }
                let same = o_shown.len() <= p_shown.len()
                    && (p_shown.iter().zip(&o_shown)).all(|(p, o)| same_item(p, o, fonts));
                let rest = &p_shown[o_shown.len().min(p_shown.len())..];
                if !same || !rest.iter().all(|item| below(item, ours)) {
                    parts.insert("text");
                } else if !rest.is_empty() {
                    held.below += 1;
                }
            }
        }
        parts
    }

    /// Each `combo-` and `list-` case's appearances, made by
    /// `choice_for_widget` from the case's file read with pdf-structure,
    /// agree with PDFium's recorded ones in every part, or in every part
    /// but those `differ.tsv` lists for it, which must still differ: the box
    /// and the matrix; each path of the frame and a combo box's drop button,
    /// in order, as the region it covers within 0.01 pt in its color; the
    /// clip within 0.05 pt; and what the `/Tx` sequence shows, in order, each
    /// string at its origin within 0.05 pt in its color, size, font, and
    /// codes, and each list box highlight's rectangle within 0.05 pt in its
    /// color. Where PDFium writes no clip, a combo box's here is its text
    /// box; where PDFium writes no `/Tx`, the one here is empty; and where
    /// PDFium writes a list box's lines past those written here, each is
    /// wholly below the box less the border's reach: each rule held on
    /// every such case, and on one at least.
    #[test]
    fn made_choices_agree_with_pdfiums() {
        let recorded: HashMap<String, String> = read_tsv::<2>("chromium-8152.txt")
            .into_iter()
            .map(|[k, v]| (k, v))
            .collect();
        let cases: Vec<String> = read_tsv::<3>("cases.tsv")
            .into_iter()
            .filter(|[name, kind, _]| {
                kind == "forms-ap-all" && (name.starts_with("combo-") || name.starts_with("list-"))
            })
            .map(|[name, _, _]| name)
            .collect();

        let mut listed: HashMap<String, BTreeSet<(String, Option<usize>)>> = HashMap::new();
        let mut rules = BTreeSet::new();
        for line in differ_lines() {
            if line[0] == "forms-ap-all" {
                assert_eq!(line.len(), 3, "{line:?}");
                if RULES.contains(&line[1].as_str()) {
                    assert!(
                        rules.insert(line[1].clone()),
                        "a rule listed twice: {line:?}"
                    );
                }
            } else if cases.contains(&line[0]) {
                assert_eq!(line.len(), 3, "a choice case's differ.tsv line: {line:?}");
                let (part, at) = match line[1].split_once('@') {
                    Some((part, at)) => (part, Some(at.parse().unwrap())),
                    None => (line[1].as_str(), None),
                };
                assert!(PARTS.contains(&part), "{line:?}");
                assert!(
                    listed
                        .entry(line[0].clone())
                        .or_default()
                        .insert((part.to_owned(), at)),
                    "a part listed twice: {line:?}"
                );
            }
        }
        assert_eq!(rules.len(), RULES.len(), "a differ.tsv line for each rule");

        let mut held = Held::default();
        let mut wrong = Vec::new();
        for name in &cases {
            let pdfium = super::widget_records(&recorded[name]);
            let (ours, fonts) = made_for(name);
            assert_eq!(
                pdfium.len(),
                ours.len(),
                "{name}: a record for each annotation"
            );
            let mut found = BTreeSet::new();
            let mut said = Vec::new();
            for (index, (pdfium, ours)) in pdfium.iter().zip(&ours).enumerate() {
                let here = differing(pdfium, ours, &fonts, &mut held);
                if !here.is_empty() {
                    let made = ours.as_ref().map(|a| &a.made);
                    said.push(format!(
                        "  idx {index}: {here:?}\n    pdfium: {pdfium:?}\n    ours:   {made:?}"
                    ));
                }
                found.extend(here.into_iter().map(|part| (index, part)));
            }
            let expected = listed.remove(name).unwrap_or_default();
            let covers = |(part, at): &(String, Option<usize>),
                          (index, differs): &(usize, &str)| {
                part == differs && at.is_none_or(|at| at == *index)
            };
            let unlisted: Vec<_> = found
                .iter()
                .filter(|f| !expected.iter().any(|e| covers(e, f)))
                .collect();
            let agreeing: Vec<_> = expected
                .iter()
                .filter(|e| !found.iter().any(|f| covers(e, f)))
                .collect();
            if !unlisted.is_empty() || !agreeing.is_empty() {
                wrong.push(format!(
                    "{name}: differs unlisted in {unlisted:?}; listed but agrees: {agreeing:?}\n{}",
                    said.join("\n")
                ));
            }
        }
        assert!(
            listed.is_empty(),
            "differ.tsv lists cases that aren't choice cases: {listed:?}"
        );
        for (rule, count) in [("clip", held.clip), ("tx", held.tx), ("below", held.below)] {
            if count == 0 {
                wrong.push(format!("the {rule} rule held on no case"));
            }
        }
        assert!(
            wrong.is_empty(),
            "{} choice case(s) disagreed with chromium/8152:\n{}",
            wrong.len(),
            wrong.join("\n")
        );
    }
}

/// The appearance `pdf_forms::appearance::text_for_widget` makes for each widget
/// of a `forms-ap` case, interpreted with `pdf_gfx::interpret` and recorded
/// in the five parts PDFium's answers are read into, and held to them part
/// by part.
#[cfg(all(feature = "text-layout", feature = "programs"))]
mod made_here {
    use std::collections::{BTreeSet, HashMap, HashSet};
    use std::fs;
    use std::ops::ControlFlow;

    use pdf_forms::appearance;
    use pdf_forms::pdf_annots::{Annot, BorderStyle, TypedAnnot};
    use pdf_forms::pdf_content_streams::{Concat, LineCap, LineJoin, Paint as Painting, ResBody};
    use pdf_forms::pdf_fonts::Fonts;
    use pdf_forms::pdf_gfx::{
        self, ActiveColorSpace, ColorValues, Kind, Opts, PathDrawn, Seg, State,
    };
    use pdf_forms::pdf_objects::pdf_tokenizer::Rules;
    use pdf_forms::pdf_objects::{Dict, Object, Resolve};
    use pdf_forms::pdf_semantics::pages;
    use pdf_forms::{Field, Form, Limits, Widget};
    use pdf_structure::read::Recovery;
    use pdf_structure::{Reading, read};

    use super::{
        Color, Made, Paint, Painted, Segment, Shown, Tx, differ_lines, esc, made_appearances,
        read_tsv, set,
    };

    /// A record's parts a `differ.tsv` line may name: `made`, an appearance
    /// made on one side and not the other, then the five.
    const PARTS: [&str; 6] = ["made", "bbox", "matrix", "paint", "clip", "text"];

    /// The two differences that are rules, each a line under the kind's
    /// name: where PDFium writes no clip, the appearance made here clips to
    /// the box less the border's reach; where PDFium writes no `/Tx`
    /// sequence, the one made here is empty.
    const RULES: [&str; 2] = ["clip", "tx"];

    /// The name a font PDFium adds to the appearance's resources is given
    /// here, and one this crate falls back to where the default resources
    /// lack the font: neither is the default resources'.
    pub(super) const ADDED: &str = "(added)";

    /// Places: a text's origin and a clip, in points.
    pub(super) const PLACE: f32 = 0.05;
    /// A path's points and a stroke's numbers.
    pub(super) const PATH: f32 = 0.01;
    /// A color's components, a size, a box, and a matrix.
    pub(super) const EXACT: f32 = 0.001;

    /// One annotation of a case's page, as made here.
    struct Annotation {
        /// What's made, or why nothing is.
        made: Result<Made, String>,
        /// The box less the border's reach, `[x y w h]`, turned the right
        /// way round.
        client: Option<[f32; 4]>,
    }

    /// The box less `annot`'s border's reach on every side, as `[x y w h]`:
    /// its rectangle's, turned by a quarter turn or three by `/MK /R`, as
    /// PDFium turns it (`abs(R % 360)`: either swaps the sides).
    pub(super) fn client_box(annot: &Annot<'_>) -> [f32; 4] {
        let border = annot.border();
        let width = border.width;
        let reach = match border.style {
            _ if !(width > 0.0 && width.is_finite()) => 0.0,
            BorderStyle::Beveled | BorderStyle::Inset => 2.0 * width,
            _ => width,
        };
        let rotation = match annot.typed() {
            TypedAnnot::Widget(widget) => widget.appearance_chars.as_ref().map(|mk| mk.rotation),
            _ => None,
        };
        let (w, h) = (annot.rect().width(), annot.rect().height());
        let (w, h) = match rotation.unwrap_or(0).unsigned_abs() % 360 {
            90 | 270 => (h, w),
            _ => (w, h),
        };
        let (x0, x1) = (reach.min(w - reach), reach.max(w - reach));
        let (y0, y1) = (reach.min(h - reach), reach.max(h - reach));
        [x0, y0, x1 - x0, y1 - y0]
    }

    /// A color as the record gives it, from the color space and components
    /// the state holds.
    pub(super) fn color(space: &ActiveColorSpace, values: &ColorValues, stroke: bool) -> Color {
        let op = match space {
            ActiveColorSpace::DeviceGray => "g",
            ActiveColorSpace::DeviceRgb => "rg",
            ActiveColorSpace::DeviceCmyk => "k",
            other => panic!("a color space the maker doesn't write: {other:?}"),
        };
        Color {
            op: if stroke { op.to_uppercase() } else { op.into() },
            components: values.as_slice().to_vec(),
        }
    }

    pub(super) fn painted(state: &State, path: &PathDrawn<'_>) -> Painted {
        let paint = match path.paint {
            Painting::Fill => Paint::Fill,
            Painting::FillEvenOdd => Paint::EvenOddFill,
            Painting::Stroke | Painting::CloseStroke => Paint::Stroke {
                width: state.line_width,
                cap: match state.line_cap {
                    LineCap::Butt => 0,
                    LineCap::Round => 1,
                    LineCap::Square => 2,
                },
                join: match state.line_join {
                    LineJoin::Miter => 0,
                    LineJoin::Round => 1,
                    LineJoin::Bevel => 2,
                },
                dash: state.dash.array.to_vec(),
                phase: state.dash.phase,
            },
            other => panic!("a paint the maker doesn't write: {other:?}"),
        };
        let stroke = matches!(paint, Paint::Stroke { .. });
        let color = match stroke {
            true => color(&state.stroke_color_space, &state.stroke_color, true),
            false => color(&state.fill_color_space, &state.fill_color, false),
        };
        Painted {
            paint,
            color,
            segments: segments(path),
        }
    }

    pub(super) fn segments(path: &PathDrawn<'_>) -> Vec<Segment> {
        path.segments
            .iter()
            .map(|seg| match *seg {
                Seg::Move(p) => Segment::Move([p.x, p.y]),
                Seg::Line(p) => Segment::Line([p.x, p.y]),
                Seg::Curve(a, b, p) => Segment::Curve([a.x, a.y, b.x, b.y, p.x, p.y]),
                Seg::Close => Segment::Close,
            })
            .collect()
    }

    /// `segments`' corners, where they're one closed rectangle with sides
    /// along the axes, as `re` places it: `[x0 y0 x1 y1]`.
    pub(super) fn rectangle(segments: &[Segment]) -> Option<[f32; 4]> {
        let [
            Segment::Move(a),
            Segment::Line(b),
            Segment::Line(c),
            Segment::Line(d),
            Segment::Line(e),
            Segment::Close,
        ] = segments
        else {
            return None;
        };
        let aligned = (a[1] == b[1] && b[0] == c[0] && c[1] == d[1] && d[0] == a[0])
            || (a[0] == b[0] && b[1] == c[1] && c[0] == d[0] && d[1] == a[1]);
        let aligned = aligned && a == e;
        aligned.then(|| {
            [
                a[0].min(c[0]),
                a[1].min(c[1]),
                a[0].max(c[0]),
                a[1].max(c[1]),
            ]
        })
    }

    /// `re`'s path, as the interpreter places it: a move to `(x, y)`, four
    /// lines round to it, and a close.
    fn rect_path([x, y, w, h]: [f32; 4]) -> Vec<Segment> {
        vec![
            Segment::Move([x, y]),
            Segment::Line([x + w, y]),
            Segment::Line([x + w, y + h]),
            Segment::Line([x, y + h]),
            Segment::Line([x, y]),
            Segment::Close,
        ]
    }

    /// PDFium's paths with each `re` as the path it places.
    pub(super) fn placed(paint: &[Painted]) -> Vec<Painted> {
        paint
            .iter()
            .map(|p| Painted {
                segments: p
                    .segments
                    .iter()
                    .flat_map(|s| match *s {
                        Segment::Rect(r) => rect_path(r),
                        ref other => vec![other.clone()],
                    })
                    .collect(),
                ..p.clone()
            })
            .collect()
    }

    /// The paths made here with a solid stroke round a rectangle as the
    /// region it covers, which PDFium fills: the ring between the
    /// rectangle grown and shrunk by half the width, even-odd, or where the
    /// width passes the rectangle's, the grown rectangle whole.
    pub(super) fn regions(paint: Vec<Painted>) -> Vec<Painted> {
        paint
            .into_iter()
            .map(|p| {
                let Paint::Stroke {
                    width, ref dash, ..
                } = p.paint
                else {
                    return p;
                };
                let Some([x0, y0, x1, y1]) = rectangle(&p.segments).filter(|_| dash.is_empty())
                else {
                    return p;
                };
                let half = width / 2.0;
                let (w, h) = (x1 - x0, y1 - y0);
                let mut segments = rect_path([x0 - half, y0 - half, w + width, h + width]);
                let fill = if w >= width && h >= width {
                    segments.extend(rect_path([x0 + half, y0 + half, w - width, h - width]));
                    Paint::EvenOddFill
                } else {
                    Paint::Fill
                };
                Painted {
                    paint: fill,
                    color: Color {
                        op: p.color.op.to_lowercase(),
                        components: p.color.components,
                    },
                    segments,
                }
            })
            .collect()
    }

    /// What's made for `widget` of `field`, interpreted and recorded; its
    /// font named by its `/DA` name, or [`ADDED`] for a fallback.
    fn record<'o>(
        form: &Form<'o>,
        field: &Field<'o>,
        widget: &Widget<'o>,
        objects: &'o Reading<&[u8]>,
    ) -> Result<Made, String> {
        let limits = Limits::DEFAULT;
        let read = appearance::widget_text(
            field,
            widget,
            objects,
            Rules::Pdfium,
            None,
            &limits,
            &mut (),
        )
        .map_err(|e| e.to_string())?;
        let made = appearance::text_for_widget(
            form,
            field,
            widget,
            objects,
            Rules::Pdfium,
            None,
            &limits,
            &mut (),
        )
        .map_err(|e| e.to_string())?;
        let font = match made.font.is_fallback() {
            true => ADDED.to_string(),
            false => String::from_utf8_lossy(read.font_name().unwrap()).into_owned(),
        };
        let ap = &made.appearance;

        // The test's binder: each resource the content needs under its
        // name, an object written already by reference.
        let mut fonts = Dict::new();
        for need in &ap.needs.0 {
            let bound = match need.res.body() {
                ResBody::Written(id) => Object::from(*id),
                ResBody::Objects(objects) => objects[0].clone(),
                other => panic!("a resource the maker doesn't name: {other:?}"),
            };
            fonts.set(need.name.clone(), bound);
        }
        let mut resources = Dict::new();
        resources.set("Font", fonts);
        let resources = Object::from(resources);
        let font_need = ap.needs.0.first().map(|need| need.name.clone());

        let (mut paint, mut tx, mut inside, mut stray) = (Vec::new(), None::<Tx>, false, false);
        let mut tf: Option<Vec<u8>> = None;
        let content = Concat::new(&[&ap.content]);
        pdf_gfx::interpret(
            objects,
            &content,
            Some(&resources),
            &pdf_gfx::Limits::DEFAULT,
            &Fonts::default(),
            Opts::default(),
            |event| {
                match event.kind {
                    Kind::Op if event.at.name() == b"Tf" => {
                        tf = event
                            .at
                            .operand(1)
                            .and_then(|o| o.name())
                            .map(|n| n.into_owned());
                    }
                    Kind::MarkedBegin(mark) if mark.tag == b"Tx" => {
                        inside = true;
                        tx = Some(Tx {
                            clip: None,
                            text: None,
                        });
                    }
                    Kind::MarkedEnd(mark) if mark.tag == b"Tx" => inside = false,
                    Kind::Path(path) if path.clip.is_some() && inside => {
                        let clip = rectangle(&segments(&path))
                            .map(|[x0, y0, x1, y1]| [x0, y0, x1 - x0, y1 - y0]);
                        stray |= clip.is_none();
                        if let Some(tx) = tx.as_mut() {
                            tx.clip = clip;
                        }
                    }
                    Kind::Path(path) if !inside => paint.push(painted(event.state, &path)),
                    Kind::Text(show) if inside => {
                        let named = tf.is_some() && tf == font_need;
                        let shown = Shown {
                            font: if named { font.clone() } else { "?".into() },
                            size: show.size,
                            at: [show.start.e, show.start.f],
                            codes: esc(show.string),
                        };
                        let fill = color(
                            &event.state.fill_color_space,
                            &event.state.fill_color,
                            false,
                        );
                        if let Some(tx) = tx.as_mut() {
                            tx.text.get_or_insert((fill, Vec::new())).1.push(shown);
                        }
                    }
                    Kind::Path(_) | Kind::Text(_) => stray = true,
                    _ => {}
                }
                ControlFlow::Continue(())
            },
        )
        .map_err(|e| format!("our content doesn't interpret: {e}"))?;
        if stray {
            return Err("our content paints inside /Tx, or clips outside it".into());
        }
        Ok(Made {
            bbox: [ap.bbox.x0, ap.bbox.y0, ap.bbox.x1, ap.bbox.y1],
            matrix: ap.matrix.to_array(),
            paint,
            tx,
        })
    }

    /// Each annotation of `name`'s page as made here, and the names of the
    /// default resources' fonts.
    fn made_for(name: &str) -> (Vec<Annotation>, HashSet<String>) {
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let mut opts = read::Options::DEFAULT;
        opts.recovery = Recovery::Pdfium;
        let reading = Reading::open(&bytes[..], &opts).unwrap();
        let objects = &reading;
        let catalog = pdf_forms::pdf_semantics::catalog(objects, reading.trailer().dict())
            .unwrap()
            .unwrap();
        let table = pages::pages(
            objects,
            catalog,
            Rules::Pdfium,
            &pdf_forms::pdf_semantics::Limits::DEFAULT,
            &mut (),
        )
        .unwrap();
        let page = table.iter().next().and_then(|page| page.dict).unwrap();
        let form = Form::read(objects, catalog, Rules::Pdfium, &Limits::DEFAULT)
            .unwrap()
            .unwrap();
        let fonts = form
            .font_names()
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .collect();
        let annots = page
            .get("Annots")
            .and_then(|object| match object.as_reference() {
                Some(id) => objects.resolve(id),
                None => Some(object),
            });
        let mut made = Vec::new();
        for entry in annots.and_then(Object::as_array).unwrap_or(&[]) {
            let id = entry.as_reference();
            let found = form.fields().iter().find_map(|field| {
                let widget = field
                    .widgets()
                    .iter()
                    .find(|w| w.id().is_some() && w.id() == id)?;
                Some((field, *widget))
            });
            made.push(match found {
                None => Annotation {
                    made: Err("no field's widget".into()),
                    client: None,
                },
                Some((field, widget)) => {
                    let annot = widget
                        .annot(objects, Rules::Pdfium, &Limits::DEFAULT)
                        .unwrap();
                    Annotation {
                        made: record(&form, field, &widget, objects),
                        client: Some(client_box(&annot)),
                    }
                }
            });
        }
        (made, fonts)
    }

    pub(super) fn near<const N: usize>(a: [f32; N], b: [f32; N], within: f32) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() <= within)
    }

    pub(super) fn same_color(a: &Color, b: &Color) -> bool {
        a.op == b.op
            && a.components.len() == b.components.len()
            && a.components
                .iter()
                .zip(&b.components)
                .all(|(a, b)| (a - b).abs() <= EXACT)
    }

    fn same_segments(a: &[Segment], b: &[Segment]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|pair| match pair {
                (Segment::Move(a), Segment::Move(b)) | (Segment::Line(a), Segment::Line(b)) => {
                    near(*a, *b, PATH)
                }
                (Segment::Curve(a), Segment::Curve(b)) => near(*a, *b, PATH),
                (Segment::Close, Segment::Close) => true,
                _ => false,
            })
    }

    /// A filled path's corners, where it's one polygon: a move and lines,
    /// closed or not (a fill closes it), its last corner dropped where it's
    /// back at the first.
    fn polygon(segments: &[Segment]) -> Option<Vec<[f32; 2]>> {
        let (Segment::Move(first), rest) = segments.split_first()? else {
            return None;
        };
        let rest = match rest.split_last() {
            Some((Segment::Close, before)) => before,
            _ => rest,
        };
        let mut corners = vec![*first];
        for segment in rest {
            let Segment::Line(p) = segment else {
                return None;
            };
            corners.push(*p);
        }
        if corners.len() > 1 && near(corners[0], corners[corners.len() - 1], PATH) {
            corners.pop();
        }
        Some(corners)
    }

    /// Whether two polygons have the same corners in the same ring, from
    /// any of them and either way round: the same region filled.
    fn same_polygon(a: &[[f32; 2]], b: &[[f32; 2]]) -> bool {
        let n = a.len();
        n == b.len()
            && (0..n).any(|from| {
                (0..n).all(|i| near(a[i], b[(from + i) % n], PATH))
                    || (0..n).all(|i| near(a[i], b[(from + n - i) % n], PATH))
            })
    }

    /// A dash array as the pattern it draws: one with an odd number of
    /// lengths repeats them, so `[3]` is `[3 3]` (ISO 32000-2, §8.4.3.6).
    fn pattern(dash: &[f32]) -> Vec<f32> {
        match dash.len() % 2 {
            1 => dash.iter().chain(dash).copied().collect(),
            _ => dash.to_vec(),
        }
    }

    pub(super) fn same_paint(pdfium: &[Painted], ours: &[Painted]) -> bool {
        pdfium.len() == ours.len()
            && pdfium.iter().zip(ours).all(|(a, b)| {
                let fill = matches!(
                    (&a.paint, &b.paint),
                    (Paint::Fill, Paint::Fill) | (Paint::EvenOddFill, Paint::EvenOddFill)
                );
                let region = match (polygon(&a.segments), polygon(&b.segments)) {
                    (Some(pa), Some(pb)) if fill => same_polygon(&pa, &pb),
                    _ => same_segments(&a.segments, &b.segments),
                };
                let paint = match (&a.paint, &b.paint) {
                    (Paint::Fill, Paint::Fill) | (Paint::EvenOddFill, Paint::EvenOddFill) => true,
                    (
                        Paint::Stroke {
                            width,
                            cap,
                            join,
                            dash,
                            phase,
                        },
                        Paint::Stroke {
                            width: w,
                            cap: c,
                            join: j,
                            dash: d,
                            phase: p,
                        },
                    ) => {
                        let (dash, d) = (pattern(dash), pattern(d));
                        (width - w).abs() <= PATH
                            && cap == c
                            && join == j
                            && dash.len() == d.len()
                            && dash.iter().zip(&d).all(|(a, b)| (a - b).abs() <= PATH)
                            && (phase - p).abs() <= PATH
                    }
                    _ => false,
                };
                paint && same_color(&a.color, &b.color) && region
            })
    }

    fn same_text(
        pdfium: &Option<(Color, Vec<Shown>)>,
        ours: &Option<(Color, Vec<Shown>)>,
        fonts: &HashSet<String>,
    ) -> bool {
        match (pdfium, ours) {
            (None, None) => true,
            (Some((pc, ps)), Some((oc, os))) => {
                same_color(pc, oc)
                    && ps.len() == os.len()
                    && ps.iter().zip(os).all(|(p, o)| {
                        let font = if fonts.contains(&p.font) {
                            p.font.as_str()
                        } else {
                            ADDED
                        };
                        font == o.font
                            && (p.size - o.size).abs() <= EXACT
                            && near(p.at, o.at, PLACE)
                            && p.codes == o.codes
                    })
            }
            _ => false,
        }
    }

    /// How often each rule applied, and held.
    #[derive(Default)]
    struct Held {
        clip: usize,
        tx: usize,
    }

    /// The parts in which what's made here for one annotation differs from
    /// PDFium's record of it.
    fn differing(
        pdfium: &Option<Made>,
        ours: &Annotation,
        fonts: &HashSet<String>,
        held: &mut Held,
    ) -> BTreeSet<&'static str> {
        let mut parts = BTreeSet::new();
        let (pdfium, made) = match (pdfium, &ours.made) {
            (None, Err(_)) => return parts,
            (Some(pdfium), Ok(made)) => (pdfium, made),
            _ => {
                parts.insert("made");
                return parts;
            }
        };
        if !near(pdfium.bbox, made.bbox, EXACT) {
            parts.insert("bbox");
        }
        if !near(pdfium.matrix, made.matrix, EXACT) {
            parts.insert("matrix");
        }
        if !same_paint(&placed(&pdfium.paint), &regions(made.paint.clone())) {
            parts.insert("paint");
        }
        match (&pdfium.tx, &made.tx) {
            // No `/Tx` in PDFium's: an empty one here.
            (
                None,
                Some(Tx {
                    clip: None,
                    text: None,
                }),
            ) => held.tx += 1,
            (None, Some(Tx { clip, text })) => {
                if clip.is_some() {
                    parts.insert("clip");
                }
                if text.is_some() {
                    parts.insert("text");
                }
            }
            (_, None) => {
                parts.insert("clip");
                parts.insert("text");
            }
            (Some(p), Some(o)) => {
                let clip = match (p.clip, o.clip) {
                    // No clip in PDFium's: the box less the border's reach.
                    (None, Some(o)) => {
                        let held_here = ours.client.is_some_and(|client| near(client, o, PLACE));
                        held.clip += usize::from(held_here);
                        held_here
                    }
                    (Some(p), Some(o)) => near(p, o, PLACE),
                    (_, None) => false,
                };
                if !clip {
                    parts.insert("clip");
                }
                if !same_text(&p.text, &o.text, fonts) {
                    parts.insert("text");
                }
            }
        }
        parts
    }

    /// Each `forms-ap` case's appearances, made by `text_for_widget` from the
    /// case's file read with pdf-structure, agree with PDFium's recorded
    /// ones in every part, or in every part but those `differ.tsv` lists
    /// for it, which must still differ: the size, every text origin within
    /// 0.05 pt, every string byte for byte, the box and the matrix, the
    /// clip, and each path painted under the text, in order, as the region
    /// it covers (a solid border's ring, each band's six corners from any
    /// of them, a line's ends, width, and dashes) within 0.01 pt in its
    /// color. Where PDFium writes no clip the clip here is the box less the
    /// border's reach, and where it writes no `/Tx` the `/Tx` here is
    /// empty, each held on every such case.
    #[test]
    fn made_appearances_agree_with_pdfiums() {
        let recorded: HashMap<String, String> = read_tsv::<2>("chromium-8152.txt")
            .into_iter()
            .map(|[k, v]| (k, v))
            .collect();
        let cases: Vec<String> = read_tsv::<3>("cases.tsv")
            .into_iter()
            .filter(|[_, kind, _]| kind == "forms-ap")
            .map(|[name, _, _]| name)
            .collect();

        // Each case's listed parts, each for one annotation (`part@N`) or any.
        let mut listed: HashMap<String, BTreeSet<(String, Option<usize>)>> = HashMap::new();
        let mut rules = BTreeSet::new();
        for line in differ_lines() {
            if line[0] == "forms-ap" {
                assert_eq!(line.len(), 3, "{line:?}");
                assert!(RULES.contains(&line[1].as_str()), "{line:?}");
                assert!(
                    rules.insert(line[1].clone()),
                    "a rule listed twice: {line:?}"
                );
            } else if cases.contains(&line[0]) {
                assert_eq!(line.len(), 3, "a forms-ap case's differ.tsv line: {line:?}");
                let (part, at) = match line[1].split_once('@') {
                    Some((part, at)) => (part, Some(at.parse().unwrap())),
                    None => (line[1].as_str(), None),
                };
                assert!(PARTS.contains(&part), "{line:?}");
                assert!(
                    listed
                        .entry(line[0].clone())
                        .or_default()
                        .insert((part.to_owned(), at)),
                    "a part listed twice: {line:?}"
                );
            }
        }
        assert_eq!(rules.len(), RULES.len(), "a differ.tsv line for each rule");

        let mut held = Held::default();
        let mut wrong = Vec::new();
        for name in &cases {
            let pdfium = made_appearances(&recorded[name]);
            let (ours, fonts) = made_for(name);
            assert_eq!(
                pdfium.len(),
                ours.len(),
                "{name}: a record for each annotation"
            );
            let mut found = BTreeSet::new();
            let mut said = Vec::new();
            for (index, (pdfium, ours)) in pdfium.iter().zip(&ours).enumerate() {
                let here = differing(pdfium, ours, &fonts, &mut held);
                if !here.is_empty() {
                    said.push(format!(
                        "  idx {index}: {here:?}\n    pdfium: {pdfium:?}\n    ours:   {:?}",
                        ours.made
                    ));
                }
                found.extend(here.into_iter().map(|part| (index, part)));
            }
            let expected = listed.remove(name).unwrap_or_default();
            let covers = |(part, at): &(String, Option<usize>),
                          (index, differs): &(usize, &str)| {
                part == differs && at.is_none_or(|at| at == *index)
            };
            let unlisted: Vec<_> = found
                .iter()
                .filter(|f| !expected.iter().any(|e| covers(e, f)))
                .collect();
            let agreeing: Vec<_> = expected
                .iter()
                .filter(|e| !found.iter().any(|f| covers(e, f)))
                .collect();
            if !unlisted.is_empty() || !agreeing.is_empty() {
                wrong.push(format!(
                    "{name}: differs unlisted in {unlisted:?}; listed but agrees: {agreeing:?}\n{}",
                    said.join("\n")
                ));
            }
        }
        assert!(
            listed.is_empty(),
            "differ.tsv lists cases that aren't forms-ap cases: {listed:?}"
        );
        if held.clip == 0 {
            wrong.push("the clip rule held on no case".into());
        }
        if held.tx == 0 {
            wrong.push("the /Tx rule held on no case".into());
        }
        assert!(
            wrong.is_empty(),
            "{} forms-ap case(s) disagreed with chromium/8152:\n{}",
            wrong.len(),
            wrong.join("\n")
        );
    }
}

/// The cases a test writes into `tests/pdfium/written` (`embed`), which
/// `cases.py` lists: `forms-ap` cases (`ap-*`) whose default resources hold a
/// TrueType program as `pdf_fonts::embed::write` writes it, a Type 0 font of
/// all its glyphs, under `FX`; and read-back cases (`back-*`, `fonts`), each a
/// widget whose appearance `appearance::text_with_fallback` makes in
/// Helvetica and that program, drawn on the page too, so that PDFium's text
/// page reads it back, the program written of the glyphs returned.
#[cfg(all(feature = "embed", feature = "text-layout"))]
mod written {
    use std::collections::{BTreeMap, HashMap};
    use std::fs;
    use std::path::PathBuf;

    use pdf_fixtures::fixture::{self, Body, Xref};
    use pdf_forms::appearance::{Fallback, FieldFont, TextField, WidgetLook, text_with_fallback};
    use pdf_forms::pdf_content_streams::{Concat, Res, ResBody};
    use pdf_forms::pdf_fonts::embed::{self, Glyphs, Program};
    use pdf_forms::pdf_fonts::{Fonts, Std14};
    use pdf_forms::pdf_gfx::{self, DefaultAppearance, TextPage, TextPageOpts};
    use pdf_forms::pdf_objects::{Dict, Object, ObjectId, Resolve, Streams, dict};
    use pdf_forms::{FieldFlags, Limits};
    use pdf_structure::{Reading, read};
    use unicode_normalization::UnicodeNormalization;

    use super::{read_tsv, set};

    /// A name WinAnsiEncoding has all but two characters of (`Ł` and `ễ`),
    /// which the program has.
    const VALUE: &str = "\u{141}ukasz Nguy\u{1ec5}n \u{d6}l\u{e7}er";

    const COMB: i64 = 1 << 24;
    const MULTILINE: i64 = 1 << 12;

    fn program() -> Program {
        Program::open(pdf_fixtures::font::latin(1000).build(), 0).unwrap()
    }

    /// `objects`, a font as `embed::write` writes it, from object `first` on,
    /// each reference `(i, 0)` among them made the `i`th's.
    fn placed(objects: &[Object], first: u32) -> Vec<(u32, Body)> {
        (first..)
            .zip(objects)
            .map(|(n, object)| {
                let mut object = object.clone();
                object.renumber(|to| match to.generation {
                    0 => Some(ObjectId::new(first + to.number, 0)),
                    _ => Some(to),
                });
                let body = match object.as_stream() {
                    Some(stream) => Body::stream(stream.dict.clone(), stream.data.bytes().unwrap()),
                    None => Body::Object(object),
                };
                (n, body)
            })
            .collect()
    }

    /// A file of one page whose annotations are `widgets`, each a text field
    /// that is its own widget, from object 5; its form's default resources
    /// `fonts`; `more` its other objects, and `page` the page's other
    /// entries.
    fn file(widgets: &[Dict], fonts: Dict, more: Vec<(u32, Body)>, page: Dict) -> Vec<u8> {
        let refs: Vec<Object> = (5..).take(widgets.len()).map(Object::reference).collect();
        let mut page_dict = dict! {
            "Type" => Object::name("Page"),
            "Parent" => Object::reference(2),
            "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(612), Object::from(792)],
            "Annots" => refs.clone(),
        };
        for (key, value) in page.iter() {
            page_dict.set(key.to_vec(), value.clone());
        }
        let mut bodies: Vec<(u32, Body)> = vec![
            (
                1,
                dict! {
                    "Type" => Object::name("Catalog"),
                    "Pages" => Object::reference(2),
                    "AcroForm" => dict! { "Fields" => refs, "DR" => dict! { "Font" => fonts } },
                }
                .into(),
            ),
            (
                2,
                dict! {
                    "Type" => Object::name("Pages"), "Kids" => vec![Object::reference(3)],
                    "Count" => 1,
                }
                .into(),
            ),
            (3, page_dict.into()),
        ];
        for (i, widget) in widgets.iter().enumerate() {
            let mut widget = widget.clone();
            widget.set("T", Object::text(&format!("f{i}")));
            widget.set("FT", Object::name("Tx"));
            widget.set("Type", Object::name("Annot"));
            widget.set("Subtype", Object::name("Widget"));
            widget.set("P", Object::reference(3));
            bodies.push((5 + i as u32, widget.into()));
        }
        bodies.extend(more);
        bodies.sort_by_key(|(n, _)| *n);
        fixture::file(
            "1.7",
            &bodies,
            dict! { "Root" => Object::reference(1) },
            Xref::Table,
        )
    }

    fn rect(x0: i32, y0: i32, x1: i32, y1: i32) -> Object {
        Object::from([x0, y0, x1, y1].map(Object::from).to_vec())
    }

    /// A text field's widget: its value, `/DA`, rectangle, and flags and
    /// `/MaxLen` where they're given.
    fn tx(value: &str, da: &str, rect: Object, flags: i64, max_len: Option<i64>) -> Dict {
        let mut widget = dict! {
            "V" => Object::text(value),
            "DA" => Object::text(da),
            "Rect" => rect,
        };
        if flags != 0 {
            widget.set("Ff", flags);
        }
        if let Some(n) = max_len {
            widget.set("MaxLen", n);
        }
        widget
    }

    /// A `forms-ap` case of `widgets` whose default resources hold
    /// Helvetica at 100 and the program, all its glyphs, as `FX` at 110.
    fn embedded(widgets: &[Dict]) -> Vec<u8> {
        let program = program();
        let mut all = Glyphs::new();
        let ascii: String = (0x20u8..0x7F).map(char::from).collect();
        all.add(&program, &ascii).unwrap();
        all.add(&program, "\u{e9}\u{e7}\u{d6}\u{141}\u{1ec5}\u{301}")
            .unwrap();
        let fx = embed::write(&program, &all, &embed::Options::default()).unwrap();
        let mut more = placed(&fx, 110);
        more.push((100, Std14::Helvetica.dict().into()));
        let fonts = dict! { "FX" => Object::reference(110), "Helv" => Object::reference(100) };
        file(widgets, fonts, more, Dict::new())
    }

    /// A read-back case: one widget at `[0 100 w 100 + h]`, `value` in
    /// Helvetica and the program at `size` (`0` automatic), its appearance
    /// made with `flags` and `max_len`, drawn on the page by its content too;
    /// the program, where anything's drawn in it, written of its glyphs at
    /// 110. The file, and what's read back of it: its value composed, and
    /// the appearance's content.
    fn read_back(
        value: &str,
        (w, h): (f32, f32),
        size: f32,
        flags: i64,
        max_len: Option<u32>,
    ) -> (Vec<u8>, Option<Back>) {
        let program = program();
        let font = FieldFont::standard(Std14::Helvetica);
        let fallback = Fallback::new(Res::written(ObjectId::new(110, 0)), &program);
        let mut field = TextField::new(value);
        field.flags = FieldFlags::from_bits(flags as u32);
        field.max_len = max_len;
        let mut look = WidgetLook::new(w, h);
        look.border_color = Some(pdf_forms::pdf_content_streams::Color::Gray(0.0));
        let da = DefaultAppearance::new(
            "Helv",
            size,
            Some(pdf_forms::pdf_content_streams::Color::Gray(0.0)),
        );
        let made = text_with_fallback(
            &field,
            &look,
            &da,
            font.res().unwrap(),
            &font.layout(),
            &fallback,
            &Limits::DEFAULT,
        )
        .unwrap();
        assert!(!made.appearance.overflowed, "{value:?}");

        let mut names = Dict::new();
        let mut more = Vec::new();
        for need in &made.appearance.needs.0 {
            let bound = match need.res.body() {
                ResBody::Written(id) => Object::from(*id),
                ResBody::Objects(_) => Object::reference(100),
                other => panic!("{other:?}"),
            };
            names.set(need.name.clone(), bound);
        }
        more.push((100, Std14::Helvetica.dict().into()));
        if !made.glyphs.is_empty() {
            let fx = embed::write(&program, &made.glyphs, &embed::Options::default()).unwrap();
            more.extend(placed(&fx, 110));
        }
        let appearance = &made.appearance;
        let [a, b, c, d, e, f] = appearance.matrix.to_array();
        let bbox = appearance.bbox;
        let numbers =
            |v: &[f32]| Object::from(v.iter().map(|&n| Object::from(n)).collect::<Vec<_>>());
        more.push((
            50,
            Body::stream(
                dict! {
                    "Type" => Object::name("XObject"),
                    "Subtype" => Object::name("Form"),
                    "BBox" => numbers(&[bbox.x0, bbox.y0, bbox.x1, bbox.y1]),
                    "Matrix" => numbers(&[a, b, c, d, e, f]),
                    "Resources" => dict! { "Font" => names },
                },
                &appearance.content,
            ),
        ));
        more.push((4, Body::stream(Dict::new(), b"q 1 0 0 1 0 100 cm /Ap Do Q")));
        let mut widget = tx(
            value,
            &format!("/Helv {size} Tf 0 g"),
            Object::from(0),
            flags,
            max_len.map(i64::from),
        );
        widget.set("Rect", numbers(&[0.0, 100.0, w, 100.0 + h]));
        widget.set("AP", dict! { "N" => Object::reference(50) });
        widget.set("MK", dict! { "BC" => vec![Object::from(0)] });
        let page = dict! {
            "Contents" => Object::reference(4),
            "Resources" => dict! { "XObject" => dict! { "Ap" => Object::reference(50) } },
        };
        let back = Back {
            value: value.nfc().collect(),
            content: made.appearance.content.clone(),
        };
        let file = file(
            &[widget],
            dict! { "Helv" => Object::reference(100) },
            more,
            page,
        );
        (file, Some(back))
    }

    /// What a read-back case shows: its value composed, and its appearance's
    /// content.
    struct Back {
        value: String,
        content: Vec<u8>,
    }

    /// Each written case: its name, its file, and for a read-back case what
    /// it shows.
    fn written_cases() -> Vec<(&'static str, Vec<u8>, Option<Back>)> {
        let fx = |size: i32| format!("/FX {size} Tf 0 g");
        let nfd: String = VALUE.nfd().collect();
        let long = format!("{VALUE} {VALUE}");
        let back = |name, value: &str, boxed, flags, max_len| {
            let (file, back) = read_back(value, boxed, 12.0, flags, max_len);
            (name, file, back)
        };
        let back_sized = |name, value: &str, boxed, size, flags| {
            let (file, back) = read_back(value, boxed, size, flags, None);
            (name, file, back)
        };
        vec![
            (
                "ap-embedded-line",
                embedded(&[
                    tx("Hello 123", &fx(12), rect(10, 10, 290, 30), 0, None),
                    tx(
                        "\u{d6}l\u{e7}er \u{e9}",
                        &fx(12),
                        rect(10, 40, 290, 60),
                        0,
                        None,
                    ),
                    tx("Hello 123", &fx(0), rect(10, 70, 290, 100), 0, None),
                    tx("Hg", &fx(100), rect(10, 110, 290, 260), 0, None),
                ]),
                None,
            ),
            (
                "ap-embedded-multiline",
                embedded(&[
                    tx("Hg", &fx(100), rect(10, 10, 290, 312), MULTILINE, None),
                    tx(
                        "Hello 123\nNguyen Olcer",
                        &fx(0),
                        rect(10, 320, 290, 380),
                        MULTILINE,
                        None,
                    ),
                ]),
                None,
            ),
            (
                "ap-embedded-comb",
                embedded(&[
                    tx("Hello", &fx(12), rect(10, 10, 290, 30), COMB, Some(10)),
                    tx("Hello", &fx(0), rect(10, 40, 290, 60), COMB, Some(10)),
                ]),
                None,
            ),
            (
                "ap-comb-cluster",
                embedded(&[
                    tx(
                        "e\u{301}e\u{301}ab",
                        &fx(12),
                        rect(10, 10, 290, 30),
                        COMB,
                        Some(6),
                    ),
                    tx(
                        "e\u{301}e\u{301}ab",
                        "/Helv 12 Tf 0 g",
                        rect(10, 40, 290, 60),
                        COMB,
                        Some(6),
                    ),
                ]),
                None,
            ),
            back("back-mixed", VALUE, (280.0, 20.0), 0, None),
            back("back-nfd", &nfd, (280.0, 20.0), 0, None),
            back("back-comb", VALUE, (380.0, 20.0), COMB, Some(19)),
            back_sized("back-multiline", &long, (130.0, 60.0), 10.0, MULTILINE),
            back("back-winansi", "Zo\u{eb} Caf\u{e9}", (280.0, 20.0), 0, None),
        ]
    }

    fn written_folder() -> PathBuf {
        set().join("written")
    }

    /// Writes `tests/pdfium/written`: run it when the maker or a case
    /// changes, then `just pdfium-answers`.
    #[test]
    #[ignore = "writes into the source tree"]
    fn the_written_cases_are_written() {
        let folder = written_folder();
        fs::create_dir_all(&folder).unwrap();
        for file in fs::read_dir(&folder).unwrap() {
            fs::remove_file(file.unwrap().path()).unwrap();
        }
        for (name, file, _) in written_cases() {
            fs::write(folder.join(format!("{name}.pdf")), file).unwrap();
        }
    }

    /// The oracle's written files are what the maker makes, and no others; a
    /// value given decomposed is drawn as its composed form is, the same
    /// bytes, and one Helvetica draws whole embeds no program.
    #[test]
    fn the_written_cases_are_the_maker() {
        let cases = written_cases();
        let case = |name: &str| cases.iter().find(|case| case.0 == name).unwrap();
        let file = |name: &str| &case(name).1;
        let content = |name: &str| &case(name).2.as_ref().unwrap().content;
        // The same appearance, and the same font written of the same glyphs.
        assert_eq!(content("back-mixed"), content("back-nfd"));
        let font = |name: &str| {
            let file = file(name);
            let at = file
                .windows(12)
                .position(|w| w == b"110 0 obj\n<<")
                .unwrap();
            let to = at + file[at..].windows(5).position(|w| w == b"xref\n").unwrap();
            file[at..to].to_vec()
        };
        assert_eq!(font("back-mixed"), font("back-nfd"));
        let winansi = file("back-winansi");
        assert!(!winansi.windows(9).any(|w| w == b"FontFile2"));
        assert!(file("back-mixed").windows(9).any(|w| w == b"FontFile2"));

        let folder = written_folder();
        if !folder.exists() {
            return;
        }
        let mut names: Vec<String> = (fs::read_dir(&folder).unwrap())
            .map(|file| file.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let mut expected: Vec<String> = cases
            .iter()
            .map(|(name, ..)| format!("{name}.pdf"))
            .collect();
        expected.sort();
        assert_eq!(names, expected, "run the_written_cases_are_written");
        for (name, file, _) in &cases {
            let written = fs::read(folder.join(format!("{name}.pdf"))).unwrap();
            assert!(
                written == *file,
                "{name}: run the_written_cases_are_written"
            );
        }
    }

    /// Where a text page puts a character: within 0.05 pt, the `forms-ap`
    /// cases' bound for a string's origin.
    const PLACE: f32 = 0.05;

    /// Each read-back case's characters, as PDFium's text page reads them
    /// from the page (`fonts`: each one's character and origin), are the
    /// value composed, each where the appearance draws it: pdf-gfx's text
    /// page of the same file, every character it reads drawn within 0.05 pt
    /// of PDFium's, PDFium's spaces and line breaks between them passed over
    /// where its text page adds them. A space is held by its place in the
    /// text alone: where it's drawn after a gap (a comb cell's), PDFium's text
    /// page reads one at the gap's start, where the glyph before it ends.
    #[test]
    fn read_back_cases_agree() {
        let recorded: HashMap<String, String> = read_tsv::<2>("chromium-8152.txt")
            .into_iter()
            .map(|[k, v]| (k, v))
            .collect();
        let values: BTreeMap<&str, String> = written_cases()
            .into_iter()
            .filter_map(|(name, _, back)| Some((name, back?.value)))
            .collect();
        let cases: Vec<String> = read_tsv::<3>("cases.tsv")
            .into_iter()
            .filter(|[_, kind, _]| kind == "fonts")
            .map(|[name, _, _]| name)
            .collect();
        assert_eq!(cases.len(), values.len(), "a case for each read-back file");
        let mut wrong = Vec::new();
        for name in &cases {
            let value = &values[name.as_str()];
            let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
            let reading = Reading::open(&bytes[..], &read::Options::DEFAULT).unwrap();
            let page = (&reading)
                .resolve(ObjectId::new(3, 0))
                .and_then(Object::as_dict)
                .unwrap();
            let content = page
                .get(b"Contents")
                .and_then(Object::as_reference)
                .unwrap();
            let content = reading.decoded(content, 1 << 20).unwrap();
            let resources = page.get(b"Resources").unwrap();
            let text = TextPage::extract(
                &reading,
                &Concat::new(&[&content]),
                Some(resources),
                &pdf_gfx::Limits::DEFAULT,
                &Fonts::default(),
                TextPageOpts::default(),
            )
            .unwrap();
            let ours: Vec<(char, [f32; 2])> = (text.chars().iter())
                .filter(|c| !c.generated)
                .map(|c| (c.ch, [c.origin.x, c.origin.y - 100.0]))
                .collect();
            let read: String = ours.iter().map(|(ch, _)| ch).collect();
            if read != *value {
                wrong.push(format!("{name}: read back here as {read:?}, not {value:?}"));
                continue;
            }
            // PDFium's characters, each its code point and origin.
            let pdfium: Vec<(char, [f32; 2])> = recorded[name.as_str()]
                .split(" ; ")
                .map(|entry| {
                    let words: Vec<&str> = entry.split(' ').collect();
                    let ch = char::from_u32(u32::from_str_radix(words[0], 16).unwrap()).unwrap();
                    (ch, [words[1].parse().unwrap(), words[2].parse().unwrap()])
                })
                .collect();
            let mut theirs = pdfium.iter();
            for &(ch, at) in &ours {
                let found = theirs.find(|(c, _)| *c == ch || !matches!(c, ' ' | '\r' | '\n'));
                match found {
                    Some(&(c, p)) if c == ch => {
                        let far = (p[0] - at[0]).abs() > PLACE || (p[1] - at[1]).abs() > PLACE;
                        if far && ch != ' ' {
                            wrong.push(format!("{name}: {ch:?} at {at:?} here, {p:?} in PDFium"));
                        }
                    }
                    other => wrong.push(format!("{name}: {ch:?} here, {other:?} in PDFium")),
                }
            }
            if theirs.any(|(c, _)| !matches!(c, ' ' | '\r' | '\n')) {
                wrong.push(format!("{name}: PDFium reads more than {value:?}"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
