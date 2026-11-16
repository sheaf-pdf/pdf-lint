//! 2D PDF geometry primitives and page coordinate-space transforms.
//!
//! Coordinates in PDF content streams and page dictionaries are expressed in
//! default user space ("PageSpace": points of 1/72 inch by default, `x` right,
//! `y` up, before `/Rotate`), while viewer coordinates ("ViewSpace") place the
//! origin `(0, 0)` at the visible top-left corner (`x` right, `y` down, after
//! `/Rotate`). [`PageGeom`] bridges both spaces without trigonometric rounding
//! drift.

use pdf_objects::{Dict, Object, Resolve};
use pdf_semantics::pages;

/// A 2D point in PDF coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    /// Horizontal coordinate (`x`).
    pub x: f32,
    /// Vertical coordinate (`y`).
    pub y: f32,
}

impl Point {
    /// The origin `(0, 0)`.
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };

    /// Creates a point at `(x, y)`.
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    /// Returns `true` if both `x` and `y` are finite.
    #[must_use]
    pub const fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    /// Euclidean distance to `other`.
    #[must_use]
    pub fn distance_to(self, other: Point) -> f32 {
        (other.x - self.x).hypot(other.y - self.y)
    }
}

/// An axis-aligned rectangle normalized as `x0 <= x1` and `y0 <= y1` when
/// constructed via [`Rect::new`] or [`Rect::from_xywh`].
///
/// Exhaustive on purpose so callers can construct and pattern-match sides
/// directly.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    /// Left (`min x`).
    pub x0: f32,
    /// Bottom (`min y` in PageSpace; top in ViewSpace).
    pub y0: f32,
    /// Right (`max x`).
    pub x1: f32,
    /// Top (`max y` in PageSpace; bottom in ViewSpace).
    pub y1: f32,
}

