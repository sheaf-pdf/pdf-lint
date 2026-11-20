//! Holds `pdf-forms`'s `Rules::Pdfium` reading of the field tree to PDFium's
//! answers recorded at `chromium/8152` in `tests/pdfium/chromium-8152.txt`:
//! for each widget of a page, its field's type, flags, fully qualified name,
//! and a text field's value (the `forms` cases). The `forms-ap` cases record
//! the appearance PDFium makes for a text field that has none; their answers
//! are read here into what each widget's appearance paints, clips and shows,
//! and the heights of the faces PDFium draws the standard 14 in are held to
//! them.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use pdf_forms::pdf_objects::pdf_tokenizer::Rules;
use pdf_forms::pdf_objects::{Object, Resolve};
use pdf_forms::pdf_semantics::pages;
use pdf_forms::{Field, FieldKind, FieldValue, Form, Limits};
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

#[test]
fn pdfium_recorded_cases_agree() {
    let recorded: HashMap<String, String> = read_tsv::<2>("chromium-8152.txt")
        .into_iter()
        .map(|[k, v]| (k, v))
        .collect();
    let differs: HashMap<String, String> = read_tsv::<2>("differ.tsv")
        .into_iter()
        .map(|[k, why]| (k, why))
        .collect();

    let cases = read_tsv::<3>("cases.tsv");
    assert!(!cases.is_empty(), "cases.tsv must not be empty");

    let mut wrong = Vec::new();
    for [name, kind, _files] in &cases {
        assert!(
            kind == "forms" || kind == "forms-ap",
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

    for k in differs.keys() {
        assert!(
            recorded.contains_key(k),
            "differ.tsv names unknown case {k}"
        );
    }

    assert!(
        wrong.is_empty(),
        "{} case(s) disagreed with chromium/8152:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// A color as a `forms-ap` record gives it: the operator that set it (`g`,
/// `rg`, `k`, or `G`, `RG`, `K` for a stroke) and its components.
#[derive(Debug, PartialEq)]
struct Color {
    op: String,
    components: Vec<f32>,
}

/// How a path of a made appearance is painted.
#[derive(Debug, PartialEq)]
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
#[derive(Debug, PartialEq)]
enum Segment {
    Rect([f32; 4]),
    Move([f32; 2]),
    Line([f32; 2]),
    Curve([f32; 6]),
    Close,
}

/// A path painted before the appearance's `/Tx` sequence.
#[derive(Debug, PartialEq)]
struct Painted {
    paint: Paint,
    color: Color,
    segments: Vec<Segment>,
}

/// A `Tj` in the `/Tx` sequence: its font's resource name (`%` where no `Tf`
/// came first), size, origin, and codes as `esc` writes them.
#[derive(Debug, PartialEq)]
struct Shown {
    font: String,
    size: f32,
    at: [f32; 2],
    codes: String,
}

/// What the `/Tx` sequence holds: its clip rectangle (none where it has none),
/// and the color of its text with each string shown (none where it shows none).
#[derive(Debug, PartialEq)]
struct Tx {
    clip: Option<[f32; 4]>,
    text: Option<(Color, Vec<Shown>)>,
}

/// A made appearance, in the record's five parts.
#[derive(Debug, PartialEq)]
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
