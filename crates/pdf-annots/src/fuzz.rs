//! In-process fuzzing entrypoints exercised by unit tests and `fuzz/fuzz_targets/annots.rs`.
//!
//! Verifies on arbitrary byte inputs:
//! 1. Zero panics across `page_annots`, `page_annots_with`, `page_tabs`,
//!    `DestTable::read`, `resolve_threads`, and `stream_placement_matrix`.
//! 2. Sink invariance: `&mut ()` and `Vec` fault sinks produce identical
//!    `Result<Vec<Annot>, Error>` under both `Rules::Iso` and `Rules::Pdfium`.
//! 3. `Debug` formatting never panics on any parsed `Annot`, `DestTable`,
//!    `AnnotThread`, or `Fault`.
//! 4. A `/Redact` mark synthesized from what was read (its quadrilaterals and
//!    overlay text) never panics, and its mark is `synth_annot`'s.
//! 5. Flattening with redaction marks drawn never keeps back what was drawn
//!    or removed without them: it draws and removes at least as many
//!    annotations, and keeps no more.
//! 6. `Border::read` of an annotation's dictionary gives the border
//!    `Annot::from_dict` read, with a sink and without, and tells what
//!    `Annot::from_dict_with` tells while it reads the border.
//! 7. The `AnnotMut` setters of `/MK`, `/AP /D`, and state dictionaries,
//!    each on a copy of an annotation's dictionary, write what they're given
//!    and keep what they say they keep, as `Appearances::read` and
//!    `Annot::from_dict` read the copy back: a slot holds the states given,
//!    in their order, or the one state set beside the others; the other slots
//!    and `/AS` are as they were; an `/AP` that isn't a direct dictionary is
//!    left as it is for `/R` and `/D`; a widget's `/MK` reads back as set,
//!    and a direct `/MK`'s other entries are kept.
//! 8. Under `text-layout`, a `/FreeText`'s text and a `/Redact`'s overlay
//!    text drawn in a font given (the `*_with_font` synthesizers and
//!    builders) never panic; in Helvetica they give what the defaults give
//!    wherever the font can draw the text, a builder builds exactly where its
//!    synthesizer succeeds, keeping the font for the form's default resources
//!    (`BuiltAnnot::da_font`) where the defaults keep none, and in a font that
//!    can't draw the text the error leaves nothing built.

use std::collections::BTreeMap;

use pdf_objects::parse::Parser;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Str, Stream, Value, dict};
use pdf_semantics::dest::DestTable;
use pdf_semantics::pages;

use crate::annot::{Annot, page_annots, page_annots_with, page_tabs};
use crate::ap::{AppearanceEntry, AppearanceMode, Appearances, stream_placement_matrix};
use crate::build::{AnnotMut, BuiltAnnot};
use crate::flatten::{FlattenOpts, FlattenTarget, plan_flatten};
use crate::limits::Limits;
use crate::replies::resolve_threads;
use crate::style::{AnnotColor, AppearanceChars, Border, DefaultAppearance};
use crate::subtypes::TypedAnnot;
use crate::synth::{RedactMark, synth_annot, synth_redact};
#[cfg(feature = "text-layout")]
use crate::{
    style::PaintStyle,
    synth::{synth_free_text, synth_free_text_with_font, synth_redact_with_font},
};

