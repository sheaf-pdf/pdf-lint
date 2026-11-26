//! Unified resource limits across all PDF reading and writing subsystems.

/// Resource caps bounding memory and work across every subsystem when reading or writing a PDF.
///
/// Customize limits by mutating a copy of [`Limits::DEFAULT`] (for example,
/// `limits.content.content = 1 << 20;`).
///
/// Where multiple crates share overlapping tree/content limits, `pdfs` forwards a
/// single source of truth: [`linearize`](Limits::linearize) inherits `page_depth`,
/// `tree_depth`, and `tree_nodes` from [`semantics`](Limits::semantics) (and uses
/// `min(linearize.pages, semantics.pages)`), while [`fonts`](Limits::fonts)
/// inherits its content-lexing limits from [`content`](Limits::content).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// The reading's (pdf-structure's): what a stream and the reading's
    /// streams in all decode to, how many cross-reference sections and
    /// entries are read, and pdf-objects' parser's caps on each object.
    pub read: pdf_structure::Limits,
    /// What a page's content is read under (pdf-content-streams'): what its
    /// streams joined come to
    /// ([`content`](pdf_content_streams::Limits::content)), its operands and
    /// nesting, its inline images, and pdf-color's caps on a color space its
    /// resources name.
    pub content: pdf_content_streams::Limits,
    /// What a page tree, a name tree and an XMP packet are walked under
    /// (pdf-semantics'). Its `page_depth`, `tree_depth` and `tree_nodes` are
    /// also the caps of [`linearize`](Limits::linearize)'s walks.
    pub semantics: pdf_semantics::Limits,
    /// What a font is read under, and a page's text, how deep and how often
    /// its forms are followed among it (pdf-fonts'). Its `content` is taken
    /// from [`content`](Limits::content) at each call, whatever it's set to
    /// here.
    pub fonts: pdf_fonts::Limits,
    /// What a page's graphics and text state machine and content re-encoder run
    /// under (`pdf-gfx`'s): per-page caps on saved states, operators, glyphs,
    /// path segments, nested marked content, interpreted bytes, Form XObject
    /// depth, and decompressed bytes. Its `content` and `fonts` are taken from
    /// [`content`](Limits::content) and [`fonts`](Limits::fonts) at each call.
    pub gfx: pdf_gfx::Limits,
    /// What page annotations, reply threads, and flattening run under
    /// (`pdf-annots`'s). Its `content` and `semantics` are synced from
    /// [`content`](Limits::content) and [`semantics`](Limits::semantics) at each
    /// call.
    pub annots: pdf_annots::Limits,
    /// What Image XObjects and inline images decode under (`pdf-images`'s,
    /// `images` feature). Its `content` and `color` are synced from
    /// [`content`](Limits::content) at each call.
    #[cfg(feature = "images")]
    pub images: pdf_images::Limits,
    /// What 2D page, tile, annotation, and form rasterization runs under
    /// (`pdf-raster`'s, `raster` feature). Its `gfx`, `images`, `color`,
    /// `semantics`, `annots`, and `forms` limits are synced from this struct's
    /// fields at each call.
    #[cfg(feature = "raster")]
    pub raster: pdf_raster::Limits,
    /// What a file's linearization is checked under, and a document is
    /// linearized under (pdf-linearize's): the graph a plan is made of, and
    /// its hint tables. Its `page_depth`, `tree_depth` and `tree_nodes` are
    /// taken from [`semantics`](Limits::semantics)' at each call, whatever
    /// they're set to here, and its `pages` is the lesser of its own and
    /// `semantics.pages`, as it caps the page tree's walk and the pages its
    /// hint tables may say.
    pub linearize: pdf_linearize::Limits,
    /// What one call's fonts' streams, and a page's text's forms, may
    /// decompress to, in bytes, in all: 256 MiB by default. A font's cost is
    /// charged each time a call reads it, kept or not, and a form's content
    /// each time a page's text draws it. A page's own content isn't charged:
    /// [`content`](Limits::content) caps it.
    pub font_budget: usize,
}

