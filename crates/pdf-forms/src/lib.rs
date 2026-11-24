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
//!   form's after that; under PDFium's rules, a maximum length neither gives
//!   is the first of the field's widgets' to hold one ([`Field::max_len`]).
//! - A [`Widget`] is a kid with no partial name and no kids of its own, or the
//!   field's own dictionary; [`Widget::annot`] reads it as a
//!   [`pdf_annots::Annot`].
//! - A choice field's options and selection, and a button field's export
//!   values ([`Field::options`], [`Field::selected`], [`Field::selection`]),
//!   are found with the field and decoded when they're asked for
//!   ([`Field::top_index`] is read with it), and [`Widget::states`] reads a
//!   widget's on state and `/AS`. [`group`] holds a check box or radio button field's
//!   rules as pure functions of plain data: which widgets a press or a value
//!   turns on, and what each widget's `/AS` and the field's `/V` become
//!   ([`Field::group`] reads one). With `text-layout`,
//!   `appearance::check_box`, `radio_button`, and `push_button` make a
//!   button's appearances, and `appearance::combo_box` and `list_box` a
//!   choice field's, from plain data; `appearance::toggle_for_widget`,
//!   `push_for_widget`, and `choice_for_widget` make a `Widget`'s from what
//!   this crate reads.
//! - The walk ends within [`Limits`] whatever the file says. Recoverable
//!   departures go to a [`pdf_objects::fault::Sink`] as [`Fault`]s; `&mut ()`
//!   collects none and reads the same form.
//! - `Debug` output prints numbers and counts, never a field's name or value,
//!   an option, a caption, or a state's name, but for the one character an
//!   appearance couldn't be drawn for (`appearance::Unmade::Unencodable`,
//!   `UnencodableOption`) and the one ZapfDingbats code a caption names for
//!   a mark (`appearance::Mark::Other`).
//! - With `text-layout` (a default feature), the `appearance` module makes the
//!   appearances a field without one would show, held to those PDFium makes:
//!   `appearance::text` makes a text field's from plain data, in the font
//!   `FieldFont` reads, and `appearance::text_for_widget` makes a `Widget`'s from
//!   what this crate reads. With `embed` too, `appearance::text_with_fallback`
//!   draws what the field's font can't in a TrueType program the caller gives,
//!   and returns the glyphs it drew, for the caller to embed.
//! - [`default_resources::font_name`] gives the name a writer puts a font under
//!   in the form's default resources, where no other font holds it, for a
//!   default appearance string to name it by.
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

// The README's second example makes an appearance, which `text-layout` adds.
#[cfg(all(doctest, feature = "text-layout"))]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

pub use pdf_annots;
pub use pdf_content_streams;
pub use pdf_fonts;
pub use pdf_gfx;
pub use pdf_objects;
pub use pdf_semantics;

#[cfg(feature = "text-layout")]
pub mod appearance;
pub mod default_resources;
mod error;
mod fault;
mod field;
mod form;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
pub mod group;
mod limits;
mod phases;
mod read;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_buttons;
#[cfg(test)]
mod tests_group;

pub use error::Error;
pub use fault::Fault;
pub use field::{
    Field, FieldFlags, FieldKind, FieldOption, FieldType, FieldValue, Widget, WidgetStates,
    selected_options,
};
pub use form::Form;
pub use limits::{Cap, Limits};
pub use pdf_annots::Quadding;
