# pdfs

Read, inspect, edit, and write PDF files through a single Rust dependency.

`pdfs` combines a modular family of PDF crates behind an ergonomic, viewer-compatible API: open files lazily from disk or memory, inspect pages and metadata, extract text and fonts, draw new pages in Standard 14 or embedded TrueType fonts, encrypt with AES-256, linearize for fast web viewing, or append incremental updates. Every underlying crate in the family is also re-exported at the root (`pdfs::structure`, `pdfs::objects`, `pdfs::gfx`, `pdfs::annots`, etc.) at matching versions when you need low-level control; the longer `pdfs::pdf_structure`, `pdfs::pdf_objects` and the rest remain and name the same crates.

## Quickstart: Write and Read Page Text

`pdfs::open(path)` opens a file on disk using positional reads (without loading the entire file into memory), and `pdfs::from_bytes` opens an in-memory buffer. Objects are parsed lazily on first access and cached safely across threads:

```rust
use pdfs::prelude::*;

// Create a one-page PDF in memory so this example runs standalone.
let mut doc = Document::new();
let page = doc.add_page(Rect::LETTER);
page.begin_text();
page.font(standard_font(Std14::Helvetica), 24.0);
page.text_move(72.0, 700.0);
page.show(&Std14::Helvetica.codes("Hello, world").unwrap());
page.end_text();
let file = doc.to_bytes().unwrap();

// Open the PDF and extract text from each page.
let pdf = pdfs::from_bytes(file).unwrap();
let pages = pdf.pages().unwrap();
assert_eq!(pages.len(), 1);
for page in &pages {
    assert_eq!(page.geom().media_box, Rect::LETTER);
    let shown = page.text().unwrap();
    assert_eq!(shown.text(), "Hello, world");
    assert!(shown.is_whole());
}
```

`Page::text` extracts strings in content-stream drawing order (including nested Form XObjects), while `Page::text_page` and `Page::search` (`text-layout` feature, enabled by default) reconstruct spatial reading order with synthetic spaces/newlines, `unicode-bidi` RTL reordering, selection quads, and search. `Page::geom` computes normalized page boundary boxes and closed-form `PageSpace <-> ViewSpace` transforms, `Page::interpret` runs the bounded graphics/text state machine to stream placed paths, glyph origins/quads, images, shadings, clips, and marked content, `Page::encode` re-encodes streams into position-independent `Tm` + `Td` operations, and `Page::content` returns the page's decoded, concatenated content streams and parsed operators.

## Drawing Pages with Standard 14 Fonts

`Document::add_page` returns a `ContentWriter` for drawing shapes and text. Shared resources (such as fonts) are automatically deduplicated when saving. `Std14::codes` encodes Unicode strings into a Standard 14 font's single-byte encoding and returns an error if a character cannot be represented:

```rust
use pdfs::prelude::*;

let mut doc = Document::new();
for title in ["Größe", "Café €5"] {
    let page = doc.add_page(Rect::LETTER);
    page.fill_color(Color::rgb8([0, 82, 155]));
    page.rect(0.0, 742.0, 612.0, 50.0);
    page.paint(Paint::Fill);
    page.fill_color(Color::Gray(0.0));
    page.begin_text();
    page.font(standard_font(Std14::TimesRoman), 18.0);
    page.text_move(72.0, 700.0);
    page.show(&Std14::TimesRoman.codes(title).unwrap());
    page.end_text();
}
assert!(Std14::TimesRoman.codes("漢字").is_err());
let file = doc.to_bytes().unwrap();

let pdf = pdfs::from_bytes(file).unwrap();
let pages = pdf.pages().unwrap();
let titles: Vec<String> = (pages.iter())
    .map(|page| page.text().unwrap().text())
    .collect();
assert_eq!(titles, ["Größe", "Café €5"]);
```

For text beyond a Standard 14 font's single-byte encoding (CJK, Hebrew, Arabic, symbols past the Basic Multilingual Plane), `Document::embed_font` (`embed` feature, enabled by default) embeds a TrueType program: `EmbeddedFont::codes` encodes any string its `cmap` maps, `Canvas::text_box` lays text out in it (and `Canvas::text_box_in` in a `WithFallback` of a Standard 14 font and it, each character in the first that has it), and each save writes the program subset to the glyphs drawn, with a `/ToUnicode` map so the text reads back. An annotation's text is drawn in it too: `pdfs::annots::BuiltAnnot::free_text_with_font` and `redact_with_font` (`text-layout` feature) take `font.res()` and `&font`, and `Document::add_annot` writes their appearance streams naming the font. It also puts the font in the form's default resources (`/AcroForm /DR /Font`, a form made where the document has none) under the name the annotation's `/DA` gives (`/Helv` by default), or under the first free one after it (`/Helv1`) where that name is another font's, which `/DA` is then written to name: a viewer that draws the text again from `/DA` draws it in the same font.

