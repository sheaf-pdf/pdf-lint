# pdf-semantics

Read and write PDF document-level data structures—dates, page trees, name and number trees, `/Info` metadata, and XMP conformance packets (ISO 32000-2, §7.7.3, §7.9, §14.3).

`pdf-semantics` operates over any [pdf-objects](https://crates.io/crates/pdf-objects) `Resolve` (and `Streams` for XMP streams), such as a `pdf-structure` `&Reading`, `&Document`, or an in-memory closure, without performing file I/O directly. `pdf-objects` is re-exported at `pdf_semantics::pdf_objects`.

## Core Concepts

- **Page trees (`pages`):**
  - `pages::pages` walks a document's `/Pages` tree in page order under either strict ISO 32000-2 rules or PDFium's rules as of chromium/8152 (`Rules::Iso` vs. `Rules::Pdfium`).
  - `Page` resolves inherited attributes (`media_box`, `crop_box`, `rotate`, `user_unit`, `attribute`) and reports their `Origin` (direct on the page, inherited from an ancestor, or default).
  - `pages::write` constructs balanced `/Pages` node hierarchies with configurable fanout (`pages::FANOUT`).
- **Name & number trees (`tree`):** `tree::names` and `tree::numbers` lazily iterate `(key, value)` entries in tree order with cycle and depth protection; `tree::write_names` and `tree::write_numbers` serialize sorted entries into flat trees.
- **Dates (`date::Date`):** Parse (`Date::read`) and format (`Date::write`) PDF `D:...` date strings, tracking omitted fields and legacy syntax `Quirk`s (such as trailing PDF 1.7 apostrophes). With the default `clock` feature, `Date::now` and `Date::from_unix` construct dates from timestamps.
- **XMP conformance claims (`xmp`):** `xmp::read` and `xmp::read_from` extract namespace-verified PDF/A, PDF/UA, and PDF/VT conformance claims (`Claims::conformance` -> `PdfA`, `PdfUa`) without entity expansion or `DOCTYPE` processing; `xmp::write` serializes conformance claims back into a valid XMP packet.
- **Document `/Info` dictionaries (`info`):** `info::read` decodes standard `/Info` text strings (under ISO or PDFium rules) and dates (`info::Dated`).
- **Diagnostics & safety (`Fault`, `Limits`):** Spec departures are reported with byte spans to a `pdf-objects` `fault::Sink<Fault>` (skipped at zero cost when `()` is passed). `Limits` bounds tree depth, node counts, page counts, and XMP packet sizes; locked or capped objects fail explicitly with `Unreadable` rather than silently truncating results.

## Quickstart

Write a balanced page tree for three pages and read it back, round-trip a PDF/A-2b XMP packet, and normalize a PDF 1.7 date:

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

## Cargo Features

- **`clock`** *(default)*: Enables `Date::now` and `Date::from_unix` via [chrono](https://crates.io/crates/chrono) (without timezone databases). Disable default features if you only parse and format date strings.
- **`tracing`** *(optional)*: Emits numeric [tracing](https://crates.io/crates/tracing) spans around page-tree walks/writes and XMP packet reads.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
