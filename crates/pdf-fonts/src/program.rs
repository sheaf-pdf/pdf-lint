// Parts of this file translate FreeType's code, Copyright (C) 1996-2023 by David
// Turner, Robert Wilhelm, and Werner Lemberg, under the FreeType Project License:
// `FTL.TXT`, and the crate's README for which parts.

//! The programs fonts embed: `/FontFile`, `/FontFile2` or `/FontFile3`, read as
//! PDFium's FreeType reads them, for what only a program says: each glyph's outline
//! box (`CPDF_Font::GetCharBBox`), a font's heights where its descriptor lacks them
//! (`CPDF_Font::CheckFontMetrics`), and the text of a code PDFium's encoding leaves to
//! it (`LoadGlyphMap`). TrueType outlines are skrifa's, CFF's and Type 1's read-fonts'
//! charstrings, each point placed where FreeType places it unscaled, through the
//! font's matrix.
//!
//! A program is told apart by its bytes, as FreeType tells it, whichever key
//! names it: an SFNT (TrueType, or OpenType over CFF), a Type 1 font, or a
//! bare CFF. One that can't be read is
//! [`FontProgram::Malformed`](crate::FontProgram::Malformed), and the font's boxes and
//! heights are guessed, as without a program. A code is
//! mapped to its glyph as PDFium maps it (`LoadGlyphMap`,
//! `GlyphFromCharCode`), through the encoding and FreeType's charmaps, its
//! format 2 and 4 `cmap`s read its way (translated from its code, under `FTL.TXT`,
//! as the README lists), and the glyphs' names; where that
//! rests on what isn't modelled (an encoding with no table here, two glyphs
//! one code point names), the code's box is unknown.
//!
//! Programs are untrusted, so their work is bounded: a TrueType glyph's
//! composites and points are counted before skrifa draws it; a CFF or
//! Type 1 glyph's charstrings are evaluated within fuel, its share of the
//! font's, an accented character's components charged for being looked up; and a
//! Type 1 program is scanned ahead of read-fonts, and read once ([`type1`]). A
//! glyph or a font past its bound has
//! its boxes unknown. What a program decompresses to is charged to the
//! budget its font is read within, and so is what drawing its glyphs
//! costs ([`Face::draw`]).

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use skrifa::MetadataProvider;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::model::Error as PsError;
use skrifa::raw::ps::cff::CffFontRef;
use skrifa::raw::ps::cff::index::Index;
use skrifa::raw::ps::charstring::{self, CharstringContext, CharstringKind, CommandSink};
use skrifa::raw::ps::type1::Type1Font;
use skrifa::raw::ps::{FontMatrix, PredefinedEncoding};
use skrifa::raw::tables::cmap::{CmapSubtable, PlatformId};
use skrifa::raw::tables::glyf::{Anchor, Glyph};
use skrifa::raw::types::{CFF_SFNT_VERSION, Fixed, GlyphId, Tag};
use skrifa::raw::{FileRef, FontRead, FontRef, TableProvider};

use crate::font::{Base, OwnText, Simple, SimpleMap};
use crate::{Encoding, Limits, Rect, glyph_text, name_code_point, type1};

/// A font's program, read, as FreeType opens it: its format, its units per
/// em, what its face says of its heights, and what drawing its glyphs
/// needs.
pub(crate) struct Face {
    data: Box<[u8]>,
    format: Format,
    upem: u16,
    /// Its bounding box, ascender and descender, in its own units, as
    /// FreeType gives them (`FT_FaceRec`).
    bbox: [i32; 4],
    ascender: i32,
    descender: i32,
    /// Whether its glyphs' boxes are PDFium's: not where FreeType draws
    /// them hinted (a tricky TrueType font), where its units per em aren't
    /// FreeType's, or where a Type 1 font's charstrings aren't found as
    /// read-fonts finds them.
    exact: bool,
    /// Whether its units per em are FreeType's, so its heights are.
    units: bool,
    /// An SFNT's glyphs, as FreeType counts them (`num_glyphs`): `maxp`'s,
    /// or fewer where `loca` holds fewer.
    glyphs: u32,
    /// How much charstring a CFF or Type 1 glyph may evaluate.
    fuel: usize,
    /// The most glyphs a TrueType glyph's composites may draw, and how deep.
    composites: (u64, u8),
    /// A Type 1 program read, once, as it's opened: read-fonts' font, and
    /// what the scan found of it ([`type1::read`]).
    type1: Option<(Type1Font, type1::Scanned)>,
    /// A bare CID-keyed CFF's glyph by CID, as FreeType maps them
    /// (`cff_charset_cid_to_gindex`): the first its charset gives the CID,
    /// None where none does. FreeType keeps none for one in an OpenType font, whose
    /// CIDs are its glyphs (`cff_font_load`).
    cids: Option<Box<[Option<u16>]>>,
}

/// A program's format, as FreeType tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    /// TrueType outlines in an SFNT.
    Glyf,
    /// CFF: bare, or an OpenType font's `CFF ` table at this range.
    Cff {
        at: usize,
        len: usize,
        sfnt: bool,
    },
    Type1,
}

/// A program open for reading, borrowing its bytes, or its [`Face`]'s
/// Type 1 font.
enum View<'a> {
    Glyf(FontRef<'a>),
    Cff {
        cff: CffFontRef<'a>,
        sfnt: Option<FontRef<'a>>,
    },
    Type1 {
        font: &'a Type1Font,
        scanned: &'a type1::Scanned,
    },
}

/// What drawing a glyph came to.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Drawn {
    /// The box its outline's points and control points fit in, in the
    /// program's units, as FreeType's control box has it: all 0 for one
    /// that draws nothing.
    Box([i32; 4]),
    /// FreeType can't load it: no such glyph.
    Missing,
    /// Unknown: what FreeType makes of it can't be said here.
    Unknown,
}

impl Face {
    /// `data`, a font's program, read under `limits`, its format told apart as
    /// FreeType tells it; None where it can't be read. `named` is the font's name in
    /// the PDF, for FreeType's tricky fonts. A Type 1 program is read here, once.
    pub(crate) fn open(mut data: Vec<u8>, named: &[u8], limits: &Limits) -> Option<Face> {
        // FreeType takes this tag for TrueType's (`tt_face_init`), which read-fonts doesn't.
        if data.starts_with(&[0, 2, 0, 0]) {
            data[1] = 1;
        }
        let format = format(&data)?;
        let mut face = Face {
            data: Box::default(),
            format,
            upem: 1000,
            bbox: [0; 4],
            ascender: 0,
            descender: 0,
            exact: true,
            units: true,
            glyphs: 0,
            fuel: limits.glyph_fuel,
            composites: (limits.composite_glyphs, limits.composite_depth),
            type1: None,
            cids: None,
        };
        if let Some(font) = sfnt(&data) {
            face.sfnt(&font, named)?;
        }
        let fuel =
            |glyphs: u32| (limits.glyph_fuel).max(limits.font_fuel / (glyphs.max(1) as usize));
        match format {
            Format::Glyf => {}
            Format::Cff { .. } => {
                let Some(View::Cff { cff, sfnt }) = view(&data, format, face.upem) else {
                    return None;
                };
                if sfnt.is_none() {
                    let bbox = cff.metadata()?.bbox();
                    face.bbox = rounded_out([bbox.x_min, bbox.y_min, bbox.x_max, bbox.y_max]);
                    (face.ascender, face.descender) = (face.bbox[3], face.bbox[1]);
                    // A bare CFF's units are its top dictionary's matrix's,
                    // 1000 where it has none (`cff_face_init`); an OpenType
                    // font's are its `head`'s.
                    let units = cff.matrix().map_or(Ok(1000), |m| u16::try_from(m.scale));
                    face.units = units.is_ok();
                    face.upem = units.unwrap_or(1000);
                    face.exact &= face.units;
                }
                face.fuel = fuel(cff.num_glyphs());
                if sfnt.is_none() && cff.is_cid() {
                    face.cids = Some(cids(&cff).unwrap_or_default());
                }
            }
            // Scanned first, as read-fonts reads it, so it's handed only a
            // program it reads within bounds, and its charstrings are had for fuel of
            // ours: read-fonts' own evaluation of a glyph, whose charstrings it keeps,
            // bounds its operations and not what its accents and subroutines read.
            Format::Type1 => {
                let scanned = type1::read(&data, limits)?;
                let font = Type1Font::new(&data).ok()?;
                let bbox = font.bbox();
                face.bbox = rounded_out([bbox.x_min, bbox.y_min, bbox.x_max, bbox.y_max]);
                (face.ascender, face.descender) = (face.bbox[3], face.bbox[1]);
                // Its units are its matrix's (`t1_parse_font_matrix`).
                let units = u16::try_from(font.upem());
                face.units = units.is_ok();
                face.upem = units.unwrap_or(1000);
                face.exact &= face.units && charstrings_agree(&font, &scanned.charstrings);
                face.fuel = fuel(font.num_glyphs());
                face.type1 = Some((font, scanned));
            }
        }
        face.data = data.into_boxed_slice();
        Some(face)
    }

    /// An SFNT's units per em, box and heights, as FreeType reads them
    /// (`sfnt_load_face`), and whether it's a tricky font, whose glyphs
    /// FreeType draws hinted for PDFium. None where FreeType wouldn't
    /// load it: no `head`, `hhea` or `hmtx`, or units per em it refuses.
    fn sfnt(&mut self, font: &FontRef<'_>, named: &[u8]) -> Option<()> {
        let head = font.head().ok()?;
        let hhea = font.hhea().ok()?;
        font.hmtx().ok()?;
        let hhea_metrics = hhea.number_of_h_metrics();
        self.upem = head.units_per_em();
        if !(16..=16384).contains(&self.upem) {
            return None;
        }
        self.bbox = [head.x_min(), head.y_min(), head.x_max(), head.y_max()].map(i32::from);
        // Where `loca` is shorter than `maxp` says, FreeType reads it on into
        // what follows it, where it can, and else takes the glyphs it has
        // (`tt_face_load_loca`).
        self.glyphs = font.maxp().map_or(0, |m| u32::from(m.num_glyphs()));
        if let Some(loca) = font
            .table_directory
            .table_records()
            .iter()
            .find(|r| r.tag() == Tag::new(b"loca"))
        {
            let shift = if head.index_to_loc_format() != 0 {
                2
            } else {
                1
            };
            let entries = (loca.length() as usize).min(0x10000 << shift) >> shift;
            if entries < self.glyphs as usize + 1 {
                let records = font.table_directory.table_records();
                let next = records
                    .iter()
                    .map(|r| r.offset())
                    .filter(|&o| o > loca.offset())
                    .min();
                let room = next
                    .map_or(font.data().len(), |n| n as usize)
                    .saturating_sub(loca.offset() as usize);
                match (self.glyphs as usize + 1) << shift <= room {
                    true => self.exact = false,
                    false => self.glyphs = entries.saturating_sub(1) as u32,
                }
            }
        }
        let os2 = font.os2().ok();
        let typo = os2.as_ref().map(|o| {
            (
                i32::from(o.s_typo_ascender()),
                i32::from(o.s_typo_descender()),
            )
        });
        let win = os2.as_ref().map(|o| {
            (
                i32::from(o.us_win_ascent() as i16),
                -i32::from(o.us_win_descent() as i16),
            )
        });
        let hhea = (
            i32::from(hhea.ascender().to_i16()),
            i32::from(hhea.descender().to_i16()),
        );
        let use_typo = os2
            .as_ref()
            .is_some_and(|o| o.fs_selection().bits() & 0x80 != 0);
        (self.ascender, self.descender) = match (use_typo, typo, win) {
            (true, Some(typo), _) => typo,
            _ if hhea != (0, 0) => hhea,
            (_, Some(typo), _) if typo != (0, 0) => typo,
            (_, _, Some(win)) => win,
            _ => hhea,
        };
        self.exact &= !tricky(font, named);
        if self.format == Format::Glyf {
            // FreeType takes a `loca` as long for any format but 0, read-fonts only for
            // 1; and with long metrics that don't fit `hmtx`, or none, FreeType reads on
            // as far as the table goes where skrifa has no glyph's side bearing, which
            // places its outline.
            let metrics = usize::from(hhea_metrics) * 4;
            let room = font.table_data(Tag::new(b"hmtx")).map_or(0, |t| t.len());
            self.exact &= matches!(head.index_to_loc_format(), 0 | 1);
            self.exact &= metrics != 0 && metrics <= room;
            // FreeType reads a `cmap` as far as it goes, where read-fonts has none of one
            // whose records don't fit, so which glyph a code draws isn't known.
            self.exact &= match font.cmap() {
                Ok(cmap) => usize::from(cmap.num_tables()) == cmap.encoding_records().len(),
                Err(_) => font.table_data(Tag::new(b"cmap")).is_none(),
            };
        }
        Some(())
    }

