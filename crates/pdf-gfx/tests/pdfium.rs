//! `pdf-gfx`'s page geometry, content-stream and Form XObject interpreter, and
//! content re-encoder held to PDFium's own answers (`chromium/8152`), case by
//! case.
//!
//! Each case lives in `tests/pdfium/` (`cases.tsv` and `cases/*.pdf`, generated
//! by `cases.py`), alongside PDFium's recorded output in `chromium-8152.txt`
//! (recorded by `oracle/pdfium.py`, documented in `oracle/README.md`). Cases
//! listed in `differ.tsv` document intentional departures (such as refusing
//! cyclic Form XObject `Do` chains immediately rather than expanding 40 levels)
//! and must continue to differ. Every `gfx` case is also round-tripped through
//! [`pdf_gfx::encode`] and re-interpreted to verify that position-independent
//! re-encoding preserves every placed path, image, shading, clip, mark, and
//! glyph origin.

use std::collections::HashMap;
use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use pdf_gfx::pdf_content_streams::{Concat, FillRule, Join, LineCap, LineJoin, Paint, TextMode};
use pdf_gfx::pdf_fonts::Fonts;
use pdf_gfx::pdf_objects::pdf_tokenizer::Rules;
use pdf_gfx::pdf_objects::{ObjectId, Resolve, Streams, WithStreams};
use pdf_gfx::pdf_semantics::pages::{Inheritable, pages};
use pdf_gfx::pdf_semantics::{Limits as SemanticsLimits, catalog};
use pdf_gfx::{
    Clip, ClipPart, Error, Kind, Limits, Marks, Opts, PageGeom, Point, Rect, Seg, Unwritable,
    encode, interpret,
};
use pdf_structure::read::{Options, Reading};

const LONG: usize = 64;

fn set() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pdfium")
}

fn rows(path: &Path) -> Vec<Vec<String>> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').map(str::to_string).collect())
        .collect()
}

fn recorded() -> HashMap<String, String> {
    let files: Vec<PathBuf> = fs::read_dir(set())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            name.starts_with("chromium-") && name.ends_with(".txt")
        })
        .collect();
    assert_eq!(
        files.len(),
        1,
        "expected one chromium-*.txt file: {files:?}"
    );
    rows(&files[0])
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect()
}

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

fn agree(a: &str, b: &str) -> bool {
    let a: Vec<&str> = a.split_whitespace().collect();
    let b: Vec<&str> = b.split_whitespace().collect();
    a.len() == b.len()
        && a.iter().zip(&b).all(|(x, y)| {
            x == y
                || match (x.parse::<f32>(), y.parse::<f32>()) {
                    (Ok(fx), Ok(fy)) => (fx - fy).abs() <= 1e-4 || (fx.is_nan() && fy.is_nan()),
                    _ => false,
                }
        })
}

fn geom_answer(bytes: &[u8]) -> String {
    let Ok(reading) = Reading::open(bytes, &Options::DEFAULT) else {
        return "no-page".to_string();
    };
    let objects = &reading;
    let Ok(Some(cat)) = catalog(objects, reading.trailer().dict()) else {
        return "no-page".to_string();
    };
    let Ok(table) = pages(
        objects,
        cat,
        Rules::Pdfium,
        &SemanticsLimits::DEFAULT,
        &mut (),
    ) else {
        return "no-page".to_string();
    };
    let Some(page) = table.get(0) else {
        return "no-page".to_string();
    };
    let geom = PageGeom::from_page(page);
    let size = geom.view_size();
    let rot = geom.rotation.quarter_turns();
    let to_view = geom.to_view();
    let mut out = vec![
        "size".to_string(),
        format!("{}", size.width),
        format!("{}", size.height),
        "rot".to_string(),
        rot.to_string(),
        "dev".to_string(),
    ];
    for (px, py) in [(0.0, 0.0), (50.0, 80.0), (120.0, 180.0), (220.0, 330.0)] {
        let v = to_view.apply(Point::new(px, py));
        out.push((v.x.round() as i32).to_string());
        out.push((v.y.round() as i32).to_string());
    }
    out.join(" ")
}

