//! PDF color space parser and component counter (ISO 32000-2, §8.6).
//!
//! `pdf-color` reads color spaces through any [`Resolve`](pdf_objects::Resolve) implementation
//! (`&reading`, `&document`, or a closure) without decoding ICC profile streams, evaluating
//! functions, or converting pixel values.
//!
//! # Core Concepts
//!
//! - **PDFium-compatible component counts ([`components`], [`counted`]):** Counts components for
//!   any `/ColorSpace` entry or inline-image color space array using PDFium's rules as of
//!   chromium/8152, sizing unfiltered inline images in content streams even on malformed inputs.
//! - **Resource dictionary lookup ([`Spaces`], [`Named`]):** Resolves a page or form's
//!   `/ColorSpace` resource dictionary as PDFium reaches it and pairs each entry with its
//!   component count ([`Spaces::components`]).
//! - **Typed ISO 32000-2 spaces ([`Space`], [`Family`], [`Colorant`]):** Parses color space
//!   definitions into typed families, component counts ([`Space::components`]), base/alternate
//!   spaces, lookup tables, and spot/process colorants, reporting spec departures as [`Fault`]s
//!   to a [`Sink`](pdf_objects::fault::Sink) (skipped when `()` is passed). On fault-free spaces
//!   within [`Limits`], [`Space::components`] matches [`components`] (except for `Pattern` spaces
//!   with >16 components).
//! - **Per-space resource caps ([`Limits`]):** Bounds nesting depth and object visits per space
//!   (`Limits::space_visits`) so cyclic or shared function graphs cannot cause exponential work.
//!   Reader caps, locked files, and source I/O errors propagate as [`Unreadable`](pdf_objects::Unreadable).
//!
//! `pdf-objects` is re-exported at [`pdf_objects`] (`Object`, `Resolve`, `Span`, [`dict!`](pdf_objects::dict),
//! and [`Sink`](pdf_objects::fault::Sink)).

/// Re-export of `pdf-objects` for values, resolvers, spans, and fault sinks.
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
