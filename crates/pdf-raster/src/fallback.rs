//! Thread-safe cross-page font cache ([`RenderCache`]) and caller-supplied
//! substitute font hook ([`FontFallback`]).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use pdf_fonts::{FallbackProgram, Font, FontStyle, Fonts, Limits as FontLimits, Std14};

/// Style and family descriptor passed to [`FontFallback::fallback_font`] when a
/// PDF font dictionary omits an embedded font program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct FontQuery {
    /// Matched Standard 14 font, if any.
    pub std14: Option<Std14>,
    /// Style flags extracted from `/Flags`, `/FontWeight`, and `/BaseFont`.
    pub style: FontStyle,
}

impl FontQuery {
    /// Builds a [`FontQuery`] from a parsed [`Font`].
    #[must_use]
    pub const fn from_font(font: &Font) -> Self {
        Self {
            std14: font.std14(),
            style: font.style(),
        }
    }
}

/// Zero-I/O callback trait supplying substitute TrueType, OpenType/CFF, CFF, or
/// Type 1 font bytes when a PDF references an unembedded font.
///
/// Implementations are `Send + Sync` so multi-threaded page/tile workers can
/// share a single font provider (such as bundled Liberation / Noto faces or
/// `fontdb` system fonts).
pub trait FontFallback: Send + Sync {
    /// Returns raw SFNT/CFF/Type 1 font bytes for `query`, or `None` to fall
    /// back to `pdf-raster`'s built-in vector glyphs.
    fn fallback_font(&self, query: &FontQuery) -> Option<Arc<[u8]>>;
}

impl<F> FontFallback for F
where
    F: Fn(&FontQuery) -> Option<Arc<[u8]>> + Send + Sync,
{
    fn fallback_font(&self, query: &FontQuery) -> Option<Arc<[u8]>> {
        self(query)
    }
}

/// Cross-page thread-safe cache of parsed PDF [`Fonts`] and opened
/// [`FallbackProgram`]s.
///
/// Sharing a single [`RenderCache`] across `render_page` calls on the same
/// document avoids re-parsing embedded font streams, CMap tables, and substitute
/// font programs on every page or tile.
#[derive(Default)]
pub struct RenderCache {
    fonts: Fonts,
    fallbacks: Mutex<HashMap<FontQuery, Option<Arc<FallbackProgram>>>>,
}

impl fmt::Debug for RenderCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self
            .fallbacks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len();
        f.debug_struct("RenderCache")
            .field("cached_fallbacks", &count)
            .finish_non_exhaustive()
    }
}

impl RenderCache {
    /// Creates an empty [`RenderCache`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Underlying `pdf-fonts` [`Fonts`] cache.
    #[must_use]
    pub const fn fonts(&self) -> &Fonts {
        &self.fonts
    }

    /// Resolves and caches a [`FallbackProgram`] from `provider` for `query`.
    pub(crate) fn fallback_program(
        &self,
        provider: &dyn FontFallback,
        query: FontQuery,
        limits: &FontLimits,
    ) -> Option<Arc<FallbackProgram>> {
        let lock = || {
            self.fallbacks
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
        };
        if let Some(cached) = lock().get(&query) {
            return cached.clone();
        }
        let opened = provider
            .fallback_font(&query)
            .and_then(|bytes| FallbackProgram::open(bytes.to_vec(), limits))
            .map(Arc::new);
        lock().insert(query, opened.clone());
        opened
    }
}
