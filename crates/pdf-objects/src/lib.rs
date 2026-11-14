//! Owned PDF syntax objects, parser, serializer, and cross-crate traits (ISO 32000-2, §7.3).
//!
//! `pdf-objects` represents PDF values in memory while preserving byte spans and exact
//! dictionary structure for diagnostics and lossless round-tripping, alongside traits
//! (`Resolve`, `Streams`, `fault::Sink`) shared across higher-level crates.
//!
//! # Core Concepts
//!
//! - **Span-aware values ([`Object`], [`Value`], [`Span`]):** Every parsed [`Object`] pairs a
//!   [`Value`] with an optional [`Span`] pointing to its source bytes in the file or inside a
//!   decoded object stream. Mutating a value via [`Object::value_mut`] or `as_*_mut` clears the
//!   span on the modified value and its containers, whereas [`Object::as_written_mut`] updates
//!   in-place representations (such as decrypting a string) while preserving its span.
//!   [`Object::renumber`] rewrites indirect references, and [`Object::unplace`] strips all spans
//!   when moving an object between files.
//! - **Exact dictionaries ([`Dict`]):** Preserves entry order, key spans, and duplicate keys.
//!   Standard lookups return the last occurrence of a key (matching PDF viewers), while
//!   [`Dict::duplicates`] exposes shadowed entries for linting. Use [`dict!`] to construct
//!   dictionaries ergonomically.
//! - **Strings and text ([`Str`], [`Form`], [`text`]):** [`Str`] stores raw decoded bytes (plus
//!   whether the literal or hex [`Form`] was used) or marks strings whose encoding/encryption
//!   state is unknown. [`text`] decodes and encodes PDF text strings (PDFDocEncoding, UTF-16BE,
//!   and UTF-8 with BOM) under either ISO 32000-2 rules or PDFium's rules as of chromium/8152
//!   ([`text::decode_with`], [`text::encode`], [`text::encode_as`], [`Object::text`]).
//! - **Streams ([`Stream`], [`Data`]):** Stores a stream's dictionary alongside either a byte
//!   range in the source file (`Data::At`) or owned in-memory bytes (`Data::Bytes`).
//! - **Parsing and serialization ([`parse`], [`serialize`]):**
//!   - [`parse`] parses direct objects, indirect objects, and windowed byte slices
//!     ([`parse::Parser::window`]) under [`Limits`] and either ISO 32000-2 or PDFium lexical
//!     [`Rules`](pdf_tokenizer::Rules) ([`parse::Parser::rules`]), reporting syntax anomalies to
//!     a [`parse::Sink`].
//!   - [`serialize`] formats direct and indirect objects back into canonical PDF syntax, with
//!     pluggable on-the-fly encryption ([`serialize::Encrypt`]) and reference renumbering
//!     ([`serialize::Renumber`]).
//! - **Cross-crate traits ([`Resolve`], [`Streams`], [`fault`]):**
//!   - [`Resolve`] resolves an [`ObjectId`] to `&Object` (implemented for file readers,
//!     in-memory documents, and `Fn(ObjectId) -> Option<&Object>` closures), with
//!     [`Resolve::try_resolve`] distinguishing missing objects (`Ok(None)`) from unreadable ones
//!     (`Err(Unreadable)`).
//!   - [`Streams`] decodes and decrypts stream data up to a caller-supplied byte limit
//!     ([`WithStreams`] pairs a resolver with a stream-decoding closure).
//!   - [`fault::Sink`] and [`fault::Place`] provide zero-cost when-disabled (`()`) diagnostic
//!     reporting across domain crates.

pub mod fault;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
pub mod object;
pub mod parse;
pub mod serialize;
pub mod streams;
pub mod text;
pub mod version;

pub use object::{Data, Dict, Entry, Form, Object, ObjectId, Span, Str, Stream, Value};
pub use parse::Limits;
/// Re-export of `pdf-tokenizer` for lexical [`Rules`](pdf_tokenizer::Rules) selection.
pub use pdf_tokenizer;
pub use streams::{Streams, WithStreams};
pub use version::Version;

