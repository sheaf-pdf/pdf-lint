//! Safe [`RenderBackend`] and oracle helpers backed by [`pdfium_render`].

use std::collections::HashMap;
use std::env;
use std::fmt;
use std::io::Cursor;
use std::ops::{BitAnd, BitOr, BitOrAssign};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use pdf_gfx::{PageGeom, Rect, Rotation};
use pdfium_render::prelude::{
    PdfBitmap, PdfBitmapFormat, PdfColor, PdfDocument, PdfMatrix, PdfPage, PdfPageIndex,
    PdfPageRenderRotation, PdfRenderConfig, Pdfium as RawPdfium, PdfiumError, PdfiumInternalError,
    PdfiumLibraryBindings, Pixels,
};

use crate::backend::{BackendError, RenderBackend, RenderedDoc};
use crate::limits::Limits;
use crate::opts::{RenderMedium, RenderOpts, RenderSummary, Viewport};
use crate::pixmap::{PixelFormat, Pixmap};
use crate::rubric::{MAX_PIXELS, RgbaImage};

/// Errors produced by [`Pdfium`] and [`PdfiumBackend`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// `libpdfium` could not be located via environment variables or the OS
    /// dynamic library search path.
    #[error(
        "libpdfium not found (set PDF_RASTER_PDFIUM or PDFIUM_LIB_DIR, or install libpdfium on the system library path); searched: {searched}"
    )]
    LibraryNotFound {
        /// Human-readable list of candidate locations checked.
        searched: String,
    },
    /// Binding to `libpdfium` failed.
    #[error("failed to bind to libpdfium ({path}): {reason}")]
    LibraryLoad {
        /// Path or library name attempted.
        path: PathBuf,
        /// Diagnostic from `pdfium-render` / `libloading`.
        reason: String,
    },
    /// PDFium rejected a document on open.
    #[error("PDFium failed to load document ({detail})")]
    DocumentLoad {
        /// Human-readable diagnostic from `pdfium-render`.
        detail: String,
        /// `true` when PDFium reported `FPDF_ERR_PASSWORD`.
        locked: bool,
    },
    /// Requested page index failed to load.
    #[error("PDFium failed to load page {page_index}: {detail}")]
    PageLoad {
        /// Zero-based page index.
        page_index: usize,
        /// Diagnostic from `pdfium-render`.
        detail: String,
    },
    /// Requested bitmap dimensions were zero, oversized, or rejected by PDFium.
    #[error("invalid or unsupported bitmap dimensions {width}x{height}")]
    InvalidDimensions {
        /// Requested width in pixels.
        width: u32,
        /// Requested height in pixels.
        height: u32,
    },
    /// Password contained an interior NUL byte.
    #[error("password contains an interior NUL byte")]
    InvalidPassword,
}

static INIT_LOCK: Mutex<()> = Mutex::new(());
static GLOBAL_PDFIUM: OnceLock<Arc<RawPdfium>> = OnceLock::new();

fn register_global(pdfium: Arc<RawPdfium>) -> &'static RawPdfium {
    GLOBAL_PDFIUM.get_or_init(|| pdfium).as_ref()
}

fn try_init_global(
    bind: impl FnOnce() -> Result<Box<dyn PdfiumLibraryBindings>, Error>,
) -> Result<&'static RawPdfium, Error> {
    if let Some(p) = GLOBAL_PDFIUM.get() {
        return Ok(p.as_ref());
    }
    let _guard = INIT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(p) = GLOBAL_PDFIUM.get() {
        return Ok(p.as_ref());
    }
    let bindings = bind()?;
    let instance = Arc::new(RawPdfium::new(bindings));
    let _ = GLOBAL_PDFIUM.set(instance);
    Ok(GLOBAL_PDFIUM.get().expect("just initialized").as_ref())
}

#[cfg(not(target_arch = "wasm32"))]
fn bind_from_env_or_system() -> Result<Box<dyn PdfiumLibraryBindings>, Error> {
    if let Some(explicit) = env::var_os("PDF_RASTER_PDFIUM")
        && !explicit.is_empty()
    {
        let p = PathBuf::from(explicit);
        if !p.is_file() {
            return Err(Error::LibraryLoad {
                path: p,
                reason: "PDF_RASTER_PDFIUM does not point to an existing file".to_string(),
            });
        }
        return RawPdfium::bind_to_library(&p).map_err(|e| map_bind_err(p, e));
    }

    if let Some(dir) = env::var_os("PDFIUM_LIB_DIR")
        && !dir.is_empty()
    {
        let candidate = RawPdfium::pdfium_platform_library_name_at_path(&PathBuf::from(dir));
        if !candidate.is_file() {
            return Err(Error::LibraryLoad {
                path: candidate,
                reason: "PDFIUM_LIB_DIR does not contain a libpdfium binary".to_string(),
            });
        }
        return RawPdfium::bind_to_library(&candidate).map_err(|e| map_bind_err(candidate, e));
    }

    let sys_name = RawPdfium::pdfium_platform_library_name();
    match RawPdfium::bind_to_system_library() {
        Ok(b) => Ok(b),
        Err(PdfiumError::LoadLibraryError(_)) => Err(Error::LibraryNotFound {
            searched: format!("system dynamic loader ({})", sys_name.to_string_lossy()),
        }),
        Err(other) => Err(map_bind_err(PathBuf::from(sys_name), other)),
    }
}

