//! Operands as written, read when asked: a name, its `#` escapes
//! decoded, any operand as a pdf-objects value, and an array's elements.

use std::borrow::Cow;
use std::fmt;
use std::iter::FusedIterator;
use std::num::IntErrorKind;
use std::ops::Range;

use pdf_objects::{Dict, Form, Object, Str, Value};

use crate::fault::{self, Fault, Read, Sink};
use crate::limits::Limits;
use crate::scan::{
    UNCAPPED, hex_string, is_number, literal_string, next_raw, skip_space, word_end,
};

/// An operand as written, and where it is in the content read: what an
/// operation's operands are
/// ([`Operation::operands`](crate::Operation::operands),
/// [`Operation::arg`](crate::Operation::arg)), an array's elements
/// ([`Operand::elements`]) and an inline image's values
/// ([`InlineImage::entry`](crate::InlineImage::entry)). Nothing is read
/// from it until it's asked: [`name`](Operand::name),
/// [`object`](Operand::object) and the kind it is, from how it starts.
///
/// ```
/// use pdf_content_streams::{Limits, ResCx, operations};
///
/// let content = b"/F1 12 Tf [(Hel) -20 (lo)] TJ";
/// let res = ResCx::none();
/// let mut read = operations(content, &res, &Limits::DEFAULT);
/// let tf = read.next().unwrap().unwrap();
/// // Operand 0 is the last, as PDFium takes them.
/// let font = tf.arg(1).unwrap();
/// assert_eq!((font.bytes(), font.range()), (&b"/F1"[..], 0..3));
/// assert_eq!(font.name().as_deref(), Some(&b"F1"[..]));
/// assert!(tf.arg(0).unwrap().is_number());
///
/// let tj = read.next().unwrap().unwrap();
/// let shown = tj.arg(0).unwrap();
/// assert!(shown.is_array());
/// let elements: Vec<_> = shown.elements().map(|e| e.bytes()).collect();
/// assert_eq!(elements, [&b"(Hel)"[..], b"-20", b"(lo)"]);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Operand<'a> {
    bytes: &'a [u8],
    /// Where `bytes` starts in the content.
    at: usize,
}

impl<'a> Operand<'a> {
    /// The operand at `range` in `content`.
    pub(crate) fn new(content: &'a [u8], range: Range<usize>) -> Operand<'a> {
        Operand {
            at: range.start,
            bytes: &content[range],
        }
    }

    /// Its bytes, as written.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// Where it is in the content read.
    pub fn range(&self) -> Range<usize> {
        self.at..self.at + self.bytes.len()
    }

    /// The name it is, its `#` escapes decoded ([`name`]); None when it
    /// isn't one.
    pub fn name(&self) -> Option<Cow<'a, [u8]>> {
        name(self.bytes)
    }

    /// It read as a PDF object ([`object`]), under `limits`' caps on how
    /// deep arrays and dictionaries nest and how many values it holds;
    /// None for what isn't one.
    pub fn object(&self, limits: &Limits) -> Option<Object> {
        object(self.bytes, limits)
    }

    /// It read as [`object`](Operand::object) reads it, and where ISO
    /// 32000-2 would read what's inside it otherwise told to `sink`
    /// ([`fault`](crate::fault)), at offsets into the content: each
    /// element's, key's and value's own faults, a key that's an object but
    /// no name, a word that's no object, and a key with no value, as
    /// they're read, those before where reading stops among them. Its own
    /// faults, as a token, are the tokens' to tell, or its array's, for an
    /// element. What's read is the same, whatever the sink.
    ///
    /// ```
    /// use pdf_content_streams::fault::{Fault, Sink};
    /// use pdf_content_streams::{Limits, ResCx, operations};
    /// use pdf_content_streams::pdf_objects::Object;
    /// use std::ops::Range;
    ///
    /// #[derive(Default)]
    /// struct Kept<'a>(Vec<(Range<usize>, Fault<'a>)>);
    /// impl<'a> Sink<'a> for Kept<'a> {
    ///     fn fault(&mut self, at: Range<usize>, fault: Fault<'a>) {
    ///         self.0.push((at, fault));
    ///     }
    /// }
    ///
    /// let content = b"/Span <</MCID 0 1 /Alt>> BDC";
    /// let res = ResCx::none();
    /// let bdc = operations(content, &res, &Limits::DEFAULT).next().unwrap().unwrap();
    /// let mut kept = Kept::default();
    /// let props = bdc.arg(0).unwrap().object_with(&Limits::DEFAULT, &mut kept);
    /// let props = props.unwrap();
    /// assert_eq!(props.as_dict().unwrap().get(b"MCID"), Some(&Object::from(0)));
    /// let (one, alt) = (&content[16..17], &content[18..22]);
    /// assert_eq!(
    ///     kept.0,
    ///     [
    ///         (16..17, Fault::Key { written: one }),
    ///         (18..22, Fault::NoValue { written: alt }),
    ///     ]
    /// );
    /// ```
    pub fn object_with(
        &self,
        limits: &Limits,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Option<Object> {
        let mut read = Reading {
            s: self.bytes,
            base: self.at,
            values: limits.values,
            sink,
        };
        match read.item(&mut 0, limits.depth, None)? {
            Item::Value(object) => object,
            Item::Close => None,
        }
    }

    /// An array's elements, as written, each where it is, in order: each
    /// a number, a name, a string, an array or a dictionary with all it
    /// holds, `true`, `false` or `null`, or what's no object (a closing
    /// delimiter with nothing to close, a word that's no number); none
    /// when it isn't an array. Nothing is gathered: each is found as it's
    /// asked for, where an array's [`object`](Operand::object) finds it.
    pub fn elements(&self) -> Elements<'a> {
        let inner = match self.bytes {
            [b'[', inner @ ..] => inner.strip_suffix(b"]").unwrap_or(inner),
            _ => &[],
        };
        Elements {
            inner,
            base: self.at + 1,
            at: 0,
        }
    }

