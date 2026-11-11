//! Read PDF content streams (ISO 32000-2, 7.8.2) as PDFium reads them, as
//! of chromium/8125, write them, and take ranges out of them. No file is
//! read here: the caller gives a page's streams and what its resources
//! hold, or a reader of the file to take them from.
//!
//! - **Read** a stream's tokens ([`tokens`](fn@tokens)) and operations
//!   ([`operations`](fn@operations)), each with its range and its bytes as
//!   written, and an [`Operand`]'s value when it's asked for ([`name`],
//!   [`object`]).
//! - **Find where an inline image ends** ([`InlineImage`]) as PDFium finds
//!   it. An unfiltered image ends at its size, counted from its color
//!   space's components ([`ResCx::read`]). Flate and LZW data is decoded
//!   through pdf-filters to where PDFium's decoders stop. DCT and CCITTFax
//!   data is decoded with the `image-ends` feature, where a reading asks
//!   ([`Limits::coded_ends`]).
//! - **Read a page's streams as one** ([`Concat`], [`Join`],
//!   [`Join::push_from`]), each place mapped back to its stream ([`Pos`],
//!   [`SpanAcross`]).
//! - **Know what each operator is** ([`Operator`]) and where it may be used
//!   ([`State`]).
//! - **Check a page**: its `q`/`Q` balance ([`balance`](fn@balance)), its
//!   marked content ([`marked`](fn@marked)) and the resource names it uses
//!   ([`names_used`], [`kept`]).
//! - **See where ISO 32000-2 reads content otherwise** ([`fault`]), told to
//!   a sink you give.
//! - **Write content** ([`ContentWriter`]), its resources ([`Res`]) named
//!   when it's finished, clear of what a resources dictionary binds
//!   ([`Reserved`]).
//! - **Take ranges out of a page** ([`splice`](fn@splice), [`splice_each`],
//!   [`without_keys`]), only where nothing drawn after them can change.
//!
//! Reading is bounded by [`Limits`], each cap met before the work it
//! bounds, and it never panics on content. A `Debug` never prints a
//! string's or an image's bytes. pdf-objects and pdf-color are re-exported
//! whole, so their types are named at the versions this crate is built
//! with.
//!
//! # Features
//!
//! All off by default:
//!
//! - **`image-ends`**: DCT and CCITTFax data's ends found by decoding it,
//!   on zune-jpeg and hayro-ccitt, where a reading asks for it.
//! - **`simd`**: the dependencies' SIMD, picked as they run by what the CPU
//!   has. Nothing read or digested changes. The `unsafe` code it turns on
//!   is theirs.
//! - **`tracing`**: a span over each of a page's passes ([`balance`],
//!   [`marked`](fn@marked), [`names_used`], [`kept`]), its edits
//!   ([`splice`](fn@splice), [`splice_each`] and each of its checks,
//!   [`without_keys`], [`Spliced::streams`]), the writer's names
//!   ([`ContentWriter::named`]) and a resources dictionary's reading
//!   ([`ResCx::read`], [`Reserved::read`]). Reading tokens and operations
//!   has none, as its loop is the caller's. A span's fields are lengths
//!   and counts, never content.
//!
//! A page's two streams, taken from a reader of the file and read as one:
//!
//! ```
//! use pdf_content_streams::pdf_objects::{Object, ObjectId, Unreadable, WithStreams, Why};
//! use pdf_content_streams::{Join, Limits, Operator, Pos, Properties, ResCx, balance, marked};
//!
//! // A reader that gives streams 4 and 5, decoded.
//! let nothing = |_: ObjectId| None::<&Object>;
//! let streams = WithStreams::new(nothing, |id: ObjectId, _limit: usize| match id.number {
//!     4 => Ok(b"q /Span <</MCID 0>> BDC BT (Hel".to_vec()),
//!     5 => Ok(b"lo) Tj ET EMC Q".to_vec()),
//!     _ => Err(Unreadable::new(id, Why::Damaged, "no such stream")),
//! });
//! let mut join = Join::new(&Limits::DEFAULT);
//! join.push_from(&streams, ObjectId::new(4, 0)).unwrap();
//! join.push_from(&streams, ObjectId::new(5, 0)).unwrap();
//! let page = join.finish();
//!
//! // Each operation, with the operands before it.
//! let res = ResCx::none();
//! let ops: Vec<_> = page.operations(&res, &Limits::DEFAULT).collect::<Result<_, _>>().unwrap();
//! let names: Vec<_> = ops.iter().map(|op| op.name()).collect();
//! assert_eq!(names, [&b"q"[..], b"BDC", b"BT", b"Tj", b"ET", b"EMC", b"Q"]);
//! assert_eq!(ops[3].operator(), Operator::Show);
//!
//! // Operands are read when asked. Operand 0 is the last, as PDFium takes them.
//! assert_eq!(ops[1].arg(1).unwrap().name().as_deref(), Some(&b"Span"[..]));
//! let props = ops[1].arg(0).unwrap().object(&Limits::DEFAULT).unwrap();
//! let mcid = props.as_dict().and_then(|d| d.get(b"MCID"));
//! assert_eq!(mcid.and_then(Object::as_integer), Some(0));
//!
//! // The string runs from the first stream into the second, as in PDFium.
//! let shown = page.span(ops[3].range());
//! assert_eq!(shown.start, Pos { stream: 0, at: 27 });
//! assert_eq!(shown.end, Pos { stream: 1, at: 6 });
//!
//! // The page's `q`/`Q` balance, and its marked content.
//! assert_eq!(balance(&page, &res, &Limits::DEFAULT).unwrap().net, 0);
//! let lists = Properties::default();
//! let found = marked(&page, &lists, &res, &Limits::DEFAULT).unwrap();
//! assert_eq!(found[0].tag, b"Span");
//! assert_eq!(found[0].span.to_string(), "stream 0 byte 2 to stream 1 byte 13");
//! ```

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
pub mod iter;
mod limits;
mod marked;
mod names;
mod operand;
mod operations;
mod operator;
mod page;
mod phases;
mod res;
mod scan;
mod splice;
mod state;
mod tokens;
mod unprinted;
mod write;

/// pdf-objects, whose values operands are read as and property lists are
/// given in: named through it, they're the version this crate is built
/// with.
pub use pdf_objects;

/// pdf-color, whose [`Limits`](pdf_color::Limits) [`Limits::color`] is:
/// named through it, it's the version this crate is built with.
pub use pdf_color;

pub use balance::{Balance, balance};
pub use error::{Error, ErrorKind};
pub use inline::{ImageStream, InlineImage, ResCx};
pub use limits::{CodedEnds, Limits};
pub use marked::{Marked, MarkedEvent, MarkedNest, Properties, Props, marked};
pub use names::{Kept, NameSet, ResKind, kept, names_used};
pub use operand::{Elements, Operand, name, object};
pub use operations::{Operation, Operations, operations};
pub use operator::{Category, FillRule, Operands, Operator, Operators, Paint, Type};
pub use page::{Concat, Join, Pos, Pushed, SpanAcross, TooLarge};
pub use res::{Res, ResBody, ResId, Unheld};
pub use splice::{Rewrite, SpliceError, Spliced, splice, splice_each, without_keys};
pub use state::State;
pub use tokens::{Tok, Tokens, tokens};
pub use write::{
    Blend, Color, ContentWriter, GState, LineCap, LineJoin, Need, Reserved, ResourceNeeds,
    TextMode, write_number, write_string,
};

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

#[cfg(test)]
mod tests;
