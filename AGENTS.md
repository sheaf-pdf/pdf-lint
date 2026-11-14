# Architecture & Contributor Guide (`AGENTS.md`)

This document gives developers and coding agents a fast mental model of the workspace architecture, cross-crate traits, design conventions, and test invariants.

## 1. Which Crate Should You Touch?

- **Building an application or high-level workflow?** Start in **[`pdfs`](crates/pdfs)**. It wraps the entire library family behind ergonomic `Pdf`, `Page`, and `Document` types and re-exports every underlying crate (`pdfs::pdf_structure`, `pdfs::pdf_objects`, etc.).
- **Working on PDF validation / CLI reporting?** Look at **[`pdf-lint`](crates/pdf-lint)** (rule orchestration, baseline/incremental diffing, CLI/JSON output) and **[`pdf-arlington`](crates/pdf-arlington)** (TSV grammar loader and predicate evaluator).
- **Working on a specific PDF subsystem?** Crates are layered strictly bottom-up with no circular dependencies:

```text
                    ┌──────────────┐      ┌──────────────┐
                    │     pdfs     │      │   pdf-lint   │
                    └──────┬───────┘      └──────┬───────┘
                           │                     │
        ┌──────────────────┼─────────────────────┼──────────────────┐
        ▼                  ▼                     ▼                  ▼
┌───────────────┐  ┌───────────────┐     ┌───────────────┐  ┌───────────────┐
│ pdf-linearize │  │   pdf-fonts   │     │ pdf-semantics │  │ pdf-arlington │
└───────┬───────┘  └───────┬───────┘     └───────┬───────┘  └───────┬───────┘
        │                  ▼                     │                  │
        │        ┌───────────────────┐           │                  │
        │        │pdf-content-streams│           │                  │
        │        └─────────┬─────────┘           │                  │
        ▼                  ▼                     │                  │
┌───────────────┐  ┌───────────────┐             │                  │
│ pdf-structure │  │   pdf-color   │             │                  │
└──┬────┬────┬──┘  └───────┬───────┘             │                  │
   │    │    └────────┐    └──────────┬──────────┴──────────────────┘
   ▼    ▼             ▼               ▼
┌───────────┐  ┌─────────────┐  ┌───────────┐
│ pdf-crypt │  │ pdf-filters │  │pdf-objects│
└───────────┘  └─────────────┘  └─────┬─────┘
                                      ▼
                               ┌─────────────┐
                               │pdf-tokenizer│
                               └─────────────┘
```

