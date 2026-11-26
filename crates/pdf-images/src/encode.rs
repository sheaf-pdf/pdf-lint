//! Clean, metadata-free PDF image stream re-encoders (`FlateDecode` + PNG predictor and pure-Rust
//! baseline `DCTDecode`).
//!
//! Designed for redaction pixel scrubbing and document sanitization:
//! re-encoded Image XObject streams contain freshly constructed PDF dictionaries and compressed
//! pixel samples only—never EXIF, XMP, IPTC, `APPn` segments, or JBIG2 symbol dictionaries.

use std::f32::consts::PI;

use pdf_color::Family;
use pdf_filters::{Backend, Encoder, Layout, Level, Prediction, Row, encode};
use pdf_objects::{Dict, Object, ObjectId, Stream, dict};

use crate::error::EncodeError;
use crate::rgba::RgbaImage;
use crate::samples::RawImage;

/// Encoded PDF Image XObject stream paired with an optional companion `/SMask` soft-mask stream.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct EncodedImage {
    /// Primary Image XObject stream (`/Type /XObject`, `/Subtype /Image`).
    pub stream: Stream,
    /// Optional 8-bit `DeviceGray` `/SMask` stream when the source image carried non-opaque alpha
    /// (`alpha < 255`).
    pub smask: Option<Stream>,
}

impl EncodedImage {
    /// Links an allocated `smask_id` onto `self.stream.dict` under `/SMask`.
    pub fn attach_smask(&mut self, smask_id: ObjectId) {
        self.stream.dict.set("SMask", smask_id);
    }
}

/// Re-encodes a [`RawImage`] losslessly using `FlateDecode` (with PNG `Sub` prediction for 8- and
/// 16-bit images and `Prediction::None` for 1-, 2-, and 4-bit packed images).
///
/// Any bi-level `CCITTFaxDecode` or `JBIG2Decode` image is cleanly re-encoded as a 1-bit
/// `FlateDecode` stream—never as JBIG2. If the `RawImage` carried an embedded JPX alpha plane with
/// any non-opaque pixel, a companion 8-bit `DeviceGray` `/SMask` stream is returned in
/// [`EncodedImage::smask`].
///
/// `color_space` supplies the PDF `/ColorSpace` object to write into the stream dictionary when
/// the image is not an `/ImageMask` stencil and its space is non-device (such as `Indexed`,
/// `ICCBased`, `Separation`, `DeviceN`, or CIE spaces); device spaces (`DeviceGray`, `DeviceRGB`,
/// `DeviceCMYK`) are inferred automatically when `color_space` is `None`.
///
/// # Errors
///
/// Returns [`EncodeError`] if dimensions are zero or Flate/predictor encoding rejects the layout.
pub fn encode_raw_flate(
    raw: &RawImage,
    color_space: Option<Object>,
    level: Level,
) -> Result<EncodedImage, EncodeError> {
    let w = raw.width();
    let h = raw.height();
    if w == 0 || h == 0 {
        return Err(EncodeError::Dimensions {
            width: w,
            height: h,
        });
    }
    let bpc = raw.bits_per_component();
    let comps = raw.components();
    let raw_bytes = raw.stream_bytes();

    let (prediction, parms) = if bpc >= 8 {
        let layout = Layout {
            colors: u32::from(comps),
            bits_per_component: u32::from(bpc),
            columns: w,
        };
        let pred = Prediction::Png(layout, Row::Sub);
        let dp = dict! {
            "Predictor" => i64::from(pred.predictor()),
            "Colors" => i64::from(comps),
            "BitsPerComponent" => i64::from(bpc),
            "Columns" => i64::from(w),
        };
        (pred, Some(dp))
    } else {
        (Prediction::None, None)
    };

    let mut compressed = Vec::new();
    encode(
        &raw_bytes,
        &[Encoder::Flate {
            level,
            predictor: prediction,
        }],
        Backend::MinizOxide,
        &mut compressed,
    )?;

    let mut d = Dict::new();
    d.set("Type", Object::name("XObject"));
    d.set("Subtype", Object::name("Image"));
    d.set("Width", i64::from(w));
    d.set("Height", i64::from(h));

    if raw.meta().stencil {
        d.set("ImageMask", true);
        d.set("BitsPerComponent", 1_i64);
    } else {
        let cs_obj = match color_space {
            Some(obj) => obj,
            None => match raw.meta().color_space {
                Some(Family::DeviceGray) => Object::name("DeviceGray"),
                Some(Family::DeviceRGB) => Object::name("DeviceRGB"),
                Some(Family::DeviceCMYK) => Object::name("DeviceCMYK"),
                _ => match comps {
                    1 => Object::name("DeviceGray"),
                    3 => Object::name("DeviceRGB"),
                    4 => Object::name("DeviceCMYK"),
                    _ => Object::name("DeviceRGB"),
                },
            },
        };
        d.set("ColorSpace", cs_obj);
        d.set("BitsPerComponent", i64::from(bpc));
    }

    if raw.meta().interpolate {
        d.set("Interpolate", true);
    }
    if let Some(intent) = raw.meta().intent.and_then(|i| i.as_pdf_name()) {
        d.set("Intent", Object::name(intent));
    }
    if !raw.meta().has_default_decode(raw.prepared_space()) {
        let mut arr = Vec::with_capacity(raw.meta().decode.len() * 2);
        for &(lo, hi) in &raw.meta().decode {
            arr.push(Object::from(lo));
            arr.push(Object::from(hi));
        }
        d.set("Decode", arr);
    }

    d.set("Filter", Object::name("FlateDecode"));
    if let Some(dp) = parms {
        d.set("DecodeParms", dp);
    }

    let smask = match raw.embedded_alpha() {
        Some(alpha) if alpha.iter().any(|&a| a < 255) => {
            Some(encode_gray_smask(alpha, w, h, level)?)
        }
        _ => None,
    };

    Ok(EncodedImage {
        stream: Stream::new(d, compressed),
        smask,
    })
}