#[cfg(target_arch = "wasm32")]
fn bind_from_env_or_system() -> Result<Box<dyn PdfiumLibraryBindings>, Error> {
    RawPdfium::bind_to_system_library().map_err(|e| Error::LibraryLoad {
        path: PathBuf::from("wasm"),
        reason: e.to_string(),
    })
}

fn map_bind_err(path: PathBuf, err: PdfiumError) -> Error {
    Error::LibraryLoad {
        path,
        reason: err.to_string(),
    }
}

fn map_doc_err(err: PdfiumError) -> Error {
    match err {
        PdfiumError::CStringConversionError(_) => Error::InvalidPassword,
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            Error::DocumentLoad {
                detail: "password required or incorrect".to_string(),
                locked: true,
            }
        }
        other => Error::DocumentLoad {
            detail: other.to_string(),
            locked: false,
        },
    }
}

/// Thin [`RenderBackend`] adapter over [`pdfium_render::prelude::Pdfium`].
///
/// Can be constructed directly from an existing [`pdfium_render::prelude::Pdfium`]
/// or `Arc<Pdfium>` (so host applications share a single global `pdfium-render`
/// binding and lock), or bound from a library path / environment variables via
/// [`PdfiumBackend::bind_to_library`] and [`PdfiumBackend::from_env`].
#[derive(Clone, Copy)]
pub struct PdfiumBackend {
    pdfium: &'static RawPdfium,
}

impl fmt::Debug for PdfiumBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PdfiumBackend").finish()
    }
}

impl PdfiumBackend {
    /// Wraps an owned [`pdfium_render::prelude::Pdfium`] instance, promoting it
    /// to the shared process-wide handle.
    #[must_use]
    pub fn new(pdfium: RawPdfium) -> Self {
        Self::from_arc(Arc::new(pdfium))
    }

    /// Wraps a shared `Arc<pdfium_render::prelude::Pdfium>` instance so callers
    /// that already manage a `Pdfium` handle in `Arc` share the exact same
    /// instance without re-binding.
    #[must_use]
    pub fn from_arc(pdfium: Arc<RawPdfium>) -> Self {
        Self {
            pdfium: register_global(pdfium),
        }
    }

    /// Wraps an already-`'static` [`pdfium_render::prelude::Pdfium`] reference.
    #[must_use]
    pub const fn from_static(pdfium: &'static RawPdfium) -> Self {
        Self { pdfium }
    }

    /// Wraps a [`Pdfium`] oracle handle.
    #[must_use]
    pub const fn from_oracle(pdfium: Pdfium) -> Self {
        Self {
            pdfium: pdfium.raw(),
        }
    }

    /// Binds dynamically to `libpdfium` at `path` (or reuses the active
    /// `pdfium-render` instance if already initialized in this process).
    ///
    /// # Errors
    ///
    /// Returns [`Error::LibraryLoad`] if `path` cannot be loaded.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn bind_to_library(path: impl AsRef<Path>) -> Result<Self, Error> {
        let p = path.as_ref().to_path_buf();
        let raw =
            try_init_global(|| RawPdfium::bind_to_library(&p).map_err(|e| map_bind_err(p, e)))?;
        Ok(Self { pdfium: raw })
    }

    /// Binds dynamically to the OS system `libpdfium` library.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if `libpdfium` cannot be loaded from the system path.
    pub fn bind_to_system_library() -> Result<Self, Error> {
        let raw = try_init_global(|| {
            RawPdfium::bind_to_system_library()
                .map_err(|e| map_bind_err(PathBuf::from("libpdfium"), e))
        })?;
        Ok(Self { pdfium: raw })
    }

    /// Locates and binds to `libpdfium` from `$PDF_RASTER_PDFIUM`,
    /// `$PDFIUM_LIB_DIR`, or the OS dynamic library search path.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LibraryNotFound`] or [`Error::LibraryLoad`] on failure.
    pub fn from_env() -> Result<Self, Error> {
        let raw = try_init_global(bind_from_env_or_system)?;
        Ok(Self { pdfium: raw })
    }

    /// Alias for [`PdfiumBackend::from_env`].
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if `libpdfium` cannot be found or loaded.
    pub fn try_global() -> Result<Self, Error> {
        Self::from_env()
    }

    /// Underlying [`pdfium_render::prelude::Pdfium`] handle.
    #[must_use]
    pub const fn pdfium(&self) -> &'static RawPdfium {
        self.pdfium
    }
}

