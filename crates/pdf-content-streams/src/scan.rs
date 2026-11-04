//! What a token is and where it ends, as PDFium's `CPDF_StreamParser`
//! ends them: pdf-tokenizer's tokens, read by PDFium's rules
//! ([`Pdfium`]), white-space and comments passed over, a word of digits,
//! signs and points a number, strings nested, an array or a dictionary
//! read as PDFium's `ReadNextObject` reads one ([`walk`]), and a closing
//! delimiter with nothing to close a token of its own. Content's tokens
//! come from here: the rest of the crate takes the ranges it cuts, or the
//! tokenizer it makes.

use std::ops::Range;

use pdf_tokenizer::{Kind, Pdfium, Token, Tokenizer};

use crate::error::{Error, ErrorKind};

/// No cap on how deep arrays and dictionaries nest: for reading again
/// what was read under one, which holds none deeper. What's open is kept
/// in a list on the heap, not on the stack, so any depth is safe.
pub(crate) const UNCAPPED: usize = usize::MAX;

/// How deep PDFium's content parser reads arrays and dictionaries, the
/// outermost at 0, at chromium/8076 and chromium/8125 (chromium/7881's
/// was 512): an object inside one at this depth it reads as none, a word
/// read for it. Where that reads otherwise than ISO 32000-2, it's an
/// error here ([`ErrorKind::Depth`]), as past
/// [`Limits::depth`](crate::Limits::depth).
pub(crate) const PDFIUM_DEPTH: usize = 64;

/// What a raw token is, before an inline image is made one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Raw {
    Op,
    /// An operand, and whether PDFium reads it as an object: a closer with
    /// nothing to close, a brace and a dictionary PDFium reads as none
    /// aren't.
    Operand {
        object: bool,
    },
}

/// The tokens of `s` from `at`, by PDFium's rules.
pub(crate) fn tokenizer(s: &[u8], at: usize) -> Tokenizer<'_, Pdfium> {
    Tokenizer::at(s, at).with_rules(Pdfium)
}

/// The token from `from` on, past white-space and comments; None at the
/// end. An array or a dictionary is read as PDFium reads it ([`walk`]),
/// skimming what it holds where PDFium judges nothing of it; a token that
/// opens neither is read as the walk reads one where an operand is,
/// without entering it.
pub(crate) fn next_raw(
    s: &[u8],
    from: usize,
    depth: usize,
) -> Option<Result<(Range<usize>, Raw), Error>> {
    raw(s, from, depth, true, true)
}

/// [`next_raw`], every token read by the walk: an array's by walking every
/// token it holds, not by skimming them, and one that opens nothing by
/// entering the walk, not before it. What the fuzz body holds the skim and
/// the shortcut to, from where each token is read, so the tokens and every
/// fault they tell are the walk's.
#[cfg(any(test, fuzzing))]
pub(crate) fn next_raw_walked(
    s: &[u8],
    from: usize,
    depth: usize,
) -> Option<Result<(Range<usize>, Raw), Error>> {
    raw(s, from, depth, false, false)
}

/// [`next_raw`], skimming an array's tokens where PDFium judges nothing
/// of them if `skim`, and reading a token that opens nothing before the
/// walk if `leaf`.
fn raw(
    s: &[u8],
    from: usize,
    depth: usize,
    skim: bool,
    leaf: bool,
) -> Option<Result<(Range<usize>, Raw), Error>> {
    let mut tokens = tokenizer(s, from);
    let token = tokens.next_significant()?;
    if token.kind == Kind::Keyword && !matches!(token.bytes, b"true" | b"false" | b"null") {
        return Some(Ok((token.range(), Raw::Op)));
    }
    // A token that opens nothing, read here as the walk reads one at the
    // top: a string that doesn't end an error where it starts, an object
    // that holds none an object, and a closer, a brace or `>` a word that's
    // none. Only an array or a dictionary is walked.
    let operand = |object| Some(Ok((token.range(), Raw::Operand { object })));
    match token.kind {
        _ if !leaf => {}
        Kind::ArrayOpen | Kind::DictOpen => {}
        Kind::String { closed: false } => {
            let (at, kind) = (token.start, ErrorKind::String);
            return Some(Err(Error { at, kind }));
        }
        Kind::HexString { closed: false } => {
            let (at, kind) = (token.start, ErrorKind::HexString);
            return Some(Err(Error { at, kind }));
        }
        // `true`, `false` and `null` the only keywords left.
        Kind::Integer
        | Kind::Real
        | Kind::Name
        | Kind::String { .. }
        | Kind::HexString { .. }
        | Kind::Keyword => return operand(true),
        _ => return operand(false),
    }
    Some(
        walk_from(s, token, tokens, TOP, depth, skim, &mut ()).map(|read| {
            (
                read.at,
                Raw::Operand {
                    object: read.object,
                },
            )
        }),
    )
}

