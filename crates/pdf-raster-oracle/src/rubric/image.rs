//! Straight-alpha sRGB [`RgbaImage`] re-export and compositing helpers.

use pdf_raster::Pixmap;
use pdf_raster::pdf_images::Limits as ImageLimits;
pub use pdf_raster::pdf_images::RgbaImage;

use super::Error;

/// Maximum pixel count permitted in a single [`RgbaImage`] scored by the rubric
/// (64 megapixels = 256 MiB).
pub const MAX_PIXELS: usize = 64 * 1024 * 1024;

const fn rubric_image_limits() -> ImageLimits {
    let mut lim = ImageLimits::DEFAULT;
    lim.max_width = 65_535;
    lim.max_height = 65_535;
    lim.max_pixels = MAX_PIXELS as u64;
    lim.max_sample_bytes = MAX_PIXELS * 4;
    lim
}

const RUBRIC_IMAGE_LIMITS: ImageLimits = rubric_image_limits();

/// Rubric construction, [`Pixmap`] conversion, backdrop compositing, and PPM
/// inspection helpers on [`pdf_raster::pdf_images::RgbaImage`].
pub trait RgbaImageExt: Sized {
    /// Allocates a `width x height` image filled with `rgba`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidDimensions`] if `width == 0`, `height == 0`, or
    /// `width * height` exceeds [`MAX_PIXELS`].
    fn from_solid(width: u32, height: u32, rgba: [u8; 4]) -> Result<Self, Error>;

    /// Wraps an existing `RGBA8` buffer of length `width * height * 4`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidDimensions`] if dimensions are zero/oversized or
    /// `pixels.len()` does not equal `width * height * 4`.
    fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, Error>;

    /// Converts any [`Pixmap`] (`Rgba8`, `Bgra8`, or `Rgba8Premul`) into an
    /// unpremultiplied straight-alpha sRGB [`RgbaImage`] for rubric scoring.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidDimensions`] if `pixmap` exceeds [`MAX_PIXELS`].
    fn from_pixmap(pixmap: &Pixmap) -> Result<Self, Error>;

    /// Composites straight-alpha `RGBA8` over a solid opaque sRGB `backdrop` (`[R, G, B]`).
    #[must_use]
    fn composite_solid(&self, backdrop: [u8; 3]) -> Vec<[u8; 3]>;

    /// Composites straight-alpha `RGBA8` over a `cell x cell` sRGB checkerboard (`light` / `dark`).
    #[must_use]
    fn composite_checkerboard(&self, cell: u32, light: [u8; 3], dark: [u8; 3]) -> Vec<[u8; 3]>;

    /// Encodes the image (composited over white if translucent) as a binary `P6` PPM byte buffer
    /// for quick visual inspection without external image-encoding crates.
    #[must_use]
    fn to_ppm(&self) -> Vec<u8>;
}

impl RgbaImageExt for RgbaImage {
    fn from_solid(width: u32, height: u32, rgba: [u8; 4]) -> Result<Self, Error> {
        let _ = checked_pixel_count(width, height)?;
        RgbaImage::solid(width, height, rgba, &RUBRIC_IMAGE_LIMITS)
            .map_err(|_| Error::InvalidDimensions { width, height })
    }

    fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, Error> {
        let count = checked_pixel_count(width, height)?;
        if pixels.len() != count * 4 {
            return Err(Error::InvalidDimensions { width, height });
        }
        RgbaImage::new(width, height, pixels, &RUBRIC_IMAGE_LIMITS)
            .map_err(|_| Error::InvalidDimensions { width, height })
    }

    fn from_pixmap(pixmap: &Pixmap) -> Result<Self, Error> {
        pixmap
            .to_rgba_image(&RUBRIC_IMAGE_LIMITS)
            .map_err(|_| Error::InvalidDimensions {
                width: pixmap.width(),
                height: pixmap.height(),
            })
    }

    fn composite_solid(&self, backdrop: [u8; 3]) -> Vec<[u8; 3]> {
        self.as_bytes()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&px| blend_over(px, backdrop))
            .collect()
    }

    fn composite_checkerboard(&self, cell: u32, light: [u8; 3], dark: [u8; 3]) -> Vec<[u8; 3]> {
        let cell = cell.max(1);
        let w = self.width() as usize;
        let h = self.height();
        let bytes = self.as_bytes();
        let mut out = Vec::with_capacity(w * (h as usize));
        for y in 0..h {
            let cy = (y / cell) & 1;
            for x in 0..self.width() {
                let cx = (x / cell) & 1;
                let bg = if (cx ^ cy) == 0 { light } else { dark };
                let idx = ((y as usize) * w + (x as usize)) * 4;
                let px = &bytes[idx..idx + 4];
                out.push(blend_over([px[0], px[1], px[2], px[3]], bg));
            }
        }
        out
    }

    fn to_ppm(&self) -> Vec<u8> {
        let header = format!("P6\n{} {}\n255\n", self.width(), self.height());
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
        let img = RgbaImage::from_solid(2, 2, *b"SECR").unwrap();
        let dbg = format!("{img:?}");
        assert!(!dbg.contains("SECR"), "{dbg}");
        assert!(dbg.contains("byte_len: 16"), "{dbg}");
    }

    #[test]
    fn bounds_and_compositing_behave_deterministically() {
        assert!(RgbaImage::from_solid(0, 10, [0; 4]).is_err());
        assert!(RgbaImage::from_solid(10, 0, [0; 4]).is_err());
        assert!(RgbaImage::from_solid(100_000, 100_000, [0; 4]).is_err());

        let mut img = RgbaImage::from_solid(4, 4, [200, 100, 50, 128]).unwrap();
        assert!(!img.is_opaque());
        assert!(img.is_uniform());
        img.fill_rect(2, 2, 10, 10, [10, 20, 30, 255]);
        assert!(!img.is_uniform());
        assert_eq!(img.pixel(3, 3), Some([10, 20, 30, 255]));
        assert_eq!(img.pixel(4, 4), None);

        let over_white = img.composite_solid([255, 255, 255]);
        assert_eq!(over_white[0], [227, 177, 152]);
        assert_eq!(over_white[15], [10, 20, 30]);
    }
}