impl Rect {
    /// US Letter (`612 × 792` points, `8.5 × 11` inches).
    pub const LETTER: Rect = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: 612.0,
        y1: 792.0,
    };

    /// ISO A4 (`595.28 × 841.89` points, `210 × 297` mm).
    pub const A4: Rect = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: 595.28,
        y1: 841.89,
    };

    /// The unit square `[0, 1] × [0, 1]`, in which PDF image XObjects and
    /// inline images live before transformation by the CTM.
    pub const UNIT: Rect = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: 1.0,
        y1: 1.0,
    };

    /// Creates a normalized rectangle from two opposite corners `(a, b)` and
    /// `(c, d)` in any order.
    #[must_use]
    pub const fn new(a: f32, b: f32, c: f32, d: f32) -> Rect {
        Rect {
            x0: a.min(c),
            y0: b.min(d),
            x1: a.max(c),
            y1: b.max(d),
        }
    }

    /// Creates a normalized rectangle from corner `(x, y)` and signed or
    /// unsigned extents `(w, h)`.
    #[must_use]
    pub fn from_xywh(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::new(x, y, x + w, y + h)
    }

    /// Parses a normalized rectangle from a 4-number PDF array (ISO 32000-2,
    /// §7.9.5). Returns `None` if fewer than four direct finite numbers are
    /// present.
    #[must_use]
    pub fn from_pdf_array(array: &[Object]) -> Option<Rect> {
        if array.len() < 4 {
            return None;
        }
        let a = array[0].as_number()? as f32;
        let b = array[1].as_number()? as f32;
        let c = array[2].as_number()? as f32;
        let d = array[3].as_number()? as f32;
        let rect = Rect::new(a, b, c, d);
        rect.is_finite().then_some(rect)
    }

    /// Converts this rectangle into a 4-element PDF array `[x0 y0 x1 y1]`.
    #[must_use]
    pub fn to_pdf_array(self) -> Vec<Object> {
        vec![
            Object::from(f64::from(self.x0)),
            Object::from(f64::from(self.y0)),
            Object::from(f64::from(self.x1)),
            Object::from(f64::from(self.y1)),
        ]
    }

    /// Width (`x1 - x0`), clamped to `>= 0.0`.
    #[must_use]
    pub fn width(self) -> f32 {
        (self.x1 - self.x0).max(0.0)
    }

    /// Height (`y1 - y0`), clamped to `>= 0.0`.
    #[must_use]
    pub fn height(self) -> f32 {
        (self.y1 - self.y0).max(0.0)
    }

    /// Dimensions as a [`PageSize`].
    #[must_use]
    pub fn size(self) -> PageSize {
        PageSize {
            width: self.width(),
            height: self.height(),
        }
    }

    /// Returns `true` if `x0 >= x1` or `y0 >= y1` (zero or inverted area).
    #[must_use]
    pub fn is_empty(self) -> bool {
        !(self.x0 < self.x1 && self.y0 < self.y1)
    }

    /// Returns `true` if all four coordinates are finite.
    #[must_use]
    pub const fn is_finite(self) -> bool {
        self.x0.is_finite() && self.y0.is_finite() && self.x1.is_finite() && self.y1.is_finite()
    }

    /// Returns `true` if `p` lies inside or on the boundary of `self`.
    #[must_use]
    pub fn contains(self, p: Point) -> bool {
        p.x >= self.x0 && p.x <= self.x1 && p.y >= self.y0 && p.y <= self.y1
    }

    /// Returns `true` if every point of `other` lies inside or on the boundary
    /// of `self`.
    #[must_use]
    pub fn contains_rect(self, other: Rect) -> bool {
        other.x0 >= self.x0 && other.y0 >= self.y0 && other.x1 <= self.x1 && other.y1 <= self.y1
    }

    /// Non-empty intersection of `self` and `other`, or `None` when disjoint
    /// or touching only along an edge/point.
    #[must_use]
    pub fn intersect(self, other: Rect) -> Option<Rect> {
        let x0 = self.x0.max(other.x0);
        let y0 = self.y0.max(other.y0);
        let x1 = self.x1.min(other.x1);
        let y1 = self.y1.min(other.y1);
        (x0 < x1 && y0 < y1).then_some(Rect { x0, y0, x1, y1 })
    }

    /// Intersection of `self` and `other`, returning [`Rect::default`] when
    /// disjoint (including zero-area intersections).
    #[must_use]
    pub fn intersect_or_empty(self, other: Rect) -> Rect {
        let x0 = self.x0.max(other.x0);
        let y0 = self.y0.max(other.y0);
        let x1 = self.x1.min(other.x1);
        let y1 = self.y1.min(other.y1);
        if x0 <= x1 && y0 <= y1 {
            Rect { x0, y0, x1, y1 }
        } else {
            Rect::default()
        }
    }

    /// Smallest axis-aligned rectangle enclosing both `self` and `other`.
    #[must_use]
    pub const fn union(self, other: Rect) -> Rect {
        Rect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    /// Shrinks `self` inward by `dx` on left/right and `dy` on bottom/top.
    /// Returns `None` if the resulting rectangle has negative width or height.
    #[must_use]
    pub fn inset(self, dx: f32, dy: f32) -> Option<Rect> {
        let x0 = self.x0 + dx;
        let y0 = self.y0 + dy;
        let x1 = self.x1 - dx;
        let y1 = self.y1 - dy;
        (x0 <= x1 && y0 <= y1).then_some(Rect { x0, y0, x1, y1 })
    }

    /// Expands `self` outward by `d` on all four sides.
    #[must_use]
    pub fn expand(self, d: f32) -> Rect {
        Rect::new(self.x0 - d, self.y0 - d, self.x1 + d, self.y1 + d)
    }

    /// The four corners counterclockwise from `(x0, y0)`:
    /// `[(x0, y0), (x1, y0), (x1, y1), (x0, y1)]`.
    #[must_use]
    pub const fn corners(self) -> [Point; 4] {
        [
            Point::new(self.x0, self.y0),
            Point::new(self.x1, self.y0),
            Point::new(self.x1, self.y1),
            Point::new(self.x0, self.y1),
        ]
    }
}

