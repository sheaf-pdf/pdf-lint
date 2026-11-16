//! A program cut to the glyphs a document draws, by typst's subsetter, given the
//! glyphs' closure as it's formed here.

use std::{fmt, mem};

use subsetter::GlyphRemapper;

use super::{Error, Glyphs, Program, face, sfnt};

/// A program cut to some of its glyphs.
pub(crate) struct Subset {
    /// The program it's cut to, as the subsetter writes it: its `glyf`, `loca`,
    /// `head`, `hhea`, `hmtx` and `maxp`, its `cvt `, `fpgm` and `prep` where it has
    /// them, and its `name` and `post`; no `cmap`, which a CIDFont's program holds none
    /// of (ISO 32000-2, 9.9.1).
    pub(crate) program: Vec<u8>,
    /// Its glyphs' numbers in the program it's cut from, in their order: each glyph's
    /// number in it is its place here, so ascending, `.notdef` first.
    pub(crate) glyphs: Vec<u16>,
}

impl fmt::Debug for Subset {
    /// Its length and how many glyphs; never a byte of it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subset")
            .field("len", &self.program.len())
            .field("glyphs", &self.glyphs.len())
            .finish()
    }
}

impl Program {
    /// This program cut to the glyphs `glyphs` keeps: those, `.notdef` and every glyph
    /// a composite among them draws, in order of their numbers in this program, so the
    /// same glyphs cut to the same bytes whatever order they were drawn in.
    ///
    /// The closure is formed here, each glyph read once, and handed to the subsetter
    /// whole and in order, so that its own finds nothing more to take and its order is
    /// this one.
    pub(crate) fn subset(&self, glyphs: &Glyphs) -> Result<Subset, Error> {
        self.cut(glyphs.texts.keys().copied())
    }

    /// This program with every glyph, in its own order.
    pub(crate) fn whole(&self) -> Result<Subset, Error> {
        self.cut(0..self.glyphs)
    }

    /// This program cut to `drawn`, `.notdef` and their components.
    fn cut(&self, drawn: impl IntoIterator<Item = u16>) -> Result<Subset, Error> {
        let font = face(&self.data, self.index)?;
        let outlines = sfnt::Tables::of(&self.data, &font)?.glyf()?;
        let count = self.glyphs;
        let mut kept = vec![false; usize::from(count)];
        let mut work = Vec::new();
        for glyph in std::iter::once(0).chain(drawn) {
            let slot = kept.get_mut(usize::from(glyph));
            let slot = slot.ok_or(Error::Glyph { glyph, count })?;
            if !mem::replace(slot, true) {
                work.push(glyph);
            }
        }
        while let Some(glyph) = work.pop() {
            let data = outlines.glyph(glyph);
            let data = data.ok_or(Error::Malformed { table: "loca" })?;
            // Each within the count, as `open` checked.
            for component in sfnt::components(data) {
                if let Some(slot) = kept.get_mut(usize::from(component))
                    && !mem::replace(slot, true)
                {
                    work.push(component);
                }
            }
        }
        let glyphs: Vec<u16> = (0..count)
            .zip(&kept)
            .filter(|g| *g.1)
            .map(|g| g.0)
            .collect();
        let remapper = GlyphRemapper::new_from_glyphs_sorted(&glyphs);
        let program = subsetter::subset(&self.data, self.index, &remapper).map_err(said)?;
        Ok(Subset { program, glyphs })
    }
}

/// Why the subsetter refused, in this crate's words, so that its error type isn't
/// this crate's API.
fn said(error: subsetter::Error) -> Error {
    Error::Subset(match error {
        subsetter::Error::MalformedFont | subsetter::Error::UnknownKind => {
            "a table it rewrites doesn't read"
        }
        subsetter::Error::OverflowError => "a table it rewrites overflows",
        subsetter::Error::Unimplemented | subsetter::Error::CFFError => {
            "the program holds what it doesn't take"
        }
        _ => "it failed",
    })
}