struct PdfiumRenderedDoc {
    doc: PdfDocument<'static>,
    page_count: usize,
    geoms: Mutex<HashMap<usize, PageGeom>>,
}

impl RenderedDoc for PdfiumRenderedDoc {
    fn page_count(&self) -> usize {
        self.page_count
    }

    fn page_geom(&self, page_index: usize) -> Result<PageGeom, BackendError> {
        if page_index >= self.page_count {
            return Err(BackendError::PageOutOfBounds {
                page_index,
                page_count: self.page_count,
            });
        }
        {
            let guard = self
                .geoms
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(&geom) = guard.get(&page_index) {
                return Ok(geom);
            }
        }
        let idx =
            PdfPageIndex::try_from(page_index).map_err(|_| BackendError::PageOutOfBounds {
                page_index,
                page_count: self.page_count,
            })?;
        let page = self.doc.pages().get(idx).map_err(|e| {
            BackendError::custom(Error::PageLoad {
                page_index,
                detail: e.to_string(),
            })
        })?;
        let geom = extract_pdf_page_geom(&page);
        let mut guard = self
            .geoms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.insert(page_index, geom);
        Ok(geom)
    }

    fn render_page(
        &self,
        page_index: usize,
        opts: &RenderOpts,
    ) -> Result<(Pixmap, RenderSummary), BackendError> {
        let geom = self.page_geom(page_index)?;
        let resolved = opts
            .viewport
            .resolve(&geom, &opts.limits)
            .map_err(BackendError::Raster)?;

        let idx =
            PdfPageIndex::try_from(page_index).map_err(|_| BackendError::PageOutOfBounds {
                page_index,
                page_count: self.page_count,
            })?;
        let page = self.doc.pages().get(idx).map_err(|e| {
            BackendError::custom(Error::PageLoad {
                page_index,
                detail: e.to_string(),
            })
        })?;

        let w_px = Pixels::try_from(resolved.width)
            .map_err(|_| BackendError::Raster(crate::Error::Limit(crate::LimitKind::Dimensions)))?;
        let h_px = Pixels::try_from(resolved.height)
            .map_err(|_| BackendError::Raster(crate::Error::Limit(crate::LimitKind::Dimensions)))?;

        let [br, bg, bb, ba] = opts.background.unwrap_or([0, 0, 0, 0]);
        let clear = PdfColor::new(br, bg, bb, ba);

        let mut cfg = PdfRenderConfig::new()
            .set_format(PdfBitmapFormat::BGRA)
            .clear_before_rendering(true)
            .set_clear_color(clear)
            .render_annotations(opts.draw.annots)
            .use_print_quality(opts.medium == RenderMedium::Print)
            .set_path_smoothing(opts.anti_alias.paths)
            .set_text_smoothing(opts.anti_alias.text)
            .set_image_smoothing(opts.anti_alias.images);

        let mut bitmap =
            PdfBitmap::empty(w_px, h_px, PdfBitmapFormat::BGRA).map_err(BackendError::custom)?;

        match opts.viewport {
            Viewport::Page { rotation, .. } | Viewport::Dpi { rotation, .. } if opts.draw.forms => {
                cfg = cfg
                    .set_fixed_size(w_px, h_px)
                    .rotate(to_pdfium_rotation(rotation), false)
                    .render_form_data(true);
                page.render_into_bitmap_with_config(&mut bitmap, &cfg)
                    .map_err(BackendError::custom)?;
            }
            Viewport::Tile {
                page_width,
                page_height,
                x,
                y,
                rotation,
                ..
            } if opts.draw.forms => {
                let pw = Pixels::try_from(page_width).map_err(|_| {
                    BackendError::Raster(crate::Error::Limit(crate::LimitKind::Dimensions))
                })?;
                let ph = Pixels::try_from(page_height).map_err(|_| {
                    BackendError::Raster(crate::Error::Limit(crate::LimitKind::Dimensions))
                })?;
                let ox = Pixels::try_from(x)
                    .ok()
                    .and_then(Pixels::checked_neg)
                    .ok_or_else(|| {
                        BackendError::Raster(crate::Error::InvalidViewport(
                            "tile offset overflows Pixels",
                        ))
                    })?;
                let oy = Pixels::try_from(y)
                    .ok()
                    .and_then(Pixels::checked_neg)
                    .ok_or_else(|| {
                        BackendError::Raster(crate::Error::InvalidViewport(
                            "tile offset overflows Pixels",
                        ))
                    })?;
                // Ensure overhang regions outside `page_width x page_height` are cleared first.
                if x.saturating_add(resolved.width) > page_width
                    || y.saturating_add(resolved.height) > page_height
                {
                    let clear_only = PdfRenderConfig::new()
                        .set_fixed_size(w_px, h_px)
                        .set_clear_color(clear)
                        .render_form_data(false)
                        .clip(0, 0, 0, 0);
                    let _ = page.render_into_bitmap_with_config(&mut bitmap, &clear_only);
                }
                cfg = cfg
                    .set_fixed_size(pw, ph)
                    .set_origin(ox, oy)
                    .rotate(to_pdfium_rotation(rotation), false)
                    .render_form_data(true);
                page.render_into_bitmap_with_config(&mut bitmap, &cfg)
                    .map_err(BackendError::custom)?;
            }
            _ => {
                // Matrix path (`FPDF_RenderPageBitmapWithMatrix`): exact affine
                // mapping from PDFium's unrotated ViewSpace onto `[0..w, 0..h]`.
                let Some(view0_to_page) = geom.to_view().inverse() else {
                    return Err(BackendError::Raster(crate::Error::InvalidViewport(
                        "singular unrotated view transform",
                    )));
                };
                let m = view0_to_page.then(resolved.page_to_device);
                cfg = cfg
                    .set_fixed_size(w_px, h_px)
                    .render_form_data(false)
                    .reset_matrix(PdfMatrix::new(m.a, m.b, m.c, m.d, m.e, m.f))
                    .map_err(BackendError::custom)?;
                page.render_into_bitmap_with_config(&mut bitmap, &cfg)
                    .map_err(BackendError::custom)?;
            }
        }

        let rgba = bitmap_to_tight_rgba(&bitmap, resolved.width, resolved.height)?;
        let pixmap = rgba_vec_to_pixmap(
            rgba,
            resolved.width,
            resolved.height,
            opts.format,
            &opts.limits,
        )?;
        Ok((pixmap, RenderSummary::default()))
    }
}

