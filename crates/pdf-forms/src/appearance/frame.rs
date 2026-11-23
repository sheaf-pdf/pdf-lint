//! What every made appearance shares: the widget's box and what's drawn
//! around its content, the appearance made, and why one couldn't be.

use std::fmt;

use pdf_content_streams::{Color, LineCap, Paint, ResourceNeeds};
use pdf_gfx::{BoxBorder, BoxBorderStyle, Canvas, Matrix, Point, Rect, Seg, UnencodableChar};
use pdf_objects::Unreadable;

use crate::error::Error;
use crate::limits::Cap;

/// A widget's box and what's drawn around its content, as plain data: the
/// box's size, its background and border colors and its rotation (`/MK`,
/// ISO 32000-2, §12.5.6.19), and its border (§12.5.4).
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
    /// none for no fill. A component outside `0.0..=1.0` is drawn as the
    /// nearer end, in this color and the border's.
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
///
/// The maker of plain data ([`text`](super::text)) gives the first four,
/// and [`FontNotBindable`](Self::FontNotBindable) for a font of a set it's
/// given with no resource; [`NotText`](Self::NotText),
/// [`FontNotBindable`](Self::FontNotBindable) for a field's font, and
/// [`Unreadable`](Self::Unreadable) come from reading a widget for it
/// ([`widget_text`](super::widget_text),
/// [`text_for_widget`](super::text_for_widget)). An [`Error`] of the read
/// layer converts into it, a cap to [`Limit`](Self::Limit).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Unmade {
    /// No font has the whole of a grapheme cluster of the text, as given or
    /// composed: the first of its characters as the caller gave it that the
    /// first font can't draw, and its byte offset in the text the caller
    /// gave. A password field's characters are drawn as `*`, so it's never
    /// one of them, only `*` at offset `0` where no font has that.
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
    /// appearance with that object's resource. Or a font of a set past the
    /// first, which some of the text is drawn in, has no resource
    /// (`pdf_gfx::FontSet::face_resource`).
    #[error("the field's font can't be named by the appearance's resources")]
    FontNotBindable,
    /// Reading the widget, its value, or its font failed for a caller-level
    /// reason (locked encryption, a reader cap, a detached handle, or I/O),
    /// as [`Error::Unreadable`] has it.
    #[error("{0}")]
    Unreadable(Unreadable),
    /// The field isn't the kind of button a button's reader makes: a check
    /// box or a radio button for [`widget_toggle`](super::widget_toggle)
    /// and [`toggle_for_widget`](super::toggle_for_widget), a push button
    /// for [`widget_push`](super::widget_push) and
    /// [`push_for_widget`](super::push_for_widget).
    #[error("the field isn't a button of the kind asked for")]
    NotButton,
    /// The push button shows an icon (`/MK /I`), which isn't drawn, and its
    /// caption isn't shown alone (`/MK /TP` other than `0`): made without
    /// its icon, it would show nothing, or a caption where the icon's place
    /// was to be. From [`widget_push`](super::widget_push) and
    /// [`push_for_widget`](super::push_for_widget).
    #[error("the push button shows an icon, which isn't drawn")]
    Icon,
}

impl From<Error> for Unmade {
    fn from(err: Error) -> Self {
        match err {
            Error::Limit(cap) => Self::Limit(cap),
            Error::Unreadable(unreadable) => Self::Unreadable(unreadable),
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

/// `color` with each component within `0.0..=1.0`, the nearer end for one
/// outside it, as a default appearance string's color is read.
pub(crate) fn clamped(color: Color) -> Color {
    match color {
        Color::Gray(g) => Color::Gray(g.clamp(0.0, 1.0)),
        Color::Rgb(c) => Color::Rgb(c.map(|v| v.clamp(0.0, 1.0))),
        Color::Cmyk(c) => Color::Cmyk(c.map(|v| v.clamp(0.0, 1.0))),
    }
}

/// `look` with its colors' components within `0.0..=1.0` ([`clamped`]).
pub(crate) fn in_range<'a>(look: &WidgetLook<'a>) -> WidgetLook<'a> {
    WidgetLook {
        background: look.background.map(clamped),
        border_color: look.border_color.map(clamped),
        ..*look
    }
}

/// The quarter turns counterclockwise a widget's rotation (`/MK /R`) comes
/// to: `90`, `180`, or `270` degrees, or any of those and a whole turn
/// either way, is one, two, or three; any other number is none. A negative
/// rotation is clockwise, so `-90` is three, as ISO 32000-2, §12.5.6.19 has
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
    edge(canvas, look, bbox);
}

