//! miniz_oxide's inflater and deflater, through its calls on slices.

use miniz_oxide::DataFormat;
use miniz_oxide::deflate::core::{CompressorOxide, TDEFLFlush, TDEFLStatus, compress};
use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};

use super::grow;
use crate::Fault;

/// `data` inflated into `out` past `start`, which it grows to at most
/// `most` bytes: how many it wrote, and the fault that stopped it, if one
/// did.
pub(super) fn inflate(
    data: &[u8],
    out: &mut Vec<u8>,
    start: usize,
    most: usize,
) -> (usize, Option<Fault>) {
    // What's written is the window it reads matches from.
    let flags = inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER
        | inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
    let mut state = Box::<DecompressorOxide>::default();
    let (mut read, mut written) = (0, 0);
    let fault = loop {
        let (status, used, made) =
            decompress(&mut state, &data[read..], &mut out[start..], written, flags);
        read += used;
        written += made;
        match status {
            TINFLStatus::Done => break None,
            TINFLStatus::HasMoreOutput => {
                if !grow(out, start, most) {
                    break Some(Fault::Limit);
                }
            }
            TINFLStatus::FailedCannotMakeProgress | TINFLStatus::NeedsMoreInput => {
                break Some(Fault::Truncated);
            }
            TINFLStatus::Adler32Mismatch => break Some(Fault::Checksum),
            // Failed, BadParam, and whatever a later version adds; the
            // decompressor takes a byte at a time, so the fault's in the
            // last it took.
            _ => {
                break Some(Fault::Corrupt {
                    at: read.saturating_sub(1),
                });
            }
        }
    };
    (written, fault)
}

/// `data` deflated at `level` into zlib data in `out` past `start`, which
/// it grows as it fills: how many bytes it wrote. The compressor is set
/// as flate2 sets it.
pub(super) fn deflate(data: &[u8], level: u8, out: &mut Vec<u8>, start: usize) -> usize {
    // Half the data's length, a guess for most data, up to a MiB, so that
    // a stream that compresses well doesn't take much more.
    out.resize(start + (data.len() / 2).clamp(64, 1 << 20), 0);
    let mut state = Box::<CompressorOxide>::default();
    state.set_format_and_level(DataFormat::Zlib, level);
    let (mut read, mut written) = (0, 0);
    loop {
        let (status, used, made) = compress(
            &mut state,
            &data[read..],
            &mut out[start + written..],
            TDEFLFlush::Finish,
        );
        read += used;
        written += made;
        match status {
            TDEFLStatus::Done => return written,
            // More to write than there's room for.
            TDEFLStatus::Okay => {
                grow(out, start, usize::MAX);
            }
            // A call after it's done, a buffer it can't write to, and the
            // failure of a callback, which this isn't given.
            other => unreachable!("deflating into a buffer with room: {other:?}"),
        }
    }
}
