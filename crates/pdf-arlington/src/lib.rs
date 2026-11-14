//! Embedded Arlington PDF Model grammar and predicate evaluator over [`pdf_objects`].
//!
//! The Arlington PDF Model is the PDF Association's machine-readable TSV
//! specification of ISO 32000-2: one [`Table`] per PDF object kind and one
//! [`Row`] per dictionary key or array element specifying allowed types, PDF
//! version availability, whether the entry is required, allowed values, and
//! linked child tables.
//!
//! # Modules & Entry Points
//!
//! - **[`grammar`](mod@grammar):** Load the compiled-in model with
//!   [`Grammar::embedded`] (upstream `tsv/latest` at [`COMMIT`] plus audited
//!   spec [`grammar::corrections`] and [`grammar::additions`]), or load an
//!   external `tsv/latest` checkout with [`Grammar::load`].
//! - **[`predicate`]:** Parse and evaluate Arlington `fn:` predicate
//!   expressions against objects in context ([`FileCx`] for file-wide state
//!   over any [`pdf_objects::Resolve`], and [`Cx`] for a specific object/row).
//!   Indeterminate checks return [`Truth::Unknown`] rather than guessing.
//! - **[`extension`]:** Inspect extensions declared in a file's catalog
//!   `/Extensions` dictionary and XMP metadata ([`extension::declared`]), or
//!   list all extensions referenced by a grammar ([`Grammar::extensions`]).

pub mod extension;
pub mod grammar;
pub mod predicate;

/// pdf-objects, whose objects and versions this crate's functions take:
/// named through it, they're the version this crate is built with.
pub use pdf_objects;

pub use grammar::{ArlType, At, Grammar, Row, Table, Wrap, WrapFn, Wrapped};
pub use predicate::{Cx, FileCx, Gate, Latin, Reference, Since, Truth};

/// The upstream commit the embedded model is (`model/SOURCE.md`).
pub const COMMIT: &str = "d148fee5a0cb6dcacadf58c09ed842e32847024d";

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
