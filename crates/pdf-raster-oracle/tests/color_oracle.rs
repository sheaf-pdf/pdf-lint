//! Live PDFium (`as of chromium/8152`) + `0–100` rubric oracle suite for `pdf-color`.

use pdf_color::pdf_objects::{Object, ObjectId, Resolve, Streams, dict};
use pdf_color::{EvalOptions, Fault, Limits, PreparedSpace, Spaces};
use pdf_fixtures::fixture::{self, Body, Xref};
use pdf_raster_oracle::rubric::{
    BaselineRow, CaseEvaluation, RgbaImage, RgbaImageExt, RubricProfile, evaluate_case,
    format_baseline_tsv, parse_baseline_tsv,
};
use pdf_raster_oracle::{RenderOptions, require_pdfium};
use pdf_structure::read::{Options, Reading};

/// Builds a 1-page PDF of `swatch_ops.len()` 16x16pt vertical color strips
/// (`width = 16 * N, height = 16`) alongside extra indirect objects from ID 5 onwards.
fn build_swatch_pdf(
    color_spaces: pdf_color::pdf_objects::Dict,
    extra_bodies: Vec<(u32, Body)>,
    swatch_ops: &[String],
) -> Vec<u8> {
    let w = (swatch_ops.len() * 16) as i64;
    let mut stream_text = String::new();
    for (i, op) in swatch_ops.iter().enumerate() {
        let x = i * 16;
        stream_text.push_str(&format!("q {op} {x} 0 16 16 re f Q\n"));
    }
    let content_bytes = stream_text.into_bytes();
    let mut bodies = vec![
        (
            1,
            Body::Object(Object::from(
                dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) },
            )),
        ),
        (
            2,
            Body::Object(Object::from(
                dict! { "Type" => Object::name("Pages"), "Kids" => vec![Object::reference(3)], "Count" => 1 },
            )),
        ),
        (
            3,
            Body::Object(Object::from(dict! {
                "Type" => Object::name("Page"),
                "Parent" => Object::reference(2),
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(w), Object::from(16)],
                "Contents" => Object::reference(4),
                "Resources" => dict! { "ColorSpace" => color_spaces },
            })),
        ),
        (
            4,
            Body::stream(
                dict! { "Length" => content_bytes.len() as i64 },
                &content_bytes,
            ),
        ),
    ];
    bodies.extend(extra_bodies);
    fixture::file(
        "1.7",
        &bodies,
        dict! { "Root" => Object::reference(1) },
        Xref::Table,
    )
}

/// Evaluates `(space_name, components)` swatches through `PreparedSpace` over
/// `reading` using `options`, painting a `16 * N x 16` [`RgbaImage`] to compare
/// directly against `libpdfium`.
fn render_swatches_via_pdf_color<'o, S>(
    reading: &'o Reading<S>,
    spaces: Spaces<'o>,
    options: &EvalOptions<'o>,
    swatches: &[(&[u8], &[f32])],
    sink: &mut Vec<(pdf_color::pdf_objects::fault::Place, Fault)>,
) -> RgbaImage
where
    &'o Reading<S>: Resolve<'o> + Streams,
{
    let w = (swatches.len() * 16) as u32;
    let mut img = RgbaImage::from_solid(w, 16, [255, 255, 255, 255]).unwrap();
    for (i, (cs_name, comps)) in swatches.iter().enumerate() {
        let space_val = spaces.get(cs_name).expect("named in test dict").value;
        let prep = PreparedSpace::prepare(reading, space_val, options, &Limits::DEFAULT, sink)
            .unwrap()
            .unwrap();
        let rgb = prep.eval(comps).unwrap().srgb8_over([255, 255, 255]);
        img.fill_rect((i * 16) as u32, 0, 16, 16, [rgb[0], rgb[1], rgb[2], 255]);
    }
    img
}

