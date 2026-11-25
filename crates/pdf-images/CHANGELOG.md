# Changelog

## 0.1.0-alpha.1 (Unreleased)

- Initial release of `pdf-images`: zero-I/O PDF Image XObject and inline image (`BI..ID..EI`) decoding over `pdf_objects::{Resolve, Streams}` under explicit `Limits` and `Rules::Iso` / `Rules::Pdfium` policies.
- Two-tier extraction: native-sample `RawImage` (`1, 2, 4, 8, 16` bpc across unfiltered/standard-filtered streams, `DCTDecode`, `CCITTFaxDecode`, `JBIG2Decode` with `/JBIG2Globals`, and `JPXDecode` with `/SMaskInData`) and color-evaluated, mask-composited `RgbaImage` (`/ImageMask` stencils, explicit & color-key `/Mask`, and `/SMask` + `/Matte`).
- Native-space rectangular pixel scrubbing (`RawImage::fill_rect_raw`) and clean metadata-free `FlateDecode` + PNG predictor and baseline `DCTDecode` re-encoders for redaction and sanitization workflows.
