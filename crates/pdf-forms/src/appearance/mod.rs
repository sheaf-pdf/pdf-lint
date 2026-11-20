//! Appearances made for fields that have none (`text-layout`).
//!
//! A text widget with a value and no appearance stream (`/AP`) shows nothing
//! in a viewer that doesn't make one. This module makes it, as PDFium as of
//! chromium/8152 makes it for the same field, from plain data.
//!
//! - [`FieldFont`] is the font a field's text is drawn in: the default
//!   resources' font (`/DR /Font`) its default appearance string names, read
//!   with `pdf-fonts`, or a standard 14 font chosen by that name where the
//!   resources have none. It's read once and shared across widgets and
//!   threads; [`FieldFont::layout`] gives what `pdf-gfx`'s text layout
//!   measures and encodes with.

mod font;
#[cfg(test)]
mod tests;

pub use font::{FieldFont, FieldLayoutFont};
