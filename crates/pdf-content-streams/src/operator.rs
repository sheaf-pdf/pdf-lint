//! Content stream operators (ISO 32000-2, Annex A): each by its name, with
//! the operands the clause defining it gives it, its category (8.2, Table
//! 50) and that clause; and sets of them.

use std::fmt;

/// What an operand of an operator is, as the clause defining the operator
/// says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Type {
    /// A number, integer or real.
    Number,
    /// An integer.
    Integer,
    /// A name.
    Name,
    /// A string.
    String,
    /// An array: `d`'s of numbers, `TJ`'s of strings and numbers.
    Array,
    /// A name, or a dictionary written inline: a marked-content property
    /// list, or the name of one in the resources' `/Properties` (14.6.2).
    NameOrDict,
}

/// The operands an operator takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Operands {
    /// These, in order.
    Fixed(&'static [Type]),
    /// A number for each component of the current color space, which
    /// `SC` and `sc` set only in a space of one, three or four (8.6.8).
    Components,
    /// A number for each component of the current color space, any space;
    /// or, in a pattern space, the pattern's name, after the components of
    /// an uncolored pattern's own space (`SCN`, `scn`; 8.6.8). One operand
    /// at least, and no most: PDF 2.0 sets no limit on a DeviceN space's
    /// colorants (C.2).
    ComponentsOrPattern,
}

/// What an operator is for: the sixteen categories of ISO 32000-2, 8.2
/// (Table 50), which the graphics objects' states are told in
/// ([`State`](crate::State)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Category {
    /// `w`, `J`, `j`, `M`, `d`, `ri`, `i`, `gs`, `q`, `Q` (8.4.4).
    GeneralGraphicsState,
    /// `cm` (8.4.4).
    SpecialGraphicsState,
    /// `m`, `l`, `c`, `v`, `y`, `h`, `re` (8.5.2.1).
    PathConstruction,
    /// `S`, `s`, `f`, `F`, `f*`, `B`, `B*`, `b`, `b*`, `n` (8.5.3.1).
    PathPainting,
    /// `W`, `W*` (8.5.4).
    ClippingPath,
    /// `BT`, `ET` (9.4.1).
    TextObject,
    /// `Tc`, `Tw`, `Tz`, `TL`, `Tf`, `Tr`, `Ts` (9.3.1).
    TextState,
    /// `Td`, `TD`, `Tm`, `T*` (9.4.2).
    TextPositioning,
    /// `Tj`, `TJ`, `'`, `"` (9.4.3).
    TextShowing,
    /// `d0`, `d1` (9.6.4).
    Type3Font,
    /// `CS`, `cs`, `SC`, `SCN`, `sc`, `scn`, `G`, `g`, `RG`, `rg`, `K`,
    /// `k` (8.6.8).
    Color,
    /// `sh` (8.7.4.2).
    Shading,
    /// `BI`, `ID`, `EI` (8.9.7).
    InlineImage,
    /// `Do` (8.8.1).
    XObject,
    /// `MP`, `DP`, `BMC`, `BDC`, `EMC` (14.6.1).
    MarkedContent,
    /// `BX`, `EX` (7.8.2).
    Compatibility,
}

impl Category {
    /// The clause of ISO 32000-2 that defines its operators, as
    /// `"8.4.4"`.
    pub const fn clause(self) -> &'static str {
        match self {
            Category::GeneralGraphicsState | Category::SpecialGraphicsState => "8.4.4",
            Category::PathConstruction => "8.5.2.1",
            Category::PathPainting => "8.5.3.1",
            Category::ClippingPath => "8.5.4",
            Category::TextObject => "9.4.1",
            Category::TextState => "9.3.1",
            Category::TextPositioning => "9.4.2",
            Category::TextShowing => "9.4.3",
            Category::Type3Font => "9.6.4",
            Category::Color => "8.6.8",
            Category::Shading => "8.7.4.2",
            Category::InlineImage => "8.9.7",
            Category::XObject => "8.8.1",
            Category::MarkedContent => "14.6.1",
            Category::Compatibility => "7.8.2",
        }
    }

    /// Its operators.
    pub const fn operators(self) -> Operators {
        let mut set = Operators::NONE;
        let mut i = 0;
        while i < Operator::ALL.len() {
            let op = Operator::ALL[i];
            if let Some(category) = op.category() {
                if category as u8 == self as u8 {
                    set = set.with(op);
                }
            }
            i += 1;
        }
        set
    }
}

