//! Deterministic `/AP /N` appearance Form XObject synthesis over
//! [`pdf_gfx::Canvas`] (ISO 32000-2 §12.5.5).

use std::fmt;

use pdf_content_streams::{
    Blend, Color, GState, LineCap, LineJoin, Paint, Res, ResBody, Reserved, ResourceNeeds,
};
use pdf_fonts::{Encoding, Std14};
use pdf_gfx::{Canvas, Point, Quad, Rect};
use pdf_objects::{Dict, Object, Stream, dict};

use crate::annot::Annot;
use crate::resolve::QuadPoints;
use crate::style::{
    AnnotColor, Border, BorderStyle, DefaultAppearance, LineEnding, PaintStyle, Quadding,
};
use crate::subtypes::{PolyKind, ShapeKind, StampName, TextIcon, TextMarkupKind, TypedAnnot};

/// Self-contained synthesized Form XObject stream ready to be stored as an
/// annotation's `/AP /N` (or `/RO` redaction overlay) stream.
///
/// Coordinates inside `content` use local `[0, 0, width, height]` Form space
/// (`bbox = Rect::new(0.0, 0.0, rect.width(), rect.height())`) so relocating an
/// annotation's `/Rect` without resizing requires no stream rewrite.
///
/// Its `Debug` implementation reports byte lengths rather than raw stream
/// content.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub struct SynthesizedAp {
    /// Form `/BBox` (`[0, 0, width, height]`).
    pub bbox: Rect,
    /// Uncompressed Form content-stream bytes.
    pub content: Vec<u8>,
    /// Form `/Resources` dictionary containing any generated `/ExtGState` or
    /// `/Font` entries.
    pub resources: Dict,
}

impl fmt::Debug for SynthesizedAp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SynthesizedAp")
            .field("bbox", &self.bbox)
            .field("content_len", &self.content.len())
            .field("resource_categories", &self.resources.len())
            .finish()
    }
}

impl SynthesizedAp {
    fn from_canvas(bbox: Rect, canvas: Canvas) -> Self {
        let (content, needs) = canvas.finish(&Reserved::default());
        let resources = build_resources_dict(&needs);
        Self {
            bbox,
            content,
            resources,
        }
    }

    /// Converts this appearance into an uncompressed [`Stream`] with
    /// `/Type /XObject`, `/Subtype /Form`, `/BBox`, and `/Resources`.
    #[must_use]
    pub fn to_stream(&self) -> Stream {
        let mut d = dict! {
            "Type" => Object::name("XObject"),
            "Subtype" => Object::name("Form"),
            "BBox" => vec![
                Object::from(f64::from(self.bbox.x0)),
                Object::from(f64::from(self.bbox.y0)),
                Object::from(f64::from(self.bbox.x1)),
                Object::from(f64::from(self.bbox.y1)),
            ],
        };
        if !self.resources.is_empty() {
            d.set("Resources", self.resources.clone());
        }
        Stream::new(d, self.content.clone())
    }
}

fn build_resources_dict(needs: &ResourceNeeds) -> Dict {
    let mut res = Dict::new();
    for need in &needs.0 {
        let cat_key = need.kind.key();
        let entry_obj = match need.res.body() {
            ResBody::Objects(objs) => match objs.first() {
                Some(first) => first.clone(),
                None => continue,
            },
            ResBody::Written(id) => Object::from(*id),
            _ => continue,
        };
        if let Some(cat_dict) = res.get_mut(cat_key).and_then(Object::as_dict_mut) {
            cat_dict.set(need.name.clone(), entry_obj);
        } else {
            let mut cat_dict = Dict::new();
            cat_dict.set(need.name.clone(), entry_obj);
            res.set(cat_key, cat_dict);
        }
    }
    res
}

fn apply_opacity(canvas: &mut Canvas, stroke_alpha: f32, fill_alpha: f32, multiply: bool) {
    let sa = stroke_alpha.clamp(0.0, 1.0);
    let fa = fill_alpha.clamp(0.0, 1.0);
    if !multiply && (sa - 1.0).abs() <= 1e-6 && (fa - 1.0).abs() <= 1e-6 {
        return;
    }
    let mut gs = GState::default();
    if (sa - 1.0).abs() > 1e-6 {
        gs.stroke_alpha = Some(sa);
    }
    if (fa - 1.0).abs() > 1e-6 {
        gs.fill_alpha = Some(fa);
    }
    if multiply {
        gs.blend = Some(Blend::Multiply);
    }
    canvas.gstate(gs);
}

/// Sets the line width and, for a dashed border, the dash array of a stroked
/// path: `[3]` where the border has none (ISO 32000-2, 12.5.4, Table 168).
fn apply_border_stroke(canvas: &mut Canvas, border: &Border) {
    canvas.line_width(border.width.max(0.0));
    if border.style == BorderStyle::Dashed {
        if border.dash.is_empty() {
            canvas.dash(&[3.0], 0.0);
        } else {
            canvas.dash(&border.dash, 0.0);
        }
    }
}

