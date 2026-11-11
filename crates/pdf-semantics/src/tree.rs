//! Name trees and number trees (ISO 32000-2, 7.9.6 and 7.9.7): walked under
//! caps, a key and its value at a time, and written.
//!
//! [`names`] and [`numbers`] are iterators, reading nothing until they're
//! asked and no more than what's asked for needs: a node's own entries,
//! then each of its kids', as the tree is written. Each gives a key and its
//! value borrowed from what the resolver keeps, the value as written, so a
//! reference stays one for the caller to follow ([`follow`](crate::follow)).
//!
//! What a walk can't read goes one of two ways:
//!
//! - **It's failed** by an object past a cap the reader reads under, one
//!   that takes a password, or bytes the source didn't give: the iterator
//!   gives the [`Unreadable`] and then ends. So does a chain of references
//!   too long to follow.
//! - **It goes on** past anything else, told to its [`Sink`]. What leaves
//!   part of the tree unread makes [`Names::whole`] false, as a kid that's
//!   no dictionary or an object that doesn't read do, and a tree past
//!   [`Limits::tree_depth`] or [`Limits::tree_nodes`]. What loses nothing
//!   leaves it true, as a node reached a second time does: it isn't read
//!   again, so a loop ends and everything's been read once.
//!
//! [`write_names`] and [`write_numbers`] write a tree flat, one root holding
//! every entry, sorted. That's a whole tree at any size, and nodes under a
//! root would each need an object's number.
//!
//! ```
//! use pdf_semantics::tree::{names, write_names};
//! use pdf_semantics::Limits;
//! use pdf_semantics::pdf_objects::{Object, ObjectId};
//!
//! let tree = write_names([
//!     (b"b".to_vec(), Object::reference(5)),
//!     (b"a".to_vec(), Object::from(1)),
//! ])
//! .unwrap();
//! let root = Object::from(tree);
//!
//! let nothing = |_: ObjectId| -> Option<&Object> { None };
//! let mut walk = names(nothing, &root, &Limits::DEFAULT, ());
//! let (key, value) = walk.next().unwrap().unwrap();
//! assert_eq!((key, value.as_integer()), (&b"a"[..], Some(1)));
//! let (key, value) = walk.next().unwrap().unwrap();
//! assert_eq!((key, value.as_reference()), (&b"b"[..], Some(ObjectId::new(5, 0))));
//! assert!(walk.next().is_none());
//! assert!(walk.whole());
//! ```

use std::collections::HashSet;
use std::fmt;
use std::iter::FusedIterator;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::{Dict, Object, ObjectId, Resolve, Span, Unreadable, Value, Why};

use crate::fault::Fault;
use crate::limits::Limits;
use crate::{HOPS, followed};

/// The entries of the name tree at `root`, in the order the tree is written:
/// each key's bytes and its value. `root` may be a reference, followed as a
/// node's kids are; null, or a reference to nothing, is a tree with no
/// entries.
pub fn names<'o, R: Resolve<'o>, S: Sink<Fault>>(
    objects: R,
    root: &'o Object,
    limits: &Limits,
    sink: S,
) -> Names<'o, R, S> {
    Names(Walk::new(objects, root, limits, sink))
}

/// The entries of the number tree at `root`, as [`names`] gives a name
/// tree's: each key, an integer, and its value.
pub fn numbers<'o, R: Resolve<'o>, S: Sink<Fault>>(
    objects: R,
    root: &'o Object,
    limits: &Limits,
    sink: S,
) -> Numbers<'o, R, S> {
    Numbers(Walk::new(objects, root, limits, sink))
}

/// A name tree's entries, as [`names`] gives them.
pub struct Names<'o, R, S>(Walk<'o, R, S, &'o [u8]>);

/// A number tree's entries, as [`numbers`] gives them.
pub struct Numbers<'o, R, S>(Walk<'o, R, S, i64>);

