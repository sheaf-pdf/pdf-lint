# pdf-images

Zero-I/O PDF Image XObject and inline image (`BI..ID..EI`) decoder, mask compositor, native-space pixel scrubber, and clean stream re-encoder (ISO 32000-2, §7.4.6–§7.4.9, §8.9, §11.6.5), supporting both strict ISO 32000-2 (`Rules::Iso`) and PDFium (`Rules::Pdfium`, as of chromium/8152) policies.

`pdf-images` operates over any [pdf-objects](https://crates.io/crates/pdf-objects) `Resolve` + `Streams` provider without performing file I/O. It re-exports [pdf-objects](https://crates.io/crates/pdf-objects), [pdf-color](https://crates.io/crates/pdf-color), [pdf-filters](https://crates.io/crates/pdf-filters), and [pdf-content-streams](https://crates.io/crates/pdf-content-streams) at the crate root.

## Core Concepts

- **Two-tier decoding (`RawImage` and `RgbaImage`):**
  - **Tier 1 (`RawImage`, `SampleData`, `decode_xobject_raw`, `decode_inline_raw`, `decode_stream_raw`):** Extracts image samples in their native color space and bit depth (`1, 2, 4, 8, 16` bpc)—keeping bi-level `CCITTFaxDecode`, `JBIG2Decode`, and `/ImageMask` streams bit-packed—before `/Decode` mapping or color conversion.
  - **Tier 2 (`RgbaImage`, `decode_xobject`, `decode_inline`, `decode_stream`, `RawImage::to_rgba8`):** Applies `/Decode` intervals, evaluates colors through `pdf-color`'s `PreparedSpace` (`Rules::Iso` or `Rules::Pdfium`), colors `/ImageMask` stencils, and composites explicit stencil `/Mask`s, pre-`/Decode` integer color-key `/Mask` arrays, `/SMask` soft-mask streams with `/Matte` un-premultiplication, and `JPXDecode` `/SMaskInData` opacity channels into straight-alpha `RGBA8`.
- **All PDF image codecs (`ImageCodec`):**
  - Uncompressed and standard-filtered (`FlateDecode`, `LZWDecode`, `ASCII85Decode`, `ASCIIHexDecode`, `RunLengthDecode`) packed bitstreams (`1, 2, 4, 8, 16` bpc).
  - `DCTDecode` (`zune-jpeg`): 8-bit grayscale, RGB, YCbCr, CMYK, and Adobe `APP14` YCCK with `/ColorTransform` support.
  - `CCITTFaxDecode` (`hayro-ccitt`): Group 4 MMR (`K < 0`), Group 3 1D (`K == 0`), Group 3 2D (`K > 0`), `/BlackIs1`, `/EncodedByteAlign`, and truncated-stream partial-row recovery.
  - `JBIG2Decode` (`hayro-jbig2`): Embedded and sequential/random-access segments with indirect `/JBIG2Globals` resolution via `Streams::decoded`.
  - `JPXDecode` (`hayro-jpeg2000`): Codestream and JP2 container decoding across `1..=16` bit depths (rescaling intermediate non-PDF depths to PDF `1, 2, 4, 8, 16` bpc), embedded JP2 color specifications (including `colr` ICC profiles with `icc`), and `/SMaskInData` (`0, 1, 2`).
- **Redaction pixel scrubbing & clean re-encoding (`RawImage::fill_rect_raw`, `RawImage::fill_rect_decoded`, `encode_raw_flate`, `encode_rgba_flate`, `encode_jpeg`):**
  - Burn rectangular redaction regions directly in an image's native sample space and bit depth (`RawImage::fill_rect_raw` for pre-`/Decode` integer samples or `RawImage::fill_rect_decoded` to invert `/Decode` automatically; both reject short slices and color-key-transparent fills via `ScrubError`) without touching surrounding pixels, then re-encode cleanly via lossless `FlateDecode` + PNG `Sub` prediction (preserving `/Mask`, `/SMask`, and `/OC`) or pure-Rust baseline `DCTDecode` ([jpeg-encoder](https://crates.io/crates/jpeg-encoder))—stripping EXIF, XMP, IPTC, comment metadata, and JBIG2 symbol dictionaries (writing only a minimal JFIF or Adobe `APP14` header on JPEG output). Note that separate indirect `/SMask` and explicit stencil `/Mask` streams are preserved by reference and not modified by `fill_rect_raw` / `fill_rect_decoded` alone (re-attach scrubbed companion streams via `EncodedImage::attach_mask` / `EncodedImage::attach_smask`).
- **A JPEG file passed through unchanged (`wrap_jpeg`):** A baseline or progressive JPEG's bytes become a `/DCTDecode` Image XObject as they are, nothing decoded and nothing encoded again, with `/Width`, `/Height`, `/ColorSpace` and `/BitsPerComponent` read from the frame header (`/Decode [1 0 1 0 1 0 1 0]` for Adobe's inverted CMYK). A JPEG a PDF reader won't decode (lossless, hierarchical, arithmetic-coded, 12-bit, two components) is refused with a typed `JpegRefused`.
- **Bounded resources, codec isolation & fault sinks (`Limits`, `Fault`):**
  - Dimensions, pixel counts, and byte budgets are checked before allocating buffers, recoverable departures (`Fault`) are reported to any `pdf-objects` `fault::Sink<Fault>` at zero cost when `()` is passed, and internal panics in third-party decoders are converted to `Error::Codec` via `std::panic::catch_unwind` under default `panic = "unwind"` builds (`panic = "abort"` binaries bypass unwind guards).

