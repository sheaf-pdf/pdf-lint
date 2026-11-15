//! Owned PDF syntax objects, byte-span tracking, parser, serializer, text codecs, and resolver
//! traits (ISO 32000-2 §7.3).
//!
//! `pdf-objects` represents PDF values in memory while preserving source byte spans and exact
//! dictionary structure for diagnostics and copy-on-write serialization. It contains no file I/O,
//! stream decompression, or cipher implementations, exposing traits ([`Resolve`], [`Streams`],
//! [`fault::Sink`]) so higher-level crates can share one object model.
//!
//! # Quickstart
//!
//! ```
//! use pdf_objects::text::{self, Encoding, Quirk};
//! use pdf_objects::{Object, Span, Version, dict};
//!
//! // Dictionaries preserve entry order and duplicate keys; `get` returns the last one.
//! let page = dict! {
//!     "Type" => Object::name("Page"),
//!     "Rotate" => 90,
//!     "Rotate" => 180,
//! };
//! assert_eq!(page.get("Rotate").and_then(Object::as_integer), Some(180));
//! let shadowed: Vec<_> = page.duplicates().map(|e| e.value().as_integer()).collect();
//! assert_eq!(shadowed, [Some(90)]);
//!
//! // Attach or inspect byte spans.
//! let span = Span::new(None, 40, 43);
//! let rotate = Object::from(180).with_span(span);
//! assert_eq!(rotate.at().map(|s| (s.stream(), s.start(), s.end())), Some((None, 40, 43)));
//!
//! // Decode and encode ISO 32000-2 text strings (UTF-16BE BOM with a trailing odd byte).
//! let title = text::decode(b"\xFE\xFF\x00H\x00i\x00");
//! assert_eq!((title.text.as_str(), title.encoding), ("Hi", Encoding::Utf16Be));
//! assert_eq!(title.quirks, [Quirk::OddByte]);
//! assert_eq!(text::decode(&text::encode("Hi é")).text, "Hi é");
//!
//! // Parse and compare PDF header versions.
//! assert_eq!(Version::parse(b" 2.0\n"), Some(Version::new(2, 0)));
//! assert!(Version::new(1, 7) < Version::new(2, 0));
//! ```
//!
//! # Modules & Core Types
//!
//! - **Values and spans ([`Object`], [`Value`], [`Dict`], [`Name`], [`Str`], [`Stream`], [`Span`]):**
//!   Compact 64-byte [`Object`]s with inline small-buffer strings/names ([`SmallBytes`]). Mutating
//!   an [`Object`] via [`Object::value_mut`] or `as_*_mut` clears the span on the edited node and
//!   its ancestors, while [`Object::as_written_mut`] preserves spans for in-place transforms such
//!   as string decryption.
//! - **Text string codecs ([`text`], [`text::pdfium`]):** [`text`] implements bidirectional
//!   ISO 32000-2 §7.9.2.2 text string decoding and encoding (`PDFDocEncoding`, `UTF-16BE`, and
//!   `UTF-8` with BOM, including language escape sequences). [`text::pdfium`] reproduces PDFium's
//!   text decoding rules as of chromium/8152.
//! - **Parser and serializer ([`parse`], [`serialize`]):** Stack-safe iterative [`parse::Parser`]
//!   supporting direct values, indirect objects, streams, and positional windows under [`Limits`]
//!   and lexical [`Rules`]; deterministic [`serialize`] functions with on-the-fly
//!   [`serialize::Renumber`] and [`serialize::Encrypt`] hooks.
//! - **Resolver and diagnostic traits ([`Resolve`], [`Streams`], [`fault`]):** Borrowed indirect
//!   object lookup ([`Resolve`], [`ResolveFn`]), bounded decoded stream access ([`Streams`],
//!   [`WithStreams`]), and zero-cost-when-disabled fault sinks ([`fault::Sink`]).

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
#[cfg(all(not(feature = "std"), test))]
extern crate std;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
#[cfg(target_has_atomic = "ptr")]
use alloc::sync::Arc;

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

pub use object::{
    Data, Dict, Entry, Form, Name, Object, ObjectId, SmallBytes, Span, Str, Stream, Value,
};
pub use parse::Limits;
/// Re-export of `pdf-tokenizer` for lexical tokenizing and [`Rules`].
pub use pdf_tokenizer;
pub use pdf_tokenizer::Rules;
pub use streams::{DecodedStream, FilterStep, FilterTail, Streams, WithStreams};
pub use version::Version;

