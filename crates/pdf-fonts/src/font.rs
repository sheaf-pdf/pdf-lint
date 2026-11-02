//! A file's font, as read: the codes a string is in it, the CID and text each code
//! is, how far each advances and how high its glyphs reach, read as PDFium reads
//! them, so a glyph's box is where PDFium draws it. What's known of neither its
//! dictionary nor Adobe's metrics is guessed, and the font says what it guessed
//! ([`FontApprox`]). A predefined CMap is Adobe's, as hayro-cmap bundles it with the
//! `predefined-cmaps` feature, split as PDFium splits it.
//!
//! Every metric is in glyph units: thousandths of text space at a font size of 1,
//! as `/Widths` gives them; a Type 3 font's are its glyph space through its
//! `/FontMatrix`.
//!
//! Each value of a dictionary is read by what PDFium's accessor for it does, as of
//! chromium/8125, however far that is from ISO 32000-2. A whole number is `true` as
//! 1, what's no number 0, a real cut from its 32-bit float, and past 32 bits 0, and
//! it's held as PDFium holds it: a simple font's widths in 16 bits, a CID font's in
//! 32 and its `/DW` and vertical metrics in 16. A reference is followed once, for a
//! number, an array or a stream, and one to a reference is then none; a dictionary
//! has a reference more, and a stream's will do. So what no real writer writes reads
//! as what PDFium lays out, and the font's reading doesn't part from it for what's
//! hostile.
//!
//! Text is where it parts on purpose, where PDFium's is the worse: a name of joined
//! parts is the Adobe Glyph List's text, a Dingbats name is text only in a ZapfDingbats
//! font, WinAnsiEncoding's 0xA0 and 0xAD are Windows-1252's, a Type 3 font's names are
//! read for text as a simple font's are, and a CID's text is Adobe's UCS2 CMap's.
//!
//! Every array is read within its own length: a `/W` range of 2³¹ CIDs is one range,
//! as it's written, not 2³¹ widths. What a font's streams decompress to (its
//! `/ToUnicode`, its CMap's chain, its CharProcs) is charged to a budget it's read
//! within, a page's in an interpreter, so a page's fonts are bounded with its
//! content ([`Font::read`], [`FontScope::font`](crate::FontScope::font)).

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;

use pdf_content_streams::{ResCx, operations};
use pdf_objects::object::Value;
use pdf_objects::{Dict, Object, ObjectId, Resolve};

use crate::cmap::{CMap, CidMap, Ranges, Scheme, number, two_bytes};
use crate::predefined::{self, BfString, Collection, Split};
use crate::reader::Reader;
use crate::{Encoding, Error, Fault, Limits, Sink, Std14, Streams, glyph_text};

/// A rectangle, in glyph units unless it says otherwise: thousandths of text
/// space at a font size of 1. Normalized, `x0 <= x1` and `y0 <= y1`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    /// Its left.
    pub x0: f32,
    /// Its bottom.
    pub y0: f32,
    /// Its right.
    pub x1: f32,
    /// Its top.
    pub y1: f32,
}

impl Rect {
    /// The rectangle with corners `(a, b)` and `(c, d)`, in either order.
    pub fn new(a: f32, b: f32, c: f32, d: f32) -> Rect {
        Rect {
            x0: a.min(c),
            y0: b.min(d),
            x1: a.max(c),
            y1: b.max(d),
        }
    }
}

/// How far a font's glyphs reach above and below the baseline, and the box all
/// of them fit in, in glyph units: thousandths of text space at a font size
/// of 1. A Type 3 font whose matrix turns glyph space over may have its ascent
/// below its descent, as PDFium keeps them; a glyph's box spans the two either
/// way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontMetrics {
    /// Above the baseline.
    pub ascent: f32,
    /// Below the baseline: negative.
    pub descent: f32,
    /// The box all its glyphs fit in, where it has one.
    pub bbox: Option<Rect>,
}

/// A map of glyph space to text space: `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Affine {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Affine {
    const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// The smallest rectangle that holds the image of `r`: its four corners'
    /// images, bounded.
    fn bounds(self, r: Rect) -> Rect {
        let corners = [(r.x0, r.y0), (r.x1, r.y0), (r.x0, r.y1), (r.x1, r.y1)].map(|(x, y)| {
            (
                self.a * x + self.c * y + self.e,
                self.b * x + self.d * y + self.f,
            )
        });
        let low = |f: fn(&(f32, f32)) -> f32| corners.iter().map(f).fold(f32::INFINITY, f32::min);
        let high =
            |f: fn(&(f32, f32)) -> f32| corners.iter().map(f).fold(f32::NEG_INFINITY, f32::max);
        Rect {
            x0: low(|p| p.0),
            y0: low(|p| p.1),
            x1: high(|p| p.0),
            y1: high(|p| p.1),
        }
    }
}

/// What kind of font a font dictionary is: its `/Subtype`, and a Type 0 font's
/// descendant's. A subtype PDFium doesn't know reads as Type 1, as in PDFium.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontKind {
    /// `/Type1`, or a subtype PDFium doesn't know.
    Type1,
    /// `/MMType1`.
    MMType1,
    /// `/TrueType`. One PDFium reads as GBK, by its name, has its codes by GBK-EUC-H
    /// and no `/Widths` read.
    TrueType,
    /// `/Type3`.
    Type3,
    /// `/Type0`, with its descendant's kind.
    Type0(Descendant),
}

/// A Type 0 font's CIDFont: its glyphs CFF's, or TrueType's. Any other subtype
/// reads as CIDFontType2, as in PDFium.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Descendant {
    /// `/CIDFontType0`.
    CidFontType0,
    /// `/CIDFontType2`, or a subtype PDFium doesn't know.
    CidFontType2,
}

/// What of a font's metrics is guessed: each is exact where it's false.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct FontApprox {
    /// Its codes' CIDs may not be PDFium's: a predefined CMap that the build hasn't,
    /// its codes split by its `/ToUnicode` codespace and their CIDs unknown; an
    /// `/Encoding` that's neither a name nor a stream, which PDFium doesn't load; or
    /// an embedded CMap that uses a predefined one but Identity, which this reads and
    /// PDFium doesn't, or whose chain can't be read whole.
    pub codes: bool,
    /// Its widths: a simple font without `/Widths` that isn't one of the standard 14
    /// drawn without a program, or a Type 0 font without a CIDFont.
    pub widths: bool,
    /// Its ascent and descent: PDFium takes them from the face it draws in, one it
    /// substitutes, which may be the system's, as no descriptor gives them with a
    /// `/FontBBox` and no program read gives them (a Type 3 font: no `/FontBBox`, or
    /// an `A` or `g` whose `d1` gives no box).
    pub heights: bool,
    /// Its glyphs' outlines, so a glyph's box may not hold its outline as PDFium's
    /// does: it embeds no program that was read, as for the standard 14, which
    /// PDFium draws in a face of its own or the system's. In a Type 3 font, one of
    /// whose glyphs' CharProc's `d1` gives no box, and that has no `/FontBBox`.
    pub boxes: bool,
}

impl FontApprox {
    /// Whether any of them is.
    pub fn any(self) -> bool {
        self.codes || self.widths || self.heights || self.boxes
    }
}

/// What was read of a font's program ([`Font::program`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum FontProgram {
    /// None: the font embeds none, or it was read for its text alone
    /// ([`Fonts::for_text`](crate::Fonts::for_text)). So far every font's, as this
    /// version reads no program.
    #[default]
    None,
    /// Read: its glyphs' boxes, where it's known which glyph a code draws, and its
    /// heights where its descriptor lacks them, are the program's.
    Read,
    /// One that can't be read: of no format known, or broken. Its boxes and heights
    /// are guessed, as without one.
    Malformed,
}

/// How a code's glyph is placed when the font writes vertically (ISO 32000-2,
/// 9.7.4.3), in glyph units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VerticalMetrics {
    /// How far it advances down the line: negative.
    pub w1: f32,
    /// How far its vertical origin is along the line from its horizontal one.
    pub vx: f32,
    /// How far its vertical origin is up from its horizontal one.
    pub vy: f32,
}

/// The ascent and descent a font that says nothing of its heights is taken to have:
/// an em above the baseline and a quarter below, where most fonts' glyphs fit.
const GUESSED: FontMetrics = FontMetrics {
    ascent: 1000.0,
    descent: -250.0,
    bbox: None,
};

/// A file's font, as read. It owns all it holds, so it can be kept and shared
/// across threads.
#[derive(Debug)]
pub struct Font {
    /// The object it's read from; None for one written in the resources as a
    /// dictionary.
    id: Option<ObjectId>,
    kind: FontKind,
    codes: Codes,
    widths: Widths,
    /// Where it writes vertically: `/W2` by CID, and `/DW2`.
    vertical: Option<VerticalWidths>,
    metrics: FontMetrics,
    /// A simple font's glyphs' outline boxes by code, where they're known: a Type 3
    /// font's, and a standard 14 font's drawn without a program.
    boxes: Option<Box<[Option<Rect>; 256]>>,
    /// What was read of its program.
    program: FontProgram,
    /// The codes whose widths are guessed where its others' aren't
    /// ([`Font::width_guessed`]), a bit each.
    guessed: [u64; 4],
    /// The most any of its metrics is from 0, in glyph units: widths, vertical
    /// advances and origins, ascent and descent; infinite where one isn't finite
    /// ([`Font::extent`]).
    extent: f32,
    /// What reading its streams decompressed, in bytes: its `/ToUnicode`, its CMap's
    /// chain and its CharProcs, each once.
    cost: usize,
    decoder: Decoder,
    /// A Type 0 font's codes' text where its `/ToUnicode` has none.
    cid_text: Option<CidText>,
    approx: FontApprox,
}

