//! Where an inline image's data ends, as PDFium's `ReadInlineStream`
//! finds it, from its dictionary as PDFium reads it and the components of
//! the color space it's drawn in ([`ResCx`]); and what an image read says
//! ([`InlineImage`]).

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::ops::Range;

use pdf_filters::{Backend, Faults, Predictor, decode_prefix_into};
use pdf_tokenizer::decode::{self, pdfium};
use pdf_tokenizer::is_white_space;

use crate::data::{Places, RunLength, Searched, a85_end, a85_stop, ei_scan, find};
use crate::error::{Error, ErrorKind};
use crate::fault::{self, Fault, Sink};
use crate::limits::{CodedEnds, Limits};
use crate::operand::{Operand, WORD, hex, iso_name, literal, name};
use crate::scan::{
    Event, Raw, TOP, UNCAPPED, Visit, elements, is_number, is_space, next_raw, walk,
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
    /// `/ColorSpace` dictionary's key, its `#` escapes decoded as PDFium
    /// decodes them (pdf-objects reads a key so by PDFium's rules), as an
    /// image's color space is looked up by its name read so. A key read by
    /// ISO 32000-2's rules, as pdf-structure reads a file, is the same
    /// name wherever the two readings of it agree: where it has no `#` that
    /// ISO reads otherwise and is no longer than PDFium reads
    /// ([`Fault::Name`], [`Fault::Long`]). Where it's otherwise, the key's
    /// bytes as written (pdf-objects' `Entry::key_at`), read by
    /// [`name`](crate::name), give PDFium's. PDFium
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
    found: Found,
}

impl<'a> InlineImage<'a> {
    /// The image at `at` in `content`, `BI` to `EI`, its data at `data`,
    /// its end found as `found` says.
    pub(crate) fn new(
        content: &'a [u8],
        at: Range<usize>,
        data: Range<usize>,
        found: Found,
    ) -> InlineImage<'a> {
        InlineImage {
            content,
            at,
            data,
            found,
        }
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

    /// What PDFium makes of its data as it reads the image to draw it
    /// (its `ReadInlineStream`): a stream of the bytes it keeps, a stream
    /// of none, or no stream and so no image ([`ImageStream`]).
    ///
    /// ```
    /// use pdf_content_streams::{ImageStream, Limits, ResCx, operations};
    ///
    /// let content = b"BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 41> 7 EI \
    ///     BI /W 0 /H 1 /BPC 8 /CS /G ID xy EI \
    ///     BI /W 1 /H 1 /BPC 8 /CS /G /F /XYZ ID xy EI";
    /// let res = ResCx::none();
    /// let read: Vec<_> = operations(content, &res, &Limits::DEFAULT)
    ///     .map(|op| op.unwrap().inline_image().unwrap().stream())
    ///     .collect();
    /// // The filter's end, and the tokens before `EI` with it.
    /// assert_eq!(read[0], ImageStream::Data(38..43));
    /// assert_eq!(&content[38..43], b"41> 7");
    /// // No bytes, a size of 0; no stream, a filter PDFium can't decode.
    /// assert_eq!(read[1..], [ImageStream::Empty, ImageStream::Missing]);
    /// ```
    pub fn stream(&self) -> ImageStream {
        match self.found {
            Found::Counted => ImageStream::Data(self.data.clone()),
            Found::Empty => ImageStream::Empty,
            Found::Walk => ImageStream::Missing,
            Found::Filter => {
                // On from the filter's end to the last token before `EI`,
                // which ends where the image does. It was read under a cap
                // on depth, so nothing in it is deeper.
                let (s, mut end) = (self.content, self.data.end);
                while let Some(Ok((r, _))) = next_raw(s, end, UNCAPPED) {
                    if r.end == self.at.end && &s[r.clone()] == b"EI" {
                        break;
                    }
                    end = r.end;
                }
                ImageStream::Data(self.data.start..end)
            }
        }
    }

    /// The value under `key`, as written, as PDFium reads the image's
    /// dictionary: `key` a name's, without its `/`, matched as written,
    /// its `#` escapes not decoded. Of a key 8.9.7 abbreviates (Table
    /// 91), either spelling finds the value: where both are written, the
    /// abbreviation's, unless the full name's is an abbreviated name
    /// (Table 92, `/RGB` say), which PDFium sets over it; so
    /// `entry(b"CS")` and `entry(b"ColorSpace")` are the same. Of a key
    /// written twice, the last; and a value PDFium reads as no object (a
    /// keyword, a closer, a brace or a dictionary that's none) takes the
    /// key out, as PDFium takes out a key it reads no value for.
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
                None if &k[1..] == key => found = value.object(),
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
    /// reads neither, and finds the data otherwise. Read from its
    /// dictionary as ISO 32000-2 reads it, to its first `ID`, one written
    /// as a key's value among them, where PDFium reads on past such an
    /// `ID` ([`entry`](InlineImage::entry)); and, where an object that's
    /// no name is written for a key, which ends the dictionary as PDFium
    /// reads it, to that object, as ISO 32000-2's keys are names (7.3.7).
    pub fn length(&self) -> Option<u64> {
        let mut dict = ImageDict::default();
        // It was read under a cap on depth, so nothing in it is deeper.
        let s = &self.content[..self.at.end];
        let set = |key: &[u8], value| dict.set(key, value);
        image_dict(s, self.at.start, UNCAPPED, set, &mut ()).ok()?;
        int(&self.content[dict.iso(L)?])
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

/// What PDFium makes of an inline image's data as it reads the image to
/// draw it (its `ReadInlineStream`), as [`InlineImage::stream`] says.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImageStream {
    /// A stream of the bytes here: an unfiltered image's, as many as its
    /// size counts, its [`data`](InlineImage::data); a filtered image's,
    /// from its data's start to where its filter's data ends, and on over
    /// any tokens written between that end and `EI`, the white-space
    /// before `EI` left out, which PDFium keeps too. Where PDFium decodes
    /// the data to find its end (Flate, LZW, CCITTFax, DCT), its end as
    /// [`tokens`](crate::tokens) finds it.
    Data(Range<usize>),
    /// A stream of no bytes: an unfiltered image whose size counts to 0,
    /// whose data, read on to `EI` from its start, PDFium keeps none of.
    Empty,
    /// No stream, and so no image, which PDFium neither keeps nor draws:
    /// its data read on to `EI` from its start, its filter one PDFium has
    /// no decoder for, one whose data's end isn't found or has no `EI`
    /// after it, or one that refuses the data or its parameters, or its
    /// size one PDFium can't count.
    Missing,
}

/// How an inline image's data's end was found, which says what PDFium
/// makes of it ([`ImageStream`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Found {
    /// By its size, unfiltered.
    Counted,
    /// By its filter's end.
    Filter,
    /// By the walk to `EI`, unfiltered, its size counting to 0.
    Empty,
    /// By the walk to `EI`: no end found.
    Walk,
}

