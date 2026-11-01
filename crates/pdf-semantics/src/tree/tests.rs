use std::borrow::Cow;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::io;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use pdf_objects::parse::Parser;
use pdf_objects::{Str, dict, serialize};
use pdf_structure::bytes::{self, ByteSource, Options, Reading};
use pdf_structure::fixture::{self, Body, Xref};
use pdf_structure::source::Document;

use super::*;
use crate::catalog;

/// Keeps what it's told.
#[derive(Default)]
struct Kept(Vec<(Place, Fault)>);

impl Sink for Kept {
    fn fault(&mut self, at: Place, fault: Fault) {
        self.0.push((at, fault));
    }
}

impl Kept {
    fn faults(&self) -> Vec<Fault> {
        self.0.iter().map(|(_, fault)| *fault).collect()
    }
}

/// A sink that wants no faults, and fails if it's told one.
struct Unwanted;

impl Sink for Unwanted {
    fn fault(&mut self, _: Place, fault: Fault) {
        panic!("{fault} told to a sink that wants none");
    }

    fn wants(&self) -> bool {
        false
    }
}

fn parsed(text: &str) -> Object {
    Parser::at(text.as_bytes(), 0).value(&mut ()).unwrap()
}

/// Objects 1 and on, written as `objects` have them.
fn file(objects: &[&str]) -> BTreeMap<ObjectId, Object> {
    let numbered = (1..).zip(objects);
    (numbered.map(|(n, text)| ((n, 0), parsed(text)))).collect()
}

/// A resolver for `objects`, which says what `says` has of an object too.
struct Saying<'o> {
    objects: &'o BTreeMap<ObjectId, Object>,
    says: BTreeMap<ObjectId, Why>,
    asked: Cell<usize>,
}

impl<'o> Saying<'o> {
    fn new(objects: &'o BTreeMap<ObjectId, Object>, says: &[(u32, Why)]) -> Saying<'o> {
        Saying {
            objects,
            says: says.iter().map(|&(n, why)| ((n, 0), why)).collect(),
            asked: Cell::new(0),
        }
    }
}

impl<'o> Resolve<'o> for Saying<'o> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.objects.get(&id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        self.asked.set(self.asked.get() + 1);
        match self.says.get(&id) {
            Some(why) => Err(Unreadable::new(id, *why, "said")),
            None => Ok(self.resolve(id)),
        }
    }
}

/// What a walk of a name tree gave.
#[derive(Debug, PartialEq)]
struct Walked {
    keys: Vec<String>,
    whole: bool,
    faults: Vec<Fault>,
}

/// The name tree whose root is object 1 of `objects`, walked to its end.
fn walked(objects: &[&str], limits: &Limits) -> Walked {
    let objects = file(objects);
    let resolve = |id: ObjectId| objects.get(&id);
    let root = Object::reference(1);
    let mut walk = names(&resolve, &root, limits, Kept::default());
    let keys = (walk.by_ref())
        .map(|entry| String::from_utf8_lossy(entry.unwrap().0).into_owned())
        .collect();
    let whole = walk.whole();
    let faults = walk.into_sink().faults();
    Walked {
        keys,
        whole,
        faults,
    }
}

/// [`walked`] under the default caps: its keys, and whether it's whole.
fn read(objects: &[&str]) -> (Vec<String>, bool, Vec<Fault>) {
    let walk = walked(objects, &Limits::DEFAULT);
    (walk.keys, walk.whole, walk.faults)
}

fn strings(keys: &[&str]) -> Vec<String> {
    keys.iter().map(ToString::to_string).collect()
}

/// `(n)` written with `/Limits` of its own, a leaf of the keys given.
fn leaf(keys: &[&str]) -> String {
    let pairs: Vec<String> = keys.iter().map(|k| format!("({k}) 0")).collect();
    let (first, last) = (keys.first().unwrap(), keys.last().unwrap());
    format!(
        "<< /Limits [({first}) ({last})] /Names [{}] >>",
        pairs.join(" ")
    )
}

// A number tree with kids, by reference, its values as written; and read
// over each of a file's resolvers, the same.
#[test]
fn reads_a_number_tree_with_kids() {
    let objects = [
        (
            1,
            dict! { "Type" => "Catalog", "PageLabels" => Object::reference(4) }.into(),
        ),
        (
            2,
            dict! { "Nums" => vec![Object::from(0), dict! { "S" => "r" }.into()] }.into(),
        ),
        (
            3,
            dict! { "Nums" => vec![Object::from(2), Object::reference(5)] }.into(),
        ),
        (
            4,
            dict! { "Kids" => vec![Object::reference(2), Object::reference(3)] }.into(),
        ),
        (
            5,
            dict! { "S" => "D", "St" => 5, "P" => Object::string("Tab ") }.into(),
        ),
    ]
    .map(|(n, object): (u32, Object)| (n, Body::Object(object)));
    let bytes = fixture::file(
        "1.7",
        &objects,
        dict! { "Root" => Object::reference(1) },
        Xref::Table,
    );

    fn entries<'o, R: Resolve<'o>>(objects: &R, trailer: &'o Dict) -> Vec<(i64, ObjectId, bool)> {
        let catalog = catalog(objects, trailer).unwrap().unwrap();
        let root = catalog.get(b"PageLabels").unwrap();
        let mut walk = numbers(objects, root, &Limits::DEFAULT, ());
        let entries = (walk.by_ref())
            .map(|entry| {
                let (key, value) = entry.unwrap();
                let id = value.as_reference().unwrap_or((0, 0));
                (key, id, value.as_dict().is_some())
            })
            .collect();
        assert!(walk.whole());
        entries
    }

    let expected = [(0, (0, 0), true), (2, (5, 0), false)];
    let document = bytes::read(&bytes);
    assert_eq!(entries(&&document, document.trailer.dict()), expected);
    let reading = Reading::open(&bytes[..], &Options::DEFAULT).unwrap();
    assert_eq!(entries(&&reading, reading.trailer().dict()), expected);
    let source = ByPosition::new(&bytes);
    let reading = Reading::open(&source, &Options::DEFAULT).unwrap();
    assert_eq!(entries(&&reading, reading.trailer().dict()), expected);
}

