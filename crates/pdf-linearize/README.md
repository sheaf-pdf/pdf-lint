# pdf-linearize

Lay out PDF files for reading over a network (ISO 32000-2, Annex F), and check a file that says it's laid out so. The engine has no object model and does no I/O. A caller describes its objects as plain data, from whatever model holds them, and writes their bodies itself.

It's made for what writes PDF files a viewer can show a page of before the rest arrives, and for what checks such a file. It's built on [pdf-objects](https://crates.io/crates/pdf-objects) and re-exports it whole, so `pdf_linearize::pdf_objects` names its types at the version this crate is built with. Its default `family` feature writes and reads files through [pdf-structure](https://crates.io/crates/pdf-structure) and [pdf-semantics](https://crates.io/crates/pdf-semantics), both re-exported whole too.

## What it does

- **Plan a file's order.** `plan` takes a `Graph`: each object's id and what it refers to, the pages in order, the page tree's nodes, what a viewer wants at open, and the groups a hint table names. It gives a `Plan`: what to write in file order, each object's new number, and what each object stream holds (F.3).
- **Group objects by the pages that use them, with no plan.** `plan::page_uses` says which pages use each object, and `plan::group` which objects share an object stream. A writer that doesn't linearize can pack by page with them too.
- **Assemble the bytes around your bodies.** Write each of the plan's bodies however you hold them, and give `assemble` their lengths. The `Layout` holds the padded linearization dictionary, both cross-reference sections and the hint stream. `Layout::pieces` says what comes next in the file: its bytes, or one of your bodies. Its own streams go through a closure you give, which filters or encrypts them, so no filter or key is here.
- **Read and write hint tables.** Each of F.4's tables is a typed value in `hint`, written to bits and read back. `hint::tables` reads all a hint stream says it has, from any bytes.
- **Check a file's facts.** `check` takes a file's `check::Facts`, as any model's reader gives them, and says whether the file is linearized, whether it holds and whether its hints read (`check::Verdict`). Each thing wrong is a `check::Finding`, told to a `check::Sink` with the clause it departs from and its place. Its `check::Grade` is `Error` for what a reader relying on the file gets wrong, or `Departure` for what real writers do and viewers go past.
- **Linearize a document.** With `family`, `family::linearize` writes a pdf-structure `Document` as a linearized file, and `family::linearize_bytes` a file's bytes. Either writes cross-reference tables, streams, or streams with object streams. Inherited page attributes are pushed down to each page. A page tree that gives a page no place of its own is refused (`family::PageTree`). On any refusal, `out` is as it was given.
- **Write encrypted files.** Under a pdf-structure `Encryptor`, the file's own or a new one, every string and stream that ISO 32000-2 (7.6) encrypts is encrypted under the number it's written at, the hint stream's data too. Under RC4, a string or stream that would go at a keystream the file read used is refused, unless the caller accepts what that leaks: an encryptor told to reuse them, or `family::Rc4` for `linearize_bytes`. `family::Linearized::keystreams_reused` then lists each object so written.
- **Check a file you've read.** `family::check` and `family::facts` read a file's facts from a pdf-structure `Reading`: its sections, its hint data, decrypted where a password opened it, its page tree, and the graph of its objects. `family::check_with_reading` and `family::facts_with_reading` take a file already read whole. A page tree's node in an object stream the reader doesn't decode reads as damaged, as pdf-structure's reader gives it, so the tree is judged without it.
- **Tell what wasn't judged from what's wrong.** What the reader didn't read for its own reason is said in the facts (`check::Unread`): no key, a cap it reads under, or a filter it doesn't take. A `/Filter` or `/DecodeParms` that isn't one is the file's. Each such part, and what's past a cap the check reads under, is one finding of `check::Grade::Unjudged`, and nothing that depends on it is judged. It never makes a file not hold, and `check::Verdict::judged_whole` says whether any part wasn't judged.
- **Read hostile files.** `Limits` caps the numbers a file uses, its pages, the shared lists' entries and visits, and the entries hint tables can ask for. Past a cap, a plan is refused (`plan::Error`), so is a table (`hint::Error`), and the check says what it didn't judge. With `family`, a page tree is walked under `Limits::pages` too, and pdf-semantics' default caps on its depth. Nothing panics on any graph, any hint table's bytes or any facts.
- **Share what it gives across threads.** `Plan`, `Layout`, `check::Facts` and `family::Linearized` are `Send` and `Sync`. A `Debug` of facts or of a layout holds no byte of the hint data or of the layout's bytes.

With `()` as the sink, the check looks for nothing the verdict doesn't need, and doesn't plan the file's graph. The verdict is the same whatever the sink. A part not judged is then said by `check::Verdict::judged_whole`, false where the hint stream, its tables, the sections or the page tree weren't. The file's graph is judged only for a sink that wants findings.

## Features

- **`family`**, on by default: the `family` module, over pdf-structure and pdf-semantics. Without it, the engine takes pdf-objects, thiserror and bitstream-io alone.
- **`zlib-rs`**, off by default: pdf-filters' zlib-rs backend, through pdf-structure, so `family::Options`' `deflate` and `read` can name it.
- **`tracing`**, off by default: a span at `debug` over a plan, an assembly and each round of its first-page section, and a check, through [tracing](https://crates.io/crates/tracing). With `family`, a document's linearizing and a file's facts are spans too, each stage and read a span within them. A span's fields are numbers, never a byte of a file. An object, a page, a table's entry and a finding have none.
- **`insecure-test-writer`**, off by default, for fuzzing, with `family`: it turns on pdf-structure's test writer, which draws a new file's key from a seeded generator and derives every IV, so a fuzz input writes the same bytes each run. It's never for a document's protection, and a build that ships leaves it off.

## An example

A plan of two pages that share a font, and the bytes around its bodies:

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

## How it's held to the standard, qpdf and PDFium

It follows ISO 32000-2's Annex F, written in this crate's own code. Each item of a hint table runs across its entries to a byte boundary, as qpdf, PDFium and real files have it, where F.4.1 has the fields run on regardless of bytes. The repository's `tests/qpdf.rs` holds what's written to qpdf's `--check-linearization` (12.4.2). qpdf warns of some files ISO 32000-2 allows, and the tests say why for each. `tests/pdfium` holds the check to PDFium's availability API as of chromium/8125, and `tests/pdfium/differ.tsv` lists where the check reads a case otherwise. Those files aren't in the package.

## Exhaustive on purpose, or not

`plan::What`, `layout::Piece` and `plan::Use` are exhaustive, as a writer or a grouping must handle every one. So are the hint tables' headers, entries and tables, and `check::Attribute`, as the standard's tables and its list of inherited attributes are closed. Every other enum and every struct with public fields is `#[non_exhaustive]`, so each can grow in a minor release.

## What isn't here

- **I/O and bodies, in the engine.** It's told lengths, and gives back what goes around them.
- **Another model's documents.** The engine takes any model's objects as plain data, and `family` is pdf-structure's adapter. The repository's pdf-linearize-lopdf, never published, is lopdf's.
- **Repair.** A file that doesn't hold is told so by `check`, not put right. A page tree that gives a page no place of its own is refused, not rebuilt.
- **A page label table or an overflow hint stream, written.** Annex F leaves both optional. In a file read, both are read.
- **Signatures.** The file is written anew, so a signature's byte range doesn't hold afterwards. Whether to rewrite a signed file is the caller's choice.

## Stability

This is an alpha: any alpha may change the API, and the CHANGELOG says how to follow. Depend on the exact version, `pdf-linearize = "=0.1.0-alpha.1"`. It needs Rust 1.89 or later.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
