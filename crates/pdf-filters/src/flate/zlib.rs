//! zlib-rs's inflater and deflater, through its safe API.

use zlib_rs::{Deflate, DeflateFlush, Inflate, InflateError, InflateFlush, Status};

use super::grow;
use crate::Fault;

/// The most zlib takes or gives in a call: a C `unsigned int`'s worth.
const CALL: usize = u32::MAX as usize;

/// `data` inflated into `out` past `start`, which it grows to at most
/// `most` bytes: how many it wrote, and the fault that stopped it, if one
/// did.
pub(super) fn inflate(
    data: &[u8],
    out: &mut Vec<u8>,
    start: usize,
    most: usize,
) -> (usize, Option<Fault>) {
    let mut state = Inflate::new(true, 15);
    let fault = loop {
        let (read, written) = (state.total_in() as usize, state.total_out() as usize);
        let (input, end) = (read.saturating_add(CALL).min(data.len()), out.len());
        let room = (start + written).saturating_add(CALL).min(end);
        let status = state.decompress(
            &data[read..input],
            &mut out[start + written..room],
            InflateFlush::NoFlush,
        );
        let (read, written) = (state.total_in() as usize, state.total_out() as usize);
        match status {
            Ok(Status::StreamEnd) => break None,
            // It stops when the room it was given is full, or the input
            // is spent: when it's all spent, the data ends early.
            Ok(Status::Ok | Status::BufError) => {
                if start + written == end {
                    if !grow(out, start, most) {
                        break Some(Fault::Limit);
                    }
                } else if start + written < room && read == data.len() {
                    break Some(Fault::Truncated);
                }
            }
            // zlib checks the checksum after writing everything.
            Err(InflateError::DataError)
                if state.error_message() == Some("incorrect data check") =>
            {
                break Some(Fault::Checksum);
            }
            // Data that isn't deflate data, and what can't happen to a zlib
            // header without a preset dictionary (`NeedDict`) or to a
            // stream made here (`StreamError`, `MemError`). It reads whole
            // bytes ahead, and gives back those it didn't use, so the fault
            // is in the last it kept.
            Err(_) => {
                break Some(Fault::Corrupt {
                    at: read.saturating_sub(1),
                });
            }
        }
    };
    (state.total_out() as usize, fault)
}

/// `data` deflated at `level` into zlib data in `out` past `start`, which
/// it grows as it fills: how many bytes it wrote. Window, memory level and
/// strategy are zlib's defaults, and the calls are flate2's, as its
/// `ZlibEncoder` makes them: the data given until it's all taken, then
/// finished, each call with 32 KiB to write into, its buffer's room. Where
/// that room runs out changes what zlib-rs writes (at level 6, its parse
/// starts again there), so the bytes are flate2's only with the same.
pub(super) fn deflate(data: &[u8], level: u8, out: &mut Vec<u8>, start: usize) -> usize {
    const ROOM: usize = 32 * 1024;
    let mut state = Deflate::new(i32::from(level), true, 15);
    loop {
        let (read, written) = (state.total_in() as usize, state.total_out() as usize);
        let end = start + written + ROOM;
        // At once, to the room it needs or twice what it had.
        if out.len() < end {
            out.resize(end.max(start + 2 * (out.len() - start)), 0);
        }
        let flush = match read < data.len() {
            true => DeflateFlush::NoFlush,
            false => DeflateFlush::Finish,
        };
        // zlib-rs takes a C `unsigned int`'s worth at a call, and the rest
        // at the next.
        match state.compress(&data[read..], &mut out[start + written..end], flush) {
            Ok(Status::StreamEnd) => return state.total_out() as usize,
            Ok(Status::Ok | Status::BufError) => {}
            // A call after it's done, and allocation failing.
            Err(e) => unreachable!("deflating into a buffer with room: {e:?}"),
        }
    }
}
