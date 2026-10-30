//! Tokens that tile what they're read from, inline images by each of
//! PDFium's rules, errors that say where, the caps, operands read as
//! objects, a page's streams read as one and joined under the cap, and
//! operations: the tokens grouped, every operand kept, taken from the
//! last.

use std::borrow::Cow;
use std::ops::Range;

use pdf_objects::{Form, Object, Str, Value, dict};

use crate::fuzz::blank;
use crate::*;

/// Numbers below what each call is given, the same each run.
pub(crate) struct Random(pub(crate) u64);

impl Random {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub(crate) fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n.max(1) as u64).unwrap_or(0)
    }
}

fn read<'a>(s: &'a [u8], res: &ResCx, lim: &Limits) -> Vec<(Range<usize>, Tok<'a>)> {
    tokens(s, res, lim).map(Result::unwrap).collect()
}

/// `s`'s tokens under the default caps, with no resources.
fn toks(s: &[u8]) -> Vec<Tok<'_>> {
    read(s, &ResCx::none(), &Limits::DEFAULT)
        .into_iter()
        .map(|(_, t)| t)
        .collect()
}

fn error(s: &[u8], lim: &Limits) -> Error {
    tokens(s, &ResCx::none(), lim)
        .find_map(Result::err)
        .expect("an error")
}

/// Read until the end or an error, `s`'s tokens are in order, each
/// within it, and only white-space and comments are between them; to the
/// end, after the last one too. Reading stops after an error.
fn assert_tiles(s: &[u8], res: &ResCx, lim: &Limits) {
    let mut at = 0;
    let mut lexed = tokens(s, res, lim);
    for token in lexed.by_ref() {
        let Ok((r, _)) = token else {
            assert!(lexed.next().is_none(), "nothing after an error");
            return;
        };
        assert!(
            at <= r.start && r.start < r.end && r.end <= s.len(),
            "{r:?} after {at}"
        );
        assert!(
            blank(&s[at..r.start]),
            "{:?}",
            String::from_utf8_lossy(&s[at..r.start])
        );
        at = r.end;
    }
    assert!(blank(&s[at..]), "{:?}", String::from_utf8_lossy(&s[at..]));
}

const PAGE: &[u8] = b"%!comment (not a string\n\
    q 0.5 0 0 -1.25 +.5 72. cm /Artifact <</Type /Pagination /Attached [/Top]>> BDC\n\
    BT /F#311 12 Tf [(a\\)(b) c) -250 <4142 43>] TJ (\\\\)Tj ET EMC\n\
    [1 [2 [3]] <</A <</B (c)>>>>] true false null Q { } ) ] >> > %end";

#[test]
fn tokens_are_as_written() {
    use Tok::{Op, Operand};
    assert_eq!(
        toks(PAGE),
        vec![
            Op(b"q"),
            Operand(b"0.5"),
            Operand(b"0"),
            Operand(b"0"),
            Operand(b"-1.25"),
            Operand(b"+.5"),
            Operand(b"72."),
            Op(b"cm"),
            Operand(b"/Artifact"),
            Operand(b"<</Type /Pagination /Attached [/Top]>>"),
            Op(b"BDC"),
            Op(b"BT"),
            Operand(b"/F#311"),
            Operand(b"12"),
            Op(b"Tf"),
            Operand(b"[(a\\)(b) c) -250 <4142 43>]"),
            Op(b"TJ"),
            Operand(b"(\\\\)"),
            Op(b"Tj"),
            Op(b"ET"),
            Op(b"EMC"),
            Operand(b"[1 [2 [3]] <</A <</B (c)>>>>]"),
            Operand(b"true"),
            Operand(b"false"),
            Operand(b"null"),
            Op(b"Q"),
            Operand(b"{"),
            Operand(b"}"),
            Operand(b")"),
            Operand(b"]"),
            Operand(b">>"),
            Operand(b">"),
        ]
    );
    assert_tiles(PAGE, &ResCx::none(), &Limits::DEFAULT);
}

#[test]
fn names_decode_their_escapes() {
    assert_eq!(name(b"/F#311").as_deref(), Some(&b"F11"[..]));
    assert_eq!(name(b"/A#20B").as_deref(), Some(&b"A B"[..]));
    assert!(matches!(name(b"/Plain"), Some(Cow::Borrowed(b"Plain"))));
    // An escape with no two hex digits stays as written.
    assert_eq!(name(b"/A#2").as_deref(), Some(&b"A#2"[..]));
    assert_eq!(name(b"/#zz").as_deref(), Some(&b"#zz"[..]));
    assert_eq!(name(b"/").as_deref(), Some(&b""[..]));
    assert_eq!(name(b"(F1)"), None);
}

/// A string of `bytes` in `form`, as an operand reads.
fn string(bytes: &[u8], form: Form) -> Object {
    Object::new(Value::String(Str::Bytes(bytes.to_vec()), Some(form)))
}

