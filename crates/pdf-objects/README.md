# pdf-objects

Owned PDF syntax objects, byte-span tracking, iterative parser and serializer, text string codecs, and shared resolver traits built on [pdf-tokenizer](https://crates.io/crates/pdf-tokenizer) (ISO 32000-2 §7.3). Supports `#![no_std]` + `alloc` when default features (`std`) are disabled.

`pdf-objects` provides the zero-I/O object representation shared across higher-level crates (`pdf_objects::Object`, `Resolve`, `Streams`, and `fault::Sink`). For an end-to-end PDF reader and writer, see [pdfs](https://crates.io/crates/pdfs).

## Core Concepts

- **Compact, span-preserving values (`Object`, `Value`, `SmallBytes`, `Span`):** Each `Object` fits in a single 64-byte cache line on 64-bit targets, pairing a `Value` with an optional `Span` recording its source file or object-stream byte range and storing short names/strings inline via `SmallBytes`.
- **Edit tracking on mutation:** Mutating an `Object` via `value_mut`, `as_dict_mut`, `as_array_mut`, or `as_stream_mut` automatically clears the `Span` on the modified node and its ancestors while leaving untouched siblings' spans intact—enabling copy-on-write serialization.
- **Dictionaries as written (`Dict`):** Preserves entry order, key spans, and duplicate keys (`get` returns the last value for a key, `set` updates the last entry and cleans up shadowed duplicates, and `duplicates` yields shadowed entries). Duplicate detection up to 64 entries uses an inline bitset with zero heap allocation.
- **Separate ISO and PDFium text codecs (`text`, `text::pdfium`):** `text` implements bidirectional ISO 32000-2 §7.9.2.2 text string decoding and encoding (`PDFDocEncoding`, `UTF-16BE`, and `UTF-8` with BOM, including `0x1B` language escape sequences). `text::pdfium` reproduces PDFium's text decoding behavior (tested against PDFium as of chromium/8152).
- **Stack-safe parser (`parse::Parser`, `Limits`):** Iterative heap-stack parser for direct values, indirect objects (`header`, `body`, `stream`), and streaming positional windows (`Parser::window`) under configurable nesting and value-count `Limits`, using either `Rules::Iso` or `Rules::Pdfium` (as of chromium/8152).
- **Deterministic serializer (`serialize`):** Iterative serializer for direct values and indirect objects/streams, rejecting unparseable constructs (`InlineStream`, `NulName`, `ZeroObjectId`, `RealOutOfRange`) and supporting on-the-fly `Renumber` and `Encrypt` hooks.
- **Zero-I/O resolver traits (`Resolve`, `Streams`, `fault::Sink`):**
  - `Resolve` looks up indirect objects by `ObjectId` (implemented for `&BTreeMap`, `&HashMap`, smart pointers, trait objects, and closures wrapped in `ResolveFn` / `resolve_fn`), with `try_resolve` returning `Unreadable`.
  - `Streams` extends `Resolve` to return decrypted, filter-decoded stream bytes bounded by `limit` (`WithStreams` pairs any resolver with a bounded stream closure).
  - `fault::Sink<F>` collects recoverable spec departures with their `Place`; pass `&mut ()` to disable fault collection at zero cost.

## Quickstart: Objects, Dictionaries, and Text Strings

```rust
use pdf_objects::text::{self, Encoding, Quirk};
use pdf_objects::{Object, Span, Version, dict};

// Dictionaries preserve entry order and duplicate keys; `get` returns the last one.
let page = dict! {
    "Type" => Object::name("Page"),
    "Rotate" => 90,
    "Rotate" => 180,
};
assert_eq!(page.get("Rotate").and_then(Object::as_integer), Some(180));
let shadowed: Vec<_> = page.duplicates().map(|e| e.value().as_integer()).collect();
assert_eq!(shadowed, [Some(90)]);

// Attach or inspect byte spans.
let span = Span::new(None, 40, 43);
let rotate = Object::from(180).with_span(span);
assert_eq!(rotate.at().map(|s| (s.stream(), s.start(), s.end())), Some((None, 40, 43)));

// Decode and encode ISO 32000-2 text strings (UTF-16BE BOM with a trailing odd byte).
let title = text::decode(b"\xFE\xFF\x00H\x00i\x00");
assert_eq!((title.text.as_str(), title.encoding), ("Hi", Encoding::Utf16Be));
assert_eq!(title.quirks, [Quirk::OddByte]);
assert_eq!(text::decode(&text::encode("Hi é")).text, "Hi é");

// Parse and compare PDF header versions.
assert_eq!(Version::parse(b" 2.0\n"), Some(Version::new(2, 0)));
assert!(Version::new(1, 7) < Version::new(2, 0));
```

## Parsing Indirect Objects & Streams

Because `/Length` on a stream may be an indirect reference or inaccurate in damaged files, `Parser` lets the caller choose between the declared length (`head.declared(len)`) and scanning for a standalone `endstream` keyword (`head.scanned()`):

```rust
use pdf_objects::parse::{Body, OwnedFault, Parser};
use pdf_objects::{Span, Value};

let file = b"4 0 obj\n<< /Length 5 /N#00 1 >>\nstream\nhello\nendstream\nendobj\n";
let mut faults: Vec<(Span, OwnedFault)> = Vec::new();
let mut parser = Parser::at(file, 0);
let id = parser.header(&mut faults).unwrap();
let Body::Stream(head) = parser.body(id, &mut faults).unwrap() else { panic!() };
let length = head.dict().get("Length").and_then(|l| l.as_integer());
let declared = length.and_then(|len| head.declared(usize::try_from(len).ok()?));
let data = declared.unwrap_or_else(|| head.scanned());
let stream = parser.stream(head, data, &mut faults).unwrap();
let Value::Stream(s) = stream.value() else { panic!() };
assert_eq!(data.slice_in(file), Some(&b"hello"[..]));
assert_eq!(s.data.at(), Some(data));
assert_eq!(
    faults[0].1.to_string(),
    "the name /N#00 has #00, NUL, which a name can't hold, which readers read differently"
);
```

## Editing and Serializing Objects

```rust
use pdf_objects::parse::Parser;
use pdf_objects::{Object, serialize};

let file = b"<< /Type /Annot /Rect [0 0 10 10] /Contents (old) >>";
let mut annot = Parser::at(file, 0).value(&mut ()).unwrap();
let dict = annot.as_dict_mut().unwrap();
dict.set("Contents", Object::text("new"));
let rect = dict.get_mut("Rect").and_then(Object::as_array_mut).unwrap();
rect[2] = Object::from(20);

// Modified containers drop their byte spans; untouched elements keep theirs.
assert_eq!(annot.at(), None);
let dict = annot.as_dict().unwrap();
assert_eq!(dict.get("Rect").unwrap().at(), None);
let rect = dict.get("Rect").and_then(Object::as_array).unwrap();
assert_eq!(rect[0].at().unwrap().slice_in(file), Some(&b"0"[..]));
assert!(dict.get("Type").unwrap().at().is_some());

let mut out = Vec::new();
serialize::write(&mut out, &annot).unwrap();
assert_eq!(out, b"<</Type/Annot/Rect[0 0 20 10]/Contents(new)>>");
```

## Scope & Non-Goals

- **File structure, stream filters, and ciphers:** Cross-reference tables/streams, object streams, decompression, and encryption live in [pdf-structure](https://crates.io/crates/pdf-structure), [pdf-filters](https://crates.io/crates/pdf-filters), and [pdf-crypt](https://crates.io/crates/pdf-crypt).

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
