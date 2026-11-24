//! A widget's appearance from what the read layer gives: its field's text,
//! flags, and default appearance, and the widget's box, border, and `/MK`.

use std::borrow::Cow;
use std::fmt;

use pdf_annots::{BorderStyle, Quadding, TypedAnnot};
use pdf_content_streams::{Color, Res};
use pdf_fonts::Std14;
use pdf_gfx::{BoxBorder, BoxBorderStyle, DefaultAppearance, FontSet};
use pdf_objects::fault::{Place, Sink};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{ObjectId, Resolve, Streams, Why, text};

use super::font::FieldFont;
use super::frame::{Appearance, Unmade, WidgetLook};
use super::text::TextField;
#[cfg(feature = "embed")]
use super::text::{Fallback, MadeWithFallback};
use crate::fault::Fault;
use crate::field::{Field, FieldFlags, FieldKind, FieldValue, Widget};
use crate::form::Form;
use crate::limits::{Cap, Limits};
use crate::read::Reader;

/// One widget of a text field, read for its appearance: the text, the
/// field's flags, maximum length, and quadding, the widget's box, border,
/// and `/MK`, and the default appearance string parsed. No font is read:
/// [`font_name`](Self::font_name) names the one to draw in, and
/// [`make`](Self::make) draws in a font the caller keeps, so a form's
/// widgets share a handful of fonts, each read once.
///
/// `Debug` prints lengths and numbers, never the text or the font's name.
#[derive(Clone)]
pub struct WidgetText<'a> {
    text: Cow<'a, str>,
    flags: FieldFlags,
    max_len: Option<u32>,
    quadding: Quadding,
    width: f32,
    height: f32,
    background: Option<Color>,
    border_color: Option<Color>,
    rotation: i32,
    border_width: f32,
    border_style: BoxBorderStyle,
    dash: Vec<f32>,
    da: DefaultAppearance,
}

impl WidgetText<'_> {
    /// The field as [`text`](super::text) takes it: the text, flags, maximum
    /// length, and quadding read.
    #[must_use]
    pub fn field(&self) -> TextField<'_> {
        TextField {
            text: &self.text,
            flags: self.flags,
            max_len: self.max_len,
            quadding: self.quadding,
        }
    }

    /// The widget's box, colors, rotation, and border as
    /// [`text`](super::text) takes them.
    #[must_use]
    pub fn look(&self) -> WidgetLook<'_> {
        WidgetLook {
            width: self.width,
            height: self.height,
            background: self.background,
            border_color: self.border_color,
            rotation: self.rotation,
            border: BoxBorder::new(self.border_width, self.border_style).with_dash(&self.dash),
        }
    }

    /// The default appearance string parsed: empty where there is none, or
    /// it can't be read.
    #[must_use]
    pub const fn default_appearance(&self) -> &DefaultAppearance {
        &self.da
    }

    /// The font resource name the default appearance string gives (`Tf`),
    /// for the caller's own map of fonts; `None` where it gives none, and the
    /// text is drawn in Helvetica ([`Fault::MissingDefaultAppearance`]).
    #[must_use]
    pub fn font_name(&self) -> Option<&[u8]> {
        self.da.font_name.as_deref()
    }

    /// Makes the widget's appearance in `font`, named by `font_res`:
    /// [`text`](super::text) on what was read.
    ///
    /// # Errors
    ///
    /// As [`text`](super::text).
    pub fn make<F: FontSet + ?Sized>(
        &self,
        font_res: Res,
        font: &F,
        limits: &Limits,
    ) -> Result<Appearance, Unmade> {
        super::text(
            &self.field(),
            &self.look(),
            &self.da,
            font_res,
            font,
            limits,
        )
    }

    /// Makes the widget's appearance in `font`, named by `font_res`, and
    /// `fallback` for what `font` can't draw:
    /// [`text_with_fallback`](super::text_with_fallback) on what was read
    /// (`embed`).
    ///
    /// # Errors
    ///
    /// As [`text_with_fallback`](super::text_with_fallback).
    #[cfg(feature = "embed")]
    pub fn make_with_fallback<F: FontSet + ?Sized>(
        &self,
        font_res: Res,
        font: &F,
        fallback: &Fallback<'_>,
        limits: &Limits,
    ) -> Result<MadeWithFallback, Unmade> {
        super::text_with_fallback(
            &self.field(),
            &self.look(),
            &self.da,
            font_res,
            font,
            fallback,
            limits,
        )
    }
}

impl fmt::Debug for WidgetText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WidgetText")
            .field("field", &self.field())
            .field("look", &self.look())
            .field("da", &self.da)
            .finish()
    }
}

