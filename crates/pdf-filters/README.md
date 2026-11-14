# pdf-filters

Standalone decoders and Flate encoder for PDF's general-purpose stream filters (ISO 32000-2 §7.4), with zero dependencies on any PDF object model.

`pdf-filters` decodes `FlateDecode`, `LZWDecode`, `ASCIIHexDecode`, `ASCII85Decode`, and `RunLengthDecode` (including TIFF and PNG row predictors), and encodes `FlateDecode` streams (with optional PNG predictors). When damaged data is encountered, decoders recover as much output as possible while reporting every spec fault alongside the partial data.

## Key Capabilities

- **Fault-tolerant decoding (`decode`, `decode_into`):** Returns `Ok(bytes)` on clean streams, or `Err(Error { data, faults })` containing both the partially decoded output and every `Fault` with its filter index.
- **Strict output caps:** Every decode call takes a maximum output byte limit so small compressed inputs ("zip bombs") cannot exhaust memory.
- **Prefix decoding for inline images (`decode_prefix_into`):** Decodes self-delimiting compressed data inside content streams (where no `/Length` exists) and returns `Prefix { read, ended }`, matching zlib's and PDFium's exact consumed-byte counts.
- **Explicit Flate backend selection (`Backend`):** Supports safe-Rust [miniz_oxide](https://crates.io/crates/miniz_oxide) (`miniz_oxide` feature, enabled by default) and high-throughput [zlib-rs](https://crates.io/crates/zlib-rs) (`zlib-rs` feature). Callers pass `Backend` explicitly so Cargo feature unification in downstream crates never silently alters behavior.
- **Configurable fault policy (`Reading`):** Defaults to `Reading::Qpdf` (follows ISO 32000-2 and `qpdf` behavior, while continuing past full 4,096-entry LZW tables like PDFium/pdf.js/poppler). Pass `Reading::Pdfium` to `decode_as` / `decode_prefix_into_as` to match PDFium's LZW and predictor edge cases.
- **Flate encoding (`encode`, `Encoder`):** Compresses data with configurable compression `Level` (0–9) and optional PNG row `Prediction` (e.g. `Row::Up` for cross-reference streams).

## Quickstart: Decoding & Fault Recovery

```rust
use pdf_filters::{Backend, Fault, Filter, Predictor, decode};

// zlib.compress(b"hello hello hello hello", 9)
let data = [
    0x78, 0xda, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57,
    0xc8, 0x40, 0x27, 0x01, 0x68, 0x03, 0x08, 0xb1,
];
let flate = Filter::Flate { predictor: Predictor::None };
let decoded = decode(&data, &[flate], 1 << 20, Backend::MinizOxide);
assert_eq!(decoded.unwrap(), b"hello hello hello hello");

// Corrupting the Adler-32 checksum still yields the decoded bytes alongside Fault::Checksum.
let mut damaged = data;
damaged[15] ^= 1;
let error = decode(&damaged, &[flate], 1 << 20, Backend::MinizOxide).unwrap_err();
assert_eq!(error.data, b"hello hello hello hello");
assert_eq!(error.faults, [(0, Fault::Checksum)]);
```

Filters chain in `/Filter` pipeline order:

```rust
use pdf_filters::{Backend, Filter, decode};

let error = decode(b"48 65 6C 6C 6F", &[Filter::AsciiHex], 1 << 20, Backend::MinizOxide);
assert_eq!(error.unwrap_err().data, b"Hello"); // missing trailing `>` reported as Fault::NoEnd
```

## Inline Image Prefix Decoding

```rust
use pdf_filters::{Backend, Filter, Predictor, decode_prefix_into};

let data = [
    0x78, 0xda, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57,
    0xc8, 0x40, 0x27, 0x01, 0x68, 0x03, 0x08, 0xb1,
    b' ', b'E', b'I', b' ', b'Q',
];
let flate = Filter::Flate { predictor: Predictor::None };
let mut out = Vec::new();
let (prefix, decoded) = decode_prefix_into(&data, flate, 1 << 20, Backend::MinizOxide, &mut out);
assert_eq!((out, decoded), (b"hello hello hello hello".to_vec(), Ok(())));
assert_eq!((prefix.read, prefix.ended), (16, true)); // trailing ` EI Q` left unread
```

## Flate Encoding with PNG Predictors

```rust
use pdf_filters::{Backend, Encoder, Layout, Level, Prediction, Row, decode, encode};

// Four-byte cross-reference stream rows (/W [1 2 1]) compressed with PNG Up prediction.
let rows = [1, 0, 0x10, 0, 1, 0, 0x35, 0, 2, 0, 0x55, 0];
let layout = Layout { colors: 1, bits_per_component: 8, columns: 4 };
let prediction = Prediction::Png(layout, Row::Up);
assert_eq!(prediction.predictor(), 12); // /DecodeParms << /Predictor 12 /Columns 4 >>
let encoder = Encoder::Flate { level: Level::DEFAULT, predictor: prediction };
let mut out = Vec::new();
encode(&rows, &[encoder], Backend::MinizOxide, &mut out).unwrap();
let decoded = decode(&out, &[encoder.filter()], 1 << 20, Backend::MinizOxide);
assert_eq!(decoded.unwrap(), rows);
```

## Scope & Non-Goals

- **Image-specific codecs (`DCTDecode`, `JPXDecode`, `JBIG2Decode`, `CCITTFaxDecode`):** Not included here.
- **Stream decryption (`/Crypt`):** Handled prior to filter decoding by [pdf-crypt](https://crates.io/crates/pdf-crypt) / [pdf-structure](https://crates.io/crates/pdf-structure).
- **Encoding non-Flate filters:** Only `FlateDecode` (with optional PNG prediction) is encoded.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
