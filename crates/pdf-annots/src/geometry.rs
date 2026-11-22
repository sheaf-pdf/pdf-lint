//! The shapes some annotations draw, as points: what a synthesized
//! appearance draws them by, public so that a viewer that paints a mark
//! itself paints the same shape, to the bit.
//!
//! A picture of a mark (a caret, an icon) is given in its own unit square,
//! `(0, 0)` its top left as it reads and `(1, 1)` its bottom right, y running
//! down it as a picture's does, and drawn through a `frame`: the [`Matrix`]
//! that takes that square onto where the mark is. Each point is
//! `frame.apply` of the picture's own, nothing else, so whoever draws the
//! same picture through the same frame has the same numbers. On a page that
//! isn't turned the frame of the box `[x0 y0 x1 y1]` is `Matrix { a: x1 - x0,
//! b: 0.0, c: 0.0, d: y0 - y1, e: x0, f: y1 }`; a page turned by its
//! `/Rotate` turns it with the page, so the picture still reads upright.

use pdf_gfx::{Matrix, Point, Rect};

/// The frame of `r` on a page that isn't turned: its top left at the unit
/// square's origin, the square's y down the box.
pub(crate) fn upright_frame(r: Rect) -> Matrix {
    Matrix {
        a: r.width(),
        b: 0.0,
        c: 0.0,
        d: -r.height(),
        e: r.x0,
        f: r.y1,
    }
}

/// A picture's points, each its own place in the unit square, through
/// `frame`.
fn through<const N: usize>(frame: Matrix, unit: [(f32, f32); N]) -> [Point; N] {
    unit.map(|(x, y)| frame.apply(Point::new(x, y)))
}

/// A caret's triangle (`/Caret`, ISO 32000-2 §12.5.6.11): its base along
/// the bottom of its box as it reads, its tip at the middle of the top, the
/// box's unit square taken where the caret is by `frame` (see the module's
/// docs). The points are the frame applied to `(0, 1)`, `(0.5, 0)` and
/// `(1, 1)`, in that order.
#[must_use]
pub fn caret_outline(frame: Matrix) -> [Point; 3] {
    through(frame, [(0.0, 1.0), (0.5, 0.0), (1.0, 1.0)])
}
