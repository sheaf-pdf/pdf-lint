//! The writer: values as PDF syntax (ISO 32000-2, 7.3), as a direct
//! object, or as an indirect object's `n g obj` to `endobj`, which the
//! parser ([`parse`](crate::parse)) reads back as the same value, under
//! limits that take in its count of values. The writer keeps to
//! [`Limits::DEFAULT`]'s nesting, but not to its 2^25 values, which a
//! value in memory may pass.
//!
//! - **Reals** in their shortest form that reads back as the same f64,
//!   past i64's range too, never with an exponent; a magnitude past Annex
//!   C's is clamped to it, and one too small for it written as 0.
//! - **Names and strings** byte by byte, a string in its [`Form`], or
//!   literal when it has none.
//! - **Dictionaries** with every entry, in order, a key written twice
//!   included, though ISO 32000-2 has a dictionary hold one entry for a
//!   key (7.3.7): [`Dict::remove_duplicates`] leaves each key's last.
//! - **Streams** with their data as it is, and its length as their
//!   `/Length`, in place of each `/Length` entry or after the others. A
//!   stream reads back only as an indirect object, as it is one (7.3.8):
//!   one written alone by [`write()`], or inside another value, is
//!   written all the same, though no reader reads it back as a stream.
//!
//! Numbers, names and strings are written byte by byte rather than through
//! `write!`, which a writer of thousands of objects pays for at every one;
//! a real alone goes through it, for its shortest form.
//!
//! What it can't write as the value is refused, and then nothing is
//! written ([`Error`]): a real that isn't finite, values nested deeper than
//! a parser reads them under [`Limits::DEFAULT`], a string whose bytes
//! aren't known or that's still ciphertext, and a stream whose data isn't
//! in memory, which [`indirect_stream`] writes from data given apart.

use std::fmt;
use std::io::Write as _;

use crate::object::{Dict, Entry, Form, Object, ObjectId, Str, Value};
use crate::parse::Limits;

/// The largest magnitude a real is written at: Annex C's, for readers
/// that hold reals in single precision.
const MAX_REAL: f64 = 3.4e38;

/// The smallest nonzero magnitude: a real nearer 0 is written as 0.
const MIN_REAL: f64 = 1.175e-38;

/// The bytes a name writes as they are: `!`–`~` but `#` and the
/// delimiters.
const REGULAR: [bool; 256] = {
    let mut regular = [false; 256];
    let mut b = b'!';
    while b <= b'~' {
        regular[b as usize] = true;
        b += 1;
    }
    let special = b"#()<>[]{}/%";
    let mut i = 0;
    while i < special.len() {
        regular[special[i] as usize] = false;
        i += 1;
    }
    regular
};

/// Why the writer wrote nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A real that's NaN or an infinity, which no reader can read back.
    NonFinite {
        /// The indirect object it's in, when that's known.
        id: Option<ObjectId>,
    },
    /// Arrays and dictionaries nested deeper than a parser reads them under
    /// [`Limits::DEFAULT`], so what's written reads back.
    Nested {
        /// The indirect object they're in, when that's known.
        id: Option<ObjectId>,
        /// How deeply they may nest.
        limit: usize,
    },
    /// A string read as text, whose bytes aren't known ([`Str::Text`]).
    Text {
        /// The indirect object it's in, when that's known.
        id: Option<ObjectId>,
    },
    /// A string that's still ciphertext ([`Str::Encrypted`]), which is
    /// right to write only under the key, and the number and generation,
    /// it was encrypted with.
    Encrypted {
        /// The indirect object it's in, when that's known.
        id: Option<ObjectId>,
    },
    /// A stream whose data isn't in memory, as
    /// [`Data::Bytes`](crate::Data::Bytes): [`indirect_stream`] writes one
    /// from data given apart.
    Data {
        /// The indirect object it's in, when that's known.
        id: Option<ObjectId>,
    },
}

impl Error {
    /// The error, said of object `id`.
    fn within(self, id: ObjectId) -> Error {
        match self {
            Error::NonFinite { id: None } => Error::NonFinite { id: Some(id) },
            Error::Nested { id: None, limit } => Error::Nested {
                id: Some(id),
                limit,
            },
            Error::Text { id: None } => Error::Text { id: Some(id) },
            Error::Encrypted { id: None } => Error::Encrypted { id: Some(id) },
            Error::Data { id: None } => Error::Data { id: Some(id) },
            e => e,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NonFinite { id } => {
                write!(f, "a real that isn't a finite number{}", in_object(id))
            }
            Error::Nested { id, limit } => write!(
                f,
                "arrays and dictionaries nested deeper than {limit}{}",
                in_object(id)
            ),
            Error::Text { id } => write!(
                f,
                "a string read as text, whose bytes aren't known{}",
                in_object(id)
            ),
            Error::Encrypted { id } => {
                write!(f, "a string that's still ciphertext{}", in_object(id))
            }
            Error::Data { id } => {
                write!(f, "a stream whose data isn't in memory{}", in_object(id))
            }
        }
    }
}

impl std::error::Error for Error {}

/// " in object n g", when `id` is known, for an error's text.
fn in_object(id: &Option<ObjectId>) -> String {
    id.map(|(n, g)| format!(" in object {n} {g}"))
        .unwrap_or_default()
}

