//! Whether a file that says it's linearized holds (ISO 32000-2, Annex F),
//! as pdf-linearize's check judges it, from the facts its adapter reads of
//! the file's reading
//! ([`Reads::linearization`](super::reading::Reads::linearization)): so
//! only where the check has the reading
//! ([`check_with_reading`](super::check_with_reading)), as what's judged is
//! the file as written, its sections, its hint stream's data and where each
//! object is.
//!
//! A file says it's linearized as pdf-linearize takes it to before it tells
//! anything ([`Facts::claim`]): its first object a dictionary, not a stream,
//! with `/Linearized`, starting within 1,024 bytes of its header. One that
//! doesn't has nothing told, and no verdict
//! ([`Report::linearized`](super::Report::linearized)). One that's
//! linearized by pdf-linearize's test, its `/Linearized` a number whose
//! integer part is 1 and its `/L` its length, has what the check tells
//! found, and the verdict, holds or fails, by whether an error is among it.
//! One linearized as first written and updated after ([`first_written`]),
//! its `/L` its first revision's, has that found ([`Rule::Updated`]), in the
//! newest trailer, so that the update answers for it, and what the check
//! tells of the file as first written, its first bytes read as a file of
//! their own
//! ([`Reads::first_written`](super::reading::Reads::first_written)), found
//! as a linearized file's is, of what's held, where a place in a stream's
//! data whose number a later revision lists has no bytes; its verdict, that
//! it's updated. What pdf-linearize says of the whole, that `/L` isn't its
//! length and an update follows, isn't found, as that's what `updated` says.
//! One that isn't taken for linearized otherwise has what the check tells of
//! it found, what's wrong with `/Linearized` or `/L` as a value, an `/L`
//! that isn't the file's length nor its first revision's, and an update
//! after it, a departure; its verdict, that it isn't taken.
//!
//! What the check tells is a [`Rule::Linearization`] finding, an error
//! always ([`Grade::Error`]) and a departure that readers get past only
//! where [`Options::departures`](super::Options::departures) asks, else
//! counted by table and key
//! ([`Report::departures`](super::Report::departures)); each kind once in
//! the linearization dictionary's object, its table
//! `LinearizationParameterDict` and its path the dictionary's, at its first
//! place ([`Places`]), its value pdf-linearize's words, which cite the
//! clause, with how many times it was told, every one counted
//! ([`Kept::keep_each`]), held where the dictionary is; where a later
//! revision wrote the dictionary's number again, so that the object there is
//! another, the first revision's, in no object, held where every object is
//! ([`Whose`]). What's wrong with one of the dictionary's values as a value,
//! or `/H`'s count of items, is what the model holds the dictionary to too:
//! it's found only where the walk found nothing by the model's rules in the
//! dictionary at that key, or for `/H`'s count, at or inside its array
//! ([`Walk::find_told`]). What pdf-linearize doesn't judge
//! ([`Grade::Unjudged`]), a part of the file the reader didn't read for a
//! reason of its own, a hint stream whose data is encrypted and not
//! decrypted among it, sections or a page tree no key opens, past a cap of
//! the reader's, or through filters it doesn't take, or past a cap of the
//! check's own ([`LIMITS`]), and a file whose bytes the source didn't give,
//! are counted as not checked, once each, where the dictionary is held, and
//! never as a finding.

use std::collections::HashMap;

use pdf_linearize::Limits;
use pdf_linearize::check::{Facts, Finding as Fault, Grade, Place, Raw, Sink, Unread, Verdict};
use pdf_linearize::hint::located;

use super::content::times;
use super::kept::{Kept, Where};
use super::told::Told;
use super::{
    Finding, Hold, Linearized, Made, Rule, Walk, entry_at, first_object, linearization, object_path,
};
use crate::object::{Object, ObjectId, Span, Value};
use crate::source::{Revision, Source};

/// The table a linearization's findings are in: the model's for the
/// dictionary that says a file is linearized (Table F.1).
const TABLE: &str = "LinearizationParameterDict";

/// What pdf-linearize's check reads hint tables under: its caps, which ISO
/// 32000-2 doesn't set.
const LIMITS: Limits = Limits::DEFAULT;

/// Why a linearization isn't checked whole: its hint stream's data is
/// encrypted, and the reader didn't decrypt it.
const LOCKED: &str = "a linearization whose hint stream's data isn't decrypted";

/// Why a linearization isn't checked whole, for what pdf-linearize told it
/// didn't judge, `fault`: [`LOCKED`] for a hint stream no key opens, and
/// otherwise the part that wasn't read and why, in pdf-linearize's words for
/// the reason, or the part past a cap of its check's ([`LIMITS`]), its count
/// left out, as each file's would make a row of its own.
fn unjudged(fault: &Fault) -> String {
    match fault {
        Fault::HintUnread {
            why: Unread::Locked,
        } => LOCKED.to_string(),
        Fault::HintUnread { why } => format!("a linearization whose hint stream isn't read: {why}"),
        Fault::SectionsUnread { why } => {
            format!("a linearization whose sections aren't read as far as its main one: {why}")
        }
        Fault::PagesUnread { why } => format!("a linearization whose page tree isn't read: {why}"),
        Fault::HintTablesCapped { .. } => {
            "a linearization whose hint tables are past a cap of the check's".to_string()
        }
        Fault::GraphCapped { .. } => {
            "a linearization whose objects are past a cap of the check's".to_string()
        }
        _ => "a linearization not judged whole".to_string(),
    }
}

/// Why a linearization isn't checked: the source didn't give the bytes its
/// facts are read from.
const UNGIVEN: &str = "a linearization whose bytes the source didn't give";

/// Whether `fault` is of what the model holds the dictionary to as well:
/// a value of it missing, of another type or a reference, or `/H`'s count
/// of items. It gives way to what the walk found by the model's rules.
fn of_the_model(fault: &Fault) -> bool {
    matches!(
        fault,
        Fault::Version | Fault::BadValue { .. } | Fault::Indirect { .. } | Fault::HintItems { .. }
    )
}

/// How many bytes of the file were linearized as first written, where an
/// update after them voids that (ISO 32000-2, F.1, Table F.1): of a claim
/// whose `/Linearized` pdf-linearize takes
/// ([`FirstObject::version_one`](pdf_linearize::check::FirstObject::version_one)),
/// and whose `/L` is an integer that isn't the file's length from its
/// header, but that ends the first of `revisions` from there, at the
/// `%%EOF` after its last section or past the line end right after it
/// ([`Revision::end`]), and a later revision after it. None otherwise.
fn first_written(facts: &Facts, revisions: &[Revision]) -> Option<usize> {
    let first = facts.first_object.as_ref()?;
    let len = facts.header.checked_add(number(Some(&first.l))?)?;
    if !first.version_one() || len == facts.len || revisions.len() < 2 {
        return None;
    }
    let len = usize::try_from(len).ok()?;
    let end = revisions.first()?.end?;
    (end.eof()..=end.line()).contains(&len).then_some(len)
}

/// The key a finding at `place` is told at: the dictionary's key it's of,
/// `H` for what's of the hint stream, a page's entry in its tables among
/// it, `-` for the rest.
fn key(place: Option<Place>) -> String {
    match place {
        Some(Place::Key(key)) => key.to_string().trim_start_matches('/').to_string(),
        Some(
            Place::HintByte(_)
            | Place::HintBit(_)
            | Place::Table(_)
            | Place::HintKey(_)
            | Place::SharedEntry(_)
            | Place::Position(_)
            | Place::Page(_),
        ) => "H".to_string(),
        _ => "-".to_string(),
    }
}

/// `raw` as a byte or a length: a non-negative integer.
fn number(raw: Option<&Raw>) -> Option<u64> {
    match raw {
        Some(&Raw::Integer(n)) => u64::try_from(n).ok(),
        _ => None,
    }
}

/// The file a linearization's facts are read of, as the places they give
/// are found in it ([`Places`]): the file the check reads, or the file as
/// it was first written, read as a file of its own
/// ([`Reads::first_written`](super::reading::Reads::first_written)).
pub(super) trait Placing {
    /// How many bytes it has.
    fn size(&self) -> usize;

    /// Each object it lists, by number and generation.
    fn ids(&self) -> Vec<ObjectId>;

    /// Object `id`, where it reads.
    fn object(&self, id: ObjectId) -> Option<&Object>;

    /// Where object `id` was read from, its bytes.
    fn record(&self, id: ObjectId) -> Option<Span>;

    /// Stream `id`'s data, decoded, where it decodes cleanly.
    fn decoded(&self, id: ObjectId) -> Option<Vec<u8>>;
}

/// The file the check reads, as its source has it.
struct Checked<'a>(&'a dyn Source);

impl Placing for Checked<'_> {
    fn size(&self) -> usize {
        self.0.size().unwrap_or(0)
    }

    fn ids(&self) -> Vec<ObjectId> {
        self.0.ids()
    }

    fn object(&self, id: ObjectId) -> Option<&Object> {
        self.0.get(id).ok()
    }

    fn record(&self, id: ObjectId) -> Option<Span> {
        self.0.record(id).map(|record| record.at)
    }

    fn decoded(&self, id: ObjectId) -> Option<Vec<u8>> {
        self.0.decoded_within(id, usize::MAX)?.ok()
    }
}

