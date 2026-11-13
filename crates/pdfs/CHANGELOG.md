# Changelog

## 0.1.0-alpha.1 (unreleased)

First release. It's an alpha: any alpha may change the API, and each entry here says what changed and how to update. Depend on the exact version, `=0.1.0-alpha.1`; a plain `0.1.0-alpha.1` lets cargo take later alphas too. It pins each crate it re-exports exactly, so a release of any of them comes as a release of pdfs.

### Added

- The family re-exported whole: pdf-tokenizer `0.2`, pdf-objects and pdf-crypt `=0.1.0-alpha.2`, pdf-filters `=0.1.0-alpha.3`, and pdf-structure, pdf-color, pdf-content-streams, pdf-semantics, pdf-fonts and pdf-linearize `=0.1.0-alpha.1`.
- `open`, `from_bytes` and `Options`: a file opened by its path or from any source, as a viewer reads it by default (PDFium's recovery and rules, zlib-rs, no fault looked for). `Pdf` reads each object when it's asked for. `Pdf::unlock` opens it by a password after.
- `Pdf::pages`, `Page` and `Page::content`: a file's pages, each with what it inherits, and a page's streams joined (`Content`), with their operations.
- `Page::text` and `Page::fonts`: the strings a page's content shows, in its fonts, its forms followed, as PDFium reads them for its text (pdf-fonts' `Shown`), and the fonts its resources name (`NamedFont`), each read once a file.
- `Pdf::version`, `encryption`, `info`, `claims`, `linearized` and `stream`: what the file is and says of itself, and any stream's data decoded. `Pdf::reading` gives the reading, for the rest.
- `Document`: a new document (`Document::new`) or a file edited (`Pdf::edit`), its pages added with a `ContentWriter` each (`add_page`), what they draw with written once a file, the standard 14 fonts by name (`standard_font`) and claims as XMP (`claim`).
- `Document::save`, `save_with`, `save_linearized` and `save_update`: a document saved whole, linearized, or as an update appended to the file read, each with a `_to` form that writes into memory. `encrypt` and `decrypt` say how it's encrypted.
- `page`, `file` and `build`: each recipe as a function over the family's traits, for a reading or a document of the caller's own.
- `Limits`, every crate's caps in one value, with `font_budget`, and `Error`, every crate's error wrapped as it is.
- `prelude`: the facade's types and calls, and the family's items they take and give: `Color`, `ContentWriter`, `Paint`, `Res`, `Std14`, `Font`, `Shown`, `ShownString`, `Unread`, `Rect`, `Claims`, `Info`, `Verdict`, `Encryption`, `Encoded`, `Password`, `Permissions`, `Protection`, `Rules`, `Version`, `Dict`, `Object`, `ObjectId` and `Why`.
- Features, each the crates' own: `programs` and `clock` by default, and `image-ends`, `predefined-cmaps`, `simd` and `tracing`.

### Security

- `Document::encrypt` is AES-256, revision 6, its key and salts from the operating system. It's written under the document's own version, 1.7 for a new one, and `encrypt`'s docs say what a checker by version finds. An edited encrypted file is saved under its own encryption unless `decrypt` or `encrypt` says otherwise. A locked file isn't edited.
- Under an edited RC4 file's own encryption, new plaintext at a keystream the file used is refused unless the caller asks (`Document::reuse_rc4_keystreams`, or `rc4` in `save_linearized`'s options), and what's so written is listed.
- A save never writes over the file a `Pdf` reads by position (`Error::SameFile`), and an update is appended only to the file read, as it was (`Error::NotTheBase`). Neither is a durable write.
- What's behind a password reads as locked, never as ciphertext. No `Debug` holds a password, a key or a file's content, and a span's fields are numbers.
- The `insecure-test-writer` feature, for tests and fuzzing, turns on pdf-structure's and pdf-linearize's test writer, which draws a new encryption's key from a seeded generator. A build that ships leaves it off.

### Notes

- It needs Rust 1.89. Nothing panics on any file, and past a cap a call says so.
- `Options`, `Limits`, `Error` and `NamedFont` are `#[non_exhaustive]`, so each can grow in a minor release. The crates' own types keep their own policies.
- What isn't here: layout, images, forms, signatures, font embedding, claims merged into a file's XMP, and a durable save.