/// A file's bytes, read a window at a time, until it's told to fail.
struct ByPosition<'b> {
    bytes: &'b [u8],
    failing: AtomicBool,
}

impl ByPosition<'_> {
    fn new(bytes: &[u8]) -> ByPosition<'_> {
        ByPosition {
            bytes,
            failing: AtomicBool::new(false),
        }
    }
}

impl ByteSource for &ByPosition<'_> {
    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        if self.failing.load(Ordering::Relaxed) {
            return Err(io::Error::other("the source stopped"));
        }
        Ok(Cow::Borrowed(&self.bytes[range]))
    }
}

// No tree: a catalog without the key has no root to walk, and a root that's
// null, or a reference to nothing, is a tree with no entries and no fault.
#[test]
fn no_labels_is_none() {
    let objects = file(&["<< /Type /Catalog >>", "null"]);
    let resolve = |id: ObjectId| objects.get(&id);
    let trailer = dict! { "Root" => Object::reference(1) };
    let catalog = catalog(&resolve, &trailer).unwrap().unwrap();
    assert!(catalog.get(b"PageLabels").is_none());
    let roots = [Object::null(), Object::reference(2), Object::reference(9)];
    for root in &roots {
        let mut walk = numbers(&resolve, root, &Limits::DEFAULT, Kept::default());
        assert!(walk.next().is_none(), "{root:?}");
        assert!(walk.whole(), "{root:?}");
        assert_eq!(walk.into_sink().faults(), [], "{root:?}");
    }
}

// Written, in a file, and read back: what was written, in order.
#[test]
fn number_tree_round_trips() {
    let label = |prefix: &str| Object::from(dict! { "S" => "D", "P" => Object::text(prefix) });
    let entries = vec![(3, label("A-")), (0, label("é"))];
    let tree = write_numbers(entries).unwrap();

    let objects = [
        (
            1,
            Body::from(dict! { "Type" => "Catalog", "PageLabels" => Object::reference(2) }),
        ),
        (2, Body::from(tree)),
    ];
    let bytes = fixture::file(
        "1.7",
        &objects,
        dict! { "Root" => Object::reference(1) },
        Xref::Table,
    );
    let document = bytes::read(&bytes);
    let objects = &document;
    let catalog = catalog(&objects, document.trailer.dict()).unwrap().unwrap();
    let root = catalog.get(b"PageLabels").unwrap();
    let mut walk = numbers(&objects, root, &Limits::DEFAULT, Kept::default());
    let read: Vec<(i64, Vec<u8>)> = (walk.by_ref())
        .map(|entry| {
            let (key, value) = entry.unwrap();
            let prefix = value.as_dict().unwrap().get(b"P").unwrap();
            (key, prefix.as_bytes().unwrap().to_vec())
        })
        .collect();
    assert_eq!(
        read,
        [(0, b"\xFE\xFF\x00\xE9".to_vec()), (3, b"A-".to_vec())]
    );
    assert!(walk.whole());
    assert_eq!(walk.into_sink().faults(), []);
}

#[test]
fn a_name_tree_written_reads_back_sorted_with_no_fault() {
    let root = Object::from(
        write_names([
            (b"b".to_vec(), Object::reference(7)),
            (b"(a)\\\n\xFF".to_vec(), Object::from(1)),
            (b"ab".to_vec(), Object::from(2)),
            (b"a".to_vec(), Object::from(3)),
            (Vec::new(), Object::from(4)),
        ])
        .unwrap(),
    );
    let mut out = Vec::new();
    serialize::write(&mut out, &root).unwrap();
    let again = Parser::at(&out, 0).value(&mut ()).unwrap();

    let expected: [&[u8]; 5] = [b"", b"(a)\\\n\xFF", b"a", b"ab", b"b"];
    for root in [&root, &again] {
        let nothing = |_: ObjectId| -> Option<&Object> { None };
        let mut walk = names(&nothing, root, &Limits::DEFAULT, Kept::default());
        let keys: Vec<&[u8]> = (walk.by_ref()).map(|entry| entry.unwrap().0).collect();
        assert_eq!(keys, expected);
        assert!(walk.whole());
        assert_eq!(walk.into_sink().faults(), []);
    }
}

#[test]
fn a_tree_written_is_sorted_with_each_key_once() {
    let one = Object::from(1);
    let root = write_numbers([(5, one.clone()), (-1, one.clone()), (2, one.clone())]).unwrap();
    let keys: Vec<i64> = (root.get(b"Nums").unwrap().as_array().unwrap().iter())
        .step_by(2)
        .map(|key| key.as_integer().unwrap())
        .collect();
    assert_eq!(keys, [-1, 2, 5]);
    assert_eq!(root.len(), 1);

    let root = write_names([(b"b".to_vec(), one.clone()), (b"a".to_vec(), one.clone())]).unwrap();
    let keys: Vec<&[u8]> = (root.get(b"Names").unwrap().as_array().unwrap().iter())
        .step_by(2)
        .map(|key| key.as_bytes().unwrap())
        .collect();
    assert_eq!(keys, [b"a", b"b"]);

    assert_eq!(
        write_names([(b"a".to_vec(), one.clone()), (b"a".to_vec(), one.clone())]),
        Err(TreeError::RepeatedName { key: b"a".to_vec() })
    );
    assert_eq!(
        write_numbers([(7, one.clone()), (0, one.clone()), (7, one.clone())]),
        Err(TreeError::RepeatedNumber { key: 7 })
    );
    let stream = Object::stream(Dict::new());
    assert_eq!(
        write_names([(b"a".to_vec(), stream.clone())]),
        Err(TreeError::StreamValue)
    );
    assert_eq!(write_numbers([(1, stream)]), Err(TreeError::StreamValue));

    let empty = write_names([]).unwrap();
    assert_eq!(empty.get(b"Names").unwrap().as_array(), Some(&[][..]));
    let empty = Object::from(empty);
    let nothing = |_: ObjectId| -> Option<&Object> { None };
    let mut walk = names(&nothing, &empty, &Limits::DEFAULT, Kept::default());
    assert!(walk.next().is_none());
    assert!(walk.whole());
    assert_eq!(walk.into_sink().faults(), []);
}

