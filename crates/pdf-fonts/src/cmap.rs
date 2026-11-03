//! A font's CMaps, as what's written in them says: an embedded CMap's codespace,
//! how it splits a string into codes and each code's CID (ISO 32000-2, 9.7.5), and
//! a `/ToUnicode` CMap's text (9.10.3). Each is lexed as content, by the rules a
//! content stream is: PDFium has a word parser of its own for CMaps, so a crafted
//! one can split otherwise there.

use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

use pdf_content_streams::{Operand, Operation, ResCx, operations};
use pdf_objects::{ObjectId, Resolve};

use crate::predefined::{self, BfString, Collection};
use crate::reader::Reader;
use crate::{Error, Fault, Limits, Sink, Streams, glyph_text};

/// Values for ranges of codes or CIDs, kept as the ranges they're given in, so a
/// range of 2³¹ costs what a range of one does. Where ranges overlap, the first
/// given wins, as PDFium reads `/W`, or the last, as a CMap's later mappings, and
/// those of the CMap that uses another, win.
#[derive(Debug, Clone, Default)]
pub(crate) struct Ranges<T> {
    /// Disjoint, in order: first, last, value.
    pub(crate) pieces: Vec<(u32, u32, T)>,
}

impl<T: Copy> Ranges<T> {
    /// `ranges`, each its first, its last and its value; one whose last is before
    /// its first is none.
    pub(crate) fn new(ranges: &[(u32, u32, T)], last_wins: bool) -> Ranges<T> {
        let ranges: Vec<_> = ranges.iter().filter(|r| r.0 <= r.1).collect();
        let rank = |i: usize| if last_wins { ranges.len() - i } else { i };
        let mut starts: Vec<usize> = (0..ranges.len()).collect();
        starts.sort_by_key(|&i| ranges[i].0);
        let mut bounds: Vec<u64> = (ranges.iter())
            .flat_map(|r| [u64::from(r.0), u64::from(r.1) + 1])
            .collect();
        bounds.sort_unstable();
        bounds.dedup();
        // From each bound to the next, the ranges that cover it, the one that wins
        // on top; those that have ended are passed over.
        let mut covering = BinaryHeap::new();
        let mut next = starts.iter().peekable();
        let mut pieces: Vec<(u32, u32, T)> = Vec::new();
        let mut winners: Vec<usize> = Vec::new();
        for pair in bounds.windows(2) {
            let (at, end) = (pair[0], pair[1]);
            while let Some(&&i) = next.peek() {
                if u64::from(ranges[i].0) != at {
                    break;
                }
                covering.push(Reverse((rank(i), i)));
                next.next();
            }
            while let Some(&Reverse((_, i))) = covering.peek() {
                if u64::from(ranges[i].1) >= at {
                    break;
                }
                covering.pop();
            }
            let Some(&Reverse((_, i))) = covering.peek() else {
                continue;
            };
            let (lo, hi) = (at as u32, (end - 1) as u32);
            let joins = winners.last() == Some(&i)
                && pieces.last().is_some_and(|p| u64::from(p.1) + 1 == at);
            match pieces.last_mut() {
                Some(piece) if joins => piece.1 = hi,
                _ => {
                    pieces.push((lo, hi, ranges[i].2));
                    winners.push(i);
                }
            }
        }
        Ranges { pieces }
    }

    /// The value of the range `code` is in.
    pub(crate) fn get(&self, code: u32) -> Option<T> {
        let i = self.pieces.partition_point(|p| p.1 < code);
        let &(lo, _, value) = self.pieces.get(i)?;
        (lo <= code).then_some(value)
    }
}

