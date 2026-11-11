//! Resources' digests: pinned for resources as a writer builds them, every
//! kind of value fed apart, and equal values digesting alike however
//! they're written. Resources equal where their ids are and their objects
//! are the same, exactly where their digests' bytes are, whatever a
//! resource made by hand says its id is; and held in memory.

use std::collections::{BTreeSet, HashMap};
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
    assert_eq!(Res::dict(list).unwrap().id, pinned);

    let gstate = dict! { "Type" => "ExtGState", "ca" => 0.3f32 };
    let pinned = ResId(0x5e992d673c12d51cee9fe27e39ef7b67);
    assert_eq!(Res::dict(gstate).unwrap().id, pinned);

    let pinned = ResId(0x43f0d2ed6dff504a255aa16751823034);
    assert_eq!(Res::written(ObjectId::new(7, 0)).id, pinned);

    let q = Object::from(Stream::new(Dict::new(), b"q\n".to_vec()));
    let pinned = ResId(0x003dfd866ae26b2ca535f8c218113750);
    assert_eq!(Res::of([q]).unwrap().id, pinned);

    let font = dict! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    };
    let pinned = ResId(0xb31ab5babf0445a2a50ae8ab6340c26e);
    assert_eq!(Res::dict(font).unwrap().id, pinned);

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
    let res = Res::dict(list.clone()).unwrap();
    assert_eq!(res.id, id(Object::from(list.clone())));
    let objects: Arc<[Object]> = Arc::new([Object::from(list.clone())]);
    assert_eq!(res.body, ResBody::Objects(objects.clone()));
    assert_eq!(res, Res::of(objects).unwrap());
    assert_eq!(res, Res::of(vec![Object::from(list)]).unwrap());

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
    assert_eq!(Res::of(font()).unwrap(), Res::of(font()).unwrap());
    assert_ne!(Res::of(font()).unwrap().id, Res::of(reversed).unwrap().id);
}

/// The bytes [`feed`] writes for `object`.
fn fed(object: &Object) -> Vec<u8> {
    let mut out = Vec::new();
    feed(&mut out, object);
    out
}

/// A resource of `objects` as one built by hand might be, its id theirs or
/// one given, nothing refused.
fn by_hand(id: ResId, objects: Vec<Object>) -> Res {
    Res {
        id,
        body: ResBody::Objects(objects.into()),
    }
}

/// Every pair of values of every kind is the same exactly where their
/// digests' bytes are: each value the same as itself alone.
#[test]
fn values_are_the_same_exactly_where_they_feed_alike() {
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
        Object::from(f64::NAN),
        Object::from("a"),
        Object::bytes(*b"a"),
        Object::new(Value::String(Str::Text("a".into()), None)),
        Object::new(Value::String(Str::Encrypted(b"a".to_vec()), None)),
        Object::from(Vec::new()),
        Object::from(vec![Object::from(1)]),
        Object::from(vec![Object::from(1), Object::from(2)]),
        Object::from(Dict::new()),
        Object::from(dict! { "a" => 1 }),
        Object::from(dict! { "a" => 1, "b" => 2 }),
        Object::from(dict! { "a" => 2 }),
        Object::from(ObjectId::new(1, 0)),
        Object::from(ObjectId::new(1, 1)),
        stream(Dict::new(), Data::Bytes(b"a".to_vec())),
        stream(dict! { "a" => 1 }, Data::Bytes(b"a".to_vec())),
        stream(Dict::new(), Data::Bytes(b"b".to_vec())),
        stream(Dict::new(), at(None, 0, 1)),
        stream(Dict::new(), at(Some(ObjectId::new(5, 0)), 0, 1)),
        stream(Dict::new(), Data::Unplaced),
    ];
    for (i, a) in values.iter().enumerate() {
        for (j, b) in values.iter().enumerate() {
            assert_eq!(same(a, b), fed(a) == fed(b), "{a:?} {b:?}");
            assert_eq!(same(a, b), i == j, "{a:?} {b:?}");
        }
    }
}

