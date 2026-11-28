//! Live PDFium (`as of chromium/8152`) + `0–100` rubric and `RenderBackend`
//! parity suite for `pdf-raster` and `pdfs`.

use std::io::Cursor;
use std::sync::Arc;
use std::thread;

use pdf_fixtures::fixture::{Body as FixtureBody, Xref, file as write_fixture_file};
use pdf_fixtures::font::{Glyph as FixtureGlyph, TrueType};
use pdf_raster::pdf_annots::{AnnotFlags, SynthesizedAp};
use pdf_raster::pdf_content_streams::{Blend, Color, GState};
use pdf_raster::pdf_gfx::{Canvas, Matrix, PageGeom, Rect, Rotation};
use pdf_raster::pdf_images::{JpegColorSpace, encode_jpeg};
use pdf_raster::pdf_objects::pdf_tokenizer::Rules;
use pdf_raster::pdf_objects::{Object, Resolve, Stream, Value, dict};
use pdf_raster::pdf_semantics::Limits as SemLimits;
use pdf_raster::pdf_semantics::pages::pages;
use pdf_raster::pdfium::{Error as PdfiumError, LoadFailure};
use pdf_raster::{
    AntiAlias, BackendError, Draw, Parts, PixelFormat, Pixmap, Refusal, RenderBackend, RenderCache,
    RenderOpts, RenderSource, Viewport, render_page_with,
};
use pdf_raster_oracle::rubric::{
    CaseEvaluation, RgbaImage, RgbaImageExt, RubricProfile, compare_backends, evaluate_case,
    score_images,
};
use pdf_raster_oracle::{RenderOptions, require_pdfium, require_pdfium_backend};
use pdf_structure::read::Recovery;
use pdf_structure::{Options as OpenOptions, Reading};
use pdfs::prelude::{Document, Paint, Password, Protection, RustBackend};

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

fn build_fixture_ttf() -> Vec<u8> {
    let mut tt = TrueType::new(1000);
    let g_a = tt.glyph(FixtureGlyph::Box(100, 0, 700, 750), 800);
    tt.map('A', g_a);
    let g_bar = tt.glyph(FixtureGlyph::Box(100, 0, 700, 300), 800);
    let g_b = tt.glyph(
        FixtureGlyph::Composite(vec![(g_bar, 0, 0), (g_bar, 0, 450)]),
        800,
    );
    tt.map('B', g_b);
    let g_sp = tt.glyph(FixtureGlyph::Empty, 400);
    tt.map(' ', g_sp);
    tt.build()
}