/// Synthesizes `/AP /N` for `Highlight`, `Underline`, `Squiggly`, or
/// `StrikeOut` annotations from `quads`.
#[must_use]
pub fn synth_text_markup(
    kind: TextMarkupKind,
    rect: Rect,
    quads: &[QuadPoints],
    color: AnnotColor,
    opacity: f32,
) -> Option<SynthesizedAp> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let bbox = Rect::new(0.0, 0.0, rect.width(), rect.height());
    let mut canvas = Canvas::new();
    let Some(c) = color.to_content_color() else {
        return Some(SynthesizedAp::from_canvas(bbox, canvas));
    };

    let shift = |p: Point| Point::new(p.x - rect.x0, p.y - rect.y0);

    canvas.with_state(|cv| {
        let is_hl = kind == TextMarkupKind::Highlight;
        apply_opacity(cv, opacity, opacity, is_hl);
        cv.fill_color(c);
        cv.stroke_color(c);

        for qp in quads {
            let [bl, br, tr, tl] = qp.to_quad().0.map(shift);
            let h = ((tl.y - bl.y).hypot(tl.x - bl.x)).max(1.0);
            match kind {
                TextMarkupKind::Highlight => {
                    cv.polyline(&[bl, br, tr, tl], true, Paint::Fill);
                }
                TextMarkupKind::Underline => {
                    let up = (h * 0.08).clamp(0.5, 2.0);
                    let nx = (tl.x - bl.x) / h;
                    let ny = (tl.y - bl.y) / h;
                    cv.line_width((h * 0.075).clamp(0.6, 2.5));
                    cv.line(
                        Point::new(bl.x + nx * up, bl.y + ny * up),
                        Point::new(br.x + nx * up, br.y + ny * up),
                    );
                }
                TextMarkupKind::StrikeOut => {
                    let frac = 0.42;
                    let p0 = Point::new(bl.x + (tl.x - bl.x) * frac, bl.y + (tl.y - bl.y) * frac);
                    let p1 = Point::new(br.x + (tr.x - br.x) * frac, br.y + (tr.y - br.y) * frac);
                    cv.line_width((h * 0.075).clamp(0.6, 2.5));
                    cv.line(p0, p1);
                }
                TextMarkupKind::Squiggly => {
                    let len = (br.x - bl.x).hypot(br.y - bl.y);
                    if len <= 0.5 {
                        continue;
                    }
                    let step = (h * 0.18).clamp(1.5, 4.0);
                    let amp = (h * 0.10).clamp(0.6, 2.2);
                    let ux = (br.x - bl.x) / len;
                    let uy = (br.y - bl.y) / len;
                    let nx = (tl.x - bl.x) / h;
                    let ny = (tl.y - bl.y) / h;
                    let count = ((len / step).ceil() as usize).clamp(2, 512);
                    let mut pts = Vec::with_capacity(count + 1);
                    for i in 0..=count {
                        let t = (i as f32 / count as f32) * len;
                        let off = if i % 2 == 0 { 0.3 * amp } else { 1.3 * amp };
                        pts.push(Point::new(
                            bl.x + ux * t + nx * off,
                            bl.y + uy * t + ny * off,
                        ));
                    }
                    cv.line_width((h * 0.06).clamp(0.5, 1.8));
                    cv.line_join(LineJoin::Round);
                    cv.polyline(&pts, false, Paint::Stroke);
                }
            }
        }
    });

    Some(SynthesizedAp::from_canvas(bbox, canvas))
}

/// Synthesizes `/AP /N` for `/Square` or `/Circle` annotations.
///
/// A plain `/Square` (no corner radius, no cloudy effect) fills its box, less
/// `rect_diff`, and draws its border inside it in any of the five border
/// styles (`pdf_gfx::Canvas::box_border`). A `/Circle`, and a `/Square` with
/// corner radii or a cloudy effect, is one path stroked along the border's
/// center line, solid or dashed: the beveled, inset, and underline styles are
/// a rectangle's, and come out solid there.
#[must_use]
pub fn synth_shape(
    kind: ShapeKind,
    rect: Rect,
    style: &PaintStyle,
    rect_diff: Option<[f32; 4]>,
) -> Option<SynthesizedAp> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let bbox = Rect::new(0.0, 0.0, rect.width(), rect.height());
    let [dl, db, dr, dt] = rect_diff.unwrap_or([0.0; 4]);
    let sc = style
        .stroke
        .and_then(AnnotColor::to_content_color)
        .filter(|_| style.border.width > 0.0);
    let fc = style.fill.and_then(AnnotColor::to_content_color);
    let mut canvas = Canvas::new();

    let radius = style
        .border
        .horizontal_radius
        .min(style.border.vertical_radius);
    let plain_square = kind == ShapeKind::Square
        && radius.max(0.0) == 0.0
        && style.border.cloudy_intensity.is_none();
    if plain_square {
        let outer = Rect::new(
            dl.min(bbox.x1),
            db.min(bbox.y1),
            (bbox.x1 - dr).max(0.0),
            (bbox.y1 - dt).max(0.0),
        );
        if outer.width() <= 0.0 || outer.height() <= 0.0 || (fc.is_none() && sc.is_none()) {
            return Some(SynthesizedAp::from_canvas(bbox, canvas));
        }
        canvas.with_state(|cv| {
            apply_opacity(
                cv,
                style.opacity,
                style.fill_opacity.unwrap_or(style.opacity),
                false,
            );
            if let Some(c) = fc {
                cv.fill_color(c);
                cv.fill_rect(outer);
            }
            if let Some(c) = sc {
                cv.box_border(outer, &style.border.to_box_border(), c, fc);
            }
        });
        return Some(SynthesizedAp::from_canvas(bbox, canvas));
    }

    let half_w = if sc.is_some() {
        style.border.width / 2.0
    } else {
        0.0
    };
    let inner = Rect::new(
        (dl + half_w).min(bbox.x1),
        (db + half_w).min(bbox.y1),
        (bbox.x1 - dr - half_w).max(0.0),
        (bbox.y1 - dt - half_w).max(0.0),
    );
    if inner.width() <= 0.0 || inner.height() <= 0.0 {
        return Some(SynthesizedAp::from_canvas(bbox, canvas));
    }

    let paint = match (fc, sc) {
        (Some(_), Some(_)) => Paint::FillStroke,
        (Some(_), None) => Paint::Fill,
        (None, Some(_)) => Paint::Stroke,
        (None, None) => return Some(SynthesizedAp::from_canvas(bbox, canvas)),
    };

    canvas.with_state(|cv| {
        apply_opacity(
            cv,
            style.opacity,
            style.fill_opacity.unwrap_or(style.opacity),
            false,
        );
        if let Some(c) = sc {
            cv.stroke_color(c);
            apply_border_stroke(cv, &style.border);
        }
        if let Some(c) = fc {
            cv.fill_color(c);
        }
        match kind {
            ShapeKind::Square => cv.rounded_rect(inner, radius, paint),
            ShapeKind::Circle => cv.ellipse(inner, paint),
        }
    });

    Some(SynthesizedAp::from_canvas(bbox, canvas))
}

