//! Tests of `Border::read`: an annotation's border read alone, as
//! `Annot::border` reads it.

use std::collections::BTreeMap;

use pdf_objects::fault::Place;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, dict};

use crate::{Annot, Border, BorderStyle, Cap, Error, Fault, Limits};

type Objects = BTreeMap<ObjectId, Object>;
type Told = Vec<(Place, Fault)>;

/// The entries a border is read from.
const BORDER_KEYS: [&str; 3] = ["BS", "Border", "BE"];

fn numbers(values: &[f64]) -> Object {
    Object::from(values.iter().map(|&v| Object::from(v)).collect::<Vec<_>>())
}

fn rect() -> Object {
    numbers(&[0.0, 0.0, 40.0, 20.0])
}

fn border(width: f32, style: BorderStyle, dash: &[f32]) -> Border {
    Border {
        width,
        style,
        dash: dash.to_vec(),
        ..Border::default()
    }
}

/// The objects the dictionaries below refer to.
fn objects() -> Objects {
    let mut objects = Objects::new();
    let mut put = |n: u32, object: Object| objects.insert(ObjectId::new(n, 0), object);
    put(
        1,
        Object::from(dict! { "W" => 3, "S" => Object::name("I") }),
    );
    put(2, numbers(&[1.0, 1.0, 2.0]));
    put(3, numbers(&[5.0, 1.0]));
    // Two references to each other: a loop.
    put(4, Object::from(ObjectId::new(5, 0)));
    put(5, Object::from(ObjectId::new(4, 0)));
    put(6, Object::from(4.5));
    put(
        7,
        Object::from(dict! { "S" => Object::name("C"), "I" => 2 }),
    );
    objects
}

/// An annotation's dictionary, the border it has, and what reading the
/// border alone tells of it.
struct Case {
    what: &'static str,
    dict: Dict,
    border: Border,
    told: Vec<Fault>,
}

fn case(what: &'static str, dict: Dict, border: Border, told: &[Fault]) -> Case {
    Case {
        what,
        dict,
        border,
        told: told.to_vec(),
    }
}

/// A `/Square` with `entries` beside its `/Subtype` and `/Rect`.
fn square(entries: Dict) -> Dict {
    let mut dict = dict! {
        "Subtype" => Object::name("Square"),
        "Rect" => rect(),
    };
    for entry in entries.entries() {
        dict.set(entry.key(), entry.value().clone());
    }
    dict
}