impl RenderBackend for PdfiumBackend {
    fn name(&self) -> &'static str {
        "pdfium"
    }

    fn open_bytes(
        &self,
        bytes: Arc<[u8]>,
        password: Option<&str>,
    ) -> Result<Box<dyn RenderedDoc>, BackendError> {
        if password.is_some_and(|pw| pw.contains('\0')) {
            return Err(BackendError::custom(Error::InvalidPassword));
        }
        let doc = self
            .pdfium
            .load_pdf_from_reader(Cursor::new(bytes), password)
            .map_err(|e| match map_doc_err(e) {
                Error::DocumentLoad { locked: true, .. } => BackendError::Locked,
                other => BackendError::custom(other),
            })?;
        let page_count = usize::try_from(doc.pages().len()).unwrap_or(0);
        Ok(Box::new(PdfiumRenderedDoc {
            doc,
            page_count,
            geoms: Mutex::new(HashMap::new()),
        }))
    }
}

fn to_pdfium_rotation(rot: Rotation) -> PdfPageRenderRotation {
    match rot {
        Rotation::R0 => PdfPageRenderRotation::None,
        Rotation::R90 => PdfPageRenderRotation::Degrees90,
        Rotation::R180 => PdfPageRenderRotation::Degrees180,
        Rotation::R270 => PdfPageRenderRotation::Degrees270,
    }
}

fn from_pdfium_rotation(rot: PdfPageRenderRotation) -> Rotation {
    match rot {
        PdfPageRenderRotation::None => Rotation::R0,
        PdfPageRenderRotation::Degrees90 => Rotation::R90,
        PdfPageRenderRotation::Degrees180 => Rotation::R180,
        PdfPageRenderRotation::Degrees270 => Rotation::R270,
    }
}

fn extract_pdf_page_geom(page: &PdfPage<'_>) -> PageGeom {
    let rotation = page.rotation().map_or(Rotation::R0, from_pdfium_rotation);
    let pdfium_w = page.width().value;
    let pdfium_h = page.height().value;

    let crop_box = page.boundaries().crop().ok().map(|b| {
        Rect::new(
            b.bounds.left().value,
            b.bounds.bottom().value,
            b.bounds.right().value,
            b.bounds.top().value,
        )
    });

    let media_box = if let Ok(b) = page.boundaries().media() {
        Rect::new(
            b.bounds.left().value,
            b.bounds.bottom().value,
            b.bounds.right().value,
            b.bounds.top().value,
        )
    } else {
        let (unrot_w, unrot_h) = match rotation {
            Rotation::R90 | Rotation::R270 => (pdfium_h, pdfium_w),
            Rotation::R0 | Rotation::R180 => (pdfium_w, pdfium_h),
        };
        Rect::new(0.0, 0.0, unrot_w, unrot_h)
    };

    let mut geom = PageGeom::new(media_box, crop_box, rotation);
    let unscaled_w = geom.view_size().width;
    if unscaled_w > 0.0 && pdfium_w > 0.0 && pdfium_w.is_finite() {
        let ratio = pdfium_w / unscaled_w;
        if (ratio - 1.0).abs() > 1e-4 && ratio.is_finite() {
            geom.user_unit = ratio;
        }
    }
    geom
}

