//! pdf-semantics timed on inputs made here, so that a change to its page
//! walk, its tree walks, its XMP reader or its dates can be judged before
//! and after it, by `cargo bench -p pdf-semantics`:
//!
//! - `pages`: `pages` by each set of rules (`iso`, `pdfium`), with `()`,
//!   which looks for no fault (`none`), and with a `Vec` that keeps them
//!   (`faults`), of a tree of 7,000 pages balanced in nodes of at most 32,
//!   every page at one depth under three nodes (`balanced`); of 64 pages
//!   under a chain of 64 nodes, one inside the next (`chain`); and of a root
//!   listing one page object 7,000 times (`repeated`). The root has the
//!   `/MediaBox` and `/Resources` every page inherits.
//! - `pages/parents`: each page's `parents`, of the balanced tree's 7,000
//!   pages and the chain's 64, walked by `next` (`balanced/next`,
//!   `chain/next`) and counted (`balanced/count`, `chain/count`).
//! - `trees`: `tree::names` of a name tree of 100,000 entries, written flat
//!   by `tree::write_names` (`flat`) and in nodes of at most 32 entries or
//!   kids (`nodes`), each walked to its end with `()` and with a `Vec`.
//! - `xmp`: `xmp::read` of a 4 KiB packet holding every claim as attributes
//!   (`attributes-4k`) and as elements (`elements-4k`) among the properties
//!   a writer puts beside them; of a 1 MiB packet of other properties with
//!   one claim at its end (`large-1m`); of a packet binding 123 namespaces
//!   on one element, every element named in one of them
//!   (`namespaces-123`); of a claim holding an element 250 levels deep
//!   (`deep-elements-250`); and of 250 claim elements, one inside the next
//!   (`deep-claims-250`).
//! - `dates`: `Date::read` and `Date::write` of a date of each form: a year
//!   (`year`), a day (`day`), a local time (`local`), UT (`utc`), an offset
//!   ahead of UT (`ahead`) and an offset behind it with PDF 1.7's last
//!   apostrophe (`behind`). Nothing here needs the `clock` feature.
//!
//! Throughput is in pages, entries, bytes of a packet, or dates. Each input
//! is held to what it gives before it's timed: a page tree to every page in
//! order, each listing's count, where each page's `/MediaBox` came from and
//! its faults, none but each listing again of the repeated page; each
//! page's parents to its chain; a name tree to every entry in order, whole,
//! with no fault; a packet to its claims, with no fault; a date to what it
//! writes and what it read past. The objects are made here into a map,
//! which is the resolver; nothing is read from disk, and the inputs are the
//! same on every run.

use std::collections::BTreeMap;
use std::hint::black_box;

use criterion::{Criterion, Throughput};
use pdf_semantics::date::{Date, Quirk};
use pdf_semantics::pages::{Origin, Pages, pages};
use pdf_semantics::pdf_objects::fault::{Place, Sink};
use pdf_semantics::pdf_objects::pdf_tokenizer::Rules;
use pdf_semantics::pdf_objects::{Dict, Object, ObjectId, Unreadable};
use pdf_semantics::tree::{names, write_names};
use pdf_semantics::xmp::{Claims, read};
use pdf_semantics::{Fault, Limits};

/// Each departure a walk or a packet tells, kept.
type Faults = Vec<(Place, Fault)>;

/// The objects of a file, by number: the resolver.
type Objects = BTreeMap<ObjectId, Object>;

/// The most kids a node of a balanced tree holds, as a writer's default.
const FANOUT: usize = 32;

/// How many pages the balanced tree and the repeated one list.
const PAGES: u32 = 7_000;

/// How many nodes the chain has, one inside the next, and how many pages
/// its last holds.
const CHAIN: u32 = 64;

/// A reference to `id`.
fn reference(id: ObjectId) -> Object {
    Object::from(id)
}

/// The array of `ids`, each a reference.
fn references(ids: &[ObjectId]) -> Object {
    Object::from(ids.iter().copied().map(reference).collect::<Vec<_>>())
}