/// Encodes an [`RgbaImage`] losslessly as an 8-bit `DeviceRGB` `FlateDecode` Image XObject stream
/// (plus an 8-bit `DeviceGray` `/SMask` stream if any pixel has `alpha < 255`).
///
/// # Errors
///
/// Returns [`EncodeError`] if Flate/predictor encoding fails.
pub fn encode_rgba_flate(image: &RgbaImage, level: Level) -> Result<EncodedImage, EncodeError> {
    let w = image.width();
    let h = image.height();
    let pixels = (w as usize) * (h as usize);
    let mut rgb = Vec::with_capacity(pixels * 3);
    let mut alpha = Vec::with_capacity(pixels);
    let mut has_transparency = false;

    for &[r, g, b, a] in image.as_bytes().as_chunks::<4>().0 {
        rgb.extend_from_slice(&[r, g, b]);
        alpha.push(a);
        if a < 255 {
            has_transparency = true;
        }
    }

    let layout = Layout {
        colors: 3,
        bits_per_component: 8,
        columns: w,
    };
    let pred = Prediction::Png(layout, Row::Sub);
    let mut compressed = Vec::new();
    encode(
        &rgb,
        &[Encoder::Flate {
            level,
            predictor: pred,
        }],
        Backend::MinizOxide,
        &mut compressed,
    )?;

    let d = dict! {
        "Type" => Object::name("XObject"),
        "Subtype" => Object::name("Image"),
        "Width" => i64::from(w),
        "Height" => i64::from(h),
        "ColorSpace" => Object::name("DeviceRGB"),
        "BitsPerComponent" => 8_i64,
        "Filter" => Object::name("FlateDecode"),
        "DecodeParms" => dict! {
            "Predictor" => i64::from(pred.predictor()),
            "Colors" => 3_i64,
            "BitsPerComponent" => 8_i64,
            "Columns" => i64::from(w),
        },
    };

    let smask = if has_transparency {
        Some(encode_gray_smask(&alpha, w, h, level)?)
    } else {
        None
    };

    Ok(EncodedImage {
        stream: Stream::new(d, compressed),
        smask,
    })
}

fn encode_gray_smask(
    alpha: &[u8],
    width: u32,
    height: u32,
    level: Level,
) -> Result<Stream, EncodeError> {
    let layout = Layout {
        colors: 1,
        bits_per_component: 8,
        columns: width,
    };
    let pred = Prediction::Png(layout, Row::Sub);
    let mut compressed = Vec::new();
    encode(
        alpha,
        &[Encoder::Flate {
            level,
            predictor: pred,
        }],
        Backend::MinizOxide,
        &mut compressed,
    )?;
    let d = dict! {
        "Type" => Object::name("XObject"),
        "Subtype" => Object::name("Image"),
        "Width" => i64::from(width),
        "Height" => i64::from(height),
        "ColorSpace" => Object::name("DeviceGray"),
        "BitsPerComponent" => 8_i64,
        "Filter" => Object::name("FlateDecode"),
        "DecodeParms" => dict! {
            "Predictor" => i64::from(pred.predictor()),
            "Colors" => 1_i64,
            "BitsPerComponent" => 8_i64,
            "Columns" => i64::from(width),
        },
    };
    Ok(Stream::new(d, compressed))
}

