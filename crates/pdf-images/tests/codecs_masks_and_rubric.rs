//! Comprehensive integration tests for `pdf-images` across all codecs (`Raw`, `DCTDecode`,
//! `CCITTFaxDecode`, `JBIG2Decode` + `/JBIG2Globals`, `JPXDecode` + `/SMaskInData`), bit depths
//! (`1, 2, 4, 8, 16`), color spaces, masks (`/ImageMask`, explicit `/Mask`, color-key `/Mask`,
//! `/SMask` + `/Matte`), inline images (`BI..ID..EI`), native-space redaction scrubbing, clean
//! `FlateDecode` & `DCTDecode` re-encoding, resource caps, and fault-sink parity.

use std::collections::BTreeMap;

use pdf_images::pdf_color::Family;
use pdf_images::pdf_content_streams::{ResCx, operations};
use pdf_images::pdf_filters::Level;
use pdf_images::pdf_objects::fault::Place;
use pdf_images::pdf_objects::{
    Dict, Object, ObjectId, Resolve, Stream, Unreadable, Why, WithStreams, dict,
};
use pdf_images::{
    DecodeOptions, EncodeError, Error, Fault, ImageCodec, JpegColorSpace, LimitKind, Limits,
    MaskSpec, SMaskInData, SampleData, ScrubError, decode_inline, decode_inline_raw, decode_stream,
    decode_stream_raw, decode_stream_with, decode_xobject, encode_jpeg, encode_jpeg_bytes,
    encode_raw_flate, encode_rgba_flate,
};

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
    raw2.fill_rect_raw(1, 0, 2, 1, &[0]).unwrap();
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
    raw4.fill_rect_raw(1, 0, 1, 1, &[2]).unwrap();
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
    jb_raw.fill_rect_raw(4, 2, 4, 2, &[1]).unwrap(); // `1` = white in 1-bit DeviceGray
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

