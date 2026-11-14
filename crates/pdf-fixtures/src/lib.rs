//! Internal test-fixture builders and `qpdf` oracle helpers (unpublished).
//!
//! - **Synthetic PDF builders ([`fixture`]):** Assemble valid or intentionally malformed PDF files
//!   from `pdf-objects` values or raw byte snippets, with cross-reference tables or streams,
//!   stream-filter encodings, and deterministic test encryption.
//! - **`qpdf` oracle runner ([`qpdf`]):** Invokes `PDF_LINT_QPDF` (or `qpdf` on `PATH`) to parse
//!   files as JSON and decode streams, object streams, or encrypted strings for differential
//!   testing against workspace crates.
//!
//! Public return types use only `pdf-objects` values, bytes, and strings so crates that
//! `pdf-fixtures` depends on (such as `pdf-structure`) can still use `pdf-fixtures` as a path
//! `dev-dependency` without type-version conflicts.

pub mod fixture;
mod json;
pub mod qpdf;
