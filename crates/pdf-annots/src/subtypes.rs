//! Annotation subtypes and typed per-subtype views (ISO 32000-2 §12.5.6).

use std::fmt;

use pdf_gfx::Point;
use pdf_objects::fault::Sink;
use pdf_objects::{Dict, ObjectId, Resolve};
use pdf_semantics::action::Action;
use pdf_semantics::dest::Dest;

use crate::error::Error;
use crate::fault::Fault;
use crate::resolve::{QuadPoints, Reader};
use crate::style::{
    AnnotColor, AppearanceChars, DefaultAppearance, LineEnding, Quadding, read_color, read_mk,
};

/// Annotation `/Subtype` (ISO 32000-2 §12.5.6 Table 171).
///
/// Its `Debug` implementation reports byte lengths for unrecognized [`Self::Other`]
/// names rather than raw file bytes.
#[derive(Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AnnotSubtype {
    /// `/Text`: sticky note (§12.5.6.4).
    Text,
    /// `/Link`: hypertext link (§12.5.6.5).
    Link,
    /// `/FreeText`: text box / callout drawn directly on the page (§12.5.6.6).
    FreeText,
    /// `/Line`: single straight line or dimension arrow (§12.5.6.7).
    Line,
    /// `/Square`: rectangle (§12.5.6.8).
    Square,
    /// `/Circle`: ellipse (§12.5.6.8).
    Circle,
    /// `/Polygon`: closed polygon (§12.5.6.9).
    Polygon,
    /// `/PolyLine`: open polyline (§12.5.6.9).
    PolyLine,
    /// `/Highlight`: text highlight (§12.5.6.10).
    Highlight,
    /// `/Underline`: text underline (§12.5.6.10).
    Underline,
    /// `/Squiggly`: squiggly text underline (§12.5.6.10).
    Squiggly,
    /// `/StrikeOut`: text strikeout (§12.5.6.10).
    StrikeOut,
    /// `/Stamp`: rubber stamp (§12.5.6.12).
    Stamp,
    /// `/Caret`: insertion caret (§12.5.6.11).
    Caret,
    /// `/Ink`: freehand ink strokes (§12.5.6.13).
    Ink,
    /// `/Popup`: pop-up window for markup text editing (§12.5.6.14).
    Popup,
    /// `/FileAttachment`: embedded file attachment (§12.5.6.15).
    FileAttachment,
    /// `/Sound`: sound annotation (§12.5.6.16, deprecated in PDF 2.0).
    Sound,
    /// `/Movie`: movie annotation (§12.5.6.17, deprecated in PDF 2.0).
    Movie,
    /// `/Widget`: interactive form widget (§12.5.6.19).
    Widget,
    /// `/Screen`: screen multimedia region (§12.5.6.18).
    Screen,
    /// `/PrinterMark`: printer's registration/color mark (§12.5.6.20).
    PrinterMark,
    /// `/TrapNet`: trapping network (§12.5.6.21, deprecated in PDF 2.0).
    TrapNet,
    /// `/Watermark`: fixed-size/fixed-position watermark (§12.5.6.22).
    Watermark,
    /// `/3D`: 3D artwork (§13.6).
    ThreeD,
    /// `/RichMedia`: rich media annotation (§13.7).
    RichMedia,
    /// `/XFAWidget`: XFA form widget recognized by PDFium.
    XfaWidget,
    /// `/Redact`: redaction mark (§12.5.6.23).
    Redact,
    /// `/Projection`: geospatial/measurement projection (PDF 2.0 §12.5.6.24).
    Projection,
    /// Missing or unrecognized `/Subtype`.
    Other(Box<[u8]>),
}

impl fmt::Debug for AnnotSubtype {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text => f.write_str("Text"),
            Self::Link => f.write_str("Link"),
            Self::FreeText => f.write_str("FreeText"),
            Self::Line => f.write_str("Line"),
            Self::Square => f.write_str("Square"),
            Self::Circle => f.write_str("Circle"),
            Self::Polygon => f.write_str("Polygon"),
            Self::PolyLine => f.write_str("PolyLine"),
            Self::Highlight => f.write_str("Highlight"),
            Self::Underline => f.write_str("Underline"),
            Self::Squiggly => f.write_str("Squiggly"),
            Self::StrikeOut => f.write_str("StrikeOut"),
            Self::Stamp => f.write_str("Stamp"),
            Self::Caret => f.write_str("Caret"),
            Self::Ink => f.write_str("Ink"),
            Self::Popup => f.write_str("Popup"),
            Self::FileAttachment => f.write_str("FileAttachment"),
            Self::Sound => f.write_str("Sound"),
            Self::Movie => f.write_str("Movie"),
            Self::Widget => f.write_str("Widget"),
            Self::Screen => f.write_str("Screen"),
            Self::PrinterMark => f.write_str("PrinterMark"),
            Self::TrapNet => f.write_str("TrapNet"),
            Self::Watermark => f.write_str("Watermark"),
            Self::ThreeD => f.write_str("ThreeD"),
            Self::RichMedia => f.write_str("RichMedia"),
            Self::XfaWidget => f.write_str("XfaWidget"),
            Self::Redact => f.write_str("Redact"),
            Self::Projection => f.write_str("Projection"),
            Self::Other(b) => f.debug_struct("Other").field("name_len", &b.len()).finish(),
        }
    }
}

