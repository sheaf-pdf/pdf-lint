//! What the writer writes, read back by the reader: cross-reference
//! sections that read as sound, tables and streams, a full write's and an
//! update's, placed past any bytes before the header; object streams,
//! their members read back through the sections that list them; and an
//! encrypted file's objects, encrypted as they're written, read back by
//! their password, nothing the list encrypts in the clear.

use std::sync::Arc;

use pdf_crypt::Fault;
use pdf_crypt::rand_core::SeedableRng;
use pdf_crypt::writer::{self, Kind, Settings};
use pdf_filters::{Backend, Filter, Level, Predictor};
use pdf_objects::dict;
use pdf_objects::object::{Dict, Form, Object, ObjectId, Str, Stream, Value};
use pdf_objects::serialize::{self, Encrypt, Refused};

use super::{Deflate, Encryptor, Entry, Error, Section, Unencryptable};
use crate::bytes::{self, Limits, Options, Reading, XrefEntry};
use crate::fixture::{self, Body, Xref};
use crate::{Encryption, Password, Source, XrefKind, write};

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

// Encrypted while written: the objects of a plaintext file, written through
// an `Encrypting` by pdf-objects' writer into files assembled from a
// `Section`, read back by the reader by their password.

/// What [`plaintext`] holds in a string or a stream, each once.
pub(crate) const PAGE_STRING: &[u8] = b"a page's string";
pub(crate) const FORM_STRING: &[u8] = b"a form's string";
pub(crate) const CONTENT: &[u8] = b"q /Fx Do Q BT (the page's content) Tj ET";
pub(crate) const XMP: &[u8] = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">an XMP title</x:xmpmeta>";
pub(crate) const ATTACHMENT: &[u8] = b"an attachment's data";
pub(crate) const FIELD_NAME: &[u8] = b"a signature's field";
pub(crate) const TITLE: &[u8] = b"an Info title";

/// Every string and stream [`plaintext`] holds but a signature's
/// `/Contents`.
pub(crate) const SECRETS: [&[u8]; 7] = [
    PAGE_STRING,
    FORM_STRING,
    CONTENT,
    XMP,
    ATTACHMENT,
    FIELD_NAME,
    TITLE,
];

/// A signature's `/Contents`, which is never encrypted, and how it's
/// written.
const SIGNATURE: &[u8] = &[0x30, 0x82, 0x01, 0x0a, 0xde, 0xad, 0xbe, 0xef];
pub(crate) const SIGNATURE_WRITTEN: &[u8] = b"<3082010ADEADBEEF>";
/// The same of a signature dictionary with no `/Type`, which is optional
/// (ISO 32000-2 Table 255): told by its `/ByteRange` and `/Contents`.
const UNTYPED_SIGNATURE: &[u8] = &[0x30, 0x82, 0x02, 0x0b, 0xca, 0xfe, 0xba, 0xbe];
pub(crate) const UNTYPED_SIGNATURE_WRITTEN: &[u8] = b"<3082020BCAFEBABE>";

/// The first `/ID` element the tests' encryptions are made from.
pub(crate) const ID: &[u8] = b"0123456789abcdef";

/// Where the tests' sources keep their encryption dictionary: past every
/// object a plaintext file has.
const ENCRYPT: u32 = 40;

/// A literal string.
fn lit(text: &[u8]) -> Object {
    Object::new(Value::String(
        Str::Bytes(text.to_vec()),
        Some(Form::Literal),
    ))
}

/// A file to encrypt, built here, numbered from 1 and with its trailer: a
/// page with a string in its dictionary, whose content draws a form
/// XObject with strings in its stream's dictionary; XMP metadata; an
/// embedded file with its `/Params`; a signature, and one with no `/Type`,
/// each with its `/ByteRange`; an Info title; and the `/ID` the RC4 and
/// AES-128 keys are made from.
pub(crate) fn plaintext() -> (Vec<(u32, Object)>, Dict) {
    let piece = |private: &[u8]| {
        dict! { "Editor" => dict! {
            "LastModified" => lit(b"D:20261016120000Z"),
            "Private" => lit(private),
        } }
    };
    let r = Object::reference;
    let stream = |dict: Dict, data: &[u8]| Object::from(Stream::new(dict, data.to_vec()));
    let objects = vec![
        (
            1,
            Object::from(dict! { "Type" => "Pages", "Kids" => vec![r(4)], "Count" => 1 }),
        ),
        (
            2,
            stream(
                dict! {
                    "Type" => "XObject",
                    "Subtype" => "Form",
                    "BBox" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(100)],
                    "PieceInfo" => piece(FORM_STRING),
                    "LastModified" => lit(b"D:20261016120000Z"),
                },
                b"0 0 100 100 re f",
            ),
        ),
        (3, stream(dict! {}, CONTENT)),
        (
            4,
            Object::from(dict! {
                "Type" => "Page",
                "Parent" => r(1),
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(612), Object::from(792)],
                "Resources" => dict! { "XObject" => dict! { "Fx" => r(2) } },
                "Contents" => r(3),
                "PieceInfo" => piece(PAGE_STRING),
                "LastModified" => lit(b"D:20261016120000Z"),
            }),
        ),
        (
            5,
            stream(dict! { "Type" => "Metadata", "Subtype" => "XML" }, XMP),
        ),
        (
            6,
            stream(
                dict! {
                    "Type" => "EmbeddedFile",
                    "Subtype" => "text/plain",
                    "Params" => dict! {
                        "Size" => ATTACHMENT.len() as i64,
                        "ModDate" => lit(b"D:20261016120000Z"),
                        "CheckSum" => Object::hex([0x5a; 16]),
                    },
                },
                ATTACHMENT,
            ),
        ),
        (
            7,
            Object::from(dict! {
                "Type" => "Filespec",
                "F" => lit(b"notes.txt"),
                "EF" => dict! { "F" => r(6) },
            }),
        ),
        (
            8,
            Object::from(dict! {
                "Type" => "Sig",
                "Filter" => "Adobe.PPKLite",
                "SubFilter" => "adbe.pkcs7.detached",
                "ByteRange" => vec![Object::from(0), Object::from(10), Object::from(20), Object::from(30)],
                "Contents" => Object::hex(SIGNATURE),
                "M" => lit(b"D:20261016120000Z"),
            }),
        ),
        (
            9,
            Object::from(dict! { "FT" => "Sig", "T" => lit(FIELD_NAME), "V" => r(8) }),
        ),
        (
            10,
            Object::from(dict! {
                "Filter" => "Adobe.PPKLite",
                "ByteRange" => vec![Object::from(0), Object::from(10), Object::from(20), Object::from(30)],
                "Contents" => Object::hex(UNTYPED_SIGNATURE),
            }),
        ),
        (11, Object::from(dict! { "FT" => "Sig", "V" => r(10) })),
        (
            12,
            Object::from(dict! {
                "Type" => "Catalog",
                "Pages" => r(1),
                "Metadata" => r(5),
                "Names" => dict! { "EmbeddedFiles" => dict! {
                    "Names" => vec![lit(b"notes.txt"), r(7)],
                } },
                "AcroForm" => dict! { "Fields" => vec![r(9), r(11)] },
            }),
        ),
        (13, Object::from(dict! { "Title" => lit(TITLE) })),
    ];
    let id = Object::hex(ID);
    let trailer = dict! { "Root" => r(12), "Info" => r(13), "ID" => vec![id.clone(), id] };
    (objects, trailer)
}