fn bitmap_to_tight_rgba(
    bitmap: &PdfBitmap<'_>,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, BackendError> {
    let raw = bitmap.as_rgba_bytes();
    let w_bytes = (width as usize) * 4;
    let h = height as usize;
    let expected = w_bytes * h;
    if raw.len() == expected {
        return Ok(raw);
    }
    if h == 0 || raw.len() < expected || !raw.len().is_multiple_of(h) {
        return Err(BackendError::custom(Error::InvalidDimensions {
            width,
            height,
        }));
    }
    let stride = raw.len() / h;
    let mut tight = Vec::with_capacity(expected);
    for y in 0..h {
        tight.extend_from_slice(&raw[y * stride..y * stride + w_bytes]);
    }
    Ok(tight)
}

fn rgba_vec_to_pixmap(
    rgba: Vec<u8>,
    w: u32,
    h: u32,
    format: PixelFormat,
    limits: &Limits,
) -> Result<Pixmap, BackendError> {
    let out = match format {
        PixelFormat::Rgba8 => rgba,
        PixelFormat::Bgra8 => {
            let mut buf = Vec::with_capacity(rgba.len());
            for &[r, g, b, a] in rgba.as_chunks::<4>().0 {
                buf.extend_from_slice(&[b, g, r, a]);
            }
            buf
        }
        PixelFormat::Rgba8Premul => {
            let mut buf = Vec::with_capacity(rgba.len());
            for &[r, g, b, a] in rgba.as_chunks::<4>().0 {
                let pm = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
                buf.extend_from_slice(&[pm(r), pm(g), pm(b), a]);
            }
            buf
        }
    };
    Pixmap::from_bytes(w, h, format, out, limits).map_err(BackendError::Raster)
}

// ---------------------------------------------------------------------------
// Direct single-page oracle helpers (`Pdfium`, `RenderFlags`, `RenderOptions`)
// built cleanly over `pdfium-render` for conformance & shadow-mode callers
// ---------------------------------------------------------------------------

/// Bitflags mapping onto [`PdfRenderConfig`] flags for [`Pdfium::render_page`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct RenderFlags(u32);

impl RenderFlags {
    /// Default viewer display flags (`0`).
    pub const NONE: RenderFlags = RenderFlags(0);
    /// Render non-interactive annotations (`FPDF_ANNOT = 0x01`).
    pub const ANNOT: RenderFlags = RenderFlags(0x01);
    /// LCD subpixel text optimization (`FPDF_LCD_TEXT = 0x02`).
    pub const LCD_TEXT: RenderFlags = RenderFlags(0x02);
    /// Disable platform-native text output (`FPDF_NO_NATIVETEXT = 0x04`).
    pub const NO_NATIVETEXT: RenderFlags = RenderFlags(0x04);
    /// Grayscale output (`FPDF_GRAYSCALE = 0x08`).
    pub const GRAYSCALE: RenderFlags = RenderFlags(0x08);
    /// Render for printing (`FPDF_PRINTING = 0x800`).
    pub const PRINTING: RenderFlags = RenderFlags(0x800);
    /// Disable text anti-aliasing (`FPDF_RENDER_NO_SMOOTHTEXT = 0x1000`).
    pub const NO_SMOOTHTEXT: RenderFlags = RenderFlags(0x1000);
    /// Disable image anti-aliasing (`FPDF_RENDER_NO_SMOOTHIMAGE = 0x2000`).
    pub const NO_SMOOTHIMAGE: RenderFlags = RenderFlags(0x2000);
    /// Disable path anti-aliasing (`FPDF_RENDER_NO_SMOOTHPATH = 0x4000`).
    pub const NO_SMOOTHPATH: RenderFlags = RenderFlags(0x4000);

    /// Raw integer flag bits.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Combines two flag sets (`const`-friendly).
    #[must_use]
    pub const fn union(self, other: RenderFlags) -> RenderFlags {
        RenderFlags(self.0 | other.0)
    }

    const fn contains(self, flag: RenderFlags) -> bool {
        (self.0 & flag.0) == flag.0
    }
}

impl BitOr for RenderFlags {
    type Output = RenderFlags;

