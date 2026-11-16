# Changelog

## 0.1.0-alpha.1 (unreleased)

First release. Until `0.1.0` stabilizes, alpha releases may include breaking API changes documented here; depend on the exact version (`=0.1.0-alpha.1`).

### Added

- `Annot`, `page_annots`, `page_annots_with`, `page_tabs`, `TabOrder`, and `MarkupInfo`: zero-I/O page `/Annots` traversal and `&Dict` wrapper under `Rules::Iso` and `Rules::Pdfium` (as of chromium/8152).
- `AnnotSubtype` and `TypedAnnot`: typed views for `Text`, `Link`, `FreeText`, `Highlight`/`Underline`/`Squiggly`/`StrikeOut` (`TextMarkupAnnot`, `QuadPoints`), `Line`, `Square`/`Circle` (`ShapeAnnot`), `Polygon`/`PolyLine` (`PolyAnnot`), `Ink`, `Stamp`, `Caret`, `Popup`, `FileAttachment`, `Redact`, and `Widget`.
- `AnnotFlags`, `AnnotColor`, `Border`, `BorderStyle`, `AppearanceChars`, `DefaultAppearance`, `LineEnding`, and `Quadding`: common visual, border, appearance-characteristic (`/MK`), and `/DA` default-appearance parsing and serialization.
- `Appearances`, `AppearanceEntry`, `AppearanceMode`, `placement_matrix`, and `stream_placement_matrix`: `/AP` + `/AS` resolution and ISO 32000-2 §12.5.5 Form XObject placement matrices.
- Re-exports `Dest`, `ExplicitDest`, `PageTarget`, `DestFit`, `DestNameKind`, `DestTable`, `ResolvedDest`, `resolve_dest`, `Action`, `NamedAction`, `AdditionalActions`, and `OpenAction` from `pdf-semantics`.
- `AnnotThread`, `ReplyKind`, `MarkedState`, `ReviewState`, and `resolve_threads`: `/IRT` reply/group threading and effective state aggregation.
- `SynthesizedAp`, `synth_annot`, `synth_text_markup`, `synth_shape`, `synth_line`, `synth_poly`, `synth_ink`, `synth_stamp`, `synth_text_note`, `synth_free_text`, and `synth_redact`: deterministic `/AP /N` (and `/RO`) Form XObject synthesis over `pdf_gfx::Canvas`.
- `AnnotMut`, `BuiltAnnot`, and `push_page_annot`: in-place `&mut Dict` annotation mutators and pure typed annotation builders.
- `FlattenTarget`, `FlattenOpts`, `FlattenPlan`, `plan_flatten`, and `plan_flatten_filtered`: zero-I/O page annotation flattening with `q ... Q` isolation and collision-free `/Resources/XObject` allocation.
- `Limits`, `Cap`, `Error`, and `Fault`: deterministic resource caps and non-fatal diagnostics reported via `pdf-objects`' `fault::Sink`.

### Notes

- Re-exports `pdf-objects`, `pdf-content-streams`, `pdf-semantics`, `pdf-fonts`, and `pdf-gfx` at the crate root. Requires Rust 1.92.
- Never panics on untrusted input, and `Debug` implementations and `tracing` spans never include file plaintext, URIs, destination names, or annotation text.
