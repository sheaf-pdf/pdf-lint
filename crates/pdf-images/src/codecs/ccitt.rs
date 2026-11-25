//! `CCITTFaxDecode` (ITU-T T.4 / T.6 Group 3 & Group 4) adapter over `hayro-ccitt`.

use std::panic::{AssertUnwindSafe, catch_unwind};

use hayro_ccitt::{DecodeSettings, DecoderContext, EncodingMode};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Dict, Resolve};

use crate::error::Error;
use crate::fault::Fault;
use crate::header::{ImageCodec, check_dimensions, check_sample_budget, read_bool, read_int};
use crate::limits::Limits;
use crate::samples::SampleData;

const FAX_MAX_DIM: u32 = 65_535;

/// Decodes a `CCITTFaxDecode` stream into 1-bit packed [`SampleData`].
///
/// Follows ISO 32000-2 §7.4.6 Table 11:
/// - When `/BlackIs1` is `false` (default), black pixels produce sample `0` and white pixels
///   produce sample `1`.
/// - When `/BlackIs1` is `true`, black pixels produce sample `1` and white pixels produce sample
///   `0`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_ccitt<'o, R, S>(
    objects: R,
    data: &[u8],
    parms: Option<&Dict>,
    width: u32,
    height: u32,
    limits: &Limits,
    place: Place,
    sink: &mut S,
) -> Result<SampleData, Error>
where
    R: Resolve<'o> + Copy,
    S: Sink<Fault> + ?Sized,
{
    check_dimensions(width, height, limits)?;
    if width > FAX_MAX_DIM || height > FAX_MAX_DIM {
        return Err(Error::Codec {
            codec: ImageCodec::CcittFax,
            detail: format!("dimensions ({width}x{height}) exceed CCITT maximum ({FAX_MAX_DIM})"),
        });
    }

    let k = match parms {
        Some(d) => read_int(objects, d, b"K")?.unwrap_or(0),
        None => 0,
    };
    let end_of_line = match parms {
        Some(d) => read_bool(objects, d, b"EndOfLine")?.unwrap_or(false),
        None => false,
    };
    let byte_align = match parms {
        Some(d) => read_bool(objects, d, b"EncodedByteAlign")?.unwrap_or(false),
        None => false,
    };
    let columns_raw = match parms {
        Some(d) => read_int(objects, d, b"Columns")?.unwrap_or(1728),
        None => 1728,
    };
    let rows_raw = match parms {
        Some(d) => read_int(objects, d, b"Rows")?.unwrap_or(0),
        None => 0,
    };
    let end_of_block = match parms {
        Some(d) => read_bool(objects, d, b"EndOfBlock")?.unwrap_or(true),
        None => true,
    };
    let black_is_1 = match parms {
        Some(d) => read_bool(objects, d, b"BlackIs1")?.unwrap_or(false),
        None => false,
    };
    let damaged_allowed = match parms {
        Some(d) => read_int(objects, d, b"DamagedRowsBeforeError")?.unwrap_or(0),
        None => 0,
    };

    let columns = if columns_raw == 0 {
        width
    } else if (1..=i64::from(FAX_MAX_DIM)).contains(&columns_raw) {
        columns_raw as u32
    } else {
        return Err(Error::Codec {
            codec: ImageCodec::CcittFax,
            detail: format!("invalid /Columns ({columns_raw})"),
        });
    };
    if columns > limits.max_width {
        return Err(Error::Limit {
            kind: crate::limits::LimitKind::Width,
            value: u64::from(columns),
            max: u64::from(limits.max_width),
        });
    }

    let rows = if rows_raw <= 0 || rows_raw > i64::from(FAX_MAX_DIM) {
        height
    } else {
        rows_raw as u32
    };

    let stride = (width as usize).div_ceil(8);
    let total_bytes = check_sample_budget((stride as u64) * u64::from(height), limits)?;

    let settings = DecodeSettings {
        columns,
        rows,
        end_of_block,
        end_of_line,
        rows_are_byte_aligned: byte_align,
        encoding: if k < 0 {
            EncodingMode::Group4
        } else if k == 0 {
            EncodingMode::Group3_1D
        } else {
            EncodingMode::Group3_2D
        },
        strict: false,
    };

    let default_byte = if black_is_1 { 0x00_u8 } else { 0xFF_u8 };
    let tail_bits = (width & 7) as u8;
    let last_byte_mask = if tail_bits == 0 {
        0xFF_u8
    } else {
        !((1_u8 << (8 - tail_bits)) - 1)
    };

    let mut row_sink = PackedRowSink {
        width,
        height,
        stride,
        black_is_1,
        last_byte_mask,
        rows_pushed: 0,
        data: vec![default_byte; total_bytes],
    };
    if tail_bits != 0 {
        for r in 0..(height as usize) {
            row_sink.data[(r + 1) * stride - 1] &= last_byte_mask;
        }
    }

    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let mut ctx = DecoderContext::new(settings);
        hayro_ccitt::decode(data, &mut row_sink, &mut ctx)
    }));

    match outcome {
        Ok(Ok(_)) => {
            if row_sink.rows_pushed < height {
                sink.fault(place, Fault::TruncatedFax);
            }
            Ok(SampleData::Packed {
                bpc: 1,
                stride,
                data: row_sink.data,
            })
        }
        Ok(Err(err)) => {
            if row_sink.rows_pushed > 0 || damaged_allowed > 0 {
                sink.fault(place, Fault::TruncatedFax);
                Ok(SampleData::Packed {
                    bpc: 1,
                    stride,
                    data: row_sink.data,
                })
            } else {
                Err(Error::Codec {
                    codec: ImageCodec::CcittFax,
                    detail: format!("{err:?}"),
                })
            }
        }
        Err(_) => Err(Error::Codec {
            codec: ImageCodec::CcittFax,
            detail: "internal CCITT decoder panic caught".to_string(),
        }),
    }
}

