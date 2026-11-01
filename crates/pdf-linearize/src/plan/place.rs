//! How a plan is made: the objects by index, each given its place in the file
//! (F.3), the object streams of each region, the writes, and the numbers.

use std::ops::Range;

use super::group::{Use, group};
use super::shared::{self, Lists};
use super::uses;
use super::{Container, Error, FileGroup, Graph, Group, Kind, Node, Part, Plan, What, Write};
use crate::{Id, Limits};

/// A `u32` that stands for none in a list of indices.
pub(super) const NONE: u32 = u32::MAX;

/// A graph by index: each object's place among the ids in order, and its
/// references as those places. A reference to no object is dropped.
pub(super) struct Flat<'g> {
    graph: &'g Graph,
    pub(super) ids: Vec<Id>,
    /// Each object's place in `graph.nodes`.
    nodes: Vec<u32>,
    starts: Vec<usize>,
    refs: Vec<u32>,
}

impl<'g> Flat<'g> {
    /// `graph` by index, or the object given twice.
    fn of(graph: &'g Graph) -> Result<Flat<'g>, Error> {
        let mut sorted: Vec<(Id, u32)> = (0..).zip(&graph.nodes).map(|(i, n)| (n.id, i)).collect();
        sorted.sort_unstable();
        if let Some(pair) = sorted.windows(2).find(|pair| pair[0].0 == pair[1].0) {
            return Err(Error::ObjectRepeated(pair[0].0));
        }
        let (ids, nodes): (Vec<Id>, Vec<u32>) = sorted.into_iter().unzip();
        let mut flat = Flat {
            graph,
            ids,
            nodes,
            starts: Vec::with_capacity(graph.nodes.len() + 1),
            refs: Vec::with_capacity(graph.refs.len()),
        };
        flat.starts.push(0);
        for at in 0..flat.nodes.len() {
            let node = &graph.nodes[flat.nodes[at] as usize];
            for id in &graph.refs[node.refs.clone()] {
                if let Some(to) = flat.index(*id) {
                    flat.refs.push(to);
                }
            }
            flat.starts.push(flat.refs.len());
        }
        Ok(flat)
    }

    /// The object `id` names.
    fn index(&self, id: Id) -> Option<u32> {
        self.ids.binary_search(&id).ok().map(|at| at as u32)
    }

    /// What object `at` refers to.
    pub(super) fn refs(&self, at: u32) -> &[u32] {
        &self.refs[self.starts[at as usize]..self.starts[at as usize + 1]]
    }

    /// The caller's node for object `at`.
    fn node(&self, at: u32) -> &Node {
        &self.graph.nodes[self.nodes[at as usize] as usize]
    }
}

/// The objects, as they're given their places: one list in file order, and
/// which part of it is what.
struct Placing<'f, 'g> {
    flat: &'f Flat<'g>,
    /// What no walk goes into.
    stop: &'f [bool],
    placed: Vec<bool>,
    list: Vec<u32>,
    stack: Vec<u32>,
    regions: Vec<Region>,
}

/// A stretch of the objects in file order.
struct Region {
    what: Holds,
    part: Part,
    list: Range<usize>,
}

/// What a [`Region`] holds, which the plan keeps a range of writes for.
#[derive(Clone, Copy)]
enum Holds {
    Nothing,
    Page(u32),
    Thumb(u32),
    ThumbShared,
    Group(Group),
    /// A root of [`Group::EmbeddedFile`], which that group goes on through.
    File(u32),
}

impl Placing<'_, '_> {
    /// Object `at` next in the file, unless it's placed already.
    fn place(&mut self, at: u32) {
        if !std::mem::replace(&mut self.placed[at as usize], true) {
            self.list.push(at);
        }
    }

    /// What's been placed since `start`, as a region.
    fn region(&mut self, start: usize, part: Part, what: Holds) {
        let list = start..self.list.len();
        self.regions.push(Region { what, part, list });
    }

    /// A page's section: its object, then `own`, those that are `late` last.
    fn section(&mut self, page: u32, own: &[u32], wanted: impl Fn(u32) -> bool) {
        self.place(page);
        for late in [false, true] {
            for &at in own {
                if self.flat.node(at).late == late && wanted(at) {
                    self.place(at);
                }
            }
        }
    }

