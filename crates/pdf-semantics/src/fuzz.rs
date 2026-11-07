//! What the fuzz target checks, which unit tests check too: the promises
//! the walks and the writers of trees make, the page tree's, dates' and XMP's,
//! on any input. It's compiled only for tests and under cargo-fuzz's
//! `--cfg fuzzing`.

use std::collections::BTreeMap;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::parse::Parser;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Unreadable, Value, Why, serialize};

use crate::date::{Date, Quirk};
use crate::pages::{Error, Inheritable, Origin, Page, Pages, Rect, pages};
use crate::tree::{self, names, numbers};
use crate::xmp::{self, Claims, metadata_stream};
use crate::{Fault, HOPS, Limits, catalog, follow};

/// How many values of an input are read as objects.
const OBJECTS: usize = 4096;

/// How many of them are each walked as a tree's root, as a run of the
/// target shouldn't take the square of its input.
const ROOTS: usize = 32;

/// How many failed starts an input may have, each a byte on from the last.
const STARTS: usize = 512;

/// Which kind of tree a walk reads an object as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A name tree.
    Names,
    /// A number tree.
    Numbers,
}

/// What a walk gave: its keys' bytes (a number's big-endian, with its sign
/// flipped, so that they sort as it does) and values, whether it was whole,
/// and what ended it, if anything did.
#[derive(Debug, Clone, PartialEq)]
pub struct Walked<'o> {
    /// Each key and value, in the order given.
    pub entries: Vec<(Vec<u8>, &'o Object)>,
    /// What [`Names::whole`](tree::Names::whole) said at the end.
    pub whole: bool,
    /// The object that ended it, and why.
    pub failed: Option<(ObjectId, Why)>,
}

/// A number's key, as [`Walked`] holds it.
fn number_key(key: i64) -> Vec<u8> {
    ((key as u64) ^ (1 << 63)).to_be_bytes().to_vec()
}

/// The number [`number_key`] made `key` of.
fn number_of(key: &[u8]) -> i64 {
    let bytes: [u8; 8] = key.try_into().expect("a number's key is eight bytes");
    (u64::from_be_bytes(bytes) ^ (1 << 63)) as i64
}

/// The tree at `root`, as `kind`, walked to its end through `sink`.
fn walked<'o, R: Resolve<'o>>(
    objects: R,
    root: &'o Object,
    kind: Kind,
    limits: &Limits,
    sink: &mut dyn Sink<Fault>,
) -> Walked<'o> {
    let mut entries = Vec::new();
    let mut failed = None;
    let whole = match kind {
        Kind::Names => {
            let mut walk = names(objects, root, limits, sink);
            for entry in walk.by_ref() {
                match entry {
                    Ok((key, value)) => entries.push((key.to_vec(), value)),
                    Err(unreadable) => failed = Some((unreadable.id, unreadable.why)),
                }
            }
            assert!(walk.next().is_none(), "it stays ended");
            walk.whole()
        }
        Kind::Numbers => {
            let mut walk = numbers(objects, root, limits, sink);
            for entry in walk.by_ref() {
                match entry {
                    Ok((key, value)) => entries.push((number_key(key), value)),
                    Err(unreadable) => failed = Some((unreadable.id, unreadable.why)),
                }
            }
            assert!(walk.next().is_none(), "it stays ended");
            walk.whole()
        }
    };
    Walked {
        entries,
        whole,
        failed,
    }
}

/// Keeps what it's told.
#[derive(Debug, Default)]
struct Kept(Vec<Fault>);

impl Sink<Fault> for Kept {
    fn fault(&mut self, _: Place, fault: Fault) {
        self.0.push(fault);
    }
}

/// Wants no faults, and fails if it's told one.
struct Unwanted;

impl Sink<Fault> for Unwanted {
    fn fault(&mut self, _: Place, fault: Fault) {
        panic!("{fault} told to a sink that wants none");
    }

    fn wants(&self) -> bool {
        false
    }
}

/// A recursive walk of a tree as the iterators are to walk it, to `depth`:
/// each node's own entries, then its kids, each reached by reference once.
/// Under no cap but depth, it's what they give.
struct Plain<'o, R> {
    objects: R,
    kind: Kind,
    depth: usize,
    seen: std::collections::HashSet<ObjectId>,
    out: Walked<'o>,
}

/// What a reference on the way comes to, to [`Plain`].
enum Plainly<'o> {
    Is(&'o Object),
    Nothing,
    Stop,
    Failed,
}

