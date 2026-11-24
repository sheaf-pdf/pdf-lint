//! A text field's appearance: its text laid out in the widget's box as
//! PDFium as of chromium/8152 lays it out (`fpdfsdk/cpdfsdk_appstream.cpp`,
//! `SetAsTextField`, over `core/fpdfdoc/cpvt_variabletext.cpp`).

use std::borrow::Cow;
use std::fmt;

use pdf_annots::Quadding;
use pdf_content_streams::{Color, Res, Reserved};
use pdf_fonts::FontMetrics;
#[cfg(feature = "embed")]
use pdf_fonts::embed::{Glyphs, Program};
#[cfg(feature = "embed")]
use pdf_gfx::WithFallback;
use pdf_gfx::{
    Canvas, DefaultAppearance, FontSet, LaidOutText, Point, Rect, TextAlign, TextBoxStyle,
    UnencodableChar, VerticalAlign, WrapMode, face_for, layout_text,
};
use unicode_normalization::{IsNormalized, UnicodeNormalization, is_nfc_quick};
use unicode_segmentation::UnicodeSegmentation;

use super::frame::{self, Appearance, Unmade, WidgetLook};
use crate::field::FieldFlags;
use crate::limits::{Cap, Limits};

/// What a text field shows, and how, as plain data: the text, the field's
/// flags (`/Ff`), its maximum length (`/MaxLen`), and its quadding (`/Q`).
///
/// It may gain fields: build it with [`new`](Self::new) and set them.
/// `Debug` prints the text's length, never the text.
#[derive(Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct TextField<'a> {
    /// The text shown: the field's value, or what the caller shows for it
    /// (a value formatted for display, say).
    pub text: &'a str,
    /// The field's flags. [`FieldFlags::MULTILINE`],
    /// [`FieldFlags::PASSWORD`], and [`FieldFlags::COMB`] change what's
    /// drawn; [`FieldFlags::DO_NOT_SCROLL`] is an editor's rule and doesn't.
    pub flags: FieldFlags,
    /// The field's maximum length in characters: with [`FieldFlags::COMB`],
    /// the number of cells. Text past it is drawn only where the field
    /// isn't a comb field.
    pub max_len: Option<u32>,
    /// How each line is justified.
    pub quadding: Quadding,
}

impl<'a> TextField<'a> {
    /// A single line of `text`, left-justified, with no flags and no
    /// maximum length.
    #[must_use]
    pub const fn new(text: &'a str) -> TextField<'a> {
        TextField {
            text,
            flags: FieldFlags::from_bits(0),
            max_len: None,
            quadding: Quadding::Left,
        }
    }
}

impl fmt::Debug for TextField<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextField")
            .field("text_len", &self.text.len())
            .field("flags", &self.flags)
            .field("max_len", &self.max_len)
            .field("quadding", &self.quadding)
            .finish()
    }
}

/// The sizes PDFium chooses an automatic size from, in points
/// (`core/fpdfdoc/cpvt_variabletext.cpp`, `kFontSizeSteps`).
pub(super) const AUTO_SIZES: [f32; 25] = [
    4.0, 6.0, 8.0, 9.0, 10.0, 12.0, 14.0, 18.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 55.0,
    60.0, 70.0, 80.0, 90.0, 100.0, 110.0, 120.0, 130.0, 144.0,
];

/// The size below which PDFium takes a size for none and chooses one
/// (`core/fxcrt/fx_system.h`, `FXSYS_IsFloatZero`).
pub(super) const NO_SIZE: f32 = 0.0001;

/// How many of [`AUTO_SIZES`] a multiline field chooses from: the first
/// quarter, 12 points at most.
const MULTILINE_SIZES: usize = AUTO_SIZES.len() / 4;