    /// The program open, borrowing its bytes, or its Type 1 font read.
    fn view(&self) -> Option<View<'_>> {
        match &self.type1 {
            Some((font, scanned)) => Some(View::Type1 { font, scanned }),
            None => view(&self.data, self.format, self.upem),
        }
    }

    /// Its box's left, bottom, right and top, as it gives them, and its heights, in
    /// glyph units, as PDFium normalizes them (`CheckFontMetrics`); None where its
    /// units per em aren't FreeType's.
    pub(crate) fn heights(&self) -> Option<([f32; 4], f32, f32)> {
        if !self.units {
            return None;
        }
        let n = |v: i32| normalize(i64::from(v), self.upem) as f32;
        Some((self.bbox.map(n), n(self.ascender), n(self.descender)))
    }

    /// Whether its glyphs' boxes are PDFium's ([`Face::exact`]).
    pub(crate) fn exact(&self) -> bool {
        self.exact
    }

    /// Whether FreeType takes it for an SFNT, as PDFium asks (`CFX_Font::IsTTFont`).
    pub(crate) fn is_sfnt(&self) -> bool {
        matches!(self.format, Format::Glyf | Format::Cff { sfnt: true, .. })
    }

    /// The box PDFium gives a simple font's code drawing glyph `gid` of it,
    /// in glyph units (`CPDF_SimpleFont::LoadCharMetrics`): its outline's
    /// box, normalized; `-1` on every side where FreeType can't load it, as
    /// PDFium leaves it. None where that can't be said. With it, what drawing
    /// it cost ([`Face::draw`]).
    fn simple_box(&self, view: &View<'_>, gid: u32) -> (Option<Rect>, usize) {
        let (drawn, cost) = self.draw(view, gid);
        let found = match drawn {
            Drawn::Box(b) => Some(self.normalized(b)),
            Drawn::Missing => Some(Rect::new(-1.0, -1.0, -1.0, -1.0)),
            Drawn::Unknown => None,
        };
        (found, cost)
    }

    /// The box PDFium gives a CIDFont's code drawing glyph `gid` of it, in
    /// glyph units (`CFX_Face::GetCharBBox`): its outline's box, normalized,
    /// its top raised by a 64th of itself, as PDFium raises it; all 0 where
    /// FreeType can't load it, or it's no glyph. None where that can't be said.
    /// With it, what drawing it cost ([`Face::draw`]).
    fn cid_box(&self, view: &View<'_>, gid: Option<u32>) -> (Option<Rect>, usize) {
        let Some(gid) = gid else {
            return (Some(Rect::default()), 0);
        };
        let (drawn, cost) = self.draw(view, gid);
        let found = match drawn {
            Drawn::Box(b) => {
                let [x0, y0, x1, y1] = b.map(|v| normalize(i64::from(v), self.upem));
                // PDFium's `int` sum, held within one.
                let top = match y1 <= MAX_RECT_TOP {
                    true => y1.saturating_add(y1 / 64),
                    false => i32::MAX,
                };
                Some(Rect::new(x0 as f32, y0 as f32, x1 as f32, top as f32))
            }
            Drawn::Missing => Some(Rect::default()),
            Drawn::Unknown => None,
        };
        (found, cost)
    }

    fn normalized(&self, b: [i32; 4]) -> Rect {
        let [x0, y0, x1, y1] = b.map(|v| normalize(i64::from(v), self.upem) as f32);
        Rect::new(x0, y0, x1, y1)
    }

    /// Glyph `gid` drawn as FreeType loads it unscaled
    /// (`FT_LOAD_NO_SCALE`), its points' control box; and what drawing it
    /// cost, in bytes: of charstring evaluated, or of glyph data read
    /// counting a TrueType glyph's composites, and one a point and a
    /// component drawn.
    fn draw(&self, view: &View<'_>, gid: u32) -> (Drawn, usize) {
        if !self.exact {
            return (Drawn::Unknown, 0);
        }
        match view {
            View::Glyf(font) => draw_glyf(font, gid, self.glyphs, self.composites),
            View::Cff { cff, .. } => draw_cff(cff, self.cids.as_deref(), gid, self.upem, self.fuel),
            View::Type1 { font, scanned } => draw_type1(font, scanned, gid, self.fuel),
        }
    }
}

/// `data`, an SFNT or CFF program of `format` and `upem` units per em,
/// open, borrowing it. None where it doesn't parse as its format, or it's a
/// Type 1 program, which only [`Face::open`] reads.
fn view(data: &[u8], format: Format, upem: u16) -> Option<View<'_>> {
    Some(match format {
        Format::Glyf => View::Glyf(sfnt(data)?),
        Format::Cff {
            at,
            len,
            sfnt: true,
        } => View::Cff {
            cff: CffFontRef::new(data.get(at..at + len)?, 0, Some(i32::from(upem))).ok()?,
            sfnt: Some(sfnt(data)?),
        },
        Format::Cff { sfnt: false, .. } => View::Cff {
            cff: CffFontRef::new(data, 0, None).ok()?,
            sfnt: None,
        },
        // Read once, as the face is opened.
        Format::Type1 => return None,
    })
}

/// A CIDFont's program, its glyphs drawn as they're asked for, each once.
pub(crate) struct CidGlyphs {
    face: Face,
    /// Its `/CIDToGIDMap` stream's data: each CID's glyph in two bytes,
    /// big-endian. None where it has none, and each CID is its glyph.
    to_gid: Option<Box<[u8]>>,
    /// The boxes drawn so far, by glyph, each with what drawing it cost.
    drawn: Mutex<HashMap<u32, (Option<Rect>, usize)>>,
}

impl std::fmt::Debug for CidGlyphs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CidGlyphs")
            .field("format", &self.face.format)
            .finish_non_exhaustive()
    }
}