/// An embedded CMap: its codespace, how it splits a string into codes, each code's
/// CID, and whether it writes vertically.
#[derive(Debug, Default)]
pub(crate) struct CidMap {
    codespace: Vec<(Vec<u8>, Vec<u8>)>,
    pub(crate) scheme: Scheme,
    /// Each range's first code and the CID it maps to, of those that end within 16 bits,
    /// which are the only ones that map a code within them, as PDFium's table holds only
    /// those (`CPDF_CMapParser::HandleCid`).
    pub(crate) cids: Ranges<(u32, u32)>,
    /// The others, by where they end, for the codes past 16 bits alone: first, last and
    /// the CID the first maps to. A code is in the first that ends at or past it, or in
    /// none, as PDFium looks it up (`CPDF_CMap::CIDFromCharCode`).
    beyond: Vec<(u32, u32, u32)>,
    /// The predefined CMap it uses but Identity, for the codes it maps none of.
    base: Option<&'static predefined::CMap>,
    pub(crate) vertical: bool,
}

/// How a CMap's codespace splits a string into codes, as PDFium has it
/// (`CPDF_CMapParser::HandleCodeSpaceRange`): by one range, a byte each, or two
/// where it's of two bytes; by more, each code by the ranges ([`mixed`]); by none,
/// two bytes each.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Scheme {
    OneByte,
    #[default]
    TwoBytes,
    Mixed,
}

/// A range's mapping of codes to CIDs, as they're read: its first code, its last,
/// and its first code and the CID that maps to.
type CidRange = (u32, u32, (u32, u32));

impl CidMap {
    /// The CMap stream `id`, over the CMaps it uses: the stream its `/UseCMap`
    /// names, followed while they're streams it hasn't read yet, at most
    /// [`Limits::cmaps`] of them, each decompressed within `budget`; and the
    /// predefined CMap one names, by name or by `usecmap`. That's the standard's
    /// reading (ISO 32000-2, 9.7.5.3); PDFium uses none of them, and gives a code past
    /// the stream's own ranges CID 0, so anything taken from them sets `unknown`, as
    /// does a predefined CMap the build hasn't, a stream of the chain that can't be
    /// read, or one past the cap.
    pub(crate) fn read<'o, O: Resolve<'o> + Streams, S: Sink + ?Sized>(
        reader: &Reader<'_, O, S>,
        id: ObjectId,
        unknown: &mut bool,
        budget: &mut usize,
    ) -> Result<CidMap, Error> {
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut next = Some(id);
        let mut base = None;
        let cut = |id: ObjectId, unknown: &mut bool| {
            reader.tell(Some(id), None, Fault::UseCMap);
            *unknown = true;
        };
        while let Some(id) = next.take() {
            // A chain back to a CMap it's read adds nothing.
            if !seen.insert(id) {
                reader.tell(Some(id), None, Fault::UseCMap);
                break;
            }
            let Some((id, stream)) = reader.stream(id) else {
                cut(id, unknown);
                break;
            };
            // A stream past the cap: the chain is ours to cut, not the file's
            // fault.
            if chain.len() >= reader.limits.cmaps {
                reader.tell(Some(id), None, Fault::TooManyCMaps);
                *unknown = true;
                break;
            }
            let Some(data) = reader.charged(id, budget)? else {
                cut(id, unknown);
                break;
            };
            chain.push((id, data));
            if let Some(uses) = stream.dict.get(b"UseCMap") {
                match (uses.as_reference(), uses.as_name()) {
                    (Some(id), _) => next = Some(id),
                    (_, Some(name)) => base = Some(name),
                    _ => {}
                }
            }
        }
        let mut map = CidMap::default();
        let mut ranges = Vec::new();
        if let Some(name) = base {
            let last = chain.last().map(|&(id, _)| id);
            map.uses(reader, last, name, &mut ranges, unknown);
        }
        for (id, data) in chain.iter().skip(1).rev() {
            map.parse(reader, *id, data, &mut ranges, unknown);
        }
        *unknown |= !ranges.is_empty() || !map.codespace.is_empty() || map.vertical;
        if let Some((id, data)) = chain.first() {
            map.parse(reader, *id, data, &mut ranges, unknown);
        }
        let (within, beyond): (Vec<_>, Vec<_>) = ranges.iter().partition(|r| r.1 <= 0xffff);
        map.cids = Ranges::new(&within, true);
        map.beyond = beyond
            .iter()
            .map(|&(lo, hi, (_, cid))| (lo, hi, cid))
            .collect();
        map.beyond.sort_by_key(|range| range.1);
        map.scheme = match map.codespace.len() {
            0 => Scheme::TwoBytes,
            1 if map.codespace[0].0.len() == 2 => Scheme::TwoBytes,
            1 => Scheme::OneByte,
            _ => Scheme::Mixed,
        };
        Ok(map)
    }

