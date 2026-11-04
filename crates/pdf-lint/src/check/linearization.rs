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
//! anything ([`claimed`]): its first object a dictionary, not a stream, with
//! `/Linearized`, starting within 1,024 bytes of its header. One that
//! doesn't has nothing told, and no verdict
//! ([`Report::linearized`](super::Report::linearized)). One that's
//! linearized by pdf-linearize's test, its `/Linearized` 1 and its `/L` its
//! length, has what the check tells found, and the verdict, holds or fails,
//! by whether an error is among it; one that isn't taken for linearized has
//! only what's wrong with `/Linearized` or `/L` as a value found, and no
//! verdict.
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
//! ([`Kept::keep_each`]), held where the dictionary is. What's wrong with one of the dictionary's values as a value, or
//! `/H`'s count of items, is what the model holds the dictionary to too:
//! it's found only where the walk found nothing by the model's rules in
//! the dictionary at that key, or for `/H`'s count, at or inside its array
//! ([`Walk::find_told`]). A hint stream whose data is encrypted and not
//! decrypted, and a file whose bytes the source didn't give, are counted as
//! not checked, once, where the dictionary is held.

use std::collections::{HashMap, HashSet};

use pdf_linearize::check::{DictKey, Facts, FirstObject, Grade, HintData, Place, Raw, RowEntry};
use pdf_linearize::hint::located;
use pdf_linearize::{Finding as Fault, Kind, Limits};

use super::content::times;
use super::kept::{Kept, Where};
use super::told::Told;
use super::{Finding, Linearized, Rule, Walk, entry_at, first_object, linearization, object_path};
use crate::object::{Object, ObjectId, Span, Value};
use crate::source::Source;

/// The table a linearization's findings are in: the model's for the
/// dictionary that says a file is linearized (Table F.1).
const TABLE: &str = "LinearizationParameterDict";

/// What pdf-linearize's check reads hint tables under: its caps, which ISO
/// 32000-2 doesn't set.
const LIMITS: Limits = Limits::DEFAULT;

/// Why a linearization isn't checked whole: its hint stream's data is
/// encrypted, and the reader didn't decrypt it.
const LOCKED: &str = "a linearization whose hint stream's data isn't decrypted";

/// Why a linearization isn't checked: the source didn't give the bytes its
/// facts are read from.
const UNGIVEN: &str = "a linearization whose bytes the source didn't give";

/// The first object `facts` give, where it says the file is linearized, as
/// pdf-linearize's check takes a claim before it tells anything: a
/// dictionary, not a stream, with `/Linearized`, starting within the first
/// 1,024 bytes from the header.
fn claimed(facts: &Facts) -> Option<&FirstObject> {
    let first = facts.first_object.as_ref()?;
    let within = first.at.saturating_sub(facts.header) < 1024;
    (!first.stream && first.linearized != Raw::Absent && within).then_some(first)
}

/// Whether `fault` is of what the model holds the dictionary to as well:
/// a value of it missing, of another type or a reference, or `/H`'s count
/// of items. It gives way to what the walk found by the model's rules.
fn of_the_model(fault: &Fault) -> bool {
    matches!(
        fault,
        Fault::Version | Fault::BadValue { .. } | Fault::Indirect { .. } | Fault::HintItems { .. }
    )
}

/// Whether `fault` is told of a file that says it's linearized and isn't
/// taken for one: what's wrong with `/Linearized` or `/L` as a value.
fn of_a_claim(fault: &Fault) -> bool {
    matches!(
        fault,
        Fault::Version | Fault::BadValue { .. } | Fault::Indirect { .. }
    )
}

/// Where `fault` is said of, as pdf-linearize places it, but that a
/// `/Linearized` not 1 is of that key's value, which pdf-linearize places
/// nowhere, as the file isn't taken for linearized.
fn place(fault: &Fault) -> Option<Place> {
    match fault {
        Fault::Version => Some(Place::Key(DictKey::Linearized)),
        _ => fault.place(),
    }
}

/// The key a finding at `place` is told at: the dictionary's key it's of,
/// `H` for what's of the hint stream, `-` for the rest.
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

