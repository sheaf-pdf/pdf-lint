//! The parser: values, and indirect objects, read from a file's bytes or
//! an object stream's decoded data (ISO 32000-2, 7.3), on
//! [`pdf_tokenizer`]. Each value is placed where it was written, in a
//! [`Span`]; what the parser reads past is a [`Fault`], told to a [`Sink`],
//! and what stops a value is an [`Error`].
//!
//! An indirect object (7.3.10) is read in steps, so that which bytes are a
//! stream's data stays the caller's to choose: [`Parser::header`] reads its
//! `n g obj`; [`Parser::body`] its value, or a stream's dictionary to where
//! its data starts, with both extents the data may have, by a `/Length`
//! the caller resolves ([`StreamHead::declared`]) and up to `endstream`
//! ([`StreamHead::scanned`]); and [`Parser::stream`] the rest of the
//! stream, after the extent the caller chose.

use std::fmt;

use pdf_tokenizer::decode::How;
use pdf_tokenizer::{Kind, Token, Tokenizer};

use crate::object::{Data, Dict, Entry, Form, Object, ObjectId, Span, Str, Stream, Value};

/// How deeply arrays and dictionaries may nest in an object, unless a
/// parser is given other [`Limits`]: [`Limits::DEFAULT`]'s `nesting`.
pub const NESTING: usize = 256;

/// The caps a parser reads under, so that what it reads can't make it
/// recurse, or hold in memory, without end. A caller sets them on a copy
/// of [`Limits::DEFAULT`] (`limits.nesting = 64`, say) and gives it to
/// [`Parser::limits`]. Only a value reaches them: a token where a value
/// should be that can't start one is the error it is, under any limits.
///
/// The writer ([`serialize`](crate::serialize)) keeps to the default's
/// nesting, so that a parser reads what it writes, but not to its count
/// of values: it writes a value of more than 2^25, which a parser reads
/// back only under limits that take them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// How deeply arrays and dictionaries may nest: a value inside this
    /// many of them reads, and one inside more doesn't. [`NESTING`], 256,
    /// by default.
    pub nesting: usize,
    /// How many values one call may read, in all: the value, an array's
    /// elements, a dictionary's keys and values, and theirs. What a value
    /// holds in memory grows with its count of values more than with its
    /// bytes (an empty array is two bytes written, and tens in memory), so
    /// this caps that. 2^25 by default.
    pub values: usize,
}

impl Limits {
    /// The limits a parser reads under unless it's given others: values
    /// nested at most [`NESTING`] deep, and at most 2^25 of them in one
    /// call.
    pub const DEFAULT: Limits = Limits {
        nesting: NESTING,
        values: 1 << 25,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}

/// Where a parser says what it read past: a checker keeps it; `()` drops
/// it. A fault borrows from `'a`, the data the parser reads, so a sink can
/// keep it as it is. A sink borrowed, `&mut S` or `&mut dyn Sink`, is a
/// sink too.
pub trait Sink<'a> {
    /// A fault at `at`, which the parser read past.
    fn fault(&mut self, at: Span, fault: Fault<'a>);
}

/// Drops every fault.
impl Sink<'_> for () {
    fn fault(&mut self, _: Span, _: Fault<'_>) {}
}

/// Tells the sink it borrows.
impl<'a, S: Sink<'a> + ?Sized> Sink<'a> for &mut S {
    fn fault(&mut self, at: Span, fault: Fault<'a>) {
        (**self).fault(at, fault);
    }
}

/// What the parser read past: the value is read, as readers read it, and
/// the fault said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Fault<'a> {
    /// An integer outside 64 bits, read as the nearest that fits.
    IntegerOverflow,
    /// A real too large for 64 bits, read as the largest.
    RealOverflow,
    /// A name with a `#` without two hex digits after it, or with `#00`,
    /// NUL, which a name can't hold (ISO 32000-2, 7.3.5), readers reading
    /// each their own way: read with the `#` as itself, and `#00` as NUL.
    Name {
        /// The name as written, its `/` first.
        written: &'a [u8],
        /// Whether it has a `#` without two hex digits after it.
        stray: bool,
        /// Whether it has `#00`.
        nul: bool,
    },
    /// No end of line after a stream's `stream` keyword.
    NoEol,
    /// A lone CR after a stream's `stream` keyword, where ISO 32000-2 wants
    /// CR LF or LF (7.3.8.1).
    LoneCr,
    /// No `endobj` after an indirect object.
    NoEndobj {
        /// The object.
        id: ObjectId,
    },
}

impl fmt::Display for Fault<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Fault::IntegerOverflow => {
                write!(
                    f,
                    "an integer outside 64 bits, read as the nearest that fits"
                )
            }
            Fault::RealOverflow => write!(f, "a real too large for 64 bits, read as the largest"),
            Fault::Name {
                written,
                stray,
                nul,
            } => {
                let written = String::from_utf8_lossy(&written[..written.len().min(40)]);
                let what = match (stray, nul) {
                    (true, true) => "a # without two hex digits after it, and #00, NUL",
                    (true, false) => "a # without two hex digits after it",
                    _ => "#00, NUL, which a name can't hold",
                };
                write!(
                    f,
                    "the name {written} has {what}, which readers read differently"
                )
            }
            Fault::NoEol => write!(f, "no end of line after `stream`"),
            Fault::LoneCr => write!(
                f,
                "a lone CR after `stream`, where ISO 32000-2 wants CR LF or LF"
            ),
            Fault::NoEndobj { id: (n, g) } => write!(f, "{n} {g} obj has no endobj"),
        }
    }
}