    /// What `roots` reach that's not placed, in the order a walk goes into it,
    /// never into what's placed, or what `stop` marks: what's placed reaches
    /// nothing a walk hasn't placed already, or a stop.
    fn walk(&mut self, roots: impl Iterator<Item = u32>) {
        let flat = self.flat;
        let roots: Vec<u32> = roots.collect();
        for &root in roots.iter().rev() {
            self.stack.push(root);
        }
        while let Some(at) = self.stack.pop() {
            if self.stop[at as usize] || self.placed[at as usize] {
                continue;
            }
            self.place(at);
            for &r in flat.refs(at).iter().rev() {
                if !self.placed[r as usize] {
                    self.stack.push(r);
                }
            }
        }
    }
}

/// What `group`'s roots name, in order.
fn roots_of<'a>(flat: &'a Flat<'_>, group: Group) -> impl Iterator<Item = u32> + 'a {
    (flat.graph.groups.iter())
        .filter(move |(g, _)| *g == group)
        .flat_map(|(_, roots)| roots.iter())
        .filter_map(|id| flat.index(*id))
}

/// The most numbers `limits` lets a file use: one short of `u32`'s last at
/// most, so that counting them can't overflow, what's written adding only four
/// numbers to the objects and object streams.
fn cap(limits: &Limits) -> usize {
    limits.objects.min(u32::MAX as usize - 1)
}

/// `graph` planned, as [`plan`](super::plan()) says.
pub(super) fn plan(graph: &Graph, limits: &Limits) -> Result<Plan, Error> {
    if graph.pages.is_empty() {
        return Err(Error::NoPages);
    }
    if graph.pages.len() > limits.pages {
        return Err(Error::TooManyPages {
            pages: graph.pages.len(),
            limit: limits.pages,
        });
    }
    if graph.nodes.len() > cap(limits) {
        return Err(Error::TooManyObjects {
            numbers: graph.nodes.len(),
            limit: limits.objects,
        });
    }
    let flat = Flat::of(graph)?;
    let objects = flat.ids.len();
    let catalog = (flat.index(graph.catalog)).ok_or(Error::NoCatalog(graph.catalog))?;
    let first = graph.first_page as usize;
    if first >= graph.pages.len() {
        return Err(Error::FirstPagePast {
            index: graph.first_page,
            pages: graph.pages.len(),
        });
    }

    let mut is_page = vec![false; objects];
    let mut pages = Vec::with_capacity(graph.pages.len());
    for page in &graph.pages {
        let at = flat.index(page.id).ok_or(Error::NoPage(page.id))?;
        if at == catalog {
            return Err(Error::CatalogIsPage(page.id));
        }
        if std::mem::replace(&mut is_page[at as usize], true) {
            return Err(Error::PageRepeated(page.id));
        }
        pages.push(at);
    }
    let packed = graph.nodes.iter().find(|node| node.packable);
    if let (Kind::Table, Some(node)) = (graph.kind, packed) {
        return Err(Error::PackedTable(node.id));
    }
    let packable: Vec<bool> = (0..objects as u32)
        .map(|at| flat.node(at).packable && at != catalog && !is_page[at as usize])
        .collect();

    // What no walk goes into, a page's own walk apart.
    let open: Vec<u32> = (graph.open.iter())
        .filter_map(|id| flat.index(*id))
        .filter(|&at| !is_page[at as usize])
        .collect();
    let tree_nodes: Vec<u32> = (graph.tree_nodes.iter())
        .filter_map(|id| flat.index(*id))
        .collect();
    let mut stop = is_page.clone();
    stop[catalog as usize] = true;
    open.iter().for_each(|&at| stop[at as usize] = true);
    tree_nodes.iter().for_each(|&at| stop[at as usize] = true);

    // The first page's walk first, then the others': it's `Use::Page(0)`, and a
    // later page's `Use` counts from 1 in page order, whichever page is first.
    let others = (pages.iter().enumerate()).filter(|&(i, _)| i != first);
    let walks: Vec<u32> = std::iter::once(pages[first])
        .chain(others.map(|(_, &at)| at))
        .collect();
    let walked = uses::walk(&walks, &stop, |at| flat.refs(at));
    let uses = &walked.uses;

    let mut placing = Placing {
        flat: &flat,
        stop: &stop,
        placed: vec![false; objects],
        list: Vec::with_capacity(objects),
        stack: Vec::new(),
        regions: Vec::new(),
    };
    let mut seal = Sealing {
        container: vec![NONE; objects],
        groups: Vec::new(),
    };

    // Part 4: the catalog, and what the caller lists.
    placing.place(catalog);
    open.iter().for_each(|&at| placing.place(at));
    placing.region(0, Part::Open, Holds::Nothing);
    seal.region(&placing, &packable, 0, Use::None);

    // Part 6: all the first page's walk went into, what later pages reach of it
    // too, then the outlines where they're to be read first.
    let start = placing.list.len();
    placing.section(pages[first], walked.entered(0), |_| true);
    placing.region(start, Part::FirstPage, Holds::Page(graph.first_page));
    seal.region(&placing, &packable, start, Use::Page(0));
    let given = |group: Group| graph.groups.iter().any(|(g, _)| *g == group);
    if graph.outlines_first && given(Group::Outlines) {
        let start = placing.list.len();
        placing.walk(roots_of(&flat, Group::Outlines));
        placing.region(start, Part::FirstPage, Holds::Group(Group::Outlines));
        seal.region(&placing, &packable, start, Use::Page(0));
    }

    // Parts 7 and 8's object streams, by their uses. One of a single page's
    // members stays in the page's section; any other is part 8's, and so is
    // what's in it.
    let mut to_shared = vec![false; objects];
    let later = (0..objects as u32).filter(|&at| {
        let at = at as usize;
        !placing.placed[at] && packable[at] && uses[at] != Use::None
    });
    for members in seal.group(later.map(|at| (at, uses[at as usize]))) {
        let one = uses[members[0] as usize];
        let own = matches!(one, Use::Page(_)) && (members.iter()).all(|&m| uses[m as usize] == one);
        if !own {
            members.iter().for_each(|&m| to_shared[m as usize] = true);
        }
    }

    // Part 7: each other page in order, skipping the first (F.3.8), by its
    // walk's order.
    let page_of = |walk: usize| (if walk <= first { walk - 1 } else { walk }) as u32;
    for (walk, &page) in walks.iter().enumerate().skip(1) {
        let start = placing.list.len();
        let own = |at: u32| {
            let at = at as usize;
            uses[at] == Use::Page(walk as u32) && !to_shared[at]
        };
        placing.section(page, walked.entered(walk), own);
        placing.region(start, Part::Pages, Holds::Page(page_of(walk)));
    }

    // Part 8: what several later pages reach, and what's in an object stream
    // with another's, by number.
    let start = placing.list.len();
    for at in 0..objects {
        if uses[at] == Use::Shared || to_shared[at] {
            placing.place(at as u32);
        }
    }
    placing.region(start, Part::Shared, Holds::Nothing);

    // Part 9: the tree, thumbnails by page, each group, and what's left.
    let start = placing.list.len();
    tree_nodes.iter().for_each(|&at| placing.place(at));
    placing.region(start, Part::Other, Holds::Nothing);
    seal.region(&placing, &packable, start, Use::None);
    // What one thumbnail alone reaches is that thumbnail's; what several do, by
    // the walk that tells a page's own from what's shared, is theirs together.
    let mut stopped = stop.clone();
    for (at, placed) in placing.placed.iter().enumerate() {
        stopped[at] |= *placed;
    }
    let thumbs: Vec<(u32, u32)> = (0..)
        .zip(&graph.pages)
        .filter_map(|(page, at)| Some((page, flat.index(at.thumb?)?)))
        .filter(|&(_, at)| !stopped[at as usize])
        .collect();
    let walks: Vec<u32> = thumbs.iter().map(|&(_, at)| at).collect();
    let thumbed = uses::walk(&walks, &stopped, |at| flat.refs(at));
    for (walk, &(page, _)) in thumbs.iter().enumerate() {
        let start = placing.list.len();
        for &at in thumbed.entered(walk) {
            if thumbed.uses[at as usize] == Use::Page(walk as u32) {
                placing.place(at);
            }
        }
        placing.region(start, Part::Other, Holds::Thumb(page));
        seal.region(&placing, &packable, start, Use::None);
    }
    let start = placing.list.len();
    for at in 0..objects {
        if thumbed.uses[at] == Use::Shared {
            placing.place(at as u32);
        }
    }
    placing.region(start, Part::Other, Holds::ThumbShared);
    seal.region(&placing, &packable, start, Use::None);
    // Each group that has a table is sealed alone; what none has goes with what
    // no group reached.
    let mut rest = None;
    let files: Vec<u32> = roots_of(&flat, Group::EmbeddedFile).collect();
    for group in Group::ALL {
        if !given(group) || (group == Group::Outlines && graph.outlines_first) {
            continue;
        }
        let start = placing.list.len();
        if group == Group::EmbeddedFile {
            // Each file is a group of its own in its table (F.4.7).
            placing.region(start, Part::Other, Holds::Group(group));
            for &root in &files {
                let start = placing.list.len();
                placing.walk(std::iter::once(root));
                placing.region(start, Part::Other, Holds::File(root));
                seal.region(&placing, &packable, start, Use::None);
            }
            continue;
        }
        placing.walk(roots_of(&flat, group));
        placing.region(start, Part::Other, Holds::Group(group));
        match group {
            Group::Other => rest = Some(start),
            _ => seal.region(&placing, &packable, start, Use::None),
        }
    }
    let start = placing.list.len();
    (0..objects as u32).for_each(|at| placing.place(at));
    placing.region(start, Part::Other, Holds::Nothing);
    seal.region(&placing, &packable, rest.unwrap_or(start), Use::None);
    debug_assert_eq!(placing.list.len(), objects);

    let mut written = Writing::of(&flat, &placing, seal, uses, graph);
    written.number(graph.kind, limits)?;
    let lists = shared::lists(shared::Walks {
        flat: &flat,
        stop: &stop,
        entry: &written.entry_of(objects),
        pages: &pages,
        first,
        groups: &Group::ALL.map(|group| match group.lists_shared() {
            true => roots_of(&flat, group).collect(),
            false => Vec::new(),
        }),
        files: &files,
        entries: written.entries.len(),
        limits,
    })?;

    Ok(written.into_plan(flat, walked.uses, pages[first], lists))
}

/// The object streams as they're decided: each sealed by the [`group`] call
/// that made it.
struct Sealing {
    /// Each object's stream, as a place in `groups`.
    container: Vec<u32>,
    groups: Vec<Vec<u32>>,
}

impl Sealing {
    /// `members`' object streams, kept, with each's members.
    fn group(&mut self, members: impl Iterator<Item = (u32, Use)>) -> &[Vec<u32>] {
        let start = self.groups.len();
        for stream in group(members) {
            let at = self.groups.len() as u32;
            stream.iter().for_each(|&m| self.container[m as usize] = at);
            self.groups.push(stream);
        }
        &self.groups[start..]
    }

