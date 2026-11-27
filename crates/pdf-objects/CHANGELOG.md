# Changelog

## 0.1.0-alpha.3 (unreleased)

### Added

- `#![no_std]` + `alloc` support when `default-features = false` (`std` feature enabled by default).
- `SmallBytes` and `Name` inline small-buffer byte types (`<= 15` bytes inline), shrinking `Object` to 64 bytes and `Entry` to 112 bytes on 64-bit targets.
- `ResolveFn` / `resolve_fn` closure adapter and `Resolve` implementations for `&BTreeMap<ObjectId, Object>`, `&HashMap<ObjectId, Object, S>` (`std`), `Box<R>`, `Arc<R>`, `&WithStreams<R, F>`, and `&(dyn Resolve + Send + Sync)`.
- `Streams::decoded_standard`, `DecodedStream`, `FilterTail`, `FilterStep`, and shared `/Filter` + `/DecodeParms` pipeline parsing (`streams::parse_filter_chain`, `parse_filter_chain_with`, `ParsedFilterChain`, `StandardFilter`, `FilterPredictor`, `PredictorLayout`, `Unfiltered`, `expand_filter_name`, `FILTERS`): decode indirect streams through leading standard filters while stopping at trailing non-standard or image codecs (`DCTDecode`, `CCITTFaxDecode`, `JBIG2Decode`, `JPXDecode`), defaulting to `Streams::decoded` for existing implementations.
- `text::pdfium` submodule (`decode`, `Text`, `Encoding`, `Quirk`) for PDFium text-string decoding (as of chromium/8152), `text::decode_str(bytes, rules)`, `Str` decode helpers (`decode`, `decode_pdfium`, `decode_str`, `is_encrypted`), `Text::languages` for ISO 32000-2 §7.9.2.2.2 `0x1B` language tags, and `const fn pdf_doc_byte(char) -> Option<u8>`.
- `OwnedFault` (`Fault::into_owned`), `FaultSink` adapter from `fault::Sink<OwnedFault>` to `parse::Sink`, and `parse::Sink` implementations for `Vec<(Span, Fault<'a>)>` and `Vec<(Span, OwnedFault)>`.
- Parser faults `Fault::ZeroObjectNumber`, `Fault::SignedObjectId`, `Fault::StreamSpace`, `Fault::BeforeEndstream`, and `Fault::NoEndobj`; serializer errors `serialize::Error::RealOutOfRange`, `InlineStream`, `NulName`, and `ZeroObjectId`.
- `Limits::new`, `Limits::with_nesting`, and `Limits::with_values`; `Object::eq_value`, `Object::name`, `Object::with_span`, and `Object::clear_span`; `Span::try_new`, `Span::slice_in`, and `Span::stream_slice_in`; `ExactSizeIterator` on `Dict` iterators (`Iter`, `IterMut`, `LastEntries`, `Duplicates`).

### Changed

- **Breaking:** Removed blanket `impl<F: Fn(ObjectId) -> Option<&Object>> Resolve for F` in favor of `ResolveFn` / `resolve_fn` to avoid trait coherence conflicts on smart pointers and wrapper types.
- **Breaking:** Encapsulated `Span` fields (`start()`, `end()`, `stream()`, `len()`, `is_empty()`, `range()`), `Object::at` (`at()`, `with_span()`, `clear_span()`), and `Entry` fields (`key()`, `key_at()`, `value()`, `value_mut()`, `set_key()`).
- **Breaking:** Replaced `Value::Name(Vec<u8>)` with `Value::Name(Name)` and `Str::Bytes` / `Str::Encrypted` payloads with `SmallBytes`; removed `Str::Text(String)` and `From<&str>` / `From<String>` -> `Value::Name` conversions on `Object` (use `Object::name` or `Object::text`).
- **Breaking:** Separated ISO 32000-2 `text` (`Encoding::{Utf16Be, Utf8, PdfDoc}`) from `text::pdfium`; `serialize::name` now returns `Result<(), serialize::Error>` to reject NUL bytes; replaced `Encrypt::clear_key` with `Encrypt::is_clear_key(&self, id, dict, key) -> bool`.
- **Breaking:** `Parser::header(&mut self, sink)` now accepts a fault sink, `Parser::body` / `Parser::stream` consume trailing `endobj` (or emit `Fault::NoEndobj`), and `parse::Error` is boxed (`8` bytes, dereferencing to `parse::ErrorKind`).
- Converted `Object` (`Drop`, `Clone`, `PartialEq`, `eq_value`), `Parser`, and `serialize` container walks to iterative heap-stack state machines for constant call-stack usage on deeply nested inputs.
- `Dict` lookup and duplicate detection use an inline 64-entry `u64` bitset and deterministic index sorting above 64 entries; `Dict::set` updates the last matching entry in place and removes shadowed duplicates.
- `WithStreams::decoded` now enforces the caller's byte `limit` (`Why::Limit`), and `Debug` redacts `Str::Encrypted` and `Data::Bytes` payloads alongside capped `Fault` byte previews.
- Raised MSRV from Rust 1.89 to 1.92 to match the workspace.

