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
//!
//! Its caps are one value, [`Limits`].

pub use layout::{Encoded, Layout, Piece, Stream, StreamKind, assemble};
pub use limits::Limits;
pub use plan::{
    Container, FileGroup, Graph, Group, Kind, MEMBERS, Node, Page, Part, Plan, Use, What, Write,
    group, plan,
};

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

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
