//! PDF's objects (ISO 32000-2, 7.3), as a reader read them: owned, each
//! value with where it was written when the reader knows, and each
//! dictionary as written, in order and with any key written twice.
//!
//! - **Where each value was written:** an [`Object`] is a [`Value`] and
//!   its [`Span`], a run of the file's bytes or of an object stream's
//!   decoded data, so what reads it can point at the bytes it's about.
//! - **Dictionaries as written:** a [`Dict`] keeps its entries in order,
//!   a key written twice and each key's span among them; a lookup takes a
//!   key's last entry, as readers do, and [`Dict::duplicates`] gives the
//!   others.
//! - **Strings as their bytes:** a [`Str`] is its bytes, its escapes
//!   decoded; or text a reader decoded without saying from which encoding,
//!   or ciphertext it didn't decrypt, where what depends on the bytes
//!   can't be told.
//! - **Text strings decoded by the spec** ([`text`]): UTF-16BE or UTF-8
//!   after their byte order marks, PDFDocEncoding without one, and where a
//!   string departs from the spec, said.
//! - **Versions** ([`Version`]), read from a header's or a catalog's text,
//!   in order.
//!
//! [`dict!`] builds a dictionary from keys and values.

pub mod object;
pub mod text;
pub mod version;

pub use object::{Dict, Entry, Object, ObjectId, Span, Str, Stream, Value};
pub use version::Version;

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
