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
//! Under [`Reading::Pdfium`], a code past the table's next entry that
//! isn't the first since a clear code is read as PDFium's decoder reads
//! it, as of chromium/8125: as that entry, the last code's bytes and
//! their first byte again ([`Fault::AsNext`]); the entry the code after
//! it makes then holds that code, not its bytes, read through the table
//! each time the entry is read, as the table has it then; and a code
//! from 258 on right after it is the last read. PDFium's table holds each
//! entry as a code and a byte, read through each time; here an entry's
//! bytes are where they were decoded, as in the default reading, but for
//! such an entry, whose bytes can change as the table grows.
//!
//! A decoder of its own, not [weezl](https://crates.io/crates/weezl)'s,
//! though weezl reads both `/EarlyChange` values (as TIFF's LZW and GIF's)
//! to the same bytes and reads on past a full table alike: weezl says
//! nothing of a full table, and a code that isn't in the table is an error
//! with no place in weezl. Both are what a checker reports. Codes are read
//! as weezl reads them, from a 64-bit buffer filled eight bytes at a time,
//! and an entry's bytes are copied at once.
//!
//! What it read is counted by the bits of the codes it read, not by the
//! bytes the buffer took ahead of them: to the last bit of the last code,
//! rounded up to a byte, as PDFium's LZW decoder counts what it consumed.

use crate::{Fault, Out, Prefix, Reading};

const CLEAR: u16 = 256;
const END: u16 = 257;
/// The entries a table holds, the 258 codes for bytes, a clear code and an
/// end among them.
const FULL: usize = 4096;
/// The most bytes PDFium's decoder gives for one code read through its
/// table: of a longer run, through entries that hold codes past the table
/// as it then was, the last this many.
const RUN: usize = 4000;

/// `data`'s codes, 9 to 12 bits wide, up to a code of 257; with
/// `early_change`, codes widen one entry early; read as `reading` reads
/// them. What of `data` it read.
pub(crate) fn decode(data: &[u8], early_change: bool, reading: Reading, out: &mut Out) -> Prefix {
    match reading {
        Reading::Qpdf => codes(data, early_change, out),
        Reading::Pdfium => pdfium(data, early_change, out),
    }
}

/// `data`'s codes, as [`decode`] reads them under [`Reading::Qpdf`].
fn codes(data: &[u8], early_change: bool, out: &mut Out) -> Prefix {
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
            return Prefix::unended(bits.read_to());
        };
        match code {
            CLEAR => {
                table.clear();
                width = 9;
                last = None;
                continue;
            }
            END => return Prefix::ended(bits.read_to()),
            _ => {}
        }
        // An entry, or the one the last code is about to make: its bytes
        // and their first byte again.
        let entry = (code > END).then(|| usize::from(code - END - 1));
        if entry.is_some_and(|i| i > table.len() || (i == table.len() && last.is_none())) {
            out.fault(Fault::Code { at });
            return Prefix::unended(bits.read_to());
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
            return Prefix::unended(bits.read_to());
        }
        last = Some((start, out.data.len() - start));
    }
}

/// An entry of the table [`pdfium`] reads by.
#[derive(Clone, Copy)]
enum Entry {
    /// Its bytes, where they are in what's decoded: the default reading's
    /// every entry, which is the same each time it's read.
    At(usize, usize),
    /// A code and a byte after it: made after a code past the table, or
    /// one whose entry is such, its bytes read through the table each time
    /// it's read, as the code's entry may yet come or change.
    After(u16, u8),
}

/// The last code [`pdfium`] read, for the entry the next makes.
#[derive(Clone, Copy)]
struct Last {
    code: u16,
    /// Where its bytes are, when the table gives them the same each time:
    /// a byte's, or an entry's held at a place.
    at: Option<(usize, usize)>,
    /// The first of the bytes it gave.
    first: u8,
}

