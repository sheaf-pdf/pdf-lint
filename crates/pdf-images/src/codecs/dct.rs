//! `DCTDecode` (JPEG) adapter over `zune-jpeg`.

use std::borrow::Cow;
use std::panic::{AssertUnwindSafe, catch_unwind};

use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Dict, Resolve, Rules};
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use crate::error::Error;
use crate::fault::Fault;
use crate::header::{ImageCodec, check_dimensions, check_sample_budget, read_int};
use crate::limits::Limits;
use crate::samples::SampleData;

/// Decoded JPEG output before PDF `/Decode` or color space evaluation.
pub(crate) struct DctDecoded {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) components: u8,
    pub(crate) samples: SampleData,
}

/// Decodes a `DCTDecode` byte slice into 8-bit native-channel [`SampleData`].
///
/// Note: `catch_unwind` isolates internal panics in `zune-jpeg` (`0.5.15`) under default
/// `panic = "unwind"` builds; `panic = "abort"` binaries abort on internal third-party panics.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_dct<'o, R, S>(
    objects: R,
    data: &[u8],
    parms: Option<&Dict>,
    dict_width: u32,
    dict_height: u32,
    expected_components: u8,
    rules: Rules,
    limits: &Limits,
    place: Place,
    sink: &mut S,
) -> Result<DctDecoded, Error>
where
    R: Resolve<'o> + Copy,
    S: Sink<Fault> + ?Sized,
{
    let parms_ct = match parms {
        Some(d) => read_int(objects, d, b"ColorTransform")?,
        None => None,
    };

    let Some(soi) = find_soi(data) else {
        return Err(Error::Codec {
            codec: ImageCodec::Dct,
            detail: "missing JPEG SOI (FF D8) marker".to_string(),
        });
    };
    let span = &data[soi..];
    let scanned = scan_jpeg_headers(span);

    if let Some(precision) = scanned.precision
        && precision != 8
    {
        return Err(Error::Codec {
            codec: ImageCodec::Dct,
            detail: format!("unsupported {precision}-bit JPEG sample precision"),
        });
    }

    // Check SOF dimensions and component count upfront before building a patched buffer or
    // invoking `zune-jpeg`.
    let sof_comps = match scanned.components {
        Some(c) if !(1..=4).contains(&c) => {
            return Err(Error::Codec {
                codec: ImageCodec::Dct,
                detail: format!("unsupported JPEG component count ({c})"),
            });
        }
        Some(c) => {
            if let (Some(w), Some(h)) = (scanned.width, scanned.height) {
                let pixels = check_dimensions(u32::from(w), u32::from(h), limits)?;
                let _ = check_sample_budget(pixels.saturating_mul(u64::from(c)), limits)?;
            }
            c
        }
        None => expected_components,
    };

    // Configure synthetic APP14 + `zune-jpeg` (`0.5.15`) output colorspace so we extract raw
    // channels in the image's own component count in a single decoder pass.
    let do_transform = resolve_color_transform(sof_comps, parms_ct, scanned.adobe_transform, rules);
    let (synthetic_app14, out_cs, manual_ycck) = match sof_comps {
        1 => (None, ColorSpace::Luma, false),
        2 => (
            None,
            ColorSpace::MultiBand(core::num::NonZeroU32::new(2).unwrap()),
            false,
        ),
        3 => {
            if do_transform {
                (Some(1_u8), ColorSpace::RGB, false)
            } else {
                (Some(0_u8), ColorSpace::RGB, false)
            }
        }
        4 => (Some(0_u8), ColorSpace::CMYK, do_transform),
        _ => (None, ColorSpace::RGB, false),
    };

    let patched = patch_jpeg_bytes(span, scanned.app14_range, synthetic_app14);
    let max_w = (limits.max_width as usize).min(65_535);
    let max_h = (limits.max_height as usize).min(65_535);

    let run = catch_unwind(AssertUnwindSafe(|| -> Result<DctDecoded, Error> {
        let options = DecoderOptions::default()
            .set_max_width(max_w)
            .set_max_height(max_h)
            .set_strict_mode(false)
            .jpeg_set_out_colorspace(out_cs);
        let mut decoder = JpegDecoder::new_with_options(ZCursor::new(patched.as_ref()), options);
        decoder.decode_headers().map_err(|e| Error::Codec {
            codec: ImageCodec::Dct,
            detail: format!("{e}"),
        })?;
        let Some(info) = decoder.info() else {
            return Err(Error::Codec {
                codec: ImageCodec::Dct,
                detail: "JPEG headers missing frame info".to_string(),
            });
        };

        let w = u32::from(info.width);
        let h = u32::from(info.height);
        let comps = info.components;
        if !(1..=4).contains(&comps) {
            return Err(Error::Codec {
                codec: ImageCodec::Dct,
                detail: format!("unsupported JPEG component count ({comps})"),
            });
        }

        let pixels = check_dimensions(w, h, limits)?;
        let out_bytes = check_sample_budget(pixels.saturating_mul(u64::from(comps)), limits)?;
        let Some(buf_size) = decoder.output_buffer_size() else {
            return Err(Error::Codec {
                codec: ImageCodec::Dct,
                detail: "JPEG output buffer size overflow".to_string(),
            });
        };
        if buf_size != out_bytes {
            return Err(Error::Codec {
                codec: ImageCodec::Dct,
                detail: "JPEG output size mismatch".to_string(),
            });
        }

        let mut buf = vec![0_u8; out_bytes];
        decoder.decode_into(&mut buf).map_err(|e| Error::Codec {
            codec: ImageCodec::Dct,
            detail: format!("{e}"),
        })?;

        if manual_ycck {
            ycck_to_cmyk_in_place(&mut buf);
        }

        Ok(DctDecoded {
            width: w,
            height: h,
            components: comps,
            samples: SampleData::U8(buf),
        })
    }));

    let decoded = match run {
        Ok(res) => res?,
        Err(_) => {
            return Err(Error::Codec {
                codec: ImageCodec::Dct,
                detail: "internal JPEG decoder panic caught".to_string(),
            });
        }
    };

    if decoded.width != dict_width || decoded.height != dict_height {
        sink.fault(place, Fault::DimensionMismatch);
    }
    if decoded.components != expected_components {
        sink.fault(place, Fault::ComponentMismatch);
    }

    Ok(decoded)
}

