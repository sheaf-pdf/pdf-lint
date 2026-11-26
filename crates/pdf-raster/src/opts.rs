//! Viewport resolution, overlay selection, anti-aliasing policy, and rendering
//! options ([`Viewport`], [`Draw`], [`Appearances`], [`RenderOpts`],
//! [`RenderSummary`]).

use pdf_gfx::{Matrix, PageGeom, Rotation};
use pdf_objects::pdf_tokenizer::Rules;

use crate::error::Error;
use crate::limits::Limits;
use crate::pixmap::{PixelFormat, check_pixmap_budget};

/// Resolved integer pixel dimensions and `PageSpace -> DeviceSpace` matrix
/// returned by [`Viewport::resolve`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedViewport {
    /// Target bitmap width in pixels (`>= 1`).
    pub width: u32,
    /// Target bitmap height in pixels (`>= 1`).
    pub height: u32,
    /// Exact affine matrix mapping PDF PageSpace (`y` up, before `/Rotate`) onto
    /// device pixel coordinates (`(0, 0)` at top-left, `y` down).
    pub page_to_device: Matrix,
}

/// Target region and scale to rasterize — shaped directly around viewer full-page
/// and tile jobs (`sheaf`) as well as DPI and custom affine views.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Viewport {
    /// Whole visible page scaled to `width` device pixels after applying extra
    /// clockwise `rotation`, computing
    /// `height = (width * shown.height / shown.width).round().max(1.0)`.
    Page {
        /// Target pixel width (`>= 1`).
        width: u32,
        /// Extra clockwise rotation added to the page's `/Rotate`.
        rotation: Rotation,
    },
    /// Sub-rectangle `(x, y, w, h)` of the full page scaled to
    /// `(page_width, page_height)` after extra clockwise `rotation`.
    Tile {
        /// Virtual full-page pixel width (`>= 1`).
        page_width: u32,
        /// Virtual full-page pixel height (`>= 1`).
        page_height: u32,
        /// Tile left offset in virtual page pixels.
        x: u32,
        /// Tile top offset in virtual page pixels.
        y: u32,
        /// Tile width in pixels (`>= 1`).
        w: u32,
        /// Tile height in pixels (`>= 1`).
        h: u32,
        /// Extra clockwise rotation added to the page's `/Rotate`.
        rotation: Rotation,
    },
    /// Dots per inch (`72.0` = 1 pixel per 1/72-inch point, honoring page
    /// `/UserUnit`), with extra clockwise `rotation`.
    Dpi {
        /// Resolution in dots per inch (`> 0.0`).
        dpi: f32,
        /// Extra clockwise rotation added to the page's `/Rotate`.
        rotation: Rotation,
    },
    /// Explicit pixel buffer dimensions and `PageSpace -> DeviceSpace` matrix.
    Custom {
        /// Target pixel width (`>= 1`).
        width: u32,
        /// Target pixel height (`>= 1`).
        height: u32,
        /// Transform from PageSpace onto `[0..width] × [0..height]` (`y` down).
        page_to_device: Matrix,
    },
}

impl Default for Viewport {
    fn default() -> Self {
        Self::Dpi {
            dpi: 72.0,
            rotation: Rotation::R0,
        }
    }
}

