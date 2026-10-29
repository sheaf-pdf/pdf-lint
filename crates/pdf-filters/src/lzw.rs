//! `LZWDecode`'s codes (ISO 32000-2, 7.4.4.2), before its predictor.
//!
//! A table that fills, 4,096 entries with no clear code to empty it, is
//! read past: the codes after it are read against the table as it is,
//! 12 bits wide, until a clear code or the end, and [`Fault::TableFull`]
//! says where it filled without stopping the decode. The spec has an
//! encoder issue a clear code before then and says nothing of a decoder;
//! PDFium, pdf.js and poppler read on so, where qpdf stops. A code that
//! isn't in the table yet stops it ([`Fault::Code`], the byte it begins
//! in), and data that ends without a code of 257 is decoded to its end
//! ([`Fault::NoEnd`]).
//!
//! A decoder of its own, not [weezl](https://crates.io/crates/weezl)'s,
//! though weezl reads both `/EarlyChange` values (as TIFF's LZW and GIF's)
//! to the same bytes and reads on past a full table alike: weezl says
//! nothing of a full table, and a code that isn't in the table is an error
//! with no place in weezl. Both are what a checker reports. Codes are read
//! as weezl reads them, from a 64-bit buffer filled eight bytes at a time,
//! and an entry's bytes are copied at once.

use crate::{Fault, Out};

const CLEAR: u16 = 256;
const END: u16 = 257;
/// The entries a table holds, the 258 codes for bytes, a clear code and an
/// end among them.
const FULL: usize = 4096;