`Document::save(path)` writes compressed PDF 1.7 output (Flate-encoded cross-reference and object streams by default; customizable via `Document::save_options`). `Document::claim` and `Document::set_metadata` attach XMP metadata and conformance claims, while `Document::set_info`, `Document::set_outlines`, `Document::set_page_labels`, `Document::add_embedded_file`, and `Document::set_catalog_view` author `/Info` dictionaries, bookmarks, page labels, embedded/associated files, and viewer preferences (read back via `Pdf::metadata`, `Pdf::outlines`, `Pdf::page_labels`, `Pdf::embedded_files`, `Pdf::associated_files`, `Pdf::optional_content`, and `Pdf::catalog_view`).

## Encryption and Linearization

`Document::encrypt` configures AES-256 encryption with user ("open") and owner ("permissions") passwords via `Protection`. `Document::save_linearized` writes a linearized ("Fast Web View") PDF so viewers can render the first page over HTTP before downloading the whole file:

```rust
use pdfs::prelude::*;

let mut doc = Document::new();
for n in 1..=3 {
    let page = doc.add_page(Rect::LETTER);
    page.begin_text();
    page.font(standard_font(Std14::Helvetica), 12.0);
    page.text_move(72.0, 720.0);
    page.show(&Std14::Helvetica.codes(&format!("Page {n}")).unwrap());
    page.end_text();
}
// "reader" can open the document without print permission; "owner" has full access.
let reader = Password("reader").prepared().unwrap();
let owner = Password("owner").prepared().unwrap();
let mut protection = Protection::new(&reader);
protection.owner = Some(&owner);
protection.permissions = Permissions::ALL.without(Permissions::PRINT);
doc.encrypt(&protection).unwrap();
let mut file = Vec::new();
let options = doc.linearize_options();
doc.save_linearized_to(&mut file, &options).unwrap();

// Opening without a password leaves encrypted content locked.
let locked = pdfs::from_bytes(&file[..]).unwrap();
assert_eq!(*locked.encryption(), Encryption::Locked);

// Opening with the user password unlocks content and reports permissions.
let passwords = [Password("reader")];
let mut opening = Options::DEFAULT;
opening.passwords = &passwords;
let pdf = opening.from_bytes(&file[..]).unwrap();
let Encryption::Opened { permissions, .. } = pdf.encryption() else {
    panic!("not opened");
};
assert!(!permissions.contains(Permissions::PRINT));
let third = pdf.pages().unwrap().get(2).unwrap().text().unwrap();
assert_eq!(third.text(), "Page 3");
let verdict = pdf.linearized().unwrap();
assert!(verdict.linearized && verdict.holds && verdict.judged_whole);
```

You can also unlock an already-opened locked `Pdf` later with `Pdf::unlock`. Passwords are only borrowed during key derivation and never stored.

## Editing Existing Files and Incremental Updates

`Pdf::edit` creates an editable `Document` backed by the original file. `Document::save_update` appends only modified or newly added objects as a PDF incremental update, while `Document::save` writes a clean full file (preserving existing encryption unless changed with `Document::decrypt` or `Document::encrypt`):

```rust
use pdfs::prelude::*;

let mut doc = Document::new();
let page = doc.add_page(Rect::LETTER);
page.begin_text();
page.font(standard_font(Std14::Helvetica), 12.0);
page.show(&Std14::Helvetica.codes("First").unwrap());
page.end_text();
let file = doc.to_bytes().unwrap();

// Edit the existing PDF and emit only the incremental update bytes.
let read = pdfs::from_bytes(&file[..]).unwrap();
let mut edited = read.edit().unwrap();
let page = edited.add_page(Rect::new(0.0, 0.0, 400.0, 300.0));
page.begin_text();
page.font(standard_font(Std14::Courier), 12.0);
page.show(&Std14::Courier.codes("Second").unwrap());
page.end_text();
let mut update = Vec::new();
edited.save_update_to(&mut update).unwrap();

// Concatenating the original bytes and the update yields a two-page PDF.
let updated = pdfs::from_bytes([&file[..], &update[..]].concat()).unwrap();
let pages = updated.pages().unwrap();
let texts: Vec<String> = (pages.iter())
    .map(|page| page.text().unwrap().text())
    .collect();
assert_eq!(texts, ["First", "Second"]);
```

