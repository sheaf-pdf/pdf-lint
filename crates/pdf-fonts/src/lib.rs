//! PDF font reader, text extractor, Standard 14 metrics, CMap interpreter, and glyph bounding-box
//! reader (ISO 32000-2, §9), matching PDFium's behavior as of chromium/8152; and, with `embed`, a
//! TrueType font subsetter and embedder.
//!
//! `pdf-fonts` operates over any [`Resolve`](pdf_objects::Resolve) and
//! [`Streams`](pdf_objects::Streams) provider (such as a `pdf-structure` `&Reading` or
//! [`WithStreams`](pdf_objects::WithStreams)) without performing file I/O directly.
//!
//! # Core Concepts
//!
//! - **Font parsing & metrics ([`Font::read`]):** Splits strings into character codes
//!   ([`Font::codes`]) and resolves CIDs, widths, vertical metrics, glyph bounding boxes, and
//!   Unicode text across Type 1, MMType1, TrueType, Type 3, and Type 0 fonts, tracking estimated
//!   values in [`FontApprox`].
//! - **Caching ([`Fonts`], [`FontScope`]):** Shares parsed fonts across pages while charging
//!   stream and glyph-outline budgets per page.
//! - **Page text extraction ([`shown`]):** Walks page content streams and nested Form XObjects
//!   (up to [`Limits::forms`] deep and [`Limits::form_uses`] draws) in content order, reporting
//!   skipped or undecodable streams in [`Unread`].
//! - **Encodings & Standard 14 fonts ([`Encoding`], [`Std14`]):** Maps codes and glyph names to
//!   Unicode ([`glyph_text`], [`name_code_point`], [`cp1252`]), resolves Standard 14 fonts and
//!   PDFium aliases ([`Std14::named`]) with Adobe metrics, and builds Standard 14 font
//!   dictionaries ([`Std14::dict`]) and encoded strings ([`Std14::codes`], [`Encoding::code`],
//!   refusing unmapped characters via [`Unencodable`]).
//! - **Font embedding (the `embed` module, with the `embed` feature):** Opens a TrueType
//!   program for embedding, encodes strings as two-byte codes for a Type 0 font over
//!   `/Identity-H` (each the glyph's number in the program, unshaped), records the glyphs a
//!   document draws with their text, and writes the font: its program subset to those glyphs
//!   as `/FontFile2` (ISO 32000-2, 9.9), with `/W`, `/CIDToGIDMap`, a descriptor and a
//!   `/ToUnicode` CMap, as `pdf-objects` values.
//!
//! Spec departures emit [`Fault`]s to a [`Sink`](pdf_objects::fault::Sink) (skipped when `()` is
//! passed). Reads enforce [`Limits`] and caller byte budgets without panicking, failing only when
//! a budget is exceeded ([`Error::TooLarge`]) or the underlying provider fails
//! ([`Error::Unreadable`]). `pdf-objects` and `pdf-content-streams` are re-exported at
//! [`pdf_objects`] and [`pdf_content_streams`].
//!
//! # Cargo Features
//!
//! - **`programs`** *(default)*: Parses embedded font programs via `skrifa` for
//!   [`Font::char_box`], fallback heights, and program character mappings ([`FontProgram`]).
//!   [`Font::read_for_text`] and [`Fonts::for_text`] skip program parsing.
//! - **`predefined-cmaps`** *(optional)*: Bundles Adobe's predefined CJK CMaps via `hayro-cmap`
//!   (otherwise reporting [`Fault::PredefinedNotBuilt`]).
//! - **`tracing`** *(optional)*: Emits numeric `tracing` spans over font reads, program/CMap/CharProc
//!   parsing, [`shown`] walks, and (with `embed`) font writes.
//! - **`embed`** *(optional)*: Opens a TrueType program for embedding (the `embed` module): the
//!   codes text is drawn in, the glyphs a document draws, and the font written as a Type 0 font
//!   over `/Identity-H`, its program cut to them via `subsetter`, read through `skrifa`. CFF,
//!   CFF2 and bitmap-only programs are refused. Independent of `programs`, which is the
//!   reading's.
//!
//! # Example
//!
//! Read `Arial,Bold` (mapped to `Helvetica-Bold`) with a `/ToUnicode` CMap stream and verify
//! budget accounting on capped streams:
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
//!     "Subtype" => Object::name("Type1"), "BaseFont" => Object::name("Arial,Bold"),
//!     "Encoding" => Object::name("WinAnsiEncoding"), "ToUnicode" => Object::reference(1),
//! };
//! let cmap = b"1 begincodespacerange <00> <FF> endcodespacerange\n\
//!     1 beginbfchar <01> <FB01> endbfchar";
//! let stream = Stream::new(dict! {}, cmap.to_vec());
//! let objects = BTreeMap::from([(ObjectId::new(1, 0), Object::from(stream))]);
//!
//! // Its objects, and its streams' data as they hold it.
//! let streams = WithStreams::new(&objects, |id, _| {
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
//! let capped = WithStreams::new(&objects, |id, _| {
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
    Codes, Descendant, FallbackProgram, Font, FontApprox, FontKind, FontMetrics, FontProgram,
    FontStyle, GlyphBox, OutlineVerb, Type3Glyph, VerticalMetrics,
};
pub use fonts::{FontId, FontScope, Fonts};
pub use limits::Limits;
pub use shown::{Shown, ShownString, Unread, shown};
pub use std14::{Std14, Unencodable};

mod afm;
mod cmap;
#[cfg(feature = "embed")]
pub mod embed;
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
