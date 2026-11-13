//! What [`check`](super::check) judges: the facts of a file as written, as raw
//! as a reader can give them, no document model's type among them, an object by
//! its pdf-objects [`ObjectId`]. Every position is from the file's first byte,
//! as a section's rows have them. A value the file gives is given as it stands,
//! wrong or not: what's wrong with it is for the check to say. The offsets the
//! file writes (the linearization dictionary's, the hint tables') count from
//! its header (ISO 32000-2, 7.5.2), so the check holds each against a position
//! [`Facts::header`] bytes past it.

use std::fmt;

use super::KeyName;
use crate::plan::{Graph, Kind};
use pdf_objects::ObjectId;

/// A value as a file writes it, for what a check wants to know of one.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub enum Raw {
    /// There's no such key.
    #[default]
    Absent,
    /// An integer.
    Integer(i64),
    /// A real number.
    Real(f64),
    /// A reference, or a value with one in it.
    Reference(ObjectId),
    /// An array, with no reference in it.
    Array,
    /// Anything else.
    Other,
}

/// The file's first object, after its header.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FirstObject {
    /// Where its `n g obj` starts.
    pub at: u64,
    /// Where it ends: past its `endobj`.
    pub end: u64,
    /// Its number and generation.
    pub id: ObjectId,
    /// Whether it's a stream, whose dictionary is no linearization dictionary.
    pub stream: bool,
    /// `/Linearized`.
    pub linearized: Raw,
    /// `/L`.
    pub l: Raw,
    /// `/H`, as a whole.
    pub h: Raw,
    /// `/H`'s items, where it's an array.
    pub h_items: Vec<Raw>,
    /// `/O`.
    pub o: Raw,
    /// `/E`.
    pub e: Raw,
    /// `/N`.
    pub n: Raw,
    /// `/T`.
    pub t: Raw,
    /// `/P`.
    pub p: Raw,
}

impl FirstObject {
    /// Whether its `/Linearized` is a number whose integer part is 1, the
    /// version the check takes (Table F.1): where it isn't, the check tells
    /// [`Finding::Version`](super::Finding::Version), and the file isn't
    /// taken as linearized.
    #[must_use]
    pub fn version_one(&self) -> bool {
        match self.linearized {
            Raw::Integer(version) => version == 1,
            Raw::Real(version) => version.floor() == 1.0,
            _ => false,
        }
    }

    /// Object `id` from `at` to `end`, with none of the keys yet.
    #[must_use]
    pub const fn new(at: u64, end: u64, id: ObjectId) -> FirstObject {
        FirstObject {
            at,
            end,
            id,
            stream: false,
            linearized: Raw::Absent,
            l: Raw::Absent,
            h: Raw::Absent,
            h_items: Vec::new(),
            o: Raw::Absent,
            e: Raw::Absent,
            n: Raw::Absent,
            t: Raw::Absent,
            p: Raw::Absent,
        }
    }
}

/// What a section's row says of its number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RowEntry {
    /// Free.
    Free,
    /// In use, written at this position.
    InUse(u64),
    /// In this object stream, at this place in it.
    Compressed {
        /// The object stream's number.
        stream: u32,
        /// Which of its objects it is.
        index: u32,
    },
    /// A stream's row of a type no reader knows, which reads as null (7.5.8.3).
    Null,
}

/// One row of a section, as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RowFacts {
    /// The number it's for.
    pub number: u32,
    /// Its generation.
    pub generation: u16,
    /// What it says.
    pub entry: RowEntry,
}

impl RowFacts {
    /// A row of `number`, at `generation`, saying `entry`.
    #[must_use]
    pub const fn new(number: u32, generation: u16, entry: RowEntry) -> RowFacts {
        RowFacts {
            number,
            generation,
            entry,
        }
    }
}

