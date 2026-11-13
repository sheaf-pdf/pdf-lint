//! Files read through the facade, written by pdf-fixtures and pdf-structure's
//! writer: what each call gives under the defaults and under options set,
//! encrypted files opened and unlocked, damaged and capped ones, and what a
//! call refuses.

use std::borrow::Cow;
use std::io;
use std::ops::Range;

use pdf_fixtures::fixture::{self, Body, Xref};
use pdfs::pdf_content_streams::{Pushed, TooLarge};
use pdfs::pdf_crypt::writer::{Kind, Settings};
use pdfs::pdf_crypt::{Encoded, Password, Protection, Refused};
use pdfs::pdf_filters::Backend;
use pdfs::pdf_linearize::family;
use pdfs::pdf_objects::fault::Place;
use pdfs::pdf_objects::object::Stream;
use pdfs::pdf_objects::version::Version;
use pdfs::pdf_objects::{Dict, Object, ObjectId, Why, dict};
use pdfs::pdf_semantics::info::{Dated, Trapped};
use pdfs::pdf_semantics::pages::{Inheritable, Origin, Rect};
use pdfs::pdf_semantics::xmp::{self, Claims};
use pdfs::pdf_structure::read::{ByteSource, Faults, Identity, Recovery};
use pdfs::pdf_structure::write::{self, Encryptor, Pack};
use pdfs::pdf_structure::{Document, Encryption, Source as _, XrefKind};
use pdfs::pdf_tokenizer::Rules;
use pdfs::{Error, Limits, Options};

fn r(n: u32) -> Object {
    Object::reference(n)
}

fn content_stream(data: &str) -> Body {
    Body::stream(dict! {}, data.as_bytes())
}

/// A catalog, a root over `pages`, each a page of the root's, the root's
/// `/MediaBox` 612 by 792, and `more` beside them; numbered from 1, the
/// catalog 1, the root 2, the pages from 3.
fn file_of(version: &str, pages: &[Dict], more: &[(u32, Body)], catalog: Dict) -> Vec<u8> {
    let kids: Vec<Object> = (0..pages.len() as u32).map(|i| r(3 + i)).collect();
    let mut objects = vec![
        (1, Body::from(catalog)),
        (
            2,
            Body::from(dict! {
                "Type" => "Pages", "Kids" => kids, "Count" => pages.len() as i64,
                "MediaBox" => vec![Object::from(0), 0.into(), 612.into(), 792.into()],
            }),
        ),
    ];
    for (i, page) in pages.iter().enumerate() {
        let mut page = page.clone();
        page.set("Type", "Page");
        page.set("Parent", r(2));
        objects.push((3 + i as u32, Body::from(page)));
    }
    objects.extend(more.iter().cloned());
    fixture::file(version, &objects, dict! { "Root" => r(1) }, Xref::Table)
}

fn catalog() -> Dict {
    dict! { "Type" => "Catalog", "Pages" => r(2) }
}

/// One page drawing stream 10, and object 10.
fn one_page(content: &str) -> Vec<u8> {
    let page = dict! { "Contents" => r(10) };
    file_of("1.7", &[page], &[(10, content_stream(content))], catalog())
}

/// The defaults are a viewer's: PDFium's rules and recovery, zlib-rs, no
/// fault looked for, each crate's caps, no password.
#[test]
fn the_defaults_are_a_viewers() {
    let options = Options::DEFAULT;
    assert_eq!(options.rules, Rules::Pdfium);
    assert_eq!(options.recovery, Recovery::Pdfium);
    assert_eq!(options.flate, Backend::ZlibRs);
    assert_eq!(options.faults, Faults::Off);
    assert_eq!(options.limits, Limits::DEFAULT);
    assert!(options.passwords.is_empty());
    let pdf = pdfs::from_bytes(one_page("0 0 m")).unwrap();
    assert_eq!(pdf.rules(), Rules::Pdfium);
    assert_eq!(*pdf.limits(), Limits::DEFAULT);
    assert!(pdf.identity().is_none());
}

