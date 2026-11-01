//! A caller's graph to Annex F's order, the new numbers and the object
//! streams' members (ISO 32000-2, F.3).
//!
//! A linearized file is two groups of objects in eleven parts. The dictionary,
//! the first page's cross-reference section, what a viewer wants at open
//! ([`Part::Open`]), the hint stream and the first page ([`Part::FirstPage`])
//! come first and take the highest numbers; then each other page with what only
//! it uses ([`Part::Pages`]), what pages share ([`Part::Shared`]), everything
//! else ([`Part::Other`]) and the main cross-reference section, numbered from 1.
//! A [`Graph`] says what a plan needs to know of the objects, and nothing here
//! sees a body: [`plan()`] gives what a caller serializes, in file order
//! ([`Plan::writes`]), under the numbers it gives ([`Plan::number`]).
//!
//! # Where an object goes
//!
//! [`Use`] says which pages reach an object, walking from each page and never
//! into another page, a page tree node or what a viewer has at open. The first
//! page's walk is kept as it went, as part 6, whatever a later page then reaches
//! of it: an object the first page shares stays with the first page (F.3.7,
//! F.4.3), which `Use` alone can't say. An object another page's walk alone
//! reaches is in that page's section, and one several reach is in part 8. An
//! object placed once stays there: part 4 is placed first, then the first page,
//! the other pages, part 8 and part 9.
//!
//! # Object streams
//!
//! [`group()`] is called once per region, what's packable there going to it, so a
//! region's object streams hold only its objects and a table can name it as one
//! stretch of the file. A stream with one later page's members alone stays in
//! that page's section; one with several pages' or shared members is in part 8,
//! one entry of the shared table, so what a stream holds costs a viewer one
//! fetch. Only a cross-reference stream lists a member, so under [`Kind::Table`]
//! nothing's packed. Each thumbnail and each embedded file is a region of its own,
//! so its table can name it.
//!
//! # What a plan costs
//!
//! The walk and the numbers take one pass over the objects and their
//! references, each reference found by a binary search once. Each page's list of
//! shared objects ([`Plan::shared`]) can't come from that walk, which enters an
//! object twice at most however many pages reach it: it's a walk of its own for
//! each later page, the sum of what each page reaches, which is what the hint
//! tables write, under [`Limits`].

use std::ops::Range;

mod group;
mod place;
mod shared;
#[cfg(test)]
mod tests;
mod uses;

pub use group::{MEMBERS, Use, group};
pub use uses::page_uses;

use crate::{Id, Limits};

/// What lists a file's objects, which decides the layout's own numbers and
/// whether anything can be packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Kind {
    /// Cross-reference tables: nothing can be packed.
    Table,
    /// Cross-reference streams, each an object of the layout's own, which may
    /// list members of object streams.
    Stream,
}

/// Which page goes first, in part 6, as the dictionary's `/P` says (F.3.7): what an
/// adapter's options take, which [`Graph::first_page`] is then told as a place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum FirstPage {
    /// The page the catalog's `/OpenAction` opens at, a destination or a `/GoTo`
    /// action's, named or not; page 0 where it opens at no page of this file.
    #[default]
    OpenAction,
    /// This page, from 0: past the last it's refused ([`Error::FirstPagePast`]).
    Index(u32),
}

/// The objects of a part that are grouped for a table of the hint stream, in the
/// order they're laid out (F.3.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Group {
    /// The document outline.
    Outlines,
    /// Article threads' information dictionaries.
    ThreadInfo,
    /// Named destinations.
    Dests,
    /// The document information dictionary.
    Info,
    /// The interactive form's fields.
    Form,
    /// The logical structure hierarchy.
    Structure,
    /// The renditions name tree.
    Renditions,
    /// Embedded file streams.
    EmbeddedFile,
    /// Anything else a caller would have together; no table names it.
    Other,
}

impl Group {
    /// Every group, in its order in the file.
    pub(crate) const ALL: [Group; 9] = [
        Group::Outlines,
        Group::ThreadInfo,
        Group::Dests,
        Group::Info,
        Group::Form,
        Group::Structure,
        Group::Renditions,
        Group::EmbeddedFile,
        Group::Other,
    ];

