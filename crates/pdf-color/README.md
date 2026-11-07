# pdf-color

PDF's color spaces (ISO 32000-2, 8.6), read over any resolver of objects: `&reading`, `&document` or a closure. Nothing here converts a color, opens an ICC profile, evaluates a function or writes a space, which is pdf-objects' values, written as they are.

It's made for what reads content, which needs to know how many components a space has before it can say where an unfiltered inline image's data ends, and for what checks a file.

## What it does

- **Components as PDFium counts them.** `components` reads the value a `/ColorSpace` dictionary holds under a name, as written, or an inline image's array, as PDFium does as of chromium/8125: whatever the value holds, well-formed by ISO's rules or not. It gives the count, 3 where PDFium loads no space, or none for a reference to nothing, for which PDFium has no space at all.
- **As it's read.** A family is the first four bytes of a name or a string, a number is what PDFium's accessors make of it, and references are followed as far as they do, so a name through two references is no space, and a real for `/N` is cut to an integer. A function's read by its dictionary alone, for how many inputs and outputs it has.
- **Where it parts from PDFium.** No stream is decoded, so a sampled function too short for its sizes and a PostScript function that doesn't parse count as loading. Past the caps a branch doesn't load, where PDFium has only its stack. And it reads what the resolver gives, and what the file's reader makes of an object is that reader's own: a reference's generation counts, a real has been through an `f64`, a key is as the reader read it, a value nested in place holds all that the reader reads, to 256 levels, where PDFium's parser stops 64 levels from the object's root, and a stream without its `endstream`, or with a space after its `stream`, is read otherwise. `tests/pdfium/differ.tsv` lists each, with why.
- **The spaces, typed, as ISO 32000-2 says them.** `Space::read` gives a value's family and what its family holds: a base or an alternate space, boxed; `/N`; a white point; an Indexed table; a tint transform as written; a DeviceN space's names, each through its references and by its position, and its attributes. Everything borrows from the file. `Space::components` is how many values a color in it takes, `Space::colorants` a Separation's one or a DeviceN's names, and `Colorant::of` what a name says of itself: the four process names, `All`, `None` or a spot.
- **Each departure told.** A `Fault` names its clause, and is told to a sink, pdf-objects' `fault::Sink<Fault>`, with its `fault::Place`, the object it's in and where in it it was written. Reading goes on past each, so a space's parameters are each an `Option`. A sink that wants none, `()`, has none looked for: only what a `Space` holds is looked up, not a `/BlackPoint`, a tint transform or a `/Colorants` entry, and a check is on a budget of its own, so whatever the sink wants, the space given is the same, an `Err` included. A read only a check makes can't be the `Err`: damaged, it's told; any other failure leaves the check unsaid.
- **The two readings' one promise.** A space read with a sink that wants faults, none told, counts by `Space::components` as `components` counts it, where `counted` meets no cap, but for a Pattern space over more than 16 components, which no clause refuses and PDFium does. So a reader that takes one of them can trust the other on a well-formed space. A reference to a reference is told, as `Chained`, for that reason, though no clause forbids it, and so are a tint transform that PDFium wouldn't load and a white point that PDFium's 32-bit reading makes 0. With `()`, which has none looked for, a count says nothing of whether a space loads. Each reading spends its own visits, so past a cap only one of them may have met it: `TooDeep` is told at the depth `components` caps at, but `counted` can meet `space_visits` with no `TooManyVisits` told.
- **What a resources dictionary names.** `Spaces::read` finds a page's or a form's `/ColorSpace` dictionary, a stream's too, through at most two references each to `/Resources` and `/ColorSpace`, past which, or in a loop, it has none, and that's no error. A form with no `/Resources` takes what holds it, the parent form's or the page's; one with `/Resources` and none here takes the page's; so none is what its reader asks. For a page from pdf-semantics, it takes `Attribute::written`, not `value`, which has been followed. `Spaces::iter` and `Spaces::get` give each name's key as it was read, where it was written and its value, for `components`. It's PDFium's reading and tells nothing: an ISO check of a resources dictionary reads the dictionary itself.
- **Under caps.** `Limits` says how deep a space nests and how many objects are looked up for one, so a stitching function's shared subfunctions stop at a cost, not an exponent. Each is per space, so a large dictionary of honest spaces is never cut, and no cap covers the whole of one: its cost is its entries times `space_visits` times what each visit scans, which a caller reading untrusted files bounds by its own means. A cap met counts as what doesn't load, which `counted` tells apart. A reading holds a stack of the objects on its path and keeps nothing between calls, so these are plain functions over a reading shared between threads.
- **What fails.** An object that's damaged isn't there, as for PDFium, and any other failure to read one, past a reader's cap, locked, or a source that didn't give its bytes, is the `Err`, whole: a typed `pdf_objects::Unreadable`.

```rust
use pdf_color::pdf_objects::{Object, ObjectId, dict};
use pdf_color::{Limits, Spaces, components};

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

The same value, typed, with what's wrong with it told:

```rust
use pdf_color::pdf_objects::{Object, ObjectId};
use pdf_color::{Colorant, Fault, Family, Limits, Space};

let spot = Object::from(vec![Object::from("Separation"), Object::from("Gold"), Object::from("DeviceCMYK")]);
let nothing = |_: ObjectId| None;
let mut faults = Vec::new();
let space = Space::read(nothing, &spot, &Limits::DEFAULT, &mut faults).unwrap().unwrap();
assert_eq!(space.family(), Family::Separation);
assert_eq!(space.alternate(), Some(&Space::DeviceCMYK));
let colorant = space.colorants().next().unwrap();
assert_eq!(Colorant::of(colorant), Colorant::Spot);
// It has no tint transform.
assert_eq!(faults[0].1, Fault::Missing { element: 3 });
```

## How it's held to PDFium

`Space` is ISO's, so it's PDFium's only by the promise above. Each reading in `components` is a case in `tests/pdfium`, a file whose page draws an inline image in a space its resources name, so that the image's raw size says what PDFium counted, recorded at the pinned build. A plain `cargo test` holds the count to the recorded answers, and the repository's `just test-pdfium` asks every case of PDFium again; its `oracle/README.md` says how. Those files aren't in the package.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE)) or MIT license ([LICENSE-MIT](../../LICENSE-MIT)) at your option.
