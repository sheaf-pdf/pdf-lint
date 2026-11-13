# pdfs

Read and write PDF files through one dependency. `pdfs` re-exports the crates of one family whole, each at the version it's built with, so you name any of their types through it and they work together. On top of them it has the recipes that span them, with a viewer's defaults: open a file, list its pages, read a page's content, fonts and text, and write a document, new or a file edited.

Each recipe is also a function over the family's traits, in `pdfs::page`, `pdfs::file` and `pdfs::build`, so code with a reading or a document of its own calls the same recipes. `pdfs::prelude` names what they take and give in one `use`.

## The family

Drop to a crate, through `pdfs`, for what the recipes don't do.

- **[pdf-tokenizer](https://crates.io/crates/pdf-tokenizer)** reads a file's bytes as tokens, by ISO 32000-2's rules or PDFium's, with no `std` or `alloc`. Use it to scan bytes without parsing objects.
- **[pdf-objects](https://crates.io/crates/pdf-objects)** has PDF's objects, their parser and writer, and text strings. Use it to build or inspect a dictionary, or to decode a string by hand.
- **[pdf-filters](https://crates.io/crates/pdf-filters)** decodes the standard filters and encodes Flate. Use it for a stream's data outside a file.
- **[pdf-crypt](https://crates.io/crates/pdf-crypt)** is the standard security handler. Use it to prepare a password or set what a protection grants.
- **[pdf-structure](https://crates.io/crates/pdf-structure)** reads a file whole or an object at a time, and writes one whole or as an update. Use it for an object by number, a file's revisions, or a save's every option (`Pdf::reading`, `Document::structure`).
- **[pdf-color](https://crates.io/crates/pdf-color)** reads color spaces. Use it to count a space's components or type it.
- **[pdf-content-streams](https://crates.io/crates/pdf-content-streams)** reads and writes content. Use it for a page's tokens and operators, its marked content, or content spliced.
- **[pdf-semantics](https://crates.io/crates/pdf-semantics)** reads and writes dates, name and number trees, the page tree and XMP claims. Use it to walk a name tree or read a date.
- **[pdf-fonts](https://crates.io/crates/pdf-fonts)** reads fonts as PDFium reads them, and the standard 14. Use it for a code's width, a glyph's box, or a font's encoding.
- **[pdf-linearize](https://crates.io/crates/pdf-linearize)** writes and checks linearized files. Use it to plan a layout for another object model, or to read the hint tables.

## Read a page's text

`pdfs::open(path)` opens a file on disk, reading it by position, and `pdfs::from_bytes` one in memory. Each object is read when it's first asked for, and kept, from any thread:

```rust
use pdfs::prelude::*;

// A file of one page, written here so the example runs on its own.
let mut doc = Document::new();
let page = doc.add_page(Rect::LETTER);
page.begin_text();
page.font(standard_font(Std14::Helvetica), 24.0);
page.text_move(72.0, 700.0);
page.show(&Std14::Helvetica.codes("Hello, world").unwrap());
page.end_text();
let file = doc.to_bytes().unwrap();

// Read it as any file is read.
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

`Page::text` gives the strings the content shows, in order, each in its font, the forms it draws followed, as PDFium reads them for its text. There's no layout: strings come as the content shows them, not where they are. `Page::content` gives the page's streams joined, and their operations.

## Write a page in a standard 14 font

`Document::add_page` gives a page's content writer. What it draws with, a font among it, is written once a file however many pages use it. `Std14::codes` puts text in a standard 14 font's codes, and refuses a character the font has no code for:

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

`Document::save(path)` writes it to a file, and `Document::claim` adds what it claims to conform to, as XMP. A save writes a cross-reference stream and object streams, deflated, under the header 1.7. `Document::save_options` gives those options to change.

## Encrypt and linearize

`Document::encrypt` encrypts each save after it with AES-256, under the passwords and permissions a `Protection` says. `Document::save_linearized` lays the file out for a viewer that shows the first page before the rest arrives:

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
// "reader" opens it, but may not print; "owner" may do anything.
let reader = Password("reader").prepared().unwrap();
let owner = Password("owner").prepared().unwrap();
let mut protection = Protection::new(&reader);
protection.owner = Some(&owner);
protection.permissions = Permissions::ALL.without(Permissions::PRINT);
doc.encrypt(&protection).unwrap();
let mut file = Vec::new();
let options = doc.linearize_options();
doc.save_linearized_to(&mut file, &options).unwrap();

// Without a password, nothing encrypted reads.
let locked = pdfs::from_bytes(&file[..]).unwrap();
assert_eq!(*locked.encryption(), Encryption::Locked);

// With one, it reads, and says what it grants.
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

`Pdf::unlock` opens a file after it was read without a password. Passwords are borrowed for the open, and nothing keeps them.

## Edit a file, and append an update

`Pdf::edit` gives a file's document to change. `Document::save_update` appends what changed to the file read, and `Document::save` writes it whole. An edited file is saved under its own encryption unless `Document::decrypt` or `Document::encrypt` says otherwise:

```rust
use pdfs::prelude::*;

let mut doc = Document::new();
let page = doc.add_page(Rect::LETTER);
page.begin_text();
page.font(standard_font(Std14::Helvetica), 12.0);
page.show(&Std14::Helvetica.codes("First").unwrap());
page.end_text();
let file = doc.to_bytes().unwrap();

// Add a page, and write what changed as an update after the file.
let read = pdfs::from_bytes(&file[..]).unwrap();
let mut edited = read.edit().unwrap();
let page = edited.add_page(Rect([0.0, 0.0, 400.0, 300.0]));
page.begin_text();
page.font(standard_font(Std14::Courier), 12.0);
page.show(&Std14::Courier.codes("Second").unwrap());
page.end_text();
let mut update = Vec::new();
edited.save_update_to(&mut update).unwrap();

// The file as it was, then the update: both pages.
let updated = pdfs::from_bytes([&file[..], &update[..]].concat()).unwrap();
let pages = updated.pages().unwrap();
let texts: Vec<String> = (pages.iter())
    .map(|page| page.text().unwrap().text())
    .collect();
assert_eq!(texts, ["First", "Second"]);
```

`save_update(path)` appends only to a file that holds the file read, as it was, so never to one that changed since. A save never writes over the file a `Pdf` from `pdfs::open` reads by position. To change that file in place, read it into memory first with `pdfs::from_bytes(std::fs::read(path)?)`. Neither save is a durable write.

## What it promises

- **A viewer's defaults.** `Options::DEFAULT` reads a damaged file as PDFium does, walks the page tree and decodes text by PDFium's rules, inflates on zlib-rs, and looks for no fault. A checker's are one field each: `Recovery::Verified`, `Rules::Iso`, `Faults::Kept`, `Backend::MinizOxide`.
- **Caps in one value.** `Limits` holds each crate's caps, and `font_budget` bounds what one call's fonts and forms decompress to. Past a cap, a call says so, in its error or, for a page's text, in `Shown::unread`. Nothing is guessed past one, and no file makes it panic.
- **Faults where you ask for them.** `Pdf::pages_with`, `info_with`, `claims_with` and `linearized_with` tell a crate's faults to the sink they're given. The plain forms tell nothing, and look for nothing. With `()`, a metadata packet that isn't UTF-8 reads as one that claims nothing, and `claims_with` tells the two apart.
- **What's locked reads as locked.** Behind a password, a page's content is an error whose `Why` is `Locked`, never ciphertext read as text.
- **Threads.** `Pdf`, `Content`, `Document` and pdf-fonts' `Shown` are `Send` and `Sync`.
- **No secrets in a `Debug`.** No `Debug` of the facade's holds a password, a key or a file's content.

Where a reading follows PDFium, it's PDFium as of chromium/8125, written in each crate's own code. Each crate's `tests/pdfium` in the repository records its cases, and its `differ.tsv` where it reads one otherwise.

## Features

- **`programs`**, on by default: pdf-fonts reads a font's program, for its glyphs' boxes and the text its encoding leaves to it.
- **`clock`**, on by default: pdf-semantics' `Date::now` and `Date::from_unix`.
- **`image-ends`**, off by default: pdf-content-streams ends an inline image's DCT and CCITTFax data by decoding it, as PDFium does.
- **`predefined-cmaps`**, off by default: pdf-fonts reads Adobe's predefined CMaps, from hayro-cmap's bundle, so CJK text with no `/ToUnicode` reads.
- **`simd`**, off by default: pdf-content-streams' dependencies use the CPU's SIMD, chosen as it runs.
- **`tracing`**, off by default: a span at `debug` over each recipe, and each crate's own spans, through [tracing](https://crates.io/crates/tracing). A span's fields are numbers, never a byte of a file.
- **`insecure-test-writer`**, off by default, for tests and fuzzing: pdf-structure's and pdf-linearize's test writer, which draws a new encryption's key from a seeded generator and derives every IV, so a test writes the same bytes each run. It's never for a document's protection, and a build that ships leaves it off.

## Exhaustive on purpose, or not

`Options`, `Limits`, `Error` and `NamedFont` are `#[non_exhaustive]`, so each can grow in a minor release. Set an option or a cap on a copy of `Options::DEFAULT` or `Limits::DEFAULT`. The crates' own types keep their own policies.

## What isn't here

- **Layout.** Text comes as the content shows it, not where it is on the page. A Type 3 font's glyphs and an annotation's appearance aren't read for text.
- **Images, forms, signatures and font embedding.** No recipe reads an image, fills a form, signs a file or embeds a font. A whole or linearized save writes the file anew, so a signature's byte range no longer holds.
- **A standard 14 font's widths.** PDF 2.0 asks for them, so a page drawn in one and saved as 2.0 lacks them.
- **Claims merged into a file's XMP.** `Document::claim` refuses a catalog that has a metadata stream already.
- **A durable save,** and a command line.

## Stability

This is an alpha: any alpha may change the API, and the CHANGELOG says how to follow. Depend on the exact version, `pdfs = "=0.1.0-alpha.1"`. It pins each crate it re-exports exactly, so a release of any of them comes as a release of pdfs. It needs Rust 1.89 or later.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