impl<'o, R: Resolve<'o> + Copy> Plain<'o, R> {
    fn once(&mut self, value: &'o Object) -> Plainly<'o> {
        let mut at = value;
        let mut last = ObjectId::new(0, 0);
        for _ in 0..HOPS {
            let Value::Ref(id) = at.value() else {
                return match at.value() {
                    Value::Null => Plainly::Nothing,
                    _ => Plainly::Is(at),
                };
            };
            last = *id;
            if !self.seen.insert(*id) {
                return Plainly::Stop;
            }
            match self.objects.try_resolve(*id) {
                Ok(Some(object)) => at = object,
                Ok(None) => return Plainly::Nothing,
                Err(unreadable) if unreadable.why == Why::Damaged => {
                    self.out.whole = false;
                    return Plainly::Stop;
                }
                Err(unreadable) => {
                    self.failed(&unreadable);
                    return Plainly::Failed;
                }
            }
        }
        self.failed(&Unreadable::new(last, Why::Capped, ""));
        Plainly::Failed
    }

    fn failed(&mut self, unreadable: &Unreadable) {
        self.out.whole = false;
        self.out.failed = Some((unreadable.id, unreadable.why));
    }

    /// The array at `key` of `node`; none when it ended the walk.
    fn array(&mut self, node: &'o Dict, key: &[u8]) -> Option<&'o [Object]> {
        let Some(written) = node.get(key) else {
            return Some(&[]);
        };
        match self.once(written) {
            Plainly::Is(object) => match object.value() {
                Value::Array(items) => Some(items),
                _ => {
                    self.out.whole = false;
                    Some(&[])
                }
            },
            Plainly::Nothing | Plainly::Stop => Some(&[]),
            Plainly::Failed => None,
        }
    }

    /// The entries of `items`: false when the walk ended.
    fn entries(&mut self, items: &'o [Object]) -> bool {
        for pair in items.chunks(2) {
            let [key, value] = pair else {
                self.out.whole = false;
                break;
            };
            let read = match follow(self.objects, key) {
                Ok(read) => read,
                Err(unreadable) if unreadable.why == Why::Damaged => None,
                Err(unreadable) => {
                    self.failed(&unreadable);
                    return false;
                }
            };
            let key = read.and_then(|read| match (self.kind, read.value()) {
                (Kind::Names, Value::String(string, _)) => string.as_bytes().map(<[u8]>::to_vec),
                (Kind::Numbers, Value::Integer(n)) => Some(number_key(*n)),
                _ => None,
            });
            match key {
                Some(key) => self.out.entries.push((key, value)),
                None => self.out.whole = false,
            }
        }
        true
    }

    /// The node `value` leads to, `depth` below the root: false when the
    /// walk ended.
    fn node(&mut self, value: &'o Object, depth: usize) -> bool {
        if depth > self.depth {
            self.out.whole = false;
            return true;
        }
        let node = match self.once(value) {
            Plainly::Is(node) => node,
            Plainly::Nothing => {
                self.out.whole &= depth == 0;
                return true;
            }
            Plainly::Stop => return true,
            Plainly::Failed => return false,
        };
        let Value::Dict(node) = node.value() else {
            self.out.whole = false;
            return true;
        };
        let key: &[u8] = match self.kind {
            Kind::Names => b"Names",
            Kind::Numbers => b"Nums",
        };
        let Some(items) = self.array(node, key) else {
            return false;
        };
        if !self.entries(items) {
            return false;
        }
        let Some(kids) = self.array(node, b"Kids") else {
            return false;
        };
        kids.iter().all(|kid| self.node(kid, depth + 1))
    }
}

/// The tree at `root`, as `kind`, walked plainly to `depth`.
fn plainly<'o, R: Resolve<'o> + Copy>(
    objects: R,
    root: &'o Object,
    kind: Kind,
    depth: usize,
) -> Walked<'o> {
    let mut plain = Plain {
        objects,
        kind,
        depth,
        seen: Default::default(),
        out: Walked {
            entries: Vec::new(),
            whole: true,
            failed: None,
        },
    };
    plain.node(root, 0);
    plain.out
}

