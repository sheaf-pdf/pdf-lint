//! What the reader tells of a file as it reads it ([`Fault`]), each with
//! its place, pdf-objects' [`Place`](pdf_objects::fault::Place): every
//! choice it had to make, every repair, and what's wrong that it read past;
//! and why a file's cross-reference sections don't read ([`Unwritten`]),
//! which a trailer the reader made of the file instead says.

use pdf_objects::object::ObjectId;
use pdf_objects::parse;

use super::Unfiltered;
use crate::source::{Unread, Unsupported};

/// Something the reader had to choose how to read, or read past: what a
/// reading tells pdf-objects' [`Sink`](pdf_objects::fault::Sink), and keeps,
/// each at its [`Place`](pdf_objects::fault::Place), the object it's in and
/// where it was written ([`Source::faults`](crate::Source::faults)). What's
/// read goes on past each, and what's read is the same whether faults are
/// looked for or not ([`Options::faults`](super::Options::faults)).
///
/// Its `Display` is the whole of what it says, in words; each variant's
/// fields are the whole of what those words say (an offset, a count, an
/// object, pdf-filters' or pdf-crypt's fault, a reason), so outside the
/// crate one is built by a literal, as a test compares with it. Not
/// exhaustive, as faults are found as the reader learns more of what files
/// hold; a variant's fields, though, are what it says, so none is itself.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Fault {
    /// The file's last `%%EOF` isn't on a line of its own, as ISO 32000-2
    /// 7.5.5 writes it.
    #[error("the file's last %%EOF isn't on a line of its own")]
    EofNotAlone,
    /// The file goes on for `bytes` bytes after its last `%%EOF`, within
    /// the 1024 readers look in for one.
    #[error("the file goes on for {bytes} bytes after its last %%EOF")]
    AfterEof {
        /// How many.
        bytes: usize,
    },
    /// The file's last `%%EOF` is `bytes` bytes from its end, past the
    /// 1024 readers look in for one.
    #[error("the file's last %%EOF is {bytes} bytes from its end, past the 1024 readers look in")]
    EofFar {
        /// How far, its five bytes among them.
        bytes: usize,
    },
    /// The file has no `%%EOF`.
    #[error("the file has no %%EOF")]
    NoEof,
    /// No `%PDF-` within the file's first 1024 bytes.
    #[error("no %PDF- header")]
    NoHeader,
    /// `%PDF-` starts at byte `at`, not the file's first: what's before it
    /// is junk some writers put first, and the file's offsets count from the
    /// header, past it (ISO 32000-2, 7.5.2).
    #[error("%PDF- is at byte {at}, not the file's first")]
    HeaderAt {
        /// Where it starts.
        at: usize,
    },
    /// The version after `%PDF-` doesn't read.
    #[error("the header's version doesn't read")]
    Version,
    /// The `/Prev` chain comes back to the section at byte `at`, read
    /// before: the sections are read no further.
    #[error("the /Prev chain comes back to byte {at}")]
    Loop {
        /// Where it comes back to.
        at: usize,
    },
    /// The `/Prev` chain has more sections than the reader reads,
    /// [`Limits::sections`](super::Limits::sections): those past it aren't
    /// read.
    #[error("more than {limit} cross-reference sections")]
    Sections {
        /// How many it reads.
        limit: usize,
    },
    /// An earlier section along `/Prev`, at byte `at`, doesn't read, and
    /// why: the sections are read no further.
    #[error("the section at byte {at} doesn't read: {why}")]
    Section {
        /// Where it is.
        at: usize,
        /// Why it doesn't read.
        why: Box<Unwritten>,
    },
    /// The rows of an earlier section's cross-reference stream, at byte
    /// `at`, don't read, and why: it lists nothing.
    #[error("the section, at byte {at}, doesn't read: {why}")]
    StreamRows {
        /// Where the stream is.
        at: usize,
        /// Why its rows don't read.
        why: Box<Unwritten>,
    },
    /// A hybrid file's `/XRefStm` (ISO 32000-2, 7.5.8.4), at byte `at`,
    /// doesn't read, or its rows don't, and why: its table's rows alone
    /// are read.
    #[error("its /XRefStm, at byte {at}, doesn't read: {why}")]
    XrefStm {
        /// Where the stream is.
        at: usize,
        /// Why it doesn't read.
        why: Box<Unwritten>,
    },
    /// The cross-reference section `startxref`, a `/Prev` or a `/XRefStm` puts
    /// at byte `at` doesn't read there, and is read at byte `found`
    /// ([`Recovery::Verified`](super::Recovery::Verified)): where that offset
    /// puts it counted from the file's first byte rather than its header
    /// (`from_start`, against ISO 32000-2, 7.5.2), exactly there or past
    /// white-space alone, or the nearest `xref` or cross-reference stream
    /// within 64 bytes that reads, exactly there. Of the entries it lists in
    /// use that aren't where it puts them, `moved` are read as far off, each
    /// at its own `n g obj` (exactly, or, the header's offset taken back,
    /// past white-space alone); `scanned`
    /// aren't there either, and are read where a scan of the file finds their
    /// `n g obj`, of their number and generation, exactly
    /// ([`Origin::Recovered`](crate::Origin::Recovered)); and `neither` are at
    /// none of those places, and are read where it puts them.
    #[error("{}", moved_words(*.at, *.found, *.from_start, *.moved, *.scanned, *.neither))]
    Moved {
        /// Where it's said to be.
        at: usize,
        /// Where it's read.
        found: usize,
        /// Whether that's where the offset puts it counted from the file's
        /// first byte.
        from_start: bool,
        /// How many of its entries are read as far off: object numbers,
        /// so a `u32` holds them.
        moved: u32,
        /// How many are read where a scan finds them.
        scanned: u32,
        /// How many are at none of those places.
        neither: u32,
    },
    /// `moved` of the entries the cross-reference section at byte `at` lists
    /// in use aren't where it puts them, and are read `by` bytes from there,
    /// each at its own `n g obj`
    /// ([`Recovery::Verified`](super::Recovery::Verified)): the header's
    /// offset taken back (`from_start`, the offsets counting from the file's
    /// first byte, against ISO 32000-2, 7.5.2), each there or past white-space
    /// alone, or, each exactly there, as far as its first object in use, or
    /// the one furthest into the file, is, or another section's are. `scanned`
    /// aren't there either, and are read where a scan of the file finds their
    /// `n g obj`, of their number and generation, exactly
    /// ([`Origin::Recovered`](crate::Origin::Recovered)); and `neither` are at
    /// none of those places, and are read where it puts them. Said where it
    /// reads any entry elsewhere, though a newer section lists each anew
    /// ([`Facts::recovered`](crate::Facts::recovered) says so too).
    #[error("{}", shifted_words(*.at, *.by, *.from_start, *.moved, *.scanned, *.neither))]
    Shifted {
        /// Where the section is.
        at: usize,
        /// How far each is from where the section puts it: before it where
        /// it's less than 0.
        by: i64,
        /// Whether that's the header's offset taken back.
        from_start: bool,
        /// How many are read so: object numbers, so a `u32` holds them.
        moved: u32,
        /// How many are read where a scan finds them.
        scanned: u32,
        /// How many are at none of those places.
        neither: u32,
    },
    /// The file was read by scanning
    /// ([`Trailer::unwritten`](crate::Trailer::unwritten)), and the object
    /// streams the scan found hold `objects` objects it didn't find at an
    /// offset, read from `streams` of them; and the objects of `unread` more
    /// don't read: a stream's data doesn't decode or its header doesn't say
    /// where its objects are, or the file is encrypted and no password opened
    /// it.
    #[error("{}", members_words(*.objects, *.streams, *.unread))]
    Members {
        /// How many objects are read from object streams: object numbers,
        /// so a `u32` holds them, as it does each recovery's counts.
        objects: u32,
        /// From how many.
        streams: u32,
        /// How many object streams' objects don't read.
        unread: u32,
    },
    /// The table at byte `at` numbers its first subsection from 1, but its
    /// first row is object 0's, free at generation 65535 (ISO 32000-2,
    /// 7.5.4), and each of its rows in use is exactly at its own `n g obj`
    /// as the number one before its row's, where it's written or at the
    /// section's shift ([`Fault::Moved`], [`Fault::Shifted`])
    /// ([`Recovery::Verified`](super::Recovery::Verified)): its `count` rows
    /// are read as numbered from 0, as pdf.js reads such a subsection, each
    /// object in use where its row puts it, or at that shift
    /// ([`Origin::Shifted`](crate::Origin::Shifted)). A file whose rows land
    /// so for some and not others is read as written.
    #[error("{}", renumbered_words(*.at, *.count))]
    Renumbered {
        /// Where the table is.
        at: usize,
        /// How many rows its first subsection has: object numbers, so a
        /// `u32` holds them.
        count: u32,
    },
    /// A scan of the file, as a file whose sections don't read is scanned
    /// ([`Trailer::unwritten`](crate::Trailer::unwritten)), or as one is for
    /// an object not where its section puts it
    /// ([`Origin::Recovered`](crate::Origin::Recovered)), found no
    /// `/Length` written as a number that ends at `endstream` for a
    /// stream's data, which starts at byte `start`, and no `endstream`
    /// after it before an `endobj`: it ended the data at that `endobj`, at
    /// byte `end`, and read on from there; or, neither coming after it,
    /// found no end (`end` none) and read on through the data from where it
    /// starts. Either way, as PDFium's scan does, under either
    /// [`Recovery`](super::Recovery), so that what's after the stream is
    /// found. `id` is the stream's object, its `n g obj` as the scan found
    /// it before `stream`; none where it found none, which
    /// [`Recovery::Pdfium`](super::Recovery::Pdfium)'s scan doesn't pass
    /// over, as PDFium's doesn't. Where its `/Length` doesn't end its data
    /// at `endstream` ([`Fault::Length`]), the object's own read takes its
    /// data to where the scan ended it, at that `endobj`, as PDFium's read
    /// of the object does, not to an `endstream` past it, which may be a
    /// later stream's; but where the data starts on the `stream` keyword's
    /// line, which the scan, as PDFium's, passes over to start it on the
    /// next, to an `endstream` on that line; and where no end was found,
    /// as any stream's is read. Under
    /// [`Recovery::Verified`](super::Recovery::Verified), it isn't said
    /// where the scan, reading on, meets that `endstream` on its own
    /// between objects: the data ran there, what the scan found inside is
    /// taken back, and the object's read takes its data to there.
    #[error("{}", stream_end_words(*.id, *.start, *.end))]
    StreamEnd {
        /// The stream's object.
        id: Option<ObjectId>,
        /// Where its data starts.
        start: usize,
        /// Where it was ended, at its `endobj`; none where no end was found.
        end: Option<usize>,
    },
    /// A scan of the file, as a file whose sections don't read is scanned
    /// ([`Trailer::unwritten`](crate::Trailer::unwritten)), or as one is for
    /// an object not where its section puts it
    /// ([`Origin::Recovered`](crate::Origin::Recovered)), found no `)`
    /// closing a literal string whose `(` is at byte `start` (ISO 32000-2,
    /// 7.3.4.2), which would take every byte after it. Under
    /// [`Recovery::Verified`](super::Recovery::Verified) it reads on from
    /// just past that `(`, the bytes the string held read as tokens, so that
    /// what's after it is found; `past` is how many such strings it read
    /// past, the first at `start`, each read on from just past its `(`.
    /// Under [`Recovery::Pdfium`](super::Recovery::Pdfium) it reads the
    /// rest of the file as that string, as PDFium's scan does where it
    /// reads a string, and finds nothing after it; `past` is 0. One a scan,
    /// however many such strings it meets. `id` is the object the scan was
    /// in, its `n g obj` found and neither its `stream` nor its `endobj`
    /// yet; none where it was in none, between objects or in a stream's
    /// data it read on through. The object's own read of such a string
    /// doesn't read ([`Unread`](crate::Unread)).
    #[error("{}", unclosed_words(*.id, *.start, *.past))]
    Unclosed {
        /// The object the first string is in.
        id: Option<ObjectId>,
        /// Where the first string's `(` is.
        start: usize,
        /// How many strings the scan read past, each from just past its
        /// `(`; 0 where it read the first to the file's end. A `u32`, as
        /// every count a recovery says.
        past: u32,
    },
    /// A scan of the file, as a file whose sections don't read is scanned
    /// ([`Trailer::unwritten`](crate::Trailer::unwritten)), or as one is for
    /// an object not where its section puts it
    /// ([`Origin::Recovered`](crate::Origin::Recovered)), found `id`'s
    /// number again at byte `later`, past a literal string that doesn't
    /// close, which it read past ([`Fault::Unclosed`]), and that object
    /// doesn't read there: a revision cut short, say, or bytes the string
    /// held. `id` is read at byte `at` instead, where the scan found it
    /// before: of its copies past the string, the last that reads; else
    /// the last before the string, as a scan that read no further reads
    /// it; else, where there's none, the first. A later `n g obj` of a
    /// number stands over an earlier one, as an update appends, but one
    /// past such a string only where it reads, so what's read past one
    /// takes nothing out of the reading.
    /// [`Recovery::Verified`](super::Recovery::Verified) alone: under
    /// [`Recovery::Pdfium`](super::Recovery::Pdfium) the scan reads no
    /// further than such a string.
    #[error("{}", earlier_words(*.id, *.at, *.later))]
    Earlier {
        /// The object read.
        id: ObjectId,
        /// Where its `n g obj` is.
        at: usize,
        /// Where the scan found its number again, past such a string,
        /// where that doesn't read.
        later: usize,
    },
    /// A scan of the file, as a file whose sections don't read is scanned
    /// ([`Trailer::unwritten`](crate::Trailer::unwritten)), or as one is for
    /// an object not where its section puts it
    /// ([`Origin::Recovered`](crate::Origin::Recovered)), read a literal
    /// string from its `(` at byte `start` to a `)` at byte `end`, and
    /// reads `objects` objects whose `n g obj` starts a line inside it
    /// there, of numbers it found nowhere else. A string that lacks its
    /// own `)` closes late, at a later string's or one in a stream's data,
    /// and what's written up to there is read as its bytes. Each line's
    /// first three tokens are read as qpdf's reconstruction reads a line's,
    /// so that an object written there is found. None stands over an
    /// object a scan found as written: not one of a number it found
    /// elsewhere, nor a member of an object stream it found so, which
    /// stands over one, and such a stream's members stand over those of
    /// one found inside a string. Of a number written so more than once,
    /// inside one string or two, the last stands, as a later `n g obj`
    /// does. `objects` counts those that stand, each in the string it's
    /// in, and a string none of whose objects stands isn't said. A
    /// `trailer` there isn't taken. One a string. `id` is the object the
    /// scan was in, as [`Fault::Unclosed`] has it.
    /// [`Recovery::Verified`](super::Recovery::Verified) alone, and only
    /// its first scan, which passes over each stream's data as PDFium's
    /// does.
    #[error("{}", in_string_words(*.id, *.start, *.end, *.objects))]
    InString {
        /// The object the string is in.
        id: Option<ObjectId>,
        /// Where its `(` is.
        start: usize,
        /// Where the `)` that closes it is.
        end: usize,
        /// How many objects it read there. A `u32`, as every count a
        /// recovery says.
        objects: u32,
    },
    /// The sections list `past` objects past the first `most`, as many
    /// entries as the reader reads of a file of `len` bytes
    /// ([`Limits::entries`](super::Limits::entries)): those aren't read.
    #[error(
        "the cross-reference sections list {past} objects past the first {most}, as many as the reader reads of a file of {len} bytes, which aren't read"
    )]
    Entries {
        /// How many aren't read.
        past: usize,
        /// How many are.
        most: usize,
        /// The file's length.
        len: usize,
    },
    /// A table's row for object `number` gives an offset or a generation
    /// out of range, and is read as free: the rest of the table still
    /// reads.
    #[error("the table's row for object {number} gives a number out of range, and is read as free")]
    FreeRow {
        /// The object's number.
        number: u32,
    },
    /// `misshapen` of a table's `rows` rows aren't the 20 bytes ISO
    /// 32000-2 7.5.4 writes one in, the first object `number`'s, and why:
    /// they're read all the same.
    #[error(
        "{misshapen} of the table's {rows} rows aren't 20 bytes as ISO 32000-2 7.5.4 writes them, the first object {number}'s: {why}"
    )]
    Rows {
        /// How many aren't.
        misshapen: usize,
        /// How many rows the table has.
        rows: usize,
        /// The first misshapen row's object.
        number: u32,
        /// What's wrong with that row.
        why: RowShape,
    },
    /// A cross-reference stream's rows list `past` entries past the first
    /// `most`, as many as the reader reads of a file of `len` bytes
    /// ([`Limits::entries`](super::Limits::entries)): those aren't read.
    #[error(
        "its rows list {past} entries past the first {most}, as many as the reader reads of a file of {len} bytes, which aren't read"
    )]
    StreamEntries {
        /// How many aren't read.
        past: usize,
        /// How many are.
        most: usize,
        /// The file's length.
        len: usize,
    },
    /// A section's cross-reference stream `id` doesn't list itself, at the
    /// offset it's at (ISO 32000-2, 7.5.8.2).
    #[error("cross-reference stream {id} R doesn't list itself")]
    Unlisted {
        /// The stream.
        id: ObjectId,
    },
    /// The cross-references list object 0 in use, which is always free
    /// (ISO 32000-2, 7.5.4).
    #[error("the cross-references list object 0, which is always free (ISO 32000-2, 7.5.4)")]
    ObjectZero,
    /// The last trailer of a file found by scanning lacks `keys`, which it's
    /// given from an earlier one: a linearized file's last trailer may have
    /// only `/Size`, its first page's the rest. Where no `trailer`
    /// dictionary reads, the trailers are the cross-reference streams'
    /// dictionaries. Each key as written, without its `/`, in the order
    /// taken: of `/Root`, `/Encrypt`, `/ID` and `/Info`, the newest earlier
    /// trailer's first.
    #[error(
        "the last trailer lacks {}, which it's given from an earlier one",
        names(.keys)
    )]
    Taken {
        /// The keys taken.
        keys: Vec<Vec<u8>>,
    },
    /// The trailer's `/Encrypt` is `id`, a reference to no object (ISO
    /// 32000-2, 7.3.10), so the file is read as not encrypted.
    #[error(
        "the trailer's /Encrypt is {id} R, which is no object, so the file is read as not encrypted"
    )]
    NoEncrypt {
        /// What it refers to.
        id: ObjectId,
    },
    /// The file's security handler doesn't open, and why
    /// ([`Encryption::Unsupported`](crate::Encryption::Unsupported)): its
    /// strings stay ciphertext, where they're encrypted (`strings`), and its
    /// object streams aren't read.
    #[error(
        "its security handler doesn't open: {why}; {}its object streams aren't read",
        if *.strings { "its strings stay ciphertext, and " } else { "" }
    )]
    Unopened {
        /// Why.
        why: Unsupported,
        /// Whether its strings are encrypted.
        strings: bool,
    },
    /// The trailer's `/ID` has no string first, so the file's key is made
    /// without one, as qpdf makes it.
    #[error("its trailer's /ID has no string first, so its key is made without one")]
    NoId,
    /// What's wrong with the encryption dictionary, though the file opened:
    /// its `/Perms`, as a file of revision 5 or 6 can have it.
    #[error("its encryption dictionary isn't right: {fault}")]
    Dictionary {
        /// pdf-crypt's fault.
        fault: pdf_crypt::Fault,
    },
    /// What pdf-objects' parser read past in an object, of kind `kind`, in
    /// its words: the value is read, as readers read it. The fault's
    /// bytes are where its place is.
    #[error("{text}")]
    Parse {
        /// Which of the parser's faults it is.
        kind: ParseKind,
        /// The parser's words for it.
        text: Box<str>,
    },
    /// A stream's `/Length`, when one reads (`length`), doesn't end its
    /// data at `endstream`, or none reads: its data is read up to
    /// `endstream`, found by scanning. A cross-reference stream's that's a
    /// reference is [`Fault::XrefLength`].
    #[error("{}", length_words(*.length))]
    Length {
        /// What its `/Length` says, when it reads.
        length: Option<usize>,
    },
    /// A cross-reference stream's `/Length` is a reference, which no read
    /// follows: its data is read up to `endstream`, found by scanning. Its
    /// section is read before any object is listed, so the reference
    /// can't be followed there, and every other read of the stream reads
    /// it as that one did. ISO 32000-2 has it direct (7.5.8.2, as
    /// corrected); before PDF 2.0, only `/Filter` and `/DecodeParms` had
    /// to be, of the entries every stream has.
    #[error("its /Length is a reference, which isn't followed in a cross-reference stream")]
    XrefLength,
    /// A stream's data doesn't decode cleanly through its filter `filter`,
    /// and pdf-filters' fault: what decoded is kept, as far as it went.
    #[error("its data doesn't decode cleanly through /{filter}: {fault}")]
    Decode {
        /// The filter's name, unabbreviated, as `/Filter` names it
        /// (`FlateDecode`).
        filter: &'static str,
        /// pdf-filters' fault.
        fault: pdf_filters::Fault,
    },
    /// An object stream's data doesn't decrypt cleanly, and pdf-crypt's
    /// fault: what decrypted is decoded.
    #[error("its data doesn't decrypt cleanly: {fault}")]
    Decrypt {
        /// pdf-crypt's fault.
        fault: pdf_crypt::Fault,
    },
    /// An object read from object stream `stream` starts at byte `start` of
    /// its decoded data, inside object `id`, read from it before, which
    /// runs from byte `from` to `to`: the stream's header gives it a place
    /// another's value runs over (ISO 32000-2, 7.5.7). It's read all the
    /// same.
    #[error(
        "it starts at byte {start} of object stream {stream} 0 R's data, inside {id} R, which runs from byte {from} to {to}"
    )]
    Inside {
        /// Where it starts.
        start: usize,
        /// The object stream's number.
        stream: u32,
        /// The object it's inside.
        id: ObjectId,
        /// Where that one starts.
        from: usize,
        /// Where that one ends.
        to: usize,
    },
    /// `count` of an object's strings don't decrypt cleanly, by pdf-crypt's
    /// fault `fault`: what decrypted is kept. Said once an object and a
    /// fault.
    #[error("{}", strings_words(*.count, .fault))]
    Strings {
        /// How many.
        count: usize,
        /// pdf-crypt's fault.
        fault: pdf_crypt::Fault,
    },
    /// `left` of object `id`'s strings stay ciphertext: the decoder reads
    /// the object otherwise. Only where a decoder decrypts for the reader,
    /// which only the crate's own tests give, qpdf; a reading never gives
    /// it.
    // Hidden, as only the tests' decoder gives it: not part of the API.
    #[doc(hidden)]
    #[error("{left} of {id} R's strings stay ciphertext: the decoder reads the object otherwise")]
    Ciphertext {
        /// How many.
        left: usize,
        /// The object.
        id: ObjectId,
    },
    /// The strings of `objects` objects stay ciphertext, and the decoder's
    /// words for why. Only where a decoder decrypts for the reader, which
    /// only the crate's own tests give, qpdf; a reading never gives it.
    // Hidden, as only the tests' decoder gives it: not part of the API.
    #[doc(hidden)]
    #[error("{} strings stay ciphertext: {why}", objects_words(*.objects))]
    Undecrypted {
        /// How many objects.
        objects: usize,
        /// The decoder's words.
        why: String,
    },
}

