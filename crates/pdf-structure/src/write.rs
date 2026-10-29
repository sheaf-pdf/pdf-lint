//! A writer of a file's structure (ISO 32000-2, 7.5): its cross-reference
//! sections, as a table or a stream, and object streams of the objects
//! that may go in one, deflated through [`pdf_filters`]. The objects are
//! written by pdf-objects' writer ([`pdf_objects::serialize`]), each at the
//! position a section lists it at.
//!
//! - **Cross-reference sections** ([`Section`]): a whole file's one
//!   subsection, `[0, /Size)`, every number it doesn't list written as
//!   free; or an update's, runs of the numbers it lists, which are only
//!   those it writes (7.5.6). Free entries are linked in a list from entry
//!   0, each at the generation its number is to be used at next (7.5.4).
//!   An object in an object stream is listed as its container and its
//!   place there, which only a stream can say (7.5.8.3). A whole file's
//!   stream is deflated under PNG's Up predictor, each row less the row
//!   above it, as qpdf writes its own; an update's is written as it is.
//! - **Positions counted from the header,** where `%PDF-` starts, which a
//!   whole file's section has at its start: PDFium, pdf.js and qpdf count
//!   them so, past any bytes a writer put before the header, where ISO
//!   32000-2 counts them from the file's start (7.5.4), as this crate's
//!   reader does. The two differ only for an update of a file with bytes
//!   before its header ([`Section::append`]), whose sections this crate's
//!   reader reads only by scanning.
//! - **Object streams** (7.5.7): which objects may go in one
//!   ([`packable`]), and a container of them ([`container`]), its members
//!   direct after a header of where each starts, deflated, no bigger than
//!   the reader decodes a stream to ([`MAX_DATA`]).
//! - **Deflated** by the backend and at the level a [`Deflate`] says.
//! - **Encrypted as it's written** (7.6): an encrypted file's every string
//!   and stream's data, as pdf-objects' writer writes each, by an
//!   [`Encryptor`], a reading's ([`Encryptor::of`]) or a new file's
//!   ([`Encryptor::create`]), through what it gives for the file's objects
//!   ([`Encrypting`]): with the object's key, AES's IV derived from `n g
//!   obj` and the data, so the same object encrypts to the same bytes on
//!   every save. What it leaves in the clear is the list the reader
//!   decrypts by, one list both ways (see [`bytes`](crate::bytes), "What's
//!   encrypted"). Under RC4 a changed object written again at its number
//!   and generation is encrypted with the keystream its earlier version
//!   was, which leaks what changed to one who holds both (pdf-crypt's
//!   `Method::Rc4`); a file written whole under a new AES-256 encryption
//!   ([`Encryptor::create`]) doesn't.
//!
//! What it can't write is refused ([`Error`]), and then nothing is
//! written.

mod crypt;
mod objstm;
#[cfg(test)]
pub(crate) mod tests;
mod xref;

use pdf_filters::{Backend, Encoder, Level, Prediction, Unencodable};
use pdf_objects::object::ObjectId;
use pdf_objects::serialize::{self, Refused};

// The tests' file writer writes its encrypted files' dictionaries so too.
#[cfg(any(test, feature = "fixtures"))]
pub(crate) use crypt::encryption;
pub use crypt::{Encrypting, Encryptor, Unencryptable};
pub use objstm::{MAX_DATA, MEMBERS, container, fits, packable};
pub use xref::{Entry, Section};

/// The object numbers a written file may use are below it: the
/// 8,388,607 indirect objects a file may hold (ISO 32000-1, Annex C), so
/// that a whole file's cross-reference section, which lists every number
/// from 0, has at most that many rows. It's below 2^24 too: RC4's and
/// AES-128's key for an object is made from the low three bytes of its
/// number (ISO 32000-2, 7.6.3, Algorithm 1), so no two numbers below it
/// share a key.
pub const NUMBERS: u32 = 8_388_607;

/// How the writer deflates what it compresses, a whole file's
/// cross-reference stream and each object stream: on which of
/// pdf-filters' backends, and how hard. A caller sets them on a copy of
/// [`Deflate::DEFAULT`], as it does a reading's
/// [`Options`](crate::Options).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Deflate {
    /// The backend that deflates: by default pdf-filters'
    /// `Backend::MinizOxide`, which every build of this crate has;
    /// `Backend::ZlibRs` with its `zlib-rs` feature. The two deflate to
    /// different bytes, each zlib data any inflater reads.
    pub backend: Backend,
    /// How hard it works to make the data small: pdf-filters'
    /// `Level::DEFAULT`, 6, by default.
    pub level: Level,
}

impl Deflate {
    /// How the writer deflates unless it's told otherwise: miniz_oxide, at
    /// level 6.
    pub const DEFAULT: Deflate = Deflate {
        backend: Backend::MinizOxide,
        level: Level::DEFAULT,
    };

    /// `data`, first predicted as `predictor` says, deflated at the end of
    /// `out`.
    fn encode(self, data: &[u8], predictor: Prediction, out: &mut Vec<u8>) -> Result<(), Error> {
        let flate = Encoder::Flate {
            level: self.level,
            predictor,
        };
        pdf_filters::encode(data, &[flate], self.backend, out).map_err(Error::Unencodable)
    }
}

impl Default for Deflate {
    fn default() -> Deflate {
        Deflate::DEFAULT
    }
}

/// Why the writer wrote nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An offset past a cross-reference table's ten digits.
    #[error("a cross-reference table can't list offset {0}")]
    TooFar(u64),
    /// An object in an object stream, by number: only a cross-reference
    /// stream can list one.
    #[error("a cross-reference table can't list object {0}, which is in an object stream")]
    Packed(u32),
    /// A position before the header, which a section's positions are
    /// counted from ([`Section::append`]).
    #[error("position {pos} is before the header, at {header}, which positions are counted from")]
    BeforeHeader {
        /// The position.
        pos: usize,
        /// Where the header starts.
        header: usize,
    },
    /// A value that pdf-objects' writer refuses: its error, which says
    /// why and in which object.
    #[error("{0}")]
    Serialize(serialize::Error),
    /// Data that pdf-filters doesn't encode as it was asked to: its error.
    #[error("data that doesn't encode: {0}")]
    Unencodable(Unencodable),
    /// A string or a stream's data of object `id`, in a file written
    /// encrypted, that isn't encrypted, and why: nothing is written in the
    /// clear in its place.
    #[error("object {} {} can't be encrypted: {why}", id.0, id.1)]
    Encrypt {
        /// The object.
        id: ObjectId,
        /// Why.
        why: Unencryptable,
    },
    /// An encrypted file that no password opened, whether its strings and
    /// streams are encrypted or not: there's no key to encrypt what's
    /// written into it by ([`Encryptor::of`]).
    #[error("it's encrypted, and no password opened it, so there's no key to encrypt by")]
    Locked,
    /// An encrypted file whose security handler the reader can't open, and
    /// why ([`Encryptor::of`]).
    #[error("it's encrypted by a security handler the reader can't open: {0}")]
    Unsupported(String),
}

/// pdf-objects' writer's refusal, through an [`Encrypting`]: the value it
/// can't write ([`Error::Serialize`]), or the string or stream's data it
/// can't encrypt ([`Error::Encrypt`]).
impl From<Refused<Unencryptable>> for Error {
    fn from(refused: Refused<Unencryptable>) -> Error {
        match refused {
            Refused::Write(e) => Error::Serialize(e),
            Refused::Encrypt { id, why } => Error::Encrypt { id, why },
        }
    }
}
