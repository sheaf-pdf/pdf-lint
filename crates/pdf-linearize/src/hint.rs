//! The hint tables (ISO 32000-2, F.4), both ways: what a linearized file says
//! of itself in its hint stream, as typed values, written to bits and read
//! back.
//!
//! A table's fields are bits, high-order first, of widths its header gives, and
//! it starts on a byte boundary (F.4.1). F.4.1 has the fields run on regardless
//! of bytes within a table, but what's written and read is otherwise: each run
//! of one item across the entries ends on a byte boundary, as qpdf and PDFium
//! read and write it and as real files have it, so the same here both ways.
//!
//! An entry holds what the table says, a count or a length, not the difference
//! from the least that the bits hold, so its fields are wider than the bits'
//! and adding the least to what's read can't overflow.
//!
//! # Reading anything
//!
//! A table comes from a file, so its fields can say anything. Reading past the
//! data is an error that says which bit; a width past 32 is an error before
//! it's used; a count that needs more bits than the data holds is an error
//! before there's room made for it; and as a width of 0 reads no bits at all,
//! every entry is also charged against [`Limits::hint_entries`], and pages
//! against [`Limits::pages`]. So a file of a hundred bytes can't ask for four
//! billion of anything.
//!
//! # Positions
//!
//! A position in a table is where a thing would be were the primary hint stream
//! not there (F.4.1): [`position`] takes where it is to that and [`located`]
//! takes it back.
//!
//! # Exhaustive on purpose
//!
//! Each table's header, entries and table are structs of public fields with
//! no field held back, as the standard's tables are closed and a checker or
//! a writer builds them whole: a field more would be a new standard's, and a
//! breaking release.
//!
//! # What isn't here
//!
//! A shared object's signature (F.4.3) is a value here, kept as read and
//! written as held, never computed: nothing's hashed.

use crate::Limits;

mod bits;
#[cfg(test)]
mod tests;

use bits::{Reader, Writer, width_of};

/// Why a hint table can't be read, or written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Reading ran past the data.
    #[error("{what} at bit {bit} runs past the hint data's {bits} bits")]
    PastEnd {
        /// The field being read.
        what: &'static str,
        /// The bit it starts at, in what was given to read.
        bit: u64,
        /// How many bits were given.
        bits: u64,
    },
    /// A header said a field is wider than a hint table's can be (F.4.2).
    #[error("{what}'s width is {width} bits, past 32")]
    Width {
        /// The field.
        what: &'static str,
        /// The width it said.
        width: u32,
    },
    /// A table's position in the data is past its end.
    #[error("the {table} table is at byte {at} of {len}")]
    TableAt {
        /// Which table.
        table: Table,
        /// The position its key gives.
        at: u64,
        /// How many bytes the data has.
        len: u64,
    },
    /// More pages than [`Limits::pages`].
    #[error("{pages} pages, past the cap of {limit}")]
    TooManyPages {
        /// How many were asked for.
        pages: u64,
        /// The cap.
        limit: usize,
    },
    /// More entries than [`Limits::hint_entries`].
    #[error("{entries} entries in hint tables, past the cap of {limit}")]
    TooManyEntries {
        /// How many were asked for by then.
        entries: u64,
        /// The cap.
        limit: u64,
    },
    /// A value that the widths its table says can't hold, in what's to be
    /// written.
    #[error("{what} can't hold {value} in the {width} bits its table has")]
    Value {
        /// The field.
        what: &'static str,
        /// The value.
        value: u64,
        /// The width.
        width: u32,
    },
}

/// A table of a hint stream (F.3.6, F.4): the page offset table, at the start
/// of the data, or one the stream's dictionary gives the position of by a key.
/// Its `Display` is how a finding names it, `page offset` or the key with its
/// slash, `/S`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Table {
    /// The page offset table (F.4.2).
    PageOffsets,
    /// `/S`: the shared object table (F.4.3).
    Shared,
    /// `/T`: the thumbnail table (F.4.4).
    Thumbnails,
    /// `/O`: the outline table (F.4.5).
    Outlines,
    /// `/A`: the thread information table (F.4.5).
    Threads,
    /// `/E`: the named destination table (F.4.5).
    Dests,
    /// `/V`: the interactive form table (F.4.6).
    Form,
    /// `/I`: the information dictionary table (F.4.5).
    Info,
    /// `/C`: the logical structure table (F.4.6).
    Structure,
    /// `/L`: the page label table (F.4.5).
    Labels,
    /// `/R`: the renditions name tree table (F.4.6).
    Renditions,
    /// `/B`: the embedded file stream table (F.4.7).
    Files,
}

impl Table {
    /// Its key in the hint stream's dictionary, without the slash: `b"S"`.
    /// None for the page offset table, whose place is the data's start.
    #[must_use]
    pub const fn key(self) -> Option<&'static [u8]> {
        Some(match self {
            Table::PageOffsets => return None,
            Table::Shared => b"S",
            Table::Thumbnails => b"T",
            Table::Outlines => b"O",
            Table::Threads => b"A",
            Table::Dests => b"E",
            Table::Form => b"V",
            Table::Info => b"I",
            Table::Structure => b"C",
            Table::Labels => b"L",
            Table::Renditions => b"R",
            Table::Files => b"B",
        })
    }
}

impl std::fmt::Display for Table {
    /// `page offset`, or its key with the slash: `/S`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.key() {
            None => f.write_str("page offset"),
            Some(key) => write!(f, "/{}", char::from(key[0])),
        }
    }
}

/// Where a thing at `at` in a file reads in a hint table, under a primary hint
/// stream of `hint_len` bytes at `hint_at`: as if the stream weren't there
/// (F.4.1). `at` isn't within the stream, which nothing a table names can be.
#[must_use]
pub const fn position(at: u64, hint_at: u64, hint_len: u64) -> u64 {
    match at >= hint_at.saturating_add(hint_len) {
        true => at - hint_len,
        false => at,
    }
}

