//! The lexical rules a tokenizer follows: ISO 32000-2's, or PDFium's,
//! fixed in its type or chosen as it runs.

use core::fmt::Debug;

use crate::chars::{DELIMITER, PDFIUM_SPACE, SPACE, class};

/// Which lexical rules a [`Tokenizer`](crate::Tokenizer) follows: where a
/// token ends, and what kind it is. ISO 32000-2's unless the caller
/// chooses PDFium's, with [`Tokenizer::with_rules`](crate::Tokenizer::with_rules).
/// As a value, they're chosen as the tokenizer runs; [`Iso`] and
/// [`Pdfium`] fix them in its type instead ([`RuleSet`]).
///
/// They part in three places, and nowhere else:
///
/// - **White-space:** PDFium's has the bytes 0x80 and 0xFF besides ISO
///   32000-2's six (7.2.3), so either of them ends a name or a word, and a
///   run of white-space takes them in.
/// - **Numbers:** to PDFium, a run of regular bytes that are all digits,
///   signs and points is a number: a [`Real`](crate::Kind::Real) when a
///   point is among them, and otherwise an
///   [`Integer`](crate::Kind::Integer). `--5`, `5-3` and `+` are integers
///   and `1.2.3` and `.` reals, where ISO 32000-2 (7.3.3) has a number
///   written as one sign at most, then digits with one point at most, and
///   reads them as keywords. `1e5` is a keyword under both.
/// - **Hexadecimal strings:** PDFium's runs from `<` to the next `>` over
///   any byte, and is unclosed only when the input ends first. ISO 32000-2's
///   (7.3.4.3) holds only hex digits and white-space, so it ends, unclosed,
///   before any other byte.
///
/// Comments, names, literal strings, the delimiters and
/// [`Junk`](crate::Kind::Junk) are the same under both, but where a byte
/// is white-space under one and not the other. Every promise the
/// tokenizer makes holds under either.
///
/// What a token stands for is read by ISO 32000-2's rules in
/// [`Token::pieces`](crate::Token::pieces) and the rest, and by PDFium's in
/// [`decode::pdfium`](crate::decode::pdfium).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Rules {
    /// ISO 32000-2's lexical conventions (7.2, 7.3): the default.
    #[default]
    Iso,
    /// PDFium's, as both its content stream parser (`CPDF_StreamParser`)
    /// and its object parser (`CPDF_SyntaxParser`) read tokens.
    Pdfium,
}

impl Rules {
    /// Whether `b` is white-space under these rules: NUL, tab, line feed,
    /// form feed, carriage return or space, as
    /// [`is_white_space`](crate::is_white_space) says, and under PDFium's
    /// 0x80 and 0xFF too.
    pub const fn is_white_space(self, b: u8) -> bool {
        class(b) & self.space() != 0
    }

    /// Whether `b` is regular under these rules: neither white-space nor a
    /// delimiter. The delimiters are the same under both.
    pub const fn is_regular(self, b: u8) -> bool {
        class(b) & (self.space() | DELIMITER) == 0
    }

    /// The classes that are white-space under these rules.
    #[inline(always)]
    const fn space(self) -> u8 {
        match self {
            Rules::Iso => SPACE,
            Rules::Pdfium => SPACE | PDFIUM_SPACE,
        }
    }
}

/// The rules a [`Tokenizer`](crate::Tokenizer) is built with: [`Iso`] or
/// [`Pdfium`], fixed in its type, or [`Rules`], a value chosen as it runs.
///
/// Fixed in its type, a tokenizer has nothing to choose at each token, and
/// runs as one that knows a single set of rules would: `Tokenizer<'a>` is
/// `Tokenizer<'a, Iso>`, what [`new`](crate::Tokenizer::new) and
/// [`at`](crate::Tokenizer::at) give. Chosen as it runs, for a reader that
/// learns which only then, it asks at each token, a branch a token. The
/// tokens are the same either way.
///
/// Sealed: these three are the only ones.
pub trait RuleSet: Copy + Debug + Send + Sync + sealed::Sealed {
    /// The rules, as a value.
    fn rules(self) -> Rules;
}

/// ISO 32000-2's rules ([`Rules::Iso`]), fixed in a
/// [`Tokenizer`](crate::Tokenizer)'s type: the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Iso;

/// PDFium's rules ([`Rules::Pdfium`]), fixed in a
/// [`Tokenizer`](crate::Tokenizer)'s type:
/// `Tokenizer::at(bytes, offset).with_rules(Pdfium)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Pdfium;

impl RuleSet for Iso {
    #[inline]
    fn rules(self) -> Rules {
        Rules::Iso
    }
}

impl RuleSet for Pdfium {
    #[inline]
    fn rules(self) -> Rules {
        Rules::Pdfium
    }
}

impl RuleSet for Rules {
    #[inline]
    fn rules(self) -> Rules {
        self
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Iso {}
    impl Sealed for super::Pdfium {}
    impl Sealed for super::Rules {}
}

#[cfg(test)]
mod tests {
    use crate::{Iso, Pdfium, RuleSet, Rules, is_regular, is_white_space};

    #[test]
    fn iso_32000s_rules_are_the_default() {
        assert_eq!(Rules::default(), Rules::Iso);
    }

    /// Fixed in a type or chosen as a value, each set of rules is itself.
    #[test]
    fn each_rule_set_says_its_rules() {
        assert_eq!(Iso.rules(), Rules::Iso);
        assert_eq!(Pdfium.rules(), Rules::Pdfium);
        assert_eq!(Rules::Iso.rules(), Rules::Iso);
        assert_eq!(Rules::Pdfium.rules(), Rules::Pdfium);
    }

    /// PDFium's white-space is ISO 32000-2's and 0x80 and 0xFF: no other
    /// byte, VT and NBSP not among them.
    #[test]
    fn pdfiums_white_space_has_two_bytes_more() {
        for b in 0..=u8::MAX {
            assert_eq!(Rules::Iso.is_white_space(b), is_white_space(b), "{b:#x}");
            assert_eq!(Rules::Iso.is_regular(b), is_regular(b), "{b:#x}");
            let more = matches!(b, 0x80 | 0xFF);
            assert_eq!(Rules::Pdfium.is_white_space(b), is_white_space(b) || more);
            assert_eq!(Rules::Pdfium.is_regular(b), is_regular(b) && !more);
        }
        for b in [0x0B, 0x7F, 0x85, 0xA0] {
            assert!(Rules::Pdfium.is_regular(b), "{b:#x}");
        }
    }
}
