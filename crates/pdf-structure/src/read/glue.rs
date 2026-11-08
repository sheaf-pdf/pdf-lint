//! What the reader reads a stream's filters and an encryption dictionary
//! by, and gives a viewer of a stream's data, public ([`filters`],
//! [`encryption`], [`viewed`]), each answering as the reader does, which
//! reads by them; and a document's stream held as bytes decoded as a
//! reading decodes one.

use std::borrow::Cow;

use pdf_fixtures::fixture;
use pdf_objects::object::Data;
use pdf_objects::{Streams, Why, dict};

use super::lazy::files;
use super::tests::{KINDS, encryption_updated, root, settings, to_encrypt};
use super::*;

/// A reading's decode or its error, as text to set beside another's.
fn shown(decoded: Result<Vec<u8>, Error>) -> Result<Vec<u8>, String> {
    decoded.map_err(|error| format!("{error:?}"))
}

/// Each stream of each test file, read whole and on demand, decodes
/// through the filters [`filters`] gives of its dictionary, references
/// followed through the document (none in a cross-reference stream's), as
/// the reading decodes it: the same data, or the same error, where the
/// reading hands out the stream's data at all.
#[test]
fn a_streams_filters_are_the_readers() {
    let mut decoded = 0;
    for (name, file, passwords) in files() {
        let options = Options {
            passwords: &passwords,
            ..Options::DEFAULT
        };
        let reading = Reading::open(&file[..], &options).unwrap();
        let doc = reading.to_document().unwrap();
        for (id, object) in doc.objects() {
            let Some(stream) = object.as_stream() else {
                continue;
            };
            let Ok(data) = reading.data(id) else {
                continue;
            };
            let resolve = |id| (&doc).resolve(id);
            let given = match crate::crypt::is_xref(&stream.dict) {
                true => filters(&stream.dict, None),
                false => filters(&stream.dict, Some(&resolve)),
            };
            let through = match given {
                Ok(filters) => pdf_filters::decode(&data, &filters, STREAM, Options::DEFAULT.flate)
                    .map_err(Error::Decode),
                Err(why) => Err(Error::Filters(why)),
            };
            let read = reading.decoded_within(id, STREAM);
            assert_eq!(shown(through), shown(read), "{name}: {id}");
            decoded += 1;
        }
    }
    assert!(decoded >= 40, "{decoded} streams");
}

/// A reference among a stream's filters is followed by the resolver given
/// and refused with none, as in a cross-reference stream's dictionary.
#[test]
fn a_streams_filters_follow_references_by_the_resolver_given() {
    let mut dict = Dict::new();
    dict.set("Filter", vec![Object::from("AHx"), Object::reference(7)]);
    dict.set("DecodeParms", vec![Object::null(), Object::reference(8)]);
    let flate = Object::from("FlateDecode");
    let parms: Object = dict! { "Predictor" => 12, "Columns" => 4 }.into();
    let resolve = |id: ObjectId| match id.number {
        7 => Some(&flate),
        8 => Some(&parms),
        _ => None,
    };
    let given = filters(&dict, Some(&resolve)).unwrap();
    assert!(
        matches!(
            given[..],
            [
                Filter::AsciiHex,
                Filter::Flate {
                    predictor: Predictor::Png(_)
                }
            ]
        ),
        "{given:?}"
    );
    assert_eq!(filters(&dict, None), Err(Unfiltered::Reference));
    // A reference to no object is null: no filter.
    let none = |_: ObjectId| None;
    let mut one = Dict::new();
    one.set("Filter", Object::reference(9));
    assert_eq!(filters(&one, Some(&none)), Ok(Vec::new()));
}

