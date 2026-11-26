//! Live PDFium (`as of chromium/8152`) + `0–100` rubric oracle suite for `pdf-images`.

use pdf_fixtures::fixture::{self, Body, Xref};
use pdf_images::pdf_objects::fault::Place;
use pdf_images::pdf_objects::{Dict, Object, ObjectId, Stream, dict};
use pdf_images::{
    DecodeOptions, Fault, JpegColorSpace, Limits, decode_xobject, decode_xobject_with, encode_jpeg,
};
use pdf_raster_oracle::rubric::{
    CaseEvaluation, RgbaImage as OracleRgbaImage, RubricProfile, evaluate_case,
};
use pdf_raster_oracle::{RenderOptions, require_pdfium};
use pdf_structure::read::{Options, Reading};

fn ccitt_group4_8x4_checker() -> Vec<u8> {
    struct BitPack {
        out: Vec<u8>,
        acc: u32,
        bits: u8,
    }
    impl BitPack {
        fn push(&mut self, val: u32, len: u8) {
            self.acc = (self.acc << len) | (val & ((1 << len) - 1));
            self.bits += len;
            while self.bits >= 8 {
                self.bits -= 8;
                self.out.push(((self.acc >> self.bits) & 0xFF) as u8);
            }
        }
        fn finish(mut self) -> Vec<u8> {
            if self.bits > 0 {
                self.out.push(((self.acc << (8 - self.bits)) & 0xFF) as u8);
            }
            self.out
        }
    }

    let mut bp = BitPack {
        out: Vec::new(),
        acc: 0,
        bits: 0,
    };
    bp.push(0b001, 3);
    bp.push(0b0011_0101, 8);
    bp.push(0b011, 3);
    bp.push(0b1, 1);
    bp.push(0b111, 3);
    bp.push(0b0001, 4);
    bp.push(0b001, 3);
    bp.push(0b0011_0101, 8);
    bp.push(0b011, 3);
    bp.push(0b11, 2);
    bp.push(0x001001, 24);
    bp.finish()
}

fn build_jbig2_with_globals_8x4() -> (Vec<u8>, Vec<u8>) {
    let mut pat_mmr = Vec::new();
    pat_mmr.extend_from_slice(&[0x36, 0xFF, 0x00, 0x10, 0x01]);

    let mut globals = Vec::new();
    let pd_len = (7 + pat_mmr.len()) as u32;
    globals.extend_from_slice(&0_u32.to_be_bytes());
    globals.push(0x10);
    globals.push(0x00);
    globals.push(0x00);
    globals.extend_from_slice(&pd_len.to_be_bytes());
    globals.push(0x01);
    globals.push(4);
    globals.push(4);
    globals.extend_from_slice(&1_u32.to_be_bytes());
    globals.extend_from_slice(&pat_mmr);

    let gs_mmr = [0x26_u8, 0xAA, 0x00, 0x20, 0x02];

    let mut page = Vec::new();
    page.extend_from_slice(&1_u32.to_be_bytes());
    page.push(0x30);
    page.push(0x00);
    page.push(0x01);
    page.extend_from_slice(&19_u32.to_be_bytes());
    page.extend_from_slice(&8_u32.to_be_bytes());
    page.extend_from_slice(&4_u32.to_be_bytes());
    page.extend_from_slice(&0_u32.to_be_bytes());
    page.extend_from_slice(&0_u32.to_be_bytes());
    page.push(0x00);
    page.extend_from_slice(&0_u16.to_be_bytes());

    let ht_len = (17 + 1 + 16 + 4 + gs_mmr.len()) as u32;
    page.extend_from_slice(&2_u32.to_be_bytes());
    page.push(0x17);
    page.push(0x20);
    page.push(0x00);
    page.push(0x01);
    page.extend_from_slice(&ht_len.to_be_bytes());
    page.extend_from_slice(&8_u32.to_be_bytes());
    page.extend_from_slice(&4_u32.to_be_bytes());
    page.extend_from_slice(&0_u32.to_be_bytes());
    page.extend_from_slice(&0_u32.to_be_bytes());
    page.push(0x04);
    page.push(0x41);
    page.extend_from_slice(&2_u32.to_be_bytes());
    page.extend_from_slice(&1_u32.to_be_bytes());
    page.extend_from_slice(&0_i32.to_be_bytes());
    page.extend_from_slice(&0_i32.to_be_bytes());
    page.extend_from_slice(&(4_u16 * 256).to_be_bytes());
    page.extend_from_slice(&0_u16.to_be_bytes());
    page.extend_from_slice(&gs_mmr);

    page.extend_from_slice(&3_u32.to_be_bytes());
    page.push(0x31);
    page.push(0x00);
    page.push(0x01);
    page.extend_from_slice(&0_u32.to_be_bytes());

    (globals, page)
}

