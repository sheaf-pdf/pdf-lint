//! What the writer writes, read back by the reader: cross-reference
//! sections that read as sound, tables and streams, a full write's and an
//! update's, placed past any bytes before the header; and object streams,
//! their members read back through the sections that list them.

use std::sync::Arc;

use pdf_filters::{Backend, Filter, Level, Predictor};
use pdf_objects::dict;
use pdf_objects::object::{Dict, Form, Object, ObjectId, Str, Stream, Value};
use pdf_objects::serialize;

use super::{Deflate, Entry, Error, Section};
use crate::XrefKind;
use crate::bytes::{Limits, Options, Reading, XrefEntry};
use crate::write;

fn open(bytes: &[u8]) -> Reading<Arc<[u8]>> {
    Reading::open(Arc::<[u8]>::from(bytes), &Options::DEFAULT).unwrap()
}

/// Whether `pdf` was read by its cross-reference sections, with nothing to
/// warn of.
fn sound(pdf: &Reading<Arc<[u8]>>) -> bool {
    pdf.trailer().unwritten.is_none() && pdf.warnings().is_empty()
}

/// `pdf`'s trailer's `/Size`.
fn size(pdf: &Reading<Arc<[u8]>>) -> u32 {
    let size = pdf
        .trailer()
        .dict()
        .get(b"Size")
        .and_then(Object::as_integer);
    u32::try_from(size.unwrap()).unwrap()
}

/// Where `pdf`'s newest section starts, and its kind.
fn newest(pdf: &Reading<Arc<[u8]>>) -> (usize, XrefKind) {
    pdf.facts().newest.unwrap()
}

/// `o` as built: every place it was written left out, a dictionary's keys'
/// too, its strings' forms kept.
fn unplaced(o: &Object) -> Object {
    let mut o = o.clone();
    let mut stack = vec![&mut o];
    while let Some(o) = stack.pop() {
        match o.value_mut() {
            Value::Array(a) => stack.extend(a.iter_mut()),
            Value::Dict(d) => {
                for entry in d.entries_mut() {
                    entry.key_at = None;
                    stack.push(&mut entry.value);
                }
            }
            Value::Stream(s) => {
                for entry in s.dict.entries_mut() {
                    entry.key_at = None;
                    stack.push(&mut entry.value);
                }
            }
            _ => {}
        }
    }
    o
}

/// Object `id` of `pdf`, as built.
fn value(pdf: &Reading<Arc<[u8]>>, id: ObjectId) -> Object {
    unplaced(pdf.get(id).unwrap())
}

/// `stream`'s data, inflated.
fn inflated(stream: &Stream) -> Vec<u8> {
    let flate = Filter::Flate {
        predictor: Predictor::None,
    };
    let data = stream.data.bytes().unwrap();
    pdf_filters::decode(data, &[flate], usize::MAX, Backend::MinizOxide).unwrap()
}

fn catalog() -> Object {
    Object::from(dict! { "Type" => "Catalog" })
}

/// A file the writer wrote: a catalog as object 1, then `objects`, and a
/// full table.
fn file(objects: &[(u32, Object)]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut section = Section::full(0);
    for (num, obj) in std::iter::once(&(1, catalog())).chain(objects) {
        let pos = serialize::indirect(&mut out, (*num, 0), obj).unwrap();
        section.insert(*num, Entry::InUse { pos, generation: 0 });
    }
    let pos = out.len();
    section
        .table(&mut out, dict! { "Root" => (1, 0) }, pos)
        .unwrap();
    out
}

/// A trailer that can't be written, as it holds a real that isn't a finite
/// number, is refused, and nothing is written: by a table, and by a
/// stream, whose dictionary it is.
#[test]
fn nan_and_infinities_in_a_trailer_are_refused() {
    for x in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut out = b"%PDF-1.7\n".to_vec();
        let trailer = dict! { "Root" => (1, 0), "Odd" => x };
        assert_eq!(
            Section::full(2).table(&mut out, trailer.clone(), 9),
            Err(Error::Serialize(serialize::Error::NonFinite { id: None }))
        );
        assert_eq!(
            Section::full(2).stream(&mut out, 2, trailer, 9, Deflate::DEFAULT),
            Err(Error::Serialize(serialize::Error::NonFinite {
                id: Some((2, 0))
            }))
        );
        assert_eq!(out, b"%PDF-1.7\n", "nothing written");
    }
    assert_eq!(
        Error::Serialize(serialize::Error::NonFinite { id: None }).to_string(),
        "a real that isn't a finite number"
    );
}

