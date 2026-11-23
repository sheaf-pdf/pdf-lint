//! What the read layer gives of buttons and choices: a field's options, top
//! index, selected indexes and selection, a widget's states, and a field's
//! group; each read with no feature.

use std::collections::BTreeMap;

use pdf_objects::fault::Place;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Str, Unreadable, Value, Why, dict};

use crate::group::GroupKind;
use crate::{Cap, Error, Fault, Field, FieldOption, Form, Limits, WidgetStates, fuzz, selection};

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

    /// A form XObject, as a state's appearance.
    fn form(&mut self) -> ObjectId {
        self.put(pdf_objects::Stream::new(
            dict! { "Type" => Object::name("XObject"), "Subtype" => Object::name("Form") },
            Vec::new(),
        ))
    }
}

fn refs(ids: &[ObjectId]) -> Vec<Object> {
    ids.iter().map(|&id| Object::from(id)).collect()
}

fn rect() -> Vec<Object> {
    [10, 20, 30, 40].into_iter().map(Object::from).collect()
}

fn texts(items: &[&str]) -> Vec<Object> {
    items.iter().map(|&t| Object::text(t)).collect()
}

fn catalog_of(fields: Vec<Object>) -> Dict {
    dict! { "AcroForm" => dict! { "Fields" => fields } }
}

fn read<'o>(store: &'o Store, catalog: &'o Dict) -> Form<'o> {
    Form::read(&store.objects, catalog, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap()
        .unwrap()
}

fn field<'f, 'o>(form: &'f Form<'o>, name: &str) -> &'f Field<'o> {
    &form.fields()[form.find(name).unwrap()]
}

fn options_told<'o>(
    store: &'o Store,
    field: &Field<'o>,
    rules: Rules,
) -> (Vec<FieldOption>, Vec<Fault>) {
    let mut sink: Vec<(Place, Fault)> = Vec::new();
    let options = field
        .options_with(&store.objects, rules, &Limits::DEFAULT, &mut sink)
        .unwrap();
    (options, sink.into_iter().map(|(_, fault)| fault).collect())
}

fn pair(export: &str, label: &str) -> Object {
    Object::from(texts(&[export, label]))
}

fn option(export: &str, label: Option<&str>) -> FieldOption {
    let mut option = FieldOption::new(export);
    option.label = label.map(str::to_owned);
    option
}

#[test]
fn options_are_strings_pairs_and_wrong_items_in_their_places() {
    let mut store = Store::default();
    let label = store.put(Object::text("by reference"));
    let referred = store.put(Object::from(vec![Object::text("r"), Object::from(label)]));
    let list = store.put(dict! {
        "T" => Object::text("list"),
        "FT" => Object::name("Ch"),
        "Opt" => vec![
            Object::text("alpha"),
            pair("e2", "Label two"),
            Object::from(7),
            Object::from(referred),
            Object::from(vec![Object::text("one")]),
            Object::from(vec![Object::text("a"), Object::from(1)]),
            Object::null(),
            Object::name("name"),
            Object::text("caf\u{e9}"),
        ],
        "Subtype" => Object::name("Widget"),
        "Rect" => rect(),
    });
    let catalog = catalog_of(refs(&[list]));
    let form = read(&store, &catalog);
    let list = field(&form, "list");
    let expected = vec![
        option("alpha", None),
        option("e2", Some("Label two")),
        FieldOption::default(),
        option("r", Some("by reference")),
        FieldOption::default(),
        FieldOption::default(),
        FieldOption::default(),
        FieldOption::default(),
        option("caf\u{e9}", None),
    ];
    for rules in [Rules::Iso, Rules::Pdfium] {
        let (options, faults) = options_told(&store, list, rules);
        assert_eq!(options, expected);
        // Each wrong item told once, where it is; none without a sink.
        assert_eq!(faults, vec![Fault::WrongType { key: "Opt" }; 5]);
        let quiet = list
            .options(&store.objects, rules, &Limits::DEFAULT)
            .unwrap();
        assert_eq!(quiet, expected);
    }
    assert_eq!(expected[1].label(), "Label two");
    assert_eq!(expected[0].label(), "alpha");
    assert_eq!(expected[2].label(), "");

    // An `/Opt` that is no array is no options, told.
    let mut store = Store::default();
    let wrong = store.put(dict! {
        "T" => Object::text("w"),
        "FT" => Object::name("Ch"),
        "Opt" => Object::text("alpha"),
    });
    let catalog = catalog_of(refs(&[wrong]));
    let form = read(&store, &catalog);
    let (options, faults) = options_told(&store, field(&form, "w"), Rules::Iso);
    assert_eq!(options, []);
    assert_eq!(faults, [Fault::WrongType { key: "Opt" }]);
}