/// Where what a hint table says is at `position` is in the file, under a
/// primary hint stream of `hint_len` bytes at `hint_at`: [`position`] the other
/// way. The thing right after the stream reads as `hint_at`, where nothing else
/// can be.
#[must_use]
pub const fn located(position: u64, hint_at: u64, hint_len: u64) -> u64 {
    match position >= hint_at {
        true => position.saturating_add(hint_len),
        false => position,
    }
}

/// The width of an identifier when `most` is the greatest of them (F.4.2, Table
/// F.3's item 11): a bit at least, as in [`PageOffsets::of`].
#[must_use]
pub const fn id_width(most: u32) -> u16 {
    match width_of(most as u64) {
        0 => 1,
        width => width,
    }
}

/// The page offset hint table's header (F.4.2, Table F.3): each of its thirteen
/// items by what it says. Exhaustive, as the standard's table is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PageHeader {
    /// The fewest objects any page has.
    pub min_objects: u32,
    /// Where the first page's object is, as [`position`] has it.
    pub first_page_at: u32,
    /// The width of each entry's number of objects, less the least.
    pub objects_bits: u16,
    /// The shortest page's length.
    pub min_length: u32,
    /// The width of each entry's length, less the least.
    pub length_bits: u16,
    /// The nearest a content stream starts to its page's start.
    pub min_content_at: u32,
    /// The width of each entry's content offset, less the least.
    pub content_at_bits: u16,
    /// The shortest page's content's length.
    pub min_content_length: u32,
    /// The width of each entry's content length, less the least.
    pub content_length_bits: u16,
    /// The width of each entry's count of shared objects.
    pub count_bits: u16,
    /// The width of each shared object identifier.
    pub id_bits: u16,
    /// The width of each shared object reference's numerator.
    pub numerator_bits: u16,
    /// The denominator of those fractions.
    pub denominator: u16,
}

/// One page of the page offset hint table (F.4.2, Table F.4), the entries in
/// the order of the pages' numbers, whichever page is the first page.
/// Exhaustive, as the standard's table is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageEntry {
    /// How many objects the page has.
    pub objects: u64,
    /// The page's length in bytes.
    pub length: u64,
    /// The shared objects it refers to: each's identifier, an index into the
    /// shared object table, and the numerator of where it's first wanted.
    pub shared: Vec<(u32, u32)>,
    /// Where its content stream starts, from the page's start.
    pub content_at: u64,
    /// Its content's length, the objects' own bytes included.
    pub content_length: u64,
}

/// The page offset hint table (F.4.2). Exhaustive, as the standard's table is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageOffsets {
    /// Its header.
    pub header: PageHeader,
    /// An entry per page.
    pub pages: Vec<PageEntry>,
}

impl PageOffsets {
    /// The table of `pages`, the first page's object at `first_page_at`, with a
    /// header as narrow as a reader takes: PDFium's refuses the whole of a hint
    /// stream with a width of 0 for a page's objects, length or count of shared
    /// objects, or for an identifier, as useless, so each of those has a bit at
    /// least. It also takes the content offsets and lengths to be no wider
    /// together than a page's length, as Acrobat writes them, and refuses what
    /// comes after them if they aren't. Identifiers are in `id_bits`, as the
    /// embedded file table has them in this one's width too, so it may need
    /// more than these pages do (F.4.7). A numerator's denominator is
    /// `denominator`.
    #[must_use]
    pub fn of(first_page_at: u32, pages: Vec<PageEntry>, id_bits: u16, denominator: u16) -> Self {
        let span = |of: &dyn Fn(&PageEntry) -> u64| {
            let values = pages.iter().map(of);
            let (least, most) = (values.clone().min(), values.max());
            (least.unwrap_or(0), most.unwrap_or(0))
        };
        let narrow = |(least, most): (u64, u64)| width_of(most - least);
        let objects = span(&|page| page.objects);
        let length = span(&|page| page.length);
        let content_at = span(&|page| page.content_at);
        let content_length = span(&|page| page.content_length);
        let refs = || pages.iter().flat_map(|page| &page.shared);
        let numerator = refs().map(|r| u64::from(r.1)).max().unwrap_or(0);
        let count = pages.iter().map(|page| page.shared.len()).max();
        let length_bits = narrow(length).max(1);
        let header = PageHeader {
            min_objects: objects.0 as u32,
            first_page_at,
            objects_bits: narrow(objects).max(1),
            min_length: length.0 as u32,
            length_bits,
            min_content_at: content_at.0 as u32,
            content_at_bits: narrow(content_at),
            min_content_length: content_length.0 as u32,
            content_length_bits: narrow(content_length).max(length_bits),
            count_bits: width_of(count.unwrap_or(0) as u64).max(1),
            id_bits: id_bits.max(1),
            numerator_bits: width_of(numerator),
            denominator,
        };
        PageOffsets { header, pages }
    }

    /// The table, for `pages` pages, from the start of `data`.
    ///
    /// # Errors
    ///
    /// More pages than `limits` lets it have ([`Error::TooManyPages`]), data
    /// that ends before the table ([`Error::PastEnd`]), a width past 32
    /// ([`Error::Width`]), or more entries than `limits` lets the tables hold
    /// ([`Error::TooManyEntries`]).
    pub fn read(data: &[u8], pages: u32, limits: &Limits) -> Result<PageOffsets, Error> {
        PageOffsets::read_bits(&mut Reader::new(data, limits), pages, limits)
    }

