//! Resources' digests: pinned for resources as a writer builds them, every
//! kind of value fed apart, and equal values digesting alike however
//! they're written.

use std::collections::BTreeSet;
use std::sync::Arc;

use pdf_objects::{Data, Dict, Entry, Object, Span, Str, Stream, Value, dict};

use super::*;

fn id(object: Object) -> ResId {
    ResId::of(&[object])
}

fn stream(dict: Dict, data: Data) -> Object {
    let mut stream = Stream::default();
    stream.dict = dict;
    stream.data = data;
    Object::from(stream)
}

/// One value of every kind a writer builds, and some of their edges, in
/// one array: a string's form chosen or not, each stream's data in memory
/// and its `/Length` set by [`Stream::new`].
fn every_kind() -> Object {
    let nested = dict! {
        "B" => dict! { "C" => Object::null() },
        "A" => vec![Object::from(1), Object::from(2)],
        "Z" => Object::from(ObjectId::new(3, 1)),
    };
    Object::from(vec![
        Object::null(),
        Object::from(true),
        Object::from(false),
        Object::from(0),
        Object::from(-1),
        Object::from(i64::MAX),
        Object::from(i64::MIN),
        Object::from(2.5),
        Object::from(0.0),
        Object::from(-0.0),
        Object::from(1e30),
        Object::from(0.1f32),
        Object::from(1e10f32),
        Object::from("N"),
        Object::from("Na me"),
        Object::from(""),
        Object::bytes(*b"lit"),
        Object::hex([0, 0xff]),
        Object::bytes([]),
        Object::from(Vec::new()),
        Object::from(vec![Object::from(vec![Object::from(1)]), Object::from(2)]),
        Object::from(vec![Object::from(vec![Object::from(1), Object::from(2)])]),
        Object::from(Dict::new()),
        Object::from(nested),
        Object::from(Stream::new(dict! { "Filter" => "AHx" }, b"41>".to_vec())),
        Object::from(Stream::new(Dict::new(), Vec::new())),
        Object::from(ObjectId::new(7, 0)),
        Object::from(ObjectId::new(u32::MAX, u16::MAX)),
    ])
}

/// Resources as a content writer builds them, their digests pinned: a
/// resource's name is made from its digest, so a change to how a value is
/// fed would rename every resource content written before names. A
/// property list, an ExtGState of an `f32` alpha, an object already
/// written, a stream with its `/Length`, a font, and a value of every
/// kind.
#[test]
fn digests_of_resources_as_writers_build_them_are_pinned() {
    let list = dict! { "Type" => "Pagination", "Subtype" => "Header" };
    let pinned = ResId(0xf59e69a77e0a864f2fd274e4f601de18);
    assert_eq!(Res::dict(list).id, pinned);

    let gstate = dict! { "Type" => "ExtGState", "ca" => 0.3f32 };
    let pinned = ResId(0x5e992d673c12d51cee9fe27e39ef7b67);
    assert_eq!(Res::dict(gstate).id, pinned);

    let pinned = ResId(0x43f0d2ed6dff504a255aa16751823034);
    assert_eq!(Res::written(ObjectId::new(7, 0)).id, pinned);

    let q = Object::from(Stream::new(Dict::new(), b"q\n".to_vec()));
    let pinned = ResId(0x003dfd866ae26b2ca535f8c218113750);
    assert_eq!(Res::of([q]).id, pinned);

    let font = dict! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    };
    let pinned = ResId(0xb31ab5babf0445a2a50ae8ab6340c26e);
    assert_eq!(Res::dict(font).id, pinned);

    let pinned = ResId(0x38375e5a14ba673b9d53819488c6cd64);
    assert_eq!(id(every_kind()), pinned);
}

