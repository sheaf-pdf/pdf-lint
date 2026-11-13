//! The caps a plan is made under and hint tables are read under: one value for
//! all of them.

/// The caps on what a graph may ask for, or a hint stream: past one, a plan is
/// refused, or a table, with the error that says which
/// ([`plan::Error`](crate::plan::Error), [`hint::Error`](crate::hint::Error)),
/// and a check says what it doesn't judge for it
/// ([`Grade::Unjudged`](crate::check::Grade::Unjudged)).
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
    /// tree past it isn't walked whole; the walk's other caps, on how deep a
    /// tree goes, and on a name tree's, are pdf-semantics' defaults.
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
}

impl Limits {
    /// The caps used unless others are given.
    pub const DEFAULT: Limits = Limits {
        objects: 8_388_606,
        pages: 0xFFFFE,
        shared_entries: 1 << 25,
        shared_visits: 1 << 28,
        hint_entries: 1 << 24,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}
