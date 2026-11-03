//! Content, read and checked (ISO 32000-2, 7.8.2): every content stream the
//! walk reached, as what holds it: a page's `/Contents`, its streams read as
//! one (7.7.3.3, Table 31), a form XObject's or a tiling pattern's own data
//! (8.10, 8.7.3), and each glyph description of a Type 3 font's
//! `/CharProcs` (9.6.4), each alone. What's content is what the walk reached
//! as one of those tables, the first table it reached the object as, so an
//! object the walk reached as another isn't read as content, nor one it
//! never reached.
//!
//! Each unit of content is read once: its streams, in order, the resources
//! its names are looked up in, and where it starts, a glyph at its `d0` or
//! `d1`. A stream two pages share, with the same resources, is read once.
//! The resources are a page's own or inherited `/Resources` (7.7.3.4), a
//! form's or a pattern's own, and a glyph's own, else its font's (7.8.3, as
//! its errata have it).
//!
//! The streams' data comes decoded from the source
//! ([`Source::decoded_within`]): a reader that gives none, as a document
//! alone doesn't, has no content read at all, as it says of the first
//! stream it's asked for. A file's content is read within a budget of
//! decoded bytes ([`Options::content_budget`]), each stream asked for no
//! more than is left of it, what a stream that doesn't decode cleanly held
//! spent too, so that a bomb cut short spends it as one read whole does;
//! past it, no more is read. A unit whose data doesn't decode cleanly, or
//! that the source doesn't give, or past the budget, isn't read, and is
//! counted as not checked ([`Report::unchecked`]), its column `-`: a part of
//! it would find what the rest undoes. Each unit is read as PDFium reads
//! content, through pdf-content-streams, with every place ISO 32000-2 reads
//! it otherwise kept as it's read.
//!
//! What a unit's reading finds, each kind of it once in each of the unit's
//! streams it's found in, at its first place there, with how many times it's
//! found there, and past [`KINDS`] kinds of a rule in the unit, one finding
//! more saying how many more:
//!
//! - **`q`s left open** at the end, and a **`Q` with nothing saved** to
//!   restore (8.4.2; [`Rule::Balance`]): counted by how deep the saved
//!   states are, kept at 0 when a `Q` comes with none saved, so that the
//!   first `q` left open is the last taken with none saved before it, as
//!   pdf-content-streams' `Balance` counts them; and of each stream, those
//!   it leaves open, the first of them the last it took at the lowest it
//!   went, as deep as no later stream goes below.
//! - **Marked content left open** at the end, `BMC` or `BDC`, and an **`EMC`
//!   ending none** (14.6.1; [`Rule::Marked`]), counted the same way, every
//!   `BMC` and `BDC` opening a range, however deep, as its `MarkedNest` reads
//!   them below its cap on depth.
//! - **An operator where content is that it may not be used in**, among
//!   the graphics objects of 8.2's Figure 9 ([`Rule::Operator`]), each
//!   operation stepping pdf-content-streams' `State`, a glyph from where its
//!   `d0` or `d1` must be (9.6.4); a **word Annex A doesn't name**, outside
//!   `BX` and `EX`, inside which it's passed over (7.8.2), each word a kind
//!   of its own; an **`EX` with no `BX` open**, and **`BX` left open** at the
//!   end, counted as `q` is; and **content that ends inside a graphics
//!   object**, at the operation that began it, or a glyph description with
//!   no `d0` or `d1`, all of it. After an operator that's refused, content
//!   is where it was, but that a glyph description goes on at the content
//!   stream level, the operator taken there, as after its `d0`; `ET` ends
//!   what it's in; and `BT`, `m` or `re` in a path object begins its own
//!   object, the path abandoned: a rule of the checker's, so that one
//!   mistake is one finding, and what's refused after it is only said to
//!   be.
//! - **Operands that aren't as Annex A's table has them**
//!   ([`Rule::Operands`]): how many, else the first of them whose type
//!   isn't its operator's, told from its bytes as written, an integer a
//!   number without a point; `SC` and `sc` 1, 3 or 4 numbers, as many as a
//!   space they can set has components, and `SCN` and `scn` one at least,
//!   numbers but a pattern's name last. The operations are PDFium's, so a
//!   closer that closes nothing, an operand to PDFium, is counted.
//! - **A resource name its resources lack** ([`Rule::Resource`]): the
//!   name an operation that names one uses, as pdf-content-streams'
//!   `resource` tells (`Tf`'s font, `Do`'s XObject, a color space but the
//!   device spaces and `/Pattern`, ...), as written, read as ISO 32000-2
//!   reads a name (7.3.5), as the resources' keys are read, looked up in
//!   its category (7.8.3). The resources are read through any references,
//!   and a category that isn't there, or is null, lacks every name, as
//!   does a page with no resources at all; a name whose value is null
//!   isn't there, and one that refers to nothing is, the walk finding the
//!   reference. Where what a name is in can't be told, a form's, a
//!   pattern's or a glyph's with no resources of its own, which would use
//!   its page's, or resources or a category that isn't a dictionary or
//!   refers to nothing, no name lacks, and the unit is counted as not
//!   checked. Each name once, at its first use, as written: an inline
//!   image's `/I` is `I`, which PDFium looks up as `Indexed`.
//! - **Each place ISO 32000-2 reads the content otherwise** than PDFium does
//!   ([`Rule::Content`]), by the kind of [`Fault`], its text the first's: the
//!   tokens' and the operations' own, 0x80 and 0xFF read as white-space
//!   among them, and what's inside an array or a dictionary among the
//!   operands, which reading it as an object tells. A dictionary PDFium
//!   reads as none is told twice, so: as no dictionary, by the tokens, and
//!   as what makes it none (a key that's no name, a key with no value, a
//!   word that's no object), by its reading.
//! - **What stops reading** ([`Rule::Content`]): a string, an array, a
//!   dictionary or an inline image that doesn't end, operands past the cap,
//!   and the like. What came before it stands, but where the content ends
//!   isn't known, so nothing left open at the end is found, nor content
//!   ending inside an object.
//!
//! A finding is in the stream its first byte is in, at offsets in that
//! stream's decoded data, cut at its end; at the path its holder reached it
//! by, under a page `Contents` or `Contents[1]`, under a font `CharProcs`
//! and the glyph's name; its table the holder's, and its key `Contents`,
//! `-` for a form's or a pattern's own data, or the glyph's name. A unit is
//! held when a holder of it is, or one of its streams, or what its
//! resources are written in (its `/Resources`, the page tree node a page
//! inherits them from, a category's own object), and is then told as the
//! first held holder's, else as the first's, all of it: made as that holder
//! is, else as the stream its finding is in, else as the first held stream,
//! else as its resources. So what an update writes is told with the unit
//! it's in, and [`Report::since`] carries what the revision before had in
//! the same stream, and what a stream the update wrote brings is its own.
//! Units come in the order the walk reached their first holders;
//! in a unit, the rules in [`Rule::ALL`]'s order, and each rule's kinds by
//! where they're first found. A `resource` finding's table is `Resource`
//! and its key the category.
//!
//! Then, of the categories of resources the units have, each once, as
//! they're first met, and in each its entries' order, the names no unit
//! uses ([`Rule::Unused`]), but `DefaultGray`, `DefaultRGB` and
//! `DefaultCMYK`, used without being named (8.6.5.6), and those content
//! whose resources can't be told uses, which may be any category's. A
//! category is one by the object it is, else by its resources' object and
//! its key, else by where it was written, so that a name any unit uses is
//! used; one a unit of which isn't read, or isn't read to its end, can't be
//! told, nor can any where such a unit's names could be in any (its
//! resources, or a category of them, can't be told), and what it holds that
//! nothing read uses is counted as not checked. Each counted
//! ([`Report::unused`]), and found with [`Options::unused`]: in the
//! category's object, else its resources', else the one they're in, where
//! that's held, or where a held unit has it, made as that is; at the walk's
//! way to the resources and the name's key.
//!
//! [`Options::content_budget`]: super::Options::content_budget
//! [`Options::unused`]: super::Options::unused
//! [`Report::since`]: super::Report::since
//! [`Report::unchecked`]: super::Report::unchecked
//! [`Report::unused`]: super::Report::unused
//! [`Rule::ALL`]: super::Rule::ALL
//! [`Rule::Balance`]: super::Rule::Balance
//! [`Rule::Content`]: super::Rule::Content
//! [`Rule::Marked`]: super::Rule::Marked
//! [`Rule::Operands`]: super::Rule::Operands
//! [`Rule::Operator`]: super::Rule::Operator
//! [`Rule::Resource`]: super::Rule::Resource
//! [`Rule::Unused`]: super::Rule::Unused
//! [`Source::decoded_within`]: crate::source::Source::decoded_within

use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::mem;
use std::ops::Range;
use std::ptr;
use std::rc::Rc;

use pdf_content_streams::fault::{Fault, Sink};
use pdf_content_streams::{
    CodedEnds, Concat, Error, Join, Limits, Operand, Operands, Operation, Operator, ResCx, ResKind,
    State, Type,
};

use super::{Finding, Made, Rule, Step, TOKEN, Trail, Visit, Walk};
use crate::bytes;
use crate::object::{Dict, Object, ObjectId, Span, Value};
use crate::pdf_filters;

/// How many objects are looked up for one color space an inline image is
/// drawn in, and how many subfunctions written in place are read, where
/// pdf-color reads 4,096: the cost of sizing a resources dictionary's
/// spaces is its entries times this, which a file can make its own, so the
/// checker reads with less. A space past it is sized as one that doesn't
/// load, and an unfiltered image drawn in it may end where PDFium's
/// doesn't; no honest space comes near it.
const SPACE_VISITS: usize = 256;

/// How many kinds of what a rule finds a unit's findings tell apart: past
/// them, one more finding says how many more there are, so that what
/// hostile content makes of a report is bounded.
const KINDS: usize = 32;

/// Why a unit isn't read: the file's budget of decoded content is spent.
const BUDGET: &str = "content past the file's budget of decoded content";

/// Why a unit isn't read: its streams decode, or join, past what content's
/// reading takes of one unit.
const TOO_LARGE: &str = "content decoded past what's read of one unit";

/// Why a unit isn't read: a stream of it doesn't decode cleanly.
const DECODE: &str = "a content stream that doesn't decode cleanly";

/// Why a unit isn't read: the source gives no data of a stream of it,
/// though it gave another's.
const UNGIVEN: &str = "a content stream whose data the source doesn't give";

/// Why a resource name isn't looked up: a form, a pattern or a glyph has no
/// resources of its own, where it would use its page's.
const UNOWNED: &str = "a resource name in content with no resources of its own";

/// Why a resource name isn't looked up: its unit's resources aren't a
/// dictionary.
const DAMAGED: &str = "a resource name in resources that aren't a dictionary";

/// Why a resource name isn't looked up: its category isn't a dictionary, or
/// refers to nothing.
const CATEGORY: &str = "a resource name in a category that isn't a dictionary";

/// Why a resource isn't told used or not: content that has it among its
/// resources isn't read, or isn't read to its end.
const UNTOLD: &str = "a resource content not read may use";

/// The color spaces content uses without naming them, whatever space it
/// sets (ISO 32000-2, 8.6.5.6): never unused.
const DEFAULTS: &[&[u8]] = &[b"DefaultGray", b"DefaultRGB", b"DefaultCMYK"];

/// What the walk reached that holds content, kept for the content pass
/// ([`Walk::content`]), in the order the walk reached them.
pub(super) struct Holder<'a> {
    /// A page's dictionary, a form's or a pattern's stream, or a Type 3
    /// font's dictionary.
    object: &'a Object,
    /// The indirect object it is, or is in, which holds it ([`Walk::made`]).
    holder: Option<ObjectId>,
    /// The table the walk reached it as.
    table: &'static str,
    /// The walk's way to it.
    trail: Rc<Trail<'a>>,
    kind: Kind,
}

/// What a holder's content is.
#[derive(Debug, Clone, Copy)]
enum Kind {
    /// A page's: the streams of its `/Contents`, as one.
    Page,
    /// A form XObject's or a tiling pattern's own data, the stream it is.
    Own(ObjectId),
    /// A Type 3 font's: each glyph description in its `/CharProcs`, alone.
    Glyphs,
}

/// How a unit's stream is reached from its holder, which a finding in it
/// names.
#[derive(Debug, Clone, Copy)]
enum Way<'a> {
    /// It's the holder: a form's or a pattern's own data.
    Own,
    /// The holder's `/Contents`.
    Contents,
    /// An element of the holder's `/Contents`, an array, by its place in it.
    Element(usize),
    /// An entry of the holder's `/CharProcs`, by its glyph's name.
    Glyph(&'a [u8]),
}

/// Content to read as one: its streams, in order, each with how its holder
/// reaches it, the resources its names are looked up in, and where it
/// starts.
struct Unit<'a> {
    streams: Vec<(ObjectId, Way<'a>)>,
    resources: Option<Resources<'a>>,
    start: State,
}

/// A unit's resources: the value they are, their references followed, and
/// the indirect object it is, if it's one.
#[derive(Clone, Copy)]
struct Resources<'a> {
    id: Option<ObjectId>,
    object: &'a Object,
}

/// Where the walk reached a resources dictionary, as it's first reached as
/// one: the indirect object it is or is in, and the way to it.
pub(super) struct Reached<'a> {
    holder: Option<ObjectId>,
    trail: Rc<Trail<'a>>,
}

/// Where a unit's resource names are looked up.
enum Lookup {
    /// In none: a page with no resources, which lacks every name.
    Without,
    /// In none that can be told, and why.
    Untold(&'static str),
    /// In each category its resources have, by kind: one of
    /// [`Categories`]', or why its names can't be told. A category not
    /// among them, or null, lacks every name.
    In(Vec<(ResKind, Result<usize, &'static str>)>),
}

/// What a resource name content uses is, looked up in its unit's resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Looked {
    There,
    /// Not in its category.
    Lacking,
    /// Not there, as the unit has no resources at all.
    Without,
    /// Not told, as its resources or its category can't be.
    Untold,
}

/// What tells one category of resources from another, for `unused`: the
/// object it is; else its resources' object and its key; else where it was
/// written. One that has none of these, built in memory, isn't told.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Identity {
    Object(ObjectId),
    Key(ObjectId, ResKind),
    At(Span),
}

/// A dictionary of a category of resources, as content's names are looked
/// up in it and `unused` tells it.
struct Category<'a> {
    kind: ResKind,
    dict: &'a Dict,
    /// What it is, if that can be told.
    identity: Option<Identity>,
    /// The object a finding of it is in.
    object: Option<ObjectId>,
    /// The way to it: its resources', then its key.
    trail: Rc<Trail<'a>>,
    /// Each name it holds whose value isn't null, and whether content read
    /// uses it.
    used: HashMap<&'a [u8], bool>,
    /// Whether every unit of content it's a category of was read to its
    /// end, so that what isn't used is told.
    told: bool,
    /// How it's made where its object isn't held but a unit of content
    /// that has it is: as that unit is, as what the unit's content uses
    /// is what's unused in it.
    made: Option<Made>,
}

/// The categories of resources the units of a file's content name, as
/// first met, each once.
#[derive(Default)]
struct Categories<'a> {
    all: Vec<Category<'a>>,
    by: HashMap<Identity, usize>,
    /// Every name the categories of each kind hold.
    names: HashMap<ResKind, HashSet<&'a [u8]>>,
    /// Those of them that content whose resources can't be told uses: used
    /// in every category of the kind, as which it's looked up in isn't
    /// known.
    spared: HashMap<ResKind, HashSet<&'a [u8]>>,
}

impl<'a> Categories<'a> {
    /// The category `identity` tells, made by `make` if it's new, as one
    /// none tells always is.
    fn add(&mut self, identity: Option<Identity>, make: impl FnOnce() -> Category<'a>) -> usize {
        if let Some(i) = identity.and_then(|identity| self.by.get(&identity)) {
            return *i;
        }
        let category = make();
        let names = self.names.entry(category.kind).or_default();
        names.extend(category.used.keys().copied());
        let i = self.all.len();
        if let Some(identity) = identity {
            self.by.insert(identity, i);
        }
        self.all.push(category);
        i
    }

    /// `name`, of `kind`, used by content whose resources can't be told:
    /// used wherever a category of its kind holds it.
    fn spare(&mut self, kind: ResKind, name: &[u8]) {
        if let Some(&held) = self.names.get(&kind).and_then(|names| names.get(name)) {
            self.spared.entry(kind).or_default().insert(held);
        }
    }

    /// What `name`, of `kind`, is in `lookup`, marked used where it's
    /// there; and why it can't be told, where it can't, kept in `untold`.
    fn look(
        &mut self,
        lookup: &Lookup,
        kind: ResKind,
        name: &[u8],
        untold: &mut Vec<&'static str>,
    ) -> Looked {
        let found = match lookup {
            Lookup::Without => return Looked::Without,
            Lookup::Untold(why) => Err(*why),
            Lookup::In(categories) => match categories.iter().find(|(k, _)| *k == kind) {
                None => return Looked::Lacking,
                Some((_, found)) => *found,
            },
        };
        match found {
            Ok(i) => match self.all[i].used.get_mut(name) {
                Some(used) => {
                    *used = true;
                    Looked::There
                }
                None => Looked::Lacking,
            },
            Err(why) => {
                if !untold.contains(&why) {
                    untold.push(why);
                }
                self.spare(kind, name);
                Looked::Untold
            }
        }
    }

    /// The categories of `lookup` not told, as a unit of content they're
    /// the categories of isn't read to its end; and where what it names can
    /// be in none that can be told, its resources not, or a category of
    /// them not, every category of what it may name, as which it would use
    /// isn't known.
    fn untell(&mut self, lookup: &Lookup) {
        match lookup {
            Lookup::Without => {}
            Lookup::Untold(_) => self.untell_all(None),
            Lookup::In(categories) => {
                for &(kind, found) in categories {
                    match found {
                        Ok(i) => self.all[i].told = false,
                        Err(_) => self.untell_all(Some(kind)),
                    }
                }
            }
        }
    }

    /// Every category of `kind`, or of every kind, not told.
    fn untell_all(&mut self, kind: Option<ResKind>) {
        for category in &mut self.all {
            if kind.is_none_or(|kind| category.kind == kind) {
                category.told = false;
            }
        }
    }
}

/// What tells one unit from another: its streams, in order; its resources,
/// by the value they are, where the file's reader holds it, so that two
/// written alike apart are two; and where it starts.
#[derive(PartialEq, Eq, Hash)]
struct Key {
    streams: Vec<ObjectId>,
    resources: Option<*const Object>,
    start: State,
}

impl Unit<'_> {
    fn key(&self) -> Key {
        Key {
            streams: self.streams.iter().map(|&(id, _)| id).collect(),
            resources: self.resources.map(|r| ptr::from_ref(r.object)),
            start: self.start,
        }
    }
}

/// A unit's streams as read.
enum Read {
    /// Decoded, and joined.
    Joined(Concat<'static>),
    /// Not read, and why: a stream doesn't decode cleanly, or the source
    /// doesn't give it, or the unit is past the budget, or past what
    /// content's reading joins.
    Unread(&'static str),
    /// The source gives no stream's data: it gave none of the first asked.
    NoData,
}

/// A kind of fault, as it's told in one of a unit's streams: the first of
/// it there, where it is, and how many times it's told there.
struct Told<'c> {
    fault: Fault<'c>,
    /// The stream, by its place among the unit's.
    stream: u32,
    at: Range<usize>,
    count: usize,
}

/// Each kind of place ISO 32000-2 reads content otherwise, as it's told in
/// each of a unit's streams: its first there, the one that starts first,
/// and how many, in the order the kinds are first told, for the first
/// [`KINDS`]; past them, how many more, of any kind, and where the first
/// of those is. A kind is a [`Fault`]'s variant, so that what's kept is
/// bounded, however many are told.
struct Faults<'c> {
    content: &'c Concat<'c>,
    told: Vec<Told<'c>>,
    more: usize,
    past: Option<Range<usize>>,
}

