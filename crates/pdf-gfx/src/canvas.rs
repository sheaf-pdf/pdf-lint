//! Ergonomic 2D vector, border, clip, XObject, and positioned-text canvas over
//! [`ContentWriter`].
//!
//! [`Canvas`] layers [`Point`], [`Rect`], [`Matrix`], [`Rotation`], [`Seg`],
//! scoped `q`/`Q` and marked-content helpers, inset borders in the five
//! border styles ([`BoxBorder`], [`BoxBorderStyle`]), rounded
//! rectangles, circles/ellipses, placed images/Form XObjects, and `TJ`-kerned
//! text on top of `pdf-content-streams`'s [`ContentWriter`] while preserving
//! collision-free deferred resource naming ([`Res`], [`Reserved`],
//! [`ResourceNeeds`]).

use pdf_content_streams::{
    Color, ContentWriter, FillRule, GState, LineCap, LineJoin, Paint, Res, Reserved, ResourceNeeds,
    TextMode, write_number, write_string,
};

use crate::geom::{Matrix, Point, Rect, Rotation};
use crate::path::Seg;

/// Cubic Bézier control-point factor `(4 / 3) * (sqrt(2) - 1)` approximating a
/// quarter-circle arc to within `0.027%` of the radius.
const KAPPA: f32 = 0.552_284_8;

/// One element of a `TJ` positioned text run passed to
/// [`Canvas::text_kerned`].
#[derive(Clone, Copy, PartialEq)]
pub enum TextPiece<'a> {
    /// Encoded character-code bytes in the active font.
    Codes(&'a [u8]),
    /// Position adjustment in thousandths of a unit of text space (positive
    /// values subtract from the pen position, moving glyphs left/closer in
    /// horizontal writing, per ISO 32000-2 §9.4.3).
    Adjust(f32),
}

impl std::fmt::Debug for TextPiece<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TextPiece::Codes(b) => f.debug_struct("Codes").field("len", &b.len()).finish(),
            TextPiece::Adjust(n) => f.debug_tuple("Adjust").field(n).finish(),
        }
    }
}

/// How [`Canvas::box_border`] draws a rectangle's border: the five styles a
/// border style dictionary can ask for (ISO 32000-2, §12.5.4, Table 168).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BoxBorderStyle {
    /// A solid frame.
    #[default]
    Solid,
    /// A dashed frame, by [`BoxBorder::dash`].
    Dashed,
    /// A frame with an embossed look: a light band along the top and left
    /// inside it, a dark one along the bottom and right.
    Beveled,
    /// A frame with an engraved look: a dark band along the top and left
    /// inside it, a lighter one along the bottom and right.
    Inset,
    /// One line along the bottom of the rectangle.
    Underline,
}

/// A rectangle's border for [`Canvas::box_border`]: its width, its style and,
/// for [`BoxBorderStyle::Dashed`], its dash array.
///
/// It borrows the dash array and is `Copy`, so a border read from a file is
/// drawn without cloning it.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct BoxBorder<'a> {
    /// The border's width in user units. Nothing is drawn unless it's finite
    /// and above `0.0`.
    pub width: f32,
    /// The border's style.
    pub style: BoxBorderStyle,
    /// The dash array of a [`BoxBorderStyle::Dashed`] border (dashes and gaps
    /// in user units, starting with a dash); the other styles ignore it.
    /// `[3.0]` is drawn where it's empty, holds a negative or non-finite
    /// number, or is all zeros (ISO 32000-2, §12.5.4, Table 168 and §8.4.3.6).
    pub dash: &'a [f32],
}

impl<'a> BoxBorder<'a> {
    /// A border of `width` in `style`, with no dash array.
    #[must_use]
    pub const fn new(width: f32, style: BoxBorderStyle) -> Self {
        Self {
            width,
            style,
            dash: &[],
        }
    }

    /// A solid border of `width`.
    #[must_use]
    pub const fn solid(width: f32) -> Self {
        Self::new(width, BoxBorderStyle::Solid)
    }

    /// Sets the dash array a [`BoxBorderStyle::Dashed`] border is drawn with.
    #[must_use]
    pub const fn with_dash(mut self, dash: &'a [f32]) -> Self {
        self.dash = dash;
        self
    }

    /// How far the border reaches into its rectangle from each edge, for a
    /// caller that pads what it draws inside: the width, or twice the width
    /// for [`BoxBorderStyle::Beveled`] and [`BoxBorderStyle::Inset`] (the
    /// frame, then the band inside it). `0.0` where the width isn't finite and
    /// above `0.0`, as nothing is drawn then.
    #[must_use]
    pub fn inset(&self) -> f32 {
        if !(self.width > 0.0 && self.width.is_finite()) {
            return 0.0;
        }
        match self.style {
            BoxBorderStyle::Beveled | BoxBorderStyle::Inset => 2.0 * self.width,
            BoxBorderStyle::Solid | BoxBorderStyle::Dashed | BoxBorderStyle::Underline => {
                self.width
            }
        }
    }
}

