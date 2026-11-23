//! In-place annotation dictionary mutators (`AnnotMut`), pure typed annotation
//! builders (`BuiltAnnot`), and page `/Annots` array helpers.

use std::fmt;

use pdf_content_streams::Res;
#[cfg(feature = "text-layout")]
use pdf_gfx::{LayoutFont, UnencodableChar};
use pdf_gfx::{Point, Rect};
use pdf_objects::{Dict, Object, ObjectId, Value, dict};
use pdf_semantics::action::Action;
use pdf_semantics::date::Date;
use pdf_semantics::dest::Dest;

use crate::ap::AppearanceMode;
use crate::flags::AnnotFlags;
use crate::replies::ReplyKind;
use crate::resolve::QuadPoints;
use crate::style::{
    AnnotColor, AppearanceChars, Border, BorderStyle, DefaultAppearance, LineEnding, PaintStyle,
    Quadding,
};
use crate::subtypes::{AnnotSubtype, PolyKind, ShapeKind, StampName, TextIcon, TextMarkupKind};
use crate::synth::{
    RedactMark, SynthesizedAp, overlay_text_color, synth_free_text, synth_ink, synth_line,
    synth_poly, synth_redact, synth_shape, synth_stamp, synth_text_markup, synth_text_note,
};
#[cfg(feature = "text-layout")]
use crate::synth::{synth_free_text_with_font, synth_redact_with_font};

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

    /// Binds `/AP << /N form_id >>` on this annotation, keeping the other
    /// entries of an `/AP` that is a direct dictionary.
    ///
    /// An `/AP` that is anything else, an indirect reference among them, is
    /// replaced by a new dictionary of `/N` alone: nothing here can follow
    /// a reference, so a caller who means to keep the dictionary it names
    /// changes that object itself.
    pub fn set_normal_appearance(&mut self, form_id: ObjectId) -> &mut Self {
        if let Some(ap) = self.dict.get_mut(b"AP").and_then(Object::as_dict_mut) {
            ap.set("N", form_id);
        } else {
            self.dict.set("AP", dict! { "N" => form_id });
        }
        self
    }

    /// Binds `/AP << /R form_id >>`, the rollover appearance shown while the
    /// pointer is over the annotation (ISO 32000-2 §12.5.5 Table 170),
    /// keeping the other entries of an `/AP` that is a direct dictionary.
    ///
    /// With no `/AP` a new one of `/R` alone is made. An `/AP` that is
    /// there and isn't a direct dictionary (an indirect reference, as files
    /// often have it) is left as it is and nothing is bound: replacing it
    /// would drop the `/N` it names, and nothing here can follow the
    /// reference. [`Self::has_direct_appearance`] tells the two apart
    /// beforehand; for an indirect `/AP`, set `/R` in the object it names.
    pub fn set_rollover_appearance(&mut self, form_id: ObjectId) -> &mut Self {
        match self.dict.get_mut(b"AP") {
            None => {
                self.dict.set("AP", dict! { "R" => form_id });
            }
            Some(ap) => {
                // Asked first, so that an `/AP` left alone keeps its span.
                if ap.as_dict().is_some()
                    && let Some(ap) = ap.as_dict_mut()
                {
                    ap.set("R", form_id);
                }
            }
        }
        self
    }

    /// Binds `/AP << /D form_id >>`, the down appearance shown while the
    /// mouse button is pressed within the annotation (ISO 32000-2 §12.5.5
    /// Table 170), keeping the other entries of an `/AP` that is a direct
    /// dictionary.
    ///
    /// With no `/AP` a new one of `/D` alone is made. An `/AP` that is
    /// there and isn't a direct dictionary is left as it is and nothing is
    /// bound, as [`Self::set_rollover_appearance`] leaves it: replacing it
    /// would drop the `/N` it names. [`Self::has_direct_appearance`] tells
    /// the two apart beforehand.
    pub fn set_down_appearance(&mut self, form_id: ObjectId) -> &mut Self {
        if let Some(ap) = self.appearance_dict(AppearanceMode::Down) {
            ap.set("D", form_id);
        }
        self
    }

    /// Binds the `mode` slot of `/AP` (`/N`, `/R`, or `/D`) to a state
    /// dictionary of exactly `states`, each a state's name and its Form
    /// XObject, in the order given (ISO 32000-2 §12.5.5): a check box's or
    /// a radio button's appearances under its on state's name and `Off`,
    /// say. What the slot held is replaced. A state not given isn't
    /// written, so an `/Off` the caller leaves out stays absent; a name
    /// given twice is written once, where it's first given, with the last
    /// form given for it.
    ///
    /// `/AP` is kept where it's a direct dictionary, its other slots with
    /// it, and made where there is none. Any other `/AP`, an indirect
    /// reference among them, is replaced by a new dictionary of this slot
    /// alone for `/N`, as [`Self::set_normal_appearance`] replaces it, and
    /// left as it is for `/R` and `/D`, nothing bound, as
    /// [`Self::set_rollover_appearance`] leaves it;
    /// [`Self::has_direct_appearance`] tells which beforehand. `/AS`, the
    /// state shown, is [`Self::set_appearance_state`]'s.
    pub fn set_appearance_states(
        &mut self,
        mode: AppearanceMode,
        states: &[(&[u8], ObjectId)],
    ) -> &mut Self {
        if let Some(ap) = self.appearance_dict(mode) {
            let mut slot = Dict::with_capacity(states.len());
            for &(state, form_id) in states {
                slot.set(state, form_id);
            }
            ap.set(mode.key(), slot);
        }
        self
    }

    /// Binds the state `state` of the `mode` slot of `/AP` (`/N`, `/R`, or
    /// `/D`) to `form_id`, keeping the slot's other states where it's a
    /// direct state dictionary: the state is set where the dictionary has
    /// it, and added at its end where it hasn't.
    ///
    /// A slot that is anything else becomes a dictionary of this state
    /// alone: a single Form XObject, which states don't sit beside, or an
    /// indirect reference, which may name a state dictionary that nothing
    /// here can follow; to keep that dictionary's states, set the state in
    /// the object it names. `/AP` itself is kept, made, replaced, or left
    /// as [`Self::set_appearance_states`] says.
    pub fn set_state_appearance(
        &mut self,
        mode: AppearanceMode,
        state: &[u8],
        form_id: ObjectId,
    ) -> &mut Self {
        if let Some(ap) = self.appearance_dict(mode) {
            let key = mode.key();
            // Asked first: a slot that is a stream has a dictionary too.
            if ap
                .get(key)
                .is_some_and(|slot| matches!(slot.value(), Value::Dict(_)))
                && let Some(slot) = ap.get_mut(key).and_then(Object::as_dict_mut)
            {
                slot.set(state, form_id);
            } else {
                let mut slot = Dict::new();
                slot.set(state, form_id);
                ap.set(key, slot);
            }
        }
        self
    }

    /// Whether `/AP` is a direct dictionary, one whose entries
    /// [`Self::set_normal_appearance`] and [`Self::set_rollover_appearance`]
    /// keep. `false` with no `/AP`, and for one that is an indirect
    /// reference or not a dictionary.
    ///
    /// [`Self::set_down_appearance`], [`Self::set_appearance_states`], and
    /// [`Self::set_state_appearance`] go by it too: where it's `true` they
    /// keep `/AP`'s other entries, and where it's `false` and there is an
    /// `/AP`, one that binds `/N` replaces it and one that binds `/R` or `/D`
    /// leaves it as it is.
    #[must_use]
    pub fn has_direct_appearance(&self) -> bool {
        self.dict
            .get(b"AP")
            .is_some_and(|ap| ap.as_dict().is_some())
    }

    /// `/AP`, to bind a `mode` slot in: a direct dictionary as it is, and
    /// one made where there is none. Any other `/AP` is replaced by a new
    /// dictionary for `/N`, and left as it is (`None`) for `/R` and `/D`,
    /// untouched, so it keeps its span.
    fn appearance_dict(&mut self, mode: AppearanceMode) -> Option<&mut Dict> {
        if !self.has_direct_appearance() {
            if mode != AppearanceMode::Normal && self.dict.get(b"AP").is_some() {
                return None;
            }
            self.dict.set("AP", Dict::new());
        }
        self.dict.get_mut(b"AP").and_then(Object::as_dict_mut)
    }

    /// Sets `/MK`, a widget's appearance characteristics (ISO 32000-2
    /// §12.5.6.19 Table 192), to `chars`: `/R`, `/BC`, `/BG`, `/CA`, `/RC`,
    /// `/AC`, and `/TP`, each written as `chars` has it and removed where it
    /// hasn't (`/R` where it's 0, its default), keeping the other entries
    /// of a `/MK` that is a direct dictionary: an icon (`/I`, `/RI`, `/IX`)
    /// and its fit (`/IF`) among them.
    ///
    /// A `/MK` that is anything else, an indirect reference among them, is
    /// replaced by a new dictionary of what `chars` has: nothing here can
    /// follow a reference, so a caller who means to keep the dictionary it
    /// names changes that object itself.
    pub fn set_appearance_chars(&mut self, chars: &AppearanceChars) -> &mut Self {
        if !self
            .dict
            .get(b"MK")
            .is_some_and(|mk| matches!(mk.value(), Value::Dict(_)))
        {
            self.dict.set("MK", Dict::new());
        }
        let Some(mk) = self.dict.get_mut(b"MK").and_then(Object::as_dict_mut) else {
            return self;
        };
        let caption = |text: &Option<String>| text.as_deref().map(Object::text);
        let entries = [
            (
                "R",
                (chars.rotation != 0).then(|| Object::from(i64::from(chars.rotation))),
            ),
            ("BC", chars.border_color.map(AnnotColor::to_object)),
            ("BG", chars.background_color.map(AnnotColor::to_object)),
            ("CA", caption(&chars.normal_caption)),
            ("RC", caption(&chars.rollover_caption)),
            ("AC", caption(&chars.down_caption)),
            (
                "TP",
                chars.text_position.map(|tp| Object::from(i64::from(tp))),
            ),
        ];
        for (key, value) in entries {
            match value {
                Some(value) => mk.set(key, value),
                None => {
                    mk.remove(key);
                }
            }
        }
        self
    }

    /// Sets `/RO`, the Form XObject that applying a `/Redact` mark leaves
    /// over its regions (ISO 32000-2 §12.5.6.23 Table 195).
    pub fn set_redact_overlay(&mut self, form_id: ObjectId) -> &mut Self {
        self.dict.set("RO", form_id);
        self
    }

    /// Sets `/OverlayText`, the label that applying a `/Redact` mark draws
    /// over its regions. The table asks for a `/DA` beside it
    /// ([`Self::set_default_appearance`]).
    pub fn set_overlay_text(&mut self, text: &str) -> &mut Self {
        self.dict.set("OverlayText", Object::text(text));
        self
    }

    /// Sets `/Repeat`, whether a `/Redact` mark's overlay text repeats across
    /// each region: `true` is written, and the key removed for `false`, its
    /// default.
    pub fn set_repeat(&mut self, repeat: bool) -> &mut Self {
        if repeat {
            self.dict.set("Repeat", true);
        } else {
            self.dict.remove(b"Repeat");
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
/// and `/RO` Form XObject streams, and the font its `/DA` should name.
///
/// Its `Debug` implementation reports dictionary entry counts and stream byte
/// lengths rather than raw dictionary values or plaintext, and whether there's
/// a `/DA` font, not which.
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
    /// The font the appearances draw the text in, for the document's form to
    /// hold under the name `/DA` gives (`/AcroForm /DR /Font`, ISO 32000-2,
    /// 12.7.4.3), so a viewer that makes the appearance again from `/DA`
    /// draws in it too: the font given to `free_text_with_font` or
    /// `redact_with_font` (`text-layout` feature), `None` from every other
    /// builder. A builder can't reach the document's catalog, so `/DA` is
    /// written as given: pdfs' `Document::add_annot` puts the font in the
    /// form's default resources, under another name where that one is
    /// another font's, and names it in `/DA`; [`write_into`](Self::write_into)
    /// leaves that to its caller.
    pub da_font: Option<Res>,
}

impl fmt::Debug for BuiltAnnot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BuiltAnnot")
            .field("dict_entries", &self.dict.len())
            .field("normal_ap", &self.normal_ap)
            .field("redact_overlay", &self.redact_overlay)
            .field("da_font", &self.da_font.is_some())
            .finish()
    }
}