impl<'c> Faults<'c> {
    /// None yet, of `content`.
    fn new(content: &'c Concat<'c>) -> Faults<'c> {
        Faults {
            content,
            told: Vec::new(),
            more: 0,
            past: None,
        }
    }

    /// `count` more of `fault`'s kind in the stream `at` starts in, `fault`
    /// at `at` its first there if none of the kind there starts before it.
    fn add(&mut self, at: Range<usize>, fault: Fault<'c>, count: usize) {
        let (kind, stream) = (mem::discriminant(&fault), self.content.pos(at.start).stream);
        let kept = (self.told.iter_mut())
            .find(|told| told.stream == stream && mem::discriminant(&told.fault) == kind);
        if let Some(told) = kept {
            told.count += count;
            if at.start < told.at.start {
                (told.fault, told.at) = (fault, at);
            }
        } else if self.told.len() < KINDS {
            let told = Told {
                fault,
                stream,
                at,
                count,
            };
            self.told.push(told);
        } else {
            self.past_kinds(at, count);
        }
    }

    /// `count` more past the kinds told apart, at `at`.
    fn past_kinds(&mut self, at: Range<usize>, count: usize) {
        self.more += count;
        if self.past.as_ref().is_none_or(|past| at.start < past.start) {
            self.past = Some(at);
        }
    }

    /// What `other`, of the same content, told, added to what this did.
    fn merge(&mut self, other: Faults<'c>) {
        for told in other.told {
            self.add(told.at, told.fault, told.count);
        }
        if let Some(at) = other.past {
            self.past_kinds(at, other.more);
        }
    }
}

impl<'c> Sink<'c> for Faults<'c> {
    fn fault(&mut self, at: Range<usize>, fault: Fault<'c>) {
        self.add(at, fault, 1);
    }
}

/// How deep what content opens and closes goes, `q` and `Q`, `BMC` or
/// `BDC` and `EMC`, or `BX` and `EX`, kept at 0 when a closer comes with
/// none open; and, in each of a unit's streams that opens or closes, how
/// low it goes there. What a stream leaves open at the end is what it
/// opened above that low, as far as no later stream goes below it, the
/// first of it the last opener the stream took at that low; and each
/// closer at 0 has nothing to close. So it's counted without keeping a
/// place for each opener.
#[derive(Debug, Default)]
struct Depth {
    depth: usize,
    runs: Vec<Run>,
}

/// What content opens and closes in one of a unit's streams.
#[derive(Debug)]
struct Run {
    /// The stream, by its place among the unit's.
    stream: u32,
    /// How deep it is at its lowest in the stream, and at its end.
    low: usize,
    end: usize,
    /// Where the last opener taken at `low` is.
    open: Option<Range<usize>>,
    /// The closers with nothing open, and where the first is.
    closers: usize,
    closer: Option<Range<usize>>,
}

impl Depth {
    /// The run of `stream`, begun where it isn't the last's.
    fn run(&mut self, stream: u32) -> &mut Run {
        if self.runs.last().is_none_or(|run| run.stream != stream) {
            self.runs.push(Run {
                stream,
                low: self.depth,
                end: self.depth,
                open: None,
                closers: 0,
                closer: None,
            });
        }
        self.runs.last_mut().expect("the stream's run is the last")
    }

    /// An opener at `at`, in `stream`.
    fn open(&mut self, stream: u32, at: Range<usize>) {
        let (was, depth) = (self.depth, self.depth.saturating_add(1));
        let run = self.run(stream);
        if was == run.low {
            run.open = Some(at);
        }
        run.end = depth;
        self.depth = depth;
    }

    /// A closer at `at`, in `stream`.
    fn close(&mut self, stream: u32, at: Range<usize>) {
        let (none, depth) = (self.depth == 0, self.depth.saturating_sub(1));
        let run = self.run(stream);
        if none {
            run.closers += 1;
            run.closer.get_or_insert(at);
        } else if depth < run.low {
            (run.low, run.open) = (depth, None);
        }
        run.end = depth;
        self.depth = depth;
    }

    /// What it finds, by stream, each with whether it's openers, where the
    /// first is and how many: openers left open at the end, unless reading
    /// `stopped`, as then the end isn't known; and closers with nothing
    /// open. By where each is.
    fn found(&self, stopped: bool) -> Vec<(bool, u32, Range<usize>, usize)> {
        let mut found = Vec::new();
        // How low content goes in the streams after the one read.
        let mut after = usize::MAX;
        for run in self.runs.iter().rev() {
            let left = run.end.min(after).saturating_sub(run.low);
            if let (false, Some(at), 1..) = (stopped, &run.open, left) {
                found.push((true, run.stream, at.clone(), left));
            }
            if let Some(at) = &run.closer {
                found.push((false, run.stream, at.clone(), run.closers));
            }
            after = after.min(run.low);
        }
        found.sort_by_key(|(_, _, at, _)| at.start);
        found
    }

    /// What it finds as one rule's findings, each kind once in each
    /// stream: openers left open, `open` saying what they are, counted
    /// first (`2 q left open at the end`), unless reading `stopped`; and
    /// closers with nothing open, `closer` saying what one is.
    fn told(&self, stopped: bool, open: &str, closer: &str) -> Vec<(Range<usize>, String)> {
        let mut kinds = Kinds::default();
        for (opener, stream, at, n) in self.found(stopped) {
            match opener {
                true => kinds.add_n((true, stream), at, n, many, || open.to_string()),
                false => kinds.add_n((false, stream), at, n, times, || closer.to_string()),
            }
        }
        kinds.found()
    }
}

/// `what`, and how many times it's found when that's more than once.
fn times(what: &str, n: usize) -> String {
    match n {
        1 => what.to_string(),
        n => format!("{what}, {n} times"),
    }
}

/// How many of `what` there are, before it, always, as what's left open
/// at the end is counted.
fn many(what: &str, n: usize) -> String {
    format!("{n} {what}")
}

/// What a rule finds past the kinds it tells apart in a unit: how many
/// more, of any kind.
fn more(n: usize) -> String {
    format!("{n} more, of kinds past the first {KINDS}")
}

/// How a kind's value says how many times it's found: [`times`] or
/// [`many`].
type Shown = fn(&str, usize) -> String;

/// Each kind of what a rule finds in a unit, as it's found: the first of
/// each, where it is and what it says, and how many times it's found, for
/// the first [`KINDS`] kinds; past them, how many more are found, of any
/// kind, and where the first of those is.
struct Kinds<K> {
    kinds: Vec<(K, Range<usize>, String, usize, Shown)>,
    more: usize,
    past: Option<Range<usize>>,
}

impl<K> Default for Kinds<K> {
    fn default() -> Kinds<K> {
        Kinds {
            kinds: Vec::new(),
            more: 0,
            past: None,
        }
    }
}

impl<K: PartialEq> Kinds<K> {
    /// One of `kind` at `at`, saying `value` if it's the first.
    fn add(&mut self, kind: K, at: Range<usize>, value: impl FnOnce() -> String) {
        self.add_n(kind, at, 1, times, value);
    }

    /// `n` of `kind` at `at`, saying `value` if it's the first, with how
    /// many as `shown` says it.
    fn add_n(
        &mut self,
        kind: K,
        at: Range<usize>,
        n: usize,
        shown: Shown,
        value: impl FnOnce() -> String,
    ) {
        if let Some(kept) = self.kinds.iter_mut().find(|kept| kept.0 == kind) {
            kept.3 += n;
        } else if self.kinds.len() < KINDS {
            self.kinds.push((kind, at, value(), n, shown));
        } else {
            self.more += n;
            self.past.get_or_insert(at);
        }
    }

    /// What it found, each kind at its first, with how many times, by
    /// where each is; then how many more, past the kinds told apart.
    fn found(self) -> Vec<(Range<usize>, String)> {
        let mut found: Vec<_> = (self.kinds.into_iter())
            .map(|(_, at, value, n, shown)| (at, shown(&value, n)))
            .collect();
        found.sort_by_key(|(at, _)| at.start);
        found.extend(self.past.map(|at| (at, more(self.more))));
        found
    }

    /// What it found, in [`Kinds::found`]'s order, each kind with its first
    /// value and how many times it's found; and none for how many more,
    /// with that, once.
    fn told(self) -> Vec<(Option<K>, Range<usize>, String, usize)> {
        let mut found: Vec<_> = (self.kinds.into_iter())
            .map(|(kind, at, value, n, _)| (Some(kind), at, value, n))
            .collect();
        found.sort_by_key(|(_, at, ..)| at.start);
        found.extend(self.past.map(|at| (None, at, more(self.more), 1)));
        found
    }
}

/// The operand an operation that names a resource
/// ([`Operation::resource`]) names it by, as written: `Tf`'s font, the
/// operand before its size; an inline image's color space, under its
/// `/ColorSpace` or `/CS`; the last operand of the rest.
fn written<'c>(op: &Operation<'c>) -> Option<Operand<'c>> {
    if let Some(image) = op.inline_image() {
        return image.entry(b"ColorSpace");
    }
    match op.name() {
        b"Tf" => op.arg(1),
        _ => op.arg(0),
    }
}

/// The name `written` is, without its `/`, as ISO 32000-2 reads it (7.3.5),
/// as a file's keys are read: a `#` an escape only before two hex digits,
/// every byte kept. None when it isn't one.
fn iso_name(written: &[u8]) -> Option<Cow<'_, [u8]>> {
    let name = written.strip_prefix(b"/")?;
    match (name.iter()).all(|&b| b != b'#' && pdf_tokenizer::is_regular(b)) {
        true => Some(Cow::Borrowed(name)),
        false => Some(Cow::Owned(
            pdf_tokenizer::decode::name(written).bytes().collect(),
        )),
    }
}

/// What a `resource` finding says of the name `written` that `op` uses and
/// its resources lack, the unit having `none` at all if so: the name as
/// written, and the operator.
fn lacking(written: &Operand, op: &Operation, none: bool) -> String {
    let (name, operator) = (word(written.bytes()), word(op.name()));
    match none {
        true => format!("{name} ({operator}), and it has no resources"),
        false => format!("{name} ({operator})"),
    }
}

/// A kind of what [`Rule::Operator`] finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placed<'c> {
    /// An operator where content is, which it may not be used in.
    Refused(State, Operator),
    /// A word Annex A doesn't name, outside `BX` and `EX`: each word its
    /// own kind.
    Unknown(&'c [u8]),
    /// An `EX` with no `BX` open.
    Unopened,
    /// `BX` left open at the end.
    Open,
    /// Content that ends where it is, inside a graphics object, or a
    /// glyph description that never has its `d0` or `d1`.
    Ends(State),
}

/// What's wrong with an operation's operands: they aren't as many as its
/// operator takes, or, as many, one isn't of the type it takes. Its
/// variant is the kind [`Rule::Operands`] tells apart.
#[derive(Debug, Clone, Copy)]
enum Wrong {
    /// How many it takes.
    Count(Takes),
    /// Which, from 0, and the type it takes, as a finding names it.
    Type(usize, &'static str),
}

/// How many operands an operator takes.
#[derive(Debug, Clone, Copy)]
enum Takes {
    Exactly(usize),
    /// 1, 3 or 4, `SC`'s and `sc`'s.
    Components,
    /// One at least, `SCN`'s and `scn`'s.
    OneAtLeast,
}

/// The graphics object content is in at `state`, as a finding names it:
/// none at the content stream level or where a glyph description begins.
fn object(state: State) -> Option<&'static str> {
    match state {
        State::Path => Some("a path object"),
        State::ClippingPath => Some("a clipping path object"),
        State::Text => Some("a text object"),
        State::InlineImage => Some("an inline image object"),
        _ => None,
    }
}

/// A word as a finding shows it: its first [`TOKEN`] bytes, `…` where
/// there are more.
fn word(written: &[u8]) -> String {
    let shown = String::from_utf8_lossy(&written[..written.len().min(TOKEN)]);
    match written.len() > TOKEN {
        true => format!("{shown}…"),
        false => shown.into_owned(),
    }
}

/// What a finding says of `op`, refused at `state`: where it is, or for
/// `d0` and `d1`, allowed only first in a glyph description, that it isn't
/// there.
fn refused(state: State, op: &Operation) -> String {
    let name = word(op.name());
    match (op.operator(), state, object(state)) {
        (Operator::GlyphWidth | Operator::GlyphWidthBox, ..) => {
            format!("{name} where it isn't first in a glyph description")
        }
        (_, State::ContentStream, _) => format!("{name} at the content stream level"),
        (_, State::GlyphStart, _) => {
            format!("{name} first in a glyph description, before d0 or d1")
        }
        (_, _, Some(object)) => format!("{name} in {object}"),
        _ => format!("{name} where content is"),
    }
}

/// Where content goes on after `op`, refused at `state`: where it was, as
/// what's refused changes nothing, but that a glyph description whose
/// first operator isn't `d0` or `d1` goes on at the content stream level,
/// `op` taken there where it may be, as if one had come before it; `ET`
/// ends what it's in, back at the level; and `BT`, `m` or `re` in a path
/// object begins its own, the path abandoned. A painting operator ends a
/// path wherever it is, so it's refused only where there's none to end. A
/// rule of the checker's, for how many findings one mistake makes.
fn resumed(state: State, op: &Operation) -> State {
    let level = || {
        State::ContentStream
            .step(op)
            .unwrap_or(State::ContentStream)
    };
    match (state, op.operator()) {
        (State::GlyphStart, _) => level(),
        (_, Operator::EndText) => State::ContentStream,
        (
            State::Path | State::ClippingPath,
            Operator::BeginText | Operator::Move | Operator::Rect,
        ) => level(),
        _ => state,
    }
}

/// What a finding says of an operand that isn't of the type it should be.
fn what(operand: &Operand) -> &'static str {
    match operand.bytes() {
        _ if operand.is_number() && operand.bytes().contains(&b'.') => "a real number",
        _ if operand.is_number() => "an integer",
        _ if operand.is_name() => "a name",
        _ if operand.is_string() => "a string",
        _ if operand.is_array() => "an array",
        _ if operand.is_dict() => "a dictionary",
        b"true" | b"false" => "a boolean",
        b"null" => "null",
        _ => "no object",
    }
}

/// Whether `operand`, as written, is of type `of`: an integer a number
/// written without a point; a type this checker doesn't know, any.
fn is(operand: &Operand, of: Type) -> bool {
    match of {
        Type::Number => operand.is_number(),
        Type::Integer => operand.is_number() && !operand.bytes().contains(&b'.'),
        Type::Name => operand.is_name(),
        Type::String => operand.is_string(),
        Type::Array => operand.is_array(),
        Type::NameOrDict => operand.is_name() || operand.is_dict(),
        _ => true,
    }
}

/// A type as a finding names it.
fn named(of: Type) -> &'static str {
    match of {
        Type::Number => "a number",
        Type::Integer => "an integer",
        Type::Name => "a name",
        Type::String => "a string",
        Type::Array => "an array",
        Type::NameOrDict => "a name or a dictionary",
        _ => "another type",
    }
}

/// What's wrong with `op`'s operands, by Annex A's table: how many there
/// are, else the first of them that isn't of its type, an operator's own
/// type by place; `SC` and `sc` take 1, 3 or 4 numbers, as many as a space
/// they can set has components, and `SCN` and `scn` one operand at least,
/// numbers but a pattern's name last. None for a word Annex A doesn't name,
/// whose operands aren't known. `operator` is `op`'s.
fn wrong(op: &Operation, operator: Operator) -> Option<Wrong> {
    let n = op.operands().len();
    match operator.operands()? {
        Operands::Fixed(types) if n != types.len() => {
            Some(Wrong::Count(Takes::Exactly(types.len())))
        }
        Operands::Fixed(types) => (op.operands().zip(types).enumerate())
            .find(|(_, (operand, of))| !is(operand, **of))
            .map(|(i, (_, &of))| Wrong::Type(i, named(of))),
        Operands::Components if !matches!(n, 1 | 3 | 4) => Some(Wrong::Count(Takes::Components)),
        Operands::Components => (op.operands())
            .position(|operand| !operand.is_number())
            .map(|i| Wrong::Type(i, "a number")),
        Operands::ComponentsOrPattern if n == 0 => Some(Wrong::Count(Takes::OneAtLeast)),
        Operands::ComponentsOrPattern => (op.operands().enumerate())
            .find(|(i, operand)| !(operand.is_number() || (i + 1 == n && operand.is_name())))
            .map(|(i, _)| match i + 1 == n {
                true => Wrong::Type(i, "a number or a name"),
                false => Wrong::Type(i, "a number"),
            }),
        _ => None,
    }
}

/// What a finding says of `op`'s operands, `wrong` as they are: what its
/// operator takes, and how many it has, so that the values of a kind differ
/// in their digits alone; or which isn't of its type, what it is and what
/// it should be.
fn said(op: &Operation, wrong: Wrong) -> String {
    let name = word(op.name());
    match wrong {
        Wrong::Count(takes) => {
            let takes = match takes {
                Takes::Exactly(0) => "no operands".to_string(),
                Takes::Exactly(1) => "1 operand".to_string(),
                Takes::Exactly(k) => format!("{k} operands"),
                Takes::Components => "1, 3 or 4 operands".to_string(),
                Takes::OneAtLeast => "1 operand at least".to_string(),
            };
            format!("{name} takes {takes}, not {}", op.operands().len())
        }
        Wrong::Type(i, of) => {
            let is = op
                .operands()
                .nth(i)
                .map_or("none", |operand| what(&operand));
            format!("{name}'s operand {} is {is}, not {of}", i + 1)
        }
    }
}

/// What one unit's reading found, by rule: each a place in the unit's
/// content, joined, and its value.
struct Found {
    balance: Vec<(Range<usize>, String)>,
    marked: Vec<(Range<usize>, String)>,
    operator: Vec<(Range<usize>, String)>,
    operands: Vec<(Range<usize>, String)>,
    /// Each with the category its name lacks from, none for how many more.
    resource: Vec<(Range<usize>, String, Option<ResKind>)>,
    content: Vec<(Range<usize>, String)>,
    /// Whether reading stopped before the end.
    stopped: bool,
}

/// What content is read under: pdf-content-streams' caps, an inline
/// image's DCT and CCITTFax data ended by decoding it, as PDFium ends it,
/// and its color space sized within [`SPACE_VISITS`].
fn limits() -> Limits {
    let mut limits = Limits::DEFAULT;
    limits.coded_ends = CodedEnds::Decoded;
    limits.color.space_visits = SPACE_VISITS;
    limits
}

/// What `table` makes `object`, reached as it, as `id`, the object it is
/// when it's indirect: a holder of content, when it's of the kind the table
/// is.
fn kind(table: &str, object: &Object, id: Option<ObjectId>) -> Option<Kind> {
    match (table, object.value()) {
        ("PageObject", Value::Dict(_)) => Some(Kind::Page),
        (
            "XObjectFormType1" | "XObjectFormPrinterMark" | "XObjectFormTrapNet" | "PatternType1",
            Value::Stream(_),
        ) => id.map(Kind::Own),
        ("FontType3", Value::Dict(_)) => Some(Kind::Glyphs),
        _ => None,
    }
}

