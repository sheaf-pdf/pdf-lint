//! A content stream's tokens, each with where it is and its bytes as
//! written. They're the syntax and nothing of what an operator does, so
//! an operator nobody knows is a token like any other.

use std::iter::FusedIterator;
use std::ops::Range;

use crate::error::{Error, ErrorKind};
use crate::fault::{self, Fault, Sink};
use crate::inline::{Images, ResCx};
use crate::limits::Limits;
use crate::scan::{Raw, hex_string, is_space, line_end, literal_string, next_raw};

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
/// PDFium's `ReadInlineStream` finds it, by its first filter:
/// - unfiltered, its exact size: `/H` rows of `/W` samples of its color
///   space's components, `/BPC` bits each, padded to a byte. With no
///   color space, an image mask's, one bit of one component; a name `res`
///   doesn't have reads as none, as in PDFium.
/// - `/AHx` to its `>`, `/A85` past the first byte that's none of its
///   digits or the white-space it passes over (its `~`, and a `>` after
///   it), and `/RL` to its end-of-data byte, as PDFium's decoders end
///   them.
/// - a filter PDFium has no decoder for, one of those that finds no end,
///   or an unfiltered image no size counts: none, its tokens read on to
///   the next `EI`.
///
/// Past the data so found, the image ends at the next `EI` operator, its
/// tokens read as PDFium reads them, a string one token. Its `/L` or
/// `/Length` isn't read, as PDFium reads neither. PDFium decodes `/Fl`,
/// `/LZW`, `/CCF` and `/DCT` data to find its end; here a JPEG's ends at
/// its end of image, and the rest, or a JPEG with none, at the first `EI`
/// with white-space before it and white-space, a delimiter or the end
/// after, which agree but where the encoded bytes hold such an `EI`, or a
/// JPEG's end of image comes before its last.
///
/// The dictionary is read as PDFium reads it: a key matched as written,
/// its `#` escapes not decoded, a value's decoded, and an abbreviated name
/// (8.9.7, Table 92) read as the full one. Of a key written both
/// abbreviated and in full, the abbreviation's value counts, unless the
/// full name's is an abbreviated name, which PDFium sets over it. The
/// filter is a name, or a string's bytes, naming one; `null`, a
/// dictionary or an empty array is none.
///
/// Where ISO 32000-2 would read the bytes otherwise, a sink given by
/// [`Tokens::with_sink`] is told ([`fault`](crate::fault)); the tokens are
/// the same.
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
        ended: false,
        joins: &[],
        join: 0,
        sink: (),
    }
}

/// A content stream's tokens, each with where it is ([`tokens`]), and
/// where ISO 32000-2 would read them otherwise told to `S`, a
/// [`Sink`](crate::fault::Sink): none for `()`.
#[derive(Debug, Clone)]
pub struct Tokens<'a, 'r, S = ()> {
    s: &'a [u8],
    at: usize,
    lim: Limits,
    /// Operands since the last operator.
    operands: usize,
    images: Images<'a, 'r>,
    done: bool,
    /// Whether the end was reached with no error, and the white-space
    /// after the last token not yet looked at for faults.
    ended: bool,
    /// Where each stream after the first starts, the content being a
    /// page's streams joined ([`Concat`](crate::Concat)), a space before
    /// each.
    joins: &'a [usize],
    /// The next of `joins` not yet passed.
    join: usize,
    sink: S,
}

impl<'a, S: Sink<'a>> Iterator for Tokens<'a, '_, S> {
    type Item = Result<(Range<usize>, Tok<'a>), Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let token = self.step();
        if token.is_none() {
            self.finish();
        }
        token
    }
}

impl<'a, S: Sink<'a>> FusedIterator for Tokens<'a, '_, S> {}

impl<'a, 'r, S> Tokens<'a, 'r, S> {
    /// The same tokens, from where these are, with where ISO 32000-2 would
    /// read them otherwise told to `sink` ([`fault`](crate::fault)): a
    /// token's own faults, an inline image's, and, over a page's streams
    /// joined, a string, a comment or an inline image that runs from one
    /// into the next. A sink borrowed, `&mut S`, is one, so it can be read
    /// after.
    ///
    /// ```
    /// use pdf_content_streams::fault::{Fault, Sink};
    /// use pdf_content_streams::{Limits, ResCx, tokens};
    /// use std::ops::Range;
    ///
    /// #[derive(Default)]
    /// struct Collected<'a>(Vec<(Range<usize>, Fault<'a>)>);
    /// impl<'a> Sink<'a> for Collected<'a> {
    ///     fn fault(&mut self, at: Range<usize>, fault: Fault<'a>) {
    ///         self.0.push((at, fault));
    ///     }
    /// }
    ///
    /// let content = b"q 1.2.3 w <4G> Tj Q";
    /// let mut kept = Collected::default();
    /// let res = ResCx::none();
    /// let read = tokens(content, &res, &Limits::DEFAULT).with_sink(&mut kept);
    /// assert_eq!(read.count(), 6);
    /// let written = &content[2..7];
    /// assert_eq!(kept.0[0], (2..7, Fault::Number { written }));
    /// assert_eq!(kept.0[1].0, 10..14);
    /// assert_eq!(
    ///     kept.0[1].1.to_string(),
    ///     "the hex string <4G> has a byte that's no hex digit"
    /// );
    /// ```
    pub fn with_sink<T: Sink<'a>>(self, sink: T) -> Tokens<'a, 'r, T> {
        Tokens {
            s: self.s,
            at: self.at,
            lim: self.lim,
            operands: self.operands,
            images: self.images,
            done: self.done,
            ended: self.ended,
            joins: self.joins,
            join: self.join,
            sink,
        }
    }

    /// The content read.
    pub(crate) fn content(&self) -> &'a [u8] {
        self.s
    }

    /// The same, the content being a page's streams joined, each of
    /// `joins` where a stream after the first starts, a space before it.
    pub(crate) fn joined(self, joins: &'a [usize]) -> Tokens<'a, 'r, S> {
        Tokens { joins, ..self }
    }
}

