//! Tests of the `AnnotMut` setters of `/MK`, `/AP /D`, and state
//! dictionaries: what each writes, what it keeps, and what `Annot::from_dict`
//! and `Appearances::read` read back.

use std::collections::BTreeMap;

use pdf_objects::parse::Parser;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Stream, Value, dict};

use crate::{
    Annot, AnnotColor, AnnotMut, AppearanceChars, AppearanceEntry, AppearanceMode, Appearances,
    Limits, TypedAnnot,
};

type Objects = BTreeMap<ObjectId, Object>;

/// A Form XObject at each of `numbers`, each its own stream.
fn forms(numbers: &[u32]) -> (Objects, Vec<ObjectId>) {
    let mut objects = Objects::new();
    let ids = numbers
        .iter()
        .map(|&n| {
            let id = ObjectId::new(n, 0);
            let dict = dict! {
                "Subtype" => Object::name("Form"),
                "BBox" => vec![Object::from(0), Object::from(0), Object::from(10), Object::from(10)],
            };
            objects.insert(id, Object::from(Stream::new(dict, format!("{n} g").into_bytes())));
            id
        })
        .collect();
    (objects, ids)
}

fn widget(entries: Dict) -> Dict {
    let mut dict = dict! {
        "Type" => Object::name("Annot"),
        "Subtype" => Object::name("Widget"),
        "Rect" => vec![Object::from(0), Object::from(0), Object::from(10), Object::from(10)],
    };
    for entry in entries.entries() {
        dict.set(entry.key(), entry.value().clone());
    }
    dict
}

fn states(pairs: &[(&[u8], ObjectId)]) -> AppearanceEntry {
    AppearanceEntry::States(
        pairs
            .iter()
            .map(|&(name, id)| (Box::from(name), id))
            .collect(),
    )
}

fn read(objects: &Objects, dict: &Dict) -> Appearances {
    let alone = Appearances::read(objects, dict, None, Rules::Iso, &Limits::DEFAULT)
        .unwrap()
        .expect("an /AP or an /AS");
    let whole = Annot::from_dict(objects, dict, None, Rules::Iso, &Limits::DEFAULT).unwrap();
    assert_eq!(whole.appearances(), Some(&alone));
    alone
}

fn chars_read(objects: &Objects, dict: &Dict) -> Option<AppearanceChars> {
    let annot = Annot::from_dict(objects, dict, None, Rules::Iso, &Limits::DEFAULT).unwrap();
    let TypedAnnot::Widget(widget) = annot.typed() else {
        panic!("a widget");
    };
    widget.appearance_chars.clone()
}