    fn bitor(self, rhs: RenderFlags) -> RenderFlags {
        RenderFlags(self.0 | rhs.0)
    }
}

impl BitOrAssign for RenderFlags {
    fn bitor_assign(&mut self, rhs: RenderFlags) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for RenderFlags {
    type Output = RenderFlags;

    fn bitand(self, rhs: RenderFlags) -> RenderFlags {
        RenderFlags(self.0 & rhs.0)
    }
}

/// Configuration for rendering a single PDF page via [`Pdfium`].
#[derive(Debug, Clone, PartialEq)]
pub struct RenderOptions {
    /// Target bitmap width in pixels (`>= 1`).
    pub width: u32,
    /// Target bitmap height in pixels (`>= 1`).
    pub height: u32,
    /// Initial bitmap fill color in `[R, G, B, A]` before rendering (defaults to opaque white).
    pub background: [u8; 4],
    /// Additional quarter-turn clockwise rotation (`0..=3`).
    pub rotate: u8,
    /// PDFium page rendering flags.
    pub flags: RenderFlags,
    /// Whether to render interactive form widgets (`FPDF_FFLDraw`).
    pub forms: bool,
    /// Optional explicit `[a, b, c, d, e, f]` transform and `[left, top, right, bottom]` clip rect.
    pub matrix_and_clip: Option<([f32; 6], [f32; 4])>,
}

impl RenderOptions {
    /// Creates options for a `width x height` opaque white bitmap with default viewer flags.
    #[must_use]
    pub const fn new(width: u32, height: u32) -> RenderOptions {
        RenderOptions {
            width,
            height,
            background: [255, 255, 255, 255],
            rotate: 0,
            flags: RenderFlags::ANNOT,
            forms: false,
            matrix_and_clip: None,
        }
    }

    /// Sets crisp, un-smoothed path/image flags suitable for deterministic color-swatch strips.
    #[must_use]
    pub const fn crisp_swatches(width: u32, height: u32) -> RenderOptions {
        RenderOptions {
            width,
            height,
            background: [255, 255, 255, 255],
            rotate: 0,
            flags: RenderFlags::NO_SMOOTHPATH.union(RenderFlags::NO_SMOOTHIMAGE),
            forms: false,
            matrix_and_clip: None,
        }
    }
}

/// Thread-safe handle to the process-wide [`pdfium_render::prelude::Pdfium`]
/// instance for direct page inspection and rendering into [`RgbaImage`].
#[derive(Clone, Copy)]
pub struct Pdfium {
    raw: &'static RawPdfium,
}

impl fmt::Debug for Pdfium {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pdfium").finish()
    }
}

impl Pdfium {
    /// Loads (or reuses) `libpdfium` from `$PDF_RASTER_PDFIUM`,
    /// `$PDFIUM_LIB_DIR`, or the OS dynamic library search path.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LibraryNotFound`] or [`Error::LibraryLoad`] on failure.
    pub fn from_env() -> Result<Self, Error> {
        let raw = try_init_global(bind_from_env_or_system)?;
        Ok(Self { raw })
    }

    /// Alias for [`Pdfium::from_env`].
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if `libpdfium` cannot be found or loaded.
    pub fn try_global() -> Result<Self, Error> {
        Self::from_env()
    }

