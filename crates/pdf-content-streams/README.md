# pdf-content-streams

PDF content streams (ISO 32000-2, §7.8.2), read as PDFium reads them, in a small Rust crate on [pdf-objects](https://crates.io/crates/pdf-objects): each token with where it is and its bytes as written, an inline image's data found by PDFium's rules, each operation with its operands held in the item, operands read as values only when asked, a page's streams joined as one, every place in them mapped back to the stream it's in, each operator typed by ISO 32000-2's Annex A, with the operands it takes and where it may be used, and a page's `q`/`Q` balance, its marked content and the resource names it uses.

It's made for what reads content to draw it, check it or edit it. A token is a range of the bytes read, so what it says can point at them; an operation of sixteen operands or fewer allocates nothing; nothing is read that isn't asked for; and no file is read here: the caller gives the streams, decoded, what an inline image's size depends on, and the property lists marked content names.

## What it does

- **Tokens.** `tokens` gives a stream's tokens in order, each with its range: an operator, any word that isn't a number, `true`, `false` or `null`; an operand as written, a number, a name, a string, an array or a dictionary with all it holds, or a closing delimiter with nothing to close, which PDFium passes over; and an inline image, `BI` to `EI`, one token, with where its data is. Between them is only white-space and comments. Tokens end where PDFium's `CPDF_StreamParser` ends them: a word of digits, signs and points is a number, and strings nest. What an operator does is no part of it, so one nobody knows is a token like any other.
- **Inline images, as PDFium finds their data** (`ReadInlineStream`): an unfiltered image by its exact size, from `/W`, `/H`, `/BPC` and its color space's components; `/AHx`, `/A85` and `/RL` to their ends as PDFium's decoders find them; then on to the next `EI`, its tokens read as PDFium reads them. A filter PDFium can't decode, one of those that finds no end, or an image no size counts reads on from `ID` to the next `EI`; `/L` and `/Length` PDFium doesn't read. Where PDFium decodes the data to find its end, `/Fl`, `/LZW`, `/CCF` and `/DCT`, it isn't decoded here: a JPEG ends at its end of image, and the others, or a JPEG with none, at the first `EI` with white-space before it and white-space, a delimiter or the end after, which agree but where the encoded bytes hold such an `EI`. The dictionary is read as PDFium reads it: a key as written, its `#` escapes not decoded, an abbreviated name as the full one, and of a key written both abbreviated and in full the abbreviation's value, unless the full name's is an abbreviated name. The components of a color space the resources name are the caller's to give (`ResCx`), counted as PDFium counts them (`family_components`).
- **Operations.** `operations` gives a stream's operations in order, a standard iterator of `Operation`s: each operator, or inline image, with the operands before it, its tokens grouped. An operation's operands' ranges are held in the item, sixteen of them, as many as PDFium keeps, before they spill to the heap; every operand is kept, up to the cap. Operand `i` is taken from the last, as PDFium takes them (`arg`), and `operands` gives them all in order. Operands at the end, which no operator takes, are no operation. An inline image is one operation, `BI` to `EI` (`InlineImage`): where its dictionary and its data are, the value under a key, either spelling, as PDFium reads it (`entry`), and the length it gives its data (`length`).
- **Operands read when asked.** An `Operand` is its bytes as written and where they are, and says what kind it is from how it starts: a number, a name, a string, an array or a dictionary. `name` decodes a name's `#` escapes, borrowing it when there are none. `object` reads any operand as a pdf-objects `Object`: numbers, names, strings in their form, literal or hex, and arrays and dictionaries, a dictionary keeping every entry as pdf-objects' `Dict` does, in one pass however deep. `elements` gives an array's elements as written, each where it is, gathering nothing.
- **Operators.** `Operator` types each operator of ISO 32000-2's Annex A by its name, as written (`Operator::of`, `Operation::operator`), `Other` for any other word, and gives what the clause defining it says: the count and types of its operands (`Operands`, `Type`; `SC` and `sc` take a number for each component of the color space, `SCN` and `scn` the same or a pattern's name), its category of 8.2 (`Category`) and the clause. `paint` says how a painting operator paints, `F` as `f`, `clip` the rule `W` or `W*` clips by, and `builds_path` which build the path. `Operators` is a set of them, each category one. `State` says where each may be used, among the graphics objects of 8.2 (its Figure 9, with the errata: `q`, `Q` and marked content in a text object too): the content stream level, a path, a clipping path, a text object, an inline image, and the start of a Type 3 glyph description, where `d0` or `d1` comes first and nowhere else. `after` gives the state an operator leaves, where it's allowed, and `step` an operation's, an inline image `BI`, `ID` and `EI` in turn. Holding content to them, an operator's operands or where it's used, is the caller's.
- **A page as one.** `Concat` joins a page's `/Contents` streams with a space between each, as PDFium joins them (one stream is borrowed as it is), and maps an offset in them back to a stream and a place in it (`Pos`, `SpanAcross`), and back again. A string or an inline image can still run from one stream into the next, as in PDFium. `Join` joins streams as they're read, under a cap, and decodes each straight into the joined bytes when its decoder appends to a buffer, as pdf-filters' `decode_into` does: a stream past the cap, or that won't decode, is refused, the bytes cut back to where they were.
- **Balance, marked content and names.** `balance` counts a page's `q`s and `Q`s (`Balance`): the net count, and the lowest the running count goes, below 0 where a `Q` restores a state the content didn't save; ISO 32000-2 asks that they balance, a page's streams read as one (8.4.2). `marked` finds its marked content (14.6), each range a `BMC` or `BDC` opens and its `EMC` ends (`Marked`): its tag, its property list, written in the content or named and looked up in the lists the caller gives (`Properties`, its resources' `/Properties`), borrowed so ranges naming one list share it, where it is, from one stream into another, and how many ranges it's inside; an `EMC` that ends none, a range still open at the end, or ranges nested past the cap are errors. `names_used` gives the resource names content uses, by category (`ResKind`, `NameSet`): `Tf`'s font, `Do`'s external object, `gs`'s graphics state, `sh`'s shading, a color space but for the device ones, an inline image's too, a pattern and a named property list; and `kept` what content keeps once ranges of it are taken out, its balance and names, and the names only those ranges use, the ranges in any order and overlapping, each operation found in them by one search. Each is counted an operation at a time, `Balance::take`, `MarkedNest::step`, told as each range opens and ends (`MarkedEvent`), with those still open (`open`), and `NameSet::take`, from the resource an operation names (`Operation::resource`), so a reader stepping through content for more than one finds them all in one pass.
- **Caps, and errors that say where.** It reads under `Limits`: operands before an operator, how deep arrays and dictionaries nest, an inline image's data, and a page's streams joined. A string, array, dictionary or inline image that doesn't end, or one past a cap, is an `Error` with where it starts, and nothing comes after it. It never panics, whatever the bytes, and every token moves it on.
- **Where ISO 32000-2 reads it otherwise.** Read by PDFium's rules, content can mean something else by ISO's: a word of digits, signs and points that's a number to PDFium and a keyword to ISO, or a number past what's read; a hex string with a byte that's no hex digit; a name with a `#` that escapes nothing, or NUL; a closer with nothing to close, or a brace; inside an array, a dictionary or an inline image's dictionary, a word that's no object, a key that's no name, or a key with no value; an inline image's key with a `#` escape, which ISO decodes and PDFium doesn't, or written both abbreviated and in full where PDFium reads the full name's value and ISO the abbreviation's; a string at any depth, an inline image's data, or a comment that reads a token as comment, running from one of a page's streams into the next, where ISO lets them divide only between its tokens; operands at the end that no operator takes; and an inline image whose length ends its data elsewhere than it's found, whose `ID` has no white-space after it, or with tokens before its `EI`. Each is a `fault::Fault`, which names its clause and whether it's a "shall", told with where it is to a `fault::Sink` the caller gives: `with_sink` on the tokens or the operations, and `object_with` for what's inside an array or a dictionary, read when asked. What's read is the same through any sink. The sink is a type parameter, `()` unless one's given, which wants no faults, so a reader that doesn't ask for them pays for none.