#[test]
fn operands_read_as_objects() {
    use Form::{Hex, Literal};
    let o = |s: &[u8]| object(s, 8);
    assert_eq!(o(b"12"), Some(Object::from(12)));
    assert_eq!(o(b"-.5"), Some(Object::from(-0.5)));
    assert_eq!(o(b"72."), Some(Object::from(72.0)));
    assert_eq!(o(b"true"), Some(Object::from(true)));
    assert_eq!(o(b"null"), Some(Object::null()));
    assert_eq!(o(b"/A#20B"), Some(Object::from("A B")));
    // Escapes, octal codes, a line continued, and an end of line.
    assert_eq!(
        o(b"(a\\(b\\)\\101\\0533\\\nc\r\nd)"),
        Some(string(b"a(b)A+3c\nd", Literal))
    );
    assert_eq!(o(b"<48 65 6>"), Some(string(b"He`", Hex)));
    assert_eq!(
        o(b"[1 /N (s) [2]]"),
        Some(Object::from(vec![
            1.into(),
            Object::from("N"),
            string(b"s", Literal),
            Object::from(vec![2.into()]),
        ]))
    );
    // A key that isn't a name is passed over.
    assert_eq!(
        o(b"<</A 1 2 /B /C>>"),
        Some(Object::from(dict! { "A" => 1, "B" => "C" }))
    );
    for not in [&b"--5"[..], b")", b"Tj", b"1.2.3"] {
        assert_eq!(o(not), None, "{}", String::from_utf8_lossy(not));
    }
    // Stray closers inside are no elements; one past `depth`, or one that
    // doesn't end, is no object.
    assert_eq!(
        o(b"[1 >> <</A 2 ]>>]"),
        Some(Object::from(vec![
            1.into(),
            Object::from(dict! { "A" => 2 }),
        ]))
    );
    assert_eq!(object(b"[[[1]]]", 2), None);
    assert!(object(b"[[1]]", 2).is_some());
    assert_eq!(o(b"[1 (2"), None);
}

/// An operand nested 500 deep around 8 MiB is read in one pass, not once
/// for each level it's inside.
#[test]
fn deep_operands_read_in_one_pass() {
    let mut s = b"[".repeat(500);
    s.push(b'<');
    s.resize(s.len() + (8 << 20), b'0');
    s.push(b'>');
    s.extend(b"]".repeat(500));
    let started = std::time::Instant::now();
    let mut o = object(&s, Limits::DEFAULT.depth).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    for _ in 0..500 {
        let Value::Array(mut inner) = o.into_value() else {
            panic!("an array")
        };
        o = inner.pop().unwrap();
    }
    assert!(
        matches!(o.into_value(), Value::String(Str::Bytes(bytes), _) if bytes.len() == 4 << 20)
    );
}

/// `s`'s one inline image: where the token is, and its data.
fn image<'a>(s: &'a [u8], res: &ResCx) -> (Range<usize>, &'a [u8]) {
    read(s, res, &Limits::DEFAULT)
        .into_iter()
        .find_map(|(r, t)| match t {
            Tok::InlineImage(data) => Some((r, &s[data])),
            _ => None,
        })
        .expect("an inline image")
}

/// An inline image with `dict`, `data` and `EI`, between `q` and `Q`:
/// `data` is what's found, and `Q` follows it.
fn assert_image(dict: &str, data: &[u8], res: &ResCx) {
    let mut s = format!("q BI {dict} ID ").into_bytes();
    s.extend_from_slice(data);
    s.extend_from_slice(b"\nEI Q");
    let (r, found) = image(&s, res);
    assert_eq!(found, data, "{dict}");
    assert_eq!(&s[r.clone()][..2], b"BI");
    assert_eq!(&s[r.end - 2..r.end], b"EI");
    let ops: Vec<Tok> = read(&s, res, &Limits::DEFAULT)
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert_eq!(ops.len(), 3, "{dict}");
    assert_eq!(ops[2], Tok::Op(b"Q"), "{dict}");
    assert_tiles(&s, res, &Limits::DEFAULT);
}

#[test]
fn unfiltered_data_is_its_exact_size() {
    let none = ResCx::none();
    // Data with ` EI ` in it: 4 bytes of gray.
    assert_image("/W 4 /H 1 /BPC 8 /CS /G", b" EI ", &none);
    assert_image(
        "/Width 2 /Height 2 /BitsPerComponent 8 /ColorSpace /DeviceGray",
        b"a EI",
        &none,
    );
    // RGB and CMYK.
    assert_image("/W 2 /H 1 /BPC 8 /CS /RGB", b" EI EI", &none);
    assert_image("/W 1 /H 1 /BPC 8 /CS /DeviceCMYK", b" EI ", &none);
    // Rows padded to a byte: 3 one-bit samples are a byte a row.
    assert_image("/W 3 /H 2 /BPC 1 /CS /G", b"EI", &none);
    // 16 bits a sample.
    assert_image("/W 1 /H 1 /BPC 16 /CS /G", b"EI", &none);
    // An image mask has no color space: a bit a sample.
    assert_image("/IM true /W 10 /H 2 /D [1 0]", b" EI ", &none);
    // An indexed image: one component, whatever its base.
    assert_image(
        "/W 2 /H 2 /BPC 8 /CS [/I /RGB 1 <000000FFFFFF>] /I true",
        b"EI E",
        &none,
    );
    assert_image(
        "/W 2 /H 1 /BPC 8 /CS [/DeviceN [/Cyan /Spot] /DeviceCMYK {}]",
        b" EI ",
        &none,
    );
    // A name the resources have, by its components; one they don't is
    // none, as in PDFium: one bit of one component.
    let res = ResCx::new([(b"CS0".to_vec(), 3), (b"Sp#1".to_vec(), 1)]);
    assert_image("/W 2 /H 1 /BPC 8 /CS /CS0", b" EI EI", &res);
    assert_image("/W 2 /H 1 /BPC 8 /CS /Sp#231", b"EI", &res);
    assert_image("/W 16 /H 1 /BPC 8 /CS /Absent", b"EI", &res);
    // Reals are their whole part.
    assert_image("/W 4.9 /H 1.0 /BPC 8 /CS /G", b" EI ", &none);
}