struct PackedRowSink {
    width: u32,
    height: u32,
    stride: usize,
    black_is_1: bool,
    last_byte_mask: u8,
    rows_pushed: u32,
    data: Vec<u8>,
}

impl hayro_ccitt::Decoder for PackedRowSink {
    fn push_row(&mut self, changes: &[u32]) {
        if self.rows_pushed >= self.height {
            return;
        }
        let y = self.rows_pushed as usize;
        self.rows_pushed += 1;
        let row = &mut self.data[y * self.stride..(y + 1) * self.stride];

        // `changes` alternates white -> black at even indices and black -> white at odd indices.
        let mut idx = 0;
        while idx < changes.len() {
            let start = changes[idx].min(self.width);
            let end = changes
                .get(idx + 1)
                .copied()
                .unwrap_or(self.width)
                .min(self.width);
            if start < end {
                set_bit_run(row, start as usize, end as usize, self.black_is_1);
            }
            idx += 2;
        }
        if let Some(last) = row.last_mut() {
            *last &= self.last_byte_mask;
        }
    }
}

fn set_bit_run(row: &mut [u8], start: usize, end: usize, bit_one: bool) {
    let first_byte = start >> 3;
    let last_byte = (end - 1) >> 3;
    let start_mask = 0xFF_u8 >> (start & 7);
    let end_shift = 8 - ((end - 1) & 7) - 1;
    let end_mask = !((1_u8 << end_shift) - 1);

    if first_byte == last_byte {
        let mask = start_mask & end_mask;
        if bit_one {
            row[first_byte] |= mask;
        } else {
            row[first_byte] &= !mask;
        }
        return;
    }

    if bit_one {
        row[first_byte] |= start_mask;
        row[first_byte + 1..last_byte].fill(0xFF);
        row[last_byte] |= end_mask;
    } else {
        row[first_byte] &= !start_mask;
        row[first_byte + 1..last_byte].fill(0x00);
        row[last_byte] &= !end_mask;
    }
}
