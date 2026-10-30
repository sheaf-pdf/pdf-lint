//! Where each operator may be used: the graphics objects of ISO 32000-2,
//! 8.2 (Figure 9), with the errata, and the start of a Type 3 glyph
//! description (9.6.4).

use crate::operations::Operation;
use crate::operator::{Operator, Operators};

/// Where content is, among the graphics objects of ISO 32000-2, 8.2
/// (Figure 9): each allows only some operators, and some begin or end
/// one ([`after`](State::after)). A content stream begins at
/// [`ContentStream`](State::ContentStream); a Type 3 glyph description
/// at [`GlyphStart`](State::GlyphStart).
///
/// A shading (`sh`) and an external object (`Do`) are objects too, ended
/// as soon as they're begun, so each leaves content at the content stream
/// level. Where 8.2 names operators by their category, it's that
/// category's [`Operators`]; a text object allows the general graphics
/// state, `q` and `Q` among it (9.4.1, as its errata have it), and marked
/// content (14.6.1, likewise). An operator Annex A doesn't name,
/// [`Operator::Other`], is allowed in none: ISO 32000-2 passes over one
/// only inside `BX` and `EX` (7.8.2), which a caller counts.
///
/// ```
/// use pdf_content_streams::{Limits, Operator, ResCx, State, operations};
///
/// let content = b"q 0 0 9 9 re W n BT /F1 12 Tf (Hi) Tj ET Q";
/// let mut state = State::ContentStream;
/// for op in operations(content, &ResCx::none(), &Limits::DEFAULT) {
///     state = state.step(&op.unwrap()).expect("allowed where it is");
/// }
/// assert_eq!(state, State::ContentStream);
///
/// assert_eq!(State::ContentStream.after(Operator::Rect), Some(State::Path));
/// assert_eq!(State::Path.after(Operator::Clip), Some(State::ClippingPath));
/// assert!(!State::Text.allows(Operator::Transform));
/// assert!(!State::ContentStream.allows(Operator::Show));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum State {
    /// Between graphics objects, where a content stream begins (8.2's
    /// content stream level): the general and special graphics state,
    /// color, text state, marked content and compatibility operators, and
    /// those that begin an object: `m` and `re` a path, `BT` text, `BI` an
    /// inline image, `sh` a shading and `Do` an external object.
    #[default]
    ContentStream,
    /// Inside a path object, which `m` or `re` begins: path construction;
    /// then `W` or `W*`, making it a clip, or a painting operator, which
    /// ends it.
    Path,
    /// Inside a path object after `W` or `W*`: a painting operator alone,
    /// which ends it.
    ClippingPath,
    /// Inside a text object, `BT` to `ET`: the general graphics state,
    /// color, text state, showing and positioning, marked content and
    /// compatibility operators.
    Text,
    /// Inside an inline image, `BI` to `EI`: `ID`.
    InlineImage,
    /// Where a Type 3 glyph description begins, `d0` or `d1` first, after
    /// which it's at the content stream level (9.6.4): the two are allowed
    /// nowhere else.
    GlyphStart,
}

impl State {
    /// Every state, in the order they're declared.
    const ALL: [State; 6] = [
        State::ContentStream,
        State::Path,
        State::ClippingPath,
        State::Text,
        State::InlineImage,
        State::GlyphStart,
    ];

    /// What each state allows, by its place in `ALL`.
    const ALLOWED: [Operators; 6] = {
        let mut allowed = [Operators::NONE; 6];
        let mut i = 0;
        while i < State::ALL.len() {
            let state = State::ALL[i];
            let mut set = Operators::NONE;
            let mut k = 0;
            while k <= Operator::ALL.len() {
                let op = if k < Operator::ALL.len() {
                    Operator::ALL[k]
                } else {
                    Operator::Other
                };
                if state.after(op).is_some() {
                    set = set.with(op);
                }
                k += 1;
            }
            allowed[state as usize] = set;
            i += 1;
        }
        allowed
    };

    /// Whether `op` may be used here.
    pub const fn allows(self, op: Operator) -> bool {
        self.after(op).is_some()
    }

    /// The operators that may be used here.
    pub const fn allowed(self) -> Operators {
        State::ALLOWED[self as usize]
    }

    /// Where content is after `op`, where it may be used here: none where
    /// it may not.
    pub const fn after(self, op: Operator) -> Option<State> {
        // What the content stream level and a text object allow, but the
        // operators that begin or end an object.
        const LEVEL: Operators = Operators::GENERAL_GRAPHICS_STATE
            .union(Operators::SPECIAL_GRAPHICS_STATE)
            .union(Operators::COLOR)
            .union(Operators::TEXT_STATE)
            .union(Operators::MARKED_CONTENT)
            .union(Operators::COMPATIBILITY);
        const TEXT: Operators = Operators::GENERAL_GRAPHICS_STATE
            .union(Operators::COLOR)
            .union(Operators::TEXT_STATE)
            .union(Operators::TEXT_SHOWING)
            .union(Operators::TEXT_POSITIONING)
            .union(Operators::MARKED_CONTENT)
            .union(Operators::COMPATIBILITY);
        let painted = Operators::PATH_PAINTING.contains(op);
        match self {
            State::ContentStream => match op {
                Operator::Move | Operator::Rect => Some(State::Path),
                Operator::BeginText => Some(State::Text),
                Operator::BeginInlineImage => Some(State::InlineImage),
                Operator::Shade | Operator::Draw => Some(State::ContentStream),
                _ if LEVEL.contains(op) => Some(State::ContentStream),
                _ => None,
            },
            State::Path => match op {
                Operator::Clip | Operator::ClipEvenOdd => Some(State::ClippingPath),
                _ if Operators::PATH_CONSTRUCTION.contains(op) => Some(State::Path),
                _ if painted => Some(State::ContentStream),
                _ => None,
            },
            State::ClippingPath if painted => Some(State::ContentStream),
            State::ClippingPath => None,
            State::Text => match op {
                Operator::EndText => Some(State::ContentStream),
                _ if TEXT.contains(op) => Some(State::Text),
                _ => None,
            },
            State::InlineImage => match op {
                Operator::InlineImageData => Some(State::InlineImage),
                Operator::EndInlineImage => Some(State::ContentStream),
                _ => None,
            },
            State::GlyphStart => match op {
                Operator::GlyphWidth | Operator::GlyphWidthBox => Some(State::ContentStream),
                _ => None,
            },
        }
    }

    /// Where content is after `op`, an operation, where it may be used
    /// here: none where it may not. An inline image, one operation from
    /// `BI` to `EI`, is `BI`, `ID` and `EI` in turn, so it ends where it
    /// began; any other operation is its operator
    /// ([`Operation::operator`]).
    pub fn step(self, op: &Operation) -> Option<State> {
        if op.is_inline_image() {
            self.after(Operator::BeginInlineImage)?
                .after(Operator::InlineImageData)?
                .after(Operator::EndInlineImage)
        } else {
            self.after(op.operator())
        }
    }
}