#[test]
fn filtered_data_ends_at_its_marker() {
    let none = ResCx::none();
    assert_image("/F /AHx /W 2 /H 2 /BPC 8 /CS /G", b"0E EI 0>", &none);
    assert_image(
        "/Filter /ASCIIHexDecode /W 2 /H 2 /BPC 8 /CS /G",
        b"0E EI 0>",
        &none,
    );
    assert_image(
        "/F [/A85 /Fl] /W 9 /H 9 /BPC 8 /CS /G",
        b"9jqo^ EI ~>",
        &none,
    );
    // RunLength: a run of 4 bytes, ` EI `, then its end.
    assert_image("/F /RL /W 4 /H 1 /BPC 8 /CS /G", b"\x03 EI \x80", &none);
    assert_image(
        "/F /DCT /W 9 /H 9 /BPC 8 /CS /RGB",
        b"\xff\xd8\xff\xe0 EI \x00\xff\xd9",
        &none,
    );
    // PDF 2.0's length, abbreviated or not.
    assert_image("/F /Fl /W 9 /H 9 /BPC 8 /CS /G /L 6", b"x EI y", &none);
    assert_image("/F /LZW /Length 6 /W 9 /H 9", b"x EI y", &none);
    // With nothing to go by, the first `EI` between white-space.
    assert_image(
        "/F /CCF /DP <</K -1 /Columns 9>> /W 9 /H 9",
        b"xEI EIy",
        &none,
    );
    assert_image("/F /Fl /W 9 /H 9 /BPC 8 /CS /G", b"", &none);
}

#[test]
fn an_image_ends_at_the_next_ei_past_its_data() {
    // Past the counted data, PDFium reads tokens to the next `EI`.
    let s = b"BI /W 1 /H 1 /BPC 8 /CS /G ID x 1 2 % EI\nEI Q";
    let (r, data) = image(s, &ResCx::none());
    assert_eq!(data, b"x");
    assert_eq!(&s[r.end..], b" Q");
    // And `EI` straight after the data.
    let s = b"BI /W 1 /H 1 /BPC 8 /CS /G ID xEI Q";
    assert_eq!(image(s, &ResCx::none()).1, b"x");
    // A key that isn't a name is passed over.
    let s = b"BI 7 /W 1 /H 1 /BPC 8 /CS /G ID x EI";
    assert_eq!(image(s, &ResCx::none()).1, b"x");
}

#[test]
fn errors_say_where() {
    let lim = &Limits::DEFAULT;
    let at = |s: &[u8]| {
        let e = error(s, lim);
        (e.at, e.kind)
    };
    use ErrorKind::*;
    assert_eq!(at(b"0 0 m (a(b)c"), (6, String));
    assert_eq!(at(b"q <414"), (2, HexString));
    assert_eq!(at(b"[1 2 ("), (5, String));
    assert_eq!(at(b"[1 [2]"), (0, Array));
    assert_eq!(at(b"/P << /A [1] >"), (3, Dict));
    // A `]` doesn't close a dictionary: it's passed over.
    assert_eq!(at(b"/P [ << /A 1 ] "), (5, Dict));
    assert_eq!(at(b"q BI /W 1 /H 1"), (2, InlineImage));
    assert_eq!(at(b"q BI /W 1 ID abc"), (2, InlineImage));
    assert_eq!(
        at(b"BI /W 9 /H 9 /BPC 8 /CS /G ID abc EI"),
        (0, InlineImage)
    );
    assert_eq!(at(b"Q BI /F /AHx ID 0E EI"), (2, InlineImage));
    assert_eq!(at(b"BI /F /A85 ID 9jqo EI"), (0, InlineImage));
    assert_eq!(at(b"BI /F /RL ID \x05abc EI"), (0, InlineImage));
    // The tokens before an error come first.
    let lexed: Vec<_> = tokens(b"q (a", &ResCx::none(), lim).collect();
    assert_eq!(lexed.len(), 2);
    assert_eq!(lexed[0], Ok((0..1, Tok::Op(b"q"))));
    assert!(lexed[1].is_err());
}

