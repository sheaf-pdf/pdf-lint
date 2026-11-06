//! `ASCIIHexDecode` (ISO 32000-2, 7.4.2).

use crate::{Fault, Out, Prefix, is_white_space};

/// `data`'s hex digits as bytes, up to `>`, white-space ignored; an odd
/// last digit is as if a 0 followed it: what of `data` it read.
pub(crate) fn decode(data: &[u8], out: &mut Out) -> Prefix {
    let mut high = None;
    for (at, &byte) in data.iter().enumerate() {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            b'>' => {
                let _ = finish(high, out);
                return Prefix::ended(at + 1);
            }
            _ if is_white_space(byte) => continue,
            _ => {
                // What decoded is kept, but not a digit without its pair.
                out.fault(Fault::Byte { at, byte });
                return Prefix::unended(at + 1);
            }
        };
        high = match high {
            None => Some(digit),
            Some(high) => {
                if out.extend(&[high << 4 | digit]).is_err() {
                    return Prefix::unended(at + 1);
                }
                None
            }
        };
    }
    if finish(high, out).is_ok() {
        out.fault(Fault::NoEnd);
    }
    Prefix::unended(data.len())
}

/// An odd last digit, as if a 0 followed it.
fn finish(high: Option<u8>, out: &mut Out) -> Result<(), ()> {
    match high {
        Some(high) => out.extend(&[high << 4]),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use crate::fuzz::{backends, prefix};
    use crate::{Fault, Filter, decode};

    /// `data` decoded, by the first backend compiled in, which it doesn't
    /// read.
    fn hex(data: &[u8]) -> Result<Vec<u8>, crate::Error> {
        decode(data, &[Filter::AsciiHex], 1 << 20, backends()[0])
    }

    #[test]
    fn digits_of_either_case_decode_white_space_between_them_ignored() {
        assert_eq!(hex(b"48 65\r\n6c\t6C\x0c6F\0>").unwrap(), b"Hello");
        assert_eq!(hex(b">").unwrap(), b"");
    }

    #[test]
    fn an_odd_last_digit_is_as_if_a_0_followed_it() {
        assert_eq!(hex(b"4 8 7>").unwrap(), b"\x48\x70");
    }

    #[test]
    fn what_follows_the_end_is_ignored() {
        assert_eq!(hex(b"41>42 zz").unwrap(), b"A");
    }

    #[test]
    fn no_end_decodes_everything_and_says_so() {
        let error = hex(b"414").unwrap_err();
        assert_eq!(error.data, b"A\x40");
        assert_eq!(error.faults, [(0, Fault::NoEnd)]);
    }

    #[test]
    fn a_byte_it_doesnt_take_stops_it_without_the_digit_before() {
        // Vertical tab isn't PDF white-space, though it's C's.
        let error = hex(b"41 4\x0b2>").unwrap_err();
        assert_eq!(error.data, b"A");
        assert_eq!(error.faults, [(0, Fault::Byte { at: 4, byte: 0x0b })]);
    }

    #[test]
    fn the_limit_stops_it() {
        let error = decode(b"414243>", &[Filter::AsciiHex], 2, backends()[0]).unwrap_err();
        assert_eq!(error.data, b"AB");
        assert_eq!(error.faults, [(0, Fault::Limit)]);
    }

    /// `data` read as data that may run on: how far, whether it ended,
    /// what it gave and its faults.
    fn read(data: &[u8]) -> (usize, bool, Vec<u8>, Vec<Fault>) {
        let (prefix, out, faults) = prefix(data, Filter::AsciiHex, backends()[0]);
        (prefix.read, prefix.ended, out, faults)
    }

    /// Data that runs on is read to its `>`, and what follows isn't read;
    /// without one, to its end; at a byte it doesn't take, to that byte.
    #[test]
    fn what_it_read_is_to_its_end() {
        assert_eq!(read(b"48 69> EI Q"), (6, true, b"Hi".to_vec(), vec![]));
        assert_eq!(read(b"4 8 7>41"), (6, true, b"\x48\x70".to_vec(), vec![]));
        assert_eq!(read(b">"), (1, true, vec![], vec![]));
        let no_end = vec![Fault::NoEnd];
        assert_eq!(read(b"48 6"), (4, false, b"H\x60".to_vec(), no_end));
        let byte = vec![Fault::Byte { at: 4, byte: b'x' }];
        assert_eq!(read(b"41 4x2> EI"), (5, false, b"A".to_vec(), byte));
        assert_eq!(read(b""), (0, false, vec![], vec![]));
    }

    /// At the limit, it's read to the digit whose byte is over it; at the
    /// `>`, a last digit over it, ended.
    #[test]
    fn what_it_read_at_the_limit() {
        for backend in backends() {
            let at = |data: &[u8], limit| {
                let mut out = Vec::new();
                let (prefix, decoded) =
                    crate::decode_prefix_into(data, Filter::AsciiHex, limit, backend, &mut out);
                (prefix.read, prefix.ended, out, decoded.map_err(|f| f.0))
            };
            let limit = Err(vec![(0, Fault::Limit)]);
            assert_eq!(at(b"414243>", 2), (6, false, b"AB".to_vec(), limit.clone()));
            assert_eq!(at(b"41 4> EI", 1), (5, true, b"A".to_vec(), limit));
            assert_eq!(at(b"414243> EI", 3), (7, true, b"ABC".to_vec(), Ok(())));
        }
    }
}
