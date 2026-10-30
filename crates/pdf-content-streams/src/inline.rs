//! Where an inline image's data ends, as PDFium's `ReadInlineStream`
//! finds it, from its dictionary as PDFium reads it and the components of
//! the color space it's drawn in ([`ResCx`]); and what an image read says
//! ([`InlineImage`]).

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::ops::Range;

use crate::error::{Error, ErrorKind};
use crate::fault::{self, Fault, Read, Sink};
use crate::limits::Limits;
use crate::operand::{Operand, hex, literal, name};
use crate::scan::{
    Raw, UNCAPPED, a85_end, ei_scan, elements, find, int, is_number, is_space, next_raw,
    run_length_end,
};

/// What reading content needs of the resources it's drawn with: how many
/// components each named color space has, which sizes an unfiltered
/// inline image drawn in one ([`tokens`](crate::tokens)).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResCx {
    spaces: HashMap<Vec<u8>, u32>,
}

impl ResCx {
    /// No resources: what an annotation's `/DA` is read with.
    pub fn none() -> ResCx {
        ResCx::default()
    }

    /// Color spaces by name, each with its components: each name as a
    /// `/ColorSpace` dictionary's key, its `#` escapes decoded. PDFium
    /// counts a space named by a family alone as [`family_components`]
    /// does, an array by its first element the same, but an ICCBased
    /// space by its stream's `/N` (1, 3 or 4, and 3 otherwise) and a
    /// DeviceN space by its colorants' names (3 when there are none), and
    /// 3 for what it can't make out.
    pub fn new(spaces: impl IntoIterator<Item = (Vec<u8>, u32)>) -> ResCx {
        ResCx {
            spaces: spaces.into_iter().collect(),
        }
    }
}

/// The components a color space family has with no more said: what a
/// name in the resources, or an array's first element, gives. PDFium
/// gives what it can't load 3.
pub fn family_components(family: &[u8]) -> u32 {
    match family {
        b"G" | b"DeviceGray" | b"CalGray" | b"I" | b"Indexed" | b"Separation" | b"Pattern" => 1,
        b"CMYK" | b"DeviceCMYK" => 4,
        _ => 3,
    }
}

/// An inline image (ISO 32000-2, 8.9.7), `BI` to `EI`, one operation
/// ([`Operation::inline_image`](crate::Operation::inline_image)): where
/// it is, its dictionary and its data, as written.
///
/// ```
/// use pdf_content_streams::{Limits, ResCx, operations};
///
/// let content = b"q BI /W 4 /H 1 /BPC 8 /CS /G /L 4 ID abcd EI Q";
/// let res = ResCx::none();
/// let mut read = operations(content, &res, &Limits::DEFAULT);
/// read.next();
/// let image = read.next().unwrap().unwrap().inline_image().unwrap();
/// assert_eq!((image.at(), image.data()), (2..44, 37..41));
/// assert_eq!(&content[image.dict()], b" /W 4 /H 1 /BPC 8 /CS /G /L 4 ");
/// let space = image.entry(b"ColorSpace").unwrap();
/// assert_eq!(space.name().as_deref(), Some(&b"G"[..]));
/// assert_eq!(image.length(), Some(4));
/// ```
#[derive(Clone)]
pub struct InlineImage<'a> {
    content: &'a [u8],
    at: Range<usize>,
    data: Range<usize>,
}

