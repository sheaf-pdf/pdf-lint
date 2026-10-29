# pdf-objects

PDF's objects, their parser and their writer, in one small Rust crate on [pdf-tokenizer](https://crates.io/crates/pdf-tokenizer): owned, each value with where it was written, and each dictionary as written, in order and with any key written twice (ISO 32000-2, §7.3). And text strings, decoded by the spec (§7.9.2.2).

It's made for a reader that checks files as much as for one that shows or edits them. What was written stays, so what reads the objects can point at the bytes a value came from, see a key written twice, and say where a string departs from the spec; the parser says every fault it reads past; and the writer writes a value as the parser reads it back.

## What it does

- **Where each value was written.** An `Object` is a `Value` and its `Span`, when the reader knows: a run of the file's bytes, or of an object stream's decoded data (`Span::stream`).
- **Dictionaries as written.** A `Dict` keeps its entries in order, a key written twice and each key's span among them. A lookup (`get`, `entry`, `iter`) takes a key's last entry, as readers do; `duplicates` gives the others, and `entries` every one. A lookup allocates nothing for a dictionary of up to 32 entries.
- **Edits that say what they changed.** An `Object`'s value is read with `value`, and changed with `value_mut`, `as_dict_mut`, `as_array_mut` or `as_stream_mut`, each an edit, which drops its span. A value inside another is reached through each value that holds it, so their spans go too, and a value no edit reached keeps its own: what still has a span is still what its bytes say. `as_written_mut` keeps the span, for a reader changing a value into what the same bytes stand for, as decrypting a string does. A `Dict` is changed by `set`, `push`, `remove`, `get_mut` and `iter_mut`, and `remove_duplicates` removes what `duplicates` gives, leaving each key's last entry.
- **A stream's data** is where it was written (`Data::At`), its bytes in memory (`Data::Bytes`; `Stream::new` and `set_bytes`, which set `/Length`), or neither (`Data::Unplaced`). An edit of its dictionary leaves its data as it was. A `Stream` is made by `Stream::new` or `Object::stream`, or from `Stream::default()`, as it may grow.
- **Strings as their bytes.** A `Str` is `Bytes`, its escapes decoded; or `Text`, text a reader decoded without saying from which encoding; or `Encrypted`, ciphertext it didn't decrypt. Of those two, what depends on the bytes can't be told, and `as_bytes` gives none. Beside it, `Value::String` keeps the string's `Form`, `Literal` or `Hex`, as it was written; a string built without choosing one (`Object::bytes`, where `Object::hex` chooses hex), or read by a reader that doesn't say, has none.
- **Text strings decoded by the spec.** `text::decode` reads UTF-16BE or UTF-8 after their byte order marks, and PDFDocEncoding (Annex D) without one, and says each `Quirk`, where the string departs from the spec: UTF-16LE's mark, which PDF doesn't have, an odd last byte, a lone surrogate, UTF-8 that isn't, a byte PDFDocEncoding leaves undefined.
- **Versions.** A `Version` is read from a header's `%PDF-1.7` or a catalog's `/Version`, and versions order.
- **A parser.** `parse::Parser` reads a value, or an indirect object, from a file's bytes or an object stream's decoded data, from any offset, each value placed and each string in its form. What it reads past, an integer too large or a name's stray `#`, say, is a `Fault`, which it tells a `Sink` as it reads: a checker keeps them, and `()` drops them. What stops a value, an array that doesn't end, say, is an `Error`, with where. It reads under `Limits`: arrays and dictionaries nested at most 256 deep (`NESTING`), and at most 2^25 values in one call, keys among them, unless it's given others, set on a copy of `Limits::DEFAULT`.
- **A stream's two extents, the choice the caller's.** An indirect object is read in steps: `header`, `n g obj`; `body`, its value, or a stream's dictionary to where its data starts; and `stream`, the rest. A stream's data may be as long as its `/Length` says, when `endstream` follows it (`declared`), or run up to the first `endstream` (`scanned`), and readers choose differently, so the caller resolves the `/Length`, which may be a reference, and passes `stream` the one it chose.
- **A window of a file.** A reader that reads a file by position gives the parser the bytes it has read from an offset on, where they start in the file, and whether they end it (`Parser::window`): spans, offsets and errors are then the file's. Where the window ends before the file does, a call whose read reaches its end, where more bytes could change what it reads, is `Error::Short`, and the reader reads again with more. Read so, the parser reads as it would with the whole file.
- **A writer.** `serialize::write` writes a value as PDF syntax, and `serialize::indirect` as an indirect object, `n g obj` to `endobj`, which the parser reads back as the same value, under limits that take in its count of values (a value in memory may have more than the default's 2^25), and a stream as an indirect object: a real in its shortest form that reads back as the same f64, past i64's range too, never with an exponent, its magnitude clamped as Annex C has it; names and strings byte by byte, a string in its `Form`, or literal when it has none; a dictionary's every entry, in order, a key written twice included, which ISO 32000-2 doesn't allow (`remove_duplicates` leaves each key's last); and a stream's bytes as they are, their length its `/Length`. `indirect_stream` writes a stream whose data is given apart, as it is, such as data a reader decrypted and hands out: plaintext, encoded by the filters the stream's dictionary names. What it can't write as the value it refuses, writing nothing: a real that isn't finite, values nested deeper than the parser reads them under its default limits, a string read as text or still ciphertext, and a stream whose data isn't in memory.
- **Encrypted as it's written.** An encrypted file's indirect objects are written through an encryptor, a `serialize::Encrypt` (`indirect_with`, `indirect_stream_with`): every string in the object, a stream's dictionary's included, and a stream's data pass through it as they're written, given the object's number and generation, so its plaintext can't skip encryption by a caller's omission, and a stream's `/Length` is its encrypted data's. It says what it leaves in the clear: an object whole (an encryption dictionary, a cross-reference stream), or a dictionary's entry (a signature's `/Contents`). `()` encrypts nothing: `indirect` and `indirect_stream` write as through it, as they did, by a walk of their own with nothing to encrypt, so a file that isn't encrypted pays for no encryptor. A direct value is never encrypted, as an object stream's members are encrypted with their container, whole, and a trailer isn't (§7.6.2). The encryptor refusing, nothing is written (`Refused`). pdf-objects holds no cipher: pdf-structure's writer implements the trait over pdf-crypt, by the list its reader decrypts by. A reader's data as read (`Data::At`) is, in an encrypted file, ciphertext, right to copy only whole, under the key, and the number and generation, it was encrypted with.
- **References resolved.** `Resolve` is what gives the indirect object a reference names: `resolve(id)` gives it, or none when there's none, which a reference reads as, null (§7.3.10). Any closure from an `ObjectId` to an `Option<&Object>` is one, a `dyn Fn` and a reference or a box of one among them, and a reader or a document in memory implements it for a reference to itself, so what follows references reads through any of them.
- **References walked.** `Object::references` gives the ids of the objects a value's references name, nested ones too, in the order written: itself, when it's a reference, an array's elements, a dictionary's every entry, a key written twice included, and a stream's dictionary, never its data. It walks with a stack of its own, so a value nested however deeply, as one built in memory may be, walks without overflowing the thread's.

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
let Value::Stream(s) = stream.value() else { panic!() };
assert_eq!(&file[data.start..data.end], b"hello");
assert_eq!(s.data.at(), Some(data));
assert_eq!(said.0, ["the name /N#00 has #00, NUL, which a name can't hold, which readers read differently"]);
```

Limits are set on a copy of `Limits::DEFAULT`, as more of them may come:

```rust
use pdf_objects::parse::{Error, Parser};
use pdf_objects::Limits;

let mut limits = Limits::DEFAULT;
limits.nesting = 2;
let mut parser = Parser::at(b"[[[1]]]", 0).limits(limits);
assert_eq!(parser.value(&mut ()), Err(Error::Nested { limit: 2, at: 3 }));
// A call that fails leaves the parser where it was.
assert_eq!(parser.offset(), 0);
```

And an edit, which drops the span of what it changed and of what holds it, and leaves every other value's; then the value, written:

```rust
use pdf_objects::parse::Parser;
use pdf_objects::{Object, serialize};

let file = b"<< /Type /Annot /Rect [0 0 10 10] /Contents (old) >>";
let mut annot = Parser::at(file, 0).value(&mut ()).unwrap();
let dict = annot.as_dict_mut().unwrap();
dict.set("Contents", Object::string("new"));
let rect = dict.get_mut(b"Rect").and_then(Object::as_array_mut).unwrap();
rect[2] = Object::from(20);

// The dictionary and its /Rect were edited, so they have no span now.
assert_eq!(annot.at, None);
let dict = annot.as_dict().unwrap();
assert_eq!(dict.get(b"Rect").unwrap().at, None);
// What no edit reached is still where it was written.
let rect = dict.get(b"Rect").and_then(Object::as_array).unwrap();
let zero = rect[0].at.unwrap();
assert_eq!(&file[zero.start..zero.end], b"0");
assert!(dict.get(b"Type").unwrap().at.is_some());

// Written, the edit with it.
let mut out = Vec::new();
serialize::write(&mut out, &annot).unwrap();
assert_eq!(out, b"<</Type/Annot/Rect[0 0 20 10]/Contents(new)>>");
```

## What isn't here

- **A file's structure:** the cross-reference sections, the trailer and object streams, read or written; decrypting and decoding, encrypting and encoding; and which of a stream's extents is its data. Those are a reader's, which places what it reads with `Span`s and reads each object with the parser, and a writer's, which writes each object with this one.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.