impl Viewport {
    /// Resolves pixel dimensions and the closed-form `PageSpace -> DeviceSpace`
    /// [`Matrix`] from `geom`, checking [`Limits`] before any buffer allocation.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidViewport`] on zero/non-finite dimensions or
    /// singular transforms, or [`Error::Limit`] when dimensions or byte budgets
    /// exceed `limits`.
    pub fn resolve(&self, geom: &PageGeom, limits: &Limits) -> Result<ResolvedViewport, Error> {
        match *self {
            Viewport::Page { width, rotation } => {
                if width == 0 {
                    return Err(Error::InvalidViewport("page viewport width must be >= 1"));
                }
                let mut rotated = *geom;
                rotated.rotation = geom.rotation.plus(rotation);
                let shown = rotated.view_size();
                if !(shown.width > 0.0
                    && shown.height > 0.0
                    && shown.width.is_finite()
                    && shown.height.is_finite())
                {
                    return Err(Error::InvalidViewport("page visible box has empty extent"));
                }
                let raw_h = (f64::from(width) * f64::from(shown.height) / f64::from(shown.width))
                    .round()
                    .max(1.0);
                if raw_h > f64::from(u32::MAX) {
                    return Err(Error::Limit(crate::limits::LimitKind::Dimensions));
                }
                let height = raw_h as u32;
                let _ = check_pixmap_budget(width, height, limits)?;
                let sx = (width as f32) / shown.width;
                let sy = (height as f32) / shown.height;
                let page_to_device = rotated.to_view().then(Matrix::scale(sx, sy));
                if !page_to_device.is_finite() || page_to_device.determinant() == 0.0 {
                    return Err(Error::InvalidViewport("singular page viewport matrix"));
                }
                Ok(ResolvedViewport {
                    width,
                    height,
                    page_to_device,
                })
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
                if page_width == 0 || page_height == 0 || w == 0 || h == 0 {
                    return Err(Error::InvalidViewport(
                        "tile and page dimensions must be >= 1",
                    ));
                }
                let _ = check_pixmap_budget(w, h, limits)?;
                let mut rotated = *geom;
                rotated.rotation = geom.rotation.plus(rotation);
                let shown = rotated.view_size();
                if !(shown.width > 0.0 && shown.height > 0.0) {
                    return Err(Error::InvalidViewport("page visible box has empty extent"));
                }
                let sx = (page_width as f32) / shown.width;
                let sy = (page_height as f32) / shown.height;
                let page_to_device = rotated
                    .to_view()
                    .then(Matrix::scale(sx, sy))
                    .then(Matrix::translate(-(x as f32), -(y as f32)));
                if !page_to_device.is_finite() || page_to_device.determinant() == 0.0 {
                    return Err(Error::InvalidViewport("singular tile viewport matrix"));
                }
                Ok(ResolvedViewport {
                    width: w,
                    height: h,
                    page_to_device,
                })
            }
            Viewport::Dpi { dpi, rotation } => {
                if !(dpi > 0.0 && dpi.is_finite()) {
                    return Err(Error::InvalidViewport("DPI must be positive and finite"));
                }
                let mut rotated = *geom;
                rotated.rotation = geom.rotation.plus(rotation);
                let shown = rotated.view_size();
                if !(shown.width > 0.0 && shown.height > 0.0) {
                    return Err(Error::InvalidViewport("page visible box has empty extent"));
                }
                let unit_scale = f64::from(rotated.user_unit) * f64::from(dpi) / 72.0;
                let raw_w = (f64::from(shown.width) * unit_scale).round().max(1.0);
                let raw_h = (f64::from(shown.height) * unit_scale).round().max(1.0);
                if raw_w > f64::from(u32::MAX) || raw_h > f64::from(u32::MAX) {
                    return Err(Error::Limit(crate::limits::LimitKind::Dimensions));
                }
                let (width, height) = (raw_w as u32, raw_h as u32);
                let _ = check_pixmap_budget(width, height, limits)?;
                let sx = (width as f32) / shown.width;
                let sy = (height as f32) / shown.height;
                let page_to_device = rotated.to_view().then(Matrix::scale(sx, sy));
                if !page_to_device.is_finite() || page_to_device.determinant() == 0.0 {
                    return Err(Error::InvalidViewport("singular DPI viewport matrix"));
                }
                Ok(ResolvedViewport {
                    width,
                    height,
                    page_to_device,
                })
            }
            Viewport::Custom {
                width,
                height,
                page_to_device,
            } => {
                let _ = check_pixmap_budget(width, height, limits)?;
                if !page_to_device.is_finite() || page_to_device.determinant() == 0.0 {
                    return Err(Error::InvalidViewport(
                        "custom page_to_device matrix must be finite and non-singular",
                    ));
                }
                Ok(ResolvedViewport {
                    width,
                    height,
                    page_to_device,
                })
            }
        }
    }
}

/// Which overlays above the page's `/Contents` stream are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Draw {
    /// Non-widget `/Annots` (`FPDF_ANNOT`).
    pub annots: bool,
    /// `/Subtype /Widget` interactive form annotations (`FPDF_FFLDraw` /
    /// `pdf-forms`).
    pub forms: bool,
}

impl Default for Draw {
    fn default() -> Self {
        Self::ALL
    }
}

impl Draw {
    /// Draw page content, non-widget annotations, and interactive form widgets.
    pub const ALL: Self = Self {
        annots: true,
        forms: true,
    };
    /// Draw only the page's `/Contents` stream (skipping all `/Annots`).
    pub const CONTENT_ONLY: Self = Self {
        annots: false,
        forms: false,
    };
}

