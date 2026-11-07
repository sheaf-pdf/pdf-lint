//! pdf-structure timed on files made here, so that a change to the reader
//! or the writer can be judged before and after it, by `cargo bench -p
//! pdf-structure`:
//!
//! - `open`: a file of 2,000 objects opened by `Reading::open`, its header
//!   and cross-reference section read: with a table (`table`); with a
//!   cross-reference stream, the objects but the streams in object streams
//!   (`stream`); and the table's file with its `startxref` pointing at no
//!   section, so that the reader finds the objects by scanning (`scanned`).
//! - `get`: each of the 2,000 objects of the table's file and the stream's
//!   read by number from a reading just opened, each as it's first asked
//!   for, an object stream decoded once for its members (`table/cold`,
//!   `stream/cold`), and again from a reading that has read them all and
//!   keeps them (`table/kept`, `stream/kept`).
//! - `whole`: each file read whole into a document: the table's
//!   (`table`) and the stream's (`stream`).
//! - `write`: the objects, built in memory, written as a whole file with a
//!   table (`full`); the table's file read whole written again, each object
//!   copied as the bytes it was read from (`document`); and an update after
//!   that file writing each annotation whose number is a multiple of 20,
//!   changed (`update`).
//!
//! The objects are a catalog, a page tree and 222 pages, each with its
//! content stream, whose `/Length` is an object of its own, and six link
//! annotations. Throughput is in bytes of the file read or written, but
//! `get`'s, in objects. Each input is held to what it gives before it's
//! timed: a file opened to the objects it lists, its trailer's root, and
//! scanned or not as its group says, with no fault but where it's scanned;
//! each object got to its kind; a document to its objects and no fault; a
//! write read back to the objects it was given, an update's changed. The
//! files are written here from text, the object streams' and the
//! cross-reference stream's data deflated through pdf-filters; nothing is
//! read from disk, and the inputs are the same on every run. Every path
//! into the crate is in [`api`], so that a copy of the benchmarks for
//! another version of it differs there alone.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{BatchSize, Criterion, Throughput};
use pdf_filters::{Backend, Encoder, Level, Prediction, encode};
use pdf_objects::object::{Object, ObjectId, Stream, Value};
use pdf_objects::parse::Parser;

/// The crate's API as the benchmarks reach it, the one part of them a copy
/// for another version of the crate rewrites.
mod api {
    use pdf_objects::object::{Dict, ObjectId};
    use pdf_structure::Document;
    use pdf_structure::bytes::{Options, Reading};
    pub use pdf_structure::write::Body;
    use pdf_structure::write::{self, Base};

    /// A reading of a file in memory.
    pub type Read<'f> = Reading<&'f [u8]>;

    /// `file` opened, as a reading opens a file by default.
    pub fn open(file: &[u8]) -> Read<'_> {
        Reading::open(file, &Options::DEFAULT).expect("a file in memory opens")
    }

    /// `file` read whole, as a document, by default.
    pub fn whole(file: &[u8]) -> Document {
        pdf_structure::bytes::read(file)
    }

    /// `reading` read whole, as a document whose objects are its own, which
    /// a write given it copies.
    pub fn document_of(reading: &Read) -> Document {
        reading.to_document().expect("a file in memory reads")
    }

    /// How many faults `reading` has told of what it has read.
    pub fn reading_faults(reading: &Read) -> usize {
        reading.warnings().len()
    }

    /// How many faults `doc`'s reading told.
    pub fn faults(doc: &Document) -> usize {
        doc.warnings.len()
    }

    /// Whether `reading` found its objects by scanning the file, its
    /// sections unread.
    pub fn scanned(reading: &Read) -> bool {
        reading.trailer().unwritten.is_some()
    }

    /// `bodies` written as a whole file with a table under `trailer`.
    pub fn full<'a>(
        out: &mut Vec<u8>,
        bodies: impl IntoIterator<Item = (ObjectId, Body<'a>)>,
        trailer: &Dict,
    ) {
        let options = write::Options::DEFAULT;
        write::full(out, bodies, trailer, None, None, &options).expect("the file is written");
    }

    /// `doc`, `reading` read whole ([`document_of`]), written as a whole
    /// file with a table, each object copied from `reading` where it may
    /// be.
    pub fn document(out: &mut Vec<u8>, doc: &Document, reading: &Read) {
        let options = write::Options::DEFAULT;
        write::document(out, doc, None, Some(reading), &options).expect("the file is written");
    }

    /// What an update after `reading`'s file goes after.
    pub fn base(reading: &Read) -> Base {
        Base::of(reading).expect("the file is updated")
    }

    /// An update after `base` writing `bodies`.
    pub fn update<'a>(
        out: &mut Vec<u8>,
        base: &Base,
        bodies: impl IntoIterator<Item = (ObjectId, Body<'a>)>,
    ) {
        let options = write::Options::DEFAULT;
        write::update(out, base, bodies, None, None, &options).expect("the update is written");
    }
}

