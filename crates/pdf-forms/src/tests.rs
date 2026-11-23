//! Unit, invariant, resource-cap, and hygiene tests for `pdf-forms`.

use std::collections::BTreeMap;

use pdf_objects::fault::Place;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Str, Stream, Value, Why, dict};

use crate::{
    Cap, Error, Fault, Field, FieldFlags, FieldKind, FieldType, FieldValue, Form, Limits, Quadding,
    Widget, fuzz,
};

#[derive(Default)]
struct Store {
    objects: BTreeMap<ObjectId, Object>,
    next: u32,
}

impl Store {
    fn put(&mut self, obj: impl Into<Object>) -> ObjectId {
        let id = self.reserve();
        self.objects.insert(id, obj.into());
        id
    }

    fn reserve(&mut self) -> ObjectId {
        self.next += 1;
        ObjectId::new(self.next, 0)
    }

    fn set(&mut self, id: ObjectId, obj: impl Into<Object>) {
        self.objects.insert(id, obj.into());
    }
}

fn refs(ids: &[ObjectId]) -> Vec<Object> {
    ids.iter().map(|&id| Object::from(id)).collect()
}

fn rect() -> Vec<Object> {
    [10, 20, 110, 40].into_iter().map(Object::from).collect()
}

fn catalog_of(fields: Vec<Object>) -> Dict {
    dict! { "AcroForm" => dict! { "Fields" => fields } }
}

fn read<'o>(store: &'o Store, catalog: &'o Dict) -> Form<'o> {
    Form::read(&store.objects, catalog, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap()
        .unwrap()
}

fn read_told<'o>(store: &'o Store, catalog: &'o Dict, limits: &Limits) -> (Form<'o>, Vec<Fault>) {
    let mut sink: Vec<(Place, Fault)> = Vec::new();
    let form = Form::read_with(&store.objects, catalog, Rules::Pdfium, limits, &mut sink)
        .unwrap()
        .unwrap();
    (form, sink.into_iter().map(|(_, fault)| fault).collect())
}

#[test]
fn public_types_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Limits>();
    check::<Cap>();
    check::<Error>();
    check::<Fault>();
    check::<FieldType>();
    check::<FieldKind>();
    check::<FieldFlags>();
    check::<FieldValue<'static>>();
    check::<Field<'static>>();
    check::<Widget<'static>>();
    check::<Form<'static>>();
}

#[test]
fn readme_first_example_and_links_match_invariants() {
    let readme = include_str!("../README.md");
    let lib = include_str!("lib.rs");

    assert!(
        readme.contains("as of chromium/8152"),
        "README.md must state the pinned PDFium build"
    );
    assert!(
        readme.contains("https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE"),
        "README.md must link LICENSE-APACHE"
    );
    assert!(
        readme.contains("https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT"),
        "README.md must link LICENSE-MIT"
    );
    assert!(
        !readme.contains("](../"),
        "README.md must not contain relative parent links"
    );

    let mut rest = readme;
    while let Some(pos) = rest.find("](") {
        let after = &rest[pos + 2..];
        let end = after.find(')').expect("unclosed markdown link");
        let url = &after[..end];
        assert!(
            url.starts_with("https://"),
            "every README.md link must use https://, found {url:?}"
        );
        rest = &after[end + 1..];
    }

    let readme_block = readme
        .split_once("```rust\n")
        .and_then(|(_, tail)| tail.split_once("\n```"))
        .map(|(code, _)| code)
        .expect("README.md must have a ```rust block");
    let doc_lines: String = lib
        .lines()
        .take_while(|l| l.starts_with("//!") || l.is_empty())
        .filter_map(|l| l.strip_prefix("//! ").or_else(|| l.strip_prefix("//!")))
        .collect::<Vec<_>>()
        .join("\n");
    let lib_block = doc_lines
        .split_once("```\n")
        .and_then(|(_, tail)| tail.split_once("\n```"))
        .map(|(code, _)| code)
        .expect("lib.rs must have a ``` block");
    assert_eq!(readme_block, lib_block);
}

#[test]
fn a_catalog_without_a_form_reads_as_none() {
    let store = Store::default();
    let none = dict! { "Type" => Object::name("Catalog") };
    assert!(
        Form::read(&store.objects, &none, Rules::Iso, &Limits::DEFAULT)
            .unwrap()
            .is_none()
    );
    // A form that isn't a dictionary is none too, with a fault.
    let wrong = dict! { "AcroForm" => 7 };
    let mut sink: Vec<(Place, Fault)> = Vec::new();
    let form = Form::read_with(
        &store.objects,
        &wrong,
        Rules::Iso,
        &Limits::DEFAULT,
        &mut sink,
    );
    assert!(form.unwrap().is_none());
    assert_eq!(sink[0].1, Fault::WrongType { key: "AcroForm" });
}