/// Built-in seed inputs exercising links, destinations, action chains, text
/// markup, geometric annotations, ink, redaction, replies/review states,
/// appearance dictionaries, and widgets' appearance characteristics and
/// state dictionaries, direct and by reference.
pub const SEEDS: &[&[u8]] = &[
    b"",
    b"<< /Annots [1 0 R 2 0 R] /Tabs /S >>\xff<< /Type /Catalog /Pages 3 0 R /Names << /Dests << /Names [(sec1) [4 0 R /XYZ 0 700 null]] >> >> >>\xff<< /Subtype /Link /Rect [10 20 100 40] /Dest (sec1) /Border [0 0 1] /F 4 >>\xff<< /Subtype /Highlight /Rect [10 50 120 70] /QuadPoints [10 70 120 70 10 50 120 50] /C [1 1 0] /T (Alice) /Contents (Check this) >>\xff<< /Type /Pages /Kids [4 0 R] /Count 1 >>\xff<< /Type /Page /Parent 3 0 R /MediaBox [0 0 612 792] >>",
    b"<< /Annots [1 0 R 2 0 R 3 0 R] >>\xff<< /Type /Catalog /Pages 4 0 R >>\xff<< /Subtype /Text /Rect [20 20 40 40] /Name /Comment /T (Bob) /M (D:20261115120000Z) >>\xff<< /Subtype /Text /Rect [20 20 40 40] /IRT 1 0 R /StateModel (Review) /State (Accepted) >>\xff<< /Subtype /FreeText /Rect [50 50 200 100] /DA (/Helv 12 Tf 0 0 1 rg) /Q 1 /CL [50 50 30 30] /LE /OpenArrow >>\xff<< /Type /Pages /Kids [5 0 R] /Count 1 >>\xff<< /Type /Page /Parent 4 0 R /MediaBox [0 0 200 200] >>",
    b"<< /Annots [1 0 R 2 0 R] >>\xff<< /Type /Catalog >>\xff<< /Subtype /Ink /Rect [0 0 100 100] /InkList [[10 10 20 30 40 25]] /BS << /W 2 /S /D /D [4 2] >> >>\xff<< /Subtype /Redact /Rect [10 10 80 30] /IC [0 0 0] /OverlayText (REDACTED) /DA (/Helv 10 Tf 1 g) /AP << /N 3 0 R >> >>\xff<< /Subtype /Form /BBox [0 0 70 20] >>\nstream\n0 g 0 0 70 20 re f",
    b"<< /Annots [1 0 R] >>\xff<< /Type /Catalog >>\xff<< /Subtype /Redact /Rect [100 500 300 540] /C [0 0 1] /IC [0.5] /QuadPoints [100 540 200 540 100 520 200 520 210 502 290 506 285 526 205 521] /OverlayText (b\\(5\\)) /Repeat true /DA (/Helv 6 Tf 1 g) /Q 1 >>",
    b"<< /Annots [1 0 R 2 0 R 3 0 R] >>\xff<< /Type /Catalog >>\xff<< /Subtype /Redact /Rect [10 10 80 30] /Popup 2 0 R /OverlayText (\\251 x) /Repeat true /RO 4 0 R /AP << /N 4 0 R >> >>\xff<< /Subtype /Popup /Rect [90 10 190 60] /Parent 1 0 R /Open true >>\xff<< /Subtype /FreeText /Rect [50 50 200 100] /DA (/Helv 0 Tf 0 g) /Contents (Caf\\351 \\(1\\)) /Q 2 >>\xff<< /Subtype /Form /BBox [0 0 70 20] >>\nstream\n0 g 0 0 70 20 re f",
    b"<< /Annots [1 0 R 2 0 R 3 0 R] >>\xff<< /Type /Catalog >>\xff<< /Subtype /Widget /Rect [10 10 30 30] /FT /Btn /MK << /R 90 /BC [1 0 0] /BG [0.9] /CA (4) /AC (8) /I 4 0 R /IF << /SW /A >> >> /AS /Yes /AP << /N << /Yes 4 0 R /Off 5 0 R >> /D << /Yes 4 0 R >> >> /BS << /W 2 /S /B >> >>\xff<< /Subtype /Widget /Rect [40 10 60 30] /MK 6 0 R /AP 7 0 R /AS /Off /Border [0 0 1 [2 1]] >>\xff<< /Subtype /Widget /Rect [70 10 90 30] /AP << /N 4 0 R /D 5 0 R >> /BE << /S /C /I 1 >> >>\xff<< /Subtype /Form /BBox [0 0 20 20] >>\nstream\n0 g 0 0 20 20 re f\xff<< /Subtype /Form /BBox [0 0 20 20] >>\nstream\n1 g 0 0 20 20 re f\xff<< /CA (l) /I 4 0 R >>\xff<< /N << /On 4 0 R /Off 5 0 R >> /R 4 0 R >>",
];