fn render_pdf_bytes_with_faults(
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

fn assert_rubric_case(
    pdfium_backend: &dyn RenderBackend,
    name: &str,
    pdf_bytes: &[u8],
    width: u32,
    height: u32,
    smooth: bool,
    min_score: f32,
) {
    let mut opts_pdfium = RenderOpts::with_viewport(Viewport::Page {
        width,
        rotation: Rotation::R0,
    });
    opts_pdfium.anti_alias = if smooth {
        AntiAlias::ALL
    } else {
        AntiAlias::NONE
    };
    opts_pdfium.rules = Rules::Pdfium;

    let mut opts_iso = opts_pdfium.clone();
    opts_iso.rules = Rules::Iso;

    let (pix_pdfium_mode, sum_p, faults_p) = render_pdf_bytes_with_faults(pdf_bytes, &opts_pdfium);
    let (pix_iso_mode, sum_i, faults_i) = render_pdf_bytes_with_faults(pdf_bytes, &opts_iso);
    assert!(sum_p.reported && sum_i.reported);
    assert_eq!(
        (pix_pdfium_mode.width(), pix_pdfium_mode.height()),
        (width, height)
    );
    assert!(
        faults_p.is_empty() && faults_i.is_empty(),
        "clean conforming case {name} must emit 0 faults: {faults_p:?} / {faults_i:?}"
    );

    let cand_pdfium = RgbaImage::from_pixmap(&pix_pdfium_mode).unwrap();
    let cand_iso = RgbaImage::from_pixmap(&pix_iso_mode).unwrap();

    let orac_doc = pdfium_backend
        .open_bytes(Arc::from(pdf_bytes), None)
        .unwrap_or_else(|e| panic!("PDFium open failed on {name}: {e}"));
    let (oracle_pix, oracle_sum) = orac_doc
        .render_page(0, &opts_pdfium)
        .unwrap_or_else(|e| panic!("PDFium render failed on {name}: {e}"));
    assert!(!oracle_sum.reported);
    let oracle_img = RgbaImage::from_pixmap(&oracle_pix).unwrap();
    assert!(
        !oracle_img.is_uniform(),
        "case {name} rendered completely uniform/blank in PDFium oracle"
    );

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

#[test]
#[ignore = "requires libpdfium (run via `just test-pdfium`)"]
fn pdfium_8152_rubric_conformance_across_paths_text_images_groups_patterns_and_shadings() {
    let pdfium = require_pdfium_backend();

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

    // Case F: Highlight Annotation (`FPDF_ANNOT`)
    {
        let mut hl_canvas = Canvas::new();
        let mut gs = GState::default();
        gs.blend = Some(Blend::Multiply);
        hl_canvas.gstate(gs);
        hl_canvas.fill_color(Color::rgb8([255, 235, 80]));
        hl_canvas.fill_rect(Rect::from_xywh(0.0, 0.0, 120.0, 24.0));
        let ap = SynthesizedAp::drawn(
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

#[test]
#[ignore = "requires libpdfium (run via `just test-pdfium`)"]
fn pdfium_and_rust_backends_agree_across_pages_tiles_formats_custom_viewport_and_threads() {
    let rust = RustBackend::new();
    let pdfium = require_pdfium_backend();
    let oracle = require_pdfium();

    let mut doc = Document::new();
    {
        let p1 = doc.add_page(Rect::new(0.0, 0.0, 200.0, 160.0));
        p1.fill_color(Color::rgb8([30, 100, 210]));
        p1.rect(20.0, 20.0, 70.0, 120.0);
        p1.paint(Paint::Fill);
        p1.fill_color(Color::rgb8([220, 70, 40]));
        p1.rect(110.0, 40.0, 70.0, 80.0);
        p1.paint(Paint::Fill);
    }
    {
        let p2 = doc.add_page(Rect::new(0.0, 0.0, 160.0, 120.0));
        p2.fill_color(Color::rgb8([40, 170, 90]));
        p2.rect(15.0, 15.0, 130.0, 90.0);
        p2.paint(Paint::Fill);
    }

    let user_pw = Password("viewer-pass").prepared().unwrap();
    doc.encrypt(&Protection::new(&user_pw)).unwrap();
    let bytes: Arc<[u8]> = Arc::from(doc.to_bytes().unwrap());

    assert!(matches!(
        rust.open_bytes(bytes.clone(), None),
        Err(BackendError::Locked)
    ));
    assert!(matches!(
        pdfium.open_bytes(bytes.clone(), None),
        Err(BackendError::Locked)
    ));

    let rust_doc = rust.open_bytes(bytes.clone(), Some("viewer-pass")).unwrap();
    let pdfium_doc = pdfium
        .open_bytes(bytes.clone(), Some("viewer-pass"))
        .unwrap();
    assert_eq!(rust_doc.page_count(), 2);
    assert_eq!(pdfium_doc.page_count(), 2);
    assert_eq!(
        rust_doc.page_geom(0).unwrap(),
        pdfium_doc.page_geom(0).unwrap()
    );
    assert_eq!(
        rust_doc.page_geom(1).unwrap(),
        pdfium_doc.page_geom(1).unwrap()
    );
    assert!(matches!(
        rust_doc.page_geom(5),
        Err(BackendError::PageOutOfBounds {
            page_index: 5,
            page_count: 2
        })
    ));
    assert!(matches!(
        pdfium_doc.page_geom(5),
        Err(BackendError::PageOutOfBounds {
            page_index: 5,
            page_count: 2
        })
    ));

    // `Viewport::Custom` with `draw.forms == true` is explicitly refused by `PdfiumBackend`
    // rather than silently dropping interactive form widgets; with `draw.forms == false`,
    // both backends succeed and agree!
    let custom_vp = Viewport::Custom {
        width: 200,
        height: 160,
        page_to_device: PageGeom::new(Rect::new(0.0, 0.0, 200.0, 160.0), None, Rotation::R0)
            .to_view(),
    };
    let custom_with_forms = RenderOpts::with_viewport(custom_vp);
    assert!(matches!(
        pdfium_doc.render_page(0, &custom_with_forms),
        Err(BackendError::Unsupported(_))
    ));
    let mut custom_no_forms = RenderOpts::with_viewport(custom_vp);
    custom_no_forms.draw = Draw::new(true, false);
    let rubric = RubricProfile::ConformingCore.config();
    let custom_score = compare_backends(
        &rust,
        &pdfium,
        bytes.clone(),
        Some("viewer-pass"),
        0,
        &custom_no_forms,
        rubric.visual_pdfium_floor,
    )
    .unwrap();
    assert!(
        custom_score.score >= rubric.visual_pdfium_floor,
        "{custom_score:?}"
    );

    for rot in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
        let mut opts = RenderOpts::with_viewport(Viewport::Page {
            width: 160,
            rotation: rot,
        });
        opts.format = PixelFormat::Bgra8;
        let score = compare_backends(
            &rust,
            &pdfium,
            bytes.clone(),
            Some("viewer-pass"),
            0,
            &opts,
            rubric.visual_pdfium_floor,
        )
        .unwrap();
        assert!(
            score.score >= rubric.visual_pdfium_floor,
            "page 0 rotation {rot:?} failed: {score:?}"
        );
    }

    // Include an overhang tile (`x = 140, w = 100` past `page_width = 200`) with non-white
    // background and rotation to verify single-buffer pre-fill clearing across both backends!
    for (tx, ty, rot, bg) in [
        (0, 0, Rotation::R0, Some([255, 255, 255, 255])),
        (100, 0, Rotation::R0, Some([255, 255, 255, 255])),
        (0, 80, Rotation::R0, Some([255, 255, 255, 255])),
        (100, 80, Rotation::R0, Some([255, 255, 255, 255])),
        (140, 100, Rotation::R0, Some([255, 255, 255, 255])),
        (150, 130, Rotation::R90, Some([25, 35, 45, 255])),
    ] {
        let (pw, ph) = match rot {
            Rotation::R90 | Rotation::R270 => (160, 200),
            _ => (200, 160),
        };
        let mut tile_opts = RenderOpts::with_viewport(Viewport::Tile {
            page_width: pw,
            page_height: ph,
            x: tx,
            y: ty,
            w: 100,
            h: 80,
            rotation: rot,
        });
        tile_opts.background = bg;
        tile_opts.anti_alias = AntiAlias::NONE;
        let score = compare_backends(
            &rust,
            &pdfium,
            bytes.clone(),
            Some("viewer-pass"),
            0,
            &tile_opts,
            rubric.visual_pdfium_floor,
        )
        .unwrap();
        assert!(
            score.score >= rubric.visual_pdfium_floor,
            "tile ({tx}, {ty}, {rot:?}) failed floor: {score:?}"
        );
    }

    // Concurrent multi-thread `render_page_custom_access` (`FPDF_LoadCustomDocument`) check.
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let b = bytes.clone();
            thread::spawn(move || {
                oracle
                    .render_page_custom_access(
                        Cursor::new(b.to_vec()),
                        Some("viewer-pass"),
                        0,
                        &RenderOptions::crisp_swatches(200, 160),
                    )
                    .unwrap()
            })
        })
        .collect();
    let rendered: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for img in &rendered[1..] {
        assert_eq!(img, &rendered[0]);
    }

    // Inherited `/MediaBox` with non-zero origin + direct `/CropBox`, `/UserUnit 2.0` across all
    // viewports, embedded missing glyph (`'Z'`), and concurrent `Locked` vs corrupt open stress!
    {
        let inh_stream =
            b"0.15 0.45 0.85 rg 30 40 80 60 re f 0.85 0.2 0.15 rg 110 100 80 60 re f\n";
        let inh_pdf: Arc<[u8]> = Arc::from(build_pdf_bytes(vec![
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
                    "MediaBox" => vec![Object::from(10), Object::from(20), Object::from(210), Object::from(180)],
                }),
            ),
            (
                3,
                Object::from(dict! {
                    "Type" => Object::name("Page"),
                    "Parent" => Object::reference(2),
                    "CropBox" => vec![Object::from(30), Object::from(40), Object::from(190), Object::from(160)],
                    "UserUnit" => 2.0,
                    "Contents" => Object::reference(4),
                }),
            ),
            (4, Object::from(Stream::new(dict! {}, inh_stream.to_vec()))),
        ]));

        let r_inh = rust.open_bytes(inh_pdf.clone(), None).unwrap();
        let p_inh = pdfium.open_bytes(inh_pdf.clone(), None).unwrap();
        assert_eq!(
            r_inh.page_geom(0).unwrap().crop_box,
            p_inh.page_geom(0).unwrap().crop_box
        );

        let crop_geom = PageGeom::new(Rect::new(30.0, 40.0, 190.0, 160.0), None, Rotation::R0);
        for vp in [
            Viewport::Page {
                width: 160,
                rotation: Rotation::R0,
            },
            Viewport::Dpi {
                dpi: 72.0,
                rotation: Rotation::R0,
            },
            Viewport::Tile {
                page_width: 160,
                page_height: 120,
                x: 40,
                y: 20,
                w: 80,
                h: 60,
                rotation: Rotation::R0,
            },
            Viewport::Custom {
                width: 160,
                height: 120,
                page_to_device: crop_geom.to_view(),
            },
        ] {
            let mut opts = RenderOpts::with_viewport(vp);
            opts.anti_alias = AntiAlias::NONE;
            opts.draw = Draw::CONTENT_ONLY;
            let (rp, _) = r_inh.render_page(0, &opts).unwrap();
            let (pp, _) = p_inh.render_page(0, &opts).unwrap();
            assert_eq!(
                (rp.width(), rp.height()),
                (pp.width(), pp.height()),
                "viewport {vp:?} pixel size mismatch"
            );
            let cmp = compare_backends(
                &rust,
                &pdfium,
                inh_pdf.clone(),
                None,
                0,
                &opts,
                rubric.visual_pdfium_floor,
            )
            .unwrap();
            assert!(
                cmp.score >= rubric.visual_pdfium_floor,
                "viewport {vp:?} failed: {cmp:?}"
            );
        }
    }

    // Embedded TrueType missing `'Z'` draws `.notdef` matching PDFium (`compare_backends`), and
    // Type 0 out-of-range `<0099>` (`NoGlyph`) draws zero ink in both backends (byte-for-byte):
    {
        let ttf = build_fixture_ttf();
        let z_content = b"0 0 0 rg BT /F1 48 Tf 12 20 Td (Z) Tj ET\n";
        let z_pdf: Arc<[u8]> = Arc::from(build_pdf_bytes(vec![
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
                    "Resources" => dict! { "Font" => dict! { "F1" => Object::reference(5) } },
                    "Contents" => Object::reference(4),
                }),
            ),
            (4, Object::from(Stream::new(dict! {}, z_content.to_vec()))),
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
                    dict! { "Length1" => ttf.len() as i64 },
                    ttf.clone(),
                )),
            ),
        ]));
        let z_opts = RenderOpts::with_viewport(Viewport::Page {
            width: 100,
            rotation: Rotation::R0,
        });
        let z_cmp = compare_backends(
            &rust,
            &pdfium,
            z_pdf,
            None,
            0,
            &z_opts,
            rubric.visual_pdfium_floor,
        )
        .unwrap();
        assert!(
            z_cmp.score >= rubric.visual_pdfium_floor,
            "missing 'Z' (.notdef) diverged from PDFium: {z_cmp:?}"
        );

        let cid_miss = b"0 0 0 rg BT /F1 48 Tf 12 20 Td <0099> Tj ET\n";
        let cid_pdf: Arc<[u8]> = Arc::from(build_pdf_bytes(vec![
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
                    "Resources" => dict! { "Font" => dict! { "F1" => Object::reference(5) } },
                    "Contents" => Object::reference(4),
                }),
            ),
            (4, Object::from(Stream::new(dict! {}, cid_miss.to_vec()))),
            (
                5,
                Object::from(dict! {
                    "Type" => Object::name("Font"),
                    "Subtype" => Object::name("Type0"),
                    "BaseFont" => Object::name("SubsetCid"),
                    "Encoding" => Object::name("Identity-H"),
                    "DescendantFonts" => vec![Object::reference(6)],
                }),
            ),
            (
                6,
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
                    "FontDescriptor" => Object::reference(7),
                    "CIDToGIDMap" => Object::name("Identity"),
                }),
            ),
            (
                7,
                Object::from(dict! {
                    "Type" => Object::name("FontDescriptor"),
                    "FontName" => Object::name("SubsetCid"),
                    "Flags" => 32,
                    "FontBBox" => vec![Object::from(0), Object::from(0), Object::from(1000), Object::from(1000)],
                    "Ascent" => 800,
                    "Descent" => -200,
                    "CapHeight" => 700,
                    "StemV" => 80,
                    "FontFile2" => Object::reference(8),
                }),
            ),
            (
                8,
                Object::from(Stream::new(dict! { "Length1" => ttf.len() as i64 }, ttf)),
            ),
        ]));
        let r_cid = rust
            .open_bytes(cid_pdf.clone(), None)
            .unwrap()
            .render_page(0, &z_opts)
            .unwrap()
            .0;
        let p_cid = pdfium
            .open_bytes(cid_pdf, None)
            .unwrap()
            .render_page(0, &z_opts)
            .unwrap()
            .0;
        assert_eq!(r_cid.as_bytes(), p_cid.as_bytes());
        assert!(r_cid.as_bytes().iter().all(|&b| b == 255));
    }

    // Concurrent multi-thread `Locked` vs corrupt open stress (the adapter's lock, held from a
    // load to its error code, prevents crossed codes):
    {
        let corrupt: Arc<[u8]> = Arc::from(&b"%PDF-1.7\nnot a valid pdf"[..]);
        let mut threads = Vec::new();
        for t in 0..8 {
            let backend = pdfium;
            let enc = bytes.clone();
            let bad = corrupt.clone();
            threads.push(thread::spawn(move || {
                for _ in 0..24 {
                    if t % 2 == 0 {
                        let res = backend.open_bytes(enc.clone(), Some("wrong-pass"));
                        assert!(
                            matches!(res, Err(BackendError::Locked)),
                            "expected Locked, got {}",
                            res.err().unwrap()
                        );
                    } else {
                        let res = backend.open_bytes(bad.clone(), None);
                        assert!(
                            matches!(
                                res,
                                Err(BackendError::Refused {
                                    reason: Refusal::Format,
                                    ..
                                })
                            ),
                            "expected a format refusal on corrupt bytes, got {}",
                            res.err().unwrap()
                        );
                    }
                }
            }));
        }
        for th in threads {
            th.join().unwrap();
        }
    }
}

