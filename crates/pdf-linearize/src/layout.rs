//! The bytes around a caller's bodies: the header, the linearization
//! dictionary, both cross-reference sections and the hint stream, and the order
//! to write everything in (ISO 32000-2, F.3).
//!
//! A caller serializes each of [`Plan::writes`] as the plan numbers it, whole,
//! `n 0 obj` to `endobj` and a line's end, and gives [`assemble`] their
//! lengths: nothing in a body depends on an offset, so nothing here sees one.
//! It gives a [`Layout`], whose [`pieces`](Layout::pieces) say what's next in
//! the file: bytes it holds, or the caller's body. The streams of its own go
//! through the closure a caller gives, which may filter or encrypt them, so no
//! filter or key is here.
//!
//! # The file's order
//!
//! The header and a comment of binary bytes (part 1), the dictionary (2), the
//! first-page section (3), the catalog and what's wanted at open (4), the hint
//! stream (5, which may be here or after part 6: F.3.6), the first page (6),
//! the other pages (7), what they share (8), everything else (9) and the main
//! section (11). No overflow hint stream (10).
//!
//! # Settling what depends on what
//!
//! The dictionary comes first and says where everything is, so it's a fixed
//! length, padded with spaces after `endobj`, and written last. What the hint
//! tables say doesn't depend on the hint stream's length (F.4.1). The hint
//! stream is before the first page, so a first-page section's rows do; and
//! under a stream the section's own length depends on what its rows filter to:
//!
//! - **A table** has rows of 20 bytes and room in its `/Prev` for any position,
//!   so its length is known before anything goes in it: one pass.
//! - **A stream** is given room first, from what its rows filter to at a first
//!   guess of them, and the rest is laid out behind that. Then the rows are
//!   made and filtered, and if they fit they're padded with spaces after
//!   `endobj`. If not, the room grows and it goes round again. Each round calls
//!   the closure for the hint stream and the section. `/W` is fixed for any
//!   file a hint table can describe, so a row's width doesn't move between
//!   rounds; and past what any rows filter to they fit, so a closure that's a
//!   function of its data settles: only one giving ever-longer data doesn't,
//!   and is refused ([`Error::Unsettled`]).

use std::fmt;
use std::io::Write as _;
use std::iter::FusedIterator;

use pdf_objects::version::Version;

use crate::hint::{
    self, EmbeddedFiles, Extended, FileEntry, Generic, GroupEntry, HintKeys, PageEntry,
    PageOffsets, SharedObjects, ThumbEntry, Thumbnails,
};
use crate::phases::phase;
use crate::plan::{Group, Kind, Part, Plan, What};

/// How long the dictionary is, with its padding: every value at its largest
/// comes to under 130 bytes, and with any header this writes it's well within
/// the 1,024 bytes that F.3.3 keeps it to.
const DICTIONARY: usize = 200;

/// After the header, a comment of bytes past ASCII (7.5.2).
const BINARY: &[u8] = b"%\xE2\xE3\xCF\xD3\n";

/// How many bytes each of a stream section's rows has of each field: enough for
/// any position a hint table can say, and for entry 0's generation.
const WIDTHS: [usize; 3] = [1, 4, 2];

/// A first-page table's `/Prev`: room for any position, so its length doesn't
/// move.
const PREV: usize = 20;

/// What ends a first-page section: `startxref` to nowhere (F.3.4).
const DUMMY: &[u8] = b"startxref\n0\n%%EOF\n";

/// How many rounds a first-page stream gets to settle in.
const ROUNDS: u32 = 16;

/// The most a file can be, as a hint table's positions and lengths are 32 bits
/// (F.4.2).
const LARGEST: u64 = u32::MAX as u64;

/// What [`assemble`] is told that a plan doesn't say. Set on a copy of
/// [`Options::new`]'s. Its `Debug` gives the trailer's entries by their
/// length alone, as they're the caller's bytes.
#[derive(Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Options<'a> {
    /// The header's version.
    pub version: Version,
    /// The first-page section's trailer's other entries, already written, in
    /// the numbers the plan gives: `/Info`, `/Encrypt`, `/ID` and the like. No
    /// `/Root` or a key the section writes itself.
    pub trailer: &'a [u8],
}

impl Options<'static> {
    /// A file of `version` with nothing else in its trailer.
    #[must_use]
    pub const fn new(version: Version) -> Options<'static> {
        Options {
            version,
            trailer: b"",
        }
    }
}

