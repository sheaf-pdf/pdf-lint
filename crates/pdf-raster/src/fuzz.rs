//! Fuzzing harness and seed generator for `pdf-raster`.
//!
//! Exercises [`crate::render_page_with`] and [`crate::render_content_with`]
//! under tight resource caps across `Rules::Iso` and `Rules::Pdfium`, asserting
//! zero panics and exact `Sink<Fault>` parity (`&mut ()` vs. active sink).

use std::collections::BTreeMap;

use pdf_content_streams::Concat;
use pdf_gfx::{PageGeom, Rect, Rotation};
use pdf_objects::parse::Parser;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Object, ObjectId, Unreadable, Why, WithStreams, dict};
use pdf_semantics::pages::pages;

use crate::{
    AntiAlias, Appearances, Draw, Fault, Limits, PixelFormat, RenderCache, RenderOpts, Viewport,
    render_content_with, render_page_with,
};

fn tight_limits() -> Limits {
    let mut lim = Limits::DEFAULT;
    lim.max_width = 256;
    lim.max_height = 256;
    lim.max_pixels = 16_384;
    lim.max_pixmap_bytes = 65_536;
    lim.max_layer_depth = 6;
    lim.max_pattern_cells = 64;
    lim.max_shading_samples = 16_384;
    lim.max_annots = 32;
    lim.gfx.operators = 2_048;
    lim.gfx.segments = 4_096;
    lim.gfx.glyphs = 2_048;
    lim.gfx.decompressed = 65_536;
    lim.gfx.interpreted = 65_536;
    lim.gfx.form_depth = 4;
    lim.images.max_width = 128;
    lim.images.max_height = 128;
    lim.images.max_pixels = 8_192;
    lim.images.max_sample_bytes = 32_768;
    lim.images.max_encoded_bytes = 32_768;
    lim.color.stream_bytes = 16_384;
    lim.color.sample_values = 8_192;
    lim.color.postscript_ops = 512;
    lim
}