/// A source that counts the bytes read of it.
struct CountedSource {
    parts: Parts,
    read: std::sync::atomic::AtomicUsize,
}

impl RenderSource for CountedSource {
    fn len(&self) -> usize {
        self.parts.len()
    }

    fn read_at(
        &self,
        range: std::ops::Range<usize>,
    ) -> std::io::Result<std::borrow::Cow<'_, [u8]>> {
        self.read
            .fetch_add(range.len(), std::sync::atomic::Ordering::SeqCst);
        self.parts.read_at(range)
    }
}

#[test]
#[ignore = "requires libpdfium (run via `just test-pdfium`)"]
fn pdfium_opens_a_source_in_parts_names_its_refusals_and_holds_one_lock() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    let rust = RustBackend::new();
    let pdfium = require_pdfium_backend();

    // Two pages, and a stream no page draws that is most of the file.
    let mut doc = Document::new();
    {
        let p1 = doc.add_page(Rect::new(0.0, 0.0, 200.0, 160.0));
        p1.fill_color(Color::rgb8([30, 100, 210]));
        p1.rect(20.0, 20.0, 70.0, 120.0);
        p1.paint(Paint::Fill);
    }
    {
        let p2 = doc.add_page(Rect::new(0.0, 0.0, 160.0, 120.0));
        p2.fill_color(Color::rgb8([40, 170, 90]));
        p2.rect(15.0, 15.0, 130.0, 90.0);
        p2.paint(Paint::Fill);
    }
    let mut seed = 0x9E37_79B9_u32;
    let noise: Vec<u8> = (0..1 << 20)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 24) as u8
        })
        .collect();
    doc.structure_mut().add(Object::from(Stream::new(
        dict! { "Type" => Object::name("Unused") },
        noise,
    )));
    let bytes: Arc<[u8]> = Arc::from(doc.to_bytes().unwrap());
    let split = |at: &[usize]| {
        let mut parts = Vec::new();
        let mut from = 0;
        for &to in at {
            parts.push(Arc::from(&bytes[from..to]));
            from = to;
        }
        parts.push(Arc::from(&bytes[from..]));
        Parts::new(parts)
    };

    // A source in parts opens and draws as the bytes joined do, in both engines.
    let opts = RenderOpts::dpi(96.0);
    let whole = pdfium.open_bytes(bytes.clone(), None).unwrap();
    for at in [
        vec![5],
        vec![bytes.len() / 3, bytes.len() / 3 + 1],
        vec![bytes.len() - 9],
    ] {
        let p_doc = pdfium.open_source(Arc::new(split(&at)), None).unwrap();
        let r_doc = rust.open_source(Arc::new(split(&at)), None).unwrap();
        assert_eq!((p_doc.page_count(), r_doc.page_count()), (2, 2));
        for page in 0..2 {
            assert_eq!(
                p_doc.page_geom(page).unwrap(),
                whole.page_geom(page).unwrap()
            );
            assert_eq!(
                p_doc.page_geom(page).unwrap(),
                r_doc.page_geom(page).unwrap()
            );
            let drawn = p_doc.render_page(page, &opts).unwrap().0;
            assert_eq!(
                drawn,
                whole.render_page(page, &opts).unwrap().0,
                "{at:?}/{page}"
            );
        }
    }

    // PDFium reads the blocks it needs of a source, not the whole.
    let counted = Arc::new(CountedSource {
        parts: split(&[bytes.len() / 2]),
        read: std::sync::atomic::AtomicUsize::new(0),
    });
    let counted_dyn: Arc<dyn RenderSource> = counted.clone();
    let p_doc = pdfium.open_source(counted_dyn, None).unwrap();
    let _ = p_doc.render_page(0, &opts).unwrap();
    let read = counted.read.load(Ordering::SeqCst);
    assert!(
        read < bytes.len() / 2,
        "{read} of {} bytes read",
        bytes.len()
    );
    drop(p_doc);
    // With passwords to try too, in both engines: none of them copies the source.
    for backend in [&pdfium as &dyn RenderBackend, &rust] {
        let counted = Arc::new(CountedSource {
            parts: split(&[bytes.len() / 2]),
            read: std::sync::atomic::AtomicUsize::new(0),
        });
        let counted_dyn: Arc<dyn RenderSource> = counted.clone();
        let doc = backend
            .open_source_with_passwords(counted_dyn, &["one", "two"])
            .unwrap();
        let _ = doc.render_page(0, &opts).unwrap();
        let read = counted.read.load(Ordering::SeqCst);
        assert!(
            read < bytes.len() / 2,
            "{}: {read} of {} bytes read",
            backend.name(),
            bytes.len()
        );
    }

    // A source's passwords, as bytes'.
    let mut locked = Document::new();
    locked.add_page(Rect::new(0.0, 0.0, 40.0, 40.0));
    let pw = Password("viewer-pass").prepared().unwrap();
    locked.encrypt(&Protection::new(&pw)).unwrap();
    let locked: Arc<[u8]> = Arc::from(locked.to_bytes().unwrap());
    let locked_source = || -> Arc<dyn RenderSource> {
        Arc::new(Parts::new(vec![
            Arc::from(&locked[..locked.len() / 2]),
            Arc::from(&locked[locked.len() / 2..]),
        ]))
    };
    assert!(matches!(
        pdfium.open_source(locked_source(), None),
        Err(BackendError::Locked)
    ));
    assert!(matches!(
        pdfium.open_source(locked_source(), Some("wrong")),
        Err(BackendError::Locked)
    ));
    let opened = pdfium
        .open_source_with_passwords(locked_source(), &["wrong", "viewer-pass"])
        .unwrap();
    assert_eq!(opened.page_count(), 1);
    drop(opened);

    // What isn't a PDF is refused for its format by both engines, PDFium's code kept
    // and each engine's own words in the message.
    let garbage: Arc<[u8]> = Arc::from(&b"%PDF-1.7\nnot a valid pdf"[..]);
    let refused = pdfium.open_bytes(garbage.clone(), None).err().unwrap();
    assert_eq!(refused.refusal(), Some(Refusal::Format));
    let BackendError::Refused { error, .. } = &refused else {
        unreachable!()
    };
    assert_eq!(
        error.downcast_ref::<PdfiumError>(),
        Some(&PdfiumError::DocumentLoad {
            reason: LoadFailure::Format
        })
    );
    assert_eq!(
        refused.to_string(),
        "the document was refused: it isn't a PDF the engine can read: \
         PDFium refused the document: a format error (FPDF_ERR_FORMAT)"
    );
    let refused = rust.open_bytes(garbage, None).err().unwrap();
    assert_eq!(refused.refusal(), Some(Refusal::Format));
    assert_eq!(
        refused.to_string(),
        "the document was refused: it isn't a PDF the engine can read: \
         the trailer's /Root names no catalog"
    );

    // Another security handler's file: refused for its security by both, with or
    // without a password, never asked for one.
    let other_handler: Arc<[u8]> = Arc::from(write_fixture_file(
        "1.7",
        &[
            (
                1,
                FixtureBody::Object(Object::from(dict! {
                    "Type" => Object::name("Catalog"),
                    "Pages" => Object::reference(2),
                })),
            ),
            (
                2,
                FixtureBody::Object(Object::from(dict! {
                    "Type" => Object::name("Pages"),
                    "Kids" => Object::from(vec![Object::reference(3)]),
                    "Count" => 1,
                })),
            ),
            (
                3,
                FixtureBody::Object(Object::from(dict! {
                    "Type" => Object::name("Page"),
                    "Parent" => Object::reference(2),
                    "MediaBox" => Object::from(vec![
                        Object::from(0), Object::from(0), Object::from(72), Object::from(72),
                    ]),
                })),
            ),
            (
                4,
                FixtureBody::Object(Object::from(dict! {
                    "Filter" => Object::name("Vendor.Handler"),
                    "V" => 1,
                })),
            ),
        ],
        dict! { "Root" => Object::reference(1), "Encrypt" => Object::reference(4) },
        Xref::Table,
    ));
    for password in [None, Some("any")] {
        let p_err = pdfium
            .open_bytes(other_handler.clone(), password)
            .err()
            .unwrap();
        assert_eq!(p_err.refusal(), Some(Refusal::Security), "{p_err}");
        let r_err = rust
            .open_bytes(other_handler.clone(), password)
            .err()
            .unwrap();
        assert_eq!(r_err.refusal(), Some(Refusal::Security), "{r_err}");
    }

    // The lock: while a host holds it, no render of another thread's runs; the thread
    // that holds it renders under it; and a document dropped on another thread waits too.
    let shared: Arc<dyn pdf_raster::RenderedDoc> = Arc::from(whole);
    let held = pdfium.lock();
    let under_lock = shared.render_page(0, &opts).unwrap().0;
    let done = Arc::new(AtomicBool::new(false));
    let (started, has_started) = mpsc::channel();
    let other = {
        let (shared, done, opts) = (Arc::clone(&shared), Arc::clone(&done), opts.clone());
        thread::spawn(move || {
            started.send(()).unwrap();
            let drawn = shared.render_page(0, &opts).unwrap().0;
            done.store(true, Ordering::SeqCst);
            drawn
        })
    };
    has_started.recv().unwrap();
    thread::sleep(Duration::from_millis(100));
    assert!(
        !done.load(Ordering::SeqCst),
        "a render ran while the lock was held"
    );
    drop(held);
    assert_eq!(other.join().unwrap(), under_lock);
    assert!(done.load(Ordering::SeqCst));

    // Many threads opening, drawing and dropping documents at once, a host's own raw
    // calls among them under the same lock: every render the same.
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let (bytes, opts, expect) = (bytes.clone(), opts.clone(), under_lock.clone());
            thread::spawn(move || {
                for i in 0..6 {
                    if (t + i) % 3 == 0 {
                        let _pdfium = pdfium.lock();
                        let raw = pdfium
                            .pdfium()
                            .load_pdf_from_reader(Cursor::new(bytes.clone()), None)
                            .unwrap();
                        assert_eq!(raw.pages().len(), 2);
                    } else {
                        let doc = pdfium.open_bytes(bytes.clone(), None).unwrap();
                        assert_eq!(doc.render_page(0, &opts).unwrap().0, expect);
                    }
                }
            })
        })
        .collect();
    for th in threads {
        th.join().unwrap();
    }
}