#[test]
fn the_caps_hold() {
    let depth = Limits::DEFAULT.depth;
    let nested = |n: usize| [b"[".repeat(n), b"]".repeat(n)].concat();
    assert_eq!(toks(&nested(depth)).len(), 1);
    let e = error(&nested(depth + 1), &Limits::DEFAULT);
    assert_eq!((e.at, e.kind), (depth, ErrorKind::Depth));
    let dicts = [b"<<".repeat(depth + 1), b">>".repeat(depth + 1)].concat();
    assert_eq!(error(&dicts, &Limits::DEFAULT).kind, ErrorKind::Depth);

    let three = Limits {
        operands: 3,
        ..Limits::DEFAULT
    };
    assert_eq!(read(b"1 2 3 m 4 5 6 l", &ResCx::none(), &three).len(), 8);
    let e = error(b"1 2 3 4 m", &three);
    assert_eq!((e.at, e.kind), (6, ErrorKind::Operands));

    let small = Limits {
        inline_image: 3,
        ..Limits::DEFAULT
    };
    let s = b"q BI /W 3 /H 1 /BPC 8 /CS /G ID abc EI BI /W 4 /H 1 /BPC 8 /CS /G ID abcd EI";
    let e = error(s, &small);
    assert_eq!((e.at, e.kind), (39, ErrorKind::InlineImageSize));
}

/// Content of many inline images that don't end is read once: the end
/// of the first is searched for, not each one's.
#[test]
fn unended_images_dont_search_again() {
    let s = b"BI /F /DCT ID x EI ".repeat(50_000);
    let started = std::time::Instant::now();
    assert_eq!(tokens(&s, &ResCx::none(), &Limits::DEFAULT).count(), 50_000);
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

/// What content is made of, run together into tokens.
const PIECES: &[&[u8]] = &[
    b" ",
    b"\n",
    b"0",
    b"-1.5",
    b"/N",
    b"/W 2 /H 1 /BPC 8 /CS /G",
    b"/F /AHx",
    b"/F /DCT",
    b"/L 3",
    b"(",
    b")",
    b"\\",
    b"<",
    b">",
    b"<<",
    b">>",
    b"[",
    b"]",
    b"{",
    b"%",
    b"BI",
    b"ID",
    b"EI",
    b"~>",
    b"\xff\xd9",
    b"\x80",
    b"Tj",
    b"q",
    b"Q",
];

/// Whatever the bytes, the tokens tile them, up to an error: 512 runs of
/// up to 47 pieces.
#[test]
fn tokens_tile_any_content() {
    let mut random = Random(0x853c_49e6_748f_ea9b);
    let res = ResCx::new([(b"N".to_vec(), 3)]);
    for _ in 0..512 {
        let pieces = random.below(48);
        let s: Vec<u8> = (0..pieces)
            .flat_map(|_| PIECES[random.below(PIECES.len())])
            .copied()
            .collect();
        assert_tiles(&s, &res, &Limits::DEFAULT);
    }
}

#[test]
fn streams_lex_as_one() {
    let first: &[u8] = b"q /P <</MCID 0>> BDC (ab";
    let second: &[u8] = b"c) Tj";
    let third: &[u8] = b"EMC Q";
    let c = Concat::new(&[first, second, third]);
    assert_eq!(c.bytes(), b"q /P <</MCID 0>> BDC (ab c) Tj EMC Q");
    assert_eq!(c.stream_count(), 3);
    assert_eq!(c.stream(1), second);
    assert_eq!(c.range(2), 31..36);
    let lexed = read(c.bytes(), &ResCx::none(), &Limits::DEFAULT);
    // The string runs from the first stream into the second, as in PDFium.
    let (string, _) = &lexed[4];
    assert_eq!(
        c.span(string.clone()),
        SpanAcross {
            start: Pos { stream: 0, at: 21 },
            end: Pos { stream: 1, at: 2 },
        }
    );
    let (emc, _) = &lexed[6];
    assert_eq!(c.span(emc.clone()).start, Pos { stream: 2, at: 0 });
    // The space between is the end of the stream before it.
    assert_eq!(c.pos(24), Pos { stream: 0, at: 24 });
    assert_eq!(c.pos(36), Pos { stream: 2, at: 5 });
    assert_eq!(
        c.tokens(&ResCx::none(), &Limits::DEFAULT).count(),
        lexed.len()
    );
    // One stream is read where it is.
    let one = Concat::new(&[first]);
    assert!(std::ptr::eq(one.bytes(), first));
}

/// Streams joined under `content`, the cap: the three streams pushed and
/// the bytes they join to, or the error.
fn joined(content: usize, streams: &[&[u8]]) -> Result<Concat<'static>, TooLarge> {
    let mut join = Join::new(&Limits {
        content,
        ..Limits::DEFAULT
    });
    for stream in streams {
        join.push(stream)?;
    }
    Ok(join.finish())
}

#[test]
fn streams_past_the_cap_are_refused() {
    let streams = [&[b' '; 1000][..]; 3];
    // Three streams and the two spaces between.
    let content = joined(3002, &streams).unwrap();
    assert_eq!(content.bytes().len(), 3002);
    assert_eq!(content.bytes(), Concat::new(&streams).bytes());
    for cap in [3001, 2500, 999] {
        assert_eq!(joined(cap, &streams).unwrap_err(), TooLarge { cap });
    }
    // An empty stream past the cap is too, for its space.
    let four: [&[u8]; 4] = [streams[0], streams[1], streams[2], b""];
    assert!(joined(3002, &four).is_err());
    assert_eq!(
        TooLarge { cap: 999 }.to_string(),
        "its streams decode to more than 999 bytes"
    );
}