/// Whether one of `later`, revisions after the one a linearization's facts
/// are of, lists number `n`, or may: one whose numbers aren't known.
fn listed_in(later: &[Revision], n: u32) -> bool {
    (later.iter())
        .any(|r| (r.numbers.as_ref()).is_none_or(|numbers| numbers.binary_search(&n).is_ok()))
}

/// Where each place pdf-linearize gives a finding is in the file, as a
/// finding has its bytes ([`Places::at`]).
struct Places<'a> {
    source: &'a dyn Placing,
    /// The revisions after the one the facts are of: a place in a stream's
    /// data whose number one of them lists has no bytes, as the file's
    /// stream at that number is another.
    later: &'a [Revision],
    /// The linearization dictionary, as the file the facts are of has it
    /// at its number.
    dict: Option<&'a Object>,
    /// The hint stream's number, where something's listed at `/H`.
    hint: Option<u32>,
    /// The hint stream and how many bytes its data decodes to, as the source
    /// decodes it again: none where it doesn't decode cleanly. Read the
    /// first time a place in it is asked for.
    data: Option<Option<(ObjectId, usize)>>,
    /// Where the hint stream is and how long, as a hint table's positions
    /// are as if it weren't there from ([`Facts::hint_extent`]).
    stream: (u64, u64),
    /// Where each of the hint stream's tables starts in its data, by its key
    /// in the stream's dictionary, the page offset table at 0.
    tables: Vec<(Vec<u8>, u64)>,
    /// Each number's object, by number, read the first time an object's
    /// place is asked for.
    numbers: Option<HashMap<u32, ObjectId>>,
    /// Each page's object's number, in page order, where it has one.
    pages: Vec<Option<u32>>,
}

impl<'a> Places<'a> {
    /// The places of what `facts`, of the file `source` reads, say, its
    /// linearization dictionary the object at `dict`, `later` the
    /// revisions after it.
    fn new(
        source: &'a dyn Placing,
        later: &'a [Revision],
        facts: &Facts,
        dict: ObjectId,
    ) -> Places<'a> {
        let hint = facts.hint.as_ref();
        let mut tables = vec![(b"page offset".to_vec(), 0)];
        for (key, raw) in hint.iter().flat_map(|h| &h.keys) {
            if let Some(at) = number(Some(raw)) {
                tables.push((key.clone(), at));
            }
        }
        Places {
            source,
            later,
            dict: source.object(dict),
            hint: hint.map(|h| h.number),
            data: None,
            stream: facts.hint_extent(),
            tables,
            numbers: None,
            pages: (facts.pages.iter())
                .map(|p| p.id.map(|id| id.number))
                .collect(),
        }
    }

    /// Where `fault` is said of, as pdf-linearize places it, but that what
    /// a page doesn't say itself, which it places at the page by its place
    /// among the pages, is of the page's object, or of the file as a whole
    /// where the page has none of its own: a page by its place is its
    /// entry in the hint tables otherwise.
    fn of(&self, fault: &Fault) -> Option<Place> {
        match *fault {
            Fault::PageAttribute { page, .. } => {
                let page = usize::try_from(page).ok().and_then(|p| self.pages.get(p));
                page.copied().flatten().map(Place::Object)
            }
            _ => fault.place(),
        }
    }

    /// The object at number `n`, by its generation in the file.
    fn id(&mut self, n: u32) -> Option<ObjectId> {
        let source = self.source;
        let numbers = self.numbers.get_or_insert_with(|| {
            (source.ids().into_iter())
                .map(|id| (id.number, id))
                .collect()
        });
        numbers.get(&n).copied()
    }

    /// The bytes `place` gives, as the module says: none where there are
    /// none, they're past what was read, or they're in the data of a
    /// stream whose number a later revision lists.
    fn at(&mut self, place: Option<Place>) -> Option<Span> {
        let at = self.placed(place)?;
        let later = |id: ObjectId| listed_in(self.later, id.number);
        (!at.stream.is_some_and(later)).then_some(at)
    }

    /// The bytes `place` gives in the file the facts are of.
    fn placed(&mut self, place: Option<Place>) -> Option<Span> {
        let len = self.source.size();
        let byte = |at: u64| {
            let at = usize::try_from(at).ok().filter(|&at| at < len)?;
            Some(Span::new(None, at, at + 1))
        };
        let dict = self.dict.and_then(|d| d.at);
        match place {
            Some(Place::Key(key)) => {
                let name = key.to_string();
                let entries = self.dict.and_then(Object::as_dict);
                let entry = entries.and_then(|d| {
                    let name = name.trim_start_matches('/').as_bytes();
                    (d.entries().iter()).find(|e| &e.key[..] == name)
                });
                entry.and_then(entry_at).or(dict)
            }
            Some(Place::HintByte(at)) => self.data_at(at),
            Some(Place::HintBit(bit)) => self.data_at(bit / 8),
            Some(Place::Table(table)) => {
                let at = self.table(table.trim_start_matches('/').as_bytes())?;
                self.data_at(at)
            }
            Some(Place::Page(_)) => self.data_at(0),
            Some(Place::SharedEntry(_)) => {
                let at = self.table(b"S")?;
                self.data_at(at)
            }
            Some(Place::HintKey(key)) => {
                let id = self.id(self.hint?)?;
                let written = self.source.object(id)?;
                let Value::Stream(stream) = written.value() else {
                    return None;
                };
                (stream.dict.entries().iter())
                    .find(|e| &e.key[..] == key.as_bytes())
                    .and_then(entry_at)
            }
            Some(Place::Position(position)) => {
                let (at, length) = self.stream;
                byte(located(position, at, length))
            }
            Some(Place::Object(n)) => {
                let id = self.id(n)?;
                self.source.record(id)
            }
            Some(Place::Byte(at)) => byte(at),
            _ => dict,
        }
    }

    /// Where the hint stream's table of `key` starts in its data.
    fn table(&self, key: &[u8]) -> Option<u64> {
        (self.tables.iter())
            .find(|(k, _)| &k[..] == key)
            .map(|&(_, at)| at)
    }

    /// The byte `at` of the hint stream's data, where it decodes cleanly to
    /// that many bytes, as the reading decodes it for a finding's bytes.
    fn data_at(&mut self, at: u64) -> Option<Span> {
        if self.data.is_none() {
            let id = self.hint.and_then(|n| self.id(n));
            let source = self.source;
            self.data = Some(id.and_then(|id| Some((id, source.decoded(id)?.len()))));
        }
        let (id, len) = self.data.flatten()?;
        let at = usize::try_from(at).ok().filter(|&at| at < len)?;
        Some(Span::new(Some(id), at, at + 1))
    }
}

/// Whose a linearization's findings are, and so where they're held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Whose {
    /// The linearization dictionary's, the object at its number, held where
    /// that is: it's the file's first object as first written.
    Dictionary,
    /// The first revision's, as a later revision wrote the dictionary's
    /// number over or freed it, so that the object there is another: held
    /// where every object is, as an earlier revision's trailer is, in no
    /// object.
    First,
}

/// What pdf-linearize's check tells, kept ([`Kept`]), each finding in the
/// linearization dictionary's object at the bytes its place gives
/// ([`Places::at`]), and counted, every one ([`Kept::keep_each`]).
struct Faults<'a> {
    kept: Kept<Fault>,
    places: Places<'a>,
    /// The linearization dictionary's number.
    object: ObjectId,
}

impl Sink for Faults<'_> {
    fn finding(&mut self, finding: Fault) {
        let at = self.places.at(self.places.of(&finding));
        let at = Where {
            object: Some(self.object),
            at,
        };
        self.kept.keep_each(at, finding);
    }
}

impl<'a> Walk<'a> {
    /// Whether a file that says it's linearized holds, as pdf-linearize's
    /// check judges it, where the check has the file's reading: what it
    /// tells found, and its verdict
    /// ([`Report::linearized`](super::Report::linearized)), as the module's
    /// documentation says. Of a file linearized as first written and
    /// updated after, that it was ([`Rule::Updated`]), and what the check
    /// tells of the file as first written.
    pub(super) fn linearized(&mut self) {
        let Some(reads) = self.reads else {
            return;
        };
        let facts = match reads.linearization() {
            Ok(facts) => facts,
            Err(_) => {
                // The source failing, which a file in memory never does:
                // counted where the file's first object would be told of.
                let first = first_object(self.source).filter(|&(_, o)| linearization(o));
                if let Some((id, _)) = first {
                    self.ungiven(id, Whose::Dictionary);
                }
                return;
            }
        };
        let Some(first) = facts.claim() else {
            return;
        };
        let id = ObjectId::new(first.number, first.generation);
        let revisions = self.source.revisions();
        // The object at the dictionary's number is the dictionary where its
        // record starts where the first object does, else a later
        // revision's.
        let record = self.source.record(id).map(|r| r.at);
        let at = usize::try_from(first.at).ok();
        let whose = match record.is_some_and(|r| r.stream.is_none() && Some(r.start) == at) {
            false if revisions.len() > 1 => Whose::First,
            _ => Whose::Dictionary,
        };
        if let Some(len) = first_written(&facts, revisions) {
            self.updated(len, revisions);
            let held = match reads.first_written(len) {
                Ok((facts, file)) => {
                    let places = Places::new(file, &revisions[1..], &facts, id);
                    let verdict = self.judged(&facts, places, id, whose);
                    verdict.linearized && verdict.holds
                }
                Err(_) => {
                    self.ungiven(id, whose);
                    false
                }
            };
            self.report.linearized = Some(Linearized::Updated { held });
            return;
        }
        let newest = Checked(self.source);
        let places = Places::new(&newest, &[], &facts, id);
        let verdict = self.judged(&facts, places, id, whose);
        self.report.linearized = Some(match (verdict.linearized, verdict.holds) {
            (false, _) => Linearized::NotTaken,
            (true, true) => Linearized::Holds,
            (true, false) => Linearized::Fails,
        });
    }

