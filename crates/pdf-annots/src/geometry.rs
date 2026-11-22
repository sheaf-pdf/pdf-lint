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

/// The most scallops a cloud is drawn with: past that (a step too short
/// for its outline) none is, and its outline is drawn plain.
pub(crate) const MOST_SCALLOPS: usize = 1 << 16;

/// How long a cloudy border's scallops are along its outline, in points,
/// for a border `width` wide at `intensity` (`/BE /I`, ISO 32000-2 §12.5.4
/// Table 167, from 0 to 2): at an intensity of 1, 9 points and three times
/// the width, at most 30; at another, that times the intensity, so that an
/// intensity of 0 makes none.
#[must_use]
pub fn cloud_step(width: f32, intensity: f32) -> f32 {
    (9.0 + width * 3.0).min(30.0) * intensity
}

/// A cloudy border (`/BE /S /C`) around the closed outline `outline`: each
/// side cut into scallops about `step` long, each a cubic curve `[from,
/// control, control, to]` bulging outward, away from the outline's inside
/// whichever way round it runs, in the same space as `outline` and in its
/// order. A side shorter than 0.01 has none.
///
/// None for an outline of fewer than three points, a `step` that isn't a
/// number above 0, or one that would cut the outline into more than 65,536
/// scallops.
#[must_use]
pub fn cloud_outline(outline: &[Point], step: f32) -> Vec<[Point; 4]> {
    let n = outline.len();
    if n < 3 || !(step > 0.0 && step.is_finite()) {
        return Vec::new();
    }
    let sides = || {
        (0..n).filter_map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % n]);
            let len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
            if len < 0.01 {
                return None;
            }
            Some((a, b, len, (len / step).round().max(1.0)))
        })
    };
    if sides().map(|(.., k)| f64::from(k)).sum::<f64>() > MOST_SCALLOPS as f64 {
        return Vec::new();
    }
    // Which side is out: the outline's turning, from its signed area.
    let area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    let out = if area > 0.0 { 1.0 } else { -1.0 };
    let mut curves = Vec::new();
    for (a, b, len, k) in sides() {
        let k = k as usize;
        for j in 0..k {
            let t0 = j as f32 / k as f32;
            let t1 = (j + 1) as f32 / k as f32;
            let p = Point::new(a.x + (b.x - a.x) * t0, a.y + (b.y - a.y) * t0);
            let q = Point::new(a.x + (b.x - a.x) * t1, a.y + (b.y - a.y) * t1);
            let seg = len / k as f32;
            let (nx, ny) = (out * (q.y - p.y) / seg, out * -(q.x - p.x) / seg);
            let h = seg * 0.55;
            curves.push([
                p,
                Point::new(p.x + nx * h, p.y + ny * h),
                Point::new(q.x + nx * h, q.y + ny * h),
                q,
            ]);
        }
    }
    curves
}

/// The outline a cloudy square's scallops run along: `r`'s corners, from
/// its top left round the way the page's text reads.
pub(crate) fn box_outline(r: Rect) -> [Point; 4] {
    [
        Point::new(r.x0, r.y1),
        Point::new(r.x1, r.y1),
        Point::new(r.x1, r.y0),
        Point::new(r.x0, r.y0),
    ]
}

/// The outline a cloudy circle's scallops run along: the ellipse inscribed
/// in `r` in 32 steps, from its rightmost point round the way a square's
/// runs.
pub(crate) fn ellipse_outline(r: Rect) -> Vec<Point> {
    let (rx, ry) = (r.width() / 2.0, r.height() / 2.0);
    let (cx, cy) = (r.x0 + rx, r.y0 + ry);
    (0..32)
        .map(|i| {
            let t = i as f32 / 32.0 * std::f32::consts::TAU;
            Point::new(cx + rx * t.cos(), cy - ry * t.sin())
        })
        .collect()
}
