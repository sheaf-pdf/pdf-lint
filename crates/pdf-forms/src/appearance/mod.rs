//! Appearances made for fields that have none (`text-layout`).
//!
//! A text widget with a value, a button, or a choice field's widget with no
//! appearance stream (`/AP`) shows nothing in a viewer that doesn't make one.
//! This module makes it, as PDFium as of chromium/8152 makes it for the same
//! field (a check box's or radio button's mark aside, which is drawn in
//! ZapfDingbats), from plain data.
//!
//! - [`text`] makes a text field's appearance from plain data: a
//!   [`TextField`] (its text, flags, maximum length, and quadding), a
//!   [`WidgetLook`] (the widget's box, colors, rotation, and border), its
//!   parsed default appearance string, and a font. It gives an
//!   [`Appearance`], or why one couldn't be made ([`Unmade`]).
//! - [`check_box`] and [`radio_button`] make a check box's and a radio
//!   button's from plain data (a [`ToggleButton`], a [`WidgetLook`], the
//!   parsed default appearance, and the resource ZapfDingbats is drawn
//!   under): the normal and the down appearance of its on state and of
//!   `Off` ([`ToggleAppearances`]), its mark the ZapfDingbats glyph its
//!   caption names ([`Mark`]), and no `Off` where it would paint nothing;
//!   [`push_button`] makes a push button's normal, rollover, and down
//!   appearances ([`PushAppearances`]) from a [`PushButton`] in a font.
//!   [`widget_toggle`] and [`widget_push`] read a widget of a
//!   [`Form`](crate::Form)'s buttons into that data, and
//!   [`toggle_for_widget`] and [`push_for_widget`] make one widget's.
//! - [`combo_box`] and [`list_box`] make a combo box's and a list box's
//!   from plain data: a [`ComboBox`] (the text it shows, and whether its
//!   drop button is drawn) or a [`ListBox`] (its options' labels, those
//!   selected, and the one at its top), a [`WidgetLook`], the parsed default
//!   appearance, and a font; [`widget_choice`] reads a widget of a
//!   [`Form`](crate::Form)'s choice fields into that data, and
//!   [`choice_for_widget`] makes one widget's.
//! - [`FieldFont`] is the font a field's text is drawn in: the default
//!   resources' font (`/DR /Font`) its default appearance string names, read
//!   with `pdf-fonts`, or a standard 14 font chosen by that name where the
//!   resources have none. It's read once and shared across widgets and
//!   threads; [`FieldFont::layout`] gives what `pdf-gfx`'s text layout
//!   measures and encodes with.
//! - [`widget_text`] reads a widget of a [`Form`](crate::Form)'s text field
//!   into that plain data, and [`WidgetText::make`] draws it in a font the
//!   caller keeps; [`text_for_widget`] does both and reads the font, for one
//!   widget.
//! - With `embed`, `text_with_fallback` draws the text in the field's font
//!   and a TrueType program the caller gives (`Fallback`), for what that font
//!   can't draw, and returns the program's glyphs it drew
//!   (`MadeWithFallback`), which the caller embeds once for every appearance;
//!   `WidgetText::make_with_fallback` does it for a widget read.
//!
//! One widget's appearance made and bound, with [`text_for_widget`]:
//!
//! ```
//! use std::collections::BTreeMap;
//!
//! use pdf_forms::appearance::text_for_widget;
//! use pdf_forms::pdf_content_streams::ResBody;
//! use pdf_forms::pdf_fonts::Std14;
//! use pdf_forms::pdf_gfx::Matrix;
//! use pdf_forms::pdf_objects::pdf_tokenizer::Rules;
//! use pdf_forms::pdf_objects::{Object, ObjectId, Unreadable, Why, WithStreams, dict};
//! use pdf_forms::{Form, Limits};
//!
//! let (name, helv) = (ObjectId::new(2, 0), ObjectId::new(3, 0));
//! let mut objects = BTreeMap::new();
//! // A text field that is its own widget, with a value and no `/AP`.
//! objects.insert(
//!     name,
//!     Object::from(dict! {
//!         "T" => Object::text("name"),
//!         "FT" => Object::name("Tx"),
//!         "V" => Object::text("Ada"),
//!         "DA" => Object::text("/Helv 12 Tf 0 g"),
//!         "Subtype" => Object::name("Widget"),
//!         "Rect" => vec![Object::from(72), Object::from(700), Object::from(172), Object::from(720)],
//!     }),
//! );
//! objects.insert(helv, Object::from(Std14::Helvetica.dict()));
//! let catalog = dict! {
//!     "AcroForm" => dict! {
//!         "Fields" => vec![Object::from(name)],
//!         "DR" => dict! { "Font" => dict! { "Helv" => helv } },
//!     },
//! };
//! // A font's streams are read through the same source; these objects have none.
//! let source = WithStreams::new(&objects, |id: ObjectId, _| -> Result<Vec<u8>, Unreadable> {
//!     Err(Unreadable::new(id, Why::Damaged, "no streams".to_string()))
//! });
//!
//! let limits = Limits::DEFAULT;
//! let form = Form::read(&source, &catalog, Rules::Iso, &limits).unwrap().unwrap();
//! let field = &form.fields()[form.find("name").unwrap()];
//! let widget = &field.widgets()[0];
//! let made = text_for_widget(&form, field, widget, &source, Rules::Iso, None, &limits, &mut ()).unwrap();
//!
//! // The form XObject to write as the widget's `/AP /N`: its `/BBox`, its
//! // `/Matrix`, its content, and in its `/Resources` each resource the content
//! // names, here the default resources' own font, by its object number.
//! let appearance = &made.appearance;
//! assert_eq!((appearance.bbox.width(), appearance.bbox.height()), (100.0, 20.0));
//! assert_eq!(appearance.matrix, Matrix::IDENTITY);
//! let mut fonts = dict! {};
//! for need in &appearance.needs.0 {
//!     match need.res.body() {
//!         ResBody::Written(id) => fonts.set(need.name.clone(), *id),
//!         // A fallback font's dictionary, which the caller writes as an object.
//!         other => panic!("not an object of the file: {other:?}"),
//!     }
//! }
//! assert_eq!(fonts.len(), 1);
//! assert!(appearance.content.ends_with(b"(Ada) Tj\nET\nQ\nEMC\n"));
//! // The font was found in `/DR` under the name `/DA` gives, so `/DR` is whole.
//! assert!(!made.font.is_fallback());
//! assert_eq!(made.font_name.as_deref(), Some(&b"Helv"[..]));
//! ```

mod button;
mod choice;
mod font;
mod frame;
mod line;
mod read;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_buttons;
#[cfg(test)]
mod tests_choices;
mod text;
mod widget;

pub use button::{
    Mark, PushAppearances, PushButton, ToggleAppearances, ToggleButton, check_box, push_button,
    radio_button,
};
pub use choice::{ComboBox, ListBox, combo_box, list_box};
pub use font::{FieldFont, FieldLayoutFont};
pub use frame::{Appearance, Unmade, WidgetLook};
pub use read::{
    MadePush, MadeToggle, WidgetChoice, WidgetPush, WidgetToggle, choice_for_widget,
    push_for_widget, toggle_for_widget, widget_choice, widget_push, widget_toggle,
};
#[cfg(any(test, fuzzing))]
pub(crate) use text::REACH;
#[cfg(feature = "embed")]
pub use text::{Fallback, MadeWithFallback, text_with_fallback};
pub use text::{TextField, text};
pub use widget::{MadeText, WidgetText, text_for_widget, widget_text};