    /// That the file was linearized as first written, its first `len`
    /// bytes, and that the update after them, the second of `revisions`,
    /// voids that ([`Rule::Updated`]): in the newest trailer, its table and
    /// key `-`, at its bytes, held as the trailer is.
    fn updated(&mut self, len: usize, revisions: &[Revision]) {
        let Some(made) = self.made(self.trailer_id) else {
            return;
        };
        let written = &self.source.trailer().object;
        let table = match written.value() {
            Value::Stream(_) => "XRefStream",
            _ => "FileTrailer",
        };
        let at = revisions.get(1).map_or(0, |r| r.at);
        self.report.findings.push(Finding {
            object: self.trailer_id,
            made,
            path: "trailer".to_string(),
            table: table.to_string(),
            key: "-".to_string(),
            rule: Rule::Updated,
            value: format!(
                "the file was linearized as first written, its first {len} bytes; the update after it, its section at byte {at}, voids that: it's read as an ordinary file (ISO 32000-2, F.1, Table F.1)"
            ),
            at: written.at,
            revision: None,
        });
    }

    /// How a linearization's findings are made, whose they are, its
    /// dictionary at `id`'s or the first revision's ([`Whose`]), where
    /// they're held.
    fn made_as(&self, id: ObjectId, whose: Whose) -> Option<Made> {
        match (whose, self.hold) {
            (Whose::Dictionary, _) => self.made(Some(id)),
            (Whose::First, Hold::All(_)) => Some(Made::Merged),
            (Whose::First, Hold::Only(_)) => None,
        }
    }

    /// Counted as not checked, where the linearization, its dictionary at
    /// `id`, is held ([`Whose`]): the bytes its facts are read from that
    /// the source didn't give.
    fn ungiven(&mut self, id: ObjectId, whose: Whose) {
        if self.made_as(id, whose).is_some() {
            self.unchecked("-", UNGIVEN.to_string(), TABLE, "-");
        }
    }

    /// What pdf-linearize's check tells of `facts`, at `places`, found as
    /// the module says, where the linearization, its dictionary at `id`, is
    /// held ([`Whose`]); and its verdict. The first revision's are in no
    /// object, that revision's ([`Finding::revision`]), and stand as they
    /// are, as the walk never read the dictionary as that revision had it.
    fn judged(&mut self, facts: &Facts, places: Places<'_>, id: ObjectId, whose: Whose) -> Verdict {
        let mut faults = Faults {
            kept: Kept::default(),
            places,
            object: id,
        };
        let verdict = pdf_linearize::check(facts, &LIMITS, &mut faults);
        let Some(made) = self.made_as(id, whose) else {
            return verdict;
        };
        let (object, revision) = match whose {
            Whose::Dictionary => (Some(id), None),
            Whose::First => (None, Some(1)),
        };
        // The dictionary as the walk read it, for what the model holds it
        // to: the file's, at its number.
        let dict = self.source.get(id).ok();
        // What's found, in the order told: each kind's finding, or, of what
        // the model holds the dictionary to, its place among what's found
        // where the walk found nothing.
        let mut found: Vec<Result<Finding, usize>> = Vec::new();
        let mut tells = Vec::new();
        for (i, &(at, fault, n)) in faults.kept.told().iter().enumerate() {
            // What isn't judged is no finding, departures asked for or not.
            if fault.grade() == Grade::Unjudged {
                self.unchecked("-", unjudged(&fault), TABLE, "-");
                continue;
            }
            let key = key(faults.places.of(&fault));
            if fault.grade() != Grade::Error && !self.departures {
                let at = (TABLE.to_string(), key);
                *self.report.departures.entry(at).or_default() += n;
                continue;
            }
            let finding = Finding {
                object,
                made,
                path: object_path(id),
                table: TABLE.to_string(),
                key,
                rule: Rule::Linearization,
                value: times(&fault.to_string(), n),
                at: at.at,
                revision,
            };
            if whose == Whose::First || !of_the_model(&fault) {
                found.push(Ok(finding));
                continue;
            }
            // `/H`'s count is the model's at or inside its array.
            let inside = match fault {
                Fault::HintItems { .. } => (dict.and_then(Object::as_dict))
                    .and_then(|d| d.get(b"H"))
                    .and_then(|h| self.value_at(Some(id), h.at)),
                _ => None,
            };
            found.push(Err(i));
            tells.push(Told {
                finding,
                kind: i,
                whole: None,
                family: None,
                inside,
            });
        }
        let mut stood: HashMap<usize, Finding> = (self.found_told(tells).into_iter())
            .map(|t| (t.kind, t.finding))
            .collect();
        let found = found.into_iter().filter_map(|f| match f {
            Ok(finding) => Some(finding),
            Err(i) => stood.remove(&i),
        });
        self.report.findings.extend(found);
        verdict
    }
}

#[cfg(test)]
mod tests {
    //! What's found of a linearized file's linearization, where, and as
    //! whose: files linearized by pdf-linearize, a byte changed in each, or
    //! a value of its hint tables, the hint stream's data stored as it is,
    //! so that a change keeps its length; and those files updated after, by
    //! pdf-structure's writer.

    use pdf_fixtures::fixture::{self, Body, Xref};
    use pdf_linearize::check::HintData;
    use pdf_linearize::family::{self, linearize_bytes};
    use pdf_linearize::hint::{PageOffsets, SharedObjects};
    use pdf_structure::XrefKind;
    use pdf_structure::write::{self, Base, Deflate};

    use super::*;
    use crate::bytes::{self, WithReading};
    use std::collections::BTreeMap;

    use crate::check::{Hold, Made, Options, Report, check, check_with_reading};
    use crate::dict;
    use crate::pdf_filters::{Level, Prediction};

    fn r(n: u32) -> Object {
        Object::reference(n)
    }

    /// Three pages, each with its content, the first page's font used by
    /// all three, and an image the last two share.
    fn three_pages() -> Vec<u8> {
        let page = |n: u32, contents: u32, image: bool| {
            let mut resources = dict! { "Font" => dict! { "F1" => r(9) } };
            if image {
                resources.set("XObject", dict! { "Im1" => r(10) });
            }
            let page = dict! {
                "Type" => "Page",
                "Parent" => r(2),
                "MediaBox" => vec![Object::from(0), 0.into(), 612.into(), 792.into()],
                "Resources" => resources,
                "Contents" => r(contents),
            };
            (n, Body::from(page))
        };
        let content = |n: u32, text: &str| (n, Body::stream(dict! {}, text.as_bytes()));
        let objects = vec![
            (1, dict! { "Type" => "Catalog", "Pages" => r(2) }.into()),
            (
                2,
                dict! { "Type" => "Pages", "Kids" => vec![r(3), r(4), r(5)], "Count" => 3 }.into(),
            ),
            page(3, 6, false),
            page(4, 7, true),
            page(5, 8, true),
            content(6, "BT /F1 12 Tf 72 720 Td (One) Tj ET"),
            content(7, "BT /F1 12 Tf 72 720 Td (Two) Tj ET /Im1 Do"),
            content(8, "BT /F1 12 Tf 72 720 Td (Three) Tj ET /Im1 Do"),
            (
                9,
                dict! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" }.into(),
            ),
            (
                10,
                Body::stream(
                    dict! {
                        "Type" => "XObject",
                        "Subtype" => "Form",
                        "BBox" => vec![Object::from(0), 0.into(), 10.into(), 10.into()],
                        "Resources" => dict! {},
                    },
                    b"0 0 m 10 10 l S",
                ),
            ),
        ];
        fixture::file("1.7", &objects, dict! { "Root" => r(1) }, Xref::Table)
    }

    /// Data deflated as it is, stored, not compressed.
    fn stored() -> Deflate {
        let mut deflate = Deflate::DEFAULT;
        deflate.level = Level::NONE;
        deflate
    }

    /// `file` opened by `passwords` and linearized, its hint stream's data
    /// stored as it is.
    fn linearized(file: &[u8], passwords: &[crate::pdf_crypt::Password]) -> Vec<u8> {
        let mut options = family::Options::DEFAULT;
        options.deflate = stored();
        options.read.passwords = passwords;
        linearize_bytes(file, &options).unwrap()
    }

