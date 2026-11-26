//! Recoverable ISO 32000-2 departures reported to a [`pdf_objects::fault::Sink`]
//! during rasterization.

/// Non-fatal specification departures observed while rasterizing a page, Form
/// XObject, or appearance stream.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Fault {
    /// Recoverable content-stream or graphics-state departure from `pdf-gfx`.
    Gfx(pdf_gfx::Fault),
    /// Recoverable color-space or §7.10 function departure from `pdf-color`.
    Color(pdf_color::Fault),
    /// Recoverable image dictionary, filter, or mask departure from
    /// `pdf-images`.
    Image(pdf_images::Fault),
    /// Recoverable annotation dictionary or appearance departure from
    /// `pdf-annots`.
    Annot(pdf_annots::Fault),
    /// Recoverable interactive form departure from `pdf-forms`.
    Form(pdf_forms::Fault),
    /// Recoverable optional-content (`/OCProperties`) departure from
    /// `pdf-semantics`.
    Semantics(pdf_semantics::Fault),
    /// A `/Pattern` resource was missing required entries (`/PatternType`,
    /// `/BBox`, `/XStep`, `/YStep`, or `/Shading`) or was malformed.
    InvalidPattern,
    /// A `/Shading` dictionary or stream was malformed or used an unsupported
    /// `/ShadingType`.
    InvalidShading,
    /// An `/ExtGState` `/SMask` dictionary referenced a missing or non-Form
    /// `/G` stream or had an invalid `/S` subtype.
    InvalidSoftMask,
    /// Nested tiling pattern, soft mask, or Type 3 glyph exceeded
    /// [`crate::Limits::max_layer_depth`] and was skipped.
    LayerDepthExceeded,
    /// An image XObject or inline image failed to decode and was skipped (matching
    /// viewer recovery).
    SkippedImage,
}
