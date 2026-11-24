//! A TrueType program embedded in a document as a [`LayoutFont`]
//! (`feature = "embed"` with `feature = "text-layout"`): the program itself,
//! which records nothing, and [`ProgramFont`], which records each glyph it
//! encodes.

use std::fmt;
use std::sync::{Mutex, MutexGuard, PoisonError, TryLockError};

use pdf_fonts::FontMetrics;
use pdf_fonts::embed::{Glyphs, Program};

use crate::wrap::LayoutFont;

/// A program lays text out as pdf-fonts' [`embed::write`](pdf_fonts::embed::write)
/// writes it: its metrics ([`Program::metrics`]), each character's advance its
/// glyph's ([`Program::width`], what every reader draws it at), and each
/// character's code its glyph's number in the program, two bytes, high byte
/// first ([`Program::codes`]). A character its `cmap` doesn't map has no
/// advance and no code, so a layout refuses it rather than draw `.notdef`.
///
/// Nothing is recorded: the glyphs drawn are for the caller to gather from what
/// it laid out, or to record as it goes with a [`ProgramFont`].
impl LayoutFont for Program {
    fn font_metrics(&self) -> FontMetrics {
        self.metrics()
    }

    fn char_advance(&self, ch: char) -> Option<f32> {
        let glyph = self.glyph(ch)?;
        Some(self.width(glyph))
    }

    fn encode_char(&self, ch: char, out: &mut Vec<u8>) -> bool {
        let Some(glyph) = self.glyph(ch) else {
            return false;
        };
        out.extend(glyph.to_be_bytes());
        true
    }
}

/// A program laid out in as itself ([`Program`]'s [`LayoutFont`]), each
/// character's glyph recorded in shared [`Glyphs`] as it's encoded, with the
/// character it was drawn for ([`Glyphs::add`]): for a caller that draws
/// through [`Canvas::text_box`](crate::Canvas::text_box) or
/// [`layout_text`](crate::layout_text) and embeds what was drawn, a font
/// written of the glyphs once the drawing is done.
///
/// The glyphs are behind a [`Mutex`], so that a font shared by threads drawing
/// pages records into one set without a copy of it: each character takes the
/// lock once, uncontended where one thread draws. A lock a thread panicked
/// holding is taken as it is, as what's recorded is whole at every step.
///
/// A layout encodes every character before it draws: one refused for a
/// character the program has no glyph for has recorded the glyphs of the
/// characters before it, which a font written of them then holds though
/// nothing draws them, harmlessly. Justified text is laid out flush left, as
/// word spacing (`Tw`) applies to a single-byte code 32 alone (ISO 32000-2,
/// 9.3.3), which none of its two-byte codes is: so its lines aren't spread to
/// the box, and nothing else of the layout changes.
///
/// Its `Debug` gives the program's counts and how many glyphs are recorded,
/// never which, or their text.
///
/// ```
/// use std::sync::Mutex;
///
/// use pdf_gfx::pdf_fonts::embed::{Glyphs, Program};
/// use pdf_gfx::{ProgramFont, Rect, TextBoxStyle, layout_text};
///
/// # let bytes = pdf_fixtures::font::sample(1000).build();
/// let program = Program::open(bytes, 0)?;
/// let glyphs = Mutex::new(Glyphs::new());
/// let font = ProgramFont::new(&program, &glyphs);
/// let style = TextBoxStyle {
///     size: Some(12.0),
///     ..TextBoxStyle::default()
/// };
/// let laid = layout_text(&font, "AgA", Rect::from_xywh(0.0, 0.0, 200.0, 20.0), &style)?;
///
/// // Two bytes a character, each its glyph's number; each glyph recorded once.
/// let a = program.glyph('A').unwrap().to_be_bytes();
/// assert_eq!(laid.lines[0].codes[..2], a);
/// assert_eq!(glyphs.lock().unwrap().len(), 2);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy)]
pub struct ProgramFont<'a> {
    program: &'a Program,
    glyphs: &'a Mutex<Glyphs>,
}

impl<'a> ProgramFont<'a> {
    /// `program`, laid out in, recording each glyph it encodes in `glyphs`.
    #[must_use]
    pub const fn new(program: &'a Program, glyphs: &'a Mutex<Glyphs>) -> ProgramFont<'a> {
        ProgramFont { program, glyphs }
    }

    /// The program it lays text out in.
    #[must_use]
    pub const fn program(&self) -> &'a Program {
        self.program
    }

    /// The glyphs recorded, to record more: the lock, taken as it is where a
    /// thread panicked holding it.
    fn recorded(&self) -> MutexGuard<'a, Glyphs> {
        self.glyphs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl LayoutFont for ProgramFont<'_> {
    fn font_metrics(&self) -> FontMetrics {
        self.program.font_metrics()
    }

    fn char_advance(&self, ch: char) -> Option<f32> {
        self.program.char_advance(ch)
    }

    fn encode_char(&self, ch: char, out: &mut Vec<u8>) -> bool {
        let mut character = [0; 4];
        match self
            .recorded()
            .add(self.program, ch.encode_utf8(&mut character))
        {
            Ok(codes) => {
                out.extend(codes);
                true
            }
            Err(_) => false,
        }
    }
}

impl fmt::Debug for ProgramFont<'_> {
    /// The program's counts and how many glyphs are recorded, never which, or
    /// their text; `None` for those where the caller holds the lock, which
    /// isn't waited for.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let recorded = match self.glyphs.try_lock() {
            Ok(glyphs) => Some(glyphs.len()),
            Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner().len()),
            Err(TryLockError::WouldBlock) => None,
        };
        f.debug_struct("ProgramFont")
            .field("program", self.program)
            .field("glyphs", &recorded)
            .finish()
    }
}
