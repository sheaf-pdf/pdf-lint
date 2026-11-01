# pdf-linearize

PDF files laid out for reading over a network (ISO 32000-2, Annex F), in a small Rust crate with no object model of its own and no I/O: a caller describes its objects as plain data, whichever model holds them.

It's made for what writes PDF files a viewer can show a page of before the rest arrives, and for what checks a file that says it's one. So far it plans a file's layout; what writes one, and what checks one, are to come.

## What it does

- **The plan (F.3).** `plan` takes a `Graph`, each object's id and what it refers to, the pages in order, the page tree's nodes, what a viewer wants at open and the groups a hint table names, and gives a `Plan`: what a caller serializes, in file order, each object's new number, and which objects each object stream holds. The first page's objects, shared or not, go first, then each other page with what it alone uses, what later pages share, and the rest, numbered as a linearized file's two groups are, what's in an object stream highest in its group and the hint stream last.
- **Which pages use an object.** `Use` says whether one page uses it, several or none, from a walk from each page that never goes into another page, a tree node or what's at open, and that goes into an object twice at most, however many pages share it, on a stack of its own.
- **Object streams.** `group`, a pure function of each object's number and its `Use`, says which objects share one, so a writer that doesn't linearize can use it too. A plan calls it once per region, so a stream holds one region's objects; one with several pages' members is one entry of the shared table, not a cost in each page's section. Under `Kind::Table` nothing's packed, and a packable object is refused.
- **Each page's shared objects (F.4.2).** `Plan::shared` lists each later page's entries of the shared object table: all it reaches, under what's shared too. It's a walk per page, so it costs what the table it fills costs.

Caps are one value, `Limits`, on the numbers a file uses, its pages and the shared lists' entries and visits: past one, a typed `plan::Error`, so that a crafted graph where a million pages reach a million objects is refused, not run. Nothing panics on a graph, and references to no object count for nothing.

## An example

```rust
use pdf_linearize::{Graph, Kind, Limits, Page, Part, What, plan};

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
```

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.