/// A cross-reference section, as written.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SectionFacts {
    /// Where it starts: `xref`, or its stream's `n g obj`.
    pub at: u64,
    /// How it's written.
    pub kind: Kind,
    /// Its subsections: each's first number, and how many rows.
    pub subsections: Vec<(u32, u32)>,
    /// Its rows, in the order written.
    pub rows: Vec<RowFacts>,
    /// Where the white-space before what it lists first starts, and then
    /// where that is, a table's first row or its stream's `n g obj`, in that
    /// order.
    pub first_row: Option<(u64, u64)>,
    /// The number of the stream it's written as.
    pub stream: Option<u32>,
    /// `/Size`.
    pub size: Raw,
    /// `/Prev`.
    pub prev: Raw,
    /// `/Root`.
    pub root: Raw,
    /// Where the `startxref` after it says the newest section is, if one
    /// follows it.
    pub startxref: Option<u64>,
    /// What a hybrid table's `/XRefStm` lists (7.5.8.4): none of a file that
    /// has none, and what's free in the table isn't free where it's in use
    /// here.
    pub hybrid: Vec<RowFacts>,
}

impl SectionFacts {
    /// A section at `at`, written as `kind`, of no rows and no keys yet.
    #[must_use]
    pub const fn new(at: u64, kind: Kind) -> SectionFacts {
        SectionFacts {
            at,
            kind,
            subsections: Vec::new(),
            rows: Vec::new(),
            first_row: None,
            stream: None,
            size: Raw::Absent,
            prev: Raw::Absent,
            root: Raw::Absent,
            startxref: None,
            hybrid: Vec::new(),
        }
    }
}

/// Why a reader didn't read a part of a file, where that's the reader's and
/// not the file's: what the part says isn't known, so the check judges
/// nothing that depends on it, and tells a finding that says so
/// ([`Grade::Unjudged`](super::Grade::Unjudged)). What the file gets wrong
/// isn't one of these: a part that doesn't read for that is told as the
/// file's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Unread {
    /// No key the reader has opens it: no password it was given opens the
    /// file, or the file is encrypted by a security handler it can't open.
    Locked,
    /// It's past a cap the reader reads under: a stream's data longer than
    /// it decodes, more filters or sections or rows than it reads, a page
    /// tree deeper or of more pages than it walks.
    Capped,
    /// It's through a filter the reader doesn't decode, or with a `/Crypt`
    /// filter's parameter it doesn't take. A `/Filter` or `/DecodeParms` that
    /// isn't what ISO 32000-2 has them be is the file's, and not this.
    Unfiltered,
}

impl std::fmt::Display for Unread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Unread::Locked => "no key the reader has opens it",
            Unread::Capped => "it's past a cap the reader reads under",
            Unread::Unfiltered => "it's through filters the reader doesn't take",
        })
    }
}

/// A hint stream's data, or why there isn't any. Its `Debug` gives the data
/// by its length alone: a hint stream's data decrypted is plaintext.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HintData {
    /// What it decodes to, an overflow stream's after the primary's.
    Decoded(Vec<u8>),
    /// What's at `/H` isn't a stream.
    NotAStream,
    /// It doesn't decode.
    Undecodable,
    /// The reader didn't read it, and why: nothing's said of what it holds.
    /// Where the stream's own object didn't read, its keys are none and its
    /// end is where it starts, and nothing's said of its dictionary or of
    /// its extent either.
    Unread(Unread),
}

impl fmt::Debug for HintData {
    /// What it is, and the data by its length, not its bytes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HintData::Decoded(data) => (f.debug_tuple("Decoded"))
                .field(&format_args!("{} bytes", data.len()))
                .finish(),
            HintData::NotAStream => f.write_str("NotAStream"),
            HintData::Undecodable => f.write_str("Undecodable"),
            HintData::Unread(why) => f.debug_tuple("Unread").field(why).finish(),
        }
    }
}

