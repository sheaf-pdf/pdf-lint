//! PDF's standard security handler (ISO 32000-2, 7.6.4): a file's key from
//! its encryption dictionary and a password, and its strings and streams
//! decrypted with it, or encrypted for a writer; and a new file's key and
//! dictionary.
//!
//! - **Values, not dictionaries.** A [`Dictionary`] is the encryption
//!   dictionary's values, typed; reading them from the file is the
//!   caller's, so it works with any object model. One the standard handler
//!   doesn't take is refused saying why ([`Refused`]): another security
//!   handler's by its name, a version it isn't by its `/V` and `/R`.
//! - **Every revision of the standard handler:** 2 to 4 (RC4, and AES-128
//!   in version 4), 5 (Adobe's Extension Level 3, AES-256) and 6 (ISO
//!   32000-2, AES-256), the owner's password or the user's.
//! - **A password as typed, and the bytes tried:** a [`Password`]'s text
//!   [`encodings`](Password::encodings) are its SASLprep form (RFC 4013)
//!   as of Unicode 3.2, and by a later Unicode where that's another, and
//!   UTF-8, which revisions 5 and 6 take, and the single-byte encodings 2
//!   to 4's may have been set in, each [`Encoded`] tagged with its
//!   [`Encoding`]; [`Handler::open`] tries those that fit the file's
//!   revision first, and says which opened it. A new file's password is
//!   [`prepared`](Password::prepared), strictly: SASLprep's form, the same
//!   by both Unicodes, or why it can't be one ([`Unusable`]).
//! - **A new file's encryption:** [`Handler::create`] makes a revision 6
//!   dictionary (AES-256) for a prepared password, every permission
//!   granted and the owner's password the user's, and the handler to
//!   encrypt under it. Its key and salts are drawn from a random number
//!   generator the caller passes ([`rand_core`]'s `CryptoRng`, re-exported
//!   so that a caller names the version it takes): pdf-crypt has no
//!   randomness of its own.
//! - **Every fault said, what decrypted kept,** as a checker wants it: AES
//!   ciphertext too short, a part block past the whole ones, padding that
//!   isn't, a crypt filter's method that isn't one, AES that the file's key
//!   isn't for, `/Perms` that doesn't match or doesn't confirm the key.
//! - **Encrypting for a writer:** [`Handler::encrypt_string`] and
//!   [`Handler::encrypt_stream`] encrypt as the handler decrypts, AES's IV
//!   derived from the data and what the caller names its object by, so
//!   the same object encrypts the same on every save and no IV serves two
//!   plaintexts. RC4 takes no IV; a method that isn't one is refused.
//! - **Keys kept in, and wiped:** a [`Handler`] decrypts and encrypts with
//!   the file's key and each object's, and gives out neither, its `Debug`
//!   included. Each is wiped when it's dropped (RustCrypto's `zeroize`), as
//!   is every buffer a password or a key passes through on the way to one.
//! - **RustCrypto's primitives:** MD5 (`md-5`), SHA-256, SHA-384 and
//!   SHA-512 (`sha2`), HMAC-SHA256 (`hmac`), RC4 (`rc4`), and AES-128 and
//!   AES-256 (`aes`, in CBC by `cbc`), each built to wipe its own state.
//!
//! Decrypting is a step before the filters (pdf-filters decodes those),
//! and which strings and streams aren't encrypted at all, the encryption
//! dictionary's own, a cross-reference stream, a signature's `/Contents`,
//! is the caller's to know: pdf-structure's reader decrypts by one list of
//! it, and its writer encrypts by the same. Under RC4 a changed object
//! written again at its number and generation leaks what changed
//! ([`Method::Rc4`]).
//!
//! The `writer` feature, off by default, writes an encryption dictionary of
//! every revision and encrypts under IVs given, its salts and keys given
//! too: for test files, not for protecting a document. docs.rs documents
//! the crate without it.
//!
//! A new file's encryption, from a seeded generator (RustCrypto's
//! `chacha20`, with its `rng` feature) where a program would pass the
//! operating system's, and the file opened by its password:
//!
//! ```
//! use chacha20::ChaCha20Rng;
//! use pdf_crypt::rand_core::SeedableRng;
//! use pdf_crypt::{Encoding, Handler, Password, Protection, Refused};
//!
//! // The password prepared: Ⅸ's SASLprep form is `IX`.
//! let password = Password("\u{2168}").prepared().unwrap();
//! assert_eq!(password.as_bytes(), b"IX");
//! let mut rng = ChaCha20Rng::seed_from_u64(7);
//! let (writer, dictionary) = Handler::create(&Protection::new(&password), &mut rng).unwrap();
//! assert_eq!((dictionary.v, dictionary.r, dictionary.p), (5, 6, Some(-4)));
//!
//! // A writer encrypts a string of object 5 under it, AES's IV derived from
//! // what it names the object by and the string: the same each time.
//! let title = writer.encrypt_string((5, 0), b"the Info dictionary", b"A title").unwrap();
//! assert_eq!(writer.encrypt_string((5, 0), b"the Info dictionary", b"A title").unwrap(), title);
//!
//! // A reader opens it by the password as typed, in its SASLprep form, as the
//! // owner's (revision 6 takes no /ID), and decrypts.
//! let reader = Handler::open(&dictionary, b"", &Password("\u{2168}").encodings()).unwrap();
//! assert!(reader.owner());
//! assert_eq!(reader.opened_by(), Some((0, Encoding::Saslprep)));
//! assert_eq!(reader.decrypt_string((5, 0), &title).unwrap(), b"A title");
//!
//! // Another password doesn't open it.
//! let wrong = Password("IV").encodings();
//! assert_eq!(Handler::open(&dictionary, b"", &wrong).unwrap_err(), Refused::Password);
//! ```

