//! Integration tests for `pdf-raster`.

use std::sync::Arc;

use pdf_fixtures::fixture::{Body as FixtureBody, Xref, file as write_fixture_file};
use pdf_fixtures::font::{Glyph as FixtureGlyph, TrueType};
use pdf_raster::pdf_annots::{AnnotColor, AnnotFlags, BuiltAnnot, ShapeKind};
use pdf_raster::pdf_content_streams::{Color, Concat, Reserved};
use pdf_raster::pdf_fonts::Std14;
use pdf_raster::pdf_gfx::{Canvas, PageGeom, Rect, Rotation};
use pdf_raster::pdf_objects::pdf_tokenizer::Rules;
use pdf_raster::pdf_objects::{Object, ObjectId, Resolve, Stream, Value, dict};
use pdf_raster::pdf_semantics::Limits as SemLimits;
use pdf_raster::pdf_semantics::pages::pages;
use pdf_raster::{
    AntiAlias, Appearances, Draw, Error, LimitKind, PixelFormat, Pixmap, RenderCache, RenderMedium,
    RenderOpts, Viewport, render_content, render_page, render_page_with,
};
use pdf_structure::read::Recovery;
use pdf_structure::{Options as OpenOptions, Reading};

/// Helper: serializes an object map `(id >= 1, id 1 = Catalog)` into valid PDF
/// bytes, opens it through `pdf_structure::Reading::open`, and renders page `0`
/// via `pdf_raster::render_page_with`.
fn build_pdf_bytes(objects: Vec<(u32, Object)>) -> Vec<u8> {
    let bodies: Vec<(u32, FixtureBody)> = objects
        .into_iter()
        .map(|(id, obj)| match obj.into_value() {
            Value::Stream(s) => (
                id,
                FixtureBody::Stream(s.dict, s.data.bytes().unwrap_or(&[]).to_vec()),
            ),
            other => (id, FixtureBody::Object(Object::from(other))),
        })
        .collect();
    write_fixture_file(
        "1.7",
        &bodies,
        dict! { "Root" => Object::reference(1) },
        Xref::Table,
    )
}

fn render_pdf_bytes(
    pdf_bytes: &[u8],
    opts: &RenderOpts,
) -> (Pixmap, pdf_raster::RenderSummary, Vec<pdf_raster::Fault>) {
    let mut open = OpenOptions::DEFAULT;
    open.recovery = match opts.rules {
        Rules::Iso => Recovery::Verified,
        Rules::Pdfium | _ => Recovery::Pdfium,
    };
    let reading = Reading::open(pdf_bytes, &open).expect("valid test PDF");
    let catalog_obj = reading.trailer().dict().get(b"Root").expect("Root ref");
    let catalog = (&reading)
        .resolve(catalog_obj.as_reference().unwrap())
        .unwrap()
        .as_dict()
        .unwrap();
    let tbl = pages(&reading, catalog, opts.rules, &SemLimits::DEFAULT, &mut ()).unwrap();
    let page = tbl.get(0).expect("page 0");
    let cache = RenderCache::new();
    let mut raw_faults = Vec::new();
    let (pix, sum) = render_page_with(
        &reading,
        Some(catalog),
        page,
        &cache,
        None,
        opts,
        &mut raw_faults,
    )
    .expect("render_page_with succeeds");
    let faults = raw_faults.into_iter().map(|(_, f)| f).collect();
    (pix, sum, faults)
}

// ---------------------------------------------------------------------------
// 1. Vector Paths, Fills (`NonZero` vs `EvenOdd`), Strokes, Dashes & Clips
// ---------------------------------------------------------------------------

#[test]
fn vector_paths_even_odd_winding_anisotropic_strokes_and_clips() {
    // Nested concentric squares `[10, 10, 90, 90]` and `[30, 30, 70, 70]` (same
    // winding): `f*` (`EvenOdd`) leaves a white hole in the center, whereas `f`
    // (`NonZero`) fills solid!
    let even_odd_stream = b"\
        0 0.4 0.8 rg \
        10 10 80 80 re 30 30 40 40 re f*\n";
    let pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(100)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Contents" => Object::reference(4),
            }),
        ),
        (
            4,
            Object::from(Stream::new(dict! {}, even_odd_stream.to_vec())),
        ),
    ]);

    let mut opts = RenderOpts::with_viewport(Viewport::Page {
        width: 100,
        rotation: Rotation::R0,
    });
    opts.anti_alias = AntiAlias::NONE;
    let (pix, _, faults) = render_pdf_bytes(&pdf, &opts);
    assert!(faults.is_empty());
    // Border ring `(20, 50)` is painted blue; center `(50, 50)` is untouched white!
    assert_eq!(pix.pixel_rgba8(20, 50), Some([0, 102, 204, 255]));
    assert_eq!(pix.pixel_rgba8(50, 50), Some([255, 255, 255, 255]));

    // Anisotropic CTM stroke: `4 0 0 1 0 0 cm 4 w`: vertical stroke (along y,
    // spanning x) has thickness `4 * 4 = 16` px, horizontal stroke (along x,
    // spanning y) has thickness `4 * 1 = 4` px!
    let aniso_stream = b"\
        q \
        20 20 60 60 re W n \
        1 0 0 RG \
        4 0 0 1 0 0 cm \
        4 w \
        12.5 10 m 12.5 90 l S \
        Q\n";
    let geom = PageGeom::new(Rect::new(0.0, 0.0, 100.0, 100.0), None, Rotation::R0);
    let concat = Concat::new(&[&aniso_stream[..]]);
    let empty = std::collections::BTreeMap::<ObjectId, Object>::new();
    let prov = pdf_raster::pdf_objects::WithStreams::new(&empty, |id, _| {
        Err(pdf_raster::pdf_objects::Unreadable::new(
            id,
            pdf_raster::pdf_objects::Why::Damaged,
            "none",
        ))
    });
    let (apix, _) =
        render_content(prov, &concat, None, &geom, &RenderCache::new(), None, &opts).unwrap();
    // Vertical line centered at `x = 4 * 12.5 = 50`, width `4 * 4 = 16` (`42..58`),
    // clipped to `y in 20..80` (`ViewSpace y in 20..80`):
    assert_eq!(apix.pixel_rgba8(50, 50), Some([255, 0, 0, 255]));
    assert_eq!(apix.pixel_rgba8(44, 50), Some([255, 0, 0, 255]));
    assert_eq!(apix.pixel_rgba8(38, 50), Some([255, 255, 255, 255]));
    // Clipped above `y = 20` (`y = 10` is outside `[20..80]` clip!):
    assert_eq!(apix.pixel_rgba8(50, 10), Some([255, 255, 255, 255]));
}

// ---------------------------------------------------------------------------
// 2. Embedded TrueType, Type 3 (`d0` & `d1`), Text Clipping (`7 Tr`), & Fallback
// ---------------------------------------------------------------------------

fn build_fixture_ttf() -> Vec<u8> {
    let mut tt = TrueType::new(1000);
    // Glyph 1 ('A'): solid block `[100, 0, 700, 750]`, advance 800.
    let g_a = tt.glyph(FixtureGlyph::Box(100, 0, 700, 750), 800);
    tt.map('A', g_a);
    // Glyph 2 ('B'): two horizontal bars via composite, advance 800.
    let g_bar = tt.glyph(FixtureGlyph::Box(100, 0, 700, 300), 800);
    let g_b = tt.glyph(
        FixtureGlyph::Composite(vec![(g_bar, 0, 0), (g_bar, 0, 450)]),
        800,
    );
    tt.map('B', g_b);
    // Glyph 3 (' '): empty space, advance 400.
    let g_sp = tt.glyph(FixtureGlyph::Empty, 400);
    tt.map(' ', g_sp);
    tt.build()
}

