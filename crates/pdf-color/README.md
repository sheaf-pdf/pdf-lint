# pdf-color

PDF color space and §7.10 function parser, component counter, and color evaluator (ISO 32000-2, §7.10 & §8.6), built on [pdf-objects](https://crates.io/crates/pdf-objects).

`pdf-color` operates over `Resolve` (for zero-stream parsing and component counting) and `Resolve` + `Streams` (for bounded function and color evaluation) without performing file I/O.

## Core Concepts

- **PDFium-compatible component counting (`components`, `counted`, `Spaces`):**
  - `components` and `counted` compute the number of color components for any `/ColorSpace` entry or inline-image color space array using PDFium's rules as of chromium/8152 (falling back to 3 components when PDFium fails to load a space, or `None` for dangling references).
  - `Spaces::read` locates a page or form XObject's `/ColorSpace` resource dictionary (following PDFium's reference-depth rules) and `Spaces::components` yields each named color space alongside its component count.
- **Typed ISO 32000-2 inspection & image helpers (`Space`, `Family`, `Colorant`):**
  - `Space::read` parses a color space into its typed `Family` (`DeviceGray`, `DeviceRGB`, `DeviceCMYK`, `CalGray`, `CalRGB`, `Lab`, `ICCBased`, `Indexed`, `Pattern`, `Separation`, `DeviceN`), exposing component counts (`Space::components`), default image `/Decode` intervals (`Space::default_decode`), lookup palette extraction (`Indexed::lookup_bytes`), base/alternate spaces, white points, tint transforms, and `Colorant` names (`Colorant::of`).
  - Spec departures are reported with byte spans to a `pdf-objects` `fault::Sink<Fault>`. Passing `()` skips validation-only lookups at zero extra cost.
  - On any space that reports zero faults (and stays within caps), `Space::components` agrees with `components` (aside from `Pattern` spaces over >16 components, which PDFium rejects).
- **§7.10 functions & color evaluation (`Function`, `PreparedSpace`, `EvalOptions`, `Paint`):**
  - `Function::read` loads and evaluates `Type 0` (sampled, `1..=32` bps; `/Order 3` cubic-spline declarations evaluate with multilinear interpolation matching PDFium), `Type 2` (exponential), `Type 3` (stitching), and `Type 4` (PostScript calculator stack machine, enforcing strict Table 42 integer/boolean types under `Rules::Iso` and PDFium's float-coercing stack under `Rules::Pdfium`) functions.
  - `PreparedSpace::prepare` compiles device, CIE (`CalGray`, `CalRGB`, `Lab`), `Indexed`, `Separation`, and `DeviceN` spaces to sRGB under either `Rules::Iso` (including `/DefaultGray`, `/DefaultRGB`, `/DefaultCMYK`) or `Rules::Pdfium` (as of chromium/8152), with `PreparedSpace::eval_with_rules` switching policies without re-preparing.
  - Enabling the optional `icc` Cargo feature adds pure-Rust `ICCBased` and `/OutputIntents` profile conversion via `moxcms` (falling back to `/Alternate` or the `/N` device space and reporting `Fault::IccProfile` on malformed profiles; when `icc` is disabled, `ICCBased` spaces evaluate through their `/Alternate` or `/N` device fallback with no fault).
- **Resource bounds & error handling (`Limits`):** `Limits` caps color space nesting depth, per-space object visits, decoded stream bytes, unpacked sample counts, and PostScript calculator steps/stack depth. Corrupt objects and streams are treated as missing/faulted, while reader caps, locked files, or I/O errors propagate as `pdf_objects::Unreadable`.

## Quickstart

Count components across a `/Resources` dictionary:

```rust
use pdf_color::pdf_objects::{Object, ObjectId, dict, resolve_fn};
use pdf_color::{Limits, Spaces, components};

let resources = Object::from(dict! {
    "ColorSpace" => dict! {
        "Spot" => vec![Object::name("Separation"), Object::name("Gold"), Object::name("DeviceCMYK")],
        "Plain" => Object::name("DeviceRGB"),
    },
});
let nothing = resolve_fn(|_: ObjectId| None);
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
use pdf_color::pdf_objects::{Object, ObjectId, resolve_fn};
use pdf_color::{Colorant, Family, Fault, Limits, Space};

let spot = Object::from(vec![
    Object::name("Separation"),
    Object::name("Gold"),
    Object::name("DeviceCMYK"),
]);
let nothing = resolve_fn(|_: ObjectId| None);
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