/// Which of pdf-objects' parser's faults a [`Fault::Parse`] is: a kind of
/// pdf-objects' [`parse::Fault`] each, which borrows the bytes it names, as
/// a fault a reading keeps can't. Not exhaustive, as pdf-objects' isn't:
/// one it adds is [`ParseKind::Other`] until this crate names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ParseKind {
    /// [`parse::Fault::IntegerOverflow`].
    IntegerOverflow,
    /// [`parse::Fault::RealOverflow`].
    RealOverflow,
    /// [`parse::Fault::RealUnderflow`].
    RealUnderflow,
    /// [`parse::Fault::Name`].
    Name,
    /// [`parse::Fault::NoEol`].
    NoEol,
    /// [`parse::Fault::LoneCr`].
    LoneCr,
    /// [`parse::Fault::NoEndobj`].
    NoEndobj,
    /// [`parse::Fault::Number`].
    Number,
    /// [`parse::Fault::OutOfRange`].
    OutOfRange,
    /// [`parse::Fault::HexString`].
    HexString,
    /// [`parse::Fault::StringCr`].
    StringCr,
    /// [`parse::Fault::Long`].
    Long,
    /// [`parse::Fault::HighByte`].
    HighByte,
    /// One pdf-objects names that this crate doesn't yet.
    Other,
}

