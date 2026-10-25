//! Writing objects as PDF syntax, for the tests: whole files with a
//! cross-reference table or an uncompressed cross-reference stream, and
//! files assembled from objects' bytes as written by hand.

use std::fmt::Write as _;

use crate::object::{Dict, Object, Str, Value};

/// The kind of cross-reference section a file is written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Xref {
    Table,
    Stream,
}

/// An indirect object to write: a value, or a stream's dictionary and
/// data.
#[derive(Debug, Clone)]
pub(crate) enum Body {
    Object(Object),
    Stream(Dict, Vec<u8>),
}

impl Body {
    pub(crate) fn stream(dict: Dict, data: &[u8]) -> Body {
        Body::Stream(dict, data.to_vec())
    }
}

impl From<Object> for Body {
    fn from(object: Object) -> Body {
        Body::Object(object)
    }
}

impl From<Dict> for Body {
    fn from(dict: Dict) -> Body {
        Body::Object(dict.into())
    }
}

/// `objects`, each object `n` generation 0, written as a file at
/// `version` with a section of `kind`, under `trailer`, which is given
/// `/Size` (and for a stream, `/Type /XRef` and `/W`).
pub(crate) fn file(version: &str, objects: &[(u32, Body)], trailer: Dict, kind: Xref) -> Vec<u8> {
    let mut out = format!("%PDF-{version}\n").into_bytes();
    out.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n");
    let mut offsets = Vec::new();
    for (n, body) in objects {
        offsets.push((*n, out.len()));
        indirect(&mut out, *n, body);
    }
    let last = objects.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let at = |n: u32| offsets.iter().find(|(m, _)| *m == n).map(|(_, at)| *at);
    let mut trailer = trailer;
    match kind {
        Xref::Table => {
            let size = last + 1;
            let xref = out.len();
            let _ = write!(out_string(&mut out), "xref\n0 {size}\n");
            for n in 0..size {
                let row = match at(n) {
                    Some(offset) if n > 0 => format!("{offset:010} 00000 n \n"),
                    _ => format!("0000000000 {:05} f \n", if n == 0 { 65535 } else { 0 }),
                };
                out.extend_from_slice(row.as_bytes());
            }
            trailer.set("Size", i64::from(size));
            out.extend_from_slice(b"trailer\n");
            dict(&mut out, &trailer);
            let _ = write!(out_string(&mut out), "\nstartxref\n{xref}\n%%EOF\n");
        }
        Xref::Stream => {
            let own = last + 1;
            let size = own + 1;
            let xref = out.len();
            let mut rows = Vec::new();
            for n in 0..size {
                let (kind, field) = match (n, at(n)) {
                    (n, _) if n == own => (1u8, xref),
                    (0, _) | (_, None) => (0, 0),
                    (_, Some(offset)) => (1, offset),
                };
                rows.push(kind);
                rows.extend_from_slice(&(field as u32).to_be_bytes());
                rows.extend_from_slice(&[0, if n == 0 { 0xFF } else { 0 }]);
            }
            trailer.set("Type", "XRef");
            trailer.set("Size", i64::from(size));
            trailer.set("W", vec![Object::from(1), Object::from(4), Object::from(2)]);
            trailer.set(
                "Index",
                vec![Object::from(0), Object::from(i64::from(size))],
            );
            indirect(&mut out, own, &Body::Stream(trailer, rows));
            let _ = write!(out_string(&mut out), "startxref\n{xref}\n%%EOF\n");
        }
    }
    out
}

/// A file of these objects, each written between its `n 0 obj` and
/// `endobj` as given, with a table and `trailer` as written. The bodies
/// may be anything at all.
pub(crate) fn assembled(version: &str, objects: &[(u32, &[u8])], trailer: &str) -> Vec<u8> {
    let mut out = format!("%PDF-{version}\n").into_bytes();
    let mut offsets = Vec::new();
    for (n, body) in objects {
        offsets.push((*n, out.len()));
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let size = objects.iter().map(|(n, _)| *n).max().unwrap_or(0) + 1;
    let xref = out.len();
    let _ = write!(out_string(&mut out), "xref\n0 {size}\n");
    for n in 0..size {
        let row = match offsets.iter().find(|(m, _)| *m == n) {
            Some((_, offset)) if n > 0 => format!("{offset:010} 00000 n \n"),
            _ => format!("0000000000 {:05} f \n", if n == 0 { 65535 } else { 0 }),
        };
        out.extend_from_slice(row.as_bytes());
    }
    let _ = write!(
        out_string(&mut out),
        "trailer\n{trailer}\nstartxref\n{xref}\n%%EOF\n"
    );
    out
}

/// `out` as a string to write into, for ASCII only.
fn out_string(out: &mut Vec<u8>) -> Appender<'_> {
    Appender(out)
}

struct Appender<'a>(&'a mut Vec<u8>);

impl std::fmt::Write for Appender<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

fn indirect(out: &mut Vec<u8>, n: u32, body: &Body) {
    out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
    match body {
        Body::Object(object) => value(out, object),
        Body::Stream(stream_dict, data) => {
            let mut stream_dict = stream_dict.clone();
            stream_dict.set("Length", data.len() as i64);
            dict(out, &stream_dict);
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(data);
            out.extend_from_slice(b"\nendstream");
        }
    }
    out.extend_from_slice(b"\nendobj\n");
}

/// `o` as PDF syntax.
pub(crate) fn value(out: &mut Vec<u8>, o: &Object) {
    match &o.value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Integer(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Value::Real(r) => {
            let mut text = format!("{r}");
            if !text.contains('.') {
                text.push_str(".0");
            }
            out.extend_from_slice(text.as_bytes());
        }
        Value::Name(name) => self::name(out, name),
        Value::String(Str::Bytes(bytes) | Str::Encrypted(bytes)) => string(out, bytes),
        Value::String(Str::Text(text)) => {
            let utf16: Vec<u8> = [0xFE, 0xFF]
                .into_iter()
                .chain(text.encode_utf16().flat_map(u16::to_be_bytes))
                .collect();
            string(out, &utf16);
        }
        Value::Array(array) => {
            out.push(b'[');
            for (i, element) in array.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                value(out, element);
            }
            out.push(b']');
        }
        Value::Dict(d) => dict(out, d),
        Value::Stream(stream) => dict(out, &stream.dict),
        Value::Ref((n, g)) => out.extend_from_slice(format!("{n} {g} R").as_bytes()),
    }
}

fn dict(out: &mut Vec<u8>, d: &Dict) {
    out.extend_from_slice(b"<<");
    for entry in d.entries() {
        out.push(b' ');
        name(out, &entry.key);
        out.push(b' ');
        value(out, &entry.value);
    }
    out.extend_from_slice(b" >>");
}

/// A name, escaping `#` and any byte outside `!` to `~` that's regular.
fn name(out: &mut Vec<u8>, name: &[u8]) {
    out.push(b'/');
    for &b in name {
        if (0x21..=0x7E).contains(&b) && b != b'#' && pdf_tokenizer::is_regular(b) {
            out.push(b);
        } else {
            out.extend_from_slice(format!("#{b:02X}").as_bytes());
        }
    }
}

/// A literal string, escaping `(`, `)` and `\`, and writing any byte
/// outside printable ASCII as `\ddd`.
fn string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(b'(');
    for &b in bytes {
        match b {
            b'(' | b')' | b'\\' => out.extend_from_slice(&[b'\\', b]),
            0x20..=0x7E => out.push(b),
            _ => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
        }
    }
    out.push(b')');
}
