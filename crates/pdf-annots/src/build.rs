//! In-place annotation dictionary mutators (`AnnotMut`), pure typed annotation
//! builders (`BuiltAnnot`), and page `/Annots` array helpers.

use std::fmt;

use pdf_gfx::{Point, Rect};
use pdf_objects::{Dict, Object, ObjectId, dict};
use pdf_semantics::action::Action;
use pdf_semantics::date::Date;
use pdf_semantics::dest::Dest;

use crate::flags::AnnotFlags;
use crate::replies::ReplyKind;
use crate::resolve::QuadPoints;
use crate::style::{
    AnnotColor, Border, BorderStyle, DefaultAppearance, LineEnding, PaintStyle, Quadding,
};
use crate::subtypes::{AnnotSubtype, PolyKind, ShapeKind, StampName, TextIcon, TextMarkupKind};
use crate::synth::{
    SynthesizedAp, synth_free_text, synth_ink, synth_line, synth_poly, synth_redact, synth_shape,
    synth_stamp, synth_text_markup, synth_text_note,
};

/// Zero-I/O mutable wrapper around an annotation [`Dict`] (standalone
/// annotation or merged Field+Widget dictionary).
///
/// Its `Debug` implementation reports entry counts rather than dictionary keys
/// or plaintext values.
pub struct AnnotMut<'a> {
    dict: &'a mut Dict,
}

impl fmt::Debug for AnnotMut<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnnotMut")
            .field("entries", &self.dict.len())
            .finish()
    }
}

impl<'a> AnnotMut<'a> {
    /// Wraps `dict` for in-place annotation mutation.
    #[must_use]
    pub fn new(dict: &'a mut Dict) -> Self {
        Self { dict }
    }

    /// Underlying mutable [`Dict`].
    pub fn dict_mut(&mut self) -> &mut Dict {
        self.dict
    }

    /// Sets `/Subtype`.
    pub fn set_subtype(&mut self, subtype: &AnnotSubtype) -> &mut Self {
        self.dict.set("Subtype", Object::name(subtype.as_bytes()));
        self
    }

    /// Sets `/Rect` to normalized `rect`.
    pub fn set_rect(&mut self, rect: Rect) -> &mut Self {
        self.dict.set(
            "Rect",
            vec![
                Object::from(f64::from(rect.x0)),
                Object::from(f64::from(rect.y0)),
                Object::from(f64::from(rect.x1)),
                Object::from(f64::from(rect.y1)),
            ],
        );
        self
    }

    /// Sets `/F` to `flags`.
    pub fn set_flags(&mut self, flags: AnnotFlags) -> &mut Self {
        if flags.is_empty() {
            self.dict.remove(b"F");
        } else {
            self.dict.set("F", i64::from(flags.bits() as i32));
        }
        self
    }

    /// Sets `/Contents` text.
    pub fn set_contents(&mut self, contents: &str) -> &mut Self {
        self.dict.set("Contents", Object::text(contents));
        self
    }

    /// Sets `/NM` unique page-scoped annotation identifier.
    pub fn set_name(&mut self, name: &str) -> &mut Self {
        self.dict.set("NM", Object::text(name));
        self
    }

    /// Sets `/M` modification date to `date.write()`.
    pub fn set_modified(&mut self, date: &Date) -> &mut Self {
        self.dict.set("M", Object::text(&date.write()));
        self
    }

    /// Sets `/C` color.
    pub fn set_color(&mut self, color: AnnotColor) -> &mut Self {
        self.dict.set("C", color.to_object());
        self
    }

    /// Sets `/IC` interior color.
    pub fn set_interior_color(&mut self, color: AnnotColor) -> &mut Self {
        self.dict.set("IC", color.to_object());
        self
    }

    /// Sets `/BS` border style dictionary (and removes legacy `/Border`).
    pub fn set_border(&mut self, border: &Border) -> &mut Self {
        self.dict.remove(b"Border");
        let mut bs = dict! {
            "Type" => Object::name("Border"),
            "W" => f64::from(border.width.max(0.0)),
            "S" => Object::name(border.style.as_name()),
        };
        if border.style == BorderStyle::Dashed && !border.dash.is_empty() {
            bs.set(
                "D",
                border
                    .dash
                    .iter()
                    .map(|&v| Object::from(f64::from(v)))
                    .collect::<Vec<_>>(),
            );
        }
        self.dict.set("BS", bs);
        self
    }