/// Each encrypted test file's encryption dictionary read by
/// [`encryption`], references followed through the reading or its
/// document, gives the values the reader opened it by; and so does one
/// whose values are references, each asked of the resolver in the order
/// the reader reads them, which learns each object the dictionary refers
/// to.
#[test]
fn an_encryption_dictionarys_values_are_the_readers() {
    let mut encrypted = 0;
    let mut cases: Vec<(String, Vec<u8>, Vec<Password<'static>>)> = files();
    for kind in KINDS {
        let file = fixture::encrypted(&settings(kind, b""), &to_encrypt(&[]), root());
        let own = &file[find(&file, b"6 0 obj").unwrap()..];
        let own = &own[..find(own, b"endobj").unwrap()];
        let file = match find(own, b"/StmF /StdCF").is_some() {
            true => encryption_updated(
                &file,
                "/StmF /StdCF",
                "/StmF 9 0 R /StrF 9 0 R",
                &[(9, "/StdCF")],
            ),
            false => encryption_updated(&file, "/P -4", "/P 9 0 R", &[(9, "-4")]),
        };
        cases.push((format!("{kind:?}, by reference"), file, Vec::new()));
    }
    for (name, file, passwords) in cases {
        let options = Options {
            passwords: &passwords,
            ..Options::DEFAULT
        };
        let reading = Reading::open(&file[..], &options).unwrap();
        let Some(stored) = reading.stored() else {
            continue;
        };
        let mut asked = Vec::new();
        let values = encryption(&stored.dict, &mut |id| {
            asked.push(id);
            reading.get(id).ok().map(Cow::Borrowed)
        });
        assert_eq!(values, stored.values, "{name}");
        let doc = reading.to_document().unwrap();
        let values = encryption(&stored.dict, &mut |id| {
            (&doc).resolve(id).map(Cow::Borrowed)
        });
        assert_eq!(values, stored.values, "{name}, through its document");
        if name.ends_with("by reference") {
            assert!(asked.contains(&ObjectId::new(9, 0)), "{name}: {asked:?}");
            assert!(
                matches!(reading.encryption(), Encryption::Opened { .. }),
                "{name}"
            );
        }
        encrypted += 1;
    }
    assert!(encrypted >= 10, "{encrypted} encrypted files");
}

/// What a viewer takes of each answer a reading's decode gives
/// ([`viewed`]), by kind: the data; what decoded before a fault that isn't
/// the limit; past the limit, the caller's or the reader's own cap; and
/// why there's none.
#[test]
fn what_a_viewer_takes_of_a_decode_is_by_its_kind() {
    let id = ObjectId::new(4, 0);
    let decode = |data: &[u8], fault| {
        Err(Error::Decode(pdf_filters::Error {
            data: data.to_vec(),
            faults: vec![(0, fault)],
        }))
    };
    let why = |decoded: Result<Vec<u8>, Error>, capped| match viewed(id, decoded, capped) {
        Ok(data) => Ok(data),
        Err(unreadable) => {
            assert_eq!(unreadable.id, id);
            Err(unreadable.why)
        }
    };
    assert_eq!(why(Ok(b"BT".to_vec()), false), Ok(b"BT".to_vec()));
    let checksum = pdf_filters::Fault::Checksum;
    assert_eq!(why(decode(b"BT", checksum), false), Ok(b"BT".to_vec()));
    assert_eq!(why(decode(b"", checksum), false), Err(Why::Damaged));
    let limit = pdf_filters::Fault::Limit;
    assert_eq!(why(decode(b"BT", limit), false), Err(Why::Limit));
    assert_eq!(why(decode(b"BT", limit), true), Err(Why::Capped));
    let io = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "short");
    assert_eq!(why(Err(Error::Io(io)), false), Err(Why::Source));
    for error in [Error::Locked, Error::Encrypted, Error::Unsupported] {
        assert_eq!(why(Err(error), false), Err(Why::Locked));
    }
    let many = Unfiltered::Filters { count: 26 };
    assert_eq!(why(Err(Error::Filters(many)), false), Err(Why::Capped));
    let other = Unfiltered::Unsupported {
        filter: b"JBIG2Decode".to_vec(),
    };
    assert_eq!(why(Err(Error::Filters(other)), false), Err(Why::Filter));
    assert_eq!(why(Err(Error::NotAStream { id }), false), Err(Why::Damaged));
    let missing = Unread::Missing { id };
    assert_eq!(why(Err(Error::Unread(missing)), false), Err(Why::Damaged));
    let unread = Unread::Parse(parse::Error::Short { at: 7 });
    let said = viewed(id, Err(Error::Unread(unread.clone())), false).unwrap_err();
    assert_eq!(Some(said), unread.unreadable(id));
}

/// A document with a stream of each kind held as bytes, `data` and `dict`
/// at 2, and another of `/Filter /FlateDecode` at 3 that's `/Filter`'s
/// value by reference.
fn held(dict: Dict, data: &[u8]) -> Document {
    let mut doc = Document::new(Version::new(1, 7), root());
    doc.set(ObjectId::new(1, 0), dict! { "Type" => "Catalog" });
    let stream = pdf_objects::object::Stream::new(dict, data.to_vec());
    doc.set(ObjectId::new(2, 0), Object::from(stream));
    doc.set(ObjectId::new(3, 0), Object::from("FlateDecode"));
    doc
}

