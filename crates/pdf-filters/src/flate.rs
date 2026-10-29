//! `FlateDecode`'s zlib data (RFC 1950), before its predictor, inflated and
//! deflated by the backend a caller names: each in a module of its own,
//! behind its feature.

#[cfg(feature = "miniz_oxide")]
mod miniz;
#[cfg(feature = "zlib-rs")]
mod zlib;

use crate::{Backend, Fault, Level, Out};

/// `data` decompressed by `backend`: a zlib header, deflate data and its
/// checksum. As in qpdf, a checksum that doesn't match is read as the end,
/// and data that ends early keeps what decompressed.
pub(crate) fn decode(data: &[u8], backend: Backend, out: &mut Out) {
    if !header(data) {
        out.fault(Fault::Header);
        return;
    }
    // Grown as it fills, to a byte past the limit, so that data of just
    // the limit's length decompresses whole; what's written is the front.
    let most = out.limit.saturating_add(1);
    let start = out.data.len();
    out.data.resize(start + data.len().max(64).min(most), 0);
    let (mut written, mut fault): (usize, Option<Fault>) = match backend {
        #[cfg(feature = "miniz_oxide")]
        Backend::MinizOxide => miniz::inflate(data, &mut out.data, start, most),
        #[cfg(feature = "zlib-rs")]
        Backend::ZlibRs => zlib::inflate(data, &mut out.data, start, most),
    };
    if written > out.limit {
        written = out.limit;
        fault = Some(Fault::Limit);
    }
    out.data.truncate(start + written);
    if let Some(fault) = fault {
        out.fault(fault);
    }
}

/// `data` compressed by `backend` at `level` into zlib data, appended to
/// `out`: a window of 32 KiB, zlib's memory level 8 and its default
/// strategy, each backend set and called as flate2 sets and calls it.
pub(crate) fn encode(data: &[u8], level: Level, backend: Backend, out: &mut Vec<u8>) {
    let start = out.len();
    // Each backend makes room as it fills.
    let written: usize = match backend {
        #[cfg(feature = "miniz_oxide")]
        Backend::MinizOxide => miniz::deflate(data, level.get(), out, start),
        #[cfg(feature = "zlib-rs")]
        Backend::ZlibRs => zlib::deflate(data, level.get(), out, start),
    };
    out.truncate(start + written);
}

/// `data`'s room past `start` doubled, to `most` bytes at most: false when
/// it's there already. What's added is zeros.
fn grow(data: &mut Vec<u8>, start: usize, most: usize) -> bool {
    let room = data.len() - start;
    if room >= most {
        return false;
    }
    data.resize(start.saturating_add(room.saturating_mul(2).min(most)), 0);
    true
}

/// Whether `data` begins with a zlib header for deflate data without a
/// preset dictionary (RFC 1950, 2.2), which a PDF has no way to give.
fn header(data: &[u8]) -> bool {
    let [cmf, flg, ..] = *data else {
        return false;
    };
    let deflate = cmf & 0x0F == 8 && cmf >> 4 <= 7;
    let check = (u16::from(cmf) << 8 | u16::from(flg)) % 31 == 0;
    deflate && check && flg & 0x20 == 0
}

#[cfg(test)]
mod tests {
    use crate::fuzz::{backends, zlib};
    use crate::{Backend, Fault, Filter, Level, Predictor, decode};

    const FLATE: Filter = Filter::Flate {
        predictor: Predictor::None,
    };

    /// Python's zlib.compress(b"hello hello hello hello", 9).
    const HELLO: [u8; 16] = [
        0x78, 0xda, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc8, 0x40, 0x27, 0x01, 0x68, 0x03, 0x08,
        0xb1,
    ];

    fn flate(data: &[u8], backend: Backend) -> Result<Vec<u8>, crate::Error> {
        decode(data, &[FLATE], 1 << 24, backend)
    }

    #[test]
    fn zlib_data_decompresses() {
        for backend in backends() {
            assert_eq!(
                flate(&HELLO, backend).unwrap(),
                b"hello hello hello hello",
                "{backend:?}"
            );
        }
    }

    #[test]
    fn more_than_its_first_guess_decompresses_as_it_grows() {
        let data: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
        for backend in backends() {
            let compressed = zlib(&data, Level::DEFAULT, backend);
            assert_eq!(flate(&compressed, backend).unwrap(), data, "{backend:?}");
        }
    }

    #[test]
    fn a_checksum_that_doesnt_match_keeps_everything() {
        let mut data = HELLO;
        data[15] ^= 1;
        for backend in backends() {
            let error = flate(&data, backend).unwrap_err();
            assert_eq!(error.data, b"hello hello hello hello", "{backend:?}");
            assert_eq!(error.faults, [(0, Fault::Checksum)], "{backend:?}");
        }
    }

    #[test]
    fn data_that_ends_early_keeps_what_decompressed() {
        for backend in backends() {
            let error = flate(&HELLO[..10], backend).unwrap_err();
            assert!(
                b"hello hello hello hello".starts_with(&error.data),
                "{backend:?}: {error:?}"
            );
            assert_eq!(error.faults, [(0, Fault::Truncated)], "{backend:?}");
        }
    }

    #[test]
    fn corrupt_data_stops_it() {
        for backend in backends() {
            // A block of the reserved type, 11.
            let error = flate(&[0x78, 0x9c, 0x07, 0x00], backend).unwrap_err();
            assert_eq!(error.data, b"", "{backend:?}");
            assert!(
                matches!(error.faults[..], [(0, Fault::Corrupt { .. })]),
                "{backend:?}: {error:?}"
            );
        }
    }

