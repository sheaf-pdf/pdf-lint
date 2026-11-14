//! PDF content stream lexer, operator parser, page analyzer, writer, and byte-preserving splicer
//! (ISO 32000-2, §7.8.2), matching PDFium's parsing rules as of chromium/8125.
//!
//! `pdf-content-streams` operates on decoded stream bytes and resource dictionaries (or any
//! `Streams` / `Resolve` provider) without performing file I/O directly.
//!
//! # Core Concepts
//!
//! - **Tokens & operations ([`tokens`](fn@tokens), [`operations`](fn@operations), [`Operand`]):**
//!   Lexes tokens and operator/operand sequences with byte spans, deferring operand decoding until
//!   requested ([`name`], [`object`]). [`Operator`] and [`State`] type ISO 32000-2 Annex A
//!   operators and graphics-object state transitions, while [`fault`] reports ISO 32000-2
//!   departures to a caller-supplied sink.
//! - **Inline images ([`InlineImage`], [`ResCx`]):** Sizes unfiltered inline images via resource
//!   color space component counts ([`ResCx::read`]), decodes Flate and LZW inline images via
//!   `pdf-filters` to PDFium's stop point, and optionally decodes DCT and CCITTFax ends with the
//!   `image-ends` feature ([`Limits::coded_ends`]).
//! - **Multi-stream pages ([`Join`], [`Concat`]):** Joins a page's `/Contents` streams
//!   ([`Join::push_from`], [`Join::push_contents`]) while mapping spans back to individual streams
//!   ([`Pos`], [`SpanAcross`]) and recording undecodable streams ([`Concat::undecoded`]).
//! - **Page analysis ([`balance`](fn@balance), [`marked`](fn@marked), [`names_used`], [`kept`]):**
//!   Analyzes `q`/`Q` balance, marked-content sequences, and referenced resource names.
//! - **Writing & splicing ([`ContentWriter`], [`splice`](fn@splice), [`splice_each`], [`without_keys`]):**
//!   Writes content streams with collision-free deferred resource naming ([`Res`], [`Reserved`]),
//!   and removes byte ranges from existing streams only when downstream graphics state is
//!   unaffected.
//!
//! All passes enforce [`Limits`] up front without panicking, and `Debug` implementations never
//! include raw string or image bytes. `pdf-objects` and `pdf-color` are re-exported at
//! [`pdf_objects`] and [`pdf_color`].
//!
//! # Cargo Features
//!
//! All optional and disabled by default:
//!
//! - **`image-ends`**: Decodes DCT (`zune-jpeg`) and CCITTFax (`hayro-ccitt`) inline images when
//!   requested via [`Limits::coded_ends`].
//! - **`simd`**: Enables runtime SIMD acceleration in `memchr`, `twox-hash`, and `zune-jpeg`.
//! - **`tracing`**: Emits numeric `tracing` spans over page passes ([`balance`],
//!   [`marked`](fn@marked), [`names_used`], [`kept`]), splices ([`splice`](fn@splice),
//!   [`splice_each`], [`without_keys`], [`Spliced::streams`]), [`ContentWriter::named`], and
//!   resource reads ([`ResCx::read`], [`Reserved::read`]).
//!
//! # Example
//!
//! Join two page content streams from a reader and inspect operations, balance, and marked content:
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
pub use res::{Res, ResBody, ResError, ResId, Unheld};
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