/// The inline images of one content stream: what finds each one's data;
/// its searches for where an image's data ends, and its walks to `EI` past
/// those ends, remembered, so content of many images whose data doesn't
/// end, or whose ends have no `EI` after them, is searched and walked
/// once, not once for each image; and what finding their ends may cost
/// yet ([`Limits::image_ends`]), with one buffer for what decoding gives.
#[derive(Debug, Clone)]
pub(crate) struct Images<'a, 'r> {
    s: &'a [u8],
    res: &'r ResCx,
    lim: Limits,
    /// The last search for ASCIIHex data's end, its `>`.
    hex: Searched,
    /// The last search for where ASCII85 data stops.
    a85: Searched,
    /// The walks of RunLength data.
    run_length: RunLength,
    /// The last search for a JPEG's end of image.
    eoi: Searched,
    /// The last search for a JPEG's start of image, where its data's end
    /// is found by decoding it.
    #[cfg(feature = "image-ends")]
    soi: Searched,
    /// The walks to `EI` from where a filter's data ends.
    walks: Walks,
    /// What finding images' ends may cost yet in this reading, in bytes.
    budget: usize,
    /// What decoding an image's data gives, emptied after each image.
    decoded: Vec<u8>,
}

impl<'a, 'r> Images<'a, 'r> {
    pub(crate) fn new(s: &'a [u8], res: &'r ResCx, lim: Limits) -> Images<'a, 'r> {
        Images {
            s,
            res,
            lim,
            hex: Searched::default(),
            a85: Searched::default(),
            run_length: RunLength::default(),
            eoi: Searched::default(),
            #[cfg(feature = "image-ends")]
            soi: Searched::default(),
            walks: Walks::default(),
            budget: lim.image_ends,
            decoded: Vec::new(),
        }
    }