/// The promises of one walk of the tree at `root`, as `kind`:
///
/// - what's given is the same whatever the sink, and a sink that wants none
///   is told none;
/// - it's what a plain recursive walk gives, under the default caps and under
///   small depths, in the same order, as whole;
/// - less than whole, or failed, it's said: by a fault or by what failed it;
/// - under a small count of nodes, it's whole only if it's all of it, and
///   gives nothing the walk without one doesn't;
/// - the keys of a sink's walk are said out of order where they aren't in
///   order, and not where they are;
/// - and what it gave, written, reads back sorted, each key once, whole and
///   with no fault.
fn a_tree<'o, R: Resolve<'o> + Copy>(objects: R, root: &'o Object, kind: Kind) -> Walked<'o> {
    let mut kept = Kept::default();
    let full = walked(objects, root, kind, &Limits::DEFAULT, &mut kept);
    let quiet = walked(objects, root, kind, &Limits::DEFAULT, &mut ());
    assert_eq!(full, quiet, "the same through `()`");
    let unwanted = walked(objects, root, kind, &Limits::DEFAULT, &mut Unwanted);
    assert_eq!(full, unwanted, "the same through a sink that wants none");

    assert!(
        full.whole || full.failed.is_some() || !kept.0.is_empty(),
        "what isn't whole is said: {full:?}"
    );
    if full.failed.is_some() {
        assert!(!full.whole, "what failed isn't whole");
    }
    let ascending = (full.entries.windows(2)).all(|pair| pair[0].0 < pair[1].0);
    let out_of_order = kept.0.contains(&Fault::TreeKeyOrder);
    assert_eq!(!ascending, out_of_order, "{full:?}, {:?}", kept.0);

    if kept.0.contains(&Fault::TreeTooManyNodes) {
        return full;
    }
    assert_eq!(full, plainly(objects, root, kind, 32), "a plain walk's");
    for depth in [0, 1, 2] {
        let mut limits = Limits::DEFAULT;
        limits.tree_depth = depth;
        let cut = walked(objects, root, kind, &limits, &mut ());
        assert_eq!(cut, plainly(objects, root, kind, depth), "at depth {depth}");
    }
    for nodes in [0, 1, 3] {
        let mut limits = Limits::DEFAULT;
        limits.tree_nodes = nodes;
        let cut = walked(objects, root, kind, &limits, &mut ());
        if full.failed.is_some() || cut.failed.is_some() {
            continue;
        }
        if cut.whole {
            assert_eq!(cut.entries, full.entries, "whole under {nodes} nodes");
        }
        let mut left = full.entries.clone();
        for entry in &cut.entries {
            let at = left.iter().position(|had| had == entry);
            left.swap_remove(at.expect("under a cap, nothing the whole walk doesn't give"));
        }
    }
    written_back(&full, kind);
    full
}

/// The promises of writing what a walk gave, each key once.
fn written_back(walk: &Walked, kind: Kind) {
    let mut entries: BTreeMap<&[u8], &Object> = BTreeMap::new();
    for (key, value) in &walk.entries {
        entries.entry(key).or_insert(value);
    }
    let owned = entries.iter().map(|(k, v)| (k.to_vec(), (*v).clone()));
    let tree = match kind {
        Kind::Names => tree::write_names(owned),
        Kind::Numbers => tree::write_numbers(owned.map(|(k, v)| (number_of(&k), v))),
    };
    let root = Object::from(tree.expect("each key once, no stream a value of a tree can be"));
    let nothing = |_: ObjectId| -> Option<&Object> { None };
    let mut kept = Kept::default();
    let back = walked(nothing, &root, kind, &Limits::DEFAULT, &mut kept);
    assert_eq!(kept.0, [], "written, it has no fault");
    assert!(back.whole, "written, it's whole");
    let expected: Vec<(&[u8], &Object)> = entries.into_iter().collect();
    let got: Vec<(&[u8], &Object)> = (back.entries.iter())
        .map(|(key, value)| (&key[..], *value))
        .collect();
    assert_eq!(got, expected, "written, it's what was given, sorted");

    // As bytes too, wherever the values can be written and read.
    let mut out = Vec::new();
    if serialize::write(&mut out, &root).is_ok()
        && let Ok(again) = Parser::at(&out, 0).value(&mut ())
    {
        let again = walked(
            nothing,
            &again,
            kind,
            &Limits::DEFAULT,
            &mut Kept::default(),
        );
        let keys = |walk: &Walked| -> Vec<Vec<u8>> {
            walk.entries.iter().map(|(key, _)| key.clone()).collect()
        };
        assert_eq!(keys(&again), keys(&back), "written and read, the same keys");
    }
}

/// The trees a file's catalog holds: those of its `/Names`, `/PageLabels`,
/// and its structure tree's `/ParentTree` and `/IDTree`, each with the kind
/// it is. An object that doesn't read has none.
fn roots<'o, R: Resolve<'o> + Copy>(objects: R, catalog: &'o Dict) -> Vec<(Kind, &'o Object)> {
    let dict = |value: &'o Object| follow(objects, value).ok().flatten()?.as_dict();
    let entry = |dictionary: &'o Dict, key: &[u8]| dictionary.get(key);
    let mut roots = Vec::new();
    if let Some(trees) = entry(catalog, b"Names").and_then(dict) {
        roots.extend(trees.iter().map(|(_, root)| (Kind::Names, root)));
    }
    if let Some(root) = entry(catalog, b"PageLabels") {
        roots.push((Kind::Numbers, root));
    }
    if let Some(structure) = entry(catalog, b"StructTreeRoot").and_then(dict) {
        if let Some(root) = entry(structure, b"ParentTree") {
            roots.push((Kind::Numbers, root));
        }
        if let Some(root) = entry(structure, b"IDTree") {
            roots.push((Kind::Names, root));
        }
    }
    roots
}

/// What a walk of a page tree gave: the table, or what failed it as text, as an
/// [`Unreadable`]'s own words may differ from one resolver to another.
pub type Paged<'o> = Result<Pages<'o>, String>;

fn paged<'o, R: Resolve<'o> + Copy, S: Sink<Fault> + ?Sized>(
    objects: R,
    catalog: &'o Dict,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Paged<'o> {
    pages(objects, catalog, rules, limits, sink).map_err(|error| match error {
        Error::Unreadable(unreadable) => format!("{:?} {:?}", unreadable.id, unreadable.why),
        other => other.to_string(),
    })
}

/// The promises of one walk of the page tree under `catalog`, by `rules`:
///
/// - what's given is the same whatever the sink, and a sink that wants none
///   is told none;
/// - by PDFium's rules, no fault but what doesn't read is told;
/// - it's within the caps, and under smaller caps it's the same or an error;
/// - each object listed again counts its repeats from 0, in order, and holds
///   what it held the first time;
/// - what's absent is what stands for it, and what's present names where it's
///   from: the page's own dictionary, or a node on its `/Parent` chain.
fn a_table<'o, R: Resolve<'o> + Copy>(
    objects: R,
    catalog: &'o Dict,
    rules: Rules,
) -> (Paged<'o>, Vec<Fault>) {
    let limits = Limits::DEFAULT;
    let mut kept = Kept::default();
    let full = paged(objects, catalog, rules, &limits, &mut kept);
    let quiet = paged(objects, catalog, rules, &limits, &mut ());
    assert_eq!(full, quiet, "the same through `()`");
    let unwanted = paged(objects, catalog, rules, &limits, &mut Unwanted);
    assert_eq!(full, unwanted, "the same through a sink that wants none");

    if rules == Rules::Pdfium {
        let only = |fault: &Fault| matches!(fault, Fault::Damaged { .. });
        assert!(kept.0.iter().all(only), "{:?}", kept.0);
    }
    if let Ok(table) = &full {
        let_it_hold(table, &limits, rules);
    }
    for (depth, count) in [
        (1, usize::MAX),
        (2, usize::MAX),
        (usize::MAX, 1),
        (usize::MAX, 3),
    ] {
        let mut smaller = limits;
        if depth != usize::MAX {
            smaller.page_depth = depth;
        }
        if count != usize::MAX {
            smaller.pages = count;
        }
        if let Ok(cut) = paged(objects, catalog, rules, &smaller, &mut ()) {
            assert_eq!(
                Ok(cut),
                full,
                "under {depth}, {count}: all of it, or an error"
            );
        }
    }
    (full, kept.0)
}

/// The promises of a table that came to be.
fn let_it_hold(table: &Pages, limits: &Limits, rules: Rules) {
    assert!(table.len() <= limits.pages);
    assert!(table.len() <= table.counted(), "{}", table.counted());
    if rules == Rules::Iso {
        assert_eq!(table.len(), table.counted());
    } else {
        assert!(!table.is_empty(), "no pages is an error");
    }
    let mut listed: BTreeMap<ObjectId, &Page> = BTreeMap::new();
    let mut repeats: BTreeMap<ObjectId, u32> = BTreeMap::new();
    for page in table {
        assert!(page.parents().count() <= limits.page_depth);
        match page.id {
            Some(id) => {
                let repeat = repeats.entry(id).or_insert(0);
                assert_eq!(page.repeat, *repeat, "{id:?}");
                *repeat += 1;
                let first = listed.entry(id).or_insert(page);
                assert_eq!(first.dict, page.dict, "{id:?}");
                assert_eq!(first.media_box(), page.media_box(), "{id:?}");
            }
            None => assert_eq!(page.repeat, 0),
        }
        let media = page.media_box();
        if media.origin == Origin::Default {
            assert_eq!(media.value, Rect::LETTER);
        }
        let crop = page.crop_box();
        if crop.origin == Origin::Default {
            assert_eq!(crop.value, media.value);
        }
        let rotate = page.rotate();
        if rotate.origin == Origin::Default {
            assert_eq!(rotate.value, 0);
        }
        let unit = page.user_unit();
        match unit.origin {
            Origin::Default => assert_eq!(unit.value, 1.0),
            _ => assert!(unit.value > 0.0),
        }
        for (attribute, key) in [
            (Inheritable::Resources, &b"Resources"[..]),
            (Inheritable::MediaBox, b"MediaBox"),
            (Inheritable::CropBox, b"CropBox"),
            (Inheritable::Rotate, b"Rotate"),
        ] {
            let Some(found) = page.attribute(attribute) else {
                continue;
            };
            match found.origin {
                Origin::Own => {
                    let dict = page.dict.expect("what has its own has a dictionary");
                    assert!(std::ptr::eq(dict.get(key).expect("its own"), found.written));
                }
                Origin::Ancestor(Some(id)) => {
                    assert!(page.parents().any(|parent| parent == id), "{id:?}");
                }
                Origin::Ancestor(None) => {}
                Origin::Default => panic!("an attribute that's found isn't a default"),
            }
        }
        if page.dict.is_none() {
            assert_eq!(page.media_box().origin, Origin::Default);
            assert!(page.attribute(Inheritable::Resources).is_none());
        }
    }
}

/// The page tree under `catalog` by both rules, each held to its promises; and,
/// where ISO's told no fault, the same pages by each, as a well-formed tree is.
fn both_tables<'o, R: Resolve<'o> + Copy>(objects: R, catalog: &'o Dict) -> [Paged<'o>; 2] {
    let (iso, faults) = a_table(objects, catalog, Rules::Iso);
    let (pdfium, _) = a_table(objects, catalog, Rules::Pdfium);
    if let (Ok(by_iso), true) = (&iso, faults.is_empty()) {
        let Ok(by_pdfium) = &pdfium else {
            panic!("no fault by ISO's rules, and by PDFium's {pdfium:?}");
        };
        assert_eq!(by_iso.len(), by_pdfium.len());
        assert_eq!(by_iso.counted(), by_pdfium.counted());
        for (a, b) in by_iso.iter().zip(by_pdfium) {
            assert_eq!((a.id, a.repeat), (b.id, b.repeat));
            assert!(a.dict.zip(b.dict).is_some_and(|(a, b)| std::ptr::eq(a, b)));
            assert!(a.parents().eq(b.parents()));
        }
    }
    [iso, pdfium]
}

/// What a file's catalog comes to: its trees, and its page tree by each rules.
#[derive(Debug, Clone, PartialEq)]
pub struct Over<'o> {
    /// Each tree's walk.
    pub trees: Vec<Walked<'o>>,
    /// Its pages, by ISO's rules and by PDFium's.
    pub pages: Option<[Paged<'o>; 2]>,
    /// Its metadata stream, or why it isn't known.
    pub metadata: Option<Result<Option<ObjectId>, Why>>,
}

/// Every tree the catalog `trailer` leads to holds, walked and held to its
/// promises ([`a_tree`]), and its page tree ([`both_tables`]): what each gave,
/// for a caller to set beside the same file through another resolver.
pub fn over<'o, R: Resolve<'o> + Copy>(objects: R, trailer: &'o Dict) -> Over<'o> {
    let Ok(Some(catalog)) = catalog(objects, trailer) else {
        return Over {
            trees: Vec::new(),
            pages: None,
            metadata: None,
        };
    };
    let roots = roots(objects, catalog);
    let trees = (roots.into_iter().take(ROOTS))
        .map(|(kind, root)| a_tree(objects, root, kind))
        .collect();
    Over {
        trees,
        pages: Some(both_tables(objects, catalog)),
        metadata: Some(metadata_stream(objects, catalog).map_err(|unreadable| unreadable.why)),
    }
}

/// `data` as a series of values, the `n`th of them object `n`, read by
/// pdf-objects' parser; a value that doesn't read is passed a byte at a time.
fn values(data: &[u8]) -> BTreeMap<ObjectId, Object> {
    let mut objects = BTreeMap::new();
    let (mut at, mut failed) = (0, 0);
    let mut next = 1;
    while at < data.len() && objects.len() < OBJECTS && failed < STARTS {
        let mut parser = Parser::at(data, at);
        match parser.value(&mut ()) {
            Ok(value) => {
                objects.insert(ObjectId::new(next, 0), value);
                next += 1;
                at = parser.offset();
            }
            Err(_) => {
                failed += 1;
                at += 1;
            }
        }
    }
    objects
}

/// The fuzz target's body for the walks: `data` as objects ([`values`]), the
/// first of them the trailer, over which [`over`] walks what its catalog
/// holds; and the first of them, whatever they are, each walked as a root of
/// each kind of tree ([`a_tree`]), and each dictionary as a catalog
/// ([`both_tables`]).
pub fn walks(data: &[u8]) {
    let objects = values(data);
    let resolve = |id: ObjectId| objects.get(&id);
    if let Some(trailer) = objects.get(&ObjectId::new(1, 0)).and_then(Object::as_dict) {
        over(resolve, trailer);
    }
    for root in objects.values().take(ROOTS) {
        a_tree(resolve, root, Kind::Names);
        a_tree(resolve, root, Kind::Numbers);
        if let Some(catalog) = root.as_dict() {
            let _ = both_tables(resolve, catalog);
        }
    }
}

/// The fuzz target's body for dates: `data` as text read as a date, which
/// written reads back as the same fields and the same text, with no quirk
/// but a day its month hasn't; and, with the clock, its first bytes as a
/// count of seconds, which reads back as the same date.
pub fn dates(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    if let Ok(date) = Date::read(&text) {
        let written = date.write();
        let back = Date::read(&written).expect("a date written reads");
        assert_eq!(back.write(), written, "{text}");
        let fields = |date: &Date| {
            (
                date.year(),
                [
                    date.month(),
                    date.day(),
                    date.hour(),
                    date.minute(),
                    date.second(),
                ],
                date.offset(),
            )
        };
        assert_eq!(fields(&back), fields(&date), "{text}");
        let impossible = date.quirks().contains(&Quirk::ImpossibleDay);
        let said: &[Quirk] = if impossible {
            &[Quirk::ImpossibleDay]
        } else {
            &[]
        };
        assert_eq!(back.quirks(), said, "{text}");
    }
    #[cfg(feature = "clock")]
    if let Some(first) = data.first_chunk::<8>()
        && let Some(date) = Date::from_unix(i64::from_le_bytes(*first))
    {
        let written = date.write();
        assert_eq!(Date::read(&written), Ok(date), "{written}");
    }
}

/// Each value of `claims`.
fn claim_values(claims: &Claims) -> [&Option<String>; 6] {
    [
        &claims.pdfa_part,
        &claims.pdfa_conformance,
        &claims.pdfa_rev,
        &claims.pdfua_part,
        &claims.pdfua_rev,
        &claims.pdfvt_version,
    ]
}

/// `data` as an XMP packet:
///
/// - read the same whatever the sink;
/// - an error names a place in the packet and holds what was read;
/// - each value trimmed of ASCII's white space;
/// - [`conformance`](Claims::conformance) what the values say, where it reads;
/// - under smaller caps, each value it reads is what it reads under the
///   default ones, as it reads the same up to where it stops.
pub fn xmp(data: &[u8]) {
    const PACKET: ObjectId = ObjectId::new(9, 0);

    struct Told(usize);
    impl Sink<Fault> for Told {
        fn fault(&mut self, place: Place, _: Fault) {
            assert_eq!(place.object, Some(PACKET));
            let at = place.at.expect("a place in the packet");
            assert_eq!(at.stream, Some(PACKET));
            assert!(at.start <= at.end);
            self.0 = self.0.max(at.end);
        }
    }

    let limits = Limits::DEFAULT;
    let mut told = Told(0);
    let read = xmp::read(data, Some(PACKET), &limits, &mut told);
    assert_eq!(xmp::read(data, None, &limits, &mut ()), read);
    assert!(told.0 <= data.len());
    let claims = match &read {
        Ok(claims) => claims,
        Err(error) => {
            assert!(error.at <= data.len(), "{error}");
            assert!(error.to_string().contains(&format!("{}", error.at)));
            &error.read
        }
    };
    for value in claim_values(claims).into_iter().flatten() {
        assert_eq!(value.trim_ascii(), value);
    }
    if let Ok(said) = claims.conformance() {
        assert_eq!(said.pdfa.is_some(), claims.pdfa_part.is_some());
        assert_eq!(said.pdfua.is_some(), claims.pdfua_part.is_some());
        if let Some(pdfa) = said.pdfa {
            assert!(pdfa.to_string().starts_with("PDF/A-"));
            assert!(pdfa.level.is_none_or(|level| level.is_ascii_uppercase()));
        }
        if let Some(pdfua) = said.pdfua {
            assert!(pdfua.to_string().starts_with("PDF/UA-"));
        }
    }
    let mut small = limits;
    (small.xmp_bytes, small.xmp_depth, small.xmp_bindings) = (data.len() / 2, 4, 2);
    let mut smaller = [limits; 4];
    smaller[0].xmp_bytes = small.xmp_bytes;
    smaller[1].xmp_depth = small.xmp_depth;
    smaller[2].xmp_bindings = small.xmp_bindings;
    smaller[3] = small;
    for limits in smaller {
        let under = xmp::read(data, None, &limits, &mut ());
        let under = under.unwrap_or_else(|error| error.read);
        for (under, all) in claim_values(&under).into_iter().zip(claim_values(claims)) {
            assert!(under.is_none() || under == all, "{under:?} {all:?}");
        }
    }
}

/// What the fuzzer begins from: objects as [`walks`] reads them, the first
/// the trailer, in each of a tree's shapes and a page tree's; dates; XMP
/// packets; and a count of seconds.
/// The seeds written for the target are each of these as a file too.
#[must_use]
pub fn fixtures() -> Vec<Vec<u8>> {
    let trees = [
        // A catalog with each tree a reader looks for.
        "<< /Root 2 0 R >> \
         << /Names 3 0 R /PageLabels 6 0 R /StructTreeRoot << /ParentTree 7 0 R /IDTree 8 0 R >> >> \
         << /Dests 4 0 R /JavaScript 5 0 R >> \
         << /Kids [9 0 R 10 0 R] >> \
         << /Names [(a) 11 0 R] >> \
         << /Nums [0 << /S /D >> 3 << /S /r /St 5 /P (Tab ) >>] >> \
         << /Kids [12 0 R] >> \
         << /Names [(id) 11 0 R] >> \
         << /Limits [(a) (b)] /Names [(a) 11 0 R (b) [11 0 R /Fit]] >> \
         << /Limits [(c) (d)] /Kids [13 0 R] >> \
         << /S /JavaScript /JS (app.alert\\(1\\)) >> \
         << /Limits [0 5] /Nums [0 [11 0 R] 5 11 0 R] >> \
         << /Limits [(c) (d)] /Names [(c) 1 (d) 2] >>",
        // A loop, a node listed twice, a chain, a key by reference.
        "<< /Root << /Names << /Dests 2 0 R >> >> >> \
         << /Names [3 0 R 0] /Kids [2 0 R 4 0 R 4 0 R 5 0 R] >> \
         (k) \
         << /Limits [(k) (k)] /Names 6 0 R >> \
         7 0 R \
         [(k) 1] \
         4 0 R",
        // What's wrong with a tree, a shape each.
        "<< /Root << /PageLabels 2 0 R /Names << /A 3 0 R /B 4 0 R /C 5 0 R /D 6 0 R >> >> >> \
         << /Limits [5 1] /Nums [5 0 1 0 (a) 0 null 0 1.5 0 3] /Kids [7 8 0 R null << >>] >> \
         << /Names (a) /Kids 5 >> \
         << /Kids [<< /Limits [(a) (a)] /Kids [<< /Names [(b) 0] >>] >>] >> \
         << /Names [(b) 0 (a) 0 (a) 0 (c)] >> \
         null \
         5",
        // Past the depth a tree is read to, by a chain of its own.
        "<< /Root << /PageLabels 2 0 R >> >> << /Kids [3 0 R] >> << /Kids [4 0 R] >> \
         << /Kids [5 0 R] >> << /Limits [0 1] /Nums [0 0 1 1] >>",
        // A page tree that's well formed, what it inherits at every level.
        "<< /Root 2 0 R >> << /Type /Catalog /Pages 3 0 R >> \
         << /Type /Pages /Kids [4 0 R 5 0 R] /Count 3 /MediaBox [0 0 612 792] /Resources 8 0 R >> \
         << /Type /Pages /Parent 3 0 R /Kids [6 0 R 7 0 R] /Count 2 /Rotate 90 /CropBox [0 0 9 9] >> \
         << /Type /Page /Parent 3 0 R /UserUnit 2 >> \
         << /Type /Page /Parent 4 0 R >> \
         << /Type /Page /Parent 4 0 R /MediaBox [1 2 3 4] /Rotate 180 >> \
         << >>",
        // No root /Count, a node's trusted, kids of every kind, direct, by a
        // chain, listed twice, of another /Type.
        "<< /Root 2 0 R >> << /Pages 3 0 R >> \
         << /Kids [4 0 R 4 0 R << /Type /Page >> << /Kids [5 0 R] >> 6 0 R null 7 7 0 R 8 0 R 5 0 R] >> \
         << /Parent 3 0 R /Kids [5 0 R] /Count 1 >> \
         << /Type /Page /Parent 3 0 R >> \
         5 0 R \
         (a) \
         << /Type /Foo >>",
        // A cycle, a /Parent loop, a /Parent in place and by a chain, null over
        // what's inherited, a /Count past the tree.
        "<< /Root 2 0 R >> << /Pages 3 0 R >> \
         << /Type /Pages /Kids [4 0 R 5 0 R 3 0 R] /Count 5 /Rotate 90 >> \
         << /Type /Page /Parent 5 0 R /Rotate null >> \
         << /Type /Pages /Parent 4 0 R /Kids [6 0 R 7 0 R] >> \
         << /Type /Page /Parent << /Parent 8 0 R >> /CropBox 9 0 R >> \
         << /Type /Pages /Kids [] >> \
         3 0 R \
         null",
        // A node's /Count and its own /Parent, which only a sink that wants
        // faults reads, each a loop of references.
        "<< /Root 2 0 R >> << /Pages 3 0 R >> \
         << /Type /Pages /Kids [4 0 R 6 0 R] /Count 5 0 R >> \
         << /Type /Page /Parent 3 0 R /MediaBox [0 0 1 1] >> \
         5 0 R \
         << /Type /Pages /Parent 5 0 R /Kids [7 0 R] /Count 1 >> \
         << /Type /Page /MediaBox [0 0 1 1] >>",
        // Dates.
        "D:20261024153000-08'00'",
        "D:20261024153000Z00'00'",
        "D:20260230",
        "D:202610+05'30",
        "D:2026102415300012x",
        // XMP: attributes and elements, in each namespace, under other
        // prefixes, and what's read past.
        r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" pdfaid:part="3" pdfaid:conformance="B" pdfaid:rev="2012"/></rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#,
        r#"<d xmlns:a="http://www.aiim.org/pdfa/ns/id/" xmlns:u="http://www.aiim.org/pdfua/ns/id/" xmlns:v="http://www.npes.org/pdfvt/ns/id/"><a:part>2</a:part><a:conformance> b </a:conformance><u:part>&#49;</u:part><u:rev><![CDATA[2014]]></u:rev><v:GTS_PDFVTVersion>PDF/VT-2</v:GTS_PDFVTVersion></d>"#,
        r#"<!DOCTYPE d [<!ENTITY a "3">]><d a=1 b xmlns:a="http://www.aiim.org/pdfa/ns/id/" a:part="&a;" a:rev="1" a:rev="2"><a:part>&a;</a:part><a:part>3</a:part><!-- <a:part>2</a:part> --></e><a:conformance>A</a:conformance></d>"#,
        r#"<a xmlns="http://www.aiim.org/pdfa/ns/id/" xmlns:q="u"><part>9<b/></part><q:part q:part=""><c><d><e><f><g/></f></e></d></c></q:part>"#,
    ];
    let mut seeds: Vec<Vec<u8>> = trees.map(|seed| seed.as_bytes().to_vec()).to_vec();
    seeds.push(1_792_800_000i64.to_le_bytes().to_vec());
    seeds
}

#[cfg(test)]
mod tests {
    use pdf_objects::dict;
    use pdf_structure::bytes::{Options, Reading};
    use pdf_structure::fixture::{self, Xref};

    use super::*;

    /// A mutation of `seed`, from `random`, as a fuzzer might make it.
    fn mutated(seed: &[u8], random: &mut impl FnMut() -> usize) -> Vec<u8> {
        const PIECES: [&[u8]; 30] = [
            b"[",
            b"]",
            b"<<",
            b">>",
            b"/Kids",
            b"/Names",
            b"/Nums",
            b"/Limits",
            b"1 0 R",
            b"2 0 R",
            b"3 0 R",
            b"null",
            b"(a)",
            b"7",
            b" ",
            b"%",
            b"/Parent",
            b"/Count",
            b"/Type",
            b"/Page",
            b"/Pages",
            b"/MediaBox",
            b"/Rotate",
            b"<",
            b"/>",
            b"</a:part>",
            b"xmlns:a=\"http://www.aiim.org/pdfa/ns/id/\"",
            b"a:part=\"3\"",
            b"&#51;",
            b"&a;",
        ];
        let mut data = seed.to_vec();
        for _ in 0..1 + random() % 4 {
            let at = random() % (data.len() + 1);
            let end = match random() % 3 {
                0 => at,
                _ => data.len().min(at + 1 + random() % 8),
            };
            let piece: &[u8] = match random() % 3 {
                1 => &[],
                _ => PIECES[random() % PIECES.len()],
            };
            data.splice(at..end, piece.iter().copied());
        }
        data
    }

    fn generator(mut state: u64) -> impl FnMut() -> usize {
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as usize
        }
    }

    #[test]
    fn mutated_seeds_keep_every_promise() {
        let mut random = generator(0x5EED);
        for seed in fixtures() {
            walks(&seed);
            dates(&seed);
            xmp(&seed);
            for _ in 0..200 {
                let data = mutated(&seed, &mut random);
                walks(&data);
                dates(&data);
                xmp(&data);
            }
        }
    }

    #[test]
    fn random_bytes_keep_every_promise() {
        let mut random = generator(0xB17E5);
        for _ in 0..2_000 {
            let data: Vec<u8> = (0..random() % 64).map(|_| random() as u8).collect();
            walks(&data);
            dates(&data);
            xmp(&data);
        }
    }

    /// A seed's objects, the first its trailer, as a file; a seed that's no
    /// such thing as it is.
    fn file_of(seed: &[u8]) -> Vec<u8> {
        let objects = values(seed);
        let Some(trailer) = objects.get(&ObjectId::new(1, 0)).and_then(Object::as_dict) else {
            return seed.to_vec();
        };
        let bodies: Vec<(u32, fixture::Body)> = (objects.iter())
            .skip(1)
            .map(|(id, object)| (id.number, fixture::Body::Object(object.clone())))
            .collect();
        let root = trailer.get(b"Root").cloned().unwrap_or_else(Object::null);
        fixture::file("1.7", &bodies, dict! { "Root" => root }, Xref::Table)
    }

    /// What the target does with `data` as a file, under the default options
    /// where it has caps of its own: [`over`] through a reading and through the
    /// document it makes, the same.
    fn a_file(data: &[u8]) -> usize {
        let Ok(reading) = Reading::open(data, &Options::DEFAULT) else {
            return 0;
        };
        let read = over(&reading, reading.trailer().dict());
        let document = reading
            .to_document()
            .expect("bytes in memory make a document");
        assert_eq!(over(&document, document.trailer.dict()), read);
        let entries: usize = read.trees.iter().map(|walk| walk.entries.len()).sum();
        let pages = read.pages.iter().flatten().flatten().map(Pages::len);
        entries + pages.sum::<usize>()
    }

    #[test]
    fn files_read_alike_through_a_reading_and_its_document() {
        let mut entries = 0;
        let mut random = generator(0xF11E5);
        for seed in fixtures() {
            let file = file_of(&seed);
            entries += a_file(&file);
            for _ in 0..50 {
                a_file(&mutated(&file, &mut random));
            }
        }
        assert!(
            entries > 10,
            "the seeds' files hold trees and pages: {entries}"
        );
    }

    /// `PDF_SEMANTICS_FUZZ_SEEDS` names a folder: writes the seeds there, each
    /// also as a file, for the fuzzer to begin from (`just fuzz semantics`).
    #[test]
    #[ignore = "needs PDF_SEMANTICS_FUZZ_SEEDS, a folder to write the seeds to"]
    fn the_seeds_are_written() {
        let dir = std::env::var_os("PDF_SEMANTICS_FUZZ_SEEDS")
            .expect("PDF_SEMANTICS_FUZZ_SEEDS names a folder");
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (i, seed) in fixtures().into_iter().enumerate() {
            std::fs::write(dir.join(format!("file-{i}")), file_of(&seed)).unwrap();
            std::fs::write(dir.join(format!("objects-{i}")), seed).unwrap();
        }
    }
}
