//! What every made appearance shares: the widget's box and what's drawn
//! around its content, the appearance made, and why one couldn't be.

use std::fmt;

use pdf_content_streams::{Color, LineCap, Paint, ResourceNeeds};
use pdf_gfx::{BoxBorder, BoxBorderStyle, Canvas, Matrix, Point, Rect, UnencodableChar};

use crate::error::Error;
use crate::limits::Cap;

/// A widget's box and what's drawn around its content, as plain data: the
/// box's size, its background and border colors and its rotation (`/MK`,
/// ISO 32000-2, 12.5.6.19), and its border (12.5.4).
///
/// It may gain fields: build it with [`new`](Self::new) and set them.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct WidgetLook<'a> {
    /// The box's width: its `/Rect`'s.
    pub width: f32,
    /// The box's height: its `/Rect`'s.
    pub height: f32,
    /// The color the box is filled with under everything else (`/MK /BG`);
    /// none for no fill.
    pub background: Option<Color>,
    /// The color the border is drawn in (`/MK /BC`); none for no border but
    /// the two shaded bands of a beveled or inset one, which have colors of
    /// their own.
    pub border_color: Option<Color>,
    /// The widget's rotation (`/MK /R`), in degrees counterclockwise: a
    /// multiple of 90 turns the appearance's box, a negative one clockwise;
    /// any other number is none.
    pub rotation: i32,
    /// The border: its width, style, and dash array. Whether or not it's
    /// drawn, the text is set inside it, by its width, or twice its width
    /// for a beveled or inset border.
    pub border: BoxBorder<'a>,
}

impl WidgetLook<'static> {
    /// A box of `width` by `height` with no colors, no rotation, and a solid
    /// border of width `1`, as a widget with no border style (`/BS`) and no
    /// `/Border` has.
    #[must_use]
    pub const fn new(width: f32, height: f32) -> WidgetLook<'static> {
        WidgetLook {
            width,
            height,
            background: None,
            border_color: None,
            rotation: 0,
            border: BoxBorder::solid(1.0),
        }
    }
}

/// An appearance made for a widget: a form XObject's box (`/BBox`),
/// matrix (`/Matrix`), content, and the resources its content names.
///
/// The content is uncompressed, and names its resources by
/// `pdf-content-streams`' names for them ([`needs`](Self::needs)): the
/// caller binds those in the form XObject's `/Resources`.
///
/// `Debug` prints lengths and numbers, never the content.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub struct Appearance {
    /// The form XObject's `/BBox`: the box at the origin, its width and
    /// height swapped where the widget's rotation is a quarter turn or
    /// three.
    pub bbox: Rect,
    /// The form XObject's `/Matrix`: the turn that places the box on the
    /// widget's rectangle, or the identity.
    pub matrix: Matrix,
    /// The content stream, uncompressed.
    pub content: Vec<u8>,
    /// What the content draws with, each under the name it gives it: the
    /// font, where any text is drawn. A resource is the font's object or its
    /// dictionary, never an object number of the caller's to come.
    pub needs: ResourceNeeds,
    /// The size the text is drawn at: the default appearance's, or the one
    /// chosen for an automatic size (`0 Tf`). `0` where the text box has no
    /// width, so nothing is drawn.
    pub font_size: f32,
    /// Whether the text didn't fit: it passes the box inside the border at
    /// the size drawn, or a comb field's text has more characters than it
    /// has cells.
    pub overflowed: bool,
}

impl fmt::Debug for Appearance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Appearance")
            .field("bbox", &self.bbox)
            .field("matrix", &self.matrix)
            .field("content_len", &self.content.len())
            .field("needs", &self.needs.0.len())
            .field("font_size", &self.font_size)
            .field("overflowed", &self.overflowed)
            .finish()
    }
}

