# pdf-annots

PDF annotations, appearance placement, destinations, and actions (ISO 32000-2, §12.3.2, §12.5, §12.6), matching both ISO 32000-2 rules and PDFium's annotation and link resolution rules as of chromium/8152.

`pdf-annots` operates over borrowed [pdf-objects](https://crates.io/crates/pdf-objects) `Dict` values and any `Resolve` provider without performing file I/O. It re-exports [pdf-objects](https://crates.io/crates/pdf-objects), [pdf-content-streams](https://crates.io/crates/pdf-content-streams), [pdf-semantics](https://crates.io/crates/pdf-semantics), [pdf-fonts](https://crates.io/crates/pdf-fonts), and [pdf-gfx](https://crates.io/crates/pdf-gfx) at the crate root.

## Core Concepts

- **Page `/Annots` & `&Dict` inspection (`Annot`, `page_annots`, `page_annots_with`, `page_tabs`):**
  - Walks page `/Annots` arrays under `Rules::Iso` or `Rules::Pdfium` as of chromium/8152, decoding `/Subtype` (`AnnotSubtype`), `/Rect`, `AnnotFlags`, `AnnotColor` (`/C`, `/IC`), `Border` (`/BS`, legacy `/Border`, `/BE`), `AppearanceChars` (`/MK`), `DefaultAppearance` (`/DA`, `pdf-gfx`'s shared parser and serializer, re-exported), and `MarkupInfo` (`/T`, `/Subj`, `/CA`, `/CreationDate`, `/IRT`, `/Popup`).
  - `Annot::from_dict` and `Annot::from_dict_with` inspect any `&Dict` directly, including merged Field+Widget dictionaries.
- **Typed subtype payloads (`TypedAnnot`):**
  - Structured views for `Text` (`TextAnnot`), `Link` (`LinkAnnot`), `FreeText` (`FreeTextAnnot`), text markups (`TextMarkupAnnot`, `QuadPoints`), `Line` (`LineAnnot`), `Square`/`Circle` (`ShapeAnnot`), `Polygon`/`PolyLine` (`PolyAnnot`), `Ink` (`InkAnnot`), `Stamp` (`StampAnnot`), `Caret` (`CaretAnnot`), `Popup` (`PopupAnnot`), `FileAttachment` (`FileAttachmentAnnot`), `Redact` (`RedactAnnot`), and `Widget` (`WidgetAnnot`).
- **Appearances & §12.5.5 placement (`Appearances`, `placement_matrix`, `stream_placement_matrix`):**
  - Resolves single-stream and state-subdictionary `/AP` (`/N`, `/R`, `/D`) entries against `/AS`, and computes the exact `BBox * FormMatrix -> Rect` placement `Matrix`.
- **Destinations, actions & reply threads (`Dest`, `DestTable`, `Action`, `resolve_threads`):**
  - Re-exports `pdf_semantics::dest` and `pdf_semantics::action` for explicit/named destinations across Catalog `/Names/Dests` and `/Dests` and bounded `/A`, `/Next`, `/AA`, and `/OpenAction` chains, and resolves `/IRT` reply/group hierarchies with effective `MarkedState` and `ReviewState` tracking.
- **Appearance synthesis, builders & zero-I/O flattening (`SynthesizedAp`, `BuiltAnnot`, `AnnotMut`, `plan_flatten`):**
  - Deterministically synthesizes `/AP /N` (and `/Redact` `/RO`) Form XObjects over `pdf_gfx::Canvas` (FreeText and plain `/Square` borders in all five `/BS` styles: solid, dashed, beveled, inset, underline), constructs or mutates annotation dictionaries without file I/O, and plans graphics-state-isolated page flattening with collision-free `/Resources/XObject` names.
  - Draws a `/Caret`'s triangle, a `/FileAttachment`'s icon, and cloudy borders as scallops (`synth_caret`, `synth_file_attachment`, `synth_cloud`, and `synth_shape` and `synth_poly` under `/BE /S /C`), their shapes public (`caret_outline`, `attachment_icon_strokes`, `cloud_outline`, `cloud_step`) so a viewer that paints a mark itself paints the same numbers; and draws in place on request (`ApRect`), the form's `/BBox` the annotation's `/Rect`.
  - Authors `/Redact` marks whole (`RedactMark`, `synth_redact`, `BuiltAnnot::redact_mark`, `BuiltAnnot::redact_quads`): the mark and its `/RO` overlay drawn over each `/QuadPoints` quadrilateral, the overlay also bound as `/AP /R`, with `AnnotMut` setters for `/RO`, `/OverlayText`, and `/Repeat`.
  - Draws FreeText and redaction overlay text in Helvetica by default, or in any `pdf_gfx::LayoutFont` given (`text-layout` feature: `synth_free_text_with_font`, `synth_redact_with_font`, `BuiltAnnot::free_text_with_font`, `BuiltAnnot::redact_with_font`), such as a font embedded in the document for text beyond Helvetica's characters; a character the font can't draw is an error, with nothing built. The builders keep the font as `BuiltAnnot::da_font`, for the document's form to hold under the name `/DA` gives (`/AcroForm /DR /Font`), so a viewer that makes the appearance again from `/DA` draws in it too: [pdfs](https://crates.io/crates/pdfs)' `Document::add_annot` puts it there and renames it in `/DA` where that name is another font's, and `BuiltAnnot::write_into`, which reaches no catalog, leaves `/DA` as given.
  - Flattening keeps `/Redact` marks on the page, undrawn, unless `FlattenOpts::with_redactions(true)` asks for them: it never removes the content under a mark, so a mark painted onto the page would only look like a redaction.

## Quickstart

Inspect a page's `/Link` and `/Highlight` annotations and resolve a named destination to its 0-based target page index:

```rust
use std::collections::BTreeMap;

use pdf_annots::pdf_objects::pdf_tokenizer::Rules;
use pdf_annots::pdf_objects::{Object, ObjectId, dict};
use pdf_annots::pdf_semantics::pages;
use pdf_annots::{
    AnnotFlags, DestFit, DestTable, Limits, TextMarkupKind, TypedAnnot, page_annots,
};

let page1_id = ObjectId::new(3, 0);
let page2_id = ObjectId::new(4, 0);
let link_id = ObjectId::new(5, 0);
let mark_id = ObjectId::new(6, 0);

let mut objects = BTreeMap::new();
objects.insert(
    ObjectId::new(2, 0),
    Object::from(dict! {
        "Type" => Object::name("Pages"),
        "Kids" => vec![Object::from(page1_id), Object::from(page2_id)],
        "Count" => 2,
        "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(612), Object::from(792)],
    }),
);
let page1 = dict! {
    "Type" => Object::name("Page"),
    "Parent" => ObjectId::new(2, 0),
    "Annots" => vec![Object::from(link_id), Object::from(mark_id)],
};
objects.insert(page1_id, Object::from(page1.clone()));
objects.insert(
    page2_id,
    Object::from(dict! { "Type" => Object::name("Page"), "Parent" => ObjectId::new(2, 0) }),
);
objects.insert(
    link_id,
    Object::from(dict! {
        "Subtype" => Object::name("Link"),
        "Rect" => vec![Object::from(72), Object::from(700), Object::from(200), Object::from(720)],
        "F" => AnnotFlags::PRINT.bits() as i64,
        "Dest" => Object::text("intro"),
    }),
);
objects.insert(
    mark_id,
    Object::from(dict! {
        "Subtype" => Object::name("Highlight"),
        "Rect" => vec![Object::from(72), Object::from(650), Object::from(180), Object::from(664)],
        "QuadPoints" => vec![
            Object::from(72), Object::from(664), Object::from(180), Object::from(664),
            Object::from(72), Object::from(650), Object::from(180), Object::from(650),
        ],
        "T" => Object::text("Reviewer"),
    }),
);
let catalog = dict! {
    "Type" => Object::name("Catalog"),
    "Pages" => ObjectId::new(2, 0),
    "Names" => dict! {
        "Dests" => dict! {
            "Names" => vec![
                Object::text("intro"),
                Object::from(vec![Object::from(page2_id), Object::name("FitH"), Object::from(750)]),
            ],
        },
    },
};

let annots = page_annots(&objects, &page1, Some(page1_id), Rules::Pdfium, &Limits::DEFAULT).unwrap();
assert_eq!(annots.len(), 2);
assert!(annots[0].is_printable());

// Resolve the Link's named destination ("intro") to page index 1.
let sem_limits = pdf_annots::pdf_semantics::Limits::DEFAULT;
let table = pages::pages(&objects, &catalog, Rules::Pdfium, &sem_limits, &mut ()).unwrap();
let dests = DestTable::read(&objects, Some(&catalog), &table, Rules::Pdfium, &sem_limits, &mut ()).unwrap();
let TypedAnnot::Link(link) = annots[0].typed() else { unreachable!() };
let resolved = dests.resolve(link.effective_dest().unwrap(), Rules::Pdfium, &mut ()).unwrap();
assert_eq!(resolved.page_index, 1);
assert_eq!(resolved.fit, DestFit::FitH { top: Some(750.0) });

// Inspect the Highlight annotation's QuadPoints and author.
let TypedAnnot::TextMarkup(hl) = annots[1].typed() else { unreachable!() };
assert_eq!(hl.kind, TextMarkupKind::Highlight);
assert_eq!(hl.quad_points.len(), 1);
assert_eq!(annots[1].markup().unwrap().author.as_deref(), Some("Reviewer"));
```

## Cargo Features

- **`programs`** *(default)*: Forwards `pdf-gfx/programs` and `pdf-fonts/programs`.
- **`text-layout`** *(default)*: Forwards `pdf-gfx/text-layout` for multiline appearance layout: FreeText and redaction overlay text wrapped at words, a FreeText whose `/DA` says `0 Tf` sized to its box, and the `*_with_font` synthesizers and builders that draw that text in a `LayoutFont`.
- **`predefined-cmaps`** *(optional)*: Forwards `pdf-gfx/predefined-cmaps` and `pdf-fonts/predefined-cmaps`.
- **`tracing`** *(optional)*: Emits numeric [tracing](https://crates.io/crates/tracing) spans around page annotation walks and destination table construction.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