/// Runs annotation, destination, action, and reply-thread checks over an
/// arbitrary byte slice split on `0xFF`.
pub fn check(data: &[u8]) {
    let mut parts = data.split(|&b| b == 0xFF);
    let page_bytes = parts.next().unwrap_or_default();
    let cat_bytes = parts.next().unwrap_or_default();

    let mut parse_limits = pdf_objects::Limits::DEFAULT;
    parse_limits.nesting = 16;
    parse_limits.values = 512;

    let page_dict = Parser::at(page_bytes, 0)
        .rules(Rules::Pdfium)
        .limits(parse_limits)
        .value(&mut ())
        .ok()
        .and_then(|o| match o.into_value() {
            Value::Dict(d) => Some(d),
            _ => None,
        })
        .unwrap_or_default();

    let cat_dict = Parser::at(cat_bytes, 0)
        .rules(Rules::Pdfium)
        .limits(parse_limits)
        .value(&mut ())
        .ok()
        .and_then(|o| match o.into_value() {
            Value::Dict(d) => Some(d),
            _ => None,
        });

    let mut map: BTreeMap<ObjectId, Object> = BTreeMap::new();
    for (idx, chunk) in parts.take(24).enumerate() {
        let id = ObjectId::new((idx + 1) as u32, 0);
        if let Some((dict_part, raw_stream)) = split_once_bytes(chunk, b"\nstream\n") {
            if let Ok(obj) = Parser::at(dict_part, 0)
                .rules(Rules::Pdfium)
                .limits(parse_limits)
                .value(&mut ())
                && let Value::Dict(d) = obj.into_value()
            {
                map.insert(id, Object::from(Stream::new(d, raw_stream.to_vec())));
            }
        } else if let Ok(obj) = Parser::at(chunk, 0)
            .rules(Rules::Pdfium)
            .limits(parse_limits)
            .value(&mut ())
        {
            map.insert(id, obj);
        }
    }

    let mut sem_limits = pdf_semantics::Limits::DEFAULT;
    sem_limits.tree_depth = 16;
    sem_limits.tree_nodes = 64;
    sem_limits.pages = 16;
    sem_limits.action_chain = 32;

    let limits = Limits {
        hops: 16,
        annots_per_page: 64,
        reply_depth: 32,
        quads: 256,
        points: 1024,
        semantics: sem_limits,
        ..Limits::DEFAULT
    };

    let _ = DefaultAppearance::parse(page_bytes, &limits.content).map(|da| da.to_bytes());

    for rules in [Rules::Iso, Rules::Pdfium] {
        let plain = page_annots(&map, &page_dict, None, rules, &limits);
        let mut faults = Vec::new();
        let with_sink = page_annots_with(&map, &page_dict, None, rules, &limits, &mut faults);
        assert_eq!(plain, with_sink);
        for (_, fault) in &faults {
            let _ = format!("{fault:?}");
            let _ = format!("{fault}");
        }
        let _ = page_tabs(&map, &page_dict, None, rules, &limits, &mut ());

        if let Ok(annots) = plain {
            for a in &annots {
                let _ = format!("{a:?}");
                if let Some(ap) = a.appearances()
                    && let Some(sid) = ap.stream(AppearanceMode::Normal, false, false)
                {
                    let _ = stream_placement_matrix(&map, sid, a.rect(), &limits);
                }
                let syn = synth_annot(a);
                if let Some(syn) = &syn {
                    let _ = format!("{syn:?}");
                }
                // A redaction mark as it was read, its quadrilaterals and
                // its overlay text among what's drawn: the mark's own
                // appearance is the one `synth_annot` gives.
                if let TypedAnnot::Redact(rd) = a.typed() {
                    let mark = RedactMark {
                        quads: &rd.quad_points,
                        stroke_color: a.color(),
                        interior_color: rd.interior_color,
                        overlay_text: rd.overlay_text.as_deref(),
                        repeat: rd.repeat,
                        da: rd.da.as_ref(),
                        quadding: rd.quadding,
                    };
                    let _ = format!("{mark:?}");
                    let both = synth_redact(a.rect(), &mark);
                    if a.has_valid_rect() {
                        assert_eq!(both.as_ref().map(|(ap, _)| ap), syn.as_ref());
                    }
                    if let Some((_, overlay)) = &both {
                        let _ = format!("{overlay:?}");
                    }
                    assert_eq!(mark.bounds().is_none(), {
                        let finite = |q: &crate::QuadPoints| q.0.iter().all(|v| v.is_finite());
                        !rd.quad_points.iter().any(finite)
                    });
                    if let Some(built) = BuiltAnnot::redact_quads(&mark) {
                        let _ = format!("{built:?}");
                    }
                    #[cfg(feature = "text-layout")]
                    redact_in_fonts(a.rect(), &mark, both.as_ref());
                }
                #[cfg(feature = "text-layout")]
                if let TypedAnnot::FreeText(ft) = a.typed() {
                    free_text_in_fonts(
                        a.rect(),
                        a.contents().unwrap_or(""),
                        ft.da.as_ref(),
                        ft.quadding,
                    );
                }
                border_read_alone(&map, a, rules, &limits);
                setters_read_back(&map, a, rules, &limits);
            }
            for target in [FlattenTarget::Display, FlattenTarget::Print] {
                if let Ok(plan) = plan_flatten(
                    &map,
                    &page_dict,
                    None,
                    page_dict.get(b"Resources"),
                    FlattenOpts::for_target(target),
                    &limits,
                ) {
                    let _ = format!("{plan:?}");
                    // With redaction marks flattened too (a mark's own
                    // appearance may fail to read), nothing that was drawn
                    // or removed without them is kept back.
                    if let Ok(with_marks) = plan_flatten(
                        &map,
                        &page_dict,
                        None,
                        page_dict.get(b"Resources"),
                        FlattenOpts::for_target(target).with_redactions(true),
                        &limits,
                    ) {
                        assert!(with_marks.drawn_count >= plan.drawn_count);
                        assert!(with_marks.removed_count >= plan.removed_count);
                        assert!(with_marks.kept_annots.len() <= plan.kept_annots.len());
                    }
                }
            }
            let mut rfaults = Vec::new();
            let threads = resolve_threads(&annots, &limits, &mut rfaults);
            for t in &threads {
                let _ = format!("{t:?}");
            }

            if let Some(cat) = cat_dict.as_ref()
                && let Ok(table) = pages::pages(&map, cat, rules, &limits.semantics, &mut ())
                && let Ok(dests) =
                    DestTable::read(&map, Some(cat), &table, rules, &limits.semantics, &mut ())
            {
                let _ = format!("{dests:?}");
                for a in &annots {
                    if let TypedAnnot::Link(lnk) = a.typed()
                        && let Some(d) = lnk.effective_dest()
                    {
                        let _ = dests.resolve(d, rules, &mut ());
                    }
                }
            }
        }
    }
}

