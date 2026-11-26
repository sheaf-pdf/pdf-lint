//! Read, inspect, edit, and write PDF files through one dependency.
//!
//! `pdfs` provides a high-level, viewer-compatible API on top of a modular
//! family of PDF crates, and re-exports every crate in the family at matching
//! versions ([`structure`], [`objects`], [`fonts`], [`gfx`], [`annots`], [`forms`], etc.)
//! so you can drop down to lower-level APIs whenever needed. Each is there
//! under its own crate name too ([`pdf_structure`], [`pdf_objects`], etc.),
//! and both paths name the same items.
//!
//! # Overview
//!
//! - **Open files lazily** via [`open`] (positional disk reads on Unix/Windows)
//!   or [`from_bytes`] (in-memory buffers or custom [`pdf_structure::ByteSource`]s),
//!   configured with [`Options`]. Objects are parsed on demand and cached
//!   safely across threads on [`Pdf`].
//! - **Inspect and render pages, geometry, annotations, images, and graphics**
//!   with [`Pdf::pages`], [`Page::geom`], [`Page::annots`], [`Page::content`],
//!   [`Page::interpret`], [`Page::encode`], `Page::decode_image` (`images`
//!   feature), and `Page::render` (`raster` feature): 2D page transforms,
//!   annotations/links/destinations, placed paths, glyph origins/quads, images,
//!   shadings, clips, marked content, position-independent content re-encoding,
//!   and pure-Rust 2D page and tile rasterization.
//! - **Extract text and fonts** with [`Page::text`] and [`Page::fonts`] in
//!   content-stream order, or use `Page::text_page` and `Page::search`
//!   (`text-layout` feature) for spatial reading order, `unicode-bidi` RTL
//!   reordering, selection quads, and search.
//! - **Read document metadata and catalog structures** including version,
//!   [`Pdf::encryption`], `/Info` dictionary, XMP metadata and conformance
//!   claims, bookmarks ([`Pdf::outlines`]), page labels ([`Pdf::page_labels`]),
//!   embedded/associated files ([`Pdf::embedded_files`],
//!   [`Pdf::associated_files`]), optional content ([`Pdf::optional_content`]),
//!   viewer preferences ([`Pdf::catalog_view`]), and linearization validity
//!   ([`Pdf::linearized`]).
//! - **Create, edit, encrypt, and linearize** documents via [`Document::new`]
//!   or [`Pdf::edit`]: draw pages with [`standard_font`] and [`pdf_gfx::Canvas`]
//!   (including multiline `Canvas::text_box` under `text-layout`), draw Unicode
//!   text in a TrueType font embedded with `Document::embed_font` (`embed`
//!   feature) and subset to the glyphs drawn at each save, author `/Info`, XMP
//!   metadata/claims, bookmarks, page labels, attachments, and viewer
//!   preferences, encrypt with AES-256, and save whole, linearized, or as an
//!   incremental update.
//! - **Use standalone trait functions** in [`page`](mod@page),
//!   [`file`](mod@file), and [`build`](mod@build) over your own
//!   [`pdf_structure::Reading`] or [`pdf_structure::Document`]. Import common
//!   types in one line with [`prelude`].
//!
//! Resource limits across all subsystems live in [`Limits`], and errors unify
//! into [`Error`]. Untrusted files never cause panics. Methods ending in
//! `_with` report recoverable spec departures to a
//! [`fault::Sink`](pdf_objects::fault::Sink), while plain methods skip fault
//! collection at zero cost.
//!
//! # Crates
//!
//! Every crate of the family is re-exported at the root under two names, its
//! own and the one without `pdf_`; both name the same crate, so an item
//! reached through either path is the same item.
//!
//! | Short name | Crate | What it holds |
//! |---|---|---|
//! | [`tokenizer`] | [`pdf_tokenizer`] | The lossless, zero-copy `no_std` lexer, under ISO 32000-2's rules and PDFium's. |
//! | [`objects`] | [`pdf_objects`] | Object values with byte spans, the object parser and writer, text string codecs. |
//! | [`filters`] | [`pdf_filters`] | The standard stream filter decoders and the Flate encoder. |
//! | [`crypt`] | [`pdf_crypt`] | The standard security handler: decrypting and password-encrypting files. |
//! | [`structure`] | [`pdf_structure`] | File structure: lazy positional reading, in-memory documents, full and incremental writers. |
//! | [`color`] | [`pdf_color`] | Color space parsing, §7.10 functions, CIE/ICC evaluation, and resource lookup. |
//! | `images` | `pdf_images` | *(with `images`)* Image XObject and inline image decoding, mask compositing, pixel scrubbing, and Flate/JPEG re-encoding. |
//! | [`content_streams`] | [`pdf_content_streams`] | Content stream lexing, typed operators, graphics-state checks, splicing, and writing. |
//! | [`semantics`] | [`pdf_semantics`] | Dates, name and number trees, page trees, destinations, actions, `/Info`, and XMP claims. |
//! | [`fonts`] | [`pdf_fonts`] | Font dictionaries, encodings, CMaps, font programs, Standard 14 metrics, text extraction, TrueType embedding. |
//! | [`gfx`] | [`pdf_gfx`] | 2D geometry, the graphics and text state interpreter, the content re-encoder, the canvas, `/DA` strings. |
//! | [`annots`] | [`pdf_annots`] | Annotations, appearance synthesis, builders, reply threads, and page flattening. |
//! | [`forms`] | [`pdf_forms`] | Interactive forms: the field tree, inherited field values, widgets, button groups' rules, and text, button, and choice fields' appearances. |
//! | `raster` | `pdf_raster` | *(with `raster`)* Pure-Rust 2D PDF page, tile, Form XObject, annotation, and form rasterizer (`Page::render`, `RenderedDoc`, `RustBackend`). |
//! | [`linearize`] | [`pdf_linearize`] | The linearized ("Fast Web View") layout engine, hint tables, and validator. |
//!
//! # Features
//!
//! - **`images`** and **`raster`** *(enabled by default; `raster` implies `images`)*:
//!   enable `pdfs::images` (`Page::decode_image`) and `pdfs::raster`
//!   (`Page::render`, `RenderedDoc` on [`Pdf`], `RustBackend`).
//! - **`programs`**, **`text-layout`**, **`clock`**, and **`icc`** *(enabled by default)*:
//!   parse embedded font programs via `skrifa`, assemble `TextPage` spatial
//!   reading order / search / multiline `Canvas` layout, enable `Date::now` /
//!   `Date::from_unix` in `pdf-semantics`, and evaluate `ICCBased` /
//!   `/OutputIntents` profiles via `moxcms`.
//! - **`embed`** *(enabled by default)*: embed a TrueType program in a
//!   document, subset to the glyphs its pages draw, and draw Unicode text in
//!   it (`Document::embed_font`, `EmbeddedFont`), through pdf-fonts' `embed`;
//!   with pdf-gfx's (`ProgramFont`, which an `EmbeddedFont` lays text out
//!   through) and pdf-forms' (`appearance::text_with_fallback`), each with
//!   `text-layout`, which none of them turns on.
//!   Programs with CFF, CFF2 or bitmap-only outlines are refused, text is
//!   drawn a character at a time without shaping, and justified text in an
//!   embedded font is laid out flush left.
//! - **`image-ends`**, **`predefined-cmaps`**, **`simd`**, and **`pdfium`**: decode
//!   inline DCT/CCITTFax images to find their end, bundle Adobe predefined CJK
//!   CMaps, enable runtime CPU SIMD in `pdf-content-streams`, `pdf-images`, and
//!   `pdf-raster`, and enable `PdfiumBackend` (a thin `RenderBackend` adapter
//!   over `pdfium-render` `0.9.6`).
//! - **`tracing`**: emit `debug`-level `tracing` spans (numeric counts and
//!   lengths only, never file content).
//! - **`insecure-test-writer`**: deterministic seeded encryption keys and IVs
//!   for tests and fuzzing only (never enable in production).
//!
//! # Quickstart
//!
//! ```
//! use pdfs::prelude::*;
//!
//! // Create a one-page PDF in memory so this example runs standalone.
//! let mut doc = Document::new();
//! let page = doc.add_page(Rect::LETTER);
//! page.begin_text();
//! page.font(standard_font(Std14::Helvetica), 24.0);
//! page.text_move(72.0, 700.0);
//! page.show(&Std14::Helvetica.codes("Hello, world").unwrap());
//! page.end_text();
//! let file = doc.to_bytes().unwrap();
//!
//! // Open the PDF and extract text from each page.
//! let pdf = pdfs::from_bytes(file).unwrap();
//! let pages = pdf.pages().unwrap();
//! assert_eq!(pages.len(), 1);
//! for page in &pages {
//!     assert_eq!(page.geom().media_box, Rect::LETTER);
//!     let shown = page.text().unwrap();
//!     assert_eq!(shown.text(), "Hello, world");
//!     assert!(shown.is_whole());
//! }
//! ```