#[test]
fn a_full_table_links_its_free_entries() {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut section = Section::full(4);
    let one = serialize::indirect(&mut out, (1, 0), &catalog()).unwrap();
    section.insert(
        1,
        Entry::InUse {
            pos: one,
            generation: 0,
        },
    );
    let three = serialize::indirect(&mut out, (3, 2), &Object::from(3)).unwrap();
    section.insert(
        3,
        Entry::InUse {
            pos: three,
            generation: 2,
        },
    );
    // 2 was deleted at generation 4, 4 was never used, and 5, deleted,
    // grows the section past the size it was given.
    section.insert(2, Entry::Free { generation: 5 });
    section.insert(5, Entry::Free { generation: 1 });
    let pos = out.len();
    section
        .table(&mut out, dict! { "Root" => (1, 0) }, pos)
        .unwrap();
    let want = format!(
        "xref\n0 6\n\
         0000000002 65535 f \n\
         {one:010} 00000 n \n\
         0000000004 00005 f \n\
         {three:010} 00002 n \n\
         0000000005 00000 f \n\
         0000000000 00001 f \n\
         trailer\n<</Root 1 0 R/Size 6>>\nstartxref\n{pos}\n%%EOF\n"
    );
    assert_eq!(String::from_utf8_lossy(&out[pos..]), want);
    let pdf = open(&out);
    assert!(sound(&pdf));
    assert_eq!(newest(&pdf).1, XrefKind::Table);
    assert_eq!(size(&pdf), 6);
    assert_eq!(pdf.ids().collect::<Vec<_>>(), [(1, 0), (3, 2)]);
    assert_eq!(value(&pdf, (3, 2)), Object::from(3));
}

#[test]
fn appended_runs_free_deleted_objects_with_the_next_generation() {
    let mut out = file(&[(2, Object::from(2)), (3, Object::from(3))]);
    let base = open(&out);
    let prev = newest(&base).0;
    let mut section = Section::append(size(&base), 0);
    let marked = Object::from(dict! { "Type" => "Catalog", "Marked" => true });
    let one = serialize::indirect(&mut out, (1, 0), &marked).unwrap();
    section.insert(
        1,
        Entry::InUse {
            pos: one,
            generation: 0,
        },
    );
    section.insert(3, Entry::Free { generation: 1 });
    let five = serialize::indirect(&mut out, (5, 0), &Object::from(5)).unwrap();
    section.insert(
        5,
        Entry::InUse {
            pos: five,
            generation: 0,
        },
    );
    let pos = out.len();
    let trailer = dict! { "Root" => (1, 0), "Prev" => prev as i64 };
    section.table(&mut out, trailer, pos).unwrap();
    let want = format!(
        "xref\n0 2\n\
         0000000003 65535 f \n\
         {one:010} 00000 n \n\
         3 1\n\
         0000000000 00001 f \n\
         5 1\n\
         {five:010} 00000 n \n\
         trailer\n<</Root 1 0 R/Prev {prev}/Size 6>>\nstartxref\n{pos}\n%%EOF\n"
    );
    assert_eq!(String::from_utf8_lossy(&out[pos..]), want);
    // The reader takes object 3 out, as PDFium and qpdf do: the newest
    // section frees it.
    let pdf = open(&out);
    assert!(sound(&pdf));
    assert_eq!(pdf.facts().sections, 2);
    assert_eq!(size(&pdf), 6);
    assert_eq!(value(&pdf, (1, 0)), marked);
    assert_eq!(value(&pdf, (2, 0)), Object::from(2));
    assert!(pdf.get((3, 0)).is_err());
    assert_eq!(value(&pdf, (5, 0)), Object::from(5));
}

