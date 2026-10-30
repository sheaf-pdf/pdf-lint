//! A content stream's tokens, each with where it is and its bytes as
//! written. They're the syntax and nothing of what an operator does, so
//! an operator nobody knows is a token like any other.

use std::iter::FusedIterator;
use std::ops::Range;

use crate::error::{Error, ErrorKind};
use crate::inline::{Images, ResCx};
use crate::limits::Limits;
use crate::scan::{Raw, next_raw};

/// A token of a content stream.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Tok<'a> {
    /// An operator: any word that isn't a number, `true`, `false` or
    /// `null`.
    Op(&'a [u8]),
    /// An operand as written: a number, a name, a string, an array or a
    /// dictionary with all it holds, `true`, `false` or `null`. A closing
    /// delimiter with nothing to close is one too, as PDFium skips it.
    Operand(&'a [u8]),
    /// An inline image, `BI` to `EI`: where its data is.
    InlineImage(Range<usize>),
}

/// The tokens of `s`, in order, each with where it is. Between them is
/// only white-space and comments. After an error there's nothing more.
///
/// Tokens end where PDFium's `CPDF_StreamParser` ends them: at white-space
/// and delimiters, a word of digits, signs and points is a number, strings
/// nest, and an array or a dictionary is one token with all it holds.
/// Where PDFium reads on regardless, this fails: a string, array,
/// dictionary or inline image that doesn't end, or one past `limits`, is
/// an [`Error`] saying where. It never panics, and every token moves it
/// on.
///
/// An inline image, `BI` to `EI`, is one token. Its data is found as
/// PDFium's `ReadInlineStream` finds it, by the first of these rules that
/// applies:
/// - unfiltered, its exact size: `/H` rows of `/W` samples of its color
///   space's components, `/BPC` bits each, padded to a byte. With no
///   color space, an image mask's, one bit of one component; a name `res`
///   doesn't have reads as none, as in PDFium.
/// - `/AHx`, `/A85` and `/RL`, to their end-of-data marker;
/// - `/DCT`, to the JPEG's end of image;
/// - `/L` or `/Length` bytes, where they're given (PDF 2.0).
///
/// Past the data so found, the image ends at the next `EI` operator.
/// When no rule applies, at the first `EI` with white-space before it and
/// white-space, a delimiter or the end after. Only an image's first
/// filter counts, and the abbreviated keys and names read as the full
/// ones.
///
/// ```
/// use pdf_content_streams::{Limits, ResCx, Tok, tokens};
///
/// let content = b"q BI /W 4 /H 1 /BPC 8 /CS /G ID  EI \nEI Q";
/// let read: Vec<_> = tokens(content, &ResCx::none(), &Limits::DEFAULT)
///     .collect::<Result<_, _>>()
///     .unwrap();
/// // The image's data is its 4 bytes, ` EI ` among them.
/// assert_eq!(read[1], (2..39, Tok::InlineImage(32..36)));
/// assert_eq!(read[2], (40..41, Tok::Op(b"Q")));
/// ```
pub fn tokens<'a, 'r>(s: &'a [u8], res: &'r ResCx, limits: &Limits) -> Tokens<'a, 'r> {
    Tokens {
        s,
        at: 0,
        lim: *limits,
        operands: 0,
        images: Images::new(s, res, *limits),
        done: false,
    }
}

/// A content stream's tokens, each with where it is ([`tokens`]).
#[derive(Debug, Clone)]
pub struct Tokens<'a, 'r> {
    s: &'a [u8],
    at: usize,
    lim: Limits,
    /// Operands since the last operator.
    operands: usize,
    images: Images<'a, 'r>,
    done: bool,
}

impl<'a> Iterator for Tokens<'a, '_> {
    type Item = Result<(Range<usize>, Tok<'a>), Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let token = self.token();
        self.done = !matches!(token, Some(Ok(_)));
        token
    }
}

impl FusedIterator for Tokens<'_, '_> {}

impl<'a> Tokens<'a, '_> {
    /// The content read.
    pub(crate) fn content(&self) -> &'a [u8] {
        self.s
    }

    fn token(&mut self) -> Option<Result<(Range<usize>, Tok<'a>), Error>> {
        let s = self.s;
        let (r, kind) = match next_raw(s, self.at, self.lim.depth)? {
            Ok(raw) => raw,
            Err(e) => return Some(Err(e)),
        };
        self.at = r.end;
        let tok = match kind {
            Raw::Op if &s[r.clone()] == b"BI" => {
                self.operands = 0;
                return Some(self.images.read(r.start).map(|(end, data)| {
                    self.at = end;
                    (r.start..end, Tok::InlineImage(data))
                }));
            }
            Raw::Op => {
                self.operands = 0;
                Tok::Op(&s[r.clone()])
            }
            Raw::Operand => {
                self.operands += 1;
                if self.operands > self.lim.operands {
                    return Some(Err(Error {
                        at: r.start,
                        kind: ErrorKind::Operands,
                    }));
                }
                Tok::Operand(&s[r.clone()])
            }
        };
        Some(Ok((r, tok)))
    }
}