impl BuiltAnnot {
    /// Allocates any synthesized `/AP /N` and `/RO` streams via `alloc`, wires
    /// their indirect `ObjectId`s into `self.dict`, and allocates and returns
    /// the annotation's own `ObjectId`.
    ///
    /// A redaction's overlay is also bound as `/AP /R` beside `/N`, so a
    /// viewer shows what applying the mark leaves while the pointer is over
    /// it.
    ///
    /// [`da_font`](Self::da_font) isn't written: `alloc` reaches no catalog,
    /// so `/DA` is left as given and the form's default resources as they
    /// are. A caller that wants the font found by `/DA` puts it in
    /// `/AcroForm /DR /Font` itself, as pdfs' `Document::add_annot` does
    /// (pdf-forms' `default_resources::font_name` gives a name no other font
    /// holds), and names it in `/DA`.
    pub fn write_into(mut self, mut alloc: impl FnMut(Object) -> ObjectId) -> ObjectId {
        let ro_id = self.redact_overlay.map(|ro| {
            let ro_id = alloc(Object::from(ro.to_stream()));
            AnnotMut::new(&mut self.dict).set_redact_overlay(ro_id);
            ro_id
        });
        if let Some(ap) = self.normal_ap {
            let ap_id = alloc(Object::from(ap.to_stream()));
            let mut m = AnnotMut::new(&mut self.dict);
            m.set_normal_appearance(ap_id);
            if let Some(ro_id) = ro_id {
                m.set_rollover_appearance(ro_id);
            }
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
            da_font: None,
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
            da_font: None,
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
            da_font: None,
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
            da_font: None,
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
            da_font: None,
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
            da_font: None,
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
            da_font: None,
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
            da_font: None,
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
            da_font: None,
        }
    }

