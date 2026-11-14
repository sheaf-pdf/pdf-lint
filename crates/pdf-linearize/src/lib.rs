//! Plan, assemble, and validate linearized ("Fast Web View") PDF files (ISO 32000-2, Annex F).
//!
//! `pdf-linearize` provides a pure, model-independent layout and validation engine alongside a
//! turnkey `family` adapter (enabled by default) for `pdf-structure` and `pdf-semantics`.
//!
//! # Core Concepts
//!
//! - **Plan file order ([`plan()`]):** Describe objects, references, pages, and open-time targets
//!   in a [`Graph`] to compute a [`Plan`] specifying Annex F.3 file order, renumbered object IDs,
//!   and object-stream groupings ([`plan::page_uses`] and [`plan::group()`] also group objects by
//!   page without full linearization).
//! - **Assemble around serialized bodies ([`assemble`]):** Pass serialized body lengths to build
//!   a [`Layout`] whose [`pieces`](Layout::pieces) interleave padded linearization dictionaries,
//!   cross-reference sections, hint streams, and caller-provided bodies. Stream compression and
//!   encryption run through a caller-supplied closure.
//! - **Read and write hint tables ([`hint`]):** Encode and decode Annex F.4 bit-packed hint
//!   tables ([`hint::tables`]).
//! - **Validate linearized files ([`check()`]):** Check [`check::Facts`] extracted by any reader
//!   into a [`check::Verdict`], reporting [`check::Finding`] diagnostics graded by
//!   [`check::Grade`]. Reader caps, missing passwords, or unsupported filters ([`check::Unread`])
//!   are reported as unjudged rather than file errors.
#![cfg_attr(
    feature = "family",
    doc = "- **Turnkey `pdf-structure` integration ([`family`], enabled by default):**"
)]
#![cfg_attr(
    feature = "family",
    doc = "  Linearize a `Document` or PDF byte slice (plain or encrypted) and extract or"
)]
#![cfg_attr(
    feature = "family",
    doc = "  validate `Facts` directly from a `Reading`."
)]
//!
//! Resource usage is bounded by [`Limits`] across planning, hint decoding, validation, and
//! `family` tree walks without panicking on untrusted input. `pdf-objects` is re-exported at
//! [`pdf_objects`] (and with `family`, `pdf-structure` and `pdf-semantics` are re-exported too).
//!
//! # Cargo Features
//!
//! - **`family`** *(default)*: Enables integration with `pdf-structure` and `pdf-semantics`.
//! - **`zlib-rs`** *(optional)*: Enables the `zlib-rs` Flate backend via `pdf-structure`.
//! - **`tracing`** *(optional)*: Emits numeric `tracing` spans over planning, assembly,
//!   validation, and `family` reads/writes.
//! - **`insecure-test-writer`** *(optional)*: Deterministic encryption keys and IVs for fuzzing.
//!   Never enable in production builds.
//!
//! # Example
//!
//! Plan two pages sharing a font and assemble the linearized bytes around their bodies:
//!
//! ```
//! use std::convert::Infallible;
//!
//! use pdf_linearize::layout::{Encoded, Options, Piece, Stream};
//! use pdf_linearize::pdf_objects::ObjectId;
//! use pdf_linearize::pdf_objects::version::Version;
//! use pdf_linearize::plan::{Kind, Page, Part, What};
//! use pdf_linearize::{Graph, Limits, assemble, hint, plan};
//!
//! // A catalog (1), a page tree node (2), two pages (3, 4), a content stream
//! // each (5, 6), and a font both pages use (7).
//! let id = |number| ObjectId::new(number, 0);
//! let mut graph = Graph::new(id(1), Kind::Table);
//! graph.object(id(1), [id(2)]);
//! graph.object(id(2), [id(3), id(4)]);
//! graph.object(id(3), [id(2), id(5), id(7)]);
//! graph.object(id(4), [id(2), id(6), id(7)]);
//! for leaf in [id(5), id(6), id(7)] {
//!     graph.object(leaf, []);
//! }
//! graph.page(Page::new(id(3)));
//! graph.page(Page::new(id(4)));
//! graph.tree_node(id(2));
//! let plan = plan(&graph, &Limits::DEFAULT).unwrap();
//!
//! // The font goes with the first page, though the second uses it too. The
//! // second page is numbered from 1, and lists the font's shared entry.
//! let first = &plan.writes()[plan.part_writes(Part::FirstPage)];
//! let first: Vec<What> = first.iter().map(|write| write.what).collect();
//! assert_eq!(first, [id(3), id(5), id(7)].map(What::Object));
//! assert_eq!(plan.number(id(4)), Some(id(1)));
//! assert_eq!(plan.shared(1), [1]);
//!
//! // A body of 40 bytes for each write, however the caller writes it, and
//! // the bytes around them, the layout's own streams left as they are.
//! let lengths = vec![40; plan.writes().len()];
//! let plain = |stream: Stream| {
//!     Ok::<_, Infallible>(Encoded::plain(stream.data.to_vec()))
//! };
//! let options = Options::new(Version::new(1, 7));
//! let layout = assemble(&plan, &lengths, &options, plain).unwrap();
//!
//! // Its pieces are the whole file, in order.
//! let written: u64 = (layout.pieces())
//!     .map(|piece| match piece {
//!         Piece::Bytes(bytes) => bytes.len() as u64,
//!         Piece::Body(at) => lengths[at],
//!     })
//!     .sum();
//! assert_eq!(written, layout.len());
//!
//! // Its hint tables read back: the second page lists the font's entry.
//! let (data, keys) = (layout.hint_data(), layout.hint_keys());
//! let tables = hint::tables(data, keys, 2, &Limits::DEFAULT).unwrap();
//! assert_eq!(tables.pages.pages[1].shared, [(1, 0)]);
//! ```

