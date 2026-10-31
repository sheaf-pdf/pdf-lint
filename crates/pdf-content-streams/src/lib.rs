//! PDF content streams (ISO 32000-2, 7.8.2), read as PDFium reads them, as
//! of chromium/8125: a stream's tokens and its operations, each with where
//! it is and its bytes as written, and a page's streams as one, every place
//! in them mapped back.
//!
//! - **Tokens** ([`tokens`]): operators, operands as written, and inline
//!   images, `BI` to `EI`, each one token. Tokens end where PDFium's
//!   `CPDF_StreamParser` ends them, pdf-tokenizer's tokens read by its
//!   PDFium rules, an array or a dictionary where its `ReadNextObject`
//!   ends it. An inline image's dictionary is read as PDFium's
//!   `Handle_BeginImage` structures it, a `BI` with a keyword where a key
//!   should be no image but an operator of its own; and its data is found
//!   by PDFium's `ReadInlineStream` rules, from the components of the
//!   color spaces the caller gives ([`ResCx`]): Flate and LZW data decoded
//!   to where the decoder stops reading it, as PDFium decodes it, by
//!   [pdf-filters](https://crates.io/crates/pdf-filters), Flate on
//!   zlib-rs; DCT and CCITTFax data, which PDFium decodes too, by a
//!   stand-in, or so where a reading asks for it ([`CodedEnds`], with the
//!   `image-ends` feature, below; [`tokens`] says where). What an operator
//!   does is no part of it, so one nobody knows is a token like any other.
//! - **Operations** ([`operations`]): each operator, or inline image
//!   ([`InlineImage`]), with the operands before it ([`Operation`]), their
//!   ranges held in the item, so an operation of sixteen operands or fewer
//!   allocates nothing. Operand `i` is taken from the last, as PDFium takes
//!   them ([`Operation::arg`]), and every operand is kept, up to the cap.
//! - **Operands read when asked** ([`Operand`]), each as PDFium's content
//!   parser reads it: a name, its `#` escapes decoded ([`name`]), a number
//!   ([`Operand::number`]), any operand as a pdf-objects
//!   [`Object`](pdf_objects::Object) ([`object`]), its numbers held at 64
//!   bits as pdf-objects holds them, an array's and a dictionary's
//!   elements each read so, and an array's elements as written
//!   ([`Operand::elements`]). Where ISO 32000-2 reads one otherwise, that's
//!   a fault ([`fault`]); a reader that wants ISO's reading of an operand
//!   reads its bytes with pdf-objects' `Parser`, by ISO's rules.
//! - **Operators** ([`Operator`]): each of ISO 32000-2's Annex A by its
//!   name ([`Operation::operator`]), with the operands the clause defining
//!   it gives it ([`Operands`], [`Type`]), its category of 8.2
//!   ([`Category`]) and that clause, and sets of them ([`Operators`]); and
//!   where each may be used, the graphics objects of 8.2 ([`State`]), so
//!   content can be stepped through them ([`State::step`]).
//! - **A page as one** ([`Concat`]): its decoded `/Contents` streams
//!   joined with a space between each, as PDFium joins them, and an
//!   offset in them mapped back to a stream and a place in it ([`Pos`],
//!   [`SpanAcross`]); joined as they're decoded ([`Join`]), under a cap,
//!   each decoded straight into the joined bytes.
//! - **A page's balance, marked content and names** ([`balance`],
//!   [`marked`], [`names_used`]): its `q`/`Q` balance, its marked
//!   content's ranges, each with its tag and property list, and the
//!   resource names it uses, by category, or what it keeps once ranges of
//!   it are taken out ([`kept`]). Each is counted an operation at a time
//!   ([`Balance::take`], [`MarkedNest::step`], [`NameSet::take`]), so one
//!   pass can find several.
//! - **Caps** ([`Limits`]) **and errors that say where** ([`Error`]): a
//!   string, array, dictionary or inline image that doesn't end, one past
//!   a cap, among them what finding inline images' ends may cost in a
//!   reading, decoding a deflate bomb or reading many images again, or
//!   arrays and dictionaries nested deeper than PDFium reads. It never
//!   panics, whatever the bytes, and every token moves it on; what's open
//!   is kept on the heap, however deep.
//! - **Where ISO 32000-2 reads it otherwise** ([`fault`]): each place its
//!   rules would read the bytes another way than PDFium's, told with where
//!   it is to a sink the caller gives ([`Tokens::with_sink`],
//!   [`Operations::with_sink`], [`Operand::object_with`]); what's read is
//!   the same. With none, `()`, none is looked for. One isn't told: 0x80
//!   and 0xFF read as white-space ([`fault`] says).
//! - **Resources by what they are** ([`Res`]): a resource content draws
//!   with, its objects named by a 128-bit digest of them ([`ResId`],
//!   XXH3-128), so that equal resources are written once and named alike,
//!   however their dictionaries are written.
//!
//! No resolver: what's read from the file, the streams decoded, the color
//! spaces' components and the property lists, the caller gives.
//!
//! # Features
//!
//! - **`image-ends`**, off by default: an inline image's DCT data decoded
//!   on [zune-jpeg](https://crates.io/crates/zune-jpeg), and its CCITTFax
//!   data on [hayro-ccitt](https://crates.io/crates/hayro-ccitt), to find
//!   where it ends, as PDFium decodes them, what's decoded not kept; its
//!   output counted against [`Limits::image_ends`] before it's made. It
//!   adds `CodedEnds::Decoded`, which a reading names in
//!   [`Limits::coded_ends`] to find ends so: turned on, by this crate's
//!   user or by any other crate built beside it, it changes nothing a
//!   reading that doesn't name it gives. It needs Rust 1.87, where the
//!   crate needs 1.85. zune-jpeg is built without its SIMD features
//!   (`x86`, `neon`), which hold all its `unsafe`, so it builds with
//!   `unsafe_code` forbidden, as hayro-ccitt does always; a crate built
//!   beside this one that turns those features on, as Cargo's features
//!   are additive, turns them on here too.

mod balance;
mod data;
#[cfg(feature = "image-ends")]
mod decoders;
mod error;
pub mod fault;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod inline;
mod limits;
mod marked;
mod names;
mod operand;
mod operations;
mod operator;
mod page;
mod res;
mod scan;
mod state;
mod tokens;

/// pdf-objects, whose values operands are read as and property lists are
/// given in: named through it, they're the version this crate is built
/// with.
pub use pdf_objects;

pub use balance::{Balance, balance};
pub use error::{Error, ErrorKind};
pub use inline::{ImageStream, InlineImage, ResCx, family_components};
pub use limits::{CodedEnds, Limits};
pub use marked::{Marked, MarkedEvent, MarkedNest, Properties, Props, marked};
pub use names::{Kept, NameSet, ResKind, kept, names_used};
pub use operand::{Elements, Operand, name, object};
pub use operations::{Operation, Operations, operations};
pub use operator::{Category, FillRule, Operands, Operator, Operators, Paint, Type};
pub use page::{Concat, Join, Pos, Pushed, SpanAcross, TooLarge};
pub use res::{Res, ResBody, ResId};
pub use state::State;
pub use tokens::{Tok, Tokens, tokens};

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

#[cfg(test)]
mod tests;