impl fmt::Debug for Options<'_> {
    /// Its version, and the trailer's entries by their length, not their
    /// bytes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Options")
            .field("version", &self.version)
            .field("trailer", &format_args!("{} bytes", self.trailer.len()))
            .finish()
    }
}

/// Which of the streams of its own [`assemble`] is asking to have encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StreamKind {
    /// The primary hint stream.
    Hint,
    /// The first-page cross-reference stream.
    FirstXref,
    /// The main cross-reference stream.
    MainXref,
}

/// A stream [`assemble`] wants encoded. Its `Debug` gives its data by its
/// length alone, as a caller's closure may be handed what's then encrypted.
#[derive(Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Stream<'a> {
    /// Which it is.
    pub what: StreamKind,
    /// Its number, which an encrypting closure wants.
    pub number: u32,
    /// Its data.
    pub data: &'a [u8],
    /// A cross-reference stream's bytes per row, for a predictor.
    pub columns: Option<u32>,
}

/// A stream as it's to stand. Its `Debug` gives its entries and data by
/// their length alone.
#[derive(Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Encoded {
    /// What goes in its dictionary to say so, `/Filter` and the like, as
    /// written.
    pub entries: Vec<u8>,
    /// Its data as it's to be written.
    pub data: Vec<u8>,
}

impl Encoded {
    /// `data`, under dictionary entries `entries`.
    #[must_use]
    pub const fn new(entries: Vec<u8>, data: Vec<u8>) -> Encoded {
        Encoded { entries, data }
    }

    /// `data`, as it is.
    #[must_use]
    pub const fn plain(data: Vec<u8>) -> Encoded {
        Encoded {
            entries: Vec::new(),
            data,
        }
    }
}

impl fmt::Debug for Stream<'_> {
    /// Which it is, its number and columns, and its data by its length, not
    /// its bytes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stream")
            .field("what", &self.what)
            .field("number", &self.number)
            .field("data", &format_args!("{} bytes", self.data.len()))
            .field("columns", &self.columns)
            .finish()
    }
}

impl fmt::Debug for Encoded {
    /// Its entries and data by their length, not their bytes, which may be
    /// encrypted or hold what's to be.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Encoded")
            .field("entries", &format_args!("{} bytes", self.entries.len()))
            .field("data", &format_args!("{} bytes", self.data.len()))
            .finish()
    }
}

/// What's next in a file: see [`Layout::pieces`]. Exhaustive on purpose: a
/// writer that left out a piece of a kind it didn't know would write a
/// wrong file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Piece<'l> {
    /// These bytes.
    Bytes(&'l [u8]),
    /// The caller's body of [`Plan::writes`]'s write at this place.
    Body(usize),
}

/// Why a file can't be assembled. `E` is what a closure's refusal is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error<E> {
    /// Not one length per write.
    #[error("{lengths} lengths for {writes} writes")]
    Lengths {
        /// How many lengths were given.
        lengths: usize,
        /// How many writes the plan has.
        writes: usize,
    },
    /// A file a hint table's 32-bit fields can't describe.
    #[error("the file would be at least {len} bytes, past the {LARGEST} a hint table can say")]
    TooLarge {
        /// How long it would be, as far as that's known.
        len: u64,
    },
    /// The closure refused a stream.
    #[error("{0}")]
    Encode(E),
    /// A first-page stream that wouldn't fit, however much room it was given.
    #[error("the first-page cross-reference stream wanted more room each of {ROUNDS} rounds")]
    Unsettled,
    /// A hint table that couldn't be written, which what a plan gives can't
    /// cause.
    #[error("{0}")]
    Hint(hint::Error),
}

/// What's written around a caller's bodies, and where everything is. Its
/// `Debug` gives where each thing is and how long, never a byte it holds,
/// as the trailer's entries a caller gives and the hint data are a file's.
#[derive(Clone, PartialEq, Eq)]
pub struct Layout {
    /// The header, the dictionary and the first-page section.
    head: Vec<u8>,
    /// The hint stream, as an object.
    hint: Vec<u8>,
    /// The main section.
    main: Vec<u8>,
    /// How many writes there are, and how many before the hint stream.
    writes: usize,
    before_hint: usize,
    rows: Vec<Row>,
    len: u64,
    first_page_end: u64,
    hint_at: u64,
    first_section_at: u64,
    main_section_at: u64,
    main_first_entry: u64,
    data: Vec<u8>,
    keys: HintKeys,
}