/// What [`listed`] holds beside [`plaintext`]'s, a string or a stream's
/// data each once, for each row of the list of what's encrypted: a page's
/// metadata stream, which `/EncryptMetadata false` doesn't leave in the
/// clear; a string in the document's metadata stream's dictionary, which
/// it doesn't either; a stream whose own crypt filter is `/Identity`, and
/// one whose own is the file's `/StdCF`; a `/Type /Sig` dictionary without
/// a `/ByteRange`, which isn't a signature's; a dictionary shaped as an
/// encryption dictionary that the trailer doesn't name; and a dictionary
/// typed `/XRef` that isn't a stream.
pub(crate) const PAGE_XMP: &[u8] = b"<x:xmpmeta>a page's XMP</x:xmpmeta>";
pub(crate) const METADATA_STRING: &[u8] = b"a metadata dictionary's string";
pub(crate) const OWN_IDENTITY: &[u8] = b"data its own crypt filter leaves";
pub(crate) const OWN_STDCF: &[u8] = b"data its own crypt filter encrypts";
pub(crate) const UNSIGNED: &[u8] = b"a /Sig without a /ByteRange";
pub(crate) const SHAPED: &[u8] = b"an /O of no encryption";
pub(crate) const XREF_DICT: &[u8] = b"an /XRef dictionary's string";

/// [`plaintext`] with the list's other rows ([`PAGE_XMP`] and the rest),
/// as objects 14 to 19, and the page's `/Metadata`.
pub(crate) fn listed() -> (Vec<(u32, Object)>, Dict) {
    let (mut objects, trailer) = plaintext();
    let stream = |dict: Dict, data: &[u8]| Object::from(Stream::new(dict, data.to_vec()));
    let crypt = |name: &str| {
        dict! {
            "Filter" => vec![Object::from("Crypt")],
            "DecodeParms" => vec![Object::from(dict! { "Type" => "CryptFilterDecodeParms", "Name" => name })],
        }
    };
    for (num, obj) in &mut objects {
        match num {
            4 => obj
                .as_dict_mut()
                .unwrap()
                .set("Metadata", Object::reference(14)),
            5 => (obj.as_stream_mut().unwrap().dict).set("Note", lit(METADATA_STRING)),
            _ => {}
        }
    }
    objects.extend([
        (
            14,
            stream(dict! { "Type" => "Metadata", "Subtype" => "XML" }, PAGE_XMP),
        ),
        (15, stream(crypt("Identity"), OWN_IDENTITY)),
        (16, stream(crypt("StdCF"), OWN_STDCF)),
        (
            17,
            Object::from(dict! { "Type" => "Sig", "Contents" => lit(UNSIGNED) }),
        ),
        (
            18,
            Object::from(dict! {
                "Filter" => "Standard", "V" => 2, "R" => 3, "O" => lit(SHAPED), "U" => lit(b"x"), "P" => -4,
            }),
        ),
        (
            19,
            Object::from(dict! { "Type" => "XRef", "Note" => lit(XREF_DICT) }),
        ),
    ]);
    (objects, trailer)
}

/// The test writer's settings for `kind`, user `user`, owner `owner`, with
/// [`ID`].
fn settings(kind: Kind, encrypt_metadata: bool) -> Settings<'static> {
    Settings {
        kind,
        user: b"user",
        owner: b"owner",
        p: -4,
        id: ID,
        encrypt_metadata,
    }
}

/// The encryption dictionary the test writer writes for `kind`.
fn dictionary_of(kind: Kind, encrypt_metadata: bool) -> Dict {
    write::encryption(&writer::dictionary(&settings(kind, encrypt_metadata)), None)
}

