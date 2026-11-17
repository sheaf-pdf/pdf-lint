//! In-process fuzzing entrypoints exercised by unit tests and `fuzz/fuzz_targets/annots.rs`.
//!
//! Verifies on arbitrary byte inputs:
//! 1. Zero panics across `page_annots`, `page_annots_with`, `page_tabs`,
//!    `DestTable::read`, `resolve_threads`, and `stream_placement_matrix`.
//! 2. Sink invariance: `&mut ()` and `Vec` fault sinks produce identical
//!    `Result<Vec<Annot>, Error>` under both `Rules::Iso` and `Rules::Pdfium`.
//! 3. `Debug` formatting never panics on any parsed `Annot`, `DestTable`,
//!    `AnnotThread`, or `Fault`.

use std::collections::BTreeMap;

use pdf_objects::parse::Parser;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Stream, Value};
use pdf_semantics::dest::DestTable;
use pdf_semantics::pages;

use crate::annot::{page_annots, page_annots_with, page_tabs};
use crate::ap::{AppearanceMode, stream_placement_matrix};
use crate::flatten::{FlattenOpts, FlattenTarget, plan_flatten};
use crate::limits::Limits;
use crate::replies::resolve_threads;
use crate::style::DefaultAppearance;
use crate::subtypes::TypedAnnot;
use crate::synth::synth_annot;

/// Built-in seed inputs exercising links, destinations, action chains, text
/// markup, geometric annotations, ink, redaction, replies/review states, and
/// appearance dictionaries.
pub const SEEDS: &[&[u8]] = &[
    b"",
    b"<< /Annots [1 0 R 2 0 R] /Tabs /S >>\xff<< /Type /Catalog /Pages 3 0 R /Names << /Dests << /Names [(sec1) [4 0 R /XYZ 0 700 null]] >> >> >>\xff<< /Subtype /Link /Rect [10 20 100 40] /Dest (sec1) /Border [0 0 1] /F 4 >>\xff<< /Subtype /Highlight /Rect [10 50 120 70] /QuadPoints [10 70 120 70 10 50 120 50] /C [1 1 0] /T (Alice) /Contents (Check this) >>\xff<< /Type /Pages /Kids [4 0 R] /Count 1 >>\xff<< /Type /Page /Parent 3 0 R /MediaBox [0 0 612 792] >>",
    b"<< /Annots [1 0 R 2 0 R 3 0 R] >>\xff<< /Type /Catalog /Pages 4 0 R >>\xff<< /Subtype /Text /Rect [20 20 40 40] /Name /Comment /T (Bob) /M (D:20261115120000Z) >>\xff<< /Subtype /Text /Rect [20 20 40 40] /IRT 1 0 R /StateModel (Review) /State (Accepted) >>\xff<< /Subtype /FreeText /Rect [50 50 200 100] /DA (/Helv 12 Tf 0 0 1 rg) /Q 1 /CL [50 50 30 30] /LE /OpenArrow >>\xff<< /Type /Pages /Kids [5 0 R] /Count 1 >>\xff<< /Type /Page /Parent 4 0 R /MediaBox [0 0 200 200] >>",
    b"<< /Annots [1 0 R 2 0 R] >>\xff<< /Type /Catalog >>\xff<< /Subtype /Ink /Rect [0 0 100 100] /InkList [[10 10 20 30 40 25]] /BS << /W 2 /S /D /D [4 2] >> >>\xff<< /Subtype /Redact /Rect [10 10 80 30] /IC [0 0 0] /OverlayText (REDACTED) /DA (/Helv 10 Tf 1 g) /AP << /N 3 0 R >> >>\xff<< /Subtype /Form /BBox [0 0 70 20] >>\nstream\n0 g 0 0 70 20 re f",
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
                if let Some(syn) = synth_annot(a) {
                    let _ = format!("{syn:?}");
                }
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
