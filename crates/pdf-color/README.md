# pdf-color

PDF's color spaces (ISO 32000-2, 8.6), read over any resolver of objects: `&reading`, `&document` or a closure. Nothing here converts a color, opens an ICC profile, evaluates a function or writes a space, which is pdf-objects' values, written as they are.

It's made for what reads content, which needs to know how many components a space has before it can say where an unfiltered inline image's data ends, and for what checks a file.

## What it does

- **Components as PDFium counts them.** `components` reads the value a `/ColorSpace` dictionary holds under a name, as written, or an inline image's array, as PDFium does as of chromium/8125: whatever the value holds, well-formed by ISO's rules or not. It gives the count, 3 where PDFium loads no space, or none for a reference to nothing, for which PDFium has no space at all.
- **As it's read.** A family is the first four bytes of a name or a string, a number is what PDFium's accessors make of it, and references are followed as far as they do, so a name through two references is no space, and a real for `/N` is cut to an integer. A function's read by its dictionary alone, for how many inputs and outputs it has.
- **Where it parts from PDFium.** No stream is decoded, so a sampled function too short for its sizes and a PostScript function that doesn't parse count as loading. Past the caps a branch doesn't load, where PDFium has only its stack. And it reads what the resolver gives: a reference's generation counts, a real has been through an `f64`, and a key is as the file's reader read it. `tests/pdfium/differ.tsv` lists each, with why.
- **What a resources dictionary names.** `Spaces::read` finds a page's or a form's `/ColorSpace` dictionary, a stream's too, through at most two references each to `/Resources` and `/ColorSpace`, past which, or in a loop, it has none, and that's no error. A form with none uses its page's, so none is what its reader asks. `Spaces::iter` and `Spaces::get` give each name's key as it was read, where it was written and its value, for `components`.
- **Under caps.** `Limits` says how deep a space nests and how many objects are looked up for one, so a stitching function's shared subfunctions stop at a cost, not an exponent. Each is per space, so a large dictionary of honest spaces is never cut. A reading holds a stack of the objects on its path and keeps nothing between calls, so these are plain functions over a reading shared between threads.
- **What fails.** An object that's damaged isn't there, as for PDFium, and any other failure to read one, past a reader's cap, locked, or a source that didn't give its bytes, is the `Err`, whole: a typed `pdf_objects::Unreadable`.

```rust
use pdf_color::{Limits, Object, ObjectId, Spaces, components};
use pdf_objects::dict;

let resources = Object::from(dict! {
    "ColorSpace" => dict! {
        "Spot" => vec![Object::from("Separation"), Object::from("Gold"), Object::from("DeviceCMYK")],
        "Plain" => "DeviceRGB",
    },
});
let nothing = |_: ObjectId| None;
let spaces = Spaces::read(nothing, Some(&resources)).unwrap().unwrap();
let spot = spaces.get(b"Spot").unwrap();
assert_eq!(components(nothing, spot.value, &Limits::DEFAULT), Ok(Some(1)));
let names: Vec<&[u8]> = spaces.iter().map(|named| named.key).collect();
assert_eq!(names, [&b"Spot"[..], &b"Plain"[..]]);
```

## How it's held to PDFium

Each reading in `components` is a case in `tests/pdfium`, a file whose page draws an inline image in a space its resources name, so that the image's raw size says what PDFium counted, recorded at the pinned build. A plain `cargo test` holds the count to the recorded answers, and the repository's `just test-pdfium` asks every case of PDFium again; its `oracle/README.md` says how. Those files aren't in the package.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.
