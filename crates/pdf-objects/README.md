# pdf-objects

PDF's objects, and their parser, in one small Rust crate on [pdf-tokenizer](https://crates.io/crates/pdf-tokenizer): owned, each value with where it was written, and each dictionary as written, in order and with any key written twice (ISO 32000-2, §7.3). And text strings, decoded by the spec (§7.9.2.2).

It's made for a reader that checks files as much as for one that shows them. What was written stays, so what reads the objects can point at the bytes a value came from, see a key written twice, and say where a string departs from the spec; and the parser says every fault it reads past.

## What it does

- **Where each value was written.** An `Object` is a `Value` and its `Span`, when the reader knows: a run of the file's bytes, or of an object stream's decoded data (`Span::stream`).
- **Dictionaries as written.** A `Dict` keeps its entries in order, a key written twice and each key's span among them. A lookup (`get`, `entry`, `iter`) takes a key's last entry, as readers do; `duplicates` gives the others, and `entries` every one.
- **Strings as their bytes.** A `Str` is `Bytes`, its escapes decoded; or `Text`, text a reader decoded without saying from which encoding; or `Encrypted`, ciphertext it didn't decrypt. Of those two, what depends on the bytes can't be told, and `as_bytes` gives none.
- **Text strings decoded by the spec.** `text::decode` reads UTF-16BE or UTF-8 after their byte order marks, and PDFDocEncoding (Annex D) without one, and says each `Quirk`, where the string departs from the spec: UTF-16LE's mark, which PDF doesn't have, an odd last byte, a lone surrogate, UTF-8 that isn't, a byte PDFDocEncoding leaves undefined.
- **Versions.** A `Version` is read from a header's `%PDF-1.7` or a catalog's `/Version`, and versions order.
- **A parser.** `parse::Parser` reads a value, or an indirect object, from a file's bytes or an object stream's decoded data, from any offset, each value placed. What it reads past, an integer too large or a name's stray `#`, say, is a `Fault`, which it tells a `Sink` as it reads: a checker keeps them, and `()` drops them. What stops a value, an array that doesn't end, say, is an `Error`, with where. Arrays and dictionaries nest at most `NESTING` (256) deep.
- **A stream's two extents, the choice the caller's.** An indirect object is read in steps: `header`, `n g obj`; `body`, its value, or a stream's dictionary to where its data starts; and `stream`, the rest. A stream's data may be as long as its `/Length` says, when `endstream` follows it (`declared`), or run up to the first `endstream` (`scanned`), and readers choose differently, so the caller resolves the `/Length`, which may be a reference, and passes `stream` the one it chose.

## An example

```rust
use pdf_objects::text::{self, Encoding, Quirk};
use pdf_objects::{Object, Span, Version, dict};

// A dictionary as written, /Rotate twice: a lookup takes the last.
let page = dict! { "Type" => "Page", "Rotate" => 90, "Rotate" => 180 };
assert_eq!(page.get(b"Rotate").and_then(Object::as_integer), Some(180));
let earlier: Vec<_> = page.duplicates().map(|e| e.value.as_integer()).collect();
assert_eq!(earlier, [Some(90)]);

// A value says where it was written: here, bytes 40 to 47 of the file.
let rotate = Object::from(180).at(Span::new(None, 40, 47));
assert_eq!(rotate.at.map(|at| (at.stream, at.start, at.end)), Some((None, 40, 47)));

// A text string, by its byte order mark, and where it departs from the spec.
let title = text::decode(b"\xFE\xFF\x00H\x00i\x00");
assert_eq!((title.text.as_str(), title.encoding), ("Hi", Encoding::Utf16Be));
assert_eq!(title.quirks, [Quirk::OddByte]);

// A version, as a header or a catalog's /Version writes it.
assert_eq!(Version::parse(b" 2.0\n"), Some(Version::new(2, 0)));
assert!(Version::new(1, 7) < Version::new(2, 0));
```

And the parser, on an indirect object whose stream's `/Length` holds:

```rust
use pdf_objects::parse::{Body, Fault, Parser, Sink};
use pdf_objects::{Span, Value};

// A checker's sink keeps each fault; `()` would drop them.
struct Said(Vec<String>);
impl Sink<'_> for Said {
    fn fault(&mut self, _at: Span, fault: Fault<'_>) {
        self.0.push(fault.to_string());
    }
}

let file = b"4 0 obj\n<< /Length 5 /N#00 1 >>\nstream\nhello\nendstream\nendobj\n";
let mut said = Said(Vec::new());
let mut parser = Parser::at(file, 0);
let id = parser.header().unwrap();
let Body::Stream(head) = parser.body(id, &mut said).unwrap() else { panic!() };
// The caller resolves /Length, and chooses: here, by it when it holds.
let length = head.dict().get(b"Length").and_then(|l| l.as_integer());
let declared = length.and_then(|len| head.declared(usize::try_from(len).ok()?));
let data = declared.unwrap_or_else(|| head.scanned());
let stream = parser.stream(head, data, &mut said).unwrap();
let Value::Stream(s) = &stream.value else { panic!() };
assert_eq!(&file[data.start..data.end], b"hello");
assert_eq!(s.data, Some(data));
assert_eq!(said.0, ["the name /N#00 has #00, NUL, which a name can't hold, which readers read differently"]);
```

## What isn't here

- **A file's structure:** the cross-reference sections, the trailer, object streams, decrypting and decoding; and which of a stream's extents is its data. Those are a reader's, which places what it reads with `Span`s, and reads each object with the parser.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) or MIT license ([LICENSE-MIT](LICENSE-MIT)) at your option.