## An example

```rust
use pdf_content_streams::{
    Balance, Join, Limits, Operator, Pos, Properties, ResCx, State, Tok, balance, marked, name,
    object,
};
use pdf_objects::Object;

// A page's two streams, joined as they're read.
let mut join = Join::new(&Limits::DEFAULT);
join.push(b"q /Span <</MCID 0>> BDC (Hel").unwrap();
join.push(b"lo) Tj EMC Q").unwrap();
let page = join.finish();

let tokens: Vec<_> = page
    .tokens(&ResCx::none(), &Limits::DEFAULT)
    .collect::<Result<_, _>>()
    .unwrap();
assert_eq!(tokens[0], (0..1, Tok::Op(b"q")));

// Operands are their bytes as written, read when asked.
let (_, Tok::Operand(tag)) = tokens[1] else { panic!() };
assert_eq!(name(tag).as_deref(), Some(&b"Span"[..]));
let (_, Tok::Operand(props)) = tokens[2] else { panic!() };
let mcid = object(props, Limits::DEFAULT.depth).unwrap();
let mcid = mcid.as_dict().and_then(|d| d.get(b"MCID"));
assert_eq!(mcid.and_then(Object::as_integer), Some(0));

// The string runs from the first stream into the second, as in PDFium.
let (string, _) = tokens[4].clone();
let span = page.span(string);
assert_eq!(span.start, Pos { stream: 0, at: 24 });
assert_eq!(span.end, Pos { stream: 1, at: 3 });
assert_eq!(span.to_string(), "stream 0 byte 24 to stream 1 byte 3");

// Or each operation, with the operands before it.
let ops: Vec<_> = page
    .operations(&ResCx::none(), &Limits::DEFAULT)
    .collect::<Result<_, _>>()
    .unwrap();
let names: Vec<_> = ops.iter().map(|op| op.name()).collect();
assert_eq!(names, [&b"q"[..], b"BDC", b"Tj", b"EMC", b"Q"]);
// Operand 0 is the last, as PDFium takes them.
let props = ops[1].arg(0).unwrap();
assert!(props.is_dict());
assert_eq!(ops[1].arg(1).unwrap().name().as_deref(), Some(&b"Span"[..]));
assert_eq!(page.span(ops[2].range()).start, Pos { stream: 0, at: 24 });

// Each operator typed, and where it may be used: `Tj` in a text object
// alone.
assert_eq!(ops[2].operator(), Operator::Show);
assert_eq!(State::ContentStream.step(&ops[1]), Some(State::ContentStream));
assert_eq!(State::ContentStream.step(&ops[2]), None);

// Its `q`/`Q` balance, and its marked content, from one stream into the
// next.
let res = ResCx::none();
assert_eq!(balance(&page, &res, &Limits::DEFAULT).unwrap(), Balance::default());
let props = Properties::default();
let found = marked(&page, &props, &res, &Limits::DEFAULT).unwrap();
assert_eq!(found[0].tag, b"Span");
assert_eq!(found[0].span.to_string(), "stream 0 byte 2 to stream 1 byte 10");
```

## What isn't here

- **A file.** Finding a page's streams, decoding them and reading its resources are a reader's, which gives this crate the streams' bytes, the components of the color spaces their inline images are drawn in, and the property lists its marked content names.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.