/// A page's `/Contents` joined whole, a space between each stream, an
/// element that's no stream an empty one in its place; and its resources
/// inherited, which its content's inline images are read with.
#[test]
fn a_pages_streams_are_joined_whole() {
    let page = dict! { "Contents" => vec![r(10), r(12), r(11)] };
    let more = [
        (10, content_stream("q 1 0 0 rg")),
        (11, content_stream("0 0 9 9 re f Q")),
        (12, Body::from(dict! { "Not" => "a stream" })),
    ];
    let pdf = pdfs::from_bytes(file_of("1.7", &[page], &more, catalog())).unwrap();
    let pages = pdf.pages().unwrap();
    let content = pages.get(0).unwrap().content().unwrap();
    assert_eq!(content.bytes(), b"q 1 0 0 rg  0 0 9 9 re f Q");
    assert_eq!(content.concat().stream_count(), 3);
    let operators: Vec<Vec<u8>> = (content.operations())
        .map(|operation| operation.unwrap().name().to_vec())
        .collect();
    assert_eq!(operators, [&b"q"[..], b"rg", b"re", b"f", b"Q"]);

    // A single stream, its resources' color space inherited from the root.
    let resources = dict! { "ColorSpace" => dict! { "CS0" => "DeviceCMYK" } };
    let root = dict! {
        "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1, "Resources" => resources,
    };
    let objects = [
        (1, Body::from(catalog())),
        (2, Body::from(root)),
        (
            3,
            Body::from(dict! { "Type" => "Page", "Parent" => r(2), "Contents" => r(4) }),
        ),
        (4, content_stream("BI /W 1 /H 1 /BPC 8 /CS /CS0 ID abcd EI")),
    ];
    let file = fixture::file("1.7", &objects, dict! { "Root" => r(1) }, Xref::Table);
    let pdf = pdfs::from_bytes(file).unwrap();
    let pages = pdf.pages().unwrap();
    let page = pages.get(0).unwrap();
    let inherited = page.attributes().attribute(Inheritable::Resources).unwrap();
    assert_eq!(
        inherited.origin,
        Origin::Ancestor(Some(ObjectId::new(2, 0)))
    );
    let content = page.content().unwrap();
    assert_eq!(content.concat().stream_count(), 1);
    let image = content.operations().next().unwrap().unwrap();
    let image = image.inline_image().unwrap();
    assert_eq!(&content.bytes()[image.data()], b"abcd");

    // No /Contents: no stream.
    let pdf = pdfs::from_bytes(file_of("1.7", &[dict! {}], &[], catalog())).unwrap();
    let content = pdf.pages().unwrap().get(0).unwrap().content().unwrap();
    assert_eq!(
        (content.bytes(), content.concat().stream_count()),
        (&b""[..], 0)
    );
}

/// Pages walked by `next` and by `fold` alike, counted, from the back, and
/// by index; each with its attributes and where they came from.
#[test]
fn pages_are_walked_alike_every_way() {
    let pages: Vec<Dict> = (0..5)
        .map(|i| match i {
            2 => dict! { "Rotate" => 90 },
            _ => dict! {},
        })
        .collect();
    let pdf = pdfs::from_bytes(file_of("1.7", &pages, &[], catalog())).unwrap();
    let table = pdf.pages().unwrap();
    assert_eq!(
        (table.len(), table.counted(), table.is_empty()),
        (5, 5, false)
    );
    let ids = |page: pdfs::Page<'_, Vec<u8>>| page.attributes().id.unwrap().number;
    let mut iter = table.iter();
    assert_eq!(iter.len(), 5);
    // By `next` alone, as `from_fn` asks for each.
    let by_next: Vec<u32> = std::iter::from_fn(|| iter.next()).map(ids).collect();
    assert_eq!(iter.len(), 0);
    let by_fold = table.iter().fold(Vec::new(), |mut all, page| {
        all.push(ids(page));
        all
    });
    assert_eq!(by_next, [3, 4, 5, 6, 7]);
    assert_eq!(by_fold, by_next);
    assert_eq!(table.iter().count(), 5);
    assert_eq!(
        table.iter().rev().map(ids).collect::<Vec<_>>(),
        [7, 6, 5, 4, 3]
    );
    assert_eq!(table.iter().nth(3).map(ids), Some(6));
    assert_eq!((&table).into_iter().map(ids).collect::<Vec<_>>(), by_next);
    assert!(table.get(5).is_none());
    let third = table.get(2).unwrap().attributes();
    assert_eq!(
        (third.rotate().value, third.rotate().origin),
        (90, Origin::Own)
    );
    let media = third.media_box();
    assert_eq!(media.value, Rect([0.0, 0.0, 612.0, 792.0]));
    assert_eq!(media.origin, Origin::Ancestor(Some(ObjectId::new(2, 0))));
    assert_eq!(table.table().len(), 5);
}