/// Reads `widget` of `field`, a text field, for its appearance, under
/// `rules`, departures told to `sink`; `display` is the text to show, or
/// `None` for the field's value.
///
/// - **The widget** is read as an annotation (`Widget::annot_with`): its
///   rectangle, border (its border style dictionary's, else `/Border`'s),
///   and `/MK`'s background, border color, and rotation.
/// - **The default appearance string** is the widget's own `/DA`, else the
///   field's effective one (its own, an ancestor's, or the form's), parsed
///   once under the cap `pdf-annots` parses one under
///   ([`Limits::annots`]'s `content`), what it passes over told as
///   [`Fault::DefaultAppearance`] in the object it was read from.
///   `pdf-annots`' own coarser fault for a widget's `/DA` isn't passed on.
///   Where there is none, or it names no font, the text is drawn in
///   Helvetica at an automatic size, as PDFium draws it, and
///   [`Fault::MissingDefaultAppearance`] is told.
/// - **The quadding** is the widget's own `/Q`, else the field's.
/// - **The value:** a text string as it is; a text stream's data (ISO
///   32000-2, §12.7.5.3) decoded within [`Limits::text`] and read as a text
///   string under `rules`; any other value is no text, told as
///   [`Fault::WrongType`] for `V` (a damaged stream as
///   [`Fault::DamagedObject`]).
///
/// # Errors
///
/// [`Unmade::NotText`] for a field that isn't a text field;
/// [`Unmade::Limit`] for a cap of [`Limits::annots`] passed, or a value's
/// stream past [`Limits::text`] ([`Cap::Text`]); [`Unmade::Unreadable`] on
/// locked, I/O, or reader-cap failures.
pub fn widget_text<'a, 'o: 'a, R, S>(
    field: &'a Field<'o>,
    widget: &Widget<'o>,
    objects: R,
    rules: Rules,
    display: Option<&'a str>,
    limits: &Limits,
    sink: &mut S,
) -> Result<WidgetText<'a>, Unmade>
where
    R: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    if field.kind() != Some(FieldKind::Text) {
        return Err(Unmade::NotText);
    }
    let annot = widget.annot_with(objects, rules, limits, &mut WithoutDa(&mut *sink))?;
    let (mk, own_quadding) = match annot.typed() {
        TypedAnnot::Widget(typed) => (typed.appearance_chars.as_ref(), typed.quadding),
        _ => (None, None),
    };
    let border = annot.border();
    let border_style = match border.style {
        BorderStyle::Dashed => BoxBorderStyle::Dashed,
        BorderStyle::Beveled => BoxBorderStyle::Beveled,
        BorderStyle::Inset => BoxBorderStyle::Inset,
        BorderStyle::Underline => BoxBorderStyle::Underline,
        _ => BoxBorderStyle::Solid,
    };

    // The widget's own `/DA`, where it's a dictionary of its own, else the
    // field's: each with the object it's read from. The entry's own
    // departures were told as the widget was read.
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
    let da = parsed.unwrap_or_default();
    if da.font_name.is_none() && sink.wants() {
        sink.fault(Place::new(at, None), Fault::MissingDefaultAppearance);
    }

    let text = match display {
        Some(display) => Cow::Borrowed(display),
        None => value_text(field, objects, rules, limits, sink)?,
    };
    let rect = annot.rect();
    Ok(WidgetText {
        text,
        flags: field.flags(),
        max_len: field.max_len(),
        quadding: own_quadding.unwrap_or(field.quadding()),
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
        da,
    })
}

/// A text widget's appearance made from what the read layer gives
/// ([`text_for_widget`]), with the font it's drawn in and the name the
/// default appearance string gives it.
///
/// Where the font is a fallback ([`FieldFont::is_fallback`]), the caller
/// adds it to the default resources (`/DR /Font`) under
/// [`font_name`](Self::font_name), so a viewer that makes the appearance
/// anew finds it. With no name, no default appearance string names a font
/// at all ([`Fault::MissingDefaultAppearance`]): an entry in the default
/// resources repairs nothing by itself, and the caller writes the field a
/// `/DA` naming the entry it adds as well.
///
/// `Debug` prints the name's length, never the name.
#[non_exhaustive]
pub struct MadeText {
    /// The appearance.
    pub appearance: Appearance,
    /// The font it's drawn in.
    pub font: FieldFont,
    /// The font resource name the default appearance string gives (`Tf`),
    /// which the font is, or is to be, in the default resources under;
    /// `None` where it gives none, and the text is drawn in Helvetica.
    pub font_name: Option<Vec<u8>>,
}

