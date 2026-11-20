//! A document of appearances made for its text fields' widgets, each turned
//! by its rotation and one in a font its default resources lack, bound into
//! it with `pdf-structure` and written: read back, each widget's text is
//! where its appearance puts it (`pdf_gfx::TextPage`); and `qpdf --check`
//! finds it clean when run via `just test-qpdf`.

#![cfg(feature = "text-layout")]

use std::collections::HashMap;
use std::path::Path;

use pdf_fixtures::qpdf::Qpdf;
use pdf_forms::appearance::{Appearance, text_for_widget, widget_text};
use pdf_forms::pdf_content_streams::{Concat, ResBody, ResId};
use pdf_forms::pdf_fonts::{Fonts, Std14};
use pdf_forms::pdf_gfx::{self, Rect, TextPage, TextPageOpts};
use pdf_forms::pdf_objects::pdf_tokenizer::Rules;
use pdf_forms::pdf_objects::{Dict, Object, ObjectId, Resolve, Stream, Version, dict};
use pdf_forms::{FieldFlags, FieldKind, Form, Limits};
use pdf_structure::read::{self, Reading};
use pdf_structure::source::Document;

/// A widget made in the document: its object, rectangle, the text it shows,
/// and its rotation.
struct Placed {
    id: ObjectId,
    rect: Rect,
    shown: String,
    rotation: i32,
}

fn numbers(values: &[f32]) -> Object {
    Object::from(values.iter().map(|&v| Object::from(v)).collect::<Vec<_>>())
}

/// A text field that is its own widget, at `rect` on `page`, turned by
/// `rotation`, with `value` (none where it's empty) and `more` of its own.
fn text_field(
    page: ObjectId,
    name: &str,
    value: &str,
    rect: [f32; 4],
    rotation: i32,
    more: Dict,
) -> Dict {
    let mut field = dict! {
        "Type" => Object::name("Annot"),
        "Subtype" => Object::name("Widget"),
        "FT" => Object::name("Tx"),
        "T" => Object::text(name),
        "Rect" => numbers(&rect),
        "F" => 4,
        "P" => page,
    };
    if !value.is_empty() {
        field.set("V", Object::text(value));
    }
    let mut mk = match more.get(b"MK").and_then(Object::as_dict) {
        Some(mk) => mk.clone(),
        None => Dict::new(),
    };
    mk.set("R", rotation);
    for (key, value) in more.iter() {
        field.set(key.to_vec(), value.clone());
    }
    field.set("MK", mk);
    field
}