fn fill_code(paint: Paint) -> u8 {
    match paint.fills() {
        None => 0,
        Some(FillRule::EvenOdd) => 1,
        Some(FillRule::NonZero) => 2,
    }
}

fn cap_code(cap: LineCap) -> u8 {
    match cap {
        LineCap::Butt => 0,
        LineCap::Round => 1,
        LineCap::Square => 2,
    }
}

fn join_code(join: LineJoin) -> u8 {
    match join {
        LineJoin::Miter => 0,
        LineJoin::Round => 1,
        LineJoin::Bevel => 2,
    }
}

fn mode_code(mode: TextMode) -> u8 {
    match mode {
        TextMode::Fill => 0,
        TextMode::Stroke => 1,
        TextMode::FillStroke => 2,
        TextMode::Invisible => 3,
        TextMode::FillClip => 4,
        TextMode::StrokeClip => 5,
        TextMode::FillStrokeClip => 6,
        TextMode::Clip => 7,
    }
}

fn push_segments(words: &mut Vec<String>, segs: &[Seg]) {
    for seg in segs {
        match *seg {
            Seg::Move(p) => {
                words.extend(["m".to_string(), format!("{}", p.x), format!("{}", p.y)]);
            }
            Seg::Line(p) => {
                words.extend(["l".to_string(), format!("{}", p.x), format!("{}", p.y)]);
            }
            Seg::Curve(a, b, p) => {
                for pt in [a, b, p] {
                    words.extend(["c".to_string(), format!("{}", pt.x), format!("{}", pt.y)]);
                }
            }
            Seg::Close => words.push("h".to_string()),
        }
    }
}

fn axis_rect(segs: &[Seg]) -> Option<Rect> {
    let mut corners = [Point::ZERO; 5];
    let mut n = 0;
    for &seg in segs {
        match seg {
            Seg::Move(p) if n == 0 => corners[0] = p,
            Seg::Line(p) if n > 0 && n < 5 => corners[n] = p,
            Seg::Close => continue,
            _ => return None,
        }
        n += 1;
    }
    if n == 5 && corners[4] == corners[0] {
        n = 4;
    }
    let [a, b, c, d] = corners[..n] else {
        return None;
    };
    let across = a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y;
    let up = a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x;
    (across || up).then(|| Rect::new(a.x, a.y, c.x, c.y))
}

fn push_top_clips(words: &mut Vec<String>, clip: &Clip, obj_bounds: Option<Rect>) {
    if clip.is_unclipped() {
        return;
    }
    let mut parts: Vec<&ClipPart> = clip.parts().collect();
    parts.reverse();
    if let [ClipPart::Path { segments, .. }] = parts[..]
        && let Some(r) = axis_rect(segments)
        && obj_bounds.is_some_and(|b| r.contains_rect(b))
    {
        // PDFium's `CPDF_StreamContentParser` omits attaching `m_ClipPath` to a
        // page object whose bounding box lies wholly inside a single
        // axis-aligned rectangular clip with no text clips.
        return;
    }
    let path_parts: Vec<&[Seg]> = parts
        .iter()
        .filter_map(|p| match p {
            ClipPart::Path { segments, .. } => Some(segments.as_ref()),
            _ => None,
        })
        .collect();
    words.push("clips".to_string());
    words.push(path_parts.len().to_string());
    for segs in path_parts {
        words.push("[".to_string());
        push_segments(words, segs);
        words.push("]".to_string());
    }
}

fn push_marks(words: &mut Vec<String>, marks: &Marks) {
    if marks.is_empty() {
        return;
    }
    let mut stack: Vec<_> = marks.iter().collect();
    stack.reverse();
    words.push("marks".to_string());
    for m in stack {
        let mcid = m.mcid.map_or_else(|| "-".to_string(), |v| v.to_string());
        words.push(format!("{}:{mcid}", esc(&m.tag)));
    }
}

