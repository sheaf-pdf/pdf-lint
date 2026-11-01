//! PDF files laid out for reading over a network (ISO 32000-2, Annex F), with
//! no object model of its own and no I/O: a caller describes its objects as
//! plain data, whichever model holds them.
//!
//! - **The plan** ([`plan()`]): a [`Graph`], each object's id and what it
//!   refers to, the pages in order, and what a viewer wants at open, becomes
//!   Annex F's order, the new numbers and the object streams' members, a
//!   [`Plan`], with what each page's entry in the hint tables needs. Which
//!   pages use each object is a [`Use`], and [`group`] says which objects share
//!   an object stream by their uses, which a writer that doesn't linearize can
//!   use too.
//! - **The bytes around the bodies** ([`assemble`]): a caller writes each of the
//!   plan's bodies, however it holds them, and gives their lengths. A [`Layout`]
//!   has the padded dictionary, both cross-reference sections and the hint stream
//!   as bytes, and says what's next in the file. What a stream of its own is
//!   filtered or encrypted by is a closure, so no filter or key is here.
//! - **The hint tables** ([`hint`]): all of F.4's as typed values, written to
//!   bits and read back, [`hint::tables`] reading any bytes under a cap.
//! - **The check** ([`check()`]): the [`Facts`] of a file as written, as any model's
//!   reader gives them, held against each other, to a [`Verdict`] and a [`Finding`]
//!   for each thing wrong, by the clause it departs from and a [`Grade`].
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
//! Its caps are one value, [`Limits`]. With no features there's no object model
//! here at all.

pub use check::{Facts, Finding, Grade, Sink, Verdict, check};
pub use layout::{Encoded, Layout, Piece, Stream, StreamKind, assemble};
pub use limits::Limits;
pub use plan::{
    Container, FileGroup, Graph, Group, Kind, MEMBERS, Node, Page, Part, Plan, Use, What, Write,
    group, plan,
};

pub mod check;
#[cfg(feature = "family")]
pub mod family;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
pub mod hint;
pub mod layout;
mod limits;
pub mod plan;

/// An object's number and generation, as a caller's model holds them: the same
/// tuple pdf-objects' `ObjectId` and lopdf's are.
pub type Id = (u32, u16);

/// The README's examples, run as doctests: one of them uses `family`.
#[cfg(all(doctest, feature = "family"))]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