impl CidGlyphs {
    pub(crate) fn new(face: Face, to_gid: Option<Vec<u8>>) -> CidGlyphs {
        CidGlyphs {
            face,
            to_gid: to_gid.map(Vec::into_boxed_slice),
            drawn: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn face(&self) -> &Face {
        &self.face
    }

    /// The box PDFium gives a code of CID `cid` (`CPDF_CIDFont::GetCharBBox`):
    /// its glyph's ([`Face::cid_box`]), the glyph its `/CIDToGIDMap` gives
    /// it, none past the map's end, or the CID itself, as PDFium maps an
    /// embedded program's (`GlyphFromCharCode`), the CID kept in 16 bits as
    /// PDFium keeps it.
    pub(crate) fn char_box(&self, cid: u32) -> Option<Rect> {
        self.drawn(cid).1
    }

    /// CID `cid`'s glyph, u32::MAX for none ([`CidGlyphs::char_box`]), its
    /// box, and what drawing it cost ([`Face::draw`]): drawn the first time
    /// it's asked for, and the same whoever asks first, so the cost can be
    /// charged to each page that shows it.
    pub(crate) fn drawn(&self, cid: u32) -> (u32, Option<Rect>, usize) {
        let cid = usize::from(cid as u16);
        let gid = match &self.to_gid {
            Some(map) => map
                .get(cid * 2..cid * 2 + 2)
                .map(|g| u32::from(u16::from_be_bytes([g[0], g[1]]))),
            None => Some(cid as u32),
        };
        let key = gid.unwrap_or(u32::MAX);
        let lock = || self.drawn.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(&(found, cost)) = lock().get(&key) {
            return (key, found, cost);
        }
        // Drawn without the lock, so threads sharing the font don't wait on
        // each other's glyphs; two drawing one draw it alike.
        let (found, cost) = match (gid, self.face.exact) {
            (Some(_), false) => (None, 0),
            _ => match self.face.view() {
                Some(view) => self.face.cid_box(&view, gid),
                None => (None, 0),
            },
        };
        lock().insert(key, (found, cost));
        (key, found, cost)
    }
}

/// Past this, PDFium doesn't raise a CIDFont glyph's top
/// (`CFX_Face::GetCharBBox`), and takes it for the most an `int` holds.
const MAX_RECT_TOP: i32 = 2_114_445_437;

/// `value`, in a program's units, in glyph units, as PDFium normalizes a
/// metric (`NormalizeFontMetric`): a thousand times it, and half `upem`
/// in whole units, over `upem`, cut to its whole part, held within an
/// `int`; as it is where `upem` is 0.
fn normalize(value: i64, upem: u16) -> i32 {
    if upem == 0 {
        return value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    }
    let scaled = (value as f64 * 1000.0 + f64::from(upem / 2)) / f64::from(upem);
    // As `saturated_cast`: cut, and held within an `int`.
    scaled as i32
}

/// What format `data` is, as FreeType tries its drivers: an SFNT (a
/// collection's first font), with TrueType outlines or a CFF table; a Type
/// 1 font, plain or PFB; else bare CFF. None for none of them.
fn format(data: &[u8]) -> Option<Format> {
    if let Some(font) = sfnt(data) {
        // By its tag, whatever tables it has: `OTTO` asks for a CFF (`cff_face_init`), and
        // any other that TrueType's reader takes for `glyf` and `loca` (`tt_face_init`).
        if font.table_directory.sfnt_version() != CFF_SFNT_VERSION {
            return (font.glyf().is_ok() && font.loca(None).is_ok()).then_some(Format::Glyf);
        }
        let cff = font.table_data(Tag::new(b"CFF "))?;
        let at = cff.as_bytes().as_ptr() as usize - data.as_ptr() as usize;
        return Some(Format::Cff {
            at,
            len: cff.len(),
            sfnt: true,
        });
    }
    let type1 = |d: &[u8]| d.starts_with(b"%!PS-AdobeFont") || d.starts_with(b"%!FontType");
    if type1(data) || data.starts_with(&[0x80, 0x01]) && data.get(6..).is_some_and(type1) {
        return Some(Format::Type1);
    }
    (data.first() == Some(&1)).then_some(Format::Cff {
        at: 0,
        len: data.len(),
        sfnt: false,
    })
}

/// `data` as an SFNT: its first font, where it's a collection.
fn sfnt(data: &[u8]) -> Option<FontRef<'_>> {
    match FileRef::new(data).ok()? {
        FileRef::Font(font) => Some(font),
        FileRef::Collection(fonts) => fonts.get(0).ok(),
    }
}

/// A CFF or Type 1 box in 16.16, in whole units as FreeType rounds a face's
/// box out: its least sides down, its greatest up.
fn rounded_out(b: [Fixed; 4]) -> [i32; 4] {
    let down = |v: Fixed| v.to_bits() >> 16;
    let up = |v: Fixed| ((i64::from(v.to_bits()) + 0xFFFF) >> 16) as i32;
    [down(b[0]), down(b[1]), up(b[2]), up(b[3])]
}

/// Whether FreeType takes an SFNT for a tricky font, whose glyphs it draws
/// hinted (`tt_check_trickyness_family`): its family's name, or `named`,
/// its name in the PDF, holding one of FreeType's tricky fonts' names,
/// past a subset's tag. FreeType also knows such fonts by checksums of
/// their hinting tables; those that keep no name are missed.
fn tricky(font: &FontRef<'_>, named: &[u8]) -> bool {
    let holds_trick = |name: &str| {
        let name = match name.as_bytes().get(6) {
            Some(b'+') => &name[7.min(name.len())..],
            _ => name,
        };
        TRICK_NAMES.iter().any(|t| name.contains(t))
    };
    let families = [
        skrifa::string::StringId::FAMILY_NAME,
        skrifa::string::StringId::TYPOGRAPHIC_FAMILY_NAME,
    ];
    let in_program = families
        .into_iter()
        .flat_map(|id| font.localized_strings(id))
        .any(|s| holds_trick(&s.to_string()));
    in_program || std::str::from_utf8(named).is_ok_and(holds_trick)
}

/// The fonts FreeType draws hinted whatever it's asked (`trick_names`):
/// old CJK fonts whose glyphs their hinting assembles.
const TRICK_NAMES: [&str; 20] = [
    "cpop",
    "DFGirl-W6-WIN-BF",
    "DFGothic-EB",
    "DFGyoSho-Lt",
    "DFHei",
    "DFHSGothic-W5",
    "DFHSMincho-W3",
    "DFHSMincho-W7",
    "DFKaiSho-SB",
    "DFKaiShu",
    "DFKai-SB",
    "DFMing",
    "DLC",
    "HuaTianKaiTi?",
    "HuaTianSongTi?",
    "Ming(for ISO10646)",
    "MingLiU",
    "MingMedium",
    "PMingLiU",
    "MingLi43",
];

/// A TrueType glyph's outline box, as skrifa draws it unscaled, FreeType's
/// way, once its composites are known to draw few glyphs and points;
/// missing where it, or a glyph it draws, is past the font's `glyphs`, as
/// FreeType fails it (`load_truetype_glyph`). With it, what drawing it cost
/// ([`Face::draw`]).
fn draw_glyf(font: &FontRef<'_>, gid: u32, glyphs: u32, most: (u64, u8)) -> (Drawn, usize) {
    if gid >= glyphs {
        return (Drawn::Missing, 0);
    }
    let located = font.loca(None).ok().zip(font.glyf().ok());
    let Some((loca, glyf)) = located else {
        return (Drawn::Unknown, 0);
    };
    // An empty glyph, or one with no contours, draws nothing; one too
    // short for its header FreeType fails (`load_truetype_glyph`).
    let data = font
        .table_data(Tag::new(b"glyf"))
        .map_or(&[][..], |d| d.as_bytes());
    let (at, len) = glyph_location(&loca, data.len(), gid);
    let contours = data
        .get(at..at + 2)
        .map(|c| i16::from_be_bytes([c[0], c[1]]));
    match (len, contours) {
        (0, _) | (_, Some(0)) => return (Drawn::Box([0; 4]), len),
        (1..10, _) => return (Drawn::Missing, len),
        _ => {}
    }
    let mut counted = HashMap::new();
    let mut read = 0;
    let (nodes, points) = match composite_glyphs(
        &loca,
        &glyf,
        (glyphs, most.1),
        gid,
        0,
        &mut counted,
        &mut read,
    ) {
        None => return (Drawn::Missing, read),
        Some(drawn) => drawn,
    };
    // skrifa draws no more points than a `u16` counts, failing a glyph
    // past that only once it's made room for them all. Its own bound on
    // components, `MAX_COMPOSITE_EDGES`, is why `outlines.get` can say none
    // of a glyph within `most` that a caller has raised.
    if nodes > most.0 || points > u64::from(u16::MAX) {
        return (Drawn::Unknown, read);
    }
    let cost = read + (nodes + points) as usize;
    let outlines = font.outline_glyphs();
    let Some(glyph) = outlines.get(GlyphId::new(gid)) else {
        return (Drawn::Unknown, cost);
    };
    let mut pen = Pen::default();
    let settings = DrawSettings::unhinted(Size::unscaled(), LocationRef::default());
    let drawn = match glyph.draw(settings, &mut pen) {
        // A glyph with data that isn't a composite of empty ones, and no point drawn,
        // doesn't parse here, or a component doesn't. Whether FreeType fails it too, by a
        // parser of its own, can't be told, and skrifa draws it as nothing.
        Ok(_) if pen.bbox.is_none() && (points > 0 || nodes == 1) => Drawn::Unknown,
        Ok(_) => pen.bbox().map_or(Drawn::Unknown, Drawn::Box),
        Err(_) => Drawn::Unknown,
    };
    (drawn, cost)
}

/// Where glyph `gid`'s data is in a `glyf` table `glyf_len` long, and how
/// long it is, as FreeType reads `loca` (`tt_face_get_location`): none
/// where it starts past the table, or ends past it but for the last glyph,
/// which ends with the table; to the table's end where its end is before
/// its start.
fn glyph_location(
    loca: &skrifa::raw::tables::loca::Loca<'_>,
    glyf_len: usize,
    gid: u32,
) -> (usize, usize) {
    let gid = gid as usize;
    let Some(start) = loca.get_raw(gid).map(|p| p as usize) else {
        return (0, 0);
    };
    let mut end = loca.get_raw(gid + 1).map_or(start, |p| p as usize);
    if start > glyf_len {
        return (0, 0);
    }
    if end > glyf_len {
        match gid + 1 == loca.len() {
            true => end = glyf_len,
            false => return (0, 0),
        }
    }
    (start, end.checked_sub(start).unwrap_or(glyf_len - start))
}

/// How many glyphs glyph `gid` draws, `depth` composites in, past `most` deep being more
/// than any may, itself and its components' counted, saturating, and how many points:
/// each glyph counted once at each depth, so the count costs what the font holds, not
/// what it draws; the bytes of glyph data the count reads added to `read`.
/// None where FreeType fails it (`load_truetype_glyph`,
/// `TT_Process_Composite_Component`): it, or a component, is past
/// `glyphs`, or a component placed by matching points names a point its
/// glyph hasn't, or the composite before it hasn't.
fn composite_glyphs(
    loca: &skrifa::raw::tables::loca::Loca<'_>,
    glyf: &skrifa::raw::tables::glyf::Glyf<'_>,
    (glyphs, most): (u32, u8),
    gid: u32,
    depth: u16,
    counted: &mut HashMap<(u32, u16), Option<(u64, u64)>>,
    read: &mut usize,
) -> Option<(u64, u64)> {
    if gid >= glyphs {
        return None;
    }
    if depth > u16::from(most) {
        return Some((u64::MAX, 0));
    }
    if let Some(&n) = counted.get(&(gid, depth)) {
        return n;
    }
    // What's read of the table for it: an entry past the table reads nothing, whatever
    // it claims.
    let (start, end) = (loca.get_raw(gid as usize), loca.get_raw(gid as usize + 1));
    let table = glyf.offset_data().len();
    *read += end
        .zip(start)
        .filter(|&(e, s)| s <= e && e as usize <= table)
        .map_or(0, |(e, s)| (e - s) as usize);
    let n = match loca.get_glyf(GlyphId::new(gid), glyf) {
        Ok(Some(Glyph::Composite(composite))) => {
            let mut drawn = (1u64, 0u64);
            for component in composite.components() {
                let glyph = component.glyph.to_u32();
                let Some((nodes, points)) =
                    composite_glyphs(loca, glyf, (glyphs, most), glyph, depth + 1, counted, read)
                else {
                    counted.insert((gid, depth), None);
                    return None;
                };
                if let Anchor::Point { base, component } = component.anchor
                    && (u64::from(base) >= drawn.1 || u64::from(component) >= points)
                {
                    counted.insert((gid, depth), None);
                    return None;
                }
                drawn = (
                    drawn.0.saturating_add(nodes),
                    drawn.1.saturating_add(points),
                );
            }
            Some(drawn)
        }
        // What doesn't parse, contours and no points among it, is counted a point it can't
        // draw, so a composite of it and empty glyphs isn't taken for one that draws nothing.
        Ok(Some(Glyph::Simple(simple))) => match simple.num_points() as u64 {
            0 if simple.number_of_contours() != 0 => Some((1, 1)),
            points => Some((1, points)),
        },
        Ok(None) => Some((1, 0)),
        Err(_) => Some((1, 1)),
    };
    counted.insert((gid, depth), n);
    n
}

/// The box a skrifa outline's points fit in: every point a path command
/// names, control points among them.
#[derive(Default)]
struct Pen {
    bbox: Option<[f32; 4]>,
}

impl Pen {
    fn add(&mut self, x: f32, y: f32) {
        let b = self.bbox.get_or_insert([x, y, x, y]);
        *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    }

    /// In whole units, as unscaled TrueType points are; all 0 for none.
    fn bbox(&self) -> Option<[i32; 4]> {
        let b = self.bbox.unwrap_or_default();
        let whole =
            |v: f32| (v.is_finite() && v.fract() == 0.0 && v.abs() < 1e9).then_some(v as i32);
        Some([whole(b[0])?, whole(b[1])?, whole(b[2])?, whole(b[3])?])
    }
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.add(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.add(x, y);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.add(cx0, cy0);
        self.add(x, y);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.add(cx0, cy0);
        self.add(cx1, cy1);
        self.add(x, y);
    }

