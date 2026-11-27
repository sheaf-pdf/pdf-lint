//! Safe [`RenderBackend`] adapter over [`pdfium_render`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use parking_lot::{FairMutex, FairMutexGuard};
use pdf_gfx::{PageGeom, Rect, Rotation};
use pdfium_render::prelude::{
    PdfBitmap, PdfBitmapFormat, PdfDocument, PdfMatrix, PdfPage, PdfPageIndex,
    PdfPageRenderRotation, PdfRenderConfig, Pdfium as RawPdfium, PdfiumError, PdfiumInternalError,
    PdfiumLibraryBindings, Pixels,
};

use crate::backend::{BackendError, Refusal, RenderBackend, RenderedDoc};
use crate::engine::check_cancelled;
use crate::limits::Limits;
use crate::opts::{RenderMedium, RenderOpts, RenderSummary, Viewport};
use crate::pixmap::{PixelFormat, Pixmap};
use crate::source::RenderSource;

/// Why PDFium refused a document on open: `FPDF_GetLastError`'s code after a
/// failed load, named ([`Error::DocumentLoad`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LoadFailure {
    /// `FPDF_ERR_FILE`: the file wasn't found or its bytes didn't read.
    File,
    /// `FPDF_ERR_FORMAT`: not a PDF, or one too damaged for PDFium to read.
    Format,
    /// `FPDF_ERR_PASSWORD`: a password is needed, or the one given is wrong.
    Password,
    /// `FPDF_ERR_SECURITY`: a security handler PDFium doesn't support.
    Security,
    /// `FPDF_ERR_PAGE`: a page wasn't found, or its content didn't read.
    Page,
    /// `FPDF_ERR_UNKNOWN`: PDFium named no reason.
    Unknown,
}

impl LoadFailure {
    /// What `pdfium-render` read of `FPDF_GetLastError`, as this.
    const fn of(err: &PdfiumInternalError) -> Self {
        match err {
            PdfiumInternalError::FileError => Self::File,
            PdfiumInternalError::FormatError => Self::Format,
            PdfiumInternalError::PasswordError => Self::Password,
            PdfiumInternalError::SecurityError => Self::Security,
            PdfiumInternalError::PageError => Self::Page,
            PdfiumInternalError::Unknown => Self::Unknown,
        }
    }

    /// The refusal a host sees in [`BackendError::Refused`]; none for a
    /// password, which is [`BackendError::Locked`].
    #[must_use]
    pub const fn refusal(self) -> Option<Refusal> {
        match self {
            Self::Password => None,
            Self::Format => Some(Refusal::Format),
            Self::Security => Some(Refusal::Security),
            Self::File => Some(Refusal::Source),
            Self::Page | Self::Unknown => Some(Refusal::Unknown),
        }
    }
}

impl fmt::Display for LoadFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::File => "a file error (FPDF_ERR_FILE)",
            Self::Format => "a format error (FPDF_ERR_FORMAT)",
            Self::Password => "a password required or incorrect (FPDF_ERR_PASSWORD)",
            Self::Security => "an unsupported security scheme (FPDF_ERR_SECURITY)",
            Self::Page => "a page error (FPDF_ERR_PAGE)",
            Self::Unknown => "an unknown error (FPDF_ERR_UNKNOWN)",
        })
    }
}

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
    /// PDFium refused a document on open, and why, as its
    /// `FPDF_GetLastError` said. A host matching on [`BackendError`] sees
    /// [`LoadFailure::Password`] as [`BackendError::Locked`] and the rest as
    /// [`BackendError::Refused`], this its source.
    #[error("PDFium refused the document: {reason}")]
    DocumentLoad {
        /// PDFium's reason.
        reason: LoadFailure,
    },
    /// A document's open failed in `pdfium-render` before or apart from
    /// PDFium's own verdict on the file: the adapter's failure, not the
    /// file's.
    #[error("pdfium-render failed to open the document: {detail}")]
    Binding {
        /// Diagnostic from `pdfium-render`.
        detail: String,
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

/// The one lock over PDFium in the process ([`PdfiumBackend::lock`]): fair,
/// so threads take it in the order they asked.
static PDFIUM_LOCK: FairMutex<()> = parking_lot::const_fair_mutex(());

/// What this thread holds of [`PDFIUM_LOCK`]: the guard, and how many
/// [`PdfiumLock`]s share it, so the lock is released when the last is dropped
/// in whatever order they are.
struct Held {
    depth: usize,
    guard: Option<FairMutexGuard<'static, ()>>,
}

thread_local! {
    static HELD: RefCell<Held> = const { RefCell::new(Held { depth: 0, guard: None }) };
}

/// The process-wide PDFium lock, held: [`PdfiumBackend::lock`] gives it, and
/// dropping it gives the lock up.
///
/// It's held by a thread, so it isn't `Send`; a thread that holds one may
/// take more (each of the adapter's own calls does), and the lock is free
/// again once the thread has dropped them all.
#[must_use = "the lock is given up when this is dropped"]
pub struct PdfiumLock {
    /// Held by the thread that took it: neither `Send` nor `Sync`.
    _thread: PhantomData<*const ()>,
}

impl fmt::Debug for PdfiumLock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PdfiumLock").finish()
    }
}