/// Read from its header, as PDFium, pdf.js and qpdf read positions: the
/// reader counts them from the file's start (ISO 32000-2, 7.5.4), and
/// finds the objects of a file with bytes before its header by scanning.
#[test]
fn positions_count_from_the_header() {
    let mut out = vec![b'~'; 100];
    out.extend(file(&[(2, Object::from(2))]));
    assert_eq!(open(&out).facts().header_offset, Some(100));
    let base = open(&out[100..]);
    let mut section = Section::append(size(&base), 100);
    let pos = serialize::indirect(&mut out, (2, 0), &Object::from(22)).unwrap();
    section.insert(2, Entry::InUse { pos, generation: 0 });
    let pos = out.len();
    let trailer = dict! { "Root" => (1, 0), "Prev" => newest(&base).0 as i64 };
    section.table(&mut out, trailer, pos).unwrap();
    assert!(out.ends_with(format!("startxref\n{}\n%%EOF\n", pos - 100).as_bytes()));
    let pdf = open(&out[100..]);
    assert!(sound(&pdf));
    assert_eq!(value(&pdf, (2, 0)), Object::from(22));
    let scanned = open(&out);
    assert!(scanned.trailer().unwritten.is_some());
    assert_eq!(value(&scanned, (2, 0)), Object::from(22));
}

#[test]
fn a_table_refuses_offsets_past_ten_digits() {
    let far = 10_000_000_000;
    let mut section = Section::full(0);
    section.insert(
        1,
        Entry::InUse {
            pos: far,
            generation: 0,
        },
    );
    let mut out = b"%PDF-1.7\n".to_vec();
    assert_eq!(
        section.table(&mut out, Dict::new(), far + 10),
        Err(Error::TooFar(far as u64))
    );
    assert_eq!(out, b"%PDF-1.7\n", "nothing written");
}

/// Cross-reference stream `id`'s `/Index` and its rows, each `[type,
/// field, generation]`, read by its `/W`. A full write's, which has no
/// `/Prev`, is deflated under the PNG Up predictor, a column for each
/// byte of a row; an append's is uncompressed.
fn rows(pdf: &Reading<Arc<[u8]>>, id: ObjectId) -> (Vec<i64>, Vec<i64>, Vec<[u64; 3]>) {
    let s = pdf.get(id).unwrap().as_stream().unwrap();
    assert_eq!(s.dict.type_name(), Some(&b"XRef"[..]));
    let ints = |key: &[u8]| -> Vec<i64> {
        let array = s.dict.get(key).unwrap().as_array().unwrap();
        array.iter().map(|o| o.as_integer().unwrap()).collect()
    };
    let (index, w) = (ints(b"Index"), ints(b"W"));
    let widths: Vec<usize> = w.iter().map(|&n| n as usize).collect();
    let columns: usize = widths.iter().sum();
    if s.dict.has(b"Prev") {
        assert!(!s.dict.has(b"Filter"), "an update's is uncompressed");
        assert!(!s.dict.has(b"DecodeParms"), "an update's has no predictor");
    } else {
        let flate = s.dict.get(b"Filter").and_then(Object::as_name);
        assert_eq!(flate, Some(&b"FlateDecode"[..]));
        let parms = s.dict.get(b"DecodeParms").unwrap().as_dict().unwrap();
        assert_eq!(
            parms.get(b"Predictor").and_then(Object::as_integer),
            Some(12)
        );
        assert_eq!(
            parms.get(b"Columns").and_then(Object::as_integer),
            Some(columns as i64)
        );
    }
    let rows = pdf
        .decoded(id)
        .unwrap()
        .chunks(columns)
        .map(|row| {
            let mut fields = [0; 3];
            let mut at = 0;
            for (field, width) in fields.iter_mut().zip(&widths) {
                *field = row[at..at + width]
                    .iter()
                    .fold(0, |n, &b| n << 8 | u64::from(b));
                at += width;
            }
            fields
        })
        .collect();
    (index, w, rows)
}