/// A source that reads until it's told to fail, and then fails every read or
/// panics in it.
struct FlakySource {
    bytes: Vec<u8>,
    gone: std::sync::atomic::AtomicBool,
    panics: bool,
}

impl FlakySource {
    fn over(bytes: &[u8], gone: bool, panics: bool) -> Arc<Self> {
        Arc::new(Self {
            bytes: bytes.to_vec(),
            gone: std::sync::atomic::AtomicBool::new(gone),
            panics,
        })
    }

    fn go(&self, gone: bool) {
        self.gone.store(gone, std::sync::atomic::Ordering::SeqCst);
    }
}

impl RenderSource for FlakySource {
    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn read_at(
        &self,
        range: std::ops::Range<usize>,
    ) -> std::io::Result<std::borrow::Cow<'_, [u8]>> {
        if self.gone.load(std::sync::atomic::Ordering::SeqCst) {
            assert!(!self.panics, "the source panicked in a read");
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the disk went away",
            ));
        }
        self.bytes.read_at(range)
    }
}

/// Pages of one color each, a stream no page draws between them, so that a
/// page's objects are read when the page is, not with the file's ends.
fn padded_pages(count: usize) -> Vec<u8> {
    let mut doc = Document::new();
    let mut seed = 0x9E37_79B9_u32;
    for _ in 0..count {
        let page = doc.add_page(Rect::new(0.0, 0.0, 72.0, 72.0));
        page.fill_color(Color::rgb8([20, 120, 220]));
        page.rect(0.0, 0.0, 72.0, 72.0);
        page.paint(Paint::Fill);
        let noise: Vec<u8> = (0..1 << 16)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            })
            .collect();
        doc.structure_mut().add(Object::from(Stream::new(
            dict! { "Type" => Object::name("Unused") },
            noise,
        )));
    }
    doc.to_bytes().unwrap()
}