#[cfg(feature = "programs")]
#[test]
fn embedded_truetype_text_clipping_and_type3_d0_d1_glyphs() {
    let ttf_bytes = build_fixture_ttf();

    // Test `7 Tr` glyph-outline clipping: clip to `'A'` at `(10, 10)` size `100`
    // (`x in [10 + 10, 10 + 70] = [20, 80]`, `y in [10, 85]`), then paint full-page
    // green! Only pixels inside `'A'`'s outline become green.
    let content = b"\
        q \
        BT /F1 100 Tf 7 Tr 10 10 Td (A) Tj ET \
        0 0.75 0.25 rg 0 0 100 100 re f \
        Q \
        0.8 0.1 0.1 rg \
        BT /F3 50 Tf 10 10 Td (XY) Tj ET\n";

    // Type 3 font `/F3`:
    // - `'X'` (`88`): `d1` uncolored stencil `[0.1, 0.1, 0.9, 0.9]` -> takes caller red `[0.8, 0.1, 0.1]`!
    // - `'Y'` (`89`): `d0` colored glyph setting its own blue `0 0 1 rg`!
    let charproc_x = b"1 0 0.1 0.1 0.9 0.9 d1 0.1 0.1 0.8 0.8 re f";
    let charproc_y = b"1 0 d0 0 0 1 rg 0.1 0.1 0.8 0.8 re f";

    let pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(120), Object::from(100)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Resources" => dict! {
                    "Font" => dict! {
                        "F1" => Object::reference(5),
                        "F3" => Object::reference(8),
                    },
                },
                "Contents" => Object::reference(4),
            }),
        ),
        (4, Object::from(Stream::new(dict! {}, content.to_vec()))),
        (
            5,
            Object::from(dict! {
                "Type" => Object::name("Font"),
                "Subtype" => Object::name("TrueType"),
                "BaseFont" => Object::name("FixtureSans-Regular"),
                "Encoding" => Object::name("WinAnsiEncoding"),
                "FirstChar" => 32,
                "LastChar" => 66,
                "Widths" => {
                    let mut w = vec![Object::from(500); 35];
                    w[0] = Object::from(400); // ' '
                    w[33] = Object::from(800); // 'A'
                    w[34] = Object::from(800); // 'B'
                    w
                },
                "FontDescriptor" => Object::reference(6),
            }),
        ),
        (
            6,
            Object::from(dict! {
                "Type" => Object::name("FontDescriptor"),
                "FontName" => Object::name("FixtureSans-Regular"),
                "Flags" => 32,
                "FontBBox" => vec![Object::from(0), Object::from(-200), Object::from(1000), Object::from(800)],
                "ItalicAngle" => 0,
                "Ascent" => 800,
                "Descent" => -200,
                "CapHeight" => 750,
                "StemV" => 80,
                "FontFile2" => Object::reference(7),
            }),
        ),
        (
            7,
            Object::from(Stream::new(
                dict! { "Length1" => ttf_bytes.len() as i64 },
                ttf_bytes,
            )),
        ),
        (
            8,
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
                    "Differences" => vec![Object::from(88), Object::name("gx"), Object::name("gy")],
                },
                "CharProcs" => dict! {
                    "gx" => Object::reference(9),
                    "gy" => Object::reference(10),
                },
                "FirstChar" => 88,
                "LastChar" => 89,
                "Widths" => vec![Object::from(1), Object::from(1)],
            }),
        ),
        (9, Object::from(Stream::new(dict! {}, charproc_x.to_vec()))),
        (10, Object::from(Stream::new(dict! {}, charproc_y.to_vec()))),
    ]);

    let mut opts = RenderOpts::with_viewport(Viewport::Page {
        width: 120,
        rotation: Rotation::R0,
    });
    opts.anti_alias = AntiAlias::NONE;
    opts.hint_fonts = false;
    let (pix, sum, _) = render_pdf_bytes(&pdf, &opts);
    assert_eq!(sum.substituted_glyphs, 0);

    // Upper part of `'A'` clip (`x = 50, page_y = 75 -> dev_y = 25`, above Type 3 glyphs): green!
    assert_eq!(pix.pixel_rgba8(50, 25), Some([0, 191, 64, 255]));
    // Outside `'A'` clip (`x = 5, dev_y = 25`): untouched white!
    assert_eq!(pix.pixel_rgba8(5, 25), Some([255, 255, 255, 255]));
    // Type 3 `'X'` (`d1` uncolored stencil at `x in [15, 55], page_y in [15, 55] -> dev_y in [45, 85]`): caller red!
    assert_eq!(pix.pixel_rgba8(35, 65), Some([204, 26, 26, 255]));
    // Type 3 `'Y'` (`d0` colored glyph at `x in [65, 105], dev_y in [45, 85]`): own blue!
    assert_eq!(pix.pixel_rgba8(85, 65), Some([0, 0, 255, 255]));
}

#[test]
fn caller_font_fallback_and_builtin_glyphs_render_unembedded_text() {
    let ttf_bytes: Arc<[u8]> = Arc::from(build_fixture_ttf());
    let content = b"0 0 0 rg BT /F1 40 Tf 10 20 Td (AB) Tj ET\n";
    let pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(80)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Resources" => dict! {
                    "Font" => dict! { "F1" => Std14::Helvetica.dict() },
                },
                "Contents" => Object::reference(4),
            }),
        ),
        (4, Object::from(Stream::new(dict! {}, content.to_vec()))),
    ]);

    let reading = Reading::open(&pdf[..], &OpenOptions::DEFAULT).unwrap();
    let cat = (&reading)
        .resolve(
            reading
                .trailer()
                .dict()
                .get(b"Root")
                .unwrap()
                .as_reference()
                .unwrap(),
        )
        .unwrap()
        .as_dict()
        .unwrap();
    let tbl = pages(&reading, cat, Rules::Pdfium, &SemLimits::DEFAULT, &mut ()).unwrap();
    let page = tbl.get(0).unwrap();
    let opts = RenderOpts::with_viewport(Viewport::Page {
        width: 100,
        rotation: Rotation::R0,
    });

    // 1. Built-in vector fallback (`fallback = None`): renders non-white ink!
    let (pix_builtin, sum_builtin) =
        render_page(&reading, Some(cat), page, &RenderCache::new(), None, &opts).unwrap();
    assert_eq!(sum_builtin.substituted_glyphs, 2);
    assert!(sum_builtin.approximated);
    let non_white_builtin = pix_builtin
        .as_bytes()
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|&&px| px != [255, 255, 255, 255])
        .count();
    assert!(non_white_builtin > 50, "built-in glyphs must paint ink");

    // 2. Caller `FontFallback` (`Some(&|_| Some(ttf_bytes.clone()))`): uses caller face!
    #[cfg(feature = "programs")]
    {
        let fb = move |_: &pdf_raster::FontQuery| Some(ttf_bytes.clone());
        let (pix_fb, sum_fb) = render_page(
            &reading,
            Some(cat),
            page,
            &RenderCache::new(),
            Some(&fb),
            &opts,
        )
        .unwrap();
        assert_eq!(sum_fb.substituted_glyphs, 2);
        assert_ne!(pix_builtin, pix_fb);
    }
    #[cfg(not(feature = "programs"))]
    let _ = ttf_bytes;
}

// ---------------------------------------------------------------------------
// 3. Tile Stitching, Rotations, PixelFormats, Transparent Backdrop, & Limits
// ---------------------------------------------------------------------------

#[test]
fn tile_viewport_stitches_identically_to_full_page_and_pixel_formats_roundtrip() {
    let mut canvas = Canvas::new();
    canvas.fill_color(Color::rgb8([30, 120, 210]));
    canvas.fill_rect(Rect::from_xywh(10.0, 15.0, 55.0, 40.0));
    canvas.fill_color(Color::rgb8([240, 160, 20]));
    canvas.fill_rect(Rect::from_xywh(45.0, 35.0, 45.0, 50.0));
    let (content_bytes, _) = canvas.finish(&Reserved::default());

    let pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(100)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Contents" => Object::reference(4),
            }),
        ),
        (4, Object::from(Stream::new(dict! {}, content_bytes))),
    ]);

    for rot in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
        let mut full_opts = RenderOpts::with_viewport(Viewport::Page {
            width: 80,
            rotation: rot,
        });
        full_opts.anti_alias = AntiAlias::NONE;
        let (full, _, _) = render_pdf_bytes(&pdf, &full_opts);
        assert_eq!((full.width(), full.height()), (80, 80));

        // Render as four 40×40 tiles and verify pixel-for-pixel agreement with `full`!
        for (tx, ty) in [(0, 0), (40, 0), (0, 40), (40, 40)] {
            let mut tile_opts = RenderOpts::with_viewport(Viewport::Tile {
                page_width: 80,
                page_height: 80,
                x: tx,
                y: ty,
                w: 40,
                h: 40,
                rotation: rot,
            });
            tile_opts.anti_alias = AntiAlias::NONE;
            let (tile, _, _) = render_pdf_bytes(&pdf, &tile_opts);
            for y in 0..40 {
                for x in 0..40 {
                    assert_eq!(
                        tile.pixel_rgba8(x, y),
                        full.pixel_rgba8(tx + x, ty + y),
                        "tile ({tx},{ty}) mismatch at ({x},{y}) under {rot:?}"
                    );
                }
            }
        }
    }

    // Check transparent backdrop (`background = None`) + `Bgra8` / `Rgba8Premul`
    let mut trans_opts = RenderOpts::with_viewport(Viewport::Page {
        width: 80,
        rotation: Rotation::R0,
    });
    trans_opts.background = None;
    trans_opts.format = PixelFormat::Bgra8;
    let (bgra, _, _) = render_pdf_bytes(&pdf, &trans_opts);
    // Untouched corner `(2, 2)` is transparent `[0, 0, 0, 0]`:
    assert_eq!(bgra.pixel_rgba8(2, 2), Some([0, 0, 0, 0]));
    // Painted blue region `(20, 60)`: raw BGRA bytes have `B = 210` first!
    let raw_idx = (60 * 80 + 20) * 4;
    assert_eq!(&bgra.as_bytes()[raw_idx..raw_idx + 4], &[210, 120, 30, 255]);
    assert_eq!(bgra.pixel_rgba8(20, 60), Some([30, 120, 210, 255]));

    // Check `Limits` upfront refusals without allocation:
    let mut capped = RenderOpts::with_viewport(Viewport::Page {
        width: 500,
        rotation: Rotation::R0,
    });
    capped.limits.max_width = 256;
    let geom = PageGeom::new(Rect::LETTER, None, Rotation::R0);
    assert!(matches!(
        capped.viewport.resolve(&geom, &capped.limits),
        Err(Error::Limit(LimitKind::Dimensions))
    ));
}

// ---------------------------------------------------------------------------
// 4. OCG (`/OCProperties`), Annotations, & AcroForm Widget Synthesis
// ---------------------------------------------------------------------------

