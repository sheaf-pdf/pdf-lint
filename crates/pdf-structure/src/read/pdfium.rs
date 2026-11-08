//! A file read as PDFium reads it where its sections don't give it
//! ([`Recovery::Pdfium`]), as `cpdf_parser.cpp` has it as of chromium/8125:
//! what sends a file whose sections read to a scan of its every byte, what
//! that scan lists, and what of the sections it keeps. Read for what it does
//! and written here in this crate's terms: the scan is this crate's
//! ([`scan_of`](super::scan_of)), each object it finds read by this crate's
//! reader.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use pdf_objects::object::{Dict, Object, ObjectId, Value};
use pdf_objects::parse::{self, Parser};
use pdf_tokenizer::{Rules, is_delimiter};

use super::window::Bytes;
use super::{
    Cx, Fault, InMemory, Reader, Recovery, Rows, Scan, Section, Trailer, Unwritten, XrefEntry,
    count, is_xref_stream, scan_by,
};
use crate::phases::phase;
use crate::source::{Encryption, Facts};

/// The highest object number PDFium lists: one past it isn't an object.
const NUMBERS: u32 = 24 << 20;

/// How many pages PDFium counts at the most: a page tree that counts as
/// many or more counts none.
const PAGES: i64 = 0xF_FFFF;

/// How many kids a page tree's count looks at, at the most, before it's
/// taken as none: a bound of this crate's, which PDFium hasn't, on a
/// tree whose nodes are shared so that counting it would go on and on.
const VISITS: usize = 1 << 24;

/// The trailer's keys a scan's trailer tells it takes from an earlier one
/// ([`Fault::Taken`]): those that say what the document is.
const DOCUMENT: [&[u8]; 4] = [b"Root", b"Encrypt", b"ID", b"Info"];

/// Where PDFium stops reading a file's sections, to scan it instead, as
/// far as what it had read of them by then goes ([`loaded`]).
#[derive(Debug, Clone, Copy)]
pub(super) enum Stop {
    /// Along the `/Prev` chain: a section that doesn't read, or one read
    /// before. Only the cross-reference streams' rows are read by then.
    Chain,
    /// At the first-written table's first object, which isn't where the
    /// table puts it: by then, that table's rows too.
    Verify,
    /// At a later section's `/XRefStm` that doesn't read, the `k`th, newest
    /// first: by then, the rows of every section before it too.
    XrefStm(usize),
    /// Once every section is read, as for a trailer that gives no page.
    Whole,
}

/// What PDFium had read of a file's sections where it stops to scan the
/// file instead ([`Stop`]), which the scan is merged over: the entries it
/// had listed, a later section's over an earlier's, and the trailer its
/// sections' trailers make.
#[derive(Default)]
pub(super) struct Loaded {
    rows: Rows,
    trailer: Option<Object>,
}

/// What [`Recovery::Pdfium`]'s scan found, which its listing is made of,
/// again where a password opens the file later ([`Reader::members_as_pdfium`]).
pub(super) struct Rebuild {
    /// Each `n g obj`, in the file's order, where its `n` starts.
    objects: Vec<(ObjectId, usize)>,
    /// Each object stream, by number, where its `n` starts, in the file's
    /// order.
    streams: Vec<(u32, usize)>,
    /// The entries the sections had given, which the scan's are over.
    base: Rows,
    /// The numbers whose entries are the sections', in order: each object
    /// the scan didn't find, read where they put it.
    kept: Vec<u32>,
}

