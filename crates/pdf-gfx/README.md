# pdf-gfx

PDF 2D geometry, bounded graphics and text state interpreter, content re-encoder, and vector/text canvas (ISO 32000-2, §8–§9, §11, §14.6), matching PDFium's content interpretation rules as of chromium/8152.

`pdf-gfx` operates over decoded content streams (`Concat`) and any [pdf-objects](https://crates.io/crates/pdf-objects) `Resolve` + `Streams` provider without performing file I/O, image pixel decoding, or rasterization. It re-exports [pdf-objects](https://crates.io/crates/pdf-objects), [pdf-color](https://crates.io/crates/pdf-color), [pdf-content-streams](https://crates.io/crates/pdf-content-streams), [pdf-semantics](https://crates.io/crates/pdf-semantics), and [pdf-fonts](https://crates.io/crates/pdf-fonts) at the crate root.

## Core Concepts

- **2D geometry & page coordinate spaces (`Point`, `Rect`, `Quad`, `Matrix`, `Rotation`, `PageGeom`):**
  - Exact quarter-turn rotations (`Rotation`), affine placement (`Matrix::onto`), and closed-form transforms between PDF PageSpace (`y` up, before `/Rotate`) and viewer ViewSpace (`(0, 0)` at top-left, `y` down, after `/Rotate`) via `PageGeom::to_view` and `PageGeom::from_view`.
  - `PageGeom::from_page` reads `/MediaBox`, `/CropBox`, `/BleedBox`, `/TrimBox`, `/ArtBox`, `/Rotate`, and `/UserUnit` from a `pdf-semantics` `Page`.
- **Bounded content & Form XObject interpreter (`interpret`, `interpret_with`, `Event`, `State`):**
  - Steps through content streams and nested Form XObjects (`Do` under `Forms::Descend` or shallow `Forms::Skip`), tracking the CTM stack (`q`/`Q`/`cm`), line style, active color spaces & patterns (`ActiveColorSpace`, `ColorValues`), `ExtGState` parameters (`gs`), paths & stroked bounds (`PathDrawn`, `Seg`), path/text/form clipping (`Clip`), placed images & effective DPI (`ImageDrawn`), shadings (`ShadingDrawn`), marked content (`Mark`, `MarkProps`, `Marks`), and placed text glyphs (`TextShow`, `Glyph`).
  - `Opts` (`Wants`) skips unselected event construction while keeping `Summary` counts and errors identical; `interpret_with` reports recoverable departures (`Fault`) to any `pdf-objects` `fault::Sink<Fault>` at zero cost when `()` is passed.
- **Position-independent content re-encoding (`encode`, `encode_with`, `EncodedPage`):**
  - Rewrites relative line-moving text operators (`Td`, `TD`, `T*`, `'`, `"`) across a page stream and every Form XObject it draws into standalone `Tm` + `Td` pairs so lines no longer depend on `TL` or earlier lines, while round-tripping every placed glyph, path, clip, and mark bit-for-bit.
- **Vector, XObject & text canvas (`Canvas`, `TextPiece`):**
  - Builds content streams with scoped `q`/`Q` state, inset borders in the five border styles of ISO 32000-2 §12.5.4 (`Canvas::box_border` with `BoxBorder` and `BoxBorderStyle`: solid, dashed, beveled, inset, underline), the bands of a beveled or inset border alone (`Canvas::box_bands`), rounded rectangles, circles/ellipses, placed images/Form XObjects, marked content, and `TJ`-kerned text over `pdf-content-streams`' `ContentWriter` and deferred resource naming (`Res`, `Reserved`, `ResourceNeeds`).
- **Default appearance strings (`DefaultAppearance`):**
  - Parses a `/DA` string (ISO 32000-2, §12.7.4.3) under `pdf-content-streams`' `Limits` into its font resource name and size (`Tf`, with a size of exactly `0` kept as auto-size), device color (`g`/`G`, `rg`/`RG`, `k`/`K`), and text-state scalars (`Tc`, `Tw`, `Tz`, `TL`, `Tr`, `Ts`), and writes it back deterministically with `to_bytes`, so annotations and form fields share one reader and writer. `parse_with` reads the same and tells a `fault::Sink<Fault>` what departs from §12.7.4.3 at its offset in the string: one that fails to lex or passes a cap, an operator ignored for an operand missing or not finite, a `Tr` out of range, no `Tf` naming a font.
- **Positioned text page, search & multiline layout (`text-layout` feature):**
  - `TextPage` (`PageChar`, `TextLine`, `TextPageOpts`, `SearchOpts`, `TextMatch`) assembles placed glyphs into reading-order lines matching PDFium's `CPDF_TextPage` as of chromium/8152: synthetic spaces and line breaks, faux-bold duplicate suppression, `/ActualText` replacement, `unicode-bidi` visual-to-logical RTL reordering, soft-hyphen wrap detection, merged per-line selection quads (`selection_quads`), hit-testing (`char_index_at`), rectangular text extraction (`text_in_rect`), and Unicode search (`search`).
  - `layout_text` and `Canvas::text_box` (`LayoutFont`, `FontEncoder`, `TextBoxStyle`, `TextAlign`, `VerticalAlign`, `WrapMode`) perform word/character wrapping, horizontal/vertical alignment, `Tw` justification, automatic font sizing, and fixed-cell `comb` layout over `Std14`, `Font`, or multi-byte `FontEncoder`. `TextBoxStyle`'s options: the size or auto-size range, the line step or leading, the alignments, the wrapping, padding, comb cells, the clip and the rendering mode; `line_end_spaces`, which keeps the spaces a wrapped line breaks at (in its codes, its range and the width it's aligned by) and finds words as PDFium's line breaking does; and `font_heights_as_given`, which measures lines by the font's own ascent and descent, without the floors layout otherwise gives them.

