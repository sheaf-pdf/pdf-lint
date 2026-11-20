# Changelog

## 0.1.0-alpha.1 (unreleased)

First release. Until `0.1.0` stabilizes, alpha releases may include breaking API changes documented here; depend on the exact version (`=0.1.0-alpha.1`).

### Added

- `Form`, `Form::read`, `Form::read_with`, and `Form::from_dict`: a catalog's `/AcroForm` with its field tree read under `Rules::Iso` or `Rules::Pdfium` (as of chromium/8152), with the form's `/DA`, `/Q`, `/NeedAppearances`, `/SigFlags`, `/XFA` presence, and default resources (`Form::resources`, `Form::font`, `Form::font_names`).
- `Field`, `FieldType`, `FieldKind`, `FieldFlags`, and `FieldValue`: each field with its effective (own or inherited) type, flags, value, default value, default appearance, quadding, and maximum length, and its kind by the rules read under (a button with both the push button and radio flags is PDFium's radio button, ISO 32000-2's push button); `Form::fields`, `Form::roots`, `Form::terminals`, `Form::full_name`, and `Form::find`.
- `Widget`, `Widget::annot`, and `Widget::annot_with`: a field's widgets, separate or merged with the field, each read as a `pdf_annots::Annot`.
- `Limits`, `Cap`, `Error`, and `Fault`: caps on the fields and widgets read, the tree's depth, reference hops, and a value's items; a field reached twice is left out, so a cycle through `/Kids` ends.
- `appearance::FieldFont` and `appearance::FieldLayoutFont` (`text-layout`): the font a field's text is drawn in, found by its `/DA` name in the default resources and read with `pdf-fonts` (`FieldFont::find`, `Form::field_font`), or a standard 14 font that name falls back to (`FieldFont::standard`), with the resource an appearance names it by and, for its layout, the heights of the face PDFium (as of chromium/8152) draws a standard 14 font in, and no box for a standard 14 Type 1 font that embeds no program.
- `appearance::text`, `appearance::TextField`, `appearance::WidgetLook`, `appearance::Appearance`, and `appearance::Unmade` (`text-layout`): a text field's appearance made from plain data as PDFium (as of chromium/8152) makes it, its background, border, `/Tx` sequence with its clip, and text (a single line, wrapped lines, comb cells, a password, an automatic size from PDFium's sizes), or why none could be made.
- `appearance::widget_text`, `appearance::WidgetText`, `appearance::for_widget`, and `appearance::Made` (`text-layout`): a widget's appearance from what `Form` reads, its `/DA` the widget's own else the field's, parsed once, and its font kept by the caller across widgets or read for one.
- `Limits::text`, `Limits::fonts`, and `Limits::font_bytes`, with `Cap::Text` and `Cap::FontBytes`; `Fault::DefaultAppearance`, `Fault::MissingDefaultAppearance`, `Fault::FontNotInResources`, and `Fault::Font`.
- Features `programs` and `text-layout` (default), `predefined-cmaps`, and `tracing`, each forwarded to the crates beneath; `text-layout` adds the `appearance` module.