/// A file to take an encryption from: a catalog, and `dict`, its
/// encryption dictionary, as object [`ENCRYPT`], or in the trailer when
/// `direct`, with [`ID`].
pub(crate) fn source(dict: Dict, direct: bool) -> Vec<u8> {
    let id = Object::hex(ID);
    let mut trailer = dict! { "Root" => Object::reference(1), "ID" => vec![id.clone(), id] };
    let mut objects = vec![(1, Body::from(dict! { "Type" => "Catalog" }))];
    match direct {
        true => trailer.set("Encrypt", dict),
        false => {
            objects.push((ENCRYPT, Body::from(dict)));
            trailer.set("Encrypt", Object::reference(ENCRYPT));
        }
    }
    fixture::file("1.7", &objects, trailer, Xref::Table)
}

/// `file` opened by `password`.
fn opened(file: &[u8], password: &str) -> Reading<Arc<[u8]>> {
    Reading::open_with(Arc::from(file), &[Password(password)], &Options::DEFAULT).unwrap()
}

/// How `file`, opened by the user's password, is encrypted.
fn encryptor_of(file: &[u8]) -> Encryptor {
    Encryptor::of(&opened(file, "user")).unwrap().unwrap()
}

/// An encryption the tests write under, and what it leaves in the clear.
pub(crate) struct Method {
    pub(crate) name: &'static str,
    pub(crate) encryptor: Encryptor,
    /// Whether strings are encrypted.
    pub(crate) strings: bool,
    /// Whether streams are, but for these two.
    pub(crate) streams: bool,
    /// Whether an embedded file's data is.
    pub(crate) attachment: bool,
    /// Whether the document's metadata stream's data is.
    pub(crate) metadata: bool,
    /// Whether it has crypt filters (`/V` 4 and later), so that a stream's
    /// own `/Identity` leaves it in the clear: below, a stream is encrypted
    /// as streams are, whatever its own crypt filter's name.
    pub(crate) filters: bool,
}

/// Every encryption the writer writes under: RC4-40, RC4-128, AES-128,
/// AES-256 of revisions 5 and 6 (each a source's, by the test writer's
/// dictionary), a new one of revision 6 (`Encryptor::create`, from a
/// generator seeded 8); attachments alone (AES-128 by `/EFF`, `/StmF` and
/// `/StrF` `/Identity`); streams in the clear under encrypted strings
/// (AES-128, `/StmF /Identity`); and metadata in the clear (AES-128,
/// `/EncryptMetadata false`).
pub(crate) fn methods() -> Vec<Method> {
    use chacha20::ChaCha20Rng;
    use pdf_crypt::rand_core::SeedableRng;

    let of = |dict: Dict| encryptor_of(&source(dict, false));
    let all = |name: &'static str, encryptor| Method {
        name,
        encryptor,
        strings: true,
        streams: true,
        attachment: true,
        metadata: true,
        filters: !name.starts_with("RC4"),
    };
    let mut methods: Vec<Method> = [
        ("RC4-40", Kind::Rc4By40),
        ("RC4-128", Kind::Rc4By128),
        ("AES-128", Kind::Aes128),
        ("AES-256 R5", Kind::Aes256R5),
        ("AES-256 R6", Kind::Aes256),
    ]
    .into_iter()
    .map(|(name, kind)| all(name, of(dictionary_of(kind, true))))
    .collect();
    let prepared = Password("user").prepared().unwrap();
    let mut rng = ChaCha20Rng::seed_from_u64(8);
    let created = Encryptor::create(&prepared, true, &mut rng).unwrap();
    methods.push(all("AES-256 created", created));
    let mut eff = dictionary_of(Kind::Aes128, true);
    eff.set("StmF", "Identity");
    eff.set("StrF", "Identity");
    eff.set("EFF", "StdCF");
    methods.push(Method {
        name: "attachments alone",
        encryptor: of(eff),
        strings: false,
        streams: false,
        attachment: true,
        metadata: false,
        filters: true,
    });
    let mut identity = dictionary_of(Kind::Aes128, true);
    identity.set("StmF", "Identity");
    methods.push(Method {
        name: "streams in the clear",
        encryptor: of(identity),
        strings: true,
        streams: false,
        attachment: false,
        metadata: false,
        filters: true,
    });
    methods.push(Method {
        name: "metadata in the clear",
        encryptor: of(dictionary_of(Kind::Aes128, false)),
        strings: true,
        streams: true,
        attachment: true,
        metadata: false,
        filters: true,
    });
    methods
}