### Fixed

- `StreamHead::scanned()` skips false `endstream` matches followed by regular characters, records window-boundary touches when `endstream` lands at a non-final window cut, and strips at most one preceding EOL (`\r\n`, `\n`, or `\r`).
- `text::decode` maps all 24 undefined bytes in ISO 32000-2 Table D.2 to `U+FFFD` with `Quirk::Undefined(u8)`, restricts `Object::text` verbatim `PDFDocEncoding` selection to defined identity code points, and strips `0x1B` language escapes before UTF-8/UTF-16 decoding.
- `serialize::real` round-trips finite `f64` values whose fixed-point token fits within 255 bytes instead of clamping at `3.4e38` or flushing subnormals to `0.0`.

## 0.1.0-alpha.2 (2026-11-07)

### Changed

- Raised MSRV from Rust 1.85 to 1.89 to match the workspace.

## 0.1.0-alpha.1 (2026-11-06)

First release. Pre-1.0 alpha releases may include breaking API changes documented here; depend on the exact version (`=0.1.0-alpha.1`).

### Added

- `Object`, `Value`, `ObjectId`, `Version`, and `Span`: owned PDF object representation (null, bool, integer, real, name, string with `Form`, array, `Dict`, boxed `Stream`, and `Ref`) paired with optional byte/object-stream spans that clear along mutated paths (`value_mut`, `as_*_mut`) while preserving spans on non-structural edits (`as_written_mut`).
- `Dict`, `Entry`, and `dict!`: entry-order-preserving dictionary retaining key spans and duplicate keys (`get`, `set`, `push`, `remove`, `remove_duplicates`) with named, fused iterators (`Iter`, `IterMut`, `LastEntries`, `Duplicates`).
- `Stream` and `Data`: stream dictionary + payload representation (`Data::At`, `Data::Bytes`, `Data::Unplaced`) with `/Length` synchronization (`Stream::new`, `set_bytes`).
- `Resolve` and `Streams` (`WithStreams`, `Unreadable`, `Why`): traits decoupling domain crates from file I/O for indirect object resolution and bounded stream decoding.
- `Object::references`, `Object::renumber`, and `Object::unplace`: source-order reference iteration, reference renumbering, and span stripping.
- `fault::Sink` and `fault::Place`: zero-cost-when-disabled (`()`) or collected (`Vec<(Place, F)>`) non-fatal diagnostic sink shared across domain crates.
- `parse::Parser`, `Limits`, and `Fault`: direct and indirect object parser over `pdf-tokenizer` supporting strict ISO 32000-2 (`Rules::Iso`) and PDFium (`Rules::Pdfium`) lexical rules, windowed file reads (`Error::Short`), object streams (`in_stream`), and nesting/value caps.
- `serialize` and `serialize::Encrypt`: canonical PDF object and stream serialization (`write`, `indirect`, `indirect_stream`, `indirect_with`, `indirect_stream_with`) with on-the-fly reference renumbering (`Renumber`) and streaming string/stream encryption hooks.
- `text` codec: UTF-16BE, UTF-8, and `PDFDocEncoding` text string decoding (`decode`, `decode_with`, `Quirk`) and lossless encoding (`encode`, `encode_as`, `Object::text`, `Unencodable`).

### Notes

- Re-exports `pdf-tokenizer` at `pdf_tokenizer`. Extracted and expanded from `pdf-lint 0.1.0`'s internal object, parser, text, and version modules.
