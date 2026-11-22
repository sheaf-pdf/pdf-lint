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

use crate::subtypes::AttachmentIcon;

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

/// A pushpin: its head, then its pin.
const PUSH_PIN: &[&[(f32, f32)]] = &[
    &[
        (0.3, 0.12),
        (0.7, 0.12),
        (0.6, 0.32),
        (0.68, 0.56),
        (0.32, 0.56),
        (0.4, 0.32),
        (0.3, 0.12),
    ],
    &[(0.5, 0.56), (0.5, 0.94)],
];

/// A paperclip, one wire.
const PAPERCLIP: &[&[(f32, f32)]] = &[&[
    (0.36, 0.72),
    (0.36, 0.24),
    (0.5, 0.1),
    (0.64, 0.24),
    (0.64, 0.82),
    (0.5, 0.94),
    (0.43, 0.84),
    (0.43, 0.32),
]];

/// A bar graph: its axes, then three bars.
const GRAPH: &[&[(f32, f32)]] = &[
    &[(0.14, 0.1), (0.14, 0.86), (0.9, 0.86)],
    &[(0.32, 0.86), (0.32, 0.58)],
    &[(0.52, 0.86), (0.52, 0.36)],
    &[(0.72, 0.86), (0.72, 0.18)],
];

/// A tag: its outline, then its hole.
const TAG: &[&[(f32, f32)]] = &[
    &[
        (0.1, 0.3),
        (0.6, 0.3),
        (0.9, 0.5),
        (0.6, 0.7),
        (0.1, 0.7),
        (0.1, 0.3),
    ],
    &[
        (0.62, 0.45),
        (0.7, 0.45),
        (0.7, 0.55),
        (0.62, 0.55),
        (0.62, 0.45),
    ],
];

/// A file attachment's icon (`/FileAttachment`'s `/Name`, ISO 32000-2
/// §12.5.6.15) as the strokes it's drawn with, each a line through its
/// points, the icon's unit square taken where it is by `frame` (see the
/// module's docs): a pushpin, a paperclip, a bar graph or a tag, and a name
/// the standard doesn't have drawn as its default, the pushpin. Each point
/// is the frame applied to its own place in the square.
#[must_use]
pub fn attachment_icon_strokes(icon: &AttachmentIcon, frame: Matrix) -> Vec<Vec<Point>> {
    let strokes = match icon {
        AttachmentIcon::Paperclip => PAPERCLIP,
        AttachmentIcon::Graph => GRAPH,
        AttachmentIcon::Tag => TAG,
        AttachmentIcon::PushPin | AttachmentIcon::Other(_) => PUSH_PIN,
    };
    strokes
        .iter()
        .map(|stroke| {
            (stroke.iter())
                .map(|&(x, y)| frame.apply(Point::new(x, y)))
                .collect()
        })
        .collect()
}
