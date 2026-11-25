//! Tier-1 native-sample buffers (`SampleData`, `RawImage`), sample unpacking, and in-place
//! rectangular pixel scrubbing.

use std::borrow::Cow;
use std::fmt;

use pdf_color::PreparedSpace;

use crate::error::Error;
use crate::header::{ImageCodec, ImageMeta, check_dimensions, check_sample_budget};
use crate::limits::Limits;

/// Native sample storage for a decoded PDF image before `/Decode` mapping or color conversion.
///
/// `Debug` prints storage geometry and byte/sample counts only—never raw pixel values.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SampleData {
    /// Sub-byte packed samples (`1`, `2`, or `4` bits per component), MSB-first within each byte,
    /// padded at the end of every scanline to `stride` bytes.
    Packed {
        /// Bits per component (`1`, `2`, or `4`).
        bpc: u8,
        /// Bytes per scanline (`(width * components * bpc).div_ceil(8)`).
        stride: usize,
        /// Packed scanlines of length `stride * height`.
        data: Vec<u8>,
    },
    /// Interleaved 8-bit samples of length `width * height * components`.
    U8(Vec<u8>),
    /// Interleaved 16-bit samples of length `width * height * components`.
    U16(Vec<u16>),
}

impl fmt::Debug for SampleData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SampleData::Packed { bpc, stride, data } => f
                .debug_struct("Packed")
                .field("bpc", bpc)
                .field("stride", stride)
                .field("byte_len", &data.len())
                .finish(),
            SampleData::U8(data) => f
                .debug_struct("U8")
                .field("sample_count", &data.len())
                .finish(),
            SampleData::U16(data) => f
                .debug_struct("U16")
                .field("sample_count", &data.len())
                .finish(),
        }
    }
}

impl SampleData {
    /// Byte size occupied by this sample buffer in memory.
    #[must_use]
    pub const fn byte_len(&self) -> usize {
        match self {
            SampleData::Packed { data, .. } | SampleData::U8(data) => data.len(),
            SampleData::U16(data) => data.len() * 2,
        }
    }
}

/// Tier-1 decoded image holding samples in the image's native color space and bit depth.
///
/// Use [`RawImage::to_rgba8`](Self::to_rgba8) to evaluate colors through [`pdf_color`] and
/// composite `/ImageMask`, `/Mask`, and `/SMask` into straight-alpha `RGBA8`, or use
/// [`RawImage::fill_rect_raw`](Self::fill_rect_raw) and [`encode_raw_flate`](crate::encode_raw_flate)
/// to burn redaction boxes in native sample space without converting untouched pixels.
///
/// `Debug` prints metadata and byte counts only—never sample bytes.
#[derive(Clone)]
pub struct RawImage {
    pub(crate) meta: ImageMeta,
    pub(crate) codec: ImageCodec,
    pub(crate) samples: SampleData,
    pub(crate) embedded_alpha: Option<Vec<u8>>,
    pub(crate) prepared_space: Option<PreparedSpace>,
}

impl fmt::Debug for RawImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawImage")
            .field("meta", &self.meta)
            .field("codec", &self.codec)
            .field("samples", &self.samples)
            .field(
                "embedded_alpha_bytes",
                &self.embedded_alpha.as_ref().map(Vec::len),
            )
            .field("prepared_space", &self.prepared_space)
            .finish()
    }
}

