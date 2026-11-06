use std::collections::{HashMap, HashSet};

use super::*;

/// `numbers` as ids at generation 0.
fn ids<const N: usize>(numbers: [u32; N]) -> [ObjectId; N] {
    numbers.map(|n| ObjectId::new(n, 0))
}

/// Three pages, each with its content, under one tree node:
///
/// - 3, the first, reaches 11, which it shares with 4, and what 11 reaches, 13;
/// - 4 and 5 share 12, and what it reaches, 14;
/// - 40 is reached from nothing but the catalog, which no walk goes into.
fn three_pages(kind: Kind) -> Graph {
    let mut graph = Graph::new(ObjectId::new(1, 0), kind);
    graph.object(ObjectId::new(1, 0), ids([2, 40]));
    graph.object(ObjectId::new(2, 0), ids([3, 4, 5]));
    graph.object(ObjectId::new(3, 0), ids([2, 11, 10]));
    graph.object(ObjectId::new(4, 0), ids([2, 11, 12, 20]));
    graph.object(ObjectId::new(5, 0), ids([2, 12, 30]));
    for n in [10, 20, 30, 13, 14, 40] {
        graph.object(ObjectId::new(n, 0), []);
    }
    graph.object(ObjectId::new(11, 0), ids([13]));
    graph.object(ObjectId::new(12, 0), ids([14]));
    for n in [3, 4, 5] {
        graph.page(Page::new(ObjectId::new(n, 0)));
    }
    graph.tree_node(ObjectId::new(2, 0));
    graph
}

fn planned(graph: &Graph) -> Plan {
    plan(graph, &Limits::DEFAULT).unwrap()
}

/// What's written, in order, an object by its number in the graph.
fn order(plan: &Plan, part: Part) -> Vec<u32> {
    plan.writes()[plan.part_writes(part)]
        .iter()
        .map(|write| match write.what {
            What::Object(id) => id.number,
            What::Container(_) => panic!("{write:?} is a container"),
        })
        .collect()
}

/// `id`'s write.
fn write_of(plan: &Plan, id: ObjectId) -> Write {
    let in_container = |write: &&Write| match write.what {
        What::Object(of) => of == id,
        What::Container(at) => plan.containers()[at].members.iter().any(|m| m.0 == id),
    };
    *plan.writes().iter().find(in_container).unwrap()
}

/// The entry of the shared object table `id` is in.
fn entry_of(plan: &Plan, id: ObjectId) -> u32 {
    let write = write_of(plan, id);
    let at = plan.writes().iter().position(|w| *w == write).unwrap();
    let entry = plan.entries().iter().position(|e| e.contains(&at));
    entry.unwrap() as u32
}

/// The parts, each in F.3's order: the first page's object first, its walk's
/// order, then each other page by number, what later pages share by number, the
/// tree and what's left.
#[test]
fn objects_go_in_annex_f_parts() {
    let plan = planned(&three_pages(Kind::Table));
    assert_eq!(order(&plan, Part::Open), [1]);
    assert_eq!(order(&plan, Part::FirstPage), [3, 11, 13, 10]);
    assert_eq!(order(&plan, Part::Pages), [4, 20, 5, 30]);
    assert_eq!(order(&plan, Part::Shared), [12, 14]);
    assert_eq!(order(&plan, Part::Other), [2, 40]);
    assert_eq!(plan.page_writes(0), plan.part_writes(Part::FirstPage));
    assert_eq!(plan.page_writes(1), 5..7);
    assert_eq!(plan.page_writes(2), 7..9);
    assert_eq!((plan.page_count(), plan.first_page()), (3, 0));
}

/// The second group from 1, then the dictionary, the first group, and the hint
/// stream last; the first page's object is `/O`.
#[test]
fn numbers_run_from_the_second_group_to_the_hint_stream() {
    let plan = planned(&three_pages(Kind::Table));
    for (at, n) in (1..).zip([4, 20, 5, 30, 12, 14, 2, 40]) {
        assert_eq!(
            plan.number(ObjectId::new(n, 0)),
            Some(ObjectId::new(at, 0)),
            "object {n}"
        );
    }
    assert_eq!(plan.dictionary_number(), 9);
    for (at, n) in (10..).zip([1, 3, 11, 13, 10]) {
        assert_eq!(
            plan.number(ObjectId::new(n, 0)),
            Some(ObjectId::new(at, 0)),
            "object {n}"
        );
    }
    assert_eq!((plan.first_page_number(), plan.hint_number()), (11, 15));
    assert_eq!(plan.size(), 16);
    assert_eq!(plan.first_xref_number(), None);
    assert_eq!(plan.main_xref_number(), None);
    assert_eq!(plan.kind(), Kind::Table);
}

/// Under a stream, each section's stream takes a number where F.3.1 and F.3.6
/// have it, and each group's compressed members the highest of it, the hint
/// stream above even those of the first group.
#[test]
fn compressed_numbers_are_the_highest_of_each_group_and_the_hint_last() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Stream);
    graph.object(ObjectId::new(1, 0), ids([2]));
    graph.object(ObjectId::new(2, 0), ids([3, 4]));
    graph.object(ObjectId::new(3, 0), ids([10, 11]));
    graph.object(ObjectId::new(4, 0), ids([20, 21]));
    graph.object(ObjectId::new(10, 0), []);
    graph.object(ObjectId::new(11, 0), []).packable(true);
    graph.object(ObjectId::new(20, 0), []);
    graph.object(ObjectId::new(21, 0), []).packable(true);
    graph.page(Page::new(ObjectId::new(3, 0)));
    graph.page(Page::new(ObjectId::new(4, 0)));
    graph.tree_node(ObjectId::new(2, 0));
    let plan = planned(&graph);
    // 4, 21's container, 20, 2.
    assert_eq!(plan.number(ObjectId::new(4, 0)), Some(ObjectId::new(1, 0)));
    assert_eq!(plan.main_xref_number(), Some(5));
    assert_eq!(plan.number(ObjectId::new(21, 0)), Some(ObjectId::new(6, 0)));
    assert_eq!(plan.dictionary_number(), 7);
    assert_eq!(plan.first_xref_number(), Some(8));
    assert_eq!(plan.number(ObjectId::new(1, 0)), Some(ObjectId::new(9, 0)));
    assert_eq!(plan.number(ObjectId::new(3, 0)), Some(ObjectId::new(10, 0)));
    assert_eq!(
        plan.number(ObjectId::new(11, 0)),
        Some(ObjectId::new(13, 0))
    );
    assert_eq!(plan.hint_number(), 14);
    assert_eq!(plan.size(), 15);
    let container = plan
        .containers()
        .iter()
        .find(|c| c.members[0].0 == ObjectId::new(11, 0));
    assert_eq!(container.unwrap().members, [(ObjectId::new(11, 0), 13)]);
    assert_eq!(container.unwrap().number, 12);
}