/// `objects` written through `encryptor`'s `Encrypting` by pdf-objects'
/// writer, then its encryption dictionary, as written, at its number (past
/// every object for a new one's) or in the trailer, as it was; with
/// `trailer`'s entries, `/ID` the encryptor's first, twice ([`ID`] for a
/// new one's); and a whole section of `kind`, a stream's numbered past
/// everything.
pub(crate) fn encrypted_by(
    encryptor: &Encryptor,
    objects: &[(u32, Object)],
    trailer: &Dict,
    kind: XrefKind,
) -> Vec<u8> {
    let resolve = |id: ObjectId| {
        (objects.iter())
            .find(|(n, _)| (*n, 0) == id)
            .map(|(_, o)| o)
    };
    let encrypting = encryptor.encrypting(trailer, &resolve);
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut section = Section::full(0);
    for (num, obj) in objects {
        let pos = serialize::indirect_with(&mut out, (*num, 0), obj, &encrypting).unwrap();
        section.insert(*num, Entry::InUse { pos, generation: 0 });
    }
    let last = objects.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let mut trailer = trailer.clone();
    let dict = encryptor.dictionary().clone();
    match encryptor.direct() {
        true => trailer.set("Encrypt", dict),
        false => {
            let at = encryptor.at().unwrap_or((last + 1, 0));
            let pos = serialize::indirect(&mut out, at, &Object::from(dict)).unwrap();
            let generation = at.1;
            section.insert(at.0, Entry::InUse { pos, generation });
            trailer.set("Encrypt", Object::from(at));
        }
    }
    let id = Object::hex(encryptor.first_id().unwrap_or(ID));
    trailer.set("ID", vec![id.clone(), id]);
    let pos = out.len();
    let own = section.size().max(last + 2);
    match kind {
        XrefKind::Table => section.table(&mut out, trailer, pos),
        XrefKind::Stream => section.stream(&mut out, own, trailer, pos, Deflate::DEFAULT),
    }
    .unwrap();
    out
}

/// Whether `bytes` hold `text` anywhere.
pub(crate) fn holds(bytes: &[u8], text: &[u8]) -> bool {
    bytes.windows(text.len()).any(|w| w == text)
}

/// Whether `bytes` hold `text` where a reader finds it with no key: as it
/// is ([`holds`]) or in hex, as a string written `<…>` holds it, in either
/// case of digit; in the bytes, or in a stream's data that inflates, as an
/// object stream's members do, which a search of the bytes misses.
pub(crate) fn in_clear(bytes: &[u8], text: &[u8]) -> bool {
    let hex = |digits: &[u8; 16]| -> Vec<u8> {
        (text.iter())
            .flat_map(|&b| [digits[usize::from(b >> 4)], digits[usize::from(b & 15)]])
            .collect()
    };
    let forms = [
        text.to_vec(),
        hex(b"0123456789ABCDEF"),
        hex(b"0123456789abcdef"),
    ];
    let found = |hay: &[u8]| forms.iter().any(|form| holds(hay, form));
    // A stream's data: after `stream` and its end of line, to `endstream`.
    let data = |at: usize| {
        let data = &bytes[at..];
        let data = data.strip_prefix(b"\r").unwrap_or(data);
        let data = data.strip_prefix(b"\n")?;
        let end = data.windows(9).position(|w| w == b"endstream")?;
        Some(&data[..end])
    };
    // What inflates before an error: of encrypted data, nothing.
    let inflates_to_it = |data: &[u8]| found(&fixture::inflated(data));
    found(bytes)
        || (bytes.windows(6).enumerate())
            .filter(|&(at, w)| w == b"stream" && !bytes[..at].ends_with(b"end"))
            .filter_map(|(at, _)| data(at + 6))
            .any(inflates_to_it)
}

/// `d` as the writer writes it: what two readings of one dictionary, or a
/// reading and what it was built as, have the same.
fn as_written(d: &Dict) -> Vec<u8> {
    let mut out = Vec::new();
    serialize::write(&mut out, &Object::from(d.clone())).unwrap();
    out
}

/// `d` as built, but its `/Length`, which is its data's as written.
fn without_length(d: &Dict) -> Object {
    let mut d = d.clone();
    d.remove(b"Length");
    unplaced(&Object::from(d))
}

/// Whether `back`, read, is `plain`, as built: a stream's dictionary but
/// its `/Length`.
fn same(back: &Object, plain: &Object) -> bool {
    match (back.value(), plain.value()) {
        (Value::Stream(b), Value::Stream(p)) => without_length(&b.dict) == without_length(&p.dict),
        _ => unplaced(back) == *plain,
    }
}

/// `bytes`, opened by the user's password, are `objects`, object by
/// object, lazily and whole, each stream's data handed out as it was
/// built, with nothing to warn of; the trailer's `/ID`, [`ID`] twice, as
/// written, never decrypted. Gives the reading.
pub(crate) fn assert_decrypts_to(
    objects: &[(u32, Object)],
    bytes: &[u8],
    name: &str,
) -> Reading<Arc<[u8]>> {
    let reading = opened(bytes, "user");
    assert!(
        matches!(reading.encryption(), Encryption::Opened { .. }),
        "{name}: {:?}",
        reading.encryption()
    );
    let whole = bytes::read_opened(bytes, &[Password("user")]);
    for (num, plain) in objects {
        let id = (*num, 0);
        let lazy = reading.get(id).unwrap();
        let eager = whole.get(id).unwrap();
        assert!(same(lazy, plain), "{name}, lazily: {id:?}: {lazy:?}");
        assert!(same(eager, plain), "{name}, whole: {id:?}: {eager:?}");
        if let Some(s) = plain.as_stream() {
            let data = reading.data(id).unwrap();
            assert_eq!(data.as_ref(), s.data.bytes().unwrap(), "{name}: {id:?}");
        }
    }
    assert!(sound(&reading), "{name}: {:?}", reading.warnings());
    assert!(whole.warnings.is_empty(), "{name}: {:?}", whole.warnings);
    // The trailer's /ID as written, [`ID`] twice in hex, never decrypted.
    let id = reading.trailer().dict().get(b"ID").map(unplaced);
    let written = Object::from(vec![Object::hex(ID), Object::hex(ID)]);
    assert_eq!(id, Some(written), "{name}: the trailer's /ID as written");
    reading
}

