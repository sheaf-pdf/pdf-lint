//! A button's and a choice field's appearances from what the read layer
//! gives: the widget's box, border, `/MK`, and states, and its field's
//! default appearance, options, and selection.

use std::fmt;

use pdf_annots::{Annot, AppearanceChars, BorderStyle, HighlightMode, TypedAnnot};
use pdf_content_streams::{Color, Res};
use pdf_fonts::Std14;
use pdf_gfx::{BoxBorder, BoxBorderStyle, DefaultAppearance};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Streams, Value};

use super::button::{
    PushAppearances, PushButton, ToggleAppearances, ToggleButton, check_box, push_button,
    radio_button,
};
use super::choice::{ComboBox, ListBox, combo_box, list_box};
use super::font::FieldFont;
use super::frame::{Appearance, Unmade, WidgetLook};
use super::widget::{DaFaults, MadeText, WithoutDa};
use crate::fault::Fault;
use crate::field::{Field, FieldKind, FieldOption, FieldValue, Widget, WidgetStates};
use crate::form::Form;
use crate::group;
use crate::limits::Limits;
use crate::read::Reader;

/// What every button's reader reads of a widget: its box, colors, rotation,
/// and border, its `/MK`, its appearances, and its default appearance
/// string parsed.
#[derive(Clone)]
struct Look {
    width: f32,
    height: f32,
    background: Option<Color>,
    border_color: Option<Color>,
    rotation: i32,
    border_width: f32,
    border_style: BoxBorderStyle,
    dash: Vec<f32>,
    da: DefaultAppearance,
    /// Whether there's a default appearance string at all: the widget's,
    /// its field's, an ancestor's, or the form's.
    has_da: bool,
}

impl Look {
    fn look(&self) -> WidgetLook<'_> {
        WidgetLook {
            width: self.width,
            height: self.height,
            background: self.background,
            border_color: self.border_color,
            rotation: self.rotation,
            border: BoxBorder::new(self.border_width, self.border_style).with_dash(&self.dash),
        }
    }
}

impl fmt::Debug for Look {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Look")
            .field("look", &self.look())
            .field("da", &self.da)
            .finish()
    }
}

