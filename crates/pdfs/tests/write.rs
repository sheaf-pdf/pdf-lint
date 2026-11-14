//! Documents written through the facade and read back: new ones, of a page
//! and of a thousand, what they draw with written once, claims, encryption,
//! linearized; files edited and saved whole, linearized and as updates; and
//! what a save refuses, with nothing written.

use std::collections::HashMap;
use std::sync::Arc;

use pdf_fixtures::fixture::{self, Body, Xref};
use pdfs::pdf_content_streams::{ContentWriter, Paint, Reserved};
use pdfs::pdf_crypt::writer::{Kind, Settings};
use pdfs::pdf_crypt::{Password, Permissions, Protection};
use pdfs::pdf_fonts::Std14;
use pdfs::pdf_linearize::family::{self, Rc4};
use pdfs::pdf_objects::version::Version;
use pdfs::pdf_objects::{Dict, Object, ObjectId, dict};
use pdfs::pdf_semantics::pages::{Inheritable, Origin, Rect};
use pdfs::pdf_semantics::xmp::{self, Claims};
use pdfs::pdf_structure::write::{self, Deflate, Unencryptable};
use pdfs::pdf_structure::{Encryption, Source as _};
use pdfs::{Document, Error, Pdf};

fn r(n: u32) -> Object {
    Object::reference(n)
}

const LETTER: Rect = Rect([0.0, 0.0, 612.0, 792.0]);

/// Page `i`'s text.
fn line(i: usize) -> String {
    format!("Page {}, d\u{e9}j\u{e0} vu \u{2014} \u{20ac}{i}", i + 1)
}

/// What page `i` draws: a square filled, and its line in Helvetica, which
/// every page draws with.
fn draw(page: &mut ContentWriter, i: usize) {
    page.rect(72.0, 72.0, 144.0, 144.0);
    page.paint(Paint::Fill);
    page.begin_text();
    page.font(pdfs::standard_font(Std14::Helvetica), 12.0);
    page.text_move(72.0, 700.0);
    page.show(&Std14::Helvetica.codes(&line(i)).unwrap());
    page.end_text();
}

/// A new document of `n` pages, each a box of its own and its own line.
fn new_document(n: usize) -> (Document<'static>, Vec<Vec<u8>>) {
    let mut doc = Document::new();
    let mut contents = Vec::new();
    for i in 0..n {
        let page = doc.add_page(Rect([0.0, 0.0, 300.0 + i as f64, 400.5]));
        draw(page, i);
        contents.push(page.named(&Reserved::default()).0);
    }
    (doc, contents)
}

/// `pdf`'s pages each as its box, its content and its text.
fn read_back<S: pdfs::pdf_structure::read::ByteSource>(
    pdf: &Pdf<S>,
) -> Vec<(Rect, Vec<u8>, String)> {
    let pages = pdf.pages().unwrap();
    let read = |page: pdfs::Page<'_, S>| {
        let content = page.content().unwrap().bytes().to_vec();
        let text = page.text().unwrap();
        assert!(text.is_whole());
        (page.attributes().media_box().value, content, text.text())
    };
    pages.iter().map(read).collect()
}

/// A new document of one page and of a thousand reads back page by page as
/// written: its box, its content and its text; and the font every page
/// draws with is one object.
#[test]
fn a_new_document_reads_back_as_written() {
    for n in [1, 1000] {
        let (mut doc, contents) = new_document(n);
        let file = doc.to_bytes().unwrap();
        assert!(file.starts_with(b"%PDF-1.7\n"));
        let pdf = pdfs::from_bytes(&file[..]).unwrap();
        let mut faults = Vec::new();
        assert_eq!(pdf.pages_with(&mut faults).unwrap().len(), n);
        assert!(faults.is_empty(), "{faults:?}");
        let read = read_back(&pdf);
        for (i, (media_box, content, text)) in read.into_iter().enumerate() {
            assert_eq!(media_box, Rect([0.0, 0.0, 300.0 + i as f64, 400.5]));
            assert_eq!(content, contents[i]);
            assert_eq!(text, line(i));
        }
        let pages = pdf.pages().unwrap();
        let fonts: Vec<_> = (pages.iter())
            .map(|page| page.fonts().unwrap()[0].font.as_ref().unwrap().clone())
            .map(|font| font.unwrap().id())
            .collect();
        assert!(fonts.iter().all(|&font| font == fonts[0]), "one object");
        // A file of a cross-reference stream, its objects in object streams,
        // and no fault said of its structure.
        let mut options = pdfs::Options::DEFAULT;
        options.faults = pdfs::pdf_structure::read::Faults::Kept;
        options.recovery = pdfs::pdf_structure::read::Recovery::Verified;
        options.rules = pdfs::pdf_tokenizer::Rules::Iso;
        let checked = options.from_bytes(&file[..]).unwrap();
        assert_eq!(checked.pages().unwrap().len(), n);
        assert_eq!(checked.faults(), []);
        assert!(checked.reading().trailer().stream.is_some());
    }
}