/// F.3.7 and F.4.3: what the first page reaches is with the first page, though
/// a later page turns it `Shared`, and has an entry of its own.
#[test]
fn an_object_the_first_page_shares_stays_with_the_first_page() {
    let plan = planned(&three_pages(Kind::Table));
    for n in [11, 13] {
        assert_eq!(plan.used(ObjectId::new(n, 0)), Some(Use::Shared));
        assert_eq!(write_of(&plan, ObjectId::new(n, 0)).part, Part::FirstPage);
    }
    assert!(!order(&plan, Part::Shared).contains(&11));
}

/// A run of writes no later page reaches is one entry, each write one reaches
/// another, the first page's first; part 8's are one each.
#[test]
fn the_first_pages_entries_are_runs_and_what_later_pages_share() {
    let plan = planned(&three_pages(Kind::Table));
    let first = plan.part_writes(Part::FirstPage).start;
    let shared = plan.part_writes(Part::Shared).start;
    assert_eq!(
        plan.entries(),
        [
            first..first + 1,
            first + 1..first + 2,
            first + 2..first + 3,
            first + 3..first + 4,
            shared..shared + 1,
            shared + 1..shared + 2,
        ]
    );
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Table);
    graph.object(ObjectId::new(1, 0), []);
    graph.object(ObjectId::new(3, 0), ids([5, 6, 7, 8]));
    graph.object(ObjectId::new(4, 0), ids([7]));
    for n in [5, 6, 7, 8] {
        graph.object(ObjectId::new(n, 0), []);
    }
    graph.page(Page::new(ObjectId::new(3, 0)));
    graph.page(Page::new(ObjectId::new(4, 0)));
    let plan = planned(&graph);
    let first = plan.part_writes(Part::FirstPage).start;
    assert_eq!(order(&plan, Part::FirstPage), [3, 5, 6, 7, 8]);
    assert_eq!(
        plan.entries(),
        [first..first + 3, first + 3..first + 4, first + 4..first + 5]
    );
}

/// Each later page lists every entry it reaches, those under a shared object
/// too, and a third page's as a second's, though the walk that gave uses stops
/// at what's shared by then; the first page lists none.
#[test]
fn each_later_page_lists_all_a_shared_object_reaches() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Table);
    graph.object(ObjectId::new(1, 0), []);
    for n in [3, 4, 5, 6] {
        graph.object(ObjectId::new(n, 0), ids([10]));
        graph.page(Page::new(ObjectId::new(n, 0)));
    }
    graph.object(ObjectId::new(10, 0), ids([11]));
    graph.object(ObjectId::new(11, 0), ids([12]));
    graph.object(ObjectId::new(12, 0), []);
    let plan = planned(&graph);
    assert_eq!(plan.shared(0), []);
    for page in 1..4 {
        assert_eq!(plan.shared(page), [1, 2, 3], "page {page}");
    }
    assert_eq!(plan.shared_entries(), 9);
    // Each page and its reference, each of three objects with its own.
    assert_eq!(plan.shared_visits(), 3 * (2 + 2 + 2 + 1));

    let plan = planned(&three_pages(Kind::Table));
    let entry = |n| {
        entry_of(
            &plan,
            ObjectId {
                number: n,
                generation: 0,
            },
        )
    };
    assert_eq!(plan.shared(1), [entry(11), entry(13), entry(12), entry(14)]);
    assert_eq!(plan.shared(2), [entry(12), entry(14)]);
}

/// Under another first page: it's part 6, and `Use::Page(0)`; the others are in
/// order, the first skipped (F.3.8), its entry in the tables first, which is
/// the one with no list.
#[test]
fn another_first_page_goes_first_and_the_rest_in_order() {
    let mut graph = three_pages(Kind::Table);
    graph.first_page(1);
    let plan = planned(&graph);
    assert_eq!(order(&plan, Part::FirstPage), [4, 11, 13, 12, 14, 20]);
    assert_eq!(order(&plan, Part::Pages), [3, 10, 5, 30]);
    assert_eq!(order(&plan, Part::Shared), [] as [u32; 0]);
    assert_eq!(plan.first_page(), 1);
    assert_eq!(
        plan.first_page_number(),
        plan.number(ObjectId::new(4, 0)).unwrap().number
    );
    assert_eq!(plan.used(ObjectId::new(4, 0)), Some(Use::Page(0)));
    assert_eq!(plan.used(ObjectId::new(3, 0)), Some(Use::Page(1)));
    assert_eq!(plan.used(ObjectId::new(5, 0)), Some(Use::Page(2)));
    assert_eq!(plan.shared(1), []);
    assert_eq!(plan.page_writes(0), 7..9);
    assert_eq!(plan.page_writes(2), 9..11);
    let entry = |n| {
        entry_of(
            &plan,
            ObjectId {
                number: n,
                generation: 0,
            },
        )
    };
    assert_eq!(plan.shared(0), [entry(11), entry(13)]);
    assert_eq!(plan.shared(2), [entry(12), entry(14)]);
}