fn build_j2c_mid_gray(width: u32, height: u32, components: u16, bit_depth: u8) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0xFF, 0x4F]);
    let lsiz = 38 + 3 * components;
    out.extend_from_slice(&[0xFF, 0x51]);
    out.extend_from_slice(&lsiz.to_be_bytes());
    out.extend_from_slice(&0_u16.to_be_bytes());
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&0_u32.to_be_bytes());
    out.extend_from_slice(&0_u32.to_be_bytes());
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&0_u32.to_be_bytes());
    out.extend_from_slice(&0_u32.to_be_bytes());
    out.extend_from_slice(&components.to_be_bytes());
    for _ in 0..components {
        out.push(bit_depth - 1);
        out.push(1);
        out.push(1);
    }
    out.extend_from_slice(&[
        0xFF, 0x52, 0x00, 12, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x04, 0x04, 0x00, 0x01,
    ]);
    out.extend_from_slice(&[0xFF, 0x5C, 0x00, 4, 0x40, bit_depth << 3]);
    let psot = 14 + u32::from(components);
    out.extend_from_slice(&[0xFF, 0x90, 0x00, 10, 0x00, 0x00]);
    out.extend_from_slice(&psot.to_be_bytes());
    out.extend_from_slice(&[0x00, 0x01]);
    out.extend_from_slice(&[0xFF, 0x93]);
    out.extend(std::iter::repeat_n(0x00_u8, usize::from(components)));
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

fn wrap_jp2_container(
    j2c: &[u8],
    width: u32,
    height: u32,
    components: u16,
    bit_depth: u8,
    enum_cs: u32,
    last_channel_is_alpha: bool,
) -> Vec<u8> {
    fn push_box(dst: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
        let len = (8 + payload.len()) as u32;
        dst.extend_from_slice(&len.to_be_bytes());
        dst.extend_from_slice(tag);
        dst.extend_from_slice(payload);
    }

    let mut jp2 = Vec::new();
    push_box(&mut jp2, b"jP  ", &[0x0D, 0x0A, 0x87, 0x0A]);
    push_box(&mut jp2, b"ftyp", b"jp2 \x00\x00\x00\x00jp2 ");

    let mut jp2h = Vec::new();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&components.to_be_bytes());
    ihdr.push(bit_depth - 1);
    ihdr.push(7);
    ihdr.push(0);
    ihdr.push(0);
    push_box(&mut jp2h, b"ihdr", &ihdr);

    let mut colr = vec![1_u8, 0, 0];
    colr.extend_from_slice(&enum_cs.to_be_bytes());
    push_box(&mut jp2h, b"colr", &colr);

    if last_channel_is_alpha {
        let mut cdef = Vec::new();
        cdef.extend_from_slice(&components.to_be_bytes());
        for ch in 0..components {
            cdef.extend_from_slice(&ch.to_be_bytes());
            if ch + 1 == components {
                cdef.extend_from_slice(&1_u16.to_be_bytes());
                cdef.extend_from_slice(&0_u16.to_be_bytes());
            } else {
                cdef.extend_from_slice(&0_u16.to_be_bytes());
                cdef.extend_from_slice(&(ch + 1).to_be_bytes());
            }
        }
        push_box(&mut jp2h, b"cdef", &cdef);
    }

    push_box(&mut jp2, b"jp2h", &jp2h);
    push_box(&mut jp2, b"jp2c", j2c);
    jp2
}