/// Reads `widget` of `field` as an annotation, and its default appearance
/// string, by `widget_text`'s rules: the annotation (its rectangle, border,
/// and `/MK`), and the widget's own `/DA`, else the field's effective one,
/// parsed under the cap `pdf-annots` parses one under, what it passes over
/// told as [`Fault::DefaultAppearance`] in the object it was read from. The
/// annotation is given beside what's read of it.
fn read_look<'o, R, S>(
    field: &Field<'o>,
    widget: &Widget<'o>,
    objects: R,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<(Annot<'o>, Look, Option<ObjectId>), Unmade>
where
    R: Resolve<'o> + Copy,
    S: Sink<Fault> + ?Sized,
{
    let annot = widget.annot_with(objects, rules, limits, &mut WithoutDa(&mut *sink))?;
    let mk = mk_of(&annot);
    let border = annot.border();
    let border_style = match border.style {
        BorderStyle::Dashed => BoxBorderStyle::Dashed,
        BorderStyle::Beveled => BoxBorderStyle::Beveled,
        BorderStyle::Inset => BoxBorderStyle::Inset,
        BorderStyle::Underline => BoxBorderStyle::Underline,
        _ => BoxBorderStyle::Solid,
    };
    let (da, at) = match widget.is_merged() {
        true => (field.default_appearance(), field.id()),
        false => {
            let mut reader = Reader {
                objects,
                rules,
                limits,
                sink: &mut (),
            };
            match reader.bytes_entry(widget.dict(), widget.id(), "DA")? {
                Some(own) => (Some(own), widget.id().or(field.id())),
                None => (field.default_appearance(), field.id()),
            }
        }
    };
    let parsed = da.and_then(|da| {
        DefaultAppearance::parse_with(
            da,
            &limits.annots.content,
            &mut DaFaults {
                sink: &mut *sink,
                at,
            },
        )
    });
    let rect = annot.rect();
    let look = Look {
        width: rect.width(),
        height: rect.height(),
        background: mk
            .and_then(|mk| mk.background_color)
            .and_then(|c| c.to_content_color()),
        border_color: mk
            .and_then(|mk| mk.border_color)
            .and_then(|c| c.to_content_color()),
        rotation: mk.map_or(0, |mk| mk.rotation),
        border_width: border.width,
        border_style,
        dash: border.dash.clone(),
        da: parsed.unwrap_or_default(),
        has_da: da.is_some(),
    };
    Ok((annot, look, at))
}

/// `annot`'s `/MK`, where it's a widget that has one.
fn mk_of<'a>(annot: &'a Annot<'_>) -> Option<&'a AppearanceChars> {
    match annot.typed() {
        TypedAnnot::Widget(typed) => typed.appearance_chars.as_ref(),
        _ => None,
    }
}

/// One widget of a check box or radio button field, read for its
/// appearances: the widget's box, border, and `/MK` (its caption among
/// them), its default appearance string parsed, and the name its on state's
/// appearances go under. No font is read: [`make`](Self::make) draws the
/// mark in the ZapfDingbats the caller names.
///
/// `Debug` prints lengths and numbers, never the caption or a state's name.
#[derive(Clone)]
pub struct WidgetToggle {
    radio: bool,
    caption: Option<String>,
    look: Look,
    on_state: Vec<u8>,
}

impl WidgetToggle {
    /// The button as [`check_box`](super::check_box) and
    /// [`radio_button`](super::radio_button) take it: its normal caption.
    #[must_use]
    pub fn button(&self) -> ToggleButton<'_> {
        ToggleButton {
            caption: self.caption.as_deref(),
        }
    }

    /// The widget's box, colors, rotation, and border.
    #[must_use]
    pub fn look(&self) -> WidgetLook<'_> {
        self.look.look()
    }

    /// The default appearance string parsed: empty where there is none, or
    /// it can't be read. Its color is the mark's, and its size where it gives
    /// one; its font isn't read.
    #[must_use]
    pub const fn default_appearance(&self) -> &DefaultAppearance {
        &self.look.da
    }

    /// Whether the widget is a radio button's.
    #[must_use]
    pub const fn is_radio(&self) -> bool {
        self.radio
    }

    /// The name the on state's appearances go under in the widget's state
    /// dictionaries, and that its `/AS` takes when it's on:
    /// [`group::on_state_name`](crate::group::on_state_name) of the name its
    /// normal appearance has for on, whether the field has an `/Opt` array,
    /// and the widget's index among the field's, as the field's group
    /// ([`Field::group`](crate::Field::group)) names it.
    #[must_use]
    pub fn on_state(&self) -> &[u8] {
        &self.on_state
    }

    /// Makes the widget's appearances with the mark in ZapfDingbats, named
    /// by `font_res`: [`check_box`](super::check_box) or
    /// [`radio_button`](super::radio_button) on what was read.
    ///
    /// # Errors
    ///
    /// As [`check_box`](super::check_box).
    pub fn make(&self, font_res: Res, limits: &Limits) -> Result<ToggleAppearances, Unmade> {
        let made = match self.radio {
            true => radio_button,
            false => check_box,
        };
        made(
            &self.button(),
            &self.look(),
            &self.look.da,
            font_res,
            limits,
        )
    }
}

impl fmt::Debug for WidgetToggle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WidgetToggle")
            .field("radio", &self.radio)
            .field("button", &self.button())
            .field("look", &self.look)
            .field("on_state_len", &self.on_state.len())
            .finish()
    }
}

/// Reads `widget` of `field`, a check box or radio button field, for its
/// appearances, under `rules`, departures told to `sink`.
///
/// - **The widget** and **the default appearance string** are read as
///   [`widget_text`](super::widget_text) reads a text widget's: the
///   rectangle, the border, `/MK`'s colors, rotation, and caption, and the
///   widget's own `/DA`, else the field's effective one. A default
///   appearance that names no font isn't told: a button's mark isn't drawn
///   in it, and needs no `/DA` (ISO 32000-2, §12.7.4.3 asks it of variable
///   text).
/// - **The on state's name** is the one its normal appearance's state
///   dictionary has, read with the annotation (of its names, the first that
///   isn't `Off`, or under `Rules::Pdfium` the least, as
///   [`Widget::states`](crate::Widget::states) reads it); else, where the
///   field has an `/Opt` array, the widget's index among the field's
///   widgets ([`Widget::index`](crate::Widget::index)); else `Yes`
///   ([`group::on_state_name`]). Of `/Opt` only whether it's an array is
///   asked ([`Field::option_count`](crate::Field::option_count)): no export
///   value is decoded, so a field's widgets are each read in the time one
///   takes, however many it has, and what's wrong in `/Opt` is told once, by
///   [`Field::options_with`](crate::Field::options_with) or
///   [`Field::group_with`](crate::Field::group_with), not here for each
///   widget.
///
/// `widget` is one of `field`'s own ([`Field::widgets`](crate::Field::widgets)).
///
/// # Errors
///
/// [`Unmade::NotButton`] for a field that isn't a check box or radio button
/// field; [`Unmade::Limit`] for a cap of [`Limits::annots`] passed;
/// [`Unmade::Unreadable`] on locked, I/O, or reader-cap failures.
pub fn widget_toggle<'o, R, S>(
    field: &Field<'o>,
    widget: &Widget<'o>,
    objects: R,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<WidgetToggle, Unmade>
