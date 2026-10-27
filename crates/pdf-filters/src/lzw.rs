//! `LZWDecode`'s codes (ISO 32000-2, 7.4.4.2), before its predictor.

use crate::{Fault, Out};

const CLEAR: u16 = 256;
const END: u16 = 257;

/// `data`'s codes, 9 to 12 bits wide, up to a code of 257; with
/// `early_change`, codes widen one entry early.
pub(crate) fn decode(data: &[u8], early_change: bool, out: &mut Out) {
    let mut bits = Bits { data, at: 0 };
    let mut width = 9;
    // Each entry from 258 on, as where its bytes are in what's decoded.
    let mut table: Vec<(usize, usize)> = Vec::new();
    // Where the last code's bytes are, unless it was the first since a
    // clear code. The next code's follow them, so they and the next's
    // first byte are the next entry.
    let mut last: Option<(usize, usize)> = None;
    loop {
        let at = bits.at / 8;
        let Some(code) = bits.read(width) else {
            out.fault(Fault::NoEnd);
            return;
        };
        match code {
            CLEAR => {
                table.clear();
                width = 9;
                last = None;
                continue;
            }
            END => return,
            _ => {}
        }
        // An entry, or the one the last code is about to make: its bytes
        // and their first byte again.
        let entry = (code > END).then(|| usize::from(code - END - 1));
        if entry.is_some_and(|i| i > table.len() || (i == table.len() && last.is_none())) {
            out.fault(Fault::Code { at });
            return;
        }
        if let Some((start, len)) = last {
            let index = 258 + table.len();
            if index == 4096 {
                out.fault(Fault::TableFull { at });
                return;
            }
            table.push((start, len + 1));
            if matches!(index + usize::from(early_change), 511 | 1023 | 2047) {
                width += 1;
            }
        }
        let start = out.data.len();
        let written = match entry {
            None => out.extend(&[code as u8]),
            Some(i) => copy(out, table[i]),
        };
        if written.is_err() {
            return;
        }
        last = Some((start, out.data.len() - start));
    }
}

/// The `len` bytes decoded from `start` added again, byte by byte, so an
/// entry that ends with its own first byte, not yet added, is copied too.
fn copy(out: &mut Out, (start, len): (usize, usize)) -> Result<(), ()> {
    for i in start..start + len {
        let byte = out.data[i];
        out.extend(&[byte])?;
    }
    Ok(())
}

/// A reader of codes, most significant bit first.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    /// The next `width` bits, unless the data ends first.
    fn read(&mut self, width: u32) -> Option<u16> {
        let end = self.at + width as usize;
        if end.div_ceil(8) > self.data.len() {
            return None;
        }
        let code = (self.at..end).fold(0, |code, i| {
            code << 1 | u16::from(self.data[i / 8] >> (7 - i % 8) & 1)
        });
        self.at = end;
        Some(code)
    }
}

#[cfg(test)]
mod tests {
    use super::{CLEAR, END};
    use crate::{Fault, Filter, Predictor, decode};
    use std::collections::HashMap;

    fn lzw(data: &[u8], early_change: bool) -> Result<Vec<u8>, crate::Error> {
        let predictor = Predictor::None;
        decode(
            data,
            &[Filter::Lzw {
                early_change,
                predictor,
            }],
            1 << 20,
        )
    }

