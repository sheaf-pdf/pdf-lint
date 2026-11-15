//! Property and fuzz harnesses verifying parser, serializer, dictionary, and text-codec invariants.
//!
//! Compiled only under `#[cfg(any(test, fuzzing))]`. Exercises every input under both
//! [`Rules::Iso`] and [`Rules::Pdfium`], checks side-by-side lexical/decoding equivalence prior to
//! the first rule divergence, verifies resource limits and streaming windows, and round-trips
//! parsed values through plain, encrypted ([`Toy`]), mutated, and renumbered serialization.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

use pdf_tokenizer::decode::{self, NumberError, pdfium};
use pdf_tokenizer::{Kind, Pdfium, Rules, Token, Tokenizer};

use crate::object::{
    Data, Dict, Entry, Form, Name, Object, ObjectId, SmallBytes, Span, Str, Stream, Value,
};
use crate::parse::{Body, Error, ErrorKind, Fault, Limits, OwnedFault, Parser, Sink};
use crate::serialize::{self, Encrypt, Renumber};
use crate::text::{self, Encoding};

/// Sink recording `(Span, Display)` pairs for every emitted parser fault.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Kept {
    /// Recorded `(Span, fault.to_string())` entries in emission order.
    pub faults: Vec<(Span, String)>,
}

impl Sink<'_> for Kept {
    fn fault(&mut self, at: Span, fault: Fault<'_>) {
        self.faults.push((at, fault.to_string()));
    }
}

#[derive(Debug, Clone, Copy)]
struct How {
    offset: usize,
    seed: u16,
    stream: Option<ObjectId>,
    ciphertext: bool,
    indirect: bool,
    length: Length,
    astray: bool,
    limits: Limits,
    window: Option<Window>,
    rules: Rules,
}

#[derive(Debug, Clone, Copy)]
struct Window {
    cut: usize,
    base: usize,
}

#[derive(Debug, Clone, Copy)]
enum Length {
    Given(u16),
    Scanned(i16),
}

/// Main fuzz entry point for parser, windowing, limits, and serializer invariants.
pub fn parse(data: &[u8]) {
    let (how, input) = chosen(data);
    let iso = promised(input, how);
    let pdfium = How {
        rules: Rules::Pdfium,
        ..how
    };
    side_by_side(input, how, &iso, &promised(input, pdfium));
}

/// Fuzz entry point for both [`text`] (ISO 32000-2) and [`text::pdfium`] codecs.
pub fn texts(data: &[u8]) {
    let iso = text::decode(data);
    let pdfium = text::pdfium::decode(data);
    assert_eq!(text::decode_str(data, Rules::Iso), iso.text);
    assert_eq!(text::decode_str(data, Rules::Pdfium), pdfium.text);

    let lossy = String::from_utf8_lossy(data);
    for sample in [lossy.as_ref(), &iso.text, &pdfium.text] {
        encoded(sample);
    }
}

fn encoded(written: &str) {
    let has_esc = written.contains('\u{1B}');
    let check_decoded = |bytes: &[u8]| {
        if !has_esc {
            let iso = text::decode(bytes);
            assert_eq!(iso.text, written, "{bytes:02X?} (ISO)");
            assert_eq!(iso.quirks, [], "{bytes:02X?} (ISO)");
            assert!(iso.languages.is_empty(), "{bytes:02X?} (ISO)");

            let pdfium = text::pdfium::decode(bytes);
            assert_eq!(pdfium.text, written, "{bytes:02X?} (PDFium)");
            assert_eq!(pdfium.quirks, [], "{bytes:02X?} (PDFium)");
        }
    };

    let bytes = text::encode(written);
    check_decoded(&bytes);

    for encoding in [Encoding::PdfDoc, Encoding::Utf16Be, Encoding::Utf8] {
        if let Ok(enc_bytes) = text::encode_as(written, encoding) {
            assert!(!has_esc, "U+001B must be rejected by encode_as");
            check_decoded(&enc_bytes);
            assert_eq!(
                text::decode(&enc_bytes).encoding,
                encoding,
                "{enc_bytes:02X?}"
            );
        }
    }

    let object = Object::text(written);
    let mut out = Vec::new();
    serialize::write(&mut out, &object).expect("Object::text always serializes");
    for rules in [Rules::Iso, Rules::Pdfium] {
        let read = Parser::at(&out, 0)
            .rules(rules)
            .value(&mut ())
            .expect("serialized text string parses back");
        assert_eq!(
            read.as_bytes(),
            Some(&bytes[..]),
            "{out:02X?}, by {rules:?}"
        );
        if !has_esc {
            assert_eq!(
                read.as_str().and_then(|s| s.decode_str(rules)).as_deref(),
                Some(written)
            );
        }
    }
}

type Read = (Result<Object, Error>, usize);

