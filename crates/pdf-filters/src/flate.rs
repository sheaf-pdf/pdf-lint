//! `FlateDecode`'s zlib data (RFC 1950), before its predictor. The one
//! place the decompressor is named, so that another could take its place.

use crate::{Fault, Out};
use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};

/// `data` decompressed: a zlib header, deflate data and its checksum.
/// As in qpdf, a checksum that doesn't match is read as the end, and
/// data that ends early keeps what decompressed.
pub(crate) fn decode(data: &[u8], out: &mut Out) {
    if !header(data) {
        out.fault(Fault::Header);
        return;
    }
    let flags = inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER
        | inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
    let mut state = Box::<DecompressorOxide>::default();
    // Grown as it fills, to a byte past the limit, so that data of just
    // the limit's length decompresses whole; what's written is the front.
    let most = out.limit.saturating_add(1);
    let mut buffer = vec![0; data.len().saturating_mul(4).max(64).min(most)];
    let (mut read, mut written) = (0, 0);
    let mut fault = loop {
        let (status, used, made) =
            decompress(&mut state, &data[read..], &mut buffer, written, flags);
        read += used;
        written += made;
        match status {
            TINFLStatus::Done => break None,
            TINFLStatus::HasMoreOutput if buffer.len() < most => {
                buffer.resize(buffer.len().saturating_mul(2).min(most), 0);
            }
            TINFLStatus::HasMoreOutput => break Some(Fault::Limit),
            TINFLStatus::FailedCannotMakeProgress | TINFLStatus::NeedsMoreInput => {
                break Some(Fault::Truncated);
            }
            TINFLStatus::Adler32Mismatch => break Some(Fault::Checksum),
            // Failed, BadParam, and whatever a later version adds.
            _ => break Some(Fault::Corrupt { at: read }),
        }
    };
    if written > out.limit {
        written = out.limit;
        fault = Some(Fault::Limit);
    }
    buffer.truncate(written);
    buffer.shrink_to_fit();
    out.data = buffer;
    if let Some(fault) = fault {
        out.fault(fault);
    }
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
    use crate::{Fault, Filter, Predictor, decode};

    const FLATE: Filter = Filter::Flate {
        predictor: Predictor::None,
    };

    /// Python's zlib.compress(b"hello hello hello hello", 9).
    const HELLO: [u8; 16] = [
        0x78, 0xda, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc8, 0x40, 0x27, 0x01, 0x68, 0x03, 0x08,
        0xb1,
    ];

    fn flate(data: &[u8]) -> Result<Vec<u8>, crate::Error> {
        decode(data, &[FLATE], 1 << 24)
    }

    #[test]
    fn zlib_data_decompresses() {
        assert_eq!(flate(&HELLO).unwrap(), b"hello hello hello hello");
    }

    #[test]
    fn more_than_its_first_guess_decompresses_as_it_grows() {
        let data: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&data, 6);
        assert_eq!(flate(&compressed).unwrap(), data);
    }

    #[test]
    fn a_checksum_that_doesnt_match_keeps_everything() {
        let mut data = HELLO;
        data[15] ^= 1;
        let error = flate(&data).unwrap_err();
        assert_eq!(error.data, b"hello hello hello hello");
        assert_eq!(error.faults, [(0, Fault::Checksum)]);
    }

    #[test]
    fn data_that_ends_early_keeps_what_decompressed() {
        let error = flate(&HELLO[..10]).unwrap_err();
        assert!(
            b"hello hello hello hello".starts_with(&error.data),
            "{error:?}"
        );
        assert_eq!(error.faults, [(0, Fault::Truncated)]);
    }

    #[test]
    fn corrupt_data_stops_it() {
        // A block of the reserved type, 11.
        let error = flate(&[0x78, 0x9c, 0x07, 0x00]).unwrap_err();
        assert_eq!(error.data, b"");
        assert!(
            matches!(error.faults[..], [(0, Fault::Corrupt { .. })]),
            "{error:?}"
        );
    }

    #[test]
    fn deflate_data_without_a_zlib_header_isnt_read() {
        for data in [
            &HELLO[2..],
            &[0x78][..],
            &[0x78, 0xdb],
            &[0x78, 0xbb, 0, 0, 0, 0],
        ] {
            let error = flate(data).unwrap_err();
            assert_eq!(
                (error.data, error.faults),
                (Vec::new(), [(0, Fault::Header)].to_vec())
            );
        }
    }

    #[test]
    fn data_of_just_the_limits_length_decompresses_whole() {
        assert_eq!(
            decode(&HELLO, &[FLATE], 23).unwrap(),
            b"hello hello hello hello"
        );
        let data = vec![7; 100_000];
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&data, 6);
        assert_eq!(decode(&compressed, &[FLATE], 100_000).unwrap(), data);
    }

    #[test]
    fn the_limit_stops_it() {
        let error = decode(&HELLO, &[FLATE], 22).unwrap_err();
        assert_eq!(error.data, b"hello hello hello hell");
        assert_eq!(error.faults, [(0, Fault::Limit)]);
        let error = decode(&HELLO, &[FLATE], 5).unwrap_err();
        assert_eq!(error.data, b"hello");
        assert_eq!(error.faults, [(0, Fault::Limit)]);
        let data = vec![0; 1_000_000];
        let bomb = miniz_oxide::deflate::compress_to_vec_zlib(&data, 9);
        let error = decode(&bomb, &[FLATE], 100_000).unwrap_err();
        assert_eq!(error.data.len(), 100_000);
        assert_eq!(error.faults, [(0, Fault::Limit)]);
    }
}
