//! Color spaces, read through pdf-color (ISO 32000-2, 8.6): each the walk
//! reached in the newest revision as one of the model's color space tables,
//! in the order it reached them, read again by [`Space::read`] under
//! pdf-color's caps, from its number where it has one, so that what's told
//! of it is in its object, else as it's written. What the read tells is
//! kept ([`Kept`]); none of it is found yet.

use std::mem;

use pdf_color::{Fault, Limits, Space};

use super::kept::Kept;
use super::{Objects, Walk};
use crate::object::Object;

/// The model's color space tables: what the walk reaches as one is a color
/// space, read again after it.
pub(super) const SPACES: [&str; 11] = [
    "CalGrayColorSpace",
    "CalRGBColorSpace",
    "LabColorSpace",
    "ICCBasedColorSpace",
    "IndexedColorSpace",
    "SeparationColorSpace",
    "DeviceNColorSpace",
    "PatternColorSpace",
    "DeviceGrayColorSpace",
    "DeviceRGBColorSpace",
    "DeviceCMYKColorSpace",
];

/// What color spaces are read under: pdf-color's caps, which ISO 32000-2
/// doesn't set.
const LIMITS: Limits = Limits::DEFAULT;

impl Walk<'_> {
    /// Reads each color space the walk reached, in the order it reached
    /// them: see the module's documentation.
    pub(super) fn spaces(&mut self) {
        let spaces = mem::take(&mut self.spaces);
        for space in &spaces {
            // Given by its number, the read starts in the space's object.
            let reference;
            let value = match space.id {
                Some(id) => {
                    reference = Object::from(id);
                    &reference
                }
                None => space.object,
            };
            let mut kept: Kept<Fault> = Kept::default();
            let _ = Space::read(Objects(self.source), value, &LIMITS, &mut kept);
        }
    }
}

#[cfg(test)]
mod tests {
    //! What's read of the color spaces the walk reaches.

    use super::*;

    /// Every table the walk names a color space by is the model's, so a
    /// space reached as one is read.
    #[test]
    fn each_space_table_is_the_model_s() {
        for name in SPACES {
            assert!(crate::grammar().table(name).is_some(), "{name}");
        }
    }
}
