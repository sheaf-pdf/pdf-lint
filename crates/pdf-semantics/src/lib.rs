//! Read PDF's dates, name and number trees, page tree and XMP claims (ISO
//! 32000-2, 7.7.3, 7.9 and 14.3.2), and write each of them. No file is read
//! here. A walk reads a file's objects through the caller's [`Resolve`]:
//! pdf-structure's reading or document, or a closure over a map.
//!
//! - **Read and write a date** ([`date::Date`]). Each field says whether it
//!   was written, and what the reading read past is a [`date::Quirk`].
//! - **List a file's pages** ([`pages::pages`]) by ISO's rules or PDFium's,
//!   as of chromium/8125. Each inherited attribute says where it came from
//!   ([`pages::Origin`]).
//! - **Write a page tree** ([`pages::write`]), balanced, over a caller's
//!   pages, its nodes numbered by the caller.
//! - **Walk name and number trees** ([`tree::names`], [`tree::numbers`]) a
//!   key and a value at a time, and write one flat ([`tree::write_names`],
//!   [`tree::write_numbers`]).
//! - **Read what a file claims to conform to** ([`xmp::read`],
//!   [`xmp::read_from`]): PDF/A, PDF/UA and PDF/VT, found by namespace.
//! - **Write a packet of claims** ([`xmp::write`]) that reads back as they
//!   are.
//!
//! Where a file departs from the standard is a [`Fault`], told with its
//! place to pdf-objects' [`Sink`](pdf_objects::fault::Sink). With `()` as
//! the sink, none is looked for. Each walk is bounded by [`Limits`], and it
//! never panics on input. An object past the reader's own cap or behind a
//! password stops a walk with an [`Unreadable`], so a failed read never looks
//! like an empty tree. A damaged one is a [`Fault::Damaged`], and the walk
//! goes on past it. [`catalog`] finds where a walk starts, and [`follow`]
//! follows a reference. pdf-objects is re-exported whole, so its types are
//! named through it at the version this crate is built with.
//!
//! # Features
//!
//! - **`clock`**, on by default: `Date::now` and `Date::from_unix`, on
//!   chrono.
//! - **`tracing`**, off by default: a span at `debug` over a page tree
//!   walked or counted ([`pages::pages`], [`pages::counted`]), a packet read
//!   ([`xmp::read`], [`xmp::read_from`]) and a page tree written
//!   ([`pages::write`]). A span's fields are numbers, never a byte of a
//!   file. A name or number tree's walk has none, as its loop is the
//!   caller's.
//!
//! Three pages under a tree written for them, read back; a packet of claims,
//! written and read back; and a date:
//!
//! ```
//! use std::collections::BTreeMap;
//!
//! use pdf_semantics::date::Date;
//! use pdf_semantics::pages::{self, Origin, Rect};
//! use pdf_semantics::pdf_objects::pdf_tokenizer::Rules;
//! use pdf_semantics::pdf_objects::{Object, ObjectId, dict};
//! use pdf_semantics::{Limits, xmp};
//!
//! // Three pages, objects 1 to 3, under a tree whose nodes are numbered from 10.
//! let numbered = [ObjectId::new(1, 0), ObjectId::new(2, 0), ObjectId::new(3, 0)];
//! let mut next = 10;
//! let ids = || {
//!     next += 1;
//!     ObjectId::new(next - 1, 0)
//! };
//! let written = pages::write(&numbered, 2, &Limits::DEFAULT, ids).unwrap();
//!
//! // The nodes, a box on the root for every page, and each page under its node.
//! let media = vec![Object::from(0), Object::from(0), Object::from(612), Object::from(792)];
//! let mut objects = BTreeMap::new();
//! for (id, mut node) in written.nodes {
//!     if id == written.root {
//!         node.set("MediaBox", media.clone());
//!     }
//!     objects.insert(id, Object::from(node));
//! }
//! for (&page, &parent) in numbered.iter().zip(&written.parents) {
//!     objects.insert(page, Object::from(dict! { "Type" => "Page", "Parent" => parent }));
//! }
//!
//! // Read back by ISO's rules with nothing told, the last page's box its root's.
//! let resolve = |id: ObjectId| objects.get(&id);
//! let catalog = dict! { "Pages" => written.root };
//! let mut faults = Vec::new();
//! let table = pages::pages(resolve, &catalog, Rules::Iso, &Limits::DEFAULT, &mut faults).unwrap();
//! assert!(faults.is_empty());
//! let last = table.get(2).unwrap();
//! assert_eq!(last.id, Some(ObjectId::new(3, 0)));
//! assert_eq!(last.media_box().value, Rect([0.0, 0.0, 612.0, 792.0]));
//! assert_eq!(last.media_box().origin, Origin::Ancestor(Some(written.root)));
//! assert_eq!(last.parents().collect::<Vec<_>>(), [ObjectId::new(12, 0), written.root]);
//!
//! // A packet claiming PDF/A-2b, written and read back.
//! let mut claims = xmp::Claims::default();
//! claims.pdfa_part = Some("2".into());
//! claims.pdfa_conformance = Some("B".into());
//! let packet = xmp::write(&claims).unwrap();
//! let read = xmp::read(packet.as_bytes(), None, &Limits::DEFAULT, &mut ()).unwrap();
//! assert_eq!(read.conformance().unwrap().pdfa.unwrap().to_string(), "PDF/A-2b");
//!
//! // A date: the apostrophe PDF 1.7 wrote last is read past, and not written.
//! let date = Date::read("D:20261111093000-08'00'").unwrap();
//! assert_eq!((date.year(), date.month()), (2026, Some(11)));
//! assert_eq!(date.write(), "D:20261111093000-08'00");
//! ```

