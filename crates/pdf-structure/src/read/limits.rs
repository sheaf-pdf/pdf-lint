//! What a reading reads under: one value for every cap, and the options a
//! caller opens a file with.

use pdf_crypt::Password;
use pdf_filters::Backend;

use super::{BUDGET, SECTIONS, STREAM};

/// The caps a reading of a file reads under, so that what a file says
/// can't make the reader read, or hold in memory, without end. A caller
/// sets them on a copy of [`Limits::DEFAULT`] (`limits.stream = 1 << 20`,
/// say), as it does [`Options`]'.
/// Not exhaustive, as more caps may come.
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
/// Flate backend its streams inflate on, whether its faults are looked
/// for, and how it's read where its sections or objects aren't where it
/// says. A caller sets them on a copy
/// of [`Options::DEFAULT`] (`options.section = Some(previous)`, or
/// `options.passwords = &given`, say).
///
/// Its `Debug` prints each password as pdf-crypt's `Password` does, without
/// its text; and two are never compared, as passwords aren't.
/// Not exhaustive, as more choices may come.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Options<'a> {
    /// The caps it reads under.
    pub limits: Limits,
    /// The file as of the cross-reference section at this byte: read from
    /// there along its `/Prev` chain, as if it were the newest, so that
    /// what later updates wrote isn't in it, nor found by a scan for an
    /// object its sections put elsewhere ([`Recovery::Verified`]). For an
    /// update's [`Update::previous`](crate::Update::previous), that's the
    /// revision it updates. A position in the file's bytes, as every
    /// position a reading gives is, taken exactly: no header's offset is
    /// added to it, nor is a section looked for near it, as one that
    /// `startxref` or a `/Prev` gives is. How [`Recovery::Pdfium`] reads a
    /// file so, its doc says. None, the default, for the file as its
    /// `startxref` gives it. A reading so is of the whole file's bytes, its
    /// length the whole file's: the file as a revision was written, its
    /// bytes up to its end alone, is
    /// [`Reading::prefix`](super::Reading::prefix)'s.
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
    /// Whether faults are looked for and kept: [`Faults::Kept`] by default
    /// ([`Source::faults`](crate::Source::faults)), or [`Faults::Off`].
    pub faults: Faults,
    /// How a file is read where a section or an object isn't where the file
    /// says: [`Recovery::Verified`] by default, or as PDFium reads it
    /// ([`Recovery::Pdfium`]).
    pub recovery: Recovery,
}

