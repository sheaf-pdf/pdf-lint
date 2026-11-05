# PDFium, the oracle

pdf-tokenizer's PDFium rules (`Rules::Pdfium`, `decode::pdfium`),
pdf-objects' (`Parser::rules`), pdf-semantics' page tree (`pages::pages`),
pdf-color's count of a space's components (`components`), pdf-fonts' reading
of a font (`Font`) and
pdf-content-streams, which reads content as PDFium does throughout, follow
PDFium's reading, which no standard writes down and which changes as PDFium
does; pdf-linearize lays files out for PDFium's availability API, among
others. So each is held to one PDFium build, the pin, through cases:
small hand-made inputs, each asked of PDFium, its answer recorded beside
it. A plain `cargo test` holds each crate to the recorded answers, without
PDFium (each crate's `tests/pdfium.rs`); `just test-pdfium` asks every case
of PDFium again and holds PDFium to them.

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

What PDFium reads also depends on how it's built. pdfium-binaries' build
(its `args.gn`, in the tgz) leaves `pdf_enable_rust_jpeg` at PDFium's
default, off, so its JPEG decoder is libjpeg-turbo, whose reader says where
a JPEG's data ends; the recorded DCT answers are that decoder's. With the
Rust decoder PDFium can be built with instead (`PDF_ENABLE_RUST_JPEG`),
which says nothing of where it stopped, every inline JPEG would end at its
start of image.

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
  file, but for pdf-fonts' `programs/` (below). Flate data is written as stored blocks (`zlib.compress(data, 0)`),
  whose bytes every zlib writes alike. No two cases' names are alike but
  for case, as a file system that ignores case would keep one file for
  both.
- `programs/`, pdf-fonts' alone: the font programs its `cases.py` reads,
  which Python can't build. The crate's builders make them by hand; its
  ignored test `the_programs_are_written` writes them, then
  `oracle/pdfium.py cases --write` the cases, and
  `the_programs_are_the_builders` holds them to the builders.
- `cases/` and `cases.tsv`: what the generator writes, each case's files
  (`NAME.EXT`) and a line naming it, its kind and its files. Line endings
  in `cases/` are kept as written (`.gitattributes`).
- `chromium-N.txt`: PDFium's answer for each case, one line each, at the
  pinned build, chromium/N. There's one, and the crate's README, and its
  sources' docs wherever they say which build they read as ("as of
  chromium/N"), name the same N: `cases` holds them to it.
- `differ.tsv`: the cases the crate reads otherwise than PDFium, each with
  why, the crate's test holding each to still differ, so a change that
  settles one is seen and its line goes. Where the crate can read in more
  than one way, as pdf-content-streams finds an inline image's DCT and
  CCITTFax data's end by a stand-in or, with its `image-ends` feature, by
  decoding it, a column between the two says in which it's read otherwise
  (`all`; `default`, the stand-in; or `image-ends`, the decoders), and the
  test holds it to agree in the others.
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
- `pages`: a whole file (`.pdf`), the one kind not made around the case: it's
  asked for its page tree. The answer is `FPDF_GetPageCount`, then, in order,
  each page from the first to two past the last, its width, height and quarter
  turns (`FPDFPage_GetRotation`), or `null` where it doesn't load, `;` between
  them; past 64 pages, the first 64 and two more; `no-document` where the file
  doesn't open.
- `avail`: a whole file (`.pdf`), given PDFium only as its availability API
  (`FPDFAvail_*`) asks for it, from its first 1,024 bytes on, with the bytes it
  took counted, and opened by its password (`.password`, the bytes
  `FPDFAvail_GetDocument` is given) where there's one. The answer is
  `FPDFAvail_IsLinearized`; what `IsDocAvail` came to
  and the bytes taken by then; `no-document` where it doesn't load; else the count
  of pages and the first page, then, `;` between them, for the first page and then
  the others up to 64, what `IsPageAvail` came to, the bytes that took and the page's
  width, or `null` where it doesn't load; and the bytes got of the file's. `stuck`
  is a stage that says not yet and asks for nothing.

- `spaces`: a whole file (`.pdf`), whose first page draws inline images, there or
  in a form, each 8 samples wide, one high, of 8 bits, unfiltered, in a color space
  the resources name. PDFium sizes such an image by the components it counts, so its raw size
  says what they were: the answer is a word for each image, in order, that size
  over 8, or `none` for the 1 byte of an image with no color space (`raw:N`, a
  size neither makes; `no-images`; `no-page`).

- `fonts`: a whole file (`.pdf`), whose first page shows one string in one font from
  (0, 100), at a size of 1000, where the text page gives a glyph unit as a unit: the
  answer is, for each character of the text page, its `FPDFText_GetUnicode` in hex and
  the x and y of its origin from there, `;` between them (`none`; `no-page`). A text
  page isn't the font's: it drops a code at much the same place as itself shortly
  before it, a space after a space, and all of a string that takes up no room, so
  a case's string is of different codes, a width among them.

- `fonts-heights`: as `fonts`, and after them `top` and `bottom`, how far the first
  character's `FPDFText_GetLooseCharBox` reaches from its origin: the font's heights,
  over that character's own box. A case's first character is one with none, as it's
  a face's, and the font's heights its own, so that no answer rests on a face PDFium
  substitutes.

- `fonts-boxes`: as `fonts`, for a font whose glyphs are a program it embeds, and after
  each character's origin its `FPDFText_GetCharBox` (`box`) and its
  `FPDFText_GetLooseCharBox` (`loose`), each left, bottom, right and top from that origin.

Numbers are written with nine significant digits, which read back to the
same `f32`, and compared as `f32`s. Text and data are written byte for
byte, printable ASCII as it is and any other byte, and `%`, as `%XX`; none
is `%` alone; longer than 64 bytes, `#` with the length and FNV-1a 64 hash.

## Bumping the pin

To follow PDFium's newer reading, the pin moves to a newer build:

1. Take the new tag's `pdfium-linux-x64.tgz` from pdfium-binaries' release,
   verify its attestation (`gh attestation verify FILE --repo
   bblanchon/pdfium-binaries`), and only then take its SHA-256. Check how
   it's built: its `args.gn`, and PDFium's default for
   `pdf_enable_rust_jpeg` at the new tag (`pdfium.gni`, and the
   `build_overrides` it reads). A build with the Rust JPEG decoder moves
   every DCT case's end, which is the build's choice of decoder, not a
   reading to follow: take one with libjpeg-turbo.
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
   the build named in this file, the crates' docs (each "as of chromium/N",
   which `cases` holds to the recorded build) and their CHANGELOGs; then
   `just pdfium-answers`, which records every answer at the new build but
   for the cases `chromium-NEW.differs` lists, which keep the answers
   recorded before, and `just ci`.

Rules for two PDFium builds at once would come only if two read
differently and a caller needs both.
