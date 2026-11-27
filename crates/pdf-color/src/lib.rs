//! PDF color space parser, component counter, §7.10 function evaluator, and §8.6 color converter.
//!
//! `pdf-color` reads color spaces through any [`Resolve`](pdf_objects::Resolve) implementation
//! (`&reading`, `&document`, `&BTreeMap`, or [`resolve_fn`](pdf_objects::resolve_fn)) without
//! performing file I/O, and prepares functions ([`Function`]) and color spaces ([`PreparedSpace`])
//! over [`Resolve`](pdf_objects::Resolve) + [`Streams`](pdf_objects::Streams) under bounded
//! [`Limits`] and explicit ISO 32000-2 vs. PDFium [`Rules`](pdf_objects::Rules).
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
//!   definitions into typed families, component counts ([`Space::components`]), default `/Decode`
//!   intervals ([`Space::default_decode`]), base/alternate spaces, lookup tables
//!   ([`Indexed::lookup_bytes`]), and spot/process colorants, reporting spec departures as
//!   [`Fault`]s to a [`Sink`](pdf_objects::fault::Sink) (skipped when `()` is passed). On
//!   fault-free spaces within [`Limits`], [`Space::components`] matches [`components`] (except for
//!   `Pattern` spaces with >16 components).
//! - **Functions & color evaluation ([`Function`], [`PreparedSpace`], [`EvalOptions`], [`Paint`]):**
//!   Evaluates ISO 32000-2 §7.10 `Type 0`, `Type 2`, `Type 3`, and `Type 4` (PostScript calculator)
//!   functions and converts colors across device, CIE (`CalGray`, `CalRGB`, `Lab`), `Indexed`,
//!   `Separation`, `DeviceN`, and (with the optional `icc` Cargo feature) `ICCBased` /
//!   `/OutputIntents` profiles to sRGB under both `Rules::Iso` and `Rules::Pdfium`.
//! - **Per-space resource caps ([`Limits`]):** Bounds nesting depth, object visits, stream decode
//!   bytes, unpacked sample counts, and PostScript calculator steps/stack so cyclic or adversarial
//!   inputs cannot panic or exhaust memory/CPU.
//!   Reader caps, locked files, and source I/O errors propagate as [`Unreadable`](pdf_objects::Unreadable).
//!
//! `pdf-objects` is re-exported at [`pdf_objects`] (`Object`, `Resolve`, `Streams`,
//! [`resolve_fn`](pdf_objects::resolve_fn), `Span`, [`dict!`](pdf_objects::dict), and
//! [`Sink`](pdf_objects::fault::Sink)).

/// Re-export of `pdf-objects` for values, resolvers, streams, spans, and fault sinks.
pub use pdf_objects;

pub use count::{Counted, components, counted};
pub use eval::{EvalOptions, Paint, PreparedSpace};
pub use fault::Fault;
pub use function::{Function, FunctionKind};
pub use intent::RenderingIntent;
pub use limits::Limits;
pub use resources::{Components, Iter, Named, Spaces};
pub use space::{
    Attributes, Cie, Colorant, ColorantSpace, Colorants, DeviceN, Family, Icc, Indexed, Lookup,
    LookupError, Process, Separation, Space, Subtype,
};

mod cmyk_lut;
mod count;
mod eval;
mod fault;
mod function;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod intent;
mod limits;
mod resources;
mod space;
mod walk;

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
