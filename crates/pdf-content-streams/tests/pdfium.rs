//! Content read as PDFium reads it, held to PDFium's own reading, case by
//! case. Each case is in `tests/pdfium` (`cases.tsv`, its bytes in
//! `cases/`, written by `cases.py`): a content stream (`.content`) and the
//! page's resources (`.res`, `<< >>` where there's none), with PDFium's
//! answer for it recorded beside it at the pinned build (`chromium-N.txt`,
//! by the repository's `oracle/pdfium.py`, which `oracle/README.md`
//! describes): the page objects PDFium makes of it, in order. Here the
//! same objects are made from the crate's operations by a small
//! interpreter, and the two must agree, but for the cases `differ.tsv`
//! names, which must still differ.
//!
//! The interpreter makes what PDFium's content parser makes, as far as the
//! cases need:
//! - a path object at a painting operator (`S`, `s`, `f`, `F`, `f*`, `B`,
//!   `B*`, `b`, `b*`; not `n`, nor a clip alone), from `m` and `l` (each
//!   with two operands), `c`, `v`, `y` (their points from the last
//!   operands, 0 where there's none), `h` and `re`; its points added and
//!   dropped as PDFium adds and drops them, a lone point dropped but where
//!   a closed move draws a dot with round caps (`J` 1); and after an `m`,
//!   the path operators that follow it read as PDFium reads them there:
//!   their numbers from the first, six at most, a missing one the last
//!   read, until anything else. Its matrix is the CTM (`cm`, `q`, `Q`), and
//!   its dash array `d`'s.
//! - a text object at each `Tj`, `'`, `"` and `TJ` that shows a string,
//!   once `Tf` has set a font: the font's `/BaseFont` where the resources
//!   have the name, and PDFium's stock font, Helvetica, where they don't;
//!   its size `Tf`'s; its text the strings' bytes, each the character it
//!   is in Latin-1 (the cases' strings are ASCII, which a standard font
//!   shows as itself), none where the size is 0 (PDFium's text of a text
//!   object then has none), and a space between two strings of a `TJ` that
//!   a negative number between them moves apart, as PDFium's text puts one
//!   for a gap.
//! - an image object at an inline image, its matrix the CTM, its data what
//!   [`InlineImage::stream`] says PDFium keeps, and none where it says
//!   PDFium keeps no stream.
//! - each object's marked content, as `BDC`, `BMC` and `EMC` nest around
//!   it: each mark's `/MCID`, where its property list gives one as a
//!   number, as PDFium reads an integer from it.
//!
//! Numbers are compared as `f32`s. Text and data longer than 64 bytes are
//! compared by their length and hash, as the oracle writes them.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use pdf_content_streams::pdf_objects::parse::Parser;
use pdf_content_streams::pdf_objects::{Dict, Object, Rules, Str, Value};
use pdf_content_streams::{ImageStream, Limits, Operand, ResCx, family_components, operations};

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

/// A matrix, `a b c d e f`.
#[derive(Clone, Copy, PartialEq)]
struct M([f32; 6]);