/// What PDFium had read of `sections` (newest first), whose rows are
/// `lists`'s, and of `first_table`, the first-written section's table's
/// own rows, where it stops at `stop`: the cross-reference streams' rows, a
/// later one's over an earlier's; at [`Stop::Verify`] and after, the first
/// table's over them, as a table's rows are merged ([`by_table`]); at
/// [`Stop::XrefStm`], each section's before the one whose stream doesn't
/// read; at [`Stop::Whole`], every section's. And the trailer each
/// section's trailer makes, a later one's keys over an earlier's
/// ([`merge_trailer`]).
pub(super) fn loaded<T>(
    sections: &[Section],
    lists: &[(Rows, T)],
    first_table: Option<&Rows>,
    stop: Stop,
) -> Loaded {
    let mut rows = Rows::new();
    let each = || sections.iter().zip(lists).enumerate();
    match stop {
        Stop::Whole => {
            for (_, (_, (listed, _))) in each() {
                for (&n, &entry) in listed {
                    rows.entry(n).or_insert(entry);
                }
            }
        }
        Stop::Chain | Stop::Verify | Stop::XrefStm(_) => {
            for (_, (section, (listed, _))) in each() {
                if section.trailer.stream.is_some() {
                    for (&n, &entry) in listed {
                        rows.entry(n).or_insert(entry);
                    }
                }
            }
            if let (false, Some(table)) = (matches!(stop, Stop::Chain), first_table) {
                by_table(&mut rows, table);
            }
            if let Stop::XrefStm(k) = stop {
                let later = (k + 1)..sections.len().saturating_sub(1);
                for (listed, _) in lists[later].iter().rev() {
                    rows.extend(listed.iter().map(|(&n, &entry)| (n, entry)));
                }
            }
        }
    }
    let mut trailer: Option<Object> = None;
    for section in sections.iter().rev() {
        let newer = section.trailer.dict();
        match &mut trailer {
            None => trailer = Some(Object::new(Value::Dict(newer.clone()))),
            Some(older) => {
                if let Some(older) = older.as_dict_mut() {
                    merge_trailer(older, newer);
                }
            }
        }
    }
    Loaded { rows, trailer }
}

/// `table`'s rows merged into `rows` as PDFium merges a table's: one in use
/// over what's there, but one of a higher generation; one free, of a
/// generation past 0, over it; one free of generation 0 not at all.
fn by_table(rows: &mut Rows, table: &Rows) {
    for (&n, &(generation, entry)) in table {
        match entry {
            XrefEntry::Free if generation == 0 => {}
            XrefEntry::Free => {
                rows.insert(n, (generation, XrefEntry::Free));
            }
            _ => match rows.get(&n) {
                Some(&(had, _)) if had > generation => {}
                _ => {
                    rows.insert(n, (generation, entry));
                }
            },
        }
    }
}

/// `newer`'s keys set in `older`, each over its value there, as PDFium
/// merges a later trailer into an earlier: but `/Prev` and `/XRefStm`,
/// which stay `older`'s, or none.
fn merge_trailer(older: &mut Dict, newer: &Dict) {
    for (key, value) in newer.iter() {
        if !matches!(key, b"Prev" | b"XRefStm") {
            older.set(key, value.clone());
        }
    }
}

/// `id` listed in use at `offset` in `rows`, as PDFium lists each `n g obj`
/// it scans: over what's there, but an entry of a higher generation; none
/// past [`NUMBERS`].
fn add_object(rows: &mut Rows, id: ObjectId, offset: usize) {
    if id.number > NUMBERS {
        return;
    }
    match rows.get(&id.number) {
        Some(&(had, _)) if had > id.generation => {}
        _ => {
            let entry = XrefEntry::InUse {
                offset,
                generation: id.generation,
            };
            rows.insert(id.number, (id.generation, entry));
        }
    }
}

/// The entries the scan's `objects` give, in the file's order
/// ([`add_object`]).
fn listed_of(objects: &[(ObjectId, usize)]) -> Rows {
    let mut rows = Rows::new();
    for &(id, offset) in objects {
        add_object(&mut rows, id, offset);
    }
    rows
}

/// What PDFium reads as the number that starts `word`, a word of its: all
/// its bytes digits, signs or points, read as its sign and then its
/// digits, as far as they go, the most a `u32` holds where they're more,
/// and a minus sign's taken off it; none where it isn't so.
fn number_of(word: &[u8]) -> Option<u32> {
    let numeric = |b: &u8| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.');
    if word.is_empty() || !word.iter().all(numeric) {
        return None;
    }
    let (minus, digits) = match word {
        [b'-', rest @ ..] => (true, rest),
        [b'+', rest @ ..] => (false, rest),
        _ => (false, word),
    };
    let mut n: u32 = 0;
    for &b in digits.iter().take_while(|b| b.is_ascii_digit()) {
        let digit = u32::from(b - b'0');
        match n.checked_mul(10).and_then(|n| n.checked_add(digit)) {
            Some(more) => n = more,
            None => return Some(u32::MAX),
        }
    }
    Some(if minus { n.wrapping_neg() } else { n })
}