mod aes;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod handler;
mod hmac;
mod md5;
mod password;
mod permissions;
mod rc4;
mod sha2;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod vectors;
#[cfg(any(test, feature = "writer"))]
pub mod writer;

use std::fmt;

pub use handler::{Handler, Method, Protection};
pub use password::{Encoded, Encoding, Password, Unusable};
pub use permissions::{Changes, Permissions, Printing};
pub use rand_core;

/// An encryption dictionary's values, as the standard security handler
/// takes them (ISO 32000-2, Tables 20 and 21). Outside the crate, one is
/// made from [`Dictionary::default`], its fields set after, so that a
/// field it gains breaks no caller.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Dictionary {
    /// `/Filter`: the security handler's name, `Standard` for this one;
    /// empty when it's absent or not a name, which is refused as a
    /// dictionary that's wrong ([`Refused::Malformed`]).
    pub filter: Vec<u8>,
    /// `/V`, 0 when absent.
    pub v: i64,
    /// `/R`, 0 when absent.
    pub r: i64,
    /// `/Length`, in bits, of a dictionary of `/V` 2.
    pub length: Option<i64>,
    /// `/O`, when it's a string.
    pub o: Option<Vec<u8>>,
    /// `/U`, when it's a string.
    pub u: Option<Vec<u8>>,
    /// `/OE`, of `/V` 5, when it's a string.
    pub oe: Option<Vec<u8>>,
    /// `/UE`, of `/V` 5, when it's a string.
    pub ue: Option<Vec<u8>>,
    /// `/Perms`, of `/V` 5, when it's a string.
    pub perms: Option<Vec<u8>>,
    /// `/P`, when it's an integer.
    pub p: Option<i64>,
    /// `/EncryptMetadata`, when it's a boolean; it counts from `/V` 4.
    pub encrypt_metadata: Option<bool>,
    /// `/CF`'s crypt filters.
    pub crypt_filters: Vec<CryptFilter>,
    /// `/StmF`: the crypt filter streams are encrypted with.
    pub stm_f: Option<Vec<u8>>,
    /// `/StrF`: the crypt filter strings are encrypted with.
    pub str_f: Option<Vec<u8>>,
}

/// One of `/CF`'s crypt filters. Outside the crate, one is made from
/// [`CryptFilter::default`], its fields set after.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct CryptFilter {
    /// Its name in `/CF`.
    pub name: Vec<u8>,
    /// `/CFM`: `None`, `V2`, `AESV2` or `AESV3`.
    pub method: Option<Vec<u8>>,
}

/// Why a handler doesn't open. A dictionary refused for more than one
/// reason is refused for the first: its `/Filter`, then its `/V` and `/R`,
/// then its other values.
#[derive(Clone, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Refused {
    /// The dictionary is another security handler's, by its `/Filter`:
    /// the public-key handler's (`/Adobe.PubSec`, ISO 32000-2, 7.6.5),
    /// say, whose files open by a certificate, not a password. Its name
    /// is written lossily as UTF-8.
    #[error("its security handler is /{}, not /Standard", String::from_utf8_lossy(.name))]
    #[non_exhaustive]
    OtherHandler {
        /// Its `/Filter`, as written: never empty, as a dictionary with
        /// none is one that's wrong ([`Refused::Malformed`]).
        name: Vec<u8>,
    },
    /// The dictionary's `/V` and `/R` are a pair the standard security
    /// handler isn't, as it reads them. A consumer reads `r` to tell a
    /// later handler's revision (7 and up: ISO/TS 32003's AES-GCM is
    /// `/V 6 /R 7`) from a pair that's undocumented or not one (`/V` 0,
    /// which is `/V` absent too, or 3; an `/R` below 2).
    #[error("it's /V {v} /R {r}, which the standard security handler isn't")]
    #[non_exhaustive]
    Version {
        /// `/V`, 0 when absent.
        v: i64,
        /// `/R`, 0 when absent.
        r: i64,
    },
    /// The dictionary is wrong, and why: no `/Filter`; `/O`, `/U` or `/P`
    /// (`/OE`, `/UE` or `/Perms` of `/V` 5) missing or not of its type;
    /// `/O` or `/U` too long. A reader gives it too, in its own words, for
    /// an `/Encrypt` that isn't a dictionary.
    #[error("{0}")]
    Malformed(String),
    /// None of the passwords opens it.
    #[error("the password doesn't open it")]
    Password,
}

