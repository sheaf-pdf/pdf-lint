//! PDF document version numbers (`%PDF-1.7`, `/Version /2.0`).

use core::fmt;

/// A PDF major and minor version pair, such as `1.7` or `2.0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// Major version number before `.`.
    pub major: u8,
    /// Minor version number after `.`.
    pub minor: u8,
}

impl Version {
    /// Constructs version `major.minor`.
    #[must_use]
    pub const fn new(major: u8, minor: u8) -> Version {
        Version { major, minor }
    }

    /// Parses `<major>.<minor>` from `text`, trimming surrounding PDF whitespace
    /// ([`pdf_tokenizer::is_white_space`]).
    ///
    /// Both components must be non-empty unsigned ASCII digits fitting in `u8`.
    #[must_use]
    pub fn parse(text: &[u8]) -> Option<Version> {
        let start = text
            .iter()
            .position(|&b| !pdf_tokenizer::is_white_space(b))?;
        let end = text
            .iter()
            .rposition(|&b| !pdf_tokenizer::is_white_space(b))?;
        let trimmed = &text[start..=end];
        let dot = trimmed.iter().position(|&b| b == b'.')?;
        let (major, minor) = (&trimmed[..dot], &trimmed[dot + 1..]);
        Some(Version::new(parse_u8(major)?, parse_u8(minor)?))
    }
}

fn parse_u8(digits: &[u8]) -> Option<u8> {
    if digits.is_empty() {
        return None;
    }
    let mut acc = 0u8;
    for &b in digits {
        if !b.is_ascii_digit() {
            return None;
        }
        acc = acc.checked_mul(10)?.checked_add(b - b'0')?;
    }
    Some(acc)
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::*;

    #[test]
    fn versions_read_and_order() {
        assert_eq!(Version::parse(b"1.7"), Some(Version::new(1, 7)));
        assert_eq!(Version::parse(b" 2.0\n"), Some(Version::new(2, 0)));
        assert_eq!(Version::parse(b"\0\x0C1.4\r\t "), Some(Version::new(1, 4)));
        assert_eq!(Version::parse(b"0.0"), Some(Version::new(0, 0)));
        assert_eq!(Version::parse(b"255.255"), Some(Version::new(255, 255)));

        // Rejects missing parts, signs, extra dots, overflow, and non-PDF whitespace.
        for bad in [
            &b""[..],
            b"   ",
            b"1",
            b".7",
            b"1.",
            b"1.x",
            b"+1.7",
            b"1.+7",
            b"-1.7",
            b"1.7.0",
            b"256.0",
            b"1.256",
            "\u{00A0}1.7".as_bytes(),
        ] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }

        assert!(Version::new(1, 7) < Version::new(2, 0));
        assert_eq!(Version::new(1, 4).to_string(), "1.4");
    }
}