where
    R: Resolve<'o> + Copy,
    S: Sink<Fault> + ?Sized,
{
    let radio = match field.kind() {
        Some(FieldKind::CheckBox) => false,
        Some(FieldKind::Radio) => true,
        _ => return Err(Unmade::NotButton),
    };
    let (annot, look, _) = read_look(field, widget, objects, rules, limits, sink)?;
    let caption = mk_of(&annot).and_then(|mk| mk.normal_caption.clone());
    let states = WidgetStates::of(annot.appearances().cloned(), rules);
    let has_opt = field.option_count(objects, rules, limits)?.is_some();
    let on_state = group::on_state_name(states.on_state.as_deref(), has_opt, widget.index());
    Ok(WidgetToggle {
        radio,
        caption,
        look,
        on_state: on_state.into_owned(),
    })
}

/// A check box's or radio button's appearances made from what the read
/// layer gives ([`toggle_for_widget`]), the name its on state's go under,
/// and the font its mark is drawn in.
///
/// The caller binds each appearance as a form XObject in the widget's
/// state dictionaries (`/AP /N` and `/AP /D`): the on state's under
/// [`on_state`](Self::on_state), `Off`'s where there is one. Where the font
/// is a fallback ([`FieldFont::is_fallback`]), the appearances' resources
/// name it by its dictionary, which is all they need to draw: unlike a text
/// field's fallback, it isn't the font `/DA` names (which may be another,
/// or ZapfDingbats with an encoding of its own), so it isn't one for the
/// default resources to hold under that name.
///
/// `Debug` prints the name's length, never the name.
#[non_exhaustive]
pub struct MadeToggle {
    /// The appearances.
    pub appearances: ToggleAppearances,
    /// The name the on state's appearances go under.
    pub on_state: Vec<u8>,
    /// The font the mark is drawn in: ZapfDingbats.
    pub font: FieldFont,
}

impl fmt::Debug for MadeToggle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MadeToggle")
            .field("appearances", &self.appearances)
            .field("on_state_len", &self.on_state.len())
            .field("font", &self.font)
            .finish()
    }
}

/// Makes the appearances of `widget` of `field`, a check box or radio
/// button field of `form`: [`widget_toggle`], then [`WidgetToggle::make`]
/// in ZapfDingbats. Departures are told to `sink`.
///
/// The font is the default resources' font the default appearance string
/// names, where that is ZapfDingbats as the standard 14 font is written by
/// name (`/Type1`, `/BaseFont /ZapfDingbats`, no `/Encoding` and no program
/// embedded), so the mark's code draws the glyph its metrics are; else
/// ZapfDingbats as [`FieldFont::standard`] writes it, a fallback. Another
/// font the default resources hold isn't read. A name they lack is told as
/// [`Fault::FontNotInResources`], as for a text field's.
///
/// # Errors
///
/// As [`widget_toggle`] and [`WidgetToggle::make`]; and as
/// [`Form::field_font`] where the default resources' font is ZapfDingbats
/// written by name, which is read, or [`Form::font`] where they hold
/// another.
pub fn toggle_for_widget<'o, R, S>(
    form: &Form<'o>,
    field: &Field<'o>,
    widget: &Widget<'o>,
    objects: R,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<MadeToggle, Unmade>
where
    R: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let read = widget_toggle(field, widget, objects, rules, limits, sink)?;
    let mut font = None;
    if let Some(name) = read.default_appearance().font_name.as_deref() {
        match form.font(objects, name, limits)? {
            Some((dict, _)) if plain_zapf_dingbats(dict, objects) => {
                let found = form.field_font(objects, name, limits, sink)?;
                if !found.is_fallback() && found.res().is_some() {
                    font = Some(found);
                }
            }
            Some(_) => {}
            // Read for what it tells: the default resources lack it.
            None => {
                form.field_font(objects, name, limits, sink)?;
            }
        }
    }
    let font = font.unwrap_or_else(|| FieldFont::standard(Std14::ZapfDingbats));
    let res = font.res().ok_or(Unmade::FontNotBindable)?;
    let appearances = read.make(res, limits)?;
    Ok(MadeToggle {
        appearances,
        on_state: read.on_state,
        font,
    })
}

