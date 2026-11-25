//! `JBIG2Decode` (ITU-T T.88 / ISO/IEC 14492) adapter over `hayro-jbig2`.

use std::borrow::Cow;
use std::panic::{AssertUnwindSafe, catch_unwind};

use hayro_jbig2::{DecodeSettings, File, Globals, Page};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Data, Dict, Resolve, Streams};

use crate::error::Error;
use crate::fault::Fault;
use crate::header::{ImageCodec, check_dimensions, check_sample_budget};
use crate::limits::Limits;
use crate::samples::SampleData;

const JBIG2_FILE_HEADER: [u8; 8] = [0x97, 0x4A, 0x42, 0x32, 0x0D, 0x0A, 0x1A, 0x0A];

pub(crate) struct Jbig2Decoded {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) samples: SampleData,
}

/// Decodes a `JBIG2Decode` stream (and optional `/JBIG2Globals` stream) into 1-bit packed
/// [`SampleData`].
///
/// Per ISO 32000-2 §7.4.7, foreground (black) pixels are stored as bit `0` and background (white)
/// pixels are stored as bit `1`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_jbig2<'o, O, S>(
    objects: O,
    data: &[u8],
    parms: Option<&Dict>,
    dict_width: u32,
    dict_height: u32,
    limits: &Limits,
    place: Place,
    sink: &mut S,
) -> Result<Jbig2Decoded, Error>
where
    O: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let globals_bytes: Option<Cow<'_, [u8]>> = match parms.and_then(|d| d.get(b"JBIG2Globals")) {
        None => None,
        Some(raw) => {
            if let Some(id) = raw.as_reference() {
                Some(objects.decoded(id, limits.max_encoded_bytes)?)
            } else if let Some(stream) = raw.as_stream() {
                match &stream.data {
                    Data::Bytes(bytes) => {
                        if bytes.len() > limits.max_encoded_bytes {
                            return Err(Error::Limit {
                                kind: crate::limits::LimitKind::EncodedBytes,
                                value: bytes.len() as u64,
                                max: limits.max_encoded_bytes as u64,
                            });
                        }
                        Some(Cow::Borrowed(bytes.as_slice()))
                    }
                    _ => None,
                }
            } else {
                None
            }
        }
    };

    let settings = DecodeSettings { strict: false };

    let run = catch_unwind(AssertUnwindSafe(|| -> Result<Jbig2Decoded, Error> {
        let parsed_globals = match &globals_bytes {
            Some(bytes) if !bytes.is_empty() => {
                Some(Globals::new(bytes, &settings).map_err(|e| Error::Codec {
                    codec: ImageCodec::Jbig2,
                    detail: format!("JBIG2Globals decode failed: {e:?}"),
                })?)
            }
            _ => None,
        };

        if data.starts_with(&JBIG2_FILE_HEADER) {
            let file = File::new(data, &settings).map_err(|e| Error::Codec {
                codec: ImageCodec::Jbig2,
                detail: format!("{e:?}"),
            })?;
            let page = file.page(0).ok_or_else(|| Error::Codec {
                codec: ImageCodec::Jbig2,
                detail: "JBIG2 file contains no pages".to_string(),
            })?;
            decode_page(&page, limits)
        } else {
            let page =
                Page::new_embedded(data, parsed_globals.as_ref(), &settings).map_err(|e| {
                    Error::Codec {
                        codec: ImageCodec::Jbig2,
                        detail: format!("{e:?}"),
                    }
                })?;
            decode_page(&page, limits)
        }
    }));

    let decoded = match run {
        Ok(res) => res?,
        Err(_) => {
            return Err(Error::Codec {
                codec: ImageCodec::Jbig2,
                detail: "internal JBIG2 decoder panic caught".to_string(),
            });
        }
    };

    if decoded.width != dict_width || decoded.height != dict_height {
        sink.fault(place, Fault::DimensionMismatch);
    }

    Ok(decoded)
}

fn decode_page(page: &Page<'_>, limits: &Limits) -> Result<Jbig2Decoded, Error> {
    let w = page.width();
    let h = page.height();
    check_dimensions(w, h, limits)?;

    let stride = (w as usize).div_ceil(8);
    let total_bytes = check_sample_budget((stride as u64) * u64::from(h), limits)?;
    let mut writer = PackedJbig2Sink {
        width: w,
        stride,
        row: 0,
        col: 0,
        data: vec![0_u8; total_bytes],
    };

    page.decode(&mut writer).map_err(|e| Error::Codec {
        codec: ImageCodec::Jbig2,
        detail: format!("{e:?}"),
    })?;

    Ok(Jbig2Decoded {
        width: w,
        height: h,
        samples: SampleData::Packed {
            bpc: 1,
            stride,
            data: writer.data,
        },
    })
}

struct PackedJbig2Sink {
    width: u32,
    stride: usize,
    row: usize,
    col: u32,
    data: Vec<u8>,
}

impl hayro_jbig2::Decoder for PackedJbig2Sink {
    #[inline]
    fn push_pixel(&mut self, black: bool) {
        if self.col < self.width {
            let row_start = self.row * self.stride;
            if row_start < self.data.len() {
                // ISO 32000-2 §7.4.7: `0` = foreground (black), `1` = background (white).
                if !black {
                    let byte_idx = row_start + (self.col as usize >> 3);
                    let shift = 7 - (self.col & 7);
                    self.data[byte_idx] |= 1_u8 << shift;
                }
            }
            self.col += 1;
        }
    }

    #[inline]
    fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32) {
        let pixels = chunk_count.saturating_mul(8);
        let avail = self.width.saturating_sub(self.col);
        let take_chunks = (pixels.min(avail) / 8) as usize;
        if take_chunks > 0 {
            let row_start = self.row * self.stride;
            let byte_offset = row_start + (self.col as usize >> 3);
            if byte_offset + take_chunks <= self.data.len() {
                let fill = if black { 0x00_u8 } else { 0xFF_u8 };
                self.data[byte_offset..byte_offset + take_chunks].fill(fill);
            }
            self.col += (take_chunks as u32) * 8;
        }
    }

    #[inline]
    fn next_line(&mut self) {
        self.row += 1;
        self.col = 0;
    }
}
