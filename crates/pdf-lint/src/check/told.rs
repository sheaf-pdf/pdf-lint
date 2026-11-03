//! What a read of a color space or a font tells that the walk can't say,
//! found where the walk found nothing by the model's rules ([`MODEL`]):
//! in the fault's object at its key, at a reference to the object it's of
//! as a whole, at the family's name of the space it's of, or in the value
//! it's of where the model checks its elements each; each kind once in
//! each object, in the order told ([`Walk::find_told`]). A kind told at its
//! bytes is at the entry or element whose value holds them ([`holding`]),
//! its path and table those of what that entry or element is written in
//! ([`Walk::path_and_table`]).

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::ptr;

use super::{Finding, Graphic, MODEL, REFERENCES, Visited, Walk, entry_at};
use crate::object::{Object, ObjectId, Span, Value};

/// Whether `outer`, where a value was written, holds the bytes `at`.
fn holds(outer: Option<Span>, at: Span) -> bool {
    outer.is_some_and(|v| v.stream == at.stream && v.start <= at.start && at.end <= v.end)
}

/// Each entry or element written in `value`, through what's written in
/// place in it, whose value holds the bytes `at`, the outermost first: its
/// key or position, the dictionary, stream or array it's written in, and
/// its value.
pub(super) fn holders(value: &Object, at: Span) -> Vec<(String, &Object, &Object)> {
    let mut held = Vec::new();
    let mut here = value;
    loop {
        let entries = match here.value() {
            Value::Dict(dict) => Some(dict),
            Value::Stream(stream) => Some(&stream.dict),
            _ => None,
        };
        let next = match (entries, here.value()) {
            (Some(dict), _) => (dict.entries().iter())
                .find(|entry| holds(entry.value.at, at))
                .map(|entry| {
                    (
                        String::from_utf8_lossy(&entry.key).into_owned(),
                        &entry.value,
                    )
                }),
            (None, Value::Array(array)) => (array.iter().enumerate())
                .find(|(_, element)| holds(element.at, at))
                .map(|(i, element)| (i.to_string(), element)),
            (None, _) => None,
        };
        let Some((key, value)) = next else {
            return held;
        };
        held.push((key, here, value));
        here = value;
    }
}

/// The innermost entry or element written in `value`, through what's
/// written in place in it, whose value holds the bytes `at`: its key or
/// position, and that value. None where none does.
pub(super) fn holding(value: &Object, at: Span) -> Option<(String, &Object)> {
    (holders(value, at).pop()).map(|(key, _, value)| (key, value))
}

/// The value of the entry or element written in `value`, through what's
/// written in place in it, whose entry, its key and value together, or
/// element is at the bytes `at`, as the walk's findings are.
pub(super) fn written_at(value: &Object, at: Span) -> Option<&Object> {
    let here = holding(value, at).map_or(value, |(_, value)| value);
    if here.at == Some(at) {
        return Some(here);
    }
    let dict = match here.value() {
        Value::Dict(dict) => dict,
        Value::Stream(stream) => &stream.dict,
        _ => return None,
    };
    (dict.entries().iter())
        .find(|entry| entry_at(entry) == Some(at))
        .map(|entry| &entry.value)
}

/// What a read told, as it's found where the walk found nothing at its key
/// ([`Walk::find_told`]).
pub(super) struct Told<K> {
    pub(super) finding: Finding,
    /// Its kind, told once in each object: a variant of the fault of the
    /// crate whose read told it.
    pub(super) kind: K,
    /// The numbered object whose value, as a whole, it's of, which a read
    /// reaches by a reference: the walk checks what a reference leads to at
    /// the key holding the reference.
    pub(super) whole: Option<ObjectId>,
    /// Where the family's name of the space it's of is written, the object
    /// and the bytes of its element 0, for a kind of what family a space
    /// is told at an array.
    pub(super) family: Option<(Option<ObjectId>, Span)>,
    /// Where the value it's of is written, the object and its bytes, for a
    /// kind of what an array at its key is as a whole, its count and its
    /// elements, whose elements the model checks each: what the walk found
    /// at or inside those bytes says it.
    pub(super) inside: Option<(Option<ObjectId>, Span)>,
}

/// Which entry or element a finding's key is of, for its path and table
/// ([`Walk::path_and_table`]).
#[derive(Clone, Copy)]
pub(super) enum Keyed<'k> {
    /// The innermost holding its bytes ([`holding`]).
    Holding,
    /// The dictionary's entry of that key, innermost, that holds its bytes,
    /// else, the key absent, one of the dictionary at its bytes: a kind
    /// about one key of a dictionary.
    Entry(&'k str),
    /// An element of the array at its bytes.
    Element,
}

