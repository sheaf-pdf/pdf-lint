//! PDF's color spaces (ISO 32000-2, 8.6), read over any resolver of objects.
//!
//! - **Components as PDFium counts them** ([`components`]), as of
//!   chromium/8125: what a reader of content needs of a space, as it sizes an
//!   unfiltered inline image, so says where that image's data ends. It's the
//!   count of any value at all, as PDFium makes it, and not of a space that's
//!   well-formed by ISO's rules; no stream is decoded and no function evaluated.
//! - **The spaces, typed, as ISO 32000-2 says them** ([`Space`]): a value's
//!   [`Family`], its components, its base or alternate space and its colorants,
//!   each departure a [`Fault`] told to a [`Sink`], and looked for only for one
//!   that wants them. A space read with no fault told counts by
//!   [`Space::components`] as [`components`] counts it, but for a Pattern space
//!   over more than 16 components, which no clause refuses and PDFium does.
//! - **What a resources dictionary names** ([`Spaces`], [`Named`]), reached as
//!   PDFium reaches it, each key as it was read and where it was written.
//! - **Caps from one value** ([`Limits`]): how deep a space nests and how many
//!   objects one is worth looking up, so a hostile file's costs are bounded and
//!   nothing panics.
//!
//! Nothing here converts a color, opens an ICC profile, evaluates a function or
//! writes a space: a space is pdf-objects' values, written as they are.
//!
//! A resolver is given by value, so `&reading`, `&document` or a closure, and a
//! `&dyn Resolve` too. [`Dict`], [`Object`], [`ObjectId`], [`Resolve`], [`Span`],
//! [`Str`], [`Stream`], [`Unreadable`], [`Value`] and [`Why`] are pdf-objects',
//! re-exported.

pub use pdf_objects::{Dict, Object, ObjectId, Resolve, Span, Str, Stream, Unreadable, Value, Why};

pub use count::components;
pub use fault::{Fault, Place, Sink};
pub use limits::Limits;
pub use resources::{Named, Spaces};
pub use space::{
    Attributes, Cie, Colorant, ColorantSpace, Colorants, DeviceN, Family, Icc, Indexed, Lookup,
    Process, Separation, Space, Subtype,
};

mod count;
mod fault;
mod function;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod limits;
mod resources;
mod space;

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
