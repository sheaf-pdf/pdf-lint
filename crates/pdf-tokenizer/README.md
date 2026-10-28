# pdf-tokenizer

A lossless, zero-copy tokenizer for PDF syntax, in one small Rust crate with no dependencies that needs neither `std` nor `alloc`, and never allocates.

It splits bytes into tokens and decides nothing else. What an object is, what's valid, and how to recover a damaged file are left to the layers above it, so a strict validator and a forgiving reader can share it, each with its own policy. Decoding a token into what it stands for is a separate step that hides nothing.

Every promise below is checked on every token by the tests, a fuzz target, and thousands of real and deliberately malformed PDFs.

## What it promises

- **Lossless.** Every byte is in exactly one token, white-space and comments included. Join the tokens and you have the input back, byte for byte.
- **Total.** It never rejects input, never repairs it, and never panics. A `)` with no string open, or a `>` that isn't part of `>>`, is a `Junk` token. A string that doesn't end is a string token marked unclosed. Every token moves it forward.
- **Zero-copy.** A token is a kind, an offset, and its bytes as written, borrowed from your input. Nothing is allocated, copied or decoded.
- **From any offset.** Start where a cross-reference entry points, in the middle of a file, or at the start of a content stream. The tokenizer carries nothing from one token to the next, so started at any token, it yields that token.
- **No grammar.** `obj`, `R`, `stream`, `xref`, `trailer`, `true` and `null` are keywords like any other, and `3 0 R` is three tokens with spaces between them. Joining tokens into references, objects or cross-reference sections is the next layer's work.
- **No allocation, no I/O.** It needs only `core`, so whatever an untrusted file holds, tokenizing and decoding it can't allocate memory or touch the system. CI checks this by building it for a target with neither `std` nor `alloc`. A value's bytes come as an iterator, to collect however you like, or borrowed as written when there's nothing to decode.

It follows the lexical conventions of ISO 32000-2 (§7.2): its white-space characters, its delimiters, and its regular characters. A run of regular characters is a number only when it's written as one, so `1e5` and `--5` are keywords.

## An example

```rust
use pdf_tokenizer::{Kind, Tokenizer};

let input = b"/Kids [3 0 R] % ok";
let tokens: Vec<(Kind, &str)> = Tokenizer::new(input)
    .map(|t| (t.kind, std::str::from_utf8(t.bytes).unwrap()))
    .collect();
assert_eq!(tokens, [
    (Kind::Name, "/Kids"),
    (Kind::Space, " "),
    (Kind::ArrayOpen, "["),
    (Kind::Integer, "3"),
    (Kind::Space, " "),
    (Kind::Integer, "0"),
    (Kind::Space, " "),
    (Kind::Keyword, "R"),
    (Kind::ArrayClose, "]"),
    (Kind::Space, " "),
    (Kind::Comment, "% ok"),
]);

// Lossless: the tokens join back into the input.
let joined: Vec<u8> = Tokenizer::new(input).flat_map(|t| t.bytes.iter().copied()).collect();
assert_eq!(joined, input);
```

Nothing is normalized. `/A#20B` stays `/A#20B`, `(caf\351)` keeps its escape, and `1.50` and `-.5` keep their digits.

## Binary data: the caller decides

After `stream`, the bytes are binary, and how many there are is written elsewhere: in the stream's `/Length`, which may be a reference to an object later in the file, and may be wrong. An inline image's data, after `ID` in a content stream, has no length written at all.

The tokenizer can't know where such data ends, so the caller says. It takes the white-space before the data with `eol()` or `space_byte()`, then the data itself as one `Data` token, with `data(len)` for a length it trusts or `data_until(marker)` to scan for `endstream` or `EI`. Readers differ most right here, so the choice belongs to the reader, not the tokenizer.

```rust
use pdf_tokenizer::{Kind, Tokenizer};

let input = b"<< /Length 5 >>\nstream\r\nBT ET\nendstream";
let mut tokens = Tokenizer::new(input);
assert!(tokens.by_ref().any(|t| t.bytes == b"stream"));

// The end of line after `stream`: CR LF here, which ISO 32000-2 allows.
assert_eq!(tokens.eol().unwrap().bytes, b"\r\n");

// The data, as `/Length` says. `data_until(b"endstream")` would scan instead.
let data = tokens.data(5);
assert_eq!((data.kind, data.bytes), (Kind::Data, &b"BT ET"[..]));
```

