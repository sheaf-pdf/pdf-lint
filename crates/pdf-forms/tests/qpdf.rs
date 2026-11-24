//! Documents of the appearances made for buttons' and choice fields'
//! widgets, as for text fields' below, bound into them with `pdf-structure`
//! and written: read back, a check box's and a radio button's states and a
//! push button's appearances, bound by `pdf_annots::AnnotMut`, are where they
//! were bound, and each combo or list box's text is where its appearance puts
//! it; and `qpdf --check` finds each clean when run via `just test-qpdf`.
//!
//! A document of appearances made for its text fields' widgets, each turned
//! by its rotation and one in a font its default resources lack, bound into
//! it with `pdf-structure` and written: read back, each widget's text is
//! where its appearance puts it (`pdf_gfx::TextPage`); and `qpdf --check`
//! finds it clean when run via `just test-qpdf`. With `embed`, the same of a
//! document whose values are drawn in Helvetica and a fallback program, the
//! one font written of every appearance's glyphs.

#![cfg(feature = "text-layout")]

use std::collections::HashMap;
use std::path::Path;

use pdf_fixtures::qpdf::Qpdf;
use pdf_forms::appearance::{
    Appearance, choice_for_widget, push_for_widget, text_for_widget, toggle_for_widget,
    widget_choice, widget_text,
};
use pdf_forms::pdf_annots::{
    AnnotColor, AnnotMut, AppearanceChars, AppearanceEntry, AppearanceMode, Appearances,
};
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

/// A made appearance written as a form XObject of its own: its box, its
/// matrix, and each font it needs, an object of the file by reference or,
/// for a font written by name, one added once (`written`).
fn form_xobject(
    doc: &mut Document,
    appearance: &Appearance,
    written: &mut HashMap<ResId, ObjectId>,
) -> ObjectId {
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
    }
    let [a, b, c, d, e, f] = appearance.matrix.to_array();
    let bbox = appearance.bbox;
    doc.add(Stream::new(
        dict! {
            "Type" => Object::name("XObject"),
            "Subtype" => Object::name("Form"),
            "BBox" => numbers(&[bbox.x0, bbox.y0, bbox.x1, bbox.y1]),
            "Matrix" => numbers(&[a, b, c, d, e, f]),
            "Resources" => dict! { "Font" => fonts },
        },
        appearance.content.clone(),
    ))
}

/// A button widget made in the document: its object, the name of its on
/// state (none for a push button), and whether its normal appearance has
/// an `Off`.
struct Bound {
    id: ObjectId,
    on_state: Option<Vec<u8>>,
    has_off: bool,
}