impl AnnotSubtype {
    /// Parses a `/Subtype` name byte slice.
    #[must_use]
    pub fn from_name(name: &[u8]) -> Self {
        match name {
            b"Text" => Self::Text,
            b"Link" => Self::Link,
            b"FreeText" => Self::FreeText,
            b"Line" => Self::Line,
            b"Square" => Self::Square,
            b"Circle" => Self::Circle,
            b"Polygon" => Self::Polygon,
            b"PolyLine" => Self::PolyLine,
            b"Highlight" => Self::Highlight,
            b"Underline" => Self::Underline,
            b"Squiggly" => Self::Squiggly,
            b"StrikeOut" => Self::StrikeOut,
            b"Stamp" => Self::Stamp,
            b"Caret" => Self::Caret,
            b"Ink" => Self::Ink,
            b"Popup" => Self::Popup,
            b"FileAttachment" => Self::FileAttachment,
            b"Sound" => Self::Sound,
            b"Movie" => Self::Movie,
            b"Widget" => Self::Widget,
            b"Screen" => Self::Screen,
            b"PrinterMark" => Self::PrinterMark,
            b"TrapNet" => Self::TrapNet,
            b"Watermark" => Self::Watermark,
            b"3D" => Self::ThreeD,
            b"RichMedia" => Self::RichMedia,
            b"XFAWidget" => Self::XfaWidget,
            b"Redact" => Self::Redact,
            b"Projection" => Self::Projection,
            other => Self::Other(Box::from(other)),
        }
    }

    /// Raw PDF `/Subtype` name bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Text => b"Text",
            Self::Link => b"Link",
            Self::FreeText => b"FreeText",
            Self::Line => b"Line",
            Self::Square => b"Square",
            Self::Circle => b"Circle",
            Self::Polygon => b"Polygon",
            Self::PolyLine => b"PolyLine",
            Self::Highlight => b"Highlight",
            Self::Underline => b"Underline",
            Self::Squiggly => b"Squiggly",
            Self::StrikeOut => b"StrikeOut",
            Self::Stamp => b"Stamp",
            Self::Caret => b"Caret",
            Self::Ink => b"Ink",
            Self::Popup => b"Popup",
            Self::FileAttachment => b"FileAttachment",
            Self::Sound => b"Sound",
            Self::Movie => b"Movie",
            Self::Widget => b"Widget",
            Self::Screen => b"Screen",
            Self::PrinterMark => b"PrinterMark",
            Self::TrapNet => b"TrapNet",
            Self::Watermark => b"Watermark",
            Self::ThreeD => b"3D",
            Self::RichMedia => b"RichMedia",
            Self::XfaWidget => b"XFAWidget",
            Self::Redact => b"Redact",
            Self::Projection => b"Projection",
            Self::Other(b) => b,
        }
    }

    /// Returns `true` if this is a standard ISO 32000-2 annotation subtype.
    #[must_use]
    pub const fn is_standard(&self) -> bool {
        !matches!(self, Self::Other(_) | Self::XfaWidget)
    }

    /// Returns `true` if this subtype is a markup annotation (ISO 32000-2
    /// §12.5.6.2 Table 171).
    #[must_use]
    pub const fn is_markup(&self) -> bool {
        matches!(
            self,
            Self::Text
                | Self::FreeText
                | Self::Line
                | Self::Square
                | Self::Circle
                | Self::Polygon
                | Self::PolyLine
                | Self::Highlight
                | Self::Underline
                | Self::Squiggly
                | Self::StrikeOut
                | Self::Stamp
                | Self::Caret
                | Self::Ink
                | Self::FileAttachment
                | Self::Sound
                | Self::Redact
                | Self::Projection
        )
    }

    /// Returns PDFium's integer `FPDF_ANNOT_*` code (`0` for `Projection` and
    /// `Other`).
    #[must_use]
    pub const fn pdfium_code(&self) -> u32 {
        match self {
            Self::Text => 1,
            Self::Link => 2,
            Self::FreeText => 3,
            Self::Line => 4,
            Self::Square => 5,
            Self::Circle => 6,
            Self::Polygon => 7,
            Self::PolyLine => 8,
            Self::Highlight => 9,
            Self::Underline => 10,
            Self::Squiggly => 11,
            Self::StrikeOut => 12,
            Self::Stamp => 13,
            Self::Caret => 14,
            Self::Ink => 15,
            Self::Popup => 16,
            Self::FileAttachment => 17,
            Self::Sound => 18,
            Self::Movie => 19,
            Self::Widget => 20,
            Self::Screen => 21,
            Self::PrinterMark => 22,
            Self::TrapNet => 23,
            Self::Watermark => 24,
            Self::ThreeD => 25,
            Self::RichMedia => 26,
            Self::XfaWidget => 27,
            Self::Redact => 28,
            Self::Projection | Self::Other(_) => 0,
        }
    }
}

/// Sticky-note icon name (`/Name` on `/Subtype /Text`, ISO 32000-2 §12.5.6.4
/// Table 175).
#[derive(Clone, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TextIcon {
    /// `/Note` (default).
    #[default]
    Note,
    /// `/Comment`.
    Comment,
    /// `/Key`.
    Key,
    /// `/Help`.
    Help,
    /// `/NewParagraph`.
    NewParagraph,
    /// `/Paragraph`.
    Paragraph,
    /// `/Insert`.
    Insert,
    /// Custom icon name.
    Other(Box<[u8]>),
}

impl fmt::Debug for TextIcon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Note => f.write_str("Note"),
            Self::Comment => f.write_str("Comment"),
            Self::Key => f.write_str("Key"),
            Self::Help => f.write_str("Help"),
            Self::NewParagraph => f.write_str("NewParagraph"),
            Self::Paragraph => f.write_str("Paragraph"),
            Self::Insert => f.write_str("Insert"),
            Self::Other(b) => f.debug_struct("Other").field("name_len", &b.len()).finish(),
        }
    }
}

