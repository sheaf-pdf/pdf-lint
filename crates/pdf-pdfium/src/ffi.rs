//! Runtime-loaded C FFI bindings to `libpdfium` (`chromium/8152`), serialized behind a single
//! process-wide `Mutex`.
//!
//! Upstream PDFium is not thread-safe even across distinct `FPDF_DOCUMENT` handles (`fpdfview.h`).
//! [`Pdfium`] acquires a global lock around every library call so parallel Rust test runners never
//! race inside `libpdfium`.

use std::env;
use std::ffi::{CStr, CString, c_char, c_double, c_float, c_int, c_uchar, c_uint, c_ulong, c_void};
use std::ops::{BitAnd, BitOr, BitOrAssign};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::{Mutex, OnceLock};

use libloading::Library;

use crate::image::{RgbaImage, checked_pixel_count};

/// Errors produced by [`Pdfium`], [`RgbaImage`], or [`crate::rubric`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// `libpdfium` could not be located in `$PDF_LINT_PDFIUM`, `$PDFIUM_LIB_DIR`, or `target/`.
    #[error(
        "libpdfium not found (set PDF_LINT_PDFIUM or run `just _pdfium`); searched: {searched}"
    )]
    LibraryNotFound {
        /// Human-readable list of candidate locations checked.
        searched: String,
    },
    /// Opening `libpdfium` or resolving a required C symbol failed.
    #[error("failed to load libpdfium from {path}: {reason}")]
    LibraryLoad {
        /// Path attempted.
        path: PathBuf,
        /// Loader diagnostic.
        reason: String,
    },
    /// PDFium rejected a document on open (`FPDF_GetLastError`).
    #[error("PDFium failed to load document (FPDF_ERR code {code})")]
    DocumentLoad {
        /// Error code returned by `FPDF_GetLastError`.
        code: u64,
    },
    /// Requested page index failed to load (`FPDF_LoadPage` returned null).
    #[error("PDFium failed to load page {page_index}")]
    PageLoad {
        /// Zero-based page index.
        page_index: usize,
    },
    /// `FPDFBitmap_CreateEx` returned null or dimensions were invalid.
    #[error("invalid or unsupported bitmap dimensions {width}x{height}")]
    InvalidDimensions {
        /// Requested width in pixels.
        width: u32,
        /// Requested height in pixels.
        height: u32,
    },
    /// Two images passed to [`crate::score_images`] had different pixel dimensions.
    #[error(
        "image dimensions differ: candidate is {cand_w}x{cand_h}, reference is {ref_w}x{ref_h}"
    )]
    DimensionMismatch {
        /// Candidate width.
        cand_w: u32,
        /// Candidate height.
        cand_h: u32,
        /// Reference width.
        ref_w: u32,
        /// Reference height.
        ref_h: u32,
    },
    /// Password contained an interior NUL byte that cannot cross C FFI.
    #[error("password contains an interior NUL byte")]
    InvalidPassword,
}

/// Bitflags forwarded to `FPDF_RenderPageBitmap` / `FPDF_RenderPageBitmapWithMatrix`.
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
    /// Whether to run PDFium's `FPDF_FFLDraw` pass to render interactive form widgets.
    pub forms: bool,
    /// Optional explicit `[a, b, c, d, e, f]` transform and `[left, top, right, bottom]` clip rect
    /// for `FPDF_RenderPageBitmapWithMatrix` (overrides default whole-page fit when `Some`).
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

#[repr(C)]
struct FpdfLibraryConfig {
    version: c_int,
    user_font_paths: *const *const c_char,
    isolate: *mut c_void,
    v8_embedder_slot: c_uint,
    platform: *mut c_void,
    renderer_type: c_int,
    font_library_type: c_int,
    brotli_enabled: c_int,
    isolate_per_document: c_int,
}

#[repr(C)]
struct FsMatrix {
    a: c_float,
    b: c_float,
    c: c_float,
    d: c_float,
    e: c_float,
    f: c_float,
}