    /// Whether this group's table lists the shared objects it refers to
    /// (F.4.6). The embedded file table does too, by the file (F.4.7).
    fn lists_shared(self) -> bool {
        use Group::*;
        matches!(self, Form | Structure | Renditions)
    }
}

/// An object as a [`Graph`] holds it, which [`Graph::object`] gives back to say
/// more of it.
#[derive(Debug, Clone)]
pub struct Node {
    id: Id,
    refs: Range<usize>,
    packable: bool,
    late: bool,
}

impl Node {
    /// Whether the object may be in an object stream, which it may not until
    /// this says so: not a stream, and not at a generation or under an
    /// encryption that keeps it out. The catalog and a page are never packed,
    /// whatever this says (F.3.1).
    pub fn packable(&mut self, yes: bool) -> &mut Node {
        self.packable = yes;
        self
    }

    /// Whether the object is one a viewer can do without at first, an image or
    /// a font file, so it goes after the rest of its page's (F.3.7, F.3.8).
    pub fn late(&mut self, yes: bool) -> &mut Node {
        self.late = yes;
        self
    }
}

/// A page, as [`Graph::page`] takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Page {
    /// The page object.
    pub id: Id,
    /// Its content streams, in order.
    pub contents: Vec<Id>,
    /// Its thumbnail image, which goes in part 9 and which the page's walk
    /// doesn't go into, so it isn't among the page object's references
    /// (F.3.10).
    pub thumb: Option<Id>,
}

impl Page {
    /// The page at object `id`, with no content streams or thumbnail said yet.
    pub fn new(id: Id) -> Page {
        Page {
            id,
            contents: Vec::new(),
            thumb: None,
        }
    }
}

/// A document's objects as a plan needs them: flat, one list of references and a
/// range of it per object. Built up by its methods, which refuse nothing;
/// [`plan()`] does.
#[derive(Debug, Clone)]
pub struct Graph {
    catalog: Id,
    kind: Kind,
    nodes: Vec<Node>,
    refs: Vec<Id>,
    pages: Vec<Page>,
    first_page: u32,
    tree_nodes: Vec<Id>,
    open: Vec<Id>,
    groups: Vec<(Group, Vec<Id>)>,
    outlines_first: bool,
}

impl Graph {
    /// A graph of nothing yet, with `catalog` as its catalog, to be listed by
    /// `kind`.
    pub fn new(catalog: Id, kind: Kind) -> Graph {
        Graph {
            catalog,
            kind,
            nodes: Vec::new(),
            refs: Vec::new(),
            pages: Vec::new(),
            first_page: 0,
            tree_nodes: Vec::new(),
            open: Vec::new(),
            groups: Vec::new(),
            outlines_first: false,
        }
    }

    /// Object `id`, referring to `refs`, in the order a page wants them read
    /// (F.3.7): each object once. A reference to no object given here counts
    /// for nothing, as such a reference reads as null.
    pub fn object(&mut self, id: Id, refs: impl IntoIterator<Item = Id>) -> &mut Node {
        let start = self.refs.len();
        self.refs.extend(refs);
        self.nodes.push(Node {
            id,
            refs: start..self.refs.len(),
            packable: false,
            late: false,
        });
        self.nodes.last_mut().expect("just pushed")
    }

    /// The next page, in page order.
    pub fn page(&mut self, page: Page) {
        self.pages.push(page);
    }

    /// Which page opens first, by its place among the pages (0 unless this says
    /// otherwise): it's laid out first and named by the dictionary's `/P`
    /// (F.3.7).
    pub fn first_page(&mut self, index: u32) {
        self.first_page = index;
    }

    /// A page tree or document part node (given as a page, it's a page): no
    /// walk goes into it, and it's in part 9 (F.3.7, F.3.10).
    pub fn tree_node(&mut self, id: Id) {
        self.tree_nodes.push(id);
    }

    /// An object a viewer wants at open, placed in part 4 in the order given,
    /// after the catalog, and not gone into (F.3.5), so a page's walk stops at
    /// it and no page names it (given as a page, it's a page).
    pub fn open(&mut self, id: Id) {
        self.open.push(id);
    }

