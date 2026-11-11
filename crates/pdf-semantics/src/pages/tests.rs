use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use pdf_fixtures::fixture;
use pdf_objects::parse::Parser;
use pdf_structure::read::{self, Options, Reading};

use super::*;
use crate::catalog;
use crate::tree::tests::ByPosition;

/// What takes far less, even in a debug build: a walk's cost past a cap is
/// what these tests hold, not its speed.
const QUICK: Duration = Duration::from_secs(10);

/// Keeps what it's told.
#[derive(Default)]
struct Kept(Vec<(Place, Fault)>);

impl Sink<Fault> for Kept {
    fn fault(&mut self, at: Place, fault: Fault) {
        self.0.push((at, fault));
    }
}

impl Kept {
    fn faults(&self) -> Vec<Fault> {
        self.0.iter().map(|(_, fault)| *fault).collect()
    }
}

fn object(number: u32, body: &str) -> (u32, String) {
    (number, body.to_string())
}

fn catalog_object() -> (u32, String) {
    object(1, "<< /Type /Catalog /Pages 2 0 R >>")
}

/// A file of `objects`.
fn file(objects: &[(u32, String)]) -> Vec<u8> {
    let bodies: Vec<(u32, &[u8])> = (objects.iter())
        .map(|(number, body)| (*number, body.as_bytes()))
        .collect();
    fixture::assembled("1.7", &bodies, "<< /Root 1 0 R >>")
}

/// What a page holds, owned, to set a table beside another reading's.
#[derive(Debug, Clone, PartialEq)]
struct Held {
    id: Option<ObjectId>,
    repeat: u32,
    loads: bool,
    parents: Vec<ObjectId>,
    media: Sourced<Rect>,
    crop: Sourced<Rect>,
    rotate: Sourced<i64>,
    unit: Sourced<f64>,
    resources: Option<(Origin, Object)>,
}

fn held(page: &Page) -> Held {
    Held {
        id: page.id,
        repeat: page.repeat,
        loads: page.dict.is_some(),
        parents: page.parents().collect(),
        media: page.media_box(),
        crop: page.crop_box(),
        rotate: page.rotate(),
        unit: page.user_unit(),
        resources: (page.attribute(Inheritable::Resources))
            .map(|resources| (resources.origin, resources.written.clone())),
    }
}

/// A table, owned, or what failed it, with what was told.
#[derive(Debug, Clone, PartialEq)]
struct Table {
    pages: Result<Vec<Held>, Error>,
    counted: usize,
    faults: Vec<Fault>,
}

fn table<'o, R: Resolve<'o> + Copy>(
    objects: R,
    trailer: &'o Dict,
    rules: Rules,
    limits: &Limits,
) -> Table {
    let catalog = catalog(objects, trailer).unwrap().unwrap();
    let mut kept = Kept::default();
    let walked = pages(objects, catalog, rules, limits, &mut kept);
    let quiet = pages(objects, catalog, rules, limits, &mut ());
    let (held, counted) = match &walked {
        Ok(pages) => (Ok(pages.iter().map(held).collect()), pages.counted()),
        Err(error) => (Err(error.clone()), 0),
    };
    assert_eq!(walked, quiet, "the same whatever the sink");
    Table {
        pages: held,
        counted,
        faults: kept.faults(),
    }
}

/// `objects` as a file, opened through a document, a reading in memory and one
/// by position, which all say the same, as `rules` have it under `limits`.
fn open_with(objects: &[(u32, String)], rules: Rules, limits: &Limits) -> Table {
    let bytes = file(objects);
    let document = read::document(&bytes, &read::Options::DEFAULT);
    let reading = Reading::open(&bytes[..], &Options::DEFAULT).unwrap();
    let source = ByPosition::new(&bytes);
    let positioned = Reading::open(&source, &Options::DEFAULT).unwrap();
    let in_memory = table(&reading, reading.trailer().dict(), rules, limits);
    let by_position = table(&positioned, positioned.trailer().dict(), rules, limits);
    let whole = table(&document, document.trailer().dict(), rules, limits);
    assert_eq!(in_memory, by_position, "in memory and by position");
    assert_eq!(in_memory, whole, "a reading and a document");
    in_memory
}

fn open(objects: &[(u32, String)]) -> Result<Vec<Held>, Error> {
    open_with(objects, Rules::Pdfium, &Limits::DEFAULT).pages
}

fn tree_error(objects: &[(u32, String)]) -> Error {
    open(objects).unwrap_err()
}

#[test]
fn a_count_or_a_parent_past_a_cap_or_a_password_isn_t_checked() {
    // The root's /Count is 6 0 R; node 4's /Parent, 7 0 R, isn't on a page's
    // way up, as page 5 names the root.
    let objects = parsed(&[
        (2, "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 6 0 R >>"),
        (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] >>"),
        (4, "<< /Type /Pages /Parent 7 0 R /Kids [5 0 R] /Count 1 >>"),
        (5, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] >>"),
        (6, "2"),
        (7, "2 0 R"),
    ]);
    let catalog = Dict::from_iter([(b"Pages".to_vec(), Object::reference(2))]);
    for (failed, why) in [
        (ObjectId::new(6, 0), Why::Locked),
        (ObjectId::new(7, 0), Why::Capped),
        (ObjectId::new(6, 0), Why::Source),
    ] {
        let resolver = failing(&objects, failed, why);
        let mut kept = Kept::default();
        let walked = pages(resolver, &catalog, Rules::Iso, &Limits::DEFAULT, &mut kept);
        assert_eq!(
            walked.map(|table| table.len()),
            Ok(2),
            "{failed:?}, {why:?}"
        );
        let faults = kept.faults();
        let unchecked = Fault::Unchecked { id: failed, why };
        assert_eq!(
            faults.iter().filter(|fault| **fault == unchecked).count(),
            1,
            "{faults:?}"
        );
        // Neither said as a count or a parent that's wrong.
        let count = |fault: &Fault| matches!(fault, Fault::PageTreeCount { .. });
        let parent = Fault::PageTreeParent {
            lister: Some(ObjectId::new(2, 0)),
        };
        match failed {
            ObjectId {
                number: 6,
                generation: 0,
            } => assert!(!faults.iter().any(count), "{faults:?}"),
            _ => assert!(!faults.contains(&parent), "{faults:?}"),
        }
        let quiet = pages(resolver, &catalog, Rules::Iso, &Limits::DEFAULT, &mut ());
        assert_eq!(
            quiet.map(|table| table.len()),
            Ok(2),
            "the same with no sink"
        );
    }
    // By PDFium's rules, the root's /Count is what it goes by, which fails it.
    let resolver = failing(&objects, ObjectId::new(6, 0), Why::Locked);
    let walked = pages(resolver, &catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ());
    assert!(matches!(walked, Err(Error::Unreadable(_))), "{walked:?}");
    // A count that doesn't read is the count said.
    let resolver = failing(&objects, ObjectId::new(6, 0), Why::Damaged);
    let mut kept = Kept::default();
    pages(resolver, &catalog, Rules::Iso, &Limits::DEFAULT, &mut kept).unwrap();
    let not_a_number = Fault::PageTreeCount {
        count: None,
        under: 2,
    };
    assert!(kept.faults().contains(&not_a_number), "{:?}", kept.faults());
}

#[test]
fn pdfium_s_count_is_given_where_it_loads_no_page_under_the_root() {
    let cases = [
        ("<< /Type /Pages /Kids [] /Count 5 >>", None, 5),
        ("<< /Type /Pages /Kids [] /Count 1 >>", None, 1),
        ("<< /Type /Pages /Kids [] /Count 0 >>", None, 0),
        ("<< /Type /Pages /Kids [] >>", None, 0),
        ("<< /Type /Pages /Kids (x) /Count 2 >>", None, 2),
        // A node under the root whose /Count PDFium counts by, no page in it.
        (
            "<< /Type /Pages /Kids [3 0 R] /Count 2 >>",
            Some("<< /Type /Pages /Kids [] /Count 0 >>"),
            2,
        ),
        (
            "<< /Type /Pages /Kids [3 0 R] >>",
            Some("<< /Type /Pages /Kids [] /Count 3 >>"),
            3,
        ),
    ];
    for (root, node, count) in cases {
        let mut objects = vec![(2, root)];
        objects.extend(node.map(|node| (3, node)));
        let objects = parsed(&objects);
        let resolve = |id: ObjectId| objects.get(&id);
        let catalog = Dict::from_iter([(b"Pages".to_vec(), Object::reference(2))]);
        let walked = pages(resolve, &catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ());
        assert_eq!(walked, Err(Error::NoPages), "{root}");
        assert_eq!(
            counted(resolve, &catalog, &Limits::DEFAULT),
            Ok(count),
            "{root}"
        );
    }
    // Where it loads one, the table's count; where there's no root, none.
    let objects = parsed(&[
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 4 >>"),
        (3, "<< /Type /Page /Parent 2 0 R >>"),
    ]);
    let resolve = |id: ObjectId| objects.get(&id);
    let catalog = Dict::from_iter([(b"Pages".to_vec(), Object::reference(2))]);
    let table = pages(resolve, &catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ()).unwrap();
    assert_eq!(
        counted(resolve, &catalog, &Limits::DEFAULT),
        Ok(table.counted())
    );
    assert_eq!(table.counted(), 4);
    let nowhere = Dict::from_iter([(b"Pages".to_vec(), Object::reference(9))]);
    assert_eq!(counted(resolve, &nowhere, &Limits::DEFAULT), Ok(0));
    // A cap or a password fails it as it fails the walk.
    let resolver = failing(&objects, ObjectId::new(3, 0), Why::Locked);
    let failed = counted(resolver, &catalog, &Limits::DEFAULT);
    assert!(matches!(failed, Err(Error::Unreadable(_))), "{failed:?}");
}

fn ids(pages: &[Held]) -> Vec<(Option<ObjectId>, u32)> {
    pages.iter().map(|page| (page.id, page.repeat)).collect()
}

/// A page's media box.
fn media(page: &Held) -> [f64; 4] {
    page.media.value.0
}

/// By ISO's rules: what's listed, and what's told.
fn iso(objects: &[(u32, String)]) -> (Vec<(Option<ObjectId>, u32)>, Vec<Fault>) {
    let table = open_with(objects, Rules::Iso, &Limits::DEFAULT);
    (ids(&table.pages.unwrap()), table.faults)
}

// What's walked across threads: nothing of the walk's stays.
#[test]
fn a_table_is_send_and_sync() {
    fn is<T: Send + Sync>() {}
    is::<Pages<'static>>();
    is::<Page<'static>>();
}

#[test]
fn cyclic_kids_are_a_typed_error() {
    let started = Instant::now();
    let page = object(4, "<< /Type /Page /Parent 2 0 R >>");
    // 2 lists 3, which lists 2.
    let cycle = [
        catalog_object(),
        object(2, "<< /Type /Pages /Kids [4 0 R 3 0 R] /Count 2 >>"),
        object(3, "<< /Type /Pages /Parent 2 0 R /Kids [2 0 R] /Count 1 >>"),
        page.clone(),
    ];
    assert_eq!(tree_error(&cycle), Error::Cycle);
    // The root lists itself.
    let itself = [
        catalog_object(),
        object(2, "<< /Type /Pages /Kids [4 0 R 2 0 R] /Count 2 >>"),
        page,
    ];
    assert_eq!(tree_error(&itself), Error::Cycle);
    assert!(started.elapsed() < QUICK);
}

#[test]
fn a_parent_loop_is_a_typed_error() {
    // The page's /Parent is 3, 3's is 4, and 4's is 3 again.
    let objects = [
        catalog_object(),
        object(2, "<< /Type /Pages /Kids [5 0 R] /Count 1 >>"),
        object(3, "<< /Type /Pages /Parent 4 0 R /Kids [5 0 R] >>"),
        object(4, "<< /Type /Pages /Parent 3 0 R /Kids [3 0 R] >>"),
        object(5, "<< /Type /Page /Parent 3 0 R >>"),
    ];
    assert_eq!(tree_error(&objects), Error::ParentLoop);
    // A page that is its own /Parent.
    let objects = [
        catalog_object(),
        object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        object(3, "<< /Type /Page /Parent 3 0 R >>"),
    ];
    assert_eq!(tree_error(&objects), Error::ParentLoop);
}

/// A chain of `depth` nodes from the root (2) down, each listing the next
/// and naming the one above as its `/Parent`, and a page under the last.
fn chain(depth: u32) -> Vec<(u32, String)> {
    let mut objects = vec![catalog_object()];
    for n in 2..2 + depth {
        let parent = if n == 2 {
            String::new()
        } else {
            format!("/Parent {} 0 R ", n - 1)
        };
        objects.push(object(
            n,
            &format!("<< /Type /Pages {parent}/Kids [{} 0 R] /Count 1 >>", n + 1),
        ));
    }
    let last = 1 + depth;
    objects.push(object(
        last + 1,
        &format!("<< /Type /Page /Parent {last} 0 R /MediaBox [0 0 100 200] >>"),
    ));
    objects
}

#[test]
fn a_10000_deep_page_tree_is_a_typed_error() {
    let started = Instant::now();
    assert_eq!(tree_error(&chain(10_000)), Error::TooDeep);
    assert!(started.elapsed() < QUICK, "{:?}", started.elapsed());
    // PDFium's 1,024 levels open, and the page inherits down its 1,024
    // parents.
    let deepest = open(&chain(1024)).unwrap();
    assert_eq!(media(&deepest[0]), [0.0, 0.0, 100.0, 200.0]);
    assert_eq!(deepest[0].parents.len(), 1024);
    assert_eq!(tree_error(&chain(1025)), Error::TooDeep);

    // A shallow tree whose page names a /Parent 10,000 nodes from the top.
    let mut objects = vec![
        catalog_object(),
        object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        object(3, "<< /Type /Page /Parent 4 0 R >>"),
    ];
    for n in 4..10_004 {
        objects.push(object(
            n,
            &format!("<< /Type /Pages /Parent {} 0 R >>", n + 1),
        ));
    }
    let started = Instant::now();
    assert_eq!(tree_error(&objects), Error::TooDeep);
    assert!(started.elapsed() < QUICK, "{:?}", started.elapsed());

    // With no /Parent to hold it to, the walk's levels are the cap, 1,024 the
    // most; a chain with no root /Count the same.
    let levels = |depth: u32, count: &str| {
        let mut objects = vec![catalog_object()];
        for n in 2..2 + depth {
            objects.push(object(n, &format!("<< /Kids [{} 0 R] {count} >>", n + 1)));
        }
        objects.push(object(2 + depth, "<< /Type /Page >>"));
        open(&objects)
    };
    for count in ["/Count 1", ""] {
        assert_eq!(levels(1024, count).unwrap().len(), 1, "{count}");
        assert_eq!(levels(1025, count), Err(Error::TooDeep), "{count}");
    }
    assert_eq!(levels(10_000, ""), Err(Error::TooDeep));
}

// The count of the tree has no cap of depth, and the walk doesn't go as far as
// where it would meet its own (`depth-past-the-count`, in `tests/pdfium`).
#[test]
fn a_tree_too_deep_past_where_the_count_ends_the_walk_isn_t_met() {
    let mut objects = vec![
        catalog_object(),
        object(2, "<< /Kids [3 0 R 4 0 R] >>"),
        object(3, "<< /Type /Page >>"),
    ];
    for n in 4..10_004 {
        objects.push(object(n, &format!("<< /Kids [{} 0 R] >>", n + 1)));
    }
    objects.push(object(10_004, "<< /Kids [] >>"));
    assert_eq!(
        ids(&open(&objects).unwrap()),
        [(Some(ObjectId::new(3, 0)), 0)]
    );
    // So too a node listed inside itself, which the count passes.
    let objects = [
        catalog_object(),
        object(2, "<< /Kids [3 0 R 4 0 R] >>"),
        object(3, "<< /Type /Page >>"),
        object(4, "<< /Kids [4 0 R] >>"),
    ];
    assert_eq!(
        ids(&open(&objects).unwrap()),
        [(Some(ObjectId::new(3, 0)), 0)]
    );
}

