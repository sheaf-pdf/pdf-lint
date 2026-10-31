//! What the fuzz target checks, which unit tests check too: the promises
//! the parser makes, on any input, by ISO 32000-2's rules and by PDFium's
//! and the two side by side, and the writer's on what it read, written
//! plainly, through a toy encryptor ([`Toy`]) and through a renumbering.
//! It's compiled only for tests and under cargo-fuzz's `--cfg fuzzing`.

use std::borrow::Cow;
use std::cell::RefCell;

use pdf_tokenizer::decode::{self, pdfium};
use pdf_tokenizer::{Kind, Pdfium, Rules, Token, Tokenizer};

use crate::object::{Data, Dict, Form, Object, ObjectId, Span, Str, Stream, Value};
use crate::parse::{Body, Error, Fault, Limits, Parser, Sink};
use crate::serialize::{self, Encrypt, Renumber};

/// What a sink was told: each fault, where it is and what it says.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Kept {
    /// Each fault's place and text, in the order said.
    pub faults: Vec<(Span, String)>,
}

impl Sink<'_> for Kept {
    fn fault(&mut self, at: Span, fault: Fault<'_>) {
        self.faults.push((at, fault.to_string()));
    }
}

/// How the input's first bytes say to read the rest.
#[derive(Debug, Clone, Copy)]
struct How {
    offset: usize,
    /// What the offset is read from, which the renumbering what's read is
    /// written through is taken from too ([`renumbering`]).
    seed: u16,
    stream: Option<ObjectId>,
    ciphertext: bool,
    /// An indirect object, else a value.
    indirect: bool,
    /// A stream's `/Length`.
    length: Length,
    /// Whether to give a stream data that isn't its own too.
    astray: bool,
    /// The limits read under: the default, or small ones.
    limits: Limits,
    /// A window to read the input in too.
    window: Option<Window>,
    /// The rules read by.
    rules: Rules,
}

/// A window of a file to read the input in, as a reader by position
/// does: cut at `cut`, from byte `base` of the file.
#[derive(Debug, Clone, Copy)]
struct Window {
    cut: usize,
    base: usize,
}

/// A stream's `/Length`: as given, or the scanned extent's and this more
/// or less.
#[derive(Debug, Clone, Copy)]
enum Length {
    Given(u16),
    Scanned(i16),
}

/// The fuzz target's body: a value, or an indirect object, read from the
/// input after its first five bytes, as they choose: from an offset in it
/// (past its end, now and then), in an object stream's data or the
/// file's, its strings ciphertext or not; a stream's data by a length they
/// give, when it holds, else up to `endstream`; under the default limits,
/// or small ones the two bytes after them give (nesting below 8, values
/// below 64). Read twice, and through `()`, it's the same each time; every
/// span and fault is inside the input and inside what holds it; strings
/// are ciphertext exactly when asked, and each has the form its first byte
/// writes; a value, a header or a body that fails leaves the parser where
/// it was; and under small limits, a read that reaches them fails saying
/// which, where a value starts that the read without them takes past
/// them, or fails after, and one that doesn't is the read without them.
/// Its references, walked with a stack of their own
/// ([`Object::references`]), are a recursive walk's, in the same order.
/// What's read, the writer writes, and it reads back as the same value and
/// writes as the same bytes again ([`round_trip`]), but for a ciphertext
/// read's object holding a string: the writer refuses it whole, writing
/// nothing, and writes it with its strings taken as their bytes, which
/// read back as plain. An indirect object is written through a toy
/// encryptor too ([`toyed`]), which is given each string and stream's data
/// once, as the object's, and what it wrote, undone, is what was written
/// plainly. Written through a renumbering the offset's two bytes give
/// ([`renumbering`]), each reference named anew or null, it reads back as
/// the value renumbered in memory, which keeps its places where the
/// renumbering changes nothing ([`renumbered`]). With flag 0x80 and two
/// bytes after the five (and the limits' two), it's read as a window of a
/// file too, from byte 0 or 1,000,000, cut where those two bytes choose
/// ([`windowed`]): what isn't [`Error::Short`] reads as the whole input
/// does.
///
/// All of it by ISO 32000-2's rules and again by PDFium's
/// ([`Parser::rules`]), and the two side by side ([`side_by_side`]): read
/// alike up to the first token the two rules cut otherwise, or PDFium's
/// reads otherwise, from where the read starts; and each fault PDFium's
/// rules alone tell is told of a token ISO 32000-2 reads otherwise.
/// Panics on a broken promise.
pub fn parse(data: &[u8]) {
    let (how, input) = chosen(data);
    let iso = promised(input, how);
    let pdfium = How {
        rules: Rules::Pdfium,
        ..how
    };
    side_by_side(input, how, &iso, &promised(input, pdfium));
}

/// What reading `input` as `how` says: the object or the error, and where
/// the parser ended.
type Read = (Result<Object, Error>, usize);

/// [`parse`]'s promises, `input` read as `how` says: what it read and
/// what its sink was told.
fn promised(input: &[u8], how: How) -> (Read, Kept) {
    let mut kept = Kept::default();
    let first = read(input, how, &mut kept);
    let mut again = Kept::default();
    assert_eq!(first, read(input, how, &mut again), "the same twice");
    assert_eq!(kept, again, "said the same twice");
    assert_eq!(first, read(input, how, &mut ()), "the same through ()");
    if how.limits != Limits::DEFAULT {
        limited(input, how, &first);
    }
    if let Some(window) = how.window {
        windowed(input, how, window, &first, &kept);
    }
    let (result, offset) = first.clone();
    assert!(offset <= input.len().max(how.offset), "{offset}");
    let inside = |at: Span| at.stream == how.stream && at.start <= at.end && at.end <= input.len();
    for &(at, _) in &kept.faults {
        assert!(inside(at), "a fault in the input: {at:?}");
    }
    match result {
        Ok(object) => {
            let whole = object.at.expect("placed");
            for &(at, _) in &kept.faults {
                assert!(within(at, whole), "a fault in its object: {at:?}");
            }
            placed(input, &object, whole, how);
            let mut found = Vec::new();
            references(&object, &mut found);
            assert!(
                object.references().eq(found),
                "its references are a recursive walk's"
            );
            rewrite(input, how, &object);
        }
        Err(error) => {
            assert!(!error.to_string().is_empty());
            if !how.indirect {
                assert_eq!(
                    offset, how.offset,
                    "a value that fails leaves the parser where it was"
                );
            }
            match error {
                Error::Header { at } => {
                    assert!(how.indirect);
                    assert_eq!(at, how.offset, "the header looked for where it was given");
                    assert_eq!(
                        offset, how.offset,
                        "a header that fails leaves the parser where it was"
                    );
                }
                // The stream's own, after its body.
                Error::Endstream { .. } | Error::Extent { .. } => {}
                _ if how.indirect => {
                    let mut parser = Parser::at(input, how.offset).rules(how.rules);
                    assert!(parser.header().is_ok());
                    assert_eq!(
                        offset,
                        parser.offset(),
                        "a body that fails leaves the parser after the header"
                    );
                }
                _ => {}
            }
        }
    }
    (first, kept)
}

