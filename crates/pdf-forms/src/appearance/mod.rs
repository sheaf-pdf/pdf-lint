//! Appearances made for fields that have none (`text-layout`).
//!
//! A text widget with a value and no appearance stream (`/AP`) shows nothing
//! in a viewer that doesn't make one. This module makes it, as PDFium as of
//! chromium/8152 makes it for the same field, from plain data.
//!
//! - [`text`] makes a text field's appearance from plain data: a
//!   [`TextField`] (its text, flags, maximum length, and quadding), a
//!   [`WidgetLook`] (the widget's box, colors, rotation, and border), its
//!   parsed default appearance string, and a font. It gives an
//!   [`Appearance`], or why one couldn't be made ([`Unmade`]).
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

mod font;
mod frame;
#[cfg(test)]
mod tests;
mod text;
mod widget;

pub use font::{FieldFont, FieldLayoutFont};
pub use frame::{Appearance, Unmade, WidgetLook};
#[cfg(any(test, fuzzing))]
pub(crate) use text::REACH;
pub use text::{TextField, text};
pub use widget::{MadeText, WidgetText, text_for_widget, widget_text};