/// How a font's strings are its codes, and its codes CIDs.
#[derive(Debug)]
enum Codes {
    /// A byte each: a simple font.
    One,
    /// Two bytes each, each its CID: Identity-H or Identity-V.
    Identity,
    /// By an embedded CMap's codespace, to its CIDs.
    Embedded(CidMap),
    /// By a predefined CMap's scheme, as PDFium reads it, to the CIDs of Adobe's CMap
    /// of its name.
    Predefined {
        map: &'static predefined::CMap,
        scheme: predefined::Scheme,
    },
    /// As the decoder splits them, CIDs unknown.
    Unknown,
}

/// A Type 0 font's codes' text where its `/ToUnicode` has none, as PDFium reads it
/// (`CPDF_CIDFont::GetUnicodeFromCharCode`): a UCS2 or UTF16 CMap's codes are their
/// own; Identity's and another predefined CMap's CIDs, of a collection of Adobe's,
/// are what that collection's UCS2 CMap maps them to, the collection the predefined
/// CMap's or, for Identity, what its CIDFont's `/CIDSystemInfo` orders; an embedded
/// CMap's have none, as PDFium's on Linux haven't.
#[derive(Debug, Clone, Copy)]
enum CidText {
    Code,
    Cid(&'static predefined::CMap),
}

#[derive(Debug)]
enum Widths {
    /// A simple font's, by code.
    Simple(Box<[f32; 256]>),
    /// A CID font's `/W`, by CID, and `/DW` for the CIDs it doesn't give.
    Cid { w: Ranges<[f32; 1]>, dw: f32 },
    /// A TrueType font PDFium reads as GBK.
    Gbk,
}

#[derive(Debug)]
struct VerticalWidths {
    /// Each range's `w1`, `vx` and `vy`.
    w2: Ranges<[f32; 3]>,
    /// `/DW2`: the vertical origin's height and the advance, for the CIDs `/W2`
    /// doesn't give.
    vy: f32,
    w1: f32,
}

impl Font {
    /// The font `font`, a font dictionary, reads as, what its streams decompress to
    /// charged to `budget`. What it can't read it guesses, and says so, and where it
    /// isn't as ISO 32000-2 says it's told to `sink`, what's read being the same
    /// whatever the sink. It fails where its streams decompress past `budget`
    /// ([`Error::TooLarge`]), or where an object or a stream it reads doesn't read
    /// for a reason that's the caller's ([`Error::Unreadable`]); `budget` is then as
    /// it was.
    pub fn read<'o, O, S>(
        objects: O,
        font: &'o Dict,
        limits: &Limits,
        budget: &mut usize,
        sink: &mut S,
    ) -> Result<Font, Error>
    where
        O: Resolve<'o> + Streams + Copy,
        S: Sink + ?Sized,
    {
        Font::read_as(objects, font, limits, budget, sink, true)
    }

