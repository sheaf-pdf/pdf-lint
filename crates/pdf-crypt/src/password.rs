//! A typed password's byte forms: as typed, in UTF-8, which revisions 5
//! and 6 take, and in the single-byte encodings a password of revisions 2
//! to 4 may have been set in (ISO 32000-2, 7.6.4.3.2 and Annex D).

/// The bytes a password typed as `password` may have been set as: as
/// typed, in UTF-8, which revisions 5 and 6 take; then, for revisions 2 to
/// 4, whose passwords are bytes the spec has made from PDFDocEncoding
/// (ISO 32000-2, 7.6.4.3.2) but writers from whatever their system had, in
/// PDFDocEncoding, WinAnsiEncoding and MacRomanEncoding (Annex D), each
/// where every character has a byte, as qpdf tries them. None twice.
pub fn passwords(password: &str) -> Vec<Vec<u8>> {
    let mut passwords = vec![password.as_bytes().to_vec()];
    for byte in [pdf_doc_byte, win_ansi_byte, mac_roman_byte] {
        let encoded: Option<Vec<u8>> = password.chars().map(byte).collect();
        if let Some(encoded) = encoded.filter(|e| !passwords.contains(e)) {
            passwords.push(encoded);
        }
    }
    passwords
}

/// A PDFDocEncoding byte's character (ISO 32000-2, Annex D): Latin-1's,
/// but for the accents at 0x18–0x1F and the punctuation, ligatures and
/// letters at 0x80–0xA0; none for the three it leaves undefined. The table
/// is the one PDFium and qpdf share, which differ only on 0xAD. pdf-objects
/// decodes text strings by the same table (`text::pdf_doc_char`), kept here
/// too so that this crate needs RustCrypto's alone; a test of pdf-lint's,
/// which has both, holds the two to agreeing.
fn pdf_doc_char(b: u8) -> Option<char> {
    const ACCENTS: [char; 8] = ['˘', 'ˇ', 'ˆ', '˙', '˝', '˛', '˚', '˜'];
    const HIGH: [char; 33] = [
        '•', '†', '‡', '…', '—', '–', 'ƒ', '⁄', '‹', '›', '−', '‰', '„', '“', '”', '‘', '’', '‚',
        '™', 'ﬁ', 'ﬂ', 'Ł', 'Œ', 'Š', 'Ÿ', 'Ž', 'ı', 'ł', 'œ', 'š', 'ž', '\u{FFFD}', '€',
    ];
    match b {
        0x7F | 0x9F | 0xAD => None,
        0x18..=0x1F => Some(ACCENTS[usize::from(b - 0x18)]),
        0x80..=0xA0 => Some(HIGH[usize::from(b - 0x80)]),
        _ => Some(char::from(b)),
    }
}

/// `c`'s PDFDocEncoding byte, if it has one.
fn pdf_doc_byte(c: char) -> Option<u8> {
    (0..=255).find(|&b| pdf_doc_char(b) == Some(c))
}

/// `c`'s WinAnsiEncoding byte, if it has one (ISO 32000-2, Annex D):
/// Latin-1's, but for the punctuation, letters and euro at 0x80–0x9F.
fn win_ansi_byte(c: char) -> Option<u8> {
    const HIGH: [char; 32] = [
        '€', '\0', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\0', 'Ž', '\0', '\0',
        '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\0', 'ž', 'Ÿ',
    ];
    match u32::from(c) {
        0x01..=0x7F | 0xA0..=0xFF => u8::try_from(u32::from(c)).ok(),
        _ => (HIGH.iter().position(|&h| h == c && c != '\0')).map(|i| 0x80 + i as u8),
    }
}

/// `c`'s MacRomanEncoding byte, if it has one (ISO 32000-2, Annex D):
/// ASCII's, and Mac OS Roman's above, but for the fifteen symbols PDF's
/// encoding leaves out, and the currency sign at 0xDB.
fn mac_roman_byte(c: char) -> Option<u8> {
    const HIGH: [char; 128] = [
        'Ä', 'Å', 'Ç', 'É', 'Ñ', 'Ö', 'Ü', 'á', 'à', 'â', 'ä', 'ã', 'å', 'ç', 'é', 'è', //
        'ê', 'ë', 'í', 'ì', 'î', 'ï', 'ñ', 'ó', 'ò', 'ô', 'ö', 'õ', 'ú', 'ù', 'û', 'ü', //
        '†', '°', '¢', '£', '§', '•', '¶', 'ß', '®', '©', '™', '´', '¨', '\0', 'Æ', 'Ø', //
        '\0', '±', '\0', '\0', '¥', 'µ', '\0', '\0', '\0', '\0', '\0', 'ª', 'º', '\0', 'æ',
        'ø', //
        '¿', '¡', '¬', '\0', 'ƒ', '\0', '\0', '«', '»', '…', '\u{A0}', 'À', 'Ã', 'Õ', 'Œ',
        'œ', //
        '–', '—', '“', '”', '‘', '’', '÷', '\0', 'ÿ', 'Ÿ', '⁄', '¤', '‹', '›', 'ﬁ', 'ﬂ', //
        '‡', '·', '‚', '„', '‰', 'Â', 'Ê', 'Á', 'Ë', 'È', 'Í', 'Î', 'Ï', 'Ì', 'Ó', 'Ô', //
        '\0', 'Ò', 'Ú', 'Û', 'Ù', 'ı', 'ˆ', '˜', '¯', '˘', '˙', '˚', '¸', '˝', '˛', 'ˇ', //
    ];
    match u32::from(c) {
        0x01..=0x7F => u8::try_from(u32::from(c)).ok(),
        _ => (HIGH.iter().position(|&h| h == c && c != '\0')).map(|i| 0x80 + i as u8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_as_typed_then_in_each_single_byte_encoding() {
        assert_eq!(passwords("plain"), [b"plain".to_vec()]);
        // é: E9 in PDFDocEncoding and WinAnsiEncoding, alike, so once; 8E
        // in MacRomanEncoding.
        assert_eq!(
            passwords("café"),
            [
                "café".as_bytes().to_vec(),
                b"caf\xE9".to_vec(),
                b"caf\x8E".to_vec()
            ]
        );
        // The euro: A0, 80, and not in MacRomanEncoding, PDF's.
        assert_eq!(
            passwords("€1"),
            [
                "€1".as_bytes().to_vec(),
                b"\xA01".to_vec(),
                b"\x801".to_vec()
            ]
        );
        // What no single-byte encoding has: as typed only.
        assert_eq!(passwords("日本"), ["日本".as_bytes().to_vec()]);
        // Each table's own characters round-trip where it has them.
        for b in 0x80..=0xFF {
            if let Some(c) = pdf_doc_char(b) {
                assert_eq!(pdf_doc_byte(c), Some(b), "{b:#x}");
            }
        }
    }
}