/// A resolver of `objects` that keeps what's asked of it.
fn asking<'o, 'a>(
    objects: &'o BTreeMap<ObjectId, Object>,
    asked: &'a std::cell::RefCell<Vec<ObjectId>>,
) -> impl Fn(ObjectId) -> Option<&'o Object> + 'a
where
    'o: 'a,
{
    move |id| {
        asked.borrow_mut().push(id);
        objects.get(&id)
    }
}

/// `count` objects as a /Parent chain from object `first` up, ending at a node
/// with no /Parent.
fn parents_of(first: u32, count: u32) -> Vec<(u32, String)> {
    (first..first + count)
        .map(|n| {
            let parent = if n + 1 < first + count {
                format!("/Parent {} 0 R", n + 1)
            } else {
                String::new()
            };
            object(n, &format!("<< {parent} >>"))
        })
        .collect()
}

#[test]
fn a_parent_chain_is_read_no_further_than_its_cap() {
    let mut text = vec![
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string()),
        (3, "<< /Type /Page /Parent 4 0 R >>".to_string()),
    ];
    text.extend(parents_of(4, 5000));
    let text: Vec<(u32, &str)> = text.iter().map(|(n, s)| (*n, s.as_str())).collect();
    let objects = parsed(&text);
    let catalog = Dict::from_iter([(b"Pages".to_vec(), Object::reference(2))]);
    let asked = std::cell::RefCell::new(Vec::new());
    let resolve = asking(&objects, &asked);
    let walked = pages(resolve, &catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ());
    assert_eq!(walked, Err(Error::TooDeep));
    // The root, the page, and one past the most a chain can have.
    assert_eq!(asked.borrow().len(), 2 + 1025);
}

#[test]
fn a_parent_chain_above_a_node_already_met_is_held_to_the_cap_too() {
    // The first page's chain has 600 nodes, the second's 600 of its own up to
    // the first's: 1,200, where neither went past 1,024 alone.
    let mut objects = vec![
        catalog_object(),
        object(2, "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>"),
        object(3, "<< /Type /Page /Parent 100 0 R >>"),
        object(4, "<< /Type /Page /Parent 1000 0 R >>"),
    ];
    objects.extend(parents_of(100, 600));
    let mut own = parents_of(1000, 600);
    own.last_mut().unwrap().1 = "<< /Parent 100 0 R >>".to_string();
    objects.extend(own);
    assert_eq!(tree_error(&objects), Error::TooDeep);
}

#[test]
fn too_many_pages_or_none_are_typed_errors() {
    let mut limits = Limits::DEFAULT;
    limits.pages = 2;
    let three = [
        catalog_object(),
        object(2, "<< /Type /Pages /Kids [3 0 R 3 0 R 3 0 R] /Count 3 >>"),
        object(3, "<< /Type /Page /Parent 2 0 R >>"),
    ];
    let walked = open_with(&three, Rules::Pdfium, &limits).pages.unwrap_err();
    assert_eq!(walked.to_string(), "the page tree lists too many pages");
    assert_eq!(open(&three).unwrap().len(), 3);
    // A node listing the next twice, 20 deep: a million visits the walk
    // stops at its cap, by either rules.
    let mut objects = vec![catalog_object()];
    for n in 2..22 {
        objects.push(object(
            n,
            &format!("<< /Type /Pages /Kids [{0} 0 R {0} 0 R] >>", n + 1),
        ));
    }
    objects.push(object(22, "<< /Type /Pages /Kids [] >>"));
    let started = Instant::now();
    assert_eq!(
        open_with(&objects, Rules::Pdfium, &limits).pages,
        Err(Error::TooManyPages)
    );
    assert_eq!(
        open_with(&objects, Rules::Iso, &limits).pages,
        Err(Error::TooManyPages)
    );
    assert!(started.elapsed() < QUICK);
    // And under the default caps, each of a million visits a node's own /Kids.
    let bytes = file(&objects);
    let document = read::document(&bytes, &read::Options::DEFAULT);
    let resolver = &document;
    let catalog = document.catalog().unwrap();
    let started = Instant::now();
    assert_eq!(
        pages(resolver, catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ()),
        Err(Error::TooManyPages)
    );
    assert!(started.elapsed() < QUICK, "{:?}", started.elapsed());

    // A count of the tree past what PDFium counts is too many, not that count.
    let objects = [
        catalog_object(),
        object(2, "<< /Kids [3 0 R 4 0 R] >>"),
        object(3, "<< /Kids [5 0 R] /Count 1048574 >>"),
        object(4, "<< /Kids [5 0 R] /Count 1048574 >>"),
        object(5, "<< /Type /Page >>"),
    ];
    assert_eq!(tree_error(&objects), Error::TooManyPages);
    // One short of it isn't.
    let objects = [
        catalog_object(),
        object(2, "<< /Kids [3 0 R 5 0 R] >>"),
        object(3, "<< /Kids [5 0 R] /Count 1048573 >>"),
        object(5, "<< /Type /Page >>"),
    ];
    let table = open_with(&objects, Rules::Pdfium, &Limits::DEFAULT);
    assert_eq!(table.counted, 1_048_574);
    assert_eq!(table.pages.unwrap().len(), 2);

    let empty = [
        catalog_object(),
        object(2, "<< /Type /Pages /Kids [] /Count 0 >>"),
    ];
    assert_eq!(tree_error(&empty), Error::NoPages);
    let no_tree = [object(1, "<< /Type /Catalog >>")];
    assert_eq!(tree_error(&no_tree), Error::NoPages);
}

// The kids here read as chromium/8125 reads them, unlike the first of these
// tests' first reading: a direct kid has a place, and its number (the cases'
// `kid-direct-page`), a leaf of `/Type /Pages` is a place with no page
// (`leaf-type-pages`), and a /Count of 4 ends the table before object 5.
#[test]
fn a_page_listed_twice_is_two_pages() {
    let objects = [
        catalog_object(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R 4 0 R 3 0 R 5 0 R] /Count 5 >>",
        ),
        object(3, "<< /Type /Page /Parent 2 0 R >>"),
        object(4, "<< /Type /Page /Parent 2 0 R >>"),
        // A node listed twice lists its page twice, as PDFium walks it.
        object(
            5,
            "<< /Type /Pages /Parent 2 0 R /Kids [6 0 R 6 0 R] /Count 2 >>",
        ),
        object(6, "<< /Type /Page /Parent 5 0 R >>"),
    ];
    assert_eq!(
        ids(&open(&objects).unwrap()),
        [
            (Some(ObjectId::new(3, 0)), 0),
            (Some(ObjectId::new(4, 0)), 0),
            (Some(ObjectId::new(3, 0)), 1),
            (Some(ObjectId::new(6, 0)), 0),
            (Some(ObjectId::new(6, 0)), 1)
        ]
    );
    // A kid with /Kids is a node whatever its /Type, and any other a page.
    // A kid that names no dictionary keeps its place, with no page at it, as
    // PDFium keeps it; so does a leaf of another /Type; and one written in
    // place is a page, with no number.
    let objects = [
        catalog_object(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R 4 0 R 9 0 R << /Type /Page >> 5 0 R 7 0 R] /Count 5 >>",
        ),
        object(3, "<< /Type /Page /Kids [4 0 R] >>"),
        object(4, "<< /Type /Pages /Parent 2 0 R /MediaBox [0 0 10 10] >>"),
        object(5, "[1 2 3]"),
        object(7, "<< /Type /Page >>"),
    ];
    let read = open(&objects).unwrap();
    assert_eq!(
        ids(&read),
        [
            (Some(ObjectId::new(4, 0)), 0),
            (Some(ObjectId::new(4, 0)), 1),
            (Some(ObjectId::new(9, 0)), 0),
            (None, 0),
            (Some(ObjectId::new(5, 0)), 0),
        ]
    );
    assert_eq!(
        read.iter().map(|page| page.loads).collect::<Vec<_>>(),
        [false, false, false, true, false]
    );
    assert_eq!(media(&read[1]), Rect::LETTER.0);
    assert!(read[3].parents.is_empty());
    // A root without /Kids is the one page.
    let one = [
        catalog_object(),
        object(2, "<< /Type /Page /MediaBox [0 0 10 20] >>"),
    ];
    let read = open(&one).unwrap();
    assert_eq!(ids(&read), [(Some(ObjectId::new(2, 0)), 0)]);
    assert_eq!(media(&read[0]), [0.0, 0.0, 10.0, 20.0]);
}

/// The root's `/Kids`, `n` pages from object `first` on, under a root
/// with `keys`.
fn root(first: u32, n: u32, keys: &str) -> (u32, String) {
    let kids: String = (first..first + n).map(|k| format!("{k} 0 R ")).collect();
    object(2, &format!("<< /Type /Pages /Kids [{kids}] {keys} >>"))
}

// What a page holds is the dictionary the resolver keeps, whatever it names:
// its `/Annots` and `/Contents` are its caller's to read from there, so sharing
// them isn't this walk's. What holds here is that it clones none of them.
#[test]
fn pages_share_what_they_hold_however_often_it_s_listed() {
    // 1,000 pages naming one /Annots array of 10,000 entries read it no more.
    let mut objects = vec![
        catalog_object(),
        root(4, 1000, ""),
        object(3, &format!("[{}]", "10 0 R ".repeat(10_000))),
    ];
    for n in 4..1004 {
        objects.push(object(n, "<< /Type /Page /Parent 2 0 R /Annots 3 0 R >>"));
    }
    let started = Instant::now();
    assert_eq!(open(&objects).unwrap().len(), 1000);
    assert!(started.elapsed() < QUICK, "{:?}", started.elapsed());

    // The root lists a node 300 times, and the node its page 300 times:
    // 90,000 pages, each the same dictionary, what it inherits the same chain.
    let objects = [
        catalog_object(),
        object(
            2,
            &format!("<< /Type /Pages /Kids [{}] >>", "3 0 R ".repeat(300)),
        ),
        object(
            3,
            &format!(
                "<< /Type /Pages /Parent 2 0 R /Kids [{}] >>",
                "4 0 R ".repeat(300)
            ),
        ),
        object(
            4,
            &format!(
                "<< /Type /Page /Parent 3 0 R /Annots [{}] /Contents [5 0 R 6 0 R] \
                 /Resources << /Font << /F1 7 0 R >> >> >>",
                "10 0 R ".repeat(100)
            ),
        ),
    ];
    let bytes = file(&objects);
    let reading = Reading::open(&bytes[..], &Options::DEFAULT).unwrap();
    let resolver = &reading;
    let catalog = catalog(resolver, reading.trailer().dict())
        .unwrap()
        .unwrap();
    let started = Instant::now();
    let table = pages(resolver, catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ()).unwrap();
    assert!(started.elapsed() < QUICK, "{:?}", started.elapsed());
    assert_eq!(table.len(), 90_000);
    let first = table.get(0).unwrap();
    for (i, page) in table.iter().enumerate() {
        assert_eq!(
            (page.id, page.repeat),
            (Some(ObjectId::new(4, 0)), i as u32)
        );
        assert!(std::ptr::eq(page.dict.unwrap(), first.dict.unwrap()));
        assert!(Arc::ptr_eq(
            page.parent.as_ref().unwrap(),
            first.parent.as_ref().unwrap()
        ));
        let resources =
            |page: &Page<'_>| page.attribute(Inheritable::Resources).unwrap().value as *const _;
        assert_eq!(resources(page), resources(first));
    }
}

// A page listed again holds what it held: nothing's read for it again but the
// reference that lists it.
#[test]
fn a_page_listed_again_isn_t_read_again() {
    let objects = parsed(&[
        (2, "<< /Type /Pages /Kids [3 0 R 3 0 R 3 0 R] /Count 3 >>"),
        (3, "<< /Type /Page /Resources 9 0 R /MediaBox 9 0 R >>"),
        (9, "[0 0 1 1]"),
    ]);
    let catalog = Dict::from_iter([(b"Pages".to_vec(), Object::reference(2))]);
    let asked = std::cell::RefCell::new(Vec::new());
    let resolve = asking(&objects, &asked);
    let table = pages(resolve, &catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ()).unwrap();
    assert_eq!(table.len(), 3);
    let asked_for = |id| asked.borrow().iter().filter(|asked| **asked == id).count();
    assert_eq!(
        asked_for(ObjectId::new(9, 0)),
        2,
        "/Resources and /MediaBox, once each"
    );
    assert_eq!(asked_for(ObjectId::new(3, 0)), 3, "each listing");
}

// Pages under one node share its chain, so a /Parent chain is walked once.
#[test]
fn pages_under_one_node_share_its_chain() {
    let objects = [
        catalog_object(),
        object(2, "<< /Type /Pages /Kids [3 0 R] /Count 2 >>"),
        object(
            3,
            "<< /Type /Pages /Parent 2 0 R /Kids [4 0 R 5 0 R] /Count 2 >>",
        ),
        object(4, "<< /Type /Page /Parent 3 0 R >>"),
        object(5, "<< /Type /Page /Parent 3 0 R >>"),
    ];
    let bytes = file(&objects);
    let document = read::document(&bytes, &read::Options::DEFAULT);
    let resolver = &document;
    let catalog = document.catalog().unwrap();
    let table = pages(resolver, catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ()).unwrap();
    let chain = |index: usize| table.get(index).unwrap().parent.as_ref().unwrap();
    assert!(Arc::ptr_eq(chain(0), chain(1)));
    let parents: Vec<ObjectId> = table.get(0).unwrap().parents().collect();
    assert_eq!(parents, [ObjectId::new(3, 0), ObjectId::new(2, 0)]);
}

// A page's parents are its chain's numbered nodes, nearest first, the same
// walked by `next` as by `fold`, from its start or part of the way up.
#[test]
fn parents_are_walked_alike_by_next_and_by_fold() {
    // Page 3's /Parent is 5, whose own is written in place, and that one's 2.
    let objects = parsed(&[
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        (3, "<< /Type /Page /Parent 5 0 R >>"),
        (
            5,
            "<< /Type /Pages /Parent << /Type /Pages /Parent 2 0 R >> >>",
        ),
    ]);
    let resolve = |id: ObjectId| objects.get(&id);
    let catalog = Dict::from_iter([(b"Pages".to_vec(), Object::reference(2))]);
    let table = pages(resolve, &catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ()).unwrap();
    let page = table.get(0).unwrap();
    let chain = [ObjectId::new(5, 0), ObjectId::new(2, 0)];
    let push = |mut ids: Vec<ObjectId>, id| {
        ids.push(id);
        ids
    };

    let mut parents = page.parents();
    assert_eq!(parents.size_hint(), (0, None));
    let mut by_next = Vec::new();
    for id in parents.by_ref() {
        by_next.push(id);
    }
    assert_eq!(by_next, chain);
    assert_eq!(parents.next(), None, "once walked, it gives none again");
    assert_eq!(parents.size_hint(), (0, Some(0)));
    assert_eq!(parents.fold(Vec::new(), push), []);

    assert_eq!(page.parents().fold(Vec::new(), push), chain);
    assert_eq!(page.parents().count(), 2);
    assert_eq!(page.parents().last(), Some(chain[1]));

    let mut part = page.parents();
    assert_eq!(part.next(), Some(chain[0]));
    assert_eq!(format!("{part:?}"), format!("Parents({:?})", [chain[1]]));
    assert_eq!(part.clone().fold(Vec::new(), push), [chain[1]]);
    assert_eq!(part.collect::<Vec<_>>(), [chain[1]]);
    assert_eq!(
        format!("{:?}", page.parents()),
        format!("Parents({chain:?})")
    );
}