impl From<pdf_fonts::Rect> for Rect {
    fn from(r: pdf_fonts::Rect) -> Rect {
        Rect {
            x0: r.x0,
            y0: r.y0,
            x1: r.x1,
            y1: r.y1,
        }
    }
}

impl From<Rect> for pdf_fonts::Rect {
    fn from(r: Rect) -> pdf_fonts::Rect {
        pdf_fonts::Rect {
            x0: r.x0,
            y0: r.y0,
            x1: r.x1,
            y1: r.y1,
        }
    }
}

impl From<pages::Rect> for Rect {
    fn from(pages::Rect([a, b, c, d]): pages::Rect) -> Rect {
        Rect::new(a as f32, b as f32, c as f32, d as f32)
    }
}

impl From<Rect> for pages::Rect {
    fn from(r: Rect) -> pages::Rect {
        pages::Rect([
            f64::from(r.x0),
            f64::from(r.y0),
            f64::from(r.x1),
            f64::from(r.y1),
        ])
    }
}

/// Four corners of an oriented quadrilateral (e.g., a glyph box or placed image
/// after an affine transformation), ordered as the image of a rectangle's
/// `[(x0, y0), (x1, y0), (x1, y1), (x0, y1)]` corners.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Quad(pub [Point; 4]);

impl Quad {
    /// Axis-aligned quadrilateral from `r`'s four corners.
    #[must_use]
    pub const fn from_rect(r: Rect) -> Quad {
        Quad(r.corners())
    }

    /// Smallest axis-aligned [`Rect`] containing all four corners.
    #[must_use]
    pub fn bounds(&self) -> Rect {
        let [p0, p1, p2, p3] = self.0;
        let x0 = p0.x.min(p1.x).min(p2.x).min(p3.x);
        let y0 = p0.y.min(p1.y).min(p2.y).min(p3.y);
        let x1 = p0.x.max(p1.x).max(p2.x).max(p3.x);
        let y1 = p0.y.max(p1.y).max(p2.y).max(p3.y);
        Rect { x0, y0, x1, y1 }
    }

    /// Returns `true` if all four corners have finite coordinates.
    #[must_use]
    pub const fn is_finite(&self) -> bool {
        self.0[0].is_finite()
            && self.0[1].is_finite()
            && self.0[2].is_finite()
            && self.0[3].is_finite()
    }

    /// Length of the baseline/bottom edge (`0 -> 1`).
    #[must_use]
    pub fn baseline_len(&self) -> f32 {
        self.0[0].distance_to(self.0[1])
    }

    /// Length of the side/upward edge (`0 -> 3`).
    #[must_use]
    pub fn side_len(&self) -> f32 {
        self.0[0].distance_to(self.0[3])
    }
}

/// A 2D affine transformation matrix `[a b c d e f]` (ISO 32000-2, §8.3.3):
///
/// ```text
/// x' = a * x + c * y + e
/// y' = b * x + d * y + f
/// ```
///
/// Exhaustive on purpose so callers can inspect or construct PDF matrix
/// coefficients directly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    /// Horizontal scale / cosine (`a`).
    pub a: f32,
    /// Vertical skew / sine (`b`).
    pub b: f32,
    /// Horizontal skew / negative sine (`c`).
    pub c: f32,
    /// Vertical scale / cosine (`d`).
    pub d: f32,
    /// Horizontal translation (`e`).
    pub e: f32,
    /// Vertical translation (`f`).
    pub f: f32,
}

impl Default for Matrix {
    fn default() -> Matrix {
        Matrix::IDENTITY
    }
}