/// Resolves indirect [`ObjectId`] references to borrowed [`Object`]s.
///
/// Missing objects return `None`, which PDF treats as `null` (ISO 32000-2 §7.3.10). The `'o`
/// lifetime belongs to the trait rather than `&self`, allowing borrowed maps (`&'o BTreeMap`,
/// `&'o HashMap`), reader references, smart pointers, and [`resolve_fn`] closures to implement
/// `Resolve<'o>`.
///
/// Use [`Resolve::try_resolve`] when distinguishing an absent object (`Ok(None)`) from an
/// unreadable one (`Err(Unreadable)`).
///
/// ```
/// use std::collections::BTreeMap;
///
/// use pdf_objects::{Object, ObjectId, Resolve, resolve_fn};
///
/// // Borrowed map directly implements `Resolve`.
/// let objects = BTreeMap::from([(ObjectId::new(1, 0), Object::from(7))]);
/// assert_eq!((&objects).resolve(ObjectId::new(1, 0)).and_then(Object::as_integer), Some(7));
/// assert!((&objects).resolve(ObjectId::new(2, 0)).is_none());
///
/// // Wrap a closure with `resolve_fn`.
/// let custom = resolve_fn(|id: ObjectId| objects.get(&id));
/// assert_eq!(custom.resolve(ObjectId::new(1, 0)).and_then(Object::as_integer), Some(7));
///
/// // Implement `Resolve` on a custom container reference.
/// struct Store(BTreeMap<ObjectId, Object>);
/// impl<'o> Resolve<'o> for &'o Store {
///     fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
///         self.0.get(&id)
///     }
/// }
/// let store = Store(objects);
/// assert_eq!((&store).resolve(ObjectId::new(1, 0)).and_then(Object::as_integer), Some(7));
/// ```
pub trait Resolve<'o> {
    /// Looks up `id`, returning `None` if the object is absent or unreadable.
    #[must_use]
    fn resolve(&self, id: ObjectId) -> Option<&'o Object>;

    /// Looks up `id`, returning `Ok(None)` if absent or `Err(Unreadable)` if present in the
    /// source but unreadable.
    ///
    /// Defaults to `Ok(self.resolve(id))` for in-memory maps and [`ResolveFn`] closures.
    ///
    /// # Errors
    ///
    /// Returns [`Unreadable`] when `id` exists but cannot be read (never with [`Why::Filter`] or
    /// [`Why::Detached`], which apply only to [`Streams::decoded`]).
    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        Ok(self.resolve(id))
    }
}

/// Adapts a `Fn(ObjectId) -> Option<&'o Object>` closure or function pointer to [`Resolve`].
///
/// Using an explicit wrapper rather than a blanket `Fn` implementation keeps [`Resolve`] coherent
/// for smart pointers (`Box`, `Arc`), map references (`&BTreeMap`, `&HashMap`), and adapter types
/// (`&WithStreams`). Construct with [`resolve_fn`] or `ResolveFn(f)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ResolveFn<F: ?Sized>(pub F);

/// Wraps a `Fn(ObjectId) -> Option<&'o Object>` closure or function in [`ResolveFn`].
#[must_use]
pub const fn resolve_fn<F>(f: F) -> ResolveFn<F> {
    ResolveFn(f)
}

impl<'o, F: Fn(ObjectId) -> Option<&'o Object> + ?Sized> Resolve<'o> for ResolveFn<F> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        (self.0)(id)
    }
}

impl<'o, F: Fn(ObjectId) -> Option<&'o Object> + ?Sized> Resolve<'o> for &ResolveFn<F> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        (self.0)(id)
    }
}

impl<'o> Resolve<'o> for &(dyn Resolve<'o> + '_) {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        (**self).resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        (**self).try_resolve(id)
    }
}

impl<'o> Resolve<'o> for &(dyn Resolve<'o> + Send + '_) {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        (**self).resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        (**self).try_resolve(id)
    }
}

impl<'o> Resolve<'o> for &(dyn Resolve<'o> + Send + Sync + '_) {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        (**self).resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        (**self).try_resolve(id)
    }
}