/// A dictionary, or a stream's, as PDFium takes a value for one.
fn dict_of(o: &Object) -> Option<&Dict> {
    match o.value() {
        Value::Dict(dict) => Some(dict),
        Value::Stream(stream) => Some(&stream.dict),
        _ => None,
    }
}

/// An integer, as PDFium takes a value for one: a real cut to its whole
/// part, held to a 32-bit integer's range; anything else 0.
fn integer_of(o: &Object) -> i64 {
    let (lo, hi) = (i64::from(i32::MIN), i64::from(i32::MAX));
    match *o.value() {
        Value::Integer(n) => n.clamp(lo, hi),
        // A float's cast saturates, and a NaN is 0.
        Value::Real(r) => (r as i64).clamp(lo, hi),
        _ => 0,
    }
}

/// Where a scan as PDFium's goes on past a stream whose `stream` keyword
/// ends at `keyword`, its object's `n g obj` at `object`, as PDFium reads a
/// stream as it scans: its data from the line after `stream` as long as its
/// `/Length` says, a direct number, where `endstream` follows that past an
/// end of line, white-space and comments; else up to the first `endstream`
/// or `endobj`, a word of its own, after it; else the file's end.
fn stream_end(file: &[u8], keyword: usize, object: Option<usize>, limits: parse::Limits) -> usize {
    let line = (file.get(keyword..).unwrap_or_default().iter())
        .position(|&b| matches!(b, b'\r' | b'\n'))
        .map_or(file.len(), |i| keyword + i);
    let data = match file.get(line..line + 2) {
        Some(b"\r\n") => line + 2,
        _ => (line + 1).min(file.len()),
    };
    let length = object.and_then(|at| {
        let mut parser = Parser::at(file, at).limits(limits);
        parser.header().ok()?;
        let value = parser.value(&mut ()).ok()?;
        let length = value.as_dict()?.get(b"Length")?;
        match *length.value() {
            Value::Integer(n) => Some(n),
            // Cut to its whole part, as PDFium takes a real for an integer.
            Value::Real(r) => Some(r as i64),
            _ => None,
        }
    });
    if let Some(end) = length
        .and_then(|n| usize::try_from(n).ok())
        .and_then(|n| data.checked_add(n))
        .filter(|&end| end < file.len() || end == data)
    {
        let mut p = end;
        match file.get(p..p + 2) {
            Some(b"\r\n") => p += 2,
            Some([b'\r' | b'\n', ..]) => p += 1,
            _ => {}
        }
        if word_from(file, p).starts_with(b"endstream") {
            return end;
        }
    }
    // Both words looked for in one pass, so that the scan reads each byte
    // once, whatever follows.
    (data..file.len())
        .find(|&i| whole_word(file, i, b"endstream") || whole_word(file, i, b"endobj"))
        .unwrap_or(file.len())
}

/// The word PDFium reads at byte `at` of `file`, past white-space and
/// comments, as its rules have them: a run of regular bytes, or the
/// delimiter that starts one.
fn word_from(file: &[u8], at: usize) -> &[u8] {
    let space = |b: u8| Rules::Pdfium.is_white_space(b);
    let mut i = at;
    loop {
        while file.get(i).is_some_and(|&b| space(b)) {
            i += 1;
        }
        if file.get(i) != Some(&b'%') {
            break;
        }
        while file.get(i).is_some_and(|&b| !matches!(b, b'\r' | b'\n')) {
            i += 1;
        }
    }
    let start = i.min(file.len());
    if file.get(i).is_some_and(|&b| is_delimiter(b)) {
        return &file[start..start + 1];
    }
    while file.get(i).is_some_and(|&b| !space(b) && !is_delimiter(b)) {
        i += 1;
    }
    &file[start..i.min(file.len())]
}

/// Whether `word` is at byte `at` of `file`, a word of its own as PDFium
/// finds one: neither byte beside it regular, a digit or a delimiter.
fn whole_word(file: &[u8], at: usize, word: &[u8]) -> bool {
    let apart = |b: Option<&u8>| b.is_none_or(|&b| Rules::Pdfium.is_white_space(b));
    file.get(at..).is_some_and(|rest| rest.starts_with(word))
        && apart(file.get(at + word.len()))
        && (at == 0 || apart(file.get(at - 1)))
}