impl PdfiumLock {
    /// The lock, taken: at once where this thread holds it already, else
    /// after every thread that asked before.
    fn take() -> Self {
        HELD.with(|held| {
            let mut held = held.borrow_mut();
            if held.depth == 0 {
                held.guard = Some(PDFIUM_LOCK.lock());
            }
            held.depth += 1;
        });
        Self {
            _thread: PhantomData,
        }
    }
}

impl Drop for PdfiumLock {
    fn drop(&mut self) {
        // A thread's locals are gone only once the thread is ending, and
        // their own drop has given the lock up by then.
        let _ = HELD.try_with(|held| {
            let mut held = held.borrow_mut();
            held.depth = held.depth.saturating_sub(1);
            if held.depth == 0 {
                held.guard = None;
            }
        });
    }
}

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

/// A failed load, as the adapter's error says it.
fn map_doc_err(err: PdfiumError) -> Error {
    match err {
        PdfiumError::CStringConversionError(_) => Error::InvalidPassword,
        PdfiumError::PdfiumLibraryInternalError(internal) => Error::DocumentLoad {
            reason: LoadFailure::of(&internal),
        },
        other => Error::Binding {
            detail: other.to_string(),
        },
    }
}

/// A failed load, as a host matches on it: a password asked for, the file
/// refused and why, or the adapter's own failure.
fn open_err(err: Error) -> BackendError {
    match err {
        Error::DocumentLoad { reason } => match reason.refusal() {
            None => BackendError::Locked,
            Some(refusal) => BackendError::refused(refusal, err),
        },
        other => BackendError::custom(other),
    }
}

/// A [`RenderSource`] as PDFium reads a file: by `Read` and `Seek`, each of
/// its block requests one read, filled whole (PDFium takes a short count for
/// a failed read).
struct SourceReader {
    source: Arc<dyn RenderSource>,
    pos: u64,
}

impl Read for SourceReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = self.source.len();
        let Ok(start) = usize::try_from(self.pos) else {
            return Ok(0);
        };
        if start >= len || buf.is_empty() {
            return Ok(0);
        }
        let n = buf.len().min(len - start);
        let read = self.source.read_at(start..start + n)?;
        if read.len() != n {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the source gave fewer bytes than it was asked for",
            ));
        }
        buf[..n].copy_from_slice(&read);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for SourceReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(pos) => Some(pos),
            SeekFrom::End(by) => (self.source.len() as u64).checked_add_signed(by),
            SeekFrom::Current(by) => self.pos.checked_add_signed(by),
        };
        self.pos = pos.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "a seek to before the start")
        })?;
        Ok(self.pos)
    }
}

