//! PDF's fonts (ISO 32000-2, 9), read.
//!
//! - **Simple fonts' encodings and glyph names** ([`Encoding`], [`glyph_text`],
//!   [`name_code_point`], [`cp1252`]): which glyph a code draws, and the text a
//!   glyph's name stands for, through the Adobe Glyph List and the ITC Zapf
//!   Dingbats Glyph List.
//! - **The standard 14 fonts** ([`Std14`]): the names PDFium takes for each, and
//!   each one's widths, boxes and heights, from Adobe's metrics.
//! - **Type 1 tokens** ([`type1::scan`]): a Type 1 program read as far as its
//!   charstrings by glyph name, bounded by [`Limits`].
//!
//! Nothing here reads a file's font dictionary yet. Nothing panics on what it's
//! given, and what's read here takes no file's bytes but a program's and a
//! glyph's name.
//!
//! [`Dict`], [`Object`], [`ObjectId`], [`Resolve`], [`Span`], [`Unreadable`] and
//! [`Why`] are pdf-objects', re-exported.
//!
//! The metrics and the glyph lists are Adobe's, each under its own terms, in the
//! file itself. [`Std14::NOTICE`] is the metrics' notice, for a binary's list of
//! notices, and the README says what a binary must carry.

pub use pdf_objects::{Dict, Object, ObjectId, Resolve, Span, Unreadable, Why};

pub use encoding::{Encoding, cp1252, glyph_text, name_code_point};
pub use font::{FontMetrics, Rect};
pub use limits::Limits;
pub use std14::Std14;

mod afm;
mod encoding;
#[cfg(any(test, fuzzing))]
mod fixture;
mod font;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod limits;
// A font's reading is what calls these; only the tests and the fuzz body do yet.
#[allow(dead_code)]
mod predefined;
mod std14;
#[cfg(test)]
mod tests;
pub mod type1;

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
