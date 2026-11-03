//! Fonts, read through pdf-fonts (ISO 32000-2, 9.5 to 9.10): each the walk
//! reached in the newest revision as one of the model's font tables, in the
//! order it reached them, read again by [`Font::read_for_text`], as fonts
//! made for text are, no program read, under pdf-fonts' caps. Its objects
//! are the file's as the predicates resolve them ([`Objects`]), and its
//! streams' data, a `/ToUnicode`, an embedded CMap and those it uses, a
//! Type 3 font's glyphs, comes decoded from the source
//! ([`Source::decoded_within`]), as a font reads a reading's ([`decoded`]),
//! the file's fonts' within one budget. A source that gives no stream's
//! data, as a document alone doesn't, leaves a font that asks one unread,
//! as it leaves content. What the read tells is kept ([`Kept`]), each read
//! run to its end; but once an object it reads doesn't read for what isn't
//! the file's fault, a password or a cap, the read goes on as if it weren't
//! there, and what it tells after isn't kept ([`Cut`]).
//!
//! Of what it tells, what the walk can't say is a [`Rule::Font`] finding,
//! where the walk found nothing by the model's rules in that object at that
//! key, nor at a reference to the object it's of as a whole
//! ([`Walk::find_told`]): each kind once in each object, whichever font's
//! read tells it, at its first place there, its value pdf-fonts' words
//! with how many times; in the fault's object, a descriptor, a CIDFont, an
//! encoding, a CMap, else the font's, else the one the font is written in,
//! held where that is. Its key is the font dictionary's key the kind is
//! about, where it's one ([`kind_key`]), as the model's `missing` is at the
//! key, and any other at the entry or element whose value holds its bytes
//! ([`holding`]). Its path and table are the object's where the walk first
//! visited it as a font table or one of what a font holds ([`PARTS`]),
//! else the font's. Never told: an object that doesn't read, the reader's
//! and the model's `unresolved`, nor what's told of a value that refers to
//! one, or to a reference, which PDFium follows no further where ISO
//! 32000-2 does ([`Walk::of_the_unread`]); a `/UseCMap` to what isn't a
//! stream the file has, which the walk says at the `/UseCMap`; a predefined
//! CMap the reading hasn't, a name no CMap has being the model's `value`;
//! and a program's fault, as none is read. Nor is a font read whose
//! `/Subtype` names no kind of font as ISO 32000-2 and pdf-fonts both read
//! it, which pdf-fonts would read as a Type 1 font, a guess
//! ([`of_a_kind`]). What stops a font being read whole that isn't the
//! file's fault (an object that takes a password or is past a cap, streams
//! past the budget, a CMap chain longer than is read) is counted as not
//! checked, once a font for each, where the font, or what it's written in,
//! is held; what was read before it is found all the same.
//!
//! Before the walk, the fonts the model's `fn:FontHasLatinChars()` asks of
//! are read ([`latin`]): what a font descriptor's `/CapHeight` is required
//! by (ISO 32000-2, 9.8.1, Table 120) only a reader of its fonts can say,
//! and the predicates are given it ([`FileCx::latin`]) before any row is
//! evaluated. What's read then is the pass's after the walk, each font read
//! once.
//!
//! [`Source::decoded_within`]: crate::source::Source::decoded_within
//! [`FileCx::latin`]: crate::FileCx::latin

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::mem::{self, Discriminant};
use std::ops::RangeInclusive;
use std::ptr;

use pdf_fonts::{Decoded, Error, Fault, Font, Limits, With};
use unicode_script::{Script, UnicodeScript};

use super::content::times;
use super::kept::Kept;
use super::told::{Told, holding, written_at};
use super::{Finding, Graphic, Made, NESTING, Objects, REFERENCES, Rule, Walk};
use crate::bytes::{self, Unfiltered};
use crate::object::{Dict, Object, ObjectId, Span, Value};
use crate::pdf_filters;
use crate::source::Source;
use crate::{Latin, Resolve, Unreadable, Why};

/// The model's font tables: what the walk reaches as one is a font, read
/// again after it. A Type 0 font's CIDFont, descriptor and CMap are read as
/// its parts, and so are a simple font's descriptor and encoding.
pub(super) const FONTS: [&str; 5] = [
    "FontType1",
    "FontTrueType",
    "FontMultipleMaster",
    "FontType3",
    "FontType0",
];

/// The model's tables of what a font holds: a descriptor of each kind, a
/// Type 0 font's descendants, its CIDFont, the CIDFont's widths and its
/// CMap, a simple font's encoding, its differences and its widths, and a
/// `/ToUnicode`. Where the walk first visits an object as one of them, or as
/// a font, is the path and table of what's found in it.
pub(super) const PARTS: [&str; 15] = [
    "FontDescriptorType1",
    "FontDescriptorTrueType",
    "FontDescriptorType3",
    "FontDescriptorCIDType0",
    "FontDescriptorCIDType2",
    "ArrayOfDescendantFonts",
    "FontCIDType0",
    "FontCIDType2",
    "ArrayOfCIDGlyphMetricsW",
    "ArrayOfCIDGlyphMetricsW2",
    "CMapStream",
    "Encoding",
    "ArrayOfDifferences",
    "ArrayOfNumbersGeneral",
    "ToUnicodeCMapStream",
];

/// The kinds of font a font dictionary's `/Subtype` names (ISO 32000-2,
/// 9.5).
const KINDS: [&[u8]; 5] = [b"Type1", b"MMType1", b"TrueType", b"Type3", b"Type0"];

/// What fonts are read under: pdf-fonts' caps, which ISO 32000-2 doesn't
/// set.
const LIMITS: Limits = Limits::DEFAULT;

/// Why a font isn't read whole, other than for the file's faults: an object
/// of it takes a password the reader wasn't given.
const LOCKED: &str = "a font with an object that takes a password";

/// Why a font isn't read whole: an object of it is past a cap the reader
/// reads under.
const CAPPED: &str = "a font with an object past a cap it's read under";

/// Why a font isn't read whole: the source didn't give an object's bytes.
const UNGIVEN: &str = "a font with an object whose bytes the source didn't give";

/// Why a font isn't read whole: an object of it doesn't read, for another
/// reason a reader gives.
const UNREAD: &str = "a font with an object that doesn't read";

/// Why a font isn't read whole: its streams decode past what's left of the
/// file's budget for fonts' streams ([`FONT_BUDGET`]).
///
/// [`FONT_BUDGET`]: super::FONT_BUDGET
const BUDGET: &str = "a font whose streams decode past the file's budget for fonts' streams";

/// Why a font isn't read whole: a CMap's `/UseCMap` chain is longer than
/// pdf-fonts reads ([`Limits::cmaps`]).
const CMAPS: &str = "a font with a CMap chain longer than is read";

/// Why a font isn't read whole, given what stopped its read.
fn unread(error: &Error) -> &'static str {
    match error {
        Error::TooLarge => BUDGET,
        Error::Unreadable(unreadable) => match unreadable.why {
            Why::Locked => LOCKED,
            Why::Limit => CAPPED,
            Why::Source => UNGIVEN,
            _ => UNREAD,
        },
        _ => UNREAD,
    }
}

/// The key of a font dictionary a kind of fault is about, where it's about
/// one, which it's told at whether or not the key is written: a fault of a
/// key absent is told at no bytes, where the model's `missing` is at the
/// key. None for the rest, told at the bytes they're of ([`holding`]).
fn kind_key(fault: &Fault) -> Option<&'static str> {
    Some(match fault {
        Fault::NoWidths | Fault::WidthsLength => "Widths",
        Fault::FirstChar => "FirstChar",
        Fault::Subtype => "Subtype",
        Fault::ToUnicode => "ToUnicode",
        Fault::DescendantFonts => "DescendantFonts",
        Fault::CMap => "Encoding",
        _ => return None,
    })
}

/// The name `font`'s `/Subtype` is, as ISO 32000-2 and pdf-fonts both read
/// it: written in place, or through one reference. None where it's absent
/// or no name, a string among them, which pdf-fonts reads as the name it
/// spells, as PDFium does, or a name through a reference to a reference,
/// which ISO 32000-2 follows to its end (7.3.10) and pdf-fonts no further
/// than the first; `Err` where the reference leads to an object that
/// doesn't read for what isn't the file's fault.
fn subtype<'a>(source: &'a dyn Source, font: &'a Dict) -> Result<Option<&'a [u8]>, Unreadable> {
    let Some(written) = font.get(b"Subtype") else {
        return Ok(None);
    };
    let named = match written.as_reference() {
        Some(id) => match Objects(source).try_resolve(id) {
            Err(unreadable) if unreadable.why != Why::Damaged => return Err(unreadable),
            resolved => resolved.ok().flatten(),
        },
        None => Some(written),
    };
    Ok(named.and_then(Object::as_name))
}

/// Whether `font` is read: where its `/Subtype` names a kind of font
/// ([`KINDS`]) as ISO 32000-2 and pdf-fonts both read it ([`subtype`]).
/// pdf-fonts reads a font whose `/Subtype` is absent or no font's as a
/// Type 1 font, as PDFium does, and so one whose `/Subtype` is a reference
/// to a reference; one whose `/Subtype` is a string it reads as the kind
/// the string spells. What it tells of either is of a kind it guessed, not
/// of the font the walk took it for, whose `/Subtype` the walk says: of a
/// Type 0 font, its `/Widths` missing and its CMap no encoding. Where the
/// reference leads to an object that doesn't read for what isn't the
/// file's fault, the font is read, which goes on past it, and is counted
/// ([`Cut`]).
fn of_a_kind(source: &dyn Source, font: &Dict) -> bool {
    match subtype(source, font) {
        Ok(kind) => kind.is_some_and(|kind| KINDS.contains(&kind)),
        Err(_) => true,
    }
}

/// The file's objects as [`Objects`] gives them, `failed` set where one
/// doesn't read for what isn't the file's fault, which a font's read goes
/// on past as if it weren't there ([`Error::Unreadable`]).
#[derive(Clone, Copy)]
struct Noting<'a, 'c> {
    objects: Objects<'a>,
    failed: &'c Cell<bool>,
}

impl<'a> Resolve<'a> for Noting<'a, '_> {
    fn resolve(&self, id: ObjectId) -> Option<&'a Object> {
        self.try_resolve(id).ok().flatten()
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'a Object>, Unreadable> {
        let resolved = self.objects.try_resolve(id);
        if let Err(unreadable) = &resolved
            && unreadable.why != Why::Damaged
        {
            self.failed.set(true);
        }
        resolved
    }
}

