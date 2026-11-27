//! Zero-I/O PDF Image XObject and inline image (`BI..ID..EI`) decoder, mask
//! compositor, native-space pixel scrubber, and clean stream re-encoder
//! (ISO 32000-2, §7.4.6–§7.4.9, §8.9, §11.6.5), supporting both strict
//! ISO 32000-2 (`Rules::Iso`) and PDFium (`Rules::Pdfium`, as of
//! chromium/8152) policies.
//!
//! `pdf-images` operates over any [`Resolve`](pdf_objects::Resolve) +
//! [`Streams`](pdf_objects::Streams) provider without performing file I/O.
//!
//! # Core Concepts
//!
//! - **Two-tier decoding ([`RawImage`] and [`RgbaImage`]):**
//!   - **Tier 1 ([`RawImage`], [`SampleData`], [`decode_xobject_raw`], [`decode_inline_raw`], [`decode_stream_raw`]):**
//!     Extracts image samples in their native color space and bit depth
//!     (`1, 2, 4, 8, 16` bpc)—keeping bi-level `CCITTFaxDecode`, `JBIG2Decode`,
//!     and `/ImageMask` streams bit-packed—before `/Decode` mapping or color
//!     conversion.
//!   - **Tier 2 ([`RgbaImage`], [`decode_xobject`], [`decode_inline`], [`decode_stream`], [`RawImage::to_rgba8`]):**
//!     Applies `/Decode` intervals, evaluates colors through [`pdf_color`]'s
//!     [`PreparedSpace`](pdf_color::PreparedSpace), colors `/ImageMask`
//!     stencils, and composites explicit `/Mask` stencils, pre-`/Decode`
//!     color-key `/Mask` arrays, `/SMask` soft-mask streams with `/Matte`
//!     un-premultiplication, and `JPXDecode` `/SMaskInData` opacity channels
//!     into straight-alpha `RGBA8`.
//! - **All PDF image codecs ([`ImageCodec`]):**
//!   Supports raw/standard-filtered packed bitstreams, `DCTDecode` (`zune-jpeg`),
//!   `CCITTFaxDecode` (`hayro-ccitt`), `JBIG2Decode` (`hayro-jbig2` with
//!   `/JBIG2Globals`), and `JPXDecode` (`hayro-jpeg2000` with `/SMaskInData`
//!   and embedded JP2 color specifications).
//! - **Redaction pixel scrubbing & clean re-encoding ([`RawImage::fill_rect_raw`], [`encode_raw_flate`], [`encode_rgba_flate`], [`encode_jpeg`]):**
//!   Scrubs rectangular regions directly in native sample space and re-encodes
//!   cleanly via `FlateDecode` + PNG `Sub` prediction (preserving `/Mask`,
//!   `/SMask`, and `/OC`) or pure-Rust baseline `DCTDecode` (`jpeg-encoder`)
//!   with no EXIF, XMP, IPTC, comment, or JBIG2 dictionary leakage.
//! - **Bounded resources & third-party codec isolation ([`Limits`], [`Fault`]):**
//!   Dimensions, pixel counts, and byte budgets are checked before allocating
//!   buffers, and internal panics in third-party decoders (`zune-jpeg`,
//!   `hayro-jpeg2000`, `hayro-ccitt`, `hayro-jbig2`) are converted into
//!   [`Error::Codec`] via `std::panic::catch_unwind` under default
//!   `panic = "unwind"` builds (note that `panic = "abort"` binaries bypass
//!   unwind guards).
//!
//! # Example
//!
//! Encode a 4×4 RGB JPEG Image XObject, scrub a 2×2 redaction block in native
//! sample space, re-encode cleanly with `FlateDecode`, and evaluate to `RGBA8`:
//!
//! ```
//! use std::collections::BTreeMap;
//!
//! use pdf_images::pdf_filters::Level;
//! use pdf_images::pdf_objects::{Object, ObjectId, Unreadable, Why, WithStreams};
//! use pdf_images::{
//!     DecodeOptions, JpegColorSpace, Limits, decode_stream, decode_stream_raw, encode_jpeg,
//!     encode_raw_flate,
//! };
//!
//! // Build a clean 4x4 RGB JPEG Image XObject stream (solid red `[220, 40, 30]`).
//! let mut pixels = Vec::with_capacity(4 * 4 * 3);
//! for _ in 0..16 {
//!     pixels.extend_from_slice(&[220, 40, 30]);
//! }
//! let jpeg_xobj = encode_jpeg(&pixels, 4, 4, JpegColorSpace::Rgb, 95).unwrap();
//!
//! // Zero-I/O resolver with no indirect subsidiary streams.
//! let objects = BTreeMap::<ObjectId, Object>::new();
//! let provider = WithStreams::new(&objects, |id, _| {
//!     Err(Unreadable::new(id, Why::Damaged, "no indirect streams"))
//! });
//!
//! // Tier 1: decode native 8-bit RGB samples, scrub top-left 2x2 to black, and re-encode as Flate.
//! let mut raw = decode_stream_raw(
//!     provider,
//!     &jpeg_xobj,
//!     None,
//!     &DecodeOptions::PDFIUM,
//!     &Limits::DEFAULT,
//! )
//! .unwrap();
//! raw.fill_rect_raw(0, 0, 2, 2, &[0, 0, 0]).unwrap();
//! let clean_flate = encode_raw_flate(&raw, None, Level::DEFAULT).unwrap();
//!
//! // Tier 2: evaluate the scrubbed Flate Image XObject to straight-alpha RGBA8.
//! let rgba = decode_stream(
//!     provider,
//!     &clean_flate.stream,
//!     None,
//!     &DecodeOptions::PDFIUM,
//!     &Limits::DEFAULT,
//! )
//! .unwrap();
//! assert_eq!(rgba.pixel(0, 0), Some([0, 0, 0, 255]));
//! assert!(rgba.pixel(3, 3).unwrap()[0] > 200);
//! ```

