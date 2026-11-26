//! Integration and live `chromium/8152` PDFium `0–100` rubric suite for
//! `pdf-raster`.

use std::sync::Arc;

use pdf_fixtures::fixture::{Body as FixtureBody, Xref, file as write_fixture_file};
use pdf_fixtures::font::{Glyph as FixtureGlyph, TrueType};
use pdf_raster::pdf_annots::{AnnotColor, AnnotFlags, BuiltAnnot, ShapeKind};
#[cfg(feature = "pdfium")]
use pdf_raster::pdf_content_streams::{Blend, GState, Res};
use pdf_raster::pdf_content_streams::{Color, Concat, Reserved};
use pdf_raster::pdf_fonts::Std14;
#[cfg(feature = "pdfium")]
use pdf_raster::pdf_gfx::Matrix;
use pdf_raster::pdf_gfx::{Canvas, PageGeom, Rect, Rotation};
#[cfg(feature = "pdfium")]
use pdf_raster::pdf_images::{JpegColorSpace, encode_jpeg};
#[cfg(feature = "pdfium")]
use pdf_raster::pdf_objects::Dict;
use pdf_raster::pdf_objects::pdf_tokenizer::Rules;
use pdf_raster::pdf_objects::{Object, ObjectId, Resolve, Stream, Value, dict};
use pdf_raster::pdf_semantics::Limits as SemLimits;
use pdf_raster::pdf_semantics::pages::pages;
#[cfg(feature = "pdfium")]
use pdf_raster::pdfium::{
    Error as PdfiumError, Pdfium, RenderFlags, RenderOptions as PdfiumRenderOptions,
};
#[cfg(feature = "pdfium")]
use pdf_raster::rubric::{
    CaseEvaluation, RgbaImage as PdfiumRgba, RubricProfile, evaluate_case, score_images,
};
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

#[cfg(feature = "pdfium")]
fn try_pdfium() -> Option<Pdfium> {
    match Pdfium::try_global() {
        Ok(p) => Some(p),
        Err(PdfiumError::LibraryNotFound { .. }) => None,
        Err(other) => panic!("unexpected libpdfium load error: {other}"),
    }
}