impl Layout {
    /// What's in the file, in order: its bytes are these, one after another.
    /// Write them, or hand them on; a [`Piece::Body`] is the caller's to fill
    /// in.
    pub const fn pieces(&self) -> Pieces<'_> {
        Pieces {
            layout: self,
            at: 0,
        }
    }

    /// The file's length: the dictionary's `/L`.
    #[must_use]
    pub const fn len(&self) -> u64 {
        self.len
    }

    /// Whether the file has no bytes, which it never hasn't.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Where object `number` is, or none for a number nothing is at, or one in
    /// an object stream.
    #[must_use]
    pub fn position(&self, number: u32) -> Option<u64> {
        match self.rows.get(number as usize) {
            Some(Row::At(position)) => Some(*position),
            _ => None,
        }
    }

    /// Where the first page ends: the dictionary's `/E`.
    #[must_use]
    pub const fn first_page_end(&self) -> u64 {
        self.first_page_end
    }

    /// Where the hint stream's object is, and its length, overhead and all: the
    /// dictionary's `/H`.
    #[must_use]
    pub const fn hint_extent(&self) -> (u64, u64) {
        (self.hint_at, self.hint.len() as u64)
    }

    /// Where the first-page section starts.
    #[must_use]
    pub const fn first_section(&self) -> u64 {
        self.first_section_at
    }

    /// Where the main section starts, the first-page section's `/Prev`.
    #[must_use]
    pub const fn main_section(&self) -> u64 {
        self.main_section_at
    }

    /// Where the main section's first entry is said to be: the dictionary's
    /// `/T`.
    #[must_use]
    pub const fn main_first_entry(&self) -> u64 {
        self.main_first_entry
    }

    /// The hint stream's data before the closure had it.
    #[must_use]
    pub fn hint_data(&self) -> &[u8] {
        &self.data
    }

    /// Where each table is in that data.
    #[must_use]
    pub const fn hint_keys(&self) -> &HintKeys {
        &self.keys
    }
}

impl fmt::Debug for Layout {
    /// Where each thing is and how long, and how many writes there are and
    /// before the hint stream: never a byte it holds, which are a file's.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Layout")
            .field("len", &self.len)
            .field("head", &format_args!("{} bytes", self.head.len()))
            .field("first_section", &self.first_section_at)
            .field("writes", &self.writes)
            .field("before_hint", &self.before_hint)
            .field("hint", &self.hint_extent())
            .field("first_page_end", &self.first_page_end)
            .field("main_section", &self.main_section_at)
            .field("main_first_entry", &self.main_first_entry)
            .field("main", &format_args!("{} bytes", self.main.len()))
            .field("numbers", &self.rows.len())
            .field("hint_data", &format_args!("{} bytes", self.data.len()))
            .field("hint_keys", &self.keys)
            .finish()
    }
}

/// A [`Layout`]'s pieces, in file order: what [`Layout::pieces`] gives. Its
/// `Debug` says how many are left, never a byte of them.
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Clone)]
pub struct Pieces<'l> {
    layout: &'l Layout,
    /// The next piece's place among them all: the head, the bodies before the
    /// hint stream, the hint stream, the bodies after it and the main section.
    at: usize,
}

impl<'l> Pieces<'l> {
    /// How many pieces a layout has: its writes, and the three of its own.
    fn total(&self) -> usize {
        self.layout.writes + 3
    }

    /// The hint stream's place among the pieces, after the head and the bodies
    /// before it.
    fn hint(&self) -> usize {
        self.layout.before_hint + 1
    }
}

impl<'l> Iterator for Pieces<'l> {
    type Item = Piece<'l>;

    #[inline]
    fn next(&mut self) -> Option<Piece<'l>> {
        let (at, hint, total) = (self.at, self.hint(), self.total());
        let layout = self.layout;
        let piece = match at {
            0 => Piece::Bytes(&layout.head),
            _ if at < hint => Piece::Body(at - 1),
            _ if at == hint => Piece::Bytes(&layout.hint),
            _ if at + 1 < total => Piece::Body(at - 2),
            _ if at + 1 == total => Piece::Bytes(&layout.main),
            _ => return None,
        };
        self.at += 1;
        Some(piece)
    }

    /// Exact: what's left of the writes and the three of its own.
    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.total().saturating_sub(self.at);
        (left, Some(left))
    }

    /// One pass over what's left: each stretch of bodies a range's own fold,
    /// as `count`, `for_each` and `sum` fold.
    #[inline]
    fn fold<B, F: FnMut(B, Piece<'l>) -> B>(self, init: B, mut f: F) -> B {
        let (at, hint, total) = (self.at, self.hint(), self.total());
        let layout = self.layout;
        // What's left of the pieces from `start` to before `end`.
        let left = |start: usize, end: usize| at.max(start)..end;
        let mut acc = init;
        if at == 0 {
            acc = f(acc, Piece::Bytes(&layout.head));
        }
        acc = (left(1, hint).map(|i| Piece::Body(i - 1))).fold(acc, &mut f);
        if at <= hint {
            acc = f(acc, Piece::Bytes(&layout.hint));
        }
        acc = (left(hint + 1, total - 1).map(|i| Piece::Body(i - 2))).fold(acc, &mut f);
        if at < total {
            acc = f(acc, Piece::Bytes(&layout.main));
        }
        acc
    }
}

