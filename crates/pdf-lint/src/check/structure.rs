//! The common structures, read through pdf-semantics: the page tree (ISO
//! 32000-2, 7.7.3), by ISO's rules and by PDFium's; each name and number
//! tree the walk reaches (7.9.6, 7.9.7); and the catalog's XMP packet
//! (14.3.2), which the extensions a file declares are read from
//! ([`extension::read`](crate::extension::read)).
//!
//! The page tree is walked before the walk from the trailer, from the
//! catalog the trailer as written leads to. The trees are read after the
//! content: each root the walk reached as a name or number tree in the
//! newest revision, once, in the order it reached them, a root with a
//! number given by it, so that what's told of the root is in its object.
//! Each reads the file's objects as the predicates resolve them
//! ([`Objects`]), which says why one doesn't read as a document's own
//! resolver does, so that an object that takes a password or is past a cap
//! ends a walk, where one that doesn't parse is the file's fault. What each
//! tells is kept ([`Kept`]), each walked to its end.
//!
//! Of what a tree's walk tells, what [`Walk::tree`] doesn't say is a
//! [`Rule::Tree`] finding: a node reached again, a kid written in place,
//! `/Limits` that aren't the node's least and greatest keys, a key not past
//! the one before it or outside a node's `/Limits`, and a node under the
//! root with neither `/Kids` nor entries (the root's is the walk's own).
//! Each kind once in each object it's in, at its first place there, its
//! value the fault's words with how many times; in the object the fault is
//! in, or, for a root written in place, the object that holds it, held
//! where that is; its path, table and key those [`Walk::tree`] gives the
//! root's findings. A kind in a node two trees share is told by the first
//! alone, as the walk checks the node as the first's. What stops a tree
//! being read whole that isn't the file's fault (an object that takes a
//! password or is past a cap, a key whose bytes the reader didn't give, a
//! tree deeper or with more nodes than is read) is counted as not checked,
//! once a tree for each, where its root is held; what was read before it is
//! found all the same.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::mem::{self, Discriminant};
use std::rc::Rc;

use pdf_semantics::pages::pages;
use pdf_semantics::tree::{names, numbers};
use pdf_semantics::{Fault, Limits, Place, Sink};

use super::content::times;
use super::{Finding, Objects, Rule, Trail, Visit, Walk};
use crate::object::{Dict, Object, ObjectId};
use crate::source::Source;
use crate::{Rules, Unreadable, Why};

/// What the structures are read under: pdf-semantics' caps, which ISO
/// 32000-2 doesn't set.
const LIMITS: Limits = Limits::DEFAULT;

/// Keeps, of each kind of fault in each object, the first told, where it
/// was, and how many were told, in the order each was first told: what a
/// finding needs, so that what's kept grows with the objects and kinds of
/// fault, never with the faults, which a file can make millions of (a
/// `/Names` of a million keys out of order, a `/Kids` listing one page a
/// million times).
#[derive(Default)]
pub(crate) struct Kept {
    /// Each object's kind: its first fault, where, and how many.
    told: Vec<(Place, Fault, usize)>,
    /// Where each object's kind is in `told`, only looked up.
    index: HashMap<(Option<ObjectId>, Discriminant<Fault>), usize>,
}

impl Sink for Kept {
    fn fault(&mut self, at: Place, fault: Fault) {
        match self.index.entry((at.object, mem::discriminant(&fault))) {
            Entry::Occupied(kind) => self.told[*kind.get()].2 += 1,
            Entry::Vacant(kind) => {
                kind.insert(self.told.len());
                self.told.push((at, fault, 1));
            }
        }
    }
}

impl Kept {
    /// What's kept, in the order each was first told.
    pub(crate) fn told(&self) -> &[(Place, Fault, usize)] {
        &self.told
    }
}

/// Notes whether a walk told of an object that doesn't read
/// ([`Fault::Damaged`]), the one fault a walk by PDFium's rules tells, and
/// keeps nothing else.
#[derive(Default)]
struct Damage {
    /// Whether one was told.
    told: bool,
}

impl Sink for Damage {
    fn fault(&mut self, _: Place, fault: Fault) {
        self.told |= matches!(fault, Fault::Damaged { .. });
    }
}