#[test]
fn opt_ti_and_i_are_inherited_for_a_button_and_a_choice() {
    let mut store = Store::default();
    let (choices, list, buttons, check) = (
        store.reserve(),
        store.reserve(),
        store.reserve(),
        store.reserve(),
    );
    let shared = store.put(Object::from(texts(&["x", "y", "z"])));
    store.set(
        choices,
        dict! {
            "T" => Object::text("choices"),
            "FT" => Object::name("Ch"),
            "Opt" => shared,
            "TI" => 1,
            "I" => vec![Object::from(2)],
            "Kids" => refs(&[list]),
        },
    );
    store.set(
        list,
        dict! {
            "T" => Object::text("list"),
            "Parent" => choices,
            "V" => Object::text("z"),
            "Subtype" => Object::name("Widget"),
            "Rect" => rect(),
        },
    );
    store.set(
        buttons,
        dict! {
            "T" => Object::text("buttons"),
            "FT" => Object::name("Btn"),
            "Opt" => texts(&["apple", "pear"]),
            "Kids" => refs(&[check]),
        },
    );
    let (w0, w1) = (store.reserve(), store.reserve());
    store.set(
        check,
        dict! { "T" => Object::text("check"), "Parent" => buttons, "Kids" => refs(&[w0, w1]) },
    );
    for (i, w) in [w0, w1].into_iter().enumerate() {
        let on = store.form();
        let name = if i == 0 { "0" } else { "1" };
        store.set(
            w,
            dict! {
                "Parent" => check,
                "Subtype" => Object::name("Widget"),
                "Rect" => rect(),
                "AS" => Object::name("Off"),
                "AP" => dict! { "N" => dict! { name => on } },
            },
        );
    }
    let catalog = catalog_of(refs(&[choices, buttons]));
    for rules in [Rules::Iso, Rules::Pdfium] {
        let form = Form::read(&store.objects, &catalog, rules, &Limits::DEFAULT)
            .unwrap()
            .unwrap();
        let list = field(&form, "choices.list");
        let options = list
            .options(&store.objects, rules, &Limits::DEFAULT)
            .unwrap();
        assert_eq!(
            options,
            [option("x", None), option("y", None), option("z", None)]
        );
        assert_eq!(list.top_index(), 1);
        assert_eq!(
            list.selected(&store.objects, rules, &Limits::DEFAULT)
                .unwrap(),
            [2]
        );
        assert_eq!(
            list.selection(&store.objects, rules, &Limits::DEFAULT)
                .unwrap(),
            [2]
        );
        // The field's own dictionary holds none of them.
        assert!(list.dict().get("Opt").is_none());

        let check = field(&form, "buttons.check");
        let exports = check
            .options(&store.objects, rules, &Limits::DEFAULT)
            .unwrap();
        assert_eq!(exports, [option("apple", None), option("pear", None)]);
        let read = check
            .group(&store.objects, rules, &Limits::DEFAULT)
            .unwrap();
        let group = read.group();
        assert_eq!(group.export_value(1).as_deref(), Some("pear"));
        assert_eq!(group.on_state(1).as_deref(), Some(&b"1"[..]));
    }
}