    /// Whether it's a number as PDFium tells one: a word of digits, signs
    /// and points only. It may still read as none ([`object`]), as `--5`
    /// does.
    pub fn is_number(&self) -> bool {
        !self.bytes.is_empty() && is_number(self.bytes)
    }

    /// Whether it's a name: it starts with `/`.
    pub fn is_name(&self) -> bool {
        self.bytes.first() == Some(&b'/')
    }

    /// Whether it's a string: it starts with `(`, or with a `<` that
    /// isn't `<<`.
    pub fn is_string(&self) -> bool {
        match self.bytes {
            [b'(', ..] => true,
            [b'<', rest @ ..] => rest.first() != Some(&b'<'),
            _ => false,
        }
    }

    /// Whether it's an array: it starts with `[`.
    pub fn is_array(&self) -> bool {
        self.bytes.first() == Some(&b'[')
    }

    /// Whether it's a dictionary: it starts with `<<`.
    pub fn is_dict(&self) -> bool {
        self.bytes.starts_with(b"<<")
    }
}

impl fmt::Debug for Operand<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Operand")
            .field("bytes", &String::from_utf8_lossy(self.bytes))
            .field("range", &self.range())
            .finish()
    }
}

/// An array operand's elements, as written, each where it is
/// ([`Operand::elements`]).
#[derive(Debug, Clone)]
pub struct Elements<'a> {
    /// What's inside the array, its `[` and `]` left out.
    inner: &'a [u8],
    /// Where `inner` starts in the content.
    base: usize,
    /// Where the next element is looked for in `inner`.
    at: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Operand<'a>;

    fn next(&mut self) -> Option<Operand<'a>> {
        // The array was read under a cap on depth, so nothing in it is
        // deeper, and an element is read as the array read it.
        let Some(Ok((r, _))) = next_raw(self.inner, self.at, UNCAPPED) else {
            self.at = self.inner.len();
            return None;
        };
        self.at = r.end;
        Some(Operand {
            at: self.base + r.start,
            bytes: &self.inner[r],
        })
    }
}

impl FusedIterator for Elements<'_> {}

/// The name an operand is, its `#` escapes decoded (ISO 32000-2, 7.3.5);
/// None when it isn't one. A `#` without two hex digits after it stays as
/// written. Borrowed where there's no `#` to decode.
///
/// ```
/// use pdf_content_streams::name;
///
/// assert_eq!(name(b"/F#311").as_deref(), Some(&b"F11"[..]));
/// assert_eq!(name(b"12"), None);
/// ```
pub fn name(operand: &[u8]) -> Option<Cow<'_, [u8]>> {
    let raw = operand.strip_prefix(b"/")?;
    if !raw.contains(&b'#') {
        return Some(Cow::Borrowed(raw));
    }
    let digit = |c: Option<&u8>| (*c? as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        match (raw[i], digit(raw.get(i + 1)), digit(raw.get(i + 2))) {
            (b'#', Some(high), Some(low)) => {
                out.push(high << 4 | low);
                i += 3;
            }
            (c, ..) => {
                out.push(c);
                i += 1;
            }
        }
    }
    Some(Cow::Owned(out))
}

