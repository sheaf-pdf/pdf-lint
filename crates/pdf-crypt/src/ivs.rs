//! The IVs AES encrypts under (ISO 32000-2, 7.6.3.2 and 7.6.3.3, which ask
//! for random ones): 16 bytes each from the operating system's random number
//! generator, drawn a buffer at a time for one write.

use std::fmt;
use std::sync::{Mutex, PoisonError};

use zeroize::Zeroizing;

use crate::{Fault, Handler, Random};

/// How many bytes are drawn from the operating system at a time: 256 IVs.
const DRAWN: usize = 4096;

/// The IVs one write encrypts its AES strings and streams under
/// ([`Handler::encrypt_string`], [`Handler::encrypt_stream`]): each 16 bytes
/// from the operating system's random number generator (getrandom's), so no
/// two plaintexts share one and nothing of the key is in one. Two saves of
/// one document under the same key differ in every AES ciphertext written
/// anew, and equal plaintexts encrypt apart.
///
/// Nothing is drawn until the first IV is asked for; then 4,096 bytes at a
/// time, 256 IVs, which the threads of one write share, as it's `Sync`. The
/// bytes are wiped when it's dropped. It isn't `Clone`, as a clone would
/// give out the same IVs again: a write makes its own, by [`Ivs::new`], and
/// drops it when it's written. RC4, `/Identity` and `/None` take no IV.
///
/// A process that forks while a write holds one gives the child the IVs
/// the parent hasn't used yet, which both would then hand out: a writer
/// that forks mid-write and writes on in both makes an `Ivs` in each.
///
/// Its `Debug` says where its IVs come from, not their bytes.
pub struct Ivs {
    source: Source,
    drawn: Mutex<Drawn>,
}

/// Where an [`Ivs`]' IVs come from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The operating system's random number generator.
    Os,
    /// The same IV each time, for tests.
    #[cfg(any(test, feature = "insecure-test-writer"))]
    Fixed([u8; 16]),
    /// The first 16 bytes of HMAC-SHA256, under the file's key, of what the
    /// object is written as and the data, for tests (`Ivs::derived`).
    #[cfg(any(test, feature = "insecure-test-writer"))]
    Derived,
}

/// The bytes drawn from the operating system, and how many of them have
/// been given out. Empty until the first IV.
struct Drawn {
    buffer: Zeroizing<Vec<u8>>,
    used: usize,
}

impl Default for Ivs {
    fn default() -> Self {
        Ivs::new()
    }
}

impl fmt::Debug for Ivs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source = match self.source {
            Source::Os => "the operating system",
            #[cfg(any(test, feature = "insecure-test-writer"))]
            Source::Fixed(_) => "one IV, fixed",
            #[cfg(any(test, feature = "insecure-test-writer"))]
            Source::Derived => "derived",
        };
        f.debug_struct("Ivs")
            .field("source", &source)
            .finish_non_exhaustive()
    }
}

impl Ivs {
    /// A write's IVs, from the operating system. It draws nothing until an
    /// IV is asked for.
    #[must_use]
    pub fn new() -> Ivs {
        Ivs::of(Source::Os)
    }

    fn of(source: Source) -> Ivs {
        Ivs {
            source,
            drawn: Mutex::new(Drawn {
                buffer: Zeroizing::new(Vec::new()),
                used: 0,
            }),
        }
    }

    /// Every IV `iv`: for a test that pins a file's bytes, never for a
    /// document's protection, as two plaintexts under one IV and key show
    /// where they're equal.
    #[cfg(any(test, feature = "insecure-test-writer"))]
    #[doc(hidden)]
    #[must_use]
    pub fn fixed(iv: [u8; 16]) -> Ivs {
        Ivs::of(Source::Fixed(iv))
    }

    /// Each IV the first 16 bytes of HMAC-SHA256, under the file's key, of
    /// the length (8 bytes, big-endian) of `n g obj`, the object as it's
    /// written, then `n g obj` and the data: pdf-crypt 0.1.0-alpha.1's IVs,
    /// for tests that write the same object to the same bytes on every save.
    /// Never for a document's protection: an object unchanged across saves
    /// shows as unchanged, and the file's key keys the HMAC too.
    #[cfg(any(test, feature = "insecure-test-writer"))]
    #[doc(hidden)]
    #[must_use]
    pub fn derived() -> Ivs {
        Ivs::of(Source::Derived)
    }

    /// The IV `data` of object `id` is encrypted under by `handler`: the next
    /// 16 bytes drawn; or [`Fault::Random`], nothing given, where the
    /// operating system's generator fails. Only the test writer's derived
    /// IVs read `handler`, `id` and `data`.
    #[cfg_attr(
        not(any(test, feature = "insecure-test-writer")),
        allow(unused_variables)
    )]
    pub(crate) fn next(
        &self,
        handler: &Handler,
        id: (u32, u16),
        data: &[u8],
    ) -> Result<[u8; 16], Fault> {
        match self.source {
            Source::Os => self.drawn().map_err(Fault::Random),
            #[cfg(any(test, feature = "insecure-test-writer"))]
            Source::Fixed(iv) => Ok(iv),
            #[cfg(any(test, feature = "insecure-test-writer"))]
            Source::Derived => Ok(derived(handler, id, data)),
        }
    }

    /// The next 16 bytes of the buffer, drawn anew where it's used up. A
    /// lock another thread poisoned is taken as it is: the buffer is whole
    /// either way, as nothing of it is given out before it's drawn. A draw
    /// that fails gives nothing out of the buffer, and the next IV draws
    /// again.
    fn drawn(&self) -> Result<[u8; 16], Random> {
        let mut drawn = self.drawn.lock().unwrap_or_else(PoisonError::into_inner);
        let Drawn { buffer, used } = &mut *drawn;
        if *used >= buffer.len() {
            buffer.resize(DRAWN, 0);
            *used = buffer.len();
            crate::random::fill(buffer)?;
            *used = 0;
        }
        let iv = buffer[*used..*used + 16].try_into().expect("16 bytes");
        *used += 16;
        Ok(iv)
    }
}

/// [`Ivs::derived`]'s IV for `data` of object `id` under `handler`'s key.
#[cfg(any(test, feature = "insecure-test-writer"))]
fn derived(handler: &Handler, (n, g): (u32, u16), data: &[u8]) -> [u8; 16] {
    let context = format!("{n} {g} obj");
    let len = (context.len() as u64).to_be_bytes();
    let mac = crate::hmac::HmacSha256::new(handler.key()).mac(&[&len, context.as_bytes(), data]);
    mac[..16].try_into().expect("16 bytes")
}
