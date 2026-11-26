//! Resource caps enforced before allocating framebuffers, offscreen group/mask
//! layers, tiling pattern cells, or shading meshes.

/// Which [`Limits`] cap refused a rendering operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LimitKind {
    /// Target viewport width or height exceeded [`Limits::max_width`] /
    /// [`Limits::max_height`] (or `u16::MAX`, `65,535`).
    Dimensions,
    /// Target viewport pixel count (`width * height`) exceeded
    /// [`Limits::max_pixels`].
    Pixels,
    /// Target framebuffer byte size (`width * height * 4`) exceeded
    /// [`Limits::max_pixmap_bytes`].
    PixmapBytes,
    /// Nested transparency group, soft-mask, tiling-pattern, or Type 3 recursion
    /// exceeded [`Limits::max_layer_depth`].
    LayerDepth,
    /// Tiling pattern cell dimensions or repeat count exceeded
    /// [`Limits::max_pattern_cells`].
    PatternCells,
    /// Shading function/mesh sample budget exceeded
    /// [`Limits::max_shading_samples`].
    ShadingSamples,
    /// Annotation count on the page exceeded [`Limits::max_annots`].
    Annotations,
}

/// Resource limits for page, Form XObject, and appearance rasterization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Maximum viewport or offscreen layer width in pixels (`<= 65,535`).
    pub max_width: u32,
    /// Maximum viewport or offscreen layer height in pixels (`<= 65,535`).
    pub max_height: u32,
    /// Maximum pixel count (`width * height`) per rendered pixmap.
    pub max_pixels: u64,
    /// Maximum framebuffer byte size (`width * height * 4`).
    pub max_pixmap_bytes: usize,
    /// Maximum nested layer/group/soft-mask/pattern/Type 3 recursion depth.
    pub max_layer_depth: usize,
    /// Maximum tiling-pattern repeat cells or tile side length.
    pub max_pattern_cells: u32,
    /// Maximum shading sample evaluations per shading paint.
    pub max_shading_samples: u32,
    /// Maximum annotations evaluated per page.
    pub max_annots: usize,
    /// Graphics interpreter limits (`pdf-gfx`).
    pub gfx: pdf_gfx::Limits,
    /// Image decoder limits (`pdf-images`).
    pub images: pdf_images::Limits,
    /// Color space & §7.10 function evaluation limits (`pdf-color`).
    pub color: pdf_color::Limits,
    /// Page-tree & optional-content limits (`pdf-semantics`).
    pub semantics: pdf_semantics::Limits,
    /// Annotation inspection & appearance limits (`pdf-annots`).
    pub annots: pdf_annots::Limits,
    /// Interactive form tree & widget appearance limits (`pdf-forms`).
    pub forms: pdf_forms::Limits,
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Limits {
    /// Default production resource caps (`16,384 × 16,384` max side,
    /// `67,108,864` max pixels, `256 MiB` max framebuffer).
    pub const DEFAULT: Self = Self {
        max_width: 16_384,
        max_height: 16_384,
        max_pixels: 67_108_864,
        max_pixmap_bytes: 268_435_456,
        max_layer_depth: 32,
        max_pattern_cells: 16_384,
        max_shading_samples: 1_048_576,
        max_annots: 4_096,
        gfx: pdf_gfx::Limits::DEFAULT,
        images: pdf_images::Limits::DEFAULT,
        color: pdf_color::Limits::DEFAULT,
        semantics: pdf_semantics::Limits::DEFAULT,
        annots: pdf_annots::Limits::DEFAULT,
        forms: pdf_forms::Limits::DEFAULT,
    };
}