    /// The range that maps `code`, its first code and the CID that maps to: one within
    /// 16 bits for a code within them, the last given that covers it, and for a code past
    /// them the first of [`CidMap::beyond`] that ends at or past it.
    pub(crate) fn range_of(&self, code: u32) -> Option<(u32, u32)> {
        if code <= 0xffff {
            return self.cids.get(code);
        }
        let i = self.beyond.partition_point(|range| range.1 < code);
        let &(first, _, cid) = self.beyond.get(i)?;
        (first <= code).then_some((first, cid))
    }

    /// The CID its predefined base maps `code` to, by the code's length as its
    /// scheme sizes it (`CPDF_CMap::GetCharSize`).
    pub(crate) fn base_cid(&self, code: u32) -> Option<u32> {
        let len = match self.scheme {
            Scheme::OneByte => 1,
            Scheme::TwoBytes => 2,
            Scheme::Mixed => (4 - code.leading_zeros() / 8).max(1) as u8,
        };
        self.base?.lookup_cid_code(code, len)
    }

    /// The code `rest` starts with, by its scheme, and how many bytes it takes of
    /// it.
    pub(crate) fn next(&self, rest: &[u8]) -> (usize, u32) {
        match self.scheme {
            Scheme::OneByte => (1, u32::from(rest[0])),
            Scheme::TwoBytes => two_bytes(rest),
            Scheme::Mixed => mixed(&self.codespace, rest),
        }
    }

    /// The mappings of `data`, the CMap stream `id`, onto `ranges`: its codespace ranges,
    /// `cidrange`s and `cidchar`s, its `/WMode`, and the predefined CMap its
    /// `usecmap` names; read as far as it lexes.
    fn parse<'o, O: Resolve<'o> + Streams, S: Sink + ?Sized>(
        &mut self,
        reader: &Reader<'_, O, S>,
        id: ObjectId,
        data: &[u8],
        ranges: &mut Vec<CidRange>,
        unknown: &mut bool,
    ) {
        let limits = reader.limits;
        let cid = |operand: &Operand| {
            let cid = operand.object(&limits.content)?.as_integer()?;
            u32::try_from(cid).ok()
        };
        let none = ResCx::none();
        for operation in operations(data, &none, &limits.content) {
            let Ok(operation) = operation else {
                break;
            };
            match operation.name() {
                b"endcodespacerange" => self.codespace.extend(codespace(&operation, limits)),
                b"endcidrange" => {
                    let args: Vec<_> = operation.operands().collect();
                    for range in args.chunks_exact(3) {
                        let read = code(&range[0], limits)
                            .zip(code(&range[1], limits))
                            .zip(cid(&range[2]));
                        if let Some(((lo, hi), first)) = read {
                            ranges.push((lo, hi, (lo, first)));
                        }
                    }
                }
                b"endcidchar" => {
                    let args: Vec<_> = operation.operands().collect();
                    for pair in args.chunks_exact(2) {
                        if let Some((c, to)) = code(&pair[0], limits).zip(cid(&pair[1])) {
                            ranges.push((c, c, (c, to)));
                        }
                    }
                }
                b"def" if operation_name(&operation, 1).as_deref() == Some(&b"WMode"[..]) => {
                    let mode = operation.arg(0).and_then(|m| m.object(&limits.content));
                    let mode = mode.and_then(|m| m.as_integer());
                    self.vertical = mode.is_some_and(|m| m != 0);
                }
                b"usecmap" => {
                    if let Some(name) = operation_name(&operation, 0) {
                        self.uses(reader, Some(id), &name, ranges, unknown);
                    }
                }
                _ => {}
            }
        }
    }