    fn close(&mut self) {}
}

/// The box charstring points fit in, each placed where FreeType's CFF and
/// Type 1 engine places it unscaled (`cf2_glyphpath_hintPoint`, then
/// `ps_builder_add_point`): a 64th of it rounded, in 16.16, then cut to
/// whole units down. A move is drawn only once a line or curve follows it,
/// and a line that goes nowhere is passed over unless a hint mask was set
/// since the last, as FreeType's engine has them (`cf2_glyphpath_lineTo`).
#[derive(Default)]
struct Points {
    bbox: Option<[i32; 4]>,
    at: (Fixed, Fixed),
    moved: Option<(Fixed, Fixed)>,
    masked: bool,
    /// The font's matrix, normalized, which FreeType applies to each point
    /// (`FT_Outline_Transform`), then its offset in whole units.
    matrix: Option<FontMatrix>,
    /// The 16.16 scale FreeType forces on a CID-keyed font's glyph whose
    /// font dictionary has units of its own, each point multiplied by it
    /// (`FT_MulFix`).
    scale: Option<i64>,
}

impl Points {
    fn add(&mut self, x: Fixed, y: Fixed) {
        let (mut x, mut y) = (i64::from(unit(x)), i64::from(unit(y)));
        if let Some(m) = self.matrix {
            let e = |v: Fixed| i64::from(v.to_bits());
            (x, y) = (
                mul_fix(x, e(m.xx)) + mul_fix(y, e(m.xy)) + e(m.dx),
                mul_fix(x, e(m.yx)) + mul_fix(y, e(m.yy)) + e(m.dy),
            );
        }
        if let Some(s) = self.scale {
            (x, y) = (mul_fix(x, s), mul_fix(y, s));
        }
        let whole = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        let (x, y) = (whole(x), whole(y));
        let b = self.bbox.get_or_insert([x, y, x, y]);
        *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    }

    /// The move pending, drawn.
    fn draw_move(&mut self) {
        if let Some((x, y)) = self.moved.take() {
            self.add(x, y);
        }
    }

    fn bbox(&self) -> [i32; 4] {
        self.bbox.unwrap_or_default()
    }
}

/// A charstring coordinate in whole units as FreeType's engine has it
/// unscaled: `FT_MulFix` by a 64th, then shifted down by 10.
fn unit(v: Fixed) -> i32 {
    (mul_fix(i64::from(v.to_bits()), 0x400) >> 10) as i32
}

/// `a` times `b`, a 16.16 number, as FreeType multiplies them
/// (`FT_MulFix`): the magnitudes', rounded, signed.
fn mul_fix(a: i64, b: i64) -> i64 {
    let product =
        ((u128::from(a.unsigned_abs()) * u128::from(b.unsigned_abs()) + 0x8000) >> 16) as i64;
    if (a < 0) != (b < 0) {
        -product
    } else {
        product
    }
}

/// `a` times `b` over `c`, as FreeType works it (`FT_MulDiv`): the
/// magnitudes', rounded, signed; the most an `int` holds over 0.
fn mul_div(a: i64, b: i64, c: i64) -> i64 {
    let (n, d) = (
        u128::from(a.unsigned_abs()) * u128::from(b.unsigned_abs()),
        u128::from(c.unsigned_abs()),
    );
    let quotient = match d {
        0 => 0x7FFF_FFFF,
        _ => ((n + d / 2) / d).min(i64::MAX as u128) as i64,
    };
    if ((a < 0) != (b < 0)) != (c < 0) {
        -quotient
    } else {
        quotient
    }
}

impl CommandSink for Points {
    fn move_to(&mut self, x: Fixed, y: Fixed) {
        self.moved = Some((x, y));
        self.at = (x, y);
    }

    fn line_to(&mut self, x: Fixed, y: Fixed) {
        if self.at == (x, y) && !self.masked {
            return;
        }
        self.masked = false;
        self.draw_move();
        self.add(x, y);
        self.at = (x, y);
    }

    fn curve_to(&mut self, cx0: Fixed, cy0: Fixed, cx1: Fixed, cy1: Fixed, x: Fixed, y: Fixed) {
        self.masked = false;
        self.draw_move();
        self.add(cx0, cy0);
        self.add(cx1, cy1);
        self.add(x, y);
        self.at = (x, y);
    }

    fn close(&mut self) {}

    fn hint_mask(&mut self, _: &[u8]) {
        self.masked = true;
    }
}

/// A CFF glyph's box, its charstring evaluated within `fuel` bytes:
/// `gid` taken for a CID in a CID-keyed font, its glyph `cids` gives, and
/// its points scaled from its font dictionary's units to the font's
/// `upem`, as FreeType takes and scales them (`cff_slot_load`). With it,
/// the charstring evaluated.
fn draw_cff(
    cff: &CffFontRef<'_>,
    cids: Option<&[Option<u16>]>,
    gid: u32,
    upem: u16,
    fuel: usize,
) -> (Drawn, usize) {
    let gid = match cids {
        Some(cids) if gid != 0 => match cids.get(gid as usize).copied().flatten() {
            Some(g) => u32::from(g),
            None => return (Drawn::Missing, 0),
        },
        _ if gid >= cff.num_glyphs() => return (Drawn::Missing, 0),
        _ => gid,
    };
    let glyph = GlyphId::new(gid);
    let Some(subfont) = cff
        .subfont_index(glyph)
        .and_then(|i| cff.subfont(i, &[]).ok())
    else {
        return (Drawn::Unknown, 0);
    };
    // The matrix FreeType applies, normalized (`cff_face_init`): the top
    // dictionary's, or a CID-keyed font's dictionary's of its own, under
    // the top's where it has one, its units scaled to the top's.
    let top = cff.matrix().copied();
    let top_units = top.map_or(i32::from(upem), |m| m.scale);
    let (matrix, scale) = match (cff.is_cid(), subfont.matrix()) {
        (true, Some(&sub)) => {
            let sub = match top {
                Some(top) => top.combine(&sub),
                None => sub.normalize(),
            };
            let scale = (sub.scale != top_units)
                .then(|| mul_div(0x10000, i64::from(top_units), i64::from(sub.scale)));
            (Some(sub.matrix), scale)
        }
        _ => (top.map(|m| m.matrix), None),
    };
    let subrs = match subfont.subrs_offset() {
        0 => Index::Empty,
        at => match cff
            .data()
            .get(at as usize..)
            .and_then(|d| Index::new(d, false).ok())
        {
            Some(index) => index,
            None => return (Drawn::Unknown, 0),
        },
    };
    let Some(charstring) = cff.charstrings().get(gid as usize) else {
        return (Drawn::Unknown, 0);
    };
    let context = Type2 {
        cff,
        subrs: &subrs,
        fuel: Fuel(Cell::new(fuel.saturating_sub(charstring.len()))),
    };
    let mut sink = Points {
        matrix: matrix.filter(|&m| m != FontMatrix::IDENTITY),
        scale,
        ..Points::default()
    };
    evaluated(&context, &context.fuel, charstring, fuel, &mut sink)
}

/// `charstring` evaluated in `context`, `left` its fuel, what's left of
/// `fuel` once the charstring's charged: its points' box, or unknown where
/// it fails, runs out of fuel or passes the evaluator's own million
/// operations (FreeType's has no such bound); and how much of `fuel` it spent.
fn evaluated(
    context: &impl CharstringContext,
    left: &Fuel,
    charstring: &[u8],
    fuel: usize,
    sink: &mut Points,
) -> (Drawn, usize) {
    let drawn =
        match left.0.get() > 0 && charstring::evaluate(context, None, charstring, sink).is_ok() {
            true => Drawn::Box(sink.bbox()),
            false => Drawn::Unknown,
        };
    (drawn, fuel - left.0.get())
}

/// A CID-keyed CFF's glyph by CID ([`Face::cids`]), as FreeType maps them
/// (`cff_charset_compute_cids`): the lowest glyph its charset gives each.
/// None where it has no charset.
fn cids(cff: &CffFontRef<'_>) -> Option<Box<[Option<u16>]>> {
    let charset = cff.charset()?;
    let mut cids = Vec::new();
    for (gid, cid) in charset.iter() {
        let (gid, cid) = (gid.to_u32() as u16, usize::from(cid.to_u16()));
        if cids.len() <= cid {
            cids.resize(cid + 1, None);
        }
        cids[cid].get_or_insert(gid);
    }
    Some(cids.into_boxed_slice())
}

/// Charstring a glyph may yet evaluate, in bytes: its subroutines and
/// accented components count as they're called, and looking a component up.
struct Fuel(Cell<usize>);

impl Fuel {
    /// `data`, once it's charged; an error past what's left.
    fn burn<'d>(&self, data: &'d [u8]) -> Result<&'d [u8], PsError> {
        self.spend(data.len())?;
        Ok(data)
    }

    /// `n` charged; an error past what's left.
    fn spend(&self, n: usize) -> Result<(), PsError> {
        match self.0.get().checked_sub(n) {
            Some(left) if left > 0 => {
                self.0.set(left);
                Ok(())
            }
            _ => {
                self.0.set(0);
                Err(PsError::LimitExceeded)
            }
        }
    }
}

/// A CFF glyph's charstring context, within its fuel: the tuple read-fonts
/// evaluates CFF charstrings with, each subroutine and component charged.
struct Type2<'a, 'f> {
    cff: &'f CffFontRef<'a>,
    subrs: &'f Index<'a>,
    fuel: Fuel,
}

impl CharstringContext for Type2<'_, '_> {
    fn kind(&self) -> CharstringKind {
        CharstringKind::Type2
    }

    /// An accented character's base and accent, by standard code, as
    /// read-fonts finds them: through the charset, or ISOAdobe's.
    fn seac_components(&self, base_code: i32, accent_code: i32) -> Result<[&[u8]; 2], PsError> {
        let charset = self.cff.charset().ok_or(PsError::MissingResource)?;
        let charstring = |code: i32| {
            let sid = u8::try_from(code)
                .ok()
                .and_then(|c| PredefinedEncoding::Standard.sid(c));
            let gid = sid
                .and_then(|sid| charset.glyph_id(sid))
                .ok_or(PsError::InvalidOperation)?;
            let found = self.cff.charstrings().get(gid.to_u32() as usize);
            self.fuel.burn(found.ok_or(PsError::Malformed)?)
        };
        let accent = charstring(accent_code)?;
        Ok([charstring(base_code)?, accent])
    }

    fn global_subr(&self, index: i32) -> Result<&[u8], PsError> {
        let globals = self.cff.global_subrs();
        let found = globals.get((index + globals.subr_bias()) as usize);
        self.fuel.burn(found.ok_or(PsError::Malformed)?)
    }

    fn subr(&self, index: i32) -> Result<&[u8], PsError> {
        let found = self.subrs.get((index + self.subrs.subr_bias()) as usize);
        self.fuel.burn(found.ok_or(PsError::Malformed)?)
    }
}

/// A Type 1 glyph's box, its charstring, as the program gives it by the
/// glyph's name, evaluated within `fuel` bytes; with it, the charstring
/// evaluated.
fn draw_type1(font: &Type1Font, scanned: &type1::Scanned, gid: u32, fuel: usize) -> (Drawn, usize) {
    if gid >= font.num_glyphs() {
        return (Drawn::Missing, 0);
    }
    let found = font
        .glyph_name(GlyphId::new(gid))
        .and_then(|n| scanned.charstrings.get(n.as_bytes()));
    let Some(Some(charstring)) = found else {
        return (Drawn::Unknown, 0);
    };
    let context = Type1 {
        font,
        build_char_len: scanned.build_char_len,
        fuel: Fuel(Cell::new(fuel.saturating_sub(charstring.len()))),
    };
    let mut sink = Points {
        matrix: Some(font.matrix()).filter(|&m| m != FontMatrix::IDENTITY),
        ..Points::default()
    };
    evaluated(&context, &context.fuel, charstring, fuel, &mut sink)
}