/// Resources of equal objects are equal however the objects are written,
/// by each of the digest's rules: a dictionary's entries in another order,
/// a key written twice taken at its last value, a string literal or hex,
/// a value with where it was written or without, and an `f32` as the `f64`
/// of its shortest decimal. And not equal where the digest parts them: a
/// text string beside its bytes, an integer beside its real.
#[test]
fn equal_resources_are_equal_however_written() {
    let span = Span::new(None, 10, 11);
    let mut placed = Dict::new();
    placed.push_entry(Entry {
        key: b"K".to_vec(),
        key_at: Some(span),
        value: Object::from(vec![Object::from(1).at(span)]).at(span),
    });
    let alike = [
        (
            Object::from(dict! { "Type" => "Pagination", "Subtype" => "Header" }),
            Object::from(dict! { "Subtype" => "Header", "Type" => "Pagination" }),
        ),
        (
            Object::from(dict! { "A" => 1, "B" => 2, "A" => 3 }),
            Object::from(dict! { "B" => 2, "A" => 3 }),
        ),
        (
            Object::from(dict! { "A" => 1, "B" => 2, "A" => 3 }),
            Object::from(dict! { "A" => 3, "B" => 2 }),
        ),
        (Object::bytes(*b"a)"), Object::hex(*b"a)")),
        (Object::from(5).at(span), Object::from(5)),
        (
            Object::from(placed),
            Object::from(dict! { "K" => vec![Object::from(1)] }),
        ),
        (Object::from(0.3f32), Object::from(0.3)),
        (Object::from(1e10f32), Object::from(1e10)),
    ];
    for (a, b) in alike {
        assert!(same(&a, &b) && fed(&a) == fed(&b), "{a:?} {b:?}");
        let (x, y) = (Res::of([a.clone()]).unwrap(), Res::of([b.clone()]).unwrap());
        assert_eq!(x, y, "{a:?} {b:?}");
        assert_eq!(y, x, "{a:?} {b:?}");
    }
    let apart = [
        (
            Object::new(Value::String(Str::Text("a".into()), None)),
            Object::bytes(*b"a"),
        ),
        (Object::from(1), Object::from(1.0)),
        (
            Object::from(dict! { "A" => 1 }),
            Object::from(dict! { "A" => 1.0 }),
        ),
        (
            Object::from(vec![Object::from(0)]),
            Object::from(vec![Object::from(0.0)]),
        ),
    ];
    for (a, b) in apart {
        assert!(!same(&a, &b) && fed(&a) != fed(&b), "{a:?} {b:?}");
        assert_ne!(Res::of([a.clone()]).unwrap(), Res::of([b]).unwrap());
    }
}

/// An id names a resource, and its objects decide it: a resource made by
/// hand with another's id and other objects isn't that one, nor is one
/// whose id isn't its objects'; a clone is its resource by pointer; and
/// objects that are a reference to an object written aren't that object,
/// whose id is theirs.
#[test]
fn an_id_names_a_resource_and_its_objects_decide_it() {
    let header = Object::from(dict! { "Type" => "Pagination", "Subtype" => "Header" });
    let footer = Object::from(dict! { "Type" => "Pagination", "Subtype" => "Footer" });
    let res = Res::of([header.clone()]).unwrap();
    let forged = by_hand(res.id, vec![footer.clone()]);
    assert_ne!(res, forged);
    assert_ne!(forged, res);
    let stale = by_hand(ResId(res.id.0 ^ 1), vec![header.clone()]);
    assert_ne!(res, stale);
    assert_eq!(res, by_hand(res.id, vec![header.clone()]));

    let clone = res.clone();
    let (ResBody::Objects(a), ResBody::Objects(b)) = (&res.body, &clone.body) else {
        panic!("objects")
    };
    assert!(Arc::ptr_eq(a, b));
    assert_eq!(res, clone);

    let seven = ObjectId::new(7, 0);
    let reference = Res::of([Object::from(seven)]).unwrap();
    assert_eq!(reference.id, Res::written(seven).id);
    assert_ne!(reference, Res::written(seven));
    assert_eq!(Res::written(seven), Res::written(seven));
    assert_ne!(Res::written(seven), Res::written(ObjectId::new(7, 1)));
}

