//! qpdf, the tests' oracle, run as a process: a decoder, decoding what a
//! reader is made to ask of it ([`Qpdf::decode`], [`Qpdf::plaintext`],
//! [`Qpdf::decode_written`]), a compressed cross-reference stream's rows,
//! an object stream's objects, and in an encrypted file the strings'
//! plaintext, to set beside the reader's own decoding; and a second reader
//! ([`Qpdf::listing`], [`Qpdf::read`]).
//! A run decodes every stream it's asked for, through its JSON
//! (`--json-stream-data=inline --decode-level=generalized`), decrypted when
//! the file is encrypted. [`Qpdf::read`] reads a whole file through qpdf
//! alone, a second reader to set beside pdf-structure's. Its answers are
//! pdf-objects' values and text, but `read`'s document, so a crate whose
//! own types a document holds (pdf-structure's tests) builds its document
//! from the listing itself. The JSON is read leniently: qpdf copies a real
//! such as `+.5` into it as the file wrote it, which isn't JSON.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use pdf_objects::object::{Dict, Object, ObjectId, Str, Value};
use pdf_objects::version::Version;
use pdf_structure::read::Unwritten;
use pdf_structure::source::{Document, Trailer, Unread};

use crate::json::{self, Json};

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

/// A stream as written: its `/Filter`'s and `/DecodeParms`'s values and its
/// data, each its bytes in the file.
#[derive(Debug, Clone, Copy)]
pub struct Written<'a> {
    /// The `/Filter` entry's value, as written.
    pub filter: &'a [u8],
    /// The `/DecodeParms` entry's value, as written, where there's one.
    pub parms: Option<&'a [u8]>,
    /// The stream's data.
    pub data: &'a [u8],
}