#[test]
#[ignore = "requires libpdfium (run via `just test-pdfium`)"]
fn pdfium_oracle_rubric_conforming_wild_and_iso_normative_suites() {
    let pdfium = require_pdfium();

    // --- Suite 1: ConformingCore (ISO == PDFium, 0 faults, >= 98.0 Imperceptible/Exact vs PDFium) ---
    let ps_tint = b"{ 0.6 mul exch 0.4 mul add dup 0.5 mul dup 1.0 exch sub }";
    let sampled_lut = vec![0_u8, 40, 80, 120, 180, 220, 255, 200, 100];
    let cs_core = dict! {
        "DG" => Object::name("DeviceGray"),
        "DR" => Object::name("DeviceRGB"),
        "CR" => vec![
            Object::name("CalRGB"),
            Object::from(dict! {
                "WhitePoint" => vec![Object::from(0.9642), Object::from(1.0), Object::from(0.8249)],
                "Gamma" => vec![Object::from(1.8), Object::from(1.8), Object::from(1.8)],
                "Matrix" => vec![
                    Object::from(0.4361), Object::from(0.2225), Object::from(0.0139),
                    Object::from(0.3851), Object::from(0.7169), Object::from(0.0971),
                    Object::from(0.1431), Object::from(0.0606), Object::from(0.7141),
                ],
            }),
        ],
        "LB" => vec![
            Object::name("Lab"),
            Object::from(dict! {
                "WhitePoint" => vec![Object::from(0.95047), Object::from(1.0), Object::from(1.08883)],
                "Range" => vec![Object::from(-100.0), Object::from(100.0), Object::from(-100.0), Object::from(100.0)],
            }),
        ],
        "IX" => vec![
            Object::name("Indexed"),
            Object::name("DeviceRGB"),
            Object::from(2),
            Object::bytes(sampled_lut),
        ],
        "SP" => vec![
            Object::name("Separation"),
            Object::name("BrandBlue"),
            Object::name("DeviceRGB"),
            Object::reference(5),
        ],
        "DN" => vec![
            Object::name("DeviceN"),
            Object::from(vec![Object::name("Teal"), Object::name("Amber")]),
            Object::name("DeviceRGB"),
            Object::reference(6),
        ],
        "SN" => vec![
            Object::name("Separation"),
            Object::name("None"),
            Object::name("DeviceRGB"),
            Object::reference(5),
        ],
    };
    let extra_core = vec![
        (
            5,
            Body::Object(Object::from(dict! {
                "FunctionType" => 2,
                "Domain" => vec![Object::from(0.0), Object::from(1.0)],
                "N" => 1.0,
                "C0" => vec![Object::from(1.0), Object::from(1.0), Object::from(1.0)],
                "C1" => vec![Object::from(0.1), Object::from(0.35), Object::from(0.85)],
            })),
        ),
        (
            6,
            Body::stream(
                dict! {
                    "FunctionType" => 4,
                    "Domain" => vec![Object::from(0.0), Object::from(1.0), Object::from(0.0), Object::from(1.0)],
                    "Range" => vec![
                        Object::from(0.0), Object::from(1.0),
                        Object::from(0.0), Object::from(1.0),
                        Object::from(0.0), Object::from(1.0),
                    ],
                    "Length" => ps_tint.len() as i64,
                },
                ps_tint,
            ),
        ),
    ];

    let core_ops = vec![
        "/DG cs 0.35 sc".to_string(),
        "/DR cs 0.2 0.65 0.9 sc".to_string(),
        "/CR cs 0.5 0.2 0.8 sc".to_string(),
        "/LB cs 50 40 -30 sc".to_string(),
        "/IX cs 1 sc".to_string(),
        "/SP cs 0.75 sc".to_string(),
        "/DN cs 0.7 0.3 sc".to_string(),
        "/SN cs 0.9 sc".to_string(),
    ];
    let core_swatches: &[(&[u8], &[f32])] = &[
        (b"DG", &[0.35]),
        (b"DR", &[0.2, 0.65, 0.9]),
        (b"CR", &[0.5, 0.2, 0.8]),
        (b"LB", &[50.0, 40.0, -30.0]),
        (b"IX", &[1.0]),
        (b"SP", &[0.75]),
        (b"DN", &[0.7, 0.3]),
        (b"SN", &[0.9]),
    ];

    let core_pdf = build_swatch_pdf(cs_core, extra_core, &core_ops);
    let core_ref = pdfium
        .render_page(
            &core_pdf,
            0,
            &RenderOptions::crisp_swatches((core_swatches.len() * 16) as u32, 16),
        )
        .unwrap();

    let reading = Reading::open(&core_pdf, &Options::DEFAULT).unwrap();
    let page_res = (&reading)
        .resolve(ObjectId::new(3, 0))
        .and_then(Object::as_dict)
        .and_then(|d| d.get(b"Resources"));
    let spaces = Spaces::read(&reading, page_res).unwrap().unwrap();

    let mut iso_faults = Vec::new();
    let mut pdfium_faults = Vec::new();
    let cand_iso = render_swatches_via_pdf_color(
        &reading,
        spaces,
        &EvalOptions::ISO,
        core_swatches,
        &mut iso_faults,
    );
    let cand_pdfium = render_swatches_via_pdf_color(
        &reading,
        spaces,
        &EvalOptions::PDFIUM,
        core_swatches,
        &mut pdfium_faults,
    );
    assert!(iso_faults.is_empty(), "{iso_faults:?}");

    let mut core_cfg = RubricProfile::ConformingCore.config();
    core_cfg.visual_pdfium_floor = 98.0;
    let core_outcome = evaluate_case(
        &core_cfg,
        &CaseEvaluation {
            candidate_pdfium: Some(&cand_pdfium),
            candidate_iso: Some(&cand_iso),
            pdfium_reference: Some(&core_ref),
            iso_reference: None,
            iso_fault_count: iso_faults.len(),
            write_lint_errors: 0,
        },
    )
    .unwrap();
    assert!(core_outcome.passed, "{core_outcome:?}");

    // --- Suite 2: PdfiumWild (`DeviceCMYK` unprofiled + `CalGray /Gamma 1.0` + `Separation /All`) ---
    let cs_wild = dict! {
        "CK" => Object::name("DeviceCMYK"),
        "CG" => vec![
            Object::name("CalGray"),
            Object::from(dict! {
                "WhitePoint" => vec![Object::from(0.95047), Object::from(1.0), Object::from(1.08883)],
                "Gamma" => 1.0,
            }),
        ],
        "SA" => vec![
            Object::name("Separation"),
            Object::name("All"),
            Object::name("DeviceRGB"),
            Object::from(dict! {
                "FunctionType" => 2,
                "Domain" => vec![Object::from(0.0), Object::from(1.0)],
                "N" => 1.0,
                "C0" => vec![Object::from(1.0), Object::from(1.0), Object::from(1.0)],
                "C1" => vec![Object::from(1.0), Object::from(0.0), Object::from(0.0)],
            }),
        ],
    };
    let wild_ops = vec![
        "/CK cs 1 0 0 0 sc".to_string(),
        "/CK cs 0 1 0 0 sc".to_string(),
        "/CK cs 0.5 0.3 0.2 0.1 sc".to_string(),
        "/CG cs 0.5 sc".to_string(),
        "/SA cs 0.7 sc".to_string(),
    ];
    let wild_swatches: &[(&[u8], &[f32])] = &[
        (b"CK", &[1.0, 0.0, 0.0, 0.0]),
        (b"CK", &[0.0, 1.0, 0.0, 0.0]),
        (b"CK", &[0.5, 0.3, 0.2, 0.1]),
        (b"CG", &[0.5]),
        (b"SA", &[0.7]),
    ];
    let wild_pdf = build_swatch_pdf(cs_wild, Vec::new(), &wild_ops);
    let wild_ref = pdfium
        .render_page(
            &wild_pdf,
            0,
            &RenderOptions::crisp_swatches((wild_swatches.len() * 16) as u32, 16),
        )
        .unwrap();
    let wild_reading = Reading::open(&wild_pdf, &Options::DEFAULT).unwrap();
    let wild_res = (&wild_reading)
        .resolve(ObjectId::new(3, 0))
        .and_then(Object::as_dict)
        .and_then(|d| d.get(b"Resources"));
    let wild_spaces = Spaces::read(&wild_reading, wild_res).unwrap().unwrap();
    let mut wild_faults = Vec::new();
    let wild_cand_iso = render_swatches_via_pdf_color(
        &wild_reading,
        wild_spaces,
        &EvalOptions::ISO,
        wild_swatches,
        &mut wild_faults,
    );
    let wild_cand_pdfium = render_swatches_via_pdf_color(
        &wild_reading,
        wild_spaces,
        &EvalOptions::PDFIUM,
        wild_swatches,
        &mut Vec::new(),
    );

    let mut wild_cfg = RubricProfile::PdfiumWild.config();
    wild_cfg.visual_pdfium_floor = 98.0;
    let wild_outcome = evaluate_case(
        &wild_cfg,
        &CaseEvaluation {
            candidate_pdfium: Some(&wild_cand_pdfium),
            candidate_iso: Some(&wild_cand_iso),
            pdfium_reference: Some(&wild_ref),
            iso_reference: None,
            iso_fault_count: wild_faults.len(),
            write_lint_errors: 0,
        },
    )
    .unwrap();
    assert!(wild_outcome.passed, "{wild_outcome:?}");

    // --- Suite 3: IsoNormative (analytic ISO reference for unprofiled ISO CMYK, CalGray, All) ---
    let mut iso_ref =
        RgbaImage::from_solid((wild_swatches.len() * 16) as u32, 16, [255, 255, 255, 255]).unwrap();
    for (i, px) in [
        [0, 255, 255, 255],
        [255, 0, 255, 255],
        [102, 153, 179, 255],
        [188, 188, 188, 255],
        [77, 77, 77, 255],
    ]
    .into_iter()
    .enumerate()
    {
        iso_ref.fill_rect((i * 16) as u32, 0, 16, 16, px);
    }
    let mut iso_cfg = RubricProfile::IsoNormative.config();
    iso_cfg.visual_iso_floor = 99.5;
    let iso_outcome = evaluate_case(
        &iso_cfg,
        &CaseEvaluation {
            candidate_pdfium: Some(&wild_cand_pdfium),
            candidate_iso: Some(&wild_cand_iso),
            pdfium_reference: Some(&wild_ref),
            iso_reference: Some(&iso_ref),
            iso_fault_count: wild_faults.len(),
            write_lint_errors: 0,
        },
    )
    .unwrap();
    assert!(iso_outcome.passed, "{iso_outcome:?}");

    let rows = vec![
        BaselineRow::from_visual(
            "color_core_swatches",
            RubricProfile::ConformingCore,
            core_outcome.visual_pdfium.as_ref().unwrap(),
        ),
        BaselineRow::from_visual(
            "color_pdfium_wild_swatches",
            RubricProfile::PdfiumWild,
            wild_outcome.visual_pdfium.as_ref().unwrap(),
        ),
        BaselineRow::from_visual(
            "color_iso_normative_swatches",
            RubricProfile::IsoNormative,
            iso_outcome.visual_iso.as_ref().unwrap(),
        ),
    ];
    let tsv = format_baseline_tsv(&rows);
    let reparsed = parse_baseline_tsv(&tsv).unwrap();
    assert_eq!(reparsed.len(), 3);
    for (got, rec) in rows.iter().zip(&reparsed) {
        assert!(got.compare_drift(rec, 0.05).within_tolerance);
    }
}