/// `obj` as a direct object. What can't be written as the value is
/// refused ([`Error`]), and then nothing is written. A stream is written
/// as one, though a parser reads it back only as an indirect object
/// ([`indirect`]).
pub fn write(out: &mut Vec<u8>, obj: &Object) -> Result<(), Error> {
    let start = out.len();
    value(out, obj, 0).inspect_err(|_| out.truncate(start))
}

/// `obj` as indirect object `id`, `n g obj` to `endobj`, and where in
/// `out` it starts. Refused as [`write()`] refuses, the error naming `id`.
pub fn indirect(out: &mut Vec<u8>, id: ObjectId, obj: &Object) -> Result<usize, Error> {
    let start = out.len();
    reference(out, id);
    out.extend_from_slice(b" obj\n");
    match value(out, obj, 0) {
        Ok(()) => {
            out.extend_from_slice(b"\nendobj\n");
            Ok(start)
        }
        Err(e) => {
            out.truncate(start);
            Err(e.within(id))
        }
    }
}

/// A stream of `dict` and `data`, as indirect object `id`, `n g obj` to
/// `endobj`, and where in `out` it starts: for data that isn't in memory
/// as [`Data::Bytes`](crate::Data::Bytes), given apart, without copying
/// it there. Its `/Length` is `data`'s length. Refused as [`write()`]
/// refuses, the error naming `id`.
///
/// `data` is written as it is, so it's the stream's data only when it's
/// encoded by the filters `dict` names and, in an encrypted file,
/// encrypted as object `id`'s. A reader's [`Data::At`](crate::Data::At)
/// names data as it was read: encoded by the filters the stream had then,
/// and in an encrypted file ciphertext, right to write only under the key,
/// and the number and generation, it was encrypted with, as a string's is
/// ([`Error::Encrypted`]).
pub fn indirect_stream(
    out: &mut Vec<u8>,
    id: ObjectId,
    dict: &Dict,
    data: &[u8],
) -> Result<usize, Error> {
    let start = out.len();
    reference(out, id);
    out.extend_from_slice(b" obj\n");
    match stream(out, dict, data, 0) {
        Ok(()) => {
            out.extend_from_slice(b"\nendobj\n");
            Ok(start)
        }
        Err(e) => {
            out.truncate(start);
            Err(e.within(id))
        }
    }
}

/// `x` in its shortest form that reads back as the same f64, never with
/// an exponent and always with a point, so an integral real reads back
/// as a real. Magnitudes past `MAX_REAL`, 3.4e38, are clamped to it, and
/// those under `MIN_REAL`, 1.175e-38, written as 0, as is -0. NaN and the
/// infinities are refused ([`Error::NonFinite`]), and nothing is written.
pub fn real(out: &mut Vec<u8>, x: f64) -> Result<(), Error> {
    if !x.is_finite() {
        return Err(Error::NonFinite { id: None });
    }
    let x = if x.abs() > MAX_REAL {
        MAX_REAL.copysign(x)
    } else if x.abs() < MIN_REAL {
        0.0
    } else {
        x
    };
    let start = out.len();
    let _ = write!(out, "{x}");
    if !out[start..].contains(&b'.') {
        out.extend_from_slice(b".0");
    }
    Ok(())
}

/// `n` in decimal, as `{n}` formats it.
pub fn decimal(out: &mut Vec<u8>, mut n: u64) {
    let mut digits = [0; 20];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[at..]);
}

/// `n` in decimal, as `{n}` formats it: a minus before a negative.
fn integer(out: &mut Vec<u8>, n: i64) {
    if n < 0 {
        out.push(b'-');
    }
    decimal(out, n.unsigned_abs());
}

/// `id`'s number and generation, as `n g`.
fn reference(out: &mut Vec<u8>, (n, g): ObjectId) {
    decimal(out, n.into());
    out.push(b' ');
    decimal(out, g.into());
}

/// `b` as two uppercase hex digits, as `{b:02X}` formats it.
fn hex(out: &mut Vec<u8>, b: u8) {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    out.extend_from_slice(&[DIGITS[usize::from(b >> 4)], DIGITS[usize::from(b & 15)]]);
}

/// `/` and `name`, `#`, white-space, a delimiter or a byte outside
/// `!`–`~` written `#xx`.
pub fn name(out: &mut Vec<u8>, name: &[u8]) {
    out.push(b'/');
    if name.iter().all(|&b| REGULAR[usize::from(b)]) {
        out.extend_from_slice(name);
        return;
    }
    for &b in name {
        if REGULAR[usize::from(b)] {
            out.push(b);
        } else {
            out.push(b'#');
            hex(out, b);
        }
    }
}

/// `text` as a string in `form`. A literal escapes `\`, CR (which would
/// read back as LF) and any parenthesis left unbalanced.
pub fn string(out: &mut Vec<u8>, text: &[u8], form: Form) {
    match form {
        Form::Literal if !text.iter().any(|b| b"\\\r()".contains(b)) => {
            out.push(b'(');
            out.extend_from_slice(text);
            out.push(b')');
        }
        Form::Literal => {
            let mut open = Vec::new();
            let mut lone = Vec::new();
            for (i, &b) in text.iter().enumerate() {
                match b {
                    b'(' => open.push(i),
                    b')' if open.pop().is_none() => lone.push(i),
                    _ => {}
                }
            }
            lone.extend(open);
            lone.sort_unstable();
            let mut lone = lone.into_iter().peekable();
            out.push(b'(');
            for (i, &b) in text.iter().enumerate() {
                match b {
                    b'\\' => out.extend_from_slice(b"\\\\"),
                    b'\r' => out.extend_from_slice(b"\\r"),
                    b'(' | b')' if lone.next_if_eq(&i).is_some() => {
                        out.extend_from_slice(&[b'\\', b]);
                    }
                    b => out.push(b),
                }
            }
            out.push(b')');
        }
        Form::Hex => {
            out.push(b'<');
            for &b in text {
                hex(out, b);
            }
            out.push(b'>');
        }
    }
}