/// Synthesizes `/AP /N` for `/Line` annotations, including start/end
/// [`LineEnding`] arrowheads or markers.
#[must_use]
pub fn synth_line(
    rect: Rect,
    start: Point,
    end: Point,
    endings: (LineEnding, LineEnding),
    style: &PaintStyle,
) -> Option<SynthesizedAp> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let bbox = Rect::new(0.0, 0.0, rect.width(), rect.height());
    let mut canvas = Canvas::new();
    let Some(sc) = style.stroke.and_then(AnnotColor::to_content_color) else {
        return Some(SynthesizedAp::from_canvas(bbox, canvas));
    };
    let p0 = Point::new(start.x - rect.x0, start.y - rect.y0);
    let p1 = Point::new(end.x - rect.x0, end.y - rect.y0);
    let w = style.border.width.max(0.5);

    canvas.with_state(|cv| {
        apply_opacity(
            cv,
            style.opacity,
            style.fill_opacity.unwrap_or(style.opacity),
            false,
        );
        cv.stroke_color(sc);
        if let Some(ic) = style.fill.and_then(AnnotColor::to_content_color) {
            cv.fill_color(ic);
        } else {
            cv.fill_color(sc);
        }
        apply_border_stroke(cv, &style.border);
        cv.line_width(w);
        cv.line(p0, p1);

        let dx = p1.x - p0.x;
        let dy = p1.y - p0.y;
        let len = dx.hypot(dy);
        if len > 1e-4 {
            let ux = dx / len;
            let uy = dy / len;
            cv.dash(&[], 0.0);
            let has_ic = style.fill.and_then(AnnotColor::to_content_color).is_some();
            draw_line_ending(cv, p0, Point::new(-ux, -uy), endings.0, w, has_ic);
            draw_line_ending(cv, p1, Point::new(ux, uy), endings.1, w, has_ic);
        }
    });

    Some(SynthesizedAp::from_canvas(bbox, canvas))
}

fn draw_line_ending(
    cv: &mut Canvas,
    tip: Point,
    dir: Point,
    ending: LineEnding,
    width: f32,
    has_fill: bool,
) {
    let size = (width * 5.0).max(4.0);
    let nx = -dir.y;
    let ny = dir.x;
    let closed_paint = if has_fill {
        Paint::FillStroke
    } else {
        Paint::Stroke
    };
    match ending {
        LineEnding::None => {}
        LineEnding::OpenArrow => {
            let b1 = Point::new(
                tip.x - dir.x * size + nx * size * 0.55,
                tip.y - dir.y * size + ny * size * 0.55,
            );
            let b2 = Point::new(
                tip.x - dir.x * size - nx * size * 0.55,
                tip.y - dir.y * size - ny * size * 0.55,
            );
            cv.polyline(&[b1, tip, b2], false, Paint::Stroke);
        }
        LineEnding::ClosedArrow => {
            let b1 = Point::new(
                tip.x - dir.x * size + nx * size * 0.55,
                tip.y - dir.y * size + ny * size * 0.55,
            );
            let b2 = Point::new(
                tip.x - dir.x * size - nx * size * 0.55,
                tip.y - dir.y * size - ny * size * 0.55,
            );
            cv.polyline(&[b1, tip, b2], true, closed_paint);
        }
        LineEnding::ROpenArrow => {
            let b1 = Point::new(
                tip.x + dir.x * size + nx * size * 0.55,
                tip.y + dir.y * size + ny * size * 0.55,
            );
            let b2 = Point::new(
                tip.x + dir.x * size - nx * size * 0.55,
                tip.y + dir.y * size - ny * size * 0.55,
            );
            cv.polyline(&[b1, tip, b2], false, Paint::Stroke);
        }
        LineEnding::RClosedArrow => {
            let b1 = Point::new(
                tip.x + dir.x * size + nx * size * 0.55,
                tip.y + dir.y * size + ny * size * 0.55,
            );
            let b2 = Point::new(
                tip.x + dir.x * size - nx * size * 0.55,
                tip.y + dir.y * size - ny * size * 0.55,
            );
            cv.polyline(&[b1, tip, b2], true, closed_paint);
        }
        LineEnding::Circle => {
            cv.circle(tip, size * 0.45, closed_paint);
        }
        LineEnding::Square => {
            let r = size * 0.45;
            cv.paint_rect(
                Rect::new(tip.x - r, tip.y - r, tip.x + r, tip.y + r),
                closed_paint,
            );
        }
        LineEnding::Diamond => {
            let r = size * 0.55;
            cv.polyline(
                &[
                    Point::new(tip.x + dir.x * r, tip.y + dir.y * r),
                    Point::new(tip.x + nx * r, tip.y + ny * r),
                    Point::new(tip.x - dir.x * r, tip.y - dir.y * r),
                    Point::new(tip.x - nx * r, tip.y - ny * r),
                ],
                true,
                closed_paint,
            );
        }
        LineEnding::Butt => {
            let r = size * 0.5;
            cv.line(
                Point::new(tip.x + nx * r, tip.y + ny * r),
                Point::new(tip.x - nx * r, tip.y - ny * r),
            );
        }
        LineEnding::Slash => {
            let r = size * 0.55;
            cv.line(
                Point::new(
                    tip.x + (nx + dir.x * 0.5) * r,
                    tip.y + (ny + dir.y * 0.5) * r,
                ),
                Point::new(
                    tip.x - (nx + dir.x * 0.5) * r,
                    tip.y - (ny + dir.y * 0.5) * r,
                ),
            );
        }
    }
}

