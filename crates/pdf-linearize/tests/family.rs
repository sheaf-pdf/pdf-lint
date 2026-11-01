//! What the family adapter writes, read back with pdf-structure's reader: what it
//! changes of a document, where each thing goes, and what it refuses. Files are
//! written by hand, a marker in an object saying where it landed.

#![cfg(feature = "family")]

use std::sync::atomic::AtomicBool;

use pdf_linearize::family::{self, Error, FirstPage, Linearized, Options, PageTree};
use pdf_linearize::{Limits, hint, plan};
use pdf_objects::object::{Object, ObjectId, Value};
use pdf_objects::version::Version;
use pdf_semantics::Rules;
use pdf_semantics::pages::{Inheritable, Origin, pages};
use pdf_structure::bytes::{self, Reading};
use pdf_structure::{Document, Source as _, XrefKind, write};

mod written;
use written::{ROOT, built, file, kinds, stream, two_pages};

fn table() -> Options<'static> {
    Options::DEFAULT
}

/// `bytes` linearized under `options`, and what's said of it.
fn linearized(bytes: &[u8], options: &Options<'_>) -> Result<(Vec<u8>, Linearized), Error> {
    let reading = Reading::open(bytes, &bytes::Options::DEFAULT).map_err(Error::Read)?;
    let doc = reading.to_document().map_err(Error::Read)?;
    let mut out = Vec::new();
    let done = family::linearize(&mut out, &doc, None, Some(&reading), options)?;
    Ok((out, done))
}

fn written(bytes: &[u8], options: &Options<'_>) -> Vec<u8> {
    linearized(bytes, options).unwrap().0
}

/// Where `text` is in `bytes`: it's there once.
fn at(bytes: &[u8], text: &str) -> u64 {
    let text = text.as_bytes();
    let mut found = bytes.windows(text.len()).enumerate();
    let (first, _) = (found.find(|(_, w)| *w == text)).unwrap_or_else(|| {
        panic!("{} isn't in the file", String::from_utf8_lossy(text));
    });
    assert!(
        !found.any(|(_, w)| w == text),
        "{} is in the file twice",
        String::from_utf8_lossy(text)
    );
    first as u64
}

/// A reading of `bytes`, and its document, to read back.
fn read(bytes: &[u8]) -> (Reading<&[u8]>, Document) {
    let reading = Reading::open(bytes, &bytes::Options::DEFAULT).unwrap();
    let doc = reading.to_document().unwrap();
    (reading, doc)
}

/// The dictionary `id`'s object holds, read back.
fn dict_at(doc: &Document, id: ObjectId) -> &pdf_objects::object::Dict {
    doc.get_dict(id).unwrap()
}

/// The object in `doc` that holds `/Mark (text)`.
fn marked(doc: &Document, text: &str) -> ObjectId {
    let id = doc.objects().find(|(_, object)| {
        let dict = match object.value() {
            Value::Dict(dict) => dict,
            Value::Stream(stream) => &stream.dict,
            _ => return false,
        };
        let mark = dict.get(b"Mark");
        mark.is_some_and(|mark| match mark.value() {
            Value::String(string, _) => string.as_bytes() == Some(text.as_bytes()),
            _ => false,
        })
    });
    id.unwrap_or_else(|| panic!("nothing's marked {text}")).0
}

/// Each of the file's page objects, in order.
fn page_ids(doc: &Document) -> Vec<ObjectId> {
    let trailer = doc.trailer.dict();
    let catalog = dict_at(doc, trailer.get(b"Root").unwrap().as_reference().unwrap());
    let limits = pdf_semantics::Limits::DEFAULT;
    let table = pages(doc, catalog, Rules::Iso, &limits, &mut ()).unwrap();
    table.iter().map(|page| page.id.unwrap()).collect()
}

#[test]
fn a_node_above_the_root_only_a_parent_names_loses_what_it_held_too() {
    let bytes = file(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Parent 4 0 R /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /Rotate 90 /MediaBox [0 0 9 9] /Resources << >> >>",
            "<< /Type /Pages /Rotate 90 /MediaBox [0 0 9 9] /Resources << >> /Mine 1 >>",
        ],
        ROOT,
    );
    for options in kinds() {
        let out = written(&bytes, &options);
        let reading = Reading::open(&out[..], &bytes::Options::DEFAULT).unwrap();
        let mut findings = Vec::new();
        let verdict = family::check(&reading, &Limits::DEFAULT, &mut findings).unwrap();
        assert!(verdict.holds);
        assert_eq!(findings, []);
        let (_, doc) = read(&out);
        let above = doc.objects().find_map(|(_, object)| {
            let dict = object.as_dict()?;
            (dict.has(b"Mine") && dict.has_type(b"Pages")).then_some(dict)
        });
        assert!(!above.unwrap().has(b"Rotate"));
        let page = dict_at(&doc, page_ids(&doc)[0]);
        assert_eq!(page.get(b"Rotate").unwrap().as_integer(), Some(90));
    }
}

#[test]
fn what_going_down_kids_reads_otherwise_than_up_parent_is_refused() {
    let own = "/Resources << >>";
    let node = |under: u32, width: u32| {
        format!(
            "<< /Type /Pages /Parent 2 0 R /Kids [{under} 0 R] /Count 1 /MediaBox [0 0 {width} {width}] >>"
        )
    };
    let page = |parent: u32| format!("<< /Type /Page /Parent {parent} 0 R {own} >>");
    let root = "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>";
    let catalog = "<< /Type /Catalog /Pages 2 0 R >>";
    let refused = |objects: &[&str], wanted: usize, key: &str| {
        let got = family::linearize_bytes(&file(objects, ROOT), &[], &table());
        assert!(
            matches!(
                got,
                Err(Error::PageTree(PageTree::Kids { page, attribute }))
                    if page == wanted && attribute == key
            ),
            "{got:?}"
        );
    };
    // Each node lists the page whose /Parent is the other, whose box then differs.
    let apart = [
        catalog,
        root,
        &node(5, 100),
        &node(6, 200),
        &page(4),
        &page(3),
    ];
    refused(&apart, 0, "MediaBox");
    // Above the root, where going down starts: by /Parent alone it's inherited.
    let above = [
        catalog,
        "<< /Type /Pages /Parent 4 0 R /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /Resources << >> >>",
        "<< /Type /Pages /MediaBox [0 0 9 9] >>",
    ];
    refused(&above, 0, "MediaBox");
    // A page that names no /Parent, under a node going down gives something to.
    let no_parent = [
        catalog,
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 9 9] >>",
        "<< /Type /Page /Resources << >> >>",
    ];
    refused(&no_parent, 0, "MediaBox");
    // Apart, and each reads what it did: the same value, at either place.
    let alike = [
        catalog,
        root,
        &node(5, 100),
        &node(6, 100),
        &page(4),
        &page(3),
    ];
    for options in kinds() {
        let out = written(&file(&alike, ROOT), &options);
        let (_, doc) = read(&out);
        for id in page_ids(&doc) {
            let held = dict_at(&doc, id).get(b"MediaBox").unwrap();
            assert_eq!(held.as_array().unwrap().len(), 4);
        }
    }
    // Where the pages say all they have, there's nothing to read otherwise.
    let own_box = "<< /Type /Page /Parent 3 0 R /Resources << >> /MediaBox [0 0 7 7] >>";
    let own_too = "<< /Type /Page /Parent 4 0 R /Resources << >> /MediaBox [0 0 8 8] >>";
    let said = [
        catalog,
        root,
        &node(6, 100),
        &node(5, 200),
        own_box,
        own_too,
    ];
    holds_clean(&written(&file(&said, ROOT), &table()));
}

