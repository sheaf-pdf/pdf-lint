//! Tokens that tile what they're read from, inline images by each of
//! PDFium's rules, errors that say where, the caps, operands read as
//! objects, a page's streams read as one and joined under the cap,
//! operations: the tokens grouped, every operand kept, taken from the
//! last; the faults; the operator table: each operator by its name, its
//! operands, category and clause as a table of the tests' own reads them,
//! and the graphics objects' states; and over the operations, a page's
//! `q`/`Q` balance, marked content and resource names, and their steppers
//! driven by hand.

use std::borrow::Cow;
use std::ops::Range;

use pdf_objects::{Form, Object, Str, Value, dict};

use crate::fault::{Fault, Sink};
use crate::fuzz::{Collected, Unwanted, blank, faulted, fixtures};
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
    // A `#` with fewer than two bytes after it stays as written; with two
    // that aren't hex digits, it escapes them as PDFium reads it, each
    // read as 0 (`/F#3G 10 Tf` finds the font `F0` in PDFium 7881 and
    // 8076).
    assert_eq!(name(b"/A#2").as_deref(), Some(&b"A#2"[..]));
    assert_eq!(name(b"/#zz").as_deref(), Some(&b"\0"[..]));
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
    let o = |s: &[u8]| object(s, &deep(8));
    assert_eq!(o(b"12"), Some(Object::from(12)));
    assert_eq!(o(b"-.5"), Some(Object::from(-0.5)));
    assert_eq!(o(b"72."), Some(Object::from(72.0)));
    assert_eq!(o(b"true"), Some(Object::from(true)));
    assert_eq!(o(b"null"), Some(Object::null()));
    assert_eq!(o(b"/A#20B"), Some(Object::from("A B")));
    // Escapes, octal codes, a line continued, and an end of line, a CR LF
    // read as written, as PDFium reads it (`(a\rb\r\nc) Tj` shows those
    // bytes in PDFium 7881 and 8076).
    assert_eq!(
        o(b"(a\\(b\\)\\101\\0533\\\nc\r\nd)"),
        Some(string(b"a(b)A+3c\r\nd", Literal))
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
    assert_eq!(object(b"[[[1]]]", &deep(2)), None);
    assert!(object(b"[[1]]", &deep(2)).is_some());
    assert_eq!(o(b"[1 (2"), None);
}

/// An operand nested 500 deep around 8 MiB is read in one pass, not once
/// for each level it's inside.
/// A number past 64 bits reads as pdf-objects reads one, the nearest
/// integer that fits or the largest finite real of its sign, so an array
/// holding one keeps its place, and a real read writes; and an operand of
/// more values than the cap reads as none.
#[test]
fn operands_read_as_pdf_objects_reads_them() {
    let lim = &Limits::DEFAULT;
    let integer = |s: &[u8]| object(s, lim).and_then(|o| o.as_integer());
    assert_eq!(integer(b"99999999999999999999"), Some(i64::MAX));
    assert_eq!(integer(b"-99999999999999999999"), Some(i64::MIN));
    assert_eq!(integer(b"9223372036854775807"), Some(i64::MAX));
    assert_eq!(integer(b"+-1"), None);
    let huge = format!("1{}.0", "0".repeat(400));
    let real = |s: &[u8]| object(s, lim).and_then(|o| o.as_number());
    assert_eq!(real(huge.as_bytes()), Some(f64::MAX));
    assert_eq!(real(format!("-{huge}").as_bytes()), Some(f64::MIN));
    let mut written = Vec::new();
    pdf_objects::serialize::real(&mut written, real(huge.as_bytes()).unwrap()).unwrap();
    let read = object(b"[1 99999999999999999999 3]", lim).unwrap();
    let items: Vec<_> = read
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o.as_integer())
        .collect();
    assert_eq!(items, [Some(1), Some(i64::MAX), Some(3)]);

    // Values: the operand, an array's elements, a dictionary's keys and
    // values, and theirs, what's no object among them.
    let values = |values| Limits { values, ..*lim };
    assert!(object(b"[[] [] []]", &values(4)).is_some());
    assert_eq!(object(b"[[] [] []]", &values(3)), None);
    assert!(object(b"<</A [1]>>", &values(4)).is_some());
    assert_eq!(object(b"<</A [1]>>", &values(3)), None);
    assert_eq!(object(b"1", &values(0)), None);
    // A list of many empty arrays, past the cap, read as none.
    let wide = [
        b"/Span <<".as_slice(),
        &b"[]".repeat(1 << 17),
        b">> BDC EMC",
    ]
    .concat();
    let props = Properties::default();
    let small = values(1 << 16);
    let found = marked(&Concat::new(&[&wide]), &props, &ResCx::none(), &small).unwrap();
    assert_eq!(found[0].props, None);
    let found = marked(&Concat::new(&[&wide]), &props, &ResCx::none(), lim).unwrap();
    assert!(found[0].props.is_some());
}

/// A word of more digits than 64 bits hold with a sign after them, such
/// as `99999999999999999999--5`, is no number as ISO 32000-2 writes one
/// (7.3.3), as `--5` isn't: it reads as no object, and is told so, where a
/// number that long alone reads as the nearest that fits.
#[test]
fn a_long_word_of_digits_and_signs_reads_as_none() {
    let lim = &Limits::DEFAULT;
    for not in [
        &b"99999999999999999999--5"[..],
        b"-99999999999999999999+5",
        b"+99999999999999999999-",
        b"99999999999999999999+",
    ] {
        assert_eq!(object(not, lim), None, "{}", String::from_utf8_lossy(not));
    }
    let read = object(b"[1 99999999999999999999--5 3]", lim).unwrap();
    assert_eq!(read, Object::from(vec![1.into(), 3.into()]));
    let s = b"99999999999999999999--5 w";
    assert_eq!(
        told(s),
        [(
            0..23,
            "99999999999999999999--5 is a number here and a keyword to ISO 32000-2".to_string()
        )]
    );
}

/// A string operand reads to its end, a hex string's bytes its digits
/// paired, what's no digit passed over, as PDFium reads them; one that
/// doesn't end reads as none, alone or inside an array or a dictionary.
#[test]
fn a_string_reads_to_its_end_or_as_none() {
    let lim = &Limits::DEFAULT;
    assert_eq!(object(b"<4G1>", lim), Some(string(b"A", Form::Hex)));
    assert_eq!(object(b"<41 4G>", lim), Some(string(b"A@", Form::Hex)));
    for not in [&b"(a"[..], b"<41", b"[1 <41", b"<</A (b"] {
        assert_eq!(object(not, lim), None, "{}", String::from_utf8_lossy(not));
    }
}

/// A number is one word of digits, signs and points, as PDFium's rules
/// tell one: bytes of more than one token, or none, aren't one.
#[test]
fn a_number_is_one_word() {
    use crate::scan::is_number;
    for word in [&b"1"[..], b"--5", b"1.2.3", b"+", b"."] {
        assert!(is_number(word), "{}", String::from_utf8_lossy(word));
    }
    for not in [&b""[..], b"1 2", b"1(", b"1/2", b"1\x80", b"1e5", b"x"] {
        assert!(!is_number(not), "{}", String::from_utf8_lossy(not));
    }
}

/// An operand's number as PDFium's content parser reads it (`GetNumber`),
/// an `f32`: a number word as `FX_Number` holds it, by its first 255
/// bytes, and 0 for what's no number. The words and values a reader of
/// content by PDFium's rules is held to; and as PDFium 7881 and 8076 read
/// `cm`'s last operand (one-page files, 2026-10-30): 0 for `true`,
/// `false`, `null`, a name, a string or an array, a word of 255 bytes read
/// whole and one of 256 by its first 255, so a real cut before its point
/// is an integer. An array's elements read alike, a `TJ`'s.
#[test]
fn an_operands_number_is_read_as_pdfium_reads_it() {
    let number = |word: &[u8]| {
        let s = [word, b" cm"].concat();
        let read = operations_of(&s, &ResCx::none(), &Limits::DEFAULT);
        read[0].as_ref().unwrap().arg(0).unwrap().number()
    };
    for (word, n) in [
        (&b"1.2.3"[..], 1.2),
        (b"--5.0", -5.0),
        (b"+-.5", -0.5),
        (b"5.", 5.0),
        (b".", 0.0),
        (b"-", 0.0),
        (b"007", 7.0),
        (b"4294967295", 4_294_967_295.0),
        (b"4294967296", 0.0),
        (b"+2147483648", 0.0),
        (b"-2147483648", -2_147_483_648.0),
        (b"5-3", 5.0),
        (b"--5", 0.0),
        (b"/Name", 0.0),
        (b"(5)", 0.0),
        (b"2147483648", 2_147_483_648.0),
        (b"16777217", 16_777_216.0),
        (b"true", 0.0),
        (b"false", 0.0),
        (b"null", 0.0),
        (b"<35>", 0.0),
        (b"[5]", 0.0),
        (b"<</N 5>>", 0.0),
    ] {
        assert_eq!(number(word), n, "{}", String::from_utf8_lossy(word));
    }
    assert!(number(&[b"9".repeat(40), b".0".to_vec()].concat()).is_infinite());
    let zeros = |n: usize, then: &[u8]| [b"0".repeat(n).as_slice(), then].concat();
    assert_eq!(number(&zeros(254, b"2")), 2.0);
    assert_eq!(number(&zeros(255, b"2")), 0.0);
    assert_eq!(number(&zeros(254, b"7.5")), 7.0);
    assert_eq!(number(&zeros(255, b"7.5")), 0.0);

    let s = b"[(a) -250 (b) --5 1.2.3 /N [7]] TJ";
    let read = operations_of(s, &ResCx::none(), &Limits::DEFAULT);
    let tj = read[0].as_ref().unwrap().arg(0).unwrap();
    let numbers: Vec<f32> = tj.elements().map(|e| e.number()).collect();
    assert_eq!(numbers, [0.0, -250.0, 0.0, 0.0, 1.2, 0.0, 0.0]);
}

/// A name as PDFium's content parser reads it: a `#` with two bytes after
/// it stands for the byte they make, one that's no hex digit read as 0,
/// and with fewer it's itself; and a name longer than 255 bytes, its `/`
/// among them, is its first 255. As PDFium 7881 and 8076 find `Tf`'s font
/// (one-page files, 2026-10-30): `F0` for `/F#30`, `/F#3G`, `/F#3g` and
/// `/#460`, none for `/F#G0`, `/F#3`, `/F0#`, `/F0#2`, `/F#300` and
/// `/#F#30`; and a font named by 254 bytes for those and any more. The
/// resource an operation names is read so, and a name in an array or a
/// dictionary.
#[test]
fn a_name_is_read_as_pdfium_reads_it() {
    for (written, read) in [
        (&b"/F#30"[..], &b"F0"[..]),
        (b"/F#3G", b"F0"),
        (b"/F#3g", b"F0"),
        (b"/#460", b"F0"),
        (b"/F#G0", b"F\0"),
        (b"/F#3", b"F#3"),
        (b"/F0#", b"F0#"),
        (b"/F0#2", b"F0#2"),
        (b"/F#300", b"F00"),
        (b"/#F#30", b"\xf030"),
    ] {
        let shown = String::from_utf8_lossy(written);
        assert_eq!(name(written).as_deref(), Some(read), "{shown}");
        let s = [b"BT ", written, b" 10 Tf ET"].concat();
        let ops = operations_of(&s, &ResCx::none(), &Limits::DEFAULT);
        let font = ops[1].as_ref().unwrap().resource();
        assert_eq!(font, Some((ResKind::Font, read.into())), "{shown}");
    }
    let f254 = b"F".repeat(254);
    for more in [&b""[..], b"X", b"XX", b"#41"] {
        let long = [b"/", f254.as_slice(), more].concat();
        assert_eq!(name(&long).as_deref(), Some(&f254[..]));
    }
    // A `#` the cut leaves one byte after is itself.
    let cut = [b"/", &f254[..252], b"#41"].concat();
    let kept = [&f254[..252], b"#4"].concat();
    assert_eq!(name(&cut).as_deref(), Some(&kept[..]));
    let read = object(b"<</F#3G [/G#3G]>>", &Limits::DEFAULT);
    let g0 = Object::from(vec![Object::from("G0")]);
    assert_eq!(read, Some(Object::from(dict! { "F0" => g0 })));
}

/// A string as PDFium's content parser reads it: a literal string's CR or
/// CR LF written in it as written, where ISO 32000-2 reads LF, every escape
/// as ISO reads it; and a literal's or a hex string's first 32,767 bytes,
/// read. As PDFium 7881 and 8076 show `Tj`'s and `TJ`'s text (one-page
/// files, 2026-10-30): `(a\rb\r\nc)` those bytes, `[(a\rb) -250 (c)]` `a\rb`
/// and `c`, and 32,767 bytes of a string of 32,768 read, 32,768 `\101`
/// escapes or 32,768 pairs of hex digits and one more digit among them.
#[test]
fn a_string_is_read_as_pdfium_reads_it() {
    use Form::{Hex, Literal};
    let lim = &Limits::DEFAULT;
    assert_eq!(
        object(b"(a\rb\r\nc)", lim),
        Some(string(b"a\rb\r\nc", Literal))
    );
    assert_eq!(object(b"(a\r\rb)", lim), Some(string(b"a\r\rb", Literal)));
    assert_eq!(object(b"(a\n\rb)", lim), Some(string(b"a\n\rb", Literal)));
    // An end of line escaped is nothing, and `\015` a CR, as ISO reads them.
    let escaped = object(b"(a\\\rb\\\r\nc\\\nd\\015)", lim);
    assert_eq!(escaped, Some(string(b"abcd\r", Literal)));
    let array = object(b"[(a\rb) -250 (c)]", lim);
    let shown = vec![
        string(b"a\rb", Literal),
        Object::from(-250),
        string(b"c", Literal),
    ];
    assert_eq!(array, Some(Object::from(shown)));

    let length = |written: &[u8]| match object(written, lim).map(Object::into_value) {
        Some(Value::String(Str::Bytes(bytes), _)) => bytes.len(),
        other => panic!("{other:?}"),
    };
    let literal = |inside: Vec<u8>| [b"(", inside.as_slice(), b")"].concat();
    let hex = |inside: Vec<u8>| [b"<", inside.as_slice(), b">"].concat();
    for (written, read) in [
        (literal(b"a".repeat(32_766)), 32_766),
        (literal(b"a".repeat(32_767)), 32_767),
        (literal(b"a".repeat(32_768)), 32_767),
        (literal(b"\\101".repeat(32_768)), 32_767),
        (hex(b"41".repeat(32_767)), 32_767),
        (hex([b"41".repeat(32_767), b"4".to_vec()].concat()), 32_767),
        (hex(b"41".repeat(32_768)), 32_767),
    ] {
        assert_eq!(length(&written), read);
    }
    let first = object(&literal(b"\\101".repeat(32_768)), lim);
    assert_eq!(first, Some(string(&b"A".repeat(32_767), Literal)));
    let first = object(&hex([b"41".repeat(32_767), b"4".to_vec()].concat()), lim);
    assert_eq!(first, Some(string(&b"A".repeat(32_767), Hex)));
}