/// The document, written, and each widget made in it.
fn document() -> (Vec<u8>, Vec<Placed>) {
    let mut doc = Document::new(Version::new(1, 7), dict! {});
    let (page, pages) = (doc.new_id(), doc.new_id());
    let helvetica = doc.add(Std14::Helvetica.dict());

    let fields = [
        (
            "city",
            "Trenton",
            [72.0, 700.0, 272.0, 720.0],
            0,
            dict! {
                "MK" => dict! { "BC" => numbers(&[0.0]) },
                "BS" => dict! { "W" => 1, "S" => Object::name("S") },
            },
        ),
        (
            "note",
            "Turned a quarter, and wrapped across its lines",
            [72.0, 540.0, 112.0, 680.0],
            90,
            dict! {
                "Ff" => 1 << 12,
                "DA" => Object::text("/Helv 10 Tf 0 0 0.502 rg"),
                "MK" => dict! { "BG" => numbers(&[0.9]), "BC" => numbers(&[0.0, 0.0, 1.0]) },
                "BS" => dict! { "W" => 2, "S" => Object::name("B") },
            },
        ),
        (
            "code",
            "A1B2C3",
            [300.0, 700.0, 420.0, 720.0],
            180,
            dict! {
                "Ff" => 1 << 24,
                "MaxLen" => 6,
                "Q" => 1,
                "MK" => dict! { "BC" => numbers(&[1.0, 0.0, 0.0]) },
                "BS" => dict! { "W" => 1, "S" => Object::name("D"), "D" => numbers(&[3.0, 1.0]) },
            },
        ),
        (
            "pin",
            "hunter2",
            [300.0, 600.0, 340.0, 680.0],
            270,
            dict! {
                "Ff" => 1 << 13,
                "MK" => dict! { "BG" => numbers(&[0.1, 0.2, 0.3, 0.4]) },
                "BS" => dict! { "W" => 1, "S" => Object::name("I") },
            },
        ),
        (
            "times",
            "Drawn in Times-Roman",
            [72.0, 480.0, 272.0, 500.0],
            0,
            dict! {
                "DA" => Object::text("/TiRo 12 Tf 0 g"),
                "Q" => 2,
                "MK" => dict! { "BC" => numbers(&[0.0]) },
                "BS" => dict! { "W" => 1, "S" => Object::name("U") },
            },
        ),
        ("empty", "", [72.0, 440.0, 272.0, 460.0], 0, Dict::new()),
    ];
    let mut widgets = Vec::new();
    for (name, value, rect, rotation, more) in fields {
        let id = doc.add(text_field(page, name, value, rect, rotation, more));
        widgets.push(id);
    }
    let refs = || {
        Object::from(
            widgets
                .iter()
                .map(|&id| Object::from(id))
                .collect::<Vec<_>>(),
        )
    };
    let form = doc.add(dict! {
        "Fields" => refs(),
        "DA" => Object::text("/Helv 0 Tf 0 g"),
        "DR" => dict! { "Font" => dict! { "Helv" => helvetica } },
    });
    doc.set(
        page,
        dict! {
            "Type" => Object::name("Page"),
            "Parent" => pages,
            "MediaBox" => numbers(&[0.0, 0.0, 612.0, 792.0]),
            "Resources" => dict! {},
            "Annots" => refs(),
        },
    );
    doc.set(
        pages,
        dict! { "Type" => Object::name("Pages"), "Kids" => vec![Object::from(page)], "Count" => 1 },
    );
    let catalog = doc.add(dict! {
        "Type" => Object::name("Catalog"),
        "Pages" => pages,
        "AcroForm" => form,
    });
    doc.trailer_mut().dict_mut().set("Root", catalog);

    // Each widget made, with the name its `/DA` gives a font the default
    // resources lack, which the caller adds to them.
    let limits = Limits::DEFAULT;
    let mut appearances: Vec<(Appearance, Option<Vec<u8>>, Placed)> = Vec::new();
    {
        let catalog = doc.get_dict(catalog).unwrap();
        let form = Form::read(&doc, catalog, Rules::Iso, &limits)
            .unwrap()
            .unwrap();
        for field in form.fields() {
            assert_eq!(field.kind(), Some(FieldKind::Text));
            for widget in field.widgets() {
                let read =
                    widget_text(field, widget, &doc, Rules::Iso, None, &limits, &mut ()).unwrap();
                let shown = match field.flags().contains(FieldFlags::PASSWORD) {
                    true => "*".repeat(read.field().text.chars().count()),
                    false => read.field().text.to_string(),
                };
                let mut faults = Vec::new();
                let made = text_for_widget(
                    &form,
                    field,
                    widget,
                    &doc,
                    Rules::Iso,
                    None,
                    &limits,
                    &mut faults,
                )
                .unwrap();
                // Only the font the resources lack is told.
                assert_eq!(faults.is_empty(), !made.font.is_fallback());
                let added = match made.font.is_fallback() {
                    true => Some(made.font_name.expect("every /DA here names its font")),
                    false => None,
                };
                let rect = widget.annot(&doc, Rules::Iso, &limits).unwrap().rect();
                let rotation = read.look().rotation;
                let id = widget.id().unwrap();
                let place = Placed {
                    id,
                    rect,
                    shown,
                    rotation,
                };
                appearances.push((made.appearance, added, place));
            }
        }
    }

    // Bound: each font the appearance needs, a fallback added to the
    // default resources too, once; each appearance a form XObject of its own.
    let mut written: HashMap<ResId, ObjectId> = HashMap::new();
    let mut placed = Vec::new();
    for (appearance, added, place) in appearances {
        let mut fonts = Dict::new();
        for need in &appearance.needs.0 {
            let font = match need.res.body() {
                ResBody::Written(id) => *id,
                ResBody::Objects(objects) => *written
                    .entry(need.res.id())
                    .or_insert_with(|| doc.add(objects[0].clone())),
                other => panic!("a resource the maker doesn't name: {other:?}"),
            };
            fonts.set(need.name.clone(), font);
            if let Some(name) = &added {
                let form = doc.get_dict_mut(form).unwrap();
                let dr = form.get_mut(b"DR").and_then(Object::as_dict_mut).unwrap();
                let fonts = dr.get_mut(b"Font").and_then(Object::as_dict_mut).unwrap();
                fonts.set(name.clone(), font);
            }
        }
        let [a, b, c, d, e, f] = appearance.matrix.to_array();
        let bbox = appearance.bbox;
        let stream = Stream::new(
            dict! {
                "Type" => Object::name("XObject"),
                "Subtype" => Object::name("Form"),
                "BBox" => numbers(&[bbox.x0, bbox.y0, bbox.x1, bbox.y1]),
                "Matrix" => numbers(&[a, b, c, d, e, f]),
                "Resources" => dict! { "Font" => fonts },
            },
            appearance.content,
        );
        let ap = doc.add(stream);
        doc.get_dict_mut(place.id)
            .unwrap()
            .set("AP", dict! { "N" => ap });
        placed.push(place);
    }

    let mut bytes = Vec::new();
    pdf_structure::write::document(
        &mut bytes,
        &doc,
        None,
        None,
        &pdf_structure::write::Options::DEFAULT,
    )
    .unwrap();
    (bytes, placed)
}