/// Target color space for baseline JPEG (`DCTDecode`) encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum JpegColorSpace {
    /// 1-channel `DeviceGray`.
    Gray,
    /// 3-channel `DeviceRGB` encoded as 4:4:4 `YCbCr`.
    Rgb,
    /// 4-channel `DeviceCMYK` (`0 = 0% ink, 255 = 100% ink`) encoded as 4:4:4 `YCCK` with a clean
    /// 14-byte Adobe `APP14` (`transform = 2`) header.
    Cmyk,
}

impl JpegColorSpace {
    /// Number of bytes per pixel expected in the input slice (`1`, `3`, or `4`).
    #[must_use]
    pub const fn components(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
        }
    }

    const fn pdf_name(self) -> &'static str {
        match self {
            Self::Gray => "DeviceGray",
            Self::Rgb => "DeviceRGB",
            Self::Cmyk => "DeviceCMYK",
        }
    }
}

/// Encodes interleaved 8-bit `pixels` into a clean PDF `DCTDecode` Image XObject [`Stream`] at
/// `quality` (`1..=100`), containing zero EXIF, XMP, IPTC, or comment segments.
///
/// # Errors
///
/// Returns [`EncodeError::Dimensions`] if `width` or `height` is `0` or `> 65_535`, or
/// [`EncodeError::BufferLength`] if `pixels.len() != width * height * space.components()`.
pub fn encode_jpeg(
    pixels: &[u8],
    width: u32,
    height: u32,
    space: JpegColorSpace,
    quality: u8,
) -> Result<Stream, EncodeError> {
    let bytes = encode_jpeg_bytes(pixels, width, height, space, quality)?;
    let d = dict! {
        "Type" => Object::name("XObject"),
        "Subtype" => Object::name("Image"),
        "Width" => i64::from(width),
        "Height" => i64::from(height),
        "ColorSpace" => Object::name(space.pdf_name()),
        "BitsPerComponent" => 8_i64,
        "Filter" => Object::name("DCTDecode"),
    };
    Ok(Stream::new(d, bytes))
}