impl ParseKind {
    /// The kind of `fault`.
    pub(crate) fn of(fault: &parse::Fault<'_>) -> ParseKind {
        match fault {
            parse::Fault::IntegerOverflow => ParseKind::IntegerOverflow,
            parse::Fault::RealOverflow => ParseKind::RealOverflow,
            parse::Fault::RealUnderflow => ParseKind::RealUnderflow,
            parse::Fault::Name { .. } => ParseKind::Name,
            parse::Fault::NoEol => ParseKind::NoEol,
            parse::Fault::LoneCr => ParseKind::LoneCr,
            parse::Fault::NoEndobj { .. } => ParseKind::NoEndobj,
            parse::Fault::Number { .. } => ParseKind::Number,
            parse::Fault::OutOfRange { .. } => ParseKind::OutOfRange,
            parse::Fault::HexString { .. } => ParseKind::HexString,
            parse::Fault::StringCr { .. } => ParseKind::StringCr,
            parse::Fault::Long { .. } => ParseKind::Long,
            parse::Fault::HighByte { .. } => ParseKind::HighByte,
            _ => ParseKind::Other,
        }
    }
}

/// What's wrong with a cross-reference table's row that isn't written as
/// ISO 32000-2 7.5.4 writes one, 20 bytes ([`Fault::Rows`]).
/// Not exhaustive, as more ways a row is written otherwise may be told apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum RowShape {
    /// Its fields aren't a 10-digit offset, a space, a 5-digit generation,
    /// a space and `n` or `f`.
    #[error("it isn't ten digits, a space, five digits, a space and n or f")]
    Fields,
    /// Its end of line isn't two bytes: a space and CR or LF, or CR LF.
    #[error("its end of line isn't a space and CR or LF, or CR LF")]
    Eol,
}