#[test]
fn an_error_names_its_stream() {
    let mut join = Join::new(&Limits::DEFAULT);
    join.push(b"q").unwrap();
    join.push(b"0 g (open").unwrap();
    let content = join.finish();
    let e = content
        .tokens(&ResCx::none(), &Limits::DEFAULT)
        .find_map(Result::err)
        .unwrap();
    assert_eq!(e.at, 6);
    let pos = content.pos(e.at);
    assert_eq!(pos, Pos { stream: 1, at: 4 });
    let in_stream = Error {
        at: pos.at as usize,
        ..e
    };
    assert_eq!(
        format!("{}: {in_stream}", pos.stream),
        "1: a string that doesn't end at byte 4"
    );
    assert_eq!(pos.to_string(), "stream 1 byte 4");
}

/// Each stream pushed is the next, an empty one too, its space before it
/// kept: a place's stream is the index of the stream given.
#[test]
fn every_stream_pushed_is_one() {
    let mut join = Join::new(&Limits::DEFAULT);
    join.push(b"q").unwrap();
    join.push(b"").unwrap();
    join.push_with(|_, _| Ok::<_, ()>(())).unwrap();
    join.push(b"Q").unwrap();
    let content = join.finish();
    assert_eq!(content.bytes(), b"q   Q");
    assert_eq!(content.stream_count(), 4);
    assert_eq!(
        (0..4).map(|i| content.range(i)).collect::<Vec<_>>(),
        [0..1, 2..2, 3..3, 4..5]
    );
    assert_eq!(content.pos(4), Pos { stream: 3, at: 0 });
    assert_eq!(content.pos(3), Pos { stream: 2, at: 0 });
    assert_eq!(
        content.bytes(),
        Concat::new(&[&b"q"[..], b"", b"", b"Q"]).bytes()
    );
    // None at all.
    let none = Join::new(&Limits::DEFAULT).finish();
    assert_eq!((none.bytes(), none.stream_count()), (&b""[..], 0));
}

/// A decoder appends to the joined bytes, given the room after the space
/// before it; what it can't decode, or decodes past its room, is cut back,
/// the space too, and no stream.
#[test]
fn a_stream_decoded_into_the_join() {
    let mut join = Join::new(&Limits {
        content: 20,
        ..Limits::DEFAULT
    });
    assert_eq!(join.room(), 20);
    join.push(b"q 0 g").unwrap();
    assert_eq!(join.room(), 14);
    let mut given = None;
    join.push_with(|out, room| {
        given = Some((out.len(), room));
        out.extend_from_slice(b"1 g");
        Ok::<_, &str>(())
    })
    .unwrap();
    // Given the bytes so far and the space, and the room after them.
    assert_eq!(given, Some((6, 14)));
    assert_eq!(join.room(), 10);
    // A decoder that fails, having written some.
    let failed = join.push_with(|out, _| {
        out.extend_from_slice(b"0 0 m");
        Err("not flate")
    });
    assert_eq!(failed, Err(Pushed::Decode("not flate")));
    assert_eq!(failed.unwrap_err().to_string(), "not flate");
    assert_eq!(join.room(), 10);
    // One that writes past its room.
    let over = join.push_with(|out, room| {
        out.resize(out.len() + room + 1, b' ');
        Ok::<_, &str>(())
    });
    assert_eq!(over, Err(Pushed::TooLarge(TooLarge { cap: 20 })));
    // One that fills it.
    join.push_with(|out, room| {
        out.resize(out.len() + room, b'Q');
        Ok::<_, &str>(())
    })
    .unwrap();
    assert_eq!(join.room(), 0);
    // Past the cap, for the space alone.
    let past = join.push_with(|_, _| Ok::<_, &str>(()));
    assert_eq!(past, Err(Pushed::TooLarge(TooLarge { cap: 20 })));
    let content = join.finish();
    assert_eq!(content.bytes(), b"q 0 g 1 g QQQQQQQQQQ");
    assert_eq!(content.stream_count(), 3);
    assert_eq!(content.stream(1), b"1 g");
}

#[test]
fn the_types_are_send_and_sync() {
    fn both<T: Send + Sync>() {}
    both::<Tokens>();
    both::<Tok>();
    both::<ResCx>();
    both::<Concat>();
    both::<Join>();
    both::<Error>();
    both::<Pushed<std::io::Error>>();
    both::<Operations>();
    both::<Operation>();
    both::<Operand>();
    both::<Elements>();
    both::<InlineImage>();
}

/// `s`'s operations under `lim`, until the end or an error.
fn operations_of<'a>(s: &'a [u8], res: &ResCx, lim: &Limits) -> Vec<Result<Operation<'a>, Error>> {
    operations(s, res, lim).collect()
}

/// `s`'s operations under the default caps, with no resources: each
/// one's name and its operands' bytes.
fn ops(s: &[u8]) -> Vec<(&[u8], Vec<&[u8]>)> {
    operations_of(s, &ResCx::none(), &Limits::DEFAULT)
        .into_iter()
        .map(|op| {
            let op = op.unwrap();
            (op.name(), op.operands().map(|o| o.bytes()).collect())
        })
        .collect()
}