/// Where the hint stream is and how long, as pdf-linearize's check places a
/// hint table's positions as if it weren't there ([`located`]): `/H`'s
/// offset and length, each 0 where it isn't a non-negative integer, but for
/// a length outside what the stream listed at that offset takes, from past
/// its `endobj` to where what's listed after it starts: then the most it
/// takes, as the check tells `/H`'s length wrong once and places the rest by
/// the stream that's there (Table F.1).
fn extent(facts: &Facts) -> (u64, u64) {
    let items = facts.first_object.as_ref().map_or(&[][..], |f| &f.h_items);
    let at = number(items.first()).unwrap_or(0);
    let length = number(items.get(1)).unwrap_or(0);
    let Some(hint) = facts.hint.as_ref().filter(|h| h.at == at) else {
        return (at, length);
    };
    let least = hint.end.saturating_sub(at);
    let next = next_after(facts, hint.end.saturating_sub(1));
    let most = next.saturating_sub(at).max(least);
    match (least..=most).contains(&length) {
        true => (at, length),
        false => (at, most),
    }
}

/// Where the first thing after byte `at` starts of what the sections list,
/// each table section's `xref` and the file's end, as pdf-linearize's check
/// takes them: of a number listed more than once, only its first row, the
/// first section's before the main one's and a hybrid's stream's before its
/// table's.
fn next_after(facts: &Facts, at: u64) -> u64 {
    let sections = || facts.first.iter().chain(&facts.main);
    let mut listed = HashSet::new();
    let rows = (sections().flat_map(|s| s.hybrid.iter().chain(&s.rows)))
        .filter(|row| listed.insert(row.number))
        .filter_map(|row| match row.entry {
            RowEntry::InUse(start) => Some(start),
            _ => None,
        });
    let tables = (sections().filter(|s| s.kind == Kind::Table)).map(|s| s.at);
    (rows.chain(tables).chain([facts.len]))
        .filter(|&start| start > at)
        .min()
        .unwrap_or(u64::MAX)
}

/// Where each place pdf-linearize gives a finding is in the file, as a
/// finding has its bytes ([`Places::at`]).
struct Places<'a> {
    source: &'a dyn Source,
    /// The linearization dictionary, as the file has it at its number.
    dict: Option<&'a Object>,
    /// The hint stream's number, where something's listed at `/H`.
    hint: Option<u32>,
    /// The hint stream and how many bytes its data decodes to, as the source
    /// decodes it again: none where it doesn't decode cleanly. Read the
    /// first time a place in it is asked for.
    data: Option<Option<(ObjectId, usize)>>,
    /// Where the hint stream is and how long, as a hint table's positions
    /// are as if it weren't there from ([`extent`]).
    stream: (u64, u64),
    /// Where each of the hint stream's tables starts in its data, by its key
    /// in the stream's dictionary, the page offset table at 0.
    tables: Vec<(Vec<u8>, u64)>,
    /// Each number's object, by number, read the first time an object's
    /// place is asked for.
    numbers: Option<HashMap<u32, ObjectId>>,
}

impl<'a> Places<'a> {
    /// The places of what `facts`, of the file `source` reads, say, its
    /// linearization dictionary `dict`.
    fn new(source: &'a dyn Source, facts: &Facts, dict: Option<&'a Object>) -> Places<'a> {
        let hint = facts.hint.as_ref();
        let mut tables = vec![(b"page offset".to_vec(), 0)];
        for (key, raw) in hint.iter().flat_map(|h| &h.keys) {
            if let Some(at) = number(Some(raw)) {
                tables.push((key.clone(), at));
            }
        }
        Places {
            source,
            dict,
            hint: hint.map(|h| h.number),
            data: None,
            stream: extent(facts),
            tables,
            numbers: None,
        }
    }

    /// The object at number `n`, by its generation in the file.
    fn id(&mut self, n: u32) -> Option<ObjectId> {
        let source = self.source;
        let numbers = self.numbers.get_or_insert_with(|| {
            (source.ids().into_iter())
                .map(|(n, g)| (n, (n, g)))
                .collect()
        });
        numbers.get(&n).copied()
    }

    /// The bytes `place` gives, as the module says: none where there are
    /// none, or they're past what was read.
    fn at(&mut self, place: Option<Place>) -> Option<Span> {
        let len = self.source.size().unwrap_or(0);
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
                let written = self.source.get(id).ok()?;
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
                self.source.record(id).map(|record| record.at)
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
            self.data = Some(id.and_then(|id| {
                let data = source.decoded_within(id, usize::MAX)?.ok()?;
                Some((id, data.len()))
            }));
        }
        let (id, len) = self.data.flatten()?;
        let at = usize::try_from(at).ok().filter(|&at| at < len)?;
        Some(Span::new(Some(id), at, at + 1))
    }
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

