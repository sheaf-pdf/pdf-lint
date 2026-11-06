//! Text strings (ISO 32000-2, 7.9.2.2): UTF-16BE or UTF-8 after their byte
//! order marks, PDFDocEncoding (Annex D) without one. [`decode`] follows the
//! spec and says where a string departs from it, rather than reading it as
//! some reader does: PDFium, for one, reads `FF FE` as UTF-16LE's mark and
//! drops an odd last byte without a word. [`decode_with`] reads by either
//! ([`Rules`]), and says the same.
//!
//! [`encode`] writes a text so that it reads back as itself; [`encode_as`]
//! writes it in an encoding the caller chooses, or refuses;
//! [`Object::text`](crate::Object::text) is the string `encode` gives.

use pdf_tokenizer::Rules;

/// A text string's text, how it was encoded, and how it departs from the
/// spec. A decoder's account, read by its fields: `#[non_exhaustive]`, as
/// it may say more, such as the languages a PDFium reading takes out with
/// their escapes (ISO 32000-2, 7.9.2.2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Text {
    /// The text, U+FFFD where a character doesn't decode. By PDFium's rules
    /// ([`decode_with`]), a surrogate alone still is, but bytes that aren't
    /// UTF-8 are left out, and PDFDocEncoding's undefined bytes are U+0000.
    pub text: String,
    /// The encoding its byte order mark says.
    pub encoding: Encoding,
    /// Where it departs from the spec, in order.
    pub quirks: Vec<Quirk>,
}

/// How a text string is encoded: by its byte order mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Encoding {
    /// After `FE FF`.
    Utf16Be,
    /// After `EF BB BF`, which PDF 2.0 brought in.
    Utf8,
    /// With neither mark.
    PdfDoc,
    /// After `FF FE`, which PDF hasn't, and PDFium's rules read
    /// ([`decode_with`]). Nothing encodes it ([`encode_as`]).
    Utf16Le,
}

/// Why a text can't be written as [`encode_as`] was asked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Unencodable {
    /// A character PDFDocEncoding has no byte for, or U+0018 to U+001F,
    /// U+007F and the like, whose bytes it reads as something else.
    #[error("PDFDocEncoding has no byte for {0:?}")]
    Character(char),
    /// `þÿ`, `ï»¿` or `ÿþ` at its start, whose bytes in PDFDocEncoding would
    /// read as a byte order mark: the last by PDFium's rules.
    #[error("in PDFDocEncoding, it would start with a byte order mark's bytes")]
    Mark,
    /// U+001B, in UTF-16BE or UTF-8, where it starts a language escape
    /// (ISO 32000-2, 7.9.2.2.2), which a reader takes out with what follows.
    #[error("U+001B would start a language escape")]
    Escape,
    /// An encoding PDF doesn't have: UTF-16LE.
    #[error("PDF has no such encoding")]
    NoSuchEncoding,
}

/// Where a text string departs from the spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Quirk {
    /// It starts `FF FE`, UTF-16LE's mark, which PDF doesn't have: it's
    /// read as PDFDocEncoding, `ÿþ` and on, where PDFium reads UTF-16LE, as
    /// its rules do here.
    LittleEndianMark,
    /// UTF-16 with an odd byte at its end, left out.
    OddByte,
    /// UTF-16 with a surrogate that isn't paired, read as U+FFFD.
    LoneSurrogate,
    /// UTF-8 that isn't, its bad bytes read as U+FFFD, or by PDFium's rules
    /// as those rules read them ([`decode_with`]).
    BadUtf8,
    /// A byte PDFDocEncoding leaves undefined, read as U+FFFD, or U+0000 by
    /// PDFium's rules: 0x7F, 0x9F or 0xAD. (qpdf reads 0xAD as a soft
    /// hyphen, U+00AD.)
    Undefined(u8),
    /// By PDFium's rules, a language escape (ISO 32000-2, 7.9.2.2.2) with no
    /// U+001B to close it, which takes out the rest of the text.
    UnclosedEscape,
}