impl RawImage {
    /// Constructs a [`RawImage`] from validated parts under `limits`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Limit`] if dimensions or buffer sizes exceed `limits`, or
    /// [`Error::InvalidBitsPerComponent`] if `meta.bits_per_component` does not match `samples`.
    pub fn from_parts(
        meta: ImageMeta,
        codec: ImageCodec,
        samples: SampleData,
        embedded_alpha: Option<Vec<u8>>,
        prepared_space: Option<PreparedSpace>,
        limits: &Limits,
    ) -> Result<Self, Error> {
        let pixels = check_dimensions(meta.width, meta.height, limits)?;
        let comps = u64::from(meta.components);
        match &samples {
            SampleData::Packed { bpc, stride, data } => {
                if !matches!(*bpc, 1 | 2 | 4) || *bpc != meta.bits_per_component {
                    return Err(Error::InvalidBitsPerComponent {
                        bpc: i64::from(meta.bits_per_component),
                    });
                }
                let expected_stride =
                    ((u64::from(meta.width) * comps * u64::from(*bpc)).div_ceil(8)) as usize;
                let expected_len = check_sample_budget(
                    (expected_stride as u64).saturating_mul(u64::from(meta.height)),
                    limits,
                )?;
                if *stride != expected_stride || data.len() != expected_len {
                    return Err(Error::NotAnImage);
                }
            }
            SampleData::U8(data) => {
                if meta.bits_per_component != 8 {
                    return Err(Error::InvalidBitsPerComponent {
                        bpc: i64::from(meta.bits_per_component),
                    });
                }
                let expected = check_sample_budget(pixels.saturating_mul(comps), limits)?;
                if data.len() != expected {
                    return Err(Error::NotAnImage);
                }
            }
            SampleData::U16(data) => {
                if meta.bits_per_component != 16 {
                    return Err(Error::InvalidBitsPerComponent {
                        bpc: i64::from(meta.bits_per_component),
                    });
                }
                let expected_samples =
                    check_sample_budget(pixels.saturating_mul(comps).saturating_mul(2), limits)?
                        / 2;
                if data.len() != expected_samples {
                    return Err(Error::NotAnImage);
                }
            }
        }
        if let Some(alpha) = &embedded_alpha {
            let expected_alpha = check_sample_budget(pixels, limits)?;
            if alpha.len() != expected_alpha {
                return Err(Error::NotAnImage);
            }
        }
        Ok(RawImage {
            meta,
            codec,
            samples,
            embedded_alpha,
            prepared_space,
        })
    }