/// An inline image's `/W`, `/H` and `/BPC` as PDFium reads an integer from
/// each, held in 32 bits, and the size they count; an image whose size
/// can't be counted, filtered or not, read on to `EI` from its data's
/// start; and its color space's name as PDFium decodes it. As PDFium 8076
/// reads each (one-page files, 2026-10-30; 7881 alike but where a row is
/// 2^32 − 7 bits or more, below): the data, or the operations after it.
/// Where PDFium counts no data, or reads none, the data here runs on to
/// `EI`, as for any image no size counts.
#[test]
fn an_inline_images_numbers_are_read_as_pdfium_reads_them() {
    let res = ResCx::new([(b"CP".to_vec(), 3)]);
    let image = |dict: &str| format!("BI {dict} ID xyzw EI 5 5 m 7 7 l S");
    let all = Some("xyzw");
    let cases: Vec<(String, Option<&str>)> = vec![
        // A width: a real's whole part, `true` 1, what's no number 0, an
        // unsigned one past `i32` wrapped, negative, as none.
        (image("/W 1.9 /H 1 /BPC 8 /CS /G"), Some("x")),
        (image("/W 2.9 /H 1 /BPC 8 /CS /G"), Some("xy")),
        (image("/W +2 /H 1 /BPC 8 /CS /G"), Some("xy")),
        (image("/W true /H 1 /BPC 8 /CS /G"), Some("x")),
        (image("/W --3 /H 1 /BPC 8 /CS /G"), all),
        (image("/W false /H 1 /BPC 8 /CS /G"), all),
        (image("/W (3) /H 1 /BPC 8 /CS /G"), all),
        (image("/W /N /H 1 /BPC 8 /CS /G"), all),
        (image("/W -0.5 /H 1 /BPC 8 /CS /G"), all),
        (image("/W 4294967297 /H 1 /BPC 8 /CS /G"), all),
        (image("/W -1 /H 1 /BPC 8 /CS /G"), all),
        (image("/W 4294967295 /H 1 /BPC 8 /CS /G"), all),
        (image("/W 2147483648 /H 1 /BPC 8 /CS /G"), all),
        (image("/W 5000000000.0 /H 1 /BPC 8 /CS /G"), all),
        (
            image(&format!("/W {}2 /H 1 /BPC 8 /CS /G", "0".repeat(254))),
            Some("xy"),
        ),
        (
            image(&format!("/W {}2 /H 1 /BPC 8 /CS /G", "0".repeat(255))),
            all,
        ),
        // A height and bits per component alike.
        (image("/W 1 /H 1.9 /BPC 8 /CS /G"), Some("x")),
        (image("/W 1 /H true /BPC 8 /CS /G"), Some("x")),
        (image("/W 2 /H -1 /BPC 8 /CS /G"), all),
        (image("/W 2 /H 1 /BPC true /CS /G"), Some("x")),
        (image("/W 2 /H 1 /BPC 2.9 /CS /G"), Some("x")),
        (image("/W 1 /H 1 /BPC 16 /CS /G"), Some("xy")),
        (image("/W 2 /H 1 /BPC -8 /CS /G"), all),
        (image("/W 2 /H 1 /BPC --8 /CS /G"), all),
        (image("/W 1 /H 1 /BPC -8 /CS /RGB"), all),
        // No bits to a pixel: no size, whatever the width; a negative
        // width at one bit a pixel, though its row fits 32 bits unsigned:
        // none.
        (image("/W -1 /H 1 /BPC 0 /CS /G"), all),
        (image("/W -1 /H 1 /BPC 1 /CS /G"), all),
        (image("/W 4294967295 /H 1 /BPC 1 /CS /G"), all),
        // A key matched as written, `/B#5GC` and `/B#50C` no `/BPC`, so 0
        // bits a component where there's a color space.
        (image("/W 8 /H 1 /B#5GC 8 /CS /G"), all),
        (image("/W 8 /H 1 /B#50C 8 /CS /G"), all),
        // The color space's name as PDFium decodes it: `/C#5G` is `CP`.
        (image("/W 1 /H 1 /BPC 8 /CS /C#5G"), Some("xyz")),
        (image("/W 1 /H 1 /BPC 8 /CS /C#50"), Some("xyz")),
        (image("/W 1 /H 1 /BPC 8 /CS /CQ"), Some("x")),
        // A filter's name decoded.
        (
            image("/W 1 /H 1 /BPC 8 /CS /G /F /A#48x").replace("xyzw", "41>"),
            Some("41>"),
        ),
    ];
    for (content, data) in &cases {
        let s = content.as_bytes();
        let read = operations_of(s, &res, &Limits::DEFAULT);
        let read: Vec<_> = read.into_iter().map(Result::unwrap).collect();
        let names: Vec<_> = read.iter().map(|op| op.name()).collect();
        assert_eq!(names, [&b"BI"[..], b"m", b"l", b"S"], "{content}");
        let found = read[0].inline_image().map(|i| &s[i.data()]);
        assert_eq!(found, data.map(str::as_bytes), "{content}");
    }

    // A size that takes the rest of the content: PDFium reads nothing
    // after it, and here the image doesn't end. A width past `i32` by a
    // real is its largest, at one bit a pixel; a height of -1 is 2^32 − 1
    // rows; a row of 2^32 − 1 bits is 2^29 bytes (7881 overflows adding 7
    // first, and reads on to `EI`).
    for dict in [
        "/W 5000000000.0 /H 1 /BPC 1 /CS /G",
        "/IM true /W 5000000000.0 /H 1",
        "/W 1 /H -1 /BPC 8 /CS /G",
        "/W 1 /H 4294967295 /BPC 8 /CS /G",
        "/W 1431655765 /H 1 /BPC 3 /CS /G",
        "/W 1431655764 /H 1 /BPC 3 /CS /G",
    ] {
        let content = image(dict);
        let e = error(content.as_bytes(), &Limits::DEFAULT);
        assert_eq!((e.at, e.kind), (0, ErrorKind::InlineImage), "{content}");
    }

    // A filtered image whose size can't be counted has no data to PDFium,
    // which reads on to the first `EI`, not to its filter's end: two paths
    // drawn, where one is when it can be counted.
    let tail = "41 EI 1 1 m 2 2 l S 42> EI 5 5 m 7 7 l S";
    let mut cases = vec![];
    for (dict, data) in [
        ("/W -1 /H 1 /BPC 8 /CS /G /F /AHx", "41"),
        ("/W 2147483648 /H 1 /BPC 8 /CS /G /F /AHx", "41"),
        ("/W -1 /H 1 /BPC 8 /CS /G /F /A85", "41"),
        ("/W 2 /H 4294967295 /BPC 8 /CS /G /F /RL", "41"),
        ("/W 1 /H 1 /BPC 8 /CS /G /F /AHx", "41 EI 1 1 m 2 2 l S 42>"),
        ("/F /AHx", "41 EI 1 1 m 2 2 l S 42>"),
    ] {
        cases.push((format!("BI {dict} ID {tail}"), data));
    }
    for (content, data) in &cases {
        let s = content.as_bytes();
        let read: Vec<_> = (operations_of(s, &res, &Limits::DEFAULT).into_iter())
            .map(Result::unwrap)
            .collect();
        let found = read[0].inline_image().map(|i| &s[i.data()]);
        assert_eq!(found, Some(data.as_bytes()), "{content}");
        let paths = read.iter().filter(|op| op.name() == b"S").count();
        assert_eq!(paths, if data.len() == 2 { 2 } else { 1 }, "{content}");
    }
}

#[test]
fn deep_operands_read_in_one_pass() {
    let mut s = b"[".repeat(500);
    s.push(b'<');
    s.resize(s.len() + (8 << 20), b'0');
    s.push(b'>');
    s.extend(b"]".repeat(500));
    let started = std::time::Instant::now();
    let mut o = object(&s, &Limits::DEFAULT).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    for _ in 0..500 {
        let Value::Array(mut inner) = o.into_value() else {
            panic!("an array")
        };
        o = inner.pop().unwrap();
    }
    // Its first 32,767 bytes, the most PDFium keeps of a string.
    assert!(matches!(o.into_value(), Value::String(Str::Bytes(bytes), _) if bytes.len() == 32_767));
}

/// `s`'s one inline image: where the token is, and its data.
/// The default caps but on depth, `depth`.
fn deep(depth: usize) -> Limits {
    Limits {
        depth,
        ..Limits::DEFAULT
    }
}

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
    // Flate, LZW and CCITTFax data, which PDFium decodes to find its end,
    // end at the first `EI` between white-space; PDF 2.0's length,
    // abbreviated or not, PDFium doesn't read.
    assert_image("/F /Fl /W 9 /H 9 /BPC 8 /CS /G /L 2", b"xEIy", &none);
    assert_image("/F /LZW /Length 2 /W 9 /H 9", b"xEIy", &none);
    assert_image(
        "/F /CCF /DP <</K -1 /Columns 9>> /W 9 /H 9",
        b"xEI EIy",
        &none,
    );
    assert_image("/F /Fl /W 9 /H 9 /BPC 8 /CS /G", b"", &none);
    // A filter PDFium can't decode: its tokens read to the next `EI`, a
    // string one token.
    assert_image("/F /XYZ /W 9 /H 9 /BPC 8 /CS /G", b"(x EI y)", &none);
}

/// Inline images as PDFium reads them, each case's data and the paths
/// drawn after it those PDFium's page objects show for the same content:
/// a key written both abbreviated and in full, a key's `#` escapes, a
/// filter by what its value is, a filter PDFium has no decoder for or one
/// that finds no end, an image no size counts, and ASCII85's end.
#[test]
fn inline_images_read_as_pdfium_reads_them() {
    let res = ResCx::new([
        (b"CS0".to_vec(), 1),
        (b"CS1".to_vec(), 3),
        (b"Indexed".to_vec(), 3),
    ]);
    // An `EI` the data may hide, a path, an `EI`, a path.
    let two =
        |dict: &str, data: &str| format!("BI {dict} ID {data} EI 0 0 m 9 9 l S EI 5 5 m 7 7 l S");
    // The data, an `EI` and a path.
    let one = |dict: &str, data: &str| format!("BI {dict} ID {data} EI 5 5 m 7 7 l S");
    // A string holding an `EI` and a path, then an `EI` and a path.
    let string = |dict: &str| one(dict, "(x EI 0 0 m 9 9 l S)");
    let gray = "/W 1 /H 1 /BPC 8 /CS /G";
    let cases: Vec<(String, &str, usize)> = vec![
        // The abbreviation's value, unless the full name's is an
        // abbreviated name, wherever each is written.
        (
            two(
                "/W 1 /H 1 /BPC 8 /CS /DeviceGray /ColorSpace /DeviceRGB",
                "a",
            ),
            "a",
            2,
        ),
        (
            two(
                "/W 1 /H 1 /BPC 8 /ColorSpace /DeviceRGB /CS /DeviceGray",
                "a",
            ),
            "a",
            2,
        ),
        (
            two("/W 1 /Width 3 /H 1 /BPC 8 /CS /DeviceGray", "a"),
            "a",
            2,
        ),
        (
            two("/Width 3 /W 1 /H 1 /BPC 8 /CS /DeviceGray", "a"),
            "a",
            2,
        ),
        (
            two("/W 1 /H 1 /BPC 8 /CS /G /ColorSpace /RGB", "a"),
            "a E",
            1,
        ),
        (
            two("/W 1 /H 1 /BPC 8 /CS /G /ColorSpace /DeviceRGB", "a"),
            "a",
            2,
        ),
        (
            two("/W 1 /H 1 /BPC 8 /CS /DeviceGray /ColorSpace /RGB", "a"),
            "a E",
            1,
        ),
        (
            two("/W 1 /H 1 /BPC 8 /CS /CS0 /ColorSpace /CS1", "a"),
            "a",
            2,
        ),
        (
            one(&format!("{gray} /F /XYZ /Filter /AHx"), "41>"),
            "41>",
            1,
        ),
        (
            one(&format!("{gray} /F /AHx /Filter /XYZ"), "41>"),
            "41>",
            1,
        ),
        // A key's escapes aren't decoded, a value's are; an abbreviated
        // name is looked up by its full name.
        (two("/W 1 /H 1 /BPC 8 /C#53 /RGB", "a"), "a", 2),
        (two(&format!("{gray} /#46 /XYZ"), "a"), "a", 2),
        (two("/W 1 /H 1 /BPC 8 /CS /R#47B", "a"), "a E", 1),
        (one(&format!("{gray} /F /A#48x"), "41>"), "41>", 1),
        (two("/W 1 /H 1 /BPC 8 /CS /I", "a"), "a E", 1),
        // A filter PDFium has no decoder for, by name or as a string's
        // bytes, a number, a boolean or an array first: the tokens read on
        // to the next `EI`, its `/L` not read; `null`, a dictionary, an
        // empty array or what's no object is none.
        (two(&format!("{gray} /F /XYZ /L 19"), "a"), "a", 2),
        (
            string(&format!("{gray} /F /XYZ")),
            "(x EI 0 0 m 9 9 l S)",
            1,
        ),
        (
            string(&format!("{gray} /F [/XYZ /Fl]")),
            "(x EI 0 0 m 9 9 l S)",
            1,
        ),
        (
            string(&format!("{gray} /F /JPXDecode")),
            "(x EI 0 0 m 9 9 l S)",
            1,
        ),
        (
            string(&format!("{gray} /F (Fl)")),
            "(x EI 0 0 m 9 9 l S)",
            1,
        ),
        (
            string(&format!("{gray} /F (AHx)")),
            "(x EI 0 0 m 9 9 l S)",
            1,
        ),
        (
            string(&format!("{gray} /F <41>")),
            "(x EI 0 0 m 9 9 l S)",
            1,
        ),
        (string(&format!("{gray} /F 5")), "(x EI 0 0 m 9 9 l S)", 1),
        (
            string(&format!("{gray} /F true")),
            "(x EI 0 0 m 9 9 l S)",
            1,
        ),
        (string(&format!("{gray} /F [5]")), "(x EI 0 0 m 9 9 l S)", 1),
        (
            string(&format!("{gray} /F [(XYZ)]")),
            "(x EI 0 0 m 9 9 l S)",
            1,
        ),
        (
            string(&format!("{gray} /F [[/AHx]]")),
            "(x EI 0 0 m 9 9 l S)",
            1,
        ),
        (one(&format!("{gray} /F (ASCIIHexDecode)"), "41>"), "41>", 1),
        (
            one(&format!("{gray} /F [(ASCIIHexDecode)]"), "41>"),
            "41>",
            1,
        ),
        (two(&format!("{gray} /F null"), "a"), "a", 2),
        (two(&format!("{gray} /F <<>>"), "a"), "a", 2),
        (two(&format!("{gray} /F []"), "a"), "a", 2),
        (two(&format!("{gray} /F [null]"), "a"), "a", 2),
        (two(&format!("{gray} /F foo"), "a"), "a", 2),
        // No size counted: the tokens read on to the next `EI`.
        (string("/H 1 /BPC 8 /CS /G"), "(x EI 0 0 m 9 9 l S)", 1),
        (string("/W 0 /H 1 /BPC 8 /CS /G"), "(x EI 0 0 m 9 9 l S)", 1),
        (two("/H 1 /BPC 8 /CS /G /L 19", "a"), "a", 2),
        // ASCII85 to the first byte that's none of its digits, and a `>`
        // after it; ASCIIHex, ASCII85 and RunLength that don't end read
        // on to the next `EI`.
        (two(&format!("{gray} /F /A85"), "9jqo~"), "9jqo~", 2),
        (two(&format!("{gray} /F /A85"), "9jqov"), "9jqov", 2),
        (two(&format!("{gray} /F [/A85 /Fl]"), "9jqo~"), "9jqo~", 2),
        (one(&format!("{gray} /F /A85"), "9jqo^~>"), "9jqo^~>", 1),
        (one(&format!("{gray} /F /A85"), "~>"), "", 1),
        (one(&format!("{gray} /F /AHx"), "0E"), "0E", 1),
        (one(&format!("{gray} /F /A85"), "9jqo"), "9jqo", 1),
        (one(&format!("{gray} /F /RL"), "\x05abc"), "\x05abc", 1),
    ];
    for (content, data, paths) in &cases {
        let s = content.as_bytes();
        let read = operations_of(s, &res, &Limits::DEFAULT);
        let read: Vec<_> = read.into_iter().map(Result::unwrap).collect();
        let image = read[0].inline_image().expect("an image");
        assert_eq!(&s[image.data()], data.as_bytes(), "{content}");
        let drawn = read.iter().filter(|op| op.name() == b"S").count();
        assert_eq!(drawn, *paths, "{content}");
    }
    // Either spelling finds the value PDFium reads, and the color space
    // used is the one PDFium looks up.
    let s = b"BI /W 1 /H 1 /BPC 8 /CS /CS0 /ColorSpace /CS1 ID a EI";
    let op = operations_of(s, &res, &Limits::DEFAULT).remove(0).unwrap();
    let image = op.inline_image().unwrap();
    let cs = |key: &[u8]| image.entry(key).map(|o| o.bytes());
    assert_eq!(
        (cs(b"CS"), cs(b"ColorSpace")),
        (Some(&b"/CS0"[..]), Some(&b"/CS0"[..]))
    );
    assert_eq!(
        op.resource(),
        Some((ResKind::ColorSpace, b"CS0"[..].into()))
    );
    let s = b"BI /W 1 /H 1 /BPC 8 /CS /G /ColorSpace /RGB ID abc EI";
    let op = operations_of(s, &res, &Limits::DEFAULT).remove(0).unwrap();
    assert_eq!(
        op.inline_image().unwrap().entry(b"CS").unwrap().bytes(),
        b"/RGB"
    );
    let s = b"BI /W 1 /H 1 /BPC 8 /CS /I ID abc EI";
    let op = operations_of(s, &res, &Limits::DEFAULT).remove(0).unwrap();
    assert_eq!(
        op.resource(),
        Some((ResKind::ColorSpace, b"Indexed"[..].into()))
    );
    // `/L` before `/Length`, as 8.9.7 has it.
    let s = b"BI /F /XYZ /Length 9 /L 2 ID ab EI";
    let op = operations_of(s, &res, &Limits::DEFAULT).remove(0).unwrap();
    assert_eq!(op.inline_image().unwrap().length(), Some(2));
}

/// An inline image's key ISO 32000-2 reads otherwise: written with a `#`
/// escape, or both abbreviated and in full where the full name's value is
/// an abbreviated name that means another, told with its dictionary, in
/// the order of their ends.
#[test]
fn an_inline_images_keys_read_otherwise() {
    let s = b"BI /W 1 /H 1 /BPC 8 /CS /G /ColorSpace /RGB ID abc EI";
    let dict = place(s, b" /W 1 /H 1 /BPC 8 /CS /G /ColorSpace /RGB ");
    let words = "an inline image's ColorSpace is read from its full name, and by ISO 32000-2 \
                 from its abbreviation";
    assert_eq!(told(s), [(dict, words.to_string())]);
    let s = b"BI /W 1 /H 1 /BPC 8 /CS /DeviceGray /ColorSpace /RGB ID abc EI";
    assert_eq!(told(s).len(), 1);
    let s = b"BI /W 1 /H 1 /BPC 8 /F /XYZ /Filter /AHx ID 41> EI";
    let found = told(s);
    assert_eq!(found.len(), 1);
    assert!(found[0].1.contains(" Filter "));
    // The same meaning, or PDFium reading the abbreviation as ISO does,
    // isn't told.
    assert_eq!(
        told(b"BI /W 1 /H 1 /BPC 8 /CS /DeviceRGB /ColorSpace /RGB ID abc EI"),
        []
    );
    assert_eq!(
        told(b"BI /W 1 /H 1 /BPC 8 /CS /G /ColorSpace /DeviceRGB ID a EI"),
        []
    );
    assert_eq!(told(b"BI /W 1 /Width 3 /H 1 /BPC 8 /CS /G ID a EI"), []);
    // A key with an escape that ISO reads as one of an image's, and not
    // one it doesn't read.
    let s = b"BI /W 1 /H 1 /BPC 8 /C#53 /RGB /#46 /XYZ /A#42 1 ID a EI";
    let key = |w: &str| format!("{w} is an inline image's key to ISO 32000-2, and no key here");
    assert_eq!(
        told(s),
        [
            (place(s, b"/C#53"), key("/C#53")),
            (place(s, b"/#46"), key("/#46"))
        ]
    );
    // A token's fault later in the dictionary is told before the keys'.
    let s = b"BI /W 1 /H 1 /BPC 8 /CS /G /ColorSpace /RGB /X <4G> ID abc EI";
    let found = told(s);
    assert_eq!(found[0].0, place(s, b"<4G>"));
    assert_eq!(
        found[1].0,
        place(s, b" /W 1 /H 1 /BPC 8 /CS /G /ColorSpace /RGB /X <4G> ")
    );
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
    // An object where a key should be ends the dictionary, as PDFium ends
    // it, the data read from after it: here none counted, so on to the
    // next `EI` (PDFium 7881 and 8076 alike: `BI 7 /W 1 /H 1 /BPC 8 /CS
    // /G ID (x EI 0 0 m 9 9 l S) EI 5 5 m 7 7 l S` an image of no data,
    // then one path).
    let s = b"BI 7 /W 1 /H 1 /BPC 8 /CS /G ID x EI";
    assert_eq!(image(s, &ResCx::none()).1, b"/W 1 /H 1 /BPC 8 /CS /G ID x");
}