#[test]
fn a_field_inherits_what_it_doesnt_hold() {
    let mut store = Store::default();
    let (top, mid, leaf, own) = (
        store.reserve(),
        store.reserve(),
        store.reserve(),
        store.reserve(),
    );
    store.set(
        top,
        dict! {
            "T" => Object::text("top"),
            "FT" => Object::name("Tx"),
            "Ff" => i64::from((FieldFlags::MULTILINE | FieldFlags::REQUIRED).bits()),
            "V" => Object::text("from top"),
            "DV" => Object::text("default"),
            "DA" => Object::text("/Helv 9 Tf"),
            "Q" => 1,
            "MaxLen" => 12,
            "Kids" => refs(&[mid]),
        },
    );
    store.set(
        mid,
        dict! { "T" => Object::text("mid"), "Parent" => top, "Kids" => refs(&[leaf, own]) },
    );
    store.set(
        leaf,
        dict! { "T" => Object::text("leaf"), "Parent" => mid, "Rect" => rect() },
    );
    store.set(
        own,
        dict! {
            "T" => Object::text("own"),
            "Parent" => mid,
            "FT" => Object::name("Ch"),
            "Ff" => i64::from(FieldFlags::COMBO.bits()),
            "V" => Object::text("mine"),
            "DA" => Object::text("/Cour 7 Tf"),
            "Q" => 2,
            "MaxLen" => 3,
            "Rect" => rect(),
        },
    );
    let catalog = catalog_of(refs(&[top]));
    let (form, faults) = read_told(&store, &catalog, &Limits::DEFAULT);
    assert_eq!(faults, []);
    assert_eq!(form.roots(), [0]);

    let leaf = &form.fields()[form.find("top.mid.leaf").unwrap()];
    assert_eq!(leaf.field_type(), Some(FieldType::Text));
    assert_eq!(leaf.kind(), Some(FieldKind::Text));
    assert!(leaf.flags().contains(FieldFlags::MULTILINE));
    assert_eq!(leaf.value(), Some(&FieldValue::Text("from top".into())));
    assert_eq!(
        leaf.default_value(),
        Some(&FieldValue::Text("default".into()))
    );
    assert_eq!(leaf.default_appearance(), Some(&b"/Helv 9 Tf"[..]));
    assert_eq!(leaf.quadding(), Quadding::Center);
    assert_eq!(leaf.max_len(), Some(12));
    assert_eq!(leaf.parent(), Some(1));
    assert!(leaf.is_terminal());

    let own = &form.fields()[form.find("top.mid.own").unwrap()];
    assert_eq!(own.kind(), Some(FieldKind::Combo));
    assert!(!own.flags().contains(FieldFlags::MULTILINE));
    assert_eq!(own.value(), Some(&FieldValue::Text("mine".into())));
    assert_eq!(own.default_appearance(), Some(&b"/Cour 7 Tf"[..]));
    assert_eq!(own.quadding(), Quadding::Right);
    assert_eq!(own.max_len(), Some(3));

    let mid = &form.fields()[1];
    assert!(!mid.is_terminal());
    assert_eq!(mid.kids(), [2, 3]);
    assert_eq!(form.terminals().map(|(i, _)| i).collect::<Vec<_>>(), [2, 3]);
}

/// A `/V` or `/DV` that reaches nothing (null, a reference to no object or
/// to null) is an absent one, and the parent's is inherited, as every other
/// inherited entry is read; a value of the wrong type is the field's own,
/// and no value.
#[test]
fn a_value_that_reaches_nothing_is_inherited() {
    let mut store = Store::default();
    let top = store.reserve();
    let null = store.put(Object::from(Value::Null));
    let dangling = ObjectId::new(9_999, 0);
    let mut kids = Vec::new();
    for (name, value) in [
        ("dangling", Object::from(dangling)),
        ("null", Object::from(Value::Null)),
        ("by-reference", Object::from(null)),
        ("wrong", Object::from(4)),
        ("own", Object::text("mine")),
    ] {
        kids.push(store.put(dict! {
            "T" => Object::text(name),
            "Parent" => top,
            "V" => value.clone(),
            "DV" => value,
            "Rect" => rect(),
        }));
    }
    store.set(
        top,
        dict! {
            "T" => Object::text("top"),
            "FT" => Object::name("Tx"),
            "V" => Object::text("from top"),
            "DV" => Object::text("default"),
            "Kids" => refs(&kids),
        },
    );
    let catalog = catalog_of(refs(&[top]));
    for rules in [Rules::Iso, Rules::Pdfium] {
        let form = Form::read(&store.objects, &catalog, rules, &Limits::DEFAULT)
            .unwrap()
            .unwrap();
        let values = |name: &str| {
            let field = &form.fields()[form.find(name).unwrap()];
            (field.value().cloned(), field.default_value().cloned())
        };
        let text = |s: &str| Some(FieldValue::Text(s.into()));
        for inherits in ["top.dangling", "top.null", "top.by-reference"] {
            assert_eq!(
                values(inherits),
                (text("from top"), text("default")),
                "{inherits}"
            );
        }
        assert_eq!(values("top.wrong"), (None, None));
        assert_eq!(values("top.own"), (text("mine"), text("mine")));
    }
}

