//! PDF's fonts (ISO 32000-2, 9), read.
//!
//! - **A file's font, as read** ([`Font`]): from its dictionary, over pdf-objects'
//!   [`Resolve`](pdf_objects::Resolve) for its objects and
//!   [`Streams`](pdf_objects::Streams) for its streams' data, the codes a string is
//!   in it, each code's CID, width, vertical metrics and text, and how high its glyphs
//!   reach, each as PDFium reads them, what no real writer writes included. What it
//!   can't know it guesses, and says ([`FontApprox`]); what isn't as ISO 32000-2 has
//!   it is told to pdf-objects' [`Sink`](pdf_objects::fault::Sink) ([`Fault`]),
//!   what's read being the same whatever the sink. It fails only where its streams
//!   decompress past the budget it's read within, or where what it reads doesn't read
//!   for a reason that's the caller's ([`Error`]).
//! - **Each font read once** ([`Fonts`], [`FontScope`]): those that are objects, by
//!   number, kept for every page that draws in them, and charged to each.
//! - **Simple fonts' encodings and glyph names** ([`Encoding`], [`glyph_text`],
//!   [`name_code_point`], [`cp1252`]): which glyph a code draws, and the text a
//!   glyph's name stands for, through the Adobe Glyph List and the ITC Zapf
//!   Dingbats Glyph List.
//! - **The standard 14 fonts** ([`Std14`]): the names PDFium takes for each, and
//!   each one's widths, boxes and heights, from Adobe's metrics.
//! - **Programs,** with the `programs` feature, on by default, through skrifa: each
//!   glyph's outline box ([`Font::char_box`]), a font's heights where its descriptor
//!   lacks them, and the text of the codes PDFium's encoding leaves to its program, as
//!   PDFium's FreeType gives them, each bounded by [`Limits`] and charged to the budget
//!   ([`FontProgram`]). Without it, in [`Font::read_for_text`] and in
//!   [`Fonts::for_text`], none's read: a glyph's box from one is none, its heights are
//!   guessed, and those codes' text by its font's name.
//!
//! Nothing panics on what a file holds. A pdf-structure `&Reading` gives a font its
//! objects and streams, and so does anything else that implements pdf-objects'
//! `Resolve` and `Streams`, or [`WithStreams`](pdf_objects::WithStreams), a resolver
//! and a function as one; with `predefined-cmaps`, a Type 0 font's predefined CMap is
//! Adobe's, where without it it reads as one the build hasn't.
//!
//! ```
//! use pdf_fonts::pdf_objects::{Object, ObjectId, Unreadable, Why, WithStreams, dict};
//! use pdf_fonts::{Font, Limits};
//!
//! // A font with no streams, over a resolver of nothing.
//! let none = |_: ObjectId| -> Option<&Object> { None };
//! let objects = WithStreams::new(&none, |id, _| {
//!     Err(Unreadable::new(id, Why::Damaged, "none"))
//! });
//! let font = dict! {
//!     "Subtype" => "Type1", "BaseFont" => "Arial,Bold", "Encoding" => "WinAnsiEncoding",
//! };
//! let mut budget = 1 << 20;
//! let mut faults = Vec::new();
//! let limits = Limits::DEFAULT;
//! let read = Font::read(objects, &font, &limits, &mut budget, &mut faults).unwrap();
//! assert_eq!(read.width(u32::from(b'A')), 722.0);
//! assert_eq!(read.text(b"A\x80").as_deref(), Some("A€"));
//! assert!(read.approx().heights && faults.is_empty());
//! ```
//!
//! pdf-objects is re-exported whole, none of its items by name, so what a font is
//! read over and told to, a resolver, a `Streams` or a sink of one's own, names
//! pdf-objects' items at the version this crate takes (`pdf_fonts::pdf_objects::Dict`);
//! and pdf-content-streams, whose `Limits` are what a CMap and a CharProc are lexed
//! under, is too.
//!
//! The metrics and the glyph lists are Adobe's, each under its own terms, in the
//! file itself. [`Std14::NOTICE`] is the metrics' notice, for a binary's list of
//! notices. What's taken from FreeType is under `FTL.TXT`, and the README says
//! what that is, and what a binary must carry of it and of what the features pull
//! in too.

pub use pdf_content_streams;
pub use pdf_objects;

pub use encoding::{Encoding, cp1252, glyph_text, name_code_point};
pub use error::Error;
pub use fault::Fault;
pub use font::{
    Descendant, Font, FontApprox, FontKind, FontMetrics, FontProgram, Rect, VerticalMetrics,
};
pub use fonts::{FontId, FontScope, Fonts};
pub use limits::Limits;
pub use std14::Std14;

mod afm;
mod cmap;
mod encoding;
mod error;
mod fault;
#[cfg(any(test, fuzzing))]
mod fixture;
mod font;
mod fonts;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod limits;
mod predefined;
#[cfg_attr(not(feature = "programs"), path = "no_program.rs")]
mod program;
mod reader;
mod std14;
#[cfg(test)]
mod tests;
#[cfg_attr(not(feature = "programs"), allow(dead_code))]
mod type1;

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
