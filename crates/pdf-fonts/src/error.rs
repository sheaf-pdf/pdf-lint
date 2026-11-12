//! Why a font isn't read.

use pdf_objects::Unreadable;

/// What stops a font being read. What's wrong with its dictionary never does:
/// what can't be read of that is guessed, and said ([`FontApprox`](crate::FontApprox)).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// What its streams decompress to is past what's left of the budget it's
    /// read within ([`Why::Limit`](pdf_objects::Why::Limit)): the caller's budget alone,
    /// never a cap, so that read within more it may read.
    #[error("a font's streams decompress past what's left to decompress")]
    TooLarge,
    /// An object or a stream a font reads doesn't read, for a reason that isn't
    /// the font's: anything but [`Why::Damaged`](pdf_objects::Why::Damaged), which is
    /// an object that isn't there or a stream whose decode fails before it gives a
    /// byte, and [`Why::Filter`](pdf_objects::Why::Filter), a stream whose filters the
    /// reader doesn't decode, each read as none; nor a stream's
    /// [`Why::Limit`](pdf_objects::Why::Limit), which is [`Error::TooLarge`]. A locked
    /// file, a source that failed or a cap met, an object or a stream past a cap of
    /// what gives it ([`Why::Capped`](pdf_objects::Why::Capped)), is the caller's to
    /// know, not a font with no widths. It's the first that didn't read: the reading
    /// goes on past it as if it weren't there, decoding no stream after it, so what's
    /// told after it is of a font without it.
    #[error("{0}")]
    Unreadable(Unreadable),
}

impl From<Unreadable> for Error {
    fn from(unreadable: Unreadable) -> Error {
        Error::Unreadable(unreadable)
    }
}
