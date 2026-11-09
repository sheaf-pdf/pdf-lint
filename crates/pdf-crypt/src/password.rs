//! A password as typed, and the bytes tried for it: its SASLprep form (RFC
//! 4013) and UTF-8, which revisions 5 and 6 take, and the single-byte
//! encodings a password of revisions 2 to 4 may have been set in (ISO
//! 32000-2, 7.6.4.3.2, 7.6.4.3.3 and Annex D); and the one form a new file's
//! password takes.
//!
//! SASLprep is of Unicode 3.2 (RFC 3454, sections 4 and 7: normalization
//! form KC as UAX #15 3.2.0 has it, and 3.2's unassigned code points).
//! `stringprep` normalizes by the Unicode `unicode-normalization` is of, so
//! its form is taken back to 3.2's as UAX #15 has it done (section 11.3,
//! Guaranteeing Process Stability): Corrigendum #4's five characters mapped
//! first to the decompositions 3.2 gave them, a character 3.2 hadn't
//! assigned refused, and the text composed by 3.2's definition of a
//! blocked character, which Corrigendum #5 changed. Where the two forms
//! differ, a reader tries both, 3.2's first; a new file takes neither.

use std::borrow::Cow;
use std::fmt;

use stringprep::saslprep;
use stringprep::tables::{
    ascii_control_character, change_display_properties_or_deprecated,
    inappropriate_for_canonical_representation, inappropriate_for_plain_text,
    non_ascii_control_character, non_character_code_point, private_use, tagging_character,
    unassigned_code_point,
};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::{canonical_combining_class, compose};
use zeroize::Zeroizing;

use crate::Random;