fn promised(input: &[u8], how: How) -> (Read, Kept) {
    let mut kept = Kept::default();
    let first = read(input, how, &mut kept);

    let mut again = Kept::default();
    assert_eq!(first, read(input, how, &mut again), "deterministic parse");
    assert_eq!(kept, again, "deterministic faults");
    assert_eq!(first, read(input, how, &mut ()), "same result with () sink");

    // Also verify `Vec<(Span, OwnedFault)>` sink matches `Kept` 1-for-1.
    let mut owned_vec: Vec<(Span, OwnedFault)> = Vec::new();
    assert_eq!(first, read(input, how, &mut owned_vec));
    let owned_strings: Vec<(Span, String)> = owned_vec
        .into_iter()
        .map(|(sp, f)| (sp, f.to_string()))
        .collect();
    assert_eq!(kept.faults, owned_strings);

    let mut unwanted = Unwanted::default();
    assert_eq!(
        first,
        read(input, how, &mut unwanted),
        "same result when wants() == false"
    );
    assert_eq!(
        unwanted.0,
        [],
        "sink with wants() == false receives nothing"
    );

    if how.limits != Limits::DEFAULT {
        limited(input, how, &first);
    }
    if let Some(window) = how.window {
        windowed(input, how, window, &first, &kept);
    }

    let (result, offset) = first.clone();
    assert!(offset <= input.len().max(how.offset), "{offset}");
    let inside =
        |at: Span| at.stream() == how.stream && at.start() <= at.end() && at.end() <= input.len();
    for &(at, _) in &kept.faults {
        assert!(inside(at), "fault span out of input bounds: {at:?}");
    }

    match result {
        Ok(object) => {
            let whole = object.at().expect("parsed object has Span");
            let fault_bounds = if how.indirect {
                Span::new(how.stream, how.offset.min(whole.start()), whole.end())
            } else {
                whole
            };
            for &(at, _) in &kept.faults {
                assert!(
                    within(at, fault_bounds),
                    "fault {at:?} outside {fault_bounds:?}"
                );
            }
            placed(input, &object, whole, how);
            let mut found = Vec::new();
            references(&object, &mut found);
            assert!(
                object.references().eq(found),
                "Object::references matches recursive walk"
            );
            assert!(object.eq_value(&object));
            rewrite(input, how, &object);
        }
        Err(error) => {
            assert!(!error.to_string().is_empty());
            if !how.indirect {
                assert_eq!(
                    offset, how.offset,
                    "failed value() leaves parser offset unchanged"
                );
            }
            match error.kind() {
                &ErrorKind::Header { at } => {
                    assert!(how.indirect);
                    assert_eq!(at, how.offset);
                    assert_eq!(
                        offset, how.offset,
                        "failed header() leaves parser offset unchanged"
                    );
                }
                ErrorKind::Endstream { .. } | ErrorKind::Extent { .. } => {}
                _ if how.indirect => {
                    let mut parser = Parser::at(input, how.offset).rules(how.rules);
                    assert!(parser.header(&mut ()).is_ok());
                    assert_eq!(
                        offset,
                        parser.offset(),
                        "failed body() leaves parser right after header"
                    );
                }
                _ => {}
            }
        }
    }
    (first, kept)
}

fn side_by_side(input: &[u8], how: How, iso: &(Read, Kept), pdfium: &(Read, Kept)) {
    let from = how.offset.min(input.len());
    let at = parting(input, from);
    if at == input.len() {
        assert_eq!(iso, pdfium, "identical when token streams never diverge");
    } else {
        let cut = |rules| {
            let mut kept = Kept::default();
            let read = read_in(&input[..at], 0, false, How { rules, ..how }, &mut kept);
            (read, kept)
        };
        assert_eq!(
            cut(Rules::Iso),
            cut(Rules::Pdfium),
            "identical in window cut at divergence offset {at}"
        );
    }

    underflows(input, &iso.0.0, &iso.1);
    high(input, how, &pdfium.0.0, &pdfium.1);
    assert_eq!(told(&pdfium.1, &[Fault::RealUnderflow]), []);
    assert_eq!(told(&iso.1, &high_bytes()), []);

    let mut own = Own::default();
    let pdfium_how = How {
        rules: Rules::Pdfium,
        ..how
    };
    let _ = read(input, pdfium_how, &mut own);
    for at in own.0 {
        let token = Tokenizer::at(input, at.start()).with_rules(Pdfium).next();
        let token = token.filter(|t| t.end() == at.end()).expect("a token");
        let iso_tok = Tokenizer::at(input, at.start()).next();
        assert!(
            iso_tok != Some(token) || read_otherwise(&token),
            "PDFium-only fault at {at:?}, which ISO reads identically"
        );
    }
}

fn told(kept: &Kept, faults: &[Fault<'_>]) -> Vec<Span> {
    let said: Vec<String> = faults.iter().map(Fault::to_string).collect();
    kept.faults
        .iter()
        .filter(|(_, fault)| said.contains(fault))
        .map(|&(at, _)| at)
        .collect()
}

fn high_bytes() -> [Fault<'static>; 2] {
    [&[0x80u8][..], &[0xFF]].map(|written| Fault::HighByte { written })
}

fn underflows(input: &[u8], read: &Result<Object, Error>, kept: &Kept) {
    let tiny = |at: Span| {
        matches!(
            decode::number(&input[at.start()..at.end()]),
            Err(NumberError::Underflow)
        )
    };
    let told = told(kept, &[Fault::RealUnderflow]);
    for &at in &told {
        let token = Tokenizer::at(input, at.start()).next();
        let token = token.filter(|t| t.kind == Kind::Real && t.end() == at.end());
        assert!(token.is_some() && tiny(at), "too small, at {at:?}");
    }
    let Ok(object) = read else {
        return;
    };
    let mut found = Vec::new();
    reals(object, &mut |at, real| {
        if tiny(at) {
            let zero = match input[at.start()] {
                b'-' => -0.0f64,
                _ => 0.0,
            };
            assert_eq!(real.to_bits(), zero.to_bits(), "too small, read at {at:?}");
            found.push(at);
        }
    });
    assert_eq!(found, told);
}

fn reals(o: &Object, each: &mut impl FnMut(Span, f64)) {
    let entries = |d: &Dict, each: &mut _| d.entries().iter().for_each(|e| reals(e.value(), each));
    match o.value() {
        Value::Real(real) => each(o.at().expect("placed"), *real),
        Value::Array(a) => a.iter().for_each(|o| reals(o, each)),
        Value::Dict(d) => entries(d, each),
        Value::Stream(s) => entries(&s.dict, each),
        _ => {}
    }
}

fn high(input: &[u8], how: How, read: &Result<Object, Error>, kept: &Kept) {
    let [x80, xff] = high_bytes().map(|fault| fault.to_string());
    let mut told = Vec::new();
    for (at, fault) in &kept.faults {
        let byte = match fault {
            said if *said == x80 => 0x80,
            said if *said == xff => 0xFF,
            _ => continue,
        };
        assert_eq!(
            (at.end() - at.start(), input[at.start()]),
            (1, byte),
            "{at:?}"
        );
        assert!(Rules::Pdfium.is_white_space(byte) && !Rules::Iso.is_white_space(byte));
        assert!(!told.contains(at), "told twice, at {at:?}");
        told.push(*at);
    }

    let mut found = Vec::new();
    let mut spaces = |from: usize, to: usize| {
        let mut t = Tokenizer::at(input, from).with_rules(Pdfium);
        while t.offset() < to {
            let Some(token) = t.next() else {
                break;
            };
            if token.kind == Kind::Space {
                let bytes = (token.start..token.end().min(to))
                    .filter(|&at| matches!(input[at], 0x80 | 0xFF));
                found.extend(bytes.map(|at| Span::new(how.stream, at, at + 1)));
            }
        }
    };

    if how.indirect {
        let mut hp = Parser::at(input, how.offset).rules(Rules::Pdfium);
        if hp.header(&mut ()).is_ok() {
            let mut t = Tokenizer::at(input, how.offset).with_rules(Pdfium);
            let n = t.next_significant().expect("header n");
            let _g = t.next_significant().expect("header g");
            let obj = t.next_significant().expect("header obj");
            spaces(n.start, obj.end());
        }
    }

    let Ok(object) = read else {
        assert_eq!(told, found, "header high bytes when body failed");
        return;
    };
    let whole = object.at().expect("placed");
    match object.as_stream().and_then(|s| s.data.at()) {
        Some(data) => {
            spaces(whole.start(), data.start());
            spaces(data.end(), whole.end());
        }
        None => spaces(whole.start(), whole.end()),
    }
    assert_eq!(told, found, "each high byte told once in order");
}

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

fn read_otherwise(token: &Token<'_>) -> bool {
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
        Kind::Real => {
            let iso = match decode::number(bytes) {
                Ok(x) => Some(x),
                Err(NumberError::Underflow) => Some(0.0),
                Err(_) => None,
            };
            long || iso != Some(pdfium::real(bytes))
        }
        Kind::Name => long || !pdfium::name(bytes).eq(decode::name(bytes).bytes()),
        Kind::LiteralString => !pdfium::literal(bytes).eq(decode::literal(bytes).bytes()),
        Kind::HexString => !pdfium::hex(bytes).eq(decode::hex(bytes).bytes()),
        _ => false,
    }
}

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