impl<'a> InlineImage<'a> {
    /// The image at `at` in `content`, `BI` to `EI`, its data at `data`.
    pub(crate) fn new(content: &'a [u8], at: Range<usize>, data: Range<usize>) -> InlineImage<'a> {
        InlineImage { content, at, data }
    }

    /// Where it is, `BI` to `EI`.
    pub fn at(&self) -> Range<usize> {
        self.at.clone()
    }

    /// Where its dictionary is: what's between `BI` and `ID`.
    pub fn dict(&self) -> Range<usize> {
        // `ID` ends at its data, or at the one white-space byte before it.
        let before = self.content[self.data.start - 1];
        let id = self.data.start - usize::from(is_space(before)) - 2;
        self.at.start + 2..id
    }

    /// Where its data is, as [`tokens`](crate::tokens) found it.
    pub fn data(&self) -> Range<usize> {
        self.data.clone()
    }

    /// The value under `key`, as written, as PDFium reads the image's
    /// dictionary: `key` a name's, without its `/`, matched as written,
    /// its `#` escapes not decoded. Of a key 8.9.7 abbreviates (Table
    /// 91), either spelling finds the value: where both are written, the
    /// abbreviation's, unless the full name's is an abbreviated name
    /// (Table 92, `/RGB` say), which PDFium sets over it; so
    /// `entry(b"CS")` and `entry(b"ColorSpace")` are the same. Of a key
    /// written twice, the last.
    pub fn entry(&self, key: &[u8]) -> Option<Operand<'a>> {
        let pair = KEYS
            .iter()
            .position(|&(full, short, _)| key == full.as_bytes() || key == short.as_bytes());
        let mut dict = ImageDict::default();
        let mut found = None;
        // It was read under a cap on depth, so nothing in it is deeper.
        image_dict(
            &self.content[..self.at.end],
            self.at.start,
            UNCAPPED,
            |k, value| match pair {
                Some(_) => dict.set(k, value),
                None if &k[1..] == key => found = Some(value),
                None => {}
            },
            &mut (),
        )
        .ok()?;
        let found = match pair {
            Some(i) => dict.get(self.content, i),
            None => found,
        };
        found.map(|r| Operand::new(self.content, r))
    }

    /// The length it gives its data, its `/L` or `/Length` (PDF 2.0),
    /// where that's a length: a non-negative number under 2^32, a real's
    /// whole part. Where both are written, `/L`'s, as 8.9.7 has it; PDFium
    /// reads neither, and finds the data otherwise.
    pub fn length(&self) -> Option<u64> {
        self.entry(b"L").and_then(|l| int(l.bytes()))
    }
}

impl fmt::Debug for InlineImage<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("InlineImage")
            .field("at", &self.at)
            .field("dict", &self.dict())
            .field("data", &self.data)
            .finish()
    }
}

/// The inline images of one content stream: what finds each one's data,
/// and the last search for a JPEG's end of image.
#[derive(Debug, Clone)]
pub(crate) struct Images<'a, 'r> {
    s: &'a [u8],
    res: &'r ResCx,
    lim: Limits,
    /// The last search for a JPEG's end of image: from where, and where
    /// it was. Content of many JPEG images that don't end is searched
    /// once, not once per image.
    eoi: Option<(usize, Option<usize>)>,
}

impl<'a, 'r> Images<'a, 'r> {
    pub(crate) fn new(s: &'a [u8], res: &'r ResCx, lim: Limits) -> Images<'a, 'r> {
        Images {
            s,
            res,
            lim,
            eoi: None,
        }
    }

    /// The inline image whose `BI` is at `bi`: where it ends, past its
    /// `EI`, and where its data is. Where ISO 32000-2 would read it
    /// otherwise, `sink` is told, if it wants: its dictionary's tokens'
    /// own faults and its entries', then its keys', its `ID`'s, its
    /// length's and what's before its `EI`.
    pub(crate) fn read(
        &mut self,
        bi: usize,
        sink: &mut impl Sink<'a>,
    ) -> Result<(usize, Range<usize>), Error> {
        let s = self.s;
        let unterminated = Error {
            at: bi,
            kind: ErrorKind::InlineImage,
        };
        let mut dict = ImageDict::default();
        let set = |key: &[u8], value| dict.set(key, value);
        let at = image_dict(s, bi, self.lim.depth, set, sink)?;
        // One white-space byte after `ID`, then the data.
        let start = at + usize::from(s.get(at).is_some_and(|&c| is_space(c)));
        // A decoder that finds no end of its data is none to PDFium, which
        // then reads on from `ID` as it does for a filter it can't decode.
        let end = match self.first_filter(&dict) {
            None => self
                .unfiltered(&dict)
                .map_or(End::Walk, |n| End::Counted(start.saturating_add(n))),
            Some(Filter::Hex) => find(s, start, b">").map_or(End::Walk, |at| End::Counted(at + 1)),
            Some(Filter::A85) => a85_end(s, start).map_or(End::Walk, End::Counted),
            Some(Filter::RunLength) => run_length_end(s, start).map_or(End::Walk, End::Counted),
            Some(Filter::Dct) => self
                .eoi(start)
                .map_or(End::Scan, |eoi| End::Counted(eoi + 2)),
            Some(Filter::Decoded) => End::Scan,
            Some(Filter::Other) => End::Walk,
        };
        let (data, end) = match end {
            End::Counted(end) if end <= s.len() => {
                (start..end, self.ei_from(end).ok_or(unterminated)?.end)
            }
            End::Counted(_) => return Err(unterminated),
            End::Scan => {
                let ei = ei_scan(s, start).ok_or(unterminated)?;
                (start..(ei - 1).max(start), ei + 2)
            }
            End::Walk => {
                let ei = self.ei_from(start).ok_or(unterminated)?;
                // Its data to its `EI`, the white-space byte before it left
                // out.
                let before = ei.start > start && is_space(s[ei.start - 1]);
                (start..ei.start - usize::from(before), ei.end)
            }
        };
        if data.len() > self.lim.inline_image {
            return Err(Error {
                at: bi,
                kind: ErrorKind::InlineImageSize,
            });
        }
        if sink.wants() {
            self.faults(&dict, bi + 2..at - 2, &data, end, sink);
        }
        Ok((end, data))
    }

