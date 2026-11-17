//! PDF annotations, appearance placement, destinations, and actions
//! (ISO 32000-2, §12.3.2, §12.5, §12.6), matching both ISO 32000-2 rules and
//! PDFium's annotation and link resolution rules as of chromium/8152.
//!
//! `pdf-annots` operates over borrowed [`Dict`](pdf_objects::Dict) values and
//! any [`Resolve`](pdf_objects::Resolve) provider without performing file I/O.
//! Standalone annotation dictionaries, direct `/Annots` entries, and merged
//! Field+Widget dictionaries share the same [`Annot`] wrapper.
//!
//! # Core Concepts
//!
//! - **Page `/Annots` & `&Dict` inspection ([`Annot`], [`page_annots`], [`page_annots_with`], [`page_tabs`]):**
//!   - Walks page `/Annots` arrays under [`Rules::Iso`](pdf_objects::pdf_tokenizer::Rules::Iso)
//!     or [`Rules::Pdfium`](pdf_objects::pdf_tokenizer::Rules::Pdfium), decoding `/Subtype`
//!     ([`AnnotSubtype`]), `/Rect`, [`AnnotFlags`], [`AnnotColor`] (`/C`, `/IC`), [`Border`]
//!     (`/BS`, legacy `/Border`, `/BE`), [`AppearanceChars`] (`/MK`), [`DefaultAppearance`]
//!     (`/DA`, `pdf-gfx`'s shared parser and serializer, re-exported), and [`MarkupInfo`] (`/T`,
//!     `/Subj`, `/CA`, `/CreationDate`, `/IRT`, `/Popup`).
//!   - [`Annot::from_dict`] and [`Annot::from_dict_with`] inspect any `&Dict` directly (including
//!     merged Field+Widget dictionaries).
//! - **Typed subtype payloads ([`TypedAnnot`]):**
//!   - Structured views for `Text` ([`TextAnnot`]), `Link` ([`LinkAnnot`]), `FreeText`
//!     ([`FreeTextAnnot`]), text markups ([`TextMarkupAnnot`], [`QuadPoints`]), `Line`
//!     ([`LineAnnot`]), `Square`/`Circle` ([`ShapeAnnot`]), `Polygon`/`PolyLine` ([`PolyAnnot`]),
//!     `Ink` ([`InkAnnot`]), `Stamp` ([`StampAnnot`]), `Caret` ([`CaretAnnot`]), `Popup`
//!     ([`PopupAnnot`]), `FileAttachment` ([`FileAttachmentAnnot`]), `Redact` ([`RedactAnnot`]),
//!     and `Widget` ([`WidgetAnnot`]).
//! - **Appearances & §12.5.5 placement ([`Appearances`], [`placement_matrix`], [`stream_placement_matrix`]):**
//!   - Resolves single-stream and state-subdictionary `/AP` (`/N`, `/R`, `/D`) entries against
//!     `/AS`, and computes the exact `BBox * FormMatrix -> Rect` placement [`Matrix`](pdf_gfx::Matrix).
//! - **Destinations, actions & reply threads ([`Dest`], [`DestTable`], [`Action`], [`resolve_threads`]):**
//!   - Re-exports [`pdf_semantics::dest`] and [`pdf_semantics::action`] for explicit/named
//!     destinations and bounded `/A`, `/Next`, `/AA`, and `/OpenAction` chains, and resolves
//!     `/IRT` reply/group hierarchies with effective [`MarkedState`] and [`ReviewState`] tracking.
//! - **Appearance synthesis, builders & zero-I/O flattening ([`SynthesizedAp`], [`BuiltAnnot`], [`AnnotMut`], [`plan_flatten`]):**
//!   - Deterministically synthesizes `/AP /N` (and `/Redact` `/RO`) Form XObjects over
//!     [`pdf_gfx::Canvas`] (FreeText and plain `/Square` borders in all five `/BS` styles: solid,
//!     dashed, beveled, inset, underline), constructs or mutates annotation dictionaries without
//!     file I/O, and plans graphics-state-isolated page flattening with collision-free
//!     `/Resources/XObject` names.
//!   - Authors `/Redact` marks whole ([`RedactMark`], [`synth_redact`],
//!     [`BuiltAnnot::redact_mark`], [`BuiltAnnot::redact_quads`]): the mark and its `/RO` overlay
//!     drawn over each `/QuadPoints` quadrilateral, the overlay also bound as `/AP /R`, with
//!     [`AnnotMut`] setters for `/RO`, `/OverlayText`, and `/Repeat`.
//!   - Flattening keeps `/Redact` marks on the page, undrawn, unless
//!     [`FlattenOpts::with_redactions`] asks for them: it never removes the content under a
//!     mark, so a mark painted onto the page would only look like a redaction.
//!
//! # Quickstart
//!
//! Inspect a page's `/Link` and `/Highlight` annotations and resolve a named destination to its
//! 0-based target page index:
//!
//! ```
//! use std::collections::BTreeMap;
//!
//! use pdf_annots::pdf_objects::pdf_tokenizer::Rules;
//! use pdf_annots::pdf_objects::{Object, ObjectId, dict};
//! use pdf_annots::pdf_semantics::pages;
//! use pdf_annots::{
//!     AnnotFlags, DestFit, DestTable, Limits, TextMarkupKind, TypedAnnot, page_annots,
//! };
//!
//! let page1_id = ObjectId::new(3, 0);
//! let page2_id = ObjectId::new(4, 0);
//! let link_id = ObjectId::new(5, 0);
//! let mark_id = ObjectId::new(6, 0);
//!
//! let mut objects = BTreeMap::new();
//! objects.insert(
//!     ObjectId::new(2, 0),
//!     Object::from(dict! {
//!         "Type" => Object::name("Pages"),
//!         "Kids" => vec![Object::from(page1_id), Object::from(page2_id)],
//!         "Count" => 2,
//!         "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(612), Object::from(792)],
//!     }),
//! );
//! let page1 = dict! {
//!     "Type" => Object::name("Page"),
//!     "Parent" => ObjectId::new(2, 0),
//!     "Annots" => vec![Object::from(link_id), Object::from(mark_id)],
//! };
//! objects.insert(page1_id, Object::from(page1.clone()));
//! objects.insert(
//!     page2_id,
//!     Object::from(dict! { "Type" => Object::name("Page"), "Parent" => ObjectId::new(2, 0) }),
//! );
//! objects.insert(
//!     link_id,
//!     Object::from(dict! {
//!         "Subtype" => Object::name("Link"),
//!         "Rect" => vec![Object::from(72), Object::from(700), Object::from(200), Object::from(720)],
//!         "F" => AnnotFlags::PRINT.bits() as i64,
//!         "Dest" => Object::text("intro"),
//!     }),
//! );
//! objects.insert(
//!     mark_id,
//!     Object::from(dict! {
//!         "Subtype" => Object::name("Highlight"),
//!         "Rect" => vec![Object::from(72), Object::from(650), Object::from(180), Object::from(664)],
//!         "QuadPoints" => vec![
//!             Object::from(72), Object::from(664), Object::from(180), Object::from(664),
//!             Object::from(72), Object::from(650), Object::from(180), Object::from(650),
//!         ],
//!         "T" => Object::text("Reviewer"),
//!     }),
//! );
//! let catalog = dict! {
//!     "Type" => Object::name("Catalog"),
//!     "Pages" => ObjectId::new(2, 0),
//!     "Names" => dict! {
//!         "Dests" => dict! {
//!             "Names" => vec![
//!                 Object::text("intro"),
//!                 Object::from(vec![Object::from(page2_id), Object::name("FitH"), Object::from(750)]),
//!             ],
//!         },
//!     },
//! };
//!
//! let annots = page_annots(&objects, &page1, Some(page1_id), Rules::Pdfium, &Limits::DEFAULT).unwrap();
//! assert_eq!(annots.len(), 2);
//! assert!(annots[0].is_printable());
//!
//! // Resolve the Link's named destination ("intro") to page index 1.
//! let sem_limits = pdf_annots::pdf_semantics::Limits::DEFAULT;
//! let table = pages::pages(&objects, &catalog, Rules::Pdfium, &sem_limits, &mut ()).unwrap();
//! let dests = DestTable::read(&objects, Some(&catalog), &table, Rules::Pdfium, &sem_limits, &mut ()).unwrap();
//! let TypedAnnot::Link(link) = annots[0].typed() else { unreachable!() };
//! let resolved = dests.resolve(link.effective_dest().unwrap(), Rules::Pdfium, &mut ()).unwrap();
//! assert_eq!(resolved.page_index, 1);
//! assert_eq!(resolved.fit, DestFit::FitH { top: Some(750.0) });
//!
//! // Inspect the Highlight annotation's QuadPoints and author.
//! let TypedAnnot::TextMarkup(hl) = annots[1].typed() else { unreachable!() };
//! assert_eq!(hl.kind, TextMarkupKind::Highlight);
//! assert_eq!(hl.quad_points.len(), 1);
//! assert_eq!(annots[1].markup().unwrap().author.as_deref(), Some("Reviewer"));
//! ```

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

