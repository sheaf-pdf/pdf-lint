//! PDF's standard security handler (ISO 32000-2, 7.6.4), decrypting: the
//! keys from a file's encryption dictionary and password, and its strings
//! and streams decrypted with them.
//!
//! Its primitives are its own, and need nothing else: MD5, SHA-256,
//! SHA-384 and SHA-512, RC4, and AES-128 and AES-256, each checked against
//! its standard's test vectors.

// The handler, next, uses them; until then only the tests and the fuzz
// target do.
#![cfg_attr(not(any(test, fuzzing)), allow(dead_code))]

mod aes;
#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzz;
mod md5;
mod rc4;
mod sha2;
#[cfg(test)]
mod vectors;
