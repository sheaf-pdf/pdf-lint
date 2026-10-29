//! `RunLengthDecode` (ISO 32000-2, 7.4.5).

use crate::{Fault, Out};

/// `data`'s runs, up to a length of 128: a length of 0 to 127 copies that
/// many and one more bytes, and of 129 to 255 repeats the next byte 257
/// less that many times.
pub(crate) fn decode(data: &[u8], out: &mut Out) {
    let mut at = 0;
    while let Some(&length) = data.get(at) {
        at += 1;
        let done = match length {
            128 => return,
            0..128 => {
                let n = usize::from(length) + 1;
                let run = &data[at..data.len().min(at + n)];
                at += run.len();
                if run.len() < n {
                    let _ = out.extend(run);
                    out.fault(Fault::Truncated);
                    return;
                }
                out.extend(run)
            }
            _ => {
                let Some(&byte) = data.get(at) else {
                    out.fault(Fault::Truncated);
                    return;
                };
                at += 1;
                out.repeat(byte, 257 - usize::from(length))
            }
        };
        if done.is_err() {
            return;
        }
    }
    out.fault(Fault::NoEnd);
}

#[cfg(test)]
mod tests {
    use crate::fuzz::backends;
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
}
