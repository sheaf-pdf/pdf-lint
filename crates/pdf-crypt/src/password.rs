//! A password as typed, and the bytes tried for it: its SASLprep form (RFC
//! 4013) and UTF-8, which revisions 5 and 6 take, and the single-byte
//! encodings a password of revisions 2 to 4 may have been set in (ISO
//! 32000-2, 7.6.4.3.2, 7.6.4.3.3 and Annex D); and the one form a new file's
//! password takes.

use std::borrow::Cow;
use std::fmt;

use stringprep::saslprep;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

/// A password as typed: text, before any encoding. A reader tries it in
/// each encoding it may have been set in ([`Password::encodings`]); a new
/// file takes it in one, strictly ([`Password::prepared`]). Its `Debug`
/// leaves the text out.
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
    /// SASLprep's form of the text (RFC 4013), in UTF-8: what revisions 5
    /// and 6 take (ISO 32000-2, 7.6.4.3.3).
    Saslprep,
    /// UTF-8, as typed: what revisions 5 and 6 take where SASLprep leaves
    /// the text as it is, and what writers that skip SASLprep set.
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
    /// SASLprep's form from 5 to 6, whose passwords are text; the
    /// single-byte encodings from 2 to 4, whose passwords are bytes; UTF-8,
    /// and bytes as given, in any.
    pub fn fits(self, revision: i64) -> bool {
        match self {
            Encoding::Saslprep => (5..=6).contains(&revision),
            Encoding::PdfDoc | Encoding::WinAnsi | Encoding::MacRoman => {
                (2..=4).contains(&revision)
            }
            Encoding::Utf8 | Encoding::Verbatim => true,
        }
    }
}

/// A password's bytes, in one encoding of its text
/// ([`Password::encodings`], [`Password::prepared`]) or as they were given
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
    /// The bytes it may have been set as, in the order a reader tries them,
    /// leniently: its SASLprep form (RFC 4013), in UTF-8, which revisions 5
    /// and 6 take (ISO 32000-2, 7.6.4.3.3), where SASLprep takes the text
    /// and makes it another; as typed, in UTF-8, as writers that skip
    /// SASLprep set it; then, for revisions 2 to 4, whose passwords are
    /// bytes the spec has made from PDFDocEncoding (7.6.4.3.2) but writers
    /// from whatever their system had, the text composed (NFC, so that
    /// accents typed apart from their letters have a byte) in
    /// PDFDocEncoding, WinAnsiEncoding and MacRomanEncoding (Annex D), each
    /// where every character has a byte, as qpdf tries them. A password
    /// SASLprep refuses (with an emoji, unassigned in the Unicode SASLprep
    /// is of, or a control character) has the rest alone.
    ///
    /// None twice among those a revision takes first ([`Encoding::fits`]):
    /// a single-byte form is left out where UTF-8's or one before it has its
    /// bytes, but not for SASLprep's, which no revision takes beside it.
    /// [`Handler::open`](crate::Handler::open) tries no bytes twice.
    pub fn encodings(&self) -> Vec<Encoded> {
        let text = self.0;
        let mut encodings = Vec::with_capacity(5);
        // SASLprep borrows the text it leaves as it is (ASCII), and may
        // give text it made the same.
        if let Ok(Cow::Owned(form)) = saslprep(text) {
            let form = Zeroizing::new(form.into_bytes());
            if form[..] != *text.as_bytes() {
                encodings.push(Encoded {
                    encoding: Encoding::Saslprep,
                    bytes: form,
                });
            }
        }
        let typed = encodings.len();
        encodings.push(Encoded::new(Encoding::Utf8, text.as_bytes()));
        let tables = [Encoding::PdfDoc, Encoding::WinAnsi, Encoding::MacRoman];
        // A byte a character, so sized at once.
        let len = text.nfc().count();
        'tables: for encoding in tables {
            let mut bytes = Zeroizing::new(Vec::with_capacity(len));
            for c in text.nfc() {
                match byte(encoding, c) {
                    Some(b) => bytes.push(b),
                    None => continue 'tables,
                }
            }
            if !encodings[typed..]
                .iter()
                .any(|e| e.as_bytes() == &bytes[..])
            {
                encodings.push(Encoded { encoding, bytes });
            }
        }
        encodings
    }

    /// The bytes a new file's password is set as, strictly, which
    /// [`Handler::create`](crate::Handler::create) takes: its SASLprep form
    /// (RFC 4013) in UTF-8, as revisions 5 and 6 take it (ISO 32000-2,
    /// 7.6.4.3.3), tagged [`Encoding::Saslprep`]; or why it can't be. Not a
    /// password SASLprep refuses, the character named where one is why
    /// ([`Unusable::Prohibited`], [`Unusable::Bidi`]); nor the empty one,
    /// one that prepares to nothing, or one that prepares to over 127
    /// bytes, which a reader would cut.
    pub fn prepared(&self) -> Result<Encoded, Unusable> {
        let text = self.0;
        if text.is_empty() {
            return Err(Unusable::Empty);
        }
        let bytes = match saslprep(text) {
            Ok(Cow::Borrowed(form)) => Zeroizing::new(form.as_bytes().to_vec()),
            Ok(Cow::Owned(form)) => Zeroizing::new(form.into_bytes()),
            Err(_) => return Err(refused(text)),
        };
        match bytes.len() {
            0 => Err(Unusable::Vanished),
            len @ 128.. => Err(Unusable::TooLong { len }),
            _ => Ok(Encoded {
                encoding: Encoding::Saslprep,
                bytes,
            }),
        }
    }
}

