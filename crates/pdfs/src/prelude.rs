//! Convenience re-exports for common PDF reading, rendering, and writing
//! workflows (`use pdfs::prelude::*;`).
//!
//! Includes `pdfs`'s primary entry points (`Pdf`, `Document`, `Page`, `Options`,
//! `Limits`, `open`, `from_bytes`, `standard_font`, `EmbeddedFont` with `embed`,
//! `RustBackend` with `raster`, and `PdfiumBackend` with `pdfium`) alongside
//! frequently used types from the re-exported crate family (`Rect`, `Std14`,
//! `Color`, `Paint`, `Password`, `Protection`, `Permissions`, `Encryption`,
//! `Object`, `Dict`, `ImageDrawn`, `ImageOf`, `RawImage` / `RgbaImage` with
//! `images`, `Pixmap` / `RenderOpts` / `RenderSummary` / `Renderer` / `Viewport`
//! / `RenderBackend` / `RenderedDoc` / `RenderSource` / `Parts` / `Refusal` with
//! `raster`, etc.) and annotation builder
//! types (`BuiltAnnot`, `RedactMark`, `DefaultAppearance`, `Quadding`,
//! `QuadPoints`, `PaintStyle`).

#[cfg(feature = "embed")]
pub use crate::EmbeddedFont;
#[cfg(feature = "pdfium")]
pub use crate::PdfiumBackend;
#[cfg(feature = "raster")]
pub use crate::RustBackend;
#[cfg(any(unix, windows))]
pub use crate::open;
pub use crate::{
    Content, Document, Error, Limits, NamedFont, Options, Page, Pages, Pdf, Rect, from_bytes,
    standard_font,
};

pub use pdf_annots::{
    Annot, AnnotColor, AnnotFlags, AnnotMut, AnnotSubtype, BuiltAnnot, DefaultAppearance,
    FlattenOpts, FlattenTarget, PaintStyle, QuadPoints, Quadding, RedactMark, TextMarkupKind,
    TypedAnnot,
};
pub use pdf_content_streams::{Color, ContentWriter, Paint, Res};
pub use pdf_crypt::{Encoded, Password, Permissions, Protection};
pub use pdf_fonts::{Font, Shown, ShownString, Std14, Unread};
pub use pdf_gfx::{
    Canvas, EncodedPage, ImageDrawn, ImageOf, Matrix, PageGeom, Point, Quad, Rotation,
};
#[cfg(feature = "text-layout")]
pub use pdf_gfx::{
    SearchOpts, TextAlign, TextBoxStyle, TextPage, TextPageOpts, VerticalAlign, WrapMode,
};
#[cfg(feature = "images")]
pub use pdf_images::{RawImage, RgbaImage};
pub use pdf_linearize::check::Verdict;
pub use pdf_objects::pdf_tokenizer::Rules;
pub use pdf_objects::version::Version;
pub use pdf_objects::{Dict, Object, ObjectId, Why};
#[cfg(feature = "raster")]
pub use pdf_raster::{
    BackendError, Parts, PixelFormat, Pixmap, Refusal, RenderBackend, RenderOpts, RenderSource,
    RenderSummary, RenderedDoc, Renderer, Viewport,
};
pub use pdf_semantics::action::{Action, OpenAction};
pub use pdf_semantics::dest::{Dest, DestFit, DestTable, ResolvedDest};
pub use pdf_semantics::files::{AfRelationship, EmbeddedFile, FileSpec, NewEmbeddedFile};
pub use pdf_semantics::info::Info;
pub use pdf_semantics::labels::{LabelRange, LabelStyle, PageLabels};
pub use pdf_semantics::ocg::{OcConfig, OcProperties, OcState, Ocg};
pub use pdf_semantics::outlines::{OutlineFlags, OutlineItem, OutlineNode};
pub use pdf_semantics::pages::PdfBox;
pub use pdf_semantics::viewer::{
    CatalogView, Direction, Duplex, PageLayout, PageMode, PrintScaling, ViewerPrefs,
};
pub use pdf_semantics::xmp::{Claims, Metadata};
pub use pdf_structure::Encryption;
