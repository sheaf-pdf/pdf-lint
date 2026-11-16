//! Vector path construction, placement, and painted bounds
//! (`CPDF_StreamContentParser::AddPathPoint`, `ParsePathObject`,
//! `AddPathObject`).
//!
//! Paths are built in current user space and transformed into PageSpace by the
//! CTM active at the painting/clipping operator that ends the path. Consistent
//! with PDFium and ISO 32000-2 §8.5.2, the path under construction is not part
//! of the graphics state saved by `q` / `Q`.
//!
//! After a two-operand `m`, PDFium enters `ParsePathObject`'s fast path:
//! consecutive numeric path-construction operators populate a 6-float scratch
//! buffer from index `0` without clearing untouched slots. Any non-path
//! operator or non-numeric operand exits the fast path until the next two-arg
//! `m`.

use std::f32::consts::SQRT_2;

use pdf_content_streams::{FillRule, LineCap, LineJoin, Operation, Operator, Paint};

use crate::geom::{Point, Rect};
use crate::state::{State, nums};

/// One segment of a placed path in PageSpace.
///
/// Exhaustive on purpose: PDF paths consist solely of moves, straight lines,
/// cubic Bézier curves (`v` and `y` expand to cubic curves during
/// construction), and subpath closures (`re` expands to a closed 4-line
/// subpath).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seg {
    /// Starts a new subpath at `Point`.
    Move(Point),
    /// Straight line from the previous point to `Point`.
    Line(Point),
    /// Cubic Bézier curve with two control points and endpoint.
    Curve(Point, Point, Point),
    /// Closes the current subpath (`h`). When the pen was not already at the
    /// subpath start, the explicit closing [`Seg::Line`] precedes `Close`,
    /// matching PDFium.
    Close,
}

impl Seg {
    /// Anchor and control points belonging to this segment (`0`, `1`, or `3`
    /// points).
    pub fn points(&self) -> impl Iterator<Item = Point> {
        let pts = match *self {
            Seg::Move(p) | Seg::Line(p) => [Some(p), None, None],
            Seg::Curve(a, b, p) => [Some(a), Some(b), Some(p)],
            Seg::Close => [None; 3],
        };
        pts.into_iter().flatten()
    }
}

/// A path painted or turned into a clip by a path-ending operator (`S`, `s`,
/// `f`, `F`, `f*`, `B`, `B*`, `b`, `b*`, or `n` after `W`/`W*`).
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct PathDrawn<'e> {
    /// Segments transformed into PageSpace by the CTM at the painting operator.
    ///
    /// When a single-point path is clipped (`W n`), PDFium replaces it with a
    /// degenerate closed zero-area rectangle at PageSpace's origin so nothing
    /// subsequent is visible.
    pub segments: &'e [Seg],
    /// How the path is painted (`Paint::None` when `n` ends a clipping path).
    pub paint: Paint,
    /// Clipping rule set by a preceding `W` or `W*` on this path, if any.
    ///
    /// Note that `Event::state.clip` on the [`Kind::Path`](crate::Kind::Path)
    /// event reflects the clip *before* this path; this path's new clip takes
    /// effect on all subsequent events.
    pub clip: Option<FillRule>,
    /// Conservative axis-aligned bounding box in PageSpace enclosing everything
    /// painted (including stroke width, square-cap overhang, and miter-join
    /// reach when stroked; `0.5` pt hairlines when `line_width == 0.0`).
    pub bounds: Rect,
}

/// Maximum capacity retained across path resets so one giant path does not stay
/// permanently allocated.
const KEPT_SEGMENTS: usize = 1 << 16;

/// Mutable user-space path builder.
#[derive(Debug)]
pub(crate) struct Builder {
    segs: Vec<Seg>,
    /// Double-buffered output slice in PageSpace so path emission does not
    /// allocate per path.
    placed: Vec<Seg>,
    start: Point,
    current: Point,
    clip: Option<FillRule>,
    fast: Option<[f32; 6]>,
}

pub(crate) struct Ended<'a> {
    pub(crate) segs: &'a [Seg],
    pub(crate) paint: Paint,
    pub(crate) clip: Option<FillRule>,
    pub(crate) points: Rect,
    pub(crate) bounds: Rect,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            segs: Vec::new(),
            placed: Vec::new(),
            start: Point::ZERO,
            current: Point::ZERO,
            clip: None,
            fast: None,
        }
    }
}

impl Builder {
    pub(crate) fn len(&self) -> usize {
        self.segs.len()
    }