/// The file as qpdf reads it alone ([`Qpdf::listing`]), in pdf-objects'
/// terms.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    /// The version the header says, as qpdf gives it, when it reads as one.
    pub version: Option<Version>,
    /// Every object qpdf lists, as written but for what qpdf doesn't keep
    /// ([`Qpdf::read`] says what), its strings decrypted; or why it isn't
    /// one, in this reader's words.
    pub objects: BTreeMap<ObjectId, Result<Object, String>>,
    /// The trailer, as a reader gives it: a cross-reference stream, when
    /// one of the objects is the stream whose dictionary it is
    /// ([`Listing::stream`]), else the dictionary.
    pub trailer: Object,
    /// The cross-reference stream the trailer is, when it's one.
    pub stream: Option<ObjectId>,
    /// Why the trailer isn't as the file writes it, when qpdf reconstructed
    /// the cross-references: in qpdf's words, after this reader's.
    pub unwritten: Option<String>,
    /// qpdf's warnings, each with the object it names, if it names one.
    pub warnings: Vec<(Option<ObjectId>, String)>,
    /// The file's length in bytes, when its metadata says.
    pub size: Option<usize>,
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
    pub fn specialized(self) -> Qpdf {
        Qpdf {
            level: "specialized",
            ..self
        }
    }

    /// Not decoding at all (`--decode-level=none`): a stream's data as
    /// qpdf reads it from the file, for the tests to see which it read.
    pub fn undecoding(self) -> Qpdf {
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

    /// The program, as a command to run with arguments of the caller's: for
    /// a test that has qpdf write a file.
    pub fn command(&self) -> Command {
        Command::new(&self.program)
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
    /// (`--json-stream-data=none`), as [`Qpdf::read`] gives it, in
    /// pdf-objects' values and text. The error is why qpdf read nothing.
    pub fn listing(&self) -> Result<Listing, String> {
        let (out, err) = self.run(&self.json_args("none", &[]))?;
        let json = json::parse(&out).map_err(|why| format!("qpdf's JSON doesn't read: {why}"))?;
        let qpdf = json.get("qpdf");
        let version = (qpdf.and_then(|q| q.at(0)))
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
                objects.insert(id, indirect(value));
            } else {
                return Err(format!("qpdf's JSON has {key:?} among its objects"));
            }
        }
        let trailer = trailer.ok_or("qpdf's JSON has no trailer")?;
        let warnings = warnings(&err, &self.file);
        let unwritten = (warnings.iter())
            .find(|(_, message)| message.contains("reconstruct"))
            .map(|(_, message)| format!("qpdf read the file otherwise: {message}"));
        let (stream, trailer) = trailer_of(trailer, &objects);
        let size = (std::fs::metadata(&self.file).ok()).and_then(|m| usize::try_from(m.len()).ok());
        Ok(Listing {
            version,
            objects,
            trailer,
            stream,
            unwritten,
            warnings,
            size,
        })
    }

    /// The file as qpdf reads it, through its JSON
    /// (`--json-stream-data=none`): every object it lists, as written but
    /// for what qpdf doesn't keep, which is where anything was written, the
    /// order of a dictionary's keys and any key written twice, a name's
    /// escapes, and which encoding a text string had ([`Str::Text`]); its
    /// strings decrypted. Where qpdf reconstructed the cross-references, the
    /// trailer says so, in qpdf's words ([`Unwritten::Other`]). The error is
    /// why qpdf read nothing. Built as a reader outside pdf-structure builds
    /// a document ([`Document::new`]), from [`Qpdf::listing`]. qpdf's
    /// warnings are the listing's ([`Listing::warnings`]), beside it: a
    /// document's faults are pdf-structure's reader's, so this one has none.
    pub fn read(&self) -> Result<Document, String> {
        let listing = self.listing()?;
        let version = listing.version.unwrap_or(Version::new(1, 7));
        let mut trailer = Trailer::new(listing.trailer);
        trailer.stream = listing.stream;
        trailer.unwritten = listing.unwritten.map(Unwritten::Other);
        let mut doc = Document::new(version, trailer.dict().clone());
        *doc.trailer_mut() = trailer;
        for (id, object) in listing.objects {
            match object {
                Ok(object) => doc.set(id, object),
                Err(why) => doc.set_unread(id, Unread::Other(why)),
            };
        }
        doc.set_header_version(listing.version);
        doc.set_size(listing.size);
        Ok(doc)
    }

    /// The data of each stream in `ids`, decoded (decompressed, and in an
    /// encrypted file decrypted) at the level it's set to, or why it can't
    /// be: where it's asked to decode and a filter is left, qpdf couldn't.
    pub fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
        let mut decoded = BTreeMap::new();
        self.objects_of("inline", ids, |id, object| {
            decoded.insert(
                id,
                object.and_then(|o| stream_data(o, id, self.level != "none")),
            );
        });
        decoded
    }

    /// In an encrypted file, each object in `ids` as qpdf reads it, its
    /// strings decrypted, or why it can't be.
    pub fn plaintext(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Object, String>> {
        let mut plain = BTreeMap::new();
        self.objects_of("none", ids, |id, object| {
            plain.insert(id, object.and_then(indirect));
        });
        plain
    }

    /// `stream`, which isn't encrypted, decoded from its bytes alone,
    /// through a file of its own, holding the stream alone, as object 3: a
    /// new file in the temp folder, removed after.
    pub fn decode_written(&self, stream: Written) -> Result<Vec<u8>, String> {
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
        let decoded = qpdf
            .decode(&[ObjectId::new(3, 0)])
            .remove(&ObjectId::new(3, 0));
        drop(file);
        decoded.unwrap_or_else(|| Err("qpdf gives nothing".to_string()))
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
        args.extend(
            (ids.iter()).map(|id| format!("--json-object={},{}", id.number, id.generation)),
        );
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
            for &id in ids {
                let object = objects.clone().and_then(|objects| {
                    (objects.get(&format!("obj:{id} R")))
                        .ok_or_else(|| format!("qpdf gives no {id} R"))
                });
                each(id, object);
            }
        }
    }

    /// qpdf's standard output and error, run with `args`: once it exits,
    /// with 0 or with 3, which is success with warnings, and within the
    /// timeout.
    pub fn run(&self, args: &[String]) -> Result<(Vec<u8>, Vec<u8>), String> {
        let ran = self.exec(args)?;
        match ran.code {
            Some(0 | 3) => Ok((ran.stdout, ran.stderr)),
            code => Err(exited(&self.program, code, &ran.stderr)),
        }
    }

    /// qpdf run with `args`, once it exits within the timeout, however it
    /// exits.
    pub fn exec(&self, args: &[String]) -> Result<Ran, String> {
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    /// Its exit code, none for a signal.
    pub code: Option<i32>,
    /// What it wrote to standard output.
    pub stdout: Vec<u8>,
    /// What it wrote to standard error.
    pub stderr: Vec<u8>,
}

