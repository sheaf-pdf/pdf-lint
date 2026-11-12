# pdf-semantics

Read PDF's dates, name and number trees, page tree and XMP claims (ISO 32000-2, 7.7.3, 7.9 and 14.3.2), and write each of them.

It's made for what reads a file's common structures to show them, check them or edit them, and for what writes them. It reads no file itself. A walk reads a file's objects through the caller's resolver, pdf-objects' `Resolve`. pdf-structure's reading and document are resolvers, and so is a closure over a map. It's built on [pdf-objects](https://crates.io/crates/pdf-objects) and re-exports it whole, so `pdf_semantics::pdf_objects` names its types at the version this crate is built with. pdf-tokenizer's `Rules` is `pdf_semantics::pdf_objects::pdf_tokenizer::Rules`.

## What it does

- **Read and write a date.** `Date::read` reads one (7.9.4). Each field says whether it was written, and `month_or_default` and the like give what stands for it if not. What the reading read past is a `Quirk`, as the apostrophe PDF 1.7 wrote last. `Date::write` writes the clause's form. `Date::new` and the `with_` methods build one, each field in order and in range.
- **List a file's pages.** `pages::pages` lists every page in order (7.7.3), by ISO's rules or PDFium's. On a well-formed tree both give the same pages. PDFium's number each page as PDFium (chromium/8125) does, and stop at the count it gives (`Pages::counted`). ISO's read on past whatever departs from the clause, and tell each. The table (`pages::Pages`) and each page in it are `Send` and `Sync`.
- **Know where a page's attributes come from.** `Page::attribute` gives an inherited attribute as written and followed. `Page::media_box`, `crop_box`, `rotate` and `user_unit` give a value, each saying whether it's the page's own, an ancestor's or a default (`Origin`). `Page::parents` walks the page's `/Parent` chain.
- **Write a page tree.** `pages::write` writes a balanced tree's nodes over the pages a caller has, every page at one depth, under nodes of at most `pages::FANOUT` kids or a fanout the caller gives. The caller numbers the nodes, as a document's `new_id` does. `pages::pages` reads back what it writes, by either rules. It refuses what wouldn't read back, as a page given twice (`pages::WriteError`).
- **Walk name and number trees.** `tree::names` and `tree::numbers` give a tree's entries a key and a value at a time, in the order they're written (7.9.6, 7.9.7). A walk reads nothing it isn't asked for, and each node once, so a loop ends. `whole` says whether anything was left unread. `tree::write_names` and `tree::write_numbers` write a tree flat, and refuse a key given twice (`tree::WriteError`).
- **Read what a file claims to conform to.** `xmp::read` reads an XMP packet's claims of PDF/A, PDF/UA and PDF/VT (14.3.2). A claim is an attribute or an element, found by its namespace, so a prefix bound elsewhere or a comment claims nothing. The first value of each counts. `Claims::conformance` reads PDF/A's and PDF/UA's as `PdfA` and `PdfUa`. `xmp::metadata_stream` says which stream holds a file's packet, for a caller that decodes it. `xmp::read_from` reads a file's packet through what gives its objects and its streams' data, pdf-objects' `Streams`.
- **Write a packet of claims.** `xmp::write` writes an XMP packet that `xmp::read` reads back as the same claims. It refuses a value that wouldn't read back so (`xmp::WriteError`).
- **See where a file departs from the standard.** Each departure is a `Fault`, told with its place to pdf-objects' `fault::Sink`. One sink can take pdf-color's and pdf-fonts' faults too. With `()` as the sink, no fault is looked for. What's read is the same through any sink.
- **Read hostile files.** `Limits` caps a tree's depth and nodes, a page tree's levels and pages, and a packet's bytes, levels and namespaces. Past a cap, a tree's walk tells it and leaves the rest unread, and a page tree's walk or a packet's read stops with an error. An object past the reader's own cap or behind a password stops a walk with pdf-objects' `Unreadable`, so a failed read never looks like an empty tree. A damaged one is a `Fault`, and the walk goes on past it. No DOCTYPE is read and no entity expanded. It never panics on input.

## Features

