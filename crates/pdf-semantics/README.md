# pdf-semantics

PDF's common structures (ISO 32000-2, 7.9), read and written, each value saying where it came from, in a small Rust crate on [pdf-objects](../pdf-objects).

It's made for what reads a file's common structures to show them, check them or edit them, and what writes them.

## What it does

- **Dates (7.9.4).** `Date::read` reads one, each field saying whether it was written, what stands for it if not by the clause's defaults, and what the reading read past (`Quirk`): the 1.7 form's last apostrophe, hours without a sign, an offset before the fields it follows, a lone digit, what follows the date, a day its month hasn't. A field out of its range makes it no date. `Date::write` writes the clause's form of the fields, never the bytes read, and `Date::new` and the `with_` methods build a date, each field in order and in range.
- **Text strings (7.9.2.2).** `text` is pdf-objects' module: read by ISO's rules or PDFium's, every departure said, and written, as itself when PDFDocEncoding reads it back so, or in UTF-16BE.

## The `clock` feature

On by default. With it, `Date::now` and `Date::from_unix` give a date from the clock or a count of seconds, on [chrono](https://crates.io/crates/chrono) without its default features, so without its time zones. A crate that only reads and writes dates turns it off with `default-features = false`, and chrono isn't built.

## An example

```rust
use pdf_semantics::date::{Date, Offset, Quirk};

let date = Date::read("D:20261024153000-08'00'").unwrap();
assert_eq!(date.month(), Some(10));
assert_eq!(date.offset(), Some(Offset::Behind { hours: 8, minutes: 0 }));

// The apostrophe PDF up to 1.7 wrote last, read past and said.
assert_eq!(date.quirks(), [Quirk::TerminatingApostrophe]);
assert_eq!(date.write(), "D:20261024153000-08'00");

// A field not written is none, and what stands for it the clause's default.
let date = Date::read("D:2026").unwrap();
assert_eq!((date.month(), date.month_or_default()), (None, 1));

// Built, each field after the one before it.
let date = Date::new(2026).unwrap().with_month(10).unwrap().with_day(24).unwrap();
assert_eq!(date.write(), "D:20261024");
assert!(Date::new(2026).unwrap().with_day(24).is_err());
```

## What it doesn't do

- **Comparing dates across offsets, or showing them.** A date is its fields; a calendar's arithmetic is a caller's, on what `_or_default` gives.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.
