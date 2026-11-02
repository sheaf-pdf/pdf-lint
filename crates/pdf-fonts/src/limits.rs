//! The caps a font is read under: one value for all of them.

/// The caps on reading a font: past one, what's being read isn't read. A caller
/// sets them on a copy of [`Limits::DEFAULT`] (`limits.proc_depth = 8`, say), as
/// more may come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
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
}

impl Limits {
    /// The caps a font is read under unless it's given others.
    pub const DEFAULT: Limits = Limits {
        proc_depth: 16,
        ints_ahead: 256,
        type1_work: 64,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}