#[derive(Default)]
struct Unwanted(Vec<Span>);

impl Sink<'_> for Unwanted {
    fn fault(&mut self, at: Span, _: Fault<'_>) {
        self.0.push(at);
    }

    fn wants(&self) -> bool {
        false
    }
}

fn significant(input: &[u8], at: usize, rules: Rules) -> Option<Token<'_>> {
    Tokenizer::at(input, at)
        .with_rules(rules)
        .next_significant()
}

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
            let limits = Limits::new(usize::from(nesting % 8), usize::from(values % 64));
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
    let window = window.map(|wseed| {
        let wseed = usize::from(wseed);
        // Exercise both `cut >= offset` and `cut < offset` (when bit 1 is set and offset > 0).
        let cut = if (wseed & 2 != 0) && !input.is_empty() {
            (wseed >> 2) % (input.len() + 1)
        } else {
            let from = offset.min(input.len());
            from + (wseed >> 2) % (input.len() - from + 1)
        };
        Window {
            cut,
            base: (wseed & 1) * 1_000_000,
        }
    });
    let how = How {
        offset,
        seed,
        stream: (flags & 1 != 0).then_some(ObjectId::new(u32::from(*c).max(1), 0)),
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

fn limited(input: &[u8], how: How, small: &(Result<Object, Error>, usize)) {
    let limits = how.limits;
    let full_how = How {
        limits: Limits::DEFAULT,
        ..how
    };
    let full = read(input, full_how, &mut ());
    let (at, past) = match small.0.as_ref().map_err(Error::kind) {
        Err(&ErrorKind::Nested { limit, at }) => {
            assert_eq!(limit, limits.nesting);
            (at, full.0.as_ref().map(|o| deepest(o) > limit))
        }
        Err(&ErrorKind::Values { limit, at }) => {
            assert_eq!(limit, limits.values);
            (at, full.0.as_ref().map(|o| count(o) > limit))
        }
        _ => {
            assert_eq!(*small, full, "same result when small limits are not hit");
            return;
        }
    };
    assert!(past.unwrap_or(true));
    assert!(at < input.len(), "{at}");
    let there = Parser::at(input, at).rules(how.rules).value(&mut ());
    let not_a_value = matches!(
        there.as_ref().map_err(Error::kind),
        Err(
            &ErrorKind::Token { at: a, .. }
                | &ErrorKind::Keyword { at: a, .. }
                | &ErrorKind::String { at: a }
        ) if a == at
    );
    assert!(!not_a_value, "limit reached at {at}: {there:?}");
}

fn count(o: &Object) -> usize {
    let entries = |d: &Dict| -> usize { d.entries().iter().map(|e| 1 + count(e.value())).sum() };
    1 + match o.value() {
        Value::Array(a) => a.iter().map(count).sum(),
        Value::Dict(d) => entries(d),
        Value::Stream(s) => entries(&s.dict),
        _ => 0,
    }
}

fn references(o: &Object, found: &mut Vec<ObjectId>) {
    let entries = |d: &Dict, found: &mut Vec<ObjectId>| {
        d.entries()
            .iter()
            .for_each(|e| references(e.value(), found));
    };
    match o.value() {
        Value::Ref(id) => found.push(*id),
        Value::Array(a) => a.iter().for_each(|o| references(o, found)),
        Value::Dict(d) => entries(d, found),
        Value::Stream(s) => entries(&s.dict, found),
        _ => {}
    }
}

fn deepest(o: &Object) -> usize {
    let entries = |d: &Dict| d.entries().iter().map(|e| deepest(e.value())).max();
    let inside = match o.value() {
        Value::Array(a) => a.iter().map(deepest).max(),
        Value::Dict(d) => entries(d),
        Value::Stream(s) => entries(&s.dict),
        _ => None,
    };
    inside.map_or(0, |d| d + 1)
}

fn read<'a>(input: &'a [u8], how: How, sink: &mut impl Sink<'a>) -> Read {
    read_in(input, 0, true, how, sink)
}