    fn read_bits(bits: &mut Reader, pages: u32, limits: &Limits) -> Result<PageOffsets, Error> {
        if u64::from(pages) > limits.pages as u64 {
            return Err(Error::TooManyPages {
                pages: u64::from(pages),
                limit: limits.pages,
            });
        }
        let header = PageHeader {
            min_objects: bits.bits32("the fewest objects a page has")?,
            first_page_at: bits.bits32("the first page's location")?,
            objects_bits: bits.width("a page's number of objects")?,
            min_length: bits.bits32("the shortest page")?,
            length_bits: bits.width("a page's length")?,
            min_content_at: bits.bits32("the nearest a content stream is")?,
            content_at_bits: bits.width("a page's content offset")?,
            min_content_length: bits.bits32("the shortest content")?,
            content_length_bits: bits.width("a page's content length")?,
            count_bits: bits.width("a page's count of shared objects")?,
            id_bits: bits.width("a shared object identifier")?,
            numerator_bits: bits.width("a shared object numerator")?,
            denominator: bits.bits16("the shared object denominator")?,
        };
        let count = u64::from(pages);
        let widths = [
            header.objects_bits,
            header.length_bits,
            header.count_bits,
            header.content_at_bits,
            header.content_length_bits,
        ];
        let each = widths.iter().map(|w| u64::from(*w)).sum();
        bits.room(count, each, "the page offset table's entries")?;
        bits.charge(count)?;

        let mut entries = vec![PageEntry::default(); pages as usize];
        let what = "a page's number of objects";
        for entry in &mut entries {
            let delta = bits.field(header.objects_bits, what)?;
            entry.objects = u64::from(header.min_objects) + u64::from(delta);
        }
        bits.align();
        for entry in &mut entries {
            let delta = bits.field(header.length_bits, "a page's length")?;
            entry.length = u64::from(header.min_length) + u64::from(delta);
        }
        bits.align();
        let mut shared = 0u64;
        let mut counts = Vec::with_capacity(pages as usize);
        for _ in 0..pages {
            let count = bits.field(header.count_bits, "a page's count of shared objects")?;
            shared += u64::from(count);
            counts.push(count);
        }
        bits.align();
        let each = u64::from(header.id_bits) + u64::from(header.numerator_bits);
        bits.room(shared, each, "the page offset table's shared objects")?;
        bits.charge(shared)?;
        for (entry, &count) in entries.iter_mut().zip(&counts) {
            entry.shared.reserve_exact(count as usize);
            for _ in 0..count {
                let id = bits.field(header.id_bits, "a shared object identifier")?;
                entry.shared.push((id, 0));
            }
        }
        bits.align();
        for entry in &mut entries {
            for reference in &mut entry.shared {
                reference.1 = bits.field(header.numerator_bits, "a shared object numerator")?;
            }
        }
        bits.align();
        for entry in &mut entries {
            let delta = bits.field(header.content_at_bits, "a page's content offset")?;
            entry.content_at = u64::from(header.min_content_at) + u64::from(delta);
        }
        bits.align();
        for entry in &mut entries {
            let delta = bits.field(header.content_length_bits, "a page's content length")?;
            entry.content_length = u64::from(header.min_content_length) + u64::from(delta);
        }
        bits.align();
        Ok(PageOffsets {
            header,
            pages: entries,
        })
    }

    /// The table, added to `out`, from the byte boundary it's at.
    ///
    /// # Errors
    ///
    /// A value its field's width can't hold, or a width past 32
    /// ([`Error::Value`]).
    pub fn write(&self, out: &mut Vec<u8>) -> Result<(), Error> {
        let header = &self.header;
        let mut bits = Writer::new(out);
        bits.bits32(header.min_objects)?;
        bits.bits32(header.first_page_at)?;
        bits.bits16(header.objects_bits)?;
        bits.bits32(header.min_length)?;
        bits.bits16(header.length_bits)?;
        bits.bits32(header.min_content_at)?;
        bits.bits16(header.content_at_bits)?;
        bits.bits32(header.min_content_length)?;
        bits.bits16(header.content_length_bits)?;
        bits.bits16(header.count_bits)?;
        bits.bits16(header.id_bits)?;
        bits.bits16(header.numerator_bits)?;
        bits.bits16(header.denominator)?;
        let pages = || self.pages.iter();
        let what = "a page's number of objects";
        let objects = pages().map(|page| page.objects);
        bits.less_column(objects, header.min_objects, header.objects_bits, what)?;
        let lengths = pages().map(|page| page.length);
        let what = "a page's length";
        bits.less_column(lengths, header.min_length, header.length_bits, what)?;
        let counts = pages().map(|page| page.shared.len() as u64);
        let what = "a page's count of shared objects";
        bits.column(counts, header.count_bits, what)?;
        let refs = || pages().flat_map(|page| &page.shared);
        let ids = refs().map(|r| u64::from(r.0));
        bits.column(ids, header.id_bits, "a shared object identifier")?;
        let numerators = refs().map(|r| u64::from(r.1));
        bits.column(
            numerators,
            header.numerator_bits,
            "a shared object numerator",
        )?;
        let what = "a page's content offset";
        let at = pages().map(|page| page.content_at);
        bits.less_column(at, header.min_content_at, header.content_at_bits, what)?;
        let what = "a page's content length";
        let length = pages().map(|page| page.content_length);
        let least = header.min_content_length;
        bits.less_column(length, least, header.content_length_bits, what)?;
        bits.finish()
    }
}

