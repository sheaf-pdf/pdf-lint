//! PDF text string codecs (ISO 32000-2 §7.9.2.2 and PDFium `chromium/8152`).
//!
//! PDF text strings represent human-readable metadata, bookmark titles, and annotation contents
//! in one of three encodings selected by an optional byte-order mark (BOM):
//! - `FE FF` — `UTF-16BE` ([`Encoding::Utf16Be`]), with embedded `0x00 0x1B <lang> 0x00 0x1B`
//!   language escapes;
//! - `EF BB BF` — `UTF-8` ([`Encoding::Utf8`], introduced in PDF 2.0), with `0x1B <lang> 0x1B`
//!   language escapes;
//! - No BOM — `PDFDocEncoding` ([`Encoding::PdfDoc`], ISO 32000-2 Annex D Table D.2).
//!
//! The top-level module implements the bidirectional ISO 32000-2 §7.9.2.2 specification
//! ([`decode`], [`encode`], [`encode_as`]). The [`pdfium`] submodule implements PDFium's forgiving
//! text string decoder as of chromium/8152 (supporting `FF FE` `UTF-16LE` BOMs, `U+0000` fallbacks
//! for undefined bytes, and PDFium's state-machine `UTF-8` recovery). Use [`decode_str`] when you
//! only need the decoded `String` dispatched by [`Rules`].
//!
//! ```
//! use pdf_objects::text::{self, Encoding, Quirk};
//!
//! // ISO 32000-2 UTF-16BE with an embedded language escape ("enUS").
//! let raw = b"\xFE\xFF\x00\x1BenUS\x00\x1B\x00H\x00i";
//! let decoded = text::decode(raw);
//! assert_eq!(decoded.encoding, Encoding::Utf16Be);
//! assert_eq!(decoded.text, "Hi");
//! assert_eq!(decoded.languages, ["enUS"]);
//! assert!(decoded.quirks.is_empty());
//!
//! // Non-standard FF FE BOM: ISO reads PDFDocEncoding with a quirk; PDFium reads UTF-16LE.
//! assert_eq!(text::decode(b"\xFF\xFEA\x00").quirks, [Quirk::LittleEndianMark, Quirk::Undefined(0)]);
//! assert_eq!(text::pdfium::decode(b"\xFF\xFEA\x00").text, "A");
//! ```

use alloc::string::String;
use alloc::vec::Vec;

use pdf_tokenizer::Rules;

/// Result of decoding a PDF text string under ISO 32000-2 §7.9.2.2 rules ([`decode`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Text {
    /// Decoded Unicode text (`U+FFFD` replaces lone surrogates, invalid UTF-8, and undefined
    /// `PDFDocEncoding` bytes).
    pub text: String,
    /// Encoding selected by the string's byte-order mark.
    pub encoding: Encoding,
    /// Language tags stripped from `0x00 0x1B ... 0x00 0x1B` (`UTF-16BE`) or `0x1B ... 0x1B`
    /// (`UTF-8`) escape sequences in order of appearance (`""` marks an end-of-language escape).
    pub languages: Vec<String>,
    /// Spec departures observed during decoding, in order.
    pub quirks: Vec<Quirk>,
}

/// ISO 32000-2 §7.9.2.2 text string encodings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Encoding {
    /// `UTF-16BE` following BOM `FE FF`.
    Utf16Be,
    /// `UTF-8` following BOM `EF BB BF` (PDF 2.0).
    Utf8,
    /// `PDFDocEncoding` (Annex D Table D.2) when neither BOM is present.
    PdfDoc,
}

/// Reason [`encode_as`] could not represent a string in the requested [`Encoding`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Unencodable {
    /// `PDFDocEncoding` has no byte mapping for this character.
    #[error("PDFDocEncoding has no byte for {0:?}")]
    Character(char),
    /// The resulting `PDFDocEncoding` bytes would begin with a BOM (`FE FF`, `EF BB BF`, or
    /// `FF FE`) and misread on decode.
    #[error("PDFDocEncoding bytes would start with a byte-order mark")]
    Mark,
    /// `U+001B` in `UTF-16BE` or `UTF-8` starts a §7.9.2.2.2 language escape sequence.
    #[error("U+001B starts a language escape sequence in UTF-16BE and UTF-8")]
    Escape,
}