/// A page tree node's count, or its kids, as PDFium first looks at a node
/// ([`Reader::count_pages`]).
enum Node<'a> {
    Counted(i64),
    Kids(&'a [Object]),
}

/// A node on the way down a page tree ([`Reader::count_pages`]): its kids,
/// the next to look at, what's counted so far, and its number where it's
/// an object of its own.
struct Frame<'a> {
    kids: &'a [Object],
    next: usize,
    count: i64,
    number: Option<u32>,
}

impl Reader {
    /// Where PDFium stops reading `sections` (newest first), their rows
    /// `lists`, placed at `offsets`, to scan the file instead, once the
    /// `/Prev` chain has read, and why: the first object in use, by number,
    /// at an offset past the header, of the first-written section's table,
    /// `first_table`, merged as PDFium merges it over the cross-reference
    /// streams' rows ([`by_table`]), whose first word at its offset, past
    /// white-space and comments, isn't its number, as PDFium checks it
    /// before it reads any later section's rows; else a later section's
    /// `/XRefStm` that doesn't read, the earliest first, as PDFium reads
    /// them earliest first, and never the first-written section's, which
    /// PDFium doesn't read. None where it reads on.
    pub(super) fn stops<'f, F: Bytes<'f>, T>(
        &self,
        file: F,
        sections: &[Section],
        lists: &[(Rows, T)],
        first_table: Option<&Rows>,
        offsets: &[usize],
    ) -> Result<Option<(Unwritten, Stop)>, F::Error> {
        let last = sections.len().saturating_sub(1);
        if let Some(why) = self.misplaced(file, sections, lists, first_table, offsets)? {
            return Ok(Some((why, Stop::Verify)));
        }
        let unread = (sections[..last].iter().enumerate().rev())
            .find_map(|(k, section)| Some((k, section.stream_unread.clone()?)));
        Ok(unread.map(|(k, why)| (why, Stop::XrefStm(k))))
    }

    /// Why PDFium finds the first-written section's table, `first_table`,
    /// out of place ([`Reader::stops`]): its first object in use, by
    /// number, at an offset past the header, isn't there. None where it is,
    /// or where the first-written section isn't a table.
    fn misplaced<'f, F: Bytes<'f>, T>(
        &self,
        file: F,
        sections: &[Section],
        lists: &[(Rows, T)],
        first_table: Option<&Rows>,
        offsets: &[usize],
    ) -> Result<Option<Unwritten>, F::Error> {
        let Some(table) = first_table else {
            return Ok(None);
        };
        let mut checked = Rows::new();
        for (section, (listed, _)) in sections.iter().zip(lists) {
            if section.trailer.stream.is_some() {
                for (&n, &entry) in listed {
                    checked.entry(n).or_insert(entry);
                }
            }
        }
        by_table(&mut checked, table);
        let header = self.facts.header_offset.unwrap_or(0);
        let first = checked
            .iter()
            .find_map(|(&n, &(generation, entry))| match entry {
                XrefEntry::InUse { offset, .. } if offset > header => {
                    Some((ObjectId::new(n, generation), offset))
                }
                _ => None,
            });
        let Some((id, offset)) = first else {
            return Ok(None);
        };
        if self.word_at(file, offset)? == Some(id.number) {
            return Ok(None);
        }
        let last = sections.len().saturating_sub(1);
        let table = offsets.get(last).copied().unwrap_or(0);
        Ok(Some(Unwritten::Misplaced { table, id, offset }))
    }

    /// The number PDFium reads the first word at `offset` as, past
    /// white-space and comments, as its rules have them
    /// ([`Rules::Pdfium`]): none where it isn't a number ([`number_of`]).
    /// It tells nothing.
    fn word_at<'f, F: Bytes<'f>>(&self, file: F, offset: usize) -> Result<Option<u32>, F::Error> {
        let space = |b: u8| Rules::Pdfium.is_white_space(b);
        let mut cx = Cx::opening(false);
        file.read_from(&mut cx, offset, 0, |w, _| {
            let bytes = w.bytes;
            let Some(mut i) = offset.checked_sub(w.base) else {
                return Ok(None);
            };
            let short = |i: usize| i >= bytes.len() && !w.last;
            loop {
                while bytes.get(i).is_some_and(|&b| space(b)) {
                    i += 1;
                }
                if bytes.get(i) != Some(&b'%') {
                    break;
                }
                while bytes.get(i).is_some_and(|&b| !matches!(b, b'\r' | b'\n')) {
                    i += 1;
                }
            }
            let start = i;
            while bytes.get(i).is_some_and(|&b| !space(b) && !is_delimiter(b)) {
                i += 1;
            }
            if short(i) {
                w.touch();
                return Ok(None);
            }
            Ok(number_of(
                &bytes[start.min(bytes.len())..i.min(bytes.len())],
            ))
        })
    }