    /// `data` encoded, a clear code when the table nears full, as the
    /// decoder reads codes: an encoder of its own, for the tests.
    fn encode(data: &[u8], early_change: bool) -> Vec<u8> {
        struct Codes {
            bits: Vec<bool>,
            width: u32,
            // The decoder's next entry, and whether it's had a code since
            // a clear code.
            next: usize,
            first: bool,
            early: usize,
        }
        impl Codes {
            fn emit(&mut self, code: u16) {
                self.bits
                    .extend((0..self.width).rev().map(|i| code >> i & 1 == 1));
                match code {
                    CLEAR => (self.width, self.next, self.first) = (9, 258, true),
                    END => {}
                    _ if self.first => self.first = false,
                    _ => {
                        if matches!(self.next + self.early, 511 | 1023 | 2047) {
                            self.width += 1;
                        }
                        self.next += 1;
                    }
                }
            }
        }
        let mut codes = Codes {
            bits: Vec::new(),
            width: 9,
            next: 258,
            first: true,
            early: usize::from(early_change),
        };
        let mut dictionary: HashMap<Vec<u8>, u16> = HashMap::new();
        let code = |dictionary: &HashMap<Vec<u8>, u16>, w: &[u8]| match w {
            [byte] => u16::from(*byte),
            _ => dictionary[w],
        };
        codes.emit(CLEAR);
        let mut w: Vec<u8> = Vec::new();
        for &byte in data {
            let mut wc = w.clone();
            wc.push(byte);
            if wc.len() == 1 || dictionary.contains_key(&wc) {
                w = wc;
                continue;
            }
            codes.emit(code(&dictionary, &w));
            match dictionary.len() + 258 {
                4000 => {
                    codes.emit(CLEAR);
                    dictionary.clear();
                }
                next => {
                    dictionary.insert(wc, next as u16);
                }
            }
            w = vec![byte];
        }
        if !w.is_empty() {
            codes.emit(code(&dictionary, &w));
        }
        codes.emit(END);
        (codes.bits.chunks(8))
            .map(|byte| (0..8).fold(0, |b, i| b << 1 | u8::from(byte.get(i) == Some(&true))))
            .collect()
    }

    #[test]
    fn the_specs_example_decodes() {
        // ISO 32000-2, 7.4.4.2: 256 45 258 258 65 259 66 257.
        let data = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        assert_eq!(lzw(&data, true).unwrap(), b"-----A---B");
        assert_eq!(encode(b"-----A---B", true), data);
    }

    #[test]
    fn codes_widen_to_12_bits_and_a_clear_code_empties_the_table() {
        // Bytes of no pattern, enough to fill the table many times over.
        let mut x = 1u32;
        let data: Vec<u8> = (0..60_000)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (x >> 16) as u8
            })
            .collect();
        for early_change in [true, false] {
            let encoded = encode(&data, early_change);
            assert_eq!(lzw(&encoded, early_change).unwrap(), data, "{early_change}");
            assert_ne!(lzw(&encoded, !early_change).ok(), Some(data.clone()));
        }
    }

    #[test]
    fn a_code_not_in_the_table_stops_it() {
        // 256, then 300 with nothing in the table, in 9 bits each.
        let error = lzw(&[0x80, 0x4B, 0x00], true).unwrap_err();
        assert_eq!(error.data, b"");
        assert_eq!(error.faults, [(0, Fault::Code { at: 1 })]);
    }

    #[test]
    fn no_end_decodes_everything_and_says_so() {
        let mut data = encode(b"-----A---B", true);
        data.truncate(7);
        let error = lzw(&data, true).unwrap_err();
        assert_eq!(error.data, b"-----A---");
        assert_eq!(error.faults, [(0, Fault::NoEnd)]);
    }

    #[test]
    fn a_full_table_stops_it() {
        // Codes of 0, 12 bits wide once wide enough, never cleared.
        let mut bits: Vec<bool> = Vec::new();
        let mut width = 9;
        for index in 257..4097 {
            bits.extend((0..width).map(|_| false));
            if matches!(index + 1, 511 | 1023 | 2047) {
                width += 1;
            }
        }
        let data: Vec<u8> = (bits.chunks(8))
            .map(|byte| (0..8).fold(0, |b, i| b << 1 | u8::from(byte.get(i) == Some(&true))))
            .collect();
        let error = lzw(&data, true).unwrap_err();
        assert_eq!(error.data, [0; 3839]);
        assert!(
            matches!(error.faults[..], [(0, Fault::TableFull { .. })]),
            "{error:?}"
        );
    }
}