fn build_image_page_pdf(w: u32, h: u32, image_stream: Stream, extra_objs: Vec<Stream>) -> Vec<u8> {
    let content = format!("q {w} 0 0 {h} 0 0 cm /Im0 Do Q").into_bytes();
    let resources = dict! {
        "XObject" => dict! { "Im0" => Object::reference(5) },
    };
    let page = dict! {
        "Type" => Object::name("Page"),
        "Parent" => Object::reference(2),
        "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(i64::from(w)), Object::from(i64::from(h))],
        "Contents" => Object::reference(4),
        "Resources" => resources,
    };
    let mut img_dict = image_stream.dict;
    let img_bytes = image_stream.data.bytes().unwrap().to_vec();
    img_dict.set("Length", img_bytes.len() as i64);

    let mut bodies = vec![
        (
            1,
            Body::Object(Object::from(dict! {
                "Type" => Object::name("Catalog"),
                "Pages" => Object::reference(2),
            })),
        ),
        (
            2,
            Body::Object(Object::from(dict! {
                "Type" => Object::name("Pages"),
                "Kids" => vec![Object::reference(3)],
                "Count" => 1,
            })),
        ),
        (3, Body::Object(Object::from(page))),
        (
            4,
            Body::stream(dict! { "Length" => content.len() as i64 }, &content),
        ),
        (5, Body::stream(img_dict, &img_bytes)),
    ];
    for (idx, s) in extra_objs.into_iter().enumerate() {
        let mut sd = s.dict;
        let sb = s.data.bytes().unwrap().to_vec();
        sd.set("Length", sb.len() as i64);
        bodies.push((6 + idx as u32, Body::stream(sd, &sb)));
    }
    fixture::file(
        "1.7",
        &bodies,
        dict! { "Root" => Object::reference(1) },
        Xref::Table,
    )
}

fn composite_over_white(img: &pdf_images::RgbaImage) -> OracleRgbaImage {
    let mut out = Vec::with_capacity(img.as_bytes().len());
    for &[r, g, b, a] in img.as_bytes().as_chunks::<4>().0 {
        let af = u32::from(a);
        let inv = 255 - af;
        out.push(((u32::from(r) * af + 255 * inv + 127) / 255) as u8);
        out.push(((u32::from(g) * af + 255 * inv + 127) / 255) as u8);
        out.push(((u32::from(b) * af + 255 * inv + 127) / 255) as u8);
        out.push(255);
    }
    OracleRgbaImage::from_rgba(img.width(), img.height(), out).unwrap()
}