#[test]
fn ocg_visibility_annotations_and_acroform_widgets_respect_draw_and_appearances() {
    // Base content draws a blue bar inside `/OC /LayerOff BDC ... EMC` (hidden
    // on screen, visible on print via `/Usage /Print << /PrintState /ON >>`),
    // plus a synthesized `/Square` annotation and a synthesized `/Tx` form widget!
    let content = b"\
        /OC /L1 BDC \
        0 0 1 rg 10 10 80 20 re f \
        EMC\n";

    let sq = BuiltAnnot::shape(
        ShapeKind::Square,
        Rect::from_xywh(10.0, 40.0, 30.0, 20.0),
        &pdf_raster::pdf_annots::PaintStyle::new(
            Some(AnnotColor::Rgb([0.0, 0.8, 0.0])),
            Some(AnnotColor::Rgb([0.0, 0.8, 0.0])),
        ),
    );
    // Strip `/AP` from `sq.dict` so we specifically test `Appearances::Synthesize` vs `WrittenOnly`!
    let mut sq_dict = sq.dict;
    sq_dict.remove(b"AP");
    sq_dict.set("F", AnnotFlags::PRINT.bits() as i64);

    let widget_dict = dict! {
        "Type" => Object::name("Annot"),
        "Subtype" => Object::name("Widget"),
        "FT" => Object::name("Btn"),
        "T" => Object::text("chk"),
        "V" => Object::name("Yes"),
        "AS" => Object::name("Yes"),
        "DA" => Object::text("/ZaDb 0 Tf 0.8 0 0 rg"),
        "MK" => dict! {
            "BG" => vec![Object::from(1.0), Object::from(1.0), Object::from(0.8)],
            "CA" => Object::text("4"),
        },
        "F" => AnnotFlags::PRINT.bits() as i64,
        "Rect" => vec![Object::from(55), Object::from(40), Object::from(85), Object::from(70)],
    };

    let pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(dict! {
                "Type" => Object::name("Catalog"),
                "Pages" => Object::reference(2),
                "OCProperties" => dict! {
                    "OCGs" => vec![Object::reference(5)],
                    "D" => dict! {
                        "BaseState" => Object::name("ON"),
                        "OFF" => vec![Object::reference(5)],
                    },
                },
                "AcroForm" => dict! {
                    "Fields" => vec![Object::reference(7)],
                },
            }),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(100)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Resources" => dict! {
                    "Properties" => dict! { "L1" => Object::reference(5) },
                },
                "Contents" => Object::reference(4),
                "Annots" => vec![Object::reference(6), Object::reference(7)],
            }),
        ),
        (4, Object::from(Stream::new(dict! {}, content.to_vec()))),
        (
            5,
            Object::from(dict! {
                "Type" => Object::name("OCG"),
                "Name" => Object::text("PrintOnlyBanner"),
                "Usage" => dict! {
                    "Print" => dict! { "PrintState" => Object::name("ON") },
                },
            }),
        ),
        (6, Object::from(sq_dict)),
        (7, Object::from(widget_dict)),
    ]);

    // 1. Default `Display` + `Draw::ALL` + `Appearances::Synthesize`:
    //    - OCG `L1` is OFF on screen (`(50, 80)` stays white),
    //    - `/Square` annotation (`(25, 50)`) is synthesized green,
    //    - CheckBox widget (`(70, 45)`) is synthesized!
    let opts_disp = RenderOpts::with_viewport(Viewport::Page {
        width: 100,
        rotation: Rotation::R0,
    });
    let (pix_disp, sum_disp, _) = render_pdf_bytes(&pdf, &opts_disp);
    assert_eq!(pix_disp.pixel_rgba8(50, 80), Some([255, 255, 255, 255]));
    assert_eq!(pix_disp.pixel_rgba8(25, 50), Some([0, 204, 0, 255]));
    assert_eq!(sum_disp.annots_drawn, 1);
    #[cfg(feature = "text-layout")]
    assert_eq!(sum_disp.forms_drawn, 1);

    // 2. `Appearances::WrittenOnly`: neither un-synthesized annot nor widget draws!
    let mut opts_written = opts_disp.clone();
    opts_written.appearances = Appearances::WrittenOnly;
    let (pix_wo, sum_wo, _) = render_pdf_bytes(&pdf, &opts_written);
    assert_eq!(sum_wo.annots_drawn, 0);
    assert_eq!(sum_wo.forms_drawn, 0);
    assert_eq!(pix_wo.pixel_rgba8(25, 50), Some([255, 255, 255, 255]));

    // 3. `RenderMedium::Print` + `Draw::CONTENT_ONLY`: OCG `/PrintState /ON`
    //    turns the blue banner ON, while annotations/forms are suppressed!
    let mut opts_print = opts_disp.clone();
    opts_print.medium = RenderMedium::Print;
    opts_print.draw = Draw::CONTENT_ONLY;
    let (pix_pr, sum_pr, _) = render_pdf_bytes(&pdf, &opts_print);
    assert_eq!(pix_pr.pixel_rgba8(50, 80), Some([0, 0, 255, 255]));
    assert_eq!(pix_pr.pixel_rgba8(25, 50), Some([255, 255, 255, 255]));
    assert_eq!(sum_pr.annots_drawn, 0);
}

#[test]
fn nested_pattern_and_type3_cycles_and_exponential_dags_are_bounded_and_cached() {
    // 1. Self-recursive tiling pattern (`P1` paints a rect using `/P1` itself)
    //    must refuse the cycle cleanly and emit `Fault::InvalidPattern`.
    {
        let page_content = b"/Pattern cs /P1 scn 0 0 64 64 re f\n";
        let pat_content = b"1 0 0 rg 0 0 8 8 re f /Pattern cs /P1 scn 8 0 8 8 re f\n";
        let pdf = build_pdf_bytes(vec![
            (
                1,
                Object::from(
                    dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
                ),
            ),
            (
                2,
                Object::from(dict! {
                    "Type" => Object::name("Pages"),
                    "Kids" => vec![Object::reference(3)],
                    "Count" => 1,
                    "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(64), Object::from(64)],
                }),
            ),
            (
                3,
                Object::from(dict! {
                    "Type" => Object::name("Page"),
                    "Parent" => Object::reference(2),
                    "Resources" => dict! {
                        "Pattern" => dict! { "P1" => Object::reference(5) },
                    },
                    "Contents" => Object::reference(4),
                }),
            ),
            (
                4,
                Object::from(Stream::new(dict! {}, page_content.to_vec())),
            ),
            (
                5,
                Object::from(Stream::new(
                    dict! {
                        "Type" => Object::name("Pattern"),
                        "PatternType" => 1,
                        "PaintType" => 1,
                        "TilingType" => 1,
                        "BBox" => vec![Object::from(0), Object::from(0), Object::from(16), Object::from(16)],
                        "XStep" => 16,
                        "YStep" => 16,
                        "Resources" => dict! {
                            "Pattern" => dict! { "P1" => Object::reference(5) },
                        },
                    },
                    pat_content.to_vec(),
                )),
            ),
        ]);
        let opts = RenderOpts::with_viewport(Viewport::Page {
            width: 64,
            rotation: Rotation::R0,
        });
        let (pix, _sum, faults) = render_pdf_bytes(&pdf, &opts);
        assert!(
            faults.contains(&pdf_raster::Fault::InvalidPattern),
            "expected cycle fault on self-recursive tiling pattern: {faults:?}"
        );
        // The non-recursive left half of the pattern cell (`x = 2, y = 58`) still painted red!
        assert_eq!(pix.pixel_rgba8(2, 58), Some([255, 0, 0, 255]));
    }

    // 2. Self-recursive Type 3 `CharProc` (`'A'` invokes `BT /F3 1 Tf (AA) Tj ET`)
    //    must refuse the cycle immediately without recursing to `max_layer_depth`.
    {
        let page_content = b"0 0 1 rg BT /F3 32 Tf 8 8 Td (A) Tj ET\n";
        let cp_a = b"1 0 d0 0 0.6 0 rg 0 0 0.5 0.5 re f BT /F3 1 Tf (AA) Tj ET";
        let pdf = build_pdf_bytes(vec![
            (
                1,
                Object::from(
                    dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
                ),
            ),
            (
                2,
                Object::from(dict! {
                    "Type" => Object::name("Pages"),
                    "Kids" => vec![Object::reference(3)],
                    "Count" => 1,
                    "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(64), Object::from(64)],
                }),
            ),
            (
                3,
                Object::from(dict! {
                    "Type" => Object::name("Page"),
                    "Parent" => Object::reference(2),
                    "Resources" => dict! {
                        "Font" => dict! { "F3" => Object::reference(5) },
                    },
                    "Contents" => Object::reference(4),
                }),
            ),
            (
                4,
                Object::from(Stream::new(dict! {}, page_content.to_vec())),
            ),
            (
                5,
                Object::from(dict! {
                    "Type" => Object::name("Font"),
                    "Subtype" => Object::name("Type3"),
                    "FontBBox" => vec![Object::from(0), Object::from(0), Object::from(1), Object::from(1)],
                    "FontMatrix" => vec![
                        Object::from(1), Object::from(0), Object::from(0),
                        Object::from(1), Object::from(0), Object::from(0),
                    ],
                    "CharProcs" => dict! { "a" => Object::reference(6) },
                    "Encoding" => dict! {
                        "Type" => Object::name("Encoding"),
                        "Differences" => vec![Object::from(65), Object::name("a")],
                    },
                    "FirstChar" => 65,
                    "LastChar" => 65,
                    "Widths" => vec![Object::from(1)],
                    "Resources" => dict! {
                        "Font" => dict! { "F3" => Object::reference(5) },
                    },
                }),
            ),
            (6, Object::from(Stream::new(dict! {}, cp_a.to_vec()))),
        ]);
        let opts = RenderOpts::with_viewport(Viewport::Page {
            width: 64,
            rotation: Rotation::R0,
        });
        let (pix, _sum, _faults) = render_pdf_bytes(&pdf, &opts);
        assert_eq!(pix.pixel_rgba8(12, 52), Some([0, 153, 0, 255]));
    }

    // 3. 20-level `4^20` (`~1.1e12` leaf uses) acyclic tiling-pattern DAG
    //    finishes instantaneously via `tile_cache`!
    {
        let mut objects = vec![
            (
                1,
                Object::from(
                    dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
                ),
            ),
            (
                2,
                Object::from(dict! {
                    "Type" => Object::name("Pages"),
                    "Kids" => vec![Object::reference(3)],
                    "Count" => 1,
                    "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(64), Object::from(64)],
                }),
            ),
            (
                3,
                Object::from(dict! {
                    "Type" => Object::name("Page"),
                    "Parent" => Object::reference(2),
                    "Resources" => dict! {
                        "Pattern" => dict! { "P0" => Object::reference(10) },
                    },
                    "Contents" => Object::reference(4),
                }),
            ),
            (
                4,
                Object::from(Stream::new(
                    dict! {},
                    b"/Pattern cs /P0 scn 0 0 64 64 re f\n".to_vec(),
                )),
            ),
        ];
        for level in 0..20_u32 {
            let id = 10 + level;
            let (body, res) = if level == 19 {
                (b"0 0.4 0.8 rg 0 0 16 16 re f\n".to_vec(), dict! {})
            } else {
                let next_id = id + 1;
                let s = b"/Pattern cs /Pn scn \
                    0 0 8 8 re f \
                    8 0 8 8 re f \
                    0 8 8 8 re f \
                    8 8 8 8 re f\n"
                    .to_vec();
                (
                    s,
                    dict! { "Pattern" => dict! { "Pn" => Object::reference(next_id) } },
                )
            };
            objects.push((
                id,
                Object::from(Stream::new(
                    dict! {
                        "Type" => Object::name("Pattern"),
                        "PatternType" => 1,
                        "PaintType" => 1,
                        "TilingType" => 1,
                        "BBox" => vec![Object::from(0), Object::from(0), Object::from(16), Object::from(16)],
                        "XStep" => 16,
                        "YStep" => 16,
                        "Resources" => res,
                    },
                    body,
                )),
            ));
        }
        let pdf = build_pdf_bytes(objects);
        let opts = RenderOpts::with_viewport(Viewport::Page {
            width: 64,
            rotation: Rotation::R0,
        });
        let (pix, sum, _faults) = render_pdf_bytes(&pdf, &opts);
        assert_eq!(pix.pixel_rgba8(32, 32), Some([0, 102, 204, 255]));
        // 1 page fill + 19 levels * 4 fills = 77 pattern draws, rendered in O(20) via tile_cache!
        assert_eq!(sum.patterns_drawn, 77);
    }

    // 4. 20-level `4^20` acyclic Type 3 `CharProc` fan-out (`'A'` draws `(BBBB)`,
    //    `'B'` draws `(CCCC)`, ..., `'T'` draws a rect) must hit the shared page-wide
    //    `gfx.operators` budget immediately instead of hanging!
    {
        let mut char_procs = pdf_raster::pdf_objects::Dict::new();
        let mut diffs: Vec<Object> = vec![Object::from(65)];
        let mut widths: Vec<Object> = Vec::new();
        let mut extra_streams = Vec::new();
        for i in 0..20_u32 {
            let name = format!("g{i}");
            let stream_id = 20 + i;
            char_procs.set(name.clone(), Object::reference(stream_id));
            diffs.push(Object::name(name));
            widths.push(Object::from(1));
            let body = if i == 19 {
                b"1 0 d0 1 0 0 rg 0 0 1 1 re f".to_vec()
            } else {
                let next_ch = char::from(b'A' + (i as u8) + 1);
                format!("1 0 d0 BT /F3 1 Tf ({next_ch}{next_ch}{next_ch}{next_ch}) Tj ET")
                    .into_bytes()
            };
            extra_streams.push((stream_id, Object::from(Stream::new(dict! {}, body))));
        }
        let mut objs = vec![
            (
                1,
                Object::from(
                    dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
                ),
            ),
            (
                2,
                Object::from(dict! {
                    "Type" => Object::name("Pages"),
                    "Kids" => vec![Object::reference(3)],
                    "Count" => 1,
                    "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(64), Object::from(64)],
                }),
            ),
            (
                3,
                Object::from(dict! {
                    "Type" => Object::name("Page"),
                    "Parent" => Object::reference(2),
                    "Resources" => dict! {
                        "Font" => dict! { "F3" => Object::reference(5) },
                    },
                    "Contents" => Object::reference(4),
                }),
            ),
            (
                4,
                Object::from(Stream::new(
                    dict! {},
                    b"BT /F3 16 Tf 4 4 Td (A) Tj ET\n".to_vec(),
                )),
            ),
            (
                5,
                Object::from(dict! {
                    "Type" => Object::name("Font"),
                    "Subtype" => Object::name("Type3"),
                    "FontBBox" => vec![Object::from(0), Object::from(0), Object::from(1), Object::from(1)],
                    "FontMatrix" => vec![
                        Object::from(1), Object::from(0), Object::from(0),
                        Object::from(1), Object::from(0), Object::from(0),
                    ],
                    "CharProcs" => char_procs,
                    "Encoding" => dict! {
                        "Type" => Object::name("Encoding"),
                        "Differences" => diffs,
                    },
                    "FirstChar" => 65,
                    "LastChar" => 84,
                    "Widths" => widths,
                    "Resources" => dict! {
                        "Font" => dict! { "F3" => Object::reference(5) },
                    },
                }),
            ),
        ];
        objs.extend(extra_streams);
        let pdf = build_pdf_bytes(objs);

        let reading = Reading::open(&pdf, &OpenOptions::DEFAULT).unwrap();
        let catalog_id = reading
            .trailer()
            .dict()
            .get(b"Root")
            .unwrap()
            .as_reference()
            .unwrap();
        let catalog = (&reading).resolve(catalog_id).unwrap().as_dict().unwrap();
        let tbl = pages(
            &reading,
            catalog,
            Rules::Pdfium,
            &SemLimits::DEFAULT,
            &mut (),
        )
        .unwrap();
        let page = tbl.get(0).unwrap();

        let mut opts = RenderOpts::with_viewport(Viewport::Page {
            width: 64,
            rotation: Rotation::R0,
        });
        opts.limits.gfx.operators = 500;
        let err = render_page(
            &reading,
            Some(catalog),
            page,
            &RenderCache::new(),
            None,
            &opts,
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                Error::Gfx(pdf_raster::pdf_gfx::Error::Limit(
                    pdf_raster::pdf_gfx::Cap::Operators
                ))
            ),
            "expected shared Cap::Operators on Type 3 DAG fan-out, got: {err:?}"
        );
    }
}