    /// [`Font::read`], its program read where `programs` says.
    pub(crate) fn read_as<'o, O, S>(
        objects: O,
        font: &'o Dict,
        limits: &Limits,
        budget: &mut usize,
        sink: &mut S,
        programs: bool,
    ) -> Result<Font, Error>
    where
        O: Resolve<'o> + Streams + Copy,
        S: Sink + ?Sized,
    {
        let reader = Reader::new(objects, limits, sink);
        let mut left = *budget;
        let read = read_kind(&reader, font, &mut left, programs);
        if let Some(unreadable) = reader.failure() {
            return Err(unreadable.into());
        }
        let mut read = read?;
        read.cost = *budget - left;
        read.extent = read.extent_of_metrics();
        *budget = left;
        Ok(read)
    }

    /// It, read from object `id`.
    pub(crate) fn set_id(&mut self, id: ObjectId) {
        self.id = Some(id);
    }

    /// The object it's read from, where it's one ([`Fonts::get`](crate::Fonts::get)).
    pub fn id(&self) -> Option<ObjectId> {
        self.id
    }

    /// What kind of font it is.
    pub fn kind(&self) -> FontKind {
        self.kind
    }

    /// Whether it writes vertically: a Type 0 font whose CMap does.
    pub fn is_vertical(&self) -> bool {
        self.vertical.is_some()
    }

    /// What of its metrics is guessed.
    pub fn approx(&self) -> FontApprox {
        self.approx
    }

    /// What was read of the program it embeds.
    pub fn program(&self) -> FontProgram {
        self.program
    }

    /// What reading its streams decompressed, in bytes, charged to the budget it was
    /// read within: for who keeps a font and charges it again at each use, as
    /// [`Fonts::get`](crate::Fonts::get) does.
    pub fn cost(&self) -> usize {
        self.cost
    }

    /// The codes `s`, a string shown in it, is, in order, each with where it is in
    /// `s`, as PDFium splits them (`CPDF_CMap::GetNextChar`):
    /// - a simple font's are a byte each;
    /// - Identity-H's and Identity-V's two;
    /// - an embedded CMap's as its codespace says;
    /// - a predefined CMap's two bytes each, or a byte but where it leads two, as
    ///   PDFium's scheme for it says;
    /// - one the build hasn't as its `/ToUnicode`'s codespace says, as text is read,
    ///   else two bytes; a code that would run past the end is what's left.
    ///
    /// A code of two bytes that runs past the end has a 0 for the byte it hasn't, as
    /// PDFium reads it, and is where its one byte is.
    pub fn codes<'s>(&'s self, s: &'s [u8]) -> impl Iterator<Item = (Range<usize>, u32)> + 's {
        let mut at = 0;
        std::iter::from_fn(move || {
            let rest = s.get(at..).filter(|r| !r.is_empty())?;
            let (len, code) = match &self.codes {
                Codes::One => (1, u32::from(rest[0])),
                Codes::Identity => two_bytes(rest),
                Codes::Embedded(map) => map.next(rest),
                Codes::Predefined { scheme, .. } if scheme.split.leads(rest[0]) => two_bytes(rest),
                Codes::Predefined { .. } => (1, u32::from(rest[0])),
                Codes::Unknown => {
                    let len = self.decoder.code_len(rest).clamp(1, rest.len());
                    (len, number(&rest[..len]))
                }
            };
            at += len;
            Some((at - len..at, code))
        })
    }

    /// Whether `Tw` is added after `code`, `len` bytes of its string, as PDFium adds
    /// it: a code 32 in a simple font, or in a Type 0 font whose CMap sizes it a byte
    /// (`CPDF_CMap::GetCharSize`), by its value where its codes are mixed, and by
    /// `len` where its CMap isn't known.
    pub fn word_spaced(&self, code: u32, len: usize) -> bool {
        code == 32
            && match &self.codes {
                Codes::One => true,
                Codes::Identity => false,
                Codes::Embedded(map) => map.scheme != Scheme::TwoBytes,
                Codes::Predefined { scheme, .. } => scheme.split != Split::TwoBytes,
                Codes::Unknown => len == 1,
            }
    }

    /// The CID `code` selects in a Type 0 font, in 16 bits, as PDFium holds one: itself
    /// in Identity, or what the CMap maps it to, 0 where it maps it to none: an embedded
    /// one, over a predefined one it uses; a predefined one by the code's length as
    /// PDFium sizes it (`CPDF_CMap::GetCharSize`), two bytes in one of two bytes each,
    /// else by its value. None for a simple font, or where the CMap is unknown.
    pub fn cid(&self, code: u32) -> Option<u32> {
        match &self.codes {
            Codes::Identity => Some(code),
            Codes::Embedded(map) => Some(match map.cids.get(code) {
                Some((lo, cid)) => cid.wrapping_add(code - lo) & 0xffff,
                None => map.base_cid(code).unwrap_or(0),
            }),
            // Below the space, what Adobe's CMaps call `.notdef`, which PDFium's own
            // tables don't hold, so its CID is 0.
            Codes::Predefined { .. } if code < 0x20 => Some(0),
            Codes::Predefined { map, scheme } => {
                let len = match scheme.split {
                    Split::Lead(_) if code < 0x100 => 1,
                    _ => 2,
                };
                Some(map.lookup_cid_code(code, len).unwrap_or(0))
            }
            Codes::One | Codes::Unknown => None,
        }
    }

    /// How far `code` advances in horizontal writing, before character and word
    /// spacing: its `/Widths` entry, as the 16 bits PDFium holds it in, 0xFFFF being
    /// none, or its CID's `/W` or `/DW`.
    pub fn width(&self, code: u32) -> f32 {
        match &self.widths {
            Widths::Simple(widths) => widths.get(code as usize).copied().unwrap_or(0.0),
            Widths::Cid { w, dw } => match self.cid(code).and_then(|c| w.get(c)) {
                Some([width]) => width,
                None => *dw,
            },
            // PDFium's fixed widths for a font it reads as GBK (`ansi_widths_fixed_`).
            Widths::Gbk if code < 0x80 => match (32..127).contains(&code) {
                true => 500.0,
                false => 0.0,
            },
            Widths::Gbk => 1000.0,
        }
    }

    /// Whether `code`'s width is guessed where its font's others aren't
    /// ([`FontApprox::widths`]): in a standard 14 font drawn without a program or
    /// `/Widths`, a code whose glyph Adobe's metrics haven't, which PDFium draws as
    /// the face it draws in has it, or as the space where that hasn't either; give a
    /// width for and no box (the Euro, which Adobe's fonts haven't); or give a width
    /// PDFium's own face doesn't ([`Std14::drawn_otherwise`]): the face PDFium draws
    /// in, its own or the system's, says how wide. No box is known for it
    /// ([`Font::glyph_box`]).
    pub fn width_guessed(&self, code: u32) -> bool {
        code < 256 && self.guessed[code as usize / 64] >> (code % 64) & 1 == 1
    }

    /// Whether a code of `s`, a string shown in it, has its width guessed
    /// ([`Font::width_guessed`]): in a simple font, a byte of it.
    pub fn width_guessed_in(&self, s: &[u8]) -> bool {
        match self.codes {
            _ if self.guessed == [0; 4] => false,
            Codes::One => s.iter().any(|&b| self.width_guessed(u32::from(b))),
            _ => self.codes(s).any(|(_, code)| self.width_guessed(code)),
        }
    }

    /// A simple font's widths by code, where each of its codes is a byte: what
    /// [`Font::width`] gives each, for who reads many.
    pub fn byte_widths(&self) -> Option<&[f32; 256]> {
        match (&self.codes, &self.widths, &self.vertical) {
            (Codes::One, Widths::Simple(widths), None) => Some(widths),
            _ => None,
        }
    }

    /// Where it writes vertically, how `code`'s glyph is placed, each number in 16 bits
    /// as PDFium holds it: its CID's `/W2`, else `/DW2`'s advance and height, centred on
    /// its width as PDFium centres it. None where it writes horizontally.
    pub fn vertical(&self, code: u32) -> Option<VerticalMetrics> {
        let vertical = self.vertical.as_ref()?;
        let given = self.cid(code).and_then(|c| vertical.w2.get(c));
        Some(match given {
            Some([w1, vx, vy]) => VerticalMetrics { w1, vx, vy },
            None => VerticalMetrics {
                w1: vertical.w1,
                vx: f32::from((self.width(code) / 2.0).trunc() as i32 as i16),
                vy: vertical.vy,
            },
        })
    }

    /// How far above and below the baseline its glyphs reach, and its box.
    pub fn metrics(&self) -> FontMetrics {
        self.metrics
    }

    /// The box `code`'s glyph's outline fits in, from its origin, where it's known: a
    /// Type 3 glyph's from its CharProc's `d1`, or else its font's `/FontBBox`, empty
    /// for a code with no CharProc, which draws nothing; a standard 14 glyph's from
    /// Adobe's AFM, but one whose width is guessed ([`Font::width_guessed`]). None
    /// where only a font program says.
    pub fn glyph_box(&self, code: u32) -> Option<Rect> {
        *self.boxes.as_ref()?.get(code as usize)?
    }

    /// The box PDFium gives `code`'s glyph from the font's program
    /// (`CPDF_Font::GetCharBBox`), in glyph units, from its origin. None where it
    /// embeds no program that was read, as this version reads none.
    #[inline]
    pub fn char_box(&self, _code: u32) -> Option<Rect> {
        None
    }

    /// Each glyph `_s` shows drawn, where it's a CIDFont whose program's outlines are
    /// read as they're asked for ([`Font::char_box`]); and what drawing each cost
    /// charged to `_budget` where `_first`, given the glyph, says it's its first
    /// charge, so what shows them pays for the glyphs it shows whichever drew them
    /// first. Too large past it. Nothing, as this version reads no program.
    pub fn charge_outlines(
        &self,
        _s: &[u8],
        _first: impl FnMut(u32) -> bool,
        _budget: &mut usize,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Whether its program's outlines were read, so [`Font::char_box`] says its codes'
    /// boxes, but where it can't say of one. Not yet of any font.
    pub fn outlined(&self) -> bool {
        false
    }

    /// The text `code` is: its `/ToUnicode` mapping, or a simple font's encoding's; in
    /// a Type 0 font, failing a mapping, the code's, or its CID's in its collection,
    /// as PDFium reads them.
    pub fn code_text(&self, code: u32) -> Option<Cow<'_, str>> {
        (self.decoder.code_text(code)).or_else(|| self.cid_text(self.cid_text?, code))
    }

    /// The text `s`, a string shown in it, is: each code's ([`Font::code_text`]), its
    /// codes split as text is read, by its `/ToUnicode`'s codespace ranges where a
    /// composite font has one and its encoding isn't Identity, or as they're drawn
    /// ([`Font::codes`]) where a Type 0 font's text is only its codes' or CIDs'. None
    /// where it doesn't say the text of one of them.
    ///
    /// CID 0, `.notdef`, which a code the CMap maps no CID takes, has none here, though
    /// PDFium reads Adobe's U+FFFD for it ([`Font::code_text`]): a string of such codes
    /// is told from another by its bytes, not taken for the same text.
    pub fn text(&self, s: &[u8]) -> Option<String> {
        let Some(cid_text) = self.cid_text else {
            return self.decoder.text(s, |_| None);
        };
        if self.decoder.to_unicode {
            return (self.decoder).text(s, |code| self.string_cid_text(cid_text, code));
        }
        let mut text = String::new();
        for (_, code) in self.codes(s) {
            text.push_str(&self.string_cid_text(cid_text, code)?);
        }
        Some(text)
    }

    /// The text `code` is in a string by `cid_text`: what [`Font::cid_text`] says, but
    /// none for CID 0.
    fn string_cid_text(&self, cid_text: CidText, code: u32) -> Option<String> {
        match cid_text {
            CidText::Code => Some(String::from(char::from_u32(code).filter(|&c| c != '\0')?)),
            CidText::Cid(to_unicode) => {
                let cid = self.cid(code).filter(|&cid| cid != 0)?;
                match to_unicode.lookup_bf_string(cid)? {
                    BfString::Char(c) if c != '\0' => Some(String::from(c)),
                    BfString::String(s) if s != "\0" => Some(s),
                    _ => None,
                }
            }
        }
    }

    /// The text `code` is by `cid_text`: the code, or its CID's mapping; none for the
    /// code or the character 0, as PDFium has none.
    fn cid_text(&self, cid_text: CidText, code: u32) -> Option<Cow<'_, str>> {
        let text = match cid_text {
            CidText::Code => String::from(char::from_u32(code)?),
            CidText::Cid(to_unicode) => match to_unicode.lookup_bf_string(self.cid(code)?)? {
                BfString::Char(c) => String::from(c),
                BfString::String(s) => s,
            },
        };
        (text != "\0").then_some(Cow::Owned(text))
    }

    /// The most any of its metrics is from 0, in glyph units: its widths, vertical
    /// advances and origins, ascent and descent; infinite where one isn't finite. A
    /// glyph's box is that far from its origin at most, at a font size of 1000.
    pub fn extent(&self) -> f32 {
        self.extent
    }

    fn extent_of_metrics(&self) -> f32 {
        let widths: Vec<f32> = match &self.widths {
            Widths::Simple(widths) => widths.to_vec(),
            Widths::Cid { w, dw } => w.pieces.iter().map(|p| p.2[0]).chain([*dw]).collect(),
            Widths::Gbk => vec![1000.0],
        };
        let vertical = self.vertical.iter().flat_map(|v| {
            let w2 = v.w2.pieces.iter().flat_map(|p| p.2);
            w2.chain([v.vy, v.w1])
        });
        let heights = [self.metrics.ascent, self.metrics.descent];
        (widths.into_iter().chain(vertical).chain(heights)).fold(0.0, |most, v| match v.is_nan() {
            true => f32::INFINITY,
            false => most.max(v.abs()),
        })
    }
}

/// `font` by its `/Subtype`, as PDFium's `CPDF_Font::Create` takes it, its
/// `/ToUnicode` read first.
fn read_kind<'o, O, S>(
    reader: &Reader<'_, O, S>,
    font: &'o Dict,
    budget: &mut usize,
    programs: bool,
) -> Result<Font, Error>
where
    O: Resolve<'o> + Streams,
    S: Sink + ?Sized,
{
    let subtype = reader.byte_string(font, b"Subtype");
    let gbk = subtype == Some(b"TrueType") && reads_as_gbk(reader, font);
    let by_cid = gbk || subtype == Some(b"Type0");
    let encoded = (!by_cid).then(|| Encoded::read(reader, font, subtype));
    let decoder = Decoder::read(reader, font, encoded.as_ref(), budget)?;
    let simple_as = |kind, decoder, encoded| simple(reader, font, kind, decoder, encoded, programs);
    Ok(match (subtype, encoded) {
        (Some(b"Type0"), _) => type0(reader, font, decoder, budget, programs)?,
        (_, None) => gbk_font(reader, font, decoder, programs),
        (Some(b"Type3"), Some(encoded)) => type3(reader, font, decoder, &encoded, budget)?,
        (Some(b"MMType1"), Some(encoded)) => simple_as(FontKind::MMType1, decoder, &encoded),
        (Some(b"TrueType"), Some(encoded)) => simple_as(FontKind::TrueType, decoder, &encoded),
        (_, Some(encoded)) => {
            if subtype != Some(b"Type1") {
                reader.tell(None, font.get(b"Subtype"), Fault::Subtype);
            }
            simple_as(FontKind::Type1, decoder, &encoded)
        }
    })
}