/// An inline image's dictionary as PDFium's `Handle_BeginImage` reads it
/// (PDFium 7881 and 8076 alike, each case's page objects): a name and its
/// value, and so on, until what's where a key should be isn't a name.
/// `ID` ends it. Any other object ends it too, the data read from after
/// it as from after `ID`, sized by the entries before it. A keyword other
/// than `ID` makes the `BI` no image but an operation of its own, and
/// what's after it content, the dictionary's tokens among it. A value
/// PDFium reads as no object (a keyword, `ID` among them, a closer or a
/// brace) takes its key out. Each case's image's data, where there's an
/// image, and the operations read, which draw the paths PDFium's page
/// objects show (an `S` each). Where PDFium's image has no data, none
/// counted, the data here is read on to the next `EI`, as for an image no
/// size counts; and PDFium makes no image of a filter it can't decode.
#[test]
fn an_inline_images_dictionary_ends_as_pdfium_reads_it() {
    let p = " 5 5 m 7 7 l S";
    let g = "/W 1 /H 1 /BPC 8 /CS /G";
    let data = "/W 1 /H 1 /BPC 8 /CS /G ID (x EI 0 0 m 9 9 l S)";
    let cases: Vec<(String, Option<&str>, &str)> = vec![
        // An object where a key should be: the dictionary ends there.
        (
            format!("BI 7 {g} ID (x EI 0 0 m 9 9 l S) EI{p}"),
            Some(data),
            "BI m l S",
        ),
        (
            format!("BI true {g} ID (x EI 0 0 m 9 9 l S) EI{p}"),
            Some(data),
            "BI m l S",
        ),
        (
            format!("BI false {g} ID (x EI 0 0 m 9 9 l S) EI{p}"),
            Some(data),
            "BI m l S",
        ),
        (
            format!("BI null {g} ID (x EI 0 0 m 9 9 l S) EI{p}"),
            Some(data),
            "BI m l S",
        ),
        (format!("BI {g} (k) xy EI{p}"), Some("x"), "BI m l S"),
        (format!("BI {g} (k) ID ab EI{p}"), Some("I"), "BI m l S"),
        (format!("BI {g} <6B> xy EI{p}"), Some("x"), "BI m l S"),
        (format!("BI {g} [1 2] xy EI{p}"), Some("x"), "BI m l S"),
        (format!("BI {g} <</A 1>> xy EI{p}"), Some("x"), "BI m l S"),
        (format!("BI {g} 1.5 xy EI{p}"), Some("x"), "BI m l S"),
        (format!("BI {g} --5 xy EI{p}"), Some("x"), "BI m l S"),
        (format!("BI {g} ) xy EI{p}"), Some("x"), "BI m l S"),
        (format!("BI {g} ] xy EI{p}"), Some("x"), "BI m l S"),
        (format!("BI {g} >> xy EI{p}"), Some("x"), "BI m l S"),
        (format!("BI {g} > xy EI{p}"), Some("x"), "BI m l S"),
        (format!("BI {g} {{ xy EI{p}"), Some("x"), "BI m l S"),
        (format!("BI {g} }} xy EI{p}"), Some("x"), "BI m l S"),
        (format!("BI {g} (k)xy EI{p}"), Some("x"), "BI m l S"),
        (format!("BI {g} 7 ID x EI{p}"), Some("I"), "BI m l S"),
        (
            format!("BI /W 2 /H 1 /BPC 8 /CS /G 7 xyz EI{p}"),
            Some("xy"),
            "BI m l S",
        ),
        // Its filter and size, the object after them or before.
        (
            format!("BI {g} /F /AHx 7 41> EI{p}"),
            Some("41>"),
            "BI m l S",
        ),
        (format!("BI {g} 7 /F /AHx 41> EI{p}"), Some("/"), "BI m l S"),
        (
            format!("BI {g} /F /A85 (k) 9jqo^~> EI{p}"),
            Some("9jqo^~>"),
            "BI m l S",
        ),
        (
            format!("BI /W 1 /H 1 /BPC 8 /CS /RGB [0] abcd EI{p}"),
            Some("abc"),
            "BI m l S",
        ),
        (
            format!("BI {g} /F /XYZ 7 (x EI 0 0 m 9 9 l S) EI{p}"),
            Some("(x EI 0 0 m 9 9 l S)"),
            "BI m l S",
        ),
        // A keyword where a key should be: no image.
        (
            format!("BI /W 1 /H 1 foo /BPC 8 /CS /G ID a EI 0 0 m 9 9 l S EI{p}"),
            None,
            "BI foo ID a EI m l S EI m l S",
        ),
        (
            format!("BI foo 0 0 m 9 9 l S{p}"),
            None,
            "BI foo m l S m l S",
        ),
        (
            format!("BI {g} foo ID x EI{p}"),
            None,
            "BI foo ID x EI m l S",
        ),
        (format!("BI EI{p}"), None, "BI EI m l S"),
        (format!("BI BI {g} ID x EI{p}"), Some("x"), "BI BI m l S"),
        (
            format!("BI /W 1 BI {g} ID x EI{p}"),
            Some("x"),
            "BI BI m l S",
        ),
        (format!("BI BI foo{p}"), None, "BI BI foo m l S"),
        (format!("BI /W 1 % c ID\nfoo{p}"), None, "BI foo m l S"),
        (
            format!("BI foo BI {g} ID x EI{p}"),
            Some("x"),
            "BI foo BI m l S",
        ),
        ("0 0 m 9 9 l S BI foo".to_string(), None, "m l S BI foo"),
        // A value PDFium reads as no object: its key taken out, an `ID`
        // too, the dictionary read on past it.
        (format!("BI {g} /F ID x EI{p}"), None, "BI ID x EI m l S"),
        (
            format!("BI /W 1 /H 1 /BPC 8 /CS ID x EI{p}"),
            None,
            "BI ID x EI m l S",
        ),
        (
            format!("BI /W 1 /H 1 /BPC 8 /CS ID 7 x EI{p}"),
            Some("x"),
            "BI m l S",
        ),
        (
            format!("BI /W 1 /H 1 /BPC 8 /CS ID ID abc EI{p}"),
            Some("a"),
            "BI m l S",
        ),
        (
            format!("BI /CS ID /W 1 /H 1 /BPC 8 ID x EI{p}"),
            Some("x"),
            "BI m l S",
        ),
        (
            format!("BI /W 1 /H 1 /BPC 8 /CS foo ID abc EI{p}"),
            Some("a"),
            "BI m l S",
        ),
        (
            format!("BI /W 3 /H 1 /BPC 8 /CS /G /CS foo ID abcdefghij EI{p}"),
            Some("a"),
            "BI m l S",
        ),
        (
            format!("BI /W foo /H 1 /BPC 8 /CS /G ID abc EI{p}"),
            Some("abc"),
            "BI m l S",
        ),
        (
            format!("BI /W 1 /H 1 /BPC 8 /CS ) ID abc EI{p}"),
            Some("a"),
            "BI m l S",
        ),
        (
            format!("BI /W 1 /H 1 /BPC 8 /CS ] ID abc EI{p}"),
            Some("a"),
            "BI m l S",
        ),
        (
            format!("BI /W 1 /H 1 /BPC 8 /CS >> ID abc EI{p}"),
            Some("a"),
            "BI m l S",
        ),
        (
            format!("BI /W 1 /H 1 /BPC 8 /CS {{ ID abc EI{p}"),
            Some("a"),
            "BI m l S",
        ),
        (
            format!("BI /W 1 /H 1 /BPC 8 /CS BI ID abc EI{p}"),
            Some("a"),
            "BI m l S",
        ),
        (
            format!("BI /W 1 /H 1 /BPC 8 /CS true ID abc EI{p}"),
            Some("abc"),
            "BI m l S",
        ),
        (
            format!("BI {g} /F /AHx /F foo ID 41> EI{p}"),
            Some("4"),
            "BI m l S",
        ),
    ];
    let res = ResCx::none();
    for (content, data, names) in &cases {
        let s = content.as_bytes();
        let read = operations_of(s, &res, &Limits::DEFAULT);
        let read: Vec<_> = read.into_iter().map(Result::unwrap).collect();
        let found: Vec<_> = read
            .iter()
            .map(|op| String::from_utf8_lossy(op.name()))
            .collect();
        assert_eq!(found.join(" "), *names, "{content}");
        let images: Vec<&[u8]> = (read.iter())
            .filter_map(|op| op.inline_image().map(|i| &s[i.data()]))
            .collect();
        assert_eq!(images, data.map(str::as_bytes).as_slice(), "{content}");
    }
}

/// A `BI` that begins no inline image is an operation of its own, with
/// the operands before it, as PDFium clears them after it (`10 20 30 40
/// BI re f`, PDFium 7881 and 8076 alike, fills a rectangle at 0 0 of no
/// size); its dictionary's tokens are content, operands of the keyword
/// that ends it (`BT BI /F1 24 Tf ... (Hi) Tj ET` shows its text at 24
/// points). An image whose dictionary an object ends has a dictionary to
/// that object, and the entries before it; a key whose value PDFium reads
/// as no object has none.
#[test]
fn a_lone_bi_is_an_operation_and_an_ended_dictionary_ends_there() {
    let res = ResCx::none();
    assert_eq!(
        ops(b"10 20 30 40 BI re f"),
        [
            (&b"BI"[..], vec![&b"10"[..], b"20", b"30", b"40"]),
            (b"re", vec![]),
            (b"f", vec![])
        ]
    );
    let s = b"BT BI /F1 24 Tf 10 10 Td (Hi) Tj ET";
    let read = operations_of(s, &res, &Limits::DEFAULT);
    let lone = read[1].as_ref().unwrap();
    assert_eq!(
        (lone.name(), lone.at(), lone.range()),
        (&b"BI"[..], 3..5, 3..5)
    );
    assert!(lone.inline_image().is_none());
    assert_eq!(lone.operator(), Operator::BeginInlineImage);
    assert_eq!(lone.resource(), None);
    assert_eq!(
        ops(s)[2],
        (&b"Tf"[..], vec![&b"/F1"[..], b"24"]),
        "the dictionary's tokens are content"
    );
    assert_eq!(toks(b"BI foo"), [Tok::Op(&b"BI"[..]), Tok::Op(b"foo")]);

    let s = b"BI /W 2 /H 1 /BPC 8 /CS /G 7 /F /AHx xy EI";
    let read = operations_of(s, &res, &Limits::DEFAULT);
    let image = read[0].as_ref().unwrap().inline_image().unwrap();
    assert_eq!(&s[image.dict()], b" /W 2 /H 1 /BPC 8 /CS /G ");
    assert_eq!(&s[image.data()], b"/F");
    assert_eq!(image.entry(b"F"), None, "past what ends it");
    assert_eq!(image.entry(b"W").unwrap().bytes(), b"2");
    let s = b"BI /W 1 /H 1 /BPC 8 /CS /G /CS foo /X 1 /X ) /Y 2 ID a EI";
    let read = operations_of(s, &res, &Limits::DEFAULT);
    let image = read[0].as_ref().unwrap().inline_image().unwrap();
    assert_eq!(
        &s[image.dict()],
        b" /W 1 /H 1 /BPC 8 /CS /G /CS foo /X 1 /X ) /Y 2 "
    );
    assert_eq!(
        (image.entry(b"CS"), image.entry(b"ColorSpace")),
        (None, None)
    );
    assert_eq!(
        (image.entry(b"X"), image.entry(b"Y").map(|y| y.bytes())),
        (None, Some(&b"2"[..]))
    );
    // `ID` as a value, then an object.
    let s = b"BI /W 1 /H 1 /BPC 8 /CS ID (k) x EI";
    let read = operations_of(s, &res, &Limits::DEFAULT);
    let image = read[0].as_ref().unwrap().inline_image().unwrap();
    assert_eq!(&s[image.dict()], b" /W 1 /H 1 /BPC 8 /CS ID ");
    assert_eq!((&s[image.data()], image.entry(b"CS")), (&b"x"[..], None));
}

/// Where ISO 32000-2 reads an inline image's dictionary otherwise than
/// PDFium structures it, told: an object where a key is written ends it
/// here (`DictEnd`, after the object's own faults and the image's keys');
/// a keyword there makes the `BI` no image (`LoneBi`, from the `BI` to the
/// keyword, told as reading passes the keyword, after what's read as
/// content before it, and for each `BI` it makes one, a `BI` read as a
/// value among them); an `ID` as a value is the key's (`NoValue`). No `ID`
/// ends a dictionary an object ends, so nothing is told of one.
#[test]
fn an_inline_images_dictionary_read_otherwise_is_told() {
    let ends = |w: &str| format!("{w} ends an inline image's dictionary, where a key is written");
    let lone = |w: &str| format!("{w} begins no inline image, a keyword where a key is written");
    let s = b"BI /W 1 /H 1 /BPC 8 /CS /G --5 x EI";
    assert_eq!(
        told(s),
        [
            (
                place(s, b"--5"),
                "--5 is a number here and a keyword to ISO 32000-2".to_string()
            ),
            (place(s, b"--5"), ends("--5"))
        ]
    );
    let s = b"BI /W 1 /H 1 /BPC 8 /CS /G (k)x EI";
    assert_eq!(told(s), [(place(s, b"(k)"), ends("(k)"))]);
    let s = b"BI /W 1 /H 1 /BPC 8 /CS /G /ColorSpace /RGB 7 abc EI";
    let words = "an inline image's ColorSpace is read from its full name, and by ISO 32000-2 \
                 from its abbreviation";
    assert_eq!(
        told(s),
        [
            (
                place(s, b" /W 1 /H 1 /BPC 8 /CS /G /ColorSpace /RGB "),
                words.to_string()
            ),
            (place(s, b"7"), ends("7"))
        ]
    );
    let s = b"BI /W 1 /H 1 /BPC 8 /CS ID 7 x EI";
    assert_eq!(
        told(s),
        [
            (place(s, b"/CS"), "the key /CS has no value".to_string()),
            (place(s, b"7"), ends("7"))
        ]
    );
    let s = b"BI /W --5 foo q";
    assert_eq!(
        told(s),
        [
            (
                place(s, b"--5"),
                "--5 is a number here and a keyword to ISO 32000-2".to_string()
            ),
            (place(s, b"BI /W --5 foo"), lone("BI /W --5 foo"))
        ]
    );
    let s = b"BI BI foo";
    assert_eq!(told(s), [(0..5, lone("BI BI")), (3..9, lone("BI foo"))]);
    let s = b"BI /A BI /B BI foo";
    assert_eq!(
        told(s),
        [
            (0..18, lone("BI /A BI /B BI foo")),
            (6..18, lone("BI /B BI foo")),
            (12..18, lone("BI foo"))
        ]
    );
    let s = b"BI /W 1 BI /W 1 /H 1 /BPC 8 /CS /G ID x EI";
    assert_eq!(told(s), [(0..10, lone("BI /W 1 BI"))]);
    // Told as reading passes the keyword: before an error after it, and
    // not where an error stops reading before it.
    let s = b"BI foo (";
    let mut kept = Collected::default();
    let read: Vec<_> = tokens(s, &ResCx::none(), &Limits::DEFAULT)
        .with_sink(&mut kept)
        .collect();
    assert_eq!(read.last().unwrap().as_ref().err().map(|e| e.at), Some(7));
    assert_eq!(kept.0, [(0..6, Fault::LoneBi { written: &s[..6] })]);
    // An image whose data doesn't end: its dictionary's tokens' own
    // faults told as they're read, before the error; and none of what
    // ends it, which the image tells when it's read.
    let s = b"BI /W <4G> 7";
    let mut kept = Collected::default();
    let read: Vec<_> = tokens(s, &ResCx::none(), &Limits::DEFAULT)
        .with_sink(&mut kept)
        .collect();
    assert_eq!(read.last().unwrap().as_ref().err().map(|e| e.at), Some(0));
    assert_eq!(kept.0, [(6..10, Fault::HexString { written: &s[6..10] })]);
    let s = b"BI /a /b /c /d foo";
    let small = Limits {
        operands: 3,
        ..Limits::DEFAULT
    };
    let mut kept = Collected::default();
    let read: Vec<_> = tokens(s, &ResCx::none(), &small)
        .with_sink(&mut kept)
        .collect();
    assert_eq!(read.last().unwrap().as_ref().err().map(|e| e.at), Some(12));
    assert_eq!(kept.0, []);
}