    /// `file` checked beside its reading under `options`, every object held
    /// as merged.
    fn checked_with(file: &[u8], options: &Options) -> Report {
        let read = WithReading::new(file, &bytes::Options::DEFAULT).unwrap();
        check_with_reading(&read, &Hold::All(Made::Merged), options)
    }

    fn checked(file: &[u8]) -> Report {
        checked_with(file, &Options::default())
    }

    /// The linearization findings: key, value and bytes.
    fn told(report: &Report) -> Vec<(&str, &str, Option<Span>)> {
        (report.findings.iter())
            .filter(|f| f.rule == Rule::Linearization)
            .map(|f| (f.key.as_str(), f.value.as_str(), f.at))
            .collect()
    }

    /// Where `needle` is in `haystack`, the first.
    fn find(haystack: &[u8], needle: &[u8]) -> usize {
        (haystack.windows(needle.len()))
            .position(|w| w == needle)
            .unwrap()
    }

    /// The facts pdf-linearize reads of `file`.
    fn facts(file: &[u8]) -> Facts {
        let reading = bytes::Reading::open(file, &bytes::Options::DEFAULT).unwrap();
        family::facts(&reading, &LIMITS).unwrap()
    }

    /// `file` with its hint stream's data changed by `change`, which keeps
    /// its length, stored as it is again: the stream's bytes are as long as
    /// they were.
    fn with_hint_data(file: &[u8], change: impl FnOnce(&Facts, &mut Vec<u8>)) -> Vec<u8> {
        let facts = facts(file);
        let hint = facts.hint.as_ref().unwrap();
        let (at, end) = (hint.at as usize, hint.end as usize);
        let start = at + find(&file[at..end], b"stream") + "stream".len();
        let start = start
            + file[start..]
                .iter()
                .take_while(|&&b| b == b'\r' || b == b'\n')
                .count();
        let stop = at + find(&file[at..end], b"endstream");
        let HintData::Decoded(mut data) = hint.data.clone() else {
            panic!("the hint stream decodes");
        };
        let len = data.len();
        change(&facts, &mut data);
        assert_eq!(data.len(), len);
        let mut again = Vec::new();
        stored()
            .encode(&data, Prediction::None, &mut again)
            .unwrap();
        let mut out = file.to_vec();
        let after = &file[start + again.len()..stop];
        assert!(
            after.iter().all(u8::is_ascii_whitespace),
            "stored as long as it was"
        );
        out[start..start + again.len()].copy_from_slice(&again);
        out
    }

    /// How many pages `facts` say, by `/N`.
    fn pages(facts: &Facts) -> u32 {
        match facts.first_object.as_ref().unwrap().n {
            Raw::Integer(n) => n as u32,
            _ => panic!("/N is an integer"),
        }
    }

    /// Where the shared object table is in the hint data, by `/S`.
    fn shared_at(facts: &Facts) -> usize {
        let hint = facts.hint.as_ref().unwrap();
        (hint.keys.iter())
            .find_map(|(key, raw)| match (key.as_slice(), raw) {
                (b"S", &Raw::Integer(at)) => Some(at as usize),
                _ => None,
            })
            .unwrap()
    }

    /// The page offset table of `data`, changed by `change`, written back
    /// in its widths.
    fn pages_changed(facts: &Facts, data: &mut [u8], change: impl FnOnce(&mut PageOffsets)) {
        let mut table = PageOffsets::read(data, pages(facts), &LIMITS).unwrap();
        change(&mut table);
        let mut out = Vec::new();
        table.write(&mut out).unwrap();
        assert!(out.len() <= shared_at(facts));
        data[..out.len()].copy_from_slice(&out);
    }

    /// The shared object table of `data`, changed by `change`, written back
    /// in its widths.
    fn shared_changed(facts: &Facts, data: &mut [u8], change: impl FnOnce(&mut SharedObjects)) {
        let at = shared_at(facts);
        let mut table = SharedObjects::read(&data[at..], &LIMITS).unwrap();
        change(&mut table);
        let mut out = Vec::new();
        table.write(&mut out).unwrap();
        data[at..at + out.len()].copy_from_slice(&out);
    }

    /// `file` with `old`, in its first 1,024 bytes, written as `new`, as
    /// long.
    fn written_as(file: &[u8], old: &str, new: &str) -> Vec<u8> {
        assert_eq!(old.len(), new.len());
        let at = find(&file[..1024], old.as_bytes());
        let mut out = file.to_vec();
        out[at..at + new.len()].copy_from_slice(new.as_bytes());
        out
    }

    /// The dictionary's entry `entry`, in `file`'s first 1,024 bytes.
    fn entry(file: &[u8], entry: &str) -> Option<Span> {
        let at = find(&file[..1024], entry.as_bytes());
        Some(Span::new(None, at, at + entry.len()))
    }

    /// The byte `at` of the hint stream's data, the stream `facts` of
    /// `file` give.
    fn hint_byte(file: &[u8], at: usize) -> Option<Span> {
        let number = facts(file).hint.unwrap().number;
        Some(Span::new(Some(ObjectId::new(number, 0)), at, at + 1))
    }

    /// A file linearized by pdf-linearize holds: nothing is found of it, no
    /// departure counted, and its linearization dictionary is its first
    /// object, as `--full` says too. Its document alone, which a check
    /// reads no linearization of, has no verdict; nor has the file before
    /// it was linearized. Its hint stream is unreached in neither.
    #[test]
    fn a_linearized_file_that_holds_has_nothing_found() {
        let file = linearized(&three_pages(), &[]);
        let report = checked(&file);
        assert_eq!(told(&report), [], "{report}");
        assert_eq!(report.linearized, Some(Linearized::Holds));
        assert!(report.departures.is_empty());
        assert_eq!(report.checked.get(TABLE), Some(&1));
        let full = report.to_string();
        assert!(
            full.contains("  content read: 4\n  linearized: holds\n"),
            "{full}"
        );
        let read = WithReading::new(&file[..], &bytes::Options::DEFAULT).unwrap();
        let alone = check(
            read.document(),
            &Hold::All(Made::Merged),
            &Options::default(),
        );
        assert_eq!((told(&alone), alone.linearized), (vec![], None));
        assert!(!alone.to_string().contains("linearized:"));
        // Its hint stream, which nothing refers to, isn't unreached, beside
        // the reading or alone.
        for report in [&report, &alone] {
            assert!(
                (report.findings.iter()).all(|f| f.rule != Rule::Unreached),
                "{report}"
            );
        }
        let before = checked(&three_pages());
        assert_eq!((told(&before), before.linearized), (vec![], None));
    }