/// Encodes interleaved 8-bit `pixels` into a standalone baseline Huffman DCT JPEG bitstream at
/// `quality` (`1..=100`) with zero metadata segments.
///
/// # Errors
///
/// Returns [`EncodeError::Dimensions`] if `width` or `height` is `0` or `> 65_535`, or
/// [`EncodeError::BufferLength`] if `pixels.len() != width * height * space.components()`.
pub fn encode_jpeg_bytes(
    pixels: &[u8],
    width: u32,
    height: u32,
    space: JpegColorSpace,
    quality: u8,
) -> Result<Vec<u8>, EncodeError> {
    if width == 0 || height == 0 || width > 65_535 || height > 65_535 {
        return Err(EncodeError::Dimensions { width, height });
    }
    let comps = space.components();
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(comps))
        .ok_or(EncodeError::Dimensions { width, height })?;
    if pixels.len() != expected {
        return Err(EncodeError::BufferLength {
            actual: pixels.len(),
            expected,
        });
    }

    let q = quality.clamp(1, 100);
    let luma_q = scale_qtable(&STD_LUMA_Q, q);
    let chroma_q = scale_qtable(&STD_CHROMA_Q, q);

    let dc_luma = build_huff_lut(&DC_LUMA_BITS, &DC_LUMA_VALS);
    let ac_luma = build_huff_lut(&AC_LUMA_BITS, &AC_LUMA_VALS);
    let dc_chroma = build_huff_lut(&DC_CHROMA_BITS, &DC_CHROMA_VALS);
    let ac_chroma = build_huff_lut(&AC_CHROMA_BITS, &AC_CHROMA_VALS);
    let cos_table = fdct_cos_table();

    let mut out = Vec::with_capacity(expected / 2 + 512);
    // SOI
    out.extend_from_slice(&[0xFF, 0xD8]);

    // Adobe APP14 for 4-channel YCCK (`transform = 2`) or 3-channel YCbCr? For Gray/RGB standard
    // JFIF/SOF0 needs no APPn at all; for CMYK we emit the minimal 14-byte Adobe APP14 header.
    if space == JpegColorSpace::Cmyk {
        out.extend_from_slice(&[
            0xFF, 0xEE, 0x00, 0x0E, b'A', b'd', b'o', b'b', b'e', 0x00, 0x64, 0x00, 0x00, 0x00,
            0x00, 0x02,
        ]);
    }

    // DQT
    write_dqt(&mut out, 0, &luma_q);
    if comps >= 3 {
        write_dqt(&mut out, 1, &chroma_q);
    }

    // SOF0 (Baseline DCT, 4:4:4 sampling `0x11`)
    let sof_len = (8 + 3 * comps) as u16;
    out.extend_from_slice(&[0xFF, 0xC0]);
    out.extend_from_slice(&sof_len.to_be_bytes());
    out.push(8); // 8-bit sample precision
    out.extend_from_slice(&(height as u16).to_be_bytes());
    out.extend_from_slice(&(width as u16).to_be_bytes());
    out.push(comps as u8);
    match space {
        JpegColorSpace::Gray => {
            out.extend_from_slice(&[1, 0x11, 0]);
        }
        JpegColorSpace::Rgb => {
            out.extend_from_slice(&[1, 0x11, 0, 2, 0x11, 1, 3, 0x11, 1]);
        }
        JpegColorSpace::Cmyk => {
            out.extend_from_slice(&[1, 0x11, 0, 2, 0x11, 1, 3, 0x11, 1, 4, 0x11, 0]);
        }
    }

    // DHT
    write_dht(&mut out, 0x00, &DC_LUMA_BITS, &DC_LUMA_VALS);
    write_dht(&mut out, 0x10, &AC_LUMA_BITS, &AC_LUMA_VALS);
    if comps >= 3 {
        write_dht(&mut out, 0x01, &DC_CHROMA_BITS, &DC_CHROMA_VALS);
        write_dht(&mut out, 0x11, &AC_CHROMA_BITS, &AC_CHROMA_VALS);
    }

    // SOS
    let sos_len = (6 + 2 * comps) as u16;
    out.extend_from_slice(&[0xFF, 0xDA]);
    out.extend_from_slice(&sos_len.to_be_bytes());
    out.push(comps as u8);
    match space {
        JpegColorSpace::Gray => {
            out.extend_from_slice(&[1, 0x00]);
        }
        JpegColorSpace::Rgb => {
            out.extend_from_slice(&[1, 0x00, 2, 0x11, 3, 0x11]);
        }
        JpegColorSpace::Cmyk => {
            out.extend_from_slice(&[1, 0x00, 2, 0x11, 3, 0x11, 4, 0x00]);
        }
    }
    out.extend_from_slice(&[0x00, 0x3F, 0x00]); // Ss=0, Se=63, Ah/Al=0

    // Entropy-coded MCUs
    let mut bw = JpegBitWriter {
        out: &mut out,
        acc: 0,
        bits: 0,
    };
    let mut prev_dc = [0_i32; 4];
    let mcu_cols = (width as usize).div_ceil(8);
    let mcu_rows = (height as usize).div_ceil(8);
    let w_usize = width as usize;
    let h_usize = height as usize;

    let mut blocks = [[0.0_f32; 64]; 4];
    for my in 0..mcu_rows {
        for mx in 0..mcu_cols {
            for by in 0..8 {
                let py = (my * 8 + by).min(h_usize - 1);
                for bx in 0..8 {
                    let px = (mx * 8 + bx).min(w_usize - 1);
                    let src = &pixels[(py * w_usize + px) * comps..];
                    let k = by * 8 + bx;
                    match space {
                        JpegColorSpace::Gray => {
                            blocks[0][k] = f32::from(src[0]) - 128.0;
                        }
                        JpegColorSpace::Rgb => {
                            let (y, cb, cr) = rgb_to_ycbcr(src[0], src[1], src[2]);
                            blocks[0][k] = y - 128.0;
                            blocks[1][k] = cb - 128.0;
                            blocks[2][k] = cr - 128.0;
                        }
                        JpegColorSpace::Cmyk => {
                            // Inverse of `ycck_to_cmyk_in_place` (`libjpeg`'s `cmyk_ycck_convert`):
                            // encode `(255 - C, 255 - M, 255 - Y)` as YCbCr and pass `K` directly.
                            let (y, cb, cr) =
                                rgb_to_ycbcr(255 - src[0], 255 - src[1], 255 - src[2]);
                            blocks[0][k] = y - 128.0;
                            blocks[1][k] = cb - 128.0;
                            blocks[2][k] = cr - 128.0;
                            blocks[3][k] = f32::from(src[3]) - 128.0;
                        }
                    }
                }
            }

            for c in 0..comps {
                let is_chroma = c == 1 || c == 2;
                let qt = if is_chroma { &chroma_q } else { &luma_q };
                let dc_lut = if is_chroma { &dc_chroma } else { &dc_luma };
                let ac_lut = if is_chroma { &ac_chroma } else { &ac_luma };
                let zz = fdct_and_quantize(&blocks[c], qt, &cos_table);
                encode_block(&mut bw, &zz, &mut prev_dc[c], dc_lut, ac_lut);
            }
        }
    }
    bw.flush();

    // EOI
    out.extend_from_slice(&[0xFF, 0xD9]);
    Ok(out)
}