impl ExactSizeIterator for Pieces<'_> {}

// `at` only grows, and `next` gives nothing once it's past the main section.
impl FusedIterator for Pieces<'_> {}

impl fmt::Debug for Pieces<'_> {
    /// How many pieces are left, and of how many: not their bytes, which are
    /// a file's.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pieces")
            .field("left", &self.len())
            .field("of", &self.total())
            .finish()
    }
}

/// What a section says of a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Free,
    At(u64),
    In { stream: u32, index: u16 },
}

/// Where the writes are, as a list of how far each starts from the first.
struct Places {
    starts: Vec<u64>,
    /// How many writes come before the hint stream.
    before_hint: usize,
    /// Where the first write is.
    bodies_at: u64,
    hint_len: u64,
}

impl Places {
    /// Where write `at` is, or the end of them, as if there were no hint
    /// stream.
    fn without_hint(&self, at: usize) -> u64 {
        self.bodies_at + self.starts[at]
    }

    /// Where write `at` is, or the end of them.
    fn at(&self, at: usize) -> u64 {
        match at >= self.before_hint {
            true => self.without_hint(at) + self.hint_len,
            false => self.without_hint(at),
        }
    }

    /// Where the hint stream is.
    fn hint_at(&self) -> u64 {
        self.without_hint(self.before_hint)
    }

    /// How long writes `range` are, from the first's start to the last's end.
    fn span(&self, range: &std::ops::Range<usize>) -> u64 {
        match range.is_empty() {
            true => 0,
            false => self.starts[range.end] - self.starts[range.start],
        }
    }
}

