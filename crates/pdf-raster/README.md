# pdf-raster

Zero-I/O pure-Rust PDF 2D page, Form XObject, annotation, and interactive form rasterizer (ISO 32000-2, §8–§9, §11, §12.5, §12.7, §14.6), built on [pdf-objects](https://crates.io/crates/pdf-objects), [pdf-color](https://crates.io/crates/pdf-color), [pdf-images](https://crates.io/crates/pdf-images), [pdf-fonts](https://crates.io/crates/pdf-fonts), [pdf-gfx](https://crates.io/crates/pdf-gfx), [pdf-annots](https://crates.io/crates/pdf-annots), and [pdf-forms](https://crates.io/crates/pdf-forms).

`pdf-raster` operates over any `Resolve` + `Streams` provider without performing file I/O, supporting both strict ISO 32000-2 (`Rules::Iso`) and viewer (`Rules::Pdfium`, as of chromium/8152) policies.

## Core Concepts

- **Viewer-aligned `Viewport`s & `Pixmap` formats:**
  - `Viewport::Page` (whole-page width fit), `Viewport::Tile` (zoomed sub-rectangle tile), `Viewport::Dpi` (physical DPI honoring page `/UserUnit`), and `Viewport::Custom` (explicit affine `PageSpace -> DeviceSpace` matrix) resolve closed-form transforms over `PageGeom` and check `Limits` before allocating pixel buffers.
  - Outputs straight `RGBA8` (`PixelFormat::Rgba8`), native `BGRA8` (`PixelFormat::Bgra8`), or premultiplied `RGBA8` (`PixelFormat::Rgba8Premul`), with redacted `Debug` formatting that never prints pixel contents.
- **Complete 2D PDF imaging pipeline (`render_page`, `render_page_with`, `render_content`, `render_content_with`):**
  - **Paths & clips:** `NonZero` and `EvenOdd` fills, anisotropic user-space strokes, caps, joins, miter limits, dash patterns, `0 w` device hairlines, `W`/`W*` path clips, `4..=7 Tr` glyph-outline text clips, and Form XObject `/BBox` clips.
  - **Text & fonts:** Embedded TrueType, OpenType/CFF, CFF, and Type 1 glyph outlines (`programs` feature), Type 3 `/CharProcs` (`d0` colored and `d1` uncolored stencils), caller-supplied `FontFallback` faces cached in `RenderCache`, and built-in vector fallback contours.
  - **Images & masks:** Indirect Image XObjects and `BI..ID..EI` inline images via `pdf-images` (`DCTDecode`, `CCITTFaxDecode`, `JBIG2Decode`, `JPXDecode`, `/ImageMask` stencils, `/Mask`, and `/SMask` + `/Matte`).
  - **Transparency, patterns & shadings:** Isolated Form XObject `/Group`s, all 16 PDF blend modes (`/BM`), constant opacities (`/ca`, `/CA`), `/Alpha` and `/Luminosity` soft masks (`/SMask` with `/BC` and `/TR`), colored/uncolored tiling patterns (`PatternType 1`), and shadings (`sh` and `PatternType 2`, `ShadingType 1..=5`).
  - **Optional content, annotations & AcroForms:** Evaluates catalog `/OCProperties` (`/OCG`, `/OCMD`, and `/Usage` `/View` vs. `/Print`), draws written `/AP /N` streams, and optionally synthesizes missing annotation and AcroForm widget appearances (`Appearances::Synthesize` vs. `Appearances::WrittenOnly`) without mutating the document.
- **Swappable `RenderBackend` & `RenderedDoc` traits:** Layer 2 traits (`BackendError`, `RenderBackend`, `RenderedDoc`) allow applications and differential test harnesses to switch cleanly between `pdfs::RustBackend` and external/FFI engines.
- **Resource budgets & unwind note:** Coordinates, dash expansions, layer nesting, and offscreen buffers are clamped and budgeted upfront before invoking the software 2D rasterizer. As defense-in-depth against bugs in third-party rasterizer or image-codec dependencies, internal rasterizer calls are additionally guarded with `std::panic::catch_unwind` (converting any unexpected panic into `Error::RasterizerPanic`); under `panic = "abort"` (including libFuzzer harnesses), an upstream dependency panic will abort instead of unwinding.

## Quickstart

Build a one-page PDF in memory with `pdf_gfx::Canvas` and rasterize it with zero I/O:

```rust
use std::collections::BTreeMap;

use pdf_raster::pdf_content_streams::{Color, Reserved};
use pdf_raster::pdf_gfx::{Canvas, Rect, Rotation};
use pdf_raster::pdf_objects::pdf_tokenizer::Rules;
use pdf_raster::pdf_objects::{Object, ObjectId, Stream, Unreadable, Why, WithStreams, dict};
use pdf_raster::pdf_semantics::Limits as SemLimits;
use pdf_raster::pdf_semantics::pages::pages;
use pdf_raster::{RenderCache, RenderOpts, Viewport, render_page};

// Draw a red banner onto a 200 × 100 pt page.
let mut canvas = Canvas::new();
canvas.fill_color(Color::rgb8([220, 40, 30]));
canvas.fill_rect(Rect::from_xywh(20.0, 20.0, 160.0, 60.0));
let (content_bytes, _) = canvas.finish(&Reserved::default());

let mut objects = BTreeMap::new();
let content_id = ObjectId::new(4, 0);
objects.insert(content_id, Object::from(Stream::new(dict! {}, content_bytes)));
objects.insert(
    ObjectId::new(3, 0),
    Object::from(dict! {
        "Type" => Object::name("Page"),
        "Parent" => ObjectId::new(2, 0),
        "Contents" => content_id,
    }),
);
objects.insert(
    ObjectId::new(2, 0),
    Object::from(dict! {
        "Type" => Object::name("Pages"),
        "Kids" => vec![Object::reference(3)],
        "Count" => 1,
        "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(200), Object::from(100)],
    }),
);
let catalog = dict! { "Type" => Object::name("Catalog"), "Pages" => ObjectId::new(2, 0) };

let provider = WithStreams::new(&objects, |id, limit| {
    let data = objects
        .get(&id)
        .and_then(Object::as_stream)
        .and_then(|s| s.data.bytes())
        .ok_or_else(|| Unreadable::new(id, Why::Damaged, "missing stream"))?;
    if data.len() > limit {
        return Err(Unreadable::new(id, Why::Limit, "stream exceeds limit"));
    }
    Ok(data.to_vec())
});

let table = pages(provider, &catalog, Rules::Pdfium, &SemLimits::DEFAULT, &mut ()).unwrap();
let opts = RenderOpts::with_viewport(Viewport::Page {
    width: 200,
    rotation: Rotation::R0,
});
let (pixmap, _) = render_page(
    provider,
    Some(&catalog),
    table.get(0).unwrap(),
    &RenderCache::default(),
    None,
    &opts,
)
.unwrap();

assert_eq!((pixmap.width(), pixmap.height()), (200, 100));
assert_eq!(pixmap.pixel_rgba8(100, 50), Some([220, 40, 30, 255]));
assert_eq!(pixmap.pixel_rgba8(5, 5), Some([255, 255, 255, 255]));
```

## Cargo Features

- **`programs`** *(default)*: Enables embedded TrueType, OpenType/CFF, CFF, and Type 1 glyph outline evaluation via `pdf-fonts/programs`.
- **`text-layout`** *(default)*: Enables on-the-fly synthesis of FreeText annotation and AcroForm text, button, and choice widget appearances.
- **`icc`** *(default)*: Enables pure-Rust `ICCBased` and `/OutputIntents` profile conversion via `pdf-color/icc` (`moxcms`).
- **`pdfium`** *(optional)*: Enables `pdf_raster::pdfium` and `PdfiumBackend`, a thin `#![forbid(unsafe_code)]` dynamic-library `RenderBackend` adapter over `pdfium-render` (`0.9.6`, `thread_safe`; semver-tied to `pdfium-render` `0.9`) so callers can swap between `pdfs::Pdf` / `pdfs::RustBackend` and PDFium at runtime.
- **`predefined-cmaps`** *(optional)*: Bundles Adobe CJK CMaps via `pdf-fonts/predefined-cmaps`.
- **`simd`** *(optional)*: Enables runtime CPU SIMD dispatch in the 2D compositor and image codecs.
- **`zlib-rs`** *(optional)*: Enables `zlib-rs` in `pdf-images` inline filter chains.
- **`tracing`** *(optional)*: Emits numeric `tracing` spans (dimensions and counts only; never pixel buffers or file plaintext).

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