/// What two pages draw with is written once: a resource equal to one
/// written before is that object, through the facade and through
/// [`pdfs::build::resource`].
#[test]
fn a_resource_two_pages_draw_with_is_written_once() {
    let mut doc = Document::new();
    let props = pdfs::pdf_content_streams::Res::dict(dict! { "Lang" => Object::text("fr") });
    let props = props.unwrap();
    for _ in 0..2 {
        let page = doc.add_page(LETTER);
        page.begin_marked("Span", props.clone());
        page.end_marked();
    }
    let file = doc.to_bytes().unwrap();
    let pdf = pdfs::from_bytes(file).unwrap();
    let pages = pdf.pages().unwrap();
    let states: Vec<Object> = (pages.iter())
        .map(|page| {
            let resources = page.attributes().attribute(Inheritable::Resources).unwrap();
            let states = resources
                .value
                .as_dict()
                .unwrap()
                .get(b"Properties")
                .unwrap();
            let (_, state) = states.as_dict().unwrap().iter().next().unwrap();
            state.clone()
        })
        .collect();
    assert!(states[0].as_reference().is_some());
    assert_eq!(states[0].as_reference(), states[1].as_reference());

    let mut doc = pdfs::pdf_structure::Document::new(Version::new(1, 7), dict! {});
    let mut written = HashMap::new();
    let font = pdfs::standard_font(Std14::Courier);
    let one = pdfs::build::resource(&mut doc, &mut written, &font, Deflate::DEFAULT).unwrap();
    let two = pdfs::build::resource(&mut doc, &mut written, &font, Deflate::DEFAULT).unwrap();
    assert_eq!(one, two);
    assert_eq!(doc.objects().count(), 1);
}

/// A resource of objects is added with their references renumbered to the
/// numbers they're given, one past them `null`, and its streams deflated
/// where they name no filter.
#[test]
fn a_resources_objects_are_numbered_as_added() {
    let mut doc = pdfs::pdf_structure::Document::new(Version::new(1, 7), dict! {});
    doc.insert(1, dict! {});
    let form = pdfs::pdf_objects::object::Stream::new(
        dict! { "Subtype" => "Form", "Resources" => dict! { "Font" => dict! { "F" => ObjectId::new(1, 0) } }, "Gone" => ObjectId::new(7, 0) },
        b"BT /F 9 Tf (a) Tj ET".to_vec(),
    );
    let objects: Arc<[Object]> = Arc::new([Object::from(form), Object::from(Std14::Symbol.dict())]);
    let res = pdfs::pdf_content_streams::Res::of(objects).unwrap();
    let mut written = HashMap::new();
    let id = pdfs::build::resource(&mut doc, &mut written, &res, Deflate::DEFAULT).unwrap();
    assert_eq!(id, ObjectId::new(2, 0));
    let form = doc.get(id).unwrap().as_stream().unwrap();
    assert_eq!(
        form.dict.get(b"Filter").unwrap().as_name(),
        Some(&b"FlateDecode"[..])
    );
    let fonts = form.dict.get(b"Resources").unwrap().as_dict().unwrap();
    let font = fonts
        .get(b"Font")
        .unwrap()
        .as_dict()
        .unwrap()
        .get(b"F")
        .unwrap();
    assert_eq!(font.as_reference(), Some(ObjectId::new(3, 0)));
    assert!(form.dict.get(b"Gone").unwrap().is_null());
    use pdfs::pdf_objects::Streams as _;
    assert_eq!(
        &*doc.decoded(id, usize::MAX).unwrap(),
        b"BT /F 9 Tf (a) Tj ET"
    );
}

/// A page's `/Resources`, and a category of it, given by reference are
/// copied before a need is bound in them: the dictionaries other pages
/// share are never changed.
#[test]
fn a_shared_resources_dictionary_is_copied_never_changed() {
    let mut doc = pdfs::pdf_structure::Document::new(Version::new(1, 7), dict! {});
    let shared = dict! { "Font" => r(6), "ColorSpace" => dict! { "CS0" => "DeviceRGB" } };
    doc.insert(5, shared.clone());
    doc.insert(6, dict! { "F1" => r(7) });
    let mut page = ContentWriter::new();
    page.font(pdfs::standard_font(Std14::TimesRoman), 10.0);
    let (_, needs) = page.finish(&Reserved::default());
    let mut written = HashMap::new();
    let mut resources = r(5);
    pdfs::build::bind(
        &mut doc,
        &mut written,
        &needs,
        &mut resources,
        Deflate::DEFAULT,
    )
    .unwrap();
    let bound = resources.as_dict().unwrap();
    assert_eq!(bound.get(b"ColorSpace"), shared.get(b"ColorSpace"));
    let fonts = bound.get(b"Font").unwrap().as_dict().unwrap();
    assert_eq!(fonts.get(b"F1"), Some(&r(7)));
    assert_eq!(fonts.len(), 2);
    assert_eq!(doc.get_dict(ObjectId::new(5, 0)), Some(&shared));
    assert_eq!(doc.get_dict(ObjectId::new(6, 0)).unwrap().len(), 1);

    // With nothing to bind, a reference stays one.
    let mut resources = r(5);
    let none = Default::default();
    pdfs::build::bind(
        &mut doc,
        &mut written,
        &none,
        &mut resources,
        Deflate::DEFAULT,
    )
    .unwrap();
    assert_eq!(resources, r(5));
}