fn find_soi(data: &[u8]) -> Option<usize> {
    data.windows(2).position(|w| w == [0xFF, 0xD8])
}

#[derive(Default)]
struct ScannedJpeg {
    precision: Option<u8>,
    width: Option<u16>,
    height: Option<u16>,
    components: Option<u8>,
    adobe_transform: Option<u8>,
    app14_range: Option<core::ops::Range<usize>>,
}

/// Scans JPEG marker segments from `FF D8` up to `SOS` (`FF DA`) to extract SOF frame geometry
/// and locate any Adobe `APP14` transform byte in a single pass before invoking `zune-jpeg`.
fn scan_jpeg_headers(span: &[u8]) -> ScannedJpeg {
    let mut out = ScannedJpeg::default();
    if span.len() < 4 || span[..2] != [0xFF, 0xD8] {
        return out;
    }
    let mut i = 2;
    while i + 1 < span.len() {
        if span[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker_start = i;
        while i < span.len() && span[i] == 0xFF {
            i += 1;
        }
        if i >= span.len() {
            break;
        }
        let marker = span[i];
        i += 1;
        match marker {
            0x00 | 0x01 | 0xD0..=0xD8 => continue,
            0xD9 | 0xDA => break,
            _ => {}
        }
        if i + 2 > span.len() {
            break;
        }
        let seg_len = u16::from_be_bytes([span[i], span[i + 1]]) as usize;
        if seg_len < 2 || i + seg_len > span.len() {
            break;
        }
        let payload = &span[i + 2..i + seg_len];
        if matches!(
            marker,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
        ) {
            if !payload.is_empty() {
                out.precision = Some(payload[0]);
            }
            if payload.len() >= 6 {
                out.height = Some(u16::from_be_bytes([payload[1], payload[2]]));
                out.width = Some(u16::from_be_bytes([payload[3], payload[4]]));
                out.components = Some(payload[5]);
            }
        } else if marker == 0xEE && payload.len() >= 12 && &payload[..5] == b"Adobe" {
            out.adobe_transform = Some(payload[11]);
            out.app14_range = Some(marker_start..(i + seg_len));
        }
        i += seg_len;
    }
    out
}

fn resolve_color_transform(
    components: u8,
    parms_ct: Option<i64>,
    adobe_transform: Option<u8>,
    rules: Rules,
) -> bool {
    let default_ct = components == 3;
    match rules {
        Rules::Iso => {
            // ISO 32000-2 §7.4.8.2: an Adobe APP14 marker in the stream overrides /ColorTransform.
            if let Some(t) = adobe_transform {
                t != 0
            } else if let Some(ct) = parms_ct {
                ct != 0
            } else {
                default_ct
            }
        }
        _ => {
            // PDFium (`CPDF_DIB::CreateDCTDecoder`): explicit /ColorTransform in /DecodeParms
            // overrides APP14; otherwise APP14 applies, falling back to `components == 3`.
            if let Some(ct) = parms_ct {
                ct != 0
            } else if let Some(t) = adobe_transform {
                t != 0
            } else {
                default_ct
            }
        }
    }
}

/// Converts interleaved `[Y, Cb, Cr, K]` samples in-place to `[C, M, Y, K]` using `libjpeg`'s
/// `ycck_cmyk_convert` convention (`out[0..3] = 255 - RGB(Y, Cb, Cr)`, `out[3] = K`).
fn ycck_to_cmyk_in_place(buf: &mut [u8]) {
    for px in buf.as_chunks_mut::<4>().0 {
        let y = f32::from(px[0]);
        let cb = f32::from(px[1]) - 128.0;
        let cr = f32::from(px[2]) - 128.0;
        let r = (y + 1.402 * cr).round().clamp(0.0, 255.0) as u8;
        let g = (y - 0.344_136 * cb - 0.714_136 * cr)
            .round()
            .clamp(0.0, 255.0) as u8;
        let b = (y + 1.772 * cb).round().clamp(0.0, 255.0) as u8;
        px[0] = 255 - r;
        px[1] = 255 - g;
        px[2] = 255 - b;
    }
}

const ADOBE_APP14_LEN: usize = 16;
const TRAILING_EOI: [u8; 2] = [0xFF, 0xD9];

fn make_adobe_app14(transform: u8) -> [u8; ADOBE_APP14_LEN] {
    [
        0xFF, 0xEE, 0x00, 0x0E, b'A', b'd', b'o', b'b', b'e', 0x00, 0x64, 0x00, 0x00, 0x00, 0x00,
        transform,
    ]
}

/// Splices a synthetic Adobe `APP14` segment right after `SOI` (removing any existing `APP14`) and
/// ensures a trailing `EOI` (`FF D9`), borrowing `span` without copying when neither edit is
/// needed.
fn patch_jpeg_bytes<'a>(
    span: &'a [u8],
    existing_app14: Option<core::ops::Range<usize>>,
    inject_transform: Option<u8>,
) -> Cow<'a, [u8]> {
    let needs_eoi = !span.ends_with(&TRAILING_EOI);
    if inject_transform.is_none() && !needs_eoi {
        return Cow::Borrowed(span);
    }

    let extra = (if inject_transform.is_some() {
        ADOBE_APP14_LEN
    } else {
        0
    }) + (if needs_eoi { 2 } else { 0 });
    let mut out = Vec::with_capacity(span.len() + extra);

    if let Some(transform) = inject_transform
        && span.len() >= 2
    {
        out.extend_from_slice(&span[..2]);
        out.extend_from_slice(&make_adobe_app14(transform));
        match existing_app14 {
            Some(r) if r.start >= 2 && r.end <= span.len() => {
                out.extend_from_slice(&span[2..r.start]);
                out.extend_from_slice(&span[r.end..]);
            }
            _ => out.extend_from_slice(&span[2..]),
        }
    } else {
        out.extend_from_slice(span);
    }

    if needs_eoi {
        out.extend_from_slice(&TRAILING_EOI);
    }
    Cow::Owned(out)
}
