//! What the fuzz target checks, which unit tests check too: the promises a plan
//! makes, on any graph, each held against a plain walk of it; what's assembled
//! from a plan, its positions against its pieces, its tables against the plan,
//! and the check of it clean; a hint stream of any bytes, read under small caps
//! and written again; any facts, judged the same whatever the sink; with
//! `family`, any bytes as a file, laid out and read back, the check of what's
//! written clean; and with `lopdf` too, any bytes as edits to a document of
//! lopdf's, the same of what that adapter writes. It's compiled only for tests
//! and under cargo-fuzz's `--cfg fuzzing`.

use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;

use crate::check::{
    self, Facts, Finding, FirstObject, Grade, HintData, HintFacts, PageFacts, Raw, RowEntry,
    RowFacts, SectionFacts, Sink,
};
use crate::hint::{self, EmbeddedFiles, Extended, Generic, HintKeys, PageOffsets};
use crate::hint::{SharedObjects, Thumbnails};
use crate::layout::{self, Encoded, Layout, Options, Piece, Stream, StreamKind, assemble};
use crate::plan::{Error, Group, Kind, Page, Part, Plan, Use, What, plan};
use pdf_objects::ObjectId;

use crate::{Graph, Limits, MEMBERS};

/// The most objects an input describes, past what two object streams hold.
const OBJECTS: usize = 600;

/// The most pages it describes.
const PAGES: usize = 32;

/// The most objects of a graph whose caps are each tried at their edge.
const SMALL: usize = 128;

/// An object as an input describes it.
#[derive(Debug, Clone, Default)]
struct Object {
    refs: Vec<ObjectId>,
    packable: bool,
    late: bool,
}

/// A graph as an input describes it, what's held against the plan of it.
#[derive(Debug, Clone)]
struct Described {
    kind: Kind,
    catalog: ObjectId,
    /// The objects given, in order, each given more than once as it's listed.
    given: Vec<(ObjectId, Object)>,
    pages: Vec<(ObjectId, Option<ObjectId>)>,
    /// Each page's content streams.
    contents: Vec<Vec<ObjectId>>,
    first: u32,
    tree_nodes: Vec<ObjectId>,
    open: Vec<ObjectId>,
    groups: Vec<(Group, Vec<ObjectId>)>,
    outlines_first: bool,
}

/// An input's bytes, then zeros for ever, so any input describes a graph, and a
/// short one a plain one.
struct Bytes<'a>(&'a [u8]);

impl Bytes<'_> {
    fn byte(&mut self) -> u8 {
        let Some((&first, rest)) = self.0.split_first() else {
            return 0;
        };
        self.0 = rest;
        first
    }

    /// A number below `n`, from as many bytes as it takes.
    fn below(&mut self, n: usize) -> usize {
        match n {
            0..=256 => self.byte() as usize % n.max(1),
            _ => (self.byte() as usize | (self.byte() as usize) << 8) % n,
        }
    }

    /// Whether a one-in-`n` chance came up.
    fn chance(&mut self, n: usize) -> bool {
        self.below(n) == n - 1
    }
}

/// The graph `data` describes: mostly one that can be planned, each thing that
/// can't by a small chance, its numbers spread by a step so they aren't its
/// order.
fn described(data: &[u8]) -> Described {
    let mut bytes = Bytes(data);
    let flags = bytes.byte();
    let count = 2 + bytes.below(OBJECTS - 1);
    let step = 1 + bytes.below(5) as u32;
    let id_of = |at: usize| ObjectId::new(1 + at as u32 * step, 0);
    let everything_packable = flags & 4 != 0;
    let kind = match flags & 1 != 0 {
        true => Kind::Stream,
        false => Kind::Table,
    };
    let mut given: Vec<(ObjectId, Object)> = Vec::with_capacity(count);
    for at in 0..count {
        let (mut id, mut object) = (id_of(at), Object::default());
        if bytes.chance(64) {
            // The number before it at another generation, or given again.
            id = id_of(at.saturating_sub(1));
            id.generation = bytes.below(2) as u16;
        }
        for _ in 0..bytes.below(8) {
            // A little past the objects, for a reference to none.
            let to = bytes.below(count + 4);
            object.refs.push(id_of(to));
        }
        let say = bytes.byte();
        object.late = say & 1 != 0;
        object.packable = match kind {
            Kind::Stream => everything_packable || say & 2 != 0,
            // As only a stream can list one, a table has one by a small chance.
            Kind::Table => say >= 250,
        };
        given.push((id, object));
    }
    let catalog = id_of(if bytes.chance(64) { 2 } else { 0 });
    let pick = |bytes: &mut Bytes| id_of(bytes.below(count + 1));
    let page_count = match bytes.chance(64) {
        true => 0,
        false => 1 + bytes.below(PAGES.min(count - 1)),
    };
    let start = 1 + bytes.below(count - page_count.min(count - 1));
    let mut pages = Vec::with_capacity(page_count);
    let mut contents = Vec::with_capacity(page_count);
    for at in 0..page_count {
        let page = match bytes.chance(128) {
            true => pick(&mut bytes),
            false => id_of(start + at),
        };
        let thumb = bytes.chance(4).then(|| pick(&mut bytes));
        pages.push((page, thumb));
        let streams = bytes.below(3);
        contents.push((0..streams).map(|_| pick(&mut bytes)).collect());
    }
    let first = match bytes.chance(4) {
        true => bytes.below(page_count + usize::from(bytes.0.len() % 16 == 1)) as u32,
        false => 0,
    };
    let list = |bytes: &mut Bytes, most| {
        let count = bytes.below(most);
        (0..count).map(|_| pick(bytes)).collect::<Vec<ObjectId>>()
    };
    let tree_nodes = list(&mut bytes, 8);
    let open = list(&mut bytes, 4);
    let mut groups = Vec::new();
    for _ in 0..bytes.below(5) {
        let group = Group::ALL[bytes.below(Group::ALL.len())];
        groups.push((group, list(&mut bytes, 4)));
    }
    Described {
        kind,
        catalog,
        given,
        pages,
        contents,
        first,
        tree_nodes,
        open,
        groups,
        outlines_first: flags & 2 != 0,
    }
}

impl Described {
    fn graph(&self) -> Graph {
        let mut graph = Graph::new(self.catalog, self.kind);
        for (id, object) in &self.given {
            let refs = object.refs.iter().copied();
            (graph.object(*id, refs))
                .packable(object.packable)
                .late(object.late);
        }
        for (&(id, thumb), contents) in self.pages.iter().zip(&self.contents) {
            let mut page = Page::new(id);
            page.thumb = thumb;
            page.contents.clone_from(contents);
            graph.page(page);
        }
        graph.first_page(self.first);
        self.tree_nodes.iter().for_each(|&id| graph.tree_node(id));
        self.open.iter().for_each(|&id| graph.open(id));
        for (group, roots) in &self.groups {
            graph.group(*group, roots.iter().copied());
        }
        graph.outlines_first(self.outlines_first);
        graph
    }

    /// Why the graph has to be refused, by the first check [`plan`] makes that
    /// it fails. A cap can't come up on an input's graph under the default
    /// ones.
    fn refusal(&self) -> Option<Error> {
        if self.pages.is_empty() {
            return Some(Error::NoPages);
        }
        let mut seen = BTreeSet::new();
        for (id, _) in &self.given {
            if !seen.insert(*id) {
                let mut ids: Vec<ObjectId> = self.given.iter().map(|(id, _)| *id).collect();
                ids.sort_unstable();
                let twice = ids.windows(2).find(|pair| pair[0] == pair[1]);
                return Some(Error::ObjectRepeated(twice.unwrap()[0]));
            }
        }
        if !seen.contains(&self.catalog) {
            return Some(Error::NoCatalog(self.catalog));
        }
        if self.first as usize >= self.pages.len() {
            return Some(Error::FirstPagePast {
                index: self.first,
                pages: self.pages.len(),
            });
        }
        let mut pages = BTreeSet::new();
        for &(id, _) in &self.pages {
            if !seen.contains(&id) {
                return Some(Error::NoPage(id));
            }
            if id == self.catalog {
                return Some(Error::CatalogIsPage(id));
            }
            if !pages.insert(id) {
                return Some(Error::PageRepeated(id));
            }
        }
        if self.kind == Kind::Table {
            let packed = self.given.iter().find(|(_, object)| object.packable);
            return packed.map(|(id, _)| Error::PackedTable(*id));
        }
        None
    }
}

/// The objects of a graph with no object given twice, and the plain walks over
/// them.
struct Plain<'d> {
    objects: BTreeMap<ObjectId, &'d Object>,
    /// What no walk goes into.
    stops: BTreeSet<ObjectId>,
    /// What a walk stops at but the page walked from.
    pages: BTreeSet<ObjectId>,
    open: Vec<ObjectId>,
}

