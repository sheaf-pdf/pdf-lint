//! What [`check`](super::check) tells: each one thing wrong with a file, by the
//! clause it departs from.

use super::{Attribute, Unread};
use crate::{hint, plan};

/// How much a [`Finding`] matters, for a caller to choose by. Ordered as
/// listed, the most first; nothing in the crate compares grades by order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Grade {
    /// What a reader relying on the file gets wrong: a position, a length, a
    /// count, an identifier past its table, a value it can't read. The file
    /// doesn't hold.
    Error,
    /// Not as the standard has it, but every reader goes on, and real writers
    /// do it.
    Departure,
    /// Not judged: a part of the file the reader didn't read, for a reason of
    /// its own ([`Unread`]), or past a cap the check reads under
    /// ([`Limits`](crate::Limits)). It says nothing of the file, and never
    /// makes it not hold.
    Unjudged,
}

/// A key of the linearization dictionary (Table F.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DictKey {
    /// `/Linearized`.
    Linearized,
    /// `/L`.
    L,
    /// `/H`.
    H,
    /// `/O`.
    O,
    /// `/E`.
    E,
    /// `/N`.
    N,
    /// `/T`.
    T,
    /// `/P`.
    P,
}

impl std::fmt::Display for DictKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DictKey::Linearized => "/Linearized",
            DictKey::L => "/L",
            DictKey::H => "/H",
            DictKey::O => "/O",
            DictKey::E => "/E",
            DictKey::N => "/N",
            DictKey::T => "/T",
            DictKey::P => "/P",
        })
    }
}

/// How many bytes of a key a [`KeyName`] holds.
const HELD: usize = 15;

/// A key of a hint stream's dictionary, as a finding owning nothing can hold
/// it: its first bytes. Its `Display` is the key with its slash, `/S`, a byte
/// that's no character's U+FFFD, and a key longer than 15 bytes cut before
/// the character that runs past them, `…` where it's cut.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyName {
    bytes: [u8; HELD],
    len: u8,
    cut: bool,
}

impl KeyName {
    /// `key`, or its first bytes: up to 15, cut before a character that runs
    /// past them.
    #[must_use]
    pub const fn new(key: &[u8]) -> KeyName {
        let len = held(key);
        let mut bytes = [0; HELD];
        bytes
            .split_at_mut(len)
            .0
            .copy_from_slice(key.split_at(len).0);
        KeyName {
            bytes,
            len: len as u8,
            cut: len < key.len(),
        }
    }

    /// The bytes it holds: the key's, or as many of its first as it holds.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

/// How many of `key`'s bytes a [`KeyName`] holds: all of them up to
/// [`HELD`], else [`HELD`], or fewer where a character that starts before
/// byte [`HELD`] runs past it, which is left out whole. Where the bytes at
/// the cut are no character's, it's at [`HELD`], and they're each U+FFFD, as
/// everywhere else.
const fn held(key: &[u8]) -> usize {
    if key.len() <= HELD {
        return key.len();
    }
    // A byte past the cut that continues a character: back to where that
    // starts, three bytes at most.
    if !continues(key[HELD]) {
        return HELD;
    }
    let mut start = HELD - 1;
    while start > HELD - 4 && continues(key[start]) {
        start -= 1;
    }
    let width = match key[start] {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 0,
    };
    // The character it starts, where that's one, runs past the cut.
    let after = key.split_at(start).1;
    match start + width > HELD && width <= after.len() {
        true => match std::str::from_utf8(after.split_at(width).0) {
            Ok(_) => start,
            Err(_) => HELD,
        },
        false => HELD,
    }
}

/// Whether `byte` continues a character in UTF-8, as `10xxxxxx`.
const fn continues(byte: u8) -> bool {
    byte & 0xC0 == 0x80
}

impl std::fmt::Display for KeyName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "/{}", String::from_utf8_lossy(self.as_bytes()))?;
        match self.cut {
            true => f.write_str("…"),
            false => Ok(()),
        }
    }
}

impl std::fmt::Debug for KeyName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

/// Where a [`Finding`] is said of, for a caller that points at it: as near to
/// the bytes as the check knows them, which [`Finding::place`] gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Place {
    /// A byte of the file, from its first.
    Byte(u64),
    /// A position as a hint table has one, as if the hint stream weren't there,
    /// from the file's first byte (the table's offset past the header):
    /// [`hint::located`] gives the byte, under the hint stream's extent the
    /// check places it by, [`Facts::hint_extent`](super::Facts::hint_extent).
    Position(u64),
    /// A byte of the hint stream's data.
    HintByte(u64),
    /// A bit of the hint stream's data.
    HintBit(u64),
    /// An object, by its number, which the file's sections place.
    Object(u32),
    /// A page, by its place among the pages.
    Page(u64),
    /// An entry of the shared object table.
    SharedEntry(u64),
    /// A table of the hint stream, by its key in the stream's dictionary.
    Table(&'static str),
    /// A key of the linearization dictionary.
    Key(DictKey),
    /// A key of the hint stream's dictionary.
    HintKey(KeyName),
}