/// What stops the parser reading a value. Each offset it names is in the
/// data the parser reads, but [`Error::Header`]'s, as the parser was given
/// it, and [`Error::Extent`]'s, as its caller gave them.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A value nested deeper in arrays and dictionaries than the parser
    /// allows, [`Limits::nesting`] ([`NESTING`] by default).
    Nested {
        /// How deeply they may nest.
        limit: usize,
        /// Where the value nested deeper starts.
        at: usize,
    },
    /// More values in one call than the parser allows, [`Limits::values`]:
    /// the value, an array's elements, a dictionary's keys and values, and
    /// theirs.
    Values {
        /// How many one call may read.
        limit: usize,
        /// Where the value past them starts.
        at: usize,
    },
    /// The data ends where a value should be.
    End,
    /// A reference, `n g R`, whose number doesn't fit 32 bits or whose
    /// generation doesn't fit 16, which numbers no object.
    Reference {
        /// Where it starts.
        at: usize,
        /// Its number, as written.
        number: Vec<u8>,
        /// Its generation, as written.
        generation: Vec<u8>,
    },
    /// A string, literal or hex, that doesn't end.
    String {
        /// Where it starts.
        at: usize,
    },
    /// An array that doesn't end.
    Array {
        /// Where it starts.
        at: usize,
    },
    /// A dictionary that doesn't end.
    Dict {
        /// Where it starts.
        at: usize,
    },
    /// A dictionary's key that isn't a name.
    Key {
        /// Where the key is.
        at: usize,
    },
    /// A keyword other than `true`, `false` and `null` where a value
    /// should be.
    Keyword {
        /// Where it is.
        at: usize,
        /// Its first 32 bytes.
        word: Vec<u8>,
    },
    /// Another token where a value should be: a `]`, `>>`, `{` or `}`, or a
    /// `)` or `>` that closes nothing.
    Token {
        /// Where it is.
        at: usize,
        /// Its bytes.
        bytes: Vec<u8>,
    },
    /// No `n g obj`, an indirect object's header.
    Header {
        /// Where it was looked for: where the parser was, as it was given
        /// when it had read nothing, past the data's end too, where a
        /// cross-reference can point.
        at: usize,
    },
    /// A stream with no `endstream` after its data.
    Endstream {
        /// The object.
        id: ObjectId,
    },
    /// A stream given data it can't have: bytes that don't start where its
    /// data does, end before they start, run past what the parser reads,
    /// or are in other data.
    Extent {
        /// The object.
        id: ObjectId,
        /// Where the data given starts.
        start: usize,
        /// Where the data given ends.
        end: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lossy = String::from_utf8_lossy;
        match self {
            Error::Nested { limit, .. } => {
                write!(f, "arrays and dictionaries nested deeper than {limit}")
            }
            Error::Values { limit, at } => write!(
                f,
                "more than {limit} values in one object, one more at byte {at}"
            ),
            Error::End => write!(f, "the data ends where a value should be"),
            Error::Reference {
                at,
                number,
                generation,
            } => write!(
                f,
                "a reference at byte {at}, {} {} R, numbers no object",
                lossy(number),
                lossy(generation)
            ),
            Error::String { at } => write!(f, "a string at byte {at} that doesn't end"),
            Error::Array { at } => write!(f, "an array at byte {at} that doesn't end"),
            Error::Dict { at } => write!(f, "a dictionary at byte {at} that doesn't end"),
            Error::Key { at } => write!(f, "a dictionary's key at byte {at} isn't a name"),
            Error::Keyword { at, word } => {
                write!(f, "`{}` at byte {at}, where a value should be", lossy(word))
            }
            Error::Token { at, bytes } => {
                write!(
                    f,
                    "{:?} at byte {at}, where a value should be",
                    lossy(bytes)
                )
            }
            Error::Header { at } => write!(f, "no `n g obj` at byte {at}"),
            Error::Endstream { id: (n, g) } => write!(f, "{n} {g} obj's stream has no endstream"),
            Error::Extent {
                id: (n, g),
                start,
                end,
            } => write!(
                f,
                "{n} {g} obj's stream can't have bytes {start}..{end} as its data: they don't start where its data does, end before they start, run past what's read, or are in other data"
            ),
        }
    }
}

impl std::error::Error for Error {}

/// Reads values, or an indirect object, from data, a file's bytes or an
/// object stream's decoded data, from an offset on. Its strings are read
/// as their bytes, or as ciphertext ([`Parser::ciphertext`]), and its
/// spans are in the file, or in an object stream's data
/// ([`Parser::in_stream`]). It reads under [`Limits`]
/// ([`Parser::limits`]). A call that fails leaves the parser where it
/// was, though it has said the faults it read past before the error.
#[derive(Debug, Clone)]
pub struct Parser<'a> {
    data: &'a [u8],
    /// Where it reads next: as it was given until a call reads, past the
    /// data's end too.
    offset: usize,
    /// The object stream whose decoded data `data` is, for its spans.
    stream: Option<ObjectId>,
    /// Whether its strings are read as ciphertext.
    ciphertext: bool,
    /// How deeply values may nest, and how many one call may read.
    limits: Limits,
}

impl<'a> Parser<'a> {
    /// A parser of `data`, reading from `offset`. Past the data's end
    /// there's nothing to read, and the offset is kept as it's given, which
    /// [`Error::Header`] names: a cross-reference can point there.
    pub fn at(data: &'a [u8], offset: usize) -> Parser<'a> {
        Parser {
            data,
            offset,
            stream: None,
            ciphertext: false,
            limits: Limits::DEFAULT,
        }
    }

    /// The parser, its data being object stream `stream`'s decoded data,
    /// which its spans are then in ([`Span::stream`]).
    pub fn in_stream(self, stream: ObjectId) -> Parser<'a> {
        Parser {
            stream: Some(stream),
            ..self
        }
    }