#[test]
fn an_xref_stream_lists_its_index() {
    // A full write: three objects, and the stream as 4.
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut section = Section::full(0);
    let mut at = Vec::new();
    for (num, obj) in [(1, catalog()), (2, Object::from(2)), (3, Object::from(3))] {
        let pos = serialize::indirect(&mut out, (num, 0), &obj).unwrap();
        section.insert(num, Entry::InUse { pos, generation: 0 });
        at.push(pos as u64);
    }
    let pos = out.len();
    section
        .stream(
            &mut out,
            4,
            dict! { "Root" => (1, 0) },
            pos,
            Deflate::DEFAULT,
        )
        .unwrap();
    assert!(out.ends_with(format!("endobj\nstartxref\n{pos}\n%%EOF\n").as_bytes()));
    let base = open(&out);
    assert!(sound(&base));
    assert_eq!(newest(&base).1, XrefKind::Stream);
    assert_eq!(size(&base), 5);
    let (index, w, got) = rows(&base, (4, 0));
    assert_eq!(index, [0, 5]);
    // As wide as the largest offset, the stream's own, needs.
    assert_eq!(w, [1, if pos < 256 { 1 } else { 2 }, 2]);
    let want = [
        [0, 0, 65535],
        [1, at[0], 0],
        [1, at[1], 0],
        [1, at[2], 0],
        [1, pos as u64, 0],
    ];
    assert_eq!(got, want);

    // An append: 2 rewritten, 3 deleted, and the stream as 5.
    let mut section = Section::append(size(&base), 0);
    let two = serialize::indirect(&mut out, (2, 0), &Object::from(22)).unwrap();
    section.insert(
        2,
        Entry::InUse {
            pos: two,
            generation: 0,
        },
    );
    section.insert(3, Entry::Free { generation: 1 });
    let pos = out.len();
    let trailer = dict! { "Root" => (1, 0), "Prev" => newest(&base).0 as i64 };
    section
        .stream(&mut out, 5, trailer, pos, Deflate::DEFAULT)
        .unwrap();
    let pdf = open(&out);
    assert!(sound(&pdf));
    assert_eq!(newest(&pdf).1, XrefKind::Stream);
    assert_eq!(pdf.facts().sections, 2);
    assert_eq!(size(&pdf), 6);
    let (index, _, got) = rows(&pdf, (5, 0));
    assert_eq!(index, [0, 1, 2, 2, 5, 1]);
    let want = [
        [0, 3, 65535],
        [1, two as u64, 0],
        [0, 0, 1],
        [1, pos as u64, 0],
    ];
    assert_eq!(got, want);
    assert_eq!(value(&pdf, (2, 0)), Object::from(22));
}

/// A full write's cross-reference stream is deflated under the PNG Up
/// predictor, into fewer bytes than its rows, and the reader reads it
/// back.
#[test]
fn a_full_xref_stream_is_compressed_with_the_up_predictor() {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut section = Section::full(0);
    let mut at = Vec::new();
    for num in 1..=200 {
        let obj = if num == 1 {
            catalog()
        } else {
            Object::from(num)
        };
        let pos = serialize::indirect(&mut out, (num, 0), &obj).unwrap();
        section.insert(num, Entry::InUse { pos, generation: 0 });
        at.push(pos as u64);
    }
    let pos = out.len();
    section
        .stream(
            &mut out,
            201,
            dict! { "Root" => (1, 0) },
            pos,
            Deflate::DEFAULT,
        )
        .unwrap();
    let pdf = open(&out);
    assert!(sound(&pdf));
    assert_eq!(size(&pdf), 202);
    let (index, w, got) = rows(&pdf, (201, 0));
    assert_eq!(index, [0, 202]);
    assert_eq!(w, [1, 2, 2]);
    let want: Vec<[u64; 3]> = std::iter::once([0, 0, 65535])
        .chain(at.iter().map(|&pos| [1, pos, 0]))
        .chain([[1, pos as u64, 0]])
        .collect();
    assert_eq!(got, want);
    let stored = pdf.data((201, 0)).unwrap();
    assert!(
        stored.len() < got.len() * 5,
        "{} bytes for {} rows of 5",
        stored.len(),
        got.len()
    );
    for num in 2..=200 {
        assert_eq!(value(&pdf, (num, 0)), Object::from(num));
    }
}

/// `members`, each its number and its object, as direct syntax.
fn as_members(members: &[(u32, Object)]) -> Vec<(u32, Vec<u8>)> {
    (members.iter())
        .map(|(num, obj)| {
            let mut bytes = Vec::new();
            serialize::write(&mut bytes, obj).unwrap();
            (*num, bytes)
        })
        .collect()
}