/// The dash array a dashed border falls back to (ISO 32000-2, §12.5.4,
/// Table 168).
const DEFAULT_BORDER_DASH: [f32; 1] = [3.0];

/// `color` at half its brightness, for a bevel's shaded side: a gray level or
/// RGB components halved; CMYK's cyan, magenta and yellow halved and its black
/// moved halfway to full, which halves each of the red, green and blue the
/// device formula gives (ISO 32000-2, §10.4.2.4).
fn darkened(color: Color) -> Color {
    match color {
        Color::Gray(g) => Color::Gray(g / 2.0),
        Color::Rgb(c) => Color::Rgb(c.map(|v| v / 2.0)),
        Color::Cmyk([c, m, y, k]) => Color::Cmyk([c / 2.0, m / 2.0, y / 2.0, (k + 1.0) / 2.0]),
    }
}

/// The shades a border's two bands are filled with, the top and left one's
/// and the bottom and right one's: for [`BoxBorderStyle::Beveled`], white
/// and `fill_hint` at half its brightness (a middle gray without a hint);
/// for every other style, [`BoxBorderStyle::Inset`]'s middle and light gray.
pub(crate) fn shades(style: BoxBorderStyle, fill_hint: Option<Color>) -> (Color, Color) {
    if style == BoxBorderStyle::Beveled {
        (
            Color::Gray(1.0),
            fill_hint.map_or(Color::Gray(0.5), darkened),
        )
    } else {
        (Color::Gray(0.5), Color::Gray(0.75))
    }
}

/// Whether a border of width `w` can be drawn in `rect`: `w` finite and above
/// `0.0`, `rect` finite, and `rect` grown by `5.0 * w` finite too.
fn border_drawable(rect: Rect, w: f32) -> bool {
    if !(w > 0.0 && w.is_finite() && rect.is_finite()) {
        return false;
    }
    let half = w / 2.0;
    // A reader bounds a stroke by its path's points and, past them, half
    // the width times the miter limit (`10` unless set): no further than
    // this from the box, for every style. Past `f32`'s range the stream
    // would be one its own interpreter can't read back.
    let reach = half * 10.0;
    rect.expand(reach).is_finite()
}

/// High-level content-stream builder wrapping [`ContentWriter`].
///
/// Coordinates are expressed in the active user space (PageSpace at the start
/// of a page stream before any [`Canvas::transform`]).
#[derive(Debug, Clone, Default)]
pub struct Canvas {
    writer: ContentWriter,
}

impl Canvas {
    /// Creates an empty canvas.
    #[must_use]
    pub fn new() -> Self {
        Self {
            writer: ContentWriter::new(),
        }
    }

    /// Wraps an existing [`ContentWriter`].
    #[must_use]
    pub fn from_writer(writer: ContentWriter) -> Self {
        Self { writer }
    }

    /// Borrows the underlying [`ContentWriter`].
    #[must_use]
    pub const fn writer(&self) -> &ContentWriter {
        &self.writer
    }

    /// Mutably borrows the underlying [`ContentWriter`].
    pub fn writer_mut(&mut self) -> &mut ContentWriter {
        &mut self.writer
    }

    /// Unwraps the underlying [`ContentWriter`].
    #[must_use]
    pub fn into_writer(self) -> ContentWriter {
        self.writer
    }

    /// Finalizes the content stream bytes and assigns collision-free resource
    /// names avoiding `reserved`.
    #[must_use]
    pub fn finish(self, reserved: &Reserved) -> (Vec<u8>, ResourceNeeds) {
        self.writer.finish(reserved)
    }

    /// Like [`Canvas::finish`], keeping `self` so the same drawing can be named
    /// against multiple pages' [`Reserved`] sets.
    #[must_use]
    pub fn named(&self, reserved: &Reserved) -> (Vec<u8>, ResourceNeeds) {
        self.writer.named(reserved)
    }

    /// Pushes the graphics state (`q`).
    pub fn save(&mut self) {
        self.writer.save();
    }

    /// Pops the graphics state (`Q`).
    pub fn restore(&mut self) {
        self.writer.restore();
    }

    /// Runs `f` inside a balanced `q` ... `Q` graphics-state scope.
    pub fn with_state(&mut self, f: impl FnOnce(&mut Self)) {
        self.writer.save();
        f(self);
        self.writer.restore();
    }

    /// Concatenates `matrix` onto the CTM (`cm`).
    pub fn transform(&mut self, matrix: Matrix) {
        self.writer.transform(matrix.to_array());
    }

    /// Sets graphics-state parameters (`gs`) from `state`.
    pub fn gstate(&mut self, state: GState) {
        self.writer.gstate(state);
    }

    /// Sets the nonstroking (fill) device color (`g`, `rg`, or `k`).
    pub fn fill_color(&mut self, color: Color) {
        self.writer.fill_color(color);
    }

    /// Sets the stroking device color (`G`, `RG`, or `K`).
    pub fn stroke_color(&mut self, color: Color) {
        self.writer.stroke_color(color);
    }

    /// Sets the stroke line width (`w`).
    pub fn line_width(&mut self, width: f32) {
        self.writer.line_width(width);
    }