#[repr(C)]
struct FsRectF {
    left: c_float,
    top: c_float,
    right: c_float,
    bottom: c_float,
}

#[repr(C)]
struct FpdfFileAccess {
    file_len: c_ulong,
    get_block: unsafe extern "C" fn(
        param: *mut c_void,
        position: c_ulong,
        buf: *mut c_uchar,
        size: c_ulong,
    ) -> c_int,
    param: *mut c_void,
}

/// `FPDF_FORMFILLINFO` at version 2 (`fpdf_formfill.h`), with XFA disabled and callbacks null.
#[repr(C)]
struct FpdfFormFillInfo {
    version: c_int,
    v1_callbacks: [*const c_void; 15],
    js_platform: *mut c_void,
    xfa_disabled: c_int,
    v2_callbacks: [*const c_void; 17],
}

impl Default for FpdfFormFillInfo {
    fn default() -> Self {
        Self {
            version: 2,
            v1_callbacks: [ptr::null(); 15],
            js_platform: ptr::null_mut(),
            xfa_disabled: 1,
            v2_callbacks: [ptr::null(); 17],
        }
    }
}

const FPDF_BITMAP_BGRA: c_int = 4;

#[allow(clippy::type_complexity)]
struct Symbols {
    // Keep the dlopen handle alive for the lifetime of the process so symbols stay valid.
    _lib: Library,
    path: PathBuf,
    init_with_config: unsafe extern "C" fn(*const FpdfLibraryConfig),
    get_last_error: unsafe extern "C" fn() -> c_ulong,
    load_mem_document64: unsafe extern "C" fn(*const c_void, usize, *const c_char) -> *mut c_void,
    load_custom_document: unsafe extern "C" fn(*mut FpdfFileAccess, *const c_char) -> *mut c_void,
    close_document: unsafe extern "C" fn(*mut c_void),
    get_page_count: unsafe extern "C" fn(*mut c_void) -> c_int,
    load_page: unsafe extern "C" fn(*mut c_void, c_int) -> *mut c_void,
    close_page: unsafe extern "C" fn(*mut c_void),
    get_page_width_f: unsafe extern "C" fn(*mut c_void) -> c_float,
    get_page_height_f: unsafe extern "C" fn(*mut c_void) -> c_float,
    bitmap_create_ex: unsafe extern "C" fn(c_int, c_int, c_int, *mut c_void, c_int) -> *mut c_void,
    bitmap_fill_rect:
        unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int, c_int, c_ulong) -> c_int,
    bitmap_get_buffer: unsafe extern "C" fn(*mut c_void) -> *mut u8,
    bitmap_get_stride: unsafe extern "C" fn(*mut c_void) -> c_int,
    bitmap_destroy: unsafe extern "C" fn(*mut c_void),
    render_page_bitmap:
        unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, c_int, c_int, c_int, c_int, c_int),
    render_page_bitmap_with_matrix:
        unsafe extern "C" fn(*mut c_void, *mut c_void, *const FsMatrix, *const FsRectF, c_int),
    init_form_fill: unsafe extern "C" fn(*mut c_void, *mut FpdfFormFillInfo) -> *mut c_void,
    exit_form_fill: unsafe extern "C" fn(*mut c_void),
    form_after_load_page: unsafe extern "C" fn(*mut c_void, *mut c_void),
    form_before_close_page: unsafe extern "C" fn(*mut c_void, *mut c_void),
    ffl_draw: unsafe extern "C" fn(
        *mut c_void,
        *mut c_void,
        *mut c_void,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
    ),
    page_to_device: unsafe extern "C" fn(
        *mut c_void,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        c_double,
        c_double,
        *mut c_int,
        *mut c_int,
    ) -> c_int,
}

// SAFETY: `Symbols` is only ever accessed through `GlobalState`'s `Mutex`, ensuring strictly
// single-threaded access across the entire process as required by `fpdfview.h`.
unsafe impl Send for Symbols {}

