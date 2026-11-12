//! What the lopdf adapter writes, read back with pdf-structure's reader and held by
//! the check: what it changes of a document, what it refuses, and that it's what the
//! family adapter writes of the same bytes, object for object, as the numbers come
//! from the same walk over a model that shares nothing with it.

use lopdf::{Document, Object as LopdfObject, dictionary};
use pdf_linearize::Limits;
use pdf_linearize::check::Verdict;
use pdf_linearize::family::{self, Options};
use pdf_linearize::pdf_objects;
use pdf_linearize::plan::{self, plan};
use pdf_linearize_lopdf::{self as adapter, Error, PageTree};
use pdf_objects::object::{Object, ObjectId, Value};
use pdf_structure::Source as _;
use pdf_structure::pdf_crypt::Password;
use pdf_structure::pdf_crypt::Protection;
use pdf_structure::read::{self, Reading};
use pdf_structure::write::{self, Encryptor};

/// The files pdf-linearize's tests write by hand, as one source.
#[path = "../../pdf-linearize/tests/written/mod.rs"]
mod written;
use written::{KEYS, ROOT, built, encrypted_as, file, stream, two_pages};

/// `bytes` as lopdf reads them.
fn loaded(bytes: &[u8]) -> Document {
    Document::load_mem(bytes).unwrap()
}

/// `bytes` read by lopdf and written by the adapter.
fn laid_out(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    adapter::linearize(&mut out, &loaded(bytes), &adapter::Options::DEFAULT).map(|_| out)
}

/// What `written` holds by the check, whatever a sink wants.
fn checked(written: &[u8]) -> (Verdict, Vec<String>) {
    let reading = Reading::open(written, &read::Options::DEFAULT).unwrap();
    let mut findings = Vec::new();
    let verdict = family::check(&reading, &Limits::DEFAULT, &mut findings).unwrap();
    let quiet = family::check(&reading, &Limits::DEFAULT, &mut ()).unwrap();
    assert_eq!(verdict, quiet);
    let said = findings.iter().map(|finding| finding.to_string()).collect();
    (verdict, said)
}

/// `written` says it's linearized, holds, its hints read and nothing's to be said of it.
fn holds(written: &[u8]) {
    let (verdict, findings) = checked(written);
    assert!(verdict.linearized, "{findings:?}");
    assert!(verdict.holds, "{findings:?}");
    assert!(verdict.hints_read, "{findings:?}");
    assert_eq!(findings, [] as [String; 0]);
    let reading = Reading::open(written, &read::Options::DEFAULT).unwrap();
    assert!(reading.facts().linearized);
}

/// Whether two objects as read back have the same value, however each was spelled: a
/// string by its bytes, a real as its `f32`, a stream's dictionary but its `/Length`.
fn alike(a: &Object, b: &Object) -> bool {
    match (a.value(), b.value()) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Integer(a), Value::Integer(b)) => a == b,
        (Value::Real(a), Value::Real(b)) => *a as f32 == *b as f32,
        (Value::Name(a), Value::Name(b)) => a == b,
        (Value::String(a, _), Value::String(b, _)) => a.as_bytes() == b.as_bytes(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| alike(a, b))
        }
        (Value::Dict(a), Value::Dict(b)) => dicts_alike(a, b),
        (Value::Stream(a), Value::Stream(b)) => dicts_alike(&a.dict, &b.dict),
        (Value::Ref(a), Value::Ref(b)) => a == b,
        _ => false,
    }
}

fn dicts_alike(a: &pdf_objects::object::Dict, b: &pdf_objects::object::Dict) -> bool {
    let kept = |dict: &'_ pdf_objects::object::Dict| -> Vec<(Vec<u8>, Object)> {
        (dict.entries().iter())
            .filter(|entry| entry.key != b"Length")
            .map(|entry| (entry.key.to_vec(), entry.value.clone()))
            .collect()
    };
    let (a, b) = (kept(a), kept(b));
    a.len() == b.len()
        && (a.iter().zip(&b)).all(|((key, a), (other, b))| key == other && alike(a, b))
}

/// What's in `file`, read back.
fn read_back(file: &[u8]) -> (Reading<&[u8]>, pdf_structure::Document) {
    let reading = Reading::open(file, &read::Options::DEFAULT).unwrap();
    let doc = reading.to_document().unwrap();
    (reading, doc)
}

/// What a string object's bytes are.
fn text(object: &Object) -> &[u8] {
    match object.value() {
        Value::String(string, _) => string.as_bytes().unwrap(),
        _ => panic!("{object:?} isn't a string"),
    }
}

/// The object `id` in `doc`, read back.
fn object(doc: &pdf_structure::Document, id: ObjectId) -> &Object {
    doc.get(id).unwrap()
}

/// The keys of `file`'s hint stream, the tables it has, which is numbered last.
fn hint_keys(file: &[u8]) -> Vec<Vec<u8>> {
    let (_, doc) = read_back(file);
    let last = doc.objects().map(|(id, _)| id).max().unwrap();
    let stream = object(&doc, last).as_stream().unwrap();
    let mut keys: Vec<Vec<u8>> = (stream.dict.entries().iter())
        .map(|entry| entry.key.to_vec())
        .filter(|key| !matches!(&key[..], b"Length" | b"Filter" | b"DecodeParms"))
        .collect();
    keys.sort();
    keys
}

/// The numbers `file` gives, each its object and a stream's data as `read_back` has it.
fn numbers(file: &[u8]) -> Vec<(ObjectId, Object, Option<Vec<u8>>)> {
    let (reading, doc) = read_back(file);
    // The hint stream's numbered last, and is as long as what it lists says.
    let last = doc.objects().map(|(id, _)| id).max().unwrap();
    let mut all: Vec<_> = doc
        .objects()
        .filter(|(id, _)| *id != last)
        .filter(|(_, object)| !object.as_dict().is_some_and(|d| d.has(b"Linearized")))
        .map(|(id, object)| {
            let data = match object.value() {
                Value::Stream(_) => Some(reading.data(id).unwrap().to_vec()),
                _ => None,
            };
            (id, object.clone(), data)
        })
        .collect();
    all.sort_by_key(|(id, _, _)| *id);
    all
}

/// That the adapter's file of `bytes` holds, and has at each number what the family's
/// has there, as both plans were told the same of the same objects.
fn same_numbers_as_the_family(bytes: &[u8]) {
    holds(&laid_out(bytes).unwrap());
    numbers_are_the_family_s(bytes);
}

/// That the adapter's file of `bytes` has at each number what the family's has there.
fn numbers_are_the_family_s(bytes: &[u8]) {
    let ours = laid_out(bytes).unwrap();
    let theirs = family::linearize_bytes(bytes, &Options::DEFAULT).unwrap();
    assert_eq!(hint_keys(&ours), hint_keys(&theirs));
    let (ours, theirs) = (numbers(&ours), numbers(&theirs));
    assert_eq!(ours.len(), theirs.len());
    for ((id, a, data), (other, b, more)) in ours.iter().zip(&theirs) {
        assert_eq!(id, other);
        assert!(alike(a, b), "{id:?}: {a:?} against {b:?}");
        assert_eq!(data, more, "{id:?}");
    }
}

