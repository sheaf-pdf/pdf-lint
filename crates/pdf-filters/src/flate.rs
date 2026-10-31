//! `FlateDecode`'s zlib data (RFC 1950), before its predictor, inflated and
//! deflated by the backend a caller names: each in a module of its own,
//! behind its feature.

#[cfg(feature = "miniz_oxide")]
mod miniz;
#[cfg(feature = "zlib-rs")]
mod zlib;

use crate::{Backend, Fault, Level, Out, Prefix};

/// `data` decompressed by `backend`: a zlib header, deflate data and its
/// checksum. As in qpdf, a checksum that doesn't match is read as the end,
/// and data that ends early keeps what decompressed. What of `data` it
/// read, as zlib counts it.
pub(crate) fn decode(data: &[u8], backend: Backend, out: &mut Out) -> Prefix {
    if !header(data) {
        out.fault(Fault::Header);
        return Prefix::unended(refused(data));
    }
    // Grown as it fills, to a byte past the limit, so that data of just
    // the limit's length decompresses whole; what's written is the front.
    let most = out.limit.saturating_add(1);
    let start = out.data.len();
    out.data.resize(start + data.len().max(64).min(most), 0);
    let (mut written, mut fault, prefix): (usize, Option<Fault>, Prefix) = match backend {
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
    prefix
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
    deflate(cmf, flg) && flg & 0x20 == 0
}

/// Whether a zlib header's two bytes are one for deflate data with a
/// window zlib takes, its check right, whatever it says of a preset
/// dictionary.
fn deflate(cmf: u8, flg: u8) -> bool {
    let deflate = cmf & 0x0F == 8 && cmf >> 4 <= 7;
    let check = (u16::from(cmf) << 8 | u16::from(flg)) % 31 == 0;
    deflate && check
}

/// How many bytes of `data`, whose header [`header`] refuses, zlib reads
/// before it refuses it: the header's two, or as many as there are; but
/// where only its preset dictionary is refused, none once the four bytes
/// after it that name the dictionary are there, as zlib then stops for the
/// dictionary without counting what it read, and as many as there are
/// before.
fn refused(data: &[u8]) -> usize {
    match *data {
        [cmf, flg, ..] if deflate(cmf, flg) => match data.len() {
            ..6 => data.len(),
            _ => 0,
        },
        [_, _, ..] => 2,
        _ => data.len(),
    }
}

#[cfg(test)]
mod tests {
    use crate::fuzz::{backends, prefix, zlib};
    use crate::{Backend, Fault, Filter, Layout, Level, Predictor, decode, decode_prefix_into};

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

    /// `data` read as data that may run on, by `backend`: how far, whether
    /// it ended, how much it gave and its faults.
    fn read(data: &[u8], backend: Backend) -> (usize, bool, usize, Vec<Fault>) {
        let (prefix, out, faults) = prefix(data, FLATE, backend);
        (prefix.read, prefix.ended, out.len(), faults)
    }

    /// Zlib data that runs on is read to its checksum's last byte,
    /// matching or not, on each backend; and data whose deflate stream is
    /// whole but its checksum isn't takes what follows as the checksum.
    #[test]
    fn what_it_read_is_to_its_checksum() {
        let text = b"one two three two one, three two one two three; ".repeat(400);
        for backend in backends() {
            let after = |data: &[u8]| [data, b" EI Q", &text].concat();
            assert_eq!(read(&after(&HELLO), backend), (16, true, 23, vec![]));
            let mut damaged = HELLO;
            damaged[15] ^= 1;
            let checksum = vec![Fault::Checksum];
            assert_eq!(
                read(&after(&damaged), backend),
                (16, true, 23, checksum.clone())
            );
            assert_eq!(
                read(&after(&HELLO[..12]), backend),
                (16, true, 23, checksum)
            );
            for level in [Level::NONE, Level::DEFAULT, Level::BEST] {
                for deflater in backends() {
                    let compressed = zlib(&text, level, deflater);
                    let want = (compressed.len(), true, text.len(), vec![]);
                    assert_eq!(read(&after(&compressed), backend), want, "{level:?}");
                }
            }
        }
    }

    /// Data that ends early is read to its end; data that's corrupt, to the
    /// byte it was found corrupt in; a header it doesn't take, to what zlib
    /// reads of it before it refuses it: two bytes, or as many as there
    /// are, or none where it asks for a preset dictionary that four bytes
    /// after it name.
    #[test]
    fn what_it_read_short_of_its_end() {
        for backend in backends() {
            let truncated = vec![Fault::Truncated];
            let (read_to, ended, _, faults) = read(&HELLO[..10], backend);
            assert_eq!((read_to, ended, faults), (10, false, truncated));
            // A block of the reserved type, 11, in the third byte.
            let data = [0x78, 0x9c, 0x07, 0x00, b' ', b'E', b'I'];
            let corrupt = vec![Fault::Corrupt { at: 2 }];
            assert_eq!(read(&data, backend), (3, false, 0, corrupt));
            // A stored block whose lengths don't match, read whole.
            let data = b"\x78\x01\x01\x05\x00\x00\x00abcde EI";
            let corrupt = vec![Fault::Corrupt { at: 6 }];
            assert_eq!(read(data, backend), (7, false, 0, corrupt));
            let header = vec![Fault::Header];
            for (data, n) in [
                (&b"\x78\x02\x03\x00 EI"[..], 2),
                (b"\x79\x9d\x03\x00", 2),
                (b"\x88\x98\x03\x00", 2),
                (b"x", 1),
                (b"x\x20abcd\x03\x00 EI", 0),
                (b"x\x20abc", 5),
            ] {
                assert_eq!(
                    read(data, backend),
                    (n, false, 0, header.clone()),
                    "{data:?}"
                );
            }
        }
    }

    /// At the limit, it's read as far as the backend read to give a byte
    /// over it, its end unread.
    #[test]
    fn what_it_read_at_the_limit() {
        let data = vec![0; 1_000_000];
        for backend in backends() {
            let bomb = zlib(&data, Level::BEST, backend);
            for limit in [5, 100_000] {
                let mut out = Vec::new();
                let (prefix, decoded) = decode_prefix_into(&bomb, FLATE, limit, backend, &mut out);
                assert!(!prefix.ended && prefix.read < bomb.len(), "{prefix:?}");
                assert_eq!(
                    (out.len(), decoded.unwrap_err().0),
                    (limit, vec![(0, Fault::Limit)])
                );
            }
            // Its end read, its output one byte over: ended.
            let mut out = Vec::new();
            let (prefix, _) = decode_prefix_into(&HELLO, FLATE, 22, backend, &mut out);
            assert_eq!((prefix.read, prefix.ended, out.len()), (16, true, 22));
        }
    }

    /// The two backends read alike: on every Flate probe of an inline
    /// image's data, clean, corrupt, cut short and followed by content, the
    /// count zlib's `total_in` gives. They part where miniz_oxide refuses
    /// what zlib reads on past: a dynamic block whose code-length code has
    /// no lengths, which zlib reads as code lengths of nothing, one bit
    /// each, before it finds no end-of-block code.
    #[test]
    fn the_backends_read_alike_but_where_miniz_oxide_refuses_sooner() {
        let text = b"one two three two one, three two one two three; ".repeat(40);
        let mut inputs = vec![HELLO.to_vec()];
        for deflater in backends() {
            let compressed = zlib(&text, Level::BEST, deflater);
            for at in (0..compressed.len()).step_by(7) {
                for flip in [0x01, 0x10, 0xff] {
                    let mut damaged = compressed.clone();
                    damaged[at] ^= flip;
                    inputs.push([&damaged[..], b" EI Q"].concat());
                }
            }
            inputs.push(compressed[..compressed.len() / 2].to_vec());
        }
        let first = backends()[0];
        for input in &inputs {
            for backend in backends() {
                assert_eq!(read(input, backend).0, read(input, first).0, "{backend:?}");
            }
        }
        // BFINAL 1, BTYPE 2, HLIT 0, HDIST 0, HCLEN 0, and four lengths of 0.
        #[cfg(all(feature = "miniz_oxide", feature = "zlib-rs"))]
        {
            let data = [&b"\x78\x01\x05\x00\x00"[..], &[0; 25]].concat();
            let truncated = (30, false, 0, vec![Fault::Truncated]);
            assert_eq!(read(&data, Backend::ZlibRs), truncated);
            let corrupt = (6, false, 0, vec![Fault::Corrupt { at: 5 }]);
            assert_eq!(read(&data, Backend::MinizOxide), corrupt);
        }
    }

    /// A predictor after it changes nothing of what it read.
    #[test]
    fn a_predictor_after_it_reads_alike() {
        let png = Filter::Flate {
            predictor: Predictor::Png(Layout {
                colors: 1,
                bits_per_component: 8,
                columns: 2,
            }),
        };
        for backend in backends() {
            let rows = zlib(&[1, 1, 1, 2, 1, 1], Level::DEFAULT, backend);
            let data = [&rows[..], b" EI"].concat();
            let (prefix, out, faults) = prefix(&data, png, backend);
            assert_eq!((prefix.read, prefix.ended), (rows.len(), true));
            assert_eq!((out, faults), (vec![1, 2, 2, 3], vec![]));
        }
    }
}