impl<'a, S: Sink<'a>> Tokens<'a, '_, S> {
    /// The next token, as [`next`](Iterator::next) gives it, but at the
    /// end with the white-space after the last token not yet looked at
    /// for faults ([`finish`](Tokens::finish)).
    pub(crate) fn step(&mut self) -> Option<Result<(Range<usize>, Tok<'a>), Error>> {
        if self.done {
            return None;
        }
        let token = self.token();
        self.done = !matches!(token, Some(Ok(_)));
        token
    }

    /// At the end, what's after the last token, told where ISO 32000-2
    /// would read it otherwise; once, and nothing after an error.
    pub(crate) fn finish(&mut self) {
        if std::mem::take(&mut self.ended) && self.sink.wants() {
            self.divided(self.at..self.s.len(), true);
        }
    }

    /// The sink, for what the operations tell.
    pub(crate) fn sink(&mut self) -> &mut S {
        &mut self.sink
    }

    fn token(&mut self) -> Option<Result<(Range<usize>, Tok<'a>), Error>> {
        let s = self.s;
        let (r, kind) = match next_raw(s, self.at, self.lim.depth) {
            None => {
                self.ended = true;
                return None;
            }
            Some(Ok(raw)) => raw,
            Some(Err(e)) => return Some(Err(e)),
        };
        let gap = self.at;
        self.at = r.end;
        if self.sink.wants() {
            self.divided(gap..r.start, true);
        }
        let tok = match kind {
            Raw::Op if &s[r.clone()] == b"BI" => {
                self.operands = 0;
                let (end, data) = match self.images.read(r.start, &mut self.sink) {
                    Ok(read) => read,
                    Err(e) => return Some(Err(e)),
                };
                self.at = end;
                if self.sink.wants() {
                    self.across(r.start..end, Some(data.clone()));
                }
                return Some(Ok((r.start..end, Tok::InlineImage(data))));
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
        if self.sink.wants() {
            if let Tok::Operand(written) = tok {
                fault::token(written, r.start, &mut self.sink);
            }
            self.across(r.clone(), None);
        }
        Some(Ok((r, tok)))
    }

    /// The next join not yet passed: the space before a stream.
    fn next_join(&self) -> Option<usize> {
        self.joins.get(self.join).map(|&start| start - 1)
    }

    /// Passes the joins before `at`.
    fn pass(&mut self, at: usize) {
        while self.next_join().is_some_and(|join| join < at) {
            self.join += 1;
        }
    }

    /// The joins to `token`'s end, passed; where one divides it inside
    /// one of ISO 32000-2's lexical tokens, it runs from one stream into
    /// the next ([`Fault::Across`]). An operand's strings and comments, at
    /// any depth in an array or a dictionary, are told each where it is;
    /// an inline image, `data` its data, is told whole, where a join
    /// divides its data or a string or a comment in it. A join between an
    /// array's or a dictionary's tokens is no fault: ISO reads it alike.
    fn across(&mut self, token: Range<usize>, data: Option<Range<usize>>) {
        self.pass(token.start);
        if !self.next_join().is_some_and(|join| join < token.end) {
            return;
        }
        let Some(data) = data else {
            self.divided(token, true);
            return;
        };
        let dict = self.divided(token.start + 2..data.start, false);
        self.pass(data.start);
        let inside = self.next_join().is_some_and(|join| join < data.end);
        let after = self.divided(data.end..token.end, false);
        if dict || inside || after {
            let written = &self.s[token.clone()];
            self.sink.fault(token, Fault::Across { written });
        }
    }

    /// What a join divides in `region`, read as the scanner reads it:
    /// each string a join is inside, and each comment a join is inside
    /// that reads a token of the next stream's first line, before any
    /// comment there, as comment. ISO 32000-2 joins with white-space that
    /// may end a line (7.7.3.3), so its comment would end at the join.
    /// Each is told, if `tell`, as running from one stream into the next
    /// ([`Fault::Across`]); returns whether there's one. The joins in
    /// `region` are passed, and it's read once, however many joins it
    /// holds, and only as far as its last.
    fn divided(&mut self, region: Range<usize>, tell: bool) -> bool {
        let s = self.s;
        let mut found = false;
        let mut at = region.start;
        loop {
            // A join between what's read is between tokens, or in
            // white-space.
            self.pass(at);
            if at >= region.end || !self.next_join().is_some_and(|join| join < region.end) {
                return found;
            }
            // A string or a comment, and where it ends; any other byte is
            // passed, a `<<` whole.
            let (end, piece) = match s[at] {
                b'(' => (literal_string(s, at).unwrap_or(region.end), true),
                b'<' if s.get(at + 1) == Some(&b'<') => (at + 2, false),
                b'<' => (hex_string(s, at).unwrap_or(region.end), true),
                b'%' => (line_end(s, at), true),
                _ => (at + 1, false),
            };
            let (start, end) = (at, end.min(region.end));
            at = end;
            let Some(join) = self.next_join().filter(|&join| piece && join < end) else {
                continue;
            };
            self.pass(end);
            let divides = s[start] != b'%' || {
                // What the comment reads as comment, past the join.
                let swallowed = &s[join + 1..end];
                let first = swallowed.iter().find(|&&c| !is_space(c));
                first.is_some_and(|&c| c != b'%')
            };
            if divides {
                found = true;
                if tell {
                    let written = &s[start..end];
                    self.sink.fault(start..end, Fault::Across { written });
                }
            }
        }
    }
}