/// A password as typed: text, before any encoding. A reader tries it in
/// each encoding it may have been set in ([`Password::encodings`]); a new
/// file takes it in one, strictly ([`Password::prepared`]). Its `Debug`
/// leaves the text out.
///
/// Exhaustive on purpose: a password is its text, built by a literal,
/// `Password(text)`, and taken apart by it, so a field more would be a
/// breaking release.
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
    /// SASLprep's form of the text (RFC 4013), as of Unicode 3.2, which
    /// SASLprep is of, in UTF-8: what revisions 5 and 6 take (ISO 32000-2,
    /// 7.6.4.3.3).
    Saslprep,
    /// SASLprep's form of the text by a later Unicode, the one
    /// `unicode-normalization` is of, in UTF-8, where it isn't 3.2's
    /// ([`Encoding::Saslprep`]): for the five characters Unicode's
    /// Corrigendum #4 remapped, the sequences Corrigendum #5 composes
    /// otherwise, and characters 3.2 hadn't assigned that a later Unicode
    /// decomposes into ones it had. What a writer that prepares a password
    /// by a later Unicode sets, as most SASLprep libraries do; never a new
    /// file's.
    SaslprepCurrentUnicode,
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
    /// SASLprep's forms from 5 to 6, whose passwords are text; the
    /// single-byte encodings from 2 to 4, whose passwords are bytes; UTF-8,
    /// and bytes as given, in any.
    #[must_use]
    pub const fn fits(self, revision: i64) -> bool {
        match self {
            Encoding::Saslprep | Encoding::SaslprepCurrentUnicode => matches!(revision, 5..=6),
            Encoding::PdfDoc | Encoding::WinAnsi | Encoding::MacRoman => matches!(revision, 2..=4),
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
    #[must_use]
    pub fn verbatim(bytes: &[u8]) -> Encoded {
        Encoded::new(Encoding::Verbatim, bytes)
    }

    /// The encoding its bytes are in.
    #[must_use]
    pub const fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// Its bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl Password<'_> {
    /// The longest text, in bytes as typed (UTF-8), that
    /// [`Password::prepared`] prepares: 1,024, eight times the 127 bytes
    /// revisions 5 and 6 keep of a password prepared (ISO 32000-2,
    /// 7.6.4.3.3), so there's room for combining marks and for characters
    /// SASLprep maps to nothing. A longer one is refused before any of it's
    /// prepared ([`Unusable::TypedTooLong`]), as finding where a long
    /// text's forms part costs the square of its length. A reader's
    /// [`Password::encodings`] takes a text of any length.
    pub const LONGEST: usize = 1024;

    /// The bytes it may have been set as, in the order a reader tries them,
    /// leniently: its SASLprep form (RFC 4013) as of Unicode 3.2, in UTF-8,
    /// which revisions 5 and 6 take (ISO 32000-2, 7.6.4.3.3), where SASLprep
    /// takes the text and makes it another ([`Encoding::Saslprep`]); its
    /// SASLprep form by a later Unicode, where that's another again, as
    /// writers that prepare it so set it
    /// ([`Encoding::SaslprepCurrentUnicode`]); as typed, in UTF-8, as
    /// writers that skip SASLprep set it; then, for revisions 2 to 4, whose
    /// passwords are bytes the spec has made from PDFDocEncoding
    /// (7.6.4.3.2) but writers from whatever their system had, the text
    /// composed (NFC, so that accents typed apart from their letters have a
    /// byte) in PDFDocEncoding, WinAnsiEncoding and MacRomanEncoding (Annex
    /// D), each where every character has a byte, as qpdf tries them. A
    /// password SASLprep refuses (with an emoji, unassigned in the Unicode
    /// SASLprep is of, or a control character) has the rest alone.
    ///
    /// None twice among those a revision takes first ([`Encoding::fits`]):
    /// a single-byte form is left out where UTF-8's or one before it has its
    /// bytes, but not for SASLprep's, which no revision takes beside it.
    /// [`Handler::open`](crate::Handler::open) tries no bytes twice.
    #[must_use]
    pub fn encodings(&self) -> Vec<Encoded> {
        let text = self.0;
        let typed = text.as_bytes();
        let mut encodings = Vec::with_capacity(6);
        let unicode_32 = saslprep_32(text);
        if let Some(form) = unicode_32.as_ref().filter(|form| form[..] != *typed) {
            encodings.push(Encoded {
                encoding: Encoding::Saslprep,
                bytes: form.clone(),
            });
        }
        let current = saslprep_current(text);
        if let Some(form) = current.filter(|form| {
            form[..] != *typed && unicode_32.as_ref().is_none_or(|other| form != other)
        }) {
            encodings.push(Encoded {
                encoding: Encoding::SaslprepCurrentUnicode,
                bytes: form,
            });
        }
        let typed_at = encodings.len();
        encodings.push(Encoded::new(Encoding::Utf8, typed));
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
            if !encodings[typed_at..]
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
    /// 7.6.4.3.3), tagged [`Encoding::Saslprep`]; or why it can't be.
    ///
    /// # Errors
    ///
    /// [`Unusable`], saying why. Not a text over [`Password::LONGEST`]
    /// bytes as typed ([`Unusable::TypedTooLong`]), refused before any of
    /// it's prepared; nor a password SASLprep refuses, the place of the
    /// character and the table that refuses it said where one is why
    /// ([`Unusable::Prohibited`], [`Unusable::Bidi`]); nor the empty one
    /// ([`Unusable::Empty`]), one that prepares to nothing
    /// ([`Unusable::Vanished`]), or one that prepares to over 127 bytes
    /// ([`Unusable::TooLong`]), which a reader would cut; nor one whose
    /// form by Unicode 3.2 and by a later Unicode differ, at the place they
    /// part ([`Unusable::Ambiguous`]), which readers would prepare apart.
    pub fn prepared(&self) -> Result<Encoded, Unusable> {
        let text = self.0;
        if text.len() > Password::LONGEST {
            return Err(Unusable::TypedTooLong {
                len: text.len(),
                most: Password::LONGEST,
            });
        }
        if text.is_empty() {
            return Err(Unusable::Empty);
        }
        // A character 3.2 hadn't assigned is refused (RFC 4013, 2.5), as
        // stringprep, checking its own form, doesn't where a later Unicode
        // decomposes it into ones 3.2 had.
        let bytes = match saslprep_current(text) {
            Some(bytes) if !text.chars().any(unassigned_code_point) => bytes,
            _ => return Err(refused(text)),
        };
        match bytes.len() {
            0 => return Err(Unusable::Vanished),
            len @ 128.. => return Err(Unusable::TooLong { len }),
            _ => {}
        }
        if saslprep_32(text).as_ref() != Some(&bytes) {
            return Err(Unusable::Ambiguous { at: parting(text) });
        }
        Ok(Encoded {
            encoding: Encoding::Saslprep,
            bytes,
        })
    }
}

/// Whether `bytes` are a new file's password as [`Password::prepared`]
/// gives it: UTF-8 that prepares to itself.
pub(crate) fn is_prepared(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| {
        Password(text)
            .prepared()
            .is_ok_and(|p| p.as_bytes() == bytes)
    })
}

/// Why a password can't be a new file's ([`Password::prepared`],
/// [`Handler::create`](crate::Handler::create)), or a new file's passwords
/// can't protect it as asked ([`Unusable::NoPassword`],
/// [`Unusable::Shared`]), or a new file's key and salts weren't drawn
/// ([`Unusable::Random`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Unusable {
    /// A character SASLprep doesn't allow (RFC 4013, 2.3 and 2.5): a
    /// control or private-use character, say, or one unassigned in the
    /// Unicode it's of, 3.2, as an emoji is. The first of the password's
    /// that it refuses whatever is around it: where it is, and which of
    /// the RFC's tables refuses it.
    ///
    /// Neither says the character, so a caller that logs the error logs
    /// nothing of the password but that it's at least `at` + 1 characters
    /// long. `Display` counts from 1: "the password's character 3 is a
    /// control character, which SASLprep doesn't allow".
    #[error(
        "the password's character {} is {why}, which SASLprep doesn't allow",
        .at + 1
    )]
    Prohibited {
        /// Its place among the password's characters as typed, from 0.
        at: usize,
        /// The table it's in.
        why: Prohibition,
    },
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
    /// Bytes in this encoding, not as [`Password::prepared`] gives them,
    /// which a new file's password must be: in another encoding than
    /// SASLprep's, or SASLprep's form by Unicode 3.2 of a password whose
    /// form by a later Unicode is another, which
    /// [`Password::encodings`] gives a reader to try.
    #[error("the password is in {0:?}, not prepared for a new file")]
    Unprepared(Encoding),
    /// A password SASLprep prepares one way by Unicode 3.2, which SASLprep
    /// is of, and another by a later Unicode, so that readers would
    /// disagree on it: one with a character of the five Unicode's
    /// Corrigendum #4 remapped (U+2F874, say), or a sequence Corrigendum #5
    /// composes otherwise (a vowel jamo after a consonant and an accent).
    /// The place of the character where the two forms part, never the
    /// character, as [`Unusable::Prohibited`]'s.
    #[error(
        "the password's character {} is where SASLprep by Unicode 3.2 and by a later Unicode differ",
        .at + 1
    )]
    Ambiguous {
        /// Its place among the password's characters as typed, from 0.
        at: usize,
    },
    /// Over [`Password::LONGEST`] bytes as typed (UTF-8): refused before
    /// any of it's prepared, as no password near so long is typed, and
    /// preparing one is slow.
    #[error("the password is {len} bytes as typed, over the {most} a password is prepared from")]
    TypedTooLong {
        /// Its length as typed.
        len: usize,
        /// The most a password is prepared from as typed:
        /// [`Password::LONGEST`].
        most: usize,
    },
    /// Neither the user's password nor the owner's, for a new file, as in
    /// [`Protection::DEFAULT`](crate::Protection::DEFAULT) before one is
    /// set: a protection left unset, whose file anyone would open as its
    /// owner, the empty password being both.
    #[error("no password is given for the new file, neither the user's nor the owner's")]
    NoPassword,
    /// Permissions withheld for a new file whose owner's password is its
    /// user's, none given apart from it or the same bytes: whoever opens
    /// the file opens it as its owner, whom the permissions don't bind
    /// (ISO 32000-2, 7.6.4.1). An owner's password of its own binds them.
    /// Bit 10, which revision 6 sets whatever's asked, isn't withheld.
    #[error(
        "the owner's password is the user's, so the permissions withheld would bind no one who opens the file"
    )]
    Shared,
    /// The operating system's random number generator failed, so a new
    /// file's key and salts weren't drawn: nothing is made.
    #[error("{0}")]
    Random(Random),
}

