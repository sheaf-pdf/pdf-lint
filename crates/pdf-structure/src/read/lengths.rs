//! A stream's `/Length` in an object stream, which ISO 32000-2, 7.5.7 lets
//! hold every object but an object stream's own `/Length`: read from the
//! object stream's decoded data, as an object in it is read. And the
//! streams whose `/Length` isn't read from one, each read alike whatever
//! read comes first: an object stream's own; a cross-reference stream's,
//! whose `/Length` no read follows, and what an encryption dictionary
//! refers to, which the open reads before it lists the object stream or
//! has its handler; and one whose object stream's decode would read
//! another object. A chain of references isn't followed.

use pdf_crypt::writer::Kind;
use pdf_fixtures::fixture;

use super::tests::{root, settings, to_encrypt};
use super::*;
use crate::fuzz::{Counted, open};
use crate::source::Source;

/// A content stream's data that holds the keyword `endstream`: a reader
/// that reads its `/Length` takes all of it, one that finds its data by
/// the first `endstream` takes its first line.
const CONTENT: &str = "0 0 m 100 100 l S\nendstream\n10 10 50 50 re f";

/// [`CONTENT`] up to the first `endstream`, as a reader that finds its data
/// so takes it, and then finds no `endobj` after that `endstream`.
const SCANNED: &str = "0 0 m 100 100 l S\n";

/// An object stream to write: its number, its dictionary's entries past
/// `/Type /ObjStm`, `/N` and `/First` (a `/Length` among them, or its
/// data's written after them), and its members, each its number and text.
struct Packed<'a> {
    number: u32,
    entries: &'a str,
    members: &'a [(u32, &'a str)],
}