/// What's read of the program a font embeds, where `programs` says it's to be read.
/// Nothing yet, whatever it says.
fn read_program(programs: bool) -> FontProgram {
    let _ = programs;
    FontProgram::None
}

/// The number of the object `dict`'s `key` refers to, for a fault's place.
fn referred(dict: &Dict, key: &[u8]) -> Option<ObjectId> {
    dict.get(key)?.as_reference()
}

/// A `/Flags` bit: a symbolic font (ISO 32000-2, 9.8.2).
const SYMBOLIC: i32 = 1 << 2;
/// A `/Flags` bit: a nonsymbolic font (ISO 32000-2, 9.8.2).
const NONSYMBOLIC: i32 = 1 << 5;
/// A `/Flags` bit: a font of capitals only (ISO 32000-2, 9.8.2).
const ALL_CAP: i32 = 1 << 16;

/// Whether a descriptor embeds a program, as PDFium's `LoadFontDescriptor` looks: one
/// of its three keys is a stream.
fn embeds<'o, O, S>(reader: &Reader<'_, O, S>, descriptor: Option<&'o Dict>) -> bool
where
    O: Resolve<'o> + Streams,
    S: Sink + ?Sized,
{
    descriptor.is_some_and(|d| {
        [&b"FontFile"[..], b"FontFile2", b"FontFile3"]
            .iter()
            .any(|key| reader.stream_for(d, key).is_some())
    })
}

/// The standard 14 font `name` is, as what's read as Type 1 takes it
/// (`CFX_StandardFont::GetStandardFontIndex`), which, as a face is chosen and
/// [`Std14::named`] isn't, doesn't read past a subset's tag.
fn standard_named(name: &[u8]) -> Option<Std14> {
    Std14::named(name).filter(|_| name.get(6) != Some(&b'+'))
}

/// Whether it's one of PDFium's fixed-pitch standard fonts.
fn fixed_pitch(font: Std14) -> bool {
    matches!(
        font,
        Std14::Courier | Std14::CourierBold | Std14::CourierOblique | Std14::CourierBoldOblique
    )
}

/// What PDFium holds where a simple font has no width: `/Widths` read as 16 bits may
/// come to the same, and it's 0 then too.
const NO_WIDTH: u16 = 0xffff;

/// The codes a font of capitals only draws as the code 32 before them, as PDFium's
/// `LoadCommon` has it.
const LOWER_CASE: [std::ops::RangeInclusive<usize>; 3] = [0x61..=0x7a, 0xe0..=0xf6, 0xf8..=0xfd];

/// A simple font's `/Widths`, as PDFium's `LoadCharWidths` reads them into a table of
/// 16 bits: from `/FirstChar` as far as `/LastChar` or its end, or as far as its end
/// where `/LastChar` is 0 or past it, `start`, or `/MissingWidth` where it has one, at
/// every other code. Each number is read as PDFium's accessors read it, and what comes
/// to [`NO_WIDTH`] is none.
fn given_widths<'o, O, S>(
    reader: &Reader<'_, O, S>,
    font: &'o Dict,
    widths: &'o [Object],
    start: u16,
    descriptor: Option<&'o Dict>,
    descriptor_id: Option<ObjectId>,
) -> [u16; 256]
where
    O: Resolve<'o> + Streams,
    S: Sink + ?Sized,
{
    let missing = descriptor
        .and_then(|d| d.get(b"MissingWidth"))
        .map(|value| reader.integer(value, descriptor_id) as u16);
    let mut table = [missing.unwrap_or(start); 256];
    let integer = |key: &[u8]| match font.get(key) {
        Some(value) => {
            let (integer, number) = reader.integer_and_whether_a_number(value);
            if !number {
                reader.tell(None, Some(value), Fault::NotANumber);
            }
            (integer, number)
        }
        None => (0, false),
    };
    let (first, first_given) = integer(b"FirstChar");
    let (last, last_given) = integer(b"LastChar");
    if !first_given || !(0..256).contains(&first) {
        reader.tell(None, font.get(b"FirstChar"), Fault::FirstChar);
    }
    if first_given && last_given && widths.len() as i64 != i64::from(last) - i64::from(first) + 1 {
        reader.tell(None, font.get(b"Widths"), Fault::WidthsLength);
    }
    // Each goes into a `size_t`, so one below 0 is past any code.
    let (first, last) = (first as usize, last as usize);
    if first < 256 {
        let end = first + widths.len();
        let last = match last {
            0 => end.wrapping_sub(1),
            last if last >= end => end.wrapping_sub(1),
            last => last,
        };
        let count = (last.min(255) + 1).saturating_sub(first);
        for (i, slot) in table[first..].iter_mut().take(count).enumerate() {
            *slot = reader.integer_at(widths, i, None) as u16;
        }
    }
    table
}

/// A Type 1, MMType1 or TrueType font (`CPDF_FaceBasedSimpleFont::LoadCommon`). A font
/// of capitals only has its lower case drawn as those, and advance as those, but where
/// it embeds a program, which isn't read.
fn simple<'o, O, S>(
    reader: &Reader<'_, O, S>,
    font: &'o Dict,
    kind: FontKind,
    decoder: Decoder,
    encoded: &Encoded<'o>,
    programs: bool,
) -> Font
where
    O: Resolve<'o> + Streams,
    S: Sink + ?Sized,
{
    let descriptor = encoded.descriptor;
    let descriptor_id = referred(font, b"FontDescriptor");
    let embedded = encoded.embedded;
    // What PDFium draws it in (`CFX_FontMapper::FindSubstFace`): Symbol, as TrueType, only
    // where its `/Flags` say it's symbolic.
    let std14 = encoded.base_font.and_then(Std14::named).filter(|&std14| {
        std14 != Std14::Symbol || kind != FontKind::TrueType || encoded.flags & SYMBOLIC != 0
    });
    let lower_cased = encoded.flags & ALL_CAP != 0 && !embedded;
    let mut approx = FontApprox::default();
    // A standard 14 glyph's metrics where no program is embedded, by the name its
    // encoding gives it. A glyph Adobe's metrics give no box (but the space's) or
    // haven't, or whose width PDFium's face gives otherwise, is guessed, and has none.
    let mut afm = std14.map(|std14| {
        let own = std14.own();
        // A code whose glyph the font hasn't draws as the space does, as in PDFium.
        let glyph = |code: u8| {
            let name = encoded.glyph_in(code, own)?;
            let bbox = std14.glyph_box(name)?;
            let drawn =
                (name == "space" || bbox != Rect::default()) && !std14.drawn_otherwise(name);
            Some((std14.glyph_width(name)?, drawn.then_some(bbox)))
        };
        let space = glyph(b' ').map_or((0.0, None), |(width, _)| (width, None));
        Box::new(std::array::from_fn::<_, 256, _>(|code| {
            glyph(code as u8).unwrap_or(space)
        }))
    });
    if let (Some(afm), true) = (&mut afm, lower_cased) {
        for code in LOWER_CASE.into_iter().flatten() {
            afm[code] = afm[code - 32];
        }
    }
    let given = reader.array_for(font, b"Widths");
    let has_widths = given.is_some();
    let widths = match given {
        Some(widths) => {
            // What PDFium reads as Type 1 of the fixed-pitch standard fonts starts at
            // their pitch.
            let fixed = kind != FontKind::TrueType
                && encoded
                    .base_font
                    .and_then(standard_named)
                    .is_some_and(fixed_pitch);
            let start = if fixed { 600 } else { NO_WIDTH };
            let mut table = given_widths(reader, font, widths, start, descriptor, descriptor_id);
            if lower_cased {
                for code in LOWER_CASE.into_iter().flatten() {
                    if table[code - 32] != 0 {
                        table[code] = table[code - 32];
                    }
                }
            }
            Box::new(table.map(|width| match width {
                NO_WIDTH => 0.0,
                width => f32::from(width),
            }))
        }
        None => match &afm {
            Some(afm) => {
                approx.widths = embedded;
                Box::new(afm.map(|(width, _)| width))
            }
            None => {
                reader.tell(None, font.get(b"Widths"), Fault::NoWidths);
                approx.widths = true;
                Box::new([0.0; 256])
            }
        },
    };
    approx.boxes = true;
    let (metrics, guessed) = heights(reader, descriptor, descriptor_id, std14);
    approx.heights = guessed;
    Font {
        id: None,
        kind,
        codes: Codes::One,
        widths: Widths::Simple(widths),
        vertical: None,
        metrics,
        guessed: match (&afm, embedded || has_widths) {
            (Some(afm), false) => bits(afm.iter().map(|(_, bbox)| bbox.is_none())),
            _ => [0; 4],
        },
        boxes: afm
            .filter(|_| !embedded)
            .map(|afm| Box::new(afm.map(|(_, bbox)| bbox))),
        program: read_program(programs),
        extent: 0.0,
        cost: 0,
        decoder,
        cid_text: None,
        approx,
    }
}

