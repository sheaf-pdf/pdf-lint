//! Holds `pdf-annots`'s `Rules::Pdfium` annotation, geometry, destination, and
//! action reading to PDFium's answers recorded at `chromium/8152` in
//! `tests/pdfium/chromium-8152.txt`.

use std::collections::HashMap;
use std::fs;
use std::ops::ControlFlow;
use std::path::PathBuf;

use pdf_annots::pdf_content_streams::{Concat, FillRule, Paint};
use pdf_annots::pdf_fonts::Fonts;
use pdf_annots::pdf_gfx::{self, Kind, Opts, Seg};
use pdf_annots::pdf_objects::pdf_tokenizer::Rules;
use pdf_annots::pdf_objects::{Dict, Object, Streams, dict};
use pdf_annots::pdf_semantics::pages::{self, Inheritable};
use pdf_annots::{
    Action, AnnotSubtype, Dest, DestFit, DestTable, FlattenOpts, FlattenTarget, Limits, PolyAnnot,
    TextMarkupAnnot, TypedAnnot, page_annots, plan_flatten,
};
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

fn num(x: f32) -> String {
    if x == 0.0 {
        return "0".to_string();
    }
    format!("{x}")
}

fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

fn esc(data: &[u8]) -> String {
    const LONG: usize = 64;
    if data.is_empty() {
        return "%".to_string();
    }
    if data.len() > LONG {
        return format!("#{}:{:016x}", data.len(), fnv1a(data));
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

fn agree_words(a: &str, b: &str) -> bool {
    let aw: Vec<&str> = a.split_whitespace().collect();
    let bw: Vec<&str> = b.split_whitespace().collect();
    if aw.len() != bw.len() {
        return false;
    }
    for (x, y) in aw.iter().zip(&bw) {
        if x == y {
            continue;
        }
        let (Ok(fx), Ok(fy)) = (x.parse::<f32>(), y.parse::<f32>()) else {
            return false;
        };
        if !(fx == fy || (fx.is_nan() && fy.is_nan())) {
            return false;
        }
    }
    true
}

fn annots_answer(bytes: &[u8]) -> String {
    let mut opts = read::Options::DEFAULT;
    opts.recovery = Recovery::Pdfium;
    let Ok(reading) = Reading::open(bytes, &opts) else {
        return "no-page".to_string();
    };
    let trailer = reading.trailer().dict();
    let Ok(Some(catalog)) = pdf_annots::pdf_semantics::catalog(&reading, trailer) else {
        return "no-page".to_string();
    };
    let Ok(table) = pages::pages(
        &reading,
        catalog,
        Rules::Pdfium,
        &pdf_annots::pdf_semantics::Limits::DEFAULT,
        &mut (),
    ) else {
        return "no-page".to_string();
    };
    let Some(page0) = table.get(0) else {
        return "no-page".to_string();
    };
    let Some(page_dict) = page0.dict else {
        return "no-page".to_string();
    };

    let dests = DestTable::read(
        &reading,
        Some(catalog),
        &table,
        Rules::Pdfium,
        &pdf_annots::pdf_semantics::Limits::DEFAULT,
        &mut (),
    )
    .unwrap();

    let annots = page_annots(
        &reading,
        page_dict,
        page0.id,
        Rules::Pdfium,
        &Limits::DEFAULT,
    )
    .unwrap();
    if annots.is_empty() {
        return "none".to_string();
    }

    let mut out = Vec::with_capacity(annots.len());
    for a in &annots {
        let idx = a.index().unwrap_or(0);
        let sub = a.subtype().pdfium_code();
        let flags = a.flags().bits();
        let [rl, rb, rr, rt] = a.raw_rect();
        let mut words = vec![
            "idx".to_string(),
            idx.to_string(),
            "sub".to_string(),
            sub.to_string(),
            "flags".to_string(),
            flags.to_string(),
            "rect".to_string(),
            num(rl),
            num(rb),
            num(rr),
            num(rt),
        ];

        if let Some([hr, vr, bw]) = a.legacy_border() {
            words.extend(["border".into(), num(hr), num(vr), num(bw)]);
        }
        if let Some(c) = a.contents().filter(|s| !s.is_empty()) {
            words.extend(["contents".into(), esc(c.as_bytes())]);
        }
        if let Some(t) = a
            .markup()
            .and_then(|m| m.author.as_deref())
            .filter(|s| !s.is_empty())
        {
            words.extend(["author".into(), esc(t.as_bytes())]);
        }
        if let Some(nm) = a.name().filter(|s| !s.is_empty()) {
            words.extend(["nm".into(), esc(nm.as_bytes())]);
        }

        match a.typed() {
            TypedAnnot::TextMarkup(TextMarkupAnnot { quad_points, .. })
                if !quad_points.is_empty() =>
            {
                words.extend(["quads".into(), quad_points.len().to_string()]);
                for qp in quad_points {
                    words.extend(qp.0.into_iter().map(num));
                }
            }
            TypedAnnot::Link(lnk) => {
                if !lnk.quad_points.is_empty() {
                    words.extend(["quads".into(), lnk.quad_points.len().to_string()]);
                    for qp in &lnk.quad_points {
                        words.extend(qp.0.into_iter().map(num));
                    }
                }
                let pdfium_link_dest = lnk.dest.as_ref().or_else(|| match lnk.actions.first() {
                    Some(Action::GoTo { dest: Some(d) } | Action::GoToR { dest: Some(d), .. }) => {
                        Some(d)
                    }
                    _ => None,
                });
                if let Some(d) = pdfium_link_dest
                    && let Some(res) = format_dest(&dests, d)
                {
                    words.extend(res);
                }
                if let Some(first_act) = lnk.actions.first() {
                    words.extend(format_action(first_act));
                }
            }
            TypedAnnot::Line(ln) => {
                if let Some((sp, ep)) = ln.endpoints {
                    words.extend(["line".into(), num(sp.x), num(sp.y), num(ep.x), num(ep.y)]);
                }
            }
            TypedAnnot::Poly(PolyAnnot { vertices, .. }) if !vertices.is_empty() => {
                words.extend(["verts".into(), vertices.len().to_string()]);
                for pt in vertices {
                    words.extend([num(pt.x), num(pt.y)]);
                }
            }
            TypedAnnot::Ink(ink) if !ink.strokes.is_empty() => {
                words.extend(["ink".into(), ink.strokes.len().to_string()]);
                for s in &ink.strokes {
                    words.extend(["stroke".into(), s.len().to_string()]);
                    for pt in s {
                        words.extend([num(pt.x), num(pt.y)]);
                    }
                }
            }
            _ => {}
        }

        out.push(words.join(" "));
    }
    out.join(" ; ")
}

fn format_dest(dests: &DestTable, d: &Dest) -> Option<Vec<String>> {
    let resolved = dests.resolve(d, Rules::Pdfium, &mut ());
    match (d, resolved) {
        (_, Some(res)) => {
            let (vw, params) = format_fit(res.fit);
            let mut w = vec![
                "dest".to_string(),
                res.page_index.to_string(),
                vw.to_string(),
            ];
            w.extend(params);
            Some(w)
        }
        // Unresolved named dest -> `FPDFLink_GetDest` returns NULL in PDFium!
        (Dest::Named { .. }, None) => None,
        (Dest::Explicit(exp), None) => {
            let (vw, params) = format_fit(exp.fit);
            let mut w = vec!["dest".to_string(), "-1".to_string(), vw.to_string()];
            w.extend(params);
            Some(w)
        }
        _ => None,
    }
}

fn format_fit(fit: DestFit) -> (u32, Vec<String>) {
    let opt = |v: Option<f32>| v.map_or_else(|| "null".to_string(), num);
    match fit {
        DestFit::XYZ { left, top, zoom } => (1, vec![opt(left), opt(top), opt(zoom)]),
        DestFit::Fit => (2, Vec::new()),
        DestFit::FitH { top } => (3, vec![top.map_or_else(|| "0".into(), num)]),
        DestFit::FitV { left } => (4, vec![left.map_or_else(|| "0".into(), num)]),
        DestFit::FitR {
            left,
            bottom,
            right,
            top,
        } => (5, vec![num(left), num(bottom), num(right), num(top)]),
        DestFit::FitB => (6, Vec::new()),
        DestFit::FitBH { top } => (7, vec![top.map_or_else(|| "0".into(), num)]),
        DestFit::FitBV { left } => (8, vec![left.map_or_else(|| "0".into(), num)]),
        _ => (0, Vec::new()),
    }
}

fn format_action(act: &Action) -> Vec<String> {
    match act {
        Action::GoTo { .. } => vec!["act".into(), "1".into()],
        Action::GoToR { file, .. } => {
            let mut w = vec!["act".into(), "2".into()];
            if let Some(f) = file.as_deref().filter(|s| !s.is_empty()) {
                w.extend(["file".into(), esc(f.as_bytes())]);
            }
            w
        }
        Action::Uri { uri, .. } => {
            let mut w = vec!["act".into(), "3".into()];
            if !uri.is_empty() {
                w.extend(["uri".into(), esc(uri)]);
            }
            w
        }
        Action::Launch { file, .. } => {
            let mut w = vec!["act".into(), "4".into()];
            if let Some(f) = file.as_deref().filter(|s| !s.is_empty()) {
                w.extend(["file".into(), esc(f.as_bytes())]);
            }
            w
        }
        Action::GoToE { .. } => vec!["act".into(), "5".into()],
        _ => vec!["act".into(), "0".into()],
    }
}

fn fill_code(paint: Paint) -> u8 {
    match paint.fills() {
        None => 0,
        Some(FillRule::EvenOdd) => 1,
        Some(FillRule::NonZero) => 2,
    }
}

fn flatten_answer(bytes: &[u8], target: FlattenTarget) -> String {
    let mut opts = read::Options::DEFAULT;
    opts.recovery = Recovery::Pdfium;
    let Ok(reading) = Reading::open(bytes, &opts) else {
        return "no-page".to_string();
    };
    let trailer = reading.trailer().dict();
    let Ok(Some(catalog)) = pdf_annots::pdf_semantics::catalog(&reading, trailer) else {
        return "no-page".to_string();
    };
    let Ok(table) = pages::pages(
        &reading,
        catalog,
        Rules::Pdfium,
        &pdf_annots::pdf_semantics::Limits::DEFAULT,
        &mut (),
    ) else {
        return "no-page".to_string();
    };
    let Some(page0) = table.get(0) else {
        return "no-page".to_string();
    };
    let Some(page_dict) = page0.dict else {
        return "no-page".to_string();
    };
    let page_res = page0.attribute(Inheritable::Resources).map(|a| a.written);

    let plan = plan_flatten(
        &reading,
        page_dict,
        page0.id,
        page_res,
        FlattenOpts::for_target(target),
        &Limits::DEFAULT,
    )
    .unwrap();

    // `FLAT_SUCCESS = 1`, `FLAT_NOTHINGTODO = 2`
    let status = if plan.changed() { 1 } else { 2 };
    let rem = plan.kept_annots.len();

    let mut existing = Vec::new();
    if let Some(c_obj) = page_dict.get(b"Contents")
        && let Some(sid) = c_obj.as_reference()
        && let Ok(decoded) = reading.decoded(sid, 1 << 20)
    {
        existing = decoded.to_vec();
    }
    let combined = plan.combine_with_page(&existing);

    let mut xo = Dict::new();
    for (name, id) in &plan.xobject_bindings {
        xo.set(name.clone(), *id);
    }
    let merged_res = Object::from(dict! { "XObject" => xo });
    let slices = [&combined[..]];
    let concat = Concat::new(&slices);

    let mut items = vec![format!("status {status} rem {rem}")];
    pdf_gfx::interpret(
        &reading,
        &concat,
        Some(&merged_res),
        &pdf_gfx::Limits::DEFAULT,
        &Fonts::default(),
        Opts::default(),
        |ev| {
            if let Kind::Path(path) = ev.kind {
                let mut w = vec![
                    "path".to_string(),
                    "fill".to_string(),
                    fill_code(path.paint).to_string(),
                    "stroke".to_string(),
                    u8::from(path.paint.strokes()).to_string(),
                ];
                for seg in path.segments {
                    match *seg {
                        Seg::Move(p) => w.extend(["m".into(), num(p.x), num(p.y)]),
                        Seg::Line(p) => w.extend(["l".into(), num(p.x), num(p.y)]),
                        Seg::Curve(a, b, p) => {
                            for pt in [a, b, p] {
                                w.extend(["c".into(), num(pt.x), num(pt.y)]);
                            }
                        }
                        Seg::Close => w.push("h".into()),
                    }
                }
                items.push(w.join(" "));
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();

    items.join(" ; ")
}

#[test]
fn pdfium_recorded_cases_agree() {
    // Silence unused import warning when matching subtype helpers.
    let _ = AnnotSubtype::Link;

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
        let bytes = fs::read(set().join("cases").join(format!("{name}.pdf"))).unwrap();
        let ours = match kind.as_str() {
            "annots" => annots_answer(&bytes),
            "annots-flatten-display" => flatten_answer(&bytes, FlattenTarget::Display),
            "annots-flatten-print" => flatten_answer(&bytes, FlattenTarget::Print),
            other => panic!("{name}: unexpected case kind {other}"),
        };
        let pdfium = recorded
            .get(name)
            .unwrap_or_else(|| panic!("{name}: no recorded answer"));
        let matches = agree_words(&ours, pdfium);
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