#[test]
fn a_fields_own_entries_come_before_its_parents() {
    let mut store = Store::default();
    let (parent, kid, none) = (store.reserve(), store.reserve(), store.reserve());
    store.set(
        parent,
        dict! {
            "T" => Object::text("p"),
            "FT" => Object::name("Ch"),
            "Opt" => texts(&["a"]),
            "TI" => 3,
            "I" => vec![Object::from(0)],
            "Kids" => refs(&[kid, none]),
        },
    );
    // Its own `/Opt`, a negative `/TI`, and an `/I` that reaches nothing,
    // which inherits.
    store.set(
        kid,
        dict! {
            "T" => Object::text("k"),
            "Parent" => parent,
            "Opt" => texts(&["b", "c"]),
            "TI" => -2,
            "I" => ObjectId::new(99, 0),
        },
    );
    store.set(
        none,
        dict! { "T" => Object::text("n"), "Parent" => parent, "Opt" => Object::null() },
    );
    let catalog = catalog_of(refs(&[parent]));
    let form = read(&store, &catalog);
    let kid = field(&form, "p.k");
    let options = kid
        .options(&store.objects, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap();
    assert_eq!(options, [option("b", None), option("c", None)]);
    assert_eq!(kid.top_index(), 0);
    assert_eq!(
        kid.selected(&store.objects, Rules::Pdfium, &Limits::DEFAULT)
            .unwrap(),
        [0]
    );
    // A null `/Opt` is none, and the parent's is taken.
    let none = field(&form, "p.n");
    let options = none
        .options(&store.objects, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap();
    assert_eq!(options, [option("a", None)]);
    assert_eq!(none.top_index(), 3);
    // A field with none anywhere.
    let mut store = Store::default();
    let bare = store.put(dict! { "T" => Object::text("b"), "FT" => Object::name("Ch") });
    let catalog = catalog_of(refs(&[bare]));
    let form = read(&store, &catalog);
    let bare = field(&form, "b");
    assert_eq!(
        bare.options(&store.objects, Rules::Iso, &Limits::DEFAULT)
            .unwrap(),
        []
    );
    assert_eq!(
        bare.selected(&store.objects, Rules::Iso, &Limits::DEFAULT)
            .unwrap(),
        []
    );
    assert_eq!(
        bare.selection(&store.objects, Rules::Iso, &Limits::DEFAULT)
            .unwrap(),
        []
    );
    assert_eq!(bare.top_index(), 0);
}

#[test]
fn selected_indexes_are_read_as_given_and_wrong_ones_left_out() {
    let mut store = Store::default();
    let two = store.put(Object::from(2));
    let fields: Vec<_> = [
        Object::from(vec![Object::from(3), Object::from(0), Object::from(two)]),
        Object::from(vec![
            Object::from(-1),
            Object::name("x"),
            Object::from(1.9),
            Object::null(),
            Object::from(4),
        ]),
        Object::from(1),
        Object::text("1"),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, selected)| {
        store.put(dict! {
            "T" => Object::text(&format!("f{i}")),
            "FT" => Object::name("Ch"),
            "I" => selected,
        })
    })
    .collect();
    let catalog = catalog_of(refs(&fields));
    let form = read(&store, &catalog);
    let told = |name: &str, rules: Rules| {
        let mut sink: Vec<(Place, Fault)> = Vec::new();
        let selected = field(&form, name)
            .selected_with(&store.objects, rules, &Limits::DEFAULT, &mut sink)
            .unwrap();
        (selected, sink.len())
    };
    for rules in [Rules::Iso, Rules::Pdfium] {
        assert_eq!(told("f0", rules), (vec![3, 0, 2], 0));
        assert_eq!(told("f2", rules), (vec![1], 1));
        assert_eq!(told("f3", rules), (vec![], 1));
    }
    // A real is an index only under PDFium's rules, cut, and told either way.
    assert_eq!(told("f1", Rules::Iso), (vec![4], 4));
    assert_eq!(told("f1", Rules::Pdfium), (vec![1, 4], 4));
}

#[test]
fn a_selection_takes_i_only_where_it_agrees_with_v() {
    let options: Vec<_> = ["alpha", "bravo", "charlie", "bravo"]
        .into_iter()
        .map(FieldOption::new)
        .collect();
    // `/I` agrees: the values each option's export value, as many of each.
    assert_eq!(selection(&options, &[3], &["bravo"]), [3]);
    assert_eq!(selection(&options, &[2, 0], &["alpha", "charlie"]), [0, 2]);
    assert_eq!(selection(&options, &[1, 3], &["bravo", "bravo"]), [1, 3]);
    // It disagrees: another option, another count, an index of none, a
    // repeat that the values don't repeat.
    assert_eq!(selection(&options, &[0], &["charlie"]), [2]);
    assert_eq!(selection(&options, &[0, 2], &["charlie"]), [2]);
    assert_eq!(selection(&options, &[9], &["charlie"]), [2]);
    assert_eq!(selection(&options, &[2, 2], &["charlie", "alpha"]), [0, 2]);
    // By the values, every option of one of them: both bravos.
    assert_eq!(selection(&options, &[], &["bravo"]), [1, 3]);
    assert_eq!(selection(&options, &[], &["typed"]), []);
    // With no value, the indices of an option, sorted and each once.
    assert_eq!(selection(&options, &[2, 0, 2, 9], &[]), [0, 2]);
    assert_eq!(selection(&[], &[0], &["alpha"]), []);
    assert_eq!(selection(&[], &[], &[]), []);

    // Through a field: `/V (e2) /I [0]` shows the second, `/V (e1) /I [0]`
    // the first, and a list's `/V` array with an `/I` that agrees.
    let mut store = Store::default();
    let opt = vec![pair("e1", "Label one"), pair("e2", "Label two")];
    let fields = [
        ("against", Object::text("e2"), vec![Object::from(0)]),
        ("with", Object::text("e1"), vec![Object::from(0)]),
        (
            "both",
            Object::from(texts(&["e1", "e2"])),
            vec![Object::from(1), Object::from(0)],
        ),
        ("name", Object::name("e2"), vec![Object::from(1)]),
    ]
    .map(|(name, value, selected)| {
        store.put(dict! {
            "T" => Object::text(name),
            "FT" => Object::name("Ch"),
            "Opt" => opt.clone(),
            "V" => value,
            "I" => selected,
        })
    });
    let catalog = catalog_of(refs(&fields));
    let form = read(&store, &catalog);
    let selected = |name: &str| {
        field(&form, name)
            .selection(&store.objects, Rules::Pdfium, &Limits::DEFAULT)
            .unwrap()
    };
    assert_eq!(selected("against"), [1]);
    assert_eq!(selected("with"), [0]);
    assert_eq!(selected("both"), [0, 1]);
    // A value that is no text is none: `/I` is used.
    assert_eq!(selected("name"), [1]);
}

#[test]
fn options_past_the_cap_are_refused_by_options_alone() {
    let mut store = Store::default();
    let mut limits = Limits::DEFAULT;
    limits.items = 3;
    let locked = Object::from(Value::String(Str::Encrypted(b"\x01".to_vec().into()), None));
    let many = store.put(dict! {
        "T" => Object::text("many"),
        "FT" => Object::name("Ch"),
        "Opt" => texts(&["a", "b", "c", "d"]),
        "I" => (0..4).map(Object::from).collect::<Vec<_>>(),
    });
    let wrong = store.put(dict! {
        "T" => Object::text("wrong"),
        "FT" => Object::name("Ch"),
        "Opt" => vec![Object::from(1), locked.clone()],
        "I" => vec![Object::name("x")],
    });
    let locked = store.put(dict! {
        "T" => Object::text("locked"),
        "FT" => Object::name("Ch"),
        "Opt" => vec![pair("a", "b"), Object::from(vec![Object::text("c"), locked])],
    });
    let catalog = catalog_of(refs(&[many, wrong, locked]));
    // The form reads as it would with no `/Opt` or `/I`: nothing is decoded,
    // and nothing of them is told.
    let mut sink: Vec<(Place, Fault)> = Vec::new();
    let form = Form::read_with(&store.objects, &catalog, Rules::Pdfium, &limits, &mut sink)
        .unwrap()
        .unwrap();
    assert_eq!(form.fields().len(), 3);
    assert!(sink.is_empty(), "{sink:?}");

    let many = field(&form, "many");
    let cap = Err(Error::Limit(Cap::Items));
    assert_eq!(many.options(&store.objects, Rules::Pdfium, &limits), cap);
    assert_eq!(
        many.selected(&store.objects, Rules::Pdfium, &limits),
        cap.clone().map(|_: Vec<FieldOption>| Vec::new())
    );
    assert!(matches!(
        many.selection(&store.objects, Rules::Pdfium, &limits),
        Err(Error::Limit(Cap::Items))
    ));
    assert!(matches!(
        many.group(&store.objects, Rules::Pdfium, &limits),
        Ok(group) if group.group().widgets.is_empty()
    ));
    assert_eq!(
        many.options(&store.objects, Rules::Pdfium, &Limits::DEFAULT)
            .unwrap()
            .len(),
        4
    );

    // A string still encrypted is unreadable, in an option or a pair.
    let wrong = field(&form, "wrong");
    let err = wrong
        .options(&store.objects, Rules::Pdfium, &limits)
        .unwrap_err();
    assert!(matches!(err, Error::Unreadable(u) if u.why == Why::Locked));
    let err = (field(&form, "locked").options(&store.objects, Rules::Iso, &limits)).unwrap_err();
    assert!(matches!(err, Error::Unreadable(u) if u.why == Why::Locked));
}

/// A store's objects, but each of `unreadable` can't be read, for the reason
/// given with it.
struct Unreadables<'a> {
    objects: &'a BTreeMap<ObjectId, Object>,
    unreadable: &'a [(ObjectId, Why)],
}

