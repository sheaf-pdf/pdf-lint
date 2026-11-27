//! Thread-safe cross-page font cache ([`RenderCache`]) and caller-supplied
//! substitute font hook ([`FontFallback`]).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use pdf_fonts::{FallbackProgram, Font, FontStyle, Fonts, Limits as FontLimits, Std14};

const MAX_CACHED_QUERIES: usize = 1_024;
const MAX_CACHED_PROGRAMS: usize = 64;

/// Family, script, and style descriptor passed to
/// [`FontFallback::fallback_font`] when a PDF font dictionary omits an embedded
/// font program.
///
/// `Debug` prints only [`Std14`], [`FontStyle`], and byte lengths/flags—never
/// raw `/BaseFont` names, `/Ordering` strings, or document text characters.
#[derive(Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct FontQuery {
    /// Matched Standard 14 font, if any.
    pub std14: Option<Std14>,
    /// Style flags extracted from `/Flags`, `/FontWeight`, and `/BaseFont`.
    pub style: FontStyle,
    base_font: Option<Box<[u8]>>,
    cid_ordering: Option<Box<[u8]>>,
    sample_char: Option<char>,
}

impl fmt::Debug for FontQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FontQuery")
            .field("std14", &self.std14)
            .field("style", &self.style)
            .field("base_font_len", &self.base_font.as_ref().map(|b| b.len()))
            .field("has_cid_ordering", &self.cid_ordering.is_some())
            .field("has_sample_char", &self.sample_char.is_some())
            .finish()
    }
}

impl FontQuery {
    /// Builds a [`FontQuery`] from a parsed [`Font`], including its raw
    /// `/BaseFont` name and CID `/Ordering` (if any).
    #[must_use]
    pub fn from_font(font: &Font) -> Self {
        Self {
            std14: font.std14(),
            style: font.style(),
            base_font: font.base_font().map(Box::from),
            cid_ordering: font.cid_ordering().map(Box::from),
            sample_char: None,
        }
    }

    /// Attaches a representative Unicode scalar (`sample_char`) being rendered,
    /// allowing CJK, Arabic, Hebrew, Cyrillic, or symbol fallback selectors to
    /// pick a face covering the target script.
    #[must_use]
    pub const fn with_sample_char(mut self, sample_char: Option<char>) -> Self {
        self.sample_char = sample_char;
        self
    }

    /// Raw `/BaseFont` PostScript name bytes from the PDF font dictionary, if
    /// present (e.g. `b"ABCDEF+ArialMT"` or `b"MS-Mincho"`).
    #[must_use]
    pub fn base_font(&self) -> Option<&[u8]> {
        self.base_font.as_deref()
    }

    /// `/BaseFont` name with any leading 6-uppercase-letter subset tag
    /// (`ABCDEF+`) stripped (e.g. `b"ArialMT"` for `b"ABCDEF+ArialMT"`).
    #[must_use]
    pub fn family_name(&self) -> Option<&[u8]> {
        let raw = self.base_font.as_deref()?;
        if raw.len() > 7 && raw[6] == b'+' && raw[..6].iter().all(u8::is_ascii_uppercase) {
            Some(&raw[7..])
        } else {
            Some(raw)
        }
    }

    /// `/CIDSystemInfo` `/Ordering` bytes (or predefined CMap ordering) on a
    /// Type 0 composite font (e.g. `b"Japan1"`, `b"GB1"`, `b"CNS1"`,
    /// `b"Korea1"`, or `b"Identity"`).
    #[must_use]
    pub fn cid_ordering(&self) -> Option<&[u8]> {
        self.cid_ordering.as_deref()
    }

    /// Representative Unicode character being rendered, if known.
    #[must_use]
    pub const fn sample_char(&self) -> Option<char> {
        self.sample_char
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

type ProviderQueryKey = (usize, FontQuery);
type BufferKey = (usize, usize);
type StoredProgram = Option<(Arc<[u8]>, Option<Arc<FallbackProgram>>)>;

#[derive(Default)]
struct FallbackMemo {
    by_query: HashMap<ProviderQueryKey, Option<Arc<FallbackProgram>>>,
    by_buffer: HashMap<BufferKey, StoredProgram>,
}

/// Cross-page thread-safe cache of parsed PDF [`Fonts`] (retaining embedded
/// simple-font programs via [`Fonts::for_rendering`]) and opened
/// [`FallbackProgram`]s.
///
/// Sharing a single [`RenderCache`] across `render_page` calls on the same
/// document avoids re-parsing embedded font streams, CMap tables, and substitute
/// font programs on every page or tile. When a [`FontFallback`] returns the same
/// `Arc<[u8]>` font buffer for multiple [`FontQuery`] keys, the font program is
/// parsed once without copying the slice per query.
pub struct RenderCache {
    fonts: Fonts,
    fallbacks: Mutex<FallbackMemo>,
}

impl Default for RenderCache {
    fn default() -> Self {
        Self {
            fonts: Fonts::for_rendering(),
            fallbacks: Mutex::new(FallbackMemo::default()),
        }
    }
}

impl fmt::Debug for RenderCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (queries, programs) = {
            let guard = self
                .fallbacks
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            (guard.by_query.len(), guard.by_buffer.len())
        };
        f.debug_struct("RenderCache")
            .field("cached_queries", &queries)
            .field("cached_programs", &programs)
            .finish_non_exhaustive()
    }
}

impl RenderCache {
    /// Creates an empty [`RenderCache`] configured with [`Fonts::for_rendering`].
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
        let prov_ptr = (provider as *const dyn FontFallback).cast::<()>() as usize;
        let key = (prov_ptr, query);
        {
            let guard = self
                .fallbacks
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(cached) = guard.by_query.get(&key) {
                return cached.clone();
            }
        }

        let raw_bytes = provider.fallback_font(&key.1);
        let opened = match raw_bytes {
            None => None,
            Some(bytes) => {
                let buf_key = (Arc::as_ptr(&bytes).cast::<u8>() as usize, bytes.len());
                {
                    let guard = self
                        .fallbacks
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    if let Some(Some((kept_arc, prog))) = guard.by_buffer.get(&buf_key)
                        && Arc::ptr_eq(kept_arc, &bytes)
                    {
                        let p = prog.clone();
                        drop(guard);
                        let mut guard = self
                            .fallbacks
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        if guard.by_query.len() >= MAX_CACHED_QUERIES {
                            guard.by_query.clear();
                        }
                        guard.by_query.insert(key, p.clone());
                        return p;
                    }
                }
                let parsed = FallbackProgram::from_slice(&bytes, limits).map(Arc::new);
                let mut guard = self
                    .fallbacks
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if guard.by_buffer.len() >= MAX_CACHED_PROGRAMS {
                    guard.by_buffer.clear();
                }
                guard
                    .by_buffer
                    .insert(buf_key, Some((bytes, parsed.clone())));
                if guard.by_query.len() >= MAX_CACHED_QUERIES {
                    guard.by_query.clear();
                }
                guard.by_query.insert(key, parsed.clone());
                return parsed;
            }
        };

        let mut guard = self
            .fallbacks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if guard.by_query.len() >= MAX_CACHED_QUERIES {
            guard.by_query.clear();
        }
        guard.by_query.insert(key, opened.clone());
        opened
    }
}