/// Claims are written as the catalog's metadata stream and read back; a
/// catalog that names one already is refused, nothing changed.
#[test]
fn claims_are_written_and_never_replace_a_packet() {
    let mut claims = Claims::default();
    claims.pdfa_part = Some("2".into());
    claims.pdfa_conformance = Some("B".into());
    let (mut doc, _) = new_document(1);
    doc.claim(&claims).unwrap();
    let file = doc.to_bytes().unwrap();
    let pdf = pdfs::from_bytes(&file[..]).unwrap();
    assert_eq!(pdf.claims().unwrap(), Some(claims.clone()));
    let metadata = pdf
        .catalog()
        .unwrap()
        .get(b"Metadata")
        .unwrap()
        .as_reference()
        .unwrap();
    let stream = pdf.reading().get(metadata).unwrap();
    let stream = stream.as_stream().unwrap();
    assert!(!stream.dict.has(b"Filter"));
    assert_eq!(
        stream.dict.get(b"Subtype").unwrap().as_name(),
        Some(&b"XML"[..])
    );

    // Again, refused: the catalog names a packet already.
    let before = doc.structure().objects().count();
    assert!(matches!(doc.claim(&claims), Err(Error::HasMetadata)));
    assert_eq!(doc.structure().objects().count(), before);
    let mut edited = pdf.edit().unwrap();
    assert!(matches!(edited.claim(&claims), Err(Error::HasMetadata)));

    // A claim that wouldn't read back is refused too.
    let (mut doc, _) = new_document(1);
    claims.pdfua_part = Some(" 1".into());
    assert!(matches!(
        doc.claim(&claims),
        Err(Error::Claims(xmp::WriteError::Spaced { .. }))
    ));
    assert!(!doc.structure().catalog().unwrap().has(b"Metadata"));
}

/// Saved twice, nothing changed between, a document writes the same bytes in
/// the clear; with a page added, a page tree over every page, its nodes'
/// numbers taken again.
#[test]
fn saved_twice_it_writes_the_same_bytes() {
    let (mut doc, _) = new_document(40);
    let first = doc.to_bytes().unwrap();
    assert_eq!(doc.to_bytes().unwrap(), first);
    let (mut again, _) = new_document(40);
    assert_eq!(again.to_bytes().unwrap(), first);
    draw(doc.add_page(LETTER), 40);
    let more = doc.to_bytes().unwrap();
    assert_ne!(more, first);
    let pdf = pdfs::from_bytes(&more[..]).unwrap();
    assert_eq!(pdf.pages().unwrap().len(), 41);
    let last = pdf.pages().unwrap().get(40).unwrap().text().unwrap().text();
    assert_eq!(last, line(40));
}

/// A document of no page is refused, nothing written.
#[test]
fn a_document_of_no_page_is_refused() {
    let mut doc = Document::new();
    let mut out = b"before".to_vec();
    let refused = doc.save_to(&mut out);
    assert!(matches!(
        refused,
        Err(Error::PageTree(
            pdfs::pdf_semantics::pages::WriteError::NoPages
        ))
    ));
    assert_eq!(out, b"before");
    let refused = doc.save_linearized_to(&mut out, &doc.linearize_options());
    assert!(matches!(refused, Err(Error::PageTree(_))));
    assert_eq!(out, b"before");
}

/// A catalog, a root over `pages` with `root`'s entries too, each a page of
/// the root's, and `more`, numbered from 1, the catalog 1, the root 2, the
/// pages from 3.
fn objects_of(pages: &[Dict], root: Dict, more: &[(u32, Body)]) -> Vec<(u32, Body)> {
    let kids: Vec<Object> = (0..pages.len() as u32).map(|i| r(3 + i)).collect();
    let mut node = dict! { "Type" => "Pages", "Kids" => kids, "Count" => pages.len() as i64 };
    for (key, value) in root.iter() {
        node.set(key, value.clone());
    }
    let mut objects = vec![
        (
            1,
            Body::from(dict! { "Type" => "Catalog", "Pages" => r(2) }),
        ),
        (2, Body::from(node)),
    ];
    for (i, page) in pages.iter().enumerate() {
        let mut page = page.clone();
        page.set("Type", "Page");
        page.set("Parent", r(2));
        objects.push((3 + i as u32, Body::from(page)));
    }
    objects.extend(more.iter().cloned());
    objects
}

/// A file of two pages under a root that gives them a box, a rotation and
/// resources.
fn two_pages(version: &str) -> Vec<u8> {
    let root = dict! {
        "MediaBox" => vec![Object::from(0), 0.into(), 612.into(), 792.into()],
        "CropBox" => vec![Object::from(10), 10.into(), 600.into(), 780.into()],
        "Rotate" => 90,
        "Resources" => dict! { "Font" => dict! { "F1" => r(9) } },
    };
    let pages = [dict! { "Contents" => r(10) }, dict! { "Contents" => r(11) }];
    let more = [
        (9, Body::from(Std14::Courier.dict())),
        (10, Body::stream(dict! {}, b"BT /F1 9 Tf (one) Tj ET")),
        (11, Body::stream(dict! {}, b"BT /F1 9 Tf (two) Tj ET")),
        (
            12,
            Body::from(dict! { "Title" => Object::text("Two pages") }),
        ),
    ];
    let objects = objects_of(&pages, root, &more);
    let trailer = dict! { "Root" => r(1), "Info" => r(12) };
    fixture::file(version, &objects, trailer, Xref::Table)
}