/// Thin [`RenderBackend`] adapter over [`pdfium_render::prelude::Pdfium`].
///
/// Can be constructed directly from an existing [`pdfium_render::prelude::Pdfium`]
/// or `Arc<Pdfium>` (sharing the exact caller-provided handle and registering it
/// for reuse across the process), or bound dynamically via
/// [`PdfiumBackend::bind_to_library`], [`PdfiumBackend::bind_to_system_library`],
/// and [`PdfiumBackend::from_env`].
///
/// # Locking
///
/// PDFium keeps state for the whole process, so two threads inside it at
/// once corrupt each other, whichever documents they use; `pdfium-render`'s
/// `thread_safe` feature locks one library call at a time, which leaves a
/// sequence of calls (a load and the error read after it, a page loaded,
/// drawn and closed) open to another thread between them. So the adapter
/// holds one process-wide lock, [`PdfiumBackend::lock`], across each of its
/// operations whole: an open, a page's geometry, a page's render, and a
/// document's drop. A [`RenderedDoc`] it opens is `Send + Sync` and safe to
/// use from any thread; renders through PDFium run one at a time.
///
/// A host that calls PDFium itself through [`PdfiumBackend::pdfium`] (text,
/// a flatten, a save) holds the same lock around those calls:
///
/// ```no_run
/// # use pdf_raster::pdfium::PdfiumBackend;
/// # fn main() -> Result<(), pdf_raster::pdfium::Error> {
/// let backend = PdfiumBackend::from_env()?;
/// {
///     let _pdfium = backend.lock();
///     // Calls on `backend.pdfium()` and what it returns, and drops of
///     // what they return, go here.
/// }
/// # Ok(())
/// # }
/// ```
///
/// The lock is fair (threads take it in the order they asked, so a long walk
/// that gives it up between pages lets a waiting render in) and a thread may
/// take it again while it holds it, so the adapter's calls can be made under
/// it. A host that made its `Pdfium` itself and gave it to
/// [`PdfiumBackend::new`] has one library and one lock as long as every call
/// it makes on that `Pdfium` is under this lock. A [`RenderSource`] an open
/// was given is read inside PDFium's calls, under the lock: its `read_at`
/// must not call PDFium or wait on a thread that does.
///
/// # Page Geometry Notes
///
/// PDFium's C API resolves inherited `/MediaBox` and `/CropBox` entries via
/// `FPDF_GetPageBoundingBox` (`crop_box`), whereas `FPDFPage_GetMediaBox` reads
/// the leaf `/Page` dictionary alone and PDFium ignores `/UserUnit`:
/// [`RenderedDoc::page_geom`] therefore populates `crop_box` from the effective
/// bounding box, falls back to `crop_box` for `media_box` when `/MediaBox` is
/// inherited from an ancestor `/Pages` node, and sets `user_unit = 1.0`.
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

    /// Underlying [`pdfium_render::prelude::Pdfium`] handle, for a host's
    /// own PDFium calls: each made, and each object they return used and
    /// dropped, under [`PdfiumBackend::lock`].
    #[must_use]
    pub const fn pdfium(&self) -> &'static RawPdfium {
        self.pdfium
    }

    /// Takes the process-wide PDFium lock, waiting behind every thread that
    /// asked before, and holds it until the returned [`PdfiumLock`] is
    /// dropped: what every operation of the adapter holds, and what a host
    /// holds around its own calls on [`PdfiumBackend::pdfium`]. There's one
    /// lock whichever `PdfiumBackend` is asked. A thread that holds it
    /// already takes it again at once.
    pub fn lock(&self) -> PdfiumLock {
        PdfiumLock::take()
    }

    /// The document `reader` gives, loaded under the lock.
    fn load<R: Read + Seek + Send + 'static>(
        &self,
        reader: R,
        password: Option<&str>,
    ) -> Result<Box<dyn RenderedDoc>, BackendError> {
        if password.is_some_and(|pw| pw.contains('\0')) {
            return Err(BackendError::custom(Error::InvalidPassword));
        }
        // Held from the load to the page count, so `FPDF_GetLastError` reads
        // this load's code and no other thread's; taken before `doc` is
        // made, so a `doc` dropped on the way out is dropped under it.
        let _pdfium = self.lock();
        let doc = self
            .pdfium
            .load_pdf_from_reader(reader, password)
            .map_err(|err| open_err(map_doc_err(err)))?;
        let page_count = usize::try_from(doc.pages().len()).unwrap_or(0);
        Ok(Box::new(PdfiumRenderedDoc {
            doc: Some(doc),
            page_count,
            geoms: Mutex::new(HashMap::new()),
        }))
    }
}

struct PdfiumRenderedDoc {
    /// The document, there until the drop, which closes it under the lock.
    doc: Option<PdfDocument<'static>>,
    page_count: usize,
    geoms: Mutex<HashMap<usize, PageGeom>>,
}

impl Drop for PdfiumRenderedDoc {
    fn drop(&mut self) {
        let _pdfium = PdfiumLock::take();
        self.doc = None;
    }
}

