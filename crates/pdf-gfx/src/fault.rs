//! Non-fatal content-stream and graphics-state departures reported to a
//! [`Sink`](pdf_objects::fault::Sink).

use std::fmt;

use pdf_objects::ObjectId;

/// A recoverable ISO 32000-2 departure observed while interpreting a content
/// stream with [`interpret_with`](crate::interpret_with).
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
        }
    }
}
