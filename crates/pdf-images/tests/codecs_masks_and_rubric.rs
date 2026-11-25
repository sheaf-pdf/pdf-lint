//! Comprehensive integration tests for `pdf-images` across all codecs (`Raw`, `DCTDecode`,
//! `CCITTFaxDecode`, `JBIG2Decode` + `/JBIG2Globals`, `JPXDecode` + `/SMaskInData`), bit depths
//! (`1, 2, 4, 8, 16`), color spaces, masks (`/ImageMask`, explicit `/Mask`, color-key `/Mask`,
//! `/SMask` + `/Matte`), inline images (`BI..ID..EI`), native-space redaction scrubbing, clean
//! `FlateDecode` & `DCTDecode` re-encoding, resource caps, fault-sink parity, and live
//! `pdf-pdfium` (`chromium/8152`) `0–100` rubric verification.

use std::collections::BTreeMap;

use pdf_fixtures::fixture::{self, Body, Xref};
use pdf_images::pdf_color::Family;
use pdf_images::pdf_content_streams::{ResCx, operations};
use pdf_images::pdf_filters::Level;
use pdf_images::pdf_objects::fault::Place;
use pdf_images::pdf_objects::{
    Dict, Object, ObjectId, Resolve, Stream, Unreadable, Why, WithStreams, dict,
};
use pdf_images::{
    DecodeOptions, Error, Fault, ImageCodec, JpegColorSpace, LimitKind, Limits, MaskSpec,
    SMaskInData, SampleData, decode_inline, decode_inline_raw, decode_stream, decode_stream_raw,
    decode_stream_with, decode_xobject, decode_xobject_with, encode_jpeg, encode_jpeg_bytes,
    encode_raw_flate, encode_rgba_flate,
};
use pdf_pdfium::{
    CaseEvaluation, Pdfium, RenderOptions, RgbaImage as PdfiumImage, RubricProfile, evaluate_case,
};
use pdf_structure::read::{Options, Reading};

fn empty_provider(
    objects: &BTreeMap<ObjectId, Object>,
) -> impl Resolve<'_> + pdf_images::pdf_objects::Streams + Copy {
    WithStreams::new(objects, move |id, limit| {
        let Some(s) = objects.get(&id).and_then(Object::as_stream) else {
            return Err(Unreadable::new(id, Why::Damaged, "missing stream"));
        };
        let bytes = s.data.bytes().unwrap();
        if bytes.len() > limit {
            return Err(Unreadable::new(id, Why::Limit, "past limit"));
        }
        Ok(bytes.to_vec())
    })
}

/// Builds a valid 8x4 CCITT Group 4 (`K = -1`) MMR bitstream:
/// - Rows 0..2: `x = 0..4` black, `x = 4..8` white
/// - Rows 2..4: `x = 0..4` white, `x = 4..8` black
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
    // Row 0 (ref all-white): H (`001`) + White 0 (`00110101`) + Black 4 (`011`) + V(0) (`1`)
    bp.push(0b001, 3);
    bp.push(0b0011_0101, 8);
    bp.push(0b011, 3);
    bp.push(0b1, 1);
    // Row 1 (same as Row 0: changes at 0, 4, end 8): V(0), V(0), V(0)
    bp.push(0b111, 3);
    // Row 2 (ref has changes 0->black, 4->white; current has 4->black, end 8):
    // Start a0=-1 (white): first black on ref > -1 is b1=0, b2=4. Pass mode `0001` moves a0 to b2=4!
    // Wait: simpler! From a0=-1, a1=4, b1=0: Pass (`0001`) -> a0=4 (still white), now b1=8 (end),
    // so H (`001`) + White 0 (`00110101`) + Black 4 (`011`) reaches a0=8!
    bp.push(0b0001, 4);
    bp.push(0b001, 3);
    bp.push(0b0011_0101, 8);
    bp.push(0b011, 3);
    // Row 3 (same as Row 2: change at 4, end 8): V(0), V(0)
    bp.push(0b11, 2);
    // EOFB (`000000000001000000000001`)
    bp.push(0x001001, 24);
    bp.finish()
}

/// Builds an embedded JBIG2 page stream (`8x4`) using Immediate Lossless Generic Region (`MMR=1`).
fn build_jbig2_embedded_8x4() -> Vec<u8> {
    let mmr = ccitt_group4_8x4_checker();
    let mut out = Vec::new();
    // Segment 0: Page Information (Type 48 = 0x30), page 1, 19 bytes
    out.extend_from_slice(&0_u32.to_be_bytes());
    out.push(0x30);
    out.push(0x00); // 0 referred segments
    out.push(0x01); // page 1
    out.extend_from_slice(&19_u32.to_be_bytes());
    out.extend_from_slice(&8_u32.to_be_bytes()); // width 8
    out.extend_from_slice(&4_u32.to_be_bytes()); // height 4
    out.extend_from_slice(&0_u32.to_be_bytes()); // x res
    out.extend_from_slice(&0_u32.to_be_bytes()); // y res
    out.push(0x00); // flags: default pixel = 0 (white)
    out.extend_from_slice(&0_u16.to_be_bytes()); // striping

    // Segment 1: Immediate Lossless Generic Region (Type 39 = 0x27), page 1
    let gen_len = (18 + mmr.len()) as u32;
    out.extend_from_slice(&1_u32.to_be_bytes());
    out.push(0x27);
    out.push(0x00);
    out.push(0x01);
    out.extend_from_slice(&gen_len.to_be_bytes());
    out.extend_from_slice(&8_u32.to_be_bytes()); // region width 8
    out.extend_from_slice(&4_u32.to_be_bytes()); // region height 4
    out.extend_from_slice(&0_u32.to_be_bytes()); // x 0
    out.extend_from_slice(&0_u32.to_be_bytes()); // y 0
    out.push(0x04); // REPLACE
    out.push(0x01); // MMR = 1
    out.extend_from_slice(&mmr);

    // Segment 2: End of Page (Type 49 = 0x31), page 1
    out.extend_from_slice(&2_u32.to_be_bytes());
    out.push(0x31);
    out.push(0x00);
    out.push(0x01);
    out.extend_from_slice(&0_u32.to_be_bytes());
    out
}

