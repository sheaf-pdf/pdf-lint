//! The three classes of byte (ISO 32000-2, 7.2.3). Every byte is exactly
//! one of them. PDFium's rules read 0x80 and 0xFF as white-space too
//! ([`Rules`](crate::Rules)).
//!
//! Each byte's classes are looked up in a table of 256 entries, made when
//! the crate is compiled from the definitions below: a load and a test a
//! byte, where a test of each byte in turn is a branch or a jump.

/// Whether `b` is white-space: NUL, tab, line feed, form feed, carriage
/// return or space.
pub const fn is_white_space(b: u8) -> bool {
    class(b) & SPACE != 0
}

/// Whether `b` is a delimiter: `(`, `)`, `<`, `>`, `[`, `]`, `{`, `}`, `/`
/// or `%`.
pub const fn is_delimiter(b: u8) -> bool {
    class(b) & DELIMITER != 0
}

/// Whether `b` is regular: neither white-space nor a delimiter. Runs of
/// regular bytes are names' bodies, numbers and keywords.
pub const fn is_regular(b: u8) -> bool {
    class(b) & (SPACE | DELIMITER) == 0
}

/// White-space by ISO 32000-2's rules (7.2.3).
pub(crate) const SPACE: u8 = 1;
/// 0x80 and 0xFF, white-space by PDFium's rules alone.
pub(crate) const PDFIUM_SPACE: u8 = 2;
/// A delimiter (7.2.3).
pub(crate) const DELIMITER: u8 = 4;
/// A delimiter that can open or close an array or a dictionary, or start a
/// token that may hold one as one of its bytes (a string or a comment): a
/// byte a skim stops at ([`Tokenizer::skim`](crate::Tokenizer::skim)).
pub(crate) const SKIMMED: u8 = 8;

/// `b`'s classes, a bit for each.
#[inline(always)]
pub(crate) const fn class(b: u8) -> u8 {
    CLASSES[b as usize]
}

const CLASSES: [u8; 256] = {
    let mut table = [0; 256];
    let mut b = 0;
    while b < 256 {
        let byte = b as u8;
        let mut class = 0;
        if matches!(byte, 0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20) {
            class |= SPACE;
        }
        if matches!(byte, 0x80 | 0xFF) {
            class |= PDFIUM_SPACE;
        }
        if matches!(
            byte,
            b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
        ) {
            class |= DELIMITER;
        }
        if matches!(byte, b'(' | b'<' | b'>' | b'[' | b']' | b'%') {
            class |= SKIMMED;
        }
        table[b] = class;
        b += 1;
    }
    table
};