/// Checks `iso` and `pdfium`, what reading `input` as `how` gave by ISO
/// 32000-2's rules and by PDFium's: alike where their tokens and what the
/// parser reads of them are. Where the two first part, from where the read
/// starts ([`parting`]), the input is cut, and read so, in a window that
/// isn't the file's last, the two rules read it alike: the same object or
/// error, the parser left at the same place, the same faults told. Where
/// they never part, the whole reads are alike. The parser starts a token
/// where a stream's data ends too, wherever that is, so the two are
/// compared from every offset, not along one walk of the tokens. And read
/// by PDFium's rules,
/// each fault they alone tell, [`Fault::Number`], [`Fault::OutOfRange`],
/// [`Fault::HexString`], [`Fault::StringCr`] and [`Fault::Long`], is told
/// at a token they read, which ISO 32000-2's rules cut or read otherwise.
fn side_by_side(input: &[u8], how: How, iso: &(Read, Kept), pdfium: &(Read, Kept)) {
    let from = how.offset.min(input.len());
    let at = parting(input, from);
    if at == input.len() {
        assert_eq!(iso, pdfium, "read alike where the rules read alike");
    } else {
        let cut = |rules| {
            let mut kept = Kept::default();
            let read = read_in(&input[..at], 0, false, How { rules, ..how }, &mut kept);
            (read, kept)
        };
        assert_eq!(
            cut(Rules::Iso),
            cut(Rules::Pdfium),
            "read alike before the rules part, at {at}"
        );
    }
    let mut own = Own::default();
    let pdfium = How {
        rules: Rules::Pdfium,
        ..how
    };
    let _ = read(input, pdfium, &mut own);
    for at in own.0 {
        let token = Tokenizer::at(input, at.start).with_rules(Pdfium).next();
        let token = token.filter(|t| t.end() == at.end).expect("a token");
        let iso = Tokenizer::at(input, at.start).next();
        assert!(
            iso != Some(token) || read_otherwise(&token),
            "a fault at {at:?}, which ISO 32000-2 reads alike"
        );
    }
}

/// Where `input` read by ISO 32000-2's rules and by PDFium's, from
/// `from`, first part: the first offset whose token they cut otherwise, or
/// the parser reads otherwise by PDFium's rules ([`read_otherwise`]), or
/// the start of the word it's in, as a word's first bytes may be a number
/// by one rule and not the other (`-.` of `-.4294967296`); the input's end
/// when none does. Before it, a token from any offset is the same by both
/// rules, read alike, and so is one cut there.
fn parting(input: &[u8], from: usize) -> usize {
    let parts = |at: usize| {
        let pdfium = Tokenizer::at(input, at).with_rules(Pdfium).next();
        pdfium != Tokenizer::at(input, at).next() || pdfium.is_some_and(|t| read_otherwise(&t))
    };
    let Some(mut at) = (from..input.len()).find(|&at| parts(at)) else {
        return input.len();
    };
    while at > from && Rules::Pdfium.is_regular(input[at - 1]) {
        at -= 1;
    }
    at
}

/// Whether the parser reads `token`, a token both rules cut alike,
/// otherwise by PDFium's rules than by ISO 32000-2's: written out, a number
/// longer than 256 bytes, or one ISO reads as a value PDFium doesn't (an
/// integer outside PDFium's unsigned or signed 32 bits); a name longer
/// than 256 bytes, or whose bytes PDFium reads otherwise; a string whose
/// bytes it does.
fn read_otherwise(token: &Token) -> bool {
    let bytes = token.bytes;
    let long = bytes.len() > 256;
    match token.kind {
        Kind::Integer => {
            let pdfium = match pdfium::number(bytes) {
                pdfium::Number::Unsigned(n) => Some(i64::from(n)),
                pdfium::Number::Signed(n) => Some(i64::from(n)),
                pdfium::Number::Real(_) => None,
            };
            long || decode::integer(bytes).ok() != pdfium
        }
        Kind::Real => long || decode::number(bytes).ok() != Some(pdfium::real(bytes)),
        Kind::Name => long || !pdfium::name(bytes).eq(decode::name(bytes).bytes()),
        Kind::String { .. } => !pdfium::literal(bytes).eq(decode::literal(bytes).bytes()),
        Kind::HexString { .. } => !pdfium::hex(bytes).eq(decode::hex(bytes).bytes()),
        _ => false,
    }
}

/// Where each fault PDFium's rules alone tell was told.
#[derive(Default)]
struct Own(Vec<Span>);

impl Sink<'_> for Own {
    fn fault(&mut self, at: Span, fault: Fault<'_>) {
        if matches!(
            fault,
            Fault::Number { .. }
                | Fault::OutOfRange { .. }
                | Fault::HexString { .. }
                | Fault::StringCr { .. }
                | Fault::Long { .. }
        ) {
            self.0.push(at);
        }
    }
}

/// The first token of `input` from `at`, by `rules`, that isn't
/// white-space or a comment.
fn significant(input: &[u8], at: usize, rules: Rules) -> Option<Token<'_>> {
    (Tokenizer::at(input, at).with_rules(rules))
        .find(|t| !matches!(t.kind, Kind::Space | Kind::Comment))
}

/// How `data`'s first five bytes say to read the rest, and the rest: with
/// flag 0x20 and two bytes after the five, small limits, from those two;
/// with flag 0x80 and two bytes after those, a window too, cut no nearer
/// than the offset read from, from byte 0 or 1,000,000.
fn chosen(data: &[u8]) -> (How, &[u8]) {
    let [a, b, flags, c, d, rest @ ..] = data else {
        let how = How {
            offset: 0,
            seed: 0,
            stream: None,
            ciphertext: false,
            indirect: false,
            length: Length::Given(0),
            astray: false,
            limits: Limits::DEFAULT,
            window: None,
            rules: Rules::Iso,
        };
        return (how, data);
    };
    let (limits, rest) = match rest {
        [nesting, values, rest @ ..] if flags & 0x20 != 0 => {
            let limits = Limits {
                nesting: usize::from(nesting % 8),
                values: usize::from(values % 64),
            };
            (limits, rest)
        }
        _ => (Limits::DEFAULT, rest),
    };
    let (window, input) = match rest {
        [e, f, input @ ..] if flags & 0x80 != 0 => (Some(u16::from_le_bytes([*e, *f])), input),
        _ => (None, rest),
    };
    let seed = u16::from_le_bytes([*a, *b]);
    let offset = match flags & 0x40 {
        0 => usize::from(seed) % (input.len() + 1),
        _ => input.len() + usize::from(seed),
    };
    let window = window.map(|seed| {
        let seed = usize::from(seed);
        let from = offset.min(input.len());
        Window {
            cut: from + (seed >> 1) % (input.len() - from + 1),
            base: (seed & 1) * 1_000_000,
        }
    });
    let how = How {
        offset,
        seed,
        stream: (flags & 1 != 0).then_some((u32::from(*c), 0)),
        ciphertext: flags & 2 != 0,
        indirect: flags & 4 != 0,
        length: match flags & 8 {
            0 => Length::Given(u16::from_le_bytes([*c, *d])),
            _ => Length::Scanned(i16::from_le_bytes([*c, *d])),
        },
        astray: flags & 0x10 != 0,
        limits,
        window,
        rules: Rules::Iso,
    };
    (how, input)
}

