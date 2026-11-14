# pdf-color

PDF color space parser and component counter (ISO 32000-2, §8.6), built on [pdf-objects](https://crates.io/crates/pdf-objects).

`pdf-color` operates over any `Resolve` implementation (`&reading`, `&document`, or a closure) without performing file I/O, evaluating functions, or decoding ICC streams. It serves two complementary needs: sizing unfiltered inline images in content streams (matching PDFium's component counts) and validating color space definitions against ISO 32000-2.

## Core Concepts

- **PDFium-compatible component counting (`components`, `counted`, `Spaces`):**
  - `components` and `counted` compute the number of color components for any `/ColorSpace` entry or inline-image color space array using PDFium's rules as of chromium/8125 (falling back to 3 components when PDFium fails to load a space, or `None` for dangling references).
  - `Spaces::read` locates a page or form XObject's `/ColorSpace` resource dictionary (following PDFium's reference-depth rules) and `Spaces::components` yields each named color space alongside its component count.
- **Typed ISO 32000-2 inspection (`Space`, `Family`, `Colorant`):**
  - `Space::read` parses a color space into its typed `Family` (`DeviceGray`, `DeviceRGB`, `DeviceCMYK`, `CalGray`, `CalRGB`, `Lab`, `ICCBased`, `Indexed`, `Pattern`, `Separation`, `DeviceN`), exposing component counts (`Space::components`), base/alternate spaces, white points, lookup tables, tint transforms, and `Colorant` names (`Colorant::of`).
  - Spec departures are reported with byte spans to a `pdf-objects` `fault::Sink<Fault>`. Passing `()` skips validation-only lookups at zero extra cost.
  - On any space that reports zero faults (and stays within caps), `Space::components` agrees with `components` (aside from `Pattern` spaces over >16 components, which PDFium rejects).
- **Resource bounds & error handling (`Limits`):** `Limits` caps color space nesting depth and per-space object visits so shared function graphs cannot trigger exponential traversal. Corrupt objects are treated as missing/faulted, while reader caps, locked files, or I/O errors propagate as `pdf_objects::Unreadable`.

## Quickstart

Count components across a `/Resources` dictionary:

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

let counts: Vec<(&[u8], Option<u32>)> = spaces
    .components(nothing, &Limits::DEFAULT)
    .map(|(named, counted)| (named.key, counted.unwrap().count))
    .collect();
assert_eq!(counts, [(&b"Spot"[..], Some(1)), (&b"Plain"[..], Some(3))]);
```

Parse the same `Separation` space as a typed ISO 32000-2 `Space` and inspect spec faults:

```rust
use pdf_color::pdf_objects::{Object, ObjectId};
use pdf_color::{Colorant, Fault, Family, Limits, Space};

let spot = Object::from(vec![
    Object::from("Separation"),
    Object::from("Gold"),
    Object::from("DeviceCMYK"),
]);
let nothing = |_: ObjectId| None;
let mut faults = Vec::new();
let space = Space::read(nothing, &spot, &Limits::DEFAULT, &mut faults).unwrap().unwrap();
assert_eq!(space.family(), Family::Separation);
assert_eq!(space.alternate(), Some(&Space::DeviceCMYK));
assert_eq!(Colorant::of(space.colorants().next().unwrap()), Colorant::Spot);
// Missing element 3 (the tint-transform function):
assert_eq!(faults[0].1, Fault::Missing { element: 3 });
```

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