impl<'o, R: Resolve<'o> + ?Sized> Resolve<'o> for Box<R> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        (**self).resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        (**self).try_resolve(id)
    }
}

impl<'o, R: Resolve<'o> + ?Sized> Resolve<'o> for &Box<R> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        (***self).resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        (***self).try_resolve(id)
    }
}

#[cfg(target_has_atomic = "ptr")]
impl<'o, R: Resolve<'o> + ?Sized> Resolve<'o> for Arc<R> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        (**self).resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        (**self).try_resolve(id)
    }
}

#[cfg(target_has_atomic = "ptr")]
impl<'o, R: Resolve<'o> + ?Sized> Resolve<'o> for &Arc<R> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        (***self).resolve(id)
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        (***self).try_resolve(id)
    }
}

impl<'o> Resolve<'o> for &'o BTreeMap<ObjectId, Object> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.get(&id)
    }
}

#[cfg(feature = "std")]
impl<'o, S: std::hash::BuildHasher> Resolve<'o>
    for &'o std::collections::HashMap<ObjectId, Object, S>
{
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.get(&id)
    }
}

/// Error returned when an indirect object or stream exists in the source but cannot be read
/// ([`Resolve::try_resolve`], [`Streams::decoded`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
#[error("{message}")]
pub struct Unreadable {
    /// Identifier of the unreadable object or stream.
    pub id: ObjectId,
    /// High-level failure category.
    pub why: Why,
    /// Diagnostic message from the reader.
    pub message: Box<str>,
}

impl Unreadable {
    /// Creates an [`Unreadable`] error for `id` with category `why` and diagnostic `message`.
    #[must_use]
    pub fn new(id: ObjectId, why: Why, message: impl Into<Box<str>>) -> Unreadable {
        Unreadable {
            id,
            why,
            message: message.into(),
        }
    }
}