/// Checks `small`, what reading `input` as `how` said under its small
/// limits, against what the default limits read: a read that reached them
/// fails saying which, where a value starts, and the default's object
/// holds a value past that limit, or the default's read fails, after; and
/// a read that didn't reach them is the default's.
fn limited(input: &[u8], how: How, small: &(Result<Object, Error>, usize)) {
    let limits = how.limits;
    let how = How {
        limits: Limits::DEFAULT,
        ..how
    };
    let full = read(input, how, &mut ());
    let (at, past) = match small.0 {
        Err(Error::Nested { limit, at }) => {
            assert_eq!(limit, limits.nesting);
            (at, full.0.as_ref().map(|o| deepest(o) > limit))
        }
        Err(Error::Values { limit, at }) => {
            assert_eq!(limit, limits.values);
            (at, full.0.as_ref().map(|o| count(o) > limit))
        }
        _ => {
            assert_eq!(*small, full, "read as without limits it doesn't reach");
            return;
        }
    };
    assert!(
        past.unwrap_or(true),
        "read without limits, it doesn't pass them"
    );
    // Read from where it starts, what reached a limit is a value, or one
    // that fails after its first token: never the error of a token that
    // can't start one.
    assert!(at < input.len(), "{at}");
    let there = Parser::at(input, at).rules(how.rules).value(&mut ());
    let not_a_value = matches!(
        there,
        Err(Error::Token { at: a, .. } | Error::Keyword { at: a, .. } | Error::String { at: a })
            if a == at
    );
    assert!(
        !not_a_value,
        "a limit reached at {at}, not a value: {there:?}"
    );
}

/// How many values a parser reads for `o`: it, and each element, key and
/// dictionary value, at every depth; a stream's are its dictionary's.
fn count(o: &Object) -> usize {
    let entries = |d: &Dict| -> usize { d.entries().iter().map(|e| 1 + count(&e.value)).sum() };
    1 + match o.value() {
        Value::Array(a) => a.iter().map(count).sum(),
        Value::Dict(d) => entries(d),
        Value::Stream(s) => entries(&s.dict),
        _ => 0,
    }
}

/// Each reference in `o`, nested ones too, put in `found` in the order
/// written, walked recursively: an array's elements, a dictionary's every
/// entry, a stream's dictionary.
fn references(o: &Object, found: &mut Vec<ObjectId>) {
    let entries = |d: &Dict, found: &mut Vec<ObjectId>| {
        (d.entries().iter()).for_each(|e| references(&e.value, found));
    };
    match o.value() {
        Value::Ref(id) => found.push(*id),
        Value::Array(a) => a.iter().for_each(|o| references(o, found)),
        Value::Dict(d) => entries(d, found),
        Value::Stream(s) => entries(&s.dict, found),
        _ => {}
    }
}

/// How deep `o`'s deepest value is nested in it: 0 for `o` itself, one
/// more for each array or dictionary around it, a stream's dictionary as
/// one.
fn deepest(o: &Object) -> usize {
    let entries = |d: &Dict| d.entries().iter().map(|e| deepest(&e.value)).max();
    let inside = match o.value() {
        Value::Array(a) => a.iter().map(deepest).max(),
        Value::Dict(d) => entries(d),
        Value::Stream(s) => entries(&s.dict),
        _ => None,
    };
    inside.map_or(0, |d| d + 1)
}

/// What reading `input` as `how` says: the object or the error, and where
/// the parser ended.
fn read<'a>(input: &'a [u8], how: How, sink: &mut impl Sink<'a>) -> Read {
    read_in(input, 0, true, how, sink)
}

/// [`read`], `input` being a window of a file from byte `base`, which
/// ends the file when `last`.
fn read_in<'a>(
    input: &'a [u8],
    base: usize,
    last: bool,
    how: How,
    sink: &mut impl Sink<'a>,
) -> (Result<Object, Error>, usize) {
    let mut parser = (Parser::at(input, how.offset))
        .window(base, last)
        .ciphertext(how.ciphertext)
        .limits(how.limits)
        .rules(how.rules);
    if let Some(stream) = how.stream {
        parser = parser.in_stream(stream);
    }
    if !how.indirect {
        let value = parser.value(sink);
        return (value, parser.offset());
    }
    let object = parser.header().and_then(|id| match parser.body(id, sink)? {
        Body::Object(object) => Ok(object),
        Body::Stream(head) => {
            let start = head.start();
            let keyword = head.keyword();
            assert!(keyword.end <= start && start - base <= input.len());
            let scanned = head.scanned();
            assert_eq!((scanned.stream, scanned.start), (how.stream, start));
            let marker = &b"endstream"[..];
            let data = &input[scanned.start - base..scanned.end - base];
            assert!(!data.windows(marker.len()).any(|w| w == marker));
            let after = &input[scanned.end - base..];
            assert!(after.is_empty() || after.starts_with(marker));
            let length = match how.length {
                Length::Given(length) => usize::from(length),
                Length::Scanned(more) => {
                    (scanned.end - start).saturating_add_signed(isize::from(more))
                }
            };
            let declared = head.declared(length);
            if let Some(declared) = declared {
                assert_eq!(
                    (declared.start, declared.end - declared.start),
                    (start, length)
                );
                let after = significant(input, declared.end - base, how.rules);
                assert!(after.is_some_and(|t| t.bytes == marker));
            }
            let data = declared.unwrap_or(scanned);
            if how.astray {
                let astray = Span::new(how.stream, data.start + 1, data.end);
                let refused = parser.clone().stream(head.clone(), astray, &mut ());
                let short = !last && matches!(refused, Err(Error::Short { .. }));
                assert!(
                    short || matches!(refused, Err(Error::Extent { .. })),
                    "{refused:?}"
                );
            }
            let object = parser.stream(head, data, sink)?;
            let Value::Stream(stream) = object.value() else {
                panic!("a stream reads as one");
            };
            assert_eq!(stream.data, Data::At(data));
            Ok(object)
        }
    });
    (object, parser.offset())
}