/// The operations of `s` are its tokens grouped: each operator or inline
/// image where its token is, with the operands since the last, each where
/// its token is, in order; operands at the end are none's.
fn assert_grouped(s: &[u8], res: &ResCx, lim: &Limits) {
    let mut read = operations(s, res, lim);
    let mut operands = Vec::new();
    for token in tokens(s, res, lim) {
        let (r, tok) = match token {
            Ok(token) => token,
            Err(e) => {
                assert_eq!(read.next().unwrap().unwrap_err(), e);
                break;
            }
        };
        let image = match tok {
            Tok::Operand(_) => {
                operands.push(r);
                continue;
            }
            Tok::Op(_) => None,
            Tok::InlineImage(data) => Some(data),
        };
        let op = read.next().unwrap().unwrap();
        assert_eq!(op.at(), r);
        assert_eq!(op.name(), &s[r.start..r.start + op.name().len()]);
        assert_eq!(op.inline_image().map(|i| i.data()), image);
        let ranges: Vec<_> = op.operands().map(|o| o.range()).collect();
        assert_eq!(ranges, operands);
        for o in op.operands() {
            assert_eq!(o.bytes(), &s[o.range()]);
        }
        let start = operands.first().map_or(r.start, |o| o.start);
        assert_eq!(op.range(), start..r.end);
        operands.clear();
    }
    assert!(read.next().is_none(), "nothing more");
}

#[test]
fn operations_are_the_tokens_grouped() {
    let two = |a, b| vec![a, b];
    assert_eq!(
        ops(PAGE),
        vec![
            (&b"q"[..], vec![]),
            (
                b"cm",
                vec![&b"0.5"[..], b"0", b"0", b"-1.25", b"+.5", b"72."]
            ),
            (
                b"BDC",
                two(&b"/Artifact"[..], b"<</Type /Pagination /Attached [/Top]>>")
            ),
            (b"BT", vec![]),
            (b"Tf", two(&b"/F#311"[..], b"12")),
            (b"TJ", vec![&b"[(a\\)(b) c) -250 <4142 43>]"[..]]),
            (b"Tj", vec![&b"(\\\\)"[..]]),
            (b"ET", vec![]),
            (b"EMC", vec![]),
            (
                b"Q",
                vec![
                    &b"[1 [2 [3]] <</A <</B (c)>>>>]"[..],
                    b"true",
                    b"false",
                    b"null"
                ]
            ),
        ]
    );
    assert_grouped(PAGE, &ResCx::none(), &Limits::DEFAULT);
    let tf = operations(PAGE, &ResCx::none(), &Limits::DEFAULT)
        .nth(4)
        .unwrap()
        .unwrap();
    assert_eq!((tf.at(), tf.range()), (117..119, 107..119));
    assert_eq!(tf.arg(1).map(|o| o.range()), Some(107..113));
    // Its Debug is the operation's, not the content's.
    assert_eq!(
        format!("{tf:?}"),
        r#"Operation { name: "Tf", at: 117..119, image: None, operands: [107..113, 114..116] }"#
    );
}

#[test]
fn args_are_taken_from_the_last() {
    let read = operations_of(b"1 2 3 4 5 6 7 l h", &ResCx::none(), &Limits::DEFAULT);
    let (l, h) = (read[0].as_ref().unwrap(), read[1].as_ref().unwrap());
    let arg = |i| l.arg(i).map(|o| o.bytes());
    assert_eq!(
        (arg(0), arg(1), arg(6)),
        (Some(&b"7"[..]), Some(&b"6"[..]), Some(&b"1"[..]))
    );
    for i in [7, 8, usize::MAX] {
        assert_eq!(l.arg(i), None, "{i}");
    }
    let from_the_last: Vec<_> = (0..7).map(|i| l.arg(i).unwrap()).collect();
    let reversed: Vec<_> = l.operands().rev().collect();
    assert_eq!(from_the_last, reversed);
    assert_eq!(l.operands().len(), 7);
    // None for an operator that has none.
    assert_eq!((h.arg(0), h.operands().len()), (None, 0));
}

/// `n` operands, each its index, and then `scn`.
fn numbered(n: usize) -> Vec<u8> {
    let mut s: Vec<u8> = (0..n).flat_map(|i| format!("{i} ").into_bytes()).collect();
    s.extend(b"scn");
    s
}

