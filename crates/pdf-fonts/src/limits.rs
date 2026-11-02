//! The caps a font is read under: one value for all of them.

/// The caps on reading a font: past one, what's being read isn't read. A caller
/// sets them on a copy of [`Limits::DEFAULT`] (`limits.glyph_fuel = 1 << 10`, say), as
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
    /// What reading a Type 1 program may cost, past which
    /// [`type1::scan`](crate::type1::scan) refuses it: this many times its size, or
    /// times 16 KiB if it's smaller, in bytes read as tokens, and in names of its
    /// `/Encoding` looked for among its glyphs, each glyph a name. 64 by default: a
    /// real program's 256 names at most, over glyphs of at least 16 bytes each, cost
    /// about 16 times its size.
    pub type1_work: usize,
    /// How much charstring a program's glyphs may evaluate, in bytes, shared out
    /// between them: each glyph its share, but no less than
    /// [`glyph_fuel`](Limits::glyph_fuel). 2^24 by default. A real glyph evaluates a
    /// few hundred. read-fonts' evaluator has a bound of its own, which no fuel raises:
    /// a glyph past a million operations, each number and operator one, is unknown.
    pub font_fuel: usize,
    /// The least charstring a glyph may evaluate, in bytes. 2^14 by default. Setting it
    /// higher doesn't raise read-fonts' million operations ([`font_fuel`](Limits::font_fuel)).
    pub glyph_fuel: usize,
    /// The most glyphs a TrueType glyph's composites may draw, itself and empty ones
    /// counted. 2,049 by default, itself and 2,048 components: skrifa draws no more
    /// components than that, empty ones aside, whatever's set here, so raising it draws
    /// no more of them. A real composite draws a few.
    pub composite_glyphs: u64,
    /// How deep a TrueType glyph's composites may nest. 32 by default.
    pub composite_depth: u8,
}

impl Limits {
    /// The caps a font is read under unless it's given others.
    pub const DEFAULT: Limits = Limits {
        content: pdf_content_streams::Limits::DEFAULT,
        cmaps: 512,
        type1_work: 64,
        font_fuel: 1 << 24,
        glyph_fuel: 1 << 14,
        composite_glyphs: 2049,
        composite_depth: 32,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}