/// An object stream holding `members`, in order.
fn container(members: &[(u32, Object)]) -> Object {
    let stream = write::container(&as_members(members), Deflate::DEFAULT).unwrap();
    Object::from(stream.unwrap())
}

/// A file of a catalog, `container` as object `num` and a cross-reference
/// stream listing each of `members` in it, in order: for the reader to
/// read them from it.
fn packed(num: u32, container: &Stream, members: &[u32], deflate: Deflate) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut section = Section::full(0);
    let container = Object::from(container.clone());
    for (n, obj) in [(1, catalog()), (num, container)] {
        let pos = serialize::indirect(&mut out, (n, 0), &obj).unwrap();
        section.insert(n, Entry::InUse { pos, generation: 0 });
    }
    for (index, &n) in (0..).zip(members) {
        section.insert(
            n,
            Entry::Packed {
                container: num,
                index,
            },
        );
    }
    let pos = out.len();
    let own = section.size();
    let trailer = dict! { "Root" => (1, 0) };
    section
        .stream(&mut out, own, trailer, pos, deflate)
        .unwrap();
    out
}

/// Every exclusion of 7.5.7, and a signature's: streams (a
/// cross-reference stream's among them), a generation past 0, an
/// encryption dictionary, the linearization dictionary and a signature
/// dictionary, told after PDFium's tell. Everything else packs, a
/// signature field's widget with it: its `/Type /Annot` comes before its
/// `/FT /Sig`.
#[test]
fn packable_leaves_out_what_iso_7_5_7_does() {
    let dict = |d: Dict| Object::from(d);
    let stream = |d: Dict| Object::from(Stream::new(d, b"data".to_vec()));
    let bytes = |b: &[u8]| Object::hex(b);
    let never = [
        ("a stream", stream(dict! {})),
        (
            "a cross-reference stream",
            stream(dict! { "Type" => "XRef" }),
        ),
        ("an object stream", stream(dict! { "Type" => "ObjStm" })),
        (
            "an encryption dictionary",
            dict(dict! {
                "Filter" => "Standard",
                "V" => 2,
                "O" => bytes(b"o"),
                "U" => bytes(b"u"),
            }),
        ),
        (
            "the linearization dictionary",
            dict(dict! { "Linearized" => 1, "L" => 1000 }),
        ),
        (
            "a signature",
            dict(dict! { "Type" => "Sig", "Filter" => "Adobe.PPKLite" }),
        ),
        ("a time stamp", dict(dict! { "Type" => "DocTimeStamp" })),
        ("a signature by /FT", dict(dict! { "FT" => "Sig" })),
        (
            "a signature without /Type",
            dict(dict! {
                "ByteRange" => vec![Object::from(0), Object::from(10)],
                "Contents" => bytes(b"sig"),
            }),
        ),
    ];
    for (what, obj) in &never {
        assert!(!write::packable((5, 0), obj), "{what}");
    }
    let catalog = catalog();
    assert!(write::packable((5, 0), &catalog));
    assert!(!write::packable((5, 1), &catalog), "generation 1");
    let packs = [
        ("a catalog", catalog),
        ("an integer", Object::from(7)),
        ("null", Object::null()),
        ("a string", bytes(b"text")),
        (
            "an array",
            Object::from(vec![Object::from((2, 0)), Object::from(1.5)]),
        ),
        ("a reference", Object::from((3, 0))),
        (
            "a signature field's widget",
            dict(dict! {
                "Type" => "Annot",
                "Subtype" => "Widget",
                "FT" => "Sig",
                "V" => (9, 0),
            }),
        ),
        (
            "a button field",
            dict(dict! { "FT" => "Btn", "V" => (9, 0) }),
        ),
        (
            "a /Filter name alone",
            dict(dict! { "Filter" => "Standard", "O" => bytes(b"o") }),
        ),
        (
            "/Contents without /ByteRange",
            dict(dict! { "Contents" => bytes(b"text") }),
        ),
    ];
    for (what, obj) in &packs {
        assert!(write::packable((5, 0), obj), "{what}");
    }
}