/// A name or number tree's root the walk reached ([`Walk::tree`]), which
/// [`Walk::structures`] reads again.
pub(super) struct Root<'a> {
    /// The root as written.
    object: &'a Object,
    /// Its number, where it's an indirect object.
    id: Option<ObjectId>,
    /// Whether it's a name tree's, not a number tree's.
    names: bool,
    /// The numbered object it is or is in, which holds what's told of the
    /// tree in no object: none for a table's trailer.
    holder: Option<ObjectId>,
    /// The walk's way to it, the path of the tree's findings.
    trail: Rc<Trail<'a>>,
    /// The table and key of the row whose value it is.
    table: &'static str,
    key: &'static str,
}

/// Why a tree isn't read whole, other than for the file's faults: an
/// object of it takes a password the reader wasn't given.
const LOCKED: &str = "a tree with an object that takes a password";

/// Why a tree isn't read whole: an object of it is past a cap the reader
/// reads under, or a chain of references to it is longer than is followed.
const CAPPED: &str = "a tree with an object past a cap it's read under";

/// Why a tree isn't read whole: the source didn't give an object's bytes.
const UNGIVEN: &str = "a tree with an object whose bytes the source didn't give";

/// Why a tree isn't read whole: an object of it doesn't read, for another
/// reason a reader gives.
const UNREAD: &str = "a tree with an object that doesn't read";

/// Why a tree isn't read whole: it goes deeper than pdf-semantics reads a
/// tree ([`Limits::tree_depth`]).
const DEEP: &str = "a tree deeper than it's read";

/// Why a tree isn't read whole: it has more nodes than pdf-semantics reads
/// of a tree ([`Limits::tree_nodes`]).
const NODES: &str = "a tree with more nodes than are read";

/// Why a tree isn't read whole: a key's bytes are ciphertext the reader
/// couldn't decrypt, so where it goes can't be told.
const KEY: &str = "a tree with a key whose bytes aren't known";

/// Why a tree isn't read whole, given what stopped its walk.
fn unread(why: Why) -> &'static str {
    match why {
        Why::Locked => LOCKED,
        Why::Limit => CAPPED,
        Why::Source => UNGIVEN,
        _ => UNREAD,
    }
}

/// What stopped `walk`, a tree's, from reading it, run to its end: why an
/// object didn't read, where one didn't.
fn stopped<T>(walk: impl Iterator<Item = Result<T, Unreadable>>) -> Option<Why> {
    walk.filter_map(Result::err).map(|e| e.why).last()
}

/// `value`, a tree finding's, without the count it ends with, `, 3 times`,
/// so that how many times a kind was found doesn't tell two findings apart
/// ([`super::like`]). The walk's own values never end so.
pub(super) fn uncounted(value: &str) -> &str {
    let Some((what, count)) = value.rsplit_once(", ") else {
        return value;
    };
    match count.strip_suffix(" times") {
        Some(n) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => what,
        _ => value,
    }
}

/// Walks the page tree that `trailer`'s catalog leads to, by ISO's rules,
/// what it tells kept, and by PDFium's, noting whether an object didn't
/// read; each to its end.
pub(super) fn page_trees(source: &dyn Source, trailer: &Dict) {
    let Ok(Some(catalog)) = pdf_semantics::catalog(Objects(source), trailer) else {
        return;
    };
    let mut kept = Kept::default();
    let _ = pages(Objects(source), catalog, Rules::Iso, &LIMITS, &mut kept);
    let mut damage = Damage::default();
    let _ = pages(
        Objects(source),
        catalog,
        Rules::Pdfium,
        &LIMITS,
        &mut damage,
    );
}

impl<'a> Walk<'a> {
    /// Keeps `root`, the visit of a name tree's root when `names`, else a
    /// number tree's, the value of `key` of its row, for
    /// [`Walk::structures`]. Not in an earlier revision's trailer, whose
    /// objects are the file's where no later revision wrote them, as the
    /// walk from the trailer reaches them.
    pub(super) fn note_root(&mut self, root: &Visit<'a>, names: bool, key: &'static str) {
        if self.revision.is_none() {
            self.roots.push(Root {
                object: root.object,
                id: root.id(),
                names,
                holder: root.holder,
                trail: root.trail.clone(),
                table: root.table.name.as_str(),
                key,
            });
        }
    }

