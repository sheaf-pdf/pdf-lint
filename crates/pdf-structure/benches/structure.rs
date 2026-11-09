//! pdf-structure timed on files made here, so that a change to the reader
//! or the writer can be judged before and after it, by `cargo bench -p
//! pdf-structure`:
//!
//! - `open`: a file of 2,000 objects opened by `Reading::open`, its header
//!   and cross-reference section read: with a table (`table`); with a
//!   cross-reference stream, the objects but the streams in object streams
//!   (`stream`); the table's file with its `startxref` pointing at no
//!   section, so that the reader finds the objects by scanning (`scanned`);
//!   the table's file with 100 bytes before its header, which its
//!   offsets count from (`prefixed`); and the table's file as a cut left
//!   it, its section and each of its rows three bytes past where they are,
//!   so that the reader finds the section and reads each object at that
//!   shift (`shifted`).
//! - `get`: each of the 2,000 objects of the table's file and the stream's
//!   read by number from a reading just opened, each as it's first asked
//!   for, an object stream decoded once for its members (`table/cold`,
//!   `stream/cold`), and again from a reading that has read them all and
//!   keeps them (`table/kept`, `stream/kept`); and each of 2,000 streams
//!   whose `/Length` objects are in one object stream, read by number from
//!   a reading just opened, the object stream decoded once for their
//!   lengths (`compressed-lengths`).
//! - `whole`: each file read whole into a document: the table's
//!   (`table`) and the stream's (`stream`).
//! - `faults`: the table's file with no object's `endobj` written, so that
//!   each object is a fault of the parser's, read whole into a document,
//!   its faults kept (`kept`) and none looked for (`off`); and a file whose
//!   encryption dictionary, of a handler the reader doesn't know, has
//!   20,000 names each a fault of the parser's, which the open tells as it
//!   reads the dictionary and the whole read tells again as it reads the
//!   object, read whole, its faults kept (`twice`), so that a fault said
//!   once costs a lookup.
//! - `walk`: what's read walked and looked up from outside the crate: the
//!   stream's file's every id listed (`ids`) and entry (`entries`) from a
//!   reading that has read them all, and its document's every object
//!   (`objects`) and record (`records`), and each object by number got from
//!   it, its record and whether it holds it (`lookup`), so that a hint to
//!   inline an iterator's steps or a lookup is judged where a caller pays
//!   for it.
//! - `write`: the objects, built in memory, written as a whole file with a
//!   table (`full`); the table's file read whole written again, each object
//!   copied as the bytes it was read from (`document`); and an update after
//!   that file writing each annotation whose number is a multiple of 20,
//!   changed (`update`); and the objects written as a whole file under a new
//!   AES-256 encryption, each IV drawn from the operating system
//!   (`encrypted`).
//!
//! The objects are a catalog, a page tree and 222 pages, each with its
//! content stream, whose `/Length` is an object of its own, and six link
//! annotations. Throughput is in bytes of the file read or written, but
//! `get`'s, in objects. Each input is held to what it gives before it's
//! timed: a file opened to the objects it lists, its trailer's root, and
//! scanned or not as its group says, with no fault but where it's scanned
//! or has bytes before its header, which is one; each object got to its
//! kind, and each of `compressed-lengths`' streams to its data found by its
//! `/Length` where the crate reads one from an object stream, by
//! `endstream` and a fault where it doesn't; a document to its objects and
//! no fault, or a fault an object, or none where none is looked for; a
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
    use pdf_structure::read::{Faults, Options, Reading};
    pub use pdf_structure::write::Body;
    use pdf_structure::write::{self, Base};
    use pdf_structure::{Document, Source};

    /// A reading of a file in memory.
    pub type Read<'f> = Reading<&'f [u8]>;

    /// `file` opened, as a reading opens a file by default.
    pub fn open(file: &[u8]) -> Read<'_> {
        Reading::open(file, &Options::DEFAULT).expect("a file in memory opens")
    }

    /// `file` read whole, as a document, by default.
    pub fn whole(file: &[u8]) -> Document {
        pdf_structure::read::document(file, &Options::DEFAULT)
    }

    /// Whether a reading can be made to look for no faults: a version of
    /// the crate that can't reads with them always.
    pub const QUIET: bool = true;

    /// `file` read whole, as a document, its faults looked for or not, as
    /// `faults` says, where it can be made to look for none ([`QUIET`]).
    pub fn whole_as(file: &[u8], faults: bool) -> Document {
        let mut options = Options::DEFAULT;
        options.faults = if faults { Faults::Kept } else { Faults::Off };
        pdf_structure::read::document(file, &options)
    }

    /// `reading` read whole, as a document whose objects are its own, which
    /// a write given it copies.
    pub fn document_of(reading: &Read) -> Document {
        reading.to_document().expect("a file in memory reads")
    }

    /// How many faults `reading` has told of what it has read.
    pub fn reading_faults(reading: &Read) -> usize {
        reading.faults().len()
    }

    /// How many faults `doc`'s reading told.
    pub fn faults(doc: &Document) -> usize {
        doc.faults().len()
    }

    /// Whether `doc` holds object `id`, which reads, with its record: each
    /// looked up by number as a caller outside the crate looks it up.
    pub fn lookup(doc: &Document, id: ObjectId) -> bool {
        doc.contains(id) && Source::get(doc, id).is_ok() && doc.record(id).is_some()
    }

    /// Whether `reading` found its objects by scanning the file, its
    /// sections unread.
    pub fn scanned(reading: &Read) -> bool {
        reading.trailer().unwritten.is_some()
    }

    /// Whether a stream's `/Length` that's an object in an object stream
    /// is read from it: a version of the crate that doesn't finds such a
    /// stream's data by `endstream`, and says so.
    pub const MEMBER_LENGTHS: bool = true;

    /// Whether a file's offsets are read from its header (ISO 32000-2,
    /// 7.5.2): a version of the crate that reads them from its first byte
    /// finds the objects of a file with bytes before its header by scanning.
    pub const FROM_HEADER: bool = true;

    /// Whether a section, and the objects it lists, away from where the
    /// file says are read where it proves they are: a version of the crate
    /// that doesn't finds the objects of a file cut so by scanning.
    pub const SHIFTS: bool = true;

    /// Whether `reading` found stream `id`'s data by its `/Length`, as it
    /// read the stream.
    pub fn declared(reading: &Read, id: ObjectId) -> bool {
        (reading.record(id).and_then(|record| record.data)).is_some_and(|data| data.declared)
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

    /// A new file's encryption (AES-256) for `password`, its key and salts
    /// from the operating system.
    pub fn created(password: &str) -> write::Encryptor {
        use pdf_structure::pdf_crypt::{Password, Protection};
        let prepared = Password(password)
            .prepared()
            .expect("a password SASLprep takes");
        write::Encryptor::create(&Protection::new(&prepared)).expect("a new encryption")
    }

    /// `bodies` written as a whole file with a table under `encryptor`, each
    /// AES IV from the operating system.
    pub fn full_under<'a>(
        out: &mut Vec<u8>,
        bodies: impl IntoIterator<Item = (ObjectId, Body<'a>)>,
        trailer: &Dict,
        encryptor: &write::Encryptor,
    ) {
        let options = write::Options::DEFAULT;
        write::full(out, bodies, trailer, Some(encryptor), None, &options)
            .expect("the file is written");
    }

    /// `file` opened by `password`.
    pub fn open_by<'f>(file: &'f [u8], password: &str) -> Read<'f> {
        let passwords = [pdf_structure::pdf_crypt::Password(password)];
        let mut options = Options::DEFAULT;
        options.passwords = &passwords;
        Reading::open(file, &options).expect("a file in memory opens")
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
    indirect_as(out, n, text, true);
}

