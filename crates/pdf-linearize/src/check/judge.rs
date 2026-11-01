//! What's held against what.

use std::collections::BTreeSet;

use super::{
    Attribute, DictKey, Facts, Finding, FirstObject, HintData, HintFacts, Raw, RowEntry, RowFacts,
    SectionFacts, Sink, Verdict,
};
use crate::Limits;
use crate::check::{Grade, KeyName};
use crate::hint::{self, HintKeys, Tables, position};
use crate::plan::{self, Kind, Part, What};

/// Looking has stopped, as nothing can change the verdict.
struct Settled;

type Go = Result<(), Settled>;

/// The test of [`Verdict::linearized`], what's told of it, and whether it passed.
fn linearized<S: Sink + ?Sized>(facts: &Facts, sink: &mut S) -> bool {
    let Some(first) = &facts.first_object else {
        return false;
    };
    if first.stream
        || first.linearized == Raw::Absent
        || first.at.saturating_sub(facts.header) >= 1024
    {
        return false;
    }
    let mut tell = |finding| {
        if sink.wants() {
            sink.finding(finding);
        }
    };
    let version = match first.linearized {
        Raw::Integer(version) => version == 1,
        Raw::Real(version) => version.floor() == 1.0,
        _ => false,
    };
    if !version {
        tell(Finding::Version);
        return false;
    }
    let length = facts.len.saturating_sub(facts.header);
    match first.l {
        Raw::Integer(l) if u64::try_from(l) == Ok(length) => true,
        Raw::Integer(l) => {
            tell(Finding::LengthNotFile { l, len: length });
            if let Some(at) = facts.update {
                tell(Finding::Updated { at });
            }
            false
        }
        Raw::Reference(_) => {
            tell(Finding::Indirect { key: DictKey::L });
            false
        }
        _ => {
            tell(Finding::BadValue { key: DictKey::L });
            false
        }
    }
}

/// A value that's a non-negative integer.
fn number(raw: Raw) -> Option<u64> {
    match raw {
        Raw::Integer(n) => u64::try_from(n).ok(),
        _ => None,
    }
}

/// What a section's rows and a file's positions are to each other.
struct Index {
    /// What each number is, by number: the first group's before the second's where
    /// a number's listed in both, a table's free row under what its `/XRefStm` says.
    rows: Vec<(u32, RowEntry)>,
    /// Every position anything listed starts at, and the file's end, in order.
    starts: Vec<u64>,
    /// Each object, where it is as if the hint stream weren't there, in order.
    places: Vec<(u64, u32)>,
    /// Every position something starts at, as if the hint stream weren't there, in
    /// order.
    ends: Vec<u64>,
    hint_at: u64,
    hint_len: u64,
}

impl Index {
    fn new(facts: &Facts, hint_at: u64, hint_len: u64) -> Index {
        let sections = || facts.first.iter().chain(&facts.main);
        let mut rows: Vec<(u32, RowEntry)> = Vec::new();
        for section in sections() {
            let listed = section.hybrid.iter().chain(&section.rows);
            rows.extend(listed.map(|row| (row.number, row.entry)));
        }
        // Stable, so the first group's row comes first, and a hybrid's before a
        // table's free one.
        rows.sort_by_key(|row| row.0);
        rows.dedup_by_key(|row| row.0);
        let mut starts: Vec<u64> = (rows.iter())
            .filter_map(|(_, entry)| match entry {
                RowEntry::InUse(at) => Some(*at),
                _ => None,
            })
            .collect();
        let tables = sections().filter(|section| section.kind == Kind::Table);
        starts.extend(tables.map(|section| section.at));
        starts.push(facts.len);
        starts.sort_unstable();
        starts.dedup();
        let mut places: Vec<(u64, u32)> = (rows.iter())
            .filter_map(|&(number, entry)| match entry {
                RowEntry::InUse(at) => Some((at, number)),
                _ => None,
            })
            .filter(|&(at, _)| !in_hint(at, hint_at, hint_len))
            .map(|(at, number)| (position(at, hint_at, hint_len), number))
            .collect();
        places.sort_unstable();
        let mut ends: Vec<u64> = (starts.iter().copied())
            .filter(|&at| !in_hint(at, hint_at, hint_len))
            .map(|at| position(at, hint_at, hint_len))
            .collect();
        ends.sort_unstable();
        ends.dedup();
        Index {
            rows,
            starts,
            places,
            ends,
            hint_at,
            hint_len,
        }
    }

    /// What number `number` is.
    fn row(&self, number: u32) -> Option<RowEntry> {
        let at = self.rows.binary_search_by_key(&number, |row| row.0).ok()?;
        Some(self.rows[at].1)
    }

    /// Whether `row` lists nothing in use, and neither does a hybrid's stream.
    fn is_free(&self, row: &RowFacts) -> bool {
        let free = |entry| matches!(entry, RowEntry::Free | RowEntry::Null);
        free(row.entry) && self.row(row.number).is_none_or(free)
    }