/// What departs from the clause is told to the sink a `_with` form is given,
/// by ISO's rules everything; the plain form tells nothing.
#[test]
fn faults_are_told_to_the_sink_given() {
    // A root without /Count, and a page without /Type.
    let objects = [
        (1, Body::from(catalog())),
        (
            2,
            Body::from(dict! { "Type" => "Pages", "Kids" => vec![r(3)] }),
        ),
        (3, Body::from(dict! { "Parent" => r(2) })),
    ];
    let file = fixture::file("1.7", &objects, dict! { "Root" => r(1) }, Xref::Table);
    let mut options = Options::DEFAULT;
    options.rules = Rules::Iso;
    let pdf = options.from_bytes(&file[..]).unwrap();
    let mut faults = Vec::new();
    let told = pdf.pages_with(&mut faults).unwrap();
    assert_eq!(told.len(), 1);
    assert!(faults.len() >= 2, "{faults:?}");
    assert_eq!(pdf.pages().unwrap().len(), 1);

    // The reading's own, kept where they're looked for, none by default.
    let mut damaged = file.clone();
    damaged.truncate(damaged.len() - b"%%EOF\n".len());
    let quiet = pdfs::from_bytes(&damaged[..]).unwrap();
    assert!(quiet.pages().is_ok());
    assert!(quiet.faults().is_empty());
    let mut options = Options::DEFAULT;
    options.faults = Faults::Kept;
    let kept = options.from_bytes(&damaged[..]).unwrap();
    assert!(!kept.faults().is_empty());
}

/// A file whose `startxref` is a few bytes off is read as PDFium reads it by
/// default, scanned; and only where it proves it is, the section near the
/// offset, under `Recovery::Verified`. Both read the page.
#[test]
fn a_damaged_file_reads_as_pdfium_reads_it_or_as_verified() {
    let file = one_page("0 0 m 9 9 l S");
    let at = file.windows(5).position(|w| w == b"xref\n").unwrap();
    let mut damaged = file[..at].to_vec();
    damaged.extend_from_slice(b"junk\n");
    damaged.extend_from_slice(&file[at..]);
    let pdf = pdfs::from_bytes(&damaged[..]).unwrap();
    assert!(pdf.reading().trailer().unwritten.is_some(), "scanned");
    let content = pdf.pages().unwrap().get(0).unwrap().content().unwrap();
    assert_eq!(content.bytes(), b"0 0 m 9 9 l S");

    let mut options = Options::DEFAULT;
    options.recovery = Recovery::Verified;
    options.faults = Faults::Kept;
    let pdf = options.from_bytes(&damaged[..]).unwrap();
    assert!(pdf.reading().trailer().unwritten.is_none(), "found near");
    let moved = (pdf.faults().iter())
        .any(|(_, fault)| matches!(fault, pdfs::pdf_structure::Fault::Moved { .. }));
    assert!(moved, "{:?}", pdf.faults());
    let content = pdf.pages().unwrap().get(0).unwrap().content().unwrap();
    assert_eq!(content.bytes(), b"0 0 m 9 9 l S");
}

