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

/// `data` as zlib data (RFC 1950) in deflate's stored blocks, not
/// compressed: for a `/FlateDecode` stream without a compressor.
pub(crate) fn zlib(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut blocks = data.chunks(usize::from(u16::MAX)).peekable();
    if data.is_empty() {
        out.extend([1, 0, 0, 0xff, 0xff]);
    }
    while let Some(block) = blocks.next() {
        let len = block.len() as u16;
        out.push(u8::from(blocks.peek().is_none()));
        out.extend(len.to_le_bytes());
        out.extend((!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    let (a, b) = (data.iter()).fold((1u32, 0u32), |(a, b), &byte| {
        let a = (a + u32::from(byte)) % 65_521;
        (a, (b + a) % 65_521)
    });
    out.extend((b << 16 | a).to_be_bytes());
    out
}

/// `data` as ASCII85, groups of zeros as `z`.
pub(crate) fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for group in data.chunks(4) {
        let mut four = [0u8; 4];
        four[..group.len()].copy_from_slice(group);
        let value = u32::from_be_bytes(four);
        if value == 0 && group.len() == 4 {
            out.push(b'z');
            continue;
        }
        let digits: Vec<u8> = (0..5)
            .rev()
            .map(|i| (value / 85u32.pow(i) % 85) as u8 + b'!')
            .collect();
        out.extend_from_slice(&digits[..group.len() + 1]);
    }
    out.extend_from_slice(b"~>");
    out
}

/// `data` as LZW codes, each byte its own 9-bit code, the table
/// cleared before it widens.
pub(crate) fn lzw(data: &[u8]) -> Vec<u8> {
    let mut codes = vec![256u16];
    for (i, &byte) in data.iter().enumerate() {
        if i > 0 && i % 250 == 0 {
            codes.push(256);
        }
        codes.push(u16::from(byte));
    }
    codes.push(257);
    let bits: Vec<bool> = (codes.iter())
        .flat_map(|&code| (0..9).rev().map(move |i| code >> i & 1 == 1))
        .collect();
    (bits.chunks(8))
        .map(|byte| (0..8).fold(0, |b, i| b << 1 | u8::from(byte.get(i) == Some(&true))))
        .collect()
}

/// `data` as runs to copy, ended by 128.
pub(crate) fn run_length(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for run in data.chunks(128) {
        out.push((run.len() - 1) as u8);
        out.extend_from_slice(run);
    }
    out.push(128);
    out
}

/// `zlib` data decompressed, for a seed written from data compressed
/// beforehand.
pub(crate) fn inflated(zlib: &[u8]) -> Vec<u8> {
    let flate = pdf_filters::Filter::Flate {
        predictor: pdf_filters::Predictor::None,
    };
    pdf_filters::decode(zlib, &[flate], usize::MAX).unwrap_or_else(|e| e.data)
}

/// A file of these objects, as [`file`] writes it with a table, encrypted
/// as `settings` say, by pdf-crypt's writer: each string, and each
/// stream's data, of each object but a signature's `/Contents`, under an
/// encryption dictionary that's the next object, with the trailer's `/ID`
/// the settings' and `trailer`'s entries.
pub(crate) fn encrypted(
    settings: &pdf_crypt::writer::Settings,
    objects: &[(u32, Body)],
    trailer: Dict,
) -> Vec<u8> {
    use pdf_crypt::writer;
    let values = writer::dictionary(settings);
    let handler = pdf_crypt::Handler::open(&values, settings.id, &[settings.user])
        .expect("the writer's opens");
    fn strings(o: &mut Object, encrypt: &mut dyn FnMut(&[u8]) -> Vec<u8>) {
        match &mut o.value {
            Value::String(Str::Bytes(bytes), _) => *bytes = encrypt(bytes),
            Value::Array(a) => a.iter_mut().for_each(|o| strings(o, encrypt)),
            Value::Dict(d) | Value::Stream(crate::object::Stream { dict: d, .. }) => {
                // A signature's /Contents, as the reader takes one.
                let typed = (d.get(b"Type"))
                    .is_none_or(|t| matches!(t.as_name(), Some(b"Sig" | b"DocTimeStamp")));
                let signature = d.has(b"ByteRange") && typed;
                for entry in d.entries_mut() {
                    if !(signature && entry.key == b"Contents") {
                        strings(&mut entry.value, encrypt);
                    }
                }
            }
            _ => {}
        }
    }
    let mut written = Vec::new();
    for (n, body) in objects {
        let id = (*n, 0);
        let iv = [*n as u8; 16];
        let mut encrypt = |bytes: &[u8]| {
            writer::encrypt_string(&handler, id, bytes, iv).expect("the writer's own method")
        };
        written.push((
            *n,
            match body {
                Body::Object(object) => {
                    let mut object = object.clone();
                    strings(&mut object, &mut encrypt);
                    Body::Object(object)
                }
                Body::Stream(dict, data) => {
                    // Metadata under /EncryptMetadata false isn't encrypted.
                    let metadata = dict.get(b"Type").and_then(Object::as_name) == Some(b"Metadata");
                    let mut dict: Object = dict.clone().into();
                    strings(&mut dict, &mut encrypt);
                    let Value::Dict(dict) = dict.value else {
                        unreachable!()
                    };
                    let data = match metadata && !handler.encrypts_metadata() {
                        true => data.clone(),
                        false => writer::encrypt_stream(&handler, id, data, None, iv)
                            .expect("the writer's own method"),
                    };
                    Body::Stream(dict, data)
                }
            },
        ));
    }
    let own = objects.iter().map(|(n, _)| *n).max().unwrap_or(0) + 1;
    let s = |bytes: &[u8]| Object::new(Value::String(Str::Bytes(bytes.to_vec()), None));
    let value = |bytes: &Option<Vec<u8>>| s(bytes.as_deref().expect("the writer's own value"));
    let mut dict = crate::dict! {
        "Filter" => "Standard",
        "V" => values.v,
        "R" => values.r,
        "O" => value(&values.o),
        "U" => value(&values.u),
        "P" => values.p.expect("the writer's own value"),
    };
    if let Some(length) = values.length {
        dict.set("Length", length);
    }
    if values.v >= 5 {
        dict.set("OE", value(&values.oe));
        dict.set("UE", value(&values.ue));
        dict.set("Perms", value(&values.perms));
    }
    if let Some(encrypt_metadata) = values.encrypt_metadata {
        dict.set("EncryptMetadata", encrypt_metadata);
    }
    if let Some(filter) = values.crypt_filters.first() {
        let method =
            String::from_utf8_lossy(filter.method.as_deref().unwrap_or_default()).into_owned();
        let std_cf = crate::dict! { "CFM" => method.as_str(), "AuthEvent" => "DocOpen" };
        dict.set("CF", crate::dict! { "StdCF" => std_cf });
        dict.set("StmF", "StdCF");
        dict.set("StrF", "StdCF");
    }
    written.push((own, Body::Object(dict.into())));
    let mut trailer = trailer;
    trailer.set("Encrypt", Object::reference(own));
    trailer.set("ID", vec![s(settings.id), s(settings.id)]);
    file("1.7", &written, trailer, Xref::Table)
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
        Value::String(Str::Bytes(bytes) | Str::Encrypted(bytes), _) => string(out, bytes),
        Value::String(Str::Text(text), _) => {
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

#[cfg(test)]
mod tests {
    #[test]
    fn zlib_data_in_stored_blocks_decompresses() {
        let flate = [pdf_filters::Filter::Flate {
            predictor: pdf_filters::Predictor::None,
        }];
        for data in [&b""[..], b"stored", &[7; 200_000]] {
            let decoded = pdf_filters::decode(&super::zlib(data), &flate, 1 << 20);
            assert_eq!(decoded.as_deref(), Ok(data));
        }
    }
}