/// The numbers of `numbers` as an array.
fn numbers(numbers: [i64; 4]) -> Object {
    Object::from(numbers.map(Object::from).to_vec())
}

/// A page tree: its objects, its catalog, each place's page in order, and
/// each page's chain of nodes up to the root, nearest first.
struct Tree {
    objects: Objects,
    catalog: Dict,
    root: ObjectId,
    pages: Vec<ObjectId>,
    chains: Vec<Vec<ObjectId>>,
}

/// A node of a tree being made: its object, its kids, and the pages under
/// it.
struct Node {
    id: ObjectId,
    kids: Vec<ObjectId>,
    count: u32,
}

/// The tree of `nodes` under `root`, each kid's node by `parent`, over
/// `pages`, in order: each node `/Type /Pages` with its `/Parent` but the
/// root, its `/Kids` and its `/Count`; each page `/Type /Page` with its
/// `/Parent`; the root with the `/MediaBox` and `/Resources` each page
/// inherits.
fn tree(
    nodes: &[Node],
    root: ObjectId,
    parent: &BTreeMap<ObjectId, ObjectId>,
    pages: Vec<ObjectId>,
) -> Tree {
    let mut objects = Objects::new();
    for node in nodes {
        let mut dict = Dict::new();
        dict.set("Type", "Pages");
        if let Some(&up) = parent.get(&node.id) {
            dict.set("Parent", reference(up));
        }
        dict.set("Kids", references(&node.kids));
        dict.set("Count", node.count);
        if node.id == root {
            dict.set("MediaBox", numbers([0, 0, 612, 792]));
            let mut resources = Dict::new();
            resources.set("ProcSet", Object::from(vec![Object::from("PDF")]));
            dict.set("Resources", resources);
        }
        objects.insert(node.id, Object::from(dict));
    }
    for &page in &pages {
        let mut dict = Dict::new();
        dict.set("Type", "Page");
        dict.set("Parent", reference(parent[&page]));
        objects.insert(page, Object::from(dict));
    }
    let chain = |page: &ObjectId| {
        std::iter::successors(parent.get(page), |node| parent.get(node))
            .copied()
            .collect()
    };
    let chains = pages.iter().map(chain).collect();
    let mut catalog = Dict::new();
    catalog.set("Type", "Catalog");
    catalog.set("Pages", reference(root));
    Tree {
        objects,
        catalog,
        root,
        pages,
        chains,
    }
}

/// `count` pages, from object 1 on, in a tree whose nodes hold at most
/// `FANOUT` kids, spread evenly, with every page at one depth: each level
/// of nodes over the one below, until one node holds them all.
fn balanced(count: u32) -> Tree {
    let pages: Vec<ObjectId> = (1..=count).map(|n| ObjectId::new(n, 0)).collect();
    let mut next = count + 1;
    let (mut nodes, mut parent) = (Vec::new(), BTreeMap::new());
    let mut level: Vec<(ObjectId, u32)> = pages.iter().map(|&id| (id, 1)).collect();
    loop {
        let groups = level.len().div_ceil(FANOUT);
        let mut rest = &level[..];
        let mut upper = Vec::with_capacity(groups);
        for left in (1..=groups).rev() {
            let (kids, after) = rest.split_at(rest.len().div_ceil(left));
            let id = ObjectId::new(next, 0);
            next += 1;
            let count = kids.iter().map(|&(_, count)| count).sum();
            for &(kid, _) in kids {
                parent.insert(kid, id);
            }
            let kids = kids.iter().map(|&(kid, _)| kid).collect();
            nodes.push(Node { id, kids, count });
            upper.push((id, count));
            rest = after;
        }
        level = upper;
        if level.len() == 1 {
            break;
        }
    }
    tree(&nodes, level[0].0, &parent, pages)
}

