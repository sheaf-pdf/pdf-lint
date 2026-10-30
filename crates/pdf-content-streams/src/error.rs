//! What stops content being read: what's wrong, and where.

/// Content that can't be read: what's wrong, and where it starts, an
/// offset into the bytes read. Read from a page's streams joined, it's
/// into the joined bytes, which [`Concat::pos`](crate::Concat::pos) maps
/// back to a stream and a place in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[error("{kind} at byte {at}")]
pub struct Error {
    /// Where what's wrong starts: a string's, an array's or a
    /// dictionary's first byte, an inline image's `BI`, or the first
    /// operand past the cap; and in marked content ([`marked`](crate::marked)),
    /// an `EMC` that ends none, a `BMC` or `BDC` past the cap, or the
    /// first operand of the first range still open at the end.
    pub at: usize,
    /// What's wrong.
    pub kind: ErrorKind,
}

/// What's wrong with content that can't be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum ErrorKind {
    /// A literal string whose `)` never comes.
    #[error("a string that doesn't end")]
    String,
    /// A hex string whose `>` never comes.
    #[error("a hex string that doesn't end")]
    HexString,
    /// An array whose `]` never comes.
    #[error("an array that doesn't end")]
    Array,
    /// A dictionary whose `>>` never comes.
    #[error("a dictionary that doesn't end")]
    Dict,
    /// An inline image whose `ID`, data or `EI` never comes: its
    /// dictionary doesn't end, its data runs past the content, or no `EI`
    /// follows it.
    #[error("an inline image that doesn't end")]
    InlineImage,
    /// Arrays and dictionaries, or marked content, nested past
    /// [`Limits::depth`](crate::Limits::depth).
    #[error("arrays, dictionaries or marked content nested too deep")]
    Depth,
    /// More operands before an operator than
    /// [`Limits::operands`](crate::Limits::operands).
    #[error("too many operands for one operator")]
    Operands,
    /// An inline image's data past
    /// [`Limits::inline_image`](crate::Limits::inline_image).
    #[error("an inline image too large")]
    InlineImageSize,
    /// Marked content, `BMC` or `BDC`, that no `EMC` ends.
    #[error("marked content that doesn't end")]
    Marked,
    /// An `EMC` with no marked content open to end.
    #[error("an EMC that ends no marked content")]
    Emc,
}
