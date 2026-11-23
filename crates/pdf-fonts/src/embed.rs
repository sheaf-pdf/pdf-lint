//! A TrueType program embedded in a PDF (ISO 32000-2, 9.9): opened for it, the codes a
//! string is drawn in, the glyphs a document draws, and the font written.
//!
//! A [`Program`] is a TrueType or OpenType font's bytes whose outlines are TrueType's
//! (`glyf`), opened once. Text is drawn in it as a Type 0 font over `/Identity-H`, each
//! character two bytes, high byte first: the number of the glyph the program's `cmap`
//! maps it to ([`Program::codes`]). That code is the glyph's CID (9.7.4.2), and it's the
//! program's own number, not one a subset gives, so a code stays the same whatever else
//! a document draws: what's drawn before more text is drawn still shows its glyphs when
//! the font is written again. There's no shaping: each character is a glyph of its own,
//! in the order given, and one the program has no glyph for is refused ([`Unmapped`]),
//! never drawn as another. [`Glyphs`] keeps the glyphs a document draws, each with the
//! text it stands for, and [`write()`] writes the font: a Type 0 font over `/Identity-H`,
//! its CIDFont's program the one given cut to the glyphs drawn, and a `/ToUnicode` that
//! reads each code back as the text its glyph was drawn for.
//!
//! A program is a caller's bytes, but it may be anyone's font, so it's checked as it's
//! opened for what the subset it's cut to would otherwise make of it: a composite
//! glyph naming a glyph the program hasn't, `loca` entries whose glyphs overlap, and
//! `name` records whose lengths overflow the table they'd be written in are refused
//! ([`Error::Malformed`]), and so is any glyph, metric or table that a cut would
//! refuse or write wrong. What's done with an opened program is then bounded by its
//! size: each glyph's data is read at most three times, and what a subset of it holds
//! is at most what it holds, and a constant. And any glyphs of it are cut: what
//! [`write()`] refuses of a program opened is a [`Glyphs`] past its glyphs, or what its
//! licence restricts.
//!
//! ```
//! use pdf_fonts::embed::{Glyphs, Program, Unmapped};
//!
//! # let bytes = pdf_fixtures::font::sample(1000).build();
//! // A TrueType program's bytes, opened: face 0, as it isn't a collection.
//! let program = Program::open(bytes, 0)?;
//! let a = program.glyph('A').unwrap();
//! assert_eq!(program.codes("A")?, a.to_be_bytes());
//!
//! // The glyphs a document draws, each with its text, as its codes are made.
//! let mut glyphs = Glyphs::new();
//! assert_eq!(glyphs.add(&program, "AAA")?.len(), 6);
//! assert_eq!(glyphs.len(), 1);
//!
//! // A character the program has no glyph for is refused, and nothing is kept.
//! let refused = glyphs.add(&program, "A\u{1F600}").unwrap_err();
//! assert_eq!((refused.at, refused.character), (1, '\u{1F600}'));
//! assert_eq!(glyphs.len(), 1);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use skrifa::instance::{LocationRef, Size};
use skrifa::raw::{FileRef, FontRef, TableProvider};
use skrifa::{MetadataProvider, charmap::Charmap};

use crate::{FontMetrics, GlyphBox};

mod sfnt;
mod subset;
mod write;

pub use write::{Options, write};

/// A TrueType program opened for embedding: its bytes, shared, and what's read of them
/// once. A clone shares the bytes. Its `Debug` gives its length, its glyphs' count, its
/// units per em and what its licence allows, never its names or a byte of it.
///
/// A variable font is embedded at its default instance: its variations are what a
/// subset of it leaves out.
#[derive(Clone)]
pub struct Program {
    pub(crate) data: Arc<[u8]>,
    index: u32,
    /// How many glyphs it has, as its `maxp` says.
    glyphs: u16,
    upem: u16,
    /// Where its `hmtx` is in `data`, and how many advances it holds: the last
    /// is every later glyph's.
    hmtx: (usize, u16),
    metrics: FontMetrics,
    embedding: Embedding,
}