/// The first four bytes of a `/BaseFont` that PDFium reads as GBK in a TrueType font
/// with no TrueType program (`kChineseFontNames`): 宋体, 楷体, 黑体, 仿宋 and 新宋,
/// themselves in GBK.
const GBK_NAMES: [&[u8]; 5] = [
    b"\xCB\xCE\xCC\xE5",
    b"\xBF\xAC\xCC\xE5",
    b"\xBA\xDA\xCC\xE5",
    b"\xB7\xC2\xCB\xCE",
    b"\xD0\xC2\xCB\xCE",
];

/// Whether a TrueType font is one PDFium reads as GBK (`CPDF_Font::Create`).
fn reads_as_gbk<'o, O, S>(reader: &Reader<'_, O, S>, font: &'o Dict) -> bool
where
    O: Resolve<'o> + Streams,
    S: Sink + ?Sized,
{
    let tagged = reader
        .byte_string(font, b"BaseFont")
        .and_then(|name| name.get(..4))
        .is_some_and(|tag| GBK_NAMES.contains(&tag));
    let programmed = || {
        reader
            .dict(font, b"FontDescriptor")
            .is_some_and(|d| d.has(b"FontFile2"))
    };
    tagged && !programmed()
}

/// A TrueType font PDFium reads as GBK (`CPDF_CIDFont::LoadGB2312`): its codes by
/// GBK-EUC-H, its `/Widths` not read. Where the build hasn't that CMap, its codes are
/// guessed.
fn gbk_font<'o, O, S>(
    reader: &Reader<'_, O, S>,
    font: &'o Dict,
    decoder: Decoder,
    programs: bool,
) -> Font
where
    O: Resolve<'o> + Streams,
    S: Sink + ?Sized,
{
    let mut approx = FontApprox {
        boxes: true,
        ..FontApprox::default()
    };
    let name = b"GBK-EUC-H";
    let known = predefined::scheme(name).zip(predefined::bundled(name));
    let (codes, cid_text) = match known {
        Some((scheme, map)) => (
            Codes::Predefined { map, scheme },
            scheme.collection.to_unicode().map(CidText::Cid),
        ),
        None => {
            reader.tell(None, font.get(b"BaseFont"), Fault::PredefinedUnknown);
            approx.codes = true;
            (Codes::Unknown, None)
        }
    };
    let descriptor = reader.dict(font, b"FontDescriptor");
    let descriptor_id = referred(font, b"FontDescriptor");
    let (metrics, guessed) = heights(reader, descriptor, descriptor_id, None);
    approx.heights = guessed;
    Font {
        id: None,
        kind: FontKind::TrueType,
        cid_text,
        codes,
        widths: Widths::Gbk,
        vertical: None,
        metrics,
        boxes: None,
        program: read_program(programs),
        guessed: [0; 4],
        extent: 0.0,
        cost: 0,
        decoder,
        approx,
    }
}

/// 256 bits, a code each, those `set` says set.
fn bits(set: impl Iterator<Item = bool>) -> [u64; 4] {
    let mut bits = [0; 4];
    for (code, _) in set.enumerate().filter(|(_, set)| *set) {
        bits[code / 64] |= 1 << (code % 64);
    }
    bits
}

/// A Type 3 font (`CPDF_Type3Font::Load`): its glyphs drawn by its CharProcs, in glyph
/// space its `/FontMatrix` maps to text space, the identity where it has none or one
/// not of six numbers, as in PDFium. Its CharProcs decompress within `budget`.
fn type3<'o, O, S>(
    reader: &Reader<'_, O, S>,
    font: &'o Dict,
    decoder: Decoder,
    encoded: &Encoded<'o>,
    budget: &mut usize,
) -> Result<Font, Error>
where
    O: Resolve<'o> + Streams,
    S: Sink + ?Sized,
{
    let matrix = reader
        .array_for(font, b"FontMatrix")
        .filter(|m| m.len() == 6)
        .map_or(Affine::IDENTITY, |m| Affine {
            a: reader.float_at(m, 0),
            b: reader.float_at(m, 1),
            c: reader.float_at(m, 2),
            d: reader.float_at(m, 3),
            e: reader.float_at(m, 4),
            f: reader.float_at(m, 5),
        });
    // The box in glyph units as PDFium keeps it (`CPDF_Type3Font::Load`): its left,
    // bottom, right and top as given, scaled by the matrix's `a` and `d` only, in
    // thousandths, each cut to a whole number, so a matrix that turns glyph space over
    // leaves the top below the bottom; none where that leaves all four 0.
    let given = reader
        .array_for(font, b"FontBBox")
        .map(|b| {
            [(0, matrix.a), (1, matrix.d), (2, matrix.a), (3, matrix.d)]
                .map(|(i, scale)| (reader.float_at(b, i) * scale * 1000.0) as i32 as f32)
        })
        .filter(|sides| *sides != [0.0; 4]);
    let bbox = given.map(|[l, b, r, t]| Rect::new(l, b, r, t));
    let mut approx = FontApprox::default();
    // Each code's CharProc, None where it has none: each stream read once however many
    // codes and names draw it.
    let char_procs = reader.dict(font, b"CharProcs");
    let mut read: HashMap<ObjectId, Option<Option<CharProc>>> = HashMap::new();
    let mut procs: [Option<Option<CharProc>>; 256] = [None; 256];
    for (code, slot) in procs.iter_mut().enumerate() {
        let id = encoded
            .glyph_name(code as u8)
            .and_then(|name| char_procs?.get(name)?.as_reference());
        let Some(id) = id else {
            continue;
        };
        *slot = match read.get(&id) {
            Some(proc) => *proc,
            None => {
                let proc = match reader.stream(id) {
                    Some(_) => Some(char_proc(reader, id, budget)?),
                    None => None,
                };
                read.insert(id, proc);
                proc
            }
        };
    }
    // PDFium's `CFX_Matrix::GetXUnit`.
    let xunit = match (matrix.a, matrix.b) {
        (a, 0.0) => a.abs(),
        (0.0, b) => b.abs(),
        (a, b) => a.hypot(b),
    };
    let mut widths = Box::new([0.0; 256]);
    let first = reader.integer_for(font, b"FirstChar", 0, None);
    if (0..256).contains(&first) {
        if let Some(given) = reader.array_for(font, b"Widths") {
            for (slot, w) in widths[first as usize..].iter_mut().zip(given) {
                *slot = (reader.float(w) * matrix.a * 1000.0).round() as i32 as f32;
            }
        }
    } else {
        reader.tell(None, font.get(b"FirstChar"), Fault::FirstChar);
    }
    // A code `/Widths` gives no width advances as its CharProc says, in thousandths
    // rounded, then through the matrix, rounded on by a half and cut, as PDFium has it
    // (`CPDF_Type3Char`).
    for (width, proc) in widths.iter_mut().zip(&procs) {
        if *width == 0.0 {
            if let Some(Some(proc)) = proc {
                let thousandths = (proc.wx * 1000.0).round() as i32;
                *width = (thousandths as f32 * xunit + 0.5) as i32 as f32;
            }
        }
    }
    // PDFium's ascent is `A`'s top and its descent `g`'s bottom
    // (`CPDF_Font::CheckFontMetrics`), the box's top and bottom where the font has no
    // such glyph, held within the box where its top is above its bottom
    // (`GetLooseBounds`); a glyph whose `d1` gives no box has the box of what it draws,
    // which isn't read. Where the ascent is below the descent, a glyph's box still spans
    // the two.
    let metrics = match given {
        Some([_, bottom, _, top]) => {
            let mut edge = |code: usize, side: fn(Rect) -> f32, of_box: f32| match procs[code] {
                None => of_box,
                Some(proc) => match proc.and_then(|p| glyph_units(matrix, p.bbox)) {
                    Some(r) => side(r),
                    None => {
                        approx.heights = true;
                        of_box
                    }
                },
            };
            let (ascent, descent) = (edge(65, |r| r.y1, top), edge(103, |r| r.y0, bottom));
            let (ascent, descent) = match top > bottom {
                true => (ascent.min(top), descent.max(bottom)),
                false => (ascent, descent),
            };
            // Where they're one, PDFium bounds a glyph by its outline.
            approx.heights |= ascent == descent;
            FontMetrics {
                ascent,
                descent,
                bbox,
            }
        }
        None => {
            approx.heights = true;
            GUESSED
        }
    };
    let boxes = procs.map(|proc| match proc {
        None => Some(Rect::default()),
        Some(proc) => match proc.and_then(|p| glyph_units(matrix, p.bbox)) {
            Some(b) => Some(b),
            None => {
                approx.boxes |= bbox.is_none();
                bbox
            }
        },
    });
    Ok(Font {
        id: None,
        kind: FontKind::Type3,
        codes: Codes::One,
        widths: Widths::Simple(widths),
        vertical: None,
        metrics,
        boxes: Some(Box::new(boxes)),
        program: FontProgram::None,
        guessed: [0; 4],
        extent: 0.0,
        cost: 0,
        decoder,
        cid_text: None,
        approx,
    })
}