/// Whether the font dictionary `dict` is ZapfDingbats as the standard 14
/// font is written by name: `/Subtype /Type1`, a `/BaseFont` of
/// ZapfDingbats, no `/Encoding`, and no program in its descriptor, each
/// entry followed once where it's a reference.
fn plain_zapf_dingbats<'o, R: Resolve<'o>>(dict: &'o Dict, objects: R) -> bool {
    let entry = |key: &[u8]| {
        dict.get(key).and_then(|raw| match raw.as_reference() {
            Some(id) => objects.resolve(id),
            None => Some(raw),
        })
    };
    let named = |key: &[u8]| entry(key).and_then(Object::as_name);
    let programs = entry(b"FontDescriptor")
        .and_then(Object::as_dict)
        .is_some_and(|d| {
            [&b"FontFile"[..], b"FontFile2", b"FontFile3"]
                .iter()
                .any(|key| d.get(key).is_some())
        });
    named(b"Subtype") == Some(&b"Type1"[..])
        && named(b"BaseFont").and_then(Std14::named) == Some(Std14::ZapfDingbats)
        && dict.get(b"Encoding").is_none()
        && !programs
}

/// One widget of a push button field, read for its appearances: its
/// captions, whether it has rollover and down appearances, the widget's
/// box, border, and `/MK`, and its default appearance string parsed. No
/// font is read: [`font_name`](Self::font_name) names the one to draw in,
/// and [`make`](Self::make) draws in a font the caller keeps.
///
/// `Debug` prints lengths and numbers, never a caption or the font's name.
#[derive(Clone)]
pub struct WidgetPush {
    caption: String,
    rollover_caption: Option<String>,
    down_caption: Option<String>,
    pressed_states: bool,
    look: Look,
}

impl WidgetPush {
    /// The button as [`push_button`](super::push_button) takes it.
    #[must_use]
    pub fn button(&self) -> PushButton<'_> {
        PushButton {
            caption: &self.caption,
            rollover_caption: self.rollover_caption.as_deref(),
            down_caption: self.down_caption.as_deref(),
            pressed_states: self.pressed_states,
        }
    }

    /// The widget's box, colors, rotation, and border.
    #[must_use]
    pub fn look(&self) -> WidgetLook<'_> {
        self.look.look()
    }

    /// The default appearance string parsed: empty where it can't be read;
    /// where there is none, a size of 12 and nothing else, as PDFium as of
    /// chromium/8152 draws a caption with no default appearance.
    #[must_use]
    pub const fn default_appearance(&self) -> &DefaultAppearance {
        &self.look.da
    }

    /// The font resource name the default appearance string gives (`Tf`),
    /// for the caller's own map of fonts; `None` where it gives none, and the
    /// captions are drawn in Helvetica.
    #[must_use]
    pub fn font_name(&self) -> Option<&[u8]> {
        self.look.da.font_name.as_deref()
    }

    /// Makes the widget's appearances in `font`, named by `font_res`:
    /// [`push_button`](super::push_button) on what was read.
    ///
    /// # Errors
    ///
    /// As [`push_button`](super::push_button).
    pub fn make<F: pdf_gfx::FontSet + ?Sized>(
        &self,
        font_res: Res,
        font: &F,
        limits: &Limits,
    ) -> Result<PushAppearances, Unmade> {
        push_button(
            &self.button(),
            &self.look(),
            &self.look.da,
            font_res,
            font,
            limits,
        )
    }
}

impl fmt::Debug for WidgetPush {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WidgetPush")
            .field("button", &self.button())
            .field("look", &self.look)
            .finish()
    }
}