/// `obj`, nested `depth` deep in what's written.
fn value(out: &mut Vec<u8>, obj: &Object, depth: usize) -> Result<(), Error> {
    let limit = Limits::DEFAULT.nesting;
    if depth > limit {
        return Err(Error::Nested { id: None, limit });
    }
    match obj.value() {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Integer(i) => integer(out, *i),
        Value::Real(x) => real(out, *x)?,
        Value::Name(n) => name(out, n),
        Value::String(Str::Bytes(text), form) => {
            string(out, text, form.unwrap_or(Form::Literal));
        }
        Value::String(Str::Text(_), _) => return Err(Error::Text { id: None }),
        Value::String(Str::Encrypted(_), _) => return Err(Error::Encrypted { id: None }),
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 && spaced(item) {
                    out.push(b' ');
                }
                value(out, item, depth + 1)?;
            }
            out.push(b']');
        }
        Value::Dict(d) => dictionary(out, d, None, depth)?,
        Value::Stream(s) => {
            let Some(data) = s.data.bytes() else {
                return Err(Error::Data { id: None });
            };
            stream(out, &s.dict, data, depth)?;
        }
        Value::Ref(id) => {
            reference(out, *id);
            out.extend_from_slice(b" R");
        }
    }
    Ok(())
}

/// A stream of `dict` and `data`, nested `depth` deep.
fn stream(out: &mut Vec<u8>, dict: &Dict, data: &[u8], depth: usize) -> Result<(), Error> {
    dictionary(out, dict, Some(data.len()), depth)?;
    out.extend_from_slice(b"\nstream\n");
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream");
    Ok(())
}

/// `d`, nested `depth` deep, every entry in order, a key written twice
/// included; its `/Length` the stream's `length` when it's a stream's.
fn dictionary(
    out: &mut Vec<u8>,
    d: &Dict,
    length: Option<usize>,
    depth: usize,
) -> Result<(), Error> {
    out.extend_from_slice(b"<<");
    for Entry {
        key: k, value: v, ..
    } in d.entries()
    {
        name(out, k);
        match length {
            Some(n) if k == b"Length" => {
                out.push(b' ');
                decimal(out, n as u64);
            }
            _ => {
                if spaced(v) {
                    out.push(b' ');
                }
                value(out, v, depth + 1)?;
            }
        }
    }
    if let Some(n) = length {
        if !d.has(b"Length") {
            out.extend_from_slice(b"/Length ");
            decimal(out, n as u64);
        }
    }
    out.extend_from_slice(b">>");
    Ok(())
}