#[test]
fn objects_encrypt_as_the_lazy_reader_decrypts() {
    let (plain, trailer) = plaintext();
    let mut eff = dictionary_of(Kind::Aes128, true);
    eff.set("StmF", "Identity");
    eff.set("StrF", "Identity");
    eff.set("EFF", "StdCF");
    let states = [
        ("RC4-128", dictionary_of(Kind::Rc4By128, true)),
        ("AES-128", dictionary_of(Kind::Aes128, true)),
        ("AES-256", dictionary_of(Kind::Aes256, true)),
        ("attachment only", eff),
    ];
    for (name, state) in states {
        let strings = name != "attachment only";
        let crypt = encryptor_of(&source(state, false));
        for kind in [XrefKind::Table, XrefKind::Stream] {
            let bytes = encrypted_by(&crypt, &plain, &trailer, kind);
            let name = format!("{name}, {kind:?}");
            if strings {
                // Every string and stream is encrypted, those in a stream's
                // dictionary too, but a signature's /Contents.
                for text in SECRETS {
                    assert!(
                        !holds(&bytes, text),
                        "{name}: {:?}",
                        String::from_utf8_lossy(text)
                    );
                }
            } else {
                // Only the attachment's data is.
                assert!(!holds(&bytes, ATTACHMENT), "{name}");
                for text in SECRETS.into_iter().filter(|&text| text != ATTACHMENT) {
                    assert!(
                        holds(&bytes, text),
                        "{name}: {:?}",
                        String::from_utf8_lossy(text)
                    );
                }
            }
            assert!(holds(&bytes, SIGNATURE_WRITTEN), "{name}");
            // Each string in its own format.
            let lazy = assert_decrypts_to(&plain, &bytes, &name);
            // Its /Encrypt as stored, /P and all, and its /ID[0].
            let back = Encryptor::of(&lazy).unwrap().unwrap();
            let dictionaries = [back.dictionary(), crypt.dictionary()].map(as_written);
            assert_eq!(dictionaries[0], dictionaries[1], "{name}");
            assert_eq!(back.first_id(), crypt.first_id(), "{name}");
        }
    }
}

/// The same objects encrypt to the same bytes, twice and on two threads.
#[test]
fn objects_encrypt_alike_every_time() {
    let (plain, trailer) = plaintext();
    for kind in [Kind::Rc4By128, Kind::Aes128, Kind::Aes256] {
        let crypt = encryptor_of(&source(dictionary_of(kind, true), false));
        let resolve = |id: ObjectId| (plain.iter()).find(|(n, _)| (*n, 0) == id).map(|(_, o)| o);
        let encrypting = crypt.encrypting(&trailer, &resolve);
        let encrypt = |num: u32, obj: &Object| {
            let mut out = Vec::new();
            serialize::indirect_with(&mut out, (num, 0), obj, &encrypting).unwrap();
            out
        };
        // Every object, twice, and on two threads.
        let once: Vec<Vec<u8>> = plain.iter().map(|(num, obj)| encrypt(*num, obj)).collect();
        let again = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        let all = plain.iter().map(|(num, obj)| encrypt(*num, obj));
                        all.collect::<Vec<Vec<u8>>>()
                    })
                })
                .collect();
            threads
                .into_iter()
                .map(|t| t.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(again, [once.clone(), once], "{kind:?}");
    }
}

/// What the writer writes is keyed by what it is: an AES string's IV is
/// the same for the same object and value, every time it's written, and
/// another at another number, or for another value; and it decrypts, as
/// the reader decrypts, by the number it's at.
#[test]
fn ivs_are_keyed_by_what_the_object_is() {
    let (plain, trailer) = plaintext();
    for kind in [Kind::Aes128, Kind::Aes256R5, Kind::Aes256] {
        let crypt = encryptor_of(&source(dictionary_of(kind, true), false));
        let resolve = |id: ObjectId| (plain.iter()).find(|(n, _)| (*n, 0) == id).map(|(_, o)| o);
        let encrypting = crypt.encrypting(&trailer, &resolve);
        let iv = |id: ObjectId, value: &[u8]| encrypting.string(id, value).unwrap()[..16].to_vec();
        let first = iv((5, 0), b"a value");
        assert_eq!(iv((5, 0), b"a value"), first, "{kind:?}");
        for other in [
            iv((6, 0), b"a value"),
            iv((5, 1), b"a value"),
            iv((5, 0), b"another value"),
        ] {
            assert_ne!(other, first, "{kind:?}");
        }
        // Decrypted as the reader decrypts: by the number it's at.
        let mut file = b"%PDF-1.7\n".to_vec();
        let mut section = Section::full(0);
        let objects = [(
            1,
            Object::from(dict! { "Type" => "Catalog", "V" => lit(b"a value") }),
        )];
        for (num, obj) in &objects {
            let pos = serialize::indirect_with(&mut file, (*num, 0), obj, &encrypting).unwrap();
            section.insert(*num, Entry::InUse { pos, generation: 0 });
        }
        let at = crypt.at().unwrap();
        let pos = serialize::indirect(&mut file, at, &Object::from(crypt.dictionary().clone()));
        section.insert(
            at.0,
            Entry::InUse {
                pos: pos.unwrap(),
                generation: 0,
            },
        );
        let id = Object::hex(ID);
        let trailer = dict! { "Root" => (1, 0), "Encrypt" => at, "ID" => vec![id.clone(), id] };
        let pos = file.len();
        section.table(&mut file, trailer, pos).unwrap();
        assert_decrypts_to(&objects, &file, &format!("{kind:?}"));
    }
}

