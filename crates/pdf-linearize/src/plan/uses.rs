//! Which pages use each object: the walk a plan starts from.

use std::collections::{HashMap, HashSet};

use super::group::Use;
use pdf_objects::ObjectId;

/// What [`walk`] found.
#[derive(Debug)]
pub(super) struct Walked {
    /// Each object's use, by its index.
    pub uses: Vec<Use>,
    /// Each object as a walk went into it, walk after walk: an object is here
    /// at most twice.
    pub entered: Vec<u32>,
    /// Where each walk starts in `entered`.
    pub starts: Vec<usize>,
}

impl Walked {
    /// What walk `i` went into, in order.
    pub fn entered(&self, i: usize) -> &[u32] {
        let end = self.starts.get(i + 1).copied();
        &self.entered[self.starts[i]..end.unwrap_or(self.entered.len())]
    }
}

/// Which of `pages` use each object, an index below `stop.len()`, `refs` giving
/// the references an object holds: what each page reaches, walked from the
/// pages in order, never into an object `stop` marks but the page walked from,
/// so another page's link, an annotation's `/P` or a page's `/Parent` brings in
/// nothing of another page's. An object no page reaches is [`Use::None`].
///
/// The walk from page `i` goes into an object it meets unused, which
/// becomes `Page(i)`, or one another page alone uses, which becomes
/// `Shared`; it stops at one that's `Page(i)` or `Shared` already. An
/// object is gone into only as its use changes, which is twice at most,
/// so however many pages share what an object reaches, that's walked
/// twice: by the first page that reaches it, and by the second, which
/// turns all of it shared. The walk keeps its own stack, so a deep chain
/// can't overflow the thread's. An object's references are gone into in
/// their order, each one's own before the next.
pub(super) fn walk<'g>(pages: &[u32], stop: &[bool], refs: impl Fn(u32) -> &'g [u32]) -> Walked {
    let mut uses = vec![Use::None; stop.len()];
    let mut entered = Vec::with_capacity(stop.len());
    let mut starts = Vec::with_capacity(pages.len());
    let mut stack = Vec::new();
    for (i, &page) in (0..).zip(pages) {
        starts.push(entered.len());
        meet(&mut uses, &mut stack, page, i);
        while let Some(id) = stack.pop() {
            entered.push(id);
            // Last first, as the stack gives the last back first.
            for &r in refs(id).iter().rev() {
                if r == page || !stop[r as usize] {
                    meet(&mut uses, &mut stack, r, i);
                }
            }
        }
    }
    Walked {
        uses,
        entered,
        starts,
    }
}

/// Object `id`, met on page `i`'s walk, put on `stack` to go into when
/// it's unused or another page's alone, and its use changed.
fn meet(uses: &mut [Use], stack: &mut Vec<u32>, id: u32, i: u32) {
    let used = &mut uses[id as usize];
    *used = match *used {
        Use::None => Use::Page(i),
        Use::Page(j) if j != i => Use::Shared,
        Use::Page(_) | Use::Shared => return,
    };
    stack.push(id);
}