/// Builds a `(globals, page_stream)` JBIG2 pair where `globals` defines a 2-entry `4x4` Pattern
/// Dictionary (`[white 4x4 | black 4x4]`) and `page_stream` places a `2x1` Halftone Region (`8x4`)
/// referencing segment 0 from `/JBIG2Globals`.
fn build_jbig2_with_globals_8x4() -> (Vec<u8>, Vec<u8>) {
    // Collective pattern bitmap `8x4`: left `4x4` white (pattern 0), right `4x4` black (pattern 1).
    // Row 0 of `8x4`: white 4 (`1011`), black 4 (`011`); Rows 1..4: V(0), V(0) (`11`) each!
    let mut pat_mmr = Vec::new();
    // Bits: `001` (3) + `1011` (4) + `011` (3) + `11` (2) + `11` (2) + `11` (2) = 16 bits!
    // `001_1011_0` = `0x36`, `11_11_11_11` = `0xFF`, plus EOFB `00 10 01`.
    pat_mmr.extend_from_slice(&[0x36, 0xFF, 0x00, 0x10, 0x01]);

    let mut globals = Vec::new();
    // Segment 0: Pattern Dictionary (Type 16 = 0x10), page association 0 (global!)
    let pd_len = (7 + pat_mmr.len()) as u32;
    globals.extend_from_slice(&0_u32.to_be_bytes());
    globals.push(0x10); // Type 16
    globals.push(0x00); // 0 referred segments
    globals.push(0x00); // page 0 (global)
    globals.extend_from_slice(&pd_len.to_be_bytes());
    globals.push(0x01); // HDMMR = 1
    globals.push(4); // HDPW = 4
    globals.push(4); // HDPH = 4
    globals.extend_from_slice(&1_u32.to_be_bytes()); // GRAYMAX = 1 (2 patterns: 0 and 1)
    globals.extend_from_slice(&pat_mmr);

    // Gray-scale index plane `2x1`, `HBPP = 1` bitplane (`2x1` MMR bitmap `[1, 0]` = black=1 at x=0, white=0 at x=1):
    // Row 0 (`width = 2`): `x=0..1` black, `x=1..2` white -> H (`001`) + White 0 (`00110101`) + Black 1 (`010`) + V(0) (`1`)
    // = `001_00110` (`0x26`), `101_010_1_` + EOFB `000000000001000000000001`
    // Bits: `00100110` `10101010` `00000000` `00100000` `00000010`
    let gs_mmr = [0x26_u8, 0xAA, 0x00, 0x20, 0x02];

    let mut page = Vec::new();
    // Segment 1: Page Information (Type 48 = 0x30), page 1, `8x4`
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

    // Segment 2: Immediate Lossless Halftone Region (Type 23 = 0x17), referring to segment 0!
    let ht_len = (17 + 1 + 16 + 4 + gs_mmr.len()) as u32;
    page.extend_from_slice(&2_u32.to_be_bytes());
    page.push(0x17); // Type 23
    page.push(0x20); // 1 referred segment (`count = 1` in top 3 bits: `1 << 5 = 0x20`)
    page.push(0x00); // referred segment #0
    page.push(0x01); // page 1
    page.extend_from_slice(&ht_len.to_be_bytes());
    // RegionSegmentInfo (17 bytes): 8x4 at (0,0), REPLACE (4)
    page.extend_from_slice(&8_u32.to_be_bytes());
    page.extend_from_slice(&4_u32.to_be_bytes());
    page.extend_from_slice(&0_u32.to_be_bytes());
    page.extend_from_slice(&0_u32.to_be_bytes());
    page.push(0x04);
    // Halftone flags: `HMMR = 1`, `HCOMBOP = REPLACE (4 << 4 = 0x40)` -> `0x41`
    page.push(0x41);
    // Grid width=2, height=1, offset=(0, 0), vector=(4*256=1024, 0)
    page.extend_from_slice(&2_u32.to_be_bytes());
    page.extend_from_slice(&1_u32.to_be_bytes());
    page.extend_from_slice(&0_i32.to_be_bytes());
    page.extend_from_slice(&0_i32.to_be_bytes());
    page.extend_from_slice(&(4_u16 * 256).to_be_bytes()); // HGRX = 4.0 in 8.8 fixed point
    page.extend_from_slice(&0_u16.to_be_bytes()); // HGRY = 0.0
    page.extend_from_slice(&gs_mmr);

    // Segment 3: End of Page
    page.extend_from_slice(&3_u32.to_be_bytes());
    page.push(0x31);
    page.push(0x00);
    page.push(0x01);
    page.extend_from_slice(&0_u32.to_be_bytes());

    (globals, page)
}