fn read_in<'a>(
    input: &'a [u8],
    base: usize,
    last: bool,
    how: How,
    sink: &mut impl Sink<'a>,
) -> (Result<Object, Error>, usize) {
    let mut parser = Parser::at(input, how.offset)
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
    let object = parser
        .header(sink)
        .and_then(|id| match parser.body(id, sink)? {
            Body::Object(object) => Ok(object),
            Body::Stream(head) => {
                let start = head.start();
                let keyword = head.keyword();
                assert!(keyword.end() <= start && start - base <= input.len());
                let scanned = head.scanned();
                assert_eq!((scanned.stream(), scanned.start()), (how.stream, start));
                let marker = &b"endstream"[..];
                let data_slice = &input[scanned.start() - base..scanned.end() - base];
                // Any `endstream` inside `data_slice` must be followed by a regular byte (`endstreamX`)
                // or sit inside the stripped EOL tail—never a standalone `endstream` keyword!
                for (pos, w) in data_slice.windows(marker.len()).enumerate() {
                    if w == marker {
                        let abs_after = scanned.start() - base + pos + marker.len();
                        assert!(
                            abs_after < input.len() && how.rules.is_regular(input[abs_after]),
                            "standalone endstream inside scanned payload at {pos}"
                        );
                    }
                }
                let after = &input[scanned.end() - base..];
                if !after.is_empty() {
                    let after_eol = after
                        .strip_prefix(b"\r\n")
                        .or_else(|| after.strip_prefix(b"\n"))
                        .or_else(|| after.strip_prefix(b"\r"))
                        .unwrap_or(after);
                    assert!(after_eol.starts_with(marker), "unexpected tail {after:?}");
                    if let Some(&next) = after_eol.get(marker.len()) {
                        assert!(!how.rules.is_regular(next));
                    }
                }
                let length = match how.length {
                    Length::Given(length) => usize::from(length),
                    Length::Scanned(more) => {
                        (scanned.end() - start).saturating_add_signed(isize::from(more))
                    }
                };
                let declared = head.declared(length);
                if let Some(declared) = declared {
                    assert_eq!((declared.start(), declared.len()), (start, length));
                    let tok = significant(input, declared.end() - base, how.rules);
                    assert!(tok.is_some_and(|t| t.bytes == marker));
                }
                let chosen_data = declared.unwrap_or(scanned);
                if how.astray {
                    let astray = Span::new(
                        how.stream,
                        chosen_data.start() + 1,
                        (chosen_data.start() + 1).max(chosen_data.end()),
                    );
                    let refused = parser.clone().stream(head.clone(), astray, &mut ());
                    let short = !last
                        && matches!(
                            refused.as_ref().map_err(Error::kind),
                            Err(ErrorKind::Short { .. })
                        );
                    assert!(
                        short
                            || matches!(
                                refused.as_ref().map_err(Error::kind),
                                Err(ErrorKind::Extent { .. })
                            ),
                        "{refused:?}"
                    );
                }
                let object = parser.stream(head, chosen_data, sink)?;
                let Value::Stream(stream) = object.value() else {
                    panic!("a stream reads as one");
                };
                assert_eq!(stream.data, Data::At(chosen_data));
                Ok(object)
            }
        });
    (object, parser.offset())
}

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
    let said_shifted: Vec<(Span, String)> = said
        .faults
        .iter()
        .map(|(at, text)| (shifted_span(*at, base), text.clone()))
        .collect();

    let mut kept = Kept::default();
    let last = read_in(input, base, true, how, &mut kept);
    assert_eq!(last, want, "last window matches whole input");
    assert_eq!(kept.faults, said_shifted, "last window faults match whole");

    let mut kept = Kept::default();
    let read = read_in(&input[..cut], base, false, how, &mut kept);
    if let Err(err) = &read.0
        && let &ErrorKind::Short { at } = err.kind()
    {
        assert_eq!(at, base + cut, "Short reported at exact window boundary");
        return true;
    }
    assert_eq!(read, want, "cut at {cut} matches whole");
    assert_eq!(kept.faults, said_shifted, "cut at {cut} faults match whole");
    false
}

/// Checks `input` across every single-byte window cut at `base = 0` and `base = 1_000_000`.
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
    let mut read_count = 0;
    let combos = [Rules::Iso, Rules::Pdfium]
        .map(|rules| [(rules, None), (rules, Some(ObjectId::new(7, 0)))]);
    for (rules, stream) in combos.concat() {
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
                    read_count += usize::from(!short);
                }
            }
        }
    }
    read_count
}

fn shifted(mut o: Object, base: usize) -> Object {
    shift(&mut o, base);
    o
}

