//! A text field's appearance: its text laid out in the widget's box as
//! PDFium as of chromium/8152 lays it out (`fpdfsdk/cpdfsdk_appstream.cpp`,
//! `SetAsTextField`, over `core/fpdfdoc/cpvt_variabletext.cpp`).

use std::fmt;

use pdf_annots::Quadding;
use pdf_content_streams::{Color, Res, Reserved};
use pdf_gfx::{
    Canvas, DefaultAppearance, LaidOutText, LayoutFont, Rect, TextAlign, TextBoxStyle,
    UnencodableChar, VerticalAlign, WrapMode, layout_text,
};

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
const AUTO_SIZES: [f32; 25] = [
    4.0, 6.0, 8.0, 9.0, 10.0, 12.0, 14.0, 18.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 55.0,
    60.0, 70.0, 80.0, 90.0, 100.0, 110.0, 120.0, 130.0, 144.0,
];

/// How many of [`AUTO_SIZES`] a multiline field chooses from: the first
/// quarter, 12 points at most.
const MULTILINE_SIZES: usize = AUTO_SIZES.len() / 4;

/// Makes the appearance of a text field's widget: `field`'s text in `font`,
/// named by `font_res`, in the box `look` gives, at the size and in the
/// color of `da`, the field's default appearance (ISO 32000-2, 12.7.4.3 and
/// 12.7.5.3), as PDFium as of chromium/8152 makes it for the same field.
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
/// - **Size:** `da`'s where it gives one above `0`. Otherwise (`0 Tf`, no
///   `Tf`, or a size below `0`) the largest of PDFium's sizes (4 to 144
///   points, 4 to 12 for a multiline field) at which the text fits the text
///   box, found by PDFium's search, or 4 where none does. Comb text in a
///   font with a box of its own ([`FieldFont::layout`](super::FieldFont::layout))
///   is fitted to a cell by that box instead.
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
///   equal cells, each of the text's first characters centered in one,
///   from the first cell, the middle one, or the last by the quadding; the
///   rest isn't drawn and the appearance is
///   [`overflowed`](Appearance::overflowed).
/// - **Password** ([`FieldFlags::PASSWORD`]): each character is drawn as
///   `*`. No character of the text reaches the font, the content, or an
///   error.
/// - Characters are Unicode scalar values. `da`'s font name and its other
///   text state (`Tc`, `Tw`, `Tz`, `TL`, `Tr`, `Ts`) aren't used, as PDFium
///   neither draws nor measures by them.
///
/// The font's resource name in the content is the writer's, by what the
/// font is ([`Appearance::needs`]), not `da`'s.
///
/// # Errors
///
/// [`Unmade::Limit`] with [`Cap::Text`] where the text is longer than
/// [`Limits::text`] bytes, or a comb field has more cells than that;
/// [`Unmade::NoBox`] for a box with no width or no
/// height; [`Unmade::NotFinite`] where a number of `look`, `da`'s size or
/// color, or the text laid out isn't finite; [`Unmade::Unencodable`] for a
/// character `font` has no code for.
pub fn text<F: LayoutFont + ?Sized>(
    field: &TextField<'_>,
    look: &WidgetLook<'_>,
    da: &DefaultAppearance,
    font_res: Res,
    font: &F,
    limits: &Limits,
) -> Result<Appearance, Unmade> {
    if field.text.len() > limits.text {
        return Err(Unmade::Limit(Cap::Text));
    }
    let (bbox, matrix) = frame::placed(look)?;
    if da.font_size.is_some_and(|s| !s.is_finite()) || !frame::finite_color(da.color) {
        return Err(Unmade::NotFinite);
    }
    let client = frame::client(look, bbox);
    let shown = Shown::of(field);
    if shown
        .cells
        .is_some_and(|n| usize::try_from(n).map_or(true, |n| n > limits.text))
    {
        return Err(Unmade::Limit(Cap::Text));
    }
    let laid = lay_out(field, &shown, client, da, font)?;

    let mut canvas = Canvas::new();
    frame::frame(&mut canvas, look, bbox);
    if let Some(cells) = shown.cells {
        frame::dividers(&mut canvas, look, client, cells);
    }
    match &laid {
        Some(laid) if !shown.text.is_empty() => canvas.with_tagged("Tx", |c| {
            c.with_state(|c| {
                c.clip_rect(client);
                c.fill_color(da.color.unwrap_or(Color::Gray(0.0)));
                c.draw_layout(font_res, laid);
            });
        }),
        _ => canvas.with_tagged("Tx", |_| {}),
    }
    let (content, needs) = canvas.finish(&Reserved::default());
    Ok(Appearance {
        bbox,
        matrix,
        content,
        needs,
        font_size: laid.as_ref().map_or(0.0, |laid| laid.font_size),
        overflowed: shown.cut
            || laid
                .as_ref()
                .map_or(!shown.text.is_empty(), |laid| laid.overflowed),
    })
}

/// The text as it's laid out: a single line's line breaks dropped, a comb
/// field's characters past its cells cut, a password's characters each
/// `*`; with each character's byte offset in the caller's text.
struct Shown {
    text: String,
    /// The byte offset in the caller's text of each character of `text`.
    from: Vec<usize>,
    /// Whether characters past a comb field's cells were cut.
    cut: bool,
    /// The comb field's cells, if it's one.
    cells: Option<u32>,
    password: bool,
}