/// The shared object hint table's header (F.4.3, Table F.5).
/// Exhaustive, as the standard's table is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SharedHeader {
    /// Part 8's first object's number.
    pub first_object: u32,
    /// Where it is, as [`position`] has it.
    pub first_at: u32,
    /// How many entries are the first page's.
    pub first_page_entries: u32,
    /// How many entries there are in all.
    pub entries: u32,
    /// The width of an entry's count of objects, less one, which is the width
    /// of the greatest count, not of the greatest count less one (Table F.5).
    pub objects_bits: u16,
    /// The shortest entry's length.
    pub min_length: u32,
    /// The width of an entry's length, less the least.
    pub length_bits: u16,
}

/// One entry of the shared object table (F.4.3, Table F.6).
/// Exhaustive, as the standard's table is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GroupEntry {
    /// The length of its objects.
    pub length: u64,
    /// The signature, if it says it has one, as read: never computed here.
    pub signature: Option<[u8; 16]>,
    /// How many objects it has.
    pub objects: u64,
}

/// The shared object hint table (F.4.3).
/// Exhaustive, as the standard's table is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SharedObjects {
    /// Its header.
    pub header: SharedHeader,
    /// An entry per group of objects, the first page's first.
    pub groups: Vec<GroupEntry>,
}

impl SharedObjects {
    /// The table of `groups`, the first `first_page_entries` of them the first
    /// page's, with part 8 starting at object `first_object` at `first_at`, and
    /// a header as narrow as they let it be, but for a bit at least for a
    /// length, as PDFium's reader refuses a width of 0 there as it does in
    /// [`PageOffsets::of`].
    #[must_use]
    pub fn of(
        first_object: u32,
        first_at: u32,
        first_page_entries: u32,
        groups: Vec<GroupEntry>,
    ) -> SharedObjects {
        let lengths = groups.iter().map(|group| group.length);
        let (least, most) = (lengths.clone().min().unwrap_or(0), lengths.max());
        let most_objects = groups.iter().map(|group| group.objects).max();
        let header = SharedHeader {
            first_object,
            first_at,
            first_page_entries,
            entries: groups.len() as u32,
            objects_bits: width_of(most_objects.unwrap_or(0)),
            min_length: least as u32,
            length_bits: width_of(most.unwrap_or(0) - least).max(1),
        };
        SharedObjects { header, groups }
    }

    /// The table, from the start of `data`.
    ///
    /// # Errors
    ///
    /// Data that ends before the table ([`Error::PastEnd`]), a width past 32
    /// ([`Error::Width`]), or more entries than `limits` lets the tables hold
    /// ([`Error::TooManyEntries`]).
    pub fn read(data: &[u8], limits: &Limits) -> Result<SharedObjects, Error> {
        SharedObjects::read_bits(&mut Reader::new(data, limits))
    }

    fn read_bits(bits: &mut Reader) -> Result<SharedObjects, Error> {
        let header = SharedHeader {
            first_object: bits.bits32("the first shared object's number")?,
            first_at: bits.bits32("the first shared object's location")?,
            first_page_entries: bits.bits32("the first page's count of shared entries")?,
            entries: bits.bits32("the count of shared entries")?,
            objects_bits: bits.width("a shared entry's count of objects")?,
            min_length: bits.bits32("the shortest shared entry")?,
            length_bits: bits.width("a shared entry's length")?,
        };
        let count = u64::from(header.entries);
        // Its flag is a bit of its own, so this can't be nothing.
        let each = u64::from(header.length_bits) + u64::from(header.objects_bits) + 1;
        bits.room(count, each, "the shared object table's entries")?;
        bits.charge(count)?;
        let mut groups = vec![GroupEntry::default(); header.entries as usize];
        for group in &mut groups {
            let delta = bits.field(header.length_bits, "a shared entry's length")?;
            group.length = u64::from(header.min_length) + u64::from(delta);
        }
        bits.align();
        let mut signed = vec![false; groups.len()];
        for flag in &mut signed {
            *flag = bits.field(1, "a shared entry's signature flag")? != 0;
        }
        bits.align();
        for (group, _) in groups.iter_mut().zip(&signed).filter(|(_, flag)| **flag) {
            let mut signature = [0; 16];
            for byte in &mut signature {
                *byte = bits.field(8, "a shared entry's signature")? as u8;
            }
            group.signature = Some(signature);
        }
        for group in &mut groups {
            let less = bits.field(header.objects_bits, "a shared entry's count of objects")?;
            group.objects = u64::from(less) + 1;
        }
        bits.align();
        Ok(SharedObjects { header, groups })
    }

    /// The table, added to `out`, from the byte boundary it's at.
    ///
    /// # Errors
    ///
    /// A value its field's width can't hold, or a width past 32
    /// ([`Error::Value`]).
    pub fn write(&self, out: &mut Vec<u8>) -> Result<(), Error> {
        let header = &self.header;
        let mut bits = Writer::new(out);
        bits.bits32(header.first_object)?;
        bits.bits32(header.first_at)?;
        bits.bits32(header.first_page_entries)?;
        bits.bits32(header.entries)?;
        bits.bits16(header.objects_bits)?;
        bits.bits32(header.min_length)?;
        bits.bits16(header.length_bits)?;
        let groups = || self.groups.iter();
        let lengths = groups().map(|group| group.length);
        let what = "a shared entry's length";
        bits.less_column(lengths, header.min_length, header.length_bits, what)?;
        let flags = groups().map(|group| u64::from(group.signature.is_some()));
        bits.column(flags, 1, "a shared entry's signature flag")?;
        let bytes = groups().flat_map(|group| group.signature.iter().flatten());
        bits.column(bytes.map(|b| u64::from(*b)), 8, "a signature")?;
        let what = "a shared entry's count of objects";
        let objects = groups().map(|group| group.objects);
        bits.less_column(objects, 1, header.objects_bits, what)?;
        bits.finish()
    }
}