/// What a font's read tells, kept ([`Kept`]), until an object it reads
/// doesn't read for what isn't the file's fault (`failed`): its read goes on
/// as if the object weren't there, so what it tells after, a `/ToUnicode`
/// that takes a password told as one that doesn't decode, is of a font
/// without it, and isn't kept.
struct Cut<'c> {
    kept: &'c mut Kept<Fault>,
    failed: &'c Cell<bool>,
}

impl pdf_fonts::Sink for Cut<'_> {
    fn fault(&mut self, at: pdf_fonts::Place, fault: Fault) {
        if !self.failed.get() {
            self.kept.fault(at, fault);
        }
    }
}

/// Stream `id`'s data, at most `limit` bytes of it, as `source` gives it
/// ([`Source::decoded_within`]), said as pdf-fonts' [`Streams`] for a
/// pdf-structure `Reading` says a reading's: a stream that decodes in part
/// is what decoded, as PDFium takes it, unless what stopped it is the
/// limit, which is [`Decoded::TooLarge`]; what takes a password, a source
/// that failed and filters past the reader's cap, [`Decoded::Unreadable`],
/// as the predicates' resolver says of an object that doesn't read
/// ([`Objects`]); what isn't there or isn't a stream, what doesn't read,
/// and filters the reader doesn't take, [`Decoded::None`]. A source that
/// gives no stream's data, as a document alone doesn't, gives none, which
/// `ungiven` notes, so that what's read without it isn't taken for the
/// file's.
///
/// [`Source::decoded_within`]: crate::source::Source::decoded_within
/// [`Streams`]: pdf_fonts::Streams
pub(super) fn decoded(
    source: &dyn Source,
    id: ObjectId,
    limit: usize,
    ungiven: &Cell<bool>,
) -> Decoded {
    match Objects(source).try_resolve(id) {
        Ok(Some(_)) => {}
        Ok(None) => return Decoded::None,
        Err(unreadable) if unreadable.why == Why::Damaged => return Decoded::None,
        Err(unreadable) => return Decoded::Unreadable(unreadable),
    }
    let unreadable = |why: Why, error: &bytes::Error| {
        let message = format!("{} {} R doesn't read: {error}", id.0, id.1);
        Decoded::Unreadable(Unreadable::new(id, why, message))
    };
    let Some(decoded) = source.decoded_within(id, limit) else {
        ungiven.set(true);
        return Decoded::None;
    };
    match decoded {
        Ok(data) => Decoded::Data(data),
        Err(bytes::Error::Decode(error)) => {
            let limited = (error.faults.iter()).any(|(_, f)| *f == pdf_filters::Fault::Limit);
            match (limited, error.data.is_empty()) {
                (true, _) => Decoded::TooLarge,
                (false, true) => Decoded::None,
                (false, false) => Decoded::Data(error.data),
            }
        }
        Err(error @ bytes::Error::Io(_)) => unreadable(Why::Source, &error),
        Err(
            error @ (bytes::Error::Locked
            | bytes::Error::Encrypted
            | bytes::Error::Unsupported
            | bytes::Error::Decrypt(_)),
        ) => unreadable(Why::Locked, &error),
        Err(error @ bytes::Error::Filters(Unfiltered::Filters { .. })) => {
            unreadable(Why::Limit, &error)
        }
        Err(_) => Decoded::None,
    }
}

/// `font`, a font dictionary of `source`'s, read for text within what's
/// left of `budget`, what it tells kept until an object it reads doesn't
/// read for what isn't the file's fault ([`Cut`]); none where it asked a
/// stream's data of a source that gives none ([`decoded`]), as what it read
/// then isn't the file's font.
fn read<'a>(
    source: &'a dyn Source,
    font: &'a Dict,
    budget: &mut usize,
) -> Option<(Result<Font, Error>, Kept<Fault>)> {
    let (ungiven, failed) = (Cell::new(false), Cell::new(false));
    let objects = Noting {
        objects: Objects(source),
        failed: &failed,
    };
    let streams = With::new(objects, |id, limit| {
        let decoded = decoded(source, id, limit, &ungiven);
        if let Decoded::Unreadable(_) = decoded {
            failed.set(true);
        }
        decoded
    });
    let mut kept = Kept::default();
    let mut cut = Cut {
        kept: &mut kept,
        failed: &failed,
    };
    let read = Font::read_for_text(streams, font, &LIMITS, budget, &mut cut);
    (!ungiven.get()).then_some((read, kept))
}

/// A font as it's kept once read: by its number, where it's an indirect
/// object, else by where it's kept, which the walk reaches it as too, the
/// source keeping each object it gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Key {
    /// The indirect object it is.
    Id(ObjectId),
    /// The dictionary, written in place.
    Kept(*const Object),
}

/// The fonts [`latin`] read, each by its [`Key`], with what its read told,
/// for the pass after the walk; none where it asked a stream's data of a
/// source that gives none.
pub(super) type Read = HashMap<Key, Option<(Result<Font, Error>, Kept<Fault>)>>;

/// Which font descriptors' fonts have Latin characters, as the model's
/// `fn:FontHasLatinChars()` reads it, of each descriptor it's asked of,
/// one without `/CapHeight` (ISO 32000-2, 9.8.1, Table 120); and what was
/// read for it ([`Read`]). Every object `source` lists is looked in, and
/// each dictionary written in one, no deeper than [`NESTING`]: each
/// TrueType font, and each Type 0 font, by its first descendant's
/// descriptor; not a Type 3 font's, which Table 120 doesn't ask, nor a Type
/// 1 font's, which the model doesn't. Each is read once, by
/// [`Font::read_for_text`], within what's left of `budget`. Its font has
/// Latin characters where a code it draws has a letter of the Latin script
/// in its text ([`latin_in`]), and hasn't where a code it draws has text
/// and none has one; nothing is said of it where no code has text, or the
/// read fails, so the function can't be told. A descriptor two fonts name
/// has them where either has.
pub(super) fn latin(source: &dyn Source, budget: &mut usize) -> (Latin, Read) {
    let (mut latin, mut read) = (Latin::default(), Read::new());
    for id in source.ids() {
        let Ok(object) = source.get(id) else {
            continue;
        };
        let mut stack = vec![(object, 0)];
        while let Some((o, depth)) = stack.pop() {
            let inside = match o.value() {
                Value::Dict(d) => {
                    if let Some((descriptor, descriptor_id, composite)) = asked(source, d) {
                        let key = match depth {
                            0 => Key::Id(id),
                            _ => Key::Kept(ptr::from_ref(o)),
                        };
                        let font =
                            (read.entry(key)).or_insert_with(|| self::read(source, d, budget));
                        if let Some((Ok(font), _)) = font
                            && let Some(has) = latin_in(source, d, font, composite)
                        {
                            latin.insert(descriptor, descriptor_id, has);
                        }
                    }
                    d
                }
                Value::Stream(s) => &s.dict,
                Value::Array(a) if depth < NESTING => {
                    stack.extend(a.iter().rev().map(|e| (e, depth + 1)));
                    continue;
                }
                _ => continue,
            };
            if depth < NESTING {
                stack.extend(inside.iter().rev().map(|(_, v)| (v, depth + 1)));
            }
        }
    }
    (latin, read)
}

/// The descriptor `font` is asked `fn:FontHasLatinChars()` of, where it's
/// a font whose descriptor is, and lacks `/CapHeight`, absent or null: a
/// TrueType font's own, or a Type 0 font's first descendant's, its kind
/// by its `/Subtype` as ISO 32000-2 and pdf-fonts both read it
/// ([`subtype`]); with the number its references end at, where it's an
/// indirect object, and whether the font is a Type 0 one.
fn asked<'a>(
    source: &'a dyn Source,
    font: &'a Dict,
) -> Option<(&'a Object, Option<ObjectId>, bool)> {
    let (holder, composite) = match subtype(source, font).ok().flatten()? {
        b"TrueType" => (font, false),
        b"Type0" => {
            let (_, descendants) = through(source, font.get(b"DescendantFonts")?)?;
            let (_, first) = through(source, descendants.as_array()?.first()?)?;
            (first.as_dict()?, true)
        }
        _ => return None,
    };
    let (id, descriptor) = through(source, holder.get(b"FontDescriptor")?)?;
    let cap_height = descriptor.as_dict()?.get(b"CapHeight");
    match cap_height.and_then(|value| through(source, value)) {
        Some(_) => None,
        None => Some((descriptor, id, composite)),
    }
}

/// `value` through its references, at most [`REFERENCES`]: the number of
/// the last, none where it's written in place, and what it comes to; none
/// for null, a reference to what the file hasn't, which is null (ISO
/// 32000-2, 7.3.10), or one past the last.
fn through<'a>(
    source: &'a dyn Source,
    value: &'a Object,
) -> Option<(Option<ObjectId>, &'a Object)> {
    let (mut id, mut at) = (None, value);
    for _ in 0..REFERENCES {
        match at.value() {
            Value::Ref(to) => {
                at = source.get(*to).ok()?;
                id = Some(*to);
            }
            Value::Null => return None,
            _ => return Some((id, at)),
        }
    }
    None
}

/// Whether `read`, the font `font` reads as, has Latin characters: a letter
/// of the Latin script, by Unicode's Script property, in the text of a code
/// it draws ([`Font::code_text`]); none where no code it draws has text. A
/// simple font draws the codes from `/FirstChar` that its `/Widths` gives
/// a width that isn't 0, as far as `/LastChar` ([`Font::width`]), or every
/// code where it has no `/Widths`; a Type 0 font (`composite`) every code
/// of two bytes, whose text is its `/ToUnicode`'s, a predefined CMap's
/// being one the build hasn't. ISO 32000-2 doesn't say what a font has;
/// without its program, a font says it only by the codes it gives widths
/// and text.
fn latin_in(source: &dyn Source, font: &Dict, read: &Font, composite: bool) -> Option<bool> {
    let (codes, widths) = match (composite, given_widths(source, font)) {
        (true, _) => (0..=0xffff, false),
        (false, Some(codes)) => (codes, true),
        (false, None) => (0..=255, false),
    };
    let mut text = false;
    for code in codes {
        if widths && read.width(code) == 0.0 {
            continue;
        }
        let Some(said) = read.code_text(code) else {
            continue;
        };
        if said.chars().any(|c| c.script() == Script::Latin) {
            return Some(true);
        }
        text |= !said.is_empty();
    }
    text.then_some(false)
}

