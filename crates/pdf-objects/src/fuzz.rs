//! What the fuzz target checks, which a unit test checks too: the promises
//! the parser makes, on any input, and the writer's on what it read. It's
//! compiled only for tests and under cargo-fuzz's `--cfg fuzzing`.

use pdf_tokenizer::{Kind, Tokenizer};

use crate::object::{Data, Dict, Form, Object, ObjectId, Span, Str, Stream, Value};
use crate::parse::{Body, Error, Fault, Limits, Parser, Sink};
use crate::serialize;

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
/// What's read, the writer writes, and it reads back as the same value and
/// writes as the same bytes again ([`round_trip`]), but for a ciphertext
/// read's object holding a string: the writer refuses it whole, writing
/// nothing, and writes it with its strings taken as their bytes, which
/// read back as plain.
/// Panics on a broken promise.
pub fn parse(data: &[u8]) {
    let (how, input) = chosen(data);
    let mut kept = Kept::default();
    let first = read(input, how, &mut kept);
    let mut again = Kept::default();
    assert_eq!(first, read(input, how, &mut again), "the same twice");
    assert_eq!(kept, again, "said the same twice");
    assert_eq!(first, read(input, how, &mut ()), "the same through ()");
    if how.limits != Limits::DEFAULT {
        limited(input, how, &first);
    }
    let (result, offset) = first;
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
                    let mut parser = Parser::at(input, how.offset);
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
}

