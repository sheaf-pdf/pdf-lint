# pdf-filters

PDF's standard filters for data of any kind, decoded, in one small Rust crate: `ASCIIHexDecode`, `ASCII85Decode`, `LZWDecode` and `FlateDecode` with their TIFF and PNG predictors, and `RunLengthDecode` (ISO 32000-2, §7.4).

It's made for a checker as much as a reader. Data that breaks a rule still decodes as far as a reader can take it, and every fault is said, so the data can be read and what was wrong with it reported too.

## What it promises

- **Every fault said, what decoded kept.** `decode` gives the data, or an `Error` with both what decoded and each `Fault`, with the filter it's in: a byte a filter doesn't take, a missing end-of-data marker, an LZW code not in the table, corrupt or truncated deflate data, a checksum that doesn't match, a PNG row tag that isn't one, a short last row. Each fault's documentation says whether the filter stopped there, keeping what it decoded before, or read on.
- **A limit on what comes out.** No filter's output is over the limit the caller gives (an LZW or Flate filter's before its predictor is allowed a PNG predictor's tag a row besides), so a small stream can't decode into more memory than the caller allows: a filter holds its input and its output, whose buffer grows by doubling. Data of just the limit's length decodes whole.
- **Parameters, not dictionaries.** A `Filter` says what its `/DecodeParms` do. Reading them from a stream's dictionary is the caller's, so it works with any object model.
- **The same result every time,** with nothing but the input deciding it: no I/O, no global state.

## An example

```rust
use pdf_filters::{Fault, Filter, Predictor, decode};

// zlib data: Python's zlib.compress(b"hello hello hello hello", 9).
let data = [
    0x78, 0xda, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57,
    0xc8, 0x40, 0x27, 0x01, 0x68, 0x03, 0x08, 0xb1,
];
let flate = Filter::Flate { predictor: Predictor::None };
assert_eq!(decode(&data, &[flate], 1 << 20).unwrap(), b"hello hello hello hello");

// The checksum damaged: everything decodes, and the fault is said.
let mut damaged = data;
damaged[15] ^= 1;
let error = decode(&damaged, &[flate], 1 << 20).unwrap_err();
assert_eq!(error.data, b"hello hello hello hello");
assert_eq!(error.faults, [(0, Fault::Checksum)]);
```

Filters chain as a stream's `/Filter` array lists them, each given what the one before it gave:

```rust
use pdf_filters::{Filter, decode};

let error = decode(b"48 65 6C 6C 6F", &[Filter::AsciiHex], 1 << 20).unwrap_err();
assert_eq!(error.data, b"Hello"); // read without its `>`
```

## How it reads what the spec leaves open

Where ISO 32000-2 doesn't say how to read a fault, it's read as qpdf reads it: a PNG row whose tag isn't 0 to 4 as predicted by nothing; a short last row with zeros for what it lacks (but a row a fault cut off isn't read); zlib data whose checksum doesn't match, or that ends early, as far as it decompressed. Where a fault stops a filter, what decoded before it is kept, which qpdf, giving the data undecoded instead, doesn't keep.

Where qpdf departs from the spec, it follows the spec: `ASCIIHexDecode` and `ASCII85Decode` ignore PDF's white-space (NUL among it, vertical tab not); a run-length of 128 ends the data; TIFF's predictor takes 1, 2, 4, 8 or 16 bits a sample.

## What isn't here

- **The filters for images:** `DCTDecode`, `JPXDecode`, `JBIG2Decode`, `CCITTFaxDecode`. A reader that doesn't render images, a checker's, doesn't need them.
- **`Crypt`:** decrypting is a step before the filters, and the caller's.
- **Encoding.** It decodes only.

`FlateDecode` decompresses through [miniz_oxide](https://crates.io/crates/miniz_oxide), behind one function, so that another decompressor could take its place.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.