impl PdfiumRenderedDoc {
    fn doc(&self) -> &PdfDocument<'static> {
        self.doc.as_ref().expect("the document is held until drop")
    }

    /// Page `page_index`, loaded. The caller holds the lock, from before
    /// this call until the page is dropped.
    fn page(&self, page_index: usize) -> Result<PdfPage<'_>, BackendError> {
        let out_of_bounds = || BackendError::PageOutOfBounds {
            page_index,
            page_count: self.page_count,
        };
        if page_index >= self.page_count {
            return Err(out_of_bounds());
        }
        let idx = PdfPageIndex::try_from(page_index).map_err(|_| out_of_bounds())?;
        self.doc().pages().get(idx).map_err(|e| {
            BackendError::custom(Error::PageLoad {
                page_index,
                detail: e.to_string(),
            })
        })
    }

    /// A page's geometry as it was read before, if it was.
    fn known_geom(&self, page_index: usize) -> Option<PageGeom> {
        let geoms = self
            .geoms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        geoms.get(&page_index).copied()
    }

    /// `page`'s geometry, read and kept. The caller holds the lock.
    fn read_geom(&self, page_index: usize, page: &PdfPage<'_>) -> PageGeom {
        if let Some(geom) = self.known_geom(page_index) {
            return geom;
        }
        let geom = extract_pdf_page_geom(page);
        let mut geoms = self
            .geoms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        geoms.insert(page_index, geom);
        geom
    }
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
        if let Some(geom) = self.known_geom(page_index) {
            return Ok(geom);
        }
        let _pdfium = PdfiumLock::take();
        let page = self.page(page_index)?;
        Ok(self.read_geom(page_index, &page))
    }

    fn render_page(
        &self,
        page_index: usize,
        opts: &RenderOpts,
    ) -> Result<(Pixmap, RenderSummary), BackendError> {
        check_cancelled(opts).map_err(BackendError::Raster)?;
        if matches!(opts.viewport, Viewport::Custom { .. }) && opts.draw.forms {
            return Err(BackendError::Unsupported(
                "PDFium cannot render interactive form widgets under Viewport::Custom",
            ));
        }

        if page_index >= self.page_count {
            return Err(BackendError::PageOutOfBounds {
                page_index,
                page_count: self.page_count,
            });
        }

        // Held from the page's load to its drop at the end of the render.
        let _pdfium = PdfiumLock::take();
        // A render that waited its turn may have been cancelled meanwhile.
        check_cancelled(opts).map_err(BackendError::Raster)?;
        let page = self.page(page_index)?;
        let geom = self.read_geom(page_index, &page);
        // PDFium always ignores `/UserUnit`, so resolve `Viewport::Dpi` under
        // PDFium's unscaled user-unit policy (`Rules::Pdfium`).
        let resolved = opts
            .viewport
            .resolve_with_rules(
                &geom,
                pdf_objects::pdf_tokenizer::Rules::Pdfium,
                &opts.limits,
            )
            .map_err(BackendError::Raster)?;

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
    let byte_len = pixel_count * 4;
    let mut buf = if fill == [0, 0, 0, 0] {
        vec![0_u8; byte_len]
    } else if fill == [255, 255, 255, 255] {
        vec![255_u8; byte_len]
    } else {
        let mut v = vec![0_u8; byte_len];
        for px in v.as_chunks_mut::<4>().0 {
            *px = fill;
        }
        v
    };
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
        self.load(Cursor::new(bytes), password)
    }

    /// Opens `source` where it is, through PDFium's custom file access:
    /// PDFium asks for the blocks it needs as it needs them, for as long as
    /// the document lives, and nothing copies the whole.
    fn open_source(
        &self,
        source: Arc<dyn RenderSource>,
        password: Option<&str>,
    ) -> Result<Box<dyn RenderedDoc>, BackendError> {
        self.load(SourceReader { source, pos: 0 }, password)
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::*;
    use crate::source::Parts;

    fn internal(err: PdfiumInternalError) -> BackendError {
        open_err(map_doc_err(PdfiumError::PdfiumLibraryInternalError(err)))
    }

    #[test]
    fn a_failed_load_says_whose_trouble_it_is() {
        // A password: asked for, not a refusal.
        assert!(matches!(
            internal(PdfiumInternalError::PasswordError),
            BackendError::Locked
        ));
        assert_eq!(LoadFailure::Password.refusal(), None);

        // The file's own trouble: refused, named, PDFium's code its source.
        for (code, refusal, failure) in [
            (
                PdfiumInternalError::FormatError,
                Refusal::Format,
                LoadFailure::Format,
            ),
            (
                PdfiumInternalError::SecurityError,
                Refusal::Security,
                LoadFailure::Security,
            ),
            (
                PdfiumInternalError::FileError,
                Refusal::Source,
                LoadFailure::File,
            ),
            (
                PdfiumInternalError::PageError,
                Refusal::Unknown,
                LoadFailure::Page,
            ),
            (
                PdfiumInternalError::Unknown,
                Refusal::Unknown,
                LoadFailure::Unknown,
            ),
        ] {
            let err = internal(code);
            assert_eq!(err.refusal(), Some(refusal));
            let BackendError::Refused { source, .. } = &err else {
                panic!("{err}");
            };
            assert_eq!(
                source.downcast_ref::<Error>(),
                Some(&Error::DocumentLoad { reason: failure })
            );
        }
        assert_eq!(
            internal(PdfiumInternalError::FormatError).to_string(),
            "the document was refused: it isn't a PDF the engine can read"
        );
        assert_eq!(
            Error::DocumentLoad {
                reason: LoadFailure::Security
            }
            .to_string(),
            "PDFium refused the document: an unsupported security scheme (FPDF_ERR_SECURITY)"
        );

        // The adapter's own: neither.
        let own = open_err(map_doc_err(PdfiumError::NoPagesInDocument));
        let BackendError::Backend(boxed) = &own else {
            panic!("{own}");
        };
        assert!(matches!(
            boxed.downcast_ref::<Error>(),
            Some(Error::Binding { .. })
        ));
    }

    #[test]
    fn a_source_reads_as_a_file_by_read_and_seek() {
        let whole: Vec<u8> = (0..200).collect();
        let parts = Parts::new(vec![
            Arc::from(&whole[..50]),
            Arc::from(&whole[50..51]),
            Arc::from(&whole[51..]),
        ]);
        let mut reader = SourceReader {
            source: Arc::new(parts),
            pos: 0,
        };
        // A block across every part is filled whole by one read.
        let mut block = [0_u8; 120];
        assert_eq!(reader.seek(SeekFrom::Start(40)).unwrap(), 40);
        assert_eq!(reader.read(&mut block).unwrap(), 120);
        assert_eq!(block[..], whole[40..160]);
        // The last block is as long as the bytes left; then none.
        assert_eq!(reader.read(&mut block).unwrap(), 40);
        assert_eq!(block[..40], whole[160..]);
        assert_eq!(reader.read(&mut block).unwrap(), 0);
        // From the end, as PDFium looks for the trailer; and past it.
        assert_eq!(reader.seek(SeekFrom::End(-8)).unwrap(), 192);
        assert_eq!(reader.seek(SeekFrom::Current(-2)).unwrap(), 190);
        let mut rest = Vec::new();
        reader.read_to_end(&mut rest).unwrap();
        assert_eq!(rest, whole[190..]);
        assert_eq!(reader.seek(SeekFrom::End(10)).unwrap(), 210);
        assert_eq!(reader.read(&mut block).unwrap(), 0);
        assert!(reader.seek(SeekFrom::End(-201)).is_err());
    }

    #[test]
    fn a_sources_error_is_the_reads() {
        struct Failing;
        impl RenderSource for Failing {
            fn len(&self) -> usize {
                64
            }
            fn read_at(&self, _: std::ops::Range<usize>) -> io::Result<std::borrow::Cow<'_, [u8]>> {
                Err(io::Error::other("gone"))
            }
        }
        let mut reader = SourceReader {
            source: Arc::new(Failing),
            pos: 0,
        };
        assert_eq!(reader.read(&mut [0; 8]).unwrap_err().to_string(), "gone");
    }

    #[test]
    fn the_lock_is_one_threads_at_a_time_and_a_thread_takes_it_again() {
        // A thread that holds it takes it again, and it's free only once
        // every hold is dropped, in whatever order.
        let outer = PdfiumLock::take();
        let inner = PdfiumLock::take();
        drop(outer);

        let inside = Arc::new(AtomicBool::new(false));
        let (asked, was_asked) = mpsc::channel();
        let waiter = {
            let inside = Arc::clone(&inside);
            thread::spawn(move || {
                asked.send(()).unwrap();
                let _lock = PdfiumLock::take();
                inside.store(true, Ordering::SeqCst);
            })
        };
        was_asked.recv().unwrap();
        thread::sleep(Duration::from_millis(50));
        assert!(!inside.load(Ordering::SeqCst), "taken while it was held");
        drop(inner);
        waiter.join().unwrap();
        assert!(inside.load(Ordering::SeqCst));

        // Many threads, each holding it twice over: never two inside.
        let within = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let within = Arc::clone(&within);
                thread::spawn(move || {
                    for _ in 0..200 {
                        let _a = PdfiumLock::take();
                        let _b = PdfiumLock::take();
                        assert_eq!(within.fetch_add(1, Ordering::SeqCst), 0);
                        thread::yield_now();
                        within.fetch_sub(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
    }
}
