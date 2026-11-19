//! Optional `tracing` span helper matching sibling crates: numeric counts and
//! byte lengths only, never file content, field names, or field values.

#[cfg(feature = "tracing")]
macro_rules! phase {
    ($level:ident, $name:literal $(, $field:ident = $value:expr)* $(,)?) => {
        let _span = ::tracing::span!(::tracing::Level::$level, $name $(, $field = $value)*);
        let _entered = _span.enter();
    };
}

#[cfg(not(feature = "tracing"))]
macro_rules! phase {
    ($level:ident, $name:literal $(, $field:ident = $value:expr)* $(,)?) => {
        $(let _ = &$value;)*
    };
}

pub(crate) use phase;