impl Program {
    /// Opens face `index` of `data` (0 for a program that isn't a collection) for
    /// embedding: a TrueType or OpenType font, or a collection of them (`.ttc`), whose
    /// outlines are TrueType's.
    ///
    /// An `Arc` given is kept, never copied. Its tables are read as the subset it's cut
    /// to reads them, and checked so that nothing done with it later costs more than
    /// its size: its glyphs' data, as its `loca` places it, at most its `glyf`, so that
    /// no two glyphs share bytes; each composite glyph's components within its
    /// glyphs; and the `name` records a subset keeps within what a `name` table holds.
    /// Its glyphs are read in two passes, so opening it is linear in its size, and so is
    /// all that's done with it after: there's no cap to set.
    ///
    /// And they're checked so that any of its glyphs are cut, as each table a cut takes
    /// is read for each glyph it keeps, and some are rewritten: every glyph placed by
    /// its `loca`, its data a whole glyph's, every composite's records whole; a left
    /// side bearing in `hmtx` for every glyph; an `hhea` of the 36 bytes OpenType gives
    /// it; a `maxp` of version 1.0; and a `post` of version 2.0 naming every glyph.
    ///
    /// # Errors
    ///
    /// - [`Error::NotSfnt`] for bytes that aren't a TrueType or OpenType font, or a
    ///   collection of them.
    /// - [`Error::Directory`] for a face whose table directory isn't in order of its
    ///   tables' tags, is cut short, or isn't where its collection's first list of faces
    ///   puts it.
    /// - [`Error::Index`] for an `index` past a collection's faces, or other than 0
    ///   where it isn't one.
    /// - [`Error::Outlines`] for a program whose outlines are CFF's, CFF2's or bitmaps
    ///   alone.
    /// - [`Error::Malformed`], with its table's tag, for a table it needs that's
    ///   missing or doesn't read (`head`, `maxp`, `hhea`, `hmtx`, `loca`), units per em
    ///   outside the 16 to 16,384 OpenType allows, which FreeType, and so PDFium, takes
    ///   for no program (`head`), a `loca` that doesn't place a glyph or whose glyphs
    ///   overlap, a glyph whose data isn't a whole glyph's or a composite glyph naming
    ///   a glyph past its count (`glyf`), an `hmtx` without a glyph's side bearing, an
    ///   `hhea` past its 36 bytes, a `maxp` of a version but 1.0, a `post` of version
    ///   2.0 that doesn't name every glyph, or a `name` table a subset couldn't write.
    pub fn open(data: impl Into<Arc<[u8]>>, index: u32) -> Result<Program, Error> {
        let data: Arc<[u8]> = data.into();
        let font = face(&data, index)?;
        let tables = sfnt::Tables::of(&data, &font)?;
        tables.outlines()?;
        let malformed = |table| Error::Malformed { table };
        let head = font.head().map_err(|_| malformed("head"))?;
        // OpenType's range, outside which FreeType, and so PDFium, loads no program.
        let upem = head.units_per_em();
        if !(16..=16384).contains(&upem) {
            return Err(malformed("head"));
        }
        let glyphs = font.maxp().map_err(|_| malformed("maxp"))?.num_glyphs();
        if glyphs == 0 {
            return Err(malformed("maxp"));
        }
        let long = font.hhea().map_err(|_| malformed("hhea"))?;
        let long = long.number_of_h_metrics();
        if long == 0 {
            return Err(malformed("hhea"));
        }
        let hmtx = tables.hmtx(long, glyphs).ok_or(malformed("hmtx"))?;
        let outlines = tables.glyf()?;
        outlines.check(glyphs)?;
        tables.check_rewritten(glyphs)?;
        tables.check_name()?;
        let embedding = tables.embedding();

        let skrifa = font.metrics(Size::unscaled(), LocationRef::default());
        let scale = |v: f32| v * 1000.0 / f32::from(upem);
        let units = |v: i16| scale(f32::from(v));
        let metrics = FontMetrics::new(
            scale(skrifa.ascent),
            scale(skrifa.descent),
            Some(GlyphBox::new(
                units(head.x_min()),
                units(head.y_min()),
                units(head.x_max()),
                units(head.y_max()),
            )),
        );
        let hmtx = (hmtx, long);
        Ok(Program {
            data,
            index,
            glyphs,
            upem,
            hmtx,
            metrics,
            embedding,
        })
    }

    /// The glyph `c` is drawn with: the one the program's Unicode `cmap` maps it to,
    /// through skrifa, which takes a full Unicode subtable (format 12) over a BMP one
    /// (format 4). None where it maps none, maps it to `.notdef` (glyph 0), or to a
    /// glyph past the program's count.
    #[must_use]
    pub fn glyph(&self, c: char) -> Option<u16> {
        let font = FontRef::from_index(&self.data, self.index).ok()?;
        self.mapped(&font.charmap(), c)
    }