/// A container's header gives each member's number and where it starts,
/// counted from `/First`, each member followed by a newline so numbers
/// side by side don't run together; it's deflated, the reader reads every
/// member back, and its `/Length` is written direct.
#[test]
fn a_container_lists_each_member_where_it_is() {
    let literal =
        |s: &[u8]| Object::new(Value::String(Str::Bytes(s.to_vec()), Some(Form::Literal)));
    let members = [
        (4, Object::from(12)),
        (9, Object::from(13)),
        (2, Object::from("Name")),
        (3, literal(b"two\nlines (")),
        (
            7,
            Object::from(dict! { "Kids" => vec![Object::from((4, 0)), Object::null()] }),
        ),
        (8, Object::from(-0.5)),
        (11, Object::from(true)),
    ];
    let bytes = as_members(&members);
    let stream = write::container(&bytes, Deflate::DEFAULT).unwrap().unwrap();
    let ints = |key: &[u8]| stream.dict.get(key).unwrap().as_integer().unwrap();
    assert_eq!(stream.dict.type_name(), Some(&b"ObjStm"[..]));
    assert_eq!(
        stream.dict.get(b"Filter").and_then(Object::as_name),
        Some(&b"FlateDecode"[..])
    );
    assert_eq!(ints(b"N"), 7);
    let data = inflated(&stream);
    let first = ints(b"First") as usize;
    let header = std::str::from_utf8(&data[..first]).unwrap();
    assert!(header.ends_with('\n'), "{header:?}");
    let pairs: Vec<usize> = (header.split_whitespace())
        .map(|n| n.parse().unwrap())
        .collect();
    assert_eq!(pairs.len(), 14);
    for (pair, (num, member)) in pairs.chunks(2).zip(&bytes) {
        assert_eq!(pair[0], *num as usize);
        let at = first + pair[1];
        assert_eq!(&data[at..at + member.len()], member.as_slice());
        assert_eq!(data[at + member.len()], b'\n');
    }
    assert_eq!(
        data.len(),
        first + bytes.iter().map(|(_, b)| b.len() + 1).sum::<usize>()
    );
    let numbers: Vec<u32> = members.iter().map(|&(num, _)| num).collect();
    let read = open(&packed(20, &stream, &numbers, Deflate::DEFAULT));
    let packed = (read.entries()).filter(|(_, _, e)| matches!(e, XrefEntry::Compressed { .. }));
    assert_eq!(packed.count(), 7);
    for (num, obj) in &members {
        assert_eq!(value(&read, (*num, 0)), *obj, "object {num}");
    }

    let mut out = Vec::new();
    serialize::indirect(&mut out, (20, 0), &Object::from(stream.clone())).unwrap();
    let length = format!("/Length {}>>", stream.data.bytes().unwrap().len());
    assert!(
        out.windows(length.len()).any(|w| w == length.as_bytes()),
        "{}",
        String::from_utf8_lossy(&out[..out.len().min(80)])
    );
    let reread = open(&file(&[(20, Object::from(stream))]));
    let length = reread
        .get((20, 0))
        .unwrap()
        .as_stream()
        .unwrap()
        .dict
        .get(b"Length");
    assert_eq!(
        length.and_then(Object::as_integer),
        Some(reread.data((20, 0)).unwrap().len() as i64)
    );
}

/// A container's data may be as large as the reader decodes a stream to,
/// and no larger: one a byte past it isn't made, told before anything is
/// copied or deflated (the member here is zeroed memory nothing touches).
#[test]
fn a_container_past_what_the_reader_takes_isn_t_made() {
    assert!(write::fits(write::MAX_DATA));
    assert!(!write::fits(write::MAX_DATA + 1));
    assert_eq!(write::MAX_DATA, Limits::DEFAULT.stream);
    // "1 0\n", then the member and its newline: one byte too many.
    let member = vec![0; write::MAX_DATA - 4];
    let made = write::container(&[(1, member)], Deflate::DEFAULT).unwrap();
    assert!(made.is_none());
}

