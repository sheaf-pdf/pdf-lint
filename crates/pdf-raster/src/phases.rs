//! Optional `tracing` span instrumentation (numeric dimensions and counts only;
//! never pixel bytes, stream payloads, or file plaintext).

macro_rules! phase {
    ($level:ident, $name:literal $(, $key:ident = $val:expr)* $(,)?) => {
        #[cfg(feature = "tracing")]
        let _span = ::tracing::span!(::tracing::Level::$level, $name $(, $key = $val)*).entered();
    };
}

pub(crate) use phase;