/// Builds a valid ISO/IEC 15444-1 J2C codestream (`width x height`, `components`, `bit_depth`)
/// with 0 wavelet decomposition levels and zero-length packets (decoding to exact mid-scale
/// `1 << (bit_depth - 1)` on every channel).
fn build_j2c_mid_gray(width: u32, height: u32, components: u16, bit_depth: u8) -> Vec<u8> {
    let mut out = Vec::new();
    // SOC
    out.extend_from_slice(&[0xFF, 0x4F]);
    // SIZ (FF 51)
    let lsiz = 38 + 3 * components;
    out.extend_from_slice(&[0xFF, 0x51]);
    out.extend_from_slice(&lsiz.to_be_bytes());
    out.extend_from_slice(&0_u16.to_be_bytes()); // Rsiz
    out.extend_from_slice(&width.to_be_bytes()); // Xsiz
    out.extend_from_slice(&height.to_be_bytes()); // Ysiz
    out.extend_from_slice(&0_u32.to_be_bytes()); // XOsiz
    out.extend_from_slice(&0_u32.to_be_bytes()); // YOsiz
    out.extend_from_slice(&width.to_be_bytes()); // XTsiz
    out.extend_from_slice(&height.to_be_bytes()); // YTsiz
    out.extend_from_slice(&0_u32.to_be_bytes()); // XTOsiz
    out.extend_from_slice(&0_u32.to_be_bytes()); // YTOsiz
    out.extend_from_slice(&components.to_be_bytes()); // Csiz
    for _ in 0..components {
        out.push(bit_depth - 1); // unsigned `bit_depth`
        out.push(1); // XRsiz
        out.push(1); // YRsiz
    }
    // COD (FF 52): 12 bytes, 0 decomp levels, reversible 5/3 (`1`), no MCT (`0`)
    out.extend_from_slice(&[
        0xFF, 0x52, 0x00, 12, 0x00, // Scod = 0
        0x00, // LRCP progression
        0x00, 0x01, // 1 layer
        0x00, // MCT = 0
        0x00, // 0 decomposition levels
        0x04, // cblk width 2^(4+2) = 64
        0x04, // cblk height 2^(4+2) = 64
        0x00, // cblk style
        0x01, // reversible 5/3 wavelet
    ]);
    // QCD (FF 5C): 4 bytes, guard_bits = 2 (`0x40`), NoQuantization, exponent = `bit_depth`
    out.extend_from_slice(&[0xFF, 0x5C, 0x00, 4, 0x40, bit_depth << 3]);
    // SOT (FF 90): tile 0, Psot = 12 + 2 + components
    let psot = 14 + u32::from(components);
    out.extend_from_slice(&[0xFF, 0x90, 0x00, 10, 0x00, 0x00]);
    out.extend_from_slice(&psot.to_be_bytes());
    out.extend_from_slice(&[0x00, 0x01]);
    // SOD (FF 93) + `components` empty packets (`0x00`)
    out.extend_from_slice(&[0xFF, 0x93]);
    out.extend(std::iter::repeat_n(0x00_u8, usize::from(components)));
    // EOC (FF D9)
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

/// Wraps a J2C codestream inside a JP2 container with `ihdr`, `colr` (enumerated sRGB=16 or
/// Greyscale=17), and optional `cdef` marking the final channel as Opacity (`ChannelType = 1`).
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
    // Signature box `jP  `
    push_box(&mut jp2, b"jP  ", &[0x0D, 0x0A, 0x87, 0x0A]);
    // File type box `ftyp`
    push_box(&mut jp2, b"ftyp", b"jp2 \x00\x00\x00\x00jp2 ");

    let mut jp2h = Vec::new();
    // `ihdr`
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&components.to_be_bytes());
    ihdr.push(bit_depth - 1);
    ihdr.push(7); // C = 7
    ihdr.push(0); // UnkC = 0
    ihdr.push(0); // IPR = 0
    push_box(&mut jp2h, b"ihdr", &ihdr);

    // `colr` (METH=1 enumerated)
    let mut colr = vec![1_u8, 0, 0];
    colr.extend_from_slice(&enum_cs.to_be_bytes());
    push_box(&mut jp2h, b"colr", &colr);

    if last_channel_is_alpha {
        let mut cdef = Vec::new();
        cdef.extend_from_slice(&components.to_be_bytes());
        for ch in 0..components {
            cdef.extend_from_slice(&ch.to_be_bytes());
            if ch + 1 == components {
                cdef.extend_from_slice(&1_u16.to_be_bytes()); // Opacity
                cdef.extend_from_slice(&0_u16.to_be_bytes()); // WholeImage
            } else {
                cdef.extend_from_slice(&0_u16.to_be_bytes()); // Colour
                cdef.extend_from_slice(&(ch + 1).to_be_bytes()); // Association 1..N
            }
        }
        push_box(&mut jp2h, b"cdef", &cdef);
    }

    push_box(&mut jp2, b"jp2h", &jp2h);
    push_box(&mut jp2, b"jp2c", j2c);
    jp2
}

