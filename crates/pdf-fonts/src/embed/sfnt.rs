//! A face's tables as the subsetter reads them, and the checks that keep what it does
//! with them bounded by their size.
//!
//! The subsetter reads a face otherwise than skrifa and read-fonts do: its table
//! directory by a binary search, so only in order of its tags; a collection's face
//! where the collection's first list of faces puts it; a glyph's data as `loca`
//! places it, unchecked for order; a composite glyph's components as far as their
//! glyphs' numbers read; and its `name` records' lengths summed in 16 bits. So what's
//! checked here is read its way, from the same bytes, and what passes is what it's
//! given.

use std::collections::HashSet;

use skrifa::raw::{FontRef, TableRecord};

use super::{Embedding, Error, Outlines};

/// A face's tables, by the records of its table directory, in order of their tags.
pub(super) struct Tables<'a> {
    data: &'a [u8],
    records: &'a [TableRecord],
}

impl<'a> Tables<'a> {
    /// `font`'s tables, read-fonts' face of `data`, where the subsetter finds the same:
    /// every record its directory counts, in order of their tags with none twice, and,
    /// in a collection, the face where the collection's first list puts it.
    pub(super) fn of(data: &'a [u8], font: &FontRef<'a>) -> Result<Tables<'a>, Error> {
        let directory = &font.table_directory;
        let records = directory.table_records();
        let ordered = records
            .windows(2)
            .all(|pair| pair[0].tag().to_be_bytes() < pair[1].tag().to_be_bytes());
        if records.len() != usize::from(directory.num_tables()) || !ordered {
            return Err(Error::NotSfnt);
        }
        if let Some(index) = font.ttc_index() {
            let directory = directory.offset_data().as_bytes().as_ptr() as usize;
            let at = directory.checked_sub(data.as_ptr() as usize);
            let listed = (index as usize)
                .checked_mul(4)
                .and_then(|i| data.get(12 + i..16 + i))
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
            if listed.is_none() || listed != at {
                return Err(Error::NotSfnt);
            }
        }
        Ok(Tables { data, records })
    }

    /// The table tagged `tag`, and where it begins in the program: none where it's
    /// missing or its record reaches past the program.
    fn table(&self, tag: &[u8; 4]) -> Option<(usize, &'a [u8])> {
        let at = self
            .records
            .binary_search_by(|record| record.tag().to_be_bytes().cmp(tag))
            .ok()?;
        let record = &self.records[at];
        let start = record.offset() as usize;
        let end = start.checked_add(record.length() as usize)?;
        Some((start, self.data.get(start..end)?))
    }

    /// Whether its outlines are TrueType's, as the subsetter tells them: a `glyf`
    /// table, whatever else it has.
    pub(super) fn outlines(&self) -> Result<(), Error> {
        let has = |tag| self.table(tag).is_some();
        match () {
            () if has(b"glyf") => Ok(()),
            () if has(b"CFF ") => Err(Error::Outlines(Outlines::Cff)),
            () if has(b"CFF2") => Err(Error::Outlines(Outlines::Cff2)),
            () if has(b"EBDT") || has(b"CBDT") || has(b"sbix") => {
                Err(Error::Outlines(Outlines::Bitmaps))
            }
            () => Err(Error::Malformed { table: "glyf" }),
        }
    }

    /// Where its `hmtx` begins, where it holds the `long` advances its `hhea` says it
    /// does.
    pub(super) fn hmtx(&self, long: u16) -> Option<usize> {
        let (at, hmtx) = self.table(b"hmtx")?;
        (hmtx.len() >= 4 * usize::from(long)).then_some(at)
    }

    /// Its `glyf` and `loca`, and `loca`'s format, as its `head` gives it.
    pub(super) fn glyf(&self) -> Result<Glyf<'a>, Error> {
        let (_, loca) = self
            .table(b"loca")
            .ok_or(Error::Malformed { table: "loca" })?;
        let (_, glyf) = self
            .table(b"glyf")
            .ok_or(Error::Malformed { table: "glyf" })?;
        let head = self.table(b"head").and_then(|(_, head)| head.get(50..52));
        let head = head.ok_or(Error::Malformed { table: "head" })?;
        Ok(Glyf {
            loca,
            glyf,
            // Long for any format but 0, as the subsetter and FreeType take it.
            long: head != [0, 0],
        })
    }

    /// Refused where a subset couldn't write its `name` table, which the subsetter
    /// cuts to its Unicode records of the names it keeps, each string once, adding
    /// their offsets and lengths, and the table's size, in 16 bits: a record of those
    /// reaching past 65,535 bytes or past the table, their strings past 65,535 bytes
    /// together, more records than 65,535 bytes holds, or records cut short. A table
    /// of any version but 0 it keeps whole, and one it hasn't it leaves out.
    pub(super) fn check_name(&self) -> Result<(), Error> {
        let malformed = || Error::Malformed { table: "name" };
        let Some((_, name)) = self.table(b"name") else {
            return Ok(());
        };
        let word = |at: usize| {
            name.get(at..at + 2)
                .map(|b| u16::from_be_bytes([b[0], b[1]]))
        };
        match word(0) {
            Some(0) => {}
            Some(_) => return Ok(()),
            None => return Err(malformed()),
        }
        let (count, storage) = word(2).zip(word(4)).ok_or_else(malformed)?;
        let records = name.get(6..6 + 12 * usize::from(count));
        let records = records.ok_or_else(malformed)?;
        let storage = name.get(usize::from(storage)..).ok_or_else(malformed)?;
        let mut kept = 0;
        let mut strings = HashSet::new();
        let mut stored = 0;
        for record in records.as_chunks::<12>().0 {
            let field = |i: usize| u16::from_be_bytes([record[2 * i], record[2 * i + 1]]);
            let (platform, encoding, id) = (field(0), field(1), field(3));
            let unicode = platform == 0 || platform == 3 && matches!(encoding, 0 | 1 | 10);
            if !unicode || !matches!(id, 0..=9 | 11..=14) {
                continue;
            }
            kept += 1;
            let (length, offset) = (usize::from(field(4)), usize::from(field(5)));
            let end = offset + length;
            let string = storage.get(offset..end).filter(|_| end <= 0xFFFF);
            let string = string.ok_or_else(malformed)?;
            if strings.insert(string) {
                stored += length;
            }
        }
        // Each kept record twelve bytes after a header of six, in 16 bits.
        match kept <= 5460 && stored <= 0xFFFF {
            true => Ok(()),
            false => Err(malformed()),
        }
    }

    /// What its licence allows: its `OS/2` table's `fsType`, where it has one long
    /// enough to hold it.
    pub(super) fn embedding(&self) -> Embedding {
        let fs_type = self.table(b"OS/2").and_then(|(_, os2)| os2.get(8..10));
        Embedding::of(fs_type.map_or(0, |b| u16::from_be_bytes([b[0], b[1]])))
    }
}