// Every attribute, in the order it's declared, with the key a page's own is
// read by; the key's a constant.
#[test]
fn every_attribute_is_listed_in_order_with_its_key() {
    const MEDIA_BOX: &str = Inheritable::MediaBox.key();
    assert_eq!(MEDIA_BOX, "MediaBox");
    let listed = Inheritable::ALL.map(|attribute| (attribute as usize, attribute.key()));
    let keys = [
        (0, "Resources"),
        (1, "MediaBox"),
        (2, "CropBox"),
        (3, "Rotate"),
    ];
    assert_eq!(listed, keys);
    // Each is listed once, as a match over every one says.
    let at = |attribute| match attribute {
        Inheritable::Resources => 0,
        Inheritable::MediaBox => 1,
        Inheritable::CropBox => 2,
        Inheritable::Rotate => 3,
    };
    assert!(
        Inheritable::ALL
            .iter()
            .enumerate()
            .all(|(i, &a)| at(a) == i)
    );
    for attribute in Inheritable::ALL {
        let page = format!("<< /Type /Page /Parent 2 0 R /{} 7 0 R >>", attribute.key());
        let objects = parsed(&[
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, &page),
            (7, "90"),
        ]);
        let resolve = |id: ObjectId| objects.get(&id);
        let catalog = Dict::from_iter([(b"Pages".to_vec(), Object::reference(2))]);
        let table = pages(resolve, &catalog, Rules::Iso, &Limits::DEFAULT, &mut ()).unwrap();
        let page = table.get(0).unwrap();
        for other in Inheritable::ALL {
            let found = (page.attribute(other)).map(|at| (at.origin, at.written.as_reference()));
            let own = (other == attribute).then_some((Origin::Own, Some(ObjectId::new(7, 0))));
            assert_eq!(found, own, "{attribute:?}'s key, {other:?}");
        }
    }
}

#[test]
fn a_broken_object_every_page_names_is_read_once() {
    // 1,000 pages whose /MediaBox is object 3, an array of 300,000 numbers
    // that never closes: parsed once, not once a page.
    let mut objects = vec![
        catalog_object(),
        root(4, 1000, ""),
        object(3, &format!("[{}", "1 ".repeat(300_000))),
    ];
    for n in 4..1004 {
        objects.push(object(n, "<< /Type /Page /Parent 2 0 R /MediaBox 3 0 R >>"));
    }
    let bytes = file(&objects);
    let source = ByPosition::new(&bytes);
    let reading = Reading::open(&source, &Options::DEFAULT).unwrap();
    let resolver = &reading;
    let catalog = catalog(resolver, reading.trailer().dict())
        .unwrap()
        .unwrap();
    let started = Instant::now();
    let mut kept = Kept::default();
    let table = pages(
        resolver,
        catalog,
        Rules::Pdfium,
        &Limits::DEFAULT,
        &mut kept,
    )
    .unwrap();
    assert!(started.elapsed() < QUICK, "{:?}", started.elapsed());
    assert_eq!(table.len(), 1000);
    let last = table.get(999).unwrap();
    assert_eq!(last.media_box().value, Rect::LETTER);
    assert_eq!(last.media_box().origin, Origin::Default);
    // Said, each time a page names it.
    assert_eq!(kept.faults().len(), 1000);
    assert!(kept.faults().iter().all(|fault| *fault
        == Fault::Damaged {
            id: ObjectId::new(3, 0)
        }));
    // And it fails as it failed the first time.
    for _ in 0..2 {
        let unreadable = (&reading).try_resolve(ObjectId::new(3, 0)).unwrap_err();
        assert_eq!(unreadable.why, Why::Damaged);
    }
}

#[test]
fn the_root_s_count_ends_the_table_as_it_ends_pdfium_s() {
    let tree = |count: &str| {
        let mut objects = vec![catalog_object(), root(3, 3, &format!("/Count {count}"))];
        for n in 3..6 {
            objects.push(object(n, "<< /Type /Page /Parent 2 0 R >>"));
        }
        objects.push(object(9, "1"));
        objects
    };
    let read = |count: &str| open_with(&tree(count), Rules::Pdfium, &Limits::DEFAULT);
    // PDFium loads no page past a /Count it counts by, a real's whole part
    // and a reference followed.
    for (count, pages) in [("2", 2), ("2.9", 2), ("9 0 R", 1)] {
        let read = read(count);
        assert_eq!(read.pages.unwrap().len(), pages, "{count}");
        assert_eq!(read.counted, pages, "{count}");
    }
    // Past the tree, the table is the tree's pages, and the count PDFium gives
    // the count; one PDFium doesn't count by, and the tree counts for both.
    let past = read("5");
    assert_eq!((past.pages.unwrap().len(), past.counted), (3, 5));
    for count in ["0", "-1", "1048575", "(2)"] {
        let read = read(count);
        assert_eq!((read.pages.unwrap().len(), read.counted), (3, 3), "{count}");
    }
    // The walk stops there, so a cycle past it isn't met, as PDFium
    // doesn't meet it.
    let objects = [
        catalog_object(),
        object(2, "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 1 >>"),
        object(3, "<< /Type /Page /Parent 2 0 R >>"),
        object(4, "<< /Type /Pages /Parent 2 0 R /Kids [2 0 R] >>"),
    ];
    assert_eq!(
        ids(&open(&objects).unwrap()),
        [(Some(ObjectId::new(3, 0)), 0)]
    );

    // A /Count a hostile file claims doesn't take room it hasn't pages for.
    let objects = [
        catalog_object(),
        object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1048574 >>"),
        object(3, "<< /Type /Page >>"),
    ];
    let bytes = file(&objects);
    let document = read::document(&bytes, &read::Options::DEFAULT);
    let resolver = &document;
    let catalog = document.catalog().unwrap();
    let table = pages(resolver, catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ()).unwrap();
    assert_eq!((table.len(), table.counted()), (1, 1_048_574));
    assert!(table.pages.capacity() <= ROOM, "{}", table.pages.capacity());
}

// Moved with its objects; a page's size is PDFium's reading of what's given
// here, which the cases hold (`tests/pdfium`), so it's the boxes that are held.
#[test]
fn inherited_boxes_and_rotation_read_as_pdfium_reads_them() {
    let objects = [
        catalog_object(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R 10 0 R 11 0 R 12 0 R 13 0 R 14 0 R 15 0 R] \
             /MediaBox 20 0 R /Rotate 135 >>",
        ),
        object(
            3,
            "<< /Type /Pages /Parent 2 0 R /Kids [4 0 R 5 0 R] /CropBox [50 50 550 750] >>",
        ),
        // Everything inherited.
        object(4, "<< /Type /Page /Parent 3 0 R >>"),
        // Its own rotation and media box, corners reversed, and the crop it inherits.
        object(
            5,
            "<< /Type /Page /Parent 3 0 R /Rotate -90 /MediaBox [300 400 0 0] >>",
        ),
        // Its own boxes, a number by reference; as written, the empty one too.
        object(
            10,
            "<< /Type /Page /Parent 2 0 R /Rotate 45 /MediaBox [0 0 0 0] /CropBox [-100 -100 300 21 0 R] >>",
        ),
        object(
            11,
            "<< /Type /Page /Parent 2 0 R /Rotate 0 /CropBox [700 0 900 100] >>",
        ),
        // UserUnit is its own, and not inherited.
        object(
            12,
            "<< /Type /Page /Parent 2 0 R /Rotate 0 /UserUnit 2.5 /MediaBox [0 0 100 200] >>",
        ),
        // Inherited by /Parent, not by the path that listed it: from 3.
        object(13, "<< /Type /Page /Parent 3 0 R /Rotate 0 >>"),
        // A media box that isn't four numbers is no box, and isn't inherited
        // past; a /Rotate that's a real turns by its whole part.
        object(
            14,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612] /Rotate 180.9 >>",
        ),
        // A null /CropBox is found, as in PDFium, and isn't a box: 3's
        // crop isn't inherited past it, and the crop is the media box.
        object(15, "<< /Type /Page /Parent 3 0 R /CropBox null >>"),
        object(20, "[0 0 600 800]"),
        object(21, "300"),
    ];
    let read = open(&objects).unwrap();
    assert_eq!(read.len(), 8);
    let two = Origin::Ancestor(Some(ObjectId::new(2, 0)));
    let three = Origin::Ancestor(Some(ObjectId::new(3, 0)));

    assert_eq!(
        read[0].media,
        Sourced {
            value: Rect([0.0, 0.0, 600.0, 800.0]),
            origin: two
        }
    );
    assert_eq!(
        read[0].crop,
        Sourced {
            value: Rect([50.0, 50.0, 550.0, 750.0]),
            origin: three
        }
    );
    assert_eq!(
        read[0].rotate,
        Sourced {
            value: 135,
            origin: two
        }
    );
    assert_eq!(
        read[0].unit,
        Sourced {
            value: 1.0,
            origin: Origin::Default
        }
    );
    assert_eq!(read[0].parents, [ObjectId::new(3, 0), ObjectId::new(2, 0)]);

    assert_eq!(
        read[1].rotate,
        Sourced {
            value: -90,
            origin: Origin::Own
        }
    );
    assert_eq!(
        read[1].media,
        Sourced {
            value: Rect([300.0, 400.0, 0.0, 0.0]),
            origin: Origin::Own
        }
    );
    assert_eq!(read[1].crop.origin, three);

    assert_eq!(read[2].media.value, Rect([0.0, 0.0, 0.0, 0.0]));
    assert_eq!(read[2].media.origin, Origin::Own);
    assert_eq!(read[2].crop.value, Rect([-100.0, -100.0, 300.0, 300.0]));
    assert_eq!(read[2].rotate.value, 45);

    assert_eq!(read[3].crop.value, Rect([700.0, 0.0, 900.0, 100.0]));

    assert_eq!(
        read[4].unit,
        Sourced {
            value: 2.5,
            origin: Origin::Own
        }
    );

    assert_eq!(read[5].crop.value, Rect([50.0, 50.0, 550.0, 750.0]));
    assert_eq!(read[5].crop.origin, three);

    assert_eq!(
        read[6].media,
        Sourced {
            value: Rect::LETTER,
            origin: Origin::Default
        }
    );
    assert_eq!(
        read[6].rotate,
        Sourced {
            value: 180,
            origin: Origin::Own
        }
    );

    assert_eq!(
        read[7].crop,
        Sourced {
            value: Rect([0.0, 0.0, 600.0, 800.0]),
            origin: Origin::Default
        }
    );
    assert_eq!(
        read[7].rotate,
        Sourced {
            value: 135,
            origin: two
        }
    );

    // By ISO's, null is absent: 15 inherits 3's crop.
    let iso = open_with(&objects, Rules::Iso, &Limits::DEFAULT)
        .pages
        .unwrap();
    assert_eq!(iso[7].crop.value, Rect([50.0, 50.0, 550.0, 750.0]));
    assert_eq!(iso[7].crop.origin, three);
}

// An attribute as written, what it reads as, and where it says it's from.
#[test]
fn an_attribute_says_where_it_s_from_and_what_it_s_written_as() {
    let objects = [
        catalog_object(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /Resources 6 0 R /Rotate 90 >>",
        ),
        object(3, "<< /Type /Page /Parent 2 0 R >>"),
        object(4, "<< /Type /Page /Parent 2 0 R /Resources << /A 1 >> >>"),
        object(6, "<< /B 2 >>"),
    ];
    let bytes = file(&objects);
    let document = read::document(&bytes, &read::Options::DEFAULT);
    let resolver = &document;
    let catalog = document.catalog().unwrap();
    let table = pages(resolver, catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ()).unwrap();

    let inherited = table
        .get(0)
        .unwrap()
        .attribute(Inheritable::Resources)
        .unwrap();
    assert_eq!(inherited.written.as_reference(), Some(ObjectId::new(6, 0)));
    assert!(inherited.value.as_dict().unwrap().has(b"B"));
    assert_eq!(
        inherited.origin,
        Origin::Ancestor(Some(ObjectId::new(2, 0)))
    );

    let own = table
        .get(1)
        .unwrap()
        .attribute(Inheritable::Resources)
        .unwrap();
    assert!(own.written.as_dict().unwrap().has(b"A"));
    assert_eq!(own.origin, Origin::Own);
    assert_eq!(
        table
            .get(1)
            .unwrap()
            .attribute(Inheritable::Rotate)
            .unwrap()
            .origin,
        Origin::Ancestor(Some(ObjectId::new(2, 0)))
    );
    assert!(
        table
            .get(1)
            .unwrap()
            .attribute(Inheritable::MediaBox)
            .is_none()
    );
}

// PDFium's rows, each a case in `tests/pdfium` too, where what it answers is held.

#[test]
fn by_pdfium_s_rules_a_kid_in_place_has_a_place_and_a_node_under_it_too() {
    let objects = [
        catalog_object(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R << /Type /Page /MediaBox [0 0 77 77] >> \
             << /Kids [4 0 R] >> 5 0 R] /Count 4 >>",
        ),
        object(3, "<< /Type /Page >>"),
        object(4, "<< /Type /Page >>"),
        object(5, "<< /Type /Page >>"),
    ];
    let read = open(&objects).unwrap();
    assert_eq!(
        ids(&read),
        [
            (Some(ObjectId::new(3, 0)), 0),
            (None, 0),
            (Some(ObjectId::new(4, 0)), 0),
            (Some(ObjectId::new(5, 0)), 0)
        ]
    );
    assert_eq!(media(&read[1]), [0.0, 0.0, 77.0, 77.0]);
}

#[test]
fn by_pdfium_s_rules_a_kid_that_names_no_dictionary_takes_its_place() {
    for kid in ["88 0 R", "9 0 R", "null", "7", "/Page", "10 0 R"] {
        let read = |count: &str| {
            let objects = [
                catalog_object(),
                object(
                    2,
                    &format!("<< /Type /Pages /Kids [3 0 R {kid} 5 0 R] {count} >>"),
                ),
                object(3, "<< /Type /Page >>"),
                object(5, "<< /Type /Page >>"),
                object(9, "1"),
                // A kid isn't followed through a chain.
                object(10, "3 0 R"),
            ];
            open_with(&objects, Rules::Pdfium, &Limits::DEFAULT)
        };
        // Counted, the place takes the second number, and the last has 5.
        let counted = read("/Count 3");
        let pages = counted.pages.unwrap();
        assert_eq!(pages.len(), 3, "{kid}");
        assert!(!pages[1].loads, "{kid}");
        assert_eq!(pages[2].id, Some(ObjectId::new(5, 0)), "{kid}");
        // Counted out, it still takes a place, so 5 has no number at all.
        let uncounted = read("");
        let pages = uncounted.pages.unwrap();
        assert_eq!(uncounted.counted, 2, "{kid}");
        assert_eq!(pages.len(), 2, "{kid}");
        assert_eq!(pages[0].id, Some(ObjectId::new(3, 0)), "{kid}");
        assert!(!pages[1].loads, "{kid}");
    }
}

#[test]
fn by_pdfium_s_rules_a_node_s_count_is_trusted_where_the_root_s_isn_t_there() {
    let tree = |root: &str, node: &str| {
        [
            catalog_object(),
            object(2, &format!("<< /Type /Pages /Kids [6 0 R 5 0 R] {root} >>")),
            object(
                6,
                &format!("<< /Type /Pages /Parent 2 0 R /Kids [3 0 R 4 0 R] {node} >>"),
            ),
            object(3, "<< /Type /Page /Parent 6 0 R >>"),
            object(4, "<< /Type /Page /Parent 6 0 R >>"),
            object(5, "<< /Type /Page /Parent 2 0 R >>"),
        ]
    };
    let read = |root, node| {
        let table = open_with(&tree(root, node), Rules::Pdfium, &Limits::DEFAULT);
        let pages = table
            .pages
            .unwrap()
            .iter()
            .filter_map(|page| page.id)
            .map(|id| id.number)
            .collect::<Vec<_>>();
        (pages, table.counted)
    };
    // Less than what's under it: it ends the table sooner.
    assert_eq!(read("", "/Count 1"), (vec![3, 4], 2));
    // More: the count is what it says, and the tree ends first.
    assert_eq!(read("", "/Count 9"), (vec![3, 4, 5], 10));
    // None that's positive: what's under it counts.
    assert_eq!(read("", "/Count 0"), (vec![3, 4, 5], 3));
    assert_eq!(read("", ""), (vec![3, 4, 5], 3));
    // Under a root that has one, no node's is read.
    assert_eq!(read("/Count 3", "/Count 1"), (vec![3, 4, 5], 3));
}

