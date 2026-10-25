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

#[cfg(test)]
mod tests {
    use super::*;

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