/// A document of buttons whose appearances are made and bound with
/// `AnnotMut`'s setters, as a caller saving them binds them: a check box
/// with a border and one with nothing drawn when it's off, so its normal
/// appearance has no `Off`; a radio button group of two, one on, its
/// widgets with on states of their own (`/1`) and by default (`Yes`); and a
/// push button whose `/MK` is set before it's made, with its rollover and
/// down appearances. Written, and each widget made in it.
fn button_document() -> (Vec<u8>, Vec<Bound>) {
    let mut doc = Document::new(Version::new(1, 7), dict! {});
    let (page, pages) = (doc.new_id(), doc.new_id());
    let zapf = doc.add(Std14::ZapfDingbats.dict());
    let helvetica = doc.add(Std14::Helvetica.dict());
    let on = doc.add(Stream::new(
        dict! { "Type" => Object::name("XObject"), "Subtype" => Object::name("Form"),
        "BBox" => numbers(&[0.0, 0.0, 14.0, 14.0]) },
        b"0 g 3 3 8 8 re f".to_vec(),
    ));
    let widget = |rect: [f32; 4], more: Dict| {
        let mut entries = dict! {
            "Type" => Object::name("Annot"),
            "Subtype" => Object::name("Widget"),
            "Rect" => numbers(&rect),
            "F" => 4,
            "P" => page,
        };
        for (key, value) in more.iter() {
            entries.set(key.to_vec(), value.clone());
        }
        entries
    };
    let check = doc.add(widget(
        [72.0, 700.0, 86.0, 714.0],
        dict! {
            "FT" => Object::name("Btn"),
            "T" => Object::text("agree"),
            "DA" => Object::text("/ZaDb 0 Tf 0 g"),
            "MK" => dict! { "CA" => Object::text("4"), "BC" => numbers(&[0.0]), "BG" => numbers(&[1.0]) },
        },
    ));
    let bare = doc.add(widget(
        [100.0, 700.0, 114.0, 714.0],
        dict! {
            "FT" => Object::name("Btn"),
            "T" => Object::text("bare"),
            "DA" => Object::text("/ZaDb 0 Tf 0 0 1 rg"),
            "MK" => dict! { "CA" => Object::text("8") },
            "BS" => dict! { "W" => 0 },
        },
    ));
    let group = doc.new_id();
    let radios = [
        doc.add(widget(
            [72.0, 660.0, 88.0, 676.0],
            dict! { "Parent" => group, "AS" => Object::name("1"),
            "AP" => dict! { "N" => dict! { "1" => on } },
            "MK" => dict! { "BC" => numbers(&[0.0]), "BG" => numbers(&[0.9]) },
            "BS" => dict! { "W" => 1, "S" => Object::name("B") } },
        )),
        doc.add(widget(
            [100.0, 660.0, 116.0, 676.0],
            dict! { "Parent" => group,
            "MK" => dict! { "BC" => numbers(&[0.0]) },
            "BS" => dict! { "W" => 1, "S" => Object::name("I") } },
        )),
    ];
    doc.set(
        group,
        dict! {
            "FT" => Object::name("Btn"),
            "Ff" => (1 << 15) | (1 << 14),
            "T" => Object::text("choice"),
            "DA" => Object::text("/ZaDb 0 Tf 0 g"),
            "V" => Object::name("1"),
            "Kids" => vec![Object::from(radios[0]), Object::from(radios[1])],
        },
    );
    let push = doc.add(widget(
        [72.0, 600.0, 172.0, 624.0],
        dict! {
            "FT" => Object::name("Btn"),
            "Ff" => 1 << 16,
            "T" => Object::text("send"),
            "DA" => Object::text("/Helv 0 Tf 0 g"),
            "H" => Object::name("P"),
            "BS" => dict! { "W" => 1, "S" => Object::name("B") },
        },
    ));
    let mut chars = AppearanceChars::default();
    chars.background_color = Some(AnnotColor::Gray(0.75));
    chars.border_color = Some(AnnotColor::Gray(0.0));
    chars.normal_caption = Some("Send".into());
    chars.down_caption = Some("Sending".into());
    AnnotMut::new(doc.get_dict_mut(push).unwrap()).set_appearance_chars(&chars);

    let fields = vec![
        Object::from(check),
        Object::from(bare),
        Object::from(group),
        Object::from(push),
    ];
    let annots = vec![
        Object::from(check),
        Object::from(bare),
        Object::from(radios[0]),
        Object::from(radios[1]),
        Object::from(push),
    ];
    let form = doc.add(dict! {
        "Fields" => fields,
        "DR" => dict! { "Font" => dict! { "ZaDb" => zapf, "Helv" => helvetica } },
    });
    doc.set(
        page,
        dict! {
            "Type" => Object::name("Page"),
            "Parent" => pages,
            "MediaBox" => numbers(&[0.0, 0.0, 612.0, 792.0]),
            "Resources" => dict! {},
            "Annots" => annots,
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

    // Each widget made; the toggles' states and the field's value as a
    // caller saving them sets them, from the group's rules.
    let limits = Limits::DEFAULT;
    let mut toggles = Vec::new();
    let mut pushes = Vec::new();
    {
        let catalog = doc.get_dict(catalog).unwrap();
        let form = Form::read(&doc, catalog, Rules::Iso, &limits)
            .unwrap()
            .unwrap();
        for field in form.fields() {
            let group = field.group(&doc, Rules::Iso, &limits).unwrap();
            for (index, widget) in field.widgets().iter().enumerate() {
                let id = widget.id().unwrap();
                match field.kind() {
                    Some(FieldKind::PushButton) => {
                        let made = push_for_widget(
                            &form,
                            field,
                            widget,
                            &doc,
                            Rules::Iso,
                            &limits,
                            &mut (),
                        )
                        .unwrap();
                        assert!(!made.font.is_fallback());
                        pushes.push((id, made.appearances));
                    }
                    Some(FieldKind::CheckBox | FieldKind::Radio) => {
                        let made = toggle_for_widget(
                            &form,
                            field,
                            widget,
                            &doc,
                            Rules::Iso,
                            &limits,
                            &mut (),
                        )
                        .unwrap();
                        assert!(!made.font.is_fallback());
                        let on = group.states()[index];
                        let state = group.group().state_name(index, on).into_owned();
                        assert_eq!(
                            made.on_state,
                            group.group().on_state(index).unwrap().as_ref()
                        );
                        toggles.push((id, made, state));
                    }
                    _ => {}
                }
            }
        }
    }

    let mut written: HashMap<ResId, ObjectId> = HashMap::new();
    let mut bound = Vec::new();
    for (id, made, state) in toggles {
        let ap = &made.appearances;
        let on = form_xobject(&mut doc, &ap.on, &mut written);
        let down_on = form_xobject(&mut doc, &ap.down_on, &mut written);
        let off = ap
            .off
            .as_ref()
            .map(|off| form_xobject(&mut doc, off, &mut written));
        let down_off = ap
            .down_off
            .as_ref()
            .map(|off| form_xobject(&mut doc, off, &mut written));
        let name: &[u8] = &made.on_state;
        let normal: Vec<(&[u8], ObjectId)> = [Some((name, on)), off.map(|off| (&b"Off"[..], off))]
            .into_iter()
            .flatten()
            .collect();
        let down: Vec<(&[u8], ObjectId)> = [
            Some((name, down_on)),
            down_off.map(|off| (&b"Off"[..], off)),
        ]
        .into_iter()
        .flatten()
        .collect();
        AnnotMut::new(doc.get_dict_mut(id).unwrap())
            .set_appearance_states(AppearanceMode::Normal, &normal)
            .set_appearance_states(AppearanceMode::Down, &down)
            .set_appearance_state(&state);
        bound.push(Bound {
            id,
            on_state: Some(made.on_state.clone()),
            has_off: off.is_some(),
        });
    }
    for (id, made) in pushes {
        let normal = form_xobject(&mut doc, &made.normal, &mut written);
        let rollover = form_xobject(&mut doc, made.rollover.as_ref().unwrap(), &mut written);
        let down = form_xobject(&mut doc, made.down.as_ref().unwrap(), &mut written);
        AnnotMut::new(doc.get_dict_mut(id).unwrap())
            .set_normal_appearance(normal)
            .set_rollover_appearance(rollover)
            .set_down_appearance(down);
        bound.push(Bound {
            id,
            on_state: None,
            has_off: false,
        });
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
    (bytes, bound)
}

/// Each button's appearances, read back from the file written, are bound
/// where the caller bound them: a toggle's states under its on state's name
/// and `Off` where one was made, in its normal and down appearances, its
/// `/AS` the group's name for its state; the check box with nothing drawn
/// when off has no `Off`; the push button's normal, rollover, and down
/// appearances single streams. Read again, each widget's on state is the
/// one made.
#[test]
fn made_buttons_read_back_bound_in_their_states() {
    let (bytes, bound) = button_document();
    let reading = Reading::open(&bytes[..], &read::Options::DEFAULT).unwrap();
    let limits = Limits::DEFAULT;
    let catalog = pdf_forms::pdf_semantics::catalog(&reading, reading.trailer().dict())
        .unwrap()
        .unwrap();
    let form = Form::read(&reading, catalog, Rules::Iso, &limits)
        .unwrap()
        .unwrap();
    let mut seen = Vec::new();
    for place in &bound {
        let dict = (&reading)
            .resolve(place.id)
            .and_then(Object::as_dict)
            .unwrap();
        let ap = Appearances::read(&reading, dict, Some(place.id), Rules::Iso, &limits.annots)
            .unwrap()
            .expect("each widget has its appearances");
        let names = |entry: &Option<AppearanceEntry>| match entry {
            Some(AppearanceEntry::States(states)) => {
                states.iter().map(|(name, _)| name.to_vec()).collect()
            }
            Some(AppearanceEntry::Single(_)) => vec![b"stream".to_vec()],
            None => Vec::new(),
        };
        match &place.on_state {
            Some(on) => {
                let mut normal = vec![on.clone()];
                if place.has_off {
                    normal.push(b"Off".to_vec());
                }
                assert_eq!(names(&ap.normal), normal);
                assert_eq!(names(&ap.down), [on.clone(), b"Off".to_vec()]);
                assert!(ap.rollover.is_none());
                seen.push((
                    String::from_utf8(on.clone()).unwrap(),
                    place.has_off,
                    ap.state.clone(),
                ));
            }
            None => {
                for entry in [&ap.normal, &ap.rollover, &ap.down] {
                    assert_eq!(names(entry), [b"stream".to_vec()]);
                }
            }
        }
    }
    let state = |s: &[u8]| Some(s.to_vec().into_boxed_slice());
    assert_eq!(
        seen,
        [
            ("Yes".to_string(), true, state(b"Off")),
            ("Yes".to_string(), false, state(b"Off")),
            ("1".to_string(), true, state(b"1")),
            ("Yes".to_string(), true, state(b"Off")),
        ]
    );
    // Read again, the on states are the ones the appearances were made
    // under.
    for field in form.fields() {
        for widget in field.widgets() {
            let Some(place) = bound.iter().find(|b| Some(b.id) == widget.id()) else {
                continue;
            };
            let states = widget.states(&reading, Rules::Iso, &limits).unwrap();
            assert_eq!(states.on_state.as_deref(), place.on_state.as_deref());
            assert_eq!(states.has_off, place.has_off);
        }
    }
}

#[test]
#[ignore = "needs qpdf: `just test-qpdf`"]
fn qpdf_checks_made_buttons_clean() {
    let (bytes, _) = button_document();
    let folder = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(folder).unwrap();
    let path = folder.join("forms-qpdf-buttons.pdf");
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

/// A document of choice fields whose appearances are made by
/// `choice_for_widget` and bound as their normal appearances, as a caller
/// saving them binds them: a combo box showing its selected option's label;
/// an editable one turned a quarter, showing a value outside its options;
/// and a list box of six options from its second, two selected, the last
/// below its box and so not written. Written, and each widget made in it
/// with the text it shows.
fn choice_document() -> (Vec<u8>, Vec<Placed>) {
    let mut doc = Document::new(Version::new(1, 7), dict! {});
    let (page, pages) = (doc.new_id(), doc.new_id());
    let helvetica = doc.add(Std14::Helvetica.dict());
    let fields = [
        (
            "size",
            [72.0, 700.0, 222.0, 720.0],
            0,
            dict! {
                "Ff" => 1 << 17,
                "Opt" => vec![
                    Object::from(vec![Object::text("s"), Object::text("Small")]),
                    Object::from(vec![Object::text("m"), Object::text("Medium")]),
                ],
                "V" => Object::text("m"),
                "BS" => dict! { "W" => 1, "S" => Object::name("B") },
                "MK" => dict! { "BC" => numbers(&[0.0]), "BG" => numbers(&[0.9]) },
            },
        ),
        (
            "colour",
            [72.0, 560.0, 92.0, 680.0],
            90,
            dict! {
                "Ff" => (1 << 17) | (1 << 18),
                "Opt" => vec![Object::text("red"), Object::text("green")],
                "V" => Object::text("teal"),
                "MK" => dict! { "BC" => numbers(&[0.0, 0.0, 1.0]) },
            },
        ),
        (
            "days",
            [300.0, 650.0, 380.0, 698.0],
            0,
            dict! {
                "Ff" => 1 << 21,
                "DA" => Object::text("/Helv 10 Tf 0 g"),
                "Opt" => ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
                    .map(Object::text)
                    .to_vec(),
                "V" => vec![Object::text("Tue"), Object::text("Thu")],
                "TI" => 1,
                "MK" => dict! { "BC" => numbers(&[0.0]) },
            },
        ),
    ];
    let mut widgets = Vec::new();
    for (name, rect, rotation, more) in fields {
        let mut entries = dict! {
            "Type" => Object::name("Annot"),
            "Subtype" => Object::name("Widget"),
            "FT" => Object::name("Ch"),
            "T" => Object::text(name),
            "Rect" => numbers(&rect),
            "F" => 4,
            "P" => page,
        };
        for (key, value) in more.iter() {
            entries.set(key.to_vec(), value.clone());
        }
        if let Some(mk) = entries.get_mut(b"MK").and_then(Object::as_dict_mut) {
            mk.set("R", rotation);
        }
        widgets.push(doc.add(entries));
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

    // Each widget made, with the text it shows: a combo box's, or the
    // labels of a list box's lines written.
    let limits = Limits::DEFAULT;
    let mut made = Vec::new();
    {
        let catalog = doc.get_dict(catalog).unwrap();
        let form = Form::read(&doc, catalog, Rules::Iso, &limits)
            .unwrap()
            .unwrap();
        for field in form.fields() {
            for widget in field.widgets() {
                let read =
                    widget_choice(field, widget, &doc, Rules::Iso, &limits, &mut ()).unwrap();
                let shown = match read.combo() {
                    Some(combo) => combo.text.to_string(),
                    None => read.labels()[read.top()..read.top() + 4].join(" "),
                };
                let mut faults = Vec::new();
                let choice =
                    choice_for_widget(&form, field, widget, &doc, Rules::Iso, &limits, &mut faults)
                        .unwrap();
                assert!(faults.is_empty() && !choice.font.is_fallback());
                let rect = widget.annot(&doc, Rules::Iso, &limits).unwrap().rect();
                let place = Placed {
                    id: widget.id().unwrap(),
                    rect,
                    shown,
                    rotation: read.look().rotation,
                };
                made.push((choice.appearance, place));
            }
        }
    }

    let mut written: HashMap<ResId, ObjectId> = HashMap::new();
    let mut placed = Vec::new();
    for (appearance, place) in made {
        let normal = form_xobject(&mut doc, &appearance, &mut written);
        AnnotMut::new(doc.get_dict_mut(place.id).unwrap()).set_normal_appearance(normal);
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

/// Each choice widget's appearance, read back from the file written, drawn
/// on its rectangle as a viewer draws it: the text it shows is read there,
/// each character inside the rectangle, a list box's lines from its top
/// down and none below its box.
#[test]
fn made_choices_read_back_as_their_text_on_their_widgets() {
    let (bytes, placed) = choice_document();
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
        assert_eq!(read, shown, "the characters a choice shows");
    }
    let shown: Vec<&str> = placed.iter().map(|p| p.shown.as_str()).collect();
    assert_eq!(shown, ["Medium", "teal", "Tue Wed Thu Fri"]);
    assert_eq!(placed[1].rotation, 90);
}

#[test]
#[ignore = "needs qpdf: `just test-qpdf`"]
fn qpdf_checks_made_choices_clean() {
    let (bytes, _) = choice_document();
    let folder = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(folder).unwrap();
    let path = folder.join("forms-qpdf-choices.pdf");
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

/// The document of [`fallback_document`]'s fields, each value made with
/// `appearance::text_with_fallback` in Helvetica and a fallback TrueType
/// program, the program written once (`pdf_fonts::embed::write`) of every
/// appearance's glyphs joined, under the number its resource gives: the
/// file, and each widget with the value it shows, composed.
#[cfg(feature = "embed")]
fn fallback_document() -> (Vec<u8>, Vec<Placed>) {
    use pdf_forms::appearance::{Fallback, FieldFont, text_with_fallback};
    use pdf_forms::pdf_content_streams::Res;
    use pdf_forms::pdf_fonts::embed::{self, Glyphs, Program};

    let mut doc = Document::new(Version::new(1, 7), dict! {});
    let (page, pages, font) = (doc.new_id(), doc.new_id(), doc.new_id());
    let helvetica = doc.add(Std14::Helvetica.dict());
    let program = Program::open(pdf_fixtures::font::latin(1000).build(), 0).unwrap();
    let fallback = Fallback::new(Res::written(font), &program);
    let value = "\u{141}ukasz Nguy\u{1ec5}n \u{d6}l\u{e7}er";
    let decomposed = "\u{141}ukasz Nguye\u{302}\u{303}n O\u{308}lc\u{327}er";
    let fields = [
        ("single", value, [72.0, 700.0, 352.0, 720.0], dict! {}),
        (
            "decomposed",
            decomposed,
            [72.0, 670.0, 352.0, 690.0],
            dict! {},
        ),
        (
            "comb",
            value,
            [72.0, 640.0, 452.0, 660.0],
            dict! { "Ff" => 1 << 24, "MaxLen" => 19 },
        ),
        (
            "lines",
            value,
            [72.0, 560.0, 152.0, 630.0],
            dict! { "Ff" => 1 << 12 },
        ),
        (
            "latin",
            "Zo\u{eb} Caf\u{e9}",
            [72.0, 530.0, 352.0, 550.0],
            dict! {},
        ),
    ];
    let mut widgets = Vec::new();
    for (name, value, rect, more) in &fields {
        let mut field = text_field(page, name, value, *rect, 0, more.clone());
        field.set("DA", Object::text("/Helv 12 Tf 0 g"));
        widgets.push(doc.add(field));
    }
    let refs = Object::from(
        widgets
            .iter()
            .map(|&id| Object::from(id))
            .collect::<Vec<_>>(),
    );
    let form = doc.add(dict! {
        "Fields" => refs.clone(),
        "DR" => dict! { "Font" => dict! { "Helv" => helvetica } },
    });
    doc.set(
        page,
        dict! {
            "Type" => Object::name("Page"),
            "Parent" => pages,
            "MediaBox" => numbers(&[0.0, 0.0, 612.0, 792.0]),
            "Resources" => dict! {},
            "Annots" => refs,
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

    let limits = Limits::DEFAULT;
    let helv = FieldFont::standard(Std14::Helvetica);
    let mut glyphs = Glyphs::new();
    let mut placed = Vec::new();
    let mut made_all = Vec::new();
    {
        let catalog = doc.get_dict(catalog).unwrap();
        let form = Form::read(&doc, catalog, Rules::Iso, &limits)
            .unwrap()
            .unwrap();
        for (field, (_, value, ..)) in form.fields().iter().zip(&fields) {
            let widget = &field.widgets()[0];
            let read =
                widget_text(field, widget, &doc, Rules::Iso, None, &limits, &mut ()).unwrap();
            let made = text_with_fallback(
                &read.field(),
                &read.look(),
                read.default_appearance(),
                helv.res().unwrap(),
                &helv.layout(),
                &fallback,
                &limits,
            )
            .unwrap();
            glyphs.merge(&made.glyphs);
            let rect = widget.annot(&doc, Rules::Iso, &limits).unwrap().rect();
            let id = widget.id().unwrap();
            let shown = unicode_normalization::UnicodeNormalization::nfc(*value).collect();
            placed.push(Placed {
                id,
                rect,
                shown,
                rotation: 0,
            });
            made_all.push((id, made.appearance));
        }
    }
    // The program, written once of every appearance's glyphs, at the number
    // the appearances name it by, its other objects after it.
    let objects = embed::write(&program, &glyphs, &embed::Options::default()).unwrap();
    let mut ids = vec![font];
    ids.extend((1..objects.len()).map(|_| doc.new_id()));
    for (at, mut object) in objects.into_iter().enumerate() {
        object.renumber(|to| match to.generation {
            0 => ids.get(to.number as usize).copied(),
            _ => Some(to),
        });
        doc.set(ids[at], object);
    }
    for (id, appearance) in made_all {
        let mut fonts = Dict::new();
        for need in &appearance.needs.0 {
            let bound = match need.res.body() {
                ResBody::Written(id) => *id,
                ResBody::Objects(_) => helvetica,
                other => panic!("a resource the maker doesn't name: {other:?}"),
            };
            fonts.set(need.name.clone(), bound);
        }
        let bbox = appearance.bbox;
        let stream = Stream::new(
            dict! {
                "Type" => Object::name("XObject"),
                "Subtype" => Object::name("Form"),
                "BBox" => numbers(&[bbox.x0, bbox.y0, bbox.x1, bbox.y1]),
                "Resources" => dict! { "Font" => fonts },
            },
            appearance.content,
        );
        let ap = doc.add(stream);
        doc.get_dict_mut(id).unwrap().set("AP", dict! { "N" => ap });
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

/// Each widget of the fallback document, read back from the file written,
/// shows its value composed, in Helvetica and the one font written of the
/// glyphs its appearances drew, each character inside its rectangle.
#[cfg(feature = "embed")]
#[test]
fn fallback_appearances_read_back_as_their_values() {
    let (bytes, placed) = fallback_document();
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
            .unwrap();
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
        let read: String = drawn.iter().map(|c| c.ch).collect();
        assert_eq!(read, place.shown);
        for ch in &drawn {
            let at = ch.origin;
            let inside = place.rect.x0 - 0.01 <= at.x
                && at.x <= place.rect.x1 + 0.01
                && place.rect.y0 - 0.01 <= at.y
                && at.y <= place.rect.y1 + 0.01;
            assert!(inside, "{:?} at {at:?} outside {:?}", ch.ch, place.rect);
        }
    }
}

#[cfg(feature = "embed")]
#[test]
#[ignore = "needs qpdf: `just test-qpdf`"]
fn qpdf_checks_fallback_appearances_clean() {
    let (bytes, _) = fallback_document();
    let folder = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(folder).unwrap();
    let path = folder.join("forms-qpdf-fallback.pdf");
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
    println!("{said}");
    assert_eq!(ran.code, Some(0), "{said}");
    assert!(!said.contains("WARNING"), "{said}");
}