/// `CHAIN` nodes, one inside the next, the root first, the last holding
/// `CHAIN` pages.
fn chain() -> Tree {
    let pages: Vec<ObjectId> = (1..=CHAIN).map(|n| ObjectId::new(n, 0)).collect();
    let ids: Vec<ObjectId> = (1..=CHAIN).map(|n| ObjectId::new(CHAIN + n, 0)).collect();
    let (root, last) = (ids[0], ids[ids.len() - 1]);
    let mut parent = BTreeMap::new();
    for &page in &pages {
        parent.insert(page, last);
    }
    for pair in ids.windows(2) {
        parent.insert(pair[1], pair[0]);
    }
    let mut nodes: Vec<Node> = (ids.windows(2))
        .map(|pair| Node {
            id: pair[0],
            kids: vec![pair[1]],
            count: CHAIN,
        })
        .collect();
    nodes.push(Node {
        id: last,
        kids: pages.clone(),
        count: CHAIN,
    });
    tree(&nodes, root, &parent, pages)
}

/// A root listing one page object `PAGES` times.
fn repeated() -> Tree {
    let (page, root) = (ObjectId::new(1, 0), ObjectId::new(2, 0));
    let parent = BTreeMap::from([(page, root)]);
    let kids = vec![page; PAGES as usize];
    let nodes = [Node {
        id: root,
        kids,
        count: PAGES,
    }];
    tree(&nodes, root, &parent, vec![page; PAGES as usize])
}

/// `tree`'s pages by `rules`, with `()` and with a `Vec`, held to each
/// place's page, how often it was listed before, where its `/MediaBox`
/// came from, and the faults: none, but each listing again of a page under
/// ISO's rules.
fn held_pages(name: &str, tree: &Tree, rules: Rules) {
    let resolve = |id: ObjectId| tree.objects.get(&id);
    let mut before = BTreeMap::new();
    let expected: Vec<(Option<ObjectId>, u32)> = (tree.pages.iter())
        .map(|&page| {
            let before = before.entry(page).or_insert(0);
            *before += 1;
            (Some(page), *before - 1)
        })
        .collect();
    let inherited = Origin::Ancestor(Some(tree.root));
    let held = |sink: &str, read: Result<Pages, _>| {
        let read = read.unwrap_or_else(|e| panic!("pages/{name}/{sink}: {e}"));
        let listed: Vec<(Option<ObjectId>, u32)> =
            read.iter().map(|page| (page.id, page.repeat)).collect();
        assert!(
            listed == expected,
            "pages/{name}/{sink}: each page, in order, and how often it was listed before"
        );
        assert_eq!(
            read.counted(),
            tree.pages.len(),
            "pages/{name}/{sink}: the count"
        );
        assert!(
            (read.iter()).all(|page| page.media_box().origin == inherited),
            "pages/{name}/{sink}: each /MediaBox from the root"
        );
    };
    held(
        "none",
        pages(resolve, &tree.catalog, rules, &Limits::DEFAULT, &mut ()),
    );
    let mut faults = Faults::new();
    let read = pages(resolve, &tree.catalog, rules, &Limits::DEFAULT, &mut faults);
    held("faults", read);
    // By ISO's rules each listing of a page after its first is told; by
    // PDFium's, only what doesn't read is.
    let again = match rules {
        Rules::Iso => tree.pages.len() - distinct(&tree.pages),
        _ => 0,
    };
    let told = (faults.iter())
        .filter(|(_, fault)| matches!(fault, Fault::PageTreeRepeated { .. }))
        .count();
    assert_eq!(told, again, "pages/{name}/faults: each listing again");
    assert_eq!(
        faults.len(),
        told,
        "pages/{name}/faults: none but listings again"
    );
}

/// How many objects `pages` names.
fn distinct(pages: &[ObjectId]) -> usize {
    let mut pages = pages.to_vec();
    pages.sort_unstable();
    pages.dedup();
    pages.len()
}