    /// Binds dynamically to `libpdfium` at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LibraryLoad`] on failure.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn bind_to_library(path: impl AsRef<Path>) -> Result<Self, Error> {
        PdfiumBackend::bind_to_library(path).map(|b| Self { raw: b.pdfium() })
    }

    /// Returns `true` if `libpdfium` is available and initialized.
    #[must_use]
    pub fn is_available() -> bool {
        Self::from_env().is_ok()
    }

    /// Underlying [`pdfium_render::prelude::Pdfium`] reference.
    #[must_use]
    pub const fn raw(&self) -> &'static RawPdfium {
        self.raw
    }

    /// Returns the number of pages in `pdf`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DocumentLoad`] or [`Error::InvalidPassword`] on failure.
    pub fn page_count(&self, pdf: &[u8], password: Option<&str>) -> Result<usize, Error> {
        if password.is_some_and(|pw| pw.contains('\0')) {
            return Err(Error::InvalidPassword);
        }
        let doc = self
            .raw
            .load_pdf_from_byte_slice(pdf, password)
            .map_err(map_doc_err)?;
        Ok(usize::try_from(doc.pages().len()).unwrap_or(0))
    }

    /// Returns `(width_pt, height_pt)` of `page_index` in PDF points (`1/72` inch).
    ///
    /// # Errors
    ///
    /// Returns [`Error`] on open or page-load failure.
    pub fn page_size(
        &self,
        pdf: &[u8],
        page_index: usize,
        password: Option<&str>,
    ) -> Result<(f32, f32), Error> {
        if password.is_some_and(|pw| pw.contains('\0')) {
            return Err(Error::InvalidPassword);
        }
        let doc = self
            .raw
            .load_pdf_from_byte_slice(pdf, password)
            .map_err(map_doc_err)?;
        let idx = PdfPageIndex::try_from(page_index).map_err(|_| Error::PageLoad {
            page_index,
            detail: "page index exceeds i32::MAX".to_string(),
        })?;
        let page = doc.pages().get(idx).map_err(|e| Error::PageLoad {
            page_index,
            detail: e.to_string(),
        })?;
        Ok((page.width().value, page.height().value))
    }

    /// Renders `page_index` from an in-memory `pdf` slice.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] on invalid dimensions, open failure, or render failure.
    pub fn render_page(
        &self,
        pdf: &[u8],
        page_index: usize,
        options: &RenderOptions,
    ) -> Result<RgbaImage, Error> {
        self.render_page_with_password(pdf, page_index, None, options)
    }

    /// Renders `page_index` from an in-memory `pdf` slice using an optional `password`.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] on invalid dimensions/password, open failure, or render failure.
    pub fn render_page_with_password(
        &self,
        pdf: &[u8],
        page_index: usize,
        password: Option<&str>,
        options: &RenderOptions,
    ) -> Result<RgbaImage, Error> {
        check_dims(options.width, options.height)?;
        if password.is_some_and(|pw| pw.contains('\0')) {
            return Err(Error::InvalidPassword);
        }
        let doc = self
            .raw
            .load_pdf_from_byte_slice(pdf, password)
            .map_err(map_doc_err)?;
        render_doc_page(&doc, page_index, options)
    }

    /// Renders `page_index` via `pdfium-render`'s custom `Read + Seek` file-access
    /// callback (`load_pdf_from_reader`).
    ///
    /// # Errors
    ///
    /// Returns [`Error`] on invalid dimensions/password, open failure, or render failure.
    pub fn render_page_custom_access(
        &self,
        pdf: &[u8],
        page_index: usize,
        password: Option<&str>,
        options: &RenderOptions,
    ) -> Result<RgbaImage, Error> {
        check_dims(options.width, options.height)?;
        if password.is_some_and(|pw| pw.contains('\0')) {
            return Err(Error::InvalidPassword);
        }
        let doc = self
            .raw
            .load_pdf_from_reader(Cursor::new(pdf), password)
            .map_err(map_doc_err)?;
        render_doc_page(&doc, page_index, options)
    }
}

fn check_dims(width: u32, height: u32) -> Result<(), Error> {
    if width == 0 || height == 0 {
        return Err(Error::InvalidDimensions { width, height });
    }
    let count = (width as usize)
        .checked_mul(height as usize)
        .filter(|&n| n <= MAX_PIXELS)
        .ok_or(Error::InvalidDimensions { width, height })?;
    let _ = count
        .checked_mul(4)
        .ok_or(Error::InvalidDimensions { width, height })?;
    Ok(())
}