/// Makes the appearance of a text field's widget: `field`'s text in `font`,
/// named by `font_res`, in the box `look` gives, at the size and in the
/// color of `da`, the field's default appearance (ISO 32000-2, §12.7.4.3 and
/// §12.7.5.3), as PDFium as of chromium/8152 makes it for the same field.
///
/// `font` is the field's font ([`FieldFont::layout`](super::FieldFont::layout)),
/// or any `pdf_gfx::FontSet`: fonts in order of preference, each grapheme
/// cluster of the text drawn in the first of them that has the whole of it
/// (`pdf_gfx::face_for`), the first under `font_res` and each other under the
/// resource the set gives it. `text_with_fallback` draws in the field's font
/// and a TrueType program the caller gives, with `embed`.
///
/// The box is `[0 0 width height]`, its sides swapped where the widget's
/// rotation is a quarter turn or three; the matrix turns it back onto the
/// widget's rectangle (`[0 1 -1 0 width 0]`, `[-1 0 0 -1 width height]`,
/// `[0 -1 1 0 0 height]` at one, two, and three quarter turns
/// counterclockwise, a negative rotation clockwise; a rotation that isn't a
/// multiple of 90 is none). What's drawn in it, in PDFium's order:
///
/// 1. The background, where `look` has one.
/// 2. A beveled or inset border's two shaded bands, with a border color or
///    without one: white and the background at half its brightness (a
///    middle gray with none) for beveled, a middle and a light gray for
///    inset.
/// 3. The border, where it has a color: a solid frame for a solid, beveled,
///    or inset border (`pdf_gfx::Canvas::box_border`), an underline's line,
///    or for a dashed border one open path from its bottom left corner round
///    under its own dash array (`[3]` where it has none it can use).
/// 4. A comb field's dividers where its border is solid or dashed and has a
///    color: a line between each two cells, in the border's color, width,
///    and dashes, with or without text.
/// 5. `/Tx BMC`, then in a `q`..`Q` the box less the border's reach as a
///    clip (`re W n`), the text color (`da`'s, black where it gives none),
///    and the text in `BT`..`ET`, then `EMC`. With no text, `/Tx BMC EMC`
///    with nothing between.
///
/// How the text is laid out, the box less the border's reach being the
/// text box:
///
/// - **Size:** `da`'s where it gives one of `0.0001` or more. Otherwise
///   (`0 Tf`, no `Tf`, or a size below `0.0001`, which PDFium takes for
///   none and a content stream would write as `0`) the largest of PDFium's sizes (4 to 144
///   points, 4 to 12 for a multiline field) at which the text fits the text
///   box, found by PDFium's search, or 4 where none does. Comb text in a
///   font with a box of its own ([`FieldFont::layout`](super::FieldFont::layout))
///   is fitted to a cell by that box instead, the first font's of a set.
/// - **A single line:** line breaks (`\r`, `\n`) are dropped, and the line
///   is set at the text box's middle by the font's ascent and descent, as
///   given (`pdf_gfx::TextBoxStyle::font_heights_as_given`).
/// - **Multiline** ([`FieldFlags::MULTILINE`]): `\r`, `\n`, and `\r\n` end
///   a line, a line is wrapped before the word that would pass the text
///   box, keeping the spaces it's wrapped at, words found as PDFium finds
///   them (`pdf_gfx::TextBoxStyle::line_end_spaces`: `half-baked` is one
///   word), and lines are set from the top a line height apart, the font's
///   ascent less its descent.
/// - **Comb** ([`FieldFlags::COMB`] and a maximum length above `0`, as
///   PDFium draws it whatever the other flags): the text box is that many
///   equal cells, each of the text's first grapheme clusters centered in
///   one, from the first cell, the middle one, or the last by the quadding;
///   the rest isn't drawn and the appearance is
///   [`overflowed`](Appearance::overflowed).
/// - **Password** ([`FieldFlags::PASSWORD`]): each character is drawn as
///   `*`, one a Unicode scalar value of the text as given. No character of
///   the text reaches the font, the content, or an error.
/// - **Composed:** the text is cut into extended grapheme clusters (a
///   character and the marks that go with it, Unicode Standard Annex #29),
///   and each is drawn composed (Normalization Form C, Annex #15) where a
///   font has the whole of the composed cluster, else as given: so `e` and a
///   combining acute accent are drawn as `é`, and text already composed, or
///   whose composed form no font has, as given. A cluster is drawn in one
///   font, and never split across lines. `da`'s font name and its other
///   text state (`Tc`, `Tw`, `Tz`, `TL`, `Tr`, `Ts`) aren't used, as PDFium
///   neither draws nor measures by them.
///
/// The font's resource name in the content is the writer's, by what the
/// font is ([`Appearance::needs`]), not `da`'s. A font of a set is named only
/// where something is drawn in it.
///
/// A color's components are written within `0.0..=1.0`: one of `look`'s or
/// `da`'s outside it is drawn as the nearer end, as a default appearance
/// string's are read.
///
/// ```
/// use pdf_forms::appearance::{FieldFont, TextField, WidgetLook, text};
/// use pdf_forms::pdf_annots::pdf_content_streams::Color;
/// use pdf_forms::pdf_annots::pdf_gfx::{BoxBorder, BoxBorderStyle, DefaultAppearance};
/// use pdf_forms::pdf_fonts::Std14;
/// use pdf_forms::{Limits, Quadding};
///
/// // What's shown and how: the text, right-justified.
/// let mut field = TextField::new("Hello");
/// field.quadding = Quadding::Right;
/// // The widget: 100 by 20, a black dashed border of width 1 on white.
/// let mut look = WidgetLook::new(100.0, 20.0);
/// look.background = Some(Color::Gray(1.0));
/// look.border_color = Some(Color::Gray(0.0));
/// look.border = BoxBorder::new(1.0, BoxBorderStyle::Dashed).with_dash(&[3.0, 2.0]);
/// // The field's default appearance, and the font it names.
/// let da = DefaultAppearance::parse(b"/Helv 12 Tf 0 0 1 rg", &Limits::DEFAULT.annots.content).unwrap();
/// let font = FieldFont::standard(Std14::Helvetica);
///
/// let made = text(&field, &look, &da, font.res().unwrap(), &font.layout(), &Limits::DEFAULT).unwrap();
/// assert_eq!(made.font_size, 12.0);
/// assert!(!made.overflowed);
/// // The font is the content's one resource, under the name the content uses.
/// let need = &made.needs.0[0];
/// assert_eq!(need.res, font.res().unwrap());
/// let content = String::from_utf8(made.content).unwrap();
/// assert!(content.starts_with("q\n1 g\n0 0 100 20 re\nf\nQ\n"));
/// assert!(content.contains("/Tx BMC\nq\n1 1 98 18 re\nW\nn\n0 0 1 rg\nBT\n"));
/// assert!(content.contains(&format!("/{} 12 Tf\n", String::from_utf8_lossy(&need.name))));
/// assert!(content.ends_with("(Hello) Tj\nET\nQ\nEMC\n"));
/// ```
///
/// # Errors
///
/// [`Unmade::Limit`] with [`Cap::Text`] where the text is longer than
/// [`Limits::text`] bytes, or with [`Cap::CombCells`] where a comb field has
/// more cells than [`Limits::comb_cells`]; [`Unmade::NoBox`] for a box with no width or no
/// height; [`Unmade::NotFinite`] where a number of `look`, `da`'s size or
/// color, the text box, or the text laid out isn't finite, or a string of
/// the text laid out reaches past `1e35` from the box's origin (its place,
/// and one more than the bytes of its codes, each its size wide), further
/// than a reader working in `f32` may place it; [`Unmade::Unencodable`] for
/// a grapheme cluster no font has the whole of, as given or composed: the
/// first of its characters as given that the first font can't draw, at its
/// offset in the text given; [`Unmade::FontNotBindable`] where a font of a
/// set past the first, which some of the text is drawn in, has no resource.
/// Never [`Unmade::NotText`] or [`Unmade::Unreadable`], which are the read
/// layer's ([`widget_text`](super::widget_text),
/// [`text_for_widget`](super::text_for_widget)).
pub fn text<F: FontSet + ?Sized>(
    field: &TextField<'_>,
    look: &WidgetLook<'_>,
    da: &DefaultAppearance,
    font_res: Res,
    font: &F,
    limits: &Limits,
) -> Result<Appearance, Unmade> {
    Ok(made(field, look, da, font_res, font, limits)?.appearance)
}