/// The fonts the `*_with_font` entry points are driven with: Helvetica, the
/// defaults' own, and Symbol, whose encoding has none of the Latin letters
/// most text holds, so the error is reached.
#[cfg(feature = "text-layout")]
const FONTS: [pdf_fonts::Std14; 2] = [pdf_fonts::Std14::Helvetica, pdf_fonts::Std14::Symbol];

/// Draws a redaction's overlay text in each of [`FONTS`]; `default` is what
/// [`synth_redact`] gave for the same mark.
#[cfg(feature = "text-layout")]
fn redact_in_fonts(
    rect: pdf_gfx::Rect,
    mark: &RedactMark<'_>,
    default: Option<&(crate::SynthesizedAp, crate::SynthesizedAp)>,
) {
    use pdf_content_streams::Res;

    for font in FONTS {
        let Ok(res) = Res::dict(font.dict()) else {
            continue;
        };
        let drawn = synth_redact_with_font(rect, mark, res.clone(), &font);
        let built = BuiltAnnot::redact_with_font(rect, mark, res.clone(), &font);
        match (&drawn, &built) {
            (Ok(forms), Ok(built)) => {
                assert_eq!(forms.as_ref().map(|f| &f.0), built.normal_ap.as_ref());
                assert_eq!(forms.as_ref().map(|f| &f.1), built.redact_overlay.as_ref());
                assert_eq!(built.dict, BuiltAnnot::redact_mark(rect, mark).dict);
                // The font is kept for the form's default resources.
                assert_eq!(built.da_font.as_ref(), Some(&res));
                assert_eq!(BuiltAnnot::redact_mark(rect, mark).da_font, None);
                // The mark never depends on the font; in the defaults' own
                // font, neither does the overlay.
                assert_eq!(
                    forms.as_ref().map(|f| &f.0),
                    default.map(|f| &f.0),
                    "the mark is the default's"
                );
                if font == pdf_fonts::Std14::Helvetica {
                    assert_eq!(forms.as_ref(), default);
                }
                let _ = format!("{built:?}");
            }
            (Err(a), Err(b)) => {
                assert_eq!(a, b);
                assert!(mark.overlay_text.is_some_and(|t| !t.is_empty()));
            }
            _ => panic!("the builder and the synthesizer disagree"),
        }
    }
}

