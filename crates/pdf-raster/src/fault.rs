//! Recoverable ISO 32000-2 departures and fidelity approximations reported to a
//! [`pdf_objects::fault::Sink`] during rasterization.

/// Non-fatal specification departures and visual approximations observed while
/// rasterizing a page, Form XObject, or appearance stream.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Fault {
    /// Recoverable content-stream or graphics-state departure from `pdf-gfx`.
    #[error("graphics fault: {0:?}")]
    Gfx(pdf_gfx::Fault),
    /// Recoverable color-space or §7.10 function departure from `pdf-color`.
    #[error(transparent)]
    Color(#[from] pdf_color::Fault),
    /// Recoverable image dictionary, filter, or mask departure from
    /// `pdf-images`.
    #[error(transparent)]
    Image(#[from] pdf_images::Fault),
    /// Recoverable annotation dictionary or appearance departure from
    /// `pdf-annots`.
    #[error("annotation fault: {0:?}")]
    Annot(pdf_annots::Fault),
    /// Recoverable interactive form departure from `pdf-forms`.
    #[error("form fault: {0:?}")]
    Form(pdf_forms::Fault),
    /// Recoverable optional-content (`/OCProperties`) departure from
    /// `pdf-semantics`.
    #[error(transparent)]
    Semantics(#[from] pdf_semantics::Fault),
    /// A `/Pattern` resource was missing required entries (`/PatternType`,
    /// `/BBox`, `/XStep`, `/YStep`, or `/Shading`), recursive, or malformed.
    #[error("invalid or recursive /Pattern resource")]
    InvalidPattern,
    /// A `/Shading` dictionary or stream was malformed or truncated at the
    /// configured sample/triangle cap.
    #[error("invalid or truncated /Shading resource")]
    InvalidShading,
    /// A `/Shading` dictionary requested an unimplemented patch-mesh subtype
    /// (`ShadingType 6` Coons patch mesh or `ShadingType 7` tensor-product
    /// patch mesh) and was skipped (painting `/Background` if present).
    #[error("unsupported /ShadingType {shading_type} patch mesh; skipped")]
    UnsupportedShading {
        /// `/ShadingType` integer (`6` or `7`).
        shading_type: i64,
    },
    /// An `/ExtGState` `/SMask` dictionary referenced a missing, recursive, or
    /// non-Form `/G` stream or had an invalid `/S` subtype.
    #[error("invalid or recursive /ExtGState /SMask")]
    InvalidSoftMask,
    /// Nested tiling pattern, soft mask, or Type 3 glyph exceeded
    /// [`crate::Limits::max_layer_depth`] and was skipped.
    #[error("nested pattern, soft mask, or Type 3 glyph exceeded max_layer_depth")]
    LayerDepthExceeded,
    /// An image XObject or inline image failed to decode and was skipped
    /// (matching viewer recovery).
    #[error("image XObject or inline image failed to decode and was skipped")]
    SkippedImage,
    /// Decoded image dimensions exceeded the 2D rasterizer's `u16::MAX`
    /// (`65,535` px) side limit and could not be drawn.
    #[error("image dimensions {width}x{height} exceed u16 rasterizer limit; skipped")]
    OversizedImage {
        /// Decoded image width in pixels.
        width: u32,
        /// Decoded image height in pixels.
        height: u32,
    },
    /// A damaged or undecodable `/Contents` part, Type 3 `/CharProcs` stream, or
    /// annotation `/AP` stream was skipped.
    #[error("damaged content, Type 3 CharProc, or appearance stream skipped")]
    SkippedStream,
    /// A stroke dash pattern exceeded the per-path segment expansion cap and was
    /// drawn as a solid stroke.
    #[error("dense dash pattern exceeded segment cap; drawn solid")]
    DenseDash,
    /// An unrecognized `/BM` blend mode name fell back to `Normal` compositing.
    #[error("unsupported blend mode name; composited as Normal")]
    UnsupportedBlendMode,
    /// A transparency `/Group` requested non-isolated (`/I false`), knockout
    /// (`/K true`), or non-sRGB blending space (`/CS`) semantics that are
    /// approximated via isolated `sRGB` compositing.
    #[error("transparency group approximated as isolated sRGB non-knockout")]
    ApproximatedGroup,
    /// An unembedded font had no matching [`crate::FontFallback`] glyph and was
    /// rendered with built-in legibility placeholder contours (reported at most
    /// once per font per render).
    #[error("unembedded font rendered with built-in placeholder contours")]
    SubstitutedFont,
}

impl From<pdf_gfx::Fault> for Fault {
    fn from(fault: pdf_gfx::Fault) -> Self {
        Self::Gfx(fault)
    }
}

impl From<pdf_annots::Fault> for Fault {
    fn from(fault: pdf_annots::Fault) -> Self {
        Self::Annot(fault)
    }
}

impl From<pdf_forms::Fault> for Fault {
    fn from(fault: pdf_forms::Fault) -> Self {
        Self::Form(fault)
    }
}
