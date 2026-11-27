//! Fuzzing harness and seed generator for `pdf-raster`.
//!
//! Exercises [`crate::render_page_with`] and [`crate::render_content_with`]
//! under tight resource caps across `Rules::Iso` and `Rules::Pdfium`, asserting
//! zero panics and exact `Sink<Fault>` parity (`&mut ()` vs. active sink) across
//! raw content streams, multi-object/multi-stream `FuzzStore` graphs, and full
//! parsed PDFs (`over`).

use std::borrow::Cow;
use std::collections::BTreeMap;

use pdf_content_streams::Concat;
use pdf_gfx::{PageGeom, Rect, Rotation};
use pdf_images::pdf_filters::{
    Backend as FilterBackend, Fault as FilterFault, Filter, Layout, Predictor,
    Reading as FilterReading, decode_as,
};
use pdf_objects::parse::Parser;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::streams::{
    DecodedStream, FilterPredictor, PredictorLayout, StandardFilter, parse_filter_chain,
};
use pdf_objects::{Dict, Object, ObjectId, Resolve, Stream, Streams, Unreadable, Why, dict};
use pdf_semantics::pages::pages;

use crate::{
    AntiAlias, Appearances, Draw, Fault, Limits, PixelFormat, RenderCache, RenderOpts, Viewport,
    render_content_with, render_page_with,
};

struct FuzzStore {
    objects: BTreeMap<ObjectId, Object>,
}

impl<'o> Resolve<'o> for &'o FuzzStore {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.objects.get(&id)
    }
}

impl Streams for &FuzzStore {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        let ds = self.decoded_standard(id, limit)?;
        if ds.tail.is_some() {
            return Err(Unreadable::new(id, Why::Filter, "non-standard tail"));
        }
        Ok(ds.data)
    }

    fn decoded_standard(
        &self,
        id: ObjectId,
        limit: usize,
    ) -> Result<DecodedStream<'_>, Unreadable> {
        let Some(stream) = self.objects.get(&id).and_then(Object::as_stream) else {
            return Err(Unreadable::new(id, Why::Damaged, "not a stream"));
        };
        let Some(raw) = stream.data.bytes() else {
            return Err(Unreadable::new(id, Why::Detached, "detached"));
        };
        let (chain, _) = parse_filter_chain(*self, &stream.dict)
            .map_err(|_| Unreadable::new(id, Why::Damaged, "bad filter chain"))?;
        if chain.standard.is_empty() {
            if raw.len() > limit {
                return Err(Unreadable::new(id, Why::Limit, "past limit"));
            }
            return Ok(DecodedStream::new(Cow::Borrowed(raw), chain.tail));
        }
        let filters: Vec<Filter> = chain.standard.into_iter().map(to_fuzz_filter).collect();
        match decode_as(
            raw,
            &filters,
            limit,
            FilterBackend::MinizOxide,
            FilterReading::Pdfium,
        ) {
            Ok(decoded) => Ok(DecodedStream::new(Cow::Owned(decoded), chain.tail)),
            Err(err) => {
                if err
                    .faults
                    .iter()
                    .any(|(_, f)| matches!(f, FilterFault::Limit))
                {
                    Err(Unreadable::new(id, Why::Limit, "past limit"))
                } else {
                    Err(Unreadable::new(id, Why::Damaged, "filter error"))
                }
            }
        }
    }
}

fn to_fuzz_predictor(pred: FilterPredictor) -> Predictor {
    let to_layout = |l: PredictorLayout| Layout {
        colors: l.colors,
        bits_per_component: l.bits_per_component,
        columns: l.columns,
    };
    match pred {
        FilterPredictor::None => Predictor::None,
        FilterPredictor::Tiff(l) => Predictor::Tiff(to_layout(l)),
        FilterPredictor::Png(l) => Predictor::Png(to_layout(l)),
    }
}