/// Failure category for [`Unreadable`], letting callers branch on failure reasons without
/// coupling to a specific reader's error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Why {
    /// Syntax, offset, or container corruption prevented reading the object or stream.
    Damaged,
    /// Decoded stream output exceeded the caller's `limit` in [`Streams::decoded`] or
    /// [`Streams::decoded_standard`].
    Limit,
    /// Reading exceeded one of the reader's configured resource caps (nesting depth, value count,
    /// filter chain length, stream budget, or reference hop limit).
    Capped,
    /// A required password was not supplied or decryption failed.
    Locked,
    /// Underlying byte-source I/O failed; retrying may succeed.
    Source,
    /// Unsupported filter or malformed `/Filter` / `/DecodeParms`. Returned only by
    /// [`Streams::decoded`] and [`Streams::decoded_standard`].
    Filter,
    /// Stream bytes are detached from any backing byte source. Returned only by
    /// [`Streams::decoded`] and [`Streams::decoded_standard`].
    Detached,
}

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;
    use alloc::format;
    use alloc::string::ToString;
    use core::hash::{Hash, Hasher};

    use super::*;

    fn resolves<'o, R: Resolve<'o> + ?Sized>(resolve: &R) -> (Option<i64>, bool) {
        let one = resolve
            .resolve(ObjectId::new(1, 0))
            .and_then(Object::as_integer);
        (one, resolve.resolve(ObjectId::new(2, 0)).is_none())
    }

    fn by_value<'o>(resolve: impl Resolve<'o>) -> (Option<i64>, bool) {
        resolves(&resolve)
    }

    fn nothing(_: ObjectId) -> Option<&'static Object> {
        None
    }

    #[test]
    fn resolve_fn_maps_smart_pointers_and_trait_objects() {
        let objects = BTreeMap::from([(ObjectId::new(1, 0), Object::from(7))]);

        // Direct `&BTreeMap` resolution (by reference and by value).
        assert_eq!(resolves(&(&objects)), (Some(7), true));
        assert_eq!(by_value(&objects), (Some(7), true));

        // `resolve_fn` over a closure and function pointer.
        let closure = resolve_fn(|id: ObjectId| objects.get(&id));
        assert_eq!(resolves(&closure), (Some(7), true));
        let closure_ref = &closure;
        assert_eq!(by_value(closure_ref), (Some(7), true));
        assert_eq!(by_value(closure), (Some(7), true));

        const EMPTY: ResolveFn<fn(ObjectId) -> Option<&'static Object>> = resolve_fn(nothing);
        assert_eq!(by_value(EMPTY), (None, true));

        // Boxed and Arc'd resolvers, plus Send + Sync dyn trait objects.
        let boxed: Box<dyn Resolve<'_> + Send + Sync + '_> = Box::new(&objects);
        assert_eq!(resolves(&boxed), (Some(7), true));
        assert_eq!(by_value(&boxed), (Some(7), true));
        assert_eq!(by_value(&*boxed), (Some(7), true));

        let shared: Arc<dyn Resolve<'_> + Send + Sync + '_> = Arc::new(&objects);
        assert_eq!(resolves(&shared), (Some(7), true));
        assert_eq!(by_value(&shared), (Some(7), true));
        assert_eq!(by_value(Arc::clone(&shared)), (Some(7), true));

        let dyn_ref: &(dyn Resolve<'_> + '_) = &&objects;
        assert_eq!(by_value(dyn_ref), (Some(7), true));
        let dyn_send: &(dyn Resolve<'_> + Send + '_) = &&objects;
        assert_eq!(by_value(dyn_send), (Some(7), true));
    }

    #[cfg(feature = "std")]
    #[test]
    fn resolve_hashmap() {
        let objects = std::collections::HashMap::from([(ObjectId::new(1, 0), Object::from(7))]);
        assert_eq!(by_value(&objects), (Some(7), true));
        assert_eq!((&objects).try_resolve(ObjectId::new(2, 0)), Ok(None));
    }

    #[test]
    fn unreadable_preserves_reason_and_hashes() {
        struct Locked;
        impl<'o> Resolve<'o> for Locked {
            fn resolve(&self, _: ObjectId) -> Option<&'o Object> {
                None
            }

            fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
                Err(Unreadable::new(id, Why::Locked, "password required"))
            }
        }

        let err = Locked.try_resolve(ObjectId::new(3, 1)).unwrap_err();
        assert_eq!((err.id, err.why), (ObjectId::new(3, 1), Why::Locked));
        assert_eq!(&*err.message, "password required");
        assert_eq!(err.to_string(), "password required");
        assert!(core::error::Error::source(&err).is_none());

        let boxed: Box<dyn Resolve<'_>> = Box::new(Locked);
        assert_eq!(boxed.try_resolve(ObjectId::new(3, 1)).unwrap_err(), err);

        let mut h1 = std::collections::hash_map::DefaultHasher::new();
        let mut h2 = std::collections::hash_map::DefaultHasher::new();
        err.hash(&mut h1);
        err.clone().hash(&mut h2);
        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn readme_first_code_block_matches_lib_docs() {
        fn first_block<'a>(src: &'a str, prefix: &str, fence: &str) -> &'a str {
            let start_marker = format!("{prefix}{fence}\n");
            let after_start = src
                .split_once(&start_marker)
                .expect("missing opening code fence")
                .1;
            let end_marker = format!("\n{prefix}```");
            after_start
                .split_once(&end_marker)
                .expect("missing closing code fence")
                .0
        }

        let readme_block = first_block(include_str!("../README.md"), "", "```rust");
        let lib_raw = first_block(include_str!("lib.rs"), "//! ", "```");
        let lib_block = lib_raw
            .lines()
            .map(|line| {
                line.strip_prefix("//! ")
                    .or_else(|| line.strip_prefix("//!"))
                    .expect("doc block line missing //! prefix")
            })
            .collect::<alloc::vec::Vec<_>>()
            .join("\n");
        assert_eq!(
            lib_block, readme_block,
            "first code block in src/lib.rs must match first ```rust block in README.md"
        );
    }

    #[test]
    fn readme_license_links_use_repository_urls() {
        let readme = include_str!("../README.md");
        let repository = env!("CARGO_PKG_REPOSITORY");
        for file in ["LICENSE-APACHE", "LICENSE-MIT"] {
            let link = format!("[{file}]({repository}/blob/main/{file})");
            assert!(
                readme.contains(&link),
                "{file}'s link does not match repository URL"
            );
        }
        assert!(
            !readme.contains("](../"),
            "README contains relative parent link"
        );
    }
}
