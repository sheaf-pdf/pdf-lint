//! Where a file's structures depart from ISO 32000-2. What's read goes on
//! past each [`Fault`], told to a [`Sink`] with where it is. Nothing told
//! changes what's read: the values given are the same whatever the sink,
//! and a sink that doesn't want faults, `()`, has none looked for.

use pdf_objects::{ObjectId, Span};

/// Where faults are told: a checker keeps them; `()` drops them, and wants
/// none, so none is looked for. A fault owns nothing, so a sink can keep it
/// as it is. A sink borrowed, `&mut S` or `&mut dyn Sink`, is a sink too.
pub trait Sink {
    /// A fault at `at`.
    fn fault(&mut self, at: Place, fault: Fault);

    /// Whether faults are wanted. A check that does work of its own, such
    /// as following a tree's `/Limits`, runs only when they are, and a sink
    /// that doesn't want them is told none. True unless a sink says
    /// otherwise; asked as a walk goes.
    fn wants(&self) -> bool {
        true
    }
}

/// Drops every fault, and wants none, so none is looked for.
impl Sink for () {
    #[inline]
    fn fault(&mut self, _: Place, _: Fault) {}

    #[inline]
    fn wants(&self) -> bool {
        false
    }
}

/// Tells the sink it borrows, and wants what it wants.
impl<S: Sink + ?Sized> Sink for &mut S {
    #[inline]
    fn fault(&mut self, at: Place, fault: Fault) {
        (**self).fault(at, fault);
    }

    #[inline]
    fn wants(&self) -> bool {
        (**self).wants()
    }
}

/// Where a [`Fault`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Place {
    /// The numbered object it's in; none where a walk began in a value
    /// written in place and hasn't reached one yet.
    pub object: Option<ObjectId>,
    /// Where what's at fault was written, when the reader knows.
    pub at: Option<Span>,
}

impl Place {
    /// A fault's place, for a sink to be tried on: walks make their own.
    pub const fn new(object: Option<ObjectId>, at: Option<Span>) -> Place {
        Place { object, at }
    }
}