/// PDFium reads 0x80 and 0xFF as white-space, beside ISO 32000-2's six
/// bytes, in content as everywhere (PDFium 7881 and 8076 alike: `1 2 3
/// 4<80>re f` fills the rectangle, `/F0<80>10 Tf` sets the font `F0`, and
/// either after `ID` is the one white-space byte before an inline image's
/// data): a word or a name ends at either, and it's passed over between
/// tokens, in an array, and after `ID` or before `EI`; in a string or a
/// comment it's a byte like any other. To ISO 32000-2 they're regular
/// (7.2.3), so `ID` with one after it has no white-space there.
#[test]
fn pdfiums_white_space_has_0x80_and_0xff() {
    use Tok::{Op, Operand};
    assert_eq!(
        toks(b"1 2 3 4\x80re\xfff"),
        [
            Operand(&b"1"[..]),
            Operand(b"2"),
            Operand(b"3"),
            Operand(b"4"),
            Op(b"re"),
            Op(b"f")
        ]
    );
    assert_eq!(
        toks(b"/F0\x8010\xffTf"),
        [Operand(&b"/F0"[..]), Operand(b"10"), Op(b"Tf")]
    );
    let shown = ops(b"[(a)\x80-250\xff(b)]TJ");
    assert_eq!(shown, [(&b"TJ"[..], vec![&b"[(a)\x80-250\xff(b)]"[..]])]);
    let tj = operations(b"[(a)\x80-250\xff(b)]TJ", &ResCx::none(), &Limits::DEFAULT)
        .next()
        .unwrap()
        .unwrap();
    let elements: Vec<_> = tj.arg(0).unwrap().elements().map(|e| e.bytes()).collect();
    assert_eq!(elements, [&b"(a)"[..], b"-250", b"(b)"]);
    assert_eq!(
        toks(b"(\x80\xff)Tj %\x80\nQ"),
        [Operand(&b"(\x80\xff)"[..]), Op(b"Tj"), Op(b"Q")]
    );
    assert_tiles(b"q\x80\xff Q\x80", &ResCx::none(), &Limits::DEFAULT);

    let s = b"BI /W 1 /H 1 /BPC 8 /CS /G ID\x80x EI";
    assert_eq!(image(s, &ResCx::none()).1, b"x");
    let read = operations(s, &ResCx::none(), &Limits::DEFAULT).next();
    let found = read.unwrap().unwrap().inline_image().unwrap();
    assert_eq!((found.dict(), found.data()), (2..27, 30..31));
    assert_eq!(
        told(s),
        [(
            27..29,
            "an inline image's ID has no white-space after it".to_string()
        )]
    );
    // Its tokens read to `EI`, the byte before it left out of its data.
    let s = b"BI /F /Xyz ID x y\xffEI";
    assert_eq!(image(s, &ResCx::none()).1, b"x y");
    // In a hex string, a byte ISO 32000-2 doesn't allow there.
    let words = "the hex string <41\u{fffd}42> has a byte that's no hex digit";
    assert_eq!(told(b"<41\x8042> Tj"), [(0..7, words.to_string())]);
    // A comment at a stream's end that reads the next stream's first line
    // as comment, which to ISO 32000-2 holds a token.
    let page = Concat::new(&[b"q %c", b"\x80\nQ"]);
    let across = Fault::Across {
        written: b"%c \x80",
    };
    assert_eq!(faults(&page), [(2..6, across)]);
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
    assert_eq!(at(b"Q BI /F /AHx ID 0E"), (2, InlineImage));
    assert_eq!(at(b"BI /F /A85 ID 9jqo~> 1"), (0, InlineImage));
    // The tokens before an error come first.
    let lexed: Vec<_> = tokens(b"q (a", &ResCx::none(), lim).collect();
    assert_eq!(lexed.len(), 2);
    assert_eq!(lexed[0], Ok((0..1, Tok::Op(b"q"))));
    assert!(lexed[1].is_err());
}

/// An array or a dictionary is one token to the closer that balances its
/// opener, what a string or a comment inside holds closing nothing, and a
/// closer of the other kind passed over. One that doesn't end is an error
/// where the last opener still open is, or where a string inside that
/// doesn't end starts; nested past the cap, where the first opener past it
/// is.
#[test]
fn an_array_or_a_dictionary_ends_where_its_tokens_close_it() {
    use ErrorKind::*;
    use Tok::{Op, Operand};
    assert_eq!(toks(b"[(])] TJ"), [Operand(&b"[(])]"[..]), Op(b"TJ")]);
    assert_eq!(
        toks(b"[1 %]\n2] TJ"),
        [Operand(&b"[1 %]\n2]"[..]), Op(b"TJ")]
    );
    assert_eq!(
        toks(b"<</A <4>]>>>> BDC"),
        [Operand(&b"<</A <4>]>>"[..]), Operand(b">>"), Op(b"BDC")]
    );
    assert_eq!(toks(b"[1 >> 2] q"), [Operand(&b"[1 >> 2]"[..]), Op(b"q")]);
    let lim = &Limits::DEFAULT;
    let at = |s: &[u8], lim: &Limits| {
        let e = error(s, lim);
        (e.at, e.kind)
    };
    assert_eq!(at(b"[1 <41", lim), (3, HexString));
    assert_eq!(at(b"[1 [2", lim), (3, Array));
    assert_eq!(at(b"<</A [1", lim), (5, Array));
    assert_eq!(at(b"[<</A 1", lim), (1, Dict));
    assert_eq!(at(b"[1 %]", lim), (0, Array));
    assert_eq!(at(b"<<<</A [1]>>>>", &deep(2)), (7, Depth));
    assert_eq!(at(b"[1]", &deep(0)), (0, Depth));
}

/// An array or a dictionary skimmed ends, or fails, where a walk of every
/// token it holds does, under caps on depth of none to three and the
/// default; and an inline image's markers are found where a look at every
/// window, or for the `EI` between white-space a walk of the bytes, finds
/// them: from every offset of the contents above, and of those that hold
/// an image's ends.
#[test]
fn skims_and_searches_find_what_walks_find() {
    use crate::data::{ei_scan, find};
    use crate::scan::{next_raw, next_raw_walked};
    let contents: [&[u8]; 14] = [
        b"[(])] TJ",
        b"[1 %]\n2] TJ",
        b"<</A <4>]>>>> BDC",
        b"[1 >> 2] q",
        b"[1 <41",
        b"[1 [2",
        b"<</A [1",
        b"[<</A 1",
        b"[1 %]",
        b"<<<</A [1]>>>> [[]] <<>>",
        b"[(a\\) <4G1> (b(c)) /N\x80[1] {}] >>> <<<",
        b"BI /F /AHx ID 41> EI BI /F /DCT ID x\xff\xd9 EI",
        b"xEI EI\tEI(EI/EI%EIEI EIx",
        b"\xff\xff\xd9\xd9>> >",
    ];
    let read = |r: Option<Result<(Range<usize>, scan::Raw), Error>>| r.map(|r| r.map(|(at, _)| at));
    for s in contents {
        for at in 0..=s.len() + 1 {
            for depth in [0, 1, 2, 3, Limits::DEFAULT.depth] {
                let skimmed = read(next_raw(s, at, depth));
                assert_eq!(
                    skimmed,
                    read(next_raw_walked(s, at, depth)),
                    "{s:?} from {at}"
                );
            }
            for needle in [&b"EI"[..], b">", b"\xff\xd9"] {
                let windows = s.get(at..).and_then(|rest| {
                    let found = rest.windows(needle.len()).position(|w| w == needle);
                    found.map(|n| at + n)
                });
                assert_eq!(find(s, at, needle), windows, "{s:?}: {needle:?} from {at}");
            }
            let walked = (at..s.len().saturating_sub(1)).find(|&i| {
                let space = |c: u8| b"\0\t\n\x0c\r ".contains(&c);
                let before = i > 0 && space(s[i - 1]);
                let after = s
                    .get(i + 2)
                    .is_none_or(|&c| space(c) || b"()<>[]{}/%".contains(&c));
                &s[i..i + 2] == b"EI" && before && after
            });
            assert_eq!(ei_scan(s, at), walked, "{s:?}: EI from {at}");
        }
        crate::fuzz::searched(s);
    }
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

/// Content of many inline images whose data doesn't end is read once,
/// for each filter that ends it: a JPEG's end of image, ASCIIHex's `>`,
/// the byte ASCII85 data stops at and RunLength's end-of-data byte are
/// searched for from the first image, not from each.
#[test]
fn unended_images_dont_search_again() {
    for unit in [
        &b"BI /F /DCT ID x EI "[..],
        b"BI /F /AHx ID x EI ",
        b"BI /F /A85 ID 9 EI ",
        b"BI /F /RL ID x EI ",
    ] {
        let s = unit.repeat(200_000);
        let started = std::time::Instant::now();
        let read = tokens(&s, &ResCx::none(), &Limits::DEFAULT).count();
        assert_eq!(read, 200_000, "{}", String::from_utf8_lossy(unit));
        let took = started.elapsed();
        let unit = String::from_utf8_lossy(unit);
        assert!(took < std::time::Duration::from_secs(2), "{unit}: {took:?}");
    }
}

/// The searches for where an inline image's data ends, remembered, find
/// what plain ones find, from every offset, in order, in reverse and
/// leaping about: RunLength's walk to its end-of-data byte, where ASCII85
/// data stops, ASCIIHex's `>` and a JPEG's end of image.
#[test]
fn remembered_searches_find_what_plain_ones_find() {
    use crate::data::{RunLength, Searched, a85_stop, find};
    // Bytes a RunLength walk, and the other searches, part on.
    let mut random = Vec::new();
    let mut x = 0x2545_f491_u32;
    for _ in 0..600 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        random.push(b"\x00\x01\x02\x05\x7f\x80\x81\xfe\xd9\xff>~9 z"[x as usize % 15]);
    }
    let contents: [&[u8]; 6] = [
        b"\x02\x80ab\x80\x01x\x80",
        b"\x00a\x81b\x05abcdef\x80\x00",
        b"\x01\x01\x01\x01\x01\x01\x01\x01\x80",
        b"9 EI ~> 9 x z EI 9",
        b"41 > 42> EI \xff\xff\xd9 \xd9",
        &random,
    ];
    for s in contents {
        let walk = |mut at: usize| {
            while let Some(&n) = s.get(at) {
                at += match n {
                    0..=127 => usize::from(n) + 2,
                    128 => return Some(at + 1),
                    _ => 2,
                };
            }
            None
        };
        let n = s.len() + 2;
        let leaps = (0..n).map(|i| i * 7 % n);
        let orders: [Vec<usize>; 3] = [(0..n).collect(), (0..n).rev().collect(), leaps.collect()];
        for order in orders {
            let mut run_length = RunLength::default();
            let mut searched: [Searched; 3] = Default::default();
            for at in order {
                assert_eq!(
                    run_length.end(s, at),
                    walk(at),
                    "{s:?}: RunLength from {at}"
                );
                let plain = [a85_stop(s, at), find(s, at, b">"), find(s, at, b"\xff\xd9")];
                for (k, plain) in plain.into_iter().enumerate() {
                    let remembered = searched[k].from(at, |at| match k {
                        0 => a85_stop(s, at),
                        1 => find(s, at, b">"),
                        _ => find(s, at, b"\xff\xd9"),
                    });
                    assert_eq!(remembered, plain, "{s:?}: search {k} from {at}");
                }
            }
        }
        crate::fuzz::searched(s);
    }
}

/// Content of many `BI`s that begin no inline image, each a value in the
/// dictionary of the one before, is read once: each is lone with the
/// keyword that ends them all, its dictionary not read on to it again.
#[test]
fn lone_bis_dont_read_again() {
    let mut s = b"BI /A ".repeat(50_000);
    s.extend_from_slice(b"BI foo");
    let started = std::time::Instant::now();
    let (res, mut kept) = (ResCx::none(), Collected::default());
    let read = tokens(&s, &res, &Limits::DEFAULT).with_sink(&mut kept);
    assert_eq!(read.count(), 100_002);
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    let lone: Vec<_> = (0..=50_000)
        .map(|i| {
            let at = 6 * i..s.len();
            (at.clone(), Fault::LoneBi { written: &s[at] })
        })
        .collect();
    assert_eq!(kept.0, lone);
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
    // No more than a place in them can count to, whatever the cap.
    let uncapped = Limits {
        content: usize::MAX,
        ..Limits::DEFAULT
    };
    assert_eq!(Join::new(&uncapped).room(), u32::MAX as usize);
    // A place past a stream, however far, is nowhere.
    let page = Concat::new(&streams);
    for at in [1001, u32::MAX] {
        assert_eq!(page.offset(Pos { stream: 1, at }), None);
    }
    assert_eq!(
        page.offset(Pos {
            stream: u32::MAX,
            at: 0
        }),
        None
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
    both::<Balance>();
    both::<Properties>();
    both::<Marked>();
    both::<MarkedNest>();
    both::<NameSet>();
    both::<Kept>();
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
    let s = b"/Im 0 BI /W 4 /H 1 /BPC 8 /CS /G /C#53 /RGB /L 4 ID abcd EI Q";
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
        b" /W 4 /H 1 /BPC 8 /CS /G /C#53 /RGB /L 4 "
    );
    // The entry under a key, either spelling of one 8.9.7 abbreviates, its
    // key matched as written, as its data was found by: `/C#53` isn't
    // `/CS` to PDFium.
    let entry = |key: &[u8]| image.entry(key).map(|o| o.bytes());
    assert_eq!(entry(b"CS"), Some(&b"/G"[..]));
    assert_eq!(entry(b"ColorSpace"), Some(&b"/G"[..]));
    assert_eq!(entry(b"C#53"), Some(&b"/RGB"[..]));
    assert_eq!(entry(b"Width"), Some(&b"4"[..]));
    assert_eq!(entry(b"Decode"), None);
    let w = image.entry(b"W").unwrap();
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
    assert_eq!(o[9].object(&deep(8)), None);
    assert_eq!(o[0].name().as_deref(), Some(&b"A B"[..]));
    assert_eq!(o[1].name(), None);
    assert_eq!(o[1].object(&deep(8)), Some(Object::from(12)));
    assert_eq!(
        o[5].object(&deep(8)),
        Some(Object::from(dict! { "K" => 1 }))
    );
    assert_eq!(o[5].object(&deep(0)), None);

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
    let read: Vec<_> = elements.iter().filter_map(|e| e.object(&deep(7))).collect();
    assert_eq!(array.object(&deep(8)), Some(Object::from(read)));
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

/// The faults reading `page`'s operations tells, with no resources, in
/// order: each where it is, and the fault.
fn faults<'a>(page: &'a Concat) -> Vec<(Range<usize>, Fault<'a>)> {
    let mut kept = Collected::default();
    let none = ResCx::none();
    let read = page
        .operations(&none, &Limits::DEFAULT)
        .with_sink(&mut kept);
    read.for_each(drop);
    kept.0
}

/// `words` joined by spaces, and where each is.
fn spaced(words: &[&[u8]]) -> (Vec<u8>, Vec<Range<usize>>) {
    let mut s = Vec::new();
    let mut places = Vec::new();
    for word in words {
        if !s.is_empty() {
            s.push(b' ');
        }
        places.push(s.len()..s.len() + word.len());
        s.extend_from_slice(word);
    }
    (s, places)
}

/// Each fault's words.
fn words(faults: &[(Range<usize>, Fault)]) -> Vec<String> {
    faults.iter().map(|(_, f)| f.to_string()).collect()
}

/// A word of digits, signs and points is a number to PDFium, and a
/// keyword to ISO 32000-2 unless it's an integer or a real as 7.3.3
/// writes one; and a number past what's read here is one too.
#[test]
fn numbers_iso_doesnt_write_are_faults() {
    let huge = [b"1".repeat(400), b".5".to_vec()].concat();
    let (s, at) = spaced(&[
        b"--5",
        b"1.2.3",
        b"+",
        b".",
        b"5-",
        b"+-1",
        b"+5",
        b"-.5",
        b"5.",
        b"00987",
        b"-0",
        b"w",
        b"99999999999999999999",
        b"9223372036854775807",
        b"-9223372036854775808",
        b"9223372036854775808",
        &huge,
        b"1e5",
    ]);
    let page = Concat::new(&[&s]);
    let found = faults(&page);
    let number = |i: usize| {
        (
            at[i].clone(),
            Fault::Number {
                written: &s[at[i].clone()],
            },
        )
    };
    let past = |i: usize| {
        (
            at[i].clone(),
            Fault::OutOfRange {
                written: &s[at[i].clone()],
            },
        )
    };
    let long = |i: usize| {
        (
            at[i].clone(),
            Fault::Long {
                written: &s[at[i].clone()],
            },
        )
    };
    assert_eq!(
        found,
        [
            number(0),
            number(1),
            number(2),
            number(3),
            number(4),
            number(5),
            past(12),
            past(13),
            past(14),
            past(15),
            past(16),
            long(16)
        ]
    );
    assert_eq!(
        words(&found)[..2],
        [
            "--5 is a number here and a keyword to ISO 32000-2",
            "1.2.3 is a number here and a keyword to ISO 32000-2",
        ]
    );
    assert_eq!(
        words(&found)[6],
        "99999999999999999999 is past the numbers read here"
    );
    // Its first 40 bytes.
    assert_eq!(
        words(&found)[10],
        format!("{} is past the numbers read here", "1".repeat(40))
    );
}

/// Where ISO 32000-2 reads an operand otherwise than PDFium's content
/// parser: an integer outside the 32 bits PDFium reads, or a real past an
/// `f32`, as [`Fault::OutOfRange`]; a literal string with a CR written in
/// it, as [`Fault::StringCr`]; a number or a name past 255 bytes, or a
/// string whose bytes, read, are more than 32,767, as [`Fault::Long`].
/// None for a number PDFium reads as ISO does, a CR escaped, or a word or
/// a string at the most PDFium reads. Each a token's own, told by the
/// tokens, and inside an array or a dictionary, or as an inline image's
/// value, as it's read.
#[test]
fn operands_pdfium_reads_otherwise_are_told() {
    let f = |n: usize| [b"/".as_slice(), &b"F".repeat(n - 1)].concat();
    let zeros = |n: usize| [b"0".repeat(n - 1).as_slice(), b"2"].concat();
    let literal = |n: usize| [b"(", b"a".repeat(n).as_slice(), b")"].concat();
    let hex = |n: usize| [b"<", b"41".repeat(n).as_slice(), b">"].concat();
    let escapes = [b"(", b"\\101".repeat(32_768).as_slice(), b")"].concat();
    let (s, at) = spaced(&[
        b"4294967296",
        b"+2147483648",
        b"-2147483649",
        b"4294967295",
        b"2147483648",
        b"-2147483648",
        b"340282356779733661637539395458142568448.0",
        b"340282346638528859811704183484516925440.0",
        b"(a\rb)",
        b"(a\\\rb\\015)",
        b"(a\nb)",
        &f(255),
        &f(256),
        &zeros(255),
        &zeros(256),
        &literal(32_767),
        &literal(32_768),
        &hex(32_767),
        &hex(32_768),
        &escapes,
        b"Tj",
    ]);
    let page = Concat::new(&[&s]);
    let found = faults(&page);
    let w = |i: usize| &s[at[i].clone()];
    let expected = [
        (0, Fault::OutOfRange { written: w(0) }),
        (1, Fault::OutOfRange { written: w(1) }),
        (2, Fault::OutOfRange { written: w(2) }),
        (6, Fault::OutOfRange { written: w(6) }),
        (8, Fault::StringCr { written: w(8) }),
        (12, Fault::Long { written: w(12) }),
        (14, Fault::Long { written: w(14) }),
        (16, Fault::Long { written: w(16) }),
        (18, Fault::Long { written: w(18) }),
        (19, Fault::Long { written: w(19) }),
    ]
    .map(|(i, f)| (at[i].clone(), f));
    assert_eq!(found, expected);
    let said = [
        "the string (a\rb) has a CR written in it, read as written, where ISO 32000-2 reads LF"
            .to_string(),
        format!(
            "/{} is longer than PDFium reads, and read as its first part",
            "F".repeat(39)
        ),
    ];
    assert_eq!(words(&found)[4..6], said);

    // Inside an array, and as an inline image's value: as they're read.
    let s = b"[(a\rb) 4294967296] TJ BI /W 1 /H 1 /BPC 8 /CS /G /DP (c\rd) ID x EI";
    let read: Vec<_> = operations(s, &ResCx::none(), &Limits::DEFAULT)
        .collect::<Result<_, _>>()
        .unwrap();
    let mut kept = Collected::default();
    read[0]
        .arg(0)
        .unwrap()
        .object_with(&Limits::DEFAULT, &mut kept);
    let (cr, past) = (place(s, b"(a\rb)"), place(s, b"4294967296"));
    assert_eq!(
        kept.0,
        [
            (cr.clone(), Fault::StringCr { written: &s[cr] }),
            (past.clone(), Fault::OutOfRange { written: &s[past] }),
        ]
    );
    let image = place(s, b"(c\rd)");
    let page = Concat::new(&[s]);
    let told: Vec<_> = (faults(&page).into_iter())
        .filter(|(at, _)| at.start > read[0].range().end)
        .collect();
    assert_eq!(
        told,
        [(image.clone(), Fault::StringCr { written: &s[image] })]
    );

    // A key ISO 32000-2 decodes to an image's is told so; one only PDFium
    // would decode to one, `/B#5GC`, isn't, as ISO reads its `#` as
    // itself (and tells that, a token's own).
    let s = b"BI /W 8 /H 1 /B#5GC 8 /B#50C 8 /CS /G ID x EI";
    let (lax, hex) = (place(s, b"/B#5GC"), place(s, b"/B#50C"));
    let page = Concat::new(&[s]);
    let told: Vec<_> = (faults(&page).into_iter())
        .filter(|(at, _)| *at == lax || *at == hex)
        .collect();
    let name = Fault::Name {
        written: &s[lax.clone()],
        stray: true,
        nul: false,
    };
    let key = Fault::EscapedKey {
        written: &s[hex.clone()],
    };
    assert_eq!(told, [(lax, name), (hex, key)]);
}