fn shift(o: &mut Object, base: usize) {
    let new_at = o.at().map(|at| shifted_span(at, base));
    let entries = |d: &mut Dict| {
        for entry in d.entries_mut() {
            let key_at = entry.key_at().map(|at| shifted_span(at, base));
            shift(entry.value_mut(), base);
            let moved_val = core::mem::replace(entry.value_mut(), Object::null());
            let mut rebuilt = Entry::new(Name::from(entry.key()), moved_val);
            if let Some(sp) = key_at {
                rebuilt = rebuilt.with_key_span(sp);
            }
            *entry = rebuilt;
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
    let inner = core::mem::replace(o, Object::null()).into_value();
    *o = match new_at {
        Some(sp) => Object::new(inner).with_span(sp),
        None => Object::new(inner),
    };
}

fn shifted_span(at: Span, base: usize) -> Span {
    Span::new(at.stream(), at.start() + base, at.end() + base)
}

fn shifted_error(error: Error, base: usize) -> Error {
    let kind = match error.into_kind() {
        ErrorKind::Nested { limit, at } => ErrorKind::Nested {
            limit,
            at: at + base,
        },
        ErrorKind::Values { limit, at } => ErrorKind::Values {
            limit,
            at: at + base,
        },
        ErrorKind::End => ErrorKind::End,
        ErrorKind::Reference {
            at,
            number,
            generation,
        } => ErrorKind::Reference {
            at: at + base,
            number,
            generation,
        },
        ErrorKind::String { at } => ErrorKind::String { at: at + base },
        ErrorKind::Array { at } => ErrorKind::Array { at: at + base },
        ErrorKind::Dict { at } => ErrorKind::Dict { at: at + base },
        ErrorKind::Key { at } => ErrorKind::Key { at: at + base },
        ErrorKind::Keyword { at, word } => ErrorKind::Keyword {
            at: at + base,
            word,
        },
        ErrorKind::Token { at, bytes } => ErrorKind::Token {
            at: at + base,
            bytes,
        },
        ErrorKind::Header { at } => ErrorKind::Header { at: at + base },
        ErrorKind::Endstream { id } => ErrorKind::Endstream { id },
        ErrorKind::Extent { id, start, end } => ErrorKind::Extent {
            id,
            start: start + base,
            end: end + base,
        },
        ErrorKind::Short { at } => ErrorKind::Short { at: at + base },
    };
    Error::new(kind)
}

/// Replaces any `Data::At` stream payload on `o` with `Data::Bytes` copied from `data`, then
/// strips all byte spans via [`Object::unplace`].
pub fn unplace(o: &mut Object, data: &[u8]) {
    if let Value::Stream(s) = o.as_written_mut()
        && let Some(at) = s.data.at()
    {
        s.data = Data::Bytes(data[at.start()..at.end()].to_vec());
    }
    o.unplace();
}

fn has_unserializable_parts(o: &Object) -> bool {
    let check_dict = |d: &Dict| {
        d.entries()
            .iter()
            .any(|e| e.key().contains(&0) || has_unserializable_parts(e.value()))
    };
    match o.value() {
        Value::Name(n) => n.as_bytes().contains(&0),
        Value::Real(x) => !x.is_finite() || serialize::real(&mut Vec::new(), *x).is_err(),
        Value::Ref(id) => id.number == 0,
        Value::Array(a) => a.iter().any(has_unserializable_parts),
        Value::Dict(d) => check_dict(d),
        Value::Stream(s) => check_dict(&s.dict),
        _ => false,
    }
}

fn sanitize_for_serialize(o: &mut Object) {
    let sanitize_dict = |d: &mut Dict| {
        for entry in d.entries_mut() {
            if entry.key().contains(&0) {
                let clean: Vec<u8> = entry
                    .key()
                    .iter()
                    .copied()
                    .map(|b| if b == 0 { b'_' } else { b })
                    .collect();
                entry.set_key(clean);
            }
            sanitize_for_serialize(entry.value_mut());
        }
    };
    match o.as_written_mut() {
        Value::Name(n) if n.as_bytes().contains(&0) => {
            let clean: Vec<u8> = n
                .as_bytes()
                .iter()
                .copied()
                .map(|b| if b == 0 { b'_' } else { b })
                .collect();
            *n = Name::from(clean);
        }
        Value::Real(x) if !x.is_finite() || serialize::real(&mut Vec::new(), *x).is_err() => {
            *x = if x.is_nan() {
                0.0
            } else if *x > 0.0 {
                3.4e38
            } else {
                -3.4e38
            };
        }
        Value::Ref(id) if id.number == 0 => {
            id.number = 1;
        }
        Value::Array(a) => a.iter_mut().for_each(sanitize_for_serialize),
        Value::Dict(d) => sanitize_dict(d),
        Value::Stream(s) => sanitize_dict(&mut s.dict),
        _ => {}
    }
}

fn rewrite(input: &[u8], how: How, object: &Object) {
    let parsed_id = match how.indirect {
        true => Some(
            Parser::at(input, how.offset)
                .rules(how.rules)
                .header(&mut ())
                .expect("its header"),
        ),
        false => None,
    };

    let mut object = object.clone();
    if let Some(id) = parsed_id
        && id.number == 0
    {
        let mut out = b"%PDF-".to_vec();
        assert_eq!(
            write(&mut out, &object, Some(id), input),
            Err(serialize::Error::ZeroObjectId { id: Some(id) })
        );
        assert_eq!(out, b"%PDF-");
    }

    let id = parsed_id.map(|id| ObjectId::new(id.number.max(1), id.generation));

    if how.ciphertext {
        let mut out = b"%PDF-".to_vec();
        let written = write(&mut out, &object, id, input);
        let strings = plain(&mut object);
        if strings > 0 {
            assert!(written.is_err());
            assert_eq!(out, b"%PDF-", "ciphertext refusal rolls back output");
        }
    }

    if has_unserializable_parts(&object) {
        let mut out = b"%PDF-".to_vec();
        let err = write(&mut out, &object, id, input).expect_err("unserializable value refused");
        assert!(
            matches!(
                err,
                serialize::Error::NulName { .. }
                    | serialize::Error::RealOutOfRange { .. }
                    | serialize::Error::ZeroObjectId { .. }
                    | serialize::Error::NonFinite { .. }
            ),
            "unexpected refusal: {err:?}"
        );
        assert_eq!(out, b"%PDF-", "refusal rolls back output");
        sanitize_for_serialize(&mut object);
    }

    let written = round_trip(&object, id, input, how.stream, how.limits);
    if let Some(id) = id {
        toyed(&object, id, input, how.limits, &written);
    }
    let to = renumbering(how.seed);
    renumbered(&object, id, input, how.stream, how.limits, &to, &written);

    // Exercise post-parse mutations (`Dict::set`, `Dict::remove`, `Stream::set_bytes`) before
    // re-serializing under default limits.
    if how.limits == Limits::DEFAULT {
        let mut mutated = object.clone();
        unplace(&mut mutated, input);
        let did_mutate = match mutated.value_mut() {
            Value::Dict(d) => {
                d.set("MutatedKey", 42);
                let _ = d.remove("Length");
                true
            }
            Value::Stream(s) => {
                s.dict.set("MutatedKey", Object::name("Edited"));
                s.set_bytes(b"mutated-payload".to_vec());
                true
            }
            _ => false,
        };
        if did_mutate {
            let _ = round_trip(&mutated, id, &[], how.stream, Limits::DEFAULT);
        }
    }
}

/// Deterministic reference-renumbering function derived from `seed`.
pub fn renumbering(seed: u16) -> impl Fn(ObjectId) -> Option<ObjectId> {
    move |id: ObjectId| match seed {
        0 => (id.number != 0).then_some(id),
        _ if id.number % 7 == u32::from(seed % 7) => None,
        _ => {
            let n = id.number.wrapping_add(u32::from(seed));
            (n != 0).then_some(ObjectId::new(n, id.generation))
        }
    }
}

/// Serializes `object`, parses the emitted bytes back under `limits`, verifies zero faults and
/// exact structural equality, and confirms re-serializing produces identical bytes.
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

fn read_back(
    written: &[u8],
    want: &Object,
    object: &Object,
    id: Option<ObjectId>,
    stream: Option<ObjectId>,
    limits: Limits,
) {
    let adjusted_limits = written_under(object, limits);
    let mut parser = Parser::at(written, 0).limits(adjusted_limits);
    if let Some(stream) = stream {
        parser = parser.in_stream(stream);
    }
    let text = String::from_utf8_lossy(written);
    let mut faults = Kept::default();
    let (back, after) = match id {
        Some(id) => {
            assert_eq!(parser.header(&mut faults), Ok(id));
            let back = parser.body(id, &mut faults).and_then(|body| match body {
                Body::Object(object) => Ok(object),
                Body::Stream(head) => {
                    let length = head.dict().get("Length").and_then(Object::as_integer);
                    let length = usize::try_from(length.expect("an integer /Length")).unwrap();
                    let data = head.declared(length).expect("endstream after its /Length");
                    parser.stream(head, data, &mut faults)
                }
            });
            (back, &b"\n"[..])
        }
        None => (parser.value(&mut faults), &b""[..]),
    };
    let mut back = back.unwrap_or_else(|e| panic!("{e}, reading back {text:?}"));
    assert_eq!(&written[parser.offset()..], after, "{text:?}");
    assert!(
        faults.faults.is_empty(),
        "serialized output emitted faults: {:?} in {text:?}",
        faults.faults
    );
    unplace(&mut back, written);
    assert_eq!(back, *want, "read back from {text:?}");

    let mut again = Vec::new();
    write(&mut again, &back, id, &[]).expect("what's read back writes");
    assert_eq!(again, written, "idempotent re-serialization");
}

/// Verifies `serialize::*_renumbered` matches in-memory [`Object::renumber`] and [`Toy`] encryption.
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
    let changed = moved.renumber(|rid| to.renumber(rid));
    let named = object.references().filter_map(|rid| to.renumber(rid));
    assert!(moved.references().eq(named));
    match changed {
        false => assert_eq!(moved, *object),
        true => assert_eq!(moved.at(), None),
    }
    let mut written = Vec::new();
    let at = object
        .as_stream()
        .and_then(|s| Some((&s.dict, s.data.at()?)));
    let refused = match (id, at) {
        (Some(id), Some((dict, at))) => {
            let slice = &data[at.start()..at.end()];
            serialize::indirect_stream_renumbered(&mut written, id, dict, slice, &(), to).map(drop)
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
        assert_eq!(written, plain);
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
                let slice = &data[at.start()..at.end()];
                serialize::indirect_stream_renumbered(&mut sealed, id, dict, slice, &toy, to)
            }
            None => serialize::indirect_renumbered(&mut sealed, id, object, &toy, to),
        };
        if let Err(e) = refused {
            panic!("the writer refused {object:?} renumbered through the toy: {e}");
        }
        undone(&toy, &sealed, &moved, id, limits, &written);
    }
}

