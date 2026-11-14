//! PDF standard security handler (ISO 32000-2 §7.6.4) for password-based
//! decryption, encryption, and permission flags.
//!
//! `pdf-crypt` works with plain typed [`Dictionary`] values rather than a
//! specific PDF object representation, so any reader or writer can use it:
//!
//! - **Open encrypted files ([`Handler::open`]):** Supports revisions 2–4 (RC4
//!   40–128 bit and AES-128 crypt filters), revision 5 (Adobe Extension Level 3
//!   AES-256), and revision 6 (ISO 32000-2 AES-256). [`Password::encodings`]
//!   derives candidate SASLprep, UTF-8, and single-byte encodings ordered for
//!   the file's revision.
//! - **Create AES-256 encryption ([`Handler::create`], [`Protection`]):**
//!   Builds revision 6 encryption dictionaries with user ("open") and/or owner
//!   ("permissions") passwords validated via [`Password::prepared`], drawing
//!   keys and salts from the OS CSPRNG (`getrandom`).
//! - **Permissions ([`Permissions`], [`Printing`], [`Changes`]):** Reads and
//!   encodes `/P` permission bitmasks.
//! - **Decrypt & encrypt strings and streams:** [`Handler::decrypt_string`] and
//!   [`Handler::decrypt_stream`] report malformed padding/blocks/permissions as
//!   typed [`Fault`]s alongside partial plaintext. [`Handler::encrypt_string`]
//!   and [`Handler::encrypt_stream`] draw fresh 16-byte random IVs from [`Ivs`].
//! - **Secret hygiene:** Keys, password buffers, and RustCrypto cipher states
//!   are wiped on drop (`zeroize`). [`KeyCheck`] compares handler keys in
//!   constant time without exposing key bytes, and `Debug` impls never print
//!   keys, passwords, or decrypted data.
//!
//! Stream decompression happens after decryption (see `pdf-filters`), and file-level
//! cleartext exemptions (`/Encrypt` strings, xref streams, signature `/Contents`)
//! are handled by `pdf-structure`.
//!
//! *(The `insecure-test-writer` feature enables hidden deterministic test helpers
//! and must never be enabled in production builds.)*
//!
//! ```
//! use pdf_crypt::{Changes, Encoding, Handler, Ivs, Password, Permissions, Printing, Protection, Refused};
//!
//! // Prepare open (user) and permissions (owner) passwords via RFC 4013 SASLprep.
//! // U+2168 ('Ⅸ') normalizes to "IX".
//! let user = Password("sesame").prepared().unwrap();
//! let owner = Password("\u{2168}").prepared().unwrap();
//! assert_eq!(owner.as_bytes(), b"IX");
//!
//! // Allow low-resolution printing and form filling for the user password.
//! let mut protection = Protection::new(&user);
//! protection.owner = Some(&owner);
//! protection.permissions = Permissions::from(Printing::Low) | Permissions::from(Changes::FillForms);
//! let (writer, dictionary) = Handler::create(&protection).unwrap();
//! assert_eq!((dictionary.v, dictionary.r, dictionary.p), (5, 6, Some(-2108)));
//!
//! // Encrypt a string in object (5, 0) using OS-random AES IVs.
//! let ivs = Ivs::new();
//! let title = writer.encrypt_string((5, 0), b"A title", &ivs).unwrap();
//! assert_ne!(writer.encrypt_string((5, 0), b"A title", &ivs).unwrap(), title);
//!
//! // Open with the user password and inspect granted permissions.
//! let by_user = Handler::open(&dictionary, b"", &Password("sesame").encodings()).unwrap();
//! assert!(!by_user.owner());
//! assert_eq!(by_user.permissions().printing(), Printing::Low);
//! assert!(!by_user.permissions().contains(Permissions::COPY));
//! assert_eq!(by_user.permissions().to_string(), "print, fill forms, accessibility, assemble");
//! assert_eq!(by_user.decrypt_string((5, 0), &title).unwrap(), b"A title");
//!
//! // Open with the owner password (normalizes 'Ⅸ' -> "IX" automatically).
//! let by_owner = Handler::open(&dictionary, b"", &Password("\u{2168}").encodings()).unwrap();
//! assert!(by_owner.owner());
//! assert_eq!(by_owner.opened_by(), Some((0, Encoding::Saslprep)));
//!
//! // Wrong or missing passwords fail with Refused::Password.
//! let wrong = Password("IV").encodings();
//! assert_eq!(Handler::open(&dictionary, b"", &wrong).unwrap_err(), Refused::Password);
//! assert_eq!(Handler::open(&dictionary, b"", &[]).unwrap_err(), Refused::Password);
//! ```

mod aes;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod handler;
mod hmac;
mod ivs;
mod md5;
mod password;
mod permissions;
mod random;
mod rc4;
mod sha2;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod vectors;
#[cfg(any(test, feature = "insecure-test-writer"))]
#[doc(hidden)]
pub mod writer;

use std::fmt;
use std::hash::{Hash, Hasher};

pub use handler::{Handler, KeyCheck, Method, Protection};
pub use ivs::Ivs;
pub use password::{Encoded, Encoding, Password, Prohibition, Unusable};
pub use permissions::{Changes, Permissions, Printing};
/// rand_core, the version the test writer's generator form of
/// [`Handler::create`] takes, with `insecure-test-writer` alone.
#[cfg(any(test, feature = "insecure-test-writer"))]
#[doc(hidden)]
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
/// more joins as a variant of its own. [`Fault::Random`]'s [`Random`] is
/// pdf-crypt's to make, and matched as `Fault::Random(_)`.
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
    /// The operating system's random number generator failed, so AES had no
    /// IV ([`Ivs`]): nothing is encrypted, and nothing is given as
    /// plaintext in its place.
    #[error("{0}")]
    Random(Random),
}

/// The operating system's random number generator failed, and why, in
/// getrandom's words: no IV was drawn ([`Fault::Random`]), or no new file's
/// key and salts ([`Unusable::Random`]). A build for wasm32-unknown-unknown
/// has no generator unless it picks getrandom's backend, and every draw
/// fails so.
///
/// Only pdf-crypt makes one. Two are equal where getrandom's errors are, and
/// hashed by the operating system's error code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the operating system's random number generator failed: {0}")]
pub struct Random(pub(crate) getrandom::Error);

impl Hash for Random {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.raw_os_error().hash(state);
    }
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

/// The README's example, run as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