#[test]
fn bgra8_transparent_semi_opaque_pixels_and_large_shading_downsampling() {
    // 1. Semi-opaque fill (`0.5 ca`, red `[200, 0, 0]`) over `background: None` in `PixelFormat::Bgra8`:
    //    must be unpremultiplied ONCE (`[200, 0, 0, 128]`), never twice (`[255, 0, 0, 128]`)!
    let content = b"/GS1 gs 0.7843137 0 0 rg 10 10 20 20 re f\n";
    let pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(40), Object::from(40)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Resources" => dict! {
                    "ExtGState" => dict! { "GS1" => dict! { "ca" => 0.5019608 } },
                },
                "Contents" => Object::reference(4),
            }),
        ),
        (4, Object::from(Stream::new(dict! {}, content.to_vec()))),
    ]);

    let mut bgra_opts = RenderOpts::with_viewport(Viewport::Page {
        width: 40,
        rotation: Rotation::R0,
    });
    bgra_opts.background = None;
    bgra_opts.format = PixelFormat::Bgra8;
    let (bgra, _, _) = render_pdf_bytes(&pdf, &bgra_opts);
    let px = bgra.pixel_rgba8(20, 20).unwrap();
    assert!(
        (i16::from(px[0]) - 200).abs() <= 2 && px[1] == 0 && px[2] == 0 && px[3] == 128,
        "expected straight [~200, 0, 0, 128], got {px:?}"
    );
    let vec_rgba = bgra.to_rgba8_vec();
    let idx = (20 * 40 + 20) * 4;
    assert_eq!(&vec_rgba[idx..idx + 4], &px);

    // 2. Full-page axial shading rendered at `1200 × 1000 = 1,200,000` pixels (> default
    //    `max_shading_samples = 1,048,576`): must succeed with `summary.approximated == true`
    //    rather than failing the page with `Error::Limit(ShadingSamples)`!
    let sh_pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(120), Object::from(100)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Resources" => dict! {
                    "Shading" => dict! { "Sh1" => Object::reference(5) },
                },
                "Contents" => Object::reference(4),
            }),
        ),
        (
            4,
            Object::from(Stream::new(dict! {}, b"/Sh1 sh\n".to_vec())),
        ),
        (
            5,
            Object::from(dict! {
                "ShadingType" => 2,
                "ColorSpace" => Object::name("DeviceRGB"),
                "Coords" => vec![Object::from(0), Object::from(0), Object::from(120), Object::from(0)],
                "Extend" => vec![Object::from(true), Object::from(true)],
                "Function" => dict! {
                    "FunctionType" => 2,
                    "Domain" => vec![Object::from(0), Object::from(1)],
                    "C0" => vec![Object::from(1), Object::from(0), Object::from(0)],
                    "C1" => vec![Object::from(0), Object::from(0), Object::from(1)],
                    "N" => 1,
                },
            }),
        ),
    ]);
    let large_opts = RenderOpts::with_viewport(Viewport::Page {
        width: 1200,
        rotation: Rotation::R0,
    });
    let (sh_pix, sh_sum, _) = render_pdf_bytes(&sh_pdf, &large_opts);
    assert_eq!((sh_pix.width(), sh_pix.height()), (1200, 1000));
    assert!(sh_sum.approximated);
    let left_px = sh_pix.pixel_rgba8(10, 500).unwrap();
    let right_px = sh_pix.pixel_rgba8(1190, 500).unwrap();
    assert!(left_px[0] > 240 && left_px[2] < 15, "{left_px:?}");
    assert!(right_px[2] > 240 && right_px[0] < 15, "{right_px:?}");
}