/// Each widget's appearance, read back from the file written, drawn on its
/// rectangle as a viewer draws it (its box turned by its matrix and moved
/// to the rectangle's corner, ISO 32000-2, §12.5.5): the text it shows is
/// read there, each character inside the rectangle.
#[test]
fn made_appearances_read_back_as_their_text_on_their_widgets() {
    let (bytes, placed) = document();
    let reading = Reading::open(&bytes[..], &read::Options::DEFAULT).unwrap();
    for place in &placed {
        let widget = (&reading)
            .resolve(place.id)
            .and_then(Object::as_dict)
            .unwrap();
        let ap = widget
            .get(b"AP")
            .and_then(Object::as_dict)
            .and_then(|ap| ap.get(b"N"))
            .and_then(Object::as_reference)
            .expect("each widget has its appearance");
        let content = format!("q 1 0 0 1 {} {} cm /Ap Do Q", place.rect.x0, place.rect.y0);
        let resources = Object::from(dict! { "XObject" => dict! { "Ap" => ap } });
        let page = TextPage::extract(
            &reading,
            &Concat::new(&[content.as_bytes()]),
            Some(&resources),
            &pdf_gfx::Limits::DEFAULT,
            &Fonts::default(),
            TextPageOpts::default(),
        )
        .unwrap();

        let drawn: Vec<_> = page.chars().iter().filter(|c| !c.generated).collect();
        for ch in &drawn {
            let at = ch.origin;
            let inside = place.rect.x0 - 0.01 <= at.x
                && at.x <= place.rect.x1 + 0.01
                && place.rect.y0 - 0.01 <= at.y
                && at.y <= place.rect.y1 + 0.01;
            assert!(inside, "{:?} at {at:?} outside {:?}", ch.ch, place.rect);
        }
        let mut read: Vec<char> = drawn
            .iter()
            .map(|c| c.ch)
            .filter(|c| !c.is_whitespace())
            .collect();
        let mut shown: Vec<char> = place.shown.chars().filter(|c| !c.is_whitespace()).collect();
        if place.rotation == 0 {
            assert_eq!(
                page.text().split_whitespace().collect::<Vec<_>>(),
                place.shown.split_whitespace().collect::<Vec<_>>()
            );
        }
        read.sort_unstable();
        shown.sort_unstable();
        assert_eq!(
            read, shown,
            "the characters a widget turned {} shows",
            place.rotation
        );
    }
    // Every rotation, a password, a comb, a fallback font, and nothing.
    let rotations: Vec<i32> = placed.iter().map(|p| p.rotation).collect();
    assert_eq!(rotations, [0, 90, 180, 270, 0, 0]);
    assert_eq!(placed[3].shown, "*******");
    assert!(placed[5].shown.is_empty());
}

#[test]
#[ignore = "needs qpdf: `just test-qpdf`"]
fn qpdf_checks_made_appearances_clean() {
    let (bytes, _) = document();
    let folder = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(folder).unwrap();
    let path = folder.join("forms-qpdf-made.pdf");
    std::fs::write(&path, &bytes).unwrap();
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
    assert_eq!(ran.code, Some(0), "{said}");
    assert!(!said.contains("WARNING"), "{said}");
}