#[test]
fn by_pdfium_s_rules_type_doesn_t_tell_a_node_from_a_page() {
    let one = |kid: &str, root: &str| {
        let objects = [
            catalog_object(),
            object(2, &format!("<< /Type /Pages /Kids [3 0 R] {root} >>")),
            object(3, kid),
            object(4, "<< /Type /Page >>"),
        ];
        open_with(&objects, Rules::Pdfium, &Limits::DEFAULT).pages
    };
    // With /Kids, a node whatever its /Type.
    for kind in ["/Page", "/Foo", "(Pages)", "null"] {
        let kid = format!("<< /Type {kind} /Kids [4 0 R] >>");
        let read = one(&kid, "/Count 1").unwrap();
        assert_eq!(ids(&read), [(Some(ObjectId::new(4, 0)), 0)], "{kind}");
        assert!(read[0].loads, "{kind}");
    }
    // Without, its /Type says whether it loads, under a root's /Count.
    assert!(one("<< /Type /Page >>", "/Count 1").unwrap()[0].loads);
    assert!(one("<< >>", "/Count 1").unwrap()[0].loads);
    for kind in ["/Pages", "/Foo", "(Page)", "null", "88 0 R"] {
        let kid = format!("<< /Type {kind} >>");
        let read = one(&kid, "/Count 1").unwrap();
        assert_eq!(ids(&read), [(Some(ObjectId::new(3, 0)), 0)], "{kind}");
        assert!(!read[0].loads, "{kind}");
    }
    // But with no root /Count, what the count of the tree counts loads, and
    // that's all but /Pages, which counts for none.
    for kind in ["/Foo", "(Pages)", "null", "88 0 R"] {
        let kid = format!("<< /Type {kind} >>");
        assert!(one(&kid, "").unwrap()[0].loads, "{kind}");
    }
    assert_eq!(one("<< /Type /Pages >>", ""), Err(Error::NoPages));
    // /Page with /Kids counts for one however many it lists.
    let kid = "<< /Type /Page /Kids [4 0 R 4 0 R] >>";
    assert_eq!(
        ids(&one(kid, "").unwrap()),
        [(Some(ObjectId::new(4, 0)), 0)]
    );
}

#[test]
fn by_pdfium_s_rules_the_root_s_type_doesn_t_stop_it_being_the_one_page() {
    let root = |dict: &str| {
        let objects = [catalog_object(), object(2, dict)];
        open_with(&objects, Rules::Pdfium, &Limits::DEFAULT)
            .pages
            .unwrap()
    };
    assert!(root("<< /Type /Foo >>")[0].loads);
    assert!(root("<< /Type /Page /Count 5 >>")[0].loads);
    assert!(!root("<< /Type /Pages >>")[0].loads);
    // Direct too: no number, as a kid written in place.
    let objects = [object(1, "<< /Pages << /MediaBox [0 0 2 3] >> >>")];
    let read = open(&objects).unwrap();
    assert_eq!(read[0].id, None);
    assert_eq!(media(&read[0]), [0.0, 0.0, 2.0, 3.0]);
}

#[test]
fn by_pdfium_s_rules_only_what_doesn_t_read_is_said() {
    let objects = [
        catalog_object(),
        object(
            2,
            "<< /Type /Foo /Kids [3 0 R 3 0 R << /Type /Page >> null 88 0 R 5 0 R] /Parent 7 0 R >>",
        ),
        object(
            3,
            "<< /Type /Foo /Parent << /Parent 9 0 R >> /Rotate 9 0 R >>",
        ),
        object(5, "<< /Kids [3 0 R] /Count 9 >>"),
        object(8, "9 0 R"),
        object(9, "2 0 R"),
    ];
    let table = open_with(&objects, Rules::Pdfium, &Limits::DEFAULT);
    assert_eq!(table.faults, []);
    assert!(table.pages.is_ok());
}

#[test]
fn by_pdfium_s_rules_a_root_of_type_page_with_kids_no_array_is_page_0_of_its_count() {
    let root = |dict: &str| {
        let objects = [
            catalog_object(),
            object(2, dict),
            object(3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 3 200] >>"),
            object(8, "9 0 R"),
            object(9, "[3 0 R]"),
            object(10, "7"),
            object(11, "/Page"),
        ];
        open_with(&objects, Rules::Pdfium, &Limits::DEFAULT)
    };
    // However /Kids isn't one, by any /Count PDFium goes by: page 0, kept to
    // the count, and none past it.
    for kids in ["(x)", "null", "88 0 R", "10 0 R", "8 0 R", "2 0 R"] {
        for (count, counted) in [("1", 1), ("3", 3), ("2.5", 2), ("10 0 R", 7)] {
            let dict =
                format!("<< /Type /Page /Kids {kids} /Count {count} /MediaBox [0 0 2 200] >>");
            let table = root(&dict);
            assert_eq!(table.counted, counted, "{dict}");
            let read = table.pages.unwrap();
            assert_eq!(ids(&read), [(Some(ObjectId::new(2, 0)), 0)], "{dict}");
            assert!(read[0].loads, "{dict}");
            assert_eq!(media(&read[0]), [0.0, 0.0, 2.0, 200.0], "{dict}");
        }
        // A count of 0 by PDFium's way of it: no pages.
        for count in ["", "/Count 0", "/Count -1", "/Count 1048575"] {
            let dict = format!("<< /Type /Page /Kids {kids} {count} >>");
            assert_eq!(root(&dict).pages, Err(Error::NoPages), "{dict}");
        }
        // Another /Type, none, or one that reads as /Page only through a
        // reference: no pages either.
        for kind in [
            "",
            "/Type /Foo",
            "/Type /Pages",
            "/Type (Page)",
            "/Type 11 0 R",
        ] {
            let dict = format!("<< {kind} /Kids {kids} /Count 1 >>");
            assert_eq!(root(&dict).pages, Err(Error::NoPages), "{dict}");
        }
    }
    // An array is walked, even one with nothing in it.
    let dict = "<< /Type /Page /Kids [] /Count 2 >>";
    assert_eq!(root(dict).pages, Err(Error::NoPages));
    let table = root("<< /Type /Page /Kids [3 0 R] /Count 2 /MediaBox [0 0 2 200] >>");
    assert_eq!(ids(&table.pages.unwrap()), [(Some(ObjectId::new(3, 0)), 0)]);
    // In place, it's page 0 with no number, and it inherits as a page does.
    let objects = [
        object(
            1,
            "<< /Pages << /Type /Page /Kids (x) /Count 3 /Parent 4 0 R >> >>",
        ),
        object(4, "<< /MediaBox [0 0 41 42] /Rotate 90 >>"),
    ];
    let table = open_with(&objects, Rules::Pdfium, &Limits::DEFAULT);
    assert_eq!(table.counted, 3);
    let read = table.pages.unwrap();
    assert_eq!(ids(&read), [(None, 0)]);
    assert_eq!(media(&read[0]), [0.0, 0.0, 41.0, 42.0]);
    assert_eq!(
        read[0].media.origin,
        Origin::Ancestor(Some(ObjectId::new(4, 0)))
    );
    // A /Kids that doesn't read isn't an array, and is said once.
    let objects = parsed(&[(2, "<< /Type /Page /Kids 6 0 R /Count 3 >>")]);
    let catalog = Dict::from_iter([(b"Pages".to_vec(), Object::reference(2))]);
    let mut kept = Kept::default();
    let resolver = failing(&objects, ObjectId::new(6, 0), Why::Damaged);
    let table = pages(
        resolver,
        &catalog,
        Rules::Pdfium,
        &Limits::DEFAULT,
        &mut kept,
    )
    .unwrap();
    assert_eq!((table.len(), table.counted()), (1, 3));
    assert_eq!(
        kept.faults(),
        [Fault::Damaged {
            id: ObjectId::new(6, 0)
        }]
    );
    // One that fails the walk fails it here too.
    let resolver = failing(&objects, ObjectId::new(6, 0), Why::Locked);
    let walked = pages(resolver, &catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ());
    assert!(matches!(walked, Err(Error::Unreadable(_))), "{walked:?}");
    // By ISO's rules it's a page as well, its /Kids said.
    let objects = [
        catalog_object(),
        object(
            2,
            "<< /Type /Page /Kids (x) /Count 3 /MediaBox [0 0 2 200] >>",
        ),
    ];
    let (list, faults) = iso(&objects);
    assert_eq!(list, [(Some(ObjectId::new(2, 0)), 0)]);
    assert!(faults.contains(&Fault::PageTreeType), "{faults:?}");
}

#[test]
fn inheritance_goes_up_parent_not_the_path_walked() {
    let objects = [
        catalog_object(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 21 22] >>",
        ),
        object(3, "<< /Type /Page /Parent 4 0 R >>"),
        object(4, "<< /Type /Pages /MediaBox [0 0 41 42] >>"),
    ];
    for rules in [Rules::Pdfium, Rules::Iso] {
        let read = open_with(&objects, rules, &Limits::DEFAULT).pages.unwrap();
        assert_eq!(read[0].media.value, Rect([0.0, 0.0, 41.0, 42.0]));
        assert_eq!(
            read[0].media.origin,
            Origin::Ancestor(Some(ObjectId::new(4, 0)))
        );
    }
}

#[test]
fn a_parent_is_read_written_in_place_and_through_a_chain() {
    let box_ = "/MediaBox [0 0 21 22]";
    let read = |parent: &str, extra: Vec<(u32, String)>| {
        let mut objects = vec![
            catalog_object(),
            object(
                2,
                &format!("<< /Type /Pages /Kids [3 0 R] /Count 1 {box_} >>"),
            ),
            object(3, &format!("<< /Type /Page /Parent {parent} >>")),
        ];
        objects.extend(extra);
        open_with(&objects, Rules::Pdfium, &Limits::DEFAULT)
            .pages
            .unwrap()
            .remove(0)
    };
    // In place, with no number, and given none.
    let direct = read("<< /MediaBox [0 0 31 32] >>", vec![]);
    assert_eq!(direct.media.value, Rect([0.0, 0.0, 31.0, 32.0]));
    assert_eq!(direct.media.origin, Origin::Ancestor(None));
    assert!(direct.parents.is_empty());
    // And on up from there.
    let up = read("<< /Parent 2 0 R >>", vec![]);
    assert_eq!(up.media.value, Rect([0.0, 0.0, 21.0, 22.0]));
    assert_eq!(up.media.origin, Origin::Ancestor(Some(ObjectId::new(2, 0))));
    assert_eq!(up.parents, [ObjectId::new(2, 0)]);
    // A chain of two, as far as PDFium goes.
    let chained = read("9 0 R", vec![object(9, "2 0 R")]);
    assert_eq!(
        chained.media.origin,
        Origin::Ancestor(Some(ObjectId::new(2, 0)))
    );
    assert_eq!(chained.parents, [ObjectId::new(2, 0)]);
    // Three is too many, and nothing's inherited.
    let long = read(
        "9 0 R",
        vec![
            object(9, "10 0 R"),
            object(10, "11 0 R"),
            object(11, "2 0 R"),
        ],
    );
    assert_eq!(long.media.origin, Origin::Default);
    assert!(long.parents.is_empty());
    // A stream's dictionary is a dictionary to it.
    let stream = read(
        "9 0 R",
        vec![object(
            9,
            "<< /MediaBox [0 0 31 32] /Length 1 >>\nstream\nx\nendstream",
        )],
    );
    assert_eq!(stream.media.value, Rect([0.0, 0.0, 31.0, 32.0]));
}

#[test]
fn a_catalog_s_pages_is_read_written_in_place_and_through_a_chain() {
    let tree = [
        object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        object(3, "<< /Type /Page >>"),
    ];
    let with = |pages: &str, extra: &[(u32, String)]| {
        let mut objects = vec![object(1, &format!("<< /Pages {pages} >>"))];
        objects.extend_from_slice(extra);
        objects.extend_from_slice(&tree);
        open(&objects)
    };
    assert_eq!(with("2 0 R", &[]).unwrap().len(), 1);
    assert_eq!(with("<< /Kids [3 0 R] >>", &[]).unwrap().len(), 1);
    assert_eq!(with("9 0 R", &[object(9, "2 0 R")]).unwrap().len(), 1);
    assert_eq!(
        with("9 0 R", &[object(9, "10 0 R"), object(10, "2 0 R")]),
        Err(Error::NoPages)
    );
    for pages in ["88 0 R", "null", "7"] {
        assert_eq!(with(pages, &[]), Err(Error::NoPages), "{pages}");
    }
}

#[test]
fn a_value_by_reference_is_found_as_each_set_of_rules_finds_it() {
    let read = |value: &str, rules| {
        let objects = [
            catalog_object(),
            object(
                2,
                "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 21 22] /Rotate 90 >>",
            ),
            object(
                3,
                &format!("<< /Type /Page /Parent 2 0 R /MediaBox {value} /Rotate {value} >>"),
            ),
            object(9, "null"),
            object(10, "11 0 R"),
            object(11, "[0 0 31 32]"),
            object(12, "13 0 R"),
            object(13, "180"),
        ];
        open_with(&objects, rules, &Limits::DEFAULT)
            .pages
            .unwrap()
            .remove(0)
    };
    // A reference to nothing isn't found, so the climb goes on, by either.
    for rules in [Rules::Pdfium, Rules::Iso] {
        let page = read("88 0 R", rules);
        assert_eq!(
            page.media.origin,
            Origin::Ancestor(Some(ObjectId::new(2, 0)))
        );
        assert_eq!(
            page.rotate.origin,
            Origin::Ancestor(Some(ObjectId::new(2, 0)))
        );
    }
    // Null stops it by PDFium's, and is absent by ISO's.
    let page = read("9 0 R", Rules::Pdfium);
    assert_eq!(page.media.origin, Origin::Default);
    assert_eq!(page.rotate.origin, Origin::Default);
    let page = read("9 0 R", Rules::Iso);
    assert_eq!(
        page.media.origin,
        Origin::Ancestor(Some(ObjectId::new(2, 0)))
    );
    let page = read("null", Rules::Pdfium);
    assert_eq!(page.media.origin, Origin::Default);
    let page = read("null", Rules::Iso);
    assert_eq!(
        page.media.origin,
        Origin::Ancestor(Some(ObjectId::new(2, 0)))
    );
    // A box through a chain isn't one by PDFium's, and is by ISO's.
    let page = read("10 0 R", Rules::Pdfium);
    assert_eq!(page.media.origin, Origin::Default);
    let page = read("10 0 R", Rules::Iso);
    assert_eq!(page.media.value, Rect([0.0, 0.0, 31.0, 32.0]));
    assert_eq!(page.media.origin, Origin::Own);
    // A /Rotate by a chain is, by either; a longer one by ISO's alone.
    assert_eq!(read("12 0 R", Rules::Pdfium).rotate.value, 180);
    assert_eq!(read("12 0 R", Rules::Iso).rotate.value, 180);

    let long = |rules| {
        let objects = [
            catalog_object(),
            object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            object(3, "<< /Type /Page /Parent 2 0 R /Rotate 9 0 R >>"),
            object(9, "10 0 R"),
            object(10, "11 0 R"),
            object(11, "90"),
        ];
        open_with(&objects, rules, &Limits::DEFAULT)
            .pages
            .unwrap()
            .remove(0)
            .rotate
            .value
    };
    assert_eq!(long(Rules::Pdfium), 0);
    assert_eq!(long(Rules::Iso), 90);

    // A number of a box likewise.
    let number = |rules| {
        let objects = [
            catalog_object(),
            object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            object(
                3,
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 0 R 200] >>",
            ),
            object(9, "10 0 R"),
            object(10, "3"),
        ];
        open_with(&objects, rules, &Limits::DEFAULT)
            .pages
            .unwrap()
            .remove(0)
            .media
            .value
    };
    assert_eq!(number(Rules::Pdfium), Rect([0.0, 0.0, 0.0, 200.0]));
    assert_eq!(number(Rules::Iso), Rect([0.0, 0.0, 3.0, 200.0]));
}

