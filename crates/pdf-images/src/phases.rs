//! Optional numeric `tracing` span helper (`feature = "tracing"`).
//!
//! Spans record numeric dimensions, component counts, and byte lengths only—never pixel data,
//! stream payloads, or raw PDF strings.

macro_rules! phase {
    ($name:expr $(, $($field:tt)*)?) => {
        #[cfg(feature = "tracing")]
        let _span = tracing::debug_span!($name $(, $($field)*)?).entered();
    };
}

pub(crate) use phase;
