//! The caps a plan is made under: one value for all of them.

/// The caps on what a graph may ask for: past one, a plan is refused, with
/// the [`Error`](crate::plan::Error) that says which. A caller sets them on a
/// copy of [`Limits::DEFAULT`] (`limits.pages = 1000`, say), as more may come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// How many object numbers a file may use, the object streams and the
    /// layout's own objects among them: 8,388,606 by default, so that its
    /// cross-reference sections, with the free entry at 0, hold the 8,388,607
    /// indirect objects a file may hold (ISO 32000-1, Annex C).
    pub objects: usize,
    /// How many pages a file may have: 1,048,574 by default, the most PDFium
    /// counts.
    pub pages: usize,
    /// How many entries all the lists of shared objects hold together, which
    /// is what the hint tables will write of them
    /// ([`Plan::shared`](crate::Plan::shared)): 2^24 by default.
    pub shared_entries: usize,
    /// How many objects and references making those lists may look at:
    /// 2^28 by default. Each list is a walk of what its page reaches, so a
    /// graph where a million pages reach a million objects asks for a trillion
    /// of them, and is refused rather than run.
    pub shared_visits: u64,
}

impl Limits {
    /// The caps a plan is made under unless it's given others.
    pub const DEFAULT: Limits = Limits {
        objects: 8_388_606,
        pages: 0xFFFFE,
        shared_entries: 1 << 24,
        shared_visits: 1 << 28,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}