/// Departure from ISO 32000-2 §7.9.2.2 observed by [`decode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Quirk {
    /// Starts with `FF FE` (`UTF-16LE` BOM, not part of ISO 32000-2); decoded as `PDFDocEncoding`.
    LittleEndianMark,
    /// Trailing odd byte in `UTF-16BE`, omitted.
    OddByte,
    /// Unpaired UTF-16 surrogate code unit, replaced with `U+FFFD`.
    LoneSurrogate,
    /// Invalid `UTF-8` byte sequence, replaced with `U+FFFD`.
    BadUtf8,
    /// Undefined `PDFDocEncoding` byte (one of the 24 unassigned Table D.2 codes), replaced with
    /// `U+FFFD`.
    Undefined(u8),
    /// Language escape opened by `0x00 0x1B` or `0x1B` without a matching closing delimiter.
    UnclosedEscape,
    /// Language escape whose tag is not empty, 2 ASCII letters, or 4 ASCII letters.
    MalformedEscape,
}

/// Decodes `bytes` as an ISO 32000-2 §7.9.2.2 PDF text string.
#[must_use]
pub fn decode(bytes: &[u8]) -> Text {
    let mut quirks = Vec::new();
    let mut languages = Vec::new();
    let (text, encoding) = match bytes {
        [0xFE, 0xFF, rest @ ..] => (
            decode_iso_utf16be(rest, &mut languages, &mut quirks),
            Encoding::Utf16Be,
        ),
        [0xEF, 0xBB, 0xBF, rest @ ..] => (
            decode_iso_utf8(rest, &mut languages, &mut quirks),
            Encoding::Utf8,
        ),
        _ => (decode_iso_pdf_doc(bytes, &mut quirks), Encoding::PdfDoc),
    };
    Text {
        text,
        encoding,
        languages,
        quirks,
    }
}

/// Decodes `bytes` into a `String` using [`decode`] for [`Rules::Iso`] and [`pdfium::decode`] for
/// [`Rules::Pdfium`].
#[must_use]
pub fn decode_str(bytes: &[u8], rules: Rules) -> String {
    match rules {
        Rules::Pdfium => pdfium::decode(bytes).text,
        _ => decode(bytes).text,
    }
}

fn is_valid_language_tag(tag: &[u8]) -> bool {
    matches!(tag.len(), 0 | 2 | 4) && tag.iter().all(u8::is_ascii_alphabetic)
}

fn decode_iso_utf16be(
    mut rest: &[u8],
    languages: &mut Vec<String>,
    quirks: &mut Vec<Quirk>,
) -> String {
    let mut text = String::with_capacity(rest.len() / 2);
    let mut pending_high: Option<u16> = None;

    while rest.len() >= 2 {
        if rest[0] == 0x00 && rest[1] == 0x1B {
            if pending_high.take().is_some() {
                quirks.push(Quirk::LoneSurrogate);
                text.push(char::REPLACEMENT_CHARACTER);
            }
            let after_open = &rest[2..];
            let Some(tag_len) = after_open.windows(2).position(|w| w == [0x00, 0x1B]) else {
                quirks.push(Quirk::UnclosedEscape);
                return text;
            };
            let tag = &after_open[..tag_len];
            if !is_valid_language_tag(tag) {
                quirks.push(Quirk::MalformedEscape);
            }
            languages.push(String::from_utf8_lossy(tag).into_owned());
            rest = &after_open[tag_len + 2..];
            continue;
        }

        let unit = u16::from_be_bytes([rest[0], rest[1]]);
        rest = &rest[2..];
        if (0xD800..=0xDBFF).contains(&unit) {
            if pending_high.replace(unit).is_some() {
                quirks.push(Quirk::LoneSurrogate);
                text.push(char::REPLACEMENT_CHARACTER);
            }
        } else if (0xDC00..=0xDFFF).contains(&unit) {
            if let Some(high) = pending_high.take() {
                let code =
                    0x10000 + (((u32::from(high) - 0xD800) << 10) | (u32::from(unit) - 0xDC00));
                text.push(char::from_u32(code).expect("valid UTF-16 surrogate pair"));
            } else {
                quirks.push(Quirk::LoneSurrogate);
                text.push(char::REPLACEMENT_CHARACTER);
            }
        } else {
            if pending_high.take().is_some() {
                quirks.push(Quirk::LoneSurrogate);
                text.push(char::REPLACEMENT_CHARACTER);
            }
            text.push(char::from_u32(u32::from(unit)).expect("BMP scalar value"));
        }
    }

    if pending_high.is_some() {
        quirks.push(Quirk::LoneSurrogate);
        text.push(char::REPLACEMENT_CHARACTER);
    }
    if !rest.is_empty() {
        quirks.push(Quirk::OddByte);
    }
    text
}

