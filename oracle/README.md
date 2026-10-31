# PDFium, the oracle

pdf-tokenizer's PDFium rules (`Rules::Pdfium`, `decode::pdfium`),
pdf-objects' (`Parser::rules`) and pdf-content-streams, which reads content
as PDFium does throughout, follow PDFium's reading, which no standard
writes down and which changes as PDFium does. So each is held to one PDFium
build, the pin, through cases: small hand-made inputs, each asked of
PDFium, its answer recorded beside it. A plain `cargo test` holds each
crate to the recorded answers, without PDFium (each crate's
`tests/pdfium.rs`); `just test-pdfium` asks every case of PDFium again and
holds PDFium to them.

## The pin

PDFium as of **chromium/8125**: a development snapshot. PDFium makes no
releases; its `chromium/NNNN` branches follow Chromium's build numbers,
and pdfium-binaries builds the newest of them each week. The pin is its
Linux x86_64 build at that tag, `pdfium-linux-x64.tgz`, SHA-256
`e1c3ce6e270cb5e0c20816b379ad9f2c1309972697c505f508de92bf5e7d6659`,
whose build provenance pdfium-binaries attests. `just _pdfium` fetches it
into `target/pdfium-8125/`, checks the SHA-256 before unpacking it, and,
where `gh` has `attestation` and can sign in (as CI's can), verifies the
attestation (`gh attestation verify FILE --repo bblanchon/pdfium-binaries`);
without, it says the SHA-256 alone was checked. Elsewhere than Linux x86_64,
`PDF_LINT_PDFIUM` names a PDFium library to use instead.

The library is only ever loaded by the runner below, on the cases' own
inputs. No crate depends on it or ships it.

## Running it

- `just test-pdfium`: each set's generator writes its cases again, which
  must be the committed bytes; then each case is asked of the pinned build,
  or of the library `PDF_LINT_PDFIUM` names, and each answer held to the
  recorded one. Every case that answers otherwise is listed.
- `just pdfium-answers`: each set's cases written again from its
  generator, and every answer recorded anew from the pinned build: when
  cases are added, or at a bump.
- `python3 -I -B oracle/pdfium.py run LIB SET...` prints each case's
  answer from any library; `check`, `record` and `cases` are the steps
  above. The runner is Python's standard library alone, PDFium loaded by
  `ctypes`; `-I -B` keeps the environment out and writes no bytecode.

## A set

Each crate whose reading the cases hold keeps them in `tests/pdfium/`,
outside its package, so they move with it:

- `cases.py`, the generator: every case's bytes, written in it from
  literals and Python's standard library. Nothing comes from another
  file. Flate data is written as stored blocks (`zlib.compress(data, 0)`),
  whose bytes every zlib writes alike.
- `cases/` and `cases.tsv`: what the generator writes, each case's files
  (`NAME.EXT`) and a line naming it, its kind and its files. Line endings
  in `cases/` are kept as written (`.gitattributes`).
- `chromium-N.txt`: PDFium's answer for each case, one line each, at the
  pinned build, chromium/N. There's one.
- `differ.tsv`: the cases the crate reads otherwise than PDFium, each with
  the builds it's read otherwise in (`all`; or, where a feature changes
  the reading, `default` for the build without it, or the feature's name,
  as pdf-content-streams' `image-ends`) and why. The crate's test holds
  each to still differ in those builds, and to agree in the others, so a
  change that settles one is seen and its line goes.
- `chromium-N.differs`, at a bump only: cases whose answer at chromium/N
  is PDFium's regression, kept at their recorded answer (below).

## The kinds

A case's kind says how PDFium is asked and what its answer is. Each is
asked through a one-page PDF made around the case.

- `content`: the page's content stream (`.content`) and resources
  (`.res`, or none). The answer is the page's objects, in order, `;`
  between them: a path's fill mode, whether it's stroked, its matrix, its
  points (`m`, `l`, `c`, each `h` after where it closes) and its dash
  array; an image's matrix and raw data; a text object's font, size and
  text; and each object's marks' `/MCID`s.
- `number`, `hex`, `literal`, `name`: a word (`.word`) in content. A
  `number` is `re`'s second operand, its answer the value PDFium holds, or
  `not-a-number`; a `hex` or `literal` string is shown by `Tj` in Courier,
  its answer the text shown, or `none`; a `name` is `Tf`'s font, its
  answer whether PDFium found the font `/F0` by it.
- `mediabox`: a page's `/MediaBox` (`.obj`), its answer the page's width
  and height.
- `info`: the document's `/Info` dictionary (`.obj`), its answer the text
  of its entry under the key in `.tag`.
- `kids`: what a page tree's `/Kids` holds (`.obj`), its one page object
  3, its answer that page's size, or `no-page`.

Numbers are written with nine significant digits, which read back to the
same `f32`, and compared as `f32`s. Text and data are written byte for
byte, printable ASCII as it is and any other byte, and `%`, as `%XX`; none
is `%` alone; longer than 64 bytes, `#` with the length and FNV-1a 64 hash.

## Bumping the pin

To follow PDFium's newer reading, the pin moves to a newer build:

1. Take the new tag's `pdfium-linux-x64.tgz` from pdfium-binaries' release,
   verify its attestation (`gh attestation verify FILE --repo
   bblanchon/pdfium-binaries`), and only then take its SHA-256.
2. `PDF_LINT_PDFIUM=<its lib/libpdfium.so> just test-pdfium` lists each
   case whose answer moved.
3. Read each against PDFium's changes between the two tags, its log and
   its source.
   - A deliberate change in reading is followed: the crate's code changed
     to read alike, its tests and docs with it, and its CHANGELOG saying
     so.
   - A crash or a regression isn't: the case keeps its recorded answer,
     and the new build's answer goes in `chromium-NEW.differs` (the case's
     name, a tab, the answer), which `check` accepts at that build only.
4. The tag and the SHA-256 in the justfile (`pdfium_build`, `_pdfium`),
   the build named in this file, the crates' docs and their CHANGELOGs;
   then `just pdfium-answers`, which records every answer at the new build,
   and `just ci`.

Rules for two PDFium builds at once would come only if two read
differently and a caller needs both.
