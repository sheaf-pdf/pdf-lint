# Changelog

## 0.1.0-alpha.1 (unreleased)

First release. It's an alpha: any alpha may change the API, and each entry here says what changed and how to update. Depend on the exact version, `=0.1.0-alpha.1`; a plain `0.1.0-alpha.1` lets cargo take later alphas too.

### Added

- `tokens` and `operations`: a content stream's tokens, and its operations with the operands before each, read as PDFium (chromium/8125) reads them. Each has its range and its bytes as written. An `Operand` is read as a value only when asked, by `name`, `Operand::number` or `object`.
- `InlineImage`: an inline image's dictionary and data, its end found as PDFium finds it, and what PDFium keeps of the data (`InlineImage::stream`). `ResCx::read` counts the color spaces a resources dictionary names, so an image drawn in one is sized by it.
- The `image-ends` feature: DCT and CCITTFax data's ends found by decoding it, where a reading asks (`Limits::coded_ends`). Without it, a stand-in finds them in the data's bytes.
- `Concat` and `Join`: a page's streams read as one, each place mapped back to its stream (`Pos`, `SpanAcross`). `Join::push_from` takes them from any reader of a file, through pdf-objects' `Streams`.
- `Operator`, `Operators` and `State`: each operator of ISO 32000-2's Annex A, with its operands and its category, and where it may be used.
- `balance`, `marked`, `names_used` and `kept`: a page's `q`/`Q` balance, its marked content and the resource names it uses. `Balance::take`, `MarkedNest::step` and `NameSet::take` count an operation at a time, so one pass can find several.
- `fault::Fault` and `fault::Sink`: each place ISO 32000-2 reads content otherwise than PDFium, told with where it is. With `()` as the sink, none is looked for. A `Vec` keeps each with its place.
- `ContentWriter`: content written, the same bytes for the same calls. It names its resources (`Res`) when it's finished, clear of what a page's resources bind (`Reserved::read`). Two resources are one only where they're equal, so a `HashMap<Res, _>` keeps each once.
- `splice`, `splice_each` and `without_keys`: ranges taken out of a page's content, or keys out of a property list, only where nothing drawn after them can change. What's kept is read again before it's given.
- `Limits`: caps on what reading and editing may hold or cost, each met before the work it bounds. `InlineImage::capped` says an image was sized by a color space counted past a cap, and the edits refuse content that holds one (`SpliceError::Capped`).
- The `simd` feature: the dependencies' SIMD, picked as they run. The `tracing` feature: a span over each of a page's passes and edits, the writer's names and a resources dictionary's reading. Both are off by default.

### Notes

- Where its reading differs from PDFium's, the repository's `tests/pdfium/differ.tsv` lists the case and why.
- It's built on pdf-tokenizer `0.2`, pdf-objects `=0.1.0-alpha.2`, pdf-filters `=0.1.0-alpha.3` and pdf-color `=0.1.0-alpha.1`. pdf-objects and pdf-color are re-exported whole, so their types are named at those versions. It needs Rust 1.89.
- It reads no file and keeps no image's pixels. Putting the writer's resources into a file is the caller's.
- It never panics on content. A `Debug` never prints a string's or an image's bytes, and a fault's text shows 40 bytes at most.
- `ResId`, a resource's XXH3-128 digest, names a resource and finds it, but doesn't decide what it is. A resource holds its streams' data in memory: `Res::of` and `Res::named` refuse data a reader left in the file (`Unheld`).
- The standard's own closed lists are exhaustive on purpose, so a match on every value is checked: `Tok`, `Props`, `MarkedEvent`, `Paint`, `FillRule`, and the writer's `LineCap`, `LineJoin`, `TextMode` and `Blend`. Every other enum is `#[non_exhaustive]`, as are `Limits`, `GState` and what a reading gives (`Balance`, `Marked`, `Kept`), so each can grow in a minor release. The small structs a caller builds or takes apart by their fields, an error, a place or a need, are exhaustive, each doc saying so. A `Res` is built by its constructors alone.
