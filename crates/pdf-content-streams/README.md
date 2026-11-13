# pdf-content-streams

Read PDF content streams (ISO 32000-2, 7.8.2) as PDFium reads them, write them, and take ranges out of them.

It's made for what reads content to draw it, check it or edit it, and for what writes it. It reads no file. The caller gives a page's decoded streams and what its resources hold, or a reader of the file to take them from. It's built on [pdf-tokenizer](https://crates.io/crates/pdf-tokenizer), [pdf-objects](https://crates.io/crates/pdf-objects), [pdf-filters](https://crates.io/crates/pdf-filters) and [pdf-color](https://crates.io/crates/pdf-color). It re-exports pdf-objects and pdf-color whole, so their types are named at the versions it's built with.

## What it does

- **Read a stream's tokens and operations.** `tokens` gives each token with its range and its bytes as written. `operations` gives each operator with the operands before it. Tokens end where PDFium's content parser ends them, an array or a dictionary among them.
- **Read an operand when you need its value.** An `Operand` is its bytes and where they are. `name`, `Operand::number` and `object` read it as PDFium does, `object` as a pdf-objects `Object`.
- **Find where an inline image ends.** An unfiltered image ends at its size, counted from its dictionary and its color space's components. Flate and LZW data is decoded through pdf-filters to where PDFium's decoders stop. `InlineImage::stream` says what PDFium keeps of the data.
- **Count a page's color spaces.** `ResCx::read` counts each space a resources dictionary names, through the caller's resolver, as PDFium counts it. An inline image drawn in one of those names is sized by it.
- **Read a page's streams as one.** `Concat` joins them with a space between each, as PDFium does, and maps a place in them back to its stream (`Pos`, `SpanAcross`). `Join` joins them as they're decoded, under a cap. `Join::push_from` takes each from any reader of a file, through pdf-objects' `Streams`, as that reader decodes it. `Join::push_contents` takes a page's `/Contents` whole, a stream or an array of them, as PDFium reads it. An element that's no stream is an empty stream there, and one behind a password, past the reader's cap or not given by its source is an error.
- **Know what each operator is.** `Operator` types each operator of ISO 32000-2's Annex A, with its operands (`Operands`) and its category (`Category`). `State` says where each may be used among the graphics objects of 8.2, and `State::step` moves content through them.
- **Check a page's balance, marked content and names.** `balance` counts its `q`s and `Q`s. `marked` finds each marked-content range, with its tag and its property list. `names_used` gives the resource names it uses, by category. `kept` gives the balance and names of what's left once ranges are taken out, and the names it can go without. `Balance::take`, `MarkedNest::step` and `NameSet::take` count an operation at a time, so one pass can find several.
- **See where ISO 32000-2 reads content otherwise.** Each such place is a `fault::Fault`, which names its clause. It's told with where it is to a `fault::Sink` you give: `Tokens::with_sink`, `Operations::with_sink`, `Operand::object_with`. What's read is the same through any sink. With none, `()`, no fault is looked for.
- **Write content.** `ContentWriter` writes each call as an operator and its operands, the same bytes for the same calls. It names the resources it draws with (`Res`) only when it's finished (`ContentWriter::named`), clear of the names a page's resources dictionary binds to something else (`Reserved::read`).
- **Keep each resource once.** A resource is named by a digest of its objects (`ResId`, XXH3-128), the same on every build. Two resources are one only where they're equal, their objects the same, so a `HashMap<Res, _>` keeps each once. A resource holds its streams' data in memory: `Res::of` and `Res::named` refuse data a reader left in the file (`Unheld`).
- **Take ranges out of a page.** `splice` gives the page's streams with ranges taken out, every other byte as it was. It refuses a range that could change what's drawn after it (`SpliceError`), and reads what it keeps again before it gives it. `splice_each` takes out as many groups of ranges as it can. `without_keys` writes a marked range's property list again without some of its keys.
- **Read hostile content.** Reading is bounded by `Limits`: operands, nesting, values, what finding images' ends may cost, marked content and what `splice_each`'s checks may read. Each cap is met before the work it bounds. Past most, reading stops with an `Error` that says where. A color space counted past its caps counts as one that doesn't load, and an inline image it sizes says so (`InlineImage::capped`). The edits refuse content that holds such an image. It never panics on content.
- **Keep content out of logs.** A `Debug` shows where content is and how long, and an operator's name, never a string's or an image's bytes. A fault's text (its `Display`) shows 40 bytes at most, cut at a character.

## Features

- **`image-ends`**, off by default. A reading that names `CodedEnds::Decoded` in `Limits::coded_ends` finds DCT and CCITTFax data's ends by decoding it, on [zune-jpeg](https://crates.io/crates/zune-jpeg) and [hayro-ccitt](https://crates.io/crates/hayro-ccitt), as PDFium's decoders do. Otherwise a stand-in finds them in the data's bytes. Turning the feature on changes nothing for a reading that doesn't name it.
- **`simd`**, off by default. The dependencies pick the SIMD the CPU has as they run: memchr for its searches, twox-hash for its digest and, with `image-ends`, zune-jpeg for its decoding. Nothing read or digested changes. The `unsafe` code it turns on is theirs.
- **`tracing`**, off by default. A page's passes, its edits, the writer's names and a resources dictionary's reading each run in a span, through [tracing](https://crates.io/crates/tracing). Reading tokens and operations has none, as its loop is the caller's. A span's fields are lengths and counts, never content.

## Examples

A page's two streams, taken from a reader of the file and read as one:

```rust
use pdf_content_streams::pdf_objects::{Object, ObjectId, Unreadable, WithStreams, Why};
use pdf_content_streams::{Join, Limits, Operator, Pos, Properties, ResCx, balance, marked};

// A reader that gives streams 4 and 5, decoded.
let nothing = |_: ObjectId| None::<&Object>;
let streams = WithStreams::new(nothing, |id: ObjectId, _limit: usize| match id.number {
    4 => Ok(b"q /Span <</MCID 0>> BDC BT (Hel".to_vec()),
    5 => Ok(b"lo) Tj ET EMC Q".to_vec()),
    _ => Err(Unreadable::new(id, Why::Damaged, "no such stream")),
});
let mut join = Join::new(&Limits::DEFAULT);
join.push_from(&streams, ObjectId::new(4, 0)).unwrap();
join.push_from(&streams, ObjectId::new(5, 0)).unwrap();
let page = join.finish();

// Each operation, with the operands before it.
let res = ResCx::none();
let ops: Vec<_> = page.operations(&res, &Limits::DEFAULT).collect::<Result<_, _>>().unwrap();
let names: Vec<_> = ops.iter().map(|op| op.name()).collect();
assert_eq!(names, [&b"q"[..], b"BDC", b"BT", b"Tj", b"ET", b"EMC", b"Q"]);
assert_eq!(ops[3].operator(), Operator::Show);

// Operands are read when asked. Operand 0 is the last, as PDFium takes them.
assert_eq!(ops[1].arg(1).unwrap().name().as_deref(), Some(&b"Span"[..]));
let props = ops[1].arg(0).unwrap().object(&Limits::DEFAULT).unwrap();
let mcid = props.as_dict().and_then(|d| d.get(b"MCID"));
assert_eq!(mcid.and_then(Object::as_integer), Some(0));

// The string runs from the first stream into the second, as in PDFium.
let shown = page.span(ops[3].range());
assert_eq!(shown.start, Pos { stream: 0, at: 27 });
assert_eq!(shown.end, Pos { stream: 1, at: 6 });

// The page's `q`/`Q` balance, and its marked content.
assert_eq!(balance(&page, &res, &Limits::DEFAULT).unwrap().net, 0);
let lists = Properties::default();
let found = marked(&page, &lists, &res, &Limits::DEFAULT).unwrap();
assert_eq!(found[0].tag, b"Span");
assert_eq!(found[0].span.to_string(), "stream 0 byte 2 to stream 1 byte 13");
```

Content written, its font named at the end, clear of a name the page's resources bind to another font:

```rust
use pdf_content_streams::pdf_objects::{Object, ObjectId, dict};
use pdf_content_streams::{ContentWriter, Limits, Res, Reserved};

let font = Res::dict(dict! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" }).unwrap();
let mut w = ContentWriter::new();
w.begin_text();
w.font(font.clone(), 12.0);
w.text_move(72.0, 700.0);
w.show(b"Hello, (world)");
w.end_text();
let (content, needs) = w.named(&Reserved::default());
let name = String::from_utf8_lossy(&needs.0[0].name).into_owned();
let want = format!("BT\n/{name} 12 Tf\n72 700 Td\n(Hello, \\(world\\)) Tj\nET\n");
assert_eq!(content, want.as_bytes());

// A page whose resources bind that name to another font, object 1: the
// next name up.
let objects = [Object::from(dict! { "Type" => "Font", "BaseFont" => "Courier" })];
let resolve = |id: ObjectId| objects.get((id.number as usize).checked_sub(1)?);
let resources = Object::from(dict! { "Font" => dict! { name.as_str() => Object::reference(1) } });
let (_, renamed) = w.named(&Reserved::read(resolve, Some(&resources), &Limits::DEFAULT));
assert_ne!(renamed.0[0].name, needs.0[0].name);
assert_eq!(renamed.0[0].res, font);
```

## How it's held to PDFium

It follows PDFium's behaviour as of chromium/8125, written in this crate's own code. Cases in the repository's `tests/pdfium` hold it there, each with PDFium's answer recorded from that build. Where this crate reads a case otherwise, `tests/pdfium/differ.tsv` lists it with why. Those files aren't in the package.

## Exhaustive on purpose, or not

The standard's own closed lists are exhaustive, so a match on every value is checked: a token's kinds (`Tok`), a property list's two forms (`Props`), a marked range's opening and end (`MarkedEvent`), how a painting operator paints (`Paint`, `FillRule`), and the writer's line caps, joins, text modes and blend modes. Every other enum is `#[non_exhaustive]`, as are `Limits`, `GState` and what a reading gives (`Balance`, `Marked`, `Kept`), so each can grow in a minor release. The small structs a caller builds or takes apart by their fields, an error, a place or a need, are exhaustive, each doc saying so. A `Res` is built by its constructors alone.

## What isn't here

- **A file.** Finding a page's streams, decoding them and reading its resources are a reader's. This crate takes what the reader gives.
- **An image's pixels.** Image data is decoded only to find where it ends, and what's decoded isn't kept.
- **Binding what's written.** Putting the writer's resources into a resources dictionary, and its objects into a file, is the caller's.
- **Holding content to the standard's rules for operators.** `Operator` and `State` say what each takes and where it may be used. Checking content against them is the caller's.

## Stability

This is an alpha: any alpha may change the API, and the CHANGELOG says how to follow. Depend on the exact version, `pdf-content-streams = "=0.1.0-alpha.1"`. It needs Rust 1.89 or later.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
