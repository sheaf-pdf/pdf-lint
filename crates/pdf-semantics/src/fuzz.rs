//! What the fuzz target checks, which unit tests check too: the promises
//! the walks and the writers of trees make, and dates', on any input. It's
//! compiled only for tests and under cargo-fuzz's `--cfg fuzzing`.

use std::collections::BTreeMap;

use pdf_objects::parse::Parser;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Unreadable, Value, Why, serialize};

use crate::date::{Date, Quirk};
use crate::tree::{self, names, numbers};
use crate::{Fault, HOPS, Limits, Place, Sink, catalog, follow};

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
fn walked<'o, R: Resolve<'o> + ?Sized>(
    objects: &R,
    root: &'o Object,
    kind: Kind,
    limits: &Limits,
    sink: &mut dyn Sink,
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

impl Sink for Kept {
    fn fault(&mut self, _: Place, fault: Fault) {
        self.0.push(fault);
    }
}

/// Wants no faults, and fails if it's told one.
struct Unwanted;

impl Sink for Unwanted {
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
struct Plain<'o, 'r, R: ?Sized> {
    objects: &'r R,
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

impl<'o, R: Resolve<'o> + ?Sized> Plain<'o, '_, R> {
    fn once(&mut self, value: &'o Object) -> Plainly<'o> {
        let mut at = value;
        let mut last = (0, 0);
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
        self.failed(&Unreadable::new(last, Why::Limit, ""));
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
fn plainly<'o, R: Resolve<'o> + ?Sized>(
    objects: &R,
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
fn a_tree<'o, R: Resolve<'o> + ?Sized>(objects: &R, root: &'o Object, kind: Kind) -> Walked<'o> {
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
    let back = walked(&nothing, &root, kind, &Limits::DEFAULT, &mut kept);
    assert_eq!(kept.0, [], "written, it has no fault");
    assert!(back.whole, "written, it's whole");
    let expected: Vec<(&[u8], &Object)> = entries.into_iter().collect();
    let got: Vec<(&[u8], &Object)> = (back.entries.iter())
        .map(|(key, value)| (&key[..], *value))
        .collect();
    assert_eq!(got, expected, "written, it's what was given, sorted");

    // As bytes too, wherever the values can be written and read.
    let mut out = Vec::new();
    if serialize::write(&mut out, &root).is_ok() {
        if let Ok(again) = Parser::at(&out, 0).value(&mut ()) {
            let again = walked(
                &nothing,
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
}

/// The trees a file's catalog holds: those of its `/Names`, `/PageLabels`,
/// and its structure tree's `/ParentTree` and `/IDTree`, each with the kind
/// it is. An object that doesn't read has none.
fn roots<'o, R: Resolve<'o> + ?Sized>(objects: &R, catalog: &'o Dict) -> Vec<(Kind, &'o Object)> {
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

/// Every tree the catalog `trailer` leads to holds, walked and held to its
/// promises ([`a_tree`]): what each gave, for a caller to set beside the same
/// file through another resolver.
pub fn over<'o, R: Resolve<'o> + ?Sized>(objects: &R, trailer: &'o Dict) -> Vec<Walked<'o>> {
    let Ok(Some(catalog)) = catalog(objects, trailer) else {
        return Vec::new();
    };
    let roots = roots(objects, catalog);
    (roots.into_iter().take(ROOTS))
        .map(|(kind, root)| a_tree(objects, root, kind))
        .collect()
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
                objects.insert((next, 0), value);
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
/// each kind of tree ([`a_tree`]).
pub fn walks(data: &[u8]) {
    let objects = values(data);
    let resolve = |id: ObjectId| objects.get(&id);
    if let Some(trailer) = objects.get(&(1, 0)).and_then(Object::as_dict) {
        over(&resolve, trailer);
    }
    for root in objects.values().take(ROOTS) {
        a_tree(&resolve, root, Kind::Names);
        a_tree(&resolve, root, Kind::Numbers);
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
    if let Some(first) = data.first_chunk::<8>() {
        if let Some(date) = Date::from_unix(i64::from_le_bytes(*first)) {
            let written = date.write();
            assert_eq!(Date::read(&written), Ok(date), "{written}");
        }
    }
}

/// What the fuzzer begins from: objects as [`walks`] reads them, the first
/// the trailer, in each of a tree's shapes; dates; and a count of seconds.
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
        // Dates.
        "D:20261024153000-08'00'",
        "D:20261024153000Z00'00'",
        "D:20260230",
        "D:202610+05'30",
        "D:2026102415300012x",
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
        const PIECES: [&[u8]; 16] = [
            b"[", b"]", b"<<", b">>", b"/Kids", b"/Names", b"/Nums", b"/Limits", b"1 0 R",
            b"2 0 R", b"3 0 R", b"null", b"(a)", b"7", b" ", b"%",
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
            for _ in 0..200 {
                let data = mutated(&seed, &mut random);
                walks(&data);
                dates(&data);
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
        }
    }

    /// A seed's objects, the first its trailer, as a file; a seed that's no
    /// such thing as it is.
    fn file_of(seed: &[u8]) -> Vec<u8> {
        let objects = values(seed);
        let Some(trailer) = objects.get(&(1, 0)).and_then(Object::as_dict) else {
            return seed.to_vec();
        };
        let bodies: Vec<(u32, fixture::Body)> = (objects.iter())
            .skip(1)
            .map(|(id, object)| (id.0, fixture::Body::Object(object.clone())))
            .collect();
        let root = trailer.get(b"Root").cloned().unwrap_or_else(Object::null);
        fixture::file("1.7", &bodies, dict! { "Root" => root }, Xref::Table)
    }

    /// What the target does with `data` as a file: [`over`] through a reading
    /// and through the document it makes, the same.
    fn a_file(data: &[u8]) -> usize {
        let Ok(reading) = Reading::open(data, &Options::DEFAULT) else {
            return 0;
        };
        let objects = &reading;
        let read = over(&objects, reading.trailer().dict());
        let document = reading
            .to_document()
            .expect("bytes in memory make a document");
        let objects = &document;
        assert_eq!(over(&objects, document.trailer.dict()), read);
        read.iter().map(|walk| walk.entries.len()).sum()
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
        assert!(entries > 10, "the seeds' files hold trees: {entries}");
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
