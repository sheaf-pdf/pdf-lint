//! Read PDF's fonts (ISO 32000-2, 9) as PDFium reads them. No file is read here. A
//! font is read through what gives its objects and its streams' data, pdf-objects'
//! [`Resolve`](pdf_objects::Resolve) and [`Streams`](pdf_objects::Streams). A
//! pdf-structure `&Reading` is both, and [`WithStreams`](pdf_objects::WithStreams)
//! makes both of a resolver and a function.
//!
//! - **Read a file's font** ([`Font::read`]): the codes a string is in it
//!   ([`Font::codes`]), each code's CID, width, vertical metrics and text, how far its
//!   glyphs reach and their boxes, each as PDFium (chromium/8125) reads them. What it
//!   guesses, it says ([`FontApprox`]).
//! - **Read each font once** ([`Fonts`], [`FontScope`]), kept for every page that draws
//!   in it, and charged to each page's budget.
//! - **Read the strings a page's content shows** ([`shown`]), in order, each in its
//!   font, the forms it draws followed to [`Limits::forms`] deep, as PDFium reads them
//!   for its text, with no layout, and drawn [`Limits::form_uses`] times at most. What
//!   isn't read is said ([`Unread`]).
//! - **Look up encodings and glyph names** ([`Encoding`], [`glyph_text`],
//!   [`name_code_point`], [`cp1252`]).
//! - **Find a standard 14 font** ([`Std14::named`]) by a name PDFium takes for it, with
//!   Adobe's metrics.
//!
//! Where a font departs from the standard is a [`Fault`], told with its place to
//! pdf-objects' [`Sink`](pdf_objects::fault::Sink), and the reading goes on past it.
//! With `()` as the sink, none is looked for, and what's read is the same through any
//! sink. Each reading is bounded by [`Limits`] and charged to the caller's budget. It
//! fails only past the budget ([`Error::TooLarge`]), or for what doesn't read for a
//! reason that's the caller's, a cap of what gives a stream among them
//! ([`Error::Unreadable`]). Nothing in this crate panics on what a file holds.
//! pdf-objects and pdf-content-streams are re-exported whole, so their types are named
//! through them at the versions this crate is built with.
//!
//! # Features
//!
//! - **`programs`**, on by default: a font's program read through skrifa, for
//!   [`Font::char_box`], heights and the text its encoding leaves to it
//!   ([`FontProgram`]). [`Font::read_for_text`] and [`Fonts::for_text`] read none.
//! - **`predefined-cmaps`**, off by default: Adobe's predefined CMaps, from hayro-cmap's
//!   bundle. Without it, one of the bundle's reads as a CMap the build hasn't
//!   ([`Fault::PredefinedNotBuilt`]).
//! - **`tracing`**, off by default: a span at `debug` over a font read, and within it
//!   over its program's opening, its embedded CMap's and its `/ToUnicode`'s parsing,
//!   and a Type 3 font's CharProcs; and over a walk of the strings content shows
//!   ([`shown`]). A span's fields are numbers, never a byte of a file.
//!
//! Helvetica-Bold by a name PDFium takes for it, its `/ToUnicode` read through its
//! streams, and the same font past a cap of what gives them:
//!
//! ```
//! use std::collections::BTreeMap;
//!
//! use pdf_fonts::pdf_objects::{
//!     Object, ObjectId, Stream, Unreadable, Why, WithStreams, dict,
//! };
//! use pdf_fonts::{Error, Font, Limits};
//!
//! // Helvetica-Bold, by a name PDFium takes for it, its /ToUnicode object 1.
//! let font = dict! {
//!     "Subtype" => "Type1", "BaseFont" => "Arial,Bold", "Encoding" => "WinAnsiEncoding",
//!     "ToUnicode" => Object::reference(1),
//! };
//! let cmap = b"1 begincodespacerange <00> <FF> endcodespacerange\n\
//!     1 beginbfchar <01> <FB01> endbfchar";
//! let stream = Stream::new(dict! {}, cmap.to_vec());
//! let objects = BTreeMap::from([(ObjectId::new(1, 0), Object::from(stream))]);
//!
//! // Its objects, and its streams' data as they hold it.
//! let resolve = |id: ObjectId| objects.get(&id);
//! let streams = WithStreams::new(&resolve, |id, _| {
//!     let data = objects.get(&id).and_then(Object::as_stream);
//!     let data = data.and_then(|stream| stream.data.bytes()).map(<[u8]>::to_vec);
//!     data.ok_or_else(|| Unreadable::new(id, Why::Damaged, "no such stream"))
//! });
//! let limits = Limits::DEFAULT;
//! let mut budget = 1 << 20;
//! let mut faults = Vec::new();
//! let read = Font::read(streams, &font, &limits, &mut budget, &mut faults).unwrap();
//! assert!(faults.is_empty());
//! assert_eq!(budget, (1 << 20) - cmap.len());
//!
//! // Adobe's widths; the /ToUnicode's text, then WinAnsiEncoding's; heights guessed.
//! assert_eq!(read.width(u32::from(b'A')), 722.0);
//! assert_eq!(read.codes(b"\x01A\x80").count(), 3);
//! assert_eq!(read.text(b"\x01A\x80").as_deref(), Some("\u{FB01}A€"));
//! assert!(read.approx().heights);
//!
//! // A stream past a cap of what gives it is the caller's to know, the budget kept.
//! let capped = WithStreams::new(&resolve, |id, _| {
//!     Err(Unreadable::new(id, Why::Capped, "past the reader's cap"))
//! });
//! let mut budget = 1 << 20;
//! let error = Font::read(capped, &font, &limits, &mut budget, &mut ()).unwrap_err();
//! assert!(matches!(error, Error::Unreadable(u) if u.why == Why::Capped));
//! assert_eq!(budget, 1 << 20);
//! ```
//!
//! The metrics and the glyph lists are Adobe's, each under its own terms, in the file
//! itself. [`Std14::NOTICE`] is the metrics' notice, for a binary's list of notices.
//! What's taken from FreeType is under `FTL.TXT`. The README says what that is, and what
//! a binary must carry of it and of what the features pull in.

pub use pdf_content_streams;
pub use pdf_objects;

pub use encoding::{Encoding, cp1252, glyph_text, name_code_point};
pub use error::Error;
pub use fault::Fault;
pub use font::{
    Codes, Descendant, Font, FontApprox, FontKind, FontMetrics, FontProgram, Rect, VerticalMetrics,
};
pub use fonts::{FontId, FontScope, Fonts};
pub use limits::Limits;
pub use shown::{Shown, ShownString, Unread, shown};
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
mod phases;
mod predefined;
#[cfg_attr(not(feature = "programs"), path = "no_program.rs")]
mod program;
mod reader;
mod shown;
mod std14;
#[cfg(test)]
mod tests;
#[cfg_attr(not(feature = "programs"), allow(dead_code))]
mod type1;

/// The README's example, run as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