    /// The predefined CMap `name` used by the stream `by`, under what's mapped after it:
    /// Identity's codespace and CIDs; another's CIDs, where the build has it, for the codes
    /// nothing else maps. PDFium maps those to CID 0, so either is guessed (`unknown`),
    /// as one the build hasn't is.
    fn uses<'o, O: Resolve<'o> + Streams, S: Sink + ?Sized>(
        &mut self,
        reader: &Reader<'_, O, S>,
        by: Option<ObjectId>,
        name: &[u8],
        ranges: &mut Vec<CidRange>,
        unknown: &mut bool,
    ) {
        if matches!(name, b"Identity-H" | b"Identity-V") {
            self.codespace.push((vec![0, 0], vec![0xff, 0xff]));
            ranges.push((0, 0xffff, (0, 0)));
        } else {
            let bundled = predefined::bundled(name);
            if bundled.is_none() {
                reader.tell(by, None, Fault::PredefinedUnknown);
            }
            self.base = bundled.or(self.base);
        }
        *unknown = true;
    }
}

/// The name operand `i` from the last is, decoded.
fn operation_name<'a>(operation: &Operation<'a>, i: usize) -> Option<Cow<'a, [u8]>> {
    operation.arg(i)?.name()
}

/// The code a string operand of 1 to 4 bytes is.
fn code(operand: &Operand, limits: &Limits) -> Option<u32> {
    let string = operand.object(&limits.content)?;
    let string = string.as_bytes()?;
    (1..=4).contains(&string.len()).then(|| number(string))
}

/// The code of two bytes `rest` starts with, a 0 for the second where it hasn't
/// one, as PDFium reads one (`CPDF_CMap::GetNextChar`), and how many bytes it has
/// of it.
pub(crate) fn two_bytes(rest: &[u8]) -> (usize, u32) {
    let second = rest.get(1).copied().unwrap_or(0);
    (
        rest.len().min(2),
        u32::from(rest[0]) << 8 | u32::from(second),
    )
}

/// A `codespacerange`'s ranges, from its operands: pairs of strings of one length,
/// 1 to 4 bytes.
fn codespace(operation: &Operation, limits: &Limits) -> Vec<(Vec<u8>, Vec<u8>)> {
    let string = |operand: &Operand| {
        let string = operand.object(&limits.content)?;
        Some(string.as_bytes()?.to_vec())
    };
    let args: Vec<_> = operation.operands().collect();
    (args.chunks_exact(2))
        .filter_map(|pair| {
            let (lo, hi) = (string(&pair[0])?, string(&pair[1])?);
            (lo.len() == hi.len() && (1..=4).contains(&lo.len())).then_some((lo, hi))
        })
        .collect()
}

/// The code `rest` starts with by `space`'s ranges, as PDFium reads a CMap of more
/// than one (`GetNextChar`'s mixed rule): its bytes taken a byte at a time while a
/// range of more bytes holds what's taken so far (`CheckFourByteCodeRange`); 0,
/// the bytes read taken, where none does, or where the code would run past four
/// bytes or the end. How many bytes it takes.
fn mixed(space: &[(Vec<u8>, Vec<u8>)], rest: &[u8]) -> (usize, u32) {
    let mut size = 1;
    loop {
        let codes = &rest[..size];
        match code_range(space, codes) {
            Held::Not => return (size, 0),
            Held::Whole => return (size, number(codes)),
            Held::Begun if size == 4 || size == rest.len() => return (size, 0),
            Held::Begun => size += 1,
        }
    }
}

/// What PDFium's `CheckFourByteCodeRange` says of `codes`.
enum Held {
    /// No range begins with its first byte.
    Not,
    /// It's a code.
    Whole,
    /// It begins one: it takes another byte.
    Begun,
}