    /// The inline image whose `BI` is at `bi`, or none, where a keyword
    /// is written for its dictionary's key ([`Image`]). Where ISO 32000-2
    /// would read an image otherwise, `sink` is told, if it wants: its
    /// dictionary's tokens' own faults and its entries', as they're read;
    /// then, the image read, its keys', what ends its dictionary, its
    /// `ID`'s, its length's and what's before its `EI`. Where finding its
    /// data's end would cost more than is left of
    /// [`Limits::image_ends`] for the reading, an error at `bi`
    /// ([`ErrorKind::ImageEnds`]).
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
        // A decoder that finds no end of its data, or refuses it, is none
        // to PDFium, which then reads on from its data's start as it does
        // for a filter it can't decode; and so for an image whose size it
        // can't count, which it reads no data of, filtered or not. What a
        // decoder read in vain, whatever comes after, is spent once the
        // image is found to end. Where a decoder ends the data short of
        // where ISO 32000-2 ends it, over the filter's own end of data,
        // that end too, for the faults.
        let mut vain = 0;
        let mut iso = None;
        let end = match (self.size(&dict), self.first_filter(&dict)) {
            (None, _) => End::Walk,
            (Some(size), None) => match usize::try_from(size) {
                Ok(0) => End::Empty,
                Ok(n) => End::Size(start.saturating_add(n)),
                Err(_) => End::Walk,
            },
            (_, Some(filter)) => match filter {
                Filter::Hex => (self.hex.from(start, |at| find(s, at, b">")))
                    .map_or(End::Walk, |at| End::Filter(at + 1, 0)),
                Filter::A85 => (self.a85.from(start, |at| a85_stop(s, at)))
                    .map_or(End::Walk, |stop| End::Filter(a85_end(s, start, stop), 0)),
                Filter::RunLength => {
                    (self.run_length.end(s, start)).map_or(End::Walk, |end| End::Filter(end, 0))
                }
                Filter::Flate | Filter::Lzw => self.decoded_end(bi, start, filter, &dict)?,
                Filter::Dct | Filter::CcittFax => {
                    let (end, read, past) =
                        self.coded_end(bi, start, filter, &dict, sink.wants())?;
                    (vain, iso) = (read, past);
                    end
                }
                Filter::Other => End::Walk,
            },
        };
        let (data, end, found) = match end {
            End::Size(end) if end <= s.len() => {
                let ei = self.ei_from(end).ok_or(unterminated)?;
                (start..end, ei.end, Found::Counted)
            }
            End::Size(_) => return Err(unterminated),
            End::Filter(end, decoded) => match self.walks.ei_from(s, end, self.lim.depth) {
                Ok(ei) => (start..end, ei.end, Found::Filter),
                // No `EI` after the filter's end: PDFium keeps no stream
                // of the data, and reads on from its start. What was read
                // in vain, past the end and decoded before it, is spent,
                // once the image is found to end.
                Err(passed) => {
                    let (data, end) = self.walk(start).ok_or(unterminated)?;
                    self.spend(bi, passed.saturating_add(decoded))?;
                    (data, end, Found::Walk)
                }
            },
            End::Refused(decoded) => {
                let (data, end) = self.walk(start).ok_or(unterminated)?;
                self.spend(bi, decoded)?;
                (data, end, Found::Walk)
            }
            End::Scan => {
                let ei = ei_scan(s, start).ok_or(unterminated)?;
                (start..(ei - 1).max(start), ei + 2, Found::Filter)
            }
            End::Empty => {
                let (data, end) = self.walk(start).ok_or(unterminated)?;
                (data, end, Found::Empty)
            }
            End::Walk => {
                let (data, end) = self.walk(start).ok_or(unterminated)?;
                (data, end, Found::Walk)
            }
        };
        self.spend(bi, vain)?;
        if data.len() > self.lim.inline_image {
            return Err(Error {
                at: bi,
                kind: ErrorKind::InlineImageSize,
            });
        }
        if sink.wants() {
            // Where ISO's data ends, where that's past where it's found to
            // end here and before `EI`.
            let past = |&iso: &usize| found == Found::Filter && data.end < iso && iso <= end - 2;
            let iso = iso.filter(past).unwrap_or(data.end);
            self.faults(&dict, between, &ends, &data, iso, end, sink);
        }
        Ok(Image::Read(end, data, found))
    }

    /// Where ISO 32000-2 reads an image read otherwise, told to `sink`:
    /// a key both abbreviated and in full whose value ISO reads otherwise,
    /// its dictionary at `between`, from after `BI` to what `ends` it; the
    /// object that ends it, where one does, its own faults and then that
    /// it ends it; its `ID`, where that ends it and ISO's dictionary too,
    /// with no white-space after it (an earlier `ID`, a key's value, that
    /// ends ISO's alone is [`image_dict`]'s to tell); a length that ends
    /// its data elsewhere than ISO's data ends, at `iso`: `data`'s end, or
    /// past it where a decoder ends the data short of the filter's own end
    /// of data; tokens between there and its `EI`, which ends at `end`.
    /// ISO's dictionary is read to its first `ID` ([`ImageDict::iso`]).
    #[allow(clippy::too_many_arguments)]
    fn faults(
        &self,
        dict: &ImageDict,
        between: Range<usize>,
        ends: &Ends,
        data: &Range<usize>,
        iso: usize,
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
        let ascii = ascii(s, dict.iso(F));
        match ends {
            Ends::Object(object) => {
                let written = &s[object.clone()];
                fault::token(written, object.start, sink);
                fault::nesting(s, object.start, |at, fault| sink.fault(at, fault));
                sink.fault(object.clone(), Fault::DictEnd { written });
            }
            Ends::Id(id) if !dict.ended() && !ascii && !space_after(s, id) => {
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
            if ends != Some(iso) {
                let read = iso - start;
                sink.fault(data.start..iso, Fault::ImageLength { given, read });
            }
        }
        // What's between ISO's data and `EI`, the first token to the last:
        // the tokens from the data's end, read as the walk to `EI` read
        // them, from where ISO's data ends.
        let mut between: Option<Range<usize>> = None;
        let mut at = data.end;
        while let Some(Ok((r, _))) = next_raw(s, at, self.lim.depth) {
            if r.end == end && &s[r.clone()] == b"EI" {
                break;
            }
            at = r.end;
            if r.end > iso {
                let first = r.start.max(iso);
                between = Some(between.map_or(first, |b| b.start)..r.end);
            }
        }
        if let Some(between) = between {
            let written = &s[between.clone()];
            sink.fault(between, Fault::BeforeEi { written });
        }
    }

    /// An image's first filter, by what finds its data's end, as PDFium
    /// takes it: a name, its abbreviation read as the full name (Table
    /// 92), or a string's bytes, naming a filter, each read as PDFium's
    /// content parser reads it ([`name`], [`literal`], [`hex`]: `/A#48x`
    /// is `/AHx`); of an array, its first element that's an object, as
    /// PDFium reads the array ([`elements`]: `[[/AHx]` is `/AHx`). None, as
    /// for no filter, for `null`, a dictionary or what's no object, and for
    /// an array with no object in it; [`Filter::Other`] for a name or a
    /// string that names no filter PDFium decodes, a number or a boolean.
    fn first_filter(&self, dict: &ImageDict) -> Option<Filter> {
        let s = self.s;
        let f = &s[dict.get(s, F)?];
        let first = match f.first() {
            Some(b'[') => elements(f).into_iter().find(|&(_, object)| object)?.0,
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
            b"true" | b"false" => return Some(Filter::Other),
            n if is_number(n) => return Some(Filter::Other),
            _ => return None,
        };
        Some(match &*named {
            b"ASCIIHexDecode" => Filter::Hex,
            b"ASCII85Decode" => Filter::A85,
            b"RunLengthDecode" => Filter::RunLength,
            b"DCTDecode" => Filter::Dct,
            b"FlateDecode" => Filter::Flate,
            b"LZWDecode" => Filter::Lzw,
            b"CCITTFaxDecode" => Filter::CcittFax,
            _ => Filter::Other,
        })
    }

    /// The size of an image's data as PDFium counts it from its dictionary
    /// (`ReadInlineStream`), which an unfiltered image's data is: its `/W`,
    /// `/H` and `/BPC` each as PDFium reads an integer from a value
    /// ([`integer`]), 0 where there's none, and held as PDFium holds them,
    /// in 32 bits, unsigned, the width signed where a row's bits are
    /// counted; its color space's components and `/BPC` where it has one the
    /// resources give, and one bit of one component where it has none or
    /// names one they don't; a row's bits padded to a byte. None where that
    /// count overflows 32 bits, or a row of any bits to a pixel has a
    /// negative width: PDFium then reads no data, filtered or not, and reads
    /// on to `EI` from the data's start. (PDFium's chromium/7881 pads a row
    /// by adding 7 to its bits first, so a row of 2^32 − 7 bits or more
    /// overflows there; chromium/8076 and chromium/8125, which this follows,
    /// divide, rounding up, as here.)
    fn size(&self, dict: &ImageDict) -> Option<u32> {
        let s = self.s;
        let value = |key| dict.get(s, key).map_or(0, |r| integer(&s[r]));
        let (bpc, components) = match dict.get(s, CS).and_then(|cs| self.components(&s[cs])) {
            Some(n) => (value(BPC) as u32, u32::try_from(n).ok()?),
            None => (1, 1),
        };
        let bits = bpc.checked_mul(components)?;
        let row = match u32::try_from(value(W)) {
            Ok(width) => bits.checked_mul(width)?,
            Err(_) if bits == 0 => 0,
            Err(_) => return None,
        };
        row.div_ceil(8).checked_mul(value(H) as u32)
    }

    /// The components of an inline image's color space, as written, a name
    /// read as PDFium's content parser reads it ([`name`]); None for a
    /// name the resources don't have, which PDFium reads as no color space,
    /// an abbreviation looked up by its full name (`/I` as `/Indexed`).
    /// What it can't make out has 3, as in PDFium.
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
        // Its family, the array's first element that's an object, as
        // PDFium reads the array ([`elements`]). A DeviceN space names its
        // colorants in an array, its second element, which an array here
        // never holds, as PDFium opens none in an array outside a
        // dictionary: so it's one PDFium can't make out, as
        // `family_components` counts it.
        let first = elements(cs).into_iter().find(|&(_, object)| object);
        let family = first.and_then(|(first, _)| name(first));
        Some(family.map_or(3, |family| family_components(&family).into()))
    }

    /// Where an image's Flate or LZW data from `start` ends, as PDFium's
    /// `DecodeInlineStream` finds it: where its decoder stops reading it
    /// ([`End::Filter`]), its parameters read as PDFium reads them
    /// ([`Images::params`]), the data decoded by pdf-filters, Flate's on
    /// zlib-rs, whose count of what it read is zlib's, and so PDFium's
    /// ([`Prefix::read`](pdf_filters::Prefix::read)), given all that
    /// follows, which costs nothing more than what it reads. None
    /// ([`End::Walk`]) where PDFium refuses the
    /// parameters, and none, what was read in vain said
    /// ([`End::Refused`]), where it refuses what decoding gives: an LZW
    /// code that isn't in the table, LZW data that decodes to nothing, a
    /// predictor whose rows would be no bytes, a PNG predictor on nothing
    /// decoded, or an end past 2^31 − 1 bytes. (Of an LZW code that isn't
    /// in the table, PDFium refuses one that's the first since a clear
    /// code or the data's start, and reads on past a later one as if it
    /// were the table's next entry, which isn't followed here.) What
    /// decoding gives is spent: past the budget, an error at `bi`.
    fn decoded_end(
        &mut self,
        bi: usize,
        start: usize,
        filter: Filter,
        dict: &ImageDict,
    ) -> Result<End, Error> {
        let Some(params) = self.params(dict) else {
            return Ok(End::Walk);
        };
        let data = &self.s[start..];
        let lzw = matches!(filter, Filter::Lzw);
        // The decoder alone: a predictor changes no end, but by refusing.
        let predictor = Predictor::None;
        let filter = match lzw {
            true => pdf_filters::Filter::Lzw {
                early_change: params.early_change,
                predictor,
            },
            false => pdf_filters::Filter::Flate { predictor },
        };
        let (prefix, decoded) = decode_prefix_into(
            data,
            filter,
            self.budget,
            Backend::ZlibRs,
            &mut self.decoded,
        );
        let made = self.decoded.len();
        self.decoded.clear();
        let faults = decoded.err().map_or_else(Vec::new, |Faults(faults)| faults);
        let faulted = |is: fn(&pdf_filters::Fault) -> bool| faults.iter().any(|(_, f)| is(f));
        if faulted(|f| matches!(f, pdf_filters::Fault::Limit)) {
            return Err(Error {
                at: bi,
                kind: ErrorKind::ImageEnds,
            });
        }
        self.spend(bi, made)?;
        let refused = (lzw
            && (made == 0 || faulted(|f| matches!(f, pdf_filters::Fault::Code { .. }))))
            || params.refuses(made)
            || i32::try_from(prefix.read).is_err();
        if refused {
            return Ok(End::Refused(prefix.read));
        }
        Ok(End::Filter(start + prefix.read, prefix.read))
    }

    /// What an image's `/DecodeParms` say to PDFium's Flate and LZW
    /// decoders, as PDFium reads them (`GetAndCheckDecodeParams`): its
    /// value where that's a dictionary, or, where the filter is an array,
    /// the first object of its array, where that's one; each key decoded,
    /// as PDFium decodes a name in a dictionary ([`name`]), the last of
    /// one written twice, `/BPC` read as `/BitsPerComponent`, as PDFium
    /// reads an image's abbreviated keys, its value too, and each value as
    /// PDFium reads an integer from it ([`integer`]): `/Predictor` 0,
    /// `/EarlyChange` 1, `/Colors` 1, `/BitsPerComponent` 8 and `/Columns`
    /// 1 where they're not written, or there's no dictionary. None where
    /// PDFium refuses them: `/Colors`, `/BitsPerComponent` or `/Columns`
    /// negative, or `/Columns` times `/Colors` times `/BitsPerComponent`,
    /// multiplied in that order in 32 bits, signed, overflowing, or past
    /// 2^31 − 8.
    fn params(&self, dict: &ImageDict) -> Option<Params> {
        let Some(entries) = self.decode_parms(dict) else {
            return Some(Params::DEFAULT);
        };
        let get = |key: &[u8]| entries.get(key);
        let int = |key: &[u8], default| get(key).map_or(default, integer);
        let bits_per_component = match (get(b"BitsPerComponent"), get(b"BPC")) {
            (Some(full), Some(_)) if abbreviated(full) => Some(full),
            (full, short) => short.or(full),
        };
        let params = Params {
            predictor: int(b"Predictor", 0),
            early_change: int(b"EarlyChange", 1) != 0,
            colors: int(b"Colors", 1),
            bits_per_component: bits_per_component.map_or(8, integer),
            columns: int(b"Columns", 1),
        };
        let Params {
            colors,
            bits_per_component: bpc,
            columns,
            ..
        } = params;
        if colors < 0 || bpc < 0 || columns < 0 {
            return None;
        }
        let bits = columns.checked_mul(colors)?.checked_mul(bpc)?;
        (bits <= i32::MAX - 7).then_some(params)
    }

    /// The entries of the dictionary an image's `/DecodeParms` give its
    /// first filter, as PDFium finds it (`ReadInlineStream`): its value
    /// where that's a dictionary, or, where the filter is an array, the
    /// first object of its array, where that's one. None where there's no
    /// such dictionary.
    fn decode_parms(&self, dict: &ImageDict) -> Option<Entries<'a>> {
        let s = self.s;
        let value = |key| dict.get(s, key).map(|r| &s[r]);
        let dp = match (value(F), value(DP)) {
            (Some([b'[', ..]), Some(dp @ [b'[', ..])) => elements(dp)
                .into_iter()
                .find(|&(_, object)| object)
                .map(|(first, _)| first),
            (Some([b'[', ..]), _) => None,
            (_, dp) => dp,
        };
        let dp = dp.filter(|dp| dp.starts_with(b"<<"))?;
        let mut entries = Entries::default();
        // It was read under a cap on depth, so nothing in it is deeper.
        walk(dp, 0, TOP, UNCAPPED, true, &mut entries);
        Some(entries)
    }

    /// Where an image's DCT or CCITTFax data from `start` ends, what
    /// finding it read in vain, whatever comes after it, and, where a
    /// decoder found its end, where the data ends as ISO 32000-2 reads it,
    /// which for fax data is looked for only where `wants`: as the reading
    /// asks ([`Limits::coded_ends`]), the stand-in's
    /// ([`Images::stand_in`]), nothing decoded, or, with the `image-ends`
    /// feature, the decoders' (`Images::decoders_end`).
    // Without the feature, the stand-in takes only where the data starts.
    #[cfg_attr(not(feature = "image-ends"), allow(unused_variables))]
    fn coded_end(
        &mut self,
        bi: usize,
        start: usize,
        filter: Filter,
        dict: &ImageDict,
        wants: bool,
    ) -> Result<(End, usize, Option<usize>), Error> {
        match self.lim.coded_ends {
            CodedEnds::StandIn => Ok((self.stand_in(start, filter), 0, None)),
            #[cfg(feature = "image-ends")]
            CodedEnds::Decoded => self.decoders_end(bi, start, filter, dict, wants),
        }
    }

    /// Where an image's DCT or CCITTFax data from `start` ends, as
    /// [`Images::coded_end`] gives it, found by decoding it. PDFium decodes
    /// both to find where they end (its `DecodeInlineStream`), DCT's on
    /// libjpeg-turbo and CCITTFax's on a decoder of its own; so is this
    /// (zune-jpeg, hayro-ccitt), the image's `/W` and `/H` and its
    /// parameters read as PDFium reads them ([`Images::fax`]), PDFium's
    /// refusals the walk from the data's start
    /// ([`End::Refused`]), and the end the stand-in's
    /// ([`Images::stand_in`]) where the decoder can't decode what PDFium's
    /// reads on through. What decoding makes is spent: past the budget, an
    /// error at `bi`. ISO's end is past PDFium's where the filter's own end
    /// of data follows the rows PDFium decodes: a JPEG's end of image where
    /// bytes come before its start of image (PDFium counting its end from
    /// there), fax data's end-of-block pattern
    /// ([`decoders::ccitt_iso`](crate::decoders::ccitt_iso)).
    #[cfg(feature = "image-ends")]
    fn decoders_end(
        &mut self,
        bi: usize,
        start: usize,
        filter: Filter,
        dict: &ImageDict,
        wants: bool,
    ) -> Result<(End, usize, Option<usize>), Error> {
        use crate::decoders::{self, Found};
        let s = self.s;
        let value = |key| dict.get(s, key).map_or(0, |r| integer(&s[r]));
        let (width, height, data) = (value(W), value(H), &s[start..]);
        let (found, iso) = match filter {
            Filter::Dct => {
                let soi = (self.soi.from(start, |at| find(s, at, b"\xff\xd8")))
                    .map_or(0, |soi| soi - start);
                let decoded =
                    decoders::dct(data, soi, width as u32, self.budget, &mut self.decoded).ok_or(
                        Error {
                            at: bi,
                            kind: ErrorKind::ImageEnds,
                        },
                    )?;
                self.spend(bi, decoded.made)?;
                // Its decoder read to just past its end of image, or, a
                // JPEG of one scan, to past the marker it stopped at, where
                // PDFium's ends it.
                let iso = match decoded.end {
                    Found::At { read, .. } => Some(read),
                    _ => None,
                };
                (decoded.end, iso)
            }
            _ => {
                let fax = self.fax(dict);
                let found = decoders::ccitt(data, width, height, fax);
                let iso = (wants && matches!(found, Found::At { .. }))
                    .then(|| decoders::ccitt_iso(data, width, height, fax))
                    .flatten();
                (found, iso)
            }
        };
        Ok(match found {
            Found::At { end, read } => (
                End::Filter(start + end, read),
                0,
                iso.map(|iso| start + iso),
            ),
            Found::Refused(read) => (End::Refused(read), 0, None),
            Found::Undecoded(read) => (self.stand_in(start, filter), read, None),
        })
    }

    /// Where an image's DCT or CCITTFax data from `start` ends, found in
    /// its bytes, standing in for PDFium's decoders: a JPEG's past its
    /// first end of image (FF D9) from its start, and fax data's, or a
    /// JPEG's with none, at the first `EI` with white-space before it and
    /// white-space, a delimiter or the end after it ([`End::Scan`]). They
    /// agree with the decoders wherever a JPEG holds no end of image before
    /// its own (a thumbnail's, in its metadata) and the coded bytes no such
    /// `EI`, and the data isn't damaged.
    fn stand_in(&mut self, start: usize, filter: Filter) -> End {
        let s = self.s;
        match filter {
            Filter::Dct => (self.eoi.from(start, |at| find(s, at, b"\xff\xd9")))
                .map_or(End::Scan, |eoi| End::Filter(eoi + 2, 0)),
            _ => End::Scan,
        }
    }

    /// What an image's `/DecodeParms` say to PDFium's fax decoder, as
    /// PDFium reads them (`CreateFaxDecoder`), from the dictionary
    /// [`Images::decode_parms`] finds: `/K`, `/EndOfLine`,
    /// `/EncodedByteAlign`, `/Columns` and `/Rows`, each as PDFium reads an
    /// integer from it, `/Columns` 1728 where it isn't written, and `/Rows`
    /// 0, the image's height, where it's past 65,535; and `/EndOfBlock`, as
    /// ISO 32000-2 reads it, true unless it's `false`, which PDFium doesn't
    /// read.
    #[cfg(feature = "image-ends")]
    fn fax(&self, dict: &ImageDict) -> crate::decoders::Fax {
        use crate::decoders::Fax;
        let Some(entries) = self.decode_parms(dict) else {
            return Fax::DEFAULT;
        };
        let int = |key: &[u8], default| entries.get(key).map_or(default, integer);
        let rows = int(b"Rows", 0);
        Fax {
            k: int(b"K", 0),
            end_of_line: int(b"EndOfLine", 0) != 0,
            byte_align: int(b"EncodedByteAlign", 0) != 0,
            columns: int(b"Columns", 1728),
            rows: if rows > 65535 { 0 } else { rows },
            end_of_block: entries.get(b"EndOfBlock") != Some(b"false"),
        }
    }

    /// `n` bytes more of what finding images' ends may cost in the
    /// reading spent, for the image whose `BI` is at `bi`: past the
    /// budget, an error there.
    fn spend(&mut self, bi: usize, n: usize) -> Result<(), Error> {
        self.budget = self.budget.checked_sub(n).ok_or(Error {
            at: bi,
            kind: ErrorKind::ImageEnds,
        })?;
        Ok(())
    }

    /// An image's data from `start` to the next `EI` operator, the
    /// white-space byte before it, by PDFium's rules, left out, and where
    /// that `EI` ends; None where none comes.
    fn walk(&self, start: usize) -> Option<(Range<usize>, usize)> {
        let ei = self.ei_from(start)?;
        let before = ei.start > start && is_space(self.s[ei.start - 1]);
        Some((start..ei.start - usize::from(before), ei.end))
    }

    /// The next `EI` operator from `at` ([`ei_from`]).
    fn ei_from(&self, at: usize) -> Option<Range<usize>> {
        ei_from(self.s, at, self.lim.depth)
    }
}