/// Whether the charstrings the scan found are read-fonts': each glyph's that a
/// standard code names, as read-fonts finds it for an accented character
/// (`seac_components`), the scan's for that name.
fn charstrings_agree(font: &Type1Font, charstrings: &type1::Charstrings) -> bool {
    (0..=255u8).all(|code| {
        let Some(name) = Encoding::Standard.glyph(code) else {
            return true;
        };
        let theirs = font
            .seac_components(code.into(), code.into())
            .ok()
            .map(|[base, _]| base);
        match charstrings.get(name.as_bytes()) {
            Some(Some(ours)) => theirs == Some(&ours[..]),
            Some(None) => true,
            None => theirs.is_none(),
        }
    })
}

/// A Type 1 glyph's charstring context, within its fuel: the font's own,
/// each subroutine and component charged.
struct Type1<'f> {
    font: &'f Type1Font,
    build_char_len: usize,
    fuel: Fuel,
}

impl CharstringContext for Type1<'_> {
    fn kind(&self) -> CharstringKind {
        CharstringKind::Type1
    }

    /// An accented character's components, each looked up among all the glyphs by its
    /// name, one by one, and a charstring may ask for as many as it has room for: a
    /// look-up is a far smaller thing than a byte of charstring evaluated, so a sixteenth
    /// of a byte a glyph, and one in a font of the most glyphs a charstring can name is
    /// within [`Limits::glyph_fuel`].
    fn seac_components(&self, base_code: i32, accent_code: i32) -> Result<[&[u8]; 2], PsError> {
        self.fuel.spend(self.font.num_glyphs() as usize / 8)?;
        let [base, accent] = self.font.seac_components(base_code, accent_code)?;
        Ok([self.fuel.burn(base)?, self.fuel.burn(accent)?])
    }

    fn global_subr(&self, _: i32) -> Result<&[u8], PsError> {
        Err(PsError::MissingResource)
    }

    fn subr(&self, index: i32) -> Result<&[u8], PsError> {
        self.fuel.burn(self.font.subr(index)?)
    }

    fn weight_vector(&self) -> &[Fixed] {
        self.font.weight_vector()
    }

    fn build_char_len(&self) -> usize {
        self.build_char_len
    }
}

/// The glyph PDFium's `glyph_index_` holds for a code: one, `0xffff` for
/// none, or unknown here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gid {
    Glyph(u16),
    None,
    Unknown,
}

impl Gid {
    /// What FreeType's glyph index `g` is held as, a `uint16_t`.
    fn of(g: u32) -> Gid {
        match g as u16 {
            0xffff => Gid::None,
            g => Gid::Glyph(g),
        }
    }
}

/// A code's glyph name, as PDFium takes it (`GetAdobeCharName`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named<'a> {
    Name(&'a [u8]),
    NoName,
    /// From an encoding with no table here: MacExpertEncoding's, PDFDocEncoding's.
    Unknown,
}

impl Face {
    /// What PDFium has of a simple font once it's mapped its codes to this program's
    /// glyphs (`LoadGlyphMap`, `LoadCommon`): each code's box
    /// (`CPDF_SimpleFont::GetCharBBox`), each glyph drawn once, None for a code that
    /// can't be said of; what drawing them cost between them ([`Face::draw`]); the
    /// lower case of a font of capitals that takes its capitals' glyphs; and the text
    /// PDFium's encoding has from the program.
    pub(crate) fn simple_map(&self, font: &Simple<'_>) -> SimpleMap {
        let mut map = SimpleMap::none();
        let Some(view) = self.view() else {
            return map;
        };
        let mut charmaps = Charmaps::of(&view, self.glyphs);
        let encoding = PdfiumEncoding {
            base: font.base,
            differences: font.differences,
        };
        let (mut gids, text) = match font.truetype {
            true => truetype_gids(&mut charmaps, font, &encoding),
            false => type1_gids(&mut charmaps, font, &encoding),
        };
        map.text = text;
        // An all-caps font's small letters without a glyph of their own draw
        // the capitals' (`LoadCommon`).
        if font.flags & 1 << 16 != 0 {
            for code in (b'a'..=b'z').chain(0xe0..=0xf6).chain(0xf8..=0xfd) {
                let code = code as usize;
                if gids[code] == Gid::None {
                    gids[code] = gids[code - 32];
                    map.capitals[code] = true;
                }
            }
        }
        // Where its boxes aren't PDFium's, none is said, what has no glyph among them.
        if !self.exact {
            return map;
        }
        let mut drawn: HashMap<u16, Option<Rect>> = HashMap::new();
        let mut cost = 0usize;
        for (slot, gid) in map.boxes.iter_mut().zip(gids) {
            *slot = match gid {
                Gid::Glyph(g) => *drawn.entry(g).or_insert_with(|| {
                    let (found, spent) = self.simple_box(&view, u32::from(g));
                    cost = cost.saturating_add(spent);
                    found
                }),
                Gid::None => Some(Rect::new(-1.0, -1.0, -1.0, -1.0)),
                Gid::Unknown => None,
            };
        }
        map.cost = cost;
        map
    }
}

/// A simple font's encoding as `Encoded` has read it, as PDFium keeps it for an embedded
/// program: its base, and the names its `/Differences` give, where it has them.
struct PdfiumEncoding<'a> {
    base: Base,
    differences: Option<&'a [Option<&'a [u8]>; 256]>,
}

impl PdfiumEncoding<'_> {
    /// `code`'s glyph name, as PDFium takes it (`GetAdobeCharName`): its
    /// `/Differences` name, or its base encoding's.
    fn name(&self, code: u8) -> Named<'_> {
        if let Some(name) = self.differences.and_then(|d| d[code as usize]) {
            return Named::Name(name);
        }
        let name = match (self.base, code) {
            (_, ..32) => None,
            (Base::Builtin | Base::MsSymbol, _) => None,
            (Base::MacExpert | Base::PdfDoc, _) => return Named::Unknown,
            (base, _) => base.table().and_then(|table| table.glyph(code)),
        };
        name.map_or(Named::NoName, |n| Named::Name(n.as_bytes()))
    }
}

/// The code point PDFium reads glyph name `name` as
/// (`UnicodeFromAdobeName`), 0 for none.
fn unicode_of(name: &[u8]) -> u32 {
    name_code_point(name).map_or(0, |(u, _)| u)
}

/// The codes PDFium's table of MacRomanEncoding (`kMacRomanEncoding`) leaves 0: the Mac
/// symbols and the Apple logo.
const MAC_TABLE_NONE: [u8; 15] = [
    0xad, 0xb0, 0xb2, 0xb3, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xbd, 0xc3, 0xc5, 0xc6, 0xd7, 0xf0,
];

/// A code's text by a program's Mac charmap, as PDFium has it
/// (`UnicodeFromAppleRomanCharCode`): by its glyph's name, but none for those its table
/// leaves 0. 0xC1 is the inverted exclamation mark, where the table has the pound sign.
fn apple_roman(code: u8) -> Option<String> {
    match MAC_TABLE_NONE.contains(&code) {
        true => None,
        false => Encoding::MacRoman.text(code),
    }
}

/// The text a glyph's name in a program stands for: its code point as PDFium reads it
/// (`UnicodeFromAdobeName`), else as a name of joined parts is read. PDFium's table of
/// names holds Dingbats' too.
fn name_text(name: &[u8]) -> Option<String> {
    match unicode_of(name) {
        0 => glyph_text(std::str::from_utf8(name).ok()?, true),
        unicode => char::from_u32(unicode).map(String::from),
    }
}

/// `make` of each code as what its text is.
fn each_text(make: impl Fn(u8) -> Option<String>) -> OwnText {
    (0..=255u8).map(|code| Some(make(code))).collect()
}

/// A TrueType font's codes' glyphs, as PDFium maps them
/// (`CPDF_TrueTypeFont::LoadGlyphMap`), and what that sets of their text. An embedded
/// program's own encoding for what its charmaps say; a code named, by its name, as it's
/// been read already where its base is as it was.
fn truetype_gids(
    charmaps: &mut Charmaps<'_>,
    font: &Simple<'_>,
    encoding: &PdfiumEncoding<'_>,
) -> ([Gid; 256], OwnText) {
    let mut gids = [Gid::None; 256];
    let mut text = OwnText::new();
    let symbolic = font.flags & 4 != 0;
    let base = charmaps.determine_encoding(encoding.base, symbolic);
    let names = PdfiumEncoding {
        base,
        differences: encoding.differences,
    };
    // Where the program's charmaps have moved its base, a code's text by its name in
    // the base it's moved to.
    let named = |code: u8| {
        let given = encoding.differences.and_then(|d| d[code as usize]);
        (base != encoding.base && given.is_none())
            .then(|| base.table().and_then(|table| table.text(code)))
    };
    let win_or_mac = matches!(base, Base::WinAnsi | Base::MacRoman);
    if win_or_mac && names.differences.is_none() || font.flags & 32 != 0 {
        if charmaps.glyph_names && charmaps.maps.is_empty() {
            // Glyph indices from 3 at `/FirstChar`, as PDFium sets them.
            if (0..=255).contains(&font.first_char) {
                for (i, gid) in gids.iter_mut().enumerate() {
                    let at = i as i32 - font.first_char;
                    *gid = Gid::Glyph(if at < 0 { 0 } else { (at + 3) as u16 });
                }
            }
            return (gids, each_text(|_| None));
        }
        let kind = charmaps.charmap_type(font.flags & 32 != 0);
        for (code, slot) in gids.iter_mut().enumerate() {
            let name = match names.name(code as u8) {
                Named::Unknown => {
                    *slot = Gid::Unknown;
                    text.push(None);
                    continue;
                }
                Named::NoName => {
                    *slot = charmaps.gid(code as u32);
                    text.push(Some(None));
                    continue;
                }
                Named::Name(name) => name,
            };
            text.push(named(code as u8));
            let unicode = unicode_of(name);
            *slot = match kind {
                Mapped::Symbol => charmaps.ms_symbol(code as u8),
                _ if unicode == 0 => Gid::None,
                Mapped::Unicode => charmaps.gid(unicode),
                Mapped::MacRoman => match mac_roman_code(unicode) {
                    Some(mac) => charmaps.gid(mac),
                    None => charmaps.name_gid(name),
                },
                Mapped::Other => Gid::None,
            };
            if !matches!(*slot, Gid::Glyph(0) | Gid::None) {
                continue;
            }
            if name == b".notdef" {
                *slot = charmaps.gid(32);
                continue;
            }
            *slot = charmaps.name_gid(name);
            if *slot != Gid::Glyph(0) || !font.has_to_unicode {
                continue;
            }
            let front = (font.to_unicode)(code as u32)
                .map(u32::from)
                .unwrap_or(unicode);
            if front != 0 {
                *slot = charmaps.gid(front);
            }
        }
        return (gids, text);
    }
    if charmaps.select_id(3, 0) {
        let symbols: [Gid; 256] = std::array::from_fn(|code| charmaps.ms_symbol(code as u8));
        if symbols.iter().any(|g| *g != Gid::Glyph(0)) {
            let text = match base {
                Base::Builtin if charmaps.select_id(1, 0) => each_text(apple_roman),
                Base::Builtin => each_text(|_| None),
                _ => (0..=255u8)
                    .map(|code| match names.name(code) {
                        Named::Name(_) => named(code),
                        Named::NoName => Some(None),
                        Named::Unknown => None,
                    })
                    .collect(),
            };
            return (symbols, text);
        }
    }
    if charmaps.select_id(1, 0) {
        // An embedded program's are these, whatever they are.
        let gids = std::array::from_fn(|code| charmaps.gid(code as u32));
        return (gids, each_text(apple_roman));
    }
    let code_itself = |code: u8| match code {
        0 => None,
        code => Some(String::from(char::from(code))),
    };
    if charmaps.select_unicode() {
        let unicodes: [Gid; 256] = std::array::from_fn(|code| charmaps.gid(code as u32));
        if unicodes.iter().any(|g| *g != Gid::Glyph(0)) {
            return (unicodes, each_text(code_itself));
        }
        let gids = std::array::from_fn(|code| Gid::Glyph(code as u16));
        return (gids, each_text(code_itself));
    }
    (
        std::array::from_fn(|code| Gid::Glyph(code as u16)),
        each_text(|_| None),
    )
}