/// Whether `space` holds `codes`, as PDFium checks (`CheckFourByteCodeRange`): by
/// the last range of as many bytes or more whose first byte's bounds hold its
/// first, a code where it's as long as that range, or as long as the bytes of it
/// within its bounds; else begun.
fn code_range(space: &[(Vec<u8>, Vec<u8>)], codes: &[u8]) -> Held {
    for (lo, hi) in space.iter().rev().filter(|(lo, _)| lo.len() >= codes.len()) {
        let within = (codes.iter().zip(lo.iter().zip(hi)))
            .take_while(|(c, (lo, hi))| lo <= c && c <= hi)
            .count();
        if within == lo.len() || within > 0 && codes.len() == lo.len() {
            return Held::Whole;
        }
        if within > 0 {
            return Held::Begun;
        }
    }
    Held::Not
}

/// How many bytes the code `rest` starts with takes, by `space`'s ranges: the one
/// it's in, or the shortest one's. None where there are none.
fn codespace_len(space: &[(Vec<u8>, Vec<u8>)], rest: &[u8]) -> Option<usize> {
    let within = |(lo, hi): &(Vec<u8>, Vec<u8>)| {
        rest.len() >= lo.len() && (0..lo.len()).all(|i| lo[i] <= rest[i] && rest[i] <= hi[i])
    };
    space
        .iter()
        .find(|r| within(r))
        .or_else(|| space.iter().min_by_key(|(lo, _)| lo.len()))
        .map(|(lo, _)| lo.len())
}

/// Bytes as a big-endian number: a code.
pub(crate) fn number(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0u32, |c, &b| c << 8 | u32::from(b))
}

/// A `/ToUnicode` CMap, its mappings read as PDFium's `CPDF_ToUnicodeMap` takes them:
/// a code at most 16 bits, a later mapping over an earlier, a block whose count isn't
/// the mappings it holds, or that holds what's no mapping, taken not at all.
#[derive(Debug, Default)]
pub(crate) struct CMap {
    /// Codespace ranges, each its first and last code, both as long as the codes
    /// in it.
    codespace: Vec<(Vec<u8>, Vec<u8>)>,
    /// What codes are mapped to, by range: the range's first code, and where its
    /// destination is in `units` and how long.
    mapped: Ranges<(u32, u32, u32)>,
    /// The destinations' UTF-16 units, one after another.
    units: Vec<u16>,
    /// The collection whose CIDs a code none of these maps is read as, where the CMap
    /// names one's Adobe-*-UCS2.
    base: Option<Collection>,
}

/// Mappings as they're given, the first code, the last, and [`CMap::mapped`]'s value.
type Given = Vec<(u32, u32, (u32, u32, u32))>;

impl CMap {
    /// The mappings of `bytes`, a CMap; read as far as it lexes.
    pub(crate) fn parse(bytes: &[u8], limits: &Limits) -> CMap {
        let mut cmap = CMap::default();
        let mut given = Given::new();
        let mut begun = None;
        let none = ResCx::none();
        for operation in operations(bytes, &none, &limits.content) {
            let Ok(operation) = operation else {
                break;
            };
            // A block's count is what's before its `begin`, its mappings what's before
            // its `end`, with nothing between them.
            let count = begun.take();
            match operation.name() {
                b"endcodespacerange" => cmap.codespace.extend(codespace(&operation, limits)),
                b"beginbfchar" | b"beginbfrange" => begun = Some(block_count(&operation, limits)),
                b"endbfchar" => cmap.bfchars(&operation, count, &mut given, limits),
                b"endbfrange" => cmap.bfranges(&operation, count, &mut given, limits),
                _ => cmap.names_collection(&operation),
            }
        }
        cmap.mapped = Ranges::new(&given, true);
        cmap
    }