/// `/MK` is written as given, an entry the value lacks removed, and a
/// direct `/MK`'s other entries (an icon and its fit) kept; what's written
/// reads back as given.
#[test]
fn appearance_chars_keep_a_direct_mk_s_icon() {
    let (objects, ids) = forms(&[5, 6]);
    let mut dict = widget(dict! {
        "MK" => dict! {
            "R" => 90,
            "BC" => vec![Object::from(1), Object::from(0), Object::from(0)],
            "CA" => Object::text("old"),
            "RC" => Object::text("over"),
            "I" => ids[0],
            "IX" => ids[1],
            "IF" => dict! { "SW" => Object::name("A") },
        },
    });
    let chars = AppearanceChars {
        rotation: 0,
        border_color: None,
        background_color: Some(AnnotColor::Gray(0.5)),
        normal_caption: Some("4".into()),
        rollover_caption: None,
        down_caption: Some("\u{2714}".into()),
        text_position: Some(1),
    };
    AnnotMut::new(&mut dict).set_appearance_chars(&chars);

    let mk = dict.get(b"MK").and_then(Object::as_dict).unwrap();
    for gone in [&b"R"[..], b"BC", b"RC"] {
        assert!(mk.get(gone).is_none());
    }
    assert_eq!(mk.get(b"BG"), Some(&AnnotColor::Gray(0.5).to_object()));
    assert_eq!(mk.get(b"CA"), Some(&Object::text("4")));
    assert_eq!(mk.get(b"AC"), Some(&Object::text("\u{2714}")));
    assert_eq!(mk.get(b"TP"), Some(&Object::from(1)));
    assert_eq!(mk.get(b"I"), Some(&Object::from(ids[0])));
    assert_eq!(mk.get(b"IX"), Some(&Object::from(ids[1])));
    assert_eq!(
        mk.get(b"IF"),
        Some(&Object::from(dict! { "SW" => Object::name("A") }))
    );
    assert_eq!(chars_read(&objects, &dict), Some(chars));

    // Every entry, a transparent color among them.
    let full = AppearanceChars {
        rotation: 270,
        border_color: Some(AnnotColor::Transparent),
        background_color: Some(AnnotColor::Cmyk([0.0, 0.25, 0.5, 1.0])),
        normal_caption: Some("Push".into()),
        rollover_caption: Some("Over".into()),
        down_caption: Some("Down".into()),
        text_position: Some(0),
    };
    AnnotMut::new(&mut dict).set_appearance_chars(&full);
    assert_eq!(chars_read(&objects, &dict), Some(full));
    assert!(dict.get(b"MK").and_then(Object::as_dict).unwrap().has(b"I"));

    // Nothing at all: an empty `/MK`, its icon still there.
    AnnotMut::new(&mut dict).set_appearance_chars(&AppearanceChars::default());
    assert_eq!(
        chars_read(&objects, &dict),
        Some(AppearanceChars::default())
    );
    let mk = dict.get(b"MK").and_then(Object::as_dict).unwrap();
    assert_eq!(mk.len(), 3);
}

/// A `/MK` that isn't a direct dictionary is replaced by a new one of what
/// the value has, and one is made where there's none.
#[test]
fn appearance_chars_replace_a_mk_that_isn_t_direct() {
    let (mut objects, _) = forms(&[]);
    let mk_id = ObjectId::new(7, 0);
    let named = dict! { "I" => ObjectId::new(8, 0), "CA" => Object::text("kept there") };
    objects.insert(mk_id, Object::from(named.clone()));
    let chars = AppearanceChars {
        rotation: 90,
        normal_caption: Some("l".into()),
        ..AppearanceChars::default()
    };
    let made = dict! { "R" => 90, "CA" => Object::text("l") };
    for mk in [
        Some(Object::from(mk_id)),
        Some(Object::from(5)),
        Some(Object::from(Stream::new(Dict::new(), Vec::new()))),
        None,
    ] {
        let mut dict = widget(Dict::new());
        if let Some(mk) = mk {
            dict.set("MK", mk);
        }
        AnnotMut::new(&mut dict).set_appearance_chars(&chars);
        assert_eq!(dict.get(b"MK"), Some(&Object::from(made.clone())));
        assert_eq!(chars_read(&objects, &dict), Some(chars.clone()));
    }
    // The dictionary the reference names is as it was.
    assert_eq!(objects.get(&mk_id), Some(&Object::from(named)));
}