/// Fuzz entry point exercising both raw content-stream rasterization and full
/// PDF object-graph rasterization.
pub fn run(data: &[u8]) {
    if data.is_empty() {
        return;
    }
    let selector = data[0];
    let payload = &data[1..];
    let lim = tight_limits();
    let cache = RenderCache::new();

    // 1. Direct content-stream fuzzing with synthetic resources.
    let objects = BTreeMap::<ObjectId, Object>::new();
    let provider = WithStreams::new(&objects, |id, _| {
        Err(Unreadable::new(id, Why::Damaged, "no streams"))
    });
    let geom = PageGeom::new(
        Rect::new(0.0, 0.0, 64.0, 64.0),
        None,
        match (selector >> 2) & 3 {
            1 => Rotation::R90,
            2 => Rotation::R180,
            3 => Rotation::R270,
            _ => Rotation::R0,
        },
    );
    let mut opts = RenderOpts::with_viewport(Viewport::Page {
        width: 32,
        rotation: Rotation::R0,
    });
    opts.limits = lim;
    opts.rules = if selector & 1 == 0 {
        Rules::Pdfium
    } else {
        Rules::Iso
    };
    opts.anti_alias = if selector & 2 == 0 {
        AntiAlias::ALL
    } else {
        AntiAlias::NONE
    };
    opts.format = match (selector >> 4) & 3 {
        1 => PixelFormat::Bgra8,
        2 => PixelFormat::Rgba8Premul,
        _ => PixelFormat::Rgba8,
    };
    opts.background = if selector & 64 == 0 {
        Some([255, 255, 255, 255])
    } else {
        None
    };

    let res = Object::from(dict! {
        "Font" => dict! {
            "F1" => pdf_fonts::Std14::Helvetica.dict(),
        },
    });
    let concat = Concat::new(&[payload]);
    let mut faults = Vec::<(pdf_objects::fault::Place, Fault)>::new();
    let r_sink = render_content_with(
        provider,
        &concat,
        Some(&res),
        &geom,
        &cache,
        None,
        &opts,
        &mut faults,
    );
    let r_unit = render_content_with(
        provider,
        &concat,
        Some(&res),
        &geom,
        &cache,
        None,
        &opts,
        &mut (),
    );
    match (r_sink, r_unit) {
        (Ok((p1, s1)), Ok((p2, s2))) => {
            assert_eq!(p1, p2, "Sink<Fault> must not alter rendered pixels");
            assert_eq!(s1, s2, "Sink<Fault> must not alter RenderSummary");
        }
        (Err(_), Err(_)) => {}
        (a, b) => panic!("sink parity mismatch: {a:?} vs {b:?}"),
    }

    // 2. Parse `payload` as a PDF object dict/page if small enough, exercising
    //    page tree, annotations, forms, patterns, and shadings.
    let mut parser = Parser::at(payload, 0)
        .rules(opts.rules)
        .limits(pdf_objects::Limits::new(32, 4_096));
    if let Ok(parsed) = parser.value(&mut ())
        && let Some(d) = parsed.as_dict()
    {
        let mut map = BTreeMap::new();
        map.insert(ObjectId::new(1, 0), parsed.clone());
        let page_dict = if d.has(b"Pages") {
            d.clone()
        } else {
            let mut pd = d.clone();
            pd.set("Type", Object::name("Page"));
            pd.set("Parent", ObjectId::new(2, 0));
            map.insert(ObjectId::new(3, 0), Object::from(pd));
            map.insert(
                ObjectId::new(2, 0),
                Object::from(dict! {
                    "Type" => Object::name("Pages"),
                    "Kids" => vec![Object::reference(3)],
                    "Count" => 1,
                    "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(64), Object::from(64)],
                }),
            );
            dict! { "Type" => Object::name("Catalog"), "Pages" => ObjectId::new(2, 0) }
        };
        let prov = WithStreams::new(&map, |id, cap| {
            let bytes = map
                .get(&id)
                .and_then(Object::as_stream)
                .and_then(|s| s.data.bytes())
                .ok_or_else(|| Unreadable::new(id, Why::Damaged, "no stream"))?;
            if bytes.len() > cap {
                return Err(Unreadable::new(id, Why::Limit, "past cap"));
            }
            Ok(bytes.to_vec())
        });
        opts.draw = Draw::ALL;
        opts.appearances = if selector & 128 == 0 {
            Appearances::Synthesize
        } else {
            Appearances::WrittenOnly
        };
        if let Ok(tbl) = pages(prov, &page_dict, opts.rules, &lim.semantics, &mut ())
            && let Some(page) = tbl.get(0)
        {
            let mut pf = Vec::new();
            let a = render_page_with(prov, Some(&page_dict), page, &cache, None, &opts, &mut pf);
            let b = render_page_with(prov, Some(&page_dict), page, &cache, None, &opts, &mut ());
            assert_eq!(a.is_ok(), b.is_ok());
            if let (Ok((pa, sa)), Ok((pb, sb))) = (a, b) {
                assert_eq!(pa, pb);
                assert_eq!(sa, sb);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_inputs() -> Vec<Vec<u8>> {
        vec![
            b"\x001 0 0 rg 10 10 30 30 re f".to_vec(),
            b"\x01q 2 w [3 1] 0 d 5 5 m 50 50 l S Q".to_vec(),
            b"\x02BT /F1 12 Tf 10 20 Td (Hello) Tj ET".to_vec(),
            b"\x03q 10 10 40 40 re W n 0 0 1 rg 0 0 64 64 re f Q".to_vec(),
        ]
    }

    #[test]
    fn fuzz_seeds_run_cleanly() {
        for seed in seed_inputs() {
            run(&seed);
        }
    }

    #[test]
    #[ignore = "invoked by `just fuzz raster` to populate fuzz/seeds/raster"]
    fn the_seeds_are_written() {
        let dir = std::env::var("PDF_RASTER_FUZZ_SEEDS").expect("PDF_RASTER_FUZZ_SEEDS set");
        std::fs::create_dir_all(&dir).unwrap();
        for (i, seed) in seed_inputs().into_iter().enumerate() {
            std::fs::write(format!("{dir}/seed-{i:02}.bin"), seed).unwrap();
        }
    }
}