/// A TrueType program to draw what a field's font can't, and the resource
/// the content names it by: its object to come, say, which the caller
/// writes once every appearance is made ([`text_with_fallback`]).
///
/// It may gain fields: build it with [`new`](Self::new). `Debug` prints the
/// program's counts, never a byte of it.
#[cfg(feature = "embed")]
#[derive(Clone)]
#[non_exhaustive]
pub struct Fallback<'a> {
    /// What the content names the program's font by.
    pub res: Res,
    /// The program, opened once by the caller (`pdf_fonts::embed::Program::open`).
    pub program: &'a Program,
}

#[cfg(feature = "embed")]
impl<'a> Fallback<'a> {
    /// `program`, named by `res`.
    #[must_use]
    pub const fn new(res: Res, program: &'a Program) -> Fallback<'a> {
        Fallback { res, program }
    }
}

#[cfg(feature = "embed")]
impl fmt::Debug for Fallback<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fallback")
            .field("program", self.program)
            .finish_non_exhaustive()
    }
}

/// An appearance made in a field's font and a fallback
/// ([`text_with_fallback`]), and the fallback's glyphs it draws.
///
/// `Debug` prints lengths and counts, never the content or a glyph.
#[cfg(feature = "embed")]
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub struct MadeWithFallback {
    /// The appearance.
    pub appearance: Appearance,
    /// The glyphs of the fallback's program the content draws, each with the
    /// character it's drawn for: none where the field's font draws all of
    /// the text. The caller joins every appearance's (`Glyphs::merge`) and
    /// writes one font of them, which every appearance's codes draw in, as
    /// a code is its glyph's number in the whole program. Nothing else
    /// keeps them: a font written without them draws nothing for the
    /// content's codes in it. (Where the fallback is a font a pdfs document
    /// embeds, they're given to its `EmbeddedFont::record`.)
    pub glyphs: Glyphs,
}

#[cfg(feature = "embed")]
impl fmt::Debug for MadeWithFallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MadeWithFallback")
            .field("appearance", &self.appearance)
            .field("glyphs", &self.glyphs.len())
            .finish()
    }
}