    /// Each error pdf-linearize's check tells is found once, in the
    /// linearization dictionary's object, its table and path, at the key
    /// it's of and its bytes: the dictionary's `/E`, `/T` and `/N` at their
    /// entries; a page's count and the page table at `H`, at the hint
    /// stream's data's first byte, where the page offset table is, a
    /// position at the file's byte it's at; `/H`'s length at `/H`'s entry;
    /// what doesn't read of the hint tables, at the dictionary; and
    /// a shared entry's length at the shared object table's first byte in
    /// the data. A page's length wrong is told of that page and of where
    /// the next one is, as pdf-linearize tells it.
    #[test]
    fn each_error_is_found_once_in_the_dictionary_at_its_key_and_bytes() {
        let file = linearized(&three_pages(), &[]);
        let dict = ObjectId::new(7, 0);
        let at = |changed: &[u8]| -> Vec<(String, String, Option<Span>)> {
            let report = checked(changed);
            assert_eq!(report.linearized, Some(Linearized::Fails), "{report}");
            (report.findings.iter())
                .filter(|f| f.rule == Rule::Linearization)
                .inspect(|f| {
                    assert_eq!(
                        (f.object, f.path.as_str(), f.table.as_str(), f.made),
                        (Some(dict), "7 0 R", TABLE, Made::Merged)
                    );
                })
                .map(|f| (f.key.clone(), f.value.clone(), f.at))
                .collect()
        };
        let one = |key: &str, value: &str, bytes: Option<Span>| {
            vec![(key.to_string(), value.to_string(), bytes)]
        };
        let e = written_as(&file, "/E 923", "/E 123");
        let value = "/E is 123, where the first page's last object ends by 922 to 923 (ISO 32000-2, Table F.1)";
        assert_eq!(at(&e), one("E", value, entry(&e, "/E 123")));
        let t = written_as(&file, "/T 1622", "/T 1522");
        let value = "/T is 1522, where the main section's first entry is at 1622 to 1623 (ISO 32000-2, Table F.1)";
        assert_eq!(at(&t), one("T", value, entry(&t, "/T 1522")));
        let n = written_as(&file, "/N 3", "/N 4");
        let mut both = one(
            "N",
            "/N is 4, where the page tree has 3 pages (ISO 32000-2, Table F.1)",
            entry(&n, "/N 4"),
        );
        both.extend(one(
            "H",
            "the page table gives page 3 245 bytes, where its objects take 199 (ISO 32000-2, F.4.2)",
            hint_byte(&n, 0),
        ));
        assert_eq!(at(&n), both);
        let h = written_as(&file, "/H [473 166]", "/H [473 199]");
        let value = "/H's length from byte 473 is 199, where the hint stream takes 165 to 166 (ISO 32000-2, Table F.1)";
        assert_eq!(at(&h), one("H", value, entry(&h, "/H [473 199]")));
        let s = written_as(&file, "/S 48", "/S 47");
        let value = "the hint tables don't read: a shared entry's count of objects's width is 1024 bits, past 32 (ISO 32000-2, F.4)";
        let whole = Some(Span::new(None, 23, 88));
        assert_eq!(at(&s), one("-", value, whole));
        assert_eq!(&s[23..25], b"<<");
        // Page 1's length one byte more than its objects take.
        let length = with_hint_data(&file, |facts, data| {
            pages_changed(facts, data, |table| {
                let least = u64::from(table.header.min_length);
                let page = &mut table.pages[1];
                page.length = least + ((page.length - least) ^ 1);
            });
        });
        let first = hint_byte(&length, 0);
        assert_eq!(
            at(&length),
            [
                (
                    "H",
                    "the page table gives page 1 2 objects, where its length holds 3 (ISO 32000-2, F.4.2)"
                ),
                (
                    "H",
                    "the page table gives page 1 246 bytes, where its objects take 245 (ISO 32000-2, F.4.2)"
                ),
                (
                    "H",
                    "the page table's lengths have page 2 at 1003, where its object is at 1002 as if the hint stream weren't there (ISO 32000-2, F.4.2)"
                ),
            ]
            .map(|(key, value)| (key.to_string(), value.to_string(), first))
        );
        // The last shared entry's length one byte less.
        let shared = with_hint_data(&file, |facts, data| {
            shared_changed(facts, data, |table| {
                let least = u64::from(table.header.min_length);
                let last = table.groups.len() - 1;
                let entry = &mut table.groups[last];
                entry.length = least + ((entry.length - least) ^ 1);
            });
        });
        let value = "the shared object table gives entry 3 129 bytes, where its objects take 130 (ISO 32000-2, F.4.3)";
        assert_eq!(at(&shared), one("H", value, hint_byte(&shared, 48)));
        // A position is at the file's byte it's at, past the hint stream as
        // `/H` has it, one byte shorter.
        let moved = written_as(&file, "/H [473 166]", "/H [473 165]");
        let report = checked(&moved);
        let position = (report.findings.iter())
            .find(|f| f.value.starts_with("the page table has the first page at"))
            .expect("the first page's place");
        assert_eq!(position.key, "H");
        assert_eq!(position.at, Some(Span::new(None, 639, 640)), "{report}");
    }

    /// What's wrong with one of the dictionary's values is at that key,
    /// though pdf-linearize says it of a page: `/O` not the first page's
    /// object, at `/O`'s entry, before what follows of the first page's
    /// object taken by it. What a page doesn't say itself is of the
    /// page's object, at its record, its key `-`: a departure, counted
    /// there, and found when asked for.
    #[test]
    fn whats_of_a_value_or_of_a_page_is_at_that_key_or_the_pages_object() {
        let file = linearized(&three_pages(), &[]);
        let facts = facts(&file);
        let Raw::Integer(o) = facts.first_object.as_ref().unwrap().o else {
            panic!("/O is an integer");
        };
        // Another number as long.
        let other = o ^ 1;
        let (old, new) = (format!("/O {o} "), format!("/O {other} "));
        let changed = written_as(&file, &old, &new);
        let report = checked(&changed);
        let value = format!("/O is {other}, where page 0's object is {o} (ISO 32000-2, Table F.1)");
        let at = entry(&changed, new.trim_end());
        // The first page's place told after it, as pdf-linearize takes the
        // first page's object by `/O`.
        assert_eq!(told(&report)[0], ("O", value.as_str(), at), "{report}");
        assert!(told(&report)[1..].iter().all(|f| f.0 == "H"), "{report}");
        assert_eq!(report.linearized, Some(Linearized::Fails));
        // The second page not saying its `/MediaBox` itself.
        let ObjectId { number: page, .. } = facts.pages[1].id.unwrap();
        let object = find(&file, format!("\n{page} 0 obj").as_bytes()) + 1;
        let media_box = object + find(&file[object..], b"/MediaBox");
        let mut bare = file.clone();
        bare[media_box..media_box + 9].copy_from_slice(b"/MediaBoz");
        let report = checked(&bare);
        assert_eq!(told(&report), [], "{report}");
        assert_eq!(report.linearized, Some(Linearized::Holds));
        let counted = BTreeMap::from([((TABLE.to_string(), "-".to_string()), 1)]);
        assert_eq!(report.departures, counted, "{report}");
        let asked = Options {
            departures: true,
            ..Options::default()
        };
        let report = checked_with(&bare, &asked);
        let read = WithReading::new(&bare[..], &bytes::Options::DEFAULT).unwrap();
        let record = read.document().record(ObjectId::new(page, 0)).map(|r| r.at);
        assert_eq!(record.map(|at| at.start), Some(object));
        let value = "page 1 doesn't say its /MediaBox itself (ISO 32000-2, F.3.7, F.3.10)";
        assert_eq!(told(&report), [("-", value, record)], "{report}");
    }

    /// A position is at the file's byte it's at as pdf-linearize places it,
    /// whatever `/H` says of the hint stream's length: where it says more
    /// than the stream may take, or less than it takes, or isn't an
    /// integer, past the stream as far as it may take, to where the next
    /// object starts, as the check then tells `/H` once and places the rest
    /// by the stream that's there.
    #[test]
    fn a_position_is_at_its_object_whatever_h_says_of_the_hint_streams_length() {
        let file = linearized(&three_pages(), &[]);
        // The page table's first page a byte on.
        let file = with_hint_data(&file, |facts, data| {
            pages_changed(facts, data, |table| table.header.first_page_at += 1);
        });
        let Raw::Integer(o) = facts(&file).first_object.unwrap().o else {
            panic!("/O is an integer");
        };
        let object = find(&file, format!("\n{o} 0 obj").as_bytes()) + 1;
        let value = "the page table has the first page at 474, where it's at 473 as if the hint stream weren't there (ISO 32000-2, F.4.2)";
        for h in [
            "/H [473 166]",
            "/H [473 199]",
            "/H [473 100]",
            "/H [473 1.6]",
            "/H [473]    ",
        ] {
            let report = checked(&written_as(&file, "/H [473 166]", h));
            let position = (report.findings.iter())
                .find(|f| f.value == value)
                .expect("the first page's place");
            assert_eq!(position.key, "H", "{h}");
            assert_eq!(
                position.at,
                Some(Span::new(None, object, object + 1)),
                "{h}: {report}"
            );
        }
    }

    /// The file `file`, its later pages each listing the first page's shared
    /// entry, 0, where they list the first of the two they share: so each of
    /// the two reaches what entry 1 holds and doesn't list it, and lists
    /// what it reaches nothing of. Departures, which readers get past.
    fn listing_another(file: &[u8]) -> Vec<u8> {
        with_hint_data(file, |facts, data| {
            pages_changed(facts, data, |table| {
                for page in [1, 2] {
                    assert_eq!(table.pages[page].shared[0].0, 1);
                    table.pages[page].shared[0].0 = 0;
                }
            });
        })
    }

    /// A departure from Annex F that readers get past is counted by its
    /// table and key, every one told, and found only when asked for, each
    /// kind once with how many times it was told: two pages that reach one
    /// entry of the shared object table and don't list it are two, at one
    /// place, the page offset table's first byte. The file holds all the
    /// same; `--full` says the count.
    #[test]
    fn a_departure_is_counted_each_time_and_found_when_asked() {
        let file = listing_another(&linearized(&three_pages(), &[]));
        let report = checked(&file);
        assert_eq!(told(&report), [], "{report}");
        assert_eq!(report.linearized, Some(Linearized::Holds));
        let counted = BTreeMap::from([((TABLE.to_string(), "H".to_string()), 4)]);
        assert_eq!(report.departures, counted);
        let full = report.to_string();
        assert!(
            full.contains("  departures: LinearizationParameterDict/H: 4\n"),
            "{full}"
        );
        let asked = Options {
            departures: true,
            ..Options::default()
        };
        let report = checked_with(&file, &asked);
        let first = hint_byte(&file, 0);
        assert_eq!(
            told(&report),
            [
                (
                    "H",
                    "page 1 reaches what entry 1 of the shared object table holds, and doesn't list it (ISO 32000-2, F.4.2), 2 times",
                    first
                ),
                (
                    "H",
                    "page 1 lists entry 0 of the shared object table, and reaches nothing it holds (ISO 32000-2, F.4.2), 2 times",
                    first
                ),
            ]
        );
        assert!(report.departures.is_empty());
        assert_eq!(report.linearized, Some(Linearized::Holds));
    }