    /// Sets the stroke line cap (`J`).
    pub fn line_cap(&mut self, cap: LineCap) {
        self.writer.line_cap(cap);
    }

    /// Sets the stroke line join (`j`).
    pub fn line_join(&mut self, join: LineJoin) {
        self.writer.line_join(join);
    }

    /// Sets the stroke miter limit (`M`).
    pub fn miter_limit(&mut self, limit: f32) {
        let mut buf = Vec::with_capacity(12);
        write_number(&mut buf, limit);
        self.writer.operation([buf.as_slice()], b"M");
    }

    /// Sets the stroke dash pattern (`d`).
    pub fn dash(&mut self, dashes: &[f32], phase: f32) {
        self.writer.dash(dashes, phase);
    }

    /// Intersects the active clipping path with `rect` (`re W n`).
    pub fn clip_rect(&mut self, rect: Rect) {
        self.writer
            .rect(rect.x0, rect.y0, rect.width(), rect.height());
        self.writer.clip(FillRule::NonZero);
        self.writer.paint(Paint::None);
    }

    /// Intersects the active clipping path with `segments` under `rule` (`W n`
    /// or `W* n`).
    pub fn clip_path(&mut self, segments: &[Seg], rule: FillRule) {
        self.emit_segments(segments);
        self.writer.clip(rule);
        self.writer.paint(Paint::None);
    }

    /// Fills `rect` (`re f`).
    pub fn fill_rect(&mut self, rect: Rect) {
        self.paint_rect(rect, Paint::Fill);
    }

    /// Strokes `rect` centered on its boundary (`re S`).
    pub fn stroke_rect(&mut self, rect: Rect) {
        self.paint_rect(rect, Paint::Stroke);
    }

    /// Fills and strokes `rect` (`re B`).
    pub fn fill_stroke_rect(&mut self, rect: Rect) {
        self.paint_rect(rect, Paint::FillStroke);
    }

    /// Paints `rect` with `paint`.
    pub fn paint_rect(&mut self, rect: Rect, paint: Paint) {
        self.writer
            .rect(rect.x0, rect.y0, rect.width(), rect.height());
        self.writer.paint(paint);
    }

    /// Draws an inset border of thickness `width` strictly inside `rect` (by
    /// insetting the stroked path by `width / 2.0` inside a `q`..`Q` scope).
    /// Does nothing if `width <= 0.0` or `rect` is narrower/shorter than
    /// `width`. The stroke takes the current color and dash;
    /// [`Canvas::box_border`] sets both and draws the other border styles.
    /// Nothing here keeps `rect` and `width` within what a stroke's extent
    /// can be bounded in, as [`Canvas::box_border`] does.
    pub fn border_rect(&mut self, rect: Rect, width: f32) {
        if !(width > 0.0 && width.is_finite()) {
            return;
        }
        let half = width / 2.0;
        let Some(inner) = rect.inset(half, half) else {
            return;
        };
        self.with_state(|c| {
            c.line_width(width);
            c.stroke_rect(inner);
        });
    }