/// Makes the appearance of a text field's widget as [`text`] does, in
/// `font`, the field's, and `fallback`, a TrueType program, for what `font`
/// can't draw: each grapheme cluster of the text in the first of them that
/// has the whole of it (`font`'s own fonts first, where it's a set), the
/// fallback's codes each two bytes, its glyph's number in the whole program
/// (`pdf_fonts::embed::Program::codes`), named by `fallback.res`.
///
/// What's returned is the appearance and the fallback's glyphs it draws,
/// gathered from what's drawn alone: none of a value refused, or of text
/// past a comb field's cells. Where `font` draws all of the text, none, and
/// the appearance is [`text`]'s byte for byte, its resources `font`'s alone.
/// A password's `*` is drawn in the fallback only where `font` has none, so
/// no other character of a password reaches the glyphs. The same inputs
/// give the same appearance and glyphs, whatever else the caller draws: it
/// joins every appearance's glyphs (`pdf_fonts::embed::Glyphs::merge`, in
/// any order) and writes one font of them (`pdf_fonts::embed::write`, or a
/// subset of its own behind a `/CIDToGIDMap` stream). The program is laid
/// out in as it is, recording nothing anywhere else: the glyphs returned
/// are the only record of what the font must have.
///
/// A fallback that records the glyphs it draws itself (pdf-gfx's
/// `ProgramFont`, or a font a pdfs document embeds) is drawn in by [`text`]
/// instead, after `font` in a `pdf_gfx::WithFallback`: the content is the
/// same, and nothing is returned beside it.
///
/// Heights, the automatic size, and comb cells are the layout's by the
/// fonts drawn in (`pdf_gfx::layout_text`): a line's ascent and descent are
/// the largest and smallest of the fonts drawn on it, as PDFium sets a line
/// of two fonts; comb text at an automatic size is fitted to a cell by
/// `font`'s box, as PDFium fits it by the field's font whatever draws it.
/// PDFium itself draws in no font the caller gives: what's in two fonts has
/// no appearance of PDFium's to be held to.
///
/// ```
/// use pdf_forms::Limits;
/// use pdf_forms::appearance::{Fallback, FieldFont, TextField, WidgetLook, text_with_fallback};
/// use pdf_forms::pdf_content_streams::Res;
/// use pdf_forms::pdf_fonts::Std14;
/// use pdf_forms::pdf_fonts::embed::{self, Program};
/// use pdf_forms::pdf_gfx::DefaultAppearance;
/// use pdf_forms::pdf_objects::ObjectId;
///
/// # let bytes = pdf_fixtures::font::latin(1000).build();
/// // The caller's TrueType program, opened once, and the object its font is
/// // to be written as, once every appearance is made.
/// let program = Program::open(bytes, 0)?;
/// let fallback = Fallback::new(Res::written(ObjectId::new(20, 0)), &program);
///
/// let field = TextField::new("Łukasz");
/// let look = WidgetLook::new(100.0, 20.0);
/// let da = DefaultAppearance::parse(b"/Helv 12 Tf 0 g", &Limits::DEFAULT.annots.content).unwrap();
/// let font = FieldFont::standard(Std14::Helvetica);
/// let made = text_with_fallback(
///     &field,
///     &look,
///     &da,
///     font.res().unwrap(),
///     &font.layout(),
///     &fallback,
///     &Limits::DEFAULT,
/// )?;
///
/// // Helvetica has no `Ł`, which the program draws: the content names both.
/// assert_eq!(made.appearance.needs.0.len(), 2);
/// assert_eq!(made.glyphs.len(), 1);
/// // The font the caller writes, of every appearance's glyphs joined.
/// let objects = embed::write(&program, &made.glyphs, &embed::Options::default())?;
/// assert!(!objects.is_empty());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Errors
///
/// As [`text`]: [`Unmade::Unencodable`] for a grapheme cluster neither
/// `font` nor the program has the whole of, as given or composed (a CJK
/// ideograph or an emoji against a Latin program, a control character, a
/// variation selector or joiner the program's `cmap` doesn't map), at the
/// first of its characters as given that `font` can't draw, never drawn as
/// `?` or as the program's `.notdef`.
#[cfg(feature = "embed")]
pub fn text_with_fallback<F: FontSet + ?Sized>(
    field: &TextField<'_>,
    look: &WidgetLook<'_>,
    da: &DefaultAppearance,
    font_res: Res,
    font: &F,
    fallback: &Fallback<'_>,
    limits: &Limits,
) -> Result<MadeWithFallback, Unmade> {
    let fonts = WithFallback::new(font, fallback.program, fallback.res.clone());
    let made = made(field, look, da, font_res, &fonts, limits)?;
    let glyphs = made.glyphs(font.faces());
    Ok(MadeWithFallback {
        appearance: made.appearance,
        glyphs,
    })
}

/// An appearance made, with what was shown and how it was laid out, which
/// the fallback's glyphs are gathered from.
struct Made {
    appearance: Appearance,
    #[cfg(feature = "embed")]
    shown: Shown,
    #[cfg(feature = "embed")]
    laid: Option<LaidOutText>,
}

/// What [`text`] and [`text_with_fallback`] share: the appearance of
/// `field` in `font`, a set of fonts or one, its first named by `font_res`.
fn made<F: FontSet + ?Sized>(
    field: &TextField<'_>,
    look: &WidgetLook<'_>,
    da: &DefaultAppearance,
    font_res: Res,
    font: &F,
    limits: &Limits,
) -> Result<Made, Unmade> {
    if field.text.len() > limits.text {
        return Err(Unmade::Limit(Cap::Text));
    }
    let (bbox, matrix) = frame::placed(look)?;
    if da.font_size.is_some_and(|s| !s.is_finite()) || !frame::finite_color(da.color) {
        return Err(Unmade::NotFinite);
    }
    let look = &frame::in_range(look);
    let client = frame::client(look, bbox)?;
    let cells = field
        .max_len
        .filter(|&n| n > 0 && field.flags.contains(FieldFlags::COMB));
    if cells.is_some_and(|n| usize::try_from(n).map_or(true, |n| n > limits.comb_cells)) {
        return Err(Unmade::Limit(Cap::CombCells));
    }
    let shown = Shown::of(field, cells, font);
    let laid = lay_out(field, &shown, client, da, font)?;
    let fonts = match &laid {
        Some(laid) => resources(laid, font_res, font)?,
        None => Vec::new(),
    };

    let mut canvas = Canvas::new();
    frame::frame(&mut canvas, look, bbox);
    if let Some(cells) = shown.cells {
        frame::dividers(&mut canvas, look, client, cells);
    }
    match &laid {
        Some(laid) if !shown.text.is_empty() => canvas.with_tagged("Tx", |c| {
            c.with_state(|c| {
                c.clip_rect(client);
                c.fill_color(da.color.map_or(Color::Gray(0.0), frame::clamped));
                c.draw_layout_in(&fonts, laid);
            });
        }),
        _ => canvas.with_tagged("Tx", |_| {}),
    }
    let (content, needs) = canvas.finish(&Reserved::default());
    let appearance = Appearance {
        bbox,
        matrix,
        content,
        needs,
        font_size: laid.as_ref().map_or(0.0, |laid| laid.font_size),
        overflowed: shown.cut
            || laid
                .as_ref()
                .map_or(!shown.text.is_empty(), |laid| laid.overflowed),
    };
    Ok(Made {
        appearance,
        #[cfg(feature = "embed")]
        shown,
        #[cfg(feature = "embed")]
        laid,
    })
}

