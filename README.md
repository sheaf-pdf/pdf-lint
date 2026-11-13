# pdf-lint

Checks PDF files against the
[Arlington PDF Model](https://github.com/pdf-association/arlington-pdf-model),
the PDF Association's machine-readable grammar of ISO 32000-2, and their
content, common structures, color spaces, fonts and linearization against
ISO 32000-2 itself, and points each finding at the bytes it's about.

It's made for tools that write PDFs. Most files in the wild break some
rule somewhere, so it can hold to the model only what your tool wrote:

```sh
pdf-lint out.pdf                     # every object
pdf-lint --last-update out.pdf       # what the newest incremental update wrote
pdf-lint --baseline in.pdf out.pdf   # findings in.pdf didn't already have
pdf-lint --hold objects.txt out.pdf  # the objects your tool says it wrote
```

It reads each file from its bytes, and decodes what's compressed and
decrypts what's encrypted itself.

| Crate | What |
|---|---|
| [pdf-lint](crates/pdf-lint) | The checker, a command line and a library. Its README has the rules, the options, the JSON format and how to install it |
| [pdf-tokenizer](crates/pdf-tokenizer) | A lossless, zero-copy PDF tokenizer, by ISO 32000-2's rules or PDFium's, needing neither `std` nor `alloc` |
| [pdf-filters](crates/pdf-filters) | PDF's standard filters for data of any kind, decoded, every fault said and what decoded kept; and Flate encoded, with PNG's predictors, on miniz_oxide or zlib-rs |
| [pdf-crypt](crates/pdf-crypt) | PDF's standard security handler on RustCrypto's primitives: decrypting, every fault said, and encrypting for a writer, its IVs and a new file's key and dictionary from the operating system's randomness, under an open password, a permissions password or both, a file's permissions typed, its keys wiped |
| [pdf-objects](crates/pdf-objects) | PDF's objects as a reader read them, their parser, by ISO 32000-2's rules or PDFium's, and their writer, which writes an encrypted file's through an encryptor: each value with where it was written, every fault said, and text strings read and written |
| [pdf-structure](crates/pdf-structure) | PDF's file structure, read whole or each object on demand, from any thread, from memory or a file read by position: the header, cross-reference tables and streams along `/Prev`, object streams, and recovery where the file proves it or as PDFium reads it, each object through pdf-objects' parser, decrypted through pdf-crypt and decoded through pdf-filters, and so any stream's data asked for, where it was read from recorded, every choice it made said; and written, whole or as an incremental update, cross-reference sections and object streams deflated through pdf-filters, an encrypted file's objects encrypted as they're written, through pdf-crypt, by the list it decrypts by, and an object as read copied as the bytes it was read from, checked against a hash of them |
| [pdf-content-streams](crates/pdf-content-streams) | PDF content streams read as PDFium reads them: each token with where it is, an inline image's data found by PDFium's rules, each operation with its operands, operands read as pdf-objects' values when asked, and a page's streams joined as one, every place mapped back to its stream, under caps, every error saying where, and where ISO 32000-2 reads it otherwise told to a sink; each operator typed by ISO 32000-2's Annex A, with its operands and where it may be used; a page's `q`/`Q` balance, its marked content and the resource names it uses; and content written, the same bytes for the same calls, its resources named at the end by a digest of what they are, or spliced, ranges taken out and every other byte kept, only where that can't change what's drawn after them |
| [pdf-semantics](crates/pdf-semantics) | PDF's common structures, each read and written: dates, each field saying whether it was written and each departure from the standard said; name and number trees, walked under caps; the page tree by ISO's rules or PDFium's, each page numbered as its rules number it and each value saying where it came from, and a balanced tree written over a caller's pages; and what XMP metadata claims, PDF/A, PDF/UA or PDF/VT, read by namespace under caps, from a packet or through a file's streams, and a packet of claims written that reads back as they are |
| [pdf-color](crates/pdf-color) | PDF's color spaces read over any resolver: how many components a space has as PDFium counts them, whatever it holds; each space typed as ISO 32000-2 says it, with its family, base or alternate space and colorants, and each departure told; and what a resources dictionary names, under caps |
| [pdf-fonts](crates/pdf-fonts) | PDF's fonts read as PDFium reads them: a font dictionary's codes, CIDs, widths, heights and text, what's guessed said and every fault told, a cap's and the build's apart from the file's, each font read once and charged to a budget, and a stream past a cap the caller's to know; simple fonts' encodings and the text of glyph names; the standard 14's names as PDFium takes them, with Adobe's metrics; how PDFium reads each predefined CMap, with Adobe's own behind a feature; and what a font's program says of its glyphs' boxes, its heights and its text, through skrifa |
| [pdf-linearize](crates/pdf-linearize) | PDF files laid out for reading over a network, with no object model of its own: a graph of objects, of any model, planned into Annex F's order, numbers and object streams, and the bytes around a caller's bodies assembled, the padded dictionary, both cross-reference sections and the hint tables, which also read back from any bytes, under caps; a document of pdf-structure's written so, its inherited attributes pushed down, in the clear or encrypted, a direct `/Info` encrypted as an object and an RC4 file's keystreams reused only where the caller accepts the leak; and the facts of any file that says it's one held against each other, each finding graded, and what the reader didn't read or a cap stopped told as not judged, never as the file's |
| [pdfs](crates/pdfs) | The family in one dependency: pdf-tokenizer, pdf-objects, pdf-filters, pdf-crypt, pdf-structure, pdf-color, pdf-content-streams, pdf-semantics, pdf-fonts and pdf-linearize, each re-exported whole at the versions it's built with; and a file read through them with a viewer's defaults: its pages and their content, what it says of itself and claims, and whether its linearization holds |
| [pdf-arlington](crates/pdf-arlington) | The Arlington PDF Model, embedded: its tables read, and the predicates in their cells evaluated over pdf-objects' objects |
| [pdf-fixtures](crates/pdf-fixtures) | The tests' PDF files, written from pdf-objects' values, whole or by hand and encrypted, and qpdf run as a second reader and decoder: the workspace's own, which the crates' tests take by its path, never published |
| [pdf-linearize-lopdf](crates/pdf-linearize-lopdf) | A document of lopdf's written as a linearized file through pdf-linearize's engine, in the clear, by plain functions over its public objects, as the family's adapter places and pushes down: the workspace's own, never published |

## Development

The checks are recipes in the [justfile](justfile), run with
[just](https://github.com/casey/just):

```sh
just          # list the recipes
just ci       # run everything CI runs, but the semver check
just clippy   # or one at a time
```

`just ci` also needs the `thumbv6m-none-eabi`, `x86_64-pc-windows-gnu`
and `wasm32-unknown-unknown` targets, nightly Rust with its `rust-src`
component, and Rust 1.89 installed: for the tokenizer's no-std
build, pdf-structure's lints for Windows and a 32-bit target, the
tokenizer's build against `core` alone, and the minimum Rust of every
crate, 1.89, with and without each one's features. Its `test-pdfium` needs
Python 3, and fetches the pinned PDFium build from GitHub unless
`PDF_LINT_PDFIUM` names a library: [oracle/README.md](oracle/README.md)
says how the crates are held to PDFium. Its `tokenizer-package`,
`objects-package`, `filters-package`, `crypt-package`, `color-package`,
`structure-package`, `content-streams-package`, `semantics-package`,
`fonts-package`, `linearize-package` and `pdfs-package` package the
tokenizer, pdf-objects, the filters, pdf-crypt, pdf-color, pdf-structure,
pdf-content-streams, pdf-semantics, pdf-fonts, pdf-linearize and pdfs as a
publish would (`cargo publish --dry-run`), which asks the registry over the
network. CI runs pdf-structure's tests on Windows too, which no recipe does,
as `just ci` runs on the platform it's run on.
`just tokenizer-semver`, `just objects-semver`, `just filters-semver`,
`just crypt-semver` and `just color-semver`, which `ci` leaves out, check
the tokenizer's, pdf-objects', the filters', pdf-crypt's and pdf-color's
public API against the version published on crates.io (an alpha, while its
manifest names a published version, is held to that version's API: a change
needs the next alpha) and need
[cargo-semver-checks](https://github.com/obi1kenobi/cargo-semver-checks)
installed (CI installs 0.51.0 on a Rust pinned beside it).

`just fuzz tokenize`, `just fuzz filters`, `just fuzz crypt`, `just fuzz
objects`, `just fuzz structure`, `just fuzz write`, `just fuzz content`,
`just fuzz semantics`, `just fuzz color`, `just fuzz fonts`, `just fuzz
linearize`, `just fuzz pdfs` and `just fuzz check` fuzz the tokenizer, by ISO
32000-2's rules and PDFium's, the filters, decoding and encoding, pdf-crypt's handler, new
files' dictionaries and a password's forms, pdf-objects' parser, by ISO
32000-2's rules and PDFium's, its writer and text strings, pdf-structure's
reader and writer, pdf-content-streams' tokens, operations, faults, operator
table, the balance, marked content and names over them, its writer, content
written again and the calls, its splices and lists written again, an
image's color space counted alike in its dictionary and in the resources,
and a page's `/Contents` joined,
pdf-semantics' name and number trees, page tree, dates and XMP claims, each
read and written, a file's packet read through its streams and its document
information dictionary read,
pdf-color's count of a space's components, its typed spaces and what a
resources dictionary names, pdf-fonts' scan of a Type 1 program under its
caps, its reading of names, and of a font and its program as parts and as a
file, pdf-linearize's plan of any graph, what's assembled from it, hint
tables from any bytes, any facts of a file, judged alike whatever the sink
and nothing judged of what they say wasn't read, and any bytes as a file
laid out in the clear, under its own encryption and under a new one, read
back and checked, and under caps on its trees, a file read through pdfs, and
written in the clear and encrypted, read locked and unlocked, and the reader
and checker, on nightly with
[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz); a second argument
says for how many seconds (60 by default), and a third the features they're
built with: `simd`, pdf-content-streams' dependencies on the CPU's SIMD
(`just fuzz content 300 simd`), and `predefined-cmaps`, Adobe's predefined
CMaps from pdf-fonts' bundle, which pdf-lint's binary is built without
(`just fuzz fonts 300 predefined-cmaps`). CI fuzzes each target the same
way, `content` again with `simd` and `fonts` again with `predefined-cmaps`:
two minutes on every pull request and push to main, and half an hour twice
a week, each run starting from the inputs the runs before it kept
(`.github/workflows/fuzz.yml`).

## License

pdf-tokenizer, pdf-filters, pdf-crypt, pdf-objects, pdf-structure,
pdf-content-streams, pdf-semantics, pdf-color, pdf-fonts, pdf-linearize,
pdfs, pdf-fixtures and pdf-linearize-lopdf are licensed under either of
Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) or MIT license
([LICENSE-MIT](LICENSE-MIT)) at your option, pdf-fonts' data under the terms
each of its files carries, and what it takes from FreeType under FreeType's,
[`FTL.TXT`](crates/pdf-fonts/FTL.TXT)
([its README](crates/pdf-fonts/README.md#license)). pdf-lint and
pdf-arlington are licensed under the Apache License, Version 2.0, as is the
Arlington model pdf-arlington embeds.
