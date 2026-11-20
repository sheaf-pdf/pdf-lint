//! What every made appearance shares: the widget's box and what's drawn
//! around its content, the appearance made, and why one couldn't be.

use std::fmt;

use pdf_content_streams::{Color, ResourceNeeds};
use pdf_gfx::{BoxBorder, Canvas, Matrix, Rect, UnencodableChar};

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
    /// The widget's rotation (`/MK /R`), in degrees counterclockwise.
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
    /// The form XObject's `/BBox`: the box at the origin.
    pub bbox: Rect,
    /// The form XObject's `/Matrix`.
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
    /// color isn't finite, or the text laid out at that size isn't.
    #[error("a number of the widget's look or of its default appearance isn't finite")]
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

/// The box `look` gives, at the origin: [`Unmade::NoBox`] where it has no
/// width or no height, or isn't finite, and [`Unmade::NotFinite`] where its
/// border's width or a color isn't.
pub(crate) fn bbox(look: &WidgetLook<'_>) -> Result<Rect, Unmade> {
    let sized = |v: f32| v.is_finite() && v > 0.0;
    if !(sized(look.width) && sized(look.height)) {
        return Err(Unmade::NoBox);
    }
    if !(look.border.width.is_finite()
        && finite_color(look.background)
        && finite_color(look.border_color))
    {
        return Err(Unmade::NotFinite);
    }
    Ok(Rect::new(0.0, 0.0, look.width, look.height))
}

/// The box text is set in: `bbox` less the border's reach on every side
/// ([`BoxBorder::inset`]), turned the right way round where the border is
/// wider than half the box, as PDFium sets text in a box smaller than its
/// border (`fpdfsdk/cpdfsdk_widget.cpp`, `GetClientRect`).
pub(crate) fn client(look: &WidgetLook<'_>, bbox: Rect) -> Rect {
    let inset = look.border.inset();
    Rect::new(
        bbox.x0 + inset,
        bbox.y0 + inset,
        bbox.x1 - inset,
        bbox.y1 - inset,
    )
}

/// Draws what's under the content: the box filled in the background, then
/// the border in its color (`Canvas::box_border`), or with no color only a
/// beveled or inset border's bands (`Canvas::box_bands`), as PDFium draws
/// them with no `/BC` (`fpdfsdk/cpdfsdk_appstream.cpp`,
/// `GetBorderAppStream`).
pub(crate) fn frame(canvas: &mut Canvas, look: &WidgetLook<'_>, bbox: Rect) {
    if let Some(background) = look.background {
        canvas.with_state(|c| {
            c.fill_color(background);
            c.fill_rect(bbox);
        });
    }
    match look.border_color {
        Some(color) => canvas.box_border(bbox, &look.border, color, look.background),
        None => canvas.box_bands(bbox, &look.border, look.background),
    }
}
