//! PDF's file structure, read (ISO 32000-2, 7.5): a file's header, its
//! cross-reference tables and streams along their `/Prev` chain, and each
//! object at the offset they give, through pdf-objects' parser, every
//! value, key and stream's data with where it was written. Where the
//! sections don't read, it finds the objects by scanning the file, and
//! says so.
//!
//! - **What it read, in one value:** a [`Document`], a [`Source`] read
//!   whole: each object its cross-references list, or why it doesn't
//!   read ([`Unread`]); the [`Trailer`], and why it isn't the newest
//!   section's when it isn't; the header's version; and the file's newest
//!   incremental [`Update`].
//! - **Or read on demand:** a [`Reading`], opened, then each object read
//!   when it's first asked for, kept or not, from any thread, each object
//!   stream decoded once; read whole, it's the [`Document`], by the same
//!   code. Any stream's data is handed out when it's asked for, decrypted
//!   in an encrypted file, and decoded. Its bytes are in memory, or a file
//!   is read by position, a window at a time ([`FileBytes`]), whose
//!   [`Identity`], holding the file open, tells whether it's changed.
//!   Where each object was read from is a [`Record`], and what the file's
//!   header and sections say of it, as written, its [`Facts`].
//! - **Every choice said:** what the reader had to choose how to read is a
//!   [`Warning`], with its object and where.
//! - **Object streams and cross-reference streams decoded** from their own
//!   data, through pdf-filters, under limits on what a stream and a
//!   reading decode to, which a caller sets ([`Limits`], in [`Options`]);
//!   the objects read from an object stream placed in its decoded data,
//!   which the [`Document`] keeps.
//! - **Encrypted files opened** with the passwords given
//!   ([`bytes::read_opened`]), through pdf-crypt's standard security
//!   handler: object streams decrypted before they're decoded, strings
//!   decrypted where they are; [`Encryption`] says whether the file opened.
//!   What's decrypted and what's left as written is listed in [`bytes`].
//! - **A revision before an update** read as the file was then
//!   ([`bytes::read_from`]).
//!
//! The objects are pdf-objects' types, re-exported as [`pdf_objects`];
//! and a stream that doesn't decode cleanly says so in pdf-filters' error,
//! re-exported as [`pdf_filters`], whose faults say why.

pub mod bytes;
mod filter;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
#[cfg(any(test, feature = "fixtures"))]
mod json;
// qpdf, the tests' oracle: a second reader, and a decoder to set the
// reader's decoding beside; for other crates' tests too, with `fixtures`.
#[cfg(any(test, feature = "fixtures"))]
#[doc(hidden)]
pub mod qpdf;
pub mod source;
// The tests' file writer; for other crates' tests and fuzz bodies too,
// with `fixtures`.
#[cfg(any(test, feature = "fixtures"))]
#[doc(hidden)]
pub mod write;

#[cfg(any(unix, windows))]
pub use bytes::FileBytes;
pub use bytes::{ByteSource, Error, Identity, Limits, Options, Reading, SourceId};
pub use pdf_filters;
pub use pdf_objects;
pub use source::{
    DataExtent, Document, Encryption, Facts, Origin, Record, Source, Trailer, Unread, Update,
    Warning, XrefKind,
};

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
