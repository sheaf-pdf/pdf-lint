# pdf-linearize

Plan, assemble, and validate linearized ("Fast Web View") PDF files (ISO 32000-2, Annex F).

`pdf-linearize` separates a pure, model-independent layout and validation engine (built only on [pdf-objects](https://crates.io/crates/pdf-objects)) from a ready-to-use `family` adapter (enabled by default) that linearizes and validates files directly via [pdf-structure](https://crates.io/crates/pdf-structure) and [pdf-semantics](https://crates.io/crates/pdf-semantics).

## Core Concepts

- **Turnkey linearization & checking (`family` feature, on by default):**
  - `family::linearize` and `family::linearize_bytes` rewrite a `pdf-structure` `Document` or raw PDF bytes into a valid linearized PDF (supporting cross-reference tables, cross-reference streams, object streams, and preserved or new encryption via `Encryptor`).
  - `family::check` and `family::check_with_reading` extract structural facts from a `Reading` and validate linearization parameters, cross-reference offsets, page ordering, and hint tables.
- **Model-independent engine (no I/O or document model required):**
  - **Plan (`Graph` -> `plan` -> `Plan`):** Describe object references, pages, page-tree nodes, and open-time objects as a `Graph` to compute Annex F file ordering, renumbered IDs, and object-stream groupings (`plan::page_uses` and `plan::group` can also pack non-linearized files by page).
  - **Assemble (`assemble` -> `Layout`):** Serialize your planned object bodies however your model stores them, pass their byte lengths to `assemble`, and iterate `Layout::pieces` to interleave padded linearization dictionaries, cross-reference sections, hint streams, and object bodies.
  - **Hint tables (`hint`):** Encode and decode Annex F.4 bit-packed hint tables (`hint::tables`).
  - **Validate (`check`):** Validate `check::Facts` from any PDF reader into a `check::Verdict` and emit `check::Finding` diagnostics graded as `Error`, `Departure`, or `Unjudged` (when a reader cap, missing password, or unsupported filter prevented inspection). Passing `()` as the sink skips graph-planning checks that do not affect the verdict.
- **Resource bounds & conformance:** `Limits` bounds object counts, pages, shared-object references, and hint-table sizes without panicking on hostile input. Output is tested against `qpdf --check-linearization`, and validation is tested against PDFium's availability API as of chromium/8125.

## Quickstart

Plan two pages sharing a font and assemble the linearized layout around their serialized bodies:

```rust
use std::convert::Infallible;

use pdf_linearize::layout::{Encoded, Options, Piece, Stream};
use pdf_linearize::pdf_objects::ObjectId;
use pdf_linearize::pdf_objects::version::Version;
use pdf_linearize::plan::{Kind, Page, Part, What};
use pdf_linearize::{Graph, Limits, assemble, hint, plan};

// A catalog (1), a page tree node (2), two pages (3, 4), a content stream
// each (5, 6), and a font both pages use (7).
let id = |number| ObjectId::new(number, 0);
let mut graph = Graph::new(id(1), Kind::Table);
graph.object(id(1), [id(2)]);
graph.object(id(2), [id(3), id(4)]);
graph.object(id(3), [id(2), id(5), id(7)]);
graph.object(id(4), [id(2), id(6), id(7)]);
for leaf in [id(5), id(6), id(7)] {
    graph.object(leaf, []);
}
graph.page(Page::new(id(3)));
graph.page(Page::new(id(4)));
graph.tree_node(id(2));
let plan = plan(&graph, &Limits::DEFAULT).unwrap();

// The font goes with the first page, though the second uses it too. The
// second page is numbered from 1, and lists the font's shared entry.
let first = &plan.writes()[plan.part_writes(Part::FirstPage)];
let first: Vec<What> = first.iter().map(|write| write.what).collect();
assert_eq!(first, [id(3), id(5), id(7)].map(What::Object));
assert_eq!(plan.number(id(4)), Some(id(1)));
assert_eq!(plan.shared(1), [1]);

// A body of 40 bytes for each write, however the caller writes it, and
// the bytes around them, the layout's own streams left as they are.
let lengths = vec![40; plan.writes().len()];
let plain = |stream: Stream| {
    Ok::<_, Infallible>(Encoded::plain(stream.data.to_vec()))
};
let options = Options::new(Version::new(1, 7));
let layout = assemble(&plan, &lengths, &options, plain).unwrap();

// Its pieces are the whole file, in order.
let written: u64 = (layout.pieces())
    .map(|piece| match piece {
        Piece::Bytes(bytes) => bytes.len() as u64,
        Piece::Body(at) => lengths[at],
    })
    .sum();
assert_eq!(written, layout.len());

// Its hint tables read back: the second page lists the font's entry.
let (data, keys) = (layout.hint_data(), layout.hint_keys());
let tables = hint::tables(data, keys, 2, &Limits::DEFAULT).unwrap();
assert_eq!(tables.pages.pages[1].shared, [(1, 0)]);
```

## Cargo Features

- **`family`** *(default)*: Enables the `family` module over `pdf-structure` and `pdf-semantics`. Disable default features to use the standalone layout/validation engine with custom PDF object models.
- **`zlib-rs`** *(optional)*: Enables the `zlib-rs` Flate backend through `pdf-structure`.
- **`tracing`** *(optional)*: Emits `tracing` spans (numeric metadata only) over planning, assembly, validation, and `family` reads/writes.
- **`insecure-test-writer`** *(optional)*: Deterministic encryption keys and IVs for fuzzing (`pdf-structure/insecure-test-writer`). Never enable in production builds.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