/// Synthesizes `/AP /N` for `/Polygon` or `/PolyLine` annotations.
#[must_use]
pub fn synth_poly(
    kind: PolyKind,
    rect: Rect,
    vertices: &[Point],
    endings: (LineEnding, LineEnding),
    style: &PaintStyle,
) -> Option<SynthesizedAp> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 || vertices.len() < 2 {
        return None;
    }
    let bbox = Rect::new(0.0, 0.0, rect.width(), rect.height());
    let mut canvas = Canvas::new();
    let local: Vec<Point> = vertices
        .iter()
        .map(|p| Point::new(p.x - rect.x0, p.y - rect.y0))
        .collect();
    let closed = kind == PolyKind::Polygon;
    let sc = style
        .stroke
        .and_then(AnnotColor::to_content_color)
        .filter(|_| style.border.width > 0.0);
    let fc = if closed {
        style.fill.and_then(AnnotColor::to_content_color)
    } else {
        None
    };
    let paint = match (fc, sc) {
        (Some(_), Some(_)) => Paint::FillStroke,
        (Some(_), None) => Paint::Fill,
        (None, Some(_)) => Paint::Stroke,
        (None, None) => return Some(SynthesizedAp::from_canvas(bbox, canvas)),
    };

    canvas.with_state(|cv| {
        apply_opacity(
            cv,
            style.opacity,
            style.fill_opacity.unwrap_or(style.opacity),
            false,
        );
        if let Some(c) = sc {
            cv.stroke_color(c);
            apply_border_stroke(cv, &style.border);
        }
        if let Some(c) = fc {
            cv.fill_color(c);
        }
        cv.polyline(&local, closed, paint);

        if kind == PolyKind::PolyLine
            && sc.is_some()
            && let (Some(&first), Some(&second), Some(&penult), Some(&last)) = (
                local.first(),
                local.get(1),
                local.get(local.len().wrapping_sub(2)),
                local.last(),
            )
        {
            cv.dash(&[], 0.0);
            let has_ic = if let Some(ic) = style.fill.and_then(AnnotColor::to_content_color) {
                cv.fill_color(ic);
                true
            } else {
                false
            };
            let w = style.border.width.max(0.5);
            let d0 = (first.x - second.x).hypot(first.y - second.y);
            if d0 > 1e-4 {
                draw_line_ending(
                    cv,
                    first,
                    Point::new((first.x - second.x) / d0, (first.y - second.y) / d0),
                    endings.0,
                    w,
                    has_ic,
                );
            }
            let d1 = (last.x - penult.x).hypot(last.y - penult.y);
            if d1 > 1e-4 {
                draw_line_ending(
                    cv,
                    last,
                    Point::new((last.x - penult.x) / d1, (last.y - penult.y) / d1),
                    endings.1,
                    w,
                    has_ic,
                );
            }
        }
    });

    Some(SynthesizedAp::from_canvas(bbox, canvas))
}

/// Synthesizes `/AP /N` for `/Ink` freehand annotations.
#[must_use]
pub fn synth_ink(
    rect: Rect,
    strokes: &[Vec<Point>],
    color: AnnotColor,
    border: &Border,
    opacity: f32,
) -> Option<SynthesizedAp> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let bbox = Rect::new(0.0, 0.0, rect.width(), rect.height());
    let mut canvas = Canvas::new();
    let Some(c) = color.to_content_color() else {
        return Some(SynthesizedAp::from_canvas(bbox, canvas));
    };

    canvas.with_state(|cv| {
        apply_opacity(cv, opacity, opacity, false);
        cv.stroke_color(c);
        apply_border_stroke(cv, border);
        cv.line_cap(LineCap::Round);
        cv.line_join(LineJoin::Round);
        for stroke in strokes {
            let pts: Vec<Point> = stroke
                .iter()
                .map(|p| Point::new(p.x - rect.x0, p.y - rect.y0))
                .collect();
            if pts.len() == 1 {
                cv.fill_color(c);
                cv.circle(pts[0], (border.width * 0.5).max(0.5), Paint::Fill);
            } else if pts.len() >= 2 {
                cv.polyline(&pts, false, Paint::Stroke);
            }
        }
    });

    Some(SynthesizedAp::from_canvas(bbox, canvas))
}