impl TextIcon {
    /// Parses a `/Text` icon `/Name`.
    #[must_use]
    pub fn from_name(name: &[u8]) -> Self {
        match name {
            b"Note" => Self::Note,
            b"Comment" => Self::Comment,
            b"Key" => Self::Key,
            b"Help" => Self::Help,
            b"NewParagraph" => Self::NewParagraph,
            b"Paragraph" => Self::Paragraph,
            b"Insert" => Self::Insert,
            other => Self::Other(Box::from(other)),
        }
    }

    /// Raw icon name bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Note => b"Note",
            Self::Comment => b"Comment",
            Self::Key => b"Key",
            Self::Help => b"Help",
            Self::NewParagraph => b"NewParagraph",
            Self::Paragraph => b"Paragraph",
            Self::Insert => b"Insert",
            Self::Other(b) => b,
        }
    }
}

/// Sticky-note annotation details (`/Subtype /Text`, ISO 32000-2 §12.5.6.4).
///
/// Its `Debug` implementation reports byte lengths rather than raw state
/// strings.
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TextAnnot {
    /// `/Open`: whether the note's pop-up window should open initially (`false`
    /// by default).
    pub open: bool,
    /// `/Name`: icon name (`Note` by default).
    pub icon: TextIcon,
    /// `/StateModel`: `"Marked"` or `"Review"`, if this note records a state
    /// change on its `/IRT` target.
    pub state_model: Option<String>,
    /// `/State`: state value (e.g. `"Accepted"`, `"Marked"`), if present.
    pub state: Option<String>,
}

impl fmt::Debug for TextAnnot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextAnnot")
            .field("open", &self.open)
            .field("icon", &self.icon)
            .field(
                "state_model_len",
                &self.state_model.as_ref().map(String::len),
            )
            .field("state_len", &self.state.as_ref().map(String::len))
            .finish()
    }
}

/// Visual feedback mode when clicking a `/Link` or `/Widget` (`/H`, ISO 32000-2
/// §12.5.6.5 Table 176).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HighlightMode {
    /// `/N`: no visual effect.
    None,
    /// `/I`: invert the interior of the annotation rectangle (default).
    #[default]
    Invert,
    /// `/O`: invert the annotation's border.
    Outline,
    /// `/P` (or `/T` on widgets): depress/push appearance (`/AP /D`).
    Push,
}

impl HighlightMode {
    #[must_use]
    pub(crate) fn from_name(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"N" => Self::None,
            b"I" => Self::Invert,
            b"O" => Self::Outline,
            b"P" | b"T" => Self::Push,
            _ => return None,
        })
    }
}

/// Link annotation details (`/Subtype /Link`, ISO 32000-2 §12.5.6.5).
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct LinkAnnot {
    /// `/Dest` direct/named destination, if present.
    pub dest: Option<Dest>,
    /// `/A` action chain, if present.
    pub actions: Vec<Action>,
    /// `/H` click highlight mode (`Invert` by default).
    pub highlight: HighlightMode,
    /// `/QuadPoints` active link regions, if present.
    pub quad_points: Vec<QuadPoints>,
}

impl LinkAnnot {
    /// Returns the effective destination of this link—preferring `/Dest` when
    /// present, or `/A`'s first `Action::GoTo` destination (matching PDFium's
    /// `FPDFLink_GetDest`).
    #[must_use]
    pub fn effective_dest(&self) -> Option<&Dest> {
        if let Some(d) = &self.dest {
            return Some(d);
        }
        match self.actions.first() {
            Some(Action::GoTo { dest: Some(d) }) => Some(d),
            _ => None,
        }
    }
}

/// Intent of a `/FreeText` annotation (`/IT`, ISO 32000-2 §12.5.6.6 Table 177).
#[derive(Clone, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FreeTextIntent {
    /// `/FreeText`: plain text box (default).
    #[default]
    FreeText,
    /// `/FreeTextCallout`: callout text box with `/CL` leader line.
    Callout,
    /// `/FreeTextTypeWriter`: click-to-type typewriter text with no border/box.
    TypeWriter,
    /// Unrecognized intent name.
    Other(Box<[u8]>),
}

impl fmt::Debug for FreeTextIntent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FreeText => f.write_str("FreeText"),
            Self::Callout => f.write_str("Callout"),
            Self::TypeWriter => f.write_str("TypeWriter"),
            Self::Other(b) => f.debug_struct("Other").field("name_len", &b.len()).finish(),
        }
    }
}

/// FreeText annotation details (`/Subtype /FreeText`, ISO 32000-2 §12.5.6.6).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct FreeTextAnnot {
    /// Parsed `/DA` default appearance, if present.
    pub da: Option<DefaultAppearance>,
    /// `/Q` horizontal justification (`Left` by default).
    pub quadding: Quadding,
    /// `/IT` intent (`FreeText` by default).
    pub intent: FreeTextIntent,
    /// `/CL` callout line points (`2` points `[start, end]` or `3` points
    /// `[start, knee, end]`), if present.
    pub callout_line: Vec<Point>,
    /// `/LE` callout start line-ending style.
    pub line_ending: LineEnding,
    /// `/RD` inner text-box inset `[left, bottom, right, top]` inside `/Rect`.
    pub rect_diff: Option<[f32; 4]>,
}

/// Which text-markup subtype an annotation represents (ISO 32000-2 §12.5.6.10).
///
/// Exhaustive on purpose: §12.5.6.10 defines exactly these four text-markup
/// subtypes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextMarkupKind {
    /// `/Highlight`.
    Highlight,
    /// `/Underline`.
    Underline,
    /// `/Squiggly`.
    Squiggly,
    /// `/StrikeOut`.
    StrikeOut,
}