/// What a Type 3 glyph's CharProc says of it: `wx wy d0`, or `wx wy llx lly urx ury
/// d1`, with its box's numbers in that order, all 0 where it says none.
#[derive(Debug, Clone, Copy)]
struct CharProc {
    wx: f32,
    bbox: [f32; 4],
}

/// A `d1` box, `[llx, lly, urx, ury]`, in glyph units as PDFium keeps it
/// (`CPDF_Type3Char`): its numbers in thousandths, rounded, through `matrix`, the
/// font's, its translation as it is, and rounded again. None where the numbers rounded
/// have no area between them, as PDFium takes such a box for none.
fn glyph_units(matrix: Affine, d1: [f32; 4]) -> Option<Rect> {
    let [l, b, r, t] = d1.map(|v| (v * 1000.0).round());
    if r <= l || b >= t {
        return None;
    }
    let to = matrix.bounds(Rect::new(l, b, r, t));
    Some(Rect::new(
        to.x0.round(),
        to.y0.round(),
        to.x1.round(),
        to.y1.round(),
    ))
}

/// What CharProc `id`, its own number, says of its glyph, from its content decompressed
/// within `budget`, which it takes its size from. None where it can't be read.
fn char_proc<'o, O, S>(
    reader: &Reader<'_, O, S>,
    id: ObjectId,
    budget: &mut usize,
) -> Result<Option<CharProc>, Error>
where
    O: Resolve<'o> + Streams,
    S: Sink + ?Sized,
{
    let Some(data) = reader.charged(id, budget)? else {
        return Ok(None);
    };
    Ok(Some(said_of_glyph(&data, reader.limits)))
}

/// What `data`, a CharProc's content, says of its glyph ([`CharProc`]), as PDFium
/// takes it (`Handle_SetCharWidth`, `Handle_SetCachedDevice`): wherever in it a `d0` or
/// a `d1` is, each over what one before it said, a `d0` leaving a box that was said;
/// a number by `GetNumber`, so what's no number or isn't there is 0.
fn said_of_glyph(data: &[u8], limits: &Limits) -> CharProc {
    let none = ResCx::none();
    let mut said = [0.0; 6];
    for operation in operations(data, &none, &limits.content) {
        let Ok(operation) = operation else {
            break;
        };
        let n = |i: usize| operation.arg(i).map_or(0.0, |operand| operand.number());
        match operation.name() {
            b"d0" => (said[0], said[1]) = (n(1), n(0)),
            b"d1" => said = std::array::from_fn(|i| n(5 - i)),
            _ => {}
        }
    }
    let [wx, _, llx, lly, urx, ury] = said;
    CharProc {
        wx,
        bbox: [llx, lly, urx, ury],
    }
}

/// A Type 0 font: its codes by its `/Encoding` CMap, its CIDs' widths by its CIDFont.
/// An embedded CMap's chain decompresses within `budget`; a predefined CMap is read
/// where the build has it and PDFium knows it. A name it doesn't know is read as
/// PDFium reads it, two bytes each, each its CID.
///
/// PDFium doesn't load a font with no `/Encoding` of a name or a stream, or with other
/// than one CIDFont, and draws nothing in it. Each is told ([`Fault::CMap`],
/// [`Fault::DescendantFonts`]), and read as far as it reads.
fn type0<'o, O, S>(
    reader: &Reader<'_, O, S>,
    font: &'o Dict,
    decoder: Decoder,
    budget: &mut usize,
    programs: bool,
) -> Result<Font, Error>
where
    O: Resolve<'o> + Streams,
    S: Sink + ?Sized,
{
    let mut approx = FontApprox::default();
    let fonts = reader.array_for(font, b"DescendantFonts");
    let cid_font = fonts
        .filter(|fonts| fonts.len() == 1)
        .and_then(|fonts| reader.dict_at(&fonts[0]));
    let cid_font_id = fonts.and_then(|fonts| fonts.first()?.as_reference());
    if cid_font.is_none() {
        reader.tell(None, font.get(b"DescendantFonts"), Fault::DescendantFonts);
    }
    let subtype = cid_font.and_then(|f| reader.byte_string(f, b"Subtype"));
    let descendant = match subtype {
        Some(b"CIDFontType0") => Descendant::CidFontType0,
        Some(b"CIDFontType2") => Descendant::CidFontType2,
        _ => {
            if let Some(f) = cid_font {
                reader.tell(cid_font_id, f.get(b"Subtype"), Fault::Subtype);
            }
            Descendant::CidFontType2
        }
    };
    let encoding = font.get(b"Encoding");
    let written = encoding.map(|e| (e.as_reference(), reader.hop(e).map(Object::value)));
    let (codes, vertical) = match written {
        Some((_, Some(Value::Name(n)))) if n == b"Identity-H" => (Codes::Identity, false),
        Some((_, Some(Value::Name(n)))) if n == b"Identity-V" => (Codes::Identity, true),
        Some((Some(id), Some(Value::Stream(_)))) => {
            let map = CidMap::read(reader, id, &mut approx.codes, budget)?;
            let vertical = map.vertical;
            (Codes::Embedded(map), vertical)
        }
        // A predefined CMap writes vertically where its name ends in V, as PDFium
        // reads it.
        Some((_, Some(Value::Name(n)))) => {
            let codes = match predefined::scheme(n) {
                Some(scheme) => match predefined::bundled(n) {
                    Some(map) => Codes::Predefined { map, scheme },
                    None => {
                        reader.tell(None, encoding, Fault::PredefinedUnknown);
                        approx.codes = true;
                        Codes::Unknown
                    }
                },
                None => {
                    reader.tell(None, encoding, Fault::PredefinedUnknown);
                    Codes::Identity
                }
            };
            (codes, n.ends_with(b"V"))
        }
        _ => {
            reader.tell(None, encoding, Fault::CMap);
            approx.codes = true;
            (Codes::Unknown, false)
        }
    };
    let widths = match cid_font {
        Some(f) => Widths::Cid {
            w: cid_ranges(reader, f, b"W", cid_font_id, |n| n as f32),
            dw: f32::from(reader.integer_for(f, b"DW", 1000, cid_font_id) as i16),
        },
        None => {
            approx.widths = true;
            Widths::Cid {
                w: Ranges::default(),
                dw: 1000.0,
            }
        }
    };
    let vertical = vertical.then(|| {
        let dw2 = cid_font.and_then(|f| reader.array_for(f, b"DW2"));
        let at =
            |i: usize, default: i32| dw2.map_or(default, |d| reader.integer_at(d, i, cid_font_id));
        VerticalWidths {
            w2: cid_font
                .map(|f| cid_ranges(reader, f, b"W2", cid_font_id, |n| f32::from(n as i16)))
                .unwrap_or_default(),
            vy: f32::from(at(0, 880) as i16),
            w1: f32::from(at(1, -1000) as i16),
        }
    });
    let descriptor = cid_font.and_then(|f| reader.dict(f, b"FontDescriptor"));
    let descriptor_id = cid_font.and_then(|f| referred(f, b"FontDescriptor"));
    approx.boxes = true;
    // Identity's collection is the one its CIDFont orders.
    let ordered = || {
        let info = reader.dict(cid_font?, b"CIDSystemInfo")?;
        Collection::ordered(reader.byte_string(info, b"Ordering")?)
    };
    let cid_text = match &codes {
        Codes::Predefined { scheme, .. } if scheme.unicode => Some(CidText::Code),
        Codes::Predefined { scheme, .. } => scheme.collection.to_unicode().map(CidText::Cid),
        Codes::Identity => ordered().and_then(Collection::to_unicode).map(CidText::Cid),
        Codes::One | Codes::Embedded(_) | Codes::Unknown => None,
    };
    let (metrics, heights_guessed) = heights(reader, descriptor, descriptor_id, None);
    approx.heights = heights_guessed;
    Ok(Font {
        id: None,
        kind: FontKind::Type0(descendant),
        codes,
        widths,
        vertical,
        metrics,
        boxes: None,
        program: read_program(programs),
        guessed: [0; 4],
        extent: 0.0,
        cost: 0,
        decoder,
        cid_text,
        approx,
    })
}

