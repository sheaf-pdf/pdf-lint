//! Safe [`RenderBackend`] adapter over [`pdfium_render`].

use std::collections::HashMap;
use std::env;
use std::fmt;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use pdf_gfx::{PageGeom, Rect, Rotation};
use pdfium_render::prelude::{
    PdfBitmap, PdfBitmapFormat, PdfDocument, PdfMatrix, PdfPage, PdfPageIndex,
    PdfPageRenderRotation, PdfRenderConfig, Pdfium as RawPdfium, PdfiumError, PdfiumInternalError,
    PdfiumLibraryBindings, Pixels,
};

use crate::backend::{BackendError, RenderBackend, RenderedDoc};
use crate::limits::Limits;
use crate::opts::{RenderMedium, RenderOpts, RenderSummary, Viewport};
use crate::pixmap::{PixelFormat, Pixmap};

/// Errors produced by [`PdfiumBackend`].
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
static GLOBAL_PDFIUM: OnceLock<&'static RawPdfium> = OnceLock::new();

fn try_init_global(
    bind: impl FnOnce() -> Result<Box<dyn PdfiumLibraryBindings>, Error>,
) -> Result<&'static RawPdfium, Error> {
    if let Some(&p) = GLOBAL_PDFIUM.get() {
        return Ok(p);
    }
    let _guard = INIT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(&p) = GLOBAL_PDFIUM.get() {
        return Ok(p);
    }
    let bindings = bind()?;
    let leaked: &'static RawPdfium = Box::leak(Box::new(RawPdfium::new(bindings)));
    let _ = GLOBAL_PDFIUM.set(leaked);
    Ok(*GLOBAL_PDFIUM.get().expect("just initialized"))
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
/// or `Arc<Pdfium>` (sharing the exact caller-provided handle and registering it
/// for reuse across the process), or bound dynamically via
/// [`PdfiumBackend::bind_to_library`], [`PdfiumBackend::bind_to_system_library`],
/// and [`PdfiumBackend::from_env`].
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
    /// Wraps an owned [`pdfium_render::prelude::Pdfium`] instance, keeping it
    /// live for the life of the process (never dropping any custom font provider
    /// attached to it) and registering it as the process-wide handle if none was
    /// registered yet.
    #[must_use]
    pub fn new(pdfium: RawPdfium) -> Self {
        let leaked: &'static RawPdfium = Box::leak(Box::new(pdfium));
        let _ = GLOBAL_PDFIUM.set(leaked);
        Self { pdfium: leaked }
    }

    /// Wraps a shared `Arc<pdfium_render::prelude::Pdfium>` instance so callers
    /// that already hold a `Pdfium` in `Arc` use that exact instance.
    #[must_use]
    pub fn from_arc(pdfium: Arc<RawPdfium>) -> Self {
        if let Some(&existing) = GLOBAL_PDFIUM.get()
            && std::ptr::eq(existing, pdfium.as_ref())
        {
            return Self { pdfium: existing };
        }
        let leaked_arc: &'static Arc<RawPdfium> = Box::leak(Box::new(pdfium));
        let leaked: &'static RawPdfium = leaked_arc;
        let _ = GLOBAL_PDFIUM.set(leaked);
        Self { pdfium: leaked }
    }

    /// Wraps an already-`'static` [`pdfium_render::prelude::Pdfium`] reference
    /// and registers it for process-wide reuse.
    #[must_use]
    pub fn from_static(pdfium: &'static RawPdfium) -> Self {
        let _ = GLOBAL_PDFIUM.set(pdfium);
        Self { pdfium }
    }

    /// Binds dynamically to `libpdfium` at `path` (or reuses the active
    /// `PdfiumBackend` handle if already initialized in this process).
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

    /// Binds dynamically to the OS system `libpdfium` library (or reuses the
    /// active handle if already initialized in this process).
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
        if matches!(opts.viewport, Viewport::Custom { .. }) && opts.draw.forms {
            return Err(BackendError::Unsupported(
                "PDFium cannot render interactive form widgets under Viewport::Custom",
            ));
        }

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
        let want_bgra = opts.format == PixelFormat::Bgra8;

        let mut cfg = PdfRenderConfig::new()
            .set_format(PdfBitmapFormat::BGRA)
            .set_reverse_byte_order(!want_bgra)
            .render_annotations(opts.draw.annots)
            .use_print_quality(opts.medium == RenderMedium::Print)
            .set_path_smoothing(opts.anti_alias.paths)
            .set_text_smoothing(opts.anti_alias.text)
            .set_image_smoothing(opts.anti_alias.images);

        cfg = configure_viewport_render(cfg, opts, &geom, &resolved, w_px, h_px)?;

        let pixmap = render_into_pixmap(
            &page,
            cfg,
            resolved.width,
            resolved.height,
            w_px,
            h_px,
            [br, bg, bb, ba],
            opts.format,
            &opts.limits,
        )?;
        Ok((pixmap, RenderSummary::unreported()))
    }
}