/// What's met by a walk from a page: not through another page, a tree node, the
/// catalog or what's at open, so what they reach is theirs.
#[test]
fn a_page_is_walked_through_no_node_page_or_open_object() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Table);
    graph.object(ObjectId::new(1, 0), ids([50]));
    graph.object(ObjectId::new(2, 0), ids([3, 4, 51]));
    graph.object(ObjectId::new(3, 0), ids([1, 2, 4, 6, 10]));
    graph.object(ObjectId::new(4, 0), ids([20]));
    graph.object(ObjectId::new(6, 0), ids([52]));
    for n in [10, 20, 50, 51, 52] {
        graph.object(ObjectId::new(n, 0), []);
    }
    graph.page(Page::new(ObjectId::new(3, 0)));
    graph.page(Page::new(ObjectId::new(4, 0)));
    graph.tree_node(ObjectId::new(2, 0));
    graph.open(ObjectId::new(6, 0));
    let plan = planned(&graph);
    assert_eq!(order(&plan, Part::Open), [1, 6]);
    assert_eq!(order(&plan, Part::FirstPage), [3, 10]);
    assert_eq!(order(&plan, Part::Pages), [4, 20]);
    assert_eq!(order(&plan, Part::Other), [2, 50, 51, 52]);
    for n in [1, 2, 6, 50, 51, 52] {
        assert_eq!(
            plan.used(ObjectId::new(n, 0)),
            Some(Use::None),
            "object {n}"
        );
    }
    assert_eq!(plan.shared(1), []);
}

/// A page given as a node, or at open, stays where a page goes, as a page's own
/// walk needs it first in its section.
#[test]
fn a_page_given_as_a_node_or_at_open_is_a_page() {
    let mut graph = three_pages(Kind::Table);
    graph.tree_node(ObjectId::new(3, 0));
    graph.open(ObjectId::new(4, 0));
    let plan = planned(&graph);
    assert_eq!(order(&plan, Part::Open), [1]);
    assert_eq!(order(&plan, Part::FirstPage)[0], 3);
    assert_eq!(order(&plan, Part::Pages)[0], 4);
}

/// What's `late` goes after the rest of its page's, the page first even so, and
/// the outlines follow them in part 6, as a table of their own.
#[test]
fn late_objects_and_outlines_go_last_in_their_sections() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Table);
    graph.object(ObjectId::new(1, 0), []);
    graph
        .object(ObjectId::new(3, 0), ids([10, 11, 12]))
        .late(true);
    graph.object(ObjectId::new(4, 0), ids([20, 21, 22]));
    graph.object(ObjectId::new(10, 0), []).late(true);
    graph.object(ObjectId::new(11, 0), []);
    graph.object(ObjectId::new(12, 0), []);
    graph.object(ObjectId::new(20, 0), []).late(true);
    graph.object(ObjectId::new(21, 0), []);
    graph.object(ObjectId::new(22, 0), []);
    graph.object(ObjectId::new(30, 0), ids([31, 3]));
    graph.object(ObjectId::new(31, 0), []);
    graph.page(Page::new(ObjectId::new(3, 0)));
    graph.page(Page::new(ObjectId::new(4, 0)));
    graph.group(Group::Outlines, ids([30]));
    let mut first = graph.clone();
    first.outlines_first(true);
    let plan = planned(&first);
    assert_eq!(order(&plan, Part::FirstPage), [3, 11, 12, 10, 30, 31]);
    assert_eq!(order(&plan, Part::Pages), [4, 21, 22, 20]);
    assert_eq!(plan.page_writes(0), 1..5);
    assert_eq!(plan.group_writes(Group::Outlines), Some(5..7));
    let plan = planned(&graph);
    assert_eq!(order(&plan, Part::FirstPage), [3, 11, 12, 10]);
    assert_eq!(order(&plan, Part::Other), [30, 31]);
}

/// Under `outlines_first` an object the outlines reach is in part 6 even if a
/// later page reaches it too, and it's an entry that page lists.
#[test]
fn outlines_read_first_take_what_a_later_page_reaches() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Table);
    graph.object(ObjectId::new(1, 0), []);
    graph.object(ObjectId::new(3, 0), []);
    graph.object(ObjectId::new(4, 0), ids([31]));
    graph.object(ObjectId::new(30, 0), ids([31]));
    graph.object(ObjectId::new(31, 0), []);
    graph.page(Page::new(ObjectId::new(3, 0)));
    graph.page(Page::new(ObjectId::new(4, 0)));
    graph.group(Group::Outlines, ids([30]));
    graph.outlines_first(true);
    let plan = planned(&graph);
    assert_eq!(order(&plan, Part::FirstPage), [3, 30, 31]);
    assert_eq!(plan.entries().len(), 2);
    assert_eq!(plan.shared(1), [1]);
}

