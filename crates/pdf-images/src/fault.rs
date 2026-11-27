//! Recoverable ISO 32000-2 departures reported during image decoding.

/// A recoverable specification departure reported to a [`Sink`](pdf_objects::fault::Sink) while
/// decoding an image.
///
/// Passing `&mut ()` skips fault collection at zero cost while returning the exact same decoded
/// [`RawImage`](crate::RawImage) or [`RgbaImage`](crate::RgbaImage) as an active sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Fault {
    /// Uncompressed or standard-filtered sample stream ended before `row_bytes * height` and was
    /// zero-padded.
    #[error("image sample stream ended short of expected row bytes")]
    ShortStream,
    /// Uncompressed or standard-filtered sample stream contained trailing bytes past
    /// `row_bytes * height`.
    #[error("image sample stream has trailing bytes past expected row bytes")]
    TrailingBytes,
    /// Codec header dimensions (JPEG SOF, JBIG2 page info, or JPX SIZ) differed from dictionary
    /// `/Width` or `/Height`.
    #[error("codec dimensions differ from image dictionary /Width or /Height")]
    DimensionMismatch,
    /// Codec channel count differed from the `/ColorSpace` component count.
    #[error("codec component count differs from /ColorSpace component count")]
    ComponentMismatch,
    /// `/Decode` array was not an array of `2 * N` finite numbers; default `/Decode` intervals were
    /// substituted.
    #[error("malformed /Decode array; default decode range used")]
    InvalidDecode,
    /// `/ImageMask true` dictionary also specified `/ColorSpace` (ignored per §8.9.6.2).
    #[error("/ImageMask dictionary specifies /ColorSpace")]
    StencilHasColorSpace,
    /// `/Mask` stream or color-key array was malformed or unreadable and was ignored (leaving
    /// pixels opaque).
    #[error("invalid or unreadable /Mask; ignored")]
    InvalidMask,
    /// `/SMask` stream was malformed or unreadable and was ignored (leaving pixels opaque).
    #[error("invalid or unreadable /SMask; ignored")]
    InvalidSMask,
    /// `/Matte` array on `/SMask` was not `N` finite numbers for the parent color space and was
    /// ignored.
    #[error("invalid /Matte array on /SMask; ignored")]
    InvalidMatte,
    /// `CCITTFaxDecode` stream ended early after decoding at least one row; remaining rows were
    /// filled white.
    #[error("truncated CCITTFaxDecode stream; remaining rows filled white")]
    TruncatedFax,
    /// `CCITTFaxDecode` or `JBIG2Decode` dictionary specified `/BitsPerComponent` other than `1`
    /// and was forced to `1` under `Rules::Pdfium`.
    #[error("invalid /BitsPerComponent for bilevel codec; forced to 1")]
    InvalidBitsPerComponent,
    /// Recoverable departure while parsing or preparing `/ColorSpace`.
    #[error(transparent)]
    Color(#[from] pdf_color::Fault),
    /// Recoverable departure while running leading standard filters on an inline image.
    #[error(transparent)]
    Filter(#[from] pdf_filters::Fault),
}
