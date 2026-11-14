# pdf-tokenizer

A lossless, zero-copy PDF syntax tokenizer that requires neither `std` nor `alloc` and never allocates.

`pdf-tokenizer` splits raw PDF bytes into lexical tokens without applying grammar rules, normalizing escapes, or repairing malformed input. Higher-level object parsing, stream decompression, and file recovery live in the layers above (`pdf-objects`, `pdf-structure`, or the battery-included [pdfs](https://crates.io/crates/pdfs) crate), allowing strict validators and forgiving PDF viewers to share the exact same lexer.

## Core Guarantees

- **Lossless:** Every input byte belongs to exactly one `Token` (including whitespace and comments). Concatenating token slices reproduces the original byte slice verbatim.
- **Total & non-panicking:** Never rejects input, repairs syntax, or panics. Unmatched closing delimiters (`)`, `>`) produce `Kind::Junk`; unterminated strings yield `closed: false`; every step advances forward.
- **Zero-copy & `core`-only:** Each `Token` borrows a slice of the input with its byte offset and `Kind`. Built on `#![no_std]` with zero heap allocation (`-Z build-std=core` verified in CI).
- **Stateless from any offset:** Carries no cross-token state, so you can start lexing at any xref byte offset or content stream boundary.
- **Streaming-window friendly:** On a partial file window, only a token touching the end of the slice (`token.end() == window.len()`) could change if more bytes follow; every earlier token is final.
- **Grammar-free:** Keywords like `obj`, `R`, `stream`, `xref`, `trailer`, `true`, and `null` are emitted as plain `Kind::Keyword` tokens. `skim()` fast-forwards across nested `[` / `<<` delimiters without materializing interior tokens.

## Quickstart

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

// Lossless: joining token bytes reconstructs the exact input.
let joined: Vec<u8> = Tokenizer::new(input).flat_map(|t| t.bytes.iter().copied()).collect();
assert_eq!(joined, input);
```

## ISO 32000-2 vs. PDFium Lexical Rules

By default, `Tokenizer` follows ISO 32000-2 (§7.2, §7.3). Switch to PDFium's forgiving lexical rules—tested against PDFium as of chromium/8152—via `with_rules(Pdfium)` (compile-time type state) or `with_rules(Rules::Pdfium)` (runtime enum):

- **Numbers:** Runs consisting solely of digits, `+`, `-`, and `.` (such as `--5` or `1.2.3`) lex as `Integer` or `Real` under PDFium rather than `Keyword`.
- **Hex strings:** PDFium scans `<...>` to the next `>` across arbitrary bytes, whereas ISO 32000-2 stops before non-hex/non-whitespace bytes.
- **Whitespace:** PDFium treats `0x80` and `0xFF` as whitespace in addition to the six standard PDF whitespace bytes.

```rust
use pdf_tokenizer::{Kind, Rules, Tokenizer};

let input = b"--5 1.2.3 cm <4G1> Tj";
let kinds = |rules| -> Vec<Kind> {
    Tokenizer::new(input)
        .with_rules(rules)
        .map(|t| t.kind)
        .filter(|&k| k != Kind::Space)
        .collect()
};
assert_eq!(kinds(Rules::Iso), [
    Kind::Keyword,
    Kind::Keyword,
    Kind::Keyword,
    Kind::HexString,
    Kind::Keyword,
    Kind::Junk,
    Kind::Keyword,
]);
assert_eq!(kinds(Rules::Pdfium), [
    Kind::Integer,
    Kind::Real,
    Kind::Keyword,
    Kind::HexString,
    Kind::Keyword,
]);
```

## Stream and Inline-Image Binary Data

Binary payloads following `stream` or `ID` cannot be delimited by lexical rules alone (since `/Length` may be an indirect reference or inaccurate). After reading `stream` or `ID`, advance past the trailing newline or space with `eol()` or `space_byte()`, then consume the payload with `data(len)` (known length) or `data_until(marker)` (scan for `endstream` or `EI`):

```rust
use pdf_tokenizer::{Kind, Tokenizer};

let input = b"<< /Length 5 >>\nstream\r\nBT ET\nendstream";
let mut tokens = Tokenizer::new(input);
assert!(tokens.by_ref().any(|t| t.bytes == b"stream"));

assert_eq!(tokens.eol().unwrap().bytes, b"\r\n");
let data = tokens.data(5);
assert_eq!((data.kind, data.bytes), (Kind::Data, &b"BT ET"[..]));
```

## Token Value Decoding (`decode`)

Tokens keep raw bytes as written (`/A#20B`, `(caf\351)`, `-.5`). The `decode` module lazily decodes names, literal/hex strings, and numbers without allocating, reporting escape quirks (`How`, `Quirks`) with byte offsets:

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
assert!(name.pieces().unwrap().bytes().eq(*b"A B"));
assert_eq!(Tokenizer::new(b"/Type").next().unwrap().verbatim(), Some(&b"Type"[..]));
```

`decode::pdfium` provides PDFium-compatible value decoders (`number`, `real`, `name`, `literal`, `hex`) for malformed tokens:

```rust
use pdf_tokenizer::decode::pdfium::{self, Number};

assert_eq!(pdfium::number(b"4294967296"), Number::Unsigned(0));
assert_eq!(pdfium::number(b"+-.5").float(), -0.5);
assert!(pdfium::name(b"/F#3G").eq(*b"F0"));
assert!(pdfium::hex(b"<41 G 42>").eq(*b"AB"));
```

## API Map

- **`Tokenizer::new(bytes)` / `Tokenizer::at(bytes, offset)`:** Create an iterator of `Token`s (`offset()`, `input()`, `remaining()`).
- **`next_significant()` / `significant()`:** Skip whitespace and comments without constructing tokens for them.
- **`skim()` / `skimming()`:** Jump across `[` / `]` / `<<` / `>>` delimiters and unclosed strings using SIMD-accelerated byte searches (`memchr`). *(Enable `memchr/std` in `std` binaries for runtime AVX2 detection on x86_64.)*
- **`Token::pieces()`, `verbatim()`, `integer()`, `number()`:** Decode ISO 32000-2 values and inspect `Quirks`.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