/// The page trees timed, by name, and how many nodes are over each page.
fn trees() -> [(&'static str, Tree, usize); 3] {
    [
        ("balanced", balanced(PAGES), 3),
        ("chain", chain(), CHAIN as usize),
        ("repeated", repeated(), 1),
    ]
}

fn pages_group(c: &mut Criterion) {
    let trees = trees();
    let mut group = c.benchmark_group("pages");
    for (name, tree, _) in &trees {
        let resolve = |id: ObjectId| tree.objects.get(&id);
        group.throughput(Throughput::Elements(tree.pages.len() as u64));
        for (rules, said) in [(Rules::Iso, "iso"), (Rules::Pdfium, "pdfium")] {
            held_pages(&format!("{name}/{said}"), tree, rules);
            let catalog = &tree.catalog;
            group.bench_function(format!("{name}/{said}/none"), |b| {
                b.iter(|| {
                    pages(
                        resolve,
                        black_box(catalog),
                        rules,
                        &Limits::DEFAULT,
                        &mut (),
                    )
                })
            });
            group.bench_function(format!("{name}/{said}/faults"), |b| {
                b.iter(|| {
                    let mut faults = Faults::new();
                    let read = pages(
                        resolve,
                        black_box(catalog),
                        rules,
                        &Limits::DEFAULT,
                        &mut faults,
                    );
                    (read, faults)
                })
            });
        }
    }
    group.finish();
}

/// The numbers of each page's parents, walked by `next`, added up.
fn by_next(table: &Pages) -> u64 {
    let mut sum = 0;
    for page in table {
        for id in page.parents() {
            sum += u64::from(id.number);
        }
    }
    sum
}

/// Each page's parents, counted, added up.
fn by_count(table: &Pages) -> usize {
    table.iter().map(|page| page.parents().count()).sum()
}

fn parents_group(c: &mut Criterion) {
    let trees = trees();
    let mut group = c.benchmark_group("pages/parents");
    for (name, tree, depth) in trees.iter().filter(|(name, ..)| *name != "repeated") {
        let resolve = |id: ObjectId| tree.objects.get(&id);
        let table = pages(
            resolve,
            &tree.catalog,
            Rules::Iso,
            &Limits::DEFAULT,
            &mut (),
        );
        let table = table.unwrap_or_else(|e| panic!("pages/parents/{name}: {e}"));
        let chains: Vec<Vec<ObjectId>> = (table.iter())
            .map(|page| page.parents().collect())
            .collect();
        assert!(
            chains == tree.chains,
            "pages/parents/{name}: each page's chain, nearest first"
        );
        assert!(
            chains.iter().all(|chain| chain.len() == *depth),
            "pages/parents/{name}: {depth} nodes over each page"
        );
        let sum: u64 = (tree.chains.iter().flatten())
            .map(|id| u64::from(id.number))
            .sum();
        assert_eq!(by_next(&table), sum, "pages/parents/{name}/next: the sum");
        let count: usize = tree.chains.iter().map(Vec::len).sum();
        assert_eq!(
            by_count(&table),
            count,
            "pages/parents/{name}/count: the count"
        );
        group.throughput(Throughput::Elements(tree.pages.len() as u64));
        group.bench_function(format!("{name}/next"), |b| {
            b.iter(|| by_next(black_box(&table)))
        });
        group.bench_function(format!("{name}/count"), |b| {
            b.iter(|| by_count(black_box(&table)))
        });
    }
    group.finish();
}

/// How many entries the name tree has.
const ENTRIES: u32 = 100_000;

/// The number of the object entry `at`'s value refers to, as a name tree of
/// destinations refers to each.
const VALUES: u32 = 1_000_000;

/// Entry `at`'s key: names that sort as they're numbered.
fn key(at: u32) -> Vec<u8> {
    format!("section-{at:06}").into_bytes()
}

/// A name tree of `ENTRIES` entries, each key's value a reference: its
/// objects and its root, written by `write_names` as one dictionary, the
/// root by reference.
fn flat() -> (Objects, Object) {
    let entries = (0..ENTRIES).map(|at| (key(at), Object::reference(VALUES + at)));
    let root = write_names(entries).unwrap_or_else(|e| panic!("trees/flat: {e}"));
    let id = ObjectId::new(1, 0);
    (Objects::from([(id, Object::from(root))]), reference(id))
}

/// The same entries in nodes: leaves of at most `FANOUT` entries, each
/// level of nodes over the one below of at most `FANOUT` kids, spread
/// evenly, every node but the root with its `/Limits`, the root by
/// reference.
fn in_nodes() -> (Objects, Object) {
    let mut objects = Objects::new();
    let mut next = 1;
    let mut add = |dict: Dict, objects: &mut Objects| {
        let id = ObjectId::new(next, 0);
        next += 1;
        objects.insert(id, Object::from(dict));
        id
    };
    let limits = |least: u32, greatest: u32| {
        let ends = [key(least), key(greatest)];
        Object::from(ends.map(Object::bytes).to_vec())
    };
    let all: Vec<u32> = (0..ENTRIES).collect();
    let mut level: Vec<(ObjectId, u32, u32)> = Vec::new();
    for leaf in all.chunks(FANOUT) {
        let mut entries = Vec::with_capacity(leaf.len() * 2);
        for &at in leaf {
            entries.push(Object::bytes(key(at)));
            entries.push(Object::reference(VALUES + at));
        }
        let (least, greatest) = (leaf[0], leaf[leaf.len() - 1]);
        let mut dict = Dict::new();
        dict.set("Names", entries);
        dict.set("Limits", limits(least, greatest));
        level.push((add(dict, &mut objects), least, greatest));
    }
    while level.len() > 1 {
        let groups = level.len().div_ceil(FANOUT);
        let mut rest = &level[..];
        let mut upper = Vec::with_capacity(groups);
        for left in (1..=groups).rev() {
            let (kids, after) = rest.split_at(rest.len().div_ceil(left));
            let (least, greatest) = (kids[0].1, kids[kids.len() - 1].2);
            let ids: Vec<ObjectId> = kids.iter().map(|&(id, ..)| id).collect();
            let mut dict = Dict::new();
            dict.set("Kids", references(&ids));
            if groups > 1 {
                dict.set("Limits", limits(least, greatest));
            }
            upper.push((add(dict, &mut objects), least, greatest));
            rest = after;
        }
        level = upper;
    }
    (objects, reference(level[0].0))
}

/// The name tree at `root` walked to its end with `sink`: how many entries
/// it gave, the sum of their values' numbers, whether it was whole, and the
/// sink.
fn walked<S: Sink<Fault>>(
    objects: &Objects,
    root: &Object,
    sink: S,
) -> Result<(u32, u64, bool, S), Unreadable> {
    let resolve = |id: ObjectId| objects.get(&id);
    let mut walk = names(resolve, root, &Limits::DEFAULT, sink);
    let (mut count, mut sum) = (0, 0);
    for entry in walk.by_ref() {
        let (_, value) = entry?;
        count += 1;
        sum += value.as_reference().map_or(0, |id| u64::from(id.number));
    }
    Ok((count, sum, walk.whole(), walk.into_sink()))
}

/// The name tree at `root` held to every entry, in order, whole, with no
/// fault.
fn held_tree(name: &str, objects: &Objects, root: &Object) {
    let resolve = |id: ObjectId| objects.get(&id);
    let walk = names(resolve, root, &Limits::DEFAULT, Faults::new());
    let entries: Vec<(Vec<u8>, Option<ObjectId>)> = (walk.map(|entry| {
        let (key, value) = entry.unwrap_or_else(|e| panic!("trees/{name}: {e}"));
        (key.to_vec(), value.as_reference())
    }))
    .collect();
    let expected: Vec<(Vec<u8>, Option<ObjectId>)> = (0..ENTRIES)
        .map(|at| (key(at), Some(ObjectId::new(VALUES + at, 0))))
        .collect();
    assert!(entries == expected, "trees/{name}: each entry, in order");
    let read = walked(objects, root, Faults::new());
    let (.., whole, faults) = read.unwrap_or_else(|e| panic!("trees/{name}: {e}"));
    assert!(whole, "trees/{name}: whole");
    assert_eq!(faults, [], "trees/{name}: no fault");
}

fn trees_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("trees");
    group.throughput(Throughput::Elements(u64::from(ENTRIES)));
    for (name, (objects, root)) in [("flat", flat()), ("nodes", in_nodes())] {
        held_tree(name, &objects, &root);
        group.bench_function(format!("{name}/none"), |b| {
            b.iter(|| walked(&objects, black_box(&root), ()))
        });
        group.bench_function(format!("{name}/faults"), |b| {
            b.iter(|| walked(&objects, black_box(&root), Faults::new()))
        });
    }
    group.finish();
}

