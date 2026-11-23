//! A TrueType program embedded in a document to draw Unicode text in
//! (`EmbeddedFont`): the glyphs its pages draw recorded as they're drawn, and
//! the font written at each save.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use pdf_content_streams::Res;
use pdf_fonts::embed::{self, Glyphs, Program, Unmapped};
use pdf_objects::ObjectId;
use pdf_objects::object::Value;
use pdf_structure::write::Deflate;

use crate::build;
use crate::error::Error;

/// A TrueType program embedded in a [`Document`](crate::Document) to draw
/// Unicode text in ([`Document::embed_font`](crate::Document::embed_font)):
/// [`EmbeddedFont::res`] is the resource a page's content names it by, and
/// [`EmbeddedFont::codes`] a string's codes in it, each character's glyph
/// recorded as they're made. At each save the document writes the font, its
/// program cut to the glyphs recorded by then (pdf-fonts' [`embed::write`]),
/// at the number it reserved for it; a code is its glyph's number in the
/// program, never in the cut, so what a page drew before more text was drawn
/// shows the same glyphs after.
///
/// A clone is the same font: its program's bytes and the glyphs recorded are
/// shared, with its document too, and it's `Send` and `Sync`, so pages drawn
/// on other threads draw in it. Its program's metrics and advances are read
/// without waiting; a string's codes take a lock while its glyphs are
/// recorded, once a call.
///
/// It's its document's: drawn on another document's pages, its resource names
/// an object there that isn't the font. And the font is in its document only
/// once a save has written it: [`Document::structure`](crate::Document::structure)
/// and [`Document::into_structure`](crate::Document::into_structure) before
/// then hold nothing at its number.
///
/// Text is drawn a character at a time, in the order given, each character
/// the glyph the program's `cmap` maps it to: there's no shaping, so no
/// ligatures, no kerning and no reordering of right-to-left text. And text is
/// kept a glyph, not a character ([`Glyphs`]): two characters the program
/// draws with one glyph show one code, which reads back as the smaller of
/// their texts.
///
/// Its `Debug` gives its number, its program's counts and how many glyphs are
/// recorded, never which, or their text.
#[derive(Clone)]
pub struct EmbeddedFont {
    /// Its number, reserved by its document, which every save writes it at.
    id: ObjectId,
    /// The resource its number is ([`Res::written`]).
    res: Res,
    program: Program,
    /// The glyphs drawn in it, shared with its document.
    glyphs: Arc<Mutex<Glyphs>>,
}

impl EmbeddedFont {
    /// The resource it is, to draw text in it with: what
    /// [`ContentWriter::font`](pdf_content_streams::ContentWriter::font) and
    /// [`Canvas::text_at`](pdf_gfx::Canvas::text_at) take. It's the object
    /// its document reserved for it, which a save writes the font at.
    #[must_use]
    pub fn res(&self) -> Res {
        self.res.clone()
    }

    /// `text`'s codes in it, to show with its [`res`](EmbeddedFont::res):
    /// two bytes a character, high byte first, its glyph's number in the
    /// program ([`Program::codes`]), each glyph recorded with its character,
    /// so that the font written at the next save has it.
    ///
    /// # Errors
    ///
    /// [`Unmapped`] for the first character the program has no glyph for,
    /// with where it is: a control character such as `\n`, a variation
    /// selector its `cmap` doesn't map, a code point it hasn't. Then nothing
    /// is drawn of `text`, and no glyph of it recorded.
    pub fn codes(&self, text: &str) -> Result<Vec<u8>, Unmapped> {
        self.recorded().add(&self.program, text)
    }

    /// The program it embeds: its metrics, each glyph's advance, and what its
    /// licence allows ([`Program::embedding`]).
    ///
    /// The program itself records nothing: codes made through it
    /// ([`Program::codes`]), or text laid out in it, draw glyphs the font is
    /// written without, unless they're given to [`record`](Self::record).
    #[must_use]
    pub fn program(&self) -> &Program {
        &self.program
    }

    /// Records `glyphs`, drawn in its program by something that didn't
    /// record them here, so that the font written at the next save has
    /// them: each joined to those recorded as [`Glyphs::merge`] joins them,
    /// in any order. pdf-forms' `appearance::text_with_fallback`, given
    /// [`res`](Self::res) and [`program`](Self::program) as its fallback,
    /// returns the glyphs an appearance draws (`MadeWithFallback::glyphs`),
    /// which are recorded this way; codes of a caller's own making by glyph
    /// number are too ([`Glyphs::insert`]).
    ///
    /// A glyph is a number in this font's program: one of another program's
    /// records whatever glyph has its number here.
    ///
    /// # Errors
    ///
    /// [`embed::Error::Glyph`] for the first of `glyphs` at or past the
    /// program's count, as [`embed::write`] refuses it; then none of
    /// `glyphs` is recorded, so the font is still written at the next save.
    pub fn record(&self, glyphs: &Glyphs) -> Result<(), embed::Error> {
        let count = self.program.glyph_count();
        if let Some((glyph, _)) = glyphs.iter().find(|&(glyph, _)| glyph >= count) {
            return Err(embed::Error::Glyph { glyph, count });
        }
        self.recorded().merge(glyphs);
        Ok(())
    }