impl Symbols {
    fn load(path: PathBuf) -> Result<Symbols, Error> {
        // SAFETY: Loading the user/CI-supplied `libpdfium` shared library.
        let lib = unsafe { Library::new(&path) }.map_err(|e| Error::LibraryLoad {
            path: path.clone(),
            reason: e.to_string(),
        })?;

        macro_rules! sym {
            ($name:literal) => {{
                // SAFETY: Symbol signature matches PDFium's C header (`fpdfview.h` / `fpdf_formfill.h`),
                // and `_lib` is retained in `Symbols` for `'static` so function pointers stay valid.
                let s = unsafe { lib.get::<*const c_void>($name) }.map_err(|e| {
                    Error::LibraryLoad {
                        path: path.clone(),
                        reason: format!("missing symbol {}: {e}", CStr::from_bytes_with_nul($name).unwrap_or_default().to_string_lossy()),
                    }
                })?;
                unsafe { std::mem::transmute_copy(&*s) }
            }};
        }

        let init_with_config: unsafe extern "C" fn(*const FpdfLibraryConfig) =
            sym!(b"FPDF_InitLibraryWithConfig\0");
        let get_last_error = sym!(b"FPDF_GetLastError\0");
        let load_mem_document64 = sym!(b"FPDF_LoadMemDocument64\0");
        let load_custom_document = sym!(b"FPDF_LoadCustomDocument\0");
        let close_document = sym!(b"FPDF_CloseDocument\0");
        let get_page_count = sym!(b"FPDF_GetPageCount\0");
        let load_page = sym!(b"FPDF_LoadPage\0");
        let close_page = sym!(b"FPDF_ClosePage\0");
        let get_page_width_f = sym!(b"FPDF_GetPageWidthF\0");
        let get_page_height_f = sym!(b"FPDF_GetPageHeightF\0");
        let bitmap_create_ex = sym!(b"FPDFBitmap_CreateEx\0");
        let bitmap_fill_rect = sym!(b"FPDFBitmap_FillRect\0");
        let bitmap_get_buffer = sym!(b"FPDFBitmap_GetBuffer\0");
        let bitmap_get_stride = sym!(b"FPDFBitmap_GetStride\0");
        let bitmap_destroy = sym!(b"FPDFBitmap_Destroy\0");
        let render_page_bitmap = sym!(b"FPDF_RenderPageBitmap\0");
        let render_page_bitmap_with_matrix = sym!(b"FPDF_RenderPageBitmapWithMatrix\0");
        let init_form_fill = sym!(b"FPDFDOC_InitFormFillEnvironment\0");
        let exit_form_fill = sym!(b"FPDFDOC_ExitFormFillEnvironment\0");
        let form_after_load_page = sym!(b"FORM_OnAfterLoadPage\0");
        let form_before_close_page = sym!(b"FORM_OnBeforeClosePage\0");
        let ffl_draw = sym!(b"FPDF_FFLDraw\0");
        let page_to_device = sym!(b"FPDF_PageToDevice\0");

        let config = FpdfLibraryConfig {
            version: 2,
            user_font_paths: ptr::null(),
            isolate: ptr::null_mut(),
            v8_embedder_slot: 0,
            platform: ptr::null_mut(),
            renderer_type: 0,
            font_library_type: 0,
            brotli_enabled: 0,
            isolate_per_document: 0,
        };
        // SAFETY: Called once inside `OnceLock` initialization before any other PDFium call.
        unsafe { (init_with_config)(&raw const config) };

        Ok(Symbols {
            _lib: lib,
            path,
            init_with_config,
            get_last_error,
            load_mem_document64,
            load_custom_document,
            close_document,
            get_page_count,
            load_page,
            close_page,
            get_page_width_f,
            get_page_height_f,
            bitmap_create_ex,
            bitmap_fill_rect,
            bitmap_get_buffer,
            bitmap_get_stride,
            bitmap_destroy,
            render_page_bitmap,
            render_page_bitmap_with_matrix,
            init_form_fill,
            exit_form_fill,
            form_after_load_page,
            form_before_close_page,
            ffl_draw,
            page_to_device,
        })
    }
}

