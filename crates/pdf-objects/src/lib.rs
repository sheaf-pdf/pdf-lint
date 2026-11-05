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
//!   [`Object::renumber`] names a value's references anew, an edit of each
//!   it changes, and [`Object::unplace`] drops every place in a value, for
//!   one put among another file's objects.
//! - **A stream's data** ([`Data`]): where it was written, or its bytes.
//! - **Strings as their bytes:** a [`Str`] is its bytes, its escapes
//!   decoded; or text a reader decoded without saying from which encoding,
//!   or ciphertext it didn't decrypt, where what depends on the bytes
//!   can't be told. Beside it, a string keeps its [`Form`], literal or
//!   hex, as written.
//! - **Text strings decoded by the spec, or by PDFium** ([`text`]): UTF-16BE
//!   or UTF-8 after their byte order marks, PDFDocEncoding without one, and
//!   where a string departs from the spec, said. By PDFium's rules, as of
//!   chromium/8125 ([`text::decode_with`]), language escapes come out, and
//!   UTF-16LE's mark and bad UTF-8 read as PDFium reads them.
//! - **Text strings written** ([`text::encode`], [`Object::text`]): as they
//!   are where PDFDocEncoding reads those bytes back so, else UTF-16BE; or
//!   in an encoding the caller chooses, or refused ([`text::encode_as`]).
//! - **Versions** ([`Version`]), read from a header's or a catalog's text,
//!   in order.
//! - **A parser** ([`parse`]), on pdf-tokenizer: a value, or an indirect
//!   object, from a file's bytes, a window of them that says when more of
//!   the file could change what it read ([`parse::Parser::window`]), or an
//!   object stream's decoded data, each value placed; what it reads past
//!   said to a [`parse::Sink`], which `()` drops; a stream's two extents,
//!   by its `/Length` and up to `endstream`, both given, for the caller to
//!   choose its data from; and caps on how deeply values nest and how many
//!   one call reads ([`Limits`]). It reads by ISO 32000-2's lexical rules,
//!   or by PDFium's ([`Rules`], [`parse::Parser::rules`]), each token then
//!   read as PDFium reads it, and a fault said where ISO 32000-2 reads it
//!   otherwise.
//! - **A writer** ([`serialize`]): a value as PDF syntax, or as an
//!   indirect object, which the parser reads back as the same value,
//!   under limits that take in its count of values, and a stream as an
//!   indirect object: a real in its shortest form, names and strings byte
//!   by byte, a dictionary's every entry; and what it can't write as the
//!   value refused, with nothing written. An encrypted file's indirect
//!   objects are written through an encryptor ([`serialize::Encrypt`]),
//!   which every string and stream's data passes through as it's written,
//!   given the object's number and generation, but what it leaves in the
//!   clear; `()` encrypts nothing. Through a renumbering
//!   ([`serialize::Renumber`]), each reference is written as it names it,
//!   for a file whose objects are written at numbers other than those
//!   they're held at, the value unchanged; `()` names each as it is.
//! - **References resolved** ([`Resolve`]): what gives the indirect object
//!   a reference names, a reader's or a document in memory, or any closure
//!   from an [`ObjectId`] to an object, so that what follows references
//!   reads through any of them, and why it gives none where it can say
//!   ([`Resolve::try_resolve`], [`Unreadable`]); and walked
//!   ([`Object::references`]), a value's in the order written, with a stack
//!   of the walk's own.
//! - **A stream's data, decoded** ([`Streams`]): what gives it, decrypted
//!   and decoded through its filters, as a viewer takes it, a reader's or
//!   a document's, at most a limit the caller asks for, lent where the
//!   reader holds it, and why it gives none, in the same words as a
//!   resolver's ([`Why`]); and [`With`], a resolver and a function as one,
//!   for what has its streams some other way.
//! - **Faults, told with where they are** ([`fault`]): one
//!   [`fault::Sink`] and one [`fault::Place`] for every reader of a file's
//!   objects, each reader's faults a type of its own, so that one sink can
//!   take several readers' and `()` drops them and wants none, so none is
//!   looked for. Three keep a sink of their own, as what they tell isn't
//!   placed by an object, each saying why: the parser's
//!   ([`parse::Sink`]), a content stream's reader's, and a linearized
//!   file's check's.
//!
//! [`dict!`] builds a dictionary from keys and values.

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
pub use pdf_tokenizer::Rules;
pub use streams::{Streams, With};
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
/// let objects = BTreeMap::from([(ObjectId::new(1, 0), Object::from(7))]);
/// let resolve = |id: ObjectId| objects.get(&id);
/// assert_eq!(resolve.resolve(ObjectId::new(1, 0)).and_then(Object::as_integer), Some(7));
/// assert!(resolve.resolve(ObjectId::new(2, 0)).is_none());
///
/// // A type of its own, by reference.
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
/// [`Resolve::resolve`] doesn't say why it gives none, and
/// [`Resolve::try_resolve`] does, where the resolver knows: a reader's and a
/// document's do. A closure can't say, so one over a reader loses the reason
/// and the reader's own should be given where the reason matters.
pub trait Resolve<'o> {
    /// Object `id`; none when there's none to give: the file hasn't it, or
    /// it doesn't read.
    fn resolve(&self, id: ObjectId) -> Option<&'o Object>;

    /// Object `id`; none when the file hasn't it; or why it doesn't read.
    /// By default what [`Resolve::resolve`] gives, for a resolver that can't
    /// say why.
    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        Ok(self.resolve(id))
    }
}

/// Why a resolver gives no object for a number the file has
/// ([`Resolve::try_resolve`]), or what gives a stream's data none of it
/// ([`Streams::decoded`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
#[error("{message}")]
pub struct Unreadable {
    /// The object.
    pub id: ObjectId,
    /// What kind of failure it is.
    pub why: Why,
    /// What the reader said of it, the whole message.
    pub message: String,
}

impl Unreadable {
    /// Object `id` doesn't read, for `why`, as `message` says.
    pub fn new(id: ObjectId, why: Why, message: impl Into<String>) -> Unreadable {
        Unreadable {
            id,
            why,
            message: message.into(),
        }
    }
}

/// What kind of failure an [`Unreadable`] is, so that a caller can decide
/// what to do without knowing a reader's own errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Why {
    /// What's written there isn't that object: it doesn't parse, or the file
    /// doesn't lead to it, or doesn't hold it as it says.
    Damaged,
    /// It's past a cap the reader reads under.
    Limit,
    /// It takes a password the reader wasn't given, or doesn't decrypt.
    Locked,
    /// The source of the file's bytes didn't give them: it may read if it's
    /// asked again. Or the reader has no source of bytes at all, as a
    /// document read whole hasn't, which holds the file's objects and not
    /// its streams' data: then it's read beside what read it.
    Source,
    /// It's a stream whose filters the reader doesn't decode: through a
    /// filter it doesn't take, as an image's data may be (ISO 32000-2,
    /// 7.4.1), its data there as written for a decoder that takes that
    /// filter; or with a `/Filter` or `/DecodeParms` it can't take as
    /// written, which is the file's to fix. Only what gives a stream's data
    /// says it ([`Streams::decoded`]); a resolver never does.
    Filter,
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
}
