//! Where a file's cross-reference sections or the objects they list aren't
//! where it says, reading them where the file proves they are
//! ([`Recovery::Verified`]): a section that doesn't read at its offset, read
//! where that offset puts it counted from the file's first byte rather
//! than its header, else at the nearest `xref` or cross-reference stream
//! within [`NEAR`] bytes, exactly there, that reads; and a section's
//! entries read at one shift, each at its own `n g obj` there, else, once
//! the file is shown not to be as written, where a scan of the file finds
//! its `n g obj`, else where it's written. A section read where
//! it's said pays two probes, its first object's header and its
//! furthest's, each where it's written. What's looked at to tell where
//! they are tells nothing of its own: only what's found is said, one fault
//! a section.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use pdf_objects::fault::Place;
use pdf_objects::object::ObjectId;
use pdf_objects::parse::{self, Parser};
use pdf_tokenizer::{Kind, Tokenizer, is_delimiter, is_white_space};

use super::window::Bytes;
use super::{
    Cx, Fault, Late, Misnumbered, NEAR, Reader, Recovery, Rows, Unclosed, Unended, XrefEntry,
    ended_of, scan_of, span,
};
use crate::source::Origin;

/// How many bytes from where an `n g obj` starts its three tokens are
/// looked for in: one as written ends well within it.
const HEAD: usize = 64;

/// Where a section was read away from where `startxref`, a `/Prev` or a
/// `/XRefStm` puts it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Found {
    /// Where it's said to be.
    pub(super) at: usize,
    /// Where it's read.
    pub(super) found: usize,
    /// Whether that's where the offset puts it counted from the file's
    /// first byte, not its header.
    pub(super) from_start: bool,
}

impl Found {
    /// How far it is from where it's said to be.
    pub(super) fn by(&self) -> i64 {
        between(self.at, self.found)
    }
}

/// What reading a section's entries at a shift found
/// ([`Reader::shifted`]).
#[derive(Debug, Clone)]
pub(super) struct Shift {
    /// How far.
    pub(super) by: i64,
    /// The numbers whose entries it moved.
    pub(super) moved: Vec<u32>,
    /// The numbers whose entries are neither where the section puts them
    /// nor at the shift, and are read where a scan of the file finds them.
    pub(super) scanned: Vec<u32>,
    /// How many entries in use are at none of those places.
    pub(super) neither: usize,
}

impl Shift {
    /// How each entry it moved was found, by number.
    pub(super) fn origins(self) -> BTreeMap<u32, Origin> {
        let by = self.by;
        let moved = (self.moved.into_iter()).map(|n| (n, Origin::Shifted { by }));
        let scanned = (self.scanned.into_iter()).map(|n| (n, Origin::Recovered));
        moved.chain(scanned).collect()
    }
}

/// `n` as a recovery's fault counts it: object numbers, or object streams
/// among them, so a `u32` holds them.
pub(super) fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// How far `to` is from `from`: less than 0 before it.
pub(super) fn between(from: usize, to: usize) -> i64 {
    let by = i64::try_from(from.abs_diff(to)).unwrap_or(i64::MAX);
    if to < from { -by } else { by }
}

/// Whether `b` ends a regular character's run, as white-space and
/// delimiters do (ISO 32000-2, 7.2.3).
fn stops(b: u8) -> bool {
    is_white_space(b) || is_delimiter(b)
}

/// Whether `p` starts a token on its own in `bytes`, the file's from
/// `base`: at the file's start, or after white-space or a delimiter.
fn alone_at(bytes: &[u8], base: usize, p: usize) -> Option<usize> {
    let i = p.checked_sub(base)?;
    match i {
        0 if base == 0 => Some(0),
        0 => None,
        _ => stops(*bytes.get(i - 1)?).then_some(i),
    }
}