    /// Draws `border` strictly inside `rect` in one of the five border styles
    /// (ISO 32000-2, §12.5.4, Table 168), `stroke` its color.
    ///
    /// Everything is drawn inside one `q`..`Q` scope with its colors, width
    /// and dash set there (a solid line for every style but `Dashed`,
    /// whatever dash the caller left set), so the caller's graphics state is
    /// as it was. The line cap, the line join and the miter limit are taken
    /// as they are: the defaults, unless the caller set others. Nothing is drawn
    /// where the border's width isn't finite and above `0.0`, or `rect` isn't
    /// finite, or `rect` grown by `5.0 * w` on every side isn't: numbers so
    /// near `f32`'s largest that a stroke's extent (as far as a miter at the
    /// default limit of `10` reaches) overflows, which
    /// [`interpret`](crate::interpret) refuses as
    /// [`Error::NotFinite`](crate::Error::NotFinite). What is written, read
    /// alone, interprets without a fault. With `w` the width:
    ///
    /// - [`BoxBorderStyle::Solid`]: the rectangle inset by `w / 2.0`, stroked
    ///   at `w` (what [`Canvas::border_rect`] draws). Nothing where `rect` is
    ///   narrower or shorter than `w`.
    /// - [`BoxBorderStyle::Dashed`]: the same under [`BoxBorder::dash`] at
    ///   phase `0`, or under `[3.0]` where that array can't be used.
    /// - [`BoxBorderStyle::Underline`]: one segment along the bottom, at
    ///   `rect.y0 + w / 2.0`, stroked at `w`. Nothing where `rect` is shorter
    ///   than `w` or has no width.
    /// - [`BoxBorderStyle::Beveled`] and [`BoxBorderStyle::Inset`]: the solid
    ///   frame, then two mitered bands of thickness `w` just inside it, filled:
    ///   the top and left one, then the bottom and right one. Beveled fills the
    ///   first white and the second with `fill_hint` at half its brightness
    ///   (a middle gray without a hint); Inset fills the first a middle gray
    ///   and the second a light gray. Nothing where `rect` is narrower or
    ///   shorter than `2.0 * w`; the frame alone where it's narrower or shorter
    ///   than `4.0 * w`. [`BoxBorder::inset`] says how far the whole reaches,
    ///   and [`Canvas::box_bands`] draws the bands alone.
    ///
    /// `fill_hint` is the color behind the border (a widget's background, a
    /// box's interior), used only by `Beveled`.
    pub fn box_border(
        &mut self,
        rect: Rect,
        border: &BoxBorder<'_>,
        stroke: Color,
        fill_hint: Option<Color>,
    ) {
        let w = border.width;
        if !border_drawable(rect, w) {
            return;
        }
        let half = w / 2.0;
        match border.style {
            BoxBorderStyle::Solid | BoxBorderStyle::Dashed => {
                let Some(frame) = rect.inset(half, half) else {
                    return;
                };
                self.with_state(|c| {
                    c.stroke_color(stroke);
                    c.line_width(w);
                    if border.style == BoxBorderStyle::Dashed {
                        let usable = border.dash.iter().all(|d| d.is_finite() && *d >= 0.0)
                            && border.dash.iter().any(|d| *d > 0.0);
                        let dash = if usable {
                            border.dash
                        } else {
                            &DEFAULT_BORDER_DASH
                        };
                        c.dash(dash, 0.0);
                    } else {
                        c.dash(&[], 0.0);
                    }
                    c.stroke_rect(frame);
                });
            }
            BoxBorderStyle::Underline => {
                if !(rect.x0 < rect.x1 && rect.y1 - rect.y0 >= w) {
                    return;
                }
                let y = rect.y0 + half;
                self.with_state(|c| {
                    c.stroke_color(stroke);
                    c.line_width(w);
                    c.dash(&[], 0.0);
                    c.line(Point::new(rect.x0, y), Point::new(rect.x1, y));
                });
            }
            BoxBorderStyle::Beveled | BoxBorderStyle::Inset => {
                let Some(frame) = rect.inset(half, half) else {
                    return;
                };
                // The bands' outer edge (the frame's inner one) and their
                // inner edge. No outer edge: the box is under `2 * w`, where
                // nothing is drawn. No inner edge: under `4 * w`, the frame
                // alone.
                let Some(outer) = rect.inset(w, w) else {
                    return;
                };
                let inner = rect.inset(2.0 * w, 2.0 * w);
                self.with_state(|c| {
                    c.stroke_color(stroke);
                    c.line_width(w);
                    c.dash(&[], 0.0);
                    c.stroke_rect(frame);
                    let Some(inner) = inner else {
                        return;
                    };
                    let (top_left, bottom_right) = shades(border.style, fill_hint);
                    c.bands(outer, inner, top_left, bottom_right);
                });
            }
        }
    }

    /// Draws the two bands of a [`BoxBorderStyle::Beveled`] or
    /// [`BoxBorderStyle::Inset`] `border` inside `rect` without its frame, in
    /// a `q`..`Q` scope of their own: what [`Canvas::box_border`] fills
    /// inside the frame it strokes, the same operators with the same
    /// numbers, for a caller that shades a box's edge where no color is
    /// given to stroke its frame. `fill_hint` is as there.
    ///
    /// Nothing is drawn for the other styles, nor where
    /// [`Canvas::box_border`] would draw no bands: a width or box it can't
    /// use, or a box narrower or shorter than four times the width. The
    /// fill color is set inside the scope, so the caller's graphics state is
    /// as it was. It's [`Canvas::box_bands_in`] in the style's shades.
    pub fn box_bands(&mut self, rect: Rect, border: &BoxBorder<'_>, fill_hint: Option<Color>) {
        let (top_left, bottom_right) = shades(border.style, fill_hint);
        self.box_bands_in(rect, border, top_left, bottom_right);
    }

    /// Draws the two bands of a [`BoxBorderStyle::Beveled`] or
    /// [`BoxBorderStyle::Inset`] `border` inside `rect` as
    /// [`Canvas::box_bands`] draws them, in shades the caller gives: the top
    /// and left band filled with `top_left`, the bottom and right one with
    /// `bottom_right`. For a box whose bands take other shades than its
    /// style's, such as a button's down appearance, whose two shades change
    /// places. Given the shades `box_bands` chooses, it writes what
    /// `box_bands` writes, byte for byte.
    ///
    /// Nothing is drawn where [`Canvas::box_bands`] draws nothing: another
    /// style, a width or box it can't use, or a box narrower or shorter than
    /// four times the width. The fill color is set inside a `q`..`Q` scope
    /// of the bands' own, so the caller's graphics state is as it was.
    pub fn box_bands_in(
        &mut self,
        rect: Rect,
        border: &BoxBorder<'_>,
        top_left: Color,
        bottom_right: Color,
    ) {
        let w = border.width;
        let banded = matches!(
            border.style,
            BoxBorderStyle::Beveled | BoxBorderStyle::Inset
        );
        if !(banded && border_drawable(rect, w)) {
            return;
        }
        let (Some(outer), Some(inner)) = (rect.inset(w, w), rect.inset(2.0 * w, 2.0 * w)) else {
            return;
        };
        self.with_state(|c| c.bands(outer, inner, top_left, bottom_right));
    }