fn written_under(object: &Object, limits: Limits) -> Limits {
    if object.as_stream().is_some_and(|s| !s.dict.has("Length")) {
        Limits::new(limits.nesting.max(1), limits.values.saturating_add(2))
    } else {
        limits
    }
}

/// Verifies `serialize::indirect_with` / `indirect_stream_with` against [`Toy`] encryption and
/// rollback on refusal.
pub fn toyed(object: &Object, id: ObjectId, data: &[u8], limits: Limits, plain: &[u8]) {
    let toy = Toy::default();
    let mut written = Vec::new();
    let refused = match object.as_stream() {
        Some(Stream {
            dict,
            data: Data::At(at),
        }) => serialize::indirect_stream_with(
            &mut written,
            id,
            dict,
            &data[at.start()..at.end()],
            &toy,
        ),
        _ => serialize::indirect_with(&mut written, id, object, &toy),
    };
    if let Err(e) = refused {
        panic!("the writer refused {object:?} through the toy: {e}");
    }
    undone(&toy, &written, object, id, limits, plain);

    // If the toy encrypted at least one item, refusing that item must roll back cleanly.
    if let Some((_, first_bytes, _)) = toy.seen.borrow().first().cloned() {
        let rejecting = Toy {
            refuse: Some(first_bytes),
            ..Toy::default()
        };
        let mut rollback = b"%PDF-".to_vec();
        let res = match object.as_stream() {
            Some(Stream {
                dict,
                data: Data::At(at),
            }) => serialize::indirect_stream_with(
                &mut rollback,
                id,
                dict,
                &data[at.start()..at.end()],
                &rejecting,
            ),
            _ => serialize::indirect_with(&mut rollback, id, object, &rejecting),
        };
        assert_eq!(
            res,
            Err(serialize::Refused::Encrypt {
                id,
                why: "the toy refuses it"
            })
        );
        assert_eq!(rollback, b"%PDF-");
    }
}

fn undone(toy: &Toy, written: &[u8], object: &Object, id: ObjectId, limits: Limits, plain: &[u8]) {
    let text = String::from_utf8_lossy(written);
    let mut parser = Parser::at(written, 0).limits(written_under(object, limits));
    assert_eq!(parser.header(&mut ()), Ok(id));
    let back = parser.body(id, &mut ()).and_then(|body| match body {
        Body::Object(object) => Ok(object),
        Body::Stream(head) => {
            let length = head.dict().get("Length").and_then(Object::as_integer);
            let length = usize::try_from(length.expect("an integer /Length")).unwrap();
            let data = head.declared(length).expect("endstream after its /Length");
            parser.stream(head, data, &mut ())
        }
    });
    let mut back = back.unwrap_or_else(|e| panic!("{e}, reading back {text:?}"));
    assert_eq!(&written[parser.offset()..], b"\n", "{text:?}");
    unplace(&mut back, written);
    toy.undo(id, &mut back);
    let mut again = Vec::new();
    write(&mut again, &back, Some(id), &[]).expect("what's undone writes");
    assert_eq!(again, plain, "undone {text:?} matches plain output");
}