/// `out` holds, and nothing's to be said of it.
fn holds_clean(out: &[u8]) {
    let reading = Reading::open(out, &bytes::Options::DEFAULT).unwrap();
    let mut findings = Vec::new();
    let verdict = family::check(&reading, &Limits::DEFAULT, &mut findings).unwrap();
    assert!(verdict.linearized && verdict.holds, "{findings:?}");
    assert_eq!(findings, []);
}

#[test]
fn an_object_that_s_only_a_reference_to_nothing_has_no_body_and_no_entry() {
    let catalog = "<< /Type /Catalog /Pages 2 0 R >>";
    let root = "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 9 9] /Resources << >> >>";
    let first = "<< /Type /Page /Parent 2 0 R >>";
    let later = "<< /Type /Page /Parent 2 0 R /X 6 0 R >>";
    let start = [catalog, root, first, later, later];
    // To no object; to a null; along a chain to one; round to itself, by way of another; to
    // what a file leaves out.
    let shapes: [&[&str]; 5] = [
        &["99 0 R"],
        &["7 0 R", "null"],
        &["7 0 R", "8 0 R", "null"],
        &["7 0 R", "6 0 R"],
        &["7 0 R", "<< /Linearized 1 >>"],
    ];
    for shape in shapes {
        let objects: Vec<&str> = start.iter().chain(shape).copied().collect();
        let bytes = file(&objects, ROOT);
        for options in kinds() {
            let out = written(&bytes, &options);
            holds_clean(&out);
            let (_, doc) = read(&out);
            for id in page_ids(&doc).into_iter().skip(1) {
                assert!(dict_at(&doc, id).get(b"X").unwrap().is_null(), "{shape:?}");
            }
            let bare = doc.objects().any(|(_, object)| object.is_null());
            assert!(!bare, "{shape:?}");
        }
    }
    // Along a chain to something, it's what it was.
    let objects = [
        catalog,
        root,
        first,
        later,
        later,
        "7 0 R",
        "<< /Mark (kept) >>",
    ];
    let bytes = file(&objects, ROOT);
    for options in kinds() {
        let out = written(&bytes, &options);
        holds_clean(&out);
        let (_, doc) = read(&out);
        let id = page_ids(&doc)[1];
        let (_, held) = doc
            .dereference(dict_at(&doc, id).get(b"X").unwrap())
            .unwrap();
        assert!(held.as_dict().unwrap().has(b"Mark"));
    }
}

#[test]
fn a_parent_that_s_a_page_or_no_node_keeps_what_it_holds_and_nothing_s_said_of_it() {
    let catalog = "<< /Type /Catalog /Pages 2 0 R >>";
    let own = "/MediaBox [0 0 9 9] /Resources << >>";
    let root = "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>";
    let page = format!("<< /Type /Page /Parent 2 0 R {own} >>");
    // A listed page's /Parent is another listed page.
    let under_page = format!("<< /Type /Page /Parent 3 0 R {own} >>");
    let bytes = file(&[catalog, root, &page, &under_page], ROOT);
    for options in kinds() {
        holds_clean(&written(&bytes, &options));
    }
    // A /Parent that's a dictionary holding kids of no kind, a box among them.
    let under_other = format!("<< /Type /Page /Parent 5 0 R {own} >>");
    let other = format!("<< /Mine true {own} >>");
    let bytes = file(&[catalog, root, &page, &under_other, &other], ROOT);
    for options in kinds() {
        holds_clean(&written(&bytes, &options));
    }
}

#[test]
fn a_page_the_tree_doesn_t_list_gets_what_it_inherited_from_a_node_that_loses_it() {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /Foo [4 0 R 6 0 R] >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 9 9] /Resources 5 0 R /Rotate 90 >>",
        "<< /Type /Page /Parent 2 0 R >>",
        // Unlisted, with a box of its own, and the rest from the node.
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 3 3] /Mark (stray) >>",
        "<< /Mark (resources) >>",
        // Unlisted, its parent no node of the tree: what that holds it still holds.
        "<< /Type /Page /Parent 7 0 R /Mark (other) >>",
        "<< /Type /Pages /Rotate 180 /Mark (unlisted node) >>",
    ];
    let bytes = file(&objects, ROOT);
    for options in kinds() {
        let out = written(&bytes, &options);
        holds_clean(&out);
        let (_, doc) = read(&out);
        let stray = doc.objects().find_map(|(_, object)| {
            let dict = object.as_dict()?;
            (dict.get(b"Mark")?.as_bytes()? == b"stray").then_some(dict)
        });
        let stray = stray.unwrap();
        assert_eq!(stray.get(b"Rotate").unwrap().as_integer(), Some(90));
        assert_eq!(stray.get(b"MediaBox").unwrap().as_array().unwrap().len(), 4);
        let width = stray.get(b"MediaBox").unwrap().as_array().unwrap()[2].as_integer();
        assert_eq!(width, Some(3), "its own");
        let resources = stray.get(b"Resources").unwrap().as_reference().unwrap();
        assert!(doc.get_dict(resources).unwrap().has(b"Mark"));
        // Nothing's pushed down past what didn't lose it.
        let other = doc.objects().find_map(|(_, object)| {
            let dict = object.as_dict()?;
            (dict.get(b"Mark")?.as_bytes()? == b"other").then_some(dict)
        });
        assert!(!other.unwrap().has(b"Rotate"));
    }
}

