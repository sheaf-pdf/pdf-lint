//! What the writer writes, read back by the reader: cross-reference
//! sections that read as sound, tables and streams, a full write's and an
//! update's, placed past any bytes before the header; object streams,
//! their members read back through the sections that list them; an
//! encrypted file's objects, encrypted as they're written, read back by
//! their password, nothing the list encrypts in the clear; whole files and
//! updates, read, written and read again, the same objects, and what each
//! refuses; and copies, an object as read written as the bytes it was read
//! from, a test for each rule.

use std::borrow::Cow;
use std::sync::Arc;

use pdf_crypt::Fault;
use pdf_crypt::rand_core::SeedableRng;
use pdf_crypt::writer::{self, Kind, Settings};
use pdf_filters::{Backend, Filter, Level, Predictor};
use pdf_objects::dict;
use pdf_objects::object::{Data, Dict, Form, Object, ObjectId, Span, Str, Stream, Value};
use pdf_objects::serialize::{self, Encrypt, Refused};

use super::{Deflate, Encryptor, Entry, Error, Section, Unencryptable};
use crate::bytes::{self, ByteSource, Limits, Options, Reading, XrefEntry};
use crate::fixture::{self, Body, Xref};
use crate::fuzz::Flipped;
use crate::{Encryption, Password, Record, Source, XrefKind, write};

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