    /// Where ISO 32000-2 reads an image read otherwise, told to `sink`:
    /// a key both abbreviated and in full whose value ISO reads otherwise,
    /// its dictionary at `between`, from after `BI` to `ID`; its `ID`,
    /// ending two bytes after, with no white-space after it; a length that
    /// ends its data elsewhere than `data`; tokens between its data and its
    /// `EI`, which ends at `end`.
    fn faults(
        &self,
        dict: &ImageDict,
        between: Range<usize>,
        data: &Range<usize>,
        end: usize,
        sink: &mut impl Sink<'a>,
    ) {
        let s = self.s;
        let id = between.end + 2;
        for key in dict.parted(s) {
            let written = &s[between.clone()];
            sink.fault(between.clone(), Fault::Abbreviation { written, key });
        }
        // ISO asks white-space after `ID`, and counts its length from after
        // more of it, by the last of its filters (8.9.7).
        let ascii = self
            .last_filter(dict)
            .is_some_and(|f| matches!(&*f, b"AHx" | b"ASCIIHexDecode" | b"A85" | b"ASCII85Decode"));
        if !ascii && !s.get(id).is_some_and(|&c| is_space(c)) {
            sink.fault(id - 2..id, Fault::AfterId);
        }
        if let Some(given) = dict.iso(L).and_then(|l| int(&s[l])) {
            let mut start = data.start;
            if ascii {
                while s.get(start).is_some_and(|&c| is_space(c)) && start < data.end {
                    start += 1;
                }
            }
            let ends = usize::try_from(given)
                .ok()
                .and_then(|n| start.checked_add(n));
            if ends != Some(data.end) {
                let read = data.end - start;
                sink.fault(data.clone(), Fault::ImageLength { given, read });
            }
        }
        // What's between the data and `EI`, the first token to the last.
        let mut between: Option<Range<usize>> = None;
        let mut at = data.end;
        while let Some(Ok((r, _))) = next_raw(s, at, self.lim.depth) {
            if r.end == end && &s[r.clone()] == b"EI" {
                break;
            }
            at = r.end;
            between = Some(between.map_or(r.start, |b| b.start)..r.end);
        }
        if let Some(between) = between {
            let written = &s[between.clone()];
            sink.fault(between, Fault::BeforeEi { written });
        }
    }