/// An operand as a PDF object: a name's escapes decoded, a string's
/// escapes or hex digits read, and an array's or a dictionary's elements
/// each read so. A number past what 64 bits hold reads as pdf-objects
/// reads one: an integer as the nearest that fits, a real as the largest
/// finite float of its sign. None for what isn't an object, such as a
/// stray closing delimiter or a word of signs, and for one that doesn't
/// end, nests deeper than [`Limits::depth`] or holds more values than
/// [`Limits::values`]. A dictionary keeps every entry, a key written
/// twice too, as pdf-objects' [`Dict`] does, so a lookup takes its last;
/// an entry whose key isn't a name is passed over. A string keeps its
/// [`Form`]. Nothing read has a place ([`Object::at`] is none): an
/// operand's place is in the content, where its token's range is. It's
/// read in one pass, so a deep operand costs what a flat one of its size
/// does, but it nests on the stack a level for each array or dictionary
/// ([`Limits::depth`] says what that takes).
///
/// It reads any operand's bytes, such as a token's
/// ([`Tok::Operand`](crate::Tok::Operand)), which has no [`Operand`] to
/// read it through.
///
/// ```
/// use pdf_content_streams::{Limits, object};
/// use pdf_content_streams::pdf_objects::Object;
///
/// let limits = Limits::DEFAULT;
/// let read = object(b"<</MCID 3 /Lang (en)>>", &limits).unwrap();
/// let mcid = read.as_dict().and_then(|d| d.get(b"MCID"));
/// assert_eq!(mcid.and_then(Object::as_integer), Some(3));
/// assert_eq!(object(b"--5", &limits), None);
/// let big = object(b"99999999999999999999", &limits);
/// assert_eq!(big.as_ref().and_then(Object::as_integer), Some(i64::MAX));
/// ```
pub fn object(operand: &[u8], limits: &Limits) -> Option<Object> {
    Operand {
        bytes: operand,
        at: 0,
    }
    .object_with(limits, &mut ())
}

/// What's next in an operand read as an object.
enum Item {
    /// An object, or None for what isn't one.
    Value(Option<Object>),
    /// The delimiter that ends the array or dictionary being read.
    Close,
}

/// An operand being read as an object: its bytes, where they are in the
/// content, how many more values it may hold, and the sink told where ISO
/// 32000-2 would read what's inside it otherwise.
struct Reading<'a, 's, S: ?Sized> {
    s: &'a [u8],
    /// Where `s` starts in the content.
    base: usize,
    /// The values, objects or not, it may read yet.
    values: usize,
    sink: &'s mut S,
}

impl<'a, S: Sink<'a> + ?Sized> Reading<'a, '_, S> {
    /// The item from `*at`, `*at` moved past it, inside the array or
    /// dictionary that `close` ends, if one's open, `depth` more of which
    /// may nest. None at the end, and for a string, array or dictionary
    /// that doesn't end, one nested too deep, or a value past those it may
    /// read. Inside an array or a dictionary, an item that isn't one is
    /// told to the sink, if it wants: its own faults, and a word that's no
    /// object.
    fn item(&mut self, at: &mut usize, depth: usize, close: Option<&[u8]>) -> Option<Item> {
        let s = self.s;
        let start = skip_space(s, *at);
        let rest = s.get(start..)?;
        let closes = |delimiter: &[u8]| close == Some(delimiter);
        let closer = match *rest.first()? {
            b']' => closes(b"]"),
            b'>' => rest.starts_with(b">>") && closes(b">>"),
            _ => false,
        };
        if !closer {
            self.values = self.values.checked_sub(1)?;
        }
        let (end, value) = match *rest.first()? {
            b'(' => {
                let end = literal_string(s, start).ok()?;
                let string =
                    Value::String(Str::Bytes(literal(&s[start..end])), Some(Form::Literal));
                (end, Some(string))
            }
            b'<' if rest.starts_with(b"<<") => {
                *at = start + 2;
                let dict = self.dictionary(at, depth.checked_sub(1)?)?;
                return Some(Item::Value(Some(dict)));
            }
            b'<' => {
                let end = hex_string(s, start).ok()?;
                let string = Value::String(Str::Bytes(hex(&s[start..end])), Some(Form::Hex));
                (end, Some(string))
            }
            b'[' => {
                *at = start + 1;
                let array = self.array(at, depth.checked_sub(1)?)?;
                return Some(Item::Value(Some(array)));
            }
            b']' if closes(b"]") => {
                *at = start + 1;
                return Some(Item::Close);
            }
            b'>' if rest.starts_with(b">>") && closes(b">>") => {
                *at = start + 2;
                return Some(Item::Close);
            }
            b'>' if rest.starts_with(b">>") => (start + 2, None),
            b')' | b'>' | b']' | b'{' | b'}' => (start + 1, None),
            b'/' => {
                let end = word_end(s, start + 1);
                (end, Some(Value::Name(name(&s[start..end])?.into_owned())))
            }
            _ => {
                let end = word_end(s, start);
                (end, word(&s[start..end]))
            }
        };
        *at = end;
        if close.is_some() && self.sink.wants() {
            let written = &s[start..end];
            let place = self.base + start..self.base + end;
            if fault::token(written, place.start, self.sink) == Read::Keyword {
                self.sink.fault(place, Fault::Keyword { written });
            }
        }
        Some(Item::Value(value.map(Object::new)))
    }