    /// [`Reader::rebuild`] of `file`, every byte.
    pub(super) fn rebuild_in<'f, F: Bytes<'f>>(
        &mut self,
        file: F,
        cx: &mut Cx,
        why: Unwritten,
    ) -> Result<Trailer, F::Error> {
        phase!(DEBUG, "scan", len = file.len());
        let all = file.all()?;
        Ok(self.rebuild(&all, cx, why))
    }

    /// The file `file` scanned for its objects as PDFium scans it, for
    /// `why`, its sections not read or not read on: each `n g obj` listed
    /// in the file's order ([`add_object`]), over what PDFium had read of
    /// the sections ([`Loaded`]), each object that scan doesn't find kept
    /// where they put it; each object stream's members are listed once
    /// the file's encryption is open ([`Reader::members_as_pdfium`]). The
    /// trailer it makes is each `trailer` dictionary and cross-reference
    /// stream's, in the file's order, a later one's keys over an earlier's
    /// ([`merge_trailer`]), merged over the sections' so; with what the last
    /// lacks of the document's keys said taken ([`Fault::Taken`]). Read so,
    /// the file is a scanned one, whatever its sections gave: no revision,
    /// no update, and the facts of a file whose sections don't read.
    fn rebuild(&mut self, file: &[u8], cx: &mut Cx, why: Unwritten) -> Trailer {
        let Scan {
            objects,
            object_streams,
            trailers,
            xref_streams,
        } = scan_by(
            file,
            Some(&|keyword, object| stream_end(file, keyword, object, self.limits.parser)),
        );
        let loaded = self.loaded.take().map(|loaded| *loaded).unwrap_or_default();
        let listed = listed_of(&objects);
        let kept: Vec<u32> = (loaded.rows.keys().copied())
            .filter(|n| !listed.contains_key(n))
            .collect();
        let mut entries = loaded.rows.clone();
        entries.extend(listed);
        self.entries = entries.into_iter().collect();
        self.scanned = true;
        self.revisions = Vec::new();
        self.update = None;
        self.facts = Facts {
            header_offset: self.facts.header_offset,
            recovered: true,
            ..Facts::default()
        };
        let mut numbers: Vec<u32> = object_streams.iter().map(|&(n, _)| n).collect();
        numbers.sort_unstable();
        numbers.dedup();
        self.object_streams = numbers.into();
        // The trailers, each `trailer` dictionary and cross-reference
        // stream's, in the file's order, each placed where it's written.
        let mut dicts: Vec<(usize, Object)> = Vec::new();
        for at in trailers {
            let read = Parser::at(file, at)
                .limits(self.limits.parser)
                .value(&mut cx.parsed(None));
            if let Ok(object) = read
                && object.as_dict().is_some()
            {
                dicts.push((at, object));
            }
        }
        for at in xref_streams {
            let Ok(read) = self.indirect(InMemory(file), cx, at, None);
            if let Ok((_, object, _)) = read
                && is_xref_stream(&object)
                && let Value::Stream(stream) = object.value()
            {
                let mut dict = Object::new(Value::Dict(stream.dict.clone()));
                dict.at = object.at;
                dicts.push((at, dict));
            }
        }
        dicts.sort_by_key(|&(at, _)| at);
        let last = dicts
            .last()
            .map(|(_, last)| last.as_dict().cloned().unwrap_or_default());
        let mut made: Option<Object> = None;
        for (_, object) in dicts {
            match &mut made {
                None => made = Some(object),
                Some(earlier) => {
                    let at = object.at;
                    if let (Some(earlier), Some(newer)) = (earlier.as_dict_mut(), object.as_dict())
                    {
                        merge_trailer(earlier, newer);
                    }
                    earlier.at = at;
                }
            }
        }
        if let (Some(made), Some(last)) = (&made, &last)
            && cx.wants
        {
            let taken: Vec<Vec<u8>> = (DOCUMENT.iter())
                .filter(|&&key| !last.has(key) && made.as_dict().is_some_and(|d| d.has(key)))
                .map(|&key| key.to_vec())
                .collect();
            if !taken.is_empty() {
                cx.tell(None, made.at, Fault::Taken { keys: taken });
            }
        }
        let object = match (loaded.trailer, made) {
            (Some(mut sections), Some(made)) => {
                if let (Some(sections), Some(made)) = (sections.as_dict_mut(), made.as_dict()) {
                    merge_trailer(sections, made);
                }
                sections.at = made.at;
                sections
            }
            (Some(sections), None) => sections,
            (None, Some(made)) => made,
            (None, None) => Object::new(Value::Dict(Dict::new())),
        };
        self.rebuild = Some(Box::new(Rebuild {
            objects,
            streams: object_streams,
            base: loaded.rows,
            kept,
        }));
        Trailer {
            stream: None,
            object,
            unwritten: Some(why),
        }
    }