/// A hex string of other bytes than hex digits and white-space, a name
/// with a `#` that escapes nothing or NUL, a closer with nothing open, and
/// a brace, are each a token's own fault.
#[test]
fn a_tokens_own_faults() {
    let (s, at) = spaced(&[
        b"<41 42\t\r\n>",
        b"<4G>",
        b"<41%c>",
        b"/A#41",
        b"/A#2",
        b"/A#00",
        b"/#zz#00",
        b"(not<4G>)",
        b"Tj",
        b"{",
        b"}",
        b")",
        b"]",
        b">>",
        b">",
        b"Q",
    ]);
    let page = Concat::new(&[&s]);
    let found = faults(&page);
    let w = |i: usize| &s[at[i].clone()];
    let name = |i: usize, stray, nul| Fault::Name {
        written: w(i),
        stray,
        nul,
    };
    let expected = [
        (1, Fault::HexString { written: w(1) }),
        (2, Fault::HexString { written: w(2) }),
        (4, name(4, true, false)),
        (5, name(5, false, true)),
        (6, name(6, true, true)),
        (9, Fault::Brace { written: w(9) }),
        (10, Fault::Brace { written: w(10) }),
        (11, Fault::Closer { written: w(11) }),
        (12, Fault::Closer { written: w(12) }),
        (13, Fault::Closer { written: w(13) }),
        (14, Fault::Closer { written: w(14) }),
    ]
    .map(|(i, f)| (at[i].clone(), f));
    assert_eq!(found, expected);
    assert_eq!(
        words(&found),
        [
            "the hex string <4G> has a byte that's no hex digit",
            "the hex string <41%c> has a byte that's no hex digit",
            "the name /A#2 has a # without two hex digits after it, which readers read differently",
            "the name /A#00 has #00, NUL, which a name can't hold, which readers read differently",
            "the name /#zz#00 has a # without two hex digits after it, and #00, NUL, which readers read differently",
            "{ is a delimiter here and a regular character to ISO 32000-2",
            "} is a delimiter here and a regular character to ISO 32000-2",
            ") closes nothing",
            "] closes nothing",
            ">> closes nothing",
            "> closes nothing",
        ]
    );
}

/// What's inside an array or a dictionary is told when it's read, each
/// element's, key's and value's own faults, a word that's no object, a
/// key that's no name and a key with no value, where each is in the
/// content; the operand's own faults are its token's.
#[test]
fn whats_inside_is_told_when_its_read() {
    let s: &[u8] = b"q [1 --5 R <4G> /A#zz ) { >> [/B#2] 99999999999999999999] \
        <</K 1 2 (v) [3] /V /N#00 ] /L R /M>> /C#2 m";
    let at = |w: &[u8], from: usize| {
        let i = from + s[from..].windows(w.len()).position(|x| x == w).unwrap();
        i..i + w.len()
    };
    let read: Vec<_> = operations(s, &ResCx::none(), &Limits::DEFAULT)
        .collect::<Result<_, _>>()
        .unwrap();
    let m = &read[1];
    let (array, dict, name) = (m.arg(2).unwrap(), m.arg(1).unwrap(), m.arg(0).unwrap());
    fn read_with(o: Operand<'_>) -> Vec<(Range<usize>, Fault<'_>)> {
        let mut kept = Collected::default();
        let object = o.object_with(&deep(8), &mut kept);
        assert_eq!(object, o.object(&deep(8)), "the same object");
        kept.0
    }
    let w = |r: &Range<usize>| &s[r.clone()];
    let fault = |r: Range<usize>, f: fn(&[u8]) -> Fault| (r.clone(), f(w(&r)));
    let a = array.range().start;
    assert_eq!(
        read_with(array),
        [
            fault(at(b"--5", a), |written| Fault::Number { written }),
            fault(at(b"R", a), |written| Fault::Keyword { written }),
            fault(at(b"<4G>", a), |written| Fault::HexString { written }),
            (
                at(b"/A#zz", a),
                Fault::Name {
                    written: w(&at(b"/A#zz", a)),
                    stray: true,
                    nul: false
                }
            ),
            fault(at(b")", a), |written| Fault::Closer { written }),
            fault(at(b"{", a), |written| Fault::Brace { written }),
            fault(at(b">>", a), |written| Fault::Closer { written }),
            (
                at(b"/B#2", a),
                Fault::Name {
                    written: w(&at(b"/B#2", a)),
                    stray: true,
                    nul: false
                }
            ),
            fault(at(b"99999999999999999999", a), |written| {
                Fault::OutOfRange { written }
            }),
        ]
    );
    let d = dict.range().start;
    assert_eq!(
        read_with(dict),
        [
            fault(at(b"2", d), |written| Fault::Key { written }),
            fault(at(b"(v)", d), |written| Fault::Key { written }),
            fault(at(b"[3]", d), |written| Fault::Key { written }),
            (
                at(b"/N#00", d),
                Fault::Name {
                    written: w(&at(b"/N#00", d)),
                    stray: false,
                    nul: true
                }
            ),
            fault(at(b"] /L", d).start..at(b"] /L", d).start + 1, |written| {
                Fault::Closer { written }
            }),
            fault(at(b"R", d), |written| Fault::Keyword { written }),
            fault(at(b"/M", d), |written| Fault::NoValue { written }),
        ]
    );
    // A name's own fault is its token's, told by the operations.
    assert_eq!(read_with(name), []);
    let page = Concat::new(&[s]);
    assert_eq!(
        faults(&page),
        [fault(at(b"/C#2", d), |written| Fault::Name {
            written,
            stray: true,
            nul: false
        })]
    );
    let words = words(&read_with(dict));
    assert_eq!(
        [&words[0], &words[5], &words[6]],
        [
            "2 is a dictionary's key and not a name",
            "R is no object, where one is written",
            "the key /M has no value"
        ]
    );
}

/// A string, an inline image or a comment that runs from one of a page's
/// streams into the next is told, over the page's tokens and its
/// operations; a comment only where the next stream's first line has a
/// token it reads as comment. A string or a comment inside an array or a
/// dictionary is told where it is; an array, a dictionary or an inline
/// image's dictionary divided between its tokens isn't told.
#[test]
fn a_token_across_streams() {
    let across = |streams: &[&[u8]]| {
        let page = Concat::new(streams);
        let found: Vec<_> = faults(&page)
            .into_iter()
            .map(|(at, f)| {
                let Fault::Across { written } = f else {
                    panic!("{f:?}")
                };
                assert_eq!(written, &page.bytes()[at.clone()]);
                (at, f.to_string())
            })
            .collect();
        // The tokens tell the same.
        let mut kept = Collected::default();
        let none = ResCx::none();
        let tokens = page.tokens(&none, &Limits::DEFAULT).with_sink(&mut kept);
        tokens.for_each(drop);
        let words: Vec<_> = kept
            .0
            .iter()
            .map(|(at, f)| (at.clone(), f.to_string()))
            .collect();
        assert_eq!(words, found);
        found
    };
    let one = |at: Range<usize>, words: &str| vec![(at, words.to_string())];
    assert_eq!(
        across(&[b"q (ab", b"c) Tj Q"]),
        one(2..8, "(ab c) runs from one stream into the next")
    );
    assert_eq!(
        across(&[b"BI /W 4 /H 1 /BPC 8 /CS /G ID ab", b"c EI Q"]),
        one(
            0..37,
            "BI /W 4 /H 1 /BPC 8 /CS /G ID ab c EI runs from one stream into the next"
        )
    );
    // A comment at a stream's end with no end of line, reading the next
    // stream's `Q` as comment, past an empty stream too.
    assert_eq!(
        across(&[b"q %c", b"Q"]),
        one(2..6, "%c Q runs from one stream into the next")
    );
    assert_eq!(
        across(&[b"q %c", b"", b" Q"]),
        one(2..8, "%c   Q runs from one stream into the next")
    );
    // A string across two joins is one.
    assert_eq!(across(&[b"q (a", b"b", b"c) Tj"]).len(), 1);
    // None where ISO reads alike: a comment that reads only white-space or
    // a comment as comment, an end of line before the join, operands in
    // one stream and their operator in the next, a stream that's a token.
    for streams in [
        &[&b"q %c"[..], b"  % more\nQ"][..],
        &[b"q %c", b"\nQ"],
        &[b"q %c\n", b"Q"],
        &[b"q %c\r", b"Q"],
        &[b"0 0", b"m 1 1 l", b"S"],
        &[b"q", b"(a) Tj", b"Q"],
        &[b"q %c", b" "],
        // Between an array's, a dictionary's or an image's dictionary's
        // tokens, which ISO lets streams divide.
        &[b"/Span <</MCID", b"0>> BDC EMC"],
        &[b"/Span <<", b"/MCID 0>> BDC EMC"],
        &[b"/P <</A <</B (b)", b">> /C [1 2]>> BDC EMC"],
        &[b"[", b"(a) 1 (b)] TJ"],
        &[b"[(a) 1", b"(b)] TJ"],
        &[b"[(a)", b"<62>] TJ"],
        &[b"BI /W 1 /H 1", b"/BPC 8 /CS /G ID x EI"],
        &[b"BI /W 1 /H 1 /BPC 8 /CS /G ID", b"x EI"],
        &[b"BI /W 1 /H 1 /BPC 8 /CS /G ID x", b"EI"],
        // A comment inside an array that reads only white-space.
        &[b"[(a) %c", b"  \n(b)] TJ"],
    ] {
        assert_eq!(across(streams), [], "{streams:?}");
    }
    // Inside a string inside an array or a dictionary, a hex string's
    // too, told where the string is, each string divided once.
    let words = " runs from one stream into the next";
    let strings = |streams: &[&[u8]], strings: &[&str]| {
        let page = Concat::new(streams);
        let told: Vec<_> = strings
            .iter()
            .map(|w| (place(page.bytes(), w.as_bytes()), format!("{w}{words}")))
            .collect();
        assert_eq!(across(streams), told, "{streams:?}");
    };
    strings(&[b"[(a", b"b)] TJ"], &["(a b)"]);
    strings(&[b"[(a", b"b) 1 (c", b"d)] TJ"], &["(a b)", "(c d)"]);
    strings(&[b"[<6", b"2>] TJ"], &["<6 2>"]);
    strings(&[b"/Span <</Lang (en", b"-AU)>> BDC EMC"], &["(en -AU)"]);
    strings(&[b"/P <</A [(x) (y", b"z)]>> BDC EMC"], &["(y z)"]);
    // A comment inside an array reading the next stream's first line,
    // the array closed on the line after.
    strings(&[b"[(a) %c", b"(b)]\n(c)] TJ"], &["%c (b)]"]);
    // An inline image divided in a string or a comment in its dictionary,
    // or in what's between its data and its `EI`, is told whole.
    let image = |streams: &[&[u8]]| {
        let page = Concat::new(streams);
        let found: Vec<_> = faults(&page)
            .into_iter()
            .filter(|(_, f)| matches!(f, Fault::Across { .. }))
            .map(|(at, _)| at)
            .collect();
        let at = page.bytes().iter().position(|&c| c == b'B').unwrap();
        let end = place(page.bytes(), b"EI").end;
        assert_eq!(found, vec![at..end], "{streams:?}");
    };
    image(&[b"BI /W 1 /H 1 /BPC 8 /CS /G /X (a", b"b) ID x EI"]);
    image(&[b"BI /W 1 /H 1 %c", b"/W 2\n/BPC 8 /CS /G ID x EI"]);
    image(&[b"BI /W 1 /H 1 /BPC 8 /CS /G ID x (a", b"b) EI"]);
    // A page of many streams is read once: each join isn't read back from.
    let empty = vec![&b""[..]; 200_000];
    let streams = [&[&b"q"[..]][..], &empty, &[b"Q %c"], &empty, &[b"Q"]].concat();
    let page = Concat::new(&streams);
    let started = std::time::Instant::now();
    let found = faults(&page);
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    let percent = page.bytes().iter().position(|&c| c == b'%').unwrap();
    let comment = percent..page.bytes().len();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, comment);
    // Read whole, there are no streams to cross.
    let page = Concat::new(&[b"q (ab", b"c) Tj Q"]);
    let mut kept = Collected::default();
    let none = ResCx::none();
    tokens(page.bytes(), &none, &Limits::DEFAULT)
        .with_sink(&mut kept)
        .for_each(drop);
    assert_eq!(kept.0, []);
}

/// Operands at the end that no operator takes are told once, where they
/// are, with how many; not before an error, which ends them otherwise.
#[test]
fn operands_at_the_end_are_a_fault() {
    let at_end = |streams: &[&[u8]]| {
        let page = Concat::new(streams);
        let found: Vec<_> = faults(&page)
            .into_iter()
            .map(|(at, f)| (at, f.to_string()))
            .collect();
        found
    };
    let one = |at: Range<usize>, count: usize| {
        let words = format!("{count} operands at the end that no operator takes");
        vec![(at, words)]
    };
    assert_eq!(at_end(&[b"q 1 2 % no operator"]), one(2..5, 2));
    assert_eq!(at_end(&[b"1"]), one(0..1, 1));
    assert_eq!(at_end(&[b"q 1", b"0 0 RG Q 9"]), one(13..14, 1));
    assert_eq!(at_end(&[b"q 1 2 (open"]), []);
    assert_eq!(at_end(&[b"q 0 0 m"]), []);
    // Told once, however often the end is asked for.
    let mut kept = Collected::default();
    let none = ResCx::none();
    let mut read = operations(b"q 1", &none, &Limits::DEFAULT).with_sink(&mut kept);
    assert!(read.next().is_some());
    assert!(read.next().is_none() && read.next().is_none());
    assert_eq!(kept.0, [(2..3, Fault::Trailing { count: 1 })]);
}

/// `content`'s faults, read whole with no error, and each's words.
fn told(content: &[u8]) -> Vec<(Range<usize>, String)> {
    let mut kept = Collected::default();
    let none = ResCx::none();
    operations(content, &none, &Limits::DEFAULT)
        .with_sink(&mut kept)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    kept.0
        .into_iter()
        .map(|(at, f)| (at, f.to_string()))
        .collect()
}

/// Where `w` first is in `s`.
fn place(s: &[u8], w: &[u8]) -> Range<usize> {
    let at = s.windows(w.len()).position(|x| x == w).expect("there");
    at..at + w.len()
}

