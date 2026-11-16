//! Ergonomic 2D vector, border, clip, XObject, and positioned-text canvas over
//! [`ContentWriter`].
//!
//! [`Canvas`] layers [`Point`], [`Rect`], [`Matrix`], [`Rotation`], [`Seg`],
//! scoped `q`/`Q` and marked-content helpers, inset borders, rounded
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
    /// `width`.
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
}