#[test]
fn a_user_unit_is_its_own_and_only_a_positive_number() {
    let read = |unit: &str| {
        let objects = [
            catalog_object(),
            object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 /UserUnit 9 >>"),
            object(3, &format!("<< /Type /Page /Parent 2 0 R {unit} >>")),
        ];
        open(&objects).unwrap().remove(0).unit
    };
    assert_eq!(
        read(""),
        Sourced {
            value: 1.0,
            origin: Origin::Default
        }
    );
    assert_eq!(
        read("/UserUnit 2"),
        Sourced {
            value: 2.0,
            origin: Origin::Own
        }
    );
    assert_eq!(
        read("/UserUnit 2.5"),
        Sourced {
            value: 2.5,
            origin: Origin::Own
        }
    );
    for unit in ["/UserUnit 0", "/UserUnit -2", "/UserUnit /A"] {
        assert_eq!(
            read(unit),
            Sourced {
                value: 1.0,
                origin: Origin::Default
            },
            "{unit}"
        );
    }
}

#[test]
fn a_box_is_a_box_only_as_four_numbers() {
    let read = |value: &str| {
        let objects = [
            catalog_object(),
            object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            object(3, &format!("<< /Type /Page /MediaBox {value} >>")),
            object(9, "3"),
        ];
        open(&objects).unwrap().remove(0).media
    };
    assert_eq!(read("[1 2.5 3 9 0 R]").value, Rect([1.0, 2.5, 3.0, 3.0]));
    // What's no number reads as 0, as PDFium's does.
    assert_eq!(read("[0 (a) null 5]").value, Rect([0.0, 0.0, 0.0, 5.0]));
    for value in ["[0 0 3]", "[0 0 3 4 5]", "[]", "7", "(a)"] {
        let media = read(value);
        assert_eq!(
            media,
            Sourced {
                value: Rect::LETTER,
                origin: Origin::Default
            },
            "{value}"
        );
    }
}

// What doesn't read, and what that does to each walk.

/// A resolver of `objects` that says `why` of `failing`.
fn failing<'o>(
    objects: &'o BTreeMap<ObjectId, Object>,
    failing: ObjectId,
    why: Why,
) -> impl Resolve<'o> + Copy {
    #[derive(Clone, Copy)]
    struct Failing<'o> {
        objects: &'o BTreeMap<ObjectId, Object>,
        failing: ObjectId,
        why: Why,
    }
    impl<'o> Resolve<'o> for Failing<'o> {
        fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
            self.objects.get(&id)
        }

        fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
            if id == self.failing {
                return Err(Unreadable::new(id, self.why, "said"));
            }
            Ok(self.resolve(id))
        }
    }
    Failing {
        objects,
        failing,
        why,
    }
}

fn parsed(objects: &[(u32, &str)]) -> BTreeMap<ObjectId, Object> {
    (objects.iter())
        .map(|(number, text)| {
            let value = Parser::at(text.as_bytes(), 0).value(&mut ()).unwrap();
            (ObjectId::new(*number, 0), value)
        })
        .collect()
}

#[test]
fn an_object_that_doesn_t_read_is_said_or_fails_the_walk_by_why() {
    let objects = parsed(&[
        (
            2,
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /Resources 5 0 R >>",
        ),
        (3, "<< /Type /Page /Parent 2 0 R >>"),
        (4, "<< /Type /Page /Parent 2 0 R >>"),
        (5, "<< >>"),
    ]);
    let catalog = Dict::from_iter([(b"Pages".to_vec(), Object::reference(2))]);
    for rules in [Rules::Pdfium, Rules::Iso] {
        for failed in [
            ObjectId::new(3, 0),
            ObjectId::new(5, 0),
            ObjectId::new(2, 0),
        ] {
            for why in [Why::Capped, Why::Locked, Why::Source] {
                let resolver = failing(&objects, failed, why);
                let walked = pages(resolver, &catalog, rules, &Limits::DEFAULT, &mut ());
                match walked {
                    Err(Error::Unreadable(unreadable)) => {
                        assert_eq!((unreadable.id, unreadable.why), (failed, why));
                    }
                    other => panic!("{failed:?}, {why:?}: {other:?}"),
                }
            }
            // Damaged, it's an object that isn't there, and it's said.
            let resolver = failing(&objects, failed, Why::Damaged);
            let mut kept = Kept::default();
            let walked = pages(resolver, &catalog, rules, &Limits::DEFAULT, &mut kept);
            assert!(
                kept.faults().contains(&Fault::Damaged { id: failed }),
                "{failed:?}: {:?}",
                kept.faults()
            );
            if failed == ObjectId::new(2, 0) {
                assert_eq!(walked, Err(Error::NoPages));
            } else {
                assert!(walked.is_ok(), "{failed:?}");
            }
        }
    }
    // A page damaged takes its place by PDFium's, and has none by ISO's.
    let resolver = failing(&objects, ObjectId::new(3, 0), Why::Damaged);
    let by = |rules| pages(resolver, &catalog, rules, &Limits::DEFAULT, &mut ()).unwrap();
    assert_eq!(by(Rules::Pdfium).len(), 2);
    assert_eq!(by(Rules::Pdfium).get(0).unwrap().dict, None);
    assert_eq!(by(Rules::Iso).len(), 1);
    assert_eq!(by(Rules::Iso).get(0).unwrap().id, Some(ObjectId::new(4, 0)));
}