/// Why an appearance couldn't be made. Nothing is drawn: no `?` stands in
/// for a character, and no box is guessed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Unmade {
    /// The font has no code for a character of the text: the character and
    /// its byte offset in the text the caller gave. A password field's
    /// characters are drawn as `*`, so it's never one of them, only `*` at
    /// offset `0` where the font has none for that.
    #[error("the font has no code for {:?}, at byte {} of the text", .0.character, .0.at)]
    Unencodable(UnencodableChar),
    /// The widget's box has no width or no height, or isn't finite.
    #[error("the widget's box has no width or no height")]
    NoBox,
    /// A number of the widget's look or of its default appearance's size or
    /// color isn't finite, or the box inside a border so much wider than it
    /// isn't, or the text laid out at that size isn't, or it reaches so far
    /// from the box's origin that a reader working in `f32` may not place it.
    #[error(
        "a number of the widget's look or of its default appearance isn't finite, \
         or sets the text too far out to be placed"
    )]
    NotFinite,
    /// One of [`Limits`](crate::Limits)'s caps was exceeded.
    #[error("exceeded cap on {0}")]
    Limit(Cap),
    /// The field isn't a text field.
    #[error("the field isn't a text field")]
    NotText,
    /// The field's font is a direct dictionary of the default resources
    /// that holds a reference, which an appearance's resources can't name as
    /// it is: a caller that writes it as an object of its own makes the
    /// appearance with that object's resource.
    #[error("the field's font can't be named by the appearance's resources")]
    FontNotBindable,
    /// Reading the widget, its value, or its font failed for a caller-level
    /// reason (locked encryption, a reader cap, a detached handle, or I/O).
    #[error("{0}")]
    Unreadable(Error),
}

impl From<Error> for Unmade {
    fn from(err: Error) -> Self {
        match err {
            Error::Limit(cap) => Self::Limit(cap),
            err @ Error::Unreadable(_) => Self::Unreadable(err),
        }
    }
}

/// Whether every component of `color` is finite.
pub(crate) fn finite_color(color: Option<Color>) -> bool {
    match color {
        None => true,
        Some(Color::Gray(g)) => g.is_finite(),
        Some(Color::Rgb(c)) => c.iter().all(|v| v.is_finite()),
        Some(Color::Cmyk(c)) => c.iter().all(|v| v.is_finite()),
    }
}

/// The quarter turns counterclockwise a widget's rotation (`/MK /R`) comes
/// to: `90`, `180`, or `270` degrees, or any of those and a whole turn
/// either way, is one, two, or three; any other number is none. A negative
/// rotation is clockwise, so `-90` is three, as ISO 32000-2, 12.5.6.19 has
/// it (PDFium reads it as one).
pub(crate) const fn quarter_turns(rotation: i32) -> u8 {
    match rotation.rem_euclid(360) {
        90 => 1,
        180 => 2,
        270 => 3,
        _ => 0,
    }
}

/// The appearance's box and matrix for `look`: the box at the origin,
/// turned by the widget's rotation, so its width and height swap at a
/// quarter turn and three, and the matrix that turns it back onto the
/// widget's rectangle (as PDFium places it, `fpdfsdk/cpdfsdk_widget.cpp`,
/// `GetRotatedRect` and `GetMatrix`). [`Unmade::NoBox`] where the box has no
/// width or no height, or isn't finite, and [`Unmade::NotFinite`] where its
/// border's width or a color isn't.
pub(crate) fn placed(look: &WidgetLook<'_>) -> Result<(Rect, Matrix), Unmade> {
    let sized = |v: f32| v.is_finite() && v > 0.0;
    let (w, h) = (look.width, look.height);
    if !(sized(w) && sized(h)) {
        return Err(Unmade::NoBox);
    }
    if !(look.border.width.is_finite()
        && finite_color(look.background)
        && finite_color(look.border_color))
    {
        return Err(Unmade::NotFinite);
    }
    Ok(match quarter_turns(look.rotation) {
        1 => (
            Rect::new(0.0, 0.0, h, w),
            Matrix::new(0.0, 1.0, -1.0, 0.0, w, 0.0),
        ),
        2 => (
            Rect::new(0.0, 0.0, w, h),
            Matrix::new(-1.0, 0.0, 0.0, -1.0, w, h),
        ),
        3 => (
            Rect::new(0.0, 0.0, h, w),
            Matrix::new(0.0, -1.0, 1.0, 0.0, 0.0, h),
        ),
        _ => (Rect::new(0.0, 0.0, w, h), Matrix::IDENTITY),
    })
}

/// The box text is set in: `bbox` less the border's reach on every side
/// ([`BoxBorder::inset`]), turned the right way round where the border is
/// wider than half the box, as PDFium sets text in a box smaller than its
/// border (`fpdfsdk/cpdfsdk_widget.cpp`, `GetClientRect`).
/// [`Unmade::NotFinite`] where a border so much wider than its box leaves
/// it a width or height past finite, which no content could write.
pub(crate) fn client(look: &WidgetLook<'_>, bbox: Rect) -> Result<Rect, Unmade> {
    let inset = look.border.inset();
    let client = Rect::new(
        bbox.x0 + inset,
        bbox.y0 + inset,
        bbox.x1 - inset,
        bbox.y1 - inset,
    );
    match client.width().is_finite() && client.height().is_finite() {
        true => Ok(client),
        false => Err(Unmade::NotFinite),
    }
}