/// An inline image's length that ends its data elsewhere, its `ID` with
/// no white-space after it, tokens between its data and its `EI`, and its
/// dictionary's tokens and entries, are told as the image is read.
#[test]
fn an_inline_images_faults() {
    let one = |at: Range<usize>, words: &str| vec![(at, words.to_string())];
    // Its data counted to 4 bytes, where its length says 3.
    let s = b"BI /W 4 /H 1 /BPC 8 /CS /G /L 3 ID abcd EI";
    assert_eq!(
        told(s),
        one(
            place(s, b"abcd"),
            "an inline image's data is read as 4 bytes, where its length says 3"
        )
    );
    // ASCIIHexDecode the last of its filters, or its one: its length
    // counted past more white-space after `ID`. Its data found by its
    // first filter's end where that's ASCIIHexDecode, and at the first
    // `EI` between white-space where that's Flate.
    assert_eq!(told(b"BI /F /AHx /L 3 ID   0E> EI"), []);
    assert_eq!(told(b"BI /F [/Fl /AHx] /L 3 ID   0E> EI"), []);
    let s = b"BI /F [/Fl /AHx] /L 2 ID   0E> EI";
    assert_eq!(
        told(s),
        one(
            place(s, b"  0E>"),
            "an inline image's data is read as 3 bytes, where its length says 2"
        )
    );
    let s = b"BI /F [/AHx /Fl] /L 3 ID   0E> EI";
    assert_eq!(
        told(s),
        one(
            place(s, b"  0E>"),
            "an inline image's data is read as 5 bytes, where its length says 3"
        )
    );
    // A length that's its data's, and one past the content, read to the
    // first `EI` between white-space.
    assert_eq!(told(b"BI /F /DCT /L 4 ID \xff\xd8\xff\xd9 EI"), []);
    let s = b"BI /F /Fl /L 99 ID xy EI";
    assert_eq!(
        told(s),
        one(
            place(s, b"xy"),
            "an inline image's data is read as 2 bytes, where its length says 99"
        )
    );
    // `ID` with a delimiter after it, not white-space, unless the last of
    // its filters, or its one, is one of the two ASCII ones.
    let s = b"BI/W 2/H 1/BPC 8/CS/G ID[)EI";
    assert_eq!(
        told(s),
        one(
            place(s, b"ID"),
            "an inline image's ID has no white-space after it"
        )
    );
    assert_eq!(told(b"BI /F /AHx ID>EI"), []);
    assert_eq!(told(b"BI /F [/Fl /A85] ID<~~>\nEI"), []);
    let s = b"BI /F [/A85 /Fl] ID<9jqo~>EI";
    assert_eq!(
        told(s),
        one(
            place(s, b"ID"),
            "an inline image's ID has no white-space after it"
        )
    );
    // Past its counted data, tokens PDFium reads on past to its `EI`.
    let s = b"BI /W 1 /H 1 /BPC 8 /CS /G ID x 1 2 % EI\nEI Q";
    assert_eq!(
        told(s),
        one(
            place(s, b"1 2"),
            "1 2 is between an inline image's data and its EI"
        )
    );
    assert_eq!(told(b"BI /W 1 /H 1 /BPC 8 /CS /G ID xEI Q"), []);
    // Its dictionary: its tokens' own faults, a word that's no object, a
    // key whose value is `ID`, and an object where a key is written,
    // which ends it.
    let s = b"BI /W 1 /H 1 /BPC 8 /CS /G /A#zz foo /B ) /D <4G> /F#2 ID 7 x EI";
    let stray = "has a # without two hex digits after it, which readers read differently";
    assert_eq!(
        told(s),
        [
            (place(s, b"/A#zz"), format!("the name /A#zz {stray}")),
            (
                place(s, b"foo"),
                "foo is no object, where one is written".to_string()
            ),
            (place(s, b")"), ") closes nothing".to_string()),
            (
                place(s, b"<4G>"),
                "the hex string <4G> has a byte that's no hex digit".to_string()
            ),
            (place(s, b"/F#2"), format!("the name /F#2 {stray}")),
            (place(s, b"/F#2"), "the key /F#2 has no value".to_string()),
            (
                place(s, b"7"),
                "7 ends an inline image's dictionary, where a key is written".to_string()
            ),
        ]
    );
}

/// Faults come in the order of their ends, as reading passes them: an
/// operand's own before the operands at the end it's among, and those
/// before a comment after them that runs into the next stream.
#[test]
fn faults_come_in_the_order_of_their_ends() {
    let page = Concat::new(&[b"q --5 %c", b"Q"]);
    let found = faults(&page);
    let s = page.bytes();
    assert_eq!(
        found,
        [
            (2..5, Fault::Number { written: &s[2..5] }),
            (2..5, Fault::Trailing { count: 1 }),
            (6..10, Fault::Across { written: &s[6..10] }),
        ]
    );
    // An inline image's own before its running into the next stream.
    let page = Concat::new(&[b"BI /W 3 /H 1 /BPC 8 /CS /G /L 2 ID a", b"b EI Q"]);
    let s = page.bytes();
    let image = 0..place(s, b"EI").end;
    assert_eq!(
        faults(&page),
        [
            (place(s, b"a b"), Fault::ImageLength { given: 2, read: 3 }),
            (image.clone(), Fault::Across { written: &s[image] }),
        ]
    );
}

/// The contents of the moved tests: a page, the inline images of each of
/// PDFium's rules, and the fuzz target's seeds.
fn moved_inputs() -> Vec<Vec<u8>> {
    let images = [
        ("/W 4 /H 1 /BPC 8 /CS /G", &b" EI "[..]),
        ("/W 3 /H 2 /BPC 1 /CS /G", b"EI"),
        ("/IM true /W 10 /H 2 /D [1 0]", b" EI "),
        (
            "/W 2 /H 2 /BPC 8 /CS [/I /RGB 1 <000000FFFFFF>] /I true",
            b"EI E",
        ),
        (
            "/W 2 /H 1 /BPC 8 /CS [/DeviceN [/Cyan /Spot] /DeviceCMYK {}]",
            b" EI ",
        ),
        ("/W 4.9 /H 1.0 /BPC 8 /CS /G", b" EI "),
        ("/F /AHx /W 2 /H 2 /BPC 8 /CS /G", b"0E EI 0>"),
        ("/F [/A85 /Fl] /W 9 /H 9 /BPC 8 /CS /G", b"9jqo^ EI ~>"),
        ("/F /RL /W 4 /H 1 /BPC 8 /CS /G", b"\x03 EI \x80"),
        (
            "/F /DCT /W 9 /H 9 /BPC 8 /CS /RGB",
            b"\xff\xd8\xff\xe0 EI \x00\xff\xd9",
        ),
        ("/F /Fl /W 9 /H 9 /BPC 8 /CS /G /L 4", b"xEIy"),
        ("/F /LZW /Length 4 /W 9 /H 9", b"xEIy"),
        ("/F /CCF /DP <</K -1 /Columns 9>> /W 9 /H 9", b"xEI EIy"),
        ("/F /XYZ /W 9 /H 9", b"(x EI y)"),
    ];
    let mut inputs: Vec<Vec<u8>> = images
        .iter()
        .map(|(dict, data)| [format!("q BI {dict} ID ").as_bytes(), data, b"\nEI Q"].concat())
        .collect();
    inputs.push(PAGE.to_vec());
    inputs.extend(fixtures());
    inputs
}

/// Through a sink that keeps faults, the tokens and operations are those
/// through `()`, every operand reads the same, and a sink that wants none
/// is told none: on the moved tests' contents, read whole and as two
/// streams, and on content made of their pieces.
#[test]
fn faults_change_nothing() {
    let res = ResCx::new([(b"CS3".to_vec(), 3)]);
    let lim = &Limits::DEFAULT;
    let mut told = 0;
    for s in moved_inputs() {
        let (first, second) = s.split_at(s.len() / 2);
        for page in [Concat::new(&[&s]), Concat::new(&[first, second])] {
            faulted(&page, &res, lim);
            told += faults(&page).len();
        }
    }
    assert!(told > 0, "some faults told");
    let mut random = Random(0x6a09_e667_f3bc_c908);
    for _ in 0..512 {
        let s: Vec<u8> = (0..random.below(48))
            .flat_map(|_| PIECES[random.below(PIECES.len())])
            .copied()
            .collect();
        faulted(&Concat::new(&[&s]), &res, lim);
    }
}

/// A sink that doesn't want faults is told none, of any kind, through a
/// borrow too, and what's read is the same.
#[test]
fn a_sink_that_wants_none_is_told_none() {
    let s: &[u8] =
        b"q --5 <4G> /A#2 { ) <</K 1 2 /M>> BI /W 1 /H 1 /BPC 8 /CS /G /L 3 7 x 1 EI BI foo 1";
    let page = Concat::new(&[s, b"(a", b"b) Tj %c", b"Q\n1 2"]);
    let kinds: Vec<_> = faults(&page)
        .iter()
        .map(|(_, f)| {
            format!("{f:?}")
                .split([' ', '{'])
                .next()
                .unwrap()
                .to_string()
        })
        .collect();
    // A token's own, an inline image's, the joins' and the operands' at
    // the end.
    assert_eq!(
        kinds,
        [
            "Number",
            "HexString",
            "Name",
            "Brace",
            "Closer",
            "DictEnd",
            "ImageLength",
            "BeforeEi",
            "LoneBi",
            "Across",
            "Across",
            "Trailing"
        ]
    );
    let none = ResCx::none();
    let mut unwanted = Unwanted::default();
    let read: Vec<_> = page
        .operations(&none, &Limits::DEFAULT)
        .with_sink(&mut unwanted)
        .map(|op| format!("{op:?}"))
        .collect();
    let plain: Vec<_> = page
        .operations(&none, &Limits::DEFAULT)
        .map(|op| format!("{op:?}"))
        .collect();
    assert_eq!(read, plain);
    page.tokens(&none, &Limits::DEFAULT)
        .with_sink(&mut &mut unwanted)
        .for_each(drop);
    for op in page.operations(&none, &Limits::DEFAULT) {
        for operand in op.unwrap().operands() {
            operand.object_with(&deep(8), &mut unwanted);
        }
    }
    assert_eq!(unwanted.0, []);
    // `()` wants none, so none is looked for.
    assert!(!Sink::wants(&()));
}

/// A sink borrowed, and a `dyn` one, are sinks.
#[test]
fn a_dyn_sink_is_a_sink() {
    let s: &[u8] = b"q --5 <</K 1 2>> BDC 1";
    let none = ResCx::none();
    let mut kept = Collected::default();
    let sink: &mut dyn Sink = &mut kept;
    let read: Vec<_> = operations(s, &none, &Limits::DEFAULT)
        .with_sink(&mut *sink)
        .collect::<Result<_, _>>()
        .unwrap();
    read[1].arg(0).unwrap().object_with(&deep(8), sink);
    tokens(s, &none, &Limits::DEFAULT)
        .with_sink(&mut &mut *sink)
        .for_each(drop);
    let kinds: Vec<_> = kept
        .0
        .iter()
        .map(|(at, f)| (at.clone(), f.to_string()))
        .collect();
    assert_eq!(
        kinds,
        [
            (
                2..5,
                "--5 is a number here and a keyword to ISO 32000-2".to_string()
            ),
            (
                21..22,
                "1 operands at the end that no operator takes".to_string()
            ),
            (13..14, "2 is a dictionary's key and not a name".to_string()),
            (
                2..5,
                "--5 is a number here and a keyword to ISO 32000-2".to_string()
            ),
        ]
    );
}

/// Content ISO 32000-2 reads as it's read here has no fault: the moved
/// tests' well-formed contents, their operands read with a sink too.
#[test]
fn no_fault_where_iso_reads_alike() {
    let res = ResCx::new([(b"CS3".to_vec(), 3), (b"CS0".to_vec(), 3)]);
    let lim = &Limits::DEFAULT;
    // The page less its strays, the images, the seeds less the one with
    // strays, and pages of streams split between tokens.
    let page_end = PAGE.windows(5).position(|w| w == b"Q { }").unwrap() + 1;
    let mut inputs = moved_inputs();
    inputs.retain(|s| s != PAGE && !s.ends_with(b"{ } ) ]"));
    inputs.push(PAGE[..page_end].to_vec());
    inputs.push(b"q 1 0 0 1 72 720 cm /F1 12 Tf [(Hel) -20 (lo)] TJ Q".to_vec());
    for s in &inputs {
        let page = Concat::new(&[s]);
        let mut kept = Collected::default();
        let read: Vec<_> = page
            .operations(&res, lim)
            .with_sink(&mut kept)
            .collect::<Result<_, _>>()
            .unwrap();
        for op in &read {
            for operand in op.operands() {
                operand.object_with(lim, &mut kept);
            }
        }
        assert_eq!(kept.0, [], "{}", String::from_utf8_lossy(s));
    }
    let streams: [&[u8]; 3] = [b"q 1 0 0", b"1 0 0 cm /P <</MCID 0>> BDC (ab) Tj", b"EMC Q"];
    assert_eq!(faults(&Concat::new(&streams)), []);
}

/// Each operator of Annex A, as this test reads its clause: its name,
/// its operands (`n` a number, `i` an integer, `N` a name, `s` a string,
/// `a` an array, `D` a name or a dictionary, `-` none; `c` a number for
/// each component, `p` the same or a pattern's name), its category, and
/// the clause defining it. Written apart from the crate's table, in
/// another form, so each holds the other.
const OPERATORS: &str = "
    w n general 8.4.4 | J i general 8.4.4 | j i general 8.4.4 | M n general 8.4.4
    d an general 8.4.4 | ri N general 8.4.4 | i n general 8.4.4 | gs N general 8.4.4
    q - general 8.4.4 | Q - general 8.4.4 | cm nnnnnn special 8.4.4
    m nn construct 8.5.2.1 | l nn construct 8.5.2.1 | c nnnnnn construct 8.5.2.1
    v nnnn construct 8.5.2.1 | y nnnn construct 8.5.2.1 | h - construct 8.5.2.1
    re nnnn construct 8.5.2.1
    S - paint 8.5.3.1 | s - paint 8.5.3.1 | f - paint 8.5.3.1 | F - paint 8.5.3.1
    f* - paint 8.5.3.1 | B - paint 8.5.3.1 | B* - paint 8.5.3.1 | b - paint 8.5.3.1
    b* - paint 8.5.3.1 | n - paint 8.5.3.1 | W - clip 8.5.4 | W* - clip 8.5.4
    BT - text 9.4.1 | ET - text 9.4.1
    Tc n state 9.3.1 | Tw n state 9.3.1 | Tz n state 9.3.1 | TL n state 9.3.1
    Tf Nn state 9.3.1 | Tr i state 9.3.1 | Ts n state 9.3.1
    Td nn position 9.4.2 | TD nn position 9.4.2 | Tm nnnnnn position 9.4.2
    T* - position 9.4.2
    Tj s show 9.4.3 | TJ a show 9.4.3 | ' s show 9.4.3 | \" nns show 9.4.3
    d0 nn type3 9.6.4 | d1 nnnnnn type3 9.6.4
    CS N color 8.6.8 | cs N color 8.6.8 | SC c color 8.6.8 | sc c color 8.6.8
    SCN p color 8.6.8 | scn p color 8.6.8 | G n color 8.6.8 | g n color 8.6.8
    RG nnn color 8.6.8 | rg nnn color 8.6.8 | K nnnn color 8.6.8 | k nnnn color 8.6.8
    sh N shading 8.7.4.2 | BI - image 8.9.7 | ID - image 8.9.7 | EI - image 8.9.7
    Do N xobject 8.8.1
    MP N marked 14.6.1 | DP ND marked 14.6.1 | BMC N marked 14.6.1
    BDC ND marked 14.6.1 | EMC - marked 14.6.1 | BX - compat 7.8.2 | EX - compat 7.8.2";

/// `OPERATORS`' rows: name, operands, category, clause.
fn operator_rows() -> Vec<(&'static str, Operands, Category, &'static str)> {
    let types = |code: &str| -> Operands {
        let fixed: Vec<Type> = match code {
            "c" => return Operands::Components,
            "p" => return Operands::ComponentsOrPattern,
            "-" => Vec::new(),
            _ => code
                .chars()
                .map(|c| match c {
                    'n' => Type::Number,
                    'i' => Type::Integer,
                    'N' => Type::Name,
                    's' => Type::String,
                    'a' => Type::Array,
                    'D' => Type::NameOrDict,
                    _ => panic!("{c}"),
                })
                .collect(),
        };
        Operands::Fixed(fixed.leak())
    };
    let category = |word: &str| match word {
        "general" => Category::GeneralGraphicsState,
        "special" => Category::SpecialGraphicsState,
        "construct" => Category::PathConstruction,
        "paint" => Category::PathPainting,
        "clip" => Category::ClippingPath,
        "text" => Category::TextObject,
        "state" => Category::TextState,
        "position" => Category::TextPositioning,
        "show" => Category::TextShowing,
        "type3" => Category::Type3Font,
        "color" => Category::Color,
        "shading" => Category::Shading,
        "image" => Category::InlineImage,
        "xobject" => Category::XObject,
        "marked" => Category::MarkedContent,
        "compat" => Category::Compatibility,
        _ => panic!("{word}"),
    };
    OPERATORS
        .split(['|', '\n'])
        .filter(|row| !row.trim().is_empty())
        .map(|row| {
            let cells: Vec<&str> = row.split_whitespace().collect();
            let [name, operands, of, clause] = cells[..] else {
                panic!("{row}")
            };
            (name, types(operands), category(of), clause)
        })
        .collect()
}

/// Every operator of Annex A is in `ALL` once, and reads back from its
/// name; `Other` isn't, and has none.
#[test]
fn every_operator_round_trips() {
    assert_eq!(Operator::ALL.len(), 73);
    let mut names = Vec::new();
    for (i, &op) in Operator::ALL.iter().enumerate() {
        assert_ne!(op, Operator::Other);
        assert_eq!(Operator::ALL.iter().position(|&o| o == op), Some(i));
        let name = op.name().unwrap();
        assert_eq!(Operator::of(name.as_bytes()), op, "{name}");
        names.push(name);
        // Its place in a set is its place in `ALL`.
        let one = Operators::NONE.with(op);
        assert_eq!(one.iter().collect::<Vec<_>>(), [op]);
        assert_eq!(op as usize, i);
    }
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 73);
    assert_eq!(Operator::Other as usize, 73);
    assert_eq!(Operator::Other.name(), None);
}

/// Each operator's operands, category and clause are as `OPERATORS`
/// reads its clause.
#[test]
fn every_operators_operands_and_category() {
    let rows = operator_rows();
    assert_eq!(rows.len(), 73);
    for (name, operands, category, clause) in &rows {
        let op = Operator::of(name.as_bytes());
        assert_ne!(op, Operator::Other, "{name}");
        assert_eq!(op.operands(), Some(*operands), "{name}");
        assert_eq!(op.category(), Some(*category), "{name}");
        assert_eq!(op.clause(), Some(*clause), "{name}");
        assert_eq!(category.clause(), *clause, "{name}");
    }
    for op in Operator::ALL {
        let name = op.name().unwrap();
        assert_eq!(rows.iter().filter(|r| r.0 == name).count(), 1, "{name}");
    }
    // The counts: none, one, two, three, four and six operands.
    let count = |n: usize| {
        let fixed =
            |op: &&Operator| matches!(op.operands(), Some(Operands::Fixed(t)) if t.len() == n);
        Operator::ALL.iter().filter(fixed).count()
    };
    let counts: Vec<usize> = [0, 1, 2, 3, 4, 5, 6].map(count).to_vec();
    assert_eq!(counts, [24, 24, 9, 3, 5, 0, 4]);
}

