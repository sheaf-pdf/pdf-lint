//! Unpremultiplied straight-alpha sRGB `RGBA8` image buffer and compositing helpers.

#![forbid(unsafe_code)]

use std::fmt;

use crate::ffi::Error;

/// Maximum pixel count permitted in a single [`RgbaImage`] (64 megapixels = 256 MiB).
pub const MAX_PIXELS: usize = 64 * 1024 * 1024;

/// Owned straight-alpha sRGB `RGBA8` bitmap (`4` bytes per pixel: `[R, G, B, A]`).
///
/// `Debug` prints dimensions and byte count without dumping raw pixel contents.
#[derive(Clone, PartialEq, Eq)]
pub struct RgbaImage {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl fmt::Debug for RgbaImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RgbaImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("byte_len", &self.pixels.len())
            .finish()
    }
}

impl RgbaImage {
    /// Allocates a `width x height` image filled with `rgba`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidDimensions`] if `width == 0`, `height == 0`, or `width * height`
    /// exceeds [`MAX_PIXELS`].
    pub fn new(width: u32, height: u32, rgba: [u8; 4]) -> Result<RgbaImage, Error> {
        let count = checked_pixel_count(width, height)?;
        let mut pixels = Vec::with_capacity(count * 4);
        for _ in 0..count {
            pixels.extend_from_slice(&rgba);
        }
        Ok(RgbaImage {
            width,
            height,
            pixels,
        })
    }

    /// Wraps an existing `RGBA8` buffer of length `width * height * 4`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidDimensions`] if dimensions are zero/oversized or `pixels.len()`
    /// does not equal `width * height * 4`.
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Result<RgbaImage, Error> {
        let count = checked_pixel_count(width, height)?;
        if pixels.len() != count * 4 {
            return Err(Error::InvalidDimensions { width, height });
        }
        Ok(RgbaImage {
            width,
            height,
            pixels,
        })
    }

    /// Image width in pixels (`>= 1`).
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Image height in pixels (`>= 1`).
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Underlying `RGBA8` bytes in row-major top-to-bottom order.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.pixels
    }

    /// Mutable `RGBA8` bytes in row-major top-to-bottom order.
    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    /// Consumes the image and returns its `RGBA8` byte buffer.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.pixels
    }

    /// Returns `[R, G, B, A]` at `(x, y)`, or `None` if out of bounds.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        let s = &self.pixels[idx..idx + 4];
        Some([s[0], s[1], s[2], s[3]])
    }

    /// Sets `[R, G, B, A]` at `(x, y)` if within bounds.
    pub fn set_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        self.pixels[idx..idx + 4].copy_from_slice(&rgba);
    }

    /// Fills axis-aligned rectangle `[x..x+w, y..y+h]` (clipped to image bounds) with `rgba`.
    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, rgba: [u8; 4]) {
        let x0 = x.min(self.width);
        let y0 = y.min(self.height);
        let x1 = x.saturating_add(w).min(self.width);
        let y1 = y.saturating_add(h).min(self.height);
        for py in y0..y1 {
            let row_start = (py as usize) * (self.width as usize);
            for px in x0..x1 {
                let idx = (row_start + (px as usize)) * 4;
                self.pixels[idx..idx + 4].copy_from_slice(&rgba);
            }
        }
    }

    /// Returns `true` if every pixel has `alpha == 255`.
    #[must_use]
    pub fn is_opaque(&self) -> bool {
        self.pixels.as_chunks::<4>().0.iter().all(|px| px[3] == 255)
    }

    /// Composites straight-alpha `RGBA8` over a solid opaque sRGB `backdrop` (`[R, G, B]`).
    #[must_use]
    pub fn composite_solid(&self, backdrop: [u8; 3]) -> Vec<[u8; 3]> {
        self.pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&px| blend_over(px, backdrop))
            .collect()
    }

    /// Composites straight-alpha `RGBA8` over an `cell x cell` sRGB checkerboard (`light` / `dark`).
    #[must_use]
    pub fn composite_checkerboard(&self, cell: u32, light: [u8; 3], dark: [u8; 3]) -> Vec<[u8; 3]> {
        let cell = cell.max(1);
        let w = self.width as usize;
        let mut out = Vec::with_capacity(w * (self.height as usize));
        for y in 0..self.height {
            let cy = (y / cell) & 1;
            for x in 0..self.width {
                let cx = (x / cell) & 1;
                let bg = if (cx ^ cy) == 0 { light } else { dark };
                let idx = ((y as usize) * w + (x as usize)) * 4;
                let px = &self.pixels[idx..idx + 4];
                out.push(blend_over([px[0], px[1], px[2], px[3]], bg));
            }
        }
        out
    }

    /// Encodes the image (composited over white if translucent) as a binary `P6` PPM byte buffer
    /// for quick visual inspection without external image-encoding crates.
    #[must_use]
    pub fn to_ppm(&self) -> Vec<u8> {
        let header = format!("P6\n{} {}\n255\n", self.width, self.height);
        let rgb = self.composite_solid([255, 255, 255]);
        let mut out = Vec::with_capacity(header.len() + rgb.len() * 3);
        out.extend_from_slice(header.as_bytes());
        for px in rgb {
            out.extend_from_slice(&px);
        }
        out
    }
}

pub(crate) fn checked_pixel_count(width: u32, height: u32) -> Result<usize, Error> {
    if width == 0 || height == 0 {
        return Err(Error::InvalidDimensions { width, height });
    }
    let count = (width as usize)
        .checked_mul(height as usize)
        .filter(|&n| n <= MAX_PIXELS)
        .ok_or(Error::InvalidDimensions { width, height })?;
    let _bytes = count
        .checked_mul(4)
        .ok_or(Error::InvalidDimensions { width, height })?;
    Ok(count)
}

#[inline]
fn blend_over(rgba: [u8; 4], bg: [u8; 3]) -> [u8; 3] {
    let a = u32::from(rgba[3]);
    if a == 255 {
        return [rgba[0], rgba[1], rgba[2]];
    }
    if a == 0 {
        return bg;
    }
    let inv = 255 - a;
    let ch = |fg: u8, b: u8| -> u8 { ((u32::from(fg) * a + u32::from(b) * inv + 127) / 255) as u8 };
    [ch(rgba[0], bg[0]), ch(rgba[1], bg[1]), ch(rgba[2], bg[2])]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_pixel_payload() {
        let img = RgbaImage::new(2, 2, *b"SECR").unwrap();
        let dbg = format!("{img:?}");
        assert!(!dbg.contains("SECR"), "{dbg}");
        assert!(dbg.contains("byte_len: 16"), "{dbg}");
    }

    #[test]
    fn bounds_and_compositing_behave_deterministically() {
        assert!(RgbaImage::new(0, 10, [0; 4]).is_err());
        assert!(RgbaImage::new(10, 0, [0; 4]).is_err());
        assert!(RgbaImage::new(100_000, 100_000, [0; 4]).is_err());

        let mut img = RgbaImage::new(4, 4, [200, 100, 50, 128]).unwrap();
        assert!(!img.is_opaque());
        img.fill_rect(2, 2, 10, 10, [10, 20, 30, 255]);
        assert_eq!(img.pixel(3, 3), Some([10, 20, 30, 255]));
        assert_eq!(img.pixel(4, 4), None);

        let over_white = img.composite_solid([255, 255, 255]);
        assert_eq!(over_white[0], [227, 177, 152]);
        assert_eq!(over_white[15], [10, 20, 30]);
    }
}