fn decode_iso_utf8(
    mut rest: &[u8],
    languages: &mut Vec<String>,
    quirks: &mut Vec<Quirk>,
) -> String {
    if memchr::memchr(0x1B, rest).is_none() {
        return match core::str::from_utf8(rest) {
            Ok(valid) => String::from(valid),
            Err(_) => {
                quirks.push(Quirk::BadUtf8);
                String::from_utf8_lossy(rest).into_owned()
            }
        };
    }

    let mut text = String::with_capacity(rest.len());
    let mut bad_utf8 = false;
    while let Some(esc_pos) = memchr::memchr(0x1B, rest) {
        let chunk = &rest[..esc_pos];
        match core::str::from_utf8(chunk) {
            Ok(valid) => text.push_str(valid),
            Err(_) => {
                bad_utf8 = true;
                text.push_str(&String::from_utf8_lossy(chunk));
            }
        }
        let after_open = &rest[esc_pos + 1..];
        let Some(tag_len) = memchr::memchr(0x1B, after_open) else {
            if bad_utf8 {
                quirks.push(Quirk::BadUtf8);
            }
            quirks.push(Quirk::UnclosedEscape);
            return text;
        };
        let tag = &after_open[..tag_len];
        if !is_valid_language_tag(tag) {
            quirks.push(Quirk::MalformedEscape);
        }
        languages.push(String::from_utf8_lossy(tag).into_owned());
        rest = &after_open[tag_len + 1..];
    }

    match core::str::from_utf8(rest) {
        Ok(valid) => text.push_str(valid),
        Err(_) => {
            bad_utf8 = true;
            text.push_str(&String::from_utf8_lossy(rest));
        }
    }
    if bad_utf8 {
        quirks.push(Quirk::BadUtf8);
    }
    text
}

fn decode_iso_pdf_doc(bytes: &[u8], quirks: &mut Vec<Quirk>) -> String {
    if bytes
        .iter()
        .all(|&b| matches!(b, 0x09 | 0x0A | 0x0D | 0x20..=0x7E))
    {
        return String::from(core::str::from_utf8(bytes).expect("verified ASCII subset"));
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        quirks.push(Quirk::LittleEndianMark);
    }
    let mut text = String::with_capacity(bytes.len());
    for &b in bytes {
        match pdf_doc_char(b) {
            Some(c) => text.push(c),
            None => {
                quirks.push(Quirk::Undefined(b));
                text.push(char::REPLACEMENT_CHARACTER);
            }
        }
    }
    text
}

/// Encodes `text` as an ISO 32000-2 §7.9.2.2 PDF text string.
///
/// Uses verbatim `PDFDocEncoding` when every byte is in `0x09 | 0x0A | 0x0D | 0x20..=0x7E`, and
/// `UTF-16BE` with BOM (`FE FF`) otherwise.
///
/// Note that `U+001B` is reserved as the §7.9.2.2.2 language-escape delimiter in PDF text strings;
/// use [`encode_as`] if you want `U+001B` rejected with [`Unencodable::Escape`].
#[must_use]
pub fn encode(text: &str) -> Vec<u8> {
    if is_verbatim_pdf_doc(text) {
        text.as_bytes().to_vec()
    } else {
        utf16be(text)
    }
}

/// Returns `true` if `text`'s UTF-8 bytes are verbatim `PDFDocEncoding` (`\t`, `\n`, `\r`, and
/// printable ASCII `0x20..=0x7E`).
#[inline]
pub(crate) fn is_verbatim_pdf_doc(text: &str) -> bool {
    text.bytes()
        .all(|b| matches!(b, 0x09 | 0x0A | 0x0D | 0x20..=0x7E))
}

fn utf16be(text: &str) -> Vec<u8> {
    let unit_count = text.chars().map(char::len_utf16).sum::<usize>();
    let mut bytes = Vec::with_capacity(2 + 2 * unit_count);
    bytes.extend_from_slice(&[0xFE, 0xFF]);
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    bytes
}

