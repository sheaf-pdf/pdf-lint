use std::collections::BTreeMap;

use pdf_fixtures::fixture::{self, Body, Xref};
use pdf_objects::{Dict, Form, dict};
use pdf_structure::pdf_crypt::Password;
use pdf_structure::pdf_crypt::writer::{Kind, Settings};
use pdf_structure::read::{Options, Reading};

use super::*;
use crate::date::Error as DateError;

/// Keeps what it's told.
#[derive(Default)]
struct Kept(Vec<(Place, Fault)>);

impl Sink<Fault> for Kept {
    fn fault(&mut self, at: Place, fault: Fault) {
        self.0.push((at, fault));
    }
}

impl Kept {
    fn faults(&self) -> Vec<Fault> {
        self.0.iter().map(|(_, fault)| *fault).collect()
    }
}

/// Wants no faults, and fails if it's told one.
struct Unwanted;

impl Sink<Fault> for Unwanted {
    fn fault(&mut self, _: Place, fault: Fault) {
        panic!("{fault} told to a sink that wants none");
    }

    fn wants(&self) -> bool {
        false
    }
}

/// A string of `bytes`, as a reader of a file gives it.
fn bytes(bytes: &[u8]) -> Object {
    Object::new(Value::String(
        Str::Bytes(bytes.to_vec()),
        Some(Form::Literal),
    ))
}

fn id(number: u32) -> ObjectId {
    ObjectId::new(number, 0)
}

/// What a read gave, and what it told.
type Told = (Result<Option<Info>, Unreadable>, Vec<(Place, Fault)>);

/// What `info` reads as through `objects`, by `rules`, and what it told; read
/// alike with no sink and with one that wants none.
fn read_by<'o>(objects: &'o BTreeMap<ObjectId, Object>, info: &'o Object, rules: Rules) -> Told {
    let resolve = |id: ObjectId| objects.get(&id);
    let mut kept = Kept::default();
    let read = read(resolve, info, rules, &mut kept);
    assert_eq!(super::read(resolve, info, rules, &mut ()), read);
    assert_eq!(super::read(resolve, info, rules, &mut Unwanted), read);
    (read, kept.0)
}

/// [`read_by`] by ISO's rules, whose answer is had.
fn read_iso(objects: &BTreeMap<ObjectId, Object>, info: &Object) -> (Option<Info>, Vec<Fault>) {
    let (read, told) = read_by(objects, info, Rules::Iso);
    (
        read.unwrap(),
        told.into_iter().map(|(_, fault)| fault).collect(),
    )
}

/// Objects `n` of `values`, each its number.
fn held<const N: usize>(values: [(u32, Object); N]) -> BTreeMap<ObjectId, Object> {
    values
        .into_iter()
        .map(|(n, value)| (id(n), value))
        .collect()
}

/// Every entry of Table 349 read as typed, nothing told.
#[test]
fn every_entry_is_read() {
    let info = dict! {
        "Title" => Object::text("A title"),
        "Author" => Object::text("An author"),
        "Subject" => Object::text("A subject"),
        "Keywords" => Object::text("a, b"),
        "Creator" => Object::text("A word processor"),
        "Producer" => Object::text("A PDF writer"),
        "CreationDate" => Object::text("D:20261112093000+01'00"),
        "ModDate" => Object::text("D:2026"),
        "Trapped" => "Unknown",
        "Other" => Object::text("not read"),
    };
    let objects = held([(4, info.into())]);
    let (read, told) = read_iso(&objects, &Object::reference(4));
    let read = read.unwrap();
    assert_eq!(read.title.as_deref(), Some("A title"));
    assert_eq!(read.author.as_deref(), Some("An author"));
    assert_eq!(read.subject.as_deref(), Some("A subject"));
    assert_eq!(read.keywords.as_deref(), Some("a, b"));
    assert_eq!(read.creator.as_deref(), Some("A word processor"));
    assert_eq!(read.producer.as_deref(), Some("A PDF writer"));
    let created = read.created.as_ref().and_then(Dated::date).unwrap();
    assert_eq!(created.write(), "D:20261112093000+01'00");
    let modified = read.modified.as_ref().and_then(Dated::date).unwrap();
    assert_eq!((modified.year(), modified.month()), (2026, None));
    assert_eq!(read.trapped, Some(Trapped::Unknown));
    assert_eq!(told, []);
}