/// Object `id` of `pdf`, as built, a stream's data as the reading hands it
/// out: what two files of the same objects read alike, wherever each put
/// them.
fn handed(pdf: &Reading<Arc<[u8]>>, id: ObjectId) -> Object {
    let mut o = value(pdf, id);
    if let Some(stream) = o.as_stream_mut() {
        stream.data = Data::Bytes(pdf.data(id).unwrap().into_owned());
    }
    o
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
        section.insert(
            *num,
            Entry::InUse {
                offset: pos,
                generation: 0,
            },
        );
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
            offset: one,
            generation: 0,
        },
    );
    let three = serialize::indirect(&mut out, (3, 2), &Object::from(3)).unwrap();
    section.insert(
        3,
        Entry::InUse {
            offset: three,
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
            offset: one,
            generation: 0,
        },
    );
    section.insert(3, Entry::Free { generation: 1 });
    let five = serialize::indirect(&mut out, (5, 0), &Object::from(5)).unwrap();
    section.insert(
        5,
        Entry::InUse {
            offset: five,
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
    section.insert(
        2,
        Entry::InUse {
            offset: pos,
            generation: 0,
        },
    );
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
            offset: far,
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
        section.insert(
            num,
            Entry::InUse {
                offset: pos,
                generation: 0,
            },
        );
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
            offset: two,
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
        section.insert(
            num,
            Entry::InUse {
                offset: pos,
                generation: 0,
            },
        );
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
        section.insert(
            n,
            Entry::InUse {
                offset: pos,
                generation: 0,
            },
        );
    }
    for (index, &n) in (0..).zip(members) {
        section.insert(n, Entry::Compressed { stream: num, index });
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
        section.insert(
            num,
            Entry::InUse {
                offset: pos,
                generation: 0,
            },
        );
        at.push(pos as u64);
    }
    for (num, index) in [(5, 0), (6, 1)] {
        section.insert(num, Entry::Compressed { stream: 7, index });
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
    section.insert(
        10,
        Entry::InUse {
            offset: pos,
            generation: 0,
        },
    );
    section.insert(
        6,
        Entry::Compressed {
            stream: 10,
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
            offset: 9,
            generation: 0,
        },
    );
    section.insert(
        5,
        Entry::Compressed {
            stream: 7,
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
                offset: 9,
                generation: 0,
            },
        ),
        (4, Entry::Free { generation: 2 }),
        (
            6,
            Entry::Compressed {
                stream: 1,
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
            offset: 9,
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
            offset: 50,
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

/// A section that lists a number a written file may not use, at or past
/// `NUMBERS`, is refused when it's written, before a row is made, and so
/// is a whole file's whose `/Size` is past it, and a stream numbered 0 or
/// past it: nothing is written. An update's `/Size` may be past it, as its
/// file's may, so long as the numbers it lists aren't.
#[test]
fn a_section_past_the_numbers_a_file_may_use_is_refused() {
    let entry = Entry::InUse {
        offset: 9,
        generation: 0,
    };
    let mut out = b"%PDF-1.7\n".to_vec();
    let past = |n| Err(Error::Number(n));
    for mut section in [Section::full(1), Section::append(3, 0)] {
        for n in [write::NUMBERS, write::NUMBERS + 1, u32::MAX - 1] {
            let mut listing = section.clone();
            listing.insert(n, entry);
            assert_eq!(listing.table(&mut out, Dict::new(), 20), past(n));
            let written = listing.stream(&mut out, 4, Dict::new(), 20, Deflate::DEFAULT);
            assert_eq!(written, past(n));
        }
        section.insert(2, entry);
        for n in [0, write::NUMBERS, u32::MAX] {
            let written = section.stream(&mut out, n, Dict::new(), 20, Deflate::DEFAULT);
            assert_eq!(written, past(n));
        }
    }
    let wide = Section::full(write::NUMBERS + 1);
    assert_eq!(wide.table(&mut out, Dict::new(), 20), past(write::NUMBERS));
    assert_eq!(out, b"%PDF-1.7\n", "nothing written");
    let mut wide = Section::append(u32::MAX, 0);
    wide.insert(2, entry);
    wide.table(&mut out, Dict::new(), 20).unwrap();
    assert!(holds(&out, b"/Size 4294967295"));
}

/// A container's members are numbers a written file may use, each once:
/// one numbered 0 or at or past `NUMBERS`, or a number given twice, is
/// refused. More than 65,536, more than a cross-reference stream's place
/// in it says, make none, told before anything is copied.
#[test]
fn a_container_refuses_what_a_file_can_t_list() {
    let deflate = Deflate::DEFAULT;
    let one: &[u8] = b"1";
    for n in [0, write::NUMBERS, u32::MAX] {
        let made = write::container(&[(1, one), (n, one)], deflate);
        assert_eq!(made, Err(Error::Number(n)), "{n}");
    }
    let twice = write::container(&[(5, one), (6, one), (5, one)], deflate);
    assert_eq!(twice, Err(Error::Twice(5)));
    let many: Vec<(u32, &[u8])> = (1..=65_537).map(|n| (n, one)).collect();
    assert_eq!(write::container(&many, deflate), Ok(None));
    let most = &many[..65_536];
    let made = write::container(most, deflate).unwrap().unwrap();
    assert_eq!(
        made.dict.get(b"N").and_then(Object::as_integer),
        Some(65_536)
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
/// `direct`, with [`ID`]. Its table lists those alone, in two
/// subsections, so that it lists no number free, which a whole file or an
/// update under its RC4 or AES-128 encryption would refuse an object at.
pub(crate) fn source(dict: Dict, direct: bool) -> Vec<u8> {
    let id = Object::hex(ID);
    let mut trailer = dict! { "Root" => Object::reference(1), "ID" => vec![id.clone(), id] };
    let mut out = b"%PDF-1.7\n".to_vec();
    let one = serialize::indirect(&mut out, (1, 0), &catalog()).unwrap();
    let mut rows = format!("xref\n0 2\n0000000000 65535 f \n{one:010} 00000 n \n");
    match direct {
        true => trailer.set("Encrypt", dict),
        false => {
            let at = serialize::indirect(&mut out, (ENCRYPT, 0), &Object::from(dict)).unwrap();
            rows += &format!("{ENCRYPT} 1\n{at:010} 00000 n \n");
            trailer.set("Encrypt", Object::reference(ENCRYPT));
        }
    }
    trailer.set("Size", i64::from(ENCRYPT + 1));
    let xref = out.len();
    out.extend_from_slice(rows.as_bytes());
    out.extend_from_slice(b"trailer\n");
    serialize::write(&mut out, &Object::from(trailer)).unwrap();
    out.extend_from_slice(format!("\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
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
    let encrypting = encryptor.encrypting(trailer, &resolve).unwrap();
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut section = Section::full(0);
    for (num, obj) in objects {
        let pos = serialize::indirect_with(&mut out, (*num, 0), obj, &encrypting).unwrap();
        section.insert(
            *num,
            Entry::InUse {
                offset: pos,
                generation: 0,
            },
        );
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
            section.insert(
                at.0,
                Entry::InUse {
                    offset: pos,
                    generation,
                },
            );
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
    let reading = assert_reads_to(objects, bytes, true, name);
    // The trailer's /ID as written, [`ID`] twice in hex, never decrypted.
    let id = reading.trailer().dict().get(b"ID").map(unplaced);
    let written = Object::from(vec![Object::hex(ID), Object::hex(ID)]);
    assert_eq!(id, Some(written), "{name}: the trailer's /ID as written");
    reading
}

/// `bytes`, opened by the user's password, which opens them where
/// they're `encrypted`, are `objects`, object by object, lazily and
/// whole, each stream's data handed out as it was built, with nothing to
/// warn of. Gives the reading.
pub(crate) fn assert_reads_to(
    objects: &[(u32, Object)],
    bytes: &[u8],
    encrypted: bool,
    name: &str,
) -> Reading<Arc<[u8]>> {
    let reading = opened(bytes, "user");
    assert_eq!(
        matches!(reading.encryption(), Encryption::Opened { .. }),
        encrypted,
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
        let encrypting = crypt.encrypting(&trailer, &resolve).unwrap();
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
        let encrypting = crypt.encrypting(&trailer, &resolve).unwrap();
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
            section.insert(
                *num,
                Entry::InUse {
                    offset: pos,
                    generation: 0,
                },
            );
        }
        let at = crypt.at().unwrap();
        let pos = serialize::indirect(&mut file, at, &Object::from(crypt.dictionary().clone()));
        section.insert(
            at.0,
            Entry::InUse {
                offset: pos.unwrap(),
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
    let encrypting = crypt.encrypting(&trailer, &resolve).unwrap();
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
        let encrypting = method.encryptor.encrypting(&trailer, &resolve).unwrap();
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut section = Section::full(0);
        let container = container(&members);
        for (num, obj) in [(1, catalog()), (9, container)] {
            let pos = serialize::indirect_with(&mut out, (num, 0), &obj, &encrypting).unwrap();
            section.insert(
                num,
                Entry::InUse {
                    offset: pos,
                    generation: 0,
                },
            );
        }
        for (index, (num, _)) in (0..).zip(&members) {
            section.insert(*num, Entry::Compressed { stream: 9, index });
        }
        let mut trailer = trailer.clone();
        let at = method.encryptor.at().unwrap_or((20, 0));
        let dict = Object::from(method.encryptor.dictionary().clone());
        let pos = serialize::indirect(&mut out, at, &dict).unwrap();
        section.insert(
            at.0,
            Entry::InUse {
                offset: pos,
                generation: 0,
            },
        );
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
    let encrypting = crypt.encrypting(&trailer, &resolve).unwrap();
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
    let encrypting = crypt.encrypting(&trailer, &resolve).unwrap();
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

// Whole files and updates (`write::full`, `document`, `update`), read
// back by the reader, by their password, and against what each refuses.

/// Both kinds of section.
const KINDS: [XrefKind; 2] = [XrefKind::Table, XrefKind::Stream];

/// `objects`, each at its number, generation 0, as a write's bodies.
fn bodies(objects: &[(u32, Object)]) -> Vec<(ObjectId, write::Body<'_>)> {
    (objects.iter())
        .map(|(n, o)| ((*n, 0), write::Body::from(o)))
        .collect()
}

/// A write's options: a section of `kind`, packed by `pack` when given.
fn options(kind: XrefKind, pack: Option<write::Pack<'_>>) -> write::Options<'_> {
    let mut options = write::Options::DEFAULT;
    options.kind = kind;
    options.pack = pack;
    options
}

/// `objects` written whole under `trailer`, a section of `kind`,
/// unpacked, encrypted by `encryptor` when given, its encryption
/// dictionary past every object.
fn whole(
    objects: &[(u32, Object)],
    trailer: &Dict,
    kind: XrefKind,
    encryptor: Option<&Encryptor>,
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut options = options(kind, None);
    options.encrypt_at = Some(past(objects));
    write::full(
        &mut out,
        bodies(objects),
        trailer,
        encryptor,
        None,
        &options,
    )
    .unwrap();
    out
}

/// The number past every one of `objects`'.
fn past(objects: &[(u32, Object)]) -> ObjectId {
    (objects.iter().map(|(n, _)| n + 1).max().unwrap_or(1), 0)
}

/// `objects` written whole under `trailer` with a cross-reference stream,
/// packed, grouped by `groups` when given, else by number, encrypted by
/// `encryptor` when given, its encryption dictionary past every object.
fn whole_packed(
    objects: &[(u32, Object)],
    trailer: &Dict,
    encryptor: Option<&Encryptor>,
    groups: Option<&write::Groups<'_>>,
) -> Vec<u8> {
    let mut pack = write::Pack::DEFAULT;
    pack.groups = groups;
    let mut out = Vec::new();
    let mut options = options(XrefKind::Stream, Some(pack));
    options.encrypt_at = Some(past(objects));
    write::full(
        &mut out,
        bodies(objects),
        trailer,
        encryptor,
        None,
        &options,
    )
    .unwrap();
    out
}

/// `bytes` read back, by the user's password where they're `encrypted`.
fn read_back(bytes: &[u8], encrypted: bool) -> Reading<Arc<[u8]>> {
    match encrypted {
        true => opened(bytes, "user"),
        false => open(bytes),
    }
}

/// [`plaintext`]'s page.
const PAGE: ObjectId = (4, 0);

/// `source`, opened by `password`, read whole, its page turned (its
/// `/Rotate` 90), and written whole under its own encryption, a section
/// of `kind`, packed when it's a stream, its streams' data read from the
/// reading.
fn rewrite(source: &[u8], password: &str, kind: XrefKind) -> Vec<u8> {
    let reading = opened(source, password);
    let mut doc = reading.to_document().unwrap();
    doc.get_dict_mut(PAGE).unwrap().set("Rotate", 90);
    let encryptor = Encryptor::of(&reading).unwrap();
    let pack = (kind == XrefKind::Stream).then_some(write::Pack::DEFAULT);
    let mut out = Vec::new();
    let options = options(kind, pack);
    write::document(&mut out, &doc, encryptor.as_ref(), Some(&reading), &options).unwrap();
    out
}

/// What `path`, key by key, reaches from `from` in `pdf`, each reference
/// followed: the last object a reference named, and what it reached.
fn reached(pdf: &Reading<Arc<[u8]>>, from: &Object, path: &[&[u8]]) -> (Option<ObjectId>, Object) {
    let follow = |o: &Object, id: Option<ObjectId>| match o.as_reference() {
        Some(at) => (Some(at), pdf.get(at).unwrap().clone()),
        None => (id, o.clone()),
    };
    let (mut id, mut obj) = follow(from, None);
    for key in path {
        let next = obj.as_dict().unwrap().get(key).unwrap().clone();
        (id, obj) = follow(&next, id);
    }
    (id, obj)
}

/// A rewrite of [`plaintext`], turned: its page's `/Rotate`, and what it
/// holds where [`plaintext`] put it: the Info title, the page's private
/// data, the XMP and the attachment's data.
fn rewritten(pdf: &Reading<Arc<[u8]>>) -> (i64, [Vec<u8>; 4]) {
    let trailer = Object::from(pdf.trailer().dict().clone());
    let (_, catalog) = reached(pdf, &trailer, &[b"Root"]);
    let (_, kids) = reached(pdf, &catalog, &[b"Pages", b"Kids"]);
    let page = kids.as_array().unwrap()[0].clone();
    let (_, files) = reached(pdf, &catalog, &[b"Names", b"EmbeddedFiles", b"Names"]);
    let text = |(_, o): (Option<ObjectId>, Object)| o.as_bytes().unwrap().to_vec();
    let data = |(id, _): (Option<ObjectId>, Object)| pdf.data(id.unwrap()).unwrap().into_owned();
    let held = [
        text(reached(pdf, &trailer, &[b"Info", b"Title"])),
        text(reached(pdf, &page, &[b"PieceInfo", b"Editor", b"Private"])),
        data(reached(pdf, &catalog, &[b"Metadata"])),
        data(reached(pdf, &files.as_array().unwrap()[1], &[b"EF", b"F"])),
    ];
    let (_, rotate) = reached(pdf, &page, &[b"Rotate"]);
    (rotate.as_integer().unwrap(), held)
}

/// What [`rewritten`] gives of every rewrite of [`plaintext`].
fn as_turned() -> (i64, [Vec<u8>; 4]) {
    (
        90,
        [TITLE, PAGE_STRING, XMP, ATTACHMENT].map(<[u8]>::to_vec),
    )
}

/// `pdf`'s object streams, by number.
fn object_streams_of(pdf: &Reading<Arc<[u8]>>) -> Vec<ObjectId> {
    let ids: Vec<ObjectId> = pdf.ids().collect();
    (ids.into_iter())
        .filter(|&id| {
            pdf.get(id)
                .is_ok_and(|o| o.as_stream().is_some_and(|s| s.dict.has_type(b"ObjStm")))
        })
        .collect()
}

/// The methods the tests encrypt a source by: RC4-40, RC4-128, AES-128,
/// and AES-256 of revisions 5 and 6.
fn standard(methods: &[Method]) -> impl Iterator<Item = &Method> {
    let names = ["RC4-40", "RC4-128", "AES-128", "AES-256 R5", "AES-256 R6"];
    methods.iter().filter(move |m| names.contains(&m.name))
}

/// `bytes` with their trailer's `/ID` cut to its first string, as some
/// writers write it, where it's [`ID`] twice in hex: RC4's and AES-128's
/// keys are made from that string all the same. Nothing before the
/// trailer moves, so the sections still say where each object is.
fn one_id(bytes: &[u8]) -> Vec<u8> {
    let hex: String = ID.iter().map(|b| format!("{b:02X}")).collect();
    let two = format!("/ID[<{hex}><{hex}>]");
    let one = format!("/ID[<{hex}>]");
    let at = (bytes.windows(two.len()).rposition(|w| w == two.as_bytes())).unwrap();
    [&bytes[..at], one.as_bytes(), &bytes[at + two.len()..]].concat()
}

/// [`plaintext`] encrypted by each of [`standard`]'s sources, with its
/// `/ID` of two strings and of one, a section of each kind: each with its
/// name, its kind and its method.
fn sources(methods: &[Method]) -> Vec<(String, XrefKind, &Method, Vec<u8>)> {
    let (plain, trailer) = plaintext();
    let mut sources = Vec::new();
    for method in standard(methods) {
        for kind in KINDS {
            let two = encrypted_by(&method.encryptor, &plain, &trailer, kind);
            let one = one_id(&two);
            for (ids, bytes) in [("/ID of two", two), ("/ID of one", one)] {
                let name = format!("{}, {kind:?}, {ids}", method.name);
                sources.push((name, kind, method, bytes));
            }
        }
    }
    sources
}

#[test]
fn full_writes_are_deterministic() {
    let (plain, trailer) = plaintext();
    let methods = methods();
    let encryptors = (methods.iter())
        .filter(|m| ["RC4-128", "AES-128", "AES-256 R6"].contains(&m.name))
        .map(|m| Some(&m.encryptor));
    for encryptor in std::iter::once(None).chain(encryptors) {
        for kind in KINDS {
            let name = format!("{kind:?}, encrypted: {}", encryptor.is_some());
            let write = || whole(&plain, &trailer, kind, encryptor);
            let once = write();
            let again = std::thread::scope(|scope| {
                let threads: Vec<_> = (0..2).map(|_| scope.spawn(write)).collect();
                (threads.into_iter())
                    .map(|t| t.join().unwrap())
                    .collect::<Vec<_>>()
            });
            assert_eq!(again, [once.clone(), once.clone()], "{name}");
            let pdf = read_back(&once, encryptor.is_some());
            assert!(sound(&pdf), "{name}: {:?}", pdf.warnings());
            assert_eq!(newest(&pdf).1, kind, "{name}");
        }
    }
}

/// [`plaintext`]'s page apart, in an object stream of its own, then the
/// rest by number, [`write::MEMBERS`] each: as a grouping by the pages
/// that use each object puts page 1's first.
fn page_apart(page: u32) -> impl Fn(&[u32]) -> Vec<Vec<u32>> + Sync {
    move |numbers: &[u32]| {
        let rest: Vec<u32> = numbers.iter().copied().filter(|&n| n != page).collect();
        let mut groups = vec![vec![page]];
        groups.extend(rest.chunks(write::MEMBERS).map(<[u32]>::to_vec));
        groups
    }
}

/// A packed write lists what it packs as its object stream and its place
/// there (7.5.8.3's type 2), and the reader reads every object back as it
/// was, lazily and whole. A stream, an object of generation 1 and a
/// signature dictionary as `packable` tells one (typed, untyped, and a
/// field merged with its `/FT /Sig`) stay where they are, listed by their
/// offsets, and the page has an object stream of its own, first, as the
/// caller's groups say.
#[test]
fn type_2_entries_read_back_lazily_and_eagerly() {
    let (plain, trailer) = plaintext();
    let older = (14, 1);
    let generation = Object::from(dict! { "Generation" => 1 });
    let mut given = bodies(&plain);
    given.push((older, write::Body::from(&generation)));
    let groups = page_apart(PAGE.0);
    let mut pack = write::Pack::DEFAULT;
    pack.groups = Some(&groups);
    let mut bytes = Vec::new();
    let options = options(XrefKind::Stream, Some(pack));
    write::full(&mut bytes, given, &trailer, None, None, &options).unwrap();
    let pdf = open(&bytes);
    assert!(sound(&pdf), "{:?}", pdf.warnings());
    assert_eq!(newest(&pdf).1, XrefKind::Stream);
    let xref = pdf.facts().max_stream.unwrap();
    let (index, _, got) = rows(&pdf, (xref, 0));
    assert_eq!(index, [0, i64::from(xref) + 1]);
    let eager = bytes::read(&bytes);
    let mut stayed = std::collections::BTreeSet::new();
    let mut containers: std::collections::BTreeMap<u64, Vec<(u64, ObjectId)>> =
        std::collections::BTreeMap::new();
    let all = plain
        .iter()
        .map(|(n, o)| ((*n, 0), o))
        .chain([(older, &generation)]);
    for (id, obj) in all.clone() {
        assert!(same(pdf.get(id).unwrap(), obj), "{id:?}");
        assert!(same(eager.get(id).unwrap(), obj), "whole: {id:?}");
        match got[id.0 as usize] {
            [1, _, generation] => {
                assert_eq!(generation, u64::from(id.1), "{id:?}");
                stayed.insert(id);
            }
            [2, container, index] => containers.entry(container).or_default().push((index, id)),
            row => panic!("{id:?}: {row:?}"),
        }
    }
    let streams = all.clone().filter(|(_, obj)| obj.as_stream().is_some());
    let signatures = all.filter(|(_, obj)| {
        matches!(obj.value(), Value::Dict(_))
            && obj.as_dict().is_some_and(|d| {
                let field = d.get(b"FT").and_then(Object::as_name) == Some(b"Sig");
                d.has_type(b"Sig") || d.has(b"ByteRange") || field
            })
    });
    let want: std::collections::BTreeSet<ObjectId> = (streams.chain(signatures).map(|(id, _)| id))
        .chain([older])
        .collect();
    assert_eq!(want.len(), 9);
    assert_eq!(stayed, want);
    // Two object streams: the page's, and the rest's.
    assert_eq!(containers.len(), 2);
    for (&container, members) in &containers {
        assert_eq!(got[container as usize][0], 1, "{container}");
        let stream = pdf.get((container as u32, 0)).unwrap().as_stream().unwrap();
        assert!(stream.dict.has_type(b"ObjStm"), "{container}");
        let n = stream.dict.get(b"N").unwrap().as_integer().unwrap();
        assert_eq!(n, members.len() as i64, "{container}");
        let places: Vec<u64> = members.iter().map(|&(index, _)| index).collect();
        assert_eq!(places, (0..n as u64).collect::<Vec<_>>(), "{container}");
    }
    let first = *containers.keys().next().unwrap();
    assert_eq!(got[PAGE.0 as usize], [2, first, 0], "the page's is first");
}

/// [`plaintext`] with `count` more objects, from 14, each a dictionary
/// with a string, its page naming the first quarter.
fn with_many(count: u32) -> (Vec<(u32, Object)>, Dict) {
    let (mut plain, trailer) = plaintext();
    let many: Vec<Object> = (14..14 + count).map(Object::reference).collect();
    for (n, obj) in &mut plain {
        if (*n, 0) == PAGE {
            let quarter = many[..count as usize / 4].to_vec();
            obj.as_dict_mut().unwrap().set("Many", quarter);
        }
    }
    for n in 14..14 + count {
        let text = lit(format!("object {n}").as_bytes());
        plain.push((n, Object::from(dict! { "N" => n, "Text" => text })));
    }
    (plain, trailer)
}

/// A packed write writes the same bytes every time: twice and on two
/// threads, plain and under every method that packs, with eleven object
/// streams to build on a thread per core.
#[test]
fn packed_writes_are_deterministic() {
    let (plain, trailer) = with_many(1000);
    let methods = methods();
    let packing =
        (methods.iter().filter(|m| m.encryptor.packs())).map(|m| (m.name, Some(&m.encryptor)));
    for (name, encryptor) in std::iter::once(("plain", None)).chain(packing) {
        let write = || whole_packed(&plain, &trailer, encryptor, None);
        let once = write();
        let again = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..2).map(|_| scope.spawn(write)).collect();
            (threads.into_iter())
                .map(|t| t.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(again, [once.clone(), once.clone()], "{name}");
        let containers = once.windows(12).filter(|w| w == b"/Type/ObjStm").count();
        assert_eq!(containers, 11, "{name}");
        let pdf = read_back(&once, encryptor.is_some());
        assert!(sound(&pdf), "{name}: {:?}", pdf.warnings());
        assert_eq!(object_streams_of(&pdf).len(), 11, "{name}");
    }
}

/// Object streams build the same on one thread as on eight, plain and
/// under every method that packs: forty of them, on eight threads in eight
/// runs of five. Stopped, neither builds.
#[test]
fn containers_build_alike_on_one_thread_and_on_eight() {
    let objects: Vec<(ObjectId, Object)> = (1..=4000)
        .map(|n| {
            let text = lit(format!("object {n}").as_bytes());
            ((n, 0), Object::from(dict! { "N" => n, "Text" => text }))
        })
        .collect();
    let groups: Vec<(u32, Vec<(ObjectId, super::copy::Member<'_>)>)> = (5000..)
        .zip(objects.chunks(100))
        .map(|(n, part)| (n, part.iter().map(|(id, o)| (*id, o.into())).collect()))
        .collect();
    let trailer = dict! { "Root" => (1, 0) };
    // The catalog given, which says which stream is the metadata where
    // `/EncryptMetadata false` leaves it in the clear.
    let resolve = |id: ObjectId| (objects.iter()).find(|(at, _)| *at == id).map(|(_, o)| o);
    let methods = methods();
    let encrypting: Vec<(&str, Option<write::Encrypting<'_>>)> = std::iter::once(("plain", None))
        .chain((methods.iter().filter(|m| m.encryptor.packs())).map(|m| {
            (
                m.name,
                Some(m.encryptor.encrypting(&trailer, &resolve).unwrap()),
            )
        }))
        .collect();
    for (name, encrypting) in &encrypting {
        let built = |threads, stop| {
            super::full::containers(
                &groups,
                None,
                encrypting.as_ref(),
                super::Numbering::NONE,
                Deflate::DEFAULT,
                threads,
                stop,
            )
        };
        let one = built(1, None).unwrap();
        assert_eq!(built(8, None).unwrap(), one, "{name}");
        assert_eq!(one.len(), 40, "{name}");
        for ((n, _), container) in groups.iter().zip(&one) {
            let container = container.as_ref().unwrap();
            assert!(
                container.starts_with(format!("{n} 0 obj\n").as_bytes()),
                "{name}: {n}"
            );
            assert!(holds(container, b"/Type/ObjStm/N 100"), "{name}: {n}");
        }
        let stop = std::sync::atomic::AtomicBool::new(true);
        for threads in [1, 8] {
            assert_eq!(built(threads, Some(&stop)), Err(Error::Stopped), "{name}");
        }
    }
}

/// Whether `method` encrypts `text`, one of [`plaintext`]'s.
fn encrypts(method: &Method, text: &[u8]) -> bool {
    match text {
        _ if text == CONTENT => method.streams,
        _ if text == XMP => method.metadata,
        _ if text == ATTACHMENT => method.attachment,
        _ => method.strings,
    }
}

/// A packed write under each method that packs encrypts each object
/// stream once, as a stream under its own number, and its members'
/// strings only with it: the reader reads every object back by the user's
/// password, and not without one; nothing is in the clear, even inflated,
/// but what the method leaves so and a signature's `/Contents`; the
/// `/Encrypt` dictionary and `/ID[0]` are the source's; and it writes the
/// same bytes twice.
#[test]
fn containers_encrypt_once_as_the_reader_decrypts() {
    let (plain, trailer) = plaintext();
    for method in methods().iter().filter(|m| m.encryptor.packs()) {
        let name = method.name;
        let crypt = &method.encryptor;
        let bytes = whole_packed(&plain, &trailer, Some(crypt), None);
        assert!(holds(&bytes, b"/Type/ObjStm"), "{name}");
        for text in SECRETS {
            let text_said = String::from_utf8_lossy(text);
            assert_eq!(
                in_clear(&bytes, text),
                !encrypts(method, text),
                "{name}: {text_said:?}"
            );
        }
        assert!(holds(&bytes, SIGNATURE_WRITTEN), "{name}");
        let locked = open(&bytes);
        assert!(
            !matches!(locked.encryption(), Encryption::Opened { .. }),
            "{name}"
        );
        let pdf = assert_reads_to(&plain, &bytes, true, name);
        let xref = pdf.facts().max_stream.unwrap();
        let (_, _, got) = rows(&pdf, (xref, 0));
        let packed = (plain.iter())
            .filter(|(n, _)| got[*n as usize][0] == 2)
            .count();
        assert_eq!(packed, 5, "{name}");
        let now = Encryptor::of(&pdf).unwrap().unwrap();
        let dictionaries = [now.dictionary(), crypt.dictionary()].map(as_written);
        assert_eq!(dictionaries[0], dictionaries[1], "{name}");
        if let Some(first) = crypt.first_id() {
            assert_eq!(now.first_id(), Some(first), "{name}");
        }
        let again = whole_packed(&plain, &trailer, Some(crypt), None);
        assert_eq!(again, bytes, "{name}");
    }
}

/// Under an encryption that leaves streams in the clear (`/StmF
/// /Identity`) but not strings, nothing is packed, so no string is in the
/// clear; with strings in the clear too, it packs.
#[test]
fn streams_in_the_clear_under_encrypted_strings_pack_nothing() {
    let (plain, trailer) = plaintext();
    let methods = methods();
    let by_name = |name| &methods.iter().find(|m| m.name == name).unwrap().encryptor;
    let crypt = by_name("streams in the clear");
    assert!(!crypt.packs());
    let bytes = whole_packed(&plain, &trailer, Some(crypt), None);
    assert!(!holds(&bytes, b"/ObjStm"));
    for text in [PAGE_STRING, TITLE, FIELD_NAME] {
        let text_said = String::from_utf8_lossy(text);
        assert!(!in_clear(&bytes, text), "{text_said:?}");
    }
    assert!(holds(&bytes, CONTENT));
    assert_reads_to(&plain, &bytes, true, "strings only");

    let crypt = by_name("attachments alone");
    assert!(crypt.packs());
    let bytes = whole_packed(&plain, &trailer, Some(crypt), None);
    assert!(holds(&bytes, b"/Type/ObjStm"));
    assert!(!in_clear(&bytes, ATTACHMENT));
    // Deflated in an object stream, a string a search of the bytes misses.
    assert!(!holds(&bytes, PAGE_STRING) && in_clear(&bytes, PAGE_STRING));
    assert_reads_to(&plain, &bytes, true, "attachment only");
}

/// A file's objects: a catalog (1), its page tree (2), a page (3) and its
/// content (4), and 250 dictionaries (10 to 259).
fn numbered() -> Vec<(u32, Object)> {
    let r = Object::reference;
    let mut objects = vec![
        (
            1,
            Object::from(dict! { "Type" => "Catalog", "Pages" => r(2) }),
        ),
        (
            2,
            Object::from(dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1 }),
        ),
        (
            3,
            Object::from(dict! {
                "Type" => "Page", "Parent" => r(2), "Contents" => r(4),
                "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(612), Object::from(792)],
            }),
        ),
        (
            4,
            Object::from(Stream::new(dict! {}, b"0 0 m 9 9 l S".to_vec())),
        ),
    ];
    for n in 10..260 {
        let text = lit(format!("object {n}").as_bytes());
        objects.push((n, Object::from(dict! { "Text" => text })));
    }
    objects
}

/// `objects` written whole with a cross-reference stream from
/// `xref_from`, packed with the page, 3, apart and object streams taking
/// `reuse`'s numbers first, encrypted with `encryptor` at object 6 when
/// given.
fn packed_from(
    objects: &[(u32, Object)],
    reuse: &[u32],
    xref_from: u32,
    encryptor: Option<&Encryptor>,
) -> Result<Vec<u8>, Error> {
    let id = Object::hex(ID);
    let trailer = dict! { "Root" => (1, 0), "ID" => vec![id.clone(), id] };
    let groups = page_apart(3);
    let mut pack = write::Pack::DEFAULT;
    pack.groups = Some(&groups);
    pack.reuse = reuse;
    let mut options = options(XrefKind::Stream, Some(pack));
    options.encrypt_at = Some((6, 0));
    options.xref_from = xref_from;
    let mut out = Vec::new();
    write::full(
        &mut out,
        bodies(objects),
        &trailer,
        encryptor,
        None,
        &options,
    )?;
    Ok(out)
}

/// Object streams take the numbers given to reuse first, in order,
/// passing over 0, what the file lists (a stream it writes on its own, an
/// object it packs, the `/Encrypt` dictionary), one given already and one
/// past what a file may use; then from past the last object and the least
/// the cross-reference stream takes, which goes after them.
#[test]
fn containers_take_the_source_s_numbers_first() {
    let objects = numbered();
    let methods = methods();
    let crypt = &methods
        .iter()
        .find(|m| m.name == "AES-128")
        .unwrap()
        .encryptor;
    let reuse = [0, 4, 3, write::NUMBERS, 6, 8, 8, 5];
    let cases = [
        ("plain", &reuse[..], 300, None, [6, 8, 5, 300], 301),
        (
            "encrypted",
            &reuse[..],
            300,
            Some(crypt),
            [8, 5, 300, 301],
            302,
        ),
        ("none to reuse", &[][..], 1, None, [260, 261, 262, 263], 264),
    ];
    for (name, reuse, xref_from, crypt, want, xref) in cases {
        let bytes = packed_from(&objects, reuse, xref_from, crypt).unwrap();
        let pdf = read_back(&bytes, crypt.is_some());
        assert!(sound(&pdf), "{name}: {:?}", pdf.warnings());
        assert_eq!(pdf.facts().max_stream, Some(xref), "{name}");
        assert_eq!(size(&pdf), xref + 1, "{name}");
        let (_, _, got) = rows(&pdf, (xref, 0));
        // The page's object stream, then the rest's: 1, 2 and 10 to 107;
        // 108 to 207; and 208 to 259.
        let containers = [3, 1, 108, 208].map(|n| got[n][1] as u32);
        assert_eq!(containers, want, "{name}");
        for (n, first) in want.into_iter().zip([3, 1, 108, 208]) {
            assert_eq!(got[first], [2, u64::from(n), 0], "{name}: {first}");
            assert_eq!(got[n as usize][0], 1, "{name}: {n}");
        }
        assert_eq!(got[4][0], 1, "{name}: the stream");
        for (n, obj) in &objects {
            assert!(same(pdf.get((*n, 0)).unwrap(), obj), "{name}: {n}");
        }
    }
}

/// Packing that would leave the cross-reference stream no number a file
/// may use writes every object on its own instead: four object streams
/// from the fifth number before the last leave it the last, and from the
/// fourth, none. Each whole section lists millions of numbers, deflated to
/// little, which the reader reads all of when it's told to: about 1.3 GB
/// at its peak.
#[test]
fn packing_runs_out_of_numbers_writes_plain() {
    let last = write::NUMBERS - 1;
    let objects = numbered();
    let open = |bytes: &[u8]| {
        let mut options = Options::DEFAULT;
        options.limits.entries = Some(write::NUMBERS as usize);
        Reading::open(Arc::<[u8]>::from(bytes), &options).unwrap()
    };
    let bytes = packed_from(&objects, &[], last - 4, None).unwrap();
    let pdf = open(&bytes);
    assert!(sound(&pdf), "{:?}", pdf.warnings());
    assert_eq!(pdf.facts().max_stream, Some(last));
    let (_, _, got) = rows(&pdf, (last, 0));
    assert_eq!(got[3], [2, u64::from(last - 4), 0]);
    assert_eq!(got[259], [2, u64::from(last - 1), 51]);

    // With no object stream, no object can be listed as in one.
    let bytes = packed_from(&objects, &[], last - 3, None).unwrap();
    assert!(!holds(&bytes, b"/ObjStm"));
    let pdf = open(&bytes);
    assert!(sound(&pdf), "{:?}", pdf.warnings());
    assert_eq!(pdf.facts().max_stream, Some(last - 3));
    for (n, obj) in &objects {
        assert!(same(pdf.get((*n, 0)).unwrap(), obj), "{n}");
    }
}

/// An object stream whose data would be past what the reader takes isn't
/// made: its members are written on their own, its number left free, and
/// the other object streams are made as ever. Object 11, a string as long
/// as an object stream's data may be, is in the second, with 1, 2 and 10
/// to 107: about 1.3 GB at its peak.
#[test]
fn a_container_past_what_the_reader_takes_is_written_plain() {
    let mut objects = numbered();
    for (n, obj) in &mut objects {
        if *n == 11 {
            *obj = Object::new(Value::String(
                Str::Bytes(vec![b'a'; write::MAX_DATA]),
                Some(Form::Literal),
            ));
        }
    }
    let bytes = packed_from(&objects, &[], 0, None).unwrap();
    let pdf = open(&bytes);
    assert!(sound(&pdf), "{:?}", pdf.warnings());
    let xref = pdf.facts().max_stream.unwrap();
    assert_eq!(xref, 264);
    let (_, _, got) = rows(&pdf, (xref, 0));
    for n in [1, 2].into_iter().chain(10..108) {
        assert_eq!(got[n][0], 1, "{n}");
    }
    assert_eq!(got[261][0], 0, "the second object stream's number");
    for (n, first) in [(260, 3), (262, 108), (263, 208)] {
        assert_eq!(got[n][0], 1, "{n}");
        assert_eq!(got[first], [2, n as u64, 0], "{first}");
    }
    assert_eq!(object_streams_of(&pdf), [(260, 0), (262, 0), (263, 0)]);
    for (n, obj) in &objects {
        assert!(same(pdf.get((*n, 0)).unwrap(), obj), "{n}");
    }
}

/// A cross-reference table can't list an object stream's members, so a
/// whole file with one, asked to pack, is refused.
#[test]
fn a_table_layout_refuses_to_pack() {
    let trailer = dict! { "Root" => (1, 0) };
    let mut out = b"before".to_vec();
    let options = options(XrefKind::Table, Some(write::Pack::DEFAULT));
    let e = write::full(&mut out, [], &trailer, None, None, &options).unwrap_err();
    assert_eq!(e, Error::PackedTable);
    assert_eq!(
        e.to_string(),
        "a cross-reference table can't list objects in object streams"
    );
    assert_eq!(out, b"before");
}

#[test]
fn a_full_write_keeps_the_trailer_but_the_bases_sections() {
    let linearized = dict! { "Linearized" => 1, "L" => 1000 };
    let objstm = Stream::new(dict! { "Type" => "ObjStm", "N" => 0, "First" => 0 }, vec![]);
    let xref = Stream::new(
        dict! { "Type" => "XRef", "W" => vec![Object::from(1)] },
        vec![],
    );
    let mut doc = crate::Document::new(pdf_objects::version::Version::new(1, 4), Dict::new());
    doc.insert(1, catalog());
    doc.insert(2, dict! { "Title" => "Info" });
    doc.insert(3, linearized);
    doc.insert(4, Object::from(objstm));
    doc.insert(5, Object::from(xref));
    doc.set((6, 2), Object::from(6));
    let id = Object::hex(ID);
    let mut base = dict! {
        "Root" => (1, 0),
        "Info" => (2, 0),
        "Custom" => "Kept",
        "ID" => vec![id.clone(), id],
        "Encrypt" => (9, 0),
    };
    for key in ["Prev", "XRefStm", "Size", "Length", "DL"] {
        base.set(key, 1000);
    }
    for key in [
        "Type",
        "W",
        "Index",
        "Filter",
        "DecodeParms",
        "F",
        "FFilter",
        "FDecodeParms",
    ] {
        base.set(key, "Stale");
    }
    *doc.trailer.dict_mut() = base;
    for kind in KINDS {
        let mut options = options(kind, None);
        options.version = pdf_objects::version::Version::new(1, 4);
        let mut bytes = Vec::new();
        write::document(&mut bytes, &doc, None, None, &options).unwrap();
        assert!(
            bytes.starts_with(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n"),
            "{kind:?}"
        );
        let pdf = open(&bytes);
        assert!(sound(&pdf), "{kind:?}: {:?}", pdf.warnings());
        assert_eq!(pdf.facts().sections, 1, "{kind:?}");
        // The linearization dictionary and the base's streams left out, and
        // the stream this section is, when it's one, listed.
        let (mut ids, own): (_, &[&[u8]]) = match kind {
            XrefKind::Table => (vec![(1, 0), (2, 0), (6, 2)], &[b"Size"]),
            XrefKind::Stream => (
                vec![(1, 0), (2, 0), (6, 2), (7, 0)],
                &[
                    b"Size",
                    b"Type",
                    b"W",
                    b"Index",
                    b"Length",
                    b"Filter",
                    b"DecodeParms",
                ],
            ),
        };
        assert_eq!(pdf.ids().collect::<Vec<_>>(), ids, "{kind:?}");
        // The base's keys but its sections', and this section's own.
        let trailer = pdf.trailer().dict();
        let mut kept: Vec<&[u8]> = trailer.iter().map(|(k, _)| k).collect();
        kept.retain(|k| !own.contains(k));
        kept.sort_unstable();
        assert_eq!(
            kept,
            [b"Custom".as_slice(), b"ID", b"Info", b"Root"],
            "{kind:?}"
        );
        let past = ids.pop().unwrap().0 + 1;
        assert_eq!(size(&pdf), past, "{kind:?}");
        assert_eq!(
            trailer.get(b"Custom").and_then(Object::as_name),
            Some(&b"Kept"[..])
        );
        // /ID[0] kept, and /ID[1] the hash of what comes before the section.
        let (start, _) = newest(&pdf);
        let written = twox_hash::XxHash3_128::oneshot(&bytes[..start]).to_be_bytes();
        let id = [ID, &written].map(|id| Object::hex(id.to_vec()));
        assert_eq!(
            trailer.get(b"ID").map(unplaced),
            Some(Object::from(id.to_vec()))
        );
    }
}

/// A whole file's body, or its cross-reference stream, at a number past
/// what a file may use is refused before the section lists a row: it
/// lists every number from 0, so it would grow without bound. So is a
/// body at 0, which heads the free list, where no section lists it.
#[test]
fn a_number_past_what_a_file_may_use_isnt_written() {
    let trailer = dict! { "Root" => (1, 0) };
    let catalog = catalog();
    let given = |at: u32| [(1, 0), (at, 0)].map(|id| (id, write::Body::from(&catalog)));
    let write = |at: u32, kind, xref_from| {
        let mut options = options(kind, None);
        options.xref_from = xref_from;
        let mut out = Vec::new();
        write::full(&mut out, given(at), &trailer, None, None, &options).map(|_| out)
    };
    for kind in KINDS {
        let e = write(write::NUMBERS, kind, 1).unwrap_err();
        assert_eq!(e, Error::Number(write::NUMBERS), "{kind:?}");
        assert!(e.to_string().contains("past the 8388607"), "{kind:?}: {e}");
        let e = write(0, kind, 1).unwrap_err();
        assert_eq!(e, Error::Number(0), "{kind:?}");
        let fits = write(2, kind, 1).unwrap();
        assert!(sound(&open(&fits)), "{kind:?}");
    }
    let stream = write(2, XrefKind::Stream, write::NUMBERS);
    assert_eq!(stream.unwrap_err(), Error::Number(write::NUMBERS));
}

/// An update lists only what it writes, in runs, its `/Size` past them,
/// and nothing when it writes nothing.
#[test]
fn an_update_lists_only_what_it_writes() {
    // Object 3 at generation 4; 2 rewritten, 4 added.
    let mut file = b"%PDF-1.7\n".to_vec();
    let mut full = Section::full(0);
    let objects = [((1, 0), catalog()), ((2, 0), 2.into()), ((3, 4), 3.into())];
    for (id, obj) in objects {
        let pos = serialize::indirect(&mut file, id, &obj).unwrap();
        let generation = id.1;
        full.insert(
            id.0,
            Entry::InUse {
                offset: pos,
                generation,
            },
        );
    }
    let pos = file.len();
    full.table(&mut file, dict! { "Root" => (1, 0) }, pos)
        .unwrap();
    let (twenty_two, four) = (Object::from(22), Object::from(4));
    let given =
        || [((2, 0), &twenty_two), ((4, 0), &four)].map(|(id, o)| (id, write::Body::from(o)));
    for kind in KINDS {
        let mut base = write::Base::new(
            file.len(),
            newest(&open(&file)).0,
            4,
            kind,
            dict! { "Root" => (1, 0) },
        );
        base.ends_line = true;
        let mut options = write::Options::DEFAULT;
        options.xref_from = 5;
        let mut out = file.clone();
        assert_eq!(
            write::update(&mut out, &base, [], None, None, &options),
            Ok(None)
        );
        assert_eq!(out, file);
        let update = (write::update(&mut out, &base, given(), None, None, &options))
            .unwrap()
            .unwrap();
        let pdf = open(&out);
        assert!(sound(&pdf), "{kind:?}: {:?}", pdf.warnings());
        assert_eq!(newest(&pdf), (update.start, kind));
        assert_eq!(pdf.facts().sections, 2, "{kind:?}");
        assert_eq!(size(&pdf), update.size, "{kind:?}");
        assert_eq!(value(&pdf, (2, 0)), Object::from(22));
        assert_eq!(value(&pdf, (3, 4)), Object::from(3));
        assert_eq!(value(&pdf, (4, 0)), Object::from(4));
        // Neither an /ID nor /Encrypt, with none to keep.
        let trailer = pdf.trailer().dict();
        assert!(!trailer.has(b"ID") && !trailer.has(b"Encrypt"));
        assert_eq!(update.id, None);
        match kind {
            XrefKind::Table => {
                assert_eq!(update.size, 5);
                let table = String::from_utf8_lossy(&out[file.len()..]);
                assert!(table.contains("xref\n2 1\n"), "{table}");
                assert!(table.contains("\n4 1\n"), "{table}");
            }
            XrefKind::Stream => {
                assert_eq!(update.size, 6);
                let (index, _, _) = rows(&pdf, (5, 0));
                assert_eq!(index, [2, 1, 4, 2], "{kind:?}");
            }
        }
        // Where each landed, from the header, and the file as it is now.
        let landed = |n| match update.section.get(n) {
            Some(Entry::InUse { offset: pos, .. }) => pos,
            entry => panic!("{n}: {entry:?}"),
        };
        assert!(out[landed(2)..].starts_with(b"2 0 obj"));
        assert!(out[landed(4)..].starts_with(b"4 0 obj"));
        assert_eq!(update.base.len, out.len());
        assert_eq!(update.base.prev, update.start);
        assert!(update.base.holds(&out[..]).unwrap());
    }
}

/// Bytes some writers put before `%PDF-`.
const PREFIX: [u8; 100] = [b'~'; 100];

/// The update of a file with 100 bytes before its header counts its
/// positions from there, so past them it's the same update as the file's
/// without: described by `Base::new`, with its header there, as a caller
/// that reads offsets from the header does. Read back, the reader finds
/// its objects by scanning, as it counts positions from the file's start,
/// and reads the same values; the file without them reads by its
/// sections.
#[test]
fn a_file_with_a_100_byte_prefix_appends() {
    let (plain, trailer) = plaintext();
    for kind in KINDS {
        let file = whole(&plain, &trailer, kind, None);
        let prefixed = [&PREFIX[..], &file].concat();
        let reading = open(&file);
        let mut page = reading.get(PAGE).unwrap().clone();
        page.as_dict_mut().unwrap().set("Rotate", 90);
        let turned = [(PAGE, write::Body::from(&page))];
        let base = write::Base::of(&reading).unwrap();
        let mut appended = file.clone();
        write::update(
            &mut appended,
            &base,
            turned.clone(),
            None,
            None,
            &write::Options::DEFAULT,
        )
        .unwrap();
        let mut after = write::Base::new(
            prefixed.len(),
            base.prev,
            base.size,
            kind,
            base.trailer.clone(),
        );
        after.header = PREFIX.len();
        after.ends_line = true;
        let mut after_prefix = prefixed.clone();
        let written = write::update(
            &mut after_prefix,
            &after,
            turned,
            None,
            None,
            &write::Options::DEFAULT,
        )
        .unwrap()
        .unwrap();
        // Positions count from the header, so past the prefix it's the same
        // update.
        assert!(after_prefix[PREFIX.len()..] == appended[..], "{kind:?}");
        assert_eq!(written.start, newest(&open(&appended)).0, "{kind:?}");
        // Its section's positions are the file's, past the prefix, where
        // its start counts from the header.
        let Some(Entry::InUse { offset, .. }) = written.section.get(PAGE.0) else {
            panic!("{kind:?}: no page")
        };
        assert!(after_prefix[offset..].starts_with(b"4 0 obj"), "{kind:?}");
        let unprefixed = open(&appended);
        assert!(sound(&unprefixed), "{kind:?}: {:?}", unprefixed.warnings());
        assert_eq!(unprefixed.facts().sections, 2, "{kind:?}");
        assert_eq!(rewritten(&unprefixed), as_turned(), "{kind:?}");
        let pdf = open(&after_prefix);
        assert_eq!(pdf.facts().header_offset, Some(PREFIX.len()), "{kind:?}");
        assert!(pdf.trailer().unwritten.is_some(), "scanned: {kind:?}");
        for (n, _) in &plain {
            let id = (*n, 0);
            assert_eq!(
                handed(&pdf, id),
                handed(&unprefixed, id),
                "{kind:?}: {id:?}"
            );
        }
        assert_eq!(rewritten(&pdf), as_turned(), "{kind:?}");
    }
}

/// What a mark's annotation says.
const NOTE: &[u8] = b"a mark's note";

/// An encrypted file appends encrypted, under each method, its `/ID` of
/// two strings and of one, after a section of each kind: its page turned
/// and marked (an annotation added at a new number). The page's string and
/// the mark's note are encrypted, but where the method leaves strings in
/// the clear; the update reads back, lazily and whole, its `/Encrypt`
/// where it was, as stored, `/ID[0]` kept and `/ID[1]` new; the same
/// bytes twice.
#[test]
fn encrypted_files_append_encrypted() {
    let methods = methods();
    let mut sources = sources(&methods);
    let (plain, trailer) = plaintext();
    let alone = methods
        .iter()
        .find(|m| m.name == "attachments alone")
        .unwrap();
    for kind in KINDS {
        let bytes = encrypted_by(&alone.encryptor, &plain, &trailer, kind);
        sources.push((format!("{}, {kind:?}", alone.name), kind, alone, bytes));
    }
    for (name, kind, method, source) in &sources {
        let reading = opened(source, "user");
        let mut doc = reading.to_document().unwrap();
        let note =
            doc.add(dict! { "Type" => "Annot", "Subtype" => "Square", "Contents" => lit(NOTE) });
        let page = doc.get_dict_mut(PAGE).unwrap();
        page.set("Rotate", 90);
        page.set("Annots", vec![Object::from(note)]);
        let base = write::Base::of(&reading).unwrap();
        let encryptor = Encryptor::of(&reading).unwrap();
        let given = || [PAGE, note].map(|id| (id, write::Body::from(doc.get(id).unwrap())));
        let update = || {
            let mut out = source.clone();
            let options = write::Options::DEFAULT;
            write::update(
                &mut out,
                &base,
                given(),
                encryptor.as_ref(),
                Some(&reading),
                &options,
            )
            .unwrap()
            .unwrap();
            out
        };
        let bytes = update();
        assert_eq!(update(), bytes, "{name}");
        assert!(bytes.starts_with(source), "{name}");
        let section = &bytes[source.len()..];
        for text in [PAGE_STRING, NOTE] {
            let text_said = String::from_utf8_lossy(text);
            assert_eq!(
                in_clear(section, text),
                !method.strings,
                "{name}: {text_said:?}"
            );
        }
        // The new revision, lazily and whole.
        let pdf = opened(&bytes, "user");
        assert!(sound(&pdf), "{name}: {:?}", pdf.warnings());
        assert_eq!(newest(&pdf).1, *kind, "{name}");
        assert_eq!(pdf.facts().sections, 2, "{name}");
        assert_eq!(rewritten(&pdf), as_turned(), "{name}");
        let (_, annot) = reached(&pdf, &Object::from(PAGE), &[b"Annots"]);
        let (_, contents) = reached(&pdf, &annot.as_array().unwrap()[0], &[b"Contents"]);
        assert_eq!(contents.as_bytes(), Some(NOTE), "{name}");
        let eager = bytes::read_opened(&bytes, &[Password("user")]);
        for id in pdf.ids() {
            assert_eq!(
                unplaced(eager.get(id).unwrap()),
                value(&pdf, id),
                "{name}: {id:?}"
            );
        }
        // Its /Encrypt where it was, as stored; /ID[0] kept and /ID[1] new.
        let encrypt = |pdf: &Reading<Arc<[u8]>>| pdf.trailer().dict().get(b"Encrypt").map(unplaced);
        assert_eq!(encrypt(&pdf), encrypt(&reading), "{name}");
        assert!(encrypt(&pdf).unwrap().as_reference().is_some(), "{name}");
        let now = Encryptor::of(&pdf).unwrap().unwrap();
        let was = encryptor.as_ref().unwrap();
        assert_eq!(
            as_written(now.dictionary()),
            as_written(was.dictionary()),
            "{name}"
        );
        let ids = |pdf: &Reading<Arc<[u8]>>| {
            let id = pdf
                .trailer()
                .dict()
                .get(b"ID")
                .unwrap()
                .as_array()
                .unwrap()
                .to_vec();
            id.iter()
                .map(|s| s.as_bytes().unwrap().to_vec())
                .collect::<Vec<_>>()
        };
        let (now, was) = (ids(&pdf), ids(&reading));
        assert_eq!(now[0], ID, "{name}");
        assert_eq!(now.len(), 2, "{name}");
        assert_ne!(now.get(1), was.get(1), "{name}");
    }
}

/// A file whose cross-reference stream is numbered 4,294,967,295, which
/// the reader takes: an update would have no number left past it for its
/// own, so it's refused, and nothing is written.
#[test]
fn a_base_numbered_to_the_last_number_isnt_appended_to() {
    let mut file = b"%PDF-1.7\n".to_vec();
    let mut rows = vec![0, 0, 0, 0, 0, 0xff, 0xff];
    let r = Object::reference;
    let media = vec![
        Object::from(0),
        Object::from(0),
        Object::from(200),
        Object::from(200),
    ];
    let objects = [
        dict! { "Type" => "Catalog", "Pages" => r(2) },
        dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1 },
        dict! { "Type" => "Page", "Parent" => r(2), "MediaBox" => media },
    ];
    for (num, dict) in (1..).zip(objects) {
        let pos = serialize::indirect(&mut file, (num, 0), &dict.into()).unwrap();
        rows.push(1);
        rows.extend_from_slice(&(pos as u32).to_be_bytes());
        rows.extend_from_slice(&[0, 0]);
    }
    let start = file.len();
    let xref = dict! {
        "Type" => "XRef",
        "Size" => 4,
        "W" => vec![Object::from(1), Object::from(4), Object::from(2)],
        "Index" => vec![Object::from(0), Object::from(4)],
        "Root" => r(1),
    };
    let stream = Object::from(Stream::new(xref, rows));
    serialize::indirect(&mut file, (u32::MAX, 0), &stream).unwrap();
    file.extend_from_slice(format!("startxref\n{start}\n%%EOF\n").as_bytes());
    let reading = open(&file);
    assert_eq!(reading.facts().max_stream, Some(u32::MAX));
    let base = write::Base::of(&reading).unwrap();
    assert_eq!(base.size, u32::MAX);
    let mut page = reading.get((3, 0)).unwrap().clone();
    page.as_dict_mut().unwrap().set("Rotate", 90);
    let mut out = file.clone();
    let given = [((3, 0), write::Body::from(&page))];
    let refused = write::update(
        &mut out,
        &base,
        given,
        None,
        Some(&reading),
        &write::Options::DEFAULT,
    );
    let refused = refused.unwrap_err();
    assert_eq!(refused, Error::NoNumber(u32::MAX));
    assert!(
        refused.to_string().contains("no object number left"),
        "{refused}"
    );
    assert_eq!(out, file);
}

/// An encrypted file rewrites encrypted every way, packed when its source
/// has a cross-reference stream: nothing is in the clear, even inflated;
/// the empty password doesn't open it; it reads back as it was, turned;
/// its `/Encrypt` as stored and its `/ID[0]` are the source's.
#[test]
fn rewrites_stay_encrypted() {
    let methods = methods();
    for (name, kind, _, source) in sources(&methods) {
        let bytes = rewrite(&source, "user", kind);
        for text in SECRETS {
            let text_said = String::from_utf8_lossy(text);
            assert!(!in_clear(&bytes, text), "{name}: {text_said:?}");
        }
        let locked = open(&bytes);
        assert_eq!(locked.encryption(), &Encryption::Locked, "{name}");
        let pdf = opened(&bytes, "user");
        assert!(sound(&pdf), "{name}: {:?}", pdf.warnings());
        assert_eq!(newest(&pdf).1, kind, "{name}");
        let packs = kind == XrefKind::Stream;
        assert_eq!(!object_streams_of(&pdf).is_empty(), packs, "{name}");
        assert_eq!(rewritten(&pdf), as_turned(), "{name}");
        let (was, now) = (encryptor_of(&source), Encryptor::of(&pdf).unwrap().unwrap());
        assert_eq!(
            as_written(now.dictionary()),
            as_written(was.dictionary()),
            "{name}"
        );
        assert_eq!(now.first_id(), Some(ID), "{name}");
        assert_eq!(now.at(), was.at(), "{name}");
        // Two strings, whatever the source had.
        let id = pdf.trailer().dict().get(b"ID").and_then(Object::as_array);
        assert_eq!(id.map(<[Object]>::len), Some(2), "{name}");
    }
}

/// A file whose `/EFF` encrypts its attachments alone rewrites with only
/// its attachment encrypted, packed when its source has a cross-reference
/// stream: every other string and stream is in the clear, if deflated in
/// an object stream.
#[test]
fn an_eff_file_rewrites_with_only_its_attachment_encrypted() {
    let (plain, trailer) = plaintext();
    let methods = methods();
    let alone = &methods
        .iter()
        .find(|m| m.name == "attachments alone")
        .unwrap()
        .encryptor;
    for kind in KINDS {
        let source = encrypted_by(alone, &plain, &trailer, kind);
        let bytes = rewrite(&source, "user", kind);
        assert!(!in_clear(&bytes, ATTACHMENT), "{kind:?}");
        for text in [TITLE, PAGE_STRING, FORM_STRING, CONTENT, XMP] {
            let text_said = String::from_utf8_lossy(text);
            assert!(in_clear(&bytes, text), "{kind:?}: {text_said:?}");
        }
        let pdf = opened(&bytes, "user");
        let packed = !object_streams_of(&pdf).is_empty();
        assert_eq!(packed, kind == XrefKind::Stream, "{kind:?}");
        assert_eq!(rewritten(&pdf), as_turned(), "{kind:?}");
        // Its /Encrypt as stored, /EFF and all.
        let now = Encryptor::of(&pdf).unwrap().unwrap();
        assert_eq!(
            as_written(now.dictionary()),
            as_written(alone.dictionary()),
            "{kind:?}"
        );
        assert!(now.dictionary().has(b"EFF"), "{kind:?}");
    }
}

/// A file whose objects are in an object stream encrypted whole reads
/// each as it was, lazily and whole, and rewrites encrypted and packed
/// again: each object stream encrypted whole, nothing in the clear, even
/// inflated.
#[test]
fn an_encrypted_object_stream_base_rewrites() {
    let (plain, trailer) = plaintext();
    let methods = methods();
    for method in standard(&methods) {
        let name = method.name;
        let base = whole_packed(&plain, &trailer, Some(&method.encryptor), None);
        let pdf = assert_reads_to(&plain, &base, true, name);
        assert!(!object_streams_of(&pdf).is_empty(), "{name}");
        let bytes = rewrite(&base, "user", XrefKind::Stream);
        for bytes in [&base, &bytes] {
            for text in SECRETS {
                let text_said = String::from_utf8_lossy(text);
                assert!(!in_clear(bytes, text), "{name}: {text_said:?}");
            }
        }
        let pdf = opened(&bytes, "user");
        assert!(!object_streams_of(&pdf).is_empty(), "{name}");
        assert!(sound(&pdf), "{name}: {:?}", pdf.warnings());
        assert_eq!(newest(&pdf).1, XrefKind::Stream, "{name}");
        assert_eq!(rewritten(&pdf), as_turned(), "{name}");
        let now = Encryptor::of(&pdf).unwrap().unwrap();
        let dictionaries = [now.dictionary(), method.encryptor.dictionary()].map(as_written);
        assert_eq!(dictionaries[0], dictionaries[1], "{name}");
    }
}

/// [`plaintext`] under `encryptor`, which leaves streams in the clear and
/// strings not, as a writer that packs anyway writes it: every object but
/// the streams in one object stream, its members' strings in the clear in
/// it, as its stream method leaves it; and a cross-reference stream.
fn packed_in_the_clear(encryptor: &Encryptor) -> Vec<u8> {
    let (plain, trailer) = plaintext();
    let resolve = |id: ObjectId| (plain.iter()).find(|(n, _)| (*n, 0) == id).map(|(_, o)| o);
    let encrypting = encryptor.encrypting(&trailer, &resolve).unwrap();
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut section = Section::full(0);
    let (streams, members): (Vec<_>, Vec<_>) =
        plain.iter().partition(|(_, o)| o.as_stream().is_some());
    for (num, obj) in streams {
        let pos = serialize::indirect_with(&mut out, (*num, 0), obj, &encrypting).unwrap();
        section.insert(
            *num,
            Entry::InUse {
                offset: pos,
                generation: 0,
            },
        );
    }
    let members: Vec<(u32, Object)> = members.into_iter().cloned().collect();
    let container = container(&members);
    let pos = serialize::indirect_with(&mut out, (20, 0), &container, &encrypting).unwrap();
    section.insert(
        20,
        Entry::InUse {
            offset: pos,
            generation: 0,
        },
    );
    for (index, (num, _)) in (0..).zip(&members) {
        section.insert(*num, Entry::Compressed { stream: 20, index });
    }
    let mut trailer = trailer.clone();
    let at = encryptor.at().unwrap();
    let pos =
        serialize::indirect(&mut out, at, &Object::from(encryptor.dictionary().clone())).unwrap();
    section.insert(
        at.0,
        Entry::InUse {
            offset: pos,
            generation: 0,
        },
    );
    trailer.set("Encrypt", Object::from(at));
    let pos = out.len();
    let own = section.size();
    section
        .stream(&mut out, own, trailer, pos, Deflate::DEFAULT)
        .unwrap();
    out
}

/// A file whose streams are in the clear and strings not (`/StmF
/// /Identity` under an encrypting `/StrF`) rewrites unpacked, whether it
/// has a table, a stream, or an object stream holding its members'
/// strings in the clear: no string is in the clear after.
#[test]
fn identity_stmf_under_an_encrypting_strf_isn_t_packed() {
    let (plain, trailer) = plaintext();
    let methods = methods();
    let crypt = &methods
        .iter()
        .find(|m| m.name == "streams in the clear")
        .unwrap()
        .encryptor;
    let stream = encrypted_by(crypt, &plain, &trailer, XrefKind::Stream);
    let packed = packed_in_the_clear(crypt);
    assert!(
        in_clear(&packed, PAGE_STRING),
        "the source's object stream is"
    );
    for (kind, base) in [("stream", stream), ("packed", packed)] {
        let out = rewrite(&base, "user", XrefKind::Stream);
        let pdf = opened(&out, "user");
        assert!(sound(&pdf), "{kind}: {:?}", pdf.warnings());
        assert_eq!(newest(&pdf).1, XrefKind::Stream, "{kind}");
        assert_eq!(object_streams_of(&pdf), [], "{kind}");
        for text in [PAGE_STRING, TITLE, FIELD_NAME] {
            let text_said = String::from_utf8_lossy(text);
            assert!(!in_clear(&out, text), "{kind}: {text_said:?}");
        }
        assert!(holds(&out, CONTENT), "{kind}");
        assert_eq!(rewritten(&pdf), as_turned(), "{kind}");
    }
}

/// `file`, opened by `password`, read whole, and written whole three ways
/// (a table; a stream, unpacked and packed) and as an update of its
/// catalog with a key more, under its own encryption, its streams' data
/// read from the reading; each read back by the same password: sound,
/// encrypted as it was, every object a whole write keeps the same, spans
/// aside, a stream's data as handed out, and the update's catalog
/// changed. Each object read at an offset that a whole write writes on
/// its own is copied as the bytes it was read from, where the rules for a
/// copy hold of its record.
fn assert_rewrites(file: &[u8], password: &str, name: &str) {
    let reading = opened(file, password);
    let encryptor = Encryptor::of(&reading).unwrap();
    let encrypted = encryptor.is_some();
    let doc = reading.to_document().unwrap();
    let left_out = |o: &Object| {
        o.as_stream()
            .is_some_and(|s| s.dict.has_type(b"ObjStm") || s.dict.has_type(b"XRef"))
            || o.as_dict().is_some_and(|d| d.has(b"Linearized"))
    };
    let kept: Vec<ObjectId> = (doc.objects())
        .filter(|(_, o)| !left_out(o))
        .map(|(id, _)| id)
        .collect();
    assert!(!kept.is_empty(), "{name}");
    let ways = [
        (XrefKind::Table, None),
        (XrefKind::Stream, None),
        (XrefKind::Stream, Some(write::Pack::DEFAULT)),
    ];
    for (kind, pack) in ways {
        let name = format!("{name}, {kind:?}, packed: {}", pack.is_some());
        let mut out = Vec::new();
        let options = options(kind, pack);
        let written =
            write::document(&mut out, &doc, encryptor.as_ref(), Some(&reading), &options).unwrap();
        let back = opened(&out, password);
        assert!(sound(&back), "{name}: {:?}", back.warnings());
        let opened = matches!(back.encryption(), Encryption::Opened { .. });
        assert_eq!(opened, encrypted, "{name}");
        assert_eq!(newest(&back).1, kind, "{name}");
        let encryption = (doc.trailer.dict().get(b"Encrypt")).and_then(Object::as_reference);
        let mut copies = 0;
        for &id in &kept {
            assert_eq!(handed(&back, id), handed(&reading, id), "{name}: {id:?}");
            // But the encryption dictionary, which the writer writes itself.
            if Some(id) == encryption {
                continue;
            }
            let record = doc.record(id).unwrap();
            let copied_ = copied(&out, &written, record, &reading, id);
            let at = matches!(written.section.get(id.0), Some(Entry::InUse { .. }));
            let copyable = record.origin == crate::Origin::File
                && record.endobj
                && (record.data).is_none_or(|d| d.declared && !d.indirect_length);
            assert_eq!(copied_, at && copyable, "{name}: {id:?} copied");
            copies += usize::from(copied_);
        }
        assert!(copies > 0 || pack.is_some(), "{name}: none copied");
    }
    let root = doc
        .trailer
        .dict()
        .get(b"Root")
        .and_then(Object::as_reference)
        .unwrap();
    let mut catalog = doc.get(root).unwrap().clone();
    catalog.as_dict_mut().unwrap().set("Changed", true);
    let base = write::Base::of(&reading).unwrap();
    let mut out = file.to_vec();
    let given = [(root, write::Body::from(&catalog))];
    let options = write::Options::DEFAULT;
    write::update(
        &mut out,
        &base,
        given,
        encryptor.as_ref(),
        Some(&reading),
        &options,
    )
    .unwrap();
    let back = opened(&out, password);
    let name = format!("{name}, updated");
    assert!(sound(&back), "{name}: {:?}", back.warnings());
    assert_eq!(
        back.facts().sections,
        reading.facts().sections + 1,
        "{name}"
    );
    for &id in &kept {
        let (got, was) = (handed(&back, id), handed(&reading, id));
        match id == root {
            true => assert_eq!(
                got.as_dict()
                    .unwrap()
                    .get(b"Changed")
                    .and_then(Object::as_bool),
                Some(true),
                "{name}"
            ),
            false => assert_eq!(got, was, "{name}: {id:?}"),
        }
    }
}

/// The files qpdf wrote, committed, where they are, each by the empty
/// password, and [`plaintext`] under each method, each kind of section,
/// packed or not, by the user's: read, written whole and as an update, and
/// read again, the same objects.
#[test]
fn written_files_read_back_alike_whole_and_updated() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/files");
    let mut fixtures: Vec<_> = match bytes::lazy::fixtures() {
        true => (std::fs::read_dir(dir).unwrap())
            .map(|entry| entry.unwrap().path())
            .collect(),
        false => Vec::new(),
    };
    fixtures.sort();
    assert_eq!(fixtures.len(), if bytes::lazy::fixtures() { 8 } else { 0 });
    for path in fixtures {
        let file = std::fs::read(&path).unwrap();
        assert_rewrites(&file, "", &path.display().to_string());
    }
    let (plain, trailer) = plaintext();
    for kind in KINDS {
        assert_rewrites(
            &whole(&plain, &trailer, kind, None),
            "",
            &format!("plain, {kind:?}"),
        );
    }
    assert_rewrites(
        &whole_packed(&plain, &trailer, None, None),
        "",
        "plain, packed",
    );
    for method in methods() {
        for kind in KINDS {
            let file = encrypted_by(&method.encryptor, &plain, &trailer, kind);
            assert_rewrites(&file, "user", &format!("{}, {kind:?}", method.name));
        }
        if method.encryptor.packs() {
            let file = whole_packed(&plain, &trailer, Some(&method.encryptor), None);
            assert_rewrites(&file, "user", &format!("{}, packed", method.name));
        }
    }
}

/// What each method leaves in the clear of [`listed`]'s strings and data:
/// each, and whether it's encrypted, as the list has it.
fn listed_rows(method: &Method) -> Vec<(&'static [u8], bool)> {
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
    rows
}

/// Under each method, [`listed`] written whole (a table, and a stream
/// packed) and as an update rewriting every object of a file of it,
/// twice each: the same bytes twice; what the list encrypts not in the
/// clear, as it is, in hex or inflated, and what it leaves so in the
/// clear; and every object read back by the user's password.
#[test]
fn each_method_writes_whole_packed_and_updated_with_nothing_in_the_clear() {
    let (plain, trailer) = listed();
    for method in methods() {
        let crypt = Some(&method.encryptor);
        let source = encrypted_by(&method.encryptor, &plain, &trailer, XrefKind::Table);
        let reading = opened(&source, "user");
        let base = write::Base::of(&reading).unwrap();
        let own = Encryptor::of(&reading).unwrap();
        let updated = || {
            let mut out = source.clone();
            let options = write::Options::DEFAULT;
            write::update(
                &mut out,
                &base,
                bodies(&plain),
                own.as_ref(),
                None,
                &options,
            )
            .unwrap();
            out
        };
        let ways: [(&str, &dyn Fn() -> Vec<u8>); 3] = [
            ("whole", &|| whole(&plain, &trailer, XrefKind::Table, crypt)),
            ("packed", &|| whole_packed(&plain, &trailer, crypt, None)),
            ("updated", &updated),
        ];
        for (way, write) in ways {
            let name = format!("{}, {way}", method.name);
            let bytes = write();
            assert_eq!(write(), bytes, "{name}");
            let written = match way {
                "updated" => &bytes[source.len()..],
                _ => &bytes[..],
            };
            for (text, encrypted) in listed_rows(&method) {
                let text_said = String::from_utf8_lossy(text);
                assert_eq!(in_clear(written, text), !encrypted, "{name}: {text_said:?}");
            }
            assert_reads_to(&plain, &bytes, true, &name);
        }
    }
}

/// An encrypted file written whole without an encryptor is decrypted:
/// its values as they were, its data in the clear, no `/Encrypt`, its
/// encryption dictionary left out; and under a new encryption of
/// revision 6, it opens by the new password alone, nothing in the clear,
/// the source's dictionary left out too. An RC4 source's as an AES-256
/// one's.
#[test]
fn a_decrypting_write_and_a_new_revision_6_read_back() {
    use chacha20::ChaCha20Rng;

    let (plain, trailer) = plaintext();
    let methods = methods();
    let prepared = Password("new").prepared().unwrap();
    for method in methods
        .iter()
        .filter(|m| ["RC4-40", "AES-256 R6"].contains(&m.name))
    {
        let source = encrypted_by(&method.encryptor, &plain, &trailer, XrefKind::Table);
        let reading = opened(&source, "user");
        let doc = reading.to_document().unwrap();
        let encrypt = doc
            .trailer
            .dict()
            .get(b"Encrypt")
            .and_then(Object::as_reference);
        assert_eq!(encrypt, Some((ENCRYPT, 0)), "{}", method.name);
        let mut rng = ChaCha20Rng::seed_from_u64(9);
        let created = Encryptor::create(&prepared, true, &mut rng).unwrap();
        for (way, encryptor) in [("decrypted", None), ("created", Some(&created))] {
            let name = format!("{}, {way}", method.name);
            let mut out = Vec::new();
            let options = options(XrefKind::Stream, Some(write::Pack::DEFAULT));
            write::document(&mut out, &doc, encryptor, Some(&reading), &options).unwrap();
            let back = match encryptor {
                Some(_) => opened(&out, "new"),
                None => open(&out),
            };
            assert!(sound(&back), "{name}: {:?}", back.warnings());
            assert!(!back.ids().any(|id| id == (ENCRYPT, 0)), "{name}");
            for (n, obj) in &plain {
                let id = (*n, 0);
                assert!(same(back.get(id).unwrap(), obj), "{name}: {id:?}");
                if let Some(s) = obj.as_stream() {
                    assert_eq!(
                        back.data(id).unwrap().as_ref(),
                        s.data.bytes().unwrap(),
                        "{name}"
                    );
                }
            }
            match encryptor {
                None => {
                    assert_eq!(back.encryption(), &Encryption::None, "{name}");
                    assert!(!back.trailer().dict().has(b"Encrypt"), "{name}");
                    assert!(holds(&out, CONTENT), "{name}");
                }
                Some(_) => {
                    assert!(
                        matches!(back.encryption(), Encryption::Opened { .. }),
                        "{name}"
                    );
                    assert!(!matches!(
                        opened(&out, "user").encryption(),
                        Encryption::Opened { .. }
                    ));
                    for text in SECRETS {
                        let text_said = String::from_utf8_lossy(text);
                        assert!(!in_clear(&out, text), "{name}: {text_said:?}");
                    }
                    // /ID[0] the document's, which the new key isn't made
                    // from, and /ID[1] of the bytes written.
                    let id = back
                        .trailer()
                        .dict()
                        .get(b"ID")
                        .and_then(Object::as_array)
                        .unwrap();
                    assert_eq!(id.len(), 2, "{name}");
                    assert_eq!(id[0].as_bytes(), Some(ID), "{name}");
                    assert_ne!(id[1].as_bytes(), Some(ID), "{name}");
                }
            }
        }
    }
}

/// The string `Title` of object 13 in `file`, as written: read with no
/// password, which opens none of the tests' files, so its ciphertext.
fn title_written(file: &[u8]) -> Vec<u8> {
    let locked = open(file);
    assert_eq!(locked.encryption(), &Encryption::Locked);
    let info = locked.get((13, 0)).unwrap().as_dict().unwrap();
    match info.get(b"Title").unwrap().value() {
        Value::String(Str::Encrypted(bytes), _) => bytes.clone(),
        other => panic!("{other:?}"),
    }
}

/// Under RC4, an update that writes an object again at its number and
/// generation, a string of it changed, encrypts it with the keystream its
/// earlier version was: the XOR of the two ciphertexts, both in the file,
/// is the XOR of the two plaintexts, as `write::update`'s doc and
/// pdf-crypt's `Method::Rc4` say. Under AES-128, it isn't.
#[test]
fn an_rc4_update_leaks_the_xor_of_what_changed() {
    const CHANGED: &[u8] = b"another title";
    assert_eq!(CHANGED.len(), TITLE.len());
    let (plain, trailer) = plaintext();
    let xor = |a: &[u8], b: &[u8]| a.iter().zip(b).map(|(a, b)| a ^ b).collect::<Vec<u8>>();
    let methods = methods();
    for method in methods
        .iter()
        .filter(|m| ["RC4-128", "AES-128"].contains(&m.name))
    {
        let source = encrypted_by(&method.encryptor, &plain, &trailer, XrefKind::Table);
        let reading = opened(&source, "user");
        let mut info = reading.get((13, 0)).unwrap().clone();
        info.as_dict_mut().unwrap().set("Title", lit(CHANGED));
        let base = write::Base::of(&reading).unwrap();
        let encryptor = Encryptor::of(&reading).unwrap();
        let mut out = source.clone();
        let given = [((13, 0), write::Body::from(&info))];
        let options = write::Options::DEFAULT;
        write::update(&mut out, &base, given, encryptor.as_ref(), None, &options).unwrap();
        let (was, now) = (title_written(&source), title_written(&out));
        let leaks = xor(&was, &now) == xor(TITLE, CHANGED);
        assert_eq!(leaks, method.name == "RC4-128", "{}", method.name);
    }
}

/// An object at a number its file lists free is refused, in an update and
/// in a whole write, under its RC4 and AES-128 encryptions, whose keys
/// are made of the number, at any generation; written under AES-256's, a
/// new encryption's, and with none.
#[test]
fn a_body_at_a_number_the_file_lists_free_is_refused_where_keys_are_per_object() {
    use chacha20::ChaCha20Rng;

    // Objects 1 and 3: 2 is listed free.
    let objects = [
        (1, Body::from(dict! { "Type" => "Catalog" })),
        (3, Body::from(dict! { "N" => 3 })),
    ];
    let root = dict! { "Root" => (1, 0) };
    let two = Object::from(dict! { "New" => lit(b"a new object") });
    let catalog = catalog();
    let kinds = [
        (Kind::Rc4By40, true),
        (Kind::Rc4By128, true),
        (Kind::Aes128, true),
        (Kind::Aes256R5, false),
        (Kind::Aes256, false),
    ];
    for (kind, refused) in kinds {
        let name = format!("{kind:?}");
        let file = fixture::encrypted(&settings(kind, true), &objects, root.clone());
        let reading = opened(&file, "user");
        let free = reading
            .entries()
            .any(|(n, _, e)| n == 2 && e == XrefEntry::Free);
        assert!(free, "{name}");
        let encryptor = Encryptor::of(&reading).unwrap();
        let base = write::Base::of(&reading).unwrap();
        for generation in [0, 1] {
            let id = (2, generation);
            let mut out = file.clone();
            let given = [(id, write::Body::from(&two))];
            let options = write::Options::DEFAULT;
            let updated = write::update(&mut out, &base, given, encryptor.as_ref(), None, &options);
            let mut whole = b"held".to_vec();
            let given = [
                ((1, 0), write::Body::from(&catalog)),
                (id, write::Body::from(&two)),
            ];
            let written = write::full(&mut whole, given, &root, encryptor.as_ref(), None, &options);
            match refused {
                true => {
                    assert_eq!(updated, Err(Error::Free(id)), "{name}");
                    assert_eq!(out, file, "{name}");
                    assert_eq!(written, Err(Error::Free(id)), "{name}");
                    assert_eq!(whole, b"held", "{name}");
                }
                false => {
                    updated.unwrap();
                    written.unwrap();
                    let back = opened(&out, "user");
                    assert_eq!(value(&back, id), two, "{name}");
                    let back = opened(&whole[4..], "user");
                    assert_eq!(value(&back, id), two, "{name}");
                }
            }
        }
    }
    // A new encryption's key is new; a file that isn't encrypted has none.
    let mut rng = ChaCha20Rng::seed_from_u64(10);
    let prepared = Password("user").prepared().unwrap();
    let created = Encryptor::create(&prepared, true, &mut rng).unwrap();
    let given = [
        ((1, 0), write::Body::from(&catalog)),
        ((2, 0), write::Body::from(&two)),
    ];
    let mut out = Vec::new();
    write::full(
        &mut out,
        given,
        &root,
        Some(&created),
        None,
        &write::Options::DEFAULT,
    )
    .unwrap();
    assert_eq!(value(&opened(&out, "user"), (2, 0)), two);
    let plain = fixture::file("1.7", &objects, root.clone(), Xref::Table);
    let reading = open(&plain);
    let base = write::Base::of(&reading).unwrap();
    let mut out = plain.clone();
    let given = [((2, 0), write::Body::from(&two))];
    write::update(&mut out, &base, given, None, None, &write::Options::DEFAULT).unwrap();
    assert_eq!(value(&open(&out), (2, 0)), two);
}

/// A whole file written under its reading's encryption lists each number
/// the reading listed free at the generation its entry gave, where keys
/// are made per object, so that a tool that follows its free list takes
/// none at a deleted object's key: here 2, free at generation 1, with a
/// table, a stream and packed, each read back so. Under AES-256's keys,
/// which aren't made per object, it's free at 0, as any number not
/// written.
#[test]
fn a_whole_file_lists_its_reading_s_free_numbers_at_their_generations() {
    let objects = [
        (1, Body::from(dict! { "Type" => "Catalog" })),
        (3, Body::from(dict! { "N" => 3 })),
    ];
    let root = dict! { "Root" => (1, 0) };
    let kinds = [
        (Kind::Rc4By40, true),
        (Kind::Rc4By128, true),
        (Kind::Aes128, true),
        (Kind::Aes256, false),
    ];
    for (kind, kept) in kinds {
        let name = format!("{kind:?}");
        let mut file = fixture::encrypted(&settings(kind, true), &objects, root.clone());
        // Number 2's row, free, to be used next at generation 1.
        let row: &[u8] = b"0000000000 00000 f \n";
        let at = file.windows(row.len()).position(|w| w == row).unwrap();
        file[at..at + row.len()].copy_from_slice(b"0000000000 00001 f \n");
        let reading = opened(&file, "user");
        let listed: Vec<_> = reading.entries().filter(|&(n, ..)| n == 2).collect();
        assert_eq!(listed, [(2, 1, XrefEntry::Free)], "{name}");
        let encryptor = Encryptor::of(&reading).unwrap().unwrap();
        assert_eq!(encryptor.free(2), kept.then_some(1), "{name}");
        let numbers: Vec<_> = encryptor.free_numbers().collect();
        assert_eq!(numbers, if kept { vec![(2, 1)] } else { vec![] }, "{name}");
        let doc = reading.to_document().unwrap();
        let layouts = [
            (XrefKind::Table, None),
            (XrefKind::Stream, None),
            (XrefKind::Stream, Some(write::Pack::DEFAULT)),
        ];
        for (layout, pack) in layouts {
            let mut out = Vec::new();
            let options = options(layout, pack);
            write::document(&mut out, &doc, Some(&encryptor), Some(&reading), &options).unwrap();
            let back = opened(&out, "user");
            assert!(sound(&back), "{name}, {layout:?}: {:?}", back.warnings());
            let free: Vec<_> = back.entries().filter(|&(n, ..)| n == 2).collect();
            let generation = u16::from(kept);
            assert_eq!(
                free,
                [(2, generation, XrefEntry::Free)],
                "{name}, {layout:?}"
            );
            assert_eq!(
                value(&back, (3, 0)),
                value(&reading, (3, 0)),
                "{name}, {layout:?}"
            );
        }
    }
}

/// `Encryptor::encrypting`, as a caller laying a file out has it, refuses
/// as a whole file does where `/EncryptMetadata false` leaves the
/// document's metadata stream in the clear and the catalog, which says
/// which stream that is, doesn't resolve, rather than encrypt it.
#[test]
fn encrypting_without_the_catalog_where_metadata_is_in_the_clear_is_refused() {
    let methods = methods();
    let clear = (methods.iter())
        .find(|m| m.name == "metadata in the clear")
        .unwrap();
    let (plain, trailer) = plaintext();
    let root = trailer.get(b"Root").and_then(Object::as_reference).unwrap();
    let none = |_: ObjectId| -> Option<&Object> { None };
    let refused = clear.encryptor.encrypting(&trailer, &none).err();
    assert_eq!(refused, Some(Error::Catalog(root)));
    let resolve = |id: ObjectId| (plain.iter()).find(|(n, _)| (*n, 0) == id).map(|(_, o)| o);
    assert!(clear.encryptor.encrypting(&trailer, &resolve).is_ok());
}

/// A whole write under a reading's encryption that writes its encryption
/// dictionary at another number encrypts what it's given at the number
/// the dictionary had, as any object: it's no encryption dictionary's in
/// the file written.
#[test]
fn an_object_at_a_moved_encryption_dictionary_s_number_is_encrypted() {
    let methods = methods();
    for method in standard(&methods) {
        let name = method.name;
        assert_eq!(method.encryptor.at(), Some((ENCRYPT, 0)), "{name}");
        let objects = [
            (1, catalog()),
            (
                ENCRYPT,
                Object::from(dict! { "Secret" => lit(PAGE_STRING) }),
            ),
        ];
        let mut options = write::Options::DEFAULT;
        options.encrypt_at = Some((50, 0));
        let mut out = Vec::new();
        let trailer = dict! { "Root" => (1, 0) };
        write::full(
            &mut out,
            bodies(&objects),
            &trailer,
            Some(&method.encryptor),
            None,
            &options,
        )
        .unwrap();
        assert!(!in_clear(&out, PAGE_STRING), "{name}");
        let back = opened(&out, "user");
        assert_eq!(
            back.trailer()
                .dict()
                .get(b"Encrypt")
                .and_then(Object::as_reference),
            Some((50, 0))
        );
        assert_eq!(value(&back, (ENCRYPT, 0)), objects[1].1, "{name}");
    }
}

/// A file whose encryption dictionary is in its trailer, direct, written
/// whole and as an update, keeps it there, and reads back by its
/// password.
#[test]
fn a_direct_encryption_dictionary_is_written_whole_and_updated_where_it_was() {
    let (plain, trailer) = plaintext();
    let direct = encryptor_of(&source(dictionary_of(Kind::Aes128, true), true));
    assert!(direct.direct());
    for kind in KINDS {
        let source = encrypted_by(&direct, &plain, &trailer, kind);
        let rewritten_whole = rewrite(&source, "user", kind);
        let reading = opened(&source, "user");
        let mut page = reading.get(PAGE).unwrap().clone();
        page.as_dict_mut().unwrap().set("Rotate", 90);
        let base = write::Base::of(&reading).unwrap();
        let encryptor = Encryptor::of(&reading).unwrap();
        let mut updated = source.clone();
        let given = [(PAGE, write::Body::from(&page))];
        let options = write::Options::DEFAULT;
        write::update(
            &mut updated,
            &base,
            given,
            encryptor.as_ref(),
            None,
            &options,
        )
        .unwrap();
        for (way, bytes) in [("whole", rewritten_whole), ("updated", updated)] {
            let name = format!("{kind:?}, {way}");
            let back = opened(&bytes, "user");
            assert!(sound(&back), "{name}: {:?}", back.warnings());
            let encrypt = back.trailer().dict().get(b"Encrypt").unwrap();
            assert_eq!(
                as_written(encrypt.as_dict().unwrap()),
                as_written(direct.dictionary()),
                "{name}"
            );
            assert!(Encryptor::of(&back).unwrap().unwrap().direct(), "{name}");
            assert_eq!(rewritten(&back), as_turned(), "{name}");
            for text in SECRETS {
                let text_said = String::from_utf8_lossy(text);
                assert!(!in_clear(&bytes, text), "{name}: {text_said:?}");
            }
        }
    }
}

/// [`plaintext`] written whole with a table, then updated with its page
/// turned: both files, and where the first's section starts.
fn and_updated() -> (Vec<u8>, Vec<u8>, usize) {
    let (plain, trailer) = plaintext();
    let file = whole(&plain, &trailer, XrefKind::Table, None);
    let reading = open(&file);
    let mut page = reading.get(PAGE).unwrap().clone();
    page.as_dict_mut().unwrap().set("Rotate", 90);
    let base = write::Base::of(&reading).unwrap();
    let mut updated = file.clone();
    let given = [(PAGE, write::Body::from(&page))];
    write::update(
        &mut updated,
        &base,
        given,
        None,
        None,
        &write::Options::DEFAULT,
    )
    .unwrap();
    (file, updated, base.prev)
}

/// `Base::of` gives a reading's file as its last section describes it:
/// its length, whether its last line is ended, its header at 0, its last
/// section and kind, its numbers past `/Size`, every number listed and its
/// cross-reference streams', its `/ID[0]`, its trailer and its reading.
/// It refuses a file read by scanning, a trailer without `/Root`, and a
/// reading as of an earlier revision, but not one as of its last.
#[test]
fn a_base_is_a_reading_s_file_as_its_last_section_says() {
    let (file, updated, first) = and_updated();
    let reading = open(&updated);
    let base = write::Base::of(&reading).unwrap();
    assert_eq!(base.len, updated.len());
    assert!(base.ends_line);
    assert_eq!(base.header, 0);
    assert_eq!((base.prev, base.kind), newest(&reading));
    assert!(base.prev > file.len());
    assert_eq!(base.size, 14);
    assert_eq!(base.first_id.as_deref(), Some(ID));
    assert_eq!(base.trailer, *reading.trailer().dict());
    assert_eq!(base.source, Some(reading.id()));

    // Past every number listed, and a cross-reference stream's.
    let small = fixture::assembled(
        "1.7",
        &[(1, b"<< /Type /Catalog >>"), (5, b"5")],
        "<< /Size 2 /Root 1 0 R >>",
    );
    assert_eq!(write::Base::of(&open(&small)).unwrap().size, 6);
    let stream = fixture::file(
        "1.7",
        &[(1, Body::from(dict! { "Type" => "Catalog" }))],
        dict! { "Root" => (1, 0) },
        Xref::Stream,
    );
    let reading = open(&stream);
    assert_eq!(reading.facts().max_stream, Some(2));
    assert_eq!(write::Base::of(&reading).unwrap().size, 3);
    // A last line not ended.
    let unended = [&small[..], b"%"].concat();
    assert!(!write::Base::of(&open(&unended)).unwrap().ends_line);

    // As of the last section, it's the file; as of the first, refused.
    let as_of = |section| {
        let mut options = Options::DEFAULT;
        options.section = Some(section);
        Reading::open(Arc::<[u8]>::from(&updated[..]), &options).unwrap()
    };
    let last = base.prev;
    let mut whole = write::Base::of(&as_of(last)).unwrap();
    whole.source = base.source;
    assert_eq!(whole, base);
    assert_eq!(write::Base::of(&as_of(first)), Err(Error::Earlier(first)));

    // Bytes before its header, a space or a hundred: refused, whether the
    // reader reads its sections or scans.
    for before in [&b" "[..], &PREFIX[..]] {
        let prefixed = [before, &updated[..]].concat();
        let refused = write::Base::of(&open(&prefixed));
        assert_eq!(refused, Err(Error::Prefixed(before.len())));
    }
    // Scanned, or with no catalog.
    let scanned =
        b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";
    assert_eq!(write::Base::of(&open(scanned)), Err(Error::Unsound));
    let rootless = fixture::assembled("1.7", &[(1, b"<< /Type /Catalog >>")], "<< /Size 2 >>");
    assert_eq!(write::Base::of(&open(&rootless)), Err(Error::Rootless));
}

/// A base holds while its file is as long and ends in its last section:
/// not once a byte is appended, nor another section; and the update's own
/// base holds its file.
#[test]
fn a_base_holds_until_its_file_changes() {
    let (file, updated, _) = and_updated();
    let base = write::Base::of(&open(&file)).unwrap();
    assert!(base.holds(&file[..]).unwrap());
    assert!(!base.holds(&[&file[..], b"\n"].concat()[..]).unwrap());
    assert!(!base.holds(&updated[..]).unwrap());
    let after = write::Base::of(&open(&updated)).unwrap();
    assert!(after.holds(&updated[..]).unwrap());
    let mut moved = updated.clone();
    let at = moved.windows(9).rposition(|w| w == b"startxref").unwrap() + 10;
    moved[at] = if moved[at] == b'1' { b'2' } else { b'1' };
    assert!(!after.holds(&moved[..]).unwrap());
}

/// A second update goes after the first as its `Written::base` says,
/// without reading it again: the file has three sections, each object as
/// its last section wrote it, and the base's reading kept.
#[test]
fn a_second_update_goes_after_the_first_s_written_base() {
    let (plain, trailer) = plaintext();
    let file = whole(&plain, &trailer, XrefKind::Stream, None);
    let reading = open(&file);
    let base = write::Base::of(&reading).unwrap();
    let mut page = reading.get(PAGE).unwrap().clone();
    page.as_dict_mut().unwrap().set("Rotate", 90);
    let mut out = file.clone();
    let options = write::Options::DEFAULT;
    let given = [(PAGE, write::Body::from(&page))];
    let first = write::update(&mut out, &base, given, None, None, &options)
        .unwrap()
        .unwrap();
    assert_eq!(first.base.source, base.source);
    assert!(first.base.holds(&out[..]).unwrap());
    let info = Object::from(dict! { "Title" => lit(b"the second") });
    let given = [((13, 0), write::Body::from(&info))];
    let second = write::update(&mut out, &first.base, given, None, None, &options)
        .unwrap()
        .unwrap();
    assert_eq!(second.base.source, base.source);
    assert!(second.base.holds(&out[..]).unwrap());
    let pdf = open(&out);
    assert!(sound(&pdf), "{:?}", pdf.warnings());
    assert_eq!(pdf.facts().sections, 3);
    assert_eq!(newest(&pdf), (second.start, XrefKind::Stream));
    assert_eq!(value(&pdf, (13, 0)), info);
    assert_eq!(
        value(&pdf, PAGE)
            .as_dict()
            .unwrap()
            .get(b"Rotate")
            .and_then(Object::as_integer),
        Some(90)
    );
    // Its /ID[0] kept from the file's, each update's /ID[1] its own.
    let ids = [first.id.clone().unwrap(), second.id.clone().unwrap()];
    assert_eq!([&ids[0][0], &ids[1][0]], [ID, ID]);
    assert_ne!(ids[0][1], ids[1][1]);
}

/// A write's positions count from where it starts in `out`: a whole file
/// after other bytes is the same bytes as one alone, and says the same of
/// where each object is; an update into an empty `out` is the bytes an
/// update after the file in `out` appends.
#[test]
fn positions_count_from_where_the_write_starts_in_out() {
    let (plain, trailer) = plaintext();
    for kind in KINDS {
        let options = options(kind, None);
        let mut alone = Vec::new();
        let written =
            write::full(&mut alone, bodies(&plain), &trailer, None, None, &options).unwrap();
        let mut after = b"other bytes, before the file\n".to_vec();
        let before = after.len();
        let again =
            write::full(&mut after, bodies(&plain), &trailer, None, None, &options).unwrap();
        assert_eq!(after[before..], alone[..], "{kind:?}");
        assert_eq!(again, written, "{kind:?}");
        assert_eq!(written.base.len, alone.len(), "{kind:?}");
        assert!(written.base.holds(&alone[..]).unwrap(), "{kind:?}");

        let mut page = plain[3].1.clone();
        page.as_dict_mut().unwrap().set("Rotate", 90);
        let given = || [(PAGE, write::Body::from(&page))];
        let mut update = Vec::new();
        let options = write::Options::DEFAULT;
        let apart =
            write::update(&mut update, &written.base, given(), None, None, &options).unwrap();
        let mut file = alone.clone();
        let appended =
            write::update(&mut file, &written.base, given(), None, None, &options).unwrap();
        assert_eq!(file[alone.len()..], update[..], "{kind:?}");
        assert_eq!(apart, appended, "{kind:?}");
        let pdf = open(&file);
        assert!(sound(&pdf), "{kind:?}: {:?}", pdf.warnings());
        assert_eq!(rewritten(&pdf), as_turned(), "{kind:?}");
    }
}

/// An update's trailer carries its file's newest trailer's entries but
/// its sections' own (ISO 32000-2, 7.5.6): `/Root`, `/Prev`, `/Info` and
/// `/ID` first, then the rest in the file's order, then `/Size`.
#[test]
fn an_update_carries_its_file_s_trailer_entries() {
    let (plain, mut trailer) = plaintext();
    trailer.set("Custom", "Kept");
    trailer.set("Other", 7);
    for kind in KINDS {
        let file = whole(&plain, &trailer, kind, None);
        let reading = open(&file);
        let base = write::Base::of(&reading).unwrap();
        let mut out = file.clone();
        let given = [(PAGE, write::Body::from(&plain[3].1))];
        write::update(&mut out, &base, given, None, None, &write::Options::DEFAULT).unwrap();
        let pdf = open(&out);
        let keys: Vec<&[u8]> = pdf.trailer().dict().iter().map(|(k, _)| k).collect();
        let want: &[&[u8]] = match kind {
            XrefKind::Table => &[
                b"Root", b"Prev", b"Info", b"ID", b"Custom", b"Other", b"Size",
            ],
            XrefKind::Stream => &[
                b"Root", b"Prev", b"Info", b"ID", b"Custom", b"Other", b"Type", b"Size", b"W",
                b"Index", b"Length",
            ],
        };
        assert_eq!(keys, want, "{kind:?}");
        let custom = pdf
            .trailer()
            .dict()
            .get(b"Custom")
            .and_then(Object::as_name);
        assert_eq!(custom, Some(&b"Kept"[..]), "{kind:?}");
    }
}

/// A stream whose data is where it was written takes it from the reading
/// it was read in, at its own number: decrypted, and encrypted again as
/// the write is. At another number, from a reading of another file, or
/// with no reading, it's refused, and nothing is written.
#[test]
fn a_stream_s_data_where_it_was_written_is_its_own_reading_s_alone() {
    let (plain, trailer) = plaintext();
    let methods = methods();
    let aes = &methods
        .iter()
        .find(|m| m.name == "AES-128")
        .unwrap()
        .encryptor;
    let source = encrypted_by(aes, &plain, &trailer, XrefKind::Table);
    let reading = opened(&source, "user");
    let content = reading.get((3, 0)).unwrap().clone();
    assert!(content.as_stream().unwrap().data.at().is_some());
    let catalog = catalog();
    let root = dict! { "Root" => (1, 0) };
    let given = |at: u32| {
        [
            ((1, 0), write::Body::from(&catalog)),
            ((at, 0), write::Body::from(&content)),
        ]
    };
    let options = write::Options::DEFAULT;
    for encryptor in [None, Encryptor::of(&reading).unwrap()] {
        let mut out = Vec::new();
        write::full(
            &mut out,
            given(3),
            &root,
            encryptor.as_ref(),
            Some(&reading),
            &options,
        )
        .unwrap();
        let back = read_back(&out, encryptor.is_some());
        assert_eq!(back.data((3, 0)).unwrap().as_ref(), CONTENT);
        assert_eq!(holds(&out, CONTENT), encryptor.is_none());
    }
    let other = whole(&plain, &trailer, XrefKind::Stream, None);
    let other = open(&other);
    let refusals: [(u32, Option<&dyn write::ReadFrom>); 3] =
        [(20, Some(&reading)), (3, Some(&other)), (3, None)];
    for (at, from) in refusals {
        let mut out = b"held".to_vec();
        let refused = write::full(&mut out, given(at), &root, None, from, &options);
        assert_eq!(refused, Err(Error::Placed((at, 0))));
        assert_eq!(out, b"held");
    }
}

/// A stream of another file, its data where it was written there, is
/// written from the reading given wherever its stream of that number has
/// its data at the same place: another file's bytes. Unplaced, it's
/// refused, nothing written; its data given first as its bytes, as its own
/// file's reading hands them out, it's written from them.
#[test]
fn a_stream_of_another_file_is_written_from_its_bytes_once_unplaced() {
    let stream = |data: &[u8]| Object::from(Stream::new(dict! {}, data.to_vec()));
    let (a, b) = (file(&[(3, stream(b"aaaa"))]), file(&[(3, stream(b"bbbb"))]));
    let (a, b) = (open(&a), open(&b));
    let content = a.get((3, 0)).unwrap().clone();
    let at = content.as_stream().unwrap().data.at();
    assert_eq!(at, b.get((3, 0)).unwrap().as_stream().unwrap().data.at());
    let catalog = catalog();
    let root = dict! { "Root" => (1, 0) };
    let options = write::Options::DEFAULT;
    let written = |content: &Object, out: &mut Vec<u8>| {
        let bodies = [
            ((1, 0), write::Body::from(&catalog)),
            ((3, 0), write::Body::from(content)),
        ];
        write::full(out, bodies, &root, None, Some(&b), &options)
    };
    // As read: written from the other file's bytes, at the same place.
    let mut out = Vec::new();
    written(&content, &mut out).unwrap();
    assert_eq!(open(&out).data((3, 0)).unwrap().as_ref(), b"bbbb");
    // Unplaced, it has no data to write.
    let mut unplaced = content.clone();
    unplaced.unplace();
    let mut out = b"held".to_vec();
    let refused = written(&unplaced, &mut out);
    let data = serialize::Error::Data { id: Some((3, 0)) };
    assert_eq!(refused, Err(Error::Serialize(data)));
    assert_eq!(out, b"held");
    // Its data given first as its bytes, from its own file's reading.
    let mut moved = content.clone();
    let bytes = a.data((3, 0)).unwrap().into_owned();
    moved.as_stream_mut().unwrap().set_bytes(bytes);
    moved.unplace();
    let mut out = Vec::new();
    written(&moved, &mut out).unwrap();
    assert_eq!(open(&out).data((3, 0)).unwrap().as_ref(), b"aaaa");
}

/// A document built in memory is written whole, its objects at their
/// numbers, read back as built; with no `/ID`, as it had none.
#[test]
fn a_built_document_is_written_whole() {
    let mut doc = crate::Document::new(pdf_objects::version::Version::new(2, 0), Dict::new());
    let content = doc.add(Stream::new(dict! {}, b"0 0 m 9 9 l S".to_vec()));
    let pages = doc.new_id();
    let page = doc.add(dict! { "Type" => "Page", "Parent" => pages, "Contents" => content });
    doc.set(
        pages,
        dict! { "Type" => "Pages", "Kids" => vec![Object::from(page)], "Count" => 1 },
    );
    let catalog = doc.add(dict! { "Type" => "Catalog", "Pages" => pages });
    doc.trailer.dict_mut().set("Root", catalog);
    let mut options = write::Options::DEFAULT;
    options.version = pdf_objects::version::Version::new(2, 0);
    let mut out = Vec::new();
    let written = write::document(&mut out, &doc, None, None, &options).unwrap();
    assert!(out.starts_with(b"%PDF-2.0\n"));
    assert_eq!(written.id, None);
    let pdf = open(&out);
    assert!(sound(&pdf), "{:?}", pdf.warnings());
    for (id, obj) in doc.objects() {
        assert_eq!(handed(&pdf, id), unplaced(obj), "{id:?}");
    }
    assert!(!pdf.trailer().dict().has(b"ID"));
}

/// Every refusal of a whole file's and an update's writes nothing: `out`
/// is as it was, whatever the write had written before it refused.
#[test]
fn every_refusal_writes_nothing() {
    use std::sync::atomic::AtomicBool;

    let (plain, trailer) = plaintext();
    let methods = methods();
    let by_name = |name| &methods.iter().find(|m| m.name == name).unwrap().encryptor;
    let catalog = catalog();
    let nan = Object::from(dict! { "N" => f64::NAN });
    let unknown = Object::from(Stream::new(
        dict! {
            "Filter" => vec![Object::from("Crypt")],
            "DecodeParms" => vec![Object::from(dict! { "Type" => "CryptFilterDecodeParms", "Name" => "Nonesuch" })],
        },
        b"data".to_vec(),
    ));
    fn at(n: u32, o: &Object) -> (ObjectId, write::Body<'_>) {
        ((n, 0), write::Body::from(o))
    }
    let stop = AtomicBool::new(true);
    let mut stopped = write::Options::DEFAULT;
    stopped.stop = Some(&stop);
    let not_packed = |_: &[u32]| vec![vec![1, 99]];
    let twice = |n: &[u32]| vec![n.to_vec(), n.to_vec()];
    fn packed<'g>(groups: &'g write::Groups<'g>) -> write::Options<'g> {
        let mut pack = write::Pack::DEFAULT;
        pack.groups = Some(groups);
        options(XrefKind::Stream, Some(pack))
    }
    let mut encrypt_at = write::Options::DEFAULT;
    encrypt_at.encrypt_at = Some((1, 0));
    /// A whole file's bodies, encryptor, options, and the refusal.
    type Whole<'a> = (
        Vec<(ObjectId, write::Body<'a>)>,
        Option<&'a Encryptor>,
        write::Options<'a>,
        Error,
    );
    let wholes: Vec<Whole<'_>> = vec![
        (
            vec![at(0, &catalog)],
            None,
            write::Options::DEFAULT,
            Error::Number(0),
        ),
        (
            vec![at(write::NUMBERS, &catalog)],
            None,
            write::Options::DEFAULT,
            Error::Number(write::NUMBERS),
        ),
        (
            vec![at(1, &catalog), ((1, 1), write::Body::from(&catalog))],
            None,
            write::Options::DEFAULT,
            Error::Twice(1),
        ),
        (
            vec![at(1, &catalog)],
            Some(by_name("AES-256 created")),
            encrypt_at,
            Error::EncryptAt(1),
        ),
        (
            vec![at(1, &catalog)],
            None,
            options(XrefKind::Table, Some(write::Pack::DEFAULT)),
            Error::PackedTable,
        ),
        (
            vec![at(1, &catalog)],
            None,
            packed(&not_packed),
            Error::Group(99),
        ),
        (vec![at(1, &catalog)], None, packed(&twice), Error::Group(1)),
        (vec![at(1, &catalog)], None, stopped, Error::Stopped),
        (
            vec![at(1, &catalog), at(2, &nan)],
            None,
            write::Options::DEFAULT,
            Error::Serialize(serialize::Error::NonFinite { id: Some((2, 0)) }),
        ),
        (
            vec![at(1, &catalog), at(2, &nan)],
            None,
            packed(&|n: &[u32]| vec![n.to_vec()]),
            Error::Serialize(serialize::Error::NonFinite { id: Some((2, 0)) }),
        ),
        (
            vec![at(1, &catalog), at(2, &unknown)],
            Some(by_name("AES-128")),
            write::Options::DEFAULT,
            Error::Encrypt {
                id: (2, 0),
                why: Unencryptable::Fault(Fault::Method {
                    name: "Nonesuch".to_string(),
                }),
            },
        ),
        (
            vec![at(1, &catalog)],
            Some(by_name("metadata in the clear")),
            write::Options::DEFAULT,
            Error::Catalog((12, 0)),
        ),
    ];
    for (given, encryptor, options, want) in wholes {
        let mut out = b"held".to_vec();
        let refused = write::full(&mut out, given, &trailer, encryptor, None, &options);
        assert_eq!(refused, Err(want.clone()), "{want}");
        assert_eq!(out, b"held", "{want}");
    }

    let file = whole(&plain, &trailer, XrefKind::Table, None);
    let reading = open(&file);
    let base = write::Base::of(&reading).unwrap();
    let rootless = write::Base::new(
        file.len(),
        base.prev,
        base.size,
        XrefKind::Table,
        Dict::new(),
    );
    let source = encrypted_by(by_name("AES-128"), &plain, &trailer, XrefKind::Table);
    let encrypted = write::Base::of(&opened(&source, "user")).unwrap();
    let aes = Encryptor::of(&opened(&source, "user")).unwrap();
    let mut last = base.clone();
    last.kind = XrefKind::Stream;
    last.size = write::NUMBERS;
    let page = &plain[3].1;
    let updates: Vec<(&write::Base, Option<&Encryptor>, write::Options<'_>, Error)> = vec![
        (&rootless, None, write::Options::DEFAULT, Error::Rootless),
        (&encrypted, None, write::Options::DEFAULT, Error::Unmatched),
        (
            &base,
            aes.as_ref(),
            write::Options::DEFAULT,
            Error::Unmatched,
        ),
        (
            &encrypted,
            Some(by_name("AES-256 created")),
            write::Options::DEFAULT,
            Error::Unmatched,
        ),
        (
            &last,
            None,
            write::Options::DEFAULT,
            Error::NoNumber(write::NUMBERS),
        ),
        (&base, None, stopped, Error::Stopped),
    ];
    for (base, encryptor, options, want) in updates {
        let mut out = file.clone();
        let given = [(PAGE, write::Body::from(page))];
        let refused = write::update(&mut out, base, given, encryptor, None, &options);
        assert_eq!(refused, Err(want.clone()), "{want}");
        assert_eq!(out, file, "{want}");
    }
}

/// An update's encryptor is its file's: another file's is refused, nothing
/// written, though its `/Encrypt` is at the same number (two of the test
/// writer's files, their `/ID`s other, under RC4, AES-128 and AES-256), or
/// direct (two files with one `/ID`, their dictionaries other), as its key
/// isn't the file's; one of another reading of the same bytes, by the
/// owner's password, writes, and its update reads back by the user's.
#[test]
fn an_update_under_another_file_s_encryptor_is_refused() {
    let objects = [
        (1, Body::from(dict! { "Type" => "Catalog" })),
        (2, Body::from(dict! { "Title" => lit(b"secret") })),
    ];
    let root = dict! { "Root" => (1, 0) };
    let changed = Object::from(dict! { "Title" => lit(b"changed") });
    for kind in [Kind::Rc4By128, Kind::Aes128, Kind::Aes256] {
        let name = format!("{kind:?}");
        let mut other = settings(kind, true);
        other.id = b"another file's i";
        let a = fixture::encrypted(&settings(kind, true), &objects, root.clone());
        let b = fixture::encrypted(&other, &objects, root.clone());
        let (theirs, ours) = (encryptor_of(&b), encryptor_of(&a));
        assert_eq!(theirs.at(), ours.at(), "{name}");
        let base = write::Base::of(&opened(&a, "user")).unwrap();
        let given = [((2, 0), write::Body::from(&changed))];
        let mut out = a.clone();
        let options = &write::Options::DEFAULT;
        let refused = write::update(&mut out, &base, given.clone(), Some(&theirs), None, options);
        assert_eq!(refused, Err(Error::Unmatched), "{name}");
        assert_eq!(out, a, "{name}");
        let owner = Encryptor::of(&opened(&a, "owner")).unwrap();
        write::update(&mut out, &base, given, owner.as_ref(), None, options).unwrap();
        assert_eq!(value(&opened(&out, "user"), (2, 0)), changed, "{name}");
    }
    let catalog = Object::from(dict! { "Type" => "Catalog", "Lang" => lit(b"en") });
    let a = source(dictionary_of(Kind::Aes128, true), true);
    let b = source(dictionary_of(Kind::Aes128, false), true);
    let theirs = encryptor_of(&b);
    assert!(theirs.direct());
    let reading = opened(&a, "user");
    let bases = [
        write::Base::of(&reading).unwrap(),
        write::Base::new(
            a.len(),
            newest(&reading).0,
            size(&reading),
            XrefKind::Table,
            reading.trailer().dict().clone(),
        ),
    ];
    for base in bases {
        let mut out = a.clone();
        let given = [((1, 0), write::Body::from(&catalog))];
        let options = &write::Options::DEFAULT;
        let refused = write::update(&mut out, &base, given, Some(&theirs), None, options);
        assert_eq!(refused, Err(Error::Unmatched));
        assert_eq!(out, a);
    }
}

/// What an encryption dictionary refers to is read before a reader has a
/// key, so a packed write leaves it out of its object streams, which are
/// encrypted: here `/CF`, a reference to object 41.
#[test]
fn an_encryption_dictionary_s_objects_stay_out_of_object_streams() {
    let mut dict = dictionary_of(Kind::Aes128, true);
    let filters = dict.remove(b"CF").unwrap();
    dict.set("CF", Object::reference(41));
    let id = Object::hex(ID);
    let trailer = dict! { "Root" => Object::reference(1), "ID" => vec![id.clone(), id] };
    // Objects 1, 40 and 41, listed in runs, so that none is listed free.
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut at = Vec::new();
    let objects = [
        (1, catalog()),
        (ENCRYPT, Object::from(dict)),
        (41, filters.clone()),
    ];
    for (num, obj) in &objects {
        at.push(serialize::indirect(&mut out, (*num, 0), obj).unwrap());
    }
    let mut source_trailer = trailer.clone();
    source_trailer.set("Encrypt", Object::reference(ENCRYPT));
    source_trailer.set("Size", 42);
    let xref = out.len();
    let table = format!(
        "xref\n0 2\n0000000000 65535 f \n{:010} 00000 n \n40 2\n{:010} 00000 n \n{:010} 00000 n \ntrailer\n",
        at[0], at[1], at[2]
    );
    out.extend_from_slice(table.as_bytes());
    serialize::write(&mut out, &Object::from(source_trailer)).unwrap();
    out.extend_from_slice(format!("\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    let encryptor = encryptor_of(&out);
    assert!(encryptor.dictionary_object((41, 0)));
    // By its shape alone, it may go in one.
    assert!(write::packable((41, 0), &filters));

    let (mut plain, trailer) = plaintext();
    plain.push((41, filters));
    let bytes = whole_packed(&plain, &trailer, Some(&encryptor), None);
    let pdf = opened(&bytes, "user");
    assert!(matches!(pdf.encryption(), Encryption::Opened { .. }));
    assert!(sound(&pdf), "{:?}", pdf.warnings());
    let xref = pdf.facts().max_stream.unwrap();
    let (_, _, got) = rows(&pdf, (xref, 0));
    assert_eq!(got[41][0], 1, "on its own");
    assert_eq!(got[13][0], 2, "the Info dictionary is packed");
    assert_reads_to(&plain, &bytes, true, "/CF by reference");
}

/// A file whose earlier revision named another encryption dictionary, an
/// object the reader leaves in the clear as one, written whole: the file
/// written has one section, whose `/Encrypt` is the newest's alone, so the
/// earlier one is written encrypted, as any object is, and reads back as
/// it was; updated, it stays in the clear, as its file names it still.
#[test]
fn an_earlier_revision_s_encryption_dictionary_is_written_whole_as_any_object() {
    let dict = dictionary_of(Kind::Aes128, true);
    let mut file = source(dict.clone(), false);
    let earlier = open(&file);
    let base = write::Base::of(&earlier).unwrap();
    // An update that names the same dictionary, again, as object 41.
    let mut section = Section::append(base.size, 0);
    let pos = serialize::indirect(&mut file, (41, 0), &Object::from(dict)).unwrap();
    section.insert(
        41,
        Entry::InUse {
            offset: pos,
            generation: 0,
        },
    );
    let mut trailer = base.trailer.clone();
    trailer.set("Prev", base.prev as i64);
    trailer.set("Encrypt", Object::reference(41));
    let pos = file.len();
    section.table(&mut file, trailer, pos).unwrap();

    let reading = opened(&file, "user");
    assert!(matches!(reading.encryption(), Encryption::Opened { .. }));
    let was = value(&reading, (ENCRYPT, 0));
    let doc = reading.to_document().unwrap();
    let encryptor = Encryptor::of(&reading).unwrap();
    assert_eq!(encryptor.as_ref().and_then(Encryptor::at), Some((41, 0)));
    let mut whole = Vec::new();
    let options = write::Options::DEFAULT;
    write::document(
        &mut whole,
        &doc,
        encryptor.as_ref(),
        Some(&reading),
        &options,
    )
    .unwrap();
    let back = opened(&whole, "user");
    assert!(sound(&back), "{:?}", back.warnings());
    assert_eq!(
        value(&back, (ENCRYPT, 0)),
        was,
        "the earlier dictionary as it was"
    );
    // Its strings encrypted: what was in the clear isn't, as written.
    let o = |pdf: &Reading<Arc<[u8]>>| {
        pdf.get((ENCRYPT, 0))
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"O")
            .cloned()
    };
    let written = |bytes: &[u8]| {
        let locked = open(bytes);
        o(&locked).and_then(|o| match o.into_value() {
            Value::String(Str::Encrypted(bytes) | Str::Bytes(bytes), _) => Some(bytes),
            _ => None,
        })
    };
    assert_ne!(written(&whole), written(&file));

    // An update after the file keeps naming it, so leaves it in the clear.
    let base = write::Base::of(&reading).unwrap();
    let mut updated = file.clone();
    let given = [((ENCRYPT, 0), write::Body::from(&was))];
    write::update(
        &mut updated,
        &base,
        given,
        encryptor.as_ref(),
        None,
        &options,
    )
    .unwrap();
    let back = opened(&updated, "user");
    assert_eq!(value(&back, (ENCRYPT, 0)), was);
    let mut clear = Vec::new();
    serialize::indirect(&mut clear, (ENCRYPT, 0), &was).unwrap();
    assert!(holds(&updated[file.len()..], &clear), "written as it is");
}

/// A file whose trailer's `/Encrypt` is null isn't encrypted (ISO 32000-2,
/// 7.3.9: a null is no entry), so an update after it is written with no
/// encryption, and none is said of it.
#[test]
fn an_update_after_a_null_encrypt_is_unencrypted() {
    let (plain, mut trailer) = plaintext();
    trailer.set("Encrypt", Object::null());
    let file = whole(&plain, &trailer, XrefKind::Table, None);
    let mut nulled = file.clone();
    // The writer leaves /Encrypt out; a writer that doesn't writes it null.
    let at = (nulled.windows(8).rposition(|w| w == b"trailer\n")).unwrap() + 8;
    nulled.splice(at + 2..at + 2, b"/Encrypt null".iter().copied());
    let reading = open(&nulled);
    assert_eq!(reading.encryption(), &Encryption::None);
    assert!(
        reading
            .trailer()
            .dict()
            .get(b"Encrypt")
            .is_some_and(Object::is_null)
    );
    let base = write::Base::of(&reading).unwrap();
    let mut out = nulled.clone();
    let given = [(PAGE, write::Body::from(&plain[3].1))];
    write::update(&mut out, &base, given, None, None, &write::Options::DEFAULT).unwrap();
    let back = open(&out);
    assert!(sound(&back), "{:?}", back.warnings());
    assert!(!back.trailer().dict().has(b"Encrypt"));
}

/// A reference to an object that isn't there reads as null (ISO 32000-2,
/// 7.3.10), and stays so: the writer's own objects (the object streams,
/// the cross-reference stream, a new encryption dictionary) pass over the
/// numbers what it writes refers to and doesn't hold, a trailer's
/// reference among them, which would otherwise come to name the
/// cross-reference stream itself.
#[test]
fn a_reference_to_no_object_names_none_of_the_writer_s_own() {
    use chacha20::ChaCha20Rng;

    let mut objects = numbered();
    let gone = vec![Object::reference(260), Object::reference(261)];
    objects[4].1.as_dict_mut().unwrap().set("Gone", gone);
    let trailer = dict! { "Root" => (1, 0), "Missing" => Object::reference(262) };
    let bytes = whole_packed(&objects, &trailer, None, None);
    let pdf = open(&bytes);
    assert!(sound(&pdf), "{:?}", pdf.warnings());
    assert_eq!(object_streams_of(&pdf), [(263, 0), (264, 0), (265, 0)]);
    assert_eq!(pdf.facts().max_stream, Some(266));
    for n in 260..263 {
        assert!(pdf.get((n, 0)).is_err(), "{n} is no object");
    }
    // A new encryption's dictionary, past every object, passes over them
    // too.
    let mut rng = ChaCha20Rng::seed_from_u64(11);
    let prepared = Password("user").prepared().unwrap();
    let created = Encryptor::create(&prepared, true, &mut rng).unwrap();
    let mut out = Vec::new();
    let options = write::Options::DEFAULT;
    write::full(
        &mut out,
        bodies(&objects),
        &trailer,
        Some(&created),
        None,
        &options,
    )
    .unwrap();
    let pdf = opened(&out, "user");
    let encrypt = pdf
        .trailer()
        .dict()
        .get(b"Encrypt")
        .and_then(Object::as_reference);
    assert_eq!(encrypt, Some((263, 0)));
    // And an update's cross-reference stream.
    let file = whole(&objects, &trailer, XrefKind::Stream, None);
    let reading = open(&file);
    let base = write::Base::of(&reading).unwrap();
    assert_eq!(base.size, 264);
    let mut catalog = objects[0].1.clone();
    catalog
        .as_dict_mut()
        .unwrap()
        .set("Later", Object::reference(264));
    let mut updated = file.clone();
    let given = [((1, 0), write::Body::from(&catalog))];
    let written = write::update(&mut updated, &base, given, None, None, &options)
        .unwrap()
        .unwrap();
    assert!(
        written.section.get(265).is_some(),
        "past 264, which the catalog names"
    );
    let pdf = open(&updated);
    assert!(sound(&pdf), "{:?}", pdf.warnings());
    assert!(pdf.get((264, 0)).is_err());
}

// Copies (`write::Body::Read`): an object as read, written as the bytes
// it was read from where the rules for a copy hold, each rule's test
// failing without it.

/// Objects spelled as a writer other than this one spells them, which this
/// one writes otherwise: odd spacing, a real `1.50`, a hex string, a
/// comment; and a stream, its `/Length` a number, its data as long.
const SPELLED: [(u32, &[u8]); 6] = [
    (1, b"<</Type /Catalog  /Pages 2 0 R>>"),
    (2, b"<</Type /Pages /Kids [3 0 R] /Count 1>>"),
    (
        3,
        b"<</Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /UserUnit 1.50 /Resources <<>> /Contents 6 0 R>>",
    ),
    (4, b"<48656C6C6F>"),
    (5, b"[ 1   2 % a comment\n 3 ]"),
    (6, b"<</Length 11>>\nstream\n0 0 m 1.5 l\nendstream"),
];

/// `objects`, [`SPELLED`]'s or others, as a file with a table, its trailer
/// `/Root 1 0 R`.
fn spelled_as(objects: &[(u32, &[u8])]) -> Vec<u8> {
    let size = objects.iter().map(|(n, _)| n + 1).max().unwrap_or(1);
    fixture::assembled("1.7", objects, &format!("<< /Size {size} /Root 1 0 R >>"))
}

/// [`SPELLED`] as a file.
pub(crate) fn spelled() -> Vec<u8> {
    spelled_as(&SPELLED)
}

/// Whether object `id`, which `record` says `reading` read, is in `out` as
/// the bytes it was read from, where `written` lists it at an offset.
fn copied<S: ByteSource>(
    out: &[u8],
    written: &write::Written,
    record: &Record,
    reading: &Reading<S>,
    id: ObjectId,
) -> bool {
    let bytes = reading.written(record.at).unwrap();
    match written.section.get(id.0) {
        Some(Entry::InUse { offset: pos, .. }) => out[pos..].starts_with(&bytes),
        _ => false,
    }
}

/// An object as read, as `doc` has it: its value and its record.
fn read_as(doc: &crate::Document, id: ObjectId) -> write::Body<'_> {
    write::Body::Read {
        object: Cow::Borrowed(doc.get(id).unwrap()),
        record: Cow::Borrowed(doc.record(id).unwrap()),
    }
}

/// Every object of a document, each as read, is written as the bytes it was
/// read from, given the reading it was read in: odd spacing, a real
/// `1.50`, a comment and a stream as they were; with a table and with a
/// stream; read back, the same objects.
#[test]
fn an_object_as_read_is_copied_as_the_bytes_it_was_read_from() {
    let file = spelled();
    let reading = open(&file);
    let doc = reading.to_document().unwrap();
    assert!(sound(&reading), "{:?}", reading.warnings());
    for kind in KINDS {
        let mut out = Vec::new();
        let options = options(kind, None);
        let written = write::document(&mut out, &doc, None, Some(&reading), &options).unwrap();
        for (n, _) in SPELLED {
            let record = doc.record((n, 0)).unwrap();
            assert!(
                copied(&out, &written, record, &reading, (n, 0)),
                "{kind:?}: {n}"
            );
        }
        assert!(holds(&out, b"/UserUnit 1.50") && holds(&out, b"% a comment"));
        let back = open(&out);
        assert!(sound(&back), "{kind:?}: {:?}", back.warnings());
        for (id, _) in doc.objects() {
            assert_eq!(handed(&back, id), handed(&reading, id), "{kind:?}: {id:?}");
        }
    }
}

/// Given no reading, an object as read is written anew, as nothing's read
/// to copy: the same objects, as this writer spells them.
#[test]
fn with_no_reading_given_an_object_as_read_is_written_anew() {
    let file = spelled();
    let reading = open(&file);
    let mut doc = reading.to_document().unwrap();
    // The stream's data given as bytes, as there's no reading to read it.
    let data = reading.data((6, 0)).unwrap().into_owned();
    let dict = doc.get((6, 0)).unwrap().as_stream().unwrap().dict.clone();
    doc.set((6, 0), Stream::new(dict, data));
    assert!(
        SPELLED[..5]
            .iter()
            .all(|(n, _)| doc.record((*n, 0)).is_some())
    );
    let mut out = Vec::new();
    let options = options(XrefKind::Table, None);
    write::document(&mut out, &doc, None, None, &options).unwrap();
    assert!(!holds(&out, b"1.50") && !holds(&out, b"% a comment"));
    let back = open(&out);
    assert!(sound(&back), "{:?}", back.warnings());
    for (id, _) in doc.objects() {
        assert_eq!(handed(&back, id), handed(&reading, id), "{id:?}");
    }
}

/// Rule 1: an object as read, its record another reading's than the one
/// given, refuses the write, whole or an update, nothing written: its
/// places are another file's, for all the writer knows. Two readings of
/// one file give records equal all the same, each its own reading's.
#[test]
fn a_record_of_another_reading_than_the_one_given_is_refused() {
    let file = spelled();
    let (first, second) = (open(&file), open(&file));
    let doc = first.to_document().unwrap();
    let again = second.to_document().unwrap();
    for (id, record) in doc.records() {
        let other = again.record(id).unwrap();
        assert_eq!(other, record, "{id:?}");
        assert_eq!((record.source, other.source), (first.id(), second.id()));
    }
    let options = options(XrefKind::Table, None);
    let mut out = Vec::new();
    let refused = write::document(&mut out, &doc, None, Some(&second), &options);
    assert_eq!(refused, Err(Error::Elsewhere((1, 0))));
    assert!(out.is_empty());
    let mut base = write::Base::of(&second).unwrap();
    base.source = None;
    let mut updated = file.clone();
    let refused = write::update(
        &mut updated,
        &base,
        write::bodies(&doc),
        None,
        Some(&second),
        &options,
    );
    assert_eq!(refused, Err(Error::Elsewhere((1, 0))));
    assert_eq!(updated, file);
    write::document(&mut out, &doc, None, Some(&first), &options).unwrap();
}

/// [`plaintext`] as the test writer encrypts it under `kind`, user `user`,
/// the document's metadata encrypted as `encrypt_metadata` says: spelled,
/// and its AES IVs drawn, otherwise than this writer's, so that a copy's
/// bytes are told from what this writer writes anew.
pub(crate) fn written_otherwise(kind: Kind, encrypt_metadata: bool) -> Vec<u8> {
    let (plain, trailer) = plaintext();
    let objects: Vec<(u32, fixture::Body)> = (plain.into_iter())
        .map(|(n, o)| match o.into_value() {
            Value::Stream(s) => (n, Body::Stream(s.dict, s.data.bytes().unwrap().to_vec())),
            value => (n, Body::Object(Object::new(value))),
        })
        .collect();
    fixture::encrypted(&settings(kind, encrypt_metadata), &objects, trailer)
}

/// The methods the test writer encrypts by.
pub(crate) const KINDS_ENCRYPTED: [Kind; 5] = [
    Kind::Rc4By40,
    Kind::Rc4By128,
    Kind::Aes128,
    Kind::Aes256R5,
    Kind::Aes256,
];

/// Rule 2: an object read at an offset is copied under an encryptor of
/// the reading it was read in, its ciphertext as it was; written anew
/// under another, a new one or another reading's of the same file (whose
/// key is the same, but which this writer can't tell is), and decrypted.
/// Read back alike every way, nothing in the clear but where it's
/// decrypted.
#[test]
fn objects_at_offsets_are_copied_under_their_own_reading_s_encryption_alone() {
    use chacha20::ChaCha20Rng;

    let (plain, _) = plaintext();
    let prepared = Password("new").prepared().unwrap();
    let mut rng = ChaCha20Rng::seed_from_u64(12);
    let created = Encryptor::create(&prepared, true, &mut rng).unwrap();
    for kind in KINDS_ENCRYPTED {
        let source = written_otherwise(kind, true);
        let reading = opened(&source, "user");
        let doc = reading.to_document().unwrap();
        let own = Encryptor::of(&reading).unwrap();
        let other = Encryptor::of(&opened(&source, "user")).unwrap();
        let ways = [
            ("its own", own.as_ref(), "user", true),
            ("another reading's", other.as_ref(), "user", false),
            ("decrypted", None, "", false),
            ("created", Some(&created), "new", false),
        ];
        for (way, encryptor, password, copies) in ways {
            let name = format!("{kind:?}, {way}");
            let mut out = Vec::new();
            let options = options(XrefKind::Table, None);
            let written =
                write::document(&mut out, &doc, encryptor, Some(&reading), &options).unwrap();
            let back = opened(&out, password);
            assert!(sound(&back), "{name}: {:?}", back.warnings());
            for (n, _) in &plain {
                let id = (*n, 0);
                let record = doc.record(id).unwrap();
                let was = copied(&out, &written, record, &reading, id);
                assert_eq!(was, copies, "{name}: {id:?}");
                assert_alike(&handed(&back, id), &handed(&reading, id), &name);
            }
            if encryptor.is_some() {
                for text in SECRETS {
                    let said = String::from_utf8_lossy(text);
                    assert!(!in_clear(&out, text), "{name}: {said:?}");
                }
            }
        }
    }
}

/// Rule 2, the list: under `/EncryptMetadata false`, the document's
/// metadata stream, its data in the clear, once the catalog names another
/// is written anew, encrypted, under the reading's own encryption: copied,
/// its data would be read back decrypted, as any stream's. The one the
/// catalog names now, new, is in the clear; what the list says the same of
/// is copied.
#[test]
fn an_object_the_list_says_otherwise_of_in_the_file_written_isn_t_copied() {
    const NEW_XMP: &[u8] = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">another</x:xmpmeta>";
    let source = written_otherwise(Kind::Aes128, false);
    assert!(holds(&source, XMP), "in the clear in the file read");
    let reading = opened(&source, "user");
    let mut doc = reading.to_document().unwrap();
    let root = (doc.trailer.dict().get(b"Root"))
        .and_then(Object::as_reference)
        .unwrap();
    let old = (doc.get_dict(root).unwrap().get(b"Metadata"))
        .and_then(Object::as_reference)
        .unwrap();
    let new = doc.new_id();
    let dict = dict! { "Type" => "Metadata", "Subtype" => "XML" };
    doc.set(new, Stream::new(dict, NEW_XMP.to_vec()));
    (doc.get_dict_mut(root).unwrap()).set("Metadata", Object::from(new));
    let encryptor = Encryptor::of(&reading).unwrap();
    let mut out = Vec::new();
    let options = options(XrefKind::Table, None);
    let written =
        write::document(&mut out, &doc, encryptor.as_ref(), Some(&reading), &options).unwrap();
    let copies = |id| copied(&out, &written, doc.record(id).unwrap(), &reading, id);
    assert!(!copies(old), "the old metadata, written anew");
    assert!(copies(PAGE), "the page, copied");
    assert!(!in_clear(&out, XMP) && holds(&out, NEW_XMP));
    let back = opened(&out, "user");
    assert!(sound(&back), "{:?}", back.warnings());
    assert_eq!(back.data(old).unwrap().as_ref(), XMP);
    assert_eq!(back.data(new).unwrap().as_ref(), NEW_XMP);
}

/// Rule 3: a copy is its number's, and its record's object's: an object as
/// read given at another number, or with another object's record, or one
/// read from an object stream given another's place there, refuses the
/// write, nothing written, under any encryption, a new one where nothing
/// at an offset is copied too; and an update after the reading's file,
/// which passes over what it holds, refuses one at another number rather
/// than pass it over. One an edit changed, which dropped its place, is
/// written anew with its record, the edit kept, and an update writes it.
#[test]
fn a_copy_is_of_its_own_number_and_object_or_refused() {
    use chacha20::ChaCha20Rng;

    let file = spelled();
    let reading = open(&file);
    let doc = reading.to_document().unwrap();
    let trailer = doc.trailer.dict();
    let table = options(XrefKind::Table, None);
    let write = |given: Vec<(ObjectId, write::Body<'_>)>, options: &write::Options<'_>| {
        let mut out = Vec::new();
        let written = write::full(&mut out, given, trailer, None, Some(&reading), options);
        assert!(written.is_ok() || out.is_empty());
        written.map(|written| (out, written))
    };
    // At another number.
    let mut given: Vec<_> = write::bodies(&doc).collect();
    given.push(((9, 0), read_as(&doc, (4, 0))));
    assert_eq!(
        write(given.clone(), &table).err(),
        Some(Error::Elsewhere((9, 0)))
    );
    // So too under a new encryption, which copies nothing at an offset.
    let prepared = Password("user").prepared().unwrap();
    let mut rng = ChaCha20Rng::seed_from_u64(12);
    let created = Encryptor::create(&prepared, true, &mut rng).unwrap();
    let mut out = Vec::new();
    let refused = write::full(
        &mut out,
        given,
        trailer,
        Some(&created),
        Some(&reading),
        &table,
    );
    assert_eq!(refused.err(), Some(Error::Elsewhere((9, 0))));
    assert!(out.is_empty());
    // And by an update after the reading's file, with or without it.
    let base = write::Base::of(&reading).unwrap();
    let from: &dyn write::ReadFrom = &reading;
    for from in [Some(from), None] {
        let mut out = file.clone();
        let given = [((9, 0), read_as(&doc, (4, 0)))];
        let refused = write::update(&mut out, &base, given, None, from, &table);
        assert_eq!(refused, Err(Error::Elsewhere((9, 0))));
        assert_eq!(out, file);
    }
    // With another object's record.
    let given: Vec<_> = (write::bodies(&doc))
        .map(|(id, body)| match id {
            (4, 0) => {
                let object = Cow::Borrowed(doc.get((5, 0)).unwrap());
                let record = Cow::Borrowed(doc.record((4, 0)).unwrap());
                (id, write::Body::Read { object, record })
            }
            _ => (id, body),
        })
        .collect();
    assert_eq!(write(given, &table).err(), Some(Error::Elsewhere((4, 0))));
    // Edited, written anew with the edit.
    let mut page = doc.get((3, 0)).unwrap().clone();
    page.as_dict_mut().unwrap().set("Rotate", 90);
    let given: Vec<_> = (write::bodies(&doc))
        .map(|(id, body)| match id {
            (3, 0) => {
                let record = Cow::Borrowed(doc.record(id).unwrap());
                let object = Cow::Borrowed(&page);
                (id, write::Body::Read { object, record })
            }
            _ => (id, body),
        })
        .collect();
    let (out, _) = write(given.clone(), &table).unwrap();
    assert!(!holds(&out, b"1.50"));
    let back = open(&out);
    assert_eq!(value(&back, (3, 0)), unplaced(&page));
    let mut out = file.clone();
    let written = write::update(&mut out, &base, given, None, Some(&reading), &table);
    assert!(written.unwrap().unwrap().section.lists(3));
    assert_eq!(value(&open(&out), (3, 0)), unplaced(&page));

    // In an object stream, given another member's place.
    let packed = members_spelled();
    let reading = open(&packed);
    let doc = reading.to_document().unwrap();
    let pack = options(XrefKind::Stream, Some(write::Pack::DEFAULT));
    let given: Vec<_> = (write::bodies(&doc))
        .map(|(id, body)| match id {
            (5, 0) => (id, read_as(&doc, (4, 0))),
            _ => (id, body),
        })
        .collect();
    let mut out = Vec::new();
    let trailer = doc.trailer.dict();
    let refused = write::full(&mut out, given, trailer, None, Some(&reading), &pack);
    assert_eq!(refused, Err(Error::Elsewhere((5, 0))));
    assert!(out.is_empty());
}

/// Members spelled as a writer other than this one spells them, in an
/// object stream: a hex string, odd spacing, a real `1.50`.
const MEMBERS_SPELLED: [(u32, &[u8]); 3] = [
    (4, b"<48656C6C6F>"),
    (5, b"[ 1   2 3 ]"),
    (7, b"<</UserUnit 1.50>>"),
];

/// A file of a catalog, its page tree of a page, and an object stream,
/// number 20, of [`MEMBERS_SPELLED`], as they're spelled, listed in it by
/// a cross-reference stream.
pub(crate) fn members_spelled() -> Vec<u8> {
    members_spelled_as(&MEMBERS_SPELLED)
}

/// [`members_spelled`]'s file, its object stream of `members`, as they're
/// spelled.
fn members_spelled_as(members: &[(u32, &[u8])]) -> Vec<u8> {
    let members: Vec<(u32, Vec<u8>)> = (members.iter())
        .map(|(n, bytes)| (*n, bytes.to_vec()))
        .collect();
    let container = write::container(&members, Deflate::DEFAULT)
        .unwrap()
        .unwrap();
    let kids = vec![Object::reference(3)];
    let mediabox = [0, 0, 612, 792].map(Object::from).to_vec();
    let page = dict! {
        "Type" => "Page",
        "Parent" => (2, 0),
        "MediaBox" => mediabox,
        "Resources" => Dict::new(),
    };
    let objects = [
        (
            1,
            Object::from(dict! { "Type" => "Catalog", "Pages" => (2, 0) }),
        ),
        (
            2,
            Object::from(dict! { "Type" => "Pages", "Kids" => kids, "Count" => 1 }),
        ),
        (3, Object::from(page)),
        (20, Object::from(container)),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut section = Section::full(0);
    for (n, obj) in &objects {
        let pos = serialize::indirect(&mut out, (*n, 0), obj).unwrap();
        section.insert(
            *n,
            Entry::InUse {
                offset: pos,
                generation: 0,
            },
        );
    }
    for (index, (n, _)) in (0..).zip(&members) {
        let entry = Entry::Compressed { stream: 20, index };
        section.insert(*n, entry);
    }
    let pos = out.len();
    let own = section.size();
    let trailer = dict! { "Root" => (1, 0) };
    section
        .stream(&mut out, own, trailer, pos, Deflate::DEFAULT)
        .unwrap();
    out
}

/// The decoded data of `pdf`'s object streams, one after another.
fn object_streams_data(pdf: &Reading<Arc<[u8]>>) -> Vec<u8> {
    (object_streams_of(pdf).into_iter())
        .flat_map(|id| pdf.decoded(id).unwrap())
        .collect()
}

/// Rule 4: an object found by scanning, the file's sections not reading,
/// isn't copied: written anew, as this writer spells it.
#[test]
fn an_object_found_by_scanning_isn_t_copied() {
    let mut file = spelled();
    // `startxref` pointing at the header, which isn't a section.
    let at = (file.windows(10).rposition(|w| w == b"startxref\n")).unwrap() + 10;
    let digits = file[at..].iter().take_while(|b| b.is_ascii_digit()).count();
    file[at..at + digits].fill(b'0');
    let reading = open(&file);
    let doc = reading.to_document().unwrap();
    assert!(reading.trailer().unwritten.is_some());
    assert!(
        doc.records()
            .all(|(_, r)| r.origin == crate::Origin::Recovered)
    );
    let mut out = Vec::new();
    let options = options(XrefKind::Table, None);
    write::document(&mut out, &doc, None, Some(&reading), &options).unwrap();
    assert!(!holds(&out, b"1.50") && !holds(&out, b"% a comment"));
    let back = open(&out);
    assert!(sound(&back), "{:?}", back.warnings());
    for (id, _) in doc.objects() {
        assert_eq!(handed(&back, id), handed(&reading, id), "{id:?}");
    }
}

/// Rule 5: an object `endobj` doesn't follow isn't copied: written anew,
/// with its `endobj`, the file read back sound.
#[test]
fn an_object_without_endobj_isn_t_copied() {
    let file = spelled();
    let comment = (file.windows(11).position(|w| w == b"% a comment")).unwrap();
    let endobj = comment + (file[comment..].windows(6).position(|w| w == b"endobj")).unwrap();
    let mut file = file;
    file[endobj..endobj + 6].fill(b' ');
    let reading = open(&file);
    let doc = reading.to_document().unwrap();
    assert!(!doc.record((5, 0)).unwrap().endobj);
    let mut out = Vec::new();
    let options = options(XrefKind::Table, None);
    let written = write::document(&mut out, &doc, None, Some(&reading), &options).unwrap();
    assert!(!holds(&out, b"% a comment"));
    assert!(copied(
        &out,
        &written,
        doc.record((4, 0)).unwrap(),
        &reading,
        (4, 0)
    ));
    let back = open(&out);
    assert!(sound(&back), "{:?}", back.warnings());
    assert_eq!(handed(&back, (5, 0)), handed(&reading, (5, 0)));
}

/// Rule 6: a stream whose `/Length` is a reference, or isn't how long its
/// data is, isn't copied: written anew, its `/Length` its data's, the file
/// read back sound, the data as it was.
#[test]
fn a_stream_whose_length_is_a_reference_or_wrong_isn_t_copied() {
    let by_reference: &[u8] = b"<</Length 7 0 R>>\nstream\n0 0 m 1.5 l\nendstream";
    let wrong: &[u8] = b"<</Length 99>>\nstream\n0 0 m 1.5 l\nendstream";
    for (name, stream, length) in [
        ("by reference", by_reference, &b"/Length 7 0 R"[..]),
        ("wrong", wrong, &b"/Length 99"[..]),
    ] {
        let mut objects = SPELLED.to_vec();
        objects[5].1 = stream;
        objects.push((7, b"11"));
        let file = spelled_as(&objects);
        let reading = open(&file);
        let doc = reading.to_document().unwrap();
        let data = doc.record((6, 0)).unwrap().data.unwrap();
        assert!(!data.declared || data.indirect_length, "{name}");
        let mut out = Vec::new();
        let options = options(XrefKind::Table, None);
        let written = write::document(&mut out, &doc, None, Some(&reading), &options).unwrap();
        assert!(!holds(&out, length), "{name}");
        assert!(copied(
            &out,
            &written,
            doc.record((3, 0)).unwrap(),
            &reading,
            (3, 0)
        ));
        let back = open(&out);
        assert!(sound(&back), "{name}: {:?}", back.warnings());
        let (got, was) = (handed(&back, (6, 0)), handed(&reading, (6, 0)));
        assert_alike(&got, &was, name);
        let stream = got.as_stream().unwrap();
        let len = stream.data.bytes().unwrap().len() as i64;
        assert_eq!(
            stream.dict.get(b"Length"),
            Some(&Object::from(len)),
            "{name}"
        );
    }
}

/// `got` is `was`, each as [`handed`] gives an object: a stream's
/// dictionary but its `/Length`, which is its data's as written, and its
/// data as handed out.
fn assert_alike(got: &Object, was: &Object, name: &str) {
    assert!(same(got, was), "{name}: {got:?}");
    if let (Some(got), Some(was)) = (got.as_stream(), was.as_stream()) {
        assert_eq!(got.data, was.data, "{name}");
    }
}

/// Rule 7: an object read from an object stream goes in one of the
/// write's as the bytes it was read from there; written on its own, it's
/// written anew. Read back, the same objects.
#[test]
fn a_member_is_copied_into_an_object_stream_and_alone_written_anew() {
    let file = members_spelled();
    let reading = open(&file);
    let doc = reading.to_document().unwrap();
    assert!(sound(&reading), "{:?}", reading.warnings());
    let ways = [
        (XrefKind::Stream, Some(write::Pack::DEFAULT)),
        (XrefKind::Stream, None),
        (XrefKind::Table, None),
    ];
    for (kind, pack) in ways {
        let name = format!("{kind:?}, packed: {}", pack.is_some());
        let mut out = Vec::new();
        let options = options(kind, pack);
        write::document(&mut out, &doc, None, Some(&reading), &options).unwrap();
        let back = open(&out);
        assert!(sound(&back), "{name}: {:?}", back.warnings());
        for (n, _) in MEMBERS_SPELLED {
            assert_eq!(value(&back, (n, 0)), value(&reading, (n, 0)), "{name}: {n}");
        }
        let data = object_streams_data(&back);
        for (n, bytes) in MEMBERS_SPELLED {
            assert_eq!(holds(&data, bytes), pack.is_some(), "{name}: {n}");
        }
        if pack.is_none() {
            assert!(
                !holds(&out, b"1.50") && !holds(&out, b"[ 1   2 3 ]"),
                "{name}"
            );
        }
    }
}

/// Rule 7, encrypted: a member's bytes, plaintext in any reading, are
/// copied into an object stream of the write's under any encryption: a
/// plain file's into one encrypted under each method that packs, and that
/// one's, decrypted, into a plain one and into one under a new encryption.
/// Each read back, the same objects, and nothing in the clear where it's
/// encrypted.
#[test]
fn members_are_copied_into_object_streams_under_any_encryption() {
    use chacha20::ChaCha20Rng;

    let file = members_spelled();
    let reading = open(&file);
    let doc = reading.to_document().unwrap();
    let prepared = Password("new").prepared().unwrap();
    let mut rng = ChaCha20Rng::seed_from_u64(13);
    let created = Encryptor::create(&prepared, true, &mut rng).unwrap();
    let pack = options(XrefKind::Stream, Some(write::Pack::DEFAULT));
    let assert_back = |out: &[u8], password: &str, name: &str| {
        let back = opened(out, password);
        assert!(sound(&back), "{name}: {:?}", back.warnings());
        let data = object_streams_data(&back);
        for (n, bytes) in MEMBERS_SPELLED {
            assert_eq!(value(&back, (n, 0)), value(&reading, (n, 0)), "{name}: {n}");
            assert!(holds(&data, bytes), "{name}: {n}");
        }
    };
    let methods = methods();
    for method in methods.iter().filter(|m| m.encryptor.packs() && m.streams) {
        let name = method.name;
        let mut encrypted = Vec::new();
        let encryptor = Some(&method.encryptor);
        write::document(&mut encrypted, &doc, encryptor, Some(&reading), &pack).unwrap();
        assert!(!in_clear(&encrypted, b"[ 1   2 3 ]"), "{name}");
        assert_back(&encrypted, "user", name);
        let again = opened(&encrypted, "user");
        let doc = again.to_document().unwrap();
        for (way, encryptor, password) in
            [("decrypted", None, ""), ("created", Some(&created), "new")]
        {
            let name = format!("{name}, {way}");
            let mut out = Vec::new();
            write::document(&mut out, &doc, encryptor, Some(&again), &pack).unwrap();
            assert_eq!(
                in_clear(&out, b"[ 1   2 3 ]"),
                encryptor.is_none(),
                "{name}"
            );
            assert_back(&out, password, &name);
        }
    }
}

/// Rule 8: a copy whose bytes changed since they were read refuses the
/// write, whole or an update, nothing written: a byte of the file, flipped
/// after it was read, in an object copied at an offset. One flipped where
/// nothing's copied refuses nothing.
#[test]
fn a_copy_whose_bytes_changed_since_they_were_read_is_refused() {
    let file = spelled();
    let source = Flipped::new(&file, 64);
    let reading = Reading::open(&source, &Options::DEFAULT).unwrap();
    let doc = reading.to_document().unwrap();
    let options = options(XrefKind::Table, None);
    let mut out = Vec::new();
    write::document(&mut out, &doc, None, Some(&reading), &options).unwrap();
    // `1.50`'s `5`, in the page.
    let at = (file.windows(4).position(|w| w == b"1.50")).unwrap() + 2;
    let page = doc.record((3, 0)).unwrap().at;
    assert!(page.start < at && at < page.end);
    source.flip(at);
    let mut out = Vec::new();
    let refused = write::document(&mut out, &doc, None, Some(&reading), &options);
    assert_eq!(refused, Err(Error::Changed(Some((3, 0)))));
    assert!(out.is_empty());
    let mut base = write::Base::of(&reading).unwrap();
    base.source = None;
    let mut updated = file.clone();
    let given = write::bodies(&doc);
    let refused = write::update(&mut updated, &base, given, None, Some(&reading), &options);
    assert_eq!(refused, Err(Error::Changed(Some((3, 0)))));
    assert_eq!(updated, file);
    // A byte of the table, which nothing copied holds.
    let xref = (file.windows(5).rposition(|w| w == b"xref\n")).unwrap();
    source.flip(xref + 8);
    write::document(&mut out, &doc, None, Some(&reading), &options).unwrap();
    let (back, was) = (open(&out), open(&file));
    assert!(sound(&back), "{:?}", back.warnings());
    for (id, _) in doc.objects() {
        assert_eq!(handed(&back, id), handed(&was, id), "{id:?}");
    }
}

/// A file read by position that changed since it was read, as another
/// program's update changes it, refuses a write given its reading before
/// a byte is written ([`ByteSource::unchanged`]), whole or an update,
/// though what's copied of it is as it was.
#[cfg(any(unix, windows))]
#[test]
fn a_file_changed_since_it_was_read_refuses_the_write_before_a_byte() {
    use std::io::Write as _;

    let file = spelled();
    let dir = (tempfile::Builder::new())
        .prefix("pdf-structure-changed-")
        .tempdir()
        .unwrap();
    let path = dir.path().join("changed.pdf");
    std::fs::write(&path, &file).unwrap();
    let source = crate::bytes::FileBytes::new(std::fs::File::open(&path).unwrap()).unwrap();
    let reading = Reading::open(source, &Options::DEFAULT).unwrap();
    let doc = reading.to_document().unwrap();
    let options = options(XrefKind::Table, None);
    let mut out = Vec::new();
    write::document(&mut out, &doc, None, Some(&reading), &options).unwrap();
    let appended = std::fs::OpenOptions::new().append(true).open(&path);
    appended.unwrap().write_all(b"\n").unwrap();
    assert!(!ByteSource::unchanged(reading.source()).unwrap());
    let mut out = Vec::new();
    let refused = write::document(&mut out, &doc, None, Some(&reading), &options);
    assert_eq!(refused, Err(Error::Changed(None)));
    assert!(out.is_empty());
    let mut base = write::Base::of(&reading).unwrap();
    base.source = None;
    let mut updated = file.clone();
    let given = write::bodies(&doc);
    let refused = write::update(&mut updated, &base, given, None, Some(&reading), &options);
    assert_eq!(refused, Err(Error::Changed(None)));
    assert_eq!(updated, file);
    // Given no reading, nothing's read of it, so nothing's asked.
    let given = [((1, 0), write::Body::from(doc.get((1, 0)).unwrap()))];
    let trailer = doc.trailer.dict();
    write::full(&mut out, given, trailer, None, None, &options).unwrap();
}

/// An update after a reading's file passes over each object as that
/// reading read it, which the file holds: given a document's bodies, it
/// writes what was built or changed since, and nothing where nothing was.
/// A second, after the first's `Written::base`, passes over them too, and
/// writes what was changed since the reading, which the first wrote and
/// one more. Read back with three sections, each object as last written.
#[test]
fn an_update_writes_what_of_a_document_changed_since_it_was_read() {
    let file = spelled();
    let reading = open(&file);
    let mut doc = reading.to_document().unwrap();
    let base = write::Base::of(&reading).unwrap();
    let options = write::Options::DEFAULT;
    let mut out = file.clone();
    let none = write::update(
        &mut out,
        &base,
        write::bodies(&doc),
        None,
        Some(&reading),
        &options,
    );
    assert_eq!(none, Ok(None));
    assert_eq!(out, file);
    let listed = |w: &write::Written| (1..8).filter(|&n| w.section.lists(n)).collect::<Vec<u32>>();
    doc.get_dict_mut((3, 0)).unwrap().set("Rotate", 90);
    let first = write::update(
        &mut out,
        &base,
        write::bodies(&doc),
        None,
        Some(&reading),
        &options,
    )
    .unwrap()
    .unwrap();
    assert_eq!(listed(&first), [3]);
    let array = doc.get_mut((5, 0)).unwrap().as_array_mut().unwrap();
    array.push(Object::from(4));
    let second = write::bodies(&doc);
    let second = write::update(
        &mut out,
        &first.base,
        second,
        None,
        Some(&reading),
        &options,
    )
    .unwrap()
    .unwrap();
    assert_eq!(listed(&second), [3, 5]);
    let back = open(&out);
    assert!(sound(&back), "{:?}", back.warnings());
    assert_eq!(back.facts().sections, 3);
    for (id, object) in doc.objects() {
        let stream = object.as_stream().is_some();
        let want = match stream {
            true => handed(&reading, id),
            false => unplaced(object),
        };
        assert_eq!(handed(&back, id), want, "{id:?}");
    }
}

// Whole files under a renumbering (`write::Options::renumber`): each object
// at the number it gives, each reference named as it names it, null where
// it names none, the trailer's too; encrypted in the numbers written, the
// list of what's left in the clear among them; copied, its header and the
// references that move written anew in place.

/// A renumbering, each object to where it's written: a reference to one it
/// doesn't map is written null.
type Map = std::collections::BTreeMap<ObjectId, ObjectId>;

/// `map` as the writer's renumbering.
fn by(map: &Map) -> impl Fn(ObjectId) -> Option<ObjectId> + Sync + '_ {
    |id| map.get(&id).copied()
}

/// Every one of `ids`, ascending, at the number of the one as far from the
/// end as it is from the start, at its own generation.
fn reversed(ids: &[ObjectId]) -> Map {
    let numbers = ids.iter().rev().map(|id| id.0);
    (ids.iter().zip(numbers))
        .map(|(&id, n)| (id, (n, id.1)))
        .collect()
}

/// Every one of `ids` `past` on.
fn shifted(ids: &[ObjectId], past: u32) -> Map {
    ids.iter().map(|&(n, g)| ((n, g), (n + past, g))).collect()
}

/// `ids`, ascending, numbered from 1 at generation 0, one `alike` gives an
/// earlier one for at that one's number, each passing over `free`: as a
/// writer that numbers a file's objects densely does, two streams written
/// alike sharing a number.
fn dense(
    ids: &[ObjectId],
    alike: impl Fn(ObjectId) -> Option<ObjectId>,
    free: &std::collections::BTreeSet<u32>,
) -> Map {
    let mut map = Map::new();
    let mut next = 1;
    for &id in ids {
        if let Some(first) = alike(id) {
            let at = map[&first];
            map.insert(id, at);
            continue;
        }
        while free.contains(&next) {
            next += 1;
        }
        map.insert(id, (next, 0));
        next += 1;
    }
    map
}

/// Each stream of `ids` in `pdf` that's alike, as handed out, one before
/// it, with the first it's alike.
fn alike_streams(pdf: &Reading<Arc<[u8]>>, ids: &[ObjectId]) -> Map {
    let mut seen: Vec<(ObjectId, Object)> = Vec::new();
    let mut alike = Map::new();
    for &id in ids {
        let o = handed(pdf, id);
        if o.as_stream().is_none() {
            continue;
        }
        match seen.iter().find(|(_, was)| *was == o) {
            Some((first, _)) => {
                alike.insert(id, *first);
            }
            None => seen.push((id, o)),
        }
    }
    alike
}

/// `back` holds each of `ids`, as `pdf` reads it, at the number `to` gives
/// it, its references as `to` names them, null where it names none: its
/// value as built, a stream's dictionary but its `/Length`, and its data as
/// handed out.
fn assert_moved(
    pdf: &Reading<Arc<[u8]>>,
    back: &Reading<Arc<[u8]>>,
    ids: &[ObjectId],
    to: &dyn Fn(ObjectId) -> Option<ObjectId>,
    name: &str,
) {
    for &id in ids {
        let mut want = handed(pdf, id);
        want.renumber(to);
        let at = to(id).unwrap();
        let got = handed(back, at);
        assert_alike(&got, &want, &format!("{name}: {id:?} at {at:?}"));
    }
}

/// `pdf`'s objects a whole file of its document writes under its own
/// encryption: but object streams, cross-reference streams, a
/// linearization dictionary and its encryption dictionary.
fn written_of(doc: &crate::Document) -> Vec<ObjectId> {
    let encrypt = (doc.trailer.dict().get(b"Encrypt")).and_then(Object::as_reference);
    (doc.objects())
        .filter(|(id, o)| {
            let left_out = o
                .as_stream()
                .is_some_and(|s| s.dict.has_type(b"ObjStm") || s.dict.has_type(b"XRef"))
                || o.as_dict().is_some_and(|d| d.has(b"Linearized"));
            !left_out && Some(*id) != encrypt
        })
        .map(|(id, _)| id)
        .collect()
}

/// [`SPELLED`] with a seventh object, a stream alike its sixth, both the
/// page's content: two streams a dense renumbering puts at one number.
fn shared() -> Vec<u8> {
    let page: &[u8] = b"<</Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources <<>> /Contents [6 0 R 7 0 R]>>";
    let mut objects = SPELLED.to_vec();
    objects[2].1 = page;
    objects.push((7, SPELLED[5].1));
    spelled_as(&objects)
}

/// Every test file (qpdf's committed ones where they are, by the empty
/// password, [`spelled`], [`members_spelled`] and [`shared`]) written whole
/// by its document and reading, under its own encryption, a table and a
/// stream packed, under each renumbering, and read back. Under the
/// identity, given, it's the bytes written without one, copies and all.
/// Reversed, shifted past every number, and dense (from 1, a stream alike
/// one before it at that one's number, given through `full` with the later
/// left out, which `document`, giving every object, refuses), each object
/// is the source's at its number, its references renumbered, the trailer's
/// `/Root` too, and the write said renumbered. And moved down from past
/// [`write::NUMBERS`], decrypted, written anew, the same.
#[test]
fn every_test_file_is_written_whole_under_each_renumbering_and_read_back_alike() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/files");
    let mut paths: Vec<_> = match bytes::lazy::fixtures() {
        true => (std::fs::read_dir(dir).unwrap())
            .map(|entry| entry.unwrap().path())
            .collect(),
        false => Vec::new(),
    };
    paths.sort();
    assert_eq!(paths.len(), if bytes::lazy::fixtures() { 8 } else { 0 });
    let mut files: Vec<(String, Vec<u8>)> = (paths.iter())
        .map(|p| (p.display().to_string(), std::fs::read(p).unwrap()))
        .collect();
    files.push(("spelled".into(), spelled()));
    files.push(("members spelled".into(), members_spelled()));
    files.push(("shared".into(), shared()));
    let mut sharing = 0;
    let mut copies = 0;
    for (file, bytes) in &files {
        let reading = opened(bytes, "");
        let encryptor = Encryptor::of(&reading).unwrap();
        let encrypted = encryptor.is_some();
        let doc = reading.to_document().unwrap();
        let ids: Vec<ObjectId> = doc.objects().map(|(id, _)| id).collect();
        let kept = written_of(&doc);
        let free: std::collections::BTreeSet<u32> = (encryptor.iter())
            .flat_map(|e| e.free_numbers().map(|(n, _)| n))
            .collect();
        let top = (ids.iter().map(|id| id.0))
            .chain(free.iter().copied())
            .max()
            .unwrap()
            + 1;
        let alike = alike_streams(&reading, &kept);
        sharing += alike.len();
        let maps = [
            ("reversed", reversed(&ids)),
            ("shifted", shifted(&ids, top)),
            ("dense", dense(&kept, |id| alike.get(&id).copied(), &free)),
        ];
        let root = doc
            .trailer
            .dict()
            .get(b"Root")
            .and_then(Object::as_reference);
        let ways = [
            (XrefKind::Table, None),
            (XrefKind::Stream, Some(write::Pack::DEFAULT)),
        ];
        let per_object = encryptor.as_ref().is_some_and(Encryptor::per_object);
        for (kind, pack) in ways {
            let name = format!("{file}, {kind:?}, packed: {}", pack.is_some());
            let enc = encryptor.as_ref();
            let (unrenumbered, unrenumbered_written) = {
                let mut out = Vec::new();
                let options = options(kind, pack);
                let written =
                    write::document(&mut out, &doc, enc, Some(&reading), &options).unwrap();
                (out, written)
            };
            let identity = |id| Some(id);
            let mut options = options(kind, pack);
            options.renumber = Some(&identity);
            let mut out = Vec::new();
            let written = write::document(&mut out, &doc, enc, Some(&reading), &options).unwrap();
            assert_eq!(out, unrenumbered, "{name}: under the identity");
            assert!(written.base.renumbered, "{name}");
            for (way, map) in &maps {
                let name = format!("{name}, {way}");
                let to = by(map);
                let mut options = options;
                options.renumber = Some(&to);
                let shares = *way == "dense" && !alike.is_empty();
                let given = |(id, _): &(ObjectId, write::Body<'_>)| {
                    kept.contains(id) && !(shares && alike.contains_key(id))
                };
                let mut out = Vec::new();
                let trailer = doc.trailer.dict();
                let bodies = write::bodies(&doc).filter(given);
                let written =
                    write::full(&mut out, bodies, trailer, enc, Some(&reading), &options).unwrap();
                assert!(written.base.renumbered, "{name}");
                // Each object at an offset copied without a renumbering is
                // copied under it, where its key isn't made of its number or
                // it stays, its header and references spliced.
                for (id, body) in write::bodies(&doc).filter(given) {
                    let write::Body::Read { object, record } = &body else {
                        continue;
                    };
                    let was = copied(&unrenumbered, &unrenumbered_written, record, &reading, id);
                    let at = to(id).unwrap();
                    let Some(Entry::InUse { offset, .. }) = written.section.get(at.0) else {
                        continue;
                    };
                    if was && (at == id || !per_object) {
                        let copy = copy_of(&reading, record, object, at, &to);
                        assert!(out[offset..].starts_with(&copy), "{name}: {id:?} at {at:?}");
                        copies += usize::from(at != id);
                    }
                }
                let back = opened(&out, "");
                assert!(sound(&back), "{name}: {:?}", back.warnings());
                let opens = matches!(back.encryption(), Encryption::Opened { .. });
                assert_eq!(opens, encrypted, "{name}");
                assert_moved(&reading, &back, &kept, &to, &name);
                let back_root = back.trailer().dict().get(b"Root");
                assert_eq!(
                    back_root.and_then(Object::as_reference),
                    root.and_then(&to),
                    "{name}"
                );
                // Through `document`, every object given: the same, but
                // where two share a number.
                let mut again = Vec::new();
                let through = write::document(&mut again, &doc, enc, Some(&reading), &options);
                match shares {
                    false => assert_eq!(again, out, "{name}: through document"),
                    true => {
                        let later = alike.keys().next().unwrap();
                        assert_eq!(through, Err(Error::Twice(map[later].0)), "{name}");
                        assert!(again.is_empty(), "{name}");
                    }
                }
            }
            // Moved down from past the numbers a file may use: given as
            // built, its data as bytes, decrypted.
            let left = |id: &ObjectId| {
                let encrypt = (doc.trailer.dict().get(b"Encrypt")).and_then(Object::as_reference);
                let referred = encrypt
                    .and_then(|e| doc.get(e).ok())
                    .is_some_and(|e| e.references().any(|r| r == *id));
                Some(*id) == encrypt || referred
            };
            let plain: Vec<ObjectId> = kept.iter().copied().filter(|id| !left(id)).collect();
            let up = |(n, g): ObjectId| Some((n + write::NUMBERS, g));
            let given: Vec<(ObjectId, Object)> = (plain.iter())
                .map(|&id| {
                    let mut o = handed(&reading, id);
                    o.renumber(up);
                    (up(id).unwrap(), o)
                })
                .collect();
            let mut trailer = Object::from(doc.trailer.dict().clone());
            trailer.renumber(up);
            let trailer = trailer.as_dict().unwrap().clone();
            let down = |(n, g): ObjectId| n.checked_sub(write::NUMBERS).map(|n| (n, g));
            let mut moved = options;
            moved.renumber = Some(&down);
            let mut out = Vec::new();
            let bodies = given.iter().map(|(id, o)| (*id, write::Body::from(o)));
            write::full(&mut out, bodies, &trailer, None, None, &moved).unwrap();
            let back = open(&out);
            let name = format!("{name}, moved down");
            assert!(sound(&back), "{name}: {:?}", back.warnings());
            for &id in &plain {
                assert_alike(&handed(&back, id), &handed(&reading, id), &name);
            }
        }
    }
    assert!(sharing > 0, "a file with two streams alike");
    assert!(copies > 0, "copies at new numbers");
}

/// [`SPELLED`]'s objects and a seventh naming others, as another writer
/// spells them: a reference with a comment between its tokens, one in an
/// array, a string that reads as one, and one to object 9, which nothing
/// holds.
const NAMING: (u32, &[u8]) = (
    7,
    b"<</A 2 % between\n0 R /B [3 0 R] /C (2 0 R) /D 4 0 R /E 9 0 R>>",
);

/// The bytes `written` lists object number `n` at in `out`, to its end.
fn listed_at<'o>(out: &'o [u8], written: &write::Written, n: u32) -> &'o [u8] {
    match written.section.get(n) {
        Some(Entry::InUse { offset, .. }) => &out[offset..],
        other => panic!("{n} isn't at an offset: {other:?}"),
    }
}

/// Under a renumbering, an object as read is copied as the bytes it was
/// read from, but its header where it's written at another number and each
/// reference the renumbering names otherwise, `null` where it names none,
/// written where its bytes had it: odd spacing, a real `1.50`, a hex
/// string, a comment, a stream's data and a string that reads as a
/// reference as they were, and a comment between a changed reference's
/// tokens gone. With a table and with a stream; read back, the objects
/// renumbered.
#[test]
fn a_copy_under_a_renumbering_is_its_bytes_but_its_header_and_the_references_moved() {
    let mut objects = SPELLED.to_vec();
    objects.push(NAMING);
    let file = spelled_as(&objects);
    let reading = open(&file);
    let doc = reading.to_document().unwrap();
    let ids: Vec<ObjectId> = doc.objects().map(|(id, _)| id).collect();
    // 2, 4, 6 and 7 move ten on; 1, 3 and 5 stay; 9 is no object's.
    let moves = |n: u32| n % 2 == 0 || n == 7;
    let map: Map = (ids.iter())
        .map(|&(n, g)| ((n, g), (if moves(n) { n + 10 } else { n }, g)))
        .collect();
    let to = by(&map);
    let want: [(u32, &[u8]); 7] = [
        (1, b"1 0 obj\n<</Type /Catalog  /Pages 12 0 R>>\nendobj\n"),
        (12, b"12 0 obj\n<</Type /Pages /Kids [3 0 R] /Count 1>>\nendobj\n"),
        (
            3,
            b"3 0 obj\n<</Type /Page /Parent 12 0 R /MediaBox [0 0 612 792] /UserUnit 1.50 /Resources <<>> /Contents 16 0 R>>\nendobj\n",
        ),
        (14, b"14 0 obj\n<48656C6C6F>\nendobj\n"),
        (5, b"5 0 obj\n[ 1   2 % a comment\n 3 ]\nendobj\n"),
        (
            16,
            b"16 0 obj\n<</Length 11>>\nstream\n0 0 m 1.5 l\nendstream\nendobj\n",
        ),
        (
            17,
            b"17 0 obj\n<</A 12 0 R /B [3 0 R] /C (2 0 R) /D 14 0 R /E null>>\nendobj\n",
        ),
    ];
    for kind in KINDS {
        let mut options = options(kind, None);
        options.renumber = Some(&to);
        let mut out = Vec::new();
        let written = write::document(&mut out, &doc, None, Some(&reading), &options).unwrap();
        for (n, bytes) in want {
            let at = listed_at(&out, &written, n);
            let got = String::from_utf8_lossy(&at[..bytes.len().min(at.len())]);
            assert!(at.starts_with(bytes), "{kind:?}: {n}: {got:?}");
        }
        for n in [2, 4, 6, 7] {
            assert!(!written.section.lists(n), "{kind:?}: {n}");
        }
        let back = open(&out);
        assert!(sound(&back), "{kind:?}: {:?}", back.warnings());
        assert_moved(&reading, &back, &ids, &to, &format!("{kind:?}"));
    }
}

/// A whole file under a renumbering is refused, nothing written: an object
/// it gives no number, by the number given; two it writes at one number,
/// by that number; a number given twice; an object written at 0 or past
/// [`write::NUMBERS`], or at the encryption dictionary's number, by the
/// number written. Through `document`, which gives every object, a
/// renumbering that puts two at one number is refused. One given past
/// [`write::NUMBERS`] and written below it is written.
#[test]
fn a_renumbering_that_writes_an_object_nowhere_or_twice_is_refused() {
    let catalog = catalog();
    let three = Object::from(dict! { "N" => 3 });
    let trailer = dict! { "Root" => (1, 0) };
    let at = |n: u32, o| ((n, 0), write::Body::from(o));
    let methods = methods();
    let created = &methods
        .iter()
        .find(|m| m.name == "AES-256 created")
        .unwrap();
    let mut encrypt_at = write::Options::DEFAULT;
    encrypt_at.encrypt_at = Some((7, 0));
    let cases: Vec<(Map, Vec<_>, Option<&Encryptor>, write::Options<'_>, Error)> = vec![
        (
            Map::from([((1, 0), (1, 0))]),
            vec![at(1, &catalog), at(3, &three)],
            None,
            write::Options::DEFAULT,
            Error::Unnumbered((3, 0)),
        ),
        (
            Map::from([((1, 0), (1, 0)), ((3, 0), (1, 0))]),
            vec![at(1, &catalog), at(3, &three)],
            None,
            write::Options::DEFAULT,
            Error::Twice(1),
        ),
        (
            Map::from([((1, 0), (1, 0)), ((1, 1), (2, 0))]),
            vec![at(1, &catalog), ((1, 1), write::Body::from(&three))],
            None,
            write::Options::DEFAULT,
            Error::Twice(1),
        ),
        (
            Map::from([((1, 0), (1, 0)), ((3, 0), (0, 0))]),
            vec![at(1, &catalog), at(3, &three)],
            None,
            write::Options::DEFAULT,
            Error::Number(0),
        ),
        (
            Map::from([((1, 0), (1, 0)), ((3, 0), (write::NUMBERS, 0))]),
            vec![at(1, &catalog), at(3, &three)],
            None,
            write::Options::DEFAULT,
            Error::Number(write::NUMBERS),
        ),
        (
            Map::from([((1, 0), (1, 0)), ((3, 0), (7, 0))]),
            vec![at(1, &catalog), at(3, &three)],
            Some(&created.encryptor),
            encrypt_at,
            Error::EncryptAt(7),
        ),
    ];
    for (map, given, encryptor, mut options, want) in cases {
        let to = by(&map);
        options.renumber = Some(&to);
        let mut out = b"held".to_vec();
        let refused = write::full(&mut out, given, &trailer, encryptor, None, &options);
        assert_eq!(refused, Err(want.clone()), "{want}");
        assert_eq!(out, b"held", "{want}");
    }
    assert_eq!(
        Error::Unnumbered((3, 0)).to_string(),
        "object 3 0 is given, and the renumbering gives it no number to be written at"
    );
    assert_eq!(
        Error::Twice(1).to_string(),
        "object number 1 is given twice, or two objects are written there"
    );
    // Given past the numbers a file may use, written below them.
    let far = (write::NUMBERS + 5, 0);
    let mut moved = catalog.clone();
    moved.as_dict_mut().unwrap().set("Far", Object::from(far));
    let map = Map::from([((1, 0), (1, 0)), (far, (2, 0))]);
    let to = by(&map);
    let mut options = write::Options::DEFAULT;
    options.renumber = Some(&to);
    let mut out = Vec::new();
    let given = [at(1, &moved), (far, write::Body::from(&three))];
    write::full(&mut out, given, &trailer, None, None, &options).unwrap();
    let back = open(&out);
    assert_eq!(value(&back, (2, 0)), three);
    let named = back.get((1, 0)).unwrap().as_dict().unwrap().get(b"Far");
    assert_eq!(named.and_then(Object::as_reference), Some((2, 0)));
    // Through `document`, two alike streams at one number.
    let file = shared();
    let reading = open(&file);
    let doc = reading.to_document().unwrap();
    let ids: Vec<ObjectId> = doc.objects().map(|(id, _)| id).collect();
    let alike = alike_streams(&reading, &ids);
    assert_eq!(alike, Map::from([((7, 0), (6, 0))]));
    let map = dense(&ids, |id| alike.get(&id).copied(), &Default::default());
    let to = by(&map);
    let mut options = write::Options::DEFAULT;
    options.renumber = Some(&to);
    let mut out = Vec::new();
    let refused = write::document(&mut out, &doc, None, Some(&reading), &options);
    assert_eq!(refused, Err(Error::Twice(6)));
    assert!(out.is_empty());
}

/// A reference to an object the renumbering names none of is written
/// `null`, as a reference to no object reads (ISO 32000-2, 7.3.10): in an
/// object, in a stream's dictionary, in an object stream's member, and in
/// the trailer; and the writer's own objects (the object streams, the
/// cross-reference stream, a new encryption dictionary) pass over a number
/// the renumbering gives a reference to an object not written.
#[test]
fn a_reference_the_renumbering_names_none_of_is_written_null() {
    use chacha20::ChaCha20Rng;

    let objects = [
        (
            1,
            Object::from(dict! { "Type" => "Catalog", "Gone" => (9, 0), "Kept" => (30, 0) }),
        ),
        (
            2,
            Object::from(Stream::new(dict! { "Gone" => (9, 0) }, b"data".to_vec())),
        ),
        (3, Object::from(vec![Object::reference(9), Object::from(3)])),
    ];
    let trailer = dict! { "Root" => (1, 0), "Info" => (9, 0) };
    // 1 to 3 stay; 30, which nothing holds, is written as 4; 9 is no
    // object's.
    let map: Map = [1, 2, 3]
        .map(|n| ((n, 0), (n, 0)))
        .into_iter()
        .chain([((30, 0), (4, 0))])
        .collect();
    let to = by(&map);
    let mut rng = ChaCha20Rng::seed_from_u64(14);
    let prepared = Password("user").prepared().unwrap();
    let created = Encryptor::create(&prepared, true, &mut rng).unwrap();
    for (kind, pack, encryptor) in [
        (XrefKind::Table, None, None),
        (XrefKind::Stream, Some(write::Pack::DEFAULT), None),
        (XrefKind::Table, None, Some(&created)),
    ] {
        let name = format!("{kind:?}, packed: {}", pack.is_some());
        let mut options = options(kind, pack);
        options.renumber = Some(&to);
        let mut out = Vec::new();
        let written = write::full(
            &mut out,
            bodies(&objects),
            &trailer,
            encryptor,
            None,
            &options,
        )
        .unwrap();
        let back = opened(&out, "user");
        assert!(sound(&back), "{name}: {:?}", back.warnings());
        let catalog = back.get((1, 0)).unwrap().as_dict().unwrap();
        assert!(catalog.get(b"Gone").unwrap().is_null(), "{name}");
        assert_eq!(
            catalog.get(b"Kept").and_then(Object::as_reference),
            Some((4, 0)),
            "{name}"
        );
        let stream = back.get((2, 0)).unwrap().as_stream().unwrap();
        assert!(stream.dict.get(b"Gone").unwrap().is_null(), "{name}");
        let array = back.get((3, 0)).unwrap().as_array().unwrap();
        assert!(array[0].is_null(), "{name}");
        let info = back.trailer().dict().get(b"Info").unwrap();
        assert!(info.is_null(), "{name}: {info:?}");
        assert!(
            written.base.trailer.get(b"Info").unwrap().is_null(),
            "{name}"
        );
        // 4, named and not written, taken by none of the writer's own.
        assert!(back.get((4, 0)).is_err(), "{name}");
        assert!(!written.section.lists(4), "{name}");
        if encryptor.is_some() {
            let encrypt = back.trailer().dict().get(b"Encrypt").unwrap();
            assert_eq!(encrypt.as_reference(), Some((5, 0)), "{name}");
        }
        if pack.is_some() {
            assert_eq!(object_streams_of(&back), [(5, 0)], "{name}");
            assert_eq!(back.facts().max_stream, Some(6), "{name}");
        }
    }
}

/// An object written anew under a renumbering is the bytes its value
/// renumbered in memory writes at its new number, and so is the file: plain
/// and under RC4, AES-128 and AES-256, a table reversed and a stream packed
/// shifted (packed by number, which shifting keeps in order).
#[test]
fn written_anew_renumbered_is_the_value_renumbered_written() {
    let (plain, trailer) = plaintext();
    let methods = methods();
    let encryptors = (methods.iter())
        .filter(|m| ["RC4-128", "AES-128", "AES-256 R6"].contains(&m.name))
        .map(|m| (m.name, Some(&m.encryptor)));
    let ids: Vec<ObjectId> = (plain.iter().map(|(n, _)| (*n, 0)))
        .chain([(ENCRYPT, 0)])
        .collect();
    for (name, encryptor) in std::iter::once(("plain", None)).chain(encryptors) {
        let ways = [
            (XrefKind::Table, None, reversed(&ids)),
            (
                XrefKind::Stream,
                Some(write::Pack::DEFAULT),
                shifted(&ids, ENCRYPT + 1),
            ),
        ];
        for (kind, pack, map) in ways {
            let name = format!("{name}, {kind:?}");
            let to = by(&map);
            let mut options = options(kind, pack);
            options.renumber = Some(&to);
            let mut out = Vec::new();
            write::full(
                &mut out,
                bodies(&plain),
                &trailer,
                encryptor,
                None,
                &options,
            )
            .unwrap();
            // The same objects renumbered in memory, written at their
            // numbers, under the encryptor in the numbers written.
            let moved: Vec<(ObjectId, Object)> = (plain.iter())
                .map(|(n, o)| {
                    let mut o = o.clone();
                    o.renumber(&to);
                    (to((*n, 0)).unwrap(), o)
                })
                .collect();
            let mut held = Object::from(trailer.clone());
            held.renumber(&to);
            let held = held.as_dict().unwrap().clone();
            let renumbered = encryptor.map(|e| e.renumbered(&to));
            let mut options = self::options(kind, pack);
            options.encrypt_at = None;
            let mut want = Vec::new();
            let given = moved.iter().map(|(id, o)| (*id, write::Body::from(o)));
            write::full(
                &mut want,
                given,
                &held,
                renumbered.as_deref(),
                None,
                &options,
            )
            .unwrap();
            assert_eq!(out, want, "{name}");
        }
    }
}

/// [`Pack::groups`](write::Pack::groups) is given the numbers the packed
/// objects are given at, under a renumbering as without, and a group that
/// names a number they're written at instead is refused by it.
#[test]
fn groups_are_given_the_numbers_objects_are_given_at() {
    let (plain, trailer) = with_many(20);
    let ids: Vec<ObjectId> = plain.iter().map(|(n, _)| (*n, 0)).collect();
    let map = shifted(&ids, 100);
    let to = by(&map);
    let seen = std::sync::Mutex::new(Vec::new());
    let groups = |numbers: &[u32]| {
        seen.lock().unwrap().push(numbers.to_vec());
        vec![numbers.to_vec()]
    };
    let mut pack = write::Pack::DEFAULT;
    pack.groups = Some(&groups);
    let mut options = options(XrefKind::Stream, Some(pack));
    options.renumber = Some(&to);
    let mut out = Vec::new();
    write::full(&mut out, bodies(&plain), &trailer, None, None, &options).unwrap();
    let packed: Vec<u32> = (plain.iter())
        .filter(|(n, o)| write::packable((*n, 0), o))
        .map(|(n, _)| *n)
        .collect();
    assert_eq!(*seen.lock().unwrap(), std::slice::from_ref(&packed));
    let back = open(&out);
    assert!(sound(&back), "{:?}", back.warnings());
    assert_moved(
        &open(&whole(&plain, &trailer, XrefKind::Table, None)),
        &back,
        &ids,
        &to,
        "",
    );
    let written = |numbers: &[u32]| vec![vec![numbers[0] + 100]];
    let mut pack = write::Pack::DEFAULT;
    pack.groups = Some(&written);
    options.pack = Some(pack);
    let mut out = Vec::new();
    let refused = write::full(&mut out, bodies(&plain), &trailer, None, None, &options);
    assert_eq!(refused, Err(Error::Group(packed[0] + 100)));
}

/// After a whole file written under a renumbering, what it wrote says so,
/// its trailer and its encryption dictionary in the numbers written; an
/// update after it given a document's objects as read, or a reading, is
/// refused, nothing written, as each would be written over another object
/// (in a plain file, where nothing else would refuse it); given an object
/// built at a number written, plain and under each method, by the
/// encryptor in the numbers written, it's written, and read back; the
/// encryptor as read is refused there, its dictionary elsewhere.
#[test]
fn an_update_after_a_renumbered_whole_file_is_given_its_numbers() {
    let file = spelled();
    let reading = open(&file);
    let doc = reading.to_document().unwrap();
    let ids: Vec<ObjectId> = doc.objects().map(|(id, _)| id).collect();
    let map = reversed(&ids);
    let to = by(&map);
    let mut options = write::Options::DEFAULT;
    options.renumber = Some(&to);
    let mut out = Vec::new();
    let written = write::document(&mut out, &doc, None, Some(&reading), &options).unwrap();
    let base = &written.base;
    assert!(base.renumbered);
    assert_eq!(
        base.trailer.get(b"Root").and_then(Object::as_reference),
        Some(map[&(1, 0)])
    );
    let plain = write::Options::DEFAULT;
    for from in [Some(&reading as &dyn write::ReadFrom), None] {
        let mut updated = out.clone();
        let refused = write::update(&mut updated, base, write::bodies(&doc), None, from, &plain);
        assert_eq!(refused, Err(Error::Renumbered));
        assert_eq!(updated, out);
    }
    let built = Object::from(dict! { "Built" => true });
    let mut updated = out.clone();
    let given = [((3, 0), write::Body::from(&built))];
    let refused = write::update(
        &mut updated,
        base,
        given.clone(),
        None,
        Some(&reading),
        &plain,
    );
    assert_eq!(refused, Err(Error::Renumbered));
    assert_eq!(updated, out);
    let second = write::update(&mut updated, base, given, None, None, &plain).unwrap();
    assert!(second.unwrap().base.renumbered);
    let back = open(&updated);
    assert!(sound(&back), "{:?}", back.warnings());
    assert_eq!(value(&back, (3, 0)), built);
    assert_moved(&reading, &back, &[(1, 0), (2, 0)], &to, "updated");
    assert_eq!(
        Error::Renumbered.to_string(),
        "the file was written under a renumbering, so an update after it is given objects at its \
         numbers, built or edited, and no reading"
    );

    // Encrypted: the encryption dictionary moved with the rest.
    let (plain_objects, trailer) = plaintext();
    let methods = methods();
    for method in standard(&methods) {
        let name = method.name;
        let source = encrypted_by(&method.encryptor, &plain_objects, &trailer, XrefKind::Table);
        let reading = opened(&source, "user");
        let doc = reading.to_document().unwrap();
        let own = Encryptor::of(&reading).unwrap().unwrap();
        let ids: Vec<ObjectId> = doc.objects().map(|(id, _)| id).collect();
        let map = reversed(&ids);
        let to = by(&map);
        let mut options = write::Options::DEFAULT;
        options.renumber = Some(&to);
        let mut out = Vec::new();
        let written =
            write::document(&mut out, &doc, Some(&own), Some(&reading), &options).unwrap();
        let renumbered = own.renumbered(&to);
        assert_eq!(renumbered.at(), Some(map[&(ENCRYPT, 0)]), "{name}");
        assert_eq!(
            written.base.encryption.as_ref().map(as_written),
            Some(as_written(renumbered.dictionary())),
            "{name}"
        );
        let mut info = handed(&reading, (13, 0));
        info.as_dict_mut()
            .unwrap()
            .set("Title", lit(b"a title updated"));
        let at = map[&(13, 0)];
        let given = [(at, write::Body::from(&info))];
        let mut refused = out.clone();
        let wrong = write::update(
            &mut refused,
            &written.base,
            given.clone(),
            Some(&own),
            None,
            &plain,
        );
        assert_eq!(wrong, Err(Error::Unmatched), "{name}");
        assert_eq!(refused, out, "{name}");
        let mut updated = out.clone();
        write::update(
            &mut updated,
            &written.base,
            given,
            Some(&renumbered),
            None,
            &plain,
        )
        .unwrap();
        assert!(!in_clear(&updated, b"a title updated"), "{name}");
        let back = opened(&updated, "user");
        assert!(sound(&back), "{name}: {:?}", back.warnings());
        assert_eq!(value(&back, at), info, "{name}");
        assert_moved(&reading, &back, &[(4, 0), (5, 0)], &to, name);
    }
}

/// [`listed`] under each method, its encryption dictionary at [`ENCRYPT`],
/// written whole by its document and reading under its own encryption, a
/// table and, where it packs, a stream packed, under each renumbering:
/// reversed, shifted, dense (passing over the numbers it lists free), and
/// the encryption dictionary's number swapped with the Info's, so that a
/// string is written where the dictionary was, the rest copied. Read back
/// by each password that opens the source, every object is the source's
/// at its number; what the list encrypts isn't in the clear, and what it
/// leaves in the clear is, the document's metadata moved among them under
/// `/EncryptMetadata false`, and its dictionary's string not.
#[test]
fn each_method_writes_whole_renumbered_with_nothing_in_the_clear() {
    let (plain, trailer) = listed();
    for method in methods() {
        let source = encrypted_by(&method.encryptor, &plain, &trailer, XrefKind::Table);
        let reading = opened(&source, "user");
        let own = Encryptor::of(&reading).unwrap().unwrap();
        let doc = reading.to_document().unwrap();
        let ids: Vec<ObjectId> = doc.objects().map(|(id, _)| id).collect();
        let kept = written_of(&doc);
        let free: std::collections::BTreeSet<u32> = own.free_numbers().map(|(n, _)| n).collect();
        let mut swapped: Map = ids.iter().map(|&id| (id, id)).collect();
        swapped.insert((ENCRYPT, 0), (13, 0));
        swapped.insert((13, 0), (ENCRYPT, 0));
        let maps = [
            ("reversed", reversed(&ids)),
            ("shifted", shifted(&ids, ENCRYPT + 1)),
            ("dense", dense(&kept, |_| None, &free)),
            ("swapped", swapped),
        ];
        let passwords: Vec<&str> = ["user", "owner"]
            .into_iter()
            .filter(|p| matches!(opened(&source, p).encryption(), Encryption::Opened { .. }))
            .collect();
        for (way, map) in &maps {
            let to = by(map);
            let mut ways = vec![(XrefKind::Table, None)];
            if own.packs() {
                ways.push((XrefKind::Stream, Some(write::Pack::DEFAULT)));
            }
            for (kind, pack) in ways {
                let name = format!("{}, {way}, {kind:?}", method.name);
                let mut options = options(kind, pack);
                options.renumber = Some(&to);
                let mut out = Vec::new();
                write::document(&mut out, &doc, Some(&own), Some(&reading), &options).unwrap();
                for (text, encrypted) in listed_rows(&method) {
                    let said = String::from_utf8_lossy(text);
                    assert_eq!(in_clear(&out, text), !encrypted, "{name}: {said:?}");
                }
                for password in &passwords {
                    let back = opened(&out, password);
                    let name = format!("{name}, by {password}");
                    assert!(sound(&back), "{name}: {:?}", back.warnings());
                    assert_moved(&reading, &back, &kept, &to, &name);
                }
            }
        }
    }
}

/// [`plaintext`] as the test writer encrypts it, under each method it
/// encrypts by, the document's metadata encrypted and not, written whole
/// by its document and reading reversed, a table and a stream packed: read
/// back by the user's and the owner's password, the same objects, renumbered,
/// nothing in the clear but the metadata where it's left so (from `/V` 4).
#[test]
fn written_otherwise_files_write_whole_renumbered() {
    for kind in KINDS_ENCRYPTED {
        for encrypt_metadata in [true, false] {
            let source = written_otherwise(kind, encrypt_metadata);
            let reading = opened(&source, "user");
            let own = Encryptor::of(&reading).unwrap().unwrap();
            let doc = reading.to_document().unwrap();
            let ids: Vec<ObjectId> = doc.objects().map(|(id, _)| id).collect();
            let kept = written_of(&doc);
            let map = reversed(&ids);
            let to = by(&map);
            for (xref, pack) in [
                (XrefKind::Table, None),
                (XrefKind::Stream, Some(write::Pack::DEFAULT)),
            ] {
                let name = format!("{kind:?}, metadata encrypted: {encrypt_metadata}, {xref:?}");
                let mut options = options(xref, pack);
                options.renumber = Some(&to);
                let mut out = Vec::new();
                write::document(&mut out, &doc, Some(&own), Some(&reading), &options).unwrap();
                // `/EncryptMetadata` is `/V` 4's and later's.
                let filters = !matches!(kind, Kind::Rc4By40 | Kind::Rc4By128);
                for text in SECRETS {
                    let clear = text == XMP && !encrypt_metadata && filters;
                    let said = String::from_utf8_lossy(text);
                    assert_eq!(in_clear(&out, text), clear, "{name}: {said:?}");
                }
                for password in ["user", "owner"] {
                    let back = opened(&out, password);
                    assert!(sound(&back), "{name}: {:?}", back.warnings());
                    assert_moved(&reading, &back, &kept, &to, &format!("{name}, {password}"));
                }
            }
        }
    }
}

/// What the encryption dictionary refers to, read before a reader has a
/// key, stays in the clear at the number it's written at, and what's
/// written at the number it had is encrypted, as any object: here its
/// `/CF`, by reference, with a note, moved, and the Info, with its title,
/// put at its number; under AES-128 and AES-256, with the dictionary at a
/// number and direct in the trailer, where it stays, its `/CF` renumbered;
/// with a table, and packed, the `/CF` on its own, out of object streams.
#[test]
fn an_encryption_dictionary_s_referent_moves_in_the_clear() {
    const CF_NOTE: &[u8] = b"a crypt filter's note";
    let (mut plain, trailer) = plaintext();
    for (kind, direct) in [
        (Kind::Aes128, false),
        (Kind::Aes256, false),
        (Kind::Aes128, true),
        (Kind::Aes256, true),
    ] {
        let name = format!("{kind:?}, direct: {direct}");
        let mut dict = dictionary_of(kind, true);
        let mut filters = dict.remove(b"CF").unwrap();
        filters.as_dict_mut().unwrap().set("Note", lit(CF_NOTE));
        dict.set("CF", Object::reference(41));
        // A source of that dictionary, with its /CF as object 41.
        let id = Object::hex(ID);
        let mut source_trailer =
            dict! { "Root" => Object::reference(1), "ID" => vec![id.clone(), id] };
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut section = Section::full(0);
        let mut objects = vec![(1, catalog()), (41, filters.clone())];
        match direct {
            true => source_trailer.set("Encrypt", dict.clone()),
            false => {
                objects.push((ENCRYPT, Object::from(dict.clone())));
                source_trailer.set("Encrypt", Object::reference(ENCRYPT));
            }
        }
        for (n, o) in &objects {
            let pos = serialize::indirect(&mut out, (*n, 0), o).unwrap();
            section.insert(
                *n,
                Entry::InUse {
                    offset: pos,
                    generation: 0,
                },
            );
        }
        let pos = out.len();
        section.table(&mut out, source_trailer, pos).unwrap();
        let encryptor = encryptor_of(&out);
        assert!(encryptor.dictionary_object((41, 0)), "{name}");
        plain.retain(|(n, _)| *n != 41);
        plain.push((41, filters.clone()));
        let source = encrypted_by(&encryptor, &plain, &trailer, XrefKind::Table);
        let reading = opened(&source, "user");
        let own = Encryptor::of(&reading).unwrap().unwrap();
        assert_eq!(own.direct(), direct, "{name}");
        let doc = reading.to_document().unwrap();
        let ids: Vec<ObjectId> = doc.objects().map(|(id, _)| id).collect();
        let mut map: Map = ids.iter().map(|&id| (id, id)).collect();
        map.insert((41, 0), (50, 0));
        map.insert((13, 0), (41, 0));
        let to = by(&map);
        for (kind, pack) in [
            (XrefKind::Table, None),
            (XrefKind::Stream, Some(write::Pack::DEFAULT)),
        ] {
            let name = format!("{name}, {kind:?}");
            let mut options = options(kind, pack);
            options.renumber = Some(&to);
            let mut out = Vec::new();
            write::document(&mut out, &doc, Some(&own), Some(&reading), &options).unwrap();
            assert!(!in_clear(&out, TITLE), "{name}: the Info at 41");
            assert!(holds(&out, CF_NOTE), "{name}: /CF at 50, on its own");
            let back = opened(&out, "user");
            assert!(sound(&back), "{name}: {:?}", back.warnings());
            assert_moved(&reading, &back, &written_of(&doc), &to, &name);
            let encrypt = back.trailer().dict().get(b"Encrypt").unwrap();
            let stored = match direct {
                true => encrypt.as_dict().unwrap().clone(),
                false => {
                    let at = encrypt.as_reference().unwrap();
                    back.get(at).unwrap().as_dict().unwrap().clone()
                }
            };
            let cf = stored.get(b"CF").and_then(Object::as_reference);
            assert_eq!(cf, Some((50, 0)), "{name}");
            let direct_back = Encryptor::of(&back).unwrap().unwrap().direct();
            assert_eq!(direct_back, direct, "{name}");
        }
    }
}

/// A renumbering onto a number the file read lists free is refused under
/// RC4 and AES-128, whose keys are made of the number, as the key would be
/// a deleted object's; under AES-256, it's written.
#[test]
fn a_renumbering_onto_a_free_number_is_refused_where_keys_are_per_object() {
    let objects = [
        (1, Body::from(dict! { "Type" => "Catalog" })),
        (3, Body::from(dict! { "N" => 3 })),
    ];
    let root = dict! { "Root" => (1, 0) };
    for (kind, refused) in [
        (Kind::Rc4By40, true),
        (Kind::Rc4By128, true),
        (Kind::Aes128, true),
        (Kind::Aes256R5, false),
        (Kind::Aes256, false),
    ] {
        let name = format!("{kind:?}");
        let file = fixture::encrypted(&settings(kind, true), &objects, root.clone());
        let reading = opened(&file, "user");
        let encryptor = Encryptor::of(&reading).unwrap();
        let doc = reading.to_document().unwrap();
        let map = Map::from([((1, 0), (1, 0)), ((3, 0), (2, 0))]);
        let to = by(&map);
        let mut options = write::Options::DEFAULT;
        options.renumber = Some(&to);
        let mut out = b"held".to_vec();
        let written = write::document(&mut out, &doc, encryptor.as_ref(), Some(&reading), &options);
        match refused {
            true => {
                assert_eq!(written, Err(Error::Free((2, 0))), "{name}");
                assert_eq!(out, b"held", "{name}");
            }
            false => {
                written.unwrap();
                let back = opened(&out[4..], "user");
                assert!(sound(&back), "{name}: {:?}", back.warnings());
                assert_eq!(value(&back, (2, 0)), value(&reading, (3, 0)), "{name}");
            }
        }
    }
}

/// Under RC4, an object written whole at a number another object had in the
/// file read is encrypted with that object's keystream, as
/// `Options::renumber`'s doc and pdf-crypt's `Method::Rc4` say: here two
/// titles of one length swapped, the XOR of the ciphertexts at one number,
/// the file read and the file written, the XOR of the two titles; and so
/// is one an update writes so, the file and its update. Under AES-128, it
/// isn't.
#[test]
fn an_rc4_renumbering_leaks_the_xor_of_what_moved() {
    const ONE: &[u8] = b"the first title";
    const TWO: &[u8] = b"a second title!";
    assert_eq!(ONE.len(), TWO.len());
    let objects = [
        (1, Body::from(dict! { "Type" => "Catalog" })),
        (2, Body::from(dict! { "Title" => lit(ONE) })),
        (3, Body::from(dict! { "Title" => lit(TWO) })),
    ];
    let root = dict! { "Root" => (1, 0) };
    let xor = |a: &[u8], b: &[u8]| a.iter().zip(b).map(|(a, b)| a ^ b).collect::<Vec<u8>>();
    let title = |file: &[u8], n: u32| {
        let locked = open(file);
        let dict = locked.get((n, 0)).unwrap().as_dict().unwrap().clone();
        match dict.get(b"Title").unwrap().value() {
            Value::String(Str::Encrypted(bytes), _) => bytes.clone(),
            other => panic!("{other:?}"),
        }
    };
    for kind in [Kind::Rc4By128, Kind::Aes128] {
        let file = fixture::encrypted(&settings(kind, true), &objects, root.clone());
        let reading = opened(&file, "user");
        let encryptor = Encryptor::of(&reading).unwrap();
        let doc = reading.to_document().unwrap();
        let map = Map::from([((1, 0), (1, 0)), ((2, 0), (3, 0)), ((3, 0), (2, 0))]);
        let to = by(&map);
        let mut options = write::Options::DEFAULT;
        options.renumber = Some(&to);
        let mut out = Vec::new();
        write::document(&mut out, &doc, encryptor.as_ref(), Some(&reading), &options).unwrap();
        let back = opened(&out, "user");
        let got = back
            .get((2, 0))
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Title")
            .cloned();
        assert_eq!(
            got.and_then(|t| t.as_bytes().map(<[u8]>::to_vec)),
            Some(TWO.to_vec())
        );
        let leaks = xor(&title(&file, 2), &title(&out, 2)) == xor(ONE, TWO);
        assert_eq!(leaks, kind == Kind::Rc4By128, "{kind:?}");
        // The same, the encryption dictionary where it is.
        let base = write::Base::of(&reading).unwrap();
        let swapped = moving(&reading, &[(2, 3), (3, 2)]);
        let (updated, _) = updated_under(
            &file,
            &base,
            write::bodies(&doc),
            encryptor.as_ref(),
            Some(&reading),
            &swapped,
        );
        let back = opened(&updated, "user");
        assert_eq!(value(&back, (2, 0)), value(&reading, (3, 0)), "{kind:?}");
        let leaks = xor(&title(&file, 2), &title(&updated, 2)) == xor(ONE, TWO);
        assert_eq!(leaks, kind == Kind::Rc4By128, "{kind:?}, updated");
    }
}

// Copies under a renumbering: an object as read written as the bytes it was
// read from, its header and the references that move written anew in place
// (rule 9); at another number only where its key isn't made of its number
// (rule 2).

/// What a copy of `object`, read as `record` in `reading`, written at `at`
/// under the renumbering `to` is: the bytes it was read from, but its header,
/// `n g obj`, of the number written where that's another, and each reference
/// `to` names otherwise, `n g R` or `null`, in place of the bytes its place
/// names. Made here as the writer doesn't make it, a reference's place found
/// by recursing.
fn copy_of<S: ByteSource>(
    reading: &Reading<S>,
    record: &Record,
    object: &Object,
    at: ObjectId,
    to: &dyn Fn(ObjectId) -> Option<ObjectId>,
) -> Vec<u8> {
    fn placed(o: &Object, out: &mut Vec<(ObjectId, Span)>) {
        match o.value() {
            Value::Ref(id) => out.push((*id, o.at.unwrap())),
            Value::Array(array) => array.iter().for_each(|o| placed(o, out)),
            Value::Dict(dict) => dict.entries().iter().for_each(|e| placed(&e.value, out)),
            Value::Stream(stream) => {
                (stream.dict.entries().iter()).for_each(|e| placed(&e.value, out))
            }
            _ => {}
        }
    }
    let bytes = reading.written(record.at).unwrap();
    let mut runs: Vec<(usize, usize, Vec<u8>)> = Vec::new();
    if record.origin == crate::Origin::File && at != record.id {
        let header = bytes.windows(3).position(|w| w == b"obj").unwrap() + 3;
        runs.push((0, header, format!("{} {} obj", at.0, at.1).into_bytes()));
    }
    let mut references = Vec::new();
    placed(object, &mut references);
    for (id, span) in references {
        let named = to(id);
        if named != Some(id) {
            let text = match named {
                Some((n, g)) => format!("{n} {g} R"),
                None => "null".to_string(),
            };
            let run = (span.start - record.at.start, span.end - record.at.start);
            runs.push((run.0, run.1, text.into_bytes()));
        }
    }
    let mut out = Vec::new();
    let mut from = 0;
    for (start, end, text) in runs {
        out.extend_from_slice(&bytes[from..start]);
        out.extend_from_slice(&text);
        from = end;
    }
    out.extend_from_slice(&bytes[from..]);
    out
}

/// Members naming others, spelled as another writer spells them: a
/// reference in an array, one with a comment between its tokens, and one to
/// object 9, which nothing holds.
const MEMBERS_NAMING: [(u32, &[u8]); 3] = [
    (4, b"<48656C6C6F>"),
    (5, b"[ 1   2 3 0 R ]"),
    (7, b"<</UserUnit 1.50 /Up 2 % between\n0 R /Gone 9 0 R>>"),
];

/// Under a renumbering, a member's bytes go into an object stream of the
/// write's as they were read but each reference the renumbering names
/// otherwise, `null` where it names none, and the comment between a changed
/// reference's tokens gone, at the number written, under any encryption: a
/// plain file's written plain and under each method that packs, and that
/// one's, read by its password, renumbered again under its own encryption,
/// decrypted and under a new encryption. Each read back, the objects
/// renumbered, nothing in the clear where it's encrypted.
#[test]
fn members_are_copied_renumbered_into_object_streams_under_any_encryption() {
    use chacha20::ChaCha20Rng;

    let file = members_spelled_as(&MEMBERS_NAMING);
    let reading = open(&file);
    let doc = reading.to_document().unwrap();
    let ids: Vec<ObjectId> = doc.objects().map(|(id, _)| id).collect();
    let kept = written_of(&doc);
    let map = shifted(&ids, 100);
    let to = by(&map);
    let prepared = Password("new").prepared().unwrap();
    let mut rng = ChaCha20Rng::seed_from_u64(15);
    let created = Encryptor::create(&prepared, true, &mut rng).unwrap();
    let mut pack = options(XrefKind::Stream, Some(write::Pack::DEFAULT));
    pack.renumber = Some(&to);
    // Its members' bytes, renumbered `by` on, in the object streams written.
    let members = |by: u32| -> [Vec<u8>; 3] {
        [
            b"<48656C6C6F>".to_vec(),
            format!("[ 1   2 {} 0 R ]", 3 + by).into_bytes(),
            format!("<</UserUnit 1.50 /Up {} 0 R /Gone null>>", 2 + by).into_bytes(),
        ]
    };
    let assert_back = |out: &[u8], password: &str, by: u32, name: &str| {
        let back = opened(out, password);
        assert!(sound(&back), "{name}: {:?}", back.warnings());
        let data = object_streams_data(&back);
        for bytes in members(by) {
            assert!(
                holds(&data, &bytes),
                "{name}: {:?}",
                String::from_utf8_lossy(&bytes)
            );
        }
        back
    };
    let mut out = Vec::new();
    write::document(&mut out, &doc, None, Some(&reading), &pack).unwrap();
    let back = assert_back(&out, "", 100, "plain");
    assert_moved(&reading, &back, &kept, &to, "plain");
    let methods = methods();
    for method in methods.iter().filter(|m| m.encryptor.packs() && m.streams) {
        let name = method.name;
        let mut encrypted = Vec::new();
        let encryptor = Some(&method.encryptor);
        write::document(&mut encrypted, &doc, encryptor, Some(&reading), &pack).unwrap();
        for bytes in members(100) {
            assert!(!in_clear(&encrypted, &bytes), "{name}");
        }
        let back = assert_back(&encrypted, "user", 100, name);
        assert_moved(&reading, &back, &kept, &to, name);
        // Read by its password, and renumbered again, as it was read.
        let again = opened(&encrypted, "user");
        let doc = again.to_document().unwrap();
        let ids: Vec<ObjectId> = doc.objects().map(|(id, _)| id).collect();
        let kept = written_of(&doc);
        let map = shifted(&ids, 100);
        let to = by(&map);
        let mut pack = pack;
        pack.renumber = Some(&to);
        let own = Encryptor::of(&again).unwrap().unwrap();
        for (way, encryptor, password) in [
            ("its own", Some(&own), "user"),
            ("decrypted", None, ""),
            ("created", Some(&created), "new"),
        ] {
            let name = format!("{name}, {way}");
            let mut out = Vec::new();
            write::document(&mut out, &doc, encryptor, Some(&again), &pack).unwrap();
            for bytes in members(200) {
                assert_eq!(in_clear(&out, &bytes), encryptor.is_none(), "{name}");
            }
            let back = assert_back(&out, password, 200, &name);
            assert_moved(&again, &back, &kept, &to, &name);
        }
    }
}

/// A stream whose data, and a string in its dictionary, read as a
/// reference, which its dictionary's `/Ref` is.
const DATA_NAMING: (u32, &[u8]) = (
    8,
    b"<</Length 5 /Big (15 0 R) /Ref 5 0 R>>\nstream\n5 0 R\nendstream",
);

/// Rule 9: under a renumbering, an object as read whose references aren't
/// where its bytes have them refuses the write, nothing written, as it
/// isn't what its bytes say: a reference with no place, one the
/// renumbering moves and one it doesn't, and one put in (by
/// `as_written_mut`, which keeps the object's own); two whose places are
/// swapped, so out of the order walked; one placed past the object's bytes;
/// one whose place reads as another reference; one placed on bytes that
/// read as it inside a string's token, and inside a stream's data; and a
/// member's with no place. Without a renumbering, nothing of it is asked:
/// copied as the bytes it was read from.
#[test]
fn a_copy_whose_references_aren_t_where_its_bytes_have_them_is_refused() {
    let mut objects = SPELLED.to_vec();
    objects.extend([NAMING, DATA_NAMING]);
    let file = spelled_as(&objects);
    let reading = open(&file);
    let doc = reading.to_document().unwrap();
    let ids: Vec<ObjectId> = doc.objects().map(|(id, _)| id).collect();
    // 5, 6 and 7 move; 9 is no object's.
    let mut map: Map = ids.iter().map(|&id| (id, id)).collect();
    map.extend([5, 6, 7].map(|n| ((n, 0), (n + 10, 0))));
    let to = by(&map);
    let mut renumbered = options(XrefKind::Table, None);
    renumbered.renumber = Some(&to);
    let plain = options(XrefKind::Table, None);
    // Object `n` as read, but as `edit` changes its dictionary's places.
    let edited = |n: u32, edit: &dyn Fn(&mut Dict)| {
        let mut object = doc.get((n, 0)).unwrap().clone();
        match object.as_written_mut() {
            Value::Dict(dict) => edit(dict),
            Value::Stream(stream) => edit(&mut stream.dict),
            other => panic!("{other:?}"),
        }
        assert!(object.at.is_some());
        object
    };
    let span_of = |n: u32, key: &[u8]| -> Span {
        let object = doc.get((n, 0)).unwrap();
        let value = object.as_dict().unwrap().get(key).unwrap();
        match value.value() {
            Value::Array(array) => array[0].at.unwrap(),
            _ => value.at.unwrap(),
        }
    };
    let past = doc.record((3, 0)).unwrap().at.end;
    let data = doc.record((8, 0)).unwrap().data.unwrap().at;
    let string = (file.windows(8).position(|w| w == b"(15 0 R)")).unwrap() + 2;
    let inside = Span::new(None, string, string + 5);
    let cases: Vec<(&str, u32, Object)> = vec![
        (
            "a moved one with no place",
            3,
            edited(3, &|d| d.get_mut(b"Contents").unwrap().at = None),
        ),
        (
            "one kept with no place",
            3,
            edited(3, &|d| d.get_mut(b"Parent").unwrap().at = None),
        ),
        (
            "one put in",
            1,
            edited(1, &|d| d.push("Added", Object::reference(3))),
        ),
        (
            "two swapped",
            7,
            edited(7, &|d| {
                let (b, dd) = (span_of(7, b"B"), span_of(7, b"D"));
                match d.get_mut(b"B").unwrap().as_written_mut() {
                    Value::Array(array) => array[0].at = Some(dd),
                    other => panic!("{other:?}"),
                }
                d.get_mut(b"D").unwrap().at = Some(b);
            }),
        ),
        (
            "one past its bytes",
            3,
            edited(3, &|d| {
                let at = Span::new(None, past + 4, past + 9);
                d.get_mut(b"Contents").unwrap().at = Some(at);
            }),
        ),
        (
            "one that reads as another",
            3,
            edited(3, &|d| {
                *d.get_mut(b"Contents").unwrap().as_written_mut() = Value::Ref((7, 0))
            }),
        ),
        (
            "one inside a string",
            8,
            edited(8, &|d| d.get_mut(b"Ref").unwrap().at = Some(inside)),
        ),
        (
            "one in its stream's data",
            8,
            edited(8, &|d| d.get_mut(b"Ref").unwrap().at = Some(data)),
        ),
    ];
    for (name, n, object) in &cases {
        let given = |object: &Object| -> Vec<(ObjectId, write::Body<'_>)> {
            let record = Cow::Borrowed(doc.record((*n, 0)).unwrap());
            (write::bodies(&doc))
                .map(|(id, body)| match id == (*n, 0) {
                    true => (
                        id,
                        write::Body::Read {
                            object: Cow::Owned(object.clone()),
                            record: record.clone(),
                        },
                    ),
                    false => (id, body),
                })
                .collect()
        };
        let trailer = doc.trailer.dict();
        let mut out = Vec::new();
        let refused = write::full(
            &mut out,
            given(object),
            trailer,
            None,
            Some(&reading),
            &renumbered,
        );
        assert_eq!(refused, Err(Error::Elsewhere((*n, 0))), "{name}");
        assert!(out.is_empty(), "{name}");
        // Without a renumbering, copied as it was read.
        let written = write::full(
            &mut out,
            given(object),
            trailer,
            None,
            Some(&reading),
            &plain,
        )
        .unwrap();
        let record = doc.record((*n, 0)).unwrap();
        assert!(copied(&out, &written, record, &reading, (*n, 0)), "{name}");
    }

    // A member's, with no place.
    let file = members_spelled_as(&MEMBERS_NAMING);
    let reading = open(&file);
    let doc = reading.to_document().unwrap();
    let ids: Vec<ObjectId> = doc.objects().map(|(id, _)| id).collect();
    let map = shifted(&ids, 100);
    let to = by(&map);
    let mut pack = options(XrefKind::Stream, Some(write::Pack::DEFAULT));
    pack.renumber = Some(&to);
    let mut member = doc.get((5, 0)).unwrap().clone();
    match member.as_written_mut() {
        Value::Array(array) => array[2].at = None,
        other => panic!("{other:?}"),
    }
    let given = (write::bodies(&doc)).map(|(id, body)| match id {
        (5, 0) => (
            id,
            write::Body::Read {
                object: Cow::Borrowed(&member),
                record: Cow::Borrowed(doc.record(id).unwrap()),
            },
        ),
        _ => (id, body),
    });
    let mut out = Vec::new();
    let trailer = doc.trailer.dict();
    let refused = write::full(&mut out, given, trailer, None, Some(&reading), &pack);
    assert_eq!(refused, Err(Error::Elsewhere((5, 0))));
    assert!(out.is_empty());
}

/// Rule 2 under a renumbering, for each method the test writer encrypts by,
/// the document's metadata encrypted and not: [`plaintext`] as it spells it
/// and draws its IVs, written whole by its document and reading under its
/// own encryption, a table and a stream packed, under three renumberings:
/// every object shifted, every other number kept, and the encryption
/// dictionary's number swapped with the Info's. An object at an offset is
/// copied at a new number where no key is made of its number (AES-256, its
/// ciphertext the source's), and else written anew, as its key would be
/// another's; at its own number it's copied, its references spliced. Read
/// back by the user's and the owner's password, every object the source's
/// at its number, nothing in the clear but the metadata where it's left so.
#[test]
fn copies_go_to_new_numbers_where_keys_aren_t_made_of_them() {
    let mut moved = 0;
    for kind in KINDS_ENCRYPTED {
        let per_object = !matches!(kind, Kind::Aes256R5 | Kind::Aes256);
        for encrypt_metadata in [true, false] {
            let source = written_otherwise(kind, encrypt_metadata);
            let reading = opened(&source, "user");
            let own = Encryptor::of(&reading).unwrap().unwrap();
            assert_eq!(own.per_object(), per_object, "{kind:?}");
            let doc = reading.to_document().unwrap();
            let ids: Vec<ObjectId> = doc.objects().map(|(id, _)| id).collect();
            let kept = written_of(&doc);
            let top = ids.iter().map(|id| id.0).max().unwrap() + 1;
            let halved: Map = (ids.iter())
                .map(|&(n, g)| ((n, g), if n % 2 == 0 { (n, g) } else { (n + top, g) }))
                .collect();
            let encrypt = (doc.trailer.dict().get(b"Encrypt"))
                .and_then(Object::as_reference)
                .unwrap();
            let mut swapped: Map = ids.iter().map(|&id| (id, id)).collect();
            swapped.insert(encrypt, (13, 0));
            swapped.insert((13, 0), encrypt);
            let maps = [
                ("shifted", shifted(&ids, top)),
                ("halved", halved),
                ("swapped", swapped),
            ];
            for (way, map) in &maps {
                let to = by(map);
                for (xref, pack) in [
                    (XrefKind::Table, None),
                    (XrefKind::Stream, Some(write::Pack::DEFAULT)),
                ] {
                    let name = format!(
                        "{kind:?}, metadata encrypted: {encrypt_metadata}, {way}, {xref:?}"
                    );
                    let mut options = options(xref, pack);
                    options.renumber = Some(&to);
                    let mut out = Vec::new();
                    let written =
                        write::document(&mut out, &doc, Some(&own), Some(&reading), &options)
                            .unwrap();
                    for (id, body) in write::bodies(&doc).filter(|(id, _)| kept.contains(id)) {
                        let write::Body::Read { object, record } = &body else {
                            continue;
                        };
                        let at = to(id).unwrap();
                        let Some(Entry::InUse { offset, .. }) = written.section.get(at.0) else {
                            continue;
                        };
                        let copy = copy_of(&reading, record, object, at, &to);
                        let copied = out[offset..].starts_with(&copy);
                        assert_eq!(copied, at == id || !per_object, "{name}: {id:?} at {at:?}");
                        moved += usize::from(copied && at != id);
                    }
                    let filters = !matches!(kind, Kind::Rc4By40 | Kind::Rc4By128);
                    for text in SECRETS {
                        let clear = text == XMP && !encrypt_metadata && filters;
                        let said = String::from_utf8_lossy(text);
                        assert_eq!(in_clear(&out, text), clear, "{name}: {said:?}");
                    }
                    for password in ["user", "owner"] {
                        let back = opened(&out, password);
                        assert!(sound(&back), "{name}: {:?}", back.warnings());
                        assert_moved(&reading, &back, &kept, &to, &format!("{name}, {password}"));
                    }
                }
            }
        }
    }
    assert!(moved > 0, "AES-256's copied at new numbers");
}

// Updates under a renumbering (`write::Options::renumber` given to
// `write::update`): what it's given in its file's numbers; what moves, or
// names what does, written at its number, copied where the rules hold;
// what doesn't, passed over; the carried trailer renumbered; the
// encryption dictionary and what it refers to where the file has them.

/// Every number of `pdf`'s at its own, but those `moves` gives another:
/// a renumbering of an update that names each object of its file, so that
/// no reference is written null.
fn moving(pdf: &Reading<Arc<[u8]>>, moves: &[(u32, u32)]) -> Map {
    let mut map: Map = pdf.ids().map(|id| (id, id)).collect();
    for &(from, to) in moves {
        map.insert((from, 0), (to, 0));
    }
    map
}

/// `bodies` given to an update after `base` under `map`, from `from`: the
/// file and the update after it, and what it wrote.
fn updated_under<'a>(
    file: &[u8],
    base: &write::Base,
    bodies: impl IntoIterator<Item = (ObjectId, write::Body<'a>)>,
    encryptor: Option<&Encryptor>,
    from: Option<&dyn write::ReadFrom>,
    map: &Map,
) -> (Vec<u8>, write::Written) {
    let to = by(map);
    let mut options = write::Options::DEFAULT;
    options.renumber = Some(&to);
    let mut out = file.to_vec();
    let written = write::update(&mut out, base, bodies, encryptor, from, &options)
        .unwrap()
        .unwrap();
    (out, written)
}

/// The numbers an update lists, ascending, but its cross-reference
/// stream's, the last, where it's one.
fn listed_numbers(written: &write::Written) -> Vec<u32> {
    let mut listed: Vec<u32> = (1..written.size)
        .filter(|&n| written.section.lists(n))
        .collect();
    if written.base.kind == XrefKind::Stream {
        listed.pop();
    }
    listed
}

/// [`SPELLED`] and [`NAMING`] as a file with a table, its trailer naming
/// the hex string too (`/Custom`), and as its document written whole with a
/// stream, each updated under a renumbering that moves the catalog, the
/// page and the hex string past every number, given every object of its
/// document as read and its reading. Written, at the numbers
/// the renumbering gives them: what moves, and what names what does (the
/// pages, naming the page; the seventh, naming the pages, the page and the
/// string, and object 9, which nothing holds, null; the stream file's
/// cross-reference stream, an object of its document whose dictionary
/// names the catalog); each copied, its header and the references that
/// move written anew in place. Passed over: the array and the stream, which
/// neither move nor name what does. Read back with two sections, `/Root`
/// the catalog's new number and `/Custom` the string's, each object the
/// source's at its number, its references renumbered, and what moved still
/// at its old number, as the file had it.
#[test]
fn an_update_under_a_renumbering_writes_what_moves_and_what_names_it() {
    let mut objects = SPELLED.to_vec();
    objects.push(NAMING);
    let trailer = "<< /Size 8 /Root 1 0 R /Custom 4 0 R >>";
    let table = fixture::assembled("1.7", &objects, trailer);
    let stream = {
        let reading = open(&table);
        let doc = reading.to_document().unwrap();
        let mut out = Vec::new();
        let options = options(XrefKind::Stream, None);
        write::document(&mut out, &doc, None, Some(&reading), &options).unwrap();
        out
    };
    for (name, file) in [("table", table), ("stream", stream)] {
        let reading = open(&file);
        let doc = reading.to_document().unwrap();
        let base = write::Base::of(&reading).unwrap();
        let top = base.size;
        let map = moving(&reading, &[(1, top), (3, top + 1), (4, top + 2)]);
        let to = by(&map);
        let (out, written) = updated_under(
            &file,
            &base,
            write::bodies(&doc),
            None,
            Some(&reading),
            &map,
        );
        // The stream file's cross-reference stream.
        let xref: Vec<u32> = (doc.objects())
            .filter(|(_, o)| o.as_stream().is_some_and(|s| s.dict.has_type(b"XRef")))
            .map(|(id, _)| id.0)
            .collect();
        let want = [&[2, 7][..], &xref, &[top, top + 1, top + 2]].concat();
        assert_eq!(listed_numbers(&written), want, "{name}");
        for (id, body) in write::bodies(&doc) {
            let write::Body::Read { object, record } = &body else {
                panic!("{name}: {id:?} is as read");
            };
            let at = to(id).unwrap();
            if !written.section.lists(at.0) {
                continue;
            }
            let copy = copy_of(&reading, record, object, at, &to);
            let got = listed_at(&out, &written, at.0);
            assert!(got.starts_with(&copy), "{name}: {id:?} at {at:?}");
        }
        assert!(written.base.renumbered, "{name}");
        let root = written
            .base
            .trailer
            .get(b"Root")
            .and_then(Object::as_reference);
        assert_eq!(root, Some((top, 0)), "{name}");
        let back = open(&out);
        assert!(sound(&back), "{name}: {:?}", back.warnings());
        assert_eq!(back.facts().sections, 2, "{name}");
        let named = |key: &[u8]| {
            back.trailer()
                .dict()
                .get(key)
                .and_then(Object::as_reference)
        };
        assert_eq!(named(b"Root"), Some((top, 0)), "{name}");
        assert_eq!(named(b"Custom"), Some((top + 2, 0)), "{name}");
        let ids: Vec<ObjectId> = (doc.objects())
            .filter(|(_, o)| o.as_stream().is_none_or(|s| !s.dict.has_type(b"XRef")))
            .map(|(id, _)| id)
            .collect();
        assert_moved(&reading, &back, &ids, &to, name);
        for id in [(1, 0), (3, 0), (4, 0)] {
            assert_eq!(handed(&back, id), handed(&reading, id), "{name}: {id:?}");
        }
    }
}

/// The test writer's files under each of its methods, the document's
/// metadata encrypted and not, each updated under a renumbering that moves
/// the catalog, the page, the metadata stream and the Info past every
/// number, given every object of its document as read and its reading.
/// Written, those and what names them (the pages, naming the page), and
/// nothing else: under AES-256 each copied, its ciphertext the source's;
/// under RC4 and AES-128, which make keys of the number, what moves written
/// anew and the pages copied at its own number, its reference written anew
/// in place. Read back by the user's and the owner's password, `/Root` and
/// `/Info` follow, every object is the source's at its number, the
/// encryption dictionary where it was; nothing the update writes is in the
/// clear but the metadata where it's left so, at its new number.
#[test]
fn each_method_updates_renumbered_copying_where_keys_aren_t_made_of_numbers() {
    let mut moved = 0;
    for kind in KINDS_ENCRYPTED {
        let per_object = !matches!(kind, Kind::Aes256R5 | Kind::Aes256);
        for encrypt_metadata in [true, false] {
            let name = format!("{kind:?}, metadata encrypted: {encrypt_metadata}");
            let source = written_otherwise(kind, encrypt_metadata);
            let reading = opened(&source, "user");
            let own = Encryptor::of(&reading).unwrap().unwrap();
            let doc = reading.to_document().unwrap();
            let base = write::Base::of(&reading).unwrap();
            let top = base.size;
            let moves = [(12, top), (4, top + 1), (5, top + 2), (13, top + 3)];
            let map = moving(&reading, &moves);
            let to = by(&map);
            let (out, written) = updated_under(
                &source,
                &base,
                write::bodies(&doc),
                Some(&own),
                Some(&reading),
                &map,
            );
            assert_eq!(
                listed_numbers(&written),
                [1, top, top + 1, top + 2, top + 3],
                "{name}"
            );
            for (id, body) in write::bodies(&doc) {
                let write::Body::Read { object, record } = &body else {
                    continue;
                };
                let at = to(id).unwrap();
                if !written.section.lists(at.0) {
                    continue;
                }
                let copy = copy_of(&reading, record, object, at, &to);
                let copied = listed_at(&out, &written, at.0).starts_with(&copy);
                assert_eq!(copied, at == id || !per_object, "{name}: {id:?} at {at:?}");
                moved += usize::from(copied && at != id);
            }
            let filters = !matches!(kind, Kind::Rc4By40 | Kind::Rc4By128);
            for text in SECRETS {
                let clear = text == XMP && !encrypt_metadata && filters;
                let said = String::from_utf8_lossy(text);
                assert_eq!(
                    in_clear(&out[source.len()..], text),
                    clear,
                    "{name}: {said:?}"
                );
            }
            let encrypt = doc.trailer.dict().get(b"Encrypt").cloned();
            for password in ["user", "owner"] {
                let name = format!("{name}, {password}");
                let back = opened(&out, password);
                assert!(sound(&back), "{name}: {:?}", back.warnings());
                let trailer = back.trailer().dict();
                let named = |key: &[u8]| trailer.get(key).and_then(Object::as_reference);
                assert_eq!(named(b"Root"), Some((top, 0)), "{name}");
                assert_eq!(named(b"Info"), Some((top + 3, 0)), "{name}");
                assert_eq!(
                    trailer.get(b"Encrypt").map(unplaced),
                    encrypt.as_ref().map(unplaced)
                );
                assert_moved(&reading, &back, &written_of(&doc), &to, &name);
            }
        }
    }
    assert!(moved > 0, "AES-256's copied at new numbers");
}

/// [`listed`] under each method, its encryption dictionary at [`ENCRYPT`],
/// updated under a renumbering that moves every object but the encryption
/// dictionary past every number, given its document's objects as read and
/// its reading, so that each is written: what the list encrypts isn't in
/// the clear in the update, and what it leaves in the clear is, the
/// document's metadata at its new number among them under
/// `/EncryptMetadata false`; read back by the user's password, every object
/// is the source's at its number.
#[test]
fn each_method_updates_renumbered_with_nothing_in_the_clear() {
    let (plain, trailer) = listed();
    for method in methods() {
        let name = method.name;
        let source = encrypted_by(&method.encryptor, &plain, &trailer, XrefKind::Table);
        let reading = opened(&source, "user");
        let own = Encryptor::of(&reading).unwrap().unwrap();
        let doc = reading.to_document().unwrap();
        let base = write::Base::of(&reading).unwrap();
        let encrypt = own.at().unwrap();
        let moves: Vec<(u32, u32)> = (reading.ids())
            .filter(|&id| id != encrypt)
            .map(|(n, _)| (n, n + base.size))
            .collect();
        let map = moving(&reading, &moves);
        let to = by(&map);
        let (out, written) = updated_under(
            &source,
            &base,
            write::bodies(&doc),
            Some(&own),
            Some(&reading),
            &map,
        );
        assert!(!written.section.lists(encrypt.0), "{name}");
        for (text, encrypted) in listed_rows(&method) {
            let said = String::from_utf8_lossy(text);
            assert_eq!(
                in_clear(&out[source.len()..], text),
                !encrypted,
                "{name}: {said:?}"
            );
        }
        let back = opened(&out, "user");
        assert!(sound(&back), "{name}: {:?}", back.warnings());
        assert_moved(&reading, &back, &written_of(&doc), &to, name);
    }
}

/// An update given the identity, every number named as it is, is the
/// update given none, byte for byte: of [`SPELLED`] and [`NAMING`] written
/// with a table and with a stream, and of [`plaintext`] under each method,
/// a table and a stream, given its document's objects as read, one changed,
/// and its reading, it passes over the rest as with none; and what it
/// writes says it was renumbered.
#[test]
fn an_update_under_the_identity_is_the_update_under_none() {
    let mut objects = SPELLED.to_vec();
    objects.push(NAMING);
    let spelled = spelled_as(&objects);
    let (plain, trailer) = plaintext();
    let mut sources: Vec<(String, Vec<u8>, &str, ObjectId)> = Vec::new();
    for kind in KINDS {
        let reading = open(&spelled);
        let doc = reading.to_document().unwrap();
        let mut out = Vec::new();
        write::document(&mut out, &doc, None, Some(&reading), &options(kind, None)).unwrap();
        sources.push((format!("spelled, {kind:?}"), out, "", (3, 0)));
    }
    let methods = methods();
    for method in &methods {
        for kind in KINDS {
            let file = encrypted_by(&method.encryptor, &plain, &trailer, kind);
            sources.push((format!("{}, {kind:?}", method.name), file, "user", PAGE));
        }
    }
    for (name, file, password, changed) in sources {
        let reading = opened(&file, password);
        let own = Encryptor::of(&reading).unwrap();
        let mut doc = reading.to_document().unwrap();
        doc.get_dict_mut(changed).unwrap().set("Rotate", 90);
        let base = write::Base::of(&reading).unwrap();
        let to = |id| Some(id);
        let mut ways = Vec::new();
        for renumber in [None, Some(&to as &(dyn serialize::Renumber + Sync))] {
            let mut options = write::Options::DEFAULT;
            options.renumber = renumber;
            let mut out = file.clone();
            let written = write::update(
                &mut out,
                &base,
                write::bodies(&doc),
                own.as_ref(),
                Some(&reading),
                &options,
            )
            .unwrap()
            .unwrap();
            assert_eq!(listed_numbers(&written), [changed.0], "{name}");
            assert_eq!(written.base.renumbered, renumber.is_some(), "{name}");
            ways.push(out);
        }
        assert_eq!(ways[0], ways[1], "{name}");
    }
}

/// A second update after one under a renumbering goes after its
/// `Written::base`, which says it was renumbered: given an object as read,
/// or a reading, it's refused, nothing written; given objects in the
/// file's numbers now, built or edited, under a renumbering of its own and
/// the file's own encryptor, it writes them, `/Root` following the catalog
/// again. Read back with three sections, each object at its last number,
/// what each moved still at its old one. Plain, and under AES-128 and
/// AES-256.
#[test]
fn a_second_update_after_a_renumbered_one_is_given_the_numbers_it_wrote() {
    let mut sources = vec![("plain".to_string(), spelled(), "", (1, 0))];
    for kind in [Kind::Aes128, Kind::Aes256] {
        sources.push((
            format!("{kind:?}"),
            written_otherwise(kind, true),
            "user",
            (12, 0),
        ));
    }
    for (name, file, password, catalog) in sources {
        let reading = opened(&file, password);
        let own = Encryptor::of(&reading).unwrap();
        let doc = reading.to_document().unwrap();
        let base = write::Base::of(&reading).unwrap();
        let top = base.size;
        let first_map = moving(&reading, &[(catalog.0, top)]);
        let (first_out, first) = updated_under(
            &file,
            &base,
            write::bodies(&doc),
            own.as_ref(),
            Some(&reading),
            &first_map,
        );
        assert!(first.base.renumbered, "{name}");
        assert_eq!(first.base.source, base.source, "{name}");
        let plain = write::Options::DEFAULT;
        for from in [Some(&reading as &dyn write::ReadFrom), None] {
            let mut refused = first_out.clone();
            let given = write::bodies(&doc);
            let got = write::update(&mut refused, &first.base, given, own.as_ref(), from, &plain);
            assert_eq!(got, Err(Error::Renumbered), "{name}");
            assert_eq!(refused, first_out, "{name}");
        }
        // The catalog as the file has it now, at its number there, edited.
        let after = opened(&first_out, password);
        let mut edited = after.get((top, 0)).unwrap().clone();
        edited.as_dict_mut().unwrap().set("Lang", lit(b"en"));
        let next = first.base.size;
        let second_map = moving(&after, &[(top, next)]);
        let given = [((top, 0), write::Body::from(&edited))];
        let (out, second) = updated_under(
            &first_out,
            &first.base,
            given,
            own.as_ref(),
            None,
            &second_map,
        );
        assert!(second.base.renumbered, "{name}");
        assert_eq!(listed_numbers(&second), [next], "{name}");
        let back = opened(&out, password);
        assert!(sound(&back), "{name}: {:?}", back.warnings());
        assert_eq!(back.facts().sections, 3, "{name}");
        let root = back
            .trailer()
            .dict()
            .get(b"Root")
            .and_then(Object::as_reference);
        assert_eq!(root, Some((next, 0)), "{name}");
        let mut want = handed(&reading, catalog);
        want.as_dict_mut().unwrap().set("Lang", lit(b"en"));
        assert_alike(&handed(&back, (next, 0)), &want, &name);
        assert_eq!(handed(&back, catalog), handed(&reading, catalog), "{name}");
        assert_eq!(handed(&back, (top, 0)), handed(&after, (top, 0)), "{name}");
    }
}

/// [`plaintext`] under `kind`, its encryption dictionary at [`ENCRYPT`]
/// with its `/CF` by reference, as object 41, the Info at 13: a file whose
/// encryption dictionary refers to an object.
fn with_cf_at_41(kind: Kind) -> Vec<u8> {
    let (mut plain, trailer) = plaintext();
    let mut dict = dictionary_of(kind, true);
    let filters = dict.remove(b"CF").unwrap();
    dict.set("CF", Object::reference(41));
    let id = Object::hex(ID);
    let mut source_trailer = dict! { "Root" => Object::reference(1), "ID" => vec![id.clone(), id] };
    source_trailer.set("Encrypt", Object::reference(ENCRYPT));
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut section = Section::full(0);
    let objects = [
        (1, catalog()),
        (41, filters.clone()),
        (ENCRYPT, Object::from(dict)),
    ];
    for (n, o) in &objects {
        let pos = serialize::indirect(&mut out, (*n, 0), o).unwrap();
        let entry = Entry::InUse {
            offset: pos,
            generation: 0,
        };
        section.insert(*n, entry);
    }
    let pos = out.len();
    section.table(&mut out, source_trailer, pos).unwrap();
    let encryptor = encryptor_of(&out);
    assert!(encryptor.dictionary_object((41, 0)));
    plain.push((41, filters));
    encrypted_by(&encryptor, &plain, &trailer, XrefKind::Table)
}

/// An update under a renumbering is refused, nothing written: one that
/// moves the encryption dictionary, or moves what it refers to (its `/CF`,
/// object 41), each by its number, and one that names either none, given
/// without it (given, it's an object given no number; and the dictionary,
/// given while its `/CF` moves, is written again, at its own number); one
/// that writes another object at 41, given without it, by that number, or
/// at the dictionary's; one that writes another object where one it passes
/// over is, by that number; one that gives an object given no number; and
/// one given at the number of one it passes over, though it's written
/// elsewhere. Under AES-128 and AES-256, and plain.
#[test]
fn an_update_that_moves_what_stays_where_it_is_is_refused() {
    let encrypt = (ENCRYPT, 0);
    for kind in [Kind::Aes128, Kind::Aes256] {
        let file = with_cf_at_41(kind);
        let reading = opened(&file, "user");
        let own = Encryptor::of(&reading).unwrap();
        let doc = reading.to_document().unwrap();
        let base = write::Base::of(&reading).unwrap();
        let top = base.size;
        let all = || write::bodies(&doc).collect::<Vec<_>>();
        let without = |numbers: &[u32]| {
            (write::bodies(&doc).filter(|(id, _)| !numbers.contains(&id.0))).collect::<Vec<_>>()
        };
        // The Info moved too, so that something's written.
        let mut nameless = moving(&reading, &[(13, top)]);
        nameless.remove(&encrypt);
        let mut cf_nameless = moving(&reading, &[(13, top)]);
        cf_nameless.remove(&(41, 0));
        let cases = [
            (
                "the dictionary moved",
                moving(&reading, &[(ENCRYPT, top)]),
                all(),
                Error::Moved(encrypt),
            ),
            (
                "the dictionary named none",
                nameless,
                without(&[ENCRYPT]),
                Error::Moved(encrypt),
            ),
            (
                "its /CF moved",
                moving(&reading, &[(41, top)]),
                without(&[ENCRYPT]),
                Error::Moved((41, 0)),
            ),
            (
                "its /CF moved, it given",
                moving(&reading, &[(41, top)]),
                all(),
                Error::EncryptAt(ENCRYPT),
            ),
            (
                "its /CF named none",
                cf_nameless,
                without(&[ENCRYPT, 41]),
                Error::Moved((41, 0)),
            ),
            (
                "the Info at its /CF's",
                moving(&reading, &[(13, 41)]),
                without(&[41]),
                Error::Moved((41, 0)),
            ),
            (
                "the Info at its number",
                moving(&reading, &[(13, ENCRYPT)]),
                without(&[ENCRYPT]),
                Error::EncryptAt(ENCRYPT),
            ),
            (
                "the Info at its /CF's, given",
                moving(&reading, &[(13, 41)]),
                all(),
                Error::Twice(41),
            ),
        ];
        for (case, map, given, want) in cases {
            let name = format!("{kind:?}, {case}");
            let to = by(&map);
            let mut options = write::Options::DEFAULT;
            options.renumber = Some(&to);
            let mut out = file.clone();
            let got = write::update(
                &mut out,
                &base,
                given,
                own.as_ref(),
                Some(&reading),
                &options,
            );
            assert_eq!(got, Err(want), "{name}");
            assert_eq!(out, file, "{name}");
        }
    }
    let file = spelled();
    let reading = open(&file);
    let doc = reading.to_document().unwrap();
    let base = write::Base::of(&reading).unwrap();
    let mut unnumbered = moving(&reading, &[]);
    unnumbered.remove(&(3, 0));
    let mut elsewhere = moving(&reading, &[]);
    elsewhere.insert((5, 1), (base.size, 0));
    // Each case's renumbering, and the array given again, at a generation.
    let cases = [
        (
            "the page at the array's",
            moving(&reading, &[(3, 5)]),
            None,
            Error::Twice(5),
        ),
        (
            "the page named none",
            unnumbered,
            None,
            Error::Unnumbered((3, 0)),
        ),
        (
            "the array given twice, moved",
            elsewhere,
            Some(1),
            Error::Twice(5),
        ),
    ];
    let again = Object::from(vec![Object::from(4)]);
    for (case, map, generation, want) in cases {
        let to = by(&map);
        let mut options = write::Options::DEFAULT;
        options.renumber = Some(&to);
        let mut given: Vec<_> = write::bodies(&doc).collect();
        if let Some(generation) = generation {
            given.push(((5, generation), write::Body::from(&again)));
        }
        let mut out = file.clone();
        let got = write::update(&mut out, &base, given, None, Some(&reading), &options);
        assert_eq!(got, Err(want), "{case}");
        assert_eq!(out, file, "{case}");
    }
}
