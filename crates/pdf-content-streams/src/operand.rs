//! Operands as written, read when asked: a name, its `#` escapes
//! decoded, a number, any operand as a pdf-objects value, and an array's
//! elements, each as PDFium's content parser reads it.

use std::borrow::Cow;
use std::fmt;
use std::iter::FusedIterator;
use std::ops::Range;

use pdf_objects::{Dict, Form, Object, Str, Value};
use pdf_tokenizer::Kind;
use pdf_tokenizer::decode::{self, NumberError, pdfium};

use crate::fault::{self, Fault, Read, Sink};
use crate::limits::Limits;
use crate::scan::{UNCAPPED, is_number, next_raw, significant, skip_space};

/// An operand as written, and where it is in the content read: what an
/// operation's operands are
/// ([`Operation::operands`](crate::Operation::operands),
/// [`Operation::arg`](crate::Operation::arg)), an array's elements
/// ([`Operand::elements`]) and an inline image's values
/// ([`InlineImage::entry`](crate::InlineImage::entry)). Nothing is read
/// from it until it's asked: [`name`](Operand::name),
/// [`number`](Operand::number), [`object`](Operand::object) and the kind
/// it is, from how it starts.
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
/// assert_eq!(tf.arg(0).unwrap().number(), 12.0);
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

    /// The name it is, its `#` escapes decoded as PDFium decodes them
    /// ([`name`]); None when it isn't one.
    pub fn name(&self) -> Option<Cow<'a, [u8]>> {
        name(self.bytes)
    }

    /// The number it is, as PDFium's content parser reads an operand
    /// (`GetNumber`): a number word ([`is_number`](Operand::is_number)) by
    /// its first 255 bytes, the most of a word that parser keeps, read by
    /// pdf-tokenizer's [`decode::pdfium::number`] as an `f32`; and 0 for
    /// what isn't one, a name, a string, `true` or an array among it. So
    /// `--5` is 0, `1.2.3` 1.2, `4294967296` 0 (a word without a sign past
    /// 32 bits), `+2147483648` 0 (one with a sign past `i32`) and a real
    /// past an `f32`'s range infinite, where ISO 32000-2 (7.3.3) reads
    /// `--5` and `1.2.3` as no number and the others as written
    /// ([`Fault::Number`], [`Fault::OutOfRange`]). An operator that takes
    /// an integer, such as `Tr`, `J` or `j`, reads PDFium's as this cut to
    /// its whole part, where that's in `i32`'s range. An array's elements
    /// read alike ([`elements`](Operand::elements)): a `TJ`'s, a `d`'s.
    ///
    /// [`object`](Operand::object) reads a number otherwise, as
    /// pdf-objects does, at 64 bits.
    ///
    /// ```
    /// use pdf_content_streams::{Limits, ResCx, operations};
    ///
    /// let content = b"--5 1.2.3 4294967296 +2147483648 0.5 7 re";
    /// let read = operations(content, &ResCx::none(), &Limits::DEFAULT).next();
    /// let re = read.unwrap().unwrap();
    /// let numbers: Vec<f32> = re.operands().map(|o| o.number()).collect();
    /// assert_eq!(numbers, [0.0, 1.2, 0.0, 0.0, 0.5, 7.0]);
    /// ```
    pub fn number(&self) -> f32 {
        if !self.is_number() {
            return 0.0;
        }
        pdfium::number(&self.bytes[..self.bytes.len().min(WORD)]).float()
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
    /// struct Collected<'a>(Vec<(Range<usize>, Fault<'a>)>);
    /// impl<'a> Sink<'a> for Collected<'a> {
    ///     fn fault(&mut self, at: Range<usize>, fault: Fault<'a>) {
    ///         self.0.push((at, fault));
    ///     }
    /// }
    ///
    /// let content = b"/Span <</MCID 0 1 /Alt>> BDC";
    /// let res = ResCx::none();
    /// let limits = Limits::DEFAULT;
    /// let bdc = operations(content, &res, &limits).next().unwrap().unwrap();
    /// let mut kept = Collected::default();
    /// let props = bdc.arg(0).unwrap().object_with(&limits, &mut kept);
    /// let mcid = props.unwrap().as_dict().unwrap().get(b"MCID").cloned();
    /// assert_eq!(mcid, Some(Object::from(0)));
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

/// The name an operand is, as PDFium's content parser reads one
/// (`PDF_NameDecode`); None when it isn't one. Of a name longer than 255
/// bytes, its `/` among them, its first 255, the most of a word that
/// parser keeps. A `#` with two bytes after it stands for the byte they
/// make as hex digits, a byte that isn't one read as 0, where ISO 32000-2
/// (7.3.5) reads a `#` as an escape only before two hex digits
/// ([`Fault::Name`]); with fewer, it's itself. Borrowed where there's no
/// `#` to decode.
///
/// ```
/// use pdf_content_streams::name;
///
/// assert_eq!(name(b"/F#311").as_deref(), Some(&b"F11"[..]));
/// assert_eq!(name(b"/F#3G").as_deref(), Some(&b"F0"[..]));
/// assert_eq!(name(b"/F#3").as_deref(), Some(&b"F#3"[..]));
/// assert_eq!(name(b"12"), None);
/// ```
pub fn name(operand: &[u8]) -> Option<Cow<'_, [u8]>> {
    if operand.first() != Some(&b'/') {
        return None;
    }
    let word = &operand[..operand.len().min(WORD)];
    if !word.contains(&b'#') {
        return Some(Cow::Borrowed(&word[1..]));
    }
    Some(Cow::Owned(pdfium::name(word).collect()))
}