#[cfg(feature = "embed")]
impl Made {
    /// The glyphs drawn in `face`, a program's, each code of a run or a comb
    /// cell drawn in it two bytes, its glyph's number, with the character it
    /// draws (a `\t` drawn as a space).
    fn glyphs(&self, face: usize) -> Glyphs {
        let mut glyphs = Glyphs::new();
        let Some(laid) = self.laid.as_ref().filter(|_| !self.shown.text.is_empty()) else {
            return glyphs;
        };
        let mut keep = |text: Option<&str>, codes: Option<&[u8]>| {
            let (Some(text), Some(codes)) = (text, codes) else {
                return;
            };
            let mut character = [0; 4];
            for (ch, &code) in text.chars().zip(codes.as_chunks::<2>().0) {
                let ch = if ch == '\t' { ' ' } else { ch };
                let glyph = u16::from_be_bytes(code);
                glyphs.insert(glyph, ch.encode_utf8(&mut character));
            }
        };
        for line in &laid.lines {
            for run in line.runs.iter().filter(|run| run.face == face) {
                keep(
                    self.shown.text.get(run.text.clone()),
                    line.codes.get(run.codes.clone()),
                );
            }
            for cell in line.cells.iter().filter(|cell| cell.face == face) {
                keep(self.shown.text.get(cell.text.clone()), Some(&cell.codes));
            }
        }
        glyphs
    }
}

/// The resource each face of `font` that `laid` draws in is drawn under:
/// the first's `font_res`, each other's the set's own
/// (`pdf_gfx::FontSet::face_resource`), [`Unmade::FontNotBindable`] where
/// it has none. A face below the last drawn in that draws nothing stands
/// under `font_res`, which no string drawn names.
pub(super) fn resources<F: FontSet + ?Sized>(
    laid: &LaidOutText,
    font_res: Res,
    font: &F,
) -> Result<Vec<Res>, Unmade> {
    let drawn = || {
        (laid.lines.iter()).flat_map(|line| {
            (line.runs.iter().map(|run| run.face)).chain(line.cells.iter().map(|cell| cell.face))
        })
    };
    let last = drawn().max().unwrap_or(0);
    let mut fonts = Vec::with_capacity(last + 1);
    fonts.push(font_res.clone());
    for face in 1..=last {
        match font.face_resource(face) {
            Some(res) => fonts.push(res),
            None if drawn().any(|f| f == face) => return Err(Unmade::FontNotBindable),
            None => fonts.push(font_res.clone()),
        }
    }
    Ok(fonts)
}

/// The text as it's laid out: a single line's line breaks dropped, a comb
/// field's grapheme clusters past its cells cut, each cluster composed where
/// a font has it so, a password's characters each `*`; with each
/// character's byte offset in the caller's text, a composed cluster's each
/// the cluster's.
struct Shown {
    text: String,
    /// The byte offset in the caller's text of each character of `text`.
    from: Vec<usize>,
    /// Whether clusters past a comb field's cells were cut.
    cut: bool,
    /// The comb field's cells, if it's one.
    cells: Option<u32>,
    password: bool,
}

impl Shown {
    /// What `field` shows in `font`, with `cells` its comb cells.
    fn of<F: FontSet + ?Sized>(field: &TextField<'_>, cells: Option<u32>, font: &F) -> Shown {
        let flags = field.flags;
        let single = cells.is_some() || !flags.contains(FieldFlags::MULTILINE);
        let password = flags.contains(FieldFlags::PASSWORD);
        let text = field.text;
        let mut shown = Shown {
            text: String::with_capacity(text.len()),
            from: Vec::with_capacity(text.len()),
            cut: false,
            cells,
            password,
        };
        // Composed text not cut into cells is shown as given, a character
        // at a time, and so is a password's, a star a character.
        let composed = text.is_ascii() || is_nfc_quick(text.chars()) == IsNormalized::Yes;
        if password || (composed && cells.is_none()) {
            for (at, ch) in text.char_indices() {
                if single && is_break(ch) {
                    continue;
                }
                if cells.is_some_and(|n| shown.from.len() as u64 >= u64::from(n)) {
                    shown.cut = true;
                    break;
                }
                shown.push(if password && !is_break(ch) { '*' } else { ch }, at);
            }
            return shown;
        }

        // A cluster at a time, of the text with a single line's line breaks
        // dropped, each byte of it at its offset in the caller's.
        let (given, offsets) = match single && text.contains(['\r', '\n']) {
            false => (Cow::Borrowed(text), None),
            true => {
                let mut given = String::with_capacity(text.len());
                let mut offsets = Vec::with_capacity(text.len());
                for (at, ch) in text.char_indices().filter(|&(_, ch)| !is_break(ch)) {
                    given.push(ch);
                    offsets.extend(at..at + ch.len_utf8());
                }
                (Cow::Owned(given), Some(offsets))
            }
        };
        let offset = |at: usize| offsets.as_ref().map_or(at, |offsets| offsets[at]);
        let mut nfc = String::new();
        for (count, (at, cluster)) in clusters(&given).enumerate() {
            if cells.is_some_and(|n| count as u64 >= u64::from(n)) {
                shown.cut = true;
                break;
            }
            nfc.clear();
            if !composed {
                nfc.extend(cluster.nfc());
            }
            if !composed && nfc != cluster && face_for(font, &nfc).is_ok() {
                for ch in nfc.chars() {
                    shown.push(ch, offset(at));
                }
            } else {
                for (i, ch) in cluster.char_indices() {
                    shown.push(ch, offset(at + i));
                }
            }
        }
        shown
    }