// The operands' shapes the table gives (`operators!`, below).
const NONE: Operands = Operands::Fixed(&[]);
const NUMBER: Operands = Operands::Fixed(&[Type::Number]);
const INTEGER: Operands = Operands::Fixed(&[Type::Integer]);
const NAME: Operands = Operands::Fixed(&[Type::Name]);
const STRING: Operands = Operands::Fixed(&[Type::String]);
const ARRAY: Operands = Operands::Fixed(&[Type::Array]);
const NUMBERS_2: Operands = Operands::Fixed(&[Type::Number; 2]);
const NUMBERS_3: Operands = Operands::Fixed(&[Type::Number; 3]);
const NUMBERS_4: Operands = Operands::Fixed(&[Type::Number; 4]);
const NUMBERS_6: Operands = Operands::Fixed(&[Type::Number; 6]);
const DASH: Operands = Operands::Fixed(&[Type::Array, Type::Number]);
const FONT: Operands = Operands::Fixed(&[Type::Name, Type::Number]);
const SPACED: Operands = Operands::Fixed(&[Type::Number, Type::Number, Type::String]);
const TAGGED: Operands = Operands::Fixed(&[Type::Name, Type::NameOrDict]);
const COMPONENTS: Operands = Operands::Components;
const PATTERN: Operands = Operands::ComponentsOrPattern;

