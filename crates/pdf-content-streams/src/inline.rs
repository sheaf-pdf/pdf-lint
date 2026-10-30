//! Where an inline image's data ends, as PDFium's `ReadInlineStream`
//! finds it, from its dictionary and the components of the color space
//! it's drawn in ([`ResCx`]); and what an image read says
//! ([`InlineImage`]).

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;

use crate::error::{Error, ErrorKind};
use crate::limits::Limits;
use crate::operand::{Operand, name};
use crate::scan::{
    Raw, UNCAPPED, ei_scan, elements, find, int, is_space, next_raw, run_length_end,
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
/// let space = image.entry(&[b"CS", b"ColorSpace"]).unwrap();
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

    /// The value under one of `keys`, as written: the last such entry's,
    /// as PDFium's dictionary keeps the last. Each key is a name's,
    /// without its `/`, matched with its `#` escapes decoded; an image's
    /// keys may be abbreviated (8.9.7), so `&[b"CS", b"ColorSpace"]` finds
    /// its color space either way.
    pub fn entry(&self, keys: &[&[u8]]) -> Option<Operand<'a>> {
        let mut found = None;
        // It was read under a cap on depth, so nothing in it is deeper.
        image_dict(
            &self.content[..self.at.end],
            self.at.start,
            UNCAPPED,
            |key, value| {
                if name(key).is_some_and(|k| keys.contains(&&*k)) {
                    found = Some(value);
                }
            },
        )
        .ok()?;
        found.map(|r| Operand::new(self.content, r))
    }

    /// The length it gives its data, its `/L` or `/Length`, where that's a
    /// length, as its data's end is found from it: a non-negative number
    /// under 2^32, a real's whole part.
    pub fn length(&self) -> Option<u64> {
        self.entry(&[b"L", b"Length"]).and_then(|l| int(l.bytes()))
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
    /// `EI`, and where its data is.
    pub(crate) fn read(&mut self, bi: usize) -> Result<(usize, Range<usize>), Error> {
        let s = self.s;
        let unterminated = Error {
            at: bi,
            kind: ErrorKind::InlineImage,
        };
        let mut dict = ImageDict::default();
        let at = image_dict(s, bi, self.lim.depth, |key, value| dict.set(key, value))?;
        // One white-space byte after `ID`, then the data.
        let start = at + usize::from(s.get(at).is_some_and(|&c| is_space(c)));
        let counted = match self.first_filter(&dict) {
            None => self.unfiltered(&dict).map(|n| start.saturating_add(n)),
            Some(Filter::Hex) => Some(find(s, start, b">").ok_or(unterminated)? + 1),
            Some(Filter::A85) => Some(find(s, start, b"~>").ok_or(unterminated)? + 2),
            Some(Filter::RunLength) => Some(run_length_end(s, start).ok_or(unterminated)?),
            Some(Filter::Dct) => self.eoi(start).map(|eoi| eoi + 2),
            Some(Filter::Other) => None,
        }
        .or_else(|| {
            let length = dict.l.and_then(|l| int(&s[l]))?;
            start
                .checked_add(usize::try_from(length).ok()?)
                .filter(|&end| end <= s.len())
        });
        let (data, end) = match counted {
            Some(end) if end <= s.len() => (start..end, self.ei_from(end).ok_or(unterminated)?),
            Some(_) => return Err(unterminated),
            None => {
                let ei = ei_scan(s, start).ok_or(unterminated)?;
                (start..(ei - 1).max(start), ei + 2)
            }
        };
        if data.len() > self.lim.inline_image {
            return Err(Error {
                at: bi,
                kind: ErrorKind::InlineImageSize,
            });
        }
        Ok((end, data))
    }

    /// An image's first filter, by what finds its data's end; None when
    /// it has none.
    fn first_filter(&self, dict: &ImageDict) -> Option<Filter> {
        let f = &self.s[dict.f.clone()?];
        let first = match f.first() {
            Some(b'[') => *elements(f, self.lim.depth).first()?,
            _ => f,
        };
        // A filter that isn't a name is none, as PDFium reads it.
        Some(match &*name(first)? {
            b"AHx" | b"ASCIIHexDecode" => Filter::Hex,
            b"A85" | b"ASCII85Decode" => Filter::A85,
            b"RL" | b"RunLengthDecode" => Filter::RunLength,
            b"DCT" | b"DCTDecode" => Filter::Dct,
            _ => Filter::Other,
        })
    }

    /// The size of an unfiltered image's data, as PDFium counts it; None
    /// when it counts none.
    fn unfiltered(&self, dict: &ImageDict) -> Option<usize> {
        let s = self.s;
        let value = |r: &Option<Range<usize>>| int(&s[r.clone()?]);
        let (components, bpc) = match &dict.cs {
            None => (1, 1),
            Some(cs) => match self.components(&s[cs.clone()]) {
                Some(n) => (n, value(&dict.bpc)?),
                None => (1, 1),
            },
        };
        let pitch = value(&dict.w)?
            .checked_mul(bpc)?
            .checked_mul(components)?
            .div_ceil(8);
        let size = pitch.checked_mul(value(&dict.h)?)?;
        usize::try_from(size).ok().filter(|&n| n > 0)
    }

    /// The components of an inline image's color space, as written; None
    /// for a name the resources don't have, which PDFium reads as no
    /// color space. What it can't make out has 3, as in PDFium.
    fn components(&self, cs: &[u8]) -> Option<u64> {
        if let Some(n) = name(cs) {
            return match &*n {
                b"G" | b"DeviceGray" => Some(1),
                b"RGB" | b"DeviceRGB" => Some(3),
                b"CMYK" | b"DeviceCMYK" => Some(4),
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

    /// The next `EI` operator from `at`: where it ends.
    fn ei_from(&self, mut at: usize) -> Option<usize> {
        loop {
            let (r, kind) = next_raw(self.s, at, self.lim.depth)?.ok()?;
            if matches!(kind, Raw::Op) && &self.s[r.clone()] == b"EI" {
                return Some(r.end);
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

/// What an inline image's dictionary says that finds its data's end:
/// where each value is.
#[derive(Default)]
struct ImageDict {
    w: Option<Range<usize>>,
    h: Option<Range<usize>>,
    bpc: Option<Range<usize>>,
    cs: Option<Range<usize>>,
    f: Option<Range<usize>>,
    l: Option<Range<usize>>,
}

impl ImageDict {
    fn set(&mut self, key: &[u8], value: Range<usize>) {
        let slot = match name(key).as_deref() {
            Some(b"W" | b"Width") => &mut self.w,
            Some(b"H" | b"Height") => &mut self.h,
            Some(b"BPC" | b"BitsPerComponent") => &mut self.bpc,
            Some(b"CS" | b"ColorSpace") => &mut self.cs,
            Some(b"F" | b"Filter") => &mut self.f,
            Some(b"L" | b"Length") => &mut self.l,
            _ => return,
        };
        *slot = Some(value);
    }
}

/// The dictionary of the inline image whose `BI` is at `bi`, to its `ID`:
/// `entry` is given each key, as written, and where its value is. What
/// isn't a name where a key should be is passed over. Returns where `ID`
/// ends.
fn image_dict(
    s: &[u8],
    bi: usize,
    depth: usize,
    mut entry: impl FnMut(&[u8], Range<usize>),
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
        if matches!(kind, Raw::Op) && &s[r.clone()] == b"ID" {
            return Ok(at);
        }
        match key.take() {
            Some(key) => entry(&s[key], r),
            None if s[r.start] == b'/' => key = Some(r),
            None => {}
        }
    }
}

enum Filter {
    Hex,
    A85,
    RunLength,
    Dct,
    Other,
}