/// The next `EI` operator from `at` in `s`, its tokens read as PDFium
/// reads them under `depth`, a string or an array one token: where it is.
pub(crate) fn ei_from(s: &[u8], mut at: usize, depth: usize) -> Option<Range<usize>> {
    loop {
        let (r, kind) = next_raw(s, at, depth)?.ok()?;
        if matches!(kind, Raw::Op) && &s[r.clone()] == b"EI" {
            return Some(r);
        }
        at = r.end;
    }
}

/// The walks to the next `EI` operator from where inline images' data
/// ends, in one content stream, where PDFium walks from a decoder's end
/// (its `ReadInlineStream`): each place one that found none began at, or
/// passed as a token's start, remembered, as a walk from it finds none
/// either. So content of many images whose data ends where no `EI` comes
/// after reads what's after it once, not once for each image.
#[derive(Debug, Clone, Default)]
pub(crate) struct Walks {
    /// The places a walk finds no `EI` from.
    endless: Places,
}

impl Walks {
    /// The next `EI` operator from `at` in `s`, the content stream it's
    /// always given, read as [`ei_from`] reads it under `depth`: where it
    /// is; or, where the walk meets the end, an error, or a place a walk
    /// that found none began at or passed as a token's start, how many
    /// bytes it passed.
    pub(crate) fn ei_from(
        &mut self,
        s: &[u8],
        at: usize,
        depth: usize,
    ) -> Result<Range<usize>, usize> {
        let mut place = at;
        let stopped = loop {
            if self.endless.has(place) {
                break place;
            }
            let Some(Ok((r, kind))) = next_raw(s, place, depth) else {
                break s.len();
            };
            if self.endless.has(r.start) {
                break r.start;
            }
            if matches!(kind, Raw::Op) && &s[r.clone()] == b"EI" {
                return Ok(r);
            }
            place = r.end;
        };
        // None from `at`: it, and each token's start passed, remembered,
        // on a second walk, as only one that finds none remembers.
        self.endless.set(s, at);
        let mut place = at;
        while let Some(Ok((r, _))) = next_raw(s, place, depth) {
            if r.start >= stopped {
                break;
            }
            self.endless.set(s, r.start);
            place = r.end;
        }
        Err(stopped - at)
    }
}

