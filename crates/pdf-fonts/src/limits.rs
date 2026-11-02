//! The caps a font is read under: one value for all of them.

/// The caps on reading a font: past one, what's being read isn't read. A caller
/// sets them on a copy of [`Limits::DEFAULT`] (`limits.proc_depth = 8`, say), as
/// more may come.
///
/// What a font's streams decompress to isn't capped here: that's the budget
/// [`Font::read`](crate::Font::read) is given, the caller's, shared with what else
/// it reads. What a font holds beyond its streams is bounded by what it reads:
/// its `/W` by the array's length, its encoding at 256 strings, a CMap's mappings
/// by the CMap's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// The caps on lexing a CMap or a CharProc, as content.
    pub content: pdf_content_streams::Limits,
    /// How long a CMap's chain by `/UseCMap` may be, past which what's read of it
    /// is guessed. 512 by default.
    pub cmaps: usize,
    /// How deep a Type 1 program's procedures may nest, past which
    /// [`type1::scan`](crate::type1::scan) refuses it. 16 by default: Adobe's own
    /// `/OtherSubrs` nest a few levels. Each level is a few calls on the stack, so
    /// a caller that sets it far higher gives them room.
    pub proc_depth: usize,
    /// How many integers in a row a Type 1 program may have, as a reader that
    /// looks past each integer to the token after it, which for another integer
    /// looks past that in turn, reads each by a further call on the stack. 256 by
    /// default: real programs have a dozen or so (`/BlueValues`).
    pub ints_ahead: usize,
    /// What reading a Type 1 program may cost, in bytes read as tokens: this many
    /// times its size, or times 16 KiB if it's smaller. 64 by default.
    pub type1_work: usize,
    /// How much charstring a program's glyphs may evaluate, in bytes, shared out
    /// between them: each glyph its share, but no less than
    /// [`glyph_fuel`](Limits::glyph_fuel). 2^24 by default. A real glyph evaluates a
    /// few hundred.
    pub font_fuel: usize,
    /// The least charstring a glyph may evaluate, in bytes. 2^14 by default.
    pub glyph_fuel: usize,
    /// The most glyphs a TrueType glyph's composites may draw, itself counted. 2^12
    /// by default: a real composite draws a few.
    pub composite_glyphs: u64,
    /// How deep a TrueType glyph's composites may nest. 32 by default.
    pub composite_depth: u8,
}

impl Limits {
    /// The caps a font is read under unless it's given others.
    pub const DEFAULT: Limits = Limits {
        content: pdf_content_streams::Limits::DEFAULT,
        cmaps: 512,
        proc_depth: 16,
        ints_ahead: 256,
        type1_work: 64,
        font_fuel: 1 << 24,
        glyph_fuel: 1 << 14,
        composite_glyphs: 1 << 12,
        composite_depth: 32,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}
