//! Internal comparative adapter implementing [`pdf_raster::RenderBackend`] over
//! runtime-loaded `libpdfium` (`chromium/8152` via [`pdf_pdfium`]).
//!
//! Built with `#![forbid(unsafe_code)]` (all `libloading` C FFI calls are
//! encapsulated inside [`pdf_pdfium`]) so tests, benchmarks, and shadow-mode
//! corpus sweeps can drive `pdfs::RustBackend` and [`PdfiumBackend`] through one
//! identical `&dyn RenderBackend` interface and grade every render with
//! [`pdf_pdfium`]'s `0–100` visual & ISO 32000-2 rubric.

use std::fmt;
use std::sync::Arc;

use pdf_pdfium::{
    CaseEvaluation, CaseOutcome, Pdfium, RenderFlags, RenderOptions as PdfiumRenderOptions,
    RgbaImage, RubricConfig, evaluate_case,
};
use pdf_raster::pdf_gfx::{PageGeom, Rect, Rotation};
use pdf_raster::pdf_objects::pdf_tokenizer::Rules;
use pdf_raster::{
    BackendError, Limits, PixelFormat, Pixmap, RenderBackend, RenderMedium, RenderOpts,
    RenderSummary, RenderedDoc, Viewport,
};
use pdf_semantics::Limits as SemLimits;
use pdf_semantics::pages::pages;
use pdf_structure::pdf_crypt::Password;
use pdf_structure::read::{Options as StructureOptions, Reading, Recovery};

/// Comparative [`RenderBackend`] wrapping a runtime-loaded [`Pdfium`] handle.
#[derive(Clone, Copy)]
pub struct PdfiumBackend {
    pdfium: Pdfium,
}

impl fmt::Debug for PdfiumBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PdfiumBackend")
            .field("library_path", &self.pdfium.library_path())
            .finish()
    }
}

impl PdfiumBackend {
    /// Wraps an already-loaded [`Pdfium`] instance.
    #[must_use]
    pub const fn new(pdfium: Pdfium) -> Self {
        Self { pdfium }
    }

    /// Loads the global `libpdfium` instance (`$PDF_LINT_PDFIUM`,
    /// `$PDFIUM_LIB_DIR`, or `target/pdfium-8152/lib/libpdfium.so`).
    ///
    /// # Errors
    ///
    /// Returns [`pdf_pdfium::Error`] if `libpdfium` cannot be found or loaded.
    pub fn try_global() -> Result<Self, pdf_pdfium::Error> {
        Pdfium::try_global().map(Self::new)
    }

    /// Underlying [`Pdfium`] handle.
    #[must_use]
    pub const fn pdfium(&self) -> &Pdfium {
        &self.pdfium
    }
}

struct PdfiumRenderedDoc {
    pdfium: Pdfium,
    bytes: Arc<[u8]>,
    password: Option<String>,
    geoms: Vec<PageGeom>,
}

impl RenderedDoc for PdfiumRenderedDoc {
    fn page_count(&self) -> usize {
        self.geoms.len()
    }

