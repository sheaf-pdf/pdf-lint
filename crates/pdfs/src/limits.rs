//! The caps a file is read under: one value for every crate's.

/// The caps a file is read under, so that what a file says can't make a
/// reader read, or hold in memory, without end: each crate's own, nested, set
/// on a copy of [`Limits::DEFAULT`] (`limits.content.content = 1 << 20`, say),
/// as more may come.
///
/// Where one crate's caps hold a cap another's also has, the facade passes
/// one value: [`linearize`](Limits::linearize)'s walks of the page tree and
/// a name tree are made under [`semantics`](Limits::semantics)' caps on them,
/// so a caller sets those once, there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// The reading's (pdf-structure's): what a stream and the reading's
    /// streams in all decode to, how many cross-reference sections and
    /// entries are read, and pdf-objects' parser's caps on each object.
    pub read: pdf_structure::Limits,
    /// What a page's content is read under (pdf-content-streams'): what its
    /// streams joined come to ([`content`](pdf_content_streams::Limits::content)),
    /// its operands and nesting, its inline images, and pdf-color's caps on
    /// a color space its resources name.
    pub content: pdf_content_streams::Limits,
    /// What a page tree, a name tree and an XMP packet are walked under
    /// (pdf-semantics'). Its `page_depth`, `tree_depth` and `tree_nodes` are
    /// also the caps of [`linearize`](Limits::linearize)'s walks.
    pub semantics: pdf_semantics::Limits,
    /// What a font is read under (pdf-fonts').
    pub fonts: pdf_fonts::Limits,
    /// What a file's linearization is checked under (pdf-linearize's): the
    /// graph a plan is made of, and its hint tables. Its `page_depth`,
    /// `tree_depth` and `tree_nodes` are taken from
    /// [`semantics`](Limits::semantics)' at each call, whatever they're set
    /// to here.
    pub linearize: pdf_linearize::Limits,
    /// What one call's fonts' streams may decompress to, in bytes, in all:
    /// 256 MiB by default. A font's cost is charged each time a call reads
    /// it, kept or not.
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
        linearize: pdf_linearize::Limits::DEFAULT,
        font_budget: 256 << 20,
    };

    /// [`linearize`](Limits::linearize), its walks' caps
    /// [`semantics`](Limits::semantics)'.
    pub(crate) const fn linearizing(&self) -> pdf_linearize::Limits {
        let mut linearize = self.linearize;
        linearize.page_depth = self.semantics.page_depth;
        linearize.tree_depth = self.semantics.tree_depth;
        linearize.tree_nodes = self.semantics.tree_nodes;
        linearize
    }
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
    /// pdf-linearize's say.
    #[test]
    fn each_crates_defaults_and_one_value_for_the_walks() {
        let limits = Limits::DEFAULT;
        assert_eq!(limits.read, pdf_structure::Limits::DEFAULT);
        assert_eq!(limits.content, pdf_content_streams::Limits::DEFAULT);
        assert_eq!(limits.semantics, pdf_semantics::Limits::DEFAULT);
        assert_eq!(limits.fonts, pdf_fonts::Limits::DEFAULT);
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
    }
}
