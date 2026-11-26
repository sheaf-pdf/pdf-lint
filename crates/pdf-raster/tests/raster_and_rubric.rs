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
