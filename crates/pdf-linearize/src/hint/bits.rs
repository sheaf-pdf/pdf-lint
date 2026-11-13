//! Bits, high-order first, through bitstream-io, with what a hint table wants
//! around them: a position to say where a read went wrong, a cap, and nothing
//! of that crate's in what the rest of the module sees.

use bitstream_io::{BigEndian, BitRead, BitReader, BitWrite, BitWriter};

use super::Error;
use crate::Limits;

/// The most bits a field of a hint table has (F.4.2).
const WIDEST: u32 = 32;

/// How many bits `value` takes: none for 0, as a width of 0 can hold only that.
pub(super) const fn width_of(value: u64) -> u16 {
    (u64::BITS - value.leading_zeros()) as u16
}

/// A table's bits, being read.
pub(super) struct Reader<'a> {
    bits: BitReader<&'a [u8], BigEndian>,
    len: u64,
    at: u64,
    charged: u64,
    limit: u64,
}

impl<'a> Reader<'a> {
    /// The bits of `data`, from its first, nothing charged yet.
    pub(super) fn new(data: &'a [u8], limits: &Limits) -> Reader<'a> {
        Reader {
            bits: BitReader::new(data),
            len: data.len() as u64 * 8,
            at: 0,
            charged: 0,
            limit: limits.hint_entries,
        }
    }

    /// What's been charged to this count of entries, by this table and before.
    pub(super) fn charged(&self) -> u64 {
        self.charged
    }

    /// Goes on from a count of entries other tables have charged.
    pub(super) fn charged_already(&mut self, charged: u64) {
        self.charged = charged;
    }

    /// The next `width` bits, a field of what `what` says, `width` at most 32.
    pub(super) fn field(&mut self, width: u16, what: &'static str) -> Result<u32, Error> {
        let width = u32::from(width);
        if width > WIDEST {
            return Err(Error::Width { what, width });
        }
        let value = self.bits.read_var::<u32>(width);
        // Reading can only fail at the data's end, a width past 32 being
        // refused.
        let value = value.map_err(|_| Error::PastEnd {
            what,
            bit: self.at,
            bits: self.len,
        })?;
        self.at += u64::from(width);
        Ok(value)
    }

    /// The next 32 bits.
    pub(super) fn bits32(&mut self, what: &'static str) -> Result<u32, Error> {
        self.field(32, what)
    }

    /// The next 16 bits.
    pub(super) fn bits16(&mut self, what: &'static str) -> Result<u16, Error> {
        Ok(self.field(16, what)? as u16)
    }

    /// The next 16 bits, the width of `what`, at most 32 (F.4.2).
    pub(super) fn width(&mut self, what: &'static str) -> Result<u16, Error> {
        let width = self.bits16(what)?;
        self.check_width(width, what)?;
        Ok(width)
    }

    /// `width`, the width of `what`, at most 32.
    pub(super) fn check_width(&self, width: u16, what: &'static str) -> Result<(), Error> {
        match u32::from(width) > WIDEST {
            true => Err(Error::Width {
                what,
                width: u32::from(width),
            }),
            false => Ok(()),
        }
    }

    /// On to the next byte boundary: each run of an item ends on one.
    pub(super) fn align(&mut self) {
        self.bits.byte_align();
        self.at = self.at.next_multiple_of(8);
    }

    /// That there are the bits for `count` things of `each` bits, before
    /// there's room made for them.
    pub(super) fn room(&self, count: u64, each: u64, what: &'static str) -> Result<(), Error> {
        let wanted = u128::from(count) * u128::from(each);
        if wanted > u128::from(self.len - self.at) {
            return Err(Error::PastEnd {
                what,
                bit: self.at,
                bits: self.len,
            });
        }
        Ok(())
    }

    /// `entries` more of what a width of 0 could otherwise have for nothing.
    pub(super) fn charge(&mut self, entries: u64) -> Result<(), Error> {
        self.charged = self.charged.saturating_add(entries);
        if self.charged > self.limit {
            return Err(Error::TooManyEntries {
                entries: self.charged,
                limit: self.limit,
            });
        }
        Ok(())
    }
}

/// A table's bits, being written, added to a list of bytes.
pub(super) struct Writer<'a> {
    bits: BitWriter<&'a mut Vec<u8>, BigEndian>,
}

