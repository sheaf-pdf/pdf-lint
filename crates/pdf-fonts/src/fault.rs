//! Where a font departs from ISO 32000-2. A font's reading goes on past each
//! [`Fault`], told to a [`Sink`] with where it is. Nothing told changes what's
//! read: the font given is the same whatever the sink, an `Err` included, and a
//! sink that doesn't want faults, `()`, has none looked for.

use pdf_objects::{ObjectId, Span};

/// Where faults are told: a checker keeps them; `()` drops them, and wants none,
/// so none is looked for. A fault owns nothing, so a sink can keep it as it is. A
/// sink borrowed, `&mut S` or `&mut dyn Sink`, is a sink too, and a `Vec` keeps
/// each with its place.
pub trait Sink {
    /// A fault at `at`.
    fn fault(&mut self, at: Place, fault: Fault);

    /// Whether faults are wanted. A sink that doesn't want them is told none.
    /// True unless a sink says otherwise; asked as a reading goes.
    fn wants(&self) -> bool {
        true
    }
}

/// Drops every fault, and wants none, so none is looked for.
impl Sink for () {
    #[inline]
    fn fault(&mut self, _: Place, _: Fault) {}

    #[inline]
    fn wants(&self) -> bool {
        false
    }
}

/// Tells the sink it borrows, and wants what it wants.
impl<S: Sink + ?Sized> Sink for &mut S {
    #[inline]
    fn fault(&mut self, at: Place, fault: Fault) {
        (**self).fault(at, fault);
    }

    #[inline]
    fn wants(&self) -> bool {
        (**self).wants()
    }
}

/// Keeps each fault with its place, in the order they were told.
impl Sink for Vec<(Place, Fault)> {
    fn fault(&mut self, at: Place, fault: Fault) {
        self.push((at, fault));
    }
}

/// Where a [`Fault`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Place {
    /// The numbered object it's in, where that's another than the font
    /// dictionary, which a reading is given as a dictionary: its descriptor, its
    /// CIDFont, a CMap's stream, an object that doesn't read.
    pub object: Option<ObjectId>,
    /// Where what's at fault was written, when the reader knows.
    pub at: Option<Span>,
}

impl Place {
    /// A fault's place, for a sink to be tried on: a reading makes its own.
    pub const fn new(object: Option<ObjectId>, at: Option<Span>) -> Place {
        Place { object, at }
    }
}

/// A place where a font isn't what ISO 32000-2 says it is, which a reading comes
/// to and goes on past. Each names its clause, but for the last two, which are of
/// the file rather than its font.
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
    /// encoding is read. `MacExpertEncoding` is a base encoding, though it's only read
    /// as PDFium reads the name, as WinAnsiEncoding, and not in a dictionary.
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
    /// read, a loop, or one past [`Limits::cmaps`](crate::Limits::cmaps) (9.7.5.3).
    /// What's read of it is.
    #[error("a CMap's /UseCMap chain can't be read whole")]
    UseCMap,
    /// A `/ToUnicode` that's no stream, or one that doesn't decode (9.10.3). Text
    /// is read as with none.
    #[error("a /ToUnicode that's no stream")]
    ToUnicode,
    /// A CMap named that this reading doesn't have: one that isn't predefined
    /// (9.7.5.2), or one PDFium doesn't know, whose codes are two bytes each, each its
    /// CID, as PDFium reads them; or one a build without the `predefined-cmaps` feature
    /// hasn't, whose codes are guessed.
    #[error("a predefined CMap this reading doesn't have")]
    PredefinedUnknown,
    /// A program that doesn't read as its format (9.9), or that's past
    /// [`Limits`](crate::Limits). It reads as no program at all, as PDFium reads one
    /// FreeType doesn't load, its glyphs' boxes and its heights guessed.
    #[error("a font program that doesn't read")]
    ProgramMalformed,
    /// A reference to a reference (7.3.10), which ISO 32000-2 allows and PDFium
    /// follows only to a dictionary, or a number or a string in a value it has already
    /// followed one to. Where it doesn't, what it leads to reads as none.
    #[error("a reference to a reference")]
    Chained,
    /// An object a font reads doesn't read ([`Why::Damaged`](crate::Why::Damaged)).
    /// It's read as not there.
    #[error("object {} {} doesn't read", .id.0, .id.1)]
    Damaged {
        /// The object.
        id: ObjectId,
    },
}
