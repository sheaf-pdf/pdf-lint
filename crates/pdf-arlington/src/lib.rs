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

#[cfg(test)]
mod tests {
    /// The README is shown on crates.io and docs.rs, where a path beside the
    /// crate reaches nothing: its licence link is the repository's URL, the
    /// manifest's `repository` and the file, and every link it has is a URL,
    /// so none is a path such as `../`. The crate is Apache-2.0 alone, as the
    /// model it embeds is, so the link is to `LICENSE-APACHE` alone; the
    /// model's own `LICENSE` and `NOTICE.txt`, which the package holds, are
    /// named, not linked.
    #[test]
    fn the_readmes_licence_link_reaches_the_file() {
        let readme = include_str!("../README.md");
        let repository = env!("CARGO_PKG_REPOSITORY");
        let link = format!("[LICENSE-APACHE]({repository}/blob/main/LICENSE-APACHE)");
        assert!(
            readme.contains(&link),
            "LICENSE-APACHE's link isn't the repository's URL"
        );
        for (at, _) in readme.match_indices("](") {
            let target = &readme[at + 2..];
            let target = &target[..target.find(')').unwrap_or(target.len())];
            assert!(
                target.starts_with("https://"),
                "a link isn't a URL: {target}"
            );
        }
    }
}