    /// Sets `/T` markup author/title.
    pub fn set_author(&mut self, author: &str) -> &mut Self {
        self.dict.set("T", Object::text(author));
        self
    }

    /// Sets `/Subj` markup subject.
    pub fn set_subject(&mut self, subject: &str) -> &mut Self {
        self.dict.set("Subj", Object::text(subject));
        self
    }

    /// Sets `/CA` markup opacity (`0.0..=1.0`).
    pub fn set_opacity(&mut self, opacity: f32) -> &mut Self {
        let ca = opacity.clamp(0.0, 1.0);
        if (ca - 1.0).abs() <= 1e-6 {
            self.dict.remove(b"CA");
        } else {
            self.dict.set("CA", f64::from(ca));
        }
        self
    }

    /// Sets `/IRT` reply target and `/RT` reply kind.
    pub fn set_in_reply_to(&mut self, target: ObjectId, kind: ReplyKind) -> &mut Self {
        self.dict.set("IRT", target);
        match kind {
            ReplyKind::Reply => {
                self.dict.remove(b"RT");
            }
            ReplyKind::Group => {
                self.dict.set("RT", Object::name("Group"));
            }
        }
        self
    }

    /// Sets `/Popup` reference.
    pub fn set_popup(&mut self, popup_id: ObjectId) -> &mut Self {
        self.dict.set("Popup", popup_id);
        self
    }

    /// Sets `/DA` default appearance string.
    pub fn set_default_appearance(&mut self, da: &DefaultAppearance) -> &mut Self {
        self.dict.set("DA", Object::bytes(da.to_bytes()));
        self
    }

    /// Sets `/Q` quadding.
    pub fn set_quadding(&mut self, q: Quadding) -> &mut Self {
        self.dict.set("Q", q as i64);
        self
    }

    /// Sets `/AS` active appearance state name.
    pub fn set_appearance_state(&mut self, state: &[u8]) -> &mut Self {
        self.dict.set("AS", Object::name(state));
        self
    }

    /// Binds `/AP << /N form_id >>` on this annotation.
    pub fn set_normal_appearance(&mut self, form_id: ObjectId) -> &mut Self {
        if let Some(ap) = self.dict.get_mut(b"AP").and_then(Object::as_dict_mut) {
            ap.set("N", form_id);
        } else {
            self.dict.set("AP", dict! { "N" => form_id });
        }
        self
    }

    /// Removes `/AP` and `/AS` from this annotation.
    pub fn clear_appearances(&mut self) -> &mut Self {
        self.dict.remove(b"AP");
        self.dict.remove(b"AS");
        self
    }

    /// Sets `/QuadPoints` from `quads`.
    pub fn set_quad_points(&mut self, quads: &[QuadPoints]) -> &mut Self {
        let mut arr = Vec::with_capacity(quads.len() * 8);
        for q in quads {
            arr.extend(q.0.into_iter().map(|v| Object::from(f64::from(v))));
        }
        self.dict.set("QuadPoints", arr);
        self
    }

    /// Sets `/Dest` (and removes `/A` so the link obeys ISO 32000-2 §12.5.6.5).
    pub fn set_dest(&mut self, dest: &Dest) -> &mut Self {
        self.dict.remove(b"A");
        self.dict.set("Dest", dest.to_object());
        self
    }

    /// Sets `/A` action dictionary (and removes `/Dest`).
    pub fn set_action_dict(&mut self, action: Dict) -> &mut Self {
        self.dict.remove(b"Dest");
        self.dict.set("A", action);
        self
    }
}

/// Constructed annotation dictionary paired with optional synthesized `/AP /N`
/// and `/RO` Form XObject streams.
///
/// Its `Debug` implementation reports dictionary entry counts and stream byte
/// lengths rather than raw dictionary values or plaintext.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub struct BuiltAnnot {
    /// Annotation dictionary (`/Type /Annot`, `/Subtype`, `/Rect`, etc.).
    pub dict: Dict,
    /// Synthesized `/AP /N` normal appearance stream, if any.
    pub normal_ap: Option<SynthesizedAp>,
    /// Synthesized `/RO` redaction overlay Form XObject stream (for `/Redact`),
    /// if any.
    pub redact_overlay: Option<SynthesizedAp>,
}

impl fmt::Debug for BuiltAnnot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BuiltAnnot")
            .field("dict_entries", &self.dict.len())
            .field("normal_ap", &self.normal_ap)
            .field("redact_overlay", &self.redact_overlay)
            .finish()
    }
}