#[cfg(feature = "pdfium")]
fn pixmap_to_pdfium_rgba(pix: &Pixmap) -> PdfiumRgba {
    PdfiumRgba::from_pixmap(pix).unwrap()
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

// ---------------------------------------------------------------------------
// 5. Live PDFium (`chromium/8152`) `0–100` Rubric Conformance Suite
// ---------------------------------------------------------------------------

#[cfg(feature = "pdfium")]
#[test]
fn pdfium_8152_rubric_conformance_across_paths_text_images_groups_patterns_and_shadings() {
    let Some(pdfium) = try_pdfium() else {
        eprintln!("skipping live PDFium rubric test (libpdfium not present)");
        return;
    };

    // Case A: Vector paths, clips, strokes, dashes, & EvenOdd winding
    {
        let stream = b"\
            0.92 0.95 0.98 rg 10 10 180 180 re f \
            0.85 0.15 0.20 rg 25 25 70 70 re 42 42 36 36 re f* \
            q 110 25 70 70 re W n \
              0.15 0.55 0.30 rg 95 10 100 100 re f \
            Q \
            0.10 0.20 0.65 RG 6 w [10 6] 0 d 25 130 m 175 170 l S\n";
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
                    "Contents" => Object::reference(4),
                }),
            ),
            (4, Object::from(Stream::new(dict! {}, stream.to_vec()))),
        ]);
        assert_rubric_case(&pdfium, "vectors_clips_dashes", &pdf, 200, 200, true, 96.0);
    }

    // Case B: Embedded TrueType glyphs & Type 3 (`d0` + `d1`) glyphs
    #[cfg(feature = "programs")]
    {
        let ttf_bytes = build_fixture_ttf();
        let stream = b"\
            0.1 0.15 0.45 rg BT /F1 64 Tf 16 110 Td (AB A) Tj ET \
            0.75 0.15 0.10 rg BT /F3 48 Tf 20 25 Td (XY) Tj ET\n";
        let cp_x = b"1 0 0.1 0.1 0.9 0.9 d1 0.1 0.1 0.8 0.8 re f";
        let cp_y = b"1 0 d0 0.1 0.55 0.25 rg 0.1 0.1 0.8 0.8 re f";
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
                    "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(220), Object::from(180)],
                }),
            ),
            (
                3,
                Object::from(dict! {
                    "Type" => Object::name("Page"),
                    "Parent" => Object::reference(2),
                    "Resources" => dict! {
                        "Font" => dict! { "F1" => Object::reference(5), "F3" => Object::reference(8) },
                    },
                    "Contents" => Object::reference(4),
                }),
            ),
            (4, Object::from(Stream::new(dict! {}, stream.to_vec()))),
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
                        w[0] = Object::from(400);
                        w[33] = Object::from(800);
                        w[34] = Object::from(800);
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
                    "CharProcs" => dict! { "gx" => Object::reference(9), "gy" => Object::reference(10) },
                    "FirstChar" => 88,
                    "LastChar" => 89,
                    "Widths" => vec![Object::from(1), Object::from(1)],
                }),
            ),
            (9, Object::from(Stream::new(dict! {}, cp_x.to_vec()))),
            (10, Object::from(Stream::new(dict! {}, cp_y.to_vec()))),
        ]);
        assert_rubric_case(
            &pdfium,
            "embedded_ttf_and_type3",
            &pdf,
            220,
            180,
            true,
            95.0,
        );
    }

    // Case C: Placed DCT JPEG XObject + 1-bit `/ImageMask` Stencil
    {
        let mut rgb_px = Vec::with_capacity(16 * 16 * 3);
        for y in 0..16u8 {
            for x in 0..16u8 {
                rgb_px.extend_from_slice(&[x * 16, y * 16, 180]);
            }
        }
        let jpeg_stream = encode_jpeg(&rgb_px, 16, 16, JpegColorSpace::Rgb, 98).unwrap();
        // 8x8 1-bit stencil checkerboard (`0` = painted, `1` = transparent):
        let stencil_bits = vec![
            0b0000_1111u8,
            0b0000_1111,
            0b0000_1111,
            0b0000_1111,
            0b1111_0000,
            0b1111_0000,
            0b1111_0000,
            0b1111_0000,
        ];
        let stencil_stream = Stream::new(
            dict! {
                "Type" => Object::name("XObject"),
                "Subtype" => Object::name("Image"),
                "Width" => 8,
                "Height" => 8,
                "ImageMask" => true,
                "BitsPerComponent" => 1,
            },
            stencil_bits,
        );
        let stream = b"\
            q 80 0 0 80 20 20 cm /Im1 Do Q \
            q 0.85 0.2 0.1 rg 64 0 0 64 116 28 cm /St1 Do Q\n";
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
                    "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(200), Object::from(120)],
                }),
            ),
            (
                3,
                Object::from(dict! {
                    "Type" => Object::name("Page"),
                    "Parent" => Object::reference(2),
                    "Resources" => dict! {
                        "XObject" => dict! { "Im1" => Object::reference(5), "St1" => Object::reference(6) },
                    },
                    "Contents" => Object::reference(4),
                }),
            ),
            (4, Object::from(Stream::new(dict! {}, stream.to_vec()))),
            (5, Object::from(jpeg_stream)),
            (6, Object::from(stencil_stream)),
        ]);
        assert_rubric_case(
            &pdfium,
            "jpeg_xobject_and_stencil",
            &pdf,
            200,
            120,
            false,
            97.0,
        );
    }

    // Case D: Isolated Transparency Group, `/BM /Multiply`, `/ca`, & `/SMask /Luminosity`
    {
        let page_stream = b"\
            0.95 0.85 0.20 rg 20 20 100 100 re f \
            q /GS1 gs 0.20 0.65 0.90 rg 60 40 100 100 re f Q \
            q /GS2 gs /Fm1 Do Q\n";
        let mask_group_stream = b"0.6 0.6 0.6 rg 30 30 120 120 re f";
        let isolated_form_stream = b"\
            0.9 0.2 0.3 rg 20 90 60 60 re f \
            q /GS_MUL gs 0.2 0.8 0.4 rg 45 75 60 60 re f Q\n";

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
                    "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(180), Object::from(180)],
                }),
            ),
            (
                3,
                Object::from(dict! {
                    "Type" => Object::name("Page"),
                    "Parent" => Object::reference(2),
                    "Resources" => dict! {
                        "ExtGState" => dict! {
                            "GS1" => dict! { "Type" => Object::name("ExtGState"), "BM" => Object::name("Multiply"), "ca" => 0.75 },
                            "GS2" => dict! {
                                "Type" => Object::name("ExtGState"),
                                "SMask" => dict! {
                                    "Type" => Object::name("Mask"),
                                    "S" => Object::name("Luminosity"),
                                    "G" => Object::reference(6),
                                },
                            },
                        },
                        "XObject" => dict! { "Fm1" => Object::reference(5) },
                    },
                    "Contents" => Object::reference(4),
                }),
            ),
            (4, Object::from(Stream::new(dict! {}, page_stream.to_vec()))),
            (
                5,
                Object::from(Stream::new(
                    dict! {
                        "Type" => Object::name("XObject"),
                        "Subtype" => Object::name("Form"),
                        "BBox" => vec![Object::from(0), Object::from(0), Object::from(180), Object::from(180)],
                        "Group" => dict! { "S" => Object::name("Transparency"), "I" => true },
                        "Resources" => dict! {
                            "ExtGState" => dict! {
                                "GS_MUL" => dict! { "BM" => Object::name("Multiply") },
                            },
                        },
                    },
                    isolated_form_stream.to_vec(),
                )),
            ),
            (
                6,
                Object::from(Stream::new(
                    dict! {
                        "Type" => Object::name("XObject"),
                        "Subtype" => Object::name("Form"),
                        "BBox" => vec![Object::from(0), Object::from(0), Object::from(180), Object::from(180)],
                        "Group" => dict! { "S" => Object::name("Transparency"), "I" => true, "CS" => Object::name("DeviceRGB") },
                    },
                    mask_group_stream.to_vec(),
                )),
            ),
        ]);
        assert_rubric_case(
            &pdfium,
            "transparency_group_multiply_smask",
            &pdf,
            180,
            180,
            true,
            96.0,
        );
    }

    // Case E: Colored Tiling Pattern (`PatternType 1`) + Axial (`ShadingType 2`) & Radial (`ShadingType 3`) Shadings
    {
        let page_stream = b"\
            q /Pattern cs /P1 scn 15 95 170 70 re f Q \
            q 15 15 75 65 re W n /Sh2 sh Q \
            q 110 15 75 65 re W n /Sh3 sh Q\n";
        let pat_stream = b"\
            0.95 0.90 0.80 rg 0 0 20 20 re f \
            0.20 0.45 0.80 rg 0 0 10 10 re 10 10 10 10 re f\n";

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
                    "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(200), Object::from(180)],
                }),
            ),
            (
                3,
                Object::from(dict! {
                    "Type" => Object::name("Page"),
                    "Parent" => Object::reference(2),
                    "Resources" => dict! {
                        "Pattern" => dict! { "P1" => Object::reference(5) },
                        "Shading" => dict! { "Sh2" => Object::reference(6), "Sh3" => Object::reference(7) },
                    },
                    "Contents" => Object::reference(4),
                }),
            ),
            (4, Object::from(Stream::new(dict! {}, page_stream.to_vec()))),
            (
                5,
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
                    pat_stream.to_vec(),
                )),
            ),
            (
                6,
                Object::from(dict! {
                    "ShadingType" => 2,
                    "ColorSpace" => Object::name("DeviceRGB"),
                    "Coords" => vec![Object::from(15), Object::from(15), Object::from(90), Object::from(80)],
                    "Extend" => vec![Object::from(true), Object::from(true)],
                    "Function" => dict! {
                        "FunctionType" => 2,
                        "Domain" => vec![Object::from(0), Object::from(1)],
                        "C0" => vec![Object::from(0.9), Object::from(0.1), Object::from(0.2)],
                        "C1" => vec![Object::from(0.1), Object::from(0.8), Object::from(0.4)],
                        "N" => 1,
                    },
                }),
            ),
            (
                7,
                Object::from(dict! {
                    "ShadingType" => 3,
                    "ColorSpace" => Object::name("DeviceRGB"),
                    "Coords" => vec![
                        Object::from(147.5), Object::from(47.5), Object::from(4.0),
                        Object::from(147.5), Object::from(47.5), Object::from(36.0),
                    ],
                    "Extend" => vec![Object::from(true), Object::from(true)],
                    "Function" => dict! {
                        "FunctionType" => 2,
                        "Domain" => vec![Object::from(0), Object::from(1)],
                        "C0" => vec![Object::from(1.0), Object::from(0.95), Object::from(0.2)],
                        "C1" => vec![Object::from(0.15), Object::from(0.2), Object::from(0.75)],
                        "N" => 1,
                    },
                }),
            ),
        ]);
        assert_rubric_case(
            &pdfium,
            "tiling_pattern_and_shadings",
            &pdf,
            200,
            180,
            true,
            96.0,
        );
    }

    // Case F: Highlight + Square Annotations (`FPDF_ANNOT`)
    {
        let mut hl_canvas = Canvas::new();
        let mut gs = GState::default();
        gs.blend = Some(Blend::Multiply);
        hl_canvas.gstate(gs);
        hl_canvas.fill_color(Color::rgb8([255, 235, 80]));
        hl_canvas.fill_rect(Rect::from_xywh(0.0, 0.0, 120.0, 24.0));
        let ap = pdf_raster::pdf_annots::SynthesizedAp::drawn(
            Rect::from_xywh(0.0, 0.0, 120.0, 24.0),
            Matrix::IDENTITY,
            hl_canvas,
        );
        let ap_stream = ap.to_stream();

        let base_stream = b"0.2 0.2 0.2 rg 30 48 100 8 re f\n";
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
                    "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(180), Object::from(100)],
                }),
            ),
            (
                3,
                Object::from(dict! {
                    "Type" => Object::name("Page"),
                    "Parent" => Object::reference(2),
                    "Contents" => Object::reference(4),
                    "Annots" => vec![Object::reference(5)],
                }),
            ),
            (4, Object::from(Stream::new(dict! {}, base_stream.to_vec()))),
            (
                5,
                Object::from(dict! {
                    "Type" => Object::name("Annot"),
                    "Subtype" => Object::name("Highlight"),
                    "F" => AnnotFlags::PRINT.bits() as i64,
                    "Rect" => vec![Object::from(20), Object::from(40), Object::from(140), Object::from(64)],
                    "QuadPoints" => vec![
                        Object::from(20), Object::from(64),
                        Object::from(140), Object::from(64),
                        Object::from(20), Object::from(40),
                        Object::from(140), Object::from(40),
                    ],
                    "AP" => dict! { "N" => Object::reference(6) },
                }),
            ),
            (6, Object::from(ap_stream)),
        ]);
        let _ = Res::dict(Dict::new());
        assert_rubric_case(
            &pdfium,
            "annot_multiply_highlight",
            &pdf,
            180,
            100,
            true,
            97.0,
        );
    }
}

