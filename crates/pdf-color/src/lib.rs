//! PDF's color spaces (ISO 32000-2, 8.6), read over any resolver of objects.
//!
//! - **Components as PDFium counts them** ([`components`]), as of
//!   chromium/8125: what a reader of content needs of a space, as it sizes an
//!   unfiltered inline image, so says where that image's data ends. It's the
//!   count of any value at all, as PDFium makes it, and not of a space that's
//!   well-formed by ISO's rules; no stream is decoded and no function evaluated.
//! - **The spaces, typed, as ISO 32000-2 says them** ([`Space`]): a value's
//!   [`Family`], its components, its base or alternate space and its colorants,
//!   each departure a [`Fault`] told to pdf-objects'
//!   [`Sink`](pdf_objects::fault::Sink), and looked for only for one that wants
//!   them. A space read with a sink that wants faults, none told,
//!   counts by [`Space::components`] as [`components`] counts it, where
//!   [`counted`] meets no cap, but for a Pattern space over more than 16
//!   components, which no clause refuses and PDFium does.
//! - **What a resources dictionary names** ([`Spaces`], [`Named`]), reached as
//!   PDFium reaches it, each key as it was read and where it was written, and
//!   each name counted as [`counted`] counts its value
//!   ([`Spaces::components`]), as a reader of content counts them. It's
//!   PDFium's reading and tells nothing: an ISO check of a resources dictionary
//!   reads the dictionary itself.
//! - **Caps from one value** ([`Limits`]): how deep a space nests and how many
//!   objects one is worth looking up, so a hostile file's cost per space has a
//!   limit, and nothing panics. No cap covers a whole dictionary of them
//!   ([`Limits::space_visits`] says what that can come to).
//!
//! Nothing here converts a color, opens an ICC profile, evaluates a function or
//! writes a space: a space is pdf-objects' values, written as they are.
//!
//! A resolver is given by value, so `&reading`, `&document` or a closure, and a
//! `&dyn Resolve` too. What's read and what reads it are pdf-objects': its
//! [`Object`](pdf_objects::Object)s, a [`Resolve`](pdf_objects::Resolve), an
//! [`Unreadable`](pdf_objects::Unreadable) for a failed read and a
//! [`Span`](pdf_objects::Span) for where something was written. pdf-objects is
//! re-exported whole, and none of its items by name, so a caller names them,
//! [`dict!`](pdf_objects::dict) and a sink of its own
//! ([`Sink`](pdf_objects::fault::Sink)) through `pdf_color::pdf_objects`, at
//! the version this crate takes, with no dependency of its own.

/// pdf-objects, whose values this crate reads, through whose resolvers, and
/// to whose sinks it tells faults: named through it, they're the version
/// this crate is built with.
pub use pdf_objects;

pub use count::{Counted, components, counted};
pub use fault::Fault;
pub use limits::Limits;
pub use resources::{Components, Iter, Named, Spaces};
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

#[cfg(test)]
mod tests {
    /// The README is shown on crates.io and docs.rs, where a path beside
    /// the crate reaches nothing: its licence links are the repository's
    /// URLs, the manifest's `repository` and the file, and no link of it
    /// climbs out of the crate.
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
        assert!(!readme.contains("](../"), "a link climbs out of the crate");
    }
}

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