/// Which of RFC 4013's tables of what SASLprep doesn't allow (its 2.3,
/// RFC 3454's C.2 to C.9, and its 2.5, RFC 3454's A.1) refuses a
/// password's character ([`Unusable::Prohibited`]). C.1.2's spaces, which
/// SASLprep maps to a space first, and C.5's surrogates, which no `str`
/// holds, refuse none.
///
/// Exhaustive on purpose: the RFC's tables are a fixed list, so a variant
/// more would be a breaking release. `Display` writes it in words, "a
/// control character".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Prohibition {
    /// A control character (C.2.1 and C.2.2): ASCII's, U+0000 to U+001F
    /// and U+007F, or another, as U+0080 to U+009F, the line and paragraph
    /// separators and the interlinear annotation characters are.
    Control,
    /// A private-use character (C.3).
    PrivateUse,
    /// A noncharacter (C.4): U+FDD0 to U+FDEF, and the last two code points
    /// of each plane.
    NonCharacter,
    /// A character inappropriate for plain text (C.6): the replacement
    /// character, U+FFFD; the table's others are control characters too,
    /// and said to be ([`Prohibition::Control`]).
    NotText,
    /// A character inappropriate for canonical representation (C.7): an
    /// ideographic description character, U+2FF0 to U+2FFB.
    NotCanonical,
    /// A character that changes how text is displayed, or is deprecated
    /// (C.8): the left-to-right and right-to-left marks, embeddings and
    /// overrides.
    Display,
    /// A tagging character (C.9): U+E0001, and U+E0020 to U+E007F.
    Tagging,
    /// A code point Unicode 3.2, which SASLprep is of, hadn't assigned (A.1),
    /// as an emoji is, though a later Unicode may have.
    Unassigned,
}

impl fmt::Display for Prohibition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Prohibition::Control => "a control character",
            Prohibition::PrivateUse => "a private-use character",
            Prohibition::NonCharacter => "a noncharacter",
            Prohibition::NotText => "a character not for plain text",
            Prohibition::NotCanonical => "an ideographic description character",
            Prohibition::Display => "a character that changes how text is displayed",
            Prohibition::Tagging => "a tagging character",
            Prohibition::Unassigned => "a character Unicode 3.2 hadn't assigned",
        })
    }
}

impl Prohibition {
    /// The first of the tables, in the RFC's order, that `c` is in, by
    /// stringprep's own table of each; A.1's last, as SASLprep checks it
    /// last (RFC 4013, 2.5). None where it's in none.
    pub(crate) fn of(c: char) -> Option<Prohibition> {
        Some(match c {
            _ if ascii_control_character(c) || non_ascii_control_character(c) => {
                Prohibition::Control
            }
            _ if private_use(c) => Prohibition::PrivateUse,
            _ if non_character_code_point(c) => Prohibition::NonCharacter,
            _ if inappropriate_for_plain_text(c) => Prohibition::NotText,
            _ if inappropriate_for_canonical_representation(c) => Prohibition::NotCanonical,
            _ if change_display_properties_or_deprecated(c) => Prohibition::Display,
            _ if tagging_character(c) => Prohibition::Tagging,
            _ if unassigned_code_point(c) => Prohibition::Unassigned,
            _ => return None,
        })
    }
}

/// Why SASLprep refuses `text`: the first of its characters it refuses
/// whatever is around it, alone and between two right-to-left letters, as
/// it does a character it doesn't allow, or one 3.2 hadn't assigned, by
/// its place and the table it's in ([`Prohibition::of`]); else the bidi
/// rule, which a character can break alone (a Hebrew letter composed with
/// its point, which ends it) but none breaks whatever is around it.
/// stringprep's error says neither, and its text is of the text SASLprep
/// made, not the typed.
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
    (text.chars().enumerate())
        .find(|&(_, c)| unassigned_code_point(c) || prohibited(c))
        .map_or(Unusable::Bidi, |(at, c)| Unusable::Prohibited {
            at,
            why: prohibition(c),
        })
}

/// The table that refuses `c`, a character SASLprep refuses whatever is
/// around it: the first it's in ([`Prohibition::of`]). Every such
/// character is in one itself, as a test holds of every code point; were
/// one in none, it would be one 3.2 hadn't assigned in effect, as SASLprep
/// would refuse it for what a later Unicode made of it.
fn prohibition(c: char) -> Prohibition {
    Prohibition::of(c).unwrap_or(Prohibition::Unassigned)
}