/// The bytes around a plan's bodies, whose lengths are `lengths`, one per
/// [`Plan::writes`]: `encode` says how each stream of its own stands.
///
/// # Errors
///
/// Refused, under an [`Error`]: not one length per write ([`Error::Lengths`]);
/// a file past what a hint table's positions can say, 4 GiB
/// ([`Error::TooLarge`]); a refusal of `encode`'s ([`Error::Encode`]); a
/// first-page stream that `encode` makes longer each round
/// ([`Error::Unsettled`]); and a hint table that can't be written, which a
/// plan can't cause ([`Error::Hint`]).
pub fn assemble<E>(
    plan: &Plan,
    lengths: &[u64],
    options: &Options<'_>,
    mut encode: impl FnMut(Stream<'_>) -> Result<Encoded, E>,
) -> Result<Layout, Error<E>> {
    let writes = plan.writes().len();
    phase!(DEBUG, "assemble", writes = writes);
    if lengths.len() != writes {
        return Err(Error::Lengths {
            lengths: lengths.len(),
            writes,
        });
    }
    let mut starts = Vec::with_capacity(writes + 1);
    let mut total = 0u64;
    starts.push(0);
    for &length in lengths {
        total = total.saturating_add(length);
        if total > LARGEST {
            return Err(Error::TooLarge { len: total });
        }
        starts.push(total);
    }

    let mut head = format!("%PDF-{}\n", options.version).into_bytes();
    head.extend_from_slice(BINARY);
    let dictionary_at = head.len() as u64;
    let first_section_at = dictionary_at + DICTIONARY as u64;

    let mut places = Places {
        starts,
        before_hint: plan.part_writes(Part::FirstPage).start,
        bodies_at: first_section_at,
        hint_len: 0,
    };
    let at = Fixed {
        dictionary_at,
        first_section_at,
    };

    // The room the section is given: what it comes to, where a table's can't
    // change, and a stream's is first found from rows at a first guess.
    let mut room = match plan.kind() {
        Kind::Table => first_table(plan, &rows(plan, &places, at, 0), 0, options).len() as u64,
        Kind::Stream => {
            let rows = rows(plan, &places, at, 0);
            let section = first_stream(plan, &rows, 0, options, &mut encode)?;
            grown(section.len() as u64)
        }
    };
    let mut settled = None;
    for _ in 0..ROUNDS {
        phase!(DEBUG, "round", room = room);
        places.bodies_at = first_section_at + room;
        places.hint_len = 0;
        if places.without_hint(writes) > LARGEST {
            return Err(Error::TooLarge {
                len: places.without_hint(writes),
            });
        }
        let (data, keys) = hints(plan, &places).map_err(Error::Hint)?;
        let encoded = encode(Stream {
            what: StreamKind::Hint,
            number: plan.hint_number(),
            data: &data,
            columns: None,
        })
        .map_err(Error::Encode)?;
        let hint = hint_object(plan.hint_number(), &keys, &encoded);
        places.hint_len = hint.len() as u64;
        let main_at = places.at(writes);
        if main_at > LARGEST {
            return Err(Error::TooLarge { len: main_at });
        }
        let rows = rows(plan, &places, at, main_at);
        let mut section = match plan.kind() {
            Kind::Table => first_table(plan, &rows, main_at, options),
            Kind::Stream => first_stream(plan, &rows, main_at, options, &mut encode)?,
        };
        let wanted = section.len() as u64;
        if wanted <= room {
            pad(&mut section, room as usize);
            settled = Some((data, keys, hint, rows, section, main_at));
            break;
        }
        room = room.max(grown(wanted));
    }
    let Some((data, keys, hint, rows, section, main_at)) = settled else {
        return Err(Error::Unsettled);
    };

    let (main, t) = match plan.kind() {
        Kind::Table => main_table(plan, &rows, main_at, first_section_at),
        Kind::Stream => (
            main_stream(plan, &rows, first_section_at, &mut encode)?,
            main_at,
        ),
    };
    let len = main_at + main.len() as u64;
    if len > LARGEST {
        return Err(Error::TooLarge { len });
    }
    let first_page_end = places.at(plan.part_writes(Part::FirstPage).end);
    let (hint_at, hint_len) = (places.hint_at(), hint.len() as u64);
    write_dictionary(&mut head, plan, len, (hint_at, hint_len), first_page_end, t);
    head.extend_from_slice(&section);

    Ok(Layout {
        head,
        hint,
        main,
        writes,
        before_hint: places.before_hint,
        rows,
        len,
        first_page_end,
        hint_at,
        first_section_at,
        main_section_at: main_at,
        main_first_entry: t,
        data,
        keys,
    })
}

/// A little more than `wanted`, as a section's next length may differ a little.
fn grown(wanted: u64) -> u64 {
    wanted + wanted / 4 + 16
}

/// `section` padded to `room` with spaces and a line's end, before the dummy
/// `startxref` that ends it: after a stream's `endobj`, as nothing else can
/// move. A table's has no padding to take, its length being what it was given.
fn pad(section: &mut Vec<u8>, room: usize) {
    let padding = room - section.len();
    if padding > 0 {
        let at = section.len() - DUMMY.len();
        let mut fill = vec![b' '; padding];
        fill[padding - 1] = b'\n';
        section.splice(at..at, fill);
    }
}

/// Where the things before the writes are.
#[derive(Clone, Copy)]
struct Fixed {
    dictionary_at: u64,
    first_section_at: u64,
}

/// What each number is, by number: 0 free, the layout's own and each write at
/// their places, and each member in its stream.
fn rows(plan: &Plan, places: &Places, at: Fixed, main_at: u64) -> Vec<Row> {
    let mut rows = vec![Row::Free; plan.size() as usize];
    rows[plan.dictionary_number() as usize] = Row::At(at.dictionary_at);
    rows[plan.hint_number() as usize] = Row::At(places.hint_at());
    if let Some(number) = plan.first_xref_number() {
        rows[number as usize] = Row::At(at.first_section_at);
    }
    if let Some(number) = plan.main_xref_number() {
        rows[number as usize] = Row::At(main_at);
    }
    for (w, write) in plan.writes().iter().enumerate() {
        rows[write.number as usize] = Row::At(places.at(w));
        if let What::Container(container) = write.what {
            let members = &plan.containers()[container].members;
            for (index, (_, number)) in (0..).zip(members) {
                rows[*number as usize] = Row::In {
                    stream: write.number,
                    index,
                };
            }
        }
    }
    rows
}

/// `rows` as a table's lines (7.5.4). What's in an object stream can't come up:
/// a plan puts nothing in one under a table.
fn table_rows(out: &mut Vec<u8>, rows: &[Row]) {
    for row in rows {
        let _ = match row {
            Row::At(position) => writeln!(out, "{position:010} 00000 n "),
            _ => writeln!(out, "0000000000 65535 f "),
        };
    }
}

/// `rows` as a cross-reference stream's data (7.5.8.3).
fn stream_rows(rows: &[Row]) -> Vec<u8> {
    let mut data = Vec::with_capacity(rows.len() * WIDTHS.iter().sum::<usize>());
    for row in rows {
        let (kind, field, last) = match *row {
            Row::Free => (0u8, 0u32, u16::MAX),
            Row::At(position) => (1, position as u32, 0),
            Row::In { stream, index } => (2, stream, index),
        };
        data.push(kind);
        data.extend_from_slice(&field.to_be_bytes());
        data.extend_from_slice(&last.to_be_bytes());
    }
    data
}

/// The numbers the first-page section lists.
fn first_rows<'r>(plan: &Plan, rows: &'r [Row]) -> &'r [Row] {
    &rows[plan.dictionary_number() as usize..]
}

