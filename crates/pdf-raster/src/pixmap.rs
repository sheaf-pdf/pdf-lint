//! Rendered pixel buffer ([`Pixmap`]) and pixel channel layout ([`PixelFormat`]).

use std::fmt;

use crate::error::Error;
use crate::limits::{LimitKind, Limits};

/// Channel ordering and alpha representation of a [`Pixmap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum PixelFormat {
    /// Straight (unpremultiplied) 8-bit `sRGB` `[R, G, B, A]` — ready for PNG
    /// encoding, transparent stamp overlays, and [`crate::rubric`] scoring.
    #[default]
    Rgba8,
    /// Native 8-bit `[B, G, R, A]` (straight alpha when `background` is opaque,
    /// premultiplied when transparent, matching `sheaf` GPUI textures and
    /// PDFium's `FPDFBitmap_BGRA`).
    Bgra8,
    /// Premultiplied 8-bit `sRGB` `[R, G, B, A]` — zero-copy direct output from
    /// the 2D compositor.
    Rgba8Premul,
}

/// Rendered 2D pixel buffer (`width * height * 4` bytes, top-to-bottom
/// row-major).
///
/// `Debug` prints dimensions, [`PixelFormat`], and byte length only—never pixel
/// contents.
#[derive(Clone, PartialEq, Eq)]
pub struct Pixmap {
    width: u32,
    height: u32,
    format: PixelFormat,
    data: Vec<u8>,
}

impl fmt::Debug for Pixmap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pixmap")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("byte_len", &self.data.len())
            .finish()
    }
}

impl Pixmap {
    /// Allocates a zero-initialized [`Pixmap`] after checking `limits`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidViewport`] if `width == 0` or `height == 0`, or
    /// [`Error::Limit`] if dimensions, pixel count, or byte size exceed
    /// `limits`.
    pub fn new(
        width: u32,
        height: u32,
        format: PixelFormat,
        limits: &Limits,
    ) -> Result<Self, Error> {
        let byte_len = check_pixmap_budget(width, height, limits)?;
        Ok(Self {
            width,
            height,
            format,
            data: vec![0; byte_len],
        })
    }

    /// Wraps an existing `width * height * 4` byte buffer in a [`Pixmap`] after
    /// checking `limits`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidViewport`] if dimensions are zero or `data.len()`
    /// does not equal `width * height * 4`, or [`Error::Limit`] if `limits` is
    /// exceeded.
    pub fn from_bytes(
        width: u32,
        height: u32,
        format: PixelFormat,
        data: Vec<u8>,
        limits: &Limits,
    ) -> Result<Self, Error> {
        let byte_len = check_pixmap_budget(width, height, limits)?;
        if data.len() != byte_len {
            return Err(Error::InvalidViewport("pixmap buffer length mismatch"));
        }
        Ok(Self {
            width,
            height,
            format,
            data,
        })
    }

    /// Width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Pixel channel layout.
    #[must_use]
    pub const fn format(&self) -> PixelFormat {
        self.format
    }

    /// Raw row-major pixel bytes (`width * height * 4`).
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Mutable row-major pixel bytes (`width * height * 4`).
    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Consumes the [`Pixmap`] and returns its raw pixel byte vector.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.data
    }

    /// Reads pixel `(x, y)` converted to straight-alpha `[R, G, B, A]`, or
    /// `None` if `(x, y)` is out of bounds.
    #[must_use]
    pub fn pixel_rgba8(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        let px: [u8; 4] = self.data.get(idx..idx + 4)?.try_into().ok()?;
        Some(match self.format {
            PixelFormat::Rgba8 => px,
            PixelFormat::Bgra8 => unpremul_rgba([px[2], px[1], px[0], px[3]]),
            PixelFormat::Rgba8Premul => unpremul_rgba(px),
        })
    }

    /// Returns a straight-alpha `RGBA8` copy of all pixels (`width * height * 4`
    /// bytes), regardless of [`Self::format`].
    #[must_use]
    pub fn to_rgba8_vec(&self) -> Vec<u8> {
        match self.format {
            PixelFormat::Rgba8 => self.data.clone(),
            PixelFormat::Bgra8 => {
                let mut out = self.data.clone();
                for px in out.as_chunks_mut::<4>().0 {
                    *px = unpremul_rgba([px[2], px[1], px[0], px[3]]);
                }
                out
            }
            PixelFormat::Rgba8Premul => {
                let mut out = self.data.clone();
                for px in out.as_chunks_mut::<4>().0 {
                    *px = unpremul_rgba(*px);
                }
                out
            }
        }
    }
}

pub(crate) fn unpremul_rgba([r, g, b, a]: [u8; 4]) -> [u8; 4] {
    if a == 0 {
        [0, 0, 0, 0]
    } else if a == 255 {
        [r, g, b, 255]
    } else {
        let un = |c: u8| {
            let v = (u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a);
            v.min(255) as u8
        };
        [un(r), un(g), un(b), a]
    }
}

pub(crate) fn check_pixmap_budget(
    width: u32,
    height: u32,
    limits: &Limits,
) -> Result<usize, Error> {
    if width == 0 || height == 0 {
        return Err(Error::InvalidViewport(
            "viewport width and height must be >= 1",
        ));
    }
    let max_w = limits.max_width.min(u32::from(u16::MAX));
    let max_h = limits.max_height.min(u32::from(u16::MAX));
    if width > max_w || height > max_h {
        return Err(Error::Limit(LimitKind::Dimensions));
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > limits.max_pixels {
        return Err(Error::Limit(LimitKind::Pixels));
    }
    let bytes_u64 = pixels
        .checked_mul(4)
        .ok_or(Error::Limit(LimitKind::PixmapBytes))?;
    let bytes = usize::try_from(bytes_u64).map_err(|_| Error::Limit(LimitKind::PixmapBytes))?;
    if bytes > limits.max_pixmap_bytes {
        return Err(Error::Limit(LimitKind::PixmapBytes));
    }
    Ok(bytes)
}