    /// `text` as a Type 0 font over `/Identity-H` shows it: two bytes a character, high
    /// byte first, the number of its glyph in the program ([`Program::glyph`]). A
    /// character past the Basic Multilingual Plane is one glyph, so two bytes, like any.
    ///
    /// Nothing is kept: a code draws its glyph only in a font written of [`Glyphs`]
    /// that keep it, and shows `.notdef` in one that don't. [`Glyphs::add`] gives the
    /// same codes, and keeps their glyphs.
    ///
    /// # Errors
    ///
    /// [`Unmapped`] for the first character the program has no glyph for, with where it
    /// is: a control character such as `\n`, a variation selector the `cmap` doesn't map,
    /// a code point the program hasn't. Nothing is drawn in its place.
    pub fn codes(&self, text: &str) -> Result<Vec<u8>, Unmapped> {
        let font = FontRef::from_index(&self.data, self.index).ok();
        let charmap = font.as_ref().map(Charmap::new);
        let mut codes = Vec::with_capacity(2 * text.len());
        for (at, character) in text.char_indices() {
            let glyph = charmap.as_ref().and_then(|map| self.mapped(map, character));
            let glyph = glyph.ok_or(Unmapped { at, character })?;
            codes.extend(glyph.to_be_bytes());
        }
        Ok(codes)
    }

    /// The glyph `charmap`, the program's, maps `c` to, past `.notdef` and within the
    /// program's glyphs.
    fn mapped(&self, charmap: &Charmap<'_>, c: char) -> Option<u16> {
        let glyph = charmap.map(c)?.to_u32();
        let glyph = u16::try_from(glyph).ok()?;
        (glyph != 0 && glyph < self.glyphs).then_some(glyph)
    }

    /// Glyph `glyph`'s advance in thousandths of an em, as the font [`write()`] writes
    /// gives it in its `/W` (ISO 32000-2, 9.7.4.3): its advance in `hmtx`, a thousand
    /// times over the program's units per em, to the nearest whole thousandth, as PDFium
    /// reads a CIDFont's widths as integers. So text laid out by it is where every
    /// reader draws it. 0 for a glyph past its count.
    #[must_use]
    pub fn width(&self, glyph: u16) -> f32 {
        if glyph >= self.glyphs {
            return 0.0;
        }
        let (at, long) = self.hmtx;
        let entry = at + 4 * usize::from(glyph.min(long - 1));
        let advance = self
            .data
            .get(entry..entry + 2)
            .map_or(0, |b| u16::from_be_bytes([b[0], b[1]]));
        (f64::from(advance) * 1000.0 / f64::from(self.upem)).round() as f32
    }

    /// Its heights and box, in thousandths of an em, as a layout measures a line by:
    /// its ascent and descent as skrifa gives them (its `OS/2`'s typographic ones
    /// where it says to use them, else its `hhea`'s), and its `head`'s box.
    #[must_use]
    pub fn metrics(&self) -> FontMetrics {
        self.metrics
    }

    /// How many glyphs it has, `.notdef` among them, as its `maxp` says.
    #[must_use]
    pub fn glyph_count(&self) -> u16 {
        self.glyphs
    }

    /// What its licence lets a document do with it, as its `OS/2` table's `fsType`
    /// says: for a caller to read before drawing in it, as one writing PDF/A must.
    #[must_use]
    pub fn embedding(&self) -> Embedding {
        self.embedding
    }
}

impl fmt::Debug for Program {
    /// Its length, how many glyphs, its units per em and what its licence allows;
    /// never its names or a byte of it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Program")
            .field("len", &self.data.len())
            .field("glyphs", &self.glyphs)
            .field("units_per_em", &self.upem)
            .field("embedding", &self.embedding)
            .finish_non_exhaustive()
    }
}

/// Face `index` of `data`, as read-fonts reads it, refused where it isn't one.
fn face(data: &[u8], index: u32) -> Result<FontRef<'_>, Error> {
    match FileRef::new(data).map_err(|_| Error::NotSfnt)? {
        FileRef::Font(font) => match index {
            0 => Ok(font),
            _ => Err(Error::Index { index, faces: 1 }),
        },
        FileRef::Collection(fonts) => {
            let faces = fonts.len();
            if index >= faces {
                return Err(Error::Index { index, faces });
            }
            fonts.get(index).map_err(|_| Error::NotSfnt)
        }
    }
}