    #[test]
    fn deflate_data_without_a_zlib_header_isnt_read() {
        for backend in backends() {
            for data in [
                &HELLO[2..],
                &[0x78][..],
                &[0x78, 0xdb],
                &[0x78, 0xbb, 0, 0, 0, 0],
            ] {
                let error = flate(data, backend).unwrap_err();
                assert_eq!(
                    (error.data, error.faults),
                    (Vec::new(), [(0, Fault::Header)].to_vec())
                );
            }
        }
    }

    #[test]
    fn data_of_just_the_limits_length_decompresses_whole() {
        let data = vec![7; 100_000];
        for backend in backends() {
            assert_eq!(
                decode(&HELLO, &[FLATE], 23, backend).unwrap(),
                b"hello hello hello hello"
            );
            let compressed = zlib(&data, Level::DEFAULT, backend);
            assert_eq!(
                decode(&compressed, &[FLATE], 100_000, backend).unwrap(),
                data
            );
        }
    }

    #[test]
    fn the_limit_stops_it() {
        let data = vec![0; 1_000_000];
        for backend in backends() {
            let error = decode(&HELLO, &[FLATE], 22, backend).unwrap_err();
            assert_eq!(error.data, b"hello hello hello hell");
            assert_eq!(error.faults, [(0, Fault::Limit)]);
            let error = decode(&HELLO, &[FLATE], 5, backend).unwrap_err();
            assert_eq!(error.data, b"hello");
            assert_eq!(error.faults, [(0, Fault::Limit)]);
            let bomb = zlib(&data, Level::BEST, backend);
            let error = decode(&bomb, &[FLATE], 100_000, backend).unwrap_err();
            assert_eq!(error.data.len(), 100_000);
            assert_eq!(error.faults, [(0, Fault::Limit)]);
        }
    }

    /// Data deflated at each level on each backend inflates to itself on
    /// every backend: what miniz_oxide deflates, zlib-rs inflates, and the
    /// other way.
    #[test]
    fn every_level_deflates_to_what_every_backend_inflates() {
        let text = b"one two three two one, three two one two three; ".repeat(400);
        let noise: Vec<u8> = (0..70_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let runs = vec![9; 200_000];
        for data in [&b""[..], b"a", &text, &noise, &runs] {
            for level in (0..=9).map(|n| Level::new(n).unwrap()) {
                for backend in backends() {
                    let compressed = zlib(data, level, backend);
                    for inflater in backends() {
                        assert_eq!(
                            decode(&compressed, &[FLATE], data.len(), inflater).as_deref(),
                            Ok(data),
                            "{} bytes at {level:?} by {backend:?}, inflated by {inflater:?}",
                            data.len()
                        );
                    }
                }
            }
        }
    }

    /// Level 0 stores the data, so it's a few bytes longer; 9 makes what
    /// compresses at least as small as 6 does, and 6 smaller than 1.
    #[test]
    fn the_levels_go_from_stored_to_smallest() {
        let text = b"one two three two one, three two one two three; ".repeat(400);
        for backend in backends() {
            let at = |level| zlib(&text, Level::new(level).unwrap(), backend).len();
            assert!(at(0) > text.len(), "{backend:?}");
            assert!(at(9) <= at(6) && at(6) < at(1), "{backend:?}");
        }
        assert_eq!(Level::default(), Level::DEFAULT);
        assert_eq!(
            [Level::NONE, Level::DEFAULT, Level::BEST].map(Level::get),
            [0, 6, 9]
        );
        assert_eq!(Level::new(10), None);
    }

    /// Deflated data a fault cut off on one backend inflates on the other
    /// alike in its fault, if not in how much came out first.
    #[test]
    fn a_stream_cut_short_is_truncated_on_every_backend() {
        let text = b"one two three two one, three two one two three; ".repeat(400);
        for backend in backends() {
            let compressed = zlib(&text, Level::DEFAULT, backend);
            for inflater in backends() {
                for cut in [3, compressed.len() / 2, compressed.len() - 1] {
                    let error = flate(&compressed[..cut], inflater).unwrap_err();
                    assert!(text.starts_with(&error.data), "{backend:?}, {inflater:?}");
                    assert_eq!(error.faults, [(0, Fault::Truncated)], "{inflater:?}, {cut}");
                }
            }
        }
    }

    /// zlib-rs deflates at 6 as flate2's `ZlibEncoder` has it deflate,
    /// given 32 KiB to write into at a call: where its room runs out
    /// changes what it writes, and given room for the whole it writes
    /// 57,354 bytes of this data. These are flate2's 57,353, by length and
    /// FNV-1a.
    #[cfg(feature = "zlib-rs")]
    #[test]
    fn zlib_rs_deflates_as_flate2_has_it_deflate() {
        let mut data = Vec::new();
        for i in 1..=5200u32 {
            let x = i.wrapping_mul(2_654_435_761) >> 20;
            let (y, contents) = (x % 997, i * 3);
            let object = format!(
                "{i} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {x} {y}] /Contents {contents} 0 R >>\nendobj\n"
            );
            data.extend_from_slice(object.as_bytes());
        }
        let deflated = zlib(&data, Level::DEFAULT, Backend::ZlibRs);
        let fnv = (deflated.iter()).fold(0xcbf2_9ce4_8422_2325_u64, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        });
        assert_eq!((deflated.len(), fnv), (57_353, 0xd49f_1e1c_1c94_b7b6));
        assert_eq!(
            decode(&deflated, &[FLATE], data.len(), Backend::ZlibRs),
            Ok(data)
        );
    }
}
