# pdf-linearize

PDF files laid out for reading over a network (ISO 32000-2, Annex F), in a small Rust crate with no object model of its own and no I/O: a caller describes its objects as plain data, whichever model holds them.

It's made for what writes PDF files a viewer can show a page of before the rest arrives, and for what checks a file that says it's one. It plans a file's layout, assembles what goes around a caller's bodies, the hint tables among it, writes a document of pdf-structure's or of lopdf's through both, and checks a file that says it's one, of any model's reading.

## What it does

- **The plan (F.3).** `plan` takes a `Graph`, each object's id and what it refers to, the pages in order, the page tree's nodes, what a viewer wants at open and the groups a hint table names, and gives a `Plan`: what a caller serializes, in file order, each object's new number, and which objects each object stream holds. The first page's objects, shared or not, go first, then each other page with what it alone uses, what later pages share, and the rest, numbered as a linearized file's two groups are, what's in an object stream highest in its group and the hint stream last.
- **Which pages use an object.** `Use` says whether one page uses it, several or none, from a walk from each page that never goes into another page, a tree node or what's at open, and that goes into an object twice at most, however many pages share it, on a stack of its own.
- **Object streams.** `group`, a pure function of each object's number and its `Use`, says which objects share one, so a writer that doesn't linearize can use it too, its `Use`s from `page_uses`, the same walk over ids with no plan. A plan calls it once per region, so a stream holds one region's objects; one with several pages' members is one entry of the shared table, not a cost in each page's section. Under `Kind::Table` nothing's packed, and a packable object is refused.
- **Each page's shared objects (F.4.2).** `Plan::shared` lists each later page's entries of the shared object table: all it reaches, under what's shared too. It's a walk per page, so it costs what the table it fills costs.

- **The bytes around the bodies (F.3).** A caller writes each of the plan's writes, whole, however it holds them, and gives `assemble` their lengths, as nothing in a body depends on an offset. It gives a `Layout`: the header, the dictionary, the first-page section, the hint stream and the main section as bytes, whose `pieces` say what comes next in the file, those or a caller's body. The hint stream and any cross-reference streams go through a closure the caller gives, which filters or encrypts them, so no filter or key is in here. The dictionary is padded to a fixed length; a table's `/Prev` has room for any position; a stream section is given room, and goes round if what it filters to doesn't fit.
- **The hint tables (F.4), both ways.** The page offset, shared object, thumbnail, generic, extended and embedded file tables as typed values, each `write` to bits and `read` back, and `hint::tables` for all a hint stream's dictionary says it has. Reading any bytes, a width past 32 or a field past the data is a typed `hint::Error`, and a count that wants more bits than there are is refused before there's room made for it. Each table's every item is on the byte boundary every reader and real file puts it on; every position is as if the hint stream weren't there.

- **A document of the family (`family`, a default feature).** `family::linearize` writes a pdf-structure `Document`, and `family::linearize_bytes` a file's bytes, in any of three kinds: tables, streams, or streams with object streams, deflated by a `write::Deflate`. Inherited page attributes are pushed down to each page, as written, and taken off the tree's nodes. The rest is copied through pdf-structure's writer where it can be, its references renumbered in place, and its bytes otherwise as they were. The catalog's `/OpenAction` says which page goes first, or `FirstPage::Index` does. A tree that doesn't give each page a place of its own is refused by a typed `family::Error`: a page listed twice or written in place, or one that ISO's rules and PDFium's, as of chromium/8125, read differently, since the numbers and tables depend on which pages a viewer finds. On any refusal `out` is as it was given.
- **Encrypted files.** `family::linearize` under a `write::Encryptor`, the file's own or a new one, and `linearize_bytes` by its passwords: every string and stream encrypted under the number it's written at, the hint stream's data too, a cross-reference stream not; the encryption dictionary and what it refers to in part 4 in the clear. An object as read is copied at its new number only under AES-256. With no encryptor an encrypted document is written in the clear. Under RC4 this layout reuses every number's keystream on another object, as its sections can list none free and every object moves, so one who keeps both files has the XOR of each pair of plaintexts: pdf-crypt's `Method::Rc4` says so, and `Encryptor::create`, a new key, is the way out.
- **A document of lopdf's (`lopdf`, off by default).** `lopdf::linearize` writes a lopdf `Document`, under its `lopdf::Options`, by plain functions over its public objects, and `lopdf::graph` says what a plan's told of it: as `family` places and pushes down, from the same keys, so a document read from one file has the numbers `family` gives that file where the two read the same objects, in the plainest kind, tables, nothing in object streams and the hint stream unfiltered. lopdf has no public writer of one object, so a small one is here. A document that's encrypted, or was when lopdf read it, is refused, as lopdf takes `/Encrypt` out of a file the empty password opens and its encryption works under each object's own number; so is a tree that gives a page no place of its own, whose pages aren't what `Document::get_pages` lists, or with a kid ISO's rules and PDFium's read as different things. It needs lopdf's Rust, 1.88, where the rest of the crate needs 1.85.

