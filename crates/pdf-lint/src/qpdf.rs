//! qpdf, run as a process, decoding what the bytes reader can't read in
//! plain view ([`Decode`]): a compressed cross-reference stream's rows, an
//! object stream's objects, and in an encrypted file the strings' plaintext.
//! A run decodes every stream it's asked for, through its JSON
//! (`--json-stream-data=inline --decode-level=generalized`), decrypted when
//! the file is encrypted. [`Qpdf::read`] reads a whole file through qpdf
//! alone, a second reader to set beside the bytes reader. The JSON is read
//! leniently: qpdf copies a real such as `+.5` into it as the file wrote
//! it, which isn't JSON.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::bytes::{Decode, Written};
use crate::json::{self, Json};
use crate::object::{Dict, Object, ObjectId, Str, Stream, Value};
use crate::source::{Document, Trailer, Warning};
use crate::version::Version;

/// How many objects one run is asked for by number: past it, a command line
/// grows long.
const PER_RUN: usize = 4096;

/// The oldest qpdf it decodes with. 11.0 is the first to write the JSON
/// it's read through (`--json=2`), but until 11.9 a name that isn't UTF-8
/// is written into it as its bytes, which isn't JSON, and nothing of that
/// run reads.
pub const OLDEST: &str = "11.9";

/// qpdf, to decode one file's streams.
#[derive(Debug, Clone)]
pub struct Qpdf {
    program: PathBuf,
    file: PathBuf,
    password: Option<String>,
    timeout: Duration,
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

    /// The program it runs.
    pub fn program_path(&self) -> &Path {
        &self.program
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

    /// qpdf's version, as [`Qpdf::version`] says it, when it can decode;
    /// else why it can't: it doesn't run, or it's older than [`OLDEST`]. A
    /// version that doesn't read as one is taken as it is.
    pub fn ready(&self) -> Result<String, String> {
        let version = self.version()?;
        let number = version.strip_prefix("qpdf version ");
        match number.and_then(|n| Some((n, release(n)?))) {
            Some((number, at)) if Some(at) < release(OLDEST) => Err(format!(
                "{} is version {number}, older than {OLDEST}",
                self.program.display()
            )),
            _ => Ok(version),
        }
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
                objects.insert(id, indirect(value));
            } else {
                return Err(format!("qpdf's JSON has {key:?} among its objects"));
            }
        }
        let trailer = trailer.ok_or("qpdf's JSON has no trailer")?;
        let warnings = warnings(&err, &self.file);
        Ok(Document {
            trailer: trailer_of(trailer, &objects, &warnings),
            objects,
            header,
            warnings,
            decoded: BTreeMap::new(),
            update: None,
            size: (std::fs::metadata(&self.file).ok()).and_then(|m| usize::try_from(m.len()).ok()),
            hex: None,
            metadata: None,
        })
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
                "--decode-level=generalized",
            ]
            .map(String::from),
        );
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

