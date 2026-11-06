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
//! - **Name and number trees** ([`tree`]): ISO 32000-2, 7.9.6 and 7.9.7.
//!   Walked a key and a value at a time under caps, a loop or a tree too
//!   deep said, and written.
//! - **The page tree** ([`pages`]): ISO 32000-2, 7.7.3. Every page in order,
//!   by ISO's rules or PDFium's as of chromium/8125, each inherited attribute
//!   saying which dictionary it came from, a value that's absent saying what
//!   stands for it.
//! - **XMP claims** ([`xmp`]): ISO 32000-2, 14.3.2. What a packet says a file
//!   conforms to, PDF/A, PDF/UA or PDF/VT, read as XML under caps, by the
//!   namespace each is bound to, so a prefix or a comment that merely looks
//!   like one claims nothing.
//!
//! What walks a file's objects reads them through [`Resolve`], given by value,
//! so `&reading` over pdf-structure's reading, lazily or by position, `&document`
//! over its document, or a closure alike: [`catalog`] finds where a walk starts
//! from a trailer and [`follow`] follows a reference. A reader's own resolver
//! says why it gives no object ([`Resolve::try_resolve`]), and a closure can't,
//! so it reads as a reference to nothing. What a walk reads past is a [`Fault`],
//! told to pdf-objects' [`Sink`](pdf_objects::fault::Sink) with its
//! [`Place`](pdf_objects::fault::Place), or dropped by `()`; its caps are one
//! value, [`Limits`]. pdf-objects is re-exported whole, so a sink of one's own
//! names the trait at the version this crate takes.

pub use pdf_objects;
pub use pdf_objects::pdf_tokenizer::Rules;
pub use pdf_objects::{
    Dict, Object, ObjectId, Resolve, Span, Str, Stream, Unreadable, Value, Why, dict, text,
};

pub use fault::Fault;
pub use limits::Limits;

pub mod date;
mod fault;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod limits;
pub mod pages;
pub mod tree;
pub mod xmp;

/// How many references a value may go through to its object.
const HOPS: usize = 32;

/// `value` with its references followed: none for null, or for a reference
/// to an object the file hasn't, which is null (ISO 32000-2, 7.3.10). An
/// object that doesn't read gives the resolver's [`Unreadable`], and a chain
/// of 32 references or more, which a loop of them is, one of [`Why::Capped`].
pub fn follow<'o, R: Resolve<'o>>(
    objects: R,
    value: &'o Object,
) -> Result<Option<&'o Object>, Unreadable> {
    followed(&objects, value)
}

/// [`follow`], through a resolver a walk holds.
fn followed<'o, R: Resolve<'o>>(
    objects: &R,
    value: &'o Object,
) -> Result<Option<&'o Object>, Unreadable> {
    let (mut at, mut last) = (value, ObjectId::new(0, 0));
    for _ in 0..HOPS {
        match at.value() {
            Value::Ref(id) => {
                last = *id;
                match objects.try_resolve(*id)? {
                    Some(object) => at = object,
                    None => return Ok(None),
                }
            }
            Value::Null => return Ok(None),
            _ => return Ok(Some(at)),
        }
    }
    Err(too_long(last))
}

/// The error of a chain of references that ends at `last` after too many.
fn too_long(last: ObjectId) -> Unreadable {
    let message = format!("{last} R is past {HOPS} references, or in a loop of them");
    Unreadable::new(last, Why::Capped, message)
}

/// The catalog a trailer's `/Root` leads to (ISO 32000-2, 7.5.5, 7.7.2):
/// none where there's no `/Root`, or it isn't a dictionary. A reading has its
/// trailer (`reading.trailer().dict()`), and a document too
/// (`document.trailer.dict()`), and this finds where their walks start.
pub fn catalog<'o, R: Resolve<'o>>(
    objects: R,
    trailer: &'o Dict,
) -> Result<Option<&'o Dict>, Unreadable> {
    let Some(root) = trailer.get(b"Root") else {
        return Ok(None);
    };
    Ok(follow(objects, root)?.and_then(Object::as_dict))
}

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
