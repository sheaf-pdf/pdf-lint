//! Internal test-fixture builders and `qpdf` oracle helpers (unpublished).
//!
//! - **Synthetic PDF builders ([`fixture`]):** Assemble valid or intentionally malformed PDF files
//!   from `pdf-objects` values or raw byte snippets, with cross-reference tables or streams,
//!   stream-filter encodings, and deterministic test encryption.
//! - **TrueType program builders ([`font`]):** Assemble TrueType programs of given glyphs
//!   (boxes, contours, composites), `cmap` subtables of formats 4 and 12, `OS/2`, `post`,
//!   `name` and hinting tables, or tables changed by hand, in an SFNT or a collection.
//! - **`qpdf` oracle runner ([`qpdf`]):** Invokes `PDF_LINT_QPDF` (or `qpdf` on `PATH`) to parse
//!   files as JSON and decode streams, object streams, or encrypted strings for differential
//!   testing against workspace crates.
//!
//! Public return types use only `pdf-objects` values, bytes, and strings so crates that
//! `pdf-fixtures` depends on (such as `pdf-structure`) can still use `pdf-fixtures` as a path
//! `dev-dependency` without type-version conflicts.

pub mod fixture;
pub mod font;
mod json;
pub mod qpdf;