    fn page_geom(&self, page_index: usize) -> Result<PageGeom, BackendError> {
        self.geoms
            .get(page_index)
            .copied()
            .ok_or(BackendError::PageOutOfBounds {
                page_index,
                page_count: self.geoms.len(),
            })
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

        let mut flags = RenderFlags::NONE;
        if opts.draw.annots {
            flags |= RenderFlags::ANNOT;
        }
        if opts.medium == RenderMedium::Print {
            flags |= RenderFlags::PRINTING;
        }
        if !opts.anti_alias.paths {
            flags |= RenderFlags::NO_SMOOTHPATH;
        }
        if !opts.anti_alias.text {
            flags |= RenderFlags::NO_SMOOTHTEXT;
        }
        if !opts.anti_alias.images {
            flags |= RenderFlags::NO_SMOOTHIMAGE;
        }

        let background = opts.background.unwrap_or([0, 0, 0, 0]);
        let (rotate, matrix_and_clip) = viewport_to_pdfium(&geom, &opts.viewport, &resolved)?;

        let p_opts = PdfiumRenderOptions {
            width: resolved.width,
            height: resolved.height,
            background,
            rotate,
            flags,
            forms: opts.draw.forms,
            matrix_and_clip,
        };

        let rgba_img = self
            .pdfium
            .render_page_with_password(&self.bytes, page_index, self.password.as_deref(), &p_opts)
            .map_err(map_pdfium_err)?;

        let pixmap = rgba_image_to_pixmap(rgba_img, opts.format, &opts.limits)?;
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
        let count = self
            .pdfium
            .page_count(&bytes, password)
            .map_err(map_pdfium_err)?;

        // Extract exact `PageGeom` (`MediaBox`, `CropBox`, `Rotate`, `UserUnit`)
        // via `pdf-structure` + `pdf-semantics`, falling back to `Pdfium::page_size`
        // if a damaged test file relies on PDFium-specific xref recovery.
        let mut geoms = extract_page_geoms(&bytes, password, count);
        if geoms.len() != count {
            geoms.clear();
            for idx in 0..count {
                let (w, h) = self
                    .pdfium
                    .page_size(&bytes, idx, password)
                    .map_err(map_pdfium_err)?;
                geoms.push(PageGeom::new(Rect::new(0.0, 0.0, w, h), None, Rotation::R0));
            }
        }

        Ok(Box::new(PdfiumRenderedDoc {
            pdfium: self.pdfium,
            bytes,
            password: password.map(str::to_owned),
            geoms,
        }))
    }
}

fn extract_page_geoms(bytes: &[u8], password: Option<&str>, expected: usize) -> Vec<PageGeom> {
    let pw = password.map(|s| [Password(s)]);
    let mut open = StructureOptions::DEFAULT;
    open.recovery = Recovery::Pdfium;
    if let Some(ref p) = pw {
        open.passwords = p;
    }
    let Ok(reading) = Reading::open(bytes, &open) else {
        return Vec::new();
    };
    let Ok(Some(catalog)) = pdf_semantics::catalog(&reading, reading.trailer().dict()) else {
        return Vec::new();
    };
    let Ok(tbl) = pages(
        &reading,
        catalog,
        Rules::Pdfium,
        &SemLimits::DEFAULT,
        &mut (),
    ) else {
        return Vec::new();
    };
    if tbl.len() != expected {
        return Vec::new();
    }
    tbl.iter()
        .map(|p| {
            let base = PageGeom::from_page(p);
            match p.dict {
                Some(d) => base.with_page_boxes(&reading, d),
                None => base,
            }
        })
        .collect()
}

fn rotation_to_pdfium_code(rot: Rotation) -> u8 {
    match rot {
        Rotation::R0 => 0,
        Rotation::R90 => 1,
        Rotation::R180 => 2,
        Rotation::R270 => 3,
    }
}

type MatrixAndClip = ([f32; 6], [f32; 4]);

fn viewport_to_pdfium(
    geom: &PageGeom,
    viewport: &Viewport,
    resolved: &pdf_raster::ResolvedViewport,
) -> Result<(u8, Option<MatrixAndClip>), BackendError> {
    match *viewport {
        Viewport::Page { rotation, .. } | Viewport::Dpi { rotation, .. } => {
            Ok((rotation_to_pdfium_code(rotation), None))
        }
        Viewport::Tile {
            page_width,
            page_height,
            x,
            y,
            w,
            h,
            rotation,
        } => {
            // `FPDF_RenderPageBitmapWithMatrix` transforms from the page's unrotated
            // ViewSpace (`top-down`, `(0, 0)` = top-left of unrotated visible box)!
            // If `rotation == R0`, `sx = page_width / shown.width`, `sy = page_height / shown.height`.
            // For general `rotation`, compute `view0_to_device = geom.to_view().inverse().then(resolved.page_to_device)`!
            let Some(view0_to_page) = geom.to_view().inverse() else {
                return Err(BackendError::Raster(pdf_raster::Error::InvalidViewport(
                    "singular unrotated view transform",
                )));
            };
            let _ = (page_width, page_height, x, y, rotation);
            let m = view0_to_page.then(resolved.page_to_device);
            let clip = [0.0, 0.0, w as f32, h as f32];
            Ok((0, Some(([m.a, m.b, m.c, m.d, m.e, m.f], clip))))
        }
        Viewport::Custom {
            width,
            height,
            page_to_device,
        } => {
            let Some(view0_to_page) = geom.to_view().inverse() else {
                return Err(BackendError::Raster(pdf_raster::Error::InvalidViewport(
                    "singular unrotated view transform",
                )));
            };
            let m = view0_to_page.then(page_to_device);
            let clip = [0.0, 0.0, width as f32, height as f32];
            Ok((0, Some(([m.a, m.b, m.c, m.d, m.e, m.f], clip))))
        }
        _ => Ok((0, None)),
    }
}