/// A file of everything a plan is told of, as the family's own tests lay it out: an
/// outline with a closed item, thumbnails, threads, a form, named destinations and
/// files, a structure tree, document parts, images, a font's file, and a page and a
/// node the tree doesn't list.
fn everything(more: &str) -> Vec<u8> {
    let mut objects = two_pages(&format!(
        "/Outlines 9 0 R /AcroForm 13 0 R /Threads 15 0 R /Names 17 0 R /Dests 18 0 R \
         /StructTreeRoot 19 0 R /DPartRoot 20 0 R /ViewerPreferences 22 0 R {more}"
    ));
    objects[2] = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Thumb 12 0 R /Annots [14 0 R] \
                  /Resources << /XObject << /I 23 0 R >> /Font << /F 24 0 R >> >> /B [16 0 R] >>"
        .into();
    objects[3] = "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Rotate 90 /Own 27 0 R \
                  /Annots [28 0 R] /DPart 21 0 R >>"
        .into();
    objects.push("<< /Type /Outlines /First 10 0 R /Last 11 0 R /Count 2 >>".into());
    objects.push(
        // Its /Next after /First, so a walk of the links would go down first.
        "<< /Title (a) /Parent 9 0 R /First 29 0 R /Last 29 0 R /Count -1 /Next 11 0 R >>".into(),
    );
    objects.push("<< /Title (b) /Parent 9 0 R /Prev 10 0 R /Dest [4 0 R /Fit] >>".into());
    objects.push(stream(
        "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
        "z",
    ));
    objects.push("<< /Fields [14 0 R 39 0 R] >>".into());
    objects.push("<< /FT /Tx /Subtype /Widget /T (w) /Rect [0 0 9 9.5] >>".into());
    objects.push("[30 0 R]".into());
    objects.push("<< /T 30 0 R /N 16 0 R /V 16 0 R /P 3 0 R /R [0 0 1 1] >>".into());
    objects.push("<< /Dests 31 0 R /EmbeddedFiles 32 0 R /Renditions 33 0 R >>".into());
    objects.push("<< /Far [4 0 R /Fit] >>".into());
    objects.push("<< /Type /StructTreeRoot /K [34 0 R] >>".into());
    objects.push("<< /Type /DPartRoot /DPartRootNode 21 0 R >>".into());
    objects.push("<< /Type /DPart /Parent 20 0 R /Start 4 0 R >>".into());
    objects.push("<< /HideToolbar true >>".into());
    objects.push(stream(
        "/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray \
         /BitsPerComponent 8",
        "i",
    ));
    objects.push(
        "<< /Type /Font /Subtype /TrueType /BaseFont /A /FontDescriptor 25 0 R >>".to_string(),
    );
    objects.push("<< /Type /FontDescriptor /FontName /A /FontFile2 26 0 R >>".into());
    objects.push(stream("/Length1 1", "f"));
    objects.push("<< /Mine true /Stray 35 0 R >>".into());
    objects.push("<< /Subtype /Link /Dest [3 0 R /Fit] >>".into());
    objects.push("<< /Title (under closed) /Parent 10 0 R >>".into());
    objects.push("<< /I 40 0 R /F 16 0 R >>".into());
    objects.push("<< /Names [(far) [4 0 R /Fit]] >>".into());
    objects.push("<< /Kids [38 0 R] >>".into());
    objects.push("<< /Names [(clip) << /S /MR >>] >>".into());
    objects.push("<< /S /P /Pg 3 0 R >>".into());
    objects.push("<< /Type /Page /Mark (a page the tree doesn't list) >>".into());
    objects.push("<< /Type /Filespec /F (a.txt) /EF << /F 37 0 R >> >>".into());
    objects.push(stream("/Type /EmbeddedFile", "attached"));
    objects.push("<< /Limits [(file) (file)] /Names [(file) 36 0 R] >>".into());
    objects.push("<< /FT /Tx /T (only the form names this) >>".into());
    objects.push("<< /Title (thread) >>".into());
    let objects: Vec<&str> = objects.iter().map(String::as_str).collect();
    with_info(file(&objects, ROOT))
}