#[inline]
fn rgb_to_ycbcr(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let rf = f32::from(r);
    let gf = f32::from(g);
    let bf = f32::from(b);
    let y = 0.299 * rf + 0.587 * gf + 0.114 * bf;
    let cb = -0.168_736 * rf - 0.331_264 * gf + 0.5 * bf + 128.0;
    let cr = 0.5 * rf - 0.418_688 * gf - 0.081_312 * bf + 128.0;
    (y, cb, cr)
}

fn fdct_cos_table() -> [[f32; 8]; 8] {
    let mut table = [[0.0_f32; 8]; 8];
    let inv_sqrt2 = 0.5_f32.sqrt();
    for (u, row) in table.iter_mut().enumerate() {
        let cu = if u == 0 { inv_sqrt2 } else { 1.0 };
        for (x, slot) in row.iter_mut().enumerate() {
            *slot = 0.5 * cu * (((2 * x + 1) * u) as f32 * PI / 16.0).cos();
        }
    }
    table
}

fn fdct_and_quantize(block: &[f32; 64], qtable_zz: &[u8; 64], cos: &[[f32; 8]; 8]) -> [i32; 64] {
    let mut tmp = [0.0_f32; 64];
    // 1D FDCT across rows
    for y in 0..8 {
        let row = &block[y * 8..y * 8 + 8];
        for u in 0..8 {
            let c = &cos[u];
            tmp[y * 8 + u] = row[0] * c[0]
                + row[1] * c[1]
                + row[2] * c[2]
                + row[3] * c[3]
                + row[4] * c[4]
                + row[5] * c[5]
                + row[6] * c[6]
                + row[7] * c[7];
        }
    }
    // 1D FDCT down columns + zig-zag + quantize
    let mut freq = [0.0_f32; 64];
    for x in 0..8 {
        for v in 0..8 {
            let c = &cos[v];
            freq[v * 8 + x] = tmp[x] * c[0]
                + tmp[8 + x] * c[1]
                + tmp[16 + x] * c[2]
                + tmp[24 + x] * c[3]
                + tmp[32 + x] * c[4]
                + tmp[40 + x] * c[5]
                + tmp[48 + x] * c[6]
                + tmp[56 + x] * c[7];
        }
    }
    let mut zz = [0_i32; 64];
    for i in 0..64 {
        let q = f32::from(qtable_zz[i]);
        zz[i] = (freq[ZIGZAG[i]] / q).round() as i32;
    }
    zz
}

fn scale_qtable(natural: &[u8; 64], quality: u8) -> [u8; 64] {
    let scale = if quality < 50 {
        5000 / u32::from(quality)
    } else {
        200 - 2 * u32::from(quality)
    };
    let mut zz = [0_u8; 64];
    for i in 0..64 {
        let base = u32::from(natural[ZIGZAG[i]]);
        let v = ((base * scale + 50) / 100).clamp(1, 255);
        zz[i] = v as u8;
    }
    zz
}

fn write_dqt(out: &mut Vec<u8>, id: u8, zz_table: &[u8; 64]) {
    out.extend_from_slice(&[0xFF, 0xDB, 0x00, 67, id]);
    out.extend_from_slice(zz_table);
}