/// Why a password can't be a new file's ([`Password::prepared`],
/// [`Handler::create`](crate::Handler::create)).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Unusable {
    /// A character SASLprep doesn't allow (RFC 4013, 2.3 and 2.5): a
    /// control or private-use character, say, or one unassigned in the
    /// Unicode it's of, 3.2, as an emoji is. The first of the password's
    /// that it refuses whatever is around it.
    #[error("the password has U+{:04X}, a character SASLprep doesn't allow", u32::from(*.0))]
    Prohibited(char),
    /// Right-to-left text SASLprep doesn't allow (RFC 4013, 2.4): mixed
    /// with left-to-right text, or not at both ends of the password.
    #[error(
        "the password has right-to-left text that SASLprep doesn't allow: mixed with left-to-right, or not at both its ends"
    )]
    Bidi,
    /// Over 127 bytes prepared, which revisions 5 and 6 would cut to 127
    /// (ISO 32000-2, 7.6.4.3.3).
    #[error("the password is {len} bytes prepared, over 127")]
    TooLong {
        /// Its length prepared.
        len: usize,
    },
    /// Nothing prepared, though it isn't empty: each of its characters one
    /// SASLprep maps to nothing (a soft hyphen, a zero-width joiner).
    #[error("the password is nothing prepared: SASLprep maps each of its characters to nothing")]
    Vanished,
    /// The empty password.
    #[error("the password is empty")]
    Empty,
    /// Bytes in this encoding, not SASLprep's form, which a new file's
    /// password must be in.
    #[error("the password is in {0:?}, not prepared by SASLprep")]
    Unprepared(Encoding),
}

/// Why SASLprep refuses `text`: the first of its characters it refuses
/// whatever is around it, alone and between two right-to-left letters, as
/// it does a character it doesn't allow; else the bidi rule, which a
/// character can break alone (a Hebrew letter composed with its point,
/// which ends it) but none breaks whatever is around it. stringprep's error
/// says neither, and its text is of the text SASLprep made, not the typed.
fn refused(text: &str) -> Unusable {
    // A character's UTF-8 between two alefs' (U+05D0), on the stack.
    let mut around = Zeroizing::new([0xD7, 0x90, 0, 0, 0, 0, 0, 0]);
    let mut prohibited = |c: char| {
        let len = c.encode_utf8(&mut around[2..6]).len();
        around[2 + len..4 + len].copy_from_slice(&[0xD7, 0x90]);
        let alone = std::str::from_utf8(&around[2..2 + len]).expect("a character");
        let between = std::str::from_utf8(&around[..4 + len]).expect("characters");
        saslprep(alone).is_err() && saslprep(between).is_err()
    };
    text.chars()
        .find(|&c| prohibited(c))
        .map_or(Unusable::Bidi, Unusable::Prohibited)
}