#[test]
fn xref_streams_metadata_and_the_encrypt_dict_stay_plain() {
    let (plain, trailer) = listed();
    let crypt = encryptor_of(&source(dictionary_of(Kind::Aes128, false), false));
    let resolve = |id: ObjectId| (plain.iter()).find(|(n, _)| (*n, 0) == id).map(|(_, o)| o);
    let encrypting = crypt.encrypting(&trailer, &resolve);
    let written = |id: ObjectId, obj: &Object| {
        let mut out = Vec::new();
        serialize::indirect_with(&mut out, id, obj, &encrypting).unwrap();
        out
    };
    let stays = |id: ObjectId, obj: &Object| {
        let mut out = Vec::new();
        serialize::indirect(&mut out, id, obj).unwrap();
        assert_eq!(written(id, obj), out, "{id:?}");
    };
    let xref = Stream::new(
        dict! {
            "Type" => "XRef",
            "ID" => vec![lit(b"an /ID"), lit(b"an /ID")],
        },
        vec![1, 0, 0, 0],
    );
    stays((20, 0), &Object::from(xref));
    let at = crypt.at().unwrap();
    let encrypt = Object::from(crypt.dictionary().clone());
    stays(at, &encrypt);
    // A dictionary shaped as the encryption dictionary at a number that
    // isn't one is encrypted: the list tells them by number.
    assert!(!holds(&written((21, 0), &encrypt), b"/O ("), "by number");
    // The document's metadata stream's data under /EncryptMetadata false,
    // but not its dictionary's strings, nor a page's metadata.
    let (metadata, xmp) = (plain.iter())
        .find(|(n, _)| *n == 5)
        .map(|(n, o)| ((*n, 0), o))
        .unwrap();
    let bytes = written(metadata, xmp);
    assert!(holds(&bytes, XMP) && !in_clear(&bytes, METADATA_STRING));
    let page_xmp = &plain.iter().find(|(n, _)| *n == 14).unwrap().1;
    assert!(!in_clear(&written((14, 0), page_xmp), PAGE_XMP));
    // A signature's /Contents, but not its other strings.
    let signature = &plain.iter().find(|(n, _)| *n == 8).unwrap().1;
    let bytes = written((8, 0), signature);
    assert!(holds(&bytes, SIGNATURE_WRITTEN));
    assert!(!holds(&bytes, b"D:20261016120000Z"));

    // Written, the metadata is in the clear and reads as it was, and the
    // cross-reference stream's /ID is as written.
    for kind in [XrefKind::Table, XrefKind::Stream] {
        let bytes = encrypted_by(&crypt, &plain, &trailer, kind);
        assert!(holds(&bytes, XMP), "{kind:?}");
        assert_decrypts_to(&plain, &bytes, &format!("metadata in the clear, {kind:?}"));
    }
}

/// [`in_clear`] finds a string written in hex, in either case of digit,
/// on its own or in an object stream's member, deflated; not a string
/// that differs.
#[test]
fn in_clear_finds_a_string_written_in_hex() {
    let file = |form, case: fn(&mut Vec<u8>)| {
        let mut member = Vec::new();
        let value = Object::new(Value::String(Str::Bytes(b"Ann Lee".to_vec()), Some(form)));
        serialize::write(&mut member, &Object::from(dict! { "V" => value })).unwrap();
        case(&mut member);
        let container = write::container(&[(1, member.clone())], Deflate::DEFAULT).unwrap();
        let mut packed = Vec::new();
        serialize::indirect(&mut packed, (2, 0), &Object::from(container.unwrap())).unwrap();
        [member, packed]
    };
    let [plain, packed] = file(Form::Hex, |_| {});
    assert!(holds(&plain, b"<416E6E204C6565>"));
    assert!(!holds(&packed, b"416E6E204C6565"));
    let [lower, lower_packed] = file(Form::Hex, |m| m.make_ascii_lowercase());
    for bytes in [plain, packed, lower, lower_packed] {
        assert!(in_clear(&bytes, b"Ann Lee"));
        assert!(!in_clear(&bytes, b"Ann Lea"));
    }
    let [literal, packed] = file(Form::Literal, |_| {});
    assert!(in_clear(&literal, b"Ann Lee") && in_clear(&packed, b"Ann Lee"));
}