/// What an image's `/DecodeParms` say to PDFium's Flate and LZW decoders,
/// as PDFium reads them ([`Images::params`]).
#[derive(Debug, Clone, Copy)]
struct Params {
    predictor: i32,
    early_change: bool,
    colors: i32,
    bits_per_component: i32,
    columns: i32,
}

impl Params {
    /// What's read where there are none.
    const DEFAULT: Params = Params {
        predictor: 0,
        early_change: true,
        colors: 1,
        bits_per_component: 8,
        columns: 1,
    };

    /// Whether PDFium's predictor refuses what its decoder gave, `made`
    /// bytes (`ApplyPredictor`): PNG's (a `/Predictor` of 10 or more) where
    /// a row would be no bytes, or on nothing; TIFF's (2) where a row would
    /// be no bytes. Any other predicts nothing.
    fn refuses(&self, made: usize) -> bool {
        let row =
            i64::from(self.columns) * i64::from(self.colors) * i64::from(self.bits_per_component);
        match self.predictor {
            10.. => row == 0 || made == 0,
            2 => row == 0,
            _ => false,
        }
    }
}

/// A dictionary's entries, as a [`walk`] of it tells them: each key, decoded
/// as PDFium decodes a name ([`name`]), and its value's first token as
/// written, a `[` or a `<<` for an array or a dictionary, in the order
/// written.
#[derive(Default)]
struct Entries<'a> {
    /// How many arrays and dictionaries are open.
    depth: usize,
    /// The key read last, whose value is next.
    key: Option<Cow<'a, [u8]>>,
    read: Vec<(Cow<'a, [u8]>, &'a [u8])>,
}