    /// The parser, reading strings as ciphertext ([`Str::Encrypted`]) when
    /// `ciphertext`: an encrypted file's, which its reader doesn't decrypt
    /// as it reads.
    pub fn ciphertext(self, ciphertext: bool) -> Parser<'a> {
        Parser { ciphertext, ..self }
    }

    /// The parser, reading under `limits`: values nested deeper than its
    /// `nesting`, or more of them in one call than its `values`, are an
    /// error ([`Error::Nested`], [`Error::Values`]).
    pub fn limits(self, limits: Limits) -> Parser<'a> {
        Parser { limits, ..self }
    }

    /// Where it reads next: past what it has read, or where it was made
    /// at, as given, when it has read nothing. A call that fails leaves it
    /// where it was.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// The next value.
    pub fn value(&mut self, sink: &mut (impl Sink<'a> + ?Sized)) -> Result<Object, Error> {
        let mut t = Tokenizer::at(self.data, self.offset);
        let value = self.read(&mut t, sink, 0, &mut 0)?;
        self.offset = t.offset();
        Ok(value)
    }

    /// An indirect object's header, `n g obj` (ISO 32000-2, 7.3.10): its
    /// number and generation. Its error names where the parser was, as it
    /// was given when it has read nothing.
    pub fn header(&mut self) -> Result<ObjectId, Error> {
        let offset = self.offset;
        let mut t = Tokenizer::at(self.data, offset);
        let header = |t: &mut Tokenizer<'a>| {
            let n = significant(t).filter(|n| n.kind == Kind::Integer)?;
            let g = significant(t).filter(|g| g.kind == Kind::Integer)?;
            let obj = significant(t)?;
            let number = u32::try_from(n.integer().ok()?).ok()?;
            let generation = u16::try_from(g.integer().ok()?).ok()?;
            is_keyword(Some(&obj), b"obj").then_some((number, generation))
        };
        let id = header(&mut t).ok_or(Error::Header { at: offset })?;
        self.offset = t.offset();
        Ok(id)
    }

    /// Object `id`'s value, after its header: a value, said when `endobj`
    /// doesn't follow it; or a dictionary with `stream` after it, read
    /// through `stream`'s end of line, said when it isn't CR LF or LF, to
    /// where its data starts.
    pub fn body(
        &mut self,
        id: ObjectId,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<Body<'a>, Error> {
        let mut t = Tokenizer::at(self.data, self.offset);
        let body = self.read_body(&mut t, id, sink)?;
        self.offset = t.offset();
        Ok(body)
    }

    /// The stream `head` began, its data `data`, which its caller chose:
    /// one of [`StreamHead::declared`] and [`StreamHead::scanned`]; then
    /// `endstream`, and `endobj`, said when it doesn't follow. Data that
    /// doesn't start where the stream's does, ends before it starts, runs
    /// past what the parser reads, or is in other data, is an error.
    pub fn stream(
        &mut self,
        head: StreamHead<'a>,
        data: Span,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<Object, Error> {
        let StreamHead {
            id,
            dict,
            dict_at,
            start,
            ..
        } = head;
        let within = data.stream == self.stream
            && data.start == start
            && data.start <= data.end
            && data.end <= self.data.len();
        if !within {
            return Err(Error::Extent {
                id,
                start: data.start,
                end: data.end,
            });
        }
        let mut t = Tokenizer::at(self.data, data.end);
        let end = significant(&mut t).filter(|token| is_keyword(Some(token), b"endstream"));
        let end = end.ok_or(Error::Endstream { id })?;
        self.offset = t.offset();
        let from = dict_at.map_or(start, |at| at.start);
        let object = Object::new(Value::Stream(Stream {
            dict,
            data: Data::At(data),
        }))
        .at(self.span(from, end.end()));
        self.endobj(&t, id, &object, sink);
        Ok(object)
    }

    fn span(&self, start: usize, end: usize) -> Span {
        Span::new(self.stream, start, end)
    }

    /// [`Parser::body`], from `t`.
    fn read_body(
        &self,
        t: &mut Tokenizer<'a>,
        id: ObjectId,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Result<Body<'a>, Error> {
        let value = self.read(t, sink, 0, &mut 0)?;
        let at = value.at;
        let mut after = t.clone();
        let next = significant(&mut after);
        let object = match (value.into_value(), next) {
            (Value::Dict(dict), Some(keyword)) if is_keyword(Some(&keyword), b"stream") => {
                *t = after;
                let keyword = self.span(keyword.start, keyword.end());
                match t.eol() {
                    None => sink.fault(keyword, Fault::NoEol),
                    Some(eol) if eol.bytes == b"\r" => {
                        sink.fault(self.span(eol.start, eol.end()), Fault::LoneCr);
                    }
                    Some(_) => {}
                }
                return Ok(Body::Stream(StreamHead {
                    id,
                    dict,
                    dict_at: at,
                    keyword,
                    start: t.offset(),
                    data: self.data,
                    stream: self.stream,
                }));
            }
            (other, _) => {
                let mut object = Object::new(other);
                object.at = at;
                object
            }
        };
        self.endobj(t, id, &object, sink);
        Ok(Body::Object(object))
    }

    /// Says object `id`, `object`, has no `endobj` when `t` doesn't go on
    /// with it.
    fn endobj(
        &self,
        t: &Tokenizer<'a>,
        id: ObjectId,
        object: &Object,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) {
        if !is_keyword(significant(&mut t.clone()).as_ref(), b"endobj") {
            // Every value the parser reads is placed.
            if let Some(at) = object.at {
                sink.fault(at, Fault::NoEndobj { id });
            }
        }
    }

    /// The value `t` goes on with, nested `depth` deep, after `values`
    /// values the call has read.
    fn read(
        &self,
        t: &mut Tokenizer<'a>,
        sink: &mut (impl Sink<'a> + ?Sized),
        depth: usize,
        values: &mut usize,
    ) -> Result<Object, Error> {
        let token = significant(t).ok_or(Error::End)?;
        // Only a value reaches a limit: a token that can't start one is
        // the error it is, below, as it is without them.
        if starts_value(&token) {
            if depth > self.limits.nesting {
                return Err(Error::Nested {
                    limit: self.limits.nesting,
                    at: token.start,
                });
            }
            self.count(values, token.start)?;
        }
        let at = self.span(token.start, token.end());
        let value = match token.kind {
            Kind::Integer => {
                let mut look = t.clone();
                let generation = significant(&mut look).filter(|g| g.kind == Kind::Integer);
                let r = significant(&mut look).filter(|r| is_keyword(Some(r), b"R"));
                if let (Some(g), Some(r)) = (generation, r) {
                    let number = token.integer().ok().and_then(|n| u32::try_from(n).ok());
                    let generation = g.integer().ok().and_then(|g| u16::try_from(g).ok());
                    let (Some(number), Some(generation)) = (number, generation) else {
                        return Err(Error::Reference {
                            at: token.start,
                            number: token.bytes.to_vec(),
                            generation: g.bytes.to_vec(),
                        });
                    };
                    *t = look;
                    let at = self.span(token.start, r.end());
                    return Ok(Object::new(Value::Ref((number, generation))).at(at));
                }
                match token.integer() {
                    Ok(n) => Value::Integer(n),
                    Err(_) => {
                        sink.fault(at, Fault::IntegerOverflow);
                        Value::Integer(if token.bytes.starts_with(b"-") {
                            i64::MIN
                        } else {
                            i64::MAX
                        })
                    }
                }
            }
            Kind::Real => match token.number() {
                Ok(r) => Value::Real(r),
                Err(_) => {
                    sink.fault(at, Fault::RealOverflow);
                    Value::Real(if token.bytes.starts_with(b"-") {
                        f64::MIN
                    } else {
                        f64::MAX
                    })
                }
            },
            Kind::Name => Value::Name(self.name(&token, sink)),
            Kind::String { closed: true } | Kind::HexString { closed: true } => {
                let form = match token.kind {
                    Kind::HexString { .. } => Form::Hex,
                    _ => Form::Literal,
                };
                let bytes = decoded(&token);
                let s = match self.ciphertext {
                    true => Str::Encrypted(bytes),
                    false => Str::Bytes(bytes),
                };
                Value::String(s, Some(form))
            }
            Kind::String { closed: false } | Kind::HexString { closed: false } => {
                return Err(Error::String { at: token.start });
            }
            Kind::ArrayOpen => {
                let mut array = Vec::new();
                loop {
                    let mut look = t.clone();
                    match significant(&mut look) {
                        Some(close) if close.kind == Kind::ArrayClose => {
                            *t = look;
                            let at = self.span(token.start, close.end());
                            return Ok(Object::new(Value::Array(array)).at(at));
                        }
                        Some(_) => array.push(self.read(t, sink, depth + 1, values)?),
                        None => return Err(Error::Array { at: token.start }),
                    }
                }
            }
            Kind::DictOpen => {
                let mut dict = Dict::new();
                loop {
                    let key = significant(t).ok_or(Error::Dict { at: token.start })?;
                    match key.kind {
                        Kind::DictClose => {
                            let at = self.span(token.start, key.end());
                            return Ok(Object::new(Value::Dict(dict)).at(at));
                        }
                        Kind::Name => {
                            self.count(values, key.start)?;
                            let value = self.read(t, sink, depth + 1, values)?;
                            dict.push_entry(Entry {
                                key: self.name(&key, sink),
                                key_at: Some(self.span(key.start, key.end())),
                                value,
                            });
                        }
                        _ => return Err(Error::Key { at: key.start }),
                    }
                }
            }
            Kind::Keyword => match token.bytes {
                b"true" => Value::Bool(true),
                b"false" => Value::Bool(false),
                b"null" => Value::Null,
                word => {
                    return Err(Error::Keyword {
                        at: token.start,
                        word: word[..word.len().min(32)].to_vec(),
                    });
                }
            },
            _ => {
                return Err(Error::Token {
                    at: token.start,
                    bytes: token.bytes.to_vec(),
                });
            }
        };
        Ok(Object::new(value).at(at))
    }

    /// One more value read in the call, starting at `at`, `values`
    /// counting them: an error past the limit.
    fn count(&self, values: &mut usize, at: usize) -> Result<(), Error> {
        *values += 1;
        match *values > self.limits.values {
            true => Err(Error::Values {
                limit: self.limits.values,
                at,
            }),
            false => Ok(()),
        }
    }

    /// A name's bytes, its escapes decoded; and a fault where it is when it
    /// has a `#` without two hex digits after it, or `#00`, NUL, which a
    /// name can't hold (ISO 32000-2, 7.3.5), readers reading each their own
    /// way.
    fn name(&self, token: &Token<'a>, sink: &mut (impl Sink<'a> + ?Sized)) -> Vec<u8> {
        let hows: Vec<How> = (token.pieces().into_iter().flatten())
            .map(|piece| piece.how)
            .filter(|how| matches!(how, How::StrayHash | How::NulEscape))
            .collect();
        if !hows.is_empty() {
            let fault = Fault::Name {
                written: token.bytes,
                stray: hows.contains(&How::StrayHash),
                nul: hows.contains(&How::NulEscape),
            };
            sink.fault(self.span(token.start, token.end()), fault);
        }
        decoded(token)
    }
}

