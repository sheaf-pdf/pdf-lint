//! What a reading reads under: one value for every cap, and the options a
//! caller opens a file with.

use pdf_crypt::Password;
use pdf_filters::Backend;

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

/// How a file is opened: under which [`Limits`], as of which
/// cross-reference section, by which passwords where it's encrypted, which
/// Flate backend its streams inflate on, and whether its faults are looked
/// for. A caller sets them on a copy
/// of [`Options::DEFAULT`] (`options.section = Some(previous)`, or
/// `options.passwords = &given`, say).
///
/// Its `Debug` prints each password as pdf-crypt's `Password` does, without
/// its text; and two are never compared, as passwords aren't.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Options<'a> {
    /// The caps it reads under.
    pub limits: Limits,
    /// The file as of the cross-reference section at this byte: read from
    /// there along its `/Prev` chain, as if it were the newest, so that
    /// what later updates wrote isn't in it. For an update's
    /// [`Update::previous`](crate::Update::previous), that's the revision it
    /// updates. None, the default, for the file as its `startxref` gives
    /// it. A reading so is of the whole file's bytes, its length the whole
    /// file's: the file as a revision was written, its bytes up to its end
    /// alone, is [`Reading::prefix`](super::Reading::prefix)'s.
    pub section: Option<usize>,
    /// The backend that inflates the Flate streams it decodes: by default
    /// pdf-filters' `Backend::MinizOxide`, which every build of this crate
    /// has; `Backend::ZlibRs` with its `zlib-rs` feature. On clean data the
    /// two decode alike; on damaged data, how much decodes before a fault,
    /// and the place it says, can differ.
    pub flate: Backend,
    /// The passwords an encrypted file is opened by, in the empty
    /// password's place: each as typed, tried in each of its encodings
    /// (pdf-crypt's `Password::encodings`), as the owner's and then the
    /// user's, every password's that fit the file's revision first, in the
    /// order given, then every password's others (pdf-crypt's
    /// `Handler::open`, given them all). So a later password can open it
    /// before an earlier one that would open it only in an encoding that
    /// doesn't fit. None, the default, for the empty password. A wrong one
    /// leaves it [`Encryption::Locked`](crate::Encryption::Locked), or
    /// [`Encryption::Plain`](crate::Encryption::Plain) where its strings and
    /// streams aren't encrypted, as no password would: the empty one isn't
    /// tried after it. Which opened it is
    /// [`Encryption::Opened`](crate::Encryption::Opened)'s `by`, its index
    /// here. They're borrowed for the open alone: a reading keeps none.
    pub passwords: &'a [Password<'a>],
    /// Whether faults are looked for and kept, as they are by default
    /// ([`Source::faults`](crate::Source::faults)). False for a caller that
    /// wants none: then none is looked for, pdf-objects' parser asked for
    /// none, no fault made, and none kept, and a check the reader makes
    /// only to say a fault, such as reading the file's end back for its
    /// `%%EOF` or sorting an object stream's members for those that overlap,
    /// isn't made. What's read is the same either way: every object,
    /// record, trailer, section and decoded stream, what's decrypted and
    /// what's refused. But nothing says what's wrong with it: that the
    /// encryption dictionary isn't right (`/Perms`), or that a string
    /// didn't decrypt cleanly, among the rest.
    pub faults: bool,
}

impl Options<'static> {
    /// The options a file is opened with unless it's given others: the
    /// default limits, the file as its `startxref` gives it, miniz_oxide's
    /// Flate, no password but the empty one, and faults looked for.
    pub const DEFAULT: Options<'static> = Options {
        limits: Limits::DEFAULT,
        section: None,
        flate: Backend::MinizOxide,
        passwords: &[],
        faults: true,
    };
}

impl Default for Options<'_> {
    fn default() -> Self {
        Options::DEFAULT
    }
}