impl TextMarkupKind {
    /// Corresponding [`AnnotSubtype`].
    #[must_use]
    pub const fn subtype(self) -> AnnotSubtype {
        match self {
            Self::Highlight => AnnotSubtype::Highlight,
            Self::Underline => AnnotSubtype::Underline,
            Self::Squiggly => AnnotSubtype::Squiggly,
            Self::StrikeOut => AnnotSubtype::StrikeOut,
        }
    }
}

/// Highlight, Underline, Squiggly, or StrikeOut annotation details
/// (ISO 32000-2 §12.5.6.10).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct TextMarkupAnnot {
    /// Which of the four text-markup subtypes this is.
    pub kind: TextMarkupKind,
    /// `/QuadPoints` quadrilaterals enclosing the marked text.
    pub quad_points: Vec<QuadPoints>,
}

/// Straight-line annotation details (`/Subtype /Line`, ISO 32000-2 §12.5.6.7).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LineAnnot {
    /// Start and end points `((x1, y1), (x2, y2))` from `/L`, if valid.
    pub endpoints: Option<(Point, Point)>,
    /// `/LE` start and end line-ending styles (`(None, None)` by default).
    pub endings: (LineEnding, LineEnding),
    /// `/IC` interior color used to fill closed line endings.
    pub interior_color: Option<AnnotColor>,
    /// `/LL` leader-line length (`0.0` by default).
    pub leader_length: f32,
    /// `/LLE` leader-line extension (`0.0` by default).
    pub leader_extension: f32,
    /// `/LLO` leader-line offset (`0.0` by default).
    pub leader_offset: f32,
    /// `/Cap`: whether `/Contents` should be rendered as a caption on the line.
    pub caption: bool,
}

/// Geometric shape kind (`/Square` or `/Circle`, ISO 32000-2 §12.5.6.8).
///
/// Exhaustive on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShapeKind {
    /// `/Square`: axis-aligned rectangle.
    Square,
    /// `/Circle`: axis-aligned ellipse inscribed in the inner rectangle.
    Circle,
}

/// Square or Circle annotation details (ISO 32000-2 §12.5.6.8).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ShapeAnnot {
    /// Rectangle vs. ellipse.
    pub kind: ShapeKind,
    /// `/IC` interior fill color.
    pub interior_color: Option<AnnotColor>,
    /// `/RD` padding inset `[left, bottom, right, top]` between `/Rect` and the
    /// drawn shape.
    pub rect_diff: Option<[f32; 4]>,
}

/// Polygon vs. PolyLine kind (ISO 32000-2 §12.5.6.9).
///
/// Exhaustive on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PolyKind {
    /// `/Polygon`: closed polygon.
    Polygon,
    /// `/PolyLine`: open polyline.
    PolyLine,
}

/// Polygon or PolyLine annotation details (ISO 32000-2 §12.5.6.9).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct PolyAnnot {
    /// Closed polygon vs. open polyline.
    pub kind: PolyKind,
    /// `/Vertices` points in PageSpace.
    pub vertices: Vec<Point>,
    /// `/LE` start and end line endings (meaningful for `PolyLine`).
    pub endings: (LineEnding, LineEnding),
    /// `/IC` interior fill color.
    pub interior_color: Option<AnnotColor>,
}

/// Freehand ink annotation details (`/Subtype /Ink`, ISO 32000-2 §12.5.6.13).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct InkAnnot {
    /// `/InkList`: disjoint freehand strokes in PageSpace.
    pub strokes: Vec<Vec<Point>>,
}

/// Standard rubber-stamp names (`/Name` on `/Subtype /Stamp`, ISO 32000-2
/// §12.5.6.12 Table 184).
#[derive(Clone, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StampName {
    /// `/Draft` (default).
    #[default]
    Draft,
    /// `/Approved`.
    Approved,
    /// `/Experimental`.
    Experimental,
    /// `/NotApproved`.
    NotApproved,
    /// `/AsIs`.
    AsIs,
    /// `/Expired`.
    Expired,
    /// `/NotForPublicRelease`.
    NotForPublicRelease,
    /// `/Confidential`.
    Confidential,
    /// `/Final`.
    Final,
    /// `/Sold`.
    Sold,
    /// `/Departmental`.
    Departmental,
    /// `/ForComment`.
    ForComment,
    /// `/TopSecret`.
    TopSecret,
    /// `/ForPublicRelease`.
    ForPublicRelease,
    /// Custom stamp name.
    Other(Box<[u8]>),
}

impl fmt::Debug for StampName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Draft => f.write_str("Draft"),
            Self::Approved => f.write_str("Approved"),
            Self::Experimental => f.write_str("Experimental"),
            Self::NotApproved => f.write_str("NotApproved"),
            Self::AsIs => f.write_str("AsIs"),
            Self::Expired => f.write_str("Expired"),
            Self::NotForPublicRelease => f.write_str("NotForPublicRelease"),
            Self::Confidential => f.write_str("Confidential"),
            Self::Final => f.write_str("Final"),
            Self::Sold => f.write_str("Sold"),
            Self::Departmental => f.write_str("Departmental"),
            Self::ForComment => f.write_str("ForComment"),
            Self::TopSecret => f.write_str("TopSecret"),
            Self::ForPublicRelease => f.write_str("ForPublicRelease"),
            Self::Other(b) => f.debug_struct("Other").field("name_len", &b.len()).finish(),
        }
    }
}