/// One thing wrong with a file, by the clause it departs from. It owns nothing,
/// so a sink keeps it as it is; its [`Display`](std::fmt::Display) is the whole
/// message, with the place, a byte or a page or an entry of a table, where
/// there's one, and [`Finding::place`] and [`Finding::clause`] give those
/// apart. Its positions are from the file's first byte, a hint table's offsets
/// taken past the header; a linearization dictionary's value it quotes is as
/// written, counted from the header (ISO 32000-2, 7.5.2). In a file whose
/// header is its first byte, the two are the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Finding {
    /// `/Linearized` isn't a number whose integer part is 1 (Table F.1), so the
    /// file isn't taken as linearized.
    #[error("/Linearized isn't a number whose integer part is 1 (ISO 32000-2, Table F.1)")]
    Version,
    /// `/L` isn't the file's length (Table F.1), so the file isn't taken as
    /// linearized.
    #[error("/L is {l}, where the file is {len} bytes from its header (ISO 32000-2, Table F.1)")]
    LengthNotFile {
        /// What it says.
        l: i64,
        /// What the file is.
        len: u64,
    },
    /// An update follows the file's linearized part (G.7), which is why `/L`
    /// isn't its length.
    #[error(
        "an update is appended, its section at byte {at}: the hints may no longer hold (ISO 32000-2, G.7)"
    )]
    Updated {
        /// Where its section is.
        at: u64,
    },
    /// A key's value is missing or of the wrong type (Table F.1).
    #[error("{key} is missing or isn't what it has to be (ISO 32000-2, Table F.1)")]
    BadValue {
        /// The key.
        key: DictKey,
    },
    /// A key's value is a reference, where they're all direct (F.3.3).
    #[error("{key} is a reference, and the dictionary's values are direct (ISO 32000-2, F.3.3)")]
    Indirect {
        /// The key.
        key: DictKey,
    },
    /// The dictionary starts within the first 1024 bytes and runs past them
    /// (F.3.3).
    #[error("the dictionary runs on to byte {end}, past the first 1024 bytes (ISO 32000-2, F.3.3)")]
    DictionaryPast1024 {
        /// Where it ends: more than 1024 bytes past the header.
        end: u64,
    },
    /// `/H` hasn't two or four items (Table F.1).
    #[error("/H has {items} items, where it has two or four (ISO 32000-2, Table F.1)")]
    HintItems {
        /// How many it has, or `u32::MAX` where it's more.
        items: u32,
    },
    /// `/O` isn't the number of the first page's object (Table F.1).
    #[error("/O is {o}, where page {page}'s object is {computed} (ISO 32000-2, Table F.1)")]
    FirstPageObject {
        /// What it says.
        o: i64,
        /// Which page is first.
        page: u64,
        /// What that page's object is.
        computed: u32,
    },
    /// `/N` isn't how many pages the page tree has (Table F.1).
    #[error("/N is {n}, where the page tree has {pages} pages (ISO 32000-2, Table F.1)")]
    PageCount {
        /// What it says.
        n: i64,
        /// How many there are.
        pages: u64,
    },
    /// `/E` isn't where the first page ends (Table F.1).
    #[error(
        "/E is {e}, where the first page's last object ends by {least} to {most} (ISO 32000-2, Table F.1)"
    )]
    FirstPageEnd {
        /// What it says, which counts from the header.
        e: i64,
        /// The first place it could be: past that object's `endobj`.
        least: u64,
        /// The last: where what follows starts.
        most: u64,
    },
    /// `/T` isn't where the main section's first row is (Table F.1).
    #[error(
        "/T is {t}, where the main section's first entry is at {least} to {most} (ISO 32000-2, Table F.1)"
    )]
    MainFirstEntry {
        /// What it says, which counts from the header.
        t: i64,
        /// The first place it could be: the white-space before the row.
        least: u64,
        /// The last.
        most: u64,
    },
    /// No section lists an object at `/H`'s first offset (Table F.1).
    #[error(
        "nothing's listed at /H's byte {at}, where the hint stream should be (ISO 32000-2, Table F.1)"
    )]
    HintNotListed {
        /// Where `/H` says it is.
        at: i64,
    },
    /// `/H`'s length isn't the hint stream's, overhead and all (Table F.1).
    #[error(
        "/H's length from byte {at} is {length}, where the hint stream takes {least} to {most} (ISO 32000-2, Table F.1)"
    )]
    HintExtent {
        /// Where it says the stream starts.
        at: u64,
        /// What it says.
        length: i64,
        /// The fewest bytes it takes: past its `endobj`.
        least: u64,
        /// The most: to where what follows starts.
        most: u64,
    },

    /// The reader stopped reading the sections short of the main one, for a
    /// reason of its own, so nothing's said of them, of the numbers they
    /// list, of a position they place, or of how many pages the page tree
    /// has or where one is in it, as it may be read without what's listed
    /// past where they stopped (F.3.4, F.3.11).
    #[error(
        "the sections aren't read as far as the main one, as {why}, so they and what they place aren't judged (ISO 32000-2, F.3.4, F.3.11)"
    )]
    SectionsUnread {
        /// Why.
        why: Unread,
    },
    /// `startxref` gives no section that reads, for what's wrong with it.
    #[error("the sections don't read from the last startxref (ISO 32000-2, 7.5.8.2, F.3.4)")]
    NoSections,
    /// The first section's `/Prev` gives no section that reads, for what's
    /// wrong with it (F.3.4).
    #[error("no main section reads from the first section's /Prev (ISO 32000-2, F.3.4)")]
    NoMainSection,
    /// The section `startxref` gives comes after the one its `/Prev` gives, as
    /// an update's would (F.3).
    #[error(
        "the first section, at byte {first}, is after the main one, at byte {main} (ISO 32000-2, F.3)"
    )]
    SectionOrder {
        /// Where the one `startxref` gives is.
        first: u64,
        /// Where the one its `/Prev` gives is.
        main: u64,
    },
    /// The first section has more than one subsection (F.3.4).
    #[error("the first section has {count} subsections, where it has one (ISO 32000-2, F.3.4)")]
    FirstSubsections {
        /// How many it has, or `u32::MAX` where it's more.
        count: u32,
    },
    /// The first section has a free entry (F.3.4).
    #[error("the first section lists {number} free (ISO 32000-2, F.3.4)")]
    FirstFree {
        /// The number.
        number: u32,
    },
    /// The first section's `/Size` isn't both sections' entries (F.3.4).
    #[error(
        "the first section's /Size is {size}, where the sections have {total} entries (ISO 32000-2, F.3.4)"
    )]
    FirstSize {
        /// What it says.
        size: i64,
        /// How many there are.
        total: u64,
    },
    /// The first section's trailer has no `/Root` that's a reference (F.3.4).
    #[error("the first section's trailer has no /Root that names an object (ISO 32000-2, F.3.4)")]
    FirstNoRoot,
    /// The first section's first row isn't the dictionary (F.3.4).
    #[error(
        "the first section's first row is for {number}, where the dictionary is {dictionary} (ISO 32000-2, F.3.4)"
    )]
    FirstNotDictionary {
        /// The number it's for.
        number: u32,
        /// The dictionary's.
        dictionary: u32,
    },
    /// The main section has more than one subsection (F.3.11).
    #[error("the main section has {count} subsections, where it has one (ISO 32000-2, F.3.11)")]
    MainSubsections {
        /// How many it has, or `u32::MAX` where it's more.
        count: u32,
    },
    /// The main section doesn't begin at 0 (F.3.11).
    #[error("the main section begins at {from}, where it begins at 0 (ISO 32000-2, F.3.11)")]
    MainFrom {
        /// Where it begins.
        from: u32,
    },
    /// The main section's entry for 0 isn't free (F.3.11).
    #[error("the main section's entry for object 0 isn't free (ISO 32000-2, F.3.11)")]
    MainZeroInUse,
    /// One of the main section's other entries isn't in use (F.3.11).
    #[error("the main section lists {number} as not in use (ISO 32000-2, F.3.11)")]
    MainFree {
        /// The number.
        number: u32,
    },
    /// The main section's trailer has a `/Prev` (F.3.11).
    #[error("the main section's trailer has a /Prev (ISO 32000-2, F.3.11)")]
    MainPrev,
    /// The last `startxref` doesn't give the first section (F.3.11).
    #[error(
        "the startxref after the main section gives {value}, where the first section is at {first} (ISO 32000-2, F.3.11)"
    )]
    LastStartxref {
        /// What it gives.
        value: u64,
        /// Where that section is.
        first: u64,
    },

    /// The catalog is in an object stream (F.3.1).
    #[error("the catalog, {number}, is in an object stream (ISO 32000-2, F.3.1)")]
    CatalogCompressed {
        /// Its number.
        number: u32,
    },
    /// The catalog isn't among what the first section lists (F.3.1, F.3.4).
    #[error("the catalog, {number}, isn't among what the first section lists (ISO 32000-2, F.3.4)")]
    CatalogNotFirst {
        /// Its number.
        number: u32,
    },
    /// A page's object is in an object stream (F.3.1).
    #[error("page {page}'s object, {number}, is in an object stream (ISO 32000-2, F.3.1)")]
    PageCompressed {
        /// The page.
        page: u64,
        /// Its object's number.
        number: u32,
    },
    /// An object not in an object stream is numbered above one that is, in one
    /// section, where they have the highest range (F.3.1). The hint stream is
    /// last by F.3.6 and isn't one of these.
    #[error(
        "{number} isn't in an object stream, and is numbered after one that is (ISO 32000-2, F.3.1)"
    )]
    UncompressedAfterCompressed {
        /// Its number.
        number: u32,
    },
    /// A row's generation isn't 0 (F.3.1).
    #[error("{number} is at generation {generation}, where all are at 0 (ISO 32000-2, F.3.1)")]
    Generation {
        /// The number.
        number: u32,
        /// Its generation.
        generation: u16,
    },
    /// The first group's numbers don't follow the second's (F.3.1).
    #[error(
        "the first section's numbers start at {first}, where the main section's end before {next} (ISO 32000-2, F.3.1)"
    )]
    GroupsApart {
        /// Where the first group's start.
        first: u32,
        /// Where they should, past the last number there can be where the main
        /// section lists that.
        next: u64,
    },
    /// An object is numbered after one that it's written before (F.3.1).
    #[error("{number} is written before the number below it (ISO 32000-2, F.3.1)")]
    NumberOutOfOrder {
        /// The number.
        number: u32,
    },

    /// The hint stream isn't numbered last (F.3.6).
    #[error(
        "the hint stream is {hint}, where the first section's last number is {last} (ISO 32000-2, F.3.6)"
    )]
    HintNotLast {
        /// Its number.
        hint: u32,
        /// The last.
        last: u32,
    },
    /// The reader didn't read the hint stream, for a reason of its own, so
    /// nothing's said of its data or its tables (F.4).
    #[error("the hint stream isn't read, as {why}, so its tables aren't judged (ISO 32000-2, F.4)")]
    HintUnread {
        /// Why.
        why: Unread,
    },
    /// What's at `/H` isn't a stream.
    #[error("what's at /H isn't a stream (ISO 32000-2, F.3.6)")]
    HintNotAStream,
    /// The hint stream doesn't decode, for what's wrong with it: not where
    /// the reader didn't read it ([`Finding::HintUnread`]).
    #[error("the hint stream's data doesn't decode (ISO 32000-2, F.3.6)")]
    HintUndecodable,
    /// The hint stream has no `/S` (F.3.6).
    #[error("the hint stream has no /S, the shared object table's position (ISO 32000-2, F.3.6)")]
    HintNoShared,
    /// A key that gives a table's position isn't one (F.3.6).
    #[error("the hint stream's {key} isn't a position in its data (ISO 32000-2, F.3.6)")]
    HintKeyValue {
        /// The key.
        key: KeyName,
    },
    /// A value in the hint stream's dictionary but `/Length` is a reference
    /// (F.3.6).
    #[error("the hint stream's {key} is a reference, and only /Length may be (ISO 32000-2, F.3.6)")]
    HintKeyIndirect {
        /// The key.
        key: KeyName,
    },
    /// A key F.3.6's table doesn't list, which may be a registered extension's
    /// or a private table's: it isn't read.
    #[error(
        "the hint stream has {key}, which ISO 32000-2's Table F.2 doesn't list, and which isn't read"
    )]
    HintKeyOther {
        /// The key.
        key: KeyName,
    },
    /// A hint table doesn't read: past its data, or a width past 32.
    #[error("the hint tables don't read: {fault} (ISO 32000-2, F.4)")]
    HintTables {
        /// Why.
        fault: hint::Error,
    },
    /// The hint tables are past a cap the check reads them under,
    /// [`Limits::pages`](crate::Limits::pages) or
    /// [`Limits::hint_entries`](crate::Limits::hint_entries), so nothing's
    /// said of them (F.4).
    #[error("the hint tables aren't judged: {fault} (ISO 32000-2, F.4)")]
    HintTablesCapped {
        /// Which, and how far past it.
        fault: hint::Error,
    },
    /// A table's fields run past the position of what's next (F.3.6).
    #[error(
        "the {table} table ends at byte {end} of the hint data, past the next's at {next} (ISO 32000-2, F.3.6)"
    )]
    TableOverruns {
        /// Which table, by its key in the hint stream's dictionary.
        table: &'static str,
        /// Where it ends.
        end: u64,
        /// Where the next starts.
        next: u64,
    },

    /// A page doesn't say an attribute it has to (F.3.7, F.3.10).
    #[error("page {page} doesn't say its {attribute} itself (ISO 32000-2, F.3.7, F.3.10)")]
    PageAttribute {
        /// The page.
        page: u64,
        /// The attribute.
        attribute: Attribute,
    },
    /// A page tree node holds an inheritable attribute (F.3.10).
    #[error(
        "page tree node {number} holds {attribute}, which is pushed down onto its pages (ISO 32000-2, F.3.10)"
    )]
    NodeAttribute {
        /// Its number.
        number: u32,
        /// The attribute.
        attribute: Attribute,
    },
    /// The reader didn't read the page tree, for a reason of its own, so
    /// nothing's said of its pages (F.3.7).
    #[error("the page tree isn't read, as {why}, so its pages aren't judged (ISO 32000-2, F.3.7)")]
    PagesUnread {
        /// Why.
        why: Unread,
    },
    /// The page tree didn't read, for what's wrong with it, or has no pages,
    /// so nothing depending on it can be said (F.3.7): not where the reader
    /// didn't read it ([`Finding::PagesUnread`]), nor where it stopped the
    /// sections short ([`Finding::SectionsUnread`]), past which the tree's
    /// objects may be listed.
    #[error("the page tree gives no pages, or doesn't read (ISO 32000-2, F.3.7)")]
    NoPages,
    /// The first page's object isn't among what the first section lists.
    #[error(
        "the first page's object, {number}, isn't among what the first section lists (ISO 32000-2, F.3.4)"
    )]
    FirstPageNotFirst {
        /// Its number.
        number: u32,
    },

    /// The page table's position of the first page's object isn't it (F.4.2).
    #[error(
        "the page table has the first page at {hint}, where it's at {computed} as if the hint stream weren't there (ISO 32000-2, F.4.2)"
    )]
    FirstPageAt {
        /// Where it says, from the file's first byte.
        hint: u64,
        /// Where it is.
        computed: u64,
    },
    /// Other objects of the first page's section come before its page object,
    /// where that's first, and the page table has the section's start for the
    /// object's (F.3.7, F.4.2): what a reader asks for from there, by the
    /// entry's length, is the whole of it.
    #[error(
        "the page table has the first page at {hint}, the start of its section, where its object is at {computed} as if the hint stream weren't there (ISO 32000-2, F.3.7)"
    )]
    FirstPageNotFirstObject {
        /// Where it says, from the file's first byte.
        hint: u64,
        /// Where its object is.
        computed: u64,
    },
    /// Adding up the page table's lengths doesn't land on a page's object
    /// (F.4.2).
    #[error(
        "the page table's lengths have page {page} at {hint}, where its object is at {computed} as if the hint stream weren't there (ISO 32000-2, F.4.2)"
    )]
    PageAt {
        /// The page.
        page: u64,
        /// Where the lengths have it.
        hint: u64,
        /// Where it is.
        computed: u64,
    },
    /// A page's count of objects isn't what its length holds (F.4.2).
    #[error(
        "the page table gives page {page} {hint} objects, where its length holds {computed} (ISO 32000-2, F.4.2)"
    )]
    PageObjects {
        /// The page.
        page: u64,
        /// What it says.
        hint: u64,
        /// What's there.
        computed: u64,
    },
    /// A page's length isn't what its objects take (F.4.2).
    #[error(
        "the page table gives page {page} {hint} bytes, where its objects take {computed} (ISO 32000-2, F.4.2)"
    )]
    PageLength {
        /// The page.
        page: u64,
        /// What it says.
        hint: u64,
        /// What they take.
        computed: u64,
    },
    /// A page's content items run past its length (F.4.2).
    #[error(
        "the page table has page {page}'s content end at {end}, past its length of {length} (ISO 32000-2, F.4.2)"
    )]
    ContentPast {
        /// The page.
        page: u64,
        /// Where they have it end.
        end: u64,
        /// The page's length.
        length: u64,
    },
    /// The first page has entries for shared objects, where a viewer has them
    /// all by `/E` (F.4.2).
    #[error(
        "the page table gives the first page {entries} shared objects, where it has none (ISO 32000-2, F.4.2)"
    )]
    FirstPageShared {
        /// How many it gives.
        entries: u64,
    },
    /// A page's shared object identifier is past the shared object table
    /// (F.4.2).
    #[error(
        "the page table gives page {page} shared object {id}, where the table has {entries} (ISO 32000-2, F.4.2)"
    )]
    SharedIdPast {
        /// The page.
        page: u64,
        /// The identifier.
        id: u32,
        /// How many entries the table has.
        entries: u32,
    },

    /// More of the shared object table is the first page's than there is
    /// (F.4.3).
    #[error(
        "the shared object table gives the first page {first} of its {total} entries (ISO 32000-2, F.4.3)"
    )]
    SharedCounts {
        /// How many are the first page's.
        first: u32,
        /// How many there are.
        total: u32,
    },
    /// The shared object table's first object isn't where it says (F.4.3).
    #[error(
        "the shared object table has its first object past the first page, {number}, at {hint}, {} (ISO 32000-2, F.4.3)",
        placed(.computed)
    )]
    SharedFirstAt {
        /// Its number.
        number: u32,
        /// Where it says, from the file's first byte.
        hint: u64,
        /// Where it is, if it's listed.
        computed: Option<u64>,
    },
    /// An entry's count of objects isn't what its length holds (F.4.3).
    #[error(
        "the shared object table gives entry {entry} {hint} objects, where its length holds {computed} (ISO 32000-2, F.4.3)"
    )]
    SharedObjects {
        /// The entry.
        entry: u64,
        /// What it says.
        hint: u64,
        /// What's there.
        computed: u64,
    },
    /// An entry's length isn't what its objects take (F.4.3).
    #[error(
        "the shared object table gives entry {entry} {hint} bytes, where its objects take {computed} (ISO 32000-2, F.4.3)"
    )]
    SharedLength {
        /// The entry.
        entry: u64,
        /// What it says.
        hint: u64,
        /// What they take.
        computed: u64,
    },
    /// An entry says it has a signature, which is read past (F.4.3).
    #[error(
        "the shared object table says entry {entry} has a signature, which isn't checked (ISO 32000-2, F.4.3)"
    )]
    Signature {
        /// The entry.
        entry: u64,
    },

    /// A table's first object isn't listed (F.4.4 to F.4.7).
    #[error("the {table} table's first object, {number}, isn't listed (ISO 32000-2, F.4)")]
    TableObject {
        /// Which table, by its key.
        table: &'static str,
        /// The number.
        number: u32,
    },
    /// A table's first object isn't where it says (F.4.4 to F.4.7).
    #[error(
        "the {table} table has its first object at {hint}, where it's at {computed} as if the hint stream weren't there (ISO 32000-2, F.4)"
    )]
    TableAt {
        /// Which table.
        table: &'static str,
        /// Where it says, from the file's first byte.
        hint: u64,
        /// Where it is.
        computed: u64,
    },
    /// A table's count of objects isn't what its length holds (F.4.4 to F.4.7).
    #[error(
        "the {table} table gives {hint} objects, where its length holds {computed} (ISO 32000-2, F.4)"
    )]
    TableObjects {
        /// Which table.
        table: &'static str,
        /// What it says.
        hint: u64,
        /// What's there.
        computed: u64,
    },
    /// A table's length isn't what its objects take (F.4.4 to F.4.7).
    #[error(
        "the {table} table gives {hint} bytes, where its objects take {computed} (ISO 32000-2, F.4)"
    )]
    TableLength {
        /// Which table.
        table: &'static str,
        /// What it says.
        hint: u64,
        /// What they take.
        computed: u64,
    },
    /// A table's shared object identifier is past the shared object table
    /// (F.4.6, F.4.7).
    #[error(
        "the {table} table gives shared object {id}, where the table has {entries} (ISO 32000-2, F.4)"
    )]
    TableIdPast {
        /// Which table.
        table: &'static str,
        /// The identifier.
        id: u32,
        /// How many entries the table has.
        entries: u32,
    },
    /// The thumbnail table says of more pages than there are (F.4.4).
    #[error(
        "the thumbnail table's entries and gaps come to {said} pages, where there are {pages} (ISO 32000-2, F.4.4)"
    )]
    ThumbnailPages {
        /// What they come to.
        said: u64,
        /// How many there are.
        pages: u64,
    },

    /// The objects given as the file's graph can't be planned, for what they
    /// hold, so what a page reaches isn't said.
    #[error("what the file's objects reach can't be said (ISO 32000-2, F.3.7): {why}")]
    GraphRefused {
        /// Why.
        why: plan::Error,
    },
    /// The objects given as the file's graph are past a cap a plan is made
    /// under ([`Limits`](crate::Limits)), so what a page reaches isn't judged
    /// (F.3.7).
    #[error("what the file's objects reach isn't judged: {why} (ISO 32000-2, F.3.7)")]
    GraphCapped {
        /// Which, and how far past it.
        why: plan::Error,
    },
    /// An object the first page alone or with others reaches isn't in the first
    /// group (F.3.7).
    #[error(
        "{number}, which the first page reaches, isn't among what the first section lists (ISO 32000-2, F.3.7)"
    )]
    FirstPageReaches {
        /// Its number.
        number: u32,
    },
    /// An object only this later page reaches isn't in its section (F.3.8).
    #[error(
        "{number}, which only page {page} reaches, isn't in that page's section (ISO 32000-2, F.3.8)"
    )]
    PageReaches {
        /// The page.
        page: u64,
        /// Its number.
        number: u32,
    },
    /// A later page reaches what the shared object table has as this entry, and
    /// the page's list doesn't name it (F.4.2).
    #[error(
        "page {page} reaches what entry {entry} of the shared object table holds, and doesn't list it (ISO 32000-2, F.4.2)"
    )]
    SharedNotListed {
        /// The page.
        page: u64,
        /// The entry.
        entry: u64,
    },
    /// A later page lists an entry of the shared object table that holds
    /// nothing it reaches (F.4.2).
    #[error(
        "page {page} lists entry {entry} of the shared object table, and reaches nothing it holds (ISO 32000-2, F.4.2)"
    )]
    SharedNotReached {
        /// The page.
        page: u64,
        /// The entry.
        entry: u64,
    },
}