impl<'d> Plain<'d> {
    fn of(described: &'d Described) -> Plain<'d> {
        let objects: BTreeMap<ObjectId, &Object> = (described.given.iter())
            .map(|(id, object)| (*id, object))
            .collect();
        let pages: BTreeSet<ObjectId> = described.pages.iter().map(|&(id, _)| id).collect();
        let open = (described.open.iter()).filter(|id| !pages.contains(id));
        let open: Vec<ObjectId> = open.copied().collect();
        let tree = (described.tree_nodes.iter()).filter(|id| !pages.contains(id));
        let stops = (pages.iter().chain(&open).chain(tree).copied())
            .chain([described.catalog])
            .collect();
        Plain {
            objects,
            stops,
            pages,
            open,
        }
    }

    /// All `roots` reach, under the stops, `page` being a stop that's entered.
    fn reach(&self, roots: &[ObjectId], page: Option<ObjectId>) -> BTreeSet<ObjectId> {
        let mut seen = BTreeSet::new();
        let mut to_do = roots.to_vec();
        while let Some(id) = to_do.pop() {
            if !self.objects.contains_key(&id) || (self.stops.contains(&id) && Some(id) != page) {
                continue;
            }
            if seen.insert(id) {
                to_do.extend(&self.objects[&id].refs);
            }
        }
        seen
    }
}

/// Where each object is, by its write, and the writes' parts, for a plan.
fn writes_by_object(plan: &Plan) -> BTreeMap<ObjectId, usize> {
    let mut by_object = BTreeMap::new();
    for (at, write) in plan.writes().iter().enumerate() {
        match write.what {
            What::Object(id) => {
                assert!(
                    by_object.insert(id, at).is_none(),
                    "{id:?} is written twice"
                );
            }
            What::Container(container) => {
                for (member, _) in &plan.containers()[container].members {
                    assert!(by_object.insert(*member, at).is_none(), "{member:?} twice");
                }
            }
        }
    }
    by_object
}

/// `plan` of `described`, against plain walks of it: each promise of [`plan`].
fn holds(described: &Described, graph: &Graph, plan: &Plan) {
    let plain = Plain::of(described);
    let by_object = writes_by_object(plan);
    let first = described.first as usize;
    let page_ids: Vec<ObjectId> = described.pages.iter().map(|&(id, _)| id).collect();
    let first_id = page_ids[first];

    // Each object once, and nothing else: an id named nowhere has no number.
    assert_eq!(by_object.len(), plain.objects.len());
    assert!(plain.objects.keys().all(|id| by_object.contains_key(id)));
    assert_eq!(plan.number(ObjectId::new(u32::MAX, 0)), None);

    // A bijection onto 1..size, in F.3.1's order: the second group's writes in
    // file order, its xref, its compressed members; the dictionary, the first
    // xref, the first group's writes, its members, and the hint.
    let mut want = Vec::new();
    let members_of = |want: &mut Vec<u32>, of: &dyn Fn(Part) -> bool| {
        for write in plan.writes().iter().filter(|w| of(w.part)) {
            if let What::Container(at) = write.what {
                want.extend(plan.containers()[at].members.iter().map(|m| m.1));
            }
        }
    };
    let second = |part: Part| part >= Part::Pages;
    let first_group = |part: Part| part < Part::Pages;
    let numbers = |of: &dyn Fn(Part) -> bool| {
        let writes = plan.writes().iter().filter(|w| of(w.part));
        writes.map(|w| w.number).collect::<Vec<u32>>()
    };
    want.extend(numbers(&second));
    want.extend(plan.main_xref_number());
    members_of(&mut want, &second);
    want.push(plan.dictionary_number());
    want.extend(plan.first_xref_number());
    want.extend(numbers(&first_group));
    members_of(&mut want, &first_group);
    want.push(plan.hint_number());
    assert_eq!(want, (1..plan.size()).collect::<Vec<u32>>());
    assert_eq!(
        plan.first_xref_number().is_some(),
        described.kind == Kind::Stream
    );
    assert_eq!(
        plan.main_xref_number().is_some(),
        described.kind == Kind::Stream
    );
    for (id, &at) in &by_object {
        let number = plan.number(*id).unwrap();
        assert_eq!(number.generation, 0);
        match plan.writes()[at].what {
            What::Object(_) => assert_eq!(number.number, plan.writes()[at].number),
            What::Container(c) => {
                let found = plan.containers()[c].members.iter().find(|m| m.0 == *id);
                assert_eq!(number.number, found.unwrap().1);
                assert_eq!(plan.containers()[c].number, plan.writes()[at].number);
            }
        }
    }

    // Parts in order; part 4 has the catalog first.
    let parts = plan.writes().iter().map(|w| w.part);
    assert!(parts.clone().zip(parts.skip(1)).all(|(a, b)| a <= b));
    assert_eq!(
        plan.writes()[0].what,
        What::Object(described.catalog),
        "the catalog's first"
    );

    // Every page first in its section: the first's the start of part 6, the
    // others' tiling part 7 in page order, skipping the first.
    assert_eq!(plan.page_count() as usize, page_ids.len());
    assert_eq!(
        plan.first_page_number(),
        plan.number(first_id).unwrap().number
    );
    let mut next = plan.part_writes(Part::Pages).start;
    for (page, &id) in page_ids.iter().enumerate() {
        let range = plan.page_writes(page as u32);
        assert_eq!(plan.writes()[range.start].what, What::Object(id));
        if page == first {
            assert_eq!(range.start, plan.part_writes(Part::FirstPage).start);
            assert_eq!(plan.writes()[range.start].part, Part::FirstPage);
        } else {
            assert_eq!(range.start, next);
            next = range.end;
        }
    }
    assert_eq!(next, plan.part_writes(Part::Pages).end);

    // What's late is after the rest of its page's, its page object first.
    for page in 0..plan.page_count() {
        let late = |write: &crate::Write| match write.what {
            What::Object(id) => plain.objects[&id].late,
            What::Container(_) => false,
        };
        let section = &plan.writes()[plan.page_writes(page)][1..];
        let stream = section
            .iter()
            .position(|w| matches!(w.what, What::Container(_)));
        // A stream goes where its first member would, and members can be late.
        if stream.is_none() {
            let first_late = section.iter().position(late).unwrap_or(section.len());
            assert!(section[first_late..].iter().all(late), "page {page}");
        }
    }

    // Each object's use is how many pages reach it, the walk that gave it being
    // no more than a plain walk of each.
    let reaches: Vec<BTreeSet<ObjectId>> = (page_ids.iter())
        .map(|&page| plain.reach(&[page], Some(page)))
        .collect();
    let walk_of = |page: usize| match page.cmp(&first) {
        std::cmp::Ordering::Less => page as u32 + 1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => page as u32,
    };
    let outlines: Vec<ObjectId> = (described.groups.iter())
        .filter(|(group, _)| *group == Group::Outlines)
        .flat_map(|(_, roots)| roots.iter().copied())
        .collect();
    let in_outlines = match described.outlines_first {
        true => plain.reach(&outlines, None),
        false => BTreeSet::new(),
    };
    for &id in plain.objects.keys() {
        let reached: Vec<usize> = (0..page_ids.len())
            .filter(|&page| reaches[page].contains(&id))
            .collect();
        let want = match reached.as_slice() {
            [] => Use::None,
            [page] => Use::Page(walk_of(*page)),
            _ => Use::Shared,
        };
        assert_eq!(plan.used(id), Some(want), "{id:?}");

        // Where it is follows from that. What the first page reaches is in part
        // 6, shared or not; so are outlines read first; what's at open is part
        // 4; a later page's own is its own, unless it's in a stream of several.
        let at = by_object[&id];
        let part = plan.writes()[at].part;
        if id == described.catalog || plain.open.contains(&id) {
            assert_eq!(part, Part::Open, "{id:?}");
        } else if reached.contains(&first) || in_outlines.contains(&id) {
            assert_eq!(part, Part::FirstPage, "{id:?}");
        } else {
            match reached.as_slice() {
                [] => assert_eq!(part, Part::Other, "{id:?}"),
                [page] => {
                    let packed = matches!(plan.writes()[at].what, What::Container(_));
                    assert!(
                        plan.page_writes(*page as u32).contains(&at)
                            || (packed && part == Part::Shared),
                        "{id:?}"
                    );
                }
                _ => assert_eq!(part, Part::Shared, "{id:?}"),
            }
        }
    }

    // Object streams: only what may be, at most their cap, all of one region,
    // and only where a stream lists them.
    if described.kind == Kind::Table {
        assert!(plan.containers().is_empty());
    }
    let first_reach = &reaches[first];
    for (c, container) in plan.containers().iter().enumerate() {
        assert!(!container.members.is_empty() && container.members.len() <= MEMBERS);
        let at = (plan.writes().iter())
            .position(|w| w.what == What::Container(c))
            .unwrap();
        let in_first_page = plan.page_writes(described.first).contains(&at);
        let mut last = None;
        for (id, number) in &container.members {
            assert!(plain.objects[id].packable, "{id:?}");
            assert!(*id != described.catalog && !plain.pages.contains(id));
            // By its use where uses tell a stream's members apart.
            let used = (plan.writes()[at].part >= Part::Pages).then(|| plan.used(*id).map(rank));
            assert!(last.is_none_or(|last| last < (used, *id)), "{id:?}");
            last = Some((used, *id));
            assert!(*number > 0);
            assert_eq!(in_first_page, first_reach.contains(id), "{id:?}");
        }
    }
    for (group, _) in described.groups.iter().filter(|(g, _)| *g != Group::Other) {
        let range = plan.group_writes(*group).unwrap();
        let reached: BTreeSet<ObjectId> = (described.groups.iter())
            .filter(|(g, _)| g == group)
            .flat_map(|(_, roots)| plain.reach(roots, None))
            .collect();
        for write in &plan.writes()[range] {
            match write.what {
                What::Object(id) => assert!(reached.contains(&id), "{id:?}"),
                What::Container(c) => {
                    for (id, _) in &plan.containers()[c].members {
                        assert!(reached.contains(id), "{id:?} in {group:?}'s stream");
                    }
                }
            }
        }
    }

    // Entries tile part 6, then part 8 write by write; in part 6 what a later
    // page reaches has one of its own and nothing else is alone beside another
    // that could join it.
    let later_reach = |at: usize| match plan.writes()[at].what {
        What::Object(id) => plan.used(id).is_some_and(later),
        What::Container(c) => {
            (plan.containers()[c].members.iter()).any(|(id, _)| plan.used(*id).is_some_and(later))
        }
    };
    let tile = plan.part_writes(Part::FirstPage);
    let mut next = tile.start;
    let mut entries = plan.entries().iter();
    let mut last_alone = false;
    while next < tile.end {
        let entry = entries.next().unwrap();
        assert_eq!(entry.start, next);
        next = entry.end;
        let shared = entry.clone().any(later_reach);
        if shared {
            assert_eq!(entry.len(), 1);
        } else {
            assert!(
                !last_alone,
                "two stretches of what's not shared side by side"
            );
        }
        last_alone = !shared;
    }
    for at in plan.part_writes(Part::Shared) {
        assert_eq!(entries.next(), Some(&(at..at + 1)));
    }
    assert_eq!(entries.next(), None);
    if !plan.entries().is_empty() {
        assert_eq!(
            plan.entries()[0].start,
            plan.page_writes(described.first).start
        );
    }

    // Each list: all it reaches that has an entry, once, in order, a member by
    // its stream. The first page's is none.
    let entry_of_write = |at: usize| plan.entries().iter().position(|e| e.contains(&at));
    let list_of = |reach: &BTreeSet<ObjectId>| {
        let entries = (reach.iter()).filter_map(|id| entry_of_write(by_object[id]));
        let entries: BTreeSet<u32> = entries.map(|entry| entry as u32).collect();
        entries.into_iter().collect::<Vec<u32>>()
    };
    let mut count = 0;
    for (page, reach) in reaches.iter().enumerate() {
        if page == first {
            assert_eq!(plan.shared(page as u32), []);
        } else {
            assert_eq!(plan.shared(page as u32), list_of(reach), "page {page}");
            count += plan.shared(page as u32).len();
        }
    }
    for group in Group::ALL {
        let roots: Vec<ObjectId> = (described.groups.iter())
            .filter(|(g, _)| *g == group)
            .flat_map(|(_, roots)| roots.iter().copied())
            .collect();
        let lists = matches!(group, Group::Form | Group::Structure | Group::Renditions);
        if lists {
            let list = list_of(&plain.reach(&roots, None));
            assert_eq!(plan.group_shared(group), list, "{group:?}");
            count += list.len();
        } else {
            assert_eq!(plan.group_shared(group), []);
        }
    }
    // Each root of the embedded file group: a group of its own, the writes of
    // each what it reaches, one after another, and a list of its own.
    let files: Vec<ObjectId> = (described.groups.iter())
        .filter(|(g, _)| *g == Group::EmbeddedFile)
        .flat_map(|(_, roots)| roots.iter().copied())
        .filter(|id| plain.objects.contains_key(id))
        .collect();
    assert_eq!(plan.files().len(), files.len());
    let mut next = plan
        .group_writes(Group::EmbeddedFile)
        .map(|whole| whole.start);
    for (at, root) in files.iter().enumerate() {
        let reach = plain.reach(&[*root], None);
        let file = &plan.files()[at];
        assert_eq!(file.number, plan.number(*root).unwrap().number);
        assert_eq!(next, Some(file.writes.start), "{root:?}");
        next = Some(file.writes.end);
        for write in &plan.writes()[file.writes.clone()] {
            match write.what {
                What::Object(id) => assert!(reach.contains(&id), "{id:?}"),
                What::Container(c) => {
                    for (id, _) in &plan.containers()[c].members {
                        assert!(reach.contains(id), "{id:?} in {root:?}'s stream");
                    }
                }
            }
        }
        assert_eq!(plan.file_shared(at), list_of(&reach), "{root:?}");
        count += plan.file_shared(at).len();
    }
    assert_eq!(
        next,
        plan.group_writes(Group::EmbeddedFile)
            .map(|whole| whole.end)
    );
    assert_eq!(plan.shared_entries(), count);

    // Thumbnails: each's writes what it reaches, what several share theirs.
    let thumb_reach: Vec<BTreeSet<ObjectId>> = (described.pages.iter())
        .map(|&(_, thumb)| plain.reach(&Vec::from_iter(thumb), None))
        .collect();
    let in_write = |at: usize| match plan.writes()[at].what {
        What::Object(id) => vec![id],
        What::Container(c) => plan.containers()[c].members.iter().map(|m| m.0).collect(),
    };
    for (page, reach) in thumb_reach.iter().enumerate() {
        for at in plan.thumb_writes(page as u32) {
            for id in in_write(at) {
                assert!(reach.contains(&id), "{id:?} on page {page}");
            }
        }
    }
    for at in plan.thumb_shared_writes() {
        for id in in_write(at) {
            let reached = thumb_reach.iter().filter(|reach| reach.contains(&id));
            assert!(reached.count() >= 2, "{id:?} shared");
        }
    }

    // A page's contents: from the first of them in its section to the last.
    for (page, contents) in described.contents.iter().enumerate() {
        let section = plan.page_writes(page as u32);
        let own: Vec<usize> = (contents.iter())
            .filter_map(|id| by_object.get(id).copied())
            .filter(|at| section.contains(at))
            .collect();
        let want = match (own.iter().min(), own.iter().max()) {
            (Some(least), Some(most)) => *least..*most + 1,
            _ => 0..0,
        };
        assert_eq!(plan.page_contents(page as u32), want, "page {page}");
    }

    // What's given says the group exists, whatever's left of it.
    for group in Group::ALL {
        let given = described.groups.iter().any(|(g, _)| *g == group);
        assert_eq!(plan.group_writes(group).is_some(), given);
    }

    // Each cap holds at its edge, and is no more than a cap: a plan under it is
    // the same plan. Each is another plan, so only of a small graph.
    if plain.objects.len() > SMALL {
        return;
    }
    let at_edge = |edit: &dyn Fn(&mut Limits)| {
        let mut limits = Limits::DEFAULT;
        edit(&mut limits);
        crate::plan(graph, &limits)
    };
    let same = Ok(plan);
    assert_eq!(
        at_edge(&|l| l.objects = plan.size() as usize - 1).as_ref(),
        same
    );
    assert_eq!(at_edge(&|l| l.pages = page_ids.len()).as_ref(), same);
    assert_eq!(
        at_edge(&|l| l.shared_visits = plan.shared_visits()).as_ref(),
        same
    );
    assert_eq!(at_edge(&|l| l.shared_entries = count).as_ref(), same);
    let refused = at_edge(&|l| l.objects = plan.size() as usize - 2);
    assert!(matches!(refused, Err(Error::TooManyObjects { .. })));
    let refused = at_edge(&|l| l.pages = page_ids.len() - 1);
    assert!(matches!(refused, Err(Error::TooManyPages { .. })));
    if plan.shared_visits() > 0 {
        let refused = at_edge(&|l| l.shared_visits = plan.shared_visits() - 1);
        assert!(matches!(refused, Err(Error::TooManyHints { .. })));
    }
    if count > 0 {
        let refused = at_edge(&|l| l.shared_entries = count - 1);
        assert!(matches!(refused, Err(Error::TooManyHints { .. })));
    }
}

/// A stream as it is.
fn plain(stream: Stream<'_>) -> Result<Encoded, Infallible> {
    Ok(Encoded::plain(stream.data.to_vec()))
}

/// `layout` of `plan` under `lengths`: its pieces add up to where it says each
/// thing is, the dictionary's values among them, and its tables, read back, are
/// the plan's, each position landing on what it names.
fn assembled(plan: &Plan, lengths: &[u64], layout: &Layout) {
    let mut at = 0;
    let mut bodies = 0;
    let (hint_at, hint_len) = layout.hint_extent();
    let mut hint_seen = false;
    let mut positions = Vec::with_capacity(lengths.len() + 1);
    let mut pieces = layout.pieces().peekable();
    while let Some(piece) = pieces.next() {
        match piece {
            Piece::Bytes(bytes) => {
                if at == 0 {
                    assert_eq!(bytes.get(..5), Some(&b"%PDF-"[..]));
                }
                if pieces.peek().is_none() {
                    // The main section's the last, and it starts where they
                    // end.
                    positions.push(at);
                    assert_eq!(layout.main_section(), at);
                }
                if at == hint_at {
                    hint_seen = true;
                    assert_eq!(bytes.len() as u64, hint_len);
                }
                at += bytes.len() as u64;
            }
            Piece::Body(body) => {
                assert_eq!(body, bodies);
                bodies += 1;
                positions.push(at);
                at += lengths[body];
            }
        }
    }
    assert_eq!(bodies, lengths.len());
    assert!(hint_seen, "the hint stream's where it says");
    assert_eq!(at, layout.len());
    for (body, write) in plan.writes().iter().enumerate() {
        assert_eq!(layout.position(write.number), Some(positions[body]));
    }
    assert_eq!(
        layout.first_page_end(),
        positions[plan.part_writes(Part::FirstPage).end]
    );
    // Part 4, the hint stream, part 6.
    let part6 = plan.part_writes(Part::FirstPage);
    assert_eq!(hint_at + hint_len, positions[part6.start]);
    assert_eq!(layout.position(plan.hint_number()), Some(hint_at));
    assert_eq!(
        layout.position(plan.dictionary_number()),
        Some(layout.first_section() - 200)
    );
    assert_eq!(
        plan.first_xref_number().map(|n| layout.position(n)),
        (plan.kind() == Kind::Stream).then_some(Some(layout.first_section()))
    );
    assert_eq!(
        plan.main_xref_number().map(|n| layout.position(n)),
        (plan.kind() == Kind::Stream).then_some(Some(layout.main_section()))
    );
    for container in plan.containers() {
        for (_, number) in &container.members {
            assert_eq!(layout.position(*number), None);
        }
    }

    // What a position in a table is: as if the hint stream weren't there.
    let said = |write: usize| hint::position(positions[write], hint_at, hint_len);
    let span = |range: &std::ops::Range<usize>| positions[range.end] - positions[range.start];
    let tables = hint::tables(
        layout.hint_data(),
        layout.hint_keys(),
        plan.page_count(),
        &Limits::DEFAULT,
    );
    let tables = tables.expect("what's assembled reads back");
    let header = &tables.pages.header;
    assert_eq!(u64::from(header.first_page_at), said(part6.start));
    assert_eq!(tables.pages.pages.len(), plan.page_count() as usize);
    for (page, entry) in (0..plan.page_count()).zip(&tables.pages.pages) {
        let section = match page == plan.first_page() {
            true => part6.clone(),
            false => plan.page_writes(page),
        };
        assert_eq!(entry.objects, section.len() as u64);
        assert_eq!(entry.length, span(&section));
        let ids: Vec<u32> = entry.shared.iter().map(|r| r.0).collect();
        assert_eq!(ids, plan.shared(page));
        assert!(entry.shared.iter().all(|r| r.1 == 0));
        let contents = plan.page_contents(page);
        assert_eq!(
            entry.content_length,
            match contents.is_empty() {
                true => 0,
                false => span(&contents),
            }
        );
        if !contents.is_empty() {
            assert_eq!(
                entry.content_at,
                positions[contents.start] - positions[section.start]
            );
        }
    }
    let shared = &tables.shared;
    assert_eq!(shared.groups.len(), plan.entries().len());
    for (group, entry) in shared.groups.iter().zip(plan.entries()) {
        assert_eq!(group.objects, entry.len() as u64);
        assert_eq!(group.length, span(entry));
    }
    let in_first = (plan.entries().iter())
        .filter(|entry| plan.writes()[entry.start].part == Part::FirstPage)
        .count();
    assert_eq!(shared.header.first_page_entries as usize, in_first);
    let part8 = plan.part_writes(Part::Shared);
    assert_eq!(u64::from(shared.header.first_at), said(part8.start));
    assert_ne!(shared.header.first_object, 0);
    let generic = |table: Generic, range: std::ops::Range<usize>| {
        assert_eq!(table.objects as usize, range.len());
        if range.is_empty() {
            assert_eq!(table, Generic::default());
        } else {
            assert_eq!(table.first_object, plan.writes()[range.start].number);
            assert_eq!(u64::from(table.first_at), said(range.start));
            assert_eq!(u64::from(table.length), span(&range));
        }
    };
    let group = |group| plan.group_writes(group);
    let check = |table: Option<Generic>, which| match (table, group(which)) {
        (Some(table), Some(range)) => generic(table, range),
        (None, None) => {}
        other => panic!("{which:?}: {other:?}"),
    };
    check(tables.outlines, Group::Outlines);
    check(tables.threads, Group::ThreadInfo);
    check(tables.dests, Group::Dests);
    check(tables.info, Group::Info);
    let extended = |table: &Option<Extended>, which| match (table, group(which)) {
        (Some(table), Some(range)) => {
            generic(table.generic, range);
            assert_eq!(table.shared, plan.group_shared(which));
        }
        (None, None) => {}
        other => panic!("{which:?}: {other:?}"),
    };
    extended(&tables.form, Group::Form);
    extended(&tables.structure, Group::Structure);
    extended(&tables.renditions, Group::Renditions);
    assert_eq!(tables.files.is_some(), group(Group::EmbeddedFile).is_some());
    if let Some(files) = &tables.files {
        let mut entries = files.groups.iter();
        for (at, file) in plan.files().iter().enumerate() {
            if !file.writes.is_empty() {
                let entry = entries.next().unwrap();
                assert_eq!(entry.number, file.number);
                assert_eq!(entry.objects as usize, file.writes.len());
                assert_eq!(u64::from(entry.length), span(&file.writes));
                assert_eq!(entry.shared, []);
            }
            if !plan.file_shared(at).is_empty() {
                let entry = entries.next().unwrap();
                assert_eq!(entry.number, file.number);
                assert_eq!((entry.objects, entry.length), (0, 0));
                assert_eq!(entry.shared, plan.file_shared(at));
            }
        }
        assert_eq!(entries.next(), None);
    }
    let thumbs: Vec<_> = (0..plan.page_count())
        .map(|page| plan.thumb_writes(page))
        .filter(|range| !range.is_empty())
        .collect();
    let shared_thumbs = plan.thumb_shared_writes();
    match &tables.thumbnails {
        None => assert!(thumbs.is_empty() && shared_thumbs.is_empty()),
        Some(table) => {
            assert_eq!(table.pages.len(), thumbs.len());
            for (entry, range) in table.pages.iter().zip(&thumbs) {
                assert_eq!(entry.objects, range.len() as u64);
                assert_eq!(entry.length, span(range));
            }
            if let Some(first) = thumbs.first() {
                assert_eq!(table.header.first_object, plan.writes()[first.start].number);
                assert_eq!(u64::from(table.header.first_at), said(first.start));
            }
            assert_eq!(table.header.shared_objects as usize, shared_thumbs.len());
            assert_eq!(u64::from(table.header.shared_length), span(&shared_thumbs));
            let mut page = 0;
            for entry in &table.pages {
                page += entry.gap;
                assert!(!plan.thumb_writes(page).is_empty());
                page += 1;
            }
        }
    }
}

/// `plan` assembled, under lengths from `data`: with each stream as it is; with
/// a first-page stream that wants more room for a few rounds, which settles;
/// and, on a small graph as it's costly, with one that always does, which is
/// refused.
fn assembles(plan: &Plan, data: &[u8]) {
    let mut lengths = data.iter().cycle().map(|byte| 16 + u64::from(*byte));
    let lengths: Vec<u64> = match data.is_empty() {
        true => vec![16; plan.writes().len()],
        false => plan
            .writes()
            .iter()
            .map(|_| lengths.next().unwrap())
            .collect(),
    };
    let options = Options::new((1, 7));
    let layout = assemble(plan, &lengths, &options, plain).expect("settles");
    assembled(plan, &lengths, &layout);
    clean(plan, &layout);

    // Longer for some rounds; shorter after.
    let (mut calls, mut asks) = (0, 0);
    let grown = assemble(plan, &lengths, &options, |stream| {
        let mut encoded = Encoded::plain(stream.data.to_vec());
        if stream.what == StreamKind::FirstXref {
            calls += 1;
            if calls < 5 {
                asks += 1;
                encoded.data.extend(std::iter::repeat_n(0, 40 << calls));
            }
        }
        Ok::<_, Infallible>(encoded)
    });
    let grown = grown.expect("settles");
    assembled(plan, &lengths, &grown);
    clean(plan, &grown);
    assert_eq!(asks > 0, plan.kind() == Kind::Stream);

    if plan.kind() == Kind::Stream && plan.size() as usize <= SMALL {
        let mut longer = 0;
        let refused = assemble(plan, &lengths, &options, |stream| {
            let mut encoded = Encoded::plain(stream.data.to_vec());
            if stream.what == StreamKind::FirstXref {
                // Past each round's room, and not much more: it's what this
                // costs.
                longer = (longer + stream.data.len()) * 3 / 2 + 200;
                encoded.data.extend(std::iter::repeat_n(0, longer));
            }
            Ok::<_, Infallible>(encoded)
        });
        assert!(matches!(refused, Err(layout::Error::Unsettled)));
    }

    // Not one length a write.
    let refused = assemble(plan, &lengths[1..], &options, plain);
    assert!(matches!(refused, Err(layout::Error::Lengths { .. })));
}

/// Any bytes as a hint stream of a file of any number of pages, its tables at
/// any positions, under small caps: an error, or tables that write and read the
/// same.
pub fn tables(data: &[u8]) {
    let (mut limits, mut small) = (Limits::DEFAULT, Bytes(data));
    limits.pages = 256;
    limits.hint_entries = 1 << 12;
    let pages = match small.byte() % 4 {
        0 => small.below(8) as u32,
        1 => small.below(300) as u32,
        2 => u32::from_be_bytes([small.byte(), small.byte(), small.byte(), small.byte()]),
        _ => 1,
    };
    let body = data.get(16..).unwrap_or(&[]);
    let key = |small: &mut Bytes| small.below(body.len() + 2) as u64;
    let optional = |small: &mut Bytes| small.chance(2).then(|| small.below(body.len() + 2) as u64);
    let mut keys = HintKeys::new(key(&mut small));
    keys.thumbnails = optional(&mut small);
    keys.outlines = optional(&mut small);
    keys.threads = optional(&mut small);
    keys.dests = optional(&mut small);
    keys.form = optional(&mut small);
    keys.info = optional(&mut small);
    keys.structure = optional(&mut small);
    keys.labels = optional(&mut small);
    keys.renditions = optional(&mut small);
    keys.files = optional(&mut small);

    let read = hint::tables(body, &keys, pages, &limits);
    // Caps that are larger don't take back what they let through.
    if let Ok(read) = &read {
        let default = hint::tables(body, &keys, pages, &Limits::DEFAULT);
        assert_eq!(default.as_ref(), Ok(read));
    }
    let Ok(read) = read else {
        return;
    };

    // Each table, written and read again; then all of them, one after another,
    // their keys where they're now, the same to `tables`.
    let mut all = Vec::new();
    let mut again = HintKeys::new(0);
    let limits = &limits;
    let id_bits = read.pages.header.id_bits;
    let mut each = |write: &dyn Fn(&mut Vec<u8>) -> Result<(), hint::Error>| {
        let at = all.len() as u64;
        write(&mut all).expect("what was read writes");
        at
    };
    each(&|out| read.pages.write(out));
    again.shared = each(&|out| read.shared.write(out));
    let generic = |table: Generic| {
        let mut out = Vec::new();
        table.write(&mut out).unwrap();
        assert_eq!(Generic::read(&out, limits), Ok(table));
    };
    let extended = |table: &Extended| {
        let mut out = Vec::new();
        table.write(&mut out).unwrap();
        assert_eq!(Extended::read(&out, limits).as_ref(), Ok(table));
    };
    if let Some(table) = &read.thumbnails {
        again.thumbnails = Some(each(&|out| table.write(out)));
    }
    let mut one = |key: &mut Option<u64>, table: Option<Generic>| {
        if let Some(table) = table {
            generic(table);
            *key = Some(each(&|out| table.write(out)));
        }
    };
    let mut keys_again = again;
    one(&mut keys_again.outlines, read.outlines);
    one(&mut keys_again.threads, read.threads);
    one(&mut keys_again.dests, read.dests);
    one(&mut keys_again.info, read.info);
    one(&mut keys_again.labels, read.labels);
    let mut one = |key: &mut Option<u64>, table: &Option<Extended>| {
        if let Some(table) = table {
            extended(table);
            *key = Some(each(&|out| table.write(out)));
        }
    };
    one(&mut keys_again.form, &read.form);
    one(&mut keys_again.structure, &read.structure);
    one(&mut keys_again.renditions, &read.renditions);
    if let Some(table) = &read.files {
        keys_again.files = Some(each(&|out| table.write(out, id_bits)));
        let mut out = Vec::new();
        table.write(&mut out, id_bits).unwrap();
        assert_eq!(
            EmbeddedFiles::read(&out, id_bits, limits).as_ref(),
            Ok(table)
        );
    }
    if let Some(table) = &read.thumbnails {
        let mut out = Vec::new();
        table.write(&mut out).unwrap();
        assert_eq!(Thumbnails::read(&out, limits).as_ref(), Ok(table));
    }
    let mut out = Vec::new();
    read.pages.write(&mut out).unwrap();
    assert_eq!(
        PageOffsets::read(&out, pages, limits).as_ref(),
        Ok(&read.pages)
    );
    let mut out = Vec::new();
    read.shared.write(&mut out).unwrap();
    assert_eq!(SharedObjects::read(&out, limits).as_ref(), Ok(&read.shared));
    assert_eq!(
        hint::tables(&all, &keys_again, pages, limits).as_ref(),
        Ok(&read)
    );
}

/// Where [`group`](crate::group) puts what's `used` so among the members of an
/// object stream: by page, then by number.
fn rank(used: Use) -> (u32, u32) {
    match used {
        Use::Page(page) => (0, page),
        Use::Shared => (1, 0),
        Use::None => (2, 0),
    }
}

/// Whether a use is a later page's, or several's.
fn later(used: Use) -> bool {
    matches!(used, Use::Shared | Use::Page(1..))
}

/// Any bytes as a graph: refused for what it has, or planned, each promise of
/// [`plan`](crate::plan()) held against plain walks of it, the same twice, then
/// assembled, each promise of [`assemble`] held against what it says.
pub fn graphs(data: &[u8]) {
    let described = described(data);
    let graph = described.graph();
    match plan(&graph, &Limits::DEFAULT) {
        Ok(planned) => {
            assert_eq!(described.refusal(), None, "{described:?}");
            holds(&described, &graph, &planned);
            assert_eq!(plan(&graph, &Limits::DEFAULT).as_ref(), Ok(&planned));
            assembles(&planned, data);
        }
        Err(refused) => assert_eq!(described.refusal(), Some(refused), "{described:?}"),
    }
}

/// The facts `plan` and `layout` say a file has, as a reader would give them.
pub(crate) fn facts_of(plan: &Plan, layout: &Layout) -> Facts {
    let mut facts = Facts::new(layout.len());
    let dictionary = plan.dictionary_number();
    let dictionary_at = layout.position(dictionary).expect("it's written");
    let mut first = FirstObject::new(dictionary_at, layout.first_section(), dictionary, 0);
    let (hint_at, hint_len) = layout.hint_extent();
    let int = |n: u64| Raw::Integer(n as i64);
    first.linearized = Raw::Integer(1);
    first.l = int(layout.len());
    first.h = Raw::Array;
    first.h_items = vec![int(hint_at), int(hint_len)];
    first.o = Raw::Integer(i64::from(plan.first_page_number()));
    first.e = int(layout.first_page_end());
    first.n = Raw::Integer(i64::from(plan.page_count()));
    first.t = int(layout.main_first_entry());
    if plan.first_page() != 0 {
        first.p = Raw::Integer(i64::from(plan.first_page()));
    }
    facts.first_object = Some(first);

    let mut rows = vec![RowEntry::Free; plan.size() as usize];
    for (number, row) in (0u32..).zip(&mut rows) {
        if let Some(at) = layout.position(number) {
            *row = RowEntry::InUse(at);
        }
    }
    for container in plan.containers() {
        for (index, (_, number)) in (0u32..).zip(&container.members) {
            rows[*number as usize] = RowEntry::Compressed {
                stream: container.number,
                index,
            };
        }
    }
    let row_of = |number: u32| {
        let generation = match number {
            0 => u16::MAX,
            _ => 0,
        };
        RowFacts::new(number, generation, rows[number as usize])
    };
    let kind = plan.kind();
    let size = plan.size();
    let mut main = SectionFacts::new(layout.main_section(), kind);
    main.subsections = vec![(0, dictionary)];
    main.rows = (0..dictionary).map(row_of).collect();
    main.startxref = Some(layout.first_section());
    main.stream = plan.main_xref_number();
    main.size = Raw::Integer(i64::from(dictionary));
    main.first_row = Some(match kind {
        Kind::Table => (layout.main_first_entry(), layout.main_first_entry() + 1),
        Kind::Stream => (layout.main_section(), layout.main_section()),
    });
    let mut section = SectionFacts::new(layout.first_section(), kind);
    section.subsections = vec![(dictionary, size - dictionary)];
    section.rows = (dictionary..size).map(row_of).collect();
    section.size = Raw::Integer(i64::from(size));
    section.prev = int(layout.main_section());
    section.root = Raw::Reference(ObjectId::new(plan.catalog_number(), 0));
    section.startxref = Some(0);
    section.stream = plan.first_xref_number();
    facts.first = Some(section);
    facts.main = Some(main);

    let mut hint = HintFacts::new(
        hint_at,
        hint_at + hint_len,
        plan.hint_number(),
        HintData::Decoded(layout.hint_data().to_vec()),
    );
    let keys = layout.hint_keys();
    let optional = [
        (&b"T"[..], keys.thumbnails),
        (b"O", keys.outlines),
        (b"A", keys.threads),
        (b"E", keys.dests),
        (b"V", keys.form),
        (b"I", keys.info),
        (b"C", keys.structure),
        (b"L", keys.labels),
        (b"R", keys.renditions),
        (b"B", keys.files),
    ];
    hint.keys.push((b"S".to_vec(), int(keys.shared)));
    for (key, at) in optional {
        if let Some(at) = at {
            hint.keys.push((key.to_vec(), int(at)));
        }
    }
    hint.keys.push((b"Length".to_vec(), int(hint_len)));
    facts.hint = Some(hint);
    facts.pages = (0..plan.page_count())
        .map(|page| {
            let number = match page == plan.first_page() {
                true => plan.first_page_number(),
                false => plan.writes()[plan.page_writes(page).start].number,
            };
            let mut facts = PageFacts::new(Some(ObjectId::new(number, 0)));
            facts.resources = true;
            facts.media_box = true;
            facts
        })
        .collect();
    facts
}

/// Told none, and how often it's asked.
#[derive(Default)]
struct Unwanted {
    told: usize,
    asked: std::cell::Cell<usize>,
}

impl Sink for Unwanted {
    fn finding(&mut self, _: Finding) {
        self.told += 1;
    }