    /// What's packable of what's been placed since `start`, all as `used`.
    fn region(&mut self, placing: &Placing, packable: &[bool], start: usize, used: Use) {
        let list = &placing.list[start..];
        let members = list.iter().filter(|&&at| packable[at as usize]);
        self.group(members.map(|&at| (at, used)));
    }
}

/// The writes in file order, as the numbers are given.
struct Writing {
    writes: Vec<Write>,
    /// What each write is, by object or by place in `sealed`.
    sources: Vec<Source>,
    /// The streams in file order, by place in `sealed`.
    order: Vec<u32>,
    sealed: Vec<Vec<u32>>,
    /// Each stream's place in `order`, or none.
    place: Vec<u32>,
    /// Each stream's number, by its place in `order`.
    numbered: Vec<u32>,
    pages: Vec<Range<usize>>,
    thumbs: Vec<Range<usize>>,
    groups: [Option<Range<usize>>; Group::ALL.len()],
    thumb_shared: Range<usize>,
    /// Each embedded file's root and writes.
    files: Vec<(u32, Range<usize>)>,
    entries: Vec<Range<usize>>,
    /// Each write's entry, or none.
    entry: Vec<u32>,
    /// Each object's write.
    write_of: Vec<u32>,
    numbers: Vec<u32>,
    dictionary: u32,
    first_xref: Option<u32>,
    main_xref: Option<u32>,
    hint: u32,
}

/// What a write is.
#[derive(Clone, Copy)]
enum Source {
    Object(u32),
    Stream(u32),
}

impl Writing {
    /// The writes `placing`'s regions make, an object stream where its first
    /// member would be, and the shared object table's entries (F.4.3).
    fn of(flat: &Flat, placing: &Placing, seal: Sealing, uses: &[Use], graph: &Graph) -> Writing {
        let objects = flat.ids.len();
        let mut writing = Writing {
            writes: Vec::with_capacity(objects),
            sources: Vec::with_capacity(objects),
            order: Vec::with_capacity(seal.groups.len()),
            numbered: vec![0; seal.groups.len()],
            place: vec![NONE; seal.groups.len()],
            pages: vec![0..0; graph.pages.len()],
            thumbs: vec![0..0; graph.pages.len()],
            groups: [const { None }; Group::ALL.len()],
            thumb_shared: 0..0,
            files: Vec::new(),
            entries: Vec::new(),
            entry: Vec::new(),
            write_of: vec![NONE; objects],
            numbers: vec![0; objects],
            dictionary: 0,
            first_xref: None,
            main_xref: None,
            hint: 0,
            sealed: Vec::new(),
        };
        for region in &placing.regions {
            let start = writing.writes.len();
            for &at in &placing.list[region.list.clone()] {
                let stream = seal.container[at as usize];
                let at_write = writing.writes.len() as u32;
                if stream == NONE {
                    writing.write_of[at as usize] = at_write;
                    writing.push(
                        What::Object(flat.ids[at as usize]),
                        Source::Object(at),
                        region.part,
                    );
                } else if writing.place[stream as usize] == NONE {
                    let place = writing.order.len();
                    writing.place[stream as usize] = place as u32;
                    writing.order.push(stream);
                    for &m in &seal.groups[stream as usize] {
                        writing.write_of[m as usize] = at_write;
                    }
                    writing.push(What::Container(place), Source::Stream(stream), region.part);
                }
            }
            let range = start..writing.writes.len();
            match region.what {
                Holds::Nothing => {}
                Holds::Page(page) => writing.pages[page as usize] = range,
                Holds::Thumb(page) => writing.thumbs[page as usize] = range,
                Holds::ThumbShared => writing.thumb_shared = range,
                Holds::Group(group) => writing.groups[group as usize] = Some(range),
                Holds::File(root) => {
                    if let Some(all) = &mut writing.groups[Group::EmbeddedFile as usize] {
                        all.end = range.end;
                    }
                    writing.files.push((root, range));
                }
            }
        }
        writing.sealed = seal.groups;
        // Other's goes on through what no group reached.
        if let Some(other) = &mut writing.groups[Group::Other as usize] {
            *other = other.start..writing.writes.len();
        }
        writing.entries_of(uses);
        writing
    }

