//! Deterministic `/AP /N` appearance Form XObject synthesis over
//! [`pdf_gfx::Canvas`] (ISO 32000-2 §12.5.5).

use std::fmt;

use pdf_content_streams::{
    Blend, Color, GState, LineCap, LineJoin, Paint, Res, ResBody, Reserved, ResourceNeeds,
};
use pdf_fonts::{Encoding, Std14};
use pdf_gfx::{Canvas, Point, Rect};
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

fn apply_border_stroke(canvas: &mut Canvas, border: &Border) {
    canvas.line_width(border.width.max(0.0));
    if border.style == BorderStyle::Dashed && !border.dash.is_empty() {
        canvas.dash(&border.dash, 0.0);
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
    let half_w = if style.border.width > 0.0
        && style
            .stroke
            .and_then(AnnotColor::to_content_color)
            .is_some()
    {
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
    let mut canvas = Canvas::new();
    if inner.width() <= 0.0 || inner.height() <= 0.0 {
        return Some(SynthesizedAp::from_canvas(bbox, canvas));
    }

    let sc = style
        .stroke
        .and_then(AnnotColor::to_content_color)
        .filter(|_| style.border.width > 0.0);
    let fc = style.fill.and_then(AnnotColor::to_content_color);
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
            ShapeKind::Square => {
                let rad = style
                    .border
                    .horizontal_radius
                    .min(style.border.vertical_radius);
                cv.rounded_rect(inner, rad, paint);
            }
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

/// Synthesizes `/AP /N` for `/FreeText` box annotations.
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
        if let Some(bg) = style.fill.and_then(AnnotColor::to_content_color) {
            cv.fill_color(bg);
            cv.fill_rect(bbox);
        }
        if style.border.width > 0.0
            && let Some(bc) = style.stroke.and_then(AnnotColor::to_content_color)
        {
            cv.stroke_color(bc);
            cv.border_rect(bbox, style.border.width);
        }
        let pad = (style.border.width + 2.0).max(2.0);
        let Some(inner) = bbox.inset(pad, pad) else {
            return;
        };
        draw_box_text(cv, inner, contents, font_size, quadding, text_color);
    });

    Some(SynthesizedAp::from_canvas(bbox, canvas))
}

/// Synthesizes `/AP /N` (pre-apply redaction mark) and `/RO` (post-apply
/// overlay Form XObject) for a `/Redact` annotation.
#[must_use]
pub fn synth_redact(
    rect: Rect,
    stroke_color: Option<AnnotColor>,
    interior_color: Option<AnnotColor>,
    overlay_text: Option<&str>,
    repeat: bool,
    da: Option<&DefaultAppearance>,
    quadding: Quadding,
) -> Option<(SynthesizedAp, SynthesizedAp)> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let bbox = Rect::new(0.0, 0.0, rect.width(), rect.height());

    // 1. Pre-apply `/AP /N`: crisp redaction outline + faint translucent wash.
    let mut mark_cv = Canvas::new();
    let outline = stroke_color
        .and_then(AnnotColor::to_content_color)
        .unwrap_or(Color::Rgb([0.85, 0.1, 0.1]));
    mark_cv.with_state(|cv| {
        let mut gs = GState::default();
        gs.fill_alpha = Some(0.15);
        cv.gstate(gs);
        cv.fill_color(outline);
        cv.fill_rect(bbox);
    });
    mark_cv.with_state(|cv| {
        cv.stroke_color(outline);
        cv.border_rect(bbox, 1.0);
    });

    // 2. Post-apply `/RO`: opaque interior fill + optional `/OverlayText`.
    let mut ro_cv = Canvas::new();
    ro_cv.with_state(|cv| {
        let fill = interior_color
            .and_then(AnnotColor::to_content_color)
            .unwrap_or(Color::Gray(0.0));
        cv.fill_color(fill);
        cv.fill_rect(bbox);

        if let Some(txt) = overlay_text.filter(|t| !t.is_empty()) {
            let text_color = da.and_then(|d| d.color).unwrap_or(Color::Gray(1.0));
            let font_size = da
                .and_then(|d| d.font_size)
                .filter(|&s| s > 0.0)
                .unwrap_or_else(|| (bbox.height() * 0.55).clamp(6.0, 14.0));
            let shown = if repeat {
                let reps = ((bbox.width() / (font_size * 3.0)).ceil() as usize).clamp(2, 32);
                vec![txt; reps].join(" ")
            } else {
                txt.to_string()
            };
            if let Some(inner) = bbox.inset(2.0, 1.0) {
                draw_box_text(cv, inner, &shown, font_size, quadding, text_color);
            }
        }
    });

    Some((
        SynthesizedAp::from_canvas(bbox, mark_cv),
        SynthesizedAp::from_canvas(bbox, ro_cv),
    ))
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
        TypedAnnot::Redact(rd) => synth_redact(
            rect,
            annot.color(),
            rd.interior_color,
            rd.overlay_text.as_deref(),
            rd.repeat,
            rd.da.as_ref(),
            rd.quadding,
        )
        .map(|(ap, _)| ap),
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