/// An unchanged edit's save is the bytes pdf-structure's writer writes of
/// the reading's document under the same options; an edited 2.0 file is
/// written 2.0, and an older one at least 1.5, as its streams need.
#[test]
fn an_edited_file_is_saved_as_its_document_is_written() {
    let file = two_pages("1.7");
    let pdf = pdfs::from_bytes(&file[..]).unwrap();
    let mut doc = pdf.edit().unwrap();
    let saved = doc.to_bytes().unwrap();
    let reading = pdf.reading();
    let direct = reading.to_document().unwrap();
    let mut written = Vec::new();
    let options = doc.save_options();
    write::document(&mut written, &direct, None, Some(reading), &options).unwrap();
    assert_eq!(saved, written);
    assert_eq!(read_texts(&saved), ["one", "two"]);

    for (version, header) in [("2.0", "2.0"), ("1.4", "1.5"), ("1.7", "1.7")] {
        let file = two_pages(version);
        let pdf = pdfs::from_bytes(&file[..]).unwrap();
        let saved = pdf.edit().unwrap().to_bytes().unwrap();
        let wanted = format!("%PDF-{header}\n");
        assert!(saved.starts_with(wanted.as_bytes()), "{version}");
        let linearized = pdf.edit().unwrap();
        let options = linearized.linearize_options();
        let mut out = Vec::new();
        let mut linearized = linearized;
        linearized.save_linearized_to(&mut out, &options).unwrap();
        assert!(out.starts_with(wanted.as_bytes()), "linearized {version}");
    }
    // A new document is 1.7.
    let (mut doc, _) = new_document(1);
    assert_eq!(doc.save_options().version, Version::new(1, 7));
    assert!(doc.to_bytes().unwrap().starts_with(b"%PDF-1.7\n"));
}

/// Each page's text in `file`.
fn read_texts(file: &[u8]) -> Vec<String> {
    let pdf = pdfs::from_bytes(file).unwrap();
    let pages = pdf.pages().unwrap();
    pages
        .iter()
        .map(|page| page.text().unwrap().text())
        .collect()
}

/// A page added to an edited file is a kid of its root with every
/// attribute a page inherits its own, so the root's give it nothing; the
/// root's `/Count` is one more.
#[test]
fn a_page_added_to_an_edited_file_inherits_nothing() {
    let file = two_pages("1.7");
    let pdf = pdfs::from_bytes(&file[..]).unwrap();
    let mut doc = pdf.edit().unwrap();
    draw(doc.add_page(Rect([0.0, 0.0, 200.0, 300.0])), 2);
    let saved = doc.to_bytes().unwrap();
    let pdf = pdfs::from_bytes(&saved[..]).unwrap();
    let mut faults = Vec::new();
    let pages = pdf.pages_with(&mut faults).unwrap();
    assert!(faults.is_empty(), "{faults:?}");
    assert_eq!(pages.len(), 3);
    assert_eq!(pages.counted(), 3);
    let added = pages.get(2).unwrap();
    let attributes = added.attributes();
    let small = Rect([0.0, 0.0, 200.0, 300.0]);
    assert_eq!(
        (attributes.media_box().value, attributes.media_box().origin),
        (small, Origin::Own)
    );
    assert_eq!(
        (attributes.crop_box().value, attributes.crop_box().origin),
        (small, Origin::Own)
    );
    assert_eq!(
        (attributes.rotate().value, attributes.rotate().origin),
        (0, Origin::Own)
    );
    let resources = attributes.attribute(Inheritable::Resources).unwrap();
    assert_eq!(resources.origin, Origin::Own);
    assert_eq!(added.text().unwrap().text(), line(2));
    // The pages read before are as they were.
    let first = pages.get(0).unwrap().attributes();
    assert_eq!(first.rotate().value, 90);
    assert_eq!(read_texts(&saved), ["one", "two", &line(2)]);
}

/// The path of a file a `Pdf` opened by its path reads by position is
/// refused, nothing written; one read into memory first is written over.
#[test]
fn a_save_never_writes_over_the_file_read_by_position() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("two.pdf");
    let file = two_pages("1.7");
    std::fs::write(&path, &file).unwrap();
    let pdf = pdfs::open(&path).unwrap();
    let mut doc = pdf.edit().unwrap();
    draw(doc.add_page(LETTER), 2);
    assert!(matches!(doc.save(&path), Err(Error::SameFile)));
    let options = doc.linearize_options();
    assert!(matches!(
        doc.save_linearized(&path, &options),
        Err(Error::SameFile)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), file);
    // Elsewhere it's written, and the file read is read as it was.
    let other = dir.path().join("three.pdf");
    doc.save(&other).unwrap();
    assert_eq!(read_texts(&std::fs::read(&other).unwrap()).len(), 3);
    assert_eq!(pdf.pages().unwrap().len(), 2);

    // Read into memory, the file is written over.
    let pdf = pdfs::from_bytes(std::fs::read(&path).unwrap()).unwrap();
    let mut doc = pdf.edit().unwrap();
    draw(doc.add_page(LETTER), 2);
    doc.save(&path).unwrap();
    assert_eq!(read_texts(&std::fs::read(&path).unwrap()).len(), 3);
}