#[test]
fn nested_form_pattern_matrix_and_soft_mask_ctm_backdrop_and_transfer_cache() {
    // 1. Pattern inside `Fm2` inside `Fm1` where `Fm1` translates by `(+13, +0)` and `Fm2`
    //    is identity: pattern has `XStep = 20`, red in `x = 0..10`, blue in `x = 10..20`.
    //    In PageSpace, `Fm2`'s origin is at `x = 13`:
    //    - Correct single application of `Fm1` shift (`+13`): at PageSpace `x = 15` (`pat_x = 2`),
    //      pixel MUST be RED `[255, 0, 0, 255]`!
    //    - Buggy double application (`+26`): at PageSpace `x = 15`, `15 - 26 = -11 -> +9`? Wait:
    //      at PageSpace `x = 18`: single shift `+13` gives `pat_x = 5` (RED `[255, 0, 0]`),
    //      whereas double shift `+26` gives `18 - 26 = -8 == 12 mod 20` (BLUE `[0, 0, 255]`)!
    let pat_body = b"1 0 0 rg 0 0 10 20 re f 0 0 1 rg 10 0 10 20 re f\n";
    let fm2_body = b"/Pattern cs /P1 scn 0 0 40 40 re f\n";
    let fm1_body = b"/Fm2 Do\n";
    let page_body = b"/Fm1 Do\n";
    let nested_pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(60), Object::from(40)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Resources" => dict! {
                    "XObject" => dict! { "Fm1" => Object::reference(5) },
                },
                "Contents" => Object::reference(4),
            }),
        ),
        (4, Object::from(Stream::new(dict! {}, page_body.to_vec()))),
        (
            5,
            Object::from(Stream::new(
                dict! {
                    "Subtype" => Object::name("Form"),
                    "BBox" => vec![Object::from(0), Object::from(0), Object::from(50), Object::from(40)],
                    "Matrix" => vec![
                        Object::from(1), Object::from(0), Object::from(0),
                        Object::from(1), Object::from(13), Object::from(0),
                    ],
                    "Resources" => dict! {
                        "XObject" => dict! { "Fm2" => Object::reference(6) },
                    },
                },
                fm1_body.to_vec(),
            )),
        ),
        (
            6,
            Object::from(Stream::new(
                dict! {
                    "Subtype" => Object::name("Form"),
                    "BBox" => vec![Object::from(0), Object::from(0), Object::from(40), Object::from(40)],
                    "Resources" => dict! {
                        "Pattern" => dict! { "P1" => Object::reference(7) },
                    },
                },
                fm2_body.to_vec(),
            )),
        ),
        (
            7,
            Object::from(Stream::new(
                dict! {
                    "Type" => Object::name("Pattern"),
                    "PatternType" => 1,
                    "PaintType" => 1,
                    "TilingType" => 1,
                    "BBox" => vec![Object::from(0), Object::from(0), Object::from(20), Object::from(20)],
                    "XStep" => 20,
                    "YStep" => 20,
                },
                pat_body.to_vec(),
            )),
        ),
    ]);
    let mut nested_opts = RenderOpts::with_viewport(Viewport::Page {
        width: 60,
        rotation: Rotation::R0,
    });
    nested_opts.anti_alias = AntiAlias::NONE;
    let (npix, _, _) = render_pdf_bytes(&nested_pdf, &nested_opts);
    assert_eq!(
        npix.pixel_rgba8(18, 20),
        Some([255, 0, 0, 255]),
        "nested Form pattern applied outer matrix twice!"
    );

    // 2. Soft-mask CTM at `gs` (`q 0.5 0 0 0.5 40 0 cm /GS1 gs`) + distinct `/BC` on the same `/G`:
    //    `/G` paints white (`1 g`) in `[0, 0, 40, 40]` over a `100 × 40` page.
    //    - Under `q 0.5 0 0 0.5 40 0 cm /GSBlack gs` (`/BC [0]`): the white square lands at
    //      PageSpace `[40..60] × [0..20]` (`y_px = 20..40`). Painting solid red (`1 0 0 rg`)
    //      over the whole page turns `(50, 30)` red while `(10, 30)` stays white!
    //    - Next, under identity CTM, `/GSWhite gs` references the SAME `/G` stream with `/BC [1]`
    //      (white backdrop = luminosity 1 everywhere): painting solid blue (`0 0 1 rg`) at
    //      `[70, 0, 20, 40]` (outside `/G`'s `[0..40]` box!) MUST paint blue because `/BC [1]`
    //      rebuilt the mask instead of reusing `/GSBlack`'s cached mask!
    let smask_g = b"1 g 0 0 40 40 re f\n";
    let sm_page = b"\
        q 0.5 0 0 0.5 40 0 cm /GSBlack gs 1 0 0 rg -80 0 200 80 re f Q \
        q /GSWhite gs 0 0 1 rg 70 0 20 40 re f Q\n";
    let sm_pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(40)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Resources" => dict! {
                    "ExtGState" => dict! {
                        "GSBlack" => dict! {
                            "SMask" => dict! {
                                "S" => Object::name("Luminosity"),
                                "G" => Object::reference(5),
                                "BC" => vec![Object::from(0.0)],
                            },
                        },
                        "GSWhite" => dict! {
                            "SMask" => dict! {
                                "S" => Object::name("Luminosity"),
                                "G" => Object::reference(5),
                                "BC" => vec![Object::from(1.0)],
                            },
                        },
                    },
                },
                "Contents" => Object::reference(4),
            }),
        ),
        (4, Object::from(Stream::new(dict! {}, sm_page.to_vec()))),
        (
            5,
            Object::from(Stream::new(
                dict! {
                    "Subtype" => Object::name("Form"),
                    "BBox" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(40)],
                    "Group" => dict! {
                        "S" => Object::name("Transparency"),
                        "CS" => Object::name("DeviceGray"),
                    },
                },
                smask_g.to_vec(),
            )),
        ),
    ]);
    let mut sm_opts = RenderOpts::with_viewport(Viewport::Page {
        width: 100,
        rotation: Rotation::R0,
    });
    sm_opts.anti_alias = AntiAlias::NONE;
    let (smpix, _, _) = render_pdf_bytes(&sm_pdf, &sm_opts);
    // `(10, 30)` was NOT covered when `GSBlack` was shifted to `x = 40..60`, so it stays white!
    assert_eq!(smpix.pixel_rgba8(10, 30), Some([255, 255, 255, 255]));
    // `(50, 30)` WAS inside `GSBlack`'s shifted `[40..60] × [0..20]` box, so it is red!
    assert_eq!(smpix.pixel_rgba8(50, 30), Some([255, 0, 0, 255]));
    // `(80, 20)` is outside `/G`'s `0..40` rect, where `GSWhite` has `/BC [1.0]`, so it is blue!
    assert_eq!(smpix.pixel_rgba8(80, 20), Some([0, 0, 255, 255]));
}

#[test]
fn type4_and_type5_gouraud_meshes_byte_alignment_parametric_interp_and_caps() {
    fn pack_vertex_records(records: &[&[(u32, u8)]]) -> Vec<u8> {
        let mut out = Vec::new();
        for rec in records {
            let mut acc = 0_u64;
            let mut nb = 0_u8;
            for &(val, width) in *rec {
                acc = (acc << width) | u64::from(val & ((1_u32 << width) - 1));
                nb += width;
                while nb >= 8 {
                    nb -= 8;
                    out.push(((acc >> nb) & 0xFF) as u8);
                }
            }
            if nb > 0 {
                out.push(((acc << (8 - nb)) & 0xFF) as u8);
            }
        }
        out
    }

    // 1. Type 4 free-form Gouraud mesh with `BitsPerFlag = 2`, `BitsPerCoordinate = 12` (`0..4095`),
    //    `BitsPerComponent = 8` (`34` bits/vertex -> padded to 5 bytes/vertex!) and a PostScript
    //    step `/Function { 0.5 lt { 1 0 0 } { 0 1 0 } ifelse }`:
    //    Triangle `(0,0, t=0)` - `(40,0, t=1)` - `(0,40, t=0)` + edge-flag=1 `(40,40, t=1)`.
    //    At `(10, 30)` (`x=10, y=10` in PDF coords -> `t = 0.25 < 0.5`): MUST be pure RED `[255, 0, 0]`
    //    (interpolating sRGB at vertices would have blended red & green into `[191, 64, 0]`!).
    //    At `(35, 10)` (in second triangle via `flag = 1`, `x=35, y=30` -> `t = 0.875 > 0.5`):
    //    MUST be pure GREEN `[0, 255, 0]`!
    let t4_bytes = pack_vertex_records(&[
        &[(0, 2), (0, 12), (0, 12), (0, 8)],
        &[(0, 2), (4095, 12), (0, 12), (255, 8)],
        &[(0, 2), (0, 12), (4095, 12), (0, 8)],
        &[(1, 2), (4095, 12), (4095, 12), (255, 8)],
    ]);
    let ps_step = b"{ 0.5 lt { 1 0 0 } { 0 1 0 } ifelse }";

    // 2. Type 5 `2x2` lattice mesh with `BitsPerCoordinate = 12`, `BitsPerComponent = 12`
    //    (`60` bits/vertex -> padded to 8 bytes/vertex!) in `[40..80] × [0..40]` pure BLUE `[0, 0, 1]`.
    let t5_bytes = pack_vertex_records(&[
        &[(0, 12), (0, 12), (0, 12), (0, 12), (4095, 12)],
        &[(4095, 12), (0, 12), (0, 12), (0, 12), (4095, 12)],
        &[(0, 12), (4095, 12), (0, 12), (0, 12), (4095, 12)],
        &[(4095, 12), (4095, 12), (0, 12), (0, 12), (4095, 12)],
    ]);

    let mesh_pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(80), Object::from(40)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Resources" => dict! {
                    "Shading" => dict! {
                        "Sh4" => Object::reference(5),
                        "Sh5" => Object::reference(7),
                    },
                },
                "Contents" => Object::reference(4),
            }),
        ),
        (
            4,
            Object::from(Stream::new(dict! {}, b"/Sh4 sh /Sh5 sh\n".to_vec())),
        ),
        (
            5,
            Object::from(Stream::new(
                dict! {
                    "ShadingType" => 4,
                    "ColorSpace" => Object::name("DeviceRGB"),
                    "BitsPerFlag" => 2,
                    "BitsPerCoordinate" => 12,
                    "BitsPerComponent" => 8,
                    "Decode" => vec![
                        Object::from(0), Object::from(40),
                        Object::from(0), Object::from(40),
                        Object::from(0), Object::from(1),
                    ],
                    "Function" => Object::reference(6),
                },
                t4_bytes,
            )),
        ),
        (
            6,
            Object::from(Stream::new(
                dict! {
                    "FunctionType" => 4,
                    "Domain" => vec![Object::from(0), Object::from(1)],
                    "Range" => vec![
                        Object::from(0), Object::from(1),
                        Object::from(0), Object::from(1),
                        Object::from(0), Object::from(1),
                    ],
                },
                ps_step.to_vec(),
            )),
        ),
        (
            7,
            Object::from(Stream::new(
                dict! {
                    "ShadingType" => 5,
                    "ColorSpace" => Object::name("DeviceRGB"),
                    "VerticesPerRow" => 2,
                    "BitsPerCoordinate" => 12,
                    "BitsPerComponent" => 12,
                    "Decode" => vec![
                        Object::from(40), Object::from(80),
                        Object::from(0), Object::from(40),
                        Object::from(0), Object::from(1),
                        Object::from(0), Object::from(1),
                        Object::from(0), Object::from(1),
                    ],
                },
                t5_bytes,
            )),
        ),
    ]);

    let mesh_opts = RenderOpts::with_viewport(Viewport::Page {
        width: 80,
        rotation: Rotation::R0,
    });
    let (mpix, msum, mfaults) = render_pdf_bytes(&mesh_pdf, &mesh_opts);
    assert!(mfaults.is_empty(), "{mfaults:?}");
    assert!(!msum.approximated);
    assert_eq!(mpix.pixel_rgba8(10, 30), Some([255, 0, 0, 255]));
    assert_eq!(mpix.pixel_rgba8(35, 10), Some([0, 255, 0, 255]));
    assert_eq!(mpix.pixel_rgba8(60, 20), Some([0, 0, 255, 255]));

    // Capping `max_shading_samples = 1` truncates the 2-triangle mesh, setting `approximated`
    // and reporting `Fault::InvalidShading`!
    let mut capped_mesh_opts = mesh_opts;
    capped_mesh_opts.limits.max_shading_samples = 1;
    let (_, csum, cfaults) = render_pdf_bytes(&mesh_pdf, &capped_mesh_opts);
    assert!(csum.approximated);
    assert!(cfaults.contains(&pdf_raster::Fault::InvalidShading));
}