    /// Builds a `/FreeText` annotation with synthesized `/AP /N`, its text in
    /// Helvetica ([`synth_free_text`]).
    #[must_use]
    pub fn free_text(
        rect: Rect,
        contents: &str,
        da: &DefaultAppearance,
        quadding: Quadding,
        style: &PaintStyle,
    ) -> Self {
        let normal_ap = synth_free_text(rect, contents, Some(da), quadding, style);
        Self::free_text_over(rect, contents, da, quadding, style, normal_ap)
    }

    /// Builds a `/FreeText` annotation as [`Self::free_text`] does, the text
    /// of its `/AP /N` drawn in `font` under the resource `font_res`
    /// ([`synth_free_text_with_font`]): a font embedded in the document, say,
    /// for text beyond Helvetica's characters.
    ///
    /// `/DA` is written as given, and `font_res` kept as
    /// [`da_font`](Self::da_font), for the document's form to hold under the
    /// name `/DA` gives: pdfs' `Document::add_annot` puts it there, and
    /// renames it in `/DA` where that name is another font's, so a viewer
    /// that makes an appearance of its own from `/DA` draws in it too.
    /// [`write_into`](Self::write_into) leaves that to its caller.
    ///
    /// # Errors
    ///
    /// [`UnencodableChar`] for the first character of `contents` that `font`
    /// can't draw: nothing is built. The error holds that one character of
    /// the text, as [`synth_free_text_with_font`] says.
    #[cfg(feature = "text-layout")]
    pub fn free_text_with_font<F: LayoutFont + ?Sized>(
        rect: Rect,
        contents: &str,
        da: &DefaultAppearance,
        quadding: Quadding,
        style: &PaintStyle,
        font_res: Res,
        font: &F,
    ) -> Result<Self, UnencodableChar> {
        let normal_ap = synth_free_text_with_font(
            rect,
            contents,
            Some(da),
            quadding,
            style,
            font_res.clone(),
            font,
        )?;
        Ok(Self {
            da_font: Some(font_res),
            ..Self::free_text_over(rect, contents, da, quadding, style, normal_ap)
        })
    }