/// Reads `widget` of `field`, a push button field, for its appearances,
/// under `rules`, departures told to `sink`.
///
/// - **The widget** and **the default appearance string** are read as
///   [`widget_text`](super::widget_text) reads a text widget's: the
///   rectangle, the border, `/MK`'s colors, rotation, and captions, and the
///   widget's own `/DA`, else the field's effective one. With none at all,
///   the captions are drawn at 12 points, as PDFium as of chromium/8152
///   draws them; one that names no font draws them in Helvetica at an
///   automatic size, told as [`Fault::MissingDefaultAppearance`] where
///   there is a caption to draw.
/// - **The captions** are `/MK`'s `/CA`, `/RC`, and `/AC`; with `/MK /TP 1`
///   (the icon alone) and no icon, none is shown.
/// - **The rollover and down appearances** are made where the highlighting
///   mode (`/H`) is `/P`, push, or `/T`, toggle (ISO 32000-2, Table 191).
///
/// # Errors
///
/// [`Unmade::NotButton`] for a field that isn't a push button field;
/// [`Unmade::Icon`] for a widget whose `/MK` has an icon (`/I`, a stream)
/// and whose caption isn't shown alone (`/TP` other than `0`), as no icon is
/// drawn; [`Unmade::Limit`] for a cap of [`Limits::annots`] passed;
/// [`Unmade::Unreadable`] on locked, I/O, or reader-cap failures.
pub fn widget_push<'o, R, S>(
    field: &Field<'o>,
    widget: &Widget<'o>,
    objects: R,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<WidgetPush, Unmade>
where
    R: Resolve<'o> + Copy,
    S: Sink<Fault> + ?Sized,
{
    if field.kind() != Some(FieldKind::PushButton) {
        return Err(Unmade::NotButton);
    }
    let (annot, mut look, at) = read_look(field, widget, objects, rules, limits, sink)?;
    let (mk, highlight) = match annot.typed() {
        TypedAnnot::Widget(typed) => (typed.appearance_chars.as_ref(), typed.highlight),
        _ => (None, HighlightMode::default()),
    };
    let icon_only = mk.and_then(|mk| mk.text_position) == Some(1);
    let caption_only = mk.and_then(|mk| mk.text_position).unwrap_or(0) == 0;
    if !caption_only && has_icon(widget, objects, rules, limits)? {
        return Err(Unmade::Icon);
    }
    let caption = |text: Option<&String>| text.filter(|_| !icon_only).cloned();
    let made = WidgetPush {
        caption: caption(mk.and_then(|mk| mk.normal_caption.as_ref())).unwrap_or_default(),
        rollover_caption: caption(mk.and_then(|mk| mk.rollover_caption.as_ref())),
        down_caption: caption(mk.and_then(|mk| mk.down_caption.as_ref())),
        pressed_states: highlight == HighlightMode::Push,
        look: {
            if !look.has_da {
                look.da.font_size = Some(12.0);
            }
            look
        },
    };
    let button = made.button();
    let shown = [
        Some(button.caption),
        button.rollover_caption,
        button.down_caption,
    ];
    let drawn = shown.iter().flatten().any(|caption| !caption.is_empty());
    if drawn && made.look.da.font_name.is_none() && sink.wants() {
        sink.fault(Place::new(at, None), Fault::MissingDefaultAppearance);
    }
    Ok(made)
}

/// Whether `widget`'s `/MK` has a normal icon (`/I`): a stream, reached
/// through its references.
fn has_icon<'o, R: Resolve<'o> + Copy>(
    widget: &Widget<'o>,
    objects: R,
    rules: Rules,
    limits: &Limits,
) -> Result<bool, Unmade> {
    let mut reader = Reader {
        objects,
        rules,
        limits,
        sink: &mut (),
    };
    let Some((mk, within)) = reader.dict_entry(widget.dict(), widget.id(), "MK")? else {
        return Ok(false);
    };
    let Some(raw) = mk.get(b"I") else {
        return Ok(false);
    };
    Ok(reader
        .reached(raw, within)?
        .is_some_and(|(icon, _)| matches!(icon.value(), Value::Stream(_))))
}

/// A push button's appearances made from what the read layer gives
/// ([`push_for_widget`]), with the font the captions are drawn in and the
/// name the default appearance string gives it.
///
/// Where the font is a fallback ([`FieldFont::is_fallback`]), the caller
/// adds it to the default resources (`/DR /Font`) under
/// [`font_name`](Self::font_name), as `MadeText` says of a text field's.
///
/// `Debug` prints the name's length, never the name.
#[non_exhaustive]
pub struct MadePush {
    /// The appearances.
    pub appearances: PushAppearances,
    /// The font the captions are drawn in.
    pub font: FieldFont,
    /// The font resource name the default appearance string gives (`Tf`);
    /// `None` where it gives none, and the captions are drawn in Helvetica.
    pub font_name: Option<Vec<u8>>,
}

impl fmt::Debug for MadePush {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MadePush")
            .field("appearances", &self.appearances)
            .field("font", &self.font)
            .field("font_name_len", &self.font_name.as_ref().map(Vec::len))
            .finish()
    }
}