/// The dictionaries the crate's border tests read, and their recorded
/// PDFium case's, beside every way of reading a border that tells a fault
/// or takes a default.
fn cases() -> Vec<Case> {
    let wrong = |key| Fault::WrongType { key };
    vec![
        case(
            "no border entries",
            square(Dict::new()),
            Border::default(),
            &[],
        ),
        case(
            "a FreeText's dashed /BS with a cloudy /BE",
            dict! {
                "Subtype" => Object::name("FreeText"),
                "Rect" => numbers(&[100.0, 200.0, 50.0, 120.0]),
                "DA" => Object::text("/Helv 14 Tf 0.25 0.5 0.75 rg"),
                "Q" => 2,
                "IT" => Object::name("FreeTextCallout"),
                "CL" => numbers(&[30.0, 110.0, 45.0, 140.0, 50.0, 140.0]),
                "LE" => Object::name("ClosedArrow"),
                "BS" => dict! {
                    "W" => 2.5,
                    "S" => Object::name("D"),
                    "D" => numbers(&[6.0, 3.0]),
                },
                "BE" => dict! { "S" => Object::name("C"), "I" => 1.5 },
            },
            Border {
                cloudy_intensity: Some(1.5),
                ..border(2.5, BorderStyle::Dashed, &[6.0, 3.0])
            },
            &[],
        ),
        case(
            "a Line's /Border of three",
            dict! {
                "Subtype" => Object::name("Line"),
                "Rect" => numbers(&[0.0, 0.0, 100.0, 100.0]),
                "L" => numbers(&[10.0, 20.0, 80.0, 90.0]),
                "LE" => vec![Object::name("Circle"), Object::name("OpenArrow")],
                "IC" => numbers(&[1.0, 0.0, 0.0]),
                "Cap" => true,
                "Border" => numbers(&[2.0, 4.0, 1.5]),
            },
            Border {
                horizontal_radius: 2.0,
                vertical_radius: 4.0,
                ..border(1.5, BorderStyle::Solid, &[])
            },
            &[],
        ),
        case(
            "a Square's cloudy /BE",
            dict! {
                "Type" => Object::name("Annot"),
                "Subtype" => Object::name("Square"),
                "Rect" => numbers(&[80.0, 80.0, 220.0, 220.0]),
                "RD" => numbers(&[10.0, 10.0, 10.0, 10.0]),
                "C" => numbers(&[1.0, 0.0, 0.0]),
                "BE" => dict! { "S" => Object::name("C"), "I" => 1 },
            },
            Border {
                cloudy_intensity: Some(1.0),
                ..Border::default()
            },
            &[],
        ),
        case(
            "a Link's /Border with a dash array",
            dict! {
                "Subtype" => Object::name("Link"),
                "Rect" => numbers(&[10.0, 50.0, 80.0, 70.0]),
                "Border" => vec![
                    Object::from(0),
                    Object::from(0),
                    Object::from(2),
                    numbers(&[4.0, 2.0]),
                ],
            },
            border(2.0, BorderStyle::Dashed, &[4.0, 2.0]),
            &[],
        ),
        case(
            "a Link's /Border of two",
            dict! {
                "Subtype" => Object::name("Link"),
                "Rect" => numbers(&[10.0, 80.0, 80.0, 100.0]),
                "Border" => numbers(&[1.0, 2.0]),
            },
            Border::default(),
            &[wrong("Border")],
        ),
        case(
            "/BS solid",
            square(dict! { "BS" => dict! { "W" => 2, "S" => Object::name("S") } }),
            border(2.0, BorderStyle::Solid, &[]),
            &[],
        ),
        case(
            "/BS beveled",
            square(dict! { "BS" => dict! { "W" => 2, "S" => Object::name("B") } }),
            border(2.0, BorderStyle::Beveled, &[]),
            &[],
        ),
        case(
            "/BS inset",
            square(dict! { "BS" => dict! { "S" => Object::name("I") } }),
            border(1.0, BorderStyle::Inset, &[]),
            &[],
        ),
        case(
            "/BS underline",
            square(dict! { "BS" => dict! { "W" => 0, "S" => Object::name("U") } }),
            border(0.0, BorderStyle::Underline, &[]),
            &[],
        ),
        case(
            "/BS dashed with no dash array",
            square(dict! { "BS" => dict! { "S" => Object::name("D") } }),
            border(1.0, BorderStyle::Dashed, &[3.0]),
            &[],
        ),
        case(
            "/BS dashed, a dash below 0 and one no number left out",
            square(dict! { "BS" => dict! {
                "S" => Object::name("D"),
                "D" => vec![
                    Object::from(4),
                    Object::from(-1),
                    Object::text("x"),
                    Object::from(2),
                ],
            } }),
            border(1.0, BorderStyle::Dashed, &[4.0, 2.0]),
            &[],
        ),
        case(
            "/BS of a style no table has",
            square(dict! { "BS" => dict! { "W" => 2, "S" => Object::name("Q") } }),
            border(2.0, BorderStyle::Solid, &[]),
            &[wrong("S")],
        ),
        case(
            "/BS of a width below 0",
            square(dict! { "BS" => dict! { "W" => -2 } }),
            border(0.0, BorderStyle::Solid, &[]),
            &[],
        ),
        case(
            "/BS of a width that is no number",
            square(dict! { "BS" => dict! { "W" => Object::text("2") } }),
            border(1.0, BorderStyle::Solid, &[]),
            &[wrong("W")],
        ),
        case(
            "/BS no dictionary, a /Border beside it",
            square(dict! { "BS" => 5, "Border" => numbers(&[0.0, 0.0, 3.0]) }),
            border(3.0, BorderStyle::Solid, &[]),
            &[wrong("BS")],
        ),
        case(
            "/BS beside a /Border of two, which is ignored",
            square(dict! {
                "BS" => dict! { "W" => 2 },
                "Border" => numbers(&[1.0, 2.0]),
            }),
            border(2.0, BorderStyle::Solid, &[]),
            &[],
        ),
        case(
            "/Border whose dash is no array",
            square(dict! { "Border" => numbers(&[0.0, 0.0, 1.0, 5.0]) }),
            border(1.0, BorderStyle::Solid, &[]),
            &[wrong("Border")],
        ),
        case(
            "/Border whose dash array is empty",
            square(dict! { "Border" => vec![
                Object::from(0),
                Object::from(0),
                Object::from(1),
                Object::from(Vec::<Object>::new()),
            ] }),
            border(1.0, BorderStyle::Dashed, &[3.0]),
            &[],
        ),
        case(
            "/Border with a width that is no number",
            square(dict! { "Border" => vec![
                Object::from(0),
                Object::from(0),
                Object::text("x"),
            ] }),
            border(0.0, BorderStyle::Solid, &[]),
            &[wrong("Border")],
        ),
        case(
            "/Border below 0",
            square(dict! { "Border" => numbers(&[-1.0, -2.0, -3.0]) }),
            border(0.0, BorderStyle::Solid, &[]),
            &[],
        ),
        case(
            "/Border no array",
            square(dict! { "Border" => 1 }),
            Border::default(),
            &[wrong("Border")],
        ),
        case(
            "/BE no dictionary",
            square(dict! { "BE" => 1 }),
            Border::default(),
            &[wrong("BE")],
        ),
        case(
            "/BE of no effect",
            square(dict! { "BE" => dict! { "S" => Object::name("S"), "I" => 2 } }),
            Border::default(),
            &[],
        ),
        case(
            "/BE cloudy with no intensity",
            square(dict! { "BE" => dict! { "S" => Object::name("C") } }),
            Border {
                cloudy_intensity: Some(0.0),
                ..Border::default()
            },
            &[],
        ),
        case(
            "/BS, /BE, and a width by reference",
            square(dict! {
                "BS" => ObjectId::new(1, 0),
                "BE" => ObjectId::new(7, 0),
            }),
            Border {
                cloudy_intensity: Some(2.0),
                ..border(3.0, BorderStyle::Inset, &[])
            },
            &[],
        ),
        case(
            "/BS's width by reference",
            square(dict! { "BS" => dict! { "W" => ObjectId::new(6, 0) } }),
            border(4.5, BorderStyle::Solid, &[]),
            &[],
        ),
        case(
            "/Border by reference",
            square(dict! { "Border" => ObjectId::new(2, 0) }),
            Border {
                horizontal_radius: 1.0,
                vertical_radius: 1.0,
                ..border(2.0, BorderStyle::Solid, &[])
            },
            &[],
        ),
        case(
            "/Border's dash array by reference",
            square(dict! { "Border" => vec![
                Object::from(0),
                Object::from(0),
                Object::from(1),
                Object::from(ObjectId::new(3, 0)),
            ] }),
            border(1.0, BorderStyle::Dashed, &[5.0, 1.0]),
            &[],
        ),
        case(
            "/BS a reference loop",
            square(dict! { "BS" => ObjectId::new(4, 0) }),
            Border::default(),
            &[Fault::ReferenceLoop],
        ),
        case(
            "/BS a reference to nothing",
            square(dict! { "BS" => ObjectId::new(50, 0) }),
            Border::default(),
            &[],
        ),
    ]
}