/// A stream past the reader's cap is capped, never damaged and never read
/// as none; streams joined past the content's cap are too large; each `Why`
/// kept.
#[test]
fn a_stream_past_a_cap_says_so() {
    let file = one_page("0 0 m 9 9 l S");
    let mut options = Options::DEFAULT;
    options.limits.read.stream = 4;
    let pdf = options.from_bytes(&file[..]).unwrap();
    let refused = pdf.pages().unwrap().get(0).unwrap().content().unwrap_err();
    match &refused {
        Error::Content(Pushed::Decode(unreadable)) => {
            assert_eq!(unreadable.why, Why::Capped);
            assert_eq!(unreadable.id, ObjectId::new(10, 0));
        }
        other => panic!("{other:?}"),
    }
    match pdf.stream(ObjectId::new(10, 0)).unwrap_err() {
        Error::Unreadable(unreadable) => assert_eq!(unreadable.why, Why::Capped),
        other => panic!("{other:?}"),
    }
    let whole = pdfs::from_bytes(&file[..]).unwrap();
    assert_eq!(
        whole.stream(ObjectId::new(10, 0)).unwrap(),
        &b"0 0 m 9 9 l S"[..]
    );
    match whole.stream(ObjectId::new(3, 0)).unwrap_err() {
        Error::Unreadable(unreadable) => assert_eq!(unreadable.why, Why::Damaged),
        other => panic!("{other:?}"),
    }

    let mut options = Options::DEFAULT;
    options.limits.content.content = 4;
    let pdf = options.from_bytes(&file[..]).unwrap();
    let refused = pdf.pages().unwrap().get(0).unwrap().content().unwrap_err();
    assert!(
        matches!(
            refused,
            Error::Content(Pushed::TooLarge(TooLarge { cap: 4 }))
        ),
        "{refused:?}"
    );
    assert_eq!(
        refused.to_string(),
        "its streams decode to more than 4 bytes"
    );
}

/// The settings of a file encrypted under AES-256 by `user` and `owner`.
fn aes256() -> Settings<'static> {
    Settings {
        kind: Kind::Aes256,
        user: b"user",
        owner: b"owner",
        p: -4,
        id: b"0123456789abcdef",
        encrypt_metadata: true,
    }
}

/// One page, a title in `/Info`, encrypted.
fn encrypted() -> Vec<u8> {
    let objects = [
        (1, Body::from(catalog())),
        (
            2,
            Body::from(dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1 }),
        ),
        (
            3,
            Body::from(dict! { "Type" => "Page", "Parent" => r(2), "Contents" => r(4) }),
        ),
        (4, content_stream("0 0 m 9 9 l S")),
        (5, Body::from(dict! { "Title" => Object::text("A title") })),
    ];
    fixture::encrypted(
        &aes256(),
        &objects,
        dict! { "Root" => r(1), "Info" => r(5) },
    )
}

/// An encrypted file opened by no password is locked: its page's content
/// and its title are refused as locked, never read as ciphertext. Opened by
/// its password, or unlocked by its bytes after, it reads; `by` is the
/// index among those given.
#[test]
fn an_encrypted_file_opens_by_its_password_or_unlocks_after() {
    let file = encrypted();
    let locked = pdfs::from_bytes(&file[..]).unwrap();
    assert_eq!(*locked.encryption(), Encryption::Locked);
    let pages = locked.pages().unwrap();
    match pages.get(0).unwrap().content().unwrap_err() {
        Error::Content(Pushed::Decode(unreadable)) => assert_eq!(unreadable.why, Why::Locked),
        other => panic!("{other:?}"),
    }
    match locked.info().unwrap_err() {
        Error::Unreadable(unreadable) => assert_eq!(unreadable.why, Why::Locked),
        other => panic!("{other:?}"),
    }

    let given = [Password("wrong"), Password("user")];
    let mut options = Options::DEFAULT;
    options.passwords = &given;
    let pdf = options.from_bytes(&file[..]).unwrap();
    let Encryption::Opened { owner, by, .. } = pdf.encryption() else {
        panic!("{:?}", pdf.encryption());
    };
    assert!(!owner);
    assert_eq!(by.map(|(index, _)| index), Some(1));
    let content = pdf.pages().unwrap().get(0).unwrap().content().unwrap();
    assert_eq!(content.bytes(), b"0 0 m 9 9 l S");
    assert_eq!(
        pdf.info().unwrap().unwrap().title.as_deref(),
        Some("A title")
    );

    // Refused by a wrong one, given back locked; unlocked by the owner's,
    // second of three given.
    let (locked, refused) = locked.unlock(&[Encoded::verbatim(b"wrong")]).unwrap_err();
    assert_eq!(refused, Refused::Password);
    assert_eq!(*locked.encryption(), Encryption::Locked);
    let tried = [
        Encoded::verbatim(b"wrong"),
        Encoded::verbatim(b"owner"),
        Encoded::verbatim(b"user"),
    ];
    let unlocked = locked.unlock(&tried).unwrap();
    let Encryption::Opened { owner, by, .. } = unlocked.encryption() else {
        panic!("{:?}", unlocked.encryption());
    };
    assert!(owner);
    assert_eq!(by.map(|(index, _)| index), Some(1));
    let pages = unlocked.pages().unwrap();
    let content = pages.get(0).unwrap().content().unwrap();
    assert_eq!(content.bytes(), b"0 0 m 9 9 l S");
    assert_eq!(
        unlocked.info().unwrap().unwrap().title.as_deref(),
        Some("A title")
    );
}

