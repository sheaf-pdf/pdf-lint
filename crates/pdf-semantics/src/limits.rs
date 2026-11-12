//! The caps structures are walked under: one value for all of them.

/// The caps on walking a file's structures: past one, what's left isn't
/// read, and the walk says so. The page tree's writer refuses a tree past
/// [`pages`](Limits::pages) or [`page_depth`](Limits::page_depth), as a walk
/// of it would stop ([`pages::WriteError`](crate::pages::WriteError)). A
/// caller sets them on a copy of [`Limits::DEFAULT`]
/// (`limits.tree_depth = 16`, say), as more may come.
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
    /// visits: 1,048,574 by default, the most PDFium counts. A walk looks at
    /// four times as many kids, as one that's neither costs as one that is.
    pub pages: usize,
    /// How many bytes of an XMP packet are read ([`xmp`](crate::xmp)): 16 MiB
    /// by default. A packet longer than that is an error. Given its bytes
    /// ([`xmp::read`](crate::xmp::read)), the error holds what those up to the
    /// cap claim. Read through a file's streams
    /// ([`xmp::read_from`](crate::xmp::read_from)), none of it is read.
    pub xmp_bytes: usize,
    /// How many levels of an XMP packet's elements are read: 256 by default,
    /// and never more than 65,534, as the XML reader counts them in 16 bits.
    pub xmp_depth: usize,
    /// How many namespaces may be bound at once in an XMP packet, on the
    /// elements open at any point: 128 by default, as each name read looks
    /// through those bound. Together with [`xmp_bytes`](Limits::xmp_bytes)
    /// that's what a packet can cost.
    pub xmp_bindings: usize,
}

impl Limits {
    /// The caps a walk reads under unless it's given others.
    pub const DEFAULT: Limits = Limits {
        tree_depth: 32,
        tree_nodes: 100_000,
        page_depth: 1024,
        pages: 0xFFFFE,
        xmp_bytes: 16 << 20,
        xmp_depth: 256,
        xmp_bindings: 128,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}