/// Makes the appearances of `widget` of `field`, a push button field of
/// `form`: [`widget_push`], then the font its default appearance names from
/// the form's default resources ([`Form::field_font`], or Helvetica where it
/// names none), then [`WidgetPush::make`] in it. Departures are told to
/// `sink`.
///
/// # Errors
///
/// As [`widget_push`], [`Form::field_font`], and [`WidgetPush::make`];
/// [`Unmade::FontNotBindable`] for a font the appearance's resources can't
/// name ([`FieldFont::res`]).
pub fn push_for_widget<'o, R, S>(
    form: &Form<'o>,
    field: &Field<'o>,
    widget: &Widget<'o>,
    objects: R,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<MadePush, Unmade>
where
    R: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let read = widget_push(field, widget, objects, rules, limits, sink)?;
    let font = match read.font_name() {
        Some(name) => form.field_font(objects, name, limits, sink)?,
        None => FieldFont::standard(Std14::Helvetica),
    };
    let res = font.res().ok_or(Unmade::FontNotBindable)?;
    let appearances = read.make(res, &font.layout(), limits)?;
    Ok(MadePush {
        appearances,
        font,
        font_name: read.look.da.font_name,
    })
}

/// What a choice field shows: a combo box's text, or a list box's labels,
/// selection, and top.
#[derive(Clone)]
enum Choice {
    Combo(String),
    List {
        labels: Vec<String>,
        selected: Vec<usize>,
        top: usize,
    },
}

/// What a combo box or list box field shows, read once for all its
/// widgets ([`field_choice`]): for a combo box the text in its box, for a
/// list box its options' labels, the options selected, and the one at its
/// top. Every widget of the field shows the same, each in its own box
/// ([`widget_choice`]).
///
/// `Debug` prints lengths and numbers, never the text or a label.
#[derive(Clone)]
pub struct FieldChoice {
    choice: Choice,
}

impl FieldChoice {
    /// Whether the field is a combo box; else a list box.
    #[must_use]
    pub const fn is_combo(&self) -> bool {
        matches!(self.choice, Choice::Combo(_))
    }

    /// The combo box as [`combo_box`](super::combo_box) takes it, with its
    /// drop button; none for a list box.
    #[must_use]
    pub fn combo(&self) -> Option<ComboBox<'_>> {
        match &self.choice {
            Choice::Combo(text) => Some(ComboBox::new(text)),
            Choice::List { .. } => None,
        }
    }

    /// The list box as [`list_box`](super::list_box) takes it, its labels
    /// lent as they're held; none for a combo box.
    #[must_use]
    pub fn list(&self) -> Option<ListBox<'_, String>> {
        match &self.choice {
            Choice::List {
                labels,
                selected,
                top,
            } => {
                let mut list = ListBox::new(labels);
                list.selected = selected;
                list.top = *top;
                Some(list)
            }
            Choice::Combo(_) => None,
        }
    }

    /// A list box's options' labels, in order; none for a combo box.
    #[must_use]
    pub fn labels(&self) -> &[String] {
        match &self.choice {
            Choice::List { labels, .. } => labels,
            Choice::Combo(_) => &[],
        }
    }

    /// The indexes of a list box's selected options, sorted, each once;
    /// none for a combo box.
    #[must_use]
    pub fn selected(&self) -> &[usize] {
        match &self.choice {
            Choice::List { selected, .. } => selected,
            Choice::Combo(_) => &[],
        }
    }

    /// The index of the option at a list box's top (`/TI`); 0 for a combo
    /// box.
    #[must_use]
    pub const fn top(&self) -> usize {
        match &self.choice {
            Choice::List { top, .. } => *top,
            Choice::Combo(_) => 0,
        }
    }
}

impl fmt::Debug for FieldChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("FieldChoice");
        match &self.choice {
            Choice::Combo(text) => debug.field("text_len", &text.len()),
            Choice::List {
                labels,
                selected,
                top,
            } => debug
                .field("labels", &labels.len())
                .field("selected", &selected.len())
                .field("top", top),
        };
        debug.finish()
    }
}

