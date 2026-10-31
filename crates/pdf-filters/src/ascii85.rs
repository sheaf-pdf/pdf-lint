//! `ASCII85Decode` (ISO 32000-2, 7.4.3).

use crate::{Fault, Out, Prefix};
use pdf_tokenizer::is_white_space;

/// `data`'s base-85 groups as bytes, up to `~>`, white-space ignored: what
/// of `data` it read.
pub(crate) fn decode(data: &[u8], out: &mut Out) -> Prefix {
    // The group read so far, and where it began.
    let mut group = [0u8; 5];
    let mut n = 0;
    let mut start = 0;
    let mut bytes = data.iter().enumerate();
    while let Some((at, &byte)) = bytes.next() {
        match byte {
            _ if is_white_space(byte) => {}
            b'~' => {
                let next = bytes.by_ref().find(|(_, b)| !is_white_space(**b));
                return match next {
                    Some((end, b'>')) => {
                        let _ = finish(&group, n, start, out);
                        Prefix::ended(end + 1)
                    }
                    // What decoded is kept, but not the group it's in.
                    Some((after, _)) => {
                        out.fault(Fault::BrokenEnd { at });
                        Prefix::unended(after + 1)
                    }
                    None => {
                        if finish(&group, n, start, out).is_ok() {
                            out.fault(Fault::BrokenEnd { at });
                        }
                        Prefix::unended(data.len())
                    }
                };
            }
            b'z' if n == 0 => {
                if out.extend(&[0; 4]).is_err() {
                    return Prefix::unended(at + 1);
                }
            }
            b'z' => {
                out.fault(Fault::InGroup { at });
                return Prefix::unended(at + 1);
            }
            b'!'..=b'u' => {
                if n == 0 {
                    start = at;
                }
                group[n] = byte;
                n += 1;
                if n == 5 {
                    n = 0;
                    let bytes = value(&group, start, out);
                    if out.extend(&bytes).is_err() {
                        return Prefix::unended(at + 1);
                    }
                }
            }
            _ => {
                out.fault(Fault::Byte { at, byte });
                return Prefix::unended(at + 1);
            }
        }
    }
    if finish(&group, n, start, out).is_ok() {
        out.fault(Fault::NoEnd);
    }
    Prefix::unended(data.len())
}

/// A last group of `n` characters, which stands for `n` - 1 bytes: the
/// group filled out with `u`, the highest digit, and read.
fn finish(group: &[u8; 5], n: usize, start: usize, out: &mut Out) -> Result<(), ()> {
    match n {
        0 => Ok(()),
        1 => {
            out.fault(Fault::LoneByte { at: start });
            Ok(())
        }
        _ => {
            let mut filled = [b'u'; 5];
            filled[..n].copy_from_slice(&group[..n]);
            let bytes = value(&filled, start, out);
            out.extend(&bytes[..n - 1])
        }
    }
}

/// A group's four bytes: its value's low 32 bits, as qpdf reads it, with
/// a value over 2³² − 1 noted as a fault.
fn value(group: &[u8; 5], start: usize, out: &mut Out) -> [u8; 4] {
    let value = (group.iter()).fold(0u64, |value, &digit| value * 85 + u64::from(digit - b'!'));
    if value > u64::from(u32::MAX) {
        out.fault(Fault::Overflow { at: start });
    }
    (value as u32).to_be_bytes()
}

#[cfg(test)]
mod tests {
    use crate::fuzz::{backends, prefix};
    use crate::{Fault, Filter, decode};

    /// `data` decoded, by the first backend compiled in, which it doesn't
    /// read.
    fn a85(data: &[u8]) -> Result<Vec<u8>, crate::Error> {
        decode(data, &[Filter::Ascii85], 1 << 20, backends()[0])
    }

    #[test]
    fn groups_decode_with_a_short_last_one() {
        // Python's base64.a85encode(b"Hello world").
        assert_eq!(a85(b"87cURD]j7BEbo7~>").unwrap(), b"Hello world");
        assert_eq!(a85(b"87cUR\nD]j7B Ebo7\0~ >").unwrap(), b"Hello world");
        assert_eq!(a85(b"~>").unwrap(), b"");
    }

    #[test]
    fn z_is_four_zeros_between_groups() {
        assert_eq!(a85(b"z87cURz~>").unwrap(), b"\0\0\0\0Hell\0\0\0\0");
    }

    #[test]
    fn z_inside_a_group_stops_it() {
        let error = a85(b"87cURD]z~>").unwrap_err();
        assert_eq!(error.data, b"Hell");
        assert_eq!(error.faults, [(0, Fault::InGroup { at: 7 })]);
    }