#[test]
fn a_node_s_own_entries_come_before_its_kids_each_in_the_order_written() {
    let objects = [
        "<< /Names [(m) 0] /Kids [2 0 R 3 0 R] >>",
        "<< /Limits [(a) (c)] /Kids [4 0 R] /Names [(c) 0] >>",
        &leaf(&["x", "y"]),
        &leaf(&["a", "b"]),
    ];
    let (keys, whole, faults) = read(&objects);
    // As written, not sorted.
    assert_eq!(keys, strings(&["m", "c", "a", "b", "x", "y"]));
    assert!(whole);
    assert!(faults.contains(&Fault::TreeBothEntries));
}

#[test]
fn a_clean_tree_says_no_fault() {
    let objects = [
        "<< /Kids [2 0 R 3 0 R] >>",
        "<< /Limits [(a) (b)] /Kids [4 0 R] >>",
        &leaf(&["c", "d"]),
        &leaf(&["a", "b"]),
    ];
    let (keys, whole, faults) = read(&objects);
    assert_eq!(keys, strings(&["a", "b", "c", "d"]));
    assert!(whole);
    assert_eq!(faults, []);

    // The root holding the keys.
    let (keys, whole, faults) = read(&["<< /Names [(a) 1 (b) 2] >>"]);
    assert_eq!((keys, whole), (strings(&["a", "b"]), true));
    assert_eq!(faults, []);
}

#[test]
fn a_loop_ends_said_and_each_node_is_read_once() {
    let repeated = |n| Fault::TreeRepeated { id: (n, 0) };
    // A kid its parent's parent, and one its own parent.
    let (keys, whole, faults) = read(&[
        "<< /Kids [2 0 R] >>",
        "<< /Limits [(a) (a)] /Names [(a) 0] /Kids [1 0 R 2 0 R] >>",
    ]);
    assert_eq!(keys, strings(&["a"]));
    assert!(whole, "nothing's been left unread");
    assert_eq!(faults, [Fault::TreeBothEntries, repeated(1), repeated(2)]);

    // A node listed by two parents, or twice by one.
    let (keys, whole, faults) = read(&[
        "<< /Kids [3 0 R 2 0 R 2 0 R] >>",
        &leaf(&["a"]),
        "<< /Limits [(a) (a)] /Kids [2 0 R] >>",
    ]);
    assert_eq!(keys, strings(&["a"]));
    assert!(whole);
    assert_eq!(faults, [repeated(2), repeated(2)]);

    // An array of a node's reached by reference twice.
    let (keys, whole, faults) = read(&[
        "<< /Kids [2 0 R 3 0 R] >>",
        "<< /Limits [(a) (a)] /Names 4 0 R >>",
        "<< /Limits [(a) (a)] /Names 4 0 R >>",
        "[(a) 0]",
    ]);
    assert_eq!(keys, strings(&["a"]));
    assert!(whole);
    assert_eq!(faults, [repeated(4)]);

    // An object that's itself, and references in a loop, are repeats too.
    let (keys, whole, faults) = read(&["<< /Names [(a) 0] /Kids [2 0 R] >>", "3 0 R", "2 0 R"]);
    assert_eq!(keys, strings(&["a"]));
    assert!(whole);
    assert_eq!(faults, [Fault::TreeBothEntries, repeated(2)]);
}

/// The objects of a tree `levels` below its root, each by reference, the
/// lowest holding the key `a`, and a node holding `b` at the root.
fn deep(levels: u32) -> Vec<String> {
    let mut objects = vec!["<< /Names [(b) 0] /Kids [2 0 R] >>".to_string()];
    for n in 2..=levels {
        let next = n + 1;
        objects.push(format!("<< /Limits [(a) (a)] /Kids [{next} 0 R] >>"));
    }
    objects.push(leaf(&["a"]));
    objects
}

#[test]
fn a_tree_is_read_to_its_depth_and_cut_below_it() {
    let read = |levels, depth| {
        let objects = deep(levels);
        let objects: Vec<&str> = objects.iter().map(String::as_str).collect();
        let mut limits = Limits::DEFAULT;
        limits.tree_depth = depth;
        walked(&objects, &limits)
    };
    // Levels below the root, the root none.
    let at = read(32, 32);
    assert_eq!((at.keys, at.whole), (strings(&["b", "a"]), true));
    assert!(!at.faults.contains(&Fault::TreeTooDeep));
    let past = read(33, 32);
    assert_eq!((past.keys, past.whole), (strings(&["b"]), false));
    assert!(past.faults.contains(&Fault::TreeTooDeep));
    assert_eq!(Limits::DEFAULT.tree_depth, 32);

    // Below a cap of none, only the root's.
    let none = read(1, 0);
    assert_eq!((none.keys, none.whole), (strings(&["b"]), false));

    // A node written in place is as deep as any, and it's its kids that are
    // cut, not the ones beside it.
    let in_place = [
        "<< /Kids [<< /Limits [(a) (a)] /Kids [<< /Limits [(a) (a)] /Names [(a) 0] >>] >> 2 0 R] >>",
        &leaf(&["c"]),
    ];
    let mut limits = Limits::DEFAULT;
    limits.tree_depth = 1;
    let cut = walked(&in_place, &limits);
    assert_eq!((cut.keys, cut.whole), (strings(&["c"]), false));
    assert!(cut.faults.contains(&Fault::TreeTooDeep));
    limits.tree_depth = 2;
    let all = walked(&in_place, &limits);
    assert_eq!((all.keys, all.whole), (strings(&["a", "c"]), true));

    // A node first reached too deep is read where it's reached higher up.
    let shared = [
        "<< /Kids [2 0 R 3 0 R] >>",
        "<< /Limits [(a) (a)] /Kids [3 0 R] >>",
        &leaf(&["a"]),
    ];
    limits.tree_depth = 1;
    let higher = walked(&shared, &limits);
    assert_eq!((higher.keys, higher.whole), (strings(&["a"]), false));
}