#[test]
fn hidden_oc_applies_clips_and_unwinds_unbalanced_form_save_and_pattern_stencil_draws() {
    // 1. Inside hidden `/OC /LOff BDC ... EMC`, `10 10 20 20 re W n` MUST still clip the
    //    subsequent visible `1 0 0 rg 0 0 60 60 re f` fill (`ISO 32000-2`, §8.11.3.1),
    //    and an unbalanced `q` inside a hidden `/OC` Form XObject (`FmHidden`) MUST be
    //    unwound cleanly at `FormEnd`!
    let hidden_form = b"q 0 0 5 5 re W n 0 1 0 rg 0 0 60 60 re f\n"; // unbalanced `q` inside hidden form!
    let page_content = b"\
        q \
        /FmHidden Do \
        /OC /LOff BDC 0 0 1 rg 0 0 60 60 re f 10 10 20 20 re W n EMC \
        1 0 0 rg 0 0 60 60 re f \
        Q \
        /Pattern cs /PatRB scn \
        q 40 0 0 20 10 35 cm /ImMask Do Q\n";
    let pat_rb = b"1 0 0 rg 0 0 20 20 re f 0 0 1 rg 20 0 20 20 re f\n";

    // Also include a high-numbered object `(1, 65535)` to verify `EphemeralOverlay`
    // collision avoidance never shadows existing objects!
    let sq = BuiltAnnot::shape(
        ShapeKind::Square,
        Rect::from_xywh(42.0, 5.0, 14.0, 14.0),
        &pdf_raster::pdf_annots::PaintStyle::new(
            Some(AnnotColor::Rgb([0.0, 0.8, 0.0])),
            Some(AnnotColor::Rgb([0.0, 0.8, 0.0])),
        ),
    );
    let mut sq_dict = sq.dict;
    sq_dict.remove(b"AP");
    sq_dict.set("F", AnnotFlags::PRINT.bits() as i64);

    let pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(dict! {
                "Type" => Object::name("Catalog"),
                "Pages" => Object::reference(2),
                "OCProperties" => dict! {
                    "OCGs" => vec![Object::reference(5)],
                    "D" => dict! {
                        "BaseState" => Object::name("ON"),
                        "OFF" => vec![Object::reference(5)],
                    },
                },
            }),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(60), Object::from(60)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Resources" => dict! {
                    "Properties" => dict! { "LOff" => Object::reference(5) },
                    "XObject" => dict! {
                        "FmHidden" => Object::reference(6),
                        "ImMask" => Object::reference(7),
                    },
                    "Pattern" => dict! { "PatRB" => Object::reference(8) },
                },
                "Contents" => Object::reference(4),
                "Annots" => vec![Object::reference(9)],
            }),
        ),
        (
            4,
            Object::from(Stream::new(dict! {}, page_content.to_vec())),
        ),
        (
            5,
            Object::from(dict! { "Type" => Object::name("OCG"), "Name" => Object::text("Hidden") }),
        ),
        (
            6,
            Object::from(Stream::new(
                dict! {
                    "Subtype" => Object::name("Form"),
                    "BBox" => vec![Object::from(0), Object::from(0), Object::from(60), Object::from(60)],
                    "OC" => Object::reference(5),
                },
                hidden_form.to_vec(),
            )),
        ),
        (
            7,
            Object::from(Stream::new(
                dict! {
                    "Subtype" => Object::name("Image"),
                    "Width" => 2,
                    "Height" => 1,
                    "ImageMask" => true,
                },
                // Bit 0 = `0` (painted), Bit 1 = `0` (painted): full 2x1 bar painted with `PatRB`!
                vec![0b0000_0000],
            )),
        ),
        (
            8,
            Object::from(Stream::new(
                dict! {
                    "Type" => Object::name("Pattern"),
                    "PatternType" => 1,
                    "PaintType" => 1,
                    "TilingType" => 1,
                    "BBox" => vec![Object::from(0), Object::from(0), Object::from(40), Object::from(20)],
                    "XStep" => 40,
                    "YStep" => 20,
                },
                pat_rb.to_vec(),
            )),
        ),
        (9, Object::from(sq_dict)),
    ]);

    let mut opts = RenderOpts::with_viewport(Viewport::Page {
        width: 60,
        rotation: Rotation::R0,
    });
    opts.anti_alias = AntiAlias::NONE;
    let (pix, sum, _) = render_pdf_bytes(&pdf, &opts);

    // Clip `[10..30] × [10..30]` (`y_px = 30..50`) from hidden `/OC` restricted the red fill:
    // `(20, 40)` is RED, while `(5, 55)` outside `[10..30]` stayed WHITE!
    assert_eq!(pix.pixel_rgba8(20, 40), Some([255, 0, 0, 255]));
    assert_eq!(pix.pixel_rgba8(5, 55), Some([255, 255, 255, 255]));

    // Pattern-filled `/ImageMask true` at `x = 10..50, y = 35..55` (`y_px = 5..25`):
    // `PatRB` has red in `x = 0..20` (`x = 40..50` wraps to `0..10` red) and blue in `x = 20..40`!
    assert_eq!(pix.pixel_rgba8(15, 15), Some([255, 0, 0, 255]));
    assert_eq!(pix.pixel_rgba8(25, 15), Some([0, 0, 255, 255]));
    assert_eq!(sum.images_drawn, 1);
    assert_eq!(sum.annots_drawn, 1);
    assert_eq!(pix.pixel_rgba8(49, 48), Some([0, 204, 0, 255]));
}

#[test]
fn user_unit_dpi_and_embedded_missing_glyph_follow_iso_and_pdfium_rules() {
    // 1. `/UserUnit 2.0` on `100 × 80` page: `Viewport::Dpi(72)` resolves to `100 × 80` under
    //    `Rules::Pdfium` (matching PDFium) and `200 × 160` under `Rules::Iso` (ISO 32000-2 §7.7.3.3).
    let content = b"0.8 0.1 0.1 rg 0 0 100 80 re f\n";
    let uu_pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(80)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "UserUnit" => 2.0,
                "Contents" => Object::reference(4),
            }),
        ),
        (4, Object::from(Stream::new(dict! {}, content.to_vec()))),
    ]);

    let mut dpi_pdfium = RenderOpts::with_viewport(Viewport::Dpi {
        dpi: 72.0,
        rotation: Rotation::R0,
    });
    dpi_pdfium.rules = Rules::Pdfium;
    let (pix_p, _, _) = render_pdf_bytes(&uu_pdf, &dpi_pdfium);
    assert_eq!((pix_p.width(), pix_p.height()), (100, 80));

    let mut dpi_iso = dpi_pdfium.clone();
    dpi_iso.rules = Rules::Iso;
    let (pix_i, _, _) = render_pdf_bytes(&uu_pdf, &dpi_iso);
    assert_eq!((pix_i.width(), pix_i.height()), (200, 160));

    // 2. Embedded TrueType font missing `'Z'`:
    //    - With a non-empty `.notdef` box (`TrueType::new`), `'Z'` draws `.notdef` (`substituted_glyphs == 0`).
    //    - With a blank `.notdef` (`Empty`) or out-of-range CID (`OutlineOutcome::NoGlyph`), nothing
    //      is drawn (`substituted_glyphs == 0` and canvas stays pure white — never a built-in `?` stick glyph!).
    let mut blank_notdef_tt = TrueType::new(1000);
    blank_notdef_tt.glyphs[0] = (FixtureGlyph::Empty, 500);
    let g_a = blank_notdef_tt.glyph(FixtureGlyph::Box(100, 0, 700, 750), 800);
    blank_notdef_tt.map('A', g_a);
    let ttf_bytes = blank_notdef_tt.build();
    let z_stream = b"0 0 0 rg BT /F1 48 Tf 10 20 Td (Z) Tj /F2 48 Tf <0099> Tj ET\n";
    let z_pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(80)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Resources" => dict! {
                    "Font" => dict! {
                        "F1" => Object::reference(5),
                        "F2" => Object::reference(8),
                    },
                },
                "Contents" => Object::reference(4),
            }),
        ),
        (4, Object::from(Stream::new(dict! {}, z_stream.to_vec()))),
        (
            5,
            Object::from(dict! {
                "Type" => Object::name("Font"),
                "Subtype" => Object::name("TrueType"),
                "BaseFont" => Object::name("SubsetTest"),
                "Encoding" => Object::name("WinAnsiEncoding"),
                "FirstChar" => 65,
                "LastChar" => 90,
                "Widths" => vec![Object::from(700); 26],
                "FontDescriptor" => Object::reference(6),
            }),
        ),
        (
            6,
            Object::from(dict! {
                "Type" => Object::name("FontDescriptor"),
                "FontName" => Object::name("SubsetTest"),
                "Flags" => 32,
                "FontBBox" => vec![Object::from(0), Object::from(0), Object::from(1000), Object::from(1000)],
                "Ascent" => 800,
                "Descent" => -200,
                "CapHeight" => 700,
                "StemV" => 80,
                "FontFile2" => Object::reference(7),
            }),
        ),
        (
            7,
            Object::from(Stream::new(
                dict! { "Length1" => ttf_bytes.len() as i64 },
                ttf_bytes,
            )),
        ),
        (
            8,
            Object::from(dict! {
                "Type" => Object::name("Font"),
                "Subtype" => Object::name("Type0"),
                "BaseFont" => Object::name("SubsetCid"),
                "Encoding" => Object::name("Identity-H"),
                "DescendantFonts" => vec![Object::reference(9)],
            }),
        ),
        (
            9,
            Object::from(dict! {
                "Type" => Object::name("Font"),
                "Subtype" => Object::name("CIDFontType2"),
                "BaseFont" => Object::name("SubsetCid"),
                "CIDSystemInfo" => dict! {
                    "Registry" => Object::bytes(&b"Adobe"[..]),
                    "Ordering" => Object::bytes(&b"Identity"[..]),
                    "Supplement" => 0,
                },
                "DW" => 700,
                "FontDescriptor" => Object::reference(6),
                "CIDToGIDMap" => Object::name("Identity"),
            }),
        ),
    ]);
    let z_opts = RenderOpts::with_viewport(Viewport::Page {
        width: 100,
        rotation: Rotation::R0,
    });
    let (z_pix, z_sum, _) = render_pdf_bytes(&z_pdf, &z_opts);
    #[cfg(feature = "programs")]
    {
        assert_eq!(z_sum.substituted_glyphs, 0);
        assert!(
            z_pix.as_bytes().iter().all(|&b| b == 255),
            "embedded font with blank .notdef must not draw built-in stick glyph"
        );
    }
    #[cfg(not(feature = "programs"))]
    {
        assert_eq!(z_sum.substituted_glyphs, 2);
        let _ = z_pix;
    }
}