/// Draws a `/FreeText`'s text in each of [`FONTS`].
#[cfg(feature = "text-layout")]
fn free_text_in_fonts(
    rect: pdf_gfx::Rect,
    contents: &str,
    da: Option<&DefaultAppearance>,
    quadding: crate::Quadding,
) {
    use pdf_content_streams::Res;

    let style = PaintStyle::default();
    let default = synth_free_text(rect, contents, da, quadding, &style);
    for font in FONTS {
        let Ok(res) = Res::dict(font.dict()) else {
            continue;
        };
        let drawn =
            synth_free_text_with_font(rect, contents, da, quadding, &style, res.clone(), &font);
        match &drawn {
            Ok(ap) => {
                assert_eq!(ap.is_some(), default.is_some());
                if font == pdf_fonts::Std14::Helvetica {
                    assert_eq!(ap, &default);
                }
            }
            Err(_) => assert!(!contents.is_empty()),
        }
        if let Some(da) = da {
            let built = BuiltAnnot::free_text_with_font(
                rect,
                contents,
                da,
                quadding,
                &style,
                res.clone(),
                &font,
            );
            match (&drawn, &built) {
                (Ok(ap), Ok(built)) => {
                    assert_eq!(ap, &built.normal_ap);
                    let default = BuiltAnnot::free_text(rect, contents, da, quadding, &style);
                    assert_eq!(built.dict, default.dict);
                    // The font is kept for the form's default resources.
                    assert_eq!(built.da_font.as_ref(), Some(&res));
                    assert_eq!(default.da_font, None);
                }
                (Err(a), Err(b)) => assert_eq!(a, b),
                _ => panic!("the builder and the synthesizer disagree"),
            }
        }
    }
}

/// Walks the first pages of a parsed PDF trailer and exercises `DestTable`,
/// `page_annots_with`, `page_tabs`, and `resolve_threads`.
pub fn over<'o, R: Resolve<'o> + Copy>(objects: R, trailer: &'o Dict) {
    let Ok(Some(cat)) = pdf_semantics::catalog(objects, trailer) else {
        return;
    };
    let mut sem_limits = pdf_semantics::Limits::DEFAULT;
    sem_limits.tree_depth = 16;
    sem_limits.tree_nodes = 64;
    sem_limits.pages = 16;
    sem_limits.action_chain = 32;
    let limits = Limits {
        hops: 16,
        annots_per_page: 64,
        reply_depth: 32,
        quads: 256,
        points: 1024,
        semantics: sem_limits,
        ..Limits::DEFAULT
    };
    let Ok(table) = pages::pages(objects, cat, Rules::Pdfium, &limits.semantics, &mut ()) else {
        return;
    };
    let dests = DestTable::read(
        objects,
        Some(cat),
        &table,
        Rules::Pdfium,
        &limits.semantics,
        &mut (),
    )
    .ok();
    for page in table.iter().take(4) {
        let Some(pdict) = page.dict else {
            continue;
        };
        let p1 = page_annots(objects, pdict, page.id, Rules::Pdfium, &limits);
        let mut faults = Vec::new();
        let p2 = page_annots_with(objects, pdict, page.id, Rules::Pdfium, &limits, &mut faults);
        assert_eq!(p1, p2);
        let _ = plan_flatten(
            objects,
            pdict,
            page.id,
            page.attribute(pdf_semantics::pages::Inheritable::Resources)
                .map(|a| a.written),
            FlattenOpts::default(),
            &limits,
        );
        let _ = plan_flatten(
            objects,
            pdict,
            page.id,
            page.attribute(pdf_semantics::pages::Inheritable::Resources)
                .map(|a| a.written),
            FlattenOpts::default().with_redactions(true),
            &limits,
        );
        if let Ok(annots) = p1 {
            let _ = resolve_threads(&annots, &limits, &mut ());
            if let Some(dt) = &dests {
                for a in &annots {
                    if let TypedAnnot::Link(lnk) = a.typed()
                        && let Some(d) = lnk.effective_dest()
                    {
                        let _ = dt.resolve(d, Rules::Pdfium, &mut ());
                    }
                }
            }
        }
    }
}

