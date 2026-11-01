//! What the fuzz target checks, which unit tests check too: the promises a plan
//! makes, on any graph, each held against a plain walk of it. It's compiled only
//! for tests and under cargo-fuzz's `--cfg fuzzing`.

use std::collections::{BTreeMap, BTreeSet};

use crate::plan::{Error, Group, Kind, Page, Part, Plan, Use, What, plan};
use crate::{Graph, Id, Limits, MEMBERS};

/// The most objects an input describes, past what two object streams hold.
const OBJECTS: usize = 600;

/// The most pages it describes.
const PAGES: usize = 32;

/// The most objects of a graph whose caps are each tried at their edge.
const SMALL: usize = 128;

/// An object as an input describes it.
#[derive(Debug, Clone, Default)]
struct Object {
    refs: Vec<Id>,
    packable: bool,
    late: bool,
}

/// A graph as an input describes it, what's held against the plan of it.
#[derive(Debug, Clone)]
struct Described {
    kind: Kind,
    catalog: Id,
    /// The objects given, in order, each given more than once as it's listed.
    given: Vec<(Id, Object)>,
    pages: Vec<(Id, Option<Id>)>,
    first: u32,
    tree_nodes: Vec<Id>,
    open: Vec<Id>,
    groups: Vec<(Group, Vec<Id>)>,
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
/// can't by a small chance, its numbers spread by a step so they aren't its order.
fn described(data: &[u8]) -> Described {
    let mut bytes = Bytes(data);
    let flags = bytes.byte();
    let count = 2 + bytes.below(OBJECTS - 1);
    let step = 1 + bytes.below(5) as u32;
    let id_of = |at: usize| (1 + at as u32 * step, 0);
    let everything_packable = flags & 4 != 0;
    let kind = match flags & 1 != 0 {
        true => Kind::Stream,
        false => Kind::Table,
    };
    let mut given: Vec<(Id, Object)> = Vec::with_capacity(count);
    for at in 0..count {
        let (mut id, mut object) = (id_of(at), Object::default());
        if bytes.chance(64) {
            // The number before it at another generation, or given again.
            id = id_of(at.saturating_sub(1));
            id.1 = bytes.below(2) as u16;
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
    for at in 0..page_count {
        let page = match bytes.chance(128) {
            true => pick(&mut bytes),
            false => id_of(start + at),
        };
        let thumb = bytes.chance(4).then(|| pick(&mut bytes));
        pages.push((page, thumb));
    }
    let first = match bytes.chance(4) {
        true => bytes.below(page_count + usize::from(bytes.0.len() % 16 == 1)) as u32,
        false => 0,
    };
    let list = |bytes: &mut Bytes, most| {
        let count = bytes.below(most);
        (0..count).map(|_| pick(bytes)).collect::<Vec<Id>>()
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
        for &(id, thumb) in &self.pages {
            let mut page = Page::new(id);
            page.thumb = thumb;
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
    /// it fails. A cap can't come up on an input's graph under the default ones.
    fn refusal(&self) -> Option<Error> {
        if self.pages.is_empty() {
            return Some(Error::NoPages);
        }
        let mut seen = BTreeSet::new();
        for (id, _) in &self.given {
            if !seen.insert(*id) {
                let mut ids: Vec<Id> = self.given.iter().map(|(id, _)| *id).collect();
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
    objects: BTreeMap<Id, &'d Object>,
    /// What no walk goes into.
    stops: BTreeSet<Id>,
    /// What a walk stops at but the page walked from.
    pages: BTreeSet<Id>,
    open: Vec<Id>,
}

impl<'d> Plain<'d> {
    fn of(described: &'d Described) -> Plain<'d> {
        let objects: BTreeMap<Id, &Object> = (described.given.iter())
            .map(|(id, object)| (*id, object))
            .collect();
        let pages: BTreeSet<Id> = described.pages.iter().map(|&(id, _)| id).collect();
        let open = (described.open.iter()).filter(|id| !pages.contains(id));
        let open: Vec<Id> = open.copied().collect();
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
    fn reach(&self, roots: &[Id], page: Option<Id>) -> BTreeSet<Id> {
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
fn writes_by_object(plan: &Plan) -> BTreeMap<Id, usize> {
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
    let page_ids: Vec<Id> = described.pages.iter().map(|&(id, _)| id).collect();
    let first_id = page_ids[first];

    // Each object once, and nothing else: an id named nowhere has no number.
    assert_eq!(by_object.len(), plain.objects.len());
    assert!(plain.objects.keys().all(|id| by_object.contains_key(id)));
    assert_eq!(plan.number((u32::MAX, 0)), None);

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
        assert_eq!(number.1, 0);
        match plan.writes()[at].what {
            What::Object(_) => assert_eq!(number.0, plan.writes()[at].number),
            What::Container(c) => {
                let found = plan.containers()[c].members.iter().find(|m| m.0 == *id);
                assert_eq!(number.0, found.unwrap().1);
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
    assert_eq!(plan.first_page_number(), plan.number(first_id).unwrap().0);
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
    let reaches: Vec<BTreeSet<Id>> = (page_ids.iter())
        .map(|&page| plain.reach(&[page], Some(page)))
        .collect();
    let walk_of = |page: usize| match page.cmp(&first) {
        std::cmp::Ordering::Less => page as u32 + 1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => page as u32,
    };
    let outlines: Vec<Id> = (described.groups.iter())
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
        let reached: BTreeSet<Id> = (described.groups.iter())
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

    // Entries tile part 6, then part 8 write by write; in part 6 what a later page
    // reaches has one of its own and nothing else is alone beside another that
    // could join it.
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
    let list_of = |reach: &BTreeSet<Id>| {
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
        let roots: Vec<Id> = (described.groups.iter())
            .filter(|(g, _)| *g == group)
            .flat_map(|(_, roots)| roots.iter().copied())
            .collect();
        let lists = matches!(
            group,
            Group::Form | Group::Structure | Group::Renditions | Group::EmbeddedFile
        );
        if lists {
            let list = list_of(&plain.reach(&roots, None));
            assert_eq!(plan.group_shared(group), list, "{group:?}");
            count += list.len();
        } else {
            assert_eq!(plan.group_shared(group), []);
        }
    }
    assert_eq!(plan.shared_entries(), count);

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
/// [`plan`](crate::plan()) held against plain walks of it, the same twice.
pub fn graphs(data: &[u8]) {
    let described = described(data);
    let graph = described.graph();
    match plan(&graph, &Limits::DEFAULT) {
        Ok(planned) => {
            assert_eq!(described.refusal(), None, "{described:?}");
            holds(&described, &graph, &planned);
            assert_eq!(plan(&graph, &Limits::DEFAULT).as_ref(), Ok(&planned));
        }
        Err(refused) => assert_eq!(described.refusal(), Some(refused), "{described:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A generator, as the tests have no crate for one.
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

    /// Writes [`seeds`] to the folder `PDF_LINEARIZE_FUZZ_SEEDS` names, for
    /// `just fuzz linearize` to begin from.
    #[test]
    #[ignore = "writes files for the fuzzer"]
    fn the_seeds_are_written() {
        let folder = std::env::var("PDF_LINEARIZE_FUZZ_SEEDS").unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        for (at, seed) in seeds().iter().enumerate() {
            let path = std::path::Path::new(&folder).join(format!("graph-{at}"));
            std::fs::write(path, seed).unwrap();
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
