//! Clean PDF image stream re-encoders (`FlateDecode` + PNG predictor and pure-Rust
//! baseline `DCTDecode` via `jpeg-encoder`).
//!
//! Designed for redaction pixel scrubbing and document sanitization:
//! re-encoded Image XObject streams contain freshly constructed PDF dictionaries and compressed
//! pixel samples only—no EXIF, XMP, IPTC, comment segments, or JBIG2 symbol dictionaries (only a
//! minimal standard JFIF or Adobe `APP14` color-transform header on JPEG output).

use jpeg_encoder::{ColorType, Encoder as JpegEncoder, ImageBuffer, JpegColorType, SamplingFactor};
use pdf_color::Family;
use pdf_filters::{Encoder, Layout, Level, Prediction, Row, encode};
use pdf_objects::{Dict, Object, ObjectId, Stream, dict};

use crate::error::EncodeError;
use crate::header::MaskSpec;
use crate::inline_chain::flate_backend;
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

    /// Links an allocated explicit stencil `mask_id` onto `self.stream.dict` under `/Mask`.
    pub fn attach_mask(&mut self, mask_id: ObjectId) {
        self.stream.dict.set("Mask", mask_id);
    }
}

/// Re-encodes a [`RawImage`] losslessly using `FlateDecode` (with PNG `Sub` prediction for 8- and
/// 16-bit images and `Prediction::None` for 1-, 2-, and 4-bit packed images).
///
/// Any bi-level `CCITTFaxDecode` or `JBIG2Decode` image is cleanly re-encoded as a 1-bit
/// `FlateDecode` stream—never as JBIG2. Existing `/Mask` (explicit stencil `ObjectId` or color-key
/// array), `/SMask` (`ObjectId`), and `/OC` (`ObjectId`) entries from [`RawImage::meta`] are
/// preserved on the output dictionary so masked or optional-content-controlled pixels are never
/// silently exposed. If the `RawImage` carried an embedded JPX alpha plane with any non-opaque
/// pixel, a companion 8-bit `DeviceGray` `/SMask` stream is returned in [`EncodedImage::smask`]
/// (which the caller must store as an indirect object and link via [`EncodedImage::attach_smask`]).
///
/// `color_space` supplies the PDF `/ColorSpace` object to write into the stream dictionary when
/// the image is not an `/ImageMask` stencil and its space is non-device (such as `Indexed`,
/// `ICCBased`, `Separation`, `DeviceN`, or CIE spaces); device spaces (`DeviceGray`, `DeviceRGB`,
/// `DeviceCMYK`) are inferred automatically when `color_space` is `None`.
///
/// # Errors
///
/// Returns [`EncodeError::Dimensions`] if dimensions are zero, [`EncodeError::MissingColorSpace`]
/// if `color_space` is `None` for a non-stencil, non-device color space, or [`EncodeError::Flate`]
/// if Flate/predictor encoding rejects the layout.
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
    let cs_obj_opt = if raw.meta().stencil {
        None
    } else {
        match color_space {
            Some(obj) => Some(obj),
            None => match raw.meta().color_space {
                Some(Family::DeviceGray) => Some(Object::name("DeviceGray")),
                Some(Family::DeviceRGB) => Some(Object::name("DeviceRGB")),
                Some(Family::DeviceCMYK) => Some(Object::name("DeviceCMYK")),
                _ => return Err(EncodeError::MissingColorSpace),
            },
        }
    };
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
        flate_backend(),
        &mut compressed,
    )?;

    let mut d = Dict::new();
    d.set("Type", Object::name("XObject"));
    d.set("Subtype", Object::name("Image"));
    d.set("Width", i64::from(w));
    d.set("Height", i64::from(h));

    if let Some(cs_obj) = cs_obj_opt {
        d.set("ColorSpace", cs_obj);
        d.set("BitsPerComponent", i64::from(bpc));
    } else {
        d.set("ImageMask", true);
        d.set("BitsPerComponent", 1_i64);
    }

    if raw.meta().interpolate {
        d.set("Interpolate", true);
    }
    if let Some(intent) = raw.meta().intent.as_ref().and_then(|i| i.as_pdf_name()) {
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

    match &raw.meta().mask {
        MaskSpec::None => {}
        MaskSpec::Stencil(mask_id) => {
            d.set("Mask", *mask_id);
        }
        MaskSpec::ColorKey(ranges) => {
            let mut arr = Vec::with_capacity(ranges.len() * 2);
            for &(lo, hi) in ranges {
                arr.push(Object::from(i64::from(lo)));
                arr.push(Object::from(i64::from(hi)));
            }
            d.set("Mask", arr);
        }
    }
    if let Some(oc_id) = raw.meta().oc {
        d.set("OC", oc_id);
    }

    d.set("Filter", Object::name("FlateDecode"));
    if let Some(dp) = parms {
        d.set("DecodeParms", dp);
    }

    let smask = match raw.embedded_alpha() {
        Some(alpha) if alpha.iter().any(|&a| a < 255) => {
            Some(encode_gray_smask(alpha, w, h, level)?)
        }
        _ => {
            if let Some(smask_id) = raw.meta().smask {
                d.set("SMask", smask_id);
            }
            None
        }
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
    let chunks = image.as_bytes().as_chunks::<4>().0;
    let has_transparency = chunks.iter().any(|px| px[3] < 255);
    let mut rgb = Vec::with_capacity(pixels * 3);
    let mut alpha = if has_transparency {
        Vec::with_capacity(pixels)
    } else {
        Vec::new()
    };

    if has_transparency {
        for &[r, g, b, a] in chunks {
            rgb.extend_from_slice(&[r, g, b]);
            alpha.push(a);
        }
    } else {
        for &[r, g, b, _] in chunks {
            rgb.extend_from_slice(&[r, g, b]);
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
        flate_backend(),
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
        flate_backend(),
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
    /// 4-channel `DeviceCMYK` (`0 = 0% ink, 255 = 100% ink`) encoded as 4:4:4 `YCCK` with an Adobe
    /// `APP14` (`transform = 2`) header.
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
/// `quality` (`1..=100`), containing no EXIF, XMP, IPTC, or comment metadata (only a minimal JFIF
/// or Adobe `APP14` header).
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
/// `quality` (`1..=100`), containing only a standard JFIF or Adobe `APP14` header and no EXIF,
/// XMP, IPTC, or comment metadata.
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
    let Ok(w16) = u16::try_from(width)
        .map_err(|_| ())
        .and_then(|w| if w == 0 { Err(()) } else { Ok(w) })
    else {
        return Err(EncodeError::Dimensions { width, height });
    };
    let Ok(h16) = u16::try_from(height)
        .map_err(|_| ())
        .and_then(|h| if h == 0 { Err(()) } else { Ok(h) })
    else {
        return Err(EncodeError::Dimensions { width, height });
    };
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

    let mut out = Vec::with_capacity(expected / 2 + 512);
    let mut encoder = JpegEncoder::new(&mut out, quality.clamp(1, 100));
    encoder.set_sampling_factor(SamplingFactor::R_4_4_4);

    let res = match space {
        JpegColorSpace::Gray => encoder.encode(pixels, w16, h16, ColorType::Luma),
        JpegColorSpace::Rgb => encoder.encode(pixels, w16, h16, ColorType::Rgb),
        JpegColorSpace::Cmyk => encoder.encode_image(PdfCmykAsYcck {
            pixels,
            width: w16,
            height: h16,
        }),
    };
    res.map_err(|_| EncodeError::Dimensions { width, height })?;
    Ok(out)
}

/// Encodes PDF `DeviceCMYK` (`0 = 0% ink, 255 = 100% ink`) as `YCCK` (`APP14 transform = 2`)
/// matching `libjpeg`'s `cmyk_ycck_convert` / `ycck_cmyk_convert` convention (`RGB` = `255 - CMY`
/// converted to `YCbCr`, with `K` stored uninverted).
struct PdfCmykAsYcck<'a> {
    pixels: &'a [u8],
    width: u16,
    height: u16,
}

impl ImageBuffer for PdfCmykAsYcck<'_> {
    fn get_jpeg_color_type(&self) -> JpegColorType {
        JpegColorType::Ycck
    }

    fn width(&self) -> u16 {
        self.width
    }

    fn height(&self) -> u16 {
        self.height
    }

    fn fill_buffers(&self, y: u16, buffers: &mut [Vec<u8>; 4]) {
        let row_bytes = usize::from(self.width) * 4;
        let start = usize::from(y) * row_bytes;
        let row = &self.pixels[start..start + row_bytes];
        for &[c, m, yel, k] in row.as_chunks::<4>().0 {
            let (luma, cb, cr) = jpeg_encoder::rgb_to_ycbcr(255 - c, 255 - m, 255 - yel);
            buffers[0].push(luma);
            buffers[1].push(cb);
            buffers[2].push(cr);
            buffers[3].push(k);
        }
    }
}

const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

fn crc32_png(chunk_type: [u8; 4], payload: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &b in chunk_type.iter().chain(payload) {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn write_png_chunk(out: &mut Vec<u8>, chunk_type: [u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&chunk_type);
    out.extend_from_slice(payload);
    out.extend_from_slice(&crc32_png(chunk_type, payload).to_be_bytes());
}

/// Encodes straight-alpha `RGBA8` `pixels` (`width * height * 4` bytes) into a standard 8-bit
/// truecolor+alpha PNG file (`IHDR` + `IDAT` + `IEND`).
///
/// # Errors
///
/// Returns [`EncodeError::Dimensions`] if `width == 0`, `height == 0`, or the
/// byte count overflows, [`EncodeError::BufferLength`] if `pixels.len()` does
/// not equal `width * height * 4`, or [`EncodeError::Filter`] on Flate failure.
pub fn encode_png_rgba8(width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>, EncodeError> {
    if width == 0 || height == 0 {
        return Err(EncodeError::Dimensions { width, height });
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or(EncodeError::Dimensions { width, height })?;
    if pixels.len() != expected {
        return Err(EncodeError::BufferLength {
            actual: pixels.len(),
            expected,
        });
    }
    let layout = Layout {
        colors: 4,
        bits_per_component: 8,
        columns: width,
    };
    let mut idat = Vec::new();
    encode(
        pixels,
        &[Encoder::Flate {
            level: Level::DEFAULT,
            predictor: Prediction::Png(layout, Row::Sub),
        }],
        flate_backend(),
        &mut idat,
    )?;
    if u32::try_from(idat.len()).is_err() {
        return Err(EncodeError::Dimensions { width, height });
    }

    let mut ihdr = [0_u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = 6; // color type: RGBA
    ihdr[10] = 0; // deflate compression
    ihdr[11] = 0; // adaptive filter method 0
    ihdr[12] = 0; // no interlace

    let mut out = Vec::with_capacity(8 + 25 + 12 + idat.len() + 12);
    out.extend_from_slice(&PNG_SIGNATURE);
    write_png_chunk(&mut out, *b"IHDR", &ihdr);
    write_png_chunk(&mut out, *b"IDAT", &idat);
    write_png_chunk(&mut out, *b"IEND", &[]);
    Ok(out)
}
