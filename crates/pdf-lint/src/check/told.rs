//! What a read of a color space or a font tells that the walk can't say,
//! found where the walk found nothing by the model's rules ([`MODEL`]):
//! in the fault's object at its key, at a reference to the object it's of
//! as a whole, or at the family's name of the space it's of; each kind once
//! in each object, in the order told ([`Walk::find_told`]). A kind told at
//! its bytes is at the entry or element whose value holds them
//! ([`holding`]).

use std::collections::HashSet;
use std::hash::Hash;

use super::{Finding, MODEL, Walk, entry_at};
use crate::object::{Object, ObjectId, Span, Value};

/// The innermost entry or element written in `value`, through what's
/// written in place in it, whose value holds the bytes `at`: its key or
/// position, and that value. None where none does.
pub(super) fn holding(value: &Object, at: Span) -> Option<(String, &Object)> {
    let holds = |o: &Object| {
        (o.at).is_some_and(|v| v.stream == at.stream && v.start <= at.start && at.end <= v.end)
    };
    let mut held = None;
    let mut here = value;
    loop {
        let entries = match here.value() {
            Value::Dict(dict) => Some(dict),
            Value::Stream(stream) => Some(&stream.dict),
            _ => None,
        };
        let next = match (entries, here.value()) {
            (Some(dict), _) => (dict.entries().iter())
                .find(|entry| holds(&entry.value))
                .map(|entry| {
                    (
                        String::from_utf8_lossy(&entry.key).into_owned(),
                        &entry.value,
                    )
                }),
            (None, Value::Array(array)) => (array.iter().enumerate())
                .find(|(_, element)| holds(element))
                .map(|(i, element)| (i.to_string(), element)),
            (None, _) => None,
        };
        let Some((key, value)) = next else {
            return held;
        };
        here = value;
        held = Some((key, value));
    }
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
}

impl Walk<'_> {
    /// Finds each of `tells`, in order, where the walk found nothing by the
    /// model's rules in the newest revision: at its key in its object; of
    /// an object as a whole ([`Told::whole`]), at an entry or element
    /// holding a reference to it; and of a space's family
    /// ([`Told::family`]), at its family's name. Each object's kind once,
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
                    && told.insert((f.object, t.kind))
            })
            .map(|t| t.finding)
            .collect();
        self.report.findings.extend(new);
    }
}