impl<'a> Resolve<'a> for &Unreadables<'a> {
    fn resolve(&self, id: ObjectId) -> Option<&'a Object> {
        self.try_resolve(id).ok().flatten()
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'a Object>, Unreadable> {
        match self.unreadable.iter().find(|(at, _)| *at == id) {
            Some(&(_, why)) => Err(Unreadable::new(id, why, "unreadable")),
            None => Ok(self.objects.get(&id)),
        }
    }
}

/// Why `result` is unreadable; none where it isn't.
fn unreadable<T>(result: Result<T, Error>) -> Option<Why> {
    match result {
        Err(Error::Unreadable(unreadable)) => Some(unreadable.why),
        _ => None,
    }
}

#[test]
fn an_unreadable_opt_or_i_fails_the_call_not_the_read() {
    let mut store = Store::default();
    let (capped, locked, source, damaged) = (
        store.put(texts(&["x"])),
        store.put(texts(&["x"])),
        store.put(texts(&["x"])),
        store.put(texts(&["x"])),
    );
    let looping = store.reserve();
    store.set(looping, looping);
    // A list whose `/Opt` is past a reader's cap and whose `/I` is locked; a
    // check box field whose `/Opt` is past a cap; a kid with no `/Opt` of
    // its own under a field whose `/Opt` the source can't give.
    let list = store.put(dict! {
        "T" => Object::text("list"),
        "FT" => Object::name("Ch"),
        "V" => Object::text("x"),
        "Opt" => capped,
        "I" => locked,
        "Subtype" => Object::name("Widget"),
        "Rect" => rect(),
    });
    let check = store.put(dict! {
        "T" => Object::text("check"),
        "FT" => Object::name("Btn"),
        "Opt" => capped,
        "AS" => Object::name("Off"),
        "Subtype" => Object::name("Widget"),
        "Rect" => rect(),
    });
    let (parent, kid) = (store.reserve(), store.reserve());
    store.set(
        parent,
        dict! {
            "T" => Object::text("p"),
            "FT" => Object::name("Ch"),
            "Opt" => source,
            "Kids" => refs(&[kid]),
        },
    );
    store.set(
        kid,
        dict! { "T" => Object::text("k"), "Parent" => parent, "Rect" => rect() },
    );
    // A list whose own `/Opt` is a damaged object and whose `/I` is a loop,
    // under a field whose own are read: the parent's are taken.
    let (held, bad) = (store.reserve(), store.reserve());
    store.set(
        held,
        dict! {
            "T" => Object::text("h"),
            "FT" => Object::name("Ch"),
            "Opt" => texts(&["a", "b"]),
            "I" => vec![Object::from(1)],
            "Kids" => refs(&[bad]),
        },
    );
    store.set(
        bad,
        dict! {
            "T" => Object::text("b"),
            "Parent" => held,
            "Opt" => damaged,
            "I" => looping,
            "Rect" => rect(),
        },
    );
    let catalog = catalog_of(refs(&[list, check, parent, held]));
    let reasons = [
        (capped, Why::Capped),
        (locked, Why::Locked),
        (source, Why::Source),
        (damaged, Why::Damaged),
    ];
    let objects = Unreadables {
        objects: &store.objects,
        unreadable: &reasons,
    };
    let limits = Limits::DEFAULT;
    for rules in [Rules::Iso, Rules::Pdfium] {
        // The form reads as it would with none of them, telling nothing.
        let mut sink: Vec<(Place, Fault)> = Vec::new();
        let form = Form::read_with(&objects, &catalog, rules, &limits, &mut sink)
            .unwrap()
            .unwrap();
        assert!(sink.is_empty(), "{sink:?}");
        assert_eq!(form.fields().len(), 6);
        let quiet = Form::read(&objects, &catalog, rules, &limits);
        assert_eq!(
            format!("{form:?}"),
            format!("{:?}", quiet.unwrap().unwrap())
        );

        // What can't be read fails the call, inherited or not.
        let list = field(&form, "list");
        assert_eq!(
            unreadable(list.options(&objects, rules, &limits)),
            Some(Why::Capped)
        );
        assert_eq!(
            unreadable(list.selected(&objects, rules, &limits)),
            Some(Why::Locked)
        );
        assert_eq!(
            unreadable(list.selection(&objects, rules, &limits)),
            Some(Why::Capped)
        );
        let check = field(&form, "check");
        assert_eq!(
            unreadable(check.options(&objects, rules, &limits)),
            Some(Why::Capped)
        );
        assert_eq!(
            unreadable(check.group(&objects, rules, &limits)),
            Some(Why::Capped)
        );
        for name in ["p", "p.k"] {
            let options = field(&form, name).options(&objects, rules, &limits);
            assert_eq!(unreadable(options), Some(Why::Source), "{name}");
        }

        // A damaged object and a loop are told by the call, the parent's
        // entries taken.
        let bad = field(&form, "h.b");
        let mut sink: Vec<(Place, Fault)> = Vec::new();
        let options = bad.options_with(&objects, rules, &limits, &mut sink);
        assert_eq!(options, Ok(vec![option("a", None), option("b", None)]));
        assert_eq!(bad.options(&objects, rules, &limits), options);
        let selected = bad.selected_with(&objects, rules, &limits, &mut sink);
        assert_eq!(selected, Ok(vec![1]));
        assert_eq!(bad.selected(&objects, rules, &limits), selected);
        let selection = bad.selection_with(&objects, rules, &limits, &mut sink);
        assert_eq!(selection, Ok(vec![1]));
        let faults: Vec<Fault> = sink.into_iter().map(|(_, fault)| fault).collect();
        let told = [Fault::DamagedObject(damaged), Fault::ReferenceLoop];
        assert_eq!(faults, [&told[..], &told[..]].concat());
        // The parent's own, read, tell nothing.
        let mut sink: Vec<(Place, Fault)> = Vec::new();
        let held = field(&form, "h");
        assert!(
            held.selection_with(&objects, rules, &limits, &mut sink)
                .is_ok()
        );
        assert!(sink.is_empty(), "{sink:?}");
    }
}