    /// Intrinsic image width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.meta.width
    }

    /// Intrinsic image height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.meta.height
    }

    /// Bits per sample component (`1, 2, 4, 8, 16`).
    #[must_use]
    pub const fn bits_per_component(&self) -> u8 {
        self.meta.bits_per_component
    }

    /// Number of color components per pixel (`1` for `/ImageMask true` stencils).
    #[must_use]
    pub const fn components(&self) -> u8 {
        self.meta.components
    }

    /// Codec that compressed this image's stream.
    #[must_use]
    pub const fn codec(&self) -> ImageCodec {
        self.codec
    }

    /// Parsed dictionary metadata (`/Decode`, `/Mask`, `/SMask`, `/Interpolate`, etc.).
    #[must_use]
    pub const fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    /// Native sample buffer.
    #[must_use]
    pub const fn samples(&self) -> &SampleData {
        &self.samples
    }

    /// Embedded 8-bit straight-alpha plane extracted from `JPXDecode` when `/SMaskInData` is `1`
    /// or `2`, if present.
    #[must_use]
    pub fn embedded_alpha(&self) -> Option<&[u8]> {
        self.embedded_alpha.as_deref()
    }

    /// Compiled [`PreparedSpace`] for this image's color space (`None` for `/ImageMask true`).
    #[must_use]
    pub const fn prepared_space(&self) -> Option<&PreparedSpace> {
        self.prepared_space.as_ref()
    }

    /// Reads the raw pre-`/Decode` integer sample values (`0 ..= (1 << bpc) - 1`) at `(x, y)` into
    /// `out` (`out.len() >= self.components()`).
    ///
    /// Returns `false` if `(x, y)` is out of bounds or `out` is too short.
    pub fn pixel_raw(&self, x: u32, y: u32, out: &mut [u16]) -> bool {
        let comps = usize::from(self.meta.components);
        if x >= self.meta.width || y >= self.meta.height || out.len() < comps {
            return false;
        }
        let x_usize = x as usize;
        let y_usize = y as usize;
        let w_usize = self.meta.width as usize;
        match &self.samples {
            SampleData::Packed { bpc, stride, data } => {
                let row = &data[y_usize * stride..(y_usize + 1) * stride];
                let bpc_usize = usize::from(*bpc);
                let mask = (1_u16 << bpc_usize) - 1;
                let base_sample = x_usize * comps;
                for (c, slot) in out[..comps].iter_mut().enumerate() {
                    let bit_index = (base_sample + c) * bpc_usize;
                    let byte = row[bit_index >> 3];
                    let shift = 8 - bpc_usize - (bit_index & 7);
                    *slot = (u16::from(byte) >> shift) & mask;
                }
            }
            SampleData::U8(data) => {
                let idx = (y_usize * w_usize + x_usize) * comps;
                for (c, slot) in out[..comps].iter_mut().enumerate() {
                    *slot = u16::from(data[idx + c]);
                }
            }
            SampleData::U16(data) => {
                let idx = (y_usize * w_usize + x_usize) * comps;
                out[..comps].copy_from_slice(&data[idx..idx + comps]);
            }
        }
        true
    }

    /// Reads the `/Decode`-mapped floating-point component values at `(x, y)` into `out`
    /// (`out.len() >= self.components()`).
    ///
    /// Applies ISO 32000-2 §8.9.5.2 linear interpolation:
    /// `D_min + sample * (D_max - D_min) / ((1 << bpc) - 1)`.
    pub fn pixel_decoded(&self, x: u32, y: u32, out: &mut [f32]) -> bool {
        let comps = usize::from(self.meta.components);
        if out.len() < comps {
            return false;
        }
        let mut raw = [0_u16; 32];
        if comps > raw.len() || !self.pixel_raw(x, y, &mut raw[..comps]) {
            return false;
        }
        let denom = ((1_u32 << self.meta.bits_per_component) - 1) as f64;
        for c in 0..comps {
            let (d_min, d_max) = self.meta.decode[c];
            let mapped = d_min + (f64::from(raw[c]) / denom) * (d_max - d_min);
            out[c] = mapped as f32;
        }
        true
    }

    /// Unpacks or scales all samples to 8-bit (`0..=255`) interleaved `width * height * components`
    /// bytes (before `/Decode`).
    #[must_use]
    pub fn unpack_u8(&self) -> Vec<u8> {
        let comps = usize::from(self.meta.components);
        let total = (self.meta.width as usize) * (self.meta.height as usize) * comps;
        match &self.samples {
            SampleData::U8(data) => data.clone(),
            SampleData::U16(data) => data
                .iter()
                .map(|&v| ((u32::from(v) * 255 + 32_767) / 65_535) as u8)
                .collect(),
            SampleData::Packed { bpc, stride, data } => {
                let mut out = Vec::with_capacity(total);
                let row_samples = (self.meta.width as usize) * comps;
                let bpc_usize = usize::from(*bpc);
                let mask = (1_u32 << bpc_usize) - 1;
                for y in 0..(self.meta.height as usize) {
                    let row = &data[y * stride..(y + 1) * stride];
                    for s in 0..row_samples {
                        let bit_index = s * bpc_usize;
                        let v = u32::from(row[bit_index >> 3] >> (8 - bpc_usize - (bit_index & 7)))
                            & mask;
                        out.push(((v * 255 + mask / 2) / mask) as u8);
                    }
                }
                out
            }
        }
    }

    /// Returns big-endian PDF stream sample bytes suitable for raw or `FlateDecode` encoding.
    #[must_use]
    pub fn stream_bytes(&self) -> Cow<'_, [u8]> {
        match &self.samples {
            SampleData::Packed { data, .. } | SampleData::U8(data) => Cow::Borrowed(data),
            SampleData::U16(data) => {
                let mut bytes = Vec::with_capacity(data.len() * 2);
                for &v in data {
                    bytes.extend_from_slice(&v.to_be_bytes());
                }
                Cow::Owned(bytes)
            }
        }
    }

    /// Overwrites every pixel in `[x .. x + w) × [y .. y + h)` in-place with raw sample values
    /// `fill` (`fill.len() >= self.components()`), clamped to the image bounds and bit depth.
    ///
    /// Designed for redaction pixel scrubbing: pixels outside the rectangle remain bit-for-bit
    /// identical in their native color space and bit depth (`1, 2, 4, 8, 16` bpc). If an embedded
    /// JPX alpha plane is present, alpha inside the scrubbed rectangle is set to `255` (opaque).
    pub fn fill_rect_raw(&mut self, x: u32, y: u32, w: u32, h: u32, fill: &[u16]) {
        let comps = usize::from(self.meta.components);
        if fill.len() < comps || w == 0 || h == 0 {
            return;
        }
        let x0 = x.min(self.meta.width) as usize;
        let y0 = y.min(self.meta.height) as usize;
        let x1 = x.saturating_add(w).min(self.meta.width) as usize;
        let y1 = y.saturating_add(h).min(self.meta.height) as usize;
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let max_sample = ((1_u32 << self.meta.bits_per_component) - 1) as u16;
        let width_usize = self.meta.width as usize;

        match &mut self.samples {
            SampleData::Packed { bpc, stride, data } => {
                let bpc_usize = usize::from(*bpc);
                let mask = (1_u8 << bpc_usize) - 1;
                for row_y in y0..y1 {
                    let row = &mut data[row_y * *stride..(row_y + 1) * *stride];
                    for px in x0..x1 {
                        let base_sample = px * comps;
                        for (c, &val) in fill[..comps].iter().enumerate() {
                            let clamped = (val.min(max_sample) as u8) & mask;
                            let bit_index = (base_sample + c) * bpc_usize;
                            let byte_idx = bit_index >> 3;
                            let shift = 8 - bpc_usize - (bit_index & 7);
                            row[byte_idx] = (row[byte_idx] & !(mask << shift)) | (clamped << shift);
                        }
                    }
                }
            }
            SampleData::U8(data) => {
                for row_y in y0..y1 {
                    for px in x0..x1 {
                        let base = (row_y * width_usize + px) * comps;
                        for (c, &val) in fill[..comps].iter().enumerate() {
                            data[base + c] = val.min(255) as u8;
                        }
                    }
                }
            }
            SampleData::U16(data) => {
                for row_y in y0..y1 {
                    for px in x0..x1 {
                        let base = (row_y * width_usize + px) * comps;
                        data[base..base + comps].copy_from_slice(&fill[..comps]);
                    }
                }
            }
        }

        if let Some(alpha) = &mut self.embedded_alpha {
            for row_y in y0..y1 {
                let row_start = row_y * width_usize;
                alpha[row_start + x0..row_start + x1].fill(255);
            }
        }
    }
}

