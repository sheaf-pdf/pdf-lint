//! PDF versions.

use std::fmt;

/// A PDF version: `1.7`, `2.0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// The number before the point.
    pub major: u8,
    /// The number after it.
    pub minor: u8,
}

impl Version {
    /// Version `major.minor`.
    pub const fn new(major: u8, minor: u8) -> Version {
        Version { major, minor }
    }

    /// The version `text` writes, as a header's `%PDF-1.7` or a catalog's
    /// `/Version` does after its prefix: digits, a point, digits, with
    /// white-space around them allowed.
    pub fn parse(text: &[u8]) -> Option<Version> {
        let text = std::str::from_utf8(text).ok()?;
        let (major, minor) = text.trim().split_once('.')?;
        Some(Version::new(major.parse().ok()?, minor.parse().ok()?))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_read_and_order() {
        assert_eq!(Version::parse(b"1.7"), Some(Version::new(1, 7)));
        assert_eq!(Version::parse(b" 2.0\n"), Some(Version::new(2, 0)));
        assert_eq!(Version::parse(b"1"), None);
        assert_eq!(Version::parse(b"1.x"), None);
        assert!(Version::new(1, 7) < Version::new(2, 0));
        assert_eq!(Version::new(1, 4).to_string(), "1.4");
    }
}
