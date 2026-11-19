//! PDF interactive forms (ISO 32000-2, §12.7): the field tree with what each
//! field inherits, its typed value, and its widgets.
//!
//! [`Form::read`] takes a catalog and gives its `/AcroForm` with the field tree
//! read, under ISO 32000-2's rules or PDFium's as of chromium/8152. It works
//! over borrowed [`pdf_objects::Dict`] values and any [`pdf_objects::Resolve`]
//! provider, and does no file I/O.
//!
//! - A [`Field`]'s type, flags, value, default value, default appearance,
//!   quadding, and maximum length are the effective ones: its own, else its
//!   nearest ancestor's, and for the default appearance and quadding the
//!   form's after that.
//! - A [`Widget`] is a kid with no partial name and no kids of its own, or the
//!   field's own dictionary; [`Widget::annot`] reads it as a
//!   [`pdf_annots::Annot`].
//! - The walk ends within [`Limits`] whatever the file says. Recoverable
//!   departures go to a [`pdf_objects::fault::Sink`] as [`Fault`]s; `&mut ()`
//!   collects none and reads the same form.
//! - `Debug` output prints numbers and counts, never a field's name or value.
//!
//! # Example
//!
//! ```
//! use std::collections::BTreeMap;
//!
//! use pdf_forms::pdf_objects::pdf_tokenizer::Rules;
//! use pdf_forms::pdf_objects::{Object, ObjectId, dict};
//! use pdf_forms::{FieldFlags, FieldKind, FieldValue, Form, Limits, Quadding};
//!
//! let (address, city, agree) = (ObjectId::new(2, 0), ObjectId::new(3, 0), ObjectId::new(4, 0));
//!
//! let mut objects = BTreeMap::new();
//! // A field with no widget of its own: it hands its type and `/DA` down.
//! objects.insert(
//!     address,
//!     Object::from(dict! {
//!         "T" => Object::text("address"),
//!         "FT" => Object::name("Tx"),
//!         "DA" => Object::text("/Helv 9 Tf 0 0 0.502 rg"),
//!         "Kids" => vec![Object::from(city)],
//!     }),
//! );
//! // A terminal field that is its own widget.
//! objects.insert(
//!     city,
//!     Object::from(dict! {
//!         "T" => Object::text("city"),
//!         "Parent" => address,
//!         "V" => Object::text("Trenton"),
//!         "Q" => 2,
//!         "Subtype" => Object::name("Widget"),
//!         "Rect" => vec![Object::from(72), Object::from(700), Object::from(200), Object::from(716)],
//!     }),
//! );
//! objects.insert(
//!     agree,
//!     Object::from(dict! {
//!         "T" => Object::text("agree"),
//!         "FT" => Object::name("Btn"),
//!         "V" => Object::name("Yes"),
//!         "Subtype" => Object::name("Widget"),
//!         "Rect" => vec![Object::from(72), Object::from(680), Object::from(82), Object::from(690)],
//!     }),
//! );
//! let catalog = dict! {
//!     "Type" => Object::name("Catalog"),
//!     "AcroForm" => dict! {
//!         "Fields" => vec![Object::from(address), Object::from(agree)],
//!         "DA" => Object::text("/Helv 0 Tf 0 g"),
//!     },
//! };
//!
//! let form = Form::read(&objects, &catalog, Rules::Pdfium, &Limits::DEFAULT).unwrap().unwrap();
//! assert_eq!(form.fields().len(), 3);
//!
//! let city = &form.fields()[form.find("address.city").unwrap()];
//! assert_eq!(city.kind(), Some(FieldKind::Text));
//! assert_eq!(city.value(), Some(&FieldValue::Text("Trenton".into())));
//! assert_eq!(city.default_appearance(), Some(&b"/Helv 9 Tf 0 0 0.502 rg"[..]));
//! assert_eq!(city.quadding(), Quadding::Right);
//! assert!(!city.flags().contains(FieldFlags::MULTILINE));
//!
//! // The widget is the field's own dictionary, read as an annotation.
//! let widget = city.widgets()[0];
//! assert!(widget.is_merged());
//! let annot = widget.annot(&objects, Rules::Pdfium, &Limits::DEFAULT).unwrap();
//! assert_eq!(annot.rect().x1, 200.0);
//!
//! let agree = &form.fields()[form.find("agree").unwrap()];
//! assert_eq!(agree.kind(), Some(FieldKind::CheckBox));
//! assert_eq!(agree.value(), Some(&FieldValue::State(b"Yes")));
//! assert_eq!(agree.default_appearance(), Some(&b"/Helv 0 Tf 0 g"[..]));
//! ```

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

pub use pdf_annots;
pub use pdf_content_streams;
pub use pdf_fonts;
pub use pdf_gfx;
pub use pdf_objects;
pub use pdf_semantics;

mod error;
mod fault;
mod field;
mod form;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod limits;
mod phases;
mod read;
#[cfg(test)]
mod tests;

pub use error::Error;
pub use fault::Fault;
pub use field::{Field, FieldFlags, FieldKind, FieldType, FieldValue, Widget};
pub use form::Form;
pub use limits::{Cap, Limits};
pub use pdf_annots::Quadding;