/// Part 9: the tree, thumbnails by page with page 0's first whichever is the
/// first page, what they share after them, each group in its order, what's in
/// none of them last by number. What's placed stays: the thumbnail's own
/// reference to a page's object, a group's to the tree or to another group's.
/// Page 1's thumbnail reaches page 0's, so that one is what thumbnails share
/// and page 0 has none alone.
#[test]
fn part_nine_is_the_tree_thumbnails_groups_and_the_rest() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Table);
    graph.object(ObjectId::new(1, 0), []);
    graph.object(ObjectId::new(2, 0), ids([3, 4]));
    graph.object(ObjectId::new(3, 0), ids([2, 10]));
    graph.object(ObjectId::new(4, 0), ids([2]));
    graph.object(ObjectId::new(10, 0), []);
    graph.object(ObjectId::new(50, 0), ids([10, 51]));
    graph.object(ObjectId::new(51, 0), []);
    graph.object(ObjectId::new(60, 0), ids([2, 61, 3]));
    graph.object(ObjectId::new(61, 0), []);
    graph.object(ObjectId::new(70, 0), ids([61, 71]));
    graph.object(ObjectId::new(71, 0), []);
    graph.object(ObjectId::new(80, 0), []);
    graph.object(ObjectId::new(90, 0), []);
    graph.object(ObjectId::new(91, 0), []);
    let mut first = Page::new(ObjectId::new(3, 0));
    first.thumb = Some(ObjectId::new(51, 0));
    graph.page(first);
    let mut second = Page::new(ObjectId::new(4, 0));
    second.thumb = Some(ObjectId::new(50, 0));
    graph.page(second);
    graph.first_page(1);
    graph.tree_node(ObjectId::new(2, 0));
    graph.group(Group::Form, ids([70]));
    graph.group(Group::Dests, ids([60]));
    graph.group(Group::Other, ids([80]));
    graph.group(Group::Renditions, []);
    let plan = planned(&graph);
    assert_eq!(
        order(&plan, Part::Other),
        [2, 50, 51, 60, 61, 70, 71, 80, 90, 91]
    );
    let start = plan.part_writes(Part::Other).start;
    assert!(plan.thumb_writes(0).is_empty());
    assert_eq!(plan.thumb_writes(1), start + 1..start + 2);
    assert_eq!(plan.thumb_shared_writes(), start + 2..start + 3);
    assert_eq!(plan.group_writes(Group::Dests), Some(start + 3..start + 5));
    assert_eq!(plan.group_writes(Group::Form), Some(start + 5..start + 7));
    assert_eq!(plan.group_writes(Group::Other), Some(start + 7..start + 10));
    let empty = plan.group_writes(Group::Renditions).unwrap();
    assert!(empty.is_empty());
    assert_eq!(plan.group_writes(Group::Info), None);
}

/// Thumbnails in page order from page 0 whichever page is the first, each with
/// what it alone reaches, each sealed in its own object streams so its entry's
/// its own, and what several reach after the last of them.
#[test]
fn thumbnails_keep_their_own_and_share_the_rest() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Stream);
    graph.object(ObjectId::new(1, 0), []);
    graph.object(ObjectId::new(3, 0), []);
    graph.object(ObjectId::new(4, 0), []);
    graph.object(ObjectId::new(5, 0), []);
    graph.object(ObjectId::new(50, 0), ids([70, 60]));
    graph.object(ObjectId::new(51, 0), ids([71, 60]));
    graph.object(ObjectId::new(52, 0), []);
    graph.object(ObjectId::new(60, 0), ids([61])).packable(true);
    graph.object(ObjectId::new(61, 0), []).packable(true);
    graph.object(ObjectId::new(70, 0), []).packable(true);
    graph.object(ObjectId::new(71, 0), []).packable(true);
    let pages = [(3, Some(50)), (4, None), (5, Some(51))];
    for (id, thumb) in pages {
        let mut page = Page::new(ObjectId::new(id, 0));
        page.thumb = thumb.map(|thumb| ObjectId::new(thumb, 0));
        graph.page(page);
    }
    // A thumbnail that's a page has no writes of its own as one.
    let mut extra = Page::new(ObjectId::new(52, 0));
    extra.thumb = Some(ObjectId::new(3, 0));
    graph.page(extra);
    graph.first_page(2);
    let plan = planned(&graph);
    let writes = |range: std::ops::Range<usize>| -> Vec<Vec<u32>> {
        plan.writes()[range]
            .iter()
            .map(|write| match write.what {
                What::Object(id) => vec![id.number],
                What::Container(at) => plan.containers()[at]
                    .members
                    .iter()
                    .map(|m| m.0.number)
                    .collect(),
            })
            .collect()
    };
    assert_eq!(writes(plan.thumb_writes(0)), [vec![50], vec![70]]);
    assert_eq!(writes(plan.thumb_writes(1)), Vec::<Vec<u32>>::new());
    assert_eq!(writes(plan.thumb_writes(2)), [vec![51], vec![71]]);
    assert_eq!(writes(plan.thumb_writes(3)), Vec::<Vec<u32>>::new());
    assert_eq!(writes(plan.thumb_shared_writes()), [vec![60, 61]]);
    // Each thumbnail's own stream, then, and what they share in one.
    assert_eq!(plan.containers().len(), 3);
}

/// A page's content items are its own streams': from the first in its section
/// to the last, by where they are, whichever order it lists them.
#[test]
fn a_pages_contents_are_a_stretch_of_its_section() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Table);
    graph.object(ObjectId::new(1, 0), []);
    graph.object(ObjectId::new(3, 0), ids([10, 11]));
    graph.object(ObjectId::new(4, 0), ids([12, 13, 14]));
    graph.object(ObjectId::new(5, 0), ids([14]));
    for at in [10, 11, 12, 13, 14] {
        graph.object(ObjectId::new(at, 0), []);
    }
    let contents = [(3, vec![11, 10]), (4, vec![13, 12, 14, 99]), (5, vec![14])];
    for (id, contents) in contents {
        let mut page = Page::new(ObjectId::new(id, 0));
        page.contents = contents
            .into_iter()
            .map(|at| ObjectId::new(at, 0))
            .collect();
        graph.page(page);
    }
    let plan = planned(&graph);
    // By where they are: the page's walk, its references' order.
    let first = plan.page_writes(0);
    assert_eq!(plan.page_contents(0), first.start + 1..first.start + 3);
    let second = plan.page_writes(1);
    assert_eq!(plan.page_contents(1), second.start + 1..second.start + 3);
    // Page 2's only stream's page 1's too, so it's in part 8, and its own has
    // none.
    assert_eq!(plan.page_contents(2), 0..0);
}

