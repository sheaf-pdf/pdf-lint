# Changelog

## 0.1.0-alpha.1 (unreleased)

First release. It's an alpha: any alpha may change the API, and each entry here says what changed and how to update. Depend on the exact version, `=0.1.0-alpha.1`; a plain `0.1.0-alpha.1` lets cargo take later alphas too. Its unit tests don't build from the package. They take their files from pdf-fixtures, which is never published, and resolvers from pdf-structure, not yet on crates.io.

### Added

- `date::Date`: a date read (`Date::read`) and written (`Date::write`) as ISO 32000-2 (7.9.4) has it. Each field says whether it was written, and `Date::quirks` what the reading read past. `Date::new` and the `with_` methods build one.
- `pages::pages`: every page of a page tree in order, by ISO's rules or PDFium's (chromium/8125), over any resolver. `pages::counted` gives PDFium's count, also where its rules find no page under the root.
- `pages::Page`: a page's inherited attributes as written (`Page::attribute`), and its boxes, rotation and user unit as values (`media_box`, `crop_box`, `rotate`, `user_unit`), each saying where it came from (`Origin`). `Page::parents` walks its `/Parent` chain.
- `pages::write`: a balanced page tree's nodes over a caller's pages, numbered by the caller, which `pages::pages` reads back by either rules.
- `tree::names` and `tree::numbers`: a name or number tree's entries, a key and a value at a time, in the order they're written. `tree::write_names` and `tree::write_numbers` write one flat.
- `xmp::read` and `xmp::read_from`: what an XMP packet claims of PDF/A, PDF/UA and PDF/VT (14.3.2), from its bytes or through a file's streams. `xmp::metadata_stream` names the stream that holds a file's packet, for a caller that decodes it. `Claims::conformance` reads PDF/A's and PDF/UA's as `PdfA` and `PdfUa`.
- `xmp::write`: a packet of claims that `xmp::read` reads back as they are.
- `Fault` and `Limits`: where a file departs from the standard, told with its place to pdf-objects' `fault::Sink`, and the caps every walk and read is under. With `()` as the sink, no fault is looked for.
- `follow` and `catalog`: a reference followed, and the catalog a trailer leads to.
- The `clock` feature, on by default: `Date::now` and `Date::from_unix`, on chrono. The `tracing` feature, off by default: a span over a page tree walked, counted or written, and over a packet read.

### Notes

- It's built on pdf-objects `=0.1.0-alpha.2`, re-exported whole and none of its items by name. Its types, and pdf-tokenizer's `Rules`, are named through `pdf_semantics::pdf_objects` at that version. It needs Rust 1.89.
- Where the page walk by PDFium's rules reads a case otherwise than PDFium, the repository's `tests/pdfium/differ.tsv` lists it and why.
- It never panics on input. An object past the reader's own cap or behind a password ends a walk with pdf-objects' `Unreadable`, so a failed read never looks like an empty tree. A damaged one is a `Fault`, and the walk goes on past it.
- XMP is read as 14.3.2 has it, in UTF-8: a packet in UTF-16 or UTF-32 is told (`Fault::XmpNotUtf8`) and not read, so with `()` as the sink it claims nothing. A claim's element that holds an element beside its text is told (`Fault::XmpMixed`), and its value isn't taken.
- A packet written holds the claims alone. What a PDF/A part wants beside them, as a schema for a namespace it doesn't define, is the caller's.
- `pages::Inheritable` and `date::Offset` are exhaustive on purpose, so a match on every value is checked. `Inheritable::ALL` lists the attributes. Every other enum and every struct with public fields but `pages::Rect` is `#[non_exhaustive]`, so each can grow in a minor release. `Fault`'s variants aren't, so a fault can be built and compared.