/// Each kind of protection reads back by each password, with what it
/// grants; the empty password opens one with an owner's alone.
#[test]
fn a_document_is_encrypted_as_its_protection_says() {
    let user = Password("user").prepared().unwrap();
    let owner = Password("owner").prepared().unwrap();
    let mut no_print = Protection::new(&user);
    no_print.owner = Some(&owner);
    no_print.permissions = Permissions::ALL.without(Permissions::PRINT);
    let mut owner_only = Protection::DEFAULT;
    owner_only.owner = Some(&owner);
    let mut metadata_clear = Protection::new(&user);
    metadata_clear.encrypt_metadata = false;
    // Each protection, the passwords that open it, each with whether it's the
    // owner's, and what it grants: one password alone is the owner's too.
    let all = Permissions::ALL;
    let cases = [
        (Protection::new(&user), [(Some("user"), true); 2], all),
        (
            no_print,
            [(Some("user"), false), (Some("owner"), true)],
            all.without(Permissions::PRINT),
        ),
        (owner_only, [(None, false), (Some("owner"), true)], all),
        (metadata_clear, [(Some("user"), true); 2], all),
    ];
    for (protection, passwords, granted) in cases {
        let (mut doc, contents) = new_document(2);
        let mut claims = Claims::default();
        claims.pdfua_part = Some("1".into());
        doc.claim(&claims).unwrap();
        doc.encrypt(&protection).unwrap();
        let file = doc.to_bytes().unwrap();
        let opened = |password: Option<&str>| {
            let given: Vec<Password> = password.map(Password).into_iter().collect();
            let mut options = pdfs::Options::DEFAULT;
            options.passwords = &given;
            options.from_bytes(file.clone()).unwrap()
        };
        if passwords[0].0.is_some() {
            let locked = opened(None);
            assert_eq!(*locked.encryption(), Encryption::Locked);
            let pages = locked.pages();
            assert!(pages.is_err() || pages.unwrap().get(0).unwrap().content().is_err());
        }
        for (password, is_owner) in passwords {
            let pdf = opened(password);
            let Encryption::Opened {
                owner,
                permissions,
                revision,
                ..
            } = *pdf.encryption()
            else {
                panic!("{:?}", pdf.encryption());
            };
            assert_eq!((owner, permissions), (is_owner, granted), "{password:?}");
            // Revision 6, PDF 2.0's, under the document's own version.
            assert_eq!(revision, 6);
            assert_eq!(pdf.reading().header_version(), Some(Version::new(1, 7)));
            let read = read_back(&pdf);
            assert_eq!(read.len(), 2);
            assert_eq!(read[1].1, contents[1]);
            assert_eq!(read[1].2, line(1));
            assert_eq!(pdf.claims().unwrap(), Some(claims.clone()));
        }
        // In the clear again, it has no /Encrypt.
        doc.decrypt();
        let clear = doc.to_bytes().unwrap();
        let pdf = pdfs::from_bytes(&clear[..]).unwrap();
        assert_eq!(*pdf.encryption(), Encryption::None);
        assert!(!pdf.reading().trailer().dict().has(b"Encrypt"));
    }
    // No password is refused, and nothing changes.
    let (mut doc, _) = new_document(1);
    assert!(matches!(
        doc.encrypt(&Protection::DEFAULT),
        Err(Error::Protection(_))
    ));
    let pdf = pdfs::from_bytes(doc.to_bytes().unwrap()).unwrap();
    assert_eq!(*pdf.encryption(), Encryption::None);
}

/// The settings of a file encrypted by `kind` with `user` and `owner`.
fn settings(kind: Kind) -> Settings<'static> {
    Settings {
        kind,
        user: b"user",
        owner: b"owner",
        p: -4,
        id: b"0123456789abcdef",
        encrypt_metadata: true,
    }
}

/// [`two_pages`]' objects, encrypted by `kind`.
fn two_pages_encrypted(kind: Kind) -> Vec<u8> {
    let root = dict! { "MediaBox" => vec![Object::from(0), 0.into(), 612.into(), 792.into()] };
    let pages = [dict! { "Contents" => r(10) }, dict! { "Contents" => r(11) }];
    let more = [
        (10, Body::stream(dict! {}, b"0 0 m 9 9 l S")),
        (11, Body::stream(dict! {}, b"1 1 m 8 8 l S")),
        (
            12,
            Body::from(dict! { "Title" => Object::text("Two pages") }),
        ),
    ];
    let objects = objects_of(&pages, root, &more);
    let trailer = dict! { "Root" => r(1), "Info" => r(12) };
    fixture::encrypted(&settings(kind), &objects, trailer)
}

/// The passwords' encodings, to open a file by.
fn opened_by(file: &[u8], password: &str) -> Pdf<Vec<u8>> {
    let given = [Password(password)];
    let mut options = pdfs::Options::DEFAULT;
    options.passwords = &given;
    options.from_bytes(file.to_vec()).unwrap()
}