pub use pdf_objects;

pub use fault::Fault;
pub use limits::Limits;

use pdf_objects::{Dict, Object, ObjectId, Resolve, Unreadable, Value, Why};

pub mod date;
mod fault;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod limits;
pub mod pages;
mod phases;
pub mod tree;
pub mod xmp;

/// How many references a value may go through to its object.
const HOPS: usize = 32;

/// `value` with its references followed: none for null, or for a reference
/// to an object the file hasn't, which is null (ISO 32000-2, 7.3.10).
///
/// # Errors
///
/// The resolver's [`Unreadable`] for an object that doesn't read, and one of
/// [`Why::Capped`] for a chain of 32 references or more, which a loop of them
/// is.
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
/// (`document.trailer().dict()`), and this finds where their walks start.
///
/// Over a file, a walk takes the reading or the document by reference, which
/// says why it gives no object where a closure can't:
///
/// ```
/// use pdf_semantics::pages::pages;
/// use pdf_semantics::pdf_objects::ObjectId;
/// use pdf_semantics::pdf_objects::pdf_tokenizer::Rules;
/// use pdf_semantics::{Limits, catalog, xmp};
/// use pdf_structure::read::{Options, Reading};
///
/// let file = b"%PDF-1.7\n\
///     1 0 obj\n<< /Pages 2 0 R >>\nendobj\n\
///     2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
///     3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n\
///     trailer\n<< /Root 1 0 R >>\n";
/// let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
/// let root = catalog(&reading, reading.trailer().dict()).unwrap().unwrap();
/// let table = pages(&reading, root, Rules::Iso, &Limits::DEFAULT, &mut ()).unwrap();
/// assert_eq!(table.get(0).unwrap().id, Some(ObjectId::new(3, 0)));
/// assert_eq!(xmp::metadata_stream(&reading, root), Ok(None));
///
/// // The file read whole, as a document, walks the same.
/// let document = reading.to_document().unwrap();
/// let root = catalog(&document, document.trailer().dict()).unwrap().unwrap();
/// let table = pages(&document, root, Rules::Iso, &Limits::DEFAULT, &mut ()).unwrap();
/// assert_eq!(table.get(0).unwrap().id, Some(ObjectId::new(3, 0)));
/// ```
///
/// # Errors
///
/// As [`follow`] fails for `/Root`.
pub fn catalog<'o, R: Resolve<'o>>(
    objects: R,
    trailer: &'o Dict,
) -> Result<Option<&'o Dict>, Unreadable> {
    let Some(root) = trailer.get(b"Root") else {
        return Ok(None);
    };
    Ok(follow(objects, root)?.and_then(Object::as_dict))
}

#[cfg(test)]
mod tests {
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

    /// The crate doc's example is the README's first, line for line, so that
    /// what docs.rs shows and what crates.io shows are one example, and each
    /// runs as a doctest, with no feature.
    #[test]
    fn the_crate_docs_example_is_the_readmes_first() {
        let readme: Vec<&str> = (include_str!("../README.md").lines())
            .skip_while(|line| *line != "```rust")
            .skip(1)
            .take_while(|line| *line != "```")
            .collect();
        let doc: Vec<&str> = (include_str!("lib.rs").lines())
            .map_while(|line| line.strip_prefix("//!"))
            .map(|line| line.strip_prefix(' ').unwrap_or(line))
            .skip_while(|line| *line != "```")
            .skip(1)
            .take_while(|line| *line != "```")
            .collect();
        assert!(readme.len() > 10, "the README's first example: {readme:?}");
        assert_eq!(doc, readme);
    }
}

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