/// A state dictionary of exactly the states given, in their order: a state
/// not given isn't written, so a widget made with no `/Off` has none; a
/// name given twice is written once, where it's first given, with its last
/// form. What the slot held goes, and the other slots and `/AS` stay.
#[test]
fn appearance_states_are_exactly_those_given() {
    let (objects, f) = forms(&[1, 2, 3, 4, 5]);

    // No `/AP`: made, of the on state alone.
    let mut dict = widget(Dict::new());
    AnnotMut::new(&mut dict)
        .set_appearance_states(AppearanceMode::Normal, &[(b"1", f[0])])
        .set_appearance_state(b"1");
    let ap = read(&objects, &dict);
    assert_eq!(ap.normal, Some(states(&[(b"1", f[0])])));
    assert_eq!(ap.rollover, None);
    assert_eq!(ap.down, None);
    assert_eq!(ap.stream(AppearanceMode::Normal, true, false), Some(f[0]));
    let n = dict.get(b"AP").and_then(Object::as_dict).unwrap();
    let n = n.get(b"N").and_then(Object::as_dict).unwrap();
    assert!(!n.has(b"Off"));

    // A direct `/AP`: `/N` replaced, in the order given, `/D` and `/AS`
    // kept.
    let mut dict = widget(dict! {
        "AS" => Object::name("Off"),
        "AP" => dict! {
            "N" => dict! { "Yes" => f[0], "Off" => f[1] },
            "D" => f[2],
        },
    });
    AnnotMut::new(&mut dict).set_appearance_states(
        AppearanceMode::Normal,
        &[(b"Off", f[3]), (b"spouse", f[4]), (b"Off", f[1])],
    );
    let ap = read(&objects, &dict);
    assert_eq!(
        ap.normal,
        Some(states(&[(b"Off", f[1]), (b"spouse", f[4])]))
    );
    assert_eq!(ap.down, Some(AppearanceEntry::Single(f[2])));
    assert_eq!(ap.state.as_deref(), Some(&b"Off"[..]));

    // `/R` and `/D` the same, beside `/N`.
    AnnotMut::new(&mut dict)
        .set_appearance_states(AppearanceMode::Down, &[(b"Choice1", f[0]), (b"Off", f[1])])
        .set_appearance_states(AppearanceMode::Rollover, &[(b"On", f[2])]);
    let ap = read(&objects, &dict);
    assert_eq!(
        ap.normal,
        Some(states(&[(b"Off", f[1]), (b"spouse", f[4])]))
    );
    assert_eq!(ap.down, Some(states(&[(b"Choice1", f[0]), (b"Off", f[1])])));
    assert_eq!(ap.rollover, Some(states(&[(b"On", f[2])])));
}

/// One state set beside the others of a direct state dictionary: in its
/// place where the dictionary has it, at the end where it hasn't. A slot
/// that is no direct state dictionary becomes one of that state alone: a
/// stream, by reference or direct, and a reference to a state dictionary,
/// which isn't followed.
#[test]
fn one_state_is_set_beside_the_others() {
    let (mut objects, f) = forms(&[1, 2, 3, 4, 5]);
    let mut dict = widget(dict! {
        "AP" => dict! { "N" => dict! { "Yes" => f[0], "Off" => f[1] } },
    });
    AnnotMut::new(&mut dict)
        .set_state_appearance(AppearanceMode::Normal, b"Yes", f[2])
        .set_state_appearance(AppearanceMode::Normal, b"1", f[3])
        .set_state_appearance(AppearanceMode::Down, b"Off", f[4]);
    let ap = read(&objects, &dict);
    assert_eq!(
        ap.normal,
        Some(states(&[(b"Yes", f[2]), (b"Off", f[1]), (b"1", f[3])]))
    );
    assert_eq!(ap.down, Some(states(&[(b"Off", f[4])])));

    let named_id = ObjectId::new(9, 0);
    let named = dict! { "Yes" => f[0], "Off" => f[1] };
    objects.insert(named_id, Object::from(named.clone()));
    for slot in [
        Object::from(f[0]),
        Object::from(named_id),
        Object::from(Stream::new(Dict::new(), Vec::new())),
    ] {
        let mut dict = widget(dict! { "AP" => dict! { "N" => slot, "R" => f[1] } });
        AnnotMut::new(&mut dict).set_state_appearance(AppearanceMode::Normal, b"Off", f[2]);
        let ap = read(&objects, &dict);
        assert_eq!(ap.normal, Some(states(&[(b"Off", f[2])])));
        assert_eq!(ap.rollover, Some(AppearanceEntry::Single(f[1])));
    }
    assert_eq!(objects.get(&named_id), Some(&Object::from(named)));
}