/// The MacRomanEncoding code of code point `unicode`, as PDFium finds it
/// (`CharCodeFromUnicodeForEncoding`, its table's first): none for the Mac
/// symbols and the Apple logo, which its table leaves 0, nor for `¡`,
/// which it has as `£`.
fn mac_roman_code(unicode: u32) -> Option<u32> {
    (32..=255u8)
        .filter(|code| !MAC_TABLE_NONE.contains(code) && *code != 0xc1)
        .find(|&code| {
            Encoding::MacRoman
                .glyph(code)
                .is_some_and(|n| unicode_of(n.as_bytes()) == unicode)
        })
        .map(u32::from)
}

/// A Type 1 font's codes' glyphs, as PDFium maps an embedded program's
/// (`CPDF_Type1Font::LoadGlyphMap`), on Linux, and what that sets of their text: a code
/// a symbolic font's encoding doesn't name is the program's glyph's name's.
fn type1_gids(
    charmaps: &mut Charmaps<'_>,
    font: &Simple<'_>,
    encoding: &PdfiumEncoding<'_>,
) -> ([Gid; 256], OwnText) {
    charmaps.use_type1_charmap();
    let mut gids = [Gid::None; 256];
    let mut text = OwnText::new();
    if font.flags & 4 != 0 {
        for (code, slot) in gids.iter_mut().enumerate() {
            let (gid, said) = match encoding.name(code as u8) {
                Named::Name(name) => (charmaps.name_gid(name), None),
                Named::NoName => match charmaps.gid(code as u32) {
                    Gid::Glyph(0) => (Gid::Glyph(0), Some(None)),
                    Gid::Glyph(g) => (Gid::Glyph(g), Some(charmaps.glyph_text(u32::from(g)))),
                    Gid::None => (Gid::None, Some(None)),
                    Gid::Unknown => (Gid::Unknown, None),
                },
                Named::Unknown => (Gid::Unknown, None),
            };
            *slot = gid;
            text.push(said);
        }
        return (gids, text);
    }
    let unicode = charmaps.select_unicode();
    for (code, slot) in gids.iter_mut().enumerate() {
        let name = match encoding.name(code as u8) {
            Named::Name(name) => name,
            Named::NoName => {
                text.push(Some(None));
                continue;
            }
            Named::Unknown => {
                *slot = Gid::Unknown;
                text.push(None);
                continue;
            }
        };
        text.push(None);
        *slot = charmaps.name_gid(name);
        if *slot != Gid::Glyph(0) {
            continue;
        }
        *slot = match name {
            b".notdef" | b"space" => Gid::None,
            _ if unicode => charmaps.gid(unicode_of(name)),
            _ => charmaps.gid(code as u32),
        };
    }
    (gids, text)
}

/// A program's charmaps as FreeType makes them, and which is selected:
/// PDFium maps codes through them, selecting as it goes.
struct Charmaps<'a> {
    maps: Vec<Charmap<'a>>,
    selected: Option<usize>,
    /// What `FT_Get_Char_Index` holds glyph indices below.
    glyphs: u32,
    /// Whether FreeType says the program names its glyphs
    /// (`FT_HAS_GLYPH_NAMES`).
    glyph_names: bool,
    /// Its glyphs' names, each name's first, where it has them.
    names: HashMap<Vec<u8>, u32>,
    /// Each glyph's name, in order, where it has one.
    named: Vec<(u32, Vec<u8>)>,
}

struct Charmap<'a> {
    platform: u16,
    encoding: u16,
    /// Whether FreeType takes it for a Unicode one (`FT_ENCODING_UNICODE`).
    unicode: bool,
    lookup: Lookup<'a>,
}