/// Reversible toy [`Encrypt`] implementation for serializer property tests.
#[derive(Debug, Default)]
pub struct Toy {
    /// Sequence of `(id, plaintext_bytes, is_stream)` passed to the encryptor.
    pub seen: RefCell<Vec<(ObjectId, Vec<u8>, bool)>>,
    /// Optional byte slice that triggers a simulated encryption refusal.
    pub refuse: Option<Vec<u8>>,
}

impl Encrypt for Toy {
    type Error = &'static str;

    fn clear(&self, id: ObjectId, dict: Option<&Dict>, stream: bool) -> bool {
        let xref = |d: &Dict| d.has_type(b"XRef");
        id.number.is_multiple_of(5) || (stream && dict.is_some_and(xref))
    }

    fn is_clear_key(&self, _: ObjectId, dict: &Dict, key: &[u8]) -> bool {
        dict.has("ByteRange") && (key == b"Contents" || key == b"Prop_Build")
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
    fn sealed(&self, id: ObjectId, bytes: &[u8], stream: bool) -> Result<Vec<u8>, &'static str> {
        self.seen.borrow_mut().push((id, bytes.to_vec(), stream));
        if self.refuse.as_deref() == Some(bytes) {
            return Err("the toy refuses it");
        }
        let mut sealed = Vec::with_capacity(6 + bytes.len());
        sealed.extend_from_slice(&id.number.to_be_bytes());
        sealed.extend_from_slice(&id.generation.to_be_bytes());
        sealed.extend(bytes.iter().rev().copied());
        Ok(sealed)
    }

    /// Reverses [`Toy`]'s encryption in place on `o` and asserts exact call ordering.
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
        assert_eq!(next, seen.len(), "every encrypted item accounted for once");
    }

    fn opened(
        &self,
        id: ObjectId,
        o: &mut Object,
        seen: &[(ObjectId, Vec<u8>, bool)],
        next: &mut usize,
    ) {
        let mut open_vec = |raw: &[u8], stream: bool| -> Vec<u8> {
            let mut tag = [0u8; 6];
            tag[..4].copy_from_slice(&id.number.to_be_bytes());
            tag[4..].copy_from_slice(&id.generation.to_be_bytes());
            let rest = raw.strip_prefix(&tag).expect("written with object tag");
            let bytes: Vec<u8> = rest.iter().rev().copied().collect();
            assert_eq!(
                seen.get(*next),
                Some(&(id, bytes.clone(), stream)),
                "encrypted in order"
            );
            *next += 1;
            bytes
        };
        match o.as_written_mut() {
            Value::String(Str::Bytes(sealed), _) => {
                let opened = open_vec(sealed.as_slice(), false);
                *sealed = SmallBytes::from_vec(opened);
            }
            Value::Array(a) => a.iter_mut().for_each(|e| self.opened(id, e, seen, next)),
            Value::Dict(d) => self.entries(id, d, seen, next),
            Value::Stream(s) => {
                let Data::Bytes(sealed) = &mut s.data else {
                    panic!("stream data must be unplaced");
                };
                *sealed = open_vec(sealed, true);
                self.entries(id, &mut s.dict, seen, next);
            }
            _ => {}
        }
    }

    fn entries(
        &self,
        id: ObjectId,
        d: &mut Dict,
        seen: &[(ObjectId, Vec<u8>, bool)],
        next: &mut usize,
    ) {
        let has_byte_range = d.has("ByteRange");
        for entry in d.entries_mut() {
            let is_clear =
                has_byte_range && (entry.key() == b"Contents" || entry.key() == b"Prop_Build");
            if !is_clear {
                self.opened(id, entry.value_mut(), seen, next);
            }
        }
    }
}

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
        ) => serialize::indirect_stream(out, id, dict, &data[at.start()..at.end()]).map(drop),
        (Some(id), _) => serialize::indirect(out, id, object).map(drop),
        (None, _) => serialize::write(out, object),
    }
}

fn as_written(o: &mut Object) {
    match o.value_mut() {
        Value::Real(x) if *x == 0.0 => {
            *x = 0.0;
        }
        Value::String(_, form) => {
            form.get_or_insert(Form::Literal);
        }
        Value::Array(a) => a.iter_mut().for_each(as_written),
        Value::Dict(d) => d.entries_mut().for_each(|e| as_written(e.value_mut())),
        Value::Stream(s) => {
            let length = s.data.bytes().expect("stream data in memory").len() as i64;
            let mut has = false;
            for entry in s.dict.entries_mut() {
                if entry.key() == b"Length" {
                    *entry.value_mut() = Object::from(length);
                    has = true;
                } else {
                    as_written(entry.value_mut());
                }
            }
            if !has {
                s.dict.push("Length", length);
            }
        }
        _ => {}
    }
}

fn plain(o: &mut Object) -> usize {
    let entries = |d: &mut Dict| -> usize { d.entries_mut().map(|e| plain(e.value_mut())).sum() };
    match o.as_written_mut() {
        Value::String(s, _) => {
            if let Str::Encrypted(bytes) = s {
                let raw = core::mem::replace(bytes, SmallBytes::new());
                *s = Str::Bytes(raw);
            }
            1
        }
        Value::Array(a) => a.iter_mut().map(plain).sum(),
        Value::Dict(d) => entries(d),
        Value::Stream(s) => entries(&mut s.dict),
        _ => 0,
    }
}

fn within(inner: Span, outer: Span) -> bool {
    inner.stream() == outer.stream() && outer.start() <= inner.start() && inner.end() <= outer.end()
}

