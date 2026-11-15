# Changelog

## 0.1.0-alpha.1 (unreleased)

First release. Until `0.1.0` stabilizes, alpha releases may include breaking API changes documented here; depend on the exact version (`=0.1.0-alpha.1`).

### Added

- `tokens` and `operations`: zero-I/O content stream lexer (`Tokens`, `Tok`) and operator/operand parser (`Operations`, `Operation`, `Operand`, `Elements`) matching PDFium's parsing rules as of chromium/8152, with byte spans and on-demand operand decoding (`Operand::number`, `name`, `object`).
- `Operator`, `Operators`, `Category`, `Operands`, `Type`, `Paint`, `FillRule`, and `State`: typed ISO 32000-2 Annex A operator metadata, operand signatures, and graphics-object state transitions.
- `InlineImage`, `ImageStream`, and `ResCx`: inline image boundary detection and stream extraction (`InlineImage::stream`) matching PDFium (sizing unfiltered images via `ResCx::read` color space component counts and decoding Flate/LZW ends via `pdf-filters`), plus ISO 32000-2 §8.9.7 inline abbreviation expansion (`expand_inline_key`, `expand_inline_value`, `InlineImage::to_dict`).
- `Join` and `Concat`: multi-stream page `/Contents` concatenation (`Join::push_from`, `Join::push_contents`, `Join::push_or_empty`) over `pdf-objects`' `Streams`, mapping offsets and spans back to source streams (`Pos`, `SpanAcross`) and recording undecodable streams (`Concat::undecoded`).
- `balance`, `marked`, `names_used`, and `kept`: page-level and single-operation analyzers (`Balance`, `Marked`, `MarkedNest`, `Properties`, `NameSet`, `ResKind`, `Kept`) for `q`/`Q` graphics state balance, marked-content sequences, and referenced resource names.
- `fault::Fault` and `fault::Sink`: typed ISO 32000-2 content stream diagnostics with byte and stream locations, supporting zero-cost disabled sinks (`()`).
- `ContentWriter`, `Res`, and `Reserved`: deterministic content stream writer with content-addressed (`ResId`, XXH3-128) deferred resource naming (`Res::of`, `Res::named`, `Res::written`, `ContentWriter::named`) that avoids collisions with existing page resources (`Reserved::read`), plus `write_number` and `write_string`.
- `splice`, `splice_each`, and `without_keys`: byte-preserving content stream range removal and property-list key stripping (`Spliced`, `Rewrite`, `SpliceError`) verified to leave downstream graphics state unchanged.
- `Limits`: upfront resource caps on stream bytes, operands, nesting depth, inline images, and color space lookups (`Limits::DEFAULT`, `CodedEnds`).
- Optional `image-ends` (DCT and CCITTFax inline image end decoding via `zune-jpeg` and `hayro-ccitt`), `simd` (runtime SIMD in `memchr`, `twox-hash`, and `zune-jpeg`), and `tracing` (numeric phase spans) Cargo features.

### Notes

- Re-exports `pdf-objects` and `pdf-color` at `pdf_objects` and `pdf_color`. Requires Rust 1.89.
- Performs no file I/O, never panics on untrusted input, and never includes raw string or image bytes in `Debug` implementations or `tracing` spans (fault previews are capped at 40 bytes).
- Closed ISO 32000-2 syntax and graphics enums (`Tok`, `Props`, `MarkedEvent`, `Paint`, `FillRule`, `LineCap`, `LineJoin`, `TextMode`, `Blend`) are exhaustive; other enums and configuration/analysis structs (`Limits`, `GState`, `Balance`, `Marked`, `Kept`) are `#[non_exhaustive]`.