/// Decodes raw packed sample bytes (`1, 2, 4, 8, 16` bpc) into [`SampleData`], padding short
/// streams (using `1` bits for 1-bit `/ImageMask` / `DeviceGray` white default? Wait: ISO/PDFium
/// pads missing stream bytes with `0` bytes!) and reporting [`Fault::ShortStream`](crate::Fault::ShortStream)
/// or [`Fault::TrailingBytes`](crate::Fault::TrailingBytes).
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_raw_samples<S>(
    bytes: &[u8],
    width: u32,
    height: u32,
    components: u8,
    bpc: u8,
    limits: &Limits,
    place: pdf_objects::fault::Place,
    sink: &mut S,
) -> Result<SampleData, Error>
where
    S: pdf_objects::fault::Sink<crate::Fault> + ?Sized,
{
    let pixels = check_dimensions(width, height, limits)?;
    let comps = u64::from(components);
    match bpc {
        1 | 2 | 4 => {
            let stride = ((u64::from(width) * comps * u64::from(bpc)).div_ceil(8)) as usize;
            let needed = check_sample_budget((stride as u64) * u64::from(height), limits)?;
            let mut buf = vec![0_u8; needed];
            if bytes.len() < needed {
                sink.fault(place, crate::Fault::ShortStream);
                buf[..bytes.len()].copy_from_slice(bytes);
            } else {
                if bytes.len() > needed {
                    sink.fault(place, crate::Fault::TrailingBytes);
                }
                buf.copy_from_slice(&bytes[..needed]);
            }
            Ok(SampleData::Packed {
                bpc,
                stride,
                data: buf,
            })
        }
        8 => {
            let needed = check_sample_budget(pixels.saturating_mul(comps), limits)?;
            let mut buf = vec![0_u8; needed];
            if bytes.len() < needed {
                sink.fault(place, crate::Fault::ShortStream);
                buf[..bytes.len()].copy_from_slice(bytes);
            } else {
                if bytes.len() > needed {
                    sink.fault(place, crate::Fault::TrailingBytes);
                }
                buf.copy_from_slice(&bytes[..needed]);
            }
            Ok(SampleData::U8(buf))
        }
        16 => {
            let needed_bytes =
                check_sample_budget(pixels.saturating_mul(comps).saturating_mul(2), limits)?;
            let sample_count = needed_bytes / 2;
            let mut buf = vec![0_u16; sample_count];
            let usable_bytes = bytes.len().min(needed_bytes);
            if bytes.len() < needed_bytes {
                sink.fault(place, crate::Fault::ShortStream);
            } else if bytes.len() > needed_bytes {
                sink.fault(place, crate::Fault::TrailingBytes);
            }
            let (pairs, rem) = bytes[..usable_bytes].as_chunks::<2>();
            for (slot, &[hi, lo]) in buf.iter_mut().zip(pairs) {
                *slot = u16::from_be_bytes([hi, lo]);
            }
            if let &[hi] = rem {
                buf[pairs.len()] = u16::from_be_bytes([hi, 0]);
            }
            Ok(SampleData::U16(buf))
        }
        other => Err(Error::InvalidBitsPerComponent {
            bpc: i64::from(other),
        }),
    }
}
