//! The operating system's random number generator, through getrandom: what
//! AES's IVs ([`Ivs`](crate::Ivs)) and a new file's key and salts
//! ([`Handler::create`](crate::Handler::create)) are drawn from.

use crate::Random;

/// `buf` filled from the operating system's random number generator; or
/// [`Random`], where it fails, `buf` then holding nothing to use.
pub(crate) fn fill(buf: &mut [u8]) -> Result<(), Random> {
    #[cfg(test)]
    {
        DRAWS.with(|draws| draws.set(draws.get() + 1));
        if FAILING.with(std::cell::Cell::get) {
            return Err(Random(getrandom::Error::UNEXPECTED));
        }
    }
    getrandom::fill(buf).map_err(Random)
}

#[cfg(test)]
thread_local! {
    /// How many times this thread has drawn from the operating system: the
    /// tests' count of what a write or a new file draws, a thread's own, as
    /// the tests run on threads that each draw.
    pub(crate) static DRAWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Whether this thread's draws fail, as the operating system's
    /// generator can: the tests' way to [`Random`], a thread's own.
    pub(crate) static FAILING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