impl Matrix {
    /// The identity transformation `[1 0 0 1 0 0]`.
    pub const IDENTITY: Matrix = Matrix {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// Creates the matrix `[a b c d e f]`.
    #[must_use]
    pub const fn new(a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) -> Matrix {
        Matrix { a, b, c, d, e, f }
    }

    /// Creates a matrix from `[a, b, c, d, e, f]`.
    #[must_use]
    pub const fn from_array([a, b, c, d, e, f]: [f32; 6]) -> Matrix {
        Matrix { a, b, c, d, e, f }
    }

    /// Returns `[a, b, c, d, e, f]`.
    #[must_use]
    pub const fn to_array(self) -> [f32; 6] {
        [self.a, self.b, self.c, self.d, self.e, self.f]
    }

    /// Parses `[a b c d e f]` from a 6-number PDF array. Returns `None` if
    /// fewer than six direct finite numbers are present.
    #[must_use]
    pub fn from_pdf_array(array: &[Object]) -> Option<Matrix> {
        if array.len() < 6 {
            return None;
        }
        let m = Matrix {
            a: array[0].as_number()? as f32,
            b: array[1].as_number()? as f32,
            c: array[2].as_number()? as f32,
            d: array[3].as_number()? as f32,
            e: array[4].as_number()? as f32,
            f: array[5].as_number()? as f32,
        };
        m.is_finite().then_some(m)
    }

    /// Converts this matrix into a 6-element PDF array `[a b c d e f]`.
    #[must_use]
    pub fn to_pdf_array(self) -> Vec<Object> {
        self.to_array()
            .into_iter()
            .map(|v| Object::from(f64::from(v)))
            .collect()
    }

    /// Pure translation by `(tx, ty)`.
    #[must_use]
    pub const fn translate(tx: f32, ty: f32) -> Matrix {
        Matrix::new(1.0, 0.0, 0.0, 1.0, tx, ty)
    }

    /// Scaling about the origin by `(sx, sy)`.
    #[must_use]
    pub const fn scale(sx: f32, sy: f32) -> Matrix {
        Matrix::new(sx, 0.0, 0.0, sy, 0.0, 0.0)
    }

    /// Counterclockwise rotation about the origin by `radians` in PageSpace.
    #[must_use]
    pub fn rotate_rad(radians: f32) -> Matrix {
        let (s, c) = radians.sin_cos();
        Matrix::new(c, s, -s, c, 0.0, 0.0)
    }

    /// Exact counterclockwise rotation by `turns * 90°` about the origin,
    /// avoiding trigonometric rounding error.
    #[must_use]
    pub const fn quarter_turns(turns: i32) -> Matrix {
        match turns.rem_euclid(4) {
            0 => Matrix::IDENTITY,
            1 => Matrix::new(0.0, 1.0, -1.0, 0.0, 0.0, 0.0),
            2 => Matrix::new(-1.0, 0.0, 0.0, -1.0, 0.0, 0.0),
            _ => Matrix::new(0.0, -1.0, 1.0, 0.0, 0.0, 0.0),
        }
    }

    /// Orientation-preserving affine map placing a non-empty `src` rectangle
    /// onto `dst` in PageSpace (`y` up), rotated clockwise by `rotation`.
    ///
    /// Used when placing Form XObjects (`/BBox` -> target `Rect`) or images
    /// (`Rect::UNIT` -> target `Rect`). Returns `None` if `src` is empty or
    /// either rectangle has non-finite coordinates.
    #[must_use]
    pub fn onto(src: Rect, dst: Rect, rotation: Rotation) -> Option<Matrix> {
        if src.is_empty() || !src.is_finite() || !dst.is_finite() {
            return None;
        }
        let (sw, sh) = (src.width(), src.height());
        let (dw, dh) = (dst.width(), dst.height());
        let shift = Matrix::translate(-src.x0, -src.y0);
        let place = match rotation {
            Rotation::R0 => Matrix::new(dw / sw, 0.0, 0.0, dh / sh, dst.x0, dst.y0),
            Rotation::R90 => Matrix::new(0.0, -dh / sw, dw / sh, 0.0, dst.x0, dst.y1),
            Rotation::R180 => Matrix::new(-dw / sw, 0.0, 0.0, -dh / sh, dst.x1, dst.y1),
            Rotation::R270 => Matrix::new(0.0, dh / sw, -dw / sh, 0.0, dst.x1, dst.y0),
        };
        let out = shift.then(place);
        out.is_finite().then_some(out)
    }

    /// Composes `self` followed by `after`: `(self.then(after)).apply(p) ==
    /// after.apply(self.apply(p))`.
    ///
    /// In PDF content streams, `m cm` updates `ctm = m.then(ctm)` (pre-multiplying
    /// row vectors `v * m * ctm`, matching PDFium's `CFX_Matrix::operator*`).
    #[must_use]
    pub fn then(self, after: Matrix) -> Matrix {
        Matrix {
            a: self.a * after.a + self.b * after.c,
            b: self.a * after.b + self.b * after.d,
            c: self.c * after.a + self.d * after.c,
            d: self.c * after.b + self.d * after.d,
            e: self.e * after.a + self.f * after.c + after.e,
            f: self.e * after.b + self.f * after.d + after.f,
        }
    }

    /// Determinant `a * d - b * c` computed in `f64` so small scales do not
    /// underflow `f32`.
    #[must_use]
    pub fn determinant(self) -> f64 {
        f64::from(self.a) * f64::from(self.d) - f64::from(self.b) * f64::from(self.c)
    }

    /// Inverse matrix, or `None` if singular or if the inverted coefficients
    /// overflow `f32`.
    #[must_use]
    pub fn inverse(self) -> Option<Matrix> {
        let det = self.determinant();
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let inv = 1.0 / det;
        let a = f64::from(self.d) * inv;
        let b = -f64::from(self.b) * inv;
        let c = -f64::from(self.c) * inv;
        let d = f64::from(self.a) * inv;
        let e = -(f64::from(self.e) * a + f64::from(self.f) * c);
        let f = -(f64::from(self.e) * b + f64::from(self.f) * d);
        let m = Matrix {
            a: a as f32,
            b: b as f32,
            c: c as f32,
            d: d as f32,
            e: e as f32,
            f: f as f32,
        };
        m.is_finite().then_some(m)
    }

    /// Transforms a point `p`.
    #[must_use]
    pub fn apply(self, p: Point) -> Point {
        Point {
            x: self.a * p.x + self.c * p.y + self.e,
            y: self.b * p.x + self.d * p.y + self.f,
        }
    }

    /// Transforms a displacement vector `(dx, dy)` without translation.
    #[must_use]
    pub fn apply_vector(self, dx: f32, dy: f32) -> (f32, f32) {
        (self.a * dx + self.c * dy, self.b * dx + self.d * dy)
    }

    /// Transforms the four corners of `r` into a [`Quad`].
    #[must_use]
    pub fn apply_quad(self, r: Rect) -> Quad {
        Quad(r.corners().map(|p| self.apply(p)))
    }

    /// Axis-aligned bounding box of `r` after transformation.
    #[must_use]
    pub fn apply_rect(self, r: Rect) -> Rect {
        self.apply_quad(r).bounds()
    }

    /// Returns `true` if all six coefficients are finite.
    #[must_use]
    pub const fn is_finite(self) -> bool {
        self.a.is_finite()
            && self.b.is_finite()
            && self.c.is_finite()
            && self.d.is_finite()
            && self.e.is_finite()
            && self.f.is_finite()
    }

    /// Returns `true` if `self` maps axis-aligned rectangles to axis-aligned
    /// rectangles (either scaling without skew or a 90°/270° swap without skew).
    #[must_use]
    pub fn is_axis_aligned(self) -> bool {
        (self.b == 0.0 && self.c == 0.0) || (self.a == 0.0 && self.d == 0.0)
    }
}

/// Clockwise quarter-turn page rotation, matching PDF's `/Rotate` entry
/// (ISO 32000-2, §7.7.3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Rotation {
    /// `0°` (upright).
    #[default]
    R0,
    /// `90°` clockwise.
    R90,
    /// `180°` clockwise.
    R180,
    /// `270°` clockwise (`90°` counterclockwise).
    R270,
}