#[test]
fn renderer_builder_cancellation_png_and_buffer_length_behave_deterministically() {
    use pdf_raster::{Limits, Renderer};
    use std::sync::atomic::{AtomicBool, Ordering};

    let pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(64), Object::from(48)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Contents" => Object::reference(4),
            }),
        ),
        (
            4,
            Object::from(Stream::new(
                dict! {},
                b"0.2 0.6 0.9 rg 0 0 64 48 re f\n".to_vec(),
            )),
        ),
    ]);
    let reading = Reading::open(&pdf[..], &OpenOptions::DEFAULT).unwrap();
    let catalog = (&reading)
        .resolve(
            reading
                .trailer()
                .dict()
                .get(b"Root")
                .unwrap()
                .as_reference()
                .unwrap(),
        )
        .unwrap()
        .as_dict()
        .unwrap();
    let tbl = pages(
        &reading,
        catalog,
        Rules::Pdfium,
        &SemLimits::DEFAULT,
        &mut (),
    )
    .unwrap();
    let page = tbl.get(0).unwrap();
    let cache = RenderCache::new();

    let dpi_opts = RenderOpts::dpi(72.0);
    let (pix_dpi, sum_dpi) = Renderer::new(&dpi_opts)
        .with_cache(&cache)
        .render_page(&reading, Some(catalog), page)
        .unwrap();
    assert_eq!((pix_dpi.width(), pix_dpi.height()), (64, 48));
    assert!(sum_dpi.reported && !sum_dpi.approximated);

    let width_opts = RenderOpts::page_width(32);
    let (pix_w, _) = Renderer::new(&width_opts)
        .with_cache(&cache)
        .render_page(&reading, None, page)
        .unwrap();
    assert_eq!((pix_w.width(), pix_w.height()), (32, 24));

    // Cooperative cancellation via `RenderOpts::with_stop`:
    let stop = Arc::new(AtomicBool::new(true));
    let stopped_opts = RenderOpts::dpi(72.0).with_stop(stop.clone());
    assert!(matches!(
        Renderer::new(&stopped_opts).render_page(&reading, None, page),
        Err(Error::Cancelled)
    ));
    stop.store(false, Ordering::Relaxed);
    assert!(
        Renderer::new(&stopped_opts)
            .render_page(&reading, None, page)
            .is_ok()
    );

    // `Pixmap::from_bytes` reports `Error::BufferLength`:
    assert!(matches!(
        Pixmap::from_bytes(4, 4, PixelFormat::Rgba8, vec![0; 12], &Limits::DEFAULT),
        Err(Error::BufferLength {
            actual: 12,
            expected: 64,
        })
    ));

    // `Pixmap::to_png` + `Pixmap::to_rgba_image` across pixel formats:
    for fmt in [
        PixelFormat::Rgba8,
        PixelFormat::Bgra8,
        PixelFormat::Rgba8Premul,
    ] {
        let mut fopts = RenderOpts::page_width(16);
        fopts.format = fmt;
        let (p, _) = Renderer::new(&fopts)
            .render_page(&reading, None, page)
            .unwrap();
        let png = p.to_png().unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(png.windows(4).any(|w| w == b"IHDR"));
        assert!(png.windows(4).any(|w| w == b"IDAT"));
        assert!(png.ends_with(b"\x00\x00\x00\x00IEND\xaeB`\x82"));
        let rgba = p
            .to_rgba_image(&pdf_raster::pdf_images::Limits::DEFAULT)
            .unwrap();
        assert_eq!((rgba.width(), rgba.height()), (16, 12));
        assert_eq!(rgba.pixel(8, 6), p.pixel_rgba8(8, 6));
    }
}

#[test]
fn mipmap_box_downsampling_preserves_thin_strokes_under_antialias() {
    // 128x128 white RGB image with a 2-px vertical black stripe at `x = 63..65`,
    // downscaled 8x onto a 16x16 viewport. Iterative 2x box mipmapping preserves
    // dark ink (~25% coverage -> gray ~191) at `x = 7` or `x = 8`.
    let mut raw = vec![255_u8; 128 * 128 * 3];
    for y in 0..128 {
        for x in 63..65 {
            let idx = (y * 128 + x) * 3;
            raw[idx..idx + 3].fill(0);
        }
    }
    let pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(128), Object::from(128)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Resources" => dict! { "XObject" => dict! { "Im0" => Object::reference(5) } },
                "Contents" => Object::reference(4),
            }),
        ),
        (
            4,
            Object::from(Stream::new(
                dict! {},
                b"q 128 0 0 128 0 0 cm /Im0 Do Q\n".to_vec(),
            )),
        ),
        (
            5,
            Object::from(Stream::new(
                dict! {
                    "Type" => Object::name("XObject"),
                    "Subtype" => Object::name("Image"),
                    "Width" => 128,
                    "Height" => 128,
                    "ColorSpace" => Object::name("DeviceRGB"),
                    "BitsPerComponent" => 8,
                },
                raw,
            )),
        ),
    ]);
    let opts = RenderOpts::page_width(16);
    let (pix, sum, faults) = render_pdf_bytes(&pdf, &opts);
    assert!(faults.is_empty());
    assert_eq!(sum.images_drawn, 1);
    let min_center_r = (6..=9)
        .map(|x| pix.pixel_rgba8(x, 8).unwrap()[0])
        .min()
        .unwrap();
    assert!(
        min_center_r < 225,
        "2x mipmap downsampling must preserve thin vertical stripe ink on 8x downscale, got min R={min_center_r}"
    );
}

