//! `ASCIIHexDecode` (ISO 32000-2, 7.4.2).

use crate::{Fault, Out};
use pdf_tokenizer::is_white_space;

/// `data`'s hex digits as bytes, up to `>`, white-space ignored; an odd
/// last digit is as if a 0 followed it.
pub(crate) fn decode(data: &[u8], out: &mut Out) {
    let mut high = None;
    for (at, &byte) in data.iter().enumerate() {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            b'>' => {
                let _ = finish(high, out);
                return;
            }
            _ if is_white_space(byte) => continue,
            _ => {
                // What decoded is kept, but not a digit without its pair.
                out.fault(Fault::Byte { at, byte });
                return;
            }
        };
        high = match high {
            None => Some(digit),
            Some(high) => {
                if out.extend(&[high << 4 | digit]).is_err() {
                    return;
                }
                None
            }
        };
    }
    if finish(high, out).is_ok() {
        out.fault(Fault::NoEnd);
    }
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
    use crate::fuzz::backends;
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
}