    /// What `roots` reach, of what's left when the pages are placed, under the
    /// same stops as a page's walk: laid out as `group`. Being given at all
    /// says the thing exists, even with nothing left to it (F.3.6).
    pub fn group(&mut self, group: Group, roots: impl IntoIterator<Item = Id>) {
        self.groups.push((group, roots.into_iter().collect()));
    }

    /// Whether the catalog's `/PageMode` is `/UseOutlines`, which puts
    /// [`Group::Outlines`] in part 6 (F.3.5, F.3.7).
    pub fn outlines_first(&mut self, yes: bool) {
        self.outlines_first = yes;
    }
}

/// The parts of a file that hold what a caller writes (F.3), in file order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Part {
    /// Part 4: the catalog and what a viewer wants at open.
    Open,
    /// Part 6: the first page and all it reaches, and the outlines under
    /// [`Graph::outlines_first`].
    FirstPage,
    /// Part 7: each other page and what it alone uses.
    Pages,
    /// Part 8: what later pages share.
    Shared,
    /// Part 9: everything else.
    Other,
}

/// What a [`Write`] writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    /// The object with this id in the caller's graph.
    Object(Id),
    /// An object stream, by its place in [`Plan::containers`].
    Container(usize),
}

/// One body a caller serializes, in file order: an object or an object stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Write {
    /// What's written.
    pub what: What,
    /// The number it's written at, at generation 0.
    pub number: u32,
    /// The part it's in.
    pub part: Part,
}

/// An embedded file stream's own group of objects, as its hint table has them
/// (F.4.7): each root of [`Group::EmbeddedFile`] is one.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileGroup {
    /// The root's number.
    pub number: u32,
    /// Its writes, what no group before it took: none where it's all elsewhere.
    pub writes: Range<usize>,
}

/// An object stream a caller packs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Container {
    /// Its number.
    pub number: u32,
    /// Its members in the order they're packed, each with the number it's
    /// listed at.
    pub members: Vec<(Id, u32)>,
}

/// Why a graph can't be planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A linearized file names a first page.
    #[error("no pages: a linearized file has a first page (ISO 32000-2, F.3.7)")]
    NoPages,
    /// A page given twice, which Annex F can't give two places.
    #[error("page object {} {} is given twice", .0.0, .0.1)]
    PageRepeated(Id),
    /// A page with no object.
    #[error("page {} {} isn't among the objects", .0.0, .0.1)]
    NoPage(Id),
    /// A catalog with no object.
    #[error("the catalog, {} {}, isn't among the objects", .0.0, .0.1)]
    NoCatalog(Id),
    /// The catalog given as a page, which belongs in part 4 as a catalog and in
    /// a page's section as a page.
    #[error("the catalog, {} {}, is given as a page", .0.0, .0.1)]
    CatalogIsPage(Id),
    /// An object given twice.
    #[error("object {} {} is given twice", .0.0, .0.1)]
    ObjectRepeated(Id),
    /// [`Graph::first_page`] past the pages.
    #[error("the first page is page {index}, of {pages}")]
    FirstPagePast {
        /// The place asked for.
        index: u32,
        /// How many pages there are.
        pages: usize,
    },
    /// A packable object under [`Kind::Table`]: only a stream lists a member.
    #[error("object {} {} may be packed, which cross-reference tables can't list", .0.0, .0.1)]
    PackedTable(Id),
    /// More pages than [`Limits::pages`].
    #[error("{pages} pages, past the cap of {limit}")]
    TooManyPages {
        /// How many were given.
        pages: usize,
        /// The cap.
        limit: usize,
    },
    /// More numbers than [`Limits::objects`], what's packed and the layout's
    /// own among them.
    #[error("{numbers} object numbers, past the cap of {limit}")]
    TooManyObjects {
        /// How many are wanted, or at least how many.
        numbers: usize,
        /// The cap.
        limit: usize,
    },
    /// The lists of shared objects past [`Limits::shared_entries`] or
    /// [`Limits::shared_visits`].
    #[error(
        "the lists of shared objects reached {entries} entries after {visits} visits, past their caps"
    )]
    TooManyHints {
        /// The entries they'd listed when they stopped.
        entries: usize,
        /// The objects and references they'd looked at when they stopped.
        visits: u64,
    },
}