/// The `n g obj` that starts exactly at byte `p` of `bytes`, the file's from
/// `base`, which end the file where `last`: its number and generation,
/// digits each, white-space or a comment between, `obj` the keyword; and
/// none where it doesn't, or its tokens run to the bytes' end short of the
/// file's.
pub(crate) fn header_exactly(bytes: &[u8], base: usize, p: usize, last: bool) -> Option<ObjectId> {
    let i = alone_at(bytes, base, p)?;
    let within = |end: usize| end < bytes.len() || last;
    let digits =
        |token: &pdf_tokenizer::Token<'_>| token.bytes.first().is_some_and(u8::is_ascii_digit);
    let mut t = Tokenizer::at(bytes, i);
    let n = t.next()?;
    if n.kind != Kind::Integer || n.start != i || !digits(&n) || !within(n.end()) {
        return None;
    }
    let g = t.next_significant()?;
    if g.kind != Kind::Integer || !digits(&g) || !within(g.end()) {
        return None;
    }
    let obj = t.next_significant()?;
    if !obj.is_keyword(b"obj") || !within(obj.end()) {
        return None;
    }
    let number = u32::try_from(n.integer().ok()?).ok()?;
    let generation = u16::try_from(g.integer().ok()?).ok()?;
    Some(ObjectId::new(number, generation))
}

/// Whether `xref`, a word of its own, starts exactly at byte `p` of
/// `bytes`, the file's from `base`: not the `xref` inside `startxref`.
fn xref_exactly(bytes: &[u8], base: usize, p: usize) -> bool {
    let Some(i) = alone_at(bytes, base, p) else {
        return false;
    };
    bytes[i..].starts_with(b"xref") && bytes.get(i + 4).is_some_and(|&b| stops(b))
}

/// The first byte at or past `p` in `bytes`, the file's from `base`, that
/// isn't white-space: where what's at `p` starts for a reader that reads
/// past the line's end there. None where the bytes end first.
fn past_white_space(bytes: &[u8], base: usize, p: usize) -> Option<usize> {
    let i = p.checked_sub(base)?;
    let skip = bytes.get(i..)?.iter().position(|&b| !is_white_space(b))?;
    Some(p + skip)
}

/// Where a scan of a file finds each object's `n g obj` ([`scanned`]).
struct Scanned {
    /// The last in the file of each, by its id.
    last: BTreeMap<ObjectId, usize>,
    /// The earlier ones of each whose last is past a literal string that
    /// doesn't close the scan read past, in the file's order.
    earlier: BTreeMap<ObjectId, Vec<usize>>,
    /// Where those past such a string are and their bytes end, in the
    /// file's order.
    past: Vec<(usize, usize)>,
}

/// Where a scan of `file`, every byte, finds each object's `n g obj`
/// ([`scan_of`], under `limits`, as the verified recovery scans), exactly
/// ([`header_exactly`]), by its id, of those before byte `within` where
/// that's given ([`Reader::scanned_at`] reads it); each stream before there
/// whose data the scan ended at its `endobj`, or found no end to and read
/// on through, which is said ([`Fault::StreamEnd`]); the strings before
/// there it found no `)` closing and read past, which are said
/// ([`Fault::Unclosed`]); and the strings whose lines inside them gave it
/// objects before there, which are said ([`Fault::InString`]).
fn scanned(
    file: &[u8],
    within: Option<usize>,
    limits: parse::Limits,
) -> (Scanned, Vec<Unended>, Unclosed, Vec<Late>) {
    let scan = scan_of(file, limits, Recovery::Verified, within);
    let before = |at: usize| within.is_none_or(|within| at < within);
    let exact: Vec<(ObjectId, usize)> = (scan.objects.into_iter())
        .filter(|&(id, at)| before(at) && header_exactly(file, 0, at, true) == Some(id))
        .collect();
    let mut last = BTreeMap::new();
    for &(id, at) in &exact {
        last.insert(id, at);
    }
    let mut earlier: BTreeMap<ObjectId, Vec<usize>> = BTreeMap::new();
    if !scan.past.is_empty() {
        for &(id, at) in &exact {
            let found = last[&id];
            let past = (scan.past).binary_search_by_key(&found, |&(at, _)| at);
            if at != found && past.is_ok() {
                earlier.entry(id).or_default().push(at);
            }
        }
    }
    let mut unended = scan.unended;
    unended.retain(|unended| before(unended.start));
    let scanned = Scanned {
        last,
        earlier,
        past: scan.past,
    };
    (scanned, unended, scan.unclosed, scan.late)
}