impl Rotation {
    /// Normalizes a degree count (such as a page's `/Rotate`) to a clockwise
    /// quarter-turn. Multiples of `90` map to their quarter-turn; non-multiples
    /// fall back to [`Rotation::R0`], matching PDFium's `GetPageRotation`.
    #[must_use]
    pub const fn from_degrees(degrees: i64) -> Rotation {
        match degrees.rem_euclid(360) {
            90 => Rotation::R90,
            180 => Rotation::R180,
            270 => Rotation::R270,
            _ => Rotation::R0,
        }
    }

    /// Clockwise angle in degrees (`0`, `90`, `180`, or `270`).
    #[must_use]
    pub const fn degrees(self) -> i32 {
        match self {
            Rotation::R0 => 0,
            Rotation::R90 => 90,
            Rotation::R180 => 180,
            Rotation::R270 => 270,
        }
    }

    /// Number of clockwise quarter-turns (`0..=3`), matching PDFium's
    /// `FPDFPage_GetRotation`.
    #[must_use]
    pub const fn quarter_turns(self) -> u8 {
        match self {
            Rotation::R0 => 0,
            Rotation::R90 => 1,
            Rotation::R180 => 2,
            Rotation::R270 => 3,
        }
    }

    /// Rotates `90°` further clockwise.
    #[must_use]
    pub const fn cw(self) -> Rotation {
        Rotation::from_degrees(self.degrees() as i64 + 90)
    }