/// The names a path's builder and painter tell apart, as they were told:
/// each painting operator's paint, `F` and `f` both a fill; `W` and `W*`
/// clipping by their rules; and those that build the path.
#[test]
fn names_tell_paint_clip_and_path() {
    use Operator as O;
    let named: [(&[u8], Operator); 51] = [
        (b"m", O::Move),
        (b"l", O::Line),
        (b"c", O::Curve),
        (b"v", O::CurveFromPen),
        (b"y", O::CurveToEnd),
        (b"h", O::ClosePath),
        (b"re", O::Rect),
        (b"W", O::Clip),
        (b"W*", O::ClipEvenOdd),
        (b"f", O::Fill),
        (b"F", O::FillCompat),
        (b"f*", O::FillEvenOdd),
        (b"S", O::Stroke),
        (b"s", O::CloseStroke),
        (b"B", O::FillStroke),
        (b"B*", O::FillStrokeEvenOdd),
        (b"b", O::CloseFillStroke),
        (b"b*", O::CloseFillStrokeEvenOdd),
        (b"n", O::EndPath),
        (b"cm", O::Transform),
        (b"w", O::LineWidth),
        (b"J", O::LineCap),
        (b"j", O::LineJoin),
        (b"M", O::MiterLimit),
        (b"q", O::Save),
        (b"Q", O::Restore),
        (b"gs", O::ExtGState),
        (b"BT", O::BeginText),
        (b"ET", O::EndText),
        (b"Tm", O::TextMatrix),
        (b"Td", O::MoveLine),
        (b"TD", O::MoveLineLeading),
        (b"T*", O::NextLine),
        (b"Tc", O::CharSpace),
        (b"Tw", O::WordSpace),
        (b"Tz", O::Scale),
        (b"TL", O::Leading),
        (b"Ts", O::Rise),
        (b"Tr", O::RenderMode),
        (b"Tf", O::Font),
        (b"Tj", O::Show),
        (b"TJ", O::ShowArray),
        (b"'", O::NextLineShow),
        (b"\"", O::NextLineShowSpaced),
        (b"Do", O::Draw),
        (b"sh", O::Shade),
        (b"BMC", O::BeginMarked),
        (b"BDC", O::BeginMarkedProps),
        (b"EMC", O::EndMarked),
        (b"MP", O::MarkedPoint),
        (b"DP", O::MarkedPointProps),
    ];
    for (name, op) in named {
        assert_eq!(Operator::of(name), op, "{}", String::from_utf8_lossy(name));
    }
    let painted: [(&[u8], Paint); 10] = [
        (b"f", Paint::Fill),
        (b"F", Paint::Fill),
        (b"f*", Paint::FillEvenOdd),
        (b"S", Paint::Stroke),
        (b"s", Paint::CloseStroke),
        (b"B", Paint::FillStroke),
        (b"B*", Paint::FillStrokeEvenOdd),
        (b"b", Paint::CloseFillStroke),
        (b"b*", Paint::CloseFillStrokeEvenOdd),
        (b"n", Paint::None),
    ];
    let painted = painted.map(|(name, paint)| (Operator::of(name), paint));
    let clips = [
        (Operator::Clip, FillRule::NonZero),
        (Operator::ClipEvenOdd, FillRule::EvenOdd),
    ];
    let built = Operators::of(&[b"m", b"l", b"c", b"v", b"y", b"h", b"re", b"W", b"W*"]);
    for &op in Operator::ALL.iter().chain([&Operator::Other]) {
        let paint = painted.iter().find(|(o, _)| *o == op).map(|p| p.1);
        assert_eq!(op.paint(), paint, "{op:?}");
        let clip = clips.iter().find(|(o, _)| *o == op).map(|c| c.1);
        assert_eq!(op.clip(), clip, "{op:?}");
        assert_eq!(op.builds_path(), built.contains(op), "{op:?}");
    }
    // What each paint does.
    let fills = |p: Paint| p.fills();
    assert_eq!(fills(Paint::Fill), Some(FillRule::NonZero));
    assert_eq!(
        fills(Paint::CloseFillStrokeEvenOdd),
        Some(FillRule::EvenOdd)
    );
    assert_eq!(fills(Paint::CloseStroke), None);
    assert_eq!(fills(Paint::None), None);
    let strokes: Vec<&str> = painted
        .iter()
        .filter(|(_, p)| p.strokes())
        .map(|(op, _)| op.name().unwrap())
        .collect();
    assert_eq!(strokes, ["S", "s", "B", "B*", "b", "b*"]);
}

/// A word that's no operator of Annex A is `Other`, as written: case
/// counts, and nothing is trimmed.
#[test]
fn a_word_thats_no_operator_is_other() {
    for word in [
        &b"Sh"[..],
        b"BM",
        b"BD",
        b"endstream",
        b"",
        b"qq",
        b"F*",
        b"w ",
        b" w",
        b"d2",
        b"TJ*",
        b"bi",
        b"Bi",
        b"\"\"",
        b"Tf\0",
        b"true",
        b"null",
    ] {
        assert_eq!(Operator::of(word), Operator::Other, "{word:?}");
    }
    let other = Operator::Other;
    assert_eq!(
        (
            other.name(),
            other.operands(),
            other.category(),
            other.clause()
        ),
        (None, None, None, None)
    );
    assert_eq!(
        (other.paint(), other.clip(), other.builds_path()),
        (None, None, false)
    );
    // Through the operations: each operation's operator is its name's.
    let s = b"1 2 Sh 3 BM endstream q";
    let read = operations_of(s, &ResCx::none(), &Limits::DEFAULT);
    let typed: Vec<_> = read
        .iter()
        .map(|op| op.as_ref().unwrap().operator())
        .collect();
    use Operator::{Other, Save};
    assert_eq!(typed, [Other, Other, Other, Save]);
    for op in operations_of(PAGE, &ResCx::none(), &Limits::DEFAULT) {
        let op = op.unwrap();
        assert_eq!(op.operator(), Operator::of(op.name()));
    }
}

/// Each state's operators, and the state each leaves, as 8.2's Figure 9
/// draws them, with the errata (8.4.2's and 9.4.1's: `q` and `Q` in a
/// text object; 14.6.1's: marked content in one; Table 50's: the shading
/// operator is `sh`); `d0`
/// and `d1` first in a Type 3 glyph description alone; `Other` nowhere.
#[test]
fn the_states_of_graphics_objects() {
    use State::{ClippingPath, ContentStream, GlyphStart, InlineImage, Path, Text};
    let general = "w J j M d ri i gs q Q";
    let color = "CS cs SC SCN sc scn G g RG rg K k";
    let text_state = "Tc Tw Tz TL Tf Tr Ts";
    let marked = "MP DP BMC BDC EMC";
    let painting = "S s f F f* B B* b b* n";
    let level = format!("{general} cm {color} {text_state} {marked} BX EX sh Do");
    let in_text = format!("{general} {color} {text_state} {marked} BX EX Tj TJ ' \" Td TD Tm T*");
    let states: [(State, Vec<(String, State)>); 6] = [
        (
            ContentStream,
            vec![
                (level, ContentStream),
                ("m re".into(), Path),
                ("BT".into(), Text),
                ("BI".into(), InlineImage),
            ],
        ),
        (
            Path,
            vec![
                ("m l c v y h re".into(), Path),
                ("W W*".into(), ClippingPath),
                (painting.into(), ContentStream),
            ],
        ),
        (ClippingPath, vec![(painting.into(), ContentStream)]),
        (Text, vec![(in_text, Text), ("ET".into(), ContentStream)]),
        (
            InlineImage,
            vec![("ID".into(), InlineImage), ("EI".into(), ContentStream)],
        ),
        (GlyphStart, vec![("d0 d1".into(), ContentStream)]),
    ];
    for (state, moves) in &states {
        let mut allowed = Operators::NONE;
        for &op in Operator::ALL.iter().chain([&Operator::Other]) {
            let name = op.name().unwrap_or("Other");
            let to = moves
                .iter()
                .find(|(names, _)| names.split(' ').any(|n| n == name))
                .map(|m| m.1);
            assert_eq!(state.after(op), to, "{name} in {state:?}");
            assert_eq!(state.allows(op), to.is_some(), "{name} in {state:?}");
            if to.is_some() {
                allowed = allowed.with(op);
            }
        }
        assert_eq!(state.allowed(), allowed, "{state:?}");
    }
    // The errata, each on its own.
    for op in Operators::of(&[b"q", b"Q", b"BMC", b"BDC", b"EMC", b"MP", b"DP"]).iter() {
        assert_eq!(Text.after(op), Some(Text), "{op:?}: 9.4.1, 14.6.1");
    }
    assert_eq!(
        Operator::of(b"sh").category(),
        Some(Category::Shading),
        "Table 50"
    );
    assert_eq!(ContentStream.after(Operator::Shade), Some(ContentStream));
    assert_eq!(Operator::of(b"Sh"), Operator::Other);
    // Nowhere else: `cm`, a path, a shading, an XObject or an inline image
    // in a text object; `d0` and `d1` past a glyph's start; `Other`.
    for name in ["cm", "m", "re", "sh", "Do", "BI", "BT"] {
        assert!(!Text.allows(Operator::of(name.as_bytes())), "{name}");
    }
    for state in [ContentStream, Path, ClippingPath, Text, InlineImage] {
        assert!(!state.allows(Operator::GlyphWidth));
        assert!(!state.allows(Operator::GlyphWidthBox));
    }
    for state in [
        ContentStream,
        Path,
        ClippingPath,
        Text,
        InlineImage,
        GlyphStart,
    ] {
        assert!(!state.allows(Operator::Other), "{state:?}");
    }
    assert_eq!(State::default(), ContentStream);
}

/// `State::step` takes an inline image, one operation, as `BI`, `ID` and
/// `EI` in turn: back where it began at the content stream level, and
/// not allowed in a text object; a stray `ID` or `EI` is its operator
/// alone.
#[test]
fn an_inline_image_steps_back_to_where_it_began() {
    let res = ResCx::none();
    let lim = &Limits::DEFAULT;
    let walk = |s: &[u8], start: State| -> Vec<Option<State>> {
        let mut state = Some(start);
        let mut states = Vec::new();
        for op in operations(s, &res, lim) {
            state = state.and_then(|state| state.step(&op.unwrap()));
            states.push(state);
        }
        states
    };
    let image = b"BI /W 1 /H 1 /BPC 8 /CS /G ID x EI";
    let read = operations_of(image, &res, lim);
    let op = read[0].as_ref().unwrap();
    assert_eq!(op.operator(), Operator::BeginInlineImage);
    assert_eq!(
        State::ContentStream.after(op.operator()),
        Some(State::InlineImage)
    );
    assert_eq!(State::ContentStream.step(op), Some(State::ContentStream));
    for state in [
        State::Path,
        State::ClippingPath,
        State::Text,
        State::InlineImage,
        State::GlyphStart,
    ] {
        assert_eq!(state.step(op), None, "{state:?}");
    }
    use State::{ContentStream, Path, Text};
    let page = b"q BI /W 1 /H 1 /BPC 8 /CS /G ID x EI 0 0 m 1 1 l S Q";
    let level = Some(ContentStream);
    assert_eq!(
        walk(page, ContentStream),
        [level, level, Some(Path), Some(Path), level, level]
    );
    let in_text = b"BT BI /W 1 /H 1 /BPC 8 /CS /G ID x EI ET";
    assert_eq!(walk(in_text, ContentStream), [Some(Text), None, None]);
    // A stray `ID` or `EI`: its operator alone.
    let stray = operations_of(b"ID EI", &res, lim);
    let (id, ei) = (stray[0].as_ref().unwrap(), stray[1].as_ref().unwrap());
    assert_eq!(
        (id.operator(), ei.operator()),
        (Operator::InlineImageData, Operator::EndInlineImage)
    );
    assert!(id.inline_image().is_none() && ei.inline_image().is_none());
    assert_eq!(State::ContentStream.step(id), None);
    assert_eq!(State::InlineImage.step(id), Some(State::InlineImage));
    assert_eq!(State::InlineImage.step(ei), Some(State::ContentStream));
    // `d0` first in a glyph description, and then the content stream level.
    let glyph = b"750 0 d0 0 0 750 750 re f";
    assert_eq!(
        walk(glyph, State::GlyphStart),
        [Some(ContentStream), Some(Path), Some(ContentStream)]
    );
    assert_eq!(walk(glyph, ContentStream)[0], None);
}

/// A `BI` that begins no inline image, a keyword where its dictionary's
/// key should be, is allowed where an image may begin, and content stays
/// where it was, as nothing is drawn: at the content stream level, and
/// nowhere else; the content after it steps as it reads.
#[test]
fn a_lone_bi_steps_as_nothing_where_an_image_may_begin() {
    let res = ResCx::none();
    let lim = &Limits::DEFAULT;
    let read = operations_of(b"BI foo", &res, lim);
    let op = read[0].as_ref().unwrap();
    assert_eq!(
        (op.operator(), op.inline_image().is_none()),
        (Operator::BeginInlineImage, true)
    );
    assert_eq!(State::ContentStream.step(op), Some(State::ContentStream));
    for state in [
        State::Path,
        State::ClippingPath,
        State::Text,
        State::InlineImage,
        State::GlyphStart,
    ] {
        assert_eq!(state.step(op), None, "{state:?}");
    }
    use State::{ContentStream, Path, Text};
    let walk = |s: &[u8]| -> Vec<Option<State>> {
        let mut state = Some(ContentStream);
        let read = operations(s, &res, lim);
        (read.map(|op| {
            state = state.and_then(|state| state.step(&op.unwrap()));
            state
        }))
        .collect()
    };
    let level = Some(ContentStream);
    // `BI`, `re` with `/W 1` its operands, `f`, `Q`.
    assert_eq!(
        walk(b"q BI /W 1 re f Q"),
        [level, level, Some(Path), level, level]
    );
    assert_eq!(walk(b"BT BI foo ET"), [Some(Text), None, None, None]);
}

/// Operators as sets: each category's const its operators, the sixteen
/// apart and together all of Annex A's; and each operation on them.
#[test]
fn sets_of_operators() {
    let consts = [
        (
            Operators::GENERAL_GRAPHICS_STATE,
            Category::GeneralGraphicsState,
            10,
        ),
        (
            Operators::SPECIAL_GRAPHICS_STATE,
            Category::SpecialGraphicsState,
            1,
        ),
        (Operators::PATH_CONSTRUCTION, Category::PathConstruction, 7),
        (Operators::PATH_PAINTING, Category::PathPainting, 10),
        (Operators::CLIPPING_PATH, Category::ClippingPath, 2),
        (Operators::TEXT_OBJECT, Category::TextObject, 2),
        (Operators::TEXT_STATE, Category::TextState, 7),
        (Operators::TEXT_POSITIONING, Category::TextPositioning, 4),
        (Operators::TEXT_SHOWING, Category::TextShowing, 4),
        (Operators::TYPE3_FONT, Category::Type3Font, 2),
        (Operators::COLOR, Category::Color, 12),
        (Operators::SHADING, Category::Shading, 1),
        (Operators::INLINE_IMAGE, Category::InlineImage, 3),
        (Operators::XOBJECT, Category::XObject, 1),
        (Operators::MARKED_CONTENT, Category::MarkedContent, 5),
        (Operators::COMPATIBILITY, Category::Compatibility, 2),
    ];
    let mut every = Operators::NONE;
    for (set, category, len) in consts {
        assert_eq!(set, category.operators(), "{category:?}");
        assert_eq!(set.len(), len, "{category:?}");
        let theirs: Operators = Operator::ALL
            .iter()
            .copied()
            .filter(|op| op.category() == Some(category))
            .collect();
        assert_eq!(set, theirs, "{category:?}");
        assert!(every.intersection(set).is_empty(), "{category:?} apart");
        every = every.union(set);
    }
    assert_eq!(every, Operators::ALL.without(Operator::Other));
    assert_eq!(every.len(), 73);

    let (q, cm) = (Operator::Save, Operator::Transform);
    assert!(Operators::NONE.is_empty());
    assert_eq!((Operators::NONE.len(), Operators::of(&[]).len()), (0, 0));
    assert_eq!(Operators::default(), Operators::NONE);
    assert_eq!(Operators::ALL.len(), 74);
    assert!(Operators::ALL.contains(Operator::Other));
    let set = Operators::NONE.with(q).with(cm).with(q);
    assert_eq!(set.len(), 2);
    assert!(set.contains(q) && set.contains(cm) && !set.contains(Operator::Restore));
    assert_eq!(set.without(q), Operators::NONE.with(cm));
    assert_eq!(set.without(Operator::Restore), set);
    assert_eq!(set, Operators::of(&[b"cm", b"q"]));
    assert_eq!(
        Operators::of(&[b"q", b"Sh"]),
        Operators::NONE.with(q).with(Operator::Other)
    );
    let general = Operators::GENERAL_GRAPHICS_STATE;
    assert_eq!(set.intersection(general), Operators::NONE.with(q));
    assert_eq!(set.difference(general), Operators::NONE.with(cm));
    assert_eq!(set.union(general).len(), 11);
    assert_eq!(general.difference(set).len(), 9);
    assert!(!general.contains(cm));
    // In `ALL`'s order, `Other` last; from an iterator; shown by name.
    let all: Vec<_> = Operators::ALL.iter().collect();
    assert_eq!(all[..73], *Operator::ALL);
    assert_eq!(all[73], Operator::Other);
    assert_eq!(Operators::ALL.iter().collect::<Operators>(), Operators::ALL);
    assert_eq!(format!("{set:?}"), "{q, cm}");
    assert_eq!(format!("{:?}", Operators::NONE), "{}");
    assert_eq!(
        format!("{:?}", Operators::TEXT_SHOWING.with(Operator::Other)),
        "{Tj, TJ, ', \", Other}"
    );
}

const LIM: &Limits = &Limits::DEFAULT;

/// `streams`' marked ranges, read as a page's with no color spaces.
fn found<'p>(streams: &[&[u8]], props: &'p Properties) -> Result<Vec<Marked<'p>>, Error> {
    marked(&Concat::new(streams), props, &ResCx::none(), LIM)
}

fn span(start: (u32, u32), end: (u32, u32)) -> SpanAcross {
    let pos = |(stream, at)| Pos { stream, at };
    SpanAcross {
        start: pos(start),
        end: pos(end),
    }
}