/// `data`'s codes, as [`decode`] reads them under [`Reading::Pdfium`]: as
/// [`codes`] reads them but for a code from 258 on that isn't in the table
/// (module docs). One that's the first since a clear code or the start
/// stops it ([`Fault::Code`]), as it does [`codes`], for the caller to
/// refuse the data.
fn pdfium(data: &[u8], early_change: bool, out: &mut Out) -> Prefix {
    let mut bits = Bits::new(data);
    let mut width = 9;
    let mut table: Vec<Entry> = Vec::with_capacity(FULL - 258);
    let mut last: Option<Last> = None;
    // An entry's bytes, read through the table from the last back.
    let mut walked: Vec<u8> = Vec::new();
    loop {
        let at = bits.at();
        let Some(code) = bits.read(width) else {
            out.fault(Fault::NoEnd);
            return Prefix::unended(bits.read_to());
        };
        match code {
            CLEAR => {
                table.clear();
                width = 9;
                last = None;
                continue;
            }
            END => return Prefix::ended(bits.read_to()),
            _ => {}
        }
        let entry = (code > END).then(|| usize::from(code - END - 1));
        let Some(before) = last else {
            if entry.is_some() {
                out.fault(Fault::Code { at });
                return Prefix::unended(bits.read_to());
            }
            let start = out.data.len();
            if out.extend(&[code as u8]).is_err() {
                return Prefix::unended(bits.read_to());
            }
            last = Some(Last {
                code,
                at: Some((start, 1)),
                first: code as u8,
            });
            continue;
        };
        let full = 258 + table.len() >= FULL;
        if full {
            out.fault(Fault::TableFull { at });
        }
        let start = out.data.len();
        let written = match entry {
            None => out.extend(&[code as u8]),
            Some(i) => match table.get(i) {
                Some(&Entry::At(from, len)) => copy(out, (from, len)),
                Some(&Entry::After(..)) => {
                    walked.clear();
                    walk(&table, code, &out.data, &mut walked);
                    give(out, &mut walked)
                }
                // The next entry, or past it: the last code's bytes and
                // their first byte again.
                None => {
                    if i > table.len() {
                        out.fault(Fault::AsNext { at });
                    }
                    match before.at {
                        Some((from, len)) => copy(out, (from, len + 1)),
                        None => {
                            walked.clear();
                            walked.push(before.first);
                            walk(&table, before.code, &out.data, &mut walked);
                            give(out, &mut walked)
                        }
                    }
                }
            },
        };
        if written.is_err() {
            return Prefix::unended(bits.read_to());
        }
        let first = out.data[start];
        // After a code past the table, a code from 258 on is the last
        // read.
        if entry.is_some() && before.code > END && usize::from(before.code - END - 1) >= table.len()
        {
            return Prefix::unended(bits.read_to());
        }
        if !full {
            let index = 258 + table.len();
            table.push(match before.at {
                Some((from, len)) => Entry::At(from, len + 1),
                None => Entry::After(before.code, first),
            });
            if matches!(index + usize::from(early_change), 511 | 1023 | 2047) {
                width += 1;
            }
        }
        // Its bytes are the table's each time it's read where it's a
        // byte's, or its entry, made now or before, holds a place.
        let held = match entry {
            None => true,
            Some(i) => matches!(table.get(i), Some(Entry::At(..))),
        };
        last = Some(Last {
            code,
            at: held.then(|| (start, out.data.len() - start)),
            first,
        });
    }
}

/// The bytes `code` gives read through `table`, from the last back, after
/// what `walked` holds, until it holds [`RUN`]: a byte's, itself; an
/// entry's held at a place, its bytes there in `decoded`; an entry's made
/// after a code, its byte, then that code's; a code past the table, its
/// low byte, as PDFium's decoder reads each.
fn walk(table: &[Entry], mut code: u16, decoded: &[u8], walked: &mut Vec<u8>) {
    while walked.len() < RUN {
        let entry = code
            .checked_sub(258)
            .and_then(|i| table.get(usize::from(i)));
        match entry {
            Some(&Entry::After(before, byte)) => {
                walked.push(byte);
                code = before;
            }
            Some(&Entry::At(start, len)) => {
                let take = len.min(RUN - walked.len());
                walked.extend(decoded[start + len - take..start + len].iter().rev());
                return;
            }
            None => {
                walked.push(code as u8);
                return;
            }
        }
    }
}