/// Reads `content`, which begins at `start`, with `res` for its inline
/// images' color spaces, under `limits`: its operations, to their end or to
/// what stops them, each `q` and `Q`, each `BMC`, `BDC` and `EMC`, each
/// stepping where content is, among the graphics objects, its operands
/// held to its operator's, the resource it names, if any, as ISO 32000-2
/// reads the name, looked up by `look`, and each array and dictionary among
/// its operands read as an object, for what's inside it; and what each rule
/// finds of it.
fn pass(
    content: &Concat,
    start: State,
    res: &ResCx,
    limits: &Limits,
    look: &mut dyn FnMut(ResKind, &[u8]) -> Looked,
) -> Found {
    let (mut saved, mut marked) = (Depth::default(), Depth::default());
    let (mut faults, mut inside) = (Faults::new(content), Faults::new(content));
    let (mut placed, mut operands) = (Kinds::default(), Kinds::default());
    let mut named = Kinds::default();
    // `BX` and `EX`, and where content is, with where the graphics object
    // it's in began.
    let (mut compat, mut state, mut begun) = (Depth::default(), start, 0..0);
    let mut stop: Option<Error> = None;
    // Each kind is told apart in each stream it's in, by the stream's place
    // among the unit's.
    let stream = |at: &Range<usize>| content.pos(at.start).stream;
    for read in content.operations(res, limits).with_sink(&mut faults) {
        let op = match read {
            Ok(op) => op,
            Err(error) => {
                stop = Some(error);
                break;
            }
        };
        let at = op.range();
        match op.name() {
            b"q" => saved.open(stream(&at), at.clone()),
            b"Q" => saved.close(stream(&at), at.clone()),
            b"BMC" | b"BDC" => marked.open(stream(&at), at.clone()),
            b"EMC" => marked.close(stream(&at), at.clone()),
            b"BX" => compat.open(stream(&at), at.clone()),
            b"EX" => {
                if compat.depth == 0 {
                    let unopened = || "EX with no BX open".to_string();
                    placed.add((Placed::Unopened, stream(&at)), at.clone(), unopened);
                }
                compat.close(stream(&at), at.clone());
            }
            _ => {}
        }
        let operator = op.operator();
        let next = match (operator, state.step(&op)) {
            // Passed over inside `BX` and `EX` (7.8.2).
            (Operator::Other, _) if compat.depth > 0 => state,
            (Operator::Other, _) => {
                let unknown = || {
                    let word = word(op.name());
                    format!("{word}, an operator ISO 32000-2 doesn't define, outside BX and EX")
                };
                let kind = (Placed::Unknown(op.name()), stream(&at));
                placed.add(kind, at.clone(), unknown);
                resumed(state, &op)
            }
            (_, Some(next)) => next,
            (_, None) => {
                let refused = || refused(state, &op);
                let kind = (Placed::Refused(state, operator), stream(&at));
                placed.add(kind, at.clone(), refused);
                resumed(state, &op)
            }
        };
        if next != state && matches!(next, State::Path | State::Text) {
            begun = at.clone();
        }
        state = next;
        if let Some(wrong) = wrong(&op, operator) {
            let kind = ((operator, mem::discriminant(&wrong)), stream(&at));
            operands.add(kind, at.clone(), || said(&op, wrong));
        }
        if let Some((kind, _)) = op.resource()
            && let Some(written) = written(&op)
            && let Some(name) = iso_name(written.bytes())
        {
            let looked = look(kind, &name);
            if matches!(looked, Looked::Lacking | Looked::Without) {
                let none = looked == Looked::Without;
                let value = || lacking(&written, &op, none);
                let at = written.range();
                named.add(((kind, name), stream(&at)), at, value);
            }
        }
        for operand in op.operands() {
            if operand.is_array() || operand.is_dict() {
                operand.object_with(limits, &mut inside);
            }
        }
    }
    // Where content ends, unless reading stopped, as the end isn't known:
    // in an object, at where it began; or where a glyph description was to
    // begin, all of it.
    let stopped = stop.is_some();
    for (_, s, at, n) in compat.found(stopped).into_iter().filter(|found| found.0) {
        let open = || "BX left open at the end".to_string();
        placed.add_n((Placed::Open, s), at, n, many, open);
    }
    let ends = match (state, object(state)) {
        _ if stopped => None,
        (State::GlyphStart, _) => Some((
            0..content.bytes().len(),
            "a glyph description with no d0 or d1".to_string(),
        )),
        (_, Some(object)) => Some((begun, format!("content ends in {object}"))),
        _ => None,
    };
    if let Some((at, ends)) = ends {
        placed.add((Placed::Ends(state), stream(&at)), at, || ends);
    }
    faults.merge(inside);
    let mut found: Vec<(Range<usize>, String)> = (faults.told.into_iter())
        .map(|told| (told.at, times(&told.fault.to_string(), told.count)))
        .collect();
    if let Some(error) = stop {
        let value = format!("{}: content is read no further", error.kind);
        found.push((error.at..content.bytes().len(), value));
    }
    found.sort_by_key(|(at, _)| at.start);
    found.extend(faults.past.map(|at| (at, more(faults.more))));
    Found {
        balance: saved.told(
            stopped,
            "q left open at the end",
            "Q with nothing saved to restore",
        ),
        marked: marked.told(
            stopped,
            "BMC or BDC left open at the end",
            "EMC ending no marked content",
        ),
        operator: placed.found(),
        operands: operands.found(),
        // A name's value says nothing of how many times it's used, so that
        // what's alike is by the name alone.
        resource: (named.told().into_iter())
            .map(|(kind, at, value, _)| (at, value, kind.map(|((kind, _), _)| kind)))
            .collect(),
        content: found,
        stopped,
    }
}

/// Which of `content`'s streams `at`, offsets into it joined, starts in,
/// and where it is in that stream's own bytes: from where it starts, as
/// long as it is but no further than that stream's end.
fn placed(content: &Concat, at: Range<usize>) -> (usize, Range<usize>) {
    let pos = content.pos(at.start);
    let i = usize::try_from(pos.stream).unwrap_or(usize::MAX);
    let start = usize::try_from(pos.at).unwrap_or(usize::MAX);
    let len = match i < content.stream_count() {
        true => content.range(i).len(),
        false => start,
    };
    let end = start.saturating_add(at.len()).min(len).max(start);
    (i, start..end)
}

/// Why a stream whose data the reader doesn't give isn't read, by what
/// the reader says of it.
fn unread(error: &bytes::Error) -> &'static str {
    match error {
        bytes::Error::Decode(_) => DECODE,
        bytes::Error::Filters(_) => "a content stream through filters the reader doesn't take",
        bytes::Error::Decrypt(_) => "a content stream that doesn't decrypt cleanly",
        bytes::Error::Encrypted | bytes::Error::Locked | bytes::Error::Unsupported => {
            "a content stream the reader doesn't decrypt"
        }
        bytes::Error::Io(_) => "a content stream the file doesn't give",
        _ => "a content stream that doesn't read",
    }
}

impl<'a> Walk<'a> {
    /// Keeps `v` for the content pass when it holds content. Not in an
    /// earlier revision's trailer, whose objects are the file's where no
    /// later revision wrote them, as the walk from the trailer reaches them.
    /// And keeps where it's reached a resources dictionary, the first time,
    /// for the findings of `unused`.
    pub(super) fn note_content(&mut self, v: &Visit<'a>) {
        if self.revision.is_some() {
            return;
        }
        if v.table.name == "Resource" {
            (self.resources_at.entry(ptr::from_ref(v.object))).or_insert_with(|| Reached {
                holder: v.holder,
                trail: v.trail.clone(),
            });
        }
        if let Some(kind) = kind(&v.table.name, v.object, v.id()) {
            self.holders.push(Holder {
                object: v.object,
                holder: v.holder,
                table: v.table.name.as_str(),
                trail: v.trail.clone(),
                kind,
            });
        }
    }

    /// Reads the content of each holder the walk reached, each unit once,
    /// in the order the walk reached their first holders, within `budget`
    /// bytes decoded in all, and finds in what's held: see the module's
    /// documentation.
    pub(super) fn content(&mut self, budget: usize) {
        let holders = std::mem::take(&mut self.holders);
        // Each unit, with the holder it's told as: the first held holder
        // that has it, else the first. All are gathered first, so that a
        // held one isn't lost to one before it that shares its unit.
        let (mut units, mut seen): (Vec<(usize, Unit)>, HashMap<Key, usize>) = Default::default();
        for (i, holder) in holders.iter().enumerate() {
            let held = self.made(holder.holder).is_some();
            for unit in self.units(holder) {
                match seen.entry(unit.key()) {
                    Entry::Vacant(entry) => {
                        entry.insert(units.len());
                        units.push((i, unit));
                    }
                    Entry::Occupied(entry) => {
                        let (first, kept) = &mut units[*entry.get()];
                        if held && self.made(holders[*first].holder).is_none() {
                            (*first, *kept) = (i, unit);
                        }
                    }
                }
            }
        }
        // Where each unit's names are looked up, its categories each once,
        // as first met.
        let mut categories = Categories::default();
        let lookups: Vec<Lookup> = (units.iter())
            .map(|(i, unit)| self.lookup(&holders[*i], unit, &mut categories))
            .collect();
        let limits = limits();
        let none = ResCx::none();
        let mut spaces = HashMap::new();
        let mut budget = Budget {
            left: budget,
            gives: false,
        };
        for ((i, unit), lookup) in units.iter().zip(&lookups) {
            let holder = &holders[*i];
            // Held where what holds it is, or one of its streams, or what
            // its resources are written in, as an update that writes any of
            // them can change what its content finds.
            let written = self.resources_made(unit, lookup, &categories);
            let made = (self.made(holder.holder))
                .or_else(|| (unit.streams.iter()).find_map(|&(id, _)| self.made(Some(id))))
                .or(written);
            let held = made.is_some();
            if let (Some(made), Lookup::In(found)) = (made, lookup) {
                for &(_, found) in found {
                    if let Ok(c) = found {
                        categories.all[c].made.get_or_insert(made);
                    }
                }
            }
            let content = match self.read(unit, &limits, &mut budget) {
                Read::Joined(content) => content,
                Read::Unread(why) => {
                    if held {
                        let key = way_key(unit.streams[0].1);
                        self.unchecked("-", why.to_string(), holder.table, &key);
                    }
                    categories.untell(lookup);
                    continue;
                }
                Read::NoData => return,
            };
            // Only an inline image is sized by the resources' color spaces,
            // so they're read only where one may be, once for each.
            let resources = unit.resources.map(|r| r.object);
            let res = match content.bytes().windows(2).any(|w| w == b"BI") {
                false => &none,
                true => {
                    let source = self.source;
                    let objects = move |id| source.get(id).ok();
                    (spaces.entry(resources.map(ptr::from_ref))).or_insert_with(|| {
                        // A closure can't say why an object doesn't read,
                        // so this never fails.
                        ResCx::read(objects, resources, &limits).unwrap_or_default()
                    })
                }
            };
            let mut untold = Vec::new();
            let mut look = |kind, name: &[u8]| categories.look(lookup, kind, name, &mut untold);
            let found = pass(&content, unit.start, res, &limits, &mut look);
            if found.stopped {
                categories.untell(lookup);
            }
            if !held {
                continue;
            }
            self.report.content += 1;
            let rules = [
                (Rule::Balance, found.balance),
                (Rule::Marked, found.marked),
                (Rule::Operator, found.operator),
                (Rule::Operands, found.operands),
            ];
            for (rule, found) in rules {
                for (at, value) in found {
                    self.found_in(holder, unit, &content, written, rule, at, value, None);
                }
            }
            for (at, value, kind) in found.resource {
                let key = kind.map_or_else(|| "-".to_string(), category_key);
                let rule = Rule::Resource;
                self.found_in(holder, unit, &content, written, rule, at, value, Some(key));
            }
            for (at, value) in found.content {
                let rule = Rule::Content;
                self.found_in(holder, unit, &content, written, rule, at, value, None);
            }
            for why in untold {
                self.unchecked("-", why.to_string(), holder.table, "Resources");
            }
        }
        self.unused(&categories);
    }

    /// What's in `categories` that no content read uses, in the order
    /// they were met and each one's entries' order: the names of each
    /// whose object is held but those content uses, those it uses where
    /// what's used can't be told, and the default color spaces. Each
    /// counted, a finding with [`Options::unused`](super::Options::unused);
    /// of a category one of whose units wasn't read to its end, counted as
    /// not checked. One that can't be told apart from another isn't told.
    fn unused(&mut self, categories: &Categories<'a>) {
        for category in &categories.all {
            let (Some(_), Some(object)) = (category.identity, category.object) else {
                continue;
            };
            let Some(made) = self.made(Some(object)).or(category.made) else {
                continue;
            };
            let (kind, key) = (category.kind, category_key(category.kind));
            let spared = categories.spared.get(&kind);
            for entry in category.dict.last_entries() {
                let name = &entry.key[..];
                match category.used.get(name) {
                    Some(false) => {}
                    _ => continue,
                }
                let default = kind == ResKind::ColorSpace && DEFAULTS.contains(&name);
                if default || spared.is_some_and(|spared| spared.contains(name)) {
                    continue;
                }
                if !category.told {
                    self.unchecked("-", UNTOLD.to_string(), "Resource", &key);
                    continue;
                }
                let at = ("Resource".to_string(), key.clone());
                *self.report.unused.entry(at).or_default() += 1;
                if !self.unused {
                    continue;
                }
                let trail = Rc::new(Trail {
                    up: Some(category.trail.clone()),
                    step: Step::Key(name),
                    object: &entry.value,
                });
                let shown = String::from_utf8_lossy(&name[..name.len().min(TOKEN)]);
                self.report.findings.push(Finding {
                    object: Some(object),
                    made,
                    path: trail.path(),
                    table: "Resource".to_string(),
                    key: key.clone(),
                    rule: Rule::Unused,
                    value: format!("/{shown}"),
                    at: entry.key_at.or(entry.value.at),
                    revision: None,
                });
            }
        }
    }

    /// Where `unit`'s resource names are looked up, `holder`'s, its
    /// categories added to `categories` as they're met, in its resources'
    /// order: none for a page with no resources at all, and none that can be
    /// told for a form's, a pattern's or a glyph's with none of its own, as
    /// it would use the page's, or for resources that aren't a dictionary;
    /// a category, its references followed, that isn't a dictionary or
    /// refers to nothing can't be told, and one that's null is as if it
    /// weren't there (ISO 32000-2, 7.3.7).
    fn lookup(
        &self,
        holder: &Holder<'a>,
        unit: &Unit<'a>,
        categories: &mut Categories<'a>,
    ) -> Lookup {
        let Some(resources) = unit.resources else {
            return match holder.kind {
                Kind::Page => Lookup::Without,
                Kind::Own(_) | Kind::Glyphs => Lookup::Untold(UNOWNED),
            };
        };
        let Value::Dict(dict) = resources.object.value() else {
            return Lookup::Untold(DAMAGED);
        };
        let reached = self.resources_at.get(&ptr::from_ref(resources.object));
        let mut found = Vec::new();
        for entry in dict.last_entries() {
            let Some(&kind) = (ResKind::ALL.iter()).find(|kind| kind.key() == entry.key.as_slice())
            else {
                continue;
            };
            let (id, value) = match self.resolve(&entry.value) {
                Ok((_, value)) if value.is_null() => continue,
                Ok(found) => found,
                Err(_) => {
                    found.push((kind, Err(CATEGORY)));
                    continue;
                }
            };
            let Value::Dict(names) = value.value() else {
                found.push((kind, Err(CATEGORY)));
                continue;
            };
            let identity = match (id, resources.id) {
                (Some(id), _) => Some(Identity::Object(id)),
                (None, Some(id)) => Some(Identity::Key(id, kind)),
                (None, None) => value.at.map(Identity::At),
            };
            let make = || {
                let used = (names.last_entries())
                    .filter(|e| !matches!(self.resolve(&e.value), Ok((_, v)) if v.is_null()))
                    .map(|e| (&e.key[..], false))
                    .collect();
                let object = (id.or(resources.id))
                    .or_else(|| reached.and_then(|r| r.holder))
                    .or(holder.holder);
                let up = match reached {
                    Some(reached) => reached.trail.clone(),
                    None => Rc::new(Trail {
                        up: Some(holder.trail.clone()),
                        step: Step::Key(b"Resources"),
                        object: resources.object,
                    }),
                };
                let trail = Rc::new(Trail {
                    up: Some(up),
                    step: Step::Key(&entry.key),
                    object: value,
                });
                Category {
                    kind,
                    dict: names,
                    identity,
                    object,
                    trail,
                    used,
                    told: true,
                    made: None,
                }
            };
            found.push((kind, Ok(categories.add(identity, make))));
        }
        Lookup::In(found)
    }

    /// A finding of `rule`, `value`, at `at` in `unit`'s `content`, told as
    /// `holder`'s: in the stream it starts in, at its bytes in that
    /// stream's data, made as the holder is, else as that stream is, else as
    /// the unit's first held stream, else as its resources are, `written`.
    /// Its table and key the holder's, or for a name of a `category` of
    /// resources, `Resource` and the category.
    #[allow(clippy::too_many_arguments)]
    fn found_in(
        &mut self,
        holder: &Holder<'a>,
        unit: &Unit<'a>,
        content: &Concat,
        written: Option<Made>,
        rule: Rule,
        at: Range<usize>,
        value: String,
        category: Option<String>,
    ) {
        let (i, at) = placed(content, at);
        let Some(&(stream, way)) = unit.streams.get(i) else {
            return;
        };
        let made = (self.made(holder.holder))
            .or_else(|| self.made(Some(stream)))
            .or_else(|| (unit.streams.iter()).find_map(|&(id, _)| self.made(Some(id))))
            .or(written);
        let Some(made) = made else {
            return;
        };
        let (table, key) = match category {
            Some(category) => ("Resource".to_string(), category),
            None => (holder.table.to_string(), way_key(way)),
        };
        self.report.findings.push(Finding {
            object: Some(stream),
            made,
            path: path(holder, way),
            table,
            key,
            rule,
            value,
            at: Some(Span::new(Some(stream), at.start, at.end)),
            revision: None,
        });
    }

