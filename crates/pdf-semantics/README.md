# pdf-semantics

PDF's common structures (ISO 32000-2, 7.7.3, 7.9 and 14.3.2), read and written, each value saying where it came from, in a small Rust crate on [pdf-objects](https://crates.io/crates/pdf-objects).

It's made for what reads a file's common structures to show them, check them or edit them, and what writes them.

## What it does

- **Dates (7.9.4).** `Date::read` reads one, each field saying whether it was written, what stands for it if not by the clause's defaults, and what the reading read past (`Quirk`): the 1.7 form's last apostrophe, hours without a sign, an offset before the fields it follows, a lone digit, what follows the date, a day its month hasn't. A field out of its range makes it no date. `Date::write` writes the clause's form of the fields, never the bytes read, and `Date::new` and the `with_` methods build a date, each field in order and in range.
- **Text strings (7.9.2.2).** `text` is pdf-objects' module: read by ISO's rules or PDFium's, every departure said, and written, as itself when PDFDocEncoding reads it back so, or in UTF-16BE.
- **The page tree (7.7.3).** `pages::pages` lists every page in order, by one of two sets of rules, which give the same pages on a well-formed tree. PDFium's, as of chromium/8125, a development snapshot of PDFium that the crate's tests hold them to, number each page as PDFium numbers it: a kid with `/Kids` is a node whatever its `/Type`, one written in place reads as any other, one that names no dictionary has a place with no page at it, and the walk stops at the count PDFium gives, which `Pages::counted` says, and `pages::counted` where no page is under the root, which the walk makes `pages::Error::NoPages`; where PDFium goes on past a node listed inside itself, a `/Parent` chain that loops or a tree past the caps, the walk fails instead, by a typed `pages::Error`. ISO's read on past whatever departs from the clause, each said. Either way what a page inherits comes up its `/Parent` chain: `Page::attribute` gives it as written and followed, `Page::media_box`, `crop_box`, `rotate` and `user_unit` as a value, and each says where it came from, the page, an ancestor or a default (`Origin`). A page object listed twice is two pages, and nothing's copied: what a page holds is borrowed from what the resolver keeps, its `/Contents` and `/Annots` the caller's to read from its dictionary.
- **Name and number trees (7.9.6, 7.9.7).** `tree::names` and `tree::numbers` walk one a key and a value at a time, in the order it's written, borrowing from what the resolver keeps and reading nothing it hasn't been asked for, under caps of depth and nodes, each node read once so a loop ends. Each departure is told to a sink, and `whole` says whether anything was left unread. `tree::write_names` and `tree::write_numbers` write one flat, sorted, a key given twice refused.
- **XMP claims (14.3.2).** `xmp::read` reads what a packet claims it conforms to, PDF/A's part, conformance and revision, PDF/UA's part and revision and PDF/VT's version, on [quick-xml](https://crates.io/crates/quick-xml): an attribute or an element, under any prefix bound to its namespace, so a prefix bound to another, a comment or CDATA holding what looks like one claims nothing. The first value of each counts, as written, so one that says nothing doesn't lose another, and `Claims::conformance` reads PDF/A's and PDF/UA's as a number and a letter. An attribute or a value that doesn't read is told to a sink and passed; XML that stops making sense, or a packet past its caps, ends the read with an `xmp::Error` that holds what was read. No DOCTYPE is read and no entity expanded. `xmp::metadata_stream` says which stream holds a file's packet.

Whatever walks a file's objects reads them through pdf-objects' `Resolve`, given by value, so `&reading` over pdf-structure's reading in memory or by position, `&document` over its document, or a closure alike. `catalog` finds a file's catalog from its trailer, and `follow` follows a reference. A reader says why it gives no object, and a walk acts on it: an object that's damaged is said and passed; one past a cap the reader reads under, one that takes a password, or bytes the source didn't give end the walk with an `Unreadable`, so that a failed read never looks like an empty tree.

Where a file departs from the standard is a `Fault`, told to a `Sink` with its `Place`. `()` drops them and wants none, so what only a checker wants isn't looked for, and what's read is the same whatever the sink. Caps are one value, `Limits`.

## The `clock` feature