impl BuiltAnnot {
    /// Allocates any synthesized `/AP /N` and `/RO` streams via `alloc`, wires
    /// their indirect `ObjectId`s into `self.dict`, and allocates and returns
    /// the annotation's own `ObjectId`.
    pub fn write_into(mut self, mut alloc: impl FnMut(Object) -> ObjectId) -> ObjectId {
        if let Some(ro) = self.redact_overlay {
            let ro_id = alloc(Object::from(ro.to_stream()));
            self.dict.set("RO", ro_id);
        }
        if let Some(ap) = self.normal_ap {
            let ap_id = alloc(Object::from(ap.to_stream()));
            AnnotMut::new(&mut self.dict).set_normal_appearance(ap_id);
        }
        alloc(Object::from(self.dict))
    }

    /// Builds a `/Link` annotation pointing to `dest`.
    #[must_use]
    pub fn link_dest(rect: Rect, dest: &Dest) -> Self {
        let mut d = base_annot_dict(&AnnotSubtype::Link, rect, AnnotFlags::PRINT);
        d.set(
            "Border",
            vec![Object::from(0), Object::from(0), Object::from(0)],
        );
        AnnotMut::new(&mut d).set_dest(dest);
        Self {
            dict: d,
            normal_ap: None,
            redact_overlay: None,
        }
    }

    /// Builds a `/Link` annotation with a `/URI` action.
    #[must_use]
    pub fn link_uri(rect: Rect, uri: impl Into<Vec<u8>>) -> Self {
        let mut d = base_annot_dict(&AnnotSubtype::Link, rect, AnnotFlags::PRINT);
        d.set(
            "Border",
            vec![Object::from(0), Object::from(0), Object::from(0)],
        );
        AnnotMut::new(&mut d).set_action_dict(Action::uri_dict(uri));
        Self {
            dict: d,
            normal_ap: None,
            redact_overlay: None,
        }
    }

