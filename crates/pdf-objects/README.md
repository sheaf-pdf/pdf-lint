# pdf-objects

PDF's objects, in one small Rust crate with no dependencies, as a reader read them: owned, each value with where it was written, and each dictionary as written, in order and with any key written twice (ISO 32000-2, §7.3). And text strings, decoded by the spec (§7.9.2.2).

It's made for a reader that checks files as much as for one that shows them. What was written stays, so what reads the objects can point at the bytes a value came from, see a key written twice, and say where a string departs from the spec.

## What it does

- **Where each value was written.** An `Object` is a `Value` and its `Span`, when the reader knows: a run of the file's bytes, or of an object stream's decoded data (`Span::stream`).
- **Dictionaries as written.** A `Dict` keeps its entries in order, a key written twice and each key's span among them. A lookup (`get`, `entry`, `iter`) takes a key's last entry, as readers do; `duplicates` gives the others, and `entries` every one.
- **Strings as their bytes.** A `Str` is `Bytes`, its escapes decoded; or `Text`, text a reader decoded without saying from which encoding; or `Encrypted`, ciphertext it didn't decrypt. Of those two, what depends on the bytes can't be told, and `as_bytes` gives none.
- **Text strings decoded by the spec.** `text::decode` reads UTF-16BE or UTF-8 after their byte order marks, and PDFDocEncoding (Annex D) without one, and says each `Quirk`, where the string departs from the spec: UTF-16LE's mark, which PDF doesn't have, an odd last byte, a lone surrogate, UTF-8 that isn't, a byte PDFDocEncoding leaves undefined.
- **Versions.** A `Version` is read from a header's `%PDF-1.7` or a catalog's `/Version`, and versions order.

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

## What isn't here

- **A file's structure:** the cross-reference sections, the trailer, object streams, decrypting and decoding. Those are a reader's, which places what it reads with `Span`s.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) or MIT license ([LICENSE-MIT](LICENSE-MIT)) at your option.