/// Draws what's under the content, in the order PDFium draws it
/// (`fpdfsdk/cpdfsdk_appstream.cpp`, `GetBorderAppStream`):
///
/// 1. The box filled in the background.
/// 2. A beveled or inset border's two bands (`Canvas::box_bands`), with a
///    border color or without one; a beveled border's dark band is the
///    background at half its brightness, or with none, white's (a middle
///    gray in RGB).
/// 3. The border in its color, where it has one: a solid frame for a solid,
///    beveled, or inset border (`Canvas::box_border`); a dashed one as
///    [`dashed`] draws it; an underline's line.
pub(crate) fn frame(canvas: &mut Canvas, look: &WidgetLook<'_>, bbox: Rect) {
    if let Some(background) = look.background {
        canvas.with_state(|c| {
            c.fill_color(background);
            c.fill_rect(bbox);
        });
    }
    let behind = look.background.unwrap_or(Color::Rgb([1.0; 3]));
    canvas.box_bands(bbox, &look.border, Some(behind));
    let Some(color) = look.border_color else {
        return;
    };
    let border = &look.border;
    match border.style {
        BoxBorderStyle::Dashed => dashed(canvas, bbox, border, color),
        BoxBorderStyle::Underline => canvas.box_border(bbox, border, color, None),
        _ => canvas.box_border(bbox, &BoxBorder::solid(border.width), color, None),
    }
}

/// Whether a line of width `w` can be stroked about `rect`: `w` finite and
/// above `0`, and `rect` grown by as far as a stroke's miter reaches at the
/// default limit finite, as `Canvas::box_border` keeps its borders.
fn strokable(rect: Rect, w: f32) -> bool {
    w > 0.0 && w.is_finite() && rect.expand(w / 2.0 * 10.0).is_finite()
}

/// The dash array a dashed border is drawn with: its own, or `[3]` where
/// that's empty, holds a negative or non-finite number, or is all zeros
/// (ISO 32000-2, 12.5.4 and 8.4.3.6), as `Canvas::box_border` takes it.
fn dash<'a>(border: &BoxBorder<'a>) -> &'a [f32] {
    let usable = border.dash.iter().all(|d| d.is_finite() && *d >= 0.0)
        && border.dash.iter().any(|d| *d > 0.0);
    if usable { border.dash } else { &[3.0] }
}

/// A dashed border as PDFium draws it, so its dashes fall where PDFium's do:
/// one open path half its width inside `rect`, from the bottom left corner
/// up, along the top, down, and back along the bottom to where it began,
/// stroked under its own dash array from phase `0`. Nothing where `rect` is
/// narrower or shorter than the width.
fn dashed(canvas: &mut Canvas, rect: Rect, border: &BoxBorder<'_>, color: Color) {
    let w = border.width;
    if !strokable(rect, w) {
        return;
    }
    let Some(r) = rect.inset(w / 2.0, w / 2.0) else {
        return;
    };
    canvas.with_state(|c| {
        c.stroke_color(color);
        c.line_width(w);
        c.dash(dash(border), 0.0);
        c.polyline(
            &[
                Point::new(r.x0, r.y0),
                Point::new(r.x0, r.y1),
                Point::new(r.x1, r.y1),
                Point::new(r.x1, r.y0),
                Point::new(r.x0, r.y0),
            ],
            false,
            Paint::Stroke,
        );
    });
}

/// A comb field's dividers, as PDFium draws them where its border is solid
/// or dashed and has a color (`fpdfsdk/cpdfsdk_appstream.cpp`,
/// `SetAsTextField`): between each two of `cells` equal cells across
/// `client`, the box less the border, a line from its bottom to its top in
/// the border's color and width; a solid border's with projecting square
/// caps, a dashed one's under its dash array. Nothing for the other styles.
pub(crate) fn dividers(canvas: &mut Canvas, look: &WidgetLook<'_>, client: Rect, cells: u32) {
    let border = &look.border;
    let dashed = match border.style {
        BoxBorderStyle::Solid => false,
        BoxBorderStyle::Dashed => true,
        _ => return,
    };
    let Some(color) = look.border_color else {
        return;
    };
    if cells < 2 || !strokable(client, border.width) {
        return;
    }
    let step = client.width() / cells as f32;
    canvas.with_state(|c| {
        c.line_width(border.width);
        c.stroke_color(color);
        match dashed {
            true => c.dash(dash(border), 0.0),
            false => c.line_cap(LineCap::Square),
        }
        for i in 1..cells {
            let x = client.x0 + step * i as f32;
            c.line(Point::new(x, client.y0), Point::new(x, client.y1));
        }
    });
}