/// What a program's licence lets a document do with it: its `OS/2` table's `fsType`,
/// as OpenType gives its bits ([`Program::embedding`]). A program without one says
/// nothing against embedding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Embedding {
    /// What a document it's embedded in may be used for.
    pub level: Level,
    /// Whether it may be cut to the glyphs drawn: false where it asks to be embedded
    /// whole.
    pub subsetting: bool,
    /// Whether only its bitmaps may be embedded, not its outlines.
    pub bitmaps_only: bool,
}

impl Embedding {
    /// Whether [`write()`] embeds a program of this licence under `options`: not one
    /// restricted to its owner's leave ([`Level::Restricted`]) or to its bitmaps, which
    /// it refuses with [`Error::Restricted`], unless they say the caller has the right
    /// ([`Options::licensed`]). For a caller to ask before drawing.
    #[must_use]
    pub fn allows(&self, options: &Options) -> bool {
        let restricted = self.level == Level::Restricted || self.bitmaps_only;
        !restricted || options.licensed
    }

    /// What `fs_type`, an `OS/2` table's, allows. Where more than one of the usage bits
    /// is set, the least restrictive is taken, as OpenType has it.
    pub(crate) fn of(fs_type: u16) -> Embedding {
        let level = match fs_type & 0x000E {
            0 => Level::Installable,
            usage if usage & 0x0008 != 0 => Level::Editable,
            usage if usage & 0x0004 != 0 => Level::Preview,
            _ => Level::Restricted,
        };
        Embedding {
            level,
            subsetting: fs_type & 0x0100 == 0,
            bitmaps_only: fs_type & 0x0200 != 0,
        }
    }
}

/// What a document a program is embedded in may be used for ([`Embedding`]), least
/// restrictive first, so that one compares less than one more restrictive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Level {
    /// Anything, the font installed from it among it: no usage bit set.
    Installable,
    /// Viewed and edited, its text changed: bit 3.
    Editable,
    /// Viewed and printed, not edited: bit 2.
    Preview,
    /// Not embedded at all without the font's owner's leave: bit 1.
    Restricted,
}

/// The glyphs a document draws in a program, each with the text it stands for, which
/// the `/ToUnicode` of the font [`write()`] writes gives back. Its `Debug` says how many,
/// never which or their text.
///
/// Text is kept a glyph, not a character: two characters a program draws with one
/// glyph (a no-break space and a space, in some fonts) show one code, and read back as
/// the smaller of their texts, as `str` orders them, whichever was drawn first. So
/// glyphs gathered apart join to the same whatever order they're joined in
/// ([`Glyphs::merge`]).
#[derive(Clone, Default)]
pub struct Glyphs {
    pub(crate) texts: BTreeMap<u16, String>,
}

impl Glyphs {
    /// None yet.
    #[must_use]
    pub fn new() -> Glyphs {
        Glyphs::default()
    }

    /// `text`'s codes in `program` ([`Program::codes`]), each character's glyph kept
    /// with it.
    ///
    /// # Errors
    ///
    /// [`Unmapped`] for the first character the program has no glyph for; then no glyph
    /// of `text` is kept.
    pub fn add(&mut self, program: &Program, text: &str) -> Result<Vec<u8>, Unmapped> {
        let codes = program.codes(text)?;
        let mut character = [0; 4];
        for (c, &code) in text.chars().zip(codes.as_chunks::<2>().0) {
            let glyph = u16::from_be_bytes(code);
            self.insert(glyph, c.encode_utf8(&mut character));
        }
        Ok(codes)
    }

    /// Keeps `glyph`, with `text` for what it stands for, where it has no text yet or
    /// one after `text` as `str` orders them: a glyph a caller draws by its number,
    /// with a text of any length.
    pub fn insert(&mut self, glyph: u16, text: &str) {
        match self.texts.get_mut(&glyph) {
            Some(kept) if text < kept.as_str() => text.clone_into(kept),
            Some(_) => {}
            None => {
                self.texts.insert(glyph, text.to_owned());
            }
        }
    }