/// Synthesizes `/AP /N` for standard or custom `/Stamp` rubber-stamp banners.
#[must_use]
pub fn synth_stamp(
    rect: Rect,
    stamp: &StampName,
    custom_label: Option<&str>,
    color: AnnotColor,
    opacity: f32,
) -> Option<SynthesizedAp> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let bbox = Rect::new(0.0, 0.0, rect.width(), rect.height());
    let mut canvas = Canvas::new();
    let c = color
        .to_content_color()
        .unwrap_or(Color::Rgb([0.75, 0.1, 0.1]));
    let fallback_text = String::from_utf8_lossy(stamp.as_bytes()).to_uppercase();
    let label = custom_label
        .or_else(|| stamp.display_label())
        .unwrap_or(&fallback_text);

    let w = bbox.width();
    let h = bbox.height();
    let bw = (h * 0.06).clamp(1.0, 3.0);
    let Some(outer) = bbox.inset(bw, bw) else {
        return Some(SynthesizedAp::from_canvas(bbox, canvas));
    };

    canvas.with_state(|cv| {
        apply_opacity(cv, opacity, opacity, false);
        cv.stroke_color(c);
        cv.fill_color(c);
        cv.line_width(bw);
        cv.rounded_rect(outer, (h * 0.14).min(6.0), Paint::Stroke);

        let codes = winansi_lossy(label);
        if !codes.is_empty()
            && let Ok(font_res) = Res::dict(Std14::HelveticaBold.dict())
        {
            let unit_w: f32 = codes
                .iter()
                .map(|&b| std14_code_width(Std14::HelveticaBold, b) / 1000.0)
                .sum();
            let avail_w = (outer.width() - bw * 4.0).max(4.0);
            let avail_h = (outer.height() - bw * 3.0).max(4.0);
            let size = if unit_w > 0.0 {
                (avail_w / unit_w).min(avail_h * 0.65).max(4.0)
            } else {
                avail_h * 0.5
            };
            let text_w = unit_w * size;
            let x = outer.x0 + (outer.width() - text_w) * 0.5;
            let y = outer.y0 + (outer.height() - size * 0.72) * 0.5;
            cv.text_at(font_res, size, Point::new(x, y), &codes);
        }
    });

    let _ = w;
    Some(SynthesizedAp::from_canvas(bbox, canvas))
}

/// Synthesizes `/AP /N` for `/Text` sticky-note icons.
#[must_use]
pub fn synth_text_note(
    rect: Rect,
    icon: &TextIcon,
    color: AnnotColor,
    opacity: f32,
) -> Option<SynthesizedAp> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let bbox = Rect::new(0.0, 0.0, rect.width(), rect.height());
    let mut canvas = Canvas::new();
    let fill = color
        .to_content_color()
        .unwrap_or(Color::Rgb([1.0, 0.92, 0.35]));
    let stroke = Color::Gray(0.2);

    canvas.with_state(|cv| {
        apply_opacity(cv, opacity, opacity, false);
        cv.fill_color(fill);
        cv.stroke_color(stroke);
        cv.line_width(1.0);
        let pad = (bbox.width().min(bbox.height()) * 0.08).max(0.5);
        let Some(body) = bbox.inset(pad, pad) else {
            return;
        };
        match icon {
            TextIcon::Comment => {
                cv.rounded_rect(body, body.height() * 0.25, Paint::FillStroke);
            }
            _ => {
                cv.rounded_rect(body, 2.0, Paint::FillStroke);
                // Three horizontal ruling lines inside the note icon.
                let lx0 = body.x0 + body.width() * 0.2;
                let lx1 = body.x1 - body.width() * 0.2;
                for frac in [0.32_f32, 0.52, 0.72] {
                    let ly = body.y0 + body.height() * frac;
                    cv.line(Point::new(lx0, ly), Point::new(lx1, ly));
                }
            }
        }
    });

    Some(SynthesizedAp::from_canvas(bbox, canvas))
}

/// Synthesizes `/AP /N` for `/FreeText` box annotations: the background, the
/// border inside the box in any of the five border styles
/// (`pdf_gfx::Canvas::box_border`, the background its bevel's hint), and the
/// text, padded to clear the border.
#[must_use]
pub fn synth_free_text(
    rect: Rect,
    contents: &str,
    da: Option<&DefaultAppearance>,
    quadding: Quadding,
    style: &PaintStyle,
) -> Option<SynthesizedAp> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let bbox = Rect::new(0.0, 0.0, rect.width(), rect.height());
    let mut canvas = Canvas::new();

    let text_color = da.and_then(|d| d.color).unwrap_or(Color::Gray(0.0));
    let font_size = da
        .and_then(|d| d.font_size)
        .filter(|&s| s > 0.0)
        .unwrap_or(12.0);

    canvas.with_state(|cv| {
        apply_opacity(
            cv,
            style.opacity,
            style.fill_opacity.unwrap_or(style.opacity),
            false,
        );
        let background = style.fill.and_then(AnnotColor::to_content_color);
        if let Some(bg) = background {
            cv.fill_color(bg);
            cv.fill_rect(bbox);
        }
        let border = style.border.to_box_border();
        if let Some(bc) = style.stroke.and_then(AnnotColor::to_content_color) {
            cv.box_border(bbox, &border, bc, background);
        }
        // The text clears the border (a bevel's band too), whether or not a
        // stroke color draws it.
        let pad = (border.inset() + 2.0).max(2.0);
        let Some(inner) = bbox.inset(pad, pad) else {
            return;
        };
        draw_box_text(cv, inner, contents, font_size, quadding, text_color);
    });

    Some(SynthesizedAp::from_canvas(bbox, canvas))
}