/// A frame's third part: the border in its color, where it has one, as
/// [`frame`] draws it.
fn edge(canvas: &mut Canvas, look: &WidgetLook<'_>, bbox: Rect) {
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
/// (ISO 32000-2, §12.5.4 and §8.4.3.6), as `Canvas::box_border` takes it.
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

/// The background a button's down appearance (`/AP /D`) is filled with, as
/// PDFium as of chromium/8152 fills it (`fpdfsdk/cpdfsdk_appstream.cpp`,
/// `SetAsPushButton`, `SetAsCheckBox`): `background`, its components in
/// `0.0..=1.0`, each less `0.25` in its own color space and none below `0`;
/// or, where there is none, PDFium's transparent background less `0.25`,
/// `0.75 0.75 0.75 rg`.
pub(crate) fn pressed(background: Option<Color>) -> Color {
    let less = |v: f32| (v - 0.25).max(0.0);
    match background {
        None => Color::Rgb([0.75; 3]),
        Some(Color::Gray(g)) => Color::Gray(less(g)),
        Some(Color::Rgb(c)) => Color::Rgb(c.map(less)),
        Some(Color::Cmyk(c)) => Color::Cmyk(c.map(less)),
    }
}

/// `color` at half its brightness, as `Canvas::box_bands` shades a beveled
/// border's dark band: a gray level or RGB components halved, CMYK's cyan,
/// magenta, and yellow halved and its black moved halfway to full.
fn half_bright(color: Color) -> Color {
    match color {
        Color::Gray(g) => Color::Gray(g / 2.0),
        Color::Rgb(c) => Color::Rgb(c.map(|v| v / 2.0)),
        Color::Cmyk([c, m, y, k]) => Color::Cmyk([c / 2.0, m / 2.0, y / 2.0, (k + 1.0) / 2.0]),
    }
}

/// Draws what's under a button's down appearance (`/AP /D`), as PDFium as
/// of chromium/8152 draws it: [`frame`]'s three parts, the box filled in the
/// background less `0.25` ([`pressed`]), a beveled border's two bands with
/// their shades exchanged (the background at half its brightness above and
/// to the left, white below and to the right) and an inset border's black
/// and white (`Canvas::box_bands_in`), and the border as `frame` draws it.
pub(crate) fn down_frame(canvas: &mut Canvas, look: &WidgetLook<'_>, bbox: Rect) {
    canvas.with_state(|c| {
        c.fill_color(pressed(look.background));
        c.fill_rect(bbox);
    });
    let behind = look.background.unwrap_or(Color::Rgb([1.0; 3]));
    let (top_left, bottom_right) = match look.border.style {
        BoxBorderStyle::Beveled => (half_bright(behind), Color::Gray(1.0)),
        _ => (Color::Gray(0.0), Color::Gray(1.0)),
    };
    canvas.box_bands_in(bbox, &look.border, top_left, bottom_right);
    edge(canvas, look, bbox);
}

/// The control points' share of a quarter circle's radius that draws it as
/// one cubic Bézier curve: 4/3 (√2 − 1).
const BEZIER: f32 = 0.552_284_8;

/// A circle in `rect`'s box (an ellipse where it isn't square) as four
/// Bézier curves, from the middle of its left side up and round, as PDFium
/// as of chromium/8152 writes one (`fpdfsdk/cpdfsdk_appstream.cpp`,
/// `GetAP_Circle`), so the curves can be held to its own.
pub(crate) fn circle(rect: Rect) -> [Seg; 5] {
    let (rx, ry) = (rect.width() / 2.0, rect.height() / 2.0);
    let (cx, cy) = (rect.x0 + rx, rect.y0 + ry);
    let (kx, ky) = (rx * BEZIER, ry * BEZIER);
    let p = Point::new;
    [
        Seg::Move(p(rect.x0, cy)),
        Seg::Curve(p(rect.x0, cy + ky), p(cx - kx, rect.y1), p(cx, rect.y1)),
        Seg::Curve(p(cx + kx, rect.y1), p(rect.x1, cy + ky), p(rect.x1, cy)),
        Seg::Curve(p(rect.x1, cy - ky), p(cx + kx, rect.y0), p(cx, rect.y0)),
        Seg::Curve(p(cx - kx, rect.y0), p(rect.x0, cy - ky), p(rect.x0, cy)),
    ]
}

/// Half of the circle in `rect`'s box, from the end of a diameter turned
/// `turn` radians counterclockwise from the horizontal round to the other
/// end, as two Bézier curves: at a quarter turn's half (π/4) the upper left
/// half, at five times that the lower right. The points are placed where
/// PDFium as of chromium/8152 places its own under the turn it writes
/// (`GetAP_HalfCircle`).
fn half_circle(rect: Rect, turn: f32) -> [Seg; 3] {
    let (rx, ry) = (rect.width() / 2.0, rect.height() / 2.0);
    let (cx, cy) = (rect.x0 + rx, rect.y0 + ry);
    let (sin, cos) = turn.sin_cos();
    let at = |x: f32, y: f32| Point::new(x * cos - y * sin + cx, x * sin + y * cos + cy);
    let (kx, ky) = (rx * BEZIER, ry * BEZIER);
    [
        Seg::Move(at(-rx, 0.0)),
        Seg::Curve(at(-rx, ky), at(-kx, ry), at(0.0, ry)),
        Seg::Curve(at(kx, ry), at(rx, ky), at(rx, 0.0)),
    ]
}

/// Draws a radio button's round frame, under the content of a button whose
/// mark is a circle, as PDFium as of chromium/8152 draws it
/// (`fpdfsdk/cpdfsdk_appstream.cpp`, `SetAsRadioButton` and
/// `GetCircleBorderAppStream`), in the box's centre square less `1` on each
/// side, `down` for the down appearance (`/AP /D`):
///
/// 1. The circle filled in the background, where there is one; for the
///    down appearance, in the background less `0.25` ([`pressed`]), but
///    for a beveled border in the background itself.
/// 2. A solid, dashed, or underline border, where it has a color: the
///    circle half the width inside, stroked at the width, a dashed one
///    under its dash array.
/// 3. A beveled or inset border, its width doubled as for a square frame:
///    the circle stroked at half that in the border's color, where it has
///    one; then two half circles three quarters of that half inside,
///    stroked at it, the upper left one in white and the lower right in the
///    background less `0.25` for beveled, a middle and a light gray for
///    inset; for the down appearance, the background less `0.25` and white
///    for beveled, black and white for inset.
///
/// Nothing of the border is drawn where its width isn't finite and above
/// `0`, or a stroke of it about the box couldn't be bounded.
pub(crate) fn round_frame(canvas: &mut Canvas, look: &WidgetLook<'_>, bbox: Rect, down: bool) {
    let side = bbox.width().min(bbox.height());
    let (cx, cy) = ((bbox.x0 + bbox.x1) / 2.0, (bbox.y0 + bbox.y1) / 2.0);
    let half = side / 2.0 - 1.0;
    let centre = Rect::new(cx - half, cy - half, cx + half, cy + half);
    let border = &look.border;
    let beveled = border.style == BoxBorderStyle::Beveled;
    let fill = match (down, beveled) {
        (false, _) | (true, true) => look.background,
        (true, false) => Some(pressed(look.background)),
    };
    if let Some(fill) = fill {
        canvas.with_state(|c| {
            c.fill_color(fill);
            c.draw_segments(&circle(centre), Paint::Fill);
        });
    }
    let width = border.inset();
    if !strokable(centre, width) {
        return;
    }
    let stroke = |canvas: &mut Canvas, color: Color, w: f32, dashes: &[f32], path: &[Seg]| {
        canvas.with_state(|c| {
            c.line_width(w);
            if !dashes.is_empty() {
                c.dash(dashes, 0.0);
            }
            c.stroke_color(color);
            c.draw_segments(path, Paint::Stroke);
        });
    };
    match border.style {
        BoxBorderStyle::Beveled | BoxBorderStyle::Inset => {
            let w = width / 2.0;
            if let Some(color) = look.border_color {
                stroke(canvas, color, w, &[], &circle(centre));
            }
            let (top_left, bottom_right) = match (beveled, down) {
                (true, false) => (Color::Gray(1.0), pressed(look.background)),
                (true, true) => (pressed(look.background), Color::Gray(1.0)),
                (false, false) => (Color::Gray(0.5), Color::Gray(0.75)),
                (false, true) => (Color::Gray(0.0), Color::Gray(1.0)),
            };
            let reach = w * 0.75;
            let inside = Rect::new(
                centre.x0 + reach,
                centre.y0 + reach,
                centre.x1 - reach,
                centre.y1 - reach,
            );
            let quarter = std::f32::consts::FRAC_PI_4;
            stroke(canvas, top_left, w, &[], &half_circle(inside, quarter));
            stroke(
                canvas,
                bottom_right,
                w,
                &[],
                &half_circle(inside, 5.0 * quarter),
            );
        }
        _ => {
            let Some(color) = look.border_color else {
                return;
            };
            let dashes = match border.style {
                BoxBorderStyle::Dashed => dash(border),
                _ => &[],
            };
            let inside = Rect::new(
                centre.x0 + width / 2.0,
                centre.y0 + width / 2.0,
                centre.x1 - width / 2.0,
                centre.y1 - width / 2.0,
            );
            stroke(canvas, color, width, dashes, &circle(inside));
        }
    }
}