fn render_gfx<'o, O>(
    objects: O,
    content: &Concat<'_>,
    resources: Option<&'o pdf_gfx::pdf_objects::Object>,
    fonts: &Fonts,
) -> String
where
    O: Resolve<'o> + Streams + Copy,
{
    let mut items = Vec::new();
    let mut chars = Vec::new();
    let mut last_text_op: Option<(Option<Vec<u32>>, usize)> = None;

    interpret(
        objects,
        content,
        resources,
        &Limits::DEFAULT,
        fonts,
        Opts::default(),
        |ev| {
            let top = ev.at.stream.is_none();
            match ev.kind {
                Kind::Path(path) => {
                    if !path.paint.strokes() && path.paint.fills().is_none() {
                        return ControlFlow::Continue(());
                    }
                    let stroke = u8::from(path.paint.strokes());
                    let mut words = vec![
                        "path".to_string(),
                        "fill".to_string(),
                        fill_code(path.paint).to_string(),
                        "stroke".to_string(),
                        stroke.to_string(),
                    ];
                    if path.paint.strokes() {
                        words.extend([
                            "w".to_string(),
                            format!("{}", ev.state.line_width),
                            "cap".to_string(),
                            cap_code(ev.state.line_cap).to_string(),
                            "join".to_string(),
                            join_code(ev.state.line_join).to_string(),
                        ]);
                        if !ev.state.dash.array.is_empty() {
                            words.push("dash".to_string());
                            for &d in ev.state.dash.array.iter() {
                                words.push(format!("{d}"));
                            }
                            words.push("phase".to_string());
                            words.push(format!("{}", ev.state.dash.phase));
                        }
                    }
                    push_segments(&mut words, path.segments);
                    if top {
                        push_top_clips(&mut words, &ev.state.clip, Some(path.bounds));
                    }
                    push_marks(&mut words, ev.marks);
                    items.push(words.join(" "));
                }
                Kind::Image(image) => {
                    let mut words = vec![
                        "image".to_string(),
                        "px".to_string(),
                        image.width.unwrap_or(0).to_string(),
                        image.height.unwrap_or(0).to_string(),
                        "quad".to_string(),
                    ];
                    for p in image.quad.0 {
                        words.push(format!("{}", p.x));
                        words.push(format!("{}", p.y));
                    }
                    if top {
                        push_top_clips(&mut words, &ev.state.clip, Some(image.quad.bounds()));
                    }
                    push_marks(&mut words, ev.marks);
                    items.push(words.join(" "));
                }
                Kind::Text(show) => {
                    if show.font.is_none() {
                        return ControlFlow::Continue(());
                    }
                    let op_key = (
                        ev.at.stream.map(|u| u.path().to_vec()),
                        ev.at.op_range().start,
                    );
                    if last_text_op.as_ref() != Some(&op_key) {
                        last_text_op = Some(op_key);
                        let mut words = vec![
                            "text".to_string(),
                            "size".to_string(),
                            format!("{}", show.size),
                            "mode".to_string(),
                            mode_code(show.mode).to_string(),
                        ];
                        if top {
                            push_top_clips(&mut words, &ev.state.clip, None);
                        }
                        push_marks(&mut words, ev.marks);
                        items.push(words.join(" "));
                    }
                    for g in show.glyphs() {
                        if g.size_zero_dropped(show.size) {
                            continue;
                        }
                        let ch = g
                            .text()
                            .and_then(|s| s.chars().next())
                            .map_or(g.code, u32::from);
                        chars.push(format!("char {ch:04x} {} {}", g.origin.x, g.origin.y));
                    }
                }
                Kind::Shading(sh) => {
                    let mut words = vec!["shading".to_string()];
                    if top {
                        push_top_clips(&mut words, &ev.state.clip, sh.bounds);
                    }
                    push_marks(&mut words, ev.marks);
                    items.push(words.join(" "));
                }
                _ => {}
            }
            ControlFlow::Continue(())
        },
    )
    .expect("case interprets cleanly");

    items.extend(chars);
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(" ; ")
    }
}

trait GlyphExt {
    fn size_zero_dropped(&self, size: f32) -> bool;
}

impl GlyphExt for pdf_gfx::Glyph<'_> {
    fn size_zero_dropped(&self, size: f32) -> bool {
        size == 0.0
    }
}