/// Each root of the embedded file group is a group of its own, its objects
/// sealed apart from the others', with a list of what it refers to.
#[test]
fn each_embedded_file_is_a_group_of_its_own() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Stream);
    graph.object(ObjectId::new(1, 0), []);
    graph.object(ObjectId::new(3, 0), ids([10]));
    graph.object(ObjectId::new(4, 0), ids([11]));
    graph.object(ObjectId::new(5, 0), ids([11]));
    graph.object(ObjectId::new(10, 0), []);
    graph.object(ObjectId::new(11, 0), []);
    graph.object(ObjectId::new(20, 0), ids([21, 10]));
    graph.object(ObjectId::new(21, 0), []).packable(true);
    graph.object(ObjectId::new(30, 0), ids([31, 11]));
    graph.object(ObjectId::new(31, 0), []).packable(true);
    graph.object(ObjectId::new(40, 0), ids([20]));
    for id in [3, 4, 5] {
        graph.page(Page::new(ObjectId::new(id, 0)));
    }
    graph.group(Group::EmbeddedFile, ids([20, 30]));
    graph.group(Group::EmbeddedFile, ids([40, 99]));
    graph.group(Group::Other, []);
    let plan = planned(&graph);
    let whole = plan.group_writes(Group::EmbeddedFile).unwrap();
    let [first, second, third] = plan.files() else {
        panic!("{:?}", plan.files());
    };
    assert_eq!(
        first.number,
        plan.number(ObjectId::new(20, 0)).unwrap().number
    );
    assert_eq!(
        second.number,
        plan.number(ObjectId::new(30, 0)).unwrap().number
    );
    assert_eq!(
        third.number,
        plan.number(ObjectId::new(40, 0)).unwrap().number
    );
    assert_eq!(first.writes, whole.start..whole.start + 2);
    assert_eq!(second.writes, whole.start + 2..whole.start + 4);
    assert_eq!(third.writes, whole.start + 4..whole.start + 5);
    assert_eq!(whole.end, third.writes.end);
    // Each a stream of its own, then, whatever's packable.
    assert_eq!(plan.containers().len(), 2);
    let entry = |id| {
        let number = plan.number(id).unwrap().number;
        let write = plan
            .writes()
            .iter()
            .position(|w| w.number == number)
            .unwrap();
        plan.entries()
            .iter()
            .position(|e| e.contains(&write))
            .unwrap() as u32
    };
    assert_eq!(plan.file_shared(0), [entry(ObjectId::new(10, 0))]);
    assert_eq!(plan.file_shared(1), [entry(ObjectId::new(11, 0))]);
    assert_eq!(
        plan.file_shared(2),
        [entry(ObjectId::new(10, 0))],
        "by what it reaches"
    );
    assert_eq!(plan.group_shared(Group::EmbeddedFile), []);
    assert_eq!(
        plan.shared_entries(),
        3 + plan.shared(1).len() + plan.shared(2).len()
    );
}

/// A table that lists what its group refers to: through what's a page's own,
/// which has no entry, to what's shared, a member named by its stream; and the
/// first page's stretches, whose entries a group can name. A table that doesn't
/// lists nothing.
#[test]
fn a_group_lists_the_shared_entries_it_reaches() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Stream);
    graph.object(ObjectId::new(1, 0), []);
    graph.object(ObjectId::new(3, 0), ids([9]));
    graph.object(ObjectId::new(4, 0), ids([20, 22]));
    graph.object(ObjectId::new(5, 0), ids([22]));
    graph.object(ObjectId::new(9, 0), []);
    graph.object(ObjectId::new(20, 0), ids([21]));
    graph.object(ObjectId::new(21, 0), ids([3, 22]));
    graph.object(ObjectId::new(22, 0), []).packable(true);
    graph.object(ObjectId::new(23, 0), []).packable(true);
    graph.object(ObjectId::new(70, 0), ids([20, 9]));
    for n in [3, 4, 5] {
        graph.page(Page::new(ObjectId::new(n, 0)));
    }
    graph.group(Group::Form, ids([70]));
    graph.group(Group::Dests, ids([70]));
    let plan = planned(&graph);
    assert_eq!(plan.group_shared(Group::Dests), []);
    assert_eq!(plan.group_shared(Group::Info), []);
    assert_eq!(
        plan.group_shared(Group::Form),
        [
            entry_of(&plan, ObjectId::new(3, 0)),
            entry_of(&plan, ObjectId::new(22, 0))
        ],
        "9, in the first page's stretch, and 22, through 20 and 21"
    );
}

