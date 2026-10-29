//! qpdf, the tests' oracle, run as a process: decoding what the bytes
//! reader is made to ask of it (`Decode`), a compressed cross-reference
//! stream's rows, an object stream's objects, and in an encrypted file the
//! strings' plaintext, to set beside the reader's own decoding.
//! A run decodes every stream it's asked for, through its JSON
//! (`--json-stream-data=inline --decode-level=generalized`), decrypted when
//! the file is encrypted. [`Qpdf::read`] reads a whole file through qpdf
//! alone, a second reader to set beside the bytes reader. The JSON is read
//! leniently: qpdf copies a real such as `+.5` into it as the file wrote
//! it, which isn't JSON.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use pdf_objects::object::{Dict, Object, ObjectId, Str, Value};
use pdf_objects::version::Version;

use crate::bytes::{Decode, Written};
use crate::json::{self, Json};
use crate::source::{Document, Held, Trailer, Unread, Warning};

/// How many objects one run is asked for by number: past it, a command line
/// grows long.
const PER_RUN: usize = 4096;

/// qpdf, to decode one file's streams.
#[derive(Debug, Clone)]
pub struct Qpdf {
    program: PathBuf,
    file: PathBuf,
    password: Option<String>,
    timeout: Duration,
    /// The `--decode-level` its streams are decoded at.
    level: &'static str,
}

impl Qpdf {
    /// qpdf for `file`: the program `PDF_LINT_QPDF` names, else `qpdf`
    /// on the `PATH`, given a minute a run.
    pub fn new(file: impl Into<PathBuf>) -> Qpdf {
        let program = std::env::var_os("PDF_LINT_QPDF").unwrap_or_else(|| "qpdf".into());
        Qpdf {
            program: program.into(),
            file: file.into(),
            password: None,
            timeout: Duration::from_secs(60),
            level: "generalized",
        }
    }

    /// Decoding at `--decode-level=specialized`, RunLength too, as the
    /// tests do to set its decoding beside the reader's.
    #[cfg(test)]
    pub(crate) fn specialized(self) -> Qpdf {
        Qpdf {
            level: "specialized",
            ..self
        }
    }

    /// Not decoding at all (`--decode-level=none`): a stream's data as
    /// qpdf reads it from the file, for the tests to see which it read.
    #[cfg(test)]
    pub(crate) fn undecoding(self) -> Qpdf {
        Qpdf {
            level: "none",
            ..self
        }
    }

    /// Another program than the default.
    pub fn program(self, program: impl Into<PathBuf>) -> Qpdf {
        Qpdf {
            program: program.into(),
            ..self
        }
    }

    /// The file's password, user or owner, for a file whose user password
    /// isn't empty.
    pub fn password(self, password: impl Into<String>) -> Qpdf {
        Qpdf {
            password: Some(password.into()),
            ..self
        }
    }

    /// How long a run may take before it's stopped.
    pub fn timeout(self, timeout: Duration) -> Qpdf {
        Qpdf { timeout, ..self }
    }

    /// Whether the file is encrypted and won't open with the password
    /// given, none or a wrong one, as qpdf's `--requires-password` says.
    /// Nothing then decodes, and a reader would find the objects in its
    /// object streams missing.
    pub fn locked(&self) -> Result<bool, String> {
        let mut args = Vec::new();
        if let Some(password) = &self.password {
            args.push(format!("--password={password}"));
        }
        args.push("--requires-password".to_string());
        args.push(self.file.display().to_string());
        let ran = self.exec(&args)?;
        match ran.code {
            Some(0) => Ok(true),
            Some(2 | 3) => Ok(false),
            code => Err(exited(&self.program, code, &ran.stderr)),
        }
    }

    /// qpdf's version, as the first line of its `--version` says it.
    pub fn version(&self) -> Result<String, String> {
        let (out, _) = self.run(&["--version".to_string()])?;
        let text = String::from_utf8_lossy(&out);
        Ok(text.lines().next().unwrap_or_default().to_string())
    }

    /// The file as qpdf reads it, through its JSON
    /// (`--json-stream-data=none`): every object it lists, as written but
    /// for what qpdf doesn't keep, which is where anything was written, the
    /// order of a dictionary's keys and any key written twice, a name's
    /// escapes, and which encoding a text string had ([`Str::Text`]); its
    /// strings decrypted; and its warnings. Where qpdf reconstructed the
    /// cross-references, the trailer says so. The error is why qpdf read
    /// nothing.
    pub fn read(&self) -> Result<Document, String> {
        let (out, err) = self.run(&self.json_args("none", &[]))?;
        let json = json::parse(&out).map_err(|why| format!("qpdf's JSON doesn't read: {why}"))?;
        let qpdf = json.get("qpdf");
        let header = (qpdf.and_then(|q| q.at(0)))
            .and_then(|h| h.get("pdfversion"))
            .and_then(Json::as_str)
            .and_then(|v| Version::parse(v.as_bytes()));
        let Some(Json::Object(members)) = qpdf.and_then(|q| q.at(1)) else {
            return Err("qpdf's JSON has no objects".to_string());
        };
        let mut objects = BTreeMap::new();
        let mut trailer = None;
        for (key, value) in members {
            if key == "trailer" {
                trailer = Some(indirect(value)?);
            } else if let Some(id) = key.strip_prefix("obj:").and_then(reference) {
                let object = indirect(value).map(|object| (object, None));
                objects.insert(id, object.map_err(Unread::Other));
            } else {
                return Err(format!("qpdf's JSON has {key:?} among its objects"));
            }
        }
        let trailer = trailer.ok_or("qpdf's JSON has no trailer")?;
        let warnings = warnings(&err, &self.file);
        let trailer = trailer_of(trailer, &objects, &warnings);
        let mut doc = Document::read(objects, trailer, None);
        doc.header = header;
        doc.warnings = warnings;
        doc.size = (std::fs::metadata(&self.file).ok()).and_then(|m| usize::try_from(m.len()).ok());
        Ok(doc)
    }

    /// `file`, the file's bytes, read by the bytes reader with qpdf
    /// decoding what the reader can't: the object streams and strings of an
    /// encrypted file it doesn't open.
    pub fn decoding(&self, file: &[u8]) -> Document {
        crate::bytes::read_with(file, self)
    }

    /// The arguments for qpdf's JSON of the objects `ids`, all of them when
    /// there are none, its streams' data as `data` says.
    fn json_args(&self, data: &str, ids: &[ObjectId]) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        if let Some(password) = &self.password {
            args.push(format!("--password={password}"));
        }
        args.extend(
            [
                "--json=2",
                "--json-key=qpdf",
                &format!("--json-stream-data={data}"),
            ]
            .map(String::from),
        );
        args.push(format!("--decode-level={}", self.level));
        args.extend(ids.iter().map(|(n, g)| format!("--json-object={n},{g}")));
        args.push(self.file.display().to_string());
        args
    }

    /// The objects of qpdf's JSON for `ids`, in runs of [`PER_RUN`], each
    /// given to `each`, or why a run gave none, given for each of its ids.
    fn objects_of(
        &self,
        data: &str,
        ids: &[ObjectId],
        mut each: impl FnMut(ObjectId, Result<&Json, String>),
    ) {
        for ids in ids.chunks(PER_RUN) {
            let json = (self.run(&self.json_args(data, ids))).and_then(|(out, _)| {
                json::parse(&out).map_err(|why| format!("qpdf's JSON doesn't read: {why}"))
            });
            let objects = json.as_ref().map_err(String::clone).and_then(|json| {
                (json.get("qpdf").and_then(|q| q.at(1)))
                    .ok_or_else(|| "qpdf's JSON has no objects".to_string())
            });
            for &(n, g) in ids {
                let object = objects.clone().and_then(|objects| {
                    (objects.get(&format!("obj:{n} {g} R")))
                        .ok_or_else(|| format!("qpdf gives no {n} {g} R"))
                });
                each((n, g), object);
            }
        }
    }

    /// qpdf's standard output and error, run with `args`: once it exits,
    /// with 0 or with 3, which is success with warnings, and within the
    /// timeout.
    fn run(&self, args: &[String]) -> Result<(Vec<u8>, Vec<u8>), String> {
        let ran = self.exec(args)?;
        match ran.code {
            Some(0 | 3) => Ok((ran.stdout, ran.stderr)),
            code => Err(exited(&self.program, code, &ran.stderr)),
        }
    }

    /// qpdf run with `args`, once it exits within the timeout.
    fn exec(&self, args: &[String]) -> Result<Ran, String> {
        let program = self.program.display();
        let mut child = Command::new(&self.program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => format!("{program} isn't found"),
                _ => format!("{program}: {e}"),
            })?;
        let read = |mut pipe: Box<dyn Read + Send>| {
            thread::spawn(move || {
                let mut out = Vec::new();
                let _ = pipe.read_to_end(&mut out);
                out
            })
        };
        let stdout = read(Box::new(child.stdout.take().expect("piped")));
        let stderr = read(Box::new(child.stderr.take().expect("piped")));
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if start.elapsed() > self.timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "{program} ran past {:?}, and was stopped",
                        self.timeout
                    ));
                }
                Ok(None) => thread::sleep(Duration::from_millis(5)),
                Err(e) => return Err(format!("{program}: {e}")),
            }
        };
        Ok(Ran {
            code: status.code(),
            stdout: stdout.join().unwrap_or_default(),
            stderr: stderr.join().unwrap_or_default(),
        })
    }
}

/// How a run of qpdf ended: its exit code, none for a signal, and what it
/// wrote to standard output and error.
struct Ran {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// That `program` exited with `code`, and the last line of what it said,
/// `stderr`.
fn exited(program: &Path, code: Option<i32>, stderr: &[u8]) -> String {
    let said = String::from_utf8_lossy(stderr);
    let said = said.lines().last().unwrap_or("").trim();
    let code = code.map_or("a signal".to_string(), |c| c.to_string());
    format!("{} exited with {code}: {said}", program.display())
}

impl Decode for Qpdf {
    fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
        let mut decoded = BTreeMap::new();
        self.objects_of("inline", ids, |id, object| {
            decoded.insert(
                id,
                object.and_then(|o| stream_data(o, id, self.level != "none")),
            );
        });
        decoded
    }