/// An `/AP` that isn't a direct dictionary is replaced by the setters that
/// bind `/N`, and left as it is, its span with it, by those that bind `/R`
/// and `/D`, as `has_direct_appearance` says beforehand.
#[test]
fn an_indirect_ap_is_replaced_for_n_and_left_for_r_and_d() {
    let (mut objects, f) = forms(&[1, 2, 3]);
    let ap_id = ObjectId::new(9, 0);
    objects.insert(
        ap_id,
        Object::from(dict! { "N" => dict! { "Yes" => f[0], "Off" => f[1] } }),
    );
    let parsed = Parser::at(b"<< /Subtype /Widget /Rect [0 0 10 10] /AP 9 0 R >>", 0)
        .rules(Rules::Iso)
        .value(&mut ())
        .unwrap();
    let Value::Dict(indirect) = parsed.into_value() else {
        panic!("a dictionary");
    };
    let before = read(&objects, &indirect);
    assert!(indirect.get(b"AP").and_then(Object::at).is_some());
    assert!(!AnnotMut::new(&mut indirect.clone()).has_direct_appearance());

    let sets: [&dyn Fn(&mut AnnotMut<'_>); 5] = [
        &|m| {
            m.set_down_appearance(f[2]);
        },
        &|m| {
            m.set_appearance_states(AppearanceMode::Down, &[(b"Yes", f[2])]);
        },
        &|m| {
            m.set_appearance_states(AppearanceMode::Rollover, &[(b"Yes", f[2])]);
        },
        &|m| {
            m.set_state_appearance(AppearanceMode::Down, b"Yes", f[2]);
        },
        &|m| {
            m.set_state_appearance(AppearanceMode::Rollover, b"Yes", f[2]);
        },
    ];
    for set in sets {
        let mut dict = indirect.clone();
        set(&mut AnnotMut::new(&mut dict));
        assert_eq!(dict, indirect);
        assert_eq!(
            dict.get(b"AP").and_then(Object::at),
            indirect.get(b"AP").and_then(Object::at)
        );
        assert_eq!(read(&objects, &dict), before);
    }

    let mut dict = indirect.clone();
    AnnotMut::new(&mut dict).set_appearance_states(AppearanceMode::Normal, &[(b"Yes", f[2])]);
    assert!(AnnotMut::new(&mut dict).has_direct_appearance());
    assert_eq!(
        read(&objects, &dict).normal,
        Some(states(&[(b"Yes", f[2])]))
    );
    let mut dict = indirect.clone();
    AnnotMut::new(&mut dict).set_state_appearance(AppearanceMode::Normal, b"Off", f[2]);
    let ap = read(&objects, &dict);
    assert_eq!(ap.normal, Some(states(&[(b"Off", f[2])])));
    assert_eq!(ap.rollover, None);

    // No `/AP`: `/D` made alone.
    let mut dict = widget(Dict::new());
    AnnotMut::new(&mut dict).set_down_appearance(f[2]);
    let ap = read(&objects, &dict);
    assert_eq!(ap.down, Some(AppearanceEntry::Single(f[2])));
    assert_eq!(ap.normal, None);
    // A direct `/AP`: `/D` beside `/N`.
    let mut dict = widget(dict! { "AP" => dict! { "N" => f[0] } });
    AnnotMut::new(&mut dict).set_down_appearance(f[2]);
    let ap = read(&objects, &dict);
    assert_eq!(ap.normal, Some(AppearanceEntry::Single(f[0])));
    assert_eq!(ap.down, Some(AppearanceEntry::Single(f[2])));
}

/// `Debug` of the setters' wrapper, of what's read back, and of the value
/// set prints no caption.
#[test]
fn debug_prints_no_caption() {
    let (objects, _) = forms(&[]);
    let mut dict = widget(Dict::new());
    let chars = AppearanceChars {
        normal_caption: Some("SECRET_CAPTION".into()),
        rollover_caption: Some("SECRET_ROLLOVER".into()),
        down_caption: Some("SECRET_DOWN".into()),
        ..AppearanceChars::default()
    };
    let mut m = AnnotMut::new(&mut dict);
    m.set_appearance_chars(&chars);
    let printed = format!("{m:?} {chars:?}");
    let annot = Annot::from_dict(&objects, &dict, None, Rules::Iso, &Limits::DEFAULT).unwrap();
    let printed = format!("{printed} {annot:?}");
    for secret in ["SECRET", "CAPTION", "ROLLOVER", "DOWN"] {
        assert!(!printed.contains(secret), "{printed}");
    }
}
