# Changelog

## 0.1.0-alpha.1 (unreleased)

First release. Until `0.1.0` stabilizes, alpha releases may include breaking API changes documented here; depend on the exact version (`=0.1.0-alpha.1`).

### Added

- `Form`, `Form::read`, `Form::read_with`, and `Form::from_dict`: a catalog's `/AcroForm` with its field tree read under `Rules::Iso` or `Rules::Pdfium` (as of chromium/8152), with the form's `/DA`, `/Q`, `/NeedAppearances`, `/SigFlags`, `/XFA` presence, and default resources (`Form::resources`, `Form::font`, `Form::font_names`).
- `Field`, `FieldType`, `FieldKind`, `FieldFlags`, and `FieldValue`: each field with its effective (own or inherited) type, flags, value, default value, default appearance, quadding, and maximum length, and its kind by the rules read under (a button with both the push button and radio flags is PDFium's radio button, ISO 32000-2's push button); `Form::fields`, `Form::roots`, `Form::terminals`, `Form::full_name`, and `Form::find`.
- `Widget`, `Widget::annot`, and `Widget::annot_with`: a field's widgets, separate or merged with the field, each read as a `pdf_annots::Annot`.
- `Limits`, `Cap`, `Error`, and `Fault`: caps on the fields and widgets read, the tree's depth, reference hops, and a value's items; a field reached twice is left out, so a cycle through `/Kids` ends.
- Features `programs` and `text-layout` (default), `predefined-cmaps`, and `tracing`, each forwarded to the crates beneath.