/// What a `/Redact` mark's appearances are drawn from (ISO 32000-2,
/// 12.5.6.23): the regions to redact and how the mark and its overlay look.
///
/// It borrows what it holds and is `Copy`, so a mark read from a file is
/// drawn without cloning its quadrilaterals or its text. Built with
/// [`RedactMark::new`] and the `with_*` methods.
///
/// Its `Debug` implementation reports the quadrilaterals' count and the
/// overlay text's length, never the text.
#[derive(Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct RedactMark<'a> {
    /// `/QuadPoints`: the regions to redact, in the page's default user
    /// space. Empty: the whole `/Rect`.
    pub quads: &'a [QuadPoints],
    /// `/C`: the color of the mark's outline and wash before it's applied (a
    /// red where `None`).
    pub stroke_color: Option<AnnotColor>,
    /// `/IC`: the color that fills each region once the mark is applied.
    pub interior_color: Option<AnnotColor>,
    /// `/OverlayText`: the label drawn over each region once the mark is
    /// applied.
    pub overlay_text: Option<&'a str>,
    /// `/Repeat`: whether the overlay text repeats across each region.
    pub repeat: bool,
    /// `/DA`: the overlay text's color and size.
    pub da: Option<&'a DefaultAppearance>,
    /// `/Q`: the overlay text's justification.
    pub quadding: Quadding,
}

impl fmt::Debug for RedactMark<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedactMark")
            .field("quads", &self.quads.len())
            .field("stroke_color", &self.stroke_color)
            .field("interior_color", &self.interior_color)
            .field("overlay_text_len", &self.overlay_text.map(str::len))
            .field("repeat", &self.repeat)
            .field("da", &self.da)
            .field("quadding", &self.quadding)
            .finish()
    }
}

impl Default for RedactMark<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> RedactMark<'a> {
    /// A mark over its whole `/Rect`: no quadrilaterals, colors, or text.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            quads: &[],
            stroke_color: None,
            interior_color: None,
            overlay_text: None,
            repeat: false,
            da: None,
            quadding: Quadding::Left,
        }
    }

    /// Sets the regions to redact.
    #[must_use]
    pub const fn with_quads(mut self, quads: &'a [QuadPoints]) -> Self {
        self.quads = quads;
        self
    }

    /// Sets the color of the mark's outline and wash.
    #[must_use]
    pub const fn with_stroke_color(mut self, color: AnnotColor) -> Self {
        self.stroke_color = Some(color);
        self
    }

    /// Sets the color that fills each region once the mark is applied.
    #[must_use]
    pub const fn with_interior_color(mut self, color: AnnotColor) -> Self {
        self.interior_color = Some(color);
        self
    }

    /// Sets the label drawn over each region once the mark is applied.
    #[must_use]
    pub const fn with_overlay_text(mut self, text: &'a str) -> Self {
        self.overlay_text = Some(text);
        self
    }

    /// Sets whether the overlay text repeats across each region.
    #[must_use]
    pub const fn with_repeat(mut self, repeat: bool) -> Self {
        self.repeat = repeat;
        self
    }

    /// Sets the overlay text's color and size.
    #[must_use]
    pub const fn with_default_appearance(mut self, da: &'a DefaultAppearance) -> Self {
        self.da = Some(da);
        self
    }

    /// Sets the overlay text's justification.
    #[must_use]
    pub const fn with_quadding(mut self, quadding: Quadding) -> Self {
        self.quadding = quadding;
        self
    }

    /// The smallest rectangle that holds every quadrilateral, one with a
    /// number that isn't finite left out; `None` where none is left.
    #[must_use]
    pub fn bounds(&self) -> Option<Rect> {
        self.quads
            .iter()
            .filter(|q| q.0.iter().all(|v| v.is_finite()))
            .map(|q| q.bounds())
            .reduce(Rect::union)
    }

    /// The quadrilaterals that can be drawn, each in perimeter order and
    /// moved into the form space of an annotation whose `/Rect` is `rect`:
    /// one that isn't finite there is left out.
    fn local_quads(&self, rect: Rect) -> impl Iterator<Item = Quad> + '_ {
        self.quads
            .iter()
            .map(move |q| {
                Quad(
                    q.to_quad()
                        .0
                        .map(|p| Point::new(p.x - rect.x0, p.y - rect.y0)),
                )
            })
            .filter(Quad::is_finite)
    }
}

/// The rectangle `quad` is, where its sides run along the axes.
fn quad_as_rect(quad: &Quad) -> Option<Rect> {
    let [a, b, c, d] = quad.0;
    let level = |p: Point, q: Point| p.y == q.y;
    let upright = |p: Point, q: Point| p.x == q.x;
    let along = (level(a, b) && upright(b, c) && level(c, d) && upright(d, a))
        || (upright(a, b) && level(b, c) && upright(c, d) && level(d, a));
    along.then(|| quad.bounds())
}