/// `file` with an `/Info`, saved again by lopdf.
fn with_info(file: Vec<u8>) -> Vec<u8> {
    let mut doc = loaded(&file);
    let info = doc.add_object(dictionary! { "Title" => LopdfObject::string_literal("t") });
    doc.trailer.set("Info", info);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

#[test]
fn a_file_reads_back_with_its_pages_and_objects_and_holds() {
    let bytes = built(&two_pages(""));
    let mut out = Vec::new();
    let layout = adapter::linearize(&mut out, &loaded(&bytes), &adapter::Options::DEFAULT).unwrap();
    assert_eq!(out.len() as u64, layout.len());
    holds(&out);
    let (reading, doc) = read_back(&out);
    assert!(doc.faults().is_empty(), "{:?}", doc.faults());
    let lopdf = loaded(&out);
    let pages: Vec<_> = lopdf.page_iter().collect();
    assert_eq!(pages.len(), 2);
    let content = |page: (u32, u16)| {
        let contents = lopdf.get_page_contents(page);
        reading.data(contents[0].into()).unwrap().to_vec()
    };
    assert_eq!(content(pages[0]), b"0 0 m");
    assert_eq!(content(pages[1]), b"1 1 m");
    // As a table lists it, under the header the file had.
    assert!(out.starts_with(b"%PDF-1.6\n"));
    assert!(out.windows(5).any(|w| w == b"\nxref"));
    assert!(!out.windows(6).any(|w| w == b"/XRef "));
    assert!(!out.windows(6).any(|w| w == b"ObjStm"));
}

#[test]
fn what_s_written_doesn_t_depend_on_what_s_in_out_already() {
    let bytes = built(&two_pages(""));
    let mut out = b"before".to_vec();
    adapter::linearize(&mut out, &loaded(&bytes), &adapter::Options::DEFAULT).unwrap();
    assert_eq!(&out[..6], b"before");
    assert_eq!(out[6..], laid_out(&bytes).unwrap());
}

#[test]
fn it_writes_the_same_bytes_each_time() {
    let bytes = built(&two_pages(""));
    assert_eq!(laid_out(&bytes).unwrap(), laid_out(&bytes).unwrap());
}

#[test]
fn its_graph_is_what_it_plans_by() {
    let bytes = built(&two_pages(""));
    let doc = loaded(&bytes);
    let plan = plan(
        &adapter::graph(&doc, &adapter::Options::DEFAULT).unwrap(),
        &Limits::DEFAULT,
    )
    .unwrap();
    let mut out = Vec::new();
    let layout = adapter::linearize(&mut out, &doc, &adapter::Options::DEFAULT).unwrap();
    let at = layout
        .position(plan.number(ObjectId::new(3, 0)).unwrap().number)
        .unwrap() as usize;
    let number = plan.number(ObjectId::new(3, 0)).unwrap().number;
    assert!(out[at..].starts_with(format!("{number} 0 obj").as_bytes()));
    assert_eq!(number, plan.first_page_number());
}

#[test]
fn the_numbers_are_the_family_s() {
    same_numbers_as_the_family(&built(&two_pages("")));
    same_numbers_as_the_family(&everything(""));
    same_numbers_as_the_family(&everything("/PageMode /UseOutlines"));
}

#[test]
fn the_page_that_opens_goes_first_as_the_family_s_does() {
    for action in [
        "/OpenAction [4 0 R /Fit]",
        "/OpenAction << /S /GoTo /D [4 0 R /Fit] >>",
        "/OpenAction << /S /GoTo /D /Far >>",
        "/OpenAction << /S /GoTo /D (far) >>",
    ] {
        let bytes = everything(action);
        same_numbers_as_the_family(&bytes);
        let out = laid_out(&bytes).unwrap();
        let text = String::from_utf8_lossy(&out[..300]).into_owned();
        assert!(text.contains("/P 1"), "{action}: {text}");
    }
    // Opening nowhere of this file: page 0.
    let out = laid_out(&everything("/OpenAction << /S /GoTo /D [9 /Fit] >>")).unwrap();
    let text = String::from_utf8_lossy(&out[..300]).into_owned();
    assert!(!text.contains("/P 1"), "{text}");
}

#[test]
fn what_a_stream_s_length_alone_names_isn_t_written_nor_a_null_nor_what_nothing_reaches() {
    let mut objects = two_pages("/Null 10 0 R /None 99 0 R");
    objects[4] = format!("<< /Length 9 0 R >>\nstream\n{}\nendstream", "0 0 m");
    objects.push("5".into());
    objects.push("null".into());
    objects.push("<< /Mark (nothing reaches this) >>".into());
    let out = laid_out(&built(&objects)).unwrap();
    holds(&out);
    same_numbers_as_the_family(&built(&objects));
    let (reading, doc) = read_back(&out);
    // Written direct, what it names gone.
    let lopdf = loaded(&out);
    let first = lopdf.page_iter().next().unwrap();
    let contents = ObjectId::from(lopdf.get_page_contents(first)[0]);
    assert_eq!(reading.data(contents).unwrap().to_vec(), b"0 0 m");
    let length = object(&doc, contents)
        .as_stream()
        .unwrap()
        .dict
        .get(b"Length");
    assert_eq!(length.unwrap().as_integer(), Some(5));
    assert!(!out.windows(7).any(|w| w == b"nothing"));
    // What named nothing, and the null, are null.
    let catalog = lopdf.catalog().unwrap();
    assert_eq!(catalog.get(b"Null").unwrap(), &LopdfObject::Null);
    assert_eq!(catalog.get(b"None").unwrap(), &LopdfObject::Null);
    // Every object from 1 is there: a section lists none free but 0.
    assert_eq!(doc.objects().count() as u32 + 1, lopdf.max_id + 1);
}

#[test]
fn inherited_attributes_go_down_to_the_pages_and_off_the_nodes() {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        // Keys after those taken off keep their order.
        "<< /Type /Pages /MediaBox [0 0 9 9] /Kids [3 0 R] /Rotate 90 /Count 2 /Resources 6 0 R /Mine 1 >>",
        "<< /Type /Pages /Parent 2 0 R /Kids [4 0 R 5 0 R] /Count 2 /Rotate 180 /CropBox [0 0 1 1] >>",
        "<< /Type /Page /Parent 3 0 R /Mark (first) >>",
        "<< /Type /Page /Parent 3 0 R /Rotate 270 /MediaBox [0 0 5 5] /Mark (second) >>",
        "<< /Font << >> >>",
    ];
    let bytes = file(&objects, ROOT);
    let out = laid_out(&bytes).unwrap();
    holds(&out);
    same_numbers_as_the_family(&bytes);
    let lopdf = loaded(&out);
    let pages: Vec<_> = lopdf.page_iter().collect();
    let dict = |id| lopdf.get_dictionary(id).unwrap();
    let first = dict(pages[0]);
    assert_eq!(first.get(b"Rotate").unwrap().as_i64().unwrap(), 180);
    assert_eq!(first.get(b"MediaBox").unwrap().as_array().unwrap().len(), 4);
    assert_eq!(first.get(b"CropBox").unwrap().as_array().unwrap().len(), 4);
    // A reference stays one.
    assert!(first.get(b"Resources").unwrap().as_reference().is_ok());
    // Its own stays.
    let second = dict(pages[1]);
    assert_eq!(second.get(b"Rotate").unwrap().as_i64().unwrap(), 270);
    let media = second.get(b"MediaBox").unwrap().as_array().unwrap();
    assert_eq!(media[2].as_i64().unwrap(), 5);
    assert_eq!(second.get(b"CropBox").unwrap().as_array().unwrap().len(), 4);
    // Every node's lost them.
    for object in lopdf.objects.values() {
        let Ok(node) = object.as_dict() else { continue };
        if node.has_type(b"Pages") {
            for key in [&b"Resources"[..], b"MediaBox", b"CropBox", b"Rotate"] {
                assert!(!node.has(key));
            }
        }
    }
    // The document given isn't changed by any of it.
    let before = loaded(&bytes);
    let after = before.clone();
    let mut out = Vec::new();
    adapter::linearize(&mut out, &after, &adapter::Options::DEFAULT).unwrap();
    assert_eq!(before.objects, after.objects);
}

#[test]
fn a_node_only_a_parent_names_is_one_and_loses_what_it_held() {
    // Above the root, by /Parent alone.
    let objects = [
        CATALOG,
        "<< /Type /Pages /Parent 4 0 R /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Rotate 90 /MediaBox [0 0 9 9] /Resources << >> >>",
        "<< /Type /Pages /Rotate 90 /MediaBox [0 0 9 9] /Resources << >> /Mine 1 >>",
        &stream("", "0 0 m"),
    ];
    let bytes = file(&objects, ROOT);
    let out = laid_out(&bytes).unwrap();
    holds(&out);
    same_numbers_as_the_family(&bytes);
    let lopdf = loaded(&out);
    let above = (lopdf.objects.values()).find(|object| {
        object
            .as_dict()
            .is_ok_and(|dict| dict.has(b"Mine") && dict.has_type(b"Pages"))
    });
    assert!(!above.unwrap().as_dict().unwrap().has(b"Rotate"));
}

#[test]
fn what_a_page_says_is_its_parent_isn_t_walked_into_though_it_s_no_node() {
    // A broken file's: its /Parent names what's otherwise only its own.
    let objects = [
        CATALOG,
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 9] /Resources << >> >>",
        "<< /Type /Page /Parent 5 0 R /MediaBox [0 0 9 9] /Resources << >> >>",
        "<< /Mark (its parent, no node) >>",
    ];
    same_numbers_as_the_family(&file(&objects, ROOT));
}