/// A packet's start: its wrapper, `x:xmpmeta` and `rdf:RDF`.
const HEAD: &str = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
    <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
    <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n";

/// A packet's end, after its padding.
const TAIL: &str = "<?xpacket end=\"w\"?>";

/// The end of `rdf:RDF` and `x:xmpmeta`.
const CLOSE: &str = "</rdf:RDF>\n</x:xmpmeta>\n";

/// The three namespaces of the claims, bound.
const CLAIMED: &str = "xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" \
    xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\" \
    xmlns:pdfvtid=\"http://www.npes.org/pdfvt/ns/id/\"";

/// What a writer puts beside the claims: a title, an author, dates, the
/// tools, ids and a history.
const PROPERTIES: &str = "<rdf:Description rdf:about=\"\" \
    xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" \
    xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\" \
    xmlns:xmpMM=\"http://ns.adobe.com/xap/1.0/mm/\" \
    xmlns:stEvt=\"http://ns.adobe.com/xap/1.0/sType/ResourceEvent#\">\n\
    <dc:format>application/pdf</dc:format>\n\
    <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Quarterly report &amp; accounts</rdf:li></rdf:Alt></dc:title>\n\
    <dc:creator><rdf:Seq><rdf:li>A. Writer</rdf:li><rdf:li>B. Editor</rdf:li></rdf:Seq></dc:creator>\n\
    <dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">The year's figures, with notes.</rdf:li></rdf:Alt></dc:description>\n\
    <dc:subject><rdf:Bag><rdf:li>report</rdf:li><rdf:li>accounts</rdf:li><rdf:li>figures</rdf:li></rdf:Bag></dc:subject>\n\
    <xmp:CreateDate>2026-10-24T15:30:00-08:00</xmp:CreateDate>\n\
    <xmp:ModifyDate>2026-10-25T09:12:44-08:00</xmp:ModifyDate>\n\
    <xmp:MetadataDate>2026-10-25T09:12:44-08:00</xmp:MetadataDate>\n\
    <xmp:CreatorTool>A word processor 12.4</xmp:CreatorTool>\n\
    <pdf:Producer>A PDF library 3.1</pdf:Producer>\n\
    <pdf:Keywords>report; accounts; figures</pdf:Keywords>\n\
    <xmpMM:DocumentID>uuid:6c1f1b9e-3a4d-4d2b-9a7e-0b3c5d7e9f11</xmpMM:DocumentID>\n\
    <xmpMM:InstanceID>uuid:0a2b4c6d-8e0f-4a1b-8c3d-5e7f9a1b3c5d</xmpMM:InstanceID>\n\
    <xmpMM:History><rdf:Seq>\n\
    <rdf:li rdf:parseType=\"Resource\"><stEvt:action>created</stEvt:action><stEvt:when>2026-10-24T15:30:00-08:00</stEvt:when><stEvt:softwareAgent>A word processor 12.4</stEvt:softwareAgent></rdf:li>\n\
    <rdf:li rdf:parseType=\"Resource\"><stEvt:action>saved</stEvt:action><stEvt:when>2026-10-25T09:12:44-08:00</stEvt:when><stEvt:softwareAgent>A PDF library 3.1</stEvt:softwareAgent></rdf:li>\n\
    </rdf:Seq></xmpMM:History>\n\
    </rdf:Description>\n";