/// `c`'s byte in the single-byte `encoding`, if it has one.
fn byte(encoding: Encoding, c: char) -> Option<u8> {
    match encoding {
        Encoding::PdfDoc => pdf_doc_byte(c),
        Encoding::WinAnsi => win_ansi_byte(c),
        Encoding::MacRoman => mac_roman_byte(c),
        Encoding::Saslprep | Encoding::Utf8 | Encoding::Verbatim => None,
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

    /// SASLprep's form fits revisions 5 and 6 alone; the single-byte
    /// encodings, 2 to 4 alone; UTF-8 and bytes as given, any.
    #[test]
    fn each_encoding_fits_the_revisions_it_can_have_been_set_for() {
        for r in 2..=6 {
            let old = r <= 4;
            assert!(Encoding::Utf8.fits(r) && Encoding::Verbatim.fits(r), "{r}");
            assert_eq!(Encoding::Saslprep.fits(r), !old, "{r}");
            for e in [Encoding::PdfDoc, Encoding::WinAnsi, Encoding::MacRoman] {
                assert_eq!(e.fits(r), old, "{e:?} {r}");
            }
        }
    }

    /// RFC 4013's examples (section 3), prepared as the RFC has them, each
    /// its output or an error; and read, SASLprep's form first where it
    /// isn't the text as typed, and where SASLprep refuses the text, the
    /// rest alone.
    #[test]
    fn rfc_4013s_examples_prepare_as_the_rfc_has_them() {
        use Encoding::{MacRoman, PdfDoc, Saslprep, Utf8, WinAnsi};
        let prepared =
            |text: &str| (Password(text).prepared()).map(|e| (e.encoding(), e.as_bytes().to_vec()));
        let form = |encoding: Encoding, bytes: &[u8]| (encoding, bytes.to_vec());
        // 1: a soft hyphen mapped to nothing.
        assert_eq!(prepared("I\u{AD}X"), Ok(form(Saslprep, b"IX")));
        // 2: no transformation.
        assert_eq!(prepared("user"), Ok(form(Saslprep, b"user")));
        // 3: case preserved, so it won't match 2's.
        assert_eq!(prepared("USER"), Ok(form(Saslprep, b"USER")));
        // 4: the output is NFKC, the input in ISO 8859-1.
        assert_eq!(prepared("\u{AA}"), Ok(form(Saslprep, b"a")));
        // 5: the output is NFKC, so it matches 1's.
        assert_eq!(prepared("\u{2168}"), Ok(form(Saslprep, b"IX")));
        // 6: a prohibited character.
        assert_eq!(prepared("\u{7}"), Err(Unusable::Prohibited('\u{7}')));
        // 7: the bidirectional check.
        assert_eq!(prepared("\u{627}1"), Err(Unusable::Bidi));
        // Read, each as the reader tries it. U+00AD is no character of
        // PDFDocEncoding's or MacRomanEncoding's; U+00AA is AA in
        // PDFDocEncoding (and WinAnsiEncoding, alike), BB in
        // MacRomanEncoding.
        assert_eq!(
            forms("I\u{AD}X"),
            [
                form(Saslprep, b"IX"),
                form(Utf8, "I\u{AD}X".as_bytes()),
                form(WinAnsi, b"I\xADX")
            ]
        );
        assert_eq!(forms("user"), [form(Utf8, b"user")]);
        assert_eq!(forms("USER"), [form(Utf8, b"USER")]);
        assert_eq!(
            forms("\u{AA}"),
            [
                form(Saslprep, b"a"),
                form(Utf8, "\u{AA}".as_bytes()),
                form(PdfDoc, b"\xAA"),
                form(MacRoman, b"\xBB")
            ]
        );
        assert_eq!(
            forms("\u{2168}"),
            [form(Saslprep, b"IX"), form(Utf8, "\u{2168}".as_bytes())]
        );
        assert_eq!(forms("\u{7}"), [form(Utf8, b"\x07")]);
        assert_eq!(forms("\u{627}1"), [form(Utf8, "\u{627}1".as_bytes())]);
    }

    /// The single-byte encodings take the text composed (NFC), as SASLprep
    /// composes it: an accent typed apart from its letter has a byte. A
    /// form is left out where one a revision takes first has its bytes,
    /// but a single-byte form isn't set beside SASLprep's, which no
    /// revision takes with it.
    #[test]
    fn the_single_byte_encodings_take_the_text_composed() {
        use Encoding::{MacRoman, PdfDoc, Saslprep, Utf8};
        let form = |encoding: Encoding, bytes: &[u8]| (encoding, bytes.to_vec());
        // é typed as e and U+0301: composed by SASLprep, then as typed, then
        // é in PDFDocEncoding (WinAnsiEncoding's alike), MacRomanEncoding.
        assert_eq!(
            forms("cafe\u{301}"),
            [
                form(Saslprep, "café".as_bytes()),
                form(Utf8, "cafe\u{301}".as_bytes()),
                form(PdfDoc, b"caf\xE9"),
                form(MacRoman, b"caf\x8E")
            ]
        );
        // The Kelvin sign: K composed and prepared, so SASLprep's K for
        // revisions 5 and 6, and PDFDocEncoding's for 2 to 4 (the others'
        // alike).
        assert_eq!(
            forms("\u{212A}"),
            [
                form(Saslprep, b"K"),
                form(Utf8, "\u{212A}".as_bytes()),
                form(PdfDoc, b"K")
            ]
        );
    }

    /// Each character of the three tables is its own NFC, alone and beside
    /// any other of theirs, and combines with nothing before it (its
    /// canonical combining class 0), so that text wholly in a table is its
    /// own NFC: composing the text never takes from a password a
    /// single-byte form it had as typed.
    #[test]
    fn the_tables_characters_are_their_own_nfc_alone_and_in_pairs() {
        use unicode_normalization::char::canonical_combining_class;
        let tables = [Encoding::PdfDoc, Encoding::WinAnsi, Encoding::MacRoman];
        let chars: Vec<char> = ('\u{1}'..='\u{FFFF}')
            .filter(|&c| tables.iter().any(|&e| byte(e, c).is_some()))
            .collect();
        // The three's characters together, but the null.
        assert_eq!(chars.len(), 263);
        for &c in &chars {
            assert!(c.to_string().nfc().eq([c]), "U+{:04X}", u32::from(c));
            assert_eq!(canonical_combining_class(c), 0, "U+{:04X}", u32::from(c));
        }
        for &a in &chars {
            for &b in &chars {
                assert!(
                    [a, b].iter().collect::<String>().nfc().eq([a, b]),
                    "U+{:04X} U+{:04X}",
                    u32::from(a),
                    u32::from(b)
                );
            }
        }
    }

    /// Each reason a password can't be a new file's, word for word.
    #[test]
    fn each_reason_a_password_cant_be_a_new_files_says_why() {
        let why = |text: &str| Password(text).prepared().unwrap_err();
        let reasons = [
            // A character unassigned in Unicode 3.2, and a control
            // character, each the first SASLprep refuses.
            (
                why("pass\u{1F954}\u{1}"),
                Unusable::Prohibited('\u{1F954}'),
                "the password has U+1F954, a character SASLprep doesn't allow",
            ),
            (
                why("a\u{AA}\u{1}b"),
                Unusable::Prohibited('\u{1}'),
                "the password has U+0001, a character SASLprep doesn't allow",
            ),
            // Right-to-left text and left-to-right; and a Hebrew letter
            // composed with its point, which ends the text: refused alone,
            // that letter breaks the bidi rule, which it doesn't between
            // two others.
            (
                why("\u{5D0}a"),
                Unusable::Bidi,
                "the password has right-to-left text that SASLprep doesn't allow: \
                 mixed with left-to-right, or not at both its ends",
            ),
            (
                why("\u{5D0}\u{FB44}"),
                Unusable::Bidi,
                "the password has right-to-left text that SASLprep doesn't allow: \
                 mixed with left-to-right, or not at both its ends",
            ),
            // 128 bytes prepared, 192 as typed.
            (
                why(&"\u{2168}".repeat(64)),
                Unusable::TooLong { len: 128 },
                "the password is 128 bytes prepared, over 127",
            ),
            (
                why("\u{AD}\u{200D}\u{FEFF}"),
                Unusable::Vanished,
                "the password is nothing prepared: SASLprep maps each of its characters to nothing",
            ),
            (why(""), Unusable::Empty, "the password is empty"),
            (
                Unusable::Unprepared(Encoding::Utf8),
                Unusable::Unprepared(Encoding::Utf8),
                "the password is in Utf8, not prepared by SASLprep",
            ),
        ];
        for (why, reason, text) in reasons {
            assert_eq!(why, reason);
            assert_eq!(why.to_string(), text);
        }
        // 127 bytes is a password.
        let long = "a".repeat(127);
        assert_eq!(
            Password(&long).prepared().unwrap().as_bytes(),
            long.as_bytes()
        );
        assert_eq!(
            Password(&"\u{2168}".repeat(63))
                .prepared()
                .unwrap()
                .as_bytes()
                .len(),
            126
        );
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
        for e in Password("se\u{301}cret").encodings() {
            assert!(!format!("{e:?}").contains("cret"), "{e:?}");
        }
        let prepared = Password("se\u{301}cret").prepared().unwrap();
        assert_eq!(
            format!("{prepared:?}"),
            "Encoded { encoding: Saslprep, .. }"
        );
    }
}