/// Why a file's cross-reference sections don't read, from `startxref` along
/// `/Prev`, as far as the newest: what a trailer the reader made of the file
/// instead says ([`Trailer::unwritten`](crate::Trailer::unwritten)), what
/// a fault on a section further along does ([`Fault::Section`]), and what
/// stops the sections read one by one at a section that doesn't read
/// ([`SectionFault::Section`](super::SectionFault::Section)). Each in the
/// reader's words, its fields the whole of what they say; or another
/// reader's words ([`Unwritten::Other`]). Not exhaustive, as reasons are
/// found as the reader learns.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Unwritten {
    /// The file has no `startxref`.
    #[error("no startxref")]
    NoStartxref,
    /// The last `startxref`, at byte `at`, is part of a longer word.
    #[error("startxref at byte {at} is inside another word")]
    StartxrefInWord {
        /// Where.
        at: usize,
    },
    /// What follows the last `startxref` isn't an offset.
    #[error("startxref gives no offset")]
    StartxrefNoOffset,
    /// No section was read: the reader reads none, under
    /// [`Limits::sections`](super::Limits::sections) of 0.
    #[error("no section")]
    NoSection,
    /// Neither `xref` nor an object is at byte `at`, where `startxref` or a
    /// `/Prev` says a section is.
    #[error("byte {at}, which startxref or /Prev gives, is at neither `xref` nor an object")]
    Neither {
        /// Where.
        at: usize,
    },
    /// A table's subsection header, at byte `at`, doesn't read.
    #[error("a subsection's header at byte {at} doesn't read")]
    Subsection {
        /// Where.
        at: usize,
    },
    /// A table's row, at byte `at`, numbers no object: past 4,294,967,295.
    #[error("the table's row at byte {at} numbers no object")]
    RowNumber {
        /// Where.
        at: usize,
    },
    /// A table's row, at byte `at`, doesn't read.
    #[error("the table's row at byte {at} doesn't read")]
    Row {
        /// Where.
        at: usize,
    },
    /// The table at byte `at` isn't followed by `trailer`.
    #[error("the table at byte {at} has no trailer")]
    NoTrailer {
        /// Where what's there is.
        at: usize,
    },
    /// What follows a table's `trailer` doesn't read, and pdf-objects'
    /// parser's error.
    #[error("{0}")]
    Trailer(parse::Error),
    /// What follows a table's `trailer` isn't a dictionary.
    #[error("the trailer isn't a dictionary")]
    TrailerNotADictionary,
    /// The object where a cross-reference stream is said to be doesn't
    /// read, and why.
    #[error("{0}")]
    Unread(Unread),
    /// Object `id`, at byte `at`, where a cross-reference stream is said to
    /// be, isn't a stream.
    #[error("{id} R, at byte {at}, isn't a stream")]
    NotAStream {
        /// The object.
        id: ObjectId,
        /// Where.
        at: usize,
    },
    /// Object `id`, at byte `at`, where a cross-reference stream is said to
    /// be, isn't `/Type /XRef`.
    #[error("{id} R, at byte {at}, isn't /Type /XRef")]
    NotXref {
        /// The object.
        id: ObjectId,
        /// Where.
        at: usize,
    },
    /// Object `id`, whose rows are read as a cross-reference stream's,
    /// isn't a stream.
    #[error("{id} R isn't a stream")]
    Streamless {
        /// The object.
        id: ObjectId,
    },
    /// A cross-reference stream's `/W` isn't three widths of 8 bytes or
    /// fewer.
    #[error("its /W isn't three widths of 8 bytes or fewer")]
    Widths,
    /// A cross-reference stream's `/Index` isn't pairs of numbers.
    #[error("its /Index isn't pairs of numbers")]
    Index,
    /// A cross-reference stream without `/Index` has no `/Size` for one.
    #[error("it has no /Size")]
    NoSize,
    /// A cross-reference stream's `/Filter` or `/DecodeParms` are ones the
    /// reader doesn't take.
    #[error("its cross-reference stream doesn't decode: {0}")]
    Filters(Unfiltered),
    /// A cross-reference stream's data ends before its `/Index` does.
    #[error("its data ends before its /Index does")]
    DataShort,
    /// A cross-reference stream's data doesn't decode as far as its rows,
    /// and pdf-filters' first fault: [`Limit`](pdf_filters::Fault::Limit)
    /// where none stopped it short of its limit. Only the sections read one
    /// by one give it ([`Reading::sections`](super::Reading::sections)): a
    /// reading tells the decode's faults on the stream and reads its rows
    /// as far as they decode ([`Unwritten::DataShort`]).
    #[error("its data doesn't decode as far as its rows: {fault}")]
    Decode {
        /// The fault.
        fault: pdf_filters::Fault,
    },
    /// A cross-reference stream's `/Index` numbers no object: past
    /// 4,294,967,295.
    #[error("its /Index numbers no object")]
    IndexNumber,
    /// The `/Prev` chain comes back to the section at byte `at`, read
    /// before: under [`Recovery::Pdfium`](super::Recovery::Pdfium), which
    /// scans a file for that, as it does one whose sections don't read. The
    /// fault the other policy says is [`Fault::Loop`], and where
    /// [`Reading::sections`](super::Reading::sections) stops, the chain's
    /// own [`SectionFault::Loop`](super::SectionFault::Loop).
    #[error("the /Prev chain comes back to byte {at}")]
    Loop {
        /// Where it comes back to.
        at: usize,
    },
    /// The first object in use, by number, that the table at byte `table`
    /// lists, `id`, isn't at byte `offset`, where the table puts it: the
    /// first word there isn't its number. Under
    /// [`Recovery::Pdfium`](super::Recovery::Pdfium), which scans a file
    /// for that, the table being its first-written section's.
    #[error("the table at byte {table} puts {id} R at byte {offset}, where its number isn't")]
    Misplaced {
        /// The table.
        table: usize,
        /// The object.
        id: ObjectId,
        /// Where the table puts it.
        offset: usize,
    },
    /// The trailer's `/Root` gives no page: it doesn't refer to a
    /// dictionary that reads, or that dictionary's `/Pages` counts none.
    /// Under [`Recovery::Pdfium`](super::Recovery::Pdfium), which scans a
    /// file whose sections read for that.
    #[error("the trailer's /Root gives no page")]
    NoPages,
    /// A compressed cross-reference stream, its filters `filter` as
    /// written (`/FlateDecode`, or `filters` for an array), that the
    /// decoder doesn't decode, in its words. Only where a decoder decodes
    /// for the reader, which only the crate's own tests give, qpdf; a
    /// reading never gives it.
    // Hidden, as only the tests' decoder gives it: not part of the API.
    #[doc(hidden)]
    #[error("its cross-reference stream ({filter}) doesn't decode: {why}")]
    Decoder {
        /// Its filters, as written.
        filter: String,
        /// The decoder's words.
        why: String,
    },
    /// A compressed cross-reference stream, its filters `filter` as
    /// written, where the reader is made to ask a decoder and has none.
    /// Only in the crate's own tests; a reading never gives it.
    // Hidden, as only the tests' decoder gives it: not part of the API.
    #[doc(hidden)]
    #[error("its cross-reference stream is compressed ({filter}), and nothing decodes it")]
    Compressed {
        /// Its filters, as written.
        filter: String,
    },
    /// A cross-reference stream `id` the decoder decoded to `len` bytes,
    /// not the `size` its `/W` and `/Index` make (none where they make more
    /// than a count holds): the decoder may have decoded another object at
    /// its number. Only where a decoder decodes for the reader, which only
    /// the crate's own tests give, qpdf; a reading never gives it.
    // Hidden, as only the tests' decoder gives it: not part of the API.
    #[doc(hidden)]
    #[error(
        "decoded, it's {len} bytes, not the {} its /W and /Index make: the decoder may have decoded another object {id} R",
        size_words(*.size)
    )]
    Decoded {
        /// How many bytes it decoded to.
        len: usize,
        /// How many its rows are.
        size: Option<usize>,
        /// The stream.
        id: ObjectId,
    },
    /// Another reader's words: one that reads a file otherwise and makes
    /// a document of it ([`Document::new`](crate::Document::new)).
    #[error("{0}")]
    Other(String),
    /// The section at byte `at`, which the file is read as of
    /// ([`Options::section`](super::Options::section)), doesn't read, and
    /// why: under [`Recovery::Verified`](super::Recovery::Verified)
    /// nothing is read, no object and no trailer, as a scan of the file
    /// would find what later updates wrote.
    #[error(
        "the section at byte {at}, which the file is read as of, doesn't read, so nothing is: {why}"
    )]
    AsOf {
        /// Where it is.
        at: usize,
        /// Why it doesn't read.
        why: Box<Unwritten>,
    },
}