/// Every claim, as attributes of a description of their own.
const AS_ATTRIBUTES: &str = "pdfaid:part=\"2\" pdfaid:conformance=\"B\" pdfaid:rev=\"2011\" \
    pdfuaid:part=\"1\" pdfuaid:rev=\"2014\" pdfvtid:GTS_PDFVTVersion=\"PDF/VT-2\"";

/// Every claim, as elements.
const AS_ELEMENTS: &str = "<pdfaid:part>2</pdfaid:part>\n\
    <pdfaid:conformance>B</pdfaid:conformance>\n\
    <pdfaid:rev>2011</pdfaid:rev>\n\
    <pdfuaid:part>1</pdfuaid:part>\n\
    <pdfuaid:rev>2014</pdfuaid:rev>\n\
    <pdfvtid:GTS_PDFVTVersion>PDF/VT-2</pdfvtid:GTS_PDFVTVersion>\n";

/// What every claim, as written above, gives.
fn every_claim() -> Claims {
    let mut claims = Claims::default();
    claims.pdfa_part = Some("2".into());
    claims.pdfa_conformance = Some("B".into());
    claims.pdfa_rev = Some("2011".into());
    claims.pdfua_part = Some("1".into());
    claims.pdfua_rev = Some("2014".into());
    claims.pdfvt_version = Some("PDF/VT-2".into());
    claims
}