    /// A `/FreeText` annotation's dictionary, with `normal_ap` its `/AP /N`.
    fn free_text_over(
        rect: Rect,
        contents: &str,
        da: &DefaultAppearance,
        quadding: Quadding,
        style: &PaintStyle,
        normal_ap: Option<SynthesizedAp>,
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
        Self {
            dict: d,
            normal_ap,
            redact_overlay: None,
            da_font: None,
        }
    }

    /// Builds a standard `/Redact` annotation with synthesized `/AP /N` and
    /// `/RO` overlay Form XObject, both drawn over `quads` where there are
    /// any ([`synth_redact`]).
    ///
    /// [`Self::redact_mark`] over a [`RedactMark`] of these values and no
    /// outline color.
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
        Self::redact_mark(
            rect,
            &RedactMark {
                quads,
                interior_color: Some(interior_color),
                overlay_text,
                repeat,
                da,
                quadding,
                ..RedactMark::new()
            },
        )
    }

    /// Builds a `/Redact` annotation over `rect` from `mark`, with
    /// synthesized `/AP /N` and `/RO` overlay Form XObject ([`synth_redact`]).
    ///
    /// The dictionary gets `/C` and `/IC` where the mark has those colors
    /// (`/IC` as three `DeviceRGB` numbers whatever the color's space, as ISO
    /// 32000-2, 12.5.6.23, Table 195 has it; none for a transparent one),
    /// `/QuadPoints` where it has quadrilaterals, `/Q`, `/OverlayText`,
    /// `/Repeat`, and `/DA`: the mark's, or beside overlay text with none
    /// given, the one the overlay was drawn with (`/Helv 0 Tf 1 g`, white
    /// over the interior color, or `/Helv 0 Tf 0 g` with no interior color),
    /// as the table asks for a `/DA` there.
    ///
    /// With no interior color, or a transparent one, there is no `/IC` and
    /// the `/RO` fills nothing, so the two say the same: the regions are
    /// left transparent. `/QuadPoints` are written as given, though the
    /// appearances draw only what of them lies inside `rect`.
    #[must_use]
    pub fn redact_mark(rect: Rect, mark: &RedactMark<'_>) -> Self {
        Self::redact_over(rect, mark, synth_redact(rect, mark))
    }

    /// Builds a `/Redact` annotation as [`Self::redact_mark`] does, the
    /// overlay text of its `/RO` drawn in `font` under the resource
    /// `font_res` ([`synth_redact_with_font`]): a font embedded in the
    /// document, say, for a label beyond Helvetica's characters. For a
    /// `/Rect` taken from the mark's quadrilaterals, pass
    /// [`RedactMark::bounds`].
    ///
    /// The dictionary is [`Self::redact_mark`]'s: a `/DA` is written as
    /// given, or `/Helv 0 Tf 1 g` (`0 g` with no interior color) beside
    /// overlay text with none given. `font_res` is kept as
    /// [`da_font`](Self::da_font), for the document's form to hold under the
    /// name that `/DA` gives: pdfs' `Document::add_annot` puts it there, and
    /// renames it in `/DA` where that name is another font's, so a viewer
    /// that draws the label itself from `/OverlayText` and `/DA` draws in it
    /// too, as one that applies the mark with its `/RO` leaves the text laid
    /// out here. With no `/DA`, nothing names it.
    ///
    /// # Errors
    ///
    /// [`UnencodableChar`] for the first character of the overlay text that
    /// `font` can't draw: nothing is built. Its offset and what it holds of
    /// the text are as [`synth_redact_with_font`] says.
    #[cfg(feature = "text-layout")]
    pub fn redact_with_font<F: LayoutFont + ?Sized>(
        rect: Rect,
        mark: &RedactMark<'_>,
        font_res: Res,
        font: &F,
    ) -> Result<Self, UnencodableChar> {
        let forms = synth_redact_with_font(rect, mark, font_res.clone(), font)?;
        Ok(Self {
            da_font: Some(font_res),
            ..Self::redact_over(rect, mark, forms)
        })
    }

    /// A `/Redact` annotation's dictionary, with `forms` its `/AP /N` and
    /// `/RO`.
    fn redact_over(
        rect: Rect,
        mark: &RedactMark<'_>,
        forms: Option<(SynthesizedAp, SynthesizedAp)>,
    ) -> Self {
        let mut d = base_annot_dict(&AnnotSubtype::Redact, rect, AnnotFlags::PRINT);
        {
            let mut m = AnnotMut::new(&mut d);
            if let Some(c) = mark.stroke_color {
                m.set_color(c);
            }
            if let Some(rgb) = mark.interior_color.and_then(device_rgb) {
                m.set_interior_color(AnnotColor::Rgb(rgb));
            }
            m.set_quadding(mark.quadding);
            if !mark.quads.is_empty() {
                m.set_quad_points(mark.quads);
            }
            match (mark.da, mark.overlay_text) {
                (Some(da), _) => {
                    m.set_default_appearance(da);
                }
                (None, Some(_)) => {
                    m.set_default_appearance(&DefaultAppearance::new(
                        b"Helv".to_vec(),
                        0.0,
                        Some(overlay_text_color(mark)),
                    ));
                }
                (None, None) => {}
            }
            if let Some(txt) = mark.overlay_text {
                m.set_overlay_text(txt);
            }
            m.set_repeat(mark.repeat);
        }
        let (normal_ap, redact_overlay) = match forms {
            Some((ap, ro)) => (Some(ap), Some(ro)),
            None => (None, None),
        };
        Self {
            dict: d,
            normal_ap,
            redact_overlay,
            da_font: None,
        }
    }

    /// Builds a `/Redact` annotation from `mark` whose `/Rect` is the
    /// smallest rectangle that holds the mark's quadrilaterals
    /// ([`RedactMark::bounds`]), as [`Self::redact_mark`] builds it.
    ///
    /// Returns `None` where the mark has no usable quadrilateral, or they
    /// span no width or no height.
    #[must_use]
    pub fn redact_quads(mark: &RedactMark<'_>) -> Option<Self> {
        let rect = mark.bounds()?;
        (rect.width() > 0.0 && rect.height() > 0.0).then(|| Self::redact_mark(rect, mark))
    }
}

/// `color` as `DeviceRGB` components: gray three times, CMYK by the device
/// formula (ISO 32000-2 §10.4.2.4); `None` for a transparent color.
fn device_rgb(color: AnnotColor) -> Option<[f32; 3]> {
    match color {
        AnnotColor::Transparent => None,
        AnnotColor::Gray(g) => Some([g, g, g]),
        AnnotColor::Rgb(rgb) => Some(rgb),
        AnnotColor::Cmyk([c, m, y, k]) => {
            let ink = |v: f32| 1.0 - (v.clamp(0.0, 1.0) + k.clamp(0.0, 1.0)).min(1.0);
            Some([ink(c), ink(m), ink(y)])
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