    /// The object streams [`Reader::rebuild`] found, their members listed
    /// as PDFium lists them as it scans: at each stream's place in the
    /// file, each over an entry of its number listed before it, but one of
    /// a generation past 0, or a number whose own members were listed; and
    /// an `n g obj` of its number after the stream over it. Only each
    /// stream whose `n g obj` is the one listed for its number is decoded,
    /// once, kept as the container a member's read decodes, and only where
    /// the file's streams aren't encrypted, as PDFium reads them as
    /// written; where a password hasn't opened a file that encrypts its
    /// strings alone, its members are listed once one does. The whole
    /// merged over the sections' entries as [`Reader::rebuild`] merges it.
    /// What it listed, said ([`Fault::Members`]): the members whose number
    /// the scan found no `n g obj` of, and the streams whose objects don't
    /// read.
    pub(super) fn members_as_pdfium<'f, F: Bytes<'f>>(
        &mut self,
        file: F,
    ) -> Result<Option<Fault>, F::Error> {
        let Some(mut rebuild) = self.rebuild.take() else {
            return Ok(None);
        };
        let listed = listed_of(&rebuild.objects);
        // The entries as the scan's objects give them, no member listed.
        let mut entries = rebuild.base.clone();
        entries.extend(listed.iter().map(|(&n, &entry)| (n, entry)));
        self.entries = entries.into_iter().collect();
        self.slots = self.entries.iter().map(|_| OnceLock::new()).collect();
        let streams: Vec<(u32, usize)> = (rebuild.streams.iter().copied())
            .filter(|&(s, at)| {
                listed.get(&s)
                    == Some(&(
                        0,
                        XrefEntry::InUse {
                            offset: at,
                            generation: 0,
                        },
                    ))
            })
            .collect();
        let mut headers: Vec<(usize, u32, Vec<u32>)> = Vec::new();
        let mut unread = 0;
        if self.list.streams || !self.native_containers() {
            unread = streams.len();
        } else {
            for &(s, _) in &streams {
                self.containers.entry(s).or_default();
            }
            for &(s, at) in &streams {
                match self.container(file, s)?.map(|c| &c.header) {
                    Some(Ok(header)) => {
                        headers.push((at, s, header.iter().map(|&(n, _)| n).collect()));
                    }
                    _ => unread += 1,
                }
            }
        }
        // PDFium's listing, in the file's order: each object, then, at an
        // object stream's own, its members.
        let mut state = Rows::new();
        let mut holding: BTreeSet<u32> = BTreeSet::new();
        let mut next = headers.iter().peekable();
        for &(id, offset) in &rebuild.objects {
            add_object(&mut state, id, offset);
            while let Some((_, s, members)) = next.next_if(|(at, _, _)| *at == offset) {
                for (index, &n) in members.iter().enumerate() {
                    let Ok(index) = u32::try_from(index) else {
                        break;
                    };
                    if n == 0 || n > NUMBERS || *s > NUMBERS || holding.contains(&n) {
                        continue;
                    }
                    if state.get(&n).is_some_and(|&(generation, _)| generation > 0) {
                        continue;
                    }
                    let entry = XrefEntry::Compressed { stream: *s, index };
                    state.insert(n, (0, entry));
                    holding.insert(*s);
                }
            }
        }
        let found: BTreeSet<u32> = rebuild.objects.iter().map(|(id, _)| id.number).collect();
        let members: BTreeMap<u32, u32> = (state.iter())
            .filter_map(|(&n, &(_, entry))| match entry {
                XrefEntry::Compressed { stream, .. } if !found.contains(&n) => Some((n, stream)),
                _ => None,
            })
            .collect();
        let from: BTreeSet<u32> = members.values().copied().collect();
        rebuild.kept = (rebuild.base.keys().copied())
            .filter(|n| !state.contains_key(n))
            .collect();
        let mut entries = rebuild.base.clone();
        entries.extend(state);
        self.entries = entries.into_iter().collect();
        self.slots = self.entries.iter().map(|_| OnceLock::new()).collect();
        self.rebuild = Some(rebuild);
        let objects = members.len();
        let fault = Fault::Members {
            objects: count(objects),
            streams: count(from.len()),
            unread: count(unread),
        };
        Ok((objects > 0 || unread > 0).then_some(fault))
    }