    /// The units of content `holder` holds: a page's streams as one, with
    /// its resources, its own or inherited; a form's or a pattern's own
    /// data, with its own; each glyph of a Type 3 font alone
    /// ([`Walk::glyphs`]). None with no stream.
    fn units(&self, holder: &Holder<'a>) -> Vec<Unit<'a>> {
        let Some(dict) = holder.object.as_dict() else {
            return Vec::new();
        };
        let units = match holder.kind {
            Kind::Page => {
                let streams = self.contents(dict);
                // Its own, else the nearest `/Parent`'s, as an inheritable
                // key's value is found.
                let resources = (self.resources(dict))
                    .or_else(|| self.up(dict, |parent| self.resources(parent)));
                let start = State::ContentStream;
                vec![Unit {
                    streams,
                    resources,
                    start,
                }]
            }
            Kind::Own(id) => {
                let resources = self.resources(dict);
                let (streams, start) = (vec![(id, Way::Own)], State::ContentStream);
                vec![Unit {
                    streams,
                    resources,
                    start,
                }]
            }
            Kind::Glyphs => self.glyphs(dict),
        };
        (units.into_iter())
            .filter(|unit| !unit.streams.is_empty())
            .collect()
    }

    /// Each glyph description in a Type 3 font's `/CharProcs`, in order, a
    /// unit alone that starts at its `d0` or `d1`, with its own resources,
    /// else the font's; an entry that isn't a stream passed over, as what
    /// `/CharProcs` holds is checked by the walk.
    fn glyphs(&self, font: &'a Dict) -> Vec<Unit<'a>> {
        let own = self.resources(font);
        let procs =
            (font.get(b"CharProcs").map(|o| self.resolve(o))).and_then(|found| {
                match found.ok()?.1.value() {
                    Value::Dict(procs) => Some(procs),
                    _ => None,
                }
            });
        let mut glyphs = Vec::new();
        for (name, glyph) in procs.into_iter().flat_map(Dict::iter) {
            if let Ok((Some(id), glyph)) = self.resolve(glyph)
                && let Value::Stream(stream) = glyph.value()
            {
                let resources = self.resources(&stream.dict).or(own);
                let streams = vec![(id, Way::Glyph(name))];
                glyphs.push(Unit {
                    streams,
                    resources,
                    start: State::GlyphStart,
                });
            }
        }
        glyphs
    }

    /// The streams of a page's `/Contents`: one, or each of an array's, a
    /// reference to either followed; an element that isn't a stream passed
    /// over, as what `/Contents` holds is checked by the walk.
    fn contents(&self, page: &'a Dict) -> Vec<(ObjectId, Way<'a>)> {
        let stream = |raw: &'a Object| match self.resolve(raw) {
            Ok((Some(id), o)) if matches!(o.value(), Value::Stream(_)) => Some(id),
            _ => None,
        };
        let Some(contents) = page.get(b"Contents") else {
            return Vec::new();
        };
        if let Some(id) = stream(contents) {
            return vec![(id, Way::Contents)];
        }
        match self.resolve(contents).map(|(_, o)| o.value()) {
            Ok(Value::Array(streams)) => (streams.iter().enumerate())
                .filter_map(|(i, raw)| Some((stream(raw)?, Way::Element(i))))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// `dict`'s own `/Resources`, its references followed, and the object
    /// they are where they're indirect; a null is none, and so is a
    /// reference to nothing.
    fn resources(&self, dict: &'a Dict) -> Option<Resources<'a>> {
        let raw = dict.get(b"Resources")?;
        let (id, object) = self.resolve(raw).ok()?;
        (!object.is_null()).then_some(Resources { id, object })
    }

    /// How the objects `unit`'s resources are written in, as `lookup` has
    /// them in `categories`, are made, the first of them held: its
    /// `/Resources`' own, else the one the walk reached them in (a page
    /// tree node's, for resources a page inherits), and each category's.
    fn resources_made(
        &self,
        unit: &Unit<'a>,
        lookup: &Lookup,
        categories: &Categories<'a>,
    ) -> Option<Made> {
        let own = unit.resources.and_then(|resources| {
            let reached = self.resources_at.get(&ptr::from_ref(resources.object));
            (resources.id).or_else(|| reached.and_then(|reached| reached.holder))
        });
        let each = match lookup {
            Lookup::In(found) => found.as_slice(),
            Lookup::Without | Lookup::Untold(_) => &[],
        };
        let of = (each.iter()).filter_map(|&(_, found)| categories.all[found.ok()?].object);
        own.into_iter().chain(of).find_map(|id| self.made(Some(id)))
    }

    /// `unit`'s streams decoded and joined, each asked for no more than
    /// what's left of `budget`, and what each held spent from it.
    fn read(&self, unit: &Unit<'a>, limits: &Limits, budget: &mut Budget) -> Read {
        let mut join = Join::new(limits);
        for &(id, _) in &unit.streams {
            // Once the source is known to give streams' data, none is asked
            // for past the budget.
            if budget.left == 0 && budget.gives {
                return Read::Unread(BUDGET);
            }
            let within = join.room().min(budget.left);
            let decoded = match self.source.decoded_within(id, within) {
                // A source that gives no stream's data, as a document alone,
                // says so of the first it's asked for.
                None if !budget.gives => return Read::NoData,
                None => return Read::Unread(UNGIVEN),
                Some(decoded) => decoded,
            };
            budget.gives = true;
            let data = match decoded {
                Ok(data) if data.len() <= within => data,
                // More than it was asked for is cut off at the limit, as a
                // stream past it is.
                Ok(_) => {
                    budget.left -= within;
                    return Read::Unread(past(budget.left));
                }
                // What decoded before a fault, or up to the limit that cut
                // it off, is spent as if it read.
                Err(bytes::Error::Decode(cut)) => {
                    budget.left = budget.left.saturating_sub(cut.data.len());
                    let limit = (cut.faults.iter()).any(|&(_, f)| f == pdf_filters::Fault::Limit);
                    return Read::Unread(match limit {
                        true => past(budget.left),
                        false => DECODE,
                    });
                }
                Err(error) => return Read::Unread(unread(&error)),
            };
            budget.left -= data.len();
            // Never past the room it was asked within.
            if join.push(&data).is_err() {
                return Read::Unread(TOO_LARGE);
            }
        }
        Read::Joined(join.finish())
    }
}

/// What's left of a file's budget of decoded content, and whether its
/// source has given a stream's data yet.
struct Budget {
    left: usize,
    gives: bool,
}

/// Why a unit cut off at a limit isn't read, `left` of the budget being
/// left: the budget, where that's spent, else what's read of one unit.
fn past(left: usize) -> &'static str {
    match left {
        0 => BUDGET,
        _ => TOO_LARGE,
    }
}

/// The path to a unit's stream reached by `way` from `holder`, as the walk
/// writes the stream's own when it reaches it from there: the holder's,
/// then `Contents`, `Contents[1]`, or `CharProcs` and the glyph's name.
fn path<'a>(holder: &Holder<'a>, way: Way<'a>) -> String {
    // A step's object is what `Trail::parents` reads, never its path, so
    // each is the holder's.
    let step = |up: &Rc<Trail<'a>>, step: Step<'a>| {
        Rc::new(Trail {
            up: Some(up.clone()),
            step,
            object: holder.object,
        })
    };
    let trail = match way {
        Way::Own => holder.trail.clone(),
        Way::Contents => step(&holder.trail, Step::Key(b"Contents")),
        Way::Element(i) => step(&step(&holder.trail, Step::Key(b"Contents")), Step::Index(i)),
        Way::Glyph(name) => step(
            &step(&holder.trail, Step::Key(b"CharProcs")),
            Step::Key(name),
        ),
    };
    trail.path()
}

/// A category's key in `/Resources`, as a finding names it.
fn category_key(kind: ResKind) -> String {
    String::from_utf8_lossy(kind.key()).into_owned()
}

/// The key a finding in a stream reached by `way` is at: its holder's
/// `Contents`, `-` for its own data, or a glyph's name.
fn way_key(way: Way) -> String {
    match way {
        Way::Own => "-".to_string(),
        Way::Contents | Way::Element(_) => "Contents".to_string(),
        Way::Glyph(name) => String::from_utf8_lossy(name).into_owned(),
    }
}

/// Whether a finding of `rule` at `key` is content's whose value says how
/// many times a kind was found and where content's bytes are, which
/// [`super::like`] sets aside: every rule's but `resource`'s, whose value
/// is a name, and `unused`'s; and `resource`'s past the kinds it tells
/// apart, its key `-`, which says how many more.
pub(super) fn counted(rule: Rule, key: &str) -> bool {
    match rule {
        Rule::Balance | Rule::Marked | Rule::Operator | Rule::Operands | Rule::Content => true,
        Rule::Resource => key == "-",
        _ => false,
    }
}

/// `value`, a content finding's of `rule`, with its digits left out, and
/// the count it ends with, `, 3 times`, so that a count, an offset or a
/// length doesn't tell two findings apart; but an operator's findings keep
/// a word's own digits, `d0`'s and `d1`'s, so that its name does.
pub(super) fn uncounted(rule: Rule, value: &str) -> String {
    let words = matches!(rule, Rule::Operator | Rule::Operands);
    let mut word = false;
    let digitless: String = (value.chars())
        .filter(|&c| {
            let kept = !c.is_ascii_digit() || (words && word);
            word = c.is_alphabetic() || (word && c.is_ascii_digit());
            kept
        })
        .collect();
    match digitless.strip_suffix(",  times") {
        Some(uncounted) => uncounted.to_string(),
        None => digitless,
    }
}

#[cfg(test)]
mod tests {
    //! Which streams a check asks its source for, and with how much of the
    //! budget left: each it reaches once, whatever holds it; and what's
    //! found in them, where, and as whose.

    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use pdf_structure::fixture;

    use pdf_content_streams::{Balance, ErrorKind, MarkedNest, Properties};

    use super::super::{CONTENT_BUDGET, Options, Report, Unchecked, check};
    use super::*;
    use crate::bytes::WithReading;
    use crate::check::{Hold, Made};
    use crate::source::{Facts, Record, Revision, Source, Trailer, Unread, Warning};
    use crate::version::Version;

    /// A source that answers as the one it wraps, and keeps each stream it's
    /// asked the data of, and within how much; but gives none of
    /// `without`'s.
    struct Counting<'s> {
        source: &'s dyn Source,
        asked: RefCell<Vec<(ObjectId, usize)>>,
        without: Option<ObjectId>,
    }

    impl Source for Counting<'_> {
        fn ids(&self) -> Vec<ObjectId> {
            self.source.ids()
        }

        fn get(&self, id: ObjectId) -> Result<&Object, Unread> {
            self.source.get(id)
        }

        fn trailer(&self) -> &Trailer {
            self.source.trailer()
        }

        fn header_version(&self) -> Option<Version> {
            self.source.header_version()
        }

        fn warnings(&self) -> &[Warning] {
            self.source.warnings()
        }

        fn size(&self) -> Option<usize> {
            self.source.size()
        }

        fn metadata(&self) -> Option<&[u8]> {
            self.source.metadata()
        }

        fn record(&self, id: ObjectId) -> Option<&Record> {
            self.source.record(id)
        }

        fn decoded(&self, stream: ObjectId) -> Option<&[u8]> {
            self.source.decoded(stream)
        }

        fn revisions(&self) -> &[Revision] {
            self.source.revisions()
        }

        fn facts(&self) -> Option<&Facts> {
            self.source.facts()
        }

        fn in_the_clear(&self, stream: ObjectId) -> bool {
            self.source.in_the_clear(stream)
        }

        fn decoded_within(
            &self,
            id: ObjectId,
            limit: usize,
        ) -> Option<Result<Vec<u8>, bytes::Error>> {
            self.asked.borrow_mut().push((id, limit));
            match Some(id) == self.without {
                true => None,
                false => self.source.decoded_within(id, limit),
            }
        }
    }

    /// A stream's object: its dictionary's `entries` and its `data`, with
    /// the `/Length` it has.
    fn stream(entries: &str, data: &str) -> String {
        format!(
            "<< {entries} /Length {} >>\nstream\n{data}\nendstream",
            data.len()
        )
    }

    /// A file of these objects, each its number and body, the catalog 1
    /// and the page tree 2.
    fn file(objects: &[(u32, String)]) -> Vec<u8> {
        let objects: Vec<(u32, &[u8])> = (objects.iter())
            .map(|(n, body)| (*n, body.as_bytes()))
            .collect();
        fixture::assembled("1.7", &objects, "<< /Size 100 /Root 1 0 R >>")
    }

    /// The catalog, and a page tree of `kids`, with `resources` its pages
    /// inherit.
    fn tree(kids: &str, resources: &str) -> Vec<(u32, String)> {
        let count = kids.split(" R").count() - 1;
        vec![
            (1, "<< /Type /Catalog /Pages 2 0 R >>".to_string()),
            (
                2,
                format!("<< /Type /Pages /Kids [{kids}] /Count {count} {resources} >>"),
            ),
        ]
    }