impl<'a> Walk<'a> {
    /// What's written as object `object`, else the trailer; none where it
    /// doesn't read.
    pub(super) fn written(&self, object: Option<ObjectId>) -> Option<&'a Object> {
        match object {
            Some(id) => self.source.get(id).ok(),
            None => Some(&self.source.trailer().object),
        }
    }

    /// Where the value written at the bytes `at` in `object` is, through
    /// its references to what reads: the object it comes to and its bytes,
    /// where it's written whole at `at` and comes to no dictionary or
    /// stream, which a kind of a key absent is told at ([`Told::inside`]).
    pub(super) fn value_at(
        &self,
        object: Option<ObjectId>,
        at: Option<Span>,
    ) -> Option<(Option<ObjectId>, Span)> {
        let (written, at) = (self.written(object)?, at?);
        let mut there = holding(written, at).map_or(written, |(_, value)| value);
        if there.at != Some(at) {
            return None;
        }
        let mut object = object;
        for _ in 0..REFERENCES {
            let Some(id) = there.as_reference() else {
                break;
            };
            let Ok(next) = self.source.get(id) else {
                break;
            };
            (object, there) = (Some(id), next);
        }
        match there.value() {
            Value::Dict(_) | Value::Stream(_) => None,
            _ => Some((object, there.at?)),
        }
    }

    /// The path and table of what's found at the bytes `at` in `object`,
    /// written there as `written`, at the key `keyed` says: of the
    /// dictionary or array written in place in it whose entry or element
    /// that is, where the walk visited it as one of what a space or a font
    /// is or holds ([`Walk::parts_in`]); else `first`, the walk's first
    /// visit of the object as one of those, else `graphic`'s, the space's
    /// or the font's read.
    pub(super) fn path_and_table(
        &self,
        object: Option<ObjectId>,
        written: Option<&Object>,
        at: Option<Span>,
        keyed: Keyed<'_>,
        first: Option<&Visited<'a>>,
        graphic: &Graphic<'a>,
    ) -> (String, &'static str) {
        let container = written.zip(at).and_then(|(written, at)| {
            let held = holders(written, at);
            // The value at `at`, where it's written whole there.
            let there = match held.last() {
                Some(&(_, _, value)) => value,
                None => written,
            };
            let there = (there.at == Some(at)).then_some(there);
            let container = match keyed {
                Keyed::Holding => held.last().map_or(written, |&(_, container, _)| container),
                Keyed::Entry(key) => match held.iter().rev().find(|(k, ..)| k == key) {
                    Some(&(_, container, _)) => container,
                    None => there?,
                },
                Keyed::Element => there?,
            };
            // Written in place, not the object as a whole.
            (!ptr::eq(container, written)).then_some(container)
        });
        let visited = container
            .and_then(|container| self.parts_in.get(&(object, container.at?)))
            .or(first);
        match visited {
            Some(visited) => (visited.trail.path(), visited.table),
            None => (graphic.trail.path(), graphic.table),
        }
    }

    /// Finds each of `tells`, in order, where the walk found nothing by the
    /// model's rules in the newest revision: at its key in its object; of
    /// an object as a whole ([`Told::whole`]), at an entry or element
    /// holding a reference to it; of a space's family ([`Told::family`]),
    /// at its family's name; and of an array's count and elements
    /// ([`Told::inside`]), at or inside the array. Each object's kind once,
    /// so that what two reads tell, a space and its base, or two fonts of
    /// one descriptor, is found once.
    pub(super) fn find_told<K: Copy + Eq + Hash>(&mut self, tells: Vec<Told<K>>) {
        if tells.is_empty() {
            return;
        }
        let model = || {
            (self.report.findings.iter())
                .filter(|f| f.revision.is_none() && MODEL.contains(&f.rule))
        };
        let found: HashSet<(Option<ObjectId>, &str)> =
            model().map(|f| (f.object, f.key.as_str())).collect();
        let sited: HashSet<(Option<ObjectId>, Span)> =
            match tells.iter().any(|t| t.family.is_some()) {
                true => model().filter_map(|f| Some((f.object, f.at?))).collect(),
                false => HashSet::new(),
            };
        let mut inner: HashMap<Option<ObjectId>, Vec<Span>> = HashMap::new();
        if tells.iter().any(|t| t.inside.is_some()) {
            for f in model() {
                if let Some(at) = f.at {
                    inner.entry(f.object).or_default().push(at);
                }
            }
        }
        let said_inside = |(object, value): (Option<ObjectId>, Span)| {
            (inner.get(&object)).is_some_and(|at| at.iter().any(|&at| holds(Some(value), at)))
        };
        let wholes: HashSet<ObjectId> = tells.iter().filter_map(|t| t.whole).collect();
        let referred: HashSet<ObjectId> = match wholes.is_empty() {
            true => HashSet::new(),
            false => model()
                .filter_map(|f| {
                    let written = self.source.get(f.object?).ok()?;
                    written_at(written, f.at?)?.as_reference()
                })
                .filter(|id| wholes.contains(id))
                .collect(),
        };
        let mut told = HashSet::new();
        let new: Vec<Finding> = (tells.into_iter())
            .filter(|t| {
                let f = &t.finding;
                !found.contains(&(f.object, f.key.as_str()))
                    && !t.whole.is_some_and(|id| referred.contains(&id))
                    && !t.family.is_some_and(|site| sited.contains(&site))
                    && !t.inside.is_some_and(said_inside)
                    && told.insert((f.object, t.kind))
            })
            .map(|t| t.finding)
            .collect();
        self.report.findings.extend(new);
    }
}