fn write_dht(out: &mut Vec<u8>, class_id: u8, bits: &[u8; 16], vals: &[u8]) {
    let len = (2 + 1 + 16 + vals.len()) as u16;
    out.extend_from_slice(&[0xFF, 0xC4]);
    out.extend_from_slice(&len.to_be_bytes());
    out.push(class_id);
    out.extend_from_slice(bits);
    out.extend_from_slice(vals);
}

fn build_huff_lut(bits: &[u8; 16], vals: &[u8]) -> [(u16, u8); 256] {
    let mut lut = [(0_u16, 0_u8); 256];
    let mut code = 0_u16;
    let mut idx = 0_usize;
    for (len_minus_1, &count) in bits.iter().enumerate() {
        let bit_len = (len_minus_1 + 1) as u8;
        for _ in 0..count {
            let sym = vals[idx] as usize;
            lut[sym] = (code, bit_len);
            code += 1;
            idx += 1;
        }
        code <<= 1;
    }
    lut
}

struct JpegBitWriter<'a> {
    out: &'a mut Vec<u8>,
    acc: u32,
    bits: u8,
}

impl JpegBitWriter<'_> {
    #[inline]
    fn put(&mut self, code: u16, len: u8) {
        if len == 0 {
            return;
        }
        let mask = (1_u32 << len) - 1;
        self.acc = (self.acc << len) | (u32::from(code) & mask);
        self.bits += len;
        while self.bits >= 8 {
            self.bits -= 8;
            let byte = ((self.acc >> self.bits) & 0xFF) as u8;
            self.out.push(byte);
            if byte == 0xFF {
                self.out.push(0x00);
            }
        }
    }

    fn flush(&mut self) {
        if self.bits > 0 {
            let pad = 8 - self.bits;
            self.put((1_u16 << pad) - 1, pad);
        }
    }
}

#[inline]
fn category_and_bits(val: i32) -> (u8, u16) {
    if val == 0 {
        return (0, 0);
    }
    let abs = val.unsigned_abs();
    let cat = (32 - abs.leading_zeros()) as u8;
    let amp = if val > 0 {
        val as u16
    } else {
        ((val - 1) & ((1_i32 << cat) - 1)) as u16
    };
    (cat, amp)
}

fn encode_block(
    bw: &mut JpegBitWriter<'_>,
    zz: &[i32; 64],
    prev_dc: &mut i32,
    dc_lut: &[(u16, u8); 256],
    ac_lut: &[(u16, u8); 256],
) {
    let diff = zz[0] - *prev_dc;
    *prev_dc = zz[0];
    let (dc_cat, dc_amp) = category_and_bits(diff);
    let (dc_code, dc_len) = dc_lut[usize::from(dc_cat)];
    bw.put(dc_code, dc_len);
    bw.put(dc_amp, dc_cat);

    let mut zero_run = 0_u8;
    for &coef in &zz[1..] {
        if coef == 0 {
            zero_run += 1;
        } else {
            while zero_run >= 16 {
                let (zrl_code, zrl_len) = ac_lut[0xF0];
                bw.put(zrl_code, zrl_len);
                zero_run -= 16;
            }
            let (cat, amp) = category_and_bits(coef);
            let sym = usize::from((zero_run << 4) | cat);
            let (ac_code, ac_len) = ac_lut[sym];
            bw.put(ac_code, ac_len);
            bw.put(amp, cat);
            zero_run = 0;
        }
    }
    if zero_run > 0 {
        let (eob_code, eob_len) = ac_lut[0x00];
        bw.put(eob_code, eob_len);
    }
}

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

const STD_LUMA_Q: [u8; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];

const STD_CHROMA_Q: [u8; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

const DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_LUMA_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

const DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_CHROMA_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

const AC_LUMA_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7D];
const AC_LUMA_VALS: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08, 0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2A, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7,
    0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5,
    0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2,
    0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8,
    0xF9, 0xFA,
];

const AC_CHROMA_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
const AC_CHROMA_VALS: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xA1, 0xB1, 0xC1, 0x09, 0x23, 0x33, 0x52, 0xF0,
    0x15, 0x62, 0x72, 0xD1, 0x0A, 0x16, 0x24, 0x34, 0xE1, 0x25, 0xF1, 0x17, 0x18, 0x19, 0x1A, 0x26,
    0x27, 0x28, 0x29, 0x2A, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5,
    0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3,
    0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA,
    0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8,
    0xF9, 0xFA,
];