/// `group` is called once per region, so a stream holds one region's alone:
/// what a viewer has at open, the first page's, each table's, and a part 9's
/// rest. Of a later page's, a stream of its own stays in its section.
#[test]
fn an_object_stream_holds_one_regions_objects() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Stream);
    graph.object(ObjectId::new(1, 0), []);
    graph.object(ObjectId::new(2, 0), []).packable(true);
    graph.object(ObjectId::new(3, 0), ids([10]));
    graph.object(ObjectId::new(4, 0), ids([20]));
    graph.object(ObjectId::new(10, 0), []).packable(true);
    graph.object(ObjectId::new(20, 0), []).packable(true);
    graph.object(ObjectId::new(30, 0), []).packable(true);
    graph.object(ObjectId::new(40, 0), []).packable(true);
    graph.object(ObjectId::new(41, 0), []).packable(true);
    graph.object(ObjectId::new(50, 0), []).packable(true);
    graph.page(Page::new(ObjectId::new(3, 0)));
    graph.page(Page::new(ObjectId::new(4, 0)));
    graph.open(ObjectId::new(2, 0));
    graph.tree_node(ObjectId::new(50, 0));
    graph.group(Group::Info, ids([30]));
    graph.group(Group::Other, ids([40]));
    let plan = planned(&graph);
    let part_of = |n| {
        write_of(
            &plan,
            ObjectId {
                number: n,
                generation: 0,
            },
        )
        .part
    };
    assert_eq!(plan.containers().len(), 6);
    assert_eq!(part_of(2), Part::Open);
    assert_eq!(part_of(10), Part::FirstPage);
    assert_eq!(part_of(20), Part::Pages);
    assert_eq!(part_of(30), Part::Other);
    let write = |n| {
        write_of(
            &plan,
            ObjectId {
                number: n,
                generation: 0,
            },
        )
    };
    assert_ne!(write(30), write(40));
    assert_ne!(write(30), write(50));
    assert_ne!(write(40), write(50));
    assert_eq!(write(40), write(41), "Other and what no group reached");
    let page = plan.page_writes(1);
    assert_eq!(page.len(), 2);
    assert!(matches!(
        plan.writes()[page.start].what,
        What::Object(ObjectId {
            number: 4,
            generation: 0
        })
    ));
    assert_eq!(plan.entries().len(), 1, "the first page's");
    assert_eq!(plan.shared(1), []);
}

/// A stream with two pages' members is part 8's, whatever it holds, one entry
/// each of them lists; the one before, of one page's, stays in its section.
#[test]
fn an_object_stream_of_two_pages_is_a_shared_object() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Stream);
    graph.object(ObjectId::new(1, 0), []);
    graph.object(ObjectId::new(3, 0), []);
    let own: Vec<ObjectId> = (100..201).map(|n| ObjectId::new(n, 0)).collect();
    graph.object(ObjectId::new(4, 0), own.iter().copied());
    for &id in &own {
        graph.object(id, []).packable(true);
    }
    graph.object(ObjectId::new(5, 0), ids([300]));
    graph.object(ObjectId::new(300, 0), []).packable(true);
    for n in [3, 4, 5] {
        graph.page(Page::new(ObjectId::new(n, 0)));
    }
    let plan = planned(&graph);
    assert_eq!(plan.containers().len(), 2);
    assert_eq!(write_of(&plan, ObjectId::new(100, 0)).part, Part::Pages);
    assert_eq!(write_of(&plan, ObjectId::new(199, 0)).part, Part::Pages);
    assert_eq!(write_of(&plan, ObjectId::new(200, 0)).part, Part::Shared);
    assert_eq!(write_of(&plan, ObjectId::new(300, 0)).part, Part::Shared);
    assert_eq!(plan.part_writes(Part::Shared).len(), 1);
    let entry = entry_of(&plan, ObjectId::new(300, 0));
    assert_eq!(plan.shared(1), [entry]);
    assert_eq!(plan.shared(2), [entry]);
    assert_eq!(plan.page_writes(1).len(), 2, "its object and its stream");
    assert_eq!(plan.page_writes(2).len(), 1, "its object alone");
}

/// At most [`MEMBERS`] a stream, by number, a stream where its first member
/// would be, its members in order at the highest numbers.
#[test]
fn object_streams_fill_by_number_up_to_their_cap() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Stream);
    graph.object(ObjectId::new(1, 0), []);
    let own: Vec<ObjectId> = (100..331).rev().map(|n| ObjectId::new(n, 0)).collect();
    graph.object(ObjectId::new(3, 0), own.iter().copied());
    for &id in &own {
        graph.object(id, []).packable(true);
    }
    graph.page(Page::new(ObjectId::new(3, 0)));
    let plan = planned(&graph);
    assert_eq!(plan.containers().len(), 3);
    let sizes = plan.containers().iter().map(|c| c.members.len());
    // The walk goes into the highest number first, so its stream is first.
    assert_eq!(sizes.collect::<Vec<_>>(), [31, MEMBERS, MEMBERS]);
    assert_eq!(plan.containers()[0].members[0].0, ObjectId::new(300, 0));
    assert_eq!(plan.containers()[1].members[0].0, ObjectId::new(200, 0));
    assert_eq!(plan.containers()[2].members[0].0, ObjectId::new(100, 0));
    assert_eq!(plan.part_writes(Part::FirstPage).len(), 4);
}

/// The catalog and a page are never packed, whatever a caller says (F.3.1).
#[test]
fn the_catalog_and_pages_are_never_packed() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Stream);
    graph.object(ObjectId::new(1, 0), []).packable(true);
    graph.object(ObjectId::new(3, 0), ids([4])).packable(true);
    graph.object(ObjectId::new(4, 0), []).packable(true);
    graph.page(Page::new(ObjectId::new(3, 0)));
    let plan = planned(&graph);
    assert_eq!(plan.containers().len(), 1);
    assert_eq!(plan.containers()[0].members[0].0, ObjectId::new(4, 0));
    assert!(matches!(
        write_of(&plan, ObjectId::new(1, 0)).what,
        What::Object(ObjectId {
            number: 1,
            generation: 0
        })
    ));
    assert!(matches!(
        write_of(&plan, ObjectId::new(3, 0)).what,
        What::Object(ObjectId {
            number: 3,
            generation: 0
        })
    ));
}