*(Internal workspace-only crates: `crates/pdf-fixtures` provides test PDF builders and `qpdf` oracle helpers; `crates/pdf-linearize-lopdf` tests `pdf-linearize`'s model-agnostic engine against `lopdf`.)*

## 2. Core Cross-Crate Concepts

### Zero-I/O Domain Crates (`Resolve` and `Streams`)
Only `pdf-structure` (and `pdfs` on top of it) reads or writes files. Domain crates—`pdf-color`, `pdf-semantics`, `pdf-content-streams`, `pdf-fonts`, and `pdf-arlington`—operate over two traits defined in [`pdf-objects`](crates/pdf-objects):
- **`Resolve`**: Resolves an indirect `ObjectId` to `Option<&Object>` (or `try_resolve` -> `Result<Option<&Object>, Unreadable>` to distinguish missing objects from locked, capped, damaged, or I/O-failed reads). Implemented for `&pdf_structure::Reading`, `&pdf_structure::Document`, `&pdf_structure::WithReading`, and any `Fn(ObjectId) -> Option<&Object>` closure.
- **`Streams`**: Extends `Resolve` with `decoded(id, limit)` to return decrypted, filter-decoded stream bytes up to `limit`. Use `pdf_objects::WithStreams` to pair a closure resolver with a stream-decoding function in tests or custom callers.

### Strict ISO 32000-2 vs. Viewer (`PDFium`) Reading Modes
Real-world PDFs often violate ISO 32000-2 while rendering fine in viewers. Several crates support both policies explicitly:
- **ISO mode (`Rules::Iso`, `Recovery::Verified`)**: Follows ISO 32000-2 syntax and structure rules; used by checkers (`pdf-lint`).
- **PDFium mode (`Rules::Pdfium`, `Recovery::Pdfium`)**: Emulates PDFium's forgiving behavior (pinned to **`chromium/8125`** and verified against recorded outputs in `tests/pdfium/`); used by viewer-style callers (`pdfs` defaults).

### Non-Fatal `Fault`s vs. Hard `Error`s (and Zero Panics)
- **No panics on untrusted input:** Every parser, decoder, tree walker, and font/content reader enforces explicit resource caps (`Limits::DEFAULT`) and returns typed errors when caps or budgets are exceeded.
- **Fault sinks (`pdf_objects::fault::Sink<F>`):** Recoverable spec departures (e.g., bad stream `/Length`, duplicate dict keys, malformed dates, damaged filter checksums) are reported with byte/object locations (`Place` / `Span`) to a `Sink`. Passing `&mut ()` disables fault collection and skips fault-only work at zero cost while producing the exact same parsed result.

### Byte Spans & Copy-on-Write Serialization
- `pdf_objects::Object` pairs a `Value` with an optional byte `Span`, and `Dict` preserves original entry order and duplicate keys.
- Mutating an `Object` via `value_mut` or `as_*_mut` clears the `Span` on the edited node and its ancestors. When `pdf-structure` saves an edited document, untouched objects with intact `Record` hashes can be copied directly from the source bytes, while modified objects are serialized fresh.

### Security & `Debug` / `tracing` Hygiene
- `pdf-crypt` wipes keys, password buffers, and cipher schedules on drop via `zeroize`.
- Public `Debug` implementations and optional `tracing` spans across the workspace **never** include file plaintext, stream bytes, keys, or passwords (only counts, offsets, and IDs). Unit tests actively assert this.

## 3. Essential Commands

Run development checks via [`just`](justfile):

| Command | Purpose |
|---|---|
| `just fmt` | Check formatting (`cargo fmt --all` to fix). |
| `just clippy` | Run Clippy with `-D warnings` across workspace feature combinations. |
| `just test` | Run unit tests and doctests across default, `--no-default-features`, and extra features. |
| `just doc` | Build rustdoc with `RUSTDOCFLAGS="-D warnings"` across feature combinations. |
| `just test-pdfium` | Verify generated cases, doc build pins, and PDFium (`chromium/8125`) answers. |
| `just test-qpdf` | Run ignored cross-checks against pinned `qpdf` 12.4.2. |
| `just fuzz <target>` | Run libFuzzer target on nightly (see `fuzz/Cargo.toml`). |
| `just ci` | Full local CI suite (requires Rust 1.89, nightly `rust-src`, and cross targets). |

## 4. Repository & Test Invariants (Watch Out!)

Several automated tests enforce strict consistency across code and documentation:

1. **Feature-matrix compilation & rustdoc (`just clippy`, `just test`, `just doc`):**
   Crates are tested and documented with `--no-default-features` (e.g., `pdf-semantics` without `clock`, `pdf-linearize` without `family`, `pdf-fonts` without `programs`, `pdfs` without `programs`/`clock`, `pdf-filters` with only `zlib-rs`). Avoid unguarded `[`intra_doc_links`]` or doctests that reference feature-gated items when those features are off.
2. **README doctests & `lib.rs` first-example parity:**
   Each library crate compiles its `README.md` via `#[cfg(doctest)] #[doc = include_str!("../README.md")]`. In `pdfs`, `pdf-fonts`, `pdf-crypt`, `pdf-linearize`, `pdf-semantics`, and `pdf-content-streams`, a unit test asserts that the **first** ` ```rust ` block in `README.md` matches the **first** ` ``` ` block in `src/lib.rs` (`//!`) line-for-line.
3. **README URLs & license links:**
   Published crates test that `README.md` contains exact GitHub `https://` links to `LICENSE-APACHE` and `LICENSE-MIT` (and in `pdf-fonts`, exact links to `` [`FTL.TXT`](...) `` and `` [`assets/afm/MustRead.html`](...) ``), contains no `](../` relative paths, and in several crates uses `https://` for every Markdown link.
4. **`pdf-lint` CLI `--help` sync:**
   `crates/pdf-lint/tests/cli.rs` asserts that the ` ```text ` block immediately under `## The command line` in `crates/pdf-lint/README.md` matches `pdf-lint --help` output byte-for-byte (and stays within 80 columns).
5. **PDFium `chromium/8125` pin check (`oracle/pdfium.py`):**
   Every crate with a `tests/pdfium/` directory (`pdf-tokenizer`, `pdf-objects`, `pdf-structure`, `pdf-content-streams`, `pdf-semantics`, `pdf-color`, `pdf-fonts`, `pdf-linearize`) must include the exact phrase `as of chromium/8125` in its `README.md`, and any `as of chromium/NNNN` mention in `README.md` or `src/` must match the recorded `chromium-NNNN.txt` file.
6. **MSRV & Workspace Dependencies:**
   Published library crates target **`rust-version = "1.89"`**. Pre-release crates in `[workspace.dependencies]` use exact version pins (`=0.1.0-alpha.N`), and `just _package` enforces a strict allowlist of packaged files (`src/**/*.rs`, `README.md`, `CHANGELOG.md`, `Cargo.toml`, `Cargo.lock`, plus `pdf-fonts` assets).