impl StampName {
    /// Parses a `/Stamp` `/Name`.
    #[must_use]
    pub fn from_name(name: &[u8]) -> Self {
        match name {
            b"Draft" => Self::Draft,
            b"Approved" => Self::Approved,
            b"Experimental" => Self::Experimental,
            b"NotApproved" => Self::NotApproved,
            b"AsIs" => Self::AsIs,
            b"Expired" => Self::Expired,
            b"NotForPublicRelease" => Self::NotForPublicRelease,
            b"Confidential" => Self::Confidential,
            b"Final" => Self::Final,
            b"Sold" => Self::Sold,
            b"Departmental" => Self::Departmental,
            b"ForComment" => Self::ForComment,
            b"TopSecret" => Self::TopSecret,
            b"ForPublicRelease" => Self::ForPublicRelease,
            other => Self::Other(Box::from(other)),
        }
    }

    /// Raw stamp `/Name` bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Draft => b"Draft",
            Self::Approved => b"Approved",
            Self::Experimental => b"Experimental",
            Self::NotApproved => b"NotApproved",
            Self::AsIs => b"AsIs",
            Self::Expired => b"Expired",
            Self::NotForPublicRelease => b"NotForPublicRelease",
            Self::Confidential => b"Confidential",
            Self::Final => b"Final",
            Self::Sold => b"Sold",
            Self::Departmental => b"Departmental",
            Self::ForComment => b"ForComment",
            Self::TopSecret => b"TopSecret",
            Self::ForPublicRelease => b"ForPublicRelease",
            Self::Other(b) => b,
        }
    }

    /// Default display label for standard stamps (e.g. `"NOT APPROVED"`).
    #[must_use]
    pub const fn display_label(&self) -> Option<&'static str> {
        Some(match self {
            Self::Draft => "DRAFT",
            Self::Approved => "APPROVED",
            Self::Experimental => "EXPERIMENTAL",
            Self::NotApproved => "NOT APPROVED",
            Self::AsIs => "AS IS",
            Self::Expired => "EXPIRED",
            Self::NotForPublicRelease => "NOT FOR PUBLIC RELEASE",
            Self::Confidential => "CONFIDENTIAL",
            Self::Final => "FINAL",
            Self::Sold => "SOLD",
            Self::Departmental => "DEPARTMENTAL",
            Self::ForComment => "FOR COMMENT",
            Self::TopSecret => "TOP SECRET",
            Self::ForPublicRelease => "FOR PUBLIC RELEASE",
            Self::Other(_) => return None,
        })
    }
}

/// Rubber-stamp annotation details (`/Subtype /Stamp`, ISO 32000-2 §12.5.6.12).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct StampAnnot {
    /// `/Name` stamp identifier (`Draft` by default).
    pub name: StampName,
}

/// Caret annotation symbol (`/Sy` on `/Subtype /Caret`, ISO 32000-2 §12.5.6.11).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CaretSymbol {
    /// `/None`: no extra symbol.
    #[default]
    None,
    /// `/P`: new-paragraph pilcrow (`¶`) symbol associated with the caret.
    Paragraph,
}

/// Caret annotation details (`/Subtype /Caret`, ISO 32000-2 §12.5.6.11).
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct CaretAnnot {
    /// `/Sy` symbol (`None` by default).
    pub symbol: CaretSymbol,
    /// `/RD` inset rectangle differences, if present.
    pub rect_diff: Option<[f32; 4]>,
}

/// Pop-up window annotation details (`/Subtype /Popup`, ISO 32000-2 §12.5.6.14).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PopupAnnot {
    /// `/Parent` indirect annotation object ID that owns this pop-up, if any.
    pub parent: Option<ObjectId>,
    /// `/Open`: whether the pop-up window is initially open (`false` by
    /// default).
    pub open: bool,
}

/// File-attachment icon (`/Name` on `/Subtype /FileAttachment`, ISO 32000-2
/// §12.5.6.15 Table 187).
#[derive(Clone, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AttachmentIcon {
    /// `/PushPin` (default).
    #[default]
    PushPin,
    /// `/Graph`.
    Graph,
    /// `/Paperclip`.
    Paperclip,
    /// `/Tag`.
    Tag,
    /// Custom icon name.
    Other(Box<[u8]>),
}

impl fmt::Debug for AttachmentIcon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PushPin => f.write_str("PushPin"),
            Self::Graph => f.write_str("Graph"),
            Self::Paperclip => f.write_str("Paperclip"),
            Self::Tag => f.write_str("Tag"),
            Self::Other(b) => f.debug_struct("Other").field("name_len", &b.len()).finish(),
        }
    }
}

impl AttachmentIcon {
    /// Parses a `/FileAttachment` `/Name`.
    #[must_use]
    pub fn from_name(name: &[u8]) -> Self {
        match name {
            b"PushPin" => Self::PushPin,
            b"Graph" => Self::Graph,
            b"Paperclip" => Self::Paperclip,
            b"Tag" => Self::Tag,
            other => Self::Other(Box::from(other)),
        }
    }
}

/// File-attachment annotation details (`/Subtype /FileAttachment`, ISO 32000-2
/// §12.5.6.15).
///
/// Its `Debug` implementation reports `file_name`'s byte length rather than the
/// filename string.
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileAttachmentAnnot {
    /// File name extracted from `/FS` (`/UF` or `/F`), if present.
    pub file_name: Option<String>,
    /// Indirect `ObjectId` of the `/FS` file specification dictionary, if
    /// indirect.
    pub fs_id: Option<ObjectId>,
    /// `/Name` icon (`PushPin` by default).
    pub icon: AttachmentIcon,
}

impl fmt::Debug for FileAttachmentAnnot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileAttachmentAnnot")
            .field("file_name_len", &self.file_name.as_ref().map(String::len))
            .field("fs_id", &self.fs_id)
            .field("icon", &self.icon)
            .finish()
    }
}