impl<'a> Entries<'a> {
    /// The value of `key`, decoded, as written: the last where it's written
    /// twice.
    fn get(&self, key: &[u8]) -> Option<&'a [u8]> {
        (self.read.iter().rev()).find_map(|(k, v)| (**k == *key).then_some(*v))
    }

    /// `value`'s first token read where a value is: the dictionary's, its
    /// key's.
    fn value(&mut self, value: &'a [u8]) {
        if self.depth == 1 {
            if let Some(key) = self.key.take() {
                self.read.push((key, value));
            }
        }
    }
}

impl<'a> Visit<'a> for Entries<'a> {
    fn visit(&mut self, event: Event<'a>) {
        match event {
            Event::Leaf(token) => self.value(token.bytes),
            Event::Open(token) => {
                self.value(token.bytes);
                self.depth += 1;
            }
            Event::Key(token) if self.depth == 1 => self.key = name(token.bytes),
            Event::Close(_) => self.depth = self.depth.saturating_sub(1),
            _ => {}
        }
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
const DP: usize = 3;
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
    /// The values as ISO 32000-2's dictionary ends with them, where an
    /// `ID` written as a key's value ends it before PDFium's (8.9.7).
    iso: Option<[[Option<Range<usize>>; 2]; KEYS.len()]>,
}