impl Limits {
    /// The caps a file is read under unless it's given others: each crate's
    /// own defaults, and a font budget of 256 MiB.
    pub const DEFAULT: Limits = Limits {
        read: pdf_structure::Limits::DEFAULT,
        content: pdf_content_streams::Limits::DEFAULT,
        semantics: pdf_semantics::Limits::DEFAULT,
        fonts: pdf_fonts::Limits::DEFAULT,
        gfx: pdf_gfx::Limits::DEFAULT,
        annots: pdf_annots::Limits::DEFAULT,
        #[cfg(feature = "images")]
        images: pdf_images::Limits::DEFAULT,
        #[cfg(feature = "raster")]
        raster: pdf_raster::Limits::DEFAULT,
        linearize: pdf_linearize::Limits::DEFAULT,
        font_budget: 256 << 20,
    };

    /// [`fonts`](Limits::fonts), its `content` [`content`](Limits::content).
    pub(crate) const fn reading_fonts(&self) -> pdf_fonts::Limits {
        let mut fonts = self.fonts;
        fonts.content = self.content;
        fonts
    }

    /// [`gfx`](Limits::gfx), its `content` and `fonts` synced from
    /// [`content`](Limits::content) and [`fonts`](Limits::fonts).
    pub(crate) const fn reading_gfx(&self) -> pdf_gfx::Limits {
        let mut gfx = self.gfx;
        gfx.content = self.content;
        gfx.fonts = self.reading_fonts();
        gfx
    }

    /// [`annots`](Limits::annots), its `content` and `semantics` synced from
    /// [`content`](Limits::content) and [`semantics`](Limits::semantics).
    pub(crate) const fn reading_annots(&self) -> pdf_annots::Limits {
        let mut annots = self.annots;
        annots.content = self.content;
        annots.semantics = self.semantics;
        annots
    }

    /// `images`, its `content` and `color` synced from
    /// [`content`](Limits::content).
    #[cfg(feature = "images")]
    pub(crate) const fn reading_images(&self) -> pdf_images::Limits {
        let mut images = self.images;
        images.content = self.content;
        images.color = self.content.color;
        images
    }

    /// `raster`, its subsystem limits synced from this struct's fields.
    #[cfg(feature = "raster")]
    pub(crate) const fn reading_raster(&self) -> pdf_raster::Limits {
        let mut raster = self.raster;
        raster.gfx = self.reading_gfx();
        raster.images = self.reading_images();
        raster.color = self.content.color;
        raster.semantics = self.semantics;
        raster.annots = self.reading_annots();
        raster.forms.annots = raster.annots;
        raster.forms.fonts = self.reading_fonts();
        raster
    }

    /// Element-wise narrower of [`Self::reading_raster`] and `other`, so
    /// customizing one cap on `RenderOpts::limits` never drops any cap the
    /// `Pdf` was opened with (and vice versa).
    #[cfg(feature = "raster")]
    pub(crate) fn narrower_raster(&self, other: &pdf_raster::Limits) -> pdf_raster::Limits {
        let mut r = self.reading_raster();
        r.max_width = r.max_width.min(other.max_width);
        r.max_height = r.max_height.min(other.max_height);
        r.max_pixels = r.max_pixels.min(other.max_pixels);
        r.max_pixmap_bytes = r.max_pixmap_bytes.min(other.max_pixmap_bytes);
        r.max_layer_depth = r.max_layer_depth.min(other.max_layer_depth);
        r.max_pattern_cells = r.max_pattern_cells.min(other.max_pattern_cells);
        r.max_shading_samples = r.max_shading_samples.min(other.max_shading_samples);
        r.max_annots = r.max_annots.min(other.max_annots);

        r.gfx.content = min_content(r.gfx.content, other.gfx.content);
        r.gfx.fonts = min_fonts(r.gfx.fonts, other.gfx.fonts);
        r.gfx.form_depth = r.gfx.form_depth.min(other.gfx.form_depth);
        r.gfx.states = r.gfx.states.min(other.gfx.states);
        r.gfx.operators = r.gfx.operators.min(other.gfx.operators);
        r.gfx.glyphs = r.gfx.glyphs.min(other.gfx.glyphs);
        r.gfx.segments = r.gfx.segments.min(other.gfx.segments);
        r.gfx.marked = r.gfx.marked.min(other.gfx.marked);
        r.gfx.interpreted = r.gfx.interpreted.min(other.gfx.interpreted);
        r.gfx.decompressed = r.gfx.decompressed.min(other.gfx.decompressed);

        r.images.max_width = r.images.max_width.min(other.images.max_width);
        r.images.max_height = r.images.max_height.min(other.images.max_height);
        r.images.max_pixels = r.images.max_pixels.min(other.images.max_pixels);
        r.images.max_sample_bytes = r.images.max_sample_bytes.min(other.images.max_sample_bytes);
        r.images.max_encoded_bytes = r
            .images
            .max_encoded_bytes
            .min(other.images.max_encoded_bytes);
        r.images.mask_depth = r.images.mask_depth.min(other.images.mask_depth);
        r.images.color = min_color(r.images.color, other.images.color);
        r.images.content = min_content(r.images.content, other.images.content);

        r.color = min_color(r.color, other.color);
        r.semantics = min_semantics(r.semantics, other.semantics);
        r.annots = min_annots(r.annots, other.annots);

        r.forms.hops = r.forms.hops.min(other.forms.hops);
        r.forms.fields = r.forms.fields.min(other.forms.fields);
        r.forms.depth = r.forms.depth.min(other.forms.depth);
        r.forms.items = r.forms.items.min(other.forms.items);
        r.forms.annots = min_annots(r.forms.annots, other.forms.annots);
        r.forms.text = r.forms.text.min(other.forms.text);
        r.forms.comb_cells = r.forms.comb_cells.min(other.forms.comb_cells);
        r.forms.fonts = min_fonts(r.forms.fonts, other.forms.fonts);
        r.forms.font_budget = r.forms.font_budget.min(other.forms.font_budget);
        r
    }