impl pdf_linearize::Sink for Faults<'_> {
    fn finding(&mut self, finding: Fault) {
        let at = self.places.at(place(&finding));
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
    /// documentation says.
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
                if let Some((id, _)) = first
                    && self.made(Some(id)).is_some()
                {
                    self.unchecked("-", UNGIVEN.to_string(), TABLE, "-");
                }
                return;
            }
        };
        let Some(first) = claimed(&facts) else {
            return;
        };
        let id = (first.number, first.generation);
        let dict = self.source.get(id).ok();
        let mut faults = Faults {
            kept: Kept::default(),
            places: Places::new(self.source, &facts, dict),
            object: id,
        };
        let verdict = pdf_linearize::check(&facts, &LIMITS, &mut faults);
        if verdict.linearized {
            self.report.linearized = Some(match verdict.holds {
                true => Linearized::Holds,
                false => Linearized::Fails,
            });
        }
        let Some(made) = self.made(Some(id)) else {
            return;
        };
        let locked = facts
            .hint
            .as_ref()
            .is_some_and(|h| h.data == HintData::Locked);
        if verdict.linearized && locked {
            self.unchecked("-", LOCKED.to_string(), TABLE, "-");
        }
        // What's found, in the order told: each kind's finding, or, of what
        // the model holds the dictionary to, its place among what's found
        // where the walk found nothing.
        let mut found: Vec<Result<Finding, usize>> = Vec::new();
        let mut tells = Vec::new();
        for (i, &(at, fault, n)) in faults.kept.told().iter().enumerate() {
            if !verdict.linearized && !of_a_claim(&fault) {
                continue;
            }
            let key = key(place(&fault));
            if fault.grade() != Grade::Error && !self.departures {
                let at = (TABLE.to_string(), key);
                *self.report.departures.entry(at).or_default() += n;
                continue;
            }
            let finding = Finding {
                object: Some(id),
                made,
                path: object_path(id),
                table: TABLE.to_string(),
                key,
                rule: Rule::Linearization,
                value: times(&fault.to_string(), n),
                at: at.at,
                revision: None,
            };
            if !of_the_model(&fault) {
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
    }
}

#[cfg(test)]
mod tests {
    //! What's found of a linearized file's linearization, where, and as
    //! whose: files linearized by pdf-linearize, a byte changed in each, or
    //! a value of its hint tables, the hint stream's data stored as it is,
    //! so that a change keeps its length.

    use pdf_linearize::check::{HintFacts, RowFacts, SectionFacts};
    use pdf_linearize::family::{self, linearize_bytes};
    use pdf_linearize::hint::{PageOffsets, SharedObjects};
    use pdf_structure::fixture::{self, Body, Xref};
    use pdf_structure::write::Deflate;

    use super::*;
    use crate::bytes::{self, WithReading};
    use std::collections::BTreeMap;

    use crate::check::{Hold, Made, Options, Report, Unchecked, check, check_with_reading};
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

    /// `file` linearized, its hint stream's data stored as it is.
    fn linearized(file: &[u8], passwords: &[crate::pdf_crypt::Password]) -> Vec<u8> {
        let mut options = family::Options::DEFAULT;
        options.deflate = stored();
        linearize_bytes(file, passwords, &options).unwrap()
    }

    /// `file` checked beside its reading under `options`, every object held
    /// as merged.
    fn checked_with(file: &[u8], options: &Options) -> Report {
        let read = WithReading::new(file, &[], &bytes::Options::DEFAULT).unwrap();
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
        Some(Span::new(Some((number, 0)), at, at + 1))
    }

