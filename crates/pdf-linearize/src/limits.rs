//! The caps a plan is made under and hint tables are read under: one value for
//! all of them.

/// The caps on what a graph may ask for, or a hint stream: past one, a plan is
/// refused, or a table, with the error that says which
/// ([`plan::Error`](crate::plan::Error), [`hint::Error`](crate::hint::Error)),
/// and a check says what it doesn't judge for it
/// ([`Grade::Unjudged`](crate::check::Grade::Unjudged)). With `family`, a
/// page tree's and a name tree's walks are under it too.
/// A caller sets them on a copy of [`Limits::DEFAULT`] (`limits.pages = 1000`,
/// say), as more may come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// How many object numbers a file may use, the object streams and the
    /// layout's own objects among them: 8,388,606 by default, so that its
    /// cross-reference sections, with the free entry at 0, hold the 8,388,607
    /// indirect objects a file may hold (ISO 32000-1, Annex C).
    pub objects: usize,
    /// How many pages a file may have, or its hint tables say it has: 1,048,574
    /// by default, the most PDFium counts. With `family`, a page tree's walk
    /// lists no more pages than this, and visits no more of its nodes, so a
    /// tree past it isn't walked whole; how deep it goes is
    /// [`page_depth`](Limits::page_depth).
    pub pages: usize,
    /// How many entries all the lists of shared objects hold together, which
    /// is what the hint tables will write of them
    /// ([`Plan::shared`](crate::Plan::shared)): 2^25 by default.
    pub shared_entries: usize,
    /// How many objects and references making those lists may look at:
    /// 2^28 by default. Each list is a walk of what its page reaches, so a
    /// graph where a million pages reach a million objects asks for a trillion
    /// of them, and is refused rather than run.
    pub shared_visits: u64,
    /// How many entries a file's hint tables may ask a reader to hold, all of
    /// them together ([`hint::tables`](crate::hint::tables)): 2^24 by default.
    /// A field of no bits reads nothing, so what the data holds doesn't bound
    /// them by itself.
    pub hint_entries: u64,
    /// With `family`, how many levels of nodes a page tree has, from its root
    /// down, and how many nodes a page's `/Parent` chain has, as its walk
    /// reads them: 1,024 by default, pdf-semantics' own
    /// (`pdf_semantics::Limits::page_depth`). A tree past it isn't walked:
    /// a document's isn't linearized, and a file's is told as not judged.
    /// The engine, without `family`, reads no tree, nor this.
    pub page_depth: usize,
    /// With `family`, how many levels below its root a name tree is read, as
    /// a document's `/Dests`, `/EmbeddedFiles` and templates are read for
    /// where they go: 32 by default, pdf-semantics' own
    /// (`pdf_semantics::Limits::tree_depth`). What's below isn't read, so a
    /// destination there opens no page, and a file there isn't grouped as
    /// one. The engine, without `family`, reads no tree, nor this.
    pub tree_depth: usize,
    /// With `family`, how many nodes of a name tree are read, as
    /// [`tree_depth`](Limits::tree_depth) says: 100,000 by default,
    /// pdf-semantics' own (`pdf_semantics::Limits::tree_nodes`). The engine,
    /// without `family`, reads no tree, nor this.
    pub tree_nodes: usize,
}

impl Limits {
    /// The caps used unless others are given.
    pub const DEFAULT: Limits = Limits {
        objects: 8_388_606,
        pages: 0xFFFFE,
        shared_entries: 1 << 25,
        shared_visits: 1 << 28,
        hint_entries: 1 << 24,
        page_depth: 1024,
        tree_depth: 32,
        tree_nodes: 100_000,
    };

    /// The caps pdf-semantics' walks of a page tree and a name tree are made
    /// under: the pages, the page tree's depth, and a name tree's depth and
    /// nodes, each this one's; the rest its defaults, which no walk here
    /// reaches.
    #[cfg(feature = "family")]
    pub(crate) fn trees(&self) -> pdf_semantics::Limits {
        let mut trees = pdf_semantics::Limits::DEFAULT;
        trees.pages = self.pages;
        trees.page_depth = self.page_depth;
        trees.tree_depth = self.tree_depth;
        trees.tree_nodes = self.tree_nodes;
        trees
    }
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}

#[cfg(all(test, feature = "family"))]
mod tests {
    use super::*;

    /// The caps a walk of pdf-semantics' is made under are its own defaults,
    /// written here as numbers, as the engine builds without it.
    #[test]
    fn the_trees_caps_are_pdf_semantics_defaults() {
        let theirs = pdf_semantics::Limits::DEFAULT;
        assert_eq!(Limits::DEFAULT.trees(), theirs);
        let ours = Limits::DEFAULT;
        assert_eq!(
            [
                ours.pages,
                ours.page_depth,
                ours.tree_depth,
                ours.tree_nodes
            ],
            [
                theirs.pages,
                theirs.page_depth,
                theirs.tree_depth,
                theirs.tree_nodes
            ]
        );
    }

    /// Each is the walk's own, not a default.
    #[test]
    fn each_tree_cap_is_the_callers() {
        let mut limits = Limits::DEFAULT;
        (
            limits.pages,
            limits.page_depth,
            limits.tree_depth,
            limits.tree_nodes,
        ) = (1, 2, 3, 4);
        let trees = limits.trees();
        assert_eq!(
            [
                trees.pages,
                trees.page_depth,
                trees.tree_depth,
                trees.tree_nodes
            ],
            [1, 2, 3, 4]
        );
    }
}