#[test]
fn a_parent_is_read_as_the_family_reads_one_though_it_s_a_stream_or_a_reference() {
    let root = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
    let own = "/MediaBox [0 0 9 9] /Resources << >>";
    // A stream that holds /Kids is a node: nothing walks into it, and it loses what it held.
    let page = format!("<< /Type /Page /Parent 4 0 R /Rotate 90 {own} >>");
    let node = stream("/Kids [] /Rotate 90 /Mine 5 0 R", "data");
    let held = "<< /Mark (only the stream names this) >>";
    let bytes = file(&[CATALOG, root, &page, &node, held], ROOT);
    same_numbers_as_the_family(&bytes);
    let out = loaded(&laid_out(&bytes).unwrap());
    let mine = (out.objects.values()).find_map(|object| {
        let stream = object.as_stream().ok()?;
        stream.dict.has(b"Mine").then_some(&stream.dict)
    });
    assert!(!mine.unwrap().has(b"Rotate"));
    let page = (out.get_pages().values().next()).map(|id| out.get_dictionary(*id).unwrap());
    assert_eq!(
        page.unwrap().get(b"Rotate").unwrap(),
        &LopdfObject::Integer(90)
    );
    // One that doesn't is no node, and keeps what it holds.
    let page = "<< /Type /Page /Parent 4 0 R /MediaBox [0 0 9 9] /Resources 5 0 R >>";
    let form = stream("/Resources 5 0 R", "data");
    let resources = "<< /Mine true >>";
    numbers_are_the_family_s(&file(&[CATALOG, root, page, &form, resources], ROOT));
    // An object that's a reference to a node is that node.
    let page = format!("<< /Type /Page /Parent 4 0 R {own} >>");
    let node = "<< /Kids [] /Mine 6 0 R >>";
    let held = "<< /Mark (only the node names this) >>";
    let objects = [CATALOG, root, &page, "5 0 R", node, held];
    same_numbers_as_the_family(&file(&objects, ROOT));
}

#[test]
fn an_outline_s_links_are_read_as_the_family_reads_them_though_they_name_a_stream() {
    let root = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 9] /Resources << >> >>";
    let catalog = "<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R /PageMode /UseOutlines >>";
    let outlines = "<< /First 5 0 R /Last 6 0 R /Count 3 >>";
    let first = "<< /Title (a) /Parent 4 0 R /Next 7 0 R /Last 6 0 R >>";
    // /Last names a stream, and /Next an object that's a reference to an item.
    let last = stream("/Length1 1", "profile");
    let beside = "<< /Title (b) /Parent 4 0 R >>";
    let objects = [catalog, root, page, outlines, first, &last, "8 0 R", beside];
    let bytes = file(&objects, ROOT);
    same_numbers_as_the_family(&bytes);
    // Part 6 has them all, after the first page's object.
    let out = loaded(&laid_out(&bytes).unwrap());
    let number = |wanted: &dyn Fn(&LopdfObject) -> bool| {
        let found = out.objects.iter().find(|(_, object)| wanted(object));
        found.unwrap().0.0
    };
    let page = number(&|object| object.as_dict().is_ok_and(|d| d.has_type(b"Page")));
    let beside = number(&|object| {
        let title = object.as_dict().and_then(|d| d.get(b"Title"));
        title.is_ok_and(|title| title.as_str().is_ok_and(|title| title == b"b"))
    });
    let last = number(&|object| object.as_stream().is_ok_and(|s| s.dict.has(b"Length1")));
    assert!(beside > page, "{beside} {page}");
    assert!(last > page, "{last} {page}");
}

#[test]
fn a_kid_of_another_type_with_no_pages_under_it_is_no_place() {
    let objects = [
        CATALOG,
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 1 /MediaBox [0 0 9 9] /Resources << >> >>",
        "<< /Type /Other /Parent 2 0 R /Kids [] >>",
        "<< /Type /Page /Parent 2 0 R >>",
    ];
    same_numbers_as_the_family(&file(&objects, ROOT));
}

#[test]
fn a_stream_s_length_by_reference_in_a_document_built_by_hand_isn_t_followed() {
    // lopdf's reader makes each /Length direct, but its documents are the caller's.
    let bytes = built(&two_pages(""));
    let mut doc = loaded(&bytes);
    let length = doc.add_object(dictionary! { "Mark" => LopdfObject::string_literal("length") });
    let content = doc.get_object_mut((5, 0)).unwrap().as_stream_mut().unwrap();
    content.dict.set("Length", length);
    let mut out = Vec::new();
    adapter::linearize(&mut out, &doc, &adapter::Options::DEFAULT).unwrap();
    holds(&out);
    // Hexadecimal, as a string is here.
    assert!(!out.windows(12).any(|w| w == b"6C656E677468"));
    let (reading, written) = read_back(&out);
    let streams = written.objects().filter_map(|(id, object)| {
        let stream = object.as_stream()?;
        let length = stream.dict.get(b"Length")?;
        (stream.dict.has(b"Mark")).then(|| (id, length.as_integer()))
    });
    for (id, length) in streams {
        assert_eq!(
            length.map(|length| length as usize),
            Some(reading.data(id).unwrap().len())
        );
    }
}

#[test]
fn what_a_file_s_old_layout_held_isn_t_written_though_something_names_it() {
    let mut objects = two_pages("/One 9 0 R /Two 10 0 R /Three 11 0 R");
    objects.push(stream("/Type /ObjStm /N 0 /First 0", ""));
    objects.push(stream("/Type /XRef /W [1 1 1] /Size 1", ""));
    objects.push("<< /Linearized 1 /L 1 >>".into());
    let bytes = built(&objects);
    let out = laid_out(&bytes).unwrap();
    same_numbers_as_the_family(&bytes);
    let lopdf = loaded(&out);
    let catalog = lopdf.catalog().unwrap();
    for key in [&b"One"[..], b"Two", b"Three"] {
        assert_eq!(catalog.get(key).unwrap(), &LopdfObject::Null);
    }
}