## Quickstart

Encode a 4×4 RGB JPEG Image XObject, scrub a 2×2 redaction block in native sample space, re-encode cleanly with `FlateDecode`, and evaluate to `RGBA8`:

```rust
use std::collections::BTreeMap;

use pdf_images::pdf_filters::Level;
use pdf_images::pdf_objects::{Object, ObjectId, Unreadable, Why, WithStreams};
use pdf_images::{
    DecodeOptions, JpegColorSpace, Limits, decode_stream, decode_stream_raw, encode_jpeg,
    encode_raw_flate,
};

// Build a clean 4x4 RGB JPEG Image XObject stream (solid red `[220, 40, 30]`).
let mut pixels = Vec::with_capacity(4 * 4 * 3);
for _ in 0..16 {
    pixels.extend_from_slice(&[220, 40, 30]);
}
let jpeg_xobj = encode_jpeg(&pixels, 4, 4, JpegColorSpace::Rgb, 95).unwrap();

// Zero-I/O resolver with no indirect subsidiary streams.
let objects = BTreeMap::<ObjectId, Object>::new();
let provider = WithStreams::new(&objects, |id, _| {
    Err(Unreadable::new(id, Why::Damaged, "no indirect streams"))
});

// Tier 1: decode native 8-bit RGB samples, scrub top-left 2x2 to black, and re-encode as Flate.
let mut raw = decode_stream_raw(
    provider,
    &jpeg_xobj,
    None,
    &DecodeOptions::PDFIUM,
    &Limits::DEFAULT,
)
.unwrap();
raw.fill_rect_raw(0, 0, 2, 2, &[0, 0, 0]).unwrap();
let clean_flate = encode_raw_flate(&raw, None, Level::DEFAULT).unwrap();

// Tier 2: evaluate the scrubbed Flate Image XObject to straight-alpha RGBA8.
let rgba = decode_stream(
    provider,
    &clean_flate.stream,
    None,
    &DecodeOptions::PDFIUM,
    &Limits::DEFAULT,
)
.unwrap();
assert_eq!(rgba.pixel(0, 0), Some([0, 0, 0, 255]));
assert!(rgba.pixel(3, 3).unwrap()[0] > 200);
```

## Cargo Features

- **`icc`** *(optional)*: Forwards `pdf-color/icc` (`moxcms`) to evaluate `ICCBased` color spaces, `/OutputIntents`, and embedded JP2 `colr` ICC profiles rather than their `/Alternate` spaces.
- **`simd`** *(optional)*: Enables runtime SIMD dispatch in `zune-jpeg`, `hayro-jbig2`, and `hayro-jpeg2000` (`pdf-images` itself remains `#![forbid(unsafe_code)]`).
- **`zlib-rs`** *(optional)*: Enables `pdf-filters/zlib-rs` alongside `miniz_oxide`.
- **`tracing`** *(optional)*: Emits numeric [tracing](https://crates.io/crates/tracing) spans with dimensions and byte counts (never pixel bytes or file plaintext).

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.

Baseline JPEG re-encoding (`encode_jpeg`, `encode_jpeg_bytes`) uses [jpeg-encoder](https://crates.io/crates/jpeg-encoder), licensed under `(MIT OR Apache-2.0) AND IJG`. Under the Independent JPEG Group license terms, distributions of executable code must state in accompanying documentation that *"this software is based in part on the work of the Independent JPEG Group."*