    /// Fills a bevel's two mitered bands between `outer` and `inner`, with no
    /// scope of their own: the top and left one in `top_left`, then the
    /// bottom and right one in `bottom_right`.
    fn bands(&mut self, outer: Rect, inner: Rect, top_left: Color, bottom_right: Color) {
        self.fill_color(top_left);
        self.polyline(
            &[
                Point::new(outer.x0, outer.y0),
                Point::new(outer.x0, outer.y1),
                Point::new(outer.x1, outer.y1),
                Point::new(inner.x1, inner.y1),
                Point::new(inner.x0, inner.y1),
                Point::new(inner.x0, inner.y0),
            ],
            true,
            Paint::Fill,
        );
        self.fill_color(bottom_right);
        self.polyline(
            &[
                Point::new(outer.x0, outer.y0),
                Point::new(outer.x1, outer.y0),
                Point::new(outer.x1, outer.y1),
                Point::new(inner.x1, inner.y1),
                Point::new(inner.x1, inner.y0),
                Point::new(inner.x0, inner.y0),
            ],
            true,
            Paint::Fill,
        );
    }

    /// Strokes a single line segment from `from` to `to` (`m l S`).
    pub fn line(&mut self, from: Point, to: Point) {
        self.writer.move_to(from.x, from.y);
        self.writer.line_to(to.x, to.y);
        self.writer.paint(Paint::Stroke);
    }

    /// Draws a polyline or polygon through `points` with `paint`. Does nothing
    /// if `points` has fewer than 2 points.
    pub fn polyline(&mut self, points: &[Point], closed: bool, paint: Paint) {
        let Some((&first, rest)) = points.split_first() else {
            return;
        };
        if rest.is_empty() {
            return;
        }
        self.writer.move_to(first.x, first.y);
        for &p in rest {
            self.writer.line_to(p.x, p.y);
        }
        if closed {
            self.writer.close();
        }
        self.writer.paint(paint);
    }

    /// Draws a rectangle with circular corners of `radius` (clamped to half the
    /// shorter side) and paints it with `paint`.
    pub fn rounded_rect(&mut self, rect: Rect, radius: f32, paint: Paint) {
        let (w, h) = (rect.width(), rect.height());
        let r = radius.max(0.0).min(w / 2.0).min(h / 2.0);
        if r == 0.0 {
            self.paint_rect(rect, paint);
            return;
        }
        let k = r * KAPPA;
        let (x0, y0, x1, y1) = (rect.x0, rect.y0, rect.x1, rect.y1);
        self.writer.move_to(x0 + r, y0);
        self.writer.line_to(x1 - r, y0);
        self.writer
            .curve_to(x1 - r + k, y0, x1, y0 + r - k, x1, y0 + r);
        self.writer.line_to(x1, y1 - r);
        self.writer
            .curve_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
        self.writer.line_to(x0 + r, y1);
        self.writer
            .curve_to(x0 + r - k, y1, x0, y1 - r + k, x0, y1 - r);
        self.writer.line_to(x0, y0 + r);
        self.writer
            .curve_to(x0, y0 + r - k, x0 + r - k, y0, x0 + r, y0);
        self.writer.close();
        self.writer.paint(paint);
    }

    /// Draws an ellipse inscribed in `rect` using four cubic Bézier arcs and
    /// paints it with `paint`.
    pub fn ellipse(&mut self, rect: Rect, paint: Paint) {
        let rx = rect.width() / 2.0;
        let ry = rect.height() / 2.0;
        let cx = rect.x0 + rx;
        let cy = rect.y0 + ry;
        let kx = rx * KAPPA;
        let ky = ry * KAPPA;
        self.writer.move_to(cx + rx, cy);
        self.writer
            .curve_to(cx + rx, cy + ky, cx + kx, cy + ry, cx, cy + ry);
        self.writer
            .curve_to(cx - kx, cy + ry, cx - rx, cy + ky, cx - rx, cy);
        self.writer
            .curve_to(cx - rx, cy - ky, cx - kx, cy - ry, cx, cy - ry);
        self.writer
            .curve_to(cx + kx, cy - ry, cx + rx, cy - ky, cx + rx, cy);
        self.writer.close();
        self.writer.paint(paint);
    }

    /// Draws a circle centered at `center` with `radius` and paints it with
    /// `paint`.
    pub fn circle(&mut self, center: Point, radius: f32, paint: Paint) {
        let r = radius.max(0.0);
        let box_rect = Rect::new(center.x - r, center.y - r, center.x + r, center.y + r);
        self.ellipse(box_rect, paint);
    }

    /// Emits `segments` and ends the path with `paint`.
    pub fn draw_segments(&mut self, segments: &[Seg], paint: Paint) {
        if segments.is_empty() {
            return;
        }
        self.emit_segments(segments);
        self.writer.paint(paint);
    }