/// The order, the numbers and the object streams of a linearized file: what
/// [`plan()`] makes of a [`Graph`]. Nothing's kept of the graph but numbers and
/// ranges, and it reads the same from any thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    kind: Kind,
    /// Every object's id, sorted, with its number and use at the same place.
    ids: Vec<Id>,
    numbers: Vec<u32>,
    uses: Vec<Use>,
    writes: Vec<Write>,
    containers: Vec<Container>,
    /// Each page's writes, by page.
    pages: Vec<Range<usize>>,
    /// Each page's own content streams' writes, by page.
    contents: Vec<Range<usize>>,
    /// Each page's thumbnail's writes, by page.
    thumbs: Vec<Range<usize>>,
    /// What thumbnails share.
    thumb_shared: Range<usize>,
    /// Each group's writes, by its place in [`Group::ALL`], if it was given.
    groups: [Option<Range<usize>>; Group::ALL.len()],
    first_page: u32,
    first_page_number: u32,
    entries: Vec<Range<usize>>,
    /// The lists of shared objects, one after another; each page's and group's
    /// range of them.
    listed: Vec<u32>,
    page_lists: Vec<Range<usize>>,
    group_lists: [Range<usize>; Group::ALL.len()],
    files: Vec<FileGroup>,
    file_lists: Vec<Range<usize>>,
    visits: u64,
    dictionary: u32,
    first_xref: Option<u32>,
    main_xref: Option<u32>,
    hint: u32,
}

impl Plan {
    /// What lists the file's objects.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The number and generation `id` is written at, or none for an id that
    /// names no object, a reference to which a writer writes as null.
    pub fn number(&self, id: Id) -> Option<Id> {
        let at = self.ids.binary_search(&id).ok()?;
        Some((self.numbers[at], 0))
    }

    /// Which pages use `id`, or none for an id that names no object. The first
    /// page is always `Use::Page(0)`, then the others count from 1 in page
    /// order, so under another first page a page's `Use` isn't its place.
    pub fn used(&self, id: Id) -> Option<Use> {
        let at = self.ids.binary_search(&id).ok()?;
        Some(self.uses[at])
    }

    /// What a caller serializes, in file order, part by part.
    pub fn writes(&self) -> &[Write] {
        &self.writes
    }

    /// The object streams a caller packs, in file order.
    pub fn containers(&self) -> &[Container] {
        &self.containers
    }

    /// `part`'s writes, as a range of [`writes`](Plan::writes).
    pub fn part_writes(&self, part: Part) -> Range<usize> {
        let start = self.writes.partition_point(|w| w.part < part);
        start..start + self.writes[start..].partition_point(|w| w.part == part)
    }

    /// The writes of `page`, by its place among the pages: for the first page,
    /// all of part 6 but the outlines'; for another, its section of part 7. The
    /// page object is the first of them.
    ///
    /// # Panics
    ///
    /// Past the pages.
    pub fn page_writes(&self, page: u32) -> Range<usize> {
        self.pages[page as usize].clone()
    }

    /// The writes from `page`'s first content stream to its last, by its place
    /// among the pages, as far as they're the page's own, in its section:
    /// none where it has no content stream there, as one in part 8 isn't its
    /// alone (F.4.2).
    ///
    /// # Panics
    ///
    /// Past the pages.
    pub fn page_contents(&self, page: u32) -> Range<usize> {
        self.contents[page as usize].clone()
    }

    /// The writes of `page`'s thumbnail, by its place among the pages, what no
    /// other thumbnail or anything before them has, and the thumbnail's object
    /// first if it's that page's alone.
    ///
    /// # Panics
    ///
    /// Past the pages.
    pub fn thumb_writes(&self, page: u32) -> Range<usize> {
        self.thumbs[page as usize].clone()
    }