    fn push(&mut self, what: What, source: Source, part: Part) {
        self.writes.push(Write {
            what,
            number: 0,
            part,
        });
        self.sources.push(source);
    }

    /// The shared object table's entries: in part 6, each stretch that no later
    /// page reaches is one, and each write one reaches another; in part 8, each
    /// write is one.
    fn entries_of(&mut self, uses: &[Use]) {
        let later = |at: u32| matches!(uses[at as usize], Use::Shared | Use::Page(1..));
        self.entry = vec![NONE; self.writes.len()];
        let mut run = None;
        for w in 0..self.writes.len() {
            match self.writes[w].part {
                Part::FirstPage => {
                    let shared = match self.sources[w] {
                        Source::Object(at) => later(at),
                        Source::Stream(s) => self.sealed[s as usize].iter().any(|&m| later(m)),
                    };
                    self.entry[w] = match (shared, run) {
                        (false, Some(entry)) => {
                            self.entries[entry as usize].end = w + 1;
                            entry
                        }
                        _ => {
                            let entry = self.entries.len() as u32;
                            self.entries.push(w..w + 1);
                            run = (!shared).then_some(entry);
                            entry
                        }
                    };
                }
                Part::Shared => {
                    self.entry[w] = self.entries.len() as u32;
                    self.entries.push(w..w + 1);
                }
                _ => {}
            }
        }
    }