#[cfg(feature = "raster")]
pub use backend::RustBackend;
pub use build::standard_font;
pub use document::Document;
#[cfg(feature = "embed")]
pub use embed::EmbeddedFont;
pub use error::Error;
pub use limits::Limits;
#[cfg(any(unix, windows))]
pub use open::open;
pub use open::{Options, from_bytes};
pub use page::{Content, NamedFont};
pub use pdf::{Page, Pages, PagesIter, Pdf};
pub use pdf_gfx::Rect;
#[cfg(feature = "pdfium")]
pub use pdf_raster::PdfiumBackend;

#[cfg(feature = "raster")]
pub mod backend;
pub mod build;
mod document;
#[cfg(feature = "embed")]
mod embed;
mod error;
pub mod file;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod limits;
mod open;
pub mod page;
mod pdf;
mod phases;
pub mod prelude;

/// Lossless, zero-copy `no_std` PDF lexer supporting ISO 32000-2 and PDFium rules.
pub use pdf_tokenizer;
/// [`pdf_tokenizer`] under its shorter name: the same crate, so either path names
/// the same items.
pub use pdf_tokenizer as tokenizer;

/// Owned PDF object values with byte spans, object parser/writer, and text string codecs.
pub use pdf_objects;
/// [`pdf_objects`] under its shorter name: the same crate, so either path names
/// the same items.
pub use pdf_objects as objects;