enum Lookup<'a> {
    /// An SFNT's `cmap` subtable.
    Table(CmapSubtable<'a>),
    /// A format 2 one, read as FreeType reads it.
    Format2(Format2<'a>),
    /// A format 4 one, read as FreeType reads it.
    Format4(Format4<'a>),
    /// A Type 1 or CFF program's encoding: each code's glyph.
    Codes(Box<[u32; 256]>),
    /// One FreeType makes from the glyphs' names ([`PsUnicodes`]).
    Names(Box<PsUnicodes>),
}

/// How PDFium maps a TrueType font's named codes
/// (`CPDF_TrueTypeFont::CharmapType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mapped {
    Unicode,
    MacRoman,
    Symbol,
    Other,
}

impl<'a> Charmaps<'a> {
    /// `view`'s charmaps, as FreeType makes them at open, its Unicode one
    /// selected (`find_unicode_charmap`).
    fn of(view: &View<'a>, sfnt_glyphs: u32) -> Charmaps<'a> {
        let mut charmaps = Charmaps {
            maps: Vec::new(),
            selected: None,
            glyphs: 0,
            glyph_names: false,
            names: HashMap::new(),
            named: Vec::new(),
        };
        // Each glyph's name, in order, where it has one.
        let mut named: Vec<(u32, Vec<u8>)> = Vec::new();
        match view {
            View::Glyf(font)
            | View::Cff {
                sfnt: Some(font), ..
            } => {
                charmaps.glyphs = sfnt_glyphs;
                if let Ok(post) = font.post()
                    && post.version().to_major_minor() != (3, 0)
                {
                    charmaps.glyph_names = true;
                    for gid in 0..charmaps.glyphs.min(u32::from(u16::MAX)) {
                        if let Some(name) =
                            post.glyph_name(skrifa::raw::types::GlyphId16::new(gid as u16))
                        {
                            named.push((gid, name.as_bytes().to_vec()));
                        }
                    }
                }
                if let Ok(cmap) = font.cmap() {
                    let data = cmap.offset_data().as_bytes();
                    for (i, record) in cmap.encoding_records().iter().enumerate() {
                        let Ok(table) = cmap.subtable(i as u16) else {
                            continue;
                        };
                        let lookup = match table {
                            CmapSubtable::Format14(_) => continue,
                            CmapSubtable::Format2(_) => {
                                let at = record.subtable_offset().to_u32() as usize;
                                match data.get(at..).and_then(Format2::read) {
                                    Some(table) => Lookup::Format2(table),
                                    None => continue,
                                }
                            }
                            CmapSubtable::Format4(_) => {
                                let at = record.subtable_offset().to_u32() as usize;
                                match data.get(at..).and_then(Format4::read) {
                                    Some(table) => Lookup::Format4(table),
                                    None => continue,
                                }
                            }
                            table => Lookup::Table(table),
                        };
                        let platform = match record.platform_id() {
                            PlatformId::Unicode => 0,
                            PlatformId::Macintosh => 1,
                            PlatformId::ISO => 2,
                            PlatformId::Windows => 3,
                            PlatformId::Custom => 4,
                            _ => continue,
                        };
                        let encoding = record.encoding_id();
                        charmaps.maps.push(Charmap {
                            platform,
                            encoding,
                            unicode: matches!((platform, encoding), (0 | 2, _) | (3, 1 | 10)),
                            lookup,
                        });
                    }
                }
                let has_unicode =
                    (charmaps.maps.iter()).any(|m| m.unicode || (m.platform, m.encoding) == (3, 0));
                if !has_unicode && charmaps.glyph_names {
                    charmaps.push_names(&named);
                }
                if let View::Cff { cff, .. } = view {
                    named = charmaps.opentype_cff(cff);
                }
            }
            View::Cff { cff, sfnt: None } => {
                charmaps.glyphs = cff.num_glyphs();
                if !cff.is_cid() {
                    charmaps.glyph_names = true;
                    named = charset_names(cff);
                    charmaps.push_names(&named);
                    if let Some((encoding, codes)) = cff_encoding(cff) {
                        charmaps.push_codes(encoding, codes);
                    }
                }
            }
            View::Type1 { font, .. } => {
                charmaps.glyphs = font.num_glyphs();
                charmaps.glyph_names = true;
                named.extend(
                    font.glyph_names()
                        .map(|(gid, name)| (gid.to_u32(), name.as_bytes().to_vec())),
                );
                charmaps.push_names(&named);
                if let Some(encoding) = font.encoding() {
                    match encoding.predefined() {
                        Some(PredefinedEncoding::IsoLatin1) => {
                            if let Some(unicodes) = PsUnicodes::of(&named) {
                                charmaps.maps.push(Charmap {
                                    platform: 7,
                                    encoding: 3,
                                    unicode: false,
                                    lookup: Lookup::Names(Box::new(unicodes)),
                                });
                            }
                        }
                        predefined => {
                            let codes = Box::new(std::array::from_fn(|c| {
                                encoding.map(c as u8).map_or(0, |g| g.to_u32())
                            }));
                            let id = match predefined {
                                Some(PredefinedEncoding::Standard) => 0,
                                Some(PredefinedEncoding::Expert) => 1,
                                _ => 2,
                            };
                            charmaps.push_codes(id, codes);
                        }
                    }
                }
            }
        }
        for (gid, name) in &named {
            charmaps.names.entry(name.clone()).or_insert(*gid);
        }
        charmaps.named = named;
        charmaps.select_unicode();
        charmaps
    }

    /// What an OpenType font's CFF adds to the charmaps of its `cmap` and `post`
    /// (`cff_face_init`), and the names it has: a CFF that isn't CID-keyed has names
    /// whatever its `post` says, and they're its charset's, whatever that says
    /// (`cff_get_name_index`), so a CID-keyed one's are its CIDs read as strings, where
    /// its `post` says it has any. A Unicode charmap of them where there's no Windows
    /// Unicode one or one of platform 0, and one of its encoding.
    fn opentype_cff(&mut self, cff: &CffFontRef<'_>) -> Vec<(u32, Vec<u8>)> {
        let keyed = cff.is_cid();
        self.glyph_names |= !keyed;
        let named = charset_names(cff);
        let unicode =
            (self.maps.iter()).any(|m| m.platform == 0 || (m.platform, m.encoding) == (3, 1));
        if !unicode {
            self.push_names(&named);
        }
        if !keyed && let Some((encoding, codes)) = cff_encoding(cff) {
            self.push_codes(encoding, codes);
        }
        match self.glyph_names {
            true => named,
            false => Vec::new(),
        }
    }

    /// The Unicode charmap FreeType makes from the glyphs' names, `named`,
    /// where any of them has a code point.
    fn push_names(&mut self, named: &[(u32, Vec<u8>)]) {
        if let Some(unicodes) = PsUnicodes::of(named) {
            self.maps.push(Charmap {
                platform: 3,
                encoding: 1,
                unicode: true,
                lookup: Lookup::Names(Box::new(unicodes)),
            });
        }
    }

    /// A Type 1 or CFF program's encoding, as FreeType's Adobe charmap of
    /// it: `encoding` 0 standard, 1 expert, 2 custom.
    fn push_codes(&mut self, encoding: u16, codes: Box<[u32; 256]>) {
        self.maps.push(Charmap {
            platform: 7,
            encoding,
            unicode: false,
            lookup: Lookup::Codes(codes),
        });
    }

    /// `FT_Select_Charmap(FT_ENCODING_UNICODE)`: the last UCS-4 one, else
    /// the last Unicode one.
    fn select_unicode(&mut self) -> bool {
        let ucs4 = |m: &Charmap<'_>| {
            let format13 = matches!(m.lookup, Lookup::Table(CmapSubtable::Format13(_)));
            matches!((m.platform, m.encoding), (3, 10) | (0, 4))
                || (m.platform, m.encoding) == (0, 6) && format13
        };
        let found = (self.maps.iter().rposition(|m| m.unicode && ucs4(m)))
            .or_else(|| self.maps.iter().rposition(|m| m.unicode));
        if found.is_some() {
            self.selected = found;
        }
        found.is_some()
    }

    /// `UseTTCharmap`: the first with these ids, selected.
    fn select_id(&mut self, platform: u16, encoding: u16) -> bool {
        let found =
            (self.maps.iter()).position(|m| (m.platform, m.encoding) == (platform, encoding));
        if found.is_some() {
            self.selected = found;
        }
        found.is_some()
    }

    /// `UseTTCharmapUnicode`: Windows Unicode, else the first Unicode one
    /// where there's no Windows symbol one.
    fn select_tt_unicode(&mut self) -> bool {
        if self.select_id(3, 1) {
            return true;
        }
        let symbol = self.maps.iter().any(|m| (m.platform, m.encoding) == (3, 0));
        let first = self
            .maps
            .iter()
            .position(|m| m.unicode && (m.platform, m.encoding) != (3, 0));
        match (first, symbol) {
            (Some(i), false) => {
                self.selected = Some(i);
                true
            }
            _ => false,
        }
    }

    /// `DetermineCharmapType`, selecting as it goes.
    fn charmap_type(&mut self, nonsymbolic: bool) -> Mapped {
        if self.select_tt_unicode() {
            return Mapped::Unicode;
        }
        let order = match nonsymbolic {
            true => [((1, 0), Mapped::MacRoman), ((3, 0), Mapped::Symbol)],
            false => [((3, 0), Mapped::Symbol), ((1, 0), Mapped::MacRoman)],
        };
        (order.into_iter())
            .find(|&((p, e), _)| self.select_id(p, e))
            .map_or(Mapped::Other, |(_, kind)| kind)
    }

    /// `CPDF_TrueTypeFont::DetermineEncoding`: a symbolic font's
    /// WinAnsiEncoding or MacRomanEncoding, where its charmaps have none of
    /// that platform's, the other's, else none.
    fn determine_encoding(&self, base: Base, symbolic: bool) -> Base {
        if !symbolic || !matches!(base, Base::WinAnsi | Base::MacRoman) || self.maps.is_empty() {
            return base;
        }
        let win = self.maps.iter().any(|m| matches!(m.platform, 0 | 3));
        let mac = self.maps.iter().any(|m| m.platform == 1);
        match (base, win, mac) {
            (Base::WinAnsi, false, true) => Base::MacRoman,
            (Base::WinAnsi, false, false) => Base::Builtin,
            (Base::MacRoman, true, false) => Base::WinAnsi,
            (Base::MacRoman, false, false) => Base::Builtin,
            _ => base,
        }
    }

    /// `UseType1Charmap`: the program's encoding where it has one past a
    /// first Unicode one.
    fn use_type1_charmap(&mut self) {
        match self.maps.first().map(|m| m.unicode) {
            None => {}
            Some(true) if self.maps.len() == 1 => {}
            Some(unicode) => self.selected = Some(usize::from(unicode)),
        }
    }

    /// `FT_Get_Char_Index` through the charmap selected, as PDFium holds
    /// it.
    fn gid(&self, code: u32) -> Gid {
        let Some(map) = self.selected.and_then(|i| self.maps.get(i)) else {
            return Gid::Glyph(0);
        };
        let found = match &map.lookup {
            Lookup::Table(CmapSubtable::Format8(_)) => return Gid::Unknown,
            Lookup::Format2(table) => table.glyph(code),
            Lookup::Table(table) => table.map_codepoint(code).map_or(0, |g| g.to_u32()),
            Lookup::Format4(table) => table.glyph(code, self.glyphs),
            Lookup::Codes(codes) => codes.get(code as usize).copied().unwrap_or(0),
            Lookup::Names(unicodes) => match unicodes.glyph(code) {
                Some(found) => found,
                None => return Gid::Unknown,
            },
        };
        Gid::of(if found < self.glyphs { found } else { 0 })
    }

    /// `GetGlyphIndexForMSSymbol`: the code in the symbol font's private
    /// pages, or as it is.
    fn ms_symbol(&self, code: u8) -> Gid {
        for page in [0x00, 0xf0, 0xf1, 0xf2] {
            match self.gid(page << 8 | u32::from(code)) {
                Gid::Glyph(0) => {}
                found => return found,
            }
        }
        Gid::Glyph(0)
    }

    /// What the name of glyph `gid` (`FT_Get_Glyph_Name`) stands for, where it has one.
    fn glyph_text(&self, gid: u32) -> Option<String> {
        let at = self.named.binary_search_by_key(&gid, |(g, _)| *g).ok()?;
        name_text(&self.named[at].1)
    }

    /// `FT_Get_Name_Index`: the first glyph of that name, 0 for none, a name that's
    /// not UTF-8 also as read-fonts keeps a Type 1 glyph's.
    fn name_gid(&self, name: &[u8]) -> Gid {
        let gid = match self.glyph_names {
            true => (self.names.get(name).copied())
                .or_else(|| self.names.get(&type1::decoded_name(name)[..]).copied())
                .unwrap_or(0),
            false => 0,
        };
        Gid::of(gid)
    }
}

/// Each glyph of `cff`'s name by its charset, in order, where it has one.
fn charset_names(cff: &CffFontRef<'_>) -> Vec<(u32, Vec<u8>)> {
    (cff.charset().into_iter().flat_map(|c| c.iter()))
        .filter_map(|(gid, sid)| Some((gid.to_u32(), cff.string(sid)?.to_vec())))
        .collect()
}

/// A format 4 `cmap` subtable, from its start to the `cmap` table's end, as
/// FreeType reads one: refused where its validation fails
/// (`tt_cmap4_validate`, at its default level), and looked up its way
/// (`tt_cmap4_char_index`), segments that overlap or are out of order
/// searched as it searches them.
struct Format4<'a> {
    table: &'a [u8],
    segments: usize,
    /// Its segments' starts and ends out of order.
    unsorted: bool,
    /// Its segments overlapping, in order.
    overlapping: bool,
}