/// [`indirect`], `endobj` left out unless `endobj`.
fn indirect_as(out: &mut Vec<u8>, n: u32, text: &Text, endobj: bool) {
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
    match endobj {
        true => out.extend_from_slice(b"\nendobj\n"),
        false => out.push(b'\n'),
    }
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
    table_file_as(objects, scanned, true)
}

/// [`table_file`], each object's `endobj` left out unless `endobj`.
fn table_file_as(objects: &[(u32, Text)], scanned: bool, endobj: bool) -> Vec<u8> {
    let mut out = header();
    let mut offsets = vec![0; objects.len() + 1];
    for (n, text) in objects {
        offsets[*n as usize] = out.len();
        indirect_as(&mut out, *n, text, endobj);
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

/// [`table_file`]'s file as a cut before its first object left it: its
/// `startxref` and every row `by` bytes past where they are.
fn shifted_file(objects: &[(u32, Text)], by: usize) -> Vec<u8> {
    let mut out = header();
    let mut offsets = vec![0; objects.len() + 1];
    for (n, text) in objects {
        offsets[*n as usize] = out.len();
        indirect_as(&mut out, *n, text, true);
    }
    let xref = out.len();
    let size = objects.len() + 1;
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in &offsets[1..] {
        out.extend_from_slice(format!("{:010} 00000 n \n", offset + by).as_bytes());
    }
    let startxref = xref + by;
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

/// How many names the `faults/twice` file's encryption dictionary has.
const NAMES: usize = 20_000;

/// A catalog, an empty page tree and an encryption dictionary at offsets,
/// with a table and a trailer naming them: the dictionary's handler is one
/// the reader doesn't know, and each of its [`NAMES`] names has a `#` with
/// no hexadecimal digits after it, a fault of the parser's.
fn twice_file() -> Vec<u8> {
    let names = " /A#zz 1".repeat(NAMES);
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [] /Count 0 >>".to_owned(),
        format!("<< /Filter /Unknown /V 1 /R 2{names} >>"),
    ];
    let mut out = header();
    let mut offsets = Vec::new();
    for (n, value) in (1..).zip(&objects) {
        offsets.push(out.len());
        indirect(&mut out, n, &Text::Value(value.clone()));
    }
    let xref = out.len();
    let size = objects.len() + 1;
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    let trailer = format!(
        "trailer\n<< /Size {size} /Root 1 0 R /Encrypt 3 0 R /ID [<00> <00>] >>\n\
         startxref\n{xref}\n%%EOF\n"
    );
    out.extend_from_slice(trailer.as_bytes());
    out
}

/// The objects in object streams of [`MEMBERS`] each, by number, numbered
/// past them, but the streams and their lengths, at offsets, as many
/// writers put them (`get/compressed-lengths` times lengths in an object
/// stream); and a cross-reference stream (`/W [1 4 2]`) after them,
/// numbered last, naming the catalog. The object streams' data and the
/// cross-reference stream's are deflated.
fn stream_file(objects: &[(u32, Text)]) -> Vec<u8> {
    packed_file(objects, |n, text| match text {
        Text::Value(_) => !length(n),
        Text::Stream(..) => false,
    })
}

/// The objects, those `packed` says in object streams of [`MEMBERS`] each,
/// by number, numbered past them, and the rest at offsets; and a
/// cross-reference stream (`/W [1 4 2]`) after them, numbered last, naming
/// the catalog. The object streams' data and the cross-reference stream's
/// are deflated.
fn packed_file(objects: &[(u32, Text)], packed: impl Fn(u32, &Text) -> bool) -> Vec<u8> {
    let mut out = header();
    // Each number's row: its type, and its two fields.
    let mut rows: Vec<(u8, u32, u16)> = vec![(0, 0, 65535)];
    rows.resize(objects.len() + 1, (0, 0, 0));
    let mut members = Vec::new();
    for (n, text) in objects {
        match text {
            Text::Value(value) if packed(*n, text) => members.push((*n, value.as_str())),
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

/// How many streams `get/compressed-lengths`' file holds.
const LENGTHS: u32 = 2_000;

/// A catalog, 1, and [`LENGTHS`] streams at offsets, 2 on, each a page's
/// content, its `/Length` an object of its own numbered past the streams;
/// the lengths in one object stream, which ISO 32000-2, 7.5.7 lets hold
/// them.
fn lengths_file() -> Vec<u8> {
    let mut objects = vec![(1, Text::Value("<< /Type /Catalog >>".into()))];
    let mut lengths = Vec::new();
    for k in 0..LENGTHS {
        let content = format!("BT /F1 12 Tf 72 720 Td (Stream {k}.) Tj ET\n");
        let dict = format!("<< /Length {} 0 R >>", 2 + LENGTHS + k);
        lengths.push((2 + LENGTHS + k, Text::Value(content.len().to_string())));
        objects.push((2 + k, Text::Stream(dict, content.into_bytes())));
    }
    objects.extend(lengths);
    packed_file(&objects, |n, _| n >= 2 + LENGTHS)
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
/// with a table, with a stream, and scanned; and the table's with a fault
/// an object.
struct Files {
    table: Vec<u8>,
    stream: Vec<u8>,
    scanned: Vec<u8>,
    prefixed: Vec<u8>,
    shifted: Vec<u8>,
    faulty: Vec<u8>,
    twice: Vec<u8>,
    lengths: Vec<u8>,
}

impl Files {
    fn new(objects: &[(u32, Text)]) -> Files {
        Files {
            table: table_file(objects, false),
            stream: stream_file(objects),
            scanned: table_file(objects, true),
            prefixed: [&[b'~'; 100][..], &table_file(objects, false)].concat(),
            shifted: shifted_file(objects, 3),
            faulty: table_file_as(objects, false, false),
            twice: twice_file(),
            lengths: lengths_file(),
        }
    }
}

/// The ids of the objects written, from 1, but the object streams and the
/// cross-reference stream.
fn ids() -> Vec<ObjectId> {
    (1..=OBJECTS).map(|n| ObjectId::new(n, 0)).collect()
}

/// `reading`, held to what its file lists: `listed` objects, the trailer's
/// root the catalog, scanned or not, and with `faults` faults but where
/// scanned.
fn opened(name: &str, reading: &Read, listed: usize, scanned: bool, faults: usize) {
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
        assert_eq!(api::reading_faults(reading), faults, "open/{name}: faults");
    }
}

fn open_group(c: &mut Criterion, files: &Files) {
    // Each object; the stream's file lists its object streams and itself;
    // the prefixed file says its header is past its first byte, and the
    // shifted one that its section and its rows are three bytes off.
    let cases: [(&str, &[u8], usize, bool, usize); 5] = [
        ("table", &files.table, OBJECTS as usize, false, 0),
        (
            "stream",
            &files.stream,
            OBJECTS as usize + CONTAINERS + 1,
            false,
            0,
        ),
        ("scanned", &files.scanned, OBJECTS as usize, true, 0),
        (
            "prefixed",
            &files.prefixed,
            OBJECTS as usize,
            !api::FROM_HEADER,
            1,
        ),
        ("shifted", &files.shifted, OBJECTS as usize, !api::SHIFTS, 1),
    ];
    for &(name, file, listed, scanned, faults) in &cases {
        opened(name, &api::open(file), listed, scanned, faults);
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

/// Each of `compressed-lengths`' streams got from `reading`, a stream, its
/// data found by its `/Length` where the crate reads one from an object
/// stream ([`api::MEMBER_LENGTHS`]), else by `endstream`, a fault each.
fn got_lengths(reading: &Read, streams: &[ObjectId]) {
    for &id in streams {
        let object =
            (reading.get(id)).unwrap_or_else(|e| panic!("get/compressed-lengths: {id} R: {e}"));
        assert!(
            object.as_stream().is_some(),
            "get/compressed-lengths: {id} R"
        );
        assert_eq!(
            api::declared(reading, id),
            api::MEMBER_LENGTHS,
            "get/compressed-lengths: {id} R, by its /Length"
        );
    }
    let faults = if api::MEMBER_LENGTHS {
        0
    } else {
        streams.len()
    };
    assert_eq!(
        api::reading_faults(reading),
        faults,
        "get/compressed-lengths: the faults"
    );
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
    let streams: Vec<ObjectId> = (2..2 + LENGTHS).map(|n| ObjectId::new(n, 0)).collect();
    got_lengths(&api::open(&files.lengths), &streams);
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
    group.throughput(Throughput::Elements(streams.len() as u64));
    let file = &files.lengths;
    group.bench_function("compressed-lengths", |b| {
        b.iter_batched(
            || api::open(file),
            |reading| {
                for &id in &streams {
                    let _ = black_box(reading.get(id));
                }
                reading
            },
            BatchSize::SmallInput,
        )
    });
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

fn walk_group(c: &mut Criterion, files: &Files) {
    let ids = ids();
    let reading = api::open(&files.stream);
    got("stream", &reading, &ids);
    let doc = api::document_of(&reading);
    let listed = OBJECTS as usize + CONTAINERS + 1;
    assert_eq!(
        reading.ids().count(),
        listed,
        "walk/ids: the objects listed"
    );
    assert_eq!(
        reading.entries().count(),
        listed + 1,
        "walk/entries: with 0"
    );
    assert_eq!(doc.objects().count(), listed, "walk/objects: the objects");
    assert_eq!(doc.records().count(), listed, "walk/records: each read");
    for &id in &ids {
        assert!(api::lookup(&doc, id), "walk/lookup: {id} R");
    }
    let mut group = c.benchmark_group("walk");
    group.throughput(Throughput::Elements(listed as u64));
    group.bench_function("ids", |b| {
        b.iter(|| black_box(&reading).ids().fold(0, |n, id| n ^ id.number))
    });
    group.bench_function("entries", |b| {
        b.iter(|| {
            black_box(&reading)
                .entries()
                .fold(0, |n, (number, ..)| n ^ number)
        })
    });
    group.bench_function("objects", |b| {
        b.iter(|| {
            black_box(&doc)
                .objects()
                .fold(0, |n, (id, _)| n ^ id.number)
        })
    });
    group.bench_function("records", |b| {
        b.iter(|| {
            black_box(&doc)
                .records()
                .fold(0, |n, (_, r)| n ^ r.at.start)
        })
    });
    group.throughput(Throughput::Elements(ids.len() as u64));
    group.bench_function("lookup", |b| {
        b.iter(|| {
            for &id in &ids {
                black_box(api::lookup(black_box(&doc), id));
            }
        })
    });
    group.finish();
}

fn faults_group(c: &mut Criterion, files: &Files) {
    let file = &files.faulty;
    let cases: [(&str, bool); 2] = [("kept", true), ("off", false)];
    for &(name, kept) in &cases {
        let doc = api::whole_as(file, kept);
        let listed = OBJECTS as usize;
        assert_eq!(doc.objects().count(), listed, "faults/{name}: the objects");
        // Each object's own: once, where a reader says a `/Length`
        // object's fault once, at its own read; and as many as one more for
        // each content stream where it says it with the stream too, as the
        // base the benchmarks are set beside does.
        let said = api::faults(&doc);
        match kept || !api::QUIET {
            true => assert!(
                (listed..=listed + PAGES as usize).contains(&said),
                "faults/{name}: the faults, {said}"
            ),
            false => assert_eq!(said, 0, "faults/{name}: no fault"),
        }
    }
    // The dictionary's faults, as the open tells them, and the file's own
    // that it doesn't open: each once, where a reader says a fault on an
    // object once, at its own read; and as many as twice, as the base the
    // benchmarks are set beside does.
    let twice = &files.twice;
    let opened = api::reading_faults(&api::open(twice));
    assert!(opened > NAMES, "faults/twice: the open's faults, {opened}");
    let said = api::faults(&api::whole(twice));
    assert!(
        (opened..=2 * opened).contains(&said),
        "faults/twice: the faults, {said}"
    );
    let mut group = c.benchmark_group("faults");
    group.throughput(Throughput::Bytes(file.len() as u64));
    for (name, kept) in cases {
        group.bench_function(name, |b| b.iter(|| api::whole_as(black_box(file), kept)));
    }
    group.throughput(Throughput::Bytes(twice.len() as u64));
    group.bench_function("twice", |b| b.iter(|| api::whole(black_box(twice))));
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
    // Under a new encryption, opened by its password, every object read.
    let created = api::created("bench");
    let mut encrypted = Vec::new();
    api::full_under(&mut encrypted, bodies(), &trailer, &created);
    let opened = api::open_by(&encrypted, "bench");
    for (id, _) in &built {
        (opened.get(*id)).unwrap_or_else(|e| panic!("write/encrypted: {id} R: {e}"));
    }
    assert_eq!(api::reading_faults(&opened), 0, "write/encrypted: no fault");
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
    group.throughput(Throughput::Bytes(encrypted.len() as u64));
    group.bench_function("encrypted", |b| {
        b.iter(|| {
            let mut out = Vec::new();
            api::full_under(&mut out, bodies(), &trailer, &created);
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
    walk_group(&mut criterion, &files);
    faults_group(&mut criterion, &files);
    write_group(&mut criterion, &objects, &files);
    criterion.final_summary();
}