    /// [`linearize`](Limits::linearize), its walks' caps
    /// [`semantics`](Limits::semantics)', its pages the lesser of the two.
    pub(crate) const fn linearizing(&self) -> pdf_linearize::Limits {
        let mut linearize = self.linearize;
        if self.semantics.pages < linearize.pages {
            linearize.pages = self.semantics.pages;
        }
        linearize.page_depth = self.semantics.page_depth;
        linearize.tree_depth = self.semantics.tree_depth;
        linearize.tree_nodes = self.semantics.tree_nodes;
        linearize
    }
}

#[cfg(feature = "raster")]
fn min_color(mut a: pdf_color::Limits, b: pdf_color::Limits) -> pdf_color::Limits {
    a.space_depth = a.space_depth.min(b.space_depth);
    a.space_visits = a.space_visits.min(b.space_visits);
    a.stream_bytes = a.stream_bytes.min(b.stream_bytes);
    a.sample_values = a.sample_values.min(b.sample_values);
    a.postscript_ops = a.postscript_ops.min(b.postscript_ops);
    a.postscript_stack = a.postscript_stack.min(b.postscript_stack);
    a
}

#[cfg(feature = "raster")]
fn min_content(
    mut a: pdf_content_streams::Limits,
    b: pdf_content_streams::Limits,
) -> pdf_content_streams::Limits {
    a.operands = a.operands.min(b.operands);
    a.depth = a.depth.min(b.depth);
    a.values = a.values.min(b.values);
    a.inline_image = a.inline_image.min(b.inline_image);
    a.image_ends = a.image_ends.min(b.image_ends);
    a.content = a.content.min(b.content);
    a.color = min_color(a.color, b.color);
    a.space_values = a.space_values.min(b.space_values);
    a.marked = a.marked.min(b.marked);
    a.splice_reads = a.splice_reads.min(b.splice_reads);
    a.hops = a.hops.min(b.hops);
    a
}

#[cfg(feature = "raster")]
fn min_fonts(mut a: pdf_fonts::Limits, b: pdf_fonts::Limits) -> pdf_fonts::Limits {
    a.content = min_content(a.content, b.content);
    a.cmaps = a.cmaps.min(b.cmaps);
    a.type1_work = a.type1_work.min(b.type1_work);
    a.font_fuel = a.font_fuel.min(b.font_fuel);
    a.glyph_fuel = a.glyph_fuel.min(b.glyph_fuel);
    a.composite_glyphs = a.composite_glyphs.min(b.composite_glyphs);
    a.composite_depth = a.composite_depth.min(b.composite_depth);
    a.forms = a.forms.min(b.forms);
    a.form_uses = a.form_uses.min(b.form_uses);
    a
}

