//! PDF file structure reader, in-memory document model, and full or incremental writer
//! (ISO 32000-2, §7.5–7.6).
//!
//! `pdf-structure` parses PDF headers, cross-reference tables and streams along `/Prev` chains,
//! object streams (via [`pdf_filters`]), and encrypted documents (via [`pdf_crypt`]) into
//! span-annotated [`pdf_objects`] values. It also builds, edits, and writes full PDFs or
//! incremental updates.
//!
//! # Core Concepts
//!
//! - **On-demand and whole-file reading ([`read`]):**
//!   - [`Reading`] (`Reading::open`) parses the header, cross-reference sections, and security
//!     handler up front, then reads indirect objects and decodes object streams lazily on demand
//!     across threads from in-memory bytes or windowed file reads ([`FileBytes`], [`Identity`]).
//!   - [`Document`] ([`read::document`], `Reading::into_document`) holds all indirect objects,
//!     the [`Trailer`], [`Revision`] history, incremental [`Update`] metadata, and [`Unread`]
//!     object failures in memory for inspection and editing.
//!   - [`WithReading`] pairs a [`Document`] with its [`Reading`] so stream payloads can still be
//!     decoded on demand; [`Source`] unifies read access across all three.
//! - **Damaged-file recovery & diagnostics:** [`Options::recovery`] selects between
//!   [`read::Recovery::Verified`] (default offset-shift verification and fallback full-file scan)
//!   and [`read::Recovery::Pdfium`] (PDFium-compatible cross-reference and fallback scan rules).
//!   Every repair or syntax departure is recorded as a [`Fault`] (reported via
//!   [`Source::tell_faults`], or skipped at zero cost with [`read::Faults::Off`]), and [`Record`]
//!   and [`Facts`] describe how and where objects were found.
//! - **Encryption & streams:** Passwords in [`Options::passwords`] or [`Reading::unlock`]
//!   transparently decrypt strings, object streams, and stream payloads through [`pdf_crypt`]
//!   ([`Reading::decoded_within`], [`Reading::decoded_standard_within`], [`read::filter_chain`]).
//!   [`Encryption`] summarizes unlock state and permissions, and [`Limits`] bounds stream and
//!   total per-reading decode sizes.
//! - **Writing ([`write`](mod@write)):**
//!   - [`write::document`] and [`write::full`] write complete files with cross-reference tables
//!     or compressed cross-reference/object streams, optional renumbering, and preserved or new
//!     AES-256 encryption ([`write::Encryptor`], [`write::Encrypting`]).
//!   - [`write::update`] appends incremental updates onto a [`write::Base`], copying unmodified
//!     [`write::Body::Read`] bytes when verified against their read [`Record`] (or re-attached via
//!     [`Reading::adopt`]) and preventing RC4 keystream reuse.
//!   - [`write::Writer`] and [`write::UpdateWriter`] stream whole files and incremental updates one
//!     [`write::Body`] at a time to any `std::io::Write` (no `Seek`) with bounded scratch memory.
//!   - [`write::Parts`], [`write::Section`], and [`write::container`] expose low-level layout
//!     building blocks for custom writers.
//! - **Revisions:** Read a file as of an earlier cross-reference section ([`Options::section`])
//!   or open an earlier revision's byte prefix as a standalone file ([`Reading::prefix`],
//!   [`Prefix`]).
//! - **Tracing:** With the optional `tracing` feature, reader and writer phases emit
//!   [`tracing`](https://docs.rs/tracing) spans containing numeric offsets and counts only.
//!
//! Underlying crate types are re-exported at [`pdf_objects`], [`pdf_filters`], and [`pdf_crypt`].

mod crypt;
mod filter;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod phases;
// qpdf, the tests' oracle: a decoder to set the reader's decoding beside,
// and a second reader.
#[cfg(test)]
mod qpdf;
pub mod read;
pub mod source;
pub mod write;

pub use pdf_crypt;
pub use pdf_filters;
pub use pdf_objects;
#[cfg(any(unix, windows))]
pub use read::FileBytes;
pub use read::{
    ByteSource, Error, Fault, Identity, Limits, Options, Prefix, Reading, SourceId, WithReading,
};
pub use source::{
    DataExtent, Document, Encryption, Facts, Origin, Record, Revision, RevisionEnd, Source,
    Trailer, Unread, Update, XrefKind,
};

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
