//! The three classes of byte (ISO 32000-2, 7.2.3). Every byte is exactly
//! one of them. PDFium's rules read 0x80 and 0xFF as white-space too
//! ([`Rules`](crate::Rules)).

/// Whether `b` is white-space: NUL, tab, line feed, form feed, carriage
/// return or space.
pub const fn is_white_space(b: u8) -> bool {
    matches!(b, 0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

/// Whether `b` is a delimiter: `(`, `)`, `<`, `>`, `[`, `]`, `{`, `}`, `/`
/// or `%`.
pub const fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Whether `b` is regular: neither white-space nor a delimiter. Runs of
/// regular bytes are names' bodies, numbers and keywords.
pub const fn is_regular(b: u8) -> bool {
    !is_white_space(b) && !is_delimiter(b)
}