impl fmt::Debug for MadeText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MadeText")
            .field("appearance", &self.appearance)
            .field("font", &self.font)
            .field("font_name_len", &self.font_name.as_ref().map(Vec::len))
            .finish()
    }
}

/// Makes the appearance of `widget` of `field`, a text field of `form`:
/// [`widget_text`], then the font its default appearance names from the
/// form's default resources ([`Form::field_font`], or Helvetica where it
/// names none), then [`WidgetText::make`] in it. Departures are told to
/// `sink`; `display` is the text to show, or `None` for the field's value.
///
/// It reads the font for each widget: a caller making many widgets calls
/// [`widget_text`] for each and keeps a [`FieldFont`] for each font name.
///
/// Text fields alone are made, as the name says: a button's or a choice
/// field's widget is [`Unmade::NotText`].
///
/// # Errors
///
/// As [`widget_text`], [`Form::field_font`], and [`WidgetText::make`];
/// [`Unmade::FontNotBindable`] for a font the appearance's resources can't
/// name ([`FieldFont::res`]).
#[allow(clippy::too_many_arguments)]
pub fn text_for_widget<'o, R, S>(
    form: &Form<'o>,
    field: &Field<'o>,
    widget: &Widget<'o>,
    objects: R,
    rules: Rules,
    display: Option<&str>,
    limits: &Limits,
    sink: &mut S,
) -> Result<MadeText, Unmade>
where
    R: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let read = widget_text(field, widget, objects, rules, display, limits, sink)?;
    let font = match read.font_name() {
        Some(name) => form.field_font(objects, name, limits, sink)?,
        None => FieldFont::standard(Std14::Helvetica),
    };
    let res = font.res().ok_or(Unmade::FontNotBindable)?;
    let appearance = read.make(res, &font.layout(), limits)?;
    Ok(MadeText {
        appearance,
        font,
        font_name: read.da.font_name,
    })
}

/// The text a text field's value gives: a text string's, or a text stream's
/// data read as one; none for any other value, told.
fn value_text<'a, 'o: 'a, R, S>(
    field: &'a Field<'o>,
    objects: R,
    rules: Rules,
    limits: &Limits,
    sink: &mut S,
) -> Result<Cow<'a, str>, Unmade>
where
    R: Resolve<'o> + Streams + Copy,
    S: Sink<Fault> + ?Sized,
{
    let fault = match field.value() {
        None => return Ok(Cow::Borrowed("")),
        Some(FieldValue::Text(text)) => return Ok(Cow::Borrowed(text)),
        Some(FieldValue::Stream(Some(id))) => match objects.decoded(*id, limits.text) {
            Ok(data) => return Ok(Cow::Owned(text::decode_str(&data, rules))),
            Err(unreadable) if unreadable.why == Why::Limit => {
                return Err(Unmade::Limit(Cap::Text));
            }
            Err(unreadable) if unreadable.why == Why::Damaged => Fault::DamagedObject(*id),
            Err(unreadable) => return Err(Unmade::Unreadable(unreadable)),
        },
        Some(_) => Fault::WrongType { key: "V" },
    };
    if sink.wants() {
        sink.fault(Place::new(field.id(), None), fault);
    }
    Ok(Cow::Borrowed(""))
}

/// Passes a widget's faults on but `pdf-annots`' for its `/DA`, which the
/// default appearance string's own parse tells more finely.
struct WithoutDa<'s, S: ?Sized>(&'s mut S);

impl<S: Sink<Fault> + ?Sized> Sink<Fault> for WithoutDa<'_, S> {
    fn fault(&mut self, at: Place, fault: Fault) {
        if fault != Fault::Annot(pdf_annots::Fault::InvalidDefaultAppearance) {
            self.0.fault(at, fault);
        }
    }

    fn wants(&self) -> bool {
        self.0.wants()
    }
}

/// Passes a default appearance string's faults on as this crate's, placed
/// in the object the string was read from; one with no font is said by
/// [`Fault::MissingDefaultAppearance`] instead.
struct DaFaults<'s, S: ?Sized> {
    sink: &'s mut S,
    at: Option<ObjectId>,
}

impl<S: Sink<Fault> + ?Sized> Sink<pdf_gfx::Fault> for DaFaults<'_, S> {
    fn fault(&mut self, place: Place, fault: pdf_gfx::Fault) {
        if matches!(fault, pdf_gfx::Fault::DefaultAppearanceNoFont) {
            return;
        }
        let place = match (place.object, self.at) {
            (None, Some(id)) => place.in_object(id),
            _ => place,
        };
        self.sink.fault(place, Fault::from(fault));
    }

    fn wants(&self) -> bool {
        self.sink.wants()
    }
}
