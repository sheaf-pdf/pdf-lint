//! Internal workspace PDFium (`as of chromium/8152`) oracle helpers, library
//! discovery, and `0–100` visual/ISO conformance rubric.
//!
//! This crate is `publish = false`: it keeps test-only `target/pdfium-8152`
//! discovery, custom-reader oracle helpers, and the differential scoring rubric
//! out of published [`pdf_raster`], while avoiding dev-dependency cycles from
//! `pdf-color` and `pdf-images` upward into `pdf-raster`.

use std::env;
use std::io::{Cursor, Read, Seek};
use std::ops::{BitOr, BitOrAssign};
use std::path::{Path, PathBuf};

use pdf_raster::pdfium::pdfium_render::prelude::{
    PdfBitmap, PdfBitmapFormat, PdfColor, PdfMatrix, PdfPageIndex, PdfPageRenderRotation,
    PdfRenderConfig, Pdfium as RawPdfium, Pixels,
};
use pdf_raster::{PdfiumBackend, pdfium};

pub mod rubric;

use rubric::{RgbaImage, RgbaImageExt};

/// Pinned Chromium branch number for the workspace PDFium oracle (`as of chromium/8152`).
pub const PINNED_CHROMIUM_BUILD: u32 = 8152;

/// Errors returned by [`Pdfium`] oracle helpers.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Underlying [`pdf_raster::pdfium::Error`].
    #[error(transparent)]
    Pdfium(#[from] pdfium::Error),
    /// Underlying `pdfium-render` error during custom oracle rendering.
    #[error("PDFium render error: {0}")]
    Render(String),
    /// Invalid or oversized image dimensions for [`RgbaImage`].
    #[error(transparent)]
    Rubric(#[from] rubric::Error),
}

/// Bitflags mirroring PDFium's `FPDF_*` render flags for low-level oracle probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderFlags(u32);

impl RenderFlags {
    /// Empty flag set (`0`).
    pub const NONE: Self = Self(0);
    /// Render annotations (`FPDF_ANNOT`).
    pub const ANNOT: Self = Self(1 << 0);
    /// Render for printing (`FPDF_PRINTING`).
    pub const PRINTING: Self = Self(1 << 1);
    /// Disable anti-aliased text (`FPDF_RENDER_NO_SMOOTHTEXT`).
    pub const NO_SMOOTHTEXT: Self = Self(1 << 2);
    /// Disable anti-aliased images (`FPDF_RENDER_NO_SMOOTHIMAGE`).
    pub const NO_SMOOTHIMAGE: Self = Self(1 << 3);
    /// Disable anti-aliased paths (`FPDF_RENDER_NO_SMOOTHPATH`).
    pub const NO_SMOOTHPATH: Self = Self(1 << 4);

    const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl BitOr for RenderFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for RenderFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Low-level PDFium render configuration for swatch/codec oracle probes and
/// `FPDF_RenderPageBitmapWithMatrix` checks.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// Output bitmap width in pixels.
    pub width: u32,
    /// Output bitmap height in pixels.
    pub height: u32,
    /// Pre-clear `[R, G, B, A]` background color.
    pub background: [u8; 4],
    /// Clockwise quarter-turn rotation (`0`, `1` = 90°, `2` = 180°, `3` = 270°).
    pub rotate: i32,
    /// PDFium render flags.
    pub flags: RenderFlags,
    /// Whether to draw interactive AcroForm widgets (`FPDF_FFLDraw`).
    pub forms: bool,
    /// Optional explicit `(matrix [a, b, c, d, e, f], clip [left, top, right, bottom])`.
    pub matrix_and_clip: Option<([f32; 6], [f32; 4])>,
}

impl RenderOptions {
    /// Crisp 1:1 swatch / pixel-grid probe over opaque white with all smoothing
    /// and annotations disabled.
    #[must_use]
    pub const fn crisp_swatches(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            background: [255, 255, 255, 255],
            rotate: 0,
            flags: RenderFlags(
                RenderFlags::NO_SMOOTHPATH.0
                    | RenderFlags::NO_SMOOTHTEXT.0
                    | RenderFlags::NO_SMOOTHIMAGE.0,
            ),
            forms: false,
            matrix_and_clip: None,
        }
    }
}

/// Discovers and binds [`PdfiumBackend`] for workspace tests, checking:
/// 1. `$PDF_RASTER_PDFIUM` (explicit library file path)
/// 2. `$PDF_LINT_PDFIUM` (explicit library file path used by `just test-pdfium`)
/// 3. `$PDFIUM_LIB_DIR` (directory containing platform `libpdfium`)
/// 4. `<workspace_root>/target/pdfium-8152/lib/<libpdfium>` (unpacked by `just _pdfium`)
/// 5. OS dynamic library search path
///
/// # Panics
///
/// Panics if `libpdfium` cannot be found or loaded, so ignored PDFium oracle
/// tests fail loudly instead of silently skipping.
#[must_use]
pub fn require_pdfium_backend() -> PdfiumBackend {
    try_discover_pdfium_backend()
        .unwrap_or_else(|err| panic!("live PDFium oracle required (run `just test-pdfium`): {err}"))
}

/// Returns a [`Pdfium`] oracle handle backed by [`require_pdfium_backend`].
///
/// # Panics
///
/// Panics if `libpdfium` cannot be found or loaded.
#[must_use]
pub fn require_pdfium() -> Pdfium {
    Pdfium {
        backend: require_pdfium_backend(),
    }
}