#[test]
fn values_and_stream_data_read_back_as_they_were() {
    let data: Vec<u8> = (0..=255).collect();
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Mine 5 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 9 9.5] /Resources << >> >>"
            .to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
    ];
    let mut content = format!("<< /Length {} /Mark (x) >>\nstream\n", data.len()).into_bytes();
    content.extend_from_slice(&data);
    content.extend_from_slice(b"\nendstream");
    objects.push(content);
    objects.push(
        b"<< /Odd#20name#28 (a\\(b\\)\\\\) /Hex <00FF> /Big 9007199254740993 /Neg -3 /Real -0.25 \
          /Yes true /No false /Nil null /Arr [1 [2 [3]] << /In [4] >>] >>"
            .to_vec(),
    );
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut rows = Vec::new();
    for (number, object) in (1..).zip(&objects) {
        rows.push(bytes.len());
        bytes.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let at = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for row in rows {
        bytes.extend_from_slice(format!("{row:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R /Own (kept) >>\nstartxref\n{at}\n%%EOF\n")
            .as_bytes(),
    );

    let out = laid_out(&bytes).unwrap();
    holds(&out);
    same_numbers_as_the_family(&bytes);
    assert!(out.starts_with(b"%PDF-1.4\n"));
    let (reading, doc) = read_back(&out);
    let trailer = doc.trailer().dict();
    assert_eq!(text(trailer.get(b"Own").unwrap()), b"kept");
    let catalog = doc.get_dict(trailer.get(b"Root").unwrap().as_reference().unwrap());
    let mine = catalog
        .unwrap()
        .get(b"Mine")
        .unwrap()
        .as_reference()
        .unwrap();
    let mine = doc.get_dict(mine).unwrap();
    assert_eq!(text(mine.get(b"Odd name(").unwrap()), b"a(b)\\");
    assert_eq!(text(mine.get(b"Hex").unwrap()), [0, 255]);
    assert_eq!(
        mine.get(b"Big").unwrap().as_integer(),
        Some(9007199254740993)
    );
    assert_eq!(mine.get(b"Real").unwrap().as_number(), Some(-0.25));
    let found = doc.objects().find(|(_, object)| match object.value() {
        Value::Stream(stream) => stream.dict.has(b"Mark"),
        _ => false,
    });
    assert_eq!(reading.data(found.unwrap().0).unwrap().to_vec(), data);
}

#[test]
fn a_file_of_object_streams_loses_them_and_a_cross_reference_stream_s_entries() {
    let source = {
        let mut options = Options::DEFAULT;
        options.kind = pdf_structure::XrefKind::Stream;
        options.pack = true;
        family::linearize_bytes(&everything(""), &options).unwrap()
    };
    let out = laid_out(&source).unwrap();
    holds(&out);
    same_numbers_as_the_family(&source);
    assert!(!out.windows(6).any(|w| w == b"ObjStm"));
    let (_, doc) = read_back(&out);
    let trailer = doc.trailer().dict();
    for key in [
        &b"W"[..],
        b"Index",
        b"Filter",
        b"DecodeParms",
        b"Type",
        b"Length",
    ] {
        assert!(!trailer.has(key), "{}", String::from_utf8_lossy(key));
    }
    assert!(trailer.has(b"Info"));
}

#[test]
fn the_id_is_as_the_document_holds_it() {
    let bytes = file(
        &written::two_pages("")
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        "/Root 1 0 R /ID [<AABB> <CCDD>]",
    );
    let out = laid_out(&bytes).unwrap();
    holds(&out);
    let (_, doc) = read_back(&out);
    let id = doc.trailer().dict().get(b"ID").unwrap().as_array().unwrap();
    assert_eq!(text(&id[0]), [0xAA, 0xBB]);
    assert_eq!(text(&id[1]), [0xCC, 0xDD]);
}

/// `objects` as a file: the adapter's refusal of it, `out` left as it was given.
fn refused(objects: &[&str]) -> Error {
    let doc = loaded(&file(objects, ROOT));
    let mut out = b"given".to_vec();
    let why = adapter::linearize(&mut out, &doc, &adapter::Options::DEFAULT).unwrap_err();
    assert_eq!(out, b"given");
    assert!(!why.to_string().is_empty());
    why
}

const CATALOG: &str = "<< /Type /Catalog /Pages 2 0 R >>";
const PAGE: &str = "<< /Type /Page /Parent 2 0 R >>";

#[test]
fn a_tree_that_gives_a_page_no_place_of_its_own_is_refused() {
    let tree = |why: Error| match why {
        Error::PageTree(why) => why,
        why => panic!("{why}"),
    };
    let node = "<< /Type /Pages /Kids [3 0 R 3 0 R] /Count 2 >>";
    assert_eq!(
        tree(refused(&[CATALOG, node, PAGE])),
        PageTree::Repeated { page: 1 }
    );
    let node = "<< /Type /Pages /Kids [<< /Type /Page >>] /Count 1 >>";
    assert_eq!(
        tree(refused(&[CATALOG, node])),
        PageTree::Direct { page: 0 }
    );
    let node = "<< /Type /Pages /Kids [null] /Count 1 >>";
    assert_eq!(
        tree(refused(&[CATALOG, node])),
        PageTree::NoPage { page: 0 }
    );
    let node = "<< /Type /Pages /Kids [9 0 R] /Count 1 >>";
    assert_eq!(
        tree(refused(&[CATALOG, node])),
        PageTree::NoPage { page: 0 }
    );
    let node = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
    assert_eq!(
        tree(refused(&[CATALOG, node, "(a string)"])),
        PageTree::NoPage { page: 0 }
    );
    // A node listing its own ancestor: its root, and itself.
    let node = "<< /Type /Pages /Kids [3 0 R 2 0 R] /Count 1 >>";
    assert_eq!(
        tree(refused(&[CATALOG, node, PAGE])),
        PageTree::Repeated { page: 1 }
    );
    // A node listed in two places.
    let root = "<< /Type /Pages /Kids [3 0 R 3 0 R] /Count 1 >>";
    let under = "<< /Type /Pages /Parent 2 0 R /Kids [4 0 R] /Count 1 >>";
    let page = "<< /Type /Page /Parent 3 0 R >>";
    assert_eq!(
        tree(refused(&[CATALOG, root, under, page])),
        PageTree::Repeated { page: 1 }
    );
}

#[test]
fn pages_that_aren_t_what_lopdf_lists_are_refused() {
    // A leaf with no /Type is a page by ISO 32000-2's rules, and no page of lopdf's.
    let node = "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>";
    let bare = "<< /Parent 2 0 R >>";
    let tree = refused(&[CATALOG, node, PAGE, bare]);
    assert!(matches!(
        tree,
        Error::PageTree(PageTree::Listed { page: 1 })
    ));
    let tree = refused(&[CATALOG, node, bare, PAGE]);
    assert!(matches!(
        tree,
        Error::PageTree(PageTree::Listed { page: 0 })
    ));
    // A node with no /Type: lopdf doesn't go into it.
    let under = "<< /Kids [4 0 R] /Parent 2 0 R >>";
    let node = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
    let page = "<< /Type /Page /Parent 3 0 R >>";
    let tree = refused(&[CATALOG, node, under, page]);
    assert!(matches!(
        tree,
        Error::PageTree(PageTree::Listed { page: 0 })
    ));
}

#[test]
fn a_tree_the_two_rules_read_apart_is_refused_as_the_family_s_is() {
    let rules = |objects: &[&str], wanted: PageTree, page: usize| {
        let bytes = file(objects, ROOT);
        assert_eq!(
            laid_out(&bytes).unwrap_err().to_string(),
            Error::PageTree(wanted).to_string()
        );
        let family = family::linearize_bytes(&bytes, &Options::DEFAULT);
        assert!(
            matches!(
                family,
                Err(family::Error::PageTree(family::PageTree::Rules { page: at })) if at == page
            ),
            "{family:?}"
        );
    };
    let own = "/MediaBox [0 0 9 9] /Resources << >>";
    let under = format!("<< /Type /Page /Parent 2 0 R {own} >>");
    // A root with /Kids that says it's a page: the one page to ISO's, a node to the others.
    let root = format!("<< /Type /Page /Kids [3 0 R] /Count 1 {own} >>");
    let wanted = PageTree::Listed { page: 0 };
    rules(&[CATALOG, &root, &under], wanted, 0);
    // A kid with /Kids that says it's a page: a page to ISO's, a node to PDFium's.
    let root = "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>";
    let kid = format!("<< /Type /Page /Parent 2 0 R /Kids [4 0 R] {own} >>");
    let its = format!("<< /Type /Page /Parent 3 0 R {own} >>");
    let objects = [CATALOG, root, &kid, &its, &under];
    rules(&objects, PageTree::Rules { page: 0 }, 0);
    // One with no /Kids that says it's a node: no place to ISO's, a place to PDFium's.
    let root = "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 1 >>";
    let objects = [CATALOG, root, "<< /Type /Pages /Parent 2 0 R >>", &under];
    rules(&objects, PageTree::Rules { page: 0 }, 0);
    // A kid whose object is a reference, to a page listed in its own right too.
    let root = "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>";
    let objects = [CATALOG, root, "4 0 R", &under];
    rules(&objects, PageTree::Rules { page: 0 }, 0);
}

#[test]
fn a_root_that_says_it_s_a_page_is_refused_where_lopdf_lists_none() {
    let page = "<< /Type /Page /MediaBox [0 0 9 9] /Resources << >> >>";
    assert!(matches!(
        refused(&[CATALOG, page]),
        Error::PageTree(PageTree::Listed { page: 0 })
    ));
    let page = "<< /MediaBox [0 0 9 9] /Resources << >> >>";
    assert!(matches!(
        refused(&[CATALOG, page]),
        Error::PageTree(PageTree::Listed { page: 0 })
    ));
}

#[test]
fn a_parent_chain_that_goes_round_or_is_in_place_is_refused() {
    let node = "<< /Type /Pages /Kids [3 0 R] /Count 1 /Parent 2 0 R >>";
    assert!(matches!(
        refused(&[CATALOG, node, PAGE]),
        Error::PageTree(PageTree::Parents { page: 0 })
    ));
    // In place, with nothing to give, it's no matter; giving something it's refused.
    let node = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
    let page = "<< /Type /Page /Parent << /MediaBox [0 0 9 9] >> >>";
    assert!(matches!(
        refused(&[CATALOG, node, page]),
        Error::PageTree(PageTree::DirectNode { page: 0 })
    ));
    let page = "<< /Type /Page /Parent << /Mine true >> >>";
    let bytes = file(&[CATALOG, node, page], ROOT);
    laid_out(&bytes).unwrap();
}

#[test]
fn a_null_nearer_a_page_than_a_value_is_refused_as_the_family_s_is() {
    let node = "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 9 9] /Rotate 90 >>";
    let attribute = |objects: &[&str], wanted: &'static str| {
        let tree = file(objects, ROOT);
        assert!(matches!(
            laid_out(&tree),
            Err(Error::PageTree(PageTree::Attribute { page: 0, attribute })) if attribute == wanted
        ));
        let family = family::linearize_bytes(&tree, &Options::DEFAULT);
        assert!(matches!(
            family,
            Err(family::Error::PageTree(family::PageTree::Attribute { page: 0, attribute }))
                if attribute == wanted
        ));
    };
    let page = "<< /Type /Page /Parent 2 0 R /Rotate null /Resources << >> >>";
    attribute(&[CATALOG, node, page], "Rotate");
    // By a reference to a null object, too.
    let page = "<< /Type /Page /Parent 2 0 R /Rotate 4 0 R /Resources << >> >>";
    attribute(&[CATALOG, node, page, "null"], "Rotate");
    // On a node between.
    let root = "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 9 9] >>";
    let under = "<< /Type /Pages /Parent 2 0 R /Kids [4 0 R] /Count 1 /MediaBox null >>";
    let page = "<< /Type /Page /Parent 3 0 R /Resources << >> >>";
    attribute(&[CATALOG, root, under, page], "MediaBox");
    // With no value above it, a null is none by either rule.
    let page = "<< /Type /Page /Parent 2 0 R /CropBox null /Resources << >> >>";
    let tree = file(&[CATALOG, node, page], ROOT);
    holds(&laid_out(&tree).unwrap());
    same_numbers_as_the_family(&tree);
    // A reference to nothing is passed by both.
    let page = "<< /Type /Page /Parent 2 0 R /Rotate 9 0 R /Resources << >> >>";
    let tree = file(&[CATALOG, node, page], ROOT);
    holds(&laid_out(&tree).unwrap());
    same_numbers_as_the_family(&tree);
}