/// Checks that `input`, read as `how` in a window of a file from byte
/// `window.base`, reads as the whole input did, `whole` saying `said`,
/// every place `window.base` on (value, spans, faults, error, and where
/// the parser ended): whole, as the file's last window; and cut at
/// `window.cut`, as a window that isn't, but where it's [`Error::Short`],
/// at the cut. Whether the cut read was short.
fn windowed(
    input: &[u8],
    how: How,
    window: Window,
    whole: &(Result<Object, Error>, usize),
    said: &Kept,
) -> bool {
    let Window { cut, base } = window;
    let want = (
        whole
            .0
            .clone()
            .map(|o| shifted(o, base))
            .map_err(|e| shifted_error(e, base)),
        whole.1 + base,
    );
    let said: Vec<(Span, String)> = (said.faults.iter())
        .map(|(at, said)| (shifted_span(*at, base), said.clone()))
        .collect();
    let mut kept = Kept::default();
    let last = read_in(input, base, true, how, &mut kept);
    assert_eq!(last, want, "the file's last window reads as the whole");
    assert_eq!(kept.faults, said, "said as the whole");
    let mut kept = Kept::default();
    let read = read_in(&input[..cut], base, false, how, &mut kept);
    if let Err(Error::Short { at }) = read.0 {
        assert_eq!(at, base + cut, "short where the window ends");
        return true;
    }
    assert_eq!(read, want, "cut at {cut}, it reads as the whole");
    assert_eq!(kept.faults, said, "cut at {cut}, said as the whole");
    false
}

/// Checks `input`'s every window ([`windowed`]): read from its start as a
/// value, and as an indirect object, a stream's data by a `/Length` of
/// `length` and by its scanned extent, in the file and in an object
/// stream's data, by ISO 32000-2's rules and by PDFium's; cut at every
/// byte, from bytes 0 and 1,000,000 of a file. How many of the cut windows
/// weren't [`Error::Short`].
pub fn every_window(input: &[u8], length: u16) -> usize {
    let how = How {
        offset: 0,
        seed: 0,
        stream: None,
        ciphertext: false,
        indirect: false,
        length: Length::Given(length),
        astray: true,
        limits: Limits::DEFAULT,
        window: None,
        rules: Rules::Iso,
    };
    let mut read = 0;
    let each = [Rules::Iso, Rules::Pdfium].map(|rules| [(rules, None), (rules, Some((7, 0)))]);
    for (rules, stream) in each.concat() {
        for (indirect, length) in [
            (false, how.length),
            (true, how.length),
            (true, Length::Scanned(0)),
        ] {
            let how = How {
                stream,
                indirect,
                length,
                rules,
                ..how
            };
            let mut said = Kept::default();
            let whole = self::read(input, how, &mut said);
            for cut in 0..=input.len() {
                for base in [0, 1_000_000] {
                    let short = windowed(input, how, Window { cut, base }, &whole, &said);
                    read += usize::from(!short);
                }
            }
        }
    }
    read
}

/// `o`, its places shifted `base` on.
fn shifted(mut o: Object, base: usize) -> Object {
    shift(&mut o, base);
    o
}

/// Shifts every place in `o` `base` on: its span, its values' and keys',
/// and a stream's data's, as a window there places them. It isn't an
/// edit, so it keeps them.
fn shift(o: &mut Object, base: usize) {
    o.at = o.at.map(|at| shifted_span(at, base));
    let entries = |d: &mut Dict| {
        for entry in d.entries_mut() {
            entry.key_at = entry.key_at.map(|at| shifted_span(at, base));
            shift(&mut entry.value, base);
        }
    };
    match o.as_written_mut() {
        Value::Array(a) => a.iter_mut().for_each(|e| shift(e, base)),
        Value::Dict(d) => entries(d),
        Value::Stream(s) => {
            if let Data::At(at) = &mut s.data {
                *at = shifted_span(*at, base);
            }
            entries(&mut s.dict);
        }
        _ => {}
    }
}

/// `at`, shifted `base` on.
fn shifted_span(at: Span, base: usize) -> Span {
    Span::new(at.stream, at.start + base, at.end + base)
}

/// `error`, each offset it names shifted `base` on.
fn shifted_error(error: Error, base: usize) -> Error {
    match error {
        Error::Nested { limit, at } => Error::Nested {
            limit,
            at: at + base,
        },
        Error::Values { limit, at } => Error::Values {
            limit,
            at: at + base,
        },
        Error::End => Error::End,
        Error::Reference {
            at,
            number,
            generation,
        } => Error::Reference {
            at: at + base,
            number,
            generation,
        },
        Error::String { at } => Error::String { at: at + base },
        Error::Array { at } => Error::Array { at: at + base },
        Error::Dict { at } => Error::Dict { at: at + base },
        Error::Key { at } => Error::Key { at: at + base },
        Error::Keyword { at, word } => Error::Keyword {
            at: at + base,
            word,
        },
        Error::Token { at, bytes } => Error::Token {
            at: at + base,
            bytes,
        },
        Error::Header { at } => Error::Header { at: at + base },
        Error::Endstream { id } => Error::Endstream { id },
        Error::Extent { id, start, end } => Error::Extent {
            id,
            start: start + base,
            end: end + base,
        },
        Error::Short { at } => Error::Short { at: at + base },
    }
}

/// Takes every place out of `o`, its values' spans and its keys'
/// ([`Object::unplace`]), making a stream's data first the bytes of `data`
/// it was read from, so that a value read compares with one built. Only an
/// indirect object is a stream a parser reads (ISO 32000-2, 7.3.8), so a
/// stream inside `o` is one built, whose data is its bytes already.
pub fn unplace(o: &mut Object, data: &[u8]) {
    if let Value::Stream(s) = o.as_written_mut() {
        if let Some(at) = s.data.at() {
            s.data = Data::Bytes(data[at.start..at.end].to_vec());
        }
    }
    o.unplace();
}

/// Checks that `object`, read from `input` as `how` says, is written, and
/// reads back the same way under the same limits ([`round_trip`]); and, as
/// an indirect object, through the toy encryptor ([`toyed`]); and written
/// through the renumbering the offset's seed gives ([`renumbering`]), it
/// reads back as renumbered in memory ([`renumbered`]). A ciphertext
/// read's object holding a string is refused whole, nothing written, then
/// written with its strings taken as their bytes, and read back as plain.
fn rewrite(input: &[u8], how: How, object: &Object) {
    let id = match how.indirect {
        true => Some(
            (Parser::at(input, how.offset).rules(how.rules))
                .header()
                .expect("its header"),
        ),
        false => None,
    };
    let mut object = object.clone();
    if how.ciphertext {
        let mut out = b"%PDF-".to_vec();
        let written = write(&mut out, &object, id, input);
        match plain(&mut object) {
            0 => assert!(written.is_ok(), "{written:?}"),
            _ => {
                assert_eq!(written, Err(serialize::Error::Encrypted { id }));
                assert_eq!(out, b"%PDF-", "nothing written");
            }
        }
    }
    let written = round_trip(&object, id, input, how.stream, how.limits);
    if let Some(id) = id {
        toyed(&object, id, input, how.limits, &written);
    }
    let to = renumbering(how.seed);
    renumbered(&object, id, input, how.stream, how.limits, &to, &written);
}

