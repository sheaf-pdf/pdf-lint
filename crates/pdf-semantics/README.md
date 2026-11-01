# pdf-semantics

PDF's common structures (ISO 32000-2, 7.9), read and written, each value saying where it came from, in a small Rust crate on [pdf-objects](../pdf-objects).

It's made for what reads a file's common structures to show them, check them or edit them, and what writes them.

## What it does

- **Dates (7.9.4).** `Date::read` reads one, each field saying whether it was written, what stands for it if not by the clause's defaults, and what the reading read past (`Quirk`): the 1.7 form's last apostrophe, hours without a sign, an offset before the fields it follows, a lone digit, what follows the date, a day its month hasn't. A field out of its range makes it no date. `Date::write` writes the clause's form of the fields, never the bytes read, and `Date::new` and the `with_` methods build a date, each field in order and in range.
- **Text strings (7.9.2.2).** `text` is pdf-objects' module: read by ISO's rules or PDFium's, every departure said, and written, as itself when PDFDocEncoding reads it back so, or in UTF-16BE.
- **Name and number trees (7.9.6, 7.9.7).** `tree::names` and `tree::numbers` walk one a key and a value at a time, in the order it's written, borrowing from what the resolver keeps and reading nothing it hasn't been asked for, under caps of depth and nodes, each node read once so a loop ends. Each departure is told to a sink, and `whole` says whether anything was left unread. `tree::write_names` and `tree::write_numbers` write one flat, sorted, a key given twice refused.

Whatever walks a file's objects reads them through pdf-objects' `Resolve`, so over pdf-structure's reading in memory or by position, its document, or a closure alike. `catalog` finds a file's catalog from its trailer, and `follow` follows a reference. A reader says why it gives no object, and a walk acts on it: an object that's damaged is said and passed; one past a cap the reader reads under, one that takes a password, or bytes the source didn't give end the walk with an `Unreadable`, so that a failed read never looks like an empty tree.

Where a file departs from the standard is a `Fault`, told to a `Sink` with its `Place`. `()` drops them and wants none, so what only a checker wants isn't looked for, and what's read is the same whatever the sink. Caps are one value, `Limits`.

## The `clock` feature

On by default. With it, `Date::now` and `Date::from_unix` give a date from the clock or a count of seconds, on [chrono](https://crates.io/crates/chrono) without its default features, so without its time zones. A crate that only reads and writes dates, or walks trees, turns it off with `default-features = false`, and chrono isn't built.

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

A name tree, over a closure; a resolver of a file's goes the same way:

```rust
use std::collections::BTreeMap;

use pdf_objects::{Object, ObjectId, dict};
use pdf_semantics::tree::names;
use pdf_semantics::{Fault, Limits, Place, Sink};

#[derive(Default)]
struct Faults(Vec<Fault>);

impl Sink for Faults {
    fn fault(&mut self, _: Place, fault: Fault) {
        self.0.push(fault);
    }
}

// A root, its kid by reference and one listed again.
let leaf = dict! {
    "Limits" => vec![Object::string("a"), Object::string("b")],
    "Names" => vec![
        Object::string("a"), Object::reference(7),
        Object::string("b"), Object::from(2),
    ],
};
let objects = BTreeMap::from([((2, 0), Object::from(leaf))]);
let resolve = |id: ObjectId| objects.get(&id);
let root = Object::from(dict! { "Kids" => vec![Object::reference(2), Object::reference(2)] });

let mut walk = names(&resolve, &root, &Limits::DEFAULT, Faults::default());
let (key, value) = walk.next().unwrap().unwrap();
assert_eq!((key, value.as_reference()), (&b"a"[..], Some((7, 0))));
let (key, value) = walk.next().unwrap().unwrap();
assert_eq!((key, value.as_integer()), (&b"b"[..], Some(2)));
assert!(walk.next().is_none());

// Read once, said, and nothing left unread.
assert!(walk.whole());
assert_eq!(walk.into_sink().0, [Fault::TreeRepeated { id: (2, 0) }]);
```

## What it doesn't do

- **Comparing dates across offsets, or showing them.** A date is its fields; a calendar's arithmetic is a caller's, on what `_or_default` gives.
- **Looking a key up by `/Limits`.** A walk's `find` serves, reading what comes before the key.
- **A tree written in nodes.** Nodes under a root would each need an object's number, so that's for what holds a document. A flat tree is a whole one at any size.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.
