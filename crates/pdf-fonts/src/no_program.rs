//! What stands for the `programs` feature's reading of a font's program, without it:
//! nothing can be opened, so nothing here is reached.

use crate::font::{OutlineOutcome, OutlineVerb, Simple, SimpleMap};
use crate::type1::Refusal;
use crate::{GlyphBox, Limits};

/// A font's program, open: there's never one.
#[allow(dead_code)]
pub(crate) struct Face(());

impl Face {
    /// Refused, whatever it's given.
    pub(crate) fn open(_: Vec<u8>, _: &[u8], _: &Limits) -> Result<Face, Refusal> {
        Err(Refusal::Malformed)
    }

    pub(crate) fn upem(&self) -> u16 {
        1000
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

    pub(crate) fn outline_char(
        &self,
        _: char,
        _: Option<f32>,
        _: &mut Vec<OutlineVerb>,
    ) -> Option<f32> {
        None
    }
}

/// A simple font's program, open: there's never one.
#[derive(Debug)]
pub(crate) struct SimpleGlyphs(());

impl SimpleGlyphs {
    pub(crate) fn new(_: Face, _: Box<[Option<u16>; 256]>) -> SimpleGlyphs {
        SimpleGlyphs(())
    }

    pub(crate) fn outline_status(
        &self,
        _: u32,
        _: Option<f32>,
        _: &mut Vec<OutlineVerb>,
    ) -> OutlineOutcome {
        OutlineOutcome::NoProgram
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

    pub(crate) fn char_box(&self, _: u32) -> Option<GlyphBox> {
        None
    }

    pub(crate) fn drawn(&self, _: u32) -> (u32, Option<GlyphBox>, usize) {
        (0, None, 0)
    }

    pub(crate) fn outline_status(
        &self,
        _: u32,
        _: Option<f32>,
        _: &mut Vec<OutlineVerb>,
    ) -> OutlineOutcome {
        OutlineOutcome::NoProgram
    }
}
