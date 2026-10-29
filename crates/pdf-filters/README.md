# pdf-filters

PDF's standard filters for data of any kind, decoded, in one small Rust crate: `ASCIIHexDecode`, `ASCII85Decode`, `LZWDecode` and `FlateDecode` with their TIFF and PNG predictors, and `RunLengthDecode` (ISO 32000-2, §7.4). And `FlateDecode` encoded, with PNG's predictors, for a writer.

It's made for a checker as much as a reader. Data that breaks a rule still decodes as far as a reader can take it, and every fault is said, so the data can be read and what was wrong with it reported too.

## What it promises

- **Every fault said, what decoded kept.** `decode` gives the data, or an `Error` with both what decoded and each `Fault`, with the filter it's in: a byte a filter doesn't take, a missing end-of-data marker, an LZW code not in the table, corrupt or truncated deflate data, a checksum that doesn't match, a PNG row tag that isn't one, a short last row. Each fault's documentation says whether the filter stopped there, keeping what it decoded before, or read on.
- **A limit on what comes out.** No filter's output is over the limit the caller gives (an LZW or Flate filter's before its predictor is allowed a PNG predictor's tag a row besides), so a small stream can't decode into more memory than the caller allows: a filter holds its input and its output, whose buffer grows by doubling. Data of just the limit's length decodes whole.
- **Parameters, not dictionaries.** A `Filter` says what its `/DecodeParms` do. Reading them from a stream's dictionary is the caller's, so it works with any object model.
- **The same result every time,** with nothing but the input deciding it: no I/O, no global state.
- **Either Flate backend, named at each call.** `FlateDecode` inflates and deflates through [miniz_oxide](https://crates.io/crates/miniz_oxide) (the `miniz_oxide` feature, on by default) or [zlib-rs](https://crates.io/crates/zlib-rs) (the `zlib-rs` feature), each compiled in by its feature, both if both are on. Each call that decodes or encodes names its `Backend`, which exists only with its feature, so turning a feature on, in this crate or any that depends on it, changes nothing a call gives. There's no precedence between them. Without either, the crate doesn't build.
- **Into the caller's buffer.** `decode_into` appends what it decodes to a `Vec` the caller gives, keeping what it held (the limit counts only what's added), and `encode` appends what it encodes, so streams join in one buffer and one buffer serves many. `decode` gives a `Vec` of its own.

## An example

```rust
use pdf_filters::{Backend, Fault, Filter, Predictor, decode};

// zlib data: Python's zlib.compress(b"hello hello hello hello", 9).
let data = [
    0x78, 0xda, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57,
    0xc8, 0x40, 0x27, 0x01, 0x68, 0x03, 0x08, 0xb1,
];
let flate = Filter::Flate { predictor: Predictor::None };
let decoded = decode(&data, &[flate], 1 << 20, Backend::MinizOxide);
assert_eq!(decoded.unwrap(), b"hello hello hello hello");

// The checksum damaged: everything decodes, and the fault is said.
let mut damaged = data;
damaged[15] ^= 1;
let error = decode(&damaged, &[flate], 1 << 20, Backend::MinizOxide).unwrap_err();
assert_eq!(error.data, b"hello hello hello hello");
assert_eq!(error.faults, [(0, Fault::Checksum)]);
```

Filters chain as a stream's `/Filter` array lists them, each given what the one before it gave:

```rust
use pdf_filters::{Backend, Filter, decode};

let error = decode(b"48 65 6C 6C 6F", &[Filter::AsciiHex], 1 << 20, Backend::MinizOxide);
assert_eq!(error.unwrap_err().data, b"Hello"); // read without its `>`
```

## Encoding

An `Encoder` says what a writer chooses as well as the filter: for Flate, a `Level` (0, stored, to 9, the smallest; 6 by default, as zlib's) and a `Prediction`, none or PNG's with a `Row` filter for every row (Up, say, for a cross-reference stream's rows). `Encoder::filter` is the `Filter` that decodes what it encodes, which a writer's `/Filter` and `/DecodeParms` say, and `Prediction::predictor` the `/Predictor` it writes (10 plus the row filter's tag, `Row::tag`), which the `Filter` doesn't hold, as PNG's 10 to 15 decode alike. Encoders are listed as a stream's `/Filter` lists them, the last applied first. Each backend deflates as [flate2](https://crates.io/crates/flate2)'s `ZlibEncoder` does on it (zlib-rs is called as flate2 calls it, as what it writes depends on how much room each call has), and with flate2 1.1 on the same versions of the backends the bytes were found the same at levels 6 and 9: measured, not promised, as a release of either can change them. The two backends' bytes differ.

```rust
use pdf_filters::{Backend, Encoder, Layout, Level, Prediction, Row, decode, encode};

// A cross-reference stream's rows, /W [1 2 1]: four bytes a row.
let rows = [1, 0, 0x10, 0, 1, 0, 0x35, 0, 2, 0, 0x55, 0];
let layout = Layout { colors: 1, bits_per_component: 8, columns: 4 };
let prediction = Prediction::Png(layout, Row::Up);
assert_eq!(prediction.predictor(), 12); // its /DecodeParms: << /Predictor 12 /Columns 4 >>
let encoder = Encoder::Flate { level: Level::DEFAULT, predictor: prediction };
let mut out = Vec::new();
encode(&rows, &[encoder], Backend::MinizOxide, &mut out).unwrap();
let decoded = decode(&out, &[encoder.filter()], 1 << 20, Backend::MinizOxide);
assert_eq!(decoded.unwrap(), rows);
```

Only Flate encodes, with no predictor or PNG's: what PDF writers write for data of any kind. `Encoder`, `Prediction` and `Row` are non-exhaustive, so others can join them.

## How it reads what the spec leaves open

Where ISO 32000-2 doesn't say how to read a fault, it's read as qpdf reads it: a PNG row whose tag isn't 0 to 4 as predicted by nothing; a short last row with zeros for what it lacks (but a row a fault cut off isn't read); zlib data whose checksum doesn't match, or that ends early, as far as it decompressed. Where a fault stops a filter, what decoded before it is kept, which qpdf, giving the data undecoded instead, doesn't keep.

Where qpdf departs from the spec, it follows the spec: `ASCIIHexDecode` and `ASCII85Decode` ignore PDF's white-space (NUL among it, vertical tab not); a run-length of 128 ends the data; TIFF's predictor takes 1, 2, 4, 8 or 16 bits a sample.

## What isn't here

- **The filters for images:** `DCTDecode`, `JPXDecode`, `JBIG2Decode`, `CCITTFaxDecode`. A reader that doesn't render images, a checker's, doesn't need them.
- **`Crypt`:** decrypting is a step before the filters, and the caller's.
- **Encoders but Flate's,** and TIFF's predictor's: Flate, with PNG's predictors or none, is what a writer needs for data of any kind.

## The backends

miniz_oxide is safe Rust throughout; zlib-rs is faster to inflate, and to deflate at the default level (at 9, miniz_oxide is), and its `unsafe` code is behind the safe API this crate calls (on Rust's allocator, without its C allocator). On data that decodes cleanly, the two decode alike. On damaged data, where each stops, how much it gave before, and the place a `Fault::Corrupt` says can differ by backend, as each reads ahead differently. A checker whose reports must not change with a dependency's features names one backend at every call.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.
