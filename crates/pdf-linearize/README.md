# pdf-linearize

PDF files laid out for reading over a network (ISO 32000-2, Annex F), in a small Rust crate with no object model of its own and no I/O: a caller describes its objects as plain data, whichever model holds them.

It's made for what writes PDF files a viewer can show a page of before the rest arrives, and for what checks a file that says it's one. So far it plans a file's layout and assembles what goes around a caller's bodies, the hint tables among it; what checks one is to come.

## What it does

- **The plan (F.3).** `plan` takes a `Graph`, each object's id and what it refers to, the pages in order, the page tree's nodes, what a viewer wants at open and the groups a hint table names, and gives a `Plan`: what a caller serializes, in file order, each object's new number, and which objects each object stream holds. The first page's objects, shared or not, go first, then each other page with what it alone uses, what later pages share, and the rest, numbered as a linearized file's two groups are, what's in an object stream highest in its group and the hint stream last.
- **Which pages use an object.** `Use` says whether one page uses it, several or none, from a walk from each page that never goes into another page, a tree node or what's at open, and that goes into an object twice at most, however many pages share it, on a stack of its own.
- **Object streams.** `group`, a pure function of each object's number and its `Use`, says which objects share one, so a writer that doesn't linearize can use it too. A plan calls it once per region, so a stream holds one region's objects; one with several pages' members is one entry of the shared table, not a cost in each page's section. Under `Kind::Table` nothing's packed, and a packable object is refused.
- **Each page's shared objects (F.4.2).** `Plan::shared` lists each later page's entries of the shared object table: all it reaches, under what's shared too. It's a walk per page, so it costs what the table it fills costs.

- **The bytes around the bodies (F.3).** A caller writes each of the plan's writes, whole, however it holds them, and gives `assemble` their lengths, as nothing in a body depends on an offset. It gives a `Layout`: the header, the dictionary, the first-page section, the hint stream and the main section as bytes, whose `pieces` say what comes next in the file, those or a caller's body. The hint stream and any cross-reference streams go through a closure the caller gives, which filters or encrypts them, so no filter or key is in here. The dictionary is padded to a fixed length; a table's `/Prev` has room for any position; a stream section is given room, and goes round if what it filters to doesn't fit.
- **The hint tables (F.4), both ways.** The page offset, shared object, thumbnail, generic, extended and embedded file tables as typed values, each `write` to bits and `read` back, and `hint::tables` for all a hint stream's dictionary says it has. Reading any bytes, a width past 32 or a field past the data is a typed `hint::Error`, and a count that wants more bits than there are is refused before there's room made for it. Each table's every item is on the byte boundary every reader and real file puts it on; every position is as if the hint stream weren't there.

Caps are one value, `Limits`, on the numbers a file uses, its pages, the shared lists' entries and visits and the entries hint tables can ask for: past one, a typed error, so that a crafted graph where a million pages reach a million objects is refused, not run, and a hint stream of a hundred bytes can't ask for four billion of anything. Nothing panics on a graph or on a table's bytes, and references to no object count for nothing.

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

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.