impl ImageDict {
    /// `key`, a name as written, its `/` first, has `value`, or is taken
    /// out where it has none. The first `ID` given as a value ends ISO
    /// 32000-2's dictionary, with the values before it.
    fn set(&mut self, key: &[u8], value: Value) {
        if matches!(value, Value::Id) && self.iso.is_none() {
            self.iso = Some(self.values.clone());
        }
        let value = value.object();
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

    /// Whether ISO 32000-2's dictionary ended before PDFium's, at an `ID`
    /// written as a key's value.
    fn ended(&self) -> bool {
        self.iso.is_some()
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
    /// add), in its dictionary, which ends at its first `ID`, a key's value
    /// too.
    fn iso(&self, i: usize) -> Option<Range<usize>> {
        let [full, short] = self.iso.as_ref().unwrap_or(&self.values)[i].clone();
        short.or(full)
    }

    /// The keys PDFium reads otherwise than ISO 32000-2, `s` what the
    /// dictionary is in: both spellings written, PDFium's value the full
    /// name's, an abbreviated name, and ISO's the abbreviation's
    /// ([`iso`](ImageDict::iso)), which means another.
    fn parted<'s>(&'s self, s: &'s [u8]) -> impl Iterator<Item = &'static str> + 's {
        (0..KEYS.len()).filter_map(move |i| {
            let [Some(full), Some(_)] = self.values[i].clone() else {
                return None;
            };
            let short = self.iso.as_ref().unwrap_or(&self.values)[i][1].clone()?;
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

/// What's written for an inline image's key, as [`image_dict`] gives it.
pub(crate) enum Value {
    /// An object: where it is.
    Object(Range<usize>),
    /// What PDFium reads as no object, a keyword, a closer, a brace or a
    /// dictionary that's none, which takes the key out.
    None,
    /// `ID`, which PDFium reads as no object too, and which ends ISO
    /// 32000-2's dictionary (8.9.7).
    Id,
}

impl Value {
    /// Where the object is, if it's one.
    fn object(self) -> Option<Range<usize>> {
        match self {
            Value::Object(at) => Some(at),
            Value::None | Value::Id => None,
        }
    }
}

/// What reading an inline image's `BI` gives ([`Images::read`]).
pub(crate) enum Image {
    /// An image: where it ends, past its `EI`, where its data is, and how
    /// that was found.
    Read(usize, Range<usize>, Found),
    /// No image, a keyword where its dictionary's key is written: where
    /// that keyword is.
    Lone(Range<usize>),
}

/// The dictionary of the inline image whose `BI` is at `bi`, as PDFium's
/// `Handle_BeginImage` reads it, and what ends it ([`Ends`]): a name, its
/// value, and so on, until what's written for a key isn't a name. `entry`
/// is given each key, as written, and its value ([`Value`]): where it is,
/// or what PDFium reads as no object (a keyword, `ID` among them, a
/// closer, a brace or a dictionary that's none), which takes the key out.
/// Where ISO 32000-2 would read it otherwise, `sink` is told, if it wants:
/// each key's and value's own faults, where PDFium ends a value before ISO
/// would among them ([`fault::nesting`]), a key with a `#` escape ISO
/// reads as an image's, a word that's no object as a value, a key whose
/// value is `ID`, which ends ISO's dictionary, and, the first such `ID`,
/// its own white-space after it where ISO asks it ([`Fault::AfterId`]);
/// not what ends PDFium's, which is the image's to tell
/// ([`Images::read`]).
fn image_dict<'a>(
    s: &'a [u8],
    bi: usize,
    depth: usize,
    mut entry: impl FnMut(&[u8], Value),
    sink: &mut (impl Sink<'a> + ?Sized),
) -> Result<Ends, Error> {
    let unterminated = Error {
        at: bi,
        kind: ErrorKind::InlineImage,
    };
    let mut at = bi + 2;
    // ISO 32000-2's dictionary, to its first `ID`, a key's value too: its
    // filters, full name and abbreviation, and whether it's ended.
    let (mut filter, mut ended): ([Option<Range<usize>>; 2], bool) = ([None, None], false);
    loop {
        let (key, kind) = next_raw(s, at, depth).ok_or(unterminated)??;
        match kind {
            Raw::Op if &s[key.clone()] == b"ID" => return Ok(Ends::Id(key)),
            Raw::Op => return Ok(Ends::Keyword(key)),
            Raw::Operand { .. } if s[key.start] != b'/' => return Ok(Ends::Object(key)),
            Raw::Operand { .. } => {}
        }
        if sink.wants() {
            key_faults(s, key.clone(), sink);
        }
        let (value, kind) = next_raw(s, key.end, depth).ok_or(unterminated)??;
        at = value.end;
        let written = &s[value.clone()];
        let id = matches!(kind, Raw::Op) && written == b"ID";
        if sink.wants() {
            match kind {
                _ if id => {
                    let written = &s[key.clone()];
                    sink.fault(key.clone(), Fault::NoValue { written });
                    let [full, short] = filter.clone();
                    if !ended && !ascii(s, short.or(full)) && !space_after(s, &value) {
                        sink.fault(value.clone(), Fault::AfterId);
                    }
                }
                Raw::Op => sink.fault(value.clone(), Fault::Keyword { written }),
                Raw::Operand { .. } => {
                    fault::token(written, value.start, sink);
                    fault::nesting(s, value.start, |at, fault| sink.fault(at, fault));
                }
            }
        }
        let none = !matches!(kind, Raw::Operand { object: true });
        let key = &s[key];
        if !ended {
            match key {
                b"/Filter" => filter[0] = (!none).then(|| value.clone()),
                b"/F" => filter[1] = (!none).then(|| value.clone()),
                _ => {}
            }
        }
        ended |= id;
        let value = match () {
            _ if id => Value::Id,
            _ if none => Value::None,
            _ => Value::Object(value),
        };
        entry(key, value);
    }
}

/// Whether the last of an image's filters, or its one, its value at `f`,
/// is one of the two ASCII ones, ASCIIHexDecode and ASCII85Decode, as ISO
/// 32000-2 reads its name, decoded (7.3.5): where ISO asks no white-space
/// after `ID`, and counts the data's length from after any (8.9.7).
fn ascii(s: &[u8], f: Option<Range<usize>>) -> bool {
    let Some(f) = f.map(|f| &s[f]) else {
        return false;
    };
    let last = match f.first() {
        Some(b'[') => elements(f).last().map(|&(last, _)| last),
        _ => Some(f),
    };
    let ascii =
        |n: Cow<[u8]>| matches!(&*n, b"AHx" | b"ASCIIHexDecode" | b"A85" | b"ASCII85Decode");
    last.and_then(iso_name).is_some_and(ascii)
}

/// Whether the byte after `id`, an `ID`, is white-space as ISO 32000-2
/// reads it (7.2.3).
fn space_after(s: &[u8], id: &Range<usize>) -> bool {
    s.get(id.end).is_some_and(|&c| is_white_space(c))
}

/// An inline image's key at `r`, a name: where ISO 32000-2 would read it
/// otherwise, told to `sink`.
fn key_faults<'a>(s: &'a [u8], r: Range<usize>, sink: &mut (impl Sink<'a> + ?Sized)) {
    let written = &s[r.clone()];
    fault::token(written, r.start, sink);
    // A key ISO reads by its escapes decoded, which PDFium matches as
    // written.
    let iso = || iso_name(written).is_some_and(|k| *k != written[1..] && image_key(&k));
    if written.contains(&b'#') && iso() {
        sink.fault(r, Fault::EscapedKey { written });
    }
}

/// An inline image's value as PDFium reads an integer from it
/// (`GetIntegerFor`): a number word by its first 255 bytes, as
/// pdf-tokenizer's [`Number::integer`](pdfium::Number::integer) holds it
/// (an unsigned one past `i32` wrapped, a real cut to its whole part,
/// saturating), `true` 1, and 0 for anything else.
fn integer(value: &[u8]) -> i32 {
    match value {
        b"true" => 1,
        v if is_number(v) => pdfium::number(&v[..v.len().min(WORD)]).integer(),
        _ => 0,
    }
}

/// A length, as ISO 32000-2 writes a number (7.3.3): a non-negative one
/// under 2^32, a real's whole part.
fn int(number: &[u8]) -> Option<u64> {
    let value = decode::number(number).ok()?;
    (0.0..4_294_967_296.0)
        .contains(&value)
        .then_some(value as u64)
}

/// An inline image's first filter, by what finds its data's end.
#[derive(Clone, Copy)]
enum Filter {
    Hex,
    A85,
    RunLength,
    /// Flate and LZW, which PDFium decodes to find their end, as here.
    Flate,
    Lzw,
    /// DCT and CCITTFax, which PDFium decodes to find their end, as here
    /// with the `image-ends` feature, which without it a stand-in finds
    /// ([`Images::coded_end`]).
    Dct,
    CcittFax,
    /// One PDFium has no decoder for.
    Other,
}

/// How an inline image's data ends.
enum End {
    /// An unfiltered image's, where its size counts it to.
    Size(usize),
    /// An unfiltered image's whose size counts to 0: at the next `EI`
    /// operator, PDFium keeping a stream of none.
    Empty,
    /// Where its filter's data ends, as PDFium's decoder finds it, and how
    /// many bytes of it were decoded to find it, which are read in vain if
    /// no `EI` comes after.
    Filter(usize, usize),
    /// At the next `EI` operator, as [`End::Walk`], its decoder having
    /// refused the data, read in vain: how many bytes it read.
    Refused(usize),
    /// At the first `EI` with white-space before it and white-space, a
    /// delimiter or the end after it.
    Scan,
    /// At the next `EI` operator, its tokens read as PDFium reads them.
    Walk,
}