/// A place where a file's structure isn't what ISO 32000-2 says it shall
/// be. What each does to a walk is on the walk's own doc. Each names its
/// clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Fault {
    /// A reference to an object the file lists but whose bytes aren't that
    /// object, or that doesn't lead to it: read as no object, as 7.3.10 has
    /// a reference to nothing, though the file meant one.
    #[error("{} {} R doesn't read", .id.0, .id.1)]
    Damaged {
        /// The object.
        id: ObjectId,
    },
    /// A node, or an array of one, that the tree reaches by reference for
    /// a second time, by a loop or by two parents: each node has one place
    /// in a tree (7.9.6). It isn't read again, so nothing's lost.
    #[error("the tree reaches {} {} R again", .id.0, .id.1)]
    TreeRepeated {
        /// The object it reaches again.
        id: ObjectId,
    },
    /// A node below as many levels as the tree is read
    /// ([`Limits::tree_depth`](crate::Limits::tree_depth)): the node and
    /// what's under it aren't read. ISO 32000-2 sets no such cap.
    #[error("the tree goes deeper than it's read")]
    TreeTooDeep,
    /// More nodes than a tree is read
    /// ([`Limits::tree_nodes`](crate::Limits::tree_nodes)): the walk stops
    /// there. ISO 32000-2 sets no such cap.
    #[error("the tree has more nodes than are read")]
    TreeTooManyNodes,
    /// A node that isn't a dictionary (7.9.6, 7.9.7), or a kid that's null
    /// or a reference to nothing.
    #[error("a node of the tree isn't a dictionary")]
    TreeNodeNotDictionary,
    /// A kid written in place: `/Kids` holds references to its nodes
    /// (7.9.6, Table 36; 7.9.7, Table 37). It's read as any other.
    #[error("a kid is written in place, not as a reference")]
    TreeKidDirect,
    /// A node with `/Kids` and `/Names` or `/Nums` too: a root has one of
    /// them and any other node is an intermediate node or a leaf (7.9.6).
    /// Both are read, its own entries first.
    #[error("a node has both /Kids and its own entries")]
    TreeBothEntries,
    /// A node with none of `/Kids`, `/Names` and `/Nums` (7.9.6, Table 36;
    /// 7.9.7, Table 37).
    #[error("a node has neither /Kids nor entries of its own")]
    TreeNoEntries,
    /// A root with `/Limits`, which only the nodes under it have (7.9.6,
    /// Table 36; 7.9.7, Table 37).
    #[error("the root has /Limits")]
    TreeRootLimits,
    /// A node under the root without `/Limits` (7.9.6, Table 36; 7.9.7,
    /// Table 37).
    #[error("a node under the root has no /Limits")]
    TreeNoLimits,
    /// A `/Limits` that isn't two keys, least first, or that doesn't read
    /// (7.9.6, Table 36; 7.9.7, Table 37). Told only to a sink that wants
    /// faults, and what's under it isn't held to it.
    #[error("a node's /Limits isn't its least and greatest keys")]
    TreeBadLimits,
    /// `/Names`, `/Nums` or `/Kids` isn't an array (7.9.6, Table 36; 7.9.7,
    /// Table 37), so what it holds isn't read.
    #[error("a node's /{key} isn't an array")]
    TreeNotArray {
        /// Which of them.
        key: &'static str,
    },
    /// An array of keys and values that ends on a key (7.9.6, 7.9.7): with
    /// no value, it isn't given.
    #[error("the last key has no value")]
    TreeOddKey,
    /// A key that's another type than its tree's, or null (7.9.6 with its
    /// erratum 307; 7.9.7): it isn't given, nor its value.
    #[error("a key isn't of its tree's type")]
    TreeKeyKind,
    /// A key whose bytes the reader didn't give, as it's ciphertext in a
    /// file it couldn't decrypt: it isn't given, nor its value, since keys
    /// compare by their bytes (7.9.6).
    #[error("a key's bytes aren't known")]
    TreeKeyUnread,
    /// A key that isn't past the one before it, in the order the walk
    /// reaches them: keys ascend, and don't repeat (7.9.6, 7.9.7). It's
    /// given where it stands. Told only to a sink that wants faults.
    #[error("a key isn't past the one before it")]
    TreeKeyOrder,
    /// A key outside the `/Limits` of a node over it (7.9.6, 7.9.7). It's
    /// given. Told only to a sink that wants faults.
    #[error("a key is outside its node's /Limits")]
    TreeKeyOutsideLimits,
    /// A catalog's `/Pages` written in place, where it's an indirect reference
    /// (7.7.2, Table 29). It's read as any other.
    #[error("the catalog's /Pages is written in place, not as a reference")]
    PageTreeRootDirect,
    /// A `/Parent` on the root of the page tree (7.7.3.2, Table 30).
    #[error("the root of the page tree has a /Parent")]
    PageTreeRootParent,
    /// A page tree with no pages under its root: a node has at least one kid
    /// and a count of at least 1 (7.7.3.2, with its erratum 271).
    #[error("the page tree has no pages")]
    PageTreeEmpty,
    /// A kid written in place, where `/Kids` holds indirect references
    /// (7.7.3.2, Table 30). It's read as any other, with no number.
    #[error("a kid of the page tree is written in place, not as a reference")]
    PageTreeKidDirect,
    /// A kid that's null, or a reference to nothing, or to no page object or
    /// node (7.7.3.2, Table 30, with its erratum 271): it isn't a page, and no
    /// page's number is taken by it. A stream, whose dictionary is read all
    /// the same, is said too.
    #[error("a kid of the page tree isn't a dictionary")]
    PageTreeKidNotDictionary,
    /// A reference that names another reference on the page tree's way, where
    /// 7.3.10 has it name an object, and Tables 30 and 31 say what kind. It's
    /// followed; readers differ here, PDFium stopping after one or two.
    #[error("a reference in the page tree names another reference")]
    PageTreeChain,
    /// A `/Type` that doesn't say what's there (7.7.3.2, Table 30; 7.7.3.3,
    /// Table 31): absent, or other than `/Pages` or `/Page`, so `/Kids` says
    /// which it is; or `/Page` with `/Kids`, a page by its `/Type`; or `/Page`
    /// at the root, the one page of the tree.
    #[error("a /Type in the page tree doesn't say what's there")]
    PageTreeType,
    /// A node with no kids: `/Kids` absent, or not an array, or empty (7.7.3.2,
    /// Table 30, with its erratum 271).
    #[error("a node of the page tree has no kids")]
    PageTreeNoKids,
    /// A page or a node that the tree lists again: there's one reference to
    /// each node (7.7.3.2). A page or a node listed again is walked again,
    /// numbering its pages again.
    #[error("the page tree lists {} {} R again", .id.0, .id.1)]
    PageTreeRepeated {
        /// The object listed again.
        id: ObjectId,
    },
    /// A node listed inside itself, which by 7.7.3.2 can't be a tree. It isn't
    /// walked again, so a walk ends.
    #[error("the page tree lists {} {} R inside itself", .id.0, .id.1)]
    PageTreeCycle {
        /// The node.
        id: ObjectId,
    },
    /// A `/Parent` that's absent, or isn't the node that lists it (7.7.3.2,
    /// Table 30; 7.7.3.3, Table 31). What's inherited still comes up `/Parent`
    /// (7.7.3.4). Told only to a sink that wants faults.
    #[error("a /Parent isn't the node that lists it")]
    PageTreeParent,
    /// A `/Parent` written in place, where it's an indirect reference (7.7.3.2,
    /// Table 30; 7.7.3.3, Table 31). It's read as any other.
    #[error("a /Parent is written in place, not as a reference")]
    PageTreeParentDirect,
    /// A `/Parent` chain that comes back to itself: what's inherited stops
    /// there (7.7.3.4).
    #[error("a /Parent chain loops")]
    PageTreeParentLoop,
    /// A node's `/Count` that isn't the pages found under it, at least 1
    /// (7.7.3.2, Table 30, with its erratum 271). It isn't what the walk goes
    /// by. Told only to a sink that wants faults.
    #[error("a node's /Count isn't the pages under it")]
    PageTreeCount,
    /// A page with no `/MediaBox`, its own or inherited, that's an array of
    /// four (7.7.3.3, Table 31; 7.9.5). Letter stands for it.
    #[error("a page has no /MediaBox")]
    PageNoMediaBox,
    /// XMP metadata that begins with a mark of UTF-16, where it's encoded as
    /// UTF-8 (14.3.2, with its erratum 296). Nothing in it is read.
    #[error("XMP metadata begins with a mark of UTF-16, not as UTF-8")]
    XmpNotUtf8,
    /// An attribute in XMP metadata that doesn't read as XML, which 14.3.2 has
    /// it written in: no quotes, no value, or given twice. It's passed.
    #[error("an attribute in XMP metadata doesn't read")]
    XmpAttribute,
    /// A claim's value in XMP metadata that doesn't read as XML, which 14.3.2
    /// has it written in: an entity of no meaning without a DOCTYPE, which
    /// isn't read, or bytes that aren't UTF-8. It's as though it weren't written.
    #[error("a value in XMP metadata doesn't read")]
    XmpValue,
}
