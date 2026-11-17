# Changelog

## 0.1.0-alpha.1 (unreleased)

First release. It's an alpha: any alpha may change the API, and each entry here says what changed and how to update. Depend on the exact version, `=0.1.0-alpha.1`; a plain `0.1.0-alpha.1` lets cargo take later alphas too. Its unit tests don't build from the package. They take their files from pdf-fixtures, which is never published, and resolvers from pdf-structure, not yet on crates.io.

### Added

- `date::Date`: a date read (`Date::read`) and written (`Date::write`) as ISO 32000-2 (7.9.4) has it. Each field says whether it was written, and `Date::quirks` what the reading read past. `Date::new` and the `with_` methods build one.
- `pages::pages`: every page of a page tree in order, by ISO's rules or PDFium's (chromium/8152), over any resolver. `pages::counted` gives PDFium's count, also where its rules find no page under the root.
- `pages::Page`: a page's inherited attributes as written (`Page::attribute`), and its boxes, rotation and user unit as values (`media_box`, `crop_box`, `rotate`, `user_unit`), each saying where it came from (`Origin`). `Page::parents` walks its `/Parent` chain.
- `pages::write`: a balanced page tree's nodes over a caller's pages, numbered by the caller, which `pages::pages` reads back by either rules.
- `tree::names` and `tree::numbers`: a name or number tree's entries, a key and a value at a time, in the order they're written. `tree::write_names` and `tree::write_numbers` write one flat.
- `xmp::read` and `xmp::read_from`: what an XMP packet claims of PDF/A, PDF/UA and PDF/VT (14.3.2), from its bytes or through a file's streams. `xmp::metadata_stream` names the stream that holds a file's packet, for a caller that decodes it. `Claims::conformance` reads PDF/A's and PDF/UA's as `PdfA` and `PdfUa`.
- `xmp::write`: a packet of claims that `xmp::read` reads back as they are.
- `info::read`: a document information dictionary (14.3.3) read as `info::Info`, its text decoded by ISO's rules or PDFium's, its dates read (`info::Dated`, one that doesn't read kept as its text) and `/Trapped` named (`info::Trapped`).
- `dest::Dest`, `dest::DestTable`, `dest::resolve_dest`, and `dest::read`: explicit and named destinations (12.3.2) resolved across `/Names/Dests` and `/Dests` by ISO's rules or PDFium's.
- `action::Action`, `action::NamedAction`, `action::AdditionalActions`, `action::OpenAction`, `action::read_chain`, `action::read_additional`, `action::open_action`, and `action::read_file_spec`: bounded `/A`, `/Next`, `/AA`, and `/OpenAction` action chains (7.7.2, 12.6).
- `outlines::OutlineItem`, `outlines::OutlineNode`, `outlines::OutlineFlags`, `outlines::read`, and `outlines::write`: document outline (`/Outlines`, 12.3.3) hierarchies read under cycle and depth caps (with signed `/Count` checks) and written doubly-linked.
- `labels::PageLabels`, `labels::LabelRange`, `labels::LabelStyle`, `labels::read`, and `labels::write`: catalog `/PageLabels` number trees (12.4.2) read, formatted as of PDFium chromium/8152, and written.
- `files::FileSpec`, `files::EmbeddedFile`, `files::AfRelationship`, `files::NewEmbeddedFile`, `files::read_spec`, `files::embedded_files`, `files::associated_files`, `files::write_embedded`, and `files::write_embedded_tree`: file specifications, `/Names/EmbeddedFiles` attachments, and `/AF` associated files (7.11, 14.13).
- `ocg::OcProperties`, `ocg::OcConfig`, `ocg::Ocg`, `ocg::OcState`, and `ocg::read`: optional content groups (`/OCProperties`, `/OCG`, `/OCMD`, 8.11) and boolean `/P` / `/VE` visibility evaluation.
- `viewer::CatalogView`, `viewer::ViewerPrefs`, `viewer::read`, `viewer::write_prefs`, and `viewer::apply_catalog`: catalog `/PageLayout`, `/PageMode`, `/Lang`, and `/ViewerPreferences` (7.7.2, 12.2).
- `info::write`, `xmp::Metadata`, `xmp::read_metadata`, `xmp::read_metadata_from`, `xmp::write_metadata`, `xmp::check_info_sync`, `xmp::date_to_xmp`, and `xmp::xmp_to_date`: `/Info` serialization, Dublin Core / XMP Basic / Adobe PDF / PDF/X / PDF/E packet metadata, and `/Info` <-> XMP field synchronization checks.
- `Fault` and `Limits`: where a file departs from the standard, told with its place to pdf-objects' `fault::Sink`, and the caps every walk and read is under. With `()` as the sink, no fault is looked for.
- `follow` and `catalog`: a reference followed, and the catalog a trailer leads to.
- The `clock` feature, on by default: `Date::now` and `Date::from_unix`, on chrono. The `tracing` feature, off by default: a span over a page tree walked, counted or written, and over a packet read.

### Notes

- It's built on pdf-objects `=0.1.0-alpha.3`, re-exported whole and none of its items by name. Its types, and pdf-tokenizer's `Rules`, are named through `pdf_semantics::pdf_objects` at that version. It needs Rust 1.92.
- Where the page walk by PDFium's rules reads a case otherwise than PDFium, the repository's `tests/pdfium/differ.tsv` lists it and why.
- It never panics on input. An object past the reader's own cap or behind a password ends a walk with pdf-objects' `Unreadable`, so a failed read never looks like an empty tree. A damaged one is a `Fault`, and the walk goes on past it.
- XMP is read as 14.3.2 has it, in UTF-8: a packet in UTF-16 or UTF-32 is told (`Fault::XmpNotUtf8`) and not read, so with `()` as the sink it claims nothing. A claim's element that holds an element beside its text is told (`Fault::XmpMixed`), and its value isn't taken.
- A packet written holds the claims alone. What a PDF/A part wants beside them, as a schema for a namespace it doesn't define, is the caller's.
- A `Debug` holds none of a file's text, which may be decrypted: `info::Info`'s, `Claims`' and the XMP errors' give a text's length, and the XML reader's words, which quote the packet, by theirs.
- `pages::Inheritable`, `date::Offset`, `info::Dated` and `info::Trapped` are exhaustive on purpose, so a match on every value is checked. `Inheritable::ALL` lists the attributes. Every other enum and every struct with public fields but `pages::PdfBox` is `#[non_exhaustive]`, so each can grow in a minor release. `Fault`'s variants aren't, so a fault can be built and compared.
