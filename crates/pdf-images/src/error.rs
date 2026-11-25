//! Hard errors and encoding errors returned by `pdf-images`.

use std::borrow::Cow;

use pdf_objects::Unreadable;

use crate::header::ImageCodec;
use crate::limits::LimitKind;

/// Why an Image XObject or inline image could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Underlying object or stream read failed (locked file, reader cap, I/O error, or damaged
    /// leading filter).
    #[error(transparent)]
    Unreadable(#[from] Unreadable),
    /// Target object is absent, is not an Image XObject stream, or lacks positive `/Width` or
    /// `/Height`.
    #[error("not a valid PDF image stream or inline image")]
    NotAnImage,
    /// Image dimensions, byte budget, or mask depth exceeded [`Limits`](crate::Limits).
    #[error("image {kind:?} ({value}) exceeds limit ({max})")]
    Limit {
        /// Which cap was exceeded.
        kind: LimitKind,
        /// Requested value.
        value: u64,
        /// Configured cap.
        max: u64,
    },
    /// Non-stencil image has no usable `/ColorSpace` (and is not a `JPXDecode` stream carrying its
    /// own color specification).
    #[error("image lacks a usable /ColorSpace")]
    MissingColorSpace,
    /// `/BitsPerComponent` is missing or not one of `1, 2, 4, 8, 16` (or `1` for `/ImageMask`).
    #[error("unsupported /BitsPerComponent ({bpc})")]
    InvalidBitsPerComponent {
        /// Value found in the image dictionary, or `0` if absent/non-integer.
        bpc: i64,
    },
    /// Unsupported codec filter or trailing filter after an image codec.
    #[error("unsupported image filter /{}", lossy(.filter))]
    UnsupportedFilter {
        /// Raw filter name.
        filter: Vec<u8>,
    },
    /// Unrecoverable codec failure while decoding image samples.
    #[error("{codec:?} decode failed: {detail}")]
    Codec {
        /// Which codec failed.
        codec: ImageCodec,
        /// Diagnostic message (never contains raw stream bytes).
        detail: String,
    },
}

/// Why clean image re-encoding failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum EncodeError {
    /// Dimensions are zero or exceed the format's maximum (e.g. `65_535` for baseline JPEG).
    #[error("invalid image dimensions ({width}x{height}) for encoding")]
    Dimensions {
        /// Requested width.
        width: u32,
        /// Requested height.
        height: u32,
    },
    /// Provided sample buffer length does not match `width * height * components`.
    #[error("pixel buffer length ({actual}) does not match expected ({expected})")]
    BufferLength {
        /// Provided byte length.
        actual: usize,
        /// Required byte length.
        expected: usize,
    },
    /// Flate/predictor encoding rejected the row layout.
    #[error(transparent)]
    Flate(#[from] pdf_filters::Unencodable),
}

fn lossy(bytes: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}
