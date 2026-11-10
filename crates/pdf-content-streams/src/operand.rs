//! Operands as written, read when asked: a name, its `#` escapes
//! decoded, a number, any operand as a pdf-objects value, and an array's
//! elements, each as PDFium's content parser reads it.

use std::borrow::Cow;
use std::fmt;
use std::iter::FusedIterator;
use std::ops::Range;

use pdf_objects::{Dict, Form, Object, Str, Value};
use pdf_tokenizer::decode::pdfium::{self, Number};
use pdf_tokenizer::{Kind, Token};

use crate::fault::{self, Fault, Read, Sink};
use crate::limits::Limits;
use crate::scan::{Event, TOP, Visit, element, is_number, tokenizer, walk, walk_from};

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
    #[must_use]
    pub const fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// Where it is in the content read.
    #[must_use]
    pub const fn range(&self) -> Range<usize> {
        self.at..self.at + self.bytes.len()
    }

    /// The name it is, its `#` escapes decoded as PDFium decodes them
    /// ([`name`]); None when it isn't one.
    #[must_use]
    pub fn name(&self) -> Option<Cow<'a, [u8]>> {
        name(self.bytes)
    }

    /// The number it is, as PDFium's content parser reads an operand
    /// (`GetNumber`): a number word ([`is_number`](Operand::is_number)) by
    /// its first 255 bytes, the most of a word that parser keeps, read by
    /// pdf-tokenizer's [`decode::pdfium::number`](pdfium::number) as an
    /// `f32`; and 0 for what isn't one, a name, a string, `true` or an array
    /// among it. So `--5` is 0, `1.2.3` 1.2, `4294967296` 0 (a word without a
    /// sign past 32 bits), `+2147483648` 0 (one with a sign past `i32`) and a
    /// real past an `f32`'s range infinite, where ISO 32000-2 (7.3.3) reads
    /// `--5` and `1.2.3` as no number and the others as written
    /// ([`Fault::Number`], [`Fault::OutOfRange`]). An operator that takes an
    /// integer, such as `Tr`, `J` or `j`, reads PDFium's as this cut to its
    /// whole part, where that's in `i32`'s range. An array's elements read
    /// alike ([`elements`](Operand::elements)): a `TJ`'s, a `d`'s.
    ///
    /// [`object`](Operand::object) reads the same number, held as
    /// pdf-objects holds one, at 64 bits.
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
    #[must_use]
    pub fn number(&self) -> f32 {
        if !self.is_number() {
            return 0.0;
        }
        pdfium::number(&self.bytes[..self.bytes.len().min(WORD)]).float()
    }

    /// It read as a PDF object ([`object`]), under `limits`' caps on how
    /// deep arrays and dictionaries nest and how many values it holds;
    /// None for what isn't one.
    #[must_use]
    pub fn object(&self, limits: &Limits) -> Option<Object> {
        object(self.bytes, limits)
    }

    /// It read as [`object`](Operand::object) reads it, and where ISO
    /// 32000-2 would read what's inside it otherwise told to `sink`
    /// ([`fault`](crate::fault)), at offsets into the content: each
    /// element's, key's and value's own faults, a word that's no object,
    /// and, where a dictionary is none as PDFium reads it, a key that's no
    /// name or a key with no value, as they're read, those before where
    /// reading stops among them. Its own faults, as a token, are the
    /// tokens' to tell, or its array's, for an element: among them where
    /// PDFium ends what it holds before ISO would
    /// ([`Fault::NestedArray`], [`Fault::NoDict`]). What's read is the
    /// same, whatever the sink.
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
    /// let content = b"/Span <</MCID 0 /Alt (A) 1 /Lang>> BDC";
    /// let res = ResCx::none();
    /// let limits = Limits::DEFAULT;
    /// let bdc = operations(content, &res, &limits).next().unwrap().unwrap();
    /// let mut kept = Collected::default();
    /// // A key that's no name: PDFium reads no dictionary, and ends the
    /// // operand there, `BDC`'s last operands what follows.
    /// let props = bdc.arg(2).unwrap();
    /// assert_eq!(props.bytes(), b"<</MCID 0 /Alt (A) 1");
    /// assert_eq!(props.object_with(&limits, &mut kept), None);
    /// assert_eq!(kept.0, [(25..26, Fault::Key { written: b"1" })]);
    /// assert_eq!(bdc.arg(0).unwrap().bytes(), b">>");
    /// // Without the key that's no name, the list is read.
    /// let read = pdf_content_streams::object(b"<</MCID 0 /Alt (A)>>", &limits);
    /// let mcid = read.unwrap().as_dict().unwrap().get(b"MCID").cloned();
    /// assert_eq!(mcid, Some(Object::from(0)));
    /// ```
    pub fn object_with(
        &self,
        limits: &Limits,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Option<Object> {
        // What opens nothing is read where it starts, as the walk reads it at
        // the top, where nothing inside it is told: an object that holds
        // none, if a value may be read; none for a string that doesn't end or
        // a word that's no object.
        let mut tokens = tokenizer(self.bytes, 0);
        let first = tokens.next_significant()?;
        let one = limits.values > 0;
        match first.kind {
            Kind::ArrayOpen | Kind::DictOpen => {}
            Kind::LiteralString | Kind::HexString if !first.closed => return None,
            Kind::Integer | Kind::Real | Kind::Name | Kind::LiteralString | Kind::HexString => {
                return one.then(|| leaf(first)).flatten();
            }
            Kind::Keyword if matches!(first.bytes, b"true" | b"false" | b"null") => {
                return one.then(|| leaf(first)).flatten();
            }
            _ => return None,
        }
        let mut read = Reading::new(self, limits.values, sink, true);
        walk_from(
            self.bytes,
            first,
            tokens,
            TOP,
            limits.depth,
            false,
            &mut read,
        )
        .ok()?;
        read.read?
    }

    /// It read as [`object`](Operand::object) reads it, but under a cap of
    /// `cap` values too, and stopped at the lesser of the two, what's
    /// read past it not built: what's read and how many values reading it
    /// counted, or which cap it met, `limits.values` where they're equal.
    /// Built only if `make`, so that, without, it counts alone, nothing
    /// kept, and what's read is none.
    pub(crate) fn object_within(&self, limits: &Limits, cap: usize, make: bool) -> Within {
        let lesser = limits.values.min(cap);
        let met = || match limits.values <= cap {
            true => Within::Values,
            false => Within::Cap,
        };
        // What opens nothing is one value, read where it starts, as
        // [`object_with`](Operand::object_with) reads it.
        let mut tokens = tokenizer(self.bytes, 0);
        let Some(first) = tokens.next_significant() else {
            return Within::Read(None, 0);
        };
        let opens = match first.kind {
            Kind::ArrayOpen | Kind::DictOpen => true,
            Kind::LiteralString | Kind::HexString if !first.closed => {
                return Within::Read(None, 0);
            }
            Kind::Integer | Kind::Real | Kind::Name | Kind::LiteralString | Kind::HexString => {
                false
            }
            Kind::Keyword if matches!(first.bytes, b"true" | b"false" | b"null") => false,
            _ => return Within::Read(None, 0),
        };
        if !opens {
            return match lesser {
                0 => met(),
                _ => Within::Read(make.then(|| leaf(first)).flatten(), 1),
            };
        }
        let mut told = ();
        let mut read = Reading::new(self, lesser, &mut told, make);
        let walked = walk_from(
            self.bytes,
            first,
            tokens,
            TOP,
            limits.depth,
            false,
            &mut read,
        );
        if read.past {
            return met();
        }
        let counted = lesser - read.values;
        match walked {
            Ok(_) => Within::Read(read.read.flatten(), counted),
            Err(_) => Within::Read(None, counted),
        }
    }

    /// [`Operand::object_with`] by the walk, whatever it opens: what the
    /// tests and the fuzz body hold its reading of what opens nothing to.
    #[cfg(any(test, fuzzing))]
    pub(crate) fn walked(
        &self,
        limits: &Limits,
        sink: &mut (impl Sink<'a> + ?Sized),
    ) -> Option<Object> {
        let mut read = Reading::new(self, limits.values, sink, true);
        walk(self.bytes, 0, TOP, limits.depth, false, &mut read)?.ok()?;
        read.read?
    }

    /// What's inside it, told to `sink` as
    /// [`object_with`](Operand::object_with) tells it, with no object made:
    /// the same faults, in the same order, at the same places, under the
    /// same caps, for a reader that wants the faults alone. Nothing is
    /// read into an object, so a string's bytes aren't read, nor a name's
    /// escapes decoded, and nothing is kept.
    ///
    /// ```
    /// use pdf_content_streams::fault::{Fault, Sink};
    /// use pdf_content_streams::{Limits, ResCx, operations};
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
    /// let content = b"[(a) 1.2.3 (b)] TJ";
    /// let res = ResCx::none();
    /// let limits = Limits::DEFAULT;
    /// let tj = operations(content, &res, &limits).next().unwrap().unwrap();
    /// let (mut told, mut read) = (Collected::default(), Collected::default());
    /// tj.arg(0).unwrap().faults_with(&limits, &mut told);
    /// tj.arg(0).unwrap().object_with(&limits, &mut read);
    /// assert_eq!(told.0, [(5..10, Fault::Number { written: b"1.2.3" })]);
    /// assert_eq!(told.0, read.0);
    /// ```
    pub fn faults_with(&self, limits: &Limits, sink: &mut (impl Sink<'a> + ?Sized)) {
        // What doesn't want faults is told none.
        if !sink.wants() {
            return;
        }
        let mut read = Reading::new(self, limits.values, sink, false);
        walk(self.bytes, 0, TOP, limits.depth, false, &mut read);
    }

    /// An array's elements, as written, each where it is, in order, as
    /// PDFium reads them: each a number, a name, a string, a dictionary
    /// (to where PDFium ends it), `true`, `false` or `null`, or what's no
    /// object (a closing delimiter with nothing to close, a word that's no
    /// number, a `[` that opens no array, as PDFium reads an array inside
    /// an array, outside any dictionary); none when it isn't an array.
    /// Nothing is gathered: each is found as it's asked for, where an
    /// array's [`object`](Operand::object) finds it. An array of PDFium's
    /// ends at the first `]` read for an element, so `[[1] 2]` is `[[1]`,
    /// its elements `[` and `1`.
    pub const fn elements(&self) -> Elements<'a> {
        let array = match self.bytes {
            [b'[', ..] => self.bytes,
            _ => &[],
        };
        Elements {
            array,
            base: self.at,
            at: 1,
        }
    }

    /// Whether it's a number as PDFium tells one: a word of digits, signs
    /// and points only. It may be none as ISO 32000-2 writes a number, as
    /// `--5` isn't, which PDFium reads as 0 ([`number`](Operand::number),
    /// [`object`]).
    #[must_use]
    pub fn is_number(&self) -> bool {
        !self.bytes.is_empty() && is_number(self.bytes)
    }

    /// Whether it's a name: it starts with `/`.
    #[must_use]
    pub fn is_name(&self) -> bool {
        self.bytes.first() == Some(&b'/')
    }

    /// Whether it's a string: it starts with `(`, or with a `<` that
    /// isn't `<<`.
    #[must_use]
    pub fn is_string(&self) -> bool {
        match self.bytes {
            [b'(', ..] => true,
            [b'<', rest @ ..] => rest.first() != Some(&b'<'),
            _ => false,
        }
    }

    /// Whether it's an array: it starts with `[`.
    #[must_use]
    pub fn is_array(&self) -> bool {
        self.bytes.first() == Some(&b'[')
    }

    /// Whether it's a dictionary: it starts with `<<`. It may still read as
    /// none, where PDFium reads it so ([`object`]), such as `<< 1`, read to
    /// a key that's no name.
    #[must_use]
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
#[must_use = "an iterator is lazy: it gives nothing until asked"]
#[derive(Debug, Clone)]
pub struct Elements<'a> {
    /// The array, from its `[`.
    array: &'a [u8],
    /// Where `array` starts in the content.
    base: usize,
    /// Where the next element is looked for in `array`.
    at: usize,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Operand<'a>;

    fn next(&mut self) -> Option<Operand<'a>> {
        // An element is read as the array read it.
        let r = element(self.array, &mut self.at)?.at;
        Some(Operand {
            at: self.base + r.start,
            bytes: &self.array[r],
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
#[must_use]
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

/// An operand as a PDF object, read as PDFium's content parser reads it: a
/// number word as [`Operand::number`] reads it, by its first 255 bytes,
/// held as pdf-objects holds a number it reads by PDFium's rules, at 64
/// bits (the integer PDFium holds, in 32 bits, unsigned without a sign and
/// signed with one, 0 past them; or, with a point, the real, the decimal it
/// starts with); a name's escapes decoded and a string's escapes or hex
/// digits read ([`name`]; a literal string's unescaped CR or CR LF as
/// written, where ISO 32000-2 reads LF, and a string's first 32,767 bytes
/// read, the most that parser keeps); and an array's or a dictionary's
/// elements each read so. So `--5` is the integer 0, `1.2.3` the real 1.2
/// and `4294967296` the integer 0, where ISO 32000-2 (7.3.3) reads the
/// first two as no number and the last as written: where ISO reads what's
/// inside an operand otherwise, [`Operand::object_with`] tells it, and a
/// reader that wants ISO's reading reads an operand's bytes
/// ([`Operand::bytes`]) with pdf-objects' `Parser`, by ISO's rules. None
/// for what isn't an object, such as a stray closing delimiter or a word
/// that's no number, and for one that doesn't end, nests deeper than
/// [`Limits::depth`] or PDFium reads, or holds more values than
/// [`Limits::values`]. Arrays and dictionaries are read as PDFium's
/// `ReadNextObject` reads them, the first object in the bytes: in an
/// array, outside any dictionary, a `[` opens no array, and the first `]`
/// ends it, so `[[1] 2]` is the array of 1; a word that's no object in an
/// array is dropped; and a dictionary with a key that's no name, or a
/// value that's no object, is none, so `<< /A 1 2 >>` is none, and
/// `[<< 1 >> 2]` the array of 2. A dictionary keeps every entry, a key
/// written twice too, as pdf-objects' [`Dict`] does, so a lookup takes
/// its last. A string keeps its [`Form`]. Nothing read has a place
/// ([`Object::at`] is none): an operand's place is in the content, where
/// its token's range is. It's read in one pass, what's open kept on the
/// heap, so a deep operand costs what a flat one of its size does.
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
/// assert_eq!(object(b"--5", &limits), Some(Object::from(0)));
/// let big = object(b"4294967296", &limits);
/// assert_eq!(big.as_ref().and_then(Object::as_integer), Some(0));
/// assert_eq!(object(b"Tj", &limits), None);
/// ```
#[must_use]
pub fn object(operand: &[u8], limits: &Limits) -> Option<Object> {
    Operand {
        bytes: operand,
        at: 0,
    }
    .object_with(limits, &mut ())
}

/// What reading an operand as an object under a cap of the caller's gives
/// ([`Operand::object_within`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Within {
    /// Read under both caps: what's read, an object or none, and how many
    /// values reading it counted.
    Read(Option<Object>, usize),
    /// Past [`Limits::values`] first, or with the cap: none, as
    /// [`Operand::object`] reads it.
    Values,
    /// Past the cap first: read no further.
    Cap,
}

#[cfg(test)]
thread_local! {
    /// How many values readings of operands as objects have built on this
    /// thread: what the tests hold a read under a cap to, as building none
    /// past it.
    pub(crate) static BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// An operand being read as an object, built from a [`walk`]'s steps:
/// where it is in the content, how many more values it may hold, the sink
/// told where ISO 32000-2 would read what's inside it otherwise, what's
/// open, and what's read, if it's made.
struct Reading<'a, 's, S: ?Sized> {
    /// The operand's bytes.
    s: &'a [u8],
    /// Where they start in the content.
    base: usize,
    /// The values, objects or not, it may read yet.
    values: usize,
    /// Whether it's read more values than it may: kept apart from
    /// `values`, which a caller may set to any number, `usize::MAX` too.
    past: bool,
    sink: &'s mut S,
    /// Each array or dictionary open, the outermost first.
    open: Vec<Partial>,
    /// What's read, once it is: an object, or None for what's none. None
    /// throughout once it's read more values than it may, and where it
    /// isn't made.
    read: Option<Option<Object>>,
    /// Whether what's read is made, or only told where ISO 32000-2 reads
    /// it otherwise: what's open, the values counted and a key's place are
    /// kept either way, as what's told depends on them, and nothing else.
    make: bool,
}

impl<'a, 's, S: Sink<'a> + ?Sized> Reading<'a, 's, S> {
    /// None read yet of `operand`, under a cap of `values`, told to
    /// `sink`, made if `make`.
    fn new(operand: &Operand<'a>, values: usize, sink: &'s mut S, make: bool) -> Self {
        Reading {
            s: operand.bytes,
            base: operand.at,
            values,
            past: false,
            sink,
            open: Vec::new(),
            read: None,
            make,
        }
    }
}

/// An array or a dictionary being read: what it holds so far, and a
/// dictionary's key read, with where it is, waiting for its value.
enum Partial {
    Array(Vec<Object>),
    Dict(Dict, Option<(Vec<u8>, Range<usize>)>),
}

impl<'a, S: Sink<'a> + ?Sized> Visit<'a> for Reading<'a, '_, S> {
    fn visit(&mut self, event: Event<'a>) {
        if self.past {
            return;
        }
        if !matches!(event, Event::Close(_) | Event::Unmade) {
            match self.values.checked_sub(1) {
                Some(left) => self.values = left,
                // Past the values it may read: none read, and nothing
                // told after.
                None => {
                    self.past = true;
                    self.read = None;
                    return;
                }
            }
            #[cfg(test)]
            if self.make {
                BUILT.with(|built| built.set(built.get() + 1));
            }
        }
        match event {
            Event::Leaf(token) => {
                self.inside(token);
                let value = if self.make { leaf(token) } else { None };
                self.put(value);
            }
            Event::Word(token) => {
                // A key with no value: `>>` read for it.
                let key = match self.open.last() {
                    Some(Partial::Dict(_, Some((_, key)))) if token.kind == Kind::DictClose => {
                        Some(key.clone())
                    }
                    _ => None,
                };
                match key {
                    Some(key) => self.tell(key, |written| Fault::NoValue { written }),
                    None => self.inside(token),
                }
                self.put(None);
            }
            Event::Open(token) => self.open.push(match token.kind {
                Kind::DictOpen => Partial::Dict(Dict::new(), None),
                _ => Partial::Array(Vec::new()),
            }),
            Event::Key(token) => {
                self.inside(token);
                let key = match self.make {
                    true => name(token.bytes).map_or_else(Vec::new, Cow::into_owned),
                    false => Vec::new(),
                };
                let place = token.range();
                if let Some(Partial::Dict(_, pending)) = self.open.last_mut() {
                    *pending = Some((key, place));
                }
            }
            Event::NotKey(token) => {
                if self.sink.wants() {
                    let place = self.base + token.start..self.base + token.end();
                    match fault::token(token.bytes, place.start, self.sink) {
                        Read::Object => self.tell(token.range(), |written| Fault::Key { written }),
                        Read::Keyword => {
                            self.tell(token.range(), |written| Fault::Keyword { written });
                        }
                        Read::Told => {}
                    }
                }
            }
            Event::Close(_) => {
                let value = match self.open.pop() {
                    Some(Partial::Array(elements)) => Value::Array(elements),
                    Some(Partial::Dict(dict, _)) => Value::Dict(dict),
                    None => return,
                };
                let value = self.make.then(|| Object::new(value));
                self.put(value);
            }
            Event::Unmade => {
                self.open.pop();
                self.put(None);
            }
        }
    }
}

impl<'a, S: Sink<'a> + ?Sized> Reading<'a, '_, S> {
    /// `value` read where the innermost array or dictionary open wants
    /// one, or for the operand: an element kept, a value kept under its
    /// key, which is then read, or the operand read. What's none is
    /// dropped.
    fn put(&mut self, value: Option<Object>) {
        match self.open.last_mut() {
            None => self.read = Some(value),
            Some(Partial::Array(elements)) => elements.extend(value),
            Some(Partial::Dict(dict, pending)) => {
                if let (Some((key, _)), Some(value)) = (pending.take(), value) {
                    dict.push(key, value);
                }
            }
        }
    }

    /// A token read inside an array or a dictionary, told to the sink, if
    /// it wants: its own faults, and a word that's no object.
    fn inside(&mut self, token: Token<'a>) {
        if self.open.is_empty() || !self.sink.wants() {
            return;
        }
        let at = self.base + token.start;
        if fault::token(token.bytes, at, self.sink) == Read::Keyword {
            self.tell(token.range(), |written| Fault::Keyword { written });
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

/// An object that holds none, as a token by PDFium's rules: a string's
/// bytes read in its form, a name decoded, a number word, `true`, `false`
/// or `null` ([`word`]).
fn leaf(token: Token) -> Option<Object> {
    let value = match token.kind {
        Kind::LiteralString => Value::String(Str::Bytes(literal(token.bytes)), Some(Form::Literal)),
        Kind::HexString => Value::String(Str::Bytes(hex(token.bytes)), Some(Form::Hex)),
        Kind::Name => Value::Name(name(token.bytes)?.into_owned()),
        _ => word(token.bytes)?,
    };
    Some(Object::new(value))
}

/// A word as an object: a number, `true`, `false` or `null`; none for any
/// other. A number word (digits, signs and points) as PDFium's content
/// parser reads it, by its first [`WORD`] bytes, held at 64 bits: the
/// integer PDFium holds, or, with a point among the bytes kept, the real,
/// the decimal they start with ([`pdfium::real`]). 255 bytes hold no
/// decimal past an `f64`'s range, so the real is finite.
fn word(word: &[u8]) -> Option<Value> {
    Some(match word {
        b"true" => Value::Bool(true),
        b"false" => Value::Bool(false),
        b"null" => Value::Null,
        n if is_number(n) => {
            let n = &n[..n.len().min(WORD)];
            match pdfium::number(n) {
                Number::Unsigned(n) => Value::Integer(n.into()),
                Number::Signed(n) => Value::Integer(n.into()),
                Number::Real(_) => Value::Real(pdfium::real(n)),
            }
        }
        _ => return None,
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
