//! Annotation flags bitmask (`/F`, ISO 32000-2 §12.5.3 Table 167).

use std::fmt;
use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Not};

/// Bitmask of annotation flags stored in an annotation dictionary's `/F` entry
/// (ISO 32000-2 §12.5.3 Table 167).
///
/// Reserved/unknown bits are preserved in `bits()` so round-tripping `/F`
/// never silently drops unrecognized higher bits.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct AnnotFlags(u32);

impl AnnotFlags {
    /// No flags set (`0`).
    pub const NONE: Self = Self(0);
    /// Bit 1 (`1 << 0`): do not display if the annotation does not belong to a
    /// standard annotation type and no handler is available.
    pub const INVISIBLE: Self = Self(1 << 0);
    /// Bit 2 (`1 << 1`): do not display or print the annotation or allow user
    /// interaction.
    pub const HIDDEN: Self = Self(1 << 1);
    /// Bit 3 (`1 << 2`): print the annotation when the page is printed.
    pub const PRINT: Self = Self(1 << 2);
    /// Bit 4 (`1 << 3`): do not scale the annotation's appearance to match page
    /// magnification.
    pub const NO_ZOOM: Self = Self(1 << 3);
    /// Bit 5 (`1 << 4`): do not rotate the annotation's appearance to match
    /// page `/Rotate`.
    pub const NO_ROTATE: Self = Self(1 << 4);
    /// Bit 6 (`1 << 5`): do not display the annotation on screen or allow user
    /// interaction (may still print if [`Self::PRINT`] is set).
    pub const NO_VIEW: Self = Self(1 << 5);
    /// Bit 7 (`1 << 6`): do not allow user interaction with the annotation
    /// (ignored for `/Widget`, which uses field flags).
    pub const READ_ONLY: Self = Self(1 << 6);
    /// Bit 8 (`1 << 7`): do not allow the annotation to be deleted or its
    /// position/size properties to be modified by the user.
    pub const LOCKED: Self = Self(1 << 7);
    /// Bit 9 (`1 << 8`): invert [`Self::NO_VIEW`] while hovering or selecting.
    pub const TOGGLE_NO_VIEW: Self = Self(1 << 8);
    /// Bit 10 (`1 << 9`): do not allow the annotation's `/Contents` to be
    /// modified by the user.
    pub const LOCKED_CONTENTS: Self = Self(1 << 9);

    /// Constructs [`AnnotFlags`] from raw `/F` integer bits.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Constructs [`AnnotFlags`] from a signed 32/64-bit integer as read from
    /// PDF `/F` (`i64 as i32 as u32`, matching PDFium's `GetInteger`).
    #[must_use]
    pub const fn from_pdf_int(value: i64) -> Self {
        Self((value as i32) as u32)
    }

    /// Raw bitmask value.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns `true` if every bit in `other` is set in `self`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Returns `true` if no flags are set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns `true` when this annotation is visible on screen (ISO 32000-2
    /// §12.5.3; matches `FLAT_NORMALDISPLAY` in PDFium).
    ///
    /// `known_subtype` indicates whether the viewer/caller recognizes the
    /// annotation's `/Subtype` (when `false`, [`Self::INVISIBLE`] hides it).
    #[must_use]
    pub const fn displays_on_screen(self, known_subtype: bool) -> bool {
        if self.contains(Self::HIDDEN) || self.contains(Self::NO_VIEW) {
            return false;
        }
        if !known_subtype && self.contains(Self::INVISIBLE) {
            return false;
        }
        true
    }

    /// Returns `true` when this annotation should appear on printed output
    /// (ISO 32000-2 §12.5.3; matches `FLAT_PRINT` in PDFium).
    #[must_use]
    pub const fn prints(self, known_subtype: bool) -> bool {
        if !self.contains(Self::PRINT) || self.contains(Self::HIDDEN) {
            return false;
        }
        if !known_subtype && self.contains(Self::INVISIBLE) {
            return false;
        }
        true
    }
}

impl fmt::Debug for AnnotFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("AnnotFlags")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

impl BitOr for AnnotFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for AnnotFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for AnnotFlags {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for AnnotFlags {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl BitXor for AnnotFlags {
    type Output = Self;
    fn bitxor(self, rhs: Self) -> Self {
        Self(self.0 ^ rhs.0)
    }
}

impl BitXorAssign for AnnotFlags {
    fn bitxor_assign(&mut self, rhs: Self) {
        self.0 ^= rhs.0;
    }
}

impl Not for AnnotFlags {
    type Output = Self;
    fn not(self) -> Self {
        Self(!self.0)
    }
}
