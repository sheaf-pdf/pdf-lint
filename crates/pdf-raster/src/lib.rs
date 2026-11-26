//! Zero-I/O pure-Rust PDF 2D page, Form XObject, annotation, and interactive
//! form rasterizer (ISO 32000-2, §8–§9, §11, §12.5, §12.7, §14.6), supporting
//! both strict ISO 32000-2 (`Rules::Iso`) and viewer (`Rules::Pdfium`, as of
//! chromium/8152) policies.
//!
//! `pdf-raster` operates over any [`Resolve`](pdf_objects::Resolve) +
//! [`Streams`](pdf_objects::Streams) provider without performing file I/O,
//! keeping its internal 2D software compositor private so only domain types
//! ([`Pixmap`], [`Viewport`], [`RenderOpts`], [`PageGeom`](pdf_gfx::PageGeom))
//! appear in the public API.
//!
//! # Core Concepts
//!
//! - **Viewer-aligned [`Viewport`]s & [`Pixmap`] formats:**
//!   Supports whole-page width fits (`Viewport::Page`), zoomed sub-rectangles
//!   (`Viewport::Tile`), physical DPI (`Viewport::Dpi`, honoring `/UserUnit`),
//!   and explicit affine transforms (`Viewport::Custom`), outputting straight
//!   `RGBA8`, native `BGRA8`, or premultiplied `RGBA8` under upfront [`Limits`]
//!   budgets.
//! - **Complete 2D PDF imaging pipeline ([`render_page`], [`render_page_with`], [`render_content`]):**
//!   - Vector paths (`NonZero` & `EvenOdd` fills, anisotropic user-space strokes,
//!     caps, joins, miter limits, dash patterns, and `0 w` device hairlines).
//!   - Clipping stacks (`W` / `W*` path clips, `4..=7 Tr` glyph-outline text
//!     clips, and Form XObject `/BBox` clips).
//!   - Text rendering across embedded TrueType, OpenType/CFF, CFF, and Type 1
//!     programs (`pdf-fonts`), Type 3 `/CharProcs` (`d0` colored and `d1`
//!     uncolored stencils), caller-supplied [`FontFallback`] faces, and built-in
//!     vector fallback contours.
//!   - Placed Image XObjects and inline images via `pdf-images` (`DCTDecode`,
//!     `CCITTFaxDecode`, `JBIG2Decode`, `JPXDecode`, `/ImageMask` stencils,
//!     `/Mask`, and `/SMask` + `/Matte`).
//!   - PDF §11 transparency groups (`/Group`), all 16 separable and
//!     non-separable blend modes (`/BM`), constant opacities (`/ca`, `/CA`), and
//!     `/Alpha` / `/Luminosity` soft masks (`/SMask` with `/BC` and `/TR`).
//!   - Colored and uncolored tiling patterns (`PatternType 1`) and shadings
//!     (`sh` and `PatternType 2`, `ShadingType 1..=5`) evaluated through
//!     `pdf-color`.
//!   - Optional content groups (`/OCProperties` `/OCG` & `/OCMD`), non-widget
//!     `/Annots` (`pdf-annots`), and interactive `/AcroForm` widgets
//!     (`pdf-forms`), with stateless [`Appearances::Synthesize`] or
//!     [`Appearances::WrittenOnly`] modes.
//!
//! # Example
//!
//! Build a one-page PDF in memory with [`pdf_gfx::Canvas`] and rasterize it
//! with zero I/O:
//!
//! ```
//! use std::collections::BTreeMap;
//!
//! use pdf_raster::pdf_content_streams::{Color, Reserved};
//! use pdf_raster::pdf_gfx::{Canvas, Rect, Rotation};
//! use pdf_raster::pdf_objects::pdf_tokenizer::Rules;
//! use pdf_raster::pdf_objects::{Object, ObjectId, Stream, Unreadable, Why, WithStreams, dict};
//! use pdf_raster::pdf_semantics::Limits as SemLimits;
//! use pdf_raster::pdf_semantics::pages::pages;
//! use pdf_raster::{RenderCache, RenderOpts, Viewport, render_page};
//!
//! // Draw a red banner onto a 200 × 100 pt page.
//! let mut canvas = Canvas::new();
//! canvas.fill_color(Color::rgb8([220, 40, 30]));
//! canvas.fill_rect(Rect::from_xywh(20.0, 20.0, 160.0, 60.0));
//! let (content_bytes, _) = canvas.finish(&Reserved::default());
//!
//! let mut objects = BTreeMap::new();
//! let content_id = ObjectId::new(4, 0);
//! objects.insert(content_id, Object::from(Stream::new(dict! {}, content_bytes)));
//! objects.insert(
//!     ObjectId::new(3, 0),
//!     Object::from(dict! {
//!         "Type" => Object::name("Page"),
//!         "Parent" => ObjectId::new(2, 0),
//!         "Contents" => content_id,
//!     }),
//! );
//! objects.insert(
//!     ObjectId::new(2, 0),
//!     Object::from(dict! {
//!         "Type" => Object::name("Pages"),
//!         "Kids" => vec![Object::reference(3)],
//!         "Count" => 1,
//!         "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(200), Object::from(100)],
//!     }),
//! );
//! let catalog = dict! { "Type" => Object::name("Catalog"), "Pages" => ObjectId::new(2, 0) };
//!
//! let provider = WithStreams::new(&objects, |id, limit| {
//!     let data = objects
//!         .get(&id)
//!         .and_then(Object::as_stream)
//!         .and_then(|s| s.data.bytes())
//!         .ok_or_else(|| Unreadable::new(id, Why::Damaged, "missing stream"))?;
//!     if data.len() > limit {
//!         return Err(Unreadable::new(id, Why::Limit, "stream exceeds limit"));
//!     }
//!     Ok(data.to_vec())
//! });
//!
//! let table = pages(provider, &catalog, Rules::Pdfium, &SemLimits::DEFAULT, &mut ()).unwrap();
//! let opts = RenderOpts::with_viewport(Viewport::Page {
//!     width: 200,
//!     rotation: Rotation::R0,
//! });
//! let (pixmap, _) = render_page(
//!     provider,
//!     Some(&catalog),
//!     table.get(0).unwrap(),
//!     &RenderCache::default(),
//!     None,
//!     &opts,
//! )
//! .unwrap();
//!
//! assert_eq!((pixmap.width(), pixmap.height()), (200, 100));
//! assert_eq!(pixmap.pixel_rgba8(100, 50), Some([220, 40, 30, 255]));
//! assert_eq!(pixmap.pixel_rgba8(5, 5), Some([255, 255, 255, 255]));
//! ```