/// The renumbering the fuzz body writes what it read through, from `seed`:
/// each reference as it is, for 0; else each number `seed` on, wrapping
/// past the largest, the generation kept, but a number whose remainder by 7
/// is `seed`'s, which it names none of.
pub fn renumbering(seed: u16) -> impl Fn(ObjectId) -> Option<ObjectId> {
    move |(n, g)| match seed {
        0 => Some((n, g)),
        _ if n % 7 == u32::from(seed % 7) => None,
        _ => Some((n.wrapping_add(u32::from(seed)), g)),
    }
}

/// Checks that the writer writes `object`, as indirect object `id` when
/// there's one, else as a value, a stream's data from `data` when it's
/// where it was written there; that what it wrote reads back, read as a
/// parser under `limits` reads it, in object stream `stream`'s data when
/// there's one, as the same value as the writer writes it
/// ([`as_written`]), places aside, the parser saying only that a name
/// holds NUL, `#00`, for each name that does; and that the value read back
/// is written as the same bytes again. Panics when any of that fails, or
/// the writer refuses `object`; else gives what it wrote.
///
/// A stream's dictionary without a `/Length` is written with one, two
/// values more, one deeper, which the limits read back take in.
pub fn round_trip(
    object: &Object,
    id: Option<ObjectId>,
    data: &[u8],
    stream: Option<ObjectId>,
    limits: Limits,
) -> Vec<u8> {
    let mut written = Vec::new();
    if let Err(e) = write(&mut written, object, id, data) {
        panic!("the writer refused {object:?}: {e}");
    }
    let mut want = object.clone();
    unplace(&mut want, data);
    as_written(&mut want);
    read_back(&written, &want, object, id, stream, limits);
    written
}

/// Checks that `written`, what the writer wrote of `object`, as indirect
/// object `id` when there's one, else as a value, reads back, read as a
/// parser under `limits` reads it, in object stream `stream`'s data when
/// there's one, as `want`, its places taken out ([`unplace`]), the parser
/// saying only that a name holds NUL, `#00`, for each name that does; and
/// that the value read back is written as the same bytes again, as it is.
fn read_back(
    written: &[u8],
    want: &Object,
    object: &Object,
    id: Option<ObjectId>,
    stream: Option<ObjectId>,
    limits: Limits,
) {
    let mut parser = Parser::at(written, 0).limits(written_under(object, limits));
    if let Some(stream) = stream {
        parser = parser.in_stream(stream);
    }
    let text = String::from_utf8_lossy(written);
    let mut nul = Nul::default();
    // What's read back, and what follows it.
    let (back, after) = match id {
        Some(id) => {
            assert_eq!(parser.header(), Ok(id));
            let back = parser.body(id, &mut nul).and_then(|body| match body {
                Body::Object(object) => Ok(object),
                Body::Stream(head) => {
                    let length = head.dict().get(b"Length").and_then(Object::as_integer);
                    let length = usize::try_from(length.expect("an integer /Length")).unwrap();
                    let data = head.declared(length).expect("endstream after its /Length");
                    parser.stream(head, data, &mut nul)
                }
            });
            (back, &b"\nendobj\n"[..])
        }
        None => (parser.value(&mut nul), &b""[..]),
    };
    let mut back = back.unwrap_or_else(|e| panic!("{e}, reading back {text:?}"));
    assert!(written[parser.offset()..] == *after, "{text:?}");
    unplace(&mut back, written);
    assert_eq!(back, *want, "read back from {text:?}");
    assert_eq!(nul.0, nul_names(want), "a fault but for a name's NUL");
    let mut again = Vec::new();
    write(&mut again, &back, id, &[]).expect("what's read back writes");
    assert_eq!(again, written, "written again, the same bytes");
}

/// Checks that the writer writes `object` with each reference as `to`
/// names it ([`serialize::write_renumbered`]; as indirect object `id` when
/// there's one, [`serialize::indirect_renumbered`], a stream's data from
/// `data` when it's where it was written there,
/// [`serialize::indirect_stream_renumbered`]), and that what it wrote reads
/// back, as [`round_trip`] reads it, as `object` renumbered in memory
/// ([`Object::renumber`]), places aside; and as an indirect object,
/// through the toy encryptor too, undone, as what it wrote plainly
/// ([`toyed`]). Renumbered in memory, `object`'s references are its own as
/// `to` names them, those it names none of left out; where `to` changes
/// none, `object` is unchanged, every place kept, and what the writer
/// writes is `plain`, what it wrote of `object` with no renumbering; where
/// it changes one, `object`'s own place goes. Panics when any of that
/// fails, or the writer refuses `object`.
pub fn renumbered<N: Renumber>(
    object: &Object,
    id: Option<ObjectId>,
    data: &[u8],
    stream: Option<ObjectId>,
    limits: Limits,
    to: &N,
    plain: &[u8],
) {
    let mut moved = object.clone();
    let changed = moved.renumber(|id| to.renumber(id));
    let named = object.references().filter_map(|id| to.renumber(id));
    assert!(moved.references().eq(named), "its references, renumbered");
    match changed {
        false => assert_eq!(moved, *object, "nothing changed, every place kept"),
        true => assert_eq!(moved.at, None, "a reference changed, the place gone"),
    }
    let mut written = Vec::new();
    let at = object
        .as_stream()
        .and_then(|s| Some((&s.dict, s.data.at()?)));
    let refused = match (id, at) {
        (Some(id), Some((dict, at))) => {
            let data = &data[at.start..at.end];
            serialize::indirect_stream_renumbered(&mut written, id, dict, data, &(), to).map(drop)
        }
        (Some(id), None) => {
            serialize::indirect_renumbered(&mut written, id, object, &(), to).map(drop)
        }
        (None, _) => {
            serialize::write_renumbered(&mut written, object, to).map_err(serialize::Refused::Write)
        }
    };
    if let Err(e) = refused {
        panic!("the writer refused {object:?} renumbered: {e}");
    }
    if !changed {
        assert!(written == plain, "renumbering nothing, the plain bytes");
    }
    let mut want = moved.clone();
    unplace(&mut want, data);
    as_written(&mut want);
    read_back(&written, &want, object, id, stream, limits);
    if let Some(id) = id {
        let toy = Toy::default();
        let mut sealed = Vec::new();
        let refused = match at {
            Some((dict, at)) => {
                let data = &data[at.start..at.end];
                serialize::indirect_stream_renumbered(&mut sealed, id, dict, data, &toy, to)
            }
            None => serialize::indirect_renumbered(&mut sealed, id, object, &toy, to),
        };
        if let Err(e) = refused {
            panic!("the writer refused {object:?} renumbered through the toy: {e}");
        }
        undone(&toy, &sealed, &moved, id, limits, &written);
    }
}