fn gfx_answer_and_reencoded(bytes: &[u8]) -> (String, Result<String, Error>) {
    let Ok(reading) = Reading::open(bytes, &Options::DEFAULT) else {
        return ("no-page".to_string(), Ok("no-page".to_string()));
    };
    let objects = &reading;
    let Ok(Some(cat)) = catalog(objects, reading.trailer().dict()) else {
        return ("no-page".to_string(), Ok("no-page".to_string()));
    };
    let Ok(table) = pages(
        objects,
        cat,
        Rules::Pdfium,
        &SemanticsLimits::DEFAULT,
        &mut (),
    ) else {
        return ("no-page".to_string(), Ok("no-page".to_string()));
    };
    let Some(page) = table.get(0) else {
        return ("no-page".to_string(), Ok("no-page".to_string()));
    };
    let Some(page_dict) = page.dict else {
        return ("no-page".to_string(), Ok("no-page".to_string()));
    };
    let resources = page.attribute(Inheritable::Resources).map(|r| r.written);
    let mut join = Join::new(&Limits::DEFAULT.content);
    if let Some(contents) = page_dict.get(b"Contents") {
        join.push_contents(objects, contents)
            .expect("page contents decode");
    }
    let content = join.finish();
    let fonts = Fonts::default();
    let direct = render_gfx(objects, &content, resources, &fonts);

    let reencoded = encode(objects, &content, resources, &Limits::DEFAULT, &fonts).map(|encoded| {
        let form_overrides: HashMap<ObjectId, Vec<u8>> = encoded.forms.into_iter().collect();
        let patched = WithStreams::new(objects, |id, limit| {
            if let Some(overridden) = form_overrides.get(&id) {
                Ok(overridden.clone())
            } else {
                objects.decoded(id, limit).map(|b| b.into_owned())
            }
        });
        let enc_content = Concat::new(&[encoded.content.as_slice()]);
        render_gfx(patched, &enc_content, resources, &fonts)
    });

    (direct, reencoded)
}

#[cfg(feature = "text-layout")]
fn text_layout_answer(bytes: &[u8], queries: Option<&str>) -> String {
    use pdf_gfx::{SearchOpts, TextPage, TextPageOpts};

    let Ok(reading) = Reading::open(bytes, &Options::DEFAULT) else {
        return "no-page".to_string();
    };
    let objects = &reading;
    let Ok(Some(cat)) = catalog(objects, reading.trailer().dict()) else {
        return "no-page".to_string();
    };
    let Ok(table) = pages(
        objects,
        cat,
        Rules::Pdfium,
        &SemanticsLimits::DEFAULT,
        &mut (),
    ) else {
        return "no-page".to_string();
    };
    let Some(page) = table.get(0) else {
        return "no-page".to_string();
    };
    let Some(page_dict) = page.dict else {
        return "no-page".to_string();
    };
    let resources = page.attribute(Inheritable::Resources).map(|r| r.written);
    let mut join = Join::new(&Limits::DEFAULT.content);
    if let Some(contents) = page_dict.get(b"Contents") {
        join.push_contents(objects, contents)
            .expect("page contents decode");
    }
    let content = join.finish();
    let fonts = Fonts::default();
    let tp = TextPage::extract(
        objects,
        &content,
        resources,
        &Limits::DEFAULT,
        &fonts,
        TextPageOpts::default(),
    )
    .expect("text page extracts");

    // Also verify re-encoding preserves TextPage text and line structure.
    let encoded = encode(objects, &content, resources, &Limits::DEFAULT, &fonts)
        .expect("gfx-text case encodes");
    let form_overrides: HashMap<ObjectId, Vec<u8>> = encoded.forms.into_iter().collect();
    let patched = WithStreams::new(objects, |id, limit| {
        if let Some(overridden) = form_overrides.get(&id) {
            Ok(overridden.clone())
        } else {
            objects.decoded(id, limit).map(|b| b.into_owned())
        }
    });
    let enc_content = Concat::new(&[encoded.content.as_slice()]);
    let tp_reencoded = TextPage::extract(
        patched,
        &enc_content,
        resources,
        &Limits::DEFAULT,
        &fonts,
        TextPageOpts::default(),
    )
    .expect("reencoded text page extracts");
    assert_eq!(
        tp.text(),
        tp_reencoded.text(),
        "re-encoded TextPage text diverged"
    );

    let pcs = tp.chars();
    let mut tokens = Vec::new();
    let mut char_to_tok = vec![0usize; pcs.len() + 1];
    let mut i = 0usize;
    while i < pcs.len() {
        let pc = &pcs[i];
        let tid = tokens.len();
        if pc.soft_hyphen {
            char_to_tok[i] = tid;
            tokens.push("002d:0:1".to_string());
            if i + 1 < pcs.len() && pcs[i + 1].generated && pcs[i + 1].ch == '\n' {
                char_to_tok[i + 1] = tid;
                i += 2;
                continue;
            }
            i += 1;
        } else if pc.generated && pc.ch == '\n' {
            char_to_tok[i] = tid;
            tokens.push("000a:1:0".to_string());
            i += 1;
        } else {
            char_to_tok[i] = tid;
            tokens.push(format!(
                "{:04x}:{}:0",
                u32::from(pc.ch),
                u8::from(pc.generated)
            ));
            i += 1;
        }
    }
    char_to_tok[pcs.len()] = tokens.len();

    let rect_count = tp.selection_rects(0..tp.len()).len();
    let mut parts = vec![
        format!(
            "chars {}",
            if tokens.is_empty() {
                "none".to_string()
            } else {
                tokens.join(" ")
            }
        ),
        format!("rects {rect_count}"),
    ];

    if let Some(q_text) = queries {
        for line in q_text.lines() {
            if line.is_empty() {
                continue;
            }
            let (q_str, fl_str) = line.split_once('\t').expect("query tab flags");
            let fl: u32 = fl_str.parse().expect("flags int");
            let opts = SearchOpts {
                match_case: (fl & 1) != 0,
                whole_word: (fl & 2) != 0,
                consecutive: (fl & 4) != 0,
                ignore_soft_hyphens: false,
            };
            let hits: Vec<String> = tp
                .search(q_str, opts)
                .into_iter()
                .map(|m| {
                    let s = char_to_tok[m.chars.start];
                    let e = char_to_tok[m.chars.end - 1] + 1;
                    format!("{s}..{e}")
                })
                .collect();
            parts.push(format!(
                "find {} {fl} {}",
                esc(q_str.as_bytes()),
                if hits.is_empty() {
                    "none".to_string()
                } else {
                    hits.join(" ")
                }
            ));
        }
    }

    parts.join(" ; ")
}