#[test]
fn raw_image_with_more_than_32_components_decodes_without_panicking() {
    // 33-component `DeviceN` raw image with a Type 4 PostScript tint transform
    // that discards 32 inputs and maps the first component onto `[c0, 0.5, 1.0 - c0]`.
    let mut ps_body = String::from("{ ");
    for _ in 0..32 {
        ps_body.push_str("pop ");
    }
    ps_body.push_str("dup 0.5 exch 1.0 exch sub }");
    let ps_bytes = ps_body.into_bytes();

    let mut domain = Vec::with_capacity(66);
    for _ in 0..33 {
        domain.push(Object::from(0.0));
        domain.push(Object::from(1.0));
    }
    let names: Vec<Object> = (0..33).map(|i| Object::name(format!("Spot{i}"))).collect();

    let mut objects = BTreeMap::new();
    let tint_id = ObjectId::new(10, 0);
    objects.insert(
        tint_id,
        Object::from(Stream::new(
            dict! {
                "FunctionType" => 4,
                "Domain" => domain,
                "Range" => vec![
                    Object::from(0.0), Object::from(1.0),
                    Object::from(0.0), Object::from(1.0),
                    Object::from(0.0), Object::from(1.0),
                ],
            },
            ps_bytes,
        )),
    );

    let xobj_id = ObjectId::new(11, 0);
    // 2x1 image, 8 bpc, 33 components = 66 sample bytes:
    // pixel (0, 0) has Spot0 = 255 (`1.0` -> `[255, 128, 0]`),
    // pixel (1, 0) has Spot0 = 0 (`0.0` -> `[0, 128, 255]`).
    let mut img_samples = vec![0_u8; 66];
    img_samples[0] = 255;
    objects.insert(
        xobj_id,
        Object::from(Stream::new(
            dict! {
                "Type" => Object::name("XObject"),
                "Subtype" => Object::name("Image"),
                "Width" => 2,
                "Height" => 1,
                "BitsPerComponent" => 8,
                "ColorSpace" => vec![
                    Object::name("DeviceN"),
                    Object::from(names),
                    Object::name("DeviceRGB"),
                    Object::from(tint_id),
                ],
            },
            img_samples,
        )),
    );

    let provider = empty_provider(&objects);
    let raw = decode_stream_raw(
        provider,
        objects.get(&xobj_id).unwrap().as_stream().unwrap(),
        None,
        &DecodeOptions::ISO,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(raw.components(), 33);

    let mut dec_px = [0.0_f32; 33];
    assert!(raw.pixel_decoded(0, 0, &mut dec_px));
    assert!((dec_px[0] - 1.0).abs() < 1e-5);
    assert!((dec_px[32] - 0.0).abs() < 1e-5);

    let rgba = decode_xobject(
        provider,
        xobj_id,
        None,
        &DecodeOptions::ISO,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(rgba.pixel(0, 0), Some([255, 128, 0, 255]));
    assert_eq!(rgba.pixel(1, 0), Some([0, 128, 255, 255]));
}

#[test]
fn explicit_stencil_mask_different_resolution_smask_precedence_and_indexed_matte() {
    let mut store = BTreeMap::new();

    // 1. Explicit 1-bit stencil `/Mask` stream (`2x2`) at a different resolution over a `4x4`
    //    `DeviceRGB` parent image (`MaskSpec::Stencil`):
    //    `(0,0)` painted (`0`), `(1,0)` transparent (`1`), `(0,1)` transparent (`1`), `(1,1)` painted (`0`).
    let mask_id = ObjectId::new(1, 0);
    store.insert(
        mask_id,
        Object::from(Stream::new(
            dict! {
                "Subtype" => Object::name("Image"),
                "Width" => 2,
                "Height" => 2,
                "ImageMask" => true,
            },
            // Row 0: bits `0, 1` (`0b0100_0000`); Row 1: bits `1, 0` (`0b1000_0000`)
            vec![0b0100_0000, 0b1000_0000],
        )),
    );
    let mut rgb_4x4 = Vec::with_capacity(4 * 4 * 3);
    for _ in 0..16 {
        rgb_4x4.extend_from_slice(&[200, 50, 25]);
    }
    let parent_with_mask = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 4,
            "Height" => 4,
            "ColorSpace" => Object::name("DeviceRGB"),
            "BitsPerComponent" => 8,
            "Mask" => mask_id,
        },
        rgb_4x4.clone(),
    );
    let raw_masked = decode_stream_raw(
        empty_provider(&store),
        &parent_with_mask,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(raw_masked.meta().mask, MaskSpec::Stencil(mask_id));
    let out_masked = raw_masked
        .to_rgba8(
            empty_provider(&store),
            &DecodeOptions::PDFIUM,
            &Limits::DEFAULT,
        )
        .unwrap();
    // Top-left 2x2 quadrant `(0..2, 0..2)` is opaque; top-right `(2..4, 0..2)` is transparent;
    // bottom-left `(0..2, 2..4)` is transparent; bottom-right `(2..4, 2..4)` is opaque!
    assert_eq!(out_masked.pixel(0, 0), Some([200, 50, 25, 255]));
    assert_eq!(out_masked.pixel(1, 1), Some([200, 50, 25, 255]));
    assert_eq!(out_masked.pixel(2, 0), Some([0, 0, 0, 0]));
    assert_eq!(out_masked.pixel(0, 3), Some([0, 0, 0, 0]));
    assert_eq!(out_masked.pixel(3, 3), Some([200, 50, 25, 255]));

    // 2. When `/SMask` is ALSO present alongside `/Mask` (either stencil stream or color-key array),
    //    `/Mask` MUST be ignored per ISO 32000-2 §8.9.6.1!
    let smask_180_id = ObjectId::new(2, 0);
    store.insert(
        smask_180_id,
        Object::from(Stream::new(
            dict! {
                "Subtype" => Object::name("Image"),
                "Width" => 4,
                "Height" => 4,
                "ColorSpace" => Object::name("DeviceGray"),
                "BitsPerComponent" => 8,
            },
            vec![180_u8; 16],
        )),
    );
    let both_masks = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 4,
            "Height" => 4,
            "ColorSpace" => Object::name("DeviceRGB"),
            "BitsPerComponent" => 8,
            "Mask" => mask_id,
            "SMask" => smask_180_id,
        },
        rgb_4x4,
    );
    let out_both = decode_stream(
        empty_provider(&store),
        &both_masks,
        None,
        &DecodeOptions::ISO,
        &Limits::DEFAULT,
    )
    .unwrap();
    // Quadrant `(2, 0)` would have been masked to alpha `0` by `mask_id`, but `/SMask` wins (`180`)!
    assert_eq!(out_both.pixel(2, 0), Some([200, 50, 25, 180]));

    let both_ck_and_smask = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 2,
            "Height" => 1,
            "ColorSpace" => Object::name("DeviceGray"),
            "BitsPerComponent" => 8,
            "Mask" => vec![Object::from(0), Object::from(0)],
            "SMask" => Object::from(ObjectId::new(3, 0)),
        },
        vec![0, 255],
    );
    store.insert(
        ObjectId::new(3, 0),
        Object::from(Stream::new(
            dict! {
                "Subtype" => Object::name("Image"),
                "Width" => 2,
                "Height" => 1,
                "ColorSpace" => Object::name("DeviceGray"),
                "BitsPerComponent" => 8,
            },
            vec![200, 200],
        )),
    );
    let out_ck_smask = decode_stream(
        empty_provider(&store),
        &both_ck_and_smask,
        None,
        &DecodeOptions::PDFIUM,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(out_ck_smask.pixel(0, 0), Some([0, 0, 0, 200]));

    // 3. `Indexed` over `DeviceRGB` with `/SMask` + 3-component `/Matte [1.0, 1.0, 1.0]`
    //    (ISO 32000-2 §11.6.5.3: `/Matte` is in the BASE color space, NOT 1-component `Indexed`!).
    let indexed_smask_id = ObjectId::new(4, 0);
    store.insert(
        indexed_smask_id,
        Object::from(Stream::new(
            dict! {
                "Subtype" => Object::name("Image"),
                "Width" => 2,
                "Height" => 1,
                "ColorSpace" => Object::name("DeviceGray"),
                "BitsPerComponent" => 8,
                "Matte" => vec![Object::from(1.0), Object::from(1.0), Object::from(1.0)],
            },
            vec![128, 255],
        )),
    );
    let indexed_parent = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 2,
            "Height" => 1,
            "ColorSpace" => vec![
                Object::name("Indexed"),
                Object::name("DeviceRGB"),
                Object::from(1),
                // Entry 0: `[255, 128, 128]` (pure red 50% premultiplied over white matte `[1, 1, 1]`)
                // Entry 1: `[0, 200, 0]` (opaque green)
                Object::bytes(&[255_u8, 128, 128, 0, 200, 0][..]),
            ],
            "BitsPerComponent" => 8,
            "SMask" => indexed_smask_id,
        },
        vec![0, 1],
    );
    for opts in [DecodeOptions::ISO, DecodeOptions::PDFIUM] {
        let mut faults = Vec::new();
        let rgba = decode_stream_with(
            empty_provider(&store),
            &indexed_parent,
            None,
            &opts,
            &Limits::DEFAULT,
            &mut faults,
        )
        .unwrap();
        assert!(faults.is_empty(), "unexpected faults: {faults:?}");
        let p0 = rgba.pixel(0, 0).unwrap();
        assert!(
            p0[0] == 255 && p0[1] <= 2 && p0[2] <= 2 && p0[3] == 128,
            "{p0:?}"
        );
        assert_eq!(rgba.pixel(1, 0), Some([0, 200, 0, 255]));
    }

    // Conversely, a 1-component `/Matte [1.0]` on `Indexed` over `DeviceRGB` is invalid and reports `Fault::InvalidMatte`.
    let bad_matte_id = ObjectId::new(5, 0);
    store.insert(
        bad_matte_id,
        Object::from(Stream::new(
            dict! {
                "Subtype" => Object::name("Image"),
                "Width" => 2,
                "Height" => 1,
                "ColorSpace" => Object::name("DeviceGray"),
                "BitsPerComponent" => 8,
                "Matte" => vec![Object::from(1.0)],
            },
            vec![128, 255],
        )),
    );
    let bad_indexed_parent = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 2,
            "Height" => 1,
            "ColorSpace" => vec![
                Object::name("Indexed"),
                Object::name("DeviceRGB"),
                Object::from(1),
                Object::bytes(&[255_u8, 128, 128, 0, 200, 0][..]),
            ],
            "BitsPerComponent" => 8,
            "SMask" => bad_matte_id,
        },
        vec![0, 1],
    );
    let mut bad_faults = Vec::new();
    let _ = decode_stream_with(
        empty_provider(&store),
        &bad_indexed_parent,
        None,
        &DecodeOptions::ISO,
        &Limits::DEFAULT,
        &mut bad_faults,
    )
    .unwrap();
    assert!(
        bad_faults.iter().any(|&(_, f)| f == Fault::InvalidMatte),
        "{bad_faults:?}"
    );
}