/// A resource is a map's key, hashed by its id: resources made apart of
/// equal objects are one key, and one made by hand with another's id and
/// other objects a key of its own.
#[test]
fn a_resource_is_a_key() {
    let gstate = |ca: f64, first: bool| {
        let dict = match first {
            true => dict! { "Type" => "ExtGState", "ca" => ca },
            false => dict! { "ca" => ca, "Type" => "ExtGState" },
        };
        Res::dict(dict).unwrap()
    };
    let mut written: HashMap<Res, ObjectId> = HashMap::new();
    for (n, res) in [gstate(0.5, true), gstate(0.5, false), gstate(0.25, true)]
        .into_iter()
        .enumerate()
    {
        let next = ObjectId::new(n as u32 + 1, 0);
        written.entry(res).or_insert(next);
    }
    assert_eq!(written.len(), 2);
    assert_eq!(written[&gstate(0.5, false)], ObjectId::new(1, 0));
    assert_eq!(written[&gstate(0.25, false)], ObjectId::new(3, 0));
    let forged = by_hand(
        gstate(0.5, true).id,
        vec![Object::from(dict! { "Type" => "ExtGState", "ca" => 0.75 })],
    );
    assert!(!written.contains_key(&forged));
    written.insert(forged.clone(), ObjectId::new(4, 0));
    assert_eq!(written.len(), 3);
    assert_eq!(written[&gstate(0.5, true)], ObjectId::new(1, 0));
    assert_eq!(written[&forged], ObjectId::new(4, 0));
}

/// A resource holds every stream's data in memory: a stream whose data is
/// where a reader found it, or neither in memory nor placed, is refused,
/// at the top of its objects, in a dictionary or an array, or in a
/// stream's dictionary, naming which of its objects holds it; one in
/// memory is taken, wherever it is. A resource named by hand is refused
/// alike, and one written already never is.
#[test]
fn a_resource_holds_its_streams_in_memory() {
    let placed = Data::At(Span::new(None, 10, 20));
    let in_stream = Data::At(Span::new(Some(ObjectId::new(4, 0)), 0, 7));
    let held = || stream(dict! { "Length" => 2 }, Data::Bytes(b"q\n".to_vec()));
    let font = Object::from(dict! { "Type" => "Font" });
    for data in [placed, in_stream, Data::Unplaced] {
        let unheld = || stream(Dict::new(), data.clone());
        let nested = [
            unheld(),
            Object::from(dict! { "Resources" => dict! { "Form" => unheld() } }),
            Object::from(vec![Object::from(1), Object::from(vec![unheld()])]),
            stream(dict! { "Inner" => unheld() }, Data::Bytes(Vec::new())),
        ];
        for (at, bad) in nested.into_iter().enumerate() {
            let mut objects = vec![font.clone(); at];
            objects.push(bad.clone());
            objects.push(held());
            let refused = Res::of(objects).unwrap_err();
            assert_eq!(refused, Unheld { object: at }, "{bad:?}");
            assert_eq!(
                refused.to_string(),
                format!("object {at} of the resource holds a stream whose data isn't in memory")
            );
            match Res::of([bad.clone()]) {
                Err(Unheld { object, .. }) => assert_eq!(object, 0),
                Ok(res) => panic!("{res:?}"),
            }
        }
        let dict = dict! { "Type" => "XObject", "S" => unheld() };
        assert_eq!(Res::dict(dict).unwrap_err(), Unheld { object: 0 });
        let body = ResBody::Objects(vec![font.clone(), unheld()].into());
        assert_eq!(Res::named(ResId(7), body), Err(Unheld { object: 1 }));
    }
    let taken = [
        held(),
        Object::from(dict! { "Resources" => dict! { "Form" => held() } }),
        Object::from(vec![Object::from(vec![held()])]),
        stream(dict! { "Inner" => held() }, Data::Bytes(Vec::new())),
    ];
    let res = Res::of(taken.to_vec()).unwrap();
    assert_eq!(res.id(), ResId::of(&taken));
    assert!(Res::dict(dict! { "S" => held() }).is_ok());
    // Named by hand: equal where the id and the objects are.
    let named = Res::named(res.id(), res.body().clone()).unwrap();
    assert_eq!(named, res);
    assert_ne!(Res::named(ResId(7), res.body().clone()).unwrap(), res);
    let written = Res::named(ResId(7), ResBody::Written(ObjectId::new(3, 0))).unwrap();
    assert_eq!(written.id(), ResId(7));
    assert!(matches!(written.body(), ResBody::Written(id) if *id == ObjectId::new(3, 0)));
}