/// An empty dictionary, null and a reference to nothing: each none or
/// nothing, and nothing told.
#[test]
fn nothing_there_is_none() {
    let objects = held([(4, Object::from(Dict::new())), (5, Object::null())]);
    let (read, told) = read_iso(&objects, &Object::reference(4));
    assert_eq!((read, told), (Some(Info::default()), vec![]));
    for absent in [Object::null(), Object::reference(5), Object::reference(9)] {
        assert_eq!(read_iso(&objects, &absent), (None, vec![]));
    }
    // An entry that's null, or a reference to nothing, is absent (7.3.7).
    let nulls = dict! { "Title" => Object::null(), "CreationDate" => Object::reference(9) };
    let (read, told) = read_iso(&objects, &nulls.into());
    assert_eq!((read, told), (Some(Info::default()), vec![]));
}

/// An entry by reference is followed, through a chain, and its faults are
/// placed in the object it's in.
#[test]
fn a_reference_is_followed() {
    let info = dict! {
        "Title" => Object::reference(5),
        "Trapped" => Object::reference(6),
        "ModDate" => Object::reference(7),
    };
    let objects = held([
        (4, info.into()),
        (5, Object::reference(8)),
        (6, "True".into()),
        (7, Object::text("D:202613")),
        (8, Object::text("By way of two")),
    ]);
    let (read, told) = read_by(&objects, &Object::reference(4), Rules::Iso);
    let read = read.unwrap().unwrap();
    assert_eq!(read.title.as_deref(), Some("By way of two"));
    assert_eq!(read.trapped, Some(Trapped::True));
    assert_eq!(read.modified, Some(Dated::Text("D:202613".into())));
    let wrong = DateError::OutOfRange {
        field: crate::date::Field::Month,
    };
    assert_eq!(Date::read("D:202613"), Err(wrong));
    let [(place, fault)] = told.as_slice() else {
        panic!("{told:?}")
    };
    assert_eq!(
        *fault,
        Fault::InfoDate {
            key: "ModDate",
            error: wrong
        }
    );
    assert_eq!(place.object, Some(id(7)));
}

/// A date that reads is a date, its quirks kept; one that doesn't is kept as
/// its text and told, why with it.
#[test]
fn a_date_that_does_not_read_is_kept_as_written() {
    let info = dict! {
        "CreationDate" => Object::text("D:20261112093000-08'00'"),
        "ModDate" => Object::text("12 November 2026"),
    };
    let objects = held([(4, info.into())]);
    let (read, told) = read_by(&objects, &Object::reference(4), Rules::Iso);
    let read = read.unwrap().unwrap();
    let created = read.created.as_ref().and_then(Dated::date).unwrap();
    assert_eq!(
        created.quirks(),
        [crate::date::Quirk::TerminatingApostrophe]
    );
    assert_eq!(read.modified, Some(Dated::Text("12 November 2026".into())));
    assert_eq!(read.modified.as_ref().and_then(Dated::date), None);
    let fault = Fault::InfoDate {
        key: "ModDate",
        error: DateError::NoPrefix,
    };
    assert_eq!(told.iter().map(|(_, f)| *f).collect::<Vec<_>>(), [fault]);
    assert_eq!(fault.to_string(), "/ModDate isn't a date: a date begins D:");
    assert_eq!(told[0].0.object, Some(id(4)));
    assert!(told[0].0.at.is_none(), "a value built has no span");
}

/// A text string in each encoding, decoded by each rules as pdf-objects
/// decodes it; little-endian UTF-16, which PDF hasn't, read apart.
#[test]
fn a_text_string_is_decoded_by_the_rules() {
    let strings: [&[u8]; 6] = [
        b"Caf\xe9",
        b"\xfe\xff\x00C\x00a\x00f\x00\xe9",
        b"\xef\xbb\xbfCaf\xc3\xa9",
        b"\xff\xfeC\x00a\x00f\x00\xe9\x00",
        b"\xfe\xff\x00\x1b\x00e\x00n\x00\x1b\x00C\x00a\x00f\x00\xe9",
        b"\xff\xfe",
    ];
    for string in strings {
        let objects = held([(4, dict! { "Title" => bytes(string) }.into())]);
        for rules in [Rules::Iso, Rules::Pdfium] {
            let (read, told) = read_by(&objects, &Object::reference(4), rules);
            let title = read.unwrap().unwrap().title.unwrap();
            assert_eq!(title, text::decode_with(string, rules).text, "{string:?}");
            assert_eq!(told, [], "{string:?}");
        }
    }
    let objects = held([(4, dict! { "Title" => bytes(strings[3]) }.into())]);
    let title = |rules| {
        read_by(&objects, &Object::reference(4), rules)
            .0
            .unwrap()
            .unwrap()
            .title
    };
    assert_eq!(title(Rules::Pdfium).as_deref(), Some("Café"));
    assert_eq!(title(Rules::Iso).as_deref(), Some("ÿþC\0a\0f\0é\0"));
    // A string a reader gave as text is that text.
    let given = Object::new(Value::String(Str::Text("Café".into()), None));
    let objects = held([(4, dict! { "Title" => given }.into())]);
    let (read, _) = read_iso(&objects, &Object::reference(4));
    assert_eq!(read.unwrap().title.as_deref(), Some("Café"));
}