/// Every operand is kept, past the sixteen held in the item too, up to
/// the cap; past the cap is the error where the first past it starts.
#[test]
fn every_operand_is_kept_to_the_cap() {
    let cap = Limits::DEFAULT.operands;
    assert_eq!(cap, 65_536);
    for n in [0, 1, 15, 16, 17, 33, cap] {
        let s = numbered(n);
        let read = operations_of(&s, &ResCx::none(), &Limits::DEFAULT);
        assert_eq!(read.len(), 1, "{n}");
        let op = read[0].as_ref().unwrap();
        assert_eq!(op.operands().len(), n);
        assert_eq!(op.inline(), n <= 16, "{n} held in the item");
        for (i, o) in op.operands().enumerate() {
            assert_eq!(o.bytes(), i.to_string().as_bytes());
        }
        if n > 0 {
            assert_eq!(op.arg(0).unwrap().bytes(), (n - 1).to_string().as_bytes());
            assert_eq!(op.arg(n - 1).unwrap().bytes(), b"0");
        }
        assert_eq!(op.arg(n), None);
    }
    let mut s = b"q ".to_vec();
    s.extend(numbered(cap + 1));
    let read = operations_of(&s, &ResCx::none(), &Limits::DEFAULT);
    assert_eq!(read.len(), 2);
    assert_eq!(read[0].as_ref().unwrap().name(), b"q");
    let first_past = s.len() - b"65536 scn".len();
    assert_eq!(
        read[1].as_ref().unwrap_err(),
        &Error {
            at: first_past,
            kind: ErrorKind::Operands
        }
    );
}

#[test]
fn an_inline_image_is_one_operation() {
    let s = b"/Im 0 BI /W 4 /H 1 /BPC 8 /CS /RGB /C#53 /G /L 4 ID abcd EI Q";
    let read = operations_of(s, &ResCx::none(), &Limits::DEFAULT);
    assert_eq!(read.len(), 2);
    let op = read[0].as_ref().unwrap();
    assert_eq!((op.name(), op.at()), (&b"BI"[..], 6..59));
    // The operands before `BI` are its.
    let operands: Vec<_> = op.operands().map(|o| o.bytes()).collect();
    assert_eq!(operands, [&b"/Im"[..], b"0"]);
    assert_eq!(op.range(), 0..59);
    let image = op.inline_image().unwrap();
    assert_eq!((image.at(), image.data()), (6..59, 52..56));
    assert_eq!(&s[image.data()], b"abcd");
    assert_eq!(
        &s[image.dict()],
        b" /W 4 /H 1 /BPC 8 /CS /RGB /C#53 /G /L 4 "
    );
    // The last entry under any of the keys, its key's escapes decoded, as
    // its data was found by.
    let entry = |keys: &[&[u8]]| image.entry(keys).map(|o| o.bytes());
    assert_eq!(entry(&[b"CS", b"ColorSpace"]), Some(&b"/G"[..]));
    assert_eq!(entry(&[b"ColorSpace"]), None);
    assert_eq!(entry(&[b"W"]), Some(&b"4"[..]));
    let w = image.entry(&[b"W"]).unwrap();
    assert_eq!(&s[w.range()], b"4");
    assert_eq!(image.length(), Some(4));
    assert_eq!(
        read[1].as_ref().unwrap().inline_image().map(|i| i.at()),
        None
    );
    assert_eq!(
        format!("{image:?}"),
        "InlineImage { at: 6..59, dict: 8..49, data: 52..56 }"
    );

    // Data straight after `ID`'s white-space byte, or after `ID` where a
    // delimiter ends it; a length by either key, a real's whole part, or
    // none that isn't one.
    let image = |s: &'static [u8]| {
        let op = operations_of(s, &ResCx::none(), &Limits::DEFAULT)
            .remove(0)
            .unwrap();
        op.inline_image().unwrap()
    };
    let i = image(b"BI /F /AHx /Length 3.7 ID\n0E>EI");
    assert_eq!((i.dict(), i.length()), (2..23, Some(3)));
    let i = image(b"BI /F /AHx /L -3 ID 0E> EI");
    assert_eq!((i.dict(), i.length()), (2..17, None));
    let i = image(b"BI /F /AHx /L /Three ID 0E> EI");
    assert_eq!(i.length(), None);
    let i = image(b"BI/W 2/H 1/BPC 8/CS/G ID[)EI");
    assert_eq!((i.dict(), i.data()), (2..22, 24..26));
}

#[test]
fn operands_at_the_end_are_no_operation() {
    assert_eq!(ops(b"q 1 2"), [(&b"q"[..], vec![])]);
    assert_eq!(ops(b"1 2 3"), []);
    assert_eq!(ops(b""), []);
    assert_eq!(ops(b" % 1 2 Q"), []);
    // The same at the end of a page's streams.
    let page = Concat::new(&[b"q 1", b"0 0 RG Q 9"]);
    let read: Vec<_> = page
        .operations(&ResCx::none(), &Limits::DEFAULT)
        .map(|op| op.unwrap().name())
        .collect();
    assert_eq!(read, [&b"q"[..], b"RG", b"Q"]);
}

#[test]
fn an_error_ends_the_operations() {
    let lim = &Limits::DEFAULT;
    let read = operations_of(b"q 1 0 0 1 0 0 cm (open", &ResCx::none(), lim);
    let names: Vec<_> = read[..2]
        .iter()
        .map(|op| op.as_ref().unwrap().name())
        .collect();
    assert_eq!(names, [&b"q"[..], b"cm"]);
    assert_eq!(
        read[2].as_ref().unwrap_err(),
        &Error {
            at: 17,
            kind: ErrorKind::String
        }
    );
    assert_eq!(read.len(), 3);
    // The operands before an error are no operation's.
    let read = operations_of(b"q 1 2 [3 BT", &ResCx::none(), lim);
    assert_eq!(read.len(), 2);
    assert_eq!(
        read[1].as_ref().unwrap_err(),
        &Error {
            at: 6,
            kind: ErrorKind::Array
        }
    );
    // Nothing after it, asked again.
    let none = ResCx::none();
    let mut read = operations(b"BI /W 1 ID", &none, lim);
    assert_eq!(
        read.next().unwrap().unwrap_err().kind,
        ErrorKind::InlineImage
    );
    assert!(read.next().is_none() && read.next().is_none());
}