    /// Each object's entry in the shared object table, or none.
    fn entry_of(&self, objects: usize) -> Vec<u32> {
        (0..objects)
            .map(|at| match self.write_of[at] {
                NONE => NONE,
                w => self.entry[w as usize],
            })
            .collect()
    }

    /// The numbers (F.3.1, F.3.6). From 1, the second group: parts 7, 8 and 9's
    /// writes in file order, the main cross-reference stream, then their object
    /// streams' members in order. After it, the first group: the dictionary,
    /// the first-page cross-reference stream, parts 4 and 6's writes in file
    /// order, their members, and the hint stream last.
    fn number(&mut self, kind: Kind, limits: &Limits) -> Result<(), Error> {
        let members: usize = self.sealed.iter().map(Vec::len).sum();
        let own = match kind {
            Kind::Table => 2,
            Kind::Stream => 4,
        };
        let numbers = self.writes.len() + members + own;
        if numbers > cap(limits) {
            return Err(Error::TooManyObjects {
                numbers,
                limit: limits.objects,
            });
        }
        let streams = kind == Kind::Stream;
        let second = |part| part >= Part::Pages;
        let first = |part| part < Part::Pages;
        let mut next = 1;
        self.number_writes(&mut next, second);
        self.main_xref = streams.then(|| take(&mut next));
        self.number_members(&mut next, second);
        self.dictionary = take(&mut next);
        self.first_xref = streams.then(|| take(&mut next));
        self.number_writes(&mut next, first);
        self.number_members(&mut next, first);
        self.hint = take(&mut next);
        Ok(())
    }