/// An entry of another type is told and read as none; so is a `/Trapped`
/// that isn't one of the three names, a boolean among them.
#[test]
fn what_is_not_its_type_is_told() {
    let info = dict! {
        "Title" => "ANameNotAString",
        "Author" => 5,
        "CreationDate" => Object::from(Dict::new()),
        "Trapped" => true,
    };
    let objects = held([(4, info.into())]);
    let (read, told) = read_iso(&objects, &Object::reference(4));
    assert_eq!(read, Some(Info::default()));
    let said = [
        Fault::InfoNotText { key: "Title" },
        Fault::InfoNotText { key: "Author" },
        Fault::InfoNotText {
            key: "CreationDate",
        },
        Fault::InfoTrapped,
    ];
    assert_eq!(told, said);
    assert_eq!(
        said[0].to_string(),
        "/Title of the document information dictionary isn't a text string"
    );
    assert_eq!(
        said[3].to_string(),
        "/Trapped isn't /True, /False or /Unknown"
    );
    for (name, trapped) in [("True", Trapped::True), ("False", Trapped::False)] {
        let objects = held([(4, dict! { "Trapped" => name }.into())]);
        let (read, told) = read_iso(&objects, &Object::reference(4));
        assert_eq!((read.unwrap().trapped, told), (Some(trapped), vec![]));
    }
    let objects = held([(4, dict! { "Trapped" => "Partly" }.into())]);
    let (read, told) = read_iso(&objects, &Object::reference(4));
    assert_eq!(
        (read.unwrap().trapped, told),
        (None, vec![Fault::InfoTrapped])
    );
}

/// What isn't a dictionary is told and read as none, in the object it is.
#[test]
fn an_info_that_is_not_a_dictionary_is_told() {
    let stream = Object::stream(dict! { "Title" => Object::text("a stream") });
    let objects = held([(4, Object::from(5)), (5, stream)]);
    for number in [4, 5] {
        let (read, told) = read_by(&objects, &Object::reference(number), Rules::Iso);
        assert_eq!(read, Ok(None));
        assert_eq!(told.len(), 1);
        assert_eq!(told[0].1, Fault::InfoNotDictionary);
        assert_eq!(told[0].0.object, Some(id(number)));
    }
    let direct = Object::text("in place");
    let (read, told) = read_by(&objects, &direct, Rules::Iso);
    assert_eq!(read, Ok(None));
    assert_eq!(told[0].0.object, None, "written in place, in no object");
}