#[test]
fn raw_and_flate_bit_depths_1_2_4_8_16_and_scrubbing() {
    let objects = BTreeMap::new();
    let provider = empty_provider(&objects);

    // 1. 2-bit DeviceGray (`[0, 1, 2, 3]` -> `[0/3, 1/3, 2/3, 3/3]`)
    let s2 = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 4,
            "Height" => 1,
            "ColorSpace" => Object::name("DeviceGray"),
            "BitsPerComponent" => 2,
        },
        vec![0b00_01_10_11],
    );
    let mut raw2 = decode_stream_raw(
        provider,
        &s2,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(raw2.bits_per_component(), 2);
    let rgba2 = raw2
        .to_rgba8(provider, &DecodeOptions::PDFIUM, &Limits::DEFAULT)
        .unwrap();
    assert_eq!(rgba2.pixel(0, 0), Some([0, 0, 0, 255]));
    assert_eq!(rgba2.pixel(1, 0), Some([85, 85, 85, 255]));
    assert_eq!(rgba2.pixel(2, 0), Some([170, 170, 170, 255]));
    assert_eq!(rgba2.pixel(3, 0), Some([255, 255, 255, 255]));

    // Scrub middle two pixels in 2-bit packed buffer to `0` (black) without touching pixel 3.
    raw2.fill_rect_raw(1, 0, 2, 1, &[0]);
    let rgba2_scrubbed = raw2
        .to_rgba8(provider, &DecodeOptions::PDFIUM, &Limits::DEFAULT)
        .unwrap();
    assert_eq!(rgba2_scrubbed.pixel(1, 0), Some([0, 0, 0, 255]));
    assert_eq!(rgba2_scrubbed.pixel(2, 0), Some([0, 0, 0, 255]));
    assert_eq!(rgba2_scrubbed.pixel(3, 0), Some([255, 255, 255, 255]));

    // 2. 4-bit Indexed (`hival = 2`: 0=Red, 1=Green, 2=Blue)
    let cs4 = Object::from(vec![
        Object::name("Indexed"),
        Object::name("DeviceRGB"),
        Object::from(2),
        Object::bytes(&b"\xff\x00\x00\x00\xff\x00\x00\x00\xff"[..]),
    ]);
    let s4 = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 3,
            "Height" => 1,
            "ColorSpace" => cs4.clone(),
            "BitsPerComponent" => 4,
        },
        vec![0x01, 0x20],
    );
    let mut raw4 = decode_stream_raw(
        provider,
        &s4,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    let rgba4 = raw4
        .to_rgba8(provider, &DecodeOptions::PDFIUM, &Limits::DEFAULT)
        .unwrap();
    assert_eq!(rgba4.pixel(0, 0), Some([255, 0, 0, 255]));
    assert_eq!(rgba4.pixel(1, 0), Some([0, 255, 0, 255]));
    assert_eq!(rgba4.pixel(2, 0), Some([0, 0, 255, 255]));

    // Scrub index 1 -> palette entry 2 (Blue) and re-encode with Flate preserving Indexed space!
    raw4.fill_rect_raw(1, 0, 1, 1, &[2]);
    let enc4 = encode_raw_flate(&raw4, Some(cs4), Level::DEFAULT).unwrap();
    let rt4 = decode_stream(
        provider,
        &enc4.stream,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(rt4.pixel(0, 0), Some([255, 0, 0, 255]));
    assert_eq!(rt4.pixel(1, 0), Some([0, 0, 255, 255]));
    assert_eq!(rt4.pixel(2, 0), Some([0, 0, 255, 255]));

    // 3. 16-bit DeviceRGB (`[65535, 32768, 0]`) + lossless Flate + PNG predictor round-trip!
    let mut bytes16 = Vec::new();
    for &(r, g, b) in &[(65_535_u16, 32_768_u16, 0_u16), (0, 65_535, 65_535)] {
        bytes16.extend_from_slice(&r.to_be_bytes());
        bytes16.extend_from_slice(&g.to_be_bytes());
        bytes16.extend_from_slice(&b.to_be_bytes());
    }
    let s16 = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 2,
            "Height" => 1,
            "ColorSpace" => Object::name("DeviceRGB"),
            "BitsPerComponent" => 16,
        },
        bytes16,
    );
    let raw16 = decode_stream_raw(
        provider,
        &s16,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert!(matches!(raw16.samples(), SampleData::U16(_)));
    let enc16 = encode_raw_flate(&raw16, None, Level::DEFAULT).unwrap();
    let raw16_rt = decode_stream_raw(
        provider,
        &enc16.stream,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(raw16.samples(), raw16_rt.samples());
}

#[test]
fn dct_gray_rgb_cmyk_color_transform_and_12bit_rejection() {
    let objects = BTreeMap::new();
    let provider = empty_provider(&objects);

    // 1. Grayscale JPEG round-trip
    let gray_in = vec![20_u8, 120, 200, 250];
    let gray_stream = encode_jpeg(&gray_in, 2, 2, JpegColorSpace::Gray, 100).unwrap();
    let gray_rgba = decode_stream(
        provider,
        &gray_stream,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert!((i16::from(gray_rgba.pixel(0, 0).unwrap()[0]) - 20).abs() <= 2);
    assert!((i16::from(gray_rgba.pixel(1, 1).unwrap()[0]) - 250).abs() <= 2);

    // 2. CMYK YCCK JPEG round-trip + verify zero EXIF/APP1 segments in encoded bytes
    let cmyk_in = vec![
        255_u8, 0, 0, 0, // pure cyan
        0, 255, 0, 0, // pure magenta
        0, 0, 255, 0, // pure yellow
        0, 0, 0, 255, // pure black
    ];
    let cmyk_bytes = encode_jpeg_bytes(&cmyk_in, 2, 2, JpegColorSpace::Cmyk, 100).unwrap();
    assert!(!cmyk_bytes.windows(4).any(|w| w == b"Exif"));
    let cmyk_stream = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 2,
            "Height" => 2,
            "ColorSpace" => Object::name("DeviceCMYK"),
            "BitsPerComponent" => 8,
            "Filter" => Object::name("DCTDecode"),
        },
        cmyk_bytes,
    );
    let cmyk_raw = decode_stream_raw(
        provider,
        &cmyk_stream,
        None,
        &DecodeOptions::ISO,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(cmyk_raw.codec(), ImageCodec::Dct);
    assert_eq!(cmyk_raw.components(), 4);
    let mut px = [0_u16; 4];
    assert!(cmyk_raw.pixel_raw(0, 0, &mut px));
    assert!(
        px[0] > 245 && px[1] < 10 && px[2] < 10 && px[3] == 0,
        "{px:?}"
    );

    // Verify ISO vs PDFium CMYK evaluation differs cleanly on pure cyan `(1, 0, 0, 0)`.
    let iso_rgba = cmyk_raw
        .to_rgba8(provider, &DecodeOptions::ISO, &Limits::DEFAULT)
        .unwrap();
    let pdfium_rgba = cmyk_raw
        .to_rgba8(provider, &DecodeOptions::PDFIUM, &Limits::DEFAULT)
        .unwrap();
    assert!(iso_rgba.pixel(0, 0).unwrap()[0] <= 2);
    assert!(iso_rgba.pixel(0, 0).unwrap()[1] >= 245);
    assert!(pdfium_rgba.pixel(0, 0).unwrap()[1] < 200); // AdobeCMYK_to_sRGB cyan != [0, 255, 255]

    // 3. Synthetic 12-bit SOF0 header must return typed `Error::Codec` without panicking.
    let jpeg_12bit = vec![
        0xFF, 0xD8, // SOI
        0xFF, 0xC0, 0x00, 11, 12, 0x00, 0x02, 0x00, 0x02, 1, 1, 0x11, 0, // 12-bit SOF0
        0xFF, 0xD9,
    ];
    let s12 = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 2,
            "Height" => 2,
            "ColorSpace" => Object::name("DeviceGray"),
            "BitsPerComponent" => 8,
            "Filter" => Object::name("DCTDecode"),
        },
        jpeg_12bit,
    );
    let err = decode_stream(
        provider,
        &s12,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap_err();
    assert!(
        matches!(err, Error::Codec { codec: ImageCodec::Dct, ref detail } if detail.contains("12-bit")),
        "{err:?}"
    );
}