    /// The glyphs drawn in it, to record more: its lock, taken as it is where
    /// a thread panicked holding it, as what's recorded is whole at every
    /// step.
    fn recorded(&self) -> MutexGuard<'_, Glyphs> {
        self.glyphs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for EmbeddedFont {
    /// Its number, its program's counts and how many glyphs are recorded;
    /// never which, or their text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedFont")
            .field("id", &self.id)
            .field("program", &self.program)
            .field("glyphs", &self.recorded().len())
            .finish()
    }
}

/// What [`Canvas::text_box`](pdf_gfx::Canvas::text_box) and
/// [`layout_text`](pdf_gfx::layout_text) lay text out in: pdf-gfx's
/// [`ProgramFont`](pdf_gfx::ProgramFont) over its program and the glyphs its
/// document records. Its program's metrics ([`Program::metrics`]), each
/// character's advance its glyph's ([`Program::width`], what every reader
/// draws it at), and each character's code, its glyph recorded as it's
/// encoded, as [`EmbeddedFont::codes`] records it.
///
/// A layout encodes every character before it draws: one refused for a
/// character the program has no glyph for has recorded the glyphs of the
/// characters before it, which the font is then written with though nothing
/// draws them, harmlessly. Justified text is laid out flush left, as word
/// spacing (`Tw`) applies to a single-byte code 32 alone (ISO 32000-2,
/// 9.3.3), which none of its two-byte codes is: so its lines aren't spread to
/// the box, and nothing else of the layout changes.
///
/// It's a font to fall back to as well: after another in a
/// [`WithFallback`](pdf_gfx::WithFallback) (`WithFallback::new(&first,
/// &font, font.res())`), it draws what the first can't and records the
/// glyphs it draws, in [`Canvas::text_box_in`](pdf_gfx::Canvas::text_box_in)
/// and in pdf-forms' `appearance::text`, which take such a set.
#[cfg(feature = "text-layout")]
impl pdf_gfx::LayoutFont for EmbeddedFont {
    fn font_metrics(&self) -> pdf_fonts::FontMetrics {
        pdf_gfx::LayoutFont::font_metrics(&self.laid_out())
    }

    fn char_advance(&self, ch: char) -> Option<f32> {
        pdf_gfx::LayoutFont::char_advance(&self.laid_out(), ch)
    }

    fn encode_char(&self, ch: char, out: &mut Vec<u8>) -> bool {
        pdf_gfx::LayoutFont::encode_char(&self.laid_out(), ch, out)
    }
}

#[cfg(feature = "text-layout")]
impl EmbeddedFont {
    /// Its program as pdf-gfx lays text out in it, recording into the glyphs
    /// its document writes the font of.
    fn laid_out(&self) -> pdf_gfx::ProgramFont<'_> {
        pdf_gfx::ProgramFont::new(&self.program, &self.glyphs)
    }
}

/// What a document keeps of a font embedded in it: the font its handles are,
/// the numbers its first save took for its objects past the first, and how
/// it's written.
pub(crate) struct Kept {
    font: EmbeddedFont,
    /// The numbers of its objects past the first, which its first save took
    /// and each save after writes them at again.
    rest: Vec<ObjectId>,
    options: embed::Options,
}

impl Kept {
    /// `program`, embedded in `doc` at a number reserved for it now, to be
    /// written as `options` say: what its document keeps of it, and the
    /// font a caller draws in.
    pub(crate) fn new(
        doc: &mut pdf_structure::Document,
        program: Program,
        options: &embed::Options,
    ) -> (Kept, EmbeddedFont) {
        let id = doc.new_id();
        let font = EmbeddedFont {
            id,
            res: Res::written(id),
            program,
            glyphs: Arc::new(Mutex::new(Glyphs::new())),
        };
        let kept = Kept {
            font: font.clone(),
            rest: Vec::new(),
            options: *options,
        };
        (kept, font)
    }

    /// The font written into `doc` as its glyphs drawn by now say (pdf-fonts'
    /// [`embed::write`]): its Type 0 font at its reserved number, the rest at
    /// the numbers its first save took, a number more taken or one no longer
    /// needed taken out where their count changes, each reference `(i, 0)`
    /// among them made the `i`th's number, and each stream deflated by
    /// `deflate`. Nothing is put in `doc` where it isn't written.
    pub(crate) fn write(
        &mut self,
        doc: &mut pdf_structure::Document,
        deflate: Deflate,
    ) -> Result<(), Error> {
        let glyphs = self.font.recorded().clone();
        let objects = embed::write(&self.font.program, &glyphs, &self.options)?;
        let rest = objects.len().saturating_sub(1);
        while self.rest.len() < rest {
            self.rest.push(doc.new_id());
        }
        for extra in self.rest.drain(rest..) {
            doc.remove(extra);
        }
        let mut ids = Vec::with_capacity(objects.len());
        ids.push(self.font.id);
        ids.extend(&self.rest);
        let mut made = Vec::with_capacity(objects.len());
        for mut object in objects {
            if let Value::Stream(stream) = object.value_mut() {
                build::deflated(stream, deflate)?;
            }
            object.renumber(|to| match to.generation {
                0 => ids.get(to.number as usize).copied(),
                _ => None,
            });
            made.push(object);
        }
        for (object, &id) in made.into_iter().zip(&ids) {
            doc.set(id, object);
        }
        Ok(())
    }
}