#[test]
fn each_kind_reads_back_linearized_with_the_same_pages_and_objects() {
    let bytes = built(&two_pages(""));
    for options in kinds() {
        let (out, done) = linearized(&bytes, &options).unwrap();
        assert_eq!(out.len() as u64, done.layout.len());
        let (reading, doc) = read(&out);
        assert!(reading.facts().linearized, "{options:?}");
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        let ids = page_ids(&doc);
        assert_eq!(ids.len(), 2);
        let content = |page: ObjectId| {
            let contents = dict_at(&doc, page).get(b"Contents").unwrap();
            reading
                .data(contents.as_reference().unwrap())
                .unwrap()
                .to_vec()
        };
        assert_eq!(content(ids[0]), b"0 0 m");
        assert_eq!(content(ids[1]), b"1 1 m");
        let font = marked(&doc, "font");
        let resources = dict_at(&doc, ids[1]).get(b"Resources").unwrap();
        assert_eq!(resources.as_reference(), Some(marked(&doc, "resources")));
        let fonts = dict_at(&doc, marked(&doc, "resources"))
            .get(b"Font")
            .unwrap();
        let named = fonts.as_dict().unwrap().get(b"F0").unwrap();
        assert_eq!(named.as_reference(), Some(font));
    }
}

#[test]
fn an_inherited_attribute_is_on_each_page_and_on_no_node() {
    let bytes = built(&two_pages(""));
    for options in kinds() {
        let out = written(&bytes, &options);
        let (_, doc) = read(&out);
        let trailer = doc.trailer.dict();
        let catalog = dict_at(&doc, trailer.get(b"Root").unwrap().as_reference().unwrap());
        let limits = pdf_semantics::Limits::DEFAULT;
        let table = pages(&doc, catalog, Rules::Iso, &limits, &mut ()).unwrap();
        let first = table.get(0).unwrap();
        let media = first.attribute(Inheritable::MediaBox).unwrap();
        assert_eq!(media.origin, Origin::Own);
        assert_eq!(first.media_box().value.0, [0.0, 0.0, 100.0, 200.0]);
        let second = table.get(1).unwrap();
        assert_eq!(second.media_box().value.0, [0.0, 0.0, 30.0, 40.0]);
        for page in &table {
            let resources = page.attribute(Inheritable::Resources).unwrap();
            assert_eq!(resources.origin, Origin::Own);
            // As written: a reference stays one.
            assert_eq!(
                resources.written.as_reference(),
                Some(marked(&doc, "resources"))
            );
        }
        let node = first.parents().next().unwrap();
        for key in [&b"MediaBox"[..], b"Resources", b"CropBox", b"Rotate"] {
            assert!(!dict_at(&doc, node).has(key), "the node keeps it");
        }
        assert!(dict_at(&doc, node).has(b"Kids"));
    }
}

#[test]
fn only_what_changes_is_written_anew() {
    // The second page needs nothing, and keeps its spacing as a copy; the first
    // and the node get theirs from their values.
    let bytes = built(&two_pages(""));
    let out = written(&bytes, &table());
    assert!(at(&out, "/MediaBox [0 0 30 40] /Resources ") > 0);
    let (_, doc) = read(&out);
    let copy = String::from_utf8_lossy(&out).contains("<<  /Type /Page /Parent ");
    assert!(copy, "the second page isn't copied");
    let ids = page_ids(&doc);
    assert!(dict_at(&doc, ids[0]).has(b"MediaBox"));
}

#[test]
fn every_page_in_its_part_and_shared_after_them() {
    let mut objects = two_pages("");
    // Each page takes a marked object of its own, and both one more.
    objects[2] = "<< /Type /Page /Parent 2 0 R /Contents [5 0 R] /Own 9 0 R /Both 11 0 R >>".into();
    objects[3] = "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Own 10 0 R /Both 11 0 R >>".into();
    objects.push("<< /Mark (first alone) >>".into());
    objects.push("<< /Mark (second alone) >>".into());
    objects.push("<< /Mark (shared) >>".into());
    let bytes = built(&objects);
    let (out, done) = linearized(&bytes, &table()).unwrap();
    let e = done.layout.first_page_end();
    assert!(at(&out, "/Mark (first alone)") < e);
    assert!(at(&out, "/Mark (first content)") < e);
    // What the first page uses stays with it, whoever else does.
    assert!(at(&out, "/Mark (shared)") < e);
    assert!(at(&out, "/Mark (font)") < e);
    assert!(at(&out, "/Mark (second alone)") > e);
    assert!(at(&out, "/Mark (second content)") > e);
    let hint = done.layout.hint_extent().0;
    assert!(at(&out, "/Type /Catalog") < hint);
    assert!(hint < at(&out, "/Mark (first alone)"));
    // Images and font files last among a page's.
    assert!(done.id.is_none());
}

#[test]
fn a_page_s_objects_go_in_the_order_it_wants_them_read() {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Other 5 0 R /Resources 6 0 R /B [7 0 R] /Annots [8 0 R] /MediaBox [0 0 9 9] >>",
        "<< /Length 3 /Mark (content) >>\nstream\nq Q\nendstream",
        "<< /Mark (other) >>",
        "<< /Mark (resources) >>",
        "<< /Mark (bead) >>",
        "<< /Mark (annotation) >>",
    ];
    let out = written(&file(&objects, ROOT), &table());
    let places = ["annotation", "bead", "resources", "other", "content"]
        .map(|mark| at(&out, &format!("/Mark ({mark})")));
    assert!(
        places.windows(2).all(|pair| pair[0] < pair[1]),
        "{places:?}"
    );
}