/// `Border::read` of `annot`'s dictionary gives the border `Annot::from_dict`
/// read for it, with a sink and without, and tells what
/// `Annot::from_dict_with` tells while it reads the border: `Annot` tells, in
/// its order of reading, what's told of the dictionary's other entries, then
/// of `/Border` alone (for `Annot::legacy_border`, whatever `/BS` says), then
/// what `Border::read_with` tells.
fn border_read_alone(
    map: &BTreeMap<ObjectId, Object>,
    annot: &Annot<'_>,
    rules: Rules,
    limits: &Limits,
) {
    let (dict, id) = (annot.dict(), annot.id());
    let read = Border::read(map, dict, id, rules, limits);
    assert_eq!(read.as_ref(), Ok(annot.border()));
    let mut told = Vec::new();
    assert_eq!(
        Border::read_with(map, dict, id, rules, limits, &mut told),
        read
    );

    let mut whole = Vec::new();
    assert!(Annot::from_dict_with(map, dict, id, rules, limits, &mut whole).is_ok());
    let mut others = dict.clone();
    for key in ["BS", "Border", "BE"] {
        others.remove(key);
    }
    let mut rest = Vec::new();
    assert!(Annot::from_dict_with(map, &others, id, rules, limits, &mut rest).is_ok());
    let mut of_border = Vec::new();
    if let Some(legacy) = dict.get(b"Border") {
        let legacy = dict! { "Border" => legacy.clone() };
        assert!(Border::read_with(map, &legacy, id, rules, limits, &mut of_border).is_ok());
    }
    of_border.extend(told);
    assert!(
        (0..=rest.len()).any(|at| whole == [&rest[..at], &of_border[..], &rest[at..]].concat())
    );
}

/// The keys of `/MK` that `AnnotMut::set_appearance_chars` writes.
const MK_KEYS: [&[u8]; 7] = [b"R", b"BC", b"BG", b"CA", b"RC", b"AC", b"TP"];

