//! PDF content streams (ISO 32000-2, 7.8.2), read as PDFium reads them: a
//! stream's tokens and its operations, each with where it is and its bytes
//! as written, and a page's streams as one, every place in them mapped
//! back.
//!
//! - **Tokens** ([`tokens`]): operators, operands as written, and inline
//!   images, `BI` to `EI`, each one token. Tokens end where PDFium's
//!   `CPDF_StreamParser` ends them, and an inline image's data is found by
//!   PDFium's `ReadInlineStream` rules, from the components of the color
//!   spaces the caller gives ([`ResCx`]). What an operator does is no part
//!   of it, so one nobody knows is a token like any other.
//! - **Operations** ([`operations`]): each operator, or inline image
//!   ([`InlineImage`]), with the operands before it ([`Operation`]), their
//!   ranges held in the item, so an operation of sixteen operands or fewer
//!   allocates nothing. Operand `i` is taken from the last, as PDFium takes
//!   them ([`Operation::arg`]), and every operand is kept, up to the cap.
//! - **Operands read when asked** ([`Operand`]): a name, its `#` escapes
//!   decoded ([`name`]), any operand as a pdf-objects
//!   [`Object`](pdf_objects::Object) ([`object`]), an array's and a
//!   dictionary's elements each read so, and an array's elements as
//!   written ([`Operand::elements`]).
//! - **A page as one** ([`Concat`]): its decoded `/Contents` streams
//!   joined with a space between each, as PDFium joins them, and an
//!   offset in them mapped back to a stream and a place in it ([`Pos`],
//!   [`SpanAcross`]); joined as they're decoded ([`Join`]), under a cap,
//!   each decoded straight into the joined bytes.
//! - **Caps** ([`Limits`]) **and errors that say where** ([`Error`]): a
//!   string, array, dictionary or inline image that doesn't end, or one
//!   past a cap. It never panics, whatever the bytes, and every token moves
//!   it on.
//!
//! No resolver: what's read from the file, the streams decoded and the
//! color spaces' components, the caller gives.

mod error;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod inline;
mod limits;
mod operand;
mod operations;
mod page;
mod scan;
mod tokens;

pub use error::{Error, ErrorKind};
pub use inline::{InlineImage, ResCx, family_components};
pub use limits::Limits;
pub use operand::{Elements, Operand, name, object};
pub use operations::{Operation, Operations, operations};
pub use page::{Concat, Join, Pos, Pushed, SpanAcross, TooLarge};
pub use tokens::{Tok, Tokens, tokens};

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

#[cfg(test)]
mod tests;