#[test]
fn each_thing_a_table_names_has_its_table() {
    let mut objects = two_pages(
        "/Outlines 9 0 R /Threads [10 0 R] /Dests 12 0 R /AcroForm 13 0 R /StructTreeRoot 15 0 R /Names 16 0 R",
    );
    objects[2] = "<< /Type /Page /Parent 2 0 R /Contents [5 0 R] /Thumb 21 0 R >>".into();
    objects[3] = "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Thumb 22 0 R >>".into();
    objects.push("<< /Type /Outlines /Mark (outlines) >>".into());
    objects.push("<< /I 11 0 R >>".into());
    objects.push("<< /Title (t) /Mark (thread information) >>".into());
    objects.push("<< /Mark (dests) >>".into());
    objects.push("<< /Fields [14 0 R] >>".into());
    objects.push("<< /FT /Tx /T (a) /Mark (field) >>".into());
    objects.push("<< /Type /StructTreeRoot /Mark (structure) >>".into());
    objects.push("<< /Dests 17 0 R /Renditions 18 0 R /EmbeddedFiles 19 0 R >>".into());
    objects.push("<< /Names [(a) [3 0 R /Fit]] /Mark (name tree) >>".into());
    objects.push("<< /Names [] /Mark (renditions) >>".into());
    objects.push("<< /Names [(f) 20 0 R] /Mark (file names) >>".into());
    objects.push("<< /Type /Filespec /Mark (file) >>".into());
    objects.push(stream("/Width 1 /Height 1 /Mark (first thumbnail)", "t"));
    objects.push(stream("/Width 1 /Height 1 /Mark (second thumbnail)", "u"));
    objects.push("<< /Title (i) /Mark (information) >>".into());
    let objects: Vec<&str> = objects.iter().map(String::as_str).collect();
    let bytes = file(&objects, "/Root 1 0 R /Info 23 0 R");
    let (out, done) = linearized(&bytes, &table()).unwrap();
    let keys = done.layout.hint_keys();
    for (name, key) in [
        ("thumbnails", keys.thumbnails),
        ("outlines", keys.outlines),
        ("threads", keys.threads),
        ("dests", keys.dests),
        ("form", keys.form),
        ("information", keys.info),
        ("structure", keys.structure),
        ("renditions", keys.renditions),
        ("files", keys.files),
    ] {
        assert!(key.is_some(), "{name}");
    }
    let tables = hint::tables(done.layout.hint_data(), keys, 2, &Limits::DEFAULT).unwrap();
    let (_, doc) = read(&out);
    let number = |mark: &str| marked(&doc, mark).0;
    assert_eq!(tables.outlines.unwrap().first_object, number("outlines"));
    assert_eq!(
        tables.threads.unwrap().first_object,
        number("thread information")
    );
    assert_eq!(tables.dests.unwrap().first_object, number("dests"));
    assert_eq!(tables.form.unwrap().generic.first_object, number("field"));
    assert_eq!(tables.info.unwrap().first_object, number("information"));
    assert_eq!(
        tables.structure.unwrap().generic.first_object,
        number("structure")
    );
    assert_eq!(
        tables.renditions.unwrap().generic.first_object,
        number("renditions")
    );
    assert_eq!(tables.files.unwrap().groups[0].number, number("file"));
    // Each page's content items are its own stream's, read through an array.
    for entry in &tables.pages.pages {
        assert!(entry.content_length > 0);
        assert!(entry.content_at > 0);
    }
    let thumbnails = tables.thumbnails.unwrap();
    assert_eq!(
        thumbnails
            .pages
            .iter()
            .filter(|page| page.objects > 0)
            .count(),
        2
    );
    // Only /Dests of the name dictionary goes with the destinations.
    assert!(at(&out, "/Mark (name tree)") < at(&out, "/Mark (renditions)"));
    assert!(at(&out, "/Mark (file names)") > at(&out, "/Mark (file)"));
}

#[test]
fn images_and_font_files_come_after_the_rest_of_a_page() {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 9] /Resources 4 0 R /Contents 5 0 R >>"
            .to_string(),
        "<< /Font << /F0 8 0 R >> /XObject << /Im0 6 0 R >> >>".to_string(),
        stream("/Mark (content)", "q Q"),
        stream("/Mark (image) /Subtype /Image /Width 1 /Height 1", "x"),
        stream("/Mark (font file)", "y"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Foo /FontDescriptor 9 0 R >>".to_string(),
        "<< /Type /FontDescriptor /FontName /Foo /FontFile 7 0 R /Mark (descriptor) >>".to_string(),
    ];
    let out = written(&built(&objects), &table());
    let content = at(&out, "/Mark (content)");
    assert!(
        at(&out, "/Mark (descriptor)") < content,
        "resources before content"
    );
    assert!(content < at(&out, "/Mark (image)"));
    assert!(content < at(&out, "/Mark (font file)"));
}

#[test]
fn a_page_s_parent_is_not_walked_whatever_it_names() {
    let mut objects = two_pages("");
    objects[2] =
        "<< /Type /Page /Parent 9 0 R /Contents 5 0 R /MediaBox [0 0 9 9] /Resources 8 0 R >>"
            .into();
    objects.push("<< /Mark (no node) >>".into());
    let (out, done) = linearized(&built(&objects), &table()).unwrap();
    assert!(at(&out, "/Mark (no node)") > done.layout.first_page_end());
}

#[test]
fn a_thumbnail_is_part_nine_and_no_part_of_its_page() {
    let mut objects = two_pages("");
    objects[2] = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Thumb 9 0 R >>".into();
    objects.push(stream("/Mark (thumbnail) /Width 1 /Height 1", "z"));
    let (out, done) = linearized(&built(&objects), &table()).unwrap();
    assert!(at(&out, "/Mark (thumbnail)") > done.layout.first_page_end());
    assert!(at(&out, "/Mark (thumbnail)") > at(&out, "/Mark (second content)"));
    let (_, doc) = read(&out);
    let thumb = dict_at(&doc, page_ids(&doc)[0]).get(b"Thumb").unwrap();
    assert_eq!(thumb.as_reference(), Some(marked(&doc, "thumbnail")));
}

