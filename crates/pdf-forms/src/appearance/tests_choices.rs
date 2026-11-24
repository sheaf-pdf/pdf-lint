//! Unit, refusal, and hygiene tests for a combo box's and a list box's
//! appearances, from plain data and from the read layer.

use std::collections::BTreeMap;

use pdf_content_streams::Color;
use pdf_fonts::Std14;
use pdf_gfx::{BoxBorder, BoxBorderStyle, DefaultAppearance, UnencodableChar};
use pdf_objects::fault::Place;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Streams, Unreadable, Why, WithStreams, dict};

use super::{
    Appearance, ComboBox, FieldChoice, FieldFont, ListBox, Unmade, WidgetChoice, WidgetLook,
    choice_for_widget, combo_box, field_choice, list_box, widget_choice,
};
use crate::{Cap, Fault, Form, Limits};

type Objects = BTreeMap<ObjectId, Object>;

fn id(n: u32) -> ObjectId {
    ObjectId::new(n, 0)
}

/// `objects`, whose fonts hold no stream.
fn reading(objects: &Objects) -> impl Resolve<'_> + Streams + Copy + '_ {
    WithStreams::new(objects, |id: ObjectId, _: usize| {
        Err(Unreadable::new(id, Why::Damaged, "no stream"))
    })
}

fn da(text: &str) -> DefaultAppearance {
    DefaultAppearance::parse(text.as_bytes(), &Limits::DEFAULT.annots.content).unwrap()
}

fn helvetica() -> FieldFont {
    FieldFont::standard(Std14::Helvetica)
}

fn text(content: &[u8]) -> &str {
    std::str::from_utf8(content).unwrap()
}

/// A combo box's appearance in Helvetica under `limits`.
fn combo_in(
    combo: &ComboBox<'_>,
    look: &WidgetLook<'_>,
    da: &DefaultAppearance,
    limits: &Limits,
) -> Result<Appearance, Unmade> {
    let font = helvetica();
    combo_box(combo, look, da, font.res().unwrap(), &font.layout(), limits)
}

/// A list box's appearance in Helvetica under `limits`.
fn list_in(
    list: &ListBox<'_>,
    look: &WidgetLook<'_>,
    da: &DefaultAppearance,
    limits: &Limits,
) -> Result<Appearance, Unmade> {
    let font = helvetica();
    list_box(list, look, da, font.res().unwrap(), &font.layout(), limits)
}

/// Each string's origin in `content`, from its `Tm`, and the string.
fn shown(content: &[u8]) -> Vec<(f32, f32, String)> {
    let mut origin = (f32::NAN, f32::NAN);
    let mut shown = Vec::new();
    for line in text(content).lines() {
        if let Some(rest) = line.strip_suffix(" Tm") {
            let numbers: Vec<f32> = rest.split(' ').map(|n| n.parse().unwrap()).collect();
            origin = (numbers[4], numbers[5]);
        } else if let Some(rest) = line.strip_suffix(") Tj") {
            shown.push((origin.0, origin.1, rest[1..].to_string()));
        }
    }
    shown
}

/// Each rectangle filled in `color` in `content`, `[x y w h]`.
fn filled(content: &[u8], color: &str) -> Vec<[f32; 4]> {
    let lines: Vec<&str> = text(content).lines().collect();
    (lines.windows(3))
        .filter(|w| w[0] == color && w[1].ends_with(" re") && w[2] == "f")
        .map(|w| {
            let n: Vec<f32> = w[1]
                .split(' ')
                .take(4)
                .map(|n| n.parse().unwrap())
                .collect();
            [n[0], n[1], n[2], n[3]]
        })
        .collect()
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.001
}

const BLUE: &str = "0 0.2 0.4431 rg";
const BUTTON: &str = "0.8627 0.8627 0.8627 rg";