/// Redaction mark annotation details (`/Subtype /Redact`, ISO 32000-2
/// §12.5.6.23).
///
/// Its `Debug` implementation omits `overlay_text` content.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub struct RedactAnnot {
    /// `/QuadPoints` sub-regions within `/Rect` to redact (when empty, the
    /// entire `/Rect` is marked for redaction).
    pub quad_points: Vec<QuadPoints>,
    /// `/IC` interior fill color painted over the redacted region after
    /// removal.
    pub interior_color: Option<AnnotColor>,
    /// `/RO` Form XObject stream overlaid on the redacted area after removal,
    /// if present.
    pub overlay_form: Option<ObjectId>,
    /// `/OverlayText` label drawn over the redacted region after removal, if
    /// present.
    pub overlay_text: Option<String>,
    /// `/Repeat`: whether `overlay_text` repeats to fill the redacted box.
    pub repeat: bool,
    /// Parsed `/DA` appearance for `overlay_text`, if present.
    pub da: Option<DefaultAppearance>,
    /// `/Q` justification for `overlay_text` (`Left` by default).
    pub quadding: Quadding,
}

impl fmt::Debug for RedactAnnot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedactAnnot")
            .field("quad_points", &self.quad_points.len())
            .field("interior_color", &self.interior_color)
            .field("overlay_form", &self.overlay_form)
            .field(
                "overlay_text_len",
                &self.overlay_text.as_ref().map(String::len),
            )
            .field("repeat", &self.repeat)
            .field("da", &self.da)
            .field("quadding", &self.quadding)
            .finish()
    }
}

/// Surface-level form widget annotation details (`/Subtype /Widget`, ISO
/// 32000-2 §12.5.6.19).
///
/// Full `/AcroForm` field tree inheritance and value handling live in the
/// higher-level form layer, which reuses [`Annot`](crate::Annot) directly on
/// standalone and merged Field+Widget dictionaries.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct WidgetAnnot {
    /// `/H` highlight mode (`Invert` by default).
    pub highlight: HighlightMode,
    /// `/MK` appearance characteristics, if present.
    pub appearance_chars: Option<AppearanceChars>,
    /// `/Parent` parent field `ObjectId`, when the widget is separate from its
    /// field node.
    pub parent: Option<ObjectId>,
    /// Direct `/DA` default appearance on this widget/merged dict, if present.
    pub da: Option<DefaultAppearance>,
    /// Direct `/Q` quadding on this widget/merged dict, if present.
    pub quadding: Option<Quadding>,
    /// `/A` activation action chain, if present.
    pub actions: Vec<Action>,
}

/// Subtype-specific decoded payload of an [`Annot`](crate::Annot).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum TypedAnnot {
    /// `/Text` sticky note.
    Text(TextAnnot),
    /// `/Link` hypertext link.
    Link(LinkAnnot),
    /// `/FreeText` text box / callout.
    FreeText(FreeTextAnnot),
    /// `/Highlight`, `/Underline`, `/Squiggly`, or `/StrikeOut`.
    TextMarkup(TextMarkupAnnot),
    /// `/Line` straight line / dimension.
    Line(LineAnnot),
    /// `/Square` or `/Circle`.
    Shape(ShapeAnnot),
    /// `/Polygon` or `/PolyLine`.
    Poly(PolyAnnot),
    /// `/Ink` freehand strokes.
    Ink(InkAnnot),
    /// `/Stamp` rubber stamp.
    Stamp(StampAnnot),
    /// `/Caret` insertion mark.
    Caret(CaretAnnot),
    /// `/Popup` pop-up window.
    Popup(PopupAnnot),
    /// `/FileAttachment` embedded file reference.
    FileAttachment(FileAttachmentAnnot),
    /// `/Redact` redaction mark.
    Redact(RedactAnnot),
    /// `/Widget` interactive form widget.
    Widget(WidgetAnnot),
    /// Any other annotation subtype (`Sound`, `Movie`, `Screen`, `Watermark`,
    /// `PrinterMark`, `TrapNet`, `3D`, `RichMedia`, `Projection`, or custom).
    Other(AnnotSubtype),
}

fn read_line_endings<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    dict: &'o Dict,
    within: Option<ObjectId>,
) -> Result<(LineEnding, LineEnding), Error> {
    let Some(raw) = dict.get(b"LE") else {
        return Ok((LineEnding::None, LineEnding::None));
    };
    let Some((obj, id)) = reader.reached(raw, within)? else {
        return Ok((LineEnding::None, LineEnding::None));
    };
    if let Some(name) = obj.as_name() {
        let le = LineEnding::from_name(name).unwrap_or(LineEnding::None);
        return Ok((le, LineEnding::None));
    }
    let Some(arr) = obj.as_array() else {
        reader.say(id, obj.at(), Fault::WrongType { key: "LE" });
        return Ok((LineEnding::None, LineEnding::None));
    };
    let mut pick = |idx: usize| -> Result<LineEnding, Error> {
        let Some(item) = arr.get(idx) else {
            return Ok(LineEnding::None);
        };
        let Some((o, oid)) = reader.reached(item, id)? else {
            return Ok(LineEnding::None);
        };
        let Some(n) = o.as_name() else {
            reader.say(oid, o.at(), Fault::WrongType { key: "LE" });
            return Ok(LineEnding::None);
        };
        Ok(LineEnding::from_name(n).unwrap_or(LineEnding::None))
    };
    Ok((pick(0)?, pick(1)?))
}