#[test]
fn a_tree_is_read_to_its_count_of_nodes_and_stops() {
    let objects = [
        "<< /Kids [2 0 R 3 0 R 4 0 R] >>",
        &leaf(&["a"]),
        &leaf(&["b"]),
        &leaf(&["c"]),
    ];
    let with = |nodes| {
        let mut limits = Limits::DEFAULT;
        limits.tree_nodes = nodes;
        walked(&objects, &limits)
    };
    let all = with(4);
    assert_eq!((all.keys, all.whole), (strings(&["a", "b", "c"]), true));
    assert_eq!(all.faults, []);
    let cut = with(3);
    assert_eq!((cut.keys, cut.whole), (strings(&["a", "b"]), false));
    assert_eq!(cut.faults, [Fault::TreeTooManyNodes]);
    // Said once, however many kids are left.
    let two = with(2);
    assert_eq!((two.keys, two.whole), (strings(&["a"]), false));
    assert_eq!(two.faults, [Fault::TreeTooManyNodes]);
    let none = with(0);
    assert_eq!((none.keys, none.whole), (strings(&[]), false));
    assert_eq!(none.faults, [Fault::TreeTooManyNodes]);
    assert_eq!(Limits::DEFAULT.tree_nodes, 100_000);
}

#[test]
fn each_fault_is_said_and_whole_says_what_it_lost() {
    use Fault::*;
    let names_array = TreeNotArray { key: "Names" };
    let kids_array = TreeNotArray { key: "Kids" };
    /// What a tree is, its objects, the keys and `whole` it reads as, and the
    /// faults said.
    type Case<'c> = (&'c str, &'c [&'c str], &'c [&'c str], bool, &'c [Fault]);
    let cases: &[Case] = &[
        // What loses nothing.
        (
            "a kid written in place",
            &["<< /Kids [<< /Limits [(a) (a)] /Names [(a) 0] >>] >>"],
            &["a"],
            true,
            &[TreeKidDirect],
        ),
        (
            "both, the root",
            &["<< /Names [(a) 0] /Kids [] >>"],
            &["a"],
            true,
            &[TreeBothEntries],
        ),
        ("neither, the root", &["<< >>"], &[], true, &[TreeNoEntries]),
        (
            "neither, under the root",
            &["<< /Kids [2 0 R] >>", "<< /Limits [(a) (a)] >>"],
            &[],
            true,
            &[TreeNoEntries],
        ),
        (
            "a root with /Limits",
            &["<< /Limits [(a) (a)] /Names [(a) 0] >>"],
            &["a"],
            true,
            &[TreeRootLimits],
        ),
        (
            "a node without",
            &["<< /Kids [2 0 R] >>", "<< /Names [(a) 0] >>"],
            &["a"],
            true,
            &[TreeNoLimits],
        ),
        (
            "keys out of order",
            &["<< /Names [(b) 0 (a) 0] >>"],
            &["b", "a"],
            true,
            &[TreeKeyOrder],
        ),
        (
            "a key repeated",
            &["<< /Names [(a) 0 (a) 1] >>"],
            &["a", "a"],
            true,
            &[TreeKeyOrder],
        ),
        (
            "a shorter key after a longer",
            &["<< /Names [(ab) 0 (a) 1] >>"],
            &["ab", "a"],
            true,
            &[TreeKeyOrder],
        ),
        (
            "keys out of order across nodes",
            &["<< /Kids [2 0 R 3 0 R] >>", &leaf(&["c"]), &leaf(&["a"])],
            &["c", "a"],
            true,
            &[TreeKeyOrder],
        ),
        (
            "outside a leaf's /Limits",
            &[
                "<< /Kids [2 0 R] >>",
                "<< /Limits [(b) (c)] /Names [(a) 0 (b) 0 (c) 0 (d) 0] >>",
            ],
            &["a", "b", "c", "d"],
            true,
            &[TreeKeyOutsideLimits, TreeKeyOutsideLimits],
        ),
        (
            "outside a parent's /Limits, inside its own",
            &[
                "<< /Kids [2 0 R] >>",
                "<< /Limits [(b) (b)] /Kids [3 0 R] >>",
                &leaf(&["c"]),
            ],
            &["c"],
            true,
            &[TreeKeyOutsideLimits],
        ),
        (
            "/Limits not an array",
            &["<< /Kids [2 0 R] >>", "<< /Limits 5 /Names [(a) 0] >>"],
            &["a"],
            true,
            &[TreeBadLimits],
        ),
        (
            "/Limits of one",
            &["<< /Kids [2 0 R] >>", "<< /Limits [(a)] /Names [(a) 0] >>"],
            &["a"],
            true,
            &[TreeBadLimits],
        ),
        (
            "/Limits greatest first",
            &[
                "<< /Kids [2 0 R] >>",
                "<< /Limits [(b) (a)] /Names [(a) 0] >>",
            ],
            &["a"],
            true,
            &[TreeBadLimits],
        ),
        (
            "/Limits of another type",
            &["<< /Kids [2 0 R] >>", "<< /Limits [1 2] /Names [(a) 0] >>"],
            &["a"],
            true,
            &[TreeBadLimits],
        ),
        // A key by reference, a kid and arrays by a chain: all's read.
        (
            "by reference",
            &[
                "<< /Kids 2 0 R >>",
                "3 0 R",
                "[4 0 R]",
                "5 0 R",
                "<< /Limits 6 0 R /Names 7 0 R >>",
                "[8 0 R 8 0 R]",
                "[8 0 R 0]",
                "(a)",
            ],
            &["a"],
            true,
            &[],
        ),
        (
            "a null /Kids",
            &["<< /Names [] /Kids null >>"],
            &[],
            true,
            &[],
        ),
        (
            "/Names by a reference to nothing",
            &["<< /Kids [] /Names 9 0 R >>"],
            &[],
            true,
            &[TreeBothEntries],
        ),
        // What leaves part of it unread.
        (
            "an odd last key",
            &["<< /Names [(a) 0 (b)] >>"],
            &["a"],
            false,
            &[TreeOddKey],
        ),
        (
            "a key of another type, and what follows it",
            &["<< /Names [1 0 (a) 0] >>"],
            &["a"],
            false,
            &[TreeKeyKind],
        ),
        (
            "a null key",
            &["<< /Names [null 0 (a) 0] >>"],
            &["a"],
            false,
            &[TreeKeyKind],
        ),
        (
            "a key that's a reference to nothing",
            &["<< /Names [9 0 R 0 (a) 0] >>"],
            &["a"],
            false,
            &[TreeKeyKind],
        ),
        (
            "/Names not an array",
            &["<< /Names (a) >>"],
            &[],
            false,
            &[names_array],
        ),
        (
            "/Kids not an array",
            &["<< /Kids 3 >>"],
            &[],
            false,
            &[kids_array],
        ),
        (
            "/Names by a reference to what isn't one",
            &["<< /Names 2 0 R >>", "5"],
            &[],
            false,
            &[names_array],
        ),
        (
            "a kid not a dictionary",
            &["<< /Kids [2 0 R 3 0 R] >>", "7", &leaf(&["a"])],
            &["a"],
            false,
            &[TreeNodeNotDictionary],
        ),
        (
            "a kid that's a string",
            &["<< /Kids [2 0 R] >>", "(a)"],
            &[],
            false,
            &[TreeNodeNotDictionary],
        ),
        (
            "a null kid",
            &["<< /Kids [null] >>"],
            &[],
            false,
            &[TreeNodeNotDictionary],
        ),
        (
            "a kid that's a reference to nothing",
            &["<< /Kids [9 0 R] >>"],
            &[],
            false,
            &[TreeNodeNotDictionary],
        ),
        (
            "a root not a dictionary",
            &["7"],
            &[],
            false,
            &[TreeNodeNotDictionary],
        ),
    ];
    for (what, objects, keys, whole, faults) in cases {
        let (read_keys, read_whole, read_faults) = read(objects);
        assert_eq!(read_keys, strings(keys), "{what}");
        assert_eq!(read_whole, *whole, "{what}");
        assert_eq!(read_faults, *faults, "{what}");
    }
}