    /// Where the object or the object stream holding `number` is, in the file.
    fn written_at(&self, number: u32) -> Option<u64> {
        match self.row(number)? {
            RowEntry::InUse(at) => Some(at),
            RowEntry::Compressed { stream, .. } => match self.row(stream)? {
                RowEntry::InUse(at) => Some(at),
                _ => None,
            },
            _ => None,
        }
    }

    /// Where the object or the object stream holding `number` is, as if the hint
    /// stream weren't there.
    fn placed_at(&self, number: u32) -> Option<u64> {
        let at = self.written_at(number)?;
        Some(position(at, self.hint_at, self.hint_len))
    }

    /// Whether an object's at `at`, as if the hint stream weren't there.
    fn is_placed(&self, at: u64) -> bool {
        (self.places.binary_search_by_key(&at, |&(place, _)| place)).is_ok()
    }

    /// What starts next after `at`, in the file.
    fn next_after(&self, at: u64) -> u64 {
        let next = self.starts.partition_point(|&start| start <= at);
        self.starts.get(next).copied().unwrap_or(u64::MAX)
    }

    /// Of what's at `at` as if the hint stream weren't there: how many objects
    /// `length` bytes hold, and how many bytes `objects` of them take; none where
    /// nothing's at `at`, or fewer than `objects` follow it.
    fn holds(&self, at: u64, length: u64, objects: u64) -> (u64, Option<u64>) {
        let from = self.places.partition_point(|&(place, _)| place < at);
        let end = at.saturating_add(length);
        let to = self.places.partition_point(|&(place, _)| place < end);
        let held = to.saturating_sub(from) as u64;
        if self.places.get(from).is_none_or(|&(place, _)| place != at) {
            return (held, None);
        }
        let taken = match objects {
            0 => Some(0),
            objects => usize::try_from(objects - 1)
                .ok()
                .and_then(|more| from.checked_add(more))
                .and_then(|last| self.places.get(last))
                .map(|&(last, _)| {
                    let next = self.ends.partition_point(|&start| start <= last);
                    let end = self.ends.get(next).copied().unwrap_or(last);
                    end - at
                }),
        };
        (held, taken)
    }
}

/// Whether `at` is in the hint stream's bytes.
fn in_hint(at: u64, hint_at: u64, hint_len: u64) -> bool {
    at >= hint_at && at - hint_at < hint_len
}

/// The fewest bytes a hint stream from `at` to `end` takes, and the most: to where
/// what follows it starts.
fn hint_room(index: &Index, at: u64, end: u64) -> (u64, u64) {
    let least = end.saturating_sub(at);
    let most = index.next_after(end.saturating_sub(1)).saturating_sub(at);
    (least, most.max(least))
}

/// How a check goes.
struct Judge<'a, S: ?Sized> {
    facts: &'a Facts,
    first_object: &'a FirstObject,
    limits: &'a Limits,
    sink: &'a mut S,
    holds: bool,
    index: Index,
    /// Which page is first, by `/P`.
    first_page: u64,
}