/// The name an operand is, its `#` escapes decoded as ISO 32000-2 reads
/// them (7.3.5), every byte of it: a `#` without two hex digits after it
/// stays as written. None when it isn't one. For what's judged by ISO's
/// reading: whether a key is one 8.9.7 reads, an image's last filter.
pub(crate) fn iso_name(operand: &[u8]) -> Option<Cow<'_, [u8]>> {
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

/// An operand as a PDF object: a name's escapes decoded and a string's
/// escapes or hex digits read as PDFium's content parser reads them
/// ([`name`]; a literal string's unescaped CR or CR LF as written, where
/// ISO 32000-2 reads LF, and a string's first 32,767 bytes read, the most
/// that parser keeps), and an array's or a dictionary's elements each read
/// so. A number is read as ISO 32000-2 writes one (7.3.3), as
/// pdf-objects reads it, at 64 bits: [`Operand::number`] reads one as
/// PDFium does. A number past what 64 bits hold reads as pdf-objects
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
        let token = significant(s, *at)?;
        let (start, end, written) = (token.start, token.end(), token.bytes);
        let closer = match token.kind {
            Kind::ArrayClose => close == Some(b"]"),
            Kind::DictClose => close == Some(b">>"),
            _ => false,
        };
        if !closer {
            self.values = self.values.checked_sub(1)?;
        }
        let value = match token.kind {
            Kind::String { closed: true } => Some(Value::String(
                Str::Bytes(literal(written)),
                Some(Form::Literal),
            )),
            Kind::HexString { closed: true } => {
                Some(Value::String(Str::Bytes(hex(written)), Some(Form::Hex)))
            }
            // A string that doesn't end.
            Kind::String { .. } | Kind::HexString { .. } => return None,
            Kind::DictOpen => {
                *at = end;
                let dict = self.dictionary(at, depth.checked_sub(1)?)?;
                return Some(Item::Value(Some(dict)));
            }
            Kind::ArrayOpen => {
                *at = end;
                let array = self.array(at, depth.checked_sub(1)?)?;
                return Some(Item::Value(Some(array)));
            }
            _ if closer => {
                *at = end;
                return Some(Item::Close);
            }
            Kind::Name => Some(Value::Name(name(written)?.into_owned())),
            Kind::Integer | Kind::Real | Kind::Keyword => word(written),
            // A closer with nothing to close, or a brace.
            _ => None,
        };
        *at = end;
        if close.is_some() && self.sink.wants() {
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
/// fits, a real the largest finite float of its sign. A number is read as
/// ISO 32000-2 writes one (7.3.3), so a word only PDFium reads as a
/// number, such as `--5`, is none.
fn word(word: &[u8]) -> Option<Value> {
    let negative = word.first() == Some(&b'-');
    Some(match word {
        b"true" => Value::Bool(true),
        b"false" => Value::Bool(false),
        b"null" => Value::Null,
        n if n.contains(&b'.') => Value::Real(match decode::number(n) {
            Ok(real) => real,
            Err(NumberError::Overflow) if negative => f64::MIN,
            Err(NumberError::Overflow) => f64::MAX,
            Err(NumberError::NotANumber) => return None,
        }),
        n => Value::Integer(match decode::integer(n) {
            Ok(integer) => integer,
            Err(NumberError::Overflow) if negative => i64::MIN,
            Err(NumberError::Overflow) => i64::MAX,
            Err(NumberError::NotANumber) => return None,
        }),
    })
}

/// The most of a word PDFium's content parser keeps: a number or a name
/// longer than this, its `/` counted, is read as its first `WORD` bytes.
pub(crate) const WORD: usize = 255;

/// The most of a string's value PDFium's content parser keeps: a literal or
/// hex string whose bytes, read, are more is read as its first `STRING`.
pub(crate) const STRING: usize = 32_767;

/// A literal string's bytes as PDFium's content parser reads them: `(…)`
/// as lexed, its escapes as ISO 32000-2 reads them (7.3.4.2), an
/// unescaped CR or CR LF as written, where ISO reads LF, and the first
/// [`STRING`] of them.
pub(crate) fn literal(string: &[u8]) -> Vec<u8> {
    pdfium::literal(string).take(STRING).collect()
}

/// A hex string's bytes as PDFium's content parser reads them: `<…>` as
/// lexed, what isn't a hex digit passed over, an odd last digit followed
/// by 0, and the first [`STRING`] of them.
pub(crate) fn hex(string: &[u8]) -> Vec<u8> {
    pdfium::hex(string).take(STRING).collect()
}