- **The check (F.3, F.4).** `check` takes `Facts`, plain data as raw as a reader can give: the file's first object, each section's rows as written, the hint stream's keys and decoded data, the pages in order, and the graph of its objects if there's one. It says a `Verdict`, whether it's linearized, whether it holds and whether its hints read, and tells each `Finding` to a `Sink`: `Vec<Finding>` keeps them, and `()` wants none and stops looking once the verdict's settled, the `Verdict` the same whatever the sink. A hint table is held to the layout, each page and shared entry where adding up the lengths has it, holding what it counts, and what each page reaches to its section and its list where the graph is given. Each finding has a `grade`: `Error` for what a reader relying on the file gets wrong, a position, a length, a count, `Departure` for what real files do and viewers go on past, which most linearized files by real writers have some of. A caller that tells the findings takes what the check judges by from the facts too, so it needn't repeat its rules: `Facts::claim`, the first object where it says the file is linearized; `FirstObject::version_one`, whether `/Linearized` is the version taken; and `Facts::hint_extent`, the hint stream's offset and length a `Place::Position` is located by (`hint::located`). A finding about one of the dictionary's values is placed at that key.
- **Of a reading (`family`).** `family::check` and `family::facts` read those facts from a `pdf_structure::Reading`: its sections by `Reading::sections`, its hint data decrypted where a password opened it, its page tree by pdf-semantics, its graph by a whole read. A file that doesn't say it's linearized has no more read than it takes to say so, and has no findings.

Without default features it's the engine alone, with no object model: `thiserror` and `bitstream-io` are all it takes. The `lopdf` feature adds lopdf alone, without its default features. With `family`, the crates its API is typed in are re-exported at the root, and the `zlib-rs` feature lets `family::Options`' `deflate` and `read` name that Flate backend, as pdf-structure's does.

Caps are one value, `Limits`, on the numbers a file uses, its pages, the shared lists' entries and visits and the entries hint tables can ask for: past one, a typed error, so that a crafted graph where a million pages reach a million objects is refused, not run, and a hint stream of a hundred bytes can't ask for four billion of anything. Nothing panics on a graph, on a table's bytes or on any facts, and references to no object count for nothing.

## An example

