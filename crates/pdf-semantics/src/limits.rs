//! The caps structures are walked under: one value for all of them.

/// The caps on walking a file's structures: past one, what's left isn't
/// read, and the walk says so. A caller sets them on a copy of
/// [`Limits::DEFAULT`] (`limits.tree_depth = 16`, say), as more may come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// How many levels below its root a name or number tree is read
    /// ([`tree`](crate::tree)): 32 by default.
    pub tree_depth: usize,
    /// How many nodes of a name or number tree are read: 100,000 by default.
    pub tree_nodes: usize,
    /// How many levels of nodes a page tree has, from its root down, and how
    /// many nodes a `/Parent` chain has ([`pages`](crate::pages)): 1,024 by
    /// default, as PDFium's walk goes.
    pub page_depth: usize,
    /// How many pages a page tree lists, and how many of its nodes a walk
    /// visits: 1,048,574 by default, the most PDFium counts.
    pub pages: usize,
}

impl Limits {
    /// The caps a walk reads under unless it's given others.
    pub const DEFAULT: Limits = Limits {
        tree_depth: 32,
        tree_nodes: 100_000,
        page_depth: 1024,
        pages: 0xFFFFE,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}