/// An edited encrypted file is saved under its own encryption unless told
/// otherwise, whole, linearized and as an update, each read back by its
/// password; decrypted, in the clear.
#[test]
fn an_edited_encrypted_file_keeps_its_encryption() {
    let file = two_pages_encrypted(Kind::Aes256);
    let pdf = opened_by(&file, "user");
    let mut doc = pdf.edit().unwrap();
    draw(doc.add_page(LETTER), 2);
    let saved = doc.to_bytes().unwrap();
    let mut linearized = Vec::new();
    let options = doc.linearize_options();
    doc.save_linearized_to(&mut linearized, &options).unwrap();
    let mut update = Vec::new();
    doc.save_update_to(&mut update).unwrap().unwrap();
    let appended = [&file[..], &update].concat();
    for written in [&saved, &linearized, &appended] {
        let locked = pdfs::from_bytes(&written[..]).unwrap();
        assert_eq!(*locked.encryption(), Encryption::Locked);
        let pdf = opened_by(written, "user");
        assert!(matches!(pdf.encryption(), Encryption::Opened { .. }));
        let read = read_back(&pdf);
        assert_eq!(read.len(), 3);
        assert_eq!(read[2].2, line(2));
    }
    doc.decrypt();
    let clear = doc.to_bytes().unwrap();
    assert_eq!(
        *pdfs::from_bytes(&clear[..]).unwrap().encryption(),
        Encryption::None
    );
    assert_eq!(read_texts(&clear).len(), 3);
    // An update can't change a file's encryption.
    let mut out = Vec::new();
    let refused = doc.save_update_to(&mut out);
    assert!(
        matches!(refused, Err(Error::Write(write::Error::Unmatched))),
        "{refused:?}"
    );
    assert!(out.is_empty());

    // A locked file isn't edited: there's nothing to write it from.
    let locked = pdfs::from_bytes(&file[..]).unwrap();
    assert!(matches!(
        locked.edit(),
        Err(Error::Write(write::Error::Locked))
    ));
}

/// Under an RC4 file's own encryption, what would put new plaintext under
/// a keystream the file used is refused unless the caller says so, as a
/// linearized file and as an update, and what's so written is listed.
#[test]
fn rc4_keystreams_are_reused_only_when_asked() {
    let file = two_pages_encrypted(Kind::Rc4By128);
    let pdf = opened_by(&file, "user");

    // Linearized, objects move to numbers others had.
    let mut doc = pdf.edit().unwrap();
    let mut options = doc.linearize_options();
    let mut out = Vec::new();
    let refused = doc.save_linearized_to(&mut out, &options);
    let keystream = |error: &Error| match error {
        Error::Linearize(family::Error::Write(write::Error::Encrypt { why, .. })) => {
            matches!(why, Unencryptable::Keystream)
        }
        _ => false,
    };
    assert!(refused.as_ref().is_err_and(keystream), "{refused:?}");
    assert!(out.is_empty());
    // The options' `rc4` is pdf-linearize's for a file's bytes, and isn't
    // read here: the document's encryptor is told.
    options.rc4 = Rc4::ReusingKeystreams;
    let refused = doc.save_linearized_to(&mut out, &options);
    assert!(refused.as_ref().is_err_and(keystream), "{refused:?}");
    assert!(out.is_empty());
    doc.reuse_rc4_keystreams();
    let linearized = doc.save_linearized_to(&mut out, &options).unwrap();
    assert!(!linearized.keystreams_reused.is_empty());
    assert_eq!(read_back(&opened_by(&out, "user")).len(), 2);

    // As an update, a string changed at its number.
    let mut doc = pdf.edit().unwrap();
    let info = doc
        .structure_mut()
        .get_dict_mut(ObjectId::new(12, 0))
        .unwrap();
    info.set("Title", Object::text("Another title"));
    let mut out = Vec::new();
    let refused = doc.save_update_to(&mut out);
    let keystream = matches!(
        &refused,
        Err(Error::Write(write::Error::Encrypt {
            why: Unencryptable::Keystream,
            ..
        }))
    );
    assert!(keystream, "{refused:?}");
    assert!(out.is_empty());
    doc.reuse_rc4_keystreams();
    let written = doc.save_update_to(&mut out).unwrap().unwrap();
    assert_eq!(written.keystreams_reused, [ObjectId::new(12, 0)]);
    let updated = opened_by(&[&file[..], &out].concat(), "user");
    let info = updated.info().unwrap().unwrap();
    assert_eq!(info.title.unwrap().to_string(), "Another title");

    // Written whole, nothing goes under a keystream used: nothing to ask.
    let mut doc = pdf.edit().unwrap();
    let written = doc.save_to(&mut Vec::new()).unwrap();
    assert!(written.keystreams_reused.is_empty());
}