/// Encodes `text` in the caller-selected [`Encoding`].
///
/// # Errors
///
/// Returns [`Unencodable`] if `text` contains a character unrepresentable in `PDFDocEncoding`,
/// starts with bytes matching a BOM in `PDFDocEncoding`, or contains `U+001B` in `UTF-16BE` /
/// `UTF-8`.
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
            let mut bytes = Vec::with_capacity(text.len());
            for c in text.chars() {
                bytes.push(pdf_doc_byte(c).ok_or(Unencodable::Character(c))?);
            }
            match bytes.as_slice() {
                [0xFE, 0xFF, ..] | [0xFF, 0xFE, ..] | [0xEF, 0xBB, 0xBF, ..] => {
                    Err(Unencodable::Mark)
                }
                _ => Ok(bytes),
            }
        }
    }
}

/// Maps `c` to its ISO 32000-2 Annex D Table D.2 `PDFDocEncoding` byte, or `None` if undefined.
#[must_use]
pub const fn pdf_doc_byte(c: char) -> Option<u8> {
    match c {
        '\t' | '\n' | '\r' | ' '..='~' | '\u{A1}'..='\u{AC}' | '\u{AE}'..='\u{FF}' => Some(c as u8),
        '˘' => Some(0x18),
        'ˇ' => Some(0x19),
        'ˆ' => Some(0x1A),
        '˙' => Some(0x1B),
        '˝' => Some(0x1C),
        '˛' => Some(0x1D),
        '˚' => Some(0x1E),
        '˜' => Some(0x1F),
        '•' => Some(0x80),
        '†' => Some(0x81),
        '‡' => Some(0x82),
        '…' => Some(0x83),
        '—' => Some(0x84),
        '–' => Some(0x85),
        'ƒ' => Some(0x86),
        '⁄' => Some(0x87),
        '‹' => Some(0x88),
        '›' => Some(0x89),
        '−' => Some(0x8A),
        '‰' => Some(0x8B),
        '„' => Some(0x8C),
        '“' => Some(0x8D),
        '”' => Some(0x8E),
        '‘' => Some(0x8F),
        '’' => Some(0x90),
        '‚' => Some(0x91),
        '™' => Some(0x92),
        'ﬁ' => Some(0x93),
        'ﬂ' => Some(0x94),
        'Ł' => Some(0x95),
        'Œ' => Some(0x96),
        'Š' => Some(0x97),
        'Ÿ' => Some(0x98),
        'Ž' => Some(0x99),
        'ı' => Some(0x9A),
        'ł' => Some(0x9B),
        'œ' => Some(0x9C),
        'š' => Some(0x9D),
        'ž' => Some(0x9E),
        '€' => Some(0xA0),
        _ => None,
    }
}

/// Maps a `PDFDocEncoding` byte to its Unicode character per ISO 32000-2 Annex D Table D.2.
///
/// Returns `None` for the 24 undefined byte values (`0x00..=0x08`, `0x0B..=0x0C`, `0x0E..=0x17`,
/// `0x7F`, `0x9F`, and `0xAD`).
#[must_use]
pub const fn pdf_doc_char(b: u8) -> Option<char> {
    const ACCENTS: [char; 8] = ['˘', 'ˇ', 'ˆ', '˙', '˝', '˛', '˚', '˜'];
    const HIGH: [char; 33] = [
        '•', '†', '‡', '…', '—', '–', 'ƒ', '⁄', '‹', '›', '−', '‰', '„', '“', '”', '‘', '’', '‚',
        '™', 'ﬁ', 'ﬂ', 'Ł', 'Œ', 'Š', 'Ÿ', 'Ž', 'ı', 'ł', 'œ', 'š', 'ž', '\u{FFFD}', '€',
    ];
    match b {
        0x00..=0x08 | 0x0B..=0x0C | 0x0E..=0x17 | 0x7F | 0x9F | 0xAD => None,
        0x18..=0x1F => Some(ACCENTS[(b - 0x18) as usize]),
        0x80..=0xA0 => Some(HIGH[(b - 0x80) as usize]),
        _ => Some(b as char),
    }
}