/// Where an object is as if the hint stream weren't there, as
/// [`Finding::SharedFirstAt`] says it: at a position, or listed nowhere.
fn placed(computed: &Option<u64>) -> String {
    match computed {
        Some(at) => format!("where it's at {at} as if the hint stream weren't there"),
        None => "where nothing lists it in use".to_string(),
    }
}

impl Finding {
    /// Where it's said of: none for what's of the file as a whole.
    #[must_use]
    pub const fn place(&self) -> Option<Place> {
        use Finding::*;
        use Place::*;
        Some(match *self {
            HintUnread { .. }
            | SectionsUnread { .. }
            | PagesUnread { .. }
            | NoSections
            | NoMainSection
            | FirstSubsections { .. }
            | FirstSize { .. }
            | FirstNoRoot
            | MainSubsections { .. }
            | MainFrom { .. }
            | MainPrev
            | HintNotAStream
            | HintUndecodable
            | HintNoShared
            | NoPages
            | FirstPageShared { .. }
            | SharedCounts { .. }
            | HintTablesCapped { .. }
            | GraphRefused { .. }
            | GraphCapped { .. } => return None,
            HintTables { fault } => match fault {
                hint::Error::PastEnd { bit, .. } => HintBit(bit),
                hint::Error::TableAt { table, .. } => Table(table),
                _ => return None,
            },
            Version => Key(DictKey::Linearized),
            LengthNotFile { .. } => Key(DictKey::L),
            BadValue { key } | Indirect { key } => Key(key),
            HintItems { .. } | HintNotListed { .. } | HintExtent { .. } => Key(DictKey::H),
            PageCount { .. } => Key(DictKey::N),
            FirstPageEnd { .. } => Key(DictKey::E),
            MainFirstEntry { .. } => Key(DictKey::T),
            FirstPageObject { .. } => Key(DictKey::O),
            Updated { at } => Byte(at),
            DictionaryPast1024 { end } => Byte(end),
            SectionOrder { first, .. } => Byte(first),
            LastStartxref { value, .. } => Byte(value),
            MainZeroInUse => Object(0),
            FirstFree { number }
            | FirstNotDictionary { number, .. }
            | MainFree { number }
            | CatalogCompressed { number }
            | CatalogNotFirst { number }
            | UncompressedAfterCompressed { number }
            | Generation { number, .. }
            | NumberOutOfOrder { number }
            | NodeAttribute { number, .. }
            | FirstPageNotFirst { number }
            | SharedFirstAt { number, .. }
            | TableObject { number, .. }
            | FirstPageReaches { number }
            | PageReaches { number, .. }
            | PageCompressed { number, .. } => Object(number),
            GroupsApart { first, .. } => Object(first),
            HintNotLast { hint, .. } => Object(hint),
            HintKeyValue { key } | HintKeyIndirect { key } | HintKeyOther { key } => HintKey(key),
            TableOverruns { end, .. } => HintByte(end),
            PageAttribute { page, .. }
            | PageAt { page, .. }
            | PageObjects { page, .. }
            | PageLength { page, .. }
            | ContentPast { page, .. }
            | SharedIdPast { page, .. }
            | SharedNotListed { page, .. }
            | SharedNotReached { page, .. } => Page(page),
            FirstPageAt { computed, .. }
            | FirstPageNotFirstObject { computed, .. }
            | TableAt { computed, .. } => Position(computed),
            SharedObjects { entry, .. } | SharedLength { entry, .. } | Signature { entry } => {
                SharedEntry(entry)
            }
            TableObjects { table, .. } | TableLength { table, .. } | TableIdPast { table, .. } => {
                Table(table)
            }
            ThumbnailPages { .. } => Table("T"),
        })
    }