    /// The last of an image's filters, or its one, as ISO 32000-2 reads
    /// its dictionary: a name's, decoded.
    fn last_filter(&self, dict: &ImageDict) -> Option<Cow<'a, [u8]>> {
        let f = &self.s[dict.iso(F)?];
        let last = match f.first() {
            Some(b'[') => *elements(f, self.lim.depth).last()?,
            _ => f,
        };
        name(last)
    }

    /// An image's first filter, by what finds its data's end, as PDFium
    /// takes it: a name, its abbreviation read as the full name (Table
    /// 92), or a string's bytes, naming a filter. None, as for no filter,
    /// for `null`, a dictionary or what's no object, and for an empty
    /// array; [`Filter::Other`] for a name or a string that names no filter
    /// PDFium decodes, a number, a boolean, or an array first in an array.
    fn first_filter(&self, dict: &ImageDict) -> Option<Filter> {
        let s = self.s;
        let f = &s[dict.get(s, F)?];
        let first = match f.first() {
            Some(b'[') => *elements(f, self.lim.depth).first()?,
            _ => f,
        };
        let named: Cow<[u8]> = match first {
            [b'/', ..] => {
                let n = name(first)?;
                expanded(&n).map_or(n, Cow::Borrowed)
            }
            [b'(', ..] => Cow::Owned(literal(first)),
            [b'<', b'<', ..] => return None,
            [b'<', ..] => Cow::Owned(hex(first)),
            [b'[', ..] | b"true" | b"false" => return Some(Filter::Other),
            n if is_number(n) => return Some(Filter::Other),
            _ => return None,
        };
        Some(match &*named {
            b"ASCIIHexDecode" => Filter::Hex,
            b"ASCII85Decode" => Filter::A85,
            b"RunLengthDecode" => Filter::RunLength,
            b"DCTDecode" => Filter::Dct,
            b"FlateDecode" | b"LZWDecode" | b"CCITTFaxDecode" => Filter::Decoded,
            _ => Filter::Other,
        })
    }

    /// The size of an unfiltered image's data, as PDFium counts it; None
    /// when it counts none.
    fn unfiltered(&self, dict: &ImageDict) -> Option<usize> {
        let s = self.s;
        let value = |key| int(&s[dict.get(s, key)?]);
        let (components, bpc) = match dict.get(s, CS) {
            None => (1, 1),
            Some(cs) => match self.components(&s[cs]) {
                Some(n) => (n, value(BPC)?),
                None => (1, 1),
            },
        };
        let pitch = value(W)?
            .checked_mul(bpc)?
            .checked_mul(components)?
            .div_ceil(8);
        let size = pitch.checked_mul(value(H)?)?;
        usize::try_from(size).ok().filter(|&n| n > 0)
    }

    /// The components of an inline image's color space, as written; None
    /// for a name the resources don't have, which PDFium reads as no
    /// color space, an abbreviation looked up by its full name (`/I` as
    /// `/Indexed`). What it can't make out has 3, as in PDFium.
    fn components(&self, cs: &[u8]) -> Option<u64> {
        if let Some(n) = name(cs) {
            return match expanded(&n).unwrap_or(&n) {
                b"DeviceGray" => Some(1),
                b"DeviceRGB" => Some(3),
                b"DeviceCMYK" => Some(4),
                n => self.res.spaces.get(n).map(|&c| c.into()),
            };
        }
        if cs.first() != Some(&b'[') {
            return Some(3);
        }
        let parts = elements(cs, self.lim.depth);
        let family = parts.first().and_then(|f| name(f));
        Some(match family.as_deref() {
            Some(b"DeviceN") => parts
                .get(1)
                .map(|names| elements(names, self.lim.depth).len() as u64)
                .filter(|&n| n > 0)
                .unwrap_or(3),
            Some(family) => family_components(family).into(),
            None => 3,
        })
    }

    /// The next `EI` operator from `at`, its tokens read as PDFium reads
    /// them, a string or an array one token: where it is.
    fn ei_from(&self, mut at: usize) -> Option<Range<usize>> {
        loop {
            let (r, kind) = next_raw(self.s, at, self.lim.depth)?.ok()?;
            if matches!(kind, Raw::Op) && &self.s[r.clone()] == b"EI" {
                return Some(r);
            }
            at = r.end;
        }
    }

    /// Where the next JPEG end of image (`FF D9`) from `at` is.
    fn eoi(&mut self, at: usize) -> Option<usize> {
        if let Some((from, found)) = self.eoi {
            if at >= from && found.is_none_or(|f| f >= at) {
                return found;
            }
        }
        let found = find(self.s, at, b"\xff\xd9");
        self.eoi = Some((at, found));
        found
    }
}