    fn wants(&self) -> bool {
        self.asked.set(self.asked.get() + 1);
        false
    }
}

/// The same verdict whatever the sink, none told what doesn't want any, and
/// that verdict what the findings say. What's kept is given back.
fn judged_alike(facts: &Facts, limits: &Limits) -> (check::Verdict, Vec<Finding>) {
    let mut kept = Vec::new();
    let verdict = check::check(facts, limits, &mut kept);
    assert_eq!(check::check(facts, limits, &mut ()), verdict);
    let mut unwanted = Unwanted::default();
    assert_eq!(check::check(facts, limits, &mut unwanted), verdict);
    assert_eq!(unwanted.told, 0);
    let by = &mut kept.clone() as &mut dyn Sink;
    assert_eq!(check::check(facts, limits, by), verdict);
    let error = kept.iter().any(|finding| finding.grade() == Grade::Error);
    assert_eq!(verdict.holds, verdict.linearized && !error, "{kept:?}");
    (verdict, kept)
}

/// What's assembled holds: the check of it finds nothing but what a plan can't
/// help.
fn clean(plan: &Plan, layout: &Layout) {
    let (verdict, findings) = judged_alike(&facts_of(plan, layout), &Limits::DEFAULT);
    assert!(verdict.linearized && verdict.holds && verdict.hints_read);
    assert_eq!(findings, [], "{plan:?}");
}

/// Any facts: judged under small caps, never panicking, the same whatever the
/// sink. Half are what's assembled from a plan of the input, each field given a
/// value of the input's; half are all from the input.
pub fn facts(data: &[u8]) {
    let mut limits = Limits::DEFAULT;
    limits.pages = 256;
    limits.hint_entries = 1 << 12;
    limits.objects = 1 << 12;
    limits.shared_entries = 1 << 12;
    limits.shared_visits = 1 << 16;
    let described = described(data);
    let mut bytes = Bytes(data);
    let choice = bytes.byte();
    let mut facts = match plan(&described.graph(), &Limits::DEFAULT) {
        Ok(planned) if choice & 1 == 0 && planned.size() < 2000 => {
            let lengths = vec![16 + u64::from(choice); planned.writes().len()];
            let options = Options::new((1, 7));
            let layout = assemble(&planned, &lengths, &options, plain).expect("settles");
            let mut facts = facts_of(&planned, &layout);
            spoiled(&mut facts, &mut bytes);
            facts
        }
        _ => made_up(&mut bytes),
    };
    if choice & 2 != 0 {
        facts.graph = Some(described.graph());
    }
    judged_alike(&facts, &limits);
}

/// A number of the input's, near where numbers are and far.
fn number_from(bytes: &mut Bytes) -> u64 {
    match bytes.below(4) {
        0 => bytes.below(64) as u64,
        1 => bytes.below(1 << 16) as u64,
        2 => u64::from_be_bytes(std::array::from_fn(|_| bytes.byte())),
        _ => u64::from(u32::MAX) - bytes.below(8) as u64,
    }
}

/// A number an object has, of the input's: few, now and then the last there can
/// be.
fn object_from(bytes: &mut Bytes) -> u32 {
    match bytes.chance(16) {
        true => u32::MAX - bytes.below(2) as u32,
        false => bytes.below(64) as u32,
    }
}

/// An `/N` of the input's: a table of none reads, as do a few pages.
fn count_from(bytes: &mut Bytes) -> Raw {
    match bytes.below(4) {
        0 => Raw::Integer(0),
        1 => Raw::Integer(bytes.below(8) as i64),
        _ => raw_from(bytes),
    }
}

/// A value of the input's.
fn raw_from(bytes: &mut Bytes) -> Raw {
    match bytes.below(8) {
        0 => Raw::Absent,
        1 => Raw::Real(number_from(bytes) as f64 / 3.0),
        2 => Raw::Reference(ObjectId::new(number_from(bytes) as u32, 0)),
        3 => Raw::Array,
        4 => Raw::Other,
        _ => Raw::Integer(number_from(bytes) as i64),
    }
}

fn row_from(bytes: &mut Bytes) -> RowFacts {
    let entry = match bytes.below(4) {
        0 => RowEntry::Free,
        1 => RowEntry::Compressed {
            stream: object_from(bytes),
            index: bytes.below(8) as u32,
        },
        _ => RowEntry::InUse(number_from(bytes)),
    };
    RowFacts::new(object_from(bytes), bytes.below(2) as u16, entry)
}

fn section_from(bytes: &mut Bytes) -> SectionFacts {
    let kind = match bytes.below(2) {
        0 => Kind::Table,
        _ => Kind::Stream,
    };
    let mut section = SectionFacts::new(number_from(bytes), kind);
    section.rows = (0..bytes.below(32)).map(|_| row_from(bytes)).collect();
    section.hybrid = (0..bytes.below(4)).map(|_| row_from(bytes)).collect();
    section.subsections = (0..bytes.below(4))
        .map(|_| (bytes.below(64) as u32, bytes.below(64) as u32))
        .collect();
    section.size = raw_from(bytes);
    section.prev = raw_from(bytes);
    section.root = raw_from(bytes);
    section.first_row = bytes
        .chance(2)
        .then(|| (number_from(bytes), number_from(bytes)));
    section.startxref = bytes.chance(2).then(|| number_from(bytes));
    section.stream = bytes.chance(2).then(|| bytes.below(64) as u32);
    section
}

fn hint_from(bytes: &mut Bytes) -> HintFacts {
    let data = match bytes.below(8) {
        0 => HintData::Locked,
        1 => HintData::NotAStream,
        2 => HintData::Undecodable,
        _ => {
            let length = bytes.below(bytes.0.len() + 1).min(bytes.0.len());
            let (data, rest) = bytes.0.split_at(length);
            bytes.0 = rest;
            HintData::Decoded(data.to_vec())
        }
    };
    // The tables read only from an /S inside the data, so it's mostly that.
    let room = match &data {
        HintData::Decoded(data) => data.len(),
        _ => 0,
    };
    let mut hint = HintFacts::new(
        number_from(bytes),
        number_from(bytes),
        bytes.below(64) as u32,
        data,
    );
    hint.overflow = bytes
        .chance(4)
        .then(|| (number_from(bytes), number_from(bytes)));
    let keys: [&[u8]; 8] = [b"S", b"T", b"O", b"I", b"V", b"B", b"Length", b"P"];
    for key in keys {
        if !bytes.chance(4) {
            let raw = match key == b"S" && !bytes.chance(4) {
                true => Raw::Integer(bytes.below(room + 1) as i64),
                false => raw_from(bytes),
            };
            hint.keys.push((key.to_vec(), raw));
        }
    }
    hint
}

/// Facts from the input alone, what the rest depends on first, mostly as it
/// should be.
fn made_up(bytes: &mut Bytes) -> Facts {
    let len = number_from(bytes);
    let mut facts = Facts::new(len);
    facts.header = bytes.below(4) as u64 * 512;
    let mut first = FirstObject::new(
        number_from(bytes),
        number_from(bytes),
        bytes.below(64) as u32,
        bytes.below(2) as u16,
    );
    first.stream = bytes.chance(16);
    first.linearized = match bytes.chance(8) {
        true => raw_from(bytes),
        false => Raw::Integer(1),
    };
    first.l = match bytes.chance(8) {
        true => raw_from(bytes),
        false => Raw::Integer(len.saturating_sub(facts.header) as i64),
    };
    first.h = match bytes.chance(8) {
        true => raw_from(bytes),
        false => Raw::Array,
    };
    first.h_items = (0..bytes.below(6)).map(|_| raw_from(bytes)).collect();
    first.o = raw_from(bytes);
    first.e = raw_from(bytes);
    first.n = count_from(bytes);
    first.t = raw_from(bytes);
    first.p = raw_from(bytes);
    facts.first_object = bytes
        .chance(16)
        .then_some(())
        .map_or(Some(first), |()| None);
    facts.first = (!bytes.chance(16)).then(|| section_from(bytes));
    facts.main = (!bytes.chance(16)).then(|| section_from(bytes));
    facts.update = bytes.chance(4).then(|| number_from(bytes));
    facts.hint = (!bytes.chance(8)).then(|| hint_from(bytes));
    facts.pages = (0..bytes.below(32))
        .map(|_| {
            let id = bytes
                .chance(8)
                .then_some(())
                .map_or(Some(ObjectId::new(bytes.below(64) as u32, 0)), |()| None);
            let mut page = PageFacts::new(id);
            page.resources = bytes.chance(2);
            page.media_box = bytes.chance(2);
            page
        })
        .collect();
    facts.nodes = (0..bytes.below(4))
        .map(|_| (bytes.below(64) as u32, check::Attribute::Rotate))
        .collect();
    facts
}

/// What's assembled, with a few of its fields given values of the input's.
fn spoiled(facts: &mut Facts, bytes: &mut Bytes) {
    for _ in 0..1 + bytes.below(4) {
        match bytes.below(16) {
            0 => facts.len = number_from(bytes),
            1 => facts.first = Some(section_from(bytes)),
            2 => facts.main = Some(section_from(bytes)),
            3 => facts.hint = Some(hint_from(bytes)),
            4 => facts.pages.truncate(bytes.below(8)),
            n => {
                let Some(first) = &mut facts.first_object else {
                    continue;
                };
                let raw = raw_from(bytes);
                match n {
                    5 => first.h_items = vec![raw, raw],
                    6 => first.o = raw,
                    7 => first.e = raw,
                    8 => first.n = count_from(bytes),
                    9 => first.t = raw,
                    10 => first.p = raw,
                    11 => first.linearized = raw,
                    12 => first.l = raw,
                    _ => {
                        let Some(HintData::Decoded(data)) =
                            facts.hint.as_mut().map(|hint| &mut hint.data)
                        else {
                            continue;
                        };
                        if !data.is_empty() {
                            let at = bytes.below(data.len());
                            data[at] = bytes.byte();
                        }
                    }
                }
            }
        }
    }
}

/// Any bytes as a file, read and written again in the three kinds: in the
/// clear; as it's encrypted, where it is, and a password opens it; and now and
/// then, by the input's length, under a new encryption. Where it's written, it
/// reads linearized by its password, with the same pages, and every object the
/// plan numbers is the same value at its number under the numbering, a stream's
/// data the same; on any refusal, `out` as it was. The new encryption's key and
/// every AES IV are the operating system's, so the bytes written differ from run
/// to run, and a failure that hangs on them may not come again from its input.
#[cfg(feature = "family")]
pub fn over(data: &[u8]) {
    use pdf_structure::XrefKind;
    use pdf_structure::read::{self, Reading};
    use pdf_structure::write::Encryptor;

    use crate::family;

    let Ok(reading) = Reading::open(data, &read::Options::DEFAULT) else {
        return;
    };
    let Ok(doc) = reading.to_document() else {
        return;
    };
    let own = Encryptor::of(&reading).ok().flatten();
    let new = data.len().is_multiple_of(8).then(|| {
        let password = pdf_structure::pdf_crypt::Password(NEW).prepared().ok()?;
        let mut protection = pdf_structure::pdf_crypt::Protection::new(&password);
        protection.encrypt_metadata = data.len().is_multiple_of(16);
        Encryptor::create(&protection).ok()
    });
    for (kind, stream, pack) in [
        (Kind::Table, XrefKind::Table, false),
        (Kind::Stream, XrefKind::Stream, false),
        (Kind::Stream, XrefKind::Stream, true),
    ] {
        let mut options = family::Options::DEFAULT;
        options.kind = stream;
        options.pack = pack;
        options.threads = 1;
        written_back(&reading, &doc, None, "", kind, &options);
        if let Some(own) = &own {
            written_back(&reading, &doc, Some(own), "", kind, &options);
        }
        if let Some(Some(new)) = &new {
            written_back(&reading, &doc, Some(new), NEW, kind, &options);
        }
    }
}

/// The password of what [`over`] writes under a new encryption.
#[cfg(feature = "family")]
const NEW: &str = "new";

/// What [`over`] holds of `doc`, written under `encryptor` as `options` says,
/// which `password` opens, or the empty one: among it, that its facts read
/// whole beside its reading are those of its reading alone.
#[cfg(feature = "family")]
fn written_back<S: pdf_structure::ByteSource>(
    reading: &pdf_structure::Reading<S>,
    doc: &pdf_structure::Document,
    encryptor: Option<&pdf_structure::write::Encryptor>,
    password: &str,
    kind: Kind,
    options: &crate::family::Options<'_>,
) {
    use pdf_objects::Value;
    use pdf_semantics::Rules;
    use pdf_semantics::pages::{Inheritable, Origin, pages};
    use pdf_structure::Encryption;
    use pdf_structure::read::{self, Reading, WithReading};
    use pdf_structure::write::Encryptor;

    use crate::family::{self, graph::Prepared};

    let mut out = b"before".to_vec();
    let written = family::linearize(&mut out, doc, encryptor, Some(reading), options);
    let Ok(done) = written else {
        assert_eq!(out, b"before");
        return;
    };
    assert_eq!(out.len() as u64 - 6, done.layout.len());
    let file = &out[6..];
    let passwords = [pdf_structure::pdf_crypt::Password(password)];
    let mut opened = read::Options::DEFAULT;
    opened.passwords = if password.is_empty() { &[] } else { &passwords };
    let again = Reading::open(file, &opened).expect("it reads");
    assert!(again.facts().linearized);
    assert_eq!(
        matches!(again.encryption(), Encryption::Opened { .. }),
        encryptor.is_some(),
        "{:?}",
        again.encryption()
    );
    let back = again.to_document().expect("it reads whole");

    let pack = options.pack && encryptor.is_none_or(Encryptor::packs);
    let prepared = Prepared::of(doc, encryptor, options).expect("it was written");
    let planned = plan(
        &prepared.graph(kind, pack, &Default::default()),
        &options.limits,
    );
    let planned = planned.expect("it was planned");
    let limits = pdf_semantics::Limits::DEFAULT;
    let root = back
        .trailer()
        .dict()
        .get(b"Root")
        .and_then(|root| root.as_reference());
    let catalog = root
        .and_then(|root| back.get_dict(root))
        .expect("it has a catalog");
    let listed = pages(&back, catalog, Rules::Iso, &limits, &mut ()).expect("it has pages");
    assert_eq!(listed.len(), planned.page_count() as usize);
    // What's written holds, nothing that says otherwise of what a page reaches.
    let read = family::facts(&again, &Limits::DEFAULT).expect("it's in memory");
    // The same facts of it read whole beside its reading.
    let whole = WithReading::new(file, &opened).expect("it reads");
    let beside = family::facts_with_reading(&whole, &Limits::DEFAULT).expect("it's in memory");
    assert_eq!(format!("{beside:?}"), format!("{read:?}"));
    let (verdict, findings) = judged_alike(&read, &Limits::DEFAULT);
    assert!(verdict.linearized && verdict.holds && verdict.hints_read);
    assert!(
        (findings.iter()).all(|finding| matches!(finding, Finding::PageAttribute { .. })),
        "{findings:?}"
    );
    for page in &listed {
        for attribute in [
            Inheritable::Resources,
            Inheritable::MediaBox,
            Inheritable::CropBox,
            Inheritable::Rotate,
        ] {
            if let Some(attribute) = page.attribute(attribute) {
                assert_eq!(attribute.origin, Origin::Own, "it's pushed down");
            }
        }
    }
    for (id, body) in prepared.bodies() {
        let ObjectId { number, .. } = planned.number(*id).expect("it was given");
        let Ok(object) = again.get(ObjectId::new(number, 0)) else {
            panic!("{id:?} isn't at {number}");
        };
        same_under(body.object(), object, &planned);
        if let (Value::Stream(_), Value::Stream(_)) = (body.object().value(), object.value())
            && let (Ok(before), Ok(after)) =
                (reading.data(*id), again.data(ObjectId::new(number, 0)))
        {
            assert_eq!(before, after, "{id:?}'s data");
        }
    }
}

/// Whether `written`, read back, is `value` under `planned`'s numbers: a
/// reference to what it doesn't number reads null.
#[cfg(feature = "family")]
fn same_under(value: &pdf_objects::Object, written: &pdf_objects::Object, planned: &Plan) {
    use pdf_objects::{Dict, Value};

    let mut stack = vec![(value, written)];
    let dicts = |a: &'_ Dict, b: &'_ Dict| {
        assert_eq!(
            a.iter().filter(|(key, _)| *key != b"Length").count(),
            b.iter().filter(|(key, _)| *key != b"Length").count(),
            "{a:?} {b:?}"
        );
    };
    while let Some((a, b)) = stack.pop() {
        match (a.value(), b.value()) {
            (Value::Ref(id), written) => match planned.number(*id) {
                Some(number) => assert_eq!(written, &Value::Ref(number), "{id:?}"),
                None => assert_eq!(written, &Value::Null, "{id:?}"),
            },
            (Value::Array(x), Value::Array(y)) => {
                assert_eq!(x.len(), y.len());
                stack.extend(x.iter().zip(y));
            }
            (Value::Dict(x), Value::Dict(y)) => {
                dicts(x, y);
                let pairs = x.iter().filter(|(key, _)| *key != b"Length");
                stack.extend(pairs.map(|(key, value)| (value, y.get(key).expect("kept"))));
            }
            (Value::Stream(x), Value::Stream(y)) => {
                dicts(&x.dict, &y.dict);
                let pairs = x.dict.iter().filter(|(key, _)| *key != b"Length");
                stack.extend(pairs.map(|(key, value)| (value, y.dict.get(key).expect("kept"))));
            }
            (Value::String(x, _), Value::String(y, _)) => assert_eq!(x.as_bytes(), y.as_bytes()),
            (Value::Real(x), Value::Real(y)) => {
                assert!(
                    x == y || (*x as f32) == (*y as f32) || x.is_nan(),
                    "{x} {y}"
                );
            }
            (Value::Real(x), Value::Integer(y)) => assert_eq!(*x, *y as f64),
            (x, y) => assert_eq!(x, y),
        }
    }
}

/// The objects [`lopdf_document`] starts from: a tree of three pages by two
/// nodes, what they inherit, share and have alone, and an outline and a page no
/// tree lists.
#[cfg(all(feature = "family", feature = "lopdf"))]
fn lopdf_start() -> ::lopdf::Document {
    use ::lopdf::{Dictionary, Document, Object, Stream, dictionary};

    let reference = |number: u32| Object::Reference((number, 0));
    let page = |parent: u32, more: Dictionary| {
        let mut dict = dictionary! { "Type" => "Page", "Parent" => reference(parent) };
        dict.extend(&more);
        Object::Dictionary(dict)
    };
    let mut doc = Document::new();
    doc.version = "1.7".to_string();
    let objects: [(u32, Object); 13] = [
        (
            1,
            Object::Dictionary(dictionary! {
                "Type" => "Catalog",
                "Pages" => reference(2),
                "Outlines" => reference(9),
                "Foo" => reference(12),
                "OpenAction" => vec![reference(4), Object::Name(b"Fit".to_vec())],
            }),
        ),
        (
            2,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![reference(3), reference(10)],
                "Count" => 3,
                "MediaBox" => vec![0.into(), 0.into(), 100.into(), reference(13)],
                "Resources" => reference(7),
                "Rotate" => 90,
            }),
        ),
        (3, page(2, dictionary! { "Contents" => reference(5) })),
        (
            4,
            page(
                10,
                dictionary! { "Contents" => reference(6), "X" => reference(8) },
            ),
        ),
        (
            5,
            Object::Stream(Stream::new(Dictionary::new(), b"q Q".to_vec())),
        ),
        (
            6,
            Object::Stream(Stream::new(Dictionary::new(), b"Q q".to_vec())),
        ),
        (
            7,
            Object::Dictionary(dictionary! { "Font" => Dictionary::new() }),
        ),
        (8, Object::Dictionary(dictionary! { "Mine" => true })),
        (9, Object::Dictionary(dictionary! { "Count" => 0 })),
        (
            10,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Parent" => reference(2),
                "Kids" => vec![reference(4), reference(11)],
                "Count" => 2,
            }),
        ),
        (11, page(10, dictionary! { "X" => reference(8) })),
        (12, page(10, dictionary! { "Mine" => true })),
        (13, Object::Integer(200)),
    ];
    doc.objects.extend(
        objects
            .into_iter()
            .map(|(number, object)| ((number, 0), object)),
    );
    doc.trailer.set("Root", reference(1));
    doc
}