/// Reads what `field`, a combo box or list box field, shows, under
/// `rules`, departures told to `sink`: once for the field, whose widgets
/// each take it ([`widget_choice`]), so its options are decoded once
/// however many widgets it has.
///
/// - **The options** are [`Field::options_with`](crate::Field::options_with)'s
///   labels, and the selection [`selected_options`](crate::selected_options)
///   over them, [`Field::selected_with`](crate::Field::selected_with)'s
///   `/I`, and the value's text strings, as
///   [`Field::selection`](crate::Field::selection) finds it, each decoded
///   once, its departures told. A value that is no text string or array of
///   them is none, told as [`Fault::WrongType`] for `V`.
/// - **A combo box's text** is the label of the option the value's text
///   (an array's first) selects, `/I`'s first index where it agrees, else
///   the first option of that export value; else that text as it is, as an
///   editable combo box's value outside its options is; as PDFium as of
///   chromium/8152 shows it. With no value, none: the default value
///   (`/DV`) is a reset's (ISO 32000-2, Table 226), where PDFium shows it.
/// - **A list box's** labels are every option's, its selection that (none
///   with no value, whatever `/I` has), and its top
///   [`Field::top_index`](crate::Field::top_index).
///
/// # Errors
///
/// [`Unmade::NotChoice`] for a field that isn't a combo box or list box
/// field; [`Unmade::Limit`] for `/Opt` or `/I` past [`Limits::items`];
/// [`Unmade::Unreadable`] on locked, I/O, or reader-cap failures.
pub fn field_choice<'o, R, S>(
    field: &Field<'o>,
    objects: R,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<FieldChoice, Unmade>
where
    R: Resolve<'o> + Copy,
    S: Sink<Fault> + ?Sized,
{
    let combo = match field.kind() {
        Some(FieldKind::Combo) => true,
        Some(FieldKind::List) => false,
        _ => return Err(Unmade::NotChoice),
    };
    let options = field.options_with(objects, rules, limits, &mut *sink)?;
    let indices = field.selected_with(objects, rules, limits, &mut *sink)?;
    let values: Vec<&str> = match field.value() {
        None => Vec::new(),
        Some(FieldValue::Text(text)) => vec![text.as_str()],
        Some(FieldValue::Items(items)) => items.iter().map(String::as_str).collect(),
        Some(_) => {
            if sink.wants() {
                sink.fault(Place::new(field.id(), None), Fault::WrongType { key: "V" });
            }
            Vec::new()
        }
    };
    let choice = match combo {
        true => Choice::Combo(match values.first() {
            Some(value) => match shown_option(&options, &indices, value) {
                Some(index) => options[index].label().to_owned(),
                None => (*value).to_owned(),
            },
            None => String::new(),
        }),
        false => {
            let selected = crate::selected_options(&options, &indices, &values);
            Choice::List {
                labels: (options.into_iter())
                    .map(|option| option.label.unwrap_or(option.export))
                    .collect(),
                selected,
                top: field.top_index(),
            }
        }
    };
    Ok(FieldChoice { choice })
}

/// One widget of a combo box or list box field, read for its appearance:
/// what its field shows ([`FieldChoice`], lent), and the widget's box,
/// border, and `/MK`, and its default appearance string parsed. No font is
/// read: [`font_name`](Self::font_name) names the one to draw in, and
/// [`make`](Self::make) draws in a font the caller keeps.
///
/// `Debug` prints lengths and numbers, never the text, a label, or the
/// font's name.
#[derive(Clone)]
pub struct WidgetChoice<'c> {
    choice: &'c FieldChoice,
    look: Look,
}

impl<'c> WidgetChoice<'c> {
    /// What the widget's field shows, as it was given to [`widget_choice`].
    #[must_use]
    pub const fn choice(&self) -> &'c FieldChoice {
        self.choice
    }

    /// The widget's box, colors, rotation, and border.
    #[must_use]
    pub fn look(&self) -> WidgetLook<'_> {
        self.look.look()
    }

    /// The default appearance string parsed: empty where there is none, or
    /// it can't be read.
    #[must_use]
    pub const fn default_appearance(&self) -> &DefaultAppearance {
        &self.look.da
    }

    /// The font resource name the default appearance string gives (`Tf`),
    /// for the caller's own map of fonts; `None` where it gives none, and the
    /// text is drawn in Helvetica ([`Fault::MissingDefaultAppearance`]).
    #[must_use]
    pub fn font_name(&self) -> Option<&[u8]> {
        self.look.da.font_name.as_deref()
    }

    /// Makes the widget's appearance in `font`, named by `font_res`:
    /// [`combo_box`](super::combo_box) or [`list_box`](super::list_box) on
    /// what was read.
    ///
    /// # Errors
    ///
    /// As [`combo_box`](super::combo_box) or [`list_box`](super::list_box).
    pub fn make<F: pdf_gfx::FontSet + ?Sized>(
        &self,
        font_res: Res,
        font: &F,
        limits: &Limits,
    ) -> Result<Appearance, Unmade> {
        let look = self.look();
        let da = &self.look.da;
        match &self.choice.choice {
            Choice::Combo(text) => {
                combo_box(&ComboBox::new(text), &look, da, font_res, font, limits)
            }
            Choice::List {
                labels,
                selected,
                top,
            } => {
                let mut list = ListBox::new(labels);
                list.selected = selected;
                list.top = *top;
                list_box(&list, &look, da, font_res, font, limits)
            }
        }
    }
}