/// A packet of `body`, padded with lines of spaces to `size` bytes where
/// it's shorter, as a writer pads one to be edited in place.
fn packet(body: &str, size: usize) -> Vec<u8> {
    let mut packet = format!("{HEAD}{body}{CLOSE}");
    while packet.len() + TAIL.len() < size {
        let line = (size - packet.len() - TAIL.len()).min(100);
        packet.push_str(&" ".repeat(line - 1));
        packet.push('\n');
    }
    packet.push_str(TAIL);
    packet.into_bytes()
}

/// The packets timed, by name, and what each claims.
fn packets() -> Vec<(&'static str, Vec<u8>, Claims)> {
    let attributes =
        format!("{PROPERTIES}<rdf:Description rdf:about=\"\" {CLAIMED} {AS_ATTRIBUTES}/>\n");
    let elements = format!(
        "{PROPERTIES}<rdf:Description rdf:about=\"\" {CLAIMED}>\n{AS_ELEMENTS}</rdf:Description>\n"
    );
    let mut large = String::from(
        "<rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" ",
    );
    large.push_str(CLAIMED);
    large.push_str(">\n<dc:subject><rdf:Bag>\n");
    let mut keyword = 0;
    while large.len() < (1 << 20) - 4096 {
        large.push_str(&format!("<rdf:li>keyword {keyword}</rdf:li>\n"));
        keyword += 1;
    }
    large.push_str("</rdf:Bag></dc:subject>\n<pdfaid:part>3</pdfaid:part>\n</rdf:Description>\n");
    let mut large_claims = Claims::default();
    large_claims.pdfa_part = Some("3".into());
    let mut spaces = String::from("<rdf:Description rdf:about=\"\" ");
    spaces.push_str(CLAIMED);
    for at in 0..120 {
        spaces.push_str(&format!(" xmlns:ns{at}=\"http://example.com/ns/{at}/\""));
    }
    spaces.push_str(">\n");
    let claims: Vec<&str> = AS_ELEMENTS.lines().collect();
    for at in 0..120 {
        spaces.push_str(&format!("<ns{at}:value>value {at}</ns{at}:value>\n"));
        if at % 20 == 19 {
            spaces.push_str(claims[at / 20]);
            spaces.push('\n');
        }
    }
    spaces.push_str("</rdf:Description>\n");
    let mut deep = format!(
        "<rdf:Description rdf:about=\"\" {CLAIMED} xmlns:ex=\"http://example.com/ns/deep/\">\n<pdfaid:part>\n"
    );
    deep.push_str(&"<ex:level>\n".repeat(250));
    deep.push('9');
    deep.push_str(&"</ex:level>\n".repeat(250));
    deep.push_str(
        "</pdfaid:part>\n<pdfaid:conformance>B</pdfaid:conformance>\n</rdf:Description>\n",
    );
    let mut deep_claims_said = Claims::default();
    deep_claims_said.pdfa_conformance = Some("B".into());
    let mut nested = format!("<rdf:Description rdf:about=\"\" {CLAIMED}>\n");
    nested.push_str(&"<pdfaid:part>\n".repeat(250));
    nested.push('3');
    nested.push_str(&"</pdfaid:part>\n".repeat(250));
    nested.push_str("</rdf:Description>\n");
    let mut nested_claims = Claims::default();
    nested_claims.pdfa_part = Some("3".into());
    vec![
        ("attributes-4k", packet(&attributes, 4096), every_claim()),
        ("elements-4k", packet(&elements, 4096), every_claim()),
        ("large-1m", packet(&large, 1 << 20), large_claims),
        ("namespaces-123", packet(&spaces, 0), every_claim()),
        ("deep-elements-250", packet(&deep, 0), deep_claims_said),
        ("deep-claims-250", packet(&nested, 0), nested_claims),
    ]
}