pub use pdf_color;
pub use pdf_content_streams;
pub use pdf_filters;
pub use pdf_objects;

mod codecs;
mod encode;
mod error;
mod fault;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod header;
mod inline_chain;
mod limits;
mod phases;
mod rgba;
mod samples;

pub use encode::{
    EncodedImage, JpegColorSpace, encode_jpeg, encode_jpeg_bytes, encode_png_rgba8,
    encode_raw_flate, encode_rgba_flate,
};
pub use error::{EncodeError, Error, ScrubError};
pub use fault::Fault;
pub use header::{ImageCodec, ImageMeta, MaskSpec, RenderingIntent, SMaskInData};
pub use limits::{LimitKind, Limits};
pub use rgba::{
    DecodeOptions, RgbaImage, decode_inline, decode_inline_raw, decode_inline_raw_with,
    decode_inline_with, decode_stream, decode_stream_raw, decode_stream_raw_with,
    decode_stream_with, decode_xobject, decode_xobject_raw, decode_xobject_raw_with,
    decode_xobject_with,
};
pub use samples::{RawImage, SampleData};

#[cfg(test)]
mod doc_tests {
    use super::*;

    #[test]
    fn public_types_are_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<Limits>();
        check::<LimitKind>();
        check::<Error>();
        check::<EncodeError>();
        check::<ScrubError>();
        check::<Fault>();
        check::<ImageCodec>();
        check::<ImageMeta>();
        check::<MaskSpec>();
        check::<SMaskInData>();
        check::<RenderingIntent>();
        check::<SampleData>();
        check::<RawImage>();
        check::<RgbaImage>();
        check::<DecodeOptions<'static>>();
        check::<EncodedImage>();
        check::<JpegColorSpace>();
    }

    #[test]
    fn readme_links_and_first_code_block_match_lib_rs() {
        let readme = include_str!("../README.md");
        let lib_rs = include_str!("lib.rs");
        let repository = env!("CARGO_PKG_REPOSITORY");
        for file in ["LICENSE-APACHE", "LICENSE-MIT"] {
            let link = format!("[{file}]({repository}/blob/main/{file})");
            assert!(
                readme.contains(&link),
                "{file}'s link isn't the repository's URL"
            );
        }
        assert!(!readme.contains("](../"), "a link climbs out of the crate");

        let readme_block = readme
            .split_once("```rust\n")
            .and_then(|(_, rest)| rest.split_once("\n```"))
            .map(|(code, _)| code)
            .expect("first ```rust block in README.md");

        let mut lib_lines = Vec::new();
        let mut inside = false;
        for line in lib_rs.lines() {
            let Some(doc) = line.strip_prefix("//!") else {
                if inside {
                    break;
                }
                continue;
            };
            let trimmed = doc.strip_prefix(' ').unwrap_or(doc);
            if !inside {
                if trimmed.starts_with("```") {
                    inside = true;
                }
            } else if trimmed == "```" {
                break;
            } else {
                lib_lines.push(trimmed);
            }
        }
        assert_eq!(lib_lines.join("\n"), readme_block);
    }

    #[test]
    fn debug_redacts_pixel_samples_and_buffers() {
        let secret_rgb = vec![
            0xDE_u8, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
        ];
        let stream = encode_jpeg(&secret_rgb, 2, 2, JpegColorSpace::Rgb, 90).unwrap();
        let objects =
            std::collections::BTreeMap::<pdf_objects::ObjectId, pdf_objects::Object>::new();
        let provider = pdf_objects::WithStreams::new(&objects, |id, _| {
            Err(pdf_objects::Unreadable::new(
                id,
                pdf_objects::Why::Damaged,
                "none",
            ))
        });
        let raw = decode_stream_raw(
            provider,
            &stream,
            None,
            &DecodeOptions::PDFIUM,
            &Limits::DEFAULT,
        )
        .unwrap();
        let rgba = raw
            .to_rgba8(provider, &DecodeOptions::PDFIUM, &Limits::DEFAULT)
            .unwrap();

        let raw_dbg = format!("{raw:?}");
        let rgba_dbg = format!("{rgba:?}");
        assert!(raw_dbg.contains("sample_count: 12"), "{raw_dbg}");
        assert!(!raw_dbg.contains("222"), "{raw_dbg}");
        assert!(rgba_dbg.contains("byte_len: 16"), "{rgba_dbg}");
        assert!(!rgba_dbg.contains("255"), "{rgba_dbg}");
    }
}

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