impl fmt::Debug for WidgetChoice<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WidgetChoice")
            .field("choice", &self.choice)
            .field("look", &self.look)
            .finish()
    }
}

/// Reads `widget` of `field`, a combo box or list box field, for its
/// appearance, under `rules`, departures told to `sink`; `choice` is what
/// the field shows, read once for all its widgets by [`field_choice`].
///
/// **The widget** and **the default appearance string** are read as
/// [`widget_text`](super::widget_text) reads a text widget's: the
/// rectangle, the border, `/MK`'s colors and rotation, and the widget's
/// own `/DA`, else the field's effective one. Where there is none, or it
/// names no font, the text is drawn in Helvetica, and
/// [`Fault::MissingDefaultAppearance`] is told. Nothing of the field's
/// options is read here, so a field's widgets are each read in the time
/// one takes.
///
/// # Errors
///
/// [`Unmade::NotChoice`] for a field that isn't a combo box or list box
/// field; [`Unmade::Limit`] for a cap of [`Limits::annots`] passed;
/// [`Unmade::Unreadable`] on locked, I/O, or reader-cap failures.
pub fn widget_choice<'o, 'c, R, S>(
    field: &Field<'o>,
    widget: &Widget<'o>,
    choice: &'c FieldChoice,
    objects: R,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<WidgetChoice<'c>, Unmade>
where
    R: Resolve<'o> + Copy,
    S: Sink<Fault> + ?Sized,
{
    if !matches!(field.kind(), Some(FieldKind::Combo | FieldKind::List)) {
        return Err(Unmade::NotChoice);
    }
    let (_, look, at) = read_look(field, widget, objects, rules, limits, sink)?;
    if look.da.font_name.is_none() && sink.wants() {
        sink.fault(Place::new(at, None), Fault::MissingDefaultAppearance);
    }
    Ok(WidgetChoice { choice, look })
}

/// The option a combo box shows for its value's first text, `value`, as
/// PDFium as of chromium/8152 finds it (`CPDF_FormField::GetSelectedIndex`):
/// `/I`'s first index where its option's export value is `value`, else the
/// first option whose export value is; none where no option's is.
fn shown_option(options: &[FieldOption], indices: &[usize], value: &str) -> Option<usize> {
    let by_index = indices
        .first()
        .copied()
        .filter(|&index| options.get(index).is_some_and(|o| o.export == value));
    by_index.or_else(|| options.iter().position(|o| o.export == value))
}

/// Makes the appearance of `widget` of `field`, a combo box or list box
/// field of `form`: [`field_choice`] and [`widget_choice`], then the font
/// its default appearance names from the form's default resources
/// ([`Form::field_font`], or Helvetica where it names none), then
/// [`WidgetChoice::make`] in it. Departures are told to `sink`.
///
/// It reads the field's options and the font for each widget: a caller
/// making many widgets calls [`field_choice`] once for a field and
/// [`widget_choice`] for each of its widgets, and keeps a [`FieldFont`] for
/// each font name.
/// What's made is a text field's ([`MadeText`]): the appearance, its font,
/// and the name `/DA` gives it, under which a fallback is added to the
/// default resources.
///
/// # Errors
///
/// As [`field_choice`], [`widget_choice`], [`Form::field_font`], and
/// [`WidgetChoice::make`];
/// [`Unmade::FontNotBindable`] for a font the appearance's resources can't
/// name ([`FieldFont::res`]).
pub fn choice_for_widget<'o, R, S>(
    form: &Form<'o>,
    field: &Field<'o>,
    widget: &Widget<'o>,
    objects: R,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<MadeText, Unmade>
where
    R: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let choice = field_choice(field, objects, rules, limits, sink)?;
    let read = widget_choice(field, widget, &choice, objects, rules, limits, sink)?;
    let font = match read.font_name() {
        Some(name) => form.field_font(objects, name, limits, sink)?,
        None => FieldFont::standard(Std14::Helvetica),
    };
    let res = font.res().ok_or(Unmade::FontNotBindable)?;
    let appearance = read.make(res, &font.layout(), limits)?;
    Ok(MadeText {
        appearance,
        font,
        font_name: read.look.da.font_name,
    })
}
