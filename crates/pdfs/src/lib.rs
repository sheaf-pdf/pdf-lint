//! Read and write PDF files through one dependency: the crates of one
//! family, each re-exported whole, so that a caller names any of their
//! types through `pdfs`, at the versions it's built with, and gets one set
//! that works together; and the recipes that span them, with a viewer's
//! defaults on top.
//!
//! - **A file read** ([`open`] by its path, [`from_bytes`] from a source),
//!   under [`Options`] whose defaults are PDFium's: a [`Pdf`], each object
//!   read when it's first asked for, from any thread.
//! - **Its pages** ([`Pdf::pages`]), each with what it inherits and from
//!   where, and **a page's content** ([`Page::content`]), its streams
//!   decoded and joined, and its operations.
//! - **A page's text** ([`Page::text`]): the strings its content shows, in
//!   order, each in its font, the forms it draws followed, as PDFium reads
//!   them, with no layout; and **its fonts** ([`Page::fonts`]), each read once
//!   a file.
//! - **What the file is and says**: its version, its encryption, what its
//!   document information dictionary says and its XMP claims, whether its
//!   linearization holds, and any stream's data decoded.
//! - **One set of caps** ([`Limits`]) nesting each crate's, and **one
//!   error** ([`Error`]) wrapping each crate's.
//! - **Each recipe a function** ([`page`](mod@page), [`file`](mod@file))
//!   over the family's traits, for a caller with a reading of its own, and
//!   **what else a file holds** through the reading ([`Pdf::reading`]) and
//!   the crates.
//!
//! What a call meets that departs from the standard is told to the sink its
//! `_with` form is given, each crate's faults to that crate's sink; its plain
//! form tells nothing, and looks for nothing. Nothing reads past a cap
//! without saying so, and no file makes it panic.
//!
//! A page's content, read from a file in memory:
//!
//! ```
//! let file = b"%PDF-1.7\n\
//!     1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
//!     2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>\nendobj\n\
//!     3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n\
//!     4 0 obj\n<< /Length 27 >>\nstream\n0 0 1 rg 72 72 144 144 re f\nendstream\nendobj\n\
//!     xref\n0 5\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n\
//!     0000000139 00000 n \n0000000202 00000 n \n\
//!     trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n279\n%%EOF\n";
//!
//! let pdf = pdfs::from_bytes(&file[..]).unwrap();
//! let pages = pdf.pages().unwrap();
//! assert_eq!(pages.len(), 1);
//! let page = pages.get(0).unwrap();
//! assert_eq!(page.attributes().media_box().value.0, [0.0, 0.0, 612.0, 792.0]);
//!
//! let content = page.content().unwrap();
//! assert_eq!(content.bytes(), b"0 0 1 rg 72 72 144 144 re f");
//! let operators: Vec<Vec<u8>> = content
//!     .operations()
//!     .map(|operation| operation.unwrap().name().to_vec())
//!     .collect();
//! assert_eq!(operators, [&b"rg"[..], b"re", b"f"]);
//! ```

pub use error::Error;
pub use limits::Limits;
#[cfg(any(unix, windows))]
pub use open::open;
pub use open::{Options, from_bytes};
pub use page::{Content, NamedFont};
pub use pdf::{Page, Pages, PagesIter, Pdf};

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
}

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
