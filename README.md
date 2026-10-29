# pdf-lint

Checks PDF files against the
[Arlington PDF Model](https://github.com/pdf-association/arlington-pdf-model),
the PDF Association's machine-readable grammar of ISO 32000-2, and points
each finding at the bytes it's about.

It's made for tools that write PDFs. Most files in the wild break some
rule somewhere, so it can hold to the model only what your tool wrote:

```sh
pdf-lint out.pdf                     # every object
pdf-lint --last-update out.pdf       # what the newest incremental update wrote
pdf-lint --baseline in.pdf out.pdf   # findings in.pdf didn't already have
pdf-lint --hold objects.txt out.pdf  # the objects your tool says it wrote
```

It reads each file from its bytes, and decodes what's compressed and
decrypts what's encrypted itself.

| Crate | What |
|---|---|
| [pdf-lint](crates/pdf-lint) | The checker, a command line and a library. Its README has the rules, the options, the JSON format and how to install it |
| [pdf-tokenizer](crates/pdf-tokenizer) | A lossless, zero-copy PDF tokenizer, needing neither `std` nor `alloc` |
| [pdf-filters](crates/pdf-filters) | PDF's standard filters for data of any kind, decoded, every fault said and what decoded kept; and Flate encoded, with PNG's predictors, on miniz_oxide or zlib-rs |
| [pdf-crypt](crates/pdf-crypt) | PDF's standard security handler on RustCrypto's primitives: decrypting, every fault said, and encrypting for a writer, a new file's key and dictionary from randomness it's given, its keys wiped |
| [pdf-objects](crates/pdf-objects) | PDF's objects as a reader read them, their parser and their writer, which writes an encrypted file's through an encryptor: each value with where it was written, every fault said, and text strings decoded by the spec |
| [pdf-structure](crates/pdf-structure) | PDF's file structure, read whole or each object on demand, from any thread, from memory or a file read by position: the header, cross-reference tables and streams along `/Prev`, object streams and recovery by scanning, each object through pdf-objects' parser, decrypted through pdf-crypt and decoded through pdf-filters, and so any stream's data asked for, where it was read from recorded, every choice it made said; and written, whole or as an incremental update, cross-reference sections and object streams deflated through pdf-filters, and an encrypted file's objects encrypted as they're written, through pdf-crypt, by the list it decrypts by |
| [pdf-arlington](crates/pdf-arlington) | The Arlington PDF Model, embedded: its tables read, and the predicates in their cells evaluated over pdf-objects' objects |

## Development

The checks are recipes in the [justfile](justfile), run with
[just](https://github.com/casey/just):

```sh
just          # list the recipes
just ci       # run everything CI runs
just clippy   # or one at a time
```

`just ci` also needs the `thumbv6m-none-eabi`, `x86_64-pc-windows-gnu`
and `wasm32-unknown-unknown` targets, nightly Rust with its `rust-src`
component, and Rust 1.85 installed: for the tokenizer's no-std build,
pdf-structure's lints for Windows and a 32-bit target, the tokenizer's
build against `core` alone, and the minimum Rust of the tokenizer, the
filters, pdf-crypt, pdf-objects, pdf-structure and pdf-arlington.

`just fuzz tokenize`, `just fuzz filters`, `just fuzz crypt`, `just fuzz
objects`, `just fuzz structure`, `just fuzz write` and `just fuzz check`
fuzz the tokenizer, the filters, decoding and encoding, pdf-crypt's
handler, new files' dictionaries and a password's forms, pdf-objects'
parser and writer, pdf-structure's reader and writer, and the reader and
checker, on nightly with
[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz); a second argument
says for how many seconds (60 by default).

## License

pdf-tokenizer, pdf-filters, pdf-crypt, pdf-objects and pdf-structure are
licensed under either of Apache License, Version 2.0
([LICENSE-APACHE](LICENSE-APACHE)) or MIT license
([LICENSE-MIT](LICENSE-MIT)) at your option. pdf-lint and pdf-arlington
are licensed under the Apache License, Version 2.0, as is the Arlington
model pdf-arlington embeds.