/// A version's release, `(11, 9)` of `11.9.0`: none when it doesn't begin
/// with two numbers.
fn release(version: &str) -> Option<(u32, u32)> {
    let mut numbers = version.split('.').map(|n| n.parse::<u32>().ok());
    Some((numbers.next()??, numbers.next()??))
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
            decoded.insert(id, object.and_then(|o| stream_data(o, id)));
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

    /// Through a file of its own, holding the stream alone, as object 3.
    fn decode_written(&self, stream: Written) -> Result<Vec<u8>, String> {
        static FILES: AtomicUsize = AtomicUsize::new(0);
        let name = format!(
            "pdf-lint-{}-{}.pdf",
            std::process::id(),
            FILES.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, alone(stream)).map_err(|e| format!("{}: {e}", path.display()))?;
        let qpdf = Qpdf {
            file: path.clone(),
            password: None,
            ..self.clone()
        };
        let decoded = qpdf.decode(&[(3, 0)]).remove(&(3, 0));
        let _ = std::fs::remove_file(&path);
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
fn stream_data(object: &Json, (n, g): ObjectId) -> Result<Vec<u8>, String> {
    let stream = object
        .get("stream")
        .ok_or_else(|| format!("{n} {g} R isn't a stream"))?;
    // qpdf leaves out the filters it decoded: any left, it couldn't.
    if let Some(filter) = stream.get("dict").and_then(|d| d.get("/Filter")) {
        return Err(format!("qpdf doesn't decode its filters ({filter:?})"));
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
    match object(dict)?.value {
        Value::Dict(dict) => Ok(Object::new(Value::Stream(Stream { dict, data: None }))),
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
                Value::String(Str::Bytes(unhex(hex)?))
            } else if let Some(text) = s.strip_prefix("u:") {
                Value::String(Str::Text(text.to_string()))
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
    objects: &BTreeMap<ObjectId, Result<Object, String>>,
    warnings: &[Warning],
) -> Trailer {
    let unwritten = (warnings.iter())
        .find(|w| w.message.contains("reconstruct"))
        .map(|w| format!("qpdf read the file otherwise: {}", w.message));
    let stream = (objects.iter()).find_map(|(&id, object)| match object {
        Ok(Object {
            value: Value::Stream(s),
            ..
        }) if s.dict.get(b"Type").and_then(Object::as_name) == Some(b"XRef")
            && Some(&s.dict) == trailer.as_dict() =>
        {
            Some((id, s.clone()))
        }
        _ => None,
    });
    match stream {
        Some((id, s)) => Trailer {
            stream: Some(id),
            object: Object::new(Value::Stream(s)),
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

    use super::*;
    use crate::bytes;
    use crate::check::{Finding, Hold, Made, Options, check};
    use crate::dict;
    use crate::object::{Object, Str, Value};
    use crate::text;
    use crate::write::{self, Body, Xref};

    /// qpdf, which these tests need: `PDF_LINT_QPDF`, else `qpdf` on the
    /// `PATH`. Its absence fails them, rather than passing them unrun.
    fn qpdf(file: &Path) -> Qpdf {
        let qpdf = Qpdf::new(file);
        if let Err(why) = qpdf.version() {
            panic!("these tests need qpdf ({why}): install it, or name it in PDF_LINT_QPDF");
        }
        qpdf
    }

    /// A scratch folder of the test's own.
    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pdf-lint-qpdf-{}-{test}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A clean file, as the checker's tests have it.
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

    /// The clean file, written, then rewritten by qpdf with `args`; with
    /// `--object-streams=generate`, its dictionaries in object streams, and
    /// a cross-reference stream, compressed.
    fn rewritten(test: &str, args: &[&str]) -> PathBuf {
        let dir = scratch(test);
        let plain = dir.join("plain.pdf");
        let root = dict! { "Root" => Object::reference(1) };
        std::fs::write(&plain, write::file("1.7", &clean(), root, Xref::Table)).unwrap();
        let out = dir.join("compressed.pdf");
        let qpdf = qpdf(&plain);
        let mut command = Command::new(&qpdf.program);
        command.args(args).arg(&plain).arg(&out);
        let status = command.status().unwrap();
        assert!(matches!(status.code(), Some(0 | 3)), "qpdf {args:?} failed");
        out
    }

    /// A decoder that mustn't be asked.
    struct Unasked;

    impl Decode for Unasked {
        fn decode(&self, ids: &[ObjectId]) -> BTreeMap<ObjectId, Result<Vec<u8>, String>> {
            panic!("asked to decode {ids:?}")
        }
    }

    #[test]
    fn a_compressed_file_reads_without_qpdf_as_through_it_and_checks_clean() {
        let path = rewritten("compressed", &["--object-streams=generate"]);
        let data = std::fs::read(&path).unwrap();
        let doc = bytes::read_with(&data, &Unasked);
        assert_eq!(doc.trailer.unwritten, None);
        assert!(doc.trailer.stream.is_some(), "a cross-reference stream");
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        let mut in_streams = 0;
        for (id, object) in &doc.objects {
            let object = object
                .as_ref()
                .unwrap_or_else(|why| panic!("{id:?}: {why}"));
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
        let report = check(&doc, &Hold::All(Made::Merged), &Options::default());
        assert_eq!(report.findings, [], "{report}");
        assert_eq!(report.checked.get("AnnotText"), Some(&1), "{report}");

        // Through qpdf, asked for every stream, alike: the same streams'
        // data, and the same objects read from them.
        let through = bytes::read_through(&data, &qpdf(&path));
        assert!(through.warnings.is_empty(), "{:?}", through.warnings);
        assert_eq!(through.decoded, doc.decoded);
        assert_eq!(through.objects, doc.objects);
    }

    #[test]
    fn an_encrypted_files_object_streams_decrypt_through_qpdf() {
        let path = rewritten(
            "encrypted",
            &[
                "--object-streams=generate",
                "--encrypt",
                "",
                "owner",
                "256",
                "--",
            ],
        );
        let data = std::fs::read(&path).unwrap();
        let doc = bytes::read_with(&data, &qpdf(&path));
        assert_eq!(doc.trailer.unwritten, None);
        let annot = (doc.objects.values().flatten())
            .find(|o| {
                o.as_dict()
                    .is_some_and(|d| d.get(b"Subtype").and_then(Object::as_name) == Some(b"Text"))
            })
            .expect("the annotation reads");
        assert!(annot.at.unwrap().stream.is_some(), "in an object stream");
        let contents = annot.as_dict().unwrap().get(b"Contents").unwrap();
        assert_eq!(
            contents.value,
            Value::String(Str::Bytes(b"A note".to_vec())),
            "plaintext"
        );
    }

    #[test]
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

    #[test]
    fn a_qpdf_is_ready_when_it_runs_and_is_11_9_or_later() {
        let version = qpdf(Path::new("x.pdf")).ready().unwrap();
        assert!(version.starts_with("qpdf version "), "{version}");
        let missing = Qpdf::new("x.pdf").program("/nonexistent/qpdf").ready();
        assert_eq!(missing, Err("/nonexistent/qpdf isn't found".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn a_qpdf_older_than_11_9_isnt_ready() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("old");
        let said = |version: &str| {
            let program = dir.join(format!("qpdf-{version}"));
            let script = format!("#!/bin/sh\necho 'qpdf version {version}'\n");
            std::fs::write(&program, script).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            let qpdf = Qpdf::new("x.pdf").program(&program);
            // Refused a moment while another test's fork holds it open, as
            // a_qpdf_that_hangs_is_stopped says.
            loop {
                match qpdf.ready() {
                    Err(why) if why.contains("Text file busy") => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    said => {
                        return said
                            .map_err(|why| why.replace(&program.display().to_string(), "qpdf"));
                    }
                }
            }
        };
        for old in ["10.6.3", "11.8.0"] {
            assert_eq!(
                said(old),
                Err(format!("qpdf is version {old}, older than 11.9"))
            );
        }
        for new in ["11.9.0", "11.10.1", "12.0.0"] {
            assert_eq!(said(new), Ok(format!("qpdf version {new}")));
        }
        // What doesn't read as a version is taken as it is.
        assert_eq!(said("next"), Ok("qpdf version next".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn a_qpdf_that_hangs_is_stopped() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("hangs");
        let program = dir.join("hang.sh");
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
            numbers.value,
            Value::Array(vec![
                Object::from(0.5),
                Object::from(-0.25),
                Object::from(12),
                Object::null(),
                Object::from(true),
            ])
        );
        let text = &dict.get(b"Odd Name").unwrap().value;
        assert_eq!(text, &Value::String(Str::Text("Café".to_string())));
        let bytes = &dict.get(b"Bytes").unwrap().value;
        assert_eq!(bytes, &Value::String(Str::Bytes(vec![0, 0xFF, 0x10])));
        assert_eq!(dict.get(b"Pages").unwrap().value, Value::Ref((2, 0)));
        let stream = indirect(&json::parse(br#"{"stream": {"dict": {"/Length": 5}}}"#).unwrap());
        let stream = stream.unwrap();
        assert!(
            matches!(&stream.value, Value::Stream(s) if s.dict.get(b"Length").and_then(Object::as_integer) == Some(5) && s.data.is_none())
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
    /// any key written twice, and which encoding a text string had. `a` with
    /// these left out, as `b` has them, compared with `b`: the first
    /// difference, what kind it is and where, or none. `nothing` says
    /// whether a value of `a` is null, or a reference to no object, which
    /// qpdf reads as null.
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
            Value::String(Str::Bytes(_)) => "bytes",
            Value::String(Str::Text(_)) => "text",
            Value::String(Str::Encrypted(_)) => "ciphertext",
            Value::Array(_) => "an array",
            Value::Dict(_) => "a dictionary",
            Value::Stream(_) => "a stream",
            Value::Ref(_) => "a reference",
        };
        let differ = || {
            let kinds = format!("{} and {}", kind(&a.value), kind(&b.value));
            Some((kinds, format!("{at}: {:?} and {:?}", a.value, b.value)))
        };
        let explained = |kind: &str| {
            Some((
                kind.to_string(),
                format!("{at}: {:?} and {:?}", a.value, b.value),
            ))
        };
        match (&a.value, &b.value) {
            (Value::String(Str::Bytes(x)), Value::String(Str::Text(y))) => {
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
            (Value::Ref(_), Value::Null) if nothing(a) => {
                explained("a reference to no object, which qpdf writes as null")
            }
            (Value::Integer(_), Value::Integer(_))
                if a.value != b.value && at.ends_with("/Length") =>
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
            (Value::Dict(x), Value::Dict(y))
            | (Value::Stream(Stream { dict: x, .. }), Value::Stream(Stream { dict: y, .. })) => {
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
                        "keys qpdf leaves out, whose values are null or refer to no object"
                            .to_string(),
                        format!(
                            "{at}: {}",
                            only_x.iter().map(name).collect::<Vec<_>>().join(" ")
                        ),
                    ));
                }
                if !only_x.is_empty() || !only_y.is_empty() {
                    let names =
                        |keys: &[Vec<u8>]| keys.iter().map(name).collect::<Vec<_>>().join(" ");
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
            }
            (x, y) => (x != y).then(differ).flatten(),
        }
    }

    /// The differences between `a`, read from its bytes, and `b`, read
    /// through qpdf alone, each with what kind it is.
    fn differences(a: &Document, b: &Document) -> Vec<(String, String)> {
        let nothing = |o: &Object| match &o.value {
            Value::Null => true,
            Value::Ref(id) => match a.objects.get(id) {
                None | Some(Err(_)) => true,
                Some(Ok(o)) => o.is_null(),
            },
            _ => false,
        };
        let is_xref = |o: &Object| match &o.value {
            Value::Stream(s) => s.dict.get(b"Type").and_then(Object::as_name) == Some(b"XRef"),
            _ => false,
        };
        let mut found: Vec<(String, String)> = Vec::new();
        let mut push = |kind: &str, d: String| found.push((kind.to_string(), d));
        for (id, x) in &a.objects {
            let (n, g) = id;
            match (x, b.objects.get(id)) {
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
        for (id, y) in (b.objects.iter()).filter(|(id, _)| !a.objects.contains_key(id)) {
            let (n, g) = id;
            let kind = match y {
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
    fn a_file_reads_through_qpdf_alone_as_through_its_bytes() {
        let encrypt = ["--encrypt", "", "owner", "256", "--"];
        for (test, args) in [
            ("alike-plain", vec!["--object-streams=disable"]),
            ("alike-compressed", vec!["--object-streams=generate"]),
            (
                "alike-encrypted",
                [&["--object-streams=generate"][..], &encrypt].concat(),
            ),
            (
                "alike-encrypted-plain",
                [&["--object-streams=disable"][..], &encrypt].concat(),
            ),
        ] {
            let path = rewritten(test, &args);
            let data = std::fs::read(&path).unwrap();
            let qpdf = qpdf(&path);
            let bytes = bytes::read_with(&data, &qpdf);
            let alone = qpdf.read().unwrap();
            assert_eq!(differences(&bytes, &alone), [], "{test}");
            assert!(alone.warnings.is_empty(), "{test}: {:?}", alone.warnings);
            assert_eq!(alone.trailer.unwritten, None, "{test}");
            assert!(
                (alone.objects.values().flatten()).all(|o| o.at.is_none()),
                "{test}: qpdf says nowhere"
            );
            // Checked, the two read alike, but for where.
            let findings = |doc: &Document| {
                let report = check(doc, &Hold::All(Made::Merged), &Options::default());
                (report.findings.into_iter())
                    .map(|f| Finding { at: None, ..f })
                    .collect::<Vec<_>>()
            };
            assert_eq!(findings(&bytes), findings(&alone), "{test}");
        }
    }

    #[test]
    fn what_qpdf_keeps_and_loses_reading_alone() {
        let dir = scratch("loses");
        let path = dir.join("odd.pdf");
        let catalog = b"<< /Type /Catalog /Pages 2 0 R /Odd#20Name 1 /Bin#FFx 2 /Dup 1 /Dup 2 /Utf16 <FEFF00410042> /Real +.5 >>";
        let pages = b"<< /Type /Pages /Kids [] /Count 0 >>";
        let file = write::assembled(
            "1.7",
            &[(1, &catalog[..]), (2, &pages[..])],
            "<< /Size 3 /Root 1 0 R >>",
        );
        std::fs::write(&path, &file).unwrap();
        let alone = qpdf(&path).read().unwrap();
        let catalog = alone.objects[&(1, 0)].as_ref().unwrap().as_dict().unwrap();
        assert!(catalog.has(b"Odd Name") && catalog.has(b"Bin\xFFx"));
        assert_eq!(
            catalog.entries().iter().filter(|e| e.key == b"Dup").count(),
            1
        );
        assert_eq!(catalog.get(b"Dup").and_then(Object::as_integer), Some(2));
        assert_eq!(
            catalog.get(b"Utf16").unwrap().value,
            Value::String(Str::Text("AB".to_string())),
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
        let unreadable = dir.join("unreadable.pdf");
        std::fs::write(&unreadable, b"not a PDF").unwrap();
        let why = qpdf(&unreadable).read().unwrap_err();
        assert!(why.contains("exited with 2"), "{why}");

        // Nor what qpdf reconstructs is as the file writes it.
        let damaged = dir.join("damaged.pdf");
        let mut broken = write::file(
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
        assert!(alone.objects.contains_key(&(1, 0)));
    }

    #[test]
    fn a_cross_reference_stream_whose_number_an_update_reuses_decodes_from_its_bytes() {
        let path = rewritten("reused", &["--object-streams=generate"]);
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
        let updated = path.with_file_name("updated.pdf");
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
    fn an_encrypted_files_strings_in_plain_view_decrypt_through_qpdf() {
        let path = rewritten(
            "plain-view",
            &[
                "--object-streams=disable",
                "--encrypt",
                "",
                "owner",
                "256",
                "--",
            ],
        );
        let data = std::fs::read(&path).unwrap();
        let doc = bytes::read_with(&data, &qpdf(&path));
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        let annot = (doc.objects.values().flatten())
            .find(|o| {
                o.as_dict()
                    .is_some_and(|d| d.get(b"Subtype").and_then(Object::as_name) == Some(b"Text"))
            })
            .expect("the annotation reads");
        let contents = annot.as_dict().unwrap().get(b"Contents").unwrap();
        assert_eq!(
            contents.value,
            Value::String(Str::Text("A note".to_string())),
            "plaintext, as qpdf gives it"
        );
        let at = contents.at.unwrap();
        assert_eq!(at.stream, None, "in plain view");
        assert!(
            !data[at.start..at.end].windows(6).any(|w| w == b"A note"),
            "placed on its ciphertext"
        );
        // Without a decoder, it stays ciphertext.
        let doc = bytes::read(&data);
        let annot = (doc.objects.values().flatten())
            .find(|o| o.at.is_some_and(|a| a.start == annot.at.unwrap().start))
            .unwrap();
        let contents = annot.as_dict().unwrap().get(b"Contents").unwrap();
        assert!(matches!(contents.value, Value::String(Str::Encrypted(_))));
    }

    /// Every PDF under `PDF_LINT_CORPUS` read from its bytes, qpdf
    /// decoding, and through qpdf alone, and the differences counted by what
    /// they're of, with examples; and the findings one reading gives and the
    /// other doesn't, but for where and for what only the bytes can show
    /// (`duplicate` and `reader`).
    #[test]
    #[ignore = "needs PDF_LINT_CORPUS, a folder of PDF files, and qpdf"]
    fn a_corpus_reads_alike_through_qpdf_alone() {
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering};

        use crate::check::Rule;

        let dir = std::env::var_os("PDF_LINT_CORPUS").expect("PDF_LINT_CORPUS names a folder");
        let mut folders = vec![PathBuf::from(dir)];
        let mut files = Vec::new();
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    folders.push(path);
                } else if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
                {
                    files.push(path);
                }
            }
        }
        files.sort();
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
            findings_differing: usize,
            differences: Kinds,
            findings: Kinds,
        }
        let tally = Mutex::new(Tally::default());
        let next = AtomicUsize::new(0);
        let start = Instant::now();
        let like = |f: &Finding| {
            let object = f
                .object
                .map_or("the trailer".to_string(), |(n, g)| format!("{n} {g} R"));
            format!("{} {}/{} in {object}", f.rule, f.table, f.key)
        };
        let findings = |doc: &Document| {
            let report = check(doc, &Hold::All(Made::Merged), &Options::default());
            (report.findings.into_iter())
                .filter(|f| !matches!(f.rule, Rule::Duplicate | Rule::Reader))
                .map(|f| Finding { at: None, ..f })
                .collect::<Vec<_>>()
        };
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
                        if let Ok(a) = &alone
                            && let Some(why) = &a.trailer.unwritten
                        {
                            found.push(("qpdf reconstructed".to_string(), why.clone()));
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
                        // The findings one reading gives and the other
                        // doesn't, as many times as it gives them: by object,
                        // table, key and rule, as the path an object is first
                        // reached by follows the order of keys, which qpdf
                        // sorts, and a value prints as its reader gives it.
                        let mut only = Vec::new();
                        if let Ok(a) = &alone {
                            let mut count: BTreeMap<String, (isize, Rule)> = BTreeMap::new();
                            for f in findings(&bytes) {
                                count.entry(like(&f)).or_insert((0, f.rule)).0 += 1;
                            }
                            for f in findings(a) {
                                count.entry(like(&f)).or_insert((0, f.rule)).0 -= 1;
                            }
                            for (f, (n, rule)) in count {
                                let side = if n > 0 { "the bytes" } else { "qpdf" };
                                for _ in 0..n.unsigned_abs() {
                                    only.push((format!("{rule} only through {side}"), f.clone()));
                                }
                            }
                        }
                        let mut tally = tally.lock().unwrap();
                        tally.objects += bytes.objects.len();
                        tally.files_differing += usize::from(!found.is_empty());
                        tally.findings_differing += usize::from(!only.is_empty());
                        for (kind, d) in found {
                            add(&mut tally.differences, kind, format!("{name}: {d}"));
                        }
                        for (kind, f) in only {
                            add(&mut tally.findings, kind, format!("{name}: {f}"));
                        }
                    }
                });
            }
        });
        let tally = tally.into_inner().unwrap();
        println!(
            "{} files in {:.1} s, {} objects: {} files read otherwise, {} with other findings",
            files.len(),
            start.elapsed().as_secs_f64(),
            tally.objects,
            tally.files_differing,
            tally.findings_differing,
        );
        for (kind, (n, examples)) in tally.differences.iter().chain(&tally.findings) {
            println!("{kind}: {n}");
            for example in examples {
                let example: String = example.chars().take(300).collect();
                println!("    {example}");
            }
        }
    }
}
