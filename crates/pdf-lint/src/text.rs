//! Text strings (ISO 32000-2, 7.9.2.2): UTF-16BE or UTF-8 after their byte
//! order marks, PDFDocEncoding (Annex D) without one. Decoding follows the
//! spec and says where a string departs from it, rather than reading it as
//! some reader does: PDFium, for one, reads `FF FE` as UTF-16LE's mark and
//! drops an odd last byte without a word.

/// A text string's text, how it was encoded, and how it departs from the
/// spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    pub text: String,
    pub encoding: Encoding,
    pub quirks: Vec<Quirk>,
}

/// How a text string is encoded: by its byte order mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// After `FE FF`.
    Utf16Be,
    /// After `EF BB BF`, which PDF 2.0 brought in.
    Utf8,
    /// With neither mark.
    PdfDoc,
}

/// Where a text string departs from the spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quirk {
    /// It starts `FF FE`, UTF-16LE's mark, which PDF doesn't have: it's
    /// read as PDFDocEncoding, `ÿþ` and on, where PDFium reads UTF-16LE.
    LittleEndianMark,
    /// UTF-16BE with an odd byte at its end, left out.
    OddByte,
    /// UTF-16BE with a surrogate that isn't paired, read as U+FFFD.
    LoneSurrogate,
    /// UTF-8 that isn't, its bad bytes read as U+FFFD.
    BadUtf8,
    /// A byte PDFDocEncoding leaves undefined, read as U+FFFD: 0x7F, 0x9F
    /// or 0xAD. (qpdf reads 0xAD as a soft hyphen, U+00AD.)
    Undefined(u8),
}

/// `bytes`, a text string, decoded.
pub fn decode(bytes: &[u8]) -> Text {
    let mut quirks = Vec::new();
    let (text, encoding) = match bytes {
        [0xFE, 0xFF, rest @ ..] => {
            let (pairs, odd) = rest.as_chunks::<2>();
            if !odd.is_empty() {
                quirks.push(Quirk::OddByte);
            }
            let units = pairs.iter().map(|&pair| u16::from_be_bytes(pair));
            let mut text = String::with_capacity(pairs.len());
            for unit in char::decode_utf16(units) {
                text.push(unit.unwrap_or_else(|_| {
                    quirks.push(Quirk::LoneSurrogate);
                    char::REPLACEMENT_CHARACTER
                }));
            }
            (text, Encoding::Utf16Be)
        }
        [0xEF, 0xBB, 0xBF, rest @ ..] => {
            let text = String::from_utf8_lossy(rest).into_owned();
            if std::str::from_utf8(rest).is_err() {
                quirks.push(Quirk::BadUtf8);
            }
            (text, Encoding::Utf8)
        }
        _ => {
            if bytes.starts_with(&[0xFF, 0xFE]) {
                quirks.push(Quirk::LittleEndianMark);
            }
            let text = (bytes.iter())
                .map(|&b| {
                    pdf_doc_char(b).unwrap_or_else(|| {
                        quirks.push(Quirk::Undefined(b));
                        char::REPLACEMENT_CHARACTER
                    })
                })
                .collect();
            (text, Encoding::PdfDoc)
        }
    };
    Text {
        text,
        encoding,
        quirks,
    }
}

/// A PDFDocEncoding byte's character (ISO 32000-2, Annex D): Latin-1's,
/// but for the accents at 0x18–0x1F and the punctuation, ligatures and
/// letters at 0x80–0xA0; none for the three it leaves undefined. The table
/// is the one PDFium and qpdf share, which differ only on 0xAD.
pub fn pdf_doc_char(b: u8) -> Option<char> {
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

    #[test]
    fn each_encoding_by_its_mark() {
        let utf16: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain("Marked é".encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        let text = decode(&utf16);
        assert_eq!(
            (text.text.as_str(), text.encoding),
            ("Marked é", Encoding::Utf16Be)
        );
        assert!(text.quirks.is_empty());
        let utf8 = decode(b"\xEF\xBB\xBFcaf\xC3\xA9");
        assert_eq!(
            (utf8.text.as_str(), utf8.encoding),
            ("café", Encoding::Utf8)
        );
        let pdf_doc = decode(b"caf\xE9 \x84 \x92 \x18");
        assert_eq!(pdf_doc.text, "café — ™ ˘");
        assert_eq!(pdf_doc.encoding, Encoding::PdfDoc);
    }

    #[test]
    fn departures_from_the_spec_are_said() {
        let odd = decode(&[0xFE, 0xFF, 0x00, 0x41, 0x00]);
        assert_eq!((odd.text.as_str(), odd.quirks), ("A", vec![Quirk::OddByte]));
        let lone = decode(&[0xFE, 0xFF, 0xD8, 0x00]);
        assert_eq!(lone.quirks, [Quirk::LoneSurrogate]);
        assert_eq!(decode(b"\xEF\xBB\xBF\xFF").quirks, [Quirk::BadUtf8]);
        let little = decode(&[0xFF, 0xFE, 0x41, 0x00]);
        assert_eq!(little.text, "ÿþA\0");
        assert_eq!(little.quirks, [Quirk::LittleEndianMark]);
        let undefined = decode(b"a\x7Fb\x9F\xAD");
        assert_eq!(undefined.text, "a\u{FFFD}b\u{FFFD}\u{FFFD}");
        assert_eq!(
            undefined.quirks,
            [
                Quirk::Undefined(0x7F),
                Quirk::Undefined(0x9F),
                Quirk::Undefined(0xAD)
            ]
        );
    }

    /// The table agrees with qpdf's (`QUtil.cc`) on every defined byte;
    /// spot checks of each range.
    #[test]
    fn the_pdf_doc_table() {
        assert_eq!(pdf_doc_char(b'A'), Some('A'));
        assert_eq!(pdf_doc_char(0x1F), Some('˜'));
        assert_eq!(pdf_doc_char(0x80), Some('•'));
        assert_eq!(pdf_doc_char(0x93), Some('ﬁ'));
        assert_eq!(pdf_doc_char(0x9E), Some('ž'));
        assert_eq!(pdf_doc_char(0xA0), Some('€'));
        assert_eq!(pdf_doc_char(0xFF), Some('ÿ'));
        assert_eq!(pdf_doc_char(0x09), Some('\t'));
    }
}