/// Standard PDF stream filter decoders and Flate encoder (`miniz_oxide` and `zlib-rs`).
pub use pdf_filters;
/// [`pdf_filters`] under its shorter name: the same crate, so either path names
/// the same items.
pub use pdf_filters as filters;

/// Standard PDF security handler for decrypting and password-encrypting files.
pub use pdf_crypt;
/// [`pdf_crypt`] under its shorter name: the same crate, so either path names
/// the same items.
pub use pdf_crypt as crypt;

/// PDF file structure: lazy positional reading, in-memory documents, and full/incremental writers.
pub use pdf_structure;
/// [`pdf_structure`] under its shorter name: the same crate, so either path names
/// the same items.
pub use pdf_structure as structure;

/// PDF color space parsing, §7.10 functions, CIE/ICC evaluation, and resource lookup.
pub use pdf_color;
/// [`pdf_color`] under its shorter name: the same crate, so either path names
/// the same items.
pub use pdf_color as color;

/// PDF Image XObject and inline image decoding, mask compositing, pixel scrubbing, and Flate/JPEG re-encoding (`images` feature).
#[cfg(feature = "images")]
pub use pdf_images;
/// [`pdf_images`] under its shorter name (`images` feature): the same crate, so
/// either path names the same items.
#[cfg(feature = "images")]
pub use pdf_images as images;

/// PDF content stream lexing, typed operators, graphics-state checks, splicing, and writing.
pub use pdf_content_streams;
/// [`pdf_content_streams`] under its shorter name: the same crate, so either path names
/// the same items.
pub use pdf_content_streams as content_streams;

/// Common PDF structures: dates, name/number trees, page trees, destinations, actions, `/Info`, and XMP claims.
pub use pdf_semantics;
/// [`pdf_semantics`] under its shorter name: the same crate, so either path names
/// the same items.
pub use pdf_semantics as semantics;

/// PDF font dictionaries, encodings, CMaps, embedded font programs, Standard 14 metrics, text extraction, and TrueType font embedding.
pub use pdf_fonts;
/// [`pdf_fonts`] under its shorter name: the same crate, so either path names
/// the same items.
pub use pdf_fonts as fonts;

/// PDF 2D geometry, bounded graphics and text state interpreter, content re-encoder, and vector/text canvas.
pub use pdf_gfx;
/// [`pdf_gfx`] under its shorter name: the same crate, so either path names
/// the same items.
pub use pdf_gfx as gfx;

/// PDF annotations, appearance synthesis, builders, reply threads, and zero-I/O page flattening.
pub use pdf_annots;
/// [`pdf_annots`] under its shorter name: the same crate, so either path names
/// the same items.
pub use pdf_annots as annots;

/// PDF interactive forms: the field tree, inherited field values, widgets,
/// button groups' rules, and text, button, and choice fields' appearances.
pub use pdf_forms;
/// [`pdf_forms`] under its shorter name: the same crate, so either path names
/// the same items.
pub use pdf_forms as forms;

/// Pure-Rust 2D PDF page, tile, Form XObject, annotation, and interactive form rasterizer (`raster` feature).
#[cfg(feature = "raster")]
pub use pdf_raster;
/// [`pdf_raster`] under its shorter name (`raster` feature): the same crate, so
/// either path names the same items.
#[cfg(feature = "raster")]
pub use pdf_raster as raster;

