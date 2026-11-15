# Changelog

## 0.1.0-alpha.3 (unreleased)

### Added
- `#![no_std]` + `alloc` support when `default-features = false` (`std` feature enabled by default).
- `SmallBytes` and `Name` types with inline small-buffer storage (`<= 15` bytes inline, thin `Box<[u8]>` up to `u32::MAX`, boxed spill above), shrinking `Object` to 64 bytes (one cache line) and `Entry` to 112 bytes on 64-bit targets.
- `ResolveFn` and `resolve_fn` wrapper for closure resolvers, plus `Resolve` implementations for `&BTreeMap<ObjectId, Object>`, `&HashMap<ObjectId, Object, S>` (`std`), `Box<R>`, `Arc<R>`, `&WithStreams<R, F>`, and `&(dyn Resolve + Send + Sync)`.
- `text::pdfium` submodule (`pdfium::decode`, `pdfium::Text`, `pdfium::Encoding`, `pdfium::Quirk` including `SplitSurrogate`) dedicated to PDFium text-string decoding (as of chromium/8152), plus `text::decode_str(bytes, rules)` and `Str` decode helpers (`decode`, `decode_pdfium`, `decode_str`, `is_encrypted`).
- `Text::languages` on ISO `text::Text` capturing ISO 32000-2 §7.9.2.2.2 `0x1B` language escape tags, plus `const fn pdf_doc_byte(char) -> Option<u8>`.
- `Limits::new`, `Limits::with_nesting`, and `Limits::with_values` constructors.
- `OwnedFault` (`Fault::into_owned`), `FaultSink` adapter from `fault::Sink<OwnedFault>` to `parse::Sink`, and `parse::Sink` implementations for `Vec<(Span, Fault<'a>)>` and `Vec<(Span, OwnedFault)>`.
- Parser faults `Fault::ZeroObjectNumber`, `Fault::SignedObjectId`, `Fault::StreamSpace`, `Fault::BeforeEndstream`, and `Fault::NoEndobj`.
- Serializer error variants `serialize::Error::RealOutOfRange`, `InlineStream`, `NulName`, and `ZeroObjectId`.
- `Object::eq_value`, `Object::name`, `Object::with_span`, `Object::clear_span`, `Span::try_new`, `Span::slice_in`, `Span::stream_slice_in`, and `ExactSizeIterator` on `Dict` iterators (`Iter`, `IterMut`, `LastEntries`, `Duplicates`).

### Changed
- **Breaking:** Removed blanket `impl<F: Fn(ObjectId) -> Option<&Object>> Resolve for F` in favor of `ResolveFn` / `resolve_fn` to eliminate trait coherence conflicts on smart pointers and wrapper types.
- **Breaking:** Encapsulated `Span` fields (`start()`, `end()`, `stream()`, `len()`, `is_empty()`, `range()`), `Object::at` (`at()`, `with_span()`, `clear_span()`), and `Entry` fields (`key()`, `key_at()`, `value()`, `value_mut()`, `set_key()`).
- **Breaking:** Replaced `Value::Name(Vec<u8>)` with `Value::Name(Name)` and `Str::Bytes(Vec<u8>)` / `Str::Encrypted(Vec<u8>)` with `SmallBytes`; removed `Str::Text(String)` and implicit `From<&str>` / `From<String>` -> `Value::Name` conversions on `Object` (use `Object::name` or `Object::text`).
- **Breaking:** Separated ISO 32000-2 `text` (`Encoding::{Utf16Be, Utf8, PdfDoc}`) from `text::pdfium`; `serialize::name` now returns `Result<(), serialize::Error>` to reject NUL bytes; `Encrypt::clear_key` replaced by predicate `Encrypt::is_clear_key(&self, id, dict, key) -> bool`.
- **Breaking:** `Parser::header(&mut self, sink)` now accepts a fault sink, and `Parser::body` / `Parser::stream` consume trailing `endobj` when present (or emit `Fault::NoEndobj`); `parse::Error` is boxed (`8` bytes) and dereferences to `parse::ErrorKind`.
- Converted `Object` (`Drop`, `Clone`, `PartialEq`, `eq_value`), `Parser`, and `serialize` container walks to iterative heap-stack state machines for constant call-stack usage on deeply nested trees.
- `Dict` lookup and duplicate detection use a 64-entry inline `u64` bitset and deterministic index sorting above 64 entries; `Dict::set` updates the last matching entry in place and removes shadowed earlier duplicates.
- `WithStreams::decoded` now enforces the caller's byte `limit` (`Why::Limit`), and `Debug` redacts `Str::Encrypted` and `Data::Bytes` payloads alongside capped `Fault` byte previews.

### Fixed
- `StreamHead::scanned()` now skips false `endstream` matches followed by regular characters (such as `endstreamX`), records window-boundary touches when `endstream` ends at a non-final window cut, and strips at most one preceding stream-framing EOL (`\r\n`, `\n`, or `\r`).
- `text::decode` now maps all 24 undefined bytes in ISO 32000-2 Table D.2 (`0x00..=0x08`, `0x0B..=0x0C`, `0x0E..=0x17`, `0x7F..=0x80`, `0xAD`) to `U+FFFD` with `Quirk::Undefined(u8)`, restricts `Object::text` verbatim `PDFDocEncoding` selection to defined identity code points, and strips `0x1B` language escapes at the byte/unit level prior to UTF-8/UTF-16 decoding.
- `serialize::real` no longer silently clamps finite `f64` values to `3.4e38` or flushes subnormals like `1e-45f32` to `0.0`, instead round-tripping all finite reals whose fixed-point token fits within 255 bytes.

## 0.1.0-alpha.2 (2026-11-07)