use api::{Body, Read};

/// How many pages, each nine objects: its dictionary, its content stream,
/// the stream's length and six annotations.
const PAGES: u32 = 222;

/// How many objects, numbered from 1: the catalog, the page tree and the
/// pages' nine each.
const OBJECTS: u32 = 2 + PAGES * 9;

/// How many objects an object stream holds: as many as most writers put
/// in one.
const MEMBERS: u32 = 100;

/// How many object streams the stream's file has: one for each
/// [`MEMBERS`] of the objects but the content streams and their lengths.
const CONTAINERS: usize = (OBJECTS - 2 * PAGES).div_ceil(MEMBERS) as usize;

/// Whether object `n` is a page's content stream.
fn content(n: u32) -> bool {
    n >= 3 && (n - 3) % 9 == 1
}

/// Whether object `n` is a content stream's length.
fn length(n: u32) -> bool {
    n >= 3 && (n - 3) % 9 == 2
}

/// An object to write: a value's text, or a stream's dictionary's text and
/// its data.
enum Text {
    Value(String),
    Stream(String, Vec<u8>),
}

/// Each object, by number, from 1: the catalog; the page tree; each page's
/// dictionary, its content stream, the stream's `/Length` and six link
/// annotations, each with a rectangle, a border and a URI action.
fn objects() -> Vec<(u32, Text)> {
    let mut objects = vec![(1, Text::Value("<< /Type /Catalog /Pages 2 0 R >>".into()))];
    let kids: Vec<String> = (0..PAGES).map(|k| format!("{} 0 R", 3 + k * 9)).collect();
    let pages = format!(
        "<< /Type /Pages /Kids [{}] /Count {PAGES} >>",
        kids.join(" ")
    );
    objects.push((2, Text::Value(pages)));
    for k in 0..PAGES {
        let page = 3 + k * 9;
        let annots: Vec<String> = (3..9).map(|i| format!("{} 0 R", page + i)).collect();
        let dict = format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /ProcSet [/PDF /Text] /Font << /F1 << /Type /Font \
             /Subtype /Type1 /BaseFont /Helvetica >> >> >> \
             /Contents {} 0 R /Annots [{}] >>",
            page + 1,
            annots.join(" "),
        );
        objects.push((page, Text::Value(dict)));
        let mut content = String::from("BT /F1 12 Tf 14 TL 72 720 Td\n");
        for line in 0..8 {
            let _ = writeln!(content, "(Page {k}, line {line} of the text.) Tj T*");
        }
        content.push_str("ET\n");
        let length = content.len();
        let dict = format!("<< /Length {} 0 R >>", page + 2);
        objects.push((page + 1, Text::Stream(dict, content.into_bytes())));
        objects.push((page + 2, Text::Value(length.to_string())));
        for i in 0..6 {
            let (x, y) = (72 + 80 * i, 100 + 2 * k);
            let annot = format!(
                "<< /Type /Annot /Subtype /Link /Rect [{x} {y} {} {}] \
                 /Border [0 0 0] /A << /S /URI /URI (https://example.com/{k}/{i}) >> \
                 /P {page} 0 R >>",
                x + 60,
                y + 12,
            );
            objects.push((page + 3 + i, Text::Value(annot)));
        }
    }
    assert_eq!(objects.len(), OBJECTS as usize, "the objects' count");
    objects
}

/// The bytes of `n 0 obj`, the object, and `endobj`.
fn indirect(out: &mut Vec<u8>, n: u32, text: &Text) {
    out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
    match text {
        Text::Value(value) => out.extend_from_slice(value.as_bytes()),
        Text::Stream(dict, data) => {
            out.extend_from_slice(dict.as_bytes());
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(data);
            out.extend_from_slice(b"\nendstream");
        }
    }
    out.extend_from_slice(b"\nendobj\n");
}

/// The file's header, and the comment of four bytes past ASCII that marks
/// it binary.
fn header() -> Vec<u8> {
    b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec()
}