    /// Builds a text-markup annotation (`Highlight`, `Underline`, `Squiggly`,
    /// or `StrikeOut`) with a synthesized `/AP /N` stream.
    #[must_use]
    pub fn text_markup(
        kind: TextMarkupKind,
        quads: &[QuadPoints],
        color: AnnotColor,
        opacity: f32,
    ) -> Self {
        let rect = union_quad_bounds(quads).unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0));
        let subtype = kind.subtype();
        let mut d = base_annot_dict(&subtype, rect, AnnotFlags::PRINT);
        {
            let mut m = AnnotMut::new(&mut d);
            m.set_color(color)
                .set_opacity(opacity)
                .set_quad_points(quads);
        }
        let normal_ap = synth_text_markup(kind, rect, quads, color, opacity);
        Self {
            dict: d,
            normal_ap,
            redact_overlay: None,
        }
    }

    /// Builds a `/Square` or `/Circle` annotation with a synthesized `/AP /N`
    /// stream.
    #[must_use]
    pub fn shape(kind: ShapeKind, rect: Rect, style: &PaintStyle) -> Self {
        let subtype = match kind {
            ShapeKind::Square => AnnotSubtype::Square,
            ShapeKind::Circle => AnnotSubtype::Circle,
        };
        let mut d = base_annot_dict(&subtype, rect, AnnotFlags::PRINT);
        {
            let mut m = AnnotMut::new(&mut d);
            if let Some(sc) = style.stroke {
                m.set_color(sc);
            }
            if let Some(ic) = style.fill {
                m.set_interior_color(ic);
            }
            m.set_border(&style.border).set_opacity(style.opacity);
        }
        let normal_ap = synth_shape(kind, rect, style, None);
        Self {
            dict: d,
            normal_ap,
            redact_overlay: None,
        }
    }

    /// Builds a `/Line` annotation with synthesized `/AP /N` stream (expanding
    /// `/Rect` to enclose both endpoints and arrowhead padding).
    #[must_use]
    pub fn line(
        start: Point,
        end: Point,
        endings: (LineEnding, LineEnding),
        style: &PaintStyle,
    ) -> Self {
        let pad = (style.border.width * 6.0).max(6.0);
        let rect = Rect::new(
            start.x.min(end.x) - pad,
            start.y.min(end.y) - pad,
            start.x.max(end.x) + pad,
            start.y.max(end.y) + pad,
        );
        let mut d = base_annot_dict(&AnnotSubtype::Line, rect, AnnotFlags::PRINT);
        d.set(
            "L",
            vec![
                Object::from(f64::from(start.x)),
                Object::from(f64::from(start.y)),
                Object::from(f64::from(end.x)),
                Object::from(f64::from(end.y)),
            ],
        );
        if endings != (LineEnding::None, LineEnding::None) {
            d.set(
                "LE",
                vec![
                    Object::name(endings.0.as_name()),
                    Object::name(endings.1.as_name()),
                ],
            );
        }
        {
            let mut m = AnnotMut::new(&mut d);
            if let Some(sc) = style.stroke {
                m.set_color(sc);
            }
            if let Some(ic) = style.fill {
                m.set_interior_color(ic);
            }
            m.set_border(&style.border).set_opacity(style.opacity);
        }
        let normal_ap = synth_line(rect, start, end, endings, style);
        Self {
            dict: d,
            normal_ap,
            redact_overlay: None,
        }
    }

    /// Builds a `/Polygon` or `/PolyLine` annotation with synthesized `/AP /N`.
    #[must_use]
    pub fn poly(
        kind: PolyKind,
        vertices: &[Point],
        endings: (LineEnding, LineEnding),
        style: &PaintStyle,
    ) -> Self {
        let pad = (style.border.width * 5.0).max(4.0);
        let rect = points_bounds(vertices, pad).unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0));
        let subtype = match kind {
            PolyKind::Polygon => AnnotSubtype::Polygon,
            PolyKind::PolyLine => AnnotSubtype::PolyLine,
        };
        let mut d = base_annot_dict(&subtype, rect, AnnotFlags::PRINT);
        let mut v_arr = Vec::with_capacity(vertices.len() * 2);
        for p in vertices {
            v_arr.push(Object::from(f64::from(p.x)));
            v_arr.push(Object::from(f64::from(p.y)));
        }
        d.set("Vertices", v_arr);
        if kind == PolyKind::PolyLine && endings != (LineEnding::None, LineEnding::None) {
            d.set(
                "LE",
                vec![
                    Object::name(endings.0.as_name()),
                    Object::name(endings.1.as_name()),
                ],
            );
        }
        {
            let mut m = AnnotMut::new(&mut d);
            if let Some(sc) = style.stroke {
                m.set_color(sc);
            }
            if let Some(ic) = style.fill {
                m.set_interior_color(ic);
            }
            m.set_border(&style.border).set_opacity(style.opacity);
        }
        let normal_ap = synth_poly(kind, rect, vertices, endings, style);
        Self {
            dict: d,
            normal_ap,
            redact_overlay: None,
        }
    }

    /// Builds an `/Ink` freehand annotation with synthesized `/AP /N`.
    #[must_use]
    pub fn ink(strokes: &[Vec<Point>], color: AnnotColor, border: &Border, opacity: f32) -> Self {
        let all_pts: Vec<Point> = strokes.iter().flatten().copied().collect();
        let pad = (border.width * 2.0).max(2.0);
        let rect = points_bounds(&all_pts, pad).unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0));
        let mut d = base_annot_dict(&AnnotSubtype::Ink, rect, AnnotFlags::PRINT);
        let ink_list: Vec<Object> = strokes
            .iter()
            .map(|s| {
                let mut coords = Vec::with_capacity(s.len() * 2);
                for p in s {
                    coords.push(Object::from(f64::from(p.x)));
                    coords.push(Object::from(f64::from(p.y)));
                }
                Object::from(coords)
            })
            .collect();
        d.set("InkList", ink_list);
        {
            let mut m = AnnotMut::new(&mut d);
            m.set_color(color).set_border(border).set_opacity(opacity);
        }
        let normal_ap = synth_ink(rect, strokes, color, border, opacity);
        Self {
            dict: d,
            normal_ap,
            redact_overlay: None,
        }
    }

    /// Builds a `/Stamp` rubber-stamp annotation with synthesized `/AP /N`.
    #[must_use]
    pub fn stamp(
        rect: Rect,
        stamp: StampName,
        custom_label: Option<&str>,
        color: AnnotColor,
        opacity: f32,
    ) -> Self {
        let mut d = base_annot_dict(&AnnotSubtype::Stamp, rect, AnnotFlags::PRINT);
        d.set("Name", Object::name(stamp.as_bytes()));
        {
            let mut m = AnnotMut::new(&mut d);
            m.set_color(color).set_opacity(opacity);
        }
        let normal_ap = synth_stamp(rect, &stamp, custom_label, color, opacity);
        Self {
            dict: d,
            normal_ap,
            redact_overlay: None,
        }
    }

    /// Builds a `/Text` sticky-note annotation with synthesized `/AP /N`.
    #[must_use]
    pub fn text_note(rect: Rect, contents: &str, icon: TextIcon, color: AnnotColor) -> Self {
        let flags = AnnotFlags::PRINT | AnnotFlags::NO_ZOOM | AnnotFlags::NO_ROTATE;
        let mut d = base_annot_dict(&AnnotSubtype::Text, rect, flags);
        d.set("Name", Object::name(icon.as_bytes()));
        {
            let mut m = AnnotMut::new(&mut d);
            m.set_contents(contents).set_color(color);
        }
        let normal_ap = synth_text_note(rect, &icon, color, 1.0);
        Self {
            dict: d,
            normal_ap,
            redact_overlay: None,
        }
    }

    /// Builds a `/FreeText` annotation with synthesized `/AP /N`.
    #[must_use]
    pub fn free_text(
        rect: Rect,
        contents: &str,
        da: &DefaultAppearance,
        quadding: Quadding,
        style: &PaintStyle,
    ) -> Self {
        let mut d = base_annot_dict(&AnnotSubtype::FreeText, rect, AnnotFlags::PRINT);
        {
            let mut m = AnnotMut::new(&mut d);
            m.set_contents(contents)
                .set_default_appearance(da)
                .set_quadding(quadding)
                .set_border(&style.border)
                .set_opacity(style.opacity);
            if let Some(bg) = style.fill {
                m.set_color(bg);
            }
        }
        let normal_ap = synth_free_text(rect, contents, Some(da), quadding, style);
        Self {
            dict: d,
            normal_ap,
            redact_overlay: None,
        }
    }

    /// Builds a standard `/Redact` annotation with synthesized `/AP /N` and
    /// `/RO` overlay Form XObject.
    #[must_use]
    pub fn redact(
        rect: Rect,
        quads: &[QuadPoints],
        interior_color: AnnotColor,
        overlay_text: Option<&str>,
        repeat: bool,
        da: Option<&DefaultAppearance>,
        quadding: Quadding,
    ) -> Self {
        let mut d = base_annot_dict(&AnnotSubtype::Redact, rect, AnnotFlags::PRINT);
        {
            let mut m = AnnotMut::new(&mut d);
            m.set_interior_color(interior_color).set_quadding(quadding);
            if !quads.is_empty() {
                m.set_quad_points(quads);
            }
            if let Some(da_val) = da {
                m.set_default_appearance(da_val);
            }
        }
        if let Some(txt) = overlay_text {
            d.set("OverlayText", Object::text(txt));
        }
        if repeat {
            d.set("Repeat", true);
        }
        let (normal_ap, redact_overlay) = match synth_redact(
            rect,
            None,
            Some(interior_color),
            overlay_text,
            repeat,
            da,
            quadding,
        ) {
            Some((ap, ro)) => (Some(ap), Some(ro)),
            None => (None, None),
        };
        Self {
            dict: d,
            normal_ap,
            redact_overlay,
        }
    }
}