/// Under every method, a file's objects written through its `Encrypting`,
/// with a table and with a stream, read back by its password as they were
/// built, and nothing the method encrypts is in the clear, as it is, in hex
/// or inflated, while what it leaves in the clear is as written: a row of
/// the list of what's encrypted each, both ways. In the clear: a
/// signature's `/Contents`, typed or not; the cross-reference stream's
/// strings, as the trailer's, never encrypted; the encryption dictionary,
/// as stored, which reads back as its encryption; a stream whose own crypt
/// filter is `/Identity`, from `/V` 4, below which there are no crypt
/// filters; and under `/EncryptMetadata false`, the
/// document's metadata stream's data, not its dictionary's strings or a
/// page's metadata. Encrypted where strings are: a `/Type /Sig` without
/// `/ByteRange`, a dictionary shaped as an encryption dictionary, and one
/// typed `/XRef` that isn't a stream. A stream whose own crypt filter is
/// the file's encrypted by it, its streams encrypted or not.
#[test]
fn each_method_writes_what_the_reader_decrypts_and_nothing_else_in_the_clear() {
    let (plain, trailer) = listed();
    for method in methods() {
        let crypt = &method.encryptor;
        for kind in [XrefKind::Table, XrefKind::Stream] {
            let name = format!("{}, {kind:?}", method.name);
            let bytes = encrypted_by(crypt, &plain, &trailer, kind);
            let strings = [
                PAGE_STRING,
                FORM_STRING,
                FIELD_NAME,
                TITLE,
                METADATA_STRING,
                UNSIGNED,
                SHAPED,
                XREF_DICT,
            ];
            let mut rows: Vec<(&[u8], bool)> = strings.map(|s| (s, method.strings)).to_vec();
            rows.extend([
                (CONTENT, method.streams),
                (PAGE_XMP, method.streams),
                (XMP, method.metadata),
                (ATTACHMENT, method.attachment),
                (OWN_STDCF, true),
                (OWN_IDENTITY, !method.filters),
                (SIGNATURE, false),
                (UNTYPED_SIGNATURE, false),
            ]);
            for (text, encrypted) in rows {
                let text_said = String::from_utf8_lossy(text);
                assert_eq!(in_clear(&bytes, text), !encrypted, "{name}: {text_said:?}");
            }
            assert!(holds(&bytes, SIGNATURE_WRITTEN), "{name}");
            assert!(holds(&bytes, UNTYPED_SIGNATURE_WRITTEN), "{name}");
            let reading = assert_decrypts_to(&plain, &bytes, &name);
            assert_eq!(reading.facts().newest.map(|(_, k)| k), Some(kind), "{name}");
            // Its /Encrypt as stored, where it was, and its /ID[0].
            let back = Encryptor::of(&reading).unwrap().unwrap();
            let dictionaries = [back.dictionary(), crypt.dictionary()].map(as_written);
            assert_eq!(dictionaries[0], dictionaries[1], "{name}");
            assert_eq!(
                back.first_id(),
                Some(crypt.first_id().unwrap_or(ID)),
                "{name}"
            );
            let last = (plain.len() + 1) as u32;
            assert_eq!(back.at(), Some(crypt.at().unwrap_or((last, 0))), "{name}");
            assert_eq!(back.packs(), crypt.packs(), "{name}");
        }
    }
}

/// An object stream is encrypted whole, as a stream, under the methods
/// that pack, its members' strings with it, and its members read back
/// from it; none where streams are left in the clear and strings aren't.
#[test]
fn a_container_is_encrypted_whole_where_the_method_packs() {
    let members: Vec<(u32, Object)> = (2..6)
        .map(|n| (n, Object::from(dict! { "N" => n, "S" => lit(PAGE_STRING) })))
        .collect();
    for method in methods() {
        let packs = method.encryptor.packs();
        assert_eq!(packs, method.streams || !method.strings, "{}", method.name);
        if !packs {
            continue;
        }
        let trailer = dict! { "Root" => (1, 0) };
        let objects = [(1, catalog())];
        let resolve = |id: ObjectId| {
            (objects.iter())
                .find(|(n, _)| (*n, 0) == id)
                .map(|(_, o)| o)
        };
        let encrypting = method.encryptor.encrypting(&trailer, &resolve);
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut section = Section::full(0);
        let container = container(&members);
        for (num, obj) in [(1, catalog()), (9, container)] {
            let pos = serialize::indirect_with(&mut out, (num, 0), &obj, &encrypting).unwrap();
            section.insert(num, Entry::InUse { pos, generation: 0 });
        }
        for (index, (num, _)) in (0..).zip(&members) {
            section.insert(
                *num,
                Entry::Packed {
                    container: 9,
                    index,
                },
            );
        }
        let mut trailer = trailer.clone();
        let at = method.encryptor.at().unwrap_or((20, 0));
        let dict = Object::from(method.encryptor.dictionary().clone());
        let pos = serialize::indirect(&mut out, at, &dict).unwrap();
        section.insert(at.0, Entry::InUse { pos, generation: 0 });
        trailer.set("Encrypt", Object::from(at));
        let id = Object::hex(method.encryptor.first_id().unwrap_or(ID));
        trailer.set("ID", vec![id.clone(), id]);
        let pos = out.len();
        let own = section.size();
        section
            .stream(&mut out, own, trailer, pos, Deflate::DEFAULT)
            .unwrap();
        assert_eq!(
            in_clear(&out, PAGE_STRING),
            !method.streams,
            "{}",
            method.name
        );
        let reading = opened(&out, "user");
        for (num, obj) in &members {
            assert_eq!(value(&reading, (*num, 0)), *obj, "{}", method.name);
        }
        assert!(sound(&reading), "{}: {:?}", method.name, reading.warnings());
    }
}

/// `Encryptor::of` gives none for a file that isn't encrypted, and refuses
/// one no password opened, whether its strings and streams are encrypted
/// or not, and one whose security handler the reader can't open, each
/// saying why.
#[test]
fn an_encryptor_isnt_had_of_a_locked_plain_or_unsupported_reading() {
    let plain = fixture::file(
        "1.7",
        &[(1, Body::from(dict! { "Type" => "Catalog" }))],
        dict! { "Root" => (1, 0) },
        Xref::Table,
    );
    assert!(Encryptor::of(&opened(&plain, "user")).unwrap().is_none());
    let locked = opened(&source(dictionary_of(Kind::Aes128, true), false), "wrong");
    assert_eq!(*locked.encryption(), Encryption::Locked);
    assert_eq!(Encryptor::of(&locked).unwrap_err(), Error::Locked);
    let mut identity = dictionary_of(Kind::Aes128, true);
    identity.set("StmF", "Identity");
    identity.set("StrF", "Identity");
    let plain_only = opened(&source(identity, false), "wrong");
    assert_eq!(*plain_only.encryption(), Encryption::Plain);
    assert_eq!(Encryptor::of(&plain_only).unwrap_err(), Error::Locked);
    let mut other = dictionary_of(Kind::Aes128, true);
    other.set("Filter", "Nonesuch");
    let unsupported = opened(&source(other, false), "user");
    let Encryption::Unsupported(why) = unsupported.encryption().clone() else {
        panic!("{:?}", unsupported.encryption());
    };
    assert_eq!(
        Encryptor::of(&unsupported).unwrap_err(),
        Error::Unsupported(why.clone())
    );
    assert_eq!(
        Error::Locked.to_string(),
        "it's encrypted, and no password opened it, so there's no key to encrypt by"
    );
    assert_eq!(
        Error::Unsupported(why.clone()).to_string(),
        format!("it's encrypted by a security handler the reader can't open: {why}")
    );
}