/// Linearized ("Fast Web View") PDF layout engine, hint tables, and validator.
pub use pdf_linearize;
/// [`pdf_linearize`] under its shorter name: the same crate, so either path names
/// the same items.
pub use pdf_linearize as linearize;

#[cfg(test)]
mod tests {
    /// Each crate of the family is at the root under two names, its own and
    /// the one without `pdf_`, and an item named through either is the same
    /// item: a value of one path's type is taken where the other's is asked.
    #[test]
    fn each_crate_is_under_its_short_name_and_its_own() {
        fn same<T>(_: fn(T) -> T, _: fn(T) -> T) {}
        fn id<T>(value: T) -> T {
            value
        }
        same(
            id::<crate::tokenizer::Rules>,
            id::<crate::pdf_tokenizer::Rules>,
        );
        same(
            id::<crate::objects::ObjectId>,
            id::<crate::pdf_objects::ObjectId>,
        );
        same(
            id::<crate::filters::Backend>,
            id::<crate::pdf_filters::Backend>,
        );
        same(
            id::<crate::crypt::Permissions>,
            id::<crate::pdf_crypt::Permissions>,
        );
        same(
            id::<crate::structure::Document>,
            id::<crate::pdf_structure::Document>,
        );
        same(id::<crate::color::Limits>, id::<crate::pdf_color::Limits>);
        #[cfg(feature = "images")]
        same(id::<crate::images::Limits>, id::<crate::pdf_images::Limits>);
        same(
            id::<crate::content_streams::ContentWriter>,
            id::<crate::pdf_content_streams::ContentWriter>,
        );
        same(
            id::<crate::semantics::date::Date>,
            id::<crate::pdf_semantics::date::Date>,
        );
        same(id::<crate::fonts::Std14>, id::<crate::pdf_fonts::Std14>);
        same(id::<crate::gfx::Canvas>, id::<crate::pdf_gfx::Canvas>);
        same(
            id::<crate::annots::BuiltAnnot>,
            id::<crate::pdf_annots::BuiltAnnot>,
        );
        same(id::<crate::forms::Limits>, id::<crate::pdf_forms::Limits>);
        #[cfg(feature = "raster")]
        same(id::<crate::raster::Pixmap>, id::<crate::pdf_raster::Pixmap>);
        same(
            id::<crate::linearize::Limits>,
            id::<crate::pdf_linearize::Limits>,
        );
    }

    /// The README is shown on crates.io and docs.rs, where a path beside the
    /// crate reaches nothing: its licence links are the repository's URLs, the
    /// manifest's `repository` and the file, and every link it has is a URL.
    #[test]
    fn the_readmes_licence_links_reach_the_files() {
        let readme = include_str!("../README.md");
        let repository = env!("CARGO_PKG_REPOSITORY");
        for file in ["LICENSE-APACHE", "LICENSE-MIT"] {
            let link = format!("[{file}]({repository}/blob/main/{file})");
            assert!(
                readme.contains(&link),
                "{file}'s link isn't the repository's URL"
            );
        }
        for (at, _) in readme.match_indices("](") {
            let target = &readme[at + 2..];
            let target = &target[..target.find(')').unwrap_or(target.len())];
            assert!(
                target.starts_with("https://"),
                "a link isn't a URL: {target}"
            );
        }
    }

    /// The crate doc's example is the README's first, line for line, so that
    /// what docs.rs shows first and what crates.io shows first are one
    /// example, and each runs as a doctest.
    #[test]
    fn the_crate_docs_example_is_the_readmes_first() {
        let readme: Vec<&str> = (include_str!("../README.md").lines())
            .skip_while(|line| *line != "```rust")
            .skip(1)
            .take_while(|line| *line != "```")
            .collect();
        let doc: Vec<&str> = (include_str!("lib.rs").lines())
            .map_while(|line| line.strip_prefix("//!"))
            .map(|line| line.strip_prefix(' ').unwrap_or(line))
            .skip_while(|line| *line != "```")
            .skip(1)
            .take_while(|line| *line != "```")
            .collect();
        assert!(readme.len() > 10, "the README's first example: {readme:?}");
        assert_eq!(doc, readme);
    }