/// A font's heights, as PDFium bounds a character (`cpdf_textpage.cpp`'s
/// `GetLooseBounds`, from `CPDF_Font::LoadFontDescriptor` and `CheckFontMetrics`), and
/// whether they're guessed: with a descriptor whose `/FontBBox` isn't all 0, its
/// `Ascent` and `Descent`, a `Descent` over 10 read as below the baseline, within the
/// box where its top is above its bottom. Else PDFium takes them from the face it draws
/// in, one it substitutes, its own for the standard 14 or the system's, which varies by
/// OS, and they're guessed: the descriptor's, else the standard 14 font's AFM's, else
/// the box's top and bottom, else an em above and a quarter below.
fn heights<'o, O, S>(
    reader: &Reader<'_, O, S>,
    descriptor: Option<&'o Dict>,
    descriptor_id: Option<ObjectId>,
    std14: Option<Std14>,
) -> (FontMetrics, bool)
where
    O: Resolve<'o> + Streams,
    S: Sink + ?Sized,
{
    let get = |key: &[u8]| descriptor.map_or(0, |d| reader.integer_for(d, key, 0, descriptor_id));
    let own = descriptor
        .and_then(|d| reader.array_for(d, b"FontBBox"))
        .map(|b| [0, 1, 2, 3].map(|i| reader.integer_at(b, i, descriptor_id) as f32))
        .filter(|sides| *sides != [0.0; 4]);
    let ascent = get(b"Ascent");
    let descent = match get(b"Descent") {
        descent if descent > 10 => -descent,
        descent => descent,
    };
    let given = ascent != 0 || descent != 0;
    let (ascent, descent) = (ascent as f32, descent as f32);
    let within = |(ascent, descent): (f32, f32)| match own {
        Some([_, bottom, _, top]) if top > bottom => (ascent.min(top), descent.max(bottom)),
        _ => (ascent, descent),
    };
    let own_box = own.map(|[l, b, r, t]| Rect::new(l, b, r, t));
    let bbox = own_box.or(std14.and_then(|f| f.metrics().bbox));
    let (ascent, descent) = match (own_box, given, std14) {
        (Some(_), true, _) => within((ascent, descent)),
        (_, true, _) => (ascent, descent),
        (_, false, Some(std14)) => {
            let m = std14.metrics();
            within((m.ascent, m.descent))
        }
        (Some(b), false, None) => (b.y1, b.y0),
        (None, false, None) => (GUESSED.ascent, GUESSED.descent),
    };
    let exact = own_box.is_some() && given;
    let metrics = FontMetrics {
        ascent,
        descent,
        bbox,
    };
    (metrics, !exact || ascent == descent)
}

/// A CIDFont's `/W` or `/W2`, the first range given for a CID winning, as in PDFium,
/// each number as `hold` holds it.
fn cid_ranges<'o, O, S, const N: usize>(
    reader: &Reader<'_, O, S>,
    cid_font: &'o Dict,
    key: &[u8],
    cid_font_id: Option<ObjectId>,
    hold: fn(i32) -> f32,
) -> Ranges<[f32; N]>
where
    O: Resolve<'o> + Streams,
    S: Sink + ?Sized,
{
    let array = reader.array_for(cid_font, key).unwrap_or_default();
    let cut = match key {
        b"W" => Fault::W,
        _ => Fault::W2,
    };
    let cut = |at: &Object| reader.tell(cid_font_id, Some(at), cut);
    Ranges::new(&metrics_array(reader, array, cid_font_id, cut, hold), false)
}

/// A `/W` or `/W2` array's entries, read as PDFium's `LoadMetricsArray` reads them:
/// `c [v …]` gives `c` and the CIDs after it values in turn, and `first last v` gives
/// the range one; `N` numbers a value. An array where a range's last CID should be ends
/// it, `cut` told; an element that's a reference to nothing is passed over; a CID below
/// 0 is none, one before an array's values included, so what follows it is still the
/// CID it would be.
fn metrics_array<'o, O, S, const N: usize>(
    reader: &Reader<'_, O, S>,
    array: &'o [Object],
    object: Option<ObjectId>,
    cut: impl Fn(&Object),
    hold: fn(i32) -> f32,
) -> Vec<(u32, u32, [f32; N])>
where
    O: Resolve<'o> + Streams,
    S: Sink + ?Sized,
{
    let cid = |n: i32| n.max(0) as u32;
    let mut out = Vec::new();
    let mut first = None;
    let mut last = None;
    let mut values = Vec::with_capacity(N);
    for item in array {
        let Some(item) = reader.hop(item) else {
            continue;
        };
        if let Some(list) = item.as_array() {
            let (Some(start), None) = (first.take(), last) else {
                cut(item);
                return out;
            };
            // Past what an `int` holds, PDFium passes it over.
            if i64::from(start) + list.len() as i64 > i64::from(i32::MAX) {
                continue;
            }
            for (at, chunk) in (start..).zip(list.chunks(N)) {
                let value = std::array::from_fn(|k| match chunk.get(k) {
                    Some(v) => hold(reader.integer(v, object)),
                    None => hold(0),
                });
                if at >= 0 {
                    out.push((cid(at), cid(at), value));
                }
            }
            continue;
        }
        let n = reader.integer(item, object);
        match (first, last) {
            (None, _) => first = Some(n),
            (Some(_), None) => last = Some(n),
            (Some(lo), Some(hi)) => {
                values.push(hold(n));
                if values.len() == N {
                    if hi >= 0 {
                        let value = std::array::from_fn(|k| values[k]);
                        out.push((cid(lo), cid(hi), value));
                    }
                    (first, last) = (None, None);
                    values.clear();
                }
            }
        }
    }
    out
}

/// A simple font's encoding as written: its base encoding, and the glyphs its
/// `/Differences` name over it; and of its descriptor what that rests on.
struct Encoded<'a> {
    /// PDFium's base encoding, none where that's the font program's own.
    base: Option<Encoding>,
    differences: [Option<&'a [u8]>; 256],
    /// Its descriptor, where it's read one.
    descriptor: Option<&'a Dict>,
    /// Its `/BaseFont`.
    base_font: Option<&'a [u8]>,
    /// Its descriptor's `/Flags`, PDFium's default where it has none.
    flags: i32,
    /// Whether its descriptor embeds a program.
    embedded: bool,
}

impl<'a> Encoded<'a> {
    /// The encoding of `font`, of `subtype`, as PDFium's `CPDF_Type1Font::Load` and
    /// `LoadCommon` come to it for what's read as Type 1 or TrueType, and
    /// `CPDF_Type3Font::Load` for Type 3, what's at fault in it told.
    fn read<O, S>(
        reader: &Reader<'_, O, S>,
        font: &'a Dict,
        subtype: Option<&'a [u8]>,
    ) -> Encoded<'a>
    where
        O: Resolve<'a> + Streams,
        S: Sink + ?Sized,
    {
        if subtype == Some(b"Type3") {
            let start = Encoded {
                base: None,
                differences: [None; 256],
                descriptor: None,
                base_font: None,
                flags: 0,
                embedded: false,
            };
            return start.load(reader, font, false, false, false);
        }
        let descriptor = reader.dict(font, b"FontDescriptor");
        let embedded = embeds(reader, descriptor);
        let true_type = subtype == Some(b"TrueType");
        let name = reader.byte_string(font, b"BaseFont");
        let std14 = name.and_then(standard_named).filter(|_| !true_type);
        let symbol_font = matches!(std14, Some(Std14::Symbol | Std14::ZapfDingbats));
        let flags = match (descriptor, std14) {
            (Some(d), _) => {
                let id = referred(font, b"FontDescriptor");
                reader.integer_for(d, b"Flags", NONSYMBOLIC, id)
            }
            (None, Some(_)) if symbol_font => SYMBOLIC,
            (None, Some(_)) => NONSYMBOLIC,
            (None, None) => 0,
        };
        let base = match std14 {
            Some(Std14::Symbol) => Some(Encoding::Symbol),
            Some(Std14::ZapfDingbats) => Some(Encoding::ZapfDingbats),
            Some(_) if flags & NONSYMBOLIC != 0 => Some(Encoding::Standard),
            _ => None,
        };
        let base = match flags & SYMBOLIC {
            0 => Some(Encoding::Standard),
            _ => base,
        };
        // PDFium's name for the font takes off a subset's tag where it embeds a program.
        let bare = name.map(|name| match (embedded, name.get(6)) {
            (true, Some(b'+')) => &name[7..],
            _ => name,
        });
        let symbol_named = std14 == Some(Std14::Symbol) || bare == Some(b"Symbol");
        let start = Encoded {
            base,
            differences: [None; 256],
            descriptor,
            base_font: name,
            flags,
            embedded,
        };
        start.load(reader, font, true, symbol_named, true_type && embedded)
    }