/// Whether `obj` starts with a regular character, so needs white-space
/// after what comes before it.
fn spaced(obj: &Object) -> bool {
    matches!(
        obj.value(),
        Value::Null | Value::Bool(_) | Value::Integer(_) | Value::Real(_) | Value::Ref(_)
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::dict;
    use crate::fuzz;
    use crate::object::{Data, Span, Stream};
    use crate::parse::{Body, Parser};

    /// The objects a file holds, by number and generation.
    struct Objects(BTreeMap<ObjectId, Object>);

    impl Objects {
        fn get(&self, id: ObjectId) -> Option<&Object> {
            self.0.get(&id)
        }
    }

    /// What [`file`] wrote, each object read back in turn by the parser, a
    /// stream's data by its `/Length`, and its places taken out, so that it
    /// compares with what was built.
    fn open(bytes: &[u8]) -> Objects {
        let mut objects = BTreeMap::new();
        let mut at = 0;
        while at < bytes.len() {
            let mut parser = Parser::at(bytes, at);
            let id = parser.header().unwrap();
            let mut obj = match parser.body(id, &mut ()).unwrap() {
                Body::Object(obj) => obj,
                Body::Stream(head) => {
                    let length = head.dict().get(b"Length").and_then(Object::as_integer);
                    let data = head.declared(length.unwrap() as usize).unwrap();
                    parser.stream(head, data, &mut ()).unwrap()
                }
            };
            fuzz::unplace(&mut obj, bytes);
            let endobj = b"\nendobj\n";
            assert!(bytes[parser.offset()..].starts_with(endobj));
            at = parser.offset() + endobj.len();
            objects.insert(id, obj);
        }
        Objects(objects)
    }

    /// A file's objects as the writer writes them: `objects`, each an
    /// indirect object, one after another.
    fn file(objects: &[(u32, Object)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (num, obj) in objects {
            indirect(&mut out, (*num, 0), obj).unwrap();
        }
        out
    }

    /// `obj` written as object 2 of a file, and read back.
    fn round_trip(obj: &Object) -> Object {
        open(&file(&[(2, obj.clone())]))
            .get((2, 0))
            .unwrap()
            .clone()
    }

    /// `obj` as written.
    fn written(obj: &Object) -> String {
        let mut out = Vec::new();
        write(&mut out, obj).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn reals_round_trip_at_f64() {
        let kept = [
            0.1,
            1.0 / 3.0,
            595.32,
            1e-7,
            1e20,
            3.4e38,
            -3.4e38,
            1.2e-38,
            612.0,
            -612.0,
            0.0,
        ]
        .map(Object::from)
        .into_iter()
        .chain([0.1f32, 1.0 / 3.0, 595.32, 841.89, 1e20].map(Object::from));
        let clamped = [
            (1e39, 3.4e38),
            (-f64::MAX, -3.4e38),
            (f64::from(f32::MAX), 3.4e38),
            (1e-39, 0.0),
            (-1e-300, 0.0),
            (-0.0, 0.0),
        ];
        let wrote: Vec<Object> = kept
            .clone()
            .chain(clamped.iter().map(|&(x, _)| Object::from(x)))
            .collect();
        let want: Vec<f64> = kept
            .map(|o| match *o.value() {
                Value::Real(x) => x,
                _ => panic!("{o:?} isn't a real"),
            })
            .chain(clamped.iter().map(|&(_, y)| y))
            .collect();
        let Value::Array(back) = round_trip(&Object::from(wrote)).into_value() else {
            panic!("not an array");
        };
        assert_eq!(back.len(), want.len());
        for (back, want) in back.iter().zip(want) {
            let Value::Real(x) = back.value() else {
                panic!("{want} read back as {back:?}");
            };
            assert_eq!(x.to_bits(), want.to_bits(), "{want} read back as {x}");
        }
        // Shortest digits, a point always, and never an exponent.
        assert_eq!(written(&Object::from(612.0)), "612.0");
        assert_eq!(written(&Object::from(-0.0)), "0.0");
        assert_eq!(written(&Object::from(1e-7)), "0.0000001");
        assert_eq!(written(&Object::from(0.1f32)), "0.1");
        assert_eq!(written(&Object::from(1e20)), "100000000000000000000.0");
    }

    #[test]
    fn nan_and_infinities_are_refused() {
        for x in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut out = b"%PDF-1.7\n".to_vec();
            assert_eq!(
                write(&mut out, &Object::from(x)),
                Err(Error::NonFinite { id: None })
            );
            let deep = Object::from(vec![
                Object::from(1),
                Object::from(dict! { "Rect" => vec![Object::from(x)] }),
            ]);
            assert_eq!(
                indirect(&mut out, (7, 0), &deep),
                Err(Error::NonFinite { id: Some((7, 0)) })
            );
            let stream = Stream::new(
                dict! { "Decode" => vec![Object::from(x)] },
                b"data".to_vec(),
            );
            assert_eq!(
                indirect(&mut out, (8, 1), &Object::from(stream)),
                Err(Error::NonFinite { id: Some((8, 1)) })
            );
            assert_eq!(out, b"%PDF-1.7\n", "nothing written");
        }
        assert_eq!(
            Error::NonFinite { id: None }.to_string(),
            "a real that isn't a finite number"
        );
        assert_eq!(
            Error::NonFinite { id: Some((7, 0)) }.to_string(),
            "a real that isn't a finite number in object 7 0"
        );
    }

    #[test]
    fn strings_names_and_streams_round_trip() {
        let literal =
            |s: &[u8]| Object::new(Value::String(Str::Bytes(s.to_vec()), Some(Form::Literal)));
        let hex = |s: &[u8]| Object::hex(s);
        let name = |s: &[u8]| Object::new(Value::Name(s.to_vec()));
        let values = Object::from(vec![
            literal(b"balanced (parens (nest)) as they are"),
            literal(b"unbalanced ) and ( and ((x)"),
            literal(b"a back\\slash, and \\) escaped"),
            literal(b"CR\r alone, CR LF\r\n and LF\n"),
            literal(b"\x00\x80\xff binary"),
            literal(b""),
            hex(b"\x00\x01()\\\r"),
            hex(b""),
            name(b"A B"),
            name(b"#23 and /slash"),
            name(b"\x00\t\x7f\x80\xff"),
            name(b"()<>[]{}%"),
            name(b"Plain-Name_1.0"),
            name(b""),
            name(b"After"),
            Object::from(dict! {
                "True" => true,
                "Null" => Object::null(),
                "Ref" => (3, 0),
                "Neg" => -1,
                "Mixed" => vec![
                    name(b"A"),
                    name(b"B"),
                    Object::from(1),
                    Object::from(2.5),
                    Object::from(false),
                    Object::null(),
                    Object::from((3, 0)),
                    literal(b"s"),
                ],
            }),
        ]);
        let content = b"\x00binary\nendstream\nendobj\n\xff".to_vec();
        let mut long = Stream::new(dict! { "Kind" => "Test" }, content.clone());
        long.dict.set("Length", 999);
        let mut unsaid = Stream::new(dict! {}, content.clone());
        unsaid.dict.remove(b"Length");
        let mut indirect = Stream::new(dict! {}, content.clone());
        indirect.dict.set("Length", Object::from((9, 0)));
        let pdf = open(&file(&[
            (2, values.clone()),
            (3, Object::from(long)),
            (4, Object::from(unsaid)),
            (5, Object::from(indirect)),
        ]));
        assert_eq!(pdf.get((2, 0)).unwrap(), &values);
        for id in [(3, 0), (4, 0), (5, 0)] {
            let s = pdf.get(id).unwrap().as_stream().unwrap();
            assert_eq!(s.data.bytes(), Some(content.as_slice()), "{id:?}");
            let length = s.dict.get(b"Length").unwrap();
            assert_eq!(length, &Object::from(content.len() as i64), "{id:?}");
        }
        assert_eq!(
            pdf.get((3, 0))
                .unwrap()
                .as_stream()
                .unwrap()
                .dict
                .get(b"Kind")
                .unwrap(),
            &name(b"Test")
        );
    }

    /// Numbers, names and strings are written byte by byte: as `write!`
    /// formatted them.
    #[test]
    fn numbers_names_and_strings_write_as_formatting_would() {
        let bytes = |obj: &Object| {
            let mut out = Vec::new();
            write(&mut out, obj).unwrap();
            out
        };
        let mut integers = vec![i64::MAX, i64::MIN, i64::MIN + 1, u32::MAX.into()];
        for power in 0..19 {
            let ten = 10i64.pow(power);
            integers.extend([ten - 1, ten, ten + 1, -ten, 1 - ten]);
        }
        for i in integers {
            assert_eq!(written(&Object::from(i)), format!("{i}"));
        }
        for id in [(0, 0), (7, 0), (10, 1), (u32::MAX, u16::MAX)] {
            let (n, g) = id;
            assert_eq!(written(&Object::from(id)), format!("{n} {g} R"));
            let mut out = Vec::new();
            indirect(&mut out, id, &Object::null()).unwrap();
            assert_eq!(out, format!("{n} {g} obj\nnull\nendobj\n").into_bytes());
        }
        let every: Vec<u8> = (0..=255).collect();
        let mut name = b"/".to_vec();
        for &b in &every {
            match b.is_ascii_graphic() && !b"#()<>[]{}/%".contains(&b) {
                true => name.push(b),
                false => name.extend(format!("#{b:02X}").bytes()),
            }
        }
        assert_eq!(bytes(&Object::new(Value::Name(every.clone()))), name);
        assert_eq!(
            written(&Object::new(Value::Name(b"Plain-Name_1.0".to_vec()))),
            "/Plain-Name_1.0"
        );
        let hex: String = every.iter().map(|b| format!("{b:02X}")).collect();
        let string =
            |text: &[u8], form| Object::new(Value::String(Str::Bytes(text.to_vec()), Some(form)));
        assert_eq!(written(&string(&every, Form::Hex)), format!("<{hex}>"));
        // A literal with nothing to escape is written as it is.
        let plain: Vec<u8> = (every.iter().copied())
            .filter(|b| !b"\\\r()".contains(b))
            .collect();
        let want = [b"(".as_slice(), &plain, b")"].concat();
        assert_eq!(bytes(&string(&plain, Form::Literal)), want);
        assert_eq!(
            written(&string(b"a\\b\rc(d) e)", Form::Literal)),
            "(a\\\\b\\rc(d) e\\))"
        );
        let mut stream = Stream::new(dict! {}, vec![0; 1234]);
        let head = |s: &Stream| bytes(&Object::from(s.clone()))[..16].to_vec();
        assert_eq!(head(&stream), b"<</Length 1234>>");
        stream.dict.remove(b"Length");
        assert_eq!(head(&stream), b"<</Length 1234>>");
    }

    /// A string with no form is written as a literal one, and reads back
    /// as one.
    #[test]
    fn a_string_with_no_form_is_written_literal() {
        assert_eq!(written(&Object::string("a (b")), "(a \\(b)");
        assert_eq!(written(&Object::bytes(*b"\x01")), "(\x01)");
        let back = round_trip(&Object::string("x"));
        assert_eq!(
            *back.value(),
            Value::String(Str::Bytes(b"x".to_vec()), Some(Form::Literal))
        );
    }

    /// What can't be written as the value is refused, said of the object
    /// it's in when that's known, and nothing is written, wherever in the
    /// value it is.
    #[test]
    fn each_refusal_says_why_and_writes_nothing() {
        let text = Object::new(Value::String(Str::Text("x".to_string()), None));
        let ciphertext = Value::String(Str::Encrypted(b"x".to_vec()), Some(Form::Hex));
        let at = Stream {
            dict: dict! { "Length" => 4 },
            data: Data::At(Span::new(None, 0, 4)),
        };
        let mut deep = Object::from(1);
        for _ in 0..=Limits::DEFAULT.nesting {
            deep = Object::from(vec![deep]);
        }
        let cases = [
            (
                text,
                Error::Text { id: None },
                "a string read as text, whose bytes aren't known",
            ),
            (
                Object::new(ciphertext),
                Error::Encrypted { id: None },
                "a string that's still ciphertext",
            ),
            (
                Object::from(at),
                Error::Data { id: None },
                "a stream whose data isn't in memory",
            ),
            (
                Object::stream(dict! {}),
                Error::Data { id: None },
                "a stream whose data isn't in memory",
            ),
            (
                deep,
                Error::Nested {
                    id: None,
                    limit: 256,
                },
                "arrays and dictionaries nested deeper than 256",
            ),
        ];
        for (refused, error, said) in cases {
            let within = error.within((7, 2));
            assert_eq!(error.to_string(), said);
            assert_eq!(within.to_string(), format!("{said} in object 7 2"));
            // Alone, and after what the writer would write.
            let after = Object::from(vec![
                Object::from(1),
                Object::from(dict! { "A" => 2, "B" => refused.clone() }),
            ]);
            let mut out = b"%PDF-1.7\n".to_vec();
            for obj in [&refused, &after] {
                assert_eq!(write(&mut out, obj), Err(error), "{said}");
                assert_eq!(indirect(&mut out, (7, 2), obj), Err(within), "{said}");
            }
            let dict = dict! { "B" => refused };
            assert_eq!(indirect_stream(&mut out, (7, 2), &dict, b""), Err(within));
            assert_eq!(out, b"%PDF-1.7\n", "nothing written: {said}");
        }
    }

    /// A stream's data given apart is written as a stream holding it is,
    /// and a stream read where it was written, written so, is its bytes.
    #[test]
    fn a_streams_data_given_apart_is_written_as_it_would_be_held() {
        let data = b"\x00binary\nendstream\n".to_vec();
        let dict = dict! { "Kind" => "Test", "Length" => Object::from((9, 0)) };
        let held = Object::from(Stream {
            dict: dict.clone(),
            data: Data::Bytes(data.clone()),
        });
        let mut written = b"%PDF-1.7\n".to_vec();
        assert_eq!(indirect(&mut written, (4, 1), &held), Ok(9));
        let mut apart = b"%PDF-1.7\n".to_vec();
        assert_eq!(indirect_stream(&mut apart, (4, 1), &dict, &data), Ok(9));
        assert_eq!(apart, written);
        assert!(
            apart.ends_with(b"/Length 18>>\nstream\n\x00binary\nendstream\n\nendstream\nendobj\n")
        );
        // Read, its data is where it was written; written apart again, the
        // same bytes.
        let mut parser = Parser::at(&apart, 9);
        let id = parser.header().unwrap();
        let Ok(Body::Stream(head)) = parser.body(id, &mut ()) else {
            panic!("a stream");
        };
        let declared = head.declared(data.len()).unwrap();
        let read = parser.stream(head, declared, &mut ()).unwrap();
        let s = read.as_stream().unwrap();
        assert_eq!(write(&mut Vec::new(), &read), Err(Error::Data { id: None }));
        let mut again = b"%PDF-1.7\n".to_vec();
        let at = s.data.at().unwrap();
        indirect_stream(&mut again, id, &s.dict, &apart[at.start..at.end]).unwrap();
        assert_eq!(again, apart);
    }

    /// Every entry of a dictionary is written, in order, a key written
    /// twice included, and reads back so; a stream's `/Length` is its
    /// data's in each of its entries.
    #[test]
    fn a_key_written_twice_is_written_twice() {
        let d = dict! { "A" => 1, "B" => 2, "A" => 3 };
        assert_eq!(written(&Object::from(d.clone())), "<</A 1/B 2/A 3>>");
        assert_eq!(round_trip(&Object::from(d.clone())), Object::from(d));
        let s = Stream {
            dict: dict! { "Length" => 1, "Filter" => "FlateDecode", "Length" => 2 },
            data: Data::Bytes(b"data".to_vec()),
        };
        assert_eq!(
            written(&Object::from(s)),
            "<</Length 4/Filter/FlateDecode/Length 4>>\nstream\ndata\nendstream"
        );
    }

    /// Random numbers, by xorshift from a seed, as the fuzz tests have them.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, below: usize) -> usize {
            (self.next() % below as u64) as usize
        }

        /// At most `most` bytes of any value, half of them those a name or
        /// a string writes otherwise than as itself.
        fn bytes(&mut self, most: usize) -> Vec<u8> {
            const SPECIAL: &[u8] = b"\\()\r\n\t #/%<>[]{}\x00\x7f\x80\xff";
            (0..self.below(most + 1))
                .map(|_| match self.below(2) {
                    0 => SPECIAL[self.below(SPECIAL.len())],
                    _ => self.next() as u8,
                })
                .collect()
        }

        fn integer(&mut self) -> i64 {
            match self.below(4) {
                0 => [0, 1, -1, i64::MAX, i64::MIN, i64::MIN + 1][self.below(6)],
                1 => self.below(2000) as i64 - 1000,
                _ => self.next() as i64,
            }
        }

        /// Any finite real: Annex C's bounds and either side of them, ones
        /// past i64's range, decimals, and any bits that are finite.
        fn real(&mut self) -> f64 {
            let bounds = [3.4e38f64, 1.175e-38];
            match self.below(4) {
                0 => {
                    let x = bounds[self.below(2)];
                    let x = match self.below(3) {
                        0 => f64::from_bits(x.to_bits() - 1),
                        1 => x,
                        _ => f64::from_bits(x.to_bits() + 1),
                    };
                    [x, -x][self.below(2)]
                }
                1 => [
                    0.0,
                    -0.0,
                    f64::MAX,
                    f64::MIN,
                    f64::MIN_POSITIVE,
                    5e-324,
                    1e19,
                    0.1,
                ][self.below(8)],
                2 => self.integer() as f64 / 10f64.powi(self.below(8) as i32),
                _ => loop {
                    let x = f64::from_bits(self.next());
                    if x.is_finite() {
                        break x;
                    }
                },
            }
        }

        /// A value of no more than `*budget` values, of which it takes
        /// its own, nested no more than `depth` deeper.
        fn value(&mut self, budget: &mut usize, depth: usize) -> Object {
            *budget = budget.saturating_sub(1);
            let kinds = if *budget > 0 && depth > 0 { 9 } else { 7 };
            match self.below(kinds) {
                0 => match self.below(3) {
                    0 => Object::null(),
                    n => Object::from(n == 1),
                },
                1 => Object::from(self.integer()),
                2 => Object::from(self.real()),
                3 => Object::new(Value::Name(self.bytes(12))),
                4 => {
                    let form = [None, Some(Form::Literal), Some(Form::Hex)][self.below(3)];
                    Object::new(Value::String(Str::Bytes(self.bytes(24)), form))
                }
                5 => {
                    let n = [0, 1, u32::MAX, self.next() as u32][self.below(4)];
                    let g = [0, 1, u16::MAX, self.next() as u16][self.below(4)];
                    Object::from((n, g))
                }
                6 => Object::from(Vec::new()),
                7 => Object::from(self.array(budget, depth - 1)),
                _ => Object::from(self.dict(budget, depth - 1)),
            }
        }

        /// An array of values no more than `depth` deeper.
        fn array(&mut self, budget: &mut usize, depth: usize) -> Vec<Object> {
            (0..self.below(8))
                .map(|_| self.value(budget, depth))
                .collect()
        }

        /// A dictionary of values no more than `depth` deeper, now and then
        /// with a key written twice, or `/Length`.
        fn dict(&mut self, budget: &mut usize, depth: usize) -> Dict {
            let mut d = Dict::new();
            for _ in 0..self.below(8) {
                let key = match (self.below(6), d.entries().last()) {
                    (0, Some(entry)) => entry.key.clone(),
                    (1, _) => b"Length".to_vec(),
                    _ => self.bytes(8),
                };
                d.push(key, self.value(budget, depth));
            }
            d
        }

        /// A value nested as deep as a parser reads it, `depth` deep
        /// already, each level an array or a dictionary, beside a few
        /// values more.
        fn deep(&mut self, depth: usize) -> Object {
            let mut inner = match self.below(3) {
                0 => Object::from(Vec::new()),
                1 => Object::from(Dict::new()),
                _ => self.value(&mut 1, 0),
            };
            for _ in depth..Limits::DEFAULT.nesting {
                let beside = self.value(&mut 1, 0);
                inner = match self.below(2) {
                    0 => Object::from(vec![beside, inner]),
                    _ => Object::from(dict! { "A" => beside, "B" => inner }),
                };
            }
            inner
        }

        /// A stream of up to 64 bytes, `endstream` among them now and then,
        /// its dictionary's values as deep as a parser reads them when
        /// `deep`.
        fn stream(&mut self, budget: &mut usize, deep: bool) -> Object {
            let mut data = self.bytes(64);
            if self.below(4) == 0 {
                data.extend_from_slice(b"\nendstream\n");
            }
            let mut dict = self.dict(budget, 3);
            if deep {
                dict.push("Deep", self.deep(1));
            }
            Object::from(Stream {
                dict,
                data: Data::Bytes(data),
            })
        }

        /// A value of a few hundred values at most, one in a hundred
        /// nested as deep as a parser reads it; one in eight a stream,
        /// three a dictionary and two an array.
        fn object(&mut self) -> Object {
            let deep = self.below(100) == 0;
            let most = self.below(300);
            let mut budget = 1 + self.below(1 + most);
            match (self.below(8), deep) {
                (0, _) => self.stream(&mut budget, deep),
                (_, true) => self.deep(0),
                (1..=3, _) => Object::from(self.dict(&mut budget, 5)),
                (4 | 5, _) => Object::from(self.array(&mut budget, 5)),
                _ => self.value(&mut budget, 6),
            }
        }

        fn id(&mut self) -> ObjectId {
            (self.next() as u32, self.next() as u16)
        }
    }

    /// Any value the writer writes reads back as the same value as it was
    /// written, places aside: a string with no form as a literal one, a real
    /// as the writer clamps it, and a stream's `/Length` its data's; and
    /// written again, it's the same bytes. Values, and streams, as indirect
    /// objects, which is how a stream is read; values as direct objects too.
    #[test]
    fn values_written_read_back_as_written_and_write_the_same_bytes_again() {
        let mut random = Random(0x9e37_79b9_7f4a_7c15);
        for _ in 0..20_000 {
            let object = random.object();
            let id = random.id();
            fuzz::round_trip(&object, Some(id), &[], None, Limits::DEFAULT);
            if object.as_stream().is_none() {
                fuzz::round_trip(&object, None, &[], None, Limits::DEFAULT);
            }
        }
    }

    /// How many values `o` holds that hold none: its numbers, names and
    /// the like, and its empty arrays and dictionaries; a stream's are its
    /// dictionary's, but in a `/Length`, whose value isn't written.
    fn leaves(o: &Object) -> usize {
        match o.value() {
            Value::Array(a) if !a.is_empty() => a.iter().map(leaves).sum(),
            Value::Dict(d) if !d.is_empty() => d.entries().iter().map(|e| leaves(&e.value)).sum(),
            Value::Stream(s) => (s.dict.entries().iter())
                .filter(|e| e.key != b"Length")
                .map(|e| leaves(&e.value))
                .sum(),
            _ => 1,
        }
    }

    /// `o` with `string` in place of its leaf `*n`, counting from 0 as
    /// [`leaves`] counts them.
    fn plant(o: &mut Object, n: &mut usize, string: &Value) {
        match o.value_mut() {
            Value::Array(a) if !a.is_empty() => a.iter_mut().for_each(|e| plant(e, n, string)),
            Value::Dict(d) if !d.is_empty() => {
                d.entries_mut().for_each(|e| plant(&mut e.value, n, string));
            }
            Value::Stream(s) => (s.dict.entries_mut())
                .filter(|e| e.key != b"Length")
                .for_each(|e| plant(&mut e.value, n, string)),
            leaf => {
                if *n == 0 {
                    *leaf = string.clone();
                }
                // Past 0, it never comes to 0 again.
                *n = n.wrapping_sub(1);
            }
        }
    }

    /// `object` with `string` in place of one of its leaves, chosen by
    /// `random`, or in the dictionary of a stream that has none.
    fn planted(random: &mut Random, object: &Object, string: Str) -> Object {
        let mut object = object.clone();
        let string = Value::String(string, [None, Some(Form::Hex)][random.below(2)]);
        match leaves(&object) {
            0 => match object.as_stream_mut() {
                Some(s) => s.dict.push("S", Object::new(string)),
                None => unreachable!("only a stream holds no leaf"),
            },
            n => plant(&mut object, &mut random.below(n), &string),
        }
        object
    }

    /// Random values, each holding a string read as text in place of one
    /// of its values, are refused, and nothing is written: through
    /// `write`, `indirect`, and for a stream, `indirect_stream`.
    #[test]
    fn values_holding_text_are_refused_wherever_it_is() {
        refused_wherever(Str::Text("text".to_string()), |id| Error::Text { id });
    }

    /// Random values, each holding ciphertext in place of one of its
    /// values, are refused, and nothing is written: through `write`,
    /// `indirect`, and for a stream, `indirect_stream`.
    #[test]
    fn values_holding_ciphertext_are_refused_wherever_it_is() {
        let ciphertext = Str::Encrypted(b"\x8f\x02".to_vec());
        refused_wherever(ciphertext, |id| Error::Encrypted { id });
    }

    /// Random values with `string` planted in each, refused as `error`
    /// says.
    fn refused_wherever(string: Str, error: fn(Option<ObjectId>) -> Error) {
        let mut random = Random(0x2545_f491_4f6c_dd1d);
        for _ in 0..2_000 {
            let object = random.object();
            let object = planted(&mut random, &object, string.clone());
            let id = random.id();
            let mut out = b"%PDF-1.7\n".to_vec();
            assert_eq!(write(&mut out, &object), Err(error(None)), "{object:?}");
            assert_eq!(indirect(&mut out, id, &object), Err(error(Some(id))));
            if let Some(s) = object.as_stream() {
                let data = s.data.bytes().unwrap();
                let apart = indirect_stream(&mut out, id, &s.dict, data);
                assert_eq!(apart, Err(error(Some(id))));
            }
            assert_eq!(out, b"%PDF-1.7\n", "nothing written");
        }
    }

    /// The writer writes values nested as deep as a parser reads them under
    /// the default limits, which reads them back, and refuses one deeper,
    /// as the parser does.
    #[test]
    fn values_nest_as_deep_as_the_parser_reads_them_and_no_deeper() {
        let limit = Limits::DEFAULT.nesting;
        type Wrap = fn(Object) -> Object;
        let wraps: [(Wrap, &[u8], &[u8]); 2] = [
            (|o| Object::from(vec![o]), b"[", b"]"),
            (|o| Object::from(dict! { "K" => o }), b"<</K ", b">>"),
        ];
        // An integer, and an empty array, which is one deeper than what's
        // in it, inside as many arrays or dictionaries as may be.
        for inner in [Object::from(1), Object::from(Vec::new())] {
            for (wrap, open, close) in wraps {
                let mut deepest = inner.clone();
                for _ in 0..limit {
                    deepest = wrap(deepest);
                }
                let mut out = Vec::new();
                write(&mut out, &deepest).unwrap();
                let mut read = Parser::at(&out, 0).value(&mut ()).unwrap();
                fuzz::unplace(&mut read, &out);
                assert_eq!(read, deepest);
                assert!(indirect(&mut Vec::new(), (1, 0), &deepest).is_ok());
                // One deeper: the writer refuses it, and the parser refuses
                // it written by hand, staying where it was.
                let past = wrap(deepest);
                let nested = Error::Nested { id: None, limit };
                assert_eq!(write(&mut Vec::new(), &past), Err(nested));
                assert_eq!(
                    indirect(&mut Vec::new(), (1, 0), &past),
                    Err(nested.within((1, 0)))
                );
                let text = [open, &out, close].concat();
                let mut parser = Parser::at(&text, 0);
                let refused = parser.value(&mut ());
                let Err(crate::parse::Error::Nested { limit: refused, .. }) = refused else {
                    panic!("read past the limit: {refused:?}");
                };
                assert_eq!(refused, limit);
                assert_eq!(parser.offset(), 0);
            }
        }
    }
}