#[test]
fn what_going_down_kids_reads_otherwise_than_up_parent_is_refused_as_the_family_s_is() {
    let root = "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>";
    let node = |under: u32, width: u32| {
        format!(
            "<< /Type /Pages /Parent 2 0 R /Kids [{under} 0 R] /Count 1 /MediaBox [0 0 {width} {width}] >>"
        )
    };
    let page = |parent: u32| format!("<< /Type /Page /Parent {parent} 0 R /Resources << >> >>");
    let (a, b, c, d) = (node(5, 100), node(6, 200), page(4), page(3));
    let apart = file(&[CATALOG, root, &a, &b, &c, &d], ROOT);
    assert!(matches!(
        laid_out(&apart),
        Err(Error::PageTree(PageTree::Kids {
            page: 0,
            attribute: "MediaBox"
        }))
    ));
    assert!(matches!(
        family::linearize_bytes(&apart, &Options::DEFAULT),
        Err(family::Error::PageTree(family::PageTree::Kids {
            page: 0,
            attribute: "MediaBox"
        }))
    ));
    // The same value at either place is what it was by either way.
    let (a, b) = (node(5, 100), node(6, 100));
    let alike = file(&[CATALOG, root, &a, &b, &c, &d], ROOT);
    holds(&laid_out(&alike).unwrap());
    same_numbers_as_the_family(&alike);
    let above = [
        CATALOG,
        "<< /Type /Pages /Parent 4 0 R /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /Resources << >> >>",
        "<< /Type /Pages /MediaBox [0 0 9 9] >>",
    ];
    assert!(matches!(
        refused(&above),
        Error::PageTree(PageTree::Kids {
            page: 0,
            attribute: "MediaBox"
        })
    ));
}