/// Synthesizes `/AP /N` (the mark before it's applied) and `/RO` (the overlay
/// Form XObject that applying it leaves) for a `/Redact` annotation whose
/// `/Rect` is `rect`.
///
/// With no quadrilateral in `mark` (or none whose numbers are all finite),
/// both cover the whole box. Otherwise each quadrilateral, moved into the
/// form's space, is drawn alone:
///
/// - `/AP /N`: a translucent wash over the quadrilateral and its outline, at
///   a width of `1.0`: inside the quadrilateral where it's a rectangle along
///   the axes (as `pdf_gfx::Canvas::border_rect` draws it, so an edge on the
///   form's `/BBox` isn't cut in half), along its edges otherwise.
/// - `/RO`: the interior color over the quadrilateral, then the overlay text
///   laid out, unrotated, in the quadrilateral's bounding box, repeated there
///   under [`RedactMark::repeat`].
///
/// The overlay holds the text once for each quadrilateral (more under
/// `repeat`): its size grows with the two together. Returns `None` where
/// `rect` has no width or no height.
#[must_use]
pub fn synth_redact(rect: Rect, mark: &RedactMark<'_>) -> Option<(SynthesizedAp, SynthesizedAp)> {
    Some((redact_mark_ap(rect, mark)?, redact_overlay_ap(rect, mark)?))
}

/// `/AP /N` of a `/Redact`: a crisp outline and a faint translucent wash over
/// each region.
fn redact_mark_ap(rect: Rect, mark: &RedactMark<'_>) -> Option<SynthesizedAp> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let bbox = Rect::new(0.0, 0.0, rect.width(), rect.height());
    let whole = mark.local_quads(rect).next().is_none();

    let mut mark_cv = Canvas::new();
    let outline = mark
        .stroke_color
        .and_then(AnnotColor::to_content_color)
        .unwrap_or(Color::Rgb([0.85, 0.1, 0.1]));
    mark_cv.with_state(|cv| {
        let mut gs = GState::default();
        gs.fill_alpha = Some(0.15);
        cv.gstate(gs);
        cv.fill_color(outline);
        if whole {
            cv.fill_rect(bbox);
        }
        for quad in mark.local_quads(rect) {
            match quad_as_rect(&quad) {
                Some(r) => cv.fill_rect(r),
                None => cv.polyline(&quad.0, true, Paint::Fill),
            }
        }
    });
    mark_cv.with_state(|cv| {
        cv.stroke_color(outline);
        if whole {
            cv.border_rect(bbox, 1.0);
        }
        for quad in mark.local_quads(rect) {
            match quad_as_rect(&quad) {
                Some(r) => cv.border_rect(r, 1.0),
                None => cv.with_state(|c| {
                    c.line_width(1.0);
                    c.polyline(&quad.0, true, Paint::Stroke);
                }),
            }
        }
    });
    Some(SynthesizedAp::from_canvas(bbox, mark_cv))
}

/// `/RO` of a `/Redact`: the opaque interior fill of each region, then the
/// `/OverlayText` in each.
fn redact_overlay_ap(rect: Rect, mark: &RedactMark<'_>) -> Option<SynthesizedAp> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let bbox = Rect::new(0.0, 0.0, rect.width(), rect.height());
    let whole = mark.local_quads(rect).next().is_none();

    let mut ro_cv = Canvas::new();
    ro_cv.with_state(|cv| {
        let fill = mark
            .interior_color
            .and_then(AnnotColor::to_content_color)
            .unwrap_or(Color::Gray(0.0));
        cv.fill_color(fill);
        if whole {
            cv.fill_rect(bbox);
        }
        // Every fill before any text: the text sets its own fill color, and
        // no region's fill then covers another's label.
        for quad in mark.local_quads(rect) {
            match quad_as_rect(&quad) {
                Some(r) => cv.fill_rect(r),
                None => cv.polyline(&quad.0, true, Paint::Fill),
            }
        }

        if let Some(txt) = mark.overlay_text.filter(|t| !t.is_empty()) {
            if whole {
                draw_overlay_text(cv, bbox, txt, mark);
            }
            for quad in mark.local_quads(rect) {
                draw_overlay_text(cv, quad.bounds(), txt, mark);
            }
        }
    });
    Some(SynthesizedAp::from_canvas(bbox, ro_cv))
}

/// Draws a redaction's overlay text in `region`, repeated across it under
/// `/Repeat`.
fn draw_overlay_text(cv: &mut Canvas, region: Rect, txt: &str, mark: &RedactMark<'_>) {
    let text_color = mark.da.and_then(|d| d.color).unwrap_or(Color::Gray(1.0));
    let font_size = mark
        .da
        .and_then(|d| d.font_size)
        .filter(|&s| s > 0.0)
        .unwrap_or_else(|| (region.height() * 0.55).clamp(6.0, 14.0));
    let shown = if mark.repeat {
        let reps = ((region.width() / (font_size * 3.0)).ceil() as usize).clamp(2, 32);
        vec![txt; reps].join(" ")
    } else {
        txt.to_string()
    };
    if let Some(inner) = region.inset(2.0, 1.0) {
        draw_box_text(cv, inner, &shown, font_size, mark.quadding, text_color);
    }
}