/// PDFium-compatible text string decoder (verified against PDFium as of chromium/8152).
///
/// Differs from strict ISO 32000-2 ([`decode`]) as follows:
/// - `FF FE` is treated as a `UTF-16LE` BOM ([`pdfium::Encoding::Utf16Le`]);
/// - Language escapes strip any UTF-16 code units between `0x001B` delimiters, allowing split
///   surrogate halves to pair across an escape ([`pdfium::Quirk::SplitSurrogate`]);
/// - Invalid `UTF-8` sequences are skipped or overlong-decoded per PDFium's byte state machine
///   rather than replaced with `U+FFFD`;
/// - `PDFDocEncoding` maps `0x00..=0x17` to `U+0000..=U+0017` and maps the 3 unassigned slots
///   (`0x7F`, `0x9F`, `0xAD`) to `U+0000`.
pub mod pdfium {
    use alloc::string::String;
    use alloc::vec::Vec;

    /// Result of decoding a PDF text string under PDFium (`chromium/8152`) rules.
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[non_exhaustive]
    pub struct Text {
        /// Decoded Unicode text (lone surrogates become `U+FFFD`, as Rust `String`s require valid
        /// UTF-8).
        pub text: String,
        /// Encoding selected by the BOM.
        pub encoding: Encoding,
        /// Departures from strict ISO 32000-2 observed while decoding.
        pub quirks: Vec<Quirk>,
    }

    /// Encodings recognized by PDFium's text string decoder.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    #[non_exhaustive]
    pub enum Encoding {
        /// `UTF-16BE` following `FE FF`.
        Utf16Be,
        /// `UTF-16LE` following `FF FE` (PDFium extension).
        Utf16Le,
        /// `UTF-8` following `EF BB BF`.
        Utf8,
        /// `PDFDocEncoding` when no BOM is present.
        PdfDoc,
    }

    /// Spec departures observed by [`decode`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    #[non_exhaustive]
    pub enum Quirk {
        /// Starts with `FF FE` (`UTF-16LE` BOM).
        LittleEndianMark,
        /// Trailing odd byte in `UTF-16`, omitted.
        OddByte,
        /// Unpaired UTF-16 surrogate code unit, replaced with `U+FFFD`.
        LoneSurrogate,
        /// Surrogate pair whose high and low halves were separated by a language escape.
        SplitSurrogate,
        /// Input after `EF BB BF` was not valid `UTF-8`.
        BadUtf8,
        /// `0x7F`, `0x9F`, or `0xAD` in `PDFDocEncoding`, mapped to `U+0000`.
        Undefined(u8),
        /// Language escape opened by `U+001B` with no closing `U+001B`.
        UnclosedEscape,
    }