    /// Whether number `n`'s entry is one the sections gave, in a file
    /// [`Reader::rebuild`] scanned: an object the scan didn't find, read
    /// where they put it.
    pub(super) fn sections_gave(&self, n: u32) -> bool {
        (self.rebuild.as_ref()).is_some_and(|rebuild| rebuild.kept.binary_search(&n).is_ok())
    }

    /// Whether PDFium, having opened the file by its sections, would scan
    /// it for want of a page: under [`Recovery::Pdfium`], where the
    /// sections read and the file is read as of none given, it isn't
    /// encrypted or a password opened it, as PDFium opens no other, and its
    /// trailer's `/Root` gives no page ([`Reader::gives_pages`]).
    pub(super) fn scans_for_pages<'f, F: Bytes<'f>>(&self, file: F) -> Result<bool, F::Error> {
        let opened = matches!(
            self.encryption,
            Encryption::None | Encryption::Opened { .. }
        );
        if self.recovery != Recovery::Pdfium || self.scanned || self.as_of.is_some() || !opened {
            return Ok(false);
        }
        Ok(!self.gives_pages(file)?)
    }

    /// Whether the trailer's `/Root` gives a page, as PDFium asks of a file
    /// it opens: it refers to an object that's a dictionary, or a stream's,
    /// whose `/Pages` is one too, by a reference or not ([`Reader::direct`]);
    /// and that has no `/Kids`, or PDFium counts a page under it
    /// ([`Reader::count_pages`]).
    pub(super) fn gives_pages<'f, F: Bytes<'f>>(&self, file: F) -> Result<bool, F::Error> {
        let Some(&Value::Ref(root)) = self.trailer.dict().get(b"Root").map(Object::value) else {
            return Ok(false);
        };
        let Some(catalog) = self.by_number(file, root)?.and_then(dict_of) else {
            return Ok(false);
        };
        let Some(node) = catalog.get(b"Pages") else {
            return Ok(false);
        };
        let number = match node.value() {
            Value::Ref(id) => Some(id.number),
            _ => None,
        };
        let Some(pages) = self.direct(file, node)?.and_then(dict_of) else {
            return Ok(false);
        };
        if !pages.has(b"Kids") {
            return Ok(true);
        }
        Ok(self
            .count_pages(file, pages, number)?
            .is_some_and(|n| n > 0))
    }

    /// Object `id`, by its number alone, as PDFium reads a reference: the
    /// object its number's entry lists, read the first time it's asked
    /// for; none where it isn't listed or doesn't read.
    fn by_number<'f, F: Bytes<'f>>(
        &self,
        file: F,
        id: ObjectId,
    ) -> Result<Option<&Object>, F::Error> {
        let Some(i) = self.index(id.number).filter(|_| id.number != 0) else {
            return Ok(None);
        };
        if self.entries[i].1.1 == XrefEntry::Free {
            return Ok(None);
        }
        Ok((self.slot(file, i)?.object.as_ref().ok()).map(|(object, _)| object))
    }