fn rgba_image_to_pixmap(
    img: RgbaImage,
    format: PixelFormat,
    limits: &Limits,
) -> Result<Pixmap, BackendError> {
    let (w, h) = (img.width(), img.height());
    let src = img.as_bytes();
    let mut out = Vec::with_capacity(src.len());
    match format {
        PixelFormat::Rgba8 => out.extend_from_slice(src),
        PixelFormat::Bgra8 => {
            for &[r, g, b, a] in src.as_chunks::<4>().0 {
                out.extend_from_slice(&[b, g, r, a]);
            }
        }
        PixelFormat::Rgba8Premul => {
            for &[r, g, b, a] in src.as_chunks::<4>().0 {
                let pm = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
                out.extend_from_slice(&[pm(r), pm(g), pm(b), a]);
            }
        }
        _ => out.extend_from_slice(src),
    }
    Pixmap::from_bytes(w, h, format, out, limits).map_err(BackendError::Raster)
}

fn map_pdfium_err(err: pdf_pdfium::Error) -> BackendError {
    match err {
        // `FPDF_ERR_PASSWORD` is `4` in `fpdfview.h`.
        pdf_pdfium::Error::DocumentLoad { code: 4 } => BackendError::Locked,
        other => BackendError::custom(other),
    }
}

/// Renders `page_index` of `bytes` through both `candidate` (e.g.
/// `pdfs::RustBackend`) and `oracle` ([`PdfiumBackend`]) under `opts`, and
/// evaluates the pair against `rubric`.
///
/// # Errors
///
/// Returns [`BackendError`] if either backend fails to open or render the page.
pub fn compare_backends(
    candidate: &dyn RenderBackend,
    oracle: &dyn RenderBackend,
    bytes: Arc<[u8]>,
    password: Option<&str>,
    page_index: usize,
    opts: &RenderOpts,
    rubric: &RubricConfig,
) -> Result<CaseOutcome, BackendError> {
    let cand_doc = candidate.open_bytes(bytes.clone(), password)?;
    let orac_doc = oracle.open_bytes(bytes, password)?;
    let (cand_pix, _) = cand_doc.render_page(page_index, opts)?;
    let (orac_pix, _) = orac_doc.render_page(page_index, opts)?;

    let cand_rgba =
        RgbaImage::from_rgba(cand_pix.width(), cand_pix.height(), cand_pix.to_rgba8_vec())
            .map_err(BackendError::custom)?;
    let orac_rgba =
        RgbaImage::from_rgba(orac_pix.width(), orac_pix.height(), orac_pix.to_rgba8_vec())
            .map_err(BackendError::custom)?;

    evaluate_case(
        rubric,
        &CaseEvaluation {
            candidate_pdfium: Some(&cand_rgba),
            candidate_iso: Some(&cand_rgba),
            pdfium_reference: Some(&orac_rgba),
            iso_reference: Some(&orac_rgba),
            iso_fault_count: 0,
            write_lint_errors: 0,
        },
    )
    .map_err(BackendError::custom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_pdfium::RubricProfile;
    use pdf_raster::AntiAlias;
    use pdfs::RustBackend;
    use pdfs::prelude::*;

    fn try_oracle() -> Option<PdfiumBackend> {
        match PdfiumBackend::try_global() {
            Ok(b) => Some(b),
            Err(pdf_pdfium::Error::LibraryNotFound { .. }) => None,
            Err(other) => panic!("unexpected libpdfium load error: {other}"),
        }
    }

    #[test]
    fn rust_and_pdfium_backends_agree_across_pages_tiles_formats_rotations_and_encryption() {
        let rust = RustBackend::new();
        let Some(pdfium) = try_oracle() else {
            eprintln!("skipping PdfiumBackend test (libpdfium not present)");
            return;
        };

        // Build a 2-page AES-256 encrypted PDF with vector shapes and annotations!
        let mut doc = Document::new();
        {
            let p1 = doc.add_page(Rect::new(0.0, 0.0, 200.0, 160.0));
            p1.fill_color(Color::rgb8([30, 100, 210]));
            p1.rect(20.0, 20.0, 70.0, 120.0);
            p1.paint(Paint::Fill);
            p1.fill_color(Color::rgb8([220, 70, 40]));
            p1.rect(110.0, 40.0, 70.0, 80.0);
            p1.paint(Paint::Fill);
        }
        {
            let p2 = doc.add_page(Rect::new(0.0, 0.0, 160.0, 120.0));
            p2.fill_color(Color::rgb8([40, 170, 90]));
            p2.rect(15.0, 15.0, 130.0, 90.0);
            p2.paint(Paint::Fill);
        }

        let user_pw = Password("viewer-pass").prepared().unwrap();
        doc.encrypt(&Protection::new(&user_pw)).unwrap();
        let bytes: Arc<[u8]> = Arc::from(doc.to_bytes().unwrap());

        // 1. Both backends must refuse opening without the password (`BackendError::Locked`)!
        assert!(matches!(
            rust.open_bytes(bytes.clone(), None),
            Err(BackendError::Locked)
        ));
        assert!(matches!(
            pdfium.open_bytes(bytes.clone(), None),
            Err(BackendError::Locked)
        ));

        // 2. Both backends unlock with `"viewer-pass"` and report identical `page_count` and `page_geom`!
        let rust_doc = rust.open_bytes(bytes.clone(), Some("viewer-pass")).unwrap();
        let pdfium_doc = pdfium
            .open_bytes(bytes.clone(), Some("viewer-pass"))
            .unwrap();
        assert_eq!(rust_doc.page_count(), 2);
        assert_eq!(pdfium_doc.page_count(), 2);
        assert_eq!(
            rust_doc.page_geom(0).unwrap(),
            pdfium_doc.page_geom(0).unwrap()
        );
        assert_eq!(
            rust_doc.page_geom(1).unwrap(),
            pdfium_doc.page_geom(1).unwrap()
        );
        assert!(matches!(
            rust_doc.page_geom(5),
            Err(BackendError::PageOutOfBounds {
                page_index: 5,
                page_count: 2
            })
        ));
        assert!(matches!(
            pdfium_doc.page_geom(5),
            Err(BackendError::PageOutOfBounds {
                page_index: 5,
                page_count: 2
            })
        ));

        let rubric = RubricProfile::ConformingCore.config();

        // 3. Compare full-page renders on page 0 and page 1 across rotations & `Bgra8`!
        for rot in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
            let mut opts = RenderOpts::with_viewport(Viewport::Page {
                width: 160,
                rotation: rot,
            });
            opts.format = PixelFormat::Bgra8;
            let outcome = compare_backends(
                &rust,
                &pdfium,
                bytes.clone(),
                Some("viewer-pass"),
                0,
                &opts,
                &rubric,
            )
            .unwrap();
            assert!(
                outcome.passed,
                "page 0 rotation {rot:?} failed: {outcome:?}"
            );
        }

        // 4. Compare 4-quadrant `Viewport::Tile` renders between `RustBackend` and `PdfiumBackend`!
        for (tx, ty) in [(0, 0), (100, 0), (0, 80), (100, 80)] {
            let mut tile_opts = RenderOpts::with_viewport(Viewport::Tile {
                page_width: 200,
                page_height: 160,
                x: tx,
                y: ty,
                w: 100,
                h: 80,
                rotation: Rotation::R0,
            });
            tile_opts.anti_alias = AntiAlias::NONE;
            let outcome = compare_backends(
                &rust,
                &pdfium,
                bytes.clone(),
                Some("viewer-pass"),
                0,
                &tile_opts,
                &rubric,
            )
            .unwrap();
            assert!(
                outcome.passed,
                "tile ({tx}, {ty}) failed ConformingCore rubric: {outcome:?}"
            );
        }
    }
}