    /// A page under the tree, with `more` in it.
    fn page(more: &str) -> String {
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] {more} >>")
    }

    /// Each stream `file`'s check asks its source for, with how much it may
    /// decode to, holding what `hold` holds, within `budget`.
    fn asked(file: &[u8], hold: &Hold, budget: Option<usize>) -> Vec<(ObjectId, usize)> {
        let opened = WithReading::new(file, &[], &bytes::Options::DEFAULT).unwrap();
        let counting = Counting {
            source: &opened,
            asked: RefCell::new(Vec::new()),
            without: None,
        };
        let options = Options {
            content_budget: budget,
            ..Options::default()
        };
        check(&counting, hold, &options);
        counting.asked.into_inner()
    }

    /// The streams of [`asked`], sorted, every object held.
    fn streams(file: &[u8]) -> Vec<u32> {
        let mut asked: Vec<u32> = (asked(file, &Hold::All(Made::Merged), None).into_iter())
            .map(|((n, _), _)| n)
            .collect();
        asked.sort_unstable();
        asked
    }

    /// Every content stream the walk reaches is asked for once: a page's
    /// one; each of an array's, direct or through a reference; one two
    /// pages share with the resources they inherit, once; a form's, a
    /// pattern's, each glyph's; and what isn't content, or isn't reached
    /// as it, never.
    #[test]
    fn each_content_stream_reached_is_asked_for_once() {
        let fonts = "/Font << /T3 30 0 R >>";
        let resources =
            format!("/Resources << /XObject << /Fm 20 0 R >> /Pattern << /P 21 0 R >> {fonts} >>");
        let mut objects = tree("3 0 R 4 0 R 5 0 R 6 0 R", &resources);
        objects.extend([
            (3, page("/Contents 10 0 R")),
            (4, page("/Contents [11 0 R 12 0 R]")),
            (5, page("/Contents 13 0 R")),
            (6, page("/Contents 10 0 R /Thumb 15 0 R")),
            (10, stream("", "q /Fm Do Q")),
            (11, stream("", "q")),
            (12, stream("", "Q")),
            (13, "[14 0 R 99 0 R 2 0 R]".to_string()),
            (14, stream("", "/P scn")),
            (
                15,
                stream(
                    "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
                    "x",
                ),
            ),
            (
                20,
                stream(
                    "/Type /XObject /Subtype /Form /BBox [0 0 9 9] /Resources << >>",
                    "0 0 m 9 9 l S",
                ),
            ),
            (
                21,
                stream(
                    "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 \
                     /BBox [0 0 9 9] /XStep 9 /YStep 9 /Resources << >>",
                    "0 0 9 9 re f",
                ),
            ),
            (
                30,
                "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 9 9] /FontMatrix [1 0 0 1 0 0] \
                 /CharProcs << /a 31 0 R /b 32 0 R >> /Encoding << /Differences [97 /a /b] >> \
                 /FirstChar 97 /LastChar 98 /Widths [9 9] /Resources << >> >>"
                    .to_string(),
            ),
            (31, stream("", "9 0 d0 0 0 9 9 re f")),
            (32, stream("", "9 0 0 0 9 9 d1 0 0 9 9 re f")),
        ]);
        assert_eq!(streams(&file(&objects)), [10, 11, 12, 14, 20, 21, 31, 32]);
    }

    /// A stream two pages share is read once for each resources it's read
    /// with: once where both inherit the same, twice where one has its own.
    #[test]
    fn a_stream_shared_with_other_resources_is_asked_for_again() {
        let shared = |more: &str| {
            let mut objects = tree("3 0 R 4 0 R", "/Resources << >>");
            objects.extend([
                (3, page("/Contents 10 0 R")),
                (4, page(&format!("/Contents 10 0 R {more}"))),
                (10, stream("", "q Q")),
            ]);
            streams(&file(&objects))
        };
        assert_eq!(shared(""), [10]);
        assert_eq!(shared("/Resources << >>"), [10, 10]);
    }

    /// What isn't held is read too, as what it uses is the file's.
    #[test]
    fn a_held_page_and_an_unheld_one_are_both_read() {
        let mut objects = tree("3 0 R 4 0 R", "");
        objects.extend([
            (3, page("/Contents 10 0 R")),
            (4, page("/Contents 11 0 R")),
            (10, stream("", "q Q")),
            (11, stream("", "q Q")),
        ]);
        let hold = Hold::Only([((3, 0), Made::Built)].into());
        let mut asked: Vec<ObjectId> = (asked(&file(&objects), &hold, None).into_iter())
            .map(|(id, _)| id)
            .collect();
        asked.sort_unstable();
        assert_eq!(asked, [(10, 0), (11, 0)]);
    }

    /// Three pages of a stream each: 10 bytes, `second`, and 30 bytes.
    fn three_pages(second: String) -> Vec<u8> {
        let mut objects = tree("3 0 R 4 0 R 5 0 R", "");
        objects.extend([
            (3, page("/Contents 10 0 R")),
            (4, page("/Contents 11 0 R")),
            (5, page("/Contents 12 0 R")),
            (10, stream("", &"q Q ".repeat(3)[..10])),
            (11, second),
            (12, stream("", &"q Q ".repeat(8)[..30])),
        ]);
        file(&objects)
    }

    /// Each stream is asked for within what's left of the budget, and of
    /// what content's reading joins, the less; once none of the budget is
    /// left, no unit after is asked for, and with none from the start, the
    /// first is asked for nothing, so that a source that gives none says
    /// so. A stream cut off by it spends what it held, as one read whole
    /// would; and so does one that doesn't decode cleanly.
    #[test]
    fn content_is_read_within_its_budget() {
        let file = three_pages(stream("", &"q Q ".repeat(5)));
        let hold = Hold::All(Made::Merged);
        let asked_within = |budget| asked(&file, &hold, budget);
        let room = Limits::DEFAULT.content;
        let each = [((10, 0), room), ((11, 0), room), ((12, 0), room)];
        assert_eq!(asked_within(None), each);
        assert_eq!(asked_within(Some(CONTENT_BUDGET)), each);
        let all = [((10, 0), 100), ((11, 0), 90), ((12, 0), 70)];
        assert_eq!(asked_within(Some(100)), all);
        // Spent whole by the first two.
        assert_eq!(asked_within(Some(30)), [((10, 0), 30), ((11, 0), 20)]);
        // The second cut off, spending what's left.
        assert_eq!(asked_within(Some(25)), [((10, 0), 25), ((11, 0), 15)]);
        assert_eq!(asked_within(Some(10)), [((10, 0), 10)]);
        assert_eq!(asked_within(Some(0)), [((10, 0), 0)]);
        // Hex data without its `>`, all of it decoded, and a fault.
        let hex: String = (b"q Q ".repeat(5).iter())
            .map(|b| format!("{b:02x}"))
            .collect();
        let faulty = three_pages(stream("/Filter /ASCIIHexDecode", &hex));
        let opened = WithReading::new(&faulty[..], &[], &bytes::Options::DEFAULT).unwrap();
        let Some(Err(bytes::Error::Decode(error))) = opened.decoded_within((11, 0), 100) else {
            panic!("hex data without its end decodes with a fault");
        };
        assert_eq!(error.data.len(), 20);
        assert_eq!(asked(&faulty, &hold, Some(100)), all);
    }

    /// A pass needs a source that gives streams' data: a document alone is
    /// asked for the first stream, gives none, and is asked for no more.
    #[test]
    fn a_document_alone_is_asked_for_one_stream() {
        let doc = bytes::read(&three_pages(stream("", "q Q")));
        let counting = Counting {
            source: &doc,
            asked: RefCell::new(Vec::new()),
            without: None,
        };
        check(&counting, &Hold::All(Made::Merged), &Options::default());
        let room = Limits::DEFAULT.content;
        assert_eq!(counting.asked.into_inner(), [((10, 0), room)]);
    }

    /// What a check of `file`, read beside its reading, holding what `hold`
    /// holds, finds of content: each finding's rule, made, object, path and
    /// value, and its bytes in its stream's data, as the reading decodes it.
    fn found_held(file: &[u8], hold: &Hold) -> Vec<(Rule, Made, u32, String, String, String)> {
        let opened = WithReading::new(file, &[], &bytes::Options::DEFAULT).unwrap();
        let report = check(&opened, hold, &Options::default());
        (report.findings.iter())
            .filter(|f| counted(f.rule, &f.key))
            .map(|f| {
                let at = f.at.unwrap();
                let data = opened.decoded_within(at.stream.unwrap(), usize::MAX);
                let data = data.unwrap().unwrap();
                let bytes = String::from_utf8_lossy(&data[at.start..at.end]).into_owned();
                let object = f.object.unwrap().0;
                (
                    f.rule,
                    f.made,
                    object,
                    f.path.clone(),
                    f.value.clone(),
                    bytes,
                )
            })
            .collect()
    }

    /// [`found_held`], every object held: each finding's rule, object, path,
    /// value and bytes.
    fn found(file: &[u8]) -> Vec<(Rule, u32, String, String, String)> {
        (found_held(file, &Hold::All(Made::Merged)).into_iter())
            .map(|(rule, _, object, path, value, bytes)| (rule, object, path, value, bytes))
            .collect()
    }

    /// A file of a page whose `/Contents` is `streams`, numbered from 10:
    /// one, or an array of them, and with `more` objects, among which a
    /// form XObject 20, `/Fm` in the resources the page inherits.
    fn page_of(streams: &[&str], more: &[(u32, String)]) -> Vec<u8> {
        let resources = match more {
            [] => "/Resources << >>",
            _ => "/Resources << /XObject << /Fm 20 0 R >> >>",
        };
        let mut objects = tree("3 0 R", resources);
        let numbers: Vec<String> = (10..)
            .take(streams.len())
            .map(|n| format!("{n} 0 R"))
            .collect();
        let contents = match numbers[..] {
            [ref one] => one.clone(),
            _ => format!("[{}]", numbers.join(" ")),
        };
        objects.push((3, page(&format!("/Contents {contents}"))));
        for (n, data) in (10..).zip(streams) {
            objects.push((n, stream("", data)));
        }
        objects.extend_from_slice(more);
        file(&objects)
    }

    /// [`found`] of a page of `streams`.
    fn found_in_page(streams: &[&str]) -> Vec<(Rule, u32, String, String, String)> {
        found(&page_of(streams, &[]))
    }

    /// A finding of `rule` in stream `n`, the `i`th of a page's `/Contents`
    /// array, or its one stream where `i` is none.
    fn at(
        rule: Rule,
        n: u32,
        i: Option<usize>,
        value: &str,
        bytes: &str,
    ) -> (Rule, u32, String, String, String) {
        let path = match i {
            Some(i) => format!("Root/Pages/Kids[0]/Contents[{i}]"),
            None => "Root/Pages/Kids[0]/Contents".to_string(),
        };
        (rule, n, path, value.to_string(), bytes.to_string())
    }

    /// `q`s left open, at the first left open, the last taken with none
    /// saved before it; and a `Q` with nothing saved to restore, at the
    /// first, with how many: by where each is. A page's streams are one, so
    /// a `q` in one and its `Q` in the next balance; a form is read alone,
    /// and what it leaves open is its own.
    #[test]
    fn each_shape_of_unbalanced_q_and_q_is_found_where_it_is() {
        let open = "1 q left open at the end";
        let saved = "Q with nothing saved to restore";
        assert_eq!(found_in_page(&["q 0 0 m S Q"]), []);
        assert_eq!(
            found_in_page(&["q q Q"]),
            [at(Rule::Balance, 10, None, open, "q")]
        );
        let two = "2 q left open at the end";
        assert_eq!(
            found_in_page(&["q Q 1 g q q"]),
            [at(Rule::Balance, 10, None, two, "q")]
        );
        assert_eq!(found_in_page(&["q Q 1 g q q"])[0].4, "q");
        assert_eq!(
            found_in_page(&["Q q Q"]),
            [at(Rule::Balance, 10, None, saved, "Q")]
        );
        let twice = "Q with nothing saved to restore, 2 times";
        assert_eq!(
            found_in_page(&["Q Q q q Q"]),
            [
                at(Rule::Balance, 10, None, twice, "Q"),
                at(Rule::Balance, 10, None, open, "q")
            ]
        );
        // Where each is: the open `q` is the third, after `Q Q `.
        let file = page_of(&["Q Q q q Q"], &[]);
        let opened = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
        let report = check(&opened, &Hold::All(Made::Merged), &Options::default());
        let spans: Vec<_> = report.findings.iter().map(|f| f.at.unwrap()).collect();
        let (start, end) = (spans[1].start, spans[1].end);
        assert_eq!((spans[0].start, spans[0].end, start, end), (0, 1, 4, 5));
        // Across a page's streams, as one.
        assert_eq!(found_in_page(&["q 1 0 0 1 9 9 cm", "Q"]), []);
        assert_eq!(
            found_in_page(&["q 1 0 0 1 9 9 cm", "Q Q"]),
            [at(Rule::Balance, 11, Some(1), saved, "Q")]
        );
        // A form alone: the page around its `Do` doesn't balance it.
        let form = stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 9 9] /Resources << >>",
            "q 0 0 m 9 9 l S",
        );
        let found = found(&page_of(&["q /Fm Do Q"], &[(20, form)]));
        let in_form = (
            Rule::Balance,
            20,
            "Root/Pages/Resources/XObject/Fm".to_string(),
        );
        assert_eq!(
            found,
            [(
                in_form.0,
                in_form.1,
                in_form.2,
                open.to_string(),
                "q".to_string()
            )]
        );
    }

    /// A glyph's finding is at its font's table, the glyph's name its key,
    /// under its font's `/CharProcs`; a pattern's at its own table, its key
    /// `-`, as a form's is. Each in the order the walk reached its holder,
    /// the font first, as the resources' keys are walked.
    #[test]
    fn a_glyphs_and_a_patterns_findings_are_their_holders() {
        let font = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 9 9] /FontMatrix [1 0 0 1 0 0] \
                    /CharProcs << /a 31 0 R >> /Encoding << /Differences [97 /a] >> \
                    /FirstChar 97 /LastChar 97 /Widths [9] /Resources << >> >>";
        let pattern = stream(
            "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 \
             /BBox [0 0 9 9] /XStep 9 /YStep 9 /Resources << >>",
            "0 0 9 9 re f Q",
        );
        let resources = "/Resources << /Font << /T3 30 0 R >> /Pattern << /P 21 0 R >> >>";
        let mut objects = tree("3 0 R", resources);
        objects.extend([
            (3, page("/Contents 10 0 R")),
            (10, stream("", "/Pattern cs /P scn BT /T3 9 Tf (a) Tj ET")),
            (21, pattern),
            (30, font.to_string()),
            (31, stream("", "9 0 d0 q 0 0 9 9 re f")),
        ]);
        let file = file(&objects);
        let opened = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
        let report = check(&opened, &Hold::All(Made::Merged), &Options::default());
        let found: Vec<_> = (report.findings.iter())
            .map(|f| {
                (
                    f.rule,
                    f.object,
                    f.path.as_str(),
                    f.table.as_str(),
                    f.key.as_str(),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                (
                    Rule::Balance,
                    Some((31, 0)),
                    "Root/Pages/Resources/Font/T3/CharProcs/a",
                    "FontType3",
                    "a"
                ),
                (
                    Rule::Balance,
                    Some((21, 0)),
                    "Root/Pages/Resources/Pattern/P",
                    "PatternType1",
                    "-"
                ),
            ],
            "{report}"
        );
        assert_eq!(report.content, 3);
    }

    /// Every content of up to five of these operations: the depths of `q`
    /// and of marked content count as `Balance` and `MarkedNest` count them,
    /// where what's left open is the net less the lowest, a closer with
    /// nothing open each step below 0, and the first left open the
    /// outermost range `MarkedNest` leaves open. And where `MarkedNest` stops,
    /// past its cap on depth, the depth goes on: 600 ranges, nested and
    /// ended, are no finding.
    #[test]
    fn the_depths_count_as_balance_and_marked_nest_do() {
        let words = ["q", "Q", "/T BMC", "/T <</MCID 0>> BDC", "EMC", "0 0 m S"];
        let (mut contents, mut last) = (vec![String::new()], vec![String::new()]);
        for _ in 0..5 {
            last = (last.iter())
                .flat_map(|c| words.iter().map(move |w| format!("{c} {w}")))
                .collect();
            contents.extend(last.iter().cloned());
        }
        assert_eq!(contents.len(), 1 + 6 + 36 + 216 + 1296 + 7776);
        let (res, limits, props) = (ResCx::none(), Limits::DEFAULT, Properties::default());
        for content in &contents {
            let joined = Concat::new(&[content.as_bytes()]);
            let (mut saved, mut marked) = (Depth::default(), Depth::default());
            let mut balance = Balance::default();
            let mut nest = MarkedNest::new(&props, &limits);
            let (mut emcs, mut first_emc) = (0, None);
            for op in joined.operations(&res, &limits) {
                let op = op.unwrap();
                balance.take(&op);
                match nest.step(&joined, &op) {
                    Err(e) if e.kind == ErrorKind::Emc => {
                        emcs += 1;
                        first_emc.get_or_insert(e.at);
                    }
                    Err(e) => panic!("{content}: {e}"),
                    Ok(_) => {}
                }
                match op.name() {
                    b"q" => saved.open(0, op.range()),
                    b"Q" => saved.close(0, op.range()),
                    b"BMC" | b"BDC" => marked.open(0, op.range()),
                    b"EMC" => marked.close(0, op.range()),
                    _ => {}
                }
            }
            // What's left open and the closers with nothing open, each with
            // where the first is and how many, of the one stream.
            let ends = |depth: &Depth| {
                let (mut open, mut closers) = (None, None);
                for (opener, stream, at, n) in depth.found(false) {
                    assert_eq!(stream, 0, "{content}");
                    match opener {
                        true => open = Some((at.start, n)),
                        false => closers = Some((at.start, n)),
                    }
                }
                (open, closers)
            };
            let (open, closers) = ends(&saved);
            let left = open.map_or(0, |(_, n)| n);
            assert_eq!(left, saved.depth, "{content}");
            let left = i32::try_from(left).unwrap();
            assert_eq!(left, balance.net - balance.min_prefix, "{content}");
            let closers = i32::try_from(closers.map_or(0, |(_, n)| n)).unwrap();
            assert_eq!(closers, -balance.min_prefix, "{content}");
            let taken = saved.runs.first().is_some_and(|run| run.open.is_some());
            assert_eq!(taken, content.contains('q'), "{content}");
            let (open, closers) = ends(&marked);
            let closers = closers.map_or((0, None), |(at, n)| (n, Some(at)));
            assert_eq!(closers, (emcs, first_emc));
            assert_eq!(open.map_or(0, |(_, n)| n), nest.open().len(), "{content}");
            let left = match nest.done() {
                Err(e) => Some(e.at),
                Ok(_) => None,
            };
            assert_eq!(open.map(|(at, _)| at), left, "{content}");
        }
        let deep = format!("{}{}", "/T BMC ".repeat(600), "EMC ".repeat(600));
        let joined = Concat::new(&[deep.as_bytes()]);
        let mut nest = MarkedNest::new(&props, &limits);
        let stepped = (joined.operations(&res, &limits)).map(|op| nest.step(&joined, &op.unwrap()));
        let depth = stepped.filter_map(Result::err).next().map(|e| e.kind);
        assert_eq!(depth, Some(ErrorKind::Depth));
        assert_eq!(found_in_page(&[&deep]), []);
    }

    /// An `EMC` ending none, at the first, with how many; and a `BMC` or
    /// `BDC` left open, at the first of those left open, its tag and all,
    /// in the stream it's in, though the content goes on into another.
    #[test]
    fn marked_content_left_open_or_ended_twice_is_found() {
        let ending = "EMC ending no marked content";
        assert_eq!(
            found_in_page(&["EMC /A BMC EMC"]),
            [at(Rule::Marked, 10, None, ending, "EMC")]
        );
        let twice = "EMC ending no marked content, 2 times";
        assert_eq!(
            found_in_page(&["EMC /A BMC EMC EMC"]),
            [at(Rule::Marked, 10, None, twice, "EMC")]
        );
        let open = "1 BMC or BDC left open at the end";
        assert_eq!(
            found_in_page(&["0 0 m S /A BMC 0 0 m", "S"]),
            [at(Rule::Marked, 10, Some(0), open, "/A BMC")]
        );
        assert_eq!(
            found_in_page(&["/A BMC /B <</MCID 0>> BDC EMC", "0 0 m S"]),
            [at(Rule::Marked, 10, Some(0), open, "/A BMC")]
        );
        assert_eq!(found_in_page(&["/A BMC", "EMC"]), []);
    }

    /// Each thing that stops content's reading is found where it starts, to
    /// its stream's end: what came before it stands, a `Q` with nothing
    /// saved among it, but nothing's left open where the end isn't known.
    #[test]
    fn what_stops_reading_is_found_and_leaves_the_end_untold() {
        let operands = format!("{}m", "1 ".repeat(1 << 16 | 1));
        let deep = "<< /A ".repeat(70);
        let stops = [
            ("(abc", "a string that doesn't end", "(abc"),
            ("<4142", "a hex string that doesn't end", "<4142"),
            ("[1 2", "an array that doesn't end", "[1 2"),
            ("<< /A 1", "a dictionary that doesn't end", "<< /A 1"),
            (
                "BI /W 1 /H 1 /CS /G /BPC 8 ID x",
                "an inline image that doesn't end",
                "BI /W 1 /H 1 /CS /G /BPC 8 ID x",
            ),
            (
                &deep,
                "arrays, dictionaries or marked content nested too deep",
                &deep[64 * 6..],
            ),
            (&operands, "too many operands for one operator", "1 m"),
        ];
        let saved = "Q with nothing saved to restore";
        for (stop, what, bytes) in stops {
            let content = format!("Q q /A BMC {stop}");
            let value = format!("{what}: content is read no further");
            assert_eq!(
                found_in_page(&[&content]),
                [
                    at(Rule::Balance, 10, None, saved, "Q"),
                    at(Rule::Content, 10, None, &value, bytes)
                ],
                "{what}"
            );
        }
        // In a stream before the last, to that stream's end.
        let value = "a string that doesn't end: content is read no further";
        assert_eq!(
            found_in_page(&["q (abc", "Q"]),
            [at(Rule::Content, 10, Some(0), value, "(abc")]
        );
    }

    /// A finding's bytes are in the stream it starts in, cut at its end:
    /// the last token of one, and a string that runs on into the next.
    #[test]
    fn a_finding_at_a_streams_end_is_within_that_stream() {
        let open = "1 q left open at the end";
        assert_eq!(
            found_in_page(&["0 0 m S q", "0 0 m S"]),
            [at(Rule::Balance, 10, Some(0), open, "q")]
        );
        let across = "(a b) runs from one stream into the next";
        assert_eq!(
            found_in_page(&["0 0 m S BT (a", "b) Tj ET"]),
            [at(Rule::Content, 10, Some(0), across, "(a")]
        );
    }

    /// Each kind of fault, by who tells it, once a unit, at its first, with
    /// how many: a token's own, an inline image's, the operations', and
    /// what's inside an array or a dictionary, read as an object; and a
    /// dictionary that's none, told as no dictionary and as what makes it
    /// none. By where each is first. The operations are as PDFium cuts
    /// them, a closer that closes nothing an operand, so their operands'
    /// count is.
    #[test]
    fn each_kind_of_fault_is_found_once_a_unit() {
        let content = "BT ) [(a) foo] TJ ET \
                       BI /W 1 /H 1 /CS /G /BPC 8 /L 5 ID x EI \
                       /P <</MCID 0 1>> BDC EMC ) 0 0 m S 1 2";
        let found = found_in_page(&[content]);
        let values = |rule| -> Vec<(&str, &str)> {
            (found.iter())
                .filter(|(of, ..)| *of == rule)
                .map(|(_, _, _, value, bytes)| (value.as_str(), bytes.as_str()))
                .collect()
        };
        assert_eq!(
            values(Rule::Content),
            [
                (") closes nothing, 3 times", ")"),
                ("foo is no object, where one is written", "foo"),
                (
                    "an inline image's data is 1 bytes, where its length says 5",
                    "x"
                ),
                (
                    "<</MCID 0 1 is no dictionary here, where ISO 32000-2 reads one",
                    "<</MCID 0 1"
                ),
                ("1 is a dictionary's key and not a name", "1"),
                ("2 operands at the end that no operator takes", "1 2"),
            ]
        );
        assert_eq!(
            values(Rule::Operands),
            [
                ("TJ takes 1 operand, not 2", ") [(a) foo] TJ"),
                ("BDC takes 2 operands, not 3", "/P <</MCID 0 1>> BDC"),
                ("m takes 2 operands, not 3", ") 0 0 m"),
            ]
        );
        assert!(
            found
                .iter()
                .all(|f| matches!(f.0, Rule::Content | Rule::Operands))
        );
    }

    /// A 0x80 or 0xFF that content's reading takes for white-space, where
    /// ISO 32000-2 reads a regular character, is a kind of fault like any:
    /// found once a unit, at the first, with how many; one in a string
    /// isn't, and the rest read as they would with a space there.
    #[test]
    fn bytes_read_as_white_space_are_found_once_a_unit() {
        let data: &[u8] = b"q\x80Q 0 0 m\xff9 9 l S BT (\x80) Tj ET";
        let mut objects: Vec<(u32, Vec<u8>)> = (tree("3 0 R", "/Resources << >>").into_iter())
            .map(|(n, body)| (n, body.into_bytes()))
            .collect();
        objects.push((3, page("/Contents 10 0 R").into_bytes()));
        let head = format!("<< /Length {} >>\nstream\n", data.len());
        objects.push((10, [head.as_bytes(), data, b"\nendstream"].concat()));
        let objects: Vec<(u32, &[u8])> = (objects.iter())
            .map(|(n, body)| (*n, body.as_slice()))
            .collect();
        let file = fixture::assembled("1.7", &objects, "<< /Size 100 /Root 1 0 R >>");
        let value = "0x80 is white-space here and a regular character to ISO 32000-2, 2 times";
        assert_eq!(
            found(&file),
            [(
                Rule::Content,
                10,
                "Root/Pages/Kids[0]/Contents".to_string(),
                value.to_string(),
                "\u{fffd}".to_string()
            )]
        );
    }

    /// What `rule` finds in `file`: each finding's value and bytes.
    fn found_of(rule: Rule, file: &[u8]) -> Vec<(String, String)> {
        (found(file).into_iter())
            .filter(|(of, ..)| *of == rule)
            .map(|(_, _, _, value, bytes)| (value, bytes))
            .collect()
    }

    /// What `rule` finds in a page of one stream, `content`.
    fn in_page(rule: Rule, content: &str) -> Vec<(String, String)> {
        found_of(rule, &page_of(&[content], &[]))
    }

    /// A finding's value and bytes.
    fn told(value: &str, bytes: &str) -> (String, String) {
        (value.to_string(), bytes.to_string())
    }

    /// A file of a page that shows text in a Type 3 font, whose one glyph,
    /// `a`, object 31, is `glyph`.
    fn glyph_file(glyph: &str) -> Vec<u8> {
        let font = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 9 9] /FontMatrix [1 0 0 1 0 0] \
                    /CharProcs << /a 31 0 R >> /Encoding << /Differences [97 /a] >> \
                    /FirstChar 97 /LastChar 97 /Widths [9] /Resources << >> >>";
        let mut objects = tree("3 0 R", "/Resources << /Font << /T3 30 0 R >> >>");
        objects.extend([
            (3, page("/Contents 10 0 R")),
            (10, stream("", "BT /T3 9 Tf (a) Tj ET")),
            (30, font.to_string()),
            (31, stream("", glyph)),
        ]);
        file(&objects)
    }

    /// Each graphics object refuses what Figure 9 doesn't allow in it, by
    /// the operator and where content is: one finding each, at the
    /// operation, its operands with it. What it allows is no finding.
    #[test]
    fn each_state_refuses_what_figure_9_doesnt_allow_in_it() {
        let allowed = "q 1 0 0 1 9 9 cm 1 w 0 g /T BMC 0 0 m 9 9 l 0 9 9 0 re W n EMC \
                       BT /F1 9 Tf 1 0 0 1 0 0 Tm (a) Tj [(b)] TJ 0 g ET \
                       /Sh sh /Im Do BX EX Q";
        assert_eq!(in_page(Rule::Operator, allowed), []);
        let refused = [
            // The content stream level.
            ("(a) Tj", "Tj at the content stream level", "(a) Tj"),
            ("9 9 l", "l at the content stream level", "9 9 l"),
            ("n", "n at the content stream level", "n"),
            ("ET", "ET at the content stream level", "ET"),
            ("W", "W at the content stream level", "W"),
            // A path object.
            ("0 0 m 1 g 9 9 l f", "g in a path object", "1 g"),
            (
                "0 0 m 1 0 0 1 9 9 cm f",
                "cm in a path object",
                "1 0 0 1 9 9 cm",
            ),
            ("0 0 m /Im Do f", "Do in a path object", "/Im Do"),
            ("0 0 m (a) Tj f", "Tj in a path object", "(a) Tj"),
            // A clipping path object.
            ("0 0 m W 9 9 l n", "l in a clipping path object", "9 9 l"),
            ("0 0 m W* q n", "q in a clipping path object", "q"),
            // A text object.
            (
                "BT 1 0 0 1 9 9 cm ET",
                "cm in a text object",
                "1 0 0 1 9 9 cm",
            ),
            ("BT /Im Do ET", "Do in a text object", "/Im Do"),
            ("BT /Sh sh ET", "sh in a text object", "/Sh sh"),
            ("BT BT ET", "BT in a text object", "BT"),
            (
                "BT 9 0 d0 ET",
                "d0 where it isn't first in a glyph description",
                "9 0 d0",
            ),
        ];
        for (content, value, bytes) in refused {
            assert_eq!(
                in_page(Rule::Operator, content),
                [told(value, bytes)],
                "{content}"
            );
        }
        // An operator in two places is two kinds.
        assert_eq!(
            in_page(Rule::Operator, "(a) Tj 0 0 m (b) Tj f (c) Tj"),
            [
                told("Tj at the content stream level, 2 times", "(a) Tj"),
                told("Tj in a path object", "(b) Tj"),
            ]
        );
    }

    /// The errata let a text object hold the general graphics state, `q`
    /// and `Q` among it, and marked content (9.4.1, 14.6.1): none of it is
    /// refused there.
    #[test]
    fn the_errata_allow_q_and_marked_content_in_a_text_object() {
        let content = "BT q /F1 9 Tf 2 w 1 J Q /T BMC (a) Tj EMC /T /P MP ET";
        assert_eq!(in_page(Rule::Operator, content), []);
    }

    /// An inline image may begin at the content stream level alone: in a
    /// text object it's refused, the whole image its bytes, and content
    /// goes on in the text object. A `BI` that begins none, as PDFium reads
    /// it, is refused where an image may not begin, and found as no image
    /// wherever it is.
    #[test]
    fn an_inline_image_or_a_lone_bi_in_a_text_object_is_refused() {
        let image = "BI /W 1 /H 1 /CS /G /BPC 8 ID x EI";
        assert_eq!(in_page(Rule::Operator, image), []);
        assert_eq!(
            in_page(Rule::Operator, &format!("BT {image} (a) Tj ET")),
            [told("BI in a text object", image)]
        );
        let lone = "BI /W 1 Q";
        assert_eq!(in_page(Rule::Operator, &format!("q {lone}")), []);
        let file = page_of(&[&format!("BT q {lone} ET")], &[]);
        assert_eq!(
            found_of(Rule::Operator, &file),
            [told("BI in a text object", "BI")]
        );
        let no_image = "BI /W 1 Q begins no inline image, a keyword where a key is written";
        assert_eq!(
            found_of(Rule::Content, &file),
            [told(no_image, "BI /W 1 Q")]
        );
    }

    /// A word Annex A doesn't name is found outside `BX` and `EX`, each
    /// word its own kind, and passed over inside them, however they nest; an
    /// `EX` with no `BX` open, and `BX` left open at the end, are found too.
    #[test]
    fn a_word_annex_a_doesnt_name_is_passed_over_only_inside_bx_and_ex() {
        let foo = "foo, an operator ISO 32000-2 doesn't define, outside BX and EX";
        assert_eq!(in_page(Rule::Operator, "1 foo"), [told(foo, "1 foo")]);
        assert_eq!(in_page(Rule::Operator, "BX 1 foo EX"), []);
        assert_eq!(
            in_page(Rule::Operator, "BX BX foo EX foo EX foo"),
            [told(foo, "foo")]
        );
        let bar = "bar, an operator ISO 32000-2 doesn't define, outside BX and EX";
        assert_eq!(
            in_page(Rule::Operator, "foo bar foo"),
            [told(&format!("{foo}, 2 times"), "foo"), told(bar, "bar")]
        );
        // Not refused where content is besides: an unknown word only.
        assert_eq!(
            in_page(Rule::Operator, "0 0 m foo 9 9 l f"),
            [told(foo, "foo")]
        );
        assert_eq!(
            in_page(Rule::Operator, "EX q EX Q"),
            [told("EX with no BX open, 2 times", "EX")]
        );
        assert_eq!(
            in_page(Rule::Operator, "BX BX foo EX"),
            [told("1 BX left open at the end", "BX")]
        );
        // A long word is shown cut.
        let long = "w".repeat(TOKEN + 1);
        let shown = format!("{}…", &long[..TOKEN]);
        let value = &in_page(Rule::Operator, &long)[0].0;
        assert!(
            value.starts_with(&format!("{shown}, an operator")),
            "{value}"
        );
    }

    /// A glyph description's first operator is `d0` or `d1`, and they're
    /// allowed nowhere else: one that begins otherwise is refused at its
    /// first, and goes on at the content stream level, as after `d0`; one
    /// with no operator at all has no `d0` or `d1`, all of it the finding.
    #[test]
    fn a_glyph_description_begins_with_d0_or_d1_alone() {
        let glyph = |data: &str| found_of(Rule::Operator, &glyph_file(data));
        assert_eq!(glyph("9 0 d0 0 0 9 9 re f"), []);
        assert_eq!(glyph("9 0 0 0 9 9 d1 0 0 9 9 re f"), []);
        let d0 = "d0 where it isn't first in a glyph description";
        assert_eq!(
            glyph("0 0 9 9 re f 9 0 d0"),
            [
                told(
                    "re first in a glyph description, before d0 or d1",
                    "0 0 9 9 re"
                ),
                told(d0, "9 0 d0"),
            ]
        );
        assert_eq!(
            glyph("q 9 0 d0 Q"),
            [
                told("q first in a glyph description, before d0 or d1", "q"),
                told(d0, "9 0 d0"),
            ]
        );
        let d1 = "d1 where it isn't first in a glyph description";
        assert_eq!(glyph("9 0 d0 9 0 0 0 9 9 d1"), [told(d1, "9 0 0 0 9 9 d1")]);
        assert_eq!(
            glyph(""),
            [told("a glyph description with no d0 or d1", "")]
        );
        // Where it's the whole glyph's.
        let found = found(&glyph_file(""));
        let path = "Root/Pages/Resources/Font/T3/CharProcs/a";
        assert_eq!((found[0].1, found[0].2.as_str()), (31, path));
        // A page's.
        assert_eq!(in_page(Rule::Operator, "9 0 d0"), [told(d0, "9 0 d0")]);
    }

    /// After an operator that's refused, content is where it was; but `BT`,
    /// `m` or `re` in a path object begins its own object, the path
    /// abandoned, and `ET` ends what it's in: so one mistake is one
    /// finding.
    #[test]
    fn content_goes_on_where_it_was_after_a_refused_operator() {
        let cases = [
            ("0 0 m BT (a) Tj ET", "BT in a path object", "BT"),
            (
                "0 0 m W 0 0 m 9 9 l f",
                "m in a clipping path object",
                "0 0 m",
            ),
            (
                "0 0 m W 0 0 9 9 re f",
                "re in a clipping path object",
                "0 0 9 9 re",
            ),
            ("0 0 m ET 1 g", "ET in a path object", "ET"),
            (
                "0 0 m 1 g 2 g 9 9 l f",
                "g in a path object, 2 times",
                "1 g",
            ),
        ];
        for (content, value, bytes) in cases {
            assert_eq!(
                in_page(Rule::Operator, content),
                [told(value, bytes)],
                "{content}"
            );
        }
        // A path in a text object is refused, and the text object goes on.
        assert_eq!(
            in_page(Rule::Operator, "BT 0 0 m 9 9 l f (a) Tj ET"),
            [
                told("m in a text object", "0 0 m"),
                told("l in a text object", "9 9 l"),
                told("f in a text object", "f"),
            ]
        );
    }

    /// Content that ends inside a graphics object is found at where it
    /// began, across a page's streams as one; not where reading stopped
    /// before the end, which isn't known.
    #[test]
    fn content_ending_inside_an_object_is_found_where_it_began() {
        let ends = [
            ("1 g 0 0 m 9 9 l", "content ends in a path object", "0 0 m"),
            ("BT (a) Tj", "content ends in a text object", "BT"),
            (
                "0 0 9 9 re W",
                "content ends in a clipping path object",
                "0 0 9 9 re",
            ),
        ];
        for (content, value, bytes) in ends {
            assert_eq!(
                in_page(Rule::Operator, content),
                [told(value, bytes)],
                "{content}"
            );
        }
        let file = |streams: &[&str]| found_of(Rule::Operator, &page_of(streams, &[]));
        assert_eq!(file(&["BT", "(a) Tj ET"]), []);
        assert_eq!(
            file(&["q BT", "(a) Tj"]),
            [told("content ends in a text object", "BT")]
        );
        assert_eq!(in_page(Rule::Operator, "BX BT (abc"), []);
        assert_eq!(in_page(Rule::Operator, "BX 0 0 m [1"), []);
        // What came before a stop stands.
        assert_eq!(
            in_page(Rule::Operator, "n BT (abc"),
            [told("n at the content stream level", "n")]
        );
    }

    /// Each operation's operands are held to its operator's in Annex A's
    /// table: how many, then each one's type, as written; `SC` and `sc`
    /// take 1, 3 or 4 numbers, `SCN` and `scn` numbers but a name last. A
    /// word Annex A doesn't name takes any.
    #[test]
    fn operands_are_as_many_as_their_operator_takes_and_of_its_types() {
        let wrong = [
            ("1 2 3 m", "m takes 2 operands, not 3"),
            ("(a) m", "m takes 2 operands, not 1"),
            ("m", "m takes 2 operands, not 0"),
            ("(a) 1 m", "m's operand 1 is a string, not a number"),
            ("1 2 3 re", "re takes 4 operands, not 3"),
            ("1 q", "q takes no operands, not 1"),
            ("1.5 J", "J's operand 1 is a real number, not an integer"),
            ("9 /F1 Tf", "Tf's operand 1 is an integer, not a name"),
            (
                "/T (x) BDC",
                "BDC's operand 2 is a string, not a name or a dictionary",
            ),
            ("true 0 d", "d's operand 1 is a boolean, not an array"),
            ("[1] null d", "d's operand 2 is null, not a number"),
            ("1 2 SC", "SC takes 1, 3 or 4 operands, not 2"),
            ("1 2 3 4 5 sc", "sc takes 1, 3 or 4 operands, not 5"),
            ("/P sc", "sc's operand 1 is a name, not a number"),
            ("scn", "scn takes 1 operand at least, not 0"),
            ("/P 0.5 scn", "scn's operand 1 is a name, not a number"),
            (
                "1 (x) SCN",
                "SCN's operand 2 is a string, not a number or a name",
            ),
            (
                "1 BI /W 1 /H 1 /CS /G /BPC 8 ID x EI",
                "BI takes no operands, not 1",
            ),
        ];
        for (content, value) in wrong {
            assert_eq!(
                in_page(Rule::Operands, content),
                [told(value, content)],
                "{content}"
            );
        }
        let right = [
            "0 0 m 1 2 l h S",
            "1 J -2 j 1.5 w [1 2] 0 d [] 0.5 d",
            "/F1 9 Tf BT 1 2 (a) \" (b) ' [(c) 1] TJ ET",
            "/T /P BDC EMC /T <</MCID 0>> BDC EMC /T MP /T /P DP",
            "0.5 SC 1 2 3 sc 0 0 0 1 SC",
            "/P1 scn 0.5 /P1 scn 1 2 3 4 5 6 7 SCN 1 SCN",
            "1 2 foo",
        ];
        for content in right {
            assert_eq!(in_page(Rule::Operands, content), [], "{content}");
        }
    }

    /// One kind is one finding, however many times it's found, saying how
    /// many: of a refused operator, and of operands.
    #[test]
    fn a_kind_found_many_times_is_one_finding() {
        let many = "n ".repeat(40_000);
        assert_eq!(
            in_page(Rule::Operator, &many),
            [told("n at the content stream level, 40000 times", "n")]
        );
        let many = "1 q ".repeat(1_000);
        assert_eq!(
            in_page(Rule::Operands, &many),
            [told("q takes no operands, not 1, 1000 times", "1 q")]
        );
    }

    /// Past 32 kinds of a rule in a unit, one finding more, at the first
    /// past them, says how many more there are, of any kind: a kind told
    /// apart already is still counted as itself.
    #[test]
    fn past_32_kinds_one_finding_says_how_many_more() {
        let words: Vec<String> = (0..40).map(|i| format!("w{i}")).collect();
        let content = format!("{} w0 w39", words.join(" "));
        let found = in_page(Rule::Operator, &content);
        assert_eq!(found.len(), KINDS + 1);
        let unknown = ", an operator ISO 32000-2 doesn't define, outside BX and EX";
        assert_eq!(found[0], told(&format!("w0{unknown}, 2 times"), "w0"));
        assert_eq!(found[31], told(&format!("w31{unknown}"), "w31"));
        let more = format!("9 more, of kinds past the first {KINDS}");
        assert_eq!(found[KINDS], told(&more, "w32"));
        // Operands alike: every operator Annex A names but an inline
        // image's, with eight operands, which none takes but `SCN` and
        // `scn`, which take any number.
        let named: Vec<&str> = (Operator::ALL.iter())
            .filter_map(|op| op.name())
            .filter(|name| !matches!(*name, "BI" | "ID" | "EI" | "SCN" | "scn"))
            .collect();
        let content: String = (named.iter())
            .map(|name| format!("1 2 3 4 5 6 7 8 {name}\n"))
            .collect();
        let found = in_page(Rule::Operands, &content);
        assert_eq!(found.len(), KINDS + 1);
        let more = format!(
            "{} more, of kinds past the first {KINDS}",
            named.len() - KINDS
        );
        assert_eq!(found[KINDS].0, more);
    }

    /// An operator's findings are alike whatever their counts, but for the
    /// operator's own digits: `d0` is not `d1`, nor `d`.
    #[test]
    fn an_operators_findings_are_carried_by_its_name() {
        let operator = |value: &str| uncounted(Rule::Operator, value);
        assert_eq!(
            operator("n at the content stream level, 40000 times"),
            operator("n at the content stream level")
        );
        assert_eq!(
            operator("2 BX left open at the end"),
            operator("1 BX left open at the end")
        );
        let d0 = "d0 where it isn't first in a glyph description";
        assert_ne!(operator(d0), operator(&d0.replace("d0", "d1")));
        let operands = |value: &str| uncounted(Rule::Operands, value);
        assert_eq!(
            operands("m takes 2 operands, not 3"),
            operands("m takes 2 operands, not 0, 2 times")
        );
        assert_ne!(
            operands("d1 takes 6 operands, not 5"),
            operands("d takes 2 operands, not 5")
        );
        assert_ne!(
            operands("d0's operand 1 is a name, not a number"),
            operands("d1's operand 1 is a name, not a number")
        );
        // Through the reports: a second `n` is carried, a `d1` where a `d0`
        // was isn't.
        let report = |content: &str| {
            let file = page_of(&[content], &[]);
            let opened = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
            check(&opened, &Hold::All(Made::Merged), &Options::default())
        };
        let values = |found: Vec<&Finding>| -> Vec<String> {
            found.into_iter().map(|f| f.value.clone()).collect()
        };
        let before = report("n 9 0 d0");
        assert_eq!(
            values(report("n n 9 0 d0").since(&before)),
            Vec::<String>::new()
        );
        let after = report("n 9 0 0 0 9 9 d1");
        assert_eq!(values(after.since(&before)), [d0.replace("d0", "d1")]);
    }

    /// A kind is told apart in each of a page's streams it's found in, at its
    /// first there, with how many there: so what one stream leaves open at
    /// the end is its own, and an update that writes one stream answers for
    /// what it brings, though another of the page's streams had its kind.
    #[test]
    fn a_kind_is_found_once_in_each_stream_it_is_in() {
        let saved = "Q with nothing saved to restore";
        let thrice = "Q with nothing saved to restore, 3 times";
        let level = "n at the content stream level";
        assert_eq!(
            found_in_page(&["Q 0 0 m S", "Q Q Q n 0 0 m S"]),
            [
                at(Rule::Balance, 10, Some(0), saved, "Q"),
                at(Rule::Balance, 11, Some(1), thrice, "Q"),
                at(Rule::Operator, 11, Some(1), level, "n"),
            ]
        );
        let open = "1 q left open at the end";
        assert_eq!(
            found_in_page(&["q", "q"]),
            [
                at(Rule::Balance, 10, Some(0), open, "q"),
                at(Rule::Balance, 11, Some(1), open, "q"),
            ]
        );
        // The second `q` closed by the next stream's `Q`, the first not.
        assert_eq!(
            found_in_page(&["q q", "Q", ""]),
            [at(Rule::Balance, 10, Some(0), open, "q")]
        );
        // What a stream opens and a later one closes below is closed.
        assert_eq!(found_in_page(&["q", "q", "Q Q"]), []);
        // A fault too, and what one stream leaves open and another the
        // same.
        let closer = |n, i| at(Rule::Content, n, Some(i), ") closes nothing", ")");
        let faults: Vec<_> = (found_in_page(&["BT ) ET", "BT ) ET"]).into_iter())
            .filter(|found| found.0 == Rule::Content)
            .collect();
        assert_eq!(faults, [closer(10, 0), closer(11, 1)]);
        let two = "2 BMC or BDC left open at the end";
        assert_eq!(
            found_in_page(&["/A BMC", "EMC /B BMC /C BMC", "/D BMC"]),
            [
                at(Rule::Marked, 11, Some(1), two, "/B BMC"),
                at(Rule::Marked, 12, Some(2), &two.replace('2', "1"), "/D BMC"),
            ]
        );
        // An update that writes the second stream alone: what it brings is
        // its own, the first's carried.
        let report = |streams: &[&str]| {
            let file = page_of(streams, &[]);
            let opened = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
            check(&opened, &Hold::All(Made::Merged), &Options::default())
        };
        let before = report(&["Q 0 0 m S", "0 0 m S"]);
        let after = report(&["Q 0 0 m S", "Q Q Q n 0 0 m S"]);
        let since: Vec<_> = (after.since(&before).into_iter())
            .map(|f| (f.object.unwrap().0, f.value.as_str()))
            .collect();
        assert_eq!(since, [(11, thrice), (11, level)]);
    }

    /// Content is held where what its resources are written in is, as an
    /// update that writes them alone can make a name lack; and found in all
    /// the same, a finding its stream had before carried. A page's inherited
    /// resources are written in the page tree node they're in.
    #[test]
    fn content_is_held_where_its_resources_are() {
        let report = |resources: &str, font: &str, held: Option<u32>| {
            let mut objects = tree("3 0 R", resources);
            objects.extend([
                (3, page("/Contents 10 0 R")),
                (10, stream("", "q BT /F1 9 Tf (a) Tj ET")),
                (40, format!("<< /Font << /{font} 50 0 R >> >>")),
                (50, FONT.to_string()),
            ]);
            let file = file(&objects);
            let opened = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
            let hold = match held {
                Some(n) => Hold::Only([((n, 0), Made::Merged)].into()),
                None => Hold::All(Made::Merged),
            };
            check(&opened, &hold, &Options::default())
        };
        let found = |found: Vec<&Finding>| -> Vec<(u32, Rule, String)> {
            (found.into_iter())
                .filter(|f| matches!(f.rule, Rule::Balance | Rule::Resource))
                .map(|f| (f.object.unwrap().0, f.rule, f.value.clone()))
                .collect()
        };
        let before = report("/Resources 40 0 R", "F1", None);
        let open = (10, Rule::Balance, "1 q left open at the end".to_string());
        assert_eq!(found(before.findings.iter().collect()), vec![open.clone()]);
        // The resources, 40, written alone, without the font.
        let after = report("/Resources 40 0 R", "F2", Some(40));
        let lacks = (10, Rule::Resource, "/F1 (Tf)".to_string());
        let all = found(after.findings.iter().collect());
        assert_eq!(all, [open.clone(), lacks.clone()]);
        assert_eq!(found(after.since(&before)), [lacks]);
        // Inherited from the page tree node, 2, which holds them, and not
        // held by what they name.
        let direct = "/Resources << /Font << /F2 50 0 R >> >>";
        let held = |n| found(report(direct, "F1", Some(n)).findings.iter().collect());
        assert_eq!(held(2).len(), 2);
        assert_eq!(held(50), []);
    }

    /// Two pages share a stream, with the same resources: it's read once,
    /// as the first held page's, made as it is; else as the stream is, at
    /// the first page's path; and with neither held, it's found in by
    /// nothing, and counted as read by nothing.
    #[test]
    fn content_is_found_in_as_its_holder_or_its_stream_is_held() {
        let mut objects = tree("3 0 R 4 0 R", "/Resources << >>");
        objects.extend([
            (3, page("/Contents 10 0 R")),
            (4, page("/Contents 10 0 R")),
            (10, stream("", "q")),
        ]);
        let file = file(&objects);
        let open = "1 q left open at the end".to_string();
        let held = |objects: &[(u32, Made)]| {
            let hold = Hold::Only(objects.iter().map(|&(n, made)| ((n, 0), made)).collect());
            let found: Vec<_> = (found_held(&file, &hold).into_iter())
                .map(|(rule, made, n, path, value, _)| (rule, made, n, path, value))
                .collect();
            let opened = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
            (found, check(&opened, &hold, &Options::default()).content)
        };
        let first = "Root/Pages/Kids[0]/Contents".to_string();
        let second = "Root/Pages/Kids[1]/Contents".to_string();
        let found = |made, path: &String| (Rule::Balance, made, 10, path.clone(), open.clone());
        assert_eq!(
            held(&[(3, Made::Built)]),
            (vec![found(Made::Built, &first)], 1)
        );
        assert_eq!(
            held(&[(10, Made::Merged)]),
            (vec![found(Made::Merged, &first)], 1)
        );
        assert_eq!(held(&[(1, Made::Built)]), (vec![], 0));
        assert_eq!(
            held(&[(4, Made::Built)]),
            (vec![found(Made::Built, &second)], 1)
        );
        assert_eq!(
            held(&[(4, Made::Built), (10, Made::Merged)]),
            (vec![found(Made::Built, &second)], 1)
        );
    }

    /// A content finding is carried by a like one, whose count, offset or
    /// number of times differs: by number, by path and from a source; one of
    /// a kind the earlier report hasn't is the file's own.
    #[test]
    fn content_findings_are_carried_whatever_their_counts() {
        let report = |content: &str| {
            let file = page_of(&[content], &[]);
            let opened = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
            let options = Options {
                paths: true,
                ..Options::default()
            };
            check(&opened, &Hold::All(Made::Merged), &options)
        };
        let values = |found: Vec<&Finding>| -> Vec<String> {
            found.into_iter().map(|f| f.value.clone()).collect()
        };
        let before = report("Q q 0 0 m S EMC");
        let after = report("0 0 m S Q Q q q EMC EMC");
        assert_eq!(after.findings.len(), 3, "{after}");
        assert_eq!(values(after.since(&before)), Vec::<String>::new());
        assert_eq!(values(after.by_path(&before)), Vec::<String>::new());
        assert_eq!(values(after.beyond(&[&before])), Vec::<String>::new());
        let stopped = report("Q q 0 0 m S EMC (abc");
        let new = ["a string that doesn't end: content is read no further"];
        assert_eq!(values(stopped.since(&before)), new);
        assert_eq!(values(stopped.by_path(&before)), new);
        assert_eq!(values(stopped.beyond(&[&before])), new);
    }

    /// A unit whose data doesn't decode cleanly isn't read, and is counted
    /// as not checked, its column `-`, by why, where it's held; and so is one
    /// past the budget.
    #[test]
    fn a_unit_that_isnt_read_is_counted_as_not_checked() {
        let mut objects = tree("3 0 R 4 0 R", "");
        objects.extend([
            (3, page("/Contents 10 0 R")),
            (4, page("/Contents 11 0 R")),
            (10, stream("/Filter /FlateDecode", "not zlib")),
            (11, stream("", "q q q q q q")),
        ]);
        let file = file(&objects);
        let opened = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
        let not_checked = |hold: &Hold, budget| {
            let options = Options {
                content_budget: budget,
                ..Options::default()
            };
            let report: Report = check(&opened, hold, &options);
            let unchecked: Vec<_> = (report.unchecked.into_iter())
                .map(|(u, n)| (u.why, u.table, u.key, n))
                .collect();
            (unchecked, report.content)
        };
        let unchecked = |why: &str| {
            (
                why.to_string(),
                "PageObject".to_string(),
                "Contents".to_string(),
                1,
            )
        };
        let all = Hold::All(Made::Merged);
        assert_eq!(not_checked(&all, None), (vec![unchecked(DECODE)], 1));
        assert_eq!(
            not_checked(&all, Some(5)),
            (vec![unchecked(DECODE), unchecked(BUDGET)], 0)
        );
        let second = Hold::Only([((4, 0), Made::Built)].into());
        assert_eq!(not_checked(&second, None), (vec![], 1));
        assert_eq!(not_checked(&second, Some(5)), (vec![unchecked(BUDGET)], 0));
        // The column `-`.
        let report = check(&opened, &all, &Options::default());
        let columns: Vec<_> = report
            .unchecked
            .keys()
            .map(|u: &Unchecked| u.column)
            .collect();
        assert_eq!(columns, ["-"]);
    }

    /// An encrypted file's content is read as the reader decrypts it: its
    /// finding's bytes are its plaintext, and none of its document alone,
    /// which gives no stream's data.
    #[test]
    fn an_encrypted_files_content_is_read_decrypted() {
        use crate::pdf_crypt::writer::{self, Kind, Settings};
        use crate::pdf_crypt::{Encoded, Handler};

        let id = b"0123456789abcdef";
        let settings = Settings {
            kind: Kind::Aes256,
            user: b"",
            owner: b"owner",
            p: -4,
            id,
            encrypt_metadata: true,
        };
        let dictionary = writer::dictionary(&settings);
        let handler = Handler::open(&dictionary, id, &[Encoded::verbatim(b"")]).unwrap();
        let content = b"0 0 m 9 9 l S q";
        let data = writer::encrypt_stream(&handler, (10, 0), content, None, [7; 16]).unwrap();
        let hex = |b: &[u8]| b.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let d = &dictionary;
        let mut entries = format!("/Filter /Standard /V {} /R {} /P -4", d.v, d.r);
        if let Some(length) = d.length {
            entries.push_str(&format!(" /Length {length}"));
        }
        for (name, value) in [
            ("O", &d.o),
            ("U", &d.u),
            ("OE", &d.oe),
            ("UE", &d.ue),
            ("Perms", &d.perms),
        ] {
            if let Some(value) = value {
                entries.push_str(&format!(" /{name} <{}>", hex(value)));
            }
        }
        entries
            .push_str(" /CF << /StdCF << /CFM /AESV3 /Length 32 >> >> /StmF /StdCF /StrF /StdCF");
        let mut objects: Vec<(u32, Vec<u8>)> = (tree("3 0 R", "/Resources << >>").into_iter())
            .map(|(n, body)| (n, body.into_bytes()))
            .collect();
        objects.push((3, page("/Contents 10 0 R").into_bytes()));
        let mut body = format!("<< /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(&data);
        body.extend_from_slice(b"\nendstream");
        objects.push((10, body));
        let objects: Vec<(u32, &[u8])> = objects.iter().map(|(n, b)| (*n, &b[..])).collect();
        let trailer = format!(
            "<< /Size 11 /Root 1 0 R /Encrypt << {entries} >> /ID [<{0}> <{0}>] >>",
            hex(id)
        );
        let file = fixture::assembled("1.7", &objects, &trailer);
        assert!(!file.windows(content.len()).any(|w| w == content));
        let open = "1 q left open at the end";
        assert_eq!(found(&file), [at(Rule::Balance, 10, None, open, "q")]);
        let opened = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
        let all = Hold::All(Made::Merged);
        let alone = check(opened.document(), &all, &Options::default());
        let content = alone.findings.iter().any(|f| counted(f.rule, &f.key));
        assert!(!content, "{alone}");
    }

    /// A Type 1 font, for resources that hold one.
    const FONT: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";

    /// A file of one page, object 3, whose content is `content`, object 10,
    /// with `more` in its dictionary, its resources among it, and `objects`
    /// besides, each its number and body.
    fn resourced(more: &str, content: String, objects: &[(u32, &str)]) -> Vec<u8> {
        let mut all = tree("3 0 R", "");
        all.push((3, page(&format!("/Contents 10 0 R {more}"))));
        all.push((10, content));
        all.extend(objects.iter().map(|&(n, body)| (n, body.to_string())));
        file(&all)
    }

    /// What a check of `file` finds of `rule`, every object held and unused
    /// resources found: each finding's object, path, table and key, value,
    /// and its bytes, in its stream's decoded data or in the file; and the
    /// report.
    #[allow(clippy::type_complexity)]
    fn found_by(rule: Rule, file: &[u8]) -> (Vec<(u32, String, String, String, String)>, Report) {
        let opened = WithReading::new(file, &[], &bytes::Options::DEFAULT).unwrap();
        let options = Options {
            unused: true,
            ..Options::default()
        };
        let report = check(&opened, &Hold::All(Made::Merged), &options);
        let found = (report.findings.iter())
            .filter(|f| f.rule == rule)
            .map(|f| {
                let bytes = match f.at {
                    Some(
                        at @ Span {
                            stream: Some(id), ..
                        },
                    ) => {
                        let data = opened.decoded_within(id, usize::MAX).unwrap().unwrap();
                        data[at.start..at.end].to_vec()
                    }
                    Some(at) => file[at.start..at.end].to_vec(),
                    None => Vec::new(),
                };
                (
                    f.object.unwrap().0,
                    f.path.clone(),
                    format!("{}/{}", f.table, f.key),
                    f.value.clone(),
                    String::from_utf8_lossy(&bytes).into_owned(),
                )
            })
            .collect();
        (found, report)
    }

    /// What `rule` finds in `file`, as [`found_by`] has it: each finding's
    /// table and key, value and bytes.
    fn named(rule: Rule, file: &[u8]) -> Vec<(String, String, String)> {
        (found_by(rule, file).0.into_iter())
            .map(|(_, _, at, value, bytes)| (at, value, bytes))
            .collect()
    }

    /// A finding's table and key, value and bytes.
    fn lacks(at: &str, value: &str, bytes: &str) -> (String, String, String) {
        (at.to_string(), value.to_string(), bytes.to_string())
    }

    /// The checks `report` didn't make for `why`: each table and key, and
    /// how many.
    fn not_made(report: &Report, why: &str) -> Vec<(String, String, usize)> {
        (report.unchecked.iter())
            .filter(|(u, _)| u.why == why)
            .map(|(u, &n)| (u.table.clone(), u.key.clone(), n))
            .collect()
    }

    /// The name a resource is looked up by is the operand `resource()`
    /// names it by: for each operator that names one, PDFium's reading of
    /// the operand is `resource()`'s name, but for an inline image's
    /// abbreviation, which `resource()` gives as the space it stands for,
    /// `I` as `Indexed`. ISO's reading of it, the key looked up, decodes
    /// `#41`. What names no resource, a device space, `/Pattern` or an
    /// inline image's abbreviation of one, isn't looked up.
    #[test]
    fn the_name_looked_up_is_the_operand_resource_names() {
        let content: &[u8] = b"/F#41 9 Tf /X#41 Do /G#41 gs /S#41 sh /C#41 cs /C#41 CS \
            /P#41 scn 0 /P#41 SCN /T /P#41 BDC EMC /T /P#41 DP \
            BI /CS /C#41 /W 1 /H 1 /BPC 8 /F /AHx ID 00> EI \
            BI /ColorSpace /I /W 1 /H 1 /BPC 8 /F /AHx ID 00> EI \
            /DeviceRGB cs /Pattern CS 0.5 scn BI /CS /G /W 1 /H 1 /BPC 8 /F /AHx ID 00> EI";
        let (res, limits) = (ResCx::none(), limits());
        let mut named = Vec::new();
        for op in pdf_content_streams::operations(content, &res, &limits) {
            let op = op.unwrap();
            let Some((kind, name)) = op.resource() else {
                continue;
            };
            let written = written(&op).unwrap();
            let pdfium = pdf_content_streams::name(written.bytes()).unwrap();
            let operator = String::from_utf8_lossy(op.name());
            match written.bytes() {
                b"/I" => assert_eq!(&*name, b"Indexed"),
                _ => assert_eq!(pdfium, name, "{operator}"),
            }
            let iso = iso_name(written.bytes()).unwrap().into_owned();
            named.push((kind, String::from_utf8(iso).unwrap()));
        }
        let named: Vec<(ResKind, &str)> = named.iter().map(|(k, n)| (*k, n.as_str())).collect();
        assert_eq!(
            named,
            [
                (ResKind::Font, "FA"),
                (ResKind::XObject, "XA"),
                (ResKind::ExtGState, "GA"),
                (ResKind::Shading, "SA"),
                (ResKind::ColorSpace, "CA"),
                (ResKind::ColorSpace, "CA"),
                (ResKind::Pattern, "PA"),
                (ResKind::Pattern, "PA"),
                (ResKind::Properties, "PA"),
                (ResKind::Properties, "PA"),
                (ResKind::ColorSpace, "CA"),
                (ResKind::ColorSpace, "I"),
            ]
        );
        // ISO's reading: a `#` before what aren't two hex digits is itself.
        assert_eq!(iso_name(b"/F#3G").as_deref(), Some(&b"F#3G"[..]));
        assert_eq!(iso_name(b"/F#30").as_deref(), Some(&b"F0"[..]));
        assert_eq!(iso_name(b"12"), None);
    }

    /// A name each operator that names a resource uses, which its category
    /// lacks, is found, once in a unit, at the name as written, its table
    /// `Resource` and its key the category, in the content stream; the
    /// device spaces and `/Pattern` are no resources. A page with no
    /// resources at all, its own or its parents', lacks every name it uses.
    #[test]
    fn a_name_its_resources_lack_is_found_by_each_operator() {
        let content = "/F1 9 Tf /X1 Do /G1 gs /S1 sh /C1 cs /C2 CS /P1 scn 0 0 0 /P2 SCN \
                       /T /M1 BDC EMC /T /M2 DP BI /CS /C3 /W 1 /H 1 /BPC 8 /F /AHx ID 00> EI \
                       /DeviceGray cs /Pattern cs /DeviceCMYK CS \
                       BI /CS /RGB /W 1 /H 1 /BPC 8 /F /AHx ID 00> EI /F1 12 Tf";
        let resources = "/Resources << /ProcSet [/PDF] >>";
        let file = resourced(resources, stream("", content), &[]);
        assert_eq!(
            named(Rule::Resource, &file),
            [
                lacks("Resource/Font", "/F1 (Tf)", "/F1"),
                lacks("Resource/XObject", "/X1 (Do)", "/X1"),
                lacks("Resource/ExtGState", "/G1 (gs)", "/G1"),
                lacks("Resource/Shading", "/S1 (sh)", "/S1"),
                lacks("Resource/ColorSpace", "/C1 (cs)", "/C1"),
                lacks("Resource/ColorSpace", "/C2 (CS)", "/C2"),
                lacks("Resource/Pattern", "/P1 (scn)", "/P1"),
                lacks("Resource/Pattern", "/P2 (SCN)", "/P2"),
                lacks("Resource/Properties", "/M1 (BDC)", "/M1"),
                lacks("Resource/Properties", "/M2 (DP)", "/M2"),
                lacks("Resource/ColorSpace", "/C3 (BI)", "/C3"),
            ]
        );
        let (found, _) = found_by(Rule::Resource, &file);
        assert_eq!(
            (found[0].0, found[0].1.as_str()),
            (10, "Root/Pages/Kids[0]/Contents")
        );
        let none = resourced("", stream("", "/F1 9 Tf /F1 9 Tf /F2 9 Tf"), &[]);
        let without = "and it has no resources";
        assert_eq!(
            named(Rule::Resource, &none),
            [
                lacks("Resource/Font", &format!("/F1 (Tf), {without}"), "/F1"),
                lacks("Resource/Font", &format!("/F2 (Tf), {without}"), "/F2"),
            ]
        );
    }

    /// Resources are read as ISO 32000-2 has them, not as PDFium reads
    /// them: through any references, three among them, as 7.3.10 lets them
    /// be, their names there. What should be a dictionary and isn't,
    /// resources or a category, a stream given for one, a number, an array,
    /// or a category that refers to nothing, has names that can't be told:
    /// none is found lacking, and each unit that looks one up is counted as
    /// not checked, by why.
    #[test]
    fn resources_are_read_through_references_and_never_lack_where_damaged() {
        let chain = [
            (40, "41 0 R"),
            (41, "42 0 R"),
            (42, "<< /Font 43 0 R /ColorSpace 45 0 R >>"),
            (43, "44 0 R"),
            (44, "<< /F1 50 0 R >>"),
            (45, "46 0 R"),
            (46, "47 0 R"),
            (47, "<< /CS1 [/CalGray << /WhitePoint [1 1 1] >>] >>"),
            (50, FONT),
        ];
        let used = stream("", "/F1 9 Tf /F2 9 Tf /CS1 cs /CS2 cs");
        let file = resourced("/Resources 40 0 R", used, &chain);
        assert_eq!(
            named(Rule::Resource, &file),
            [
                lacks("Resource/Font", "/F2 (Tf)", "/F2"),
                lacks("Resource/ColorSpace", "/CS2 (cs)", "/CS2"),
            ]
        );
        let tf = stream("", "/F1 9 Tf /F2 9 Tf");
        let as_stream = stream("/Font << /F1 50 0 R >>", "");
        let fonts_as_stream = stream("/F1 50 0 R", "");
        let cases = [
            ("/Resources 40 0 R", tf.clone(), as_stream.as_str(), DAMAGED),
            ("/Resources 5", tf.clone(), "null", DAMAGED),
            (
                "/Resources << /Font 40 0 R >>",
                tf.clone(),
                &fonts_as_stream,
                CATEGORY,
            ),
            ("/Resources << /Font [/F1] >>", tf.clone(), "null", CATEGORY),
            (
                "/Resources << /Font 99 0 R >>",
                tf.clone(),
                "null",
                CATEGORY,
            ),
            (
                "/Resources << /ColorSpace 5 >>",
                stream("", "/C1 cs"),
                "null",
                CATEGORY,
            ),
        ];
        for (resources, content, forty, why) in cases {
            let file = resourced(resources, content, &[(40, forty), (50, FONT)]);
            let (found, report) = found_by(Rule::Resource, &file);
            assert_eq!(found, [], "{resources}");
            let counted = [("PageObject".to_string(), "Resources".to_string(), 1)];
            assert_eq!(not_made(&report, why), counted, "{resources}");
        }
    }

    /// A category the resources don't have, or that's null, lacks every
    /// name, and so does a name whose value is null (ISO 32000-2, 7.3.7);
    /// but a name that refers to nothing is there, the walk finding its
    /// reference unresolved, and no `resource` finding is made of it.
    #[test]
    fn a_category_absent_or_null_lacks_and_a_reference_to_nothing_is_there() {
        let tf = || stream("", "/F1 9 Tf");
        let lacking = [lacks("Resource/Font", "/F1 (Tf)", "/F1")];
        for resources in [
            "/Resources << >>",
            "/Resources << /Font null >>",
            "/Resources << /Font 40 0 R >>",
            "/Resources << /Font << /F1 null >> >>",
            "/Resources << /Font << /F1 40 0 R >> >>",
        ] {
            let file = resourced(resources, tf(), &[(40, "null")]);
            assert_eq!(named(Rule::Resource, &file), lacking, "{resources}");
        }
        let nothing = resourced("/Resources << /Font << /F1 99 0 R >> >>", tf(), &[]);
        assert_eq!(named(Rule::Resource, &nothing), []);
        let unresolved: Vec<_> = (named(Rule::Unresolved, &nothing).into_iter())
            .map(|(at, value, _)| (at, value))
            .collect();
        assert_eq!(unresolved.len(), 1, "{unresolved:?}");
        assert!(unresolved[0].1.contains("99 0 R"), "{unresolved:?}");
    }

    /// Names are read as ISO 32000-2 reads them, content's and the
    /// resources' keys alike, where PDFium reads a `#` before what aren't
    /// two hex digits otherwise: `/F#3G` is `F#3G`, not PDFium's `F0`, so
    /// in content it lacks from resources that have `/F0`, a `content`
    /// finding saying where the readings part, and as a key it doesn't
    /// hold the `/F0` content uses, a `reader` finding saying so; `/F#30` is
    /// `F0` to both, either way.
    #[test]
    fn names_are_read_as_iso_reads_them_on_both_sides() {
        let checked = |key: &str, used: &str| {
            let resources = format!("/Resources << /Font << {key} 50 0 R >> >>");
            let content = stream("", &format!("BT {used} 9 Tf (a) Tj ET"));
            let file = resourced(&resources, content, &[(50, FONT)]);
            let values = |rule| -> Vec<String> {
                (named(rule, &file).into_iter())
                    .map(|(_, value, _)| value)
                    .collect()
            };
            (
                named(Rule::Resource, &file),
                values(Rule::Content),
                values(Rule::Reader),
            )
        };
        let (lacking, content, reader) = checked("/F0", "/F#3G");
        assert_eq!(lacking, [lacks("Resource/Font", "/F#3G (Tf)", "/F#3G")]);
        assert_eq!(content.len(), 1, "{content:?}");
        assert!(content[0].starts_with("the name /F#3G has"), "{content:?}");
        assert_eq!(reader, Vec::<String>::new());
        let (lacking, content, reader) = checked("/F#3G", "/F0");
        assert_eq!(lacking, [lacks("Resource/Font", "/F0 (Tf)", "/F0")]);
        assert_eq!(content, Vec::<String>::new());
        assert_eq!(reader.len(), 1, "{reader:?}");
        for (key, used) in [("/F0", "/F#30"), ("/F#30", "/F0")] {
            let (lacking, content, reader) = checked(key, used);
            assert_eq!(lacking, [], "{key} {used}");
            assert_eq!((content, reader), (vec![], vec![]), "{key} {used}");
        }
    }

    /// An inline image's `/I`, which PDFium looks up as `Indexed`, the
    /// space it abbreviates, is looked up as written, `I`: 8.9.7's
    /// abbreviations are of a space's family, not of a name the resources
    /// map.
    #[test]
    fn an_inline_images_i_is_looked_up_as_written() {
        let image = || stream("", "BI /CS /I /W 1 /H 1 /BPC 8 /F /AHx ID 00> EI");
        let space = "[/Indexed /DeviceRGB 0 <000000>]";
        let with = |key: &str| {
            let resources = format!("/Resources << /ColorSpace << /{key} {space} >> >>");
            named(Rule::Resource, &resourced(&resources, image(), &[]))
        };
        assert_eq!(
            with("Indexed"),
            [lacks("Resource/ColorSpace", "/I (BI)", "/I")]
        );
        assert_eq!(with("I"), []);
    }

    /// A name in resources two pages inherit, one of them using it, is
    /// used; one neither uses is found unused, in the page tree's node that
    /// holds the resources, at its path and its key, and counted, in the
    /// report's display too; and without [`Options::unused`], counted only.
    #[test]
    fn a_shared_resources_name_one_page_uses_is_used() {
        let image = stream(
            "/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray \
             /BitsPerComponent 8",
            "x",
        );
        let resources = "/Resources << /XObject << /Im 20 0 R /Spare 21 0 R >> >>";
        let mut objects = tree("3 0 R 4 0 R", resources);
        objects.extend([
            (3, page("/Contents 10 0 R")),
            (4, page("/Contents 11 0 R")),
            (10, stream("", "q 9 0 0 9 0 0 cm /Im Do Q")),
            (11, stream("", "0 0 m 9 9 l S")),
            (20, image.clone()),
            (21, image),
        ]);
        let file = file(&objects);
        let (found, report) = found_by(Rule::Unused, &file);
        let spare = (
            2,
            "Root/Pages/Resources/XObject/Spare".to_string(),
            "Resource/XObject".to_string(),
            "/Spare".to_string(),
            "/Spare".to_string(),
        );
        assert_eq!(found, [spare]);
        let counted: BTreeMap<_, _> = [(("Resource".to_string(), "XObject".to_string()), 1)].into();
        assert_eq!(report.unused, counted);
        assert!(
            (report.to_string()).contains("\n  unused: Resource/XObject: 1\n"),
            "{report}"
        );
        let opened = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
        let report = check(&opened, &Hold::All(Made::Merged), &Options::default());
        assert!(!report.findings.iter().any(|f| f.rule == Rule::Unused));
        assert_eq!(report.unused, counted);
    }

    /// A category that's an object of its own is one category, whatever
    /// resources hold it: a page's and a form's that share their fonts use
    /// what either uses, and the one neither uses is found once, in that
    /// object, at the path the walk first reached it by.
    #[test]
    fn an_indirect_category_two_resources_share_is_one() {
        let form = stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 9 9] /Resources << /Font 30 0 R >>",
            "BT /F2 9 Tf (a) Tj ET",
        );
        let resources = "/Resources << /Font 30 0 R /XObject << /Fm 20 0 R >> >>";
        let content = stream("", "BT /F1 9 Tf (a) Tj ET /Fm Do");
        let fonts = "<< /F1 50 0 R /F2 50 0 R /F3 50 0 R >>";
        let file = resourced(resources, content, &[(20, &form), (30, fonts), (50, FONT)]);
        let (found, _) = found_by(Rule::Unused, &file);
        let f3 = (
            30,
            "Root/Pages/Kids[0]/Resources/Font/F3".to_string(),
            "Resource/Font".to_string(),
            "/F3".to_string(),
            "/F3".to_string(),
        );
        assert_eq!(found, [f3]);
    }

    /// A form with no resources of its own would use its page's, which
    /// isn't told here: its names aren't found lacking, it's counted as not
    /// checked, and what it uses is used in every category of its kind.
    #[test]
    fn a_form_with_no_resources_spares_what_it_uses() {
        let form = stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 9 9]",
            "BT /F1 9 Tf (a) Tj ET",
        );
        let resources =
            "/Resources << /Font << /F1 50 0 R /F2 50 0 R >> /XObject << /Fm 20 0 R >> >>";
        let file = resourced(resources, stream("", "/Fm Do"), &[(20, &form), (50, FONT)]);
        let (found, report) = found_by(Rule::Unused, &file);
        let unused: Vec<_> = found.iter().map(|f| f.3.as_str()).collect();
        assert_eq!(unused, ["/F2"]);
        assert_eq!(named(Rule::Resource, &file), []);
        let counted = [("XObjectFormType1".to_string(), "Resources".to_string(), 1)];
        assert_eq!(not_made(&report, UNOWNED), counted);
    }

    /// `DefaultGray`, `DefaultRGB` and `DefaultCMYK` are used by content
    /// that sets a device space, without being named (ISO 32000-2,
    /// 8.6.5.6): never unused.
    #[test]
    fn the_default_spaces_are_never_unused() {
        let cal = "<< /WhitePoint [1 1 1] >>";
        let resources = format!(
            "/Resources << /ColorSpace << /DefaultRGB [/CalRGB {cal}] /DefaultGray [/CalGray {cal}] \
             /DefaultCMYK [/ICCBased 60 0 R] /Spare [/CalGray {cal}] >> >>"
        );
        let file = resourced(&resources, stream("", "1 0 0 rg 0 0 9 9 re f"), &[]);
        let unused: Vec<_> = (found_by(Rule::Unused, &file).0.into_iter())
            .map(|f| f.3)
            .collect();
        assert_eq!(unused, ["/Spare"]);
    }

    /// An unused resource is found and counted where the object it's in is
    /// held, made as it's held: the category's own object, else its
    /// resources', else the one that holds them; or where content that has
    /// it is held, made as that is; and neither where none is. A name the
    /// resources lack is found as content's findings are, where its page or
    /// its stream is held, or what its resources are written in, all of the
    /// content then.
    #[test]
    fn resource_and_unused_findings_are_held_as_their_objects_are() {
        let content = || stream("", "/X1 Do");
        let fonts = "<< /F1 50 0 R >>";
        let direct = resourced(
            "/Resources << /Font << /F1 50 0 R >> >>",
            content(),
            &[(50, FONT)],
        );
        let category = resourced(
            "/Resources << /Font 30 0 R >>",
            content(),
            &[(30, fonts), (50, FONT)],
        );
        let resources = "<< /Font << /F1 50 0 R >> >>";
        let own = resourced(
            "/Resources 40 0 R",
            content(),
            &[(40, resources), (50, FONT)],
        );
        let found = |file: &[u8], held: &[(u32, Made)]| {
            let opened = WithReading::new(file, &[], &bytes::Options::DEFAULT).unwrap();
            let options = Options {
                unused: true,
                ..Options::default()
            };
            let hold = Hold::Only(held.iter().map(|&(n, made)| ((n, 0), made)).collect());
            let report = check(&opened, &hold, &options);
            let of = |rule| -> Vec<(u32, Made)> {
                (report.findings.iter())
                    .filter(|f| f.rule == rule)
                    .map(|f| (f.object.unwrap().0, f.made))
                    .collect()
            };
            let counted: usize = report.unused.values().sum();
            (of(Rule::Unused), counted, of(Rule::Resource))
        };
        let built = Made::Built;
        let merged = Made::Merged;
        assert_eq!(
            found(&direct, &[(3, built)]),
            (vec![(3, built)], 1, vec![(10, built)])
        );
        assert_eq!(
            found(&direct, &[(10, merged)]),
            (vec![(3, merged)], 1, vec![(10, merged)])
        );
        assert_eq!(found(&direct, &[(50, built)]), (vec![], 0, vec![]));
        assert_eq!(
            found(&category, &[(30, merged)]),
            (vec![(30, merged)], 1, vec![(10, merged)])
        );
        assert_eq!(
            found(&category, &[(3, built)]),
            (vec![(30, built)], 1, vec![(10, built)])
        );
        let both = (vec![(40, built)], 1, vec![(10, built)]);
        assert_eq!(found(&own, &[(40, built)]), both);
        assert_eq!(found(&own, &[(3, built)]), both);
    }

    /// A category one of whose units of content isn't read, its data not
    /// decoding cleanly, or isn't read to its end, can't be told: what it
    /// holds that nothing read uses is counted as not checked, a name each,
    /// and not found; another page's use of it is a use all the same.
    #[test]
    fn a_category_of_content_not_read_whole_is_untold() {
        let resources = "/Resources << /Font << /F1 50 0 R /F2 50 0 R /F3 50 0 R >> >>";
        for (second, untold) in [
            (stream("/Filter /FlateDecode", "not zlib"), 2),
            (stream("", "BT /F2 9 Tf (a) Tj ET (abc"), 1),
        ] {
            let mut objects = tree("3 0 R 4 0 R", resources);
            objects.extend([
                (3, page("/Contents 10 0 R")),
                (4, page("/Contents 11 0 R")),
                (10, stream("", "BT /F1 9 Tf (a) Tj ET")),
                (11, second),
                (50, FONT.to_string()),
            ]);
            let (found, report) = found_by(Rule::Unused, &file(&objects));
            assert_eq!(found, []);
            assert_eq!(report.unused, BTreeMap::new());
            let counted = [("Resource".to_string(), "Font".to_string(), untold)];
            assert_eq!(not_made(&report, UNTOLD), counted);
        }
    }

    /// `resource` and `unused` findings are alike by their names: a report
    /// with the same name lacking, and the same unused, carries them, by
    /// number, by path and from a source, wherever and however often the
    /// name is used; another name is the file's own.
    #[test]
    fn resource_and_unused_findings_are_carried_by_their_names() {
        let report = |content: &str, spare: &str| {
            let resources = format!("/Resources << /Font << /F1 50 0 R /{spare} 50 0 R >> >>");
            let file = resourced(&resources, stream("", content), &[(50, FONT)]);
            let opened = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
            let options = Options {
                unused: true,
                paths: true,
                ..Options::default()
            };
            check(&opened, &Hold::All(Made::Merged), &options)
        };
        let values = |found: Vec<&Finding>| -> Vec<String> {
            (found.into_iter())
                .filter(|f| matches!(f.rule, Rule::Resource | Rule::Unused))
                .map(|f| f.value.clone())
                .collect()
        };
        let before = report("BT /F1 9 Tf (a) Tj /F9 9 Tf (b) Tj ET", "F2");
        let all: Vec<&Finding> = before.findings.iter().collect();
        assert_eq!(values(all), ["/F9 (Tf)", "/F2"]);
        let after = report(
            "0 0 m 9 9 l S BT /F1 9 Tf /F9 12 Tf (b) Tj /F9 9 Tf (c) Tj ET",
            "F2",
        );
        assert_eq!(values(after.since(&before)), Vec::<String>::new());
        assert_eq!(values(after.by_path(&before)), Vec::<String>::new());
        assert_eq!(values(after.beyond(&[&before])), Vec::<String>::new());
        let other = report("BT /F1 9 Tf (a) Tj /F8 9 Tf (b) Tj ET", "F3");
        let new = ["/F8 (Tf)", "/F3"];
        assert_eq!(values(other.since(&before)), new);
        assert_eq!(values(other.by_path(&before)), new);
        assert_eq!(values(other.beyond(&[&before])), new);
        // Past 32 names, what says how many more is carried whatever the
        // count, as no name is in it.
        let names = |n: usize, again: &str| -> String {
            let used: String = (0..n).map(|i| format!("/X{i} 9 Tf ")).collect();
            format!("BT {used}{again}ET")
        };
        let before = report(&names(34, ""), "F2");
        let more = format!("2 more, of kinds past the first {KINDS}");
        assert!(values(before.findings.iter().collect()).contains(&more));
        let after = report(&names(34, "/X33 9 Tf "), "F2");
        assert_eq!(values(after.since(&before)), Vec::<String>::new());
    }

    /// Where a unit of content whose names can be in any category, as it has
    /// no resources of its own, isn't read to its end, no category is told:
    /// what each holds that nothing read uses is counted as not checked,
    /// not found unused, as the unit may use it.
    #[test]
    fn content_with_no_resources_not_read_tells_no_category() {
        let report = |form: &str| {
            let form = format!("/Type /XObject /Subtype /Form /BBox [0 0 9 9] {form}");
            let resources = "/Resources << /Font << /F1 50 0 R >> /XObject << /Fm 20 0 R >> >>";
            let mut objects = tree("3 0 R", resources);
            objects.extend([
                (3, page("/Contents 10 0 R")),
                (10, stream("", "/Fm Do")),
                (20, stream(&form, "BT /F1 9 Tf (a) Tj ET")),
                (50, FONT.to_string()),
            ]);
            let file = file(&objects);
            let opened = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
            let options = Options {
                unused: true,
                ..Options::default()
            };
            check(&opened, &Hold::All(Made::Merged), &options)
        };
        let read = report("");
        assert_eq!(read.unused, BTreeMap::new());
        assert_eq!(not_made(&read, UNTOLD), []);
        let unread = report("/Filter /FlateDecode");
        assert_eq!(unread.unused, BTreeMap::new());
        assert!(!unread.findings.iter().any(|f| f.rule == Rule::Unused));
        let font = [("Resource".to_string(), "Font".to_string(), 1)];
        assert_eq!(not_made(&unread, UNTOLD), font);
    }

    /// A source that gives a stream's data but not another's: the unit that
    /// one is in isn't read, counted as not checked, and the rest are, the
    /// resources' unused names after them; and a document alone, with none
    /// of the budget, is still checked as one with no content read.
    #[test]
    fn a_stream_the_source_doesnt_give_is_counted_as_not_checked() {
        let resources = "/Resources << /Font << /Spare 50 0 R >> >>";
        let mut objects = tree("3 0 R 4 0 R 5 0 R", resources);
        objects.extend([
            (3, page("/Contents 10 0 R")),
            (4, page("/Contents 11 0 R")),
            (5, page("/Contents 12 0 R")),
            (10, stream("", "q Q")),
            (11, stream("", "q Q")),
            (12, stream("", "q")),
            (50, FONT.to_string()),
        ]);
        let file = file(&objects);
        let opened = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
        let check_without = |n| {
            let without = Counting {
                source: &opened,
                asked: RefCell::new(Vec::new()),
                without: Some((n, 0)),
            };
            let report = check(&without, &Hold::All(Made::Merged), &Options::default());
            let found: Vec<_> = (report.findings.iter())
                .filter(|f| counted(f.rule, &f.key))
                .map(|f| f.object.unwrap().0)
                .collect();
            let not_given: Vec<_> = (report.unchecked.iter())
                .filter(|(u, _)| u.why == UNGIVEN)
                .map(|(u, n)| (u.table.clone(), *n))
                .collect();
            (found, not_given, report.content, report.unused)
        };
        let unused = BTreeMap::from([(("Resource".to_string(), "Font".to_string()), 1)]);
        let (found, not_given, content, counted) = check_without(11);
        assert_eq!((found, content), (vec![12], 2));
        assert_eq!(not_given, [("PageObject".to_string(), 1)]);
        assert_eq!(counted, BTreeMap::new());
        let (found, not_given, content, counted) = check_without(12);
        assert_eq!((found, content, counted), (vec![], 2, BTreeMap::new()));
        assert_eq!(not_given, [("PageObject".to_string(), 1)]);
        // The first asked for: as a source that gives none.
        let (found, not_given, content, counted) = check_without(10);
        assert_eq!((found, not_given, content), (vec![], vec![], 0));
        assert_eq!(counted, BTreeMap::new());
        let (.., counted) = check_without(99);
        assert_eq!(counted, unused);
        // A document alone, none of the budget given it.
        let options = Options {
            content_budget: Some(0),
            ..Options::default()
        };
        let all = Hold::All(Made::Merged);
        let alone = check(opened.document(), &all, &options);
        let plain = check(opened.document(), &all, &Options::default());
        assert_eq!(alone, plain);
    }
}
