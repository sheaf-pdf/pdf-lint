# pdf-lint & pdfs

A workspace of Rust crates for reading, writing, and validating PDF files from raw bytes—with zero panics on untrusted input, exact byte-offset tracking, and both strict ISO 32000-2 and viewer-compatible (PDFium) reading modes.

Two main entry points sit at the top of the workspace:

- **[`pdf-lint`](crates/pdf-lint)** — A CLI and library that validates PDFs against the [Arlington PDF Model](https://github.com/pdf-association/arlington-pdf-model) (the PDF Association's machine-readable grammar of ISO 32000-2) plus ISO 32000-2 rules for content streams, page trees, name/number trees, XMP metadata, color spaces, fonts, and linearization. Every finding points to the exact bytes responsible.
- **[`pdfs`](crates/pdfs)** — A single, battery-included library dependency for reading, inspecting, extracting text from, creating, editing, encrypting, and linearizing PDFs, while re-exporting all underlying crates at matching versions.

## Quickstart

### Validating PDFs (`pdf-lint`)

Most real-world PDFs already contain minor spec violations, which creates noise when testing a tool that edits existing files. `pdf-lint` can restrict validation to only what your tool wrote:

```sh
pdf-lint out.pdf                     # check every object in the file
pdf-lint --last-update out.pdf       # check only objects touched in the latest incremental update
pdf-lint --baseline in.pdf out.pdf   # report only findings not already present in in.pdf
pdf-lint --hold objects.txt out.pdf  # check only the object IDs listed by your writer
```

See [crates/pdf-lint/README.md](crates/pdf-lint/README.md) for rule categories, CLI flags, and JSON output format.

### Reading & writing PDFs in Rust (`pdfs`)

Start with [`pdfs`](crates/pdfs) unless you only need a single low-level layer (such as zero-alloc tokenizing or standalone stream filters):

```rust
use pdfs::prelude::*;

// Create a one-page PDF in memory
let mut doc = Document::new();
let page = doc.add_page(Rect::LETTER);
page.begin_text();
page.font(standard_font(Std14::Helvetica), 24.0);
page.text_move(72.0, 700.0);
page.show(&Std14::Helvetica.codes("Hello, world").unwrap());
page.end_text();
let bytes = doc.to_bytes().unwrap();

// Open and read text back
let pdf = pdfs::from_bytes(bytes).unwrap();
let pages = pdf.pages().unwrap();
assert_eq!(pages.get(0).unwrap().text().unwrap().text(), "Hello, world");
```

See [crates/pdfs/README.md](crates/pdfs/README.md) for encryption, linearization, incremental updates, and feature flags.

## Workspace Crates

| Crate | Role |
|---|---|
| **[pdfs](crates/pdfs)** | High-level facade for opening, reading text/fonts/metadata, creating, editing, encrypting, and linearizing PDFs; re-exports the library family below. |
| **[pdf-lint](crates/pdf-lint)** | PDF validator CLI and library built on the Arlington PDF Model and ISO 32000-2 checks, with byte-accurate spans and baseline/incremental filtering. |
| **[pdf-arlington](crates/pdf-arlington)** | Embedded Arlington PDF Model TSV grammar and predicate evaluator over `pdf-objects`. |
| **[pdf-tokenizer](crates/pdf-tokenizer)** | Lossless, zero-copy, `no_std` / zero-alloc PDF lexer supporting both ISO 32000-2 and PDFium lexical rules. |
| **[pdf-filters](crates/pdf-filters)** | Standard PDF stream filters (`FlateDecode`, `LZWDecode`, `ASCIIHexDecode`, `ASCII85Decode`, `RunLengthDecode` + TIFF/PNG predictors) with fault reporting and Flate encoding (`miniz_oxide` or `zlib-rs`). |
| **[pdf-crypt](crates/pdf-crypt)** | Standard PDF security handler (revisions 2–6: RC4, AES-128, AES-256) for password preparation, decryption, new AES-256 encryption, and permission flags. |
| **[pdf-objects](crates/pdf-objects)** | Owned PDF object values with byte spans, duplicate-preserving dictionaries, text string codecs, parser/serializer, and the cross-crate `Resolve`, `Streams`, and `fault::Sink` traits. |
| **[pdf-structure](crates/pdf-structure)** | PDF file structure reader and writer: cross-reference tables/streams, object streams, lazy positional reads, damaged-file recovery, full saves, and incremental updates. |
| **[pdf-content-streams](crates/pdf-content-streams)** | Content stream reader, typed Annex A operator model, `q`/`Q` and marked-content analysis, safe byte-range splicing, and deterministic `ContentWriter`. |
| **[pdf-semantics](crates/pdf-semantics)** | High-level PDF data structures: dates, name/number trees, page trees (read via ISO or PDFium rules, written balanced), `/Info` dictionaries, and XMP conformance claims. |
| **[pdf-color](crates/pdf-color)** | PDF color space parser, component counting (ISO 32000-2 and PDFium rules), colorant classification, and `/ColorSpace` resource lookup. |
| **[pdf-fonts](crates/pdf-fonts)** | PDF font reader (character codes, CIDs, widths, encodings, CMaps, `/ToUnicode`, embedded font programs via `skrifa`), Standard 14 font metrics/encoding, and page text extraction. |
| **[pdf-gfx](crates/pdf-gfx)** | PDF 2D geometry (`PageGeom`, `Matrix::onto`), bounded content-stream and Form XObject interpreter, position-independent text re-encoder (`encode`), spatial `TextPage` / search, and vector/text `Canvas` (with multiline `text_box`). |
| **[pdf-linearize](crates/pdf-linearize)** | Model-agnostic engine and `pdf-structure` adapter for writing and validating linearized ("Fast Web View", Annex F) PDFs and hint tables. |
| **[pdf-fixtures](crates/pdf-fixtures)** | *Internal (unpublished).* Test PDF builders and `qpdf` oracle runner shared across workspace test suites. |
| **[pdf-linearize-lopdf](crates/pdf-linearize-lopdf)** | *Internal (unpublished).* Reference adapter demonstrating `pdf-linearize` on `lopdf` documents. |

For an architectural map of how these crates connect and conventions for contributors/agents, see [AGENTS.md](AGENTS.md).

## Development

Tasks are defined in the [justfile](justfile) and run with [just](https://github.com/casey/just) and [cargo-nextest](https://nexte.st):

```sh
just              # list all available recipes
just test-unit    # run workspace unit and integration tests via cargo-nextest
just test         # run unit tests (nextest) and doctests across feature combinations
just clippy       # run clippy across workspace feature combinations
just doc          # build rustdoc with -D warnings across feature combinations
just ci           # run the full local CI suite (all checks except cargo-semver-checks)
```

### Oracle & Cross-Target Checks

- **PDFium oracle (`just test-pdfium`):** Requires Python 3. Downloads a pinned PDFium build (`chromium/8152` on Linux x86_64, or uses `PDF_LINT_PDFIUM`) and verifies recorded test expectations across crates that emulate PDFium behavior. See [oracle/README.md](oracle/README.md).
- **qpdf oracle (`just test-qpdf`):** Downloads a pinned `qpdf` binary on Linux x86_64 (or uses `PDF_LINT_QPDF`) to cross-check file reading, writing, encryption, and linearization.
- **Full `just ci` prerequisites:** Requires `cargo-nextest`, Rust **1.92** (workspace MSRV), `nightly` with `rust-src` (for `pdf-tokenizer`'s `core`-only build), and targets `thumbv6m-none-eabi`, `x86_64-pc-windows-gnu`, and `wasm32-unknown-unknown`. Note that `*-package` recipes run `cargo publish --dry-run` and require network access.
- **Semver checks (`just <crate>-semver`):** Uses [`cargo-semver-checks`](https://github.com/obi1kenobi/cargo-semver-checks) to check published crates against crates.io.
- **Fuzzing (`just fuzz <target> [seconds] [features]`):** Runs [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) on nightly (`tokenize`, `filters`, `crypt`, `objects`, `structure`, `write`, `content`, `semantics`, `color`, `fonts`, `embed`, `gfx`, `linearize`, `pdfs`, `arlington`, `check`), e.g. `just fuzz content 300 simd`.

## License

- `pdf-tokenizer`, `pdf-filters`, `pdf-crypt`, `pdf-objects`, `pdf-structure`, `pdf-content-streams`, `pdf-semantics`, `pdf-color`, `pdf-fonts`, `pdf-gfx`, `pdf-linearize`, `pdfs`, `pdf-fixtures`, and `pdf-linearize-lopdf` are dual-licensed under **Apache-2.0** ([LICENSE-APACHE](LICENSE-APACHE)) or **MIT** ([LICENSE-MIT](LICENSE-MIT)), at your option.
- `pdf-fonts` bundles Adobe Core 14 metrics, glyph lists, and code adapted from FreeType under their respective licenses, including [`FTL.TXT`](crates/pdf-fonts/FTL.TXT); see [crates/pdf-fonts/README.md](crates/pdf-fonts/README.md#license) for attribution requirements.
- `pdf-lint` and `pdf-arlington` (along with the embedded Arlington PDF Model) are licensed under **Apache-2.0** ([LICENSE-APACHE](LICENSE-APACHE)).