    fn plaintext(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Object, String>> {
        let mut plain = BTreeMap::new();
        self.objects_of("none", ids, |id, object| {
            plain.insert(id, object.and_then(indirect));
        });
        plain
    }

    /// Through a file of its own, holding the stream alone, as object 3: a
    /// new file in the temp folder, removed after.
    fn decode_written(&self, stream: Written) -> Result<Vec<u8>, String> {
        // tempfile's error says the path it tried.
        let mut file = tempfile::Builder::new()
            .prefix("pdf-lint-")
            .suffix(".pdf")
            .tempfile()
            .map_err(|e| e.to_string())?;
        let path = file.path().to_path_buf();
        (file.as_file_mut().write_all(&alone(stream)))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let qpdf = Qpdf {
            file: path,
            password: None,
            ..self.clone()
        };
        let decoded = qpdf.decode(&[(3, 0)]).remove(&(3, 0));
        drop(file);
        decoded.unwrap_or_else(|| Err("qpdf gives nothing".to_string()))
    }
}

/// A file of `stream` alone, as object 3, beside a catalog and an empty page
/// tree, which qpdf reads a file only with.
fn alone(stream: Written) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for body in [
        &b"<< /Type /Catalog /Pages 2 0 R >>"[..],
        b"<< /Type /Pages /Kids [] /Count 0 >>",
    ] {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    offsets.push(out.len());
    out.extend_from_slice(format!("3 0 obj\n<< /Length {} /Filter ", stream.data.len()).as_bytes());
    out.extend_from_slice(stream.filter);
    if let Some(parms) = stream.parms {
        out.extend_from_slice(b" /DecodeParms ");
        out.extend_from_slice(parms);
    }
    out.extend_from_slice(b" >>\nstream\n");
    out.extend_from_slice(stream.data);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// The decoded data of `object`, stream `id` in qpdf's JSON.
fn stream_data(object: &Json, (n, g): ObjectId, decoding: bool) -> Result<Vec<u8>, String> {
    let stream = object
        .get("stream")
        .ok_or_else(|| format!("{n} {g} R isn't a stream"))?;
    // qpdf leaves out the filters it decoded: any left, it couldn't, when
    // it was asked to.
    if let Some(filter) = stream.get("dict").and_then(|d| d.get("/Filter")) {
        if decoding {
            return Err(format!("qpdf doesn't decode its filters ({filter:?})"));
        }
    }
    let data = (stream.get("data").and_then(Json::as_str))
        .ok_or_else(|| format!("qpdf gives no data for {n} {g} R"))?;
    json::base64(data)
}

/// An object of qpdf's JSON: `{"value": …}`, or a stream's
/// `{"stream": {"dict": …}}`.
fn indirect(json: &Json) -> Result<Object, String> {
    if let Some(value) = json.get("value") {
        return object(value);
    }
    let dict = (json.get("stream").and_then(|s| s.get("dict")))
        .ok_or("an object of qpdf's JSON that's neither a value nor a stream")?;
    match object(dict)?.into_value() {
        Value::Dict(dict) => Ok(Object::stream(dict)),
        _ => Err("a stream of qpdf's JSON whose dictionary isn't one".to_string()),
    }
}

/// A value of qpdf's JSON.
fn object(json: &Json) -> Result<Object, String> {
    let value = match json {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) if n.contains(['.', 'e', 'E']) => {
            Value::Real(n.parse().map_err(|_| format!("{n} isn't a number"))?)
        }
        Json::Number(n) => Value::Integer(n.parse().map_err(|_| format!("{n} isn't an integer"))?),
        Json::String(s) => {
            if let Some(hex) = s.strip_prefix("b:") {
                Value::String(Str::Bytes(unhex(hex)?), None)
            } else if let Some(text) = s.strip_prefix("u:") {
                Value::String(Str::Text(text.to_string()), None)
            } else if let Some(id) = reference(s) {
                Value::Ref(id)
            } else {
                Value::Name(name(s)?)
            }
        }
        Json::Array(elements) => {
            Value::Array(elements.iter().map(object).collect::<Result<_, _>>()?)
        }
        Json::Object(members) => {
            let mut dict = Dict::new();
            for (key, value) in members {
                dict.push(name(key)?, object(value)?);
            }
            Value::Dict(dict)
        }
    };
    Ok(Object::new(value))
}

/// A name of qpdf's JSON: `/Name`, its bytes UTF-8, or `n:/Name` with
/// `#xx` escapes, for one whose bytes aren't.
fn name(s: &str) -> Result<Vec<u8>, String> {
    if let Some(name) = s.strip_prefix('/') {
        return Ok(name.as_bytes().to_vec());
    }
    let escaped = (s.strip_prefix("n:/")).ok_or_else(|| format!("{s:?} isn't a name"))?;
    let mut out = Vec::new();
    let mut bytes = escaped.bytes();
    while let Some(b) = bytes.next() {
        if b == b'#' {
            let pair = [bytes.next(), bytes.next()];
            let hex = pair
                .iter()
                .flatten()
                .map(|&b| b as char)
                .collect::<String>();
            let byte = (u8::from_str_radix(&hex, 16).ok().filter(|_| hex.len() == 2))
                .ok_or_else(|| format!("{s:?} has an escape that isn't one"))?;
            out.push(byte);
        } else {
            out.push(b);
        }
    }
    Ok(out)
}

/// A reference of qpdf's JSON, `n g R`.
fn reference(s: &str) -> Option<ObjectId> {
    let mut words = s.split(' ');
    let id = (words.next()?.parse().ok()?, words.next()?.parse().ok()?);
    (words.next() == Some("R") && words.next().is_none()).then_some(id)
}

/// Hex digits' bytes.
fn unhex(hex: &str) -> Result<Vec<u8>, String> {
    let digits = hex.as_bytes();
    if digits.len() % 2 == 1 {
        return Err(format!("b:{hex} has an odd digit"));
    }
    (digits.chunks(2))
        .map(|pair| {
            (std::str::from_utf8(pair).ok())
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| format!("b:{hex} isn't hex"))
        })
        .collect()
}

/// qpdf's trailer, as the reader gives it: a cross-reference stream, when
/// one of the objects is the stream whose dictionary it is; not as the file
/// writes it, when qpdf reconstructed the cross-references.
fn trailer_of(
    trailer: Object,
    objects: &BTreeMap<ObjectId, Held>,
    warnings: &[Warning],
) -> Trailer {
    let unwritten = (warnings.iter())
        .find(|w| w.message.contains("reconstruct"))
        .map(|w| format!("qpdf read the file otherwise: {}", w.message));
    let stream = (objects.iter()).find_map(|(&id, object)| {
        let s = object.as_ref().ok()?.0.as_stream()?;
        let xref = s.dict.get(b"Type").and_then(Object::as_name) == Some(b"XRef");
        (xref && Some(&s.dict) == trailer.as_dict()).then(|| (id, s.clone()))
    });
    match stream {
        Some((id, s)) => Trailer {
            stream: Some(id),
            object: Object::from(s),
            unwritten,
        },
        None => Trailer {
            stream: None,
            object: trailer,
            unwritten,
        },
    }
}