/// `bytes`, a text string, decoded.
pub fn decode(bytes: &[u8]) -> Text {
    let mut quirks = Vec::new();
    let (text, encoding) = match bytes {
        [0xFE, 0xFF, rest @ ..] => {
            let pairs = rest.chunks_exact(2);
            if !pairs.remainder().is_empty() {
                quirks.push(Quirk::OddByte);
            }
            let mut text = String::with_capacity(pairs.len());
            let units = pairs.map(|pair| u16::from_be_bytes([pair[0], pair[1]]));
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

/// `bytes`, a text string, decoded by `rules`: by ISO 32000-2's, as
/// [`decode`] does; or by PDFium's, as of chromium/8125, each departure from
/// the spec said as `decode` says it, where it differs from `decode` as
/// follows.
///
/// - `FF FE` is UTF-16LE's mark ([`Encoding::Utf16Le`]).
/// - Language escapes (ISO 32000-2, 7.9.2.2.2) come out of UTF-16 and UTF-8,
///   whatever is between the two U+001B; one with no second takes out the
///   rest. `decode` leaves them in the text. In PDFDocEncoding, 0x1B is an
///   accent, as it is for `decode`.
/// - Bytes that aren't UTF-8 are left out, not replaced: a byte that doesn't
///   belong where it is, a sequence cut short, a code point past U+10FFFF,
///   and all of what starts with a byte from 0xF8 up. A sequence longer than
///   it need be reads as the character it spells, and one that spells a
///   surrogate as a surrogate does.
/// - PDFDocEncoding's undefined bytes are U+0000.
///
/// PDFium keeps a surrogate alone, which a [`String`] can't hold: it's U+FFFD
/// here.
pub fn decode_with(bytes: &[u8], rules: Rules) -> Text {
    match rules {
        Rules::Pdfium => decode_as_pdfium(bytes),
        _ => decode(bytes),
    }
}

/// [`decode_with`], by PDFium's rules.
fn decode_as_pdfium(bytes: &[u8]) -> Text {
    let mut quirks = Vec::new();
    let (units, encoding) = match bytes {
        [0xFE, 0xFF, rest @ ..] => (
            utf16(rest, u16::from_be_bytes, &mut quirks),
            Encoding::Utf16Be,
        ),
        [0xFF, 0xFE, rest @ ..] => {
            quirks.push(Quirk::LittleEndianMark);
            (
                utf16(rest, u16::from_le_bytes, &mut quirks),
                Encoding::Utf16Le,
            )
        }
        [0xEF, 0xBB, 0xBF, rest @ ..] => {
            if std::str::from_utf8(rest).is_err() {
                quirks.push(Quirk::BadUtf8);
            }
            (utf8_as_pdfium(rest), Encoding::Utf8)
        }
        _ => {
            let text = (bytes.iter())
                .map(|&b| {
                    pdf_doc_char(b).unwrap_or_else(|| {
                        quirks.push(Quirk::Undefined(b));
                        '\0'
                    })
                })
                .collect();
            let encoding = Encoding::PdfDoc;
            return Text {
                text,
                encoding,
                quirks,
            };
        }
    };
    let mut text = String::with_capacity(units.len());
    let mut escaped = false;
    let kept = units.into_iter().filter(|&unit| {
        escaped ^= unit == 0x1B;
        !escaped && unit != 0x1B
    });
    for unit in char::decode_utf16(kept) {
        text.push(unit.unwrap_or_else(|_| {
            quirks.push(Quirk::LoneSurrogate);
            char::REPLACEMENT_CHARACTER
        }));
    }
    if escaped {
        quirks.push(Quirk::UnclosedEscape);
    }
    Text {
        text,
        encoding,
        quirks,
    }
}

/// UTF-16's code units, an odd last byte left out.
fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16, quirks: &mut Vec<Quirk>) -> Vec<u16> {
    let pairs = bytes.chunks_exact(2);
    if !pairs.remainder().is_empty() {
        quirks.push(Quirk::OddByte);
    }
    pairs.map(|pair| unit([pair[0], pair[1]])).collect()
}

/// UTF-16's code units of what PDFium's rules read in `bytes`, written as
/// UTF-8: a byte from 0xC0 to 0xF7 starts a sequence whatever came before,
/// its length by its high bits; a byte under 0x80 is its character, and
/// ends a sequence not finished; a byte from 0x80 to 0xBF goes on a sequence,
/// and is left out where none is going on; a byte from 0xF8 up ends one, and
/// is left out.
fn utf8_as_pdfium(bytes: &[u8]) -> Vec<u16> {
    let mut units = Vec::with_capacity(bytes.len());
    let (mut pending, mut code) = (0, 0u32);
    for &b in bytes {
        match b {
            0x00..=0x7F => {
                pending = 0;
                units.push(u16::from(b));
            }
            0x80..=0xBF if pending > 0 => {
                pending -= 1;
                code = code << 6 | u32::from(b & 0x3F);
                if pending == 0 {
                    match char::from_u32(code) {
                        Some(c) => units.extend_from_slice(c.encode_utf16(&mut [0; 2])),
                        // A surrogate: it fits a code unit.
                        None if code <= 0xFFFF => units.push(code as u16),
                        None => {}
                    }
                }
            }
            0x80..=0xBF => {}
            0xC0..=0xDF => (pending, code) = (1, u32::from(b & 0x1F)),
            0xE0..=0xEF => (pending, code) = (2, u32::from(b & 0x0F)),
            0xF0..=0xF7 => (pending, code) = (3, u32::from(b & 0x07)),
            0xF8..=0xFF => pending = 0,
        }
    }
    units
}

/// `text` as a text string that reads back as `text`: as it is, when
/// PDFDocEncoding reads those bytes back so (ASCII but for 0x18 to 0x1F,
/// which it gives accents, and 0x7F, which it leaves undefined), and otherwise
/// UTF-16BE after its mark. So [`decode`] gives `text` for any text, and no
/// bytes that are as they are can be taken for a mark.
///
/// U+001B is written as UTF-16BE like the other controls, and [`decode`]
/// gives it back, but it starts a language escape there (ISO 32000-2,
/// 7.9.2.2.2), so a reader that takes those out takes it and what follows
/// ([`decode_with`], by PDFium's rules). [`encode_as`] refuses it.
pub fn encode(text: &str) -> Vec<u8> {
    if as_it_is(text) {
        text.as_bytes().to_vec()
    } else {
        utf16be(text)
    }
}

/// Whether `text`'s bytes read back as `text`, with no mark: [`encode`]'s
/// first arm.
pub(crate) fn as_it_is(text: &str) -> bool {
    text.bytes()
        .all(|b| b.is_ascii() && !matches!(b, 0x18..=0x1F | 0x7F))
}

/// `text` as UTF-16BE after its mark.
fn utf16be(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(2 + 2 * text.len());
    bytes.extend_from_slice(&[0xFE, 0xFF]);
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    bytes
}

/// `text` as a text string in `encoding`, or why it can't be one that reads
/// back as `text` ([`Unencodable`]). PDFDocEncoding takes the characters
/// [`pdf_doc_char`] gives, and refuses a text whose bytes would start as a
/// mark. UTF-16BE and UTF-8 take every character but U+001B, where it would
/// start a language escape; UTF-8 is PDF 2.0's, for a caller writing 2.0 to
/// choose. UTF-16LE isn't PDF's, so nothing is written in it.
pub fn encode_as(text: &str, encoding: Encoding) -> Result<Vec<u8>, Unencodable> {
    if matches!(encoding, Encoding::Utf16Be | Encoding::Utf8) && text.contains('\u{1B}') {
        return Err(Unencodable::Escape);
    }
    match encoding {
        Encoding::Utf16Be => Ok(utf16be(text)),
        Encoding::Utf8 => {
            let mut bytes = Vec::with_capacity(3 + text.len());
            bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            bytes.extend_from_slice(text.as_bytes());
            Ok(bytes)
        }
        Encoding::PdfDoc => {
            let bytes = (text.chars())
                .map(|c| pdf_doc_byte(c).ok_or(Unencodable::Character(c)))
                .collect::<Result<Vec<u8>, _>>()?;
            match bytes.as_slice() {
                [0xFE, 0xFF, ..] | [0xFF, 0xFE, ..] | [0xEF, 0xBB, 0xBF, ..] => {
                    Err(Unencodable::Mark)
                }
                _ => Ok(bytes),
            }
        }
        Encoding::Utf16Le => Err(Unencodable::NoSuchEncoding),
    }
}

/// The PDFDocEncoding byte that [`pdf_doc_char`] reads as `c`: its Latin-1
/// byte, or one of the bytes that isn't Latin-1's.
fn pdf_doc_byte(c: char) -> Option<u8> {
    let latin1 = u8::try_from(u32::from(c)).ok();
    (latin1.into_iter().chain(0x18..=0x1F).chain(0x80..=0xA0)).find(|&b| pdf_doc_char(b) == Some(c))
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

    fn pdfium(bytes: &[u8]) -> Text {
        decode_with(bytes, Rules::Pdfium)
    }

    /// UTF-16BE after its mark, each unit given.
    fn be(units: &[u16]) -> Vec<u8> {
        let units = units.iter().flat_map(|u| u.to_be_bytes());
        [0xFE, 0xFF].into_iter().chain(units).collect()
    }

    #[test]
    fn iso_s_rules_are_decode() {
        for bytes in [
            &b"caf\xE9"[..],
            b"\xFF\xFEA\x00",
            b"\xEF\xBB\xBFa\xFFb",
            b"\xFE\xFF\x00\x1B\x00e\x00\x1B\x00A",
            b"a\x7Fb",
        ] {
            assert_eq!(decode_with(bytes, Rules::Iso), decode(bytes));
        }
        assert_eq!(decode(b"\xFE\xFF\x00\x1B").text, "\u{1B}");
    }

    #[test]
    fn pdfium_s_rules_read_ff_fe_as_utf_16le() {
        let little = pdfium(b"\xFF\xFEA\x00\xE9\x00");
        assert_eq!(little.text, "Aé");
        assert_eq!(little.encoding, Encoding::Utf16Le);
        assert_eq!(little.quirks, [Quirk::LittleEndianMark]);
        let odd = pdfium(b"\xFF\xFEA\x00\x00");
        assert_eq!(odd.text, "A");
        assert_eq!(odd.quirks, [Quirk::LittleEndianMark, Quirk::OddByte]);
        let pair = pdfium(b"\xFF\xFE\x3D\xD8\x00\xDE");
        assert_eq!(pair.text, "😀");
        // What isn't quite a mark is PDFDocEncoding.
        assert_eq!(pdfium(b"\xFF").encoding, Encoding::PdfDoc);
        assert_eq!(pdfium(b"\xFE").text, "þ");
    }

    #[test]
    fn pdfium_s_rules_take_language_escapes_out() {
        let lang = pdfium(&be(&[0x41, 0x1B, 0x65, 0x6E, 0x1B, 0x42]));
        assert_eq!((lang.text.as_str(), lang.quirks), ("AB", vec![]));
        let two = pdfium(&be(&[0x1B, 0x65, 0x1B, 0x41, 0x1B, 0x1B, 0x42]));
        assert_eq!((two.text.as_str(), two.quirks), ("AB", vec![]));
        assert_eq!(pdfium(b"\xEF\xBB\xBF\x1BenUS\x1BA").text, "A");
        assert_eq!(pdfium(b"\xFF\xFE\x1B\x00e\x00\x1B\x00A\x00").text, "A");
        for (bytes, text) in [
            (be(&[0x41, 0x1B, 0x65, 0x42]), "A"),
            (be(&[0x41, 0x1B]), "A"),
            (b"\xEF\xBB\xBFA\x1BenUS".to_vec(), "A"),
        ] {
            let unclosed = pdfium(&bytes);
            assert_eq!(unclosed.text, text);
            assert_eq!(unclosed.quirks, [Quirk::UnclosedEscape]);
        }
        // An accent in PDFDocEncoding, under both.
        assert_eq!(pdfium(b"a\x1Bb").text, "a˙b");
        assert_eq!(pdfium(b"a\x1Bb").quirks, []);
        // A surrogate's two halves meet once the escape's out.
        assert_eq!(pdfium(&be(&[0xD83D, 0x1B, 0x1B, 0xDE00])).text, "😀");
    }

    #[test]
    fn pdfium_s_rules_on_surrogates_and_undefined_bytes() {
        let lone = pdfium(&be(&[0xD800, 0x41, 0xDC00]));
        assert_eq!(lone.text, "\u{FFFD}A\u{FFFD}");
        assert_eq!(lone.quirks, [Quirk::LoneSurrogate, Quirk::LoneSurrogate]);
        let undefined = pdfium(b"a\x7Fb\x9F\xAD");
        assert_eq!(undefined.text, "a\0b\0\0");
        assert_eq!(
            undefined.quirks,
            [
                Quirk::Undefined(0x7F),
                Quirk::Undefined(0x9F),
                Quirk::Undefined(0xAD)
            ]
        );
    }

    /// Each shape PDFium's UTF-8 reading was asked about, its answer at
    /// chromium/8125 beside it.
    #[test]
    fn pdfium_s_rules_on_bad_utf_8() {
        for (bytes, text) in [
            (&b"a\xFFb"[..], "ab"),
            (b"a\xFEb", "ab"),
            (b"a\x80b", "ab"),
            (b"\xC3A", "A"),
            (b"A\xC3", "A"),
            (b"\xE2\x82A", "A"),
            (b"\xE2\x82A\x82", "A"),
            (b"\xE2\xC3\xA9", "é"),
            (b"\xC3\xC3\xA9", "é"),
            (b"\xC3\xA9\xA9", "é"),
            (b"\xE2\x82\xFF\x82A", "A"),
            (b"\xE2\x82\xF8\x82A", "A"),
            (b"a\xC0\x80b", "a\0b"),
            (b"\xC1\x81", "A"),
            (b"\xE0\x81\x81", "A"),
            (b"\xF0\x80\x81\x81", "A"),
            (b"a\xC0\x9BenUS\xC0\x9Bb", "ab"),
            (b"\xED\xA0\x80A", "\u{FFFD}A"),
            (b"\xED\xA0\xBD\xED\xB8\x80", "😀"),
            (b"\xF4\x90\x80\x81", ""),
            (b"\xF4\x90\x80\x9BA", "A"),
            (b"\xF7\xBF\xBF\xBF", ""),
            (b"\xF8\x80\x80\x81\x81", ""),
            (b"\xFC\x80\x80\x80\x81\x81", ""),
        ] {
            let read = pdfium(&[b"\xEF\xBB\xBF", bytes].concat());
            assert_eq!(read.text, text, "{bytes:02X?}");
            assert_eq!(read.encoding, Encoding::Utf8);
            assert_eq!(read.quirks.first(), Some(&Quirk::BadUtf8), "{bytes:02X?}");
        }
        let good = pdfium(b"\xEF\xBB\xBF\xF0\x9F\x98\x80caf\xC3\xA9");
        assert_eq!((good.text.as_str(), good.quirks), ("😀café", vec![]));
    }

    #[test]
    fn a_text_is_encoded_as_it_is_or_as_utf_16be() {
        assert_eq!(encode("plain (text)\n\t\0"), b"plain (text)\n\t\0");
        assert_eq!(encode(""), b"");
        assert_eq!(encode("é"), b"\xFE\xFF\x00\xE9");
        assert_eq!(encode("😀"), b"\xFE\xFF\xD8\x3D\xDE\x00");
        assert_eq!(encode("þÿ"), b"\xFE\xFF\x00\xFE\x00\xFF");
        // ASCII that PDFDocEncoding reads as something else.
        assert_eq!(encode("a\x7F"), b"\xFE\xFF\x00a\x00\x7F");
        for b in 0x18..=0x1F {
            let text = char::from(b).to_string();
            assert_eq!(encode(&text), [0xFE, 0xFF, 0x00, b], "{b:02X}");
        }
        assert_eq!(encode("\x17\x20"), b"\x17\x20");
        assert_eq!(encode("~"), b"~");
    }

    /// Every character, as it is and among others, reads back.
    #[test]
    fn what_s_encoded_reads_back() {
        let some = [
            '\0', 'A', '\u{18}', '\u{1B}', '\u{7F}', '\u{AD}', 'é', 'þ', 'ÿ', '€',
        ];
        let some = some
            .into_iter()
            .chain(['\u{FFFD}', '\u{FEFF}', '\u{FFFE}', '😀']);
        for a in some.clone() {
            for b in some.clone() {
                let text: String = [a, b].iter().collect();
                let read = decode(&encode(&text));
                assert_eq!(read.text, text);
                assert_eq!(read.quirks, []);
                if !text.contains('\u{1B}') {
                    assert_eq!(pdfium(&encode(&text)).text, text);
                    assert_eq!(pdfium(&encode(&text)).quirks, []);
                }
            }
        }
        for c in (0..=0x10FFFF).filter_map(char::from_u32) {
            let text = c.to_string();
            assert_eq!(decode(&encode(&text)).text, text);
        }
    }

    #[test]
    fn a_text_is_encoded_as_asked() {
        assert_eq!(encode_as("é", Encoding::Utf16Be), Ok(encode("é")));
        assert_eq!(
            encode_as("A", Encoding::Utf16Be),
            Ok(b"\xFE\xFF\x00A".to_vec())
        );
        assert_eq!(
            encode_as("é", Encoding::Utf8),
            Ok(b"\xEF\xBB\xBF\xC3\xA9".to_vec())
        );
        assert_eq!(
            encode_as("é€˘", Encoding::PdfDoc),
            Ok(b"\xE9\xA0\x18".to_vec())
        );
        assert_eq!(encode_as("", Encoding::PdfDoc), Ok(vec![]));
        assert_eq!(
            encode_as("A", Encoding::Utf16Le),
            Err(Unencodable::NoSuchEncoding)
        );
    }

    #[test]
    fn what_wouldn_t_read_back_is_refused() {
        for text in ["þÿ", "þÿa", "ï»¿", "ï»¿a", "ÿþ", "ÿþa"] {
            assert_eq!(
                encode_as(text, Encoding::PdfDoc),
                Err(Unencodable::Mark),
                "{text}"
            );
            assert_eq!(decode(&encode(text)).text, text);
        }
        for text in ["þ", "ÿ", "ï»", "aþÿ"] {
            assert!(encode_as(text, Encoding::PdfDoc).is_ok(), "{text}");
        }
        for c in [
            '\u{18}', '\u{1B}', '\u{1F}', '\u{7F}', '\u{9F}', '\u{AD}', '\u{FFFD}', '😀',
        ] {
            let text = format!("a{c}");
            assert_eq!(
                encode_as(&text, Encoding::PdfDoc),
                Err(Unencodable::Character(c))
            );
        }
        for encoding in [Encoding::Utf16Be, Encoding::Utf8] {
            assert_eq!(encode_as("a\u{1B}", encoding), Err(Unencodable::Escape));
        }
        assert!(encode_as("\u{1B}", Encoding::Utf16Le).is_err());
        for error in [
            Unencodable::Character('😀'),
            Unencodable::Mark,
            Unencodable::Escape,
            Unencodable::NoSuchEncoding,
        ] {
            assert!(!error.to_string().is_empty());
        }
    }

    /// Each defined byte's character encodes as that byte, and each
    /// character encoded reads back, under either rules.
    #[test]
    fn pdf_doc_encoding_both_ways() {
        for b in 0..=u8::MAX {
            match pdf_doc_char(b) {
                Some(c) => {
                    assert_eq!(pdf_doc_byte(c), Some(b), "{b:02X}");
                    let text = c.to_string();
                    let bytes = encode_as(&text, Encoding::PdfDoc).unwrap();
                    assert_eq!(bytes, [b]);
                    assert_eq!(decode(&bytes).text, text);
                    assert_eq!(pdfium(&bytes).text, text);
                }
                None => assert_eq!(pdf_doc_byte(char::from(b)), None, "{b:02X}"),
            }
        }
        assert_eq!(pdf_doc_byte('\u{FFFD}'), None);
    }
}