#[test]
fn a_source_that_fails_fails_the_walk_where_it_s_read() {
    let objects = [
        catalog_object(),
        object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        object(3, "<< /Type /Page >>"),
    ];
    let bytes = file(&objects);
    let source = ByPosition::new(&bytes);
    let reading = Reading::open(&source, &Options::DEFAULT).unwrap();
    let resolver = &reading;
    let catalog = catalog(resolver, reading.trailer().dict())
        .unwrap()
        .unwrap();
    source.failing.store(true, Ordering::Relaxed);
    let walked = pages(resolver, catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ());
    match walked {
        Err(Error::Unreadable(unreadable)) => assert_eq!(unreadable.why, Why::Source),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_chain_of_references_too_long_fails_iso_s_walk() {
    let mut objects = vec![object(1, "<< /Pages 2 0 R >>")];
    for n in 2..40 {
        objects.push(object(n, &format!("{} 0 R", n + 1)));
    }
    objects.push(object(40, "<< /Type /Pages /Kids [] >>"));
    let walked = open_with(&objects, Rules::Iso, &Limits::DEFAULT).pages;
    match walked {
        Err(Error::Unreadable(unreadable)) => assert_eq!(unreadable.why, Why::Capped),
        other => panic!("{other:?}"),
    }
    assert_eq!(tree_error(&objects), Error::NoPages);
}

// ISO's rules.

/// A well-formed tree, deep and with what it inherits.
fn well_formed() -> Vec<(u32, String)> {
    vec![
        catalog_object(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 3 /MediaBox [0 0 612 792] /Resources << >> >>",
        ),
        object(
            3,
            "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R 6 0 R] /Count 2 /Rotate 90 >>",
        ),
        object(4, "<< /Type /Page /Parent 2 0 R /CropBox [0 0 10 10] >>"),
        object(5, "<< /Type /Page /Parent 3 0 R >>"),
        object(6, "<< /Type /Page /Parent 3 0 R /MediaBox 7 0 R >>"),
        object(7, "[0 0 100 100]"),
    ]
}

#[test]
fn on_a_well_formed_tree_the_two_give_the_same_and_iso_s_says_nothing() {
    let by_pdfium = open_with(&well_formed(), Rules::Pdfium, &Limits::DEFAULT);
    let by_iso = open_with(&well_formed(), Rules::Iso, &Limits::DEFAULT);
    assert_eq!(by_iso.faults, []);
    assert_eq!(by_pdfium.faults, []);
    assert_eq!(by_iso.pages, by_pdfium.pages);
    assert_eq!(by_iso.counted, 3);
    assert_eq!(by_pdfium.counted, 3);
    let pages = by_iso.pages.unwrap();
    assert_eq!(
        ids(&pages),
        [
            (Some(ObjectId::new(5, 0)), 0),
            (Some(ObjectId::new(6, 0)), 0),
            (Some(ObjectId::new(4, 0)), 0)
        ]
    );
}

/// `well_formed()` with object `number` written as `body`, by ISO's rules.
fn iso_with(number: u32, body: &str) -> (Vec<(Option<ObjectId>, u32)>, Vec<Fault>) {
    let mut objects = well_formed();
    match objects.iter_mut().find(|(n, _)| *n == number) {
        Some(found) => found.1 = body.to_string(),
        None => objects.push(object(number, body)),
    }
    iso(&objects)
}

const FIVE: (Option<ObjectId>, u32) = (Some(ObjectId::new(5, 0)), 0);
const SIX: (Option<ObjectId>, u32) = (Some(ObjectId::new(6, 0)), 0);
const FOUR: (Option<ObjectId>, u32) = (Some(ObjectId::new(4, 0)), 0);

#[test]
fn by_iso_s_rules_a_kid_that_s_no_dictionary_has_no_place_and_is_said() {
    for kid in ["88 0 R", "null", "9 0 R", "7"] {
        let node = format!("<< /Type /Pages /Parent 2 0 R /Kids [5 0 R {kid} 6 0 R] /Count 2 >>");
        let (list, faults) = iso_with(3, &node);
        assert_eq!(list, [FIVE, SIX, FOUR], "{kid}");
        assert!(
            faults.contains(&Fault::PageTreeKidNotDictionary),
            "{kid}: {faults:?}"
        );
        // A null isn't a dictionary written in place, as an integer is no
        // dictionary either: it's that it's written in place that's said.
        assert_eq!(
            faults.contains(&Fault::PageTreeKidDirect),
            kid == "7",
            "{kid}: {faults:?}"
        );
    }
    // A stream's dictionary is read all the same, and said.
    let (list, faults) = iso_with(
        5,
        "<< /Type /Page /Parent 3 0 R /Length 1 >>\nstream\nx\nendstream",
    );
    assert_eq!(list, [FIVE, SIX, FOUR]);
    assert_eq!(faults, [Fault::PageTreeKidNotDictionary]);
}

#[test]
fn by_iso_s_rules_a_kid_in_place_is_read_and_said() {
    let node = "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R << /Type /Page >> 6 0 R] /Count 3 >>";
    let (list, faults) = iso_with(3, node);
    assert_eq!(list, [FIVE, (None, 0), SIX, FOUR]);
    assert!(faults.contains(&Fault::PageTreeKidDirect), "{faults:?}");
    // The kid in place has no /Parent, and the count of the root is off by it.
    let lister = Some(ObjectId::new(3, 0));
    assert!(faults.contains(&Fault::PageTreeParent { lister }));
    let count = Some(3);
    assert!(faults.contains(&Fault::PageTreeCount { count, under: 4 }));
}

#[test]
fn by_iso_s_rules_type_tells_a_node_from_a_page() {
    // /Page with /Kids is a page.
    let (list, faults) = iso_with(5, "<< /Type /Page /Parent 3 0 R /Kids [6 0 R] >>");
    assert_eq!(list, [FIVE, SIX, FOUR]);
    assert!(faults.contains(&Fault::PageTreeType));
    // /Pages without /Kids is a node with none, where PDFium has a place.
    let (list, faults) = iso_with(5, "<< /Type /Pages /Parent 3 0 R >>");
    assert_eq!(list, [SIX, FOUR]);
    assert!(faults.contains(&Fault::PageTreeNoKids), "{faults:?}");
    // A null /Kids is none, so where /Type says neither, it's a page.
    let (list, faults) = iso_with(5, "<< /Type /Foo /Parent 3 0 R /Kids null >>");
    assert_eq!(list, [FIVE, SIX, FOUR]);
    assert_eq!(faults, [Fault::PageTreeType]);
    // Neither, and /Kids tells.
    for kind in ["", "/Type /Foo", "/Type (Pages)"] {
        let (list, faults) = iso_with(5, &format!("<< {kind} /Parent 3 0 R >>"));
        assert_eq!(list, [FIVE, SIX, FOUR], "{kind}");
        assert!(faults.contains(&Fault::PageTreeType), "{kind}");
        let (list, faults) = iso_with(5, &format!("<< {kind} /Parent 3 0 R /Kids [7 0 R] >>"));
        assert!(!list.contains(&FIVE), "{kind}");
        assert!(faults.contains(&Fault::PageTreeType), "{kind}");
    }
    // A /Type through a reference is read as what it is.
    let mut objects = well_formed();
    objects[4].1 = "<< /Type 8 0 R /Parent 3 0 R >>".to_string();
    objects.push(object(8, "/Page"));
    assert_eq!(iso(&objects), (vec![FIVE, SIX, FOUR], vec![]));
}

#[test]
fn by_iso_s_rules_each_departure_of_a_node_is_said() {
    let said = |number, body: &str, fault: Fault| {
        let (_, faults) = iso_with(number, body);
        assert!(faults.contains(&fault), "{body}: {faults:?}");
    };
    // No kids, or none that's an array, or empty.
    let kids = "/Type /Pages /Parent 2 0 R /Count 0";
    said(3, &format!("<< {kids} >>"), Fault::PageTreeNoKids);
    said(3, &format!("<< {kids} /Kids [] >>"), Fault::PageTreeNoKids);
    said(3, &format!("<< {kids} /Kids 7 >>"), Fault::PageTreeNoKids);
    said(
        3,
        &format!("<< {kids} /Kids null >>"),
        Fault::PageTreeNoKids,
    );
    // A /Count that isn't what's under it, at any node, as written where it's
    // an integer.
    let node = "/Type /Pages /Parent 2 0 R /Kids [5 0 R 6 0 R]";
    let counts = [
        ("/Count 1", Some(1)),
        ("/Count 3", Some(3)),
        ("", None),
        ("/Count 2.0", None),
        ("/Count (2)", None),
    ];
    for (written, count) in counts {
        let fault = Fault::PageTreeCount { count, under: 2 };
        said(3, &format!("<< {node} {written} >>"), fault);
    }
    // One right by a reference is right.
    let mut objects = well_formed();
    objects[2].1 = format!("<< {node} /Count 8 0 R >>");
    objects.push(object(8, "2"));
    assert_eq!(iso(&objects).1, []);
    // A /Parent that isn't the holder, or isn't there, of a node or a page,
    // each the holder's.
    let kids = "/Type /Pages /Kids [5 0 R 6 0 R] /Count 2";
    let (root, node) = (Some(ObjectId::new(2, 0)), Some(ObjectId::new(3, 0)));
    said(
        3,
        &format!("<< {kids} >>"),
        Fault::PageTreeParent { lister: root },
    );
    said(
        3,
        &format!("<< {kids} /Parent 4 0 R >>"),
        Fault::PageTreeParent { lister: root },
    );
    said(
        5,
        "<< /Type /Page >>",
        Fault::PageTreeParent { lister: node },
    );
    said(
        5,
        "<< /Type /Page /Parent 2 0 R >>",
        Fault::PageTreeParent { lister: node },
    );
    // /Parent written in place.
    said(
        5,
        "<< /Type /Page /Parent << /Parent 2 0 R >> >>",
        Fault::PageTreeParentDirect,
    );
    // A /Parent loop: an error by PDFium's, and here it's said, and ends there.
    let (list, faults) = iso_with(5, "<< /Type /Page /Parent 5 0 R >>");
    assert_eq!(list, [FIVE, SIX, FOUR]);
    assert!(faults.contains(&Fault::PageTreeParentLoop));
    // A page with no /MediaBox: nothing above gives one.
    let objects = [
        catalog_object(),
        object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        object(3, "<< /Type /Page /Parent 2 0 R >>"),
    ];
    let (_, faults) = iso(&objects);
    assert_eq!(faults, [Fault::PageNoMediaBox]);
}

#[test]
fn by_iso_s_rules_a_node_or_a_page_listed_twice_is_said_and_listed_again() {
    let (list, faults) = iso_with(
        3,
        "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R 6 0 R 5 0 R] /Count 3 >>",
    );
    assert_eq!(list, [FIVE, SIX, (Some(ObjectId::new(5, 0)), 1), FOUR]);
    assert!(faults.contains(&Fault::PageTreeRepeated {
        id: ObjectId::new(5, 0)
    }));
    let (list, faults) = iso_with(
        2,
        "<< /Type /Pages /Kids [3 0 R 3 0 R 4 0 R] /Count 5 /MediaBox [0 0 1 1] >>",
    );
    assert_eq!(
        list,
        [
            FIVE,
            SIX,
            (Some(ObjectId::new(5, 0)), 1),
            (Some(ObjectId::new(6, 0)), 1),
            FOUR
        ]
    );
    assert!(faults.contains(&Fault::PageTreeRepeated {
        id: ObjectId::new(3, 0)
    }));
}

#[test]
fn by_iso_s_rules_a_cycle_is_said_and_not_walked_again() {
    let (list, faults) = iso_with(
        3,
        "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R 3 0 R 2 0 R 6 0 R] /Count 2 >>",
    );
    assert_eq!(list, [FIVE, SIX, FOUR]);
    assert!(faults.contains(&Fault::PageTreeCycle {
        id: ObjectId::new(3, 0)
    }));
    assert!(faults.contains(&Fault::PageTreeCycle {
        id: ObjectId::new(2, 0)
    }));
}

#[test]
fn by_iso_s_rules_the_count_doesn_t_end_the_table() {
    let objects = [
        catalog_object(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 1 /MediaBox [0 0 1 1] >>",
        ),
        object(3, "<< /Type /Page /Parent 2 0 R >>"),
        object(4, "<< /Type /Page /Parent 2 0 R >>"),
        object(5, "<< /Type /Page /Parent 2 0 R >>"),
    ];
    let (list, faults) = iso(&objects);
    assert_eq!(list.len(), 3);
    let count = Some(1);
    assert_eq!(faults, [Fault::PageTreeCount { count, under: 3 }]);
    assert_eq!(open(&objects).unwrap().len(), 1);
    let table = open_with(&objects, Rules::Iso, &Limits::DEFAULT);
    assert_eq!(table.counted, 3);
}

#[test]
fn by_iso_s_rules_a_node_s_count_is_of_what_s_under_it_alone() {
    // The node's one page comes after the one before it, which isn't under it.
    let objects = [
        catalog_object(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 1 1] >>",
        ),
        object(3, "<< /Type /Page /Parent 2 0 R >>"),
        object(4, "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R] /Count 1 >>"),
        object(5, "<< /Type /Page /Parent 4 0 R >>"),
    ];
    assert_eq!(iso(&objects).1, []);
}

#[test]
fn by_iso_s_rules_a_count_and_a_parent_say_what_the_walk_found() {
    // A node's /Count as written where it's an integer, and the pages under
    // it, one page said as one.
    let cases = [
        (
            "/Count 1",
            "5 0 R 6 0 R",
            Some(1),
            2,
            "is 1, with 2 pages under it",
        ),
        (
            "/Count 3",
            "5 0 R",
            Some(3),
            1,
            "is 3, with 1 page under it",
        ),
        (
            "/Count 8 0 R",
            "5 0 R",
            Some(0),
            1,
            "is 0, with 1 page under it",
        ),
        (
            "/Count (1)",
            "5 0 R",
            None,
            1,
            "isn't a number of pages: 1 is under it",
        ),
        (
            "",
            "5 0 R 6 0 R",
            None,
            2,
            "isn't a number of pages: 2 are under it",
        ),
    ];
    for (written, kids, count, under, said) in cases {
        let mut objects = well_formed();
        objects[2].1 = format!("<< /Type /Pages /Parent 2 0 R /Kids [{kids}] {written} >>");
        objects.push(object(8, "0"));
        let (_, faults) = iso(&objects);
        let fault = Fault::PageTreeCount { count, under };
        assert!(faults.contains(&fault), "{written}: {faults:?}");
        assert_eq!(fault.to_string(), format!("a node's /Count {said}"));
    }
    // An empty tree's root, which counts none.
    let objects = [
        catalog_object(),
        object(2, "<< /Type /Pages /Kids [] /Count 0 >>"),
    ];
    let empty = Fault::PageTreeCount {
        count: Some(0),
        under: 0,
    };
    assert!(iso(&objects).1.contains(&empty));
    assert_eq!(
        empty.to_string(),
        "a node's /Count is 0, with no pages under it, where a node has at least one"
    );
    let wrong = Fault::PageTreeCount {
        count: Some(5),
        under: 0,
    };
    assert_eq!(
        wrong.to_string(),
        "a node's /Count is 5, with 0 pages under it"
    );

    // A /Parent that isn't the node that lists it, which has a number.
    let (_, faults) = iso_with(3, "<< /Type /Pages /Kids [5 0 R 6 0 R] /Count 2 >>");
    let numbered = Fault::PageTreeParent {
        lister: Some(ObjectId::new(2, 0)),
    };
    assert_eq!(
        faults.iter().filter(|fault| **fault == numbered).count(),
        1,
        "{faults:?}"
    );
    assert_eq!(
        numbered.to_string(),
        "a /Parent isn't 2 0 R, the node that lists it"
    );
    // Under a node written in place, a kid or the root, no /Parent can name
    // it: the page's says so, and the kid's own names the root.
    let in_place = Fault::PageTreeParent { lister: None };
    let objects = [
        catalog_object(),
        object(
            2,
            "<< /Type /Pages /Kids [<< /Type /Pages /Kids [3 0 R] /Count 1 >>] /Count 1 /MediaBox [0 0 1 1] >>",
        ),
        object(3, "<< /Type /Page /Parent 2 0 R >>"),
    ];
    let (list, faults) = iso(&objects);
    assert_eq!(list, [(Some(ObjectId::new(3, 0)), 0)]);
    assert!(faults.contains(&in_place), "{faults:?}");
    assert!(faults.contains(&numbered), "{faults:?}");
    let objects = [
        object(
            1,
            "<< /Type /Catalog /Pages << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 1 1] >> >>",
        ),
        object(3, "<< /Type /Page /Parent 2 0 R >>"),
    ];
    let (_, faults) = iso(&objects);
    assert!(faults.contains(&in_place), "{faults:?}");
    assert_eq!(
        in_place.to_string(),
        "a /Parent isn't the node that lists it, which is written in place"
    );
}

#[test]
fn by_iso_s_rules_a_root_s_departures_are_said() {
    // No pages under it is an empty table, said.
    let objects = [
        catalog_object(),
        object(2, "<< /Type /Pages /Kids [] /Count 0 >>"),
    ];
    let (list, faults) = iso(&objects);
    assert_eq!(list, []);
    assert!(faults.contains(&Fault::PageTreeEmpty));
    assert!(faults.contains(&Fault::PageTreeNoKids));
    // But a catalog with none is no table at all, by either.
    let objects = [object(1, "<< /Type /Catalog >>")];
    assert_eq!(
        open_with(&objects, Rules::Iso, &Limits::DEFAULT).pages,
        Err(Error::NoPages)
    );
    // Written in place, a /Parent on it, a page by its /Type.
    let objects = [object(
        1,
        "<< /Pages << /Type /Pages /Kids [] /Parent 1 0 R >> >>",
    )];
    let (_, faults) = iso(&objects);
    assert!(faults.contains(&Fault::PageTreeRootDirect), "{faults:?}");
    assert!(faults.contains(&Fault::PageTreeRootParent), "{faults:?}");
    let objects = [
        catalog_object(),
        object(2, "<< /Type /Page /MediaBox [0 0 1 1] >>"),
    ];
    let (list, faults) = iso(&objects);
    assert_eq!(list, [(Some(ObjectId::new(2, 0)), 0)]);
    assert_eq!(faults, [Fault::PageTreeType]);
}

#[test]
fn by_iso_s_rules_a_chain_is_followed_and_said() {
    let (list, faults) = iso_with(
        3,
        "<< /Type /Pages /Parent 2 0 R /Kids [8 0 R 6 0 R] /Count 2 >>",
    );
    assert_eq!(list, [SIX, FOUR]);
    assert!(faults.contains(&Fault::PageTreeKidNotDictionary));
    let mut objects = well_formed();
    objects.push(object(8, "5 0 R"));
    objects[2].1 = "<< /Type /Pages /Parent 2 0 R /Kids [8 0 R 6 0 R] /Count 2 >>".to_string();
    let (list, faults) = iso(&objects);
    // Its number the object's own.
    assert_eq!(list, [FIVE, SIX, FOUR]);
    assert_eq!(faults, [Fault::PageTreeChain]);
    // And PDFium's has a place there with no page.
    let read = open(&objects).unwrap();
    assert_eq!(read[0].id, Some(ObjectId::new(8, 0)));
    assert!(!read[0].loads);
}

#[test]
fn only_a_sink_that_wants_them_has_its_checks_run() {
    let objects = parsed(&[
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 8 0 R >>"),
        (3, "<< /Type /Page /Parent 2 0 R >>"),
        (8, "1"),
    ]);
    let catalog = Dict::from_iter([(b"Pages".to_vec(), Object::reference(2))]);
    let asked = std::cell::RefCell::new(Vec::new());
    let resolve = |id: ObjectId| {
        asked.borrow_mut().push(id);
        objects.get(&id)
    };
    pages(resolve, &catalog, Rules::Iso, &Limits::DEFAULT, &mut ()).unwrap();
    // `()` doesn't want the /Count held to anything.
    assert!(
        !asked.borrow().contains(&ObjectId::new(8, 0)),
        "{:?}",
        asked.borrow()
    );
    asked.borrow_mut().clear();
    let mut kept = Kept::default();
    pages(resolve, &catalog, Rules::Iso, &Limits::DEFAULT, &mut kept).unwrap();
    assert!(asked.borrow().contains(&ObjectId::new(8, 0)));
    assert_eq!(kept.faults(), [Fault::PageNoMediaBox]);
}

#[test]
fn a_walk_reads_nothing_past_the_count_or_any_content() {
    let objects = parsed(&[
        (2, "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 1 >>"),
        (3, "<< /Type /Page /Contents 5 0 R /Annots 6 0 R >>"),
        (4, "<< /Type /Page >>"),
    ]);
    let catalog = Dict::from_iter([(b"Pages".to_vec(), Object::reference(2))]);
    let asked = std::cell::RefCell::new(Vec::new());
    let resolve = |id: ObjectId| {
        asked.borrow_mut().push(id);
        objects.get(&id)
    };
    pages(resolve, &catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ()).unwrap();
    assert_eq!(*asked.borrow(), [ObjectId::new(2, 0), ObjectId::new(3, 0)]);
}

#[test]
fn an_error_s_words_are_the_whole_message() {
    assert_eq!(
        Error::Cycle.to_string(),
        "the page tree lists a node inside itself"
    );
    assert_eq!(
        Error::ParentLoop.to_string(),
        "a page's /Parent chain loops"
    );
    assert_eq!(Error::TooDeep.to_string(), "the page tree is too deep");
    assert_eq!(
        Error::TooManyPages.to_string(),
        "the page tree lists too many pages"
    );
    assert_eq!(Error::NoPages.to_string(), "the file has no pages");
    let unreadable = Unreadable::new(ObjectId::new(3, 0), Why::Locked, "takes a password");
    let error = Error::from(unreadable);
    assert_eq!(error.to_string(), "takes a password");
    assert!(std::error::Error::source(&error).is_none());
}

#[test]
fn a_fault_says_where_it_is() {
    let objects = [
        catalog_object(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 1 1] >>",
        ),
        object(3, "<< /Type /Pages /Parent 2 0 R /Kids [null] /Count 0 >>"),
    ];
    let bytes = file(&objects);
    let document = read::document(&bytes, &read::Options::DEFAULT);
    let resolver = &document;
    let catalog = document.catalog().unwrap();
    let mut kept = Kept::default();
    pages(resolver, catalog, Rules::Iso, &Limits::DEFAULT, &mut kept).unwrap();
    let (place, _) = (kept.0.iter())
        .find(|(_, fault)| *fault == Fault::PageTreeKidNotDictionary)
        .unwrap();
    assert_eq!(place.object, Some(ObjectId::new(3, 0)));
    assert!(place.at.is_some());

    // Back at the node that lists it, the fault's in that node.
    let objects = [
        catalog_object(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R null] /Count 1 /MediaBox [0 0 1 1] >>",
        ),
        object(
            3,
            "<< /Type /Pages /Parent 2 0 R /Kids [4 0 R null] /Count 1 >>",
        ),
        object(4, "<< /Type /Page /Parent 3 0 R >>"),
    ];
    let bytes = file(&objects);
    let document = read::document(&bytes, &read::Options::DEFAULT);
    let resolver = &document;
    let catalog = document.catalog().unwrap();
    let mut kept = Kept::default();
    pages(resolver, catalog, Rules::Iso, &Limits::DEFAULT, &mut kept).unwrap();
    let places: Vec<Option<ObjectId>> = (kept.0.iter())
        .filter(|(_, fault)| *fault == Fault::PageTreeKidNotDictionary)
        .map(|(place, _)| place.object)
        .collect();
    assert_eq!(
        places,
        [Some(ObjectId::new(3, 0)), Some(ObjectId::new(2, 0))]
    );

    // What's said of a kid's own dictionary is in the kid's object, and of the
    // reference to it in the node that lists it.
    let objects = [
        catalog_object(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 5 0 R 2 0 R] /Count 4 /MediaBox [0 0 1 1] >>",
        ),
        object(3, "<< /Parent 2 0 R >>"),
        object(4, "<< /Type /Foo /Parent 2 0 R >>"),
        object(
            5,
            "<< /Type /Page /Parent 2 0 R /Length 0 >>\nstream\n\nendstream",
        ),
    ];
    let bytes = file(&objects);
    let document = read::document(&bytes, &read::Options::DEFAULT);
    let resolver = &document;
    let catalog = document.catalog().unwrap();
    let mut kept = Kept::default();
    pages(resolver, catalog, Rules::Iso, &Limits::DEFAULT, &mut kept).unwrap();
    let of = |wanted: fn(&Fault) -> bool| -> Vec<Option<ObjectId>> {
        (kept.0.iter())
            .filter(|(_, fault)| wanted(fault))
            .map(|(place, _)| place.object)
            .collect()
    };
    assert_eq!(
        of(|fault| *fault == Fault::PageTreeType),
        [Some(ObjectId::new(3, 0)), Some(ObjectId::new(4, 0))]
    );
    assert_eq!(
        of(|fault| *fault == Fault::PageTreeKidNotDictionary),
        [Some(ObjectId::new(5, 0))]
    );
    assert_eq!(
        of(|fault| matches!(fault, Fault::PageTreeRepeated { .. })),
        [Some(ObjectId::new(2, 0))]
    );
    assert_eq!(
        of(|fault| matches!(fault, Fault::PageTreeCycle { .. })),
        [Some(ObjectId::new(2, 0))]
    );

    // Each said of the reference to a kid is in the node that lists it, whatever
    // was said of the kid before it.
    let objects = [
        catalog_object(),
        object(
            2,
            "<< /Type /Pages /Kids [3 0 R 4 0 R 4 0 R] /Count 1 /MediaBox [0 0 1 1] >>",
        ),
        object(3, "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R] /Count 0 >>"),
        object(4, "<< /Type /Pages /Parent 2 0 R /Kids [6 0 R] /Count 1 >>"),
        object(5, "<< /Type /Pages /Parent 3 0 R /Kids [3 0 R] /Count 0 >>"),
        object(6, "<< /Type /Page /Parent 4 0 R >>"),
    ];
    let bytes = file(&objects);
    let document = read::document(&bytes, &read::Options::DEFAULT);
    let resolver = &document;
    let catalog = document.catalog().unwrap();
    let mut kept = Kept::default();
    pages(resolver, catalog, Rules::Iso, &Limits::DEFAULT, &mut kept).unwrap();
    let of = |wanted: fn(&Fault) -> bool| -> Vec<Option<ObjectId>> {
        (kept.0.iter())
            .filter(|(_, fault)| wanted(fault))
            .map(|(place, _)| place.object)
            .collect()
    };
    assert_eq!(
        of(|fault| matches!(fault, Fault::PageTreeCycle { .. })),
        [Some(ObjectId::new(5, 0))]
    );
    assert_eq!(
        of(|fault| *fault
            == Fault::PageTreeRepeated {
                id: ObjectId::new(4, 0)
            }),
        [Some(ObjectId::new(2, 0))]
    );

    // And what's said on the way up a /Parent chain, in the node that has it.
    let objects = [
        catalog_object(),
        object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        object(3, "<< /Type /Page /Parent 4 0 R >>"),
        object(4, "<< /Type /Pages /Parent << /Type /Pages >> >>"),
    ];
    let bytes = file(&objects);
    let document = read::document(&bytes, &read::Options::DEFAULT);
    let resolver = &document;
    let catalog = document.catalog().unwrap();
    let mut kept = Kept::default();
    pages(resolver, catalog, Rules::Iso, &Limits::DEFAULT, &mut kept).unwrap();
    let (place, _) = (kept.0.iter())
        .find(|(_, fault)| *fault == Fault::PageTreeParentDirect)
        .unwrap();
    assert_eq!(place.object, Some(ObjectId::new(4, 0)));
}

