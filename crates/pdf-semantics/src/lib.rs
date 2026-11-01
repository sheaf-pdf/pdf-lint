//! PDF's common structures, read and written, each value saying where it
//! came from.
//!
//! - **Dates** ([`date`]): ISO 32000-2, 7.9.4. [`date::Date::read`] reads
//!   one, each field saying whether it was written and what stands for it
//!   if not, and each departure from the clause said; [`date::Date::write`]
//!   writes the clause's form. With the `clock` feature, on by default, a
//!   date is had from the clock or a count of seconds.
//! - **Text strings** ([`text`]): ISO 32000-2, 7.9.2.2, pdf-objects' module,
//!   read by ISO's rules or PDFium's and written.

pub use pdf_objects::text;

pub mod date;

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