fn read_da<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    dict: &'o Dict,
    within: Option<ObjectId>,
) -> Result<Option<DefaultAppearance>, Error> {
    let Some(da_bytes) = reader.byte_string_entry(dict, within, "DA")? else {
        return Ok(None);
    };
    match DefaultAppearance::parse(da_bytes, reader.limits) {
        Some(da) => {
            if da.font_name.is_none() {
                reader.say(within, None, Fault::InvalidDefaultAppearance);
            }
            Ok(Some(da))
        }
        None => {
            reader.say(within, None, Fault::InvalidDefaultAppearance);
            Ok(None)
        }
    }
}

pub(crate) fn read_typed<'o, R: Resolve<'o>, S: Sink<Fault> + ?Sized>(
    reader: &mut Reader<'_, R, S>,
    subtype: &AnnotSubtype,
    dict: &'o Dict,
    within: Option<ObjectId>,
) -> Result<TypedAnnot, Error> {
    match subtype {
        AnnotSubtype::Text => {
            let open = reader.bool_entry(dict, within, "Open")?.unwrap_or(false);
            let icon = reader
                .name_entry(dict, within, "Name")?
                .map_or(TextIcon::Note, TextIcon::from_name);
            let state_model = reader.text_or_name_entry(dict, within, "StateModel")?;
            let state = reader.text_or_name_entry(dict, within, "State")?;
            Ok(TypedAnnot::Text(TextAnnot {
                open,
                icon,
                state_model,
                state,
            }))
        }
        AnnotSubtype::Link => {
            let has_dest = dict.get(b"Dest").is_some();
            let has_a = dict.get(b"A").is_some();
            if has_dest && has_a {
                reader.say(within, None, Fault::LinkHasBothDestAndAction);
            }
            let dest = match dict.get(b"Dest") {
                Some(raw) => reader.dest(raw, within, true)?,
                None => None,
            };
            let actions = match dict.get(b"A") {
                Some(raw) => reader.action_chain(raw, within)?,
                None => Vec::new(),
            };
            let highlight = reader
                .name_entry(dict, within, "H")?
                .and_then(HighlightMode::from_name)
                .unwrap_or_default();
            let quad_points = reader.quad_points(dict, within)?.unwrap_or_default();
            Ok(TypedAnnot::Link(LinkAnnot {
                dest,
                actions,
                highlight,
                quad_points,
            }))
        }
        AnnotSubtype::FreeText => {
            let da = read_da(reader, dict, within)?;
            let quadding = reader
                .int_entry(dict, within, "Q")?
                .map_or(Quadding::Left, Quadding::from_pdf_int);
            let intent = match reader.name_entry(dict, within, "IT")? {
                None | Some(b"FreeText") => FreeTextIntent::FreeText,
                Some(b"FreeTextCallout") => FreeTextIntent::Callout,
                Some(b"FreeTextTypeWriter" | b"FreeTextTypewriter") => FreeTextIntent::TypeWriter,
                Some(other) => FreeTextIntent::Other(Box::from(other)),
            };
            let mut budget = reader.limits.points;
            let callout_line = match reader.array_entry(dict, within, "CL")? {
                Some((arr, cl_id)) => {
                    if arr.len() != 4 && arr.len() != 6 {
                        reader.say(cl_id, None, Fault::InvalidPoints { key: "CL" });
                    }
                    reader
                        .point_pairs(arr, cl_id, None, "CL", &mut budget)?
                        .unwrap_or_default()
                }
                None => Vec::new(),
            };
            let (line_ending, _) = read_line_endings(reader, dict, within)?;
            let rect_diff = reader.rect4(dict, within, "RD")?.map(|(raw, _)| raw);
            Ok(TypedAnnot::FreeText(FreeTextAnnot {
                da,
                quadding,
                intent,
                callout_line,
                line_ending,
                rect_diff,
            }))
        }
        AnnotSubtype::Highlight
        | AnnotSubtype::Underline
        | AnnotSubtype::Squiggly
        | AnnotSubtype::StrikeOut => {
            let kind = match subtype {
                AnnotSubtype::Highlight => TextMarkupKind::Highlight,
                AnnotSubtype::Underline => TextMarkupKind::Underline,
                AnnotSubtype::Squiggly => TextMarkupKind::Squiggly,
                _ => TextMarkupKind::StrikeOut,
            };
            let quad_points = match reader.quad_points(dict, within)? {
                Some(qp) => qp,
                None => {
                    reader.say(within, None, Fault::InvalidQuadPoints);
                    Vec::new()
                }
            };
            Ok(TypedAnnot::TextMarkup(TextMarkupAnnot {
                kind,
                quad_points,
            }))
        }
        AnnotSubtype::Line => {
            let endpoints = if let Some((raw, _)) = reader.rect4(dict, within, "L")? {
                Some((Point::new(raw[0], raw[1]), Point::new(raw[2], raw[3])))
            } else {
                reader.say(within, None, Fault::InvalidPoints { key: "L" });
                None
            };
            let endings = read_line_endings(reader, dict, within)?;
            let interior_color = read_color(reader, dict, within, "IC")?;
            let leader_length = reader.float_entry(dict, within, "LL")?.unwrap_or(0.0);
            let leader_extension = reader.float_entry(dict, within, "LLE")?.unwrap_or(0.0);
            let leader_offset = reader.float_entry(dict, within, "LLO")?.unwrap_or(0.0);
            let caption = reader.bool_entry(dict, within, "Cap")?.unwrap_or(false);
            Ok(TypedAnnot::Line(LineAnnot {
                endpoints,
                endings,
                interior_color,
                leader_length,
                leader_extension,
                leader_offset,
                caption,
            }))
        }
        AnnotSubtype::Square | AnnotSubtype::Circle => {
            let kind = if *subtype == AnnotSubtype::Square {
                ShapeKind::Square
            } else {
                ShapeKind::Circle
            };
            let interior_color = read_color(reader, dict, within, "IC")?;
            let rect_diff = reader.rect4(dict, within, "RD")?.map(|(raw, _)| raw);
            Ok(TypedAnnot::Shape(ShapeAnnot {
                kind,
                interior_color,
                rect_diff,
            }))
        }
        AnnotSubtype::Polygon | AnnotSubtype::PolyLine => {
            let kind = if *subtype == AnnotSubtype::Polygon {
                PolyKind::Polygon
            } else {
                PolyKind::PolyLine
            };
            let mut budget = reader.limits.points;
            let vertices = match reader.array_entry(dict, within, "Vertices")? {
                Some((arr, v_id)) => reader
                    .point_pairs(arr, v_id, None, "Vertices", &mut budget)?
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            let endings = read_line_endings(reader, dict, within)?;
            let interior_color = read_color(reader, dict, within, "IC")?;
            Ok(TypedAnnot::Poly(PolyAnnot {
                kind,
                vertices,
                endings,
                interior_color,
            }))
        }
        AnnotSubtype::Ink => {
            let mut budget = reader.limits.points;
            let mut strokes = Vec::new();
            match reader.array_entry(dict, within, "InkList")? {
                Some((outer, outer_id)) => {
                    for sub in outer {
                        let Some((sub_obj, sub_id)) = reader.reached(sub, outer_id)? else {
                            continue;
                        };
                        let Some(arr) = sub_obj.as_array() else {
                            reader.say(
                                sub_id,
                                sub_obj.at(),
                                Fault::InvalidPoints { key: "InkList" },
                            );
                            continue;
                        };
                        if let Some(pts) =
                            reader.point_pairs(arr, sub_id, sub_obj.at(), "InkList", &mut budget)?
                        {
                            strokes.push(pts);
                        }
                    }
                }
                None => {
                    reader.say(within, None, Fault::InvalidPoints { key: "InkList" });
                }
            }
            Ok(TypedAnnot::Ink(InkAnnot { strokes }))
        }
        AnnotSubtype::Stamp => {
            let name = reader
                .name_entry(dict, within, "Name")?
                .map_or(StampName::Draft, StampName::from_name);
            Ok(TypedAnnot::Stamp(StampAnnot { name }))
        }
        AnnotSubtype::Caret => {
            let symbol = match reader.name_entry(dict, within, "Sy")? {
                Some(b"P") => CaretSymbol::Paragraph,
                _ => CaretSymbol::None,
            };
            let rect_diff = reader.rect4(dict, within, "RD")?.map(|(raw, _)| raw);
            Ok(TypedAnnot::Caret(CaretAnnot { symbol, rect_diff }))
        }
        AnnotSubtype::Popup => {
            let parent = dict.get(b"Parent").and_then(|o| o.as_reference());
            let open = reader.bool_entry(dict, within, "Open")?.unwrap_or(false);
            Ok(TypedAnnot::Popup(PopupAnnot { parent, open }))
        }
        AnnotSubtype::FileAttachment => {
            let (file_name, fs_id) = match dict.get(b"FS") {
                Some(raw) => {
                    let direct_ref = raw.as_reference();
                    let name = reader.file_spec(raw, within, "FS")?;
                    (name, direct_ref)
                }
                None => {
                    reader.say(within, None, Fault::WrongType { key: "FS" });
                    (None, None)
                }
            };
            let icon = reader
                .name_entry(dict, within, "Name")?
                .map_or(AttachmentIcon::PushPin, AttachmentIcon::from_name);
            Ok(TypedAnnot::FileAttachment(FileAttachmentAnnot {
                file_name,
                fs_id,
                icon,
            }))
        }
        AnnotSubtype::Redact => {
            let quad_points = reader.quad_points(dict, within)?.unwrap_or_default();
            let interior_color = read_color(reader, dict, within, "IC")?;
            let overlay_form = match dict.get(b"RO") {
                Some(raw) => match reader.reached(raw, within)? {
                    Some((obj, ro_id)) if obj.as_stream().is_some() => ro_id,
                    Some((obj, ro_id)) => {
                        reader.say(ro_id, obj.at(), Fault::WrongType { key: "RO" });
                        None
                    }
                    None => None,
                },
                None => None,
            };
            let overlay_text = reader.text_entry(dict, within, "OverlayText")?;
            let repeat = reader.bool_entry(dict, within, "Repeat")?.unwrap_or(false);
            let da = read_da(reader, dict, within)?;
            let quadding = reader
                .int_entry(dict, within, "Q")?
                .map_or(Quadding::Left, Quadding::from_pdf_int);
            Ok(TypedAnnot::Redact(RedactAnnot {
                quad_points,
                interior_color,
                overlay_form,
                overlay_text,
                repeat,
                da,
                quadding,
            }))
        }
        AnnotSubtype::Widget => {
            let highlight = reader
                .name_entry(dict, within, "H")?
                .and_then(HighlightMode::from_name)
                .unwrap_or_default();
            let appearance_chars = read_mk(reader, dict, within)?;
            let parent = dict.get(b"Parent").and_then(|o| o.as_reference());
            let da = read_da(reader, dict, within)?;
            let quadding = reader
                .int_entry(dict, within, "Q")?
                .map(Quadding::from_pdf_int);
            let actions = match dict.get(b"A") {
                Some(raw) => reader.action_chain(raw, within)?,
                None => Vec::new(),
            };
            Ok(TypedAnnot::Widget(WidgetAnnot {
                highlight,
                appearance_chars,
                parent,
                da,
                quadding,
                actions,
            }))
        }
        other => Ok(TypedAnnot::Other(other.clone())),
    }
}
