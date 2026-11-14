# pdf-structure

PDF file structure reader, editable in-memory document model, and full or incremental writer (ISO 32000-2, §7.5–7.6).

`pdf-structure` parses PDF headers, cross-reference tables and streams (along `/Prev` chains), object streams (via [pdf-filters](https://crates.io/crates/pdf-filters)), and encrypted files (via [pdf-crypt](https://crates.io/crates/pdf-crypt)) into [pdf-objects](https://crates.io/crates/pdf-objects) values. Every value preserves its byte span, and every structural repair or syntax anomaly is recorded as a typed `Fault` with its location.

## Core Concepts

- **On-demand or whole-file reading:**
  - `Reading::open` reads only the header, cross-reference chain, and security handler up front, then parses indirect objects and decodes object streams lazily on demand from any thread. Input bytes can live in memory (`&[u8]`, `Vec<u8>`, `Arc<[u8]>`) or be read by file offset in growing windows (`FileBytes`, with `Identity` tracking whether an open file changed on disk).
  - `read::document` (or `Reading::to_document` / `Reading::into_document`) materializes all indirect objects into an owned `Document`.
  - `WithReading` pairs an owned `Document` with its `Reading` so callers can mutate objects while still decoding raw stream payloads on demand.
  - `Source` (plus `pdf-objects`' `Resolve` and `Streams`) provides a uniform read interface across `Reading`, `Document`, and `WithReading`.
- **In-memory document editing (`Document`):** Build a new document with `Document::new` or edit a parsed one (`add`, `set`, `get_mut`, `remove`, `catalog_mut`, `trailer_mut`), follow reference chains (`dereference`), walk objects reachable from the trailer (`reachable`), and strip unreachable objects (`prune`).
- **Damaged-file recovery & PDFium compatibility (`Options::recovery`):**
  - `Recovery::Verified` (default) recovers shifted cross-reference sections/entries when the file proves the target objects match, and falls back to full-file scanning when cross-reference sections cannot be read.
  - `Recovery::Pdfium` mirrors PDFium's cross-reference and full-file fallback scan rules as of chromium/8125 (verified against recorded cases in `tests/pdfium`).
- **Encryption & stream decoding:** Pass candidate passwords via `Options::passwords` (or unlock later with `Reading::unlock`) to transparently decrypt strings, object streams, and stream payloads (`Reading::data`, `Reading::decoded_within`, `Streams::decoded`) across Standard Security Handler revisions 2–6. `Document::encryption` reports unlock status, permissions, and cipher metadata.
- **Revisions:** Inspect incremental-update history via `revisions()`, read a file as of an earlier cross-reference section (`Options::section`), or open an earlier revision's byte prefix as a standalone file (`Reading::prefix`).
- **Full & incremental writing (`write`):**
  - Write a complete file (`write::document`, `write::full`) with cross-reference tables or compressed cross-reference/object streams (`Options::pack`), optional reference renumbering (`Options::renumber`), and preserved or newly created AES-256 encryption (`Encryptor::of`, `Encryptor::create`).
  - Append incremental updates (`write::update`, `Base::of`) carrying forward only modified objects while verifying unchanged source files (`Base::holds`).
  - Unmodified objects (`Body::Read`) copy verified raw bytes directly when safe, avoiding unnecessary re-serialization, and RC4 writes prevent keystream reuse across generations unless explicitly overridden.
  - Low-level layout primitives (`write::Parts`, `write::Section`, `write::container`, `Encryptor::laid_out`) let custom writers (such as `pdf-linearize`) lay out files directly.
- **Resource limits & diagnostics:** `Limits` caps decoded stream sizes, total per-reading decode budgets, `/Prev` chain depth, filter counts, and object parser limits. Pass `Faults::Off` in `Options::faults` to skip fault collection when only parsed values are needed.

## Quickstart

### Read a file and inspect objects & faults

```rust
use pdf_structure::pdf_objects::{Object, ObjectId};
use pdf_structure::{Fault, Options, Source, read};

let mut file = b"%PDF-1.7\n".to_vec();
let catalog = file.len();
file.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
let pages = file.len();
file.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
let xref = file.len();
let table = format!("0000000000 65535 f \n{catalog:010} 00000 n \n{pages:010} 00000 n \n");
let tail = format!("trailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n");
file.extend_from_slice(format!("xref\n0 3\n{table}{tail}").as_bytes());
file.extend_from_slice(b"% more\n");

let doc = read::document(&file, &Options::DEFAULT);
assert_eq!(doc.trailer().unwritten, None);

let pages = doc.get(ObjectId::new(2, 0)).unwrap();
let count = pages.as_dict().unwrap().get(b"Count").and_then(Object::as_integer);
assert_eq!(count, Some(0));
let at = pages.at.unwrap();
assert_eq!(&file[at.start..at.end], b"<< /Type /Pages /Kids [] /Count 0 >>");

let [(place, fault)] = doc.faults() else { panic!() };
assert_eq!(*fault, Fault::AfterEof { bytes: 8 });
assert_eq!(place.at.map(|at| &file[at.start..at.end]), Some(&b"\n% more\n"[..]));
```

### Read lazily across threads or from disk

```rust
use std::sync::Arc;

use pdf_structure::{Options, Origin, Reading};
use pdf_structure::pdf_objects::ObjectId;

let file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n\
    xref\n0 2\n0000000000 65535 f \n0000000009 00000 n \n\
    trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n45\n%%EOF\n";
let bytes: Arc<[u8]> = Arc::from(&file[..]);
let reading = Reading::open(bytes, &Options::DEFAULT).unwrap();

std::thread::scope(|scope| {
    scope.spawn(|| assert!(reading.get(ObjectId::new(1, 0)).is_ok()));
});
let record = reading.record(ObjectId::new(1, 0)).unwrap();
assert_eq!(record.origin, Origin::File);
assert_eq!(reading.to_document().unwrap(), pdf_structure::read::document(file, &Options::DEFAULT));
```

### Build, prune, and write whole files or incremental updates

```rust
use pdf_structure::pdf_objects::{Object, ObjectId, dict};
use pdf_structure::write::{self, Base, Pack, ReadFrom};
use pdf_structure::{Options, Reading, Source, XrefKind};

let file = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n\
    xref\n0 2\n0000000000 65535 f \n0000000009 00000 n \n\
    trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n45\n%%EOF\n";
let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
let from: &dyn ReadFrom = &reading;
let mut doc = reading.to_document().unwrap();
doc.catalog_mut().unwrap().set("Lang", Object::text("en"));

// Write a compressed full file with object streams and a cross-reference stream.
let mut options = write::Options::DEFAULT;
options.kind = XrefKind::Stream;
options.pack = Some(Pack::DEFAULT);
let mut whole = Vec::new();
write::document(&mut whole, &doc, None, Some(from), &options).unwrap();
assert!(Reading::open(&whole[..], &Options::DEFAULT).unwrap().faults().is_empty());

// Or append an incremental update containing only changed objects.
let base = Base::of(&reading).unwrap();
let mut updated = file.to_vec();
let written = write::update(
    &mut updated,
    &base,
    write::bodies(&doc),
    None,
    Some(from),
    &write::Options::DEFAULT,
)
.unwrap()
.unwrap();
assert!(written.section.lists(1));
assert_eq!(Reading::open(&updated[..], &Options::DEFAULT).unwrap().facts().sections, 2);
```

## Cargo Features

- **`zlib-rs`** *(optional)*: Enables the `zlib-rs` Flate backend in `pdf-filters` alongside the default `miniz_oxide` backend (`Options::flate`, `write::Deflate`).
- **`tracing`** *(optional)*: Emits `tracing` spans (numeric metadata only—never keys, passwords, or plaintext) around open, scan, object-stream decode, and write phases.
- **`insecure-test-writer`** *(optional, hidden)*: Deterministic seeded encryption keys and IVs for workspace tests and fuzzing. Never enable in production builds.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