    /// The writes of what several thumbnails reach and nothing before them has
    /// (F.3.10, F.4.4), after the last thumbnail's.
    pub fn thumb_shared_writes(&self) -> Range<usize> {
        self.thumb_shared.clone()
    }

    /// The writes of `group`, or none if the graph wasn't given it.
    /// [`Group::Other`]'s is followed by what no group reached.
    pub fn group_writes(&self, group: Group) -> Option<Range<usize>> {
        self.groups[group as usize].clone()
    }

    /// Each root of [`Group::EmbeddedFile`] as its own group, in the order given
    /// (F.4.7).
    pub fn files(&self) -> &[FileGroup] {
        &self.files
    }

    /// The entries of the shared object table (F.4.3), each as a range of
    /// [`writes`](Plan::writes), the first page's then part 8's. In part 6, a
    /// stretch no later page reaches is one entry and each write one reaches is
    /// another; in part 8, each write is one.
    pub fn entries(&self) -> &[Range<usize>] {
        &self.entries
    }

    /// The entries of the shared object table that `page` reaches, by its place
    /// among the pages, in order, each once, a member by its object stream
    /// (F.3.1): none for the first page (F.4.2).
    ///
    /// # Panics
    ///
    /// Past the pages.
    pub fn shared(&self, page: u32) -> &[u32] {
        &self.listed[self.page_lists[page as usize].clone()]
    }

    /// The entries of the shared object table that `group` reaches, in order,
    /// each once: for a group whose table lists them (F.4.6), otherwise none.
    pub fn group_shared(&self, group: Group) -> &[u32] {
        &self.listed[self.group_lists[group as usize].clone()]
    }

    /// The entries of the shared object table that the embedded file at `file`
    /// among [`files`](Plan::files) reaches, in order, each once (F.4.7).
    ///
    /// # Panics
    ///
    /// Past the files.
    pub fn file_shared(&self, file: usize) -> &[u32] {
        &self.listed[self.file_lists[file].clone()]
    }

    /// How many pages there are.
    pub fn page_count(&self) -> u32 {
        self.pages.len() as u32
    }

    /// Which page opens first, by its place among the pages: the dictionary's
    /// `/P`.
    pub fn first_page(&self) -> u32 {
        self.first_page
    }

    /// The catalog's number: a section's `/Root`.
    pub fn catalog_number(&self) -> u32 {
        self.writes[0].number
    }

    /// The first page object's number: the dictionary's `/O`.
    pub fn first_page_number(&self) -> u32 {
        self.first_page_number
    }

    /// The linearization dictionary's number, the first of the first group.
    pub fn dictionary_number(&self) -> u32 {
        self.dictionary
    }

    /// The first-page cross-reference stream's number, under [`Kind::Stream`].
    pub fn first_xref_number(&self) -> Option<u32> {
        self.first_xref
    }

    /// The main cross-reference stream's number, under [`Kind::Stream`].
    pub fn main_xref_number(&self) -> Option<u32> {
        self.main_xref
    }

    /// The hint stream's number, the last the file uses, wherever it's written
    /// (F.3.6).
    pub fn hint_number(&self) -> u32 {
        self.hint
    }

    /// The file's `/Size`: one past its last number.
    pub fn size(&self) -> u32 {
        self.hint + 1
    }

    /// How many entries the lists of shared objects hold, against
    /// [`Limits::shared_entries`].
    pub fn shared_entries(&self) -> usize {
        self.listed.len()
    }

    /// How many objects and references making those lists looked at, against
    /// [`Limits::shared_visits`].
    pub fn shared_visits(&self) -> u64 {
        self.visits
    }
}

/// `graph` planned: Annex F's order and numbers (F.3), and the lists of shared
/// objects each later page's entry in the hint tables wants (F.4.2).
///
/// Refused, under an [`Error`]: no pages; a page, or the catalog, not among the
/// objects; an object or a page given twice; the catalog given as a page; a
/// [`first_page`](Graph::first_page) past the pages; under [`Kind::Table`], an
/// object said to be packable; past a cap of `limits`.
pub fn plan(graph: &Graph, limits: &Limits) -> Result<Plan, Error> {
    place::plan(graph, limits)
}
