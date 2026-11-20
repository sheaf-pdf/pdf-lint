//! Non-fatal content-stream and graphics-state departures reported to a
//! [`Sink`](pdf_objects::fault::Sink).

use std::fmt;

use pdf_content_streams::Operator;
use pdf_objects::ObjectId;

/// A recoverable ISO 32000-2 departure observed while interpreting a content
/// stream with [`interpret_with`](crate::interpret_with), or reading a default
/// appearance string with
/// [`DefaultAppearance::parse_with`](crate::DefaultAppearance::parse_with).
///
/// Its `Debug` implementation reports resource names by byte length rather than
/// raw file bytes.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Fault {
    /// `Q` encountered with an empty graphics-state stack (ISO 32000-2, §8.4.2).
    ExtraRestore,
    /// `EMC` encountered with no open `BMC`/`BDC` in the current content stream
    /// (ISO 32000-2, §14.6.1).
    ExtraEndMarked,
    /// A content stream ended while `count` `BMC`/`BDC` sequences remained
    /// unclosed (ISO 32000-2, §14.6.1).
    UnclosedMarked {
        /// Number of unclosed marked-content sequences closed implicitly at the
        /// end of the stream.
        count: usize,
    },
    /// A Form XObject (`Do`) referenced itself directly or indirectly on the
    /// active call chain.
    FormCycle {
        /// Object ID of the recursive Form XObject.
        form: ObjectId,
    },
    /// A Form XObject (`Do`) exceeded [`Limits::form_depth`](crate::Limits::form_depth).
    FormDepth {
        /// Object ID of the Form XObject not entered.
        form: ObjectId,
    },
    /// An operator referenced a resource name absent from the effective
    /// `/Resources` dictionary (`Font`, `ExtGState`, `XObject`, `Shading`,
    /// `ColorSpace`, `Pattern`, or `Properties`).
    MissingResource {
        /// Resource subdictionary key (e.g. `"Font"`, `"ExtGState"`).
        category: &'static str,
        /// Referenced name bytes (decoded).
        name: Vec<u8>,
    },
    /// An indirect object referenced from `/Resources` or a graphics dictionary
    /// was damaged (`Why::Damaged`) and treated as absent.
    Damaged {
        /// Object ID of the damaged object.
        id: ObjectId,
    },
    /// A `/DA` string failed to lex or passed a `pdf-content-streams` cap
    /// (ISO 32000-2, §12.7.4.3): the error, its offset into the string and
    /// its kind, which tells a cap passed from bytes that don't lex.
    /// [`DefaultAppearance::parse_with`](crate::DefaultAppearance::parse_with)
    /// returns `None`.
    DefaultAppearanceUnreadable(pdf_content_streams::Error),
    /// A `/DA` operator ignored, or `Tf` read in part, for an operand it
    /// needs that's missing or not finite, or a `Tf` font that isn't a name.
    DefaultAppearanceOperand {
        /// Byte offset of the operator in the string.
        at: usize,
        /// The operator.
        operator: Operator,
    },
    /// A `/DA` `Tr` ignored for a mode outside `0..=7` (ISO 32000-2, §9.3.6).
    DefaultAppearanceTextMode {
        /// Byte offset of the `Tr` in the string.
        at: usize,
    },
    /// A `/DA` string in which no `Tf` names a font: ISO 32000-2, §12.7.4.3,
    /// asks for one at least.
    DefaultAppearanceNoFont,
}

impl fmt::Debug for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fault::ExtraRestore => f.write_str("ExtraRestore"),
            Fault::ExtraEndMarked => f.write_str("ExtraEndMarked"),
            Fault::UnclosedMarked { count } => f
                .debug_struct("UnclosedMarked")
                .field("count", count)
                .finish(),
            Fault::FormCycle { form } => f.debug_struct("FormCycle").field("form", form).finish(),
            Fault::FormDepth { form } => f.debug_struct("FormDepth").field("form", form).finish(),
            Fault::MissingResource { category, name } => f
                .debug_struct("MissingResource")
                .field("category", category)
                .field("name_len", &name.len())
                .finish(),
            Fault::Damaged { id } => f.debug_struct("Damaged").field("id", id).finish(),
            Fault::DefaultAppearanceUnreadable(error) => f
                .debug_tuple("DefaultAppearanceUnreadable")
                .field(error)
                .finish(),
            Fault::DefaultAppearanceOperand { at, operator } => f
                .debug_struct("DefaultAppearanceOperand")
                .field("at", at)
                .field("operator", operator)
                .finish(),
            Fault::DefaultAppearanceTextMode { at } => f
                .debug_struct("DefaultAppearanceTextMode")
                .field("at", at)
                .finish(),
            Fault::DefaultAppearanceNoFont => f.write_str("DefaultAppearanceNoFont"),
        }
    }
}
