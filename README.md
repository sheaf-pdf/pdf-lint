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
| [pdf-tokenizer](crates/pdf-tokenizer) | A lossless, zero-copy PDF tokenizer, with no dependencies and needing neither `std` nor `alloc` |
| [pdf-filters](crates/pdf-filters) | PDF's standard filters for data of any kind, decoded, every fault said and what decoded kept |
| [pdf-crypt](crates/pdf-crypt) | PDF's standard security handler, decrypting on RustCrypto's primitives, every fault said |
| [pdf-objects](crates/pdf-objects) | PDF's objects as a reader read them, and their parser: each value with where it was written, every fault said, and text strings decoded by the spec |

## Development

The checks are recipes in the [justfile](justfile), run with
[just](https://github.com/casey/just):

```sh
just          # list the recipes
just ci       # run everything CI runs
just clippy   # or one at a time
```

`just ci` also needs the `thumbv6m-none-eabi` target and Rust 1.85
installed, for the tokenizer's no-std build and the minimum Rust of the
tokenizer, the filters, pdf-crypt and pdf-objects.

`just fuzz tokenize`, `just fuzz filters`, `just fuzz crypt`, `just fuzz
objects` and `just fuzz check` fuzz the tokenizer, the filters,
pdf-crypt's handler, pdf-objects' parser, and the reader and checker, on
nightly with
[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz); a second argument
says for how many seconds (60 by default).

## License

pdf-tokenizer, pdf-filters, pdf-crypt and pdf-objects are licensed under
either of Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
or MIT license ([LICENSE-MIT](LICENSE-MIT)) at your option. pdf-lint is
licensed under the Apache License, Version 2.0, as is the Arlington
model it embeds.