/// The limits what the writer wrote of `object` reads back under, when
/// `object` was read under `limits`: a stream's dictionary without a
/// `/Length` is written with one, two values more, one deeper.
fn written_under(object: &Object, limits: Limits) -> Limits {
    let mut limits = limits;
    if object.as_stream().is_some_and(|s| !s.dict.has(b"Length")) {
        limits.values = limits.values.saturating_add(2);
        limits.nesting = limits.nesting.max(1);
    }
    limits
}

/// Checks that `object`, written as indirect object `id` through a toy
/// encryptor ([`Toy`]), a stream's data from `data` when it's where it was
/// written there, gives the toy each string and stream's data the writer
/// writes once, in the order written, as object `id`'s, and reads back,
/// under the limits `object` was read under, as what the toy undoes
/// ([`Toy::undo`]) into `plain`, what the writer wrote of `object` through
/// `()`. Panics when any of that fails.
pub fn toyed(object: &Object, id: ObjectId, data: &[u8], limits: Limits, plain: &[u8]) {
    let toy = Toy::default();
    let mut written = Vec::new();
    let refused = match object.as_stream() {
        Some(Stream {
            dict,
            data: Data::At(at),
        }) => {
            serialize::indirect_stream_with(&mut written, id, dict, &data[at.start..at.end], &toy)
        }
        _ => serialize::indirect_with(&mut written, id, object, &toy),
    };
    if let Err(e) = refused {
        panic!("the writer refused {object:?} through the toy: {e}");
    }
    undone(&toy, &written, object, id, limits, plain);
}

/// Checks that `written`, what the writer wrote of `object` as indirect
/// object `id` through `toy`, gave the toy each string and stream's data the
/// writer writes once, in the order written, as object `id`'s, and reads
/// back, under the limits `object` was read under, as what the toy undoes
/// ([`Toy::undo`]) into `plain`.
fn undone(toy: &Toy, written: &[u8], object: &Object, id: ObjectId, limits: Limits, plain: &[u8]) {
    let text = String::from_utf8_lossy(written);
    let mut parser = Parser::at(written, 0).limits(written_under(object, limits));
    assert_eq!(parser.header(), Ok(id));
    let back = parser.body(id, &mut ()).and_then(|body| match body {
        Body::Object(object) => Ok(object),
        Body::Stream(head) => {
            let length = head.dict().get(b"Length").and_then(Object::as_integer);
            let length = usize::try_from(length.expect("an integer /Length")).unwrap();
            let data = head.declared(length).expect("endstream after its /Length");
            parser.stream(head, data, &mut ())
        }
    });
    let mut back = back.unwrap_or_else(|e| panic!("{e}, reading back {text:?}"));
    assert!(written[parser.offset()..] == *b"\nendobj\n", "{text:?}");
    unplace(&mut back, written);
    toy.undo(id, &mut back);
    let mut again = Vec::new();
    write(&mut again, &back, Some(id), &[]).expect("what's undone writes");
    assert!(
        again == plain,
        "undone, {text:?} is what was written plainly"
    );
}

/// A toy encryptor, for the writer's tests and the fuzz body: each string
/// and stream's data written as the number (four bytes) and generation
/// (two) of the object it's in, big-endian, then its bytes reversed, which
/// [`Toy::undo`] undoes. An object whose number is a multiple of 5, or a
/// cross-reference stream (a stream of `/Type /XRef`), is written as it
/// is, and so is a dictionary's `/Contents` when it has a `/ByteRange`, as
/// a signature's.
/// It keeps what it's given, in order, and refuses a string or data that's
/// `refuse`.
#[derive(Debug, Default)]
pub struct Toy {
    /// Each string and stream's data it was given, in order: its object,
    /// its bytes, and whether it's a stream's data.
    pub seen: RefCell<Vec<(ObjectId, Vec<u8>, bool)>>,
    /// What it refuses to encrypt.
    pub refuse: Option<Vec<u8>>,
}

impl Encrypt for Toy {
    type Error = &'static str;

    fn clear(&self, id: ObjectId, dict: Option<&Dict>, stream: bool) -> bool {
        let xref = |d: &Dict| d.get(b"Type").and_then(Object::as_name) == Some(b"XRef");
        id.0 % 5 == 0 || (stream && dict.is_some_and(xref))
    }

    fn clear_key(&self, _: ObjectId, dict: &Dict) -> Option<&'static [u8]> {
        dict.has(b"ByteRange").then_some(b"Contents")
    }

    fn string<'t>(&self, id: ObjectId, text: &'t [u8]) -> Result<Cow<'t, [u8]>, &'static str> {
        self.sealed(id, text, false).map(Cow::Owned)
    }

    fn stream<'d>(
        &self,
        id: ObjectId,
        _: &Dict,
        data: &'d [u8],
    ) -> Result<Cow<'d, [u8]>, &'static str> {
        self.sealed(id, data, true).map(Cow::Owned)
    }
}

impl Toy {
    /// `bytes` of object `id`, as the toy writes them, kept as given.
    fn sealed(
        &self,
        (n, g): ObjectId,
        bytes: &[u8],
        stream: bool,
    ) -> Result<Vec<u8>, &'static str> {
        self.seen
            .borrow_mut()
            .push(((n, g), bytes.to_vec(), stream));
        if self.refuse.as_deref() == Some(bytes) {
            return Err("the toy refuses it");
        }
        let mut sealed = [&n.to_be_bytes()[..], &g.to_be_bytes()].concat();
        sealed.extend(bytes.iter().rev());
        Ok(sealed)
    }

    /// `o`, read back from what the writer wrote of it as object `id`
    /// through the toy, its places taken out ([`unplace`]), with each
    /// string and stream's data the toy wrote as it was given: walked as
    /// the writer walks it, each checked to be what the toy was given next,
    /// as object `id`'s, and every one it was given undone. Panics where
    /// it isn't so.
    pub fn undo(&self, id: ObjectId, o: &mut Object) {
        let seen = self.seen.borrow();
        let (dict, stream) = match o.value() {
            Value::Dict(d) => (Some(d), false),
            Value::Stream(s) => (Some(&s.dict), true),
            _ => (None, false),
        };
        let mut next = 0;
        if !self.clear(id, dict, stream) {
            self.opened(id, o, &seen, &mut next);
        }
        assert_eq!(next, seen.len(), "the toy was given each once");
    }

    /// `o` undone, as [`Toy::undo`] has it, from the `*next`th of `seen`.
    fn opened(
        &self,
        id: ObjectId,
        o: &mut Object,
        seen: &[(ObjectId, Vec<u8>, bool)],
        next: &mut usize,
    ) {
        let mut open = |sealed: &mut Vec<u8>, stream: bool| {
            let (n, g) = id;
            let tag = [&n.to_be_bytes()[..], &g.to_be_bytes()].concat();
            let rest = sealed
                .strip_prefix(&tag[..])
                .expect("written as its object's");
            let bytes: Vec<u8> = rest.iter().rev().copied().collect();
            assert_eq!(
                seen.get(*next),
                Some(&(id, bytes.clone(), stream)),
                "given in order"
            );
            *next += 1;
            *sealed = bytes;
        };
        match o.as_written_mut() {
            Value::String(Str::Bytes(sealed), _) => open(sealed, false),
            Value::Array(a) => a.iter_mut().for_each(|e| self.opened(id, e, seen, next)),
            Value::Dict(d) => self.entries(id, d, seen, next),
            Value::Stream(s) => {
                let Data::Bytes(sealed) = &mut s.data else {
                    panic!("a stream's data unplaced");
                };
                open(sealed, true);
                self.entries(id, &mut s.dict, seen, next);
            }
            _ => {}
        }
    }

    /// `d`'s entries undone, but the one it leaves in the clear.
    fn entries(
        &self,
        id: ObjectId,
        d: &mut Dict,
        seen: &[(ObjectId, Vec<u8>, bool)],
        next: &mut usize,
    ) {
        let clear = self.clear_key(id, d);
        for entry in d.entries_mut() {
            if clear != Some(&entry.key[..]) {
                self.opened(id, &mut entry.value, seen, next);
            }
        }
    }
}