#[test]
fn approximated_summary_faults_font_query_dedup_and_scene_limits() {
    use pdf_raster::{Fault, FontFallback, FontQuery, Renderer};
    use std::sync::Mutex;

    // 1. ShadingType 6 (Coons patch mesh) emits `UnsupportedShading { shading_type: 6 }`,
    //    sets `approximated = true`, and paints `/Background`!
    let sh6_pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(32), Object::from(32)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Resources" => dict! { "Shading" => dict! { "S6" => Object::reference(5) } },
                "Contents" => Object::reference(4),
            }),
        ),
        (4, Object::from(Stream::new(dict! {}, b"/S6 sh\n".to_vec()))),
        (
            5,
            Object::from(Stream::new(
                dict! {
                    "ShadingType" => 6,
                    "ColorSpace" => Object::name("DeviceRGB"),
                    "BitsPerCoordinate" => 8,
                    "BitsPerComponent" => 8,
                    "BitsPerFlag" => 8,
                    "Decode" => vec![
                        Object::from(0), Object::from(32),
                        Object::from(0), Object::from(32),
                        Object::from(0), Object::from(1),
                        Object::from(0), Object::from(1),
                        Object::from(0), Object::from(1),
                    ],
                    "Background" => vec![Object::from(0.8), Object::from(0.1), Object::from(0.2)],
                },
                vec![0; 32],
            )),
        ),
    ]);
    let (sh_pix, sh_sum, sh_faults) = render_pdf_bytes(&sh6_pdf, &RenderOpts::page_width(32));
    assert!(sh_sum.approximated);
    assert!(sh_faults.contains(&Fault::UnsupportedShading { shading_type: 6 }));
    let bg_px = sh_pix.pixel_rgba8(16, 16).unwrap();
    assert!(bg_px[0] > 180 && bg_px[1] < 50);

    // 2. Dense dash + non-isolated transparency group + damaged `/Contents` stream + unembedded Std14:
    let approx_pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(64), Object::from(64)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Group" => dict! { "S" => Object::name("Transparency"), "I" => false },
                "Resources" => dict! {
                    "Font" => dict! {
                        "F1" => dict! {
                            "Type" => Object::name("Font"),
                            "Subtype" => Object::name("Type1"),
                            "BaseFont" => Object::name("ABCDEF+CustomSans"),
                        },
                    },
                },
                "Contents" => vec![Object::reference(4), Object::reference(5)],
            }),
        ),
        (
            4,
            Object::from(Stream::new(
                dict! { "Filter" => Object::name("FlateDecode") },
                b"not-valid-zlib-stream".to_vec(),
            )),
        ),
        (
            5,
            Object::from(Stream::new(
                dict! {},
                b"[0.001 0.001] 0 d 2 w 4 4 m 60 60 l S BT /F1 14 Tf 8 20 Td (A) Tj (B) Tj ET\n"
                    .to_vec(),
            )),
        ),
    ]);
    let (_, ap_sum, ap_faults) = render_pdf_bytes(&approx_pdf, &RenderOpts::page_width(64));
    assert!(ap_sum.approximated);
    assert!(ap_faults.contains(&Fault::SkippedStream), "{ap_faults:?}");
    assert!(
        ap_faults.contains(&Fault::ApproximatedGroup),
        "{ap_faults:?}"
    );
    assert!(ap_faults.contains(&Fault::DenseDash), "{ap_faults:?}");
    assert_eq!(
        ap_faults
            .iter()
            .filter(|f| matches!(f, Fault::SubstitutedFont))
            .count(),
        1,
        "SubstitutedFont should emit once per font: {ap_faults:?}"
    );

    // 3. `FontFallback` receives `base_font`, stripped `family_name`, and `sample_char`:
    type SeenQuery = (Option<String>, Option<String>, Option<char>);
    struct SpyFallback {
        seen: Mutex<Vec<SeenQuery>>,
        ttf: Arc<[u8]>,
    }
    impl FontFallback for SpyFallback {
        fn fallback_font(&self, query: &FontQuery) -> Option<Arc<[u8]>> {
            self.seen.lock().unwrap().push((
                query
                    .base_font()
                    .map(|b| String::from_utf8_lossy(b).into_owned()),
                query
                    .family_name()
                    .map(|b| String::from_utf8_lossy(b).into_owned()),
                query.sample_char(),
            ));
            Some(self.ttf.clone())
        }
    }
    let mut tt = TrueType::new(1000);
    let gid_a = tt.glyph(FixtureGlyph::Box(100, 0, 700, 700), 800);
    tt.map('A', gid_a);
    tt.map('B', gid_a);
    let spy = SpyFallback {
        seen: Mutex::new(Vec::new()),
        ttf: Arc::from(tt.build()),
    };
    let reading = Reading::open(&approx_pdf[..], &OpenOptions::DEFAULT).unwrap();
    let catalog = (&reading)
        .resolve(
            reading
                .trailer()
                .dict()
                .get(b"Root")
                .unwrap()
                .as_reference()
                .unwrap(),
        )
        .unwrap()
        .as_dict()
        .unwrap();
    let tbl = pages(
        &reading,
        catalog,
        Rules::Pdfium,
        &SemLimits::DEFAULT,
        &mut (),
    )
    .unwrap();
    let cache = RenderCache::new();
    let _ = Renderer::new(&RenderOpts::page_width(64))
        .with_cache(&cache)
        .with_fallback(&spy)
        .render_page(&reading, Some(catalog), tbl.get(0).unwrap())
        .unwrap();
    let seen = spy.seen.lock().unwrap();
    assert_eq!(
        seen.len(),
        1,
        "RenderCache memoizes fallback query per provider: {seen:?}"
    );
    assert_eq!(seen[0].0.as_deref(), Some("ABCDEF+CustomSans"));
    assert_eq!(seen[0].1.as_deref(), Some("CustomSans"));
    assert_eq!(seen[0].2, Some('A'));

    // 4. `max_tile_pixels` and `max_scene_bytes` caps:
    let tile_and_img_pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(64), Object::from(64)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "Resources" => dict! { "Pattern" => dict! { "P1" => Object::reference(5) } },
                "Contents" => Object::reference(4),
            }),
        ),
        (
            4,
            Object::from(Stream::new(
                dict! {},
                b"/Pattern cs /P1 scn 0 0 64 64 re f\n".to_vec(),
            )),
        ),
        (
            5,
            Object::from(Stream::new(
                dict! {
                    "Type" => Object::name("Pattern"),
                    "PatternType" => 1,
                    "PaintType" => 1,
                    "TilingType" => 1,
                    "BBox" => vec![Object::from(0), Object::from(0), Object::from(16), Object::from(16)],
                    "XStep" => 16,
                    "YStep" => 16,
                    "Resources" => dict! {},
                },
                b"0.1 0.5 0.2 rg 0 0 8 8 re f\n".to_vec(),
            )),
        ),
    ]);
    let tile_reading = Reading::open(&tile_and_img_pdf[..], &OpenOptions::DEFAULT).unwrap();
    let tile_cat = (&tile_reading)
        .resolve(
            tile_reading
                .trailer()
                .dict()
                .get(b"Root")
                .unwrap()
                .as_reference()
                .unwrap(),
        )
        .unwrap()
        .as_dict()
        .unwrap();
    let tile_tbl = pages(
        &tile_reading,
        tile_cat,
        Rules::Pdfium,
        &SemLimits::DEFAULT,
        &mut (),
    )
    .unwrap();
    let tile_page = tile_tbl.get(0).unwrap();

    let mut small_tile_opts = RenderOpts::page_width(64);
    small_tile_opts.limits.max_tile_pixels = 64;
    assert!(matches!(
        Renderer::new(&small_tile_opts).render_page(&tile_reading, Some(tile_cat), tile_page),
        Err(Error::Limit(LimitKind::PatternCells))
    ));

    let mut small_scene_opts = RenderOpts::page_width(64);
    small_scene_opts.limits.max_scene_bytes = 64;
    assert!(matches!(
        Renderer::new(&small_scene_opts).render_page(&tile_reading, Some(tile_cat), tile_page),
        Err(Error::Limit(LimitKind::SceneBytes))
    ));
}

#[cfg(feature = "pdfium")]
#[test]
#[ignore = "requires libpdfium (run via `just test-pdfium`)"]
fn pdfium_backend_trait_covers_dpi_premul_transparent_bg_crop_rotate_tile_overhang_and_limits() {
    use pdf_raster::{BackendError, PdfiumBackend, RenderBackend};

    let backend = PdfiumBackend::from_env().expect("PDF_RASTER_PDFIUM or PDFIUM_LIB_DIR set");

    // Page with `/CropBox [20 10 120 90]` (`100 × 80` cropped view), `/Rotate 90` (-> `80 × 100`),
    // and a semi-transparent (`0.5 ca`) red square filling the cropped view!
    let content = b"/GS1 gs 0.8 0 0 rg 20 10 100 80 re f\n";
    let pdf = build_pdf_bytes(vec![
        (
            1,
            Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            ),
        ),
        (
            2,
            Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(200), Object::from(200)],
            }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "CropBox" => vec![Object::from(20), Object::from(10), Object::from(120), Object::from(90)],
                "Rotate" => 90,
                "Resources" => dict! {
                    "ExtGState" => dict! { "GS1" => dict! { "ca" => 0.5019608 } },
                },
                "Contents" => Object::reference(4),
            }),
        ),
        (4, Object::from(Stream::new(dict! {}, content.to_vec()))),
    ]);

    let doc: Arc<dyn pdf_raster::RenderedDoc> =
        Arc::from(backend.open_bytes(Arc::from(pdf), None).unwrap());
    assert_eq!(doc.page_count(), 1);
    assert!(matches!(
        doc.render_page(3, &RenderOpts::default()),
        Err(BackendError::PageOutOfBounds {
            page_index: 3,
            page_count: 1
        })
    ));

    // 1. `Viewport::Dpi(72.0)` with `/Rotate 90` + `/CropBox 100×80` -> `80 × 100`!
    //    Test both `Rgba8Premul` and `Bgra8` over `background: None` (single unpremultiplication!).
    let mut dpi_premul = RenderOpts::with_viewport(Viewport::Dpi {
        dpi: 72.0,
        rotation: Rotation::R0,
    });
    dpi_premul.background = None;
    dpi_premul.format = PixelFormat::Rgba8Premul;
    let (p_premul, sum_pdfium) = doc.render_page(0, &dpi_premul).unwrap();
    assert!(!sum_pdfium.reported);
    assert_eq!((p_premul.width(), p_premul.height()), (80, 100));
    let raw_premul_idx = (50 * 80 + 40) * 4;
    let raw_premul = &p_premul.as_bytes()[raw_premul_idx..raw_premul_idx + 4];
    assert!(
        (i16::from(raw_premul[0]) - 102).abs() <= 3 && raw_premul[3] == 128,
        "expected premultiplied [~102, 0, 0, 128], got {raw_premul:?}"
    );

    let mut dpi_bgra = dpi_premul.clone();
    dpi_bgra.format = PixelFormat::Bgra8;
    let (p_bgra, _) = doc.render_page(0, &dpi_bgra).unwrap();
    let straight_from_bgra = p_bgra.pixel_rgba8(40, 50).unwrap();
    assert!(
        (i16::from(straight_from_bgra[0]) - 204).abs() <= 3 && straight_from_bgra[3] == 128,
        "expected straight [~204, 0, 0, 128] (no double unpremul!), got {straight_from_bgra:?}"
    );

    // 2. Limit errors (`LimitKind::Pixels`) enforced upfront before PDFium allocation!
    let mut capped = dpi_bgra.clone();
    capped.limits.max_pixels = 1_000;
    assert!(matches!(
        doc.render_page(0, &capped),
        Err(BackendError::Raster(Error::Limit(LimitKind::Pixels)))
    ));

    // 3. `Viewport::Tile` overhang with extra rotation (`R90`) and non-white background
    //    (`Some([10, 20, 30, 255])`): overhang pixels outside page must equal `[10, 20, 30, 255]`!
    let mut overhang_tile = RenderOpts::with_viewport(Viewport::Tile {
        page_width: 100,
        page_height: 80,
        x: 70,
        y: 50,
        w: 50,
        h: 50,
        rotation: Rotation::R90,
    });
    overhang_tile.background = Some([10, 20, 30, 255]);
    let (tile_pix, _) = doc.render_page(0, &overhang_tile).unwrap();
    assert_eq!(tile_pix.pixel_rgba8(48, 48), Some([10, 20, 30, 255]));

    // 4. Concurrent renders of the SAME `Arc<dyn RenderedDoc>` across 4 OS threads!
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let shared = doc.clone();
            let opts = dpi_bgra.clone();
            std::thread::spawn(move || shared.render_page(0, &opts).unwrap().0)
        })
        .collect();
    for h in handles {
        assert_eq!(h.join().unwrap(), p_bgra);
    }
}