/// The span of `part` in `s`, one stream.
fn span_of(s: &[u8], part: &[u8]) -> SpanAcross {
    let at = s.windows(part.len()).position(|w| w == part).unwrap() as u32;
    span((0, at), (0, at + part.len() as u32))
}

#[test]
fn marked_ranges_have_their_lists() {
    let ocg = dict! { "Type" => "OCG" };
    let props = Properties::new([
        (b"P0".to_vec(), dict! { "Subtype" => "Header" }),
        (b"oc1".to_vec(), ocg.clone()),
    ]);
    let first: &[u8] =
        b"/Artifact <</Type /Pagination /S#75btype /Footer /A [1 (b\\)) <0a>]>> BDC q";
    let second: &[u8] =
        b"/OC /oc1 BDC /Span BMC EMC EMC Q EMC /Artifact /P0 BDC EMC /X /None BDC EMC";
    let m = found(&[first, second], &props).unwrap();
    let tags: Vec<&[u8]> = m.iter().map(|m| m.tag.as_slice()).collect();
    assert_eq!(tags, [&b"Artifact"[..], b"OC", b"Span", b"Artifact", b"X"]);
    assert_eq!(
        m.iter().map(|m| m.depth).collect::<Vec<_>>(),
        [0, 1, 2, 0, 0]
    );
    // The first runs from the first stream into the second.
    assert_eq!(m[0].span, span((0, 0), (1, 36)));
    let Some(Props::Inline(list)) = &m[0].props else {
        panic!("{:?}", m[0].props)
    };
    assert_eq!(list.get(b"Subtype").unwrap(), &Object::from("Footer"));
    assert_eq!(
        list.get(b"A").unwrap(),
        &Object::from(vec![
            1.into(),
            string(b"b)", Form::Literal),
            string(&[10], Form::Hex),
        ])
    );
    // Named lists, found or not.
    assert_eq!(m[1].props, Some(Props::Named(b"oc1".to_vec(), Some(&ocg))));
    assert_eq!(m[2].props, None);
    assert_eq!(
        m[3].props.as_ref().and_then(Props::dict),
        Some(&dict! { "Subtype" => "Header" })
    );
    assert_eq!(m[4].props, Some(Props::Named(b"None".to_vec(), None)));
}

#[test]
fn marked_content_must_balance() {
    let props = Properties::default();
    let kind = |s: &[u8]| found(&[s], &props).unwrap_err();
    assert_eq!(
        kind(b"q /A BMC Q EMC EMC"),
        Error {
            at: 15,
            kind: ErrorKind::Emc
        }
    );
    assert_eq!(
        kind(b"/A BMC /B BMC EMC"),
        Error {
            at: 0,
            kind: ErrorKind::Marked
        }
    );
    let deep = Limits { depth: 2, ..*LIM };
    let e = marked(
        &Concat::new(&[b"/A BMC /B BMC /C BMC EMC EMC EMC"]),
        &props,
        &ResCx::none(),
        &deep,
    )
    .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Depth);
    // Marked content inside a string or an inline image isn't.
    assert!(found(&[b"(EMC) Tj BI /W 3 /H 1 /CS /G /BPC 8 ID EMC EI"], &props).is_ok());
}

#[test]
fn balance_is_the_q_count() {
    let of = |s: &[u8]| balance(&Concat::new(&[s]), &ResCx::none(), LIM).unwrap();
    // A state left saved, one restored that wasn't saved, and both.
    let b = |net, min_prefix| Balance { net, min_prefix };
    assert_eq!(of(b"q 1 0 0 1 0 0 cm"), b(1, 0));
    assert_eq!(of(b"Q q"), b(0, -1));
    assert_eq!(of(b"Q"), b(-1, -1));
    assert_eq!(of(b"q q Q Q Q q"), b(0, -1));
    // Not in strings, names or an image's data.
    assert_eq!(
        of(b"(Q) Tj /Q gs BI /W 1 /H 1 /CS /G /BPC 8 ID Q EI"),
        b(0, 0)
    );
    // Across streams.
    let c = Concat::new(&[b"q", b"Q Q"]);
    assert_eq!(balance(&c, &ResCx::none(), LIM).unwrap(), b(-1, -1));
}

#[test]
fn names_used_are_by_category() {
    let s: &[u8] = b"/GS0 gs /F1 9 Tf /CS0 cs /DeviceRGB CS /P0 scn 0.5 /P1 SCN /Im1 Do \
        /Sh0 sh /OC /oc0 BDC EMC /X /Named DP /Y <<>> BDC EMC \
        BI /W 1 /H 1 /CS /Idx /BPC 8 ID x EI BI /W 1 /H 1 /CS /RGB /BPC 8 ID xyz EI \
        /Artifact BMC /F9 1 Tf EMC";
    let c = Concat::new(&[s]);
    let except = [span_of(s, b"/Artifact BMC /F9 1 Tf EMC")];
    let names = names_used(&c, &ResCx::none(), LIM, &except).unwrap();
    let of = |kind| names.of(kind).collect::<Vec<_>>();
    assert_eq!(of(ResKind::ExtGState), [b"GS0"]);
    assert_eq!(of(ResKind::Font), [b"F1"]);
    assert_eq!(of(ResKind::ColorSpace), [&b"CS0"[..], b"Idx"]);
    assert_eq!(of(ResKind::Pattern), [b"P0", b"P1"]);
    assert_eq!(of(ResKind::XObject), [b"Im1"]);
    assert_eq!(of(ResKind::Shading), [b"Sh0"]);
    assert_eq!(of(ResKind::Properties), [&b"Named"[..], b"oc0"]);
    assert_eq!(names.len(), 10);
    assert!(!names.contains(ResKind::Font, b"F9"));
}

/// Marked content stepped through an operation at a time: each range
/// told as it opens and as it ends, by its index, one open spanning where
/// it starts alone, one ended through its `EMC`; what's found is what
/// [`marked`] finds, and the errors are its own, where they are.
#[test]
fn marked_content_stepped_by_hand() {
    use MarkedEvent::{Ended, Opened};
    let header = dict! { "Subtype" => "Header" };
    let props = Properties::new([(b"P0".to_vec(), header.clone())]);
    let s: &[u8] = b"/A BMC q /B /P0 BDC 0 0 m EMC Q /C <</K 1>> BDC EMC EMC";
    let page = Concat::new(&[s]);
    let none = ResCx::none();
    let mut nest = MarkedNest::new(&props, LIM);
    let (mut names, mut events, mut open) = (Vec::new(), Vec::new(), Vec::new());
    for op in page.operations(&none, LIM) {
        let op = op.unwrap();
        let event = nest.step(&page, &op).unwrap();
        open.push(nest.open().collect::<Vec<_>>());
        match event {
            Some(Opened(i)) => {
                let open = nest.get(i).unwrap();
                let start = page.pos(op.range().start);
                assert_eq!((open.span.start, open.span.end), (start, start));
            }
            Some(Ended(i)) => {
                let ended = nest.get(i).unwrap();
                assert_eq!(ended.span.end, page.pos(op.at().end));
            }
            None => {}
        }
        names.push(op.name());
        events.push(event);
    }
    assert_eq!(
        names,
        [
            &b"BMC"[..],
            b"q",
            b"BDC",
            b"m",
            b"EMC",
            b"Q",
            b"BDC",
            b"EMC",
            b"EMC"
        ]
    );
    assert_eq!(
        events,
        [
            Some(Opened(0)),
            None,
            Some(Opened(1)),
            None,
            Some(Ended(1)),
            None,
            Some(Opened(2)),
            Some(Ended(2)),
            Some(Ended(0)),
        ]
    );
    // The ranges open after each, the outermost first.
    let (outer, inner): (&[usize], &[usize]) = (&[0], &[0, 1]);
    assert_eq!(
        open,
        [
            outer,
            outer,
            inner,
            inner,
            outer,
            outer,
            &[0, 2],
            outer,
            &[]
        ]
    );
    assert!(nest.get(3).is_none());
    let stepped = nest.done().unwrap();
    assert_eq!(stepped, marked(&page, &props, &none, LIM).unwrap());
    assert_eq!(stepped[0].span, span_of(s, s));
    assert_eq!(stepped[1].span, span_of(s, b"/B /P0 BDC 0 0 m EMC"));
    assert_eq!(
        stepped[1].props,
        Some(Props::Named(b"P0".to_vec(), Some(&header)))
    );
    assert_eq!(stepped[2].props, Some(Props::Inline(dict! { "K" => 1 })));
    assert_eq!((stepped[2].depth, &stepped[2].tag[..]), (1, &b"C"[..]));

    // An `EMC` with none open, where it is; one still open at the end,
    // where the first of those starts; one past the cap, where its
    // operator is.
    let stepped = |s: &[u8], lim: &Limits| {
        let page = Concat::new(&[s]);
        let mut nest = MarkedNest::new(&props, lim);
        for op in page.operations(&none, lim) {
            nest.step(&page, &op.unwrap())?;
        }
        nest.done()
    };
    let error = |at, kind| Err(Error { at, kind });
    assert_eq!(stepped(b"/A BMC EMC EMC", LIM), error(11, ErrorKind::Emc));
    assert_eq!(
        stepped(b"q /A BMC /B BMC EMC", LIM),
        error(2, ErrorKind::Marked)
    );
    assert_eq!(
        stepped(b"q /A BMC /B BMC", LIM),
        error(2, ErrorKind::Marked)
    );
    let deep = Limits { depth: 1, ..*LIM };
    assert_eq!(
        stepped(b"/A BMC /B BMC EMC EMC", &deep),
        error(10, ErrorKind::Depth)
    );
    for (s, lim) in [
        (&b"/A BMC EMC EMC"[..], LIM),
        (b"q /A BMC /B BMC EMC", LIM),
        (b"q /A BMC /B BMC", LIM),
        (b"/A BMC /B BMC EMC EMC", &deep),
    ] {
        assert_eq!(
            stepped(s, lim),
            marked(&Concat::new(&[s]), &props, &none, lim)
        );
    }
}

/// `q`/`Q` balance and resource names counted an operation at a time are
/// what [`balance`] and [`names_used`] give: each operation names one
/// resource at most, by its category, a name used twice counted once.
#[test]
fn balance_and_names_stepped_by_hand() {
    let s: &[u8] = b"Q q /GS0 gs /F1 9 Tf (x) Tj /CS0 cs /DeviceGray CS /P0 scn \
        BI /W 1 /H 1 /CS /CS1 /BPC 8 ID x EI q /Im#201 Do Q /F1 9 Tf /X /P1 DP";
    let page = Concat::new(&[s]);
    let none = ResCx::none();
    let mut counted = Balance::default();
    let mut names = NameSet::default();
    let mut each = Vec::new();
    for op in page.operations(&none, LIM) {
        let op = op.unwrap();
        counted.take(&op);
        names.take(&op);
        each.push(op.resource().map(|(kind, name)| (kind, name.into_owned())));
    }
    assert_eq!(
        counted,
        Balance {
            net: 0,
            min_prefix: -1
        }
    );
    assert_eq!(counted, balance(&page, &none, LIM).unwrap());
    assert_eq!(names, names_used(&page, &none, LIM, &[]).unwrap());
    let named = |kind, name: &[u8]| Some((kind, name.to_vec()));
    assert_eq!(
        each,
        [
            None,
            None,
            named(ResKind::ExtGState, b"GS0"),
            named(ResKind::Font, b"F1"),
            None,
            named(ResKind::ColorSpace, b"CS0"),
            None,
            named(ResKind::Pattern, b"P0"),
            named(ResKind::ColorSpace, b"CS1"),
            None,
            named(ResKind::XObject, b"Im 1"),
            None,
            named(ResKind::Font, b"F1"),
            named(ResKind::Properties, b"P1"),
        ]
    );
    assert_eq!(names.len(), 7);
    let every: Vec<_> = names.iter().collect();
    assert_eq!(every[0], (ResKind::ColorSpace, &b"CS0"[..]));
    names.remove(ResKind::Font, b"F1");
    names.remove(ResKind::Font, b"F2");
    assert_eq!(names.len(), 6);
    assert!(!names.contains(ResKind::Font, b"F1") && !names.is_empty());
}

/// What content keeps once ranges are taken out: what's left's balance
/// and names, and the names only what's taken out uses. An operation is
/// taken out only when all of it, its operands too, is inside a range; a
/// range may be a whole stream, and one that's nowhere takes nothing.
#[test]
fn what_content_keeps_without_ranges() {
    let s: &[u8] = b"q /F1 9 Tf /Artifact BMC Q /F2 9 Tf /F1 9 Tf /Im1 Do EMC /Im2 Do";
    let page = Concat::new(&[s, b"/GS0 gs Q /F4 9 Tf"]);
    let none = ResCx::none();
    let artifact = span_of(s, b"/Artifact BMC Q /F2 9 Tf /F1 9 Tf /Im1 Do EMC");
    let k = kept(&page, &none, LIM, &[artifact]).unwrap();
    assert_eq!(
        k.balance,
        Balance {
            net: 0,
            min_prefix: 0
        }
    );
    let list = |names: &NameSet| -> Vec<(ResKind, Vec<u8>)> {
        names.iter().map(|(kind, n)| (kind, n.to_vec())).collect()
    };
    let named = |kind, name: &[u8]| (kind, name.to_vec());
    assert_eq!(
        list(&k.names),
        [
            named(ResKind::ExtGState, b"GS0"),
            named(ResKind::Font, b"F1"),
            named(ResKind::Font, b"F4"),
            named(ResKind::XObject, b"Im2"),
        ]
    );
    // `F1` is used outside it too.
    assert_eq!(
        list(&k.dropped),
        [named(ResKind::Font, b"F2"), named(ResKind::XObject, b"Im1")]
    );
    assert_eq!(
        names_used(&page, &none, LIM, &[artifact]),
        Ok(k.names.clone())
    );
    // A range from the first `Tf`'s operand on leaves that `Tf` kept.
    let from_nine = span_of(s, b"9 Tf /Artifact BMC Q /F2 9 Tf /F1 9 Tf /Im1 Do EMC");
    assert_eq!(kept(&page, &none, LIM, &[from_nine]), Ok(k));
    // The second stream, whole.
    let second = page.span(page.range(1));
    let k = kept(&page, &none, LIM, &[second]).unwrap();
    assert_eq!(
        k.balance,
        Balance {
            net: 0,
            min_prefix: 0
        }
    );
    assert_eq!(
        list(&k.dropped),
        [
            named(ResKind::ExtGState, b"GS0"),
            named(ResKind::Font, b"F4")
        ]
    );
    // Nowhere: past the second stream's end, and in no stream.
    let nowhere = [span((1, 0), (1, 99)), span((2, 0), (2, 0))];
    let all = kept(&page, &none, LIM, &[]).unwrap();
    assert_eq!(
        all.balance,
        Balance {
            net: -1,
            min_prefix: -1
        }
    );
    assert_eq!(all.names.len(), 6);
    assert!(all.dropped.is_empty());
    assert_eq!(kept(&page, &none, LIM, &nowhere), Ok(all));

    // Ranges in no order, overlapping: an operation is taken out when one
    // of them holds all of it, whichever.
    let s: &[u8] = b"/A BMC /F1 9 Tf /F2 9 Tf EMC /F3 9 Tf /F4 9 Tf /F5 9 Tf";
    let page = Concat::new(&[s]);
    let ranges = [
        span_of(s, b"/F4 9 Tf /F5"),
        span_of(s, b"/F2 9 Tf EMC /F3 9 Tf"),
        span_of(s, b"/A BMC /F1 9 Tf /F2 9 Tf EMC"),
        span_of(s, b"9 Tf EMC /F3"),
    ];
    let k = kept(&page, &none, LIM, &ranges).unwrap();
    let fonts = |names: &NameSet| {
        names
            .of(ResKind::Font)
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>()
    };
    assert_eq!(fonts(&k.names), [b"F5"]);
    assert_eq!(fonts(&k.dropped), [b"F1", b"F2", b"F3", b"F4"]);
    let mut reversed = ranges;
    reversed.reverse();
    assert_eq!(kept(&page, &none, LIM, &reversed), Ok(k));
}

/// A content of many ranges, each taken out, keeps what's outside them
/// in time that grows with the ranges as their count does, not as its
/// square: 100,000 ranges, as many as a page's 1.5 MB can make.
#[test]
fn many_ranges_taken_out_read_in_one_pass() {
    let s = b"/A BMC q Q EMC ".repeat(100_000);
    let page = Concat::new(&[&s]);
    let none = ResCx::none();
    let props = Properties::default();
    let found = marked(&page, &props, &none, LIM).unwrap();
    let ranges: Vec<SpanAcross> = found.iter().map(|m| m.span).rev().collect();
    let started = std::time::Instant::now();
    let k = kept(&page, &none, LIM, &ranges).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(k, Kept::default());
}

/// Ranges naming one property list share it, whatever its size: each
/// range's is the one the lists given hold, not a copy.
#[test]
fn ranges_share_their_named_list() {
    let big: pdf_objects::Dict = (0..1_000).map(|i| (format!("K{i}"), i)).collect();
    let props = Properties::new([(b"P".to_vec(), big)]);
    let s = b"/T /P BDC EMC ".repeat(20_000);
    let started = std::time::Instant::now();
    let found = found(&[&s], &props).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(found.len(), 20_000);
    let list = props.get(b"P").unwrap();
    for m in &found {
        let read = m.props.as_ref().and_then(Props::dict).unwrap();
        assert!(std::ptr::eq(read, list), "the list given");
    }
}

/// Each pass gives the error that stops the operations, where it is.
#[test]
fn the_passes_give_the_operations_error() {
    let page = Concat::new(&[b"q /F1 9 Tf", b"/A BMC (unended"]);
    let none = ResCx::none();
    let e = Error {
        at: 18,
        kind: ErrorKind::String,
    };
    let props = Properties::default();
    assert_eq!(balance(&page, &none, LIM), Err(e));
    assert_eq!(marked(&page, &props, &none, LIM), Err(e));
    assert_eq!(names_used(&page, &none, LIM, &[]), Err(e));
    assert_eq!(kept(&page, &none, LIM, &[]), Err(e));
}