    fn push(&mut self, ch: char, at: usize) {
        self.text.push(ch);
        self.from.push(at);
    }

    /// `err`, at a byte of [`text`](Self::text), at the character of
    /// `given`, the caller's text, it was shown for.
    fn unencodable(&self, err: UnencodableChar, given: &str) -> Unmade {
        if self.password {
            return Unmade::Unencodable(UnencodableChar {
                at: 0,
                character: err.character,
            });
        }
        let index = self.text[..err.at].chars().count();
        let at = self.from.get(index).copied().unwrap_or(0);
        Unmade::Unencodable(UnencodableChar {
            at,
            character: given[at..].chars().next().unwrap_or(err.character),
        })
    }
}

/// Whether `ch` ends a line: `\r` or `\n`.
fn is_break(ch: char) -> bool {
    ch == '\r' || ch == '\n'
}

/// Each extended grapheme cluster of `text` (Unicode Standard Annex #29),
/// with its byte offset: a byte at a time where it's ASCII but `\r`, each
/// byte a cluster of its own, as `pdf_gfx::layout_text` lays such text out
/// a character at a time.
fn clusters(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let ascii = text.is_ascii() && !text.contains('\r');
    let bytes = ascii.then(|| (0..text.len()).map(move |at| (at, &text[at..=at])));
    let graphemes = (!ascii).then(|| text.grapheme_indices(true));
    bytes
        .into_iter()
        .flatten()
        .chain(graphemes.into_iter().flatten())
}

/// `shown` laid out in `client`, the text box; none where the text box has
/// no width, which nothing fits.
fn lay_out<F: FontSet + ?Sized>(
    field: &TextField<'_>,
    shown: &Shown,
    client: Rect,
    da: &DefaultAppearance,
    font: &F,
) -> Result<Option<LaidOutText>, Unmade> {
    if client.width() <= 0.0 {
        return Ok(None);
    }
    let multiline = field.flags.contains(FieldFlags::MULTILINE);
    let mut style = TextBoxStyle {
        align: match field.quadding {
            Quadding::Left => TextAlign::Left,
            Quadding::Center => TextAlign::Center,
            Quadding::Right => TextAlign::Right,
        },
        vertical_align: match multiline {
            true => VerticalAlign::Top,
            false => VerticalAlign::Middle,
        },
        wrap: match multiline && shown.cells.is_none() {
            true => WrapMode::Word,
            false => WrapMode::NoWrap,
        },
        leading_factor: 1.0,
        comb: shown.cells,
        clip: false,
        line_end_spaces: true,
        font_heights_as_given: true,
        // ASCII's clusters are its characters, which it's laid out by alike.
        clusters: !shown.text.is_ascii(),
        ..TextBoxStyle::default()
    };

    let size = match da.size().filter(|size| *size >= NO_SIZE) {
        Some(size) => size,
        None => auto_size(field, shown, client, &style, font)?,
    };
    style.size = Some(size);
    let laid = layout_text(font, &shown.text, client, &style)
        .map_err(|err| shown.unencodable(err, field.text))?;
    if !placeable(&laid) {
        return Err(Unmade::NotFinite);
    }
    Ok(Some(laid))
}

/// How far from the box's origin a string of the text may reach: the place
/// it's shown at (`x` and `y` from `0`, each as far as it is), and one more
/// than the bytes of its codes, each its size and word spacing wide,
/// together: a glyph's code is a byte or more, so each glyph is counted.
/// No widget's text comes near it; past it, a reader working in `f32` may not
/// place the string. `pdf-gfx`'s interpreter refuses one that reaches past
/// `3e37`, counting each glyph its font's extent wide, and a standard 14
/// font's widths times a size pass `f32::MAX` from about `3e35`.
pub(crate) const REACH: f64 = 1e35;