/// Resolves indirect [`ObjectId`] references to borrowed [`Object`]s.
///
/// Missing objects return `None`, which PDF treats as the `null` object (ISO 32000-2, §7.3.10).
/// The returned lifetime `'o` is tied to the trait rather than `&self`, allowing any closure
/// `Fn(ObjectId) -> Option<&'o Object>` as well as reader/document references to implement
/// `Resolve<'o>`.
///
/// ```
/// use std::collections::BTreeMap;
///
/// use pdf_objects::{Object, ObjectId, Resolve};
///
/// // A closure over a map of objects.
/// let objects = BTreeMap::from([(ObjectId::new(1, 0), Object::from(7))]);
/// let resolve = |id: ObjectId| objects.get(&id);
/// assert_eq!(resolve.resolve(ObjectId::new(1, 0)).and_then(Object::as_integer), Some(7));
/// assert!(resolve.resolve(ObjectId::new(2, 0)).is_none());
///
/// // A custom container implementing Resolve by reference.
/// struct File(BTreeMap<ObjectId, Object>);
/// impl<'o> Resolve<'o> for &'o File {
///     fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
///         self.0.get(&id)
///     }
/// }
/// let file = File(objects.clone());
/// assert_eq!((&file).resolve(ObjectId::new(1, 0)), objects.get(&ObjectId::new(1, 0)));
/// ```
///
/// Use [`Resolve::try_resolve`] when you need to distinguish an absent object (`Ok(None)`) from
/// an unreadable object (`Err(Unreadable)`). Note that wrapping a reader in a closure erases
/// error reasons; pass `&reading` or `&document` directly where failure reasons matter.
pub trait Resolve<'o> {
    /// Looks up `id`, returning `None` if the object is absent or unreadable.
    #[must_use]
    fn resolve(&self, id: ObjectId) -> Option<&'o Object>;

    /// Looks up `id`, returning `Ok(None)` if absent or `Err(Unreadable)` if present but
    /// unreadable.
    ///
    /// Defaults to `Ok(self.resolve(id))` for resolvers (such as closures) that do not track
    /// failure reasons.
    ///
    /// # Errors
    ///
    /// Returns [`Unreadable`] when `id` exists in the source but cannot be read (never with
    /// [`Why::Filter`] or [`Why::Detached`], which apply only to [`Streams::decoded`]).
    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        Ok(self.resolve(id))
    }
}

/// Error returned when an indirect object or stream exists in the file but cannot be read
/// ([`Resolve::try_resolve`], [`Streams::decoded`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
#[error("{message}")]
pub struct Unreadable {
    /// Identifier of the unreadable object or stream.
    pub id: ObjectId,
    /// High-level failure category.
    pub why: Why,
    /// Human-readable diagnostic message from the underlying reader.
    pub message: String,
}

impl Unreadable {
    /// Creates an [`Unreadable`] error for `id` with category `why` and diagnostic `message`.
    #[must_use]
    pub fn new(id: ObjectId, why: Why, message: impl Into<String>) -> Unreadable {
        Unreadable {
            id,
            why,
            message: message.into(),
        }
    }
}

/// Failure category for [`Unreadable`], allowing callers to handle errors without depending on
/// a specific reader's error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Why {
    /// Syntax, offset, or container corruption prevented reading the object or stream.
    Damaged,
    /// Decoded stream output exceeded the caller-requested `limit` in [`Streams::decoded`].
    Limit,
    /// Reading exceeded one of the reader's configured resource caps (nesting depth, object
    /// value count, filter chain length, stream budget, or reference hop limit).
    Capped,
    /// The file requires a password that was not supplied, or decryption failed.
    Locked,
    /// Underlying byte-source I/O failed; retrying may succeed.
    Source,
    /// The stream uses an unsupported filter (such as image-specific compression) or malformed
    /// `/Filter` / `/DecodeParms` entries. Returned only by [`Streams::decoded`].
    Filter,
    /// The stream's raw bytes are not held in memory (for example, on a standalone `Document`
    /// after dropping its `Reading`). Returned only by [`Streams::decoded`].
    Detached,
}