/// The primary hint stream. Its `Debug` gives each key by its name, as a
/// [`KeyName`] shows it, and the data as [`HintData`]'s does, by its length.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub struct HintFacts {
    /// Where its `n g obj` starts: `/H`'s first offset, past the header, where
    /// that lists it.
    pub at: u64,
    /// Where it ends: past its `endobj`, or an indirect `/Length`'s object's
    /// that follows it (Table F.1).
    pub end: u64,
    /// Its number.
    pub number: u32,
    /// Its dictionary's keys, and what each says, as written.
    pub keys: Vec<(Vec<u8>, Raw)>,
    /// An overflow stream's place and end, where `/H` names one: its third
    /// item, past the header.
    pub overflow: Option<(u64, u64)>,
    /// Its data.
    pub data: HintData,
}

impl HintFacts {
    /// Hint stream `number` from `at` to `end`, holding `data`, with no keys
    /// yet.
    #[must_use]
    pub const fn new(at: u64, end: u64, number: u32, data: HintData) -> HintFacts {
        HintFacts {
            at,
            end,
            number,
            keys: Vec::new(),
            overflow: None,
            data,
        }
    }
}

impl fmt::Debug for HintFacts {
    /// Its fields, each key by its name and its value, and the data by its
    /// length.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let keys = self.keys.iter().map(|(key, raw)| (KeyName::new(key), raw));
        f.debug_struct("HintFacts")
            .field("at", &self.at)
            .field("end", &self.end)
            .field("number", &self.number)
            .field("keys", &keys.collect::<Vec<_>>())
            .field("overflow", &self.overflow)
            .field("data", &self.data)
            .finish()
    }
}

/// An attribute a page inherits (7.7.3.4), and which a linearized file has
/// pushed down onto its pages (F.3.7, F.3.10). Its `Display` is its key,
/// `/MediaBox`; [`Attribute::ALL`] lists them. Exhaustive on purpose, as
/// 7.7.3.4's list is closed and `ALL`'s length is the count of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Attribute {
    /// `/Resources`.
    Resources,
    /// `/MediaBox`.
    MediaBox,
    /// `/CropBox`.
    CropBox,
    /// `/Rotate`.
    Rotate,
}

impl Attribute {
    /// Every attribute, in the order they're declared, so a table indexed by
    /// them can take its length from here (`Attribute::ALL.len()`).
    pub const ALL: [Attribute; 4] = [
        Attribute::Resources,
        Attribute::MediaBox,
        Attribute::CropBox,
        Attribute::Rotate,
    ];

    /// Its key in a page's or a node's dictionary, without the slash:
    /// `b"MediaBox"`.
    #[must_use]
    pub const fn key(self) -> &'static [u8] {
        match self {
            Attribute::Resources => b"Resources",
            Attribute::MediaBox => b"MediaBox",
            Attribute::CropBox => b"CropBox",
            Attribute::Rotate => b"Rotate",
        }
    }
}

impl fmt::Display for Attribute {
    /// Its key, with the slash: `/MediaBox`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Attribute::Resources => "/Resources",
            Attribute::MediaBox => "/MediaBox",
            Attribute::CropBox => "/CropBox",
            Attribute::Rotate => "/Rotate",
        })
    }
}

/// A page, as the page tree gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PageFacts {
    /// Its object, where it has one of its own.
    pub id: Option<ObjectId>,
    /// Whether it says `/Resources` itself.
    pub resources: bool,
    /// Whether it says `/MediaBox` itself.
    pub media_box: bool,
}

impl PageFacts {
    /// The page at `id`, saying neither `/Resources` nor `/MediaBox` itself.
    #[must_use]
    pub const fn new(id: Option<ObjectId>) -> PageFacts {
        PageFacts {
            id,
            resources: false,
            media_box: false,
        }
    }
}

/// Where an object is, where a reader has read it. An object no [`Extent`]
/// names ends where the next thing a section lists starts, which is as good but
/// for bytes between objects, and for the end of what `/E` should end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Extent {
    /// Its number.
    pub number: u32,
    /// Where its `n g obj` starts.
    pub at: u64,
    /// Where it ends: past its `endobj`.
    pub end: u64,
}

impl Extent {
    /// Object `number`, from `at` to `end`.
    #[must_use]
    pub const fn new(number: u32, at: u64, end: u64) -> Extent {
        Extent { number, at, end }
    }
}

