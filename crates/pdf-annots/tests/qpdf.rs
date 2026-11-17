//! Cross-checks `pdf-annots`'s annotation builders, synthesized Form XObject
//! appearances, and page flattening output against `qpdf --check` when run via
//! `just test-qpdf`.

use std::path::Path;

use pdf_annots::pdf_content_streams::Color;
use pdf_annots::pdf_gfx::{Point, Rect};
use pdf_annots::pdf_objects::{Dict, Object, Stream, Version, dict};
use pdf_annots::pdf_semantics::pages::{FANOUT, write};
use pdf_annots::{
    AnnotColor, Border, BuiltAnnot, DefaultAppearance, Dest, FlattenOpts, LineEnding, PaintStyle,
    QuadPoints, Quadding, ShapeKind, StampName, TextMarkupKind, plan_flatten, push_page_annot,
};
use pdf_fixtures::qpdf::Qpdf;
use pdf_structure::Source;
use pdf_structure::source::Document;

fn check_clean_with_qpdf(bytes: &[u8], label: &str) {
    let folder = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(folder).unwrap();
    let path = folder.join(format!("annots-qpdf-{label}.pdf"));
    std::fs::write(&path, bytes).unwrap();
    let qpdf = Qpdf::new(&path);
    if let Err(why) = qpdf.version() {
        panic!("this test needs qpdf ({why}): install it, or name it in PDF_LINT_QPDF");
    }
    let ran = qpdf
        .exec(&["--check".to_string(), path.display().to_string()])
        .expect("qpdf runs");
    let said =
        String::from_utf8_lossy(&ran.stdout).to_string() + &String::from_utf8_lossy(&ran.stderr);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(ran.code, Some(0), "{label}: {said}");
    assert!(!said.contains("WARNING"), "{label}: {said}");
}

#[test]
#[ignore = "needs qpdf: `just test-qpdf`"]
fn qpdf_accepts_synthesized_annotations_and_flattened_pages() {
    let mut doc = Document::new(Version::new(1, 7), dict! {});
    let p1 = doc.new_id();
    let p2 = doc.new_id();
    let tree = write(
        &[p1, p2],
        FANOUT,
        &pdf_annots::pdf_semantics::Limits::DEFAULT,
        || doc.new_id(),
    )
    .unwrap();
    for (id, node) in tree.nodes {
        doc.set(id, node);
    }

    let base_content = doc.add(Stream::new(
        dict! {},
        b"0.2 g 50 700 100 20 re f\n".to_vec(),
    ));
    let mut page1 = dict! {
        "Type" => Object::name("Page"),
        "Parent" => tree.parents[0],
        "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(612), Object::from(792)],
        "Contents" => base_content,
        "Resources" => dict! {},
    };
    let page2 = dict! {
        "Type" => Object::name("Page"),
        "Parent" => tree.parents[1],
        "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(612), Object::from(792)],
        "Resources" => dict! {},
    };

    let da = DefaultAppearance::new(b"Helv".to_vec(), 12.0, Some(Color::Rgb([0.0, 0.2, 0.6])));
    for built in [
        BuiltAnnot::link_dest(Rect::new(50.0, 700.0, 150.0, 720.0), &Dest::fit(p2)),
        BuiltAnnot::text_markup(
            TextMarkupKind::Highlight,
            &[QuadPoints([
                72.0, 660.0, 180.0, 660.0, 72.0, 646.0, 180.0, 646.0,
            ])],
            AnnotColor::Rgb([1.0, 0.95, 0.2]),
            0.5,
        ),
        BuiltAnnot::shape(
            ShapeKind::Square,
            Rect::new(72.0, 560.0, 160.0, 610.0),
            &PaintStyle::new(
                Some(AnnotColor::Rgb([0.8, 0.1, 0.1])),
                Some(AnnotColor::Rgb([1.0, 0.9, 0.9])),
            ),
        ),
        BuiltAnnot::line(
            Point::new(72.0, 500.0),
            Point::new(180.0, 530.0),
            (LineEnding::None, LineEnding::ClosedArrow),
            &PaintStyle::new(Some(AnnotColor::Gray(0.1)), Some(AnnotColor::Gray(0.1))),
        ),
        BuiltAnnot::stamp(
            Rect::new(72.0, 420.0, 220.0, 468.0),
            StampName::Final,
            None,
            AnnotColor::Rgb([0.1, 0.5, 0.2]),
            1.0,
        ),
        BuiltAnnot::free_text(
            Rect::new(72.0, 340.0, 240.0, 390.0),
            "Verified with qpdf",
            &da,
            Quadding::Left,
            &PaintStyle::new(
                Some(AnnotColor::Gray(0.4)),
                Some(AnnotColor::Rgb([1.0, 1.0, 0.95])),
            )
            .with_border(Border::default()),
        ),
    ] {
        let aid = built.write_into(|obj| doc.add(obj));
        assert!(push_page_annot(&mut page1, aid));
    }

    doc.set(p1, page1);
    doc.set(p2, page2);
    let cat = doc.add(dict! { "Type" => Object::name("Catalog"), "Pages" => tree.root });
    doc.trailer_mut().dict_mut().set("Root", cat);

    let mut unflattened_bytes = Vec::new();
    pdf_structure::write::document(
        &mut unflattened_bytes,
        &doc,
        None,
        None,
        &pdf_structure::write::Options::DEFAULT,
    )
    .unwrap();
    check_clean_with_qpdf(&unflattened_bytes, "unflattened");

    // Flatten page 1 and verify `qpdf --check` passes cleanly on the flattened PDF too!
    let plan = {
        let p1_dict = doc.get(p1).unwrap().as_dict().unwrap();
        plan_flatten(
            &doc,
            p1_dict,
            Some(p1),
            p1_dict.get(b"Resources"),
            FlattenOpts::DEFAULT,
            &pdf_annots::Limits::DEFAULT,
        )
        .unwrap()
    };
    assert_eq!(plan.drawn_count, 5);
    let combined = plan.combine_with_page(b"0.2 g 50 700 100 20 re f\n");
    let new_content_id = doc.add(Stream::new(dict! {}, combined));
    let p1_mut = doc.get_mut(p1).unwrap().as_dict_mut().unwrap();
    p1_mut.set("Contents", new_content_id);
    p1_mut.remove(b"Annots");
    let mut xo = Dict::new();
    for (name, id) in plan.xobject_bindings {
        xo.set(name, id);
    }
    p1_mut.set("Resources", dict! { "XObject" => xo });

    let mut flattened_bytes = Vec::new();
    pdf_structure::write::document(
        &mut flattened_bytes,
        &doc,
        None,
        None,
        &pdf_structure::write::Options::DEFAULT,
    )
    .unwrap();
    check_clean_with_qpdf(&flattened_bytes, "flattened");
}