/// An indirect object's value, read after its header.
#[derive(Debug, Clone)]
pub enum Body<'a> {
    /// A value that isn't a stream.
    Object(Object),
    /// A stream, read to where its data starts.
    Stream(StreamHead<'a>),
}

/// A stream read to where its data starts: its dictionary, and both
/// extents its data may have, neither resolved nor chosen.
#[derive(Debug, Clone)]
pub struct StreamHead<'a> {
    id: ObjectId,
    dict: Dict,
    dict_at: Option<Span>,
    keyword: Span,
    start: usize,
    /// What the parser reads, for the extents.
    data: &'a [u8],
    stream: Option<ObjectId>,
}

impl StreamHead<'_> {
    /// Its dictionary.
    pub fn dict(&self) -> &Dict {
        &self.dict
    }

    /// Where its `stream` keyword was written.
    pub fn keyword(&self) -> Span {
        self.keyword
    }

    /// Where its data starts: after `stream` and its end of line.
    pub fn start(&self) -> usize {
        self.start
    }

    /// Its data by a `/Length` of `length`: that many bytes from where it
    /// starts, when `endstream` follows them, past white-space; else none.
    pub fn declared(&self, length: usize) -> Option<Span> {
        let end = (self.start.checked_add(length)).filter(|&end| end <= self.data.len())?;
        let after = significant(&mut Tokenizer::at(self.data, end));
        is_keyword(after.as_ref(), b"endstream").then(|| Span::new(self.stream, self.start, end))
    }

    /// Its data up to the first `endstream`, found by scanning, wherever
    /// it is; or to the end of what the parser reads, when none comes.
    pub fn scanned(&self) -> Span {
        let data = Tokenizer::at(self.data, self.start).data_until(b"endstream");
        Span::new(self.stream, data.start, data.end())
    }
}

/// The next token that isn't white-space or a comment.
fn significant<'t>(t: &mut Tokenizer<'t>) -> Option<Token<'t>> {
    t.find(|token| !matches!(token.kind, Kind::Space | Kind::Comment))
}

/// Whether `token` starts a value: a number, a name, a string that ends,
/// `[`, `<<`, `true`, `false` or `null`.
fn starts_value(token: &Token) -> bool {
    match token.kind {
        Kind::Integer | Kind::Real | Kind::Name | Kind::ArrayOpen | Kind::DictOpen => true,
        Kind::String { closed } | Kind::HexString { closed } => closed,
        Kind::Keyword => matches!(token.bytes, b"true" | b"false" | b"null"),
        _ => false,
    }
}

fn is_keyword(token: Option<&Token>, word: &[u8]) -> bool {
    token.is_some_and(|t| t.kind == Kind::Keyword && t.bytes == word)
}

