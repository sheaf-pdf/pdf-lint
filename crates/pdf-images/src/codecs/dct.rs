//! `DCTDecode` (JPEG) adapter over `zune-jpeg`.

use std::panic::{AssertUnwindSafe, catch_unwind};

use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Dict, Resolve, Rules};
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::{ZByteIoError, ZByteReaderTrait, ZSeekFrom};
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

    let max_w = (limits.max_width as usize).min(65_535);
    let max_h = (limits.max_height as usize).min(65_535);

    let run = catch_unwind(AssertUnwindSafe(|| -> Result<DctDecoded, Error> {
        // Probe headers first on a patched view so zune-jpeg's dimension checks use `limits`.
        let options = DecoderOptions::default()
            .set_max_width(max_w)
            .set_max_height(max_h)
            .set_strict_mode(false);
        let mut probe = JpegDecoder::new_with_options(
            PatchedJpeg::new(span, scanned.app14_range.clone(), None),
            options,
        );
        probe.decode_headers().map_err(|e| Error::Codec {
            codec: ImageCodec::Dct,
            detail: format!("{e}"),
        })?;
        let Some(info) = probe.info() else {
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

        // Decide whether YCbCr->RGB (3-ch) or YCCK->CMYK (4-ch) transform applies.
        let do_transform = resolve_color_transform(comps, parms_ct, scanned.adobe_transform, rules);

        // Configure synthetic APP14 + zune-jpeg output colorspace so we extract raw channels in the
        // image's own component count.
        let (synthetic_app14, out_cs, manual_ycck) = match comps {
            1 => (None, ColorSpace::Luma, false),
            2 => (
                None,
                ColorSpace::MultiBand(core::num::NonZeroU32::new(2).unwrap()),
                false,
            ),
            3 => {
                if do_transform {
                    // Force YCbCr -> RGB.
                    (Some(1_u8), ColorSpace::RGB, false)
                } else {
                    // Force literal 3-channel RGB copy without YCbCr conversion.
                    (Some(0_u8), ColorSpace::RGB, false)
                }
            }
            4 => {
                // Always extract raw 4-channel IDCT+upsampled planes from zune-jpeg, then apply
                // BT.601 YCCK -> CMYK when `do_transform` is true.
                (Some(0_u8), ColorSpace::CMYK, do_transform)
            }
            _ => unreachable!(),
        };

        let decode_opts = options.jpeg_set_out_colorspace(out_cs);
        let reader = PatchedJpeg::new(span, scanned.app14_range.clone(), synthetic_app14);
        let mut decoder = JpegDecoder::new_with_options(reader, decode_opts);
        decoder.decode_headers().map_err(|e| Error::Codec {
            codec: ImageCodec::Dct,
            detail: format!("{e}"),
        })?;
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
    adobe_transform: Option<u8>,
    app14_range: Option<core::ops::Range<usize>>,
}

/// Scans JPEG marker segments from `FF D8` up to `SOS` (`FF DA`) to locate any Adobe `APP14`
/// transform byte and check SOF precision before handing the stream to `zune-jpeg`.
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
        ) && !payload.is_empty()
        {
            out.precision = Some(payload[0]);
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

fn make_adobe_app14(transform: u8) -> [u8; ADOBE_APP14_LEN] {
    [
        0xFF, 0xEE, 0x00, 0x0E, b'A', b'd', b'o', b'b', b'e', 0x00, 0x64, 0x00, 0x00, 0x00, 0x00,
        transform,
    ]
}

/// Zero-copy virtual JPEG byte reader that optionally splices an Adobe `APP14` segment right after
/// `SOI` (removing any existing `APP14`) and ensures a trailing `EOI` (`FF D9`) for truncated inline
/// or XObject streams.
struct PatchedJpeg<'a> {
    segments: [&'a [u8]; 4],
    app14_buf: [u8; ADOBE_APP14_LEN],
    use_app14: bool,
    total_len: usize,
    pos: usize,
}

const TRAILING_EOI: [u8; 2] = [0xFF, 0xD9];

impl<'a> PatchedJpeg<'a> {
    fn new(
        span: &'a [u8],
        existing_app14: Option<core::ops::Range<usize>>,
        inject_transform: Option<u8>,
    ) -> Self {
        let app14_buf = make_adobe_app14(inject_transform.unwrap_or(0));
        let use_app14 = inject_transform.is_some();

        let (soi_part, mid_part, rest_part) = if span.len() >= 2 {
            match existing_app14 {
                Some(r) if use_app14 && r.start >= 2 && r.end <= span.len() => {
                    (&span[..2], &span[2..r.start], &span[r.end..])
                }
                _ => (&span[..2], &[][..], &span[2..]),
            }
        } else {
            (span, &[][..], &[][..])
        };

        let needs_eoi = span.len() < 2 || span[span.len() - 2..] != TRAILING_EOI;
        let tail_eoi: &[u8] = if needs_eoi { &TRAILING_EOI } else { &[] };

        let total_len = soi_part.len()
            + (if use_app14 { ADOBE_APP14_LEN } else { 0 })
            + mid_part.len()
            + rest_part.len()
            + tail_eoi.len();

        PatchedJpeg {
            segments: [soi_part, mid_part, rest_part, tail_eoi],
            app14_buf,
            use_app14,
            total_len,
            pos: 0,
        }
    }

    fn byte_at(&self, mut idx: usize) -> Option<u8> {
        if idx >= self.total_len {
            return None;
        }
        let s0 = self.segments[0];
        if idx < s0.len() {
            return Some(s0[idx]);
        }
        idx -= s0.len();
        if self.use_app14 {
            if idx < ADOBE_APP14_LEN {
                return Some(self.app14_buf[idx]);
            }
            idx -= ADOBE_APP14_LEN;
        }
        for seg in &self.segments[1..] {
            if idx < seg.len() {
                return Some(seg[idx]);
            }
            idx -= seg.len();
        }
        None
    }

    fn copy_from_pos(&self, out: &mut [u8]) -> usize {
        let avail = self.total_len.saturating_sub(self.pos);
        let n = out.len().min(avail);
        for (i, slot) in out[..n].iter_mut().enumerate() {
            *slot = self.byte_at(self.pos + i).unwrap_or(0);
        }
        n
    }
}

impl ZByteReaderTrait for PatchedJpeg<'_> {
    fn read_byte_no_error(&mut self) -> u8 {
        let b = self.byte_at(self.pos).unwrap_or(0);
        if self.pos < self.total_len {
            self.pos += 1;
        }
        b
    }

    fn read_exact_bytes(&mut self, buf: &mut [u8]) -> Result<(), ZByteIoError> {
        self.peek_exact_bytes(buf)?;
        self.pos += buf.len();
        Ok(())
    }

    fn read_bytes(&mut self, buf: &mut [u8]) -> Result<usize, ZByteIoError> {
        let n = self.copy_from_pos(buf);
        self.pos += n;
        Ok(n)
    }

    fn peek_bytes(&mut self, buf: &mut [u8]) -> Result<usize, ZByteIoError> {
        Ok(self.copy_from_pos(buf))
    }

    fn peek_exact_bytes(&mut self, buf: &mut [u8]) -> Result<(), ZByteIoError> {
        let left = self.total_len.saturating_sub(self.pos);
        if buf.len() > left {
            return Err(ZByteIoError::NotEnoughBytes(buf.len(), left));
        }
        self.copy_from_pos(buf);
        Ok(())
    }

    fn z_seek(&mut self, from: ZSeekFrom) -> Result<u64, ZByteIoError> {
        let (base, by) = match from {
            ZSeekFrom::Start(n) => (0_usize, i64::try_from(n).unwrap_or(i64::MAX)),
            ZSeekFrom::End(n) => (self.total_len, n),
            ZSeekFrom::Current(n) => (self.pos, n),
        };
        let mag = usize::try_from(by.unsigned_abs()).unwrap_or(usize::MAX);
        let next = if by >= 0 {
            base.saturating_add(mag)
        } else {
            base.saturating_sub(mag)
        };
        self.pos = next.min(self.total_len);
        Ok(self.pos as u64)
    }

    fn is_eof(&mut self) -> Result<bool, ZByteIoError> {
        Ok(self.pos >= self.total_len)
    }

    fn z_position(&mut self) -> Result<u64, ZByteIoError> {
        Ok(self.pos as u64)
    }

    fn read_remaining(&mut self, sink: &mut Vec<u8>) -> Result<usize, ZByteIoError> {
        let left = self.total_len.saturating_sub(self.pos);
        let mut rest = vec![0_u8; left];
        let n = self.read_bytes(&mut rest)?;
        sink.extend_from_slice(&rest[..n]);
        Ok(n)
    }
}