/// qpdf's warnings, a line each on its standard error: `WARNING: file
/// (object 5 0, offset 408): message`, the file a name such as `file object
/// stream 12` for an object stream's data, and the parenthesis there or
/// not.
fn warnings(stderr: &[u8], file: &Path) -> Vec<Warning> {
    let file = file.display().to_string();
    (String::from_utf8_lossy(stderr).lines())
        .filter_map(|line| line.strip_prefix("WARNING: "))
        .map(|line| {
            let line = line.strip_prefix(file.as_str()).unwrap_or(line);
            let (context, message) = match line.split_once("): ") {
                Some((context, message)) => (context, message),
                None => ("", line.trim_start_matches(':').trim_start()),
            };
            let object = (context.rfind("object "))
                .and_then(|at| reference(&format!("{} R", context[at + 7..].split(',').next()?)));
            let offset = (context.rfind("offset "))
                .map(|at| context[at + 7..].trim_end_matches(')'))
                .map(|offset| format!(" (offset {offset})"))
                .unwrap_or_default();
            Warning {
                object,
                at: None,
                message: format!("{message}{offset}"),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use pdf_filters::Backend;
    use pdf_objects::dict;
    use pdf_objects::object::{Data, Form, Object, Str, Value};
    use pdf_objects::text;
    use tempfile::TempDir;

    use super::*;
    use crate::Password;
    use crate::bytes;
    use crate::fixture::{self, Body, Xref};
    use crate::source::Source;

    /// qpdf, which these tests need: `PDF_LINT_QPDF`, else `qpdf` on the
    /// `PATH`. They're ignored, so that `cargo test` needs no qpdf, and run
    /// by `just test-qpdf` against the pinned one; its absence then fails
    /// them, rather than passing them unrun.
    fn qpdf(file: &Path) -> Qpdf {
        let qpdf = Qpdf::new(file);
        if let Err(why) = qpdf.version() {
            panic!("these tests need qpdf ({why}): install it, or name it in PDF_LINT_QPDF");
        }
        qpdf
    }

    /// A scratch folder of the test's own, removed when it's dropped.
    fn scratch(test: &str) -> TempDir {
        tempfile::Builder::new()
            .prefix(&format!("pdf-lint-qpdf-{test}-"))
            .tempdir()
            .unwrap()
    }

    /// A clean file: a catalog, its page tree and page, and a text
    /// annotation with its appearance stream.
    fn clean() -> Vec<(u32, Body)> {
        let r = Object::reference;
        let form = dict! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(20), Object::from(20)],
            "Resources" => dict! { "ProcSet" => vec![Object::from("PDF")] },
        };
        vec![
            (1, dict! { "Type" => "Catalog", "Pages" => r(2) }.into()),
            (2, dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1 }.into()),
            (
                3,
                dict! {
                    "Type" => "Page",
                    "Parent" => r(2),
                    "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(612), Object::from(792)],
                    "Resources" => dict! {},
                    "Annots" => vec![r(4)],
                }
                .into(),
            ),
            (
                4,
                dict! {
                    "Type" => "Annot",
                    "Subtype" => "Text",
                    "Rect" => vec![Object::from(100), Object::from(100), Object::from(120), Object::from(120)],
                    "Contents" => Object::string("A note"),
                    "AP" => dict! { "N" => r(5) },
                }
                .into(),
            ),
            (5, Body::stream(form, b"0 0 m 20 20 l S")),
        ]
    }

    /// The files qpdf wrote from [`clean`], committed in `tests/files` so
    /// that what's read from them needs no qpdf, nor a run's random /ID:
    /// each name, and qpdf's arguments.
    const FIXTURES: [(&str, &[&str]); 8] = [
        ("qpdf-plain", &["--object-streams=disable"]),
        ("qpdf-compressed", &["--object-streams=generate"]),
        (
            "qpdf-rc4-40",
            &[
                "--object-streams=generate",
                "--allow-weak-crypto",
                "--encrypt",
                "",
                "owner",
                "40",
                "--",
            ],
        ),
        (
            "qpdf-rc4-128",
            &[
                "--object-streams=generate",
                "--allow-weak-crypto",
                "--encrypt",
                "",
                "owner",
                "128",
                "--use-aes=n",
                "--",
            ],
        ),
        (
            "qpdf-aes-128",
            &[
                "--object-streams=generate",
                "--encrypt",
                "",
                "owner",
                "128",
                "--use-aes=y",
                "--",
            ],
        ),
        (
            "qpdf-aes-256-r5",
            &[
                "--object-streams=generate",
                "--encrypt",
                "",
                "owner",
                "256",
                "--force-R5",
                "--",
            ],
        ),
        (
            "qpdf-aes-256",
            &[
                "--object-streams=generate",
                "--encrypt",
                "",
                "owner",
                "256",
                "--",
            ],
        ),
        (
            "qpdf-aes-256-unpacked",
            &[
                "--object-streams=disable",
                "--encrypt",
                "",
                "owner",
                "256",
                "--",
            ],
        ),
    ];

    /// A committed fixture's path, by name.
    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/files")
            .join(format!("{name}.pdf"))
    }

    /// The fixtures written anew by the qpdf `PDF_LINT_QPDF` names, else
    /// the PATH's, as `just fixtures` does: only when [`clean`] or the
    /// kinds change.
    #[test]
    #[ignore = "needs qpdf; writes the committed fixtures anew"]
    fn the_fixtures_are_written() {
        for (name, args) in FIXTURES {
            let (_dir, rewritten) = rewritten(name, args);
            std::fs::copy(rewritten, fixture(name)).unwrap();
        }
    }

    /// The clean file, written, then rewritten by qpdf with `args`; with
    /// `--object-streams=generate`, its dictionaries in object streams, and
    /// a cross-reference stream, compressed: its path, in the test's folder,
    /// which is removed when it's dropped.
    fn rewritten(test: &str, args: &[&str]) -> (TempDir, PathBuf) {
        let dir = scratch(test);
        let plain = dir.path().join("plain.pdf");
        let root = dict! { "Root" => Object::reference(1) };
        std::fs::write(&plain, fixture::file("1.7", &clean(), root, Xref::Table)).unwrap();
        let out = dir.path().join("compressed.pdf");
        let qpdf = qpdf(&plain);
        let mut command = Command::new(&qpdf.program);
        // An /ID from the content, not the clock, so that a run can't
        // draw one that begins FE FF, which qpdf reads as UTF-16 text;
        // qpdf can't for a file it encrypts.
        if !args.contains(&"--encrypt") {
            command.arg("--deterministic-id");
        }
        command.args(args).arg(&plain).arg(&out);
        let status = command.status().unwrap();
        assert!(matches!(status.code(), Some(0 | 3)), "qpdf {args:?} failed");
        (dir, out)
    }

    /// A decoder that mustn't be asked.
    struct Unasked;

    impl Decode for Unasked {
        fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
            panic!("asked to decode {ids:?}")
        }
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn a_compressed_file_reads_without_qpdf_as_through_it() {
        let path = fixture("qpdf-compressed");
        let data = std::fs::read(&path).unwrap();
        let doc = bytes::read_with(&data, &Unasked);
        assert_eq!(doc.trailer.unwritten, None);
        assert!(doc.trailer.stream.is_some(), "a cross-reference stream");
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        let mut in_streams = 0;
        assert_eq!(doc.unread().next(), None);
        for (id, object) in doc.objects() {
            let at = object.at.unwrap();
            if let Some(stream) = at.stream {
                in_streams += 1;
                let written = &doc.decoded[&stream][at.start..at.end];
                assert!(
                    written.starts_with(b"<<") && written.ends_with(b">>"),
                    "{id:?}"
                );
            }
        }
        assert!(
            in_streams >= 4,
            "the dictionaries are in object streams: {in_streams}"
        );

        // Through qpdf, asked for every stream, alike: the same streams'
        // data, and the same objects read from them.
        let through = bytes::read_through(&data, &qpdf(&path));
        assert!(through.warnings.is_empty(), "{:?}", through.warnings);
        assert_eq!(through.decoded, doc.decoded);
        assert!(through.objects().eq(doc.objects()));
        assert!(through.unread().eq(doc.unread()));
    }

    /// A decoder that mustn't be asked anything: neither to decode nor for
    /// plaintext.
    struct Unaskable;

    impl Decode for Unaskable {
        fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
            panic!("asked to decode {ids:?}")
        }

        fn plaintext(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Object, String>> {
            panic!("asked for the plaintext of {ids:?}")
        }
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn an_encrypted_files_object_streams_decrypt_by_the_reader_as_through_qpdf() {
        let kinds = [
            "qpdf-rc4-40",
            "qpdf-rc4-128",
            "qpdf-aes-128",
            "qpdf-aes-256-r5",
            "qpdf-aes-256",
        ];
        for kind in kinds {
            let path = fixture(kind);
            let data = std::fs::read(&path).unwrap();
            // The reader alone, by the empty user password and by the
            // owner's.
            let doc = bytes::read_with(&data, &Unaskable);
            assert_eq!(
                doc.encryption,
                crate::Encryption::Opened {
                    owner: false,
                    by: None
                },
                "{kind}"
            );
            assert!(doc.warnings.is_empty(), "{kind}: {:?}", doc.warnings);
            assert_eq!(doc.trailer.unwritten, None, "{kind}");
            let annot = (doc.objects().map(|(_, o)| o))
                .find(|o| {
                    o.as_dict().is_some_and(|d| {
                        d.get(b"Subtype").and_then(Object::as_name) == Some(b"Text")
                    })
                })
                .unwrap_or_else(|| panic!("{kind}: the annotation reads"));
            assert!(
                annot.at.unwrap().stream.is_some(),
                "{kind}: in an object stream"
            );
            let contents = annot.as_dict().unwrap().get(b"Contents").unwrap();
            assert_eq!(
                *contents.value(),
                Value::String(Str::Bytes(b"A note".to_vec()), Some(Form::Literal)),
                "{kind}"
            );
            let owned = bytes::read_opened(&data, &[Password("owner")]);
            let by = Some((0, pdf_crypt::Encoding::Utf8));
            assert_eq!(
                owned.encryption,
                crate::Encryption::Opened { owner: true, by },
                "{kind}"
            );
            assert_eq!(owned.decoded, doc.decoded, "{kind}");
            // Through qpdf, the object streams' data the same, byte for byte.
            let through = bytes::read_through(&data, &qpdf(&path));
            assert!(!through.decoded.is_empty(), "{kind}");
            assert_eq!(through.decoded, doc.decoded, "{kind}");
        }
    }

    /// A metadata stream under `/EncryptMetadata false` whose own crypt
    /// filter encrypts it, and an embedded file with a crypt filter of its
    /// own, each encrypted by one that isn't the one `/StmF` names, decrypt
    /// by the reader as through qpdf, byte for byte, by the user's password
    /// and the owner's, as does the file's other stream, by `/StmF`'s. But
    /// an embedded file without one of its own, under an `/EFF` that isn't
    /// `/StmF`, qpdf decrypts as `/StmF` says, as it reads `/EFF` only to
    /// tell what a writer should encrypt new embedded files by: under
    /// `/StmF /Identity`, as written; the reader by `/EFF`'s, which ISO
    /// 32000-2's Table 20 says decrypts it, to the plaintext it was written
    /// from. qpdf takes a stream's own crypt filter's name from parameters
    /// of `/Type /CryptFilterDecodeParms`, which the files have.
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn a_metadata_stream_and_an_embedded_file_decrypt_as_through_qpdf_but_where_it_ignores_eff() {
        use crate::bytes::streams::{ATTACHED, Attached, CONTENT, XMP, aes_128_and_rc4, attached};
        use pdf_crypt::writer::Kind;

        let own = |name: &str| {
            format!(
                " /Filter /Crypt /DecodeParms << /Type /CryptFilterDecodeParms /Name /{name} >>"
            )
        };
        let (rc4, std_cf) = (own("RC4CF"), own("StdCF"));
        // Each file, and whether qpdf hands its embedded file out as
        // written, where the reader decrypts it.
        let cases = [
            // /StmF and /EFF AES-128; the metadata stream and the embedded
            // file each RC4 by its own.
            (
                Attached {
                    metadata: (&rc4, "RC4CF"),
                    embedded: (&rc4, "RC4CF"),
                    ..aes_128_and_rc4("/EFF /StdCF")
                },
                false,
            ),
            // /StmF /Identity; the metadata stream RC4 by its own, and the
            // embedded file RC4 by /EFF's.
            (
                Attached {
                    streams: "Identity",
                    metadata: (&rc4, "RC4CF"),
                    embedded: ("", "RC4CF"),
                    ..aes_128_and_rc4("/EFF /RC4CF")
                },
                true,
            ),
            // /StmF /Identity; the metadata stream AES-256 by its own, and
            // the embedded file by /EFF's.
            (
                Attached {
                    kind: Kind::Aes256,
                    filters: &[("StdCF", "AESV3")],
                    streams: "Identity",
                    strings: "StdCF",
                    eff: "/EFF /StdCF",
                    metadata: (&std_cf, "StdCF"),
                    embedded: ("", "StdCF"),
                },
                true,
            ),
        ];
        let dir = scratch("crypt-filters");
        let ids = [(3, 0), (4, 0), (6, 0)];
        for (i, (how, ignored)) in cases.iter().enumerate() {
            let path = dir.path().join(format!("{i}.pdf"));
            let file = attached(how, &[]);
            std::fs::write(&path, &file).unwrap();
            for password in ["user", "owner"] {
                let what = format!("{i}, {password}");
                let options = bytes::Options::DEFAULT;
                let reading = bytes::Reading::open_with(&file[..], &[Password(password)], &options);
                let reading = reading.unwrap();
                let theirs = qpdf(&path).undecoding().password(password).decode(&ids);
                for (id, plain) in ids.into_iter().zip([XMP, ATTACHED, CONTENT]) {
                    let mine = reading.data(id).unwrap();
                    assert_eq!(mine, plain, "{what}: {id:?}");
                    let theirs = theirs[&id].as_deref();
                    if id == (4, 0) && *ignored {
                        let at = reading.record(id).unwrap().data.unwrap().at;
                        assert_eq!(theirs, Ok(&file[at.start..at.end]), "{what}: {id:?}");
                    } else {
                        assert_eq!(theirs, Ok(&mine[..]), "{what}: {id:?}");
                    }
                }
            }
        }
    }

    /// Whether `x`, read by `a`, and `y`, by `b`, are alike, references
    /// followed in step, each pair once: a dictionary's keys in any order, a
    /// string's form aside, and a stream's data decoded, its `/Length`
    /// aside. Else where they differ.
    fn alike(
        (a, x): (&bytes::Reading<Vec<u8>>, &Object),
        (b, y): (&bytes::Reading<Vec<u8>>, &Object),
        followed: &mut BTreeMap<ObjectId, ObjectId>,
    ) -> Result<(), String> {
        let differ = || Err(format!("{:?} and {:?}", x.value(), y.value()));
        let dicts = |x: &Dict, y: &Dict, followed: &mut BTreeMap<_, _>| {
            let keys = |d: &Dict| {
                (d.iter().map(|(k, _)| k.to_vec()))
                    .filter(|k| k != b"Length")
                    .collect::<std::collections::BTreeSet<_>>()
            };
            if keys(x) != keys(y) {
                return Err(format!("keys {:?} and {:?}", keys(x), keys(y)));
            }
            keys(x).iter().try_for_each(|k| {
                let (x, y) = (x.get(k).unwrap(), y.get(k).unwrap());
                alike((a, x), (b, y), followed)
            })
        };
        match (x.value(), y.value()) {
            (Value::String(s, _), Value::String(t, _)) if s == t => Ok(()),
            (Value::Array(s), Value::Array(t)) if s.len() == t.len() => {
                (s.iter().zip(t)).try_for_each(|(x, y)| alike((a, x), (b, y), followed))
            }
            (Value::Dict(s), Value::Dict(t)) => dicts(s, t, followed),
            (Value::Ref(i), Value::Ref(j)) => match followed.get(i) {
                Some(seen) if seen == j => Ok(()),
                Some(_) => differ(),
                None => {
                    followed.insert(*i, *j);
                    let (x, y) = (a.get(*i).unwrap(), b.get(*j).unwrap());
                    match (x.value(), y.value()) {
                        (Value::Stream(s), Value::Stream(t)) => {
                            dicts(&s.dict, &t.dict, followed)?;
                            let (s, t) = (a.decoded(*i).unwrap(), b.decoded(*j).unwrap());
                            (s == t).then_some(()).ok_or(format!("{i:?}'s data"))
                        }
                        _ => alike((a, x), (b, y), followed),
                    }
                }
            },
            (x, y) if x == y => Ok(()),
            _ => differ(),
        }
    }

    /// A new file's encryption, as pdf-crypt creates it (revision 6,
    /// AES-256) and the test writer writes it, opens in qpdf by the
    /// password's SASLprep form, which qpdf takes as given, preparing no
    /// password (Ⅸ, set as `IX`, opens only by `IX`): its `--check` finds
    /// nothing wrong; `--show-encryption` says revision 6, AESv3 for
    /// strings, streams and embedded files, `/P -4` with every permission,
    /// the password the owner's and the user's, and the key drawn first;
    /// and `--decrypt` writes a file the reader reads, from the trailer's
    /// `/Root` and `/Info`, as it reads this one by the password as typed.
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn a_new_files_encryption_opens_in_qpdf_and_decrypts_as_the_reader_reads_it() {
        use chacha20::ChaCha20Rng;
        use pdf_crypt::rand_core::{Rng, SeedableRng};

        let dir = scratch("created");
        let mut objects = clean();
        objects.push((6, dict! { "Title" => Object::string("A title") }.into()));
        let cases = [
            ("user", "user"),
            ("p\u{e4}ssw\u{f6}rd", "p\u{e4}ssw\u{f6}rd"),
            ("\u{2168}", "IX"),
        ];
        for (i, (typed, prepared)) in cases.into_iter().enumerate() {
            let password = Password(typed).prepared().unwrap();
            assert_eq!(password.as_bytes(), prepared.as_bytes(), "{typed}");
            let mut rng = ChaCha20Rng::seed_from_u64(6);
            let trailer = dict! { "Root" => Object::reference(1), "Info" => Object::reference(6) };
            let file = fixture::created(&password, true, &objects, trailer, &mut rng);
            let path = dir.path().join(format!("{i}.pdf"));
            std::fs::write(&path, &file).unwrap();
            let qpdf = qpdf(&path).password(prepared);
            let run = |args: &[&Path]| {
                let mut all = vec![format!("--password={prepared}")];
                all.extend(args.iter().map(|a| a.display().to_string()));
                qpdf.exec(&all).unwrap()
            };
            let check = run(&[Path::new("--check"), &path]);
            let said = String::from_utf8_lossy(&check.stderr).into_owned();
            assert_eq!((check.code, said.as_str()), (Some(0), ""), "{typed}");
            let shown = run(&[
                Path::new("--show-encryption"),
                Path::new("--show-encryption-key"),
                &path,
            ]);
            let shown = String::from_utf8_lossy(&shown.stdout).into_owned();
            let mut key = [0; 32];
            ChaCha20Rng::seed_from_u64(6).fill_bytes(&mut key);
            let key: String = key.iter().map(|b| format!("{b:02x}")).collect();
            let lines = [
                "R = 6",
                "P = -4",
                &format!("Encryption key = {key}"),
                "Supplied password is owner password",
                "Supplied password is user password",
                "modify anything: allowed",
                "stream encryption method: AESv3",
                "string encryption method: AESv3",
                "file encryption method: AESv3",
            ];
            for line in lines {
                assert!(shown.lines().any(|l| l == line), "{typed}: {line}\n{shown}");
            }
            assert_eq!(shown.matches(": allowed").count(), 9, "{typed}: {shown}");
            if typed != prepared {
                let as_typed = Qpdf::new(&path).password(typed);
                assert_eq!(as_typed.locked(), Ok(true), "{typed}: qpdf prepares none");
            }
            let decrypted = dir.path().join(format!("{i}-decrypted.pdf"));
            let wrote = run(&[
                Path::new("--decrypt"),
                Path::new("--compress-streams=n"),
                Path::new("--decode-level=none"),
                &path,
                &decrypted,
            ]);
            assert_eq!(wrote.code, Some(0), "{typed}");
            let options = bytes::Options::DEFAULT;
            let ours = bytes::Reading::open_with(file, &[Password(typed)], &options).unwrap();
            let opened = crate::Encryption::Opened {
                owner: true,
                by: Some((0, password_by(typed, prepared))),
            };
            assert_eq!(*ours.encryption(), opened, "{typed}");
            let theirs = std::fs::read(&decrypted).unwrap();
            let theirs = bytes::Reading::open(theirs, &options).unwrap();
            assert_eq!(*theirs.encryption(), crate::Encryption::None, "{typed}");
            let mut followed = BTreeMap::new();
            for key in [&b"Root"[..], b"Info"] {
                let x = ours.trailer().dict().get(key).unwrap();
                let y = theirs.trailer().dict().get(key).unwrap();
                let alike = alike((&ours, x), (&theirs, y), &mut followed);
                assert_eq!(alike, Ok(()), "{typed}");
            }
            assert_eq!(followed.len(), 6, "{typed}: every object followed");
        }
    }

    /// The encoding that opens a file set with `prepared` by `typed`: its
    /// SASLprep form where that's another text, else as typed.
    fn password_by(typed: &str, prepared: &str) -> pdf_crypt::Encoding {
        match typed == prepared {
            true => pdf_crypt::Encoding::Utf8,
            false => pdf_crypt::Encoding::Saslprep,
        }
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn a_qpdf_that_cant_run_says_why_for_every_stream() {
        let decoded = Qpdf::new("x.pdf")
            .program("/nonexistent/qpdf")
            .decode(&[(1, 0), (2, 0)]);
        assert_eq!(decoded.len(), 2);
        for why in decoded.values() {
            assert!(
                why.as_ref().unwrap_err().contains("/nonexistent/qpdf"),
                "{why:?}"
            );
        }
        let qpdf = qpdf(Path::new("/nonexistent/file.pdf"));
        let why = qpdf.decode(&[(1, 0)]).remove(&(1, 0)).unwrap().unwrap_err();
        assert!(why.contains("exited with 2"), "{why}");
    }

    /// A stream qpdf decodes through a file of its own, made in the temp
    /// folder and removed after, as the comparisons over a folder of files
    /// have it decode one: hex digits to their bytes, and a Flate stream
    /// that isn't zlib qpdf's refusal.
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn a_stream_decodes_through_a_file_of_its_own() {
        let qpdf = qpdf(Path::new("x.pdf"));
        let hex = Written {
            filter: b"/ASCIIHexDecode",
            parms: None,
            data: b"3132 33>",
        };
        assert_eq!(qpdf.decode_written(hex), Ok(b"123".to_vec()));
        let flate = Written {
            filter: b"/FlateDecode",
            parms: None,
            data: b"not zlib",
        };
        let why = qpdf.decode_written(flate).unwrap_err();
        assert!(why.contains("qpdf doesn't decode"), "{why}");
    }

    #[cfg(unix)]
    #[test]
    fn a_qpdf_that_hangs_is_stopped() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("hangs");
        let program = dir.path().join("hang.sh");
        std::fs::write(&program, "#!/bin/sh\nsleep 10\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let qpdf = Qpdf::new("x.pdf")
            .program(&program)
            .timeout(Duration::from_millis(100));
        // Another test's process, forked before the script was closed, holds
        // it open until it runs its own program: then running the script is
        // refused (ETXTBSY) a moment.
        let (why, took) = loop {
            let start = Instant::now();
            let why = qpdf.decode(&[(1, 0)]).remove(&(1, 0)).unwrap().unwrap_err();
            if !why.contains("Text file busy") {
                break (why, start.elapsed());
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert!(why.contains("was stopped"), "{why}");
        assert!(took < Duration::from_secs(5));
    }

    #[test]
    fn qpdfs_json_reads_as_values() {
        let json = json::parse(
            br#"{"/Type": "/Catalog", "n:/Bin#ffx": [+.5, -0.25, 12, null, true],
                 "/Odd Name": "u:Caf\u00e9", "/Bytes": "b:00ff10", "/Pages": "2 0 R"}"#,
        )
        .unwrap();
        let object = object(&json).unwrap();
        let dict = object.as_dict().unwrap();
        assert_eq!(
            dict.get(b"Type").and_then(Object::as_name),
            Some(&b"Catalog"[..])
        );
        let numbers = dict
            .get(b"Bin\xFFx")
            .expect("a name with its escape decoded");
        assert_eq!(
            *numbers.value(),
            Value::Array(vec![
                Object::from(0.5),
                Object::from(-0.25),
                Object::from(12),
                Object::null(),
                Object::from(true),
            ])
        );
        let text = dict.get(b"Odd Name").unwrap().value();
        assert_eq!(text, &Value::String(Str::Text("Café".to_string()), None));
        let bytes = dict.get(b"Bytes").unwrap().value();
        assert_eq!(bytes, &Value::String(Str::Bytes(vec![0, 0xFF, 0x10]), None));
        assert_eq!(*dict.get(b"Pages").unwrap().value(), Value::Ref((2, 0)));
        let stream = indirect(&json::parse(br#"{"stream": {"dict": {"/Length": 5}}}"#).unwrap());
        let stream = stream.unwrap();
        assert!(
            matches!(stream.value(), Value::Stream(s) if s.dict.get(b"Length").and_then(Object::as_integer) == Some(5) && s.data == Data::Unplaced)
        );
        for bad in [
            &br#"{"value": "no prefix"}"#[..],
            br#"{"value": "2 0 obj"}"#,
            br#"{"value": "b:0g"}"#,
            br#"{"value": "b:0"}"#,
            br#"{"value": "n:/x#4"}"#,
            br#"{"value": 99999999999999999999}"#,
            br#"{"value": +-}"#,
            br#"{"neither": 1}"#,
            br#"{"stream": {"dict": 5}}"#,
        ] {
            let json = json::parse(bad).unwrap();
            assert!(indirect(&json).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn qpdfs_warnings_read_with_their_objects() {
        let stderr = b"WARNING: in.pdf (object 1 0, offset 19): dictionary has duplicated key /Dup; last occurrence overrides earlier ones\n\
WARNING: in.pdf object stream 12 (object 5 0, offset 34): expected dictionary key but found non-name object; inserting key /QPDFFake1\n\
WARNING: in.pdf: file is damaged\n\
WARNING: in.pdf (offset 1234): xref not found\n\
qpdf: operation succeeded with warnings\n";
        let warnings = warnings(stderr, Path::new("in.pdf"));
        let read: Vec<_> = (warnings.iter())
            .map(|w| (w.object, w.message.as_str()))
            .collect();
        assert_eq!(
            read,
            [
                (
                    Some((1, 0)),
                    "dictionary has duplicated key /Dup; last occurrence overrides earlier ones (offset 19)"
                ),
                (
                    Some((5, 0)),
                    "expected dictionary key but found non-name object; inserting key /QPDFFake1 (offset 34)"
                ),
                (None, "file is damaged"),
                (None, "xref not found (offset 1234)"),
            ]
        );
    }

    /// What reading through qpdf alone loses: where anything was written,
    /// any key written twice, which encoding a text string had, and whether
    /// a string was written literal or hex. `a` with these left out, as `b`
    /// has them, compared with `b`: the first difference, what kind it is
    /// and where, or none. `nothing` says whether a value of `a` is null,
    /// or a reference to no object, which qpdf reads as null.
    fn difference(
        a: &Object,
        b: &Object,
        at: &str,
        nothing: &dyn Fn(&Object) -> bool,
    ) -> Option<(String, String)> {
        let kind = |v: &Value| match v {
            Value::Null => "null",
            Value::Bool(_) => "a boolean",
            Value::Integer(_) => "an integer",
            Value::Real(_) => "a real",
            Value::Name(_) => "a name",
            Value::String(Str::Bytes(_), _) => "bytes",
            Value::String(Str::Text(_), _) => "text",
            Value::String(Str::Encrypted(_), _) => "ciphertext",
            Value::Array(_) => "an array",
            Value::Dict(_) => "a dictionary",
            Value::Stream(_) => "a stream",
            Value::Ref(_) => "a reference",
        };
        let differ = || {
            let kinds = format!("{} and {}", kind(a.value()), kind(b.value()));
            Some((kinds, format!("{at}: {:?} and {:?}", a.value(), b.value())))
        };
        let explained = |kind: &str| {
            Some((
                kind.to_string(),
                format!("{at}: {:?} and {:?}", a.value(), b.value()),
            ))
        };
        // Two dictionaries, or two streams' dictionaries.
        let dicts = |x: &Dict, y: &Dict| {
            let keys = |d: &Dict| {
                d.iter()
                    .map(|(k, _)| k.to_vec())
                    .collect::<std::collections::BTreeSet<_>>()
            };
            let name = |k: &Vec<u8>| format!("/{}", String::from_utf8_lossy(k));
            let (only_x, only_y): (Vec<_>, Vec<_>) = (
                keys(x).difference(&keys(y)).cloned().collect(),
                keys(y).difference(&keys(x)).cloned().collect(),
            );
            if only_y.is_empty()
                && !only_x.is_empty()
                && only_x.iter().all(|k| x.get(k).is_some_and(nothing))
            {
                return Some((
                    "keys qpdf leaves out, whose values are null or refer to no object".to_string(),
                    format!(
                        "{at}: {}",
                        only_x.iter().map(name).collect::<Vec<_>>().join(" ")
                    ),
                ));
            }
            if !only_x.is_empty() || !only_y.is_empty() {
                let names = |keys: &[Vec<u8>]| keys.iter().map(name).collect::<Vec<_>>().join(" ");
                let kind = format!(
                    "keys only the bytes give [{}], only qpdf [{}]",
                    names(&only_x),
                    names(&only_y)
                );
                return Some((kind, at.to_string()));
            }
            (x.last_entries()).find_map(|e| {
                let name = String::from_utf8_lossy(&e.key);
                difference(&e.value, y.get(&e.key)?, &format!("{at}/{name}"), nothing)
            })
        };
        match (a.value(), b.value()) {
            (Value::String(Str::Bytes(x), _), Value::String(Str::Text(y), _)) => {
                if &text::decode(x).text == y {
                    None
                } else if x.starts_with(&[0xFF, 0xFE]) {
                    explained(
                        "a string after FF FE, which qpdf reads as UTF-16LE, and the spec as PDFDocEncoding",
                    )
                } else {
                    differ()
                }
            }
            (Value::String(x, _), Value::String(y, _)) => (x != y).then(differ).flatten(),
            (Value::Ref(_), Value::Null) if nothing(a) => {
                explained("a reference to no object, which qpdf writes as null")
            }
            (Value::Integer(_), Value::Integer(_))
                if a.value() != b.value() && at.ends_with("/Length") =>
            {
                explained("a /Length qpdf recovered, which it writes for the one written")
            }
            (Value::Array(x), Value::Array(y)) => {
                if x.len() != y.len() {
                    let lengths = format!("{at}: {} and {} elements", x.len(), y.len());
                    return Some(("arrays of other lengths".to_string(), lengths));
                }
                (x.iter().zip(y).enumerate())
                    .find_map(|(i, (x, y))| difference(x, y, &format!("{at}[{i}]"), nothing))
            }
            (Value::Dict(x), Value::Dict(y)) => dicts(x, y),
            (Value::Stream(x), Value::Stream(y)) => dicts(&x.dict, &y.dict),
            (x, y) => (x != y).then(differ).flatten(),
        }
    }

    /// The differences between `a`, read from its bytes, and `b`, read
    /// through qpdf alone, each with what kind it is.
    fn differences(a: &Document, b: &Document) -> Vec<(String, String)> {
        let nothing = |o: &Object| match o.value() {
            Value::Null => true,
            Value::Ref(id) => a.get(*id).map_or(true, Object::is_null),
            _ => false,
        };
        let is_xref = |o: &Object| match o.value() {
            Value::Stream(s) => s.dict.get(b"Type").and_then(Object::as_name) == Some(b"XRef"),
            _ => false,
        };
        let mut found: Vec<(String, String)> = Vec::new();
        let mut push = |kind: &str, d: String| found.push((kind.to_string(), d));
        for id in a.ids() {
            let (n, g) = id;
            match (a.get(id), b.contains(id).then(|| b.get(id))) {
                (_, None) => push("only the bytes list it", format!("{n} {g} R")),
                (Ok(x), Some(Ok(y))) => {
                    if let Some((kind, d)) = difference(x, y, &format!("{n} {g} R"), &nothing) {
                        let kind = match is_xref(x) && d.contains("/ID[") {
                            true => "an older cross-reference stream's /ID, which isn't encrypted, and qpdf decrypts".to_string(),
                            false => format!("value: {kind}"),
                        };
                        push(&kind, d);
                    }
                }
                (Err(why), Some(Ok(y))) if y.is_null() => {
                    push("only qpdf reads it, as null", format!("{n} {g} R: {why}"))
                }
                (Err(why), Some(Ok(_))) => push("only qpdf reads it", format!("{n} {g} R: {why}")),
                (Ok(_), Some(Err(why))) => {
                    push("only the bytes read it", format!("{n} {g} R: {why}"))
                }
                (Err(_), Some(Err(_))) => {}
            }
        }
        for id in b.ids().into_iter().filter(|&id| !a.contains(id)) {
            let (n, g) = id;
            let kind = match b.get(id) {
                Ok(y) if is_xref(y) => "only qpdf lists it: a cross-reference stream",
                Ok(y) if y.is_null() => "only qpdf lists it, as null",
                _ => "only qpdf lists it",
            };
            push(kind, format!("{n} {g} R"));
        }
        if a.trailer.stream != b.trailer.stream {
            let streams = format!("{:?} and {:?}", a.trailer.stream, b.trailer.stream);
            push("trailer stream", streams);
        }
        let trailer = difference(&a.trailer.object, &b.trailer.object, "trailer", &nothing);
        if let Some((kind, d)) = trailer {
            push(&format!("trailer: {kind}"), d);
        }
        if a.header != b.header {
            push("header", format!("{:?} and {:?}", a.header, b.header));
        }
        found
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn a_file_reads_through_qpdf_alone_as_through_its_bytes() {
        for test in [
            "qpdf-plain",
            "qpdf-compressed",
            "qpdf-aes-256",
            "qpdf-aes-256-unpacked",
        ] {
            let path = fixture(test);
            let data = std::fs::read(&path).unwrap();
            let qpdf = qpdf(&path);
            let bytes = bytes::read_with(&data, &qpdf);
            let alone = qpdf.read().unwrap();
            assert_eq!(differences(&bytes, &alone), [], "{test}");
            assert!(alone.warnings.is_empty(), "{test}: {:?}", alone.warnings);
            assert_eq!(alone.trailer.unwritten, None, "{test}");
            assert!(
                alone.objects().all(|(_, o)| o.at.is_none()),
                "{test}: qpdf says nowhere"
            );
        }
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn what_qpdf_keeps_and_loses_reading_alone() {
        let dir = scratch("loses");
        let path = dir.path().join("odd.pdf");
        let catalog = b"<< /Type /Catalog /Pages 2 0 R /Odd#20Name 1 /Bin#FFx 2 /Dup 1 /Dup 2 /Utf16 <FEFF00410042> /Real +.5 >>";
        let pages = b"<< /Type /Pages /Kids [] /Count 0 >>";
        let file = fixture::assembled(
            "1.7",
            &[(1, &catalog[..]), (2, &pages[..])],
            "<< /Size 3 /Root 1 0 R >>",
        );
        std::fs::write(&path, &file).unwrap();
        let alone = qpdf(&path).read().unwrap();
        let catalog = alone.get((1, 0)).unwrap().as_dict().unwrap();
        assert!(catalog.has(b"Odd Name") && catalog.has(b"Bin\xFFx"));
        assert_eq!(
            catalog.entries().iter().filter(|e| e.key == b"Dup").count(),
            1
        );
        assert_eq!(catalog.get(b"Dup").and_then(Object::as_integer), Some(2));
        assert_eq!(
            *catalog.get(b"Utf16").unwrap().value(),
            Value::String(Str::Text("AB".to_string()), None),
            "text, its encoding lost"
        );
        assert_eq!(catalog.get(b"Real").and_then(Object::as_number), Some(0.5));
        assert!(
            (alone.warnings.iter())
                .any(|w| w.object == Some((1, 0)) && w.message.contains("duplicated key /Dup")),
            "{:?}",
            alone.warnings
        );
        let bytes = bytes::read(&file);
        assert_eq!(differences(&bytes, &alone), []);

        // A file qpdf can't read at all says why.
        let unreadable = dir.path().join("unreadable.pdf");
        std::fs::write(&unreadable, b"not a PDF").unwrap();
        let why = qpdf(&unreadable).read().unwrap_err();
        assert!(why.contains("exited with 2"), "{why}");

        // Nor what qpdf reconstructs is as the file writes it.
        let damaged = dir.path().join("damaged.pdf");
        let mut broken = fixture::file(
            "1.7",
            &clean(),
            dict! { "Root" => Object::reference(1) },
            Xref::Table,
        );
        let at = broken.windows(9).rposition(|w| w == b"startxref").unwrap();
        broken.truncate(at);
        broken.extend_from_slice(b"startxref\n999999\n%%EOF\n");
        std::fs::write(&damaged, &broken).unwrap();
        let alone = qpdf(&damaged).read().unwrap();
        assert!(alone.trailer.unwritten.is_some(), "{:?}", alone.warnings);
        assert!(alone.contains((1, 0)));
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn a_cross_reference_stream_whose_number_an_update_reuses_decodes_from_its_bytes() {
        let path = fixture("qpdf-compressed");
        let mut data = std::fs::read(&path).unwrap();
        let old = bytes::read(&data);
        let (x, _) = old.trailer.stream.expect("a cross-reference stream");
        let root = old
            .trailer
            .dict()
            .get(b"Root")
            .unwrap()
            .as_reference()
            .unwrap();
        let size = old
            .trailer
            .dict()
            .get(b"Size")
            .unwrap()
            .as_integer()
            .unwrap();
        let prev = {
            let at = data.windows(9).rposition(|w| w == b"startxref").unwrap();
            let rest = String::from_utf8_lossy(&data[at + 9..]).into_owned();
            rest.split_whitespace().next().unwrap().to_string()
        };
        // An update that gives the stream's number to another stream.
        let offset = data.len();
        data.extend_from_slice(
            format!("{x} 0 obj\n<< /Length 11 /Filter /ASCIIHexDecode >>\nstream\n3132333435>\nendstream\nendobj\n").as_bytes(),
        );
        let xref = data.len();
        data.extend_from_slice(
            format!(
                "xref\n0 1\n0000000000 65535 f \n{x} 1\n{offset:010} 00000 n \ntrailer\n<< /Size {size} /Root {} 0 R /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n",
                root.0
            )
            .as_bytes(),
        );
        let dir = scratch("reused");
        let updated = dir.path().join("updated.pdf");
        std::fs::write(&updated, &data).unwrap();
        let qpdf = qpdf(&updated);
        assert_eq!(
            qpdf.decode(&[(x, 0)]).remove(&(x, 0)).unwrap(),
            Ok(b"12345".to_vec()),
            "by its number, qpdf decodes the other stream"
        );
        let doc = bytes::read_with(&data, &qpdf);
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        assert_eq!(doc.trailer.unwritten, None);
        assert_eq!(differences(&doc, &qpdf.read().unwrap()), []);
    }

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn an_encrypted_files_strings_in_plain_view_decrypt_to_their_bytes_and_through_qpdf() {
        let path = fixture("qpdf-aes-256-unpacked");
        let data = std::fs::read(&path).unwrap();
        let contents = |doc: &Document| {
            let annot = (doc.objects().map(|(_, o)| o))
                .find(|o| {
                    o.as_dict().is_some_and(|d| {
                        d.get(b"Subtype").and_then(Object::as_name) == Some(b"Text")
                    })
                })
                .expect("the annotation reads");
            annot.as_dict().unwrap().get(b"Contents").unwrap().clone()
        };
        // Decrypted by the reader, its bytes; through qpdf, as qpdf's JSON
        // gives it, text, its encoding unsaid.
        let through = bytes::read_through(&data, &qpdf(&path));
        assert!(through.warnings.is_empty(), "{:?}", through.warnings);
        assert_eq!(
            *contents(&through).value(),
            Value::String(Str::Text("A note".to_string()), Some(Form::Hex))
        );
        let doc = bytes::read_opened(&data, &[]);
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        let opened = crate::Encryption::Opened {
            owner: false,
            by: None,
        };
        assert_eq!(doc.encryption, opened);
        let contents = contents(&doc);
        assert_eq!(
            *contents.value(),
            Value::String(Str::Bytes(b"A note".to_vec()), Some(Form::Hex)),
            "its bytes"
        );
        let at = contents.at.unwrap();
        assert_eq!(at.stream, None, "in plain view");
        assert!(
            !data[at.start..at.end].windows(6).any(|w| w == b"A note"),
            "placed on its ciphertext"
        );
        // A password that doesn't open it, though its user password is
        // empty, is all that's tried: it's locked, and the string stays
        // ciphertext, where it is.
        let doc = bytes::read_opened(&data, &[Password("wrong")]);
        assert_eq!(doc.encryption, crate::Encryption::Locked);
        let annot = (doc.objects().map(|(_, o)| o))
            .find(|o| {
                o.as_dict()
                    .is_some_and(|d| d.get(b"Contents").is_some_and(|c| c.at == Some(at)))
            })
            .expect("the annotation reads");
        let contents = annot.as_dict().unwrap().get(b"Contents").unwrap();
        assert!(matches!(
            *contents.value(),
            Value::String(Str::Encrypted(_), _)
        ));
    }

    /// Every PDF under `PDF_LINT_CORPUS` read from its bytes, qpdf
    /// decoding, and through qpdf alone, and the differences counted by what
    /// they're of, with examples.
    #[test]
    #[ignore = "needs PDF_LINT_CORPUS, a folder of PDF files, and qpdf"]
    fn a_corpus_reads_alike_through_qpdf_alone() {
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let files = corpus();
        /// Each kind's count, and its first examples.
        type Kinds = BTreeMap<String, (usize, Vec<String>)>;
        let add = |kinds: &mut Kinds, kind: String, example: String| {
            let (n, examples) = kinds.entry(kind).or_default();
            *n += 1;
            if examples.len() < 5 {
                examples.push(example);
            }
        };
        #[derive(Default)]
        struct Tally {
            objects: usize,
            files_differing: usize,
            differences: Kinds,
        }
        let tally = Mutex::new(Tally::default());
        let next = AtomicUsize::new(0);
        let start = Instant::now();
        thread::scope(|scope| {
            for _ in 0..thread::available_parallelism().map_or(1, |n| n.get()) {
                scope.spawn(|| {
                    while let Some(path) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let name = path.file_name().unwrap().to_string_lossy().into_owned();
                        let data = std::fs::read(path).unwrap();
                        let qpdf = Qpdf::new(path);
                        let bytes = bytes::read_with(&data, &qpdf);
                        let alone = qpdf.read();
                        let mut found = match &alone {
                            Ok(alone) => differences(&bytes, alone),
                            Err(why) => vec![("qpdf reads nothing".to_string(), why.clone())],
                        };
                        if let Some(why) = &bytes.trailer.unwritten {
                            found.push(("the bytes reader scanned".to_string(), why.clone()));
                        }
                        if let Ok(a) = &alone {
                            if let Some(why) = &a.trailer.unwritten {
                                found.push(("qpdf reconstructed".to_string(), why.clone()));
                            }
                        }
                        // Where either repaired the file, each its own way,
                        // their differences are apart.
                        let repaired = bytes.trailer.unwritten.is_some()
                            || alone.as_ref().is_ok_and(|a| a.trailer.unwritten.is_some());
                        if repaired {
                            for (kind, _) in &mut found {
                                if !kind.starts_with("qpdf reconstructed")
                                    && !kind.starts_with("the bytes reader scanned")
                                {
                                    *kind = format!("in a repaired file, {kind}");
                                }
                            }
                        }
                        let mut tally = tally.lock().unwrap();
                        tally.objects += bytes.ids().len();
                        tally.files_differing += usize::from(!found.is_empty());
                        for (kind, d) in found {
                            add(&mut tally.differences, kind, format!("{name}: {d}"));
                        }
                    }
                });
            }
        });
        let tally = tally.into_inner().unwrap();
        println!(
            "{} files in {:.1} s, {} objects: {} files read otherwise",
            files.len(),
            start.elapsed().as_secs_f64(),
            tally.objects,
            tally.files_differing,
        );
        for (kind, (n, examples)) in &tally.differences {
            println!("{kind}: {n}");
            for example in examples {
                let example: String = example.chars().take(300).collect();
                println!("    {example}");
            }
        }
    }

    /// Streams written by hand, decoded by the reader and by qpdf (at
    /// `specialized`): alike, but where the reader says a fault qpdf
    /// passes over, and where qpdf departs from the spec.
    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn hand_made_streams_decode_as_through_qpdf_but_where_it_departs_from_the_spec() {
        /// How the reader's decoding should stand to qpdf's.
        #[derive(Debug, PartialEq)]
        enum Expect {
            /// The same bytes, and no fault.
            Alike,
            /// The same bytes, and the reader says this fault.
            AlikeBut(&'static str),
            /// The reader stops where the spec says, qpdf reads on.
            Shorter,
            /// qpdf gives it undecoded; the reader decodes it.
            QpdfRefuses,
            /// The reader doesn't decode it; qpdf does.
            ReaderRefuses,
        }
        use Expect::*;
        let text = b"BT /F1 12 Tf (stream data) Tj ET\n".repeat(20);
        let hex = |data: &[u8]| -> Vec<u8> {
            data.iter()
                .flat_map(|b| format!("{b:02x}").into_bytes())
                .collect()
        };
        let ended = |data: &[u8]| [data, b">"].concat();
        let mut damaged = fixture::zlib(&text);
        *damaged.last_mut().unwrap() ^= 1;
        // Rows of four, each by a tag of its own, 0 to 4.
        let rows: Vec<u8> = (0..40u8)
            .flat_map(|i| [i % 5, i, i.wrapping_mul(7), 200, i ^ 0x55])
            .collect();
        let png = dict! { "Predictor" => 12, "Columns" => 4 };
        // Sixteen rows of four pixels of three samples.
        let tiff = dict! { "Predictor" => 2, "Colors" => 3, "Columns" => 4 };
        let tiff3 = dict! { "Predictor" => 2, "BitsPerComponent" => 3 };
        let identity = dict! { "Name" => "Identity" };
        let crypt: Vec<Object> = vec![identity.into(), Object::null()];
        let crypted: Vec<Object> = vec![Object::from("Crypt"), Object::from("FlateDecode")];
        let cases: Vec<(&str, Dict, Vec<u8>, Expect)> = vec![
            (
                "ASCIIHex",
                dict! { "Filter" => "ASCIIHexDecode" },
                ended(&hex(&text)),
                Alike,
            ),
            (
                "ASCII85",
                dict! { "Filter" => "ASCII85Decode" },
                fixture::ascii85(&text),
                Alike,
            ),
            (
                "LZW",
                dict! { "Filter" => "LZWDecode" },
                fixture::lzw(&text),
                Alike,
            ),
            (
                "RunLength",
                dict! { "Filter" => "RunLengthDecode" },
                fixture::run_length(&text),
                Alike,
            ),
            (
                "Flate",
                dict! { "Filter" => "FlateDecode" },
                fixture::zlib(&text),
                Alike,
            ),
            (
                "Fl",
                dict! { "Filter" => "Fl" },
                fixture::zlib(&text),
                Alike,
            ),
            (
                "PNG",
                dict! { "Filter" => "FlateDecode", "DecodeParms" => png },
                fixture::zlib(&rows),
                Alike,
            ),
            (
                "TIFF",
                dict! { "Filter" => "FlateDecode", "DecodeParms" => tiff },
                fixture::zlib(&rows[..192]),
                Alike,
            ),
            (
                "Crypt",
                dict! { "Filter" => crypted, "DecodeParms" => crypt },
                fixture::zlib(&text),
                Alike,
            ),
            (
                "checksum",
                dict! { "Filter" => "FlateDecode" },
                damaged,
                AlikeBut("Checksum"),
            ),
            (
                "no >",
                dict! { "Filter" => "ASCIIHexDecode" },
                hex(&text),
                AlikeBut("NoEnd"),
            ),
            // ISO 32000-2, 7.4.5: 128 is the end; qpdf reads on past it.
            (
                "past 128",
                dict! { "Filter" => "RunLengthDecode" },
                [&fixture::run_length(&text)[..], b"\x02abc\x80"].concat(),
                Shorter,
            ),
            // 7.2.3: NUL is white-space, which qpdf takes as a fault.
            (
                "NUL",
                dict! { "Filter" => "ASCIIHexDecode" },
                ended(&[&b"41\x00"[..], &hex(&text)].concat()),
                QpdfRefuses,
            ),
            // 7.4.4.4: TIFF's samples are 1, 2, 4, 8 or 16 bits.
            (
                "3 bits",
                dict! { "Filter" => "FlateDecode", "DecodeParms" => tiff3 },
                fixture::zlib(&rows),
                ReaderRefuses,
            ),
        ];
        let dir = scratch("hand-made");
        let path = dir.path().join("streams.pdf");
        let mut objects = clean();
        let first = 10;
        for (i, (_, dict, data, _)) in cases.iter().enumerate() {
            objects.push((first + i as u32, Body::stream(dict.clone(), data)));
        }
        let root = dict! { "Root" => Object::reference(1) };
        std::fs::write(&path, fixture::file("1.7", &objects, root, Xref::Table)).unwrap();
        let data = std::fs::read(&path).unwrap();
        let doc = bytes::read(&data);
        let ids: Vec<ObjectId> = (0..cases.len()).map(|i| (first + i as u32, 0)).collect();
        let theirs = Qpdf::new(&path).specialized().decode(&ids);
        for (i, (name, _, _, expect)) in cases.iter().enumerate() {
            let id = ids[i];
            let object = doc.get(id).unwrap();
            let Value::Stream(stream) = object.value() else {
                panic!("{name}")
            };
            let at = stream.data.at().unwrap();
            let written = &data[at.start..at.end];
            let theirs = &theirs[&id];
            let got = match crate::filter::filters(&stream.dict, Some(&|_| None)) {
                Err(_) => match theirs {
                    Ok(_) => ReaderRefuses,
                    Err(why) => panic!("{name}: neither decodes: {why}"),
                },
                Ok(filters) => {
                    let (mine, faults) = match pdf_filters::decode(
                        written,
                        &filters,
                        bytes::STREAM,
                        Backend::MinizOxide,
                    ) {
                        Ok(data) => (data, Vec::new()),
                        Err(e) => (e.data, e.faults),
                    };
                    match (theirs, &faults[..]) {
                        (Ok(t), []) if *t == mine => Alike,
                        (Ok(t), [(_, fault)]) if *t == mine => {
                            let kind = format!("{fault:?}");
                            AlikeBut(if kind.starts_with("Checksum") {
                                "Checksum"
                            } else if kind.starts_with("NoEnd") {
                                "NoEnd"
                            } else {
                                "other"
                            })
                        }
                        (Ok(t), []) if t.starts_with(&mine) && t.len() > mine.len() => Shorter,
                        (Err(why), []) if why.contains("doesn't decode its filters") => QpdfRefuses,
                        (theirs, faults) => {
                            panic!("{name}: qpdf {theirs:?}, the reader's faults {faults:?}")
                        }
                    }
                }
            };
            assert_eq!(&got, expect, "{name}");
        }
    }

    /// The PDF files under `PDF_LINT_CORPUS`, sorted.
    fn corpus() -> Vec<PathBuf> {
        let dir = std::env::var_os("PDF_LINT_CORPUS").expect("PDF_LINT_CORPUS names a folder");
        let mut folders = vec![PathBuf::from(dir)];
        let mut files = Vec::new();
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    folders.push(path);
                } else if (path.extension()).is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
                    files.push(path);
                }
            }
        }
        files.sort();
        files
    }

    /// A difference between decoding through the reader and through qpdf
    /// that's understood, as `kind` names it, and why it's no fault of
    /// the reader's.
    fn explained(kind: &str) -> Option<&'static str> {
        let stopping = [
            "Byte",
            "BrokenEnd",
            "InGroup",
            "Code",
            "Header",
            "Corrupt",
            "Truncated",
            "Limit",
        ];
        let faults = kind
            .rsplit_once("faults: ")
            .map_or("", |(_, faults)| faults);
        let stopped = stopping.iter().any(|f| faults.contains(f));
        match kind {
            _ if kind.starts_with("qpdf doesn't read the stream the reader does") => Some(
                "qpdf finds a stream by its number, which an update can give another, reads one of a file it reconstructed, or refuses the whole file; the reader decodes the bytes it reads",
            ),
            _ if kind.starts_with("a stream decodes alike") => Some(
                "the same bytes: the reader says what qpdf reads without a word (no end marker, a checksum that doesn't match, an unknown row tag, a short row, data that ends early)",
            ),
            _ if kind.starts_with("qpdf leaves a stream undecoded") && stopped => Some(
                "a fault stopped a filter: qpdf gives the data undecoded, the reader what decoded before the fault",
            ),
            _ if kind.starts_with("qpdf leaves a stream undecoded")
                && faults.contains("TableFull") =>
            {
                Some(
                    "an LZW table full with no clear code: qpdf stops and gives the data undecoded, the reader reads on against the table as it is, as PDFium and pdf.js do",
                )
            }
            _ if kind
                .starts_with("a stream decodes otherwise (the reader's is a prefix of qpdf's)")
                && faults.contains("Limit") =>
            {
                Some("the reader stops at its limit, 256 MiB a stream")
            }
            _ if kind.starts_with("qpdf can't decode a stream") && faults == "Limit" => Some(
                "a predictor's rows too long to hold: qpdf exits for the overflow, the reader stops at its limit",
            ),
            _ => None,
        }
    }

    /// Every stream with filters the reader decodes, in each file of a
    /// folder (`PDF_LINT_CORPUS`), decoded by the reader and by qpdf (at
    /// `specialized`, RunLength too), byte for byte; and the object
    /// streams' data and objects as the reader reads them and as it reads
    /// them asking qpdf. Each difference is counted by its kind, with
    /// examples, and each kind must be one [`explained`] knows. A file that
    /// encrypts is left out: its streams are qpdf's to decrypt. With
    /// `PDF_LINT_ORACLE_DUMP` naming a folder, each stream that differs is
    /// written there as written, as the reader decodes it and as qpdf does.
    #[test]
    #[ignore = "needs PDF_LINT_CORPUS, a folder of PDF files, and qpdf"]
    fn a_corpus_decodes_alike_through_qpdf() {
        use std::sync::Mutex;

        use pdf_filters::Fault;

        let files = corpus();
        type Kinds = BTreeMap<String, (usize, Vec<String>)>;
        #[derive(Default)]
        struct Tally {
            files: usize,
            encrypted: usize,
            streams: usize,
            alike: usize,
            kinds: Kinds,
        }
        let tally = Mutex::new(Tally::default());
        let next = AtomicUsize::new(0);
        let start = Instant::now();
        let dump = std::env::var_os("PDF_LINT_ORACLE_DUMP").map(PathBuf::from);
        // A fault's kind, without its fields.
        let kind = |fault: &Fault| {
            let named = format!("{fault:?}");
            named
                .split([' ', '{'])
                .next()
                .unwrap_or_default()
                .to_string()
        };
        let faults = |faults: &[(usize, Fault)]| {
            let mut kinds: Vec<String> = faults.iter().map(|(_, f)| kind(f)).collect();
            kinds.sort();
            kinds.dedup();
            match kinds.is_empty() {
                true => "none".to_string(),
                false => kinds.join(", "),
            }
        };
        // How the reader's data stands to qpdf's.
        let relation = |ours: &[u8], theirs: &[u8]| match () {
            _ if theirs.starts_with(ours) => "the reader's is a prefix of qpdf's",
            _ if ours.starts_with(theirs) => "qpdf's is a prefix of the reader's",
            _ => "neither is a prefix of the other",
        };
        thread::scope(|scope| {
            for _ in 0..thread::available_parallelism().map_or(1, |n| n.get()) {
                scope.spawn(|| {
                    while let Some(path) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let name = path.file_name().unwrap().to_string_lossy().into_owned();
                        let data = std::fs::read(path).unwrap();
                        let ours = bytes::read(&data);
                        let encrypted = (ours.trailer.dict().get(b"Encrypt")).is_some_and(|e| !e.is_null());
                        if encrypted {
                            let mut tally = tally.lock().unwrap();
                            tally.files += 1;
                            tally.encrypted += 1;
                            continue;
                        }
                        let qpdf = Qpdf::new(path).specialized();
                        let mut found: Vec<(String, String)> = Vec::new();
                        // The reader's own streams, and their objects, read
                        // asking qpdf.
                        let through = bytes::read_through(&data, &qpdf);
                        let ids: std::collections::BTreeSet<&ObjectId> = ours.decoded.keys().chain(through.decoded.keys()).collect();
                        for id in ids {
                            let kind = match (ours.decoded.get(id), through.decoded.get(id)) {
                                (Some(a), Some(b)) if a == b => continue,
                                (Some(a), Some(b)) => format!("an object stream decodes otherwise: {}", relation(a, b)),
                                (Some(_), None) => "an object stream only the reader decodes".to_string(),
                                (None, _) => "an object stream only qpdf decodes".to_string(),
                            };
                            found.push((kind, format!("{} {} R", id.0, id.1)));
                        }
                        for id in ours.ids() {
                            let kind = match (ours.get(id), through.contains(id).then(|| through.get(id))) {
                                (Ok(a), Some(Ok(b))) if a == b => continue,
                                (Err(_), Some(Err(_)) | None) => continue,
                                (Ok(_), Some(Ok(_))) => "an object reads otherwise",
                                (Ok(_), Some(Err(_)) | None) => "an object only the reader reads",
                                (Err(_), Some(Ok(_))) => "an object only qpdf's decoding reads",
                            };
                            found.push((kind.to_string(), format!("{} {} R", id.0, id.1)));
                        }
                        // Every stream with filters it decodes.
                        let resolve = |id| ours.get(id).ok();
                        let streams: Vec<(ObjectId, Vec<pdf_filters::Filter>, &[u8])> = (ours.objects())
                            .filter_map(|(id, object)| {
                                let Value::Stream(stream) = object.value() else {
                                    return None;
                                };
                                let at = stream.data.at().filter(|at| at.stream.is_none())?;
                                let filters = crate::filter::filters(&stream.dict, Some(&resolve)).ok()?;
                                (!filters.is_empty()).then(|| (id, filters, &data[at.start..at.end]))
                            })
                            .collect();
                        let ids: Vec<ObjectId> = streams.iter().map(|&(id, _, _)| id).collect();
                        let mut theirs = qpdf.decode(&ids);
                        let mut alike = 0;
                        // Each that differs, its kind as far as the data
                        // tells; whether qpdf read the same bytes is asked
                        // after, of those.
                        let mut differing: Vec<(ObjectId, String, bool)> = Vec::new();
                        for (id, filters, written) in &streams {
                            let (mine, mine_faults) = match pdf_filters::decode(written, filters, bytes::STREAM, Backend::MinizOxide) {
                                Ok(data) => (data, Vec::new()),
                                Err(e) => (e.data, e.faults),
                            };
                            let mut faults = faults(&mine_faults);
                            if mine_faults.iter().any(|(i, _)| *i > 0) {
                                faults = format!("{faults} (in a later filter)");
                            }
                            let theirs = theirs.remove(id);
                            if let Some(dir) = &dump {
                                if !matches!(&theirs, Some(Ok(t)) if *t == mine && mine_faults.is_empty()) {
                                    let file = |what: &str| dir.join(format!("{name}-{}-{}.{what}", id.0, id.1));
                                    std::fs::write(file("raw"), written).unwrap();
                                    std::fs::write(file("ours"), &mine).unwrap();
                                    let qpdf = match &theirs {
                                        Some(Ok(t)) => t.clone(),
                                        Some(Err(why)) => format!("ERR {why}").into_bytes(),
                                        None => b"NONE".to_vec(),
                                    };
                                    std::fs::write(file("qpdf"), qpdf).unwrap();
                                }
                            }
                            // Whether it matters which bytes qpdf read.
                            let (kind, which) = match theirs {
                                Some(Ok(t)) if t == mine && mine_faults.is_empty() => {
                                    alike += 1;
                                    continue;
                                }
                                Some(Ok(t)) if t == mine => (format!("a stream decodes alike, the reader's faults: {faults}"), false),
                                Some(Ok(t)) => (
                                    format!("a stream decodes otherwise ({}), the reader's faults: {faults}", relation(&mine, &t)),
                                    true,
                                ),
                                Some(Err(why)) if why.contains("doesn't decode its filters") => {
                                    (format!("qpdf leaves a stream undecoded, the reader's faults: {faults}"), true)
                                }
                                Some(Err(_)) | None => (format!("qpdf can't decode a stream, the reader's faults: {faults}"), true),
                            };
                            differing.push((*id, kind, which));
                        }
                        let asked: Vec<ObjectId> = (differing.iter()).filter(|d| d.2).map(|d| d.0).collect();
                        let read = Qpdf::new(path).undecoding().decode(&asked);
                        for (id, kind, which) in differing {
                            let written = streams.iter().find(|s| s.0 == id).map(|s| s.2);
                            let same = matches!(read.get(&id), Some(Ok(raw)) if Some(&raw[..]) == written);
                            let kind = match which && !same {
                                true => format!("qpdf doesn't read the stream the reader does: {kind}"),
                                false => kind,
                            };
                            found.push((kind, format!("{} {} R", id.0, id.1)));
                        }
                        let mut tally = tally.lock().unwrap();
                        tally.files += 1;
                        tally.streams += streams.len();
                        tally.alike += alike;
                        for (kind, example) in found {
                            let (n, examples) = tally.kinds.entry(kind).or_default();
                            *n += 1;
                            if examples.len() < 5 {
                                examples.push(format!("{name}: {example}"));
                            }
                        }
                    }
                });
            }
        });
        let tally = tally.into_inner().unwrap();
        println!(
            "{} files ({} encrypted, left out) in {:.0?}: {} streams with filters the reader decodes, {} alike",
            tally.files,
            tally.encrypted,
            start.elapsed(),
            tally.streams,
            tally.alike
        );
        let mut unexplained = Vec::new();
        for (kind, (n, examples)) in &tally.kinds {
            let why = explained(kind);
            println!(
                "{n:>8}  {kind}{}",
                why.map_or(String::new(), |why| format!(": {why}"))
            );
            for example in examples {
                println!("            e.g. {example}");
            }
            if why.is_none() {
                unexplained.push(kind.clone());
            }
        }
        assert!(unexplained.is_empty(), "unexplained: {unexplained:#?}");
    }

    /// The passwords the test suites' encrypted files are opened with, as
    /// their tests give them.
    const PASSWORDS: [&str; 26] = [
        "user",
        "owner",
        "z",
        "u",
        "quack",
        "asdfqwer",
        "π",
        "ʬ",
        "Á•Ž€π🥔",
        "Á•Ž€",
        "y",
        "x",
        "user3",
        "owner3",
        "pass",
        "khoai",
        "attachment",
        "623",
        "19723102477",
        "secret",
        "usersecret",
        "ownersecret",
        "test",
        "password",
        "foo",
        "a",
    ];

    /// A difference between decrypting through the reader and through
    /// qpdf that's understood, as `kind` names it, and why it's no fault of
    /// the reader's.
    fn decryption_explained(kind: &str) -> Option<&'static str> {
        match kind {
            _ if kind.contains("in a file qpdf refuses") => Some(
                "qpdf refuses the whole file (a page tree without /Kids), and so gives none of its streams",
            ),
            _ if kind.starts_with("verdicts differ: the reader: plain; qpdf: locked") => Some(
                "nothing the reader reads is encrypted (/StmF and /StrF /Identity, as when only embedded files are), so it reads whole without the password, which qpdf asks for",
            ),
            _ if kind.starts_with("verdicts differ: the reader: opens; qpdf: can't open (root of pages tree has no /Kids array)") => Some(
                "qpdf refuses the whole file for its page tree; the reader opens it",
            ),
            "verdicts differ: the reader: can't open (its /Encrypt doesn't read); qpdf: opens" => Some(
                "junk before %PDF, which qpdf shifts every offset for and the reader doesn't yet: no object reads, the encryption dictionary first",
            ),
            _ if kind.starts_with("decrypted, ") && kind.contains("by a /Crypt filter that names none") => Some(
                "qpdf decrypts what isn't encrypted (see below), and what it gives doesn't decode",
            ),
            _ if kind.starts_with("decrypted, ") && kind.contains("an embedded file by /EFF") => Some(
                "qpdf decrypts it as /StmF says (see below), and what it gives doesn't decode",
            ),
            _ if kind.starts_with("decrypted, ") => explained(&kind["decrypted, ".len()..]),
            _ if kind.starts_with("decrypts otherwise") && kind.contains("an embedded file by /EFF") => Some(
                "an embedded file without a crypt filter of its own: the reader decrypts it by the one /EFF names, as ISO 32000-2's Table 20 has it; qpdf as /StmF says, reading /EFF only for what a writer should encrypt new embedded files by",
            ),
            _ if kind.contains("by a /Crypt filter that names none") => Some(
                "a /Crypt filter without /Name is /Identity (ISO 32000-2, Table 14), and the data plain (XMP, zlib): qpdf takes a /Name only from parameters of /Type /CryptFilterDecodeParms or in an array, and else decrypts by /StmF",
            ),
            "decrypts alike, the reader's fault: Short { len: 16 }" => Some(
                "just an IV: both decrypt it to nothing; the reader says it lacks the padding AES-CBC always writes",
            ),
            _ if kind.starts_with("decrypts otherwise (the reader's is a prefix of qpdf's), the reader's fault: PartBlock") => Some(
                "a part block past the whole blocks: the reader leaves it off, qpdf pads it with zeros and decrypts it",
            ),
            _ => None,
        }
    }

    /// Every encrypted PDF under `PDF_LINT_CORPUS`, opened by the reader
    /// with the empty password and by qpdf: whether it opens, is locked, or
    /// has a handler that can't open, the reader's verdict against qpdf's
    /// `--requires-password`; and in each the reader opens, every stream's
    /// data decrypted as the reader decrypts (its handler, a stream's own
    /// `/Crypt`, else an embedded file's `/EFF` where it's a name, a
    /// cross-reference stream not, the catalog's metadata stream not under
    /// `/EncryptMetadata false` but by its own) against qpdf's
    /// (`undecoding`), and those
    /// with filters the reader decodes, decrypted and decoded, against
    /// qpdf's at `specialized`. Each difference counted by its kind, with
    /// examples, and each kind must be one [`decryption_explained`] knows.
    #[test]
    #[ignore = "needs PDF_LINT_CORPUS, a folder of PDF files, and qpdf"]
    fn a_corpus_decrypts_alike_through_qpdf() {
        use std::sync::Mutex;

        let files = corpus();
        type Kinds = BTreeMap<String, (usize, Vec<String>)>;
        #[derive(Default)]
        struct Tally {
            files: usize,
            encrypted: usize,
            verdicts: BTreeMap<String, usize>,
            streams: usize,
            alike: usize,
            decoded: usize,
            decoded_alike: usize,
            kinds: Kinds,
        }
        let tally = Mutex::new(Tally::default());
        let next = AtomicUsize::new(0);
        let start = Instant::now();
        let dump = std::env::var_os("PDF_LINT_ORACLE_DUMP").map(PathBuf::from);
        let relation = |ours: &[u8], theirs: &[u8]| match () {
            _ if theirs.starts_with(ours) => "the reader's is a prefix of qpdf's",
            _ if ours.starts_with(theirs) => "qpdf's is a prefix of the reader's",
            _ => "neither is a prefix of the other",
        };
        thread::scope(|scope| {
            for _ in 0..thread::available_parallelism().map_or(1, |n| n.get()) {
                scope.spawn(|| {
                    while let Some(path) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let name = path.file_name().unwrap().to_string_lossy().into_owned();
                        let data = std::fs::read(path).unwrap();
                        let (ours, handler) = bytes::read_opened_by(&data, &[]);
                        if ours.encryption == crate::Encryption::None {
                            tally.lock().unwrap().files += 1;
                            continue;
                        }
                        // Locked, or plain, its embedded files encrypted
                        // alone, by the first password the test suites use
                        // that opens it for qpdf, if one does, given to the
                        // reader as the command line gives it.
                        let password = matches!(ours.encryption, crate::Encryption::Locked | crate::Encryption::Plain)
                            .then(|| PASSWORDS.iter().find(|p| Qpdf::new(path).password(**p).locked() == Ok(false)))
                            .flatten();
                        let (ours, handler) = match password {
                            Some(password) => {
                                bytes::read_opened_by(&data, &[Password(password)])
                            }
                            None => (ours, handler),
                        };
                        let qpdf = match password {
                            Some(password) => Qpdf::new(path).password(*password),
                            None => Qpdf::new(path),
                        };
                        let mut found: Vec<(String, String)> = Vec::new();
                        // The verdicts.
                        let mut args = vec!["--requires-password".to_string(), path.display().to_string()];
                        if let Some(password) = password {
                            args.insert(0, format!("--password={password}"));
                        }
                        let theirs = match qpdf.exec(&args) {
                            Ok(ran) => match ran.code {
                                Some(0) => "locked".to_string(),
                                Some(3) => "opens".to_string(),
                                Some(2) if ran.stderr.is_empty() => "isn't encrypted".to_string(),
                                _ => {
                                    // Its error, after any warnings, without
                                    // the file's name.
                                    let stderr = String::from_utf8_lossy(&ran.stderr);
                                    let why = stderr.lines().rfind(|l| !l.is_empty()).unwrap_or_default();
                                    let why = why.rsplit_once("): ").map_or(why, |(_, w)| w);
                                    let why = why.rsplit_once(".pdf: ").map_or(why, |(_, w)| w);
                                    format!("can't open ({why})")
                                }
                            },
                            Err(why) => format!("doesn't run ({why})"),
                        };
                        let (mine, why) = match &ours.encryption {
                            crate::Encryption::Opened { .. } => ("opens", String::new()),
                            crate::Encryption::Locked => ("locked", String::new()),
                            crate::Encryption::Plain => ("plain", String::new()),
                            crate::Encryption::Unsupported(why) => ("can't open", why.clone()),
                            crate::Encryption::None => unreachable!(),
                        };
                        let by = if password.is_some() { " by a password" } else { "" };
                        let verdict = format!("the reader: {mine}{by}; qpdf: {theirs}");
                        if !theirs.starts_with(mine) {
                            let why = if why.is_empty() { String::new() } else { format!(" ({why})") };
                            let verdict = format!("the reader: {mine}{why}; qpdf: {theirs}");
                            found.push((format!("verdicts differ: {verdict}"), String::new()));
                        }
                        // What qpdf gives of a file it refuses is nothing.
                        let refused = if theirs.starts_with("can't open") { ", in a file qpdf refuses" } else { "" };
                        // Each stream, decrypted.
                        let mut streams = 0;
                        let mut alike = 0;
                        let (mut decoded, mut decoded_alike) = (0, 0);
                        if let Some(handler) = &handler {
                            let resolve = |id| ours.get(id).ok();
                            // The document's metadata stream, the catalog's,
                            // which /EncryptMetadata false leaves plain.
                            let catalog = ours.trailer.dict().get(b"Root").and_then(|r| match *r.value() {
                                Value::Ref(id) => resolve(id),
                                _ => None,
                            });
                            let metadata = catalog
                                .and_then(Object::as_dict)
                                .and_then(|c| c.get(b"Metadata"))
                                .and_then(|m| match *m.value() {
                                    Value::Ref(id) => Some(id),
                                    _ => None,
                                });
                            // The crypt filter /EFF names, which decrypts an
                            // embedded file without one of its own.
                            let followed = |o: &'_ Object| match *o.value() {
                                Value::Ref(id) => resolve(id),
                                _ => None,
                            };
                            let eff = (ours.trailer.dict().get(b"Encrypt"))
                                .and_then(|e| followed(e).or(Some(e)))
                                .and_then(Object::as_dict)
                                .and_then(|d| d.get(b"EFF"))
                                .and_then(|e| followed(e).or(Some(e)))
                                .and_then(Object::as_name);
                            // Each stream: decrypted, its filters, whether a
                            // /Crypt filter of its names none, and whether it's
                            // an embedded file decrypted by /EFF's.
                            type Decrypted = (ObjectId, Result<Vec<u8>, pdf_crypt::Error>, Vec<pdf_filters::Filter>, bool, bool);
                            let mine: Vec<Decrypted> = (ours.objects())
                                .filter_map(|(id, object)| {
                                    let Value::Stream(stream) = object.value() else {
                                        return None;
                                    };
                                    let at = stream.data.at().filter(|at| at.stream.is_none())?;
                                    let written = &data[at.start..at.end];
                                    let typed = stream.dict.get(b"Type").and_then(Object::as_name);
                                    let crypt = crate::filter::crypt_filter(&stream.dict, Some(&resolve));
                                    let own = crypt.as_deref().filter(|&c| c != b"Identity");
                                    let plain = typed == Some(b"XRef")
                                        || (Some(id) == metadata && !handler.encrypts_metadata() && own.is_none());
                                    // A /Crypt filter that names no crypt
                                    // filter, so /Identity.
                                    let named = match stream.dict.get(b"DecodeParms").map(|p| p.value()) {
                                        Some(Value::Dict(d)) => d.has(b"Name"),
                                        Some(Value::Array(a)) => {
                                            a.iter().any(|p| matches!(p.value(), Value::Dict(d) if d.has(b"Name")))
                                        }
                                        _ => false,
                                    };
                                    let unnamed = crypt.is_some() && !named;
                                    let embedded = typed == Some(b"EmbeddedFile") && crypt.is_none();
                                    let by_eff = embedded && eff.is_some();
                                    let crypt = crypt.or_else(|| eff.filter(|_| embedded).map(<[u8]>::to_vec));
                                    let decrypted = match plain {
                                        true => Ok(written.to_vec()),
                                        false => {
                                            let method = crypt.map(|name| handler.named(&name));
                                            handler.decrypt_stream(id, written, method.as_ref())
                                        }
                                    };
                                    let filters = crate::filter::filters(&stream.dict, Some(&resolve)).unwrap_or_default();
                                    Some((id, decrypted, filters, unnamed, by_eff))
                                })
                                .collect();
                            let ids: Vec<ObjectId> = mine.iter().map(|(id, ..)| *id).collect();
                            let mut raw = qpdf.clone().undecoding().decode(&ids);
                            let with_filters: Vec<ObjectId> = (mine.iter())
                                .filter(|(_, _, filters, ..)| !filters.is_empty())
                                .map(|(id, ..)| *id)
                                .collect();
                            let mut specialized = qpdf.clone().specialized().decode(&with_filters);
                            for (id, decrypted, filters, unnamed, by_eff) in mine {
                                streams += 1;
                                let example = format!("{} {} R", id.0, id.1);
                                let (plain, fault) = match decrypted {
                                    Ok(plain) => (Some(plain), String::new()),
                                    Err(e) => (e.data, format!(", the reader's fault: {:?}", e.fault)),
                                };
                                let by = match (unnamed, by_eff) {
                                    (true, _) => ", by a /Crypt filter that names none",
                                    (false, true) => ", an embedded file by /EFF",
                                    (false, false) => "",
                                };
                                let by = format!("{by}{refused}");
                                let theirs = raw.remove(&id);
                                if let Some(dir) = &dump {
                                    if !matches!((&plain, &theirs), (Some(p), Some(Ok(t))) if p == t) {
                                        let file = |what: &str| dir.join(format!("{name}-{}-{}.{what}", id.0, id.1));
                                        std::fs::write(file("ours"), plain.as_deref().unwrap_or(b"NONE")).unwrap();
                                        let qpdf = match &theirs {
                                            Some(Ok(t)) => t.clone(),
                                            Some(Err(why)) => format!("ERR {why}").into_bytes(),
                                            None => b"NONE".to_vec(),
                                        };
                                        std::fs::write(file("qpdf"), qpdf).unwrap();
                                    }
                                }
                                match (&plain, &theirs) {
                                    (Some(p), Some(Ok(t))) if p == t && fault.is_empty() => alike += 1,
                                    (Some(p), Some(Ok(t))) if p == t => found.push((format!("decrypts alike{fault}"), example.clone())),
                                    (Some(p), Some(Ok(t))) => found.push((format!("decrypts otherwise ({}){fault}{by}", relation(p, t)), example.clone())),
                                    (None, Some(Ok(_))) => found.push((format!("only qpdf decrypts{fault}{by}"), example.clone())),
                                    (_, Some(Err(_)) | None) => found.push((format!("qpdf gives no data{fault}{by}"), example.clone())),
                                }
                                if filters.is_empty() {
                                    continue;
                                }
                                decoded += 1;
                                let Some(plain) = plain else { continue };
                                let (mine, mine_faults) = match pdf_filters::decode(&plain, &filters, bytes::STREAM, Backend::MinizOxide) {
                                    Ok(data) => (data, Vec::new()),
                                    Err(e) => (e.data, e.faults),
                                };
                                // As the decoding oracle names them.
                                let mut kinds: Vec<String> = (mine_faults.iter())
                                    .map(|(_, f)| format!("{f:?}").split([' ', '{']).next().unwrap_or_default().to_string())
                                    .collect();
                                kinds.sort();
                                kinds.dedup();
                                let faults = if kinds.is_empty() { "none".to_string() } else { kinds.join(", ") };
                                let unnamed = match (unnamed, refused.is_empty()) {
                                    (true, _) => " (by a /Crypt filter that names none)",
                                    (false, false) => " (in a file qpdf refuses)",
                                    (false, true) if by_eff => " (an embedded file by /EFF)",
                                    (false, true) => "",
                                };
                                let kind = match specialized.remove(&id) {
                                    Some(Ok(t)) if t == mine && mine_faults.is_empty() => {
                                        decoded_alike += 1;
                                        continue;
                                    }
                                    Some(Ok(t)) if t == mine => format!("a stream decodes alike, the reader's faults: {faults}"),
                                    Some(Ok(t)) => format!("a stream decodes otherwise ({}){unnamed}, the reader's faults: {faults}", relation(&mine, &t)),
                                    Some(Err(why)) if why.contains("doesn't decode its filters") => {
                                        format!("qpdf leaves a stream undecoded{unnamed}, the reader's faults: {faults}")
                                    }
                                    Some(Err(_)) | None => format!("qpdf can't decode a stream{unnamed}, the reader's faults: {faults}"),
                                };
                                found.push((format!("decrypted, {kind}"), example));
                            }
                        }
                        let mut tally = tally.lock().unwrap();
                        tally.files += 1;
                        tally.encrypted += 1;
                        *tally.verdicts.entry(verdict).or_default() += 1;
                        tally.streams += streams;
                        tally.alike += alike;
                        tally.decoded += decoded;
                        tally.decoded_alike += decoded_alike;
                        for (kind, example) in found {
                            let (n, examples) = tally.kinds.entry(kind).or_default();
                            *n += 1;
                            if examples.len() < 5 {
                                examples.push(format!("{name}: {example}"));
                            }
                        }
                    }
                });
            }
        });
        let tally = tally.into_inner().unwrap();
        println!(
            "{} files, {} encrypted, in {:.0?}: {} streams in those the reader opens, {} decrypted alike; {} with filters, {} decoded alike",
            tally.files,
            tally.encrypted,
            start.elapsed(),
            tally.streams,
            tally.alike,
            tally.decoded,
            tally.decoded_alike
        );
        for (verdict, n) in &tally.verdicts {
            println!("{n:>8}  {verdict}");
        }
        let mut unexplained = Vec::new();
        for (kind, (n, examples)) in &tally.kinds {
            let why = decryption_explained(kind);
            println!(
                "{n:>8}  {kind}{}",
                why.map_or(String::new(), |why| format!(": {why}"))
            );
            for example in examples {
                println!("            e.g. {example}");
            }
            if why.is_none() {
                unexplained.push(kind.clone());
            }
        }
        assert!(unexplained.is_empty(), "unexplained: {unexplained:#?}");
    }
}