    /// The writes whose part `of` says, numbered in file order.
    fn number_writes(&mut self, next: &mut u32, of: impl Fn(Part) -> bool) {
        for w in 0..self.writes.len() {
            if of(self.writes[w].part) {
                let number = take(next);
                self.writes[w].number = number;
                match self.sources[w] {
                    Source::Object(at) => self.numbers[at as usize] = number,
                    Source::Stream(s) => self.numbered[self.place[s as usize] as usize] = number,
                }
            }
        }
    }

    /// The members of the streams whose part `of` says, numbered in order.
    fn number_members(&mut self, next: &mut u32, of: impl Fn(Part) -> bool) {
        for w in 0..self.writes.len() {
            if let (true, Source::Stream(s)) = (of(self.writes[w].part), self.sources[w]) {
                for &m in &self.sealed[s as usize] {
                    self.numbers[m as usize] = take(next);
                }
            }
        }
    }

    /// Each page's writes from its first content stream in its section to its
    /// last, in file order.
    fn contents(&self, flat: &Flat) -> Vec<Range<usize>> {
        (flat.graph.pages.iter().zip(&self.pages))
            .map(|(page, section)| {
                let own = (page.contents.iter())
                    .filter_map(|id| flat.index(*id))
                    .map(|at| self.write_of[at as usize] as usize)
                    .filter(|write| section.contains(write));
                let least = own.clone().min();
                match (least, own.max()) {
                    (Some(least), Some(most)) => least..most + 1,
                    _ => 0..0,
                }
            })
            .collect()
    }

    /// The finished plan.
    fn into_plan(self, flat: Flat, uses: Vec<Use>, first: u32, lists: Lists) -> Plan {
        let contents = self.contents(&flat);
        let files = (self.files.into_iter())
            .map(|(root, writes)| FileGroup {
                number: self.numbers[root as usize],
                writes,
            })
            .collect();
        let containers = (self.order.iter().zip(&self.numbered))
            .map(|(&stream, &number)| Container {
                number,
                members: (self.sealed[stream as usize].iter())
                    .map(|&m| (flat.ids[m as usize], self.numbers[m as usize]))
                    .collect(),
            })
            .collect();
        Plan {
            kind: flat.graph.kind,
            first_page: flat.graph.first_page,
            first_page_number: self.numbers[first as usize],
            ids: flat.ids,
            numbers: self.numbers,
            uses,
            writes: self.writes,
            containers,
            pages: self.pages,
            contents,
            thumbs: self.thumbs,
            thumb_shared: self.thumb_shared,
            groups: self.groups,
            entries: self.entries,
            listed: lists.listed,
            page_lists: lists.pages,
            group_lists: lists.groups,
            files,
            file_lists: lists.files,
            visits: lists.visits,
            dictionary: self.dictionary,
            first_xref: self.first_xref,
            main_xref: self.main_xref,
            hint: self.hint,
        }
    }
}

/// The next number, taken.
fn take(next: &mut u32) -> u32 {
    *next += 1;
    *next - 1
}