impl Shown {
    fn of(field: &TextField<'_>) -> Shown {
        let flags = field.flags;
        let cells = field
            .max_len
            .filter(|&n| n > 0 && flags.contains(FieldFlags::COMB));
        let single = cells.is_some() || !flags.contains(FieldFlags::MULTILINE);
        let password = flags.contains(FieldFlags::PASSWORD);
        let mut shown = Shown {
            text: String::new(),
            from: Vec::new(),
            cut: false,
            cells,
            password,
        };
        for (at, ch) in field.text.char_indices() {
            if single && (ch == '\r' || ch == '\n') {
                continue;
            }
            if cells.is_some_and(|n| shown.from.len() as u64 >= u64::from(n)) {
                shown.cut = true;
                break;
            }
            let line_break = ch == '\r' || ch == '\n';
            shown
                .text
                .push(if password && !line_break { '*' } else { ch });
            shown.from.push(at);
        }
        shown
    }

    /// `err`, a layout's, at the character's offset in the caller's text.
    fn unencodable(&self, err: UnencodableChar) -> Unmade {
        let index = self.text[..err.at].chars().count();
        let at = match self.password {
            true => 0,
            false => self.from.get(index).copied().unwrap_or(0),
        };
        Unmade::Unencodable(UnencodableChar {
            at,
            character: err.character,
        })
    }
}

/// `shown` laid out in `client`, the text box; none where the text box has
/// no width, which nothing fits.
fn lay_out<F: LayoutFont + ?Sized>(
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
        ..TextBoxStyle::default()
    };

    // Each character's advance, in glyph units, the first one the font
    // has none for refused as the layout would refuse it.
    let mut advances = Vec::with_capacity(shown.from.len());
    for (at, ch) in shown.text.char_indices() {
        let mapped = if ch == '\t' { ' ' } else { ch };
        match font.char_advance(mapped) {
            Some(advance) => advances.push(advance),
            None if ch == '\r' || ch == '\n' => {}
            None => {
                return Err(shown.unencodable(UnencodableChar { at, character: ch }));
            }
        }
    }

    let size = match da.size() {
        Some(size) => size,
        None => auto_size(field, shown, client, &style, &advances, font),
    };
    style.size = Some(size);
    let laid =
        layout_text(font, &shown.text, client, &style).map_err(|err| shown.unencodable(err))?;
    let placed = laid
        .lines
        .iter()
        .flat_map(|line| std::iter::once(line.origin).chain(line.cells.iter().map(|c| c.0)))
        .all(|p| p.is_finite());
    if !(laid.font_size.is_finite() && placed) {
        return Err(Unmade::NotFinite);
    }
    Ok(Some(laid))
}

/// The size an automatic size (`0 Tf`) comes to for `shown` in `client`.
///
/// Comb text in a font with a box of its own fills a cell by that box: the
/// smaller of the cell's width and the text box's height, each in the box's
/// units (`fpdfsdk/pwl/cpwl_edit.cpp`, `GetCharArrayAutoFontSize`). Any
/// other text takes the largest of [`AUTO_SIZES`] (the first
/// [`MULTILINE_SIZES`] for a multiline field) at which it fits, as PDFium's
/// binary search finds it (`CPVT_VariableText::GetAutoFontSize` and
/// `IsBigger`): the first size at which it's bigger than the text box, its
/// widest line wider or its lines' heights together taller by more than
/// `0.0001`, and the size before it; the first size where even that is
/// bigger, the last where none is.
fn auto_size<F: LayoutFont + ?Sized>(
    field: &TextField<'_>,
    shown: &Shown,
    client: Rect,
    style: &TextBoxStyle,
    advances: &[f32],
    font: &F,
) -> f32 {
    let metrics = font.font_metrics();
    if let (Some(cells), Some(b)) = (shown.cells, metrics.bbox) {
        let (box_w, box_h) = (b.x1 - b.x0, b.y1 - b.y0);
        let across = client.width() / cells as f32 * 1000.0 / box_w;
        let down = client.height() * 1000.0 / box_h;
        let size = across.min(down);
        if box_w > 0.0 && box_h > 0.0 && size.is_finite() && size > 0.0 {
            return size;
        }
    }

    let multiline = field.flags.contains(FieldFlags::MULTILINE);
    let sizes = match multiline {
        true => &AUTO_SIZES[..MULTILINE_SIZES],
        false => &AUTO_SIZES[..],
    };
    // The heights the layout sets lines by: the font's as given, or its
    // floors where they give no height (`font_heights_as_given`).
    let (ascent, descent) = {
        let (a, d) = (metrics.ascent / 1000.0, metrics.descent / 1000.0);
        match a.is_finite() && d.is_finite() && a > d {
            true => (metrics.ascent, metrics.descent),
            false => (a.max(0.70) * 1000.0, d.min(-0.15) * 1000.0),
        }
    };
    let line = |size: f32| ascent * size * 0.001 - descent * size * 0.001;
    let bigger = |size: f32| {
        let (wide, tall) = if multiline && shown.cells.is_none() {
            let trial = TextBoxStyle {
                size: Some(size),
                ..*style
            };
            match layout_text(font, &shown.text, client, &trial) {
                Ok(laid) => (
                    laid.lines.iter().map(|l| l.width).fold(0.0, f32::max),
                    line(size) * laid.lines.len() as f32,
                ),
                Err(_) => (0.0, 0.0),
            }
        } else {
            let wide = advances.iter().fold(0.0, |w, a| w + a * size * 0.001);
            (wide, line(size))
        };
        is_bigger(wide, client.width()) || is_bigger(tall, client.height())
    };

    // The first size at which the text is bigger, by a binary search over
    // the sizes as `std::lower_bound` makes it, so the same sizes are tried.
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

/// Whether `a` is bigger than `b` by more than `0.0001`, as PDFium compares
/// them (`core/fxcrt/fx_system.h`, `FXSYS_IsFloatBigger`).
fn is_bigger(a: f32, b: f32) -> bool {
    a > b && a - b >= 0.0001
}
