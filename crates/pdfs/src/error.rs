//! Why a call gives nothing: one error over every crate's.

use std::io;

use pdf_content_streams::Pushed;
use pdf_crypt::Unusable;
use pdf_objects::Unreadable;
use pdf_semantics::{pages, xmp};
use pdf_structure::write;

/// Why a call gives nothing: the error of the crate it was given by, as that
/// crate gives it, or the facade's own. Each wraps the crate's error
/// unchanged, and says what it says.
/// Not exhaustive, as more calls, and the reasons they give nothing, may come.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A path didn't take what was asked of it: a file to read didn't open,
    /// or what identifies it didn't read ([`open`](crate::open)); a path to
    /// save at didn't say whether it's the file read, or didn't take what's
    /// written ([`Document::save`](crate::Document::save) and the other saves
    /// to a path); a file an update goes after didn't open for appending, or
    /// didn't take what's appended
    /// ([`Document::save_update`](crate::Document::save_update)).
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
    /// as large. Its `Debug` gives the claims read and the XML reader's words
    /// by their length, not the packet's text.
    #[error("{0}")]
    Xmp(Box<xmp::Error>),
    /// The file's linearization wasn't checked, or a document wasn't written
    /// linearized (pdf-linearize's).
    #[error("{0}")]
    Linearize(pdf_linearize::family::Error),
    /// A document wasn't written, whole or as an update, by pdf-structure's
    /// writer, which refused it with nothing written; or a file wasn't edited
    /// ([`Pdf::edit`](crate::Pdf::edit)), as no password opened it or its
    /// security handler is one the reader can't open, so there's nothing to
    /// write its strings and streams from.
    #[error("{0}")]
    Write(write::Error),
    /// A new document's page tree wasn't written (pdf-semantics'): it has no
    /// page, or more than the tree's caps take.
    #[error("{0}")]
    PageTree(pages::WriteError),
    /// Claims that wouldn't read back as they're given, written as XMP
    /// (pdf-semantics').
    #[error("{0}")]
    Claims(xmp::WriteError),
    /// A protection pdf-crypt makes no encryption of.
    #[error("{0}")]
    Protection(Unusable),
    /// Claims written into a document whose catalog names a metadata stream
    /// already, whose packet the claims' alone would replace, and what else
    /// it says be lost.
    #[error("the catalog names a metadata stream already, which the claims alone would replace")]
    HasMetadata,
    /// A page added to an edited document whose catalog's `/Pages` names no
    /// page tree node to add it under: none by reference, a page, or one
    /// whose `/Count` isn't an integer, or can't count the pages added
    /// within 64 bits, or whose `/Kids` isn't an array.
    #[error("the catalog's /Pages names no page tree node a page can be added under")]
    NoPageTree,
    /// An update of a document no file was read for: it has nothing to go
    /// after.
    #[error("the document wasn't read from a file, so there's nothing for an update to go after")]
    NotRead,
    /// An update to a file that isn't the file read as it was: another, or
    /// that one since changed, so nothing was appended to it.
    #[error(
        "the file isn't the one the document was read from, as it was, so nothing was appended"
    )]
    NotTheBase,
    /// A save at the path of the file the document's reading reads by
    /// position, which would read the new bytes at the old places after:
    /// nothing was written.
    #[error("the file is the one the document is read from, by position, so it isn't written over")]
    SameFile,
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

impl From<write::Error> for Error {
    fn from(error: write::Error) -> Error {
        Error::Write(error)
    }
}

impl From<pages::WriteError> for Error {
    fn from(error: pages::WriteError) -> Error {
        Error::PageTree(error)
    }
}