    /// A `bfchar` block's mappings, where it holds `count` of them and each code is
    /// one. A destination of no text maps nothing.
    fn bfchars(
        &mut self,
        block: &Operation,
        count: Option<usize>,
        given: &mut Given,
        limits: &Limits,
    ) {
        let args: Vec<_> = block.operands().collect();
        if count.is_none_or(|count| count.checked_mul(2) != Some(args.len())) {
            return;
        }
        let codes: Option<Vec<u32>> = (args.chunks_exact(2))
            .map(|pair| source_code(&pair[0], limits).filter(|&c| c <= LAST_CODE))
            .collect();
        for (code, pair) in codes.into_iter().flatten().zip(args.chunks_exact(2)) {
            let units = destination(&pair[1], limits);
            self.map_each(code, units.as_deref(), given);
        }
    }

    /// A `bfrange` block's mappings, where it holds `count` of them and each is one.
    /// PDFium's last code of a range is its first's but for the low byte, and an array
    /// has a destination for each of those. Here a range is read to its last code where
    /// that's past it, on purpose: writers map `<0000> <FFFF> <0000>`, where PDFium's text
    /// is the same, by a code's own, and subsets' ranges cross a byte, whose blocks it
    /// loses. An array shorter than that is read by the low byte, as PDFium's.
    fn bfranges(
        &mut self,
        block: &Operation,
        count: Option<usize>,
        given: &mut Given,
        limits: &Limits,
    ) {
        let args: Vec<_> = block.operands().collect();
        if count.is_none_or(|count| count.checked_mul(3) != Some(args.len())) {
            return;
        }
        let mut ranges = Vec::with_capacity(args.len() / 3);
        for range in args.chunks_exact(3) {
            let (Some(lo), Some(last)) = (
                source_code(&range[0], limits),
                source_code(&range[1], limits),
            ) else {
                return;
            };
            let to = range[2].object(&limits.content);
            let items = to.as_ref().and_then(|to| to.as_array());
            let whole = (lo..=LAST_CODE).contains(&last)
                && items.is_none_or(|items| items.len() as u32 == last - lo + 1);
            let hi = if whole {
                last
            } else {
                lo & !0xff | last & 0xff
            };
            if lo > LAST_CODE || hi > LAST_CODE || lo > hi {
                return;
            }
            if items.is_some_and(|items| items.len() as u32 != hi - lo + 1) {
                return;
            }
            ranges.push((lo, hi, to));
        }
        for (lo, hi, to) in ranges {
            let to = to.as_ref();
            if let Some(items) = to.and_then(|to| to.as_array()) {
                for (code, item) in (lo..=hi).zip(items) {
                    let units = item.as_bytes().map(utf16_units);
                    self.map_each(code, units.as_deref(), given);
                }
            } else {
                // A destination of no text maps its range to no text.
                let units = to.and_then(|to| to.as_bytes()).map(utf16_units);
                let at = self.keep(units.as_deref().unwrap_or_default());
                given.push((lo, hi, (lo, at.0, at.1)));
            }
        }
    }

    /// `code` mapped to `units`, unless there's no text in them.
    fn map_each(&mut self, code: u32, units: Option<&[u16]>, given: &mut Given) {
        if let Some(units) = units.filter(|units| !units.is_empty()) {
            let at = self.keep(units);
            given.push((code, code, (code, at.0, at.1)));
        }
    }

    /// `units` kept, where they are and how many.
    fn keep(&mut self, units: &[u16]) -> (u32, u32) {
        let at = self.units.len() as u32;
        self.units.extend_from_slice(units);
        (at, units.len() as u32)
    }

    /// The collection one of `operation`'s operands names, by the name of its
    /// Adobe-*-UCS2, wherever that's said, as PDFium takes it.
    fn names_collection(&mut self, operation: &Operation) {
        for operand in operation.operands() {
            let collection = operand.name().and_then(|name| match &name[..] {
                b"Adobe-GB1-UCS2" => Some(Collection::Gb1),
                b"Adobe-CNS1-UCS2" => Some(Collection::Cns1),
                b"Adobe-Japan1-UCS2" => Some(Collection::Japan1),
                b"Adobe-Korea1-UCS2" => Some(Collection::Korea1),
                _ => None,
            });
            self.base = collection.or(self.base);
        }
    }

