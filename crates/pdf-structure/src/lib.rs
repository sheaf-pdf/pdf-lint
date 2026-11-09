//! PDF's file structure, read, and a document of it in memory (ISO
//! 32000-2, 7.5): a file's header, its cross-reference tables and streams
//! along their `/Prev` chain, and each object at the offset they give,
//! through pdf-objects' parser, every value, key and stream's data with
//! where it was written. Where a file's sections or objects aren't where
//! it says, it reads them where the file proves they are, or scans the
//! file for them, and says so. What it reads is a
//! document, which a caller can build in memory too, to number new
//! objects in, edit, walk and prune. And it writes a file of numbered
//! objects, whole or as an incremental update, and what a file's structure
//! is written in: cross-reference sections, and object streams, an
//! encrypted file's objects encrypted as they're written.
//!
//! - **What it read, in one value:** a [`Document`], a [`Source`] read
//!   whole ([`read::document`]): each object its cross-references list,
//!   kept with where it was read while it's as read, or why it doesn't
//!   read ([`Unread`]); the [`Trailer`], and why it isn't the newest
//!   section's when it isn't; the header's version; the file's newest
//!   incremental [`Update`], and each of its revisions, the first first,
//!   with its trailer ([`Revision`]).
//! - **A document in memory:** a [`Document`] built, or read whole, then
//!   added to, its new objects numbered past every number the file lists;
//!   its references followed, and what its trailer reaches walked, the
//!   rest pruned.
//! - **Or read on demand:** a [`Reading`], opened under [`Options`]
//!   ([`Reading::open`], the one way in), then each object read when it's
//!   first asked for, kept or not, from any thread, each object stream
//!   decoded once, a [`Source`] of what it has read so far; read whole,
//!   it's the [`Document`], by the same code, the reading taken or kept
//!   beside it, to hand out what the document doesn't hold
//!   ([`WithReading`], the two together, a [`Source`] that gives a
//!   stream's data decoded). Any stream's data is handed out when it's
//!   asked for, decrypted in an encrypted file, and decoded. Its bytes are
//!   in memory, or a file is read by position, a window at a time
//!   ([`FileBytes`]), whose [`Identity`], holding the file open, tells
//!   whether it's changed. Where each object was read from is a
//!   [`Record`], and what the file's header and sections say of it, as
//!   written, its [`Facts`].
//! - **Every choice said:** what the reader had to choose how to read, or
//!   read past, is a [`Fault`], kept at its place, the object it's in and
//!   where it was written, as pdf-objects' `fault::Place` says it, and told
//!   to any of pdf-objects' `fault::Sink`s ([`Source::tell_faults`]); none
//!   looked for, at no cost, where the caller wants none
//!   ([`Options::faults`], [`read::Faults::Off`]).
//! - **A damaged file read where it proves it is:** a section or an object
//!   a few bytes off is read where the file shows it is
//!   ([`read::Recovery::Verified`], the default), each recovery a
//!   [`Fault`] and each object read elsewhere its [`Origin`]. Or the file
//!   is read as PDFium reads it ([`read::Recovery::Pdfium`]), for a caller
//!   that shows it as a viewer would. The choice is
//!   [`Options::recovery`].
//! - **Its phases traced**, with the `tracing` feature: the open, its
//!   sections and a scan, each object stream decoded, a whole read, an
//!   earlier revision's open, and each write and the object streams it
//!   builds, each a span of [`tracing`](https://docs.rs/tracing), at
//!   `debug` (an object stream decoded at `trace`), its fields numbers
//!   alone, never a value, a key, a password or plaintext. Off by default,
//!   and then nothing of them is compiled.
//! - **Object streams and cross-reference streams decoded** from their own
//!   data, through pdf-filters, under limits on what a stream and a
//!   reading decode to, which a caller sets ([`Limits`], in [`Options`]);
//!   the objects read from an object stream placed in its decoded data,
//!   which the [`Document`] keeps.
//! - **Encrypted files opened** with the passwords given, each a
//!   [`Password`](pdf_crypt::Password) as typed, tried in each of its
//!   encodings ([`Options::passwords`]); or opened, then unlocked with the
//!   bytes to try ([`Reading::unlock`],
//!   [`Encoded`](pdf_crypt::Encoded)), through pdf-crypt's standard
//!   security handler: object streams decrypted before they're decoded,
//!   strings decrypted where they are; [`Encryption`] says whether the file
//!   opened, by which password, and how it's encrypted. What's decrypted
//!   and what's left as written is listed in [`read`].
//! - **A revision before an update** read as the file was then
//!   ([`Options::section`]), or the file as a revision was written, its
//!   bytes up to the revision's end ([`Revision::end`]) read as a file of
//!   their own ([`Reading::prefix`], [`Prefix`]).
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
//! - **Bodies written one at a time** ([`write::Parts`]), for a caller that
//!   lays a file out itself: each at the number it gives, under the
//!   encryption [`write::Encryptor::laid_out`] gives for those numbers,
//!   copied where that's the key it was read under.
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
//!   the clear that the reader decrypts by ([`write::Encrypting`]). AES's
//!   IVs come from the operating system, and under RC4 no new plaintext
//!   goes under a keystream the file used, but where the encryptor is told
//!   to reuse them, which lists each.
//!
//! The objects are pdf-objects' types, re-exported as [`pdf_objects`];
//! a stream that doesn't decode cleanly says so in pdf-filters' error,
//! re-exported as [`pdf_filters`], whose faults say why; and the security
//! handler, its passwords and its refusals are pdf-crypt's, re-exported as
//! [`pdf_crypt`].

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