fn configure_viewport_render(
    mut cfg: PdfRenderConfig,
    opts: &RenderOpts,
    geom: &PageGeom,
    resolved: &crate::opts::ResolvedViewport,
    w_px: Pixels,
    h_px: Pixels,
) -> Result<PdfRenderConfig, BackendError> {
    match opts.viewport {
        Viewport::Page { rotation, .. } | Viewport::Dpi { rotation, .. } if opts.draw.forms => {
            Ok(cfg
                .set_fixed_size(w_px, h_px)
                .rotate(to_pdfium_rotation(rotation), false)
                .render_form_data(true))
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
            Ok(cfg
                .set_fixed_size(pw, ph)
                .set_origin(ox, oy)
                .rotate(to_pdfium_rotation(rotation), false)
                .render_form_data(true))
        }
        _ => {
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
            Ok(cfg)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn render_into_pixmap(
    page: &PdfPage<'_>,
    cfg: PdfRenderConfig,
    width: u32,
    height: u32,
    w_px: Pixels,
    h_px: Pixels,
    [br, bg, bb, ba]: [u8; 4],
    format: PixelFormat,
    limits: &Limits,
) -> Result<Pixmap, BackendError> {
    let pixel_count = (width as usize) * (height as usize);
    let fill = if format == PixelFormat::Bgra8 {
        [bb, bg, br, ba]
    } else {
        [br, bg, bb, ba]
    };
    // Pre-fill a single tightly packed pixel buffer directly in Rust so overhang
    // tiles are cleared cleanly and `Rgba8` / `Bgra8` require zero extra copies!
    let mut buf = vec![0_u8; pixel_count * 4];
    for px in buf.as_chunks_mut::<4>().0 {
        *px = fill;
    }
    let cfg = cfg.clear_before_rendering(false);
    {
        let mut bitmap =
            PdfBitmap::from_bytes(w_px, h_px, PdfBitmapFormat::BGRA, buf.as_mut_slice())
                .map_err(BackendError::custom)?;
        page.render_into_bitmap_with_config(&mut bitmap, &cfg)
            .map_err(BackendError::custom)?;
    }
    if format == PixelFormat::Rgba8Premul {
        for px in buf.as_chunks_mut::<4>().0 {
            let a = u16::from(px[3]);
            px[0] = ((u16::from(px[0]) * a + 127) / 255) as u8;
            px[1] = ((u16::from(px[1]) * a + 127) / 255) as u8;
            px[2] = ((u16::from(px[2]) * a + 127) / 255) as u8;
        }
    }
    Pixmap::from_bytes(width, height, format, buf, limits).map_err(BackendError::Raster)
}

#[cfg(target_arch = "wasm32")]
#[allow(clippy::too_many_arguments)]
fn render_into_pixmap(
    page: &PdfPage<'_>,
    cfg: PdfRenderConfig,
    width: u32,
    height: u32,
    w_px: Pixels,
    h_px: Pixels,
    [br, bg, bb, ba]: [u8; 4],
    format: PixelFormat,
    limits: &Limits,
) -> Result<Pixmap, BackendError> {
    let cfg = cfg
        .set_reverse_byte_order(true)
        .clear_before_rendering(true)
        .set_clear_color(pdfium_render::prelude::PdfColor::new(br, bg, bb, ba));
    let mut bitmap =
        PdfBitmap::empty(w_px, h_px, PdfBitmapFormat::BGRA).map_err(BackendError::custom)?;
    page.render_into_bitmap_with_config(&mut bitmap, &cfg)
        .map_err(BackendError::custom)?;
    let mut buf = bitmap.as_rgba_bytes();
    match format {
        PixelFormat::Rgba8 => {}
        PixelFormat::Bgra8 => {
            for px in buf.as_chunks_mut::<4>().0 {
                px.swap(0, 2);
            }
        }
        PixelFormat::Rgba8Premul => {
            for px in buf.as_chunks_mut::<4>().0 {
                let a = u16::from(px[3]);
                px[0] = ((u16::from(px[0]) * a + 127) / 255) as u8;
                px[1] = ((u16::from(px[1]) * a + 127) / 255) as u8;
                px[2] = ((u16::from(px[2]) * a + 127) / 255) as u8;
            }
        }
    }
    Pixmap::from_bytes(width, height, format, buf, limits).map_err(BackendError::Raster)
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

    // `FPDF_GetPageBoundingBox` (`boundaries().bounding()`) resolves inherited
    // `/MediaBox` and `/CropBox` entries up the `/Pages` tree, whereas
    // `FPDFPage_GetMediaBox` (`boundaries().media()`) inspects the leaf `/Page`
    // dictionary alone.
    let effective_box = page
        .boundaries()
        .bounding()
        .ok()
        .map(|b| {
            Rect::new(
                b.bounds.left().value,
                b.bounds.bottom().value,
                b.bounds.right().value,
                b.bounds.top().value,
            )
        })
        .unwrap_or_else(|| {
            let (unrot_w, unrot_h) = match rotation {
                Rotation::R90 | Rotation::R270 => (pdfium_h, pdfium_w),
                Rotation::R0 | Rotation::R180 => (pdfium_w, pdfium_h),
            };
            Rect::new(0.0, 0.0, unrot_w, unrot_h)
        });

    let media_box = page.boundaries().media().ok().map_or(effective_box, |b| {
        Rect::new(
            b.bounds.left().value,
            b.bounds.bottom().value,
            b.bounds.right().value,
            b.bounds.top().value,
        )
    });

    PageGeom::new(media_box, Some(effective_box), rotation)
}
