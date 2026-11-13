//! Why a call gives nothing: one error over every crate's.

use std::io;

use pdf_content_streams::Pushed;
use pdf_objects::Unreadable;
use pdf_semantics::{pages, xmp};

/// Why a call gives nothing: the error of the crate it was given by, as that
/// crate gives it, or the facade's own. Each wraps the crate's error
/// unchanged, and says what it says.
/// Not exhaustive, as more calls, and the reasons they give nothing, may come.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The file didn't open: its path ([`open`](crate::open)), or what
    /// identifies it, didn't read.
    #[error("{0}")]
    Io(io::Error),
    /// The reading's (pdf-structure's): its source didn't give the bytes
    /// asked of it.
    #[error("{0}")]
    Read(pdf_structure::read::Error),
    /// The trailer's `/Root` names no dictionary: the file has no catalog,
    /// so no page tree and no metadata stream.
    #[error("the trailer's /Root names no catalog")]
    NoCatalog,
    /// An object on the way doesn't read, and why, as the reading says it
    /// (pdf-objects'): the catalog, the document information dictionary, a
    /// page's resources, or a stream's data. One that's past a cap the
    /// reader reads under, behind a password, or whose bytes the source
    /// didn't give is never read as none.
    #[error("{0}")]
    Unreadable(Unreadable),
    /// The page tree didn't read (pdf-semantics').
    #[error("{0}")]
    Pages(pages::Error),
    /// A page's streams weren't joined (pdf-content-streams'): past
    /// [`Limits::content`](crate::Limits::content)'s cap on what they come
    /// to, or a stream that doesn't read, its [`Why`](pdf_objects::Why)
    /// kept.
    #[error("{0}")]
    Content(Pushed<Unreadable>),
    /// The metadata stream's packet wasn't read (pdf-semantics'). Boxed, as
    /// it holds what was read, which would make every result of the facade's
    /// as large.
    #[error("{0}")]
    Xmp(Box<xmp::Error>),
    /// The file's linearization wasn't checked (pdf-linearize's).
    #[error("{0}")]
    Linearize(pdf_linearize::family::Error),
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Error {
        Error::Io(error)
    }
}

impl From<pdf_structure::read::Error> for Error {
    fn from(error: pdf_structure::read::Error) -> Error {
        Error::Read(error)
    }
}

impl From<Unreadable> for Error {
    fn from(unreadable: Unreadable) -> Error {
        Error::Unreadable(unreadable)
    }
}

impl From<pages::Error> for Error {
    fn from(error: pages::Error) -> Error {
        Error::Pages(error)
    }
}

impl From<Pushed<Unreadable>> for Error {
    fn from(pushed: Pushed<Unreadable>) -> Error {
        Error::Content(pushed)
    }
}

impl From<xmp::Error> for Error {
    fn from(error: xmp::Error) -> Error {
        Error::Xmp(Box::new(error))
    }
}

impl From<pdf_linearize::family::Error> for Error {
    fn from(error: pdf_linearize::family::Error) -> Error {
        Error::Linearize(error)
    }
}