impl Reader {
    /// The places a section said to be at `at`, which doesn't read there,
    /// may be at, in the order they're tried: where the offset puts it
    /// counted from the file's first byte, where the header isn't there
    /// (ISO 32000-2, 7.5.2, counts from the header), where an `xref` or an
    /// `n g obj` starts exactly there or past white-space alone, as a
    /// writer counting so that gives the line's end before a section puts
    /// it; then each `xref` or `n g obj` that starts exactly within
    /// [`NEAR`] bytes of it, the nearer first, the earlier of two as near.
    /// Each with whether it's the first.
    ///
    /// The search near is lopdf's lenient reader's technique, which takes
    /// the nearest `xref` within 64 bytes of an offset that isn't at a
    /// section, never the one in `startxref`. Here a cross-reference
    /// stream's `n g obj` is looked for too, and either only where it starts
    /// exactly.
    pub(super) fn near<'f, F: Bytes<'f>>(
        &self,
        file: F,
        at: usize,
    ) -> Result<Vec<(usize, bool)>, F::Error> {
        let mut places = Vec::new();
        if self.recovery != Recovery::Verified {
            return Ok(places);
        }
        let len = file.len();
        let header = self.facts.header_offset.unwrap_or(0);
        if let Some(p) = at.checked_sub(header).filter(|&p| header > 0 && p < len) {
            let (lo, hi) = (p.saturating_sub(1), p.saturating_add(HEAD).min(len));
            if let Some(bytes) = file.get(lo..hi)?
                && let Some(q) = past_white_space(&bytes, lo, p)
                && (xref_exactly(&bytes, lo, q)
                    || header_exactly(&bytes, lo, q, hi == len).is_some())
            {
                places.push((p, true));
            }
        }
        let (lo, hi) = (
            at.saturating_sub(NEAR + 1),
            at.saturating_add(NEAR + HEAD).min(len),
        );
        let Some(bytes) = file.get(lo..hi)? else {
            return Ok(places);
        };
        for d in 1..=NEAR {
            for p in [at.checked_sub(d), at.checked_add(d)].into_iter().flatten() {
                let starts = xref_exactly(&bytes, lo, p)
                    || header_exactly(&bytes, lo, p, hi == len).is_some();
                if starts && !places.iter().any(|&(q, _)| q == p) {
                    places.push((p, false));
                }
            }
        }
        Ok(places)
    }

    /// The `n g obj` at `offset`, read as the reader reads an object there,
    /// past white-space and comments: none where it doesn't read. It tells
    /// nothing, and reads [`HEAD`] bytes where they hold it, as they do
    /// but past a run of white-space or a comment.
    fn header_at<'f, F: Bytes<'f>>(
        &self,
        file: F,
        offset: usize,
    ) -> Result<Option<ObjectId>, F::Error> {
        let len = file.len();
        let hi = offset.saturating_add(HEAD).min(len);
        if let Some(bytes) = file.get(offset..hi)? {
            let parser = Parser::at(&bytes, 0).window(offset, hi == len);
            match parser.limits(self.limits.parser).header() {
                Err(parse::Error::Short { .. }) => {}
                read => return Ok(read.ok()),
            }
        }
        let mut cx = Cx::opening(false);
        file.read_from(&mut cx, offset, 0, |w, _| {
            let mut parser = w.parser(offset).limits(self.limits.parser);
            Ok(w.parsed(parser.header()).ok())
        })
    }