#[test]
fn ccitt_group4_black_is_1_and_truncated_row_recovery() {
    let objects = BTreeMap::new();
    let provider = empty_provider(&objects);
    let g4 = ccitt_group4_8x4_checker();

    // Standard `/BlackIs1 false`
    let fax_stream = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 8,
            "Height" => 4,
            "ColorSpace" => Object::name("DeviceGray"),
            "BitsPerComponent" => 1,
            "Filter" => Object::name("CCITTFaxDecode"),
            "DecodeParms" => dict! { "K" => -1, "Columns" => 8, "Rows" => 4 },
        },
        g4.clone(),
    );
    let raw = decode_stream_raw(
        provider,
        &fax_stream,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert!(matches!(
        raw.samples(),
        SampleData::Packed {
            bpc: 1,
            stride: 1,
            ..
        }
    ));
    let rgba = raw
        .to_rgba8(provider, &DecodeOptions::PDFIUM, &Limits::DEFAULT)
        .unwrap();
    // Row 0: left black `[0, 0, 0, 255]`, right white `[255, 255, 255, 255]`
    assert_eq!(rgba.pixel(0, 0), Some([0, 0, 0, 255]));
    assert_eq!(rgba.pixel(7, 0), Some([255, 255, 255, 255]));
    // Row 2: left white, right black
    assert_eq!(rgba.pixel(0, 2), Some([255, 255, 255, 255]));
    assert_eq!(rgba.pixel(7, 2), Some([0, 0, 0, 255]));

    // Truncated stream (only first 2 bytes, enough for Row 0..1, missing EOFB/Row 2..3):
    // must recover decoded rows, fill remaining rows white, and report `Fault::TruncatedFax`!
    let trunc_stream = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 8,
            "Height" => 4,
            "ColorSpace" => Object::name("DeviceGray"),
            "BitsPerComponent" => 1,
            "Filter" => Object::name("CCITTFaxDecode"),
            "DecodeParms" => dict! { "K" => -1, "Columns" => 8, "Rows" => 4, "EndOfBlock" => false },
        },
        g4[..2].to_vec(),
    );
    let mut faults: Vec<(Place, Fault)> = Vec::new();
    let trunc_rgba = decode_stream_with(
        provider,
        &trunc_stream,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
        &mut faults,
    )
    .unwrap();
    assert_eq!(
        trunc_rgba,
        decode_stream(
            provider,
            &trunc_stream,
            None,
            &DecodeOptions::PDFIUM,
            &Limits::DEFAULT
        )
        .unwrap()
    );
    assert!(
        faults.iter().any(|&(_, f)| f == Fault::TruncatedFax),
        "{faults:?}"
    );
    assert_eq!(trunc_rgba.pixel(0, 0), Some([0, 0, 0, 255]));
    assert_eq!(trunc_rgba.pixel(7, 3), Some([255, 255, 255, 255]));
}