/// A page tree in an object stream of an encrypted file no password opened
/// is an error, its `Why` locked, never a file of no pages.
#[test]
fn a_locked_page_tree_is_an_error_never_no_pages() {
    let mut doc = Document::new(Version::new(1, 7), dict! { "Root" => r(1) });
    doc.insert(1, catalog());
    doc.insert(
        2,
        dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1 },
    );
    doc.insert(
        3,
        dict! { "Type" => "Page", "Parent" => r(2), "Contents" => r(4) },
    );
    let stream = Stream::new(dict! {}, b"0 0 m".to_vec());
    doc.insert(
        4,
        Object::new(pdfs::pdf_objects::Value::Stream(Box::new(stream))),
    );
    let password = Password("user").prepared().unwrap();
    let encryptor = Encryptor::create(&Protection::new(&password)).unwrap();
    // The page tree's root alone in an object stream; the catalog on its own.
    let groups = |packed: &[u32]| vec![packed.iter().copied().filter(|&n| n == 2).collect()];
    let mut pack = Pack::DEFAULT;
    pack.groups = Some(&groups);
    let mut options = write::Options::DEFAULT;
    options.kind = XrefKind::Stream;
    options.pack = Some(pack);
    let mut file = Vec::new();
    write::document(&mut file, &doc, Some(&encryptor), None, &options).unwrap();

    let locked = pdfs::from_bytes(&file[..]).unwrap();
    assert!(locked.catalog().is_ok());
    match locked.pages().unwrap_err() {
        Error::Pages(pdfs::pdf_semantics::pages::Error::Unreadable(unreadable)) => {
            assert_eq!(unreadable.why, Why::Locked);
            assert_eq!(unreadable.id, ObjectId::new(2, 0));
        }
        other => panic!("{other:?}"),
    }
    let unlocked = locked.unlock(&Password("user").encodings()).unwrap();
    let pages = unlocked.pages().unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages.get(0).unwrap().content().unwrap().bytes(), b"0 0 m");
}

/// The version is the later of the header's and the catalog's, by number:
/// `/1.10` is later than `1.9`; an earlier `/Version` is passed over.
#[test]
fn the_version_is_the_later_by_number() {
    let version = |header: &str, named: Option<&str>| {
        let mut root = catalog();
        if let Some(named) = named {
            root.set("Version", Object::from(named));
        }
        let file = file_of(header, &[dict! {}], &[], root);
        pdfs::from_bytes(file).unwrap().version().unwrap()
    };
    assert_eq!(version("1.9", Some("1.10")), Some(Version::new(1, 10)));
    assert_eq!(version("1.7", Some("1.4")), Some(Version::new(1, 7)));
    assert_eq!(version("1.4", Some("2.0")), Some(Version::new(2, 0)));
    assert_eq!(version("1.7", None), Some(Version::new(1, 7)));
    assert_eq!(version("1.7", Some("2")), Some(Version::new(1, 7)));
}