#[cfg(feature = "text-layout")]
fn draw_box_text(
    cv: &mut Canvas,
    box_rect: Rect,
    text: &str,
    font_size: f32,
    quadding: Quadding,
    color: Color,
) {
    use pdf_gfx::{TextAlign, TextBoxStyle, WrapMode};

    if text.is_empty() {
        return;
    }
    let Ok(font_res) = Res::dict(Std14::Helvetica.dict()) else {
        return;
    };
    let align = match quadding {
        Quadding::Left => TextAlign::Left,
        Quadding::Center => TextAlign::Center,
        Quadding::Right => TextAlign::Right,
    };
    let style = TextBoxStyle {
        size: Some(font_size),
        align,
        wrap: WrapMode::Word,
        clip: true,
        ..TextBoxStyle::default()
    };
    cv.fill_color(color);
    let _ = cv.text_box(font_res, &Std14::Helvetica, text, box_rect, &style);
}

#[cfg(not(feature = "text-layout"))]
fn draw_box_text(
    cv: &mut Canvas,
    box_rect: Rect,
    text: &str,
    font_size: f32,
    quadding: Quadding,
    color: Color,
) {
    if text.is_empty() {
        return;
    }
    let Ok(font_res) = Res::dict(Std14::Helvetica.dict()) else {
        return;
    };
    cv.with_state(|c| {
        c.clip_rect(box_rect);
        c.fill_color(color);
        let line_h = font_size * 1.2;
        let mut y = box_rect.y1 - font_size;
        for line in text.lines() {
            if y < box_rect.y0 - font_size {
                break;
            }
            let codes = winansi_lossy(line);
            let w: f32 = codes
                .iter()
                .map(|&b| std14_code_width(Std14::Helvetica, b) / 1000.0 * font_size)
                .sum();
            let x = match quadding {
                Quadding::Left => box_rect.x0,
                Quadding::Center => box_rect.x0 + (box_rect.width() - w) * 0.5,
                Quadding::Right => box_rect.x1 - w,
            };
            c.text_at(font_res.clone(), font_size, Point::new(x, y), &codes);
            y -= line_h;
        }
    });
}

/// Synthesizes a normal appearance (`/AP /N`) directly from an existing decoded
/// [`Annot`], returning `None` for non-visual subtypes (`Link`, `Popup`) or
/// degenerate rectangles.
#[must_use]
pub fn synth_annot(annot: &Annot<'_>) -> Option<SynthesizedAp> {
    if !annot.has_valid_rect() {
        return None;
    }
    let rect = annot.rect();
    let opacity = annot.markup().map_or(1.0, |m| m.opacity);
    let fill_opacity = annot.markup().and_then(|m| m.fill_opacity);
    match annot.typed() {
        TypedAnnot::Text(t) => synth_text_note(
            rect,
            &t.icon,
            annot.color().unwrap_or(AnnotColor::Rgb([1.0, 0.92, 0.35])),
            opacity,
        ),
        TypedAnnot::FreeText(ft) => synth_free_text(
            rect,
            annot.contents().unwrap_or(""),
            ft.da.as_ref(),
            ft.quadding,
            &PaintStyle {
                stroke: annot.color(),
                fill: annot.color(),
                border: annot.border().clone(),
                opacity,
                fill_opacity,
            },
        ),
        TypedAnnot::TextMarkup(tm) => synth_text_markup(
            tm.kind,
            rect,
            &tm.quad_points,
            annot.color().unwrap_or(AnnotColor::Rgb([1.0, 0.92, 0.2])),
            opacity,
        ),
        TypedAnnot::Line(ln) => {
            let (start, end) = ln.endpoints?;
            synth_line(
                rect,
                start,
                end,
                ln.endings,
                &PaintStyle {
                    stroke: Some(annot.color().unwrap_or(AnnotColor::Gray(0.0))),
                    fill: ln.interior_color,
                    border: annot.border().clone(),
                    opacity,
                    fill_opacity,
                },
            )
        }
        TypedAnnot::Shape(sh) => synth_shape(
            sh.kind,
            rect,
            &PaintStyle {
                stroke: annot.color(),
                fill: sh.interior_color,
                border: annot.border().clone(),
                opacity,
                fill_opacity,
            },
            sh.rect_diff,
        ),
        TypedAnnot::Poly(pl) => synth_poly(
            pl.kind,
            rect,
            &pl.vertices,
            pl.endings,
            &PaintStyle {
                stroke: annot.color(),
                fill: pl.interior_color,
                border: annot.border().clone(),
                opacity,
                fill_opacity,
            },
        ),
        TypedAnnot::Ink(ink) => synth_ink(
            rect,
            &ink.strokes,
            annot.color().unwrap_or(AnnotColor::Gray(0.0)),
            annot.border(),
            opacity,
        ),
        TypedAnnot::Stamp(st) => synth_stamp(
            rect,
            &st.name,
            None,
            annot.color().unwrap_or(AnnotColor::Rgb([0.75, 0.1, 0.1])),
            opacity,
        ),
        // The mark alone: its overlay isn't an appearance of the annotation.
        TypedAnnot::Redact(rd) => redact_mark_ap(
            rect,
            &RedactMark {
                quads: &rd.quad_points,
                stroke_color: annot.color(),
                interior_color: rd.interior_color,
                ..RedactMark::new()
            },
        ),
        _ => None,
    }
}

fn std14_code_width(font: Std14, code: u8) -> f32 {
    Encoding::WinAnsi
        .glyph(code)
        .and_then(|g| font.glyph_width(g))
        .unwrap_or(500.0)
}

fn winansi_lossy(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for ch in s.chars() {
        out.push(Encoding::WinAnsi.code(ch).unwrap_or(b'?'));
    }
    out
}