#[test]
fn lab_color_space_multi_component_decode_and_jpx_stencil_and_indexed_precision() {
    let objects = BTreeMap::new();
    let provider = empty_provider(&objects);

    // 1. `/Lab` image (`2x1`, 8-bit): default `/Decode` for Lab is `[0 100 a_min a_max b_min b_max]`!
    //    Sample `(255, 128, 128)` -> `L* = 100, a* ~ 0, b* ~ 0` (white `[255, 255, 255, 255]`);
    //    Sample `(0, 128, 128)` -> `L* = 0, a* ~ 0, b* ~ 0` (black `[0, 0, 0, 255]`).
    let lab_cs = Object::from(vec![
        Object::name("Lab"),
        Object::from(dict! {
            "WhitePoint" => vec![Object::from(0.9505), Object::from(1.0), Object::from(1.089)],
            "Range" => vec![
                Object::from(-128.0),
                Object::from(127.0),
                Object::from(-128.0),
                Object::from(127.0),
            ],
        }),
    ]);
    let lab_stream = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 2,
            "Height" => 1,
            "ColorSpace" => lab_cs,
            "BitsPerComponent" => 8,
        },
        vec![255, 128, 128, 0, 128, 128],
    );
    let lab_rgba = decode_stream(
        provider,
        &lab_stream,
        None,
        &DecodeOptions::ISO,
        &Limits::DEFAULT,
    )
    .unwrap();
    let white = lab_rgba.pixel(0, 0).unwrap();
    assert!(
        white[0] >= 252 && white[1] >= 252 && white[2] >= 252 && white[3] == 255,
        "{white:?}"
    );
    assert_eq!(lab_rgba.pixel(1, 0), Some([0, 0, 0, 255]));

    // 2. Non-default `/Decode [1 0 1 0 0 1]` on a 3-component `DeviceRGB` image (`[0, 255, 255]` -> `[255, 0, 255]`)!
    let inv_rg_stream = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 1,
            "Height" => 1,
            "ColorSpace" => Object::name("DeviceRGB"),
            "BitsPerComponent" => 8,
            "Decode" => vec![
                Object::from(1.0),
                Object::from(0.0),
                Object::from(1.0),
                Object::from(0.0),
                Object::from(0.0),
                Object::from(1.0),
            ],
        },
        vec![0, 255, 200],
    );
    let inv_rgba = decode_stream(
        provider,
        &inv_rg_stream,
        None,
        &DecodeOptions::ISO,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(inv_rgba.pixel(0, 0), Some([255, 0, 200, 255]));

    // 3. 1-bit single-channel `JPXDecode` stencil (`/ImageMask true`):
    //    `build_j2c_mid_gray(2, 2, 1, 1)` produces raw 1-bit sample `1 << 0 == 1` everywhere!
    //    - With default `/Decode [0 1]`, sample `1` -> `1.0` -> unpainted (`[0, 0, 0, 0]`);
    //    - With inverted `/Decode [1 0]`, sample `1` -> `0.0` -> painted in `stencil_color`!
    let j2c_1bit = build_j2c_mid_gray(2, 2, 1, 1);
    let jpx_stencil_default = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 2,
            "Height" => 2,
            "ImageMask" => true,
            "Filter" => Object::name("JPXDecode"),
        },
        j2c_1bit.clone(),
    );
    let mut stencil_opts = DecodeOptions::ISO;
    stencil_opts.stencil_color = [30, 140, 220, 255];
    let st_def = decode_stream(
        provider,
        &jpx_stencil_default,
        None,
        &stencil_opts,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(st_def.pixel(0, 0), Some([0, 0, 0, 0]));

    let jpx_stencil_inv = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 2,
            "Height" => 2,
            "ImageMask" => true,
            "Decode" => vec![Object::from(1), Object::from(0)],
            "Filter" => Object::name("JPXDecode"),
        },
        j2c_1bit,
    );
    let st_inv = decode_stream(
        provider,
        &jpx_stencil_inv,
        None,
        &stencil_opts,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(st_inv.pixel(0, 0), Some([30, 140, 220, 255]));

    // 4. `JPXDecode` ignores `/BitsPerComponent` on the stream dictionary (§7.4.9) AND ignores
    //    `/Decode` when `/ColorSpace` is omitted (§8.9.5.2)!
    let j2c_8bit_rgb = build_j2c_mid_gray(2, 2, 3, 8); // raw `[128, 128, 128]`
    let jpx_ignored_entries = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 2,
            "Height" => 2,
            "BitsPerComponent" => 12, // invalid PDF bpc, must be ignored on JPX!
            "Decode" => vec![
                Object::from(0.0), Object::from(0.0),
                Object::from(0.0), Object::from(0.0),
                Object::from(0.0), Object::from(0.0),
            ], // must be ignored when `/ColorSpace` is absent on JPX!
            "Filter" => Object::name("JPXDecode"),
        },
        j2c_8bit_rgb,
    );
    let raw_jpx = decode_stream_raw(
        provider,
        &jpx_ignored_entries,
        None,
        &DecodeOptions::ISO,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(raw_jpx.bits_per_component(), 8);
    let rgba_jpx = raw_jpx
        .to_rgba8(provider, &DecodeOptions::ISO, &Limits::DEFAULT)
        .unwrap();
    assert_eq!(rgba_jpx.pixel(0, 0), Some([128, 128, 128, 255]));

    // 5. Non-PDF-depth (`3-bit`) `JPXDecode` with PDF `/ColorSpace [/Indexed /DeviceRGB 15 ...]`:
    //    `build_j2c_mid_gray(2, 2, 1, 3)` produces raw 3-bit index `4` (`1 << 2`).
    //    Palette index MUST remain `4` (pure green `[0, 255, 0]`), NOT rescaled to `(4 * 15 + 3) / 7 = 9`!
    let j2c_3bit = build_j2c_mid_gray(2, 2, 1, 3);
    let mut lut_16 = [0_u8; 16 * 3];
    lut_16[4 * 3..4 * 3 + 3].copy_from_slice(&[0, 255, 0]); // index 4 = green
    lut_16[9 * 3..9 * 3 + 3].copy_from_slice(&[255, 0, 0]); // index 9 = red (buggy rescaled index)
    let jpx_indexed_3bit = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 2,
            "Height" => 2,
            "ColorSpace" => vec![
                Object::name("Indexed"),
                Object::name("DeviceRGB"),
                Object::from(15),
                Object::bytes(&lut_16[..]),
            ],
            "Filter" => Object::name("JPXDecode"),
        },
        j2c_3bit,
    );
    let idx_rgba = decode_stream(
        provider,
        &jpx_indexed_3bit,
        None,
        &DecodeOptions::ISO,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(idx_rgba.pixel(0, 0), Some([0, 255, 0, 255]));
}