    /// A file linearized by pdf-linearize holds: nothing is found of it, no
    /// departure counted, and its linearization dictionary is its first
    /// object, as `--full` says too. Its document alone, which a check
    /// reads no linearization of, has no verdict; nor has the file before
    /// it was linearized, nor one whose `/L` isn't its length, which isn't
    /// taken for linearized.
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
        let read = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
        let alone = check(
            read.document(),
            &Hold::All(Made::Merged),
            &Options::default(),
        );
        assert_eq!((told(&alone), alone.linearized), (vec![], None));
        assert!(!alone.to_string().contains("linearized:"));
        let before = checked(&three_pages());
        assert_eq!((told(&before), before.linearized), (vec![], None));
        // An `/L` that isn't the file's length: not taken for linearized,
        // so nothing of it is held to Annex F.
        let longer = checked(&written_as(&file, "/L 1805", "/L 1806"));
        assert_eq!((told(&longer), longer.linearized), (vec![], None));
    }

    /// Each error pdf-linearize's check tells is found once, in the
    /// linearization dictionary's object, its table and path, at the key
    /// it's of and its bytes: the dictionary's `/E`, `/T` and `/N` at their
    /// entries; a page's count and the page table at `H`, at the hint
    /// stream's data's first byte, where the page offset table is, a
    /// position at the file's byte it's at; `/H`'s length at the byte it
    /// gives; what doesn't read of the hint tables, at the dictionary; and
    /// a shared entry's length at the shared object table's first byte in
    /// the data. A page's length wrong is told of that page and of where
    /// the next one is, as pdf-linearize tells it.
    #[test]
    fn each_error_is_found_once_in_the_dictionary_at_its_key_and_bytes() {
        let file = linearized(&three_pages(), &[]);
        let dict = (7, 0);
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
        assert_eq!(at(&h), one("-", value, Some(Span::new(None, 473, 474))));
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

    /// The most pdf-linearize's check says the hint stream takes, where it
    /// tells `/H`'s length wrong.
    fn told_most(facts: &Facts) -> Option<u64> {
        let mut found = Vec::new();
        pdf_linearize::check(facts, &LIMITS, &mut found);
        found.iter().find_map(|fault| match *fault {
            Fault::HintExtent { most, .. } => Some(most),
            _ => None,
        })
    }

    /// The hint stream's extent a position is placed by is the one
    /// pdf-linearize's check places it by: `/H`'s offset and length where
    /// the stream listed there takes that length, else the most it takes,
    /// to where the next thing listed starts, which the check's finding of
    /// `/H`'s length says too: of a number listed twice its first row, a
    /// hybrid's stream's before its table's, a table section's `xref` and
    /// the file's end among what's listed.
    #[test]
    fn the_hint_streams_extent_is_the_one_pdf_linearize_places_positions_by() {
        let row = |number, at| RowFacts::new(number, 0, RowEntry::InUse(at));
        let int = Raw::Integer;
        // A file of 1,000 bytes, its hint stream 2 from 100 to 140, object 5
        // next, at 300.
        let mut facts = Facts::new(1000);
        let mut first = FirstObject::new(9, 90, 1, 0);
        first.linearized = int(1);
        first.l = int(1000);
        first.h = Raw::Array;
        facts.first_object = Some(first);
        let mut section = SectionFacts::new(950, Kind::Table);
        section.rows = vec![row(1, 9), row(2, 100), row(5, 300)];
        facts.first = Some(section);
        let mut main = SectionFacts::new(900, Kind::Stream);
        main.rows = vec![row(3, 500)];
        facts.main = Some(main);
        let data = HintData::Decoded(Vec::new());
        facts.hint = Some(HintFacts::new(100, 140, 2, data));
        let h = |facts: &mut Facts, items: &[Raw]| {
            facts.first_object.as_mut().unwrap().h_items = items.to_vec();
        };
        for (items, placed) in [
            (vec![int(100), int(40)], (100, 40)),
            (vec![int(100), int(200)], (100, 200)),
            (vec![int(100), int(39)], (100, 200)),
            (vec![int(100), int(201)], (100, 200)),
            (vec![int(100), int(-40)], (100, 200)),
            (vec![int(100), Raw::Real(40.0)], (100, 200)),
            (vec![int(100)], (100, 200)),
            // No stream listed where `/H` says, or its offset no integer:
            // as it says, 0 for what isn't one.
            (vec![int(99), int(7)], (99, 7)),
            (vec![Raw::Real(100.0), int(7)], (0, 7)),
        ] {
            h(&mut facts, &items);
            assert_eq!(extent(&facts), placed, "{items:?}");
        }
        let most = |facts: &mut Facts| {
            h(facts, &[int(100), int(999)]);
            let (_, most) = extent(facts);
            assert_eq!(Some(most), told_most(facts));
            most
        };
        assert_eq!(most(&mut facts), 200);
        // 5 listed again, nearer, by the main section: its first row stands.
        facts.main.as_mut().unwrap().rows.push(row(5, 150));
        assert_eq!(most(&mut facts), 200);
        // A hybrid's stream listing 4 where its table has it free: the
        // stream's row stands.
        let section = facts.first.as_mut().unwrap();
        section.hybrid = vec![row(4, 250)];
        section.rows.push(RowFacts::new(4, 0, RowEntry::Free));
        assert_eq!(most(&mut facts), 150);
        // The main section a table at 200, before them.
        let main = facts.main.as_mut().unwrap();
        (main.at, main.kind) = (200, Kind::Table);
        assert_eq!(most(&mut facts), 100);
        // 6 right at the stream's end: it takes its own bytes, no more.
        facts.first.as_mut().unwrap().rows.push(row(6, 140));
        assert_eq!(most(&mut facts), 40);
        // Nothing listed after the stream, no table after it: to the end.
        let section = facts.first.as_mut().unwrap();
        (section.kind, section.rows, section.hybrid) = (Kind::Stream, vec![row(2, 100)], vec![]);
        facts.main = None;
        assert_eq!(most(&mut facts), 900);
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
    /// that the file isn't taken for linearized and has no verdict; a
    /// `/Linearized` not 1 (`value`), likewise; and an `/H` of three items,
    /// the model's `missing` inside the array, at `ArrayOf_4Integers`' `3`,
    /// what else pdf-linearize says of that `/H` found.
    #[test]
    fn what_the_model_finds_in_the_dictionary_isnt_found_again() {
        let file = linearized(&three_pages(), &[]);
        let model = |report: &Report| -> Vec<(String, String, Rule)> {
            (report.findings.iter())
                .filter(|f| f.object == Some((7, 0)))
                .map(|f| (f.table.clone(), f.key.clone(), f.rule))
                .collect()
        };
        let report = checked(&written_as(&file, "/L 1805", "/L (xx)"));
        assert_eq!((told(&report), report.linearized), (vec![], None));
        assert_eq!(
            model(&report),
            [(TABLE.to_string(), "L".to_string(), Rule::Type)]
        );
        let report = checked(&written_as(&file, "/Linearized 1", "/Linearized 2"));
        assert_eq!((told(&report), report.linearized), (vec![], None));
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
        let read = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
        let only = |id: ObjectId| {
            let hold = Hold::Only([(id, Made::Built)].into());
            check_with_reading(&read, &hold, &Options::default())
        };
        let dict = only((7, 0));
        assert_eq!(told(&dict).len(), 1, "{dict}");
        assert_eq!(dict.findings[0].made, Made::Built);
        assert_eq!(dict.departures.values().sum::<usize>(), 4);
        assert_eq!(dict.linearized, Some(Linearized::Fails));
        let hint = facts(&file).hint.unwrap().number;
        for other in [(1, 0), (hint, 0)] {
            let report = only(other);
            assert_eq!(told(&report), [], "{report}");
            assert!(report.departures.is_empty());
            assert_eq!(report.linearized, Some(Linearized::Fails));
        }
    }

    /// An encrypted file linearized as encrypted, opened by its user's
    /// password, has its hint stream's data decrypted, and holds; opened by
    /// none, the data isn't read, which is counted once, and nothing is
    /// found of its tables.
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
            .map(|id| (id.0, Body::from(doc.get(id).unwrap().clone())))
            .collect();
        let encrypted = fixture::encrypted(&settings, &objects, dict! { "Root" => r(1) });
        let user = [crate::pdf_crypt::Password("user")];
        let file = linearized(&encrypted, &user);
        let read = WithReading::new(&file[..], &user, &bytes::Options::DEFAULT).unwrap();
        let report = check_with_reading(&read, &Hold::All(Made::Merged), &Options::default());
        assert_eq!(told(&report), [], "{report}");
        assert_eq!(report.linearized, Some(Linearized::Holds));
        assert!(!report.unchecked.keys().any(|u| u.why == LOCKED));
        let locked = WithReading::new(&file[..], &[], &bytes::Options::DEFAULT).unwrap();
        let report = check_with_reading(&locked, &Hold::All(Made::Merged), &Options::default());
        assert_eq!(told(&report), [], "{report}");
        let row = Unchecked {
            column: "-",
            why: LOCKED.to_string(),
            table: TABLE.to_string(),
            key: "-".to_string(),
        };
        assert_eq!(report.unchecked.get(&row), Some(&1), "{report}");
    }
}