    /// Decodes `bytes` according to PDFium's text string rules as of chromium/8152.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Text {
        let mut quirks = Vec::new();
        match bytes {
            [0xFE, 0xFF, rest @ ..] => {
                let text = decode_utf16(rest, u16::from_be_bytes, &mut quirks);
                Text {
                    text,
                    encoding: Encoding::Utf16Be,
                    quirks,
                }
            }
            [0xFF, 0xFE, rest @ ..] => {
                quirks.push(Quirk::LittleEndianMark);
                let text = decode_utf16(rest, u16::from_le_bytes, &mut quirks);
                Text {
                    text,
                    encoding: Encoding::Utf16Le,
                    quirks,
                }
            }
            [0xEF, 0xBB, 0xBF, rest @ ..] => {
                if memchr::memchr(0x1B, rest).is_none()
                    && let Ok(valid) = core::str::from_utf8(rest)
                {
                    return Text {
                        text: String::from(valid),
                        encoding: Encoding::Utf8,
                        quirks,
                    };
                }
                if core::str::from_utf8(rest).is_err() {
                    quirks.push(Quirk::BadUtf8);
                }
                let units = utf8_to_units_as_pdfium(rest);
                let text = decode_units(units.iter().copied(), units.len(), &mut quirks);
                Text {
                    text,
                    encoding: Encoding::Utf8,
                    quirks,
                }
            }
            _ => {
                let text = decode_pdf_doc(bytes, &mut quirks);
                Text {
                    text,
                    encoding: Encoding::PdfDoc,
                    quirks,
                }
            }
        }
    }

    /// Maps a byte under PDFium's `kPDFDocEncoding` table (`None` only for `0x7F`, `0x9F`, `0xAD`).
    #[must_use]
    pub const fn pdf_doc_char(b: u8) -> Option<char> {
        match b {
            0x7F | 0x9F | 0xAD => None,
            0x00..=0x17 => Some(b as char),
            _ => super::pdf_doc_char(b),
        }
    }

    fn decode_pdf_doc(bytes: &[u8], quirks: &mut Vec<Quirk>) -> String {
        if bytes
            .iter()
            .all(|&b| matches!(b, 0x00..=0x17 | 0x20..=0x7E))
        {
            return String::from(core::str::from_utf8(bytes).expect("verified ASCII subset"));
        }
        let mut text = String::with_capacity(bytes.len());
        for &b in bytes {
            match pdf_doc_char(b) {
                Some(c) => text.push(c),
                None => {
                    quirks.push(Quirk::Undefined(b));
                    text.push('\0');
                }
            }
        }
        text
    }

    fn decode_utf16(
        bytes: &[u8],
        from_pair: fn([u8; 2]) -> u16,
        quirks: &mut Vec<Quirk>,
    ) -> String {
        let (pairs, odd) = bytes.as_chunks::<2>();
        if !odd.is_empty() {
            quirks.push(Quirk::OddByte);
        }
        decode_units(pairs.iter().map(|&p| from_pair(p)), pairs.len(), quirks)
    }

    fn decode_units<I: Iterator<Item = u16>>(
        units: I,
        cap: usize,
        quirks: &mut Vec<Quirk>,
    ) -> String {
        let mut text = String::with_capacity(cap);
        let mut escaped = false;
        let mut pending_high: Option<(u16, bool)> = None;

        for unit in units {
            if unit == 0x001B {
                escaped = !escaped;
                if let Some((_, split)) = &mut pending_high {
                    *split = true;
                }
                continue;
            }
            if escaped {
                continue;
            }
            if (0xD800..=0xDBFF).contains(&unit) {
                if pending_high.replace((unit, false)).is_some() {
                    quirks.push(Quirk::LoneSurrogate);
                    text.push(char::REPLACEMENT_CHARACTER);
                }
            } else if (0xDC00..=0xDFFF).contains(&unit) {
                if let Some((high, split)) = pending_high.take() {
                    if split {
                        quirks.push(Quirk::SplitSurrogate);
                    }
                    let code =
                        0x10000 + (((u32::from(high) - 0xD800) << 10) | (u32::from(unit) - 0xDC00));
                    text.push(char::from_u32(code).expect("valid surrogate pair"));
                } else {
                    quirks.push(Quirk::LoneSurrogate);
                    text.push(char::REPLACEMENT_CHARACTER);
                }
            } else {
                if pending_high.take().is_some() {
                    quirks.push(Quirk::LoneSurrogate);
                    text.push(char::REPLACEMENT_CHARACTER);
                }
                text.push(char::from_u32(u32::from(unit)).expect("BMP scalar value"));
            }
        }

        if pending_high.is_some() {
            quirks.push(Quirk::LoneSurrogate);
            text.push(char::REPLACEMENT_CHARACTER);
        }
        if escaped {
            quirks.push(Quirk::UnclosedEscape);
        }
        text
    }

    fn utf8_to_units_as_pdfium(bytes: &[u8]) -> Vec<u16> {
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
                    code = (code << 6) | u32::from(b & 0x3F);
                    if pending == 0 {
                        match char::from_u32(code) {
                            Some(c) => units.extend_from_slice(c.encode_utf16(&mut [0; 2])),
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
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::string::ToString;

    use super::*;

    fn be(units: &[u16]) -> Vec<u8> {
        let bytes = units.iter().flat_map(|u| u.to_be_bytes());
        [0xFE, 0xFF].into_iter().chain(bytes).collect()
    }

    #[test]
    fn iso_decodes_each_encoding_and_language_escapes() {
        let utf16 = decode(b"\xFE\xFF\x00\x1BenUS\x00\x1B\x00H\x00i\x00\x1B\x00\x1B\x00!");
        assert_eq!(utf16.encoding, Encoding::Utf16Be);
        assert_eq!(utf16.text, "Hi!");
        assert_eq!(utf16.languages, ["enUS", ""]);
        assert!(utf16.quirks.is_empty());

        let utf8 = decode(b"\xEF\xBB\xBF\x1Bfr\x1Bcaf\xC3\xA9\x1B\x1B");
        assert_eq!(utf8.encoding, Encoding::Utf8);
        assert_eq!(utf8.text, "café");
        assert_eq!(utf8.languages, ["fr", ""]);
        assert!(utf8.quirks.is_empty());

        let pdf_doc = decode(b"caf\xE9 \x84 \x92 \x18");
        assert_eq!(pdf_doc.encoding, Encoding::PdfDoc);
        assert_eq!(pdf_doc.text, "café — ™ ˘");
        assert!(pdf_doc.quirks.is_empty());
    }

    #[test]
    fn iso_reports_quirks_and_24_undefined_table_d2_bytes() {
        let undefined_count = (0..=u8::MAX).filter(|&b| pdf_doc_char(b).is_none()).count();
        assert_eq!(undefined_count, 24);

        let odd = decode(&[0xFE, 0xFF, 0x00, 0x41, 0x00]);
        assert_eq!(
            (odd.text.as_str(), &odd.quirks[..]),
            ("A", &[Quirk::OddByte][..])
        );

        let lone = decode(&[0xFE, 0xFF, 0xD8, 0x00]);
        assert_eq!(lone.quirks, [Quirk::LoneSurrogate]);

        let split_iso = decode(b"\xFE\xFF\xD8\x3D\x00\x1Ben\x00\x1B\xDE\x00");
        assert_eq!(split_iso.text, "\u{FFFD}\u{FFFD}");
        assert_eq!(
            split_iso.quirks,
            [Quirk::LoneSurrogate, Quirk::LoneSurrogate]
        );

        let unclosed_16 = decode(b"\xFE\xFF\x00A\x00\x1Ben");
        assert_eq!(unclosed_16.text, "A");
        assert_eq!(unclosed_16.quirks, [Quirk::UnclosedEscape]);

        let malformed_16 = decode(b"\xFE\xFF\x00\x1B123\x00\x1B\x00A");
        assert_eq!(malformed_16.text, "A");
        assert_eq!(malformed_16.quirks, [Quirk::MalformedEscape]);

        let unclosed_8 = decode(b"\xEF\xBB\xBFA\x1Ben");
        assert_eq!(unclosed_8.text, "A");
        assert_eq!(unclosed_8.quirks, [Quirk::UnclosedEscape]);

        let malformed_8 = decode(b"\xEF\xBB\xBF\x1Be\x1BA");
        assert_eq!(malformed_8.text, "A");
        assert_eq!(malformed_8.quirks, [Quirk::MalformedEscape]);

        assert_eq!(decode(b"\xEF\xBB\xBF\xFF").quirks, [Quirk::BadUtf8]);

        let little = decode(&[0xFF, 0xFE, 0x41, 0x00]);
        assert_eq!(little.text, "ÿþA\u{FFFD}");
        assert_eq!(
            little.quirks,
            [Quirk::LittleEndianMark, Quirk::Undefined(0x00)]
        );
    }

    #[test]
    fn pdfium_codec_utf16le_escapes_split_surrogate_and_bad_utf8() {
        let le = pdfium::decode(b"\xFF\xFEA\x00\xE9\x00");
        assert_eq!(le.text, "Aé");
        assert_eq!(le.encoding, pdfium::Encoding::Utf16Le);
        assert_eq!(le.quirks, [pdfium::Quirk::LittleEndianMark]);

        let odd = pdfium::decode(b"\xFF\xFEA\x00\x00");
        assert_eq!(odd.text, "A");
        assert_eq!(
            odd.quirks,
            [pdfium::Quirk::LittleEndianMark, pdfium::Quirk::OddByte]
        );

        // Surrogate pair split across a language escape joins and reports SplitSurrogate.
        let split = pdfium::decode(&be(&[0xD83D, 0x1B, 0x1B, 0xDE00]));
        assert_eq!(split.text, "😀");
        assert_eq!(split.quirks, [pdfium::Quirk::SplitSurrogate]);

        // Unpaired surrogate before an escape still reports LoneSurrogate.
        let lone = pdfium::decode(&be(&[0xD83D, 0x1B, 0x1B, 0x0041]));
        assert_eq!(lone.text, "\u{FFFD}A");
        assert_eq!(lone.quirks, [pdfium::Quirk::LoneSurrogate]);

        // PDFDoc undefined bytes become NUL in PDFium, while 0x00..=0x17 stay C0 controls.
        let c0 = pdfium::decode(b"a\0\x17b\x7F\x9F\xAD");
        assert_eq!(c0.text, "a\0\x17b\0\0\0");
        assert_eq!(
            c0.quirks,
            [
                pdfium::Quirk::Undefined(0x7F),
                pdfium::Quirk::Undefined(0x9F),
                pdfium::Quirk::Undefined(0xAD),
            ]
        );

        for (bytes, expected) in [
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
            let read = pdfium::decode(&[b"\xEF\xBB\xBF", bytes].concat());
            assert_eq!(read.text, expected, "{bytes:02X?}");
            assert_eq!(read.encoding, pdfium::Encoding::Utf8);
            assert_eq!(read.quirks.first(), Some(&pdfium::Quirk::BadUtf8));
        }

        let good = pdfium::decode(b"\xEF\xBB\xBF\xF0\x9F\x98\x80caf\xC3\xA9");
        assert_eq!((good.text.as_str(), &good.quirks[..]), ("😀café", &[][..]));
    }

    #[test]
    fn encode_round_trips_under_both_iso_and_pdfium() {
        assert_eq!(encode("plain (text)\n\t\r"), b"plain (text)\n\t\r");
        assert_eq!(encode("\0"), b"\xFE\xFF\x00\x00");
        assert_eq!(encode("\x17 "), b"\xFE\xFF\x00\x17\x00 ");
        assert_eq!(encode("é"), b"\xFE\xFF\x00\xE9");
        assert_eq!(encode("😀"), b"\xFE\xFF\xD8\x3D\xDE\x00");
        assert_eq!(encode("þÿ"), b"\xFE\xFF\x00\xFE\x00\xFF");

        let samples = [
            '\0', '\u{01}', '\t', '\n', '\r', '\u{17}', '\u{18}', '\u{1A}', '\u{1C}', 'A',
            '\u{7F}', '\u{9F}', '\u{AD}', 'é', 'þ', 'ÿ', '€', '\u{FFFD}', '\u{FEFF}', '\u{FFFE}',
            '😀',
        ];
        for a in samples {
            for b in samples {
                let s: String = [a, b].into_iter().collect();
                let encoded = encode(&s);
                let iso = decode(&encoded);
                assert_eq!(iso.text, s);
                assert!(iso.quirks.is_empty());
                let pv = pdfium::decode(&encoded);
                assert_eq!(pv.text, s);
                assert!(pv.quirks.is_empty());
                assert_eq!(decode_str(&encoded, Rules::Iso), s);
                assert_eq!(decode_str(&encoded, Rules::Pdfium), s);
            }
        }
    }

    #[test]
    fn encode_as_and_pdf_doc_table_bijection() {
        for b in 0..=u8::MAX {
            match pdf_doc_char(b) {
                Some(c) => {
                    assert_eq!(pdf_doc_byte(c), Some(b), "{b:02X}");
                    let s = c.to_string();
                    let raw = encode_as(&s, Encoding::PdfDoc).unwrap();
                    assert_eq!(raw, [b]);
                    assert_eq!(decode(&raw).text, s);
                    assert_eq!(pdfium::decode(&raw).text, s);
                }
                None => assert_eq!(pdf_doc_byte(b as char), None, "{b:02X}"),
            }
        }
        assert_eq!(pdf_doc_byte('\u{FFFD}'), None);

        for s in ["þÿ", "þÿa", "ï»¿", "ï»¿a", "ÿþ", "ÿþa"] {
            assert_eq!(
                encode_as(s, Encoding::PdfDoc),
                Err(Unencodable::Mark),
                "{s}"
            );
        }
        for c in ['\0', '\u{17}', '\u{1B}', '\u{7F}', '\u{9F}', '\u{AD}', '😀'] {
            assert_eq!(
                encode_as(&format!("a{c}"), Encoding::PdfDoc),
                Err(Unencodable::Character(c))
            );
        }
        for enc in [Encoding::Utf16Be, Encoding::Utf8] {
            assert_eq!(encode_as("a\u{1B}", enc), Err(Unencodable::Escape));
        }
        assert_eq!(
            encode_as("é", Encoding::Utf8),
            Ok(b"\xEF\xBB\xBF\xC3\xA9".to_vec())
        );
    }
}