    /// Rotates `90°` counterclockwise.
    #[must_use]
    pub const fn ccw(self) -> Rotation {
        Rotation::from_degrees(self.degrees() as i64 - 90)
    }

    /// Adds `other` clockwise.
    #[must_use]
    pub const fn plus(self, other: Rotation) -> Rotation {
        Rotation::from_degrees(self.degrees() as i64 + other.degrees() as i64)
    }

    /// Subtracts `other` clockwise.
    #[must_use]
    pub const fn minus(self, other: Rotation) -> Rotation {
        Rotation::from_degrees(self.degrees() as i64 - other.degrees() as i64)
    }

    /// Returns `true` for `R90` and `R270` (where width and height swap).
    #[must_use]
    pub const fn is_quarter(self) -> bool {
        matches!(self, Rotation::R90 | Rotation::R270)
    }
}

/// Width and height of a page or box in PDF user units (1/72 inch by default).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PageSize {
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl PageSize {
    /// US Letter (`612 × 792` points).
    pub const LETTER: PageSize = PageSize {
        width: 612.0,
        height: 792.0,
    };

    /// ISO A4 (`595.28 × 841.89` points).
    pub const A4: PageSize = PageSize {
        width: 595.28,
        height: 841.89,
    };

    /// Size after rotating by `rotation` (`R90` and `R270` swap width and
    /// height).
    #[must_use]
    pub const fn rotated(self, rotation: Rotation) -> PageSize {
        if rotation.is_quarter() {
            PageSize {
                width: self.height,
                height: self.width,
            }
        } else {
            self
        }
    }
}

/// Geometry of a PDF page (ISO 32000-2, §7.7.3.3 & §14.11.2): its boundary
/// boxes in PageSpace, `/Rotate`, `/UserUnit`, and closed-form transforms
/// between PageSpace (`y` up, before `/Rotate`) and ViewSpace (`(0, 0)` at
/// top-left, `y` down, after `/Rotate`).
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct PageGeom {
    /// Normalized `/MediaBox` (defaults to [`Rect::LETTER`] when absent or
    /// empty, matching PDFium).
    pub media_box: Rect,
    /// Effective visible box: `/CropBox` intersected with `media_box`, falling
    /// back to `media_box` if absent or if the intersection is empty.
    pub crop_box: Rect,
    /// Optional `/BleedBox` in PageSpace (normalized).
    pub bleed_box: Option<Rect>,
    /// Optional `/TrimBox` in PageSpace (normalized).
    pub trim_box: Option<Rect>,
    /// Optional `/ArtBox` in PageSpace (normalized).
    pub art_box: Option<Rect>,
    /// Effective `/Rotate` normalized to a clockwise quarter-turn.
    pub rotation: Rotation,
    /// Effective `/UserUnit` (`> 0.0`, default `1.0` = 1/72 inch).
    pub user_unit: f32,
}

