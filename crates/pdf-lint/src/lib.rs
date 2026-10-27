//! Checks a PDF file's objects against the Arlington PDF Model, the PDF
//! Association's machine-readable grammar of ISO 32000-2.
//!
//! The model is embedded (`model/`, whose `SOURCE.md` says from where): a
//! TSV table per object, a row per key or array element, saying its types,
//! the PDF version it came in and any it was deprecated in, whether it's
//! required, the values it may take and the tables its value links to. The
//! tables' layout is upstream's README.md, "TSV Data Overview"; the rules
//! for each column are INTERNAL_GRAMMAR.md's "TSV file rules" and "TSV Data
//! Fields", from "Column 1 - Key" to "Column 11 - Link". [`grammar()`]
//! reads them.

pub mod allowance;
pub mod bytes;
pub mod check;
pub mod extension;
mod filter;
#[cfg(any(test, fuzzing))]
pub mod fuzz;
pub mod grammar;
mod json;
pub mod object;
pub mod predicate;
pub mod qpdf;
pub mod source;
pub mod text;
pub mod version;
#[cfg(any(test, fuzzing))]
#[cfg_attr(fuzzing, allow(dead_code))]
pub(crate) mod write;
mod xxh64;

pub use allowance::{Allowance, Allowances};
pub use check::{
    Class, Exempt, Finding, Hold, Made, Options, Report, Rule, Unchecked, Unlisted, UnlistedIn,
    check, file_version,
};
pub use grammar::{ArlType, At, Grammar, Row, Table, Wrap, WrapFn, Wrapped, grammar, use_model};
pub use object::{Dict, Entry, Object, ObjectId, Span, Str, Stream, Value};
pub use predicate::{Cx, Gate, Reference, Since, Truth};
pub use qpdf::Qpdf;
pub use source::{Document, Encryption, Source, Trailer, Update, Warning};
pub use version::Version;

/// The upstream commit the embedded model is (`model/SOURCE.md`).
pub const COMMIT: &str = "d148fee5a0cb6dcacadf58c09ed842e32847024d";