    /// The `n g obj` `by` bytes from `offset`: where it starts exactly
    /// there ([`Reader::exactly`]), or, where `by` takes the header's
    /// offset back, as a file whose offsets count from its first byte puts
    /// it, past white-space alone there, as such a writer that gives the
    /// line's end before an object puts it.
    fn at_shift<'f, F: Bytes<'f>>(
        &self,
        file: F,
        offset: usize,
        by: i64,
    ) -> Result<Option<ObjectId>, F::Error> {
        let header = self.facts.header_offset.unwrap_or(0);
        if header > 0 && i64::try_from(header).map(|h| -h) == Ok(by) {
            let len = file.len();
            let Some(p) = offset.checked_sub(header).filter(|&p| p < len) else {
                return Ok(None);
            };
            let (lo, hi) = (p.saturating_sub(1), p.saturating_add(HEAD).min(len));
            let bytes = file.get(lo..hi)?;
            return Ok(bytes.and_then(|bytes| {
                let q = past_white_space(&bytes, lo, p)?;
                header_exactly(&bytes, lo, q, hi == len)
            }));
        }
        self.exactly(file, offset, by)
    }

    /// The numbers in use of a table's first subsection, `misnumbered`,
    /// numbered from 1 with object 0's row first, its rows `table` as read,
    /// where the file proves it's numbered from 0: at the first of `shifts`
    /// at which each row in use is at its own `n g obj` as the number one
    /// before its row's and its generation, exactly
    /// ([`Reader::at_shift`]), and at least one is in use. So none is at
    /// its own as the number written, and the subsection is renumbered
    /// from 0 ([`Fault::Renumbered`]); none where at each shift some row is
    /// otherwise, so a file whose rows land both ways, or neither, is read
    /// as written. A window each row in use a shift, the first that isn't
    /// so ending it.
    pub(super) fn renumbers<'f, F: Bytes<'f>>(
        &self,
        file: F,
        table: &Rows,
        misnumbered: Misnumbered,
        shifts: impl Iterator<Item = i64>,
    ) -> Result<Option<Vec<u32>>, F::Error> {
        'shift: for by in shifts {
            let mut renumbered = Vec::new();
            for n in 1..=misnumbered.count {
                let Some(&(generation, XrefEntry::InUse { offset, .. })) = table.get(&n) else {
                    continue;
                };
                if self.at_shift(file, offset, by)? != Some(ObjectId::new(n - 1, generation)) {
                    continue 'shift;
                }
                renumbered.push(n - 1);
            }
            return Ok((!renumbered.is_empty()).then_some(renumbered));
        }
        Ok(None)
    }

    /// The `n g obj` that starts exactly `by` bytes from `offset`
    /// ([`header_exactly`]): none where none does, or that's outside the
    /// file.
    fn exactly<'f, F: Bytes<'f>>(
        &self,
        file: F,
        offset: usize,
        by: i64,
    ) -> Result<Option<ObjectId>, F::Error> {
        let len = file.len();
        let p = isize::try_from(by)
            .ok()
            .and_then(|by| offset.checked_add_signed(by));
        let Some(p) = p.filter(|&p| p < len) else {
            return Ok(None);
        };
        let (lo, hi) = (p.saturating_sub(1), p.saturating_add(HEAD).min(len));
        let bytes = file.get(lo..hi)?;
        Ok(bytes.and_then(|bytes| header_exactly(&bytes, lo, p, hi == len)))
    }

    /// How far from `offset` object `id`'s `n g obj` starts exactly, the
    /// nearest within [`NEAR`] bytes, the earlier of two as near: none
    /// where it doesn't.
    fn found_near<'f, F: Bytes<'f>>(
        &self,
        file: F,
        id: ObjectId,
        offset: usize,
    ) -> Result<Option<i64>, F::Error> {
        let len = file.len();
        let (lo, hi) = (
            offset.saturating_sub(NEAR + 1),
            offset.saturating_add(NEAR + HEAD).min(len),
        );
        let Some(bytes) = file.get(lo..hi)? else {
            return Ok(None);
        };
        for d in 1..=NEAR {
            for p in [offset.checked_sub(d), offset.checked_add(d)]
                .into_iter()
                .flatten()
            {
                if header_exactly(&bytes, lo, p, hi == len) == Some(id) {
                    return Ok(Some(between(offset, p)));
                }
            }
        }
        Ok(None)
    }

    /// The shift a section's own reading proves, of its entries `rows`:
    /// its own move, where it was read elsewhere (`found`); else, where its
    /// first entry in use, by number, or the one furthest into the file,
    /// which a cut is likeliest to have moved, isn't read where it's
    /// written, the header's offset taken back, where its `n g obj` starts
    /// there, past white-space alone ([`Reader::at_shift`]), or how far from
    /// it its `n g obj` starts exactly within [`NEAR`] bytes. None where the
    /// section was read where
    /// it's said and those objects are where they're written, or found
    /// nowhere: two probes a section, each a window read at an object's
    /// offset.
    fn own_shift<'f, F: Bytes<'f>>(
        &self,
        file: F,
        rows: &Rows,
        found: Option<&Found>,
    ) -> Result<Option<i64>, F::Error> {
        if let Some(found) = found {
            return Ok(Some(found.by()));
        }
        let in_use = (rows.iter()).filter_map(|(&n, &(generation, entry))| match entry {
            XrefEntry::InUse { offset, .. } if n != 0 => {
                Some((ObjectId::new(n, generation), offset))
            }
            _ => None,
        });
        let first = in_use.clone().next();
        let furthest = in_use
            .max_by_key(|&(_, offset)| offset)
            .filter(|&last| Some(last) != first);
        let header = self.facts.header_offset.unwrap_or(0);
        for (id, offset) in [first, furthest].into_iter().flatten() {
            if self.header_at(file, offset)? == Some(id) {
                continue;
            }
            if header > 0 {
                let by = -i64::try_from(header).unwrap_or(i64::MAX);
                if self.at_shift(file, offset, by)? == Some(id) {
                    return Ok(Some(by));
                }
            }
            if let Some(by) = self.found_near(file, id, offset)? {
                return Ok(Some(by));
            }
        }
        Ok(None)
    }

    /// Each section's entries, `lists` (each with what reading them told,
    /// left as it is), read at one shift where the file proves one: the
    /// section's own ([`Reader::own_shift`]), or, where it has none, the
    /// first another section has, as a cut in a file moves everything
    /// after it, and an earlier section may list objects past it. Each
    /// entry in use is read where it's written, where its `n g obj` is,
    /// else at its own `n g obj` at the shift ([`Reader::at_shift`]), its
    /// offset moved there; else, as the file is then shown not to be as
    /// written, where a scan of its every byte finds its `n g obj`, its
    /// number and generation, exactly, the last in the file ([`scan_of`],
    /// as a file whose sections don't read is scanned) before byte
    /// `within` where that's given, as a reading as of a section takes
    /// nothing later updates wrote, but one past a string that doesn't
    /// close the scan read past only where it reads, which is said
    /// ([`Reader::scanned_at`]); its offset moved there; else where it's
    /// written. Where the scan is made, each stream before `within` whose
    /// data it ended at its `endobj`, or found no end to and read on
    /// through, is said to `cx` ([`Fault::StreamEnd`]), the strings
    /// before there it found no `)` closing and read past
    /// ([`Fault::Unclosed`]), and those whose lines inside them gave it an
    /// object an entry is moved to, with those alone ([`Fault::InString`]);
    /// and each stream whose data it ended at its `endobj` is kept, so that
    /// its object's read reads it so, wherever the entry is read
    /// ([`Reader::ended`]), as the file is being scanned, while the copies
    /// it checks are read as that read does. None for any section where no
    /// section has a shift, the file read as written; each section `said`
    /// to be elsewhere has its move.
    pub(super) fn shifted<'f, F: Bytes<'f>, T>(
        &mut self,
        file: F,
        cx: &mut Cx,
        lists: &mut [(Rows, T)],
        said: &[Option<Found>],
        within: Option<usize>,
    ) -> Result<Vec<Option<Shift>>, F::Error> {
        let mut own = Vec::with_capacity(lists.len());
        if self.recovery == Recovery::Verified {
            for ((rows, _), found) in lists.iter().zip(said) {
                own.push(self.own_shift(file, rows, found.as_ref())?);
            }
        }
        let Some(&first) = own.iter().flatten().next() else {
            return Ok(lists.iter().map(|_| None).collect());
        };
        // Where a scan of the file finds each object's `n g obj`, made
        // where an entry is first found at neither place, with the file's
        // bytes.
        let mut scan: Option<(Cow<'f, [u8]>, Scanned)> = None;
        // Where the entries moved to the scan's places are.
        let mut recovered = BTreeSet::new();
        // Where among the faults told the strings whose lines gave the scan
        // objects go, and those strings.
        let mut late: (usize, Vec<Late>) = (0, Vec::new());
        let mut shifts = Vec::with_capacity(lists.len());
        for ((rows, _), by) in lists.iter_mut().zip(own) {
            let by = by.unwrap_or(first);
            let mut shift = Shift {
                by,
                moved: Vec::new(),
                scanned: Vec::new(),
                neither: 0,
            };
            for (&n, (generation, entry)) in rows.iter_mut() {
                let XrefEntry::InUse { offset, .. } = entry else {
                    continue;
                };
                let id = ObjectId::new(n, *generation);
                if n == 0 || self.header_at(file, *offset)? == Some(id) {
                    continue;
                }
                // Where it is, exactly at the shift: inside the file, so
                // the sum is one.
                let moved = isize::try_from(by)
                    .ok()
                    .and_then(|by| offset.checked_add_signed(by));
                if let (true, Some(moved)) = (self.at_shift(file, *offset, by)? == Some(id), moved)
                {
                    *offset = moved;
                    shift.moved.push(n);
                    continue;
                }
                if scan.is_none() {
                    let all = file.all()?;
                    let (found, unended, unclosed, found_late) =
                        scanned(&all, within, self.limits.parser);
                    for unended in &unended {
                        cx.tell(None, Some(unended.at()), unended.fault());
                    }
                    if let Some((at, fault)) = unclosed.fault() {
                        cx.tell(None, Some(at), fault);
                    }
                    late = (cx.faults.len(), found_late);
                    self.ended = ended_of(&unended).into();
                    scan = Some((all, found));
                }
                let at = match &mut scan {
                    Some((all, found)) => self.scanned_at(all, found, id, cx),
                    None => None,
                };
                match at {
                    Some(at) if at != *offset => {
                        *offset = at;
                        shift.scanned.push(n);
                        recovered.insert(at);
                    }
                    _ => shift.neither += 1,
                }
            }
            shifts.push(Some(shift));
        }
        // The strings whose lines gave the scan an object an entry is moved
        // to, each with those, said where the scan's other faults are: the
        // others give the reading nothing.
        let (at, mut late) = late;
        late.retain_mut(|late| {
            late.objects.retain(|(_, at)| recovered.contains(at));
            !late.objects.is_empty()
        });
        if cx.wants {
            let said = late
                .iter()
                .map(|late| (Place::new(None, Some(late.at())), late.fault()));
            cx.faults.splice(at..at, said);
        }
        Ok(shifts)
    }

    /// Where `found`, a scan of `file`, finds object `id`'s `n g obj` to
    /// read it at: the last in the file, but where that's past a string
    /// that doesn't close the scan read past and doesn't read, an earlier
    /// one, as [`Reader::scan`] takes it ([`Reader::standing`]), said to
    /// `cx` the first time it's looked for ([`Fault::Earlier`]).
    fn scanned_at(
        &self,
        file: &[u8],
        found: &mut Scanned,
        id: ObjectId,
        cx: &mut Cx,
    ) -> Option<usize> {
        let &last = found.last.get(&id)?;
        let Some(earlier) = found.earlier.remove(&id) else {
            return Some(last);
        };
        let copies: Vec<(ObjectId, usize)> = (earlier.into_iter().chain([last]))
            .map(|at| (id, at))
            .collect();
        let Some((_, at, later)) = self.standing(file, &copies, &found.past) else {
            return Some(last);
        };
        found.last.insert(id, at);
        cx.tell(
            None,
            Some(span(later, later + 1)),
            Fault::Earlier { id, at, later },
        );
        Some(at)
    }

    /// Each object in the object streams a scan found
    /// ([`Reader::scan`]), listed as their headers give them: by the last
    /// such stream in the file that has it, as an update appends, unless
    /// the scan found its number at an offset, which stands; at generation
    /// 0, read from the stream ([`Origin::Stream`]). But what a literal
    /// string closed late holds stands over nothing the scan found as
    /// written ([`Fault::InString`]): a stream whose `n g obj` starts a
    /// line inside one lists its members under those of every other,
    /// wherever each is; and a member of a stream found as written stands
    /// over an object found inside such a string, an object stream among
    /// them, which then lists no member, the string then saying only the
    /// objects that stand ([`Reader::said_late`]). Each stream it decodes
    /// is kept as the container a member's read decodes, so it's decoded
    /// once, its faults told once and the budget spent once. Where the
    /// reader can't decode them, in an encrypted file
    /// no password opened, none is listed, and a password that opens it
    /// later lists them as one given at the open would; what it listed
    /// before is listed anew. What it found, said
    /// ([`Fault::Members`]): none where the scan
    /// found no object stream, or listed none and read every one's header.
    /// In a file whose sections read, a member no section lists isn't
    /// listed: a reference to it is to no object (ISO 32000-2, 7.3.10).
    /// Under [`Recovery::Pdfium`], as PDFium lists them
    /// ([`Reader::members_as_pdfium`]).
    pub(super) fn members<'f, F: Bytes<'f>>(&mut self, file: F) -> Result<Option<Fault>, F::Error> {
        if self.recovery == Recovery::Pdfium {
            return self.members_as_pdfium(file);
        }
        if !self.scanned || self.object_streams.is_empty() {
            return Ok(None);
        }
        let listed = |&(_, (_, entry)): &(u32, (u16, XrefEntry))| {
            !matches!(entry, XrefEntry::Compressed { .. })
        };
        if !self.entries.iter().all(listed) {
            self.entries = self.entries.iter().copied().filter(listed).collect();
            self.slots = self.entries.iter().map(|_| OnceLock::new()).collect();
        }
        // The objects found where a line starts inside a string.
        let late: BTreeSet<u32> = (self.late.iter())
            .flat_map(|late| late.objects.iter().map(|(id, _)| id.number))
            .collect();
        // The object streams, in the order their offsets come in the file,
        // those found inside a string first.
        let mut streams: Vec<(bool, usize, u32)> = (self.object_streams.iter())
            .filter_map(|&s| match self.entry(s) {
                Some(&(0, XrefEntry::InUse { offset, .. })) => {
                    Some((!late.contains(&s), offset, s))
                }
                _ => None,
            })
            .collect();
        streams.sort_unstable();
        let mut headers: Vec<(u32, Vec<u32>)> = Vec::new();
        let mut unread = 0;
        match self.native_containers() {
            false => unread = streams.len(),
            true => {
                for &(_, _, s) in &streams {
                    self.containers.entry(s).or_default();
                }
                for &(_, _, s) in &streams {
                    match self.container(file, s)?.map(|c| &c.header) {
                        Some(Ok(header)) => {
                            headers.push((s, header.iter().map(|&(n, _)| n).collect()));
                        }
                        _ => unread += 1,
                    }
                }
            }
        }
        // The objects found inside a string that a stream found as written
        // lists, whose members stand over them.
        let over: BTreeSet<u32> = (headers.iter())
            .filter(|(s, _)| !late.contains(s))
            .flat_map(|(_, numbers)| numbers.iter().copied())
            .filter(|n| late.contains(n))
            .collect();
        headers.retain(|(s, _)| !over.contains(s));
        let mut members = BTreeMap::new();
        for (stream, numbers) in &headers {
            for (index, &n) in numbers.iter().enumerate() {
                let Ok(index) = u32::try_from(index) else {
                    break;
                };
                if n != 0 && (self.index(n).is_none() || over.contains(&n)) {
                    members.insert(n, (*stream, index));
                }
            }
        }
        if !over.is_empty() {
            for late in &mut self.late {
                late.objects.retain(|(id, _)| !over.contains(&id.number));
            }
        }
        let from: BTreeSet<u32> = members.values().map(|&(stream, _)| stream).collect();
        let objects = members.len();
        if !members.is_empty() {
            let mut entries: BTreeMap<u32, (u16, XrefEntry)> = std::mem::take(&mut self.entries)
                .into_vec()
                .into_iter()
                .collect();
            for (n, (stream, index)) in members {
                entries.insert(n, (0, XrefEntry::Compressed { stream, index }));
            }
            self.entries = entries.into_iter().collect();
            // A slot for each entry, nothing read; the object streams kept
            // as decoded.
            self.slots = self.entries.iter().map(|_| OnceLock::new()).collect();
        }
        let fault = Fault::Members {
            objects: count(objects),
            streams: count(from.len()),
            unread: count(unread),
        };
        Ok((objects > 0 || unread > 0).then_some(fault))
    }

    /// What [`Reader::members`] says where none of the object streams a
    /// scan found is read: none of their members listed, and each said as
    /// one whose objects don't read.
    pub(super) fn unlisted(&mut self) -> Option<Fault> {
        let listed = |&(_, (_, entry)): &(u32, (u16, XrefEntry))| {
            !matches!(entry, XrefEntry::Compressed { .. })
        };
        if !self.entries.iter().all(listed) {
            self.entries = self.entries.iter().copied().filter(listed).collect();
            self.slots = self.entries.iter().map(|_| OnceLock::new()).collect();
        }
        let unread = self.object_streams.len();
        let fault = Fault::Members {
            objects: 0,
            streams: 0,
            unread: count(unread),
        };
        (self.scanned && unread > 0).then_some(fault)
    }

    /// What `faults`, those said as the file was opened, say of the strings
    /// whose lines inside them gave a scan objects ([`Fault::InString`]),
    /// said again of the objects that stand once the object streams'
    /// members are listed ([`Reader::members`]): a string none of whose
    /// objects stands, said no more.
    pub(super) fn said_late(&self, faults: &mut Vec<(Place, Fault)>) {
        if self.late.is_empty() {
            return;
        }
        faults.retain_mut(|(_, fault)| {
            let Fault::InString { start, objects, .. } = fault else {
                return true;
            };
            let Some(late) = self.late.iter().find(|late| late.start == *start) else {
                return true;
            };
            *objects = count(late.objects.len());
            !late.objects.is_empty()
        });
    }

    /// What [`Reader::members`] says, `fault`, in place of what it said as
    /// the file was opened: where faults are looked for.
    pub(super) fn said_members(&mut self, fault: Option<Fault>) {
        if !self.wants {
            return;
        }
        let said = (self.opened.iter()).position(|(_, f)| matches!(f, Fault::Members { .. }));
        match (said, fault) {
            (Some(i), Some(fault)) => self.opened[i].1 = fault,
            (Some(i), None) => drop(self.opened.remove(i)),
            (None, Some(fault)) => self.opened.push((Place::new(None, None), fault)),
            (None, None) => {}
        }
    }

    /// How object number `n`'s entry was found away from where its section
    /// put it, where it was ([`Reader::shifted`]): at a shift
    /// ([`Origin::Shifted`]), by a row renumbered ([`Origin::Renumbered`]),
    /// or by a scan ([`Origin::Recovered`]).
    pub(super) fn shift(&self, n: u32) -> Option<Origin> {
        let i = self.shifts.binary_search_by_key(&n, |&(n, _)| n).ok()?;
        Some(self.shifts[i].1)
    }

    /// Where the section said to be at `at` was read, as the reading read
    /// it: elsewhere where it didn't read there ([`Found`]).
    pub(super) fn found_at(&self, at: usize) -> usize {
        match self.found.binary_search_by_key(&at, |&(at, _)| at) {
            Ok(i) => self.found[i].1,
            Err(_) => at,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