impl<'a> Writer<'a> {
    /// A table's bits, added to `out` from the boundary it's at.
    pub(super) fn new(out: &'a mut Vec<u8>) -> Writer<'a> {
        Writer {
            bits: BitWriter::new(out),
        }
    }

    /// `value` in `width` bits, if it fits.
    fn field(&mut self, value: u64, width: u16, what: &'static str) -> Result<(), Error> {
        let bad = Error::Value {
            what,
            value,
            width: u32::from(width),
        };
        if u32::from(width) > WIDEST || width_of(value) > width {
            return Err(bad);
        }
        // Into a list of bytes, of what fits: neither of what could fail.
        (self.bits)
            .write_var::<u32>(u32::from(width), value as u32)
            .map_err(|_| bad)
    }

    /// `value` in 32 bits.
    pub(super) fn bits32(&mut self, value: u32) -> Result<(), Error> {
        self.field(u64::from(value), 32, "a field of 32 bits")
    }

    /// `value` in 16 bits.
    pub(super) fn bits16(&mut self, value: u16) -> Result<(), Error> {
        self.field(u64::from(value), 16, "a field of 16 bits")
    }

    /// Each of `values` in `width` bits, then to a byte boundary.
    pub(super) fn column(
        &mut self,
        values: impl Iterator<Item = u64>,
        width: u16,
        what: &'static str,
    ) -> Result<(), Error> {
        for value in values {
            self.field(value, width, what)?;
        }
        self.align()
    }

    /// Each of `values`, less `least`, in `width` bits, then to a byte
    /// boundary.
    pub(super) fn less_column(
        &mut self,
        values: impl Iterator<Item = u64>,
        least: u32,
        width: u16,
        what: &'static str,
    ) -> Result<(), Error> {
        for value in values {
            let delta = value.checked_sub(u64::from(least));
            let delta = delta.ok_or(Error::Value {
                what,
                value,
                width: u32::from(width),
            })?;
            self.field(delta, width, what)?;
        }
        self.align()
    }

    /// To a byte boundary.
    fn align(&mut self) -> Result<(), Error> {
        self.bits.byte_align().map_err(|_| Error::Value {
            what: "a byte boundary",
            value: 0,
            width: 0,
        })
    }

    /// The table's done: to a byte boundary, as the next table starts on one.
    pub(super) fn finish(mut self) -> Result<(), Error> {
        self.align()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What this module takes from bitstream-io, held here, so a version that
    /// behaves otherwise fails.
    #[test]
    fn its_widths_and_its_end_are_as_a_table_wants() {
        let limits = Limits::DEFAULT;
        let data = [0xAB, 0xCD, 0xEF, 0x01, 0x80];
        let mut bits = Reader::new(&data, &limits);
        // A width of 0 gives 0 and takes nothing.
        assert_eq!(bits.field(0, "none"), Ok(0));
        assert_eq!(bits.at, 0);
        // High-order first; 32 bits whole.
        assert_eq!(bits.field(32, "all"), Ok(0xABCD_EF01));
        // A boundary it's on stays where it is, and one it isn't goes on.
        bits.align();
        assert_eq!(bits.at, 32);
        assert_eq!(bits.field(1, "one"), Ok(1));
        bits.align();
        assert_eq!(bits.at, 40);
        // The end is an error before bitstream-io's asked.
        assert_eq!(
            bits.field(1, "past"),
            Err(Error::PastEnd {
                what: "past",
                bit: 40,
                bits: 40,
            })
        );
        assert_eq!(bits.field(0, "none"), Ok(0));
        // Nor does a width past 32 come to a shift, or to that data.
        assert_eq!(
            bits.field(33, "wide"),
            Err(Error::Width {
                what: "wide",
                width: 33,
            })
        );

        let mut out = vec![0x11];
        let mut bits = Writer::new(&mut out);
        bits.field(0, 0, "none").unwrap();
        bits.field(1, 1, "one").unwrap();
        bits.align().unwrap();
        bits.field(u64::from(u32::MAX), 32, "all").unwrap();
        bits.field(5, 3, "three").unwrap();
        assert_eq!(
            bits.field(8, 3, "over"),
            Err(Error::Value {
                what: "over",
                value: 8,
                width: 3,
            })
        );
        // A value past 32 bits isn't cut down to fit them.
        assert_eq!(
            bits.field(1 << 32, 32, "big"),
            Err(Error::Value {
                what: "big",
                value: 1 << 32,
                width: 32,
            })
        );
        assert_eq!(
            bits.field(0, 33, "wide"),
            Err(Error::Value {
                what: "wide",
                value: 0,
                width: 33,
            })
        );
        bits.finish().unwrap();
        assert_eq!(out, [0x11, 0x80, 0xFF, 0xFF, 0xFF, 0xFF, 0xA0]);
    }

    #[test]
    fn a_width_is_the_bits_a_value_takes() {
        let cases = [(0, 0), (1, 1), (2, 2), (3, 2), (4, 3), (255, 8), (256, 9)];
        for (value, width) in cases {
            assert_eq!(width_of(value), width, "{value}");
        }
        assert_eq!(width_of(u64::from(u32::MAX)), 32);
    }
}