/// What the two iterators have in common.
macro_rules! walks {
    ($walk:ident, $key:ty) => {
        impl<'o, R: Resolve<'o>, S: Sink<Fault>> Iterator for $walk<'o, R, S> {
            type Item = Result<($key, &'o Object), Unreadable>;

            fn next(&mut self) -> Option<Self::Item> {
                self.0.next()
            }
        }

        impl<'o, R: Resolve<'o>, S: Sink<Fault>> FusedIterator for $walk<'o, R, S> {}

        impl<'o, R, S> $walk<'o, R, S> {
            /// Whether every entry of the tree has been given, as far as the
            /// walk has gone: true until something leaves part of it unread.
            /// So walked to its end, true means all of it was read. A node
            /// reached a second time leaves it true, as it's been read; an
            /// iterator that gave an error has it false.
            pub fn whole(&self) -> bool {
                self.0.whole
            }

            /// The sink given, with what it was told.
            pub fn into_sink(self) -> S {
                self.0.sink
            }
        }

        impl<R, S> fmt::Debug for $walk<'_, R, S> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($walk))
                    .field("whole", &self.0.whole)
                    .finish_non_exhaustive()
            }
        }
    };
}

walks!(Names, &'o [u8]);
walks!(Numbers, i64);

/// A kind of tree: what its keys are, and the entry that holds them.
trait Key<'o>: Copy + Ord {
    /// The entry of a node that holds its keys and values.
    const ENTRIES: &'static str;

    /// The key `object`, its references followed, is: or what's wrong.
    fn read(object: &'o Object) -> Result<Self, Fault>;
}

impl<'o> Key<'o> for &'o [u8] {
    const ENTRIES: &'static str = "Names";

    fn read(object: &'o Object) -> Result<&'o [u8], Fault> {
        match object.value() {
            Value::String(string, _) => string.as_bytes().ok_or(Fault::TreeKeyUnread),
            _ => Err(Fault::TreeKeyKind),
        }
    }
}

impl<'o> Key<'o> for i64 {
    const ENTRIES: &'static str = "Nums";

    fn read(object: &'o Object) -> Result<i64, Fault> {
        object.as_integer().ok_or(Fault::TreeKeyKind)
    }
}

/// A node being read.
struct Node<'o, K> {
    /// Its dictionary, until its `/Kids` is read, after its own entries.
    unread: Option<&'o Dict>,
    /// The kids still to read.
    kids: &'o [Object],
    /// How many levels below the root it is.
    depth: usize,
    /// The numbered object it's in.
    object: Option<ObjectId>,
    /// Its `/Limits`, when a sink wants them.
    limits: Option<(K, K)>,
}

/// What following a node, or an array of one, comes to.
enum Reached<'o> {
    /// An object, and the number of the last of the references to it.
    Object(&'o Object, Option<ObjectId>),
    /// Null, or a reference to nothing.
    Nothing,
    /// Nothing that's lost: it's been read, and said.
    Repeated,
    /// An object that doesn't read, said.
    Damaged,
}

/// The walk, over either kind of tree.
struct Walk<'o, R, S, K> {
    objects: R,
    sink: S,
    limits: Limits,
    /// The root, until it's asked for its first entry.
    root: Option<&'o Object>,
    /// The nodes from the root down to the one being read.
    path: Vec<Node<'o, K>>,
    /// What's left of that node's keys and values.
    entries: &'o [Object],
    /// The numbered object what's being read is in.
    object: Option<ObjectId>,
    /// Every object reached by reference as a node or an array of one.
    seen: HashSet<ObjectId>,
    /// How many nodes it's read.
    nodes: usize,
    /// The last key given, when a sink wants faults.
    last: Option<K>,
    whole: bool,
    done: bool,
}

