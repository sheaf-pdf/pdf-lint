//! What a reading reads under: one value for every cap, and the options a
//! caller opens a file with.

use super::{BUDGET, SECTIONS, STREAM};

/// The caps a reading of a file reads under, so that what a file says
/// can't make the reader read, or hold in memory, without end. A caller
/// sets them on a copy of [`Limits::DEFAULT`] (`limits.stream = 1 << 20`,
/// say), as it does [`Options`]'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// pdf-objects' parser's, on each object: values nested at most
    /// [`NESTING`](super::NESTING) deep, and at most 2^25 of them in one.
    pub parser: pdf_objects::parse::Limits,
    /// How many cross-reference sections the `/Prev` chain may have:
    /// [`SECTIONS`], 1024, by default.
    pub sections: usize,
    /// How many bytes a stream the reader decodes may decode to; more is
    /// cut off there. [`STREAM`], 256 MiB, by default.
    pub stream: usize,
    /// How many bytes the streams a reading decodes, and keeps, may decode
    /// to in all; more is cut off. [`BUDGET`], 1 GiB, by default.
    pub budget: usize,
    /// How many cross-reference entries the reader reads; none for as many
    /// as the file has bytes, or 2²⁰ if that's more, the default. Objects
    /// can be numbered sparsely, so not their numbers but their count.
    pub entries: Option<usize>,
}

impl Limits {
    /// The limits a reading reads under unless it's given others.
    pub const DEFAULT: Limits = Limits {
        parser: pdf_objects::parse::Limits::DEFAULT,
        sections: SECTIONS,
        stream: STREAM,
        budget: BUDGET,
        entries: None,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}

/// How a file is opened: under which [`Limits`], and as of which
/// cross-reference section. A caller sets them on a copy of
/// [`Options::DEFAULT`] (`options.section = Some(previous)`, say).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Options {
    /// The caps it reads under.
    pub limits: Limits,
    /// The file as of the cross-reference section at this byte: read from
    /// there along its `/Prev` chain, as if it were the newest, so that
    /// what later updates wrote isn't in it, as [`read_from`](super::read_from)
    /// reads it. None, the default, for the file as its `startxref` gives
    /// it.
    pub section: Option<usize>,
}

impl Options {
    /// The options a file is opened with unless it's given others: the
    /// default limits, and the file as its `startxref` gives it.
    pub const DEFAULT: Options = Options {
        limits: Limits::DEFAULT,
        section: None,
    };
}

impl Default for Options {
    fn default() -> Options {
        Options::DEFAULT
    }
}