fn try_discover_pdfium_backend() -> Result<PdfiumBackend, pdfium::Error> {
    for var in ["PDF_RASTER_PDFIUM", "PDF_LINT_PDFIUM"] {
        if let Some(val) = env::var_os(var)
            && !val.is_empty()
        {
            return PdfiumBackend::bind_to_library(PathBuf::from(val));
        }
    }
    if let Some(dir) = env::var_os("PDFIUM_LIB_DIR")
        && !dir.is_empty()
    {
        let candidate = RawPdfium::pdfium_platform_library_name_at_path(&PathBuf::from(dir));
        return PdfiumBackend::bind_to_library(candidate);
    }
    let workspace_lib_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!("pdfium-{PINNED_CHROMIUM_BUILD}"))
        .join("lib");
    let pinned_candidate = RawPdfium::pdfium_platform_library_name_at_path(&workspace_lib_dir);
    if pinned_candidate.is_file() {
        return PdfiumBackend::bind_to_library(pinned_candidate);
    }
    PdfiumBackend::from_env()
}

/// Thin test-only PDFium oracle wrapper over [`PdfiumBackend`].
#[derive(Debug, Clone, Copy)]
pub struct Pdfium {
    backend: PdfiumBackend,
}

impl Pdfium {
    /// Underlying [`PdfiumBackend`] (`RenderBackend` implementation).
    #[must_use]
    pub const fn backend(&self) -> PdfiumBackend {
        self.backend
    }

    /// Renders `page_index` from an in-memory PDF slice using [`RenderOptions`].
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if PDFium rejects the document, page, or dimensions.
    pub fn render_page(
        &self,
        pdf_bytes: &[u8],
        page_index: usize,
        opts: &RenderOptions,
    ) -> Result<RgbaImage, Error> {
        self.render_page_custom_access(Cursor::new(pdf_bytes.to_vec()), None, page_index, opts)
    }

    /// Renders `page_index` through `FPDF_LoadCustomDocument` over any
    /// `Read + Seek` stream (`pdfium-render`'s `load_pdf_from_reader`).
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if PDFium rejects the stream, password, page, or
    /// dimensions.
    pub fn render_page_custom_access<R: Read + Seek + Send + Sync + 'static>(
        &self,
        reader: R,
        password: Option<&str>,
        page_index: usize,
        opts: &RenderOptions,
    ) -> Result<RgbaImage, Error> {
        let w_px = Pixels::try_from(opts.width)
            .ok()
            .filter(|&w| w > 0)
            .ok_or_else(|| Error::Render(format!("invalid width {}", opts.width)))?;
        let h_px = Pixels::try_from(opts.height)
            .ok()
            .filter(|&h| h > 0)
            .ok_or_else(|| Error::Render(format!("invalid height {}", opts.height)))?;

        let doc = self
            .backend
            .pdfium()
            .load_pdf_from_reader(reader, password)
            .map_err(|e| Error::Render(e.to_string()))?;
        let idx = PdfPageIndex::try_from(page_index)
            .map_err(|_| Error::Render(format!("page index {page_index} overflows u16")))?;
        let page = doc
            .pages()
            .get(idx)
            .map_err(|e| Error::Render(e.to_string()))?;

        let [br, bg, bb, ba] = opts.background;
        let rot = match opts.rotate.rem_euclid(4) {
            1 => PdfPageRenderRotation::Degrees90,
            2 => PdfPageRenderRotation::Degrees180,
            3 => PdfPageRenderRotation::Degrees270,
            _ => PdfPageRenderRotation::None,
        };

        let mut cfg = PdfRenderConfig::new()
            .set_format(PdfBitmapFormat::BGRA)
            .set_reverse_byte_order(true)
            .clear_before_rendering(true)
            .set_clear_color(PdfColor::new(br, bg, bb, ba))
            .set_fixed_size(w_px, h_px)
            .rotate(rot, false)
            .render_annotations(opts.flags.contains(RenderFlags::ANNOT))
            .use_print_quality(opts.flags.contains(RenderFlags::PRINTING))
            .set_text_smoothing(!opts.flags.contains(RenderFlags::NO_SMOOTHTEXT))
            .set_image_smoothing(!opts.flags.contains(RenderFlags::NO_SMOOTHIMAGE))
            .set_path_smoothing(!opts.flags.contains(RenderFlags::NO_SMOOTHPATH))
            .render_form_data(opts.forms && opts.matrix_and_clip.is_none());

        if let Some(([a, b, c, d, e, f], [left, top, right, bottom])) = opts.matrix_and_clip {
            cfg = cfg
                .reset_matrix(PdfMatrix::new(a, b, c, d, e, f))
                .map_err(|err| Error::Render(err.to_string()))?
                .clip(
                    left.round() as Pixels,
                    top.round() as Pixels,
                    right.round() as Pixels,
                    bottom.round() as Pixels,
                );
        }

        let mut buf = vec![0_u8; (opts.width as usize) * (opts.height as usize) * 4];
        {
            let mut bitmap =
                PdfBitmap::from_bytes(w_px, h_px, PdfBitmapFormat::BGRA, buf.as_mut_slice())
                    .map_err(|err| Error::Render(err.to_string()))?;
            page.render_into_bitmap_with_config(&mut bitmap, &cfg)
                .map_err(|err| Error::Render(err.to_string()))?;
        }
        Ok(RgbaImage::from_rgba(opts.width, opts.height, buf)?)
    }
}