#[test]
#[ignore = "requires libpdfium (run via `just test-pdfium`)"]
fn pdfium_and_rust_backends_say_a_source_that_fails_is_the_sources_trouble() {
    let rust = RustBackend::new();
    let pdfium = require_pdfium_backend();
    let bytes = padded_pages(40);
    let opts = RenderOpts::dpi(72.0);
    let blue = Some([20, 120, 220, 255]);

    // A source that gives nothing: refused for its source by both engines, the
    // source's own words in the message, where PDFium itself says a format error.
    for backend in [&pdfium as &dyn RenderBackend, &rust] {
        let err = backend
            .open_source(FlakySource::over(&bytes, true, false), None)
            .err()
            .unwrap();
        assert_eq!(err.refusal(), Some(Refusal::Source), "{}", backend.name());
        assert!(err.to_string().contains("the disk went away"), "{err}");
    }
    let err = pdfium
        .open_source(FlakySource::over(&bytes, true, false), None)
        .err()
        .unwrap();
    let BackendError::Refused { error, .. } = &err else {
        unreachable!()
    };
    assert_eq!(
        error.downcast_ref::<PdfiumError>(),
        Some(&PdfiumError::Source {
            kind: std::io::ErrorKind::TimedOut,
            detail: "the disk went away".to_owned(),
        })
    );

    // A source that reads for the open and fails after: a page not yet read is an
    // error from both engines, never a blank page returned as drawn.
    let source = FlakySource::over(&bytes, false, false);
    let p_doc = pdfium.open_source(source.clone(), None).unwrap();
    assert_eq!(
        p_doc.render_page(3, &opts).unwrap().0.pixel_rgba8(36, 36),
        blue
    );
    let known = p_doc.page_geom(3).unwrap();
    source.go(true);
    let unread = |err: BackendError| {
        let BackendError::Backend(boxed) = &err else {
            panic!("{err}");
        };
        assert!(
            matches!(
                boxed.downcast_ref::<PdfiumError>(),
                Some(PdfiumError::Source { .. })
            ),
            "{err}"
        );
        err.to_string()
    };
    assert_eq!(
        unread(p_doc.render_page(20, &opts).err().unwrap()),
        "the document's source didn't read: the disk went away"
    );
    // The document isn't trusted after it, though the source reads again: what
    // PDFium kept of the read that failed isn't known. A geometry read before stays.
    source.go(false);
    unread(p_doc.render_page(21, &opts).err().unwrap());
    unread(p_doc.page_geom(22).err().unwrap());
    assert_eq!(p_doc.page_geom(3).unwrap(), known);
    drop(p_doc);

    let source = FlakySource::over(&bytes, false, false);
    let r_doc = rust.open_source(source.clone(), None).unwrap();
    source.go(true);
    let err = r_doc.render_page(20, &opts).err().unwrap();
    assert!(
        matches!(
            &err,
            BackendError::Raster(pdf_raster::Error::Unreadable(u))
                if u.why == pdf_raster::pdf_objects::Why::Source
        ),
        "{err}"
    );
    // The pure-Rust engine reads the page again once the source does.
    source.go(false);
    assert_eq!(
        r_doc.render_page(20, &opts).unwrap().0.pixel_rgba8(36, 36),
        blue
    );

    // A source that panics in a read under PDFium is a read that failed, not an
    // unwind through PDFium's callback (which aborts the process).
    let source = FlakySource::over(&bytes, false, true);
    let p_doc = pdfium.open_source(source.clone(), None).unwrap();
    source.go(true);
    assert_eq!(
        unread(p_doc.render_page(20, &opts).err().unwrap()),
        "the document's source didn't read: the source's read panicked"
    );
    source.go(false);
    drop(p_doc);
    // And the lock was given up by each failure: PDFium still opens and draws.
    let whole = pdfium.open_bytes(Arc::from(bytes), None).unwrap();
    assert_eq!(
        whole.render_page(20, &opts).unwrap().0.pixel_rgba8(36, 36),
        blue
    );
}