/// [`Fault::Taken`]'s keys, each a name, joined.
fn names(keys: &[Vec<u8>]) -> String {
    let names: Vec<String> = (keys.iter())
        .map(|key| format!("/{}", String::from_utf8_lossy(key)))
        .collect();
    names.join(", ")
}

/// [`Fault::Length`]'s words.
fn length_words(length: Option<usize>) -> String {
    match length {
        Some(length) => format!("its /Length, {length}, doesn't end at endstream"),
        None => "it has no /Length that reads".to_string(),
    }
}

/// How many bytes `by` is, and which way: "3 bytes before".
fn off_words(by: i64) -> String {
    let way = if by < 0 { "before" } else { "after" };
    match by.unsigned_abs() {
        1 => format!("1 byte {way}"),
        n => format!("{n} bytes {way}"),
    }
}

/// What [`Fault::Moved`] and [`Fault::Shifted`] say of a section's
/// entries: how many are `by` bytes off, how many neither there nor where
/// it puts them but where a scan finds them, and how many at none of those
/// places.
fn entries_words(moved: u32, by: i64, scanned: u32, neither: u32) -> String {
    let off = off_words(by);
    let moved_words = match moved {
        1 => format!("1 of its entries is {off} where it puts it"),
        n => format!("{n} of its entries are {off} where they're put"),
    };
    let scanned_words = match scanned {
        1 => "1 is where a scan of the file finds it".to_string(),
        n => format!("{n} are where a scan of the file finds them"),
    };
    let not_found = match neither {
        1 => "1 isn't found at all".to_string(),
        n => format!("{n} aren't found at all"),
    };
    match (moved, scanned, neither) {
        (_, 0, 0) => moved_words,
        (0, 0, 1) => format!("1 of its entries is neither where it puts it nor {off}"),
        (0, 0, n) => format!("{n} of its entries are neither where they're put nor {off}"),
        (_, 0, 1) => format!("{moved_words}, and 1 is at neither place"),
        (_, 0, n) => format!("{moved_words}, and {n} are at neither place"),
        (0, s, n) => {
            let words = match s {
                1 => format!(
                    "1 of its entries is neither where it puts it nor {off}, but where a scan of the file finds it"
                ),
                s => format!(
                    "{s} of its entries are neither where they're put nor {off}, but where a scan of the file finds them"
                ),
            };
            match n {
                0 => words,
                _ => format!("{words}, and {not_found}"),
            }
        }
        (_, _, 0) => format!("{moved_words}, and {scanned_words}"),
        _ => format!("{moved_words}, {scanned_words}, and {not_found}"),
    }
}

/// What a section's offsets counting from the file's first byte says.
const FROM_START: &str = ", its offsets counting from the file's first byte, not its header";

/// [`Fault::Moved`]'s words.
fn moved_words(
    at: usize,
    found: usize,
    from_start: bool,
    moved: u32,
    scanned: u32,
    neither: u32,
) -> String {
    let by = i64::try_from(found).unwrap_or(i64::MAX) - i64::try_from(at).unwrap_or(i64::MAX);
    let from_start = if from_start { FROM_START } else { "" };
    let section = format!(
        "the section said to be at byte {at} is at byte {found}, {}{from_start}",
        off_words(by)
    );
    match (moved, scanned, neither) {
        (0, 0, 0) => section,
        _ => format!("{section}; {}", entries_words(moved, by, scanned, neither)),
    }
}

/// [`Fault::Shifted`]'s words.
fn shifted_words(
    at: usize,
    by: i64,
    from_start: bool,
    moved: u32,
    scanned: u32,
    neither: u32,
) -> String {
    let from_start = if from_start { FROM_START } else { "" };
    format!(
        "in the section at byte {at}{from_start}, {}",
        entries_words(moved, by, scanned, neither)
    )
}

/// [`Fault::Members`]' words.
fn members_words(objects: u32, streams: u32, unread: u32) -> String {
    let streams = match streams {
        1 => "1 object stream".to_string(),
        n => format!("{n} object streams"),
    };
    let read = match objects {
        1 => format!("1 object the scan found no offset of is read from {streams} it found"),
        n => format!("{n} objects the scan found no offset of are read from {streams} it found"),
    };
    match (objects, unread) {
        (_, 0) => read,
        (0, 1) => "the objects of 1 object stream the scan found don't read".to_string(),
        (0, n) => format!("the objects of {n} object streams the scan found don't read"),
        (_, 1) => format!("{read}, and those of 1 more don't read"),
        (_, n) => format!("{read}, and those of {n} more don't read"),
    }
}

/// [`Fault::Renumbered`]'s words.
fn renumbered_words(at: usize, count: u32) -> String {
    format!(
        "the table at byte {at} numbers its first subsection from 1 but starts it with object 0's row: its {count} rows are read as numbered from 0, each in use being its object's so numbered"
    )
}

/// [`Fault::StreamEnd`]'s words.
fn stream_end_words(id: Option<ObjectId>, start: usize, end: Option<usize>) -> String {
    let whose = match id {
        Some(id) => format!("{id} R's stream data"),
        None => "the data of a stream with no n g obj before it".to_string(),
    };
    match end {
        Some(end) => format!(
            "the scan ends {whose} at its endobj, at byte {end}, as no endstream comes before it and no /Length ends at one"
        ),
        None => format!(
            "the scan finds no end to {whose}, at byte {start}, as neither endstream nor endobj comes after it and no /Length ends at one, and reads on through it"
        ),
    }
}

/// [`Fault::Unclosed`]'s words.
fn unclosed_words(id: Option<ObjectId>, start: usize, past: u32) -> String {
    let place = match id {
        Some(id) => format!("at byte {start} in {id} R"),
        None => format!("at byte {start}, outside any object,"),
    };
    match past {
        0 => format!(
            "the scan finds no ) closing the string {place} and reads the rest of the file as that string, as PDFium's scan does"
        ),
        1 => format!(
            "the scan finds no ) closing the string {place} and reads on from just past its ("
        ),
        n => format!(
            "the scan finds no ) closing {n} strings, the first {place} and reads on from just past each one's ("
        ),
    }
}

/// [`Fault::Earlier`]'s words.
fn earlier_words(id: ObjectId, at: usize, later: usize) -> String {
    format!(
        "the scan finds object {} again at byte {later}, past a string that doesn't close, where it doesn't read, and reads {id} R at byte {at}, where it found it before",
        id.number
    )
}

/// [`Fault::InString`]'s words.
fn in_string_words(id: Option<ObjectId>, start: usize, end: usize, objects: u32) -> String {
    let place = match id {
        Some(id) => format!("at byte {start} in {id} R"),
        None => format!("at byte {start}, outside any object,"),
    };
    match objects {
        1 => format!(
            "the scan reads the string {place} to a ) at byte {end}, past an object whose n g obj starts a line inside it, found nowhere else, and reads that object there"
        ),
        n => format!(
            "the scan reads the string {place} to a ) at byte {end}, past {n} objects whose n g obj starts a line inside it, found nowhere else, and reads those objects there"
        ),
    }
}

/// [`Fault::Strings`]' words.
fn strings_words(count: usize, fault: &pdf_crypt::Fault) -> String {
    match count {
        1 => format!("a string of it doesn't decrypt cleanly: {fault}"),
        n => format!("{n} of its strings don't decrypt cleanly: {fault}"),
    }
}

/// How many objects [`Fault::Undecrypted`] says.
fn objects_words(objects: usize) -> String {
    match objects {
        1 => "one object's".to_string(),
        n => format!("{n} objects'"),
    }
}