/// How a reading reads a file whose cross-reference sections or objects
/// aren't where it says ([`Options::recovery`]). Whatever it takes, each
/// recovery is said: by a fault ([`Fault`](super::Fault)), or, where the
/// file is read by scanning, by why its trailer is the scan's
/// ([`Trailer::unwritten`](crate::Trailer::unwritten)); and an object read
/// elsewhere says so ([`Origin`](crate::Origin)). A reading with faults off
/// still says whether it took one
/// ([`Facts::recovered`](crate::Facts::recovered)).
/// Not exhaustive, as other readers' ways may come.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Recovery {
    /// Only where the file proves it, the default. A section that doesn't
    /// read where `startxref`, a `/Prev` or a `/XRefStm` puts it is read
    /// where that offset puts it counted from the file's first byte rather
    /// than its header, as written there, or at the nearest `xref` or
    /// cross-reference stream within 64 bytes of it, exactly there, where
    /// one reads ([`Fault::Moved`](super::Fault::Moved)). A section's
    /// entries are read at one shift: the section's own move; else one its
    /// first object in use or the one furthest into the file proves, the
    /// header's offset taken back or its `n g obj` found exactly within 64
    /// bytes; else the shift another section has, as a cut moves everything
    /// after it. Each entry is read where it's written, else at its own
    /// `n g obj` at that shift (exactly, or, the header's offset taken back,
    /// as written), else where a scan of the file finds its `n g obj`, of
    /// its number and generation, exactly
    /// ([`Origin::Recovered`](crate::Origin::Recovered)), else where it's
    /// written ([`Fault::Shifted`](super::Fault::Shifted)). A file whose
    /// newest section doesn't read is scanned for its objects. A later
    /// section that doesn't read is said, and what the sections before it
    /// gave is kept.
    #[default]
    Verified,
    /// As PDFium reads a file, as of chromium/8125: its sections where
    /// they're said, none looked for near, a `/Prev` or `/XRefStm` of 0
    /// taken for none, and the file scanned for its objects where
    /// `startxref` gives no section that reads, a section along the `/Prev`
    /// chain doesn't read or the chain comes back on itself, the first
    /// object in use its first-written table lists isn't where the table
    /// puts it, a later section's `/XRefStm` doesn't read, or, once the
    /// file is opened, its trailer's `/Root` gives no page (an encrypted
    /// file's, where a password opened it), each said by the trailer the
    /// scan makes ([`Trailer::unwritten`](crate::Trailer::unwritten)).
    /// The scan lists every `n g obj`, a later one of a number over an
    /// earlier but where that has a higher generation, and each object
    /// stream's members at its place in the file, where its data isn't
    /// encrypted; its trailer is each `trailer` dictionary and
    /// cross-reference stream's, a later one's keys over an earlier's; and
    /// it's merged over what the sections had given, each object the scan
    /// doesn't find kept where they put it. A file it doesn't scan reads
    /// as under [`Recovery::Verified`] where that takes no recovery. A file
    /// it scans is read as one whose sections don't read, whatever they
    /// gave: it has no revisions and no update
    /// ([`Reading::revisions`](super::Reading::revisions),
    /// [`Reading::update`](super::Reading::update)), and no base for an
    /// update after it ([`write::Base::of`](crate::write::Base::of)). Read as
    /// of a section ([`Options::section`]), it's read by the sections from
    /// there: where one given doesn't read, the file is scanned, but none is
    /// verified, a later one that doesn't read is said and what the newer gave
    /// kept, and no page is looked for.
    ///
    /// It parts from PDFium in what it parses: each object is read by this
    /// crate's reader, by ISO 32000-2's rules, so a damaged dictionary
    /// PDFium's parser reads past doesn't read here. And a file PDFium
    /// doesn't open, one without `%PDF-` or an encrypted one no password
    /// given opens, is read all the same, as far as it reads. A few edges
    /// of how PDFium finds and lists sections are read by this crate's
    /// rules instead, among them: a `startxref` more than 4,096 bytes from
    /// the file's end, where PDFium doesn't look, one under 9, and a
    /// cross-reference stream's negative `/Prev`, or a `/Size` negative or
    /// past PDFium's cap, each a file PDFium scans; a `/Prev` written as a
    /// real, which PDFium takes as a number; the first-written section's
    /// `/XRefStm`, which PDFium never reads; which section's entry is taken
    /// where two that read list one object; and an object stream written
    /// twice under one number, of which PDFium decodes both.
    Pdfium,
}

/// What a reading does with the faults it finds ([`Options::faults`]).
/// Not exhaustive, as other ways to take them may come.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Faults {
    /// Looked for and kept by the reading, the default: each read keeps
    /// what it tells, and the reading gives them in the whole read's order
    /// ([`Source::faults`](crate::Source::faults)).
    #[default]
    Kept,
    /// None looked for, for a caller that wants none: pdf-objects' parser
    /// asked for none, no fault made, and none kept, and a check the reader
    /// makes only to say a fault, such as reading the file's end back for
    /// its `%%EOF` or sorting an object stream's members for those that
    /// overlap, isn't made. What's read is the same either way: every
    /// object, record, trailer, section and decoded stream, what's
    /// decrypted and what's refused. But nothing says what's wrong with
    /// it: that the encryption dictionary isn't right (`/Perms`), or that a
    /// string didn't decrypt cleanly, among the rest.
    Off,
}

impl Options<'static> {
    /// The options a file is opened with unless it's given others: the
    /// default limits, the file as its `startxref` gives it, miniz_oxide's
    /// Flate, no password but the empty one, faults looked for, and the
    /// verified recovery.
    pub const DEFAULT: Options<'static> = Options {
        limits: Limits::DEFAULT,
        section: None,
        flate: Backend::MinizOxide,
        passwords: &[],
        faults: Faults::Kept,
        recovery: Recovery::Verified,
    };
}

impl Default for Options<'_> {
    fn default() -> Self {
        Options::DEFAULT
    }
}
