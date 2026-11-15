# pdf-content-streams

PDF content stream lexer, operator parser, analyzer, writer, and byte-preserving splicer (ISO 32000-2, §7.8.2), matching PDFium's parsing rules as of chromium/8152.

`pdf-content-streams` operates on decoded stream bytes and resource dictionaries without performing file I/O directly. Built on [pdf-tokenizer](https://crates.io/crates/pdf-tokenizer), [pdf-objects](https://crates.io/crates/pdf-objects), [pdf-filters](https://crates.io/crates/pdf-filters), and [pdf-color](https://crates.io/crates/pdf-color), it re-exports `pdf-objects` and `pdf-color` for version-matched interop.

## Core Concepts

- **Token & operation lexing (`tokens`, `operations`, `Operand`):**
  - `tokens` yields byte ranges and raw token slices bounded exactly where PDFium's content parser ends them (including arrays, dictionaries, and inline images).
  - `operations` groups operands with their trailing operator (`Operator` typed per ISO 32000-2 Annex A, alongside `Category` and graphics-object `State` transitions).
  - `Operand` defers value parsing until requested (`name`, `Operand::number`, `object`), and optional `fault::Sink` hooks report where ISO 32000-2 differs from PDFium at zero cost when `()` is passed.
- **Inline-image boundary detection (`InlineImage`, `ResCx`):** Unfiltered inline images are sized from their image dictionary and resource color space component counts (`ResCx::read`); Flate and LZW inline images decode via `pdf-filters` to find where PDFium stops, and optional `image-ends` support decodes DCT (JPEG) and CCITTFax streams.
- **Multi-stream pages (`Join`, `Concat`):** Concatenates a page's `/Contents` stream array with separating spaces (matching PDFium) while mapping byte spans back to individual streams (`Pos`, `SpanAcross`).
- **Page analysis (`balance`, `marked`, `names_used`, `kept`):** Inspect `q`/`Q` graphics-state balance, marked-content sequences (`BMC`/`BDC`/`EMC`) with resolved property lists, and referenced resource names—either as whole-page passes or single-operation steppers (`Balance::take`, `MarkedNest::step`, `NameSet::take`).
- **Content writing & deduplicated resources (`ContentWriter`, `Res`, `Reserved`):** Emit canonical PDF content operators while deferring resource naming (`ContentWriter::named`) so generated names avoid collisions with existing `/Resources` bindings (`Reserved::read`). `Res` hashes resource object graphs via XXH3-128 (`ResId`) so identical resources deduplicate cleanly in maps.
- **Safe byte splicing (`splice`, `splice_each`, `without_keys`):** Remove byte ranges (such as marked-content blocks) from a page's streams while preserving surrounding bytes verbatim, rejecting removals that could alter downstream graphics state and re-verifying spliced output.
- **Resource bounds & log hygiene (`Limits`):** Bounds operand counts, nesting, inline-image work, and splice checks before allocation, never panics on untrusted content, and omits raw string/image bytes from `Debug` output.

## Quickstart

Join two page content streams from a stream provider, iterate operations, and inspect graphics balance and marked content:

```rust
use pdf_content_streams::pdf_objects::{
    Object, ObjectId, Unreadable, Why, WithStreams, resolve_fn,
};
use pdf_content_streams::{Join, Limits, Operator, Pos, Properties, ResCx, balance, marked};

// A reader that gives streams 4 and 5, decoded.
let nothing = resolve_fn(|_: ObjectId| None::<&Object>);
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

Write content and assign collision-free resource names at finalization:

```rust
use pdf_content_streams::pdf_objects::{Object, ObjectId, dict, resolve_fn};
use pdf_content_streams::{ContentWriter, Limits, Res, Reserved};

let font = Res::dict(dict! {
    "Type" => Object::name("Font"),
    "Subtype" => Object::name("Type1"),
    "BaseFont" => Object::name("Helvetica"),
})
.unwrap();
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

// Avoid colliding with an existing `/Resources` binding for a different font:
let objects = [Object::from(dict! {
    "Type" => Object::name("Font"),
    "BaseFont" => Object::name("Courier"),
})];
let resolve = resolve_fn(|id: ObjectId| objects.get((id.number as usize).checked_sub(1)?));
let resources = Object::from(dict! { "Font" => dict! { name.as_str() => Object::reference(1) } });
let (_, renamed) = w.named(&Reserved::read(resolve, Some(&resources), &Limits::DEFAULT));
assert_ne!(renamed.0[0].name, needs.0[0].name);
assert_eq!(renamed.0[0].res, font);
```

## Cargo Features

- **`image-ends`** *(optional)*: Enables exact DCT ([zune-jpeg](https://crates.io/crates/zune-jpeg)) and CCITTFax ([hayro-ccitt](https://crates.io/crates/hayro-ccitt)) inline-image end detection when `Limits::coded_ends` is set to `CodedEnds::Decoded`.
- **`simd`** *(optional)*: Enables runtime CPU SIMD dispatch in `memchr`, `twox-hash`, and (when enabled) `zune-jpeg`.
- **`tracing`** *(optional)*: Emits `tracing` spans (byte lengths and counts only, never content) around page passes, splices, resource reads, and `ContentWriter::named`.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