/// The codes a simple font's `/Widths` gives widths for: from its
/// `/FirstChar`, 0 where it has none, as many as `/Widths` has, as far as
/// its `/LastChar` where that's within them and above 0, as PDFium reads
/// them, no code past 255; none where it has no `/Widths`.
fn given_widths(source: &dyn Source, font: &Dict) -> Option<RangeInclusive<u32>> {
    let integer = |key: &[u8]| {
        let (_, value) = through(source, font.get(key)?)?;
        value.as_integer()
    };
    let (_, widths) = through(source, font.get(b"Widths")?)?;
    let widths = widths.as_array()?;
    let first = integer(b"FirstChar").unwrap_or(0);
    let end = first.saturating_add(widths.len() as i64);
    let last = match integer(b"LastChar") {
        Some(last) if last > 0 && last < end => last,
        _ => end - 1,
    };
    // None where the first is below 0, which is past any code, as PDFium
    // reads it into a `size_t`, or the last is.
    let (first, last) = match (u32::try_from(first), u32::try_from(last.min(255))) {
        (Ok(first), Ok(last)) => (first, last),
        _ => (1, 0),
    };
    Some(first..=last)
}

/// Whether `value`, or what's written in place in it no deeper than
/// [`NESTING`], is a reference to an object `unread` says doesn't read as
/// ISO 32000-2 reads it.
fn refers(value: &Object, unread: impl Fn(ObjectId) -> bool) -> bool {
    let mut stack = vec![(value, 0)];
    while let Some((o, depth)) = stack.pop() {
        match o.value() {
            Value::Ref(id) if unread(*id) => return true,
            Value::Array(a) if depth < NESTING => stack.extend(a.iter().map(|e| (e, depth + 1))),
            Value::Dict(d) if depth < NESTING => {
                stack.extend(d.iter().map(|(_, v)| (v, depth + 1)))
            }
            _ => {}
        }
    }
    false
}

impl Walk<'_> {
    /// Reads each font the walk reached, in the order it reached them, its
    /// streams within `budget` bytes decoded in all, but those [`latin`]
    /// read already, which are as it read them, and finds what each read
    /// tells that the walk can't say: see the module's documentation.
    pub(super) fn fonts(&mut self, budget: usize) {
        let fonts = mem::take(&mut self.fonts);
        let mut read = mem::take(&mut self.fonts_read);
        let mut left = budget;
        // What each read tells, in order: found once all are read, against
        // the walk's findings.
        let mut tells: Vec<Told<Discriminant<Fault>>> = Vec::new();
        for font in &fonts {
            let Some(dict) = font.object.as_dict() else {
                continue;
            };
            // Of a kind pdf-fonts would guess: not read, nor counted.
            if !of_a_kind(self.source, dict) {
                continue;
            }
            let key = match font.id {
                Some(id) => Key::Id(id),
                None => Key::Kept(ptr::from_ref(font.object)),
            };
            let taken = match read.remove(&key) {
                Some(taken) => taken,
                None => self::read(self.source, dict, &mut left),
            };
            // Not read, as it asked a stream's data of a source that gives
            // none: not checked, as content isn't there.
            let Some((result, kept)) = taken else {
                continue;
            };
            let mut stops: Vec<&str> = result.err().iter().map(unread).collect();
            let damaged: HashSet<ObjectId> = (kept.told().iter())
                .filter_map(|(_, fault, _)| match fault {
                    Fault::Damaged { id } => Some(*id),
                    _ => None,
                })
                .collect();
            for &(place, fault, n) in kept.told() {
                match fault {
                    Fault::TooManyCMaps => stops.push(CMAPS),
                    // The reader's, which the walk says (`unresolved`); a
                    // predefined CMap the build or PDFium hasn't, of which
                    // a name that's no CMap's is the model's `value`; a
                    // program's, which isn't read.
                    Fault::Damaged { .. } | Fault::PredefinedUnknown | Fault::ProgramMalformed => {}
                    // In what a /UseCMap names: where that's not a stream
                    // the file has, the walk says so at the /UseCMap.
                    Fault::UseCMap if !self.a_stream(place.object) => {}
                    _ => {
                        let object = place.object.or(font.id).or(font.holder);
                        if self.of_the_unread(object, place.at, &damaged) {
                            continue;
                        }
                        if let Some(made) = self.made(object) {
                            tells.push(self.font_told(font, object, made, place.at, fault, n));
                        }
                    }
                }
            }
            if self.made(font.id.or(font.holder)).is_some() {
                stops.sort_unstable();
                stops.dedup();
                for why in stops {
                    self.unchecked("-", why.to_string(), font.table, "-");
                }
            }
        }
        self.find_told(tells);
    }

    /// Whether `object` is a stream the source has.
    fn a_stream(&self, object: Option<ObjectId>) -> bool {
        object.is_some_and(|id| (self.source.get(id)).is_ok_and(|o| o.as_stream().is_some()))
    }

    /// Whether what's written at `at` in `object` is, or holds in place, a
    /// reference to an object of `damaged`, which the read told doesn't
    /// read, or to one that's a reference itself: what's told of it is the
    /// reader's, which the walk says (`unresolved`), or what's made of a
    /// reference to a reference by following it once, as PDFium does, where
    /// ISO 32000-2 follows it to its end (7.3.10).
    fn of_the_unread(
        &self,
        object: Option<ObjectId>,
        at: Option<Span>,
        damaged: &HashSet<ObjectId>,
    ) -> bool {
        let written = match object {
            Some(id) => self.source.get(id).ok(),
            None => Some(&self.source.trailer().object),
        };
        let Some(there) = written
            .zip(at)
            .and_then(|(written, at)| written_at(written, at))
        else {
            return false;
        };
        refers(there, |id| {
            damaged.contains(&id) || (self.source.get(id)).is_ok_and(|o| o.as_reference().is_some())
        })
    }

    /// What `fault` is, told `n` times by `font`'s read, in `object`, made
    /// as `made`, at `at`: at the key [`kind_key`] gives it, and any other
    /// at the key [`holding`] gives, in its object, `-` where none holds it;
    /// its path and table the object's first visit as a font or one of
    /// [`PARTS`], else the font's.
    fn font_told(
        &self,
        font: &Graphic<'_>,
        object: Option<ObjectId>,
        made: Made,
        at: Option<Span>,
        fault: Fault,
        n: usize,
    ) -> Told<Discriminant<Fault>> {
        let written = match object {
            Some(id) => self.source.get(id).ok(),
            None => Some(&self.source.trailer().object),
        };
        let held = written
            .zip(at)
            .map(|(written, at)| (holding(written, at), written, at));
        let key = match kind_key(&fault) {
            Some(key) => key.to_string(),
            None => match held.as_ref().and_then(|(held, ..)| held.as_ref()) {
                Some((key, _)) => key.clone(),
                None => "-".to_string(),
            },
        };
        let whole = object.filter(|_| {
            held.as_ref()
                .is_some_and(|(held, written, at)| held.is_none() && written.at == Some(*at))
        });
        let (path, table) = match object.and_then(|id| self.fonts_at.get(&id)) {
            Some(first) => (first.trail.path(), first.table),
            None => (font.trail.path(), font.table),
        };
        let finding = Finding {
            object,
            made,
            path,
            table: table.to_string(),
            key,
            rule: Rule::Font,
            value: times(&fault.to_string(), n),
            at,
            revision: None,
        };
        Told {
            finding,
            kind: mem::discriminant(&fault),
            whole,
            family: None,
        }
    }
}

#[cfg(test)]
mod tests {
    //! What a font is given of a stream's data, from a reading and from a
    //! document alone; what's read of the fonts the walk reaches; and which
    //! fonts have Latin characters, as a descriptor's `/CapHeight` asks.

    use std::borrow::Cow;
    use std::io;
    use std::ops::Range;
    use std::sync::atomic::{AtomicBool, Ordering};

    use pdf_structure::fixture::{self, Body, Xref};

    use super::*;
    use crate::bytes::{ByteSource, Options, WithReading};
    use crate::check::{Hold, Made, Report, Rule, check};
    use crate::dict;
    use crate::pdf_crypt::writer::{Kind, Settings};

    /// A `/ToUnicode`'s data, as each stream here holds it.
    const PLAIN: &[u8] = b"1 beginbfchar <41> <0041> endbfchar";

    /// What [`decoded`] gives of stream `id` of `source`, within `limit`,
    /// and whether it noted that the source gives no stream's data.
    fn asked(source: &dyn Source, id: ObjectId, limit: usize) -> (Decoded, bool) {
        let ungiven = Cell::new(false);
        (decoded(source, id, limit, &ungiven), ungiven.get())
    }

    /// Why what [`asked`] gave doesn't read, where it's that.
    fn why(asked: (Decoded, bool)) -> Why {
        match asked {
            (Decoded::Unreadable(unreadable), false) => unreadable.why,
            other => panic!("{other:?}"),
        }
    }

    /// A font, object 1, its `/ToUnicode` object 2, and its data through
    /// Flate, object 3.
    fn objects() -> Vec<(u32, Body)> {
        vec![
            (
                1,
                dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica", "ToUnicode" => Object::reference(2) }
                    .into(),
            ),
            (2, Body::stream(dict! {}, PLAIN)),
            (
                3,
                Body::stream(dict! { "Filter" => "FlateDecode" }, &fixture::zlib(PLAIN)),
            ),
        ]
    }

    /// Every table the walk names a font by is the model's, so a font
    /// reached as one is read.
    #[test]
    fn each_font_table_is_the_model_s() {
        for name in FONTS {
            assert!(crate::grammar().table(name).is_some(), "{name}");
        }
    }

    /// A stream within its limit is its data, with filters or without, and
    /// past it too large, by a byte; a document alone gives none, noted,
    /// and a font that asks it isn't read, where a font that asks none is.
    #[test]
    fn a_stream_s_data_within_its_limit_and_past_it() {
        let file = fixture::file("1.7", &objects(), dict! {}, Xref::Table);
        let read = WithReading::new(&file[..], &[], &Options::DEFAULT).unwrap();
        for id in [(2, 0), (3, 0)] {
            let at = PLAIN.len();
            let data = Decoded::Data(PLAIN.to_vec());
            assert_eq!(asked(&read, id, at), (data, false), "{id:?}");
            assert_eq!(asked(&read, id, at - 1), (Decoded::TooLarge, false));
            assert_eq!(asked(read.document(), id, at), (Decoded::None, true));
        }
        // Not a stream: none. Not there: none, and nothing noted, as the
        // source isn't asked for its data.
        assert_eq!(asked(&read, (1, 0), 100), (Decoded::None, false));
        assert_eq!(asked(read.document(), (9, 0), 100), (Decoded::None, false));

        let font = read.get((1, 0)).unwrap().as_dict().unwrap();
        let mut budget = PLAIN.len();
        let (font_read, kept) = super::read(&read, font, &mut budget).unwrap();
        assert_eq!(font_read.unwrap().code_text(0x41).as_deref(), Some("A"));
        assert!(kept.told().is_empty());
        assert_eq!(budget, 0);
        let mut budget = PLAIN.len() - 1;
        let (too_large, _) = super::read(&read, font, &mut budget).unwrap();
        assert_eq!(too_large.err(), Some(Error::TooLarge));
        let mut budget = PLAIN.len();
        assert!(super::read(read.document(), font, &mut budget).is_none());
        let plain = dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica" };
        assert!(super::read(read.document(), &plain, &mut budget).is_some());
    }