impl<'o, R: Resolve<'o>, S: Sink<Fault>, K: Key<'o>> Walk<'o, R, S, K> {
    fn new(objects: R, root: &'o Object, limits: &Limits, sink: S) -> Walk<'o, R, S, K> {
        Walk {
            objects,
            sink,
            limits: *limits,
            root: Some(root),
            path: Vec::new(),
            entries: &[],
            object: None,
            seen: HashSet::new(),
            nodes: 0,
            last: None,
            whole: true,
            done: false,
        }
    }

    fn next(&mut self) -> Option<Result<(K, &'o Object), Unreadable>> {
        if self.done {
            return None;
        }
        let next = self.step();
        match next {
            Ok(Some(entry)) => Some(Ok(entry)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(unreadable) => {
                self.done = true;
                self.whole = false;
                Some(Err(unreadable))
            }
        }
    }

    /// The next entry, reading what it takes to reach it.
    fn step(&mut self) -> Result<Option<(K, &'o Object)>, Unreadable> {
        if let Some(root) = self.root.take() {
            self.enter(root, 0)?;
        }
        loop {
            match self.entries {
                [key, value, rest @ ..] => {
                    self.entries = rest;
                    if let Some(read) = self.key(key)? {
                        self.check(read, key.at);
                        return Ok(Some((read, value)));
                    }
                    continue;
                }
                [key] => {
                    self.entries = &[];
                    self.lost(key.at, Fault::TreeOddKey);
                    continue;
                }
                [] => {}
            }
            let Some(node) = self.path.last_mut() else {
                return Ok(None);
            };
            self.object = node.object;
            if let Some(unread) = node.unread.take() {
                let kids = self.array(unread, "Kids")?;
                if let Some(node) = self.path.last_mut() {
                    node.kids = kids;
                    // What's said of a kid is in the array that holds it.
                    node.object = self.object;
                }
                continue;
            }
            let (kid, rest) = match node.kids {
                [kid, rest @ ..] => (kid, rest),
                [] => {
                    self.path.pop();
                    continue;
                }
            };
            node.kids = rest;
            let depth = node.depth + 1;
            self.enter(kid, depth)?;
        }
    }

    /// Reads the node `value` leads to, `depth` levels below the root, down
    /// to its own entries; or says why not.
    fn enter(&mut self, value: &'o Object, depth: usize) -> Result<(), Unreadable> {
        let root = depth == 0;
        if depth > self.limits.tree_depth {
            self.lost(value.at, Fault::TreeTooDeep);
            return Ok(());
        }
        if self.nodes >= self.limits.tree_nodes {
            self.lost(value.at, Fault::TreeTooManyNodes);
            self.path.clear();
            return Ok(());
        }
        let (node, id) = match self.once(value)? {
            Reached::Object(node, id) => (node, id),
            Reached::Nothing if root => return Ok(()),
            Reached::Nothing => {
                self.lost(value.at, Fault::TreeNodeNotDictionary);
                return Ok(());
            }
            Reached::Repeated | Reached::Damaged => return Ok(()),
        };
        self.object = id.or(self.object);
        let Value::Dict(dict) = node.value() else {
            self.lost(node.at, Fault::TreeNodeNotDictionary);
            return Ok(());
        };
        self.nodes += 1;
        let limits = if self.sink.wants() {
            if !root && id.is_none() {
                self.say(node.at, Fault::TreeKidDirect);
            }
            self.shape(dict, node.at, root)
        } else {
            None
        };
        self.path.push(Node {
            unread: Some(dict),
            kids: &[],
            depth,
            object: self.object,
            limits,
        });
        self.entries = self.array(dict, K::ENTRIES)?;
        Ok(())
    }

    /// Says what's wrong with the shape of a node, and gives its `/Limits`.
    /// Its `/Kids` and its entries are followed: null, or a reference to
    /// nothing, is none (7.3.10), and anything else is there, an object that
    /// doesn't read among it, for the walk's own read of it to say what's
    /// wrong. Its `/Limits` is there as written, null aside, and read by
    /// [`bounds`](Self::bounds).
    fn shape(&mut self, node: &'o Dict, at: Option<Span>, root: bool) -> Option<(K, K)> {
        let objects = &self.objects;
        let held = |key: &str| {
            (node.get(key.as_bytes())).is_some_and(|o| !matches!(followed(objects, o), Ok(None)))
        };
        let (kids, entries) = (held("Kids"), held(K::ENTRIES));
        match (kids, entries) {
            (true, true) => self.say(at, Fault::TreeBothEntries),
            (false, false) => self.say(at, Fault::TreeNoEntries),
            _ => {}
        }
        let has = |key: &str| node.get(key.as_bytes()).is_some_and(|o| !o.is_null());
        match (root, has("Limits")) {
            (true, true) => self.say(at, Fault::TreeRootLimits),
            (false, false) => self.say(at, Fault::TreeNoLimits),
            (false, true) => return self.bounds(node),
            (true, false) => {}
        }
        None
    }

    /// A node's `/Limits`, none where they aren't known; what's wrong with
    /// them said, as a sink's the only one that asks. Where they don't read
    /// past a cap or for a password, they aren't checked, and that's said.
    fn bounds(&mut self, node: &'o Dict) -> Option<(K, K)> {
        let written = node.get(b"Limits")?;
        let bad = |walk: &mut Self| {
            walk.say(written.at, Fault::TreeBadLimits);
            None
        };
        let limits = self.for_check(written).ok()?;
        let [least, greatest] = limits.and_then(Object::as_array).unwrap_or_default() else {
            return bad(self);
        };
        let (least, greatest) = (self.for_check(least), self.for_check(greatest));
        let (Ok(least), Ok(greatest)) = (least, greatest) else {
            return None;
        };
        match (least.map(K::read), greatest.map(K::read)) {
            (Some(Ok(least)), Some(Ok(greatest))) if least <= greatest => Some((least, greatest)),
            (Some(Err(Fault::TreeKeyUnread)), _) | (_, Some(Err(Fault::TreeKeyUnread))) => None,
            _ => bad(self),
        }
    }

    /// `written` followed for a check, which nothing else reads: none where
    /// it names nothing or doesn't read, which the check holds as wrong; an
    /// error where an object on the way is past a cap or takes a password,
    /// said as not checked.
    fn for_check(&mut self, written: &'o Object) -> Result<Option<&'o Object>, ()> {
        match followed(&self.objects, written) {
            Ok(object) => Ok(object),
            Err(unreadable) if unreadable.why == Why::Damaged => Ok(None),
            Err(unreadable) => {
                let (id, why) = (unreadable.id, unreadable.why);
                self.say(written.at, Fault::Unchecked { id, why });
                Err(())
            }
        }
    }

    /// What `key` of a node is, when it's an array: none when the node has
    /// none, or it's lost, and said.
    fn array(&mut self, node: &'o Dict, key: &'static str) -> Result<&'o [Object], Unreadable> {
        let Some(written) = node.get(key.as_bytes()) else {
            return Ok(&[]);
        };
        match self.once(written)? {
            Reached::Object(array, id) => {
                self.object = id.or(self.object);
                match array.value() {
                    Value::Array(items) => Ok(items),
                    _ => {
                        self.lost(array.at, Fault::TreeNotArray { key });
                        Ok(&[])
                    }
                }
            }
            Reached::Nothing | Reached::Repeated | Reached::Damaged => Ok(&[]),
        }
    }

    /// `value` followed, unless a reference on the way has been followed
    /// before, as a node or an array of one: so a walk reads each once,
    /// however often it's listed, and ends.
    fn once(&mut self, value: &'o Object) -> Result<Reached<'o>, Unreadable> {
        let (mut at, mut last) = (value, None);
        for _ in 0..HOPS {
            match at.value() {
                Value::Ref(id) => {
                    last = Some(*id);
                    if !self.seen.insert(*id) {
                        self.say(at.at, Fault::TreeRepeated { id: *id });
                        return Ok(Reached::Repeated);
                    }
                    match self.objects.try_resolve(*id) {
                        Ok(Some(object)) => at = object,
                        Ok(None) => return Ok(Reached::Nothing),
                        Err(unreadable) if unreadable.why == Why::Damaged => {
                            self.lost(at.at, Fault::Damaged { id: *id });
                            return Ok(Reached::Damaged);
                        }
                        Err(unreadable) => return Err(unreadable),
                    }
                }
                Value::Null => return Ok(Reached::Nothing),
                _ => return Ok(Reached::Object(at, last)),
            }
        }
        Err(crate::too_long(last.unwrap_or(ObjectId::new(0, 0))))
    }

    /// The key written as `written`, its references followed; none when it
    /// isn't one, and said.
    fn key(&mut self, written: &'o Object) -> Result<Option<K>, Unreadable> {
        let key = match followed(&self.objects, written) {
            Ok(key) => key,
            Err(unreadable) if unreadable.why == Why::Damaged => {
                let id = unreadable.id;
                self.lost(written.at, Fault::Damaged { id });
                return Ok(None);
            }
            Err(unreadable) => return Err(unreadable),
        };
        let fault = match key.map(K::read) {
            Some(Ok(key)) => return Ok(Some(key)),
            Some(Err(fault)) => fault,
            None => Fault::TreeKeyKind,
        };
        self.lost(written.at, fault);
        Ok(None)
    }

    /// Holds `key`, written at `at` and about to be given, to the key before
    /// it and the `/Limits` over it, when a sink wants to know.
    fn check(&mut self, key: K, at: Option<Span>) {
        if !self.sink.wants() {
            return;
        }
        if self.last.is_some_and(|last| key <= last) {
            self.say(at, Fault::TreeKeyOrder);
        }
        self.last = Some(key);
        let within = |node: &Node<'o, K>| {
            (node.limits).is_none_or(|(least, greatest)| least <= key && key <= greatest)
        };
        if !self.path.iter().all(within) {
            self.say(at, Fault::TreeKeyOutsideLimits);
        }
    }

    /// Says `fault`, at something that leaves part of the tree unread.
    fn lost(&mut self, at: Option<Span>, fault: Fault) {
        self.whole = false;
        self.say(at, fault);
    }

    fn say(&mut self, at: Option<Span>, fault: Fault) {
        if self.sink.wants() {
            self.sink.fault(Place::new(self.object, at), fault);
        }
    }
}

/// `entries` written as a name tree's root: a dictionary with one `/Names`,
/// in the order 7.9.6 has, by their bytes. A value's form is the caller's, a
/// reference or written in place. Reading it back gives the same entries,
/// sorted, with no fault.
pub fn write_names(entries: impl IntoIterator<Item = (Vec<u8>, Object)>) -> Result<Dict, Error> {
    let mut entries: Vec<_> = entries.into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some(pair) = entries.windows(2).find(|pair| pair[0].0 == pair[1].0) {
        let key = pair[0].0.clone();
        return Err(Error::RepeatedName { key });
    }
    written(
        "Names",
        entries.into_iter().map(|(k, v)| (Object::bytes(k), v)),
    )
}

/// `entries` written as a number tree's root: a dictionary with one `/Nums`,
/// in ascending order (7.9.7), as [`write_names`] writes a name tree's.
pub fn write_numbers(entries: impl IntoIterator<Item = (i64, Object)>) -> Result<Dict, Error> {
    let mut entries: Vec<_> = entries.into_iter().collect();
    entries.sort_by_key(|entry| entry.0);
    if let Some(pair) = entries.windows(2).find(|pair| pair[0].0 == pair[1].0) {
        let key = pair[0].0;
        return Err(Error::RepeatedNumber { key });
    }
    written(
        "Nums",
        entries.into_iter().map(|(k, v)| (Object::from(k), v)),
    )
}

/// A root whose `holder` holds `entries`, in their order.
fn written(holder: &str, entries: impl Iterator<Item = (Object, Object)>) -> Result<Dict, Error> {
    let mut array = Vec::with_capacity(entries.size_hint().0.saturating_mul(2));
    for (key, value) in entries {
        if holds_a_stream(&value) {
            return Err(Error::StreamValue);
        }
        array.push(key);
        array.push(value);
    }
    let mut root = Dict::new();
    root.set(holder, array);
    Ok(root)
}

/// Whether `value` is a stream or has one inside it, however deep.
fn holds_a_stream(value: &Object) -> bool {
    let mut held = vec![value];
    while let Some(value) = held.pop() {
        match value.value() {
            Value::Stream(_) => return true,
            Value::Array(items) => held.extend(items),
            Value::Dict(dict) => held.extend(dict.iter().map(|(_, value)| value)),
            _ => {}
        }
    }
    false
}

/// Why a tree can't be written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A name tree's key given twice, which 7.9.6 doesn't let a tree hold,
    /// nor a reader tell which is meant.
    #[error("the name tree's key {key:?} is given twice")]
    RepeatedName {
        /// The key's bytes.
        key: Vec<u8>,
    },
    /// A number tree's key given twice.
    #[error("the number tree's key {key} is given twice")]
    RepeatedNumber {
        /// The key.
        key: i64,
    },
    /// A value that's a stream or has one inside it, which is written only as
    /// an indirect object (7.3.8): give a reference to it.
    #[error("a value of the tree holds a stream, where only a reference to one can be written")]
    StreamValue,
}

#[cfg(test)]
pub(crate) mod tests;