#[test]
fn a_page_s_parent_and_document_part_are_no_part_of_it() {
    // The part names the other page, and its sibling part's metadata, which would
    // come along if a page's walk went in.
    let mut objects = two_pages("/DPartRoot 9 0 R");
    objects[2] = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /DPart 10 0 R >>".into();
    objects.push("<< /Type /DPartRoot /DPartRootNode 11 0 R >>".into());
    objects.push("<< /Type /DPart /Parent 11 0 R /Start 3 0 R /DPM 12 0 R /Mark (part) >>".into());
    objects.push("<< /Type /DPart /DParts [[10 0 R 13 0 R]] /Mark (root part) >>".into());
    objects.push("<< /Mark (part metadata) >>".into());
    objects
        .push("<< /Type /DPart /Parent 11 0 R /Start 4 0 R /DPM 14 0 R /Mark (sibling) >>".into());
    objects.push("<< /Mark (sibling metadata) >>".into());
    let (out, done) = linearized(&built(&objects), &table()).unwrap();
    let e = done.layout.first_page_end();
    for mark in [
        "part",
        "root part",
        "part metadata",
        "sibling",
        "sibling metadata",
    ] {
        assert!(at(&out, &format!("/Mark ({mark})")) > e, "{mark}");
    }
    assert!(at(&out, "/Mark (second content)") < at(&out, "/Mark (part)"));
}

#[test]
fn a_form_s_dictionary_is_part_four_and_its_fields_are_not() {
    let mut objects = two_pages("/AcroForm 9 0 R");
    objects[3] = "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Annots [10 0 R] >>".into();
    objects.push("<< /Fields [10 0 R 11 0 R] /Mark (form) >>".into());
    objects.push("<< /FT /Tx /Subtype /Widget /T (a) /Mark (widget on page) >>".into());
    objects.push("<< /FT /Tx /T (b) /Mark (field on no page) >>".into());
    let (out, done) = linearized(&built(&objects), &table()).unwrap();
    let hint = done.layout.hint_extent().0;
    assert!(at(&out, "/Mark (form)") < hint);
    let e = done.layout.first_page_end();
    assert!(at(&out, "/Mark (widget on page)") > e);
    assert!(at(&out, "/Mark (widget on page)") < at(&out, "/Mark (field on no page)"));
    assert!(at(&out, "/Mark (second content)") < at(&out, "/Mark (field on no page)"));
    let (reading, _) = read(&out);
    assert!(reading.facts().linearized);
}

#[test]
fn what_opens_the_document_is_part_four_not_gone_into() {
    let mut objects =
        two_pages("/ViewerPreferences 9 0 R /PageMode 10 0 R /OpenAction 11 0 R /Threads 12 0 R");
    objects[3] = "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /B [15 0 R] >>".into();
    objects.push("<< /Mark (preferences) >>".into());
    objects.push("/UseNone".into());
    objects.push("<< /S /URI /URI (a) /Next 16 0 R /Mark (action) >>".into());
    objects.push("[13 0 R]".into());
    objects.push("<< /F 15 0 R /I 14 0 R /Mark (thread) >>".into());
    objects.push("<< /Title (t) /Mark (thread information) >>".into());
    objects.push("<< /T 13 0 R /N 15 0 R /V 15 0 R /P 4 0 R /R [0 0 1 1] /Mark (bead) >>".into());
    objects.push("<< /S /URI /URI (b) /Mark (next action) >>".into());
    let (out, done) = linearized(&built(&objects), &table()).unwrap();
    let hint = done.layout.hint_extent().0;
    for mark in ["preferences", "action", "thread"] {
        assert!(at(&out, &format!("/Mark ({mark})")) < hint, "{mark}");
    }
    let e = done.layout.first_page_end();
    assert!(at(&out, "/Mark (bead)") > e, "a bead is its page's");
    assert!(at(&out, "/Mark (bead)") < at(&out, "/Mark (thread information)"));
    assert!(at(&out, "/Mark (next action)") > e);
}

#[test]
fn what_the_trailer_does_not_reach_is_not_written() {
    let mut objects = two_pages("");
    objects[7] = "<< /Font << /F0 7 0 R >> /Old 10 0 R >>".into();
    objects.push("<< /Mark (no one's) >>".into());
    objects.push("<< /Linearized 1 /L 1 /Mark (old linearization) >>".into());
    objects.push("<< /Title (a) /Mark (information) >>".into());
    let objects: Vec<&str> = objects.iter().map(String::as_str).collect();
    let bytes = file(&objects, "/Root 1 0 R /Info 11 0 R");
    let out = written(&bytes, &table());
    assert!(at(&out, "/Mark (information)") > 0);
    let text = String::from_utf8_lossy(&out);
    assert!(!text.contains("no one's"));
    // A linearization dictionary among what's reached is left out as under a whole
    // file, and what names it reads null.
    assert!(!text.contains("old linearization"));
    assert!(read(&out).0.facts().linearized);
}

#[test]
fn a_length_or_a_null_only_a_reference_names_goes() {
    let mut objects = two_pages("");
    objects[4] = "<< /Mark (content) /Length 9 0 R >>\nstream\nabcde\nendstream".into();
    objects[7] = "<< /Font << /F0 7 0 R >> /Gone 10 0 R >>".into();
    objects.push("5".into());
    objects.push("null".into());
    let out = written(&built(&objects), &table());
    let (reading, doc) = read(&out);
    let id = marked(&doc, "content");
    assert_eq!(
        dict_at(&doc, id).get(b"Length").unwrap().as_integer(),
        Some(5)
    );
    assert_eq!(reading.data(id).unwrap().as_ref(), b"abcde");
    // Catalog, node, two pages, two contents, font, resources; the dictionary and
    // the hint stream.
    assert_eq!(doc.objects().count(), 10);
    let ids = page_ids(&doc);
    let resources = dict_at(&doc, ids[0])
        .get(b"Resources")
        .unwrap()
        .as_reference();
    let gone = dict_at(&doc, resources.unwrap()).get(b"Gone").unwrap();
    assert!(gone.is_null());
}

