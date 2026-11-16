//! Convenience re-exports for common PDF reading and writing workflows (`use pdfs::prelude::*;`).
//!
//! Includes `pdfs`'s primary entry points (`Pdf`, `Document`, `Page`, `Options`,
//! `Limits`, `open`, `from_bytes`, `standard_font`, and `EmbeddedFont` with
//! `embed`) alongside frequently used
//! types from the re-exported crate family (`Rect`, `Std14`, `Color`, `Paint`,
//! `Password`, `Protection`, `Permissions`, `Encryption`, `Object`, `Dict`, etc.).

#[cfg(feature = "embed")]
pub use crate::EmbeddedFont;
#[cfg(any(unix, windows))]
pub use crate::open;
pub use crate::{
    Content, Document, Error, Limits, NamedFont, Options, Page, Pages, Pdf, from_bytes,
    standard_font,
};

pub use pdf_annots::{
    Annot, AnnotColor, AnnotFlags, AnnotMut, AnnotSubtype, BuiltAnnot, FlattenOpts, FlattenTarget,
    PaintStyle, QuadPoints, TextMarkupKind, TypedAnnot,
};
pub use pdf_content_streams::{Color, ContentWriter, Paint, Res};
pub use pdf_crypt::{Encoded, Password, Permissions, Protection};
pub use pdf_fonts::{Font, Shown, ShownString, Std14, Unread};
pub use pdf_gfx::{Canvas, Matrix, PageGeom, Point, Quad, Rotation};
#[cfg(feature = "text-layout")]
pub use pdf_gfx::{
    SearchOpts, TextAlign, TextBoxStyle, TextPage, TextPageOpts, VerticalAlign, WrapMode,
};
pub use pdf_linearize::check::Verdict;
pub use pdf_objects::pdf_tokenizer::Rules;
pub use pdf_objects::version::Version;
pub use pdf_objects::{Dict, Object, ObjectId, Why};
pub use pdf_semantics::action::{Action, OpenAction};
pub use pdf_semantics::dest::{Dest, DestFit, DestTable, ResolvedDest};
pub use pdf_semantics::info::Info;
pub use pdf_semantics::pages::Rect;
pub use pdf_semantics::xmp::Claims;
pub use pdf_structure::Encryption;