/// What's wrong with a string's or stream's ciphertext, or the
/// dictionary.
///
/// Each variant's fields are the whole of what it says (a length, a count,
/// a name), so outside the crate one is built by a literal, as a test
/// compares with it, and matched without `..`; a fault found that says
/// more joins as a variant of its own.
#[derive(Clone, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Fault {
    /// AES ciphertext of `len` bytes, with no whole block past its IV:
    /// shorter than an IV, nothing decrypted, it stays ciphertext; just an
    /// IV, it decrypts to nothing.
    #[error("its AES ciphertext has no whole block past its IV")]
    Short {
        /// Its length.
        len: usize,
    },
    /// AES ciphertext with `extra` bytes past its whole blocks (a
    /// stream's white-space past them aside). The whole blocks decrypted
    /// and kept, padding and all, as the last needn't be the plaintext's
    /// last; the rest left off.
    #[error("its AES ciphertext has a part block past its whole blocks")]
    PartBlock {
        /// How many.
        extra: usize,
    },
    /// AES plaintext whose last block isn't padded as AES-CBC pads it
    /// (RFC 8018, 6.1.1). Decrypted, the block kept whole.
    #[error("its AES plaintext isn't padded")]
    Padding,
    /// A crypt filter's method that isn't one, or a crypt filter the
    /// dictionary hasn't, named `name`. Nothing decrypted.
    #[error("its crypt filter's method or name, /{}, isn't one", String::from_utf8_lossy(.name))]
    Method {
        /// The name, as written, as [`Method::Unknown`] holds it; what the
        /// fault says writes it lossily as UTF-8.
        name: Vec<u8>,
    },
    /// A crypt filter's AES that the file's key of `len` bytes isn't for:
    /// `/AESV3` below `/V` 5, `/AESV2` in a file whose key is RC4's of
    /// under 128 bits. Nothing decrypted.
    #[error("its crypt filter's AES isn't for the file's key of {len} bytes")]
    Key {
        /// The key's length.
        len: usize,
    },
    /// `/Perms`, decrypted, confirms the key (`adb`, Algorithm 13) but
    /// isn't `/P` and `/EncryptMetadata` as revisions 5 and 6 write it.
    /// The file opens all the same.
    #[error("its /Perms doesn't match its /P and /EncryptMetadata")]
    Perms,
    /// `/Perms` doesn't decrypt to `adb` by the key the password unwrapped
    /// from `/OE` or `/UE`: one of them is wrong, and nothing says which.
    /// The file opens by that key, when no other password or route gives
    /// one `/Perms` confirms, but what it decrypts may not be plaintext.
    #[error("its /Perms doesn't confirm the key its /OE or /UE unwraps to")]
    Unconfirmed,
}

/// Ciphertext that didn't decrypt cleanly: what decrypted, if anything,
/// and why it's not clean.
///
/// Its `Debug` gives the length of what decrypted, not its bytes, which
/// are a string's or a stream's plaintext:
/// `Error { data: Some(23 bytes), fault: Padding }`. So two errors whose
/// data are the same length have the same `Debug`, whatever their bytes:
/// a test compares errors by `==`, never by their `Debug` text, and takes
/// `data` itself where it wants the bytes.
///
/// Exhaustive on purpose: what decrypted and why, the two things a decrypt
/// that isn't clean gives, each field public, so an error is built by a
/// literal and taken apart by its fields; a field more would be a breaking
/// release.
#[derive(Clone, PartialEq, Eq, thiserror::Error)]
#[error("{fault}")]
pub struct Error {
    /// What decrypted, or none when nothing did and the ciphertext stays.
    pub data: Option<Vec<u8>>,
    /// Why.
    pub fault: Fault,
}

impl fmt::Debug for Error {
    /// What decrypted by its length, not its bytes, and why.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error")
            .field("data", &self.data.as_ref().map(|data| Length(data.len())))
            .field("fault", &self.fault)
            .finish()
    }
}

/// A length of bytes, written `23 bytes`, where a `Debug` leaves the bytes
/// out.
struct Length(usize);

impl fmt::Debug for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} bytes", self.0)
    }
}

/// The README's examples, run as doctests.
#[cfg(all(doctest, feature = "writer"))]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