#[test]
fn what_only_a_sink_reads_fails_no_walk() {
    // A node's /Count and its own /Parent: read only for a sink that wants
    // faults, so nothing that fails there can fail a walk, whatever its why.
    // What doesn't read is the count or the parent said; past a cap or a
    // password, as a loop of references is past one, it isn't checked.
    let by_chain = [(7, "7 0 R")];
    let shapes = [
        (
            "<< /Type /Pages /Kids [3 0 R] /Count 7 0 R >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] >>",
            None,
            Fault::PageTreeCount {
                count: None,
                under: 1,
            },
        ),
        (
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Pages /Kids [4 0 R] /Count 1 /Parent 7 0 R >>",
            Some("<< /Type /Page /MediaBox [0 0 1 1] >>"),
            Fault::PageTreeParent {
                lister: Some(ObjectId::new(2, 0)),
            },
        ),
        (
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Pages /Kids [4 0 R] /Count 7 0 R /Parent 2 0 R >>",
            Some("<< /Type /Page /Parent 3 0 R /MediaBox [0 0 1 1] >>"),
            Fault::PageTreeCount {
                count: None,
                under: 1,
            },
        ),
    ];
    let catalog = Dict::from_iter([(b"Pages".to_vec(), Object::reference(2))]);
    for (root, kid, below, fault) in shapes {
        let mut written = vec![(2, root), (3, kid)];
        written.extend(below.map(|below| (4, below)));
        written.extend(by_chain);
        let objects = parsed(&written);
        let all = [
            None,
            Some(Why::Damaged),
            Some(Why::Capped),
            Some(Why::Locked),
            Some(Why::Source),
        ];
        for why in all {
            let failed = why.unwrap_or(Why::Damaged);
            // None: 7 is the loop of references it's written as.
            let resolver = failing(
                &objects,
                if why.is_some() {
                    ObjectId::new(7, 0)
                } else {
                    ObjectId::new(0, 0)
                },
                failed,
            );
            let quiet = pages(resolver, &catalog, Rules::Iso, &Limits::DEFAULT, &mut ());
            let mut kept = Kept::default();
            let told = pages(resolver, &catalog, Rules::Iso, &Limits::DEFAULT, &mut kept);
            assert_eq!(told, quiet, "{root} {kid} {why:?}");
            assert_eq!(told.unwrap().len(), 1, "{root} {kid} {why:?}");
            let faults = kept.faults();
            if why == Some(Why::Damaged) {
                assert!(faults.contains(&fault), "{faults:?}");
                continue;
            }
            let why = why.unwrap_or(Why::Capped);
            let unchecked = Fault::Unchecked {
                id: ObjectId::new(7, 0),
                why,
            };
            assert!(faults.contains(&unchecked), "{root} {kid}: {faults:?}");
            assert!(!faults.contains(&fault), "{root} {kid}: {faults:?}");
        }
    }
}

#[test]
fn kids_that_give_nothing_are_charged_to_the_walk() {
    let nulls = |count: usize| "null ".repeat(count);
    let tree = |count: usize| {
        [
            catalog_object(),
            object(
                2,
                &format!("<< /Type /Pages /Kids [{}3 0 R] >>", nulls(count)),
            ),
            object(3, "<< /Type /Page /Parent 2 0 R >>"),
        ]
    };
    let mut limits = Limits::DEFAULT;
    limits.pages = 2;
    // Four to a page: eight kids are read, and a ninth isn't. By PDFium's rules
    // the count of the tree meets it, as the walk would stop at what's counted.
    let read = open_with(&tree(7), Rules::Iso, &limits).pages.unwrap();
    assert_eq!(ids(&read), [(Some(ObjectId::new(3, 0)), 0)]);
    assert!(open_with(&tree(7), Rules::Pdfium, &limits).pages.is_ok());
    for rules in [Rules::Iso, Rules::Pdfium] {
        assert_eq!(
            open_with(&tree(8), rules, &limits).pages,
            Err(Error::TooManyPages),
            "{rules:?}"
        );
    }

    // One node of a great many nulls, listed a great many times: a cost that's
    // the square of a file, under the default caps, and by no count a page's.
    let (kids, lists) = (30_000, 30_000);
    let objects = [
        catalog_object(),
        object(
            2,
            &format!("<< /Type /Pages /Kids [{}4 0 R] >>", "3 0 R ".repeat(lists)),
        ),
        object(3, &format!("<< /Type /Pages /Kids [{}] >>", nulls(kids))),
        object(4, "<< /Type /Page /Parent 2 0 R >>"),
    ];
    let bytes = file(&objects);
    let document = read::document(&bytes, &read::Options::DEFAULT);
    let resolver = &document;
    let catalog = document.catalog().unwrap();
    let started = Instant::now();
    assert_eq!(
        pages(resolver, catalog, Rules::Iso, &Limits::DEFAULT, &mut ()),
        Err(Error::TooManyPages)
    );
    assert_eq!(
        pages(resolver, catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ()),
        Err(Error::TooManyPages)
    );
    assert!(started.elapsed() < QUICK, "{:?}", started.elapsed());
}

/// A chain far past what the stack could go down, on a page's `/Parent` under a
/// cap raised for it: a table holds it, compares it, shows it and drops it.
#[test]
fn a_parent_chain_raised_past_the_stack_goes_along_in_a_loop() {
    let length = 300_000;
    let mut objects: BTreeMap<ObjectId, Object> = (0..length)
        .map(|n| {
            let parent = Object::reference(n + 11);
            let node = Object::from(pdf_objects::dict! { "Parent" => parent });
            (ObjectId::new(n + 10, 0), node)
        })
        .collect();
    objects.insert(
        ObjectId::new(2, 0),
        parsed(&[(2, "<< /Kids [3 0 R] /Count 1 >>")])[&ObjectId::new(2, 0)].clone(),
    );
    objects.insert(
        ObjectId::new(3, 0),
        Object::from(pdf_objects::dict! { "Type" => "Page", "Parent" => Object::reference(10) }),
    );
    let resolve = |id: ObjectId| objects.get(&id);
    let catalog = Dict::from_iter([(b"Pages".to_vec(), Object::reference(2))]);
    let mut limits = Limits::DEFAULT;
    limits.page_depth = 2 * length as usize;
    let table = pages(resolve, &catalog, Rules::Pdfium, &limits, &mut ()).unwrap();
    let again = pages(resolve, &catalog, Rules::Pdfium, &limits, &mut ()).unwrap();
    let page = table.get(0).unwrap();
    assert_eq!(page.parents().count(), length as usize);
    assert_eq!(table, again);
    assert!(format!("{table:?}").len() > length as usize);
}

#[test]
fn a_parent_chain_is_compared_and_shown_by_what_it_names() {
    let chain = |ids: &[Option<ObjectId>]| {
        let mut up = None;
        for id in ids.iter().rev() {
            up = Some(Arc::new(Parent { id: *id, up }));
        }
        up.unwrap()
    };
    let (a, b) = (Some(ObjectId::new(1, 0)), Some(ObjectId::new(2, 0)));
    assert_eq!(chain(&[a, b]), chain(&[a, b]));
    assert_eq!(chain(&[None, b]), chain(&[None, b]));
    assert_ne!(chain(&[a, b]), chain(&[a, a]));
    assert_ne!(chain(&[a, b]), chain(&[a]));
    assert_ne!(chain(&[a]), chain(&[a, b]));
    assert_ne!(chain(&[None]), chain(&[a]));
    assert_eq!(
        format!("{:?}", chain(&[a, None, b])),
        "[Some(ObjectId { number: 1, generation: 0 }), None, Some(ObjectId { number: 2, generation: 0 })]"
    );
}

#[test]
fn the_default_caps_are_pdfium_s() {
    assert_eq!(Limits::DEFAULT.page_depth, 1024);
    assert_eq!(Limits::DEFAULT.pages, 1_048_574);
}

/// `n` pages, numbered from 1.
fn numbered(n: u32) -> Vec<ObjectId> {
    (1..=n).map(|n| ObjectId::new(n, 0)).collect()
}

/// Ids from `first` on, one a call, and how many were asked for.
fn ids_from(first: u32, asked: &std::cell::Cell<usize>) -> impl FnMut() -> ObjectId + '_ {
    let mut next = first;
    move || {
        asked.set(asked.get() + 1);
        next += 1;
        ObjectId::new(next - 1, 0)
    }
}

/// `pages` written under `fanout` and the default caps, the nodes numbered
/// past the pages.
fn laid(pages: &[ObjectId], fanout: usize) -> Written {
    let asked = std::cell::Cell::new(0);
    let past = pages.iter().map(|id| id.number).max().unwrap_or(0) + 1;
    let written = write(pages, fanout, &Limits::DEFAULT, ids_from(past, &asked)).unwrap();
    assert_eq!(asked.get(), written.nodes.len(), "an id a node");
    written
}

/// Each page's depth, walking down from the root, and every node's kids
/// within the fanout, its count its pages', and its parent the node that
/// lists it; the walk meets the nodes in their order, and the pages in
/// theirs. Each node is `/Type /Pages`, `/Parent` but the root, `/Kids` and
/// `/Count`, in that order, each kid by reference.
fn walk(written: &Written, pages: &[ObjectId], fanout: usize) -> Vec<usize> {
    fn visit(
        written: &Written,
        place: &HashMap<ObjectId, usize>,
        fanout: usize,
        n: usize,
        (parent, d): (Option<ObjectId>, usize),
        seen: &mut Vec<usize>,
        order: &mut Vec<(ObjectId, usize, ObjectId)>,
    ) -> i64 {
        // A walk deeper than the nodes are many is in a loop of them.
        assert!(d <= written.nodes.len(), "node {n} at depth {d}");
        seen.push(n);
        let (id, node) = &written.nodes[n];
        let keys: Vec<&[u8]> = node.iter().map(|(key, _)| key).collect();
        let expected: &[&[u8]] = match parent {
            Some(_) => &[b"Type", b"Parent", b"Kids", b"Count"],
            None => &[b"Type", b"Kids", b"Count"],
        };
        assert_eq!(keys, expected, "node {n}");
        assert!(node.has_type(b"Pages"), "node {n}");
        let up = node.get(b"Parent").map(|up| up.as_reference().unwrap());
        assert_eq!(up, parent, "node {n}");
        let kids = node.get(b"Kids").and_then(Object::as_array).unwrap();
        assert!(!kids.is_empty() && kids.len() <= fanout, "node {n}");
        let mut count = 0;
        for kid in kids {
            let kid = kid.as_reference().expect("a kid by reference");
            count += match place.get(&kid) {
                Some(&k) => visit(written, place, fanout, k, (Some(*id), d + 1), seen, order),
                None => {
                    order.push((kid, d, *id));
                    1
                }
            };
        }
        assert_eq!(node.get(b"Count").and_then(Object::as_integer), Some(count));
        count
    }
    let place: HashMap<ObjectId, usize> = (written.nodes.iter().enumerate())
        .map(|(n, (id, _))| (*id, n))
        .collect();
    assert_eq!(place.len(), written.nodes.len(), "each node its own id");
    assert_eq!(written.root, written.nodes[0].0);
    let (mut seen, mut order) = (Vec::new(), Vec::new());
    visit(written, &place, fanout, 0, (None, 1), &mut seen, &mut order);
    assert_eq!(seen, (0..written.nodes.len()).collect::<Vec<_>>());
    let met: Vec<ObjectId> = order.iter().map(|&(p, _, _)| p).collect();
    assert_eq!(met, pages);
    let leaves: Vec<ObjectId> = order.iter().map(|&(_, _, n)| n).collect();
    assert_eq!(leaves, written.parents);
    order.iter().map(|&(_, d, _)| d).collect()
}

/// The sizes a tree is written at here: around a node's fanout and its
/// square, and a long document's.
const SIZES: [u32; 11] = [1, 2, 31, 32, 33, 64, 65, 1000, 1024, 1025, 7000];

/// The fanouts: the least, one that's odd, the default, and a flat tree.
const FANOUTS: [usize; 4] = [2, 3, FANOUT, usize::MAX];

#[test]
fn a_tree_is_balanced_within_its_fanout() {
    for fanout in FANOUTS {
        for n in SIZES {
            let pages = numbered(n);
            let written = laid(&pages, fanout);
            let depth = walk(&written, &pages, fanout);
            assert!(depth.iter().all(|&d| d == depth[0]), "{n}: {depth:?}");
            // As few levels as hold the pages.
            let holds = |d: u32| {
                fanout
                    .checked_pow(d)
                    .is_none_or(|reach| reach >= n as usize)
            };
            let fewest = (1..).find(|&d| holds(d)).unwrap() as usize;
            assert_eq!(depth[0], fewest, "{n} under {fanout}");
        }
    }
    // Spread evenly: 33 pages are two leaves of 17 and 16.
    let written = laid(&numbered(33), FANOUT);
    let sizes: Vec<i64> = (written.nodes[1..].iter())
        .map(|(_, node)| node.get(b"Count").and_then(Object::as_integer).unwrap())
        .collect();
    assert_eq!(sizes, [17, 16]);
    // 7,000 pages: 7 nodes under the root, each of 1,000 in 32 leaves.
    let written = laid(&numbered(7000), FANOUT);
    let root = &written.nodes[0].1;
    assert_eq!(
        root.get(b"Kids").and_then(Object::as_array).unwrap().len(),
        7
    );
    assert_eq!(written.nodes.len(), 1 + 7 + 7 * 32);
    // Flat: one node over every page.
    let written = laid(&numbered(7000), usize::MAX);
    assert_eq!(written.nodes.len(), 1);
    // And under nodes of 3, 64 pages are four levels.
    let pages = numbered(64);
    assert_eq!(walk(&laid(&pages, 3), &pages, 3)[0], 4);
}

/// `written`'s nodes and its pages, each `/Type /Page`, its `/Parent` the one
/// `written` gives it and a `/MediaBox` of its own, its width its number; and
/// a catalog naming the root.
fn objects_of(written: &Written, pages: &[ObjectId]) -> (BTreeMap<ObjectId, Object>, Dict) {
    let mut objects: BTreeMap<ObjectId, Object> = (written.nodes.iter())
        .map(|(id, node)| (*id, Object::from(node.clone())))
        .collect();
    for (&page, &parent) in pages.iter().zip(&written.parents) {
        let media = vec![
            Object::from(0),
            Object::from(0),
            Object::from(page.number),
            Object::from(200),
        ];
        let dict = pdf_objects::dict! { "Type" => "Page", "Parent" => parent, "MediaBox" => media };
        assert!(objects.insert(page, Object::from(dict)).is_none(), "{page}");
    }
    (objects, pdf_objects::dict! { "Pages" => written.root })
}