/// A closure, or a function, from an object's number to the object.
impl<'o, F: Fn(ObjectId) -> Option<&'o Object> + ?Sized> Resolve<'o> for F {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self(id)
    }
}

/// A reference to any resolver, for what takes one by value, as a walk does.
impl<'o> Resolve<'o> for &(dyn Resolve<'o> + '_) {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        (**self).resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        (**self).try_resolve(id)
    }
}

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// A boxed closure over `objects`. Written in a `let`, its type would
    /// need the objects' lifetime, which only a signature can name.
    fn boxed<'o>(
        objects: &'o BTreeMap<ObjectId, Object>,
    ) -> Box<dyn Fn(ObjectId) -> Option<&'o Object> + 'o> {
        Box::new(|id| objects.get(&id))
    }

    /// Object 1 as `resolve` gives it, and whether it gives none for 2.
    fn resolves<'o, R: Resolve<'o> + ?Sized>(resolve: &R) -> (Option<i64>, bool) {
        let one = resolve
            .resolve(ObjectId::new(1, 0))
            .and_then(Object::as_integer);
        (one, resolve.resolve(ObjectId::new(2, 0)).is_none())
    }

    /// No object at all.
    fn nothing(_: ObjectId) -> Option<&'static Object> {
        None
    }

    #[test]
    fn a_closure_resolves_by_reference_as_a_dyn_fn_and_boxed() {
        let objects = BTreeMap::from([(ObjectId::new(1, 0), Object::from(7))]);
        let closure = |id: ObjectId| objects.get(&id);
        assert_eq!(resolves(&closure), (Some(7), true));
        assert_eq!(resolves(&&closure), (Some(7), true));
        let dynamic = &closure as &dyn Fn(_) -> _;
        assert_eq!(resolves(dynamic), (Some(7), true));
        assert_eq!(resolves(&dynamic), (Some(7), true));
        let boxed = boxed(&objects);
        assert_eq!(resolves(&boxed), (Some(7), true));
        assert_eq!(resolves(&*boxed), (Some(7), true));
        assert_eq!(resolves(&nothing), (None, true));
    }

    /// A resolver that can't say why gives what it gives, by reference, as
    /// a `dyn Fn` and as a `dyn Resolve`.
    #[test]
    fn a_closure_can_t_say_why() {
        let objects = BTreeMap::from([(ObjectId::new(1, 0), Object::from(7))]);
        let closure = |id: ObjectId| objects.get(&id);
        assert_eq!(
            closure.try_resolve(ObjectId::new(1, 0)),
            Ok(objects.get(&ObjectId::new(1, 0)))
        );
        assert_eq!(closure.try_resolve(ObjectId::new(2, 0)), Ok(None));
        let dynamic = &closure as &dyn Fn(_) -> _;
        assert_eq!(dynamic.try_resolve(ObjectId::new(2, 0)), Ok(None));
        let resolver = &closure as &dyn Resolve;
        assert_eq!(
            resolver.try_resolve(ObjectId::new(1, 0)),
            Ok(objects.get(&ObjectId::new(1, 0)))
        );
    }

    /// One that can says so, and what it says is the whole message.
    #[test]
    fn a_resolver_says_why_it_gives_none() {
        struct Locked;
        impl<'o> Resolve<'o> for Locked {
            fn resolve(&self, _: ObjectId) -> Option<&'o Object> {
                None
            }
            fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
                Err(Unreadable::new(id, Why::Locked, "it wants a password"))
            }
        }
        let why = Locked.try_resolve(ObjectId::new(3, 1)).unwrap_err();
        assert_eq!((why.id, why.why), (ObjectId::new(3, 1), Why::Locked));
        assert_eq!(why.to_string(), "it wants a password");
        assert!(std::error::Error::source(&why).is_none());

        // Through a `dyn Resolve`, by value, it says so still.
        fn by_value<'o>(resolver: impl Resolve<'o>) -> Result<Option<&'o Object>, Unreadable> {
            resolver.try_resolve(ObjectId::new(3, 1))
        }
        let locked = &Locked as &dyn Resolve;
        assert_eq!(by_value(locked).unwrap_err().why, Why::Locked);
    }

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