/// What `Annot::from_dict_with` tells of `dict`.
fn told_by_annot(objects: &Objects, dict: &Dict, id: Option<ObjectId>, rules: Rules) -> Told {
    let mut told = Vec::new();
    Annot::from_dict_with(objects, dict, id, rules, &Limits::DEFAULT, &mut told)
        .expect("the annotation is read");
    told
}

/// `Border::read` gives `Annot::border`'s answer, with a sink and without,
/// and tells what `Annot` tells of the border: `Annot` tells, in its order
/// of reading, what's told of the dictionary's other entries, then of
/// `/Border` alone (for `Annot::legacy_border`, whatever `/BS` says), then
/// what `Border::read` tells.
#[test]
fn a_border_read_alone_is_the_annotations() {
    let objects = objects();
    let limits = Limits::DEFAULT;
    for Case {
        what,
        dict,
        border,
        told: expected,
    } in cases()
    {
        for rules in [Rules::Iso, Rules::Pdfium] {
            for id in [None, Some(ObjectId::new(99, 0))] {
                let annot = Annot::from_dict(&objects, &dict, id, rules, &limits).unwrap();
                let alone = Border::read(&objects, &dict, id, rules, &limits).unwrap();
                assert_eq!(&alone, annot.border(), "{what}");
                assert_eq!(alone, border, "{what}");

                let mut told = Vec::new();
                let with_sink =
                    Border::read_with(&objects, &dict, id, rules, &limits, &mut told).unwrap();
                assert_eq!(with_sink, alone, "{what}");
                let faults: Vec<_> = told.iter().map(|(_, fault)| fault.clone()).collect();
                assert_eq!(faults, expected, "{what}");

                let mut rest = dict.clone();
                for key in BORDER_KEYS {
                    rest.remove(key);
                }
                let rest = told_by_annot(&objects, &rest, id, rules);
                let mut of_border = match dict.get(b"Border") {
                    Some(legacy) => {
                        let legacy = dict! { "Border" => legacy.clone() };
                        let mut told = Vec::new();
                        Border::read_with(&objects, &legacy, id, rules, &limits, &mut told)
                            .unwrap();
                        told
                    }
                    None => Vec::new(),
                };
                of_border.extend(told);
                let whole = told_by_annot(&objects, &dict, id, rules);
                assert!(
                    (0..=rest.len())
                        .any(|at| whole == [&rest[..at], &of_border[..], &rest[at..]].concat()),
                    "{what}: {whole:?}, against {rest:?} and {of_border:?}"
                );
            }
        }
    }
}