    fn emit_segments(&mut self, segments: &[Seg]) {
        for &seg in segments {
            match seg {
                Seg::Move(p) => self.writer.move_to(p.x, p.y),
                Seg::Line(p) => self.writer.line_to(p.x, p.y),
                Seg::Curve(a, b, p) => self.writer.curve_to(a.x, a.y, b.x, b.y, p.x, p.y),
                Seg::Close => self.writer.close(),
            }
        }
    }

    /// Draws `xobject` under `matrix` inside a `q`..`Q` scope (`q`, `cm`, `Do`,
    /// `Q`).
    pub fn draw_xobject(&mut self, xobject: Res, matrix: Matrix) {
        self.with_state(|c| {
            c.transform(matrix);
            c.writer.draw(xobject);
        });
    }

    /// Places an Image XObject (`Rect::UNIT`) onto `dst` rotated clockwise by
    /// `rotation` (`q`, `cm`, `Do`, `Q`). Returns `false` and writes nothing if
    /// `dst` has non-finite coordinates.
    pub fn place_image(&mut self, image: Res, dst: Rect, rotation: Rotation) -> bool {
        let Some(m) = Matrix::onto(Rect::UNIT, dst, rotation) else {
            return false;
        };
        self.draw_xobject(image, m);
        true
    }

    /// Places a Form XObject whose `/BBox` is `src_bbox` onto `dst` rotated
    /// clockwise by `rotation` (`q`, `cm`, `Do`, `Q`). Returns `false` and
    /// writes nothing if `src_bbox` is empty or either rectangle is non-finite.
    pub fn place_form(&mut self, form: Res, src_bbox: Rect, dst: Rect, rotation: Rotation) -> bool {
        let Some(m) = Matrix::onto(src_bbox, dst, rotation) else {
            return false;
        };
        self.draw_xobject(form, m);
        true
    }

    /// Shows `codes` in `font` at `size` with baseline origin at `origin` (`BT`,
    /// `Tf`, `Tm`, `Tj`, `ET`).
    pub fn text_at(&mut self, font: Res, size: f32, origin: Point, codes: &[u8]) {
        self.text_with_matrix(font, size, Matrix::translate(origin.x, origin.y), codes);
    }

    /// Shows `codes` in `font` at `size` under text matrix `matrix` (`BT`, `Tf`,
    /// `Tm`, `Tj`, `ET`).
    pub fn text_with_matrix(&mut self, font: Res, size: f32, matrix: Matrix, codes: &[u8]) {
        self.writer.begin_text();
        self.writer.font(font, size);
        self.writer.text_matrix(matrix.to_array());
        self.writer.show(codes);
        self.writer.end_text();
    }

