//! Hard errors returned when reading an interactive form.

use pdf_objects::Unreadable;

use crate::limits::Cap;

/// Why reading a form, a field, or a widget failed.
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

impl From<pdf_annots::Error> for Error {
    fn from(err: pdf_annots::Error) -> Self {
        match err {
            pdf_annots::Error::Limit(cap) => Self::Limit(Cap::Annots(cap)),
            pdf_annots::Error::Unreadable(u) => Self::Unreadable(u),
            // `pdf_annots::Error` is non-exhaustive: a kind added later is a cap
            // or a read failure this crate can't name yet.
            _ => Self::Limit(Cap::Fields),
        }
    }
}