/// That `program` exited with `code`, and the last line of what it said,
/// `stderr`.
fn exited(program: &Path, code: Option<i32>, stderr: &[u8]) -> String {
    let said = String::from_utf8_lossy(stderr);
    let said = said.lines().last().unwrap_or("").trim();
    let code = code.map_or("a signal".to_string(), |c| c.to_string());
    format!("{} exited with {code}: {said}", program.display())
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
fn stream_data(object: &Json, id: ObjectId, decoding: bool) -> Result<Vec<u8>, String> {
    let stream = object
        .get("stream")
        .ok_or_else(|| format!("{id} R isn't a stream"))?;
    // qpdf leaves out the filters it decoded: any left, it couldn't, when
    // it was asked to.
    if let Some(filter) = stream.get("dict").and_then(|d| d.get("/Filter"))
        && decoding
    {
        return Err(format!("qpdf doesn't decode its filters ({filter:?})"));
    }
    let data = (stream.get("data").and_then(Json::as_str))
        .ok_or_else(|| format!("qpdf gives no data for {id} R"))?;
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
    let id = ObjectId::new(words.next()?.parse().ok()?, words.next()?.parse().ok()?);
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

/// qpdf's trailer, as a reader gives it: a cross-reference stream, at its
/// number, when one of the objects is the stream whose dictionary it is.
fn trailer_of(
    trailer: Object,
    objects: &BTreeMap<ObjectId, Result<Object, String>>,
) -> (Option<ObjectId>, Object) {
    let stream = (objects.iter()).find_map(|(&id, object)| {
        let s = object.as_ref().ok()?.as_stream()?;
        let xref = s.dict.get(b"Type").and_then(Object::as_name) == Some(b"XRef");
        (xref && Some(&s.dict) == trailer.as_dict()).then(|| (id, s.clone()))
    });
    match stream {
        Some((id, s)) => (Some(id), Object::from(s)),
        None => (None, trailer),
    }
}

/// qpdf's warnings, a line each on its standard error: `WARNING: file
/// (object 5 0, offset 408): message`, the file a name such as `file object
/// stream 12` for an object stream's data, and the parenthesis there or
/// not.
fn warnings(stderr: &[u8], file: &Path) -> Vec<(Option<ObjectId>, String)> {
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
            (object, format!("{message}{offset}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pdf_objects::object::Data;
    use tempfile::TempDir;

    use super::*;

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

    #[test]
    #[ignore = "needs qpdf: just test-qpdf"]
    fn a_qpdf_that_cant_run_says_why_for_every_stream() {
        let decoded = Qpdf::new("x.pdf")
            .program("/nonexistent/qpdf")
            .decode(&[ObjectId::new(1, 0), ObjectId::new(2, 0)]);
        assert_eq!(decoded.len(), 2);
        for why in decoded.values() {
            assert!(
                why.as_ref().unwrap_err().contains("/nonexistent/qpdf"),
                "{why:?}"
            );
        }
        let qpdf = qpdf(Path::new("/nonexistent/file.pdf"));
        let why = qpdf
            .decode(&[ObjectId::new(1, 0)])
            .remove(&ObjectId::new(1, 0))
            .unwrap()
            .unwrap_err();
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
            let why = qpdf
                .decode(&[ObjectId::new(1, 0)])
                .remove(&ObjectId::new(1, 0))
                .unwrap()
                .unwrap_err();
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
        assert_eq!(
            *dict.get(b"Pages").unwrap().value(),
            Value::Ref(ObjectId::new(2, 0))
        );
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
            .map(|(object, message)| (*object, message.as_str()))
            .collect();
        assert_eq!(
            read,
            [
                (
                    Some(ObjectId::new(1, 0)),
                    "dictionary has duplicated key /Dup; last occurrence overrides earlier ones (offset 19)"
                ),
                (
                    Some(ObjectId::new(5, 0)),
                    "expected dictionary key but found non-name object; inserting key /QPDFFake1 (offset 34)"
                ),
                (None, "file is damaged"),
                (None, "xref not found (offset 1234)"),
            ]
        );
    }
}
