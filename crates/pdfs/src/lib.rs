//! Read, inspect, edit, and write PDF files through one dependency.
//!
//! `pdfs` provides a high-level, viewer-compatible API on top of a modular
//! family of PDF crates, and re-exports every crate in the family at matching
//! versions ([`structure`], [`objects`], [`fonts`], [`gfx`], [`annots`], etc.)
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
//! - **Inspect pages, geometry, annotations, and graphics** with [`Pdf::pages`],
//!   [`Page::geom`], [`Page::annots`], [`Page::content`], [`Page::interpret`],
//!   and [`Page::encode`] (2D page transforms, annotations/links/destinations,
//!   placed paths, glyph origins/quads, images, shadings, clips, marked
//!   content, and position-independent content re-encoding).
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
//! # Features
//!
//! - **`programs`**, **`text-layout`**, and **`clock`** *(enabled by default)*:
//!   parse embedded font programs via `skrifa`, assemble `TextPage` spatial
//!   reading order / search / multiline `Canvas` layout, and enable
//!   `Date::now` / `Date::from_unix` in `pdf-semantics`.
//! - **`embed`** *(enabled by default)*: embed a TrueType program in a
//!   document, subset to the glyphs its pages draw, and draw Unicode text in
//!   it (`Document::embed_font`, `EmbeddedFont`), through pdf-fonts' `embed`.
//!   Programs with CFF, CFF2 or bitmap-only outlines are refused, text is
//!   drawn a character at a time without shaping, and justified text in an
//!   embedded font is laid out flush left.
//! - **`image-ends`**, **`predefined-cmaps`**, and **`simd`**: decode inline
//!   DCT/CCITTFax images to find their end, bundle Adobe predefined CJK CMaps,
//!   and enable runtime CPU SIMD in `pdf-content-streams` dependencies.
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

/// PDF color space parsing, component counting, and resource lookup.
pub use pdf_color;
/// [`pdf_color`] under its shorter name: the same crate, so either path names
/// the same items.
pub use pdf_color as color;

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
        #[cfg(feature = "text-layout")]
        {
            shared::<pdf_gfx::TextPage>();
            shared::<pdf_gfx::TextMatch>();
            shared::<pdf_gfx::LaidOutText>();
        }
        shared::<crate::Document<'_>>();
        #[cfg(feature = "embed")]
        shared::<crate::EmbeddedFont>();
        shared::<crate::Options<'_>>();
        shared::<crate::Limits>();
        shared::<crate::Error>();
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
