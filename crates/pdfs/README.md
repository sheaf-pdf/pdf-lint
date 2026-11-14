# pdfs

Read, inspect, edit, and write PDF files through a single Rust dependency.

`pdfs` combines a modular family of PDF crates behind an ergonomic, viewer-compatible API: open files lazily from disk or memory, inspect pages and metadata, extract text and fonts, draw new pages, encrypt with AES-256, linearize for fast web viewing, or append incremental updates. Every underlying crate in the family is also re-exported at the root (`pdfs::pdf_structure`, `pdfs::pdf_objects`, etc.) at matching versions when you need low-level control.

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
    assert_eq!(page.attributes().media_box().value, Rect::LETTER);
    let shown = page.text().unwrap();
    assert_eq!(shown.text(), "Hello, world");
    assert!(shown.is_whole());
}
```

`Page::text` extracts strings in content-stream drawing order (including nested Form XObjects), matching PDFium's text extraction rules without spatial layout reconstruction. `Page::content` returns the page's decoded, concatenated content streams and parsed operators.

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

`Document::save(path)` writes compressed PDF 1.7 output (Flate-encoded cross-reference and object streams by default; customizable via `Document::save_options`). `Document::claim` attaches XMP conformance metadata (PDF/A, PDF/UA, PDF/VT).

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
let page = edited.add_page(Rect([0.0, 0.0, 400.0, 300.0]));
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

## Architecture & Re-exported Crates

Every high-level method on `Pdf`, `Page`, and `Document` is also available as a standalone function in `pdfs::file`, `pdfs::page`, and `pdfs::build` over lower-level traits. When you need finer control, drop down to the re-exported crates directly through `pdfs`:

- **[pdf-tokenizer](https://crates.io/crates/pdf-tokenizer)** — Zero-copy, `no_std` PDF lexer (`Rules::Iso` and `Rules::Pdfium`).
- **[pdf-objects](https://crates.io/crates/pdf-objects)** — Owned PDF objects with byte spans, object parser/writer, text string codecs, and `Resolve` / `Streams` traits.
- **[pdf-filters](https://crates.io/crates/pdf-filters)** — Standard stream filter decoders and Flate encoder (`miniz_oxide` and `zlib-rs`).
- **[pdf-crypt](https://crates.io/crates/pdf-crypt)** — Standard security handler (RC4, AES-128, AES-256), password preparation, and permission flags.
- **[pdf-structure](https://crates.io/crates/pdf-structure)** — Cross-reference tables/streams, object streams, lazy `Reading`, in-memory `Document`, and full/incremental writers (`Pdf::reading`, `Document::structure`).
- **[pdf-color](https://crates.io/crates/pdf-color)** — Color space parsing and component counting.
- **[pdf-content-streams](https://crates.io/crates/pdf-content-streams)** — Content stream lexing, typed operators, graphics-state balance, splicing, and `ContentWriter`.
- **[pdf-semantics](https://crates.io/crates/pdf-semantics)** — Dates, name/number trees, page trees, `/Info` dictionaries, and XMP conformance claims.
- **[pdf-fonts](https://crates.io/crates/pdf-fonts)** — Font dictionaries, encodings, CMaps, `/ToUnicode`, embedded font programs, Standard 14 metrics, and text extraction.
- **[pdf-linearize](https://crates.io/crates/pdf-linearize)** — Linearized PDF layout engine, hint tables, and validator.

## Key Guarantees

- **Viewer defaults, validator opt-in:** `Options::DEFAULT` uses forgiving PDFium-compatible parsing (as of chromium/8125) and `zlib-rs` with zero-overhead silent fault handling. Switching to strict validation takes four fields on a copy of `Options::DEFAULT`: `Recovery::Verified`, `Rules::Iso`, `Faults::Kept`, and `Backend::MinizOxide`.
- **Unified resource caps (`Limits`):** One `Limits` struct bounds stream decompression, object nesting, tree depth, and per-call `font_budget`. Untrusted or hostile files return typed errors or partial-read markers (`Shown::unread`, `Verdict::judged_whole`) and never panic.
- **Opt-in spec diagnostics:** Methods with `_with` suffixes (`pages_with`, `info_with`, `claims_with`, `linearized_with`) stream recoverable spec departures to any `fault::Sink`.
- **Explicit lock & decode states:** Password-locked streams fail with `Why::Locked` (never returning raw ciphertext as text), and damaged/unsupported content streams are recorded explicitly (`Concat::undecoded`, `Unread::Undecoded`). Document permission flags (`Permissions`) are reported to the caller rather than enforced by `edit` or `save`.
- **Thread-safe & secret-safe:** `Pdf`, `Content`, `Document`, and `Shown` are `Send + Sync`. `Debug` implementations and `tracing` spans never expose passwords, keys, or document text.

## Cargo Features

- **`programs`** *(default)* — Parse embedded font programs via `skrifa` for glyph bounding boxes, heights, and fallback character mappings.
- **`clock`** *(default)* — Enable `Date::now` and `Date::from_unix` in `pdf-semantics`.
- **`image-ends`** — Find inline DCT (JPEG) and CCITTFax image boundaries by decoding image data, matching PDFium.
- **`predefined-cmaps`** — Bundle Adobe predefined CJK CMaps (via `hayro-cmap`) for CJK fonts without `/ToUnicode`.
- **`simd`** — Enable runtime SIMD dispatch in `pdf-content-streams` dependencies.
- **`tracing`** — Emit `debug`-level `tracing` spans (numeric metadata only) around major read/write operations.
- **`insecure-test-writer`** — Deterministic seeded encryption keys/IVs for reproducible tests and fuzzing only (**never** enable in production builds).

## Scope & Limitations

- **No spatial text layout:** Text is returned in content-stream show order; Type 3 glyph streams and annotation appearances are not walked for text.
- **No rendering, form filling, signing, or custom font embedding:** Rewriting a signed PDF whole or linearized invalidates existing byte-range signatures (use incremental updates carefully if preserving prior bytes).
- **Standard 14 `/Widths` in PDF 2.0:** Standard 14 font dictionaries omit `/Widths` (valid in PDF 1.0–1.7; PDF 2.0 requires explicit `/Widths`).
- **XMP replacement:** `Document::claim` writes a fresh XMP packet and refuses catalogs that already have a `/Metadata` stream.

## Stability

`pdfs` is currently in alpha (`=0.1.0-alpha.1`). Pin the exact version in `Cargo.toml` (`pdfs = "=0.1.0-alpha.1"`), as alpha releases may introduce breaking API changes documented in `CHANGELOG.md`. Requires Rust **1.89** or later.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