    /// Reads each name and number tree the walk reached through
    /// pdf-semantics' trees, from its root, in the order the walk reached
    /// them, and finds what they tell that [`Walk::tree`] doesn't: see the
    /// module's documentation.
    pub(super) fn structures(&mut self) {
        let roots = mem::take(&mut self.roots);
        // Each numbered object's kinds told, so that a node two trees
        // share is told of once.
        let mut told = HashSet::new();
        for root in &roots {
            self.tree_told(root, &mut told);
        }
    }

    /// Finds what pdf-semantics' walk of `root`'s tree tells that
    /// [`Walk::tree`] doesn't, but each numbered object's kinds in `told`,
    /// which it adds to; and counts what stops it reading the tree whole,
    /// but the file's faults, as not checked.
    fn tree_told(&mut self, root: &Root<'a>, told: &mut HashSet<(ObjectId, Discriminant<Fault>)>) {
        // Given by its number, the walk starts in the root's object.
        let reference;
        let given = match root.id {
            Some(id) => {
                reference = Object::from(id);
                &reference
            }
            None => root.object,
        };
        let objects = Objects(self.source);
        let mut kept = Kept::default();
        let (entries, ended): (&[u8], _) = match root.names {
            true => (b"Names", stopped(names(objects, given, &LIMITS, &mut kept))),
            false => (
                b"Nums",
                stopped(numbers(objects, given, &LIMITS, &mut kept)),
            ),
        };
        // A root with neither /Kids nor entries has no node under it, so a
        // node with neither is the root, which the walk says.
        let has = |dict: &Dict, key: &[u8]| dict.get(key).is_some_and(|o| !o.is_null());
        let bare = (root.object.as_dict()).is_some_and(|d| !has(d, b"Kids") && !has(d, entries));
        let mut unread: Vec<_> = ended.map(unread).into_iter().collect();
        for &(place, fault, n) in kept.told() {
            match fault {
                Fault::TreeRepeated { .. }
                | Fault::TreeKidDirect
                | Fault::TreeBadLimits
                | Fault::TreeKeyOrder
                | Fault::TreeKeyOutsideLimits => {}
                Fault::TreeNoEntries if !bare => {}
                Fault::TreeTooDeep => {
                    unread.push(DEEP);
                    continue;
                }
                Fault::TreeTooManyNodes => {
                    unread.push(NODES);
                    continue;
                }
                Fault::TreeKeyUnread => {
                    unread.push(KEY);
                    continue;
                }
                // The walk says the rest, and an object that doesn't read.
                _ => continue,
            }
            if let Some(id) = place.object
                && !told.insert((id, mem::discriminant(&fault)))
            {
                continue;
            }
            let object = place.object.or(root.holder);
            let Some(made) = self.made(object) else {
                continue;
            };
            self.report.findings.push(Finding {
                object,
                made,
                path: root.trail.path(),
                table: root.table.to_string(),
                key: root.key.to_string(),
                rule: Rule::Tree,
                value: times(&fault.to_string(), n),
                at: place.at,
                revision: None,
            });
        }
        if self.made(root.holder).is_some() {
            unread.sort_unstable();
            unread.dedup();
            for why in unread {
                self.unchecked("-", why.to_string(), root.table, root.key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! What's kept of a walk's faults; and what's found of a tree's, where,
    //! and as whose, and what's counted.

    use pdf_structure::fixture;

    use super::super::{Hold, Made, Options, Report, check};
    use super::*;
    use crate::bytes;

    /// A fault's place in `object`, at no bytes in particular.
    fn at(object: Option<ObjectId>) -> Place {
        Place::new(object, None)
    }

    /// A destination, the value of each name tree's key here.
    const D: &str = "[3 0 R /Fit]";

    /// A file of a catalog, object 1, with `more` in it, its page tree of a
    /// page, 2 and 3, and `objects` besides, each its number and body.
    fn file(more: &str, objects: &[(u32, String)]) -> Vec<u8> {
        let catalog = format!("<< /Type /Catalog /Pages 2 0 R {more} >>");
        let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>";
        let mut all: Vec<(u32, &[u8])> = vec![
            (1, catalog.as_bytes()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, page.as_bytes()),
        ];
        all.extend(objects.iter().map(|(n, body)| (*n, body.as_bytes())));
        let size = all.iter().map(|&(n, _)| n).max().unwrap_or(0) + 1;
        fixture::assembled("1.7", &all, &format!("<< /Size {size} /Root 1 0 R >>"))
    }

    /// A file whose name tree of destinations has its root, object 10, as
    /// `root`, and `nodes` besides.
    fn dests(root: &str, nodes: &[(u32, &str)]) -> Vec<u8> {
        let mut objects = vec![(10, root.to_string())];
        objects.extend(nodes.iter().map(|&(n, body)| (n, body.to_string())));
        file("/Names << /Dests 10 0 R >>", &objects)
    }

    /// `file` read and checked, holding what `hold` does.
    fn checked(file: &[u8], hold: &Hold, options: &Options) -> Report {
        check(&bytes::read(file), hold, options)
    }

    /// A tree finding, as [`found`] gives it.
    type Found = (Option<u32>, String, String, String, String, Made);

    /// Each tree finding of `file`'s check, holding what `hold` does: its
    /// object's number, path, table and key, value, the bytes it's at in
    /// the file, and how its object was made.
    fn found(file: &[u8], hold: &Hold) -> Vec<Found> {
        let report = checked(file, hold, &Options::default());
        (report.findings.iter())
            .filter(|f| f.rule == Rule::Tree)
            .map(|f| {
                let at = f.at.map(|at| &file[at.start..at.end]).unwrap_or_default();
                (
                    f.object.map(|(n, _)| n),
                    f.path.clone(),
                    format!("{}/{}", f.table, f.key),
                    f.value.clone(),
                    String::from_utf8_lossy(at).into_owned(),
                    f.made,
                )
            })
            .collect()
    }

    /// A finding of [`found`]'s, in object `n` as merged, at `path`, of the
    /// row `table`, at `bytes`.
    fn tree(n: u32, path: &str, table: &str, value: &str, bytes: &str) -> Found {
        let (path, table) = (path.to_string(), table.to_string());
        let (value, bytes) = (value.to_string(), bytes.to_string());
        (Some(n), path, table, value, bytes, Made::Merged)
    }

    /// A finding of [`found`]'s in the tree of destinations.
    fn dest(n: u32, value: &str, bytes: &str) -> Found {
        tree(n, "Root/Names/Dests", "Name/Dests", value, bytes)
    }

    /// Every object held, as merged.
    const ALL: Hold = Hold::All(Made::Merged);

    const ORDER: &str = "a key isn't past the one before it";
    const OUTSIDE: &str = "a key is outside its node's /Limits";
    const LIMITS_BAD: &str = "a node's /Limits isn't its least and greatest keys";
    const AGAIN: &str = "the tree reaches 11 0 R again";
    const IN_PLACE: &str = "a kid is written in place, not as a reference";
    const EMPTY: &str = "a node has neither /Kids nor entries of its own";

    /// Each kind of fault pdf-semantics' walk tells that the walk doesn't
    /// say is found once, in the object it's in, at its bytes there, with
    /// the path, table and key the walk's own tree findings have: keys out
    /// of order in the root's own `/Names`, in its object, not the
    /// catalog's, and across its kids, in the kid; a key outside its
    /// node's `/Limits`; `/Limits` out of order, or one key alone; a node
    /// listed twice, in the node that lists it, or by a loop, back to the
    /// root too, which the walk is given by its number; a kid written
    /// in place, in the object it's in; a node under the root with neither
    /// `/Kids` nor `/Names`; and a number tree's keys out of order.
    #[test]
    fn what_a_trees_walk_tells_that_the_walk_doesnt_is_found_once_as_the_walks() {
        let leaf = |limits: &str, names: &str| format!("<< /Limits [{limits}] /Names [{names}] >>");
        let (a_b, c_d) = (format!("(a) {D} (b) {D}"), format!("(c) {D} (d) {D}"));
        let shapes = [
            (
                dests(&format!("<< /Names [(b) {D} (a) {D}] >>"), &[]),
                dest(10, ORDER, "(a)"),
            ),
            (
                dests(
                    "<< /Kids [11 0 R 12 0 R] >>",
                    &[(11, &leaf("(c) (d)", &c_d)), (12, &leaf("(a) (b)", &a_b))],
                ),
                dest(12, ORDER, "(a)"),
            ),
            (
                dests(
                    "<< /Kids [11 0 R] >>",
                    &[(11, &leaf("(a) (b)", &format!("(a) {D} (c) {D}")))],
                ),
                dest(11, OUTSIDE, "(c)"),
            ),
            (
                dests("<< /Kids [11 0 R] >>", &[(11, &leaf("(b) (a)", &a_b))]),
                dest(11, LIMITS_BAD, "[(b) (a)]"),
            ),
            (
                dests("<< /Kids [11 0 R] >>", &[(11, &leaf("(a)", &a_b))]),
                dest(11, LIMITS_BAD, "[(a)]"),
            ),
            (
                dests(
                    "<< /Kids [11 0 R 11 0 R] >>",
                    &[(11, &leaf("(a) (a)", &format!("(a) {D}")))],
                ),
                dest(10, AGAIN, "11 0 R"),
            ),
            (
                dests(
                    "<< /Kids [11 0 R] >>",
                    &[(11, "<< /Limits [(a) (a)] /Kids [11 0 R] >>")],
                ),
                dest(11, AGAIN, "11 0 R"),
            ),
            (
                dests(
                    "<< /Kids [11 0 R] >>",
                    &[(11, "<< /Limits [(a) (a)] /Kids [10 0 R] >>")],
                ),
                dest(11, "the tree reaches 10 0 R again", "10 0 R"),
            ),
            (
                dests(
                    &format!("<< /Kids [{}] >>", leaf("(a) (a)", &format!("(a) {D}"))),
                    &[],
                ),
                dest(10, IN_PLACE, &leaf("(a) (a)", &format!("(a) {D}"))),
            ),
            (
                dests("<< /Kids [11 0 R] >>", &[(11, "<< /Limits [(a) (a)] >>")]),
                dest(11, EMPTY, "<< /Limits [(a) (a)] >>"),
            ),
        ];
        for (file, expected) in shapes {
            assert_eq!(found(&file, &ALL), [expected]);
        }
        let labels = "<< /Nums [1 << /S /D >> 0 << /S /r >>] >>".to_string();
        let file = file("/PageLabels 10 0 R", &[(10, labels)]);
        let order = tree(10, "Root/PageLabels", "Catalog/PageLabels", ORDER, "0");
        assert_eq!(found(&file, &ALL), [order]);
    }

    /// A tree of destinations' root, its nodes besides, and the node a
    /// finding is in and what it says.
    type Shape<'s> = (&'s str, &'s [(u32, &'s str)], u32, &'s str);

    /// What the walk says of a tree is found by it alone, once, though
    /// pdf-semantics' walk tells it too: a root with neither `/Kids` nor
    /// `/Names`, or with both, or with `/Limits`; a node under the root
    /// without `/Limits`; an odd last key, and a key of another type; a kid
    /// that's null; `/Kids` or `/Names` not an array; and a kid that doesn't
    /// read, which the walk finds unresolved.
    #[test]
    fn what_the_walk_says_of_a_tree_is_found_by_it_alone() {
        let leaf = format!("<< /Limits [(k) (k)] /Names [(k) {D}] >>");
        let shapes: [Shape; 9] = [
            ("<< >>", &[], 10, "a root with neither /Kids nor /Names"),
            (
                &format!("<< /Names [(a) {D}] /Kids [11 0 R] >>"),
                &[(11, &leaf)],
                10,
                "a node with both /Kids and /Names",
            ),
            (
                &format!("<< /Limits [(a) (a)] /Names [(a) {D}] >>"),
                &[],
                10,
                "a root with /Limits",
            ),
            (
                "<< /Kids [11 0 R] >>",
                &[(11, &format!("<< /Names [(a) {D}] >>"))],
                11,
                "a node under the root without /Limits",
            ),
            (
                &format!("<< /Names [(a) {D} (b)] >>"),
                &[],
                10,
                "its last key, [2], has no value",
            ),
            (
                &format!("<< /Names [1 {D} (a) {D}] >>"),
                &[],
                10,
                "its key [0] isn't a string",
            ),
            (
                "<< /Kids [null] >>",
                &[],
                10,
                "its /Kids[0] is null, not a dictionary",
            ),
            ("<< /Kids 5 >>", &[], 10, "its /Kids isn't an array"),
            ("<< /Names (a) >>", &[], 10, "its /Names isn't an array"),
        ];
        for (root, nodes, n, said) in shapes {
            let file = dests(root, nodes);
            let node = nodes
                .iter()
                .find(|&&(m, _)| m == n)
                .map_or(root, |&(_, body)| body);
            assert_eq!(found(&file, &ALL), [dest(n, said, node)], "{root}");
        }
        let damaged = dests("<< /Kids [11 0 R] >>", &[(11, "<< /Limits [")]);
        assert_eq!(found(&damaged, &ALL), []);
        let report = checked(&damaged, &ALL, &Options::default());
        let unresolved = (report.findings.iter()).filter(|f| {
            (f.rule, f.object, f.table.as_str()) == (Rule::Unresolved, Some((10, 0)), "Name")
        });
        assert_eq!(unresolved.count(), 1, "{report}");
    }

    /// A root written in place is in the object it's written in, which
    /// holds what's found in it; what's in a node by number, that node.
    #[test]
    fn a_root_written_in_place_is_held_by_the_object_it_s_in() {
        let in_place = file(
            &format!("/Names << /Dests << /Names [(b) {D} (a) {D}] >> >>"),
            &[],
        );
        assert_eq!(found(&in_place, &ALL), [dest(1, ORDER, "(a)")]);
        let only = |n| Hold::Only([((n, 0), Made::Built)].into());
        let mut built = dest(1, ORDER, "(a)");
        built.5 = Made::Built;
        assert_eq!(found(&in_place, &only(1)), [built]);
        assert_eq!(found(&in_place, &only(2)), []);

        let leaf = format!("<< /Limits [(a) (b)] /Names [(b) {D} (a) {D}] >>");
        let kids = file("/Names << /Dests << /Kids [11 0 R] >> >>", &[(11, leaf)]);
        assert_eq!(found(&kids, &ALL), [dest(11, ORDER, "(a)")]);
        assert_eq!(found(&kids, &only(1)), []);
    }

    /// Each kind is found once in each object it's in, at its first place
    /// there, with how many times it's there: 999 keys out of order in one
    /// node are one finding; two kinds in a node two, and a kind in two
    /// nodes two, in the order the walk first told each.
    #[test]
    fn a_kind_in_a_node_is_one_finding_saying_how_many_times() {
        let names: Vec<_> = (0..1000).rev().map(|i| format!("({i:03}) {D}")).collect();
        let file = dests(&format!("<< /Names [{}] >>", names.join(" ")), &[]);
        let many = format!("{ORDER}, 999 times");
        assert_eq!(found(&file, &ALL), [dest(10, &many, "(998)")]);

        let file = dests(
            "<< /Kids [11 0 R 12 0 R] >>",
            &[
                (
                    11,
                    &format!("<< /Limits [(b) (a)] /Names [(b) {D} (a) {D}] >>"),
                ),
                (
                    12,
                    &format!("<< /Limits [(c) (d)] /Names [(d) {D} (c) {D}] >>"),
                ),
            ],
        );
        let found = found(&file, &ALL);
        let expected = [
            dest(11, LIMITS_BAD, "[(b) (a)]"),
            dest(11, ORDER, "(a)"),
            dest(12, ORDER, "(c)"),
        ];
        assert_eq!(found, expected);
    }

    /// A tree is found in once, however many times the walk reaches it: from
    /// two rows, its root reached again; and a node two trees share, as the
    /// first's, the tree the walk checks it as.
    #[test]
    fn a_tree_reached_twice_is_found_in_once() {
        let root = format!("<< /Names [(b) {D} (a) {D}] >>");
        let twice = file("/Names << /Dests 10 0 R /AP 10 0 R >>", &[(10, root)]);
        assert_eq!(found(&twice, &ALL), [dest(10, ORDER, "(a)")]);

        let leaf = format!("<< /Limits [(a) (b)] /Names [(b) {D} (a) {D}] >>");
        let kid = "<< /Kids [11 0 R] >>".to_string();
        let shared = file(
            "/Names << /Dests 10 0 R /AP 20 0 R >>",
            &[(10, kid.clone()), (11, leaf), (20, kid)],
        );
        assert_eq!(found(&shared, &ALL), [dest(11, ORDER, "(a)")]);
    }

    /// Only what's in an object held is found, though every tree is read.
    #[test]
    fn what_s_in_a_node_not_held_isnt_found() {
        let leaf = |limits: &str, keys: [&str; 2]| {
            let [a, b] = keys;
            format!("<< /Limits [{limits}] /Names [{a} {D} {b} {D}] >>")
        };
        let file = dests(
            "<< /Kids [11 0 R 12 0 R] >>",
            &[
                (11, &leaf("(c) (d)", ["(c)", "(d)"])),
                (12, &leaf("(a) (b)", ["(a)", "(b)"])),
            ],
        );
        let held = |ns: &[u32]| Hold::Only(ns.iter().map(|&n| ((n, 0), Made::Merged)).collect());
        assert_eq!(found(&file, &held(&[12])), [dest(12, ORDER, "(a)")]);
        assert_eq!(found(&file, &held(&[1, 10, 11])), []);
    }

    /// A tree finding is carried by a like one, whose count differs: by
    /// number, by path and from a source; one of a kind the earlier report
    /// hasn't is the file's own.
    #[test]
    fn a_tree_finding_is_carried_whatever_its_count() {
        let report = |root: &str| {
            let options = Options {
                paths: true,
                ..Options::default()
            };
            checked(&dests(root, &[]), &ALL, &options)
        };
        let values = |found: Vec<&Finding>| -> Vec<String> {
            found.into_iter().map(|f| f.value.clone()).collect()
        };
        let before = report(&format!("<< /Names [(c) {D} (b) {D} (a) {D}] >>"));
        let after = report(&format!("<< /Names [(d) {D} (c) {D} (b) {D} (a) {D}] >>"));
        assert_eq!(
            values(before.findings.iter().collect()),
            [format!("{ORDER}, 2 times")]
        );
        assert_eq!(
            values(after.findings.iter().collect()),
            [format!("{ORDER}, 3 times")]
        );
        assert_eq!(values(after.since(&before)), Vec::<String>::new());
        assert_eq!(values(after.by_path(&before)), Vec::<String>::new());
        assert_eq!(values(after.beyond(&[&before])), Vec::<String>::new());
        let names = format!("(d) {D} (c) {D} (b) {D} (a) {D}");
        let kid = report(&format!(
            "<< /Kids [<< /Limits [(a) (d)] /Names [{names}] >>] >>"
        ));
        assert_eq!(kid.findings.len(), 2, "{kid}");
        assert_eq!(values(kid.since(&before)), [IN_PLACE]);
        assert_eq!(values(kid.by_path(&before)), [IN_PLACE]);
        assert_eq!(values(kid.beyond(&[&before])), [IN_PLACE]);
    }

    /// The rows a check counts as not checked for a tree of destinations:
    /// why, and how many.
    fn unread_trees(report: &Report) -> Vec<(&str, usize)> {
        (report.unchecked.iter())
            .filter(|(u, _)| (u.column, u.table.as_str(), u.key.as_str()) == ("-", "Name", "Dests"))
            .map(|(u, &n)| (u.why.as_str(), n))
            .collect()
    }

    /// A tree not read whole for what isn't the file's fault is counted as
    /// not checked, once, its table and key the root's row's, where its root
    /// is held, and what was read of it is found all the same: one deeper
    /// than pdf-semantics reads, 34 levels, and one whose keys' bytes the
    /// reader didn't give. One not read whole for its own faults isn't.
    #[test]
    fn a_tree_not_read_whole_for_what_isnt_the_file_s_fault_is_counted() {
        let mut nodes: Vec<(u32, String)> = (11..43)
            .map(|n| (n, format!("<< /Limits [(a) (a)] /Kids [{} 0 R] >>", n + 1)))
            .collect();
        nodes[0].1 = "<< /Limits [(b) (a)] /Kids [12 0 R] >>".to_string();
        nodes.push((43, format!("<< /Limits [(a) (a)] /Names [(a) {D}] >>")));
        let nodes: Vec<_> = nodes.iter().map(|(n, body)| (*n, body.as_str())).collect();
        let deep = dests("<< /Kids [11 0 R] >>", &nodes);
        let bad = || dest(11, LIMITS_BAD, "[(b) (a)]");
        assert_eq!(found(&deep, &ALL), [bad()]);
        let report = checked(&deep, &ALL, &Options::default());
        assert_eq!(unread_trees(&report), [(DEEP, 1)], "{report}");
        let only = |n| Hold::Only([((n, 0), Made::Merged)].into());
        assert_eq!(found(&deep, &only(11)), [bad()]);
        let report = checked(&deep, &only(11), &Options::default());
        assert_eq!(unread_trees(&report), [], "{report}");
        let report = checked(&deep, &only(10), &Options::default());
        assert_eq!(unread_trees(&report), [(DEEP, 1)], "{report}");
        assert_eq!(found(&deep, &only(10)), []);

        // Strings left ciphertext: an encryption dictionary the reader
        // can't open.
        let catalog = "<< /Type /Catalog /Pages 2 0 R /Names << /Dests 10 0 R >> >>";
        let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>";
        let root = format!("<< /Names [(b) {D} (a) {D}] >>");
        let objects: [(u32, &[u8]); 4] = [
            (1, catalog.as_bytes()),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, page.as_bytes()),
            (10, root.as_bytes()),
        ];
        let trailer = "<< /Size 11 /Root 1 0 R /Encrypt << /Filter /Standard /V 2 >> >>";
        let locked = fixture::assembled("1.7", &objects, trailer);
        assert_eq!(found(&locked, &ALL), []);
        let report = checked(&locked, &ALL, &Options::default());
        assert_eq!(unread_trees(&report), [(KEY, 1)], "{report}");

        let null = dests(
            "<< /Kids [null 11 0 R] >>",
            &[(11, &format!("<< /Limits [(a) (a)] /Names [(a) {D}] >>"))],
        );
        let report = checked(&null, &ALL, &Options::default());
        assert_eq!(unread_trees(&report), [], "{report}");
    }

    /// Of each kind in each object, the first told and its place, and how
    /// many, in the order each kind was first told there: a kind told in
    /// another object is another, and a kind's fields don't part it.
    #[test]
    fn kept_is_each_kind_s_first_in_each_object_with_how_many() {
        let mut kept = Kept::default();
        let first = Place::new(Some((3, 0)), Some(crate::object::Span::new(None, 5, 9)));
        kept.fault(first, Fault::PageTreeRepeated { id: (5, 0) });
        kept.fault(at(Some((3, 0))), Fault::TreeKeyOrder);
        kept.fault(at(Some((3, 0))), Fault::PageTreeRepeated { id: (6, 0) });
        kept.fault(at(Some((4, 0))), Fault::PageTreeRepeated { id: (5, 0) });
        kept.fault(at(None), Fault::TreeKeyOrder);
        for _ in 0..1000 {
            kept.fault(at(Some((3, 0))), Fault::TreeKeyOrder);
        }
        kept.fault(at(None), Fault::TreeKeyOrder);
        assert_eq!(
            kept.told(),
            [
                (first, Fault::PageTreeRepeated { id: (5, 0) }, 2),
                (at(Some((3, 0))), Fault::TreeKeyOrder, 1001),
                (at(Some((4, 0))), Fault::PageTreeRepeated { id: (5, 0) }, 1),
                (at(None), Fault::TreeKeyOrder, 2),
            ]
        );
        assert_eq!(kept.index.len(), 4);
    }

    /// Only an object that doesn't read is noted: the one fault PDFium's
    /// rules tell, and the walk's others, which they don't, aren't.
    #[test]
    fn damage_notes_an_object_that_doesn_t_read_alone() {
        let mut damage = Damage::default();
        damage.fault(at(Some((2, 0))), Fault::PageTreeEmpty);
        damage.fault(at(None), Fault::PageNoMediaBox);
        assert!(!damage.told);
        damage.fault(at(Some((2, 0))), Fault::Damaged { id: (7, 0) });
        damage.fault(at(Some((2, 0))), Fault::PageTreeEmpty);
        assert!(damage.told);
    }
}