## Quickstart

Draw a clipped banner and positioned text with `Canvas`, then interpret the resulting content stream and map a glyph box into viewer coordinates:

```rust
use std::collections::BTreeMap;
use std::ops::ControlFlow;

use pdf_gfx::pdf_content_streams::{Color, Concat, Res, Reserved};
use pdf_gfx::pdf_fonts::{Fonts, Std14};
use pdf_gfx::pdf_objects::{Object, ObjectId, Unreadable, Why, WithStreams};
use pdf_gfx::{Canvas, Kind, Limits, Opts, PageGeom, Point, Rect, Rotation, interpret};

// Draw a filled rectangle and a line of Helvetica text onto a Letter page.
let mut canvas = Canvas::new();
let font = Res::dict(Std14::Helvetica.dict()).unwrap();
canvas.with_state(|c| {
    c.clip_rect(Rect::from_xywh(36.0, 700.0, 200.0, 60.0));
    c.fill_color(Color::rgb8([230, 240, 255]));
    c.fill_rect(Rect::from_xywh(36.0, 700.0, 200.0, 60.0));
});
canvas.text_at(font, 12.0, Point::new(48.0, 720.0), b"Hello");
let (bytes, needs) = canvas.finish(&Reserved::default());

// Bind the font resource generated by the canvas into `/Resources`.
let mut font_res = pdf_gfx::pdf_objects::Dict::new();
for need in &needs.0 {
    font_res.set(need.name.clone(), Std14::Helvetica.dict());
}
let mut res_dict = pdf_gfx::pdf_objects::Dict::new();
res_dict.set("Font", font_res);
let resources = Object::from(res_dict);

// Interpret the stream with zero I/O and inspect the placed glyphs.
let objects = BTreeMap::<ObjectId, Object>::new();
let streams = WithStreams::new(&objects, |id, _| {
    Err(Unreadable::new(id, Why::Damaged, "no streams"))
});
let content = Concat::new(&[&bytes]);
let mut glyph_boxes = Vec::new();
let summary = interpret(
    streams,
    &content,
    Some(&resources),
    &Limits::DEFAULT,
    &Fonts::default(),
    Opts::default(),
    |event| {
        if let Kind::Text(show) = event.kind {
            glyph_boxes.extend(show.glyphs().map(|g| g.quad.bounds()));
        }
        ControlFlow::Continue(())
    },
)
.unwrap();
assert_eq!(summary.glyphs, 5);

// Convert the first glyph's bounding box from PageSpace (`y` up) to ViewSpace (`y` down).
let geom = PageGeom::new(Rect::LETTER, None, Rotation::R0);
let in_view = geom.to_view().apply_rect(glyph_boxes[0]);
assert!(in_view.y0 < 80.0);
```

## Cargo Features

- **`programs`** *(default)*: Forwards `pdf-fonts/programs` so embedded TrueType, Type 1, CFF, and OpenType glyph outlines tighten `Glyph::quad` boxes.
- **`text-layout`** *(default)*: Enables `TextPage` reading-order assembly, `unicode-bidi` reordering, selection quads, search, and multiline `layout_text` / `Canvas::text_box` layout.
- **`predefined-cmaps`** *(optional)*: Forwards `pdf-fonts/predefined-cmaps` for bundled Adobe CJK CMaps and UCS-2 collections.
- **`tracing`** *(optional)*: Emits numeric [tracing](https://crates.io/crates/tracing) spans around content interpretation and re-encoding.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE)) or MIT license ([LICENSE-MIT](https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT)) at your option.