/// `start` followed by `objects` at offsets, each its number and text,
/// then the object streams `packed`, unfiltered, and a cross-reference
/// stream numbered past every object, its dictionary's entries `trailer`
/// and its `/Length` `length` (none: its data's, direct), its data
/// unfiltered and `endstream` right after it, so that it reads the same by
/// its `/Length` and by `endstream`.
fn appended(
    start: &[u8],
    objects: &[(u32, &str)],
    packed: &[Packed],
    trailer: &str,
    length: Option<&str>,
) -> Vec<u8> {
    let mut file = start.to_vec();
    let mut rows: BTreeMap<u32, (u8, usize, u16)> = BTreeMap::new();
    for &(n, text) in objects {
        rows.insert(n, (1, file.len(), 0));
        file.extend_from_slice(format!("{n} 0 obj\n{text}\nendobj\n").as_bytes());
    }
    for p in packed {
        let (mut pairs, mut body) = (String::new(), String::new());
        for (i, &(n, text)) in p.members.iter().enumerate() {
            pairs.push_str(&format!("{n} {} ", body.len()));
            body.push_str(text);
            body.push('\n');
            rows.insert(n, (2, p.number as usize, u16::try_from(i).unwrap()));
        }
        let data = format!("{pairs}\n{body}");
        let length = match p.entries.contains("/Length") {
            true => String::new(),
            false => format!(" /Length {}", data.len()),
        };
        rows.insert(p.number, (1, file.len(), 0));
        let dict = format!(
            "<< /Type /ObjStm /N {} /First {} {}{length} >>",
            p.members.len(),
            pairs.len() + 1,
            p.entries,
        );
        file.extend_from_slice(
            format!(
                "{} 0 obj\n{dict}\nstream\n{data}\nendstream\nendobj\n",
                p.number
            )
            .as_bytes(),
        );
    }
    let own = rows.keys().max().map_or(1, |n| n + 1);
    let xref = file.len();
    rows.insert(own, (1, xref, 0));
    // A file's first section lists object 0, free; an update's, what it
    // writes alone.
    if rfind(start, b"startxref").is_none() {
        rows.insert(0, (0, 0, 65535));
    }
    let mut index = Vec::new();
    let mut data = Vec::new();
    for (&n, &(kind, field, generation)) in &rows {
        match index.last_mut() {
            Some((first, count)) if *first + *count == n => *count += 1,
            _ => index.push((n, 1)),
        }
        data.push(kind);
        data.extend_from_slice(&u32::try_from(field).unwrap().to_be_bytes());
        data.extend_from_slice(&generation.to_be_bytes());
    }
    let index: Vec<String> = (index.iter())
        .map(|(n, count)| format!("{n} {count}"))
        .collect();
    let index = format!("/Index [{}]", index.join(" "));
    let length = length.map_or_else(|| format!("/Length {}", data.len()), str::to_string);
    let dict = format!(
        "<< /Type /XRef /Size {} /W [1 4 2] {index} {trailer} {length} >>",
        own + 1
    );
    file.extend_from_slice(format!("{own} 0 obj\n{dict}\nstream\n").as_bytes());
    file.extend_from_slice(&data);
    file.extend_from_slice(format!("endstream\nendobj\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    file
}

/// [`appended`] to a header alone, the trailer naming catalog 1.
fn packed(objects: &[(u32, &str)], packed: &[Packed], length: Option<&str>) -> Vec<u8> {
    appended(b"%PDF-1.7\n", objects, packed, "/Root 1 0 R", length)
}

/// A catalog, its page tree and a page, 1 to 3, whose content is stream 4.
fn page() -> Vec<(u32, &'static str)> {
    vec![
        (1, "<< /Type /Catalog /Pages 2 0 R >>"),
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        (
            3,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>",
        ),
    ]
}

/// A stream whose `/Length` is `length`, its data [`CONTENT`] and an end
/// of line.
fn content(length: &str) -> String {
    format!("<< /Length {length} >>\nstream\n{CONTENT}\nendstream")
}

/// The file of a page whose content stream's `/Length` is object 5, in
/// object stream 6.
pub(crate) fn a_length_in_an_object_stream() -> Vec<u8> {
    let length = CONTENT.len().to_string();
    let members = [(5, length.as_str())];
    let stream = Packed {
        number: 6,
        entries: "",
        members: &members,
    };
    let content = content("5 0 R");
    packed(&[page(), vec![(4, &content)]].concat(), &[stream], None)
}

/// The file of two object streams, 6 holding the page tree and 7 the page,
/// each one's `/Length` an object in the other's: 6's object 8, in 7, and
/// 7's object 9, in 6.
pub(super) fn object_streams_whose_lengths_are_in_each_other() -> Vec<u8> {
    let objects = page();
    let pages = [(2, objects[1].1), (9, "0")];
    let page = [(3, objects[2].1), (8, "0")];
    let streams = [
        Packed {
            number: 6,
            entries: "/Length 8 0 R",
            members: &pages,
        },
        Packed {
            number: 7,
            entries: "/Length 9 0 R",
            members: &page,
        },
    ];
    let content = (4, "<< /Length 0 >>\nstream\n\nendstream");
    packed(&[objects[0], content], &streams, None)
}

/// The file of a page whose content stream's `/Length` is object 5, in
/// object stream 6, whose `/DecodeParms` is object 7: a stream whose
/// `/Length` is object 5 too.
pub(super) fn a_length_in_an_object_stream_that_refers() -> Vec<u8> {
    let length = CONTENT.len().to_string();
    let members = [(5, length.as_str())];
    let stream = Packed {
        number: 6,
        entries: "/DecodeParms 7 0 R",
        members: &members,
    };
    let content = content("5 0 R");
    let objects = [page(), vec![(4, &content), (7, &content)]].concat();
    packed(&objects, &[stream], None)
}

/// The file of [`a_length_in_an_object_stream`], its cross-reference
/// stream's `/Length` object 5 too, which holds the stream's length
/// rather than the content's: a cross-reference stream's entries are
/// direct (ISO 32000-2, 7.5.8.2), and the open reads its rows before it
/// lists any object stream.
pub(super) fn a_cross_reference_streams_length_in_an_object_stream() -> Vec<u8> {
    let length = CONTENT.len().to_string();
    let members = [(5, length.as_str())];
    let stream = Packed {
        number: 6,
        entries: "",
        members: &members,
    };
    let content = content(&length);
    packed(
        &[page(), vec![(4, &content)]].concat(),
        &[stream],
        Some("/Length 5 0 R"),
    )
}

/// A page whose cross-reference stream's `/Length` is object 5, at an
/// offset, which holds the stream's length, its seven rows': the open
/// reads the stream's rows before it lists any object, so a reference
/// there is followed by no read, its entries being direct (ISO 32000-2,
/// 7.5.8.2).
pub(super) fn a_cross_reference_streams_length_by_reference() -> Vec<u8> {
    let length = CONTENT.len().to_string();
    let content = content(&length);
    let rows = (7 * 7).to_string();
    packed(
        &[page(), vec![(4, &content), (5, &rows)]].concat(),
        &[],
        Some("/Length 5 0 R"),
    )
}

/// An AES-256 file, opened by an empty user's password, updated so that
/// its encryption dictionary, 6, refers to stream 9 for its
/// `/EncryptMetadata`, whose `/Length` is object 10, in object stream 11,
/// whose data is in the clear (`/Identity`): the open reads 9 for the
/// dictionary's values before it has a handler to decrypt an object
/// stream by.
pub(super) fn an_encryption_dictionarys_stream_with_its_length_in_an_object_stream() -> Vec<u8> {
    let file = fixture::encrypted(&settings(Kind::Aes256, b""), &to_encrypt(&[]), root());
    let start = find(&file, b"6 0 obj").unwrap();
    let end = start + find(&file[start..], b"endobj").unwrap();
    let own = String::from_utf8_lossy(&file[start + 7..end]).into_owned();
    assert!(own.contains("/EncryptMetadata true"), "{own}");
    let own = own.replacen("/EncryptMetadata true", "/EncryptMetadata 9 0 R", 1);
    let prev = startxref(&file);
    let content = content("10 0 R");
    let length = CONTENT.len().to_string();
    let members = [(10, length.as_str())];
    let stream = Packed {
        number: 11,
        entries: "/Filter /Crypt /DecodeParms << /Name /Identity >>",
        members: &members,
    };
    let trailer = format!(
        "/Root 1 0 R /Encrypt 6 0 R /ID [(0123456789abcdef) (0123456789abcdef)] /Prev {prev}"
    );
    appended(
        &file,
        &[(6, &own), (9, &content)],
        &[stream],
        &trailer,
        None,
    )
}

/// The offset `startxref` gives.
fn startxref(file: &[u8]) -> usize {
    let at = rfind(file, b"startxref").unwrap() + b"startxref".len();
    let digits: String = (file[at..].iter())
        .skip_while(|b| b.is_ascii_whitespace())
        .take_while(|b| b.is_ascii_digit())
        .map(|&b| char::from(b))
        .collect();
    digits.parse().unwrap()
}

/// `file`, by `passwords`, read every way the tests read: whole, on demand
/// by number and from the last back, and by position in first windows of
/// 1, 7, 64 and 4096 bytes, an object's reaching the next listed offset and
/// not, on demand from the last back: each its name and its document,
/// which a test holds to the whole read's.
pub(super) fn every_way(file: &[u8], passwords: &[Password]) -> Vec<(String, Document)> {
    let options = Options {
        passwords,
        ..Options::DEFAULT
    };
    let mut readings = vec![("whole".to_string(), document(file, &options))];
    for back in [false, true] {
        let reading = Reading::open(file, &options).unwrap();
        let mut ids: Vec<ObjectId> = reading.ids().collect();
        if back {
            ids.reverse();
        }
        ids.iter().for_each(|&id| drop(reading.get(id)));
        let name = format!("on demand, back {back}");
        readings.push((name, reading.into_document().unwrap()));
    }
    for first in [1, 7, 64, 4096] {
        for bound in [true, false] {
            let source = Counted::new(file, first);
            let reading = open(&source, passwords, &Options::DEFAULT, bound).unwrap();
            let ids: Vec<ObjectId> = reading.ids().collect();
            ids.iter().rev().for_each(|&id| drop(reading.get(id)));
            let name = format!("by position, {first} bytes, {bound}");
            readings.push((name, reading.into_document().unwrap()));
        }
    }
    readings
}

/// Each of `file`'s readings, every way ([`every_way`]), its whole read's.
pub(super) fn alike(file: &[u8], passwords: &[Password]) -> Document {
    let mut readings = every_way(file, passwords).into_iter();
    let (_, whole) = readings.next().unwrap();
    for (name, doc) in readings {
        assert_eq!(doc, whole, "{name}");
    }
    whole
}

/// Stream `n`'s data in `file`, as `doc` read it, and whether its
/// `/Length` ended it.
pub(super) fn data(doc: &Document, file: &[u8], n: u32) -> (String, bool) {
    let id = ObjectId::new(n, 0);
    let extent = doc.record(id).unwrap().data.unwrap();
    let at = extent.at;
    let data = String::from_utf8_lossy(&file[at.start..at.end]).into_owned();
    (data, extent.declared)
}

/// The fault that stream `n`'s `/Length` doesn't read, at its value, as
/// `doc` holds the stream.
pub(super) fn no_length(doc: &Document, n: u32) -> (Place, Fault) {
    let id = ObjectId::new(n, 0);
    let dict = doc.get(id).unwrap().as_dict().unwrap();
    let at = dict.get(b"Length").unwrap().at;
    (Place::new(Some(id), at), Fault::Length { length: None })
}

/// The fault that cross-reference stream `n`'s `/Length` is a reference,
/// which isn't followed, at its value, as `doc` holds the stream.
pub(super) fn unfollowed(doc: &Document, n: u32) -> (Place, Fault) {
    let (place, _) = no_length(doc, n);
    (place, Fault::XrefLength)
}

/// The faults on stream `n`, its data [`CONTENT`] found by the first
/// `endstream` in it: its `/Length` doesn't read, and past that
/// `endstream` is no `endobj`.
pub(super) fn scanned(doc: &Document, n: u32) -> [(Place, Fault); 2] {
    let id = ObjectId::new(n, 0);
    let at = doc.get(id).unwrap().at;
    let unended = Fault::Parse {
        kind: ParseKind::NoEndobj,
        text: format!("{n} 0 obj has no endobj").into(),
    };
    [no_length(doc, n), (Place::new(Some(id), at), unended)]
}

/// A content stream whose `/Length` is an object in an object stream is
/// read by it, every way, from the object stream's decoded data: its data
/// all of [`CONTENT`], ended by its `/Length`, and no fault; read on
/// demand, the stream before its length's object or after it, alike.
#[test]
fn a_streams_length_in_an_object_stream_is_read_from_it() {
    let file = a_length_in_an_object_stream();
    let doc = alike(&file, &[]);
    assert_eq!(doc.faults(), [], "{:?}", doc.faults());
    assert_eq!(data(&doc, &file, 4), (CONTENT.to_string(), true));
    let record = doc.record(ObjectId::new(4, 0)).unwrap();
    assert!(record.data.unwrap().indirect_length);
    assert_eq!(
        doc.record(ObjectId::new(5, 0)).unwrap().origin,
        Origin::Stream {
            stream: 6,
            index: 0
        }
    );
    // The stream alone, its length's object unread: the object stream
    // decoded on the way, and kept.
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    reading.get(ObjectId::new(4, 0)).unwrap();
    assert_eq!(reading.faults(), []);
    let record = reading.record(ObjectId::new(4, 0)).unwrap();
    assert!(record.data.unwrap().declared);
    assert!(reading.record(ObjectId::new(5, 0)).is_none());
    assert_eq!(reading.into_document().unwrap(), doc);
}

/// An object stream whose own `/Length` is an object in an object stream,
/// which ISO 32000-2, 7.5.7 keeps out of one, has no `/Length` that
/// reads: its data found by `endstream`, and said; and two whose lengths
/// are each in the other read so, every way, the one asked for first
/// either, without a read coming back to itself. Their members read.
#[test]
fn object_streams_whose_lengths_are_in_object_streams_have_none_that_reads() {
    let file = object_streams_whose_lengths_are_in_each_other();
    let doc = alike(&file, &[]);
    assert_eq!(doc.faults(), [no_length(&doc, 6), no_length(&doc, 7)]);
    for n in [6, 7] {
        assert!(!data(&doc, &file, n).1, "{n}");
    }
    for n in [2, 3, 8, 9] {
        assert!(doc.get(ObjectId::new(n, 0)).is_ok(), "{n}");
    }
    // Asked for first, each object stream's member, or the other's.
    for first in [2, 3, 8, 9] {
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        reading.get(ObjectId::new(first, 0)).unwrap();
        assert_eq!(reading.into_document().unwrap(), doc, "{first} first");
    }
}

/// A stream's `/Length` in an object stream whose decode reads another
/// object, its `/DecodeParms` referring to one, isn't read from it: here
/// the other is a stream whose `/Length` is the same object, which would
/// read its own read again. Both streams have no `/Length` that reads, and
/// say so, the object stream decoded first or not; the object itself reads.
#[test]
fn a_length_isnt_read_from_an_object_stream_whose_decode_reads_another_object() {
    let file = a_length_in_an_object_stream_that_refers();
    let doc = alike(&file, &[]);
    assert_eq!(doc.faults(), [scanned(&doc, 4), scanned(&doc, 7)].concat());
    for n in [4, 7] {
        assert_eq!(data(&doc, &file, n), (SCANNED.to_string(), false), "{n}");
    }
    assert_eq!(doc.get(ObjectId::new(5, 0)).unwrap().as_integer(), Some(44));
    for first in [4, 5, 7] {
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        reading.get(ObjectId::new(first, 0)).unwrap();
        assert_eq!(reading.into_document().unwrap(), doc, "{first} first");
    }
}

/// A file's cross-reference stream whose `/Length` is an object in an
/// object stream, as written ([`Reading::sections`]): its rows are the
/// ones the reading read, before an object in the object stream is read
/// and after, by `endstream`.
#[test]
fn a_sections_rows_by_a_length_in_an_object_stream_are_the_readings() {
    let file = a_cross_reference_streams_length_in_an_object_stream();
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    let before = reading.sections().unwrap();
    assert_eq!(before.stopped, None);
    reading.get(ObjectId::new(5, 0)).unwrap();
    reading.get(ObjectId::new(7, 0)).unwrap();
    let after = reading.sections().unwrap();
    assert_eq!(before, after);
    let [section] = &after.sections[..] else {
        panic!("{after:?}");
    };
    let listed: Vec<u32> = reading.entries().map(|(n, ..)| n).collect();
    let rows: Vec<u32> = (section.rows.iter()).map(|row| row.number).collect();
    assert_eq!(rows, listed);
}

/// A `/Length` that refers to an object that's itself a reference isn't
/// followed further, at an offset or in an object stream, either way
/// round: the stream has no `/Length` that reads, and says so, its data
/// found by `endstream`. qpdf reads such an object as the number of the
/// reference, a fault it says, and finds the data by scanning; PDFium
/// takes no length from a reference's object that's a reference.
#[test]
fn a_chain_of_length_references_isnt_followed() {
    let length = CONTENT.len().to_string();
    let content = content("5 0 R");
    let objects = [page(), vec![(4, content.as_str())]].concat();
    let table = |more: &[(u32, &str)]| {
        let mut all: Vec<(u32, &[u8])> = (objects.iter())
            .chain(more)
            .map(|&(n, text)| (n, text.as_bytes()))
            .collect();
        all.sort_unstable_by_key(|&(n, _)| n);
        fixture::assembled("1.7", &all, "<< /Size 7 /Root 1 0 R >>")
    };
    let member = |n, text| [(n, text)];
    let a_member = member(5, "8 0 R");
    let a_members_length = member(7, length.as_str());
    let files = [
        ("at offsets", table(&[(5, "6 0 R"), (6, &length)])),
        (
            "to an object in an object stream",
            packed(
                &[objects.clone(), vec![(5, "7 0 R")]].concat(),
                &[Packed {
                    number: 6,
                    entries: "",
                    members: &a_members_length,
                }],
                None,
            ),
        ),
        (
            "from an object in an object stream",
            packed(
                &[objects.clone(), vec![(8, length.as_str())]].concat(),
                &[Packed {
                    number: 6,
                    entries: "",
                    members: &a_member,
                }],
                None,
            ),
        ),
    ];
    for (what, file) in files {
        let doc = alike(&file, &[]);
        assert_eq!(doc.faults(), scanned(&doc, 4), "{what}");
        assert_eq!(data(&doc, &file, 4), (SCANNED.to_string(), false), "{what}");
    }
}

/// [`a_length_in_an_object_stream`] scanned, its `startxref` giving no
/// section, and the content stream written again after the cross-reference
/// stream, so that the open's search for a trailer from the file's end
/// reads it first, before the object stream's members are listed: every
/// read after the open reads its `/Length` from the object stream, every
/// way alike, and nothing the open's read of it told is said.
#[test]
fn a_scanned_files_stream_whose_length_is_in_an_object_stream_is_read_by_it() {
    let file = a_length_in_an_object_stream();
    let at = rfind(&file, b"startxref").unwrap();
    let content = format!("4 0 obj\n{}\nendobj\n", content("5 0 R"));
    let file = [&file[..at], content.as_bytes(), b"startxref\n9\n%%EOF\n"].concat();
    let doc = alike(&file, &[]);
    assert!(doc.trailer.unwritten.is_some());
    assert_eq!(doc.trailer.stream, Some(ObjectId::new(7, 0)));
    let members = Fault::Members {
        objects: 1,
        streams: 1,
        unread: 0,
    };
    assert_eq!(doc.faults(), [(Place::new(None, None), members)]);
    assert_eq!(data(&doc, &file, 4), (CONTENT.to_string(), true));
    let record = doc.record(ObjectId::new(4, 0)).unwrap();
    assert_eq!(record.at.start, at);
    assert!(record.data.unwrap().indirect_length);
    // Read alone, the stream says nothing either.
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    assert_eq!(reading.faults(), doc.faults());
    reading.get(ObjectId::new(4, 0)).unwrap();
    assert_eq!(reading.faults(), doc.faults());
}

/// A file's first object, a stream whose `/Length` is an object in an
/// object stream, as written ([`Reading::first_object`]): read as the open
/// reads, its `/Length` not read from the object stream, its data found by
/// `endstream`, before the object stream is decoded and after.
#[test]
fn a_first_objects_length_in_an_object_stream_isnt_read() {
    let length = CONTENT.len().to_string();
    let members = [(5, length.as_str())];
    let stream = Packed {
        number: 6,
        entries: "",
        members: &members,
    };
    let content = content("5 0 R");
    let file = packed(
        &[vec![(4, content.as_str())], page()].concat(),
        &[stream],
        None,
    );
    let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
    let data = |first: FirstObject| {
        let at = first.object.as_stream().unwrap().data.at().unwrap();
        String::from_utf8_lossy(&file[at.start..at.end]).into_owned()
    };
    let before = reading.first_object().unwrap().unwrap();
    assert_eq!(before.id, ObjectId::new(4, 0));
    assert_eq!(data(before.clone()), SCANNED);
    reading.get(ObjectId::new(4, 0)).unwrap();
    assert!(
        reading
            .record(ObjectId::new(4, 0))
            .unwrap()
            .data
            .unwrap()
            .declared
    );
    let after = reading.first_object().unwrap().unwrap();
    assert_eq!(after, before);
}