/// Where an object is read, as PDFium's `ReadNextObject` is asked for
/// it: how deep, and whether inside an array.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Place {
    level: usize,
    in_array: bool,
}

/// An operand: content's next object, or an inline image's value.
pub(crate) const TOP: Place = Place {
    level: 0,
    in_array: false,
};

/// An element of an array read at the [`TOP`], outside any dictionary.
pub(crate) const ELEMENT: Place = Place {
    level: 1,
    in_array: true,
};

/// What a [`walk`] read: where, from its first token to the end of the
/// last word it read, and whether PDFium makes an object of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Walked {
    pub(crate) at: Range<usize>,
    pub(crate) object: bool,
}

/// A step of a [`walk`], told to a [`Visit`] as it's read.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Event<'a> {
    /// An object that holds none, read where an object is: a number, a
    /// name, a string, `true`, `false` or `null`.
    Leaf(Token<'a>),
    /// A word read where an object is, which is none: a keyword, a closer
    /// that closes nothing, a brace, or a `[` inside an array, outside any
    /// dictionary, which PDFium drops. In an array it's passed over; for
    /// a dictionary's value, it makes the dictionary none ([`Unmade`]
    /// follows).
    ///
    /// [`Unmade`]: Event::Unmade
    Word(Token<'a>),
    /// A `[` or `<<` opening an array or a dictionary.
    Open(Token<'a>),
    /// A name read for a dictionary's key.
    Key(Token<'a>),
    /// A word read for a dictionary's key that's no name, a string's `(`
    /// or a hex string's `<` its first byte alone, the rest read on after
    /// it: the dictionary is none ([`Unmade`](Event::Unmade) follows).
    NotKey(Token<'a>),
    /// A `]` or a `>>` closing the innermost array or dictionary.
    Close(Token<'a>),
    /// The innermost dictionary is none, its reading stopped at the word
    /// read last.
    Unmade,
}

impl<'a> Event<'a> {
    /// The token read, but for [`Unmade`](Event::Unmade), which reads none.
    pub(crate) fn token(&self) -> Option<Token<'a>> {
        match *self {
            Event::Leaf(token)
            | Event::Word(token)
            | Event::Open(token)
            | Event::Key(token)
            | Event::NotKey(token)
            | Event::Close(token) => Some(token),
            Event::Unmade => None,
        }
    }
}

/// What's told each step of a [`walk`]; `()` heeds none.
pub(crate) trait Visit<'a> {
    fn visit(&mut self, event: Event<'a>);
}

impl<'a> Visit<'a> for () {
    #[inline]
    fn visit(&mut self, _: Event<'a>) {}
}

/// The object from `from` in `s`, read as PDFium's `ReadNextObject` reads
/// one at `place`, each step told to `visit`; None at the end. Its
/// rules:
///
/// - An array's elements are each read so in turn. A `[` among them, with
///   no dictionary open around it, opens no array: PDFium drops it, as it
///   drops an element that's no object (a keyword, a closer that closes
///   nothing, a brace). The first `]` read for an element ends the array,
///   even one that ends a dictionary inside it that's none.
/// - A dictionary's key is the next word: a name, or its `>>`. Any other
///   makes the dictionary none, reading stopped after that word, of which
///   a string's `(` or a hex string's `<` is the first byte alone. A value
///   is read as an object, arrays nesting freely in it; one that's none
///   makes the dictionary none.
/// - Past [`PDFIUM_DEPTH`], what PDFium reads as none, a `]` that closes an
///   array aside, and an array or a dictionary opened past `cap`, are an
///   [`ErrorKind::Depth`]; an array or a dictionary that runs to the end
///   of `s` an [`ErrorKind::Array`] or [`ErrorKind::Dict`] where the
///   innermost starts, and a string read whole that does an
///   [`ErrorKind::String`] or [`ErrorKind::HexString`] where it starts,
///   where PDFium reads on regardless.
///
/// What's open is kept on the heap, so it's read in one pass however deep.
/// With `skim`, an array's tokens are skimmed ([`Tokenizer::skim`]) where
/// PDFium judges nothing of them: all but what opens or closes an array or
/// a dictionary, or a string that doesn't end, which aren't told.
pub(crate) fn walk<'a>(
    s: &'a [u8],
    from: usize,
    place: Place,
    cap: usize,
    skim: bool,
    visit: &mut impl Visit<'a>,
) -> Option<Result<Walked, Error>> {
    let mut tokens = tokenizer(s, from);
    let first = tokens.next_significant()?;
    Some(walk_from(s, first, tokens, place, cap, skim, visit))
}

/// An array or a dictionary open in a [`walk`]: where it starts, and, for a
/// dictionary, whether its key is read, so its value is next.
#[derive(Clone, Copy)]
struct Open {
    at: usize,
    dict: bool,
    keyed: bool,
}

/// [`walk`], from its first token, `first`, `tokens` reading on after it.
fn walk_from<'a>(
    s: &'a [u8],
    first: Token<'a>,
    mut tokens: Tokenizer<'a, Pdfium>,
    place: Place,
    cap: usize,
    skim: bool,
    visit: &mut impl Visit<'a>,
) -> Result<Walked, Error> {
    let start = first.start;
    let mut open: Vec<Open> = Vec::new();
    // How many of those open are dictionaries, and arrays.
    let (mut dicts, mut arrays) = (0usize, 0usize);
    let mut next = Some(first);
    loop {
        let innermost = open.last().copied();
        let key = innermost.is_some_and(|o| o.dict && !o.keyed);
        let in_array = innermost.is_some_and(|o| !o.dict);
        // How deep what's read next is, where an object is read.
        let level = place.level + open.len();
        let token = match next.take() {
            Some(token) => token,
            None => {
                let read = if key {
                    key_word(s, &mut tokens)
                } else if skim && in_array && level <= PDFIUM_DEPTH {
                    tokens.skim()
                } else {
                    tokens.next_significant()
                };
                match read {
                    Some(token) => token,
                    // What's open runs to the end.
                    None => {
                        let (at, dict) = innermost.map_or((start, false), |o| (o.at, o.dict));
                        let kind = if dict {
                            ErrorKind::Dict
                        } else {
                            ErrorKind::Array
                        };
                        return Err(Error { at, kind });
                    }
                }
            }
        };
        // What's read here: whether it's an object, and the last word read.
        let (mut object, last) = if key {
            match token.kind {
                Kind::Name => {
                    visit.visit(Event::Key(token));
                    if let Some(o) = open.last_mut() {
                        o.keyed = true;
                    }
                    continue;
                }
                Kind::DictClose => {
                    open.pop();
                    dicts -= 1;
                    visit.visit(Event::Close(token));
                    (true, token)
                }
                _ => {
                    visit.visit(Event::NotKey(token));
                    open.pop();
                    dicts -= 1;
                    visit.visit(Event::Unmade);
                    (false, token)
                }
            }
        } else {
            let closes = in_array && token.kind == Kind::ArrayClose;
            if level > PDFIUM_DEPTH && !closes {
                let at = innermost.map_or(token.start, |o| o.at);
                let kind = ErrorKind::Depth;
                return Err(Error { at, kind });
            }
            match token.kind {
                _ if closes => {
                    open.pop();
                    arrays -= 1;
                    visit.visit(Event::Close(token));
                    (true, token)
                }
                Kind::DictOpen | Kind::ArrayOpen
                    if token.kind == Kind::DictOpen
                        || dicts > 0
                        || !(place.in_array || arrays > 0) =>
                {
                    if level >= cap {
                        let (at, kind) = (token.start, ErrorKind::Depth);
                        return Err(Error { at, kind });
                    }
                    let dict = token.kind == Kind::DictOpen;
                    open.push(Open {
                        at: token.start,
                        dict,
                        keyed: false,
                    });
                    if dict {
                        dicts += 1;
                    } else {
                        arrays += 1;
                    }
                    visit.visit(Event::Open(token));
                    continue;
                }
                Kind::String { closed: false } => {
                    let (at, kind) = (token.start, ErrorKind::String);
                    return Err(Error { at, kind });
                }
                Kind::HexString { closed: false } => {
                    let (at, kind) = (token.start, ErrorKind::HexString);
                    return Err(Error { at, kind });
                }
                Kind::Integer
                | Kind::Real
                | Kind::Name
                | Kind::String { .. }
                | Kind::HexString { .. } => {
                    visit.visit(Event::Leaf(token));
                    (true, token)
                }
                Kind::Keyword if matches!(token.bytes, b"true" | b"false" | b"null") => {
                    visit.visit(Event::Leaf(token));
                    (true, token)
                }
                // A keyword, a closer that closes nothing, a brace, or a
                // `[` PDFium drops.
                _ => {
                    visit.visit(Event::Word(token));
                    (false, token)
                }
            }
        };
        // Where what's read goes: an element is kept or dropped, a value
        // kept or making its dictionary none, and so on out.
        loop {
            match open.last_mut() {
                None => {
                    return Ok(Walked {
                        at: start..last.end(),
                        object,
                    });
                }
                Some(o) if o.dict => {
                    if object {
                        o.keyed = false;
                        break;
                    }
                    open.pop();
                    dicts -= 1;
                    visit.visit(Event::Unmade);
                }
                Some(_) => {
                    if object || last.kind != Kind::ArrayClose {
                        break;
                    }
                    open.pop();
                    arrays -= 1;
                    visit.visit(Event::Close(last));
                    object = true;
                }
            }
        }
    }
}

/// The next word where a dictionary's key is written, past white-space and
/// comments, as PDFium reads it: a token, but a string's `(` or a hex
/// string's `<`, which is that byte alone, `tokens` reading on after it,
/// the string's bytes not read, so a string that a key opens isn't read
/// once for it and again for what's after; None at the end.
fn key_word<'a>(s: &'a [u8], tokens: &mut Tokenizer<'a, Pdfium>) -> Option<Token<'a>> {
    loop {
        let at = tokens.offset();
        let kind = match s.get(at..)? {
            [c, ..] if is_space(*c) || *c == b'%' => {
                tokens.next();
                continue;
            }
            [b'(', ..] => Kind::String { closed: false },
            [b'<', next @ ..] if next.first() != Some(&b'<') => Kind::HexString { closed: false },
            _ => return tokens.next_significant(),
        };
        *tokens = tokenizer(s, at + 1);
        let bytes = &s[at..at + 1];
        return Some(Token {
            kind,
            start: at,
            bytes,
        });
    }
}

/// The elements of an array token, each as written and whether PDFium
/// reads it as an object, read as [`Operand::elements`] reads them.
///
/// [`Operand::elements`]: crate::Operand::elements
pub(crate) fn elements(array: &[u8]) -> Vec<(&[u8], bool)> {
    let mut out = Vec::new();
    let mut at = 1;
    while let Some(read) = element(array, &mut at) {
        out.push((&array[read.at], read.object));
    }
    out
}

/// The element of `array`, an array token read at the [`TOP`], from
/// `*at`, `*at` moved past it: where it is and whether it's an object;
/// None past the last, at the `]` that ends the array, or an error. A
/// dictionary that's none whose reading stops at a `]` ends the array
/// too, and that `]` is the token's last byte, so there's none after it.
/// Read again, it was read under a cap on depth, so nothing in it is
/// deeper.
pub(crate) fn element(array: &[u8], at: &mut usize) -> Option<Walked> {
    match walk(array, *at, ELEMENT, UNCAPPED, true, &mut ()) {
        Some(Ok(read)) if array[read.at.start] != b']' => {
            *at = read.at.end;
            Some(read)
        }
        _ => {
            *at = array.len();
            None
        }
    }
}

/// Whether a word is a number, as PDFium tells one: digits, signs and
/// points only, an integer or a real by PDFium's rules. Its bytes are
/// looked at, not read as a token: a word of those bytes alone is the
/// one token they read as, of one of those kinds, and any other byte
/// makes the first token another kind or ends it short of the word.
pub(crate) fn is_number(word: &[u8]) -> bool {
    !word.is_empty() && (word.iter()).all(|&c| matches!(c, b'0'..=b'9' | b'+' | b'-' | b'.'))
}

/// [`is_number`] as the word's first token tells it: of the kind PDFium's
/// rules make a number, and the whole word. What a test holds the bytes'
/// look to.
#[cfg(test)]
pub(crate) fn is_number_token(word: &[u8]) -> bool {
    tokenizer(word, 0).next().is_some_and(|t| {
        matches!(t.kind, Kind::Integer | Kind::Real) && t.bytes.len() == word.len()
    })
}

/// Whether `c` is white-space by PDFium's rules: ISO 32000-2's six bytes
/// (7.2.3), and 0x80 and 0xFF.
pub(crate) fn is_space(c: u8) -> bool {
    pdf_tokenizer::Rules::Pdfium.is_white_space(c)
}