/// Values of every kind, and values a feed without kinds or lengths
/// would run together, each digest apart: a kind's tag, each run of bytes
/// and each list counted, a string's bytes apart from a text string and
/// from ciphertext, a stream's data in memory apart from data at a place
/// and from data neither, and each place apart.
#[test]
fn every_kind_of_value_is_fed_apart() {
    let length = |n: i64| dict! { "Length" => n };
    let at = |stream, start, end| Data::At(Span::new(stream, start, end));
    let values = [
        Object::null(),
        Object::from(true),
        Object::from(false),
        Object::from(0),
        Object::from(1),
        Object::from(0.0),
        Object::from(-0.0),
        Object::from(1.0),
        Object::from("a"),
        Object::from("ab"),
        Object::bytes(*b"a"),
        Object::new(Value::String(Str::Text("a".into()), None)),
        Object::new(Value::String(Str::Encrypted(b"a".to_vec()), None)),
        Object::bytes("é".as_bytes()),
        Object::new(Value::String(Str::Text("é".into()), None)),
        Object::from(Vec::new()),
        Object::from(vec![Object::from("ab"), Object::from("c")]),
        Object::from(vec![Object::from("a"), Object::from("bc")]),
        Object::from(vec![Object::from("a/b"), Object::from("c")]),
        Object::from(vec![Object::from("a"), Object::from("b/c")]),
        Object::from(vec![Object::from(vec![Object::from(1)]), Object::from(2)]),
        Object::from(vec![Object::from(vec![Object::from(1), Object::from(2)])]),
        Object::from(Dict::new()),
        Object::from(dict! { "a" => Object::null() }),
        Object::from(dict! { "a" => "b" }),
        Object::from(dict! { "ab" => Object::null() }),
        Object::from(ObjectId::new(1, 0)),
        Object::from(ObjectId::new(0, 1)),
        Object::from(ObjectId::new(1, 1)),
        stream(length(1), Data::Bytes(b"a".to_vec())),
        stream(Dict::new(), Data::Bytes(b"a".to_vec())),
        stream(length(1), Data::Bytes(b"b".to_vec())),
        stream(length(1), at(None, 0, 1)),
        stream(length(1), at(None, 1, 2)),
        stream(length(1), at(None, 0, 2)),
        stream(length(1), at(Some(ObjectId::new(5, 0)), 0, 1)),
        stream(length(1), at(Some(ObjectId::new(5, 1)), 0, 1)),
        stream(length(1), at(Some(ObjectId::new(6, 0)), 0, 1)),
        stream(length(1), Data::Unplaced),
        stream(Dict::new(), Data::Unplaced),
    ];
    let ids: BTreeSet<ResId> = values.iter().cloned().map(id).collect();
    assert_eq!(ids.len(), values.len(), "every value apart");

    // A list of objects is counted too: a resource of two objects isn't
    // one of them alone, nor an array of the two.
    let (a, b) = (Object::from(1), Object::from(2));
    let two = ResId::of(&[a.clone(), b.clone()]);
    assert_ne!(two, ResId::of(std::slice::from_ref(&a)));
    assert_ne!(two, id(Object::from(vec![a, b])));
}

/// Values equal by what they are digest alike, however they're written:
/// a dictionary's entries in another order, a key written twice taken at
/// its last value, a string literal or hex, a value and a key with where
/// they were written or without, and an `f32` as the `f64` of its
/// shortest decimal.
#[test]
fn equal_values_digest_alike_however_written() {
    let alike = |a: Object, b: Object| assert_eq!(id(a.clone()), id(b.clone()), "{a:?} {b:?}");

    alike(
        Object::from(dict! { "Type" => "Pagination", "Subtype" => "Header" }),
        Object::from(dict! { "Subtype" => "Header", "Type" => "Pagination" }),
    );
    alike(
        Object::from(dict! { "A" => 1, "B" => 2, "A" => 3 }),
        Object::from(dict! { "B" => 2, "A" => 3 }),
    );
    alike(Object::bytes(*b"a)"), Object::hex(*b"a)"));
    alike(
        Object::bytes(*b"a)"),
        Object::new(Value::String(
            Str::Bytes(b"a)".to_vec()),
            Some(pdf_objects::Form::Literal),
        )),
    );

    let span = Span::new(None, 10, 11);
    alike(Object::from(5).at(span), Object::from(5));
    let mut placed = Dict::new();
    placed.push_entry(Entry {
        key: b"K".to_vec(),
        key_at: Some(span),
        value: Object::from(vec![Object::from(1).at(span)]).at(span),
    });
    alike(
        Object::from(placed),
        Object::from(dict! { "K" => vec![Object::from(1)] }),
    );

    alike(Object::from(0.3f32), Object::from(0.3));
    alike(Object::from(1e10f32), Object::from(1e10));

    // A stream however its data was given, `/Length` set alike.
    let mut set = Stream::default();
    set.dict = dict! { "Filter" => "AHx" };
    set.set_bytes(b"41>".to_vec());
    alike(
        Object::from(set),
        Object::from(Stream::new(dict! { "Filter" => "AHx" }, b"41>".to_vec())),
    );
}

/// A resource is named by its objects, one already written by a
/// reference to it, and a dictionary as the one object it is.
#[test]
fn a_resource_is_named_by_what_it_is() {
    let list = dict! { "Type" => "Pagination", "Subtype" => "Footer" };
    let res = Res::dict(list.clone());
    assert_eq!(res.id, id(Object::from(list.clone())));
    let objects: Arc<[Object]> = Arc::new([Object::from(list.clone())]);
    assert_eq!(res.body, ResBody::Objects(objects.clone()));
    assert_eq!(res, Res::of(objects));
    assert_eq!(res, Res::of(vec![Object::from(list)]));

    let written = Res::written(ObjectId::new(7, 0));
    assert_eq!(written.body, ResBody::Written(ObjectId::new(7, 0)));
    assert_eq!(written.id, id(Object::from(ObjectId::new(7, 0))));
    assert_ne!(written.id, Res::written(ObjectId::new(7, 1)).id);
    assert_ne!(written.id, Res::written(ObjectId::new(8, 0)).id);

    // Two objects a resource reaches, in order.
    let font = || {
        let program = Object::from(Stream::new(Dict::new(), b"glyphs".to_vec()));
        let descriptor = dict! { "FontFile2" => Object::from(ObjectId::new(1, 0)) };
        vec![Object::from(descriptor), program]
    };
    let mut reversed = font();
    reversed.reverse();
    assert_eq!(Res::of(font()), Res::of(font()));
    assert_ne!(Res::of(font()).id, Res::of(reversed).id);
}
