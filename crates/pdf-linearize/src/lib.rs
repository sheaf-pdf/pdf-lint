//! PDF files laid out for reading over a network (ISO 32000-2, Annex F), with
//! no object model of its own and no I/O: a caller describes its objects as
//! plain data, whichever model holds them.
//!
//! - **The plan** ([`plan()`]): a [`Graph`], each object's id and what it
//!   refers to, the pages in order, and what a viewer wants at open, becomes
//!   Annex F's order, the new numbers and the object streams' members, a
//!   [`Plan`], with what each page's entry in the hint tables needs. Which
//!   pages use each object is a [`Use`], and [`group`] says which objects share
//!   an object stream by their uses; a writer that doesn't linearize has them
//!   with no plan by [`page_uses`].
//! - **The bytes around the bodies** ([`assemble`]): a caller writes each of
//!   the plan's bodies, however it holds them, and gives their lengths. A
//!   [`Layout`] has the padded dictionary, both cross-reference sections and
//!   the hint stream as bytes, and says what's next in the file. What a stream
//!   of its own is filtered or encrypted by is a closure, so no filter or key
//!   is here.
//! - **The hint tables** ([`hint`]): all of F.4's as typed values, written to
//!   bits and read back, [`hint::tables`] reading any bytes under a cap.
//! - **The check** ([`check()`]): the [`Facts`] of a file as written, as any
//!   model's reader gives them, held against each other, to a [`Verdict`] and a
//!   [`Finding`] for each thing wrong, by the clause it departs from and a
//!   [`Grade`].
#![cfg_attr(
    feature = "family",
    doc = "- **The family's documents** ([`family`], a default feature): a document of"
)]
#![cfg_attr(
    feature = "family",
    doc = "  pdf-structure's, or a file's bytes, written as a linearized file: the page tree"
)]
#![cfg_attr(
    feature = "family",
    doc = "  by pdf-semantics, each body through pdf-structure's writer, copied where it can be, encrypted as the file was or under a new key."
)]
//!
//! Its caps are one value, [`Limits`]. With no features there's no document model
//! here at all: an object is its [`ObjectId`](pdf_objects::ObjectId),
//! pdf-objects' pair of a number and a generation, and the plain data a caller
//! gives of it.
//!
//! With the `tracing` feature, off by default, a plan, an assembly and a check
//! are each a span at `debug`, and with `family` a document's linearizing and a
//! file's facts, their stages and reads spans within them. A span's fields are
//! numbers, never a byte of a file; an object, a page, a table's entry and a
//! finding have none. Without it, none is compiled.

pub use check::{Facts, Finding, Grade, Sink, Verdict, check};
pub use layout::{Encoded, Layout, Piece, Stream, StreamKind, assemble};
pub use limits::Limits;
pub use pdf_objects;
pub use plan::{
    Container, FileGroup, FirstPage, Graph, Group, Kind, MEMBERS, Node, Page, Part, Plan, Use,
    What, Write, group, page_uses, plan,
};
#[cfg(feature = "family")]
pub use {pdf_semantics, pdf_structure};

pub mod check;
#[cfg(feature = "family")]
pub mod family;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
pub mod hint;
pub mod layout;
mod limits;
mod phases;
pub mod plan;

#[cfg(test)]
mod tests {
    /// The README is shown on crates.io and docs.rs, where a path beside the
    /// crate reaches nothing: its licence links are the repository's URLs, the
    /// manifest's `repository` and the file, and every link it has is a URL.
    #[test]
    fn the_readmes_licence_links_reach_the_files() {
        let readme = include_str!("../README.md");
        let repository = env!("CARGO_PKG_REPOSITORY");
        for file in ["LICENSE-APACHE", "LICENSE-MIT"] {
            let link = format!("[{file}]({repository}/blob/main/{file})");
            assert!(
                readme.contains(&link),
                "{file}'s link isn't the repository's URL"
            );
        }
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

/// The README's examples, run as doctests: one of them uses `family`.
#[cfg(all(doctest, feature = "family"))]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