    fn keeps(&mut self, op: Operator, step: &Operation<'_>) -> bool {
        let path_op = matches!(
            op,
            Operator::Move
                | Operator::Line
                | Operator::Curve
                | Operator::CurveFromPen
                | Operator::CurveToEnd
                | Operator::ClosePath
                | Operator::Rect
        );
        if self.fast.is_some()
            && !(path_op && step.inline_image().is_none() && step.operands().all(|o| o.is_number()))
        {
            self.fast = None;
        }
        self.fast.is_some()
    }

    pub(crate) fn build(&mut self, op: Operator, step: &Operation<'_>) {
        if self.keeps(op, step) {
            self.fast_op(op, step);
            return;
        }
        match op {
            Operator::Move if step.operands().len() == 2 => {
                if let Some([x, y]) = nums(step) {
                    self.move_to(Point::new(x, y));
                }
                self.fast = Some([0.0; 6]);
            }
            Operator::Line if step.operands().len() == 2 => {
                if let Some([x, y]) = nums(step) {
                    self.line_to(Point::new(x, y));
                }
            }
            Operator::Curve => {
                if let Some([x1, y1, x2, y2, x3, y3]) = nums(step) {
                    self.curve(Point::new(x1, y1), Point::new(x2, y2), Point::new(x3, y3));
                }
            }
            Operator::CurveFromPen => {
                if let Some([x2, y2, x3, y3]) = nums(step) {
                    self.curve(self.current, Point::new(x2, y2), Point::new(x3, y3));
                }
            }
            Operator::CurveToEnd => {
                if let Some([x1, y1, x3, y3]) = nums(step) {
                    let end = Point::new(x3, y3);
                    self.curve(Point::new(x1, y1), end, end);
                }
            }
            Operator::ClosePath => self.close(),
            Operator::Rect => {
                if let Some([x, y, w, h]) = nums(step) {
                    self.rect(x, y, w, h);
                }
            }
            Operator::Clip => self.clip = Some(FillRule::NonZero),
            Operator::ClipEvenOdd => self.clip = Some(FillRule::EvenOdd),
            _ => {}
        }
    }

    pub(crate) fn interrupt(&mut self) {
        self.fast = None;
    }

    fn fast_op(&mut self, op: Operator, step: &Operation<'_>) {
        let Some(n) = self.fast.as_mut() else {
            return;
        };
        for (slot, operand) in n.iter_mut().zip(step.operands()) {
            *slot = operand.number();
        }
        let n = *n;
        let used = match op {
            Operator::Move | Operator::Line => 2,
            Operator::Curve => 6,
            Operator::CurveFromPen | Operator::CurveToEnd | Operator::Rect => 4,
            _ => 0,
        };
        if !n[..used].iter().all(|v| v.is_finite()) {
            return;
        }
        let at = |i: usize| Point::new(n[i], n[i + 1]);
        match op {
            Operator::Move => self.move_to(at(0)),
            Operator::Line => self.line_to(at(0)),
            Operator::Curve => self.curve(at(0), at(2), at(4)),
            Operator::CurveFromPen => self.curve(self.current, at(0), at(2)),
            Operator::CurveToEnd => self.curve(at(0), at(2), at(2)),
            Operator::ClosePath => self.close(),
            _ => self.rect(n[0], n[1], n[2], n[3]),
        }
    }

    fn move_to(&mut self, p: Point) {
        self.current = p;
        self.start = p;
        match self.segs.last_mut() {
            Some(Seg::Move(last)) => *last = p,
            _ => self.segs.push(Seg::Move(p)),
        }
    }

    fn line_to(&mut self, p: Point) {
        self.current = p;
        if !self.segs.is_empty() {
            self.segs.push(Seg::Line(p));
        }
    }

    fn curve(&mut self, a: Point, b: Point, p: Point) {
        self.current = p;
        if !self.segs.is_empty() {
            self.segs.push(Seg::Curve(a, b, p));
        }
    }

    fn close(&mut self) {
        if self.segs.is_empty() {
            return;
        }
        if self.current != self.start {
            self.close_to(self.start);
        } else if self.segs.last() != Some(&Seg::Close) {
            self.segs.push(Seg::Close);
        }
    }

    fn close_to(&mut self, p: Point) {
        self.current = p;
        if !self.segs.is_empty() {
            self.segs.extend([Seg::Line(p), Seg::Close]);
        }
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.move_to(Point::new(x, y));
        self.line_to(Point::new(x + w, y));
        self.line_to(Point::new(x + w, y + h));
        self.line_to(Point::new(x, y + h));
        self.close_to(Point::new(x, y));
    }