#[test]
fn an_object_that_s_only_a_reference_to_nothing_has_no_body_and_no_entry() {
    let root = "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 9 9] /Resources << >> >>";
    let later = "<< /Type /Page /Parent 2 0 R /X 6 0 R >>";
    let start = [CATALOG, root, PAGE, later, later];
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
        let out = laid_out(&bytes).unwrap();
        holds(&out);
        same_numbers_as_the_family(&bytes);
        let (_, doc) = read_back(&out);
        assert!(
            !doc.objects().any(|(_, object)| object.is_null()),
            "{shape:?}"
        );
    }
}

#[test]
fn a_parent_that_s_a_page_keeps_what_it_holds_and_nothing_s_said_of_it() {
    let own = "/MediaBox [0 0 9 9] /Resources << >>";
    let root = "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>";
    let page = format!("<< /Type /Page /Parent 2 0 R {own} >>");
    let under_page = format!("<< /Type /Page /Parent 3 0 R {own} >>");
    let bytes = file(&[CATALOG, root, &page, &under_page], ROOT);
    holds(&laid_out(&bytes).unwrap());
    same_numbers_as_the_family(&bytes);
}

#[test]
fn a_page_the_tree_doesn_t_list_gets_what_it_inherited_from_a_node_that_loses_it() {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /Foo 4 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 9 9] /Resources 5 0 R /Rotate 90 >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 3 3] /Mark (stray) >>",
        "<< /Mark (resources) >>",
    ];
    let bytes = file(&objects, ROOT);
    let out = laid_out(&bytes).unwrap();
    holds(&out);
    same_numbers_as_the_family(&bytes);
    let (_, doc) = read_back(&out);
    let stray = doc.objects().find_map(|(_, object)| {
        let dict = object.as_dict()?;
        (text(dict.get(b"Mark")?) == b"stray").then_some(dict)
    });
    let stray = stray.unwrap();
    assert_eq!(stray.get(b"Rotate").unwrap().as_integer(), Some(90));
    assert!(stray.has(b"Resources"));
}

/// What says it's a page, isn't listed, and is a listed page's `/Parent` with kids is a
/// node like any: it's given nothing to hold.
#[test]
fn a_stray_page_that_s_a_node_is_given_nothing() {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /Foo 4 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 9 9] /Resources << >> >>",
        "<< /Type /Page /Parent 4 0 R >>",
        "<< /Type /Page /Parent 2 0 R /Kids [] /Mark (between) >>",
    ];
    let bytes = file(&objects, ROOT);
    holds(&laid_out(&bytes).unwrap());
    same_numbers_as_the_family(&bytes);
}

/// `objects` as a file: what lopdf holds of them, the adapter's refusal, `family`'s too.
fn refused_as_the_family_refuses(objects: &[&str]) -> Error {
    let why = refused(objects);
    assert!(family::linearize_bytes(&file(objects, ROOT), &Options::DEFAULT).is_err());
    why
}

#[test]
fn what_the_tree_is_read_by_that_doesn_t_read_is_refused_as_the_family_s_is() {
    let unread = |objects: &[&str]| {
        assert!(
            matches!(
                refused_as_the_family_refuses(objects),
                Error::PageTree(PageTree::Unread { .. })
            ),
            "{objects:?}"
        );
    };
    let own = "/MediaBox [0 0 9 9] /Resources << >>";
    let root = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
    // A page's own value, in a loop, of itself and by way of another.
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 9] /Resources 4 0 R >>";
    unread(&[CATALOG, root, page, "4 0 R"]);
    unread(&[CATALOG, root, page, "5 0 R", "4 0 R"]);
    // Past the chain's limit, where lopdf's own limit is further.
    let mut chain: Vec<String> = (4..4 + 40)
        .map(|number| format!("{} 0 R", number + 1))
        .collect();
    chain.push("<< >>".to_string());
    let mut objects = vec![CATALOG, root, page];
    objects.extend(chain.iter().map(String::as_str));
    unread(&objects);
    // A node's, whether or not a page takes it.
    let node = "<< /Type /Pages /Kids [3 0 R] /Count 1 /Rotate 4 0 R >>";
    let page = format!("<< /Type /Page /Parent 2 0 R {own} >>");
    unread(&[CATALOG, node, &page, "4 0 R"]);
    let root = "<< /Type /Pages /Kids [3 0 R] /Count 1 /Rotate 4 0 R >>";
    let page = format!("<< /Type /Page /Parent 2 0 R {own} /Rotate 90 >>");
    unread(&[CATALOG, root, &page, "4 0 R"]);
    let root = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
    // /Kids, /Type and a /Parent.
    let kids = "<< /Type /Pages /Kids 4 0 R /Count 1 >>";
    unread(&[CATALOG, kids, &page, "4 0 R"]);
    let kind = format!("<< /Type 4 0 R /Parent 2 0 R {own} >>");
    unread(&[CATALOG, root, &kind, "4 0 R"]);
    let parent = format!("<< /Type /Page /Parent 4 0 R {own} >>");
    unread(&[CATALOG, root, &parent, "4 0 R"]);
    // A box's corner, and a unit, which a reader goes on to read.
    let corner = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 4 0 R] /Resources << >> >>";
    unread(&[CATALOG, root, corner, "4 0 R"]);
    let unit = format!("<< /Type /Page /Parent 2 0 R {own} /UserUnit 4 0 R >>");
    unread(&[CATALOG, root, &unit, "4 0 R"]);
    // Naming nothing isn't that: it's absent.
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 9 9] /Resources 9 0 R >>";
    let bytes = file(&[CATALOG, root, page], ROOT);
    laid_out(&bytes).unwrap();
}

#[test]
fn a_tree_past_its_depth_going_down_is_refused_as_the_family_s_is() {
    let mut objects = vec![CATALOG.to_string()];
    for number in 2..1100 {
        objects.push(format!(
            "<< /Type /Pages /Kids [{} 0 R] /Count 1 >>",
            number + 1
        ));
    }
    objects.push("<< /Type /Page /MediaBox [0 0 9 9] /Resources << >> >>".to_string());
    let objects: Vec<&str> = objects.iter().map(String::as_str).collect();
    assert!(matches!(
        refused_as_the_family_refuses(&objects),
        Error::PageTree(PageTree::TooDeep)
    ));
}

#[test]
fn what_the_options_say_is_gone_by() {
    // A first page the options name, as the family's do.
    let bytes = built(&written::two_pages(""));
    let mut options = adapter::Options::DEFAULT;
    options.first_page = adapter::FirstPage::Index(1);
    let mut out = Vec::new();
    adapter::linearize(&mut out, &loaded(&bytes), &options).unwrap();
    assert!(checked(&out).0.holds);
    let mut family = Options::DEFAULT;
    family.first_page = family::FirstPage::Index(1);
    let theirs = family::linearize_bytes(&bytes, &family).unwrap();
    assert_eq!(numbers(&out).len(), numbers(&theirs).len());
    options.first_page = adapter::FirstPage::Index(2);
    let past = adapter::linearize(&mut Vec::new(), &loaded(&bytes), &options);
    assert!(matches!(
        past,
        Err(Error::Plan(plan::Error::FirstPagePast { .. }))
    ));
    // Caps the options lower.
    let mut options = adapter::Options::DEFAULT;
    options.limits.pages = 1;
    let over = adapter::linearize(&mut Vec::new(), &loaded(&bytes), &options);
    assert!(matches!(
        over,
        Err(Error::Plan(plan::Error::TooManyPages { .. }))
    ));
}