#[test]
#[ignore = "requires libpdfium (run via `just test-pdfium`)"]
fn pdfium_document_kept_in_a_thread_local_closes_as_its_thread_ends() {
    use std::cell::RefCell;

    thread_local! {
        static CACHE: RefCell<Vec<Box<dyn pdf_raster::RenderedDoc>>> =
            const { RefCell::new(Vec::new()) };
    }
    let pdfium = require_pdfium_backend();
    let bytes: Arc<[u8]> = Arc::from(padded_pages(2));
    let worker = {
        let bytes = bytes.clone();
        thread::spawn(move || {
            // The cache is the thread's first local, there before the thread's
            // first call into the adapter, so it's destroyed after any of the
            // adapter's or the lock's own: its documents close then.
            CACHE.with(|cache| assert!(cache.borrow().is_empty()));
            let doc = pdfium.open_bytes(bytes, None).unwrap();
            assert_eq!(doc.page_count(), 2);
            let source: Arc<dyn RenderSource> =
                Arc::new(Parts::new(vec![Arc::from(padded_pages(1))]));
            let from_source = pdfium.open_source(source, None).unwrap();
            CACHE.with(|cache| cache.borrow_mut().extend([doc, from_source]));
        })
    };
    worker.join().expect("the thread ended without a panic");
    // The lock is free after it, and PDFium still draws.
    let doc = pdfium.open_bytes(bytes, None).unwrap();
    let drawn = doc.render_page(0, &RenderOpts::dpi(72.0)).unwrap().0;
    assert_eq!(drawn.pixel_rgba8(36, 36), Some([20, 120, 220, 255]));
}
