# Changelog

## 0.1.0-alpha.1 (unreleased)

First release. Until `0.1.0` stabilizes, alpha releases may include breaking API changes documented here; depend on the exact version (`=0.1.0-alpha.1`).

### Added

- `Reading` and `ByteSource` (`FileBytes`, `Identity`, `Prefix`): thread-safe on-demand reading from in-memory bytes or windowed file reads, parsing headers, cross-reference tables and streams along `/Prev` chains, object streams, and revision history (`Options::section`, `Reading::prefix`, `Reading::sections`, `Reading::revisions`); objects read without keeping them (`Reading::object_unkept`, with the record a copy is checked by, and `Reading::object_unrecorded`, without one, so that reading a stream's dictionary doesn't hash its data).
- `Document`, `read::document`, and `WithReading`: whole-file parsing and editable in-memory document model (`add`, `set`, `get_mut`, `remove`, `dereference`, `reachable`, `prune`), plus `Reading::adopt` to re-attach an open `Reading` to a `Document` read whole.
- `Source`, `Resolve`, and `Streams`: uniform object resolution and bounded stream decoding across `Reading`, `Document`, and `WithReading` (`decoded_within`, `decoded_standard_within`, `Streams::decoded`, `Streams::decoded_standard`), alongside `read::filters` and `read::filter_chain` for inspecting `/Filter` and `/DecodeParms` pipelines.
- Damaged-file recovery (`Options::recovery`): `Recovery::Verified` (verified offset-shift repair and fallback full-file scan) and `Recovery::Pdfium` (PDFium-compatible cross-reference and fallback scan rules as of chromium/8152).
- Decryption (`Options::passwords`, `Reading::unlock`, `Encryption`, `read::encryption`): Standard Security Handler revisions 2–6 via `pdf-crypt`, handling `/StmF`, `/StrF`, `/EFF`, per-stream `/Crypt` filters, and `/EncryptMetadata`.
- `Fault`, `Limits`, and `Options::faults`: typed structural and syntax diagnostics with byte/object locations reported via `pdf-objects`' `fault::Sink` (`Faults::Kept` or zero-cost `Faults::Off`), paired with caps on stream sizes, per-reading decode budgets, `/Prev` depth, filter chains, and object nesting.
- Full and incremental writing (`write::document`, `write::full`, `write::update`, `write::Base`): cross-reference tables or compressed cross-reference and object streams (`Options::pack`), reference renumbering (`Options::renumber`), SHA-256 `/ID` calculation (`write::file_id`), and verified raw byte copying (`Body::Read`, `Record`) with in-place header/reference splicing.
- Streaming writers (`write::Writer`, `write::UpdateWriter`): stream whole files and incremental updates one `Body` at a time to any `std::io::Write` without `Seek` and with single-body bounded scratch memory, matching `write::full` and `write::update` byte-for-byte on unencrypted inputs.
- Encrypted writing (`write::Encryptor`, `write::Encrypting`, `write::encryption`): preserves an open reading's encryption (`Encryptor::of`), creates new revision-6 AES-256 encryption (`Encryptor::create`), promotes direct `/Info` dictionaries to encrypted indirect objects, draws OS-random AES IVs, and prevents RC4 keystream reuse.
- Layout building blocks (`write::Parts`, `write::Section`, `write::container`, `Encryptor::laid_out`, `write::Deflate`) for custom writers such as `pdf-linearize`.
- Optional `zlib-rs` (alternate Flate backend alongside default `miniz_oxide`) and `tracing` (numeric phase spans) Cargo features.

### Notes

- Re-exports `pdf-objects`, `pdf-filters`, and `pdf-crypt` at `pdf_objects`, `pdf_filters`, and `pdf_crypt`. Requires Rust 1.92.
- Never panics on untrusted input, and `Debug` implementations and `tracing` spans never include file plaintext, stream bytes, keys, or passwords.
- Public enums and configuration/metadata structs are `#[non_exhaustive]` so new variants and fields can be added without breaking callers.