pub use pdf_annots;
pub use pdf_color;
pub use pdf_content_streams;
pub use pdf_fonts;
pub use pdf_forms;
pub use pdf_gfx;
pub use pdf_images;
pub use pdf_objects;
pub use pdf_semantics;

mod builtin_glyphs;
mod canvas2d;
mod engine;
mod error;
mod fallback;
mod fault;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod limits;
mod opts;
mod paint;
mod phases;
mod pixmap;

pub use engine::{render_content, render_content_with, render_page, render_page_with};
pub use error::Error;
pub use fallback::{FontFallback, FontQuery, RenderCache};
pub use fault::Fault;
pub use limits::{LimitKind, Limits};
pub use opts::{
    AntiAlias, Appearances, Draw, RenderMedium, RenderOpts, RenderSummary, ResolvedViewport,
    Viewport,
};
pub use pixmap::{PixelFormat, Pixmap};

#[cfg(test)]
mod doc_tests {
    use super::*;

    #[test]
    fn public_types_are_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<Limits>();
        check::<LimitKind>();
        check::<Error>();
        check::<Fault>();
        check::<PixelFormat>();
        check::<Pixmap>();
        check::<Viewport>();
        check::<ResolvedViewport>();
        check::<Draw>();
        check::<Appearances>();
        check::<RenderMedium>();
        check::<AntiAlias>();
        check::<RenderOpts>();
        check::<RenderSummary>();
        check::<FontQuery>();
        check::<RenderCache>();
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

        fn first_rust_block_readme(md: &str) -> &str {
            let after = md
                .split_once("```rust\n")
                .expect("README.md has a ```rust block")
                .1;
            after.split_once("\n```").expect("closing fence").0
        }

        fn first_doc_block_lib(src: &str) -> String {
            let mut lines = src.lines();
            while let Some(line) = lines.next() {
                if line.trim() == "//! ```" || line.trim() == "//! ```rust" {
                    let mut out = Vec::new();
                    for body in lines.by_ref() {
                        if body.trim() == "//! ```" {
                            return out.join("\n");
                        }
                        let stripped = body
                            .strip_prefix("//! ")
                            .or_else(|| body.strip_prefix("//!"))
                            .expect("doc comment prefix");
                        out.push(stripped);
                    }
                }
            }
            panic!("lib.rs has a //! ``` block");
        }

        assert_eq!(
            first_rust_block_readme(readme),
            first_doc_block_lib(lib_rs),
            "first code block in README.md must match first //! block in src/lib.rs"
        );
    }

    #[test]
    fn debug_formatting_redacts_pixel_bytes() {
        let sentinel = [0xDE, 0xAD, 0xBE, 0xEF];
        let mut bytes = Vec::with_capacity(16);
        for _ in 0..4 {
            bytes.extend_from_slice(&sentinel);
        }
        let pix = Pixmap::from_bytes(2, 2, PixelFormat::Rgba8, bytes, &Limits::DEFAULT).unwrap();
        let dbg = format!("{pix:?}");
        assert!(dbg.contains("width: 2"));
        assert!(dbg.contains("byte_len: 16"));
        assert!(!dbg.contains("222"));
        assert!(!dbg.to_ascii_lowercase().contains("deadbeef"));
    }
}

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