/// What a file says of itself and claims, read typed; a date that doesn't
/// read is told where it's written, and kept as its text.
#[test]
fn what_a_file_says_and_claims() {
    let mut claims = Claims::default();
    claims.pdfa_part = Some("2".into());
    claims.pdfa_conformance = Some("B".into());
    let packet = xmp::write(&claims).unwrap();
    let metadata = Body::stream(
        dict! { "Type" => "Metadata", "Subtype" => "XML" },
        packet.as_bytes(),
    );
    let info = dict! {
        "Title" => Object::text("A title"),
        "CreationDate" => Object::text("D:20261112093000Z"),
        "ModDate" => Object::text("last Tuesday"),
        "Trapped" => "False",
    };
    let mut root = catalog();
    root.set("Metadata", r(10));
    let objects = [
        (1, Body::from(root)),
        (
            2,
            Body::from(dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1 }),
        ),
        (3, Body::from(dict! { "Type" => "Page", "Parent" => r(2) })),
        (10, metadata),
        (11, Body::from(info)),
    ];
    let trailer = dict! { "Root" => r(1), "Info" => r(11) };
    let file = fixture::file("1.7", &objects, trailer, Xref::Table);
    let pdf = pdfs::from_bytes(&file[..]).unwrap();
    let read = pdf.info().unwrap().unwrap();
    assert_eq!(read.title.as_deref(), Some("A title"));
    assert_eq!(
        read.created
            .as_ref()
            .and_then(Dated::date)
            .map(|date| date.year()),
        Some(2026)
    );
    assert_eq!(read.modified, Some(Dated::Text("last Tuesday".into())));
    assert_eq!(read.trapped, Some(Trapped::False));
    let mut faults: Vec<(Place, pdfs::pdf_semantics::Fault)> = Vec::new();
    assert_eq!(pdf.info_with(&mut faults).unwrap(), Some(read));
    assert_eq!(faults.len(), 1, "{faults:?}");
    assert_eq!(faults[0].0.object, Some(ObjectId::new(11, 0)));

    let claimed = pdf.claims().unwrap().unwrap();
    assert_eq!(claimed, claims);
    let mut faults: Vec<(Place, pdfs::pdf_semantics::Fault)> = Vec::new();
    assert_eq!(pdf.claims_with(&mut faults).unwrap(), Some(claims));
    assert!(faults.is_empty(), "{faults:?}");

    // None where there's none.
    let plain = pdfs::from_bytes(one_page("")).unwrap();
    assert_eq!(plain.info().unwrap(), None);
    assert_eq!(plain.claims().unwrap(), None);
}

/// A linearized file's linearization holds and is judged whole; one whose
/// hint stream no password opened holds as far as it's judged, and says it
/// wasn't judged whole.
#[test]
fn a_linearized_file_is_checked_and_what_is_locked_is_not_judged() {
    let pages: Vec<Dict> = (0..3)
        .map(|i| dict! { "Contents" => r(10 + i), "Resources" => dict! {} })
        .collect();
    let more: Vec<(u32, Body)> = (0..3).map(|i| (10 + i, content_stream("0 0 m"))).collect();
    let plain = file_of("1.7", &pages, &more, catalog());
    let not = pdfs::from_bytes(&plain[..]).unwrap().linearized().unwrap();
    assert!(!not.linearized && !not.holds);

    let file = family::linearize_bytes(&plain, &family::Options::DEFAULT).unwrap();
    let pdf = pdfs::from_bytes(&file[..]).unwrap();
    let verdict = pdf.linearized().unwrap();
    assert!(
        verdict.linearized && verdict.holds && verdict.judged_whole,
        "{verdict:?}"
    );
    let mut findings = Vec::new();
    assert_eq!(pdf.linearized_with(&mut findings).unwrap(), verdict);
    assert!(findings.is_empty(), "{findings:?}");

    let objects: Vec<(u32, Body)> = {
        let reading = pdfs::from_bytes(&plain[..]).unwrap().into_reading();
        let doc = reading.to_document().unwrap();
        (doc.ids().into_iter())
            .map(|id| (id.number, Body::from(doc.get(id).unwrap().clone())))
            .collect()
    };
    let encrypted = fixture::encrypted(&aes256(), &objects, dict! { "Root" => r(1) });
    let user = [Password("user")];
    let mut options = family::Options::DEFAULT;
    options.read.passwords = &user;
    let file = family::linearize_bytes(&encrypted, &options).unwrap();
    let locked = pdfs::from_bytes(&file[..]).unwrap();
    let verdict = locked.linearized().unwrap();
    assert!(verdict.linearized && verdict.holds, "{verdict:?}");
    assert!(!verdict.hints_read && !verdict.judged_whole, "{verdict:?}");
    let opened = locked.unlock(&Password("user").encodings()).unwrap();
    let verdict = opened.linearized().unwrap();
    assert!(
        verdict.holds && verdict.hints_read && verdict.judged_whole,
        "{verdict:?}"
    );
}

/// A source that doesn't give its bytes, read by position.
struct Failing(usize);

