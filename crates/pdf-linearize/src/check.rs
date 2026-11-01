//! Whether a linearized file holds (ISO 32000-2, Annex F): the facts of a file
//! as written, as any model's reader gives them, against what they say of each
//! other.
//!
//! [`check`] takes [`Facts`], plain data, so the judging is here whichever
//! model read the file, and gives a [`Verdict`], telling each [`Finding`] to a
//! [`Sink`]. What it holds a hint table to is the layout the sections give:
//! each page's and each shared entry's position, count and length, as a reader
//! adding them up would find them. Given the file's [`Graph`](crate::Graph),
//! what each page reaches is held to them too.
//!
//! # What holds
//!
//! - **`linearized`** is qpdf's test: the first object starts within 1024 bytes
//!   of the header, it's a dictionary whose `/Linearized` has the integer part
//!   1, and its `/L` is the file's length from the header. A dictionary that
//!   runs past those bytes is a finding of its own, where F.3.3 has it whole
//!   within them. A file with no such dictionary has no findings at all.
//! - **`holds`** is that, with no finding of [`Grade::Error`].
//! - **`hints_read`** is whether the hint stream's data decoded and every table
//!   it says it has read, under [`Limits`]. So a reading with no key for the
//!   data can hold, as far as anything can say, with this false.
//!
//! # A caller that ignores findings
//!
//! The [`Verdict`] is the same whatever the sink. A sink that doesn't want
//! findings, `()`, is told none, and once there's an error nothing more is
//! looked at, as nothing can change the verdict by then. What it still pays for
//! is the [`Facts`] and the hint tables' reading, which `hints_read` needs.
//!
//! # What it holds a position to
//!
//! Positions in the file are as the sections' rows have them, from the file's
//! first byte. An object ends where the next thing the sections list starts,
//! unless an [`Extent`] says it's read otherwise. A table's positions are as if
//! the hint stream weren't there (F.4.1), as `/H` says that is, or as the
//! stream that's there has it if `/H` is wrong, which is said once and not of
//! everything after it. So is a page's or an entry's place that's wrong: what's
//! after it is held from where its object is.
//!
//! # What it reads under
//!
//! Facts can say anything: it never panics, nothing it does is more than a sort
//! and a search per entry of a table, and those are as many as
//! [`Limits::hint_entries`](crate::Limits::hint_entries) lets them be.

mod facts;
mod finding;
mod judge;
#[cfg(test)]
mod tests;

pub use facts::{
    Attribute, Extent, Facts, FirstObject, HintData, HintFacts, PageFacts, Raw, RowEntry, RowFacts,
    SectionFacts,
};
pub use finding::{DictKey, Finding, Grade, KeyName, Place};

use crate::Limits;

/// Where findings are told: `Vec<Finding>` keeps them; `()` drops them and
/// wants none, so none is looked for past what the verdict needs. A sink
/// borrowed, `&mut S` or `&mut dyn Sink`, is a sink too.
pub trait Sink {
    /// One thing wrong with the file.
    fn finding(&mut self, finding: Finding);

    /// Whether findings are wanted. A sink that doesn't want them is told none,
    /// and looking stops once the verdict is settled. True unless a sink says
    /// otherwise; asked as a check goes.
    fn wants(&self) -> bool {
        true
    }
}

/// Drops every finding, and wants none.
impl Sink for () {
    #[inline]
    fn finding(&mut self, _: Finding) {}

    #[inline]
    fn wants(&self) -> bool {
        false
    }
}

/// Tells the sink it borrows, and wants what it wants.
impl<S: Sink + ?Sized> Sink for &mut S {
    #[inline]
    fn finding(&mut self, finding: Finding) {
        (**self).finding(finding);
    }

    #[inline]
    fn wants(&self) -> bool {
        (**self).wants()
    }
}

/// Keeps every finding, in the order they're told.
impl Sink for Vec<Finding> {
    #[inline]
    fn finding(&mut self, finding: Finding) {
        self.push(finding);
    }
}

/// What a check says of a file, whatever its sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Verdict {
    /// Whether the file says it's linearized and its `/L` holds, by the
    /// module's test.
    pub linearized: bool,
    /// Whether it does and no finding is an error.
    pub holds: bool,
    /// Whether its hint stream's data decoded and every table it names read.
    pub hints_read: bool,
}

/// What `facts` say of each other, told to `sink`, under `limits`: the module
/// says how.
pub fn check<S: Sink + ?Sized>(facts: &Facts, limits: &Limits, sink: &mut S) -> Verdict {
    judge::check(facts, limits, sink)
}