On by default. With it, `Date::now` and `Date::from_unix` give a date from the clock or a count of seconds, on [chrono](https://crates.io/crates/chrono) without its default features, so without its time zones. A crate that only reads and writes dates, or walks trees, turns it off with `default-features = false`, and chrono isn't built.

## An example

```rust
use pdf_semantics::date::{Date, Offset, Quirk};

let date = Date::read("D:20261024153000-08'00'").unwrap();
assert_eq!(date.month(), Some(10));
assert_eq!(date.offset(), Some(Offset::Behind { hours: 8, minutes: 0 }));

// The apostrophe PDF up to 1.7 wrote last, read past and said.
assert_eq!(date.quirks(), [Quirk::TerminatingApostrophe]);
assert_eq!(date.write(), "D:20261024153000-08'00");

// A field not written is none, and what stands for it the clause's default.
let date = Date::read("D:2026").unwrap();
assert_eq!((date.month(), date.month_or_default()), (None, 1));

// Built, each field after the one before it.
let date = Date::new(2026).unwrap().with_month(10).unwrap().with_day(24).unwrap();
assert_eq!(date.write(), "D:20261024");
assert!(Date::new(2026).unwrap().with_day(24).is_err());
```

A file's pages, each value saying where it came from:

```rust
use std::collections::BTreeMap;

use pdf_semantics::pages::{Inheritable, Origin, Rect, pages};
use pdf_semantics::{Limits, Object, ObjectId, Rules, dict};

let objects = BTreeMap::from([
    ((2, 0), Object::from(dict! {
        "Type" => "Pages",
        "Kids" => vec![Object::reference(3), Object::reference(3)],
        "Count" => 2,
        "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(200)],
    })),
    ((3, 0), Object::from(dict! { "Type" => "Page", "Parent" => Object::reference(2), "Rotate" => 90 })),
]);
let resolve = |id: ObjectId| objects.get(&id);
let catalog = dict! { "Pages" => Object::reference(2) };

let table = pages(resolve, &catalog, Rules::Pdfium, &Limits::DEFAULT, &mut ()).unwrap();
assert_eq!((table.len(), table.counted()), (2, 2));

// Listed twice, it's two pages, the second saying so.
let second = table.get(1).unwrap();
assert_eq!((second.id, second.repeat), (Some((3, 0)), 1));

// Its own, inherited, or what stands for it.
assert_eq!(second.rotate().value, 90);
assert_eq!(second.rotate().origin, Origin::Own);
assert_eq!(second.media_box().value, Rect([0.0, 0.0, 100.0, 200.0]));
assert_eq!(second.media_box().origin, Origin::Ancestor(Some((2, 0))));
assert_eq!(second.crop_box().origin, Origin::Default);
assert!(second.attribute(Inheritable::Resources).is_none());
```

Over a file, a walk takes the reading or the document by reference, which says why it gives no object where a closure can't:

```rust
use pdf_semantics::pages::pages;
use pdf_semantics::{Limits, Rules, catalog, xmp};
use pdf_structure::bytes::{Options, Reading};

let file = b"%PDF-1.7\n\
    1 0 obj\n<< /Pages 2 0 R >>\nendobj\n\
    2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
    3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n\
    trailer\n<< /Root 1 0 R >>\n";
let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
let root = catalog(&reading, reading.trailer().dict()).unwrap().unwrap();
let table = pages(&reading, root, Rules::Iso, &Limits::DEFAULT, &mut ()).unwrap();
assert_eq!(table.get(0).unwrap().id, Some((3, 0)));
assert_eq!(xmp::metadata_stream(&reading, root), Ok(None));
```

A name tree, over a closure:

```rust
use std::collections::BTreeMap;

use pdf_semantics::tree::names;
use pdf_semantics::{Fault, Limits, Object, ObjectId, Place, Sink, dict};

#[derive(Default)]
struct Faults(Vec<Fault>);

impl Sink for Faults {
    fn fault(&mut self, _: Place, fault: Fault) {
        self.0.push(fault);
    }
}

// A root, its kid by reference and one listed again.
let leaf = dict! {
    "Limits" => vec![Object::string("a"), Object::string("b")],
    "Names" => vec![
        Object::string("a"), Object::reference(7),
        Object::string("b"), Object::from(2),
    ],
};
let objects = BTreeMap::from([((2, 0), Object::from(leaf))]);
let resolve = |id: ObjectId| objects.get(&id);
let root = Object::from(dict! { "Kids" => vec![Object::reference(2), Object::reference(2)] });

let mut walk = names(resolve, &root, &Limits::DEFAULT, Faults::default());
let (key, value) = walk.next().unwrap().unwrap();
assert_eq!((key, value.as_reference()), (&b"a"[..], Some((7, 0))));
let (key, value) = walk.next().unwrap().unwrap();
assert_eq!((key, value.as_integer()), (&b"b"[..], Some(2)));
assert!(walk.next().is_none());

// Read once, said, and nothing left unread.
assert!(walk.whole());
assert_eq!(walk.into_sink().0, [Fault::TreeRepeated { id: (2, 0) }]);
```

What a file claims, from its packet, a claim elsewhere counting past an attribute that doesn't read:

```rust
use pdf_semantics::Limits;
use pdf_semantics::xmp::{self, PdfA};

let packet = br#"<rdf:Description xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
    xmlns:a="http://www.aiim.org/pdfa/ns/id/" a:part="2" oops=1>
    <a:conformance> b </a:conformance>
    <!-- <a:part>3</a:part> -->
</rdf:Description>"#;

let claims = xmp::read(packet, None, &Limits::DEFAULT, &mut ()).unwrap();
assert_eq!(claims.pdfa_part.as_deref(), Some("2"));
assert_eq!(claims.pdfa_conformance.as_deref(), Some("b"));

let pdfa: PdfA = claims.conformance().unwrap().pdfa.unwrap();
assert_eq!(pdfa.to_string(), "PDF/A-2b");
```

## What it doesn't do

- **Comparing dates across offsets, or showing them.** A date is its fields; a calendar's arithmetic is a caller's, on what `_or_default` gives.
- **Looking a key up by `/Limits`.** A walk's `find` serves, reading what comes before the key.
- **A page's geometry.** A page's boxes are given as they're written and followed, an empty one too: what shows of it is a caller's, by the clause or as PDFium does. Its `/Contents`, `/Annots` and other boxes are in its dictionary.
- **A page tree written.** It's read here, and a document's writer has what numbers its nodes.
- **A reference by its number alone.** A reference names an object by its number and its generation here, as `Resolve` takes them, where PDFium doesn't read the generation. It's one of the few places the walk reads otherwise than PDFium, each on purpose: a node inside itself, a `/Parent` chain that loops, a tree past the caps and none under its root are the others, which `tests/pdfium/differ.tsv` in the repository lists, not in the package.
- **Holding a file to its claims, or writing them.** A claim is the file's own word. XMP's other properties, and a packet not in UTF-8, aren't read.
- **A tree written in nodes.** Nodes under a root would each need an object's number, so that's for what holds a document. A flat tree is a whole one at any size.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.