/// `text`'s SASLprep form by the Unicode `unicode-normalization` is of,
/// `stringprep`'s, in UTF-8; none where SASLprep refuses it.
fn saslprep_current(text: &str) -> Option<Zeroizing<Vec<u8>>> {
    // SASLprep borrows the text it leaves as it is (ASCII), and moves the
    // text it makes into the buffer, not copied.
    match saslprep(text).ok()? {
        Cow::Borrowed(form) => Some(Zeroizing::new(form.as_bytes().to_vec())),
        Cow::Owned(form) => Some(Zeroizing::new(form.into_bytes())),
    }
}

/// The five characters Unicode's Corrigendum #4 remapped in Unicode 4.0,
/// each with the decomposition 3.2 gave it (NormalizationCorrections.txt):
/// what UAX #15 (section 11.3) has a later Unicode map them to first, to
/// normalize as 3.2. U+F951's, Corrigendum #3's, was corrected in 3.2
/// itself, so 3.2's and later Unicode's agree on it.
pub(crate) const CORRIGENDUM_4: [(char, char); 5] = [
    ('\u{2F868}', '\u{2136A}'),
    ('\u{2F874}', '\u{5F33}'),
    ('\u{2F91F}', '\u{43AB}'),
    ('\u{2F95F}', '\u{7AAE}'),
    ('\u{2F9BF}', '\u{4D57}'),
];

/// `text`'s SASLprep form as of Unicode 3.2, in UTF-8; none where SASLprep
/// refuses it. stringprep's form, of the text with Corrigendum #4's five
/// characters mapped to their 3.2 decompositions, composed again by 3.2's
/// rule ([`composed_32`]); none for a text with a character 3.2 hadn't
/// assigned, which stays as it is in 3.2's form and is refused there (RFC
/// 4013, 2.5), though a later Unicode may decompose it into ones 3.2 had.
/// Its checks hold for 3.2's form: the composites 3.2's rule makes where a
/// later one doesn't are left-to-right letters and vowel signs, as a test
/// holds them.
pub(crate) fn saslprep_32(text: &str) -> Option<Zeroizing<Vec<u8>>> {
    if text.chars().any(unassigned_code_point) {
        return None;
    }
    let mapped: Zeroizing<String>;
    let text = match text
        .chars()
        .any(|c| CORRIGENDUM_4.iter().any(|&(x, _)| x == c))
    {
        true => {
            // Each of the five's 3.2 decomposition is no longer in UTF-8.
            let mut each = Zeroizing::new(String::with_capacity(text.len()));
            for c in text.chars() {
                let old = CORRIGENDUM_4.iter().find(|&&(x, _)| x == c);
                each.push(old.map_or(c, |&(_, old)| old));
            }
            mapped = each;
            &mapped[..]
        }
        false => text,
    };
    match saslprep(text).ok()? {
        Cow::Borrowed(form) => Some(Zeroizing::new(form.as_bytes().to_vec())),
        Cow::Owned(form) => Some(composed_32(&Zeroizing::new(form))),
    }
}

/// `form`, a later Unicode's NFKC, composed again as Unicode 3.2 composes,
/// in UTF-8: decomposed (NFD, which is NFKD for text in NFKC), then each
/// character composed with the last starter unless blocked from it, by
/// UAX #15 3.2.0's definition D2, "either B is a starter or it has the
/// same combining class as C". Corrigendum #5 (Unicode 4.1) made it "the
/// same or higher", so that a starter no longer composes with the one
/// before it across a mark: by 3.2, U+1100 U+0300 U+1161 is U+AC00 U+0300
/// (Public Review Issue #29's example); by a later Unicode, it's as it is.
/// The sequences it changes are UAX #15's Table 10 (section 11.5), none of
/// them in text of any language. In one pass: whether a character is
/// blocked is read from the combining classes of those between it and the
/// last starter, kept as a set, so a long run of marks costs its length.
fn composed_32(form: &str) -> Zeroizing<Vec<u8>> {
    let mut decomposed = Zeroizing::new(Vec::with_capacity(form.nfd().count()));
    decomposed.extend(form.nfd());
    let mut composed: Zeroizing<Vec<char>> = Zeroizing::new(Vec::with_capacity(decomposed.len()));
    // Where the last starter is in what's composed: the characters after it
    // are those between it and the next, none of them a starter, and
    // `between` the set of their combining classes, a bit a class.
    let mut starter: Option<usize> = None;
    let mut between = [0u64; 4];
    let bit = |class: u8| (usize::from(class / 64), 1u64 << (class % 64));
    for &c in decomposed.iter() {
        let class = canonical_combining_class(c);
        if let Some(s) = starter {
            let (word, mask) = bit(class);
            let blocked = between[word] & mask != 0;
            if let (false, Some(p)) = (blocked, compose(composed[s], c)) {
                composed[s] = p;
                continue;
            }
        }
        match class {
            0 => {
                starter = Some(composed.len());
                between = [0; 4];
            }
            _ => {
                let (word, mask) = bit(class);
                between[word] |= mask;
            }
        }
        composed.push(c);
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(
        composed.iter().map(|c| c.len_utf8()).sum(),
    ));
    let mut buffer = Zeroizing::new([0; 4]);
    for c in composed.iter() {
        bytes.extend_from_slice(c.encode_utf8(&mut buffer[..]).as_bytes());
    }
    bytes
}