    #[test]
    fn a_value_over_32_bits_is_its_low_32_bits() {
        assert_eq!(a85(b"s8W-!~>").unwrap(), [0xff; 4]);
        let error = a85(b"s8W-\"~>").unwrap_err();
        assert_eq!(error.data, [0; 4]);
        assert_eq!(error.faults, [(0, Fault::Overflow { at: 0 })]);
    }

    #[test]
    fn a_last_group_of_one_character_is_no_bytes() {
        let error = a85(b"87cURD~>").unwrap_err();
        assert_eq!(error.data, b"Hell");
        assert_eq!(error.faults, [(0, Fault::LoneByte { at: 5 })]);
    }

    #[test]
    fn a_tilde_without_a_closing_angle_bracket() {
        // Followed by something else, it stops without its group.
        let error = a85(b"87cURD]~x").unwrap_err();
        assert_eq!(error.data, b"Hell");
        assert_eq!(error.faults, [(0, Fault::BrokenEnd { at: 7 })]);
        // At the end, its group is read.
        let error = a85(b"87cURD]j7BEbo7~").unwrap_err();
        assert_eq!(error.data, b"Hello world");
        assert_eq!(error.faults, [(0, Fault::BrokenEnd { at: 14 })]);
    }

    #[test]
    fn no_end_decodes_everything_and_says_so() {
        let error = a85(b"87cURD]j7BEbo7").unwrap_err();
        assert_eq!(error.data, b"Hello world");
        assert_eq!(error.faults, [(0, Fault::NoEnd)]);
    }

    #[test]
    fn a_byte_it_doesnt_take_stops_it() {
        let error = a85(b"87cURD]j7B{").unwrap_err();
        assert_eq!(error.data, b"Hello wo");
        assert_eq!(error.faults, [(0, Fault::Byte { at: 10, byte: b'{' })]);
    }

    /// `data` read as data that may run on: how far, whether it ended,
    /// what it gave and its faults.
    fn read(data: &[u8]) -> (usize, bool, Vec<u8>, Vec<Fault>) {
        let (prefix, out, faults) = prefix(data, Filter::Ascii85, backends()[0]);
        (prefix.read, prefix.ended, out, faults)
    }

    /// Data that runs on is read to the `>` of its `~>`, white-space
    /// between them or not; without one, to its end; at a fault that stops
    /// it, to the byte it stopped at, the one after a `~` that isn't `>`.
    #[test]
    fn what_it_read_is_to_its_end() {
        let hello = b"Hello world".to_vec();
        assert_eq!(
            read(b"87cURD]j7BEbo7~> EI"),
            (16, true, hello.clone(), vec![])
        );
        assert_eq!(
            read(b"87cURD]j7BEbo7~\n> EI"),
            (17, true, hello.clone(), vec![])
        );
        assert_eq!(read(b"~>~>"), (2, true, vec![], vec![]));
        let no_end = vec![Fault::NoEnd];
        assert_eq!(read(b"87cURD]j7BEbo7"), (14, false, hello.clone(), no_end));
        let broken = vec![Fault::BrokenEnd { at: 7 }];
        assert_eq!(read(b"87cURD]~ x~>"), (10, false, b"Hell".to_vec(), broken));
        let broken = vec![Fault::BrokenEnd { at: 14 }];
        assert_eq!(read(b"87cURD]j7BEbo7~ "), (16, false, hello, broken));
        let in_group = vec![Fault::InGroup { at: 7 }];
        assert_eq!(read(b"87cURD]z~>"), (8, false, b"Hell".to_vec(), in_group));
        let byte = vec![Fault::Byte { at: 10, byte: b'{' }];
        assert_eq!(
            read(b"87cURD]j7B{~>"),
            (11, false, b"Hello wo".to_vec(), byte)
        );
    }

    /// At the limit, it's read to the byte that ends the group, or the
    /// `z`, over it; at the `~>`, a last group over it, ended.
    #[test]
    fn what_it_read_at_the_limit() {
        for backend in backends() {
            let at = |data: &[u8], limit| {
                let mut out = Vec::new();
                let (prefix, decoded) =
                    crate::decode_prefix_into(data, Filter::Ascii85, limit, backend, &mut out);
                (
                    prefix.read,
                    prefix.ended,
                    out.len(),
                    decoded.map_err(|f| f.0),
                )
            };
            let limit = Err(vec![(0, Fault::Limit)]);
            assert_eq!(at(b"87cURD]j7BEbo7~>", 6), (10, false, 6, limit.clone()));
            assert_eq!(at(b"zz~>", 5), (2, false, 5, limit.clone()));
            assert_eq!(at(b"87cURD]j7BEbo7~>", 10), (16, true, 10, limit));
        }
    }
}
