//! Active clipping path state (`CPDF_ClipPath`).
//!
//! Each clipping part intersects what follows: either a vector path under a
//! [`FillRule`] (`W` / `W*` or a Form XObject's `/BBox`), or the glyph boxes of
//! text shown in a clipping rendering mode (`4..=7 Tr`) between `BT` and `ET`.
//! [`Clip`] is an `Arc`-backed persistent list so `q` / `Q` clones in `O(1)`
//! time without heap allocation and drops iteratively without recursion.

use std::sync::Arc;

use pdf_content_streams::FillRule;

use crate::geom::{Point, Quad, Rect};
use crate::path::Seg;

/// PDFium's cap on text objects accumulated in a clipping path
/// (`CPDF_ClipPath::AppendTexts`): text clips beyond this count are ignored.
pub(crate) const TEXTS: usize = 1024;

/// Active clipping stack in the graphics state, ordered innermost (most
/// recently added) first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Clip(Option<Arc<Node>>);

#[derive(Debug, PartialEq)]
struct Node {
    part: ClipPart,
    /// Conservative intersection of this part's bounds and all outer parts
    /// (`Rect::default()` when disjoint).
    bounds: Rect,
    /// Axis-aligned rectangle represented by `part`, if `part` is an
    /// axis-aligned 4-corner path (used for PDFium's `AppendPathWithAutoMerge`).
    rect: Option<Rect>,
    /// Cumulative text-clip count across this node and its ancestors, counted
    /// the way PDFium's `m_TextList` counts (`objects + 1` per `ET` sentinel).
    texts: usize,
    /// Cumulative path segment count across this node and its ancestors, what
    /// [`Limits::segments`](crate::Limits::segments) holds the clip to, kept
    /// here so that holding it never walks the list.
    segments: usize,
    outer: Clip,
}

/// One element of a [`Clip`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ClipPart {
    /// A path in PageSpace clipped under `rule`, together with the bounding box
    /// of its control and anchor points.
    Path {
        /// Placed path segments in PageSpace.
        segments: Arc<[Seg]>,
        /// Winding rule (`NonZero` or `EvenOdd`).
        rule: FillRule,
        /// Axis-aligned bounding box of `segments`' points.
        bounds: Rect,
    },
    /// Glyph boxes accumulated in a clipping text rendering mode (`4..=7 Tr`)
    /// between `BT` and `ET`.
    Text {
        /// Loose glyph quadrilaterals in PageSpace.
        glyphs: Arc<[Quad]>,
        /// Axis-aligned bounding box enclosing `glyphs`.
        bounds: Rect,
    },
}

impl ClipPart {
    /// Axis-aligned bounding box of this clip part in PageSpace.
    #[must_use]
    pub const fn bounds(&self) -> Rect {
        match self {
            ClipPart::Path { bounds, .. } | ClipPart::Text { bounds, .. } => *bounds,
        }
    }
}

/// Iterator over the [`ClipPart`]s of a [`Clip`], innermost first.
#[derive(Debug, Clone)]
pub struct ClipParts<'a> {
    next: Option<&'a Node>,
}

impl<'a> Iterator for ClipParts<'a> {
    type Item = &'a ClipPart;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.next?;
        self.next = node.outer.0.as_deref();
        Some(&node.part)
    }
}

impl std::iter::FusedIterator for ClipParts<'_> {}

impl Clip {
    /// Returns `true` when no clipping path or text clip is active.
    #[must_use]
    pub const fn is_unclipped(&self) -> bool {
        self.0.is_none()
    }

    /// Iterates active [`ClipPart`]s from innermost (most recently applied) to
    /// outermost.
    #[must_use]
    pub fn parts(&self) -> ClipParts<'_> {
        ClipParts {
            next: self.0.as_deref(),
        }
    }

    /// The path segments of every [`ClipPart::Path`] active, counted as the
    /// clip was built.
    pub(crate) fn segments(&self) -> usize {
        self.0.as_ref().map_or(0, |node| node.segments)
    }

    /// Conservative axis-aligned bounding box of everything left visible by the
    /// clip (`CPDF_ClipPath::GetClipBox`), or `None` when unclipped.
    ///
    /// Disjoint clips yield [`Rect::default`] (`0, 0, 0, 0`).
    #[must_use]
    pub fn bounds(&self) -> Option<Rect> {
        self.0.as_ref().map(|node| node.bounds)
    }

    /// Intersects `self` with `segments` filled by `rule`, whose points span
    /// `bounds`.
    ///
    /// Matches PDFium's `AppendPathWithAutoMerge`: if the immediately preceding
    /// clip part is an axis-aligned rectangle that completely contains
    /// `bounds`, `segments` replaces that outer rectangle.
    pub(crate) fn with_path(&self, segments: &[Seg], rule: FillRule, bounds: Rect) -> Clip {
        let outer = match self.0.as_deref() {
            Some(Node {
                rect: Some(r),
                outer,
                ..
            }) if r.contains_rect(bounds) => outer,
            _ => self,
        };
        let rect = axis_rect(segments);
        let part = ClipPart::Path {
            segments: segments.into(),
            rule,
            bounds,
        };
        outer.with(part, rect, 0)
    }

    /// Intersects `self` with `glyphs` produced by `objects` text-showing
    /// operations between `BT` and `ET`, unless doing so exceeds PDFium's
    /// [`TEXTS`] cap.
    pub(crate) fn with_text(&self, glyphs: Vec<Quad>, objects: usize) -> Clip {
        let texts = self.0.as_ref().map_or(0, |node| node.texts);
        if texts + objects > TEXTS {
            return self.clone();
        }
        let mut quads = glyphs.iter().map(Quad::bounds);
        let first = quads.next().unwrap_or_default();
        let bounds = quads.fold(first, Rect::union);
        let part = ClipPart::Text {
            glyphs: glyphs.into(),
            bounds,
        };
        self.with(part, None, objects + 1)
    }

    fn with(&self, part: ClipPart, rect: Option<Rect>, texts: usize) -> Clip {
        let outer = self.0.as_ref();
        let bounds = outer.map_or(part.bounds(), |node| {
            node.bounds.intersect_or_empty(part.bounds())
        });
        let segments = match &part {
            ClipPart::Path { segments, .. } => segments.len(),
            ClipPart::Text { .. } => 0,
        };
        Clip(Some(Arc::new(Node {
            part,
            bounds,
            rect,
            texts: outer.map_or(0, |node| node.texts) + texts,
            segments: outer.map_or(0, |node| node.segments) + segments,
            outer: self.clone(),
        })))
    }
}

impl Drop for Clip {
    fn drop(&mut self) {
        let mut next = self.0.take();
        while let Some(node) = next {
            next = Arc::try_unwrap(node)
                .ok()
                .and_then(|mut node| node.outer.0.take());
        }
    }
}

/// Returns `Some(rect)` if `segs` forms a 4-corner axis-aligned rectangle.
fn axis_rect(segs: &[Seg]) -> Option<Rect> {
    let mut corners = [Point::ZERO; 5];
    let mut n = 0;
    for &seg in segs {
        match seg {
            Seg::Move(p) if n == 0 => corners[0] = p,
            Seg::Line(p) if n > 0 && n < 5 => corners[n] = p,
            Seg::Close => continue,
            _ => return None,
        }
        n += 1;
    }
    if n == 5 && corners[4] == corners[0] {
        n = 4;
    }
    let [a, b, c, d] = corners[..n] else {
        return None;
    };
    let across = a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y;
    let up = a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x;
    (across || up).then(|| Rect::new(a.x, a.y, c.x, c.y))
}