/// The place among `text`'s characters, from 0, of the first where its
/// SASLprep forms by Unicode 3.2 and by a later Unicode part: the last of
/// the shortest beginning of it whose two forms differ. `text`'s own forms
/// differ, or its last. Each beginning is prepared apart, so a text costs
/// the square of its length: [`Password::prepared`] asks it of one of
/// [`Password::LONGEST`] bytes at most.
fn parting(text: &str) -> usize {
    let mut ends = text.char_indices().map(|(i, c)| i + c.len_utf8());
    let differ = |end: usize| saslprep_32(&text[..end]) != saslprep_current(&text[..end]);
    (ends.position(differ)).unwrap_or_else(|| text.chars().count().saturating_sub(1))
}

/// `c`'s byte in the single-byte `encoding`, if it has one.
fn byte(encoding: Encoding, c: char) -> Option<u8> {
    match encoding {
        Encoding::PdfDoc => pdf_doc_byte(c),
        Encoding::WinAnsi => win_ansi_byte(c),
        Encoding::MacRoman => mac_roman_byte(c),
        Encoding::Saslprep
        | Encoding::SaslprepCurrentUnicode
        | Encoding::Utf8
        | Encoding::Verbatim => None,
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

    /// SASLprep's forms fit revisions 5 and 6 alone; the single-byte
    /// encodings, 2 to 4 alone; UTF-8 and bytes as given, any: each
    /// encoding's answers for revisions 0 to 8, written out, and for
    /// revisions far from them.
    #[test]
    fn each_encoding_fits_the_revisions_it_can_have_been_set_for() {
        let (no, yes) = (false, true);
        // Each for revisions 0 to 8.
        let text = [no, no, no, no, no, yes, yes, no, no];
        let bytes = [no, no, yes, yes, yes, no, no, no, no];
        let any = [yes; 9];
        let answers = [
            (Encoding::Saslprep, text),
            (Encoding::SaslprepCurrentUnicode, text),
            (Encoding::Utf8, any),
            (Encoding::PdfDoc, bytes),
            (Encoding::WinAnsi, bytes),
            (Encoding::MacRoman, bytes),
            (Encoding::Verbatim, any),
        ];
        for (encoding, fits) in answers {
            for (r, fits) in (0..).zip(fits) {
                assert_eq!(encoding.fits(r), fits, "{encoding:?} {r}");
            }
            let far = encoding == Encoding::Utf8 || encoding == Encoding::Verbatim;
            for r in [i64::MIN, -1, 9, 127, i64::MAX] {
                assert_eq!(encoding.fits(r), far, "{encoding:?} {r}");
            }
        }
    }

    /// Whether an encoding fits a revision can be had in a constant.
    #[test]
    fn whether_an_encoding_fits_can_be_had_in_a_constant() {
        const FITS: [bool; 3] = [
            Encoding::Saslprep.fits(6),
            Encoding::PdfDoc.fits(6),
            Encoding::Verbatim.fits(7),
        ];
        assert_eq!(FITS, [true, false, true]);
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
        let control = Unusable::Prohibited {
            at: 0,
            why: Prohibition::Control,
        };
        assert_eq!(prepared("\u{7}"), Err(control));
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

    /// Each reason a password can't be a new file's, or a new file's
    /// passwords can't protect it as asked, word for word.
    #[test]
    fn each_reason_a_password_cant_be_a_new_files_says_why() {
        let why = |text: &str| Password(text).prepared().unwrap_err();
        let reasons = [
            // A character unassigned in Unicode 3.2, and a control
            // character, each the first SASLprep refuses.
            (
                why("pass\u{1F954}\u{1}"),
                Unusable::Prohibited {
                    at: 4,
                    why: Prohibition::Unassigned,
                },
                "the password's character 5 is a character Unicode 3.2 hadn't assigned, \
                 which SASLprep doesn't allow",
            ),
            (
                why("a\u{AA}\u{1}b"),
                Unusable::Prohibited {
                    at: 2,
                    why: Prohibition::Control,
                },
                "the password's character 3 is a control character, which SASLprep doesn't allow",
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
                "the password is in Utf8, not prepared for a new file",
            ),
            // A new file with neither password, and its permissions
            // under one (`create`).
            (
                Unusable::NoPassword,
                Unusable::NoPassword,
                "no password is given for the new file, neither the user's nor the owner's",
            ),
            (
                Unusable::Shared,
                Unusable::Shared,
                "the owner's password is the user's, so the permissions withheld would bind \
                 no one who opens the file",
            ),
            // A character 3.2 hadn't assigned, though a later Unicode
            // decomposes it (U+2090, LATIN SUBSCRIPT SMALL LETTER A, of
            // 4.1), before a control character.
            (
                why("x\u{2090}\u{1}"),
                Unusable::Prohibited {
                    at: 1,
                    why: Prohibition::Unassigned,
                },
                "the password's character 2 is a character Unicode 3.2 hadn't assigned, \
                 which SASLprep doesn't allow",
            ),
            // Where SASLprep by 3.2 and by a later Unicode part:
            // Corrigendum #4's character, and Corrigendum #5's vowel jamo.
            (
                why("Password\u{2F874}!"),
                Unusable::Ambiguous { at: 8 },
                "the password's character 9 is where SASLprep by Unicode 3.2 and by a later \
                 Unicode differ",
            ),
            (
                why("\u{1100}\u{300}\u{1161}\u{323}"),
                Unusable::Ambiguous { at: 2 },
                "the password's character 3 is where SASLprep by Unicode 3.2 and by a later \
                 Unicode differ",
            ),
            // Over 1,024 bytes as typed, refused before it's prepared.
            (
                why(&"a".repeat(1025)),
                Unusable::TypedTooLong {
                    len: 1025,
                    most: 1024,
                },
                "the password is 1025 bytes as typed, over the 1024 a password is prepared from",
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

    /// Corrigendum #4's five characters, which Unicode 4.0 remapped
    /// (NormalizationCorrections.txt): read, SASLprep's form by 3.2 first,
    /// with the decomposition 3.2 gave each, then by a later Unicode, with
    /// the corrected one, then as typed; for a new file, refused, at the
    /// character. U+F951, Corrigendum #3's, was corrected in 3.2 itself: one
    /// form, and a new file's.
    #[test]
    fn corrigendum_4s_characters_are_tried_as_3_2_has_them_first() {
        use Encoding::{Saslprep, SaslprepCurrentUnicode, Utf8};
        let utf8 = |text: &str| text.as_bytes().to_vec();
        let five = [
            ('\u{2F868}', '\u{2136A}', '\u{36FC}'),
            ('\u{2F874}', '\u{5F33}', '\u{5F53}'),
            ('\u{2F91F}', '\u{43AB}', '\u{243AB}'),
            ('\u{2F95F}', '\u{7AAE}', '\u{7AEE}'),
            ('\u{2F9BF}', '\u{4D57}', '\u{45D7}'),
        ];
        for (c, old, new) in five {
            assert!(CORRIGENDUM_4.contains(&(c, old)), "{c:?}");
            let typed = format!("Password{c}!");
            let want = [
                (Saslprep, utf8(&format!("Password{old}!"))),
                (SaslprepCurrentUnicode, utf8(&format!("Password{new}!"))),
                (Utf8, utf8(&typed)),
            ];
            assert_eq!(forms(&typed), want, "{c:?}");
            let why = Password(&typed).prepared().unwrap_err();
            assert_eq!(why, Unusable::Ambiguous { at: 8 }, "{c:?}");
            // 3.2's form is a password of its own, the same by both, and
            // the corrected one too.
            for alike in [old, new] {
                let typed = format!("Password{alike}!");
                assert_eq!(forms(&typed), [(Utf8, utf8(&typed))], "{alike:?}");
                let prepared = Password(&typed).prepared().unwrap();
                assert_eq!(prepared.as_bytes(), typed.as_bytes(), "{alike:?}");
            }
        }
        assert_eq!(
            forms("\u{F951}"),
            [(Saslprep, utf8("\u{964B}")), (Utf8, utf8("\u{F951}"))]
        );
        let prepared = Password("\u{F951}").prepared().unwrap();
        assert_eq!(prepared.as_bytes(), utf8("\u{964B}"));
    }

    /// Corrigendum #5's sequences, a starter after a mark that composes with
    /// the starter before it, composed as 3.2's definition D2 composes them:
    /// read, 3.2's form first, then as typed, where a later Unicode's form
    /// is the text; for a new file, refused, at the starter. A password
    /// whose forms by both agree is a new file's: `가` U+0300 U+0323, which
    /// 3.2 and a later Unicode both give as `가` U+0323 U+0300, normalized
    /// once or twice.
    #[test]
    fn corrigendum_5s_sequences_compose_as_3_2_composes_them() {
        use Encoding::{Saslprep, Utf8};
        let utf8 = |text: &str| text.as_bytes().to_vec();
        // Public Review Issue #29's: by 3.2, U+1100 U+0300 U+1161 is U+AC00
        // U+0300; by a later Unicode, as it is. Each refused at the place of
        // the starter, the third character.
        let cases = [
            ("\u{1100}\u{300}\u{1161}", "\u{AC00}\u{300}", 2),
            (
                "\u{1100}\u{300}\u{1161}\u{323}",
                "\u{AC00}\u{300}\u{323}",
                2,
            ),
            // A trailing consonant after a syllable and a mark.
            ("\u{AC00}\u{300}\u{11A8}", "\u{AC01}\u{300}", 2),
            // Oriya's vowel sign O, from its E and AA.
            ("\u{B47}\u{300}\u{B3E}", "\u{B4B}\u{300}", 2),
        ];
        for (typed, old, at) in cases {
            assert_eq!(
                forms(typed),
                [(Saslprep, utf8(old)), (Utf8, utf8(typed))],
                "{typed:?}"
            );
            let why = Password(typed).prepared().unwrap_err();
            assert_eq!(why, Unusable::Ambiguous { at }, "{typed:?}");
        }
        // 3.2's form composed once is no new file's either: by both it's
        // composed again, its marks reordered.
        let once = "\u{AC00}\u{300}\u{323}";
        let twice = "\u{AC00}\u{323}\u{300}";
        assert_eq!(forms(once), [(Saslprep, utf8(twice)), (Utf8, utf8(once))]);
        assert_eq!(Password(once).prepared().unwrap().as_bytes(), utf8(twice));
        assert_eq!(forms(twice), [(Utf8, utf8(twice))]);
        assert_eq!(Password(twice).prepared().unwrap().as_bytes(), utf8(twice));
        // A mark between starters that don't compose changes nothing.
        let apart = "\u{1100}\u{300}\u{1100}";
        assert_eq!(forms(apart), [(Utf8, utf8(apart))]);
    }

    /// A character 3.2 hadn't assigned is refused by SASLprep as of 3.2,
    /// which leaves it as it is, though a later Unicode decomposes it into
    /// ones 3.2 had: read, stringprep's form alone; for a new file,
    /// refused, named.
    #[test]
    fn a_character_3_2_hadnt_assigned_has_a_later_unicodes_form_alone() {
        use Encoding::{SaslprepCurrentUnicode, Utf8};
        let utf8 = |text: &str| text.as_bytes().to_vec();
        // LATIN SUBSCRIPT SMALL LETTER A (4.1), and a CJK compatibility
        // ideograph of 4.1, U+FA70, whose decomposition U+4E26 3.2 had.
        for (c, new) in [('\u{2090}', "a"), ('\u{FA70}', "\u{4E26}")] {
            let typed = c.to_string();
            let want = [(SaslprepCurrentUnicode, utf8(new)), (Utf8, utf8(&typed))];
            assert_eq!(forms(&typed), want, "{c:?}");
            let why = Password(&typed).prepared().unwrap_err();
            let unassigned = Unusable::Prohibited {
                at: 0,
                why: Prohibition::Unassigned,
            };
            assert_eq!(why, unassigned, "{c:?}");
        }
        // One that a later Unicode leaves as it is, an emoji: both refuse.
        assert_eq!(forms("\u{1F954}"), [(Utf8, utf8("\u{1F954}"))]);
    }

    /// The composites Unicode 3.2's rule makes where a later Unicode's
    /// doesn't, those whose decomposition has a starter after its first
    /// character, have their last starters in UAX #15's Table 10 (section
    /// 11.5), every one of it: each a letter or vowel sign that SASLprep
    /// allows, left-to-right, of 3.2, composed from a left-to-right
    /// character; so stringprep's checks of a later Unicode's form hold for
    /// 3.2's, whose characters differ only by such composites.
    #[test]
    fn the_composites_3_2s_rule_makes_are_table_10s_and_allowed() {
        use std::collections::BTreeSet;
        use stringprep::tables::bidi_l;
        use unicode_normalization::char::decompose_canonical;
        let mut lasts = BTreeSet::new();
        for p in (0..=0x10_FFFF).filter_map(char::from_u32) {
            let mut decomposed = Vec::new();
            decompose_canonical(p, |c| decomposed.push(c));
            let starter = |&c: &char| canonical_combining_class(c) == 0;
            if decomposed.len() < 2 || !decomposed[1..].iter().any(starter) {
                continue;
            }
            // One composition makes, of characters 3.2 had.
            let made = decomposed.iter().copied().nfc().eq([p]);
            if !made || decomposed.iter().any(|&c| unassigned_code_point(c)) {
                continue;
            }
            let alone = p.to_string();
            assert!(!unassigned_code_point(p), "U+{:04X}", u32::from(p));
            let prepared = saslprep(&alone).ok();
            assert_eq!(
                prepared.as_deref(),
                Some(&alone[..]),
                "U+{:04X}",
                u32::from(p)
            );
            assert!(bidi_l(p), "U+{:04X}", u32::from(p));
            assert!(
                decomposed.iter().any(|&c| bidi_l(c)),
                "U+{:04X}",
                u32::from(p)
            );
            lasts.extend(decomposed[1..].iter().copied().filter(starter));
        }
        let table_10: BTreeSet<char> = [
            0x9BE, 0x9D7, 0xB3E, 0xB56, 0xB57, 0xBBE, 0xBD7, 0xCC2, 0xCD5, 0xCD6, 0xD3E, 0xD57,
            0xDCF, 0xDDF, 0x102E,
        ]
        .into_iter()
        .chain(0x1161..=0x1175)
        .chain(0x11A8..=0x11C2)
        .filter_map(char::from_u32)
        .collect();
        assert_eq!(lasts, table_10);
    }

    /// A text of 1,024 bytes as typed is prepared, and one of 1,025 is
    /// refused before any of it's prepared: its control character, which
    /// preparing would refuse, isn't what's said.
    #[test]
    fn a_text_over_1024_bytes_is_refused_before_its_prepared() {
        assert_eq!(Password::LONGEST, 1024);
        // 126 letters and 449 soft hyphens, which SASLprep maps to nothing.
        let longest = format!("{}{}", "a".repeat(126), "\u{AD}".repeat(449));
        assert_eq!(longest.len(), 1024);
        let prepared = Password(&longest).prepared().unwrap();
        assert_eq!(prepared.as_bytes(), "a".repeat(126).as_bytes());
        // A control character at its end, 1,024 bytes, then one byte more.
        let control = format!("{}{}\u{1}", "a".repeat(127), "\u{AD}".repeat(448));
        assert_eq!(control.len(), 1024);
        let at = 127 + 448;
        assert_eq!(
            Password(&control).prepared().unwrap_err(),
            Unusable::Prohibited {
                at,
                why: Prohibition::Control
            }
        );
        let over = format!("{control}a");
        assert_eq!(over.len(), 1025);
        assert_eq!(
            Password(&over).prepared().unwrap_err(),
            Unusable::TypedTooLong {
                len: 1025,
                most: Password::LONGEST
            }
        );
        // A reader tries a text of any length, as typed among its forms.
        let forms = Password(&over).encodings();
        let typed = |f: &Encoded| f.encoding() == Encoding::Utf8 && f.as_bytes() == over.as_bytes();
        assert!(forms.iter().any(typed));
    }

    /// Each table refuses a character of its own, said by its place among
    /// the characters as typed, from 0 (a letter of two bytes before it),
    /// and by its words, counted from 1.
    #[test]
    fn each_prohibition_is_said_by_its_place_and_in_words() {
        use Prohibition::{
            Control, Display, NonCharacter, NotCanonical, NotText, PrivateUse, Tagging, Unassigned,
        };
        let cases = [
            ('\u{7}', Control, "a control character"),
            ('\u{85}', Control, "a control character"),
            ('\u{FFF9}', Control, "a control character"),
            ('\u{E000}', PrivateUse, "a private-use character"),
            ('\u{10FFFD}', PrivateUse, "a private-use character"),
            ('\u{FDD0}', NonCharacter, "a noncharacter"),
            ('\u{FFFF}', NonCharacter, "a noncharacter"),
            ('\u{FFFD}', NotText, "a character not for plain text"),
            (
                '\u{2FF0}',
                NotCanonical,
                "an ideographic description character",
            ),
            (
                '\u{200E}',
                Display,
                "a character that changes how text is displayed",
            ),
            (
                '\u{202E}',
                Display,
                "a character that changes how text is displayed",
            ),
            ('\u{E0001}', Tagging, "a tagging character"),
            ('\u{E0041}', Tagging, "a tagging character"),
            (
                '\u{1F954}',
                Unassigned,
                "a character Unicode 3.2 hadn't assigned",
            ),
            (
                '\u{221}',
                Unassigned,
                "a character Unicode 3.2 hadn't assigned",
            ),
        ];
        for (c, why, words) in cases {
            assert_eq!(why.to_string(), words);
            let typed = format!("p\u{E4}{c}ss");
            let refused = Password(&typed).prepared().unwrap_err();
            assert_eq!(refused, Unusable::Prohibited { at: 2, why }, "{c:?}");
            let said =
                format!("the password's character 3 is {words}, which SASLprep doesn't allow");
            assert_eq!(refused.to_string(), said, "{c:?}");
        }
    }

    /// Every character SASLprep refuses whatever is around it, alone and
    /// between two right-to-left letters, or that 3.2 hadn't assigned, is
    /// in one of the tables itself, so a refusal always says which; and a
    /// character in one of them that SASLprep allows is one it maps first:
    /// a deprecated tone mark, to its accent.
    #[test]
    fn every_character_saslprep_refuses_is_in_a_table() {
        let mut each = std::collections::BTreeSet::new();
        let mut allowed = Vec::new();
        for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
            let mut around = [0xD7, 0x90, 0, 0, 0, 0, 0, 0];
            let len = c.encode_utf8(&mut around[2..6]).len();
            around[2 + len..4 + len].copy_from_slice(&[0xD7, 0x90]);
            let alone = std::str::from_utf8(&around[2..2 + len]).unwrap();
            let between = std::str::from_utf8(&around[..4 + len]).unwrap();
            let refused = unassigned_code_point(c)
                || (saslprep(alone).is_err() && saslprep(between).is_err());
            match (refused, Prohibition::of(c)) {
                (true, Some(why)) => _ = each.insert(format!("{why:?}")),
                (true, None) => panic!("U+{:04X} refused in no table", u32::from(c)),
                (false, Some(_)) => allowed.push(c),
                (false, None) => {}
            }
        }
        assert_eq!(each.len(), 8, "{each:?}");
        // Mapped to nothing, to a space, or to another character first.
        let mapped = [
            '\u{340}', '\u{341}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}',
        ];
        assert_eq!(allowed, mapped);
    }

    /// Composing by 3.2's rule in one pass, the combining classes between a
    /// character and the last starter kept as a set, composes as asking each
    /// character between does: over long runs of one mark, of marks of two
    /// classes in turn, and of random marks and starters that compose.
    #[test]
    fn composing_in_one_pass_is_asking_each_character_between() {
        // UAX #15 3.2.0's D2, each character between asked.
        fn by_each(form: &str) -> Vec<u8> {
            let decomposed: Vec<char> = form.nfd().collect();
            let mut composed: Vec<char> = Vec::new();
            let mut starter: Option<usize> = None;
            for c in decomposed {
                let class = canonical_combining_class(c);
                if let Some(s) = starter {
                    let blocked =
                        (composed[s + 1..].iter()).any(|&b| canonical_combining_class(b) == class);
                    if let (false, Some(p)) = (blocked, compose(composed[s], c)) {
                        composed[s] = p;
                        continue;
                    }
                }
                if class == 0 {
                    starter = Some(composed.len());
                }
                composed.push(c);
            }
            composed.into_iter().collect::<String>().into_bytes()
        }
        let mut texts = vec![
            format!("a{}", "\u{301}".repeat(300)),
            format!("a{}", "\u{301}\u{323}".repeat(150)),
            format!("\u{1100}{}\u{1161}", "\u{300}".repeat(200)),
            format!("{}\u{1161}", "\u{300}".repeat(10)),
            // An acute blocked from the a by a candrabindu, of its class,
            // which composes with nothing; not by a dot below, of another.
            "a\u{310}\u{301}".to_string(),
            "a\u{334}\u{301}".to_string(),
            format!("a{}\u{301}", "\u{310}\u{334}".repeat(100)),
        ];
        let pool = [
            'a', 'e', 'o', '\u{1100}', '\u{1161}', '\u{11A8}', '\u{B47}', '\u{B3E}', '\u{300}',
            '\u{301}', '\u{308}', '\u{310}', '\u{323}', '\u{327}', '\u{334}', '\u{345}', '\u{5B0}',
        ];
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        for _ in 0..2000 {
            let text: String = (0..1 + seed as usize % 24)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    pool[seed as usize % pool.len()]
                })
                .collect();
            texts.push(text);
        }
        for text in &texts {
            assert_eq!(composed_32(text)[..], by_each(text)[..], "{text:?}");
        }
        // Blocked, the acute stays; not, it composes.
        assert_eq!(
            composed_32("a\u{310}\u{301}")[..],
            *"a\u{310}\u{301}".as_bytes()
        );
        assert_eq!(
            composed_32("a\u{334}\u{301}")[..],
            *"\u{E1}\u{334}".as_bytes()
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