#[test]
#[ignore = "requires libpdfium (run via `just test-pdfium`)"]
fn pdfium_oracle_rubric_verifies_all_codecs_and_masks_against_chromium_8152() {
    let pdfium = require_pdfium();

    let mut rgb_grad = Vec::with_capacity(16 * 16 * 3);
    for y in 0..16_u8 {
        for x in 0..16_u8 {
            rgb_grad.extend_from_slice(&[x * 16, y * 16, 200 - x * 8]);
        }
    }
    let dct_rgb = encode_jpeg(&rgb_grad, 16, 16, JpegColorSpace::Rgb, 95).unwrap();

    let mut cmyk_blocks = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16_u8 {
        for x in 0..16_u8 {
            let cmyk = match (x >= 8, y >= 8) {
                (false, false) => [220, 20, 10, 0],
                (true, false) => [10, 210, 30, 0],
                (false, true) => [0, 30, 230, 10],
                (true, true) => [80, 60, 40, 120],
            };
            cmyk_blocks.extend_from_slice(&cmyk);
        }
    }
    let dct_cmyk = encode_jpeg(&cmyk_blocks, 16, 16, JpegColorSpace::Cmyk, 95).unwrap();

    let ccitt_stream = Stream::new(
        dict! {
            "Type" => Object::name("XObject"),
            "Subtype" => Object::name("Image"),
            "Width" => 8,
            "Height" => 4,
            "ColorSpace" => Object::name("DeviceGray"),
            "BitsPerComponent" => 1,
            "Filter" => Object::name("CCITTFaxDecode"),
            "DecodeParms" => dict! { "K" => -1, "Columns" => 8, "Rows" => 4 },
        },
        ccitt_group4_8x4_checker(),
    );

    let (jb_globals, jb_page) = build_jbig2_with_globals_8x4();
    let jbig2_stream = Stream::new(
        dict! {
            "Type" => Object::name("XObject"),
            "Subtype" => Object::name("Image"),
            "Width" => 8,
            "Height" => 4,
            "ColorSpace" => Object::name("DeviceGray"),
            "BitsPerComponent" => 1,
            "Filter" => Object::name("JBIG2Decode"),
            "DecodeParms" => dict! { "JBIG2Globals" => Object::reference(6) },
        },
        jb_page,
    );

    let jpx_stream = Stream::new(
        dict! {
            "Type" => Object::name("XObject"),
            "Subtype" => Object::name("Image"),
            "Width" => 8,
            "Height" => 8,
            "Filter" => Object::name("JPXDecode"),
        },
        wrap_jp2_container(&build_j2c_mid_gray(8, 8, 3, 16), 8, 8, 3, 16, 16, false),
    );

    let smask_obj = Stream::new(
        dict! {
            "Type" => Object::name("XObject"),
            "Subtype" => Object::name("Image"),
            "Width" => 8,
            "Height" => 8,
            "ColorSpace" => Object::name("DeviceGray"),
            "BitsPerComponent" => 8,
            "Matte" => vec![Object::from(1.0), Object::from(1.0), Object::from(1.0)],
        },
        (0..64_u8).map(|i| i.saturating_mul(4)).collect(),
    );
    let mut smask_base_rgb = Vec::with_capacity(64 * 3);
    for i in 0..64_u8 {
        let a = f32::from(i.saturating_mul(4)) / 255.0;
        let r = ((1.0 * (1.0 - a) + 0.1 * a) * 255.0).round() as u8;
        let g = ((1.0 * (1.0 - a) + 0.4 * a) * 255.0).round() as u8;
        let b = ((1.0 * (1.0 - a) + 0.9 * a) * 255.0).round() as u8;
        smask_base_rgb.extend_from_slice(&[r, g, b]);
    }
    let smask_base = Stream::new(
        dict! {
            "Type" => Object::name("XObject"),
            "Subtype" => Object::name("Image"),
            "Width" => 8,
            "Height" => 8,
            "ColorSpace" => Object::name("DeviceRGB"),
            "BitsPerComponent" => 8,
            "SMask" => Object::reference(6),
        },
        smask_base_rgb,
    );

    let cases = vec![
        (
            "dct_rgb_16x16",
            16,
            16,
            dct_rgb,
            Vec::new(),
            RubricProfile::ConformingCore,
        ),
        (
            "dct_cmyk_16x16",
            16,
            16,
            dct_cmyk,
            Vec::new(),
            RubricProfile::PdfiumWild,
        ),
        (
            "ccitt_g4_8x4",
            8,
            4,
            ccitt_stream,
            Vec::new(),
            RubricProfile::ConformingCore,
        ),
        (
            "jbig2_globals_8x4",
            8,
            4,
            jbig2_stream,
            vec![Stream::new(Dict::new(), jb_globals)],
            RubricProfile::ConformingCore,
        ),
        (
            "jpx_16bit_8x8",
            8,
            8,
            jpx_stream,
            Vec::new(),
            RubricProfile::ConformingCore,
        ),
        (
            "smask_matte_8x8",
            8,
            8,
            smask_base,
            vec![smask_obj],
            RubricProfile::ConformingCore,
        ),
    ];

    for (name, w, h, img_stream, extra, profile) in cases {
        let pdf_bytes = build_image_page_pdf(w, h, img_stream, extra);
        let pdfium_ref = pdfium
            .render_page(&pdf_bytes, 0, &RenderOptions::crisp_swatches(w, h))
            .unwrap();

        let reading = Reading::open(&pdf_bytes, &Options::DEFAULT).unwrap();
        let xobj_id = ObjectId::new(5, 0);
        let mut faults: Vec<(Place, Fault)> = Vec::new();
        let cand_pdfium = composite_over_white(
            &decode_xobject_with(
                &reading,
                xobj_id,
                None,
                &DecodeOptions::PDFIUM,
                &Limits::DEFAULT,
                &mut faults,
            )
            .unwrap(),
        );
        let cand_iso = composite_over_white(
            &decode_xobject(
                &reading,
                xobj_id,
                None,
                &DecodeOptions::ISO,
                &Limits::DEFAULT,
            )
            .unwrap(),
        );

        let mut cfg = profile.config();
        cfg.visual_pdfium_floor = 98.0;
        if profile == RubricProfile::ConformingCore {
            cfg.visual_iso_floor = 98.0;
        }
        let outcome = evaluate_case(
            &cfg,
            &CaseEvaluation {
                candidate_pdfium: Some(&cand_pdfium),
                candidate_iso: Some(&cand_iso),
                pdfium_reference: Some(&pdfium_ref),
                iso_reference: (profile == RubricProfile::ConformingCore).then_some(&pdfium_ref),
                iso_fault_count: faults.len(),
                write_lint_errors: 0,
            },
        )
        .unwrap();
        assert!(outcome.passed, "case `{name}` failed rubric: {outcome:?}");
    }
}
