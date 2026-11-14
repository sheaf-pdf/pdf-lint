//! Read and write PDF document-level structures: dates, page trees, name and number trees,
//! `/Info` dictionaries, and XMP conformance packets (ISO 32000-2, §7.7.3, §7.9, §14.3).
//!
//! `pdf-semantics` operates over any [`Resolve`] implementation (`&Reading`, `&Document`, or a
//! closure) without performing file I/O directly. [`catalog`] locates the document catalog from a
//! trailer dictionary, and [`follow`] resolves indirect reference chains.
//!
//! # Core Concepts
//!
//! - **Dates ([`date::Date`]):** Parse and format PDF `D:...` dates, preserving field presence
//!   and recording legacy syntax [`date::Quirk`]s.
//! - **Page trees ([`pages::pages`], [`pages::write`]):** Walk `/Pages` trees under ISO 32000-2
//!   rules or PDFium's rules as of chromium/8125, tracking each inherited attribute's
//!   [`pages::Origin`], or build balanced `/Pages` trees over caller-numbered page IDs.
//! - **Name and number trees ([`tree::names`], [`tree::numbers`]):** Lazily iterate entries in
//!   key order with cycle and depth bounds, or serialize flat trees ([`tree::write_names`],
//!   [`tree::write_numbers`]).
//! - **XMP conformance claims ([`xmp::read`], [`xmp::read_from`], [`xmp::write`]):** Extract and
//!   write namespace-verified PDF/A, PDF/UA, and PDF/VT conformance claims.
//! - **Document `/Info` dictionaries ([`info::read`]):** Decode `/Info` text fields (under ISO or
//!   PDFium rules) and dates.
//!
//! Spec departures are reported as [`Fault`]s to a [`Sink`](pdf_objects::fault::Sink) (skipped
//! when `()` is passed). All walks enforce [`Limits`] without panicking: reader caps and locked
//! files return `Err(Unreadable)` so failed reads never masquerade as empty trees, while corrupt
//! objects emit [`Fault::Damaged`] and allow traversal to continue. `pdf-objects` is re-exported
//! at [`pdf_objects`].
//!
//! # Cargo Features
//!
//! - **`clock`** *(default)*: Enables `Date::now` and `Date::from_unix` via `chrono`.
//! - **`tracing`** *(optional)*: Emits numeric `tracing` spans over page-tree walks/counts
//!   ([`pages::pages`], [`pages::counted`]), page-tree writes ([`pages::write`]), and XMP packet
//!   reads ([`xmp::read`], [`xmp::read_from`]).
//!
//! # Example
//!
//! Build and read back a balanced page tree, round-trip a PDF/A-2b XMP packet, and normalize a date:
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
pub mod info;
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