/// How `data`'s first five bytes say to read the rest, and the rest: with
/// flag 0x20 and two bytes after the five, small limits, from those two.
fn chosen(data: &[u8]) -> (How, &[u8]) {
    let [a, b, flags, c, d, rest @ ..] = data else {
        let how = How {
            offset: 0,
            stream: None,
            ciphertext: false,
            indirect: false,
            length: Length::Given(0),
            astray: false,
            limits: Limits::DEFAULT,
        };
        return (how, data);
    };
    let (limits, input) = match rest {
        [nesting, values, input @ ..] if flags & 0x20 != 0 => {
            let limits = Limits {
                nesting: usize::from(nesting % 8),
                values: usize::from(values % 64),
            };
            (limits, input)
        }
        _ => (Limits::DEFAULT, rest),
    };
    let seed = usize::from(u16::from_le_bytes([*a, *b]));
    let offset = match flags & 0x40 {
        0 => seed % (input.len() + 1),
        _ => input.len() + seed,
    };
    let how = How {
        offset,
        stream: (flags & 1 != 0).then_some((u32::from(*c), 0)),
        ciphertext: flags & 2 != 0,
        indirect: flags & 4 != 0,
        length: match flags & 8 {
            0 => Length::Given(u16::from_le_bytes([*c, *d])),
            _ => Length::Scanned(i16::from_le_bytes([*c, *d])),
        },
        astray: flags & 0x10 != 0,
        limits,
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
    let there = Parser::at(input, at).value(&mut ());
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
    1 + match o.value() {
        Value::Array(a) => a.iter().map(count).sum(),
        Value::Dict(d) | Value::Stream(Stream { dict: d, .. }) => {
            d.entries().iter().map(|e| 1 + count(&e.value)).sum()
        }
        _ => 0,
    }
}

/// How deep `o`'s deepest value is nested in it: 0 for `o` itself, one
/// more for each array or dictionary around it, a stream's dictionary as
/// one.
fn deepest(o: &Object) -> usize {
    let inside = match o.value() {
        Value::Array(a) => a.iter().map(deepest).max(),
        Value::Dict(d) | Value::Stream(Stream { dict: d, .. }) => {
            d.entries().iter().map(|e| deepest(&e.value)).max()
        }
        _ => None,
    };
    inside.map_or(0, |d| d + 1)
}

/// What reading `input` as `how` says: the object or the error, and where
/// the parser ended.
fn read<'a>(input: &'a [u8], how: How, sink: &mut impl Sink<'a>) -> (Result<Object, Error>, usize) {
    let mut parser = (Parser::at(input, how.offset))
        .ciphertext(how.ciphertext)
        .limits(how.limits);
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
            assert!(keyword.end <= start && start <= input.len());
            let scanned = head.scanned();
            assert_eq!((scanned.stream, scanned.start), (how.stream, start));
            let marker = &b"endstream"[..];
            let data = &input[scanned.start..scanned.end];
            assert!(!data.windows(marker.len()).any(|w| w == marker));
            assert!(scanned.end == input.len() || input[scanned.end..].starts_with(marker));
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
                let after = Tokenizer::at(input, declared.end)
                    .find(|t| !matches!(t.kind, Kind::Space | Kind::Comment));
                assert!(after.is_some_and(|t| t.bytes == marker));
            }
            let data = declared.unwrap_or(scanned);
            if how.astray {
                let astray = Span::new(how.stream, data.start + 1, data.end);
                let refused = parser.clone().stream(head.clone(), astray, &mut ());
                assert!(matches!(refused, Err(Error::Extent { .. })), "{refused:?}");
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

/// Takes every place out of `o`, its values' spans and its keys', and
/// makes a stream's data the bytes of `data` it was read from, so that a
/// value read compares with one built.
pub fn unplace(o: &mut Object, data: &[u8]) {
    match o.value_mut() {
        Value::Array(a) => a.iter_mut().for_each(|e| unplace(e, data)),
        Value::Dict(d) => unplace_entries(d, data),
        Value::Stream(s) => {
            if let Some(at) = s.data.at() {
                s.data = Data::Bytes(data[at.start..at.end].to_vec());
            }
            unplace_entries(&mut s.dict, data);
        }
        _ => {}
    }
}

/// [`unplace`] on each of `d`'s entries, its key's span too.
fn unplace_entries(d: &mut Dict, data: &[u8]) {
    for entry in d.entries_mut() {
        entry.key_at = None;
        unplace(&mut entry.value, data);
    }
}

/// Checks that `object`, read from `input` as `how` says, is written, and
/// reads back the same way under the same limits ([`round_trip`]). A
/// ciphertext read's object holding a string is refused whole, nothing
/// written, then written with its strings taken as their bytes, and read
/// back as plain.
fn rewrite(input: &[u8], how: How, object: &Object) {
    let id = match how.indirect {
        true => Some(Parser::at(input, how.offset).header().expect("its header")),
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
    round_trip(&object, id, input, how.stream, how.limits);
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
    let mut limits = limits;
    if object.as_stream().is_some_and(|s| !s.dict.has(b"Length")) {
        limits.values = limits.values.saturating_add(2);
        limits.nesting = limits.nesting.max(1);
    }
    let mut parser = Parser::at(&written, 0).limits(limits);
    if let Some(stream) = stream {
        parser = parser.in_stream(stream);
    }
    let text = String::from_utf8_lossy(&written);
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
    unplace(&mut back, &written);
    assert_eq!(back, want, "read back from {text:?}");
    assert_eq!(nul.0, nul_names(&want), "a fault but for a name's NUL");
    let mut again = Vec::new();
    write(&mut again, &back, id, &[]).expect("what's read back writes");
    assert_eq!(again, written, "written again, the same bytes");
    written
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
    match (id, object.value()) {
        (
            Some(id),
            Value::Stream(Stream {
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
    match o.as_written_mut() {
        Value::String(s, _) => {
            if let Str::Encrypted(bytes) = s {
                *s = Str::Bytes(std::mem::take(bytes));
            }
            1
        }
        Value::Array(a) => a.iter_mut().map(plain).sum(),
        Value::Dict(d) | Value::Stream(Stream { dict: d, .. }) => {
            d.entries_mut().map(|e| plain(&mut e.value)).sum()
        }
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
        Value::Dict(d) | Value::Stream(Stream { dict: d, .. }) => entries(d),
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
    use super::parse;

    /// Inputs made of the fragments of objects, after five bytes that
    /// choose how they're read (and two that give small limits, when they
    /// choose those), run through the fuzz target's body.
    #[test]
    fn random_fragments_keep_every_promise() {
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
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut random = move |below: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % below as u64) as usize
        };
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
}