/// `find` gives the first field whose full name is the one asked for,
/// through names that hold periods, are empty, or are missing.
#[test]
fn find_is_the_first_field_of_that_full_name() {
    let mut store = Store::default();
    let top = store.reserve();
    let mid = store.reserve();
    let nameless = store.reserve();
    let leaf = |store: &mut Store, name: &str, parent| {
        store.put(dict! { "T" => Object::text(name), "Parent" => parent, "FT" => Object::name("Tx"), "Rect" => rect() })
    };
    let a = leaf(&mut store, "b.c", top);
    let b = leaf(&mut store, "c", mid);
    let c = leaf(&mut store, "", top);
    let d = leaf(&mut store, "x", nameless);
    let e = leaf(&mut store, "b", top);
    store.set(
        top,
        dict! { "T" => Object::text("a"), "Kids" => refs(&[a, mid, c, nameless, e]) },
    );
    store.set(
        mid,
        dict! { "T" => Object::text("b"), "Parent" => top, "Kids" => refs(&[b]) },
    );
    store.set(nameless, dict! { "Parent" => top, "Kids" => refs(&[d]) });
    let lone = store
        .put(dict! { "T" => Object::text("a.b"), "FT" => Object::name("Tx"), "Rect" => rect() });
    let catalog = catalog_of(refs(&[top, lone]));
    let form = read(&store, &catalog);

    let names: Vec<String> = (0..form.fields().len())
        .map(|i| form.full_name(i).unwrap())
        .collect();
    assert!(names.len() >= 9, "{names:?}");
    let asked = [
        "", "a", "a.", "a.b", "a.b.c", "a.x", "a..x", ".a", "b", "b.c", "c", "x", "a.b.c.d", "aa",
        "a.bb", "a.b.", ".",
    ];
    for name in names.iter().map(String::as_str).chain(asked) {
        let by_scan = names.iter().position(|full| full == name);
        assert_eq!(form.find(name), by_scan, "{name:?} among {names:?}");
    }
    // Two fields share `a.b.c` and two `a.b`; the first of each is found.
    assert_eq!(names.iter().filter(|n| *n == "a.b.c").count(), 2);
    assert_eq!(names.iter().filter(|n| *n == "a.b").count(), 3);
    assert_eq!(form.find("nowhere"), None);
}

#[test]
fn the_forms_da_and_q_stand_behind_a_fields() {
    let mut store = Store::default();
    let plain = store.put(dict! { "T" => Object::text("plain"), "FT" => Object::name("Tx") });
    let catalog = dict! {
        "AcroForm" => dict! {
            "Fields" => refs(&[plain]),
            "DA" => Object::text("/Helv 0 Tf 0 g"),
            "Q" => 2,
            "NeedAppearances" => true,
            "SigFlags" => 3,
            "XFA" => Vec::<Object>::new(),
        },
    };
    let form = read(&store, &catalog);
    assert_eq!(form.default_appearance(), Some(&b"/Helv 0 Tf 0 g"[..]));
    assert_eq!(form.quadding(), Quadding::Right);
    assert!(form.need_appearances());
    assert_eq!(form.sig_flags(), 3);
    assert!(form.has_xfa());
    assert_eq!(form.id(), None);
    let plain = &form.fields()[0];
    assert_eq!(plain.default_appearance(), Some(&b"/Helv 0 Tf 0 g"[..]));
    assert_eq!(plain.quadding(), Quadding::Right);
    // No widget: the field has no /Rect and no /Subtype.
    assert_eq!(plain.widgets().len(), 0);
}