/// Each page's `/Parent` chain as `written` lays it out, nearest first: no
/// longer than one more than the nodes are many, so that a chain that loops
/// ends, and differs from what it's set beside.
fn chains(written: &Written) -> Vec<Vec<ObjectId>> {
    let up: HashMap<ObjectId, ObjectId> = (written.nodes.iter())
        .filter_map(|(id, node)| Some((*id, node.get(b"Parent")?.as_reference()?)))
        .collect();
    let most = written.nodes.len() + 1;
    (written.parents.iter())
        .map(|&leaf| {
            std::iter::successors(Some(leaf), |node| up.get(node).copied())
                .take(most)
                .collect()
        })
        .collect()
}

/// The tree read back from `objects` by both rules under `limits`, through a
/// sink that keeps what's told and through `()`: no fault, `pages` each once
/// and in order, counted so, each page's `/Parent` chain as `written` has it.
fn read_back(
    objects: &BTreeMap<ObjectId, Object>,
    catalog: &Dict,
    written: &Written,
    pages_given: &[ObjectId],
    limits: &Limits,
) {
    let resolve = |id: ObjectId| objects.get(&id);
    let chains = chains(written);
    let listed: Vec<(Option<ObjectId>, u32)> =
        pages_given.iter().map(|&id| (Some(id), 0)).collect();
    for rules in [Rules::Iso, Rules::Pdfium] {
        let mut kept = Kept::default();
        let table = pages(resolve, catalog, rules, limits, &mut kept);
        let quiet = pages(resolve, catalog, rules, limits, &mut ());
        assert_eq!(table, quiet, "{rules:?}: the same whatever the sink");
        let table = table.unwrap_or_else(|e| panic!("{rules:?}, {} pages: {e}", pages_given.len()));
        assert_eq!(kept.faults(), [], "{rules:?}, {} pages", pages_given.len());
        let read: Vec<(Option<ObjectId>, u32)> = table.iter().map(|p| (p.id, p.repeat)).collect();
        assert_eq!(read, listed, "{rules:?}: every page once, in order");
        assert_eq!(table.counted(), pages_given.len(), "{rules:?}");
        for (page, chain) in table.iter().zip(&chains) {
            assert!(page.parents().eq(chain.iter().copied()), "{rules:?}");
            assert_eq!(page.media_box().origin, Origin::Own);
        }
    }
}

#[test]
fn a_tree_written_reads_back_by_both_rules() {
    for fanout in FANOUTS {
        for n in SIZES {
            let pages = numbered(n);
            let written = laid(&pages, fanout);
            let (objects, catalog) = objects_of(&written, &pages);
            read_back(&objects, &catalog, &written, &pages, &Limits::DEFAULT);
        }
    }
    // Every small size under the small fanouts, where a node of one kid
    // comes about: 5 pages under nodes of 2 are a leaf of 2 under a node of
    // its own.
    for fanout in [2, 3] {
        for n in 1..=100 {
            let pages = numbered(n);
            let written = laid(&pages, fanout);
            let (objects, catalog) = objects_of(&written, &pages);
            read_back(&objects, &catalog, &written, &pages, &Limits::DEFAULT);
        }
    }
    // Numbers in any order, and any generation, nodes before the pages.
    let pages = [
        ObjectId::new(90, 0),
        ObjectId::new(7, 3),
        ObjectId::new(50, 1),
        ObjectId::new(8, 0),
        ObjectId::new(u32::MAX, u16::MAX),
    ];
    let asked = std::cell::Cell::new(0);
    let written = write(&pages, 2, &Limits::DEFAULT, ids_from(1, &asked)).unwrap();
    assert_eq!(written.root, ObjectId::new(1, 0));
    assert_eq!(walk(&written, &pages, 2), [3; 5]);
    let (objects, catalog) = objects_of(&written, &pages);
    read_back(&objects, &catalog, &written, &pages, &Limits::DEFAULT);
}

/// A document's pages, given ids by it, the tree written over them with
/// `|| doc.new_id()`, the file written by pdf-structure's writer and read
/// back: each page in order, under its node, by either rules with nothing
/// told.
#[test]
fn a_tree_is_written_for_a_document_by_its_ids() {
    for n in [1, 33, 1025] {
        let mut doc = pdf_structure::source::Document::new(
            pdf_objects::Version::new(1, 7),
            pdf_objects::dict! {},
        );
        let pages: Vec<ObjectId> = (0..n).map(|_| doc.new_id()).collect();
        let written = write(&pages, FANOUT, &Limits::DEFAULT, || doc.new_id()).unwrap();
        for (id, node) in &written.nodes {
            doc.set(*id, node.clone());
        }
        let media = || {
            vec![
                Object::from(0),
                Object::from(0),
                Object::from(612),
                Object::from(792),
            ]
        };
        for (&page, &parent) in pages.iter().zip(&written.parents) {
            let page_dict =
                pdf_objects::dict! { "Type" => "Page", "Parent" => parent, "MediaBox" => media() };
            doc.set(page, page_dict);
        }
        let catalog = doc.add(pdf_objects::dict! { "Type" => "Catalog", "Pages" => written.root });
        doc.trailer_mut().dict_mut().set("Root", catalog);
        let mut file = Vec::new();
        let options = pdf_structure::write::Options::DEFAULT;
        pdf_structure::write::document(&mut file, &doc, None, None, &options).unwrap();

        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        let chains = chains(&written);
        for rules in [Rules::Iso, Rules::Pdfium] {
            let table = table(&reading, reading.trailer().dict(), rules, &Limits::DEFAULT);
            assert_eq!(table.faults, [], "{n}, {rules:?}");
            let held = table.pages.unwrap();
            let read: Vec<Option<ObjectId>> = held.iter().map(|page| page.id).collect();
            let given: Vec<Option<ObjectId>> = pages.iter().copied().map(Some).collect();
            assert_eq!(read, given, "{n}, {rules:?}");
            let parents: Vec<&Vec<ObjectId>> = held.iter().map(|page| &page.parents).collect();
            assert_eq!(parents, chains.iter().collect::<Vec<_>>(), "{n}, {rules:?}");
            assert_eq!(table.counted, n as usize);
        }
    }
}

/// `pages` written under `fanout` and `limits`, and how many ids were
/// asked for.
fn refused(
    pages: &[ObjectId],
    fanout: usize,
    limits: &Limits,
) -> (Result<Written, WriteError>, usize) {
    let asked = std::cell::Cell::new(0);
    let written = write(pages, fanout, limits, ids_from(1_000_000, &asked));
    (written, asked.get())
}

#[test]
fn what_wouldn_t_read_back_is_refused_before_an_id_is_asked_for() {
    let limits = Limits::DEFAULT;
    let pages = numbered(9);
    assert_eq!(refused(&[], FANOUT, &limits), (Err(WriteError::NoPages), 0));
    for fanout in [0, 1] {
        assert_eq!(
            refused(&pages, fanout, &limits),
            (Err(WriteError::Fanout { fanout }), 0)
        );
    }
    assert!(refused(&pages, 2, &limits).0.is_ok());

    // The pages' cap, exactly and one past.
    let mut capped = limits;
    capped.pages = 9;
    let (at_cap, asked) = refused(&pages, 3, &capped);
    let at_cap = at_cap.unwrap();
    assert_eq!(asked, at_cap.nodes.len());
    let (objects, catalog) = objects_of(&at_cap, &pages);
    read_back(&objects, &catalog, &at_cap, &pages, &capped);
    capped.pages = 8;
    let too_many = WriteError::TooManyPages { cap: 8 };
    assert_eq!(refused(&pages, 3, &capped), (Err(too_many), 0));

    // Nodes past it, which only a fanout of 2 comes to: 9 pages are 10
    // nodes under the root, which a walk under a cap of 9 visits too many of.
    capped.pages = 10;
    let (ten, _) = refused(&pages, 2, &capped);
    let ten = ten.unwrap();
    assert_eq!(ten.nodes.len(), 11);
    let (objects, catalog) = objects_of(&ten, &pages);
    read_back(&objects, &catalog, &ten, &pages, &capped);
    capped.pages = 9;
    let too_many = WriteError::TooManyPages { cap: 9 };
    assert_eq!(refused(&pages, 2, &capped), (Err(too_many), 0));
    let resolve = |id: ObjectId| objects.get(&id);
    for rules in [Rules::Iso, Rules::Pdfium] {
        let read = pages_of(resolve, &catalog, rules, &capped);
        assert_eq!(read, Err(Error::TooManyPages), "{rules:?}");
    }

    // The depth's cap, at the edge under a fanout of 2: 5 pages are three
    // levels of nodes, which a walk under a cap of 2 can't go down.
    let five = numbered(5);
    let mut shallow = limits;
    shallow.page_depth = 3;
    let (three, _) = refused(&five, 2, &shallow);
    let three = three.unwrap();
    assert_eq!(walk(&three, &five, 2)[0], 3);
    let (objects, catalog) = objects_of(&three, &five);
    read_back(&objects, &catalog, &three, &five, &shallow);
    shallow.page_depth = 2;
    let too_deep = WriteError::TooDeep { cap: 2 };
    assert_eq!(refused(&five, 2, &shallow), (Err(too_deep), 0));
    let resolve = |id: ObjectId| objects.get(&id);
    for rules in [Rules::Iso, Rules::Pdfium] {
        let read = pages_of(resolve, &catalog, rules, &shallow);
        assert_eq!(read, Err(Error::TooDeep), "{rules:?}");
    }
    // Under nodes of 3 they're two levels, within it.
    assert!(refused(&five, 3, &shallow).0.is_ok());

    // A page given twice, or of a page's number, the first in order.
    let id = |number, generation| ObjectId::new(number, generation);
    for (pages, twice) in [
        (vec![id(1, 0), id(2, 0), id(1, 0)], id(1, 0)),
        (vec![id(1, 0), id(2, 0), id(2, 1), id(1, 0)], id(2, 1)),
        (vec![id(5, 0), id(5, 0)], id(5, 0)),
    ] {
        let repeated = WriteError::Repeated { id: twice };
        assert_eq!(refused(&pages, FANOUT, &limits), (Err(repeated), 0));
    }
    // A page of number 0, first or among them, whatever its generation.
    for pages in [
        vec![id(0, 0), id(1, 0)],
        vec![id(1, 0), id(0, 7), id(2, 0)],
        vec![id(0, 0)],
    ] {
        assert_eq!(refused(&pages, FANOUT, &limits), (Err(WriteError::Zero), 0));
    }

    // In the order the doc gives.
    assert_eq!(refused(&[], 0, &limits).0, Err(WriteError::NoPages));
    let mut none = limits;
    (none.pages, none.page_depth) = (0, 0);
    let zeros = [id(0, 0), id(0, 0)];
    assert_eq!(
        refused(&zeros, 1, &none).0,
        Err(WriteError::Fanout { fanout: 1 })
    );
    let too_many = WriteError::TooManyPages { cap: 0 };
    assert_eq!(refused(&zeros, 2, &none).0, Err(too_many));
    none.pages = 2;
    assert_eq!(
        refused(&zeros, 2, &none).0,
        Err(WriteError::TooDeep { cap: 0 })
    );
    none.page_depth = 1;
    assert_eq!(refused(&zeros, 2, &none).0, Err(WriteError::Zero));
    let twice = [id(1, 0), id(1, 0), id(0, 0)];
    let mut tight = limits;
    tight.pages = 3;
    tight.page_depth = 2;
    let repeated = WriteError::Repeated { id: id(1, 0) };
    assert_eq!(refused(&twice, 2, &tight).0, Err(repeated));
    // A page given twice before the nodes are counted.
    let mut nine = numbered(8);
    nine.push(id(1, 0));
    capped.pages = 9;
    let repeated = WriteError::Repeated { id: id(1, 0) };
    assert_eq!(refused(&nine, 2, &capped), (Err(repeated), 0));
}

/// The tree under `catalog` read by `rules` under `limits`, or what failed it.
fn pages_of<'o>(
    resolve: impl Resolve<'o>,
    catalog: &'o Dict,
    rules: Rules,
    limits: &Limits,
) -> Result<usize, Error> {
    pages(resolve, catalog, rules, limits, &mut ()).map(|table| table.len())
}

/// Ids answered in turn, how many were asked for kept.
fn answering<'a>(
    answers: &'a [ObjectId],
    asked: &'a std::cell::Cell<usize>,
) -> impl FnMut() -> ObjectId + 'a {
    move || {
        let answer = answers[asked.get()];
        asked.set(asked.get() + 1);
        answer
    }
}

#[test]
fn an_id_that_s_a_page_s_or_a_node_s_is_refused_as_it_s_given() {
    let id = |number| ObjectId::new(number, 0);
    // 64 pages under nodes of 3: 40 nodes, the root first.
    let pages = numbered(64);
    let nodes: Vec<ObjectId> = (100..140).map(id).collect();
    let asked = std::cell::Cell::new(0);
    let fine = write(&pages, 3, &Limits::DEFAULT, answering(&nodes, &asked)).unwrap();
    assert_eq!((fine.nodes.len(), asked.get()), (40, 40));
    for at in [0, 17, 39] {
        // A page's number, whatever the generation; a node's; and 0.
        for (answer, error) in [
            (
                ObjectId::new(5, 2),
                WriteError::Taken {
                    id: ObjectId::new(5, 2),
                },
            ),
            (id(64), WriteError::Taken { id: id(64) }),
            (ObjectId::new(0, 0), WriteError::Zero),
        ] {
            let mut answers = nodes.clone();
            answers[at] = answer;
            let asked = std::cell::Cell::new(0);
            let written = write(&pages, 3, &Limits::DEFAULT, answering(&answers, &asked));
            assert_eq!((written, asked.get()), (Err(error), at + 1), "at {at}");
        }
        if at > 0 {
            let mut answers = nodes.clone();
            answers[at] = ObjectId::new(answers[at - 1].number, 9);
            let asked = std::cell::Cell::new(0);
            let written = write(&pages, 3, &Limits::DEFAULT, answering(&answers, &asked));
            let taken = WriteError::Taken { id: answers[at] };
            assert_eq!((written, asked.get()), (Err(taken), at + 1), "at {at}");
        }
    }
    // One page: one node, which mustn't be the page.
    let asked = std::cell::Cell::new(0);
    let one = write(
        &[id(1)],
        FANOUT,
        &Limits::DEFAULT,
        answering(&[id(1)], &asked),
    );
    assert_eq!(
        (one, asked.get()),
        (Err(WriteError::Taken { id: id(1) }), 1)
    );
}

#[test]
fn a_write_error_s_words_are_the_whole_message() {
    let id = ObjectId::new(12, 3);
    for (error, words) in [
        (
            WriteError::NoPages,
            "a page tree lists a page at least, and none is given",
        ),
        (
            WriteError::Fanout { fanout: 1 },
            "a page tree's nodes are given at most 1 kids, where it takes 2",
        ),
        (
            WriteError::Repeated { id },
            "object 12 is given as a page twice",
        ),
        (
            WriteError::Zero,
            "object 0 is given as a page or a node, and it's never an object",
        ),
        (
            WriteError::Taken { id },
            "object 12 is given for a node, and it's a page or a node already",
        ),
        (
            WriteError::TooManyPages { cap: 9 },
            "the page tree would list more pages, or have more nodes under its root, than 9",
        ),
        (
            WriteError::TooDeep { cap: 2 },
            "the page tree would have more levels of nodes than 2",
        ),
    ] {
        assert_eq!(error.to_string(), words);
        assert!(std::error::Error::source(&error).is_none());
    }
}