    /// The text `code` maps to, by the last mapping of it, a range's with its last
    /// unit counted up. By PDFium's rule, where units are kept apart: one unit alone
    /// is 16 bits, U+FFFF of it no text; a lone surrogate, which `str` can't hold, is
    /// U+FFFD. Failing a mapping, its text as a CID of the collection the CMap names.
    pub(crate) fn get(&self, code: u32) -> Option<Cow<'_, str>> {
        let Some((first, at, len)) = self.mapped.get(code) else {
            let cid = code & 0xffff;
            return match self.base?.to_unicode()?.lookup_bf_string(cid)? {
                BfString::Char(c) => Some(Cow::Owned(String::from(c))),
                BfString::String(s) => Some(Cow::Owned(s)),
            };
        };
        let units = &self.units[at as usize..(at + len) as usize];
        let (&last, before) = units.split_last()?;
        let last = u32::from(last) + (code - first);
        if before.is_empty() {
            let unit = last & 0xffff;
            return (unit != 0xffff).then(|| {
                let c = char::from_u32(unit).unwrap_or(char::REPLACEMENT_CHARACTER);
                Cow::Owned(String::from(c))
            });
        }
        let Ok(last) = u16::try_from(last) else {
            let mut text = decoded(before.iter().copied());
            text.extend(char::from_u32(last));
            return Some(Cow::Owned(text));
        };
        Some(Cow::Owned(decoded(before.iter().copied().chain([last]))))
    }

    /// Whether its codespace says how long a code is.
    pub(crate) fn has_codespace(&self) -> bool {
        !self.codespace.is_empty()
    }

    /// How many bytes the code `rest` starts with takes, by its codespace.
    pub(crate) fn code_len(&self, rest: &[u8]) -> Option<usize> {
        codespace_len(&self.codespace, rest)
    }
}

/// The last code a `/ToUnicode` CMap maps (PDFium's `kCidLimit`).
const LAST_CODE: u32 = 0xffff;

/// The count a `beginbfchar` or `beginbfrange` says of its block; what's no count, 0.
fn block_count(begin: &Operation, limits: &Limits) -> usize {
    (begin.arg(0))
        .and_then(|count| count.object(&limits.content)?.as_integer())
        .and_then(|count| usize::try_from(count).ok())
        .unwrap_or(0)
}

/// The code a string operand of a `/ToUnicode` CMap is, of any length its value fits
/// 32 bits in; a string of nothing is none.
fn source_code(operand: &Operand, limits: &Limits) -> Option<u32> {
    let string = operand.object(&limits.content)?;
    let string = string.as_bytes().filter(|string| !string.is_empty())?;
    string
        .iter()
        .try_fold(0u32, |c, &b| c.checked_mul(256)?.checked_add(u32::from(b)))
}

/// A `bfchar` destination's UTF-16 units: a string's, or a glyph name's text's.
fn destination(operand: &Operand, limits: &Limits) -> Option<Vec<u16>> {
    let destination = operand.object(&limits.content)?;
    if let Some(s) = destination.as_bytes() {
        return Some(utf16_units(s));
    }
    let name = std::str::from_utf8(destination.as_name()?).ok()?;
    Some(glyph_text(name, false)?.encode_utf16().collect())
}

/// A CMap destination string's units: UTF-16BE, or one byte's code.
fn utf16_units(s: &[u8]) -> Vec<u16> {
    if let [b] = s {
        return vec![u16::from(*b)];
    }
    s.chunks_exact(2)
        .map(|u| u16::from_be_bytes([u[0], u[1]]))
        .collect()
}

/// UTF-16 units' text, a lone surrogate U+FFFD.
fn decoded(units: impl Iterator<Item = u16>) -> String {
    char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}
