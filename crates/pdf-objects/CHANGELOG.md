# Changelog

## 0.1.0 (unreleased)

- **The objects.** `Object`, a `Value` (null, boolean, integer, real, name, string, array, dictionary, stream, reference) and where it was written, a `Span` of the file's bytes or of an object stream's decoded data, when the reader knows. A `Dict` keeps its entries as written, in order, a key written twice and each key's span among them: lookups take a key's last entry, `duplicates` gives the others. A `Str` is its bytes; or text a reader decoded without saying from which encoding; or ciphertext it didn't decrypt. `dict!` builds a dictionary from keys and values. `Span` is `#[non_exhaustive]`, made by `Span::new`, so a field can come without a breaking release.
- **Text strings.** `text::decode` decodes a text string by its byte order mark, UTF-16BE, UTF-8 or none for PDFDocEncoding (Annex D, `text::pdf_doc_char`), and says each `Quirk`, where it departs from the spec: UTF-16LE's mark, read as PDFDocEncoding; an odd last byte, left out; a lone surrogate or bad UTF-8, read as U+FFFD; a byte PDFDocEncoding leaves undefined.
- **Versions.** `Version`, read from a header's or a catalog's text, displayed as `1.7`, and ordered.
- **From pdf-lint.** These are pdf-lint 0.2.0's `object`, `text` and `version` modules, what they do unchanged (but for `text::passwords`, which is pdf-crypt's), which pdf-lint re-exports where they were. Rust 1.85 builds it.
- **Tests.** Unit tests for dictionaries as written, values, each text encoding and each quirk, the PDFDocEncoding table and versions, and the README's example as a doctest.
