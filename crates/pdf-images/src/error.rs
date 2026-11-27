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
    /// Image uses a non-device `/ColorSpace` (such as `Indexed`, `ICCBased`, `Separation`,
    /// `DeviceN`, or CIE spaces) and no PDF `/ColorSpace` object was supplied to [`encode_raw_flate`](crate::encode_raw_flate).
    #[error("non-device image color space requires an explicit /ColorSpace object")]
    MissingColorSpace,
    /// Flate/predictor encoding rejected the row layout.
    #[error(transparent)]
    Flate(#[from] pdf_filters::Unencodable),
    /// A JPEG file's bytes can't be an Image XObject as they are
    /// ([`wrap_jpeg`](crate::wrap_jpeg)).
    #[error("the JPEG can't be an image XObject as it is: {0}")]
    Jpeg(#[from] JpegRefused),
}

/// Why [`wrap_jpeg`](crate::wrap_jpeg) refused a file's bytes: they aren't a
/// JPEG, or not one a `/DCTDecode` reader decodes as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum JpegRefused {
    /// The bytes don't start with a JPEG's start-of-image marker (`FF D8`).
    #[error("it doesn't start with a JPEG's start-of-image marker")]
    NoStart,
    /// A header segment before the first scan is cut short, has a length
    /// that can't be, or isn't where a marker should be.
    #[error("its headers don't read")]
    Malformed,
    /// The first scan comes before any frame header.
    #[error("it has no frame header before its first scan")]
    NoFrame,
    /// The image ends before any scan.
    #[error("it has no scan")]
    NoScan,
    /// A process other than baseline, extended sequential or progressive DCT
    /// with Huffman coding: lossless, differential (hierarchical) or
    /// arithmetic-coded, the reserved `JPG` extension, or a second frame.
    /// The marker's code is given (`0xC3` for lossless, `0xC9` for
    /// arithmetic-coded sequential, `0xDE` for a hierarchy).
    #[error("its process (marker 0x{0:02X}) isn't one a /DCTDecode reader decodes")]
    Process(u8),
    /// Samples of a precision other than 8 bits.
    #[error("its samples are {0}-bit, not 8-bit")]
    Precision(u8),
    /// A number of components other than one, three or four.
    #[error("it has {0} components, not 1, 3 or 4")]
    Components(u8),
}

/// Why an in-place raw sample scrub ([`RawImage::fill_rect_raw`](crate::RawImage::fill_rect_raw))
/// was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum ScrubError {
    /// `fill.len()` is shorter than the image's component count.
    #[error("fill slice length ({actual}) is shorter than image components ({expected})")]
    InsufficientComponents {
        /// Provided `fill.len()`.
        actual: usize,
        /// Required `self.components()`.
        expected: u8,
    },
    /// `fill` matches the image's `/Mask` color-key range, which would make the scrubbed
    /// rectangle transparent instead of opaque.
    #[error("fill sample values fall inside the image's /Mask color-key transparency range")]
    ColorKeyTransparentFill,
}

fn lossy(bytes: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}
