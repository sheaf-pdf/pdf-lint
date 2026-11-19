# pdf-forms

PDF interactive forms (ISO 32000-2, §12.7): the field tree with what each field inherits, its typed value, and its widgets, read under ISO 32000-2's rules or PDFium's as of chromium/8152.

`pdf-forms` operates over borrowed [pdf-objects](https://crates.io/crates/pdf-objects) `Dict` values and any `Resolve` provider without performing file I/O. It re-exports [pdf-objects](https://crates.io/crates/pdf-objects), [pdf-content-streams](https://crates.io/crates/pdf-content-streams), [pdf-semantics](https://crates.io/crates/pdf-semantics), [pdf-fonts](https://crates.io/crates/pdf-fonts), [pdf-gfx](https://crates.io/crates/pdf-gfx), and [pdf-annots](https://crates.io/crates/pdf-annots) at the crate root.

## Core Concepts

- **The form (`Form`):**
  - `Form::read` takes a catalog and gives its `/AcroForm` with the field tree read; `Form::from_dict` starts from the `/AcroForm` dictionary. The form's `/DA`, `/Q`, `/NeedAppearances`, `/SigFlags`, and whether it holds `/XFA` are read, and its default resources' fonts (`/DR /Font`) are looked up by the name a `/DA` string gives (`Form::font`, `Form::font_names`).
  - Fields are one list, each before its kids (`Form::fields`), with the top of the tree in `Form::roots` and the terminal fields in `Form::terminals`. `Form::full_name` joins partial names with periods, and `Form::find` looks a field up by that name.
- **Fields (`Field`, `FieldType`, `FieldKind`, `FieldFlags`, `FieldValue`):**
  - A field's type (`/FT`), flags (`/Ff`), value (`/V`), default value (`/DV`), default appearance (`/DA`), quadding (`/Q`), and maximum length (`/MaxLen`) are the effective ones: its own, else its nearest ancestor's, and for `/DA` and `/Q` the form's after that. `Field::dict` has what the field itself holds.
  - `Field::kind` tells a push button, check box, radio button set, text field, combo box, list box, and signature field apart by the type and flags.
  - A value is typed as it is written: a text string, a name (a button's state), an array of text strings (a list box's selection), a dictionary (a signature), or a stream.
- **Widgets (`Widget`):**
  - A kid with no partial name and no kids of its own is a widget of its parent; a terminal field with a `/Subtype /Widget` or a `/Rect` is its own widget (`Widget::is_merged`). `Widget::annot` reads either as a `pdf_annots::Annot`: its rectangle, flags, border, `/MK`, and appearances.
- **Bounded, and quiet about content (`Limits`, `Fault`, `Error`):**
  - The walk ends within `Limits` whatever the file says: a field reached twice (a cycle through `/Kids`) is left out, a tree deeper than `Limits::depth` is cut there, and more fields and widgets than `Limits::fields` is an error.
  - Recoverable departures go to a `pdf_objects::fault::Sink` as `Fault`s; `&mut ()` collects none and reads the same form. `Debug` output of every type prints numbers and counts, never a field's name or value.

## Quickstart

Read a form whose text field inherits its type and default appearance from its parent:

```rust
use std::collections::BTreeMap;

use pdf_forms::pdf_objects::pdf_tokenizer::Rules;
use pdf_forms::pdf_objects::{Object, ObjectId, dict};
use pdf_forms::{FieldFlags, FieldKind, FieldValue, Form, Limits, Quadding};

let (address, city, agree) = (ObjectId::new(2, 0), ObjectId::new(3, 0), ObjectId::new(4, 0));

let mut objects = BTreeMap::new();
// A field with no widget of its own: it hands its type and `/DA` down.
objects.insert(
    address,
    Object::from(dict! {
        "T" => Object::text("address"),
        "FT" => Object::name("Tx"),
        "DA" => Object::text("/Helv 9 Tf 0 0 0.502 rg"),
        "Kids" => vec![Object::from(city)],
    }),
);
// A terminal field that is its own widget.
objects.insert(
    city,
    Object::from(dict! {
        "T" => Object::text("city"),
        "Parent" => address,
        "V" => Object::text("Trenton"),
        "Q" => 2,
        "Subtype" => Object::name("Widget"),
        "Rect" => vec![Object::from(72), Object::from(700), Object::from(200), Object::from(716)],
    }),
);
objects.insert(
    agree,
    Object::from(dict! {
        "T" => Object::text("agree"),
        "FT" => Object::name("Btn"),
        "V" => Object::name("Yes"),
        "Subtype" => Object::name("Widget"),
        "Rect" => vec![Object::from(72), Object::from(680), Object::from(82), Object::from(690)],
    }),
);
let catalog = dict! {
    "Type" => Object::name("Catalog"),
    "AcroForm" => dict! {
        "Fields" => vec![Object::from(address), Object::from(agree)],
        "DA" => Object::text("/Helv 0 Tf 0 g"),
    },
};

let form = Form::read(&objects, &catalog, Rules::Pdfium, &Limits::DEFAULT).unwrap().unwrap();
assert_eq!(form.fields().len(), 3);

let city = &form.fields()[form.find("address.city").unwrap()];
assert_eq!(city.kind(), Some(FieldKind::Text));
assert_eq!(city.value(), Some(&FieldValue::Text("Trenton".into())));
assert_eq!(city.default_appearance(), Some(&b"/Helv 9 Tf 0 0 0.502 rg"[..]));
assert_eq!(city.quadding(), Quadding::Right);
assert!(!city.flags().contains(FieldFlags::MULTILINE));

// The widget is the field's own dictionary, read as an annotation.
let widget = city.widgets()[0];
assert!(widget.is_merged());
let annot = widget.annot(&objects, Rules::Pdfium, &Limits::DEFAULT).unwrap();
assert_eq!(annot.rect().x1, 200.0);

let agree = &form.fields()[form.find("agree").unwrap()];
assert_eq!(agree.kind(), Some(FieldKind::CheckBox));
assert_eq!(agree.value(), Some(&FieldValue::State(b"Yes")));
assert_eq!(agree.default_appearance(), Some(&b"/Helv 0 Tf 0 g"[..]));
```

## Features

- `programs` (default): forwards `programs` to `pdf-annots`, `pdf-gfx`, and `pdf-fonts`, for embedded font program metrics.
- `text-layout` (default): forwards `text-layout` to `pdf-annots` and `pdf-gfx`.
- `predefined-cmaps`: forwards `predefined-cmaps` to `pdf-annots`, `pdf-gfx`, and `pdf-fonts`.
- `tracing`: `debug`-level spans with numeric counts only.

## Minimum supported Rust version

Rust 1.92.

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE) or [MIT license](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT) at your option.