#[test]
fn a_chain_of_parents_too_long_to_follow_is_refused() {
    // A chain past the depth, the page at its foot; the root lists it directly.
    let mut objects = vec![
        CATALOG.to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 4 0 R >>".to_string(),
    ];
    for number in 4..1100 {
        objects.push(format!("<< /Type /Pages /Parent {} 0 R >>", number + 1));
    }
    objects.push("<< /Type /Pages >>".to_string());
    let objects: Vec<&str> = objects.iter().map(String::as_str).collect();
    assert!(matches!(
        refused(&objects),
        Error::PageTree(PageTree::Parents { page: 0 })
    ));
}

#[test]
fn no_catalog_and_no_pages_are_refused() {
    let page = "<< /Type /Page >>";
    let bytes = file(&[page], "/Root 9 0 R");
    assert!(matches!(laid_out(&bytes), Err(Error::NoCatalog)));
    let bytes = file(&[page], "/Root (a string)");
    let doc = loaded(&bytes);
    let mut out = Vec::new();
    assert!(matches!(
        adapter::linearize(&mut out, &doc, &adapter::Options::DEFAULT),
        Err(Error::NoCatalog)
    ));
    let node = "<< /Type /Pages /Kids [] /Count 0 >>";
    assert!(matches!(refused(&[CATALOG, node]), Error::Plan(_)));
}

/// `doc` refused for its encryption, by both the adapter's calls, `out` as given.
fn refused_as_encrypted(doc: &Document) {
    let mut out = b"given".to_vec();
    assert!(matches!(
        adapter::linearize(&mut out, doc, &adapter::Options::DEFAULT),
        Err(Error::Encrypted)
    ));
    assert_eq!(out, b"given");
    assert!(matches!(
        adapter::graph(doc, &adapter::Options::DEFAULT),
        Err(Error::Encrypted)
    ));
}

#[test]
fn a_file_lopdf_opened_with_the_empty_password_is_refused() {
    // Which it decrypts, and takes /Encrypt out of its trailer, so it looks plain.
    let plain = built(&two_pages(""));
    let mut decrypted = 0;
    for (name, key) in KEYS {
        let doc = loaded(&encrypted_as(key, &plain));
        assert!(doc.was_encrypted(), "{name}");
        assert!(!doc.trailer.has(b"Encrypt"), "{name}");
        decrypted += usize::from(!doc.is_encrypted());
        refused_as_encrypted(&doc);
    }
    assert_eq!(decrypted, KEYS.len());
}

#[test]
fn a_file_lopdf_didn_t_open_is_refused() {
    let plain = built(&two_pages(""));
    let reading = Reading::open(&plain[..], &read::Options::DEFAULT).unwrap();
    let doc = reading.to_document().unwrap();
    let prepared = Password("user").prepared().unwrap();
    let new = Encryptor::create(&Protection::new(&prepared)).unwrap();
    let mut encrypted = Vec::new();
    let options = write::Options::DEFAULT;
    write::document(&mut encrypted, &doc, Some(&new), Some(&reading), &options).unwrap();
    let doc = loaded(&encrypted);
    assert!(doc.is_encrypted());
    refused_as_encrypted(&doc);
}

#[test]
fn a_document_marked_encrypted_by_hand_is_refused_however_it_says_so() {
    let plain = built(&two_pages(""));
    // By reference, as lopdf's own `is_encrypted` looks for it.
    let mut doc = loaded(&plain);
    let dict = doc.add_object(dictionary! { "Filter" => "Standard" });
    doc.trailer.set("Encrypt", dict);
    assert!(doc.is_encrypted());
    refused_as_encrypted(&doc);
    // In place, which it doesn't.
    let mut doc = loaded(&plain);
    let dict = dictionary! { "Filter" => "Standard" };
    doc.trailer.set("Encrypt", dict);
    assert!(!doc.is_encrypted() && !doc.was_encrypted());
    refused_as_encrypted(&doc);
    // Naming no dictionary, which lopdf's reader leaves as it is.
    let mut doc = loaded(&plain);
    doc.trailer.set("Encrypt", LopdfObject::Reference((99, 0)));
    assert!(!doc.is_encrypted() && !doc.was_encrypted());
    refused_as_encrypted(&doc);
}

#[test]
fn a_real_past_what_a_file_can_hold_is_refused() {
    let mut doc = loaded(&built(&two_pages("")));
    doc.catalog_mut().unwrap().set("Bad", f32::INFINITY);
    let mut out = Vec::new();
    let why = adapter::linearize(&mut out, &doc, &adapter::Options::DEFAULT).unwrap_err();
    assert!(
        matches!(
            why,
            Error::NonFinite {
                id: ObjectId {
                    number: 1,
                    generation: 0
                }
            }
        ),
        "{why}"
    );
    assert!(out.is_empty());
}

#[test]
fn a_stream_lopdf_couldn_t_read_is_refused_not_written_empty() {
    // Its /Length names nothing, and it's past where lopdf's reader looks.
    let mut objects = two_pages("");
    objects[4] = "<< /Length 99 0 R >>\nstream\n0 0 m\nendstream".to_string();
    let doc = loaded(&built(&objects));
    let Ok(LopdfObject::Stream(stream)) = doc.get_object((5, 0)) else {
        panic!("lopdf has no stream there");
    };
    assert!(stream.content.is_empty());
    let mut out = Vec::new();
    assert!(matches!(
        adapter::linearize(&mut out, &doc, &adapter::Options::DEFAULT),
        Err(Error::UnreadStream {
            id: ObjectId {
                number: 5,
                generation: 0
            }
        })
    ));
    assert!(out.is_empty());
}

#[test]
fn its_own_file_laid_out_again_holds() {
    let once = laid_out(&everything("")).unwrap();
    let twice = laid_out(&once).unwrap();
    holds(&twice);
    assert_eq!(numbers(&once).len(), numbers(&twice).len());
}

#[test]
fn a_document_built_in_lopdf_is_laid_out() {
    let mut doc = Document::with_version("1.5");
    let pages = doc.new_object_id();
    let content = doc.add_object(lopdf::Stream::new(dictionary! {}, b"0 0 m".to_vec()));
    let page = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages,
        "Contents" => content,
    });
    doc.objects.insert(
        pages,
        LopdfObject::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page.into()],
            "Count" => 1,
            "MediaBox" => vec![0.into(), 0.into(), 9.into(), 9.into()],
            "Resources" => dictionary! {},
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    adapter::linearize(&mut out, &doc, &adapter::Options::DEFAULT).unwrap();
    holds(&out);
    assert!(out.starts_with(b"%PDF-1.5\n"));
}
