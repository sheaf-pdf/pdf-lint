//! The workspace's tests' PDF files, and qpdf, their oracle: never
//! published, a dev-dependency of each crate whose tests use it, by its
//! path alone.
//!
//! - **Files written from objects** ([`fixture`]): pdf-objects' values
//!   written as PDF syntax, as whole files with a cross-reference table or
//!   an uncompressed cross-reference stream, encrypted by pdf-crypt's
//!   writer or as a new file is, or assembled from objects' bytes as
//!   written by hand; and data written through a filter's encoding, for a
//!   stream to decode.
//! - **qpdf as a second reader** ([`qpdf`]): the program `PDF_LINT_QPDF`
//!   names, else `qpdf` on the `PATH`, run as a process, a file read
//!   through its JSON alone, and a stream, an object stream's objects or
//!   an encrypted file's strings decoded by it, to set beside the reader's.
//!   A stream qpdf decodes alone is handed to it in a file of its own,
//!   which tempfile makes in the temp folder and removes after.
//!
//! Its answers are pdf-objects' values, bytes and text, so a crate that
//! pdf-fixtures depends on (pdf-structure) takes it in its own tests, a
//! second copy of that crate then linked whose types its tests never meet.

pub mod fixture;
mod json;
pub mod qpdf;
