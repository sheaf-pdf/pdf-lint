# pdf-lint

Validate PDF files against the [Arlington PDF Model](https://github.com/pdf-association/arlington-pdf-model) (the PDF Association's machine-readable grammar of ISO 32000-2) and ISO 32000-2 rules for content streams, page trees, name/number trees, XMP metadata, color spaces, fonts, object streams, and linearization.

`pdf-lint` ships as both a standalone command-line tool and a Rust library. It parses, decompresses, and decrypts PDFs directly from raw bytes (requiring no external tools like `qpdf`), so every finding points to the exact file or decoded-stream byte range where the problem occurs:

```text
example.pdf: PDF 1.7, 6 objects checked, 2 findings
- type: PageObject/MediaBox at Root/Pages/Kids[0], in 3 0 R (merged): [0 0 612] [bytes 163..182]
    | /MediaBox [0 0 612]
- duplicate: PageObject/Rotate at Root/Pages/Kids[0], in 3 0 R (merged): 90 (its key written again later) [bytes 152..162]
    | /Rotate 90
```

## Checking Only What Your Tool Wrote

Most real-world PDFs already contain minor spec violations. If your software edits or updates existing PDFs, validating the entire output file produces noisy reports full of pre-existing issues. `pdf-lint` provides targeted diff and scoping modes:

- **`--last-update`** — Validates only objects written in the newest incremental update (without needing the original input file). Rewritten objects are checked as `merged`, newly added objects as `built`, and findings already present in the preceding revision are suppressed.
- **`--baseline in.pdf`** — Suppresses findings that already existed in `in.pdf`. Incremental updates are matched by object ID; full rewrites (which renumber objects) are matched by structural path (e.g. `Root/Pages/Kids[3]/Annots[0]`).
- **`--hold objects.txt`** — Validates only the object IDs listed in `objects.txt` (`12 0 built` for newly constructed objects, `3 0 merged` for modified objects).
- **`--exempt keys.txt`** — Allows custom private keys in `built` objects (one PDF name per line, e.g. `/MyKey` or `/My*` prefix wildcards).
- **`--unlisted <built|all|never>`** — Controls where keys absent from the Arlington grammar are reported (`built` by default, since `merged` objects often carry third-party keys from the input PDF; Annex E second/third-class names are always allowed).

## What It Checks

`pdf-lint` walks all objects reachable from the trailer—plus object streams, linearization dictionaries, and historical trailers from earlier incremental updates—and validates:

- **Arlington object grammar:** Key types, direct vs. indirect requirements, required keys (including inherited page attributes), allowed values, special-case predicates, PDF version introductions/deprecations (`--deprecations`), broken references, unlisted keys, and duplicate dictionary keys.
- **Merged form fields & widgets:** Dictionaries that combine a single form field and widget annotation (ISO 32000-2 §12.5.6.19) are validated against both Arlington tables simultaneously unless `--one-table` is passed.
- **Declared extensions:** Automatically enables extensions declared in `/Extensions` (`ADBE_Extn3`, `ETSI_PAdES`, `/ISO_` specs) or XMP metadata (`ISO_19005_3`, `PDF_VT2`), plus any passed via `--extensions` (such as `AAPL`, `OpenOffice`, `Malforms`).
- **Page trees & viewer divergences (`page-tree`, `viewer`):** Validates `/Kids`, `/Parent`, `/Count`, cycles, and empty trees via [pdf-semantics](../pdf-semantics), and flags cases where a PDFium-based viewer sees a different page count, page object, or page box/rotation than strict ISO 32000-2 rules.
- **Name and number trees (`tree`):** Checks `/Limits`, key ordering, duplicate/looping nodes, and empty intermediate nodes.
- **XMP metadata (`xmp`):** Validates UTF-8 encoding and well-formed XML structure on uncompressed, unencrypted catalog metadata streams.
- **Color spaces & fonts (`color-space`, `font`):** Validates Indexed lookup table lengths, ICC `/Alternate` component counts, Separation/DeviceN colorants, simple font `/Widths`, CIDFont `/DescendantFonts` and `/W` arrays, `/ToUnicode` and `/UseCMap` streams, and `/CapHeight` requirements on Latin fonts via [pdf-color](../pdf-color) and [pdf-fonts](../pdf-fonts).
- **Object streams (`packed`):** Flags prohibited entries inside object streams (nested streams, `/Encrypt`, `/Length` targets, linearization dictionaries, and unencrypted strings in encrypted files).
- **Content streams (`balance`, `marked`, `operator`, `operands`, `resource`, `content`, `unused`):** Validates `q`/`Q` graphics-state balance, `BMC`/`BDC`/`EMC` nesting, allowed operator states (ISO 32000-2 Figure 9), Annex A operand counts/types, missing `/Resources` entries, lexical divergences between ISO 32000-2 and PDFium, and optionally `--unused` resources via [pdf-content-streams](../pdf-content-streams).
- **Linearization (`linearization`, `updated`):** Validates Annex F layout, offsets, and hint tables via [pdf-linearize](../pdf-linearize) (with `--departures` for minor real-world deviations), and flags linearized files whose linearization was voided by a later incremental update.
- **Low-level syntax & structure (`reader`):** Flags stream `/Length` mismatches, missing `endobj`, non-20-byte xref rows, trailing bytes past `%%EOF`, overlapping object-stream offsets, and filter/decryption faults via [pdf-structure](../pdf-structure).

Checks that cannot be completed due to missing passwords, unsupported security handlers/filters, or resource caps are recorded in `not_checked` counts rather than guessed.

## The command line

```text
Checks each FILE against the Arlington PDF Model, and its content against
ISO 32000-2, reading it from its bytes, and prints each finding with the
bytes it's at. It decodes object streams, cross-reference streams and
content streams, and decrypts what's encrypted, itself.

Usage: pdf-lint [OPTIONS] <FILE>...

Arguments:
  <FILE>...  a file to check; - is standard input

Options:
      --hold <FILE>      hold only the objects FILE names, a line each:
                         `12 0 built` or `3 0 merged`
      --built            hold every object as built, so that a key the model
                         doesn't list is a finding (by default, as merged)
      --exempt <FILE>    keys a built object may have though the model doesn't
                         list them, a name a line: `/MyKey`, or `/My*` for every
                         key it begins
      --unlisted <W>     which held objects a key the model doesn't list is a
                         finding in: built (the default), all, or never; it's
                         counted in any
      --deprecations     find what the version checked at deprecates: a key, a
                         type or a value (by default, counted only)
      --unused           find each resource the content read never uses, as a
                         font or an image its resources name for nothing (by
                         default, counted only); not --unlisted's keys
      --departures       find a linearized file's departures from Annex F
                         that readers get past, as its errors are (by default,
                         counted only)
      --extensions <E>   check each FILE under these extensions too, besides
                         those it declares: the model's names for them, a comma
                         between (Malforms,AAPL)
      --last-update      hold only the objects the newest incremental update
                         wrote: those it rewrote as merged, new ones as built;
                         and leave out what a rewritten object, or the trailer,
                         already had in the revision it updates
      --baseline <FILE>  leave out the findings FILE, the file each FILE was
                         written from, already had: an incremental update of it
                         object by object, a rewrite of it by where each finding
                         is, or, where FILE has nothing there, anywhere in it
      --pdf-version <V>  check each FILE as PDF V, 1.0 to 1.7 or 2.0, not as
                         the version its header and catalog give, and what it's
                         set against, a baseline or the revision before an
                         update, as V too
      --model <DIR>      check against the tables in DIR, a checkout of the
                         Arlington model's tsv/latest, as they are, not the
                         embedded ones, whose cells that contradict ISO 32000-2
                         are corrected
      --one-table        check each object as one table, as TestGrammar does:
                         a field and its widget merged in one dictionary as
                         one of the two (by default, as both), and a key
                         unlisted by the first table that reaches an object
                         though another that does lists it
      --allow <TSV>      leave out the findings TSV allows: a # header, then
                         table, key, rule, file, version and reason, tab apart;
                         a version, as the file is checked as
      --full             print the whole report: tables, checks not made, keys
                         the model doesn't list
      --password <PW>    the files' password, user or owner, when it isn't
                         empty: tried in its SASLprep form (RFC 4013) and as
                         UTF-8, then in PDFDocEncoding, WinAnsiEncoding and
                         MacRomanEncoding
      --jobs <N>         check N files at once (by default, as many as there
                         are CPUs); the report is in the files' order all the
                         same
      --format <F>       text (the default), or json: a line of JSON for each
                         file, with its findings and the whole report's counts
  -V, --version          print the version, and the model's commit,
                         corrections, additions and hash
  -h, --help             print this

It exits 0 when every file is clean, 1 when there are findings, and 2
when a file can't be read or the command line is wrong.
```

## JSON Output (`--format json`)

With `--format json`, `pdf-lint` emits one JSON object per line (JSON Lines):

- **Metadata & configuration:** `pdf_lint` (tool version), `file`, `version`, `checked_as`, `model` (16-hex-digit XXH64 hash of the active Arlington tables), `extensions`, `one_table`, `unlisted_in`, `deprecations`, `unused_findings`, `departure_findings`.
- **Findings (`findings`):** Array of objects with `rule`, `table`, `key`, `path`, `object` (`[num, gen]` or `null`), `made`, `value`, and `at` (`{ "stream": [num, gen] | null, "start": usize, "end": usize }` or `null`).
- **Summary counts:** `objects`, `keys`, `carried`, `allowed`, `tables`, `not_checked`, `unlisted`, `deprecated`, `content_read`, `linearized` (`"holds"`, `"fails"`, `"updated"`, `"not-taken"`, or `null`), `unused`, `departures`, `ignored`, `no_table`, `ties`, `truncated`, `two_contexts`, and `two_tables`.
- **Unreadable files:** Emit `{ "pdf_lint", "model", "file", "error" }`.

## Installation & Library Usage

### Prebuilt Binaries

Each [GitHub release](https://github.com/sheaf-pdf/pdf-lint/releases) provides self-contained archives for Linux (`x86_64`, `aarch64`, static musl), macOS (`arm64`, `x86_64`), and Windows (`x86_64`, static CRT), bundled with required third-party notices. *(On macOS, if downloaded via a browser, clear Gatekeeper quarantine with `xattr -dr com.apple.quarantine <folder>`.)*

### Building from Source

```sh
cargo install --locked pdf-lint
# or from a local checkout:
cargo install --locked --path crates/pdf-lint
```

### Using `pdf-lint` as a Rust Library

Depend on `pdf-lint` with `default-features = false` to omit CLI-only dependencies (`clap`, `serde`, `serde_json`, `anyhow`):

- **`pdf_lint::check(&source, &options)`** validates any [`pdf_structure::Source`] implementation (such as an in-memory `Document`).
- **`pdf_lint::check_with_reading(&with_reading, &options)`** additionally inspects decoded streams, historical revision prefixes, and Annex F linearization using `bytes::WithReading`.
- **`Report::since`**, **`Report::by_path`**, and **`Report::beyond`** provide programmatic baseline and incremental filtering.
- **`use_model(Grammar::load(dir)?)`** overrides the embedded Arlington grammar before the first check runs.

## Embedded Arlington Model & Corrections

[pdf-arlington](../pdf-arlington) embeds upstream's `tsv/latest` tables at a pinned commit (`pdf_lint::COMMIT`) and applies a small set of spec-backed corrections (`grammar::corrections()`) and additions (`grammar::additions()`) where upstream contradicts ISO 32000-2 (for example: `/DA` on non-variable-text button fields, `fn:IsFullyQualifiedFieldName` on hide actions, optional `/DecodeParms` length checks, `SMaskInData` defaults, legacy PDF 1.4 `/DR` on fields, and PDF 1.0 `EncodingMap` dictionaries). Pass `--model <DIR>` to check against an unmodified upstream `tsv/latest` directory instead.

## License

Licensed under the Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)).

The embedded Arlington PDF Model and logic adapted from upstream `TestGrammar` (Copyright 2020–2022 PDF Association, Inc.) are also licensed under Apache-2.0; release archives bundle `ARLINGTON-NOTICE.txt`, `THIRD-PARTY-NOTICES.txt`, and font metric notices.