#[test]
fn every_case_reads_and_reencodes_as_pdfium_reads_it() {
    let recorded = recorded();
    let differ: HashMap<String, String> = rows(&set().join("differ.tsv"))
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    let mut wrong = Vec::new();
    let cases = rows(&set().join("cases.tsv"));
    assert!(!cases.is_empty());

    for case in &cases {
        let (name, kind) = (&case[0], &case[1]);
        if kind == "gfx-text" && !cfg!(feature = "text-layout") {
            continue;
        }
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let ours = match kind.as_str() {
            "gfx-geom" => geom_answer(&bytes),
            "gfx" => {
                let (direct, reencoded) = gfx_answer_and_reencoded(&bytes);
                match reencoded {
                    Ok(reencoded) => {
                        if !agree(&direct, &reencoded) {
                            wrong.push(format!(
                                "{name} encode round-trip diverged:\n  direct    {direct}\n  reencoded {reencoded}"
                            ));
                        }
                    }
                    Err(Error::Unwritable {
                        why: Unwritable::InlineStream,
                        ..
                    }) if name == "images-inline-broken-filter" => {}
                    Err(err) => {
                        wrong.push(format!("{name} failed to re-encode: {err}"));
                    }
                }
                direct
            }
            #[cfg(feature = "text-layout")]
            "gfx-text" => {
                let q_path = set().join("cases").join(format!("{name}.queries"));
                let q_str = fs::read_to_string(q_path).ok();
                text_layout_answer(&bytes, q_str.as_deref())
            }
            other => panic!("{name}: unexpected case kind {other}"),
        };
        let pdfium = recorded
            .get(name)
            .unwrap_or_else(|| panic!("{name}: no recorded answer"));
        match (agree(pdfium, &ours), differ.contains_key(name)) {
            (true, false) | (false, true) => {}
            (false, false) => wrong.push(format!("{name}:\n  PDFium {pdfium}\n  here   {ours}")),
            (true, true) => wrong.push(format!(
                "{name}: read as PDFium reads it, but listed in differ.tsv"
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