/// A reference to no object counts for nothing, and has no number; an id with
/// another generation is written at 0, and two of one number are two objects.
#[test]
fn numbers_are_for_the_objects_given_at_generation_zero() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Table);
    graph.object(ObjectId::new(1, 0), []);
    graph.object(
        ObjectId::new(3, 0),
        [
            ObjectId::new(4, 2),
            ObjectId::new(4, 3),
            ObjectId::new(9, 0),
            ObjectId::new(4, 0),
        ],
    );
    graph.object(ObjectId::new(4, 2), []);
    graph.object(ObjectId::new(4, 3), []);
    graph.page(Page::new(ObjectId::new(3, 0)));
    graph.tree_node(ObjectId::new(88, 0));
    graph.open(ObjectId::new(89, 0));
    graph.group(Group::Info, ids([90]));
    let plan = planned(&graph);
    assert_eq!(plan.number(ObjectId::new(9, 0)), None);
    assert_eq!(plan.number(ObjectId::new(4, 0)), None);
    assert_eq!(plan.used(ObjectId::new(4, 0)), None);
    assert_eq!(plan.used(ObjectId::new(4, 3)), Some(Use::Page(0)));
    assert_eq!(plan.number(ObjectId::new(4, 2)).unwrap().generation, 0);
    assert_ne!(
        plan.number(ObjectId::new(4, 2)),
        plan.number(ObjectId::new(4, 3))
    );
    assert_eq!(plan.writes().len(), 4);
    assert_eq!(plan.size(), 7);
}

/// A page's section and its entries for a document of one page: all of it in
/// part 6, one entry, nothing to list.
#[test]
fn one_page_is_all_part_six() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Table);
    graph.object(ObjectId::new(1, 0), []);
    graph.object(ObjectId::new(3, 0), ids([4]));
    graph.object(ObjectId::new(4, 0), []);
    graph.page(Page::new(ObjectId::new(3, 0)));
    let plan = planned(&graph);
    assert_eq!(plan.size(), 6);
    assert_eq!(plan.first_page_number(), 3);
    assert_eq!(plan.entries().len(), 1);
    assert_eq!(plan.entries()[0], 1..3);
    assert_eq!(plan.shared(0), []);
    assert_eq!(plan.shared_visits(), 0);
}

/// Each refusal, its cause alone.
#[test]
fn a_graph_that_cant_be_laid_out_is_refused() {
    let refused = |graph: &Graph| plan(graph, &Limits::DEFAULT).unwrap_err();
    let base = || {
        let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Table);
        graph.object(ObjectId::new(1, 0), []);
        graph.object(ObjectId::new(3, 0), []);
        graph
    };
    assert_eq!(refused(&base()), Error::NoPages);

    let mut graph = base();
    graph.page(Page::new(ObjectId::new(3, 0)));
    graph.page(Page::new(ObjectId::new(3, 0)));
    assert_eq!(refused(&graph), Error::PageRepeated(ObjectId::new(3, 0)));

    let mut graph = base();
    graph.page(Page::new(ObjectId::new(4, 0)));
    assert_eq!(refused(&graph), Error::NoPage(ObjectId::new(4, 0)));

    let mut graph = Graph::new(ObjectId::new(2, 0), Kind::Table);
    graph.object(ObjectId::new(3, 0), []);
    graph.page(Page::new(ObjectId::new(3, 0)));
    assert_eq!(refused(&graph), Error::NoCatalog(ObjectId::new(2, 0)));

    let mut graph = base();
    graph.page(Page::new(ObjectId::new(1, 0)));
    assert_eq!(refused(&graph), Error::CatalogIsPage(ObjectId::new(1, 0)));

    let mut graph = base();
    graph.object(ObjectId::new(3, 0), []);
    graph.page(Page::new(ObjectId::new(3, 0)));
    assert_eq!(refused(&graph), Error::ObjectRepeated(ObjectId::new(3, 0)));

    let mut graph = base();
    graph.page(Page::new(ObjectId::new(3, 0)));
    graph.first_page(1);
    assert_eq!(refused(&graph), Error::FirstPagePast { index: 1, pages: 1 });

    let mut graph = base();
    graph.object(ObjectId::new(4, 0), []).packable(true);
    graph.page(Page::new(ObjectId::new(3, 0)));
    assert_eq!(refused(&graph), Error::PackedTable(ObjectId::new(4, 0)));
    // The same object, under what can list it.
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Stream);
    graph.object(ObjectId::new(1, 0), []);
    graph.object(ObjectId::new(3, 0), []);
    graph.object(ObjectId::new(4, 0), []).packable(true);
    graph.page(Page::new(ObjectId::new(3, 0)));
    assert!(plan(&graph, &Limits::DEFAULT).is_ok());
}

/// Each message is whole, saying what it's about.
#[test]
fn an_error_says_what_it_is_about() {
    let said = Error::PackedTable(ObjectId::new(4, 7)).to_string();
    assert_eq!(
        said,
        "object 4 7 may be packed, which cross-reference tables can't list"
    );
    assert_eq!(
        Error::FirstPagePast { index: 2, pages: 1 }.to_string(),
        "the first page is page 2, of 1"
    );
}

/// The caps at their edges: every number counts, a stream's and the layout's
/// own among them; the lists' entries and visits count what they cost.
#[test]
fn each_cap_holds_at_its_edge() {
    let mut limits = Limits::DEFAULT;
    let graph = three_pages(Kind::Table);
    let plan_of = |limits: &Limits| plan(&graph, limits);
    let whole = plan_of(&limits).unwrap();
    assert_eq!(limits, Limits::default());

    limits.pages = 3;
    assert_eq!(plan_of(&limits).as_ref(), Ok(&whole));
    limits.pages = 2;
    let refused = Error::TooManyPages { pages: 3, limit: 2 };
    assert_eq!(plan_of(&limits).unwrap_err(), refused);

    let mut limits = Limits::DEFAULT;
    limits.objects = whole.size() as usize - 1;
    assert_eq!(plan_of(&limits).as_ref(), Ok(&whole));
    limits.objects -= 1;
    let refused = Error::TooManyObjects {
        numbers: 15,
        limit: 14,
    };
    assert_eq!(plan_of(&limits).unwrap_err(), refused);
    limits.objects = 3;
    let refused = Error::TooManyObjects {
        numbers: 13,
        limit: 3,
    };
    assert_eq!(plan_of(&limits).unwrap_err(), refused);

    let mut limits = Limits::DEFAULT;
    limits.shared_entries = whole.shared_entries();
    assert_eq!(plan_of(&limits).as_ref(), Ok(&whole));
    limits.shared_entries -= 1;
    assert!(matches!(
        plan_of(&limits),
        Err(Error::TooManyHints { entries: 6, .. })
    ));

    let mut limits = Limits::DEFAULT;
    limits.shared_visits = whole.shared_visits();
    assert_eq!(plan_of(&limits).as_ref(), Ok(&whole));
    limits.shared_visits -= 1;
    assert!(matches!(plan_of(&limits), Err(Error::TooManyHints { .. })));
}