    /// What the model holds the dictionary to that pdf-linearize says too
    /// is found by the model alone: an `/L` that's no integer (`type`), so
    /// that the file isn't taken for linearized, and nothing else is found
    /// of it; a `/Linearized` not 1 (`value`), likewise; and an `/H` of
    /// three items, the model's `missing` inside the array, at
    /// `ArrayOf_4Integers`' `3`, what else pdf-linearize says of that `/H`
    /// found.
    #[test]
    fn what_the_model_finds_in_the_dictionary_isnt_found_again() {
        let file = linearized(&three_pages(), &[]);
        let model = |report: &Report| -> Vec<(String, String, Rule)> {
            (report.findings.iter())
                .filter(|f| f.object == Some(ObjectId::new(7, 0)))
                .map(|f| (f.table.clone(), f.key.clone(), f.rule))
                .collect()
        };
        let report = checked(&written_as(&file, "/L 1805", "/L (xx)"));
        let not_taken = Some(Linearized::NotTaken);
        assert_eq!((told(&report), report.linearized), (vec![], not_taken));
        assert_eq!(
            model(&report),
            [(TABLE.to_string(), "L".to_string(), Rule::Type)]
        );
        let report = checked(&written_as(&file, "/Linearized 1", "/Linearized 2"));
        assert_eq!((told(&report), report.linearized), (vec![], not_taken));
        assert_eq!(
            model(&report),
            [(TABLE.to_string(), "Linearized".to_string(), Rule::Value)]
        );
        let three = written_as(&file, "/H [473 166]", "/H [1 2 3]  ");
        let report = checked(&three);
        let value = "nothing's listed at /H's byte 1, where the hint stream should be (ISO 32000-2, Table F.1)";
        assert_eq!(told(&report), [("H", value, entry(&three, "/H [1 2 3]"))]);
        assert_eq!(
            model(&report),
            [
                (
                    "ArrayOf_4Integers".to_string(),
                    "3".to_string(),
                    Rule::Missing
                ),
                (TABLE.to_string(), "H".to_string(), Rule::Linearization),
            ]
        );
        assert_eq!(report.linearized, Some(Linearized::Fails));
    }

    /// A linearization's findings and departures are of its dictionary,
    /// where that's held, and what's held otherwise doesn't take them; its
    /// verdict is the file's whatever's held.
    #[test]
    fn a_linearizations_findings_are_held_where_its_dictionary_is() {
        let file = written_as(
            &listing_another(&linearized(&three_pages(), &[])),
            "/E 923",
            "/E 123",
        );
        let read = WithReading::new(&file[..], &bytes::Options::DEFAULT).unwrap();
        let only = |id: ObjectId| {
            let hold = Hold::Only([(id, Made::Built)].into());
            check_with_reading(&read, &hold, &Options::default())
        };
        let dict = only(ObjectId::new(7, 0));
        assert_eq!(told(&dict).len(), 1, "{dict}");
        assert_eq!(dict.findings[0].made, Made::Built);
        assert_eq!(dict.departures.values().sum::<usize>(), 4);
        assert_eq!(dict.linearized, Some(Linearized::Fails));
        let hint = facts(&file).hint.unwrap().number;
        for other in [ObjectId::new(1, 0), ObjectId::new(hint, 0)] {
            let report = only(other);
            assert_eq!(told(&report), [], "{report}");
            assert!(report.departures.is_empty());
            assert_eq!(report.linearized, Some(Linearized::Fails));
        }
    }

    /// `file` linearized, its hint stream's data stored as it is, as
    /// [`linearized`], under its cross-reference streams with what may be
    /// packed in object streams where `pack` says.
    fn linearized_packed(
        file: &[u8],
        passwords: &[crate::pdf_crypt::Password],
        pack: bool,
    ) -> Vec<u8> {
        let mut options = family::Options::DEFAULT;
        options.deflate = stored();
        options.kind = XrefKind::Stream;
        options.pack = pack;
        options.read.passwords = passwords;
        linearize_bytes(file, &options).unwrap()
    }

    /// What's counted as not checked of a linearization, by why: every row
    /// in its table, each at the dictionary.
    fn not_checked(report: &Report) -> BTreeMap<&str, usize> {
        (report.unchecked.iter())
            .filter(|(row, _)| row.table == TABLE)
            .map(|(row, &n)| {
                assert_eq!((row.column, row.key.as_str()), ("-", "-"), "{row:?}");
                (row.why.as_str(), n)
            })
            .collect()
    }

    /// An encrypted file linearized as encrypted, opened by its user's
    /// password, has its hint stream's data decrypted, and holds; opened by
    /// none, the data isn't read, which is counted once as not checked, in
    /// its words, and nothing is found of its tables, nor counted as a
    /// departure, whether departures are asked for or not. Packed, its page
    /// tree's nodes are in object streams no key decrypts too, which is
    /// counted once beside it.
    #[test]
    fn an_encrypted_linearized_files_hint_data_is_decrypted() {
        let id = b"0123456789abcdef";
        let settings = crate::pdf_crypt::writer::Settings {
            kind: crate::pdf_crypt::writer::Kind::Aes128,
            user: b"user",
            owner: b"owner",
            p: -4,
            id,
            encrypt_metadata: true,
        };
        let plain = three_pages();
        let read = bytes::Reading::open(&plain[..], &bytes::Options::DEFAULT).unwrap();
        let doc = read.to_document().unwrap();
        let objects: Vec<(u32, Body)> = (doc.ids().into_iter())
            .map(|id| (id.number, Body::from(doc.get(id).unwrap().clone())))
            .collect();
        let encrypted = fixture::encrypted(&settings, &objects, dict! { "Root" => r(1) });
        let user = [crate::pdf_crypt::Password("user")];
        let file = linearized(&encrypted, &user);
        let mut options = bytes::Options::DEFAULT;
        options.passwords = &user;
        let read = WithReading::new(&file[..], &options).unwrap();
        let report = check_with_reading(&read, &Hold::All(Made::Merged), &Options::default());
        assert_eq!(told(&report), [], "{report}");
        assert_eq!(report.linearized, Some(Linearized::Holds));
        assert_eq!(not_checked(&report), BTreeMap::new(), "{report}");
        let words = "a linearization whose hint stream's data isn't decrypted";
        assert_eq!(LOCKED, words);
        let locked = WithReading::new(&file[..], &bytes::Options::DEFAULT).unwrap();
        for departures in [false, true] {
            let asked = Options {
                departures,
                ..Options::default()
            };
            let report = check_with_reading(&locked, &Hold::All(Made::Merged), &asked);
            assert_eq!(told(&report), [], "{report}");
            assert!(report.departures.is_empty(), "{report}");
            assert_eq!(report.linearized, Some(Linearized::Holds));
            let rows = BTreeMap::from([(words, 1)]);
            assert_eq!(not_checked(&report), rows, "{report}");
        }
        // Each page's resources an object of its own, which packs, so that the
        // page tree's walk meets an object stream.
        let mut split = Vec::new();
        for id in doc.ids() {
            let mut object = doc.get(id).unwrap().clone();
            let page = object.as_dict_mut().filter(|page| page.has_type(b"Page"));
            if let Some(resources) = page.and_then(|page| page.get(b"Resources").cloned()) {
                let own = id.number + 100;
                object.as_dict_mut().unwrap().set("Resources", r(own));
                split.push((own, Body::from(resources)));
            }
            split.push((id.number, Body::from(object)));
        }
        let encrypted = fixture::encrypted(&settings, &split, dict! { "Root" => r(1) });
        let packed = linearized_packed(&encrypted, &user, true);
        let locked = WithReading::new(&packed[..], &bytes::Options::DEFAULT).unwrap();
        let member = |(_, _, entry): (u32, u16, crate::bytes::XrefEntry)| {
            matches!(entry, crate::bytes::XrefEntry::Compressed { .. })
        };
        assert!(locked.reading().entries().any(member), "it's packed");
        let report = check_with_reading(&locked, &Hold::All(Made::Merged), &Options::default());
        assert_eq!(told(&report), [], "{report}");
        assert!(report.departures.is_empty(), "{report}");
        let tree = "a linearization whose page tree isn't read: no key the reader has opens it";
        let rows = BTreeMap::from([(words, 1), (tree, 1)]);
        assert_eq!(not_checked(&report), rows, "{report}");
    }

    /// A linearized file whose hint stream the reader doesn't read, past its
    /// cap on a stream's data or through a filter it doesn't take, has that
    /// counted as not checked, once, and nothing found of it, nor counted as
    /// a departure.
    #[test]
    fn a_hint_stream_the_reader_doesnt_read_is_counted_as_not_checked() {
        let file = linearized(&three_pages(), &[]);
        let hint = facts(&file).hint.unwrap();
        let HintData::Decoded(data) = &hint.data else {
            panic!("decoded");
        };
        let mut options = bytes::Options::DEFAULT;
        options.limits.stream = data.len() - 1;
        let read = WithReading::new(&file[..], &options).unwrap();
        let report = check_with_reading(&read, &Hold::All(Made::Merged), &Options::default());
        assert_eq!(told(&report), [], "{report}");
        assert!(report.departures.is_empty(), "{report}");
        let why =
            "a linearization whose hint stream isn't read: it's past a cap the reader reads under";
        assert_eq!(not_checked(&report), BTreeMap::from([(why, 1)]), "{report}");
        let at = hint.at as usize;
        let filter = at + find(&file[at..], b"/FlateDecode");
        let mut unfiltered = file.clone();
        unfiltered[filter..filter + 12].copy_from_slice(b"/DCTDecode  ");
        let report = checked(&unfiltered);
        assert_eq!(told(&report), [], "{report}");
        assert!(report.departures.is_empty(), "{report}");
        assert_eq!(report.linearized, Some(Linearized::Holds));
        let why = "a linearization whose hint stream isn't read: it's through filters the reader doesn't take";
        assert_eq!(not_checked(&report), BTreeMap::from([(why, 1)]), "{report}");
    }

