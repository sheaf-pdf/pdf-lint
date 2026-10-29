//! A password as typed, and the bytes tried for it: in UTF-8, which
//! revisions 5 and 6 take, and in the single-byte encodings a password of
//! revisions 2 to 4 may have been set in (ISO 32000-2, 7.6.4.3.2 and Annex D).

use std::fmt;

use zeroize::Zeroizing;

/// A password as typed: text, before any encoding. A reader tries it in
/// each encoding it may have been set in ([`Password::encodings`]). Its
/// `Debug` leaves the text out.
#[derive(Clone, Copy)]
pub struct Password<'a>(pub &'a str);

impl fmt::Debug for Password<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(..)")
    }
}

/// How a password's text became the bytes an [`Encoded`] holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Encoding {
    /// UTF-8, as typed: what revisions 5 and 6 take (ISO 32000-2,
    /// 7.6.4.3.3).
    Utf8,
    /// PDFDocEncoding (Annex D), which revisions 2 to 4 take (7.6.4.3.2).
    PdfDoc,
    /// WinAnsiEncoding (Annex D), which writers have set revisions 2 to
    /// 4's passwords in.
    WinAnsi,
    /// MacRomanEncoding (Annex D), which writers have set revisions 2 to
    /// 4's passwords in.
    MacRoman,
    /// None: bytes as they were given ([`Encoded::verbatim`]).
    Verbatim,
}

impl Encoding {
    /// Whether a password of revision `revision` can have been set in it:
    /// the single-byte encodings from 2 to 4, whose passwords are bytes;
    /// UTF-8, and bytes as given, in any.
    pub fn fits(self, revision: i64) -> bool {
        match self {
            Encoding::PdfDoc | Encoding::WinAnsi | Encoding::MacRoman => {
                (2..=4).contains(&revision)
            }
            Encoding::Utf8 | Encoding::Verbatim => true,
        }
    }
}

/// A password's bytes, in one encoding of its text
/// ([`Password::encodings`]) or as they were given
/// ([`Encoded::verbatim`]): what [`Handler::open`](crate::Handler::open)
/// tries. They're wiped when it's dropped, and its `Debug` leaves them out.
#[derive(Clone)]
pub struct Encoded {
    encoding: Encoding,
    bytes: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for Encoded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Encoded")
            .field("encoding", &self.encoding)
            .finish_non_exhaustive()
    }
}

impl Encoded {
    /// `bytes`, in `encoding`.
    pub(crate) fn new(encoding: Encoding, bytes: &[u8]) -> Encoded {
        Encoded {
            encoding,
            bytes: Zeroizing::new(bytes.to_vec()),
        }
    }

    /// `bytes` as they are, in no encoding ([`Encoding::Verbatim`]): a
    /// password that's bytes, not text, or one the caller encoded.
    pub fn verbatim(bytes: &[u8]) -> Encoded {
        Encoded::new(Encoding::Verbatim, bytes)
    }

    /// The encoding its bytes are in.
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// Its bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl Password<'_> {
    /// The bytes it may have been set as, in the order a reader tries them:
    /// as typed, in UTF-8, which revisions 5 and 6 take; then, for
    /// revisions 2 to 4, whose passwords are bytes the spec has made from
    /// PDFDocEncoding (ISO 32000-2, 7.6.4.3.2) but writers from whatever
    /// their system had, in PDFDocEncoding, WinAnsiEncoding and
    /// MacRomanEncoding (Annex D), each where every character has a byte,
    /// as qpdf tries them. None twice.
    pub fn encodings(&self) -> Vec<Encoded> {
        let text = self.0;
        let mut encodings = vec![Encoded::new(Encoding::Utf8, text.as_bytes())];
        let tables = [Encoding::PdfDoc, Encoding::WinAnsi, Encoding::MacRoman];
        // A byte a character, so sized at once.
        let len = text.chars().count();
        'tables: for encoding in tables {
            let mut bytes = Zeroizing::new(Vec::with_capacity(len));
            for c in text.chars() {
                match byte(encoding, c) {
                    Some(b) => bytes.push(b),
                    None => continue 'tables,
                }
            }
            if !encodings.iter().any(|e| e.as_bytes() == &bytes[..]) {
                encodings.push(Encoded { encoding, bytes });
            }
        }
        encodings
    }
}

