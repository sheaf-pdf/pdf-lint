//! Resource caps enforced before allocating framebuffers, offscreen group/mask
//! layers, tiling pattern cells, or shading meshes.

/// Which [`Limits`] cap refused a rendering operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum LimitKind {
    /// Target viewport or offscreen side length exceeded [`Limits::max_width`] /
    /// [`Limits::max_height`] (or `u16::MAX`, `65,535`).
    #[error("dimensions exceed maximum allowed side length")]
    Dimensions,
    /// Target viewport or offscreen pixel count (`width * height`) exceeded
    /// [`Limits::max_pixels`].
    #[error("pixel count exceeds maximum allowed pixels")]
    Pixels,
    /// Target framebuffer byte size (`width * height * 4`) exceeded
    /// [`Limits::max_pixmap_bytes`].
    #[error("framebuffer byte size exceeds maximum allowed bytes")]
    PixmapBytes,
    /// Nested transparency group, soft-mask, tiling-pattern, or Type 3 recursion
    /// exceeded [`Limits::max_layer_depth`].
    #[error("nested layer or stream depth exceeds maximum")]
    LayerDepth,
    /// Tiling pattern cell pixel count exceeded [`Limits::max_tile_pixels`].
    #[error("tiling pattern cell pixel count exceeds maximum")]
    PatternCells,
    /// Shading pixel grid or mesh evaluation budget exceeded
    /// [`Limits::max_shading_samples`].
    #[error("shading sample or mesh budget exceeds maximum")]
    ShadingSamples,
    /// Annotation count on the page exceeded [`Limits::max_annots`].
    #[error("page annotation count exceeds maximum")]
    Annotations,
    /// Offscreen layer, pattern, shading, or mask byte budget exceeded
    /// [`Limits::max_scene_bytes`].
    #[error("offscreen scene memory exceeds maximum")]
    SceneBytes,
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
    /// Maximum width or height (in device pixels, `<= 65,535`) of a rasterized
    /// tiling-pattern cell (`2,048` by default); larger cells are clamped to
    /// this side length and mark [`crate::RenderSummary::approximated`].
    pub max_tile_side: u32,
    /// Maximum pixel count (`tile_w * tile_h`, `4,194,304` by default) of a
    /// rasterized tiling-pattern cell.
    pub max_tile_pixels: u64,
    /// Maximum pixel grid area (`grid_w * grid_h`) evaluated at 1:1 resolution
    /// per shading paint before proportional downsampling, and maximum Gouraud
    /// mesh triangle budget (`1,048,576` by default).
    pub max_shading_samples: u32,
    /// Maximum total bytes retained across per-render offscreen caches (tiling
    /// pattern cells, decoded XObject images, soft masks, and Type 3 glyph
    /// streams) or a single offscreen buffer (`128 MiB` by default).
    pub max_scene_bytes: usize,
    /// Maximum annotations evaluated per page (`4,096` by default).
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
    /// `67,108,864` max pixels, `256 MiB` max framebuffer, `128 MiB` scene
    /// cache cap).
    pub const DEFAULT: Self = Self {
        max_width: 16_384,
        max_height: 16_384,
        max_pixels: 67_108_864,
        max_pixmap_bytes: 268_435_456,
        max_layer_depth: 32,
        max_tile_side: 2_048,
        max_tile_pixels: 4_194_304,
        max_shading_samples: 1_048_576,
        max_scene_bytes: 134_217_728,
        max_annots: 4_096,
        gfx: pdf_gfx::Limits::DEFAULT,
        images: pdf_images::Limits::DEFAULT,
        color: pdf_color::Limits::DEFAULT,
        semantics: pdf_semantics::Limits::DEFAULT,
        annots: pdf_annots::Limits::DEFAULT,
        forms: pdf_forms::Limits::DEFAULT,
    };
}