    /// `file` with an update after it, by pdf-structure's writer: its
    /// catalog written again with a `/PageLayout`, and `more`, each object
    /// at its number.
    fn updated_with(file: &[u8], more: &[(ObjectId, Object)]) -> Vec<u8> {
        updated_as(file, more, None)
    }

    /// [`updated_with`], its cross-reference section of `kind` where given,
    /// else as the file's last is.
    fn updated_as(file: &[u8], more: &[(ObjectId, Object)], kind: Option<XrefKind>) -> Vec<u8> {
        let reading = bytes::Reading::open(file, &bytes::Options::DEFAULT).unwrap();
        let root = (reading.trailer().dict().get(b"Root"))
            .and_then(Object::as_reference)
            .unwrap();
        let mut catalog = reading.get(root).unwrap().clone();
        catalog
            .as_dict_mut()
            .unwrap()
            .set("PageLayout", "SinglePage");
        let bodies = [(root, &catalog)]
            .into_iter()
            .chain(more.iter().map(|(id, o)| (*id, o)));
        let bodies = bodies.map(|(id, object)| (id, write::Body::from(object)));
        let mut base = Base::of(&reading).unwrap();
        if let Some(kind) = kind {
            base.kind = kind;
        }
        let mut out = file.to_vec();
        write::update(
            &mut out,
            &base,
            bodies,
            None,
            None,
            &write::Options::DEFAULT,
        )
        .unwrap();
        out
    }

    fn updated(file: &[u8]) -> Vec<u8> {
        updated_with(file, &[])
    }

    /// A finding's object, path, table, key, how it's made and value.
    type Voided<'r> = (Option<ObjectId>, &'r str, &'r str, &'r str, Made, &'r str);