#[test]
fn a_number_tree_says_the_same_of_its_own_keys() {
    let objects = file(&[
        "<< /Kids [2 0 R 3 0 R] >>",
        "<< /Limits [10 20] /Nums [20 0 10 0 5 0 (a) 0 null 0 1.5 0 9] >>",
        "<< /Limits [1 2] /Nums [-3 0] >>",
    ]);
    let resolve = |id: ObjectId| objects.get(&id);
    let root = Object::reference(1);
    let mut walk = numbers(&resolve, &root, &Limits::DEFAULT, Kept::default());
    let keys: Vec<i64> = (walk.by_ref()).map(|entry| entry.unwrap().0).collect();
    assert_eq!(keys, [20, 10, 5, -3]);
    assert!(!walk.whole());
    let faults = walk.into_sink().faults();
    use Fault::*;
    assert_eq!(
        faults,
        [
            TreeKeyOrder,
            TreeKeyOrder,
            TreeKeyOutsideLimits,
            TreeKeyKind,
            TreeKeyKind,
            TreeKeyKind,
            TreeOddKey,
            TreeKeyOrder,
            TreeKeyOutsideLimits,
        ]
    );
}

#[test]
fn a_key_the_reader_didn_t_give_is_said_and_lost() {
    let unread = [Str::Encrypted(b"a".to_vec()), Str::Text("a".to_string())];
    for string in unread {
        let root = Object::from(dict! {
            "Names" => vec![
                Object::new(Value::String(string.clone(), None)),
                Object::from(0),
                Object::bytes("b"),
                Object::from(0),
            ],
        });
        let nothing = |_: ObjectId| -> Option<&Object> { None };
        let mut walk = names(&nothing, &root, &Limits::DEFAULT, Kept::default());
        let keys: Vec<&[u8]> = (walk.by_ref()).map(|entry| entry.unwrap().0).collect();
        assert_eq!(keys, [b"b"], "{string:?}");
        assert!(!walk.whole(), "{string:?}");
        assert_eq!(
            walk.into_sink().faults(),
            [Fault::TreeKeyUnread],
            "{string:?}"
        );
    }
}