#[test]
#[ignore = "requires libpdfium (run via `just test-pdfium`)"]
fn pdfium_oracle_icc_swatch_strip_matches_chromium_8152() {
    let pdfium = require_pdfium();

    let adobe_rgb = moxcms::ColorProfile::new_adobe_rgb().encode().unwrap();
    let gray_18 = moxcms::ColorProfile::new_gray_with_gamma(1.8)
        .encode()
        .unwrap();

    let cs = dict! {
        "AR" => vec![Object::name("ICCBased"), Object::reference(5)],
        "GR" => vec![Object::name("ICCBased"), Object::reference(6)],
    };
    let extras = vec![
        (
            5,
            Body::stream(
                dict! { "N" => 3, "Alternate" => Object::name("DeviceRGB"), "Length" => adobe_rgb.len() as i64 },
                &adobe_rgb,
            ),
        ),
        (
            6,
            Body::stream(
                dict! { "N" => 1, "Alternate" => Object::name("DeviceGray"), "Length" => gray_18.len() as i64 },
                &gray_18,
            ),
        ),
    ];
    let ops = vec![
        "/AR cs 0.7 0.2 0.1 sc".to_string(),
        "/AR cs 0.1 0.75 0.3 sc".to_string(),
        "/AR cs 0.2 0.3 0.85 sc".to_string(),
        "/GR cs 0.5 sc".to_string(),
    ];
    let swatches: &[(&[u8], &[f32])] = &[
        (b"AR", &[0.7, 0.2, 0.1]),
        (b"AR", &[0.1, 0.75, 0.3]),
        (b"AR", &[0.2, 0.3, 0.85]),
        (b"GR", &[0.5]),
    ];
    let pdf = build_swatch_pdf(cs, extras, &ops);
    let pdfium_ref = pdfium
        .render_page(&pdf, 0, &RenderOptions::crisp_swatches(64, 16))
        .unwrap();

    let reading = Reading::open(&pdf, &Options::DEFAULT).unwrap();
    let page_res = (&reading)
        .resolve(ObjectId::new(3, 0))
        .and_then(Object::as_dict)
        .and_then(|d| d.get(b"Resources"));
    let spaces = Spaces::read(&reading, page_res).unwrap().unwrap();
    let mut iso_faults = Vec::new();
    let cand_iso = render_swatches_via_pdf_color(
        &reading,
        spaces,
        &EvalOptions::ISO,
        swatches,
        &mut iso_faults,
    );
    let cand_pdfium = render_swatches_via_pdf_color(
        &reading,
        spaces,
        &EvalOptions::PDFIUM,
        swatches,
        &mut Vec::new(),
    );

    let mut cfg = RubricProfile::ConformingCore.config();
    cfg.visual_pdfium_floor = 98.0;
    let outcome = evaluate_case(
        &cfg,
        &CaseEvaluation {
            candidate_pdfium: Some(&cand_pdfium),
            candidate_iso: Some(&cand_iso),
            pdfium_reference: Some(&pdfium_ref),
            iso_reference: None,
            iso_fault_count: iso_faults.len(),
            write_lint_errors: 0,
        },
    )
    .unwrap();
    assert!(outcome.passed, "{outcome:?}");
}

