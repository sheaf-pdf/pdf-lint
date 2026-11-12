//! Where a font departs from ISO 32000-2, and where its reading meets a cap of
//! its own or what its build hasn't. A font's reading goes on past each
//! [`Fault`], told to a sink, pdf-objects'
//! [`Sink<Fault>`](pdf_objects::fault::Sink), with where it is, a
//! [`Place`](pdf_objects::fault::Place): the numbered object it's in, where
//! that's another than the font dictionary, which a reading is given as a
//! dictionary (its descriptor, its CIDFont, a CMap's stream, an object that
//! doesn't read), and where it was written. Nothing told changes what's read:
//! the font given is the same whatever the sink, an `Err` included, and a sink
//! that doesn't want faults, `()`, has none looked for; a `Vec` keeps each with
//! its place.

use pdf_objects::ObjectId;

/// A place where a font isn't what ISO 32000-2 says it is, or where its reading
/// meets a cap of its own or what its build hasn't, which a reading comes to and goes
/// on past. Each names its clause, but [`Fault::TooManyCMaps`] and
/// [`Fault::ProgramCapped`], caps that ISO 32000-2 doesn't set, which say nothing of
/// the file, and the last, which is of the file rather than its font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Fault {
    /// A `/Subtype` that's no font's (9.5), or no CIDFont's (9.7.4), or none. A
    /// font is read as Type 1, a CIDFont as Type 2.
    #[error("a /Subtype that's no font's")]
    Subtype,
    /// No `/Widths` in a simple font that isn't one of the standard 14 (9.6.2.1).
    #[error("a simple font with no /Widths")]
    NoWidths,
    /// A `/Widths` of another length than `/LastChar` less `/FirstChar` and one
    /// (9.6.2.1). It's read for as long as both go on.
    #[error("a /Widths of another length than /FirstChar and /LastChar say")]
    WidthsLength,
    /// A `/FirstChar` that's no byte's code, no number, or none where a `/Widths` is
    /// given (9.6.2.1). A code past a byte's has no width from `/Widths`, and what's no
    /// number is read as PDFium reads it.
    #[error("a /FirstChar that's no code")]
    FirstChar,
    /// A width, or a descriptor's metric or `/Flags`, that's no number (9.6.2.1,
    /// 9.7.4.3, 9.8.1). It's read as PDFium reads what's no number: `true` as 1, and
    /// anything else 0, a reference to nothing among them.
    #[error("a font's metric isn't a number")]
    NotANumber,
    /// An `/Encoding` or `/BaseEncoding` that's no base encoding's name, or an
    /// `/Encoding` that's neither that nor a dictionary (9.6.5.1). The font's own
    /// encoding is read. `MacExpertEncoding` is a base encoding, which reads as
    /// WinAnsiEncoding as PDFium reads it by name, and as `/BaseEncoding` where the program
    /// is TrueType; anywhere else it's unread.
    #[error("an /Encoding that's no encoding")]
    Encoding,
    /// A `/Differences` value that's neither a number nor a name, or a name at a
    /// code past 255 (9.6.5.1). What's neither sets the code as PDFium reads it, `true`
    /// to 1 and anything else to 0; a name past 255 names nothing, and the code goes on
    /// from it.
    #[error("a /Differences value that's neither a code nor a glyph's name")]
    Differences,
    /// A Type 0 font's `/DescendantFonts` that's not one CIDFont (9.7.6.1). It's
    /// read as having none. PDFium doesn't load such a font, and draws nothing in it.
    #[error("a /DescendantFonts that's not one CIDFont")]
    DescendantFonts,
    /// A `/W` with an array where a range's last CID goes (9.7.4.3). It's read
    /// as far as that.
    #[error("a /W cut short by an array")]
    W,
    /// A `/W2` with an array where a range's last CID goes (9.7.4.3). It's read
    /// as far as that.
    #[error("a /W2 cut short by an array")]
    W2,
    /// A Type 0 font's `/Encoding` that's neither a name nor a stream (9.7.6.1).
    /// Its codes are guessed. PDFium doesn't load such a font, and draws nothing in it.
    #[error("a Type 0 font's /Encoding is neither a name nor a stream")]
    CMap,
    /// A CMap's `/UseCMap` chain that can't be read whole: a stream that doesn't
    /// read, or a loop (9.7.5.3). What's read of it is. A chain past
    /// [`Limits::cmaps`](crate::Limits::cmaps) is [`Fault::TooManyCMaps`].
    #[error("a CMap's /UseCMap chain can't be read whole")]
    UseCMap,
    /// A CMap's `/UseCMap` chain of more streams than
    /// [`Limits::cmaps`](crate::Limits::cmaps): a cap of this reading's, which ISO
    /// 32000-2 doesn't set, told in the first stream past it. What's read of it is,
    /// and its codes are guessed past it.
    #[error("a CMap's /UseCMap chain longer than is read")]
    TooManyCMaps,
    /// A `/ToUnicode` that's no stream, or one that doesn't decode (9.10.3). Text
    /// is read as with none.
    #[error("a /ToUnicode that's no stream, or doesn't decode")]
    ToUnicode,
    /// A CMap named that no build reads as it's named: one that's no predefined CMap
    /// (9.7.5.2), or, as a Type 0 font's `/Encoding`, CNS-EUC-H or -V, which are, but
    /// which PDFium doesn't know. As an `/Encoding`, a name PDFium doesn't know is read
    /// as it reads it, its codes two bytes each, each its CID, and one it knows that
    /// ISO 32000-2 doesn't predefine (GBK2K-EUC-H, say) has its codes guessed; used by
    /// another CMap, the codes nothing else maps are guessed. It's the file's, but for
    /// CNS-EUC-H and -V.
    #[error("a CMap name that's no predefined CMap, or one PDFium doesn't know")]
    PredefinedUnknown,
    /// A predefined CMap (9.7.5.2) that a build with the `predefined-cmaps` feature reads
    /// from Adobe's bundle, and this build, without it, hasn't: of the build, not the
    /// file. Its codes are guessed, split by the font's `/ToUnicode` codespace and their
    /// CIDs unknown, or, used by another CMap, the codes nothing else maps; a build with
    /// the feature never tells it.
    #[error("a predefined CMap this build doesn't have")]
    PredefinedNotBuilt,
    /// A program that doesn't read as its format (9.9). It reads as no program at all,
    /// as PDFium reads one FreeType doesn't load, its glyphs' boxes and its heights
    /// guessed.
    #[error("a font program that doesn't read")]
    ProgramMalformed,
    /// A Type 1 program whose reading would cost past
    /// [`Limits::type1_work`](crate::Limits::type1_work): a cap of this reading's, which
    /// ISO 32000-2 doesn't set and which says nothing of the program, told in its stream.
    /// It reads as no program at all, as [`Fault::ProgramMalformed`]'s does.
    #[error("a Type 1 font program past what reading one may cost")]
    ProgramCapped,
    /// A reference to a reference (7.3.10), which ISO 32000-2 allows and PDFium
    /// follows only to a dictionary, or a number or a string in a value it has already
    /// followed one to. Where it doesn't, what it leads to reads as none.
    #[error("a reference to a reference")]
    Chained,
    /// An object a font reads doesn't read ([`Why::Damaged`](crate::Why::Damaged)).
    /// It's read as not there.
    #[error("object {} {} doesn't read", .id.number, .id.generation)]
    Damaged {
        /// The object.
        id: ObjectId,
    },
}