#[cfg(feature = "pdfium")]
fn assert_rubric_case(
    pdfium: &Pdfium,
    name: &str,
    pdf_bytes: &[u8],
    width: u32,
    height: u32,
    smooth: bool,
    min_score: f32,
) {
    let mut opts_pdfium = RenderOpts::with_viewport(Viewport::Custom {
        width,
        height,
        page_to_device: PageGeom::new(
            Rect::new(0.0, 0.0, width as f32, height as f32),
            None,
            Rotation::R0,
        )
        .to_view(),
    });
    opts_pdfium.anti_alias = if smooth {
        AntiAlias::ALL
    } else {
        AntiAlias::NONE
    };
    opts_pdfium.rules = Rules::Pdfium;

    let mut opts_iso = opts_pdfium.clone();
    opts_iso.rules = Rules::Iso;

    let (pix_pdfium_mode, _, faults_p) = render_pdf_bytes(pdf_bytes, &opts_pdfium);
    let (pix_iso_mode, _, faults_i) = render_pdf_bytes(pdf_bytes, &opts_iso);
    assert!(
        faults_p.is_empty() && faults_i.is_empty(),
        "clean conforming case {name} must emit 0 faults: {faults_p:?} / {faults_i:?}"
    );

    let cand_pdfium = pixmap_to_pdfium_rgba(&pix_pdfium_mode);
    let cand_iso = pixmap_to_pdfium_rgba(&pix_iso_mode);

    let mut flags = RenderFlags::ANNOT;
    if !smooth {
        flags |=
            RenderFlags::NO_SMOOTHPATH | RenderFlags::NO_SMOOTHTEXT | RenderFlags::NO_SMOOTHIMAGE;
    }
    let p_opts = PdfiumRenderOptions {
        width,
        height,
        background: [255, 255, 255, 255],
        rotate: 0,
        flags,
        forms: true,
        matrix_and_clip: None,
    };
    let oracle_img = pdfium
        .render_page(pdf_bytes, 0, &p_opts)
        .unwrap_or_else(|e| panic!("PDFium failed on {name}: {e}"));

    let mut cfg = RubricProfile::ConformingCore.config();
    cfg.visual_pdfium_floor = min_score;

    let eval = evaluate_case(
        &cfg,
        &CaseEvaluation {
            candidate_pdfium: Some(&cand_pdfium),
            candidate_iso: Some(&cand_iso),
            pdfium_reference: Some(&oracle_img),
            iso_reference: None,
            iso_fault_count: faults_i.len(),
            write_lint_errors: 0,
        },
    )
    .unwrap();

    let direct = score_images(&cand_pdfium, &oracle_img).unwrap();
    assert!(
        eval.passed,
        "case {name} failed ConformingCore rubric (min {min_score}): failures={:?}, visual={direct:?}",
        eval.failures
    );
}