/// Whether `laid` can be placed: its size and the place of every line, run,
/// and comb cell finite, and every string shown within [`REACH`].
fn placeable(laid: &LaidOutText) -> bool {
    let size = f64::from(laid.font_size.abs());
    let within = |at: Point, codes: &[u8], word_space: f32| {
        let wide = (codes.len() as f64 + 1.0) * (size + f64::from(word_space.abs()));
        at.is_finite()
            && (codes.is_empty() || f64::from(at.x.abs()) + f64::from(at.y.abs()) + wide <= REACH)
    };
    laid.font_size.is_finite()
        && laid.lines.iter().all(|line| {
            let run = |run: &pdf_gfx::LaidOutRun| {
                let at = Point::new(line.origin.x + run.x, line.origin.y);
                (line.codes.get(run.codes.clone())).is_some_and(|c| within(at, c, line.word_space))
            };
            within(line.origin, &line.codes, line.word_space)
                && line.runs.iter().all(run)
                && (line.cells.iter()).all(|cell| within(cell.origin, &cell.codes, 0.0))
        })
}

/// The size an automatic size (`0 Tf`) comes to for `shown` in `client`.
///
/// Comb text in a font with a box of its own fills a cell by that box: the
/// smaller of the cell's width and the text box's height, each in the box's
/// units (`fpdfsdk/pwl/cpwl_edit.cpp`, `GetCharArrayAutoFontSize`), the box
/// of a set's first font, the field's, whatever draws the text, as PDFium
/// reads its font map's first. Any other text takes the largest of
/// [`AUTO_SIZES`] (the first [`MULTILINE_SIZES`] for a multiline field) at
/// which it fits, as PDFium's binary search finds it
/// (`CPVT_VariableText::GetAutoFontSize` and `IsBigger`): the first size at
/// which it's bigger than the text box, its widest line wider or its lines'
/// heights together taller by more than `0.0001`, and the size before it;
/// the first size where even that is bigger, the last where none is. A
/// line's height is its font's ascent less its descent, or in more than one
/// font the largest ascent less the smallest descent of those drawn on it.
///
/// A grapheme cluster no font has is refused first, as the layout would
/// refuse it.
fn auto_size<F: FontSet + ?Sized>(
    field: &TextField<'_>,
    shown: &Shown,
    client: Rect,
    style: &TextBoxStyle,
    font: &F,
) -> Result<f32, Unmade> {
    let (advances, faces) = advances(shown, font, field.text)?;
    let metrics = font.face_metrics(0);
    if let (Some(cells), Some(b)) = (shown.cells, metrics.bbox) {
        let (box_w, box_h) = (b.x1 - b.x0, b.y1 - b.y0);
        let across = client.width() / cells as f32 * 1000.0 / box_w;
        let down = client.height() * 1000.0 / box_h;
        let size = across.min(down);
        if box_w > 0.0 && box_h > 0.0 && size.is_finite() && size > 0.0 {
            return Ok(size);
        }
    }

    if !(field.flags.contains(FieldFlags::MULTILINE) && shown.cells.is_none()) {
        return Ok(single_auto_size(&advances, &faces, client, font));
    }

    let sizes = &AUTO_SIZES[..MULTILINE_SIZES];
    let (ascent, descent) = faces_heights(&faces, font);
    let line = |size: f32| ascent * size * 0.001 - descent * size * 0.001;
    // Every line's in the field's font: its heights, a line each.
    let one = faces.iter().all(|&face| face == 0);
    Ok(search_sizes(sizes, |size| {
        let trial = TextBoxStyle {
            size: Some(size),
            ..*style
        };
        let (wide, tall) = match layout_text(font, &shown.text, client, &trial) {
            Ok(laid) => (
                laid.lines.iter().map(|l| l.width).fold(0.0, f32::max),
                match one {
                    true => line(size) * laid.lines.len() as f32,
                    false => (laid.lines.iter()).fold(0.0, |h, l| h + (l.ascent - l.descent)),
                },
            ),
            Err(_) => (0.0, 0.0),
        };
        is_bigger(wide, client.width()) || is_bigger(tall, client.height())
    }))
}

/// `text` on one line in `box_`, set at its middle by the faces' ascent and
/// descent as given (`pdf_gfx::TextBoxStyle::font_heights_as_given`) and
/// across by `align`, as PDFium as of chromium/8152 sets a push button's
/// caption, a combo box's text, and each line of a list box
/// (`fpdfsdk/cpdfsdk_appstream.cpp`, `GetPushButtonAppStream`,
/// `SetAsComboBox`, `SetAsListBox`, over `fpdfsdk/pwl/cpwl_edit_impl.cpp`);
/// none where there's nothing to draw: no text once its line breaks are
/// dropped, or a box with no width.
///
/// Line breaks are dropped and grapheme clusters composed where `font` has
/// them so, as [`text`] prepares a single line. The size is `size` where it's
/// `0.0001` or more; otherwise (none, `0`, or below) the largest of
/// [`AUTO_SIZES`] at which the line fits `box_` ([`single_auto_size`]).
pub(super) fn one_line<F: FontSet + ?Sized>(
    text: &str,
    box_: Rect,
    size: Option<f32>,
    align: TextAlign,
    font: &F,
) -> Result<Option<LaidOutText>, Unmade> {
    if size.is_some_and(|s| !s.is_finite()) {
        return Err(Unmade::NotFinite);
    }
    let shown = Shown::of(&TextField::new(text), None, font);
    let (advances, faces) = advances(&shown, font, text)?;
    if shown.text.is_empty() || box_.width() <= 0.0 {
        return Ok(None);
    }
    let mut style = TextBoxStyle {
        align,
        vertical_align: VerticalAlign::Middle,
        wrap: WrapMode::NoWrap,
        leading_factor: 1.0,
        clip: false,
        line_end_spaces: true,
        font_heights_as_given: true,
        clusters: !shown.text.is_ascii(),
        ..TextBoxStyle::default()
    };
    let size = match size.filter(|s| *s >= NO_SIZE) {
        Some(size) => size,
        None => single_auto_size(&advances, &faces, box_, font),
    };
    style.size = Some(size);
    let laid =
        layout_text(font, &shown.text, box_, &style).map_err(|err| shown.unencodable(err, text))?;
    if !placeable(&laid) {
        return Err(Unmade::NotFinite);
    }
    Ok(Some(laid))
}

