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
use crate::fuzz::{Kept, Unwanted, blank, faulted, fixtures};
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
    assert_eq!(at(b"Q BI /F /AHx ID 0E"), (2, InlineImage));
    assert_eq!(at(b"BI /F /A85 ID 9jqo~> 1"), (0, InlineImage));
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
    both::<Balance>();
    both::<Properties>();
    both::<Marked>();
    both::<MarkedNest>();
    both::<NameSet>();
    both::<crate::Kept>();
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

/// The faults reading `page`'s operations tells, with no resources, in
/// order: each where it is, and the fault.
fn faults<'a>(page: &'a Concat) -> Vec<(Range<usize>, Fault<'a>)> {
    let mut kept = Kept::default();
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
            past(15),
            past(16)
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
        words(&found)[8],
        format!("{} is past the numbers read here", "1".repeat(40))
    );
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
        let mut kept = Kept::default();
        let object = o.object_with(8, &mut kept);
        assert_eq!(object, o.object(8), "the same object");
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
        let mut kept = Kept::default();
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
    let mut kept = Kept::default();
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
    let mut kept = Kept::default();
    let none = ResCx::none();
    let mut read = operations(b"q 1", &none, &Limits::DEFAULT).with_sink(&mut kept);
    assert!(read.next().is_some());
    assert!(read.next().is_none() && read.next().is_none());
    assert_eq!(kept.0, [(2..3, Fault::Trailing { count: 1 })]);
}

/// `content`'s faults, read whole with no error, and each's words.
fn told(content: &[u8]) -> Vec<(Range<usize>, String)> {
    let mut kept = Kept::default();
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
    // Its dictionary: a key that's no name, its tokens' own faults, a
    // word that's no object, and a key with no value.
    let s = b"BI 7 /W 1 /H 1 /BPC 8 /CS /G /A#zz foo /B ) /D <4G> /F#2 ID x EI";
    let stray = "has a # without two hex digits after it, which readers read differently";
    assert_eq!(
        told(s),
        [
            (
                place(s, b"7"),
                "7 is a dictionary's key and not a name".to_string()
            ),
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
        b"q --5 <4G> /A#2 { ) <</K 1 2 /M>> BI 7 /W 1 /H 1 /BPC 8 /CS /G /L 3 ID x 1 EI 1";
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
            "Key",
            "ImageLength",
            "BeforeEi",
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
            operand.object_with(8, &mut unwanted);
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
    let mut kept = Kept::default();
    let sink: &mut dyn Sink = &mut kept;
    let read: Vec<_> = operations(s, &none, &Limits::DEFAULT)
        .with_sink(&mut *sink)
        .collect::<Result<_, _>>()
        .unwrap();
    read[1].arg(0).unwrap().object_with(8, sink);
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
        let mut kept = Kept::default();
        let read: Vec<_> = page
            .operations(&res, lim)
            .with_sink(&mut kept)
            .collect::<Result<_, _>>()
            .unwrap();
        for op in &read {
            for operand in op.operands() {
                operand.object_with(lim.depth, &mut kept);
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
    assert_eq!(k, crate::Kept::default());
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