/// A radio field of `kids`, each widget's `/AS` and the names of its normal
/// appearance's states.
fn radio_group(store: &mut Store, flags: i64, kids: &[(&str, &[&str])]) -> ObjectId {
    let group = store.reserve();
    let mut widgets = Vec::new();
    for (state, names) in kids {
        let mut normal = Dict::new();
        for name in *names {
            let form = store.form();
            normal.set(*name, form);
        }
        widgets.push(store.put(dict! {
            "Parent" => group,
            "Subtype" => Object::name("Widget"),
            "Rect" => rect(),
            "AS" => Object::name(*state),
            "AP" => dict! { "N" => normal },
        }));
    }
    store.set(
        group,
        dict! {
            "T" => Object::text("g"),
            "FT" => Object::name("Btn"),
            "Ff" => flags,
            "Kids" => refs(&widgets),
        },
    );
    group
}

#[test]
fn a_widgets_states_are_its_on_state_as_and_off() {
    let mut store = Store::default();
    let group = radio_group(
        &mut store,
        1 << 15,
        &[
            ("1", &["1", "Off"]),
            ("Off", &["2"]),
            ("Off", &["Yes", "On", "Off"]),
            ("Off", &[]),
            ("Off", &["Off"]),
        ],
    );
    // One whose `/N` is a stream, one with no `/AP`, one with no `/AS`.
    let stream = store.form();
    let more = [
        dict! { "AS" => Object::name("Off"), "AP" => dict! { "N" => stream } },
        dict! { "AS" => Object::name("1") },
        dict! { "AP" => dict! { "N" => dict! { "1" => stream } } },
    ];
    let mut kids = store.objects[&group]
        .as_dict()
        .unwrap()
        .get("Kids")
        .unwrap()
        .as_array()
        .unwrap()
        .to_vec();
    for mut extra in more {
        extra.set("Parent", group);
        extra.set("Subtype", Object::name("Widget"));
        extra.set("Rect", rect());
        kids.push(Object::from(store.put(extra)));
    }
    let mut dict = store.objects[&group].as_dict().unwrap().clone();
    dict.set("Kids", kids);
    store.set(group, dict);
    let catalog = catalog_of(refs(&[group]));
    let form = read(&store, &catalog);
    let g = field(&form, "g");
    let read = |rules: Rules| {
        (g.widgets().iter())
            .map(|w| w.states(&store.objects, rules, &Limits::DEFAULT).unwrap())
            .collect::<Vec<_>>()
    };
    let states = |on: Option<&[u8]>, state: Option<&[u8]>, has_off: bool| WidgetStates {
        on_state: on.map(Box::from),
        state: state.map(Box::from),
        has_off,
    };
    let iso = read(Rules::Iso);
    assert_eq!(
        iso,
        [
            states(Some(b"1"), Some(b"1"), true),
            states(Some(b"2"), Some(b"Off"), false),
            states(Some(b"Yes"), Some(b"Off"), true),
            states(None, Some(b"Off"), false),
            states(None, Some(b"Off"), true),
            states(None, Some(b"Off"), false),
            states(None, Some(b"1"), false),
            states(Some(b"1"), None, false),
        ]
    );
    assert_eq!(
        iso.iter().map(WidgetStates::is_on).collect::<Vec<_>>(),
        [true, false, false, false, false, false, false, false]
    );
    // Of two on states PDFium takes the least name, its dictionaries kept in
    // order: `On` before `Yes`.
    let pdfium = read(Rules::Pdfium);
    assert_eq!(pdfium[2].on_state.as_deref(), Some(&b"On"[..]));
    assert_eq!((&pdfium[..2], &pdfium[3..]), (&iso[..2], &iso[3..]));

    // The group: the second widget's name its own, those with none `Yes`,
    // and each state by its `/AS`; the one with no `/AP` and `/AS /1` is
    // off, since it would be made under `Yes`.
    let read = g
        .group(&store.objects, Rules::Iso, &Limits::DEFAULT)
        .unwrap();
    let group = read.group();
    assert_eq!(group.kind, GroupKind::Radio);
    assert!(!group.no_toggle_to_off && !group.in_unison);
    assert_eq!(
        read.states(),
        [true, false, false, false, false, false, false, false]
    );
    assert_eq!(group.on_state(3).as_deref(), Some(&b"Yes"[..]));
    let after = group.press(read.states(), 1);
    assert_eq!(
        after,
        [false, true, false, false, false, false, false, false]
    );
    assert_eq!(&*group.value(&after), b"2");
    assert!(!format!("{read:?} {iso:?}").contains("Yes"));
}