/// A generic hint table (F.4.5, Table F.9): one group of objects together in
/// the file. With no objects, every field is 0.
/// Exhaustive, as the standard's table is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Generic {
    /// The number of its first object.
    pub first_object: u32,
    /// Where that is, as [`position`] has it.
    pub first_at: u32,
    /// How many objects it has.
    pub objects: u32,
    /// Their length in bytes.
    pub length: u32,
}

impl Generic {
    /// The table, from the start of `data`.
    ///
    /// # Errors
    ///
    /// Data that ends before the table ([`Error::PastEnd`]), a width past 32
    /// ([`Error::Width`]), or more entries than `limits` lets the tables hold
    /// ([`Error::TooManyEntries`]).
    pub fn read(data: &[u8], limits: &Limits) -> Result<Generic, Error> {
        Generic::read_bits(&mut Reader::new(data, limits))
    }

    fn read_bits(bits: &mut Reader) -> Result<Generic, Error> {
        Ok(Generic {
            first_object: bits.bits32("a generic table's first object")?,
            first_at: bits.bits32("a generic table's location")?,
            objects: bits.bits32("a generic table's count of objects")?,
            length: bits.bits32("a generic table's length")?,
        })
    }

    /// The table, added to `out`, from the byte boundary it's at.
    ///
    /// # Errors
    ///
    /// A value its field's width can't hold, or a width past 32
    /// ([`Error::Value`]).
    pub fn write(&self, out: &mut Vec<u8>) -> Result<(), Error> {
        let mut bits = Writer::new(out);
        self.write_bits(&mut bits)?;
        bits.finish()
    }

    fn write_bits(&self, bits: &mut Writer) -> Result<(), Error> {
        bits.bits32(self.first_object)?;
        bits.bits32(self.first_at)?;
        bits.bits32(self.objects)?;
        bits.bits32(self.length)
    }
}

/// An extended generic hint table (F.4.6, Table F.10): a generic one, and the
/// shared objects its group refers to. Exhaustive, as the standard's table is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Extended {
    /// What a generic table says.
    pub generic: Generic,
    /// The width of each identifier.
    pub id_bits: u16,
    /// The entries of the shared object table the group refers to.
    pub shared: Vec<u32>,
}

impl Extended {
    /// The table of `generic` and `shared`, as narrow as they let it be.
    #[must_use]
    pub fn of(generic: Generic, shared: Vec<u32>) -> Extended {
        let most = shared.iter().copied().max().unwrap_or(0);
        Extended {
            generic,
            id_bits: width_of(u64::from(most)),
            shared,
        }
    }

    /// The table, from the start of `data`.
    ///
    /// # Errors
    ///
    /// Data that ends before the table ([`Error::PastEnd`]), a width past 32
    /// ([`Error::Width`]), or more entries than `limits` lets the tables hold
    /// ([`Error::TooManyEntries`]).
    pub fn read(data: &[u8], limits: &Limits) -> Result<Extended, Error> {
        Extended::read_bits(&mut Reader::new(data, limits))
    }

    fn read_bits(bits: &mut Reader) -> Result<Extended, Error> {
        let generic = Generic::read_bits(bits)?;
        let count = bits.bits32("an extended table's count of shared objects")?;
        let id_bits = bits.width("an extended table's identifier")?;
        let each = u64::from(id_bits);
        bits.room(u64::from(count), each, "an extended table's identifiers")?;
        bits.charge(u64::from(count))?;
        let mut shared = Vec::with_capacity(count as usize);
        for _ in 0..count {
            shared.push(bits.field(id_bits, "an extended table's identifier")?);
        }
        bits.align();
        Ok(Extended {
            generic,
            id_bits,
            shared,
        })
    }

    /// The table, added to `out`, from the byte boundary it's at.
    ///
    /// # Errors
    ///
    /// A value its field's width can't hold, or a width past 32
    /// ([`Error::Value`]).
    pub fn write(&self, out: &mut Vec<u8>) -> Result<(), Error> {
        let mut bits = Writer::new(out);
        self.generic.write_bits(&mut bits)?;
        bits.bits32(self.shared.len() as u32)?;
        bits.bits16(self.id_bits)?;
        let ids = self.shared.iter().map(|id| u64::from(*id));
        bits.column(ids, self.id_bits, "an extended table's identifier")?;
        bits.finish()
    }
}

/// The thumbnail hint table's header (F.4.4, Table F.7).
/// Exhaustive, as the standard's table is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ThumbHeader {
    /// The number of the first thumbnail's first object.
    pub first_object: u32,
    /// Where that is, as [`position`] has it.
    pub first_at: u32,
    /// How many pages have a thumbnail.
    pub pages: u32,
    /// The width of an entry's count of pages before it without one.
    pub gap_bits: u16,
    /// The shortest thumbnail's length.
    pub min_length: u32,
    /// The width of an entry's length, less the least.
    pub length_bits: u16,
    /// The fewest objects any thumbnail has.
    pub min_objects: u32,
    /// The width of an entry's number of objects, less the least.
    pub objects_bits: u16,
    /// The number of the first of the objects thumbnails share.
    pub shared_first_object: u32,
    /// Where that is, as [`position`] has it.
    pub shared_first_at: u32,
    /// How many objects thumbnails share.
    pub shared_objects: u32,
    /// Their length in bytes.
    pub shared_length: u32,
}

/// One thumbnail of the thumbnail hint table (F.4.4, Table F.8).
/// Exhaustive, as the standard's table is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ThumbEntry {
    /// How many pages since the last entry's page have none.
    pub gap: u32,
    /// How many objects it has.
    pub objects: u64,
    /// Their length in bytes.
    pub length: u64,
}