impl Default for PageGeom {
    fn default() -> PageGeom {
        PageGeom::new(Rect::LETTER, None, Rotation::R0)
    }
}

impl PageGeom {
    /// Constructs page geometry from `media_box`, an optional `crop_box`, and
    /// `rotation`, with `user_unit = 1.0`.
    ///
    /// If `media_box` is empty or non-finite, falls back to [`Rect::LETTER`].
    /// If `crop_box` is given, it is intersected with `media_box` (falling back
    /// to `media_box` if disjoint or empty, matching PDFium).
    #[must_use]
    pub fn new(media_box: Rect, crop_box: Option<Rect>, rotation: Rotation) -> PageGeom {
        let media_box = if media_box.is_empty() || !media_box.is_finite() {
            Rect::LETTER
        } else {
            media_box
        };
        let crop_box = crop_box
            .filter(|c| c.is_finite())
            .and_then(|c| media_box.intersect(c))
            .unwrap_or(media_box);
        PageGeom {
            media_box,
            crop_box,
            bleed_box: None,
            trim_box: None,
            art_box: None,
            rotation,
            user_unit: 1.0,
        }
    }

    /// Constructs page geometry from a `pdf-semantics` [`pages::Page`]'s
    /// inherited `/MediaBox`, `/CropBox`, `/Rotate`, and `/UserUnit`, plus any
    /// direct `/BleedBox`, `/TrimBox`, and `/ArtBox` arrays on the page
    /// dictionary (use [`PageGeom::with_page_boxes`] when those optional boxes
    /// may be indirect references).
    #[must_use]
    pub fn from_page(page: &pages::Page) -> PageGeom {
        let media = Rect::from(page.media_box().value);
        let crop = Rect::from(page.crop_box().value);
        let rotation = Rotation::from_degrees(page.rotate().value);
        let mut geom = PageGeom::new(media, Some(crop), rotation);
        let unit = page.user_unit().value as f32;
        if unit > 0.0 && unit.is_finite() {
            geom.user_unit = unit;
        }
        if let Some(dict) = page.dict {
            let direct_box = |key: &[u8]| dict.get(key)?.as_array().and_then(Rect::from_pdf_array);
            geom.bleed_box = direct_box(b"BleedBox");
            geom.trim_box = direct_box(b"TrimBox");
            geom.art_box = direct_box(b"ArtBox");
        }
        geom
    }