/// Linearized, a document's linearization holds and is judged whole.
#[test]
fn a_linearized_document_holds() {
    let (mut doc, _) = new_document(5);
    let options = doc.linearize_options();
    let mut out = Vec::new();
    let linearized = doc.save_linearized_to(&mut out, &options).unwrap();
    assert!(linearized.keystreams_reused.is_empty());
    let pdf = pdfs::from_bytes(&out[..]).unwrap();
    let mut findings = Vec::new();
    let verdict = pdf.linearized_with(&mut findings).unwrap();
    assert!(
        verdict.linearized && verdict.holds && verdict.judged_whole,
        "{findings:?}"
    );
    assert!(findings.is_empty(), "{findings:?}");
    assert_eq!(read_texts(&out).len(), 5);
    // Through a path too.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("linearized.pdf");
    doc.save_linearized(&path, &options).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), out);
}

/// An edited file saved as an update: appended to the file read, and to a
/// copy of it, the same update; refused, nothing written, after a file
/// that changed and to one that never held it; and `save_update_to`'s
/// bytes the update alone.
#[test]
fn an_update_is_appended_only_to_the_file_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("two.pdf");
    let file = two_pages("1.7");
    std::fs::write(&path, &file).unwrap();
    let copy = dir.path().join("copy.pdf");
    std::fs::copy(&path, &copy).unwrap();
    let changed = dir.path().join("changed.pdf");
    std::fs::write(&changed, [&file[..], b"\n%"].concat()).unwrap();
    let other = dir.path().join("other.pdf");
    let (mut new, _) = new_document(1);
    let new_file = new.to_bytes().unwrap();
    std::fs::write(&other, &new_file).unwrap();

    let pdf = pdfs::open(&path).unwrap();
    let mut doc = pdf.edit().unwrap();
    // Nothing changed: nothing written.
    assert!(doc.save_update(&path).unwrap().is_none());
    let mut out = Vec::new();
    assert!(doc.save_update_to(&mut out).unwrap().is_none());
    assert!(out.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), file);

    draw(doc.add_page(LETTER), 2);
    let alone = doc.save_update_to(&mut out).unwrap().unwrap();
    // A copy of the file takes it, and the file read.
    assert!(doc.save_update(&copy).unwrap().is_some());
    let updated = std::fs::read(&copy).unwrap();
    assert_eq!(updated, [&file[..], &out].concat());
    assert_eq!(read_texts(&updated), ["one", "two", &line(2)]);
    assert_eq!(alone.base.len, updated.len());
    assert!(doc.save_update(&path).unwrap().is_some());
    assert_eq!(std::fs::read(&path).unwrap(), updated);
    // The file read reads on as it was where it was; it holds the update
    // now, so a second is refused, and so is a write from its reading, which
    // changed under it.
    assert_eq!(pdf.pages().unwrap().len(), 2);
    assert!(matches!(doc.save_update(&path), Err(Error::NotTheBase)));
    assert_eq!(std::fs::read(&path).unwrap(), updated);
    let refused = doc.save_to(&mut Vec::new());
    assert!(
        matches!(refused, Err(Error::Write(write::Error::Changed(None)))),
        "{refused:?}"
    );
    // A file changed since, and another file, are left as they are.
    for (refused, held) in [(&changed, [&file[..], b"\n%"].concat()), (&other, new_file)] {
        assert!(matches!(doc.save_update(refused), Err(Error::NotTheBase)));
        assert_eq!(std::fs::read(refused).unwrap(), held);
    }
    // A path that holds nothing isn't made.
    let missing = dir.path().join("missing.pdf");
    assert!(matches!(doc.save_update(&missing), Err(Error::Io(_))));
    assert!(!missing.exists());

    // A new document has no file to go after.
    assert!(matches!(
        new.save_update_to(&mut Vec::new()),
        Err(Error::NotRead)
    ));
    assert!(matches!(new.save_update(&path), Err(Error::NotRead)));
}

/// A page can't be added to an edited file whose catalog names no page tree
/// node to add it under: refused, nothing written.
#[test]
fn a_page_needs_a_page_tree_to_go_under() {
    let file = fixture::file(
        "1.7",
        &[
            (
                1,
                Body::from(dict! { "Type" => "Catalog", "Pages" => r(2) }),
            ),
            (
                2,
                Body::from(dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => "one" }),
            ),
            (3, Body::from(dict! { "Type" => "Page", "Parent" => r(2) })),
        ],
        dict! { "Root" => r(1) },
        Xref::Table,
    );
    let pdf = pdfs::from_bytes(&file[..]).unwrap();
    let mut doc = pdf.edit().unwrap();
    doc.add_page(LETTER);
    let mut out = Vec::new();
    assert!(matches!(doc.save_to(&mut out), Err(Error::NoPageTree)));
    assert!(out.is_empty());
}