#[test]
fn outlines_are_shown_order_in_part_nine_or_part_six_under_use_outlines() {
    let outline = |mode: &str| {
        let mut objects = two_pages(&format!("/Outlines 9 0 R {mode}"));
        objects.push("<< /First 14 0 R /Last 12 0 R /Count 3 /Mark (outlines) >>".into());
        // Item a is closed over a1; b follows it, and z, whose /Next comes before its
        // /First as written, comes before them.
        objects.push("<< /Title (a) /Parent 9 0 R /Prev 14 0 R /Next 12 0 R /First 11 0 R /Last 11 0 R /Count -1 /Mark (a) >>".into());
        objects.push("<< /Title (a1) /Parent 10 0 R /Mark (a1) >>".into());
        objects.push("<< /Title (b) /Parent 9 0 R /Prev 10 0 R /First 13 0 R /Last 13 0 R /Count 1 /Mark (b) >>".into());
        objects.push("<< /Title (b1) /Parent 12 0 R /Mark (b1) >>".into());
        objects.push("<< /Title (z) /Parent 9 0 R /Next 10 0 R /First 15 0 R /Last 15 0 R /Count 1 /Mark (z) >>".into());
        objects.push("<< /Title (z1) /Parent 14 0 R /Mark (z1) >>".into());
        built(&objects)
    };
    let order = |out: &[u8]| {
        ["outlines", "z", "z1", "a", "b", "b1", "a1"]
            .map(|mark| at(out, &format!("/Mark ({mark})")))
    };
    let (out, done) = linearized(&outline(""), &table()).unwrap();
    let places = order(&out);
    assert!(
        places.windows(2).all(|pair| pair[0] < pair[1]),
        "{places:?}"
    );
    assert!(places[0] > done.layout.first_page_end());
    assert!(places[0] > at(&out, "/Mark (second content)"));

    let (out, done) = linearized(&outline("/PageMode /UseOutlines"), &table()).unwrap();
    let places = order(&out);
    assert!(
        places.windows(2).all(|pair| pair[0] < pair[1]),
        "{places:?}"
    );
    assert!(places[6] < done.layout.first_page_end());
    assert!(places[0] > at(&out, "/Mark (first content)"));
}

/// The dictionary's `/P`, from the file's first bytes.
fn p_of(out: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(&out[..out.len().min(200)]);
    let at = text.find("/P ")?;
    Some(text[at + 3..].split_whitespace().next()?.to_string())
}

#[test]
fn the_first_page_is_where_the_document_opens() {
    for (more, second) in [
        ("/OpenAction [4 0 R /Fit]", true),
        ("/OpenAction << /S /GoTo /D [4 0 R /Fit] >>", true),
        ("/OpenAction [3 0 R /Fit]", false),
        ("/OpenAction << /S /URI /URI (a) /D [4 0 R /Fit] >>", false),
        ("/OpenAction [99 0 R /Fit]", false),
        ("/OpenAction /there /Dests << /there [4 0 R /Fit] >>", true),
        (
            "/OpenAction << /S /GoTo /D /there >> /Dests << /there << /D [4 0 R /Fit] >> >>",
            true,
        ),
        (
            "/OpenAction (there) /Names << /Dests << /Names [(there) [4 0 R /Fit]] >> >>",
            true,
        ),
    ] {
        let objects = two_pages(more);
        let (out, done) = linearized(&built(&objects), &table()).unwrap();
        let e = done.layout.first_page_end();
        match second {
            true => {
                assert_eq!(p_of(&out).as_deref(), Some("1"), "{more}");
                assert!(at(&out, "/Mark (second content)") < e, "{more}");
            }
            false => {
                assert_eq!(p_of(&out), None, "{more}");
                assert!(at(&out, "/Mark (first content)") < e, "{more}");
            }
        }
    }
    let mut options = table();
    options.first_page = FirstPage::Index(1);
    let out = written(&built(&two_pages("")), &options);
    assert_eq!(p_of(&out).as_deref(), Some("1"));
    options.first_page = FirstPage::Index(0);
    let out = written(&built(&two_pages("/OpenAction [4 0 R /Fit]")), &options);
    assert_eq!(p_of(&out), None);
    options.first_page = FirstPage::Index(2);
    let refused = linearized(&built(&two_pages("")), &options).unwrap_err();
    assert!(matches!(
        refused,
        Error::Plan(plan::Error::FirstPagePast { index: 2, pages: 2 })
    ));
}

#[test]
fn a_page_or_node_the_tree_does_not_list_stops_a_walk() {
    let mut objects = two_pages("");
    objects[2] = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Annots [9 0 R] >>".into();
    objects.push("<< /Subtype /Link /P 10 0 R /Mark (annotation) >>".into());
    objects.push("<< /Type /Page /Parent 2 0 R /Mark (stray page) /Contents 11 0 R >>".into());
    objects.push(stream("/Mark (stray content)", "q"));
    let (out, done) = linearized(&built(&objects), &table()).unwrap();
    let e = done.layout.first_page_end();
    assert!(at(&out, "/Mark (annotation)") < e);
    assert!(at(&out, "(stray page)") > e);
    assert!(at(&out, "/Mark (stray content)") > e);
}

#[test]
fn a_stray_page_and_a_template_are_never_packed() {
    let mut objects = two_pages("/Names << /Templates << /Names [(t) 10 0 R] >> >>");
    objects[2] = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Annots [9 0 R] >>".into();
    objects.push("<< /Subtype /Link /P 11 0 R >>".into());
    objects.push("<< /Type /Template /Mark (template) >>".into());
    objects.push("<< /Type /Page /Mark (stray page) >>".into());
    let mut packed = Options::DEFAULT;
    packed.kind = XrefKind::Stream;
    packed.pack = true;
    let out = written(&built(&objects), &packed);
    // Each in the clear, as a member's in a deflated stream.
    assert!(at(&out, "/Mark (template)") > 0);
    assert!(at(&out, "(stray page)") > 0);
    assert!(
        !String::from_utf8_lossy(&out).contains("/Mark (font)"),
        "the font is packed"
    );
}