/// The fuzz body for the lopdf adapter: any bytes as edits to the objects of a
/// document of lopdf's, so its reader isn't what's under test: refused, with
/// `out` as it was given, or written, its positions counted right, and what's
/// written saying it's linearized and holding, nothing said but what a page
/// doesn't have to push, its facts the same read whole beside its reading as of
/// its reading alone. It gives whether it wrote.
#[cfg(all(feature = "family", feature = "lopdf"))]
pub fn lopdf_document(data: &[u8]) -> bool {
    use ::lopdf::{Dictionary, Object};
    use pdf_structure::read::{self, Reading, WithReading};

    use crate::family;

    const KEYS: [&str; 16] = [
        "Parent",
        "Kids",
        "Type",
        "MediaBox",
        "CropBox",
        "Resources",
        "Rotate",
        "Contents",
        "X",
        "Count",
        "First",
        "Next",
        "Foo",
        "UserUnit",
        "Pages",
        "Thumb",
    ];
    const OBJECTS: u32 = 16;
    let id = |byte: u8| (u32::from(byte) % OBJECTS + 1, 0);
    let value = |of: u8, byte: u8| match of % 11 {
        0 => Object::Null,
        1 => Object::Integer(i64::from(byte)),
        2 | 3 => Object::Reference(id(byte)),
        4 => Object::Name(if byte.is_multiple_of(2) {
            b"Page".to_vec()
        } else {
            b"Pages".to_vec()
        }),
        5 => Object::Array(vec![Object::Reference(id(byte))]),
        6 => Object::Array(Vec::new()),
        7 => Object::Array(vec![
            0.into(),
            0.into(),
            9.into(),
            Object::Reference(id(byte)),
        ]),
        8 => Object::Dictionary(Dictionary::new()),
        9 => Object::Reference((u32::from(byte) + 100, 0)),
        _ => Object::Real(f32::from(byte)),
    };
    let mut doc = lopdf_start();
    for &[what, at, key, byte] in data.as_chunks::<4>().0 {
        let at = id(at);
        let key = KEYS[usize::from(key) % KEYS.len()].as_bytes();
        match what % 4 {
            0 | 1 => {
                let dict = match doc.objects.get_mut(&at) {
                    Some(Object::Dictionary(dict)) => dict,
                    Some(Object::Stream(stream)) => &mut stream.dict,
                    _ => continue,
                };
                match what % 8 {
                    0 | 1 | 4 | 5 => dict.set(key.to_vec(), value(what >> 3, byte)),
                    _ => {
                        dict.remove(key);
                    }
                }
            }
            2 => {
                doc.objects.insert(at, value(what >> 2, byte));
            }
            _ => {
                doc.objects.remove(&at);
            }
        }
    }
    let mut out = b"before".to_vec();
    let options = crate::lopdf::Options::DEFAULT;
    let Ok(layout) = crate::lopdf::linearize(&mut out, &doc, &options) else {
        assert_eq!(out, b"before");
        return false;
    };
    assert_eq!(out.len() as u64 - 6, layout.len());
    let file = &out[6..];
    let reading = Reading::open(file, &read::Options::DEFAULT).expect("it reads");
    assert!(reading.facts().linearized);
    let read = family::facts(&reading, &Limits::DEFAULT).expect("it's in memory");
    // The same facts of it read whole beside its reading.
    let whole = WithReading::new(file, &read::Options::DEFAULT).expect("it reads");
    let beside = family::facts_with_reading(&whole, &Limits::DEFAULT).expect("it's in memory");
    assert_eq!(format!("{beside:?}"), format!("{read:?}"));
    let (verdict, findings) = judged_alike(&read, &Limits::DEFAULT);
    assert!(
        verdict.linearized && verdict.holds && verdict.hints_read,
        "{findings:?}"
    );
    assert!(
        (findings.iter()).all(|finding| matches!(finding, Finding::PageAttribute { .. })),
        "{findings:?}"
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain generator: what's fuzzed only has to vary, and a seed gives the
    /// same inputs each time.
    fn generator(mut state: u64) -> impl FnMut() -> u64 {
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        }
    }

    /// Bytes of an input, each from `random`, kept below `most` so that they
    /// mean something: references to few objects, a few of each.
    fn input(len: usize, most: u8, random: &mut impl FnMut() -> u64) -> Vec<u8> {
        (0..len).map(|_| (random() % most as u64) as u8).collect()
    }

    /// Inputs that make graphs of many shapes: a run through every flag, small
    /// and large, with few distinct bytes and with any.
    fn seeds() -> Vec<Vec<u8>> {
        let mut random = generator(0x9E37_79B9_7F4A_7C15);
        let mut seeds = Vec::new();
        for flags in 0..8 {
            for (len, most) in [(24, 8), (200, 16), (2000, 32), (4000, 255)] {
                let mut seed = input(len, most, &mut random);
                seed[0] = flags;
                seeds.push(seed);
            }
        }
        seeds
    }

    /// Inputs of what's planned and assembled, as hint streams: 16 bytes that
    /// say how to read them, then the data.
    fn table_seeds() -> Vec<Vec<u8>> {
        let mut seeds_of = Vec::new();
        for seed in seeds() {
            let described = described(&seed);
            let Ok(planned) = plan(&described.graph(), &Limits::DEFAULT) else {
                continue;
            };
            let lengths = vec![40; planned.writes().len()];
            let layout = assemble(&planned, &lengths, &Options::new((1, 7)), plain).unwrap();
            let data = layout.hint_data();
            let mut input = vec![3, 0, 0, 0];
            input.resize(16, 0);
            input.extend_from_slice(data);
            seeds_of.push(input);
        }
        seeds_of
    }

    /// Files of what the family adapter treats apart: inherited attributes, a
    /// form, an outline, threads, document parts, a thumbnail, each as read and
    /// as it lays them out.
    #[cfg(feature = "family")]
    fn file_seeds() -> Vec<Vec<u8>> {
        let file = |objects: &[&str]| {
            let mut out = b"%PDF-1.7\n".to_vec();
            let mut rows = Vec::new();
            for (number, object) in (1..).zip(objects) {
                rows.push(out.len());
                out.extend_from_slice(format!("{number} 0 obj\n{object}\nendobj\n").as_bytes());
            }
            let at = out.len();
            out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
            out.extend_from_slice(b"0000000000 65535 f \n");
            for row in rows {
                out.extend_from_slice(format!("{row:010} 00000 n \n").as_bytes());
            }
            let size = objects.len() + 1;
            let tail = format!(
                "trailer\n<< /Size {size} /Root 1 0 R /Info 13 0 R >>\nstartxref\n{at}\n%%EOF\n"
            );
            out.extend_from_slice(tail.as_bytes());
            out
        };
        let all = file(&[
            "<< /Type /Catalog /Pages 2 0 R /Outlines 8 0 R /AcroForm 10 0 R /Threads [14 0 R] /DPartRoot 16 0 R /OpenAction [4 0 R /Fit] /PageMode /UseOutlines >>",
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 9 9] /Resources 7 0 R /Rotate 90 >>",
            "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Thumb 12 0 R /DPart 17 0 R >>",
            "<< /Type /Page /Parent 2 0 R /Contents [6 0 R] /Annots [11 0 R] /B [15 0 R] >>",
            "<< /Length 3 >>\nstream\nq Q\nendstream",
            "<< /Length 18 0 R >>\nstream\nQ q\nendstream",
            "<< /Font << /F0 << /Type /Font /Subtype /Type1 /BaseFont /Courier >> >> >>",
            "<< /First 9 0 R /Last 9 0 R /Count 1 >>",
            "<< /Title (a) /Parent 8 0 R /Dest [3 0 R /Fit] >>",
            "<< /Fields [11 0 R] >>",
            "<< /FT /Tx /Subtype /Widget /T (a) /P 4 0 R >>",
            "<< /Width 1 /Height 1 /Length 1 >>\nstream\nz\nendstream",
            "<< /Title (t) >>",
            "<< /F 15 0 R /I 13 0 R >>",
            "<< /T 14 0 R /N 15 0 R /V 15 0 R /P 4 0 R /R [0 0 1 1] >>",
            "<< /DPartRootNode 17 0 R >>",
            "<< /DParts [[]] /Start 3 0 R >>",
            "3",
        ]);
        let mut seeds = vec![all.clone()];
        // What's only a reference to nothing, a page's /Parent that's a page, a
        // tree whose /Kids and /Parent disagree, and a page no tree lists.
        let root =
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 9 9] /Resources << >> >>";
        let shared = "<< /Type /Page /Parent 2 0 R /X 5 0 R >>";
        seeds.push(file(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            root,
            "<< /Type /Page /Parent 2 0 R >>",
            shared,
            "99 0 R",
        ]));
        seeds.push(file(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            root,
            "<< /Type /Page /Parent 2 0 R >>",
            shared,
            "6 0 R",
            "null",
        ]));
        let own = "/MediaBox [0 0 9 9] /Resources << >>";
        let plain = "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>";
        let (one, other) = (
            format!("<< /Type /Page /Parent 2 0 R {own} >>"),
            format!("<< /Type /Page /Parent 3 0 R {own} >>"),
        );
        seeds.push(file(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            plain,
            &one,
            &other,
        ]));
        seeds.push(file(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            plain,
            "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R] /Count 1 /MediaBox [0 0 1 1] >>",
            "<< /Type /Pages /Parent 2 0 R /Kids [6 0 R] /Count 1 /MediaBox [0 0 2 2] >>",
            "<< /Type /Page /Parent 4 0 R /Resources << >> >>",
            "<< /Type /Page /Parent 3 0 R /Resources << >> >>",
        ]));
        seeds.push(file(&[
            "<< /Type /Catalog /Pages 2 0 R /Foo 4 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 9 9] /Resources << >> >>",
            "<< /Type /Page /Parent 2 0 R >>",
            "<< /Type /Page /Parent 2 0 R >>",
        ]));
        for (kind, pack) in [
            (pdf_structure::XrefKind::Table, false),
            (pdf_structure::XrefKind::Stream, true),
        ] {
            let mut options = crate::family::Options::DEFAULT;
            options.kind = kind;
            options.pack = pack;
            seeds.push(crate::family::linearize_bytes(&all, &[], &options).unwrap());
        }
        // Encrypted, by each of the handler's revisions, by the files qpdf
        // encrypted, and as it lays them out.
        let keys = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/files");
        for key in ["rc4-40", "rc4-128", "aes-128", "aes-256-r5", "aes-256"] {
            let Ok(key) = std::fs::read(keys.join(format!("qpdf-{key}.pdf"))) else {
                continue;
            };
            let mut encrypted = encrypted_as(&key, &all);
            seeds.push(key);
            let mut options = crate::family::Options::DEFAULT;
            options.kind = pdf_structure::XrefKind::Stream;
            options.pack = true;
            let laid_out = crate::family::linearize_bytes(&encrypted, &[], &options);
            seeds.push(std::mem::take(&mut encrypted));
            seeds.push(laid_out.unwrap());
        }
        seeds
    }

    /// `plain`'s objects as a whole file under `key`'s encryption, its
    /// dictionary at a number past them.
    #[cfg(feature = "family")]
    fn encrypted_as(key: &[u8], plain: &[u8]) -> Vec<u8> {
        use pdf_structure::read::{self, Reading};
        use pdf_structure::write::{self, Encryptor};

        let options = &read::Options::DEFAULT;
        let key = Reading::open(key, options).unwrap();
        let encryptor = Encryptor::of(&key).unwrap().unwrap();
        let reading = Reading::open(plain, options).unwrap();
        let doc = reading.to_document().unwrap();
        let mut written = write::Options::DEFAULT;
        written.encrypt_at = Some(ObjectId::new(100, 0));
        let mut out = Vec::new();
        write::document(&mut out, &doc, Some(&encryptor), Some(&reading), &written).unwrap();
        out
    }

    /// The lopdf adapter's body, on random edits: a good share of them written,
    /// so that what's held is what's been run.
    #[cfg(all(feature = "family", feature = "lopdf"))]
    #[test]
    fn what_lopdf_s_adapter_writes_of_any_document_holds() {
        let mut random = generator(0x0F1E_2D3C_4B5A_6978);
        assert!(lopdf_document(&[]));
        let mut written = 0;
        for round in 0..6000 {
            let edits = 1 + round % 6;
            let data: Vec<u8> = (0..4 * edits).map(|_| (random() >> 11) as u8).collect();
            written += usize::from(lopdf_document(&data));
        }
        assert!(written > 300, "{written} of 6,000 were written");
    }

    /// The family adapter's body, on files, whole and damaged.
    #[cfg(feature = "family")]
    #[test]
    fn what_is_written_reads_back_the_same() {
        let mut random = generator(0x1234_5678_9ABC_DEF1);
        for seed in file_seeds() {
            over(&seed);
            for _ in 0..100 {
                let mut hit = seed.clone();
                for _ in 0..1 + random() % 4 {
                    let at = (random() % hit.len() as u64) as usize;
                    hit[at] = (random() % 256) as u8;
                }
                over(&hit);
            }
            for cut in (0..seed.len()).step_by(61) {
                over(&seed[..cut]);
            }
        }
    }

    /// Writes [`seeds`] and [`table_seeds`] to the folder
    /// `PDF_LINEARIZE_FUZZ_SEEDS` names, for `just fuzz linearize` to begin
    /// from.
    #[test]
    #[ignore = "writes files for the fuzzer"]
    fn the_seeds_are_written() {
        let folder = std::env::var("PDF_LINEARIZE_FUZZ_SEEDS").unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        let folder = std::path::Path::new(&folder);
        for (at, seed) in seeds().iter().enumerate() {
            std::fs::write(folder.join(format!("graph-{at}")), seed).unwrap();
        }
        for (at, seed) in table_seeds().iter().enumerate() {
            std::fs::write(folder.join(format!("tables-{at}")), seed).unwrap();
        }
        #[cfg(feature = "family")]
        for (at, seed) in file_seeds().iter().enumerate() {
            std::fs::write(folder.join(format!("file-{at}")), seed).unwrap();
        }
    }

    /// The fuzz body, on the seeds and on random inputs of each shape.
    #[test]
    fn fuzz_bodies_run() {
        for seed in seeds() {
            graphs(&seed);
        }
        let mut random = generator(0x2545_F491_4F6C_DD1D);
        let mut planned = 0;
        for round in 0..400 {
            let len = 1 + (random() % 3000) as usize;
            let most = [4, 8, 16, 64, 255][round % 5];
            let data = input(len, most, &mut random);
            graphs(&data);
            let described = described(&data);
            planned += usize::from(described.refusal().is_none());
        }
        // So that the promises above are what's been run, not the refusals.
        assert!(planned > 100, "{planned} of 400 were planned");

        let mut read = 0;
        for seed in table_seeds() {
            tables(&seed);
            read += 1;
            // Damaged, every so often: cut off, and its fields flipped.
            for cut in [20, 60, seed.len() / 2, seed.len() - 1] {
                tables(&seed[..cut.min(seed.len())]);
            }
            for at in (16..seed.len()).step_by(7) {
                let mut flipped = seed.clone();
                flipped[at] ^= 0x41;
                tables(&flipped);
            }
        }
        assert!(read > 10, "{read} seeds of tables");
        for round in 0..2000 {
            let len = (random() % 400) as usize;
            let most = [2, 8, 64, 255][round % 4];
            tables(&input(len, most, &mut random));
        }
    }

    /// Any facts, made up and spoiled, on the seeds and on random inputs.
    #[test]
    fn any_facts_are_judged_alike_whatever_the_sink() {
        for seed in seeds() {
            facts(&seed);
        }
        let mut random = generator(0xDEAD_BEEF_1234_5678);
        for round in 0..3000 {
            let len = (random() % 2000) as usize;
            let most = [4, 16, 64, 255][round % 4];
            facts(&input(len, most, &mut random));
        }
        facts(&[]);
        for byte in 0..=255 {
            facts(&[byte]);
            facts(&[byte; 40]);
            facts(&[byte; 400]);
        }
    }

    /// An empty input is a hint stream too, and short ones and flat ones.
    #[test]
    fn any_bytes_are_a_hint_stream() {
        tables(&[]);
        for byte in 0..=255 {
            tables(&[byte]);
            tables(&[byte; 40]);
            tables(&[byte; 400]);
        }
    }

    /// An empty input describes a graph too, and short ones.
    #[test]
    fn any_bytes_describe_a_graph() {
        graphs(&[]);
        for byte in 0..=255 {
            graphs(&[byte]);
            graphs(&[byte, byte, byte, byte]);
        }
    }
}