impl<S: Sink + ?Sized> Judge<'_, S> {
    /// Tells `finding`, if it's wanted, and stops if it's an error and nothing more
    /// is.
    fn say(&mut self, finding: Finding) -> Go {
        let error = finding.grade() == Grade::Error;
        if error {
            self.holds = false;
        }
        if self.sink.wants() {
            self.sink.finding(finding);
            Ok(())
        } else if error {
            Err(Settled)
        } else {
            Ok(())
        }
    }

    /// Table F.1: what the dictionary's keys are.
    fn dictionary(&mut self) -> Go {
        let first = self.first_object;
        let from_header = first.end.saturating_sub(self.facts.header);
        if from_header > 1024 {
            self.say(Finding::DictionaryPast1024 { end: from_header })?;
        }
        for (key, raw) in [
            (DictKey::H, first.h),
            (DictKey::O, first.o),
            (DictKey::E, first.e),
            (DictKey::N, first.n),
            (DictKey::T, first.t),
        ] {
            let bad = match key {
                DictKey::H => !matches!(raw, Raw::Array),
                _ => number(raw).is_none(),
            };
            match (raw, bad) {
                (Raw::Reference(_), _) => self.say(Finding::Indirect { key })?,
                (_, true) => self.say(Finding::BadValue { key })?,
                _ => {}
            }
        }
        match first.p {
            Raw::Absent => {}
            Raw::Reference(_) => self.say(Finding::Indirect { key: DictKey::P })?,
            raw if number(raw).is_none() => self.say(Finding::BadValue { key: DictKey::P })?,
            // A first page that isn't one of the pages: PDFium takes the file as not
            // linearized, and a reader has no entry to start from.
            raw => {
                if number(first.n).is_some_and(|n| n > 0 && number(raw) >= Some(n)) {
                    self.say(Finding::BadValue { key: DictKey::P })?;
                }
            }
        }
        if first.h == Raw::Array && !matches!(first.h_items.len(), 2 | 4) {
            self.say(Finding::HintItems {
                items: first.h_items.len(),
            })?;
        }
        if first.h_items.iter().any(|raw| number(*raw).is_none()) {
            let reference = (first.h_items.iter()).any(|raw| matches!(raw, Raw::Reference(_)));
            let key = DictKey::H;
            self.say(match reference {
                true => Finding::Indirect { key },
                false => Finding::BadValue { key },
            })?;
        }
        self.first_page_end()?;
        self.main_first_entry()?;
        self.hint_extent()
    }

    /// `/E`: past the first group's last object, by no more than what follows it.
    fn first_page_end(&mut self) -> Go {
        let (Some(e), Some(section)) = (number(self.first_object.e), &self.facts.first) else {
            return Ok(());
        };
        let (hint_at, hint_len) = (self.index.hint_at, self.index.hint_len);
        let last = (section.rows.iter())
            .filter_map(|row| match row.entry {
                RowEntry::InUse(at) if !in_hint(at, hint_at, hint_len) => Some((at, row.number)),
                _ => None,
            })
            .max();
        let Some((at, number)) = last else {
            return Ok(());
        };
        let read = (self.facts.objects.iter()).find(|e| e.number == number && e.start == at);
        let least = read.map_or(at.saturating_add(1), |extent| extent.end);
        let most = self.index.next_after(at).max(least);
        if e < least || e > most {
            self.say(Finding::FirstPageEnd {
                e: e as i64,
                least,
                most,
            })?;
        }
        Ok(())
    }

    /// `/T`: the white-space before a table's first row, or a stream's object.
    fn main_first_entry(&mut self) -> Go {
        let (Some(t), Some(main)) = (number(self.first_object.t), &self.facts.main) else {
            return Ok(());
        };
        let Some((least, most)) = main.first_row else {
            return Ok(());
        };
        if t < least || t > most {
            self.say(Finding::MainFirstEntry {
                t: t as i64,
                least,
                most,
            })?;
        }
        Ok(())
    }

    /// `/H`: the hint stream's place and its length, overhead and all, and the
    /// overflow stream's.
    fn hint_extent(&mut self) -> Go {
        let items = &self.first_object.h_items;
        let Some(&at) = items.first() else {
            return Ok(());
        };
        let Some(at) = number(at) else {
            return Ok(());
        };
        let Some(hint) = &self.facts.hint else {
            return self.say(Finding::HintNotListed { at: at as i64 });
        };
        if hint.at != at {
            return self.say(Finding::HintNotListed { at: at as i64 });
        }
        let streams = [
            items.get(1).map(|raw| (at, hint.end, *raw)),
            (hint.overflow).and_then(|(at, end)| Some((at, end, *items.get(3)?))),
        ];
        for (at, end, length) in streams.into_iter().flatten() {
            let Raw::Integer(length) = length else {
                continue;
            };
            let (least, most) = hint_room(&self.index, at, end);
            if u64::try_from(length).is_ok_and(|len| len >= least && len <= most) {
                continue;
            }
            self.say(Finding::HintExtent {
                at,
                length,
                least,
                most,
            })?;
        }
        Ok(())
    }

    /// F.3.4: the first section; F.3.11: the main one and `startxref`.
    fn sections(&mut self) -> Go {
        let Some(first) = &self.facts.first else {
            return self.say(Finding::NoSections);
        };
        if first.subsections.len() > 1 {
            self.say(Finding::FirstSubsections {
                count: first.subsections.len(),
            })?;
        }
        for row in &first.rows {
            if self.index.is_free(row) {
                self.say(Finding::FirstFree { number: row.number })?;
            }
        }
        if !matches!(first.root, Raw::Reference(_)) {
            self.say(Finding::FirstNoRoot)?;
        }
        if let Some(row) = first.rows.first() {
            if row.number != self.first_object.number {
                self.say(Finding::FirstNotDictionary {
                    number: row.number,
                    dictionary: self.first_object.number,
                })?;
            }
        }
        let Some(main) = &self.facts.main else {
            return self.say(Finding::NoMainSection);
        };
        if first.at > main.at {
            self.say(Finding::SectionOrder {
                first: first.at,
                main: main.at,
            })?;
        }
        let listed = |section: &SectionFacts| section.rows.len() as u64;
        let total = listed(first) + listed(main);
        if let Raw::Integer(size) = first.size {
            if u64::try_from(size) != Ok(total) {
                self.say(Finding::FirstSize { size, total })?;
            }
        } else {
            self.say(Finding::FirstSize { size: -1, total })?;
        }
        if main.subsections.len() > 1 {
            self.say(Finding::MainSubsections {
                count: main.subsections.len(),
            })?;
        }
        match main.subsections.first() {
            Some(&(from, _)) if from != 0 => self.say(Finding::MainFrom { from })?,
            _ => {}
        }
        for row in &main.rows {
            match row.number {
                0 if !matches!(row.entry, RowEntry::Free | RowEntry::Null) => {
                    self.say(Finding::MainZeroInUse)?
                }
                0 => {}
                number if self.index.is_free(row) => self.say(Finding::MainFree { number })?,
                _ => {}
            }
        }
        if main.prev != Raw::Absent {
            self.say(Finding::MainPrev)?;
        }
        if main.startxref != Some(first.at) {
            self.say(Finding::LastStartxref {
                value: main.startxref.unwrap_or(0),
                first: first.at,
            })?;
        }
        Ok(())
    }

    /// F.3.1: what's numbered what, and what's packed.
    fn numbers(&mut self) -> Go {
        let (Some(first), Some(main)) = (&self.facts.first, &self.facts.main) else {
            return Ok(());
        };
        let hint = self.facts.hint.as_ref().map(|hint| hint.number);
        for section in [first, main] {
            self.one_section(section, hint)?;
        }
        // Past u32 for a row numbered the last there can be, which no first group follows.
        let next = (main.rows.iter())
            .map(|row| u64::from(row.number) + 1)
            .max()
            .unwrap_or(0);
        if let Some(from) = first.rows.iter().map(|row| row.number).min() {
            if u64::from(from) != next {
                self.say(Finding::GroupsApart { first: from, next })?;
            }
        }
        if let Some(hint) = hint {
            // An indirect /Length follows the stream, and goes with it (Table F.1).
            let last = (first.rows.iter())
                .filter(|row| !self.with_hint(row))
                .map(|row| row.number)
                .chain([hint])
                .max();
            if let Some(last) = last.filter(|&last| last != hint) {
                self.say(Finding::HintNotLast { hint, last })?;
            }
        }
        if let Raw::Reference((number, _)) = first.root {
            if matches!(self.index.row(number), Some(RowEntry::Compressed { .. })) {
                self.say(Finding::CatalogCompressed { number })?;
            }
            if !first.rows.iter().any(|row| row.number == number) {
                self.say(Finding::CatalogNotFirst { number })?;
            }
        }
        Ok(())
    }

    /// Of one section's rows: their generations, their order, what's packed last.
    fn one_section(&mut self, section: &SectionFacts, hint: Option<u32>) -> Go {
        let mut packed = false;
        let mut last_at = None;
        let mut rows: Vec<&RowFacts> = section.rows.iter().collect();
        rows.sort_by_key(|row| row.number);
        for row in rows {
            if row.generation != 0
                && matches!(row.entry, RowEntry::InUse(_) | RowEntry::Compressed { .. })
            {
                self.say(Finding::Generation {
                    number: row.number,
                    generation: row.generation,
                })?;
            }
            match row.entry {
                RowEntry::Compressed { .. } => packed = true,
                // The hint stream is last, wherever it's written (F.3.6), and what's
                // in /H's extent with it, an indirect /Length (Table F.1).
                RowEntry::InUse(_) if Some(row.number) == hint || self.with_hint(row) => continue,
                RowEntry::InUse(at) => {
                    if packed {
                        self.say(Finding::UncompressedAfterCompressed { number: row.number })?;
                    }
                    // A section's stream is where a section is, whatever its number.
                    if Some(row.number) == section.stream {
                        continue;
                    }
                    if last_at.is_some_and(|last| at < last) {
                        self.say(Finding::NumberOutOfOrder { number: row.number })?;
                    }
                    last_at = Some(at);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Whether `row` is of what's in the hint stream's bytes as `/H` has them.
    fn with_hint(&self, row: &RowFacts) -> bool {
        let (at, len) = (self.index.hint_at, self.index.hint_len);
        matches!(row.entry, RowEntry::InUse(place) if in_hint(place, at, len))
    }

    /// F.3.6: what the hint stream's dictionary says.
    fn hint_stream(&mut self) -> Go {
        let Some(hint) = &self.facts.hint else {
            return Ok(());
        };
        match hint.data {
            HintData::NotAStream => self.say(Finding::HintNotAStream)?,
            HintData::Undecodable => self.say(Finding::HintUndecodable)?,
            _ => {}
        }
        if matches!(hint.data, HintData::NotAStream) {
            return Ok(());
        }
        if !hint.keys.iter().any(|(key, _)| key == b"S") {
            self.say(Finding::HintNoShared)?;
        }
        for (key, raw) in &hint.keys {
            let name = KeyName::new(key);
            let table = table_key(key).is_some();
            let own = STREAM_KEYS.contains(&key.as_slice());
            match raw {
                Raw::Reference(_) if key == b"Length" => {}
                Raw::Reference(_) => self.say(Finding::HintKeyIndirect { key: name })?,
                raw if table && number(*raw).is_none() => {
                    self.say(Finding::HintKeyValue { key: name })?
                }
                _ if !table && !own => self.say(Finding::HintKeyOther { key: name })?,
                _ => {}
            }
        }
        Ok(())
    }

    /// F.3.7, F.3.10: inheritable attributes pushed down; `/N`, `/O`, a page packed.
    fn pages(&mut self) -> Go {
        let facts = self.facts;
        let first = self.first_object;
        // With no key for what the page tree is in, there's nothing to say of it.
        let locked = |hint: &HintFacts| hint.data == HintData::Locked;
        if facts.pages.is_empty() && facts.hint.as_ref().is_some_and(locked) {
            return Ok(());
        }
        if facts.pages.is_empty() {
            return self.say(Finding::NoPages);
        }
        let pages = facts.pages.len() as u64;
        if let Raw::Integer(n) = first.n {
            if u64::try_from(n) != Ok(pages) {
                self.say(Finding::PageCount { n, pages })?;
            }
        }
        let page = self.first_page;
        let first_page = usize::try_from(page)
            .ok()
            .and_then(|at| facts.pages.get(at));
        if let (Raw::Integer(o), Some(id)) = (first.o, first_page.and_then(|page| page.id)) {
            if u32::try_from(o) != Ok(id.0) {
                self.say(Finding::FirstPageObject {
                    o,
                    page,
                    computed: id.0,
                })?;
            }
            if let Some(section) = &facts.first {
                if !section.rows.iter().any(|row| row.number == id.0) {
                    self.say(Finding::FirstPageNotFirst { number: id.0 })?;
                }
            }
        }
        for (at, page) in (0u64..).zip(&facts.pages) {
            if let Some((number, _)) = page.id {
                if matches!(self.index.row(number), Some(RowEntry::Compressed { .. })) {
                    self.say(Finding::PageCompressed { page: at, number })?;
                }
            }
            let held = [
                (page.resources, Attribute::Resources),
                (page.media_box, Attribute::MediaBox),
            ];
            for (_, attribute) in held.into_iter().filter(|(said, _)| !said) {
                self.say(Finding::PageAttribute {
                    page: at,
                    attribute,
                })?;
            }
        }
        for &(number, attribute) in &facts.nodes {
            self.say(Finding::NodeAttribute { number, attribute })?;
        }
        Ok(())
    }

    /// F.4.2: the page offset table: each page where adding up the lengths has it,
    /// holding what it says and taking what it says.
    fn page_table(&mut self, tables: &Tables) -> Go {
        let facts = self.facts;
        let table = &tables.pages;
        let first_page_at = u64::from(table.header.first_page_at);
        let number_of = |page: u64| {
            let page = usize::try_from(page).ok()?;
            facts.pages.get(page)?.id.map(|id| id.0)
        };
        let first_number = (number(self.first_object.o))
            .and_then(|o| u32::try_from(o).ok())
            .or_else(|| number_of(self.first_page));
        let count = table.pages.len() as u64;
        let first = self.first_page.min(count.saturating_sub(1));
        if let Some(computed) = first_number.and_then(|number| self.index.placed_at(number)) {
            let length = table
                .pages
                .get(first as usize)
                .map_or(0, |entry| entry.length);
            let (hint, within) = (first_page_at, first_page_at.saturating_add(length));
            if computed != hint && self.index.is_placed(hint) && (hint..within).contains(&computed)
            {
                self.say(Finding::FirstPageNotFirstObject { hint, computed })?;
            } else if computed != hint {
                self.say(Finding::FirstPageAt { hint, computed })?;
            }
        }
        let mut at = first_page_at;
        for page in page_order(first, count) {
            let entry = &table.pages[page as usize];
            // Where it is, by its object, so one wrong length isn't said of each page
            // after it. The first is where the table says: a writer may have its page
            // object after other objects of the part.
            let own = number_of(page).and_then(|number| self.index.placed_at(number));
            if let Some(computed) = own.filter(|_| page != first) {
                if computed != at {
                    self.say(Finding::PageAt {
                        page,
                        hint: at,
                        computed,
                    })?;
                }
                at = computed;
            }
            let (held, taken) = self.index.holds(at, entry.length, entry.objects);
            if held != entry.objects {
                self.say(Finding::PageObjects {
                    page,
                    hint: entry.objects,
                    computed: held,
                })?;
            }
            if let Some(taken) = taken.filter(|&taken| taken != entry.length) {
                self.say(Finding::PageLength {
                    page,
                    hint: entry.length,
                    computed: taken,
                })?;
            }
            let end = entry.content_at.saturating_add(entry.content_length);
            if end > entry.length {
                self.say(Finding::ContentPast {
                    page,
                    end,
                    length: entry.length,
                })?;
            }
            let entries = tables.shared.header.entries;
            if page == first && !entry.shared.is_empty() {
                self.say(Finding::FirstPageShared {
                    entries: entry.shared.len() as u64,
                })?;
            }
            for &(id, _) in &entry.shared {
                if id >= entries {
                    self.say(Finding::SharedIdPast { page, id, entries })?;
                }
            }
            at = at.saturating_add(entry.length);
        }
        Ok(())
    }

    /// F.4.3: the shared object table: its first page's entries from the first page's
    /// object, the rest from where its header says.
    fn shared_table(&mut self, tables: &Tables) -> Go {
        let table = &tables.shared;
        let header = &table.header;
        if header.first_page_entries > header.entries {
            self.say(Finding::SharedCounts {
                first: header.first_page_entries,
                total: header.entries,
            })?;
        }
        let mut at = u64::from(tables.pages.header.first_page_at);
        for (entry, group) in (0u64..).zip(&table.groups) {
            if entry == u64::from(header.first_page_entries) {
                at = u64::from(header.first_at);
                let computed = self.index.placed_at(header.first_object);
                if computed != Some(at) {
                    self.say(Finding::SharedFirstAt {
                        number: header.first_object,
                        hint: at,
                        computed,
                    })?;
                }
                // So one wrong position isn't said of each entry after it.
                at = computed.unwrap_or(at);
            }
            let (held, taken) = self.index.holds(at, group.length, group.objects);
            if held != group.objects {
                self.say(Finding::SharedObjects {
                    entry,
                    hint: group.objects,
                    computed: held,
                })?;
            }
            if let Some(taken) = taken.filter(|&taken| taken != group.length) {
                self.say(Finding::SharedLength {
                    entry,
                    hint: group.length,
                    computed: taken,
                })?;
            }
            if group.signature.is_some() {
                self.say(Finding::Signature { entry })?;
            }
            at = at.saturating_add(group.length);
        }
        Ok(())
    }

    /// One stretch of objects a table names: its first object where it says, holding
    /// what it says and taking what it says. Of none, nothing's to be where anything
    /// says, but a length is a count that's wrong, or a length that is if nothing's
    /// there.
    fn stretch(
        &mut self,
        table: &'static str,
        first: u32,
        at: u64,
        objects: u64,
        length: u64,
    ) -> Go {
        if objects == 0 {
            if length == 0 {
                return Ok(());
            }
            let from = self.index.placed_at(first).unwrap_or(at);
            let (held, _) = self.index.holds(from, length, 0);
            return self.say(match held {
                0 => Finding::TableLength {
                    table,
                    hint: length,
                    computed: 0,
                },
                held => Finding::TableObjects {
                    table,
                    hint: 0,
                    computed: held,
                },
            });
        }
        let Some(computed) = self.index.placed_at(first) else {
            return self.say(Finding::TableObject {
                table,
                number: first,
            });
        };
        if computed != at {
            self.say(Finding::TableAt {
                table,
                hint: at,
                computed,
            })?;
        }
        let (held, taken) = self.index.holds(computed, length, objects);
        if held != objects {
            self.say(Finding::TableObjects {
                table,
                hint: objects,
                computed: held,
            })?;
        }
        if let Some(taken) = taken.filter(|&taken| taken != length) {
            self.say(Finding::TableLength {
                table,
                hint: length,
                computed: taken,
            })?;
        }
        Ok(())
    }

    /// F.4.4 to F.4.7: every other table.
    fn other_tables(&mut self, tables: &Tables) -> Go {
        let entries = tables.shared.header.entries;
        let generic = [
            ("/O", &tables.outlines),
            ("/A", &tables.threads),
            ("/E", &tables.dests),
            ("/I", &tables.info),
            ("/L", &tables.labels),
        ];
        for (table, generic) in generic {
            if let Some(g) = generic {
                let (first, at) = (g.first_object, u64::from(g.first_at));
                self.stretch(table, first, at, u64::from(g.objects), u64::from(g.length))?;
            }
        }
        let extended = [
            ("/V", &tables.form),
            ("/C", &tables.structure),
            ("/R", &tables.renditions),
        ];
        for (table, extended) in extended {
            if let Some(e) = extended {
                let g = &e.generic;
                let (first, at) = (g.first_object, u64::from(g.first_at));
                self.stretch(table, first, at, u64::from(g.objects), u64::from(g.length))?;
                for &id in &e.shared {
                    if id >= entries {
                        self.say(Finding::TableIdPast { table, id, entries })?;
                    }
                }
            }
        }
        if let Some(thumbnails) = &tables.thumbnails {
            let h = &thumbnails.header;
            let objects: u64 = thumbnails.pages.iter().map(|page| page.objects).sum();
            let length: u64 = thumbnails.pages.iter().map(|page| page.length).sum();
            self.stretch("/T", h.first_object, u64::from(h.first_at), objects, length)?;
            let shared = u64::from(h.shared_objects);
            let at = u64::from(h.shared_first_at);
            let long = u64::from(h.shared_length);
            self.stretch("/T", h.shared_first_object, at, shared, long)?;
            let gaps = thumbnails.pages.iter().map(|page| u64::from(page.gap));
            let said = gaps.sum::<u64>() + thumbnails.pages.len() as u64;
            let pages = tables.pages.pages.len() as u64;
            if said > pages {
                self.say(Finding::ThumbnailPages { said, pages })?;
            }
        }
        if let Some(files) = &tables.files {
            let mut at = u64::from(files.header.first_at);
            for (group, entry) in (0u64..).zip(&files.groups) {
                let first = match group {
                    0 => files.header.first_object,
                    _ => entry.number,
                };
                let objects = u64::from(entry.objects);
                let length = u64::from(entry.length);
                if group > 0 {
                    at = self.index.placed_at(entry.number).unwrap_or(at);
                }
                self.stretch("/B", first, at, objects, length)?;
                for &id in &entry.shared {
                    if id >= entries {
                        self.say(Finding::TableIdPast {
                            table: "/B",
                            id,
                            entries,
                        })?;
                    }
                }
            }
        }
        Ok(())
    }

    /// The tables' positions against each other (F.3.6): each ending by the next
    /// one's start. An overflow into what's next is a writer's fault a reader of each
    /// table from its own key doesn't see.
    fn table_ends(&mut self, tables: &Tables, keys: &HintKeys) -> Go {
        let mut written = Vec::new();
        if tables.pages.write(&mut written).is_ok() {
            let end = written.len() as u64;
            if end > keys.shared {
                self.say(Finding::TableOverruns {
                    table: "page offset",
                    end,
                    next: keys.shared,
                })?;
            }
        }
        Ok(())
    }

    /// What a page reaches, where the file's graph is given: each later page's own in
    /// its section, the first page's in the first group, and each list as long as
    /// what's shared that it reaches.
    fn graph(&mut self, tables: &Tables) -> Go {
        let Some(graph) = &self.facts.graph else {
            return Ok(());
        };
        let plan = match plan::plan(graph, self.limits) {
            Ok(plan) => plan,
            Err(why) => return self.say(Finding::GraphRefused { why }),
        };
        let writes = plan.writes();
        let number_of = |write: usize| match writes[write].what {
            What::Object(id) => Some(id.0),
            What::Container(_) => None,
        };
        if let Some(first) = &self.facts.first {
            let listed: BTreeSet<u32> = first.rows.iter().map(|row| row.number).collect();
            for write in plan.part_writes(Part::FirstPage) {
                if let Some(number) = number_of(write).filter(|n| !listed.contains(n)) {
                    self.say(Finding::FirstPageReaches { number })?;
                }
            }
        }
        // Where each page's section is, by the table, and each shared entry's.
        let table = &tables.pages;
        let count = table.pages.len() as u64;
        let first = self.first_page.min(count.saturating_sub(1));
        let starts = self.page_starts(tables);
        let shared_at = shared_starts(tables, &self.index);
        let entry_of = |at: u64| {
            let after = shared_at.partition_point(|&(start, _, _)| start <= at);
            (after > 0)
                .then(|| shared_at[after - 1])
                .filter(|&(start, length, _)| at - start < length)
                .map(|(_, _, entry)| entry)
        };
        for page in (0..plan.page_count().min(count as u32)).filter(|&p| u64::from(p) != first) {
            let at = u64::from(page);
            let Some(entry) = table.pages.get(page as usize) else {
                continue;
            };
            let start = starts.get(page as usize).copied().unwrap_or(0);
            let listed: BTreeSet<u64> = entry.shared.iter().map(|&(id, _)| u64::from(id)).collect();
            let mut reached = BTreeSet::new();
            let own = plan.page_writes(page).filter_map(number_of);
            for number in own {
                let placed = self.index.placed_at(number);
                let inside = placed
                    .is_some_and(|placed| placed >= start && placed - start < entry.length.max(1));
                // A member of an object stream that has others' too is in a shared
                // entry, which is the stream's (F.3.1).
                let held = placed.and_then(entry_of);
                reached.extend(held);
                if !inside && !held.is_some_and(|held| listed.contains(&held)) {
                    self.say(Finding::PageReaches { page: at, number })?;
                }
            }
            let shared = (plan.shared(page).iter())
                .filter_map(|&entry| plan.entries().get(entry as usize))
                .flat_map(|writes| writes.clone())
                .filter_map(number_of)
                .filter_map(|number| self.index.placed_at(number))
                .filter_map(entry_of);
            reached.extend(shared);
            for &entry in reached.difference(&listed) {
                self.say(Finding::SharedNotListed { page: at, entry })?;
            }
            for &entry in listed.difference(&reached) {
                self.say(Finding::SharedNotReached { page: at, entry })?;
            }
        }
        Ok(())
    }

    /// Where the table has each page start, by its object if it has one.
    fn page_starts(&self, tables: &Tables) -> Vec<u64> {
        let table = &tables.pages;
        let count = table.pages.len() as u64;
        let mut starts = vec![0; table.pages.len()];
        let first = self.first_page.min(count.saturating_sub(1));
        let mut at = u64::from(table.header.first_page_at);
        for page in page_order(first, count) {
            let page = page as usize;
            let own = (self.facts.pages.get(page))
                .and_then(|page| page.id)
                .and_then(|id| self.index.placed_at(id.0));
            at = own.unwrap_or(at);
            starts[page] = at;
            at = at.saturating_add(table.pages[page].length);
        }
        starts
    }
}

/// The pages of a table of `count` in the order its entries are laid out: the first,
/// then the rest by number. None for a table of none, which an `/N` of 0 reads as.
fn page_order(first: u64, count: u64) -> impl Iterator<Item = u64> {
    let first_of = std::iter::once(first).filter(move |_| count > 0);
    first_of.chain((0..count).filter(move |&page| page != first))
}

/// Where each entry of the shared object table starts and how long it is, in order
/// of where it starts, with which it is. A wrong first place is the shared table's to
/// say: from there on they're from where the first shared object is, as it holds them.
fn shared_starts(tables: &Tables, index: &Index) -> Vec<(u64, u64, u64)> {
    let header = &tables.shared.header;
    let mut at = u64::from(tables.pages.header.first_page_at);
    let mut starts = Vec::with_capacity(tables.shared.groups.len());
    for (entry, group) in (0u64..).zip(&tables.shared.groups) {
        if entry == u64::from(header.first_page_entries) {
            at = (index.placed_at(header.first_object)).unwrap_or(u64::from(header.first_at));
        }
        starts.push((at, group.length, entry));
        at = at.saturating_add(group.length);
    }
    starts.sort_unstable();
    starts
}

/// A stream's own keys, which a hint stream has beside its tables' (7.3.8.2).
const STREAM_KEYS: [&[u8]; 8] = [
    b"Length",
    b"Filter",
    b"DecodeParms",
    b"F",
    b"FFilter",
    b"FDecodeParms",
    b"DL",
    b"Type",
];

/// Where a table's key goes in [`HintKeys`], if `key` is one (Table F.2).
fn table_key(key: &[u8]) -> Option<fn(&mut HintKeys, u64)> {
    Some(match key {
        b"S" => |keys, at| keys.shared = at,
        b"T" => |keys, at| keys.thumbnails = Some(at),
        b"O" => |keys, at| keys.outlines = Some(at),
        b"A" => |keys, at| keys.threads = Some(at),
        b"E" => |keys, at| keys.dests = Some(at),
        b"V" => |keys, at| keys.form = Some(at),
        b"I" => |keys, at| keys.info = Some(at),
        b"C" => |keys, at| keys.structure = Some(at),
        b"L" => |keys, at| keys.labels = Some(at),
        b"R" => |keys, at| keys.renditions = Some(at),
        b"B" => |keys, at| keys.files = Some(at),
        _ => return None,
    })
}

/// What a hint stream's dictionary says of where its tables are: none without its
/// `/S`. A key that isn't a position isn't read.
fn hint_keys(facts: &Facts) -> Option<HintKeys> {
    let hint = facts.hint.as_ref()?;
    let mut keys = HintKeys::default();
    let mut shared = false;
    for (key, raw) in &hint.keys {
        let (Some(set), Some(at)) = (table_key(key), number(*raw)) else {
            continue;
        };
        set(&mut keys, at);
        shared |= key == b"S";
    }
    shared.then_some(keys)
}

/// The hint tables, if they read, told of if they don't.
fn read_tables<S: Sink + ?Sized>(
    facts: &Facts,
    first: &FirstObject,
    limits: &Limits,
    sink: &mut S,
    holds: &mut bool,
) -> Option<(Tables, HintKeys)> {
    let hint = facts.hint.as_ref()?;
    let HintData::Decoded(data) = &hint.data else {
        return None;
    };
    let keys = hint_keys(facts)?;
    let pages = number(first.n).and_then(|n| u32::try_from(n).ok())?;
    match hint::tables(data, &keys, pages, limits) {
        Ok(tables) => Some((tables, keys)),
        Err(fault) => {
            *holds = false;
            if sink.wants() {
                sink.finding(Finding::HintTables { fault });
            }
            None
        }
    }
}

/// [`check`](super::check).
pub(super) fn check<S: Sink + ?Sized>(facts: &Facts, limits: &Limits, sink: &mut S) -> Verdict {
    if !linearized(facts, sink) {
        return Verdict {
            linearized: false,
            holds: false,
            hints_read: false,
        };
    }
    let first = facts.first_object.as_ref().expect("it's linearized");
    let mut holds = true;
    // Past what any reader needs of it, the verdict is the same: it comes first.
    let tables = read_tables(facts, first, limits, sink, &mut holds);
    let hints_read = tables.is_some();
    let verdict = |holds| Verdict {
        linearized: true,
        holds,
        hints_read,
    };
    if !holds && !sink.wants() {
        return verdict(false);
    }
    let item = |at: usize| first.h_items.get(at).copied().and_then(number);
    let (hint_at, hint_len) = (item(0).unwrap_or(0), item(1).unwrap_or(0));
    let mut index = Index::new(facts, hint_at, hint_len);
    // A wrong /H is said once: what's as if the hint stream weren't there goes by
    // the stream that's there, so it isn't said again of everything after it.
    if let Some(hint) = facts.hint.as_ref().filter(|hint| hint.at == hint_at) {
        let (least, most) = hint_room(&index, hint.at, hint.end);
        if hint_len < least || hint_len > most {
            index = Index::new(facts, hint_at, most);
        }
    }
    let mut judge = Judge {
        facts,
        first_object: first,
        limits,
        sink,
        holds,
        index,
        first_page: number(first.p).unwrap_or(0),
    };
    let _ = judged(&mut judge, tables.as_ref());
    verdict(judge.holds)
}

/// Everything that's held, in order, as far as it's wanted.
fn judged<S: Sink + ?Sized>(judge: &mut Judge<'_, S>, tables: Option<&(Tables, HintKeys)>) -> Go {
    judge.dictionary()?;
    judge.sections()?;
    judge.numbers()?;
    judge.hint_stream()?;
    judge.pages()?;
    if let Some((tables, keys)) = tables {
        judge.table_ends(tables, keys)?;
        judge.page_table(tables)?;
        judge.shared_table(tables)?;
        judge.other_tables(tables)?;
        judge.graph(tables)?;
    }
    Ok(())
}
