//! Resource caps bounding untrusted PDF image decoding and mask recursion.

/// Resource limits enforced before and during image decoding.
///
/// Every dimension, pixel count, and buffer size is checked **before** allocating pixel buffers or
/// running image codecs so adversarial headers cannot exhaust memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Maximum allowed image `/Width` in pixels (`32_768` by default).
    pub max_width: u32,
    /// Maximum allowed image `/Height` in pixels (`32_768` by default).
    pub max_height: u32,
    /// Maximum total pixel count `width * height` (`134_217_728`, i.e., 128 megapixels by default).
    pub max_pixels: u64,
    /// Maximum unpacked sample or RGBA8 buffer size in bytes (`256 MiB` by default).
    pub max_sample_bytes: usize,
    /// Maximum byte length read from `Streams::decoded_standard` or subsidiary streams such as
    /// `/JBIG2Globals` (`64 MiB` by default).
    pub max_encoded_bytes: usize,
    /// Maximum indirect recursion depth when decoding `/Mask` and `/SMask` streams (`4` by default).
    pub mask_depth: u8,
    /// Resource limits forwarded to [`pdf_color`] when parsing and evaluating `/ColorSpace`.
    pub color: pdf_color::Limits,
    /// Resource limits forwarded to [`pdf_content_streams`] when expanding inline image parameters.
    pub content: pdf_content_streams::Limits,
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Limits {
    /// Default resource caps suitable for untrusted PDF documents.
    pub const DEFAULT: Limits = Limits {
        max_width: 32_768,
        max_height: 32_768,
        max_pixels: 128 * 1024 * 1024,
        max_sample_bytes: 256 * 1024 * 1024,
        max_encoded_bytes: 64 * 1024 * 1024,
        mask_depth: 4,
        color: pdf_color::Limits::DEFAULT,
        content: pdf_content_streams::Limits::DEFAULT,
    };
}

/// Which cap of [`Limits`] refused an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LimitKind {
    /// `/Width` exceeded [`Limits::max_width`].
    Width,
    /// `/Height` exceeded [`Limits::max_height`].
    Height,
    /// `width * height` exceeded [`Limits::max_pixels`].
    Pixels,
    /// Decoded sample or RGBA buffer size exceeded [`Limits::max_sample_bytes`].
    SampleBytes,
    /// Encoded or standard-filtered stream size exceeded [`Limits::max_encoded_bytes`].
    EncodedBytes,
    /// Subsidiary `/Mask` or `/SMask` nesting exceeded [`Limits::mask_depth`].
    MaskDepth,
}