/// How annotations and widgets lacking a written `/AP /N` stream are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Appearances {
    /// Synthesize missing `/AP /N` streams on the fly via `pdf-annots` and
    /// `pdf-forms` without mutating the document (viewer default).
    #[default]
    Synthesize,
    /// Draw only written `/AP /N` streams (and require explicit `/AS` on
    /// multi-state widgets).
    WrittenOnly,
}

/// Target output medium controlling annotation flags (`Print` vs `NoView`) and
/// optional-content (`/OCProperties`) intent/usage evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RenderMedium {
    /// Screen viewing (`Annot::is_visible_on_screen`).
    #[default]
    Display,
    /// Print output (`Annot::is_printable`, `FPDF_PRINTING`).
    Print,
}

/// Per-category anti-aliasing / smoothing switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AntiAlias {
    /// Analytic area coverage on filled and stroked vector paths.
    pub paths: bool,
    /// Analytic area coverage on glyph outlines.
    pub text: bool,
    /// Bilinear interpolation on images whose `/Interpolate` flag is `true`.
    pub images: bool,
}

impl Default for AntiAlias {
    fn default() -> Self {
        Self::ALL
    }
}

impl AntiAlias {
    /// Smooth paths, text, and `/Interpolate true` images (viewer default).
    pub const ALL: Self = Self {
        paths: true,
        text: true,
        images: true,
    };
    /// Crisp aliased coverage (`FPDF_RENDER_NO_SMOOTH*`) for deterministic
    /// geometric and colorimetric testing.
    pub const NONE: Self = Self {
        paths: false,
        text: false,
        images: false,
    };
}

/// Complete configuration for rasterizing a PDF page or content stream.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RenderOpts {
    /// Target viewport and resolution.
    pub viewport: Viewport,
    /// Display vs. print medium.
    pub medium: RenderMedium,
    /// Initial backdrop color in straight `[R, G, B, A]` (`Some([255, 255, 255, 255])`
    /// opaque white by default; `None` for transparent `[0, 0, 0, 0]`).
    pub background: Option<[u8; 4]>,
    /// Output pixel channel layout.
    pub format: PixelFormat,
    /// Annotation and form-widget overlay selection.
    pub draw: Draw,
    /// Policy for annotations/widgets without `/AP /N`.
    pub appearances: Appearances,
    /// Anti-aliasing switches.
    pub anti_alias: AntiAlias,
    /// Whether to apply TrueType outline hinting at the effective device pixel
    /// size (`true` by default).
    pub hint_fonts: bool,
    /// Strict ISO 32000-2 (`Rules::Iso`) or viewer (`Rules::Pdfium`) semantics.
    pub rules: Rules,
    /// Resource limits enforced across interpretation, image decoding, color
    /// evaluation, and layer allocation.
    pub limits: Limits,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self {
            viewport: Viewport::default(),
            medium: RenderMedium::Display,
            background: Some([255, 255, 255, 255]),
            format: PixelFormat::Rgba8,
            draw: Draw::ALL,
            appearances: Appearances::Synthesize,
            anti_alias: AntiAlias::ALL,
            hint_fonts: true,
            rules: Rules::Pdfium,
            limits: Limits::DEFAULT,
        }
    }
}

impl RenderOpts {
    /// Creates [`RenderOpts`] for `viewport` with viewer defaults (`Rules::Pdfium`,
    /// opaque white background, `PixelFormat::Rgba8`, `Draw::ALL`).
    #[must_use]
    pub fn with_viewport(viewport: Viewport) -> Self {
        Self {
            viewport,
            ..Self::default()
        }
    }
}

/// Aggregate work and approximation metrics returned alongside a rendered
/// [`crate::Pixmap`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct RenderSummary {
    /// Underlying `pdf-gfx` content-stream interpretation summary.
    pub gfx: pdf_gfx::Summary,
    /// Image XObjects and inline images decoded and painted.
    pub images_drawn: usize,
    /// Shading fills (`sh` and `PatternType 2`) painted.
    pub shadings_drawn: usize,
    /// Tiling pattern fills/strokes (`PatternType 1`) painted.
    pub patterns_drawn: usize,
    /// Non-widget annotations drawn above the page content.
    pub annots_drawn: usize,
    /// Interactive form widget annotations drawn above the page content.
    pub forms_drawn: usize,
    /// Glyphs rendered using a fallback or built-in substitute font because the
    /// PDF font omitted an embedded font program.
    pub substituted_glyphs: usize,
    /// `true` if any font metric/program was approximated or substituted, or if
    /// a damaged image/pattern/soft-mask was skipped.
    pub approximated: bool,
}
