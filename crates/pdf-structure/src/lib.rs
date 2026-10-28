//! PDF's file structure, read (ISO 32000-2, 7.5): a file's header, its
//! cross-reference tables and streams along their `/Prev` chain, and each
//! object at the offset they give, through pdf-objects' parser, every
//! value, key and stream's data with where it was written. Where the
//! sections don't read, it finds the objects by scanning the file, and
//! says so.
//!
//! - **What it read, in one value:** a [`Document`], a [`Source`] read
//!   whole: each object its cross-references list, or why it doesn't
//!   read; the [`Trailer`], and why it isn't the newest section's when it
//!   isn't; the header's version; and the file's newest incremental
//!   [`Update`].
//! - **Every choice said:** what the reader had to choose how to read is a
//!   [`Warning`], with its object and where.
//! - **Object streams and cross-reference streams decoded** from their own
//!   data, through pdf-filters, under limits on what a stream and a
//!   reading decode to ([`bytes::STREAM`], [`bytes::BUDGET`]); the objects
//!   read from an object stream placed in its decoded data, which the
//!   [`Document`] keeps.
//! - **Encrypted files opened** with the passwords given
//!   ([`bytes::read_opened`]), through pdf-crypt's standard security
//!   handler: object streams decrypted before they're decoded, strings
//!   decrypted where they are; [`Encryption`] says whether the file opened.
//!   What's decrypted and what's left as written is listed in [`bytes`].
//! - **A revision before an update** read as the file was then
//!   ([`bytes::read_from`]).
//!
//! The objects are pdf-objects' types, re-exported as [`pdf_objects`].

pub mod bytes;
mod filter;
#[cfg(any(test, feature = "fixtures"))]
mod json;
// qpdf, the tests' oracle: a second reader, and a decoder to set the
// reader's decoding beside; for other crates' tests too, with `fixtures`.
#[cfg(any(test, feature = "fixtures"))]
#[doc(hidden)]
pub mod qpdf;
pub mod source;
// The tests' file writer; for other crates' tests too, with `fixtures`,
// and for a fuzz body's seeds.
#[cfg(any(test, fuzzing, feature = "fixtures"))]
#[doc(hidden)]
pub mod write;

pub use pdf_objects;
pub use source::{Document, Encryption, Source, Trailer, Update, Warning};

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