/// A graph where every page reaches every object costs the product of them, so
/// it's refused at the cap rather than run: here a lower one than the default,
/// 2^28, as a test binary in debug takes seconds to get there.
#[test]
fn a_graph_that_would_take_hours_is_refused() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Table);
    graph.object(ObjectId::new(1, 0), []);
    let objects: Vec<ObjectId> = (1_000_000..1_100_000)
        .map(|n| ObjectId::new(n, 0))
        .collect();
    for n in 2..100_002 {
        graph.object(ObjectId::new(n, 0), objects.iter().copied().take(1));
        graph.page(Page::new(ObjectId::new(n, 0)));
    }
    graph.object(objects[0], objects.iter().copied().skip(1));
    for &id in &objects[1..] {
        graph.object(id, []);
    }
    let mut limits = Limits::DEFAULT;
    limits.shared_visits = 1 << 20;
    let Err(refused) = plan(&graph, &limits) else {
        panic!("the lists' cost is about 10^10 visits");
    };
    // No further past the cap than the object that passed it.
    assert!(matches!(refused, Error::TooManyHints { visits, .. } if visits <= (1 << 20) + 100_000));
}

/// A plan is plain data, read from any thread.
#[test]
fn a_plan_is_sent_and_shared() {
    fn holds<T: Send + Sync + Clone>() {}
    holds::<Plan>();
    holds::<Graph>();
    holds::<Error>();
}

/// What a writer with no plan is told of which pages use what, by ids, is what
/// a plan has: a page's own alone, what two share, the walk stopping at the
/// tree and at another page.
#[test]
fn page_uses_by_id_is_what_a_plan_says() {
    let mut graph = Graph::new(ObjectId::new(1, 0), Kind::Table);
    graph.object(ObjectId::new(1, 0), [ObjectId::new(2, 0)]);
    graph.object(
        ObjectId::new(2, 0),
        [ObjectId::new(3, 0), ObjectId::new(4, 0)],
    );
    graph.object(
        ObjectId::new(3, 0),
        [
            ObjectId::new(2, 0),
            ObjectId::new(5, 0),
            ObjectId::new(7, 0),
            ObjectId::new(4, 0),
        ],
    );
    graph.object(
        ObjectId::new(4, 0),
        [
            ObjectId::new(2, 0),
            ObjectId::new(6, 0),
            ObjectId::new(7, 0),
        ],
    );
    graph.object(ObjectId::new(5, 0), []);
    graph.object(ObjectId::new(6, 0), []);
    graph.object(ObjectId::new(7, 0), [ObjectId::new(8, 1)]);
    graph.object(ObjectId::new(8, 1), []);
    graph.object(ObjectId::new(9, 0), [ObjectId::new(7, 0)]);
    graph.page(Page::new(ObjectId::new(3, 0)));
    graph.page(Page::new(ObjectId::new(4, 0)));
    graph.tree_node(ObjectId::new(2, 0));
    let planned = plan(&graph, &Limits::DEFAULT).unwrap();

    let objects: HashMap<ObjectId, Vec<ObjectId>> = [
        (
            ObjectId::new(3, 0),
            vec![
                ObjectId::new(2, 0),
                ObjectId::new(5, 0),
                ObjectId::new(7, 0),
                ObjectId::new(4, 0),
            ],
        ),
        (
            ObjectId::new(4, 0),
            vec![
                ObjectId::new(2, 0),
                ObjectId::new(6, 0),
                ObjectId::new(7, 0),
            ],
        ),
        (ObjectId::new(7, 0), vec![ObjectId::new(8, 1)]),
    ]
    .into_iter()
    .collect();
    let stop: HashSet<ObjectId> = [
        ObjectId::new(1, 0),
        ObjectId::new(2, 0),
        ObjectId::new(3, 0),
        ObjectId::new(4, 0),
    ]
    .into_iter()
    .collect();
    let found = page_uses(
        &[ObjectId::new(3, 0), ObjectId::new(4, 0)],
        &stop,
        |id, to| {
            // What it stops at it doesn't ask after: that's what the stop is for.
            assert!(
                id != ObjectId::new(1, 0) && id != ObjectId::new(2, 0),
                "asked of {id:?}"
            );
            for &reference in objects.get(&id).into_iter().flatten() {
                to(reference);
            }
        },
    );
    for id in [
        ObjectId::new(3, 0),
        ObjectId::new(4, 0),
        ObjectId::new(5, 0),
        ObjectId::new(6, 0),
        ObjectId::new(7, 0),
        ObjectId::new(8, 1),
    ] {
        assert_eq!(found.get(&id).copied(), planned.used(id), "{id:?}");
    }
    assert_eq!(found.get(&ObjectId::new(5, 0)), Some(&Use::Page(0)));
    assert_eq!(found.get(&ObjectId::new(7, 0)), Some(&Use::Shared));
    assert_eq!(found.get(&ObjectId::new(8, 1)), Some(&Use::Shared));
    for id in [
        ObjectId::new(1, 0),
        ObjectId::new(2, 0),
        ObjectId::new(9, 0),
    ] {
        assert_eq!(found.get(&id), None, "{id:?}");
    }
}