- **`clock`**, on by default. `Date::now` and `Date::from_unix` give a date from the clock or a count of seconds, on [chrono](https://crates.io/crates/chrono) without its time zones. Turn it off with `default-features = false`, and chrono isn't built.
- **`tracing`**, off by default. A page tree walked or counted (`pages::pages`, `pages::counted`), a packet read (`xmp::read`, `xmp::read_from`) and a page tree written (`pages::write`) each run in a span at `debug`, through [tracing](https://crates.io/crates/tracing). A span's fields are numbers, never a byte of a file. A name or number tree's walk has none, as its loop is the caller's.

## An example

Three pages under a tree written for them, read back; a packet of claims, written and read back; and a date:

```rust
use std::collections::BTreeMap;

use pdf_semantics::date::Date;
use pdf_semantics::pages::{self, Origin, Rect};
use pdf_semantics::pdf_objects::pdf_tokenizer::Rules;
use pdf_semantics::pdf_objects::{Object, ObjectId, dict};
use pdf_semantics::{Limits, xmp};

// Three pages, objects 1 to 3, under a tree whose nodes are numbered from 10.
let numbered = [ObjectId::new(1, 0), ObjectId::new(2, 0), ObjectId::new(3, 0)];
let mut next = 10;
let ids = || {
    next += 1;
    ObjectId::new(next - 1, 0)
};
let written = pages::write(&numbered, 2, &Limits::DEFAULT, ids).unwrap();

// The nodes, a box on the root for every page, and each page under its node.
let media = vec![Object::from(0), Object::from(0), Object::from(612), Object::from(792)];
let mut objects = BTreeMap::new();
for (id, mut node) in written.nodes {
    if id == written.root {
        node.set("MediaBox", media.clone());
    }
    objects.insert(id, Object::from(node));
}
for (&page, &parent) in numbered.iter().zip(&written.parents) {
    objects.insert(page, Object::from(dict! { "Type" => "Page", "Parent" => parent }));
}

// Read back by ISO's rules with nothing told, the last page's box its root's.
let resolve = |id: ObjectId| objects.get(&id);
let catalog = dict! { "Pages" => written.root };
let mut faults = Vec::new();
let table = pages::pages(resolve, &catalog, Rules::Iso, &Limits::DEFAULT, &mut faults).unwrap();
assert!(faults.is_empty());
let last = table.get(2).unwrap();
assert_eq!(last.id, Some(ObjectId::new(3, 0)));
assert_eq!(last.media_box().value, Rect([0.0, 0.0, 612.0, 792.0]));
assert_eq!(last.media_box().origin, Origin::Ancestor(Some(written.root)));
assert_eq!(last.parents().collect::<Vec<_>>(), [ObjectId::new(12, 0), written.root]);

// A packet claiming PDF/A-2b, written and read back.
let mut claims = xmp::Claims::default();
claims.pdfa_part = Some("2".into());
claims.pdfa_conformance = Some("B".into());
let packet = xmp::write(&claims).unwrap();
let read = xmp::read(packet.as_bytes(), None, &Limits::DEFAULT, &mut ()).unwrap();
assert_eq!(read.conformance().unwrap().pdfa.unwrap().to_string(), "PDF/A-2b");

// A date: the apostrophe PDF 1.7 wrote last is read past, and not written.
let date = Date::read("D:20261111093000-08'00'").unwrap();
assert_eq!((date.year(), date.month()), (2026, Some(11)));
assert_eq!(date.write(), "D:20261111093000-08'00");
```

## How it's held to PDFium

The page walk by PDFium's rules follows PDFium's behaviour as of chromium/8125, written in this crate's own code. Cases in the repository's `tests/pdfium` hold it there, each with PDFium's answer recorded from that build. Where this crate reads a case otherwise, `tests/pdfium/differ.tsv` lists it with why. A reference names an object by its number and its generation here, where PDFium reads the number alone. A node listed inside itself, a `/Parent` chain that loops, a tree past the caps and a root with no pages under it are errors here (`pages::Error`), where PDFium reads on quietly or loads no page. Those files aren't in the package.

## Exhaustive on purpose, or not

`pages::Inheritable` and `date::Offset` are exhaustive, as their lists are closed and a match on every value is checked. `Inheritable::ALL` lists the attributes, and `Inheritable::key` gives each one's key. Every other enum and every struct with public fields but `pages::Rect` is `#[non_exhaustive]`, so each can grow in a minor release. `Fault`'s variants aren't, so a fault can be built and compared.

## What isn't here

- **Date arithmetic.** A date is its fields. Comparing dates across offsets, or showing them, is the caller's.
- **A page's geometry and content.** A page's boxes are given as written and followed, an empty one too. Its `/Contents` and `/Annots` are in its dictionary.
- **A lookup by `/Limits`.** A walk's `find` serves, reading what comes before the key.
- **XMP not in UTF-8.** 14.3.2 has a packet in UTF-8, so one in UTF-16 or UTF-32 is told (`Fault::XmpNotUtf8`) and not read. With `()` as the sink, it reads as a packet that claims nothing, so a caller that must tell the two apart passes a sink.
- **Holding a file to its claims.** A claim is the file's own word, and XMP's other properties aren't read. A packet written holds the claims alone. What a PDF/A part wants beside them, as a schema for a namespace or the document information dictionary mirrored, is the caller's.
- **A name or number tree written in nodes.** `tree::write_names` and `write_numbers` write a tree flat. A tree in nodes, numbered by the caller as `pages::write` numbers its own, isn't written.

## Stability

This is an alpha: any alpha may change the API, and the CHANGELOG says how to follow. Depend on the exact version, `pdf-semantics = "=0.1.0-alpha.1"`. It needs Rust 1.89 or later.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
