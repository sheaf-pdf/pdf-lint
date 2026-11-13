# Changelog

## 0.1.0-alpha.1 (unreleased)

First release. It's an alpha: any alpha may change the API, and each entry here says what changed and how to update. Depend on the exact version, `=0.1.0-alpha.1`; a plain `0.1.0-alpha.1` lets cargo take later alphas too.

### Added

- `plan`: a `Graph` of a file's objects, from any object model, planned into Annex F's order, numbers and object streams (F.3), as a `Plan`. `plan::page_uses` and `plan::group` group objects by the pages that use them, for a writer that doesn't linearize.
- `assemble`: the bytes around a caller's bodies, from their lengths alone, as a `Layout`. `Layout::pieces` gives the file in order, a `layout::Pieces` iterator. A closure of the caller's filters or encrypts the layout's own streams.
- `hint`: every hint table of F.4 as a typed value, written to bits and read back. `hint::tables` reads all a hint stream has, from any bytes. `hint::Table` names each table.
- `check`: a file's `check::Facts`, from any model's reader, held against each other. It gives a `check::Verdict` and tells each `check::Finding` to a `check::Sink`, with its clause, its place and a `check::Grade`.
- `check::Grade::Unjudged`: what the reader didn't read for its own reason (`check::Unread`: no key, a cap, a filter it doesn't take), and what's past a cap of the check's own, told as not judged. It never makes a file not hold. `check::Verdict::judged_whole` says whether any part wasn't judged, whatever the sink.
- With the `family` feature, on by default: `family::linearize` and `family::linearize_bytes`, a pdf-structure document or a file's bytes written as a linearized file, in the clear or encrypted. `family::check`, `family::facts`, `family::check_with_reading` and `family::facts_with_reading` read a file's facts from a pdf-structure reading.
- `Limits`: caps on the numbers a file uses, its pages, the shared lists' entries and visits, and the entries hint tables can ask for.
- The `tracing` feature, off by default: a span over a plan, an assembly and each round of its first-page section, and a check, and with `family` over a document's linearizing and a file's facts, each stage and read within them. The `zlib-rs` feature: pdf-filters' zlib-rs backend, for `family::Options`.

### Security

- Under RC4, a string or stream that would go at a keystream the file read used is refused (`family::Error::Write` of pdf-structure's `Unencryptable::Keystream`). A caller who accepts what that leaks tells the encryptor to reuse them, and `family::Linearized::keystreams_reused` lists each object so written. `family::Options::rc4` says the same for `linearize_bytes`.
- Under an encryption, a direct `/Info` is written as an encrypted object that the trailer names, as a trailer isn't encrypted.
- What a file's object streams hid is in the clear once it's written unpacked, as only strings and streams are encrypted (7.6). A caller who minds packs, under an encryption that does.
- The hint stream's data is encrypted with the rest. PDFium reads hints before a password is given, so it reads an encrypted file's pages without them.
- A `Debug` of facts or of a layout holds no byte of the hint data or of the layout's bytes, as decrypted hint data is plaintext. A span's fields are numbers alone.
- The file is written anew, so a signature's byte range doesn't hold afterwards.
- The `insecure-test-writer` feature, for fuzzing, turns on pdf-structure's test writer, which draws a new file's key from a seeded generator and derives IVs. A build that ships leaves it off.

### Notes

- It's built on pdf-objects `=0.1.0-alpha.2`, and with `family` on pdf-structure and pdf-semantics `=0.1.0-alpha.1`, each re-exported whole. It needs Rust 1.89.
- qpdf's `--check-linearization` warns of some files ISO 32000-2 allows, which this crate writes. The repository's `tests/qpdf.rs` says why for each. Where the check reads a case otherwise than PDFium, `tests/pdfium/differ.tsv` lists it.
- Nothing panics on any graph, any hint table's bytes or any facts.
- `plan::What`, `layout::Piece`, `plan::Use`, the hint tables' headers, entries and tables, and `check::Attribute` are exhaustive on purpose, as a writer handles every one and the standard's tables and its list of inherited attributes are closed. Every other enum and every struct with public fields is `#[non_exhaustive]`, so each can grow in a minor release.