#[test]
fn a_tree_that_gives_a_page_no_place_of_its_own_is_refused() {
    let tree = |kids: &str, more: &str| {
        let mut objects = two_pages("");
        objects[1] = format!("<< /Type /Pages /Kids [{kids}] /MediaBox [0 0 9 9] {more} >>");
        built(&objects)
    };
    let page_tree = |bytes: Vec<u8>| match linearized(&bytes, &table()) {
        Err(Error::PageTree(why)) => why,
        other => panic!("{:?}", other.map(|(out, _)| out.len())),
    };
    assert_eq!(
        page_tree(tree("3 0 R 3 0 R", "/Count 2")),
        PageTree::Repeated { page: 1 }
    );
    assert_eq!(
        page_tree(tree("3 0 R << /Type /Page /Parent 2 0 R >>", "/Count 2")),
        PageTree::Direct { page: 1 }
    );
    assert_eq!(
        page_tree(tree("3 0 R 99 0 R", "/Count 2")),
        PageTree::Rules { page: 1 }
    );
    assert_eq!(
        page_tree(tree("3 0 R 99 0 R 4 0 R", "/Count 2")),
        PageTree::Rules { page: 1 }
    );
    // A kid that's a reference to a page by way of another, which PDFium's rules
    // don't follow so far.
    let mut objects = two_pages("");
    objects[1] = "<< /Type /Pages /Kids [3 0 R 9 0 R] /Count 2 /MediaBox [0 0 9 9] >>".into();
    objects.push("4 0 R".into());
    assert_eq!(page_tree(built(&objects)), PageTree::Rules { page: 1 });
    // A leaf that says it's no page is a place with no page to PDFium.
    let mut objects = two_pages("");
    objects[3] = "<< /Type /Foo /Parent 2 0 R >>".into();
    assert_eq!(page_tree(built(&objects)), PageTree::NoPage { page: 1 });
    // A count under the tree stops PDFium's walk, and not ISO's.
    assert_eq!(
        page_tree(tree("3 0 R 4 0 R", "/Count 1")),
        PageTree::Rules { page: 1 }
    );
    // A null stops PDFium's climb, and not ISO's.
    let mut objects = two_pages("");
    objects[3] = "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources null >>".into();
    assert_eq!(
        page_tree(built(&objects)),
        PageTree::Attribute {
            page: 1,
            attribute: "Resources"
        }
    );
    // A node in place holding what would have to come off it.
    let mut objects = two_pages("");
    objects[1] =
        "<< /Type /Pages /Kids [<< /Kids [3 0 R] /Count 1 /Rotate 90 >>] /Count 1 >>".into();
    assert_eq!(
        page_tree(built(&objects)),
        PageTree::DirectNode {
            attribute: "Rotate"
        }
    );
}

#[test]
fn a_null_both_rules_read_as_none_is_no_refusal() {
    let mut objects = two_pages("");
    objects[1] =
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /Rotate null /MediaBox [0 0 9 9] >>".into();
    let out = written(&built(&objects), &table());
    let (_, doc) = read(&out);
    let ids = page_ids(&doc);
    assert!(!dict_at(&doc, ids[0]).has(b"Rotate"));
}

#[test]
fn no_catalog_is_refused() {
    let bytes = file(&["<< /Type /Foo >>"], "/Info 1 0 R");
    assert!(matches!(
        linearized(&bytes, &table()),
        Err(Error::NoCatalog)
    ));
    let bytes = file(&["<< /Type /Foo >>"], "/Root << /Pages 1 0 R >>");
    assert!(matches!(
        linearized(&bytes, &table()),
        Err(Error::NoCatalog)
    ));
    let bytes = file(&["<< /Type /Catalog >>"], ROOT);
    assert!(matches!(linearized(&bytes, &table()), Err(Error::Pages(_))));
}

#[test]
fn a_root_through_a_reference_names_the_catalog_itself() {
    let mut objects = two_pages("");
    objects.push("1 0 R".into());
    let objects: Vec<&str> = objects.iter().map(String::as_str).collect();
    let bytes = file(&objects, "/Root 9 0 R");
    let mut packed = Options::DEFAULT;
    packed.kind = XrefKind::Stream;
    packed.pack = true;
    let out = written(&bytes, &packed);
    // Unpacked, as a section's /Root names it.
    assert!(at(&out, "/Type /Catalog") > 0);
    let (_, doc) = read(&out);
    let root = doc
        .trailer
        .dict()
        .get(b"Root")
        .unwrap()
        .as_reference()
        .unwrap();
    assert!(dict_at(&doc, root).has_type(b"Catalog"));
}

#[test]
fn a_length_something_else_names_is_no_part_of_its_stream_s_page() {
    let mut objects = two_pages("/Mine 9 0 R");
    objects[5] = "<< /Mark (second content) /Length 9 0 R >>\nstream\n1 1 m\nendstream".into();
    objects.push("5".into());
    let (out, done) = linearized(&built(&objects), &table()).unwrap();
    let (_, doc) = read(&out);
    let catalog = doc
        .trailer
        .dict()
        .get(b"Root")
        .unwrap()
        .as_reference()
        .unwrap();
    let mine = dict_at(&doc, catalog)
        .get(b"Mine")
        .unwrap()
        .as_reference()
        .unwrap();
    assert_eq!(doc.get(mine).unwrap().as_integer(), Some(5));
    let place = doc.record(mine).unwrap().at.start as u64;
    let section = at(&out, "(second content)");
    let tree = doc.record(page_ids_parent(&doc)).unwrap().at.start as u64;
    assert!(place > section && place > tree, "{place} {section} {tree}");
    assert!(done.layout.main_section() > place);
}

/// The first page's parent, a node, in part 9.
fn page_ids_parent(doc: &Document) -> ObjectId {
    let page = page_ids(doc)[0];
    dict_at(doc, page)
        .get(b"Parent")
        .unwrap()
        .as_reference()
        .unwrap()
}

#[test]
fn a_table_is_not_packed() {
    let mut options = table();
    options.pack = true;
    let refused = linearized(&built(&two_pages("")), &options).unwrap_err();
    assert!(matches!(refused, Error::Write(write::Error::PackedTable)));
}

#[test]
fn out_is_as_it_was_given_on_any_error_and_positions_count_from_where_it_starts() {
    let bytes = built(&two_pages(""));
    let reading = Reading::open(&bytes[..], &bytes::Options::DEFAULT).unwrap();
    let doc = reading.to_document().unwrap();
    let mut out = b"before".to_vec();
    let done = family::linearize(&mut out, &doc, None, Some(&reading), &table()).unwrap();
    assert!(out.starts_with(b"before%PDF-"));
    let alone = written(&bytes, &table());
    assert_eq!(&out[6..], &alone);
    assert_eq!(done.layout.len(), alone.len() as u64);

    // Stopped part-way: bodies aren't left behind.
    let stop = AtomicBool::new(true);
    let mut options = table();
    options.stop = Some(&stop);
    let mut out = b"before".to_vec();
    let refused = family::linearize(&mut out, &doc, None, Some(&reading), &options).unwrap_err();
    assert!(matches!(refused, Error::Write(write::Error::Stopped)));
    assert_eq!(out, b"before");

    // A stream's data no reading is given for, after other bodies went in.
    let mut out = b"before".to_vec();
    let refused = family::linearize(&mut out, &doc, None, None, &table()).unwrap_err();
    assert!(
        matches!(refused, Error::Write(write::Error::Placed(_))),
        "{refused}"
    );
    assert_eq!(out, b"before");

    // A cap.
    let mut options = table();
    options.limits.objects = 3;
    let mut out = b"before".to_vec();
    let refused = family::linearize(&mut out, &doc, None, Some(&reading), &options).unwrap_err();
    assert!(matches!(
        refused,
        Error::Plan(plan::Error::TooManyObjects { .. })
    ));
    assert_eq!(out, b"before");
}