/// `c`'s byte in the single-byte `encoding`, if it has one.
fn byte(encoding: Encoding, c: char) -> Option<u8> {
    match encoding {
        Encoding::PdfDoc => pdf_doc_byte(c),
        Encoding::WinAnsi => win_ansi_byte(c),
        Encoding::MacRoman => mac_roman_byte(c),
        Encoding::Utf8 | Encoding::Verbatim => None,
    }
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

    /// Each of `password`'s encodings, its encoding and its bytes.
    fn forms(password: &str) -> Vec<(Encoding, Vec<u8>)> {
        (Password(password).encodings().iter())
            .map(|e| (e.encoding(), e.as_bytes().to_vec()))
            .collect()
    }

    #[test]
    fn a_password_as_typed_then_in_each_single_byte_encoding() {
        assert_eq!(forms("plain"), [(Encoding::Utf8, b"plain".to_vec())]);
        // é: E9 in PDFDocEncoding and WinAnsiEncoding, alike, so once; 8E
        // in MacRomanEncoding.
        assert_eq!(
            forms("café"),
            [
                (Encoding::Utf8, "café".as_bytes().to_vec()),
                (Encoding::PdfDoc, b"caf\xE9".to_vec()),
                (Encoding::MacRoman, b"caf\x8E".to_vec())
            ]
        );
        // The euro: A0, 80, and not in MacRomanEncoding, PDF's.
        assert_eq!(
            forms("€1"),
            [
                (Encoding::Utf8, "€1".as_bytes().to_vec()),
                (Encoding::PdfDoc, b"\xA01".to_vec()),
                (Encoding::WinAnsi, b"\x801".to_vec())
            ]
        );
        // What no single-byte encoding has: as typed only.
        assert_eq!(
            forms("日本"),
            [(Encoding::Utf8, "日本".as_bytes().to_vec())]
        );
        // The empty password, once.
        assert_eq!(forms(""), [(Encoding::Utf8, Vec::new())]);
        // Each table's own characters round-trip where it has them.
        for b in 0x80..=0xFF {
            if let Some(c) = pdf_doc_char(b) {
                assert_eq!(pdf_doc_byte(c), Some(b), "{b:#x}");
            }
        }
    }

    /// The single-byte encodings fit revisions 2 to 4 alone; UTF-8 and
    /// bytes as given, any.
    #[test]
    fn each_encoding_fits_the_revisions_it_can_have_been_set_for() {
        for r in 2..=6 {
            let old = r <= 4;
            assert!(Encoding::Utf8.fits(r) && Encoding::Verbatim.fits(r), "{r}");
            for e in [Encoding::PdfDoc, Encoding::WinAnsi, Encoding::MacRoman] {
                assert_eq!(e.fits(r), old, "{e:?} {r}");
            }
        }
    }

    /// Neither a password's `Debug` nor an encoding's says its text or
    /// bytes.
    #[test]
    fn their_debug_leaves_the_password_out() {
        assert_eq!(format!("{:?}", Password("secret")), "Password(..)");
        let encoded = Encoded::verbatim(b"secret");
        assert_eq!(encoded.as_bytes(), b"secret");
        assert_eq!(encoded.encoding(), Encoding::Verbatim);
        assert_eq!(format!("{encoded:?}"), "Encoded { encoding: Verbatim, .. }");
        for e in Password("sécret").encodings() {
            assert!(!format!("{e:?}").contains("cret"), "{e:?}");
        }
    }
}