    /// PDFium's `LoadPDFEncoding` on what it starts from, `simple` for a font that's
    /// read as Type 1 or TrueType; `true_type` is whether it's drawn in a TrueType
    /// program, which so far is what its `/Subtype` says of one it embeds.
    /// `MacExpertEncoding` and `PDFDocEncoding` in a dictionary, which PDFium has tables
    /// of, leave the base as it was.
    fn load<O, S>(
        mut self,
        reader: &Reader<'_, O, S>,
        font: &'a Dict,
        simple: bool,
        symbol_named: bool,
        true_type: bool,
    ) -> Encoded<'a>
    where
        O: Resolve<'a> + Streams,
        S: Sink + ?Sized,
    {
        let written = font.get(b"Encoding");
        let encoding = written.and_then(|e| reader.hop(e));
        let own = matches!(self.base, Some(Encoding::Symbol | Encoding::ZapfDingbats));
        let symbolic = self.flags & SYMBOLIC != 0;
        let mut differences = None;
        match encoding.map(|e| (e, e.value())) {
            None => {
                if simple && symbol_named {
                    self.base = Some(Encoding::Symbol);
                } else if simple && !self.embedded && self.base.is_none() {
                    self.base = Some(Encoding::WinAnsi);
                }
            }
            Some((e, Value::Name(name))) => {
                if !own && !(symbolic && symbol_named) {
                    match &name[..] {
                        b"MacExpertEncoding" => self.base = Some(Encoding::WinAnsi),
                        name => self.named_base(reader, name, Some(e)),
                    }
                } else if !own && !true_type {
                    self.base = Some(Encoding::Symbol);
                }
            }
            Some((_, Value::Dict(d))) => {
                if !own {
                    match reader.byte_string(d, b"BaseEncoding") {
                        Some(b"MacExpertEncoding") if true_type => {
                            self.base = Some(Encoding::WinAnsi);
                        }
                        Some(b"MacExpertEncoding") => {}
                        Some(name) => self.named_base(reader, name, d.get(b"BaseEncoding")),
                        None if d.has(b"BaseEncoding") => {
                            reader.tell(None, d.get(b"BaseEncoding"), Fault::Encoding);
                        }
                        None => {}
                    }
                }
                if (!self.embedded || true_type) && self.base.is_none() {
                    self.base = Some(Encoding::Standard);
                }
                differences = d.get(b"Differences").and_then(|v| reader.array(v));
            }
            Some(_) => reader.tell(None, written, Fault::Encoding),
        }
        // PDFium's code is 32 bits unsigned, so a negative number comes back round to 0
        // after as many names as it's below it.
        let mut code = 0u32;
        for item in differences.into_iter().flatten() {
            let Some(element) = reader.hop(item) else {
                continue;
            };
            match element.as_name() {
                Some(name) => {
                    match self.differences.get_mut(code as usize) {
                        // An empty name leaves the code to its base encoding.
                        Some(slot) => *slot = Some(name).filter(|name| !name.is_empty()),
                        None => reader.tell(None, Some(item), Fault::Differences),
                    }
                    code = code.wrapping_add(1);
                }
                None => code = reader.integer_telling(element, None, Fault::Differences) as u32,
            }
        }
        self
    }

    /// `name`'s base encoding where PDFium's `GetPredefinedEncoding` knows it, told
    /// where it's no encoding's at all, leaving the base as it is otherwise.
    fn named_base<O, S>(&mut self, reader: &Reader<'_, O, S>, name: &[u8], at: Option<&'a Object>)
    where
        O: Resolve<'a> + Streams,
        S: Sink + ?Sized,
    {
        match name {
            b"WinAnsiEncoding" => self.base = Some(Encoding::WinAnsi),
            b"MacRomanEncoding" => self.base = Some(Encoding::MacRoman),
            b"StandardEncoding" => {}
            _ => reader.tell(None, at, Fault::Encoding),
        }
    }

    /// The name of the glyph `code` draws: the one `/Differences` gives it, or its base
    /// encoding's.
    fn glyph_name(&self, code: u8) -> Option<&'a [u8]> {
        match self.differences[code as usize] {
            Some(name) => Some(name),
            None => Some(self.base?.glyph(code)?.as_bytes()),
        }
    }

    /// [`Encoded::glyph_name`], where it's text, `own` the base where it has none.
    fn glyph_in(&self, code: u8, own: Encoding) -> Option<&'a str> {
        match self.differences[code as usize] {
            Some(name) => std::str::from_utf8(name).ok(),
            None => self.base.unwrap_or(own).glyph(code),
        }
    }
}

/// Where PDFium's table of a Symbol font's codes differs from its glyphs' names by the
/// Adobe Glyph List: it has the Greek letters, the list the increment, ohm and micro.
fn symbol_table_greek(code: u8) -> Option<char> {
    match code {
        0x44 => Some('\u{394}'),
        0x57 => Some('\u{3a9}'),
        0x6d => Some('\u{3bc}'),
        _ => None,
    }
}

/// How one font's strings read as text.
#[derive(Debug)]
struct Decoder {
    /// Bytes a code takes where the codespace ranges don't say.
    width: usize,
    /// Whether the codespace ranges say how long a code is.
    ranged: bool,
    /// Whether its font's `/ToUnicode` is a stream it read, whatever that maps.
    to_unicode: bool,
    cmap: CMap,
    /// A simple font's codes, by its encoding.
    encoded: Option<Box<[Option<String>]>>,
}

impl Decoder {
    /// `font`'s, its `/ToUnicode` decompressed within `budget`, charged to it; `encoded`
    /// its encoding where it's a simple font.
    fn read<'o, O, S>(
        reader: &Reader<'_, O, S>,
        font: &'o Dict,
        encoded: Option<&Encoded<'o>>,
        budget: &mut usize,
    ) -> Result<Decoder, Error>
    where
        O: Resolve<'o> + Streams,
        S: Sink + ?Sized,
    {
        let written = font.get(b"ToUnicode");
        let cmap = match written.and_then(Object::as_reference) {
            Some(id) => {
                let read = reader.decompressed(id, budget)?;
                read.map(|bytes| CMap::parse(&bytes, reader.limits))
            }
            None => None,
        };
        if written.is_some() && cmap.is_none() {
            reader.tell(None, written, Fault::ToUnicode);
        }
        Ok(Decoder {
            to_unicode: cmap.is_some(),
            ..Decoder::new(reader, font, encoded, cmap.unwrap_or_default())
        })
    }

    fn new<'o, O, S>(
        reader: &Reader<'_, O, S>,
        font: &'o Dict,
        encoded: Option<&Encoded<'o>>,
        cmap: CMap,
    ) -> Decoder
    where
        O: Resolve<'o> + Streams,
        S: Sink + ?Sized,
    {
        let Some(encoded) = encoded else {
            let name = font.get(b"Encoding").and_then(|e| reader.hop(e)?.as_name());
            let identity = matches!(name, Some(b"Identity-H" | b"Identity-V"));
            return Decoder {
                width: 2,
                ranged: !identity && cmap.has_codespace(),
                to_unicode: false,
                cmap,
                encoded: None,
            };
        };
        let type1 = !matches!(
            reader.byte_string(font, b"Subtype"),
            Some(b"TrueType" | b"Type3")
        );
        // PDFium reads `.notdef` as the space in what it reads as Type 1 of no symbols,
        // as it draws it (`CPDF_Type1Font::LoadGlyphMap`).
        let notdef_is_space = type1 && encoded.flags & SYMBOLIC == 0;
        // What PDFium leaves to a program, which isn't read, is taken for what's like
        // most programs' own encodings by the font's name.
        let bare = encoded.base_font.map(|name| match name.get(6) {
            Some(b'+') => &name[7..],
            _ => name,
        });
        let by_name = match bare {
            Some(name) if name.starts_with(b"Symbol") => Some(Encoding::Symbol),
            Some(b"ZapfDingbats" | b"Dingbats") => Some(Encoding::ZapfDingbats),
            _ => type1.then_some(Encoding::Standard),
        };
        let base = encoded.base.or(by_name);
        let dingbats =
            Some(Encoding::ZapfDingbats) == by_name || Some(Encoding::ZapfDingbats) == encoded.base;
        // A TrueType font named Symbol with no program, not said to be of no symbols, reads
        // by PDFium's table of codes, where others read by glyph names
        // (`CPDF_TrueTypeFont::LoadGlyphMap`).
        let by_table = !type1
            && reader.byte_string(font, b"Subtype") == Some(b"TrueType")
            && encoded.base == Some(Encoding::Symbol)
            && !encoded.embedded
            && encoded.flags & NONSYMBOLIC == 0;
        let encoded = (0..=255u8)
            .map(|code| match encoded.differences[code as usize] {
                Some(b".notdef") if notdef_is_space => Some(String::from(" ")),
                Some(n) => glyph_text(std::str::from_utf8(n).ok()?, dingbats),
                None => (by_table.then(|| symbol_table_greek(code)).flatten())
                    .map(String::from)
                    .or_else(|| base?.text(code)),
            })
            .collect();
        Decoder {
            width: 1,
            ranged: false,
            to_unicode: false,
            cmap,
            encoded: Some(encoded),
        }
    }

    /// `s`'s codes as text, `missed` saying a code's it has none for: None when the font
    /// doesn't say the text of one of them.
    fn text(&self, s: &[u8], missed: impl Fn(u32) -> Option<String>) -> Option<String> {
        let mut out = String::new();
        let mut at = 0;
        while at < s.len() {
            let rest = &s[at..];
            let len = self.code_len(rest).clamp(1, rest.len());
            let code = number(&rest[..len]);
            at += len;
            match self.code_text(code) {
                Some(text) => out.push_str(&text),
                None => out.push_str(&missed(code)?),
            }
        }
        Some(out)
    }

    /// The text `code` is: its `/ToUnicode` mapping, or its encoding's.
    fn code_text(&self, code: u32) -> Option<Cow<'_, str>> {
        match self.cmap.get(code) {
            Some(text) => Some(text),
            None => Some(Cow::Borrowed(
                self.encoded.as_ref()?.get(code as usize)?.as_deref()?,
            )),
        }
    }

    /// How many bytes the code `rest` starts with takes: the codespace range it's in, or
    /// the shortest one's, or the font's width.
    fn code_len(&self, rest: &[u8]) -> usize {
        if !self.ranged {
            return self.width;
        }
        self.cmap.code_len(rest).unwrap_or(self.width)
    }
}