impl ByteSource for Failing {
    fn len(&self) -> usize {
        self.0
    }

    fn read_at(&self, _: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
        Err(io::Error::other("the disk is gone"))
    }
}

/// A source that doesn't give its bytes is an error, never a panic.
#[test]
fn a_source_that_fails_is_an_error() {
    let refused = pdfs::from_bytes(Failing(4096)).unwrap_err();
    assert!(matches!(refused, Error::Read(_)), "{refused:?}");
    assert!(
        refused.to_string().contains("the disk is gone"),
        "{refused}"
    );
}

/// A file opened by its path is read by position, held open, and known by
/// its identity; a path that isn't there is an I/O error.
#[test]
fn a_file_opened_by_its_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("one.pdf");
    std::fs::write(&path, one_page("0 0 m 9 9 l S")).unwrap();
    let pdf = pdfs::open(&path).unwrap();
    assert_eq!(pdf.identity(), Some(&Identity::of(&path).unwrap()));
    let content = pdf.pages().unwrap().get(0).unwrap().content().unwrap();
    assert_eq!(content.bytes(), b"0 0 m 9 9 l S");
    let refused = pdfs::open(dir.path().join("none.pdf")).unwrap_err();
    assert!(
        matches!(&refused, Error::Io(e) if e.kind() == io::ErrorKind::NotFound),
        "{refused:?}"
    );
}

/// A file read on many threads at once: `Pdf` is `Send` and `Sync`.
#[test]
fn a_file_is_read_on_many_threads() {
    fn shared<T: Send + Sync>(_: &T) {}
    let pages: Vec<Dict> = (0..8).map(|i| dict! { "Contents" => r(20 + i) }).collect();
    let more: Vec<(u32, Body)> = (0..8)
        .map(|i| (20 + i, content_stream(&format!("{i} {i} m"))))
        .collect();
    let pdf = pdfs::from_bytes(file_of("1.7", &pages, &more, catalog())).unwrap();
    shared(&pdf);
    let read: Vec<Vec<u8>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let pdf = &pdf;
                scope.spawn(move || {
                    let pages = pdf.pages().unwrap();
                    pages.get(i).unwrap().content().unwrap().bytes().to_vec()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for (i, bytes) in read.iter().enumerate() {
        assert_eq!(bytes, format!("{i} {i} m").as_bytes());
    }
}

/// No `Debug` holds a file's text or a password: each gives lengths,
/// counts and numbers.
#[test]
fn no_debug_holds_a_files_text_or_a_password() {
    const MARKER: &str = "Zebrafish";
    let page = dict! {
        "Type" => "Page", "Parent" => r(2), "Contents" => r(10),
        "PieceInfo" => Object::text(MARKER),
    };
    let objects = [
        (1, Body::from(catalog())),
        (
            2,
            Body::from(dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1 }),
        ),
        (3, Body::from(page)),
        (10, content_stream(&format!("({MARKER}) Tj"))),
        (11, Body::from(dict! { "Title" => Object::text(MARKER) })),
    ];
    let trailer = dict! { "Root" => r(1), "Info" => r(11), "Marker" => Object::text(MARKER) };
    let file = fixture::file("1.7", &objects, trailer, Xref::Table);
    let given = [Password(MARKER)];
    let mut options = Options::DEFAULT;
    options.passwords = &given;
    let pdf = options.from_bytes(&file[..]).unwrap();
    let pages = pdf.pages().unwrap();
    let page = pages.get(0).unwrap();
    let content = page.content().unwrap();
    assert!(
        content
            .bytes()
            .windows(MARKER.len())
            .any(|w| w == MARKER.as_bytes())
    );
    let info = pdf.info().unwrap().unwrap();
    assert_eq!(info.title.as_deref(), Some(MARKER));
    let error = Error::Content(Pushed::TooLarge(TooLarge { cap: 1 }));
    let debugs = [
        format!("{options:?}"),
        format!("{pdf:?}"),
        format!("{pages:?}"),
        format!("{page:?}"),
        format!("{:?}", pages.iter()),
        format!("{content:?}"),
        format!("{info:?}"),
        format!("{error:?}"),
        format!("{:?}", pdf.limits()),
    ];
    for debug in debugs {
        assert!(!debug.contains(MARKER), "{debug}");
    }
}