#[test]
fn the_same_document_writes_the_same_bytes_on_any_threads() {
    let bytes = built(&two_pages(""));
    for threads in [1, 8] {
        let mut options = Options::DEFAULT;
        options.kind = XrefKind::Stream;
        options.pack = true;
        options.threads = 1;
        let one = written(&bytes, &options);
        options.threads = threads;
        assert_eq!(written(&bytes, &options), one);
    }
}

#[test]
fn linearize_bytes_is_linearize_of_what_it_reads() {
    let bytes = built(&two_pages(""));
    for options in kinds() {
        let out = family::linearize_bytes(&bytes, &[], &options).unwrap();
        assert_eq!(out, written(&bytes, &options));
    }
    let refused = family::linearize_bytes(b"not a file", &[], &table()).unwrap_err();
    assert!(
        matches!(refused, Error::NoCatalog | Error::Read(_)),
        "{refused}"
    );
}

/// What opens a file is the caller's: under caps it can't read under, it's refused.
#[test]
fn linearize_bytes_opens_a_file_as_it_s_told() {
    let bytes = built(&two_pages(""));
    assert!(family::linearize_bytes(&bytes, &[], &table()).is_ok());
    let mut options = table();
    options.read.limits.parser.nesting = 0;
    let refused = family::linearize_bytes(&bytes, &[], &options);
    assert!(refused.is_err(), "{:?}", refused.map(|_| ()));
}

#[test]
fn a_file_linearized_again_keeps_its_pages_and_drops_its_old_layout() {
    let bytes = built(&two_pages(""));
    for options in kinds() {
        let once = written(&bytes, &options);
        for options in kinds() {
            let twice = written(&once, &options);
            let (reading, doc) = read(&twice);
            assert!(reading.facts().linearized);
            assert_eq!(page_ids(&doc).len(), 2);
            let hints = doc.objects().filter(|(_, object)| match object.value() {
                Value::Dict(dict) => dict.has(b"Linearized"),
                _ => false,
            });
            assert_eq!(hints.count(), 1);
            assert_eq!(
                reading
                    .data(marked(&doc, "second content"))
                    .unwrap()
                    .as_ref(),
                b"1 1 m"
            );
        }
    }
}

#[test]
fn the_id_keeps_its_first_string_and_hashes_the_bodies() {
    let objects = two_pages("");
    let objects: Vec<&str> = objects.iter().map(String::as_str).collect();
    let bytes = file(&objects, "/Root 1 0 R /ID [<aabb> <ccdd>]");
    for options in kinds() {
        let (out, done) = linearized(&bytes, &options).unwrap();
        let [first, second] = done.id.clone().unwrap();
        assert_eq!(first, [0xAA, 0xBB]);
        assert_eq!(second.len(), 16);
        let (_, doc) = read(&out);
        let id = doc.trailer.dict().get(b"ID").unwrap().as_array().unwrap();
        let string = |object: &Object| match object.value() {
            Value::String(string, _) => string.as_bytes().unwrap().to_vec(),
            _ => panic!("not a string"),
        };
        assert_eq!(string(&id[0]), first);
        assert_eq!(string(&id[1]), second);
    }
}

#[test]
fn the_header_is_the_document_s_at_least_what_a_stream_needs() {
    let header = |bytes: &[u8], options: &Options<'_>| written(bytes, options)[..8].to_vec();
    let bytes = built(&two_pages(""));
    assert_eq!(header(&bytes, &table()), b"%PDF-1.6");
    let mut old = bytes.clone();
    old[7] = b'4';
    assert_eq!(header(&old, &table()), b"%PDF-1.4");
    let mut options = table();
    options.kind = XrefKind::Stream;
    assert_eq!(header(&old, &options), b"%PDF-1.5");
    assert_eq!(header(&bytes, &options), b"%PDF-1.6");
    options.version = Some(Version::new(2, 0));
    assert_eq!(header(&old, &options), b"%PDF-2.0");
}

#[test]
fn a_reference_to_no_object_is_written_null_and_the_trailer_too() {
    let mut objects = two_pages("/Foo 99 0 R");
    objects[7] = "<< /Font << /F0 7 0 R >> >>".into();
    let objects: Vec<&str> = objects.iter().map(String::as_str).collect();
    let bytes = file(&objects, "/Root 1 0 R /Info 99 0 R /Mine 7 0 R");
    let out = written(&bytes, &table());
    let (_, doc) = read(&out);
    let catalog = doc
        .trailer
        .dict()
        .get(b"Root")
        .unwrap()
        .as_reference()
        .unwrap();
    assert!(dict_at(&doc, catalog).get(b"Foo").unwrap().is_null());
    assert!(doc.trailer.dict().get(b"Info").unwrap().is_null());
    let font = doc.trailer.dict().get(b"Mine").unwrap().as_reference();
    assert_eq!(font, Some(marked(&doc, "font")));
}

#[test]
fn an_object_stream_too_large_for_a_reader_is_written_as_its_members() {
    let long = "a".repeat(100 << 20);
    let mut objects = two_pages("");
    objects[2] =
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /A 9 0 R /B 10 0 R /C 11 0 R >>".into();
    for _ in 0..3 {
        objects.push(format!("({long})"));
    }
    let bytes = built(&objects);
    let mut options = Options::DEFAULT;
    options.kind = XrefKind::Stream;
    options.pack = true;
    let (out, _) = linearized(&bytes, &options).unwrap();
    let (reading, doc) = read(&out);
    assert!(reading.facts().linearized);
    let page = page_ids(&doc)[0];
    for key in [&b"A"[..], b"B", b"C"] {
        let id = dict_at(&doc, page)
            .get(key)
            .unwrap()
            .as_reference()
            .unwrap();
        let record = doc.record(id).unwrap();
        assert_eq!(
            record.origin,
            pdf_structure::Origin::File,
            "it's on its own"
        );
    }
}