/// The first-page section, as a table, its trailer and its dummy `startxref`
/// (F.3.4): what `first_rows` says, the main section at `main_at`.
fn first_table(plan: &Plan, rows: &[Row], main_at: u64, options: &Options<'_>) -> Vec<u8> {
    let rows = first_rows(plan, rows);
    let mut out = Vec::with_capacity(rows.len() * 20 + 128 + options.trailer.len());
    let _ = writeln!(out, "xref\n{} {}", plan.dictionary_number(), rows.len());
    table_rows(&mut out, rows);
    let _ = write!(
        out,
        "trailer\n<< /Size {} /Prev {main_at:<PREV$} /Root {} 0 R",
        plan.size(),
        plan.catalog_number(),
    );
    trailer_entries(&mut out, options.trailer);
    out.extend_from_slice(b" >>\n");
    out.extend_from_slice(DUMMY);
    out
}

/// The first-page section, as a stream and its dummy `startxref` (F.3.4).
fn first_stream<E>(
    plan: &Plan,
    rows: &[Row],
    main_at: u64,
    options: &Options<'_>,
    encode: &mut impl FnMut(Stream<'_>) -> Result<Encoded, E>,
) -> Result<Vec<u8>, Error<E>> {
    let rows = first_rows(plan, rows);
    let number = plan.first_xref_number().expect("a stream has one");
    let data = stream_rows(rows);
    let encoded = encode(Stream {
        what: StreamKind::FirstXref,
        number,
        data: &data,
        columns: Some(WIDTHS.iter().sum::<usize>() as u32),
    })
    .map_err(Error::Encode)?;
    let mut out = Vec::with_capacity(encoded.data.len() + 256 + options.trailer.len());
    let [a, b, c] = WIDTHS;
    let _ = write!(
        out,
        "{number} 0 obj\n<< /Type /XRef /Size {} /W [{a} {b} {c}] /Index [{} {}] /Prev {main_at} /Root {} 0 R",
        plan.size(),
        plan.dictionary_number(),
        rows.len(),
        plan.catalog_number(),
    );
    trailer_entries(&mut out, options.trailer);
    stream_body(&mut out, &encoded);
    out.extend_from_slice(DUMMY);
    Ok(out)
}

/// The main section, as a table, its trailer of only its `/Size` and a
/// `startxref` to the first-page section (F.3.11), and the dictionary's `/T`:
/// the line's end before its first row (Table F.1).
fn main_table(plan: &Plan, rows: &[Row], main_at: u64, first_at: u64) -> (Vec<u8>, u64) {
    let rows = &rows[..plan.dictionary_number() as usize];
    let mut out = Vec::with_capacity(rows.len() * 20 + 64);
    let _ = write!(out, "xref\n0 {}", rows.len());
    let t = main_at + out.len() as u64;
    out.push(b'\n');
    table_rows(&mut out, rows);
    let _ = write!(
        out,
        "trailer\n<< /Size {} >>\nstartxref\n{first_at}\n%%EOF\n",
        rows.len(),
    );
    (out, t)
}

/// The main section, as a stream and a `startxref` to the first-page section
/// (F.3.11).
fn main_stream<E>(
    plan: &Plan,
    rows: &[Row],
    first_at: u64,
    encode: &mut impl FnMut(Stream<'_>) -> Result<Encoded, E>,
) -> Result<Vec<u8>, Error<E>> {
    let rows = &rows[..plan.dictionary_number() as usize];
    let number = plan.main_xref_number().expect("a stream has one");
    let data = stream_rows(rows);
    let encoded = encode(Stream {
        what: StreamKind::MainXref,
        number,
        data: &data,
        columns: Some(WIDTHS.iter().sum::<usize>() as u32),
    })
    .map_err(Error::Encode)?;
    let mut out = Vec::with_capacity(encoded.data.len() + 128);
    let [a, b, c] = WIDTHS;
    let _ = write!(
        out,
        "{number} 0 obj\n<< /Type /XRef /Size {} /W [{a} {b} {c}] /Index [0 {}]",
        rows.len(),
        rows.len(),
    );
    stream_body(&mut out, &encoded);
    let _ = write!(out, "startxref\n{first_at}\n%%EOF\n");
    Ok(out)
}

/// A dictionary's other entries, after a space, if there are any.
fn trailer_entries(out: &mut Vec<u8>, entries: &[u8]) {
    if !entries.is_empty() {
        out.push(b' ');
        out.extend_from_slice(entries);
    }
}

/// The rest of a stream's object: `encoded`'s entries and length, closing a
/// dictionary that's been started, and its data.
fn stream_body(out: &mut Vec<u8>, encoded: &Encoded) {
    trailer_entries(out, &encoded.entries);
    let _ = write!(out, " /Length {} >>\nstream\n", encoded.data.len());
    out.extend_from_slice(&encoded.data);
    out.extend_from_slice(b"\nendstream\nendobj\n");
}

/// The hint stream as an object, its dictionary giving where each table is
/// (F.3.6, Table F.2).
fn hint_object(number: u32, keys: &HintKeys, encoded: &Encoded) -> Vec<u8> {
    let mut out = Vec::with_capacity(encoded.data.len() + 128);
    let _ = write!(out, "{number} 0 obj\n<< /S {}", keys.shared);
    let optional = [
        ("/T", keys.thumbnails),
        ("/O", keys.outlines),
        ("/A", keys.threads),
        ("/E", keys.dests),
        ("/V", keys.form),
        ("/I", keys.info),
        ("/C", keys.structure),
        ("/R", keys.renditions),
        ("/B", keys.files),
    ];
    for (key, at) in optional {
        if let Some(at) = at {
            let _ = write!(out, " {key} {at}");
        }
    }
    stream_body(&mut out, encoded);
    out
}

/// The dictionary, padded, added to `out`: its values as direct objects, all
/// but `/P` where it's the default (F.3.3, Table F.1).
fn write_dictionary(
    out: &mut Vec<u8>,
    plan: &Plan,
    len: u64,
    (hint_at, hint_len): (u64, u64),
    first_page_end: u64,
    t: u64,
) {
    let start = out.len();
    let _ = write!(
        out,
        "{} 0 obj\n<< /Linearized 1 /L {len} /H [{hint_at} {hint_len}] /O {} /E {first_page_end} /N {} /T {t}",
        plan.dictionary_number(),
        plan.first_page_number(),
        plan.page_count(),
    );
    if plan.first_page() != 0 {
        let _ = write!(out, " /P {}", plan.first_page());
    }
    out.extend_from_slice(b" >>\nendobj\n");
    debug_assert!(out.len() - start < DICTIONARY);
    out.resize(start + DICTIONARY - 1, b' ');
    out.push(b'\n');
}

/// As the tables want it: a position, which fits, as `LARGEST` has been
/// checked.
fn at32(position: u64) -> u32 {
    position as u32
}

/// The tables of a file laid out as `places` has it, its hint stream's length
/// not yet known (F.4.1), and where each is.
fn hints(plan: &Plan, places: &Places) -> Result<(Vec<u8>, HintKeys), hint::Error> {
    let mut data = Vec::new();
    let writes = plan.writes();
    let position = |at: usize| at32(places.without_hint(at));

    // In the pages' order, whichever is the first page: "in page order" as the
    // thumbnail table's items are, which starts from page 0 by name (F.4.4),
    // and as PDFium and qpdf read it. The first page's entry counts all of part
    // 6, as what it holds is read as one.
    let first = plan.first_page();
    let part6 = plan.part_writes(Part::FirstPage);
    let entry = |page: u32| {
        let section = match page == first {
            true => part6.clone(),
            false => plan.page_writes(page),
        };
        let contents = plan.page_contents(page);
        let content_at = match contents.is_empty() {
            true => 0,
            false => places.starts[contents.start] - places.starts[section.start],
        };
        PageEntry {
            objects: section.len() as u64,
            length: places.span(&section),
            shared: plan.shared(page).iter().map(|&id| (id, 0)).collect(),
            content_at,
            content_length: places.span(&contents),
        }
    };
    let pages: Vec<PageEntry> = (0..plan.page_count()).map(entry).collect();
    // The embedded file table has its identifiers in this table's width
    // (F.4.7).
    let files = (0..plan.files().len()).flat_map(|file| plan.file_shared(file));
    let most = (pages.iter().flat_map(|page| &page.shared).map(|r| r.0))
        .chain(files.copied())
        .max();
    let id_bits = hint::id_width(most.unwrap_or(0));
    PageOffsets::of(position(part6.start), pages, id_bits, 4).write(&mut data)?;

    // Part 8, or where it would be, as a reader may take a 0 here to be a
    // fault.
    let keys_shared = data.len() as u64;
    let part8 = plan.part_writes(Part::Shared);
    let first_object = match writes.get(part8.start) {
        Some(write) => write.number,
        None => plan.main_xref_number().unwrap_or(plan.dictionary_number()),
    };
    let entries = plan.entries();
    let in_first_page = (entries.iter())
        .filter(|entry| writes[entry.start].part == Part::FirstPage)
        .count();
    let groups = (entries.iter())
        .map(|entry| GroupEntry {
            length: places.span(entry),
            signature: None,
            objects: entry.len() as u64,
        })
        .collect();
    SharedObjects::of(
        first_object,
        position(part8.start),
        in_first_page as u32,
        groups,
    )
    .write(&mut data)?;
    let mut keys = HintKeys::new(keys_shared);

    let generic = |range: &std::ops::Range<usize>| match range.is_empty() {
        true => Generic::default(),
        false => Generic {
            first_object: writes[range.start].number,
            first_at: position(range.start),
            objects: range.len() as u32,
            length: at32(places.span(range)),
        },
    };

    // Each page with a thumbnail, in page order from page 0 (F.4.4).
    let mut thumbs = Vec::new();
    let mut gap = 0;
    let mut first_thumb = None;
    for page in 0..plan.page_count() {
        let range = plan.thumb_writes(page);
        if range.is_empty() {
            gap += 1;
            continue;
        }
        first_thumb.get_or_insert(range.start);
        thumbs.push(ThumbEntry {
            gap,
            objects: range.len() as u64,
            length: places.span(&range),
        });
        gap = 0;
    }
    let shared = generic(&plan.thumb_shared_writes());
    if !thumbs.is_empty() || shared.objects != 0 {
        keys.thumbnails = Some(data.len() as u64);
        let (object, at) = match first_thumb {
            Some(thumb) => (writes[thumb].number, position(thumb)),
            None => (0, 0),
        };
        Thumbnails::of(object, at, thumbs, shared).write(&mut data)?;
    }

    // The rest, where the caller gave the group, even with nothing left to it
    // (F.3.6).
    for group in Group::ALL {
        let Some(range) = plan.group_writes(group) else {
            continue;
        };
        let at = Some(data.len() as u64);
        match group {
            Group::Outlines => keys.outlines = at,
            Group::ThreadInfo => keys.threads = at,
            Group::Dests => keys.dests = at,
            Group::Info => keys.info = at,
            Group::Form => keys.form = at,
            Group::Structure => keys.structure = at,
            Group::Renditions => keys.renditions = at,
            Group::EmbeddedFile => keys.files = at,
            _ => continue,
        }
        match group {
            Group::Outlines | Group::ThreadInfo | Group::Dests | Group::Info => {
                generic(&range).write(&mut data)?;
            }
            Group::EmbeddedFile => {
                let (object, position) = (plan.files().iter())
                    .find(|file| !file.writes.is_empty())
                    .map_or((0, 0), |file| {
                        (
                            writes[file.writes.start].number,
                            position(file.writes.start),
                        )
                    });
                let groups = file_entries(plan, places);
                EmbeddedFiles::of(object, position, groups).write(&mut data, id_bits)?;
            }
            _ => {
                Extended::of(generic(&range), plan.group_shared(group).to_vec())
                    .write(&mut data)?;
            }
        }
    }
    Ok((data, keys))
}

/// The embedded file table's groups: a file's own objects, with no shared
/// objects as a group with them holds none (F.4.7), and a group of no objects
/// for what it refers to of those.
fn file_entries(plan: &Plan, places: &Places) -> Vec<FileEntry> {
    let mut entries = Vec::new();
    for (at, file) in plan.files().iter().enumerate() {
        if !file.writes.is_empty() {
            entries.push(FileEntry {
                number: file.number,
                objects: file.writes.len() as u32,
                length: at32(places.span(&file.writes)),
                shared: Vec::new(),
            });
        }
        let shared = plan.file_shared(at);
        if !shared.is_empty() {
            entries.push(FileEntry {
                number: file.number,
                objects: 0,
                length: 0,
                shared: shared.to_vec(),
            });
        }
    }
    entries
}