    /// The array whose `[` ends at `*at`: its elements read, to its `]`.
    fn array(&mut self, at: &mut usize, depth: usize) -> Option<Object> {
        let mut elements = Vec::new();
        loop {
            match self.item(at, depth, Some(b"]")) {
                Some(Item::Value(element)) => elements.extend(element),
                Some(Item::Close) => return Some(Object::new(Value::Array(elements))),
                None => return None,
            }
        }
    }

    /// The dictionary whose `<<` ends at `*at`: its entries read, to its
    /// `>>`. A key that's an object but no name, and a key with no value,
    /// told to the sink, if it wants.
    fn dictionary(&mut self, at: &mut usize, depth: usize) -> Option<Object> {
        let mut dict = Dict::new();
        loop {
            let from = *at;
            let key = match self.item(at, depth, Some(b">>"))? {
                Item::Value(key) => key,
                Item::Close => return Some(Object::new(Value::Dict(dict))),
            };
            // Where the key is, its white-space before it left out.
            let place = skip_space(self.s, from)..*at;
            let key = match key.map(Object::into_value) {
                Some(Value::Name(key)) => key,
                Some(_) => {
                    self.tell(place, |written| Fault::Key { written });
                    continue;
                }
                None => continue,
            };
            match self.item(at, depth, Some(b">>"))? {
                Item::Value(Some(value)) => dict.push(key, value),
                Item::Value(None) => {}
                Item::Close => {
                    self.tell(place, |written| Fault::NoValue { written });
                    return Some(Object::new(Value::Dict(dict)));
                }
            }
        }
    }

    /// `fault` of what's at `place` in the operand, told to the sink, at
    /// its place in the content, if it wants faults.
    fn tell(&mut self, place: Range<usize>, fault: impl FnOnce(&'a [u8]) -> Fault<'a>) {
        if self.sink.wants() {
            let written = &self.s[place.clone()];
            let place = self.base + place.start..self.base + place.end;
            self.sink.fault(place, fault(written));
        }
    }
}

/// A word as an object: a number, `true`, `false` or `null`. A number
/// past 64 bits as pdf-objects reads one: an integer the nearest that
/// fits, a real the largest finite float of its sign.
fn word(word: &[u8]) -> Option<Value> {
    Some(match word {
        b"true" => Value::Bool(true),
        b"false" => Value::Bool(false),
        b"null" => Value::Null,
        n if !is_number(n) => return None,
        n => {
            let n = std::str::from_utf8(n).ok()?;
            if n.contains('.') {
                let real: f64 = n.parse().ok()?;
                Value::Real(match real.is_finite() {
                    true => real,
                    false => f64::MAX.copysign(real),
                })
            } else {
                Value::Integer(match n.parse::<i64>() {
                    Ok(integer) => integer,
                    Err(e) => match e.kind() {
                        IntErrorKind::PosOverflow => i64::MAX,
                        IntErrorKind::NegOverflow => i64::MIN,
                        _ => return None,
                    },
                })
            }
        }
    })
}

/// A literal string's bytes, its escapes read: `(…)` as lexed.
pub(crate) fn literal(string: &[u8]) -> Vec<u8> {
    let inner = &string[1..string.len().saturating_sub(1).max(1)];
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while let Some(&c) = inner.get(i) {
        i += 1;
        match c {
            b'\\' => {
                let Some(&e) = inner.get(i) else { break };
                i += 1;
                match e {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0c),
                    b'0'..=b'7' => {
                        let mut v = u32::from(e - b'0');
                        for _ in 0..2 {
                            match inner.get(i) {
                                Some(&d @ b'0'..=b'7') => {
                                    v = v * 8 + u32::from(d - b'0');
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(v as u8);
                    }
                    // A line continued: the end of line is no part of it.
                    b'\r' => i += usize::from(inner.get(i) == Some(&b'\n')),
                    b'\n' => {}
                    e => out.push(e),
                }
            }
            // Any end of line reads as a line feed.
            b'\r' => {
                i += usize::from(inner.get(i) == Some(&b'\n'));
                out.push(b'\n');
            }
            c => out.push(c),
        }
    }
    out
}

/// A hex string's bytes: `<…>` as lexed, what isn't a hex digit passed
/// over, an odd last digit followed by 0.
pub(crate) fn hex(string: &[u8]) -> Vec<u8> {
    let digits: Vec<u8> = string
        .iter()
        .filter_map(|&c| (c as char).to_digit(16).map(|d| d as u8))
        .collect();
    digits
        .chunks(2)
        .map(|pair| pair[0] << 4 | pair.get(1).copied().unwrap_or(0))
        .collect()
}