#[test]
fn kids_without_names_are_widgets_and_a_field_can_be_its_own() {
    let mut store = Store::default();
    let (group, a, b, merged) = (
        store.reserve(),
        store.reserve(),
        store.reserve(),
        store.reserve(),
    );
    store.set(
        group,
        dict! {
            "T" => Object::text("group"),
            "FT" => Object::name("Btn"),
            "Ff" => i64::from(FieldFlags::RADIO.bits()),
            "V" => Object::name("b"),
            "Kids" => refs(&[a, b]),
        },
    );
    for (id, state) in [(a, "a"), (b, "b")] {
        store.set(
            id,
            dict! {
                "Parent" => group,
                "Subtype" => Object::name("Widget"),
                "Rect" => rect(),
                "AS" => Object::name(state),
            },
        );
    }
    store.set(
        merged,
        dict! {
            "T" => Object::text("merged"),
            "FT" => Object::name("Btn"),
            "Subtype" => Object::name("Widget"),
            "Rect" => rect(),
        },
    );
    let catalog = catalog_of(refs(&[group, merged]));
    let form = read(&store, &catalog);
    assert_eq!(form.fields().len(), 2);

    let group_field = &form.fields()[0];
    assert_eq!(group_field.kind(), Some(FieldKind::Radio));
    assert!(group_field.is_terminal());
    assert_eq!(group_field.value(), Some(&FieldValue::State(b"b")));
    let widgets = group_field.widgets();
    assert_eq!(widgets.len(), 2);
    assert_eq!(widgets[1].id(), Some(b));
    assert!(!widgets[0].is_merged());
    let annot = widgets[1]
        .annot(&store.objects, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap();
    assert_eq!(annot.rect().x1, 110.0);
    assert_eq!(
        annot.dict().get("AS").and_then(Object::as_name),
        Some(&b"b"[..])
    );

    let merged_field = &form.fields()[1];
    assert_eq!(merged_field.kind(), Some(FieldKind::CheckBox));
    assert_eq!(merged_field.widgets().len(), 1);
    assert!(merged_field.widgets()[0].is_merged());
    assert_eq!(merged_field.widgets()[0].id(), Some(merged));
    assert!(std::ptr::eq(
        merged_field.widgets()[0].dict(),
        merged_field.dict()
    ));
}

#[test]
fn kinds_follow_the_type_and_flags() {
    let mut store = Store::default();
    let mut ids = Vec::new();
    let cases = [
        ("Btn", FieldFlags::default(), FieldKind::CheckBox),
        ("Btn", FieldFlags::RADIO, FieldKind::Radio),
        ("Btn", FieldFlags::PUSHBUTTON, FieldKind::PushButton),
        ("Tx", FieldFlags::COMB, FieldKind::Text),
        ("Ch", FieldFlags::default(), FieldKind::List),
        ("Ch", FieldFlags::COMBO | FieldFlags::EDIT, FieldKind::Combo),
        ("Sig", FieldFlags::default(), FieldKind::Signature),
    ];
    for (ft, flags, _) in cases {
        ids.push(store.put(dict! {
            "T" => Object::text("f"),
            "FT" => Object::name(ft),
            "Ff" => i64::from(flags.bits()),
        }));
    }
    let catalog = catalog_of(refs(&ids));
    let form = read(&store, &catalog);
    for (field, (ft, flags, kind)) in form.fields().iter().zip(cases) {
        assert_eq!(field.kind(), Some(kind));
        assert_eq!(field.flags(), flags);
        assert_eq!(field.field_type().unwrap().name(), ft);
    }
    assert_eq!(FieldType::from_name(b"Other"), None);

    // Both button flags: ISO 32000-2 reads a push button, PDFium a radio.
    let both = store.put(dict! {
        "T" => Object::text("both"),
        "FT" => Object::name("Btn"),
        "Ff" => i64::from((FieldFlags::PUSHBUTTON | FieldFlags::RADIO).bits()),
    });
    let catalog = catalog_of(refs(&[both]));
    assert_eq!(
        read(&store, &catalog).fields()[0].kind(),
        Some(FieldKind::Radio)
    );
    let iso = Form::read(&store.objects, &catalog, Rules::Iso, &Limits::DEFAULT)
        .unwrap()
        .unwrap();
    assert_eq!(iso.fields()[0].kind(), Some(FieldKind::PushButton));
}

#[test]
fn values_are_typed_as_written() {
    let mut store = Store::default();
    let text_stream = store.put(Object::from(Stream::new(Dict::new(), b"long".to_vec())));
    let item = store.put(Object::text("two"));
    let list = store.put(dict! {
        "T" => Object::text("list"),
        "FT" => Object::name("Ch"),
        "V" => vec![Object::text("one"), Object::from(item), Object::from(7)],
    });
    let sig = store.put(dict! {
        "T" => Object::text("sig"),
        "FT" => Object::name("Sig"),
        "V" => dict! { "Type" => Object::name("Sig") },
    });
    let long = store.put(dict! {
        "T" => Object::text("long"),
        "FT" => Object::name("Tx"),
        "V" => text_stream,
    });
    let odd = store.put(dict! {
        "T" => Object::text("odd"),
        "FT" => Object::name("Tx"),
        "V" => 4,
        "DV" => Object::from(Value::Null),
    });
    let utf16 = store.put(dict! {
        "T" => Object::text("Łukasz"),
        "FT" => Object::name("Tx"),
        "V" => Object::text("Ölçer"),
    });
    let catalog = catalog_of(refs(&[list, sig, long, odd, utf16]));
    let (form, faults) = read_told(&store, &catalog, &Limits::DEFAULT);
    let value = |name: &str| form.fields()[form.find(name).unwrap()].value().cloned();
    assert_eq!(
        value("list"),
        Some(FieldValue::Items(vec!["one".into(), "two".into()]))
    );
    assert!(matches!(value("sig"), Some(FieldValue::Dict(d)) if d.has_type("Sig")));
    assert_eq!(value("long"), Some(FieldValue::Stream(Some(text_stream))));
    assert_eq!(value("odd"), None);
    assert_eq!(value("Łukasz"), Some(FieldValue::Text("Ölçer".into())));
    assert_eq!(
        faults,
        [Fault::WrongType { key: "V" }, Fault::WrongType { key: "V" }]
    );
}

#[test]
fn a_cycle_and_a_field_listed_twice_are_each_read_once() {
    let mut store = Store::default();
    let (a, b) = (store.reserve(), store.reserve());
    store.set(
        a,
        dict! { "T" => Object::text("a"), "FT" => Object::name("Tx"), "Kids" => refs(&[b]) },
    );
    store.set(
        b,
        dict! { "T" => Object::text("b"), "Parent" => a, "Kids" => refs(&[a, b]) },
    );
    let catalog = catalog_of(refs(&[a, a]));
    let (form, faults) = read_told(&store, &catalog, &Limits::DEFAULT);
    assert_eq!(form.fields().len(), 2);
    assert_eq!(form.roots(), [0]);
    assert_eq!(form.full_name(1).as_deref(), Some("a.b"));
    assert_eq!(
        faults,
        [
            Fault::FieldLoop(a),
            Fault::FieldLoop(b),
            Fault::FieldLoop(a)
        ]
    );
}

#[test]
fn departures_are_faults_and_the_form_still_reads() {
    let mut store = Store::default();
    let parent = store.reserve();
    let stray = store.put(dict! { "T" => Object::text("stray"), "FT" => Object::name("Tx") });
    let untyped = store.put(dict! { "T" => Object::text("untyped"), "Parent" => parent });
    store.set(
        parent,
        dict! { "T" => Object::text("parent"), "Kids" => refs(&[stray, untyped]) },
    );
    let nameless = store.put(dict! { "FT" => Object::name("Tx"), "Rect" => rect() });
    let number = store.put(7);
    let dangling = ObjectId::new(99, 0);
    let fields = vec![
        Object::from(parent),
        Object::from(nameless),
        Object::from(number),
        Object::from(dangling),
        Object::from(dict! { "T" => Object::text("direct"), "FT" => Object::name("Zz") }),
    ];
    let catalog = catalog_of(fields);
    let (form, faults) = read_told(&store, &catalog, &Limits::DEFAULT);
    assert_eq!(
        faults,
        [
            Fault::ParentMismatch,
            Fault::MissingFieldType,
            Fault::MissingName,
            Fault::FieldNotDict,
            Fault::DirectField,
            Fault::WrongType { key: "FT" },
            Fault::MissingFieldType,
        ]
    );
    let names: Vec<_> = (0..form.fields().len())
        .map(|i| form.full_name(i).unwrap())
        .collect();
    assert_eq!(
        names,
        ["parent", "parent.stray", "parent.untyped", "", "direct"]
    );
    assert_eq!(form.fields()[4].id(), None);
    assert_eq!(form.fields()[4].field_type(), None);
    // The same form without a sink.
    assert_eq!(read(&store, &catalog).fields().len(), 5);
}

#[test]
fn a_real_for_an_integer_is_taken_only_under_pdfiums_rules() {
    let mut store = Store::default();
    let field = store.put(dict! {
        "T" => Object::text("f"),
        "FT" => Object::name("Tx"),
        "Ff" => Object::from(4096.0),
        "MaxLen" => -3,
    });
    let catalog = catalog_of(refs(&[field]));
    let pdfium = read(&store, &catalog);
    assert!(pdfium.fields()[0].flags().contains(FieldFlags::MULTILINE));
    assert_eq!(pdfium.fields()[0].max_len(), None);
    let iso = Form::read(&store.objects, &catalog, Rules::Iso, &Limits::DEFAULT)
        .unwrap()
        .unwrap();
    assert_eq!(iso.fields()[0].flags(), FieldFlags::default());
}

/// Under PDFium's rules a field with no `/MaxLen` of its own or an
/// ancestor's takes the first of its widgets' own: the first widget whose
/// dictionary has the entry, whatever it holds. An entry the field or an
/// ancestor holds keeps the widgets unread, `null` included, as PDFium finds
/// it; a reference to no object is none. Under ISO 32000-2's rules the entry
/// is a field's alone, and a widget's is never read.
#[test]
fn a_widgets_own_max_len_is_read_under_pdfiums_rules_alone() {
    let mut store = Store::default();
    let missing = store.reserve();
    let null = store.put(Object::from(Value::Null));
    // A field of widget kids, each with the `/MaxLen` given or none, under
    // `parent`, holding `own` itself.
    let field = |store: &mut Store,
                 name: &str,
                 own: Option<Object>,
                 parent: Option<ObjectId>,
                 widgets: Vec<Option<Object>>| {
        let id = store.reserve();
        let kids: Vec<_> = (widgets.into_iter())
            .map(|max_len| {
                let mut kid = dict! {
                    "Parent" => id,
                    "Subtype" => Object::name("Widget"),
                    "Rect" => rect(),
                };
                if let Some(max_len) = max_len {
                    kid.push("MaxLen", max_len);
                }
                store.put(kid)
            })
            .collect();
        let mut dict = dict! { "T" => Object::text(name), "FT" => Object::name("Tx") };
        if let Some(own) = own {
            dict.push("MaxLen", own);
        }
        if let Some(parent) = parent {
            dict.push("Parent", parent);
        }
        (id, dict, kids)
    };
    let n = |n: i64| Some(Object::from(n));
    let mut roots = Vec::new();
    let mut fields = Vec::new();
    for (name, own, widgets) in [
        ("second", None, vec![None, n(4)]),
        ("first", None, vec![n(7), n(4)]),
        ("own", n(6), vec![None, n(4)]),
        ("zero", None, vec![n(0), n(4)]),
        ("real", None, vec![Some(Object::from(3.7)), None]),
        ("text", None, vec![Some(Object::text("4")), n(4)]),
        ("negative", None, vec![n(-2), n(4)]),
        ("dangling", None, vec![Some(Object::from(missing)), n(4)]),
        ("own-text", Some(Object::text("x")), vec![n(4)]),
        ("none", None, vec![None, None]),
        ("own-null", Some(Object::from(Value::Null)), vec![n(4)]),
        ("own-to-null", Some(Object::from(null)), vec![n(4)]),
        ("own-dangling", Some(Object::from(missing)), vec![n(4)]),
        (
            "first-null",
            None,
            vec![Some(Object::from(Value::Null)), n(4)],
        ),
    ] {
        let (id, mut dict, kids) = field(&mut store, name, own, None, widgets);
        dict.push("Kids", refs(&kids));
        store.set(id, dict);
        roots.push(id);
        fields.push(name);
    }
    // A field's own merged widget, and fields under a parent that has a
    // `/MaxLen`, under one that has none, and under one whose is `null`.
    roots.push(store.put(dict! {
        "T" => Object::text("merged"),
        "FT" => Object::name("Tx"),
        "Subtype" => Object::name("Widget"),
        "Rect" => rect(),
    }));
    for (name, own) in [
        ("held", n(5)),
        ("bare", None),
        ("held-null", Some(Object::from(Value::Null))),
    ] {
        let (parent, mut dict, _) = field(&mut store, name, own, None, Vec::new());
        let (kid, mut kid_dict, kids) = field(&mut store, "kid", None, Some(parent), vec![n(4)]);
        kid_dict.push("Kids", refs(&kids));
        store.set(kid, kid_dict);
        dict.push("Kids", refs(&[kid]));
        store.set(parent, dict);
        roots.push(parent);
    }
    let catalog = catalog_of(refs(&roots));

    let read = |rules| {
        let mut sink: Vec<(Place, Fault)> = Vec::new();
        let form = Form::read_with(&store.objects, &catalog, rules, &Limits::DEFAULT, &mut sink)
            .unwrap()
            .unwrap();
        let max_lens: Vec<_> = [
            "merged",
            "held",
            "held.kid",
            "bare",
            "bare.kid",
            "held-null",
            "held-null.kid",
        ]
        .iter()
        .chain(&fields)
        .map(|name| (*name, form.fields()[form.find(name).unwrap()].max_len()))
        .collect();
        let faults: Vec<_> = sink.into_iter().map(|(_, fault)| fault).collect();
        // The same form read without a sink.
        let quiet = Form::read(&store.objects, &catalog, rules, &Limits::DEFAULT)
            .unwrap()
            .unwrap();
        let quiet: Vec<_> = quiet.fields().iter().map(Field::max_len).collect();
        assert_eq!(
            quiet,
            form.fields().iter().map(Field::max_len).collect::<Vec<_>>()
        );
        (max_lens, faults)
    };

    let (pdfium, faults) = read(Rules::Pdfium);
    assert_eq!(
        pdfium,
        [
            ("merged", None),
            ("held", Some(5)),
            ("held.kid", Some(5)),
            ("bare", None),
            ("bare.kid", Some(4)),
            ("held-null", None),
            ("held-null.kid", None),
            ("second", Some(4)),
            ("first", Some(7)),
            ("own", Some(6)),
            ("zero", Some(0)),
            ("real", Some(3)),
            ("text", None),
            ("negative", None),
            ("dangling", None),
            ("own-text", None),
            ("none", None),
            ("own-null", None),
            ("own-to-null", None),
            ("own-dangling", Some(4)),
            ("first-null", None),
        ]
    );
    // The widgets' entries read are told as any field's are: the real, the
    // text, and the field's own text.
    assert_eq!(faults, vec![Fault::WrongType { key: "MaxLen" }; 3]);

    // Under ISO 32000-2's rules, the field's own or an ancestor's alone.
    let (iso, faults) = read(Rules::Iso);
    let own = |name: &str| match name {
        "own" => Some(6),
        "held" | "held.kid" => Some(5),
        _ => None,
    };
    assert_eq!(
        iso,
        iso.iter()
            .map(|&(name, _)| (name, own(name)))
            .collect::<Vec<_>>()
    );
    assert_eq!(faults, [Fault::WrongType { key: "MaxLen" }]);
}

#[test]
fn the_caps_hold() {
    let mut store = Store::default();
    // A chain six deep, each field with one widget kid beside its field kid.
    let ids: Vec<_> = (0..6).map(|_| store.reserve()).collect();
    for (i, &id) in ids.iter().enumerate() {
        let widget = store.put(dict! { "Parent" => id, "Rect" => rect() });
        let mut kids = vec![Object::from(widget)];
        if let Some(&next) = ids.get(i + 1) {
            kids.push(Object::from(next));
        }
        let mut field =
            dict! { "T" => Object::text("n"), "FT" => Object::name("Tx"), "Kids" => kids };
        if i > 0 {
            field.push("Parent", ids[i - 1]);
        }
        store.set(id, field);
    }
    let catalog = catalog_of(refs(&ids[..1]));

    let mut limits = Limits::DEFAULT;
    limits.depth = 3;
    let (form, faults) = read_told(&store, &catalog, &limits);
    assert_eq!(form.fields().len(), 3);
    assert_eq!(faults, [Fault::TooDeep]);

    let mut limits = Limits::DEFAULT;
    limits.fields = 12;
    assert_eq!(read(&store, &catalog).fields().len(), 6);
    assert!(Form::read(&store.objects, &catalog, Rules::Pdfium, &limits).is_ok());
    limits.fields = 11;
    assert_eq!(
        Form::read(&store.objects, &catalog, Rules::Pdfium, &limits).unwrap_err(),
        Error::Limit(Cap::Fields)
    );
    assert_eq!(
        Error::Limit(Cap::Fields).to_string(),
        "exceeded cap on fields and widgets in a form"
    );

    let mut store = Store::default();
    let list = store.put(dict! {
        "T" => Object::text("list"),
        "FT" => Object::name("Ch"),
        "V" => vec![Object::text("a"), Object::text("b"), Object::text("c")],
    });
    let catalog = catalog_of(refs(&[list]));
    let mut limits = Limits::DEFAULT;
    limits.items = 2;
    assert_eq!(
        Form::read(&store.objects, &catalog, Rules::Pdfium, &limits).unwrap_err(),
        Error::Limit(Cap::Items)
    );

    // A chain of references longer than the hop cap reads as nothing.
    let mut store = Store::default();
    let ids: Vec<_> = (0..5).map(|_| store.reserve()).collect();
    for pair in ids.windows(2) {
        store.set(pair[0], Object::from(pair[1]));
    }
    store.set(
        ids[4],
        dict! { "T" => Object::text("far"), "FT" => Object::name("Tx") },
    );
    let catalog = catalog_of(refs(&ids[..1]));
    let mut limits = Limits::DEFAULT;
    limits.hops = 4;
    let (form, faults) = read_told(&store, &catalog, &limits);
    assert_eq!(form.fields().len(), 0);
    assert_eq!(faults, [Fault::ReferenceLoop]);
    assert_eq!(read(&store, &catalog).fields().len(), 1);
}

#[test]
fn a_value_still_encrypted_is_unreadable() {
    let mut store = Store::default();
    let locked = Object::from(Value::String(
        Str::Encrypted(b"\x01\x02".to_vec().into()),
        None,
    ));
    let field =
        store.put(dict! { "T" => Object::text("f"), "FT" => Object::name("Tx"), "V" => locked });
    let catalog = catalog_of(refs(&[field]));
    let err = Form::read(&store.objects, &catalog, Rules::Pdfium, &Limits::DEFAULT).unwrap_err();
    assert!(matches!(err, Error::Unreadable(u) if u.why == Why::Locked));
}

#[test]
fn default_resources_fonts_are_found_by_name() {
    let mut store = Store::default();
    let helv = store
        .put(dict! { "Type" => Object::name("Font"), "BaseFont" => Object::name("Helvetica") });
    let dr = store.put(dict! {
        "Font" => dict! {
            "Helv" => helv,
            "Direct" => dict! { "Type" => Object::name("Font") },
            "Wrong" => 3,
        },
    });
    let acroform = store.put(dict! { "Fields" => Vec::<Object>::new(), "DR" => dr });
    let catalog = dict! { "AcroForm" => acroform };
    let form = read(&store, &catalog);
    assert_eq!(form.id(), Some(acroform));
    assert_eq!(form.resources().unwrap().1, Some(dr));
    assert_eq!(
        form.font_names().collect::<Vec<_>>(),
        [&b"Helv"[..], b"Direct", b"Wrong"]
    );
    let font = |name: &[u8]| form.font(&store.objects, name, &Limits::DEFAULT).unwrap();
    let (dict, id) = font(b"Helv").unwrap();
    assert_eq!(id, Some(helv));
    assert!(dict.has_type("Font"));
    assert_eq!(font(b"Direct").unwrap().1, None);
    assert!(font(b"Wrong").is_none());
    assert!(font(b"Absent").is_none());
}

/// The name a font is put under in the default resources' fonts: the one
/// wanted where it's free, `null` or the font's own, else the first number
/// after it that is, never another font's; a key written twice taken by its
/// last value, as a lookup reads it; and found within a number past every
/// key the dictionary has.
#[test]
fn a_fonts_name_in_the_default_resources_is_no_other_fonts() {
    use std::borrow::Cow;

    use crate::default_resources::font_name;

    let font = ObjectId::new(12, 0);
    let other = Object::reference(7);
    let named = |fonts: &Dict, wanted: &[u8]| font_name(Some(fonts), wanted, font).into_owned();

    // Free, or the font's own: as wanted, borrowed.
    let fonts = dict! { "Helv" => other.clone(), "Mine" => font, "Gone" => Object::null() };
    for wanted in [&b"F1"[..], b"Mine", b"Gone"] {
        assert!(matches!(
            font_name(Some(&fonts), wanted, font),
            Cow::Borrowed(name) if name == wanted
        ));
    }
    assert_eq!(font_name(None, b"Helv", font), &b"Helv"[..]);
    assert_eq!(font_name(Some(&Dict::new()), b"Helv", font), &b"Helv"[..]);

    // Another font's: the first number after it that's free, null, or the
    // font's own.
    assert_eq!(named(&fonts, b"Helv"), b"Helv1");
    let mut fonts = dict! {
        "Helv" => other.clone(),
        "Helv1" => other.clone(),
        "Helv2" => Object::null(),
        "Helv3" => font,
    };
    assert_eq!(named(&fonts, b"Helv"), b"Helv2");
    fonts.set("Helv2", other.clone());
    assert_eq!(named(&fonts, b"Helv"), b"Helv3");
    // A font held directly, the font's number at another generation,
    // anything else: another's.
    let fonts = dict! {
        "Helv" => dict! { "Type" => Object::name("Font") },
        "Helv1" => ObjectId::new(12, 1),
        "Helv2" => 4,
    };
    assert_eq!(named(&fonts, b"Helv"), b"Helv3");
    // A key written twice is its last value's.
    let mut twice = dict! { "Helv" => other.clone() };
    twice.push("Helv1", font);
    twice.push("Helv1", other.clone());
    assert_eq!(named(&twice, b"Helv"), b"Helv2");
    let mut twice = dict! { "Helv" => other.clone() };
    twice.push("Helv1", other.clone());
    twice.push("Helv1", font);
    assert_eq!(named(&twice, b"Helv"), b"Helv1");
    // An empty name, numbered as any other.
    let fonts = dict! { "" => other.clone() };
    assert_eq!(named(&fonts, b""), b"1");

    // Every number up to the dictionary's length taken: the next.
    let mut full = dict! { "F" => other.clone() };
    for n in 1..1000 {
        full.set(format!("F{n}").into_bytes(), other.clone());
    }
    assert_eq!(full.len(), 1000);
    assert_eq!(named(&full, b"F"), b"F1000");
}

#[test]
fn debug_never_prints_a_name_or_a_value() {
    let mut store = Store::default();
    let widget = store.reserve();
    let field = store.put(dict! {
        "T" => Object::text("SECRETNAME"),
        "FT" => Object::name("Tx"),
        "V" => Object::text("SECRETVALUE"),
        "DV" => vec![Object::text("SECRETITEM")],
        "DA" => Object::text("/SECRETFONT 9 Tf"),
        "Kids" => refs(&[widget]),
    });
    store.set(
        widget,
        dict! { "Parent" => field, "Rect" => rect(), "TU" => Object::text("SECRETTIP") },
    );
    let state = store.put(dict! { "T" => Object::text("b"), "FT" => Object::name("Btn"), "V" => Object::name("SECRETSTATE") });
    let catalog = catalog_of(refs(&[field, state]));
    let (form, faults) = read_told(&store, &catalog, &Limits::DEFAULT);
    let shown = format!(
        "{form:?} {:?} {:?} {faults:?} {:?}",
        form.fields(),
        form.fields()[0].widgets(),
        form.fields()[0].value(),
    );
    assert!(!shown.contains("SECRET"), "{shown}");
    assert!(shown.contains("Text(11 bytes)"));
}

#[test]
fn fuzz_seeds_pass_the_checks() {
    for seed in fuzz::SEEDS {
        fuzz::check(seed);
    }
    // Each seed cut short and with a byte flipped, too.
    for seed in fuzz::SEEDS {
        for cut in (0..seed.len()).step_by(7) {
            fuzz::check(&seed[..cut]);
            let mut flipped = seed.to_vec();
            flipped[cut] ^= 0x20;
            fuzz::check(&flipped);
        }
    }
}