/// The keys of an inline image's dictionary that 8.9.7 abbreviates (Table
/// 91): each full name, its abbreviation, and whether PDFium reads the
/// abbreviation as the key. `Length` (PDF 2.0) PDFium neither abbreviates
/// nor reads.
const KEYS: [(&str, &str, bool); 10] = [
    ("BitsPerComponent", "BPC", true),
    ("ColorSpace", "CS", true),
    ("Decode", "D", true),
    ("DecodeParms", "DP", true),
    ("Filter", "F", true),
    ("Height", "H", true),
    ("ImageMask", "IM", true),
    ("Interpolate", "I", true),
    ("Length", "L", false),
    ("Width", "W", true),
];

/// The keys among [`KEYS`] whose values find an image's data's end.
const BPC: usize = 0;
const CS: usize = 1;
const F: usize = 4;
const H: usize = 5;
const L: usize = 8;
const W: usize = 9;

/// The full name an abbreviated name stands for in an inline image
/// (8.9.7, Table 92), `name` decoded; None for any other.
pub(crate) fn expanded(name: &[u8]) -> Option<&'static [u8]> {
    Some(match name {
        b"G" => b"DeviceGray",
        b"RGB" => b"DeviceRGB",
        b"CMYK" => b"DeviceCMYK",
        b"I" => b"Indexed",
        b"AHx" => b"ASCIIHexDecode",
        b"A85" => b"ASCII85Decode",
        b"LZW" => b"LZWDecode",
        b"Fl" => b"FlateDecode",
        b"RL" => b"RunLengthDecode",
        b"CCF" => b"CCITTFaxDecode",
        b"DCT" => b"DCTDecode",
        _ => return None,
    })
}

/// Whether an inline image's key, decoded, is one 8.9.7 has it read
/// (Table 91): a key of [`KEYS`], either spelling, or `Intent`.
fn image_key(key: &[u8]) -> bool {
    key == b"Intent"
        || (KEYS.iter()).any(|&(full, short, _)| key == full.as_bytes() || key == short.as_bytes())
}

/// What an inline image's dictionary says that finds its data's end:
/// where the value of each key of [`KEYS`] is, under its full name and
/// under its abbreviation apart, the last of each. A key is matched as
/// written, its `#` escapes not decoded, as PDFium matches it.
#[derive(Default)]
struct ImageDict {
    /// By [`KEYS`]' order: the full name's value, then the abbreviation's.
    values: [[Option<Range<usize>>; 2]; KEYS.len()],
}

impl ImageDict {
    /// `key`, a name as written, its `/` first, has `value`.
    fn set(&mut self, key: &[u8], value: Range<usize>) {
        let key = &key[1..];
        for (i, &(full, short, _)) in KEYS.iter().enumerate() {
            if key == full.as_bytes() {
                self.values[i][0] = Some(value);
                return;
            }
            if key == short.as_bytes() {
                self.values[i][1] = Some(value);
                return;
            }
        }
    }

    /// Where the value of key `i` of [`KEYS`] is as PDFium reads it, `s`
    /// what the dictionary is in: the abbreviation's, unless both are
    /// written and the full name's is an abbreviated name, which PDFium
    /// sets over it (its `ReplaceAbbr`). `Length`, which PDFium doesn't
    /// read, as [`iso`](ImageDict::iso).
    fn get(&self, s: &[u8], i: usize) -> Option<Range<usize>> {
        let [full, short] = self.values[i].clone();
        match (full, short) {
            (Some(full), Some(_)) if KEYS[i].2 && abbreviated(&s[full.clone()]) => Some(full),
            (full, short) => short.or(full),
        }
    }

    /// Where the value of key `i` of [`KEYS`] is as ISO 32000-2 reads it:
    /// the abbreviation's, where both are written (8.9.7, as its errata
    /// add).
    fn iso(&self, i: usize) -> Option<Range<usize>> {
        let [full, short] = self.values[i].clone();
        short.or(full)
    }