/// `object` written to `out`, as [`round_trip`] writes it: a stream whose
/// data is where it was written in `data` through
/// [`serialize::indirect_stream`].
fn write(
    out: &mut Vec<u8>,
    object: &Object,
    id: Option<ObjectId>,
    data: &[u8],
) -> Result<(), serialize::Error> {
    match (id, object.as_stream()) {
        (
            Some(id),
            Some(Stream {
                dict,
                data: Data::At(at),
            }),
        ) => serialize::indirect_stream(out, id, dict, &data[at.start..at.end]).map(drop),
        (Some(id), _) => serialize::indirect(out, id, object).map(drop),
        (None, _) => serialize::write(out, object),
    }
}

/// What the parser reads back of what the writer wrote of `o`, its places
/// aside: a string with no form as a literal one; a real as the writer
/// clamps it, to Annex C's magnitudes, its largest 3.4e38, and 0 under its
/// smallest, 1.175e-38; and a stream's `/Length` its data's length, in
/// each of its entries, or after the others. A stream's data must be
/// [`Data::Bytes`], as [`unplace`] makes it.
fn as_written(o: &mut Object) {
    match o.value_mut() {
        Value::Real(x) => {
            if x.abs() > 3.4e38 {
                *x = 3.4e38f64.copysign(*x);
            } else if x.abs() < 1.175e-38 {
                *x = 0.0;
            }
        }
        Value::String(_, form) => {
            form.get_or_insert(Form::Literal);
        }
        Value::Array(a) => a.iter_mut().for_each(as_written),
        Value::Dict(d) => d.entries_mut().for_each(|e| as_written(&mut e.value)),
        Value::Stream(s) => {
            let length = s.data.bytes().expect("a stream's data in memory").len() as i64;
            let mut has = false;
            for entry in s.dict.entries_mut() {
                match entry.key == b"Length" {
                    true => {
                        entry.value = Object::from(length);
                        has = true;
                    }
                    false => as_written(&mut entry.value),
                }
            }
            if !has {
                s.dict.push("Length", length);
            }
        }
        _ => {}
    }
}

/// Takes the ciphertext in `o` as its bytes, each string keeping its form
/// and its place, as a reader that decrypted it would hold it: how many
/// strings `o` holds.
fn plain(o: &mut Object) -> usize {
    let entries = |d: &mut Dict| -> usize { d.entries_mut().map(|e| plain(&mut e.value)).sum() };
    match o.as_written_mut() {
        Value::String(s, _) => {
            if let Str::Encrypted(bytes) = s {
                *s = Str::Bytes(std::mem::take(bytes));
            }
            1
        }
        Value::Array(a) => a.iter_mut().map(plain).sum(),
        Value::Dict(d) => entries(d),
        Value::Stream(s) => entries(&mut s.dict),
        _ => 0,
    }
}

/// A sink for what the parser says of the writer's bytes, which can only
/// be that a name holds NUL, written `#00`: how many times.
#[derive(Default)]
struct Nul(usize);

impl Sink<'_> for Nul {
    fn fault(&mut self, at: Span, fault: Fault<'_>) {
        let nul = matches!(
            fault,
            Fault::Name {
                stray: false,
                nul: true,
                ..
            }
        );
        assert!(nul, "{fault} at {at:?}, in what the writer wrote");
        self.0 += 1;
    }
}

/// How many names in `o` hold NUL, its keys among them.
fn nul_names(o: &Object) -> usize {
    let entries = |d: &Dict| -> usize {
        (d.entries().iter())
            .map(|e| usize::from(e.key.contains(&0)) + nul_names(&e.value))
            .sum()
    };
    match o.value() {
        Value::Name(name) => usize::from(name.contains(&0)),
        Value::Array(a) => a.iter().map(nul_names).sum(),
        Value::Dict(d) => entries(d),
        Value::Stream(s) => entries(&s.dict),
        _ => 0,
    }
}

/// Whether `inner` is inside `outer`.
fn within(inner: Span, outer: Span) -> bool {
    inner.stream == outer.stream && outer.start <= inner.start && inner.end <= outer.end
}

/// Checks that `o` and every value in it are placed in `input` as `how`
/// reads it, each inside what holds it, `outer`, and each string is
/// ciphertext exactly when asked, in the form its first byte writes.
fn placed(input: &[u8], o: &Object, outer: Span, how: How) {
    let at = o.at.expect("every value placed");
    assert_eq!(at.stream, how.stream);
    assert!(at.start < at.end && at.end <= input.len(), "{at:?}");
    assert!(within(at, outer), "{at:?} in {outer:?}");
    let dict = match o.value() {
        Value::String(s, form) => {
            assert_eq!(matches!(s, Str::Encrypted(_)), how.ciphertext, "{s:?}");
            assert!(!matches!(s, Str::Text(_)));
            let written = match input[at.start] {
                b'<' => Form::Hex,
                b'(' => Form::Literal,
                other => panic!("a string whose first byte is {other:#04x}"),
            };
            assert_eq!(*form, Some(written), "{at:?}");
            None
        }
        Value::Array(a) => {
            a.iter().for_each(|e| placed(input, e, at, how));
            None
        }
        Value::Dict(d) => Some(d),
        Value::Stream(s) => {
            let data = s.data.at().expect("a stream's data placed");
            assert!(
                at.start < data.start && data.end < at.end,
                "{data:?} in {at:?}"
            );
            Some(&s.dict)
        }
        _ => None,
    };
    for entry in dict.into_iter().flat_map(Dict::entries) {
        let key = entry.key_at.expect("every key placed");
        assert!(within(key, at) && key.start < key.end);
        assert_eq!(input[key.start], b'/');
        let value = entry.value.at.expect("every value placed");
        assert!(key.end <= value.start);
        placed(input, &entry.value, at, how);
    }
}