/// Each `AnnotMut` setter of `/MK`, `/AP /D`, and a state dictionary, on a
/// copy of `annot`'s dictionary, writes what it's given and keeps what it
/// says it keeps, as `Appearances::read` and `Annot::from_dict` read the copy
/// back.
fn setters_read_back(
    map: &BTreeMap<ObjectId, Object>,
    annot: &Annot<'_>,
    rules: Rules,
    limits: &Limits,
) {
    // Two forms of the fuzzer's own, at numbers no input chunk has. An input
    // may still refer to them, so what was there is read with them too.
    let forms = [ObjectId::new(1 << 20, 0), ObjectId::new((1 << 20) + 1, 0)];
    let mut objects = map.clone();
    for (byte, id) in b"01".iter().zip(forms) {
        let bbox = vec![
            Object::from(0),
            Object::from(0),
            Object::from(1),
            Object::from(1),
        ];
        let form = dict! { "Subtype" => Object::name("Form"), "BBox" => bbox };
        objects.insert(id, Object::from(Stream::new(form, vec![*byte])));
    }
    let (dict, id) = (annot.dict(), annot.id());
    let objects = &objects;
    let appearances =
        |dict: &Dict| Appearances::read(objects, dict, id, rules, limits).expect("no object fails");
    let whole = Annot::from_dict(objects, dict, id, rules, limits).is_ok();
    // `Annot::from_dict` reads the copy where it reads the dictionary, the
    // copy's appearances those `Appearances::read` reads.
    let read_back = |copy: &Dict, ap: &Option<Appearances>| {
        if whole {
            let read = Annot::from_dict(objects, copy, id, rules, limits);
            assert_eq!(read.expect("the copy is read").appearances(), ap.as_ref());
        }
    };

    let before = appearances(dict);
    let direct = AnnotMut::new(&mut dict.clone()).has_direct_appearance();
    let had_ap = dict.get(b"AP").is_some();
    // The states set: the annotation's own normal states' names but `Off`, a
    // few, or `Yes` where it has none, each given a form, and the first given
    // again with the other form. No `Off` is given, so none may be written.
    let own: Vec<&[u8]> = match before.as_ref().and_then(|ap| ap.normal.as_ref()) {
        Some(AppearanceEntry::States(states)) => states
            .iter()
            .map(|(name, _)| &**name)
            .filter(|&name| name != b"Off")
            .take(4)
            .collect(),
        _ => Vec::new(),
    };
    let names = if own.is_empty() {
        vec![&b"Yes"[..]]
    } else {
        own
    };
    let mut given: Vec<(&[u8], ObjectId)> = names
        .iter()
        .zip(forms.iter().cycle())
        .map(|(&n, &f)| (n, f))
        .collect();
    given.push((names[0], forms[1]));
    let mut once: Vec<(Box<[u8]>, ObjectId)> = Vec::new();
    for &(name, form) in &given {
        match once.iter_mut().find(|(n, _)| &**n == name) {
            Some(state) => state.1 = form,
            None => once.push((Box::from(name), form)),
        }
    }
    let one = names[names.len() - 1];

    // What a setter binding `mode` leaves of the slots it doesn't bind: as
    // they were in a direct `/AP`, none in one it made or replaced; and `/AS`
    // as it was. Where it binds nothing, all as it was.
    let others_held = |mode: AppearanceMode, bound: bool, after: &Option<Appearances>| {
        if !bound {
            assert_eq!(after, &before);
            return;
        }
        let after = after.as_ref().expect("an /AP was bound");
        for other in [
            AppearanceMode::Normal,
            AppearanceMode::Rollover,
            AppearanceMode::Down,
        ] {
            if other != mode {
                let was = before.as_ref().and_then(|ap| ap.entry(other));
                assert_eq!(after.entry(other), if direct { was } else { None });
            }
        }
        assert_eq!(after.state, before.as_ref().and_then(|ap| ap.state.clone()));
    };

    for mode in [
        AppearanceMode::Normal,
        AppearanceMode::Rollover,
        AppearanceMode::Down,
    ] {
        let bound = direct || !had_ap || mode == AppearanceMode::Normal;

        let mut copy = dict.clone();
        AnnotMut::new(&mut copy).set_appearance_states(mode, &given);
        let after = appearances(&copy);
        others_held(mode, bound, &after);
        if bound {
            let slot = after.as_ref().and_then(|ap| ap.entry(mode));
            assert_eq!(slot, Some(&AppearanceEntry::States(once.clone())));
        }
        read_back(&copy, &after);

        let mut copy = dict.clone();
        AnnotMut::new(&mut copy).set_state_appearance(mode, one, forms[0]);
        let after = appearances(&copy);
        others_held(mode, bound, &after);
        if bound {
            let Some(AppearanceEntry::States(states)) =
                after.as_ref().and_then(|ap| ap.entry(mode))
            else {
                panic!("a state dictionary is bound");
            };
            let slot_direct = direct
                && dict
                    .get(b"AP")
                    .and_then(Object::as_dict)
                    .and_then(|ap| ap.get(mode.key()))
                    .is_some_and(|slot| matches!(slot.value(), Value::Dict(_)));
            let set: Vec<_> = states.iter().filter(|(n, _)| &**n == one).collect();
            assert_eq!(set, [&(Box::from(one), forms[0])]);
            let kept: Vec<_> = states.iter().filter(|(n, _)| &**n != one).collect();
            match before.as_ref().and_then(|ap| ap.entry(mode)) {
                Some(AppearanceEntry::States(was)) if slot_direct => {
                    let was: Vec<_> = was.iter().filter(|(n, _)| &**n != one).collect();
                    assert_eq!(kept, was);
                }
                _ => assert!(kept.is_empty()),
            }
        }
        read_back(&copy, &after);
    }

    let mut copy = dict.clone();
    AnnotMut::new(&mut copy).set_down_appearance(forms[1]);
    let after = appearances(&copy);
    let bound = direct || !had_ap;
    others_held(AppearanceMode::Down, bound, &after);
    if bound {
        let down = after.as_ref().and_then(|ap| ap.down.as_ref());
        assert_eq!(down, Some(&AppearanceEntry::Single(forms[1])));
    }
    read_back(&copy, &after);

    // `/MK`: the widget's own as it was read, where it has one, and one cut
    // from what was read of the annotation.
    let own = match annot.typed() {
        TypedAnnot::Widget(widget) => widget.appearance_chars.clone(),
        _ => None,
    };
    let cut = AppearanceChars {
        rotation: 90,
        border_color: annot.color(),
        background_color: Some(AnnotColor::Transparent),
        normal_caption: annot.contents().map(String::from),
        rollover_caption: annot.name().map(String::from),
        down_caption: None,
        text_position: Some(7),
    };
    let mk_others = |dict: &Dict| -> Option<Vec<(Vec<u8>, Object)>> {
        let mk = dict.get(b"MK")?;
        let Value::Dict(mk) = mk.value() else {
            return None;
        };
        let others = mk.entries().iter().filter(|e| !MK_KEYS.contains(&e.key()));
        Some(
            others
                .map(|e| (e.key().to_vec(), e.value().clone()))
                .collect(),
        )
    };
    for chars in own.iter().chain([&cut]) {
        let mut copy = dict.clone();
        let _ = format!("{:?}", AnnotMut::new(&mut copy).set_appearance_chars(chars));
        let mk = copy.get(b"MK").and_then(Object::as_dict).expect("a /MK");
        let had: [bool; 7] = [
            chars.rotation != 0,
            chars.border_color.is_some(),
            chars.background_color.is_some(),
            chars.normal_caption.is_some(),
            chars.rollover_caption.is_some(),
            chars.down_caption.is_some(),
            chars.text_position.is_some(),
        ];
        for (key, had) in MK_KEYS.iter().zip(had) {
            assert_eq!(mk.has(key), had);
        }
        assert_eq!(
            mk_others(&copy).unwrap_or_default(),
            mk_others(dict).unwrap_or_default()
        );
        if whole && let TypedAnnot::Widget(_) = annot.typed() {
            let read =
                Annot::from_dict(objects, &copy, id, rules, limits).expect("the copy is read");
            let TypedAnnot::Widget(widget) = read.typed() else {
                panic!("a widget is read as one");
            };
            let as_written = |text: &Option<String>| {
                text.as_deref()
                    .map(|text| match Object::text(text).into_value() {
                        Value::String(Str::Bytes(bytes), _) => {
                            pdf_objects::text::decode_str(&bytes, rules)
                        }
                        _ => String::new(),
                    })
            };
            let expected = AppearanceChars {
                normal_caption: as_written(&chars.normal_caption),
                rollover_caption: as_written(&chars.rollover_caption),
                down_caption: as_written(&chars.down_caption),
                ..chars.clone()
            };
            assert_eq!(widget.appearance_chars.as_ref(), Some(&expected));
        }
    }
}

fn split_once_bytes<'a>(haystack: &'a [u8], needle: &[u8]) -> Option<(&'a [u8], &'a [u8])> {
    let pos = haystack.windows(needle.len()).position(|w| w == needle)?;
    Some((&haystack[..pos], &haystack[pos + needle.len()..]))
}

#[cfg(test)]
mod tests {
    use super::SEEDS;

    /// `PDF_ANNOTS_FUZZ_SEEDS` names a folder: writes the seed inputs there for
    /// `just fuzz annots` to begin from.
    #[test]
    #[ignore = "needs PDF_ANNOTS_FUZZ_SEEDS, a folder to write the seeds to"]
    fn the_seeds_are_written() {
        let dir = std::env::var_os("PDF_ANNOTS_FUZZ_SEEDS")
            .expect("PDF_ANNOTS_FUZZ_SEEDS names a folder");
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (i, seed) in SEEDS.iter().enumerate() {
            std::fs::write(dir.join(format!("seed-{i}")), seed).unwrap();
        }
    }
}