    /// What the crate gives a caller is sent to and shared by any thread: a
    /// file being read and what's read of it, a page's content and text, a
    /// document being written, and their options, caps and errors.
    #[test]
    fn what_it_gives_is_sent_and_shared() {
        fn shared<T: Send + Sync>() {}
        shared::<crate::Pdf>();
        shared::<crate::Pdf<&[u8]>>();
        shared::<crate::Pdf<std::sync::Arc<[u8]>>>();
        #[cfg(any(unix, windows))]
        shared::<crate::Pdf<pdf_structure::read::FileBytes>>();
        shared::<crate::Pages<'_, Vec<u8>>>();
        shared::<crate::Page<'_, Vec<u8>>>();
        shared::<crate::PagesIter<'_, Vec<u8>>>();
        shared::<crate::Content>();
        shared::<crate::NamedFont>();
        shared::<pdf_fonts::Shown>();
        shared::<crate::Rect>();
        shared::<pdf_gfx::PageGeom>();
        shared::<pdf_gfx::Summary>();
        shared::<pdf_gfx::EncodedPage>();
        shared::<pdf_gfx::Canvas>();
        shared::<pdf_gfx::Limits>();
        shared::<pdf_gfx::Error>();
        shared::<pdf_annots::Annot<'static>>();
        shared::<pdf_annots::BuiltAnnot>();
        shared::<pdf_annots::FlattenPlan>();
        shared::<pdf_annots::Limits>();
        shared::<pdf_annots::Error>();
        shared::<pdf_forms::Form<'static>>();
        shared::<pdf_forms::Field<'static>>();
        shared::<pdf_forms::Limits>();
        shared::<pdf_forms::Error>();
        #[cfg(feature = "text-layout")]
        {
            shared::<pdf_forms::appearance::FieldFont>();
            shared::<pdf_forms::appearance::Appearance>();
            shared::<pdf_forms::appearance::Unmade>();
            shared::<pdf_gfx::TextPage>();
            shared::<pdf_gfx::TextMatch>();
            shared::<pdf_gfx::LaidOutText>();
        }
        shared::<crate::Document<'_>>();
        #[cfg(feature = "embed")]
        shared::<crate::EmbeddedFont>();
        #[cfg(all(feature = "embed", feature = "text-layout"))]
        {
            shared::<pdf_forms::appearance::Fallback<'static>>();
            shared::<pdf_forms::appearance::MadeWithFallback>();
            shared::<pdf_gfx::WithFallback<'static, pdf_fonts::Std14, crate::EmbeddedFont>>();
        }
        shared::<crate::Options<'_>>();
        shared::<crate::Limits>();
        shared::<crate::Error>();
        #[cfg(feature = "images")]
        {
            shared::<pdf_images::RawImage>();
            shared::<pdf_images::RgbaImage>();
            shared::<pdf_images::Limits>();
            shared::<pdf_images::Error>();
        }
        #[cfg(feature = "raster")]
        {
            shared::<crate::RustBackend>();
            shared::<pdf_raster::Pixmap>();
            shared::<pdf_raster::RenderOpts>();
            shared::<pdf_raster::RenderCache>();
            shared::<pdf_raster::Limits>();
            shared::<pdf_raster::Error>();
        }
    }