```rust
use pdf_linearize::layout::Options;
use pdf_linearize::{
    Encoded, Graph, Kind, Limits, Page, Part, Piece, Stream, What, assemble, hint, plan,
};

// A catalog, a tree node and two pages, which share a font.
let mut graph = Graph::new((1, 0), Kind::Table);
graph.object((1, 0), [(2, 0)]);
graph.object((2, 0), [(3, 0), (4, 0)]);
graph.object((3, 0), [(2, 0), (5, 0), (7, 0)]);
graph.object((4, 0), [(2, 0), (6, 0), (7, 0)]);
for content in [(5, 0), (6, 0), (7, 0)] {
    graph.object(content, []);
}
graph.page(Page::new((3, 0)));
graph.page(Page::new((4, 0)));
graph.tree_node((2, 0));

let plan = plan(&graph, &Limits::DEFAULT).unwrap();

// The font stays with the first page, though the second uses it too.
let first = &plan.writes()[plan.part_writes(Part::FirstPage)];
assert_eq!(first.iter().map(|w| w.what).collect::<Vec<_>>(), [
    What::Object((3, 0)),
    What::Object((5, 0)),
    What::Object((7, 0)),
]);

// The second page is numbered from 1, and lists the font's entry.
assert_eq!(plan.number((4, 0)), Some((1, 0)));
assert_eq!(plan.shared(1), [1]);

// The hint stream is numbered last.
assert_eq!(plan.hint_number(), plan.size() - 1);

// A body for each write, by whatever means; assembled around their lengths, with
// its own streams left as they are.
let lengths: Vec<u64> = plan.writes().iter().map(|_| 40).collect();
let layout = assemble(&plan, &lengths, &Options::new((1, 7)), |stream: Stream| {
    Ok::<_, std::convert::Infallible>(Encoded::plain(stream.data.to_vec()))
})
.unwrap();

// The pieces are the file, in order, and it says where everything is.
let written: u64 = layout
    .pieces()
    .map(|piece| match piece {
        Piece::Bytes(bytes) => bytes.len() as u64,
        Piece::Body(at) => lengths[at],
    })
    .sum();
assert_eq!(written, layout.len());
assert!(layout.position(plan.first_page_number()).unwrap() < layout.first_page_end());

// The tables read back, the second page listing the font's entry.
let tables = hint::tables(layout.hint_data(), layout.hint_keys(), 2, &Limits::DEFAULT).unwrap();
assert_eq!(tables.pages.pages[1].shared, [(1, 0)]);
```

And from a file's bytes, with `family`:

```rust
use pdf_linearize::family::{Options, check, linearize_bytes};
use pdf_linearize::pdf_structure::{Reading, XrefKind, bytes};
use pdf_linearize::{Finding, Limits};

let mut file = b"%PDF-1.7\n".to_vec();
let mut rows = Vec::new();
for (number, object) in [
    (1, "<< /Type /Catalog /Pages 2 0 R >>"),
    (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] /Resources << >> >>"),
    (3, "<< /Type /Page /Parent 2 0 R >>"),
] {
    rows.push(file.len());
    file.extend_from_slice(format!("{number} 0 obj\n{object}\nendobj\n").as_bytes());
}
let at = file.len();
file.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
for row in rows {
    file.extend_from_slice(format!("{row:010} 00000 n \n").as_bytes());
}
file.extend_from_slice(
    format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{at}\n%%EOF\n").as_bytes(),
);

let mut options = Options::DEFAULT;
options.kind = XrefKind::Stream;
options.pack = true;
let linearized = linearize_bytes(&file, &[], &options).unwrap();
assert!(linearized.windows(11).any(|w| w == b"/Linearized"));

// What's written holds, and nothing else is to be said of it.
let reading = Reading::open(&linearized[..], &bytes::Options::DEFAULT).unwrap();
let mut findings: Vec<Finding> = Vec::new();
let verdict = check(&reading, &Limits::DEFAULT, &mut findings).unwrap();
assert!(verdict.linearized && verdict.holds && verdict.hints_read);
assert_eq!(findings, []);
```

## What isn't here

- **A command line.** It's a library; a tool or a viewer's Save As is what calls it.
- **I/O and bodies, in the engine.** It's told lengths, and gives back what goes around them. Writing what the plan says, and filtering or encrypting a stream of its own, is its caller's, or an adapter's.
- **Repair.** A file that doesn't hold is told so by `check`, not put right; writing it again through an adapter makes a new one. A page tree that gives a page no one place is refused, not rebuilt.
- **Much of lopdf's own.** The `lopdf` adapter refuses a document that's encrypted or was, and writes the plainest file there is: tables, nothing packed, the hint stream unfiltered.
- **A page label table written.** Annex F leaves it optional; one in a file read is read.
- **Signatures.** Every object moves, so a signature's byte range doesn't hold afterwards: whether to rewrite a signed file is its caller's choice.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.