`save_update(path)` verifies file identity before appending so it will never corrupt a file modified since it was opened. Full saves refuse to overwrite the same file handle a positional `pdfs::open` is actively reading; to overwrite a file in place, load it into memory first with `pdfs::from_bytes(std::fs::read(path)?)`.

## Rendering Pages and Extracting Images

With the default `raster` and `images` features, `Page::render_pixmap` (or `Page::render` when you also want `RenderSummary`) rasterizes pages or tiles into a `Pixmap` and encodes PNG bytes via `Pixmap::to_png()`, `Page::images` lists drawn Image XObjects and inline images, `Page::decode_image` decodes any Image XObject into `RgbaImage`, and `RenderBackend` lets applications swap at runtime between pure-Rust `RustBackend` and dynamic-library `PdfiumBackend` (`pdfium` feature):

```rust
#[cfg(feature = "raster")]
{
    use std::sync::Arc;
    use pdfs::prelude::*;

    let mut doc = Document::new();
    let p = doc.add_page(Rect::new(0.0, 0.0, 144.0, 72.0));
    p.fill_color(Color::rgb8([24, 112, 200]));
    p.rect(0.0, 0.0, 144.0, 72.0);
    p.paint(Paint::Fill);
    let bytes = doc.to_bytes().unwrap();

    let pdf = pdfs::from_bytes(&bytes[..]).unwrap();
    let pages = pdf.pages().unwrap();
    let page = pages.get(0).unwrap();
    let pixmap = page.render_pixmap(&RenderOpts::dpi(72.0)).unwrap();
    assert_eq!((pixmap.width(), pixmap.height()), (144, 72));
    assert_eq!(pixmap.pixel_rgba8(72, 36), Some([24, 112, 200, 255]));
    let png_bytes = pixmap.to_png().unwrap();
    assert!(png_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));

    // Or drive rendering through the object-safe `RenderBackend` trait:
    let backend: &dyn RenderBackend = &RustBackend::new();
    let rendered = backend.open_bytes(Arc::from(bytes), None).unwrap();
    assert_eq!(rendered.page_count(), 1);
}
```

## Architecture & Re-exported Crates

Every high-level method on `Pdf`, `Page`, and `Document` is also available as a standalone function in `pdfs::file`, `pdfs::page`, and `pdfs::build` over lower-level traits. When you need finer control, drop down to the re-exported crates directly through `pdfs`, each under its short name (`pdfs::gfx`) and its crate name (`pdfs::pdf_gfx`) alike:

- **[pdf-tokenizer](https://crates.io/crates/pdf-tokenizer)** — `pdfs::tokenizer`. Zero-copy, `no_std` PDF lexer (`Rules::Iso` and `Rules::Pdfium`).
- **[pdf-objects](https://crates.io/crates/pdf-objects)** — `pdfs::objects`. Owned PDF objects with byte spans, object parser/writer, text string codecs, and `Resolve` / `Streams` traits.
- **[pdf-filters](https://crates.io/crates/pdf-filters)** — `pdfs::filters`. Standard stream filter decoders and Flate encoder (`miniz_oxide` and `zlib-rs`).
- **[pdf-crypt](https://crates.io/crates/pdf-crypt)** — `pdfs::crypt`. Standard security handler (RC4, AES-128, AES-256), password preparation, and permission flags.
- **[pdf-structure](https://crates.io/crates/pdf-structure)** — `pdfs::structure`. Cross-reference tables/streams, object streams, lazy `Reading`, in-memory `Document`, and full/incremental writers (`Pdf::reading`, `Document::structure`).
- **[pdf-color](https://crates.io/crates/pdf-color)** — `pdfs::color`. Color space parsing, §7.10 functions, and CIE/ICC evaluation.
- **[pdf-images](https://crates.io/crates/pdf-images)** — `pdfs::images` (`images` feature, default). Image XObject and inline image decoding (`Page::decode_image`), mask compositing, pixel scrubbing, and clean Flate/JPEG re-encoding.
- **[pdf-content-streams](https://crates.io/crates/pdf-content-streams)** — `pdfs::content_streams`. Content stream lexing, typed operators, graphics-state balance, splicing, and `ContentWriter`.
- **[pdf-semantics](https://crates.io/crates/pdf-semantics)** — `pdfs::semantics`. Dates, name/number trees, page trees, destinations, actions, outlines, page labels, embedded/associated files, optional content groups, viewer preferences, `/Info` dictionaries, and XMP metadata/conformance packets.
- **[pdf-fonts](https://crates.io/crates/pdf-fonts)** — `pdfs::fonts`. Font dictionaries, encodings, CMaps, `/ToUnicode`, embedded font programs, Standard 14 metrics, text extraction, and TrueType font subsetting and embedding.
- **[pdf-gfx](https://crates.io/crates/pdf-gfx)** — `pdfs::gfx`. 2D PDF geometry (`PageGeom`), bounded graphics and text state interpreter, position-independent content re-encoder, spatial `TextPage` / search, and vector/text `Canvas` (with multiline `text_box`).
- **[pdf-annots](https://crates.io/crates/pdf-annots)** — `pdfs::annots`. Page annotations (`Page::annots`), typed subtype inspection, `/AP` placement and synthesis, `BuiltAnnot` / `AnnotMut` builders, reply threads, and zero-I/O page flattening (`Document::flatten_page`).
- **[pdf-forms](https://crates.io/crates/pdf-forms)** — `pdfs::forms`. Interactive forms: the `/AcroForm` field tree with inherited types, flags, values, and default appearances, each field's widgets, a check box or radio button field's rules for what a press or a value turns on (`group`), and the appearance a text field's, a button's, or a choice field's widget without one shows (`appearance`).
- **[pdf-raster](https://crates.io/crates/pdf-raster)** — `pdfs::raster` (`raster` feature, default). Pure-Rust 2D PDF page, tile, Form XObject, annotation, and interactive form rasterizer (`Page::render`, `RenderedDoc` on `Pdf`, `RustBackend`).
- **[pdf-linearize](https://crates.io/crates/pdf-linearize)** — `pdfs::linearize`. Linearized PDF layout engine, hint tables, and validator.

## Key Guarantees

- **Viewer defaults, validator opt-in:** `Options::DEFAULT` uses forgiving PDFium-compatible parsing (as of chromium/8152) and `zlib-rs` with zero-overhead silent fault handling. Switching to strict validation takes four fields on a copy of `Options::DEFAULT`: `Recovery::Verified`, `Rules::Iso`, `Faults::Kept`, and `Backend::MinizOxide`.
- **Unified resource caps (`Limits`):** One `Limits` struct bounds stream decompression, object nesting, tree depth, and per-call `font_budget`. Untrusted or hostile files return typed errors or partial-read markers (`Shown::unread`, `Verdict::judged_whole`) and never panic.
- **Opt-in spec diagnostics:** Methods with `_with` suffixes (`pages_with`, `info_with`, `claims_with`, `linearized_with`) stream recoverable spec departures to any `fault::Sink`.
- **Explicit lock & decode states:** Password-locked streams fail with `Why::Locked` (never returning raw ciphertext as text), and damaged/unsupported content streams are recorded explicitly (`Concat::undecoded`, `Unread::Undecoded`). Document permission flags (`Permissions`) are reported to the caller rather than enforced by `edit` or `save`.
- **Thread-safe & secret-safe:** `Pdf`, `Content`, `Document`, `EmbeddedFont`, and `Shown` are `Send + Sync`. `Debug` implementations and `tracing` spans never expose passwords, keys, or document text.

## Cargo Features

- **`images`** *(default)* — Enable `pdfs::images` (`pdf-images`) and `Page::decode_image` / `Page::decode_image_raw`.
- **`raster`** *(default; implies `images`)* — Enable `pdfs::raster` (`pdf-raster`), `Page::render` / `Page::render_pixmap`, `RenderedDoc` on `Pdf`, and `RustBackend`. `RustBackend` opens one buffer (`open_bytes`) or, with no copy of the whole, a source read by position or several shared buffers (`open_source`, `RenderSource`, `Parts`), and a failed open is `BackendError::Locked`, `BackendError::Refused` (with why, and converting back to the reader's own `Error`) or the reader's own error. A host's `ByteSource` is given to a backend as `AsRenderSource`.
- **`programs`** *(default)* — Parse embedded font programs via `skrifa` for glyph bounding boxes, heights, and fallback character mappings.
- **`text-layout`** *(default)* — Enable `Page::text_page`, `Page::search`, `unicode-bidi` RTL reordering, selection quads, and multiline `Canvas::text_box` layout via `pdf-gfx/text-layout`.
- **`clock`** *(default)* — Enable `Date::now` and `Date::from_unix` in `pdf-semantics`.
- **`icc`** *(default)* — Enable pure-Rust `ICCBased` and `/OutputIntents` profile evaluation via `pdf-color/icc` (`moxcms`).
- **`embed`** *(default)* — Enable `Document::embed_font` and `EmbeddedFont`: embed a TrueType program, subset to the glyphs drawn, to draw Unicode text in, via `pdf-fonts/embed` (which brings in `subsetter`), `pdf-gfx/embed` (its `ProgramFont`, which `EmbeddedFont` lays text out through), and `pdf-forms/embed` (`appearance::text_with_fallback`, a text field's value drawn in a fallback program, with `text-layout`). With a document's `EmbeddedFont` as the fallback (`Fallback::new(font.res(), font.program())`), the glyphs `text_with_fallback` returns are given to `EmbeddedFont::record`, so the next save writes the font with them; or the value is drawn by `appearance::text` in `WithFallback::new(&field_font.layout(), &font, font.res())`, where the `EmbeddedFont` records each glyph as it's drawn.
- **`image-ends`** — Find inline DCT (JPEG) and CCITTFax image boundaries by decoding image data, matching PDFium.
- **`predefined-cmaps`** — Bundle Adobe predefined CJK CMaps (via `hayro-cmap`) for CJK fonts without `/ToUnicode`.
- **`simd`** — Enable runtime SIMD dispatch in `pdf-content-streams`, `pdf-images`, and `pdf-raster`.
- **`pdfium`** *(implies `raster`)* — Enable `pdfs::PdfiumBackend` (`pdfs::raster::pdfium::PdfiumBackend`), a `#![forbid(unsafe_code)]` dynamic-library `RenderBackend` adapter over `pdfium-render` (`=0.9.6`, `thread_safe`, `pdfium_8125` ABI, verified against `chromium/8152`). `libpdfium` is not bundled: `PdfiumBackend::bind_to_library` loads an explicit path, while `PdfiumBackend::from_env` checks `$PDF_RASTER_PDFIUM`, `$PDFIUM_LIB_DIR`, and the system dynamic loader.
- **`tracing`** — Emit `debug`-level `tracing` spans (numeric metadata only) around major read/write operations.
- **`insecure-test-writer`** — Deterministic seeded encryption keys/IVs for reproducible tests and fuzzing only (**never** enable in production builds).

## Scope & Limitations

- **Pure-Rust renderer approximations & external `libpdfium`:** In `pdfs::raster` (`Page::render`), Gouraud and patch mesh shadings (`ShadingType 4–7`) fall back to `/Background` when present, non-separable blend modes (`Hue`, `Saturation`, `Color`, `Luminosity`) fall back to `Normal`, non-isolated or knockout transparency `/Group`s composite as isolated `sRGB` layers, and unembedded fonts without a `FontFallback` provider draw built-in legibility contours—each flagged via `RenderSummary::approximated` and `pdfs::raster::Fault` under `_with`. `PdfiumBackend` (`pdfium` feature) requires an external `libpdfium` shared library at runtime (`Viewport::Custom` with `draw.forms == true` is refused as `BackendError::Unsupported`).
- **Annotation & Type 3 text extraction:** `Page::text` and `Page::text_page` walk page `/Contents` and drawn Form XObjects; Type 3 glyph streams and annotation `/AP` streams are not walked automatically during text extraction (`Page::render` renders all of them visually).
- **No digital signing:** Rewriting a signed PDF whole or linearized invalidates existing byte-range signatures (use incremental updates carefully if preserving prior bytes).
- **Font embedding is TrueType only, without shaping:** `Document::embed_font` takes programs with TrueType (`glyf`) outlines; CFF- and CFF2-outline OpenType fonts (most `.otf` files and CJK `.ttc` collections) and bitmap-only fonts are refused, as are fonts whose licence restricts embedding unless the caller says it has the right. Text is drawn one glyph per character in the order given (no ligatures, kerning, or right-to-left reordering), and justified `text_box` lines in an embedded font are laid out flush left, since PDF word spacing applies only to single-byte codes.
- **Standard 14 `/Widths` in PDF 2.0:** Standard 14 font dictionaries omit `/Widths` (valid in PDF 1.0–1.7; PDF 2.0 requires explicit `/Widths`).
- **XMP replacement:** `Document::claim` writes a fresh XMP packet and refuses catalogs that already have a `/Metadata` stream.

## Stability

`pdfs` is currently in alpha (`=0.1.0-alpha.1`). Pin the exact version in `Cargo.toml` (`pdfs = "=0.1.0-alpha.1"`), as alpha releases may introduce breaking API changes documented in `CHANGELOG.md`. Requires Rust **1.92** or later.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