/// A name's or string's value, its escapes decoded, in a buffer of its
/// exact length; nothing for any other token.
fn decoded(token: &Token) -> Vec<u8> {
    if let Some(value) = token.verbatim() {
        return value.to_vec();
    }
    let Some(pieces) = token.pieces() else {
        return Vec::new();
    };
    let mut value = Vec::with_capacity(pieces.clone().bytes().count());
    value.extend(pieces.bytes());
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuzz::Kept;

    /// `input` read as a value from its start: the value or the error, and
    /// what the sink was told.
    fn value(input: &[u8]) -> (Result<Object, Error>, Kept) {
        let mut kept = Kept::default();
        let value = Parser::at(input, 0).value(&mut kept);
        (value, kept)
    }

    /// The indirect object `input` begins with, its stream's data the
    /// extent `choose` picks: the object or the error, and what the sink
    /// was told.
    fn indirect<'a>(
        input: &'a [u8],
        choose: impl FnOnce(&StreamHead<'a>) -> Span,
    ) -> (Result<Object, Error>, Kept) {
        let mut kept = Kept::default();
        let mut parser = Parser::at(input, 0);
        let object = parser
            .header()
            .and_then(|id| match parser.body(id, &mut kept)? {
                Body::Object(object) => Ok(object),
                Body::Stream(head) => {
                    let data = choose(&head);
                    parser.stream(head, data, &mut kept)
                }
            });
        (object, kept)
    }

    /// The bytes `at` is in `input`.
    fn bytes(input: &[u8], at: Span) -> &[u8] {
        &input[at.start..at.end]
    }

    /// The span of `needle`'s first occurrence in `input`.
    fn found(input: &[u8], needle: &[u8]) -> Span {
        let start = (input.windows(needle.len()))
            .position(|w| w == needle)
            .unwrap();
        Span::new(None, start, start + needle.len())
    }

    /// Each fault says what pdf-lint's reader said, word for word, where
    /// the parser read past it.
    #[test]
    fn each_fault_says_what_it_read_past() {
        let long = format!("/{}#z", "a".repeat(50));
        let cases: [(&[u8], &str, &str); 6] = [
            (
                b"99999999999999999999",
                "99999999999999999999",
                "an integer outside 64 bits, read as the nearest that fits",
            ),
            (
                b"/Ca#zt",
                "/Ca#zt",
                "the name /Ca#zt has a # without two hex digits after it, which readers read differently",
            ),
            (
                b"/N#00",
                "/N#00",
                "the name /N#00 has #00, NUL, which a name can't hold, which readers read differently",
            ),
            (
                b"/A#zz#00",
                "/A#zz#00",
                "the name /A#zz#00 has a # without two hex digits after it, and #00, NUL, which readers read differently",
            ),
            (
                long.as_bytes(),
                &long,
                "the name /aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa has a # without two hex digits after it, which readers read differently",
            ),
            (b"[ 1 ]", "", ""),
        ];
        for (input, written, said) in cases {
            let (value, kept) = value(input);
            assert!(value.is_ok(), "{value:?}");
            let want: Vec<(Span, String)> = match said {
                "" => Vec::new(),
                said => vec![(found(input, written.as_bytes()), said.to_string())],
            };
            assert_eq!(kept.faults, want);
        }
        let real = format!("-{}.5", "9".repeat(320));
        let (value, kept) = self::value(real.as_bytes());
        assert_eq!(*value.unwrap().value(), Value::Real(f64::MIN));
        assert_eq!(
            kept.faults,
            [(
                Span::new(None, 0, real.len()),
                "a real too large for 64 bits, read as the largest".to_string()
            )]
        );
        let (value, _) = self::value(b"-99999999999999999999");
        assert_eq!(*value.unwrap().value(), Value::Integer(i64::MIN));
        // The name reads as written but for its well-formed escapes.
        let (value, _) = self::value(b"/A#zz#00#41");
        assert_eq!(*value.unwrap().value(), Value::Name(b"A#zz\0A".to_vec()));

        let faults = |input: &[u8]| {
            let (object, kept) = indirect(input, StreamHead::scanned);
            assert!(object.is_ok(), "{object:?}");
            let said: Vec<String> = kept.faults.into_iter().map(|(_, s)| s).collect();
            said
        };
        assert_eq!(faults(b"5 0 obj << >> stray"), ["5 0 obj has no endobj"]);
        assert_eq!(
            faults(b"1 0 obj << >>stream xy\nendstream\nendobj"),
            ["no end of line after `stream`"]
        );
        assert_eq!(
            faults(b"1 0 obj << >>stream\rxy\nendstream\nendobj"),
            ["a lone CR after `stream`, where ISO 32000-2 wants CR LF or LF"]
        );
        assert_eq!(
            faults(b"1 0 obj << >>stream\nxy\nendstream"),
            ["1 0 obj has no endobj"]
        );
        assert!(faults(b"1 0 obj << >>stream\r\nxy\nendstream endobj").is_empty());
    }

    /// Each error says what pdf-lint's reader said, word for word.
    #[test]
    fn each_error_says_what_stopped_it() {
        let deep = format!("{}{}", "[".repeat(300), "]".repeat(300));
        let keyword = format!("x{}", "y".repeat(40));
        let cases: [(&[u8], &str); 13] = [
            (
                deep.as_bytes(),
                "arrays and dictionaries nested deeper than 256",
            ),
            (b"", "the data ends where a value should be"),
            (b" % a comment", "the data ends where a value should be"),
            (
                b"<< /Pg 18446744073284681740 0 R >>",
                "a reference at byte 7, 18446744073284681740 0 R, numbers no object",
            ),
            (
                b"[1 70000 R]",
                "a reference at byte 1, 1 70000 R, numbers no object",
            ),
            (b"  (open", "a string at byte 2 that doesn't end"),
            (b"<41 42", "a string at byte 0 that doesn't end"),
            (b"[1 2", "an array at byte 0 that doesn't end"),
            (
                b"<< /Type /Catalog",
                "a dictionary at byte 0 that doesn't end",
            ),
            (b"<< 3 /X >>", "a dictionary's key at byte 3 isn't a name"),
            (b"[endobj]", "`endobj` at byte 1, where a value should be"),
            (
                keyword.as_bytes(),
                "`xyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy` at byte 0, where a value should be",
            ),
            (b">>", "\">>\" at byte 0, where a value should be"),
        ];
        for (input, said) in cases {
            let error = value(input).0.unwrap_err();
            assert_eq!(
                error.to_string(),
                said,
                "{}",
                String::from_utf8_lossy(input)
            );
        }
        let error = |input: &[u8]| indirect(input, StreamHead::scanned).0.unwrap_err();
        assert_eq!(error(b"1 0 R").to_string(), "no `n g obj` at byte 0");
        assert_eq!(
            error(b"1 0 obj << >> stream\nabc").to_string(),
            "1 0 obj's stream has no endstream"
        );
        // Data a stream can't have: after where its data starts, ending
        // before it starts, past the input, and in another stream's data.
        let input = b"7 2 obj << >> stream\nabc endstream";
        let astray: [fn(&StreamHead) -> Span; 4] = [
            |head| Span::new(None, head.start() + 1, head.start() + 2),
            |head| Span::new(None, head.start(), head.start() - 1),
            |head| Span::new(None, head.start(), head.start() + 99),
            |head| Span::new(Some((9, 0)), head.start(), head.start() + 3),
        ];
        let said: Vec<String> = (astray.into_iter())
            .map(|astray| indirect(input, astray).0.unwrap_err().to_string())
            .collect();
        let why = "they don't start where its data does, end before they start, run past what's read, or are in other data";
        assert_eq!(
            said,
            ["22..23", "21..20", "21..120", "21..24"].map(|bytes| format!(
                "7 2 obj's stream can't have bytes {bytes} as its data: {why}"
            ))
        );
    }

    /// A header the parser was given an offset for past the data's end, as
    /// a cross-reference can give, names that offset, not the end.
    #[test]
    fn a_header_past_the_end_names_the_offset_it_was_given() {
        let data = b"1 0 obj 7 endobj";
        let mut parser = Parser::at(data, 10_000);
        assert_eq!(parser.offset(), 10_000);
        let error = parser.header().unwrap_err();
        assert_eq!(error, Error::Header { at: 10_000 });
        assert_eq!(error.to_string(), "no `n g obj` at byte 10000");
        assert_eq!(Parser::at(data, 10_000).value(&mut ()), Err(Error::End));
        // A call that fails leaves the offset where it was, here as given,
        // so the next error names it too.
        assert_eq!(parser.offset(), 10_000);
        assert_eq!(parser.header(), Err(Error::Header { at: 10_000 }));
        assert_eq!(parser.value(&mut ()), Err(Error::End));
        assert_eq!(parser.offset(), 10_000);
        // From its start, it reads.
        let mut parser = Parser::at(data, 0);
        assert_eq!(parser.header(), Ok((1, 0)));
        assert_eq!(parser.offset(), 7);
    }

    /// A call that fails, having read some of the data, leaves the parser
    /// where it was, and one that reads moves it on.
    #[test]
    fn a_call_that_fails_leaves_the_parser_where_it_was() {
        let data = b"1 0 R [1 2 3 /x";
        let mut parser = Parser::at(data, 0);
        assert_eq!(parser.header(), Err(Error::Header { at: 0 }));
        assert_eq!(parser.offset(), 0);
        assert_eq!(*parser.value(&mut ()).unwrap().value(), Value::Ref((1, 0)));
        assert_eq!(parser.offset(), 5);
        assert_eq!(parser.value(&mut ()), Err(Error::Array { at: 6 }));
        assert_eq!(parser.offset(), 5);
        // A stream with no `endstream`, after its body read.
        let data = b"1 0 obj << >> stream\nabc";
        let mut parser = Parser::at(data, 0);
        let id = parser.header().unwrap();
        let Ok(Body::Stream(head)) = parser.body(id, &mut ()) else {
            panic!("a stream");
        };
        let start = parser.offset();
        let data = head.scanned();
        assert_eq!(
            parser.stream(head, data, &mut ()),
            Err(Error::Endstream { id })
        );
        assert_eq!(parser.offset(), start);
    }

    /// A sink can keep each fault as the parser gave it, borrowing the
    /// data, and a sink borrowed, `dyn` too, is one.
    #[test]
    fn a_sink_keeps_faults_as_they_are_or_through_a_borrow() {
        struct Typed<'a>(Vec<Fault<'a>>);
        impl<'a> Sink<'a> for Typed<'a> {
            fn fault(&mut self, _: Span, fault: Fault<'a>) {
                self.0.push(fault);
            }
        }
        let data = b"[/A#zz 99999999999999999999]".to_vec();
        let mut typed = Typed(Vec::new());
        Parser::at(&data, 0).value(&mut typed).unwrap();
        let written = &data[1..6];
        let name = Fault::Name {
            written,
            stray: true,
            nul: false,
        };
        assert_eq!(typed.0, [name, Fault::IntegerOverflow]);
        let mut kept = Kept::default();
        let sink: &mut dyn Sink = &mut kept;
        Parser::at(&data, 0).value(sink).unwrap();
        Parser::at(&data, 0).value(&mut &mut *sink).unwrap();
        assert_eq!(kept.faults.len(), 4);
    }

    /// Every value, at every depth, and every key, is placed in the file,
    /// or in an object stream's decoded data.
    #[test]
    fn values_are_placed_in_the_file_or_an_object_streams_data() {
        let input = b"%x\n<< /A [1 (x) /N] /B 2 0 R /C << /D true >> >> 9";
        for stream in [None, Some((12, 0))] {
            let mut parser = Parser::at(input, 3);
            if let Some(stream) = stream {
                parser = parser.in_stream(stream);
            }
            let o = parser.value(&mut ()).unwrap();
            assert_eq!(parser.offset(), input.len() - 2);
            let at = o.at.unwrap();
            assert_eq!(at.stream, stream);
            assert_eq!(bytes(input, at), &input[3..input.len() - 2]);
            let d = o.as_dict().unwrap();
            let placed = |key: &[u8]| {
                let entry = d.entry(key).unwrap();
                let (key_at, at) = (entry.key_at.unwrap(), entry.value.at.unwrap());
                assert_eq!((key_at.stream, at.stream), (stream, stream));
                (bytes(input, key_at), bytes(input, at))
            };
            assert_eq!(placed(b"A"), (&b"/A"[..], &b"[1 (x) /N]"[..]));
            assert_eq!(placed(b"B"), (&b"/B"[..], &b"2 0 R"[..]));
            assert_eq!(placed(b"C"), (&b"/C"[..], &b"<< /D true >>"[..]));
            let Value::Array(a) = d.get(b"A").unwrap().value() else {
                panic!("an array");
            };
            let each: Vec<&[u8]> = a.iter().map(|e| bytes(input, e.at.unwrap())).collect();
            assert_eq!(each, [&b"1"[..], b"(x)", b"/N"]);
            // The value after it, from where it ended.
            assert_eq!(*parser.value(&mut ()).unwrap().value(), Value::Integer(9));
        }
    }

    /// Each string keeps the form it was written in, hex or literal, at any
    /// depth.
    #[test]
    fn strings_keep_their_form() {
        let (value, kept) = value(b"[<4142> (AB) < 41 42 > [<>]]");
        let Value::Array(a) = value.unwrap().into_value() else {
            panic!("an array");
        };
        let string = |bytes: &[u8], form| Value::String(Str::Bytes(bytes.to_vec()), Some(form));
        let ab = |form| string(b"AB", form);
        let values: Vec<&Value> = a.iter().map(|o| o.value()).collect();
        assert_eq!(
            values[..3],
            [&ab(Form::Hex), &ab(Form::Literal), &ab(Form::Hex)]
        );
        let Value::Array(inner) = a[3].value() else {
            panic!("an array");
        };
        assert_eq!(*inner[0].value(), string(b"", Form::Hex));
        assert!(kept.faults.is_empty());
    }

    /// Strings, hex or literal, at any depth, are ciphertext exactly when
    /// the parser is told they are.
    #[test]
    fn strings_are_ciphertext_when_asked() {
        let input = b"<< /A (x\\)) /B [<78>] /N /x >>";
        for ciphertext in [false, true] {
            let o = Parser::at(input, 0)
                .ciphertext(ciphertext)
                .value(&mut ())
                .unwrap();
            let d = o.as_dict().unwrap();
            let string = |bytes: &[u8], form| match ciphertext {
                true => Value::String(Str::Encrypted(bytes.to_vec()), Some(form)),
                false => Value::String(Str::Bytes(bytes.to_vec()), Some(form)),
            };
            assert_eq!(*d.get(b"A").unwrap().value(), string(b"x)", Form::Literal));
            let Value::Array(b) = d.get(b"B").unwrap().value() else {
                panic!("an array");
            };
            assert_eq!(*b[0].value(), string(b"x", Form::Hex));
            assert_eq!(*d.get(b"N").unwrap().value(), Value::Name(b"x".to_vec()));
        }
    }

    /// Values nest as deep as 256 inside the outermost, and no deeper.
    #[test]
    fn nesting_is_capped_at_256() {
        let nested = |depth: usize, open: &str, inner: &str, close: &str| {
            format!("{}{inner}{}", open.repeat(depth), close.repeat(depth))
        };
        for (open, inner, close) in [("[", "1", "]"), ("<< /K ", "1", " >>"), ("[", "", "]")] {
            // The inner value at depth 256 reads, and at 257 doesn't; an empty
            // array is one deeper than what's in it.
            let reads = match inner {
                "" => 257,
                _ => 256,
            };
            let input = nested(reads, open, inner, close);
            assert!(value(input.as_bytes()).0.is_ok(), "{reads}");
            // Refused where the value nested deeper starts.
            let input = nested(reads + 1, open, inner, close);
            assert_eq!(
                value(input.as_bytes()).0,
                Err(Error::Nested {
                    limit: NESTING,
                    at: open.len() * (NESTING + 1)
                })
            );
        }
    }

    /// The default limits are what a parser reads under unless it's given
    /// others: values nested at most `NESTING` deep, and 2^25 of them.
    #[test]
    fn the_default_limits_nest_values_at_most_nesting_deep() {
        assert_eq!(Limits::DEFAULT.nesting, NESTING);
        assert_eq!(Limits::DEFAULT.values, 1 << 25);
        assert_eq!(Limits::default(), Limits::DEFAULT);
        assert_eq!(Parser::at(b"", 0).limits, Limits::DEFAULT);
    }

    /// A parser reads as many values in one call as its limits allow, its
    /// keys among them, and refuses one more, where it starts, leaving the
    /// parser where it was; each call counts from none.
    #[test]
    fn values_are_capped_by_the_limits() {
        // Each input, its count of values, and where its last starts.
        let cases: [(&[u8], usize, usize); 6] = [
            (b"7", 1, 0),
            (b"1 0 R", 1, 0),
            (b"[1 2 3]", 4, 5),
            (b"[[] << >> ()]", 4, 10),
            (b"<< /A 1 /B [2] >>", 6, 12),
            (b"<< /A << /B 1 /B 2 >> >>", 7, 17),
        ];
        for (input, values, last) in cases {
            let data = [b"%x\n", input, b" 9"].concat();
            let under = |values| Limits {
                values,
                ..Limits::DEFAULT
            };
            let mut parser = Parser::at(&data, 3).limits(under(values));
            assert!(parser.value(&mut ()).is_ok(), "{values}");
            assert_eq!(parser.offset(), data.len() - 2);
            let mut parser = Parser::at(&data, 3).limits(under(values - 1));
            let refused = parser.value(&mut ());
            let past = Error::Values {
                limit: values - 1,
                at: 3 + last,
            };
            assert_eq!(refused, Err(past));
            assert_eq!(parser.offset(), 3);
            // An indirect object's body, as a value.
            let data = [b"1 0 obj ", input, b" endobj"].concat();
            for (values, reads) in [(values, true), (values - 1, false)] {
                let mut parser = Parser::at(&data, 0).limits(under(values));
                let id = parser.header().unwrap();
                let after = parser.offset();
                let body = parser.body(id, &mut ());
                assert_eq!(body.is_ok(), reads, "{values}");
                if !reads {
                    assert!(matches!(body, Err(Error::Values { .. })));
                    assert_eq!(parser.offset(), after);
                }
            }
        }
        // Each call counts from none.
        let mut limits = Limits::DEFAULT;
        limits.values = 2;
        let mut parser = Parser::at(b"[1] [2]", 0).limits(limits);
        assert!(parser.value(&mut ()).is_ok() && parser.value(&mut ()).is_ok());
        assert_eq!(
            Error::Values { limit: 2, at: 9 }.to_string(),
            "more than 2 values in one object, one more at byte 9"
        );
    }

    /// Only a value reaches a limit: a token where a value should be that
    /// can't start one, or none, is the error it is without them; a value
    /// past either limit reaches it where it starts.
    #[test]
    fn only_a_value_reaches_a_limit() {
        let mut limits = Limits::DEFAULT;
        limits.nesting = 0;
        limits.values = 2;
        for input in [&b"<< /A >>"[..], b"<< /A", b"[>>]", b"[endobj]", b"[(open"] {
            let without = Parser::at(input, 0).value(&mut ());
            let reached = matches!(without, Err(Error::Nested { .. } | Error::Values { .. }));
            assert!(without.is_err() && !reached, "{without:?}");
            let mut parser = Parser::at(input, 0).limits(limits);
            assert_eq!(parser.value(&mut ()), without, "{input:?}");
        }
        let input = b"<< /A 1 >>";
        let mut parser = Parser::at(input, 0).limits(limits);
        let nested = Error::Nested { limit: 0, at: 6 };
        assert_eq!(parser.value(&mut ()), Err(nested));
        limits.nesting = 1;
        let mut parser = Parser::at(input, 0).limits(limits);
        assert_eq!(
            parser.value(&mut ()),
            Err(Error::Values { limit: 2, at: 6 })
        );
    }

    /// A parser reads values nested as deep as its limits allow, and
    /// refuses one deeper, leaving the parser where it was.
    #[test]
    fn nesting_is_capped_by_the_limits() {
        for nesting in [0, 1, 7] {
            let mut limits = Limits::DEFAULT;
            limits.nesting = nesting;
            let nested = |depth| format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
            let input = nested(nesting);
            let mut parser = Parser::at(input.as_bytes(), 0).limits(limits);
            assert!(parser.value(&mut ()).is_ok(), "{nesting}");
            let input = nested(nesting + 1);
            let mut parser = Parser::at(input.as_bytes(), 0).limits(limits);
            let nested = Error::Nested {
                limit: nesting,
                at: nesting + 1,
            };
            assert_eq!(parser.value(&mut ()), Err(nested));
            assert_eq!(parser.offset(), 0);
        }
    }

    /// An indirect object that isn't a stream, in its three calls' first
    /// two.
    #[test]
    fn an_indirect_object_reads_after_its_header() {
        let input = b"12 3 obj\n[1 2]\nendobj";
        let mut parser = Parser::at(input, 0);
        assert_eq!(parser.header(), Ok((12, 3)));
        let Ok(Body::Object(o)) = parser.body((12, 3), &mut ()) else {
            panic!("an object");
        };
        assert_eq!(bytes(input, o.at.unwrap()), b"[1 2]");
        assert_eq!(parser.offset(), 14);
    }

    /// A stream's two extents: by a `/Length` that holds, one that doesn't,
    /// and none; each passed to `stream`, and nothing else.
    #[test]
    fn a_streams_data_is_the_extent_its_caller_chose() {
        let input = b"4 0 obj\n<< /Length 6 >>\nstream\r\nabcdef\nendstream\nendobj";
        let mut parser = Parser::at(input, 0);
        let id = parser.header().unwrap();
        let mut kept = Kept::default();
        let Ok(Body::Stream(head)) = parser.body(id, &mut kept) else {
            panic!("a stream");
        };
        assert!(kept.faults.is_empty());
        assert_eq!(bytes(input, head.keyword()), b"stream");
        assert_eq!(
            head.dict().get(b"Length").and_then(Object::as_integer),
            Some(6)
        );
        let start = found(input, b"abcdef").start;
        assert_eq!((head.start(), parser.offset()), (start, start));
        // A length that holds, and with white-space before `endstream`, one
        // a byte shorter or longer than the data that doesn't.
        assert_eq!(head.declared(6), Some(Span::new(None, start, start + 6)));
        assert_eq!(head.declared(7), Some(Span::new(None, start, start + 7)));
        for length in [5, 8, 99, usize::MAX] {
            assert_eq!(head.declared(length), None, "{length}");
        }
        // None: up to `endstream`.
        assert_eq!(bytes(input, head.scanned()), b"abcdef\n");
        for data in [head.declared(6).unwrap(), head.scanned()] {
            let mut parser = parser.clone();
            let o = parser.stream(head.clone(), data, &mut kept).unwrap();
            assert!(kept.faults.is_empty());
            let Value::Stream(s) = o.value() else {
                panic!("a stream");
            };
            assert_eq!(s.data, Data::At(data));
            assert_eq!(s.dict, *head.dict());
            assert_eq!(
                bytes(input, o.at.unwrap()),
                &b"<< /Length 6 >>\nstream\r\nabcdef\nendstream"[..]
            );
            assert_eq!(parser.offset(), input.len() - 7);
        }
        // Data that doesn't start where the stream's does, runs past the
        // input, ends before it starts, or is in another stream's data.
        for data in [
            Span::new(None, start + 1, start + 6),
            Span::new(None, start - 1, start + 6),
            Span::new(None, start, input.len() + 1),
            Span::new(None, start, start - 1),
            Span::new(Some((9, 0)), start, start + 6),
        ] {
            let refused = parser.clone().stream(head.clone(), data, &mut ());
            let want = Error::Extent {
                id: (4, 0),
                start: data.start,
                end: data.end,
            };
            assert_eq!(refused, Err(want), "{data:?}");
        }
    }

    /// A stream whose data holds `endstream`: its `/Length` reaches past
    /// it, scanning stops at it, and either reads, the scanned one with
    /// what follows it read as what follows the stream.
    #[test]
    fn a_stream_whose_data_holds_endstream_has_two_extents() {
        let input = b"1 0 obj << /Length 16 >> stream\nab endstream cd\nendstream endobj";
        let start = found(input, b"ab").start;
        let declared = Span::new(None, start, start + 16);
        assert_eq!(bytes(input, declared), b"ab endstream cd\n");
        let (o, kept) = indirect(input, |head| {
            assert_eq!(head.declared(16), Some(declared));
            assert_eq!(bytes(input, head.scanned()), b"ab ");
            declared
        });
        assert!(kept.faults.is_empty());
        let o = o.unwrap();
        assert_eq!(o.at.unwrap().end, input.len() - 7);
        let (o, kept) = indirect(input, StreamHead::scanned);
        let Value::Stream(s) = o.as_ref().unwrap().value() else {
            panic!("a stream");
        };
        assert_eq!(bytes(input, s.data.at().unwrap()), b"ab ");
        let at = o.unwrap().at.unwrap();
        assert_eq!(bytes(input, at), b"<< /Length 16 >> stream\nab endstream");
        assert_eq!(kept.faults, [(at, "1 0 obj has no endobj".to_string())]);
    }
}
