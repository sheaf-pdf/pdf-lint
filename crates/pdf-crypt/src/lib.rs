//! PDF's standard security handler (ISO 32000-2, 7.6.4): a file's key from
//! its encryption dictionary and a password, and its strings and streams
//! decrypted with it, or encrypted for a writer; and a new file's key and
//! dictionary.
//!
//! - **Values, not dictionaries.** A [`Dictionary`] is the encryption
//!   dictionary's values, typed; reading them from the file is the
//!   caller's, so it works with any object model.
//! - **Every revision of the standard handler:** 2 to 4 (RC4, and AES-128
//!   in version 4), 5 (Adobe's Extension Level 3, AES-256) and 6 (ISO
//!   32000-2, AES-256), the owner's password or the user's.
//! - **A password as typed, and the bytes tried:** a [`Password`]'s text
//!   [`encodings`](Password::encodings) are its SASLprep form (RFC 4013)
//!   and UTF-8, which revisions 5 and 6 take, and the single-byte
//!   encodings 2 to 4's may have been set in, each [`Encoded`] tagged with
//!   its [`Encoding`]; [`Handler::open`] tries those that fit the file's
//!   revision first, and says which opened it. A new file's password is
//!   [`prepared`](Password::prepared), strictly: SASLprep's form, or why
//!   it can't be one ([`Unusable`]).
//! - **A new file's encryption:** [`Handler::create`] makes a revision 6
//!   dictionary (AES-256) for a prepared password, every permission
//!   granted and the owner's password the user's, and the handler to
//!   encrypt under it. Its key and salts are drawn from a random number
//!   generator the caller passes ([`rand_core`]'s `CryptoRng`, re-exported
//!   so that a caller names the version it takes): pdf-crypt has no
//!   randomness of its own.
//! - **Every fault said, what decrypted kept,** as a checker wants it: AES
//!   ciphertext too short, a part block past the whole ones, padding that
//!   isn't, a crypt filter's method that isn't one, `/Perms` that doesn't
//!   match or doesn't confirm the key.
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
//! is the caller's to know.

mod aes;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod handler;
mod hmac;
mod md5;
mod password;
mod rc4;
mod sha2;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod vectors;
#[cfg(any(test, feature = "writer"))]
pub mod writer;

pub use handler::{Handler, Method};
pub use password::{Encoded, Encoding, Password, Unusable};
pub use rand_core;

/// An encryption dictionary's values, as the standard security handler
/// takes them (ISO 32000-2, Tables 20 and 21). Outside the crate, one is
/// made from [`Dictionary::default`], its fields set after, so that a
/// field it gains breaks no caller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Dictionary {
    /// `/Filter`: the security handler's name, `Standard` for this one.
    pub filter: Vec<u8>,
    /// `/V`, 0 when absent.
    pub v: i64,
    /// `/R`.
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CryptFilter {
    /// Its name in `/CF`.
    pub name: Vec<u8>,
    /// `/CFM`: `None`, `V2`, `AESV2` or `AESV3`.
    pub method: Option<Vec<u8>>,
}

/// Why a handler doesn't open.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Refused {
    /// The dictionary isn't one the standard security handler takes, and
    /// why: another handler, a version or revision it hasn't, `/O`, `/U`
    /// or `/P` (`/OE`, `/UE` or `/Perms` of `/V` 5) missing or not of its
    /// type, `/O` or `/U` too long.
    #[error("{0}")]
    Unsupported(String),
    /// None of the passwords opens it.
    #[error("the password doesn't open it")]
    Password,
}

/// What's wrong with a string's or stream's ciphertext, or the
/// dictionary.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
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
    #[error("its crypt filter's method or name, /{name}, isn't one")]
    Method {
        /// The name.
        name: String,
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
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{fault}")]
pub struct Error {
    /// What decrypted, or none when nothing did and the ciphertext stays.
    pub data: Option<Vec<u8>>,
    /// Why.
    pub fault: Fault,
}

/// The README's examples, run as doctests.
#[cfg(all(doctest, feature = "writer"))]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