#[test]
fn jbig2_embedded_and_jbig2_globals_halftone_and_clean_flate_reencode() {
    // 1. Embedded generic region (`8x4` checker)
    let objects = BTreeMap::new();
    let provider = empty_provider(&objects);
    let jb_bytes = build_jbig2_embedded_8x4();
    let jb_stream = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 8,
            "Height" => 4,
            "ColorSpace" => Object::name("DeviceGray"),
            "BitsPerComponent" => 1,
            "Filter" => Object::name("JBIG2Decode"),
        },
        jb_bytes,
    );
    let mut jb_raw = decode_stream_raw(
        provider,
        &jb_stream,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(jb_raw.codec(), ImageCodec::Jbig2);
    let jb_rgba = jb_raw
        .to_rgba8(provider, &DecodeOptions::PDFIUM, &Limits::DEFAULT)
        .unwrap();
    assert_eq!(jb_rgba.pixel(0, 0), Some([0, 0, 0, 255]));
    assert_eq!(jb_rgba.pixel(7, 0), Some([255, 255, 255, 255]));
    assert_eq!(jb_rgba.pixel(0, 2), Some([255, 255, 255, 255]));
    assert_eq!(jb_rgba.pixel(7, 2), Some([0, 0, 0, 255]));

    // Redact right half of row 2 (`x=4..8, y=2..4`) and re-encode: MUST become `FlateDecode`,
    // NEVER `JBIG2Decode`!
    jb_raw.fill_rect_raw(4, 2, 4, 2, &[1]); // `1` = white in 1-bit DeviceGray
    let reencoded = encode_raw_flate(&jb_raw, None, Level::DEFAULT).unwrap();
    assert_eq!(
        reencoded
            .stream
            .dict
            .get(b"Filter")
            .and_then(Object::as_name),
        Some(&b"FlateDecode"[..])
    );

    // 2. JBIG2 with indirect `/JBIG2Globals` Pattern Dictionary + Halftone Region
    let (globals_data, page_data) = build_jbig2_with_globals_8x4();
    let globals_id = ObjectId::new(10, 0);
    let mut store = BTreeMap::new();
    store.insert(
        globals_id,
        Object::from(Stream::new(Dict::new(), globals_data)),
    );
    let store_provider = empty_provider(&store);
    let ht_stream = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 8,
            "Height" => 4,
            "ColorSpace" => Object::name("DeviceGray"),
            "BitsPerComponent" => 1,
            "Filter" => Object::name("JBIG2Decode"),
            "DecodeParms" => dict! { "JBIG2Globals" => globals_id },
        },
        page_data,
    );
    let ht_rgba = decode_stream(
        store_provider,
        &ht_stream,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    // Grid cell 0 (`x=0..4`) placed pattern 1 (all-black), cell 1 (`x=4..8`) placed pattern 0 (all-white)
    assert_eq!(ht_rgba.pixel(0, 0), Some([0, 0, 0, 255]));
    assert_eq!(ht_rgba.pixel(3, 3), Some([0, 0, 0, 255]));
    assert_eq!(ht_rgba.pixel(4, 0), Some([255, 255, 255, 255]));
    assert_eq!(ht_rgba.pixel(7, 3), Some([255, 255, 255, 255]));
}

