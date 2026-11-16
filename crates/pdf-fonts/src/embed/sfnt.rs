//! A face's tables as the subsetter reads them, and the checks that keep what it does
//! with them bounded by their size, and every cut of them one it writes.
//!
//! The subsetter reads a face otherwise than skrifa and read-fonts do: its table
//! directory by a binary search, so only in order of its tags; a collection's face
//! where the collection's first list of faces puts it; a glyph's data as `loca`
//! places it, unchecked for order; a composite glyph's components as far as their
//! glyphs' numbers read; and its `name` records' lengths summed in 16 bits. And it
//! refuses a cut holding a glyph whose data, metrics or name it can't read, and
//! writes some tables it rewrites wrong where they aren't as OpenType has them. So
//! what's checked here is read its way, from the same bytes, every glyph of it, and
//! what passes is what it's given.

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
            return Err(Error::Directory);
        }
        if let Some(index) = font.ttc_index() {
            let directory = directory.offset_data().as_bytes().as_ptr() as usize;
            let at = directory.checked_sub(data.as_ptr() as usize);
            let listed = (index as usize)
                .checked_mul(4)
                .and_then(|i| data.get(12 + i..16 + i))
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
            if listed.is_none() || listed != at {
                return Err(Error::Directory);
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
    /// does, and a left side bearing for each of its `glyphs` after them, as OpenType
    /// has it and the subsetter reads each glyph it keeps.
    pub(super) fn hmtx(&self, long: u16, glyphs: u16) -> Option<usize> {
        let (at, hmtx) = self.table(b"hmtx")?;
        let bearings = usize::from(glyphs.saturating_sub(long));
        (hmtx.len() >= 4 * usize::from(long) + 2 * bearings).then_some(at)
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

    /// Refused where a table the subsetter rewrites, to the glyphs it keeps, would be
    /// written wrong or refused for some of its `glyphs`: an `hhea` longer than the 36
    /// bytes OpenType gives it, as the subsetter writes the advances' count over its
    /// last two; a `maxp` of a version but 1.0, which a TrueType program's is and the
    /// subsetter writes it as, over what follows; and a `post` too short for its
    /// version, or of version 2.0 and too short for its header and its glyphs' names,
    /// naming fewer glyphs than the program has, or naming more glyphs by names of its
    /// own than the 65,278 the subsetter numbers them to in 16 bits.
    pub(super) fn check_rewritten(&self, glyphs: u16) -> Result<(), Error> {
        let malformed = |table| Error::Malformed { table };
        if self.table(b"hhea").is_some_and(|(_, hhea)| hhea.len() > 36) {
            return Err(malformed("hhea"));
        }
        let maxp = self.table(b"maxp").and_then(|(_, maxp)| maxp.get(..4));
        if maxp != Some(&[0, 1, 0, 0][..]) {
            return Err(malformed("maxp"));
        }
        let Some((_, post)) = self.table(b"post") else {
            return Ok(());
        };
        match post.get(..4) {
            Some([0, 2, 0, 0]) => {}
            Some(_) => return Ok(()),
            None => return Err(malformed("post")),
        }
        let named = post.get(32..34).map(|b| u16::from_be_bytes([b[0], b[1]]));
        let named = named.ok_or(malformed("post"))?;
        let indexes = post.get(34..34 + 2 * usize::from(named));
        let indexes = indexes.ok_or(malformed("post"))?;
        // Indexes past the 258 standard names are the program's own.
        let own = indexes.as_chunks::<2>().0.iter().take(usize::from(glyphs));
        let own = own
            .filter(|&&index| u16::from_be_bytes(index) >= 258)
            .count();
        match named >= glyphs && own <= 65_278 {
            true => Ok(()),
            false => Err(malformed("post")),
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

    /// Refused where one of the face's `count` glyphs isn't placed by `loca`, or
    /// their data, as `loca` places it, comes to more than its `glyf` holds, so that
    /// two glyphs share bytes and a subset of them would hold more than the program
    /// does (`loca`); or where a glyph's data doesn't read as the subsetter writes it
    /// ([`whole`]), or a composite glyph names a glyph past `count`, which a subset
    /// would take on, past the 65,535 its glyphs are numbered in (`glyf`). The glyphs'
    /// data is summed first, so that each composite's components after it are read
    /// twice at most, in all no more than `glyf`.
    pub(super) fn check(&self, count: u16) -> Result<(), Error> {
        let mut drawn = 0;
        for glyph in 0..count {
            let data = self.glyph(glyph);
            drawn += data.ok_or(Error::Malformed { table: "loca" })?.len();
            if drawn > self.glyf.len() {
                return Err(Error::Malformed { table: "loca" });
            }
        }
        for glyph in 0..count {
            let data = self.glyph(glyph).unwrap_or_default();
            if !whole(data) || components(data).any(|component| component >= count) {
                return Err(Error::Malformed { table: "glyf" });
            }
        }
        Ok(())
    }
}

/// How many bytes a composite glyph's record of `flags` takes: its flags and glyph,
/// its offsets in words or bytes, and a scale, an x and y scale, or a 2 by 2.
fn record(flags: u16) -> usize {
    let arguments = if flags & 0x0001 != 0 { 4 } else { 2 };
    let transform = match flags {
        f if f & 0x0008 != 0 => 2,
        f if f & 0x0040 != 0 => 4,
        f if f & 0x0080 != 0 => 8,
        _ => 0,
    };
    4 + arguments + transform
}

/// Whether a glyph's `data` reads as the subsetter writes a glyph it keeps: none, or
/// its contours' count, and, for a composite, its header and every record, to the
/// one that says it's the last.
fn whole(data: &[u8]) -> bool {
    let Some(&[high, low]) = data.get(..2) else {
        return data.is_empty();
    };
    if i16::from_be_bytes([high, low]) >= 0 {
        return true;
    }
    let mut at = 10;
    while let Some(flags) = data.get(at..at + 2) {
        let flags = u16::from_be_bytes([flags[0], flags[1]]);
        at += record(flags);
        if at > data.len() {
            return false;
        }
        if flags & 0x0020 == 0 {
            return true;
        }
    }
    false
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
        at += record(flags);
        last = flags & 0x0020 == 0;
        Some(glyph)
    })
}