/// An encryption dictionary in the trailer, which the reader opens, is the
/// encryptor's as it was, with no number; written so, direct in the
/// trailer, a file reads back by its password, with a table and with a
/// stream.
#[test]
fn a_direct_encryption_dictionary_is_its_own_with_no_number() {
    let (plain, trailer) = plaintext();
    for kind in [Kind::Rc4By128, Kind::Aes128, Kind::Aes256] {
        let dict = dictionary_of(kind, true);
        let crypt = encryptor_of(&source(dict.clone(), true));
        assert!(crypt.direct(), "{kind:?}");
        assert_eq!(crypt.at(), None, "{kind:?}");
        assert_eq!(
            as_written(crypt.dictionary()),
            as_written(&dict),
            "{kind:?}"
        );
        assert_eq!(crypt.first_id(), Some(ID), "{kind:?}");
        for xref in [XrefKind::Table, XrefKind::Stream] {
            let name = format!("{kind:?}, {xref:?}");
            let bytes = encrypted_by(&crypt, &plain, &trailer, xref);
            for text in SECRETS {
                assert!(!in_clear(&bytes, text), "{name}");
            }
            let reading = assert_decrypts_to(&plain, &bytes, &name);
            let back = Encryptor::of(&reading).unwrap().unwrap();
            assert!(back.direct() && back.at().is_none(), "{name}");
            assert_eq!(as_written(back.dictionary()), as_written(&dict), "{name}");
        }
    }
}

/// What can't be encrypted is refused, and nothing written: a stream whose
/// own crypt filter names one the dictionary hasn't; an embedded file's
/// data where `/EFF` isn't a name; and, asked of the encryptor alone, a
/// stream an encryption dictionary refers to. A new file's encryption
/// refuses a password that isn't prepared.
#[test]
fn what_cant_be_encrypted_is_refused_and_nothing_written() {
    let crypt = encryptor_of(&source(dictionary_of(Kind::Aes128, true), false));
    let trailer = dict! { "Root" => (1, 0) };
    let resolve = |_: ObjectId| None;
    let encrypting = crypt.encrypting(&trailer, &resolve);
    let unknown = dict! {
        "Filter" => "Crypt",
        "DecodeParms" => dict! { "Name" => "Nonesuch" },
    };
    let mut out = b"%PDF-1.7\n".to_vec();
    let refused = serialize::indirect_stream_with(&mut out, (3, 0), &unknown, b"data", &encrypting);
    let fault = Fault::Method {
        name: "Nonesuch".to_string(),
    };
    let why = Unencryptable::Fault(fault);
    assert_eq!(
        refused,
        Err(Refused::Encrypt {
            id: (3, 0),
            why: why.clone()
        })
    );
    assert_eq!(out, b"%PDF-1.7\n", "nothing written");
    let error = Error::from(refused.unwrap_err());
    assert_eq!(error, Error::Encrypt { id: (3, 0), why });
    assert_eq!(
        error.to_string(),
        "object 3 0 can't be encrypted: its crypt filter's method or name, /Nonesuch, isn't one"
    );
    // /EFF that isn't a name.
    let mut unsaid = dictionary_of(Kind::Aes128, true);
    unsaid.set("EFF", 5);
    let crypt = encryptor_of(&source(unsaid, false));
    let encrypting = crypt.encrypting(&trailer, &resolve);
    let embedded = Object::from(Stream::new(
        dict! { "Type" => "EmbeddedFile" },
        b"data".to_vec(),
    ));
    let refused = serialize::indirect_with(&mut out, (3, 0), &embedded, &encrypting);
    let why = Unencryptable::Unsaid;
    assert_eq!(
        refused,
        Err(Refused::Encrypt {
            id: (3, 0),
            why: why.clone()
        })
    );
    assert_eq!(out, b"%PDF-1.7\n", "nothing written");
    assert_eq!(
        Error::Encrypt { id: (3, 0), why }.to_string(),
        "object 3 0 can't be encrypted: it's an embedded file's data, and the encryption \
         dictionary's /EFF, which says how that's encrypted, isn't a name"
    );
    // A stream an encryption dictionary refers to, which the writer leaves
    // whole in the clear, as the reader does.
    let at = (ENCRYPT, 0);
    assert!(encrypting.clear(at, Some(&dict! {}), true));
    let asked = encrypting.stream(at, &dict! {}, b"data");
    assert_eq!(asked, Err(Unencryptable::Dictionary));
    // A password not prepared.
    let mut rng = chacha20::ChaCha20Rng::seed_from_u64(8);
    let unprepared = pdf_crypt::Encoded::verbatim(b"user");
    assert!(Encryptor::create(&unprepared, true, &mut rng).is_err());
}