    #[cfg(feature = "raster")]
    #[test]
    fn page_render_decode_image_rendered_doc_and_rust_backend_work_end_to_end() {
        use std::sync::Arc;

        use crate::pdf_images::{JpegColorSpace, encode_jpeg};
        use crate::prelude::*;

        let mut doc = Document::new();
        let img_stream = encode_jpeg(
            &[20, 120, 220, 20, 120, 220, 20, 120, 220, 20, 120, 220],
            2,
            2,
            JpegColorSpace::Rgb,
            95,
        )
        .unwrap();
        let img_id = doc.structure_mut().add(Object::from(img_stream));
        {
            let p = doc.add_page(Rect::new(0.0, 0.0, 100.0, 100.0));
            p.fill_color(Color::rgb8([230, 50, 40]));
            p.rect(10.0, 10.0, 80.0, 80.0);
            p.paint(Paint::Fill);
        }
        let bytes = doc.to_bytes().unwrap();

        let pdf = crate::from_bytes(&bytes[..]).unwrap();
        let pages = pdf.pages().unwrap();
        let page = pages.get(0).unwrap();

        // Direct image decode on `Page`:
        let decoded = page.decode_image(img_id).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 2));

        // Direct page rasterization on `Page`:
        let opts = RenderOpts::with_viewport(Viewport::Page {
            width: 100,
            rotation: Rotation::R0,
        });
        let (pix, sum) = page.render(&opts).unwrap();
        assert_eq!((pix.width(), pix.height()), (100, 100));
        assert_eq!(pix.pixel_rgba8(50, 50), Some([230, 50, 40, 255]));
        assert_eq!(pix.pixel_rgba8(2, 2), Some([255, 255, 255, 255]));
        assert!(sum.reported && sum.gfx.segments >= 1);

        // `Pdf` directly implements `RenderedDoc`:
        assert_eq!(RenderedDoc::page_count(&pdf), 1);
        assert_eq!(RenderedDoc::page_geom(&pdf, 0).unwrap(), page.geom());
        let (dpix, _) = RenderedDoc::render_page(&pdf, 0, &opts).unwrap();
        assert_eq!(dpix, pix);

        // `RustBackend` (`RenderBackend` trait) + `From<BackendError> for Error`:
        let mut tight_open = Options::DEFAULT;
        tight_open.rules = Rules::Iso;
        tight_open.limits.raster.max_pixels = 5_000;
        let backend = RustBackend::new().with_options(tight_open);
        let rendered_doc = backend.open_bytes(Arc::from(bytes), None).unwrap();
        assert_eq!(rendered_doc.page_count(), 1);
        // Customizing one field on `opts.limits` must NOT drop `Pdf`'s `max_pixels = 5_000` cap!
        let mut tweaked_opts = opts.clone();
        tweaked_opts.limits.max_annots = 10;
        let capped_err = rendered_doc.render_page(0, &tweaked_opts).unwrap_err();
        assert!(matches!(
            Error::from(capped_err),
            Error::Raster(crate::pdf_raster::Error::Limit(
                crate::pdf_raster::LimitKind::Pixels
            ))
        ));

        // `From<BackendError> for Error` unwraps a boxed `pdfs::Error` directly:
        let wrapped = BackendError::custom(Error::NoCatalog);
        assert!(matches!(Error::from(wrapped), Error::NoCatalog));
    }

    #[test]
    fn page_geom_interpret_encode_and_canvas_round_trip() {
        use std::ops::ControlFlow;

        use crate::pdf_gfx::{Kind, Opts};
        use crate::prelude::*;

        let mut doc = Document::new();
        {
            let mut canvas = Canvas::new();
            canvas.fill_color(Color::Gray(0.2));
            canvas.fill_rect(Rect::new(50.0, 60.0, 150.0, 110.0));
            canvas.text_at(
                standard_font(Std14::Helvetica),
                14.0,
                Point::new(72.0, 700.0),
                &Std14::Helvetica.codes("Canvas Hi").unwrap(),
            );
            *doc.add_page(Rect::LETTER) = canvas.into_writer();
        }
        let pdf = crate::from_bytes(doc.to_bytes().unwrap()).unwrap();
        let pages = pdf.pages().unwrap();
        let page = pages.get(0).unwrap();

        let geom = page.geom();
        assert_eq!(geom.view_size().width, 612.0);
        assert_eq!(geom.view_size().height, 792.0);
        assert_eq!(geom.rotation, Rotation::R0);

        let mut path_count = 0;
        let mut glyphs = Vec::new();
        let summary = page
            .interpret(Opts::default(), |ev| {
                match ev.kind {
                    Kind::Path(_) => path_count += 1,
                    Kind::Text(show) => {
                        for g in show.glyphs() {
                            glyphs.push((g.text().unwrap().into_owned(), g.origin));
                        }
                    }
                    _ => {}
                }
                ControlFlow::Continue(())
            })
            .unwrap();
        assert_eq!(path_count, 1);
        assert_eq!(summary.glyphs, 9);
        assert_eq!(glyphs[0], ("C".to_string(), Point::new(72.0, 700.0)));

        let encoded = page.encode().unwrap();
        assert!(!encoded.content.is_empty());
        assert!(encoded.forms.is_empty());

        #[cfg(feature = "text-layout")]
        {
            let tp = page.text_page(TextPageOpts::default()).unwrap();
            assert_eq!(tp.text(), "Canvas Hi");
            let hits = page.search("canvas", SearchOpts::default()).unwrap();
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].chars, 0..6);
        }
    }

    #[test]
    fn annotations_destinations_and_incremental_flattening_round_trip() {
        use std::ops::ControlFlow;

        use crate::pdf_gfx::{Kind, Opts};
        use crate::prelude::*;

        let mut doc = Document::new();
        {
            let p1 = doc.add_page(Rect::LETTER);
            p1.begin_text();
            p1.font(standard_font(Std14::Helvetica), 14.0);
            p1.text_move(72.0, 700.0);
            p1.show(&Std14::Helvetica.codes("See page 2").unwrap());
            p1.end_text();
        }
        doc.add_page(Rect::LETTER);

        let ids = doc.page_ids().unwrap();
        let (p1_id, p2_id) = (ids[0].unwrap(), ids[1].unwrap());
        doc.add_annot(
            p1_id,
            BuiltAnnot::link_dest(Rect::new(72.0, 696.0, 160.0, 716.0), &Dest::fit(p2_id)),
        )
        .unwrap();
        doc.add_annot(
            p1_id,
            BuiltAnnot::text_markup(
                TextMarkupKind::Highlight,
                &[QuadPoints::from_rect(Rect::new(72.0, 696.0, 160.0, 716.0))],
                Color::Rgb([1.0, 0.95, 0.2]).into(),
                0.6,
            ),
        )
        .unwrap();

        let base_bytes = doc.to_bytes().unwrap();
        let pdf = crate::from_bytes(&base_bytes[..]).unwrap();
        let pages = pdf.pages().unwrap();
        let dests = pages.dests().unwrap();
        let page0 = pages.get(0).unwrap();
        let annots = page0.annots().unwrap();
        assert_eq!(annots.len(), 2);
        let TypedAnnot::Link(lnk) = annots[0].typed() else {
            panic!("expected Link");
        };
        let resolved = dests
            .resolve(lnk.effective_dest().unwrap(), Rules::Pdfium, &mut ())
            .unwrap();
        assert_eq!(resolved.page_index, 1);

        // Edit and flatten via an incremental update!
        let mut edited = pdf.edit().unwrap();
        let plan = edited.flatten_page(p1_id, FlattenOpts::DEFAULT).unwrap();
        assert_eq!(plan.drawn_count, 1);
        assert_eq!(plan.removed_count, 2);
        let mut update = Vec::new();
        edited.save_update_to(&mut update).unwrap();

        let flattened_pdf = crate::from_bytes([&base_bytes[..], &update[..]].concat()).unwrap();
        let fpages = flattened_pdf.pages().unwrap();
        let fp0 = fpages.get(0).unwrap();
        assert!(fp0.annots().unwrap().is_empty());
        assert_eq!(fp0.text().unwrap().text(), "See page 2");
        let mut paths = 0;
        fp0.interpret(Opts::default(), |ev| {
            if matches!(ev.kind, Kind::Path(_)) {
                paths += 1;
            }
            ControlFlow::Continue(())
        })
        .unwrap();
        assert_eq!(paths, 1);
    }

    #[test]
    fn a_redaction_is_added_with_its_overlay_as_its_rollover_appearance() {
        use crate::pdf_annots::{AppearanceMode, RedactMark};
        use crate::prelude::*;

        let mut doc = Document::new();
        doc.add_page(Rect::LETTER);
        let page_id = doc.page_ids().unwrap()[0].unwrap();
        let quads = [
            QuadPoints::from_rect(Rect::new(72.0, 700.0, 200.0, 716.0)),
            QuadPoints([210.0, 682.0, 290.0, 686.0, 285.0, 706.0, 205.0, 701.0]),
        ];
        let mark = RedactMark::new()
            .with_quads(&quads)
            .with_interior_color(Color::Gray(0.0).into())
            .with_overlay_text("REDACTED");
        let built = BuiltAnnot::redact_quads(&mark).unwrap();
        let (mark_form, overlay_form) = (
            built.normal_ap.clone().unwrap(),
            built.redact_overlay.clone().unwrap(),
        );
        doc.add_annot(page_id, built).unwrap();

        let bytes = doc.to_bytes().unwrap();
        let pdf = crate::from_bytes(&bytes[..]).unwrap();
        let pages = pdf.pages().unwrap();
        let page = pages.get(0).unwrap();
        let annots = page.annots().unwrap();
        assert_eq!(annots.len(), 1);
        assert_eq!(annots[0].rect(), Rect::new(72.0, 682.0, 290.0, 716.0));
        let TypedAnnot::Redact(rd) = annots[0].typed() else {
            panic!("expected Redact");
        };
        assert_eq!(rd.quad_points, quads);
        // /AP holds /N and /R, and /R is the /RO overlay.
        let overlay = rd.overlay_form.expect("/RO");
        let aps = annots[0].appearances().expect("/AP");
        let normal = aps
            .stream(AppearanceMode::Normal, false, false)
            .expect("/AP /N");
        assert_eq!(
            aps.stream(AppearanceMode::Rollover, false, false),
            Some(overlay)
        );
        assert_ne!(normal, overlay);
        // Each stream is the one that was built.
        let reading = pdf.reading();
        let decoded = |id| {
            crate::pdf_objects::Streams::decoded(&reading, id, usize::MAX)
                .unwrap()
                .to_vec()
        };
        assert_eq!(decoded(normal), mark_form.content);
        assert_eq!(decoded(overlay), overlay_form.content);

        // Flattening leaves the mark on the page, undrawn, unless asked.
        let mut edited = pdf.edit().unwrap();
        let plan = edited.flatten_page(page_id, FlattenOpts::DEFAULT).unwrap();
        assert_eq!((plan.drawn_count, plan.removed_count), (0, 0));
        let plan = edited
            .flatten_page(page_id, FlattenOpts::DEFAULT.with_redactions(true))
            .unwrap();
        assert_eq!((plan.drawn_count, plan.removed_count), (1, 1));
    }

    #[test]
    fn document_structures_outlines_labels_attachments_viewer_and_metadata_round_trip() {
        use crate::pdf_semantics::date::Date;
        use crate::pdf_semantics::info::Dated;
        use crate::pdf_semantics::xmp::check_info_sync;
        use crate::prelude::*;

        let mut doc = Document::new();
        for n in 1..=3 {
            let p = doc.add_page(Rect::LETTER);
            p.begin_text();
            p.font(standard_font(Std14::Helvetica), 14.0);
            p.text_move(72.0, 700.0);
            p.show(&Std14::Helvetica.codes(&format!("Page {n}")).unwrap());
            p.end_text();
        }
        let ids = doc.page_ids().unwrap();
        let (p0, p2) = (ids[0].unwrap(), ids[2].unwrap());

        doc.set_outlines(&[
            OutlineNode::new("Cover").with_dest(Dest::fit(p0)),
            OutlineNode::new("Summary")
                .with_flags(OutlineFlags::BOLD)
                .with_children([OutlineNode::new("Conclusion").with_dest(Dest::fit(p2))]),
        ])
        .unwrap();

        doc.set_page_labels(&[
            LabelRange::new(0, Some(LabelStyle::LowerRoman)),
            LabelRange::new(1, Some(LabelStyle::Decimal)).with_prefix("Sec-"),
        ])
        .unwrap();

        let xml_payload = b"<summary>ok</summary>".to_vec();
        doc.add_embedded_file(
            &NewEmbeddedFile::new("summary.xml", xml_payload.len())
                .with_subtype("application/xml")
                .with_description("Summary XML")
                .with_relationship(AfRelationship::Data),
            xml_payload.clone(),
        )
        .unwrap();

        let mut info = Info::default();
        info.title = Some("Annual Binder".into());
        info.author = Some("Jane Doe".into());
        info.created = Some(Dated::Date(Date::read("D:20261115100000Z").unwrap()));
        doc.set_info(&info);
        let mut meta = Metadata::from_info(&info);
        meta.claims.pdfa_part = Some("3".into());
        meta.claims.pdfa_conformance = Some("B".into());
        doc.set_metadata(&meta).unwrap();

        let mut vp = ViewerPrefs::default();
        vp.display_doc_title = true;
        vp.print_scaling = Some(PrintScaling::None);
        vp.duplex = Some(Duplex::Simplex);
        vp.num_copies = Some(2);
        let mut view = CatalogView::default();
        view.page_mode = Some(PageMode::UseOutlines);
        view.lang = Some("en-US".into());
        view.viewer_prefs = Some(vp);
        doc.set_catalog_view(&view).unwrap();

        let bytes = doc.to_bytes().unwrap();
        let pdf = crate::from_bytes(&bytes[..]).unwrap();

        let outlines = pdf.outlines().unwrap().unwrap();
        assert_eq!(outlines.len(), 2);
        assert_eq!(outlines[0].title, "Cover");
        assert_eq!(outlines[1].children[0].title, "Conclusion");

        let labels = pdf.page_labels().unwrap().unwrap();
        assert_eq!(labels.format(0), "i");
        assert_eq!(labels.format(1), "Sec-1");
        assert_eq!(labels.format(2), "Sec-2");

        let efs = pdf.embedded_files().unwrap();
        assert_eq!(efs.len(), 1);
        assert_eq!(efs[0].name, "summary.xml");
        assert_eq!(
            efs[0]
                .embedded
                .as_ref()
                .unwrap()
                .bytes(pdf.reading(), 1024)
                .unwrap(),
            xml_payload
        );
        assert_eq!(pdf.associated_files().unwrap().len(), 1);

        let read_info = pdf.info().unwrap().unwrap();
        let read_meta = pdf.metadata().unwrap().unwrap();
        assert_eq!(read_info, info);
        assert_eq!(read_meta, meta);
        assert!(check_info_sync(&read_meta, &read_info).is_empty());
        assert_eq!(pdf.catalog_view().unwrap(), view);
    }
}

/// The README's examples, run as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