#[test]
fn a_fault_says_where_it_is() {
    let text = "<< /Names [(a) 0 1 0] >>";
    let objects = file(&["<< /Kids [2 0 R] >>", text]);
    let resolve = |id: ObjectId| objects.get(&id);
    let root = Object::reference(1);
    let mut walk = names(&resolve, &root, &Limits::DEFAULT, Kept::default());
    walk.by_ref().for_each(|entry| assert!(entry.is_ok()));
    let told = walk.into_sink().0;
    let (no_limits, key) = (&told[0], &told[1]);
    assert_eq!(no_limits.1, Fault::TreeNoLimits);
    assert_eq!(no_limits.0.object, Some((2, 0)));
    assert_eq!(key.1, Fault::TreeKeyKind);
    assert_eq!(key.0.object, Some((2, 0)));
    let key_at = key.0.at.unwrap();
    assert_eq!(&text[key_at.start..key_at.end], "1");
    let node_at = no_limits.0.at.unwrap();
    assert_eq!((node_at.start, node_at.end), (0, text.len()));

    // An array by reference is in its own object; a node written in place
    // in its parent's, and a root in place, in none.
    let objects = file(&["<< /Names 2 0 R >>", "[1 0]"]);
    let resolve = |id: ObjectId| objects.get(&id);
    let mut walk = names(&resolve, &root, &Limits::DEFAULT, Kept::default());
    walk.by_ref().for_each(|entry| assert!(entry.is_ok()));
    assert_eq!(walk.into_sink().0[0].0.object, Some((2, 0)));

    let objects = file(&["<< /Kids [<< /Limits [(a) (a)] /Kids [] >>] >>"]);
    let resolve = |id: ObjectId| objects.get(&id);
    let mut walk = names(&resolve, &root, &Limits::DEFAULT, Kept::default());
    walk.by_ref().for_each(|entry| assert!(entry.is_ok()));
    let told = walk.into_sink().0;
    assert_eq!(told[0], (told[0].0, Fault::TreeKidDirect));
    assert_eq!(told[0].0.object, Some((1, 0)));

    let in_place = parsed("<< >>");
    let mut walk = names(&resolve, &in_place, &Limits::DEFAULT, Kept::default());
    assert!(walk.next().is_none());
    assert_eq!(walk.into_sink().0[0].0.object, None);
}

#[test]
fn what_s_read_is_the_same_whatever_the_sink() {
    let shapes: &[&[&str]] = &[
        &[
            "<< /Kids [2 0 R 3 0 R 2 0 R] /Limits 4 >>",
            "<< /Limits [(z) (y)] /Names [(b) 0 (a) 1 2 3 (c)] >>",
            "<< /Kids [9 0 R 7 <<>>] >>",
        ],
        &["<< /Names [(a) 0] /Kids [1 0 R] >>"],
    ];
    for objects in shapes {
        let objects = file(objects);
        let resolve = |id: ObjectId| objects.get(&id);
        let root = Object::reference(1);
        let limits = Limits::DEFAULT;

        let mut kept = names(&resolve, &root, &limits, Kept::default());
        let with_kept: Vec<_> = kept.by_ref().map(Result::unwrap).collect();
        let whole = kept.whole();
        assert!(!kept.into_sink().0.is_empty());

        let mut none = names(&resolve, &root, &limits, ());
        assert_eq!(
            none.by_ref().map(Result::unwrap).collect::<Vec<_>>(),
            with_kept
        );
        assert_eq!(none.whole(), whole);

        let mut unwanted = names(&resolve, &root, &limits, Unwanted);
        assert_eq!(
            unwanted.by_ref().map(Result::unwrap).collect::<Vec<_>>(),
            with_kept
        );
        assert_eq!(unwanted.whole(), whole);

        // A sink borrowed, and through `dyn`.
        let mut sink = Kept::default();
        let mut borrowed = names(&resolve, &root, &limits, &mut sink);
        assert_eq!(
            borrowed.by_ref().map(Result::unwrap).collect::<Vec<_>>(),
            with_kept
        );
        let mut sink: &mut dyn Sink = &mut sink;
        let mut through = names(&resolve, &root, &limits, &mut sink);
        assert_eq!(
            through.by_ref().map(Result::unwrap).collect::<Vec<_>>(),
            with_kept
        );
    }
}

// `/Limits` are followed only for a sink that wants faults, and what doesn't
// read there doesn't move what's read.
#[test]
fn limits_are_read_only_for_a_sink_that_wants_them() {
    let objects = file(&[
        "<< /Kids [2 0 R] >>",
        "<< /Limits 3 0 R /Names [(a) 0] >>",
        "[(a) (a)]",
    ]);
    let quiet = Saying::new(&objects, &[]);
    let root = Object::reference(1);
    let walk = names(&quiet, &root, &Limits::DEFAULT, ());
    assert_eq!(walk.count(), 1);
    assert_eq!(quiet.asked.get(), 2, "the two nodes");

    let told = Saying::new(&objects, &[]);
    let walk = names(&told, &root, &Limits::DEFAULT, Kept::default());
    assert_eq!(walk.count(), 1);
    assert_eq!(told.asked.get(), 3, "and /Limits");

    for why in [Why::Damaged, Why::Limit, Why::Locked, Why::Source] {
        let saying = Saying::new(&objects, &[(3, why)]);
        let mut walk = names(&saying, &root, &Limits::DEFAULT, Kept::default());
        assert_eq!(walk.by_ref().map(Result::unwrap).count(), 1, "{why:?}");
        assert!(walk.whole(), "{why:?}");
        assert_eq!(walk.into_sink().faults(), [Fault::TreeBadLimits], "{why:?}");
    }
}

// Nothing's read until it's asked for, or past what's asked for.
#[test]
fn a_walk_reads_only_what_it_gives() {
    let objects = file(&[
        "<< /Names [(a) 0 3 0 R 0] /Kids 2 0 R >>",
        "[4 0 R]",
        "(b)",
        "<< /Names [(c) 0] >>",
    ]);
    let saying = Saying::new(&objects, &[]);
    let root = Object::reference(1);
    let mut walk = names(&saying, &root, &Limits::DEFAULT, ());
    assert_eq!(saying.asked.get(), 0, "nothing yet");
    assert_eq!(walk.next().unwrap().unwrap().0, b"a");
    assert_eq!(saying.asked.get(), 1, "the root");
    assert_eq!(walk.next().unwrap().unwrap().0, b"b");
    assert_eq!(saying.asked.get(), 2, "the key");
    assert_eq!(walk.next().unwrap().unwrap().0, b"c");
    assert_eq!(saying.asked.get(), 4, "the array and the kid");
    assert!(walk.next().is_none());
    assert_eq!(saying.asked.get(), 4);
}