    /// The clause of ISO 32000-2 it departs from, as its message cites it.
    #[must_use]
    pub const fn clause(&self) -> &'static str {
        use Finding::*;
        match self {
            Version
            | LengthNotFile { .. }
            | BadValue { .. }
            | HintItems { .. }
            | FirstPageObject { .. }
            | PageCount { .. }
            | FirstPageEnd { .. }
            | MainFirstEntry { .. }
            | HintNotListed { .. }
            | HintExtent { .. } => "Table F.1",
            Updated { .. } => "G.7",
            Indirect { .. } | DictionaryPast1024 { .. } => "F.3.3",
            NoSections => "7.5.8.2, F.3.4",
            SectionsUnread { .. } => "F.3.4, F.3.11",
            SectionOrder { .. } => "F.3",
            NoMainSection
            | FirstSubsections { .. }
            | FirstFree { .. }
            | FirstSize { .. }
            | FirstNoRoot
            | FirstNotDictionary { .. }
            | CatalogNotFirst { .. }
            | FirstPageNotFirst { .. } => "F.3.4",
            MainSubsections { .. }
            | MainFrom { .. }
            | MainZeroInUse
            | MainFree { .. }
            | MainPrev
            | LastStartxref { .. } => "F.3.11",
            CatalogCompressed { .. }
            | PageCompressed { .. }
            | UncompressedAfterCompressed { .. }
            | Generation { .. }
            | GroupsApart { .. }
            | NumberOutOfOrder { .. } => "F.3.1",
            HintNotLast { .. }
            | HintNotAStream
            | HintUndecodable
            | HintNoShared
            | HintKeyValue { .. }
            | HintKeyIndirect { .. }
            | TableOverruns { .. } => "F.3.6",
            HintKeyOther { .. } => "Table F.2",
            HintUnread { .. }
            | HintTables { .. }
            | HintTablesCapped { .. }
            | TableObject { .. }
            | TableAt { .. }
            | TableObjects { .. }
            | TableLength { .. }
            | TableIdPast { .. } => "F.4",
            PageAttribute { .. } => "F.3.7, F.3.10",
            NodeAttribute { .. } => "F.3.10",
            PagesUnread { .. }
            | NoPages
            | FirstPageNotFirstObject { .. }
            | GraphRefused { .. }
            | GraphCapped { .. }
            | FirstPageReaches { .. } => "F.3.7",
            PageReaches { .. } => "F.3.8",
            FirstPageAt { .. }
            | PageAt { .. }
            | PageObjects { .. }
            | PageLength { .. }
            | ContentPast { .. }
            | FirstPageShared { .. }
            | SharedIdPast { .. }
            | SharedNotListed { .. }
            | SharedNotReached { .. } => "F.4.2",
            SharedCounts { .. }
            | SharedFirstAt { .. }
            | SharedObjects { .. }
            | SharedLength { .. }
            | Signature { .. } => "F.4.3",
            ThumbnailPages { .. } => "F.4.4",
        }
    }

    /// How much it matters: [`Grade::Error`] by definition for what a reader
    /// acts on (a position, a length, a count, an identifier past its table, a
    /// value it can't read), [`Grade::Departure`] where real files, read by
    /// real viewers, show that nothing goes wrong by it, and
    /// [`Grade::Unjudged`] for what wasn't judged.
    #[must_use]
    pub const fn grade(&self) -> Grade {
        use Finding::*;
        match self {
            HintUnread { .. }
            | SectionsUnread { .. }
            | PagesUnread { .. }
            | HintTablesCapped { .. }
            | GraphCapped { .. } => Grade::Unjudged,
            // A reader goes on: what these leave out it fetches by the
            // sections.
            Updated { .. }
            | DictionaryPast1024 { .. }
            | FirstSubsections { .. }
            | FirstFree { .. }
            | MainFree { .. }
            | CatalogNotFirst { .. }
            | UncompressedAfterCompressed { .. }
            | Generation { .. }
            | NumberOutOfOrder { .. }
            | HintNotLast { .. }
            | HintKeyIndirect { .. }
            | HintKeyOther { .. }
            | TableOverruns { .. }
            | PageAttribute { .. }
            | NodeAttribute { .. }
            | FirstPageNotFirstObject { .. }
            | FirstPageShared { .. }
            | Signature { .. }
            | GraphRefused { .. }
            | FirstPageReaches { .. }
            | PageReaches { .. }
            | SharedNotListed { .. }
            | SharedNotReached { .. } => Grade::Departure,
            Version
            | LengthNotFile { .. }
            | BadValue { .. }
            | Indirect { .. }
            | HintItems { .. }
            | FirstPageObject { .. }
            | PageCount { .. }
            | FirstPageEnd { .. }
            | MainFirstEntry { .. }
            | HintNotListed { .. }
            | HintExtent { .. }
            | NoSections
            | NoMainSection
            | SectionOrder { .. }
            | FirstSize { .. }
            | FirstNoRoot
            | FirstNotDictionary { .. }
            | MainSubsections { .. }
            | MainFrom { .. }
            | MainZeroInUse
            | MainPrev
            | LastStartxref { .. }
            | CatalogCompressed { .. }
            | PageCompressed { .. }
            | GroupsApart { .. }
            | HintNotAStream
            | HintUndecodable
            | HintNoShared
            | HintKeyValue { .. }
            | HintTables { .. }
            | NoPages
            | FirstPageNotFirst { .. }
            | FirstPageAt { .. }
            | PageAt { .. }
            | PageObjects { .. }
            | PageLength { .. }
            | ContentPast { .. }
            | SharedIdPast { .. }
            | SharedCounts { .. }
            | SharedFirstAt { .. }
            | SharedObjects { .. }
            | SharedLength { .. }
            | TableObject { .. }
            | TableAt { .. }
            | TableObjects { .. }
            | TableLength { .. }
            | TableIdPast { .. }
            | ThumbnailPages { .. } => Grade::Error,
        }
    }
}