/// A name's bytes as text, when it's compiled.
const fn text(name: &'static [u8]) -> &'static str {
    match std::str::from_utf8(name) {
        Ok(text) => text,
        Err(_) => panic!("an operator's name is ASCII"),
    }
}

/// The table: each operator, in the order of 8.2's categories, by its
/// variant, its name, its category and its operands.
macro_rules! operators {
    ($($(#[$doc:meta])* $variant:ident $name:literal $category:ident $operands:ident;)*) => {
        /// An operator of ISO 32000-2's Annex A, by what it does, or
        /// [`Other`](Operator::Other) for any other word in an operator's
        /// place. [`of`](Operator::of) tells one by its name, as written
        /// (case counts: `Sh` is no operator); the rest of the table is
        /// what the clause defining it says: its operands, its category
        /// and the clause ([`operands`](Operator::operands),
        /// [`category`](Operator::category),
        /// [`clause`](Operator::clause)), and the graphics objects it may
        /// be used in ([`State`](crate::State)).
        ///
        /// ```
        /// use pdf_content_streams::{Category, Operands, Operator, Paint, Type};
        ///
        /// let op = Operator::of(b"Tf");
        /// assert_eq!(op, Operator::Font);
        /// assert_eq!(op.name(), Some("Tf"));
        /// assert_eq!(op.operands(), Some(Operands::Fixed(&[Type::Name, Type::Number])));
        /// assert_eq!(op.category(), Some(Category::TextState));
        /// assert_eq!(op.clause(), Some("9.3.1"));
        /// assert_eq!(Operator::of(b"F").paint(), Some(Paint::Fill));
        /// assert_eq!(Operator::of(b"Fl"), Operator::Other);
        /// assert_eq!(Operator::ALL.len(), 73);
        /// ```
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum Operator {
            $($(#[$doc])* $variant,)*
            /// Any other word where an operator is: none Annex A names,
            /// which ISO 32000-2 passes over only inside `BX` and `EX`
            /// (7.8.2), and PDFium passes over anywhere.
            Other,
        }

        impl Operator {
            /// Every operator of Annex A, in the order of 8.2's categories:
            /// all but [`Other`](Operator::Other).
            pub const ALL: &'static [Operator] = &[$(Operator::$variant),*];

            /// The operator named `name`, as written: [`Other`](Operator::Other)
            /// for a word that's none.
            pub const fn of(name: &[u8]) -> Operator {
                match name {
                    $($name => Operator::$variant,)*
                    _ => Operator::Other,
                }
            }

            /// Its name, as written: none for [`Other`](Operator::Other).
            pub const fn name(self) -> Option<&'static str> {
                match self {
                    $(Operator::$variant => Some(const { text($name) }),)*
                    Operator::Other => None,
                }
            }

            /// The operands it takes, their count and types: none for
            /// [`Other`](Operator::Other), whose are unknown.
            pub const fn operands(self) -> Option<Operands> {
                match self {
                    $(Operator::$variant => Some($operands),)*
                    Operator::Other => None,
                }
            }

            /// Its category (8.2, Table 50): none for
            /// [`Other`](Operator::Other).
            pub const fn category(self) -> Option<Category> {
                match self {
                    $(Operator::$variant => Some(Category::$category),)*
                    Operator::Other => None,
                }
            }
        }
    };
}

operators! {
    /// `w`: the line width.
    LineWidth b"w" GeneralGraphicsState NUMBER;
    /// `J`: the line cap style.
    LineCap b"J" GeneralGraphicsState INTEGER;
    /// `j`: the line join style.
    LineJoin b"j" GeneralGraphicsState INTEGER;
    /// `M`: the miter limit.
    MiterLimit b"M" GeneralGraphicsState NUMBER;
    /// `d`: the dash pattern, an array of lengths and the phase.
    Dash b"d" GeneralGraphicsState DASH;
    /// `ri`: the rendering intent, by its name.
    RenderingIntent b"ri" GeneralGraphicsState NAME;
    /// `i`: the flatness tolerance.
    Flatness b"i" GeneralGraphicsState NUMBER;
    /// `gs`: graphics state parameters set from an `/ExtGState`
    /// resource, by its name.
    ExtGState b"gs" GeneralGraphicsState NAME;
    /// `q`: the graphics state pushed.
    Save b"q" GeneralGraphicsState NONE;
    /// `Q`: the graphics state popped, as the last `q` pushed it.
    Restore b"Q" GeneralGraphicsState NONE;
    /// `cm`: a matrix, as six numbers, multiplied into the CTM.
    Transform b"cm" SpecialGraphicsState NUMBERS_6;
    /// `m`: a subpath begun at a point.
    Move b"m" PathConstruction NUMBERS_2;
    /// `l`: a straight segment to a point.
    Line b"l" PathConstruction NUMBERS_2;
    /// `c`: a cubic Bézier curve, its two control points and its end.
    Curve b"c" PathConstruction NUMBERS_6;
    /// `v`: a curve whose first control point is where the pen is.
    CurveFromPen b"v" PathConstruction NUMBERS_4;
    /// `y`: a curve whose second control point is where it ends.
    CurveToEnd b"y" PathConstruction NUMBERS_4;
    /// `h`: the subpath closed, by a segment back to where it began.
    ClosePath b"h" PathConstruction NONE;
    /// `re`: a rectangle, a subpath of its own, from a corner, a width
    /// and a height.
    Rect b"re" PathConstruction NUMBERS_4;
    /// `S`: the path stroked.
    Stroke b"S" PathPainting NONE;
    /// `s`: closed, then stroked.
    CloseStroke b"s" PathPainting NONE;
    /// `f`: filled by the nonzero winding rule.
    Fill b"f" PathPainting NONE;
    /// `F`: `f` by its older name.
    FillCompat b"F" PathPainting NONE;
    /// `f*`: filled by the even-odd rule.
    FillEvenOdd b"f*" PathPainting NONE;
    /// `B`: filled by the nonzero winding rule, and stroked.
    FillStroke b"B" PathPainting NONE;
    /// `B*`: filled by the even-odd rule, and stroked.
    FillStrokeEvenOdd b"B*" PathPainting NONE;
    /// `b`: closed, filled and stroked.
    CloseFillStroke b"b" PathPainting NONE;
    /// `b*`: closed, filled by the even-odd rule and stroked.
    CloseFillStrokeEvenOdd b"b*" PathPainting NONE;
    /// `n`: the path ended, not painted, as after a clip.
    EndPath b"n" PathPainting NONE;
    /// `W`: the path made the clip too, by the nonzero winding rule,
    /// when it's painted.
    Clip b"W" ClippingPath NONE;
    /// `W*`: the path made the clip too, by the even-odd rule.
    ClipEvenOdd b"W*" ClippingPath NONE;
    /// `BT`: a text object begun.
    BeginText b"BT" TextObject NONE;
    /// `ET`: the text object ended.
    EndText b"ET" TextObject NONE;
    /// `Tc`: the character spacing.
    CharSpace b"Tc" TextState NUMBER;
    /// `Tw`: the word spacing.
    WordSpace b"Tw" TextState NUMBER;
    /// `Tz`: the horizontal scaling, in percent.
    Scale b"Tz" TextState NUMBER;
    /// `TL`: the leading.
    Leading b"TL" TextState NUMBER;
    /// `Tf`: the font, by its name in the resources' `/Font`, and its
    /// size.
    Font b"Tf" TextState FONT;
    /// `Tr`: the text rendering mode.
    RenderMode b"Tr" TextState INTEGER;
    /// `Ts`: the text rise.
    Rise b"Ts" TextState NUMBER;
    /// `Td`: a line's start moved by an offset from the last line's.
    MoveLine b"Td" TextPositioning NUMBERS_2;
    /// `TD`: `Td`, the leading set too.
    MoveLineLeading b"TD" TextPositioning NUMBERS_2;
    /// `Tm`: the text matrix, and with it the text line matrix, from six
    /// numbers.
    TextMatrix b"Tm" TextPositioning NUMBERS_6;
    /// `T*`: a new line, by the leading.
    NextLine b"T*" TextPositioning NONE;
    /// `Tj`: a string shown.
    Show b"Tj" TextShowing STRING;
    /// `TJ`: an array's strings shown, its numbers moving the position
    /// between them.
    ShowArray b"TJ" TextShowing ARRAY;
    /// `'`: `T*`, then `Tj`.
    NextLineShow b"'" TextShowing STRING;
    /// `"`: `'`, `Tw` and `Tc` set first.
    NextLineShowSpaced b"\"" TextShowing SPACED;
    /// `d0`: a Type 3 glyph's width, its description painting its color
    /// too.
    GlyphWidth b"d0" Type3Font NUMBERS_2;
    /// `d1`: a Type 3 glyph's width, and the box that bounds it, its
    /// description giving its shape alone.
    GlyphWidthBox b"d1" Type3Font NUMBERS_6;
    /// `CS`: the color space for stroking.
    StrokeColorSpace b"CS" Color NAME;
    /// `cs`: the color space for filling.
    FillColorSpace b"cs" Color NAME;
    /// `SC`: the color for stroking, its space not ICCBased, Separation,
    /// DeviceN or a pattern's (`SCN` sets those).
    StrokeColor b"SC" Color COMPONENTS;
    /// `SCN`: the color for stroking, in any space, a pattern among them.
    StrokeColorN b"SCN" Color PATTERN;
    /// `sc`: `SC`, for filling.
    FillColor b"sc" Color COMPONENTS;
    /// `scn`: `SCN`, for filling.
    FillColorN b"scn" Color PATTERN;
    /// `G`: DeviceGray, and a gray in it, for stroking.
    StrokeGray b"G" Color NUMBER;
    /// `g`: `G`, for filling.
    FillGray b"g" Color NUMBER;
    /// `RG`: DeviceRGB and a color in it, for stroking.
    StrokeRgb b"RG" Color NUMBERS_3;
    /// `rg`: `RG`, for filling.
    FillRgb b"rg" Color NUMBERS_3;
    /// `K`: DeviceCMYK and a color in it, for stroking.
    StrokeCmyk b"K" Color NUMBERS_4;
    /// `k`: `K`, for filling.
    FillCmyk b"k" Color NUMBERS_4;
    /// `sh`: a shading painted inside the clip, by its name in the
    /// resources' `/Shading`.
    Shade b"sh" Shading NAME;
    /// `BI`: an inline image begun, its dictionary after it.
    BeginInlineImage b"BI" InlineImage NONE;
    /// `ID`: the inline image's data next.
    InlineImageData b"ID" InlineImage NONE;
    /// `EI`: the inline image ended.
    EndInlineImage b"EI" InlineImage NONE;
    /// `Do`: an XObject painted, by its name in the resources'
    /// `/XObject`.
    Draw b"Do" XObject NAME;
    /// `MP`: a marked-content point, by its tag.
    MarkedPoint b"MP" MarkedContent NAME;
    /// `DP`: a marked-content point, by its tag, with a property list.
    MarkedPointProps b"DP" MarkedContent TAGGED;
    /// `BMC`: marked content begun, by its tag, until the `EMC` that ends
    /// it.
    BeginMarked b"BMC" MarkedContent NAME;
    /// `BDC`: marked content begun, by its tag, with a property list.
    BeginMarkedProps b"BDC" MarkedContent TAGGED;
    /// `EMC`: the marked content last begun ended.
    EndMarked b"EMC" MarkedContent NONE;
    /// `BX`: the start of a compatibility section, inside which a reader
    /// passes over an operator it doesn't know, and its operands.
    BeginCompat b"BX" Compatibility NONE;
    /// `EX`: the compatibility section ended.
    EndCompat b"EX" Compatibility NONE;
}

impl Operator {
    /// The clause of ISO 32000-2 that defines it, as `"8.4.4"`: its
    /// category's ([`Category::clause`]), none for
    /// [`Other`](Operator::Other).
    pub const fn clause(self) -> Option<&'static str> {
        match self.category() {
            Some(category) => Some(category.clause()),
            None => None,
        }
    }

    /// How it paints the path, where it's a path-painting operator: `F`
    /// as `f`.
    pub const fn paint(self) -> Option<Paint> {
        Some(match self {
            Operator::Stroke => Paint::Stroke,
            Operator::CloseStroke => Paint::CloseStroke,
            Operator::Fill | Operator::FillCompat => Paint::Fill,
            Operator::FillEvenOdd => Paint::FillEvenOdd,
            Operator::FillStroke => Paint::FillStroke,
            Operator::FillStrokeEvenOdd => Paint::FillStrokeEvenOdd,
            Operator::CloseFillStroke => Paint::CloseFillStroke,
            Operator::CloseFillStrokeEvenOdd => Paint::CloseFillStrokeEvenOdd,
            Operator::EndPath => Paint::None,
            _ => return None,
        })
    }

    /// The rule the path clips by, where it's `W` or `W*`.
    pub const fn clip(self) -> Option<FillRule> {
        match self {
            Operator::Clip => Some(FillRule::NonZero),
            Operator::ClipEvenOdd => Some(FillRule::EvenOdd),
            _ => None,
        }
    }

    /// Whether it builds the path a painting operator ends: path
    /// construction, and `W` and `W*`, which make it the clip too.
    pub const fn builds_path(self) -> bool {
        matches!(
            self,
            Operator::Move
                | Operator::Line
                | Operator::Curve
                | Operator::CurveFromPen
                | Operator::CurveToEnd
                | Operator::ClosePath
                | Operator::Rect
                | Operator::Clip
                | Operator::ClipEvenOdd
        )
    }

    /// Its place in a set of operators ([`Operators`]): its place in
    /// [`ALL`](Operator::ALL), [`Other`](Operator::Other) last.
    const fn bit(self) -> u128 {
        1 << self as u32
    }
}

/// What ends a path: how it's painted, if it is (8.5.3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Paint {
    /// `f`, and `F`.
    Fill,
    /// `f*`.
    FillEvenOdd,
    /// `S`.
    Stroke,
    /// `s`: closed, then stroked.
    CloseStroke,
    /// `B`.
    FillStroke,
    /// `B*`.
    FillStrokeEvenOdd,
    /// `b`: closed, filled and stroked.
    CloseFillStroke,
    /// `b*`.
    CloseFillStrokeEvenOdd,
    /// `n`: not painted, as after a clip.
    None,
}

impl Paint {
    /// The rule it fills by, where it fills.
    pub fn fills(self) -> Option<FillRule> {
        match self {
            Paint::Fill | Paint::FillStroke | Paint::CloseFillStroke => Some(FillRule::NonZero),
            Paint::FillEvenOdd | Paint::FillStrokeEvenOdd | Paint::CloseFillStrokeEvenOdd => {
                Some(FillRule::EvenOdd)
            }
            Paint::Stroke | Paint::CloseStroke | Paint::None => None,
        }
    }

    /// Whether it strokes.
    pub fn strokes(self) -> bool {
        matches!(
            self,
            Paint::Stroke
                | Paint::CloseStroke
                | Paint::FillStroke
                | Paint::FillStrokeEvenOdd
                | Paint::CloseFillStroke
                | Paint::CloseFillStrokeEvenOdd
        )
    }
}

/// Which rule says what's inside a path, filled or a clip (8.5.3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FillRule {
    /// The nonzero winding number rule.
    NonZero,
    /// The even-odd rule.
    EvenOdd,
}

/// Operators, as a set: of any [`Operator`], [`Other`](Operator::Other)
/// among them. A reader names those it wants this way, and each category
/// of 8.2 is one ([`Operators::COLOR`], say), as is what each graphics
/// object allows ([`State::allowed`](crate::State::allowed)).
///
/// ```
/// use pdf_content_streams::{Operator, Operators};
///
/// let wanted = Operators::of(&[b"q", b"Q", b"cm"]);
/// assert!(wanted.contains(Operator::Save));
/// assert!(!wanted.contains(Operator::Other));
/// let state = wanted.intersection(Operators::GENERAL_GRAPHICS_STATE);
/// assert_eq!(state, Operators::of(&[b"q", b"Q"]));
/// assert_eq!(format!("{state:?}"), "{q, Q}");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Operators(u128);

impl Operators {
    /// None.
    pub const NONE: Operators = Operators(0);
    /// Every operator, [`Other`](Operator::Other) too.
    pub const ALL: Operators = Operators(Operator::Other.bit() | (Operator::Other.bit() - 1));
    /// [`Category::GeneralGraphicsState`]'s.
    pub const GENERAL_GRAPHICS_STATE: Operators = Category::GeneralGraphicsState.operators();
    /// [`Category::SpecialGraphicsState`]'s.
    pub const SPECIAL_GRAPHICS_STATE: Operators = Category::SpecialGraphicsState.operators();
    /// [`Category::PathConstruction`]'s.
    pub const PATH_CONSTRUCTION: Operators = Category::PathConstruction.operators();
    /// [`Category::PathPainting`]'s.
    pub const PATH_PAINTING: Operators = Category::PathPainting.operators();
    /// [`Category::ClippingPath`]'s.
    pub const CLIPPING_PATH: Operators = Category::ClippingPath.operators();
    /// [`Category::TextObject`]'s.
    pub const TEXT_OBJECT: Operators = Category::TextObject.operators();
    /// [`Category::TextState`]'s.
    pub const TEXT_STATE: Operators = Category::TextState.operators();
    /// [`Category::TextPositioning`]'s.
    pub const TEXT_POSITIONING: Operators = Category::TextPositioning.operators();
    /// [`Category::TextShowing`]'s.
    pub const TEXT_SHOWING: Operators = Category::TextShowing.operators();
    /// [`Category::Type3Font`]'s.
    pub const TYPE3_FONT: Operators = Category::Type3Font.operators();
    /// [`Category::Color`]'s.
    pub const COLOR: Operators = Category::Color.operators();
    /// [`Category::Shading`]'s.
    pub const SHADING: Operators = Category::Shading.operators();
    /// [`Category::InlineImage`]'s.
    pub const INLINE_IMAGE: Operators = Category::InlineImage.operators();
    /// [`Category::XObject`]'s.
    pub const XOBJECT: Operators = Category::XObject.operators();
    /// [`Category::MarkedContent`]'s.
    pub const MARKED_CONTENT: Operators = Category::MarkedContent.operators();
    /// [`Category::Compatibility`]'s.
    pub const COMPATIBILITY: Operators = Category::Compatibility.operators();

    /// The operators named `names`, each as [`Operator::of`] tells it: a
    /// word that's none is [`Other`](Operator::Other).
    pub const fn of(names: &[&[u8]]) -> Operators {
        let mut set = Operators::NONE;
        let mut i = 0;
        while i < names.len() {
            set = set.with(Operator::of(names[i]));
            i += 1;
        }
        set
    }

    /// Whether it holds `op`.
    pub const fn contains(self, op: Operator) -> bool {
        self.0 & op.bit() != 0
    }

    /// It, and `op`.
    pub const fn with(self, op: Operator) -> Operators {
        Operators(self.0 | op.bit())
    }

    /// It, but `op`.
    pub const fn without(self, op: Operator) -> Operators {
        Operators(self.0 & !op.bit())
    }

    /// What either holds.
    pub const fn union(self, other: Operators) -> Operators {
        Operators(self.0 | other.0)
    }

    /// What both hold.
    pub const fn intersection(self, other: Operators) -> Operators {
        Operators(self.0 & other.0)
    }

    /// What it holds and `other` doesn't.
    pub const fn difference(self, other: Operators) -> Operators {
        Operators(self.0 & !other.0)
    }

    /// Whether it holds none.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// How many it holds.
    pub const fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// What it holds, in [`Operator::ALL`]'s order, [`Other`](Operator::Other)
    /// last.
    pub fn iter(self) -> impl Iterator<Item = Operator> {
        Operator::ALL
            .iter()
            .copied()
            .chain([Operator::Other])
            .filter(move |&op| self.contains(op))
    }
}

impl FromIterator<Operator> for Operators {
    fn from_iter<I: IntoIterator<Item = Operator>>(ops: I) -> Operators {
        ops.into_iter().fold(Operators::NONE, Operators::with)
    }
}

/// The names it holds, `Other` for [`Other`](Operator::Other).
impl fmt::Debug for Operators {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let names = self.iter().map(|op| Shown(op.name().unwrap_or("Other")));
        f.debug_set().entries(names).finish()
    }
}

/// A name shown as it's written, not quoted.
struct Shown(&'static str);

impl fmt::Debug for Shown {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.0)
    }
}