/// How many bytes [`Unwritten::Decoded`] says the rows make.
fn size_words(size: Option<usize>) -> String {
    size.map_or("many".to_string(), |size| size.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::mem::size_of;

    use pdf_objects::fault::Place;

    use super::*;
    use crate::source::Uncrypt;

    /// Object `n`, generation 0.
    fn id(n: u32) -> ObjectId {
        ObjectId::new(n, 0)
    }

    /// Which of [`Fault`]'s variants `fault` is: a match that names each,
    /// so one added fails to build here until it's given an arm, and the
    /// test below fails until it's given its words.
    fn arm(fault: &Fault) -> usize {
        match fault {
            Fault::EofNotAlone => 0,
            Fault::AfterEof { .. } => 1,
            Fault::EofFar { .. } => 2,
            Fault::NoEof => 3,
            Fault::NoHeader => 4,
            Fault::HeaderAt { .. } => 5,
            Fault::Version => 6,
            Fault::Loop { .. } => 7,
            Fault::Sections { .. } => 8,
            Fault::Section { .. } => 9,
            Fault::StreamRows { .. } => 10,
            Fault::XrefStm { .. } => 11,
            Fault::Entries { .. } => 12,
            Fault::FreeRow { .. } => 13,
            Fault::Rows { .. } => 14,
            Fault::StreamEntries { .. } => 15,
            Fault::Unlisted { .. } => 16,
            Fault::ObjectZero => 17,
            Fault::Taken { .. } => 18,
            Fault::NoEncrypt { .. } => 19,
            Fault::Unopened { .. } => 20,
            Fault::NoId => 21,
            Fault::Dictionary { .. } => 22,
            Fault::Parse { .. } => 23,
            Fault::Length { .. } => 24,
            Fault::Decode { .. } => 25,
            Fault::Decrypt { .. } => 26,
            Fault::Inside { .. } => 27,
            Fault::Strings { .. } => 28,
            Fault::Ciphertext { .. } => 29,
            Fault::Undecrypted { .. } => 30,
            Fault::Moved { .. } => 31,
            Fault::Shifted { .. } => 32,
            Fault::Members { .. } => 33,
            Fault::XrefLength => 34,
            Fault::StreamEnd { .. } => 35,
            Fault::Renumbered { .. } => 36,
            Fault::Unclosed { .. } => 37,
            Fault::Earlier { .. } => 38,
            Fault::InString { .. } => 39,
        }
    }

    /// How many variants [`arm`] names.
    const ARMS: usize = 40;

    /// Each fault, built by a literal, says its words, every variant and
    /// each way its words go; and none is left without a case.
    #[test]
    fn each_fault_says_its_words() {
        let why = || Box::new(Unwritten::Widths);
        let unsupported = Unsupported::Encrypt(Uncrypt::NotADictionary);
        let short = pdf_crypt::Fault::Short { len: 3 };
        let cases: Vec<(Fault, &str)> = vec![
            (
                Fault::EofNotAlone,
                "the file's last %%EOF isn't on a line of its own",
            ),
            (
                Fault::AfterEof { bytes: 8 },
                "the file goes on for 8 bytes after its last %%EOF",
            ),
            (
                Fault::EofFar { bytes: 2000 },
                "the file's last %%EOF is 2000 bytes from its end, past the 1024 readers look in",
            ),
            (Fault::NoEof, "the file has no %%EOF"),
            (Fault::NoHeader, "no %PDF- header"),
            (
                Fault::HeaderAt { at: 3 },
                "%PDF- is at byte 3, not the file's first",
            ),
            (Fault::Version, "the header's version doesn't read"),
            (
                Fault::Loop { at: 120 },
                "the /Prev chain comes back to byte 120",
            ),
            (
                Fault::Sections { limit: 1024 },
                "more than 1024 cross-reference sections",
            ),
            (
                Fault::Section { at: 9, why: why() },
                "the section at byte 9 doesn't read: its /W isn't three widths of 8 bytes or fewer",
            ),
            (
                Fault::StreamRows { at: 9, why: why() },
                "the section, at byte 9, doesn't read: its /W isn't three widths of 8 bytes or fewer",
            ),
            (
                Fault::XrefStm { at: 9, why: why() },
                "its /XRefStm, at byte 9, doesn't read: its /W isn't three widths of 8 bytes or fewer",
            ),
            (
                Fault::Entries {
                    past: 2,
                    most: 5,
                    len: 300,
                },
                "the cross-reference sections list 2 objects past the first 5, as many as the reader reads of a file of 300 bytes, which aren't read",
            ),
            (
                Fault::FreeRow { number: 4 },
                "the table's row for object 4 gives a number out of range, and is read as free",
            ),
            (
                Fault::Rows {
                    misshapen: 2,
                    rows: 3,
                    number: 1,
                    why: RowShape::Eol,
                },
                "2 of the table's 3 rows aren't 20 bytes as ISO 32000-2 7.5.4 writes them, the first object 1's: its end of line isn't a space and CR or LF, or CR LF",
            ),
            (
                Fault::StreamEntries {
                    past: 1,
                    most: 4,
                    len: 90,
                },
                "its rows list 1 entries past the first 4, as many as the reader reads of a file of 90 bytes, which aren't read",
            ),
            (
                Fault::Unlisted { id: id(5) },
                "cross-reference stream 5 0 R doesn't list itself",
            ),
            (
                Fault::ObjectZero,
                "the cross-references list object 0, which is always free (ISO 32000-2, 7.5.4)",
            ),
            (
                Fault::Taken {
                    keys: vec![b"Root".to_vec()],
                },
                "the last trailer lacks /Root, which it's given from an earlier one",
            ),
            (
                Fault::Taken {
                    keys: vec![b"Root".to_vec(), b"Info".to_vec()],
                },
                "the last trailer lacks /Root, /Info, which it's given from an earlier one",
            ),
            (
                Fault::NoEncrypt { id: id(9) },
                "the trailer's /Encrypt is 9 0 R, which is no object, so the file is read as not encrypted",
            ),
            (
                Fault::Unopened {
                    why: unsupported.clone(),
                    strings: true,
                },
                "its security handler doesn't open: its /Encrypt isn't a dictionary; its strings stay ciphertext, and its object streams aren't read",
            ),
            (
                Fault::Unopened {
                    why: Unsupported::Refused(pdf_crypt::Refused::Malformed("no /O".to_string())),
                    strings: false,
                },
                "its security handler doesn't open: no /O; its object streams aren't read",
            ),
            (
                Fault::NoId,
                "its trailer's /ID has no string first, so its key is made without one",
            ),
            (
                Fault::Dictionary {
                    fault: pdf_crypt::Fault::Perms,
                },
                "its encryption dictionary isn't right: its /Perms doesn't match its /P and /EncryptMetadata",
            ),
            (
                Fault::Parse {
                    kind: ParseKind::NoEndobj,
                    text: "4 0 obj has no endobj".into(),
                },
                "4 0 obj has no endobj",
            ),
            (
                Fault::Length { length: Some(99) },
                "its /Length, 99, doesn't end at endstream",
            ),
            (
                Fault::Length { length: None },
                "it has no /Length that reads",
            ),
            (
                Fault::XrefLength,
                "its /Length is a reference, which isn't followed in a cross-reference stream",
            ),
            (
                Fault::Decode {
                    filter: "FlateDecode",
                    fault: pdf_filters::Fault::Checksum,
                },
                "its data doesn't decode cleanly through /FlateDecode: the Adler-32 checksum doesn't match",
            ),
            (
                Fault::Decrypt {
                    fault: short.clone(),
                },
                "its data doesn't decrypt cleanly: its AES ciphertext has no whole block past its IV",
            ),
            (
                Fault::Inside {
                    start: 14,
                    stream: 4,
                    id: id(2),
                    from: 12,
                    to: 23,
                },
                "it starts at byte 14 of object stream 4 0 R's data, inside 2 0 R, which runs from byte 12 to 23",
            ),
            (
                Fault::Strings {
                    count: 1,
                    fault: short.clone(),
                },
                "a string of it doesn't decrypt cleanly: its AES ciphertext has no whole block past its IV",
            ),
            (
                Fault::Strings {
                    count: 2,
                    fault: short,
                },
                "2 of its strings don't decrypt cleanly: its AES ciphertext has no whole block past its IV",
            ),
            (
                Fault::Ciphertext { left: 2, id: id(4) },
                "2 of 4 0 R's strings stay ciphertext: the decoder reads the object otherwise",
            ),
            (
                Fault::Undecrypted {
                    objects: 1,
                    why: "no key".to_string(),
                },
                "one object's strings stay ciphertext: no key",
            ),
            (
                Fault::Undecrypted {
                    objects: 3,
                    why: "no key".to_string(),
                },
                "3 objects' strings stay ciphertext: no key",
            ),
            (
                Fault::Moved {
                    at: 700,
                    found: 697,
                    from_start: false,
                    moved: 1,
                    scanned: 0,
                    neither: 0,
                },
                "the section said to be at byte 700 is at byte 697, 3 bytes before; 1 of its entries is 3 bytes before where it puts it",
            ),
            (
                Fault::Moved {
                    at: 120,
                    found: 121,
                    from_start: false,
                    moved: 0,
                    scanned: 0,
                    neither: 0,
                },
                "the section said to be at byte 120 is at byte 121, 1 byte after",
            ),
            (
                Fault::Moved {
                    at: 158,
                    found: 58,
                    from_start: true,
                    moved: 4,
                    scanned: 0,
                    neither: 2,
                },
                "the section said to be at byte 158 is at byte 58, 100 bytes before, its offsets counting from the file's first byte, not its header; 4 of its entries are 100 bytes before where they're put, and 2 are at neither place",
            ),
            (
                Fault::Moved {
                    at: 571,
                    found: 575,
                    from_start: false,
                    moved: 0,
                    scanned: 0,
                    neither: 6,
                },
                "the section said to be at byte 571 is at byte 575, 4 bytes after; 6 of its entries are neither where they're put nor 4 bytes after",
            ),
            (
                Fault::Moved {
                    at: 734,
                    found: 732,
                    from_start: false,
                    moved: 0,
                    scanned: 0,
                    neither: 1,
                },
                "the section said to be at byte 734 is at byte 732, 2 bytes before; 1 of its entries is neither where it puts it nor 2 bytes before",
            ),
            (
                Fault::Members {
                    objects: 12,
                    streams: 2,
                    unread: 0,
                },
                "12 objects the scan found no offset of are read from 2 object streams it found",
            ),
            (
                Fault::Members {
                    objects: 1,
                    streams: 1,
                    unread: 1,
                },
                "1 object the scan found no offset of is read from 1 object stream it found, and those of 1 more don't read",
            ),
            (
                Fault::Members {
                    objects: 0,
                    streams: 0,
                    unread: 2,
                },
                "the objects of 2 object streams the scan found don't read",
            ),
            (
                Fault::Renumbered { at: 400, count: 7 },
                "the table at byte 400 numbers its first subsection from 1 but starts it with object 0's row: its 7 rows are read as numbered from 0, each in use being its object's so numbered",
            ),
            (
                Fault::StreamEnd {
                    id: Some(id(9)),
                    start: 30,
                    end: Some(47),
                },
                "the scan ends 9 0 R's stream data at its endobj, at byte 47, as no endstream comes before it and no /Length ends at one",
            ),
            (
                Fault::StreamEnd {
                    id: Some(id(9)),
                    start: 30,
                    end: None,
                },
                "the scan finds no end to 9 0 R's stream data, at byte 30, as neither endstream nor endobj comes after it and no /Length ends at one, and reads on through it",
            ),
            (
                Fault::StreamEnd {
                    id: None,
                    start: 30,
                    end: Some(47),
                },
                "the scan ends the data of a stream with no n g obj before it at its endobj, at byte 47, as no endstream comes before it and no /Length ends at one",
            ),
            (
                Fault::Unclosed {
                    id: Some(id(9)),
                    start: 30,
                    past: 1,
                },
                "the scan finds no ) closing the string at byte 30 in 9 0 R and reads on from just past its (",
            ),
            (
                Fault::Unclosed {
                    id: None,
                    start: 30,
                    past: 3,
                },
                "the scan finds no ) closing 3 strings, the first at byte 30, outside any object, and reads on from just past each one's (",
            ),
            (
                Fault::Unclosed {
                    id: Some(id(9)),
                    start: 30,
                    past: 0,
                },
                "the scan finds no ) closing the string at byte 30 in 9 0 R and reads the rest of the file as that string, as PDFium's scan does",
            ),
            (
                Fault::Earlier {
                    id: id(9),
                    at: 30,
                    later: 470,
                },
                "the scan finds object 9 again at byte 470, past a string that doesn't close, where it doesn't read, and reads 9 0 R at byte 30, where it found it before",
            ),
            (
                Fault::InString {
                    id: Some(id(46)),
                    start: 30,
                    end: 470,
                    objects: 1,
                },
                "the scan reads the string at byte 30 in 46 0 R to a ) at byte 470, past an object whose n g obj starts a line inside it, found nowhere else, and reads that object there",
            ),
            (
                Fault::InString {
                    id: None,
                    start: 30,
                    end: 470,
                    objects: 3,
                },
                "the scan reads the string at byte 30, outside any object, to a ) at byte 470, past 3 objects whose n g obj starts a line inside it, found nowhere else, and reads those objects there",
            ),
            (
                Fault::Shifted {
                    at: 700,
                    by: 2,
                    from_start: false,
                    moved: 30,
                    scanned: 0,
                    neither: 1,
                },
                "in the section at byte 700, 30 of its entries are 2 bytes after where they're put, and 1 is at neither place",
            ),
            (
                Fault::Shifted {
                    at: 701,
                    by: -1,
                    from_start: true,
                    moved: 1,
                    scanned: 0,
                    neither: 0,
                },
                "in the section at byte 701, its offsets counting from the file's first byte, not its header, 1 of its entries is 1 byte before where it puts it",
            ),
            (
                Fault::Moved {
                    at: 2589,
                    found: 2547,
                    from_start: false,
                    moved: 7,
                    scanned: 5,
                    neither: 0,
                },
                "the section said to be at byte 2589 is at byte 2547, 42 bytes before; 7 of its entries are 42 bytes before where they're put, and 5 are where a scan of the file finds them",
            ),
            (
                Fault::Moved {
                    at: 571,
                    found: 575,
                    from_start: false,
                    moved: 0,
                    scanned: 2,
                    neither: 1,
                },
                "the section said to be at byte 571 is at byte 575, 4 bytes after; 2 of its entries are neither where they're put nor 4 bytes after, but where a scan of the file finds them, and 1 isn't found at all",
            ),
            (
                Fault::Shifted {
                    at: 900,
                    by: -2,
                    from_start: false,
                    moved: 3,
                    scanned: 1,
                    neither: 2,
                },
                "in the section at byte 900, 3 of its entries are 2 bytes before where they're put, 1 is where a scan of the file finds it, and 2 aren't found at all",
            ),
            (
                Fault::Shifted {
                    at: 902,
                    by: 5,
                    from_start: false,
                    moved: 0,
                    scanned: 1,
                    neither: 0,
                },
                "in the section at byte 902, 1 of its entries is neither where it puts it nor 5 bytes after, but where a scan of the file finds it",
            ),
        ];
        for (fault, words) in &cases {
            assert_eq!(fault.to_string(), *words, "{fault:?}");
        }
        let covered: BTreeSet<usize> = cases.iter().map(|(fault, _)| arm(fault)).collect();
        assert_eq!(covered, (0..ARMS).collect(), "a variant without its words");
    }

    /// Which of [`Unwritten`]'s variants `why` is, as [`arm`] is of
    /// [`Fault`]'s.
    fn reason(why: &Unwritten) -> usize {
        match why {
            Unwritten::NoStartxref => 0,
            Unwritten::StartxrefInWord { .. } => 1,
            Unwritten::StartxrefNoOffset => 2,
            Unwritten::NoSection => 3,
            Unwritten::Neither { .. } => 4,
            Unwritten::Subsection { .. } => 5,
            Unwritten::RowNumber { .. } => 6,
            Unwritten::Row { .. } => 7,
            Unwritten::NoTrailer { .. } => 8,
            Unwritten::Trailer(_) => 9,
            Unwritten::TrailerNotADictionary => 10,
            Unwritten::Unread(_) => 11,
            Unwritten::NotAStream { .. } => 12,
            Unwritten::NotXref { .. } => 13,
            Unwritten::Streamless { .. } => 14,
            Unwritten::Widths => 15,
            Unwritten::Index => 16,
            Unwritten::NoSize => 17,
            Unwritten::Filters(_) => 18,
            Unwritten::DataShort => 19,
            Unwritten::IndexNumber => 20,
            Unwritten::Decoder { .. } => 21,
            Unwritten::Compressed { .. } => 22,
            Unwritten::Decoded { .. } => 23,
            Unwritten::Other(_) => 24,
            Unwritten::Decode { .. } => 25,
            Unwritten::Loop { .. } => 26,
            Unwritten::Misplaced { .. } => 27,
            Unwritten::NoPages => 28,
            Unwritten::AsOf { .. } => 29,
        }
    }

    /// How many variants [`reason`] names.
    const REASONS: usize = 30;

    /// Each reason the sections don't read, built by a literal, says its
    /// words, every variant and each way its words go.
    #[test]
    fn each_reason_says_its_words() {
        let cases: Vec<(Unwritten, &str)> = vec![
            (Unwritten::NoStartxref, "no startxref"),
            (
                Unwritten::StartxrefInWord { at: 40 },
                "startxref at byte 40 is inside another word",
            ),
            (Unwritten::StartxrefNoOffset, "startxref gives no offset"),
            (Unwritten::NoSection, "no section"),
            (
                Unwritten::Neither { at: 7 },
                "byte 7, which startxref or /Prev gives, is at neither `xref` nor an object",
            ),
            (
                Unwritten::Subsection { at: 7 },
                "a subsection's header at byte 7 doesn't read",
            ),
            (
                Unwritten::RowNumber { at: 7 },
                "the table's row at byte 7 numbers no object",
            ),
            (
                Unwritten::Row { at: 7 },
                "the table's row at byte 7 doesn't read",
            ),
            (
                Unwritten::NoTrailer { at: 7 },
                "the table at byte 7 has no trailer",
            ),
            (
                Unwritten::Trailer(parse::Error::Nested { limit: 2, at: 0 }),
                "arrays and dictionaries nested deeper than 2",
            ),
            (
                Unwritten::TrailerNotADictionary,
                "the trailer isn't a dictionary",
            ),
            (
                Unwritten::Unread(Unread::Missing { id: id(3) }),
                "3 0 R isn't in the file",
            ),
            (
                Unwritten::NotAStream { id: id(3), at: 7 },
                "3 0 R, at byte 7, isn't a stream",
            ),
            (
                Unwritten::NotXref { id: id(3), at: 7 },
                "3 0 R, at byte 7, isn't /Type /XRef",
            ),
            (Unwritten::Streamless { id: id(3) }, "3 0 R isn't a stream"),
            (
                Unwritten::Widths,
                "its /W isn't three widths of 8 bytes or fewer",
            ),
            (Unwritten::Index, "its /Index isn't pairs of numbers"),
            (Unwritten::NoSize, "it has no /Size"),
            (
                Unwritten::Filters(Unfiltered::Reference),
                "its cross-reference stream doesn't decode: its /Filter or /DecodeParms holds a reference, which a cross-reference stream's may not",
            ),
            (Unwritten::DataShort, "its data ends before its /Index does"),
            (Unwritten::IndexNumber, "its /Index numbers no object"),
            (
                Unwritten::Loop { at: 120 },
                "the /Prev chain comes back to byte 120",
            ),
            (
                Unwritten::Misplaced {
                    table: 700,
                    id: id(1),
                    offset: 15,
                },
                "the table at byte 700 puts 1 0 R at byte 15, where its number isn't",
            ),
            (Unwritten::NoPages, "the trailer's /Root gives no page"),
            (
                Unwritten::AsOf {
                    at: 120,
                    why: Box::new(Unwritten::Neither { at: 120 }),
                },
                "the section at byte 120, which the file is read as of, doesn't read, so nothing is: byte 120, which startxref or /Prev gives, is at neither `xref` nor an object",
            ),
            (
                Unwritten::Decode {
                    fault: pdf_filters::Fault::NoEnd,
                },
                "its data doesn't decode as far as its rows: no end-of-data marker",
            ),
            (
                Unwritten::Decoder {
                    filter: "/FlateDecode".to_string(),
                    why: "no data".to_string(),
                },
                "its cross-reference stream (/FlateDecode) doesn't decode: no data",
            ),
            (
                Unwritten::Compressed {
                    filter: "filters".to_string(),
                },
                "its cross-reference stream is compressed (filters), and nothing decodes it",
            ),
            (
                Unwritten::Decoded {
                    len: 3,
                    size: Some(12),
                    id: id(2),
                },
                "decoded, it's 3 bytes, not the 12 its /W and /Index make: the decoder may have decoded another object 2 0 R",
            ),
            (
                Unwritten::Decoded {
                    len: 3,
                    size: None,
                    id: id(2),
                },
                "decoded, it's 3 bytes, not the many its /W and /Index make: the decoder may have decoded another object 2 0 R",
            ),
            (
                Unwritten::Other("qpdf read the file otherwise".to_string()),
                "qpdf read the file otherwise",
            ),
        ];
        for (why, words) in &cases {
            assert_eq!(why.to_string(), *words, "{why:?}");
        }
        let covered: BTreeSet<usize> = cases.iter().map(|(why, _)| reason(why)).collect();
        assert_eq!(
            covered,
            (0..REASONS).collect(),
            "a variant without its words"
        );
    }

    /// Why a handler doesn't open, and why an `/Encrypt` isn't a
    /// dictionary the reader reads, and what's wrong with a table's row,
    /// each built by a literal, say their words, every variant.
    #[test]
    fn each_unsupported_and_row_shape_says_its_words() {
        let unsupported = |why: &Unsupported| match why {
            Unsupported::Encrypt(_) => 0,
            Unsupported::Refused(_) => 1,
        };
        let uncrypt = |why: &Uncrypt| match why {
            Uncrypt::NotADictionary => 0,
            Uncrypt::Unread => 1,
            Uncrypt::InObjectStream => 2,
        };
        let shape = |why: &RowShape| match why {
            RowShape::Fields => 0,
            RowShape::Eol => 1,
        };
        let encrypt = [
            (Uncrypt::NotADictionary, "its /Encrypt isn't a dictionary"),
            (Uncrypt::Unread, "its /Encrypt doesn't read"),
            (
                Uncrypt::InObjectStream,
                "its /Encrypt is in an object stream, which it can't be (ISO 32000-2, 7.5.7)",
            ),
        ];
        for (why, words) in &encrypt {
            assert_eq!(why.to_string(), *words);
            assert_eq!(Unsupported::Encrypt(*why).to_string(), *words);
        }
        let covered: BTreeSet<usize> = encrypt.iter().map(|(why, _)| uncrypt(why)).collect();
        assert_eq!(covered, (0..3).collect());
        let refused = [
            (pdf_crypt::Refused::Malformed("no /O".to_string()), "no /O"),
            (pdf_crypt::Refused::Password, "the password doesn't open it"),
        ];
        for (refused, words) in refused {
            assert_eq!(Unsupported::Refused(refused).to_string(), words);
        }
        let all = [
            Unsupported::Encrypt(Uncrypt::Unread),
            Unsupported::Refused(pdf_crypt::Refused::Password),
        ];
        let covered: BTreeSet<usize> = all.iter().map(unsupported).collect();
        assert_eq!(covered, (0..2).collect());
        let shapes = [
            (
                RowShape::Fields,
                "it isn't ten digits, a space, five digits, a space and n or f",
            ),
            (
                RowShape::Eol,
                "its end of line isn't a space and CR or LF, or CR LF",
            ),
        ];
        for (why, words) in &shapes {
            assert_eq!(why.to_string(), *words);
        }
        let covered: BTreeSet<usize> = shapes.iter().map(|(why, _)| shape(why)).collect();
        assert_eq!(covered, (0..2).collect());
    }

    /// Each of pdf-objects' parser's faults is of its kind, and one this
    /// crate doesn't name yet would be [`ParseKind::Other`].
    #[test]
    fn each_of_the_parsers_faults_is_of_its_kind() {
        let written = &b"/A#zz"[..];
        let cases = [
            (parse::Fault::IntegerOverflow, ParseKind::IntegerOverflow),
            (parse::Fault::RealOverflow, ParseKind::RealOverflow),
            (parse::Fault::RealUnderflow, ParseKind::RealUnderflow),
            (
                parse::Fault::Name {
                    written,
                    stray: true,
                    nul: false,
                },
                ParseKind::Name,
            ),
            (parse::Fault::NoEol, ParseKind::NoEol),
            (parse::Fault::LoneCr, ParseKind::LoneCr),
            (parse::Fault::NoEndobj { id: id(4) }, ParseKind::NoEndobj),
            (parse::Fault::Number { written }, ParseKind::Number),
            (parse::Fault::OutOfRange { written }, ParseKind::OutOfRange),
            (parse::Fault::HexString { written }, ParseKind::HexString),
            (parse::Fault::StringCr { written }, ParseKind::StringCr),
            (parse::Fault::Long { written }, ParseKind::Long),
            (parse::Fault::HighByte { written }, ParseKind::HighByte),
        ];
        let mut kinds = BTreeSet::new();
        for (fault, kind) in cases {
            assert_eq!(ParseKind::of(&fault), kind, "{fault:?}");
            kinds.insert(format!("{kind:?}"));
        }
        assert_eq!(kinds.len(), 13, "each kind once");
    }

    /// What a reading keeps of a fault, it and its place, is no larger than
    /// it need be: an error a fault nests that's larger than the rest of
    /// its fields, the reason a section doesn't read, is boxed, so the
    /// largest are those of a few integers or pdf-crypt's fault (40 bytes
    /// on a 64-bit target); and its place is pdf-objects'. What the reader
    /// kept of one before, as text, its object, its place and its words,
    /// was 72, its words on the heap besides.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn a_fault_and_its_place_are_as_small_as_their_fields_let_them_be() {
        assert!(size_of::<Fault>() <= 40, "{}", size_of::<Fault>());
        assert!(size_of::<Place>() <= 48, "{}", size_of::<Place>());
        let kept = size_of::<(Place, Fault)>();
        assert!(kept <= 88, "{kept}");
        assert!(size_of::<Unwritten>() > size_of::<Box<Unwritten>>());
    }
}