#[cfg(feature = "raster")]
fn min_semantics(mut a: pdf_semantics::Limits, b: pdf_semantics::Limits) -> pdf_semantics::Limits {
    a.tree_depth = a.tree_depth.min(b.tree_depth);
    a.tree_nodes = a.tree_nodes.min(b.tree_nodes);
    a.page_depth = a.page_depth.min(b.page_depth);
    a.pages = a.pages.min(b.pages);
    a.xmp_bytes = a.xmp_bytes.min(b.xmp_bytes);
    a.xmp_depth = a.xmp_depth.min(b.xmp_depth);
    a.xmp_bindings = a.xmp_bindings.min(b.xmp_bindings);
    a.action_chain = a.action_chain.min(b.action_chain);
    a.outline_depth = a.outline_depth.min(b.outline_depth);
    a.outline_items = a.outline_items.min(b.outline_items);
    a.embedded_bytes = a.embedded_bytes.min(b.embedded_bytes);
    a.oc_depth = a.oc_depth.min(b.oc_depth);
    a
}

#[cfg(feature = "raster")]
fn min_annots(mut a: pdf_annots::Limits, b: pdf_annots::Limits) -> pdf_annots::Limits {
    a.hops = a.hops.min(b.hops);
    a.annots_per_page = a.annots_per_page.min(b.annots_per_page);
    a.reply_depth = a.reply_depth.min(b.reply_depth);
    a.quads = a.quads.min(b.quads);
    a.points = a.points.min(b.points);
    a.content = min_content(a.content, b.content);
    a.semantics = min_semantics(a.semantics, b.semantics);
    a
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defaults are each crate's own; the walks of a linearization take
    /// the page tree's and a name tree's caps from pdf-semantics', whatever
    /// pdf-linearize's say, and the lesser of the two crates' pages; and
    /// fonts their content's from the content's.
    #[test]
    fn each_crates_defaults_and_one_value_for_the_walks() {
        let limits = Limits::DEFAULT;
        assert_eq!(limits.read, pdf_structure::Limits::DEFAULT);
        assert_eq!(limits.content, pdf_content_streams::Limits::DEFAULT);
        assert_eq!(limits.semantics, pdf_semantics::Limits::DEFAULT);
        assert_eq!(limits.fonts, pdf_fonts::Limits::DEFAULT);
        assert_eq!(limits.gfx, pdf_gfx::Limits::DEFAULT);
        assert_eq!(limits.reading_gfx(), pdf_gfx::Limits::DEFAULT);
        assert_eq!(limits.annots, pdf_annots::Limits::DEFAULT);
        assert_eq!(limits.reading_annots(), pdf_annots::Limits::DEFAULT);
        #[cfg(feature = "images")]
        {
            assert_eq!(limits.images, pdf_images::Limits::DEFAULT);
            assert_eq!(limits.reading_images(), pdf_images::Limits::DEFAULT);
        }
        #[cfg(feature = "raster")]
        {
            assert_eq!(limits.raster, pdf_raster::Limits::DEFAULT);
            assert_eq!(limits.reading_raster(), pdf_raster::Limits::DEFAULT);
        }
        assert_eq!(limits.linearize, pdf_linearize::Limits::DEFAULT);
        assert_eq!(limits.font_budget, 256 << 20);
        assert_eq!(limits.linearizing(), pdf_linearize::Limits::DEFAULT);
        let mut limits = Limits::DEFAULT;
        limits.semantics.page_depth = 4;
        limits.semantics.tree_depth = 3;
        limits.semantics.tree_nodes = 2;
        limits.linearize.page_depth = 99;
        limits.linearize.pages = 7;
        let walked = limits.linearizing();
        assert_eq!(
            (walked.page_depth, walked.tree_depth, walked.tree_nodes),
            (4, 3, 2)
        );
        assert_eq!(walked.pages, 7);
        limits.semantics.pages = 5;
        assert_eq!(limits.linearizing().pages, 5);
        let mut limits = Limits::DEFAULT;
        limits.content.operands = 9;
        limits.fonts.content.operands = 99;
        limits.fonts.forms = 3;
        let fonts = limits.reading_fonts();
        assert_eq!((fonts.content, fonts.forms), (limits.content, 3));
        limits.gfx.content.operands = 77;
        limits.gfx.fonts.forms = 88;
        limits.gfx.operators = 42;
        let gfx = limits.reading_gfx();
        assert_eq!(
            (gfx.content, gfx.fonts.forms, gfx.operators),
            (limits.content, 3, 42)
        );
    }
}
