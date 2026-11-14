# pdf-objects

Owned PDF object values, byte-span tracking, object parser/serializer, text string codecs, and shared resolver traits built on [pdf-tokenizer](https://crates.io/crates/pdf-tokenizer) (ISO 32000-2 §7.3).

`pdf-objects` forms the shared data model across the workspace. Every higher-level crate—from [pdf-structure](https://crates.io/crates/pdf-structure) to domain crates like `pdf-semantics`, `pdf-color`, and `pdf-fonts`—operates on `pdf_objects::Object` and its `Resolve`, `Streams`, and `fault::Sink` traits. For a single battery-included API to open and write PDFs, see [pdfs](https://crates.io/crates/pdfs).

## Core Concepts

- **Span-preserving values (`Object`, `Value`, `Span`):** Every `Object` pairs a `Value` with an optional `Span` recording the exact file or decoded object-stream byte range it was parsed from.
- **Edit tracking on mutation:** Mutating an `Object` via `value_mut`, `as_dict_mut`, `as_array_mut`, or `as_stream_mut` automatically clears the `Span` on the modified node and its ancestors while leaving untouched siblings' spans intact—allowing writers to tell modified objects apart from untouched ones.
- **Dictionaries as written (`Dict`):** Preserves entry order, key spans, and duplicate keys (`get` returns the last value for a key, matching PDF viewers, while `duplicates` yields shadowed entries). Lookups up to 32 entries require zero auxiliary allocation.
- **Text string codecs (`text`):** Decodes and encodes PDF text strings (`PDFDocEncoding`, `UTF-16BE`, and `UTF-8` with BOM), reporting encoding `Quirk`s. Supports both strict ISO 32000-2 rules and PDFium rules (tested against PDFium as of chromium/8152 via `text::decode_with`).
- **Object parser (`parse::Parser`, `Limits`):** Parses direct values and indirect objects (`header`, `body`, `stream`) from full buffers or streaming positional windows (`Parser::window`) under configurable nesting and value-count `Limits`, using either `Rules::Iso` or `Rules::Pdfium`.
- **Deterministic serializer (`serialize`):** Writes values and indirect objects back to PDF syntax, with optional on-the-fly reference renumbering (`serialize::Renumber`) and string/stream encryption hooks (`serialize::Encrypt`).
- **Cross-crate traits (`Resolve`, `Streams`, `fault::Sink`):**
  - `Resolve` looks up indirect objects by `ObjectId` (implemented by closures, `&Reading`, and `&Document`), with `try_resolve` returning `Unreadable` (`Why::Damaged`, `Capped`, `Locked`, `Source`).
  - `Streams` extends `Resolve` to return decrypted, filter-decoded stream bytes up to a caller-specified limit (`WithStreams` pairs any resolver with a stream closure).
  - `fault::Sink<F>` collects recoverable spec departures with their `Place`; pass `&mut ()` to skip fault collection at zero cost.

## Quickstart: Objects, Dictionaries, and Text Strings

```rust
use pdf_objects::text::{self, Encoding, Quirk};
use pdf_objects::{Object, Span, Version, dict};

// Dictionaries preserve entry order and duplicate keys; `get` returns the last one.
let page = dict! { "Type" => "Page", "Rotate" => 90, "Rotate" => 180 };
assert_eq!(page.get(b"Rotate").and_then(Object::as_integer), Some(180));
let earlier: Vec<_> = page.duplicates().map(|e| e.value.as_integer()).collect();
assert_eq!(earlier, [Some(90)]);

// Attach or inspect byte spans.
let rotate = Object::from(180).at(Span::new(None, 40, 47));
assert_eq!(rotate.at.map(|at| (at.stream, at.start, at.end)), Some((None, 40, 47)));

// Decode and encode PDF text strings (UTF-16BE BOM with a trailing odd byte).
let title = text::decode(b"\xFE\xFF\x00H\x00i\x00");
assert_eq!((title.text.as_str(), title.encoding), ("Hi", Encoding::Utf16Be));
assert_eq!(text::decode(&text::encode("Hi é")).text, "Hi é");
assert_eq!(title.quirks, [Quirk::OddByte]);

// Parse and compare PDF versions.
assert_eq!(Version::parse(b" 2.0\n"), Some(Version::new(2, 0)));
assert!(Version::new(1, 7) < Version::new(2, 0));
```

## Parsing Indirect Objects & Streams

Because `/Length` on a stream may be an indirect reference or inaccurate in damaged files, `Parser` lets the caller choose between the declared length (`head.declared(len)`) and scanning for `endstream` (`head.scanned()`):

```rust
use pdf_objects::parse::{Body, Fault, Parser, Sink};
use pdf_objects::{Span, Value};

struct Said(Vec<String>);
impl Sink<'_> for Said {
    fn fault(&mut self, _at: Span, fault: Fault<'_>) {
        self.0.push(fault.to_string());
    }
}

let file = b"4 0 obj\n<< /Length 5 /N#00 1 >>\nstream\nhello\nendstream\nendobj\n";
let mut said = Said(Vec::new());
let mut parser = Parser::at(file, 0);
let id = parser.header().unwrap();
let Body::Stream(head) = parser.body(id, &mut said).unwrap() else { panic!() };
let length = head.dict().get(b"Length").and_then(|l| l.as_integer());
let declared = length.and_then(|len| head.declared(usize::try_from(len).ok()?));
let data = declared.unwrap_or_else(|| head.scanned());
let stream = parser.stream(head, data, &mut said).unwrap();
let Value::Stream(s) = stream.value() else { panic!() };
assert_eq!(&file[data.start..data.end], b"hello");
assert_eq!(s.data.at(), Some(data));
assert_eq!(said.0, ["the name /N#00 has #00, NUL, which a name can't hold, which readers read differently"]);
```

## Editing and Serializing Objects

```rust
use pdf_objects::parse::Parser;
use pdf_objects::{Object, serialize};

let file = b"<< /Type /Annot /Rect [0 0 10 10] /Contents (old) >>";
let mut annot = Parser::at(file, 0).value(&mut ()).unwrap();
let dict = annot.as_dict_mut().unwrap();
dict.set("Contents", Object::text("new"));
let rect = dict.get_mut(b"Rect").and_then(Object::as_array_mut).unwrap();
rect[2] = Object::from(20);

// Modified containers drop their byte spans; untouched elements keep theirs.
assert_eq!(annot.at, None);
let dict = annot.as_dict().unwrap();
assert_eq!(dict.get(b"Rect").unwrap().at, None);
let rect = dict.get(b"Rect").and_then(Object::as_array).unwrap();
let zero = rect[0].at.unwrap();
assert_eq!(&file[zero.start..zero.end], b"0");
assert!(dict.get(b"Type").unwrap().at.is_some());

let mut out = Vec::new();
serialize::write(&mut out, &annot).unwrap();
assert_eq!(out, b"<</Type/Annot/Rect[0 0 20 10]/Contents(new)>>");
```

## Scope & Non-Goals

- **File structure, stream filters, and ciphers:** Cross-reference tables/streams, object streams, decompression, and encryption live in [pdf-structure](https://crates.io/crates/pdf-structure), [pdf-filters](https://crates.io/crates/pdf-filters), and [pdf-crypt](https://crates.io/crates/pdf-crypt).

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