    /// Shows positioned/kerned `pieces` in `font` at `size` under text matrix
    /// `matrix` (`BT`, `Tf`, `Tm`, `TJ`, `ET`), optionally setting `mode` when
    /// `mode != TextMode::Fill`.
    pub fn text_kerned(
        &mut self,
        font: Res,
        size: f32,
        matrix: Matrix,
        mode: TextMode,
        pieces: &[TextPiece<'_>],
    ) {
        self.writer.begin_text();
        self.writer.font(font, size);
        if mode != TextMode::Fill {
            self.writer.text_mode(mode);
        }
        self.writer.text_matrix(matrix.to_array());
        let mut arr = Vec::new();
        arr.push(b'[');
        for (i, piece) in pieces.iter().enumerate() {
            if i > 0 {
                arr.push(b' ');
            }
            match *piece {
                TextPiece::Codes(codes) => write_string(&mut arr, codes),
                TextPiece::Adjust(adj) => write_number(&mut arr, adj),
            }
        }
        arr.push(b']');
        self.writer.operation([arr.as_slice()], b"TJ");
        self.writer.end_text();
    }

    /// Emits a balanced `BMC` ... `EMC` marked-content sequence tagged `tag`.
    pub fn with_tagged(&mut self, tag: &str, f: impl FnOnce(&mut Self)) {
        self.writer.begin_tagged(tag);
        f(self);
        self.writer.end_marked();
    }

    /// Emits a balanced `BDC` ... `EMC` marked-content sequence tagged `tag`
    /// with property list resource `props`.
    pub fn with_marked(&mut self, tag: &str, props: Res, f: impl FnOnce(&mut Self)) {
        self.writer.begin_marked(tag, props);
        f(self);
        self.writer.end_marked();
    }

    #[cfg(feature = "text-layout")]
    fn set_word_space(&mut self, tw: f32) {
        let mut buf = Vec::with_capacity(12);
        write_number(&mut buf, tw);
        self.writer.operation([buf.as_slice()], b"Tw");
    }

    /// Emits a [`LaidOutText`](crate::LaidOutText) computed by
    /// [`layout_text`](crate::layout_text) in `font` as position-independent
    /// `Tm` + `Tj` (and `Tw` when justified) operations inside a single
    /// `BT`..`ET` text object.
    ///
    /// It's [`draw_layout_in`](Self::draw_layout_in) with `font` alone, for
    /// a layout in one font; a layout in several is drawn by that, given a
    /// resource for each face. Returns `false` and writes nothing as it
    /// does: for a layout made in one font and drawn as it was made, never.
    #[cfg(feature = "text-layout")]
    pub fn draw_layout(&mut self, font: Res, layout: &crate::wrap::LaidOutText) -> bool {
        self.draw_layout_in(std::slice::from_ref(&font), layout)
    }

    /// Emits a [`LaidOutText`](crate::LaidOutText) laid out in a
    /// [`FontSet`](crate::FontSet), each face under its resource in `fonts`
    /// (the first face under `fonts[0]`), as
    /// [`draw_layout`](Self::draw_layout) does in one font: inside a single
    /// `BT`..`ET` text object, a `Tf` first and again wherever the face
    /// changes, and a `Tm` and a `Tj` for each comb cell and each string of
    /// a line, placed where the layout puts it.
    ///
    /// A line all in one face is one string, its
    /// [`codes`](crate::LaidOutLine::codes) whole at its origin; a line in
    /// several is a string for each of its
    /// [`runs`](crate::LaidOutLine::runs), the bytes of `codes` the run
    /// names. So a caller that changes the `codes` of a line in several
    /// faces keeps its `runs` in step.
    ///
    /// A resource is named only where a cell or a string is drawn in it, so
    /// a layout whose text is the first face's names the first face alone,
    /// as [`draw_layout`](Self::draw_layout) writes it, byte for byte.
    ///
    /// Returns `false` and writes nothing where the layout can't be drawn
    /// whole: a face something is drawn in is past `fonts`, or a run names
    /// bytes its line's `codes` don't have. Nothing is drawn in part.
    #[cfg(feature = "text-layout")]
    pub fn draw_layout_in(&mut self, fonts: &[Res], layout: &crate::wrap::LaidOutText) -> bool {
        let font_of = |face: usize| fonts.get(face).cloned();
        if !whole(layout) || unnamed(layout, &font_of).is_some() {
            return false;
        }
        self.draw_faces(layout, font_of);
        true
    }

    /// Draws `layout`, each face under the resource `font_of` gives it: one
    /// [`whole`], and with a resource for every face drawn in ([`unnamed`]).
    #[cfg(feature = "text-layout")]
    fn draw_faces(
        &mut self,
        layout: &crate::wrap::LaidOutText,
        font_of: impl Fn(usize) -> Option<Res>,
    ) {
        // Written where a line has codes or cells, as one font's layout is.
        let first = (layout.lines.iter())
            .flat_map(strings)
            .find_map(|(face, _, _)| Some((face, font_of(face)?)));
        let Some((mut face_now, font)) = first else {
            return;
        };
        self.writer.begin_text();
        self.writer.font(font, layout.font_size);
        if layout.mode != TextMode::Fill {
            self.writer.text_mode(layout.mode);
        }
        let mut active_tw = 0.0f32;
        for line in &layout.lines {
            if !line.cells.is_empty() {
                if active_tw != 0.0 {
                    self.set_word_space(0.0);
                    active_tw = 0.0;
                }
            } else if line.codes.is_empty() {
                continue;
            } else if (line.word_space - active_tw).abs() > 1e-6 {
                self.set_word_space(line.word_space);
                active_tw = line.word_space;
            }
            for (face, at, codes) in strings(line) {
                if codes.is_empty() {
                    continue;
                }
                if face != face_now {
                    let Some(font) = font_of(face) else {
                        continue;
                    };
                    self.writer.font(font, layout.font_size);
                    face_now = face;
                }
                self.writer
                    .text_matrix(Matrix::translate(at.x, at.y).to_array());
                self.writer.show(codes);
            }
        }
        if active_tw != 0.0 {
            self.set_word_space(0.0);
        }
        self.writer.end_text();
    }

    /// Lays out `text` in `font` inside `rect` with
    /// [`layout_text`](crate::layout_text) and draws it onto `self` under
    /// `font_res` (clipping to `rect` inside `q`..`Q` when `style.clip` is
    /// `true`). Text in several fonts is
    /// [`text_box_in`](Self::text_box_in)'s.
    ///
    /// # Errors
    ///
    /// Returns [`UnencodableChar`](crate::UnencodableChar) without modifying
    /// `self` if any character in `text` cannot be encoded by `font`.
    #[cfg(feature = "text-layout")]
    pub fn text_box<F: crate::wrap::LayoutFont + ?Sized>(
        &mut self,
        font_res: Res,
        font: &F,
        text: &str,
        rect: Rect,
        style: &crate::wrap::TextBoxStyle,
    ) -> Result<crate::wrap::LaidOutText, crate::wrap::UnencodableChar> {
        let laid = crate::wrap::layout_text(font, text, rect, style)?;
        // One font's layout is in its one face.
        self.boxed(&laid, rect, style.clip, |_| Some(font_res.clone()));
        Ok(laid)
    }

    /// Lays out `text` in `font`, a [`FontSet`](crate::FontSet) of several
    /// fonts or one, inside `rect` and draws it onto `self` as
    /// [`text_box`](Self::text_box) does in one font: the set's first face
    /// under `font_res`, and every other under the resource the set gives it
    /// ([`FontSet::face_resource`](crate::FontSet::face_resource), as a
    /// [`WithFallback`](crate::WithFallback) does), as
    /// [`draw_layout_in`](Self::draw_layout_in) draws them. Text the first
    /// face draws whole is drawn as [`text_box`](Self::text_box) draws it in
    /// that font, byte for byte.
    ///
    /// # Errors
    ///
    /// Without modifying `self`:
    /// [`TextBoxError::Unencodable`](crate::TextBoxError::Unencodable) for a
    /// unit of `text` no face of `font` draws whole, as
    /// [`layout_text`](crate::layout_text) refuses it; and
    /// [`TextBoxError::NoResource`](crate::TextBoxError::NoResource) where a
    /// face past the first that some of the text is drawn in has no resource
    /// of the set's, with the face. Which text reaches such a face is the
    /// text's to say, so a set that gives a face none fails for some texts
    /// alone.
    #[cfg(feature = "text-layout")]
    pub fn text_box_in<F: crate::wrap::FontSet + ?Sized>(
        &mut self,
        font_res: Res,
        font: &F,
        text: &str,
        rect: Rect,
        style: &crate::wrap::TextBoxStyle,
    ) -> Result<crate::wrap::LaidOutText, crate::wrap::TextBoxError> {
        let laid = crate::wrap::layout_text(font, text, rect, style)?;
        let font_of = |face: usize| match face {
            0 => Some(font_res.clone()),
            face => font.face_resource(face),
        };
        if let Some(face) = unnamed(&laid, &font_of) {
            return Err(crate::wrap::TextBoxError::NoResource { face });
        }
        self.boxed(&laid, rect, style.clip, font_of);
        Ok(laid)
    }

    /// Draws `laid`, a layout in `rect` with a resource of `font_of` for
    /// every face drawn in, clipped to `rect` where `clip` says.
    #[cfg(feature = "text-layout")]
    fn boxed(
        &mut self,
        laid: &crate::wrap::LaidOutText,
        rect: Rect,
        clip: bool,
        font_of: impl Fn(usize) -> Option<Res>,
    ) {
        let norm = Rect::new(rect.x0, rect.y0, rect.x1, rect.y1);
        if clip {
            self.with_state(|c| {
                c.clip_rect(norm);
                c.draw_faces(laid, font_of);
            });
        } else {
            self.draw_faces(laid, font_of);
        }
    }
}

/// The first face `layout` draws a string in that `font_of` has no resource
/// for, if any.
#[cfg(feature = "text-layout")]
fn unnamed(
    layout: &crate::wrap::LaidOutText,
    font_of: &impl Fn(usize) -> Option<Res>,
) -> Option<usize> {
    // A face is asked once where its strings follow one another.
    let mut named = None;
    (layout.lines.iter())
        .flat_map(strings)
        .filter(|(_, _, codes)| !codes.is_empty())
        .find_map(|(face, _, _)| {
            if named == Some(face) {
                return None;
            }
            if font_of(face).is_none() {
                return Some(face);
            }
            named = Some(face);
            None
        })
}

/// Whether every run of `layout`'s lines in several faces names bytes its
/// line's codes have, so that each is drawn.
#[cfg(feature = "text-layout")]
fn whole(layout: &crate::wrap::LaidOutText) -> bool {
    layout.lines.iter().all(|line| {
        !line.cells.is_empty()
            || one_face(line).is_some()
            || (line.runs.iter()).all(|run| line.codes.get(run.codes.clone()).is_some())
    })
}

/// The face a line outside comb cells is all in, where it's in one: its
/// runs' face, or the first for a line with no run.
#[cfg(feature = "text-layout")]
fn one_face(line: &crate::wrap::LaidOutLine) -> Option<usize> {
    let face = line.runs.first().map_or(0, |run| run.face);
    (line.runs.iter().all(|run| run.face == face)).then_some(face)
}

/// Each string `line` draws, in order: its face, where it's shown, and its
/// codes; a comb line's cells, the codes whole of every other line in one
/// face, or the runs of a line in several.
#[cfg(feature = "text-layout")]
fn strings(line: &crate::wrap::LaidOutLine) -> impl Iterator<Item = (usize, Point, &[u8])> {
    let cells = line
        .cells
        .iter()
        .map(|cell| (cell.face, cell.origin, cell.codes.as_slice()));
    let one = one_face(line).filter(|_| line.cells.is_empty());
    let codes = one.map(|face| (face, line.origin, line.codes.as_slice()));
    let runs = (line.runs.iter())
        .take_while(move |_| line.cells.is_empty() && one.is_none())
        .filter_map(|run| {
            let codes = line.codes.get(run.codes.clone())?;
            // The first run, at the line's origin, is placed there as it is.
            let x = if run.x == 0.0 {
                line.origin.x
            } else {
                line.origin.x + run.x
            };
            Some((run.face, Point::new(x, line.origin.y), codes))
        });
    cells.chain(codes).chain(runs)
}