// An object that doesn't read: damaged is said and the walk goes on, with
// `whole` false; any other kind ends it, what came before it given.
#[test]
fn what_doesn_t_read_is_said_or_ends_the_walk() {
    let shapes: [(&[&str], u32, &str); 4] = [
        (
            &[
                "<< /Kids [2 0 R 3 0 R] >>",
                &leaf(&["a"]),
                "<< /Limits [(b) (b)] /Names [4 0 R 0] >>",
                "(b)",
            ],
            4,
            "a key",
        ),
        (
            &["<< /Kids [2 0 R 3 0 R] >>", &leaf(&["a"]), &leaf(&["b"])],
            3,
            "a kid",
        ),
        (
            &[
                "<< /Kids [2 0 R 3 0 R] >>",
                &leaf(&["a"]),
                "<< /Limits [(b) (b)] /Names 4 0 R >>",
                "[(b) 0]",
            ],
            4,
            "/Names",
        ),
        (
            &[
                "<< /Kids [2 0 R 3 0 R] >>",
                &leaf(&["a"]),
                "<< /Limits [(b) (b)] /Kids 4 0 R >>",
                "[]",
            ],
            4,
            "/Kids",
        ),
    ];
    for (objects, broken, what) in shapes {
        let objects = file(objects);
        let root = Object::reference(1);

        let saying = Saying::new(&objects, &[(broken, Why::Damaged)]);
        let mut walk = names(&saying, &root, &Limits::DEFAULT, Kept::default());
        let keys: Vec<&[u8]> = (walk.by_ref()).map(|entry| entry.unwrap().0).collect();
        assert_eq!(keys, [b"a"], "{what}");
        assert!(!walk.whole(), "{what}");
        let faults = walk.into_sink().faults();
        assert!(
            faults.contains(&Fault::Damaged { id: (broken, 0) }),
            "{what}: {faults:?}"
        );

        for why in [Why::Limit, Why::Locked, Why::Source] {
            let saying = Saying::new(&objects, &[(broken, why)]);
            let mut walk = names(&saying, &root, &Limits::DEFAULT, Kept::default());
            assert_eq!(walk.next().unwrap().unwrap().0, b"a", "{what}, {why:?}");
            assert!(walk.whole(), "so far, {what}, {why:?}");
            let failed = walk.next().unwrap().unwrap_err();
            assert_eq!((failed.id, failed.why), ((broken, 0), why), "{what}");
            assert!(!walk.whole(), "{what}, {why:?}");
            assert!(walk.next().is_none(), "{what}, {why:?}");
            assert!(walk.next().is_none(), "{what}, {why:?}");
        }
    }

    // What ended it ends it, with kids left to read.
    let objects = file(&["<< /Kids [2 0 R 3 0 R] >>", &leaf(&["a"]), &leaf(&["b"])]);
    let root = Object::reference(1);
    let saying = Saying::new(&objects, &[(2, Why::Locked)]);
    let mut walk = names(&saying, &root, &Limits::DEFAULT, ());
    assert_eq!(walk.next().unwrap().unwrap_err().why, Why::Locked);
    assert!(walk.next().is_none());
    assert_eq!(saying.asked.get(), 2, "the root and the kid");

    // The root's own.
    let objects = file(&["<< >>"]);
    let root = Object::reference(1);
    let saying = Saying::new(&objects, &[(1, Why::Damaged)]);
    let mut walk = numbers(&saying, &root, &Limits::DEFAULT, Kept::default());
    assert!(walk.next().is_none());
    assert!(!walk.whole());
    assert_eq!(walk.into_sink().faults(), [Fault::Damaged { id: (1, 0) }]);
    let saying = Saying::new(&objects, &[(1, Why::Locked)]);
    let mut walk = numbers(&saying, &root, &Limits::DEFAULT, ());
    assert_eq!(walk.next().unwrap().unwrap_err().why, Why::Locked);
}

#[test]
fn a_chain_of_references_too_long_ends_the_walk() {
    // A chain of `n` references from object 2, the last to object `n + 2`,
    // the key.
    let chain = |n: u32| -> BTreeMap<ObjectId, Object> {
        let mut objects = file(&["<< /Names [2 0 R 0] >>"]);
        for at in 2..n + 1 {
            objects.insert((at, 0), Object::reference(at + 1));
        }
        objects.insert((n + 1, 0), Object::bytes("a"));
        objects
    };
    let root = Object::reference(1);
    let at = chain(31);
    let resolve = |id: ObjectId| at.get(&id);
    let mut walk = names(&resolve, &root, &Limits::DEFAULT, ());
    assert_eq!(walk.next().unwrap().unwrap().0, b"a");
    let past = chain(32);
    let resolve = |id: ObjectId| past.get(&id);
    let mut walk = names(&resolve, &root, &Limits::DEFAULT, ());
    assert_eq!(walk.next().unwrap().unwrap_err().why, Why::Limit);
    assert!(walk.next().is_none());

    // Kids likewise, where they don't loop.
    let mut long = file(&["<< /Kids [2 0 R] >>"]);
    for at in 2..34 {
        long.insert((at, 0), Object::reference(at + 1));
    }
    let resolve = |id: ObjectId| long.get(&id);
    let mut walk = names(&resolve, &root, &Limits::DEFAULT, ());
    let failed = walk.next().unwrap().unwrap_err();
    assert_eq!(failed.why, Why::Limit);
    assert_eq!(
        failed.to_string(),
        format!(
            "{} 0 R is past 32 references, or in a loop of them",
            failed.id.0
        )
    );
}