    pub(crate) fn end(&mut self, paint: Paint, state: &State) -> Option<Ended<'_>> {
        match paint {
            Paint::CloseStroke | Paint::CloseFillStroke => self.close(),
            // PDFium's `Handle_CloseEOFillStrokePath` (`b*`) appends a closing
            // line even when the pen is already at `self.start`.
            Paint::CloseFillStrokeEvenOdd => self.close_to(self.start),
            _ => {}
        }
        std::mem::swap(&mut self.segs, &mut self.placed);
        self.segs.clear();
        self.segs.shrink_to(KEPT_SEGMENTS);
        let segs = &mut self.placed;
        let clip = self.clip.take();
        if segs.is_empty() {
            return None;
        }
        if segs.iter().map(|s| s.points().count()).sum::<usize>() == 1 {
            if clip.is_some() {
                segs.clear();
                segs.push(Seg::Move(Point::ZERO));
                segs.extend([Seg::Line(Point::ZERO); 4]);
                segs.push(Seg::Close);
                return Some(Ended {
                    segs,
                    paint: Paint::None,
                    clip: Some(FillRule::NonZero),
                    points: Rect::default(),
                    bounds: Rect::default(),
                });
            }
            let [Seg::Move(p), Seg::Close] = segs[..] else {
                return None;
            };
            if state.line_cap != LineCap::Round {
                return None;
            }
            segs.clear();
            segs.extend([Seg::Move(p), Seg::Line(p), Seg::Close]);
        }
        if matches!(segs.last(), Some(Seg::Move(_))) {
            segs.pop();
        }
        if !paint.strokes() && paint.fills().is_none() && clip.is_none() {
            return None;
        }
        let m = state.ctm;
        for seg in segs.iter_mut() {
            *seg = match *seg {
                Seg::Move(p) => Seg::Move(m.apply(p)),
                Seg::Line(p) => Seg::Line(m.apply(p)),
                Seg::Curve(a, b, p) => Seg::Curve(m.apply(a), m.apply(b), m.apply(p)),
                Seg::Close => Seg::Close,
            };
        }
        let points = bounds(segs);
        let bounds = if paint.strokes() {
            stroked(points, segs, state)
        } else {
            points
        };
        Some(Ended {
            segs,
            paint,
            clip,
            points,
            bounds,
        })
    }
}

pub(crate) fn finite(segs: &[Seg]) -> bool {
    segs.iter().flat_map(Seg::points).all(Point::is_finite)
}

pub(crate) fn bounds(segs: &[Seg]) -> Rect {
    let mut points = segs.iter().flat_map(Seg::points);
    let Some(first) = points.next() else {
        return Rect::default();
    };
    points.fold(Rect::new(first.x, first.y, first.x, first.y), |r, p| Rect {
        x0: r.x0.min(p.x),
        y0: r.y0.min(p.y),
        x1: r.x1.max(p.x),
        y1: r.y1.max(p.y),
    })
}

fn stroked(points: Rect, segs: &[Seg], state: &State) -> Rect {
    let width = state.line_width.abs();
    let (dx, dy) = if width == 0.0 {
        (0.5, 0.5)
    } else {
        let single_point = points.x0 == points.x1 && points.y0 == points.y1;
        let r = reach(width, !single_point && joins(segs), state);
        let m = state.ctm;
        (r * m.a.hypot(m.c), r * m.b.hypot(m.d))
    };
    Rect {
        x0: points.x0 - dx,
        y0: points.y0 - dy,
        x1: points.x1 + dx,
        y1: points.y1 + dy,
    }
}

fn reach(width: f32, has_joins: bool, state: &State) -> f32 {
    let cap = match state.line_cap {
        LineCap::Square => SQRT_2,
        LineCap::Butt | LineCap::Round => 1.0,
    };
    let join = match state.line_join {
        LineJoin::Miter if has_joins => state.miter_limit.max(1.0),
        LineJoin::Miter | LineJoin::Round | LineJoin::Bevel => 1.0,
    };
    width / 2.0 * cap.max(join)
}

fn joins(segs: &[Seg]) -> bool {
    let mut drawn = 0;
    for seg in segs {
        match seg {
            Seg::Move(_) => drawn = 0,
            Seg::Line(_) => drawn += 1,
            Seg::Curve(..) | Seg::Close => return true,
        }
        if drawn > 1 {
            return true;
        }
    }
    false
}