impl<'a> Format4<'a> {
    fn read(table: &'a [u8]) -> Option<Format4<'a>> {
        let u16_at = |at: usize| Some(u16::from_be_bytes(table.get(at..at + 2)?.try_into().ok()?));
        // Its length is where the `cmap` table ends, whatever it says.
        if table.len() < 16 {
            return None;
        }
        let segments = usize::from(u16_at(6)? / 2);
        if table.len() < 16 + segments * 8 {
            return None;
        }
        let mut read = Format4 {
            table,
            segments,
            unsorted: false,
            overlapping: false,
        };
        let glyph_ids = read.offsets() + segments * 2;
        let mut last = (0, 0);
        for n in 0..segments {
            let (start, end, _, offset) = read.segment(n);
            if start > end {
                return None;
            }
            if n > 0 && start <= last.1 {
                match last.0 > start || last.1 > end {
                    true => read.unsorted = true,
                    false => read.overlapping = true,
                }
            }
            let final_segment = n == segments - 1 && start == 0xFFFF && end == 0xFFFF;
            match offset {
                0 => {}
                0xFFFF if !final_segment => return None,
                0xFFFF => {}
                _ if !final_segment => {
                    let at = read.offsets() + n * 2 + usize::from(offset);
                    let len = (usize::from(end - start) + 1) * 2;
                    if at < glyph_ids || at + len > table.len() {
                        return None;
                    }
                }
                _ => {}
            }
            last = (start, end);
        }
        Some(read)
    }

    fn u16_at(&self, at: usize) -> u16 {
        self.table
            .get(at..at + 2)
            .map_or(0, |b| u16::from_be_bytes([b[0], b[1]]))
    }

    /// Where the range offsets start.
    fn offsets(&self) -> usize {
        16 + self.segments * 6
    }

    /// Segment `n`'s start, end, delta and range offset.
    fn segment(&self, n: usize) -> (u16, u16, i16, u16) {
        let s = self.segments * 2;
        let end = self.u16_at(14 + n * 2);
        let start = self.u16_at(16 + s + n * 2);
        let delta = self.u16_at(16 + 2 * s + n * 2) as i16;
        let offset = self.u16_at(16 + 3 * s + n * 2);
        (start, end, delta, offset)
    }

    /// The glyph `code` maps to, 0 for none, by segment `n`, as FreeType
    /// works it: through its glyph array, or by its delta.
    fn by_segment(&self, n: usize, code: u32, glyphs: u32) -> u32 {
        let (start, _, mut delta, mut offset) = self.segment(n);
        let at = self.offsets() + n * 2;
        if n == self.segments - 1
            && self.segment(n).0 == 0xFFFF
            && self.segment(n).1 == 0xFFFF
            && offset != 0
            && at + usize::from(offset) + 2 > self.table.len()
        {
            (delta, offset) = (1, 0);
        }
        match offset {
            0 => (code as i32 + i32::from(delta)) as u32 & 0xFFFF,
            _ => {
                let glyph = u32::from(
                    self.u16_at(at + usize::from(offset) + (code - u32::from(start)) as usize * 2),
                );
                match glyph {
                    0 => 0,
                    g => match (g as i32 + i32::from(delta)) as u32 & 0xFFFF {
                        g if g >= glyphs => 0,
                        g => g,
                    },
                }
            }
        }
    }

    /// `FT_Get_Char_Index` through it, below `glyphs` glyphs.
    fn glyph(&self, code: u32, glyphs: u32) -> u32 {
        if code >= 0x10000 || self.segments == 0 {
            return 0;
        }
        match self.unsorted {
            true => self.linear(code, glyphs),
            false => self.binary(code, glyphs),
        }
    }

    /// The first segment that holds `code`, as FreeType searches them in
    /// order (`tt_cmap4_char_map_linear`): none past one that starts after
    /// it.
    fn linear(&self, code: u32, glyphs: u32) -> u32 {
        for n in 0..self.segments {
            let (start, end, _, offset) = self.segment(n);
            if code < u32::from(start) {
                break;
            }
            if code <= u32::from(end) {
                let final_segment = n == self.segments - 1 && start == 0xFFFF && end == 0xFFFF;
                if offset == 0xFFFF && !final_segment {
                    continue;
                }
                return self.by_segment(n, code, glyphs);
            }
        }
        0
    }

    /// A segment that holds `code`, found by halves, as FreeType finds one
    /// (`tt_cmap4_char_map_binary`): where segments overlap, the first that
    /// holds it and has a glyph array or delta.
    fn binary(&self, code: u32, glyphs: u32) -> u32 {
        let (mut min, mut max) = (0, self.segments);
        while min < max {
            let mid = (min + max) / 2;
            let (start, end, _, offset) = self.segment(mid);
            if code < u32::from(start) {
                max = mid;
                continue;
            }
            if code > u32::from(end) {
                min = mid + 1;
                continue;
            }
            if !self.overlapping {
                let final_segment = mid == self.segments - 1 && start == 0xFFFF && end == 0xFFFF;
                return match offset == 0xFFFF && !final_segment {
                    true => 0,
                    false => self.by_segment(mid, code, glyphs),
                };
            }
            let found = mid;
            let mut chosen = (offset != 0xFFFF).then_some(found);
            let mut i = found;
            while i > 0 && code <= u32::from(self.segment(i - 1).1) {
                if self.segment(i - 1).3 != 0xFFFF {
                    chosen = Some(i - 1);
                }
                i -= 1;
            }
            if chosen.is_none() {
                for after in found + 1..self.segments {
                    if code < u32::from(self.segment(after).0) {
                        break;
                    }
                    if self.segment(after).3 != 0xFFFF {
                        chosen = Some(after);
                    }
                }
            }
            return chosen.map_or(0, |n| self.by_segment(n, code, glyphs));
        }
        0
    }
}

/// The Unicode charmap FreeType makes from a program's glyph names
/// (`ps_unicodes_init`, `ps_unicodes_char_index`): each name's code point
/// (`ps_unicode_value`), one a variant's name gives (with a period) found
/// only where none gives it plainly, and ten extra code points mapped to
/// the glyphs of their names where no name gives them. Where two glyphs
/// share a code point, which FreeType finds rests on how its sort orders
/// them, so it's unknown.
struct PsUnicodes {
    /// Code points' glyphs: None where more than one has it.
    plain: HashMap<u32, Option<u32>>,
    /// Variants' code points' glyphs, the same way.
    variants: HashMap<u32, Option<u32>>,
}

/// The code points FreeType adds for glyphs by these names, where no name
/// gives them (`ft_extra_glyph_unicodes`, `ft_extra_glyph_names`).
const EXTRA_GLYPHS: [(u32, &[u8]); 10] = [
    (0x0394, b"Delta"),
    (0x03A9, b"Omega"),
    (0x2215, b"fraction"),
    (0x00AD, b"hyphen"),
    (0x02C9, b"macron"),
    (0x03BC, b"mu"),
    (0x2219, b"periodcentered"),
    (0x00A0, b"space"),
    (0x021A, b"Tcommaaccent"),
    (0x021B, b"tcommaaccent"),
];

impl PsUnicodes {
    /// From `named`, each glyph and its name, in order; None where no name
    /// has a code point, and FreeType makes no charmap.
    fn of(named: &[(u32, Vec<u8>)]) -> Option<PsUnicodes> {
        let mut unicodes = PsUnicodes {
            plain: HashMap::new(),
            variants: HashMap::new(),
        };
        let mut extra: [Option<Option<u32>>; 10] = [None; 10];
        let add = |map: &mut HashMap<u32, Option<u32>>, unicode: u32, gid: u32| {
            map.entry(unicode)
                .and_modify(|g| *g = g.filter(|&g| g == gid))
                .or_insert(Some(gid));
        };
        for (gid, name) in named.iter().filter(|(_, n)| !n.is_empty()) {
            if let Some(n) = EXTRA_GLYPHS.iter().position(|(_, e)| *e == &name[..]) {
                extra[n].get_or_insert(Some(*gid));
            }
            let Some((unicode, variant)) = name_code_point(name).filter(|(u, _)| *u != 0) else {
                continue;
            };
            if let Some(n) = EXTRA_GLYPHS
                .iter()
                .position(|(e, _)| *e == unicode && !variant)
            {
                extra[n] = Some(None);
            }
            match variant {
                false => add(&mut unicodes.plain, unicode, *gid),
                true => add(&mut unicodes.variants, unicode, *gid),
            }
        }
        for ((unicode, _), gid) in EXTRA_GLYPHS.iter().zip(extra) {
            if let Some(Some(gid)) = gid {
                add(&mut unicodes.plain, *unicode, gid);
            }
        }
        (!unicodes.plain.is_empty() || !unicodes.variants.is_empty()).then_some(unicodes)
    }

    /// The glyph `unicode` maps to, 0 for none; None where that can't be said.
    fn glyph(&self, unicode: u32) -> Option<u32> {
        match self.plain.get(&unicode) {
            Some(found) => *found,
            None => self.variants.get(&unicode).copied().unwrap_or(Some(0)),
        }
    }
}

/// A bare CFF font's encoding as FreeType reads it (`cff_encoding_load`),
/// and its id as an Adobe charmap: 0 standard, 1 expert, 2 custom; each
/// code's glyph. A predefined encoding's code draws the last glyph its
/// charset names by the code's string, a custom one's the glyph it gives
/// (the last of a code given twice), or a supplement's string's first.
/// None where FreeType makes no charmap of it, no code being mapped.
fn cff_encoding(cff: &CffFontRef<'_>) -> Option<(u16, Box<[u32; 256]>)> {
    let data = cff.data();
    let top = skrifa::raw::tables::cff::Cff::read(data.into())
        .ok()?
        .top_dicts()
        .get(0)?;
    let offset = (skrifa::raw::ps::cff::dict::entries(top, None))
        .filter_map(Result::ok)
        .find_map(|e| match e {
            skrifa::raw::ps::cff::dict::Entry::Encoding(at) => Some(at),
            _ => None,
        })
        .unwrap_or(0);
    let glyphs = cff.num_glyphs();
    let mut sids = vec![0u16; glyphs as usize];
    for (gid, sid) in cff.charset()?.iter() {
        if let Some(slot) = sids.get_mut(gid.to_u32() as usize) {
            *slot = sid.to_u16();
        }
    }
    let mut codes = Box::new([0u32; 256]);
    let count = match offset {
        0 | 1 => {
            let predefined = match offset {
                0 => PredefinedEncoding::Standard,
                _ => PredefinedEncoding::Expert,
            };
            let mut count = 0;
            for code in 0..=255u8 {
                let sid = predefined.sid(code).map_or(0, |s| s.to_u16());
                let gid = match sid {
                    0 => None,
                    sid => sids.iter().rposition(|&s| s == sid),
                };
                if let Some(gid) = gid.filter(|&g| g != 0) {
                    codes[usize::from(code)] = gid as u32;
                    count = usize::from(code) + 1;
                }
            }
            count
        }
        at => {
            let table = data.get(at..)?;
            let format = *table.first()?;
            let n = usize::from(*table.get(1)?);
            let mut pos = 2;
            let count = match format & 0x7F {
                0 => {
                    for (j, &code) in (1..).zip(table.get(2..2 + n)?) {
                        if j < glyphs {
                            codes[usize::from(code)] = j;
                        }
                    }
                    pos += n;
                    n + 1
                }
                1 => {
                    let (mut count, mut gid) = (0, 1u32);
                    for range in table.get(2..2 + 2 * n)?.chunks(2) {
                        let left = u32::from(range[1]) + 1;
                        count = count.max(left as usize);
                        for (k, code) in (gid..gid + left).zip(u32::from(range[0])..) {
                            if k < glyphs && code < 256 {
                                codes[code as usize] = k;
                            }
                        }
                        gid += left;
                    }
                    pos += 2 * n;
                    count.min(256)
                }
                _ => return None,
            };
            if format & 0x80 != 0 {
                let supplements = usize::from(*table.get(pos)?);
                for supplement in table.get(pos + 1..pos + 1 + 3 * supplements)?.chunks(3) {
                    let sid = u16::from_be_bytes([supplement[1], supplement[2]]);
                    if let Some(gid) = sids.iter().position(|&s| s == sid) {
                        codes[usize::from(supplement[0])] = gid as u32;
                    }
                }
            }
            count
        }
    };
    let id = match offset {
        0 => 0,
        1 => 1,
        _ => 2,
    };
    (count > 0).then_some((id, codes))
}

/// A format 2 `cmap` subtable, from its start to the `cmap` table's end, as
/// FreeType reads one (`tt_cmap2_validate` at its default level,
/// `tt_cmap2_char_index`).
struct Format2<'a> {
    table: &'a [u8],
}

impl<'a> Format2<'a> {
    fn read(table: &'a [u8]) -> Option<Format2<'a>> {
        let read = Format2 { table };
        let length = usize::from(read.u16_at(2)?);
        if length > table.len() || length < 6 + 512 {
            return None;
        }
        let subheaders = (0..256)
            .filter_map(|n| read.u16_at(6 + 2 * n))
            .map(|k| usize::from(k >> 3))
            .max()?
            + 1;
        let glyph_ids = 518 + subheaders * 8;
        if glyph_ids > table.len() {
            return None;
        }
        for n in 0..subheaders {
            let at = 518 + 8 * n;
            let (count, offset) = (read.u16_at(at + 2)?, read.u16_at(at + 6)?);
            let ids = at + 6 + usize::from(offset);
            if count != 0
                && offset != 0
                && (ids < glyph_ids || ids + usize::from(count) * 2 > length)
            {
                return None;
            }
        }
        Some(read)
    }

    fn u16_at(&self, at: usize) -> Option<u16> {
        Some(u16::from_be_bytes(
            self.table.get(at..at + 2)?.try_into().ok()?,
        ))
    }

    /// The glyph `code` maps to, 0 for none: a byte by the first
    /// subheader, where its key says it's a code of one byte; two by the
    /// subheader the first's key gives, where that isn't the first.
    fn glyph(&self, code: u32) -> u32 {
        if code >= 0x10000 {
            return 0;
        }
        let (hi, lo) = ((code >> 8) as usize, code & 0xFF);
        let key = |byte: usize| self.u16_at(6 + 2 * byte).unwrap_or(0);
        let sub = match hi {
            0 if key(lo as usize) != 0 => return 0,
            0 => 518,
            _ => match 518 + usize::from(key(hi) & !7) {
                518 => return 0,
                sub => sub,
            },
        };
        let field = |at: usize| self.u16_at(sub + at).unwrap_or(0);
        let (start, count, delta, offset) = (field(0), field(2), field(4) as i16, field(6));
        let index = lo.wrapping_sub(u32::from(start));
        if index >= u32::from(count) || offset == 0 {
            return 0;
        }
        match self
            .u16_at(sub + 6 + usize::from(offset) + 2 * index as usize)
            .unwrap_or(0)
        {
            0 => 0,
            g => (i32::from(g) + i32::from(delta)) as u32 & 0xFFFF,
        }
    }
}