fn render_doc_page(
    doc: &PdfDocument<'_>,
    page_index: usize,
    options: &RenderOptions,
) -> Result<RgbaImage, Error> {
    let w_px = Pixels::try_from(options.width).map_err(|_| Error::InvalidDimensions {
        width: options.width,
        height: options.height,
    })?;
    let h_px = Pixels::try_from(options.height).map_err(|_| Error::InvalidDimensions {
        width: options.width,
        height: options.height,
    })?;
    let idx = PdfPageIndex::try_from(page_index).map_err(|_| Error::PageLoad {
        page_index,
        detail: "page index exceeds i32::MAX".to_string(),
    })?;
    let page = doc.pages().get(idx).map_err(|e| Error::PageLoad {
        page_index,
        detail: e.to_string(),
    })?;

    let [r, g, b, a] = options.background;
    let rot = match options.rotate & 3 {
        1 => PdfPageRenderRotation::Degrees90,
        2 => PdfPageRenderRotation::Degrees180,
        3 => PdfPageRenderRotation::Degrees270,
        _ => PdfPageRenderRotation::None,
    };

    let mut cfg = PdfRenderConfig::new()
        .set_format(PdfBitmapFormat::BGRA)
        .set_fixed_size(w_px, h_px)
        .clear_before_rendering(true)
        .set_clear_color(PdfColor::new(r, g, b, a))
        .render_annotations(options.flags.contains(RenderFlags::ANNOT))
        .use_lcd_text_rendering(options.flags.contains(RenderFlags::LCD_TEXT))
        .disable_native_text_rendering(options.flags.contains(RenderFlags::NO_NATIVETEXT))
        .use_grayscale_rendering(options.flags.contains(RenderFlags::GRAYSCALE))
        .use_print_quality(options.flags.contains(RenderFlags::PRINTING))
        .set_text_smoothing(!options.flags.contains(RenderFlags::NO_SMOOTHTEXT))
        .set_image_smoothing(!options.flags.contains(RenderFlags::NO_SMOOTHIMAGE))
        .set_path_smoothing(!options.flags.contains(RenderFlags::NO_SMOOTHPATH));

    if let Some(([ma, mb, mc, md, me, mf], [cl, ct, cr, cb])) = options.matrix_and_clip {
        cfg = cfg
            .render_form_data(false)
            .reset_matrix(PdfMatrix::new(ma, mb, mc, md, me, mf))
            .map_err(|_| Error::InvalidDimensions {
                width: options.width,
                height: options.height,
            })?
            .clip(cl as Pixels, ct as Pixels, cr as Pixels, cb as Pixels);
    } else if options.forms {
        cfg = cfg.rotate(rot, false).render_form_data(true);
    } else {
        // When `forms == false`, `PdfRenderConfig::apply_to_page` uses
        // `FPDF_RenderPageBitmapWithMatrix` and scales by `width_scale` /
        // `height_scale` (which are `1.0` under `set_fixed_size`), so compute
        // the exact `ViewSpace(unrotated) -> [0..w, 0..h]` matrix directly.
        let geom = extract_pdf_page_geom(&page);
        let mut rotated = geom;
        rotated.rotation = geom.rotation.plus(match options.rotate & 3 {
            1 => Rotation::R90,
            2 => Rotation::R180,
            3 => Rotation::R270,
            _ => Rotation::R0,
        });
        let shown = rotated.view_size();
        if let Some(view0_to_page) = geom.to_view().inverse()
            && shown.width > 0.0
            && shown.height > 0.0
        {
            let sx = (options.width as f32) / shown.width;
            let sy = (options.height as f32) / shown.height;
            let m = view0_to_page
                .then(rotated.to_view())
                .then(pdf_gfx::Matrix::scale(sx, sy));
            cfg = cfg
                .render_form_data(false)
                .reset_matrix(PdfMatrix::new(m.a, m.b, m.c, m.d, m.e, m.f))
                .map_err(|_| Error::InvalidDimensions {
                    width: options.width,
                    height: options.height,
                })?;
        } else {
            cfg = cfg.rotate(rot, false).render_form_data(true);
        }
    }

    let mut bitmap = PdfBitmap::empty(w_px, h_px, PdfBitmapFormat::BGRA).map_err(|_| {
        Error::InvalidDimensions {
            width: options.width,
            height: options.height,
        }
    })?;
    page.render_into_bitmap_with_config(&mut bitmap, &cfg)
        .map_err(|e| Error::PageLoad {
            page_index,
            detail: e.to_string(),
        })?;

    let tight = bitmap_to_tight_rgba(&bitmap, options.width, options.height).map_err(|_| {
        Error::InvalidDimensions {
            width: options.width,
            height: options.height,
        }
    })?;
    RgbaImage::from_rgba(options.width, options.height, tight).map_err(|_| {
        Error::InvalidDimensions {
            width: options.width,
            height: options.height,
        }
    })
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    fn minimal_red_pdf() -> Vec<u8> {
        let objs: [&[u8]; 4] = [
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 16 16] /Contents 4 0 R /Resources << >> >>",
            b"<< /Length 23 >>\nstream\n1 0 0 rg 0 0 16 16 re f\nendstream",
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        out
    }

    #[test]
    #[ignore = "requires libpdfium in PDF_RASTER_PDFIUM (run via `just test-pdfium`)"]
    fn renders_minimal_pdf_across_threads_when_pdfium_present() {
        let pdfium = Pdfium::from_env().expect("PDF_RASTER_PDFIUM must load libpdfium");
        let pdf = minimal_red_pdf();
        assert_eq!(pdfium.page_count(&pdf, None).unwrap(), 1);
        assert_eq!(pdfium.page_size(&pdf, 0, None).unwrap(), (16.0, 16.0));

        thread::scope(|s| {
            let handles: Vec<_> = (0..4)
                .map(|t| {
                    let pdf_ref = &pdf;
                    s.spawn(move || {
                        let opts = RenderOptions::crisp_swatches(16, 16);
                        let img = if t % 2 == 0 {
                            pdfium.render_page(pdf_ref, 0, &opts).unwrap()
                        } else {
                            pdfium
                                .render_page_custom_access(pdf_ref, 0, None, &opts)
                                .unwrap()
                        };
                        assert_eq!(img.pixel(8, 8), Some([255, 0, 0, 255]));
                    })
                })
                .collect();
            for h in handles {
                h.join().unwrap();
            }
        });
    }
}