#[test]
fn flate_raw_sample_limit_scrub_safety_and_encode_raw_flate_preservation() {
    // 1. Flate-compressed raw `16x16` `DeviceRGB` image (`768` decoded sample bytes, `< 30` compressed bytes):
    //    with `max_encoded_bytes = 64` and `max_sample_bytes = 4096`, BOTH `decode_stream` and `decode_xobject`
    //    must succeed (enforcing `max_sample_bytes`, NOT `max_encoded_bytes`, on non-image-codec streams!).
    let solid_rgb = vec![42_u8; 16 * 16 * 3];
    let raw_in = decode_stream_raw(
        empty_provider(&BTreeMap::new()),
        &Stream::new(
            dict! {
                "Subtype" => Object::name("Image"),
                "Width" => 16,
                "Height" => 16,
                "ColorSpace" => Object::name("DeviceRGB"),
                "BitsPerComponent" => 8,
            },
            solid_rgb,
        ),
        None,
        &DecodeOptions::ISO,
        &Limits::DEFAULT,
    )
    .unwrap();
    let flate_img = encode_raw_flate(&raw_in, None, Level::DEFAULT).unwrap();
    assert!(flate_img.stream.data.bytes().unwrap().len() < 64);

    let mut store = BTreeMap::new();
    let xobj_id = ObjectId::new(10, 0);
    // Uncompressed raw `16x16` `DeviceRGB` stream (`768` bytes) in `store` so `decode_xobject`
    // directly verifies `objects.decoded(id, fetch_cap)` uses `max_sample_bytes` (`4096`),
    // NOT `max_encoded_bytes` (`64`), while `decode_stream` on `flate_img.stream` verifies
    // `FlateDecode` raw streams also enforce `max_sample_bytes` (`4096`)!
    store.insert(
        xobj_id,
        Object::from(Stream::new(
            dict! {
                "Subtype" => Object::name("Image"),
                "Width" => 16,
                "Height" => 16,
                "ColorSpace" => Object::name("DeviceRGB"),
                "BitsPerComponent" => 8,
            },
            vec![42_u8; 16 * 16 * 3],
        )),
    );
    let mut lim = Limits::DEFAULT;
    lim.max_encoded_bytes = 64;
    lim.max_sample_bytes = 4096;
    let d1 = decode_stream(
        empty_provider(&store),
        &flate_img.stream,
        None,
        &DecodeOptions::ISO,
        &lim,
    )
    .unwrap();
    let d2 = decode_xobject(
        empty_provider(&store),
        xobj_id,
        None,
        &DecodeOptions::ISO,
        &lim,
    )
    .unwrap();
    assert_eq!(d1.pixel(0, 0), Some([42, 42, 42, 255]));
    assert_eq!(d2.pixel(15, 15), Some([42, 42, 42, 255]));

    // 2. `fill_rect_raw` rejects short `fill` slices and color-key-transparent fills, leaving samples intact!
    let ck_rgb_stream = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 2,
            "Height" => 1,
            "ColorSpace" => Object::name("DeviceRGB"),
            "BitsPerComponent" => 8,
            "Mask" => vec![
                Object::from(10), Object::from(20),
                Object::from(30), Object::from(40),
                Object::from(50), Object::from(60),
            ],
            "SMask" => ObjectId::new(90, 0),
            "OC" => ObjectId::new(91, 0),
        },
        vec![100, 110, 120, 200, 210, 220],
    );
    let mut ck_raw = decode_stream_raw(
        empty_provider(&store),
        &ck_rgb_stream,
        None,
        &DecodeOptions::ISO,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(
        ck_raw.fill_rect_raw(0, 0, 1, 1, &[0, 0]),
        Err(ScrubError::InsufficientComponents {
            actual: 2,
            expected: 3,
        })
    );
    assert_eq!(
        ck_raw.fill_rect_raw(0, 0, 1, 1, &[15, 35, 55]),
        Err(ScrubError::ColorKeyTransparentFill)
    );
    let mut untouched = [0_u16; 3];
    assert!(ck_raw.pixel_raw(0, 0, &mut untouched));
    assert_eq!(untouched, [100, 110, 120]);

    // Valid opaque fill succeeds, and `encode_raw_flate` preserves `/Mask`, `/SMask`, and `/OC`!
    assert_eq!(
        ck_raw.fill_rect_raw(0, 0, 1, 1, &[0, 0, 0]),
        Ok(Some((0, 0, 1, 1)))
    );
    let mut reencoded = encode_raw_flate(&ck_raw, None, Level::DEFAULT).unwrap();
    assert!(reencoded.stream.dict.get(b"Mask").is_some());
    assert_eq!(
        reencoded
            .stream
            .dict
            .get(b"SMask")
            .and_then(Object::as_reference),
        Some(ObjectId::new(90, 0))
    );
    assert_eq!(
        reencoded
            .stream
            .dict
            .get(b"OC")
            .and_then(Object::as_reference),
        Some(ObjectId::new(91, 0))
    );
    reencoded.attach_mask(ObjectId::new(92, 0));
    assert_eq!(
        reencoded
            .stream
            .dict
            .get(b"Mask")
            .and_then(Object::as_reference),
        Some(ObjectId::new(92, 0))
    );

    // 3. `encode_raw_flate` refuses `color_space: None` on non-device spaces (`Indexed`).
    let idx_stream = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 1,
            "Height" => 1,
            "ColorSpace" => vec![
                Object::name("Indexed"),
                Object::name("DeviceRGB"),
                Object::from(1),
                Object::bytes(&[0_u8, 0, 0, 255, 255, 255][..]),
            ],
            "BitsPerComponent" => 8,
        },
        vec![1],
    );
    let idx_raw = decode_stream_raw(
        empty_provider(&store),
        &idx_stream,
        None,
        &DecodeOptions::ISO,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(
        encode_raw_flate(&idx_raw, None, Level::DEFAULT).unwrap_err(),
        EncodeError::MissingColorSpace
    );
}