#[test]
fn a_combo_shows_its_text_in_the_box_beside_its_drop_button() {
    let mut look = WidgetLook::new(100.0, 20.0);
    look.border_color = Some(Color::Gray(0.0));
    let ten = da("/Helv 10 Tf 0 0 1 rg");
    let made = combo_in(&ComboBox::new("Label two"), &look, &ten, &Limits::DEFAULT).unwrap();
    let content = text(&made.content);
    // The text box is the box less its border and the button's 13 points:
    // the text at its left edge, at its middle.
    assert!(content.contains("/Tx BMC\nq\n1 1 85 18 re\nW\nn\n0 0 1 rg\nBT\n"));
    let [(x, y, label)] = shown(&made.content).try_into().unwrap();
    assert!(near(x, 1.0) && near(y, 6.395) && label == "Label two");
    assert_eq!((made.font_size, made.overflowed), (10.0, false));
    // The drop button after `/Tx`: its fill, bands, ring, and triangle.
    let after = &content[content.find("EMC\n").unwrap()..];
    assert_eq!(filled(after.as_bytes(), BUTTON), [[86.0, 1.0, 13.0, 18.0]]);
    assert!(after.contains("q\n1 g\n87 2 m\n"));
    assert!(after.contains("0.5 g\n87 2 m\n98 2 l\n"));
    assert!(after.contains("86.5 1.5 12 17 re\nS\n"));
    assert!(after.contains("q\n0 g\n89.5 11.5 m\n95.5 11.5 l\n92.5 8.5 l\nh\nf\nQ\n"));

    // At size 0, a single-line text field's automatic size.
    let auto = combo_in(
        &ComboBox::new("Label two"),
        &look,
        &da("/Helv 0 Tf 0 g"),
        &Limits::DEFAULT,
    )
    .unwrap();
    let [(_, y, _)] = shown(&auto.content).try_into().unwrap();
    assert_eq!(auto.font_size, 14.0);
    assert!(near(y, 4.953));

    // No drop button: the text has the whole box, and nothing follows it.
    let mut whole = ComboBox::new("Label two");
    whole.drop_button = false;
    let made = combo_in(&whole, &look, &ten, &Limits::DEFAULT).unwrap();
    let content = text(&made.content);
    assert!(content.contains("/Tx BMC\nq\n1 1 98 18 re\nW\nn\n"));
    assert!(content.ends_with("EMC\n"));
    assert!(filled(&made.content, BUTTON).is_empty());

    // A text past its box overflows it, still clipped to it.
    let long = combo_in(
        &ComboBox::new("A long label that passes the box"),
        &look,
        &ten,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert!(long.overflowed);
    assert!(text(&long.content).contains("1 1 85 18 re\nW\nn\n"));

    // No text: an empty `/Tx`, the button after it, and no font.
    let empty = combo_in(&ComboBox::new(""), &look, &ten, &Limits::DEFAULT).unwrap();
    assert!(text(&empty.content).contains("Q\n/Tx BMC\nEMC\nq\n"));
    assert!(empty.needs.0.is_empty());
    assert_eq!((empty.font_size, empty.overflowed), (0.0, false));
}

/// A box narrower than the drop button puts it over the text box's left
/// edge, the text box turned the right way round; a button 6 points high
/// or less has no triangle.
#[test]
fn a_narrow_or_low_combo_is_drawn_as_pdfium_draws_it() {
    let small = da("/Helv 8 Tf 0 g");
    let narrow = combo_in(
        &ComboBox::new("a"),
        &WidgetLook::new(12.0, 20.0),
        &small,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert!(text(&narrow.content).contains("-2 1 3 18 re\nW\nn\n"));
    assert_eq!(filled(&narrow.content, BUTTON), [[-2.0, 1.0, 13.0, 18.0]]);
    let [(x, _, _)] = shown(&narrow.content).try_into().unwrap();
    assert!(near(x, -2.0));
    assert!(narrow.overflowed);

    let low = combo_in(
        &ComboBox::new("a"),
        &WidgetLook::new(40.0, 7.0),
        &small,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(filled(&low.content, BUTTON), [[26.0, 1.0, 13.0, 5.0]]);
    assert!(
        text(&low.content).ends_with("26.5 1.5 12 4 re\nS\nQ\n"),
        "no triangle"
    );
}

#[test]
fn a_list_highlights_its_selection_from_its_top_and_writes_no_line_below() {
    let labels = ["alpha", "bravo", "charlie", "delta", "echo"];
    let look = WidgetLook::new(80.0, 40.0);
    let ten = da("/Helv 10 Tf 0 0 1 rg");
    let mut list = ListBox::new(&labels);
    // In any order, given twice, and past the labels: bravo and delta.
    list.selected = &[3, 1, 1, 99];
    let made = list_in(&list, &look, &ten, &Limits::DEFAULT).unwrap();
    let content = text(&made.content);
    assert!(content.starts_with("/Tx BMC\nq\n1 1 78 38 re\nW\nn\n"));
    assert!(content.ends_with("Q\nEMC\n"));
    // Four lines reach into the box, from its top a line height apart; the
    // fifth starts below it and isn't written.
    let lines = shown(&made.content);
    let words: Vec<&str> = lines.iter().map(|(_, _, s)| s.as_str()).collect();
    assert_eq!(words, ["alpha", "bravo", "charlie", "delta"]);
    for ((x, y, _), want) in lines.iter().zip([29.55, 17.86, 6.17, -5.52]) {
        assert!(near(*x, 1.0) && near(*y, want), "{x} {y}");
    }
    assert!(made.overflowed);
    assert_eq!(made.font_size, 10.0);
    // Each selected line over its highlight, in white; the others in the
    // default appearance's color.
    let marks = filled(&made.content, BLUE);
    assert_eq!(marks.len(), 2);
    for (mark, top) in marks.iter().zip([27.31, 3.93]) {
        assert!(near(mark[0], 1.0) && near(mark[1], top - 11.69) && near(mark[2], 78.0));
        assert!(near(mark[3], 11.69));
    }
    assert_eq!(content.matches("1 g\nBT\n").count(), 2);
    assert_eq!(content.matches("0 0 1 rg\nBT\n").count(), 2);

    // From the top index: what's above it isn't shown, nor its selection.
    list.top = 2;
    list.selected = &[1];
    let made = list_in(&list, &look, &ten, &Limits::DEFAULT).unwrap();
    let words: Vec<String> = shown(&made.content)
        .into_iter()
        .map(|(_, _, s)| s)
        .collect();
    assert_eq!(words, ["charlie", "delta", "echo"]);
    assert!(filled(&made.content, BLUE).is_empty());
    assert!(!made.overflowed, "three lines fit");

    // At size 0, 12: never automatic.
    let twelve = list_in(
        &ListBox::new(&labels),
        &look,
        &da("/Helv 0 Tf 0 g"),
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(twelve.font_size, 12.0);
    let [(_, first, _), ..] = shown(&twelve.content)[..] else {
        panic!("lines")
    };
    assert!(near(first, 27.66));

    // An empty label is a line with no text, its highlight where selected.
    let gap = ["alpha", "", "charlie"];
    let mut list = ListBox::new(&gap);
    list.selected = &[1];
    let made = list_in(&list, &look, &ten, &Limits::DEFAULT).unwrap();
    let lines = shown(&made.content);
    assert_eq!(lines.len(), 2);
    assert!(near(lines[1].1, 6.17));
    assert_eq!(filled(&made.content, BLUE).len(), 1);

    // A label wider than the box overflows it, though every line fits.
    let wide = ["A label much wider than its box"];
    let made = list_in(&ListBox::new(&wide), &look, &ten, &Limits::DEFAULT).unwrap();
    assert!(made.overflowed);

    // No line: an empty `/Tx`, as for no option or a top past them all.
    let mut past = ListBox::new(&labels);
    past.top = 5;
    for list in [ListBox::new(&[]), past] {
        let made = list_in(&list, &look, &ten, &Limits::DEFAULT).unwrap();
        assert_eq!(text(&made.content), "/Tx BMC\nEMC\n");
        assert_eq!((made.font_size, made.overflowed), (0.0, false));
    }
}

/// A list box's lines from its top past `Limits::items`, or their bytes
/// past `Limits::text`, are refused before anything is laid out: before a
/// label the font lacks or a box with no width is found.
#[test]
fn a_list_past_its_caps_is_refused_before_its_laid_out() {
    let labels = ["\u{2603}", "b", "c", "d", "e"];
    let look = WidgetLook::new(80.0, 40.0);
    let ten = da("/Helv 10 Tf 0 g");
    let mut limits = Limits::DEFAULT;
    limits.items = 3;
    let mut list = ListBox::new(&labels);
    for look in [look, WidgetLook::new(0.0, 0.0)] {
        assert_eq!(
            list_in(&list, &look, &ten, &limits),
            Err(Unmade::Limit(Cap::Items))
        );
    }
    // From the third, three lines: within the cap.
    list.top = 2;
    assert!(list_in(&list, &look, &ten, &limits).is_ok());
    let mut limits = Limits::DEFAULT;
    limits.text = 2;
    assert_eq!(
        list_in(&list, &look, &ten, &limits),
        Err(Unmade::Limit(Cap::Text))
    );
    limits.text = 3;
    assert!(list_in(&list, &look, &ten, &limits).is_ok());
}

#[test]
fn a_character_the_font_lacks_is_refused_with_its_option_and_offset() {
    let look = WidgetLook::new(80.0, 40.0);
    let ten = da("/Helv 10 Tf 0 g");
    assert_eq!(
        combo_in(&ComboBox::new("ab\u{2603}"), &look, &ten, &Limits::DEFAULT),
        Err(Unmade::Unencodable(UnencodableChar {
            at: 2,
            character: '\u{2603}'
        }))
    );
    let labels = ["\u{2603}", "a", "x\u{2603}", "c"];
    let mut list = ListBox::new(&labels);
    list.top = 1;
    let refused = list_in(&list, &look, &ten, &Limits::DEFAULT).unwrap_err();
    assert_eq!(
        refused,
        Unmade::UnencodableOption {
            option: 2,
            character: UnencodableChar {
                at: 1,
                character: '\u{2603}'
            }
        }
    );
    assert_eq!(
        refused.to_string(),
        "the font has no code for '\u{2603}', at byte 1 of option 2"
    );
    // A label below the box isn't written, so isn't refused.
    let low = ["a", "b", "c", "d", "\u{2603}"];
    assert!(list_in(&ListBox::new(&low), &look, &ten, &Limits::DEFAULT).is_ok());
}

#[test]
fn choices_refuse_no_box_numbers_not_finite_and_text_past_its_cap() {
    let ten = da("/Helv 10 Tf 0 g");
    let labels = ["a"];
    let list = ListBox::new(&labels);
    let combo = ComboBox::new("a");
    let no_box = WidgetLook::new(0.0, 20.0);
    assert_eq!(
        combo_in(&combo, &no_box, &ten, &Limits::DEFAULT),
        Err(Unmade::NoBox)
    );
    assert_eq!(
        list_in(&list, &no_box, &ten, &Limits::DEFAULT),
        Err(Unmade::NoBox)
    );
    let look = WidgetLook::new(80.0, 40.0);
    let mut huge = ten.clone();
    huge.font_size = Some(1e36);
    let mut nan = ten.clone();
    nan.color = Some(Color::Gray(f32::NAN));
    for da in [&huge, &nan] {
        assert_eq!(
            combo_in(&combo, &look, da, &Limits::DEFAULT),
            Err(Unmade::NotFinite)
        );
        assert_eq!(
            list_in(&list, &look, da, &Limits::DEFAULT),
            Err(Unmade::NotFinite)
        );
    }
    let mut border = look;
    border.border = BoxBorder::new(f32::INFINITY, BoxBorderStyle::Beveled);
    assert_eq!(
        combo_in(&combo, &border, &ten, &Limits::DEFAULT),
        Err(Unmade::NotFinite)
    );
    let mut limits = Limits::DEFAULT;
    limits.text = 3;
    assert_eq!(
        combo_in(&ComboBox::new("four"), &look, &ten, &limits),
        Err(Unmade::Limit(Cap::Text))
    );
}

/// A choice field that is its own widget: `entries`, a widget's subtype,
/// and a box of 100 by 20 where they give none.
fn choice(entries: Dict) -> Dict {
    let mut field = dict! {
        "FT" => Object::name("Ch"),
        "Subtype" => Object::name("Widget"),
        "Rect" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(20)],
    };
    for (key, value) in entries.iter() {
        field.set(key.to_vec(), value.clone());
    }
    field
}

/// A form of `fields`, each an object from 10 on, its default resources
/// `/Helv`, object 2; and `more` objects.
fn form_of(fields: Vec<Dict>, more: Vec<(u32, Object)>) -> (Objects, Dict) {
    let mut objects = Objects::new();
    objects.insert(id(2), Object::from(Std14::Helvetica.dict()));
    let mut refs = Vec::new();
    for (i, field) in fields.into_iter().enumerate() {
        let n = 10 + u32::try_from(i).unwrap();
        objects.insert(id(n), Object::from(field));
        refs.push(Object::from(id(n)));
    }
    for (n, object) in more {
        objects.insert(id(n), object);
    }
    let catalog = dict! {
        "AcroForm" => dict! {
            "Fields" => refs,
            "DR" => dict! { "Font" => dict! { "Helv" => id(2) } },
        },
    };
    (objects, catalog)
}

fn pairs() -> Object {
    Object::from(vec![
        Object::from(vec![Object::text("e1"), Object::text("Label one")]),
        Object::from(vec![Object::text("e2"), Object::text("Label two")]),
        Object::text("plain"),
    ])
}

/// A combo box shows the label of the option its value selects (`/I`'s
/// where it agrees, else the first of that export value, an array value's
/// first item's), else its value as it is; a list box its options' labels,
/// its selection, and its top, each inherited where the field has none.
#[test]
fn a_choice_widget_is_read_for_its_text_or_its_options() {
    let combo = |name: &str, more: Dict| {
        let mut entries = dict! {
            "T" => Object::text(name),
            "Ff" => 1 << 17,
            "DA" => Object::text("/Helv 10 Tf 0 g"),
            "Opt" => pairs(),
        };
        for (key, value) in more.iter() {
            entries.set(key.to_vec(), value.clone());
        }
        choice(entries)
    };
    let fields = vec![
        combo("label", dict! { "V" => Object::text("e2") }),
        combo(
            "typed",
            dict! { "Ff" => (1 << 17) | (1 << 18), "V" => Object::text("typed") },
        ),
        combo(
            "against",
            dict! { "V" => Object::text("e2"), "I" => vec![Object::from(0)] },
        ),
        combo(
            "with",
            dict! { "V" => Object::text("e1"), "I" => vec![Object::from(0)] },
        ),
        combo(
            "array",
            dict! { "V" => vec![Object::text("e2"), Object::text("e1")] },
        ),
        combo("none", Dict::new()),
        combo("plain", dict! { "V" => Object::text("plain") }),
        dict! {
            "T" => Object::text("lists"),
            "FT" => Object::name("Ch"),
            "Opt" => pairs(),
            "TI" => 1,
            "Kids" => vec![Object::from(id(30))],
        },
    ];
    let list = choice(dict! {
        "T" => Object::text("multi"),
        "Parent" => id(17),
        "Ff" => 1 << 21,
        "DA" => Object::text("/Helv 10 Tf 0 g"),
        "V" => vec![Object::text("plain"), Object::text("e1")],
    });
    let (objects, catalog) = form_of(fields, vec![(30, Object::from(list))]);
    let source = reading(&objects);
    let limits = Limits::DEFAULT;
    for rules in [Rules::Iso, Rules::Pdfium] {
        let form = Form::read(source, &catalog, rules, &limits)
            .unwrap()
            .unwrap();
        let read = |name: &str| {
            let field = &form.fields()[form.find(name).unwrap()];
            field_choice(field, source, rules, &limits, &mut ()).unwrap()
        };
        for (name, shown) in [
            ("label", "Label two"),
            ("typed", "typed"),
            ("against", "Label two"),
            ("with", "Label one"),
            ("array", "Label two"),
            ("none", ""),
            ("plain", "plain"),
        ] {
            let read = read(name);
            assert!(read.is_combo() && read.labels().is_empty(), "{name}");
            assert_eq!(read.combo().unwrap().text, shown, "{name}");
            assert!(read.combo().unwrap().drop_button);
        }
        let multi = read("lists.multi");
        assert!(!multi.is_combo() && multi.combo().is_none());
        assert_eq!(multi.labels(), ["Label one", "Label two", "plain"]);
        assert_eq!((multi.selected(), multi.top()), (&[0, 2][..], 1));
        let list = multi.list().unwrap();
        assert_eq!(list.labels, multi.labels());
        assert_eq!((list.selected, list.top), (&[0, 2][..], 1));
        let field = &form.fields()[form.find("lists.multi").unwrap()];
        let widget = &field.widgets()[0];
        let of_widget = widget_choice(field, widget, &multi, source, rules, &limits, &mut ());
        let of_widget = of_widget.unwrap();
        assert!(std::ptr::eq(of_widget.choice(), &multi));
        assert_eq!(of_widget.font_name(), Some(&b"Helv"[..]));
        // The labels are lent to the maker as they're held.
        let font = helvetica();
        let res = font.res().unwrap();
        assert_eq!(
            of_widget.make(res.clone(), &font.layout(), &limits),
            list_box(
                &list,
                &of_widget.look(),
                of_widget.default_appearance(),
                res,
                &font.layout(),
                &limits
            )
        );
    }
}

/// With no value nothing is selected, whatever `/I` has: a list box
/// highlights no line and a combo box shows no text, the default value
/// (`/DV`) aside too.
#[test]
fn no_value_selects_nothing_whatever_the_indices() {
    let abc = || {
        vec![
            Object::text("alpha"),
            Object::text("bravo"),
            Object::text("charlie"),
        ]
    };
    let list = |name: &str, more: Dict| {
        let mut entries = dict! {
            "T" => Object::text(name),
            "DA" => Object::text("/Helv 10 Tf 0 g"),
            "Opt" => abc(),
            "Rect" => vec![Object::from(0), Object::from(0), Object::from(80), Object::from(40)],
        };
        for (key, value) in more.iter() {
            entries.set(key.to_vec(), value.clone());
        }
        choice(entries)
    };
    let fields = vec![
        list("one", dict! { "I" => vec![Object::from(1)] }),
        list(
            "two",
            dict! { "Ff" => 1 << 21, "I" => vec![Object::from(0), Object::from(2)] },
        ),
        list("bare", dict! { "I" => 1 }),
        list(
            "name",
            dict! { "V" => Object::name("bravo"), "I" => vec![Object::from(1)] },
        ),
        list(
            "empty",
            dict! { "V" => Vec::<Object>::new(), "I" => vec![Object::from(1)] },
        ),
        list(
            "combo",
            dict! { "Ff" => 1 << 17, "I" => vec![Object::from(1)], "DV" => Object::text("bravo") },
        ),
    ];
    let (objects, catalog) = form_of(fields, Vec::new());
    let source = reading(&objects);
    let limits = Limits::DEFAULT;
    for rules in [Rules::Iso, Rules::Pdfium] {
        let form = Form::read(source, &catalog, rules, &limits)
            .unwrap()
            .unwrap();
        for field in form.fields() {
            let name = field.partial_name().unwrap();
            assert_eq!(field.selection(source, rules, &limits), Ok(Vec::new()));
            let read = field_choice(field, source, rules, &limits, &mut ()).unwrap();
            assert!(read.selected().is_empty(), "{name}");
            let widget = &field.widgets()[0];
            let made = choice_for_widget(&form, field, widget, source, rules, &limits, &mut ());
            let content = made.unwrap().appearance.content;
            let content = String::from_utf8(content).unwrap();
            assert!(
                !content.contains("0 0.2 0.4431 rg\n"),
                "{name}: a highlight"
            );
            match read.combo() {
                Some(combo) => assert!(combo.text.is_empty() && content.contains("/Tx BMC\nEMC\n")),
                None => assert_eq!(shown(content.as_bytes()).len(), 3, "{name}"),
            }
        }
    }
}

/// A field's options are decoded once for all its widgets: what's wrong
/// in `/Opt` and `/V` is told by `field_choice`, once, and reading each
/// widget tells only what's the widget's.
#[test]
fn a_fields_options_are_read_once_for_its_widgets() {
    let kids: Vec<Object> = (30..34).map(|n| Object::from(id(n))).collect();
    let fields = vec![dict! {
        "T" => Object::text("list"),
        "FT" => Object::name("Ch"),
        "DA" => Object::text("/Helv 10 Tf 0 g"),
        "Opt" => vec![Object::text("alpha"), Object::from(7), Object::text("charlie")],
        "V" => Object::name("alpha"),
        "Kids" => kids,
    }];
    let widgets = (30..34)
        .map(|n| {
            let widget = dict! {
                "Parent" => id(10),
                "Subtype" => Object::name("Widget"),
                "Rect" => vec![Object::from(0), Object::from(0), Object::from(80), Object::from(40)],
            };
            (n, Object::from(widget))
        })
        .collect();
    let (objects, catalog) = form_of(fields, widgets);
    let source = reading(&objects);
    let limits = Limits::DEFAULT;
    let form = Form::read(source, &catalog, Rules::Iso, &limits)
        .unwrap()
        .unwrap();
    let field = &form.fields()[0];
    assert_eq!(field.widgets().len(), 4);

    let mut told: Vec<(Place, Fault)> = Vec::new();
    let read = field_choice(field, source, Rules::Iso, &limits, &mut told).unwrap();
    let of = |key| {
        let wrong = Fault::WrongType { key };
        told.iter().filter(|(_, fault)| *fault == wrong).count()
    };
    assert_eq!((of("Opt"), of("V"), told.len()), (1, 1, 2));
    assert_eq!(read.labels(), ["alpha", "", "charlie"]);

    let mut of_widgets: Vec<(Place, Fault)> = Vec::new();
    for widget in field.widgets() {
        let one = widget_choice(
            field,
            widget,
            &read,
            source,
            Rules::Iso,
            &limits,
            &mut of_widgets,
        )
        .unwrap();
        assert!(std::ptr::eq(one.choice(), &read));
    }
    assert!(of_widgets.is_empty(), "{of_widgets:?}");
}

/// A field of another kind is `NotChoice`; a default appearance naming no
/// font is told, and a value that is no text; `choice_for_widget` makes
/// what `widget_choice` reads made in its font, a fallback where the
/// default resources lack it.
#[test]
fn a_choice_is_made_from_its_widget_in_its_font() {
    let fields = vec![
        choice(dict! {
            "T" => Object::text("combo"),
            "Ff" => 1 << 17,
            "DA" => Object::text("/Helv 0 Tf 0 g"),
            "Opt" => pairs(),
            "V" => Object::text("e2"),
            "MK" => dict! { "BC" => vec![Object::from(0)] },
        }),
        choice(dict! {
            "T" => Object::text("list"),
            "DA" => Object::text("/Cour 10 Tf 0 g"),
            "Opt" => pairs(),
            "V" => Object::text("e1"),
            "Rect" => vec![Object::from(0), Object::from(0), Object::from(80), Object::from(40)],
        }),
        choice(dict! {
            "T" => Object::text("unnamed"),
            "DA" => Object::text("0 g"),
            "Opt" => pairs(),
            "V" => Object::name("e1"),
        }),
        dict! {
            "T" => Object::text("text"),
            "FT" => Object::name("Tx"),
            "Subtype" => Object::name("Widget"),
            "Rect" => vec![Object::from(0), Object::from(0), Object::from(9), Object::from(9)],
        },
        dict! {
            "T" => Object::text("button"),
            "FT" => Object::name("Btn"),
            "Subtype" => Object::name("Widget"),
            "Rect" => vec![Object::from(0), Object::from(0), Object::from(9), Object::from(9)],
        },
    ];
    let (objects, catalog) = form_of(fields, Vec::new());
    let source = reading(&objects);
    let limits = Limits::DEFAULT;
    let form = Form::read(source, &catalog, Rules::Iso, &limits)
        .unwrap()
        .unwrap();
    let [combo, list, unnamed, text_field, button] = form.fields() else {
        panic!("five fields")
    };
    let of_combo = field_choice(combo, source, Rules::Iso, &limits, &mut ()).unwrap();
    for field in [text_field, button] {
        let widget = &field.widgets()[0];
        let read = field_choice(field, source, Rules::Iso, &limits, &mut ());
        assert_eq!(read.unwrap_err(), Unmade::NotChoice);
        let read = widget_choice(
            field,
            widget,
            &of_combo,
            source,
            Rules::Iso,
            &limits,
            &mut (),
        );
        assert_eq!(read.unwrap_err(), Unmade::NotChoice);
        let made = choice_for_widget(&form, field, widget, source, Rules::Iso, &limits, &mut ());
        assert_eq!(made.unwrap_err(), Unmade::NotChoice);
    }

    let made_of = |field| {
        let field: &crate::Field<'_> = field;
        let widget = &field.widgets()[0];
        let mut told: Vec<(Place, Fault)> = Vec::new();
        let read = field_choice(field, source, Rules::Iso, &limits, &mut told).unwrap();
        let font_name = widget_choice(field, widget, &read, source, Rules::Iso, &limits, &mut told)
            .unwrap()
            .font_name()
            .map(<[u8]>::to_vec);
        let mut faults: Vec<(Place, Fault)> = Vec::new();
        let made = choice_for_widget(
            &form,
            field,
            widget,
            source,
            Rules::Iso,
            &limits,
            &mut faults,
        )
        .unwrap();
        (read, font_name, told, made, faults)
    };

    // In the default resources' font.
    let (read, _, told, made, faults) = made_of(combo);
    assert!(told.is_empty() && faults.is_empty());
    assert!(!made.font.is_fallback());
    assert_eq!(made.font_name.as_deref(), Some(&b"Helv"[..]));
    let font = form.field_font(source, b"Helv", &limits, &mut ()).unwrap();
    let widget = &combo.widgets()[0];
    let expected = widget_choice(combo, widget, &read, source, Rules::Iso, &limits, &mut ())
        .unwrap()
        .make(font.res().unwrap(), &font.layout(), &limits)
        .unwrap();
    assert_eq!(made.appearance, expected);
    assert_eq!(made.appearance.font_size, 14.0);

    // A font the default resources lack: the standard 14 family its name
    // gives, told.
    let (read, _, told, made, faults) = made_of(list);
    assert!(told.is_empty());
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].1, Fault::FontNotInResources);
    assert!(made.font.is_fallback());
    assert_eq!(made.font.res(), FieldFont::standard(Std14::Courier).res());
    assert_eq!(read.selected(), [0]);
    assert_eq!(shown(&made.appearance.content).len(), 3);

    // No font named: Helvetica, told; a value that is a name is none, told.
    let (read, font_name, told, made, faults) = made_of(unnamed);
    let at = Place::new(unnamed.id(), None);
    assert!(told.contains(&(at, Fault::MissingDefaultAppearance)));
    assert!(told.contains(&(at, Fault::WrongType { key: "V" })));
    assert_eq!(faults, told);
    assert!(read.selected().is_empty() && font_name.is_none());
    assert_eq!(made.font.res(), helvetica().res());
    assert_eq!(made.font_name, None);
}

/// No `Debug` of what's read or made shows a text, a label, or a font's
/// name.
#[test]
fn debug_prints_no_text_or_label() {
    let labels = ["Zq#7 one", "Zq#7 two"];
    let mut list = ListBox::new(&labels);
    list.selected = &[1];
    let combo = ComboBox::new("Zq#7 combo");
    let shown = format!("{list:?} {combo:?}");
    assert!(!shown.contains("Zq#7"), "{shown}");

    let fields = vec![
        choice(dict! {
            "T" => Object::text("c"),
            "Ff" => 1 << 17,
            "DA" => Object::text("/Helv 0 Tf 0 g"),
            "Opt" => vec![Object::from(vec![Object::text("Wq#9"), Object::text("Zq#7 label")])],
            "V" => Object::text("Wq#9"),
        }),
        choice(dict! {
            "T" => Object::text("l"),
            "DA" => Object::text("/Helv 0 Tf 0 g"),
            "Opt" => vec![Object::text("Zq#7 a"), Object::text("Zq#7 b")],
            "V" => Object::text("Zq#7 b"),
        }),
    ];
    let (objects, catalog) = form_of(fields, Vec::new());
    let source = reading(&objects);
    let limits = Limits::DEFAULT;
    let form = Form::read(source, &catalog, Rules::Iso, &limits)
        .unwrap()
        .unwrap();
    for field in form.fields() {
        let widget = &field.widgets()[0];
        let of_field = field_choice(field, source, Rules::Iso, &limits, &mut ()).unwrap();
        let read = widget_choice(
            field,
            widget,
            &of_field,
            source,
            Rules::Iso,
            &limits,
            &mut (),
        )
        .unwrap();
        let made =
            choice_for_widget(&form, field, widget, source, Rules::Iso, &limits, &mut ()).unwrap();
        let shown = format!("{of_field:?} {read:?} {made:?}");
        for secret in ["Zq#7", "Wq#9", "Helv"] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
    }
}

#[test]
fn choice_types_are_send_and_sync() {
    fn both<T: Send + Sync>() {}
    both::<ComboBox<'_>>();
    both::<ListBox<'_>>();
    both::<ListBox<'_, String>>();
    both::<FieldChoice>();
    both::<WidgetChoice<'_>>();
}