## Decoding

What a token stands for is in `decode`. A name's or a string's bytes come as pieces: every byte of the token is in exactly one, which says what value byte it stands for and how. So each escape, and each place where readers or PDF versions differ, can be seen with where it was written.

```rust
use pdf_tokenizer::Tokenizer;
use pdf_tokenizer::decode::How;

let name = Tokenizer::new(b"/A#20B").next().unwrap();
let pieces: Vec<_> = name.pieces().unwrap().map(|p| (p.at, p.byte, p.how)).collect();
assert_eq!(pieces, [
    (0..1, None, How::Delimiter),
    (1..2, Some(b'A'), How::Plain),
    (2..5, Some(b' '), How::Escape),
    (5..6, Some(b'B'), How::Plain),
]);

// Its value, as the pieces' bytes; borrowed as written only when there's
// nothing to decode, which there is here.
assert!(name.pieces().unwrap().bytes().eq(*b"A B"));
assert_eq!(name.verbatim(), None);
let plain = Tokenizer::new(b"/Type").next().unwrap();
assert_eq!(plain.verbatim(), Some(&b"Type"[..]));
```

Where ISO 32000-2 leaves a choice, or a reader departs from it, the decoder still gives a byte, and the piece says which it chose:

| `How` | Written | Read as |
|---|---|---|
| `StrayHash` | a `#` in a name without two hex digits after it | `#`, as before PDF 1.2. qpdf reads NUL |
| `NulEscape` | `#00` in a name | NUL, which a name may not hold. qpdf drops the value |
| `OctalOverflow` | `\777` | its low eight bits, as ISO 32000-2 says |
| `IgnoredBackslash` | `\q` | `q`, as ISO 32000-2 says |
| `EolAsLf` | an unescaped CR, or CR LF, in a string | LF, as ISO 32000-2 says |
| `OddDigit` | `<414>`'s last digit | as if `0` followed it |
| `Dangling` | a `\` that an unclosed string ends on | nothing |

Numbers decode too. `integer()` gives an `i64`, or `Overflow` outside it, where qpdf reads the whole object holding the integer as null. `number()` gives an `f64` for an integer or a real, and `1e5` and `inf` aren't numbers.

Text strings (PDFDocEncoding, and UTF-16BE or UTF-8 with a byte-order mark) are a level up: whether a string is text depends on the key it's under.

## What it's for

- **Validators and linters** that report syntax as written: a duplicate dictionary key, `1.0` where an integer belongs, a name's escapes, the end of line after `stream`, a cross-reference offset that misses its object. A parser that normalizes has already thrown these away.
- **Parsers with different policies** over one lexical layer: a strict one that fails on damage and a forgiving one that recovers, sharing the same tokens.
- **Editors and incremental writers** that must leave the bytes they don't change exactly as they were.
- **Recovery and forensics:** scanning a damaged file for `n g obj` headers when its cross-reference table is wrong.
- **Content streams,** which use the same syntax, with operators as keywords.

## What it isn't

- **A parser.** It builds no objects, and reads no cross-reference tables or object streams.
- **A decoder.** It applies no filters and decrypts nothing.
- **A repairer.** Damaged input comes out as tokens, not fixed.

## The API

- `Tokenizer::new(bytes)` and `Tokenizer::at(bytes, offset)`: an iterator of `Token`s. Cloning one is cheap, so a clone can look ahead.
- `Token { kind, start, bytes }`: `bytes` is a slice of the input, and `range()` says where it is.
- `Kind`: the lexical kinds, and `Data` for bytes the caller measured.
- `eol()`, `space_byte()`, `data(len)` and `data_until(marker)`: for the data after `stream` and `ID`.
- `is_white_space`, `is_delimiter` and `is_regular`: ISO 32000-2's three classes of byte.
- `token.pieces()`, `token.verbatim()`, `token.integer()` and `token.number()`: what a token stands for. A value's bytes are `pieces().bytes()`, never longer than the token, and `verbatim()` borrows them as written when nothing needs decoding. `decode::name`, `literal`, `hex`, `integer` and `number` decode bytes from elsewhere.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.