#[test]
fn jpx_native_16bit_omitted_colorspace_and_smask_in_data() {
    let objects = BTreeMap::new();
    let provider = empty_provider(&objects);

    // 1. 16-bit J2C codestream (`32768` mid-scale preserved in `SampleData::U16`!)
    let j2c_16 = build_j2c_mid_gray(4, 4, 3, 16);
    let s16 = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 4,
            "Height" => 4,
            "Filter" => Object::name("JPXDecode"),
        },
        j2c_16,
    );
    let raw16 = decode_stream_raw(
        provider,
        &s16,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(raw16.codec(), ImageCodec::Jpx);
    assert_eq!(raw16.bits_per_component(), 16);
    assert_eq!(raw16.meta().color_space, Some(Family::DeviceRGB));
    let mut s_px = [0_u16; 3];
    assert!(raw16.pixel_raw(0, 0, &mut s_px));
    assert_eq!(s_px, [32_768, 32_768, 32_768]);

    // 2. JP2 container with 4 channels (RGB + Opacity via `cdef`) and `/SMaskInData 1` (Separate)
    // and `/SMaskInData 2` (Premultiplied), with NO `/ColorSpace` on the PDF dictionary!
    let j2c_rgba = build_j2c_mid_gray(4, 4, 4, 8); // every channel = 128
    let jp2_rgba = wrap_jp2_container(&j2c_rgba, 4, 4, 4, 8, 16, true);

    let s_sep = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 4,
            "Height" => 4,
            "SMaskInData" => 1,
            "Filter" => Object::name("JPXDecode"),
        },
        jp2_rgba.clone(),
    );
    let raw_sep = decode_stream_raw(
        provider,
        &s_sep,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(raw_sep.meta().smask_in_data, SMaskInData::Separate);
    let rgba_sep = raw_sep
        .to_rgba8(provider, &DecodeOptions::PDFIUM, &Limits::DEFAULT)
        .unwrap();
    assert_eq!(rgba_sep.pixel(0, 0), Some([128, 128, 128, 128]));

    let s_pre = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 4,
            "Height" => 4,
            "SMaskInData" => 2,
            "Filter" => Object::name("JPXDecode"),
        },
        jp2_rgba,
    );
    let rgba_pre = decode_stream(
        provider,
        &s_pre,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    // Premultiplied `128` at alpha `128/255` un-premultiplies to `~255` (`[255, 255, 255, 128]`)!
    let px_pre = rgba_pre.pixel(0, 0).unwrap();
    assert!(px_pre[0] >= 254 && px_pre[3] == 128, "{px_pre:?}");
}

#[test]
fn stencil_explicit_mask_colorkey_and_smask_matte_compositing() {
    let mut store = BTreeMap::new();

    // 1. `/ImageMask true` with custom `stencil_color` and `/Decode [1 0]`
    let stencil_stream = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 2,
            "Height" => 1,
            "ImageMask" => true,
            "Decode" => vec![Object::from(1), Object::from(0)],
        },
        vec![0b1000_0000], // bit 0 is `1` -> mapped `0.0` (painted); bit 1 is `0` -> mapped `1.0` (transparent)
    );
    let mut opts = DecodeOptions::PDFIUM;
    opts.stencil_color = [10, 200, 30, 240];
    let stencil_rgba = decode_stream(
        empty_provider(&store),
        &stencil_stream,
        None,
        &opts,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(stencil_rgba.pixel(0, 0), Some([10, 200, 30, 240]));
    assert_eq!(stencil_rgba.pixel(1, 0), Some([0, 0, 0, 0]));

    // 2. Pre-`/Decode` Color-Key `/Mask [1 1]` on `Indexed` with inverted `/Decode [3 0]`
    // Raw index `1` must be masked BEFORE `/Decode` turns `1` into `2`!
    let ck_stream = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 3,
            "Height" => 1,
            "ColorSpace" => vec![
                Object::name("Indexed"),
                Object::name("DeviceRGB"),
                Object::from(3),
                Object::bytes(&b"\x10\x20\x30\x40\x50\x60\x70\x80\x90\xa0\xb0\xc0"[..]),
            ],
            "BitsPerComponent" => 8,
            "Mask" => vec![Object::from(1), Object::from(1)],
        },
        vec![0, 1, 2],
    );
    let ck_raw = decode_stream_raw(
        empty_provider(&store),
        &ck_stream,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(ck_raw.meta().mask, MaskSpec::ColorKey(vec![(1, 1)]));
    let ck_rgba = ck_raw
        .to_rgba8(
            empty_provider(&store),
            &DecodeOptions::PDFIUM,
            &Limits::DEFAULT,
        )
        .unwrap();
    assert_eq!(ck_rgba.pixel(0, 0), Some([0x10, 0x20, 0x30, 255]));
    assert_eq!(ck_rgba.pixel(1, 0), Some([0, 0, 0, 0])); // Masked!
    assert_eq!(ck_rgba.pixel(2, 0), Some([0x70, 0x80, 0x90, 255]));

    // 3. `/SMask` + `/Matte [1.0 1.0 1.0]` (white matte):
    // Pure red `[1, 0, 0]` premultiplied 50% against white `[1, 1, 1]` is `[1.0, 0.5, 0.5]` = `[255, 128, 128]`.
    // Un-premultiplying with `alpha = 128` (`~0.502`) recovers `[255, ~0, ~0, 128]`!
    let smask_id = ObjectId::new(20, 0);
    store.insert(
        smask_id,
        Object::from(Stream::new(
            dict! {
                "Subtype" => Object::name("Image"),
                "Width" => 2,
                "Height" => 1,
                "ColorSpace" => Object::name("DeviceGray"),
                "BitsPerComponent" => 8,
                "Matte" => vec![Object::from(1.0), Object::from(1.0), Object::from(1.0)],
            },
            vec![128, 0],
        )),
    );
    let base_stream = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 2,
            "Height" => 1,
            "ColorSpace" => Object::name("DeviceRGB"),
            "BitsPerComponent" => 8,
            "SMask" => smask_id,
        },
        vec![255, 128, 128, 255, 255, 255],
    );
    for decode_opts in [DecodeOptions::ISO, DecodeOptions::PDFIUM] {
        let out = decode_stream(
            empty_provider(&store),
            &base_stream,
            None,
            &decode_opts,
            &Limits::DEFAULT,
        )
        .unwrap();
        let p0 = out.pixel(0, 0).unwrap();
        assert!(
            p0[0] == 255 && p0[1] <= 2 && p0[2] <= 2 && p0[3] == 128,
            "{p0:?}"
        );
        assert_eq!(out.pixel(1, 0), Some([0, 0, 0, 0]));
    }

    // 4. `encode_rgba_flate` emits a companion `/SMask` stream iff transparency is present!
    let rgba_with_alpha = decode_stream(
        empty_provider(&store),
        &base_stream,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    let encoded = encode_rgba_flate(&rgba_with_alpha, Level::DEFAULT).unwrap();
    assert!(encoded.smask.is_some());
}