/// A face's `glyf` and `loca`, read as the subsetter reads them.
pub(super) struct Glyf<'a> {
    loca: &'a [u8],
    glyf: &'a [u8],
    long: bool,
}

impl<'a> Glyf<'a> {
    /// Glyph `glyph`'s data, as `loca` places it: none where `loca` hasn't its entries
    /// or they don't place it within `glyf`, starting before it ends.
    pub(super) fn glyph(&self, glyph: u16) -> Option<&'a [u8]> {
        let entry = |n: usize| match self.long {
            true => self
                .loca
                .get(4 * n..4 * n + 4)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize),
            false => self
                .loca
                .get(2 * n..2 * n + 2)
                .map(|b| 2 * usize::from(u16::from_be_bytes([b[0], b[1]]))),
        };
        let at = usize::from(glyph);
        self.glyf.get(entry(at)?..entry(at + 1)?)
    }

    /// Refused where the face's `count` glyphs' data, as `loca` places it, comes to
    /// more than its `glyf` holds, so that two glyphs share bytes and a subset of them
    /// would hold more than the program does (`loca`), or where a composite glyph names
    /// a glyph past `count` (`glyf`), which a subset would take on, past the 65,535 its
    /// glyphs are numbered in. The glyphs' data is summed first, so that each
    /// composite's components after it are read once, in all no more than `glyf`.
    pub(super) fn check(&self, count: u16) -> Result<(), Error> {
        let mut drawn = 0;
        for glyph in 0..count {
            drawn += self.glyph(glyph).map_or(0, <[u8]>::len);
            if drawn > self.glyf.len() {
                return Err(Error::Malformed { table: "loca" });
            }
        }
        for glyph in 0..count {
            let mut components = self.glyph(glyph).into_iter().flat_map(components);
            if components.any(|component| component >= count) {
                return Err(Error::Malformed { table: "glyf" });
            }
        }
        Ok(())
    }
}

/// The glyphs a glyph's `data` names, where it's a composite, read as the subsetter
/// reads them, so that what's checked and kept here is what it keeps: each record's
/// flags and glyph, its arguments and transform skipped as its flags say, until one
/// says it's the last or the data ends. A record cut short after its glyph still names
/// it. None for a simple or an empty glyph, or a composite too short for its header.
pub(super) fn components(data: &[u8]) -> impl Iterator<Item = u16> + '_ {
    let composite = data.len() >= 10 && i16::from_be_bytes([data[0], data[1]]) < 0;
    let word = |at: usize| {
        data.get(at..at + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
    };
    let mut at = 10;
    let mut last = !composite;
    std::iter::from_fn(move || {
        if last {
            return None;
        }
        let (flags, glyph) = word(at).zip(word(at + 2))?;
        // Words or bytes for its offsets; a scale, an x and y scale, or a 2 by 2.
        let arguments = if flags & 0x0001 != 0 { 4 } else { 2 };
        let transform = match flags {
            f if f & 0x0008 != 0 => 2,
            f if f & 0x0040 != 0 => 4,
            f if f & 0x0080 != 0 => 8,
            _ => 0,
        };
        at += 4 + arguments + transform;
        last = flags & 0x0020 == 0;
        Some(glyph)
    })
}