// A file's own reader: an object that doesn't parse is damaged, and a source
// that stops ends the walk, where a closure over the reader can't say.
#[test]
fn over_a_reading_the_walk_knows_why() {
    let bytes = fixture::assembled(
        "1.7",
        &[
            (1, b"<< /Type /Catalog /PageLabels 2 0 R >>"),
            (2, b"<< /Kids [3 0 R 4 0 R] >>"),
            (3, b"<< /Limits [0 0] /Nums [0 0] >>"),
            (4, b"<< /Limits [1 1"),
        ],
        "<< /Root 1 0 R >>",
    );
    let reading = Reading::open(&bytes[..], &Options::DEFAULT).unwrap();
    let objects = &reading;
    let catalog_of = catalog(&objects, reading.trailer().dict())
        .unwrap()
        .unwrap();
    let root = catalog_of.get(b"PageLabels").unwrap();
    let mut walk = numbers(&objects, root, &Limits::DEFAULT, Kept::default());
    let keys: Vec<i64> = (walk.by_ref()).map(|entry| entry.unwrap().0).collect();
    assert_eq!(keys, [0]);
    assert!(!walk.whole());
    assert!(
        walk.into_sink()
            .faults()
            .contains(&Fault::Damaged { id: (4, 0) })
    );

    let source = ByPosition::new(&bytes);
    let reading = Reading::open(&source, &Options::DEFAULT).unwrap();
    let objects = &reading;
    let catalog_of = catalog(&objects, reading.trailer().dict())
        .unwrap()
        .unwrap();
    let root = catalog_of.get(b"PageLabels").unwrap();
    source.failing.store(true, Ordering::Relaxed);
    let mut walk = numbers(&objects, root, &Limits::DEFAULT, Kept::default());
    let failed = walk.next().unwrap().unwrap_err();
    assert_eq!((failed.id, failed.why), ((2, 0), Why::Source));
    assert!(!walk.whole());
    assert!(walk.next().is_none());
}

/// Each entry a walk of a name tree gave, whether it was whole, and its faults.
type Read<'o> = (Vec<(&'o [u8], &'o Object)>, bool, Vec<Fault>);

#[test]
fn a_document_gives_what_its_reading_gives() {
    let bytes = fixture::assembled(
        "1.7",
        &[
            (1, b"<< /Type /Catalog /Names << /Dests 2 0 R >> >>"),
            (2, b"<< /Kids [3 0 R 4 0 R 3 0 R] >>"),
            (3, b"<< /Limits [(a) (a)] /Names [(a) 5 0 R] >>"),
            (4, b"<< /Limits [(b) (b)] /Names [(b) 0 (c)] >>"),
            (5, b"[1 0 R /Fit]"),
        ],
        "<< /Root 1 0 R >>",
    );
    fn walked<'o, R: Resolve<'o>>(objects: &R, trailer: &'o Dict) -> Read<'o> {
        let catalog = catalog(objects, trailer).unwrap().unwrap();
        let root = (catalog
            .get(b"Names")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Dests"))
        .unwrap();
        let mut walk = names(objects, root, &Limits::DEFAULT, Kept::default());
        let entries = walk.by_ref().map(Result::unwrap).collect();
        let whole = walk.whole();
        (entries, whole, walk.into_sink().faults())
    }
    let reading = Reading::open(&bytes[..], &Options::DEFAULT).unwrap();
    let read = walked(&&reading, reading.trailer().dict());
    assert_eq!(read.0.iter().map(|e| e.0).collect::<Vec<_>>(), [b"a", b"b"]);
    assert_eq!(read.0[0].1.as_reference(), Some((5, 0)));
    assert!(!read.1);
    assert_eq!(
        read.2,
        [Fault::TreeOddKey, Fault::TreeRepeated { id: (3, 0) }]
    );

    let document: Document = bytes::read(&bytes);
    assert_eq!(walked(&&document, document.trailer.dict()), read);
    let source = ByPosition::new(&bytes);
    let reading = Reading::open(&source, &Options::DEFAULT).unwrap();
    assert_eq!(walked(&&reading, reading.trailer().dict()), read);
}

#[test]
fn follow_goes_through_references_to_a_value() {
    let objects = file(&["5", "1 0 R", "null", "4 0 R", "4 0 R"]);
    let resolve = |id: ObjectId| objects.get(&id);
    let (one, two, three, four, five) = (
        Object::reference(1),
        Object::reference(2),
        Object::reference(3),
        Object::reference(9),
        Object::reference(5),
    );
    let (six, null) = (Object::from(6), Object::null());
    let integer = |value| follow(&resolve, value).map(|o| o.and_then(Object::as_integer));
    assert_eq!(integer(&one), Ok(Some(5)));
    assert_eq!(integer(&two), Ok(Some(5)));
    assert_eq!(integer(&six), Ok(Some(6)));
    assert_eq!(integer(&null), Ok(None));
    assert_eq!(integer(&three), Ok(None));
    assert_eq!(integer(&four), Ok(None));
    assert_eq!(integer(&five).unwrap_err().why, Why::Limit);
    assert_eq!(follow(&resolve, &null), Ok(None), "not the null object");
    assert_eq!(follow(&resolve, &three), Ok(None), "nor one it leads to");

    let saying = Saying::new(&objects, &[(1, Why::Locked)]);
    let reached = follow(&saying, &two).unwrap_err();
    assert_eq!((reached.id, reached.why), ((1, 0), Why::Locked));
}

#[test]
fn catalog_follows_the_trailer_s_root() {
    let objects = file(&["<< /Type /Catalog >>", "1 0 R", "5"]);
    let resolve = |id: ObjectId| objects.get(&id);
    let is = |trailer: Dict| {
        let trailer = Box::leak(Box::new(trailer));
        catalog(&resolve, trailer).map(|c| c.is_some())
    };
    assert_eq!(is(dict! { "Root" => Object::reference(1) }), Ok(true));
    assert_eq!(is(dict! { "Root" => Object::reference(2) }), Ok(true));
    assert_eq!(is(dict! { "Root" => dict! {} }), Ok(true));
    assert_eq!(is(dict! { "Root" => Object::reference(3) }), Ok(false));
    assert_eq!(is(dict! { "Root" => Object::reference(9) }), Ok(false));
    assert_eq!(is(dict! {}), Ok(false));

    let saying = Saying::new(&objects, &[(1, Why::Source)]);
    let trailer = dict! { "Root" => Object::reference(1) };
    assert_eq!(catalog(&saying, &trailer).unwrap_err().why, Why::Source);
}