/// The thumbnail hint table (F.4.4). Exhaustive, as the standard's table is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Thumbnails {
    /// Its header, the objects thumbnails share in it.
    pub header: ThumbHeader,
    /// An entry per page that has a thumbnail, in page order from page 0.
    pub pages: Vec<ThumbEntry>,
}

impl Thumbnails {
    /// The table of `pages`, the first at object `first_object` at `first_at`,
    /// and what thumbnails share as given in `shared`'s fields; a header as
    /// narrow as they let it be.
    #[must_use]
    pub fn of(
        first_object: u32,
        first_at: u32,
        pages: Vec<ThumbEntry>,
        shared: Generic,
    ) -> Thumbnails {
        let span = |of: &dyn Fn(&ThumbEntry) -> u64| {
            let values = pages.iter().map(of);
            (values.clone().min().unwrap_or(0), values.max().unwrap_or(0))
        };
        let length = span(&|page| page.length);
        let objects = span(&|page| page.objects);
        let gap = pages.iter().map(|page| page.gap).max().unwrap_or(0);
        let header = ThumbHeader {
            first_object,
            first_at,
            pages: pages.len() as u32,
            gap_bits: width_of(u64::from(gap)),
            min_length: length.0 as u32,
            length_bits: width_of(length.1 - length.0),
            min_objects: objects.0 as u32,
            objects_bits: width_of(objects.1 - objects.0),
            shared_first_object: shared.first_object,
            shared_first_at: shared.first_at,
            shared_objects: shared.objects,
            shared_length: shared.length,
        };
        Thumbnails { header, pages }
    }

    /// The table, from the start of `data`.
    ///
    /// # Errors
    ///
    /// Data that ends before the table ([`Error::PastEnd`]), a width past 32
    /// ([`Error::Width`]), or more entries than `limits` lets the tables hold
    /// ([`Error::TooManyEntries`]).
    pub fn read(data: &[u8], limits: &Limits) -> Result<Thumbnails, Error> {
        Thumbnails::read_bits(&mut Reader::new(data, limits))
    }

    fn read_bits(bits: &mut Reader) -> Result<Thumbnails, Error> {
        let header = ThumbHeader {
            first_object: bits.bits32("the first thumbnail's number")?,
            first_at: bits.bits32("the first thumbnail's location")?,
            pages: bits.bits32("the count of thumbnails")?,
            gap_bits: bits.width("a thumbnail's gap")?,
            min_length: bits.bits32("the shortest thumbnail")?,
            length_bits: bits.width("a thumbnail's length")?,
            min_objects: bits.bits32("the fewest objects a thumbnail has")?,
            objects_bits: bits.width("a thumbnail's number of objects")?,
            shared_first_object: bits.bits32("the first of what thumbnails share")?,
            shared_first_at: bits.bits32("where what thumbnails share is")?,
            shared_objects: bits.bits32("the count of what thumbnails share")?,
            shared_length: bits.bits32("the length of what thumbnails share")?,
        };
        let count = u64::from(header.pages);
        let each = u64::from(header.gap_bits)
            + u64::from(header.objects_bits)
            + u64::from(header.length_bits);
        bits.room(count, each, "the thumbnail table's entries")?;
        bits.charge(count)?;
        let mut pages = vec![ThumbEntry::default(); header.pages as usize];
        for page in &mut pages {
            page.gap = bits.field(header.gap_bits, "a thumbnail's gap")?;
        }
        bits.align();
        for page in &mut pages {
            let delta = bits.field(header.objects_bits, "a thumbnail's number of objects")?;
            page.objects = u64::from(header.min_objects) + u64::from(delta);
        }
        bits.align();
        for page in &mut pages {
            let delta = bits.field(header.length_bits, "a thumbnail's length")?;
            page.length = u64::from(header.min_length) + u64::from(delta);
        }
        bits.align();
        Ok(Thumbnails { header, pages })
    }

    /// The table, added to `out`, from the byte boundary it's at.
    ///
    /// # Errors
    ///
    /// A value its field's width can't hold, or a width past 32
    /// ([`Error::Value`]).
    pub fn write(&self, out: &mut Vec<u8>) -> Result<(), Error> {
        let header = &self.header;
        let mut bits = Writer::new(out);
        bits.bits32(header.first_object)?;
        bits.bits32(header.first_at)?;
        bits.bits32(header.pages)?;
        bits.bits16(header.gap_bits)?;
        bits.bits32(header.min_length)?;
        bits.bits16(header.length_bits)?;
        bits.bits32(header.min_objects)?;
        bits.bits16(header.objects_bits)?;
        bits.bits32(header.shared_first_object)?;
        bits.bits32(header.shared_first_at)?;
        bits.bits32(header.shared_objects)?;
        bits.bits32(header.shared_length)?;
        let pages = || self.pages.iter();
        let gaps = pages().map(|page| u64::from(page.gap));
        bits.column(gaps, header.gap_bits, "a thumbnail's gap")?;
        let what = "a thumbnail's number of objects";
        let objects = pages().map(|page| page.objects);
        bits.less_column(objects, header.min_objects, header.objects_bits, what)?;
        let lengths = pages().map(|page| page.length);
        let what = "a thumbnail's length";
        bits.less_column(lengths, header.min_length, header.length_bits, what)?;
        bits.finish()
    }
}

/// The embedded file table's header (F.4.7, Table F.11).
/// Exhaustive, as the standard's table is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileHeader {
    /// The number of the first group's first object.
    pub first_object: u32,
    /// Where that is, as [`position`] has it.
    pub first_at: u32,
    /// How many groups there are.
    pub groups: u32,
    /// The width of an entry's object number.
    pub number_bits: u16,
    /// The width of an entry's number of objects.
    pub objects_bits: u16,
    /// The width of an entry's length.
    pub length_bits: u16,
    /// The width of an entry's count of shared objects.
    pub count_bits: u16,
}

