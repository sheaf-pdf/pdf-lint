//! What [`check`](super::check) judges: the facts of a file as written, as raw as a
//! reader can give them, no object model's type among them. Every position is from
//! the file's first byte, as a section's rows have them. A value the file gives is
//! given as it stands, wrong or not: what's wrong with it is for the check to say.

use crate::Id;
use crate::plan::{Graph, Kind};

/// A value as a file writes it, for what a check wants to know of one.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Raw {
    /// There's no such key.
    #[default]
    Absent,
    /// An integer.
    Integer(i64),
    /// A real number.
    Real(f64),
    /// A reference, or a value with one in it.
    Reference(Id),
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
    /// Its number.
    pub number: u32,
    /// Its generation.
    pub generation: u16,
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
    /// An object `number` and `generation` from `at` to `end`, with none of the
    /// keys yet.
    pub fn new(at: u64, end: u64, number: u32, generation: u16) -> FirstObject {
        FirstObject {
            at,
            end,
            number,
            generation,
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
    pub fn new(number: u32, generation: u16, entry: RowEntry) -> RowFacts {
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
    /// Where what it lists first is, a table's first row or its stream's object, after
    /// where the white-space before that starts.
    pub first_row: Option<(u64, u64)>,
    /// The number of the stream it's written as.
    pub stream: Option<u32>,
    /// `/Size`.
    pub size: Raw,
    /// `/Prev`.
    pub prev: Raw,
    /// `/Root`.
    pub root: Raw,
    /// What the `startxref` after it gives, if one does.
    pub startxref: Option<u64>,
    /// What a hybrid table's `/XRefStm` lists (7.5.8.4): none of a file that has
    /// none, and what's free in the table isn't free where it's in use here.
    pub hybrid: Vec<RowFacts>,
}

impl SectionFacts {
    /// A section at `at`, written as `kind`, of no rows and no keys yet.
    pub fn new(at: u64, kind: Kind) -> SectionFacts {
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

/// A hint stream's data, or why there isn't any.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HintData {
    /// What it decodes to, an overflow stream's after the primary's.
    Decoded(Vec<u8>),
    /// A reader has no key for it, so there's nothing to say of what it holds.
    Locked,
    /// What's at `/H` isn't a stream.
    NotAStream,
    /// It doesn't decode.
    Undecodable,
}

/// The primary hint stream.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct HintFacts {
    /// Where its `n g obj` starts.
    pub at: u64,
    /// Where it ends: past its `endobj`, or an indirect `/Length`'s object's that
    /// follows it (Table F.1).
    pub end: u64,
    /// Its number.
    pub number: u32,
    /// Its dictionary's keys, and what each says, as written.
    pub keys: Vec<(Vec<u8>, Raw)>,
    /// An overflow stream's place and end, where `/H` names one.
    pub overflow: Option<(u64, u64)>,
    /// Its data.
    pub data: HintData,
}

impl HintFacts {
    /// Hint stream `number` from `at` to `end`, holding `data`, with no keys yet.
    pub fn new(at: u64, end: u64, number: u32, data: HintData) -> HintFacts {
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

/// An attribute a page inherits (7.7.3.4), and which a linearized file has pushed
/// down onto its pages (F.3.7, F.3.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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

/// A page, as the page tree gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PageFacts {
    /// Its object, where it has one of its own.
    pub id: Option<Id>,
    /// Whether it says `/Resources` itself.
    pub resources: bool,
    /// Whether it says `/MediaBox` itself.
    pub media_box: bool,
}

impl PageFacts {
    /// The page at `id`, saying neither `/Resources` nor `/MediaBox` itself.
    pub fn new(id: Option<Id>) -> PageFacts {
        PageFacts {
            id,
            resources: false,
            media_box: false,
        }
    }
}

/// Where an object is, where a reader has read it. An object no [`Extent`] names
/// ends where the next thing a section lists starts, which is as good but for bytes
/// between objects, and for the end of what `/E` should end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Extent {
    /// Its number.
    pub number: u32,
    /// Where its `n g obj` starts.
    pub start: u64,
    /// Where it ends: past its `endobj`.
    pub end: u64,
}

impl Extent {
    /// Object `number`, from `start` to `end`.
    pub fn new(number: u32, start: u64, end: u64) -> Extent {
        Extent { number, start, end }
    }
}

/// The facts of a file as written, set on [`Facts::new`]'s: what isn't given isn't
/// judged, or is said to be missing where a linearized file has to have it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Facts {
    /// The file's length.
    pub len: u64,
    /// Where `%PDF-` starts.
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
    /// Each page tree node's number and what it holds that its pages inherit, once
    /// for each key.
    pub nodes: Vec<(u32, Attribute)>,
    /// What's read of where objects end. Any may be left out.
    pub objects: Vec<Extent>,
    /// The file's objects as a plan would take them, by the numbers they're written
    /// at: what's then said of what each page reaches.
    pub graph: Option<Graph>,
}

impl Facts {
    /// A file `len` bytes long, its header first, of which nothing else is known.
    pub fn new(len: u64) -> Facts {
        Facts {
            len,
            header: 0,
            first_object: None,
            first: None,
            main: None,
            update: None,
            hint: None,
            pages: Vec::new(),
            nodes: Vec::new(),
            objects: Vec::new(),
            graph: None,
        }
    }
}