#[test]
fn dct_subsampled_progressive_restart_markers_and_app14_transform_pinning() {
    use jpeg_encoder::{ColorType, Encoder, SamplingFactor};

    let objects = BTreeMap::new();
    let provider = empty_provider(&objects);

    // Build an 8x8 RGB image: left half solid red `[240, 20, 20]`, right half solid blue `[20, 40, 240]`.
    let mut rgb_8x8 = Vec::with_capacity(8 * 8 * 3);
    for _y in 0..8 {
        for x in 0..8 {
            if x < 4 {
                rgb_8x8.extend_from_slice(&[240, 20, 20]);
            } else {
                rgb_8x8.extend_from_slice(&[20, 40, 240]);
            }
        }
    }

    // 1. 4:2:0 chroma-subsampled JPEG (`SamplingFactor::F_2_2`, `SOF0` with `0x22` H2V2 luma sampling)
    let mut jpeg_420 = Vec::new();
    {
        let mut enc = Encoder::new(&mut jpeg_420, 98);
        enc.set_sampling_factor(SamplingFactor::F_2_2);
        enc.encode(&rgb_8x8, 8, 8, ColorType::Rgb).unwrap();
    }
    // Verify `SOF0` has `H=2, V=2` (`0x22`) on component 1
    assert!(jpeg_420.windows(2).any(|w| w == [0xFF, 0xC0]));

    // 2. Multi-scan progressive JPEG (`.set_progressive(true)`, `SOF2` `0xFF, 0xC2`, zero `SOF0`)
    let mut jpeg_prog = Vec::new();
    {
        let mut enc = Encoder::new(&mut jpeg_prog, 98);
        enc.set_progressive(true);
        enc.encode(&rgb_8x8, 8, 8, ColorType::Rgb).unwrap();
    }
    assert!(jpeg_prog.windows(2).any(|w| w == [0xFF, 0xC2]));
    assert!(!jpeg_prog.windows(2).any(|w| w == [0xFF, 0xC0]));

    // 3. Restart-marker JPEG (`.set_restart_interval(1)` on `16x16` -> `DRI` `0xFF, 0xDD` + `RST0..RST2`)
    let mut rgb_16x16 = Vec::with_capacity(16 * 16 * 3);
    for _y in 0..16 {
        for x in 0..16 {
            if x < 8 {
                rgb_16x16.extend_from_slice(&[240, 20, 20]);
            } else {
                rgb_16x16.extend_from_slice(&[20, 40, 240]);
            }
        }
    }
    let mut jpeg_rst = Vec::new();
    {
        let mut enc = Encoder::new(&mut jpeg_rst, 98);
        enc.set_restart_interval(1);
        enc.encode(&rgb_16x16, 16, 16, ColorType::Rgb).unwrap();
    }
    assert!(jpeg_rst.windows(2).any(|w| w == [0xFF, 0xDD]));
    assert!(jpeg_rst.windows(2).any(|w| w == [0xFF, 0xD0]));

    for (w, h, data, right_x) in [
        (8_i64, 8_i64, jpeg_420, 7_u32),
        (8, 8, jpeg_prog, 7),
        (16, 16, jpeg_rst, 15),
    ] {
        let s = Stream::new(
            dict! {
                "Subtype" => Object::name("Image"),
                "Width" => w,
                "Height" => h,
                "ColorSpace" => Object::name("DeviceRGB"),
                "BitsPerComponent" => 8,
                "Filter" => Object::name("DCTDecode"),
            },
            data,
        );
        let rgba = decode_stream(
            empty_provider(&objects),
            &s,
            None,
            &DecodeOptions::ISO,
            &Limits::DEFAULT,
        )
        .unwrap();
        let left = rgba.pixel(0, 0).unwrap();
        let right = rgba.pixel(right_x, 0).unwrap();
        assert!(left[0] > 200 && left[2] < 60, "left={left:?}");
        assert!(right[2] > 200 && right[0] < 60, "right={right:?}");
    }

    // 4. Pin `APP14` injection against `zune-jpeg` `0.5.15`:
    //    Strip any `APP0..APP15` segments from a raw 3-channel JPEG encoded with `ColorType::Ycbcr`
    //    (so raw stored channels are literal `[80, 200, 40]` with NO JFIF or Adobe marker).
    //    - `/DecodeParms << /ColorTransform 0 >>` injects `APP14 transform=0`, so `zune-jpeg`
    //      returns literal RGB `[~80, ~200, ~40]`;
    //    - `/DecodeParms << /ColorTransform 1 >>` injects `APP14 transform=1`, so `zune-jpeg`
    //      converts Y=80, Cb=200, Cr=40 -> RGB `[~0, ~105, ~208]`!
    fn strip_app_markers(jpeg: &[u8]) -> Vec<u8> {
        let mut out = Vec::from(&jpeg[..2]); // SOI
        let mut i = 2;
        while i + 1 < jpeg.len() {
            if jpeg[i] != 0xFF {
                out.extend_from_slice(&jpeg[i..]);
                break;
            }
            let m = jpeg[i + 1];
            if m == 0xDA || m == 0xD9 {
                out.extend_from_slice(&jpeg[i..]);
                break;
            }
            let seg_len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
            if !(0xE0..=0xEF).contains(&m) {
                out.extend_from_slice(&jpeg[i..i + 2 + seg_len]);
            }
            i += 2 + seg_len;
        }
        out
    }

    let mut raw_3ch = Vec::new();
    {
        let mut enc = Encoder::new(&mut raw_3ch, 100);
        enc.set_sampling_factor(SamplingFactor::R_4_4_4);
        // `ColorType::Ycbcr` writes the 3 channels verbatim without RGB->YCbCr conversion!
        let samples = [80_u8, 200, 40].repeat(4);
        enc.encode(&samples, 2, 2, ColorType::Ycbcr).unwrap();
    }
    let bare_3ch = strip_app_markers(&raw_3ch);
    assert!(!bare_3ch.windows(5).any(|w| w == b"JFIF\0" || w == b"Adobe"));

    let s_ct0 = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 2,
            "Height" => 2,
            "ColorSpace" => Object::name("DeviceRGB"),
            "BitsPerComponent" => 8,
            "Filter" => Object::name("DCTDecode"),
            "DecodeParms" => dict! { "ColorTransform" => 0 },
        },
        bare_3ch.clone(),
    );
    let s_ct1 = Stream::new(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 2,
            "Height" => 2,
            "ColorSpace" => Object::name("DeviceRGB"),
            "BitsPerComponent" => 8,
            "Filter" => Object::name("DCTDecode"),
            "DecodeParms" => dict! { "ColorTransform" => 1 },
        },
        bare_3ch,
    );
    let p_ct0 = decode_stream(
        provider,
        &s_ct0,
        None,
        &DecodeOptions::ISO,
        &Limits::DEFAULT,
    )
    .unwrap()
    .pixel(0, 0)
    .unwrap();
    let p_ct1 = decode_stream(
        provider,
        &s_ct1,
        None,
        &DecodeOptions::ISO,
        &Limits::DEFAULT,
    )
    .unwrap()
    .pixel(0, 0)
    .unwrap();
    // Literal RGB: `[~80, ~200, ~40]`
    assert!(
        (i16::from(p_ct0[0]) - 80).abs() <= 2
            && (i16::from(p_ct0[1]) - 200).abs() <= 2
            && (i16::from(p_ct0[2]) - 40).abs() <= 2,
        "ColorTransform 0 expected literal [80, 200, 40], got {p_ct0:?}"
    );
    // YCbCr converted to RGB (`Y=80, Cb=200, Cr=40` -> BT.601 `[0, 118, 208]`):
    assert!(
        p_ct1[0] < 5
            && (i16::from(p_ct1[1]) - 118).abs() <= 3
            && (i16::from(p_ct1[2]) - 208).abs() <= 3,
        "ColorTransform 1 expected YCbCr->RGB [~0, ~118, ~208], got {p_ct1:?}"
    );
}
