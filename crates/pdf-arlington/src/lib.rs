//! The Arlington PDF Model, the PDF Association's machine-readable grammar
//! of ISO 32000-2, embedded and read, with the predicates in its cells
//! evaluated over pdf-objects' objects.
//!
//! The model is embedded (`model/`, whose `SOURCE.md` says from where): a
//! TSV table per object, a row per key or array element, saying its types,
//! the PDF version it came in and any it was deprecated in, whether it's
//! required, the values it may take and the tables its value links to. The
//! tables' layout is upstream's README.md, "TSV Data Overview"; the rules
//! for each column are INTERNAL_GRAMMAR.md's "TSV file rules" and "TSV Data
//! Fields", from "Column 1 - Key" to "Column 11 - Link". [`grammar()`]
//! reads them.
//!
//! - **The tables** ([`grammar`](mod@grammar)): each row's cells read,
//!   its types, items and links split out and the version predicates
//!   around them kept beside them; the embedded model, with the cells it
//!   corrects and the rows it adds, or a checkout of upstream's
//!   `tsv/latest` read whole.
//! - **The predicates** ([`predicate`]): the language in the cells parsed,
//!   and evaluated on an object where it sits ([`Cx`]), what can't be told
//!   said so, never guessed.
//! - **Extensions** ([`extension`]): those a file declares, from its
//!   catalog and its XMP metadata, and those the model names.

pub mod extension;
pub mod grammar;
pub mod predicate;
mod xxh64;

/// pdf-objects, whose objects and versions this crate's functions take:
/// named through it, they're the version this crate is built with.
pub use pdf_objects;

pub use grammar::{ArlType, At, Grammar, Row, Table, Wrap, WrapFn, Wrapped, grammar, use_model};
pub use predicate::{Cx, Gate, Reference, Since, Truth};

/// The upstream commit the embedded model is (`model/SOURCE.md`).
pub const COMMIT: &str = "d148fee5a0cb6dcacadf58c09ed842e32847024d";

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