/// An object in an object stream is listed as its container's number and
/// its place there (7.5.8.3's type 2), and the reader finds it: in a full
/// write, and in an update after bytes before the header, which a
/// container's number isn't counted from.
#[test]
fn a_type_2_entry_is_written_as_its_container_and_place() {
    let five = Object::new(Value::String(
        Str::Bytes(b"five".to_vec()),
        Some(Form::Literal),
    ));
    let six = Object::from(dict! { "Six" => 6 });
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut section = Section::full(0);
    let objects = [
        (1, catalog()),
        (7, container(&[(5, five.clone()), (6, six.clone())])),
    ];
    let mut at = Vec::new();
    for (num, obj) in objects {
        let pos = serialize::indirect(&mut out, (num, 0), &obj).unwrap();
        section.insert(num, Entry::InUse { pos, generation: 0 });
        at.push(pos as u64);
    }
    for (num, index) in [(5, 0), (6, 1)] {
        section.insert(
            num,
            Entry::Packed {
                container: 7,
                index,
            },
        );
    }
    let pos = out.len();
    section
        .stream(
            &mut out,
            8,
            dict! { "Root" => (1, 0) },
            pos,
            Deflate::DEFAULT,
        )
        .unwrap();
    let base = open(&out);
    assert!(sound(&base));
    let (index, _, got) = rows(&base, (8, 0));
    assert_eq!(index, [0, 9]);
    let want = [
        [0, 2, 65535],
        [1, at[0], 0],
        [0, 3, 0],
        [0, 4, 0],
        [0, 0, 0],
        [2, 7, 0],
        [2, 7, 1],
        [1, at[1], 0],
        [1, pos as u64, 0],
    ];
    assert_eq!(got, want);
    for (id, obj) in [((5, 0), &five), ((6, 0), &six)] {
        assert_eq!(value(&base, id), *obj);
    }

    // An update to the file with 100 bytes before its header: 6 again,
    // in container 10. Read from its header, as PDFium, pdf.js and qpdf
    // read positions.
    let mut out = [vec![b'~'; 100], out].concat();
    assert_eq!(open(&out).facts().header_offset, Some(100));
    let base = open(&out[100..]);
    let mut section = Section::append(size(&base), 100);
    let sixty = Object::from(dict! { "Six" => 60 });
    let pos = serialize::indirect(&mut out, (10, 0), &container(&[(6, sixty.clone())])).unwrap();
    section.insert(10, Entry::InUse { pos, generation: 0 });
    section.insert(
        6,
        Entry::Packed {
            container: 10,
            index: 0,
        },
    );
    let pos = out.len();
    let trailer = dict! { "Root" => (1, 0), "Prev" => newest(&base).0 as i64 };
    section
        .stream(&mut out, 11, trailer, pos, Deflate::DEFAULT)
        .unwrap();
    let pdf = open(&out[100..]);
    assert!(sound(&pdf));
    assert_eq!(pdf.facts().sections, 2);
    let (index, _, got) = rows(&pdf, (11, 0));
    assert_eq!(index, [6, 1, 10, 2]);
    assert_eq!(got[0], [2, 10, 0]);
    assert_eq!(value(&pdf, (5, 0)), five);
    assert_eq!(value(&pdf, (6, 0)), sixty);
}

/// A table can't list an object stream's member: it's refused, and
/// nothing is written.
#[test]
fn a_table_refuses_a_packed_entry() {
    let mut section = Section::full(0);
    section.insert(
        1,
        Entry::InUse {
            pos: 9,
            generation: 0,
        },
    );
    section.insert(
        5,
        Entry::Packed {
            container: 7,
            index: 0,
        },
    );
    let mut out = b"%PDF-1.7\n".to_vec();
    assert_eq!(
        section.table(&mut out, dict! { "Root" => (1, 0) }, 20),
        Err(Error::Packed(5))
    );
    assert_eq!(out, b"%PDF-1.7\n", "nothing written");
    assert_eq!(
        Error::Packed(5).to_string(),
        "a cross-reference table can't list object 5, which is in an object stream"
    );
}