/// The border of an annotation that `Annot::from_dict` refuses under a cap
/// on the rest of it (here a chain of actions one past
/// `Limits::semantics.action_chain`) is read all the same.
#[test]
fn a_border_is_read_past_a_cap_on_the_rest() {
    let limits = Limits::DEFAULT;
    let mut objects = Objects::new();
    let mut next = None;
    for n in 0..=limits.semantics.action_chain {
        let mut action = dict! { "S" => Object::name("Named"), "N" => Object::name("NextPage") };
        if let Some(next) = next {
            action.set("Next", next);
        }
        let id = ObjectId::new(u32::try_from(n).unwrap() + 1, 0);
        objects.insert(id, Object::from(action));
        next = Some(id);
    }
    let mut widget = dict! {
        "Subtype" => Object::name("Widget"),
        "Rect" => rect(),
        "BS" => dict! { "W" => 2, "S" => Object::name("B") },
        "BE" => dict! { "S" => Object::name("C"), "I" => 1 },
        "A" => next.unwrap(),
    };
    let err = Annot::from_dict(&objects, &widget, None, Rules::Pdfium, &limits).unwrap_err();
    assert_eq!(err, Error::Limit(Cap::ActionChain));
    let alone = Border::read(&objects, &widget, None, Rules::Pdfium, &limits).unwrap();
    assert_eq!(
        alone,
        Border {
            cloudy_intensity: Some(1.0),
            ..border(2.0, BorderStyle::Beveled, &[])
        }
    );

    // Without the chain, the annotation's own.
    widget.remove(b"A");
    let annot = Annot::from_dict(&objects, &widget, None, Rules::Pdfium, &limits).unwrap();
    assert_eq!(annot.border(), &alone);
}