    /// The keys PDFium reads otherwise than ISO 32000-2, `s` what the
    /// dictionary is in: both spellings written, and the full name's value
    /// an abbreviated name that means another than the abbreviation's.
    fn parted<'s>(&'s self, s: &'s [u8]) -> impl Iterator<Item = &'static str> + 's {
        (0..KEYS.len()).filter_map(move |i| {
            let [Some(full), Some(short)] = self.values[i].clone() else {
                return None;
            };
            fn meaning(v: &[u8]) -> Cow<'_, [u8]> {
                match name(v) {
                    Some(n) => expanded(&n).map_or(n, Cow::Borrowed),
                    None => Cow::Borrowed(v),
                }
            }
            let parted =
                self.get(s, i) == Some(full.clone()) && meaning(&s[full]) != meaning(&s[short]);
            parted.then_some(KEYS[i].0)
        })
    }
}

/// Whether a value, as written, is an abbreviated name (8.9.7, Table 92).
fn abbreviated(value: &[u8]) -> bool {
    name(value).is_some_and(|n| expanded(&n).is_some())
}

/// The dictionary of the inline image whose `BI` is at `bi`, to its `ID`:
/// `entry` is given each key, as written, and where its value is. What
/// isn't a name where a key should be is passed over. Returns where `ID`
/// ends. Where ISO 32000-2 would read it otherwise, `sink` is told, if it
/// wants: each token's own faults, a key that's an object but no name, a
/// word that's no object, and a key with no value.
fn image_dict<'a>(
    s: &'a [u8],
    bi: usize,
    depth: usize,
    mut entry: impl FnMut(&[u8], Range<usize>),
    sink: &mut (impl Sink<'a> + ?Sized),
) -> Result<usize, Error> {
    let unterminated = Error {
        at: bi,
        kind: ErrorKind::InlineImage,
    };
    let mut key = None;
    let mut at = bi + 2;
    loop {
        let (r, kind) = next_raw(s, at, depth).ok_or(unterminated)??;
        at = r.end;
        let is_id = matches!(kind, Raw::Op) && &s[r.clone()] == b"ID";
        if sink.wants() {
            entry_faults(s, r.clone(), kind, is_id, key.clone(), sink);
        }
        if is_id {
            return Ok(at);
        }
        match key.take() {
            Some(key) => entry(&s[key], r),
            None if s[r.start] == b'/' => key = Some(r),
            None => {}
        }
    }
}

/// An inline image's dictionary's token at `r`, `ID` or not, after `key`,
/// a key waiting for its value: where ISO 32000-2 would read it
/// otherwise, told to `sink`.
fn entry_faults<'a>(
    s: &'a [u8],
    r: Range<usize>,
    kind: Raw,
    is_id: bool,
    key: Option<Range<usize>>,
    sink: &mut (impl Sink<'a> + ?Sized),
) {
    let written = &s[r.clone()];
    if is_id {
        if let Some(key) = key {
            let written = &s[key.clone()];
            sink.fault(key, Fault::NoValue { written });
        }
        return;
    }
    let read = match kind {
        Raw::Op => Read::Keyword,
        Raw::Operand => fault::token(written, r.start, sink),
    };
    match read {
        Read::Keyword => sink.fault(r, Fault::Keyword { written }),
        Read::Object if key.is_none() && written[0] != b'/' => {
            sink.fault(r, Fault::Key { written });
        }
        // A key ISO reads by its escapes decoded, which PDFium matches as
        // written.
        Read::Object
            if key.is_none()
                && written.contains(&b'#')
                && name(written).is_some_and(|k| *k != written[1..] && image_key(&k)) =>
        {
            sink.fault(r, Fault::EscapedKey { written });
        }
        _ => {}
    }
}

/// An inline image's first filter, by what finds its data's end.
enum Filter {
    Hex,
    A85,
    RunLength,
    Dct,
    /// Flate, LZW or CCITTFax: what PDFium finds the end of by decoding.
    Decoded,
    /// One PDFium has no decoder for.
    Other,
}

/// How an inline image's data ends.
enum End {
    /// Where it's counted to, by its size or its filter.
    Counted(usize),
    /// At the first `EI` with white-space before it and white-space, a
    /// delimiter or the end after it.
    Scan,
    /// At the next `EI` operator, its tokens read as PDFium reads them.
    Walk,
}
