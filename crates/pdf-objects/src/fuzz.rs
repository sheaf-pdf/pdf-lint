//! What the fuzz target checks, which a unit test checks too: the promises
//! the parser makes, on any input. It's compiled only for tests and under
//! cargo-fuzz's `--cfg fuzzing`.

use pdf_tokenizer::{Kind, Tokenizer};

use crate::object::{Dict, Object, ObjectId, Span, Str, Value};
use crate::parse::{Body, Error, Fault, Parser, Sink};

/// What a sink was told: each fault, where it is and what it says, and
/// where each hex string is.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Kept {
    /// Each fault's place and text, in the order said.
    pub faults: Vec<(Span, String)>,
    /// Each hex string's place, in the order read.
    pub hex: Vec<Span>,
}

impl Sink for Kept {
    fn fault(&mut self, at: Span, fault: Fault<'_>) {
        self.faults.push((at, fault.to_string()));
    }

    fn hex(&mut self, at: Span) {
        self.hex.push(at);
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
/// give, when it holds, else up to `endstream`. Read twice, and through
/// `()`, it's the same each time; every span and fault is inside the input
/// and inside what holds it; strings are ciphertext exactly when asked.
/// Panics on a broken promise.
pub fn parse(data: &[u8]) {
    let (how, input) = chosen(data);
    let mut kept = Kept::default();
    let first = read(input, how, &mut kept);
    let mut again = Kept::default();
    assert_eq!(first, read(input, how, &mut again), "the same twice");
    assert_eq!(kept, again, "said the same twice");
    assert_eq!(first, read(input, how, &mut ()), "the same through ()");
    let (result, offset) = first;
    assert!(offset <= input.len().max(how.offset), "{offset}");
    let inside = |at: Span| at.stream == how.stream && at.start <= at.end && at.end <= input.len();
    for &(at, _) in &kept.faults {
        assert!(inside(at), "a fault in the input: {at:?}");
    }
    for &at in &kept.hex {
        assert!(inside(at), "a hex string in the input: {at:?}");
        assert_eq!(input[at.start], b'<');
    }
    match result {
        Ok(object) => {
            let whole = object.at.expect("placed");
            for &(at, _) in &kept.faults {
                assert!(within(at, whole), "a fault in its object: {at:?}");
            }
            let mut hex = 0;
            placed(input, &object, whole, how, &mut hex);
            assert_eq!(hex, kept.hex.len(), "each hex string said once");
        }
        Err(error) => {
            assert!(!error.to_string().is_empty());
            if let Error::Header { at } = error {
                assert!(how.indirect);
                assert_eq!(at, how.offset, "the header looked for where it was given");
            }
        }
    }
}

/// How `data`'s first five bytes say to read the rest, and the rest.
fn chosen(data: &[u8]) -> (How, &[u8]) {
    let [a, b, flags, c, d, input @ ..] = data else {
        let how = How {
            offset: 0,
            stream: None,
            ciphertext: false,
            indirect: false,
            length: Length::Given(0),
            astray: false,
        };
        return (how, data);
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
    };
    (how, input)
}

/// What reading `input` as `how` says: the object or the error, and where
/// the parser ended.
fn read(input: &[u8], how: How, sink: &mut impl Sink) -> (Result<Object, Error>, usize) {
    let mut parser = Parser::at(input, how.offset).ciphertext(how.ciphertext);
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
            let Value::Stream(stream) = &object.value else {
                panic!("a stream reads as one");
            };
            assert_eq!(stream.data, Some(data));
            Ok(object)
        }
    });
    (object, parser.offset())
}

/// Whether `inner` is inside `outer`.
fn within(inner: Span, outer: Span) -> bool {
    inner.stream == outer.stream && outer.start <= inner.start && inner.end <= outer.end
}

/// Checks that `o` and every value in it are placed in `input` as `how`
/// reads it, each inside what holds it, `outer`, and each string is
/// ciphertext exactly when asked; counts the hex strings into `hex`.
fn placed(input: &[u8], o: &Object, outer: Span, how: How, hex: &mut usize) {
    let at = o.at.expect("every value placed");
    assert_eq!(at.stream, how.stream);
    assert!(at.start < at.end && at.end <= input.len(), "{at:?}");
    assert!(within(at, outer), "{at:?} in {outer:?}");
    let dict = match &o.value {
        Value::String(s) => {
            assert_eq!(matches!(s, Str::Encrypted(_)), how.ciphertext, "{s:?}");
            assert!(!matches!(s, Str::Text(_)));
            if input[at.start] == b'<' {
                *hex += 1;
            }
            None
        }
        Value::Array(a) => {
            a.iter().for_each(|e| placed(input, e, at, how, hex));
            None
        }
        Value::Dict(d) => Some(d),
        Value::Stream(s) => {
            let data = s.data.expect("a stream's data placed");
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
        placed(input, &entry.value, at, how, hex);
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    /// Inputs made of the fragments of objects, after five bytes that
    /// choose how they're read, run through the fuzz target's body.
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
