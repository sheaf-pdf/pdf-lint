//! A content stream's tokens, each with where it is and its bytes as
//! written. They're the syntax and nothing of what an operator does, so
//! an operator nobody knows is a token like any other.

use std::iter::FusedIterator;
use std::ops::Range;

use memchr::memchr2;
use pdf_tokenizer::{Kind, is_white_space};

use crate::error::{Error, ErrorKind};
use crate::fault::{self, Fault, Sink};
use crate::inline::{Found, Image, Images, ResCx};
use crate::limits::Limits;
use crate::scan::{Raw, next_raw, tokenizer};

/// A token of a content stream.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Tok<'a> {
    /// An operator: any word that isn't a number, `true`, `false` or
    /// `null`; `BI` among them where it begins no inline image, a keyword
    /// written where its dictionary's key should be, as PDFium reads it
    /// ([`tokens`]).
    Op(&'a [u8]),
    /// An operand as written: a number, a name, a string, an array or a
    /// dictionary as far as PDFium reads it ([`tokens`]), `true`, `false`
    /// or `null`. A closing delimiter with nothing to close is one too, as
    /// PDFium skips it.
    Operand(&'a [u8]),
    /// An inline image, `BI` to `EI`: where its data is.
    InlineImage(Range<usize>),
}

/// The tokens of `s`, in order, each with where it is. Between them is
/// only white-space and comments. After an error there's nothing more.
///
/// Tokens end where PDFium's `CPDF_StreamParser` ends them, as
/// pdf-tokenizer reads them by PDFium's rules: at white-space, 0x80 and
/// 0xFF among it beside ISO 32000-2's six bytes, and delimiters, a word of
/// digits, signs and points is a number, and strings nest. An array or a
/// dictionary is one token, ended where PDFium's `ReadNextObject` ends it:
/// an array at the first `]` read for an element, as an array inside an
/// array, outside any dictionary, opens none (its `[` dropped, as is an
/// element that's no object); a dictionary at its `>>`, or, none, at a key
/// that's no name, of which a string's `(` or a hex string's `<` is the
/// first byte alone, or at a value that's no object, a `]` that so ends it
/// ending an array it's in too; and inside a dictionary, arrays nest.
/// What's after it is read on, as content: `[[1] 2]` is `[[1]`, then `2`
/// and `]`, and `<< (x) 1 >>` is `<< (`, then `x`, an operator, `)`, `1`
/// and `>>`.
/// Where PDFium reads on regardless, this fails: a string, array,
/// dictionary or inline image that doesn't end, one past `limits`, or
/// arrays and dictionaries nested deeper than the 64 levels PDFium reads,
/// past which it reads what's nested as no object (chromium/8076 and
/// chromium/8125; chromium/7881, 512), is an [`Error`] saying where. It
/// never panics, and every token moves it on.
///
/// An inline image, `BI` to `EI`, is one token. Its data is found as
/// PDFium's `ReadInlineStream` finds it, by its first filter:
/// - unfiltered, its exact size: `/H` rows of `/W` samples of its color
///   space's components, `/BPC` bits each, padded to a byte, each number
///   as PDFium reads an integer from it, in 32 bits (a real cut to its
///   whole part, `true` 1, what's no number 0). With no color space, an
///   image mask's, one bit of one component; a name `res` doesn't have
///   reads as none, as in PDFium. An array is counted as PDFium counts it,
///   as one the resources named would be ([`ResCx::read`]).
/// - `/AHx` to its `>`, `/A85` past the first byte that's none of its
///   digits or the white-space it passes over (its `~`, and a `>` after
///   it), and `/RL` to its end-of-data byte, as PDFium's decoders end
///   them.
/// - `/Fl` and `/LZW` decoded, as PDFium decodes them to find their end,
///   to where the decoder stops reading: Flate's to what zlib counts it
///   read (on zlib-rs), its checksum among it; LZW's to its code of 257
///   or the last whole code before the data's end. The first of an array
///   of filters, with the first object of an array of `/DecodeParms`, or
///   its one with its dictionary, each parameter as PDFium reads an
///   integer from it (`/BPC` read as `/BitsPerComponent`). Where PDFium
///   refuses the parameters (`/Colors`, `/BitsPerComponent` or `/Columns`
///   negative, or multiplied past 2^31 − 8) or what decoding gives (an
///   LZW code that isn't in the table yet, LZW data that decodes to
///   nothing, a predictor whose rows would be no bytes, a PNG predictor
///   on nothing): none. (Past an LZW code that isn't in its table, but
///   for the first since a clear code, PDFium's decoder reads on as if it
///   were the next entry, which isn't followed.)
/// - `/DCT` and `/CCF`, which PDFium decodes to find their end too: by a
///   stand-in, unless the reading asks for them to be decoded
///   ([`Limits::coded_ends`], [`CodedEnds`](crate::CodedEnds)'
///   `Decoded`, with the `image-ends` feature). Decoded so, every row, to
///   where the decoder stands after the last: a JPEG on zune-jpeg, from
///   its first start of image, its end counted from there, as PDFium
///   counts it; refused where its header doesn't read or it's narrower
///   than `/W`. PDFium's JPEG decoder, libjpeg-turbo (its Rust one not
///   built, as in the build the crate is held to), reads a JPEG of several
///   scans whole before its first row, to its end of image, and one of
///   one scan as its rows need it, stopping past the marker it runs into
///   after the coded data, restarts read past: there it ends, a comment or
///   an application's segment between the scan and the end of image
///   left out. Fax data on hayro-ccitt, its rows by `/Rows`, or `/H`,
///   each `/Columns` wide (1728 where it isn't written, `/W` where it's
///   0), by `/K`'s coding, `/EndOfLine` and `/EncodedByteAlign` as PDFium
///   reads them; refused where either count is none or past 65,535. Where
///   the decoder can't decode what PDFium's reads on through, damaged
///   data, the end is the stand-in's, and so where zune-jpeg stops short
///   of the end of image of a JPEG of several scans, as at a progressive
///   JPEG's scan of no coded data, which libjpeg-turbo reads on through.
///   Where libjpeg-turbo holds a JPEG's last bits before it runs into the
///   marker after them, it stops short of it, which isn't followed. The
///   stand-in finds the
///   end in the data's bytes: a JPEG's past its first end of image, and
///   fax data's, or a JPEG's with none, at the first `EI` with white-space
///   before it and white-space, a delimiter or the end after it, which
///   agree with PDFium's decoders but where a JPEG's end of image comes
///   before its last (a thumbnail's), the coded bytes hold such an `EI`,
///   or PDFium refuses the data.
/// - a filter PDFium has no decoder for, one of those that finds no end,
///   or an unfiltered image no size counts: none, its tokens read on to
///   the next `EI`. And so for any image, filtered or not, whose size
///   overflows 32 bits as PDFium counts it, or has a negative width, which
///   PDFium reads no data of.
///
/// Past the data so found, the image ends at the next `EI` operator, its
/// tokens read as PDFium reads them, a string one token. Where no `EI`
/// comes after a filter's end, PDFium keeps no stream of the data
/// ([`ImageStream::Missing`](crate::ImageStream::Missing)), and reads on
/// from its start to the next `EI`; but an unfiltered image's counted
/// data with no `EI` after it doesn't end. Its `/L` or `/Length` isn't
/// read, as PDFium reads neither. What finding the ends costs, decoding
/// and reading in vain past an end, is capped for the reading
/// ([`Limits::image_ends`]).
///
/// The dictionary is read as PDFium's `Handle_BeginImage` reads it: a
/// name, its value, and so on, to `ID`, each value, or an object written
/// for a key, read as far as PDFium reads it, as an operand is. A value
/// PDFium reads as no object (a keyword, `ID` among them, a closer, a
/// brace or a dictionary that's none) takes its key out.
/// Where an object that's no name is written for a key, the dictionary
/// ends there, and the data is read from after it, as from after `ID`;
/// where a keyword other than `ID` is, there's no image: `BI` is an
/// operator of its own ([`Tok::Op`]), and what's after it is read as
/// content, the dictionary's tokens among it. A key is matched as written,
/// its `#` escapes not decoded, a value's decoded as PDFium decodes it
/// ([`name`](crate::name)), and an abbreviated name
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
        found: Found::Walk,
        done: false,
        ended: false,
        joins: &[],
        join: 0,
        lone: Vec::new(),
        held: Vec::new(),
        high: None,
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
    /// How the last inline image's data's end was found.
    found: Found,
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
    /// Each `BI` read as no image whose keyword reading hasn't passed, and
    /// where that keyword starts: told ([`Fault::LoneBi`]) as reading
    /// passes it. A `BI` read as content after one is a value in its
    /// dictionary, so its own dictionary is read from the same place with
    /// the keys in the same places, and ends at the same keyword: it's
    /// pushed with that keyword, its dictionary not read again.
    lone: Vec<(usize, usize)>,
    /// The faults of the operand being read where PDFium ends what it
    /// holds before ISO 32000-2 would, or of the white-space before a
    /// token, held to be told in the order of their ends with what a join
    /// divides of it ([`across`](Tokens::across), [`gap`](Tokens::gap)),
    /// the last to be told first.
    held: Vec<(Range<usize>, Fault<'a>)>,
    /// Where the content was last looked at for 0x80 or 0xFF from, the
    /// start of a gap, and where the first of them is from there, or the
    /// content's end where there's none: looked for over the rest of the
    /// content again once a gap starts past it (or before where it was
    /// looked from), so that a gap that ends before it, holding neither,
    /// isn't read for them. None before the first look.
    high: Option<(usize, usize)>,
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
    /// token's own faults, an inline image's, 0x80 and 0xFF read as
    /// white-space, and, over a page's streams joined, a string, a comment
    /// or an inline image that runs from one into the next. A sink
    /// borrowed, `&mut S`, is one, so it can be read after.
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
            found: self.found,
            done: self.done,
            ended: self.ended,
            joins: self.joins,
            join: self.join,
            lone: self.lone,
            held: self.held,
            high: self.high,
            sink,
        }
    }

    /// The content read.
    pub(crate) fn content(&self) -> &'a [u8] {
        self.s
    }

    /// How the last inline image read found its data's end.
    pub(crate) fn found(&self) -> Found {
        self.found
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
            self.gap(self.at..self.s.len());
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
            self.gap(gap..r.start);
        }
        if self
            .lone
            .first()
            .is_some_and(|&(_, keyword)| keyword == r.start)
        {
            for (bi, _) in std::mem::take(&mut self.lone) {
                if self.sink.wants() {
                    let written = &s[bi..r.end];
                    self.sink.fault(bi..r.end, Fault::LoneBi { written });
                }
            }
        }
        let tok = match kind {
            Raw::Op if &s[r.clone()] == b"BI" => {
                self.operands = 0;
                // A `BI` read while one is pending is lone with its keyword
                // (`lone`), its dictionary not read on to that keyword
                // again: content of many reads in one pass.
                let keyword = match self.lone.last() {
                    Some(&(_, keyword)) => keyword,
                    None => match self.images.read(r.start, &mut self.sink) {
                        Ok(Image::Read(end, data, found)) => {
                            self.at = end;
                            self.found = found;
                            if self.sink.wants() {
                                self.across(r.start..end, Some(data.clone()));
                            }
                            return Some(Ok((r.start..end, Tok::InlineImage(data))));
                        }
                        Ok(Image::Lone(keyword)) => keyword.start,
                        Err(e) => return Some(Err(e)),
                    },
                };
                // No image: `BI` alone, read on from after it.
                self.lone.push((r.start, keyword));
                Tok::Op(&s[r.clone()])
            }
            Raw::Op => {
                self.operands = 0;
                Tok::Op(&s[r.clone()])
            }
            Raw::Operand { .. } => {
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
                fault::nesting(s, r.clone(), |at, fault| self.held.push((at, fault)));
                self.held.reverse();
            }
            self.across(r.clone(), None);
            while let Some((at, fault)) = self.held.pop() {
                self.sink.fault(at, fault);
            }
        }
        Some(Ok((r, tok)))
    }

    /// What's between tokens at `gap`, white-space and comments, told where
    /// ISO 32000-2 would read it otherwise: each 0x80 and 0xFF in its
    /// white-space ([`Fault::HighByte`]), looked for only where the content
    /// holds one before the gap's end ([`high`](Tokens::high)), and what a
    /// join divides ([`divided`](Tokens::divided)), in the order of their
    /// ends.
    fn gap(&mut self, gap: Range<usize>) {
        let s = self.s;
        let high = match self.high {
            Some((from, high)) if (from..=high).contains(&gap.start) => high,
            _ => {
                let rest = s.get(gap.start..).unwrap_or_default();
                let high = memchr2(0x80, 0xFF, rest).map_or(s.len(), |i| gap.start + i);
                self.high = Some((gap.start, high));
                high
            }
        };
        if high < gap.end {
            fault::spaces(s, gap.clone(), |at, fault| self.held.push((at, fault)));
        }
        self.held.reverse();
        self.divided(gap, true);
        while let Some((at, fault)) = self.held.pop() {
            self.sink.fault(at, fault);
        }
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
            self.tell(token, Fault::Across { written });
        }
    }

    /// `fault` told to the sink, the faults held before it that end no
    /// later told first.
    fn tell(&mut self, at: Range<usize>, fault: Fault<'a>) {
        while let Some((held, _)) = self.held.last() {
            if held.end > at.end {
                break;
            }
            if let Some((held, before)) = self.held.pop() {
                self.sink.fault(held, before);
            }
        }
        self.sink.fault(at, fault);
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
        // With no join in it, nothing is divided, and it isn't read: what
        // the loop's first look finds, before a tokenizer is made.
        self.pass(region.start);
        if region.start >= region.end || !self.next_join().is_some_and(|join| join < region.end) {
            return false;
        }
        let mut found = false;
        let mut tokens = tokenizer(s, region.start);
        loop {
            // A join between what's read is between tokens, or in
            // white-space.
            let at = tokens.offset();
            self.pass(at);
            if at >= region.end || !self.next_join().is_some_and(|join| join < region.end) {
                return found;
            }
            // A string or a comment, and where it ends; any other token is
            // passed.
            let Some(token) = tokens.next() else {
                return found;
            };
            let piece = matches!(
                token.kind,
                Kind::String { .. } | Kind::HexString { .. } | Kind::Comment
            );
            let (start, end) = (token.start, token.end().min(region.end));
            let Some(join) = self.next_join().filter(|&join| piece && join < end) else {
                continue;
            };
            self.pass(end);
            let divides = token.kind != Kind::Comment || {
                // What the comment reads as comment, past the join, holds
                // a token by ISO 32000-2's rules: its first byte that isn't
                // their white-space (7.2.3) starts no comment.
                let swallowed = &s[join + 1..end];
                let first = swallowed.iter().find(|&&c| !is_white_space(c));
                first.is_some_and(|&c| c != b'%')
            };
            if divides {
                found = true;
                if tell {
                    let written = &s[start..end];
                    self.tell(start..end, Fault::Across { written });
                }
            }
        }
    }
}
