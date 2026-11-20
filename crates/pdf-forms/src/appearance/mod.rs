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
//!   caller keeps; [`for_widget`] does both and reads the font, for one
//!   widget.

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
pub use widget::{Made, WidgetText, for_widget, widget_text};
