# Changelog

## Unreleased

- **thiserror, without `std`.** `NumberError`'s `Display` and `Error` are derived with thiserror 2, built with its default features off, each text as before; `NumberError` is still exhaustive. It's the crate's one dependency, and it still needs only `core`: CI builds it against `core` alone (`-Z build-std=core`), for a target with neither `std` nor `alloc`. Building it now builds thiserror's derive, and the crates under it (`syn`, `quote`, `proc-macro2`, `unicode-ident`), for the host.

## 0.1.0 (2026-10-25)

- **The tokenizer.** `Tokenizer::new` and `Tokenizer::at` iterate a byte slice's `Token`s (kind, offset, and bytes as written) from any offset, losslessly: the tokens join back into the input. `offset()` says where the next token starts (not `position()`, which on a `&mut Tokenizer` would be `Iterator::position`). Kinds follow ISO 32000-2's lexical conventions, and a run of regular bytes is a number only when it's written as one.
- **Data it can't measure.** `eol()` and `space_byte()` take the white-space after `stream` and `ID`, and `data(len)` and `data_until(marker)` take the data after it as one `Data` token.
- **Byte classes.** `is_white_space`, `is_delimiter` and `is_regular`.
- **Decoding.** `Token::pieces` splits a name, a literal string or a hex string into `Piece`s, each a run of the token's bytes with the value byte it stands for and `How`, so every escape and every reader-dependent choice can be seen where it was written. `Pieces::bytes` gives the value's bytes, never more than the token's, and `Token::verbatim` the value as written, borrowed from the input, when nothing in it needs decoding. Nothing allocates. `Token::integer` and `Token::number` give an `i64` or an `f64`. `decode::name`, `literal`, `hex`, `integer` and `number` do the same for bytes from elsewhere.
- **Tests.** Unit tests for each kind, a property test and a fuzz target that check every promise on every token, and an ignored test that checks a folder of PDFs (`PDF_TOKENIZER_CORPUS`).
