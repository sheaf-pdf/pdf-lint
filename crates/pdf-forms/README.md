# pdf-forms

PDF interactive forms (ISO 32000-2, §12.7): the field tree with what each field inherits, its typed value, and its widgets, read under ISO 32000-2's rules or PDFium's as of chromium/8152; and the appearance a text field's widget without one shows, made as PDFium makes it.

`pdf-forms` operates over borrowed [pdf-objects](https://crates.io/crates/pdf-objects) `Dict` values and any `Resolve` provider without performing file I/O. It re-exports [pdf-objects](https://crates.io/crates/pdf-objects), [pdf-content-streams](https://crates.io/crates/pdf-content-streams), [pdf-semantics](https://crates.io/crates/pdf-semantics), [pdf-fonts](https://crates.io/crates/pdf-fonts), [pdf-gfx](https://crates.io/crates/pdf-gfx), and [pdf-annots](https://crates.io/crates/pdf-annots) at the crate root.

## Core Concepts

- **The form (`Form`):**
  - `Form::read` takes a catalog and gives its `/AcroForm` with the field tree read; `Form::from_dict` starts from the `/AcroForm` dictionary. The form's `/DA`, `/Q`, `/NeedAppearances`, `/SigFlags`, and whether it holds `/XFA` are read, and its default resources' fonts (`/DR /Font`) are looked up by the name a `/DA` string gives (`Form::font`, `Form::font_names`).
  - Fields are one list, each before its kids (`Form::fields`), with the top of the tree in `Form::roots` and the terminal fields in `Form::terminals`. `Form::full_name` joins partial names with periods, and `Form::find` looks a field up by that name.
- **Fields (`Field`, `FieldType`, `FieldKind`, `FieldFlags`, `FieldValue`):**
  - A field's type (`/FT`), flags (`/Ff`), value (`/V`), default value (`/DV`), default appearance (`/DA`), quadding (`/Q`), and maximum length (`/MaxLen`) are the effective ones: its own, else its nearest ancestor's, and for `/DA` and `/Q` the form's after that; under `Rules::Pdfium`, a `/MaxLen` neither gives is the first of the field's widgets' to hold one, as PDFium reads it. `Field::dict` has what the field itself holds.
  - `Field::kind` tells a push button, check box, radio button set, text field, combo box, list box, and signature field apart by the type and flags.
  - A value is typed as it is written: a text string, a name (a button's state), an array of text strings (a list box's selection), a dictionary (a signature), or a stream.
- **Widgets (`Widget`):**
  - A kid with no partial name and no kids of its own is a widget of its parent; a terminal field with a `/Subtype /Widget` or a `/Rect` is its own widget (`Widget::is_merged`). `Widget::annot` reads either as a `pdf_annots::Annot`: its rectangle, flags, border, `/MK`, and appearances.
- **A field's font (`appearance::FieldFont`, with `text-layout`):**
  - `FieldFont::find` (or `Form::field_font`, over the form's own `/DR /Font`, or `FieldFont::find_in_catalog`, over a catalog's, with no field read) reads the default resources' font a `/DA` string names with `pdf-fonts`: its codes by its encoding (WinAnsiEncoding, MacRomanEncoding, a built-in encoding with `/Differences`), its widths by `/Widths` or, for the standard 14, Adobe's metrics. A name the resources lack falls back to a standard 14 font by that name (`Arial` and `Helv` to Helvetica, `TiRo` to Times-Roman, `HelveticaLTStd-Bold` to Helvetica-Bold), read from the dictionary it will be written as, and is told as `Fault::FontNotInResources`.
  - `FieldFont::layout` gives `pdf-gfx`'s text layout its codes, widths, and heights, as PDFium as of chromium/8152 measures a field's lines: a descriptor's `/Ascent` and `/Descent` as given where it has a `/FontBBox`, past the box or not, and for a standard 14 font with no program and no heights of its own, those of the face PDFium draws it in. `FieldFont::res` is the resource an appearance names it by: the font's object, or its dictionary where that holds no reference; where it holds one, `FieldFont::direct` lends the dictionary for the caller to write as an object and name.
  - A font is read once and shared across widgets and threads.
- **A text field's appearance (`appearance::text`, with `text-layout`):**
  - `appearance::text` makes the appearance a text widget with no `/AP` shows, as PDFium as of chromium/8152 makes it, from plain data: a `TextField` (the text, flags, `/MaxLen`, `/Q`), a `WidgetLook` (the box, `/MK`'s background, border color and rotation, the border), a parsed `/DA`, and a font. It gives an `Appearance` (`/BBox`, `/Matrix`, the content uncompressed, and the font it needs, named by what it is), or why none could be made (`Unmade`: a character the font can't draw, at its offset in the text; no box; a number that isn't finite, or a size or box that sets the text too far out to be placed; a cap), never a `?` drawn. The font may be a `pdf_gfx::FontSet` of several, each part of the text drawn in the first that has it.
  - The text is cut into extended grapheme clusters (a character and the marks that go with it, by [unicode-segmentation](https://crates.io/crates/unicode-segmentation)), each drawn in one font and composed where the font has it so ([unicode-normalization](https://crates.io/crates/unicode-normalization)'s Normalization Form C), else as given: `e` and a combining acute accent draw as `é`, and text already composed draws as it is. A comb cell holds a cluster; a password's stars are one a character as given.
  - The background and border, then `/Tx BMC`, the clip, the color, and the text: a single line set at the box's middle by the font's heights; wrapped lines from the top, words found as PDFium finds them; comb cells from `/MaxLen` in each quadding, divided by a solid or dashed border's color; a password as `*`; an automatic size (`0 Tf`) from PDFium's sizes by its search. The five border styles as PDFium draws them (a beveled or inset border's bands with no border color too, a dashed one from its bottom left corner round); `/MK /R` at each quarter turn, through the box and the matrix.
  - `appearance::widget_text` reads a `Widget` of a text field into that data (the widget's own `/DA` and `/Q`, else the field's; a text stream's value), and `WidgetText::make` draws it in a font the caller keeps; `appearance::text_for_widget` reads the font too, for one widget.
- **A fallback font (`appearance::text_with_fallback`, with `embed` and `text-layout`):**
  - `appearance::text_with_fallback` draws a value in the field's font and a TrueType program the caller opens once (`appearance::Fallback`, over `pdf_fonts::embed::Program`), each cluster in the first that has it: "Łukasz Nguyễn Ölçer" in a WinAnsi Helvetica draws `Ł` and `ễ` in the program. A cluster neither has, a CJK ideograph or an emoji against a Latin program, is refused as `appearance::text` refuses it; nothing is drawn as `.notdef`.
  - The program's codes are each two bytes, its glyph's number in the whole program, so what's drawn doesn't depend on what else the caller draws. The glyphs drawn in it are returned with their text (`appearance::MadeWithFallback::glyphs`), gathered from what's drawn alone: the caller joins every appearance's (`Glyphs::merge`) and writes one font of them (`pdf_fonts::embed::write`, or its own subset behind a `/CIDToGIDMap` stream). A value the field's font draws whole draws as `appearance::text` draws it, byte for byte, with no glyph and no second font named. `WidgetText::make_with_fallback` does the same for a widget read. The program records nothing itself, so the glyphs returned are the only record of what the font must have; a fallback that records what it draws (`pdf_gfx::ProgramFont`, or a font a `pdfs` document embeds) is drawn in by `appearance::text` instead, after the field's font in a `pdf_gfx::WithFallback`, the same content.
  - The program draws in the appearance alone: it isn't the field's `/DA` font, and a viewer that makes an appearance anew draws the value in that font whatever the default resources hold, so nothing asks for it there.
- **A font in the default resources (`default_resources::font_name`):**
  - `default_resources::font_name` gives the name a writer puts a font object under in the form's `/DR /Font`, for a `/DA` string to name it by (ISO 32000-2, §12.7.4.3): the name wanted where it's free, `null`, or that font's already, else the first number after it that is (`Helv1` beside another font's `Helv`), never another font's. It reads the dictionary and changes nothing: the caller sets the entry, and names it in the `/DA` it writes. [pdfs](https://crates.io/crates/pdfs)' `Document::add_annot` does both for an annotation built in a font of its own.
- **Bounded, and quiet about content (`Limits`, `Fault`, `Error`):**
  - The walk ends within `Limits` whatever the file says: a field reached twice (a cycle through `/Kids`) is left out, a tree deeper than `Limits::depth` is cut there, and more fields and widgets than `Limits::fields` is an error. A field's font is read under `Limits::fonts`, its streams within `Limits::font_budget`; an appearance's text is within `Limits::text`, and a comb field's cells within `Limits::comb_cells`, so an empty field's dividers are bounded too.
  - Recoverable departures go to a `pdf_objects::fault::Sink` as `Fault`s; `&mut ()` collects none and reads the same form. `Debug` output of every type prints numbers and counts, never a field's name or value.

## Where it reads more, or less, than PDFium

Under `Rules::Pdfium` text strings, names, and numbers are read as PDFium reads them as of chromium/8152, a button with both the push button and the radio flags is a radio button, and a field with no `/MaxLen` of its own or an ancestor's takes the first of its widgets' own (the first widget whose dictionary has the entry), where ISO 32000-2 gives the entry to fields alone. The tree itself is read as ISO 32000-2 gives it, which differs from PDFium in two ways, each held by a recorded case:

- **More fields.** A field's type is inherited from any ancestor (PDFium makes a terminal field only where `/FT` is its own or its parent's), a terminal field with no type or no name is still read, with a `Fault`, and a field is found by walking down from `/Fields` whether or not its widget's `/Parent` leads back up.
- **No widgets from pages.** PDFium adds a page's widget that has a `/FT` to the form though `/Fields` doesn't reach it, and makes a form of such widgets where the catalog has no `/AcroForm`. `Form::read` reads the tree `/Fields` gives.

A text field's appearance is laid out as PDFium lays it out as of chromium/8152: the sizes an automatic size is chosen from and how they're searched, the heights of the faces it draws the standard 14 in, each border style's inset and how it's drawn, where a line breaks, comb cells, and turns by `/MK /R`. Those rules were learnt from PDFium's sources (BSD-3-Clause), whose files are named where each rule is kept, and are held to the appearances PDFium makes for the same fields, recorded as cases; no PDFium code is copied or translated. Where an appearance differs from PDFium's, ISO 32000-2 or Acrobat's way is kept, each held by a recorded case:

- **The clip is always written,** the box less the border, as Acrobat writes it; PDFium writes it only where the text passes that box. **An empty value is an empty `/Tx BMC EMC`,** where PDFium writes none.
- **`/MaxLen` cuts only a comb field's text,** since what's drawn is the display text the caller gives, which may be longer than the value `/MaxLen` limits; PDFium cuts every text field's text. The Comb flag with no `/MaxLen` makes no cells and the line is one string, where PDFium writes each character as a string of its own at the same place.
- **A font the default resources lack falls back by its name** to the standard 14 family and face it gives (`TiRo` to Times-Roman, `Cour` to Courier, `HelveticaLTStd-Bold` to Helvetica-Bold), whose widths and heights are nearer; PDFium draws every such font in a Helvetica of its own. A character the field's font has no code for is refused (`Unmade::Unencodable`), or drawn in the fallback the caller gives, where PDFium draws it in a font it adds from the machine's or writes it as another glyph's code, and a box with no height is refused (`Unmade::NoBox`), where PDFium writes a string in no font.
- **Text given decomposed is drawn composed** (`e` and a combining acute accent as `é`), where PDFium draws the letter and leaves out an accent its fonts don't know; and **a comb cell holds a grapheme cluster,** a letter and its accents together, where PDFium gives each character a cell of its own.
- **The border is read as ISO 32000-2 gives it:** from the border style dictionary over `/Border`, at the width `/W` gives, a fraction kept, and a dashed border and a comb field's dividers with the dash array given. PDFium reads `/Border` first, cuts the width to an integer, and dashes every dashed border `[3 3]`. A solid border wider than half its box covers it, where PDFium's ring leaves a hole.
- **A negative `/MK /R` turns clockwise,** as `/R` is counterclockwise (`-90` is three quarter turns); PDFium reads the rotation's size alone (`-90` as `90`).
- **A beveled border's dark band over a CMYK background is darker than it** (cyan, magenta and yellow halved, black moved halfway to full); PDFium halves every component, which draws the band lighter.
- **A text field's value is a text string or a text stream** (ISO 32000-2, §12.7.5.3); an array is no text, where PDFium draws its first element. A field whose type comes from its grandparent gets its appearance, as the tree above has it inherited.
- **Of two `Tf`, or two nonstroking colors, in a `/DA` the last is used,** as a content stream's graphics state has it; PDFium takes the first `Tf`, and the first `g`, else the first `rg`, else the first `k`. A stroking color (`G`, `RG`, `K`) never colors the text, here or in PDFium.
- **A `/V` that is null, or refers to a null object, is an absent one,** and the parent's value is inherited, as for a reference to no object; PDFium inherits only for that last.

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

Make the appearance of a text field's widget that has none, and bind what it names (`text-layout`):

```rust
use std::collections::BTreeMap;

use pdf_forms::appearance::text_for_widget;
use pdf_forms::pdf_content_streams::ResBody;
use pdf_forms::pdf_fonts::Std14;
use pdf_forms::pdf_gfx::Matrix;
use pdf_forms::pdf_objects::pdf_tokenizer::Rules;
use pdf_forms::pdf_objects::{Object, ObjectId, Unreadable, Why, WithStreams, dict};
use pdf_forms::{Form, Limits};

let (name, helv) = (ObjectId::new(2, 0), ObjectId::new(3, 0));
let mut objects = BTreeMap::new();
// A text field that is its own widget, with a value and no `/AP`.
objects.insert(
    name,
    Object::from(dict! {
        "T" => Object::text("name"),
        "FT" => Object::name("Tx"),
        "V" => Object::text("Ada"),
        "DA" => Object::text("/Helv 12 Tf 0 g"),
        "Subtype" => Object::name("Widget"),
        "Rect" => vec![Object::from(72), Object::from(700), Object::from(172), Object::from(720)],
    }),
);
objects.insert(helv, Object::from(Std14::Helvetica.dict()));
let catalog = dict! {
    "AcroForm" => dict! {
        "Fields" => vec![Object::from(name)],
        "DR" => dict! { "Font" => dict! { "Helv" => helv } },
    },
};
// A font's streams are read through the same source; these objects have none.
let source = WithStreams::new(&objects, |id: ObjectId, _| -> Result<Vec<u8>, Unreadable> {
    Err(Unreadable::new(id, Why::Damaged, "no streams".to_string()))
});

let limits = Limits::DEFAULT;
let form = Form::read(&source, &catalog, Rules::Iso, &limits).unwrap().unwrap();
let field = &form.fields()[form.find("name").unwrap()];
let widget = &field.widgets()[0];
let made = text_for_widget(&form, field, widget, &source, Rules::Iso, None, &limits, &mut ()).unwrap();

// The form XObject to write as the widget's `/AP /N`: its `/BBox`, its
// `/Matrix`, its content, and in its `/Resources` each resource the content
// names, here the default resources' own font, by its object number.
let appearance = &made.appearance;
assert_eq!((appearance.bbox.width(), appearance.bbox.height()), (100.0, 20.0));
assert_eq!(appearance.matrix, Matrix::IDENTITY);
let mut fonts = dict! {};
for need in &appearance.needs.0 {
    match need.res.body() {
        ResBody::Written(id) => fonts.set(need.name.clone(), *id),
        // A fallback font's dictionary, which the caller writes as an object.
        other => panic!("not an object of the file: {other:?}"),
    }
}
assert_eq!(fonts.len(), 1);
assert!(appearance.content.ends_with(b"(Ada) Tj\nET\nQ\nEMC\n"));
// The font was found in `/DR` under the name `/DA` gives, so `/DR` is whole.
assert!(!made.font.is_fallback());
assert_eq!(made.font_name.as_deref(), Some(&b"Helv"[..]));
```

## Buttons and choices

- **What the read layer gives of them.** `Field::options` decodes `/Opt`: a choice field's options (`FieldOption`, an export value and the label shown for it) or a button field's export values, one for each widget. `Field::top_index` is `/TI`, `Field::selected` is `/I`, and `Field::selection` the options the field's value selects, as `selection` finds them over plain data: `/I` where it agrees with `/V`, else the options whose export values `/V` gives, as PDFium as of chromium/8152 finds them. `/Opt`, `/TI`, and `/I` are each the field's own or the nearest ancestor's under either rules, found with the field and decoded only when they're asked for, within `Limits::items`, and an `/Opt` or `/I` that can't be read fails the call that asks for it, not `Form::read`. `Widget::states` reads a widget's on state, its `/AS`, and whether it has an `Off` appearance from `/AP` and `/AS` alone.
- **A group's rules (`group`).** `Group` and `GroupWidget` hold a check box or radio button field's widgets as plain data, filled by a caller from a model of its own or read by `Field::group`. `Group::press`, `Group::states_for`, and `Group::states_for_export` say which widgets are on after a press, for a state's name, or for an export value; `Group::value` and `Group::state_name` what the field's `/V` and each widget's `/AS` become; `on_state_name` names the state a made appearance goes under. They follow PDFium as of chromium/8152 where ISO 32000-2 says less: a check box field's widgets are always in unison, and in unison a widget goes with those of its on state and export value. Where PDFium departs from ISO 32000-2, the standard's rules are kept, each held by a recorded case:
  - **A radio button pressed while on turns off** where the field hasn't NoToggleToOff (ISO 32000-2, §12.7.5.2.4); PDFium never turns a radio button off by a press, reading the flag nowhere.
  - **The value is the name of the on widget's state, `Off` with none,** as `/V` and `/AS` are the same (ISO 32000-2, §12.7.5.2.3); with `/Opt`, PDFium writes the pressed widget's index whatever its state's name, and leaves `/V` as it was when a widget is turned off.

Press a radio button and set what its group's widgets and field become:

```rust
use std::collections::BTreeMap;

use pdf_forms::pdf_objects::pdf_tokenizer::Rules;
use pdf_forms::pdf_objects::{Object, ObjectId, Stream, dict};
use pdf_forms::{Form, Limits};

let (answer, on) = (ObjectId::new(2, 0), ObjectId::new(3, 0));
let (yes, no) = (ObjectId::new(4, 0), ObjectId::new(5, 0));
let mut objects = BTreeMap::new();
objects.insert(on, Object::from(Stream::new(dict! { "Subtype" => Object::name("Form") }, Vec::new())));
// A radio button field of two widgets, the first on.
objects.insert(
    answer,
    Object::from(dict! {
        "T" => Object::text("answer"),
        "FT" => Object::name("Btn"),
        "Ff" => 1 << 15,
        "V" => Object::name("yes"),
        "Kids" => vec![Object::from(yes), Object::from(no)],
    }),
);
for (id, name, state) in [(yes, "yes", "yes"), (no, "no", "Off")] {
    objects.insert(
        id,
        Object::from(dict! {
            "Parent" => answer,
            "Subtype" => Object::name("Widget"),
            "Rect" => vec![Object::from(72), Object::from(700), Object::from(84), Object::from(712)],
            "AS" => Object::name(state),
            "AP" => dict! { "N" => dict! { name => on } },
        }),
    );
}
let catalog = dict! { "AcroForm" => dict! { "Fields" => vec![Object::from(answer)] } };

let limits = Limits::DEFAULT;
let form = Form::read(&objects, &catalog, Rules::Iso, &limits).unwrap().unwrap();
let field = &form.fields()[form.find("answer").unwrap()];
let read = field.group(&objects, Rules::Iso, &limits).unwrap();
assert_eq!(read.states(), [true, false]);

// The second pressed: the first's `/AS` becomes `Off`, the second's and the
// field's `/V` its on state.
let group = read.group();
let states = group.press(read.states(), 1);
assert_eq!(states, [false, true]);
assert_eq!(&*group.state_name(0, states[0]), b"Off");
assert_eq!(&*group.state_name(1, states[1]), b"no");
assert_eq!(&*group.value(&states), b"no");
```

## Features

- `programs` (default): forwards `programs` to `pdf-annots`, `pdf-gfx`, and `pdf-fonts`, for embedded font program metrics.
- `text-layout` (default): forwards `text-layout` to `pdf-annots` and `pdf-gfx`, and adds the `appearance` module, with [unicode-normalization](https://crates.io/crates/unicode-normalization) and [unicode-segmentation](https://crates.io/crates/unicode-segmentation).
- `embed`: forwards `embed` to `pdf-fonts` (which brings in [subsetter](https://crates.io/crates/subsetter)) and `pdf-gfx`; with `text-layout`, adds `appearance::text_with_fallback`. It doesn't turn `text-layout` on.
- `predefined-cmaps`: forwards `predefined-cmaps` to `pdf-annots`, `pdf-gfx`, and `pdf-fonts`.
- `tracing`: `debug`-level spans with numeric counts only.

## Minimum supported Rust version

Rust 1.92.

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE) or [MIT license](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT) at your option.