    /// A stream that decodes in part is what decoded; none where nothing
    /// did, or the reader doesn't take its filters; past the reader's cap on
    /// filters, a cap's.
    #[test]
    fn a_stream_that_doesn_t_decode_whole() {
        let many: Vec<Object> = (0..26).map(|_| Object::from("ASCIIHexDecode")).collect();
        let objects = vec![
            (
                1,
                Body::stream(dict! { "Filter" => "ASCIIHexDecode" }, b"4142 zz"),
            ),
            (
                2,
                Body::stream(dict! { "Filter" => "ASCIIHexDecode" }, b"zz"),
            ),
            (
                3,
                Body::stream(dict! { "Filter" => "JBIG2Decode" }, b"data"),
            ),
            (5, Body::stream(dict! { "Filter" => many }, b"41>")),
        ];
        let file = fixture::file("1.7", &objects, dict! {}, Xref::Table);
        let read = WithReading::new(&file[..], &[], &Options::DEFAULT).unwrap();
        let data = Decoded::Data(b"AB".to_vec());
        assert_eq!(asked(&read, (1, 0), 100), (data, false));
        assert_eq!(asked(&read, (2, 0), 100), (Decoded::None, false));
        assert_eq!(asked(&read, (3, 0), 100), (Decoded::None, false));
        assert_eq!(why(asked(&read, (5, 0), 100)), Why::Limit);
    }

    /// A locked file's stream takes a password, whether it's an object of
    /// an object stream that doesn't read or its data that doesn't decrypt;
    /// a font that asks it isn't read, and its budget is as it was.
    #[test]
    fn a_locked_file_s_streams() {
        let settings = Settings {
            kind: Kind::Aes256,
            user: b"user",
            owner: b"owner",
            p: -4,
            id: b"0123456789abcdef",
            encrypt_metadata: true,
        };
        let file = fixture::encrypted(&settings, &objects(), dict! {});
        let locked = WithReading::new(&file[..], &[], &Options::DEFAULT).unwrap();
        assert!(matches!(
            locked.decoded_within((2, 0), 100),
            Some(Err(bytes::Error::Locked))
        ));
        assert_eq!(why(asked(&locked, (2, 0), 100)), Why::Locked);
        let font = locked.get((1, 0)).unwrap().as_dict().unwrap();
        let mut budget = 100;
        let (font_read, _) = super::read(&locked, font, &mut budget).unwrap();
        let Err(Error::Unreadable(unreadable)) = font_read else {
            panic!("{font_read:?}");
        };
        assert_eq!((unreadable.id, unreadable.why), ((2, 0), Why::Locked));
        assert_eq!(budget, 100);
    }

    /// A file read by position, whose source fails once it's told to.
    struct Failing {
        bytes: Vec<u8>,
        failing: AtomicBool,
    }

    impl ByteSource for Failing {
        fn len(&self) -> usize {
            self.bytes.len()
        }

        fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
            match self.failing.load(Ordering::Relaxed) {
                true => Err(io::Error::other("no bytes")),
                false => Ok(Cow::Borrowed(&self.bytes[range])),
            }
        }

