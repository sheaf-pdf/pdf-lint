//! Hard errors returned when reading or flattening annotations.

use pdf_objects::Unreadable;

use crate::limits::Cap;

/// Why reading or resolving annotations, destinations, or actions failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// One of [`Limits`](crate::Limits)'s caps was exceeded.
    #[error("exceeded cap on {0}")]
    Limit(Cap),
    /// An object or stream could not be read for a caller-level reason (locked
    /// encryption, reader cap, detached handle, or I/O failure).
    #[error("{0}")]
    Unreadable(#[from] Unreadable),
}

impl From<pdf_semantics::action::Error> for Error {
    fn from(err: pdf_semantics::action::Error) -> Self {
        match err {
            pdf_semantics::action::Error::TooLong => Self::Limit(Cap::ActionChain),
            pdf_semantics::action::Error::Unreadable(u) => Self::Unreadable(u),
            _ => Self::Limit(Cap::ActionChain),
        }
    }
}

impl From<pdf_semantics::dest::Error> for Error {
    fn from(err: pdf_semantics::dest::Error) -> Self {
        match err {
            pdf_semantics::dest::Error::TooManyDests => Self::Limit(Cap::DestTree),
            pdf_semantics::dest::Error::Unreadable(u) => Self::Unreadable(u),
            _ => Self::Limit(Cap::DestTree),
        }
    }
}
