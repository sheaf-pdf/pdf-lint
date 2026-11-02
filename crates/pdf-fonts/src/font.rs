//! What a font's metrics are given in.

/// A rectangle, in glyph units unless it says otherwise: thousandths of text
/// space at a font size of 1. Normalized, `x0 <= x1` and `y0 <= y1`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    /// Its left.
    pub x0: f32,
    /// Its bottom.
    pub y0: f32,
    /// Its right.
    pub x1: f32,
    /// Its top.
    pub y1: f32,
}

impl Rect {
    /// The rectangle with corners `(a, b)` and `(c, d)`, in either order.
    pub fn new(a: f32, b: f32, c: f32, d: f32) -> Rect {
        Rect {
            x0: a.min(c),
            y0: b.min(d),
            x1: a.max(c),
            y1: b.max(d),
        }
    }
}

/// How far a font's glyphs reach above and below the baseline, and the box all
/// of them fit in, in glyph units: thousandths of text space at a font size
/// of 1. A Type 3 font whose matrix turns glyph space over may have its ascent
/// below its descent, as PDFium keeps them; a glyph's box spans the two either
/// way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontMetrics {
    /// Above the baseline.
    pub ascent: f32,
    /// Below the baseline: negative.
    pub descent: f32,
    /// The box all its glyphs fit in, where it has one.
    pub bbox: Option<Rect>,
}