fn placed(input: &[u8], o: &Object, outer: Span, how: How) {
    let at = o.at().expect("every value placed");
    assert_eq!(at.stream(), how.stream);
    assert!(at.start() < at.end() && at.end() <= input.len(), "{at:?}");
    assert!(within(at, outer), "{at:?} in {outer:?}");
    let dict = match o.value() {
        Value::String(s, form) => {
            assert_eq!(s.is_encrypted(), how.ciphertext, "{s:?}");
            let written = match input[at.start()] {
                b'<' => Form::Hex,
                b'(' => Form::Literal,
                other => panic!("string starts with {other:#04x}"),
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
            let data = s.data.at().expect("stream data placed");
            assert!(
                at.start() < data.start() && data.end() < at.end(),
                "{data:?} in {at:?}"
            );
            Some(&s.dict)
        }
        _ => None,
    };
    for entry in dict.into_iter().flat_map(Dict::entries) {
        let key = entry.key_at().expect("every key placed");
        assert!(within(key, at) && key.start() < key.end());
        assert_eq!(input[key.start()], b'/');
        let value_at = entry.value().at().expect("every value placed");
        assert!(key.end() <= value_at.start());
        placed(input, entry.value(), at, how);
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::{every_window, parse, texts};

    const FRAGMENTS: &[&[u8]] = &[
        b"1 0 obj",
        b"0 0 obj",
        b"+1 +0 obj",
        b"2 0 R",
        b"0 0 R",
        b"+2 0 R",
        b"obj",
        b"endobj",
        b"stream\r\n",
        b"stream \r\n",
        b"stream\n",
        b"stream\r",
        b"stream",
        b"endstream",
        b"endstreamX",
        b"\r\nendstream",
        b"%comment\nendstream",
        b"\0endstream",
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
        b"99999999999999999999",
        &[b'9'; 320],
    ];

    fn randoms() -> impl FnMut(usize) -> usize {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        move |below: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % below as u64) as usize
        }
    }

    #[test]
    fn random_text_strings_keep_every_promise() {
        const PIECES: &[&[u8]] = &[
            b"\xFE\xFF",
            b"\xFF\xFE",
            b"\xEF\xBB\xBF",
            b"\x00\x1B",
            b"\x1B",
            b"en\x1B",
            b"\x00e\x00n\x00\x1B",
            b"\x00A",
            b"A\x00",
            b"(",
            b")",
            b"\\",
            b"\r",
            b"\n",
            b"\xD8\x3D",
            b"\xDE\x00",
            b"\xC3\xA9",
            b"\xC3",
            b"\xE9",
            b"\xBF",
            b"\x7F",
            b"\x18",
            b"\x9F",
            b"\xAD",
            b"\xF8",
            b"\xF4\x90\x80\x80",
            b"\xED\xA0\x80",
            b"\xF0\x9F\x98\x80",
            "þÿ".as_bytes(),
            "ï»¿".as_bytes(),
            "ÿþ".as_bytes(),
        ];
        let mut random = randoms();
        for _ in 0..5_000 {
            let mut input = Vec::new();
            for _ in 0..random(8) {
                input.extend_from_slice(PIECES[random(PIECES.len())]);
            }
            texts(&input);
        }
    }

    #[test]
    fn random_fragments_keep_every_promise() {
        let mut random = randoms();
        for _ in 0..20_000 {
            let mut input: Vec<u8> = (0..5).map(|_| random(256) as u8).collect();
            if random(2) == 0 {
                input[..2].fill(0);
            }
            if random(8) != 0 {
                input[2] &= !0x40;
            }
            if random(2) == 0 {
                input[2] &= !0x20;
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
            if input[2] & 8 != 0 && random(2) == 0 {
                input[3..5].fill(0);
            }
            for _ in 0..random(40) + 2 {
                input.extend_from_slice(FRAGMENTS[random(FRAGMENTS.len())]);
            }
            parse(&input);
        }
    }

    #[test]
    fn wide_dicts_over_64_entries_and_endstreamx_streams_keep_every_promise() {
        let mut wide = b"\x00\x00\x04\x00\x001 0 obj <<".to_vec();
        for i in 0..80 {
            wide.extend_from_slice(alloc::format!(" /K{} {}", i % 50, i).as_bytes());
        }
        wide.extend_from_slice(b" >> endobj");
        parse(&wide);

        let stream_false_marker =
            b"\x00\x00\x0C\x00\x001 0 obj << /Length 999 >>\nstream\nabc endstreamX def\r\nendstream\nendobj";
        parse(stream_false_marker);
    }

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

    #[test]
    fn random_fragments_keep_every_promise_where_the_rules_part() {
        let mut random = randoms();
        for _ in 0..20_000 {
            let mut input: Vec<u8> = (0..5).map(|_| random(256) as u8).collect();
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
                match random(2 * PARTING.len() + 2) {
                    n if n < PARTING.len() => input.extend_from_slice(PARTING[n]),
                    n if n == PARTING.len() => {
                        input.push(b'/');
                        input.extend_from_slice(&[b'P'; 300]);
                    }
                    n if n == PARTING.len() + 1 => {
                        input.extend_from_slice(&b"-0."[random(2)..]);
                        input.extend_from_slice(&[b'0'; 330]);
                        input.push(b'1');
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

    #[test]
    fn seeds_with_high_bytes_and_reals_too_small_keep_every_promise() {
        const VALUE: &[u8] = &[0, 0, 0, 0, 0];
        const INDIRECT: &[u8] = &[0, 0, 4, 2, 0];
        const WINDOW: &[u8] = &[0, 0, 0x80, 0, 0, 13, 0];
        let tiny = [b"0.".as_slice(), &[b'0'; 400], b"1"].concat();
        let reals = [b"[".as_slice(), &tiny, b" -", &tiny, b" 1.5]"].concat();
        let seeds: [(&[u8], &[u8]); 10] = [
            (VALUE, b"[1\x80 2]"),
            (VALUE, b"5\x80"),
            (VALUE, b"\x80 5\x80 6"),
            (
                VALUE,
                b"<< /A\xFF 1 /B [(\x80) 1 0\x80R %\xFF\n/C]\x80>>\xFF",
            ),
            (VALUE, b"[--1\x80 2 endobj]"),
            (
                INDIRECT,
                b"1 0 obj << /Length 2 >>\x80stream\nab\x80endstream\x80endobj",
            ),
            (INDIRECT, b"1 0 obj\x80[1 0\x80R]\xFFendobj"),
            (INDIRECT, b"1\x800 obj [1] endobj"),
            (WINDOW, b"[1\x80 2]"),
            (VALUE, &reals),
        ];
        for (how, input) in seeds {
            parse(&[how, input].concat());
        }
    }

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
            for _ in 0..random(12) + 1 {
                input.extend_from_slice(FRAGMENTS[random(FRAGMENTS.len() - 1)]);
            }
            read += every_window(&input, 3);
            cuts += 12 * (input.len() + 1);
        }
        assert!(read > cuts / 4, "{read} of {cuts} read");
    }
}