/// The height of a line in `font`'s first face at `size`: its ascent less its
/// descent, as the layout sets lines by them ([`heights`]), as PDFium as of
/// chromium/8152 steps a list box's lines down (`SetAsListBox`).
pub(super) fn line_height<F: FontSet + ?Sized>(font: &F, size: f32) -> f32 {
    let (ascent, descent) = heights(font.face_metrics(0));
    ascent * size * 0.001 - descent * size * 0.001
}

/// The size an automatic size comes to for a single line of `advances` drawn
/// in `faces` of `font` inside `box_`.
fn single_auto_size<F: FontSet + ?Sized>(
    advances: &[f32],
    faces: &[usize],
    box_: Rect,
    font: &F,
) -> f32 {
    let (ascent, descent) = faces_heights(faces, font);
    search_sizes(&AUTO_SIZES, |size| {
        let wide = advances.iter().fold(0.0, |w, a| w + a * size * 0.001);
        let tall = ascent * size * 0.001 - descent * size * 0.001;
        is_bigger(wide, box_.width()) || is_bigger(tall, box_.height())
    })
}

/// The ascent and descent, in glyph units, a single line drawn in `faces` of
/// `font` is set by: the first face's when empty, a sole face's, or from `0`
/// the largest ascent and smallest descent of the faces drawn on it.
fn faces_heights<F: FontSet + ?Sized>(faces: &[usize], font: &F) -> (f32, f32) {
    match *faces {
        [] => heights(font.face_metrics(0)),
        [face] => heights(font.face_metrics(face)),
        _ => (faces.iter()).fold((0.0f32, 0.0f32), |(a, d), &face| {
            let (ascent, descent) = heights(font.face_metrics(face));
            (a.max(ascent), d.min(descent))
        }),
    }
}

/// The largest of `sizes` before the first at which `bigger` holds (or the
/// first where even that is bigger, the last where none is), by a binary
/// search over `sizes` as `std::lower_bound` makes it, so the same sizes are
/// tried as PDFium tries.
fn search_sizes(sizes: &[f32], mut bigger: impl FnMut(f32) -> bool) -> f32 {
    let (mut first, mut count) = (0, sizes.len());
    while count > 0 {
        let step = count / 2;
        if bigger(sizes[first + step]) {
            count = step;
        } else {
            first += step + 1;
            count -= step + 1;
        }
    }
    match first {
        0 => sizes[0],
        n => sizes[n - 1],
    }
}

/// The heights, in glyph units, the layout sets a line in a font of
/// `metrics` by: its own as given, or its floors where they give no height
/// (`font_heights_as_given`).
fn heights(metrics: FontMetrics) -> (f32, f32) {
    let (a, d) = (metrics.ascent / 1000.0, metrics.descent / 1000.0);
    match a.is_finite() && d.is_finite() && a > d {
        true => (metrics.ascent, metrics.descent),
        false => (a.max(0.70) * 1000.0, d.min(-0.15) * 1000.0),
    }
}

/// Each character's advance in glyph units, in the font of `font` that
/// draws its grapheme cluster, line breaks left out, and the fonts drawn
/// in, in the order they're first drawn in; the first cluster no font has
/// refused, at its character of `given`, the caller's text, as the layout
/// refuses it. In a single font, each character's advance in it, the first
/// it has none for refused.
fn advances<F: FontSet + ?Sized>(
    shown: &Shown,
    font: &F,
    given: &str,
) -> Result<(Vec<f32>, Vec<usize>), Unmade> {
    let mut advances = Vec::with_capacity(shown.from.len());
    let mut faces: Vec<usize> = Vec::new();
    let one = font.faces() == 1;
    for (at, unit) in clusters(&shown.text) {
        if unit.chars().all(is_break) {
            continue;
        }
        let face = match one {
            true => 0,
            false => face_for(font, unit).map_err(|err| {
                let at = at + err.at;
                shown.unencodable(UnencodableChar { at, ..err }, given)
            })?,
        };
        for (i, ch) in unit.char_indices() {
            let mapped = if ch == '\t' { ' ' } else { ch };
            match font.face_advance(face, mapped) {
                Some(advance) => advances.push(advance),
                None => {
                    let at = at + i;
                    return Err(shown.unencodable(UnencodableChar { at, character: ch }, given));
                }
            }
        }
        if !faces.contains(&face) {
            faces.push(face);
        }
    }
    Ok((advances, faces))
}

/// Whether `a` is bigger than `b` by more than `0.0001`, as PDFium compares
/// them (`core/fxcrt/fx_system.h`, `FXSYS_IsFloatBigger`).
pub(super) fn is_bigger(a: f32, b: f32) -> bool {
    a > b && a - b >= 0.0001
}
