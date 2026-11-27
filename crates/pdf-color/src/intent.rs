//! PDF colorimetric rendering intent (`/Intent`, `ri`, `/ExtGState` `/RI`; ISO 32000-2, §8.6.5.8).

use std::fmt;
use std::sync::Arc;

/// Colorimetric rendering intent (`ri`, `/ExtGState` `/RI`, or Image `/Intent`; ISO 32000-2,
/// §8.6.5.8).
///
/// `Debug` prints standard variant names and redacts custom intent names to byte length only so
/// arbitrary file names are never written to logs.
#[derive(Clone, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum RenderingIntent {
    /// `/AbsoluteColorimetric`.
    AbsoluteColorimetric,
    /// `/RelativeColorimetric` (initial graphics-state default, ISO 32000-2 §8.4.1).
    #[default]
    RelativeColorimetric,
    /// `/Saturation`.
    Saturation,
    /// `/Perceptual`.
    Perceptual,
    /// Non-standard rendering intent name.
    Other(Arc<[u8]>),
}

impl RenderingIntent {
    /// Parses a rendering intent from its decoded PDF name bytes.
    #[must_use]
    pub fn from_name(name: &[u8]) -> Self {
        match name {
            b"AbsoluteColorimetric" => Self::AbsoluteColorimetric,
            b"RelativeColorimetric" => Self::RelativeColorimetric,
            b"Saturation" => Self::Saturation,
            b"Perceptual" => Self::Perceptual,
            other => Self::Other(Arc::from(other)),
        }
    }

    /// Canonical PDF name for the four standard intents, or `None` for
    /// [`RenderingIntent::Other`].
    #[must_use]
    pub const fn as_pdf_name(&self) -> Option<&'static str> {
        match self {
            Self::AbsoluteColorimetric => Some("AbsoluteColorimetric"),
            Self::RelativeColorimetric => Some("RelativeColorimetric"),
            Self::Saturation => Some("Saturation"),
            Self::Perceptual => Some("Perceptual"),
            Self::Other(_) => None,
        }
    }

    /// Raw decoded PDF name bytes for this rendering intent.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::AbsoluteColorimetric => b"AbsoluteColorimetric",
            Self::RelativeColorimetric => b"RelativeColorimetric",
            Self::Saturation => b"Saturation",
            Self::Perceptual => b"Perceptual",
            Self::Other(bytes) => bytes,
        }
    }
}

impl fmt::Debug for RenderingIntent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AbsoluteColorimetric => f.write_str("AbsoluteColorimetric"),
            Self::RelativeColorimetric => f.write_str("RelativeColorimetric"),
            Self::Saturation => f.write_str("Saturation"),
            Self::Perceptual => f.write_str("Perceptual"),
            Self::Other(name) => f
                .debug_struct("Other")
                .field("name_len", &name.len())
                .finish(),
        }
    }
}