fn to_fuzz_filter(step: StandardFilter) -> Filter {
    match step {
        StandardFilter::Flate { predictor } => Filter::Flate {
            predictor: to_fuzz_predictor(predictor),
        },
        StandardFilter::Lzw {
            early_change,
            predictor,
        } => Filter::Lzw {
            early_change,
            predictor: to_fuzz_predictor(predictor),
        },
        StandardFilter::AsciiHex => Filter::AsciiHex,
        StandardFilter::Ascii85 => Filter::Ascii85,
        StandardFilter::RunLength => Filter::RunLength,
    }
}

fn tight_limits() -> Limits {
    let mut lim = Limits::DEFAULT;
    lim.max_width = 256;
    lim.max_height = 256;
    lim.max_pixels = 16_384;
    lim.max_pixmap_bytes = 65_536;
    lim.max_layer_depth = 6;
    lim.max_tile_side = 64;
    lim.max_tile_pixels = 4_096;
    lim.max_shading_samples = 16_384;
    lim.max_scene_bytes = 2 * 1024 * 1024;
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

/// Fuzz entry point exercising multi-stream content/Form/pattern/shading/mask/
/// Type 3/image rasterization and sink parity on arbitrary `data`.
pub fn check(data: &[u8]) {
    if data.is_empty() {
        return;
    }
    let selector = data[0];
    let payload = &data[1..];
    let lim = tight_limits();
    let cache = RenderCache::new();

    let n = payload.len();
    let s1 = &payload[..n / 2];
    let s2 = &payload[n / 2..];

    let mut store = FuzzStore {
        objects: BTreeMap::new(),
    };
    // Indirect Form XObject (`/Fm1`, id 10), Image XObject (`/Im1`, id 11),
    // Tiling Pattern (`/P1`, id 12), Axial Shading (`/Sh1`, id 13), Soft-Mask
    // Form (`/G`, id 14), and Type 3 Font (`/F3`, id 15 + CharProc id 16):
    store.objects.insert(
        ObjectId::new(10, 0),
        Object::from(Stream::new(
            dict! {
                "Subtype" => Object::name("Form"),
                "BBox" => vec![Object::from(0), Object::from(0), Object::from(32), Object::from(32)],
                "Group" => dict! { "S" => Object::name("Transparency"), "I" => (selector & 8) == 0 },
            },
            s2.to_vec(),
        )),
    );
    store.objects.insert(
        ObjectId::new(11, 0),
        Object::from(Stream::new(
            dict! {
                "Subtype" => Object::name("Image"),
                "Width" => 4,
                "Height" => 4,
                "ColorSpace" => Object::name("DeviceRGB"),
                "BitsPerComponent" => 8,
            },
            s2.to_vec(),
        )),
    );
    store.objects.insert(
        ObjectId::new(12, 0),
        Object::from(Stream::new(
            dict! {
                "Type" => Object::name("Pattern"),
                "PatternType" => 1,
                "PaintType" => if selector & 32 == 0 { 1 } else { 2 },
                "TilingType" => 1,
                "BBox" => vec![Object::from(0), Object::from(0), Object::from(8), Object::from(8)],
                "XStep" => 8,
                "YStep" => 8,
            },
            s2.to_vec(),
        )),
    );
    store.objects.insert(
        ObjectId::new(13, 0),
        Object::from(dict! {
            "ShadingType" => 2,
            "ColorSpace" => Object::name("DeviceRGB"),
            "Coords" => vec![Object::from(0), Object::from(0), Object::from(32), Object::from(32)],
            "Function" => dict! {
                "FunctionType" => 2,
                "Domain" => vec![Object::from(0), Object::from(1)],
                "C0" => vec![Object::from(1.0), Object::from(0.0), Object::from(0.0)],
                "C1" => vec![Object::from(0.0), Object::from(0.0), Object::from(1.0)],
                "N" => 1,
            },
        }),
    );
    store.objects.insert(
        ObjectId::new(14, 0),
        Object::from(Stream::new(
            dict! {
                "Subtype" => Object::name("Form"),
                "BBox" => vec![Object::from(0), Object::from(0), Object::from(32), Object::from(32)],
                "Group" => dict! { "S" => Object::name("Transparency"), "CS" => Object::name("DeviceGray") },
            },
            s2.to_vec(),
        )),
    );
    store.objects.insert(
        ObjectId::new(15, 0),
        Object::from(dict! {
            "Type" => Object::name("Font"),
            "Subtype" => Object::name("Type3"),
            "FontBBox" => vec![Object::from(0), Object::from(0), Object::from(1), Object::from(1)],
            "FontMatrix" => vec![
                Object::from(1), Object::from(0), Object::from(0),
                Object::from(1), Object::from(0), Object::from(0),
            ],
            "Encoding" => dict! {
                "Type" => Object::name("Encoding"),
                "Differences" => vec![Object::from(65), Object::name("ga")],
            },
            "CharProcs" => dict! { "ga" => ObjectId::new(16, 0) },
            "FirstChar" => 65,
            "LastChar" => 65,
            "Widths" => vec![Object::from(1)],
        }),
    );
    store.objects.insert(
        ObjectId::new(16, 0),
        Object::from(Stream::new(dict! {}, s2.to_vec())),
    );

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
            "F3" => ObjectId::new(15, 0),
        },
        "XObject" => dict! {
            "Fm1" => ObjectId::new(10, 0),
            "Im1" => ObjectId::new(11, 0),
        },
        "Pattern" => dict! {
            "P1" => ObjectId::new(12, 0),
        },
        "Shading" => dict! {
            "Sh1" => ObjectId::new(13, 0),
        },
        "ExtGState" => dict! {
            "GS1" => dict! {
                "SMask" => dict! {
                    "S" => Object::name("Luminosity"),
                    "G" => ObjectId::new(14, 0),
                },
            },
        },
    });
    let concat = Concat::new(&[s1]);
    let mut faults = Vec::<(pdf_objects::fault::Place, Fault)>::new();
    let r_sink = render_content_with(
        &store,
        &concat,
        Some(&res),
        &geom,
        &cache,
        None,
        &opts,
        &mut faults,
    );
    let r_unit = render_content_with(
        &store,
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

    // 2. Parse `payload` as a PDF object dict/page if small enough.
    let mut parser = Parser::at(payload, 0)
        .rules(opts.rules)
        .limits(pdf_objects::Limits::new(32, 4_096));
    if let Ok(parsed) = parser.value(&mut ())
        && let Some(d) = parsed.as_dict()
    {
        store.objects.insert(ObjectId::new(1, 0), parsed.clone());
        let catalog = if d.has(b"Pages") {
            d.clone()
        } else {
            let mut pd = d.clone();
            pd.set("Type", Object::name("Page"));
            pd.set("Parent", ObjectId::new(2, 0));
            if !pd.has(b"Contents") {
                store.objects.insert(
                    ObjectId::new(4, 0),
                    Object::from(Stream::new(dict! {}, s1.to_vec())),
                );
                pd.set("Contents", ObjectId::new(4, 0));
            }
            store.objects.insert(ObjectId::new(3, 0), Object::from(pd));
            store.objects.insert(
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
        opts.draw = Draw::ALL;
        opts.appearances = if selector & 128 == 0 {
            Appearances::Synthesize
        } else {
            Appearances::WrittenOnly
        };
        if let Ok(tbl) = pages(&store, &catalog, opts.rules, &lim.semantics, &mut ())
            && let Some(page) = tbl.get(0)
        {
            let mut pf = Vec::new();
            let a = render_page_with(&store, Some(&catalog), page, &cache, None, &opts, &mut pf);
            let b = render_page_with(&store, Some(&catalog), page, &cache, None, &opts, &mut ());
            assert_eq!(a.is_ok(), b.is_ok());
            if let (Ok((pa, sa)), Ok((pb, sb))) = (a, b) {
                assert_eq!(pa, pb);
                assert_eq!(sa, sb);
            }
        }
    }
}

/// Alias of [`check`] retained for callers invoking `fuzz::run`.
pub fn run(data: &[u8]) {
    check(data);
}

/// Exercises full-document page rasterization over any `Resolve + Streams`
/// provider (`Reading` or `Document`), verifying sink parity on up to 4 pages
/// under both `Rules::Pdfium` and `Rules::Iso`.
pub fn over<'o, O>(objects: O, trailer: &'o Dict)
where
    O: Resolve<'o> + Streams + Copy,
{
    let lim = tight_limits();
    let Ok(Some(catalog)) = pdf_semantics::catalog(objects, trailer) else {
        return;
    };
    let cache = RenderCache::new();
    for rules in [Rules::Pdfium, Rules::Iso] {
        let Ok(tbl) = pages(objects, catalog, rules, &lim.semantics, &mut ()) else {
            continue;
        };
        let mut opts = RenderOpts::page_width(32);
        opts.rules = rules;
        opts.limits = lim;
        for page in tbl.iter().take(4) {
            let mut faults = Vec::new();
            let loud = render_page_with(
                objects,
                Some(catalog),
                page,
                &cache,
                None,
                &opts,
                &mut faults,
            );
            let silent =
                render_page_with(objects, Some(catalog), page, &cache, None, &opts, &mut ());
            assert_eq!(loud.is_ok(), silent.is_ok());
            if let (Ok((pa, sa)), Ok((pb, sb))) = (loud, silent) {
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
            b"\x001 0 0 rg 10 10 30 30 re f\x00".to_vec(),
            b"\x01q 2 w [3 1] 0 d 5 5 m 50 50 l S Q\x00".to_vec(),
            b"\x02BT /F1 12 Tf 10 20 Td (Hello) Tj /F3 12 Tf (A) Tj ET1 0 d0 0 0 1 rg 0 0 1 1 re f"
                .to_vec(),
            b"\x03q 10 10 40 40 re W n 0 0 1 rg 0 0 64 64 re f Q\x00".to_vec(),
            b"\x04/Pattern cs /P1 scn 0 0 32 32 re f /Sh1 sh1 0 0 rg 0 0 4 4 re f".to_vec(),
            b"\x05q /GS1 gs 16 0 0 16 4 4 cm /Im1 Do /Fm1 Do Q1 1 1 rg 0 0 32 32 re f".to_vec(),
        ]
    }

    #[test]
    fn fuzz_seeds_mutations_and_random_bytes_keep_every_promise() {
        for seed in seed_inputs() {
            check(&seed);
            for idx in 0..seed.len().min(24) {
                let mut mutated = seed.clone();
                mutated[idx] ^= 0x3C;
                check(&mutated);
            }
        }

        let mut state = 0x517C_C1B7_2722_0A95_u64;
        for len in [4_usize, 19, 48, 128] {
            let mut buf = vec![0_u8; len];
            for b in &mut buf {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *b = (state >> 24) as u8;
            }
            check(&buf);
        }

        let pdf_bytes = pdf_fixtures::fixture::file(
            "1.7",
            &[
                (
                    1,
                    pdf_fixtures::fixture::Body::Object(Object::from(dict! {
                        "Type" => Object::name("Catalog"),
                        "Pages" => Object::reference(2),
                    })),
                ),
                (
                    2,
                    pdf_fixtures::fixture::Body::Object(Object::from(dict! {
                        "Type" => Object::name("Pages"),
                        "Kids" => vec![Object::reference(3)],
                        "Count" => 1,
                        "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(64), Object::from(64)],
                    })),
                ),
                (
                    3,
                    pdf_fixtures::fixture::Body::Object(Object::from(dict! {
                        "Type" => Object::name("Page"),
                        "Parent" => Object::reference(2),
                        "Contents" => Object::reference(4),
                    })),
                ),
                (
                    4,
                    pdf_fixtures::fixture::Body::Stream(
                        dict! {},
                        b"0 0.5 1 rg 8 8 32 32 re f".to_vec(),
                    ),
                ),
            ],
            dict! { "Root" => Object::reference(1) },
            pdf_fixtures::fixture::Xref::Table,
        );
        let reading =
            pdf_structure::Reading::open(&pdf_bytes, &pdf_structure::Options::default()).unwrap();
        over(&reading, reading.trailer().dict());
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