    /// `o`, or the object it refers to, as PDFium reads a value through
    /// one reference: none where that doesn't read, or is a reference.
    fn direct<'a, 'f, F: Bytes<'f>>(
        &'a self,
        file: F,
        o: &'a Object,
    ) -> Result<Option<&'a Object>, F::Error> {
        let Value::Ref(id) = *o.value() else {
            return Ok(Some(o));
        };
        let object = self.by_number(file, id)?;
        Ok(object.filter(|o| !matches!(o.value(), Value::Ref(_))))
    }

    /// A page tree node `node`, as PDFium first looks at it: its `/Count`,
    /// where that's past 0 and under [`PAGES`]; else its `/Kids`, where
    /// that's an array; else no page.
    fn node<'a, 'f, F: Bytes<'f>>(&'a self, file: F, node: &'a Dict) -> Result<Node<'a>, F::Error> {
        let count = match node.get(b"Count") {
            Some(count) => self.direct(file, count)?.map_or(0, integer_of),
            None => 0,
        };
        if count > 0 && count < PAGES {
            return Ok(Node::Counted(count));
        }
        let kids = match node.get(b"Kids") {
            Some(kids) => self.direct(file, kids)?,
            None => None,
        };
        Ok(match kids.map(Object::value) {
            Some(Value::Array(kids)) => Node::Kids(kids),
            _ => Node::Counted(0),
        })
    }

    /// How many pages PDFium counts under page tree node `root`, object
    /// `number` where it's one of its own: [`Reader::node`]'s count, or,
    /// over its kids, each a dictionary or a stream's through one reference
    /// that isn't a node on the way down to it, one for a page and a node's
    /// count for a node (by its `/Type`, `/Pages` or `/Page`, else by
    /// whether it has `/Kids`), a node counted once taken as its count
    /// after, where that's past 0, as PDFium writes the count it makes into
    /// the node. None where the count comes to [`PAGES`], or [`VISITS`]
    /// kids are looked at.
    fn count_pages<'a, 'f, F: Bytes<'f>>(
        &'a self,
        file: F,
        root: &'a Dict,
        number: Option<u32>,
    ) -> Result<Option<i64>, F::Error> {
        let kids = match self.node(file, root)? {
            Node::Counted(count) => return Ok(Some(count)),
            Node::Kids(kids) => kids,
        };
        let mut path = vec![Frame {
            kids,
            next: 0,
            count: 0,
            number,
        }];
        let mut counted: BTreeMap<u32, i64> = BTreeMap::new();
        let mut visits = 0usize;
        loop {
            let i = path.len() - 1;
            let (kids, next) = (path[i].kids, path[i].next);
            let Some(kid) = kids.get(next) else {
                let done = path.pop().expect("a node on the way");
                if let Some(n) = done.number
                    && done.count > 0
                {
                    counted.insert(n, done.count);
                }
                let Some(parent) = path.last_mut() else {
                    return Ok(Some(done.count));
                };
                parent.count += done.count;
                if parent.count >= PAGES {
                    return Ok(None);
                }
                continue;
            };
            path[i].next += 1;
            visits += 1;
            if visits > VISITS {
                return Ok(None);
            }
            let number = match kid.value() {
                Value::Ref(id) => Some(id.number),
                _ => None,
            };
            let Some(dict) = self.direct(file, kid)?.and_then(dict_of) else {
                continue;
            };
            if number.is_some() && path.iter().any(|frame| frame.number == number) {
                continue;
            }
            let branch = match dict.get(b"Type").and_then(Object::as_name) {
                Some(b"Pages") => true,
                Some(b"Page") => false,
                _ => dict.has(b"Kids"),
            };
            let memo = number.and_then(|n| counted.get(&n).copied());
            match (branch, memo) {
                (false, _) => path[i].count += 1,
                (true, Some(count)) => path[i].count += count,
                (true, None) => match self.node(file, dict)? {
                    Node::Counted(count) => path[i].count += count,
                    Node::Kids(kids) => {
                        path.push(Frame {
                            kids,
                            next: 0,
                            count: 0,
                            number,
                        });
                        continue;
                    }
                },
            }
            if path[i].count >= PAGES {
                return Ok(None);
            }
        }
    }
}

#[cfg(test)]
mod tests;
