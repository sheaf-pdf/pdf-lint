//! Read and write PDF files through one dependency: the crates of one
//! family, each re-exported whole, so that a caller names any of their
//! types through `pdfs`, at the versions it's built with, and gets one set
//! that works together.

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