pub use pdf_content_streams;
pub use pdf_fonts;
pub use pdf_gfx;
pub use pdf_objects;
pub use pdf_semantics;

mod annot;
mod ap;
mod build;
mod error;
mod fault;
mod flags;
mod flatten;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod limits;
mod phases;
mod replies;
mod resolve;
mod style;
mod subtypes;
mod synth;
#[cfg(test)]
mod tests;

pub use annot::{Annot, MarkupInfo, TabOrder, page_annots, page_annots_with, page_tabs};
pub use ap::{
    AppearanceEntry, AppearanceMode, Appearances, placement_matrix, stream_placement_matrix,
};
pub use build::{AnnotMut, BuiltAnnot, push_page_annot};
pub use error::Error;
pub use fault::Fault;
pub use flags::AnnotFlags;
pub use flatten::{FlattenOpts, FlattenPlan, FlattenTarget, plan_flatten, plan_flatten_filtered};
pub use limits::{Cap, Limits};
pub use pdf_semantics::action::{Action, AdditionalActions, NamedAction, OpenAction};
pub use pdf_semantics::dest::{
    Dest, DestFit, DestNameKind, DestTable, ExplicitDest, PageTarget, ResolvedDest, resolve_dest,
};
pub use replies::{AnnotThread, MarkedState, ReplyKind, ReviewState, resolve_threads};
pub use resolve::QuadPoints;
pub use style::{
    AnnotColor, AppearanceChars, Border, BorderStyle, DefaultAppearance, LineEnding, PaintStyle,
    Quadding,
};
pub use subtypes::{
    AnnotSubtype, AttachmentIcon, CaretAnnot, CaretSymbol, FileAttachmentAnnot, FreeTextAnnot,
    FreeTextIntent, HighlightMode, InkAnnot, LineAnnot, LinkAnnot, PolyAnnot, PolyKind, PopupAnnot,
    RedactAnnot, ShapeAnnot, ShapeKind, StampAnnot, StampName, TextAnnot, TextIcon,
    TextMarkupAnnot, TextMarkupKind, TypedAnnot, WidgetAnnot,
};
pub use synth::{
    RedactMark, SynthesizedAp, synth_annot, synth_free_text, synth_ink, synth_line, synth_poly,
    synth_redact, synth_shape, synth_stamp, synth_text_markup, synth_text_note,
};