        fn first_window(&self) -> usize {
            64
        }
    }

    /// A source that fails to give a stream's data is the source's, and
    /// given them again, the data's read.
    #[test]
    fn a_source_that_fails() {
        let source = Failing {
            bytes: fixture::file("1.7", &objects(), dict! {}, Xref::Table),
            failing: AtomicBool::new(false),
        };
        let read = WithReading::new(&source, &[], &Options::DEFAULT).unwrap();
        source.failing.store(true, Ordering::Relaxed);
        assert!(matches!(
            read.decoded_within((2, 0), 100),
            Some(Err(bytes::Error::Io(_)))
        ));
        assert_eq!(why(asked(&read, (2, 0), 100)), Why::Source);
        source.failing.store(false, Ordering::Relaxed);
        let data = Decoded::Data(PLAIN.to_vec());
        assert_eq!(asked(&read, (2, 0), 100), (data, false));
    }

    fn r(n: u32) -> Object {
        Object::reference(n)
    }

    fn numbers(n: &[i64]) -> Object {
        let numbers: Vec<Object> = n.iter().map(|&n| Object::from(n)).collect();
        numbers.into()
    }

    /// The catalog, object 1, its page tree, 2, and its page, 3, whose
    /// resources' fonts are `fonts`; and `more`.
    fn on_a_page(fonts: Dict, more: Vec<(u32, Body)>) -> Vec<(u32, Body)> {
        let page = dict! {
            "Type" => "Page",
            "Parent" => r(2),
            "MediaBox" => numbers(&[0, 0, 612, 792]),
            "Resources" => dict! { "Font" => fonts },
        };
        let mut objects = vec![
            (1, dict! { "Type" => "Catalog", "Pages" => r(2) }.into()),
            (
                2,
                dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1 }.into(),
            ),
            (3, page.into()),
        ];
        objects.extend(more);
        objects
    }

    /// A font descriptor of `flags` (32 nonsymbolic, 4 symbolic), with a
    /// `/CapHeight` where `cap_height`.
    fn descriptor(flags: i64, cap_height: bool) -> Dict {
        let mut descriptor = dict! {
            "Type" => "FontDescriptor",
            "FontName" => "Arial",
            "Flags" => flags,
            "FontBBox" => numbers(&[-200, -200, 1000, 900]),
            "ItalicAngle" => 0,
            "Ascent" => 900,
            "Descent" => -200,
            "StemV" => 80,
        };
        if cap_height {
            descriptor.set("CapHeight", 700);
        }
        descriptor
    }

    /// A TrueType font, of `encoding` where it has one, its `/Widths` from
    /// code `first`, its descriptor `descriptor`.
    fn truetype(encoding: Option<Object>, first: i64, widths: &[i64], descriptor: Object) -> Dict {
        let mut font = dict! {
            "Type" => "Font",
            "Subtype" => "TrueType",
            "BaseFont" => "Arial",
            "FirstChar" => first,
            "LastChar" => first + widths.len() as i64 - 1,
            "Widths" => numbers(widths),
            "FontDescriptor" => descriptor,
        };
        if let Some(encoding) = encoding {
            font.set("Encoding", encoding);
        }
        font
    }

    /// A TrueType font in WinAnsiEncoding drawing the codes from the space
    /// to `~`, its descriptor `descriptor`: Latin letters among them.
    fn win_ansi(descriptor: Object) -> Dict {
        truetype(
            Some(Object::from("WinAnsiEncoding")),
            32,
            &[500; 95],
            descriptor,
        )
    }

    /// A TrueType font drawing only the codes past `z` it names `uni4E00`,
    /// `uni4E01` and `uni4E02` in its `/Differences`, all Han: the letters
    /// its base encoding names at other codes it gives no width.
    fn han(descriptor: Object) -> Dict {
        let names = ["uni4E00", "uni4E01", "uni4E02"].map(Object::from);
        let mut differences = vec![Object::from(123)];
        differences.extend(names);
        let encoding = dict! { "Type" => "Encoding", "Differences" => differences };
        truetype(Some(encoding.into()), 123, &[1000; 3], descriptor)
    }

    /// A Type 0 font in Identity-H, its CIDFont object `cid_font`, and its
    /// `/ToUnicode` where it has one.
    fn type0(cid_font: u32, to_unicode: Option<Object>) -> Dict {
        let mut font = dict! {
            "Type" => "Font",
            "Subtype" => "Type0",
            "BaseFont" => "Batang",
            "Encoding" => "Identity-H",
            "DescendantFonts" => vec![r(cid_font)],
        };
        if let Some(to_unicode) = to_unicode {
            font.set("ToUnicode", to_unicode);
        }
        font
    }

    /// A CIDFont of TrueType glyphs, its descriptor object `descriptor`.
    fn cid_font(descriptor: u32) -> Dict {
        let info = dict! {
            "Registry" => Object::string("Adobe"),
            "Ordering" => Object::string("Identity"),
            "Supplement" => 0,
        };
        dict! {
            "Type" => "Font",
            "Subtype" => "CIDFontType2",
            "BaseFont" => "Batang",
            "CIDSystemInfo" => info,
            "FontDescriptor" => r(descriptor),
            "CIDToGIDMap" => "Identity",
        }
    }

    /// A `/ToUnicode` CMap mapping codes 1 and up, of two bytes, to each
    /// of `text` in turn.
    fn to_unicode(text: &[u16]) -> Body {
        let mut data = String::from("1 begincodespacerange <0000> <FFFF> endcodespacerange\n");
        data += &format!("{} beginbfchar\n", text.len());
        for (i, unit) in text.iter().enumerate() {
            data += &format!("<{:04X}> <{unit:04X}>\n", i + 1);
        }
        data += "endbfchar\n";
        Body::stream(dict! {}, data.as_bytes())
    }

    /// `objects`, the catalog object 1 among them, written at `version` and
    /// checked beside its reading, every object held as built.
    fn checked(version: &str, objects: &[(u32, Body)]) -> Report {
        let file = fixture::file(version, objects, dict! { "Root" => r(1) }, Xref::Table);
        let read = WithReading::new(&file[..], &[], &Options::DEFAULT).unwrap();
        check(
            &read,
            &Hold::All(Made::Built),
            &crate::check::Options::default(),
        )
    }

    /// What a report says of fonts' `/CapHeight`s: each `missing` finding's
    /// object and table; and each table's rows not made for
    /// `fn:FontHasLatinChars`, with how many.
    type CapHeight = (Vec<(Option<u32>, String)>, Vec<(String, usize)>);

    /// [`CapHeight`] of `report`.
    fn cap_height(report: &Report) -> CapHeight {
        let missing = (report.findings.iter())
            .filter(|f| f.key == "CapHeight")
            .inspect(|f| assert_eq!(f.rule, Rule::Missing, "{f}"))
            .map(|f| (f.object.map(|(n, _)| n), f.table.clone()))
            .collect();
        let unmade = (report.unchecked.iter())
            .filter(|(u, _)| u.why == "fn:FontHasLatinChars")
            .inspect(|(u, _)| assert_eq!((u.column, u.key.as_str()), ("Required", "CapHeight")))
            .map(|(u, &n)| (u.table.clone(), n))
            .collect();
        (missing, unmade)
    }

    /// `missing CapHeight` in object `n` of `table`, and no row unmade.
    fn missing(n: u32, table: &str) -> CapHeight {
        (vec![(Some(n), table.to_string())], Vec::new())
    }

    /// Nothing missing, and no row unmade.
    fn made() -> CapHeight {
        (Vec::new(), Vec::new())
    }

    /// A row of `table` not made, and nothing missing.
    fn unmade(table: &str) -> CapHeight {
        (Vec::new(), vec![(table.to_string(), 1)])
    }

    /// A TrueType font's descriptor without `/CapHeight` is found where a
    /// code the font draws, one its `/Widths` gives a width that isn't 0,
    /// has a Latin letter in its text, and not where it has one; it isn't
    /// where the codes it draws have text and none has one, though its
    /// encoding names Latin letters at codes it gives no width, or a width
    /// of 0; and
    /// where no code has text, a symbolic font that embeds its program,
    /// without an encoding or a `/ToUnicode`, it can't be told, as before.
    #[test]
    fn a_truetype_font_without_cap_height_is_found_where_it_draws_latin_letters() {
        let a_font = |font: Dict, flags, with_cap_height| {
            let objects = on_a_page(
                dict! { "F1" => r(4) },
                vec![
                    (4, font.into()),
                    (5, descriptor(flags, with_cap_height).into()),
                ],
            );
            cap_height(&checked("1.7", &objects))
        };
        let truetype_table = "FontDescriptorTrueType";
        assert_eq!(
            a_font(win_ansi(r(5)), 32, false),
            missing(5, truetype_table)
        );
        assert_eq!(a_font(win_ansi(r(5)), 32, true), made());
        assert_eq!(a_font(han(r(5)), 32, false), made());
        // A symbolic font that embeds its program, without an encoding,
        // leaves its codes' text to the program, which isn't read: as
        // before. One that embeds none reads in WinAnsiEncoding, as PDFium
        // reads it.
        let symbolic = |embedded| {
            let mut descriptor = descriptor(4, false);
            if embedded {
                descriptor.set("FontFile2", r(6));
            }
            let objects = on_a_page(
                dict! { "F1" => r(4) },
                vec![
                    (4, truetype(None, 32, &[500; 95], r(5)).into()),
                    (5, descriptor.into()),
                    (6, Body::stream(dict! {}, b"not read")),
                ],
            );
            cap_height(&checked("1.7", &objects))
        };
        assert_eq!(symbolic(true), unmade(truetype_table));
        assert_eq!(symbolic(false), missing(5, truetype_table));
        // Its letters given a width of 0 aren't drawn: its digits and
        // punctuation have text, and no Latin letter.
        let widths: Vec<i64> = (32..127u8)
            .map(|code| match code.is_ascii_alphabetic() {
                true => 0,
                false => 500,
            })
            .collect();
        let no_letters = truetype(Some(Object::from("WinAnsiEncoding")), 32, &widths, r(5));
        assert_eq!(a_font(no_letters, 32, false), made());
        // A /CapHeight of null is absent (ISO 32000-2, 7.3.7).
        let mut null = descriptor(32, false);
        null.set("CapHeight", Object::null());
        let objects = on_a_page(
            dict! { "F1" => r(4) },
            vec![(4, win_ansi(r(5)).into()), (5, null.into())],
        );
        assert_eq!(
            cap_height(&checked("1.7", &objects)),
            missing(5, truetype_table)
        );
        // No /Widths: every code drawn, Latin letters among them.
        let mut no_widths = han(r(5));
        no_widths.remove(b"Widths");
        assert_eq!(a_font(no_widths, 32, false), missing(5, truetype_table));
    }

    /// A Type 0 font's descendant's descriptor without `/CapHeight` is
    /// found where its `/ToUnicode` gives a code a Latin letter, and not
    /// where it gives text of Han alone; without a `/ToUnicode`, its codes
    /// have no text, and it can't be told, as before.
    #[test]
    fn a_type_0_font_without_cap_height_is_found_where_its_text_is_latin() {
        let a_font = |to_unicode_of: Option<&[u16]>| {
            let mut more = vec![(5, cid_font(6).into()), (6, descriptor(4, false).into())];
            if let Some(text) = to_unicode_of {
                more.push((7, to_unicode(text)));
            }
            let font = type0(5, to_unicode_of.map(|_| r(7)));
            more.push((4, font.into()));
            cap_height(&checked("1.7", &on_a_page(dict! { "F1" => r(4) }, more)))
        };
        let table = "FontDescriptorCIDType2";
        assert_eq!(a_font(Some(&[0x4e00, 0x4e01])), made());
        assert_eq!(a_font(Some(&[0x4e00, 0x41])), missing(6, table));
        assert_eq!(a_font(None), unmade(table));
    }

    /// A Type 3 font's descriptor isn't answered, as ISO 32000-2 asks no
    /// `/CapHeight` of it (9.8.1, Table 120): its row stays not made, as
    /// before, though its glyphs' names are Latin letters.
    #[test]
    fn a_type_3_font_s_descriptor_is_asked_nothing_of() {
        let font = dict! {
            "Type" => "Font",
            "Subtype" => "Type3",
            "FontBBox" => numbers(&[0, 0, 1000, 1000]),
            "FontMatrix" => vec![
                Object::from(0.001), Object::from(0), Object::from(0),
                Object::from(0.001), Object::from(0), Object::from(0),
            ],
            "CharProcs" => dict! { "a" => r(6) },
            "Encoding" => dict! { "Type" => "Encoding", "Differences" => vec![Object::from(97), Object::from("a")] },
            "FirstChar" => 97,
            "LastChar" => 97,
            "Widths" => numbers(&[1000]),
            "FontDescriptor" => r(5),
            "Resources" => dict! {},
        };
        let mut type3 = descriptor(32, false);
        type3.remove(b"FontBBox");
        let objects = on_a_page(
            dict! { "F1" => r(4) },
            vec![
                (4, font.into()),
                (5, type3.into()),
                (6, Body::stream(dict! {}, b"1000 0 d0")),
            ],
        );
        assert_eq!(
            cap_height(&checked("1.7", &objects)),
            unmade("FontDescriptorType3")
        );
    }

    /// A descriptor two fonts name has Latin characters where either font
    /// has, whichever comes first: found once.
    #[test]
    fn a_descriptor_two_fonts_share_is_found_where_either_is_latin() {
        let objects = on_a_page(
            dict! { "F1" => r(4), "F2" => r(5) },
            vec![
                (4, han(r(6)).into()),
                (5, win_ansi(r(6)).into()),
                (6, descriptor(32, false).into()),
            ],
        );
        assert_eq!(
            cap_height(&checked("1.7", &objects)),
            missing(6, "FontDescriptorTrueType")
        );
    }

    /// A font written in place in its resources is read as one of its own
    /// is, and so is a descriptor written in place in its font, known by
    /// where it was written: each found, in the object it's written in.
    #[test]
    fn a_font_or_a_descriptor_written_in_place_is_read() {
        let in_place = on_a_page(
            dict! { "F1" => win_ansi(r(5)) },
            vec![(5, descriptor(32, false).into())],
        );
        assert_eq!(
            cap_height(&checked("1.7", &in_place)),
            missing(5, "FontDescriptorTrueType")
        );
        let its_descriptor = on_a_page(
            dict! { "F1" => r(4) },
            vec![(4, win_ansi(descriptor(32, false).into()).into())],
        );
        assert_eq!(
            cap_height(&checked("1.7", &its_descriptor)),
            missing(4, "FontDescriptorTrueType")
        );
    }

    /// A font whose `/ToUnicode` takes a password that wasn't given isn't
    /// read, and its descriptor's `/CapHeight` can't be told, as before.
    #[test]
    fn a_font_whose_stream_takes_a_password_isn_t_told() {
        let settings = Settings {
            kind: Kind::Aes256,
            user: b"user",
            owner: b"owner",
            p: -4,
            id: b"0123456789abcdef",
            encrypt_metadata: true,
        };
        let objects = on_a_page(
            dict! { "F1" => r(4) },
            vec![
                (4, type0(5, Some(r(7))).into()),
                (5, cid_font(6).into()),
                (6, descriptor(4, false).into()),
                (7, to_unicode(&[0x41])),
            ],
        );
        let file = fixture::encrypted(&settings, &objects, dict! { "Root" => r(1) });
        let locked = WithReading::new(&file[..], &[], &Options::DEFAULT).unwrap();
        let report = check(
            &locked,
            &Hold::All(Made::Built),
            &crate::check::Options::default(),
        );
        assert_eq!(cap_height(&report), unmade("FontDescriptorCIDType2"));
        // Its read is counted as not checked, and what it tells once the
        // stream doesn't read, a /ToUnicode as if it didn't decode, isn't
        // found.
        assert_eq!(unread_fonts(&report), [(LOCKED, "FontType0", 1)]);
        assert!(
            report.findings.iter().all(|f| f.rule != Rule::Font),
            "{report}"
        );
    }

    /// Every table the walk names what a font holds by is the model's, so a
    /// fault in an object first visited as one has its path.
    #[test]
    fn each_part_table_is_the_model_s() {
        for name in PARTS {
            assert!(crate::grammar().table(name).is_some(), "{name}");
        }
    }

    /// A file at `version` of a catalog, object 1, its page tree, 2, and its
    /// page, 3, whose `/Font` resources hold `fonts` as written, and
    /// `objects` besides, each its number and as written.
    fn fonts(version: &str, fonts: &str, objects: &[(u32, &str)]) -> Vec<u8> {
        let page = format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << \
             {fonts} >> >> >>"
        );
        let mut all: Vec<(u32, &[u8])> = vec![
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, page.as_bytes()),
        ];
        all.extend(objects.iter().map(|&(n, body)| (n, body.as_bytes())));
        let size = all.iter().map(|&(n, _)| n).max().unwrap_or(0) + 1;
        fixture::assembled(version, &all, &format!("<< /Size {size} /Root 1 0 R >>"))
    }

    /// A stream of `entries` and `data`, as written.
    fn stream(entries: &str, data: &str) -> String {
        let length = data.len();
        format!("<< {entries} /Length {length} >>\nstream\n{data}\nendstream")
    }

    /// Every object held, as merged.
    const ALL: Hold = Hold::All(Made::Merged);

    /// `file` read beside its reading and checked, holding what `hold`
    /// does, under `options`.
    fn report_with(file: &[u8], hold: &Hold, options: &crate::check::Options) -> Report {
        let read = WithReading::new(file, &[], &Options::DEFAULT).unwrap();
        check(&read, hold, options)
    }

    /// [`report_with`] the default options.
    fn report(file: &[u8], hold: &Hold) -> Report {
        report_with(file, hold, &crate::check::Options::default())
    }

    /// A font finding, as [`found`] gives it.
    type Found = (Option<u32>, String, String, String, String);

    /// Each font finding of `report`, of `file`: its object's number, path,
    /// table and key, value, and the bytes it's at in the file or a
    /// stream's data, `-` there.
    fn found_in(file: &[u8], report: &Report) -> Vec<Found> {
        (report.findings.iter())
            .filter(|f| f.rule == Rule::Font)
            .map(|f| {
                let at = match f.at {
                    Some(at) if at.stream.is_none() => {
                        String::from_utf8_lossy(&file[at.start..at.end]).into_owned()
                    }
                    Some(_) => "-".to_string(),
                    None => String::new(),
                };
                (
                    f.object.map(|(n, _)| n),
                    f.path.clone(),
                    format!("{}/{}", f.table, f.key),
                    f.value.clone(),
                    at,
                )
            })
            .collect()
    }

    /// [`found_in`] `file`'s check, holding what `hold` does.
    fn found(file: &[u8], hold: &Hold) -> Vec<Found> {
        found_in(file, &report(file, hold))
    }

    /// A finding of [`found`]'s, in object `n`, at the path from the page's
    /// `/Font` (`F1/Widths`), of the row `table`, of `fault` told once, at
    /// `bytes`.
    fn told(n: u32, path: &str, table: &str, fault: Fault, bytes: &str) -> Found {
        (
            Some(n),
            format!("Root/Pages/Kids[0]/Resources/Font/{path}"),
            table.to_string(),
            fault.to_string(),
            bytes.to_string(),
        )
    }

    /// The model's findings of `file`'s check, every object held: each
    /// one's object's number, table and key, and rule.
    fn model(file: &[u8]) -> Vec<(Option<u32>, String, Rule)> {
        let report = report(file, &ALL);
        (report.findings.iter())
            .filter(|f| f.rule != Rule::Font)
            .map(|f| {
                (
                    f.object.map(|(n, _)| n),
                    format!("{}/{}", f.table, f.key),
                    f.rule,
                )
            })
            .collect()
    }

    /// The rows a check counts as not checked for a font: why, its table,
    /// and how many.
    fn unread_fonts(report: &Report) -> Vec<(&str, &str, usize)> {
        (report.unchecked.iter())
            .filter(|(u, _)| u.column == "-" && u.key == "-" && u.why.starts_with("a font"))
            .map(|(u, &n)| (u.why.as_str(), u.table.as_str(), n))
            .collect()
    }

    /// A descriptor, object 5, with a `/CapHeight`, so that nothing is
    /// asked of its font's text.
    const DESCRIPTOR: (u32, &str) = (
        5,
        "<< /Type /FontDescriptor /FontName /Calibri /Flags 32 /FontBBox [0 0 1000 1000] \
         /ItalicAngle 0 /Ascent 900 /Descent -200 /CapHeight 700 /StemV 80 >>",
    );

    /// A font of `subtype` named Calibri, which isn't one of the standard
    /// 14, its descriptor [`DESCRIPTOR`], and `entries` besides.
    fn calibri(subtype: &str, entries: &str) -> String {
        format!(
            "<< /Type /Font /Subtype /{subtype} /BaseFont /Calibri /FontDescriptor 5 0 R \
             {entries} >>"
        )
    }

    /// Three codes from the space, with a width each.
    const WIDTHS: &str = "/FirstChar 32 /LastChar 34 /Widths [500 500 500]";

    /// A Type 0 font, object 4, in `encoding`, of CIDFont object 6, its
    /// descriptor 7; and the CIDFont with `entries` besides.
    fn written_type0(encoding: &str, entries: &str) -> Vec<(u32, String)> {
        vec![
            (
                4,
                format!(
                    "<< /Type /Font /Subtype /Type0 /BaseFont /Batang /Encoding {encoding} \
                     /DescendantFonts [6 0 R] >>"
                ),
            ),
            (
                6,
                format!(
                    "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Batang /CIDSystemInfo \
                     << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
                     /FontDescriptor 7 0 R /CIDToGIDMap /Identity {entries} >>"
                ),
            ),
            (
                7,
                "<< /Type /FontDescriptor /FontName /Batang /Flags 4 /FontBBox [0 0 1000 1000] \
                 /ItalicAngle 0 /Ascent 900 /Descent -200 /CapHeight 700 /StemV 80 >>"
                    .to_string(),
            ),
        ]
    }

    /// A CMap stream named `name`, of codes of two bytes, `/UseCMap`
    /// `uses` where it's given.
    fn cmap(name: &str, uses: Option<&str>) -> String {
        let uses = uses.map(|u| format!("/UseCMap {u}")).unwrap_or_default();
        cmap_of(
            name,
            &uses,
            "1 begincodespacerange <0000> <FFFF> endcodespacerange",
        )
    }

    /// A CMap stream named `name`, with `entries` besides, of `data`.
    fn cmap_of(name: &str, entries: &str, data: &str) -> String {
        stream(
            &format!(
                "/Type /CMap /CMapName /{name} /CIDSystemInfo << /Registry (Adobe) /Ordering \
                 (Identity) /Supplement 0 >> {entries}"
            ),
            data,
        )
    }

    /// `objects`, made of what [`written_type0`] and the like give.
    fn borrowed(objects: &[(u32, String)]) -> Vec<(u32, &str)> {
        objects.iter().map(|(n, o)| (*n, o.as_str())).collect()
    }

    /// What pdf-fonts tells of a font that the walk can't say is found once,
    /// at its key and in its object: the font's number, its CIDFont's, its
    /// encoding's, a CMap's, the array a chain leads to; its path and table
    /// its object's as the walk first visited it as a font or as what a
    /// font holds, else the font's.
    #[test]
    fn what_a_font_s_read_tells_that_the_walk_can_t_is_found_once_at_its_key() {
        let one = |font: String, more: &[(u32, &str)]| {
            let mut objects = vec![(4, font.as_str()), DESCRIPTOR];
            objects.extend_from_slice(more);
            found(&fonts("1.7", "/F1 4 0 R", &objects), &ALL)
        };
        // A TrueType font with no /Widths, which the model asks only of a
        // Type 1 font, at 1.7.
        assert_eq!(
            one(calibri("TrueType", ""), &[]),
            [told(4, "F1", "FontTrueType/Widths", Fault::NoWidths, "")]
        );
        // A TrueType font's /Widths of another length than its codes.
        assert_eq!(
            one(
                calibri("TrueType", "/FirstChar 32 /LastChar 34 /Widths [500 500]"),
                &[]
            ),
            [told(
                4,
                "F1",
                "FontTrueType/Widths",
                Fault::WidthsLength,
                "[500 500]"
            )]
        );
        // A /FirstChar past a byte's codes.
        assert_eq!(
            one(
                calibri("TrueType", "/FirstChar 300 /LastChar 300 /Widths [500]"),
                &[]
            ),
            [told(
                4,
                "F1",
                "FontTrueType/FirstChar",
                Fault::FirstChar,
                "300"
            )]
        );
        // A /Differences name past 255, in its encoding's object.
        let encoding = "<< /Type /Encoding /Differences [300 /A] >>";
        assert_eq!(
            one(
                calibri("TrueType", &format!("{WIDTHS} /Encoding 6 0 R")),
                &[(6, encoding)]
            ),
            [told(
                6,
                "F1/Encoding",
                "Encoding/1",
                Fault::Differences,
                "/A"
            )]
        );
        // A /ToUnicode that doesn't decode.
        let broken = stream("/Filter /ASCIIHexDecode", "zz");
        assert_eq!(
            one(
                calibri("TrueType", &format!("{WIDTHS} /ToUnicode 6 0 R")),
                &[(6, &broken)]
            ),
            [told(
                4,
                "F1",
                "FontTrueType/ToUnicode",
                Fault::ToUnicode,
                "6 0 R"
            )]
        );
        // A reference to a reference at /Widths, in the array it leads to,
        // at no bytes; what PDFium makes of it, no /Widths, isn't told.
        assert_eq!(
            one(
                calibri("TrueType", "/FirstChar 32 /LastChar 34 /Widths 6 0 R"),
                &[(6, "7 0 R"), (7, "[500 500 500]")]
            ),
            [told(
                7,
                "F1/Widths",
                "ArrayOfNumbersGeneral/-",
                Fault::Chained,
                ""
            )]
        );

        let cid = |encoding: &str, entries: &str, more: &[(u32, String)]| {
            let mut objects = written_type0(encoding, entries);
            objects.extend_from_slice(more);
            found(&fonts("1.7", "/F1 4 0 R", &borrowed(&objects)), &ALL)
        };
        // A /DescendantFonts of two.
        let mut two = written_type0("/Identity-H", "");
        two[0].1 = two[0].1.replace("[6 0 R]", "[6 0 R 6 0 R]");
        assert_eq!(
            found(&fonts("1.7", "/F1 4 0 R", &borrowed(&two)), &ALL),
            [told(
                4,
                "F1",
                "FontType0/DescendantFonts",
                Fault::DescendantFonts,
                "[6 0 R 6 0 R]"
            )]
        );
        // A /W and a /W2 cut short by an array where a range's last CID
        // goes, at that array, in the CIDFont; a /W2 is read where the font
        // writes vertically.
        assert_eq!(
            cid("/Identity-H", "/W [1 2 [500]]", &[]),
            [told(
                6,
                "F1/DescendantFonts[0]",
                "FontCIDType2/2",
                Fault::W,
                "[500]"
            )]
        );
        assert_eq!(
            cid("/Identity-V", "/W2 [1 2 [-1000 500 880]]", &[]),
            [told(
                6,
                "F1/DescendantFonts[0]",
                "FontCIDType2/2",
                Fault::W2,
                "[-1000 500 880]"
            )]
        );
        // A CMap chain that loops, in the CMap it comes back to.
        assert_eq!(
            cid(
                "8 0 R",
                "",
                &[
                    (8, cmap("Eight", Some("9 0 R"))),
                    (9, cmap("Nine", Some("8 0 R")))
                ]
            ),
            [told(8, "F1/Encoding", "CMapStream/-", Fault::UseCMap, "")]
        );
        // A CMap a /UseCMap names that doesn't decode, in it: the model
        // links a /UseCMap's stream to no table of a font's, so its path and
        // table are the font's.
        let broken = cmap_of("Nine", "/Filter /ASCIIHexDecode", "zz");
        assert_eq!(
            cid(
                "8 0 R",
                "",
                &[(8, cmap("Eight", Some("9 0 R"))), (9, broken)]
            ),
            [told(9, "F1", "FontType0/-", Fault::UseCMap, "")]
        );
    }

    /// What the walk says of a font by the model's rules isn't told again,
    /// at the key it's at, in place or through a reference: a Type 1 font's
    /// `/Widths` or `/FirstChar` missing, or its `/Widths` of another
    /// length, and a TrueType font's `/Widths` missing at 2.0; a Type 0
    /// font's `/DescendantFonts` missing; an `/Encoding` or `/BaseEncoding` that's no encoding's name, by
    /// a reference too; a `/Differences` value that's a string; a metric
    /// that's no number in a `/W`; a `/ToUnicode` that's no stream; a
    /// `/UseCMap` to nothing or to a dictionary, which pdf-fonts tells in
    /// what it names; a CIDFont's `/Subtype` that's no CIDFont's; a name
    /// that's no predefined CMap's; and what's told of a value that refers
    /// to an object that doesn't read. A predefined CMap the build hasn't is
    /// nobody's.
    #[test]
    fn what_the_walk_says_of_a_font_is_found_by_it_alone() {
        let simple = |version: &str, font: String, more: &[(u32, &str)]| {
            let mut objects = vec![(4, font.as_str()), DESCRIPTOR];
            objects.extend_from_slice(more);
            fonts(version, "/F1 4 0 R", &objects)
        };
        let cid = |encoding: &str, entries: &str, more: &[(u32, String)]| {
            let mut objects = written_type0(encoding, entries);
            objects.extend_from_slice(more);
            fonts("1.7", "/F1 4 0 R", &borrowed(&objects))
        };
        // Each file, and the object, table and key, and rule of what the
        // walk finds there.
        type Said<'a> = (Option<u32>, &'a str, Rule);
        let encoding = |entries: &str| format!("<< /Type /Encoding {entries} >>");
        let to_nothing = cmap("Eight", Some("9 0 R"));
        let cases: Vec<(Vec<u8>, Said)> = vec![
            (
                simple("1.7", calibri("Type1", ""), &[]),
                (Some(4), "FontType1/Widths", Rule::Missing),
            ),
            (
                simple(
                    "1.7",
                    calibri("Type1", "/LastChar 34 /Widths [500 500 500]"),
                    &[],
                ),
                (Some(4), "FontType1/FirstChar", Rule::Missing),
            ),
            (
                simple("2.0", calibri("TrueType", ""), &[]),
                (Some(4), "FontTrueType/Widths", Rule::Missing),
            ),
            (
                simple(
                    "1.7",
                    calibri("Type1", "/FirstChar 32 /LastChar 34 /Widths [500 500]"),
                    &[],
                ),
                (Some(4), "FontType1/Widths", Rule::Special),
            ),
            (
                simple(
                    "1.7",
                    calibri("TrueType", &format!("{WIDTHS} /Encoding /Foo")),
                    &[],
                ),
                (Some(4), "FontTrueType/Encoding", Rule::Value),
            ),
            (
                simple(
                    "1.7",
                    calibri("TrueType", &format!("{WIDTHS} /Encoding 6 0 R")),
                    &[(6, "/Foo")],
                ),
                (Some(4), "FontTrueType/Encoding", Rule::Value),
            ),
            (
                simple(
                    "1.7",
                    calibri("TrueType", &format!("{WIDTHS} /Encoding 6 0 R")),
                    &[(6, &encoding("/BaseEncoding /Foo"))],
                ),
                (Some(6), "Encoding/BaseEncoding", Rule::Value),
            ),
            (
                simple(
                    "1.7",
                    calibri("TrueType", &format!("{WIDTHS} /Encoding 6 0 R")),
                    &[(6, &encoding("/Differences [32 (x)]"))],
                ),
                (Some(6), "ArrayOfDifferences/1", Rule::Type),
            ),
            (
                simple(
                    "1.7",
                    calibri("TrueType", &format!("{WIDTHS} /ToUnicode /Identity-H")),
                    &[],
                ),
                (Some(4), "FontTrueType/ToUnicode", Rule::Type),
            ),
            (
                cid("/Identity-H", "/W [1 [500 (x)]]", &[]),
                (Some(6), "ArrayOfNumbersGeneral/1", Rule::Type),
            ),
            (
                cid("8 0 R", "", &[(8, to_nothing.clone())]),
                (Some(8), "CMapStream/UseCMap", Rule::Unresolved),
            ),
            (
                cid(
                    "8 0 R",
                    "",
                    &[(8, to_nothing.clone()), (9, "<< >>".to_string())],
                ),
                (Some(8), "CMapStream/UseCMap", Rule::Type),
            ),
            (
                {
                    let mut objects = written_type0("/Identity-H", "");
                    objects[1].1 = objects[1].1.replace("/CIDFontType2", "/CIDFontType9");
                    fonts("1.7", "/F1 4 0 R", &borrowed(&objects))
                },
                (Some(6), "FontCIDType0/Subtype", Rule::Value),
            ),
            (
                cid("/Foo", "", &[]),
                (Some(4), "FontType0/Encoding", Rule::Value),
            ),
            (
                {
                    let mut objects = written_type0("/Identity-H", "");
                    objects[0].1 = objects[0].1.replace("/DescendantFonts [6 0 R]", "");
                    fonts("1.7", "/F1 4 0 R", &borrowed(&objects))
                },
                (Some(4), "FontType0/DescendantFonts", Rule::Missing),
            ),
        ];
        for (i, (file, said)) in cases.iter().enumerate() {
            let (object, key, rule) = *said;
            let said = (object, key.to_string(), rule);
            let report = report(file, &ALL);
            assert!(model(file).contains(&said), "case {i}: {report}");
            assert_eq!(found(file, &ALL), [], "case {i}: {report}");
        }

        // A predefined CMap the build hasn't: nothing found, by either.
        let gbk = cid("/GBK-EUC-H", "", &[]);
        let checked = report(&gbk, &ALL);
        assert_eq!(found(&gbk, &ALL), [], "{checked}");
        assert!(
            (checked.findings.iter()).all(|f| f.key != "Encoding"),
            "{checked}"
        );

        // A CIDFont that doesn't read: the reader's, and what's told of the
        // /DescendantFonts naming it, not one CIDFont, isn't told either.
        let mut objects = written_type0("/Identity-H", "");
        objects[1].1 = "]".to_string();
        let damaged = fonts("1.7", "/F1 4 0 R", &borrowed(&objects));
        let said = (
            Some(4),
            "ArrayOfDescendantFonts/0".to_string(),
            Rule::Unresolved,
        );
        assert!(
            model(&damaged).contains(&said),
            "{}",
            report(&damaged, &ALL)
        );
        assert_eq!(found(&damaged, &ALL), []);
    }

    /// A font whose `/Subtype` names no kind of font as ISO 32000-2 and
    /// pdf-fonts both read it isn't read, nor counted, so nothing of the
    /// kind pdf-fonts would read it as is found: a Type 0 font whose
    /// `/Subtype` is misspelt or absent, where its read as a Type 1 font
    /// would tell its `/Widths` missing and `/Identity-H` no encoding; a
    /// dictionary that's no font at all, written in place or an object of
    /// its own; and a string, which it would read as a TrueType font where
    /// the walk takes a Type 3 one; each beside what the walk says of the
    /// `/Subtype`. Nor one through a reference to a reference, which ISO
    /// 32000-2 follows to a TrueType font's and pdf-fonts doesn't, so what
    /// would be found of that is missed, and its descriptor's `/CapHeight`
    /// can't be told. Through one reference it's read; and where that leads
    /// to an object that takes a password, it's read and counted.
    #[test]
    fn a_font_whose_subtype_names_no_kind_of_font_isn_t_read() {
        // A Type 0 font, its /Subtype as `subtype` writes it, its CIDFont's
        // /W cut short.
        let type0 = |subtype: &str| {
            let mut objects = written_type0("/Identity-H", "/W [1 2 [500]]");
            objects[0].1 = objects[0].1.replace("/Subtype /Type0", subtype);
            fonts("1.7", "/F1 4 0 R", &borrowed(&objects))
        };
        assert_eq!(
            found(&type0("/Subtype /Type0"), &ALL),
            [told(
                6,
                "F1/DescendantFonts[0]",
                "FontCIDType2/2",
                Fault::W,
                "[500]"
            )]
        );
        // A string where a name goes, which pdf-fonts reads as the name it
        // spells, as PDFium does, where the walk takes the font for a Type
        // 3 one: a /FirstChar past a byte's codes is what its read would
        // tell.
        let string = calibri("TrueType", "/FirstChar 300 /LastChar 300 /Widths [500]")
            .replace("/Subtype /TrueType", "/Subtype (TrueType)");
        type Said<'a> = (Option<u32>, &'a str, Rule);
        let cases: Vec<(Vec<u8>, Said)> = vec![
            (
                type0("/Subtype /TypeO"),
                (Some(4), "FontType0/Subtype", Rule::Value),
            ),
            (type0(""), (Some(4), "FontType0/Subtype", Rule::Missing)),
            (
                fonts("1.7", "/F1 << >>", &[]),
                (Some(3), "FontType1/Subtype", Rule::Missing),
            ),
            (
                fonts("1.7", "/F1 4 0 R", &[(4, "<< /Value 1 >>")]),
                (Some(4), "FontType1/Subtype", Rule::Missing),
            ),
            (
                fonts("1.7", "/F1 4 0 R", &[(4, &string), DESCRIPTOR]),
                (Some(4), "FontType3/Subtype", Rule::Type),
            ),
        ];
        for (i, (file, said)) in cases.iter().enumerate() {
            let (object, key, rule) = *said;
            let said = (object, key.to_string(), rule);
            let checked = report(file, &ALL);
            assert!(model(file).contains(&said), "case {i}: {checked}");
            assert_eq!(found_in(file, &checked), [], "case {i}: {checked}");
            assert_eq!(unread_fonts(&checked), [], "case {i}: {checked}");
        }

        // A TrueType font with no /Widths at 1.7, its /Subtype `subtype`,
        // and `more` besides.
        let truetype = |subtype: &str, more: &[(u32, &str)]| {
            let font = calibri("TrueType", "").replace("/Subtype /TrueType", subtype);
            let mut objects = vec![(4, font.as_str()), DESCRIPTOR];
            objects.extend_from_slice(more);
            fonts("1.7", "/F1 4 0 R", &objects)
        };
        let no_widths = [told(4, "F1", "FontTrueType/Widths", Fault::NoWidths, "")];
        assert_eq!(
            found(&truetype("/Subtype 8 0 R", &[(8, "/TrueType")]), &ALL),
            no_widths
        );
        let chained = truetype("/Subtype 8 0 R", &[(8, "9 0 R"), (9, "/TrueType")]);
        let checked = report(&chained, &ALL);
        assert_eq!(found_in(&chained, &checked), [], "{checked}");
        assert_eq!(unread_fonts(&checked), [], "{checked}");

        // Its descriptor without /CapHeight, the font drawing A in
        // WinAnsiEncoding.
        let descriptor = DESCRIPTOR.1.replace("/CapHeight 700 ", "");
        let latin = |subtype: &str, more: &[(u32, &str)]| {
            let entries = "/FirstChar 65 /LastChar 65 /Widths [500] /Encoding /WinAnsiEncoding";
            let font = calibri("TrueType", entries).replace("/Subtype /TrueType", subtype);
            let mut objects = vec![(4, font.as_str()), (5, descriptor.as_str())];
            objects.extend_from_slice(more);
            cap_height(&report(&fonts("1.7", "/F1 4 0 R", &objects), &ALL))
        };
        let table = "FontDescriptorTrueType";
        assert_eq!(
            latin("/Subtype 8 0 R", &[(8, "/TrueType")]),
            missing(5, table)
        );
        assert_eq!(
            latin("/Subtype 8 0 R", &[(8, "9 0 R"), (9, "/TrueType")]),
            unmade(table)
        );

        // Its /Subtype in an object stream of a file the reader can't open.
        let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << \
                    /Font << /F1 4 0 R >> >> >>";
        let font = calibri("TrueType", WIDTHS).replace("/Subtype /TrueType", "/Subtype 8 0 R");
        let objects = [
            (1, "<< /Type /Catalog /Pages 2 0 R >>"),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, page),
            (4, &font),
            DESCRIPTOR,
        ];
        let trailer = "/Root 1 0 R /Encrypt << /Filter /Standard /V 2 >>";
        let locked = super::super::color::tests::packed(&objects, &[(8, "/TrueType")], trailer);
        let checked = report(&locked, &ALL);
        assert_eq!(found_in(&locked, &checked), [], "{checked}");
        assert_eq!(
            unread_fonts(&checked),
            [(LOCKED, "FontType1", 1)],
            "{checked}"
        );
    }

    /// A font not read whole for what isn't the file's fault is counted as
    /// not checked, once, its table the font's, where the font or what it's
    /// written in is held, and what was read of it before is found all the
    /// same: one whose `/ToUnicode` is in an object stream of a file the
    /// reader can't open, whose read goes on as if it weren't there, and
    /// what it tells after isn't found; a CMap chain past 512; and fonts
    /// past the file's budget for their streams, those before it read.
    #[test]
    fn a_font_not_read_whole_for_what_isnt_the_file_s_fault_is_counted() {
        let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << \
                    /Font << /F1 4 0 R >> >> >>";
        // Its /Differences read before its /ToUnicode, and its /FirstChar
        // after.
        let font = calibri(
            "TrueType",
            "/FirstChar 300 /LastChar 300 /Widths [500] /Encoding 6 0 R /ToUnicode 11 0 R",
        );
        let objects = [
            (1, "<< /Type /Catalog /Pages 2 0 R >>"),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, page),
            (4, &font),
            DESCRIPTOR,
            (6, "<< /Type /Encoding /Differences [300 /A] >>"),
        ];
        let trailer = "/Root 1 0 R /Encrypt << /Filter /Standard /V 2 >>";
        let to_unicode = stream("", "1 beginbfchar <41> <0041> endbfchar");
        let locked = super::super::color::tests::packed(&objects, &[(11, &to_unicode)], trailer);
        let checked = report(&locked, &ALL);
        assert_eq!(
            unread_fonts(&checked),
            [(LOCKED, "FontTrueType", 1)],
            "{checked}"
        );
        assert_eq!(
            found(&locked, &ALL),
            [told(
                6,
                "F1/Encoding",
                "Encoding/1",
                Fault::Differences,
                "/A"
            )]
        );
        let only = |n| Hold::Only([((n, 0), Made::Merged)].into());
        assert_eq!(unread_fonts(&report(&locked, &only(4))).len(), 1);
        assert_eq!(unread_fonts(&report(&locked, &only(3))), []);

        // CMaps 8 to 520, each using the next.
        let chain: Vec<(u32, String)> = (8..521)
            .map(|n| (n, cmap(&format!("C{n}"), Some(&format!("{} 0 R", n + 1)))))
            .collect();
        let mut objects = written_type0("8 0 R", "");
        objects.extend(chain);
        let long = fonts("1.7", "/F1 4 0 R", &borrowed(&objects));
        let checked = report(&long, &ALL);
        assert_eq!(
            unread_fonts(&checked),
            [(CMAPS, "FontType0", 1)],
            "{checked}"
        );
        assert_eq!(found_in(&long, &checked), []);

        // Two fonts, each a /ToUnicode of 36 bytes and a /Differences name
        // past 255, within 40 bytes: the second's read stops at its
        // /ToUnicode, after its /Differences.
        let differences = "/Encoding << /Differences [300 /A] >>";
        let font = |to_unicode: u32| {
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica {differences} /ToUnicode \
                 {to_unicode} 0 R >>"
            )
        };
        let (first, second) = (font(6), font(7));
        let to_unicode = stream("", "1 beginbfchar <41> <0041> endbfchar");
        let two = fonts(
            "1.7",
            "/F1 4 0 R /F2 5 0 R",
            &[
                (4, &first),
                (5, &second),
                (6, &to_unicode),
                (7, &to_unicode),
            ],
        );
        let options = crate::check::Options {
            font_budget: Some(40),
            ..crate::check::Options::default()
        };
        let checked = report_with(&two, &ALL, &options);
        assert_eq!(
            unread_fonts(&checked),
            [(BUDGET, "FontType1", 1)],
            "{checked}"
        );
        let past = |n, path: &str| told(n, path, "FontType1/1", Fault::Differences, "/A");
        assert_eq!(found_in(&two, &checked), [past(4, "F1"), past(5, "F2")]);
        // Within the budget, both read.
        assert_eq!(unread_fonts(&report(&two, &ALL)), []);
    }

    /// What two fonts' reads tell in one object is told once, by the first:
    /// an encoding two fonts share, and a font two pages share, which the
    /// walk reaches and reads once.
    #[test]
    fn what_two_reads_tell_in_one_object_is_told_once() {
        let page = |fonts: &str| {
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font \
                 << {fonts} >> >> >>"
            )
        };
        let (first, second) = (page("/F1 4 0 R /F2 6 0 R"), page("/F1 4 0 R"));
        let font = calibri("TrueType", &format!("{WIDTHS} /Encoding 7 0 R"));
        let objects: [(u32, &[u8]); 8] = [
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (2, b"<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 >>"),
            (3, first.as_bytes()),
            (8, second.as_bytes()),
            (4, font.as_bytes()),
            (6, font.as_bytes()),
            (DESCRIPTOR.0, DESCRIPTOR.1.as_bytes()),
            (7, b"<< /Type /Encoding /Differences [300 /A] >>"),
        ];
        let file = fixture::assembled("1.7", &objects, "<< /Size 9 /Root 1 0 R >>");
        assert_eq!(
            found(&file, &ALL),
            [told(
                7,
                "F1/Encoding",
                "Encoding/1",
                Fault::Differences,
                "/A"
            )]
        );
    }

    /// A font's finding is found where its object is held: a font of its
    /// own, held or not, whatever holds its page; one written in place,
    /// where its page is.
    #[test]
    fn a_font_s_finding_is_held_where_its_object_is() {
        let only = |n| Hold::Only([((n, 0), Made::Merged)].into());
        let short = "/FirstChar 32 /LastChar 34 /Widths [500 500]";
        let font = calibri("TrueType", short);
        let own = fonts("1.7", "/F1 4 0 R", &[(4, &font), DESCRIPTOR]);
        let length = Fault::WidthsLength;
        let told_at = |n| told(n, "F1", "FontTrueType/Widths", length, "[500 500]");
        assert_eq!(found(&own, &only(4)), [told_at(4)]);
        assert_eq!(found(&own, &only(3)), []);
        let in_place = fonts("1.7", &format!("/F1 {font}"), &[DESCRIPTOR]);
        assert_eq!(found(&in_place, &only(3)), [told_at(3)]);
        assert_eq!(found(&in_place, &only(2)), []);
    }

    /// A font finding is carried by a like one, whose count differs: by
    /// number, by path and from a source; one of a kind the earlier report
    /// hasn't is the file's own.
    #[test]
    fn a_font_finding_is_carried_whatever_its_count() {
        let report = |names: &str, widths: &str| {
            let options = crate::check::Options {
                paths: true,
                ..crate::check::Options::default()
            };
            let font = calibri(
                "TrueType",
                &format!("{widths} /Encoding << /Differences [300 {names}] >>"),
            );
            let file = fonts("1.7", "/F1 4 0 R", &[(4, &font), DESCRIPTOR]);
            report_with(&file, &ALL, &options)
        };
        fn values(found: Vec<&Finding>) -> Vec<&str> {
            (found.into_iter())
                .filter(|f| f.rule == Rule::Font)
                .map(|f| f.value.as_str())
                .collect()
        }
        let differences = Fault::Differences.to_string();
        let none = Fault::NoWidths.to_string();
        let before = report("/A /B", "");
        let after = report("/A", WIDTHS);
        let twice = format!("{differences}, 2 times");
        assert_eq!(
            values(before.findings.iter().collect()),
            [twice.as_str(), &none],
            "{before}"
        );
        assert_eq!(
            values(after.findings.iter().collect()),
            [differences.as_str()],
            "{after}"
        );
        assert_eq!(values(after.since(&before)), Vec::<&str>::new());
        assert_eq!(values(after.by_path(&before)), Vec::<&str>::new());
        assert_eq!(values(after.beyond(&[&before])), Vec::<&str>::new());
        assert_eq!(values(before.since(&after)), [none.as_str()]);
        assert_eq!(values(before.by_path(&after)), [none.as_str()]);
        assert_eq!(values(before.beyond(&[&after])), [none.as_str()]);
    }

    /// A font read before the walk, as its descriptor lacks `/CapHeight`,
    /// isn't read again after it: its streams decode within the budget
    /// once, its `/ToUnicode` giving a code it draws a Latin letter, and
    /// what its read tells is found once.
    #[test]
    fn a_font_read_before_the_walk_isn_t_read_again() {
        let descriptor = DESCRIPTOR.1.replace("/CapHeight 700 ", "");
        let font = calibri(
            "TrueType",
            "/FirstChar 32 /LastChar 34 /Widths [500 500] /Encoding /WinAnsiEncoding \
             /ToUnicode 6 0 R",
        );
        let to_unicode = stream("", "1 beginbfchar <20> <0041> endbfchar");
        let file = fonts(
            "1.7",
            "/F1 4 0 R",
            &[(4, &font), (5, &descriptor), (6, &to_unicode)],
        );
        let options = crate::check::Options {
            font_budget: Some(36),
            ..crate::check::Options::default()
        };
        let checked = report_with(&file, &ALL, &options);
        assert_eq!(unread_fonts(&checked), [], "{checked}");
        assert_eq!(cap_height(&checked), missing(5, "FontDescriptorTrueType"));
        assert_eq!(
            found_in(&file, &checked),
            [told(
                4,
                "F1",
                "FontTrueType/Widths",
                Fault::WidthsLength,
                "[500 500]"
            )]
        );
    }
}