    /// The findings of [`Rule::Updated`], as [`Voided`] has them.
    fn voided(report: &Report) -> Vec<Voided<'_>> {
        (report.findings.iter())
            .filter(|f| f.rule == Rule::Updated)
            .map(|f| {
                let (path, table, key) = (f.path.as_str(), f.table.as_str(), f.key.as_str());
                (f.object, path, table, key, f.made, f.value.as_str())
            })
            .collect()
    }

    /// What [`Rule::Updated`] says of `file`, linearized as its first
    /// `len` bytes, and updated after.
    fn updated_value(file: &[u8], len: usize) -> String {
        let reading = bytes::Reading::open(file, &bytes::Options::DEFAULT).unwrap();
        let at = reading.revisions()[1].at;
        format!(
            "the file was linearized as first written, its first {len} bytes; the update after it, its section at byte {at}, voids that: it's read as an ordinary file (ISO 32000-2, F.1, Table F.1)"
        )
    }

    /// A file linearized, then updated: that it was is found once, in the
    /// newest trailer, from it, its table's and key `-`, merged, at the
    /// trailer's bytes; the file as first written holds, so nothing of its
    /// linearization is found, nor what pdf-linearize says of the whole,
    /// that `/L` isn't its length and an update follows; and its verdict is
    /// that it's updated. Updated again, it's found once all the same, of
    /// the first update. Its document alone has neither. Updated otherwise,
    /// its section elsewhere, it's alike, its digits aside. Nothing else is
    /// found: the hint stream, which nothing refers to, isn't unreached.
    #[test]
    fn a_linearized_file_updated_after_is_found_so_and_held_as_first_written() {
        let file = linearized(&three_pages(), &[]);
        let once = updated(&file);
        let report = checked(&once);
        let value = updated_value(&once, file.len());
        let trailer = bytes::document(&once, &bytes::Options::DEFAULT)
            .trailer()
            .clone();
        let table = match trailer.object.value() {
            Value::Stream(_) => "XRefStream",
            _ => "FileTrailer",
        };
        let found = (
            trailer.stream,
            "trailer",
            table,
            "-",
            Made::Merged,
            value.as_str(),
        );
        assert_eq!(voided(&report), [found], "{report}");
        let finding = report.findings.iter().find(|f| f.rule == Rule::Updated);
        assert_eq!(finding.unwrap().at, trailer.object.at);
        assert_eq!(told(&report), [], "{report}");
        let rules: Vec<_> = report.findings.iter().map(|f| f.rule).collect();
        assert_eq!(rules, [Rule::Updated], "{report}");
        assert!(report.departures.is_empty(), "{report}");
        assert_eq!(report.linearized, Some(Linearized::Updated { held: true }));
        assert!(report.to_string().contains("  linearized: updated\n"));
        let twice = updated(&once);
        let again = checked(&twice);
        assert_eq!(voided(&again).len(), 1, "{again}");
        assert_eq!(voided(&again)[0].5, updated_value(&once, file.len()));
        assert_eq!(told(&again), [], "{again}");
        assert_eq!(again.linearized, Some(Linearized::Updated { held: true }));
        let read = WithReading::new(&once[..], &bytes::Options::DEFAULT).unwrap();
        let alone = check(
            read.document(),
            &Hold::All(Made::Merged),
            &Options::default(),
        );
        assert_eq!((voided(&alone), alone.linearized), (vec![], None));
        // Another update, its section at another byte, is alike: what one
        // check of it found, another carries.
        let other = updated_with(
            &file,
            &[(ObjectId::new(99, 0), dict! { "Other" => 1 }.into())],
        );
        let other = checked(&other);
        assert_ne!(voided(&other)[0].5, voided(&report)[0].5);
        let since = other.since(&report);
        assert!(since.iter().all(|f| f.rule != Rule::Updated), "{since:?}");
    }

    /// A hint table's error as first written is found after an update as
    /// before it, at the same bytes of the hint stream's data, beside that
    /// the update voids the linearization; and, where the update writes
    /// over the hint stream's number, at none, as the file's stream at that
    /// number is the update's.
    #[test]
    fn an_error_as_first_written_is_found_after_an_update() {
        let file = linearized(&three_pages(), &[]);
        // Page 1's length one byte more than its objects take.
        let file = with_hint_data(&file, |facts, data| {
            pages_changed(facts, data, |table| {
                let least = u64::from(table.header.min_length);
                let page = &mut table.pages[1];
                page.length = least + ((page.length - least) ^ 1);
            });
        });
        let before = checked(&file);
        assert_eq!(before.linearized, Some(Linearized::Fails));
        assert_eq!(told(&before).len(), 3, "{before}");
        let report = checked(&updated(&file));
        assert_eq!(told(&report), told(&before), "{report}");
        assert_eq!(voided(&report).len(), 1, "{report}");
        assert_eq!(report.linearized, Some(Linearized::Updated { held: false }));
        let hint = facts(&file).hint.unwrap().number;
        let over = updated_with(
            &file,
            &[(ObjectId::new(hint, 0), dict! { "Hint" => 1 }.into())],
        );
        let report = checked(&over);
        let unplaced: Vec<_> = (told(&before).into_iter())
            .map(|(key, value, _)| (key, value, None))
            .collect();
        assert_eq!(told(&report), unplaced, "{report}");
        assert_eq!(voided(&report).len(), 1, "{report}");
        // The update's object at that number is no hint stream, and
        // nothing refers to it.
        let unreached = (report.findings.iter())
            .filter(|f| f.rule == Rule::Unreached)
            .map(|f| f.object);
        assert_eq!(
            unreached.collect::<Vec<_>>(),
            [Some(ObjectId::new(hint, 0))],
            "{report}"
        );
    }

    /// An update that writes the linearization dictionary's number again
    /// puts another object there, which nothing refers to: what's wrong
    /// with the linearization as first written is the first revision's, in
    /// no object, from the dictionary's number, held where every object is,
    /// as merged, and never by the hold of the object now at that number.
    /// The hint stream, which no linearization dictionary of the file as it
    /// now is names, is unreached.
    #[test]
    fn a_dictionary_written_over_leaves_its_findings_to_the_first_revision() {
        let file = written_as(&linearized(&three_pages(), &[]), "/E 923", "/E 123");
        let dict = facts(&file).first_object.unwrap().number;
        let over = updated_with(
            &file,
            &[(ObjectId::new(dict, 0), dict! { "Foo" => 1 }.into())],
        );
        let report = checked(&over);
        let found: Vec<_> = (report.findings.iter())
            .filter(|f| f.rule == Rule::Linearization)
            .map(|f| {
                (
                    f.object,
                    f.revision,
                    f.path.as_str(),
                    f.key.as_str(),
                    f.made,
                )
            })
            .collect();
        let path = format!("{dict} 0 R");
        assert_eq!(
            found,
            [(None, Some(1), path.as_str(), "E", Made::Merged)],
            "{report}"
        );
        assert_eq!(report.findings[1].at, entry(&file, "/E 123"));
        assert_eq!(voided(&report).len(), 1, "{report}");
        assert_eq!(report.linearized, Some(Linearized::Updated { held: false }));
        let unreached = (report.findings.iter())
            .filter(|f| f.rule == Rule::Unreached)
            .map(|f| f.object);
        let hint = facts(&file).hint.unwrap().number;
        let unreached: Vec<_> = unreached.collect();
        assert_eq!(
            unreached,
            [Some(ObjectId::new(dict, 0)), Some(ObjectId::new(hint, 0))],
            "{report}"
        );
        // Held by the number alone, as built, or the update's objects: none
        // of the linearization's, which neither wrote; the verdict stands.
        let read = WithReading::new(&over[..], &bytes::Options::DEFAULT).unwrap();
        let update = bytes::document(&over, &bytes::Options::DEFAULT)
            .update()
            .cloned()
            .unwrap();
        let theirs = update.objects.iter().map(|&id| (id, Made::Built)).collect();
        for hold in [
            Hold::Only([(ObjectId::new(dict, 0), Made::Built)].into()),
            Hold::Only(theirs),
        ] {
            let report = check_with_reading(&read, &hold, &Options::default());
            assert_eq!(told(&report), [], "{report}");
            assert!(report.departures.is_empty(), "{report}");
            assert_eq!(report.linearized, Some(Linearized::Updated { held: false }));
        }
    }

    /// That an update voided the linearization is alike whichever table
    /// the trailer it's told in is: a check after a second update, its
    /// section a cross-reference stream where the first's is a table,
    /// carries it from the check before.
    #[test]
    fn updated_is_alike_whichever_trailer_tells_it() {
        let file = linearized(&three_pages(), &[]);
        let once = updated_as(&file, &[], Some(XrefKind::Table));
        let twice = updated_as(&once, &[], Some(XrefKind::Stream));
        let (before, after) = (checked(&once), checked(&twice));
        let tables = |report: &Report| -> Vec<String> {
            (report.findings.iter())
                .filter(|f| f.rule == Rule::Updated)
                .map(|f| f.table.clone())
                .collect()
        };
        assert_eq!(tables(&before), ["FileTrailer"], "{before}");
        assert_eq!(tables(&after), ["XRefStream"], "{after}");
        let since = after.since(&before);
        assert!(since.iter().all(|f| f.rule != Rule::Updated), "{since:?}");
    }

    /// A first revision whose `/L` counts the CR of the CR LF after its
    /// `%%EOF`, not the LF, is the file as first written all the same, and
    /// what follows it an update that voids it; the file as first written
    /// is its bytes up to `/L`, so it holds.
    #[test]
    fn an_l_that_counts_a_cr_before_an_lf_ends_the_first_revision() {
        let mut file = linearized(&three_pages(), &[]);
        assert!(file.ends_with(b"%%EOF\n"));
        let len = file.len();
        file.insert(len - 1, b'\r');
        let report = checked(&updated(&file));
        assert_eq!(told(&report), [], "{report}");
        let value = updated_value(&updated(&file), len);
        assert_eq!(voided(&report).len(), 1, "{report}");
        assert_eq!(voided(&report)[0].5, value);
        assert_eq!(report.linearized, Some(Linearized::Updated { held: true }));
    }

    /// A claim that isn't taken has what pdf-linearize tells of it found:
    /// a file of one revision with a NUL after its `%%EOF` line, which the
    /// reader says too, whose `/L` isn't the file's length, nor is
    /// anything updated after it; one whose `/L` is past the file's
    /// length; and updated, that `/L` isn't its length still, and that an
    /// update follows, a departure, found only when asked for. Its verdict
    /// is that it isn't taken.
    #[test]
    fn a_claim_not_taken_has_what_pdf_linearize_tells_of_it_found() {
        let file = linearized(&three_pages(), &[]);
        let not_taken = Some(Linearized::NotTaken);
        let nul = [&file[..], b"\0"].concat();
        let report = checked(&nul);
        let length = |len: usize| {
            let value = format!(
                "/L is 1805, where the file is {len} bytes from its header (ISO 32000-2, Table F.1)"
            );
            ("L".to_string(), value, entry(&file, "/L 1805"))
        };
        let told_owned = |report: &Report| -> Vec<(String, String, Option<Span>)> {
            (told(report).into_iter())
                .map(|(key, value, at)| (key.to_string(), value.to_string(), at))
                .collect()
        };
        assert_eq!(told_owned(&report), [length(1806)], "{report}");
        assert!(
            (report.findings.iter()).any(|f| f.rule == Rule::Reader),
            "{report}"
        );
        assert_eq!((voided(&report), report.linearized), (vec![], not_taken));
        let past = written_as(&file, "/L 1805", "/L 1806");
        let report = checked(&past);
        let value =
            "/L is 1806, where the file is 1805 bytes from its header (ISO 32000-2, Table F.1)";
        assert_eq!(told(&report), [("L", value, entry(&past, "/L 1806"))]);
        assert_eq!((voided(&report), report.linearized), (vec![], not_taken));
        assert!(report.departures.is_empty(), "{report}");
        // Updated: `/L` is no revision's length, and an update follows.
        let past = updated(&past);
        let report = checked(&past);
        let value = format!(
            "/L is 1806, where the file is {} bytes from its header (ISO 32000-2, Table F.1)",
            past.len()
        );
        let at = entry(&past, "/L 1806");
        assert_eq!(told(&report), [("L", value.as_str(), at)], "{report}");
        let counted = BTreeMap::from([((TABLE.to_string(), "-".to_string()), 1)]);
        assert_eq!(report.departures, counted, "{report}");
        assert_eq!((voided(&report), report.linearized), (vec![], not_taken));
        let asked = Options {
            departures: true,
            ..Options::default()
        };
        let report = checked_with(&past, &asked);
        let update = bytes::Reading::open(&past[..], &bytes::Options::DEFAULT).unwrap();
        let section = update.revisions()[1].at;
        let departure = format!(
            "an update is appended, its section at byte {section}: the hints may no longer hold (ISO 32000-2, G.7)"
        );
        let byte = Some(Span::new(None, section, section + 1));
        assert_eq!(
            told(&report),
            [("L", value.as_str(), at), ("-", departure.as_str(), byte)],
            "{report}"
        );
    }

    /// A hold of an update's objects alone holds that it voids the
    /// linearization, in the trailer, held as the trailer is, and not what's
    /// wrong with the linearization as first written, in its dictionary,
    /// which it doesn't hold; its verdict is the file's whatever's held.
    #[test]
    fn a_hold_of_an_update_holds_that_it_voids_the_linearization() {
        let file = written_as(&linearized(&three_pages(), &[]), "/E 923", "/E 123");
        let once = updated(&file);
        let update = bytes::document(&once, &bytes::Options::DEFAULT)
            .update()
            .cloned()
            .unwrap();
        let hold = Hold::Only(update.objects.iter().map(|&id| (id, Made::Built)).collect());
        let read = WithReading::new(&once[..], &bytes::Options::DEFAULT).unwrap();
        let report = check_with_reading(&read, &hold, &Options::default());
        assert_eq!(voided(&report).len(), 1, "{report}");
        assert_eq!(voided(&report)[0].4, Made::Merged);
        assert_eq!(told(&report), [], "{report}");
        assert_eq!(report.linearized, Some(Linearized::Updated { held: false }));
        let all = checked(&once);
        assert_eq!(told(&all).len(), 1, "{all}");
        assert_eq!(told(&all)[0].0, "E", "{all}");
    }

    /// What's of an object in the linearization as first written is at its
    /// record there, in the file's bytes, which an update leaves as they
    /// were, though the update writes that object anew: the hint stream at
    /// generation 1, a departure, found when asked for at its record before
    /// the update and after it alike.
    #[test]
    fn an_objects_place_as_first_written_is_its_record_then() {
        let mut file = linearized(&three_pages(), &[]);
        let hint = facts(&file).hint.unwrap();
        let row = format!("{:010} 00000 n", hint.at);
        let row = find(&file, row.as_bytes()) + "0000000000 0000".len();
        file[row] = b'1';
        let at = hint.at as usize + find(&file[hint.at as usize..], b" 0 obj") + 1;
        file[at] = b'1';
        let asked = Options {
            departures: true,
            ..Options::default()
        };
        let generation = |report: &Report| -> Vec<Option<Span>> {
            (report.findings.iter())
                .filter(|f| f.value.contains("at generation 1"))
                .map(|f| f.at)
                .collect()
        };
        let before = checked_with(&file, &asked);
        let record = generation(&before);
        assert_eq!(record.len(), 1, "{before}");
        assert_eq!(record[0].map(|at| at.start), Some(hint.at as usize));
        let over = updated_with(
            &file,
            &[(ObjectId::new(hint.number, 1), dict! { "Hint" => 1 }.into())],
        );
        let after = checked_with(&over, &asked);
        assert_eq!(generation(&after), record, "{after}");
        assert_eq!(after.linearized, Some(Linearized::Updated { held: true }));
    }
}