pub use check::check;
pub use layout::{Layout, assemble};
pub use limits::Limits;
pub use pdf_objects;
pub use plan::{Graph, Plan, plan};
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

    /// The crate doc's example is the README's, line for line, so that what
    /// docs.rs shows and what crates.io shows are one example, and each runs
    /// as a doctest, in every build. The crate doc's lines with `family` are
    /// attributes, which this reads past.
    #[test]
    fn the_crate_docs_example_is_the_readmes() {
        let readme: Vec<&str> = (include_str!("../README.md").lines())
            .skip_while(|line| *line != "```rust")
            .skip(1)
            .take_while(|line| *line != "```")
            .collect();
        let doc: Vec<&str> = (include_str!("lib.rs").lines())
            .filter_map(|line| line.strip_prefix("//!"))
            .map(|line| line.strip_prefix(' ').unwrap_or(line))
            .skip_while(|line| *line != "```")
            .skip(1)
            .take_while(|line| *line != "```")
            .collect();
        assert!(readme.len() > 10, "the README's example: {readme:?}");
        assert_eq!(doc, readme);
    }

    /// What the crate gives a caller is plain data, sent to and shared by any
    /// thread: a plan and its graph, a layout and its pieces, a file's facts,
    /// what's judged of them, and what a document's linearizing gives.
    #[test]
    fn what_it_gives_is_sent_and_shared() {
        fn shared<T: Send + Sync>() {}
        shared::<crate::Graph>();
        shared::<crate::Plan>();
        shared::<crate::plan::Error>();
        shared::<crate::Layout>();
        shared::<crate::layout::Pieces<'_>>();
        shared::<crate::layout::Error<std::convert::Infallible>>();
        shared::<crate::hint::Tables>();
        shared::<crate::check::Facts>();
        shared::<crate::check::Verdict>();
        shared::<crate::check::Finding>();
        #[cfg(feature = "family")]
        {
            shared::<crate::family::Linearized>();
            shared::<crate::family::Error>();
        }
    }
}

/// The README's example, run as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