/// Which of `pages` use each object, by id, for a writer that groups objects by
/// page ([`group`](super::group())) and has no plan to ask
/// ([`Plan::used`](super::Plan::used)): `refs` tells each reference an object
/// holds, and the walk from page `i` goes into what it meets but what `stop`
/// holds other than the page walked from, so another page's link or a page's
/// `/Parent` brings in nothing of another's. An object no page reaches isn't in
/// the map. An object is gone into twice at most, as [`Plan`](super::Plan) has
/// it, on a stack of its own, however many pages share what it reaches.
#[must_use]
pub fn page_uses(
    pages: &[ObjectId],
    stop: &HashSet<ObjectId>,
    refs: impl Fn(ObjectId, &mut dyn FnMut(ObjectId)),
) -> HashMap<ObjectId, Use> {
    let mut index: HashMap<ObjectId, u32> = HashMap::with_capacity(pages.len());
    let mut ids: Vec<ObjectId> = Vec::with_capacity(pages.len());
    let mut lists: Vec<Vec<u32>> = Vec::with_capacity(pages.len());
    let mut indexed = |id: ObjectId, ids: &mut Vec<ObjectId>| {
        *index.entry(id).or_insert_with(|| {
            ids.push(id);
            ids.len() as u32 - 1
        })
    };
    let walks: Vec<u32> = pages.iter().map(|&id| indexed(id, &mut ids)).collect();
    let is_page: HashSet<ObjectId> = pages.iter().copied().collect();
    while lists.len() < ids.len() {
        let id = ids[lists.len()];
        let mut list = Vec::new();
        if is_page.contains(&id) || !stop.contains(&id) {
            refs(id, &mut |to| list.push(indexed(to, &mut ids)));
        }
        lists.push(list);
    }
    let stops: Vec<bool> = ids.iter().map(|id| stop.contains(id)).collect();
    let walked = walk(&walks, &stops, |at| &lists[at as usize]);
    let found = ids.iter().copied().zip(walked.uses);
    found.filter(|(_, used)| *used != Use::None).collect()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    use super::super::group::group;
    use super::*;

    /// An object's references, as a list per number.
    fn references(objects: &[(u32, Vec<u32>)]) -> Vec<Vec<u32>> {
        let highest = objects
            .iter()
            .flat_map(|(n, refs)| [*n].into_iter().chain(refs.iter().copied()));
        let mut all = vec![Vec::new(); highest.max().unwrap_or(0) as usize + 1];
        for (n, refs) in objects {
            all[*n as usize].clone_from(refs);
        }
        all
    }

    /// `stop` as [`walk`] takes it: those numbers marked, of `len`.
    fn marks(stop: &HashSet<u32>, len: usize) -> Vec<bool> {
        (0..len as u32).map(|n| stop.contains(&n)).collect()
    }

    /// 1,000 pages share a chain 1,000 objects deep: it's walked twice,
    /// by the first page and the second, and each page once, so the work
    /// doesn't grow with the pages that share it. The tree's root, which
    /// every page names as its `/Parent`, isn't walked.
    #[test]
    fn page_uses_descends_each_object_at_most_twice() {
        let root = 5000;
        let pages: Vec<u32> = (1..=1000).collect();
        let mut objects = Vec::new();
        for (i, &page) in pages.iter().enumerate() {
            let next = pages.get(i + 1).copied().unwrap_or(pages[0]);
            objects.push((page, vec![root, 1001, next]));
        }
        for n in 1001..=2000 {
            objects.push((n, vec![n + 1, 1]));
        }
        objects.push((root, pages.clone()));
        let references = references(&objects);
        let stop: HashSet<u32> = pages.iter().copied().chain([root]).collect();
        let stop = marks(&stop, references.len());
        let walked: RefCell<HashMap<u32, usize>> = RefCell::default();
        let found = walk(&pages, &stop, |id| {
            *walked.borrow_mut().entry(id).or_default() += 1;
            &references[id as usize]
        });
        let uses = found.uses;
        let walked = walked.into_inner();
        for (i, &page) in (0..).zip(&pages) {
            assert_eq!(uses[page as usize], Use::Page(i));
            assert_eq!(walked[&page], 1, "page {i}");
        }
        for n in 1001..=2000 {
            assert_eq!(uses[n], Use::Shared, "{n}");
            assert_eq!(walked[&(n as u32)], 2, "{n}");
        }
        assert_eq!(uses[2001], Use::Shared, "named, if it holds nothing");
        assert_eq!(walked[&2001], 2);
        assert!(uses[root as usize] == Use::None && !walked.contains_key(&root));
        assert_eq!(uses.iter().filter(|&&u| u != Use::None).count(), 2001);
    }

    /// Each object is used by the one page that reaches it, by several, or
    /// by none, never through another page or the tree; the grouping then
    /// puts page 1's in a container of their own, and the rest in page
    /// order, the shared, then what no page reaches.
    #[test]
    fn page_uses_sorts_objects_by_their_pages() {
        let page = |contents, resources, annots: &[u32]| {
            let mut refs = vec![1, contents, resources];
            refs.extend_from_slice(annots);
            refs
        };
        let objects: Vec<(u32, Vec<u32>)> = vec![
            (50, vec![1]),
            (1, vec![2, 3, 4]),
            (2, page(10, 11, &[12])),
            (3, page(20, 11, &[21])),
            (4, page(30, 31, &[])),
            (10, vec![]),
            (11, vec![13]),
            (12, vec![2, 3]),
            (13, vec![11]),
            (20, vec![]),
            (21, vec![3, 1, 22]),
            (22, vec![21]),
            (30, vec![]),
            (31, vec![32]),
            (32, vec![31, 30]),
            (40, vec![10]),
        ];
        let references = references(&objects);
        let pages = [2, 3, 4];
        let stop = marks(&HashSet::from([2, 3, 4, 1]), references.len());
        let found = walk(&pages, &stop, |id| &references[id as usize]);
        let uses = found.uses;
        let got: Vec<(u32, Use)> = (0..)
            .zip(&uses)
            .filter(|&(_, &used)| used != Use::None)
            .map(|(n, &used)| (n, used))
            .collect();
        let want = [
            (2, Use::Page(0)),
            (3, Use::Page(1)),
            (4, Use::Page(2)),
            (10, Use::Page(0)),
            (11, Use::Shared),
            (12, Use::Page(0)),
            (13, Use::Shared),
            (20, Use::Page(1)),
            (21, Use::Page(1)),
            (22, Use::Page(1)),
            (30, Use::Page(2)),
            (31, Use::Page(2)),
            (32, Use::Page(2)),
        ];
        assert_eq!(got, want);
        let all = objects.iter().map(|&(n, _)| (n, uses[n as usize]));
        assert_eq!(
            group(all),
            [
                vec![2, 10, 12],
                vec![3, 20, 21, 22, 4, 30, 31, 32, 11, 13, 1, 40, 50]
            ]
        );
    }

    /// The first object a walk goes into is its page, then each reference and
    /// what it reaches, in order, before the next.
    #[test]
    fn a_walk_goes_into_references_in_their_order() {
        let references = references(&[
            (1, vec![2, 5, 7]),
            (2, vec![3, 4]),
            (5, vec![6, 3]),
            (7, vec![2]),
        ]);
        let stop = vec![false; references.len()];
        let found = walk(&[1], &stop, |id| &references[id as usize]);
        assert_eq!(found.entered(0), [1, 2, 3, 4, 5, 6, 7]);
    }
}