#[test]
fn a_groups_flags_and_states_come_from_the_field_and_its_widgets() {
    let mut store = Store::default();
    // RadiosInUnison and NoToggleToOff, two widgets on as `/1`.
    let flags = 1 << 15 | 1 << 14 | 1 << 25;
    let radios = radio_group(
        &mut store,
        flags,
        &[
            ("1", &["1", "Off"]),
            ("Off", &["2", "Off"]),
            ("1", &["1", "Off"]),
        ],
    );
    let check = store.put(dict! {
        "T" => Object::text("c"),
        "FT" => Object::name("Btn"),
        "Opt" => texts(&["apple", "pear", "apple"]),
        "Kids" => Vec::<Object>::new(),
    });
    let mut kids = Vec::new();
    for (i, state) in ["0", "Off", "Off"].into_iter().enumerate() {
        let form = store.form();
        let name = ["0", "1", "2"][i];
        kids.push(store.put(dict! {
            "Parent" => check,
            "Subtype" => Object::name("Widget"),
            "Rect" => rect(),
            "AS" => Object::name(state),
            "AP" => dict! { "N" => dict! { name => form } },
        }));
    }
    let mut dict = store.objects[&check].as_dict().unwrap().clone();
    dict.set("Kids", refs(&kids));
    store.set(check, dict);
    let text = store.put(dict! {
        "T" => Object::text("t"),
        "FT" => Object::name("Tx"),
        "Subtype" => Object::name("Widget"),
        "Rect" => rect(),
    });
    let mut store_catalog = catalog_of(refs(&[radios, check, text]));
    store_catalog.set("Type", Object::name("Catalog"));
    let form = read(&store, &store_catalog);

    let read = field(&form, "g")
        .group(&store.objects, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap();
    let group = read.group();
    assert!(group.no_toggle_to_off && group.in_unison);
    assert_eq!(read.states(), [true, false, true]);
    // The on ones stay on; the second turns the two off.
    assert_eq!(group.press(read.states(), 0), [true, false, true]);
    assert_eq!(group.press(read.states(), 1), [false, true, false]);

    let mut sink: Vec<(Place, Fault)> = Vec::new();
    let check = field(&form, "c");
    let read = check
        .group_with(&store.objects, Rules::Pdfium, &Limits::DEFAULT, &mut sink)
        .unwrap();
    assert!(sink.is_empty());
    let group = read.group();
    assert_eq!(group.kind, GroupKind::CheckBox);
    assert_eq!(read.states(), [true, false, false]);
    // The third, apple too, doesn't follow the first: told apart by index.
    let after = group.press(&[false; 3], 2);
    assert_eq!(after, [false, false, true]);
    assert_eq!(&*group.value(&after), b"2");
    assert_eq!(group.export(&after).as_deref(), Some("apple"));

    // A text field is a group of none.
    let read = field(&form, "t")
        .group(&store.objects, Rules::Iso, &Limits::DEFAULT)
        .unwrap();
    assert!(read.group().widgets.is_empty() && read.states().is_empty());

    // Check boxes with no appearance yet: on where `/AS` names the state
    // they'd be made under, `Yes`, or with `/Opt` their indexes.
    let mut store = Store::default();
    let mut fields = Vec::new();
    for (name, opt, states) in [
        ("plain", None, ["Yes", "Off", "1"]),
        (
            "indexed",
            Some(texts(&["a", "b", "c"])),
            ["Yes", "1", "Off"],
        ),
    ] {
        let id = store.reserve();
        let kids: Vec<_> = (states.iter())
            .map(|&state| {
                store.put(dict! {
                    "Parent" => id,
                    "Subtype" => Object::name("Widget"),
                    "Rect" => rect(),
                    "AS" => Object::name(state),
                })
            })
            .collect();
        let mut dict = dict! {
            "T" => Object::text(name),
            "FT" => Object::name("Btn"),
            "Kids" => refs(&kids),
        };
        if let Some(opt) = opt {
            dict.set("Opt", opt);
        }
        store.set(id, dict);
        fields.push(id);
    }
    let catalog = catalog_of(refs(&fields));
    let form = Form::read(&store.objects, &catalog, Rules::Iso, &Limits::DEFAULT)
        .unwrap()
        .unwrap();
    let states = |name: &str| {
        let read = field(&form, name)
            .group(&store.objects, Rules::Iso, &Limits::DEFAULT)
            .unwrap();
        read.states().to_vec()
    };
    assert_eq!(states("plain"), [true, false, false]);
    assert_eq!(states("indexed"), [false, true, false]);
}

#[test]
fn debug_prints_no_option_or_state() {
    let option = option("secret export", Some("secret label"));
    let states = WidgetStates {
        on_state: Some(Box::from(&b"secret-on"[..])),
        state: Some(Box::from(&b"secret-on"[..])),
        has_off: false,
    };
    let shown = format!("{option:?} {states:?}");
    assert!(!shown.contains("secret"), "{shown}");
    assert!(shown.contains("is_on: true"), "{shown}");
}

#[test]
fn the_button_and_choice_seeds_reach_what_they_hold() {
    let seeds = fuzz::BUTTON_SEEDS;
    for seed in seeds {
        fuzz::check(seed);
    }
    let parsed = fuzz::parsed(seeds[0]);
    let objects = &parsed.0;
    let form = Form::read(objects, &parsed.1, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap()
        .unwrap();
    let kinds: Vec<_> = form.terminals().map(|(_, f)| f.kind().unwrap()).collect();
    assert_eq!(kinds.len(), 4, "{kinds:?}");
    let unison = field(&form, "unison")
        .group(objects, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap();
    assert_eq!(unison.states(), [true, false, false]);
    assert!(unison.group().in_unison);
    let fruit = field(&form, "fruit")
        .group(objects, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap();
    assert_eq!(fruit.group().export_value(2).as_deref(), Some("apple"));
    assert_eq!(fruit.group().on_state(1).as_deref(), Some(&b"a"[..]));

    let parsed = fuzz::parsed(seeds[1]);
    let objects = &parsed.0;
    let form = Form::read(objects, &parsed.1, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap()
        .unwrap();
    let combo = field(&form, "combo");
    let mut sink: Vec<(Place, Fault)> = Vec::new();
    let options = combo
        .options_with(objects, Rules::Pdfium, &Limits::DEFAULT, &mut sink)
        .unwrap();
    assert_eq!(options.len(), 3);
    assert_eq!(sink.len(), 1);
    assert_eq!(
        combo
            .selection(objects, Rules::Pdfium, &Limits::DEFAULT)
            .unwrap(),
        [1]
    );
    let multi = field(&form, "lists.multi");
    assert_eq!(multi.top_index(), 2);
    assert_eq!(
        multi
            .selection(objects, Rules::Pdfium, &Limits::DEFAULT)
            .unwrap(),
        [0, 2]
    );
    let single = field(&form, "lists.single");
    assert_eq!(
        single
            .selection(objects, Rules::Pdfium, &Limits::DEFAULT)
            .unwrap(),
        [1]
    );

    // The group cut from plain bytes, at each of its seeds.
    for seed in fuzz::GROUP_SEEDS {
        fuzz::check_group(seed);
    }
}