#[cfg(test)]
mod tests {
    use super::{every_window, parse};

    /// Fragments of objects, where they begin and end.
    const FRAGMENTS: &[&[u8]] = &[
        b"1 0 obj",
        b"2 0 R",
        b"obj",
        b"endobj",
        b"stream\r\n",
        b"stream\n",
        b"stream\r",
        b"stream",
        b"endstream",
        b"<< /Length 3 >>\nstream\r\n",
        b"<<>>stream\n",
        b"endstream\nendobj",
        b"<<",
        b">>",
        b"[",
        b"]",
        b"(",
        b")",
        b"<",
        b">",
        b"<4142>",
        b"(a\\)b)",
        b"/",
        b"/Length",
        b"/A#4a",
        b"/N#00",
        b"/B#z",
        b"#",
        b" ",
        b"\r",
        b"\n",
        b"%",
        b"0",
        b"7",
        b"-",
        b".",
        b"R",
        b"true",
        b"null",
        b"stray",
        b"{",
        b"\x80",
        // Numbers too large for an integer and for an f64.
        b"99999999999999999999",
        &[b'9'; 320],
    ];

    /// Numbers below what each call is given, the same each run.
    fn randoms() -> impl FnMut(usize) -> usize {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        move |below: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % below as u64) as usize
        }
    }

    /// Inputs made of the fragments of objects, after five bytes that
    /// choose how they're read (and two that give small limits, and two
    /// that give a window, when they choose those), run through the fuzz
    /// target's body.
    #[test]
    fn random_fragments_keep_every_promise() {
        let mut random = randoms();
        for _ in 0..20_000 {
            let mut input: Vec<u8> = (0..5).map(|_| random(256) as u8).collect();
            // Mostly from the first byte, and an offset past the end only
            // now and then.
            if random(2) == 0 {
                input[..2].fill(0);
            }
            if random(8) != 0 {
                input[2] &= !0x40;
            }
            // A quarter under small limits, from the two bytes after the
            // five.
            if random(2) == 0 {
                input[2] &= !0x20;
            }
            if input[2] & 0x20 != 0 {
                input.extend_from_slice(&[random(256) as u8, random(256) as u8]);
            }
            // Half in a window too, from the two bytes after those.
            if input[2] & 0x80 != 0 {
                input.extend_from_slice(&[random(256) as u8, random(256) as u8]);
            }
            // Half the indirect objects begin as one does, and half of those
            // as a stream; half the lengths from the scanned extent are its.
            if input[2] & 4 != 0 && random(2) == 0 {
                input[..2].fill(0);
                input.extend_from_slice(b"1 0 obj ");
                if random(2) == 0 {
                    input.extend_from_slice(b"<< /Length 3 >>\nstream\r\n");
                }
            }
            if input[2] & 8 != 0 && random(2) == 0 {
                input[3..5].fill(0);
            }
            for _ in 0..random(40) + 2 {
                input.extend_from_slice(FRAGMENTS[random(FRAGMENTS.len())]);
            }
            parse(&input);
        }
    }

    /// Fragments where ISO 32000-2's rules and PDFium's part: number words
    /// only PDFium's read as numbers, integers outside its 32 bits, hex
    /// strings with bytes that are no hex digits, names' `#`, literal
    /// strings' CR, its white-space, and words past its 256 bytes (and,
    /// made as they're chosen, names).
    const PARTING: &[&[u8]] = &[
        b"--5",
        b"1.2.3",
        b"+",
        b"-",
        b".",
        b"-.",
        b"5-3",
        b"4294967296",
        b"4294967295",
        b"+2147483648",
        b"-2147483649",
        b"<41G42>",
        b"<4%1>",
        b"<4",
        b"(a\rb)",
        b"(a\r\nb)",
        b"(a\\\rb)",
        b"/#5Gx",
        b"/F#3",
        b"\x80",
        b"\xff",
        b"3 --1 R",
        b"--3 0 R",
        b"4294967299 0 R",
        b"3.9 0 R",
        &[b'0'; 256],
        &[b'7'; 257],
        b"endstream\x80",
    ];

    /// Inputs made of the fragments of objects and those where the rules
    /// part, after the five bytes that choose how they're read (and the
    /// two that give small limits, and two that give a window, when they
    /// choose those), run through the fuzz target's body: each promise
    /// by both rules, and the two side by side.
    #[test]
    fn random_fragments_keep_every_promise_where_the_rules_part() {
        let mut random = randoms();
        for _ in 0..20_000 {
            let mut input: Vec<u8> = (0..5).map(|_| random(256) as u8).collect();
            // From the first byte, under the default limits, mostly.
            if random(4) != 0 {
                input[..2].fill(0);
                input[2] &= !0x60;
            }
            if input[2] & 0x20 != 0 {
                input.extend_from_slice(&[random(256) as u8, random(256) as u8]);
            }
            if input[2] & 0x80 != 0 {
                input.extend_from_slice(&[random(256) as u8, random(256) as u8]);
            }
            if input[2] & 4 != 0 && random(2) == 0 {
                input[..2].fill(0);
                input.extend_from_slice(b"1 0 obj ");
                if random(2) == 0 {
                    input.extend_from_slice(b"<< /Length 3 >>\nstream\r\n");
                }
            }
            for _ in 0..random(30) + 2 {
                match random(2 * PARTING.len() + 1) {
                    n if n < PARTING.len() => input.extend_from_slice(PARTING[n]),
                    // A name past 256 bytes.
                    n if n == PARTING.len() => {
                        input.push(b'/');
                        input.extend_from_slice(&[b'P'; 300]);
                    }
                    _ => input.extend_from_slice(FRAGMENTS[random(FRAGMENTS.len())]),
                }
                if random(2) == 0 {
                    input.push(b' ');
                }
            }
            parse(&input);
        }
    }

    /// Inputs made of the fragments of objects, read from their start in
    /// every window ([`every_window`]): cut at every byte, from bytes 0
    /// and 1,000,000 of a file, each is short or reads as the whole input
    /// does, and some cuts read.
    #[test]
    fn random_fragments_in_every_window_read_as_the_whole_or_are_short() {
        let mut random = randoms();
        let (mut cuts, mut read) = (0, 0);
        for _ in 0..500 {
            let mut input = Vec::new();
            if random(2) == 0 {
                input.extend_from_slice(b"1 0 obj ");
                if random(2) == 0 {
                    input.extend_from_slice(b"<< /Length 3 >>\nstream\r\n");
                }
            }
            // Short of the last fragment, 320 nines, for every cut's sake.
            for _ in 0..random(12) + 1 {
                input.extend_from_slice(FRAGMENTS[random(FRAGMENTS.len() - 1)]);
            }
            read += every_window(&input, 3);
            cuts += 12 * (input.len() + 1);
        }
        assert!(read > cuts / 4, "{read} of {cuts} read");
    }
}