- **Rust 1.89.** Raised MSRV from 1.85 to 1.89 to match the rest of the workspace.

## 0.1.0-alpha.1 (2026-11-06)

Initial alpha release. Pre-1.0 alpha releases may make breaking API changes; depend on exact versions (`=0.1.0-alpha.1`).

- **The objects.** Owned `Object` pairing a `Value` (null, bool, integer, real, name, string with optional `Form`, array, `Dict`, boxed `Stream`, or `Ref`) with an optional source `Span` in the file or an object stream. `Dict` preserves entry order, key spans, and duplicate keys (`get` returns the last entry; `duplicates` yields shadowed ones), with a `dict!` constructor macro.
- **Ids.** 8-byte `ObjectId { number: u32, generation: u16 }` with `Display` (`12 0`), ordering, hashing, and `(u32, u16)` conversions.
- **Editing.** Mutating accessors (`value_mut`, `as_dict_mut`, `as_array_mut`, `as_stream_mut`) clear `Span`s along the edited path so writers know what changed, while `as_written_mut` mutates in place without clearing spans (for example, decrypting strings). Includes typed scalar/container accessors, `From` conversions, and `Dict` mutation helpers (`set`, `push`, `remove`, `remove_duplicates`).
- **Each walk is a type of its own.** Named, fused iterator types for dictionary and reference traversals (`Iter`, `IterMut`, `LastEntries`, `Duplicates`, `References`).
- **A stream's data.** `Stream` pairs a `Dict` with `Data` (`At(Span)`, `Bytes(Vec<u8>)`, or `Unplaced`) and updates `/Length` via `Stream::new` / `set_bytes`.
- **A stream's data, decoded.** `Streams` trait (`decoded(id, limit) -> Result<Cow<[u8]>, Unreadable>`) and `WithStreams` adapter pairing an object resolver with a stream-decoding closure, reporting `Why` (`Damaged`, `Limit`, `Capped`, `Locked`, `Filter`, `Detached`, `Source`).
- **Faults, told with where they are.** `fault::Sink<F>` and `fault::Place` for zero-cost-when-disabled (`()`) or collected (`Vec<(Place, F)>`) non-fatal diagnostics across domain readers.
- **References resolved.** `Resolve<'o>` trait (`resolve` and `try_resolve`) decoupling domain crates from file I/O.
- **References walked.** `Object::references` iterator over all indirect references in source order using an explicit heap stack.
- **References renumbered, and places dropped.** `Object::renumber` (clearing spans along changed paths), `Object::unplace` (stripping all spans), and on-the-fly `serialize::Renumber` support during serialization.
- **Text strings.** `text::decode` and `text::decode_with` for UTF-16BE, UTF-8, and `PDFDocEncoding` text strings with `Quirk` reporting.
- **Text strings written.** `text::encode`, `text::encode_as`, and `Object::text` for lossless PDF text string encoding with `Unencodable` error reporting.
- **Versions.** `Version` (`major`, `minor`) parsing, formatting, and ordering for PDF headers and `/Version` catalog entries.
- **The parser.** `parse::Parser` over `pdf-tokenizer` supporting direct values, indirect objects (`header`, `body`, `stream`), streaming file windows (`Error::Short`), object streams (`in_stream`), nesting/value `Limits`, redacted `Debug`, and `parse::Sink` diagnostics.
- **ISO 32000-2's rules, or PDFium's.** `Parser::rules(Rules)` selecting strict ISO 32000-2 or PDFium (`chromium/8125`) lexical and number/string/name recovery rules while reporting departures via `Fault`.
- **A real too small for 64 bits is said.** `Fault::RealUnderflow` reported under ISO rules when a non-zero real underflows `f64` to `0.0`.
- **0x80 and 0xFF read as white-space are said.** `Fault::HighByte` reported once per `0x80`/`0xFF` treated as whitespace under PDFium rules when the sink wants faults.
- **What PDFium's rules read and don't tell, said.** Documented header and `f64` vs. `f32` rounding differences between `Parser` and PDFium.
- **The writer.** `serialize` functions (`write`, `indirect`, `indirect_stream`, and primitives) writing canonical PDF syntax and rejecting unserializable values (`serialize::Error`) without partial output.
- **Encrypted as it's written.** `serialize::Encrypt` hook (`indirect_with`, `indirect_stream_with`) for streaming string and stream encryption with zero overhead on unencrypted writes (`()`).
- **pdf-tokenizer, re-exported whole.** Re-exported `pdf_objects::pdf_tokenizer` so callers share the exact `Rules` version without duplicate dependency edges.
- **Exhaustive on purpose, or not, and said.** Documented exhaustiveness policy across closed PDF syntax enums vs. `#[non_exhaustive]` diagnostic, limit, and configuration types.
- **What's dropped by mistake is marked, and what can be `const` is.** Applied `#[must_use]`, `const fn`, and `# Errors` documentation consistently across constructors, accessors, and fallible APIs.
- **What a call's values take in memory, said.** Documented and tested worst-case per-value memory bounds under `Limits::values` on 64-bit targets.
- **Benchmarks.** Criterion suite (`cargo bench -p pdf-objects`) covering parsing, serialization, dictionary lookups, reference walks, and text codecs.
- **The package, checked before a publish.** Verified `cargo publish --dry-run` file allowlist, MSRV, and repository license URLs in CI.
- **From pdf-lint.** Extracted and expanded from `pdf-lint 0.1.0`'s internal `object`, `text`, `version`, and parser modules, replacing `Object::string` with `Object::text` and `Object::bytes`.
- **Tests.** Unit, property/fuzz (`just fuzz objects`), and recorded PDFium (`chromium/8125`) test suites covering parser windows, limits, rules, round-trip serialization, encryption, renumbering, and text codecs.
