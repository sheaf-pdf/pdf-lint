//! PDF's objects (ISO 32000-2, 7.3), as a reader read them, to edit and
//! to write: owned, each value with where it was written when the reader
//! knows, and each dictionary as written, in order and with any key
//! written twice.
//!
//! - **Where each value was written:** an [`Object`] is a [`Value`] and
//!   its [`Span`], a run of the file's bytes or of an object stream's
//!   decoded data, so what reads it can point at the bytes it's about.
//! - **Dictionaries as written:** a [`Dict`] keeps its entries in order,
//!   a key written twice and each key's span among them; a lookup takes a
//!   key's last entry, as readers do, and [`Dict::duplicates`] gives the
//!   others.
//! - **Edits that say what they changed:** a value is changed through
//!   [`Object::value_mut`] or an `as_*_mut` accessor, each an edit, which
//!   drops its span and the span of each value that holds it, so a value
//!   that still has a span is still what its bytes say;
//!   [`Object::as_written_mut`] keeps it, for a reader that turns a value
//!   into what the same bytes stand for, as decrypting a string does.
//! - **A stream's data** ([`Data`]): where it was written, or its bytes.
//! - **Strings as their bytes:** a [`Str`] is its bytes, its escapes
//!   decoded; or text a reader decoded without saying from which encoding,
//!   or ciphertext it didn't decrypt, where what depends on the bytes
//!   can't be told. Beside it, a string keeps its [`Form`], literal or
//!   hex, as written.
//! - **Text strings decoded by the spec** ([`text`]): UTF-16BE or UTF-8
//!   after their byte order marks, PDFDocEncoding without one, and where a
//!   string departs from the spec, said.
//! - **Versions** ([`Version`]), read from a header's or a catalog's text,
//!   in order.
//! - **A parser** ([`parse`]), on pdf-tokenizer: a value, or an indirect
//!   object, from a file's bytes, a window of them that says when more of
//!   the file could change what it read ([`parse::Parser::window`]), or an
//!   object stream's decoded data, each value placed; what it reads past
//!   said to a [`parse::Sink`], which `()` drops; a stream's two extents,
//!   by its `/Length` and up to `endstream`, both given, for the caller to
//!   choose its data from; and caps on how deeply values nest and how many
//!   one call reads ([`Limits`]).
//! - **A writer** ([`serialize`]): a value as PDF syntax, or as an
//!   indirect object, which the parser reads back as the same value,
//!   under limits that take in its count of values, and a stream as an
//!   indirect object: a real in its shortest form, names and strings byte
//!   by byte, a dictionary's every entry; and what it can't write as the
//!   value refused, with nothing written.
//! - **References resolved** ([`Resolve`]): what gives the indirect object
//!   a reference names, a reader's or a document in memory, or any closure
//!   from an [`ObjectId`] to an object, so that what follows references
//!   reads through any of them; and walked ([`Object::references`]), a
//!   value's in the order written, with a stack of the walk's own.
//!
//! [`dict!`] builds a dictionary from keys and values.

#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
pub mod object;
pub mod parse;
pub mod serialize;
pub mod text;
pub mod version;

pub use object::{Data, Dict, Entry, Form, Object, ObjectId, Span, Str, Stream, Value};
pub use parse::Limits;
pub use version::Version;

/// What gives the indirect objects a value's references name: a file's,
/// read or in memory. None for an object it hasn't, which a reference to it
/// reads as: null (ISO 32000-2, 7.3.10).
///
/// The objects' lifetime is the trait's, not `&self`'s, so that a closure
/// that gives borrows of its own is one: every `Fn(ObjectId) ->
/// Option<&Object>` is, a `dyn Fn` and a reference or a box of one among
/// them. A type that holds the objects implements it for a reference to
/// itself.
///
/// ```
/// use std::collections::BTreeMap;
///
/// use pdf_objects::{Object, ObjectId, Resolve};
///
/// // A closure over a file's objects.
/// let objects = BTreeMap::from([((1, 0), Object::from(7))]);
/// let resolve = |id: ObjectId| objects.get(&id);
/// assert_eq!(resolve.resolve((1, 0)).and_then(Object::as_integer), Some(7));
/// assert!(resolve.resolve((2, 0)).is_none());
///
/// // A type of its own, by reference.
/// struct File(BTreeMap<ObjectId, Object>);
/// impl<'o> Resolve<'o> for &'o File {
///     fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
///         self.0.get(&id)
///     }
/// }
/// let file = File(objects.clone());
/// assert_eq!((&file).resolve((1, 0)), objects.get(&(1, 0)));
/// ```
pub trait Resolve<'o> {
    /// Object `id`; none when there's none to give: the file hasn't it, or
    /// it doesn't read.
    fn resolve(&self, id: ObjectId) -> Option<&'o Object>;
}

/// A closure, or a function, from an object's number to the object.
impl<'o, F: Fn(ObjectId) -> Option<&'o Object> + ?Sized> Resolve<'o> for F {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self(id)
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
        let one = resolve.resolve((1, 0)).and_then(Object::as_integer);
        (one, resolve.resolve((2, 0)).is_none())
    }

    /// No object at all.
    fn nothing(_: ObjectId) -> Option<&'static Object> {
        None
    }

    #[test]
    fn a_closure_resolves_by_reference_as_a_dyn_fn_and_boxed() {
        let objects = BTreeMap::from([((1, 0), Object::from(7))]);
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
}