/// `walked`, bytes from the last back, added in their order, as many as
/// the limit allows.
fn give(out: &mut Out, walked: &mut [u8]) -> Result<(), ()> {
    walked.reverse();
    out.extend(walked)
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

    /// How many bytes the codes read so far take: to the last bit of the
    /// last, rounded up, whatever the buffer took after it.
    fn read_to(&self) -> usize {
        (self.next * 8 - self.held as usize).div_ceil(8)
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
    use crate::fuzz::{backends, prefix, prefix_as};
    use crate::{Fault, Filter, Predictor, Reading, decode, decode_into_as, decode_prefix_into};
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

    /// `data` read as data that may run on: how far, whether it ended,
    /// what it gave and its faults.
    fn read(data: &[u8], early_change: bool) -> (usize, bool, Vec<u8>, Vec<Fault>) {
        let predictor = Predictor::None;
        let filter = Filter::Lzw {
            early_change,
            predictor,
        };
        let (prefix, out, faults) = prefix(data, filter, backends()[0]);
        (prefix.read, prefix.ended, out, faults)
    }

    /// Data that runs on is read to its code of 257: to the code's last
    /// bit, rounded up to a byte, however many bytes after it the buffer
    /// took, as wide as the codes are, and past a full table.
    #[test]
    fn what_it_read_is_to_its_257() {
        let after = [&b" EI Q"[..], &[0xff; 16]].concat();
        let spec = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        let data = [&spec[..], &after].concat();
        assert_eq!(read(&data, true), (9, true, b"-----A---B".to_vec(), vec![]));
        let wide: Vec<u16> = (0..600u16).map(|i| i * 37 % 251 + 1).collect();
        let fill = filling();
        let full = fill.iter().copied().chain([65, 300, 4095]);
        for early_change in [true, false] {
            for codes in [
                vec![CLEAR, 65, 66, END],
                vec![65, 66, 258, END],
                vec![CLEAR, 65, 258, END],
                vec![CLEAR, END],
                [&[CLEAR][..], &wide, &[END]].concat(),
                full.clone().chain([END]).collect(),
            ] {
                let written = Codes::all(early_change, codes.iter().copied());
                let data = [&written[..], &after].concat();
                let (read, ended, _, _) = read(&data, early_change);
                assert_eq!(
                    (read, ended),
                    (written.len(), true),
                    "{early_change}, {codes:?}"
                );
            }
        }
    }

    /// Without a code of 257, it's read to the last whole code before the
    /// data's end, rounded up, the bits after it too few for a code; at a
    /// code not in the table, to that code, where PDFium's decoder reads
    /// on as if it were the table's next entry.
    #[test]
    fn what_it_read_without_its_257() {
        let codes = Codes::all(true, [CLEAR, 65, 66, 67, 68, 69, 70, 71]);
        assert_eq!(codes.len(), 9);
        let data = [&codes[..], &[0xff]].concat();
        let no_end = vec![Fault::NoEnd];
        assert_eq!(
            read(&data, true),
            (9, false, b"ABCDEFG".to_vec(), no_end.clone())
        );
        assert_eq!(read(&codes, true), (9, false, b"ABCDEFG".to_vec(), no_end));
        // 256 65 66 300 257: 300 isn't in the table, at bit 27.
        let data = [&Codes::all(true, [CLEAR, 65, 66, 300, END])[..], b" EI"].concat();
        let code = vec![Fault::Code { at: 3 }];
        assert_eq!(read(&data, true), (5, false, b"AB".to_vec(), code));
        // A code of 258 first: no entry yet.
        let data = [&Codes::all(true, [CLEAR, 258, END])[..], b" EI"].concat();
        let code = vec![Fault::Code { at: 1 }];
        assert_eq!(read(&data, true), (3, false, vec![], code));
        assert_eq!(read(b"\x80", true), (0, false, vec![], vec![Fault::NoEnd]));
    }

    /// At the limit, it's read to the code whose bytes are over it.
    #[test]
    fn what_it_read_at_the_limit() {
        let filter = Filter::Lzw {
            early_change: true,
            predictor: Predictor::None,
        };
        let data = [&Codes::all(true, [CLEAR, 65, 66, 258, END])[..], b" EI"].concat();
        for backend in backends() {
            let mut out = Vec::new();
            let (prefix, decoded) = decode_prefix_into(&data, filter, 3, backend, &mut out);
            assert_eq!(
                (prefix.read, prefix.ended, &out[..]),
                (5, false, &b"ABA"[..])
            );
            assert_eq!(decoded.unwrap_err().0, [(0, Fault::Limit)]);
        }
    }

    const A: u16 = 65;
    const B: u16 = 66;
    const C: u16 = 67;

    /// `codes` written, then ` EI`, read under PDFium's reading as data
    /// that may run on: how far, whether it ended, what it gave and its
    /// faults.
    fn as_pdfium(codes: &[u16], early_change: bool) -> (usize, bool, Vec<u8>, Vec<Fault>) {
        let data = [&Codes::all(early_change, codes.iter().copied())[..], b" EI"].concat();
        let filter = Filter::Lzw {
            early_change,
            predictor: Predictor::None,
        };
        let (prefix, out, faults) = prefix_as(&data, filter, backends()[0], Reading::Pdfium);
        (prefix.read, prefix.ended, out, faults)
    }

    /// Under PDFium's reading, a code past the table's next entry, but the
    /// first since a clear code, is read as that entry, the last code's
    /// bytes and their first byte again, and said; the codes after it read
    /// on: 257, its end; bytes; a code from 258 on, the last read. After
    /// 256 65 66 the table's next entry is 259, so 260 is past it by one,
    /// and 300 and 511 by more; 256 is 9 bits wide, at byte 0, 65 at bit
    /// 9, 66 at 18 and the code past the table at 27, byte 3.
    #[test]
    fn under_pdfiums_reading_a_code_past_the_next_entry_reads_as_it() {
        let as_next = vec![Fault::AsNext { at: 3 }];
        for (codes, read, ended, out) in [
            (&[CLEAR, A, B, 260, END][..], 6, true, &b"ABBB"[..]),
            (&[CLEAR, A, B, 300, END], 6, true, b"ABBB"),
            (&[CLEAR, A, B, 511, A, END], 7, true, b"ABBBA"),
            (&[CLEAR, A, B, 300, A, B, END], 8, true, b"ABBBAB"),
            // A code from 258 on after it is the last read: an entry's, and
            // one past the table, which gives the code's low byte (300's is
            // 44, `,`) and the first byte the last gave.
            (&[CLEAR, A, B, 300, 258, A, END], 6, false, b"ABBBAB"),
            (&[CLEAR, A, B, 300, 301, END], 6, false, b"ABBB,B"),
            // Read on past it twice, bytes between: the second's the next
            // entry's then, 65 and its first byte.
            (&[CLEAR, A, B, 300, A, 300, END], 8, true, b"ABBBAAA"),
        ] {
            let got = as_pdfium(codes, true);
            assert_eq!(
                got,
                (read, ended, out.to_vec(), as_next.clone()),
                "{codes:?}"
            );
        }
        // With no clear code first, the start is as one.
        let got = as_pdfium(&[A, B, 300, A, END], true);
        let want = (6, true, b"ABBBA".to_vec(), vec![Fault::AsNext { at: 2 }]);
        assert_eq!(got, want);
    }

    /// Under PDFium's reading, the entry a code makes after one past the
    /// table holds that code, read through the table each time the entry
    /// is read: still past it, the code's low byte, then the byte the entry
    /// added (300 65 66 300 67, then 260, its entry: `,C`); the code's own
    /// entry, where the entry is the code's, a run of its byte, the last
    /// 4,000 of it, as PDFium's decoder keeps (260 67, then 260).
    #[test]
    fn under_pdfiums_reading_an_entry_after_a_code_past_the_table_reads_through_it() {
        let got = as_pdfium(&[CLEAR, A, B, 300, C, 260, END], true);
        assert_eq!(
            got,
            (8, true, b"ABBBC,C".to_vec(), vec![Fault::AsNext { at: 3 }])
        );
        let (read, ended, out, faults) = as_pdfium(&[CLEAR, A, B, 260, C, 260, END], true);
        let want = [&b"ABBBC"[..], &[b'C'; 4000]].concat();
        assert_eq!(
            (read, ended, out, faults),
            (8, true, want, vec![Fault::AsNext { at: 3 }])
        );
        // A code past the table after it makes an entry of its own, whose
        // bytes are where they were decoded: 256 65 66 67 300 65 258, the
        // second 258 `AB` again.
        let got = as_pdfium(&[CLEAR, A, B, C, 300, A, 258, END], true);
        assert_eq!(
            got,
            (9, true, b"ABCCCAAB".to_vec(), vec![Fault::AsNext { at: 4 }])
        );
        // Under the limit, what the run gives is cut there.
        let data = Codes::all(true, [CLEAR, A, B, 260, C, 260, END]);
        let filter = Filter::Lzw {
            early_change: true,
            predictor: Predictor::None,
        };
        let mut out = Vec::new();
        let decoded = decode_into_as(
            &data,
            &[filter],
            100,
            backends()[0],
            Reading::Pdfium,
            &mut out,
        );
        assert_eq!(out, [&b"ABBBC"[..], &[b'C'; 95]].concat());
        let faults = [(0, Fault::AsNext { at: 3 }), (0, Fault::Limit)];
        assert_eq!(decoded.unwrap_err().0, faults);
    }

    /// Under PDFium's reading, a code past the table makes the table's
    /// next entry, and the codes widen with it as with any: past it by one
    /// at 9 bits wide, the table's next entry 510, with early change the
    /// codes after it are 10 bits wide, without it the code after that.
    /// And past it after a full table and a clear code.
    #[test]
    fn under_pdfiums_reading_codes_widen_past_a_code_past_the_table() {
        let bytes: Vec<u16> = (0..253u16).map(|i| i * 37 % 251 + 1).collect();
        let last = *bytes.last().unwrap() as u8;
        let mut want: Vec<u8> = bytes.iter().map(|&b| b as u8).collect();
        want.extend([last, last, b'A']);
        for early_change in [true, false] {
            let codes = [&[CLEAR][..], &bytes, &[511, A, END]].concat();
            let written = Codes::all(early_change, codes.iter().copied());
            assert_eq!(written.len(), 290);
            let (read, ended, out, faults) = as_pdfium(&codes, early_change);
            assert_eq!((read, ended, &out), (290, true, &want), "{early_change}");
            assert_eq!(faults, [Fault::AsNext { at: 285 }], "{early_change}");
        }
        let fill = filling();
        let mut want: Vec<u8> = fill.iter().map(|&b| b as u8).collect();
        want.extend(b"ABBB");
        for early_change in [true, false] {
            let codes = [&fill[..], &[CLEAR, A, B, 300, END]].concat();
            let read = Codes::all(early_change, codes.iter().copied()).len();
            let (got, ended, out, faults) = as_pdfium(&codes, early_change);
            assert_eq!((got, ended, &out), (read, true, &want), "{early_change}");
            assert!(matches!(faults[..], [Fault::AsNext { .. }]), "{faults:?}");
        }
    }

    /// Under PDFium's reading, codes in the table read as under qpdf's:
    /// the spec's example, codes widening to 12 bits, a full table read
    /// on, no end, the limit.
    #[test]
    fn under_pdfiums_reading_codes_in_the_table_read_as_under_qpdfs() {
        let mut x = 7u32;
        let noise: Vec<u8> = (0..20_000)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (x >> 16) as u8
            })
            .collect();
        let fill = filling();
        for early_change in [true, false] {
            let full = Codes::all(
                early_change,
                fill.iter().copied().chain([A, 300, 4095, END]),
            );
            let spec = encode(b"-----A---B", early_change);
            let encoded = encode(&noise, early_change);
            for data in [&spec[..], &spec[..5], &encoded, &full, &full[..3000]] {
                let filter = Filter::Lzw {
                    early_change,
                    predictor: Predictor::None,
                };
                for backend in backends() {
                    let qpdf = prefix_as(data, filter, backend, Reading::Qpdf);
                    let pdfium = prefix_as(data, filter, backend, Reading::Pdfium);
                    assert_eq!(pdfium, qpdf, "{early_change}, {} bytes", data.len());
                }
                let mut out = Vec::new();
                let pdfium =
                    decode_into_as(data, &[filter], 9, backends()[0], Reading::Pdfium, &mut out);
                let qpdf = decode(data, &[filter], 9, backends()[0]).unwrap_err();
                assert_eq!((out, pdfium.unwrap_err().0), (qpdf.data, qpdf.faults));
            }
        }
    }
}