/// One group of the embedded file table (F.4.7, Table F.12).
/// Exhaustive, as the standard's table is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileEntry {
    /// The number of the stream it's for.
    pub number: u32,
    /// How many objects it has, which is 0 where it only refers to others.
    pub objects: u32,
    /// Their length in bytes, likewise.
    pub length: u32,
    /// The entries of the shared object table it refers to.
    pub shared: Vec<u32>,
}

/// The embedded file table (F.4.7). Exhaustive, as the standard's table is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EmbeddedFiles {
    /// Its header.
    pub header: FileHeader,
    /// An entry per group.
    pub groups: Vec<FileEntry>,
}

impl EmbeddedFiles {
    /// The table of `groups`, the first's first object at `first_object` at
    /// `first_at`, with a header as narrow as they let it be.
    #[must_use]
    pub fn of(first_object: u32, first_at: u32, groups: Vec<FileEntry>) -> EmbeddedFiles {
        let most = |of: &dyn Fn(&FileEntry) -> u64| groups.iter().map(of).max().unwrap_or(0);
        let header = FileHeader {
            first_object,
            first_at,
            groups: groups.len() as u32,
            number_bits: width_of(most(&|group| u64::from(group.number))),
            objects_bits: width_of(most(&|group| u64::from(group.objects))),
            length_bits: width_of(most(&|group| u64::from(group.length))),
            count_bits: width_of(most(&|group| group.shared.len() as u64)),
        };
        EmbeddedFiles { header, groups }
    }

    /// The table, from the start of `data`, its identifiers in the width the
    /// page offset table's header gives them (F.4.7, Table F.12), `id_bits`.
    ///
    /// # Errors
    ///
    /// Data that ends before the table ([`Error::PastEnd`]), a width past 32
    /// ([`Error::Width`]), or more entries than `limits` lets the tables hold
    /// ([`Error::TooManyEntries`]).
    pub fn read(data: &[u8], id_bits: u16, limits: &Limits) -> Result<EmbeddedFiles, Error> {
        EmbeddedFiles::read_bits(&mut Reader::new(data, limits), id_bits)
    }

    fn read_bits(bits: &mut Reader, id_bits: u16) -> Result<EmbeddedFiles, Error> {
        let header = FileHeader {
            first_object: bits.bits32("the first embedded file's number")?,
            first_at: bits.bits32("the first embedded file's location")?,
            groups: bits.bits32("the count of embedded file groups")?,
            number_bits: bits.width("an embedded file's number")?,
            objects_bits: bits.width("an embedded file group's number of objects")?,
            length_bits: bits.width("an embedded file group's length")?,
            count_bits: bits.width("an embedded file group's count of shared objects")?,
        };
        let count = u64::from(header.groups);
        let each = u64::from(header.number_bits)
            + u64::from(header.objects_bits)
            + u64::from(header.length_bits)
            + u64::from(header.count_bits);
        bits.room(count, each, "the embedded file table's entries")?;
        bits.charge(count)?;
        let mut groups = vec![FileEntry::default(); header.groups as usize];
        for group in &mut groups {
            group.number = bits.field(header.number_bits, "an embedded file's number")?;
        }
        bits.align();
        for group in &mut groups {
            let what = "an embedded file group's number of objects";
            group.objects = bits.field(header.objects_bits, what)?;
        }
        bits.align();
        for group in &mut groups {
            group.length = bits.field(header.length_bits, "an embedded file group's length")?;
        }
        bits.align();
        let what = "an embedded file group's count of shared objects";
        let mut counts = Vec::with_capacity(groups.len());
        let mut shared = 0u64;
        for _ in 0..header.groups {
            let count = bits.field(header.count_bits, what)?;
            shared += u64::from(count);
            counts.push(count);
        }
        bits.align();
        bits.check_width(id_bits, "an embedded file's identifier")?;
        bits.room(
            shared,
            u64::from(id_bits),
            "the embedded file table's identifiers",
        )?;
        bits.charge(shared)?;
        for (group, &count) in groups.iter_mut().zip(&counts) {
            group.shared.reserve_exact(count as usize);
            for _ in 0..count {
                let id = bits.field(id_bits, "an embedded file's identifier")?;
                group.shared.push(id);
            }
        }
        bits.align();
        Ok(EmbeddedFiles { header, groups })
    }

    /// The table, added to `out`, from the byte boundary it's at, its
    /// identifiers in `id_bits`, the width the page offset table has them in.
    ///
    /// # Errors
    ///
    /// A value its field's width can't hold, or a width past 32
    /// ([`Error::Value`]).
    pub fn write(&self, out: &mut Vec<u8>, id_bits: u16) -> Result<(), Error> {
        let header = &self.header;
        let mut bits = Writer::new(out);
        bits.bits32(header.first_object)?;
        bits.bits32(header.first_at)?;
        bits.bits32(header.groups)?;
        bits.bits16(header.number_bits)?;
        bits.bits16(header.objects_bits)?;
        bits.bits16(header.length_bits)?;
        bits.bits16(header.count_bits)?;
        let groups = || self.groups.iter();
        let numbers = groups().map(|group| u64::from(group.number));
        bits.column(numbers, header.number_bits, "an embedded file's number")?;
        let what = "an embedded file group's number of objects";
        let objects = groups().map(|group| u64::from(group.objects));
        bits.column(objects, header.objects_bits, what)?;
        let lengths = groups().map(|group| u64::from(group.length));
        bits.column(
            lengths,
            header.length_bits,
            "an embedded file group's length",
        )?;
        let what = "an embedded file group's count of shared objects";
        let counts = groups().map(|group| group.shared.len() as u64);
        bits.column(counts, header.count_bits, what)?;
        let ids = groups()
            .flat_map(|group| &group.shared)
            .map(|id| u64::from(*id));
        bits.column(ids, id_bits, "an embedded file's identifier")?;
        bits.finish()
    }
}

