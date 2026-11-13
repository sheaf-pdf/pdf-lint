//! Read and write PDF files through one dependency. The crates of one family
//! are re-exported whole, each at the version this one is built with, so a
//! caller names any of their types through it ([`pdf_structure`], say), and
//! they work together. On top of them are the recipes that span them, with a
//! viewer's defaults.
//!
//! - **Open a file** ([`open`] by its path, [`from_bytes`] from memory or any
//!   source) under [`Options`], a viewer's by default: a [`Pdf`], each object
//!   read when it's first asked for, from any thread.
//! - **List its pages** ([`Pdf::pages`]), each with what it inherits and from
//!   where, and **read a page's content** ([`Page::content`]), its streams
//!   decoded and joined, and its operations.
//! - **Read a page's text and fonts** ([`Page::text`], [`Page::fonts`]): the
//!   strings its content shows, in order, each in its font, the forms it
//!   draws followed, as PDFium reads them for its text, with no layout.
//! - **Read what the file says of itself**: its version, its encryption, its
//!   document information dictionary and XMP claims, whether its
//!   linearization holds ([`Pdf::linearized`]), and any stream's data.
//! - **Write a document** ([`Document::new`], or a file edited,
//!   [`Pdf::edit`]): pages added, each drawn by a content writer, the
//!   standard 14 fonts by name ([`standard_font`]) and claims as XMP; saved
//!   whole, linearized or as an update, in the clear or encrypted.
//! - **Call a recipe as a function** ([`page`](mod@page), [`file`](mod@file),
//!   [`build`](mod@build)) over a reading or a document of your own. The
//!   [`prelude`] names what they take and give.
//!
//! One [`Limits`] holds every crate's caps, and one [`Error`] wraps every
//! crate's error. Past a cap, a call says so, and no file makes it panic. A
//! call's `_with` form tells a crate's faults to the sink it's given. Its
//! plain form tells nothing, and looks for nothing.
//!
//! # Features
//!
//! - **`programs`** and **`clock`**, on by default: pdf-fonts reads a font's
//!   program, and pdf-semantics has `Date::now`.
//! - **`image-ends`**, **`predefined-cmaps`** and **`simd`**, off by default:
//!   pdf-content-streams ends an inline image's data by decoding it, pdf-fonts
//!   reads Adobe's predefined CMaps, and pdf-content-streams' dependencies use
//!   the CPU's SIMD.
//! - **`tracing`**, off by default: a span at `debug` over each recipe, and
//!   each crate's own. A span's fields are numbers, never a byte of a file.
//! - **`insecure-test-writer`**, off by default, for tests and fuzzing:
//!   pdf-structure's test writer, a new encryption's key from a seeded
//!   generator, so a test writes the same bytes each run. It's never for a
//!   document's protection, and a build that ships leaves it off.
//!
//! A page's text, from a file written here first:
//!
//! ```
//! use pdfs::prelude::*;
//!
//! // A file of one page, written here so the example runs on its own.
//! let mut doc = Document::new();
//! let page = doc.add_page(Rect::LETTER);
//! page.begin_text();
//! page.font(standard_font(Std14::Helvetica), 24.0);
//! page.text_move(72.0, 700.0);
//! page.show(&Std14::Helvetica.codes("Hello, world").unwrap());
//! page.end_text();
//! let file = doc.to_bytes().unwrap();
//!
//! // Read it as any file is read.
//! let pdf = pdfs::from_bytes(file).unwrap();
//! let pages = pdf.pages().unwrap();
//! assert_eq!(pages.len(), 1);
//! for page in &pages {
//!     assert_eq!(page.attributes().media_box().value, Rect::LETTER);
//!     let shown = page.text().unwrap();
//!     assert_eq!(shown.text(), "Hello, world");
//!     assert!(shown.is_whole());
//! }
//! ```

pub use build::standard_font;
pub use document::Document;
pub use error::Error;
pub use limits::Limits;
#[cfg(any(unix, windows))]
pub use open::open;
pub use open::{Options, from_bytes};
pub use page::{Content, NamedFont};
pub use pdf::{Page, Pages, PagesIter, Pdf};

pub mod build;
mod document;
mod error;
pub mod file;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod limits;
mod open;
pub mod page;
mod pdf;
mod phases;
pub mod prelude;

/// The tokenizer: a file's bytes as tokens, by ISO 32000-2's rules or
/// PDFium's, with no `std` or `alloc` of its own.
pub use pdf_tokenizer;

/// PDF's objects: values with where each was written, their parser and
/// their writer, and text strings read and written.
pub use pdf_objects;

/// PDF's standard filters: data decoded, every fault said, and Flate
/// encoded, on miniz_oxide or zlib-rs.
pub use pdf_filters;

/// PDF's standard security handler: a file decrypted, and a writer's
/// output encrypted under a password.
pub use pdf_crypt;

/// PDF's file structure: a file read whole or each object on demand, and a
/// document written whole or as an incremental update.
pub use pdf_structure;

/// PDF's color spaces: their components as PDFium counts them, each typed,
/// and what a resources dictionary names.
pub use pdf_color;

/// Content streams: their tokens and operations as PDFium reads them, a
/// page's streams joined as one, and content written.
pub use pdf_content_streams;

/// PDF's common structures, read and written: dates, name and number trees,
/// the page tree and what XMP metadata claims.
pub use pdf_semantics;

/// PDF's fonts as PDFium reads them: codes, widths and text, and the
/// standard 14 with Adobe's metrics.
pub use pdf_fonts;

/// Linearized files: a document written for reading over a network, and a
/// file that says it's linearized checked.
pub use pdf_linearize;

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

    /// The crate doc's example is the README's first, line for line, so that
    /// what docs.rs shows first and what crates.io shows first are one
    /// example, and each runs as a doctest.
    #[test]
    fn the_crate_docs_example_is_the_readmes_first() {
        let readme: Vec<&str> = (include_str!("../README.md").lines())
            .skip_while(|line| *line != "```rust")
            .skip(1)
            .take_while(|line| *line != "```")
            .collect();
        let doc: Vec<&str> = (include_str!("lib.rs").lines())
            .map_while(|line| line.strip_prefix("//!"))
            .map(|line| line.strip_prefix(' ').unwrap_or(line))
            .skip_while(|line| *line != "```")
            .skip(1)
            .take_while(|line| *line != "```")
            .collect();
        assert!(readme.len() > 10, "the README's first example: {readme:?}");
        assert_eq!(doc, readme);
    }

    /// What the crate gives a caller is sent to and shared by any thread: a
    /// file being read and what's read of it, a page's content and text, a
    /// document being written, and their options, caps and errors.
    #[test]
    fn what_it_gives_is_sent_and_shared() {
        fn shared<T: Send + Sync>() {}
        shared::<crate::Pdf>();
        shared::<crate::Pdf<&[u8]>>();
        shared::<crate::Pdf<std::sync::Arc<[u8]>>>();
        #[cfg(any(unix, windows))]
        shared::<crate::Pdf<pdf_structure::read::FileBytes>>();
        shared::<crate::Pages<'_, Vec<u8>>>();
        shared::<crate::Page<'_, Vec<u8>>>();
        shared::<crate::PagesIter<'_, Vec<u8>>>();
        shared::<crate::Content>();
        shared::<crate::NamedFont>();
        shared::<pdf_fonts::Shown>();
        shared::<crate::Document<'_>>();
        shared::<crate::Options<'_>>();
        shared::<crate::Limits>();
        shared::<crate::Error>();
    }
}

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
