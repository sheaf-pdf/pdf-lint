//! PDF's standard security handler (ISO 32000-2, 7.6.4), decrypting: a
//! file's key from its encryption dictionary and a password, and its
//! strings and streams decrypted with it.
//!
//! - **Values, not dictionaries.** A [`Dictionary`] is the encryption
//!   dictionary's values, typed; reading them from the file is the
//!   caller's, so it works with any object model.
//! - **Every revision of the standard handler:** 2 to 4 (RC4, and AES-128
//!   in version 4), 5 (Adobe's Extension Level 3, AES-256) and 6 (ISO
//!   32000-2, AES-256), the owner's password or the user's.
//! - **Every fault said, what decrypted kept,** as a checker wants it: AES
//!   ciphertext too short, a part block past the whole ones, padding that
//!   isn't, a crypt filter's method that isn't one, `/Perms` that doesn't
//!   match.
//! - **Its own primitives,** needing nothing else: MD5, SHA-256, SHA-384
//!   and SHA-512, RC4, and AES-128 and AES-256, each checked against its
//!   standard's test vectors and against RustCrypto's.
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
mod md5;
mod rc4;
mod sha2;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod vectors;
#[cfg(any(test, feature = "writer"))]
pub mod writer;

use std::fmt;

pub use handler::{Handler, Method};

/// An encryption dictionary's values, as the standard security handler
/// takes them (ISO 32000-2, Tables 20 and 21).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dictionary {
    /// `/Filter`: the security handler's name, `Standard` for this one.
    pub filter: Vec<u8>,
    /// `/V`, 0 when absent.
    pub v: i64,
    /// `/R`.
    pub r: i64,
    /// `/Length`, in bits, of a dictionary of `/V` 2.
    pub length: Option<i64>,
    /// `/O`.
    pub o: Vec<u8>,
    /// `/U`.
    pub u: Vec<u8>,
    /// `/OE`, of revision 5 or 6.
    pub oe: Vec<u8>,
    /// `/UE`, of revision 5 or 6.
    pub ue: Vec<u8>,
    /// `/Perms`, of revision 5 or 6.
    pub perms: Vec<u8>,
    /// `/P`.
    pub p: i64,
    /// `/EncryptMetadata`, when it's a boolean; it counts from `/V` 4.
    pub encrypt_metadata: Option<bool>,
    /// `/CF`'s crypt filters.
    pub crypt_filters: Vec<CryptFilter>,
    /// `/StmF`: the crypt filter streams are encrypted with.
    pub stm_f: Option<Vec<u8>>,
    /// `/StrF`: the crypt filter strings are encrypted with.
    pub str_f: Option<Vec<u8>>,
}

/// One of `/CF`'s crypt filters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CryptFilter {
    /// Its name in `/CF`.
    pub name: Vec<u8>,
    /// `/CFM`: `None`, `V2`, `AESV2` or `AESV3`.
    pub method: Option<Vec<u8>>,
}

/// Why a handler doesn't open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The dictionary isn't one the standard security handler takes, and
    /// why: another handler, a version or revision it hasn't, `/O` or `/U`
    /// missing or too long.
    Unsupported(String),
    /// None of the passwords opens it.
    Password,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::Unsupported(why) => write!(f, "{why}"),
            Refused::Password => write!(f, "the password doesn't open it"),
        }
    }
}

impl std::error::Error for Refused {}

/// What's wrong with a string's or stream's ciphertext, or the
/// dictionary.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Fault {
    /// AES ciphertext of `len` bytes, with no whole block past its IV.
    /// Nothing decrypted: it stays ciphertext.
    Short {
        /// Its length.
        len: usize,
    },
    /// AES ciphertext with `extra` bytes past its whole blocks, which
    /// aren't white-space. The whole blocks decrypted, the rest left off.
    PartBlock {
        /// How many.
        extra: usize,
    },
    /// AES plaintext whose last block isn't padded as AES-CBC pads it
    /// (RFC 8018, 6.1.1). Decrypted, the block kept whole.
    Padding,
    /// A crypt filter's method that isn't one, or a crypt filter the
    /// dictionary hasn't, named `name`. Nothing decrypted.
    Method {
        /// The name.
        name: String,
    },
    /// A crypt filter's AES, given the file's key of `len` bytes, neither
    /// 16 nor 32, as `/AESV3` is in a file whose key is RC4's. Nothing
    /// decrypted.
    Key {
        /// The key's length.
        len: usize,
    },
    /// `/Perms`, decrypted, isn't `/P`, `/EncryptMetadata` and `adb` as
    /// revisions 5 and 6 write it (Algorithm 13). The file opens all the
    /// same.
    Perms,
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fault::Short { len } => write!(
                f,
                "its AES ciphertext is {len} bytes, no whole block past the IV"
            ),
            Fault::PartBlock { extra } => write!(
                f,
                "its AES ciphertext has {extra} bytes past its whole blocks"
            ),
            Fault::Padding => write!(f, "its AES plaintext isn't padded"),
            Fault::Method { name } => {
                write!(f, "its crypt filter's method or name, /{name}, isn't one")
            }
            Fault::Key { len } => write!(
                f,
                "its crypt filter's AES has the file's key of {len} bytes"
            ),
            Fault::Perms => write!(f, "its /Perms doesn't match its /P and /EncryptMetadata"),
        }
    }
}

/// Ciphertext that didn't decrypt cleanly: what decrypted, if anything,
/// and why it's not clean.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// What decrypted, or none when nothing did and the ciphertext stays.
    pub data: Option<Vec<u8>>,
    /// Why.
    pub fault: Fault,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.fault)
    }
}

impl std::error::Error for Error {}

/// The README's examples, run as doctests.
#[cfg(all(doctest, feature = "writer"))]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