/// What of a file's sections and its page tree the reader didn't read, and
/// why ([`Unread`]): none of either where it read, or where what didn't is
/// the file's to answer for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub struct UnreadParts {
    /// The cross-reference sections, where reading them stopped short of the
    /// main one: what's listed past where it stopped isn't known, nor where
    /// anything the sections place is, nor so what the page tree's objects
    /// are, nor whether the pages it gives are all it has.
    pub sections: Option<Unread>,
    /// The page tree: its pages, where it's said, are none, and aren't
    /// judged. The `family` feature's facts say it of no key and of a cap, not
    /// of a filter: a node in an object stream the reader doesn't decode reads
    /// as damaged, as the reader gives it.
    pub pages: Option<Unread>,
}

/// The facts of a file as written, set on [`Facts::new`]'s: what isn't given
/// isn't judged, or is said to be missing where a linearized file has to have
/// it; but what the reader didn't read, for its own reason, is said in
/// [`Facts::unread`] and [`HintData::Unread`], and isn't judged. Two
/// readings' facts compare by `==`, field by field; a `Debug` holds no byte
/// of the hint data.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Facts {
    /// The file's length.
    pub len: u64,
    /// Where `%PDF-` starts: the offsets the file writes count from there.
    pub header: u64,
    /// Its first object after that.
    pub first_object: Option<FirstObject>,
    /// The section `startxref` gives: the first-page section.
    pub first: Option<SectionFacts>,
    /// The one its `/Prev` gives: the main section.
    pub main: Option<SectionFacts>,
    /// Where the newest section is, where it's an update: one written after the
    /// section it follows (G.7).
    pub update: Option<u64>,
    /// The primary hint stream: none where nothing's listed at `/H`.
    pub hint: Option<HintFacts>,
    /// Each page, in page order: none where the page tree didn't read.
    pub pages: Vec<PageFacts>,
    /// What of the sections and the page tree the reader didn't read, for its
    /// own reason, and why.
    pub unread: UnreadParts,
    /// Each page tree node's number and what it holds that its pages inherit,
    /// once for each key.
    pub nodes: Vec<(u32, Attribute)>,
    /// What's read of where objects end. Any may be left out.
    pub objects: Vec<Extent>,
    /// The file's objects as a plan would take them, by the numbers they're
    /// written at: what's then said of what each page reaches.
    pub graph: Option<Graph>,
}

impl Facts {
    /// A file `len` bytes long, its header first, of which nothing else is
    /// known.
    #[must_use]
    pub const fn new(len: u64) -> Facts {
        Facts {
            len,
            header: 0,
            first_object: None,
            first: None,
            main: None,
            update: None,
            hint: None,
            pages: Vec::new(),
            unread: UnreadParts {
                sections: None,
                pages: None,
            },
            nodes: Vec::new(),
            objects: Vec::new(),
            graph: None,
        }
    }

    /// Its first object, where it says the file is linearized as the check
    /// takes a claim before it tells anything: a dictionary, not a stream,
    /// with `/Linearized`, starting within 1024 bytes of the header. Of a
    /// file with none, the check tells nothing.
    #[must_use]
    pub fn claim(&self) -> Option<&FirstObject> {
        super::judge::claim(self)
    }

    /// Where the primary hint stream is and how long, as the check has a
    /// hint table's positions as if it weren't there, the extent
    /// [`hint::located`](crate::hint::located) gives a position's byte by:
    /// `/H`'s first two items, each 0 where it isn't a non-negative integer,
    /// the first past the header;
    /// but where the stream listed at that offset, its object read, takes
    /// fewer bytes than that length, or more than it may, to where what's
    /// listed after it starts, the most it may, as the check then tells
    /// `/H`'s length once
    /// ([`Finding::HintExtent`](super::Finding::HintExtent)) and places the
    /// rest by the stream that's there.
    #[must_use]
    pub fn hint_extent(&self) -> (u64, u64) {
        super::judge::hint_extent(self)
    }
}
