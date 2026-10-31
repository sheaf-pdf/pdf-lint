//! PDF's file structure, read, and a document of it in memory (ISO
//! 32000-2, 7.5): a file's header, its cross-reference tables and streams
//! along their `/Prev` chain, and each object at the offset they give,
//! through pdf-objects' parser, every value, key and stream's data with
//! where it was written. Where the sections don't read, it finds the
//! objects by scanning the file, and says so. What it reads is a
//! document, which a caller can build in memory too, to number new
//! objects in, edit, walk and prune. And it writes a file of numbered
//! objects, whole or as an incremental update, and what a file's structure
//! is written in: cross-reference sections, and object streams, an
//! encrypted file's objects encrypted as they're written.
//!
//! - **What it read, in one value:** a [`Document`], a [`Source`] read
//!   whole: each object its cross-references list, kept with where it was
//!   read while it's as read, or why it doesn't read ([`Unread`]); the
//!   [`Trailer`], and why it isn't the newest section's when it isn't;
//!   the header's version; the file's newest incremental [`Update`], and
//!   each of its revisions, the first first, with its trailer
//!   ([`Revision`]).
//! - **A document in memory:** a [`Document`] built, or read whole, then
//!   added to, its new objects numbered past every number the file lists;
//!   its references followed, and what its trailer reaches walked, the
//!   rest pruned.
//! - **Or read on demand:** a [`Reading`], opened, then each object read
//!   when it's first asked for, kept or not, from any thread, each object
//!   stream decoded once; read whole, it's the [`Document`], by the same
//!   code, the reading taken or kept beside it, to hand out what the
//!   document doesn't hold. Any stream's data is handed out when it's
//!   asked for, decrypted in an encrypted file, and decoded. Its bytes are
//!   in memory, or a file is read by position, a window at a time
//!   ([`FileBytes`]), whose [`Identity`], holding the file open, tells
//!   whether it's changed. Where each object was read from is a
//!   [`Record`], and what the file's header and sections say of it, as
//!   written, its [`Facts`].
//! - **Every choice said:** what the reader had to choose how to read is a
//!   [`Warning`], with its object and where.
//! - **Object streams and cross-reference streams decoded** from their own
//!   data, through pdf-filters, under limits on what a stream and a
//!   reading decode to, which a caller sets ([`Limits`], in [`Options`]);
//!   the objects read from an object stream placed in its decoded data,
//!   which the [`Document`] keeps.
//! - **Encrypted files opened** with the passwords given, each a
//!   [`Password`] as typed, tried in each of its encodings
//!   ([`Reading::open_with`], [`bytes::read_opened`]); or opened, then
//!   unlocked with the bytes to try ([`Reading::unlock`], [`Encoded`]),
//!   through pdf-crypt's standard security handler: object streams
//!   decrypted before they're decoded, strings decrypted where they are;
//!   [`Encryption`] says whether the file opened, and by which password.
//!   What's decrypted and what's left as written is listed in [`bytes`].
//! - **A revision before an update** read as the file was then
//!   ([`bytes::read_from`]).
//! - **Whole files and updates written** ([`write::full`],
//!   [`write::document`], [`write::update`]): numbered objects, a
//!   document's among them, as a whole file, its objects in object streams
//!   built on threads where it's asked, or as an incremental update after
//!   the file they were read from, its trailer carried; what each wrote
//!   given back ([`write::Written`]), for an update after it. An object as
//!   it was read is copied as the bytes it was read from where they're
//!   right to copy ([`write::Body::Read`]), checked against a hash of them
//!   taken as it was read, its [`Record`]'s; a document read whole is
//!   copied so once a reading of its file opened again takes its objects
//!   as its own ([`Reading::adopt`]).
//! - **Sections and object streams written** ([`write`](mod@write)): a
//!   whole file's cross-reference section or an update's, as a table or a
//!   stream, a whole file's stream deflated under PNG's Up predictor; and
//!   object streams of the objects that may go in one, deflated, no
//!   bigger than the reader decodes. Deflated through pdf-filters, by the
//!   backend and at the level the caller says ([`write::Deflate`]); what
//!   can't be written refused, with nothing written ([`write::Error`]).
//!   An encrypted file's objects encrypted as pdf-objects' writer writes
//!   them, encrypted as a reading's file is or under a new AES-256
//!   encryption ([`write::Encryptor`]), by the same list of what's left in
//!   the clear that the reader decrypts by ([`write::Encrypting`]).
//!
//! The objects are pdf-objects' types, re-exported as [`pdf_objects`];
//! a stream that doesn't decode cleanly says so in pdf-filters' error,
//! re-exported as [`pdf_filters`], whose faults say why; and the security
//! handler, its passwords and its refusals are pdf-crypt's, re-exported as
//! [`pdf_crypt`].

pub mod bytes;
mod crypt;
mod filter;
// The tests' file writer; for other crates' tests and fuzz bodies too,
// with `fixtures`, the feature it's named for.
#[cfg(any(test, feature = "fixtures"))]
#[doc(hidden)]
pub mod fixture;
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
pub mod write;

#[cfg(any(unix, windows))]
pub use bytes::FileBytes;
pub use bytes::{ByteSource, Error, Identity, Limits, Options, Reading, SourceId};
pub use pdf_crypt;
pub use pdf_crypt::{Encoded, Encoding, Password};
pub use pdf_filters;
pub use pdf_objects;
pub use source::{
    DataExtent, Document, Encryption, Facts, Origin, Record, Revision, Source, Trailer, Unread,
    Update, Warning, XrefKind,
};

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