static GLOBAL_PDFIUM: OnceLock<Result<Mutex<Symbols>, Error>> = OnceLock::new();

fn platform_lib_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else if cfg!(target_os = "windows") {
        "pdfium.dll"
    } else {
        "libpdfium.so"
    }
}

fn locate_library() -> Result<PathBuf, Error> {
    if let Some(explicit) = env::var_os("PDF_LINT_PDFIUM")
        && !explicit.is_empty()
    {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Ok(p);
        }
        return Err(Error::LibraryLoad {
            path: p,
            reason: "PDF_LINT_PDFIUM does not point to an existing file".to_string(),
        });
    }

    let mut searched = Vec::new();
    let lib_name = platform_lib_name();

    if let Some(dir) = env::var_os("PDFIUM_LIB_DIR")
        && !dir.is_empty()
    {
        let candidate = PathBuf::from(dir).join(lib_name);
        if candidate.is_file() {
            return Ok(candidate);
        }
        searched.push(candidate.display().to_string());
    }

    let workspace_target = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/pdfium-8152/lib")
        .join(lib_name);
    if workspace_target.is_file() {
        return Ok(workspace_target);
    }
    searched.push(workspace_target.display().to_string());

    Err(Error::LibraryNotFound {
        searched: searched.join(", "),
    })
}

/// Thread-safe handle to the process-wide `libpdfium` instance.
///
/// Copying or cloning [`Pdfium`] is zero-cost; all methods serialize calls through a single
/// global `Mutex` so parallel test execution is completely race-free.
#[derive(Clone, Copy)]
pub struct Pdfium {
    _private: (),
}

impl std::fmt::Debug for Pdfium {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pdfium")
            .field("library", &self.library_path())
            .finish()
    }
}

impl Pdfium {
    /// Loads (or reuses) the process-wide `libpdfium` library.
    ///
    /// Search order:
    /// 1. `$PDF_LINT_PDFIUM` (exact path; fails immediately if set to a missing/invalid file).
    /// 2. `$PDFIUM_LIB_DIR/<libpdfium>`.
    /// 3. `<workspace>/target/pdfium-8152/lib/<libpdfium>` (populated by `just _pdfium`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::LibraryNotFound`] or [`Error::LibraryLoad`] if `libpdfium` cannot be
    /// found or initialized.
    pub fn try_global() -> Result<Pdfium, Error> {
        let res =
            GLOBAL_PDFIUM.get_or_init(|| locate_library().and_then(Symbols::load).map(Mutex::new));
        match res {
            Ok(_) => Ok(Pdfium { _private: () }),
            Err(e) => Err(e.clone()),
        }
    }

    /// Returns `true` if `libpdfium` is available and initialized.
    #[must_use]
    pub fn is_available() -> bool {
        Self::try_global().is_ok()
    }