/// The objects written at offsets, with a table and a trailer naming the
/// catalog; its `startxref` the table's, or, where `scanned`, one past the
/// file, which is no section.
fn table_file(objects: &[(u32, Text)], scanned: bool) -> Vec<u8> {
    let mut out = header();
    let mut offsets = vec![0; objects.len() + 1];
    for (n, text) in objects {
        offsets[*n as usize] = out.len();
        indirect(&mut out, *n, text);
    }
    let xref = out.len();
    let size = objects.len() + 1;
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in &offsets[1..] {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    let startxref = if scanned { out.len() + 1000 } else { xref };
    let trailer =
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{startxref}\n%%EOF\n");
    out.extend_from_slice(trailer.as_bytes());
    out
}

/// `data` deflated at zlib's default level.
fn deflated(data: &[u8]) -> Vec<u8> {
    let flate = Encoder::Flate {
        level: Level::DEFAULT,
        predictor: Prediction::None,
    };
    let mut out = Vec::new();
    encode(data, &[flate], Backend::MinizOxide, &mut out).expect("Flate encodes");
    out
}

/// The objects in object streams of [`MEMBERS`] each, by number, numbered
/// past them, but the streams and their lengths, at offsets, as the reader
/// reads a stream's `/Length` only from an object at one; and a
/// cross-reference stream (`/W [1 4 2]`) after them, numbered last, naming
/// the catalog. The object streams' data and the cross-reference stream's
/// are deflated.
fn stream_file(objects: &[(u32, Text)]) -> Vec<u8> {
    let mut out = header();
    // Each number's row: its type, and its two fields.
    let mut rows: Vec<(u8, u32, u16)> = vec![(0, 0, 65535)];
    rows.resize(objects.len() + 1, (0, 0, 0));
    let mut members = Vec::new();
    for (n, text) in objects {
        match text {
            Text::Value(value) if !length(*n) => members.push((*n, value.as_str())),
            _ => {
                rows[*n as usize] = (1, out.len() as u32, 0);
                indirect(&mut out, *n, text);
            }
        }
    }
    let mut container = objects.len() as u32;
    for group in members.chunks(MEMBERS as usize) {
        container += 1;
        let (mut pairs, mut body) = (String::new(), String::new());
        for (i, (n, value)) in group.iter().enumerate() {
            let _ = write!(pairs, "{n} {} ", body.len());
            body.push_str(value);
            body.push('\n');
            rows[*n as usize] = (2, container, i as u16);
        }
        let data = deflated(format!("{pairs}\n{body}").as_bytes());
        let dict = format!(
            "<< /Type /ObjStm /N {} /First {} /Filter /FlateDecode /Length {} >>",
            group.len(),
            pairs.len() + 1,
            data.len(),
        );
        rows.push((1, out.len() as u32, 0));
        indirect(&mut out, container, &Text::Stream(dict, data));
    }
    let own = container + 1;
    let xref = out.len();
    rows.push((1, xref as u32, 0));
    let mut table = Vec::with_capacity(rows.len() * 7);
    for (kind, field, generation) in &rows {
        table.push(*kind);
        table.extend_from_slice(&field.to_be_bytes());
        table.extend_from_slice(&generation.to_be_bytes());
    }
    let data = deflated(&table);
    let dict = format!(
        "<< /Type /XRef /Size {} /W [1 4 2] /Root 1 0 R /Filter /FlateDecode /Length {} >>",
        rows.len(),
        data.len(),
    );
    indirect(&mut out, own, &Text::Stream(dict, data));
    out.extend_from_slice(format!("startxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

/// `text` as one value, read by pdf-objects' parser.
fn value(text: &str) -> Object {
    let mut parser = Parser::at(text.as_bytes(), 0);
    (parser.value(&mut ())).unwrap_or_else(|e| panic!("{text}: {e}"))
}

/// The objects as values in memory, unplaced, each stream's data its
/// bytes, its `/Length` theirs.
fn built(objects: &[(u32, Text)]) -> Vec<(ObjectId, Object)> {
    (objects.iter())
        .map(|(n, text)| {
            let mut object = match text {
                Text::Value(text) => value(text),
                Text::Stream(dict, data) => {
                    let Value::Dict(dict) = value(dict).into_value() else {
                        panic!("{n} 0 obj: a stream's dictionary");
                    };
                    Object::from(Stream::new(dict, data.clone()))
                }
            };
            object.unplace();
            (ObjectId::new(*n, 0), object)
        })
        .collect()
}

/// The files the benchmarks read, by the name their benchmarks carry:
/// with a table, with a stream, and scanned.
struct Files {
    table: Vec<u8>,
    stream: Vec<u8>,
    scanned: Vec<u8>,
}

impl Files {
    fn new(objects: &[(u32, Text)]) -> Files {
        Files {
            table: table_file(objects, false),
            stream: stream_file(objects),
            scanned: table_file(objects, true),
        }
    }
}

/// The ids of the objects written, from 1, but the object streams and the
/// cross-reference stream.
fn ids() -> Vec<ObjectId> {
    (1..=OBJECTS).map(|n| ObjectId::new(n, 0)).collect()
}

/// `reading`, held to what its file lists: `listed` objects, the trailer's
/// root the catalog, scanned or not, and with no fault but where scanned.
fn opened(name: &str, reading: &Read, listed: usize, scanned: bool) {
    assert_eq!(
        reading.ids().count(),
        listed,
        "open/{name}: the objects listed"
    );
    let root = reading
        .trailer()
        .dict()
        .get(b"Root")
        .and_then(Object::as_reference);
    assert_eq!(root, Some(ObjectId::new(1, 0)), "open/{name}: the root");
    assert_eq!(api::scanned(reading), scanned, "open/{name}: scanned");
    if !scanned {
        assert_eq!(api::reading_faults(reading), 0, "open/{name}: no fault");
    }
}

fn open_group(c: &mut Criterion, files: &Files) {
    // Each object; the stream's file lists its object streams and itself.
    let cases: [(&str, &[u8], usize, bool); 3] = [
        ("table", &files.table, OBJECTS as usize, false),
        (
            "stream",
            &files.stream,
            OBJECTS as usize + CONTAINERS + 1,
            false,
        ),
        ("scanned", &files.scanned, OBJECTS as usize, true),
    ];
    for &(name, file, listed, scanned) in &cases {
        opened(name, &api::open(file), listed, scanned);
    }
    let mut group = c.benchmark_group("open");
    for (name, file, ..) in cases {
        group.throughput(Throughput::Bytes(file.len() as u64));
        group.bench_function(name, |b| b.iter(|| api::open(black_box(file))));
    }
    group.finish();
}

/// Each object of `ids` got from `reading`, held to its kind: a stream
/// for each page's content, a dictionary or an integer for the rest.
fn got(name: &str, reading: &Read, ids: &[ObjectId]) {
    for &id in ids {
        let object = (reading.get(id)).unwrap_or_else(|e| panic!("get/{name}: {id} R: {e}"));
        let kind = match object.value() {
            Value::Stream(_) => "stream",
            Value::Integer(_) => "integer",
            Value::Dict(_) => "dictionary",
            _ => "other",
        };
        let expected = match (content(id.number), length(id.number)) {
            (true, _) => "stream",
            (_, true) => "integer",
            _ => "dictionary",
        };
        assert_eq!(kind, expected, "get/{name}: {id} R");
    }
}

fn get_group(c: &mut Criterion, files: &Files) {
    let ids = ids();
    let cases: [(&str, &[u8]); 2] = [("table", &files.table), ("stream", &files.stream)];
    let kept: Vec<Read> = (cases.iter())
        .map(|&(name, file)| {
            let reading = api::open(file);
            got(name, &reading, &ids);
            reading
        })
        .collect();
    let mut group = c.benchmark_group("get");
    group.throughput(Throughput::Elements(ids.len() as u64));
    for (&(name, file), reading) in cases.iter().zip(&kept) {
        group.bench_function(format!("{name}/cold"), |b| {
            b.iter_batched(
                || api::open(file),
                |reading| {
                    for &id in &ids {
                        let _ = black_box(reading.get(id));
                    }
                    reading
                },
                BatchSize::SmallInput,
            )
        });
        group.bench_function(format!("{name}/kept"), |b| {
            b.iter(|| {
                for &id in &ids {
                    let _ = black_box(reading.get(id));
                }
            })
        });
    }
    group.finish();
}

fn whole_group(c: &mut Criterion, files: &Files) {
    let cases: [(&str, &[u8], usize); 2] = [
        ("table", &files.table, OBJECTS as usize),
        ("stream", &files.stream, OBJECTS as usize + CONTAINERS + 1),
    ];
    for &(name, file, listed) in &cases {
        let doc = api::whole(file);
        assert_eq!(doc.objects().count(), listed, "whole/{name}: the objects");
        assert_eq!(api::faults(&doc), 0, "whole/{name}: no fault");
    }
    let mut group = c.benchmark_group("whole");
    for (name, file, _) in cases {
        group.throughput(Throughput::Bytes(file.len() as u64));
        group.bench_function(name, |b| b.iter(|| api::whole(black_box(file))));
    }
    group.finish();
}

/// `written`, read whole, held to `built`: each object the same value,
/// unplaced, a stream's dictionary but its `/Length` and its data, with no
/// fault.
fn read_back(name: &str, written: &[u8], built: &[(ObjectId, Object)]) {
    let reading = api::open(written);
    let doc = api::whole(written);
    assert_eq!(api::faults(&doc), 0, "write/{name}: no fault");
    for (id, object) in built {
        let back = (reading.get(*id)).unwrap_or_else(|e| panic!("write/{name}: {id} R: {e}"));
        let mut back = back.clone();
        back.unplace();
        match (back.value(), object.value()) {
            (Value::Stream(back), Value::Stream(object)) => {
                let without = |dict: &pdf_objects::object::Dict| {
                    let mut dict = dict.clone();
                    dict.remove(b"Length");
                    dict
                };
                assert_eq!(
                    without(&back.dict),
                    without(&object.dict),
                    "write/{name}: {id} R"
                );
                let data = reading
                    .data(*id)
                    .unwrap_or_else(|e| panic!("write/{name}: {e}"));
                assert_eq!(
                    Some(&data[..]),
                    object.data.bytes(),
                    "write/{name}: {id} R's data"
                );
            }
            _ => assert_eq!(&back, object, "write/{name}: {id} R"),
        }
    }
}

fn write_group(c: &mut Criterion, objects: &[(u32, Text)], files: &Files) {
    let built = built(objects);
    let trailer = pdf_objects::dict! { "Root" => Object::reference(1) };
    let bodies = || (built.iter()).map(|(id, o)| (*id, Body::New(Cow::Borrowed(o))));
    let mut full = Vec::new();
    api::full(&mut full, bodies(), &trailer);
    read_back("full", &full, &built);
    // The table's file read whole, written again copied.
    let reading = api::open(&files.table);
    let doc = api::document_of(&reading);
    let mut document = Vec::new();
    api::document(&mut document, &doc, &reading);
    read_back("document", &document, &built);
    // Each annotation whose number is a multiple of 20 changed, an entry
    // added: 67 objects of the 2,000.
    let mut changed: Vec<(ObjectId, Object)> = Vec::new();
    for (id, object) in &built {
        if id.number % 20 == 0
            && let Some(dict) = object.as_dict()
            && dict.get(b"Type").and_then(Object::as_name) == Some(b"Annot")
        {
            let mut dict = dict.clone();
            dict.set("NM", Object::text(&format!("changed {}", id.number)));
            changed.push((*id, Object::from(dict)));
        }
    }
    assert_eq!(changed.len(), 67, "write/update: the objects changed");
    let given = || (changed.iter()).map(|(id, o)| (*id, Body::New(Cow::Borrowed(o))));
    let base = api::base(&reading);
    let mut update = files.table.clone();
    api::update(&mut update, &base, given());
    let mut after = built.clone();
    for (id, object) in &changed {
        let at = after
            .iter()
            .position(|(a, _)| a == id)
            .expect("a changed object");
        after[at].1 = object.clone();
    }
    read_back("update", &update, &after);
    let update_len = update.len() - files.table.len();
    let mut group = c.benchmark_group("write");
    group.throughput(Throughput::Bytes(full.len() as u64));
    group.bench_function("full", |b| {
        b.iter(|| {
            let mut out = Vec::new();
            api::full(&mut out, bodies(), &trailer);
            out
        })
    });
    group.throughput(Throughput::Bytes(document.len() as u64));
    group.bench_function("document", |b| {
        b.iter(|| {
            let mut out = Vec::new();
            api::document(&mut out, &doc, &reading);
            out
        })
    });
    group.throughput(Throughput::Bytes(update_len as u64));
    group.bench_function("update", |b| {
        b.iter(|| {
            let mut out = Vec::new();
            api::update(&mut out, &base, given());
            out
        })
    });
    group.finish();
}

/// The groups, as one benchmark binary: what `criterion_group!` and
/// `criterion_main!` would write, without the public function the first
/// makes.
fn main() {
    let objects = objects();
    let files = Files::new(&objects);
    let mut criterion = Criterion::default().configure_from_args();
    open_group(&mut criterion, &files);
    get_group(&mut criterion, &files);
    whole_group(&mut criterion, &files);
    write_group(&mut criterion, &objects, &files);
    criterion.final_summary();
}