/// A case of a document's stream held as bytes: what it is, its
/// dictionary and data, the limit it's decoded within, and what it gives.
type Case<'a> = (&'a str, Dict, &'a [u8], usize, Result<Vec<u8>, Why>);

/// What a document gives of its stream 2, by kind.
fn given(doc: &Document, limit: usize) -> Result<Vec<u8>, Why> {
    match doc.decoded(ObjectId::new(2, 0), limit) {
        Ok(data) => {
            assert!(matches!(data, Cow::Owned(_)), "decoded, owned");
            Ok(data.into_owned())
        }
        Err(unreadable) => Err(unreadable.why),
    }
}

/// A stream a document holds as bytes, built in memory, is decoded as a
/// reading decodes one: through its filters, a reference among them
/// followed through the document, up to the limit asked for and the
/// reader's own cap, its own `/Crypt` filter no filter, whatever it names,
/// as the bytes are plaintext; a filter it doesn't take gives none.
/// Written into a file and read, a reading gives the same.
#[test]
fn a_documents_stream_held_as_bytes_is_decoded_as_a_reading_decodes_it() {
    let text = b"BT /F1 12 Tf (held as bytes) Tj ET".repeat(4);
    let zlib = fixture::zlib(&text);
    let flate = dict! { "Filter" => "FlateDecode" };
    let by_reference = dict! { "Filter" => Object::reference(3) };
    let unknown = dict! { "Filter" => "JBIG2Decode" };
    let crypt = |name: Option<&str>| {
        let mut parms = Dict::new();
        if let Some(name) = name {
            parms.set("Name", Object::from(name));
        }
        dict! {
            "Filter" => vec![Object::from("Crypt"), Object::from("FlateDecode")],
            "DecodeParms" => vec![Object::from(parms), Object::null()],
        }
    };
    let cases: [Case; 9] = [
        ("Flate", flate.clone(), &zlib, usize::MAX, Ok(text.clone())),
        (
            "by reference",
            by_reference,
            &zlib,
            usize::MAX,
            Ok(text.clone()),
        ),
        (
            "no filter",
            Dict::new(),
            &text,
            usize::MAX,
            Ok(text.clone()),
        ),
        (
            "a filter it doesn't take",
            unknown,
            b"\x00\x01",
            usize::MAX,
            Err(Why::Filter),
        ),
        (
            "/Crypt /Identity",
            crypt(None),
            &zlib,
            usize::MAX,
            Ok(text.clone()),
        ),
        (
            "/Crypt /StdCF",
            crypt(Some("StdCF")),
            &zlib,
            usize::MAX,
            Ok(text.clone()),
        ),
        (
            "Flate past the limit",
            flate.clone(),
            &zlib,
            10,
            Err(Why::Limit),
        ),
        (
            "no filter past the limit",
            Dict::new(),
            &text,
            10,
            Err(Why::Limit),
        ),
        ("at the limit", flate, &zlib, text.len(), Ok(text.clone())),
    ];
    for (what, dict, data, limit, want) in cases {
        let doc = held(dict, data);
        assert_eq!(given(&doc, limit), want, "{what}");
        // A reading of the document written gives the same: a file that
        // isn't encrypted reads its streams as in the clear, whatever crypt
        // filter they name.
        let mut out = Vec::new();
        crate::write::document(&mut out, &doc, None, None, &crate::write::Options::DEFAULT)
            .unwrap();
        let reading = Reading::open(&out[..], &Options::DEFAULT).unwrap();
        let read = match reading.decoded(ObjectId::new(2, 0), limit) {
            Ok(data) => Ok(data.into_owned()),
            Err(unreadable) => Err(unreadable.why),
        };
        assert_eq!(read, want, "{what}, written and read");
    }
    // Past the reader's own cap, which no limit asked for passes.
    let past = vec![b' '; STREAM + 1];
    let doc = held(Dict::new(), &past);
    assert_eq!(given(&doc, usize::MAX), Err(Why::Capped));
    assert_eq!(given(&doc, STREAM).map(|d| d.len()), Err(Why::Limit));
    // A stream it holds as a span of a file it hasn't is detached, as one
    // with no data held at all.
    let mut doc = held(Dict::new(), b"");
    let mut spanned = pdf_objects::object::Stream::new(Dict::new(), Vec::new());
    spanned.data = Data::At(Span::new(None, 10, 20));
    doc.set(ObjectId::new(2, 0), Object::from(spanned));
    assert_eq!(given(&doc, usize::MAX), Err(Why::Detached));
}