/// `data`'s codes, 9 to 12 bits wide, up to a code of 257; with
/// `early_change`, codes widen one entry early.
pub(crate) fn decode(data: &[u8], early_change: bool, out: &mut Out) {
    let mut bits = Bits::new(data);
    let mut width = 9;
    // Each entry from 258 on, as where its bytes are in what's decoded.
    let mut table: Vec<(usize, usize)> = Vec::with_capacity(FULL - 258);
    // Where the last code's bytes are, unless it was the first since a
    // clear code. The next code's follow them, so they and the next's
    // first byte are the next entry.
    let mut last: Option<(usize, usize)> = None;
    loop {
        let at = bits.at();
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
            if index < FULL {
                table.push((start, len + 1));
                if matches!(index + usize::from(early_change), 511 | 1023 | 2047) {
                    width += 1;
                }
            } else {
                // Full, and 12 bits wide: read on against it as it is.
                // Where it filled is said once, the first fault of a kind
                // being the one kept.
                out.fault(Fault::TableFull { at });
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

/// The `len` bytes decoded from `start` added again, as many as the limit
/// allows: an entry that ends with its own first byte, not yet added (the
/// one the last code is about to make), takes it from its start.
fn copy(out: &mut Out, (start, len): (usize, usize)) -> Result<(), ()> {
    let end = out.data.len();
    let take = out.take(len);
    let there = take.min(end - start);
    out.data.extend_from_within(start..start + there);
    if there < take {
        let first = out.data[start];
        out.data.push(first);
    }
    crate::whole(take, len)
}

/// A reader of codes, most significant bit first, from a 64-bit buffer
/// filled from the data eight bytes at a time, or a byte at a time at its
/// end.
struct Bits<'a> {
    data: &'a [u8],
    /// The next byte to take into the buffer.
    next: usize,
    /// The bits taken and not yet read, from the buffer's top. Those below
    /// them are zeros, or the data's next bits, where eight bytes were
    /// loaded and only whole bytes counted.
    buffer: u64,
    /// How many bits at the buffer's top are taken and not yet read.
    held: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Bits<'a> {
        Bits {
            data,
            next: 0,
            buffer: 0,
            held: 0,
        }
    }

    /// The byte the next code begins in.
    fn at(&self) -> usize {
        (self.next * 8 - self.held as usize) / 8
    }

    /// The next `width` bits, at most 16, unless the data ends first.
    fn read(&mut self, width: u32) -> Option<u16> {
        if self.held < width {
            self.fill();
            if self.held < width {
                return None;
            }
        }
        let code = (self.buffer >> (64 - width)) as u16;
        self.buffer <<= width;
        self.held -= width;
        Some(code)
    }

    /// Whole bytes taken into the buffer, below the bits it holds, as many
    /// as fit: eight loaded at once where the data has them, the bits past
    /// the whole bytes that fit being the data's next, which a later fill
    /// sets again alike.
    fn fill(&mut self) {
        let room = (64 - self.held) / 8;
        match self.data[self.next..].first_chunk::<8>() {
            Some(eight) => {
                self.buffer |= u64::from_be_bytes(*eight) >> self.held;
                self.next += room as usize;
                self.held += room * 8;
            }
            None => {
                while self.held <= 56 && self.next < self.data.len() {
                    self.buffer |= u64::from(self.data[self.next]) << (56 - self.held);
                    self.next += 1;
                    self.held += 8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CLEAR, END};
    use crate::fuzz::backends;
    use crate::{Fault, Filter, Predictor, decode};
    use std::collections::HashMap;

    /// `data` decoded, by the first backend compiled in, which it doesn't
    /// read.
    fn lzw(data: &[u8], early_change: bool) -> Result<Vec<u8>, crate::Error> {
        let predictor = Predictor::None;
        decode(
            data,
            &[Filter::Lzw {
                early_change,
                predictor,
            }],
            1 << 20,
            backends()[0],
        )
    }

    /// Codes written as the decoder reads them, most significant bit first,
    /// each as wide as the decoder's table makes it: one entry early with
    /// `early_change`, and no entry past 4,095.
    struct Codes {
        bits: Vec<bool>,
        width: u32,
        // The decoder's next entry, and whether it's had a code since a
        // clear code.
        next: usize,
        first: bool,
        early: usize,
    }

    impl Codes {
        fn new(early_change: bool) -> Codes {
            Codes {
                bits: Vec::new(),
                width: 9,
                next: 258,
                first: true,
                early: usize::from(early_change),
            }
        }

        fn emit(&mut self, code: u16) {
            assert!(
                u32::from(code) < 1 << self.width,
                "{code} in {} bits",
                self.width
            );
            self.bits
                .extend((0..self.width).rev().map(|i| code >> i & 1 == 1));
            match code {
                CLEAR => (self.width, self.next, self.first) = (9, 258, true),
                END => {}
                _ if self.first => self.first = false,
                _ if self.next < 4096 => {
                    if matches!(self.next + self.early, 511 | 1023 | 2047) {
                        self.width += 1;
                    }
                    self.next += 1;
                }
                _ => {}
            }
        }

        fn bytes(&self) -> Vec<u8> {
            (self.bits.chunks(8))
                .map(|byte| (0..8).fold(0, |b, i| b << 1 | u8::from(byte.get(i) == Some(&true))))
                .collect()
        }

        /// `codes`, written.
        fn all(early_change: bool, codes: impl IntoIterator<Item = u16>) -> Vec<u8> {
            let mut written = Codes::new(early_change);
            codes.into_iter().for_each(|code| written.emit(code));
            written.bytes()
        }
    }

    /// `data` encoded, a clear code when the table nears full, as the
    /// decoder reads codes: an encoder of its own, for the tests.
    fn encode(data: &[u8], early_change: bool) -> Vec<u8> {
        let mut codes = Codes::new(early_change);
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
        codes.bytes()
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

    /// 3,839 codes of bytes, which fill the table to entry 4,095 with no
    /// clear code: each byte `1 + k * 37 % 255`, so no short read hides as
    /// padding.
    fn filling() -> Vec<u16> {
        (0..3839u32).map(|k| 1 + (k * 37 % 255) as u16).collect()
    }

    /// A full table is read past, as PDFium, pdf.js and poppler read it:
    /// the codes after it against the table as it is, 12 bits wide, to the
    /// end; and said where it filled, the byte the first code past it
    /// begins in, without stopping. Codes of 0 past it, and codes of bytes,
    /// of entries and of the last entry, 4,095.
    #[test]
    fn a_full_table_reads_on_and_says_where_it_filled() {
        let fill = filling();
        // Entry 258 + k is code k's byte and code k + 1's.
        let entry = |e: u16| {
            let k = usize::from(e - 258);
            [fill[k] as u8, fill[k + 1] as u8]
        };
        let past: Vec<u16> = (0..301u16)
            .map(|j| match j % 3 {
                0 => 1 + (j * 11 % 255),
                1 => 258 + (j * 13 % 3838),
                _ => 4095,
            })
            .collect();
        let mut want: Vec<u8> = fill.iter().map(|&b| b as u8).collect();
        for &code in &past {
            match code {
                ..=255 => want.push(code as u8),
                _ => want.extend(entry(code)),
            }
        }
        assert_eq!(want.len(), 4340);
        for (early_change, at) in [(true, 5407), (false, 5406)] {
            let codes = fill.iter().chain(&past).copied().chain([END]);
            let error = lzw(&Codes::all(early_change, codes), early_change).unwrap_err();
            assert_eq!(error.data, want, "{early_change}");
            assert_eq!(
                error.faults,
                [(0, Fault::TableFull { at })],
                "{early_change}"
            );
            // 4,140 codes of 0: the first 3,839 fill it.
            let zeros = std::iter::repeat_n(0, 4140).chain([END]);
            let error = lzw(&Codes::all(early_change, zeros), early_change).unwrap_err();
            assert_eq!(error.data, [0; 4140], "{early_change}");
            assert_eq!(
                error.faults,
                [(0, Fault::TableFull { at })],
                "{early_change}"
            );
            // Without an end, it's read to the data's: both said.
            let codes = fill.iter().chain(&past).copied();
            let error = lzw(&Codes::all(early_change, codes), early_change).unwrap_err();
            assert_eq!(error.data, want, "{early_change}");
            let faults = [(0, Fault::TableFull { at }), (0, Fault::NoEnd)];
            assert_eq!(error.faults, faults, "{early_change}");
        }
    }

    /// A table that fills is no fault where a clear code or the end comes
    /// before a code would add to it.
    #[test]
    fn a_table_filled_then_cleared_or_ended_is_clean() {
        let fill = filling();
        let bytes: Vec<u8> = fill.iter().map(|&b| b as u8).collect();
        for early_change in [true, false] {
            let ended = fill.iter().copied().chain([END]);
            let data = lzw(&Codes::all(early_change, ended), early_change);
            assert_eq!(data.unwrap(), bytes, "{early_change}");
            // A clear code, 12 bits wide, then 65 66 258 259: A B AB BA.
            let cleared = fill.iter().copied().chain([CLEAR, 65, 66, 258, 259, END]);
            let data = lzw(&Codes::all(early_change, cleared), early_change);
            let want = [&bytes[..], b"ABABBA"].concat();
            assert_eq!(data.unwrap(), want, "{early_change}");
        }
    }
}
