# Changelog

## 0.1.0-alpha.1 (unreleased)

First release. It's an alpha: any alpha may change the API, and each entry here says what changed and how to update. Depend on the exact version, `=0.1.0-alpha.1`; a plain `0.1.0-alpha.1` lets cargo take later alphas too. It pins each alpha it re-exports exactly, so a release of any of them comes as a release of pdfs, and takes pdf-tokenizer on its `0.2` line.

### Added

- The family re-exported whole: pdf-tokenizer `0.2`, pdf-objects, pdf-filters and pdf-crypt `=0.1.0-alpha.3`, pdf-color `=0.1.0-alpha.2`, and pdf-structure, pdf-content-streams, pdf-semantics, pdf-fonts, pdf-gfx, pdf-annots and pdf-linearize `=0.1.0-alpha.1`.
- `open`, `from_bytes` and `Options`: a file opened by its path or from any source, as a viewer reads it by default (PDFium's recovery and rules, zlib-rs, no fault looked for). `Pdf` reads each object when it's asked for. `Pdf::unlock` opens it by a password after.
- `Pdf::pages`, `Pages::dests`, `Pdf::open_action`, `Page`, `Page::geom`, `Page::annots`, `Page::plan_flatten`, `Page::content`, `Page::interpret` and `Page::encode`: a file's pages and destinations, each page with what it inherits, its 2D geometry (`PageGeom`), its annotations (`Annot`), its streams joined (`Content`) with their operations, its graphics and text state interpreted (`Summary`), and its content and Form XObjects re-encoded (`pdf_gfx::Encoded`).
- `Page::text`, `Page::text_page`, `Page::search` and `Page::fonts`: the strings a page's content shows, in its fonts, its forms followed (`Shown`), spatial reading-order text, selection quads and search (`pdf_gfx::TextPage` and `TextMatch` under `text-layout`), and the fonts its resources name (`NamedFont`), each read once a file.
- `Pdf::version`, `encryption`, `info`, `claims`, `metadata`, `outlines`, `page_labels`, `embedded_files`, `associated_files`, `optional_content`, `catalog_view`, `linearized` and `stream`: what the file is and says of itself, its bookmarks, page labels, attachments, optional content layers, and viewer preferences, and any stream's data decoded. `Pdf::reading` gives the reading, for the rest.
- `Document`: a new document (`Document::new`) or a file edited (`Pdf::edit`), its pages added with a `ContentWriter` each (`add_page`), page IDs inspected (`page_ids`), annotations added or flattened (`add_annot`, `flatten_page`), document structures set (`set_info`, `set_metadata`, `set_outlines`, `set_page_labels`, `add_embedded_file`, `set_catalog_view`), what they draw with written once a file, the standard 14 fonts by name (`standard_font`) and claims as XMP (`claim`).
- `Document::embed_font`, `Document::embed_font_with` and `EmbeddedFont`, with the `embed` feature: a TrueType program embedded in a document to draw Unicode text in. `EmbeddedFont::res` is the resource a page's content names it by, and `EmbeddedFont::codes` a string's codes in it, each character's glyph recorded as it's drawn. With `text-layout` it's a `pdf_gfx::LayoutFont`, so `Canvas::text_box` lays text out in it. Each save writes the font at the number the document reserved for it, its program cut to the glyphs drawn by then (pdf-fonts' `embed::write`) and each of its streams deflated, so a save after more is drawn rewrites the same objects with more glyphs, and what was drawn before still shows. `Error::Embed` wraps pdf-fonts' `embed::Error`, and `prelude` gains `EmbeddedFont`.
- `Document::save`, `save_with`, `save_linearized` and `save_update`: a document saved whole, linearized, or as an update appended to the file read, each with a `_to` form that writes into memory. `encrypt` and `decrypt` say how it's encrypted.
- `page`, `file` and `build`: each recipe as a function over the family's traits, for a reading or a document of the caller's own.
- `Limits`, every crate's caps in one value, with `font_budget`, and `Error`, every crate's error wrapped as it is.
- `prelude`: the facade's types and calls, and the family's items they take and give: `Canvas`, `Color`, `ContentWriter`, `Matrix`, `PageGeom`, `Paint`, `Point`, `Quad`, `Res`, `Rotation`, `SearchOpts`, `TextAlign`, `TextBoxStyle`, `TextPage`, `TextPageOpts`, `VerticalAlign`, `WrapMode`, `Std14`, `Font`, `Shown`, `ShownString`, `Unread`, `Rect`, `Claims`, `Info`, `Verdict`, `Encryption`, `Encoded`, `Password`, `Permissions`, `Protection`, `Rules`, `Version`, `Dict`, `Object`, `ObjectId` and `Why`.
- Features, each the crates' own: `programs`, `text-layout`, `clock` and `embed` by default, and `image-ends`, `predefined-cmaps`, `simd` and `tracing`.

### Security

- `Document::encrypt` is AES-256, revision 6, its key and salts from the operating system. It's written under the document's own version, 1.7 for a new one, and `encrypt`'s docs say what a checker by version finds. An edited encrypted file is saved under its own encryption unless `decrypt` or `encrypt` says otherwise. A locked file isn't edited.
- Under an edited RC4 file's own encryption, new plaintext at a keystream the file used is refused unless the caller asks (`Document::reuse_rc4_keystreams`, for an update and a linearized save alike), and what's so written is listed.
- A save never writes over the file a `Pdf` reads by position (`Error::SameFile`), and an update is appended only to the file read, as it was (`Error::NotTheBase`). Neither is a durable write.
- What's behind a password reads as locked, never as ciphertext. No `Debug` holds a password, a key or a file's content, and a span's fields are numbers.
- What a file permits once a password opens it binds the caller, not the facade: opened by its user's password, a file that withholds changes is edited and saved, and `decrypt` saves it in the clear. `Pdf::encryption` gives the permissions and which password opened it.
- The `insecure-test-writer` feature, for tests and fuzzing, turns on pdf-structure's and pdf-linearize's test writer, which draws a new encryption's key from a seeded generator. A build that ships leaves it off.

### Notes

- It needs Rust 1.92. Nothing panics on any file, and past a cap a call says so.
- A page's or a form's stream that doesn't decode, damaged or through a filter the reader doesn't take, is read as none and said (`Concat::undecoded`, `Unread::Undecoded`), where PDFium reads its data as written.
- A page's box is written as the content writer writes a number, one that isn't finite as 0.
- `Options`, `Limits`, `Error` and `NamedFont` are `#[non_exhaustive]`, so each can grow in a minor release. The crates' own types keep their own policies.
- Only a program with TrueType outlines is embedded: one whose outlines are CFF's, CFF2's or bitmaps alone is refused (`Error::Embed`), as is one whose licence restricts it unless `embed_font_with`'s options say the caller has the right. Text is drawn in an embedded font a character at a time, without shaping: no ligatures, no kerning, no reordering of right-to-left text. Justified text in one is laid out flush left, as word spacing applies to no two-byte code.
- An `EmbeddedFont` is its document's: drawn on another's pages, it names an object there that isn't the font. A font embedded and never drawn in is written all the same, its `.notdef` alone. A program `embed_font` takes is one every save can cut, as pdf-fonts' `Program::open` refuses any other. `page_ids`, `add_annot`, `flatten_page` and `set_outlines` place the pages added but not the fonts, which a save writes as the glyphs drawn by then say.
- What isn't here: raster images, forms, signatures, fonts embedded from CFF programs, shaping, claims merged into a file's XMP, and a durable save.