/// A file whose root's `/Count` can't count a page more within 64 bits:
/// refused before anything is written in, never wrapped; one less, and the
/// page is counted onto it.
#[test]
fn a_count_that_cant_count_another_page_is_refused() {
    let file = |count: i64| {
        let objects = objects_of(&[dict! {}], dict! { "Count" => count }, &[]);
        fixture::file("1.7", &objects, dict! { "Root" => r(1) }, Xref::Table)
    };
    let full = file(i64::MAX);
    let pdf = pdfs::from_bytes(&full[..]).unwrap();
    let mut doc = pdf.edit().unwrap();
    doc.add_page(LETTER);
    let mut out = Vec::new();
    assert!(matches!(doc.save_to(&mut out), Err(Error::NoPageTree)));
    assert!(out.is_empty());
    let root = doc.structure().get_dict(ObjectId::new(2, 0)).unwrap();
    assert_eq!(
        root.get(b"Count").and_then(Object::as_integer),
        Some(i64::MAX)
    );
    assert_eq!(
        root.get(b"Kids").and_then(Object::as_array).map(<[_]>::len),
        Some(1)
    );

    let room = file(i64::MAX - 1);
    let pdf = pdfs::from_bytes(&room[..]).unwrap();
    let mut doc = pdf.edit().unwrap();
    doc.add_page(LETTER);
    doc.save_to(&mut out).unwrap();
    let root = doc.structure().get_dict(ObjectId::new(2, 0)).unwrap();
    assert_eq!(
        root.get(b"Count").and_then(Object::as_integer),
        Some(i64::MAX)
    );
}

/// An edited file's linearized save walks its trees under the caps the file
/// was read under: a page tree deeper than they let is refused, as its own
/// pages are, and under the defaults it's written.
#[test]
fn a_linearized_save_walks_under_the_files_caps() {
    // A page under five nodes, each the only kid of the one above it.
    let mut objects = vec![(
        1,
        Body::from(dict! { "Type" => "Catalog", "Pages" => r(2) }),
    )];
    for n in 2..=6u32 {
        let node = dict! { "Type" => "Pages", "Kids" => vec![r(n + 1)], "Count" => 1 };
        let mut node = node;
        if n > 2 {
            node.set("Parent", r(n - 1));
        }
        objects.push((n, Body::from(node)));
    }
    let page = dict! { "Type" => "Page", "Parent" => r(6), "MediaBox" => vec![Object::from(0), 0.into(), 9.into(), 9.into()] };
    objects.push((7, Body::from(page)));
    let file = fixture::file("1.7", &objects, dict! { "Root" => r(1) }, Xref::Table);

    let mut options = pdfs::Options::DEFAULT;
    options.limits.semantics.page_depth = 2;
    let shallow = options.from_bytes(&file[..]).unwrap();
    assert!(matches!(shallow.pages(), Err(Error::Pages(_))));
    let mut doc = shallow.edit().unwrap();
    let linearize = doc.linearize_options();
    assert_eq!(linearize.limits.page_depth, 2);
    let mut out = Vec::new();
    let refused = doc.save_linearized_to(&mut out, &linearize);
    assert!(
        matches!(refused, Err(Error::Linearize(family::Error::Pages(_)))),
        "{refused:?}"
    );
    assert!(out.is_empty());

    let pdf = pdfs::from_bytes(&file[..]).unwrap();
    let mut doc = pdf.edit().unwrap();
    let linearize = doc.linearize_options();
    assert_eq!(linearize.limits, pdfs::pdf_linearize::Limits::DEFAULT);
    doc.save_linearized_to(&mut out, &linearize).unwrap();
    assert_eq!(read_texts(&out).len(), 1);
}

/// A page's box that isn't finite is written as the content writer writes
/// such a number, 0, so every save writes; and a resource of no objects is
/// never made, as no object would be its own.
#[test]
fn a_box_that_isnt_finite_is_written_as_0() {
    let mut doc = Document::new();
    doc.add_page(Rect([0.0, f64::NAN, f64::INFINITY, 792.0]));
    let file = doc.to_bytes().unwrap();
    assert_eq!(doc.to_bytes().unwrap(), file);
    let pdf = pdfs::from_bytes(&file[..]).unwrap();
    let pages = pdf.pages().unwrap();
    let page = pages.get(0).unwrap().attributes().dict.unwrap();
    let written: Vec<_> = (page
        .get(b"MediaBox")
        .and_then(Object::as_array)
        .unwrap()
        .iter())
    .map(Object::as_integer)
    .collect();
    assert_eq!(written, [Some(0), Some(0), Some(0), Some(792)]);
    let none = pdfs::pdf_content_streams::Res::of(Vec::new());
    assert_eq!(none, Err(pdfs::pdf_content_streams::ResError::Empty));
}

/// No `Debug` holds a page's text or what a document holds.
#[test]
fn no_debug_holds_what_a_document_holds() {
    let mut doc = Document::new();
    let page = doc.add_page(LETTER);
    page.show(b"MARKERTEXT");
    let shown = format!("{doc:?}");
    assert!(!shown.contains("MARKER"), "{shown}");
    let refused = Std14::Helvetica.codes("MARKER\u{4e2d}").unwrap_err();
    assert!(!format!("{:?}", Error::SameFile).contains("MARKER"));
    assert!(!refused.to_string().contains("MARKER"));
}

/// A document is `Send` and `Sync`.
#[test]
fn a_document_crosses_threads() {
    fn shared<T: Send + Sync>() {}
    shared::<Document<'static>>();
    shared::<Document<'_>>();
    let (mut doc, _) = new_document(3);
    let file = std::thread::spawn(move || doc.to_bytes().unwrap())
        .join()
        .unwrap();
    assert_eq!(read_texts(&file).len(), 3);
}