/// What a section lists each number as, given back: where each object
/// landed. Entry 0 is the section's own, the free list's head, so an
/// entry given for it is passed over.
#[test]
fn a_section_gives_back_what_it_lists_each_number_as() {
    let mut section = Section::full(3);
    let entries = [
        (
            1,
            Entry::InUse {
                pos: 9,
                generation: 0,
            },
        ),
        (4, Entry::Free { generation: 2 }),
        (
            6,
            Entry::Packed {
                container: 1,
                index: 3,
            },
        ),
    ];
    for (num, entry) in entries {
        section.insert(num, entry);
    }
    for (num, entry) in entries {
        assert!(section.lists(num));
        assert_eq!(section.get(num), Some(entry), "{num}");
    }
    assert_eq!(section.get(2), None, "written free, but not listed");
    assert_eq!(section.size(), 7);
    section.insert(0, Entry::Free { generation: 9 });
    assert!(!section.lists(0));
    assert_eq!(section.get(0), None);
    let mut append = Section::append(3, 0);
    append.insert(0, Entry::Free { generation: 9 });
    append.insert(
        2,
        Entry::InUse {
            pos: 9,
            generation: 0,
        },
    );
    let mut out = Vec::new();
    append.table(&mut out, Dict::new(), 20).unwrap();
    let want = "xref\n2 1\n0000000009 00000 n \ntrailer\n<</Size 3>>\nstartxref\n20\n%%EOF\n";
    assert_eq!(String::from_utf8_lossy(&out), want);
}

/// A position before the header, which positions are counted from, is
/// refused, the section's own or an object's, and nothing is written.
#[test]
fn a_position_before_the_header_is_refused() {
    let mut out = b"%PDF-1.7\n".to_vec();
    let before = Error::BeforeHeader {
        pos: 50,
        header: 100,
    };
    let section = Section::append(3, 100);
    assert_eq!(
        section.table(&mut out, Dict::new(), 50),
        Err(before.clone())
    );
    let stream = section
        .clone()
        .stream(&mut out, 3, Dict::new(), 50, Deflate::DEFAULT);
    assert_eq!(stream, Err(before.clone()));
    let mut section = section;
    section.insert(
        2,
        Entry::InUse {
            pos: 50,
            generation: 0,
        },
    );
    assert_eq!(
        section.table(&mut out, Dict::new(), 150),
        Err(before.clone())
    );
    let stream = section.stream(&mut out, 3, Dict::new(), 150, Deflate::DEFAULT);
    assert_eq!(stream, Err(before.clone()));
    assert_eq!(out, b"%PDF-1.7\n", "nothing written");
    assert_eq!(
        before.to_string(),
        "position 50 is before the header, at 100, which positions are counted from"
    );
}

/// A full write's cross-reference stream and a container, deflated on
/// either backend at any level, read back by the reader inflating on
/// either: the same objects each way. Compressed, the two backends' bytes
/// differ.
#[test]
fn deflated_on_either_backend_at_any_level_each_reads_back() {
    assert_eq!(
        Deflate::DEFAULT,
        Deflate {
            backend: Backend::MinizOxide,
            level: Level::DEFAULT
        }
    );
    assert_eq!(Deflate::default(), Deflate::DEFAULT);
    let members: Vec<(u32, Object)> = (2..300)
        .map(|n| (n, Object::from(dict! { "N" => n, "Name" => "Same" })))
        .collect();
    let numbers: Vec<u32> = members.iter().map(|&(n, _)| n).collect();
    for level in [Level::NONE, Level::DEFAULT, Level::BEST] {
        let mut files = Vec::new();
        for backend in [Backend::MinizOxide, Backend::ZlibRs] {
            let mut deflate = Deflate::DEFAULT;
            deflate.backend = backend;
            deflate.level = level;
            let stream = write::container(&as_members(&members), deflate).unwrap();
            let file = packed(300, &stream.unwrap(), &numbers, deflate);
            for flate in [Backend::MinizOxide, Backend::ZlibRs] {
                let mut options = Options::DEFAULT;
                options.flate = flate;
                let pdf = Reading::open(&file[..], &options).unwrap();
                assert!(pdf.trailer().unwritten.is_none() && pdf.warnings().is_empty());
                for (num, obj) in &members {
                    let read = unplaced(pdf.get((*num, 0)).unwrap());
                    assert_eq!(read, *obj, "{level:?} on {backend:?}, read on {flate:?}");
                }
            }
            files.push(file);
        }
        if level != Level::NONE {
            assert_ne!(files[0], files[1], "{level:?}");
        }
    }
}