/// An object that doesn't read is one that isn't there, told; one past a
/// cap, behind a password or not given fails the read, as does a chain too
/// long and a string given as ciphertext. None is an empty `Info`.
#[test]
fn what_does_not_read_for_the_reader_fails_the_read() {
    /// Object 4 its `/Info`, and any other that doesn't read, for `why`.
    struct Failing(Object, Why);
    impl<'o> Resolve<'o> for &'o Failing {
        fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
            self.try_resolve(id).ok().flatten()
        }

        fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
            match id.number {
                4 => Ok(Some(&self.0)),
                _ => Err(Unreadable::new(id, self.1, "it doesn't read")),
            }
        }
    }
    let info = dict! { "Title" => Object::reference(5), "Author" => Object::text("kept") };
    for why in [Why::Damaged, Why::Capped, Why::Locked, Why::Source] {
        let resolver = Failing(info.clone().into(), why);
        let mut kept = Kept::default();
        let read = read(&resolver, &Object::reference(4), Rules::Iso, &mut kept);
        match why {
            Why::Damaged => {
                let read = read.unwrap().unwrap();
                assert_eq!((read.title, read.author.as_deref()), (None, Some("kept")));
                assert_eq!(kept.faults(), [Fault::Damaged { id: id(5) }]);
                assert_eq!(kept.0[0].0.object, Some(id(4)));
            }
            _ => {
                let error = read.unwrap_err();
                assert_eq!((error.id, error.why), (id(5), why));
                assert_eq!(kept.faults(), []);
            }
        }
    }
    // A loop of references: past HOPS, capped.
    let objects = held([(4, Object::reference(5)), (5, Object::reference(4))]);
    let (read, _) = read_by(&objects, &Object::reference(4), Rules::Iso);
    assert_eq!(read.unwrap_err().why, Why::Capped);
    // Ciphertext, in a file no password opened.
    let encrypted = Object::new(Value::String(Str::Encrypted(b"x".to_vec()), None));
    let objects = held([(4, dict! { "Producer" => encrypted }.into())]);
    let (read, told) = read_by(&objects, &Object::reference(4), Rules::Iso);
    let error = read.unwrap_err();
    assert_eq!((error.id, error.why), (id(4), Why::Locked));
    assert_eq!(
        error.message,
        "4 0 R holds /Producer as ciphertext: it takes a password"
    );
    assert_eq!(told, []);
}

/// A file's `/Info` read through its reading: as written in the clear;
/// decrypted where its password opened it; and locked where none did, its
/// strings given as ciphertext. (One in an object stream that takes the
/// password doesn't read, which fails the read as any locked object does,
/// above.)
#[test]
fn a_file_s_info_is_read_as_its_reader_gives_it() {
    let info = dict! {
        "Title" => Object::text("Plain or not"),
        "CreationDate" => Object::text("D:20261112"),
    };
    let objects = vec![
        (1, Body::from(dict! { "Type" => "Catalog" })),
        (4, Body::from(info)),
    ];
    let trailer = dict! { "Root" => Object::reference(1), "Info" => Object::reference(4) };
    let read_file = |file: &[u8], passwords: &[Password]| {
        let mut options = Options::DEFAULT;
        options.passwords = passwords;
        let reading = Reading::open(file, &options).unwrap();
        let info = reading.trailer().dict().get(b"Info").unwrap().clone();
        let mut kept = Kept::default();
        let read = read(&reading, &info, Rules::Iso, &mut kept);
        (
            read.map(|info| info.and_then(|info| info.title)),
            kept.faults(),
        )
    };
    let clear = fixture::file("1.7", &objects, trailer.clone(), Xref::Table);
    let title = Some("Plain or not".to_string());
    assert_eq!(read_file(&clear, &[]), (Ok(title.clone()), vec![]));
    let settings = Settings {
        kind: Kind::Aes128,
        user: b"user",
        owner: b"owner",
        p: -4,
        id: b"0123456789abcdef",
        encrypt_metadata: true,
    };
    let encrypted = fixture::encrypted(&settings, &objects, trailer);
    assert_eq!(
        read_file(&encrypted, &[Password("user")]),
        (Ok(title), vec![])
    );
    let (locked, told) = read_file(&encrypted, &[]);
    let locked = locked.unwrap_err();
    assert_eq!((locked.id, locked.why, told), (id(4), Why::Locked, vec![]));
}

/// An entry's text, decrypted or not, is never in a `Debug`: only its
/// length.
#[test]
fn a_debug_holds_no_text() {
    const MARK: &str = "a marker no debug shows";
    let info = dict! {
        "Title" => Object::text(MARK),
        "Author" => Object::text(MARK),
        "Subject" => Object::text(MARK),
        "Keywords" => Object::text(MARK),
        "Creator" => Object::text(MARK),
        "Producer" => Object::text(MARK),
        "CreationDate" => Object::text(MARK),
        "ModDate" => Object::text("D:2026"),
        "Trapped" => "True",
    };
    let objects = held([(4, info.into())]);
    let read = read_iso(&objects, &Object::reference(4)).0.unwrap();
    assert_eq!(read.created, Some(Dated::Text(MARK.into())));
    let shown = format!("{read:?} {:?} {read:#?}", read.created);
    assert!(!shown.contains("marker"), "{shown}");
    assert!(shown.contains("23 bytes"), "{shown}");
    assert!(shown.contains("year: 2026"), "{shown}");
}
