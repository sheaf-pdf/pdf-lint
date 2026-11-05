//! Why a font isn't read.

use crate::Unreadable;

/// What stops a font being read. What's wrong with its dictionary never does:
/// what can't be read of that is guessed, and said ([`FontApprox`](crate::FontApprox)).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// What its streams decompress to is past what's left of the budget it's
    /// read within.
    #[error("a font's streams decompress past what's left to decompress")]
    TooLarge,
    /// An object or a stream a font reads doesn't read, for a reason that isn't
    /// the font's: anything but [`Why::Damaged`](crate::Why::Damaged), which is
    /// an object that isn't there. A locked file, a source that failed or a cap
    /// met is the caller's to know, not a font with no widths.
    #[error("{0}")]
    Unreadable(Unreadable),
}

impl From<Unreadable> for Error {
    fn from(unreadable: Unreadable) -> Error {
        Error::Unreadable(unreadable)
    }
}