fn base_annot_dict(subtype: &AnnotSubtype, rect: Rect, flags: AnnotFlags) -> Dict {
    let mut d = dict! {
        "Type" => Object::name("Annot"),
    };
    AnnotMut::new(&mut d)
        .set_subtype(subtype)
        .set_rect(rect)
        .set_flags(flags);
    d
}

fn union_quad_bounds(quads: &[QuadPoints]) -> Option<Rect> {
    let first = quads.first()?.bounds();
    Some(
        quads[1..]
            .iter()
            .fold(first, |acc, q| acc.union(q.bounds())),
    )
}

fn points_bounds(pts: &[Point], pad: f32) -> Option<Rect> {
    let first = *pts.first()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
    for p in &pts[1..] {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    Some(Rect::new(x0 - pad, y0 - pad, x1 + pad, y1 + pad))
}

/// Appends `annot_id` to `page_dict`'s direct `/Annots` array (creating one if
/// absent). Returns `false` if `/Annots` on `page_dict` is an indirect
/// reference that the caller must resolve and mutate directly.
pub fn push_page_annot(page_dict: &mut Dict, annot_id: ObjectId) -> bool {
    if page_dict
        .get(b"Annots")
        .is_some_and(|o| o.as_reference().is_some())
    {
        return false;
    }
    if let Some(arr) = page_dict.get_mut(b"Annots").and_then(Object::as_array_mut) {
        arr.push(Object::from(annot_id));
    } else {
        page_dict.set("Annots", vec![Object::from(annot_id)]);
    }
    true
}