fn xmp_group(c: &mut Criterion) {
    let packets = packets();
    let mut group = c.benchmark_group("xmp");
    for (name, packet, claims) in &packets {
        let mut faults = Faults::new();
        let read_back = read(packet, None, &Limits::DEFAULT, &mut faults);
        let read_back = read_back.unwrap_or_else(|e| panic!("xmp/{name}: {e}"));
        assert_eq!(&read_back, claims, "xmp/{name}: its claims");
        assert_eq!(faults, [], "xmp/{name}: no fault");
        if name.ends_with("-4k") {
            assert_eq!(packet.len(), 4096, "xmp/{name}: its size");
        }
        if *name == "large-1m" {
            assert_eq!(packet.len(), 1 << 20, "xmp/{name}: its size");
        }
        group.throughput(Throughput::Bytes(packet.len() as u64));
        group.bench_function(*name, |b| {
            b.iter(|| {
                let mut faults = Faults::new();
                let claims = read(black_box(packet), None, &Limits::DEFAULT, &mut faults);
                (claims, faults)
            })
        });
    }
    group.finish();
}

/// A date of each form, by name: as it's written, what writing it gives,
/// and what reading it read past.
const DATES: &[(&str, &str, &str, &[Quirk])] = &[
    ("year", "D:2026", "D:2026", &[]),
    ("day", "D:20261024", "D:20261024", &[]),
    ("local", "D:20261024153000", "D:20261024153000", &[]),
    ("utc", "D:20261024153000Z", "D:20261024153000Z", &[]),
    (
        "ahead",
        "D:20261024153000+05'30",
        "D:20261024153000+05'30",
        &[],
    ),
    (
        "behind",
        "D:20261024153000-08'00'",
        "D:20261024153000-08'00",
        &[Quirk::TerminatingApostrophe],
    ),
];

fn dates_group(c: &mut Criterion) {
    let mut group = c.benchmark_group("dates");
    group.throughput(Throughput::Elements(1));
    for &(name, text, written, quirks) in DATES {
        let date = Date::read(text).unwrap_or_else(|e| panic!("dates/read/{name}: {e}"));
        assert_eq!(
            date.quirks(),
            quirks,
            "dates/read/{name}: what it read past"
        );
        assert_eq!(date.write(), written, "dates/write/{name}: what it writes");
        group.bench_function(format!("read/{name}"), |b| {
            b.iter(|| Date::read(black_box(text)))
        });
        group.bench_function(format!("write/{name}"), |b| {
            b.iter(|| black_box(&date).write())
        });
    }
    group.finish();
}

/// The groups, as one benchmark binary: what `criterion_group!` and
/// `criterion_main!` would write, without the public function the first
/// makes.
fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    pages_group(&mut criterion);
    parents_group(&mut criterion);
    trees_group(&mut criterion);
    xmp_group(&mut criterion);
    dates_group(&mut criterion);
    criterion.final_summary();
}
