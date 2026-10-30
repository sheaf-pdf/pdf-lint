//! Where an inline image's data ends, as PDFium's `ReadInlineStream`
//! finds it, from its dictionary as PDFium reads it and the components of
//! the color space it's drawn in ([`ResCx`]); and what an image read says
//! ([`InlineImage`]).

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::ops::Range;

use pdf_tokenizer::{decode, is_white_space};

use crate::data::{a85_end, ei_scan, find, run_length_end};
use crate::error::{Error, ErrorKind};
use crate::fault::{self, Fault, Sink};
use crate::limits::Limits;
use crate::operand::{Operand, hex, literal, name};
use crate::scan::{Raw, UNCAPPED, elements, is_number, is_space, next_raw};

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

    /// Where its dictionary is: what's between `BI` and `ID`, or, where
    /// an object that's no name is written for a key, `BI` and that
    /// object, which ends it as PDFium reads it ([`tokens`](crate::tokens)).
    pub fn dict(&self) -> Range<usize> {
        let start = self.at.start + 2;
        // It was read under a cap on depth, so nothing in it is deeper;
        // read again, it ends where it ended, at `ID` or an object, never
        // at a keyword or an error.
        let s = &self.content[..self.at.end];
        match image_dict(s, self.at.start, UNCAPPED, |_, _| {}, &mut ()) {
            Ok(Ends::Id(end) | Ends::Object(end)) => start..end.start,
            Ok(Ends::Keyword(_)) | Err(_) => start..start,
        }
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
    /// written twice, the last; and a value PDFium reads as no object (a
    /// keyword, a closer or a brace) takes the key out, as PDFium takes
    /// out a key it reads no value for.
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
                None if &k[1..] == key => found = value,
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

    /// The inline image whose `BI` is at `bi`, or none, where a keyword
    /// is written for its dictionary's key ([`Image`]). Where ISO 32000-2
    /// would read an image otherwise, `sink` is told, if it wants: its
    /// dictionary's tokens' own faults and its entries', as they're read;
    /// then, the image read, its keys', what ends its dictionary, its
    /// `ID`'s, its length's and what's before its `EI`.
    pub(crate) fn read(&mut self, bi: usize, sink: &mut impl Sink<'a>) -> Result<Image, Error> {
        let s = self.s;
        let unterminated = Error {
            at: bi,
            kind: ErrorKind::InlineImage,
        };
        let mut dict = ImageDict::default();
        let set = |key: &[u8], value| dict.set(key, value);
        let read = image_dict(s, bi, self.lim.depth, set, &mut ());
        // An image's dictionary, or one that doesn't end, read again for
        // its tokens' faults, told as they're read, before what's after
        // them; not a `BI`'s that begins none, whose tokens are content.
        if sink.wants() && !matches!(read, Ok(Ends::Keyword(_))) {
            image_dict(s, bi, self.lim.depth, |_, _| {}, sink)?;
        }
        let ends = read?;
        let (between, at) = match &ends {
            Ends::Id(end) | Ends::Object(end) => (bi + 2..end.start, end.end),
            Ends::Keyword(keyword) => return Ok(Image::Lone(keyword.clone())),
        };
        // One white-space byte after `ID`, or the object that ends the
        // dictionary, by PDFium's rules, then the data.
        let start = at + usize::from(s.get(at).is_some_and(|&c| is_space(c)));
        // A decoder that finds no end of its data is none to PDFium, which
        // then reads on from its data's start as it does for a filter it
        // can't decode.
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
                // Its data to its `EI`, the white-space byte before it, by
                // PDFium's rules, left out.
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
            self.faults(&dict, between, &ends, &data, end, sink);
        }
        Ok(Image::Read(end, data))
    }

    /// Where ISO 32000-2 reads an image read otherwise, told to `sink`:
    /// a key both abbreviated and in full whose value ISO reads otherwise,
    /// its dictionary at `between`, from after `BI` to what `ends` it; the
    /// object that ends it, where one does, its own faults and then that
    /// it ends it; its `ID`, where that ends it, with no white-space after
    /// it; a length that ends its data elsewhere than `data`; tokens
    /// between its data and its `EI`, which ends at `end`.
    fn faults(
        &self,
        dict: &ImageDict,
        between: Range<usize>,
        ends: &Ends,
        data: &Range<usize>,
        end: usize,
        sink: &mut impl Sink<'a>,
    ) {
        let s = self.s;
        for key in dict.parted(s) {
            let written = &s[between.clone()];
            sink.fault(between.clone(), Fault::Abbreviation { written, key });
        }
        // ISO asks white-space after `ID`, and counts its length from after
        // more of it, by the last of its filters (8.9.7): its white-space
        // (7.2.3).
        let ascii = self
            .last_filter(dict)
            .is_some_and(|f| matches!(&*f, b"AHx" | b"ASCIIHexDecode" | b"A85" | b"ASCII85Decode"));
        match ends {
            Ends::Object(object) => {
                let written = &s[object.clone()];
                fault::token(written, object.start, sink);
                sink.fault(object.clone(), Fault::DictEnd { written });
            }
            Ends::Id(id) if !ascii && !s.get(id.end).is_some_and(|&c| is_white_space(c)) => {
                sink.fault(id.clone(), Fault::AfterId);
            }
            Ends::Id(_) | Ends::Keyword(_) => {}
        }
        if let Some(given) = dict.iso(L).and_then(|l| int(&s[l])) {
            let mut start = data.start;
            if ascii {
                while s.get(start).is_some_and(|&c| is_white_space(c)) && start < data.end {
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
    /// `key`, a name as written, its `/` first, has `value`, or is taken
    /// out where it has none.
    fn set(&mut self, key: &[u8], value: Option<Range<usize>>) {
        let key = &key[1..];
        for (i, &(full, short, _)) in KEYS.iter().enumerate() {
            if key == full.as_bytes() {
                self.values[i][0] = value;
                return;
            }
            if key == short.as_bytes() {
                self.values[i][1] = value;
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

/// How an inline image's dictionary ends, as PDFium's `Handle_BeginImage`
/// reads it: by what's written where its next key should be, and where
/// that is.
pub(crate) enum Ends {
    /// `ID`: the data is after it.
    Id(Range<usize>),
    /// An object that's no name (a number, a string, an array, a
    /// dictionary, `true`, `false`, `null`, a closer or a brace): the data
    /// is after it, as after `ID`.
    Object(Range<usize>),
    /// A keyword other than `ID`: there's no image, and `BI` is an
    /// operator of its own.
    Keyword(Range<usize>),
}

/// What reading an inline image's `BI` gives ([`Images::read`]).
pub(crate) enum Image {
    /// An image: where it ends, past its `EI`, and where its data is.
    Read(usize, Range<usize>),
    /// No image, a keyword where its dictionary's key is written: where
    /// that keyword is.
    Lone(Range<usize>),
}

/// The dictionary of the inline image whose `BI` is at `bi`, as PDFium's
/// `Handle_BeginImage` reads it, and what ends it ([`Ends`]): a name, its
/// value, and so on, until what's written for a key isn't a name. `entry`
/// is given each key, as written, and where its value is, or none for a
/// value PDFium reads as no object (a keyword, `ID` among them, a closer
/// or a brace), which takes the key out. Where ISO 32000-2 would read it
/// otherwise, `sink` is told, if it wants: each key's and value's own
/// faults, a key with a `#` escape ISO reads as an image's, a word that's
/// no object as a value, and a key whose value is `ID`, which ends ISO's
/// dictionary; not what ends it, which is the image's to tell
/// ([`Images::read`]).
fn image_dict<'a>(
    s: &'a [u8],
    bi: usize,
    depth: usize,
    mut entry: impl FnMut(&[u8], Option<Range<usize>>),
    sink: &mut (impl Sink<'a> + ?Sized),
) -> Result<Ends, Error> {
    let unterminated = Error {
        at: bi,
        kind: ErrorKind::InlineImage,
    };
    let mut at = bi + 2;
    loop {
        let (key, kind) = next_raw(s, at, depth).ok_or(unterminated)??;
        match kind {
            Raw::Op if &s[key.clone()] == b"ID" => return Ok(Ends::Id(key)),
            Raw::Op => return Ok(Ends::Keyword(key)),
            Raw::Operand if s[key.start] != b'/' => return Ok(Ends::Object(key)),
            Raw::Operand => {}
        }
        if sink.wants() {
            key_faults(s, key.clone(), sink);
        }
        let (value, kind) = next_raw(s, key.end, depth).ok_or(unterminated)??;
        at = value.end;
        let written = &s[value.clone()];
        if sink.wants() {
            match kind {
                Raw::Op if written == b"ID" => {
                    let written = &s[key.clone()];
                    sink.fault(key.clone(), Fault::NoValue { written });
                }
                Raw::Op => sink.fault(value.clone(), Fault::Keyword { written }),
                Raw::Operand => {
                    fault::token(written, value.start, sink);
                }
            }
        }
        let none =
            matches!(kind, Raw::Op) || matches!(written, b")" | b">" | b"]" | b">>" | b"{" | b"}");
        entry(&s[key], (!none).then_some(value));
    }
}

/// An inline image's key at `r`, a name: where ISO 32000-2 would read it
/// otherwise, told to `sink`.
fn key_faults<'a>(s: &'a [u8], r: Range<usize>, sink: &mut (impl Sink<'a> + ?Sized)) {
    let written = &s[r.clone()];
    fault::token(written, r.start, sink);
    // A key ISO reads by its escapes decoded, which PDFium matches as
    // written.
    if written.contains(&b'#') && name(written).is_some_and(|k| *k != written[1..] && image_key(&k))
    {
        sink.fault(r, Fault::EscapedKey { written });
    }
}

/// A non-negative number, as PDFium reads an integer from one: a real's
/// whole part.
fn int(number: &[u8]) -> Option<u64> {
    let value = decode::number(number).ok()?;
    (0.0..4_294_967_296.0)
        .contains(&value)
        .then_some(value as u64)
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
