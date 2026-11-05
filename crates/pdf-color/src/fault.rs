//! Where a color space departs from ISO 32000-2. A space's reading goes on past
//! each [`Fault`], told to a sink, pdf-objects'
//! [`Sink<Fault>`](pdf_objects::fault::Sink), with where it is, a
//! [`Place`](pdf_objects::fault::Place): the numbered object it's in, none
//! where a reading began in a value written in place and hasn't reached one
//! yet, and where it was written. Nothing told changes what's read: the space
//! given is the same whatever the sink, an `Err` included, and a sink that
//! doesn't want faults, `()`, has none looked for; a `Vec` keeps each with its
//! place.

use pdf_objects::ObjectId;

/// A place where a color space isn't what ISO 32000-2 says it is. Each names
/// its clause, but for the caps, which are ours. What a space holds where it's
/// at fault is on its field: none, unless it says otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Fault {
    /// A value that's neither a name nor an array that begins with one
    /// (8.6.3), or a reference to nothing where a space was asked for. No
    /// space is given.
    #[error("not a color space")]
    NotASpace,
    /// A name that no family has (8.6.3, Table 61). No space is given: where a
    /// resources dictionary names a space, the name is looked up there
    /// ([`Spaces`](crate::Spaces)) before it's a family at all.
    #[error("a color space of no family")]
    UnknownFamily,
    /// `G`, `RGB`, `CMYK` or `I` for a family's name, which only an inline
    /// image may write (8.9.7, Table 92). It's read as the family, and the
    /// reader of an inline image ignores this.
    #[error("a family's name abbreviated outside an inline image")]
    Abbreviated,
    /// An array with more elements than its family has (8.6.3, and each
    /// family's clause). The rest aren't read.
    #[error("a color space has elements past its family's")]
    ExtraElements,
    /// An array too short for what its family has: the element at this
    /// position isn't there (8.6.3, and each family's clause).
    #[error("a color space has no element {element}")]
    Missing {
        /// Its position in the array, the family's name being at 0.
        element: usize,
    },
    /// A CalGray, CalRGB or Lab dictionary's `/WhitePoint` that isn't three
    /// numbers with the first and last above 0 and the second 1 (8.6.5.2,
    /// Table 62; 8.6.5.3, Table 63; 8.6.5.4, Table 64). Three numbers are held
    /// whatever they are. So is told, past ISO 32000-2, a first or last above 0
    /// that PDFium's 32-bit reading makes 0, as `[99999999999 1 1]`: PDFium loads no
    /// such space, and a space with no fault told counts as it counts
    /// ([`Space`](crate::Space)).
    #[error("a /WhitePoint isn't three numbers, the first and last above 0, the second 1")]
    WhitePoint,
    /// A `/BlackPoint` that isn't three numbers of at least 0 (Tables 62 to
    /// 64). Told only to a sink that wants faults.
    #[error("a /BlackPoint isn't three numbers of at least 0")]
    BlackPoint,
    /// A `/Gamma` that isn't what its family has: a number above 0 for CalGray
    /// (Table 62), three numbers for CalRGB (Table 63). Told only to a sink
    /// that wants faults.
    #[error("a /Gamma isn't what its family has")]
    Gamma,
    /// A CalRGB `/Matrix` that isn't nine numbers (Table 63). Told only to a
    /// sink that wants faults.
    #[error("a /Matrix isn't nine numbers")]
    Matrix,
    /// A Lab `/Range` that isn't four numbers, each pair's first at most its
    /// second (Table 64). Told only to a sink that wants faults.
    #[error("a Lab /Range isn't two pairs of numbers, each in order")]
    LabRange,
    /// An ICCBased space whose second element isn't a stream (8.6.5.5).
    #[error("an ICCBased space's profile isn't a stream")]
    IccNotStream,
    /// A profile stream's `/N` that isn't the integer 1, 3 or 4 (Table 65).
    #[error("a profile stream's /N isn't 1, 3 or 4")]
    IccComponents,
    /// A profile stream's `/Alternate` that's a Pattern space or doesn't have
    /// `/N` components (Table 65). It's held all the same.
    #[error("a profile stream's /Alternate isn't a space it can have")]
    IccAlternate,
    /// A profile stream's `/Range` that isn't two numbers for each of `/N`
    /// components (Table 65). Told only to a sink that wants faults.
    #[error("a profile stream's /Range isn't two numbers for each component")]
    IccRange,
    /// An Indexed space's base that's an Indexed or a Pattern space (8.6.6.3).
    /// It's held all the same.
    #[error("an Indexed space's base is an Indexed or a Pattern space")]
    BaseSpecial,
    /// An Indexed space's high value that isn't an integer from 0 to 255
    /// (8.6.6.3).
    #[error("an Indexed space's high value isn't an integer from 0 to 255")]
    Hival,
    /// An Indexed space's table that's neither a string nor a stream (8.6.6.3).
    #[error("an Indexed space's table is neither a string nor a stream")]
    LookupKind,
    /// An Indexed space's table, a string, that isn't one byte for each
    /// component of the base for each index up to the high value (8.6.6.3). A
    /// stream's data isn't read, and a string a reader couldn't decrypt can't
    /// be told. Told only to a sink that wants faults.
    #[error("an Indexed space's table isn't as long as its base and high value say")]
    LookupLength,
    /// A Separation space's colorant, or an element of a DeviceN space's names,
    /// that isn't a name (8.6.6.4, 8.6.6.5). An element of a DeviceN space's
    /// is counted all the same, among
    /// [`DeviceN::components`](crate::DeviceN::components).
    #[error("a colorant isn't a name")]
    ColorantName,
    /// A Separation or DeviceN space's alternate that's a special space
    /// (8.6.6.4, 8.6.6.5). It's held all the same.
    #[error("an alternate space is a special space")]
    AlternateSpecial,
    /// A tint transform that's neither a dictionary nor a stream (8.6.6.4,
    /// 8.6.6.5; 7.10). Told only to a sink that wants faults.
    #[error("a tint transform isn't a function")]
    TintNotFunction,
    /// A tint transform that doesn't load, by its dictionary alone, or doesn't
    /// take one input for each component and give one output for each of its
    /// alternate's (8.6.6.4, 8.6.6.5). Told only to a sink that wants faults.
    #[error("a tint transform's inputs and outputs aren't its space's")]
    TintShape,
    /// A DeviceN space's names that aren't an array (8.6.6.5).
    #[error("a DeviceN space's names aren't an array")]
    NamesNotArray,
    /// A DeviceN space's names, none of them (8.6.6.5).
    #[error("a DeviceN space has no names")]
    NamesEmpty,
    /// A colorant among a DeviceN space's names a second time, which only
    /// `None` may be (8.6.6.5). Told only to a sink that wants faults.
    #[error("a DeviceN space names a colorant twice")]
    ColorantRepeated,
    /// `All` among a DeviceN space's names (8.6.6.5). Told only to a sink that
    /// wants faults.
    #[error("a DeviceN space names All")]
    AllInDeviceN,
    /// `None` among an NChannel space's names (8.6.6.5, with its erratum 309).
    /// Told only to a sink that wants faults.
    #[error("an NChannel space names None")]
    NoneInNChannel,
    /// An attributes dictionary's `/Subtype` that's neither `DeviceN` nor
    /// `NChannel` (Table 70).
    #[error("an attributes dictionary's /Subtype is neither DeviceN nor NChannel")]
    AttributesSubtype,
    /// An entry of a `/Colorants` dictionary that isn't a Separation space of
    /// the colorant its key names (Table 70). Told only to a sink that wants
    /// faults, or by [`Attributes::separations`](crate::Attributes::separations).
    #[error("an entry of /Colorants isn't a Separation space of its key")]
    ColorantNotSeparation,
    /// A spot colorant of an NChannel space that `/Colorants` has no entry for
    /// (8.6.6.5, Table 70): a name its process dictionary doesn't list, and not
    /// `Cyan`, `Magenta`, `Yellow` or `Black`. Told only to a sink that wants
    /// faults.
    #[error("an NChannel space's spot colorant has no entry in /Colorants")]
    ColorantUndefined,
    /// A process dictionary's `/ColorSpace` that's missing, a special space or
    /// Lab (Table 71). A space that's written is held all the same.
    #[error("a process dictionary's /ColorSpace isn't a space it can have")]
    ProcessSpace,
    /// A process dictionary's `/Components` that's missing or isn't a name for
    /// each of its space's components (Table 71).
    #[error("a process dictionary's /Components isn't a name for each component")]
    ProcessComponents,
    /// A Pattern space whose underlying space is a Pattern space (8.7.3.3). It's
    /// held all the same.
    #[error("a Pattern space's underlying space is a Pattern space")]
    UnderlyingPattern,
    /// A parameter that's no dictionary where its family has one: a CIE-based
    /// space's (8.6.5.1), and a DeviceN space's attributes, `/Colorants`,
    /// `/Process` or `/MixingHints` (Table 70). A stream's dictionary is held
    /// as the parameter of a CIE-based space, as PDFium reads it.
    #[error("a color space's parameter isn't a dictionary")]
    NotADictionary,
    /// A reference to an object that's a reference. No clause forbids it
    /// (7.3.10), and it's read through, but PDFium doesn't follow most of them,
    /// so its reading of the space can part from this one.
    #[error("a reference to an object that's a reference")]
    Chained,
    /// A reference to what's already on the way down to it, a space held in
    /// itself. No clause sets this, as no clause has a space hold a space
    /// that's itself: what's there isn't read.
    #[error("a color space reaches itself")]
    Loop,
    /// Nested deeper than a space is read
    /// ([`Limits::space_depth`](crate::Limits::space_depth)): what's there
    /// isn't read. ISO 32000-2 sets no such cap.
    #[error("a color space nests deeper than it's read")]
    TooDeep,
    /// More looked up than a space is read
    /// ([`Limits::space_visits`](crate::Limits::space_visits)): what's left
    /// isn't read. Told once of what a space holds, once of what its checks
    /// read. ISO 32000-2 sets no such cap.
    #[error("a color space takes more looked up than it's read with")]
    TooManyVisits,
    /// A reference to an object the file lists but whose bytes aren't that
    /// object, or that doesn't lead to it: read as no object, as 7.3.10 has a
    /// reference to nothing, though the file meant one.
    #[error("{id} R doesn't read")]
    Damaged {
        /// The object.
        id: ObjectId,
    },
}