    /// Keeps every glyph `other` keeps, each with its text as [`Glyphs::insert`]
    /// keeps it: where both keep a glyph, the smaller of their texts. So glyphs
    /// gathered apart (each appearance's, each thread's) join to the same whatever
    /// order they're joined in, and one font written of them serves every one.
    pub fn merge(&mut self, other: &Glyphs) {
        for (&glyph, text) in &other.texts {
            self.insert(glyph, text);
        }
    }

    /// Each glyph it keeps and its text, in the order of their numbers in the
    /// program: for a caller that cuts the program itself and writes its own
    /// `/ToUnicode`.
    pub fn iter(&self) -> impl Iterator<Item = (u16, &str)> {
        self.texts
            .iter()
            .map(|(&glyph, text)| (glyph, text.as_str()))
    }

    /// How many glyphs it keeps.
    #[must_use]
    pub fn len(&self) -> usize {
        self.texts.len()
    }

    /// Whether it keeps none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.texts.is_empty()
    }
}

impl fmt::Debug for Glyphs {
    /// How many glyphs; never which, or their text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Glyphs")
            .field("len", &self.texts.len())
            .finish()
    }
}

/// Why a program isn't opened for embedding, or the font isn't written ([`write()`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The bytes aren't a TrueType or OpenType font, nor a collection of them, or the
    /// face asked for has a table directory that doesn't read.
    #[error("not a TrueType or OpenType font program, nor a collection of them")]
    NotSfnt,
    /// The face's table directory isn't in order of its tables' tags, as OpenType asks
    /// and the subsetter finds them by, holds fewer tables than it counts, or isn't
    /// where a collection's first list of faces puts it, the one the subsetter reads.
    #[error(
        "the font program's table directory isn't in order of its tags, is cut short, \
         or isn't where its collection lists it"
    )]
    Directory,
    /// A table the program needs is missing or doesn't read as one, or holds what
    /// embedding it can't take: the table's tag.
    #[error("the font program's `{table}` table is missing or malformed")]
    Malformed {
        /// The table's tag, as OpenType spells it (`loca`, `cvt `).
        table: &'static str,
    },
    /// A face past a collection's, or other than 0 of a program that isn't one.
    #[error("face {index} asked of a font program of {faces}")]
    Index {
        /// The face asked for.
        index: u32,
        /// How many faces the program has.
        faces: u32,
    },
    /// Outlines that aren't TrueType's: CFF's, CFF2's or bitmaps alone.
    #[error("the font program's outlines are {0}, not TrueType's")]
    Outlines(Outlines),
    /// The program's licence doesn't let it be embedded ([`Embedding`]): restricted to
    /// its owner's leave, or to its bitmaps alone, where [`Options::licensed`] doesn't
    /// say the caller has the right ([`Embedding::allows`]).
    #[error("the font program's licence doesn't let its outlines be embedded")]
    Restricted(Embedding),
    /// A glyph a [`Glyphs`] keeps at or past the program's count: a `Glyphs` of another
    /// program, or a glyph given to [`Glyphs::insert`].
    #[error("glyph {glyph} is past the font program's {count} glyphs")]
    Glyph {
        /// The glyph.
        glyph: u16,
        /// How many glyphs the program has.
        count: u16,
    },
    /// The program wasn't cut to the glyphs drawn: why, as the subsetter said it.
    #[error("the font program couldn't be cut to the glyphs drawn: {0}")]
    Subset(&'static str),
}

/// Outlines a program has that aren't TrueType's ([`Error::Outlines`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Outlines {
    /// An OpenType font's `CFF ` table.
    Cff,
    /// An OpenType font's `CFF2` table.
    Cff2,
    /// Bitmaps alone (`EBDT`, `CBDT` or `sbix`), with no outlines.
    Bitmaps,
}

impl fmt::Display for Outlines {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Outlines::Cff => "CFF",
            Outlines::Cff2 => "CFF2",
            Outlines::Bitmaps => "bitmaps alone",
        })
    }
}

/// A character a program has no glyph for ([`Program::codes`]). Nothing was drawn,
/// and nothing kept. Not exhaustive, so that it can come to say more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "U+{:04X} at byte {at} has no glyph in the font program",
    u32::from(*character)
)]
#[non_exhaustive]
pub struct Unmapped {
    /// Where the character is in the text, in bytes.
    pub at: usize,
    /// The character.
    pub character: char,
}