#[test]
#[ignore = "requires libpdfium (run via `just test-pdfium`)"]
fn pdfium_oracle_verifies_off_grid_cmyk_lab_whitepoint_short_c1_and_indirect_n() {
    use pdf_color::Function;
    use pdf_color::pdf_objects::pdf_tokenizer::Rules;

    let pdfium = require_pdfium();

    // 1. Off-grid CMYK `(0.33, 0.67, 0.12, 0.25)`, Lab with non-D65 `/WhitePoint [1.2 1.0 0.5]`,
    //    Type 2 with short `/C1 [0.8]` on 3-output `/Range`, and Type 2 with absent `/C1`:
    let cs = dict! {
        "CK" => Object::name("DeviceCMYK"),
        "LB" => vec![
            Object::name("Lab"),
            Object::from(dict! {
                "WhitePoint" => vec![Object::from(1.2), Object::from(1.0), Object::from(0.5)],
                "Range" => vec![Object::from(-100.0), Object::from(100.0), Object::from(-100.0), Object::from(100.0)],
            }),
        ],
        "S1" => vec![
            Object::name("Separation"),
            Object::name("ShortC1"),
            Object::name("DeviceRGB"),
            Object::reference(5),
        ],
        "S2" => vec![
            Object::name("Separation"),
            Object::name("AbsentC1"),
            Object::name("DeviceRGB"),
            Object::reference(6),
        ],
        "SI" => vec![
            Object::name("Separation"),
            Object::name("IndirectN"),
            Object::name("DeviceRGB"),
            Object::reference(7),
        ],
    };
    let extras = vec![
        (
            5,
            Body::Object(Object::from(dict! {
                "FunctionType" => 2,
                "Domain" => vec![Object::from(0.0), Object::from(1.0)],
                "Range" => vec![
                    Object::from(0.0), Object::from(1.0),
                    Object::from(0.0), Object::from(1.0),
                    Object::from(0.0), Object::from(1.0),
                ],
                "C0" => vec![Object::from(0.0), Object::from(0.0), Object::from(0.0)],
                "C1" => vec![Object::from(0.8)],
                "N" => 1.0,
            })),
        ),
        (
            6,
            Body::Object(Object::from(dict! {
                "FunctionType" => 2,
                "Domain" => vec![Object::from(0.0), Object::from(1.0)],
                "Range" => vec![
                    Object::from(0.0), Object::from(1.0),
                    Object::from(0.0), Object::from(1.0),
                    Object::from(0.0), Object::from(1.0),
                ],
                "C0" => vec![Object::from(0.2), Object::from(0.4), Object::from(0.6)],
                "N" => 1.0,
            })),
        ),
        (
            7,
            Body::Object(Object::from(dict! {
                "FunctionType" => 2,
                "Domain" => vec![Object::from(0.0), Object::from(1.0)],
                "C0" => vec![Object::from(0.0), Object::from(0.0), Object::from(0.0)],
                "C1" => vec![Object::from(0.9), Object::from(0.1), Object::from(0.2)],
                "N" => Object::reference(8),
            })),
        ),
        (8, Body::Object(Object::from(1.0))),
    ];
    let ops = vec![
        "/CK cs 0.33 0.67 0.12 0.25 sc".to_string(),
        "/LB cs 80 0 0 sc".to_string(),
        "/S1 cs 1.0 scn".to_string(),
        "/S2 cs 0.5 scn".to_string(),
        "/SI cs 1.0 scn".to_string(),
    ];
    let pdf = build_swatch_pdf(cs, extras, &ops);
    let ref_img = pdfium
        .render_page(&pdf, 0, &RenderOptions::crisp_swatches(80, 16))
        .unwrap();

    // Verify exact recorded `chromium/8152` reference pixels:
    assert_eq!(ref_img.pixel(8, 8), Some([139, 85, 126, 255]));
    assert_eq!(ref_img.pixel(24, 8), Some([200, 198, 198, 255]));
    assert_eq!(ref_img.pixel(40, 8), Some([204, 0, 0, 255]));
    assert_eq!(ref_img.pixel(56, 8), Some([153, 179, 204, 255]));
    // Indirect `/N 8 0 R` is refused by PDFium -> swatch 4 stays unpainted white!
    assert_eq!(ref_img.pixel(72, 8), Some([255, 255, 255, 255]));

    let reading = Reading::open(&pdf, &Options::DEFAULT).unwrap();
    let page_res = (&reading)
        .resolve(ObjectId::new(3, 0))
        .and_then(Object::as_dict)
        .and_then(|d| d.get(b"Resources"));
    let spaces = Spaces::read(&reading, page_res).unwrap().unwrap();

    // Indirect `/N` (`SI`, object 7) is refused consistently by `Function::read` and
    // `PreparedSpace::prepare` under `Rules::Pdfium` (matching `shape` and live PDFium), while
    // loading under `Rules::Iso`!
    let si_val = spaces.get(b"SI").unwrap().value;
    let fn7 = (&reading).resolve(ObjectId::new(7, 0)).unwrap();
    assert!(
        Function::read(&reading, fn7, Rules::Pdfium, &Limits::DEFAULT, &mut ())
            .unwrap()
            .is_none()
    );
    assert!(
        PreparedSpace::prepare(
            &reading,
            si_val,
            &EvalOptions::PDFIUM,
            &Limits::DEFAULT,
            &mut ()
        )
        .unwrap()
        .is_none()
    );
    assert!(
        Function::read(&reading, fn7, Rules::Iso, &Limits::DEFAULT, &mut ())
            .unwrap()
            .is_some()
    );
    assert!(
        PreparedSpace::prepare(
            &reading,
            si_val,
            &EvalOptions::ISO,
            &Limits::DEFAULT,
            &mut ()
        )
        .unwrap()
        .is_some()
    );

    let eval_srgb = |name: &[u8], comps: &[f32]| -> [u8; 3] {
        let sp = spaces.get(name).unwrap().value;
        let prep = PreparedSpace::prepare(
            &reading,
            sp,
            &EvalOptions::PDFIUM,
            &Limits::DEFAULT,
            &mut (),
        )
        .unwrap()
        .unwrap();
        prep.eval(comps).unwrap().srgb8_over([255, 255, 255])
    };
    let ck = eval_srgb(b"CK", &[0.33, 0.67, 0.12, 0.25]);
    assert!(
        ck[0].abs_diff(139) <= 5 && ck[1].abs_diff(85) <= 5 && ck[2].abs_diff(126) <= 5,
        "off-grid CMYK quadrilinear vs PDFium axis lookup: {ck:?}"
    );
    assert_eq!(eval_srgb(b"LB", &[80.0, 0.0, 0.0]), [200, 198, 198]);
    assert_eq!(eval_srgb(b"S1", &[1.0]), [204, 0, 0]);
    assert_eq!(eval_srgb(b"S2", &[0.5]), [153, 179, 204]);
}