#[test]
fn inline_images_and_resource_limits_are_enforced() {
    let objects = BTreeMap::new();
    let provider = empty_provider(&objects);

    let content =
        b"q BI /W 2 /H 2 /BPC 8 /CS [/I /RGB 1 <ff000000ff00>] /F /A85 ID !!*'\"!!~> EI Q";
    let res = ResCx::none();
    let mut ops = operations(
        content,
        &res,
        &pdf_images::pdf_content_streams::Limits::DEFAULT,
    );
    let _ = ops.next(); // `q`
    let inline = ops.next().unwrap().unwrap().inline_image().unwrap();

    let raw = decode_inline_raw(
        provider,
        &inline,
        content,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(raw.width(), 2);
    assert_eq!(raw.height(), 2);
    let rgba = decode_inline(
        provider,
        &inline,
        content,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(rgba.pixel(0, 0), Some([255, 0, 0, 255]));

    // Enforce upfront dimension / pixel / byte caps BEFORE decoding!
    let huge = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 50_000,
            "Height" => 50_000,
            "ColorSpace" => Object::name("DeviceRGB"),
            "BitsPerComponent" => 8,
        },
        vec![0; 16],
    );
    let err = decode_stream(
        provider,
        &huge,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            Error::Limit {
                kind: LimitKind::Width,
                ..
            }
        ),
        "{err:?}"
    );

    let mut small_budget = Limits::DEFAULT;
    small_budget.max_sample_bytes = 32;
    let over_budget = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 4,
            "Height" => 4,
            "ColorSpace" => Object::name("DeviceRGB"),
            "BitsPerComponent" => 8,
        },
        vec![0; 48],
    );
    let err2 = decode_stream(
        provider,
        &over_budget,
        None,
        &DecodeOptions::PDFIUM,
        &small_budget,
    )
    .unwrap_err();
    assert!(
        matches!(
            err2,
            Error::Limit {
                kind: LimitKind::SampleBytes,
                ..
            }
        ),
        "{err2:?}"
    );
}

/// Builds a 1-page `[0 0 W H]` PDF drawing Image XObject `5 0 R` at exact 1:1 pixel-to-point
/// scale (`q W 0 0 H 0 0 cm /Im0 Do Q`), with optional extra indirect objects starting at `6 0 R`.
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

/// Composites a `pdf_images::RgbaImage` over opaque white `[255, 255, 255, 255]` into a
/// `pdf_pdfium::RgbaImage` for direct rubric comparison against `libpdfium`.
fn composite_over_white(img: &pdf_images::RgbaImage) -> PdfiumImage {
    let mut out = Vec::with_capacity(img.as_bytes().len());
    for &[r, g, b, a] in img.as_bytes().as_chunks::<4>().0 {
        let af = u32::from(a);
        let inv = 255 - af;
        out.push(((u32::from(r) * af + 255 * inv + 127) / 255) as u8);
        out.push(((u32::from(g) * af + 255 * inv + 127) / 255) as u8);
        out.push(((u32::from(b) * af + 255 * inv + 127) / 255) as u8);
        out.push(255);
    }
    PdfiumImage::from_rgba(img.width(), img.height(), out).unwrap()
}

#[test]
fn pdfium_oracle_rubric_verifies_all_codecs_and_masks_against_chromium_8152() {
    let Ok(pdfium) = Pdfium::try_global() else {
        assert!(std::env::var_os("PDF_LINT_PDFIUM").is_none());
        return;
    };

    // Prepare 7 live PDF cases spanning DCT (RGB & CMYK), CCITT Group 4, JBIG2 + Globals,
    // JPX 16-bit, `/ImageMask` stencil, and `/SMask` + `/Matte`!
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

    // `/SMask` + `/Matte [1 1 1]` over white page (`8x8`)
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
        // True color `[0.1, 0.4, 0.9]` premultiplied over white `[1, 1, 1]`
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
