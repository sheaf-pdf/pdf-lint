//! `RunLengthDecode` (ISO 32000-2, 7.4.5).

use crate::{Fault, Out, Prefix};

/// `data`'s runs, up to a length of 128: a length of 0 to 127 copies that
/// many and one more bytes, and of 129 to 255 repeats the next byte 257
/// less that many times. What of `data` it read.
pub(crate) fn decode(data: &[u8], out: &mut Out) -> Prefix {
    let mut at = 0;
    while let Some(&length) = data.get(at) {
        at += 1;
        let done = match length {
            128 => return Prefix::ended(at),
            0..128 => {
                let n = usize::from(length) + 1;
                let run = &data[at..data.len().min(at + n)];
                at += run.len();
                if run.len() < n {
                    let _ = out.extend(run);
                    out.fault(Fault::Truncated);
                    return Prefix::unended(at);
                }
                out.extend(run)
            }
            _ => {
                let Some(&byte) = data.get(at) else {
                    out.fault(Fault::Truncated);
                    return Prefix::unended(at);
                };
                at += 1;
                out.repeat(byte, 257 - usize::from(length))
            }
        };
        if done.is_err() {
            return Prefix::unended(at);
        }
    }
    out.fault(Fault::NoEnd);
    Prefix::unended(at)
}

#[cfg(test)]
mod tests {
    use crate::fuzz::{backends, prefix};
    use crate::{Fault, Filter, decode};

    /// `data` decoded, by the first backend compiled in, which it doesn't
    /// read.
    fn rl(data: &[u8]) -> Result<Vec<u8>, crate::Error> {
        decode(data, &[Filter::RunLength], 1 << 20, backends()[0])
    }

    #[test]
    fn runs_copy_and_repeat_up_to_128() {
        assert_eq!(
            rl(b"\x02abc\xfex\x00y\x81z\x80").unwrap(),
            b"abcxxxy"
                .iter()
                .chain(&[b'z'; 128])
                .copied()
                .collect::<Vec<_>>()
        );
        assert_eq!(rl(b"\x80").unwrap(), b"");
    }

    #[test]
    fn what_follows_128_is_ignored() {
        assert_eq!(rl(b"\x00a\x80\x00b").unwrap(), b"a");
    }

    #[test]
    fn data_ending_inside_a_run_keeps_what_there_is() {
        let error = rl(b"\x00a\x03bc").unwrap_err();
        assert_eq!(error.data, b"abc");
        assert_eq!(error.faults, [(0, Fault::Truncated)]);
        let error = rl(b"\x00a\xfe").unwrap_err();
        assert_eq!(error.data, b"a");
        assert_eq!(error.faults, [(0, Fault::Truncated)]);
    }

    #[test]
    fn no_end_decodes_everything_and_says_so() {
        let error = rl(b"\x01ab").unwrap_err();
        assert_eq!(error.data, b"ab");
        assert_eq!(error.faults, [(0, Fault::NoEnd)]);
    }

    #[test]
    fn the_limit_stops_a_repeat() {
        let error = decode(b"\x81z\x80", &[Filter::RunLength], 100, backends()[0]).unwrap_err();
        assert_eq!(error.data, [b'z'; 100]);
        assert_eq!(error.faults, [(0, Fault::Limit)]);
    }

    /// `data` read as data that may run on: how far, whether it ended,
    /// what it gave and its faults.
    fn read(data: &[u8]) -> (usize, bool, Vec<u8>, Vec<Fault>) {
        let (prefix, out, faults) = prefix(data, Filter::RunLength, backends()[0]);
        (prefix.read, prefix.ended, out, faults)
    }

    /// Data that runs on is read to its run-length of 128; without one,
    /// to its end, inside a run or not.
    #[test]
    fn what_it_read_is_to_its_end() {
        assert_eq!(read(b"\x02abc\x80 EI"), (5, true, b"abc".to_vec(), vec![]));
        assert_eq!(read(b"\x80\x00a"), (1, true, vec![], vec![]));
        let no_end = vec![Fault::NoEnd];
        assert_eq!(read(b"\x01ab\xfdx"), (5, false, b"abxxxx".to_vec(), no_end));
        let truncated = vec![Fault::Truncated];
        assert_eq!(
            read(b"\x00a\x03bc"),
            (5, false, b"abc".to_vec(), truncated.clone())
        );
        assert_eq!(read(b"\x00a\xfe"), (3, false, b"a".to_vec(), truncated));
    }

    /// At the limit, it's read to the end of the run over it.
    #[test]
    fn what_it_read_at_the_limit() {
        for backend in backends() {
            let at = |data: &[u8], limit| {
                let mut out = Vec::new();
                let (prefix, decoded) =
                    crate::decode_prefix_into(data, Filter::RunLength, limit, backend, &mut out);
                (prefix.read, prefix.ended, out, decoded.map_err(|f| f.0))
            };
            let limit = Err(vec![(0, Fault::Limit)]);
            assert_eq!(
                at(b"\x81z\x80", 100),
                (2, false, vec![b'z'; 100], limit.clone())
            );
            assert_eq!(
                at(b"\x00a\x02bcd\x80", 2),
                (6, false, b"ab".to_vec(), limit)
            );
            assert_eq!(at(b"\x00a\x80", 1), (3, true, b"a".to_vec(), Ok(())));
        }
    }
}