    /// Path to the loaded `libpdfium` binary.
    #[must_use]
    pub fn library_path(&self) -> PathBuf {
        let guard = self.lock();
        guard.path.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'static, Symbols> {
        let mutex = GLOBAL_PDFIUM
            .get()
            .expect("Pdfium handle only constructed after OnceLock succeeds")
            .as_ref()
            .expect("Pdfium handle only constructed on Ok");
        // Recover from poison if an assertion panicked in another test while not mutating C state.
        mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Returns the number of pages in `pdf` (`FPDF_GetPageCount`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::DocumentLoad`] if PDFium refuses to open `pdf`.
    pub fn page_count(&self, pdf: &[u8], password: Option<&str>) -> Result<usize, Error> {
        let c_pw = make_password(password)?;
        let syms = self.lock();
        with_mem_doc(&syms, pdf, c_pw.as_deref(), |syms, doc| {
            // SAFETY: `doc` is a valid `FPDF_DOCUMENT` under `syms` lock.
            let n = unsafe { (syms.get_page_count)(doc) };
            Ok(usize::try_from(n).unwrap_or(0))
        })
    }

    /// Returns `(width_pt, height_pt)` of `page_index` in PDF points (`1/72` inch).
    ///
    /// # Errors
    ///
    /// Returns [`Error::DocumentLoad`] or [`Error::PageLoad`] on failure.
    pub fn page_size(
        &self,
        pdf: &[u8],
        page_index: usize,
        password: Option<&str>,
    ) -> Result<(f32, f32), Error> {
        let c_pw = make_password(password)?;
        let syms = self.lock();
        with_mem_doc(&syms, pdf, c_pw.as_deref(), |syms, doc| {
            let idx = c_int::try_from(page_index).map_err(|_| Error::PageLoad { page_index })?;
            // SAFETY: `doc` is valid and `close_page` runs before `close_document`.
            let page = unsafe { (syms.load_page)(doc, idx) };
            if page.is_null() {
                return Err(Error::PageLoad { page_index });
            }
            let w = unsafe { (syms.get_page_width_f)(page) };
            let h = unsafe { (syms.get_page_height_f)(page) };
            let _ = (syms.page_to_device, syms.init_with_config);
            unsafe { (syms.close_page)(page) };
            Ok((w, h))
        })
    }

    /// Renders `page_index` from an in-memory `pdf` slice via `FPDF_LoadMemDocument64`.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if dimensions are invalid, the document fails to open, or the page
    /// fails to load.
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
    /// Returns [`Error`] on invalid dimensions/password, document open failure, or page load
    /// failure.
    pub fn render_page_with_password(
        &self,
        pdf: &[u8],
        page_index: usize,
        password: Option<&str>,
        options: &RenderOptions,
    ) -> Result<RgbaImage, Error> {
        let _ = checked_pixel_count(options.width, options.height)?;
        let c_pw = make_password(password)?;
        let syms = self.lock();
        with_mem_doc(&syms, pdf, c_pw.as_deref(), |syms, doc| {
            render_loaded_doc(syms, doc, page_index, options)
        })
    }

    /// Renders `page_index` through `FPDF_LoadCustomDocument` (`FPDF_FILEACCESS` callback),
    /// exercising PDFium's positional read path rather than contiguous buffer loading.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] on invalid dimensions/password, document open failure, or page load
    /// failure.
    pub fn render_page_custom_access(
        &self,
        pdf: &[u8],
        page_index: usize,
        password: Option<&str>,
        options: &RenderOptions,
    ) -> Result<RgbaImage, Error> {
        let _ = checked_pixel_count(options.width, options.height)?;
        let c_pw = make_password(password)?;
        let file_len = c_ulong::try_from(pdf.len()).map_err(|_| Error::DocumentLoad { code: 2 })?;
        let syms = self.lock();

        struct SliceSource<'a>(&'a [u8]);
        unsafe extern "C" fn get_block(
            param: *mut c_void,
            position: c_ulong,
            buf: *mut c_uchar,
            size: c_ulong,
        ) -> c_int {
            // SAFETY: `param` points to `SliceSource` on the stack for the duration of `doc`.
            let src = unsafe { &*(param.cast::<SliceSource<'_>>()) }.0;
            let Ok(pos) = usize::try_from(position) else {
                return 0;
            };
            let Ok(len) = usize::try_from(size) else {
                return 0;
            };
            let Some(end) = pos.checked_add(len) else {
                return 0;
            };
            let Some(slice) = src.get(pos..end) else {
                return 0;
            };
            // SAFETY: PDFium guarantees `buf` is valid for `size` bytes (`fpdfview.h`).
            unsafe { ptr::copy_nonoverlapping(slice.as_ptr(), buf, len) };
            1
        }

        let mut source = SliceSource(pdf);
        let mut access = FpdfFileAccess {
            file_len,
            get_block,
            param: (&raw mut source).cast::<c_void>(),
        };
        let pw_ptr = c_pw.as_ref().map_or(ptr::null(), |s| s.as_ptr());
        // SAFETY: `source` outlives `doc`, which is closed before returning.
        let doc = unsafe { (syms.load_custom_document)(&raw mut access, pw_ptr) };
        if doc.is_null() {
            let code = unsafe { (syms.get_last_error)() };
            return Err(Error::DocumentLoad {
                #[allow(clippy::unnecessary_cast)]
                code: code as u64,
            });
        }
        let result = render_loaded_doc(&syms, doc, page_index, options);
        unsafe { (syms.close_document)(doc) };
        result
    }
}

fn make_password(password: Option<&str>) -> Result<Option<CString>, Error> {
    password
        .map(|pw| CString::new(pw).map_err(|_| Error::InvalidPassword))
        .transpose()
}

fn with_mem_doc<T>(
    syms: &Symbols,
    pdf: &[u8],
    password: Option<&CStr>,
    f: impl FnOnce(&Symbols, *mut c_void) -> Result<T, Error>,
) -> Result<T, Error> {
    let data_ptr = if pdf.is_empty() {
        ptr::null()
    } else {
        pdf.as_ptr().cast::<c_void>()
    };
    let pw_ptr = password.map_or(ptr::null(), CStr::as_ptr);
    // SAFETY: `pdf` remains borrowed and alive until `close_document` completes below.
    let doc = unsafe { (syms.load_mem_document64)(data_ptr, pdf.len(), pw_ptr) };
    if doc.is_null() {
        let code = unsafe { (syms.get_last_error)() };
        return Err(Error::DocumentLoad {
            #[allow(clippy::unnecessary_cast)]
            code: code as u64,
        });
    }
    let out = f(syms, doc);
    // SAFETY: Every page/form handle opened inside `f` is closed before `f` returns.
    unsafe { (syms.close_document)(doc) };
    out
}

fn render_loaded_doc(
    syms: &Symbols,
    doc: *mut c_void,
    page_index: usize,
    options: &RenderOptions,
) -> Result<RgbaImage, Error> {
    let w_int = c_int::try_from(options.width).map_err(|_| Error::InvalidDimensions {
        width: options.width,
        height: options.height,
    })?;
    let h_int = c_int::try_from(options.height).map_err(|_| Error::InvalidDimensions {
        width: options.width,
        height: options.height,
    })?;
    let idx = c_int::try_from(page_index).map_err(|_| Error::PageLoad { page_index })?;

    let mut form_info = Box::new(FpdfFormFillInfo::default());
    let form_handle = if options.forms {
        // SAFETY: `form_info` stays heap-pinned until `exit_form_fill` runs before function exit.
        unsafe { (syms.init_form_fill)(doc, &raw mut *form_info) }
    } else {
        ptr::null_mut()
    };

    // SAFETY: `doc` is valid.
    let page = unsafe { (syms.load_page)(doc, idx) };
    if page.is_null() {
        if !form_handle.is_null() {
            unsafe { (syms.exit_form_fill)(form_handle) };
        }
        return Err(Error::PageLoad { page_index });
    }

    if !form_handle.is_null() {
        unsafe { (syms.form_after_load_page)(page, form_handle) };
    }

    // SAFETY: `w_int > 0`, `h_int > 0`, format 4 (`FPDFBitmap_BGRA` straight alpha).
    let bitmap =
        unsafe { (syms.bitmap_create_ex)(w_int, h_int, FPDF_BITMAP_BGRA, ptr::null_mut(), 0) };
    if bitmap.is_null() {
        if !form_handle.is_null() {
            unsafe { (syms.form_before_close_page)(page, form_handle) };
        }
        unsafe { (syms.close_page)(page) };
        if !form_handle.is_null() {
            unsafe { (syms.exit_form_fill)(form_handle) };
        }
        return Err(Error::InvalidDimensions {
            width: options.width,
            height: options.height,
        });
    }

    let [r, g, b, a] = options.background;
    let argb = (c_ulong::from(a) << 24)
        | (c_ulong::from(r) << 16)
        | (c_ulong::from(g) << 8)
        | c_ulong::from(b);
    let rotate = c_int::from(options.rotate & 3);
    let flags = options.flags.bits() as c_int;

    // SAFETY: `bitmap` and `page` are valid non-null handles.
    unsafe {
        (syms.bitmap_fill_rect)(bitmap, 0, 0, w_int, h_int, argb);
        if let Some(([ma, mb, mc, md, me, mf], [cl, ct, cr, cb])) = options.matrix_and_clip {
            let matrix = FsMatrix {
                a: ma,
                b: mb,
                c: mc,
                d: md,
                e: me,
                f: mf,
            };
            let clip = FsRectF {
                left: cl,
                top: ct,
                right: cr,
                bottom: cb,
            };
            (syms.render_page_bitmap_with_matrix)(
                bitmap,
                page,
                &raw const matrix,
                &raw const clip,
                flags,
            );
        } else {
            (syms.render_page_bitmap)(bitmap, page, 0, 0, w_int, h_int, rotate, flags);
        }
        if !form_handle.is_null() {
            (syms.ffl_draw)(form_handle, bitmap, page, 0, 0, w_int, h_int, rotate, flags);
        }
    }

    // Copy and swizzle BGRA -> RGBA before destroying `bitmap`.
    let out = copy_bgra_to_rgba(syms, bitmap, options.width, options.height);

    // Tear down in strict reverse order: bitmap -> form_before_close_page -> page -> form_env.
    unsafe {
        (syms.bitmap_destroy)(bitmap);
        if !form_handle.is_null() {
            (syms.form_before_close_page)(page, form_handle);
        }
        (syms.close_page)(page);
        if !form_handle.is_null() {
            (syms.exit_form_fill)(form_handle);
        }
    }

    out
}

fn copy_bgra_to_rgba(
    syms: &Symbols,
    bitmap: *mut c_void,
    width: u32,
    height: u32,
) -> Result<RgbaImage, Error> {
    // SAFETY: `bitmap` is a live `FPDFBitmap_BGRA` handle created with `width x height`.
    let buf_ptr = unsafe { (syms.bitmap_get_buffer)(bitmap) };
    let stride = unsafe { (syms.bitmap_get_stride)(bitmap) };
    let w = width as usize;
    let h = height as usize;
    let Ok(stride) = usize::try_from(stride) else {
        return Err(Error::InvalidDimensions { width, height });
    };
    if buf_ptr.is_null() || stride < w * 4 {
        return Err(Error::InvalidDimensions { width, height });
    }
    let total_src = stride
        .checked_mul(h)
        .ok_or(Error::InvalidDimensions { width, height })?;
    // SAFETY: PDFium allocated `height * stride` initialized bytes at `buf_ptr`.
    let src = unsafe { std::slice::from_raw_parts(buf_ptr, total_src) };
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = &src[y * stride..y * stride + w * 4];
        for &[b, g, r, a] in row.as_chunks::<4>().0 {
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    RgbaImage::from_rgba(width, height, rgba)
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
            b"<< /Length 22 >>\nstream\n1 0 0 rg 0 0 16 16 re f\nendstream",
        ];
        // Recompute exact stream length for object 4: `1 0 0 rg 0 0 16 16 re f` is 23 bytes.
        let objs: [&[u8]; 4] = [
            objs[0],
            objs[1],
            objs[2],
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
    fn renders_minimal_pdf_across_threads_when_pdfium_present() {
        let Ok(pdfium) = Pdfium::try_global() else {
            // Allow machines without target/pdfium-8152 unpacked to pass plain `cargo test` unless
            // PDF_LINT_PDFIUM was explicitly set (which `try_global` already fails on).
            assert!(env::var_os("PDF_LINT_PDFIUM").is_none());
            return;
        };
        let pdf = minimal_red_pdf();
        assert_eq!(pdfium.page_count(&pdf, None).unwrap(), 1);
        assert_eq!(pdfium.page_size(&pdf, 0, None).unwrap(), (16.0, 16.0));

        // Exercise concurrent rendering across 4 threads to verify global Mutex serialization.
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