/// Where each table starts in a hint stream's data, as the keys of its
/// dictionary say (F.3.6, Table F.2), as they're read: a position that doesn't
/// fit is for [`tables`] to refuse. Keys that aren't here, a private table's,
/// say, aren't read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct HintKeys {
    /// `/S`: the shared object table.
    pub shared: u64,
    /// `/T`: the thumbnail table.
    pub thumbnails: Option<u64>,
    /// `/O`: the outline table.
    pub outlines: Option<u64>,
    /// `/A`: the thread information table.
    pub threads: Option<u64>,
    /// `/E`: the named destination table.
    pub dests: Option<u64>,
    /// `/V`: the interactive form table.
    pub form: Option<u64>,
    /// `/I`: the information dictionary table.
    pub info: Option<u64>,
    /// `/C`: the logical structure table.
    pub structure: Option<u64>,
    /// `/L`: the page label table.
    pub labels: Option<u64>,
    /// `/R`: the renditions name tree table.
    pub renditions: Option<u64>,
    /// `/B`: the embedded file stream table.
    pub files: Option<u64>,
}

impl HintKeys {
    /// The keys of a stream that says only where its shared object table is, at
    /// `shared`.
    #[must_use]
    pub const fn new(shared: u64) -> HintKeys {
        HintKeys {
            shared,
            thumbnails: None,
            outlines: None,
            threads: None,
            dests: None,
            form: None,
            info: None,
            structure: None,
            labels: None,
            renditions: None,
            files: None,
        }
    }
}

/// Every table a hint stream's dictionary says it has, read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Tables {
    /// The page offset table, at the start of the data (F.3.6).
    pub pages: PageOffsets,
    /// The shared object table.
    pub shared: SharedObjects,
    /// The thumbnail table.
    pub thumbnails: Option<Thumbnails>,
    /// The outline table.
    pub outlines: Option<Generic>,
    /// The thread information table.
    pub threads: Option<Generic>,
    /// The named destination table.
    pub dests: Option<Generic>,
    /// The interactive form table.
    pub form: Option<Extended>,
    /// The information dictionary table.
    pub info: Option<Generic>,
    /// The logical structure table.
    pub structure: Option<Extended>,
    /// The page label table.
    pub labels: Option<Generic>,
    /// The renditions name tree table.
    pub renditions: Option<Extended>,
    /// The embedded file stream table.
    pub files: Option<EmbeddedFiles>,
}

/// Every table of a hint stream's decoded `data`, primary and any overflow
/// joined, for a file of `pages` pages, where `keys` says each is, under
/// `limits`: all of it charged to one count of entries.
///
/// # Errors
///
/// A table whose key puts it past the data ([`Error::TableAt`]), and each
/// table's own: more pages than `limits` lets the file have
/// ([`Error::TooManyPages`]), data that ends before a table
/// ([`Error::PastEnd`]), a width past 32 ([`Error::Width`]), or more entries
/// than `limits` lets the tables hold ([`Error::TooManyEntries`]).
pub fn tables(data: &[u8], keys: &HintKeys, pages: u32, limits: &Limits) -> Result<Tables, Error> {
    let mut at = At {
        data,
        limits,
        charged: 0,
    };
    let pages = at.read(Table::PageOffsets, 0, |bits| {
        PageOffsets::read_bits(bits, pages, limits)
    })?;
    let id_bits = pages.header.id_bits;
    Ok(Tables {
        shared: at.read(Table::Shared, keys.shared, SharedObjects::read_bits)?,
        thumbnails: at.optional(Table::Thumbnails, keys.thumbnails, Thumbnails::read_bits)?,
        outlines: at.optional(Table::Outlines, keys.outlines, Generic::read_bits)?,
        threads: at.optional(Table::Threads, keys.threads, Generic::read_bits)?,
        dests: at.optional(Table::Dests, keys.dests, Generic::read_bits)?,
        form: at.optional(Table::Form, keys.form, Extended::read_bits)?,
        info: at.optional(Table::Info, keys.info, Generic::read_bits)?,
        structure: at.optional(Table::Structure, keys.structure, Extended::read_bits)?,
        labels: at.optional(Table::Labels, keys.labels, Generic::read_bits)?,
        renditions: at.optional(Table::Renditions, keys.renditions, Extended::read_bits)?,
        files: at.optional(Table::Files, keys.files, |bits| {
            EmbeddedFiles::read_bits(bits, id_bits)
        })?,
        pages,
    })
}

/// A hint stream's data, each table read from where its key says, charged to
/// one count of entries.
struct At<'a> {
    data: &'a [u8],
    limits: &'a Limits,
    charged: u64,
}

impl<'a> At<'a> {
    /// The table `read` reads, at byte `key`.
    fn read<T>(
        &mut self,
        table: Table,
        key: u64,
        read: impl FnOnce(&mut Reader<'a>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let len = self.data.len() as u64;
        if key >= len {
            return Err(Error::TableAt {
                table,
                at: key,
                len,
            });
        }
        let mut bits = Reader::new(&self.data[key as usize..], self.limits);
        bits.charged_already(self.charged);
        let read = read(&mut bits)?;
        self.charged = bits.charged();
        Ok(read)
    }

    /// The table `read` reads, if there's a `key`.
    fn optional<T>(
        &mut self,
        table: Table,
        key: Option<u64>,
        read: impl FnOnce(&mut Reader<'a>) -> Result<T, Error>,
    ) -> Result<Option<T>, Error> {
        key.map(|key| self.read(table, key, read)).transpose()
    }
}