    /// Populates optional `/BleedBox`, `/TrimBox`, and `/ArtBox` (ISO 32000-2,
    /// §14.11.2) from the page dictionary `page_dict`, resolving indirect
    /// array references via `objects`.
    #[must_use]
    pub fn with_page_boxes<'o>(mut self, objects: impl Resolve<'o>, page_dict: &'o Dict) -> Self {
        let box_for = |key: &[u8]| -> Option<Rect> {
            let val = page_dict.get(key)?;
            let resolved = match val.as_reference() {
                Some(id) => objects.resolve(id)?,
                None => val,
            };
            Rect::from_pdf_array(resolved.as_array()?)
        };
        self.bleed_box = box_for(b"BleedBox");
        self.trim_box = box_for(b"TrimBox");
        self.art_box = box_for(b"ArtBox");
        self
    }

    /// Effective visible box (`crop_box`).
    #[must_use]
    pub const fn visible_box(&self) -> Rect {
        self.crop_box
    }

    /// Visible dimensions in user units after applying `rotation`.
    #[must_use]
    pub fn view_size(&self) -> PageSize {
        self.crop_box.size().rotated(self.rotation)
    }

    /// Visible dimensions scaled by `user_unit` (in 1/72-inch points).
    #[must_use]
    pub fn scaled_view_size(&self) -> PageSize {
        let size = self.view_size();
        PageSize {
            width: size.width * self.user_unit,
            height: size.height * self.user_unit,
        }
    }

    /// Matrix mapping PageSpace (`y` up, before `/Rotate`) to ViewSpace
    /// (`(0, 0)` at the visible top-left, `y` down, `[0..w] × [0..h]` after
    /// `/Rotate`).
    #[must_use]
    pub const fn to_view(&self) -> Matrix {
        let c = self.crop_box;
        match self.rotation {
            Rotation::R0 => Matrix::new(1.0, 0.0, 0.0, -1.0, -c.x0, c.y1),
            Rotation::R90 => Matrix::new(0.0, 1.0, 1.0, 0.0, -c.y0, -c.x0),
            Rotation::R180 => Matrix::new(-1.0, 0.0, 0.0, 1.0, c.x1, -c.y0),
            Rotation::R270 => Matrix::new(0.0, -1.0, -1.0, 0.0, c.y1, c.x1),
        }
    }

    /// Matrix mapping ViewSpace (`(0, 0)` at the visible top-left, `y` down)
    /// back to PageSpace (`y` up, before `/Rotate`), exact inverse of
    /// [`PageGeom::to_view`].
    #[must_use]
    pub const fn from_view(&self) -> Matrix {
        let c = self.crop_box;
        match self.rotation {
            Rotation::R0 => Matrix::new(1.0, 0.0, 0.0, -1.0, c.x0, c.y1),
            Rotation::R90 => Matrix::new(0.0, 1.0, 1.0, 0.0, c.x0, c.y0),
            Rotation::R180 => Matrix::new(-1.0, 0.0, 0.0, 1.0, c.x1, c.y0),
            Rotation::R270 => Matrix::new(0.0, -1.0, -1.0, 0.0, c.x1, c.y1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_round_trips_every_corner_and_rotation() {
        let media = Rect::new(10.0, 20.0, 622.0, 812.0);
        let crop = Rect::new(30.0, 50.0, 530.0, 750.0); // 500 x 700
        for rot in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
            let geom = PageGeom::new(media, Some(crop), rot);
            let to = geom.to_view();
            let from = geom.from_view();
            let view_rect = to.apply_rect(crop);
            let size = geom.view_size();
            assert_eq!(
                (view_rect.x0, view_rect.y0, view_rect.x1, view_rect.y1),
                (0.0, 0.0, size.width, size.height),
                "{rot:?}"
            );
            for p in crop.corners() {
                let back = from.apply(to.apply(p));
                assert!((back.x - p.x).abs() < 1e-4 && (back.y - p.y).abs() < 1e-4);
            }
            assert_eq!(to.then(from), Matrix::IDENTITY, "{rot:?}");
        }
    }

    #[test]
    fn onto_maps_source_box_onto_destination_with_positive_determinant() {
        let src = Rect::new(10.0, 20.0, 110.0, 70.0);
        let dst = Rect::new(200.0, 300.0, 400.0, 600.0);
        for rot in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
            let m = Matrix::onto(src, dst, rot).unwrap();
            assert!(m.determinant() > 0.0, "{rot:?}: det={}", m.determinant());
            let mapped = m.apply_rect(src);
            assert!((mapped.x0 - dst.x0).abs() < 1e-4);
            assert!((mapped.y0 - dst.y0).abs() < 1e-4);
            assert!((mapped.x1 - dst.x1).abs() < 1e-4);
            assert!((mapped.y1 - dst.y1).abs() < 1e-4);
            let inv = m.inverse().unwrap();
            let back = inv.apply(m.apply(Point::new(35.0, 42.0)));
            assert!((back.x - 35.0).abs() < 1e-3 && (back.y - 42.0).abs() < 1e-3);
        }
        assert!(Matrix::onto(Rect::new(5.0, 5.0, 5.0, 10.0), dst, Rotation::R0).is_none());
    }
}
