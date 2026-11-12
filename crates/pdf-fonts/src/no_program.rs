//! What stands for the `programs` feature's reading of a font's program, without it:
//! nothing can be opened, so nothing here is reached.

use crate::font::{Simple, SimpleMap};
use crate::type1::Refusal;
use crate::{Limits, Rect};

/// A font's program, open: there's never one.
#[allow(dead_code)]
pub(crate) struct Face(());

impl Face {
    /// Refused, whatever it's given.
    pub(crate) fn open(_: Vec<u8>, _: &[u8], _: &Limits) -> Result<Face, Refusal> {
        Err(Refusal::Malformed)
    }

    pub(crate) fn heights(&self) -> Option<([f32; 4], f32, f32)> {
        None
    }

    pub(crate) fn exact(&self) -> bool {
        false
    }

    pub(crate) fn is_sfnt(&self) -> bool {
        false
    }

    pub(crate) fn simple_map(&self, _: &Simple<'_>) -> SimpleMap {
        SimpleMap::none()
    }
}

/// A CIDFont's program, open: there's never one.
#[derive(Debug)]
pub(crate) struct CidGlyphs(());

impl CidGlyphs {
    pub(crate) fn new(_: Face, _: Option<Vec<u8>>) -> CidGlyphs {
        CidGlyphs(())
    }

    pub(crate) fn face(&self) -> &Face {
        &Face(())
    }

    pub(crate) fn char_box(&self, _: u32) -> Option<Rect> {
        None
    }

    pub(crate) fn drawn(&self, _: u32) -> (u32, Option<Rect>, usize) {
        (0, None, 0)
    }
}