#[test]
fn a_pages_operations_map_back_to_its_streams() {
    let streams: [&[u8]; 3] = [
        b"q 1 0 0",
        b"1 0 0 cm /P <</MCID 0>> BDC (a",
        b"b) Tj EMC Q",
    ];
    let page = Concat::new(&streams);
    let read: Vec<_> = page
        .operations(&ResCx::none(), &Limits::DEFAULT)
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(read.len(), 6);
    let cm = &read[1];
    // From its first operand in the first stream to its operator in the
    // second.
    assert_eq!(
        page.span(cm.range()),
        SpanAcross {
            start: Pos { stream: 0, at: 2 },
            end: Pos { stream: 1, at: 8 },
        }
    );
    assert_eq!(page.pos(cm.at().start), Pos { stream: 1, at: 6 });
    let mcid = read[2].arg(0).unwrap();
    assert_eq!(page.span(mcid.range()).start, Pos { stream: 1, at: 12 });
    // The string runs from the second stream into the third.
    let shown = read[3].arg(0).unwrap();
    assert_eq!(
        page.span(shown.range()),
        SpanAcross {
            start: Pos { stream: 1, at: 28 },
            end: Pos { stream: 2, at: 2 },
        }
    );
    assert_eq!(page.pos(read[5].at().start), Pos { stream: 2, at: 10 });
    assert_grouped(page.bytes(), &ResCx::none(), &Limits::DEFAULT);
}

#[test]
fn operands_are_read_when_asked() {
    let s = b"/A#20B 12 -.5 (s) <41> <</K 1>> [1 [2 3] (x) >> /N] true null --5 m";
    let read = operations_of(s, &ResCx::none(), &Limits::DEFAULT);
    let op = read[0].as_ref().unwrap();
    let o: Vec<Operand> = op.operands().collect();
    let kinds = |o: &Operand| {
        [
            o.is_number(),
            o.is_name(),
            o.is_string(),
            o.is_array(),
            o.is_dict(),
        ]
    };
    let f = false;
    assert_eq!(kinds(&o[0]), [f, true, f, f, f]);
    assert_eq!(kinds(&o[1]), [true, f, f, f, f]);
    assert_eq!(kinds(&o[2]), [true, f, f, f, f]);
    assert_eq!(kinds(&o[3]), [f, f, true, f, f]);
    assert_eq!(kinds(&o[4]), [f, f, true, f, f]);
    assert_eq!(kinds(&o[5]), [f, f, f, f, true]);
    assert_eq!(kinds(&o[6]), [f, f, f, true, f]);
    assert_eq!(kinds(&o[7]), [f; 5]);
    assert_eq!(kinds(&o[8]), [f; 5]);
    // A number to PDFium, though it reads as none.
    assert_eq!(kinds(&o[9]), [true, f, f, f, f]);
    assert_eq!(o[9].object(8), None);
    assert_eq!(o[0].name().as_deref(), Some(&b"A B"[..]));
    assert_eq!(o[1].name(), None);
    assert_eq!(o[1].object(8), Some(Object::from(12)));
    assert_eq!(o[5].object(8), Some(Object::from(dict! { "K" => 1 })));
    assert_eq!(o[5].object(0), None);

    // An array's elements as written, each where it is: a stray closer
    // too, which isn't an element of the object it reads as.
    let array = o[6];
    let elements: Vec<Operand> = array.elements().collect();
    let bytes: Vec<_> = elements.iter().map(|e| e.bytes()).collect();
    assert_eq!(bytes, [&b"1"[..], b"[2 3]", b"(x)", b">>", b"/N"]);
    for e in &elements {
        assert_eq!(&s[e.range()], e.bytes());
    }
    assert_eq!(elements[1].range(), 35..40);
    let inner: Vec<_> = elements[1].elements().map(|e| e.range()).collect();
    assert_eq!(inner, [36..37, 38..39]);
    let read: Vec<_> = elements.iter().filter_map(|e| e.object(7)).collect();
    assert_eq!(array.object(8), Some(Object::from(read)));
    // None for what isn't an array, and none in an empty one.
    assert_eq!(o[5].elements().count(), 0);
    assert_eq!(o[1].elements().count(), 0);
    let empty = operations_of(b"[ %]\n] TJ", &ResCx::none(), &Limits::DEFAULT);
    assert_eq!(
        empty[0]
            .as_ref()
            .unwrap()
            .arg(0)
            .unwrap()
            .elements()
            .count(),
        0
    );
    assert_eq!(
        format!("{:?}", o[3]),
        r#"Operand { bytes: "(s)", range: 14..17 }"#
    );
}