impl M {
    const ID: M = M([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    /// `self * r`, as PDFium's matrices multiply.
    fn mul(self, r: M) -> M {
        let [a, b, c, d, e, f] = self.0;
        let [ra, rb, rc, rd, re, rf] = r.0;
        M([
            a * ra + b * rc,
            a * rb + b * rd,
            c * ra + d * rc,
            c * rb + d * rd,
            e * ra + f * rc + re,
            e * rb + f * rd + rf,
        ])
    }

    fn words(self) -> String {
        let [a, b, c, d, e, f] = self.0;
        format!("matrix {a} {b} {c} {d} {e} {f}")
    }
}

/// What `q` saves and `Q` restores.
#[derive(Clone)]
struct State {
    ctm: M,
    round_cap: bool,
    dash: Option<(Vec<f32>, f32)>,
    font: Option<Vec<u8>>,
    size: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Seg {
    Line,
    Bezier,
    Move,
}

#[derive(Clone, Copy)]
struct Point {
    x: f32,
    y: f32,
    seg: Seg,
    close: bool,
}

/// The page's resources, as PDFium's object parser reads them.
struct Res {
    fonts: HashMap<Vec<u8>, Vec<u8>>,
    spaces: Vec<(Vec<u8>, u32)>,
}

impl Res {
    fn read(bytes: &[u8]) -> Res {
        let dict = Parser::at(bytes, 0)
            .rules(Rules::Pdfium)
            .value(&mut ())
            .ok()
            .and_then(|o| o.as_dict().cloned())
            .unwrap_or_default();
        let sub = |key: &[u8]| -> Dict {
            dict.get(key)
                .and_then(Object::as_dict)
                .cloned()
                .unwrap_or_default()
        };
        let fonts = (sub(b"Font").iter())
            .filter_map(|(k, v)| {
                let base = v.as_dict()?.get(b"BaseFont")?.as_name()?;
                Some((k.to_vec(), base.to_vec()))
            })
            .collect();
        let spaces = (sub(b"ColorSpace").iter())
            .filter_map(|(k, v)| {
                let family = match v.value() {
                    Value::Name(n) => n.clone(),
                    Value::Array(a) => a.first()?.as_name()?.to_vec(),
                    _ => return None,
                };
                Some((k.to_vec(), family_components(&family)))
            })
            .collect();
        Res { fonts, spaces }
    }
}

/// The page objects the crate's reading of `content` makes, one a line.
struct Reader<'r> {
    res: &'r Res,
    out: Vec<String>,
    st: State,
    stack: Vec<State>,
    points: Vec<Point>,
    start: (f32, f32),
    current: (f32, f32),
    clip: bool,
    /// The marks open, each its `/MCID`, where it has one.
    marks: Vec<Option<i32>>,
}

/// An operand's string, as shown: a string's bytes, read as PDFium reads
/// them.
fn shown(o: Option<Operand>) -> Option<Vec<u8>> {
    match o?.object(&Limits::DEFAULT)?.into_value() {
        Value::String(Str::Bytes(b), _) => Some(b),
        _ => None,
    }
}

impl Reader<'_> {
    fn push(&mut self, object: String) {
        let mut object = object;
        for mark in &self.marks {
            match mark {
                Some(n) => object.push_str(&format!(" mcid {n}")),
                None => object.push_str(" mcid -"),
            }
        }
        self.out.push(object);
    }

    fn add_point(&mut self, (x, y): (f32, f32), seg: Seg) {
        if seg == Seg::Move
            && self
                .points
                .last()
                .is_some_and(|l| !l.close && l.seg == Seg::Move)
            && self.current == (x, y)
        {
            return;
        }
        self.current = (x, y);
        if seg == Seg::Move {
            self.start = (x, y);
            if let Some(l) = self
                .points
                .last_mut()
                .filter(|l| l.seg == Seg::Move && !l.close)
            {
                l.x = x;
                l.y = y;
                return;
            }
        } else if self.points.is_empty() {
            return;
        }
        self.points.push(Point {
            x,
            y,
            seg,
            close: false,
        });
    }

    fn add_point_and_close(&mut self, (x, y): (f32, f32), seg: Seg) {
        self.current = (x, y);
        if self.points.is_empty() {
            return;
        }
        self.points.push(Point {
            x,
            y,
            seg,
            close: true,
        });
    }

    fn close_path(&mut self) {
        if self.points.is_empty() {
            return;
        }
        if self.start != self.current {
            self.add_point_and_close(self.start, Seg::Line);
        } else if let Some(l) = self.points.last_mut() {
            l.close = true;
        }
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.add_point((x, y), Seg::Move);
        self.add_point((x + w, y), Seg::Line);
        self.add_point((x + w, y + h), Seg::Line);
        self.add_point((x, y + h), Seg::Line);
        self.add_point_and_close((x, y), Seg::Line);
    }

    /// A path object: `fill` 0 none, 1 even-odd, 2 nonzero.
    fn path_object(&mut self, fill: i32, stroke: bool) {
        let mut points = std::mem::take(&mut self.points);
        let clip = std::mem::take(&mut self.clip);
        if points.is_empty() {
            return;
        }
        if points.len() == 1 {
            if clip {
                return;
            }
            let p = points[0];
            if p.seg != Seg::Move || !p.close || !self.st.round_cap {
                return;
            }
            points[0].close = false;
            points.push(Point {
                close: true,
                seg: Seg::Line,
                ..p
            });
        }
        if points
            .last()
            .is_some_and(|l| l.seg == Seg::Move && !l.close)
        {
            points.pop();
        }
        if !stroke && fill == 0 {
            return;
        }
        let mut object = format!(
            "path fill {fill} stroke {} {}",
            u8::from(stroke),
            self.st.ctm.mul(M::ID).words()
        );
        for p in &points {
            let seg = match p.seg {
                Seg::Line => "l",
                Seg::Bezier => "c",
                Seg::Move => "m",
            };
            object.push_str(&format!(" {seg} {} {}", p.x, p.y));
            if p.close {
                object.push_str(" h");
            }
        }
        if let Some((dash, phase)) = self.st.dash.as_ref().filter(|(d, _)| !d.is_empty()) {
            object.push_str(" dash");
            for d in dash {
                object.push_str(&format!(" {d}"));
            }
            object.push_str(&format!(" phase {phase}"));
        }
        self.push(object);
    }

    /// A text object of `strings`, a space between two where `gap` says.
    fn text_object(&mut self, strings: &[(Vec<u8>, bool)]) {
        let Some(font) = self.st.font.clone() else {
            return;
        };
        if strings.is_empty() {
            return;
        }
        let mut text = String::new();
        if self.st.size != 0.0 {
            for (s, gap) in strings {
                if *gap && !text.is_empty() {
                    text.push(' ');
                }
                text.extend(s.iter().map(|&b| char::from(b)));
            }
        }
        let base = self
            .res
            .fonts
            .get(&font)
            .map_or(&b"Helvetica"[..], Vec::as_slice);
        let object = format!(
            "text font {} size {} text {}",
            esc(base),
            self.st.size,
            esc(text.as_bytes())
        );
        self.push(object);
    }

    fn show(&mut self, string: Option<Vec<u8>>) {
        if let Some(s) = string.filter(|s| !s.is_empty()) {
            self.text_object(&[(s, false)]);
        }
    }

    fn read(&mut self, content: &[u8]) {
        let res = ResCx::new(self.res.spaces.iter().cloned());
        let lim = Limits::DEFAULT;
        let mut fast: Option<[f32; 6]> = None;
        for op in operations(content, &res, &lim) {
            let Ok(op) = op else { break };
            let n = |i: usize| op.arg(i).map_or(0.0, |o| o.number());
            let count = op.operands().len();
            let name = op.name();
            // After `m`, path operators read as PDFium reads them there,
            // while their operands are numbers.
            if let Some(params) = fast.as_mut() {
                let path_op = matches!(name, b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re");
                if path_op && op.inline_image().is_none() && op.operands().all(|o| o.is_number()) {
                    for (k, o) in op.operands().take(6).enumerate() {
                        params[k] = o.number();
                    }
                    let p = *params;
                    match name {
                        b"m" => self.add_point((p[0], p[1]), Seg::Move),
                        b"l" => self.add_point((p[0], p[1]), Seg::Line),
                        b"c" => {
                            self.add_point((p[0], p[1]), Seg::Bezier);
                            self.add_point((p[2], p[3]), Seg::Bezier);
                            self.add_point((p[4], p[5]), Seg::Bezier);
                        }
                        b"v" => {
                            self.add_point(self.current, Seg::Bezier);
                            self.add_point((p[0], p[1]), Seg::Bezier);
                            self.add_point((p[2], p[3]), Seg::Bezier);
                        }
                        b"y" => {
                            self.add_point((p[0], p[1]), Seg::Bezier);
                            self.add_point((p[2], p[3]), Seg::Bezier);
                            self.add_point((p[2], p[3]), Seg::Bezier);
                        }
                        b"h" => self.close_path(),
                        _ => self.rect(p[0], p[1], p[2], p[3]),
                    }
                    continue;
                }
                fast = None;
            }
            if let Some(image) = op.inline_image() {
                let data = match image.stream() {
                    ImageStream::Data(r) => content[r].to_vec(),
                    ImageStream::Empty => Vec::new(),
                    _ => continue,
                };
                let object = format!(
                    "image {} raw {}",
                    self.st.ctm.mul(M::ID).words(),
                    esc(&data)
                );
                self.push(object);
                continue;
            }
            match name {
                b"q" => self.stack.push(self.st.clone()),
                b"Q" => {
                    if let Some(s) = self.stack.pop() {
                        self.st = s;
                    }
                }
                b"cm" => {
                    let m = M([n(5), n(4), n(3), n(2), n(1), n(0)]);
                    self.st.ctm = m.mul(self.st.ctm);
                }
                b"J" => {
                    let v = n(0);
                    let cap = if v.is_nan() || !(i32::MIN as f32..=i32::MAX as f32).contains(&v) {
                        0
                    } else {
                        v as i32
                    };
                    self.st.round_cap = cap == 1;
                }
                b"d" => {
                    let dash = op
                        .arg(1)
                        .filter(Operand::is_array)
                        .map(|a| a.elements().map(|e| e.number()).collect());
                    self.st.dash = Some((dash.unwrap_or_default(), n(0)));
                }
                b"m" if count == 2 => {
                    self.add_point((n(1), n(0)), Seg::Move);
                    fast = Some([0.0; 6]);
                }
                b"l" if count == 2 => self.add_point((n(1), n(0)), Seg::Line),
                b"c" => {
                    self.add_point((n(5), n(4)), Seg::Bezier);
                    self.add_point((n(3), n(2)), Seg::Bezier);
                    self.add_point((n(1), n(0)), Seg::Bezier);
                }
                b"v" => {
                    self.add_point(self.current, Seg::Bezier);
                    self.add_point((n(3), n(2)), Seg::Bezier);
                    self.add_point((n(1), n(0)), Seg::Bezier);
                }
                b"y" => {
                    self.add_point((n(3), n(2)), Seg::Bezier);
                    self.add_point((n(1), n(0)), Seg::Bezier);
                    self.add_point((n(1), n(0)), Seg::Bezier);
                }
                b"h" => self.close_path(),
                b"re" => self.rect(n(3), n(2), n(1), n(0)),
                b"S" => self.path_object(0, true),
                b"s" => {
                    self.close_path();
                    self.path_object(0, true);
                }
                b"f" | b"F" => self.path_object(2, false),
                b"f*" => self.path_object(1, false),
                b"B" => self.path_object(2, true),
                b"B*" => self.path_object(1, true),
                b"b" => {
                    self.close_path();
                    self.path_object(2, true);
                }
                b"b*" => {
                    self.add_point_and_close(self.start, Seg::Line);
                    self.path_object(1, true);
                }
                b"n" => self.path_object(0, false),
                b"W" | b"W*" => self.clip = true,
                b"Tf" => {
                    self.st.size = n(0);
                    let font = op.arg(1).and_then(|o| o.name()).map(|n| n.into_owned());
                    self.st.font = Some(font.unwrap_or_default());
                }
                b"Tj" | b"'" => {
                    let s = shown(op.arg(0));
                    self.show(s);
                }
                b"\"" => {
                    let s = shown(op.arg(0));
                    self.show(s);
                }
                b"TJ" => {
                    let Some(array) = op.arg(0).filter(Operand::is_array) else {
                        continue;
                    };
                    let mut strings = Vec::new();
                    let mut gap = false;
                    for e in array.elements() {
                        if e.is_string() {
                            if let Some(s) = shown(Some(e)).filter(|s| !s.is_empty()) {
                                strings.push((s, gap));
                                gap = false;
                            }
                        } else if e.number() < 0.0 {
                            gap = true;
                        }
                    }
                    self.text_object(&strings);
                }
                b"BDC" => {
                    let mcid = (op.arg(0))
                        .and_then(|o| o.object(&Limits::DEFAULT))
                        .and_then(|o| o.as_dict().and_then(|d| d.get(b"MCID")).cloned())
                        .and_then(|v| match v.into_value() {
                            Value::Integer(i) => Some(i32::try_from(i).unwrap_or(0)),
                            Value::Real(r) => Some(r as i32),
                            _ => None,
                        });
                    self.marks.push(mcid);
                }
                b"BMC" => self.marks.push(None),
                b"EMC" => {
                    self.marks.pop();
                }
                _ => {}
            }
        }
    }
}

/// The crate's answer for a case: its page objects, as the oracle writes
/// PDFium's.
fn answer(content: &[u8], res: &[u8]) -> String {
    let res = Res::read(res);
    let mut reader = Reader {
        res: &res,
        out: Vec::new(),
        st: State {
            ctm: M::ID,
            round_cap: false,
            dash: None,
            font: None,
            size: 0.0,
        },
        stack: Vec::new(),
        points: Vec::new(),
        start: (0.0, 0.0),
        current: (0.0, 0.0),
        clip: false,
        marks: Vec::new(),
    };
    reader.read(content);
    if reader.out.is_empty() {
        return "none".to_string();
    }
    reader.out.join(" ; ")
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
        assert_eq!(kind, "content", "{name}: a kind this test doesn't read");
        let read = |ext: &str| fs::read(set().join("cases").join(format!("{name}.{ext}")));
        let content = read("content").unwrap();
        let res = read("res").unwrap_or_else(|_| b"<< >>".to_vec());
        let ours = answer(&content, &res);
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
        "{} cases:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}
