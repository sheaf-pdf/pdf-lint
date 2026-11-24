//! Unit, refusal, and hygiene tests for a check box's, a radio button's, and
//! a push button's appearances, from plain data and from the read layer.

use std::collections::BTreeMap;

use pdf_annots::{AnnotMut, AppearanceMode};
use pdf_content_streams::{Color, ResBody};
use pdf_fonts::Std14;
use pdf_gfx::{BoxBorder, BoxBorderStyle, DefaultAppearance, UnencodableChar};
use pdf_objects::fault::Place;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{
    Data, Dict, Object, ObjectId, Resolve, Stream, Streams, Unreadable, Value, Why, WithStreams,
    dict,
};

use super::{
    Appearance, FieldFont, MadePush, MadeToggle, Mark, PushAppearances, PushButton,
    ToggleAppearances, ToggleButton, Unmade, WidgetLook, WidgetPush, WidgetToggle, check_box,
    push_button, push_for_widget, radio_button, toggle_for_widget, widget_push, widget_text,
    widget_toggle,
};
use crate::{Cap, Fault, Form, Limits};

type Objects = BTreeMap<ObjectId, Object>;

fn id(n: u32) -> ObjectId {
    ObjectId::new(n, 0)
}

/// `objects` with each stream's data, held as bytes, decoded as it is.
fn reading(objects: &Objects) -> impl Resolve<'_> + Streams + Copy + '_ {
    WithStreams::new(objects, move |id: ObjectId, _: usize| {
        match objects.get(&id).map(Object::value) {
            Some(Value::Stream(stream)) => match &stream.data {
                Data::Bytes(bytes) => Ok(bytes.clone()),
                _ => Err(Unreadable::new(id, Why::Detached, "not held")),
            },
            _ => Err(Unreadable::new(id, Why::Damaged, "no stream")),
        }
    })
}

fn da(text: &str) -> DefaultAppearance {
    DefaultAppearance::parse(text.as_bytes(), &Limits::DEFAULT.annots.content).unwrap()
}

fn zapf() -> FieldFont {
    FieldFont::standard(Std14::ZapfDingbats)
}

/// A check box's or (`radio`) radio button's appearances with `caption`,
/// in `look`, at `/ZaDb 0 Tf 0 g`.
fn toggled(radio: bool, caption: Option<&str>, look: &WidgetLook<'_>) -> ToggleAppearances {
    let mut button = ToggleButton::new();
    button.caption = caption;
    let made = match radio {
        true => radio_button,
        false => check_box,
    };
    made(
        &button,
        look,
        &da("/ZaDb 0 Tf 0 g"),
        zapf().res().unwrap(),
        &Limits::DEFAULT,
    )
    .unwrap()
}

fn text(content: &[u8]) -> &str {
    std::str::from_utf8(content).unwrap()
}

/// The numbers of the last operation `op` in `content`.
fn last_operands(content: &[u8], op: &str) -> Vec<f32> {
    let text = text(content);
    let line = text
        .lines()
        .rev()
        .find(|line| line.ends_with(&format!(" {op}")))
        .unwrap_or_else(|| panic!("no {op} in {text}"));
    line.split_whitespace()
        .filter_map(|word| word.parse().ok())
        .collect()
}

#[test]
fn a_mark_is_named_by_its_captions_first_character() {
    let from = |caption: Option<&str>| Mark::from_caption(caption, Mark::Check);
    assert_eq!(from(None), Mark::Check);
    for (caption, mark) in [
        ("4", Mark::Check),
        ("8", Mark::Cross),
        ("H", Mark::Star),
        ("l", Mark::Circle),
        ("n", Mark::Square),
        ("u", Mark::Diamond),
        ("6", Mark::Other(b'6')),
        ("48", Mark::Check),
        ("l4", Mark::Circle),
        ("~", Mark::Other(b'~')),
    ] {
        assert_eq!(from(Some(caption)), mark, "{caption}");
        assert_eq!(mark.code(), caption.as_bytes()[0]);
    }
    // No glyph, a glyph with no box, or no code a caption string holds: the
    // default, as PDFium draws its default.
    for caption in ["", " ", "\u{7f}", "é", "\u{2714}", "\u{1}"] {
        assert_eq!(from(Some(caption)), Mark::Check, "{caption:?}");
        assert_eq!(
            Mark::from_caption(Some(caption), Mark::Circle),
            Mark::Circle,
            "{caption:?}"
        );
    }
}

/// Each caption draws its own code in ZapfDingbats, in a check box and a
/// radio button; one ZapfDingbats has no glyph for draws the default.
#[test]
fn each_caption_draws_its_own_code() {
    let look = WidgetLook::new(20.0, 20.0);
    for radio in [false, true] {
        for caption in ["4", "8", "6", "l", "H", "n", "u"] {
            let made = toggled(radio, Some(caption), &look);
            let on = text(&made.on.content);
            assert!(on.ends_with(&format!("({caption}) Tj\nET\nQ\n")), "{on}");
            let mark = &on[on.rfind("q\n0 g").unwrap()..];
            assert!(text(&made.down_on.content).ends_with(mark));
            // The mark's font is its one resource, the font given.
            assert_eq!(made.on.needs.0.len(), 1);
            assert_eq!(made.on.needs.0[0].res, zapf().res().unwrap());
        }
        let default = if radio { "(l) Tj" } else { "(4) Tj" };
        for caption in [None, Some(" "), Some("é"), Some("\u{2714}")] {
            let made = toggled(radio, caption, &look);
            assert!(text(&made.on.content).contains(default), "{caption:?}");
        }
    }
}

/// The mark's glyph box fits its share of the centre square of the box less
/// the border, centred on it across by its advance and up by its box; or
/// it's at `/DA`'s size.
#[test]
fn a_mark_is_sized_and_centred_by_its_glyph() {
    // A box 30 by 20 with a solid border of 1: the client 1 to 29 by 1 to
    // 19, its centre square 18 a side about (15, 10).
    let look = WidgetLook::new(30.0, 20.0);
    for (radio, caption, share, advance, [y0, y1], wide) in [
        (false, "4", 0.49, 846.0, [-14.0, 705.0], 775.0),
        (false, "8", 1.0, 677.0, [-13.0, 705.0], 606.0),
        (false, "H", 0.70, 816.0, [-14.0, 705.0], 747.0),
        (false, "l", 2.0 / 3.0, 791.0, [-14.0, 705.0], 722.0),
        (true, "l", 0.5, 791.0, [-14.0, 705.0], 722.0),
        (true, "n", 2.0 / 3.0, 761.0, [0.0, 692.0], 691.0),
    ] {
        let made = toggled(radio, Some(caption), &look);
        let size: f32 = 18.0 * share * 1000.0 / f32::max(wide, y1 - y0);
        let tf = last_operands(&made.on.content, "Tf");
        assert!((tf[0] - size).abs() < 1e-3, "{caption}: {tf:?} {size}");
        assert!((made.on.font_size - size).abs() < 1e-3);
        let tm = last_operands(&made.on.content, "Tm");
        let x = 15.0 - size * advance / 2000.0;
        let y = 10.0 - size * (y0 + y1) / 2000.0;
        assert!(
            (tm[4] - x).abs() < 1e-3 && (tm[5] - y).abs() < 1e-3,
            "{caption}: {tm:?}"
        );
        assert!(!made.on.overflowed);
        assert_eq!(made.down_off.as_ref().unwrap().font_size, 0.0);
    }

    // At a size `/DA` gives, centred the same way; past the box it says so.
    for (size, overflowed) in [(8.0, false), (40.0, true)] {
        let made = check_box(
            &ToggleButton::new(),
            &look,
            &da(&format!("/ZaDb {size} Tf 0 g")),
            zapf().res().unwrap(),
            &Limits::DEFAULT,
        )
        .unwrap();
        assert_eq!(last_operands(&made.on.content, "Tf")[0], size);
        let tm = last_operands(&made.on.content, "Tm");
        assert!((tm[4] - (15.0 - size * 846.0 / 2000.0)).abs() < 1e-3);
        assert_eq!(made.on.overflowed, overflowed);
    }
    // A border as wide as half the box leaves no square: no mark.
    let mut thick = WidgetLook::new(10.0, 10.0);
    thick.border = BoxBorder::solid(5.0);
    let made = toggled(false, None, &thick);
    assert!(!text(&made.on.content).contains("Tj"));
    assert!(made.on.needs.0.is_empty());
}

/// An `Off` that would paint nothing isn't made; one with a background, a
/// border, or bands is the frame alone, and the on state the frame and the
/// mark.
#[test]
fn an_off_that_paints_nothing_isnt_made() {
    let mut none = WidgetLook::new(12.0, 12.0);
    none.border = BoxBorder::solid(0.0);
    for radio in [false, true] {
        let made = toggled(radio, None, &none);
        assert_eq!(made.off, None, "{radio}");
        // The down background is drawn with none of its own.
        let down = made.down_off.unwrap();
        assert!(text(&down.content).starts_with("q\n0.75 0.75 0.75 rg\n"));
        // A border's width with no color draws nothing either.
        let uncolored = WidgetLook::new(12.0, 12.0);
        assert_eq!(toggled(radio, None, &uncolored).off, None);
    }
    let mut looks = Vec::new();
    let mut bg = none;
    bg.background = Some(Color::Gray(0.9));
    looks.push(bg);
    let mut bordered = WidgetLook::new(12.0, 12.0);
    bordered.border_color = Some(Color::Gray(0.0));
    looks.push(bordered);
    let mut banded = WidgetLook::new(12.0, 12.0);
    banded.border = BoxBorder::new(1.0, BoxBorderStyle::Inset);
    looks.push(banded);
    for look in &looks {
        for radio in [false, true] {
            let made = toggled(radio, Some("4"), look);
            let off = made.off.expect("a frame that paints is made");
            assert!(made.on.content.starts_with(&off.content));
            assert!(!text(&off.content).contains("Tj") && off.needs.0.is_empty());
            assert_eq!((off.bbox, off.matrix), (made.on.bbox, made.on.matrix));
        }
    }
    // A radio button's round frame, beveled, with no width and no
    // background, has nothing to draw when it's down either.
    let mut round = WidgetLook::new(12.0, 12.0);
    round.border = BoxBorder::new(0.0, BoxBorderStyle::Beveled);
    let made = toggled(true, None, &round);
    assert_eq!((made.off, made.down_off), (None, None));
}

/// The down appearances fill the background less 0.25 in its own space,
/// exchange a beveled border's shades and draw an inset one's black and
/// white.
#[test]
fn down_appearances_press_the_background_and_exchange_the_bands() {
    for (background, pressed) in [
        (Some(Color::Gray(0.75)), "0.5 g"),
        (Some(Color::Rgb([1.0, 0.0, 0.0])), "0.75 0 0 rg"),
        (Some(Color::Cmyk([0.1, 0.2, 0.3, 0.4])), "0 0 0.05 0.15 k"),
        (None, "0.75 0.75 0.75 rg"),
    ] {
        let mut look = WidgetLook::new(20.0, 20.0);
        look.background = background;
        let made = toggled(false, Some("4"), &look);
        let down = text(&made.down_off.as_ref().unwrap().content);
        assert!(
            down.starts_with(&format!("q\n{pressed}\n0 0 20 20 re\nf\nQ\n")),
            "{down}"
        );
    }
    let mut beveled = WidgetLook::new(20.0, 20.0);
    beveled.background = Some(Color::Gray(0.8));
    beveled.border = BoxBorder::new(1.0, BoxBorderStyle::Beveled);
    let made = toggled(false, None, &beveled);
    let (normal, down) = (
        text(&made.off.as_ref().unwrap().content),
        text(&made.down_off.as_ref().unwrap().content),
    );
    assert!(normal.contains("q\n1 g\n1 1 m\n"), "{normal}");
    assert!(normal.contains("f\n0.4 g\n1 1 m\n19 1 l\n"), "{normal}");
    assert!(down.contains("q\n0.4 g\n1 1 m\n"), "{down}");
    assert!(down.contains("f\n1 g\n1 1 m\n19 1 l\n"), "{down}");
    let mut inset = beveled;
    inset.border = BoxBorder::new(1.0, BoxBorderStyle::Inset);
    let made = toggled(false, None, &inset);
    let down = text(&made.down_off.as_ref().unwrap().content);
    assert!(down.contains("q\n0 g\n1 1 m\n") && down.contains("f\n1 g\n1 1 m\n19 1 l\n"));
}

/// A radio button whose mark is the circle has a round frame; any other
/// mark, and a check box's circle, a square one.
#[test]
fn a_radio_with_the_circle_has_a_round_frame() {
    let mut look = WidgetLook::new(16.0, 16.0);
    look.background = Some(Color::Gray(0.9));
    look.border_color = Some(Color::Gray(0.0));
    for (radio, caption, round) in [
        (true, None, true),
        (true, Some("l"), true),
        (true, Some("4"), false),
        (false, Some("l"), false),
    ] {
        let made = toggled(radio, caption, &look);
        let off = text(&made.off.as_ref().unwrap().content);
        assert_eq!(off.contains(" c\n"), round, "{off}");
        assert_eq!(off.contains(" re\n"), !round, "{off}");
    }
    // The circle in the centre square less 1, from the middle of its left
    // side; a solid border's circle half its width inside.
    let made = toggled(true, None, &look);
    let off = text(&made.off.as_ref().unwrap().content);
    assert!(
        off.starts_with("q\n0.9 g\n1 8 m\n1 11.866 4.134 15 8 15 c\n"),
        "{off}"
    );
    assert!(off.contains("1 w\n0 G\n1.5 8 m\n"), "{off}");
}

#[test]
fn toggles_refuse_no_box_numbers_not_finite_and_a_caption_past_its_cap() {
    let refused =
        |look: &WidgetLook<'_>, da: &DefaultAppearance, caption: Option<&str>, limits: &Limits| {
            let mut button = ToggleButton::new();
            button.caption = caption;
            let res = zapf().res().unwrap();
            let a = check_box(&button, look, da, res.clone(), limits).unwrap_err();
            let b = radio_button(&button, look, da, res, limits).unwrap_err();
            assert_eq!(a, b);
            a
        };
    let plain = da("/ZaDb 0 Tf 0 g");
    let limits = Limits::DEFAULT;
    for (w, h) in [
        (0.0, 10.0),
        (10.0, -1.0),
        (f32::NAN, 10.0),
        (10.0, f32::INFINITY),
    ] {
        assert_eq!(
            refused(&WidgetLook::new(w, h), &plain, None, &limits),
            Unmade::NoBox
        );
    }
    let mut look = WidgetLook::new(10.0, 10.0);
    look.background = Some(Color::Rgb([0.0, f32::NAN, 0.0]));
    assert_eq!(refused(&look, &plain, None, &limits), Unmade::NotFinite);
    let mut sized = plain.clone();
    sized.font_size = Some(f32::INFINITY);
    assert_eq!(
        refused(&WidgetLook::new(10.0, 10.0), &sized, None, &limits),
        Unmade::NotFinite
    );
    sized.font_size = Some(1e36);
    assert_eq!(
        refused(&WidgetLook::new(10.0, 10.0), &sized, None, &limits),
        Unmade::NotFinite
    );
    let mut small = Limits::DEFAULT;
    small.text = 2;
    assert_eq!(
        refused(&WidgetLook::new(10.0, 10.0), &plain, Some("4ab"), &small),
        Unmade::Limit(Cap::Text)
    );
}

fn helvetica() -> FieldFont {
    FieldFont::standard(Std14::Helvetica)
}

fn pushed(
    button: &PushButton<'_>,
    look: &WidgetLook<'_>,
    da: &DefaultAppearance,
) -> Result<PushAppearances, Unmade> {
    let font = helvetica();
    push_button(
        button,
        look,
        da,
        font.res().unwrap(),
        &font.layout(),
        &Limits::DEFAULT,
    )
}

/// A push button's caption is centred both ways in the box less its
/// border, at `/DA`'s size or automatic, clipped there; its rollover and
/// down appearances are made where it has them, each its own caption or the
/// normal one.
#[test]
fn a_push_buttons_caption_is_centred_and_its_states_made_where_asked() {
    let mut look = WidgetLook::new(50.0, 20.0);
    look.background = Some(Color::Gray(0.75));
    look.border = BoxBorder::new(1.0, BoxBorderStyle::Beveled);
    // "Push me" at an automatic size in a client 46 by 16: 10, as PDFium.
    for da_text in ["/Helv 0 Tf 0 g", "0 g"] {
        let made = pushed(&PushButton::new("Push me"), &look, &da(da_text)).unwrap();
        let normal = &made.normal;
        assert_eq!(normal.font_size, 10.0);
        let tm = last_operands(&normal.content, "Tm");
        assert!(
            (tm[4] - 5.27).abs() < 0.01 && (tm[5] - 6.395).abs() < 0.01,
            "{tm:?}"
        );
        assert!(text(&normal.content).contains("q\n2 2 46 16 re\nW\nn\n0 g\nBT\n"));
        assert!(made.rollover.is_none() && made.down.is_none());
    }
    // Wider than the box at 12: centred all the same, past both sides.
    let made = pushed(
        &PushButton::new("Push me"),
        &look,
        &da("/Helv 12 Tf 1 0 0 rg"),
    )
    .unwrap();
    let tm = last_operands(&made.normal.content, "Tm");
    assert!((tm[4] - 1.324).abs() < 0.01, "{tm:?}");
    assert!(made.normal.overflowed);

    // Rollover and down: their own captions, or the normal one where they
    // have none or an empty one.
    for (rollover, down, shown) in [
        (Some("Over"), Some("Down"), ["Go", "Over", "Down"]),
        (None, Some(""), ["Go", "Go", "Go"]),
    ] {
        let mut button = PushButton::new("Go");
        button.rollover_caption = rollover;
        button.down_caption = down;
        button.pressed_states = true;
        let made = pushed(&button, &look, &da("/Helv 10 Tf 0 g")).unwrap();
        let each = [
            &made.normal,
            made.rollover.as_ref().unwrap(),
            made.down.as_ref().unwrap(),
        ];
        for (made, shown) in each.iter().zip(shown) {
            assert!(text(&made.content).ends_with(&format!("({shown}) Tj\nET\nQ\n")));
        }
        // The rollover appearance's frame is the normal one's; the down
        // one's is pressed.
        assert!(text(&each[1].content).starts_with("q\n0.75 g\n"));
        assert!(text(&each[2].content).starts_with("q\n0.5 g\n"));
    }
    // No caption: the frame alone, no clip, no text, no font.
    let made = pushed(&PushButton::new(""), &look, &da("/Helv 10 Tf 0 g")).unwrap();
    let normal = text(&made.normal.content);
    assert!(!normal.contains("\nW\n") && !normal.contains("BT"));
    assert!(made.normal.needs.0.is_empty());
    assert_eq!(made.normal.font_size, 0.0);
}

#[test]
fn a_push_button_refuses_what_its_font_cant_draw_at_its_offset() {
    let look = WidgetLook::new(50.0, 20.0);
    let plain = da("/Helv 10 Tf 0 g");
    let refused = |button: &PushButton<'_>| pushed(button, &look, &plain).unwrap_err();
    assert_eq!(
        refused(&PushButton::new("a\nb\u{2603}")),
        Unmade::Unencodable(UnencodableChar {
            at: 3,
            character: '\u{2603}'
        })
    );
    // The normal caption first, then the rollover's and the down one's, each
    // at its offset in its own.
    let mut button = PushButton::new("Go");
    button.pressed_states = true;
    button.down_caption = Some("x\u{2603}");
    assert_eq!(
        refused(&button),
        Unmade::Unencodable(UnencodableChar {
            at: 1,
            character: '\u{2603}'
        })
    );
    // Not asked for, not refused.
    button.pressed_states = false;
    assert!(pushed(&button, &look, &plain).is_ok());
    let mut small = Limits::DEFAULT;
    small.text = 3;
    let font = helvetica();
    let made = push_button(
        &PushButton::new("Push"),
        &look,
        &plain,
        font.res().unwrap(),
        &font.layout(),
        &small,
    );
    assert_eq!(made.unwrap_err(), Unmade::Limit(Cap::Text));
    assert_eq!(
        pushed(&PushButton::new("Go"), &WidgetLook::new(0.0, 20.0), &plain).unwrap_err(),
        Unmade::NoBox
    );
    let mut huge = plain.clone();
    huge.font_size = Some(1e36);
    assert_eq!(
        pushed(&PushButton::new("Go"), &look, &huge).unwrap_err(),
        Unmade::NotFinite
    );
}

/// A widget's dictionary: `entries`, a widget's subtype, and a box of 20
/// by 20 where they give none.
fn widget(mut entries: Dict) -> Dict {
    entries.set("Subtype", Object::name("Widget"));
    if entries.get("Rect").is_none() {
        entries.set(
            "Rect",
            vec![
                Object::from(0),
                Object::from(0),
                Object::from(20),
                Object::from(20),
            ],
        );
    }
    entries
}

/// A form of `fields`, each an object from 10 on, its default resources
/// `/ZaDb`, object 1, written by name, `/Helv`, object 2, and `/ZaEnc`,
/// object 3, ZapfDingbats with an encoding of its own; and `more` objects.
fn form_of(fields: Vec<Dict>, more: Vec<(u32, Object)>) -> (Objects, Dict) {
    let mut objects = Objects::new();
    objects.insert(id(1), Object::from(Std14::ZapfDingbats.dict()));
    objects.insert(id(2), Object::from(Std14::Helvetica.dict()));
    let mut encoded = Std14::ZapfDingbats.dict();
    encoded.set(
        "Encoding",
        dict! { "Differences" => vec![Object::from(52), Object::name("a71")] },
    );
    objects.insert(id(3), Object::from(encoded));
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
            "DR" => dict! {
                "Font" => dict! { "ZaDb" => id(1), "Helv" => id(2), "ZaEnc" => id(3) },
            },
        },
    };
    (objects, catalog)
}

fn form_stream() -> Object {
    Object::from(Stream::new(
        dict! { "Type" => Object::name("XObject"), "Subtype" => Object::name("Form") },
        b"0 g 5 5 10 10 re f".to_vec(),
    ))
}

/// Reading a toggle asks of `/Opt` only whether it's an array: nothing of
/// it is decoded or told for a widget, so a field's widgets are each read
/// alike however many it has, and what's wrong in `/Opt` is told once, by
/// the field's group. An empty `/Opt` is one too: its widgets go by their
/// indexes.
#[test]
fn a_toggle_reads_nothing_of_opt_but_whether_it_is_there() {
    let kid = |parent: u32| widget(dict! { "Parent" => id(parent) });
    let kids =
        |from: u32| -> Vec<Object> { (from..from + 3).map(|n| Object::from(id(n))).collect() };
    let fields = vec![
        dict! {
            "T" => Object::text("wrong"),
            "FT" => Object::name("Btn"),
            "Ff" => 1 << 15,
            "Opt" => vec![Object::text("a"), Object::from(7), Object::text("c")],
            "Kids" => kids(40),
        },
        dict! {
            "T" => Object::text("empty"),
            "FT" => Object::name("Btn"),
            "Opt" => Vec::<Object>::new(),
            "Kids" => kids(43),
        },
        dict! {
            "T" => Object::text("name"),
            "FT" => Object::name("Btn"),
            "Opt" => Object::name("a"),
            "Kids" => kids(46),
        },
    ];
    let more = (40..49)
        .map(|n| (n, Object::from(kid(10 + (n - 40) / 3))))
        .collect();
    let (objects, catalog) = form_of(fields, more);
    let source = reading(&objects);
    let limits = Limits::DEFAULT;
    let form = Form::read(source, &catalog, Rules::Iso, &limits)
        .unwrap()
        .unwrap();
    let wrong_opt = |told: &[(Place, Fault)]| {
        let wrong = Fault::WrongType { key: "Opt" };
        told.iter().filter(|(_, fault)| *fault == wrong).count()
    };
    for (field, count, names, faults) in [
        (&form.fields()[0], Some(3), ["0", "1", "2"], 1),
        (&form.fields()[1], Some(0), ["0", "1", "2"], 0),
        // An `/Opt` that is no array is none, told by the group.
        (&form.fields()[2], None, ["Yes", "Yes", "Yes"], 1),
    ] {
        assert_eq!(field.option_count(source, Rules::Iso, &limits), Ok(count));
        let mut told: Vec<(Place, Fault)> = Vec::new();
        for (index, widget) in field.widgets().iter().enumerate() {
            assert_eq!(widget.index(), index);
            let read = widget_toggle(field, widget, source, Rules::Iso, &limits, &mut told);
            assert_eq!(read.unwrap().on_state(), names[index].as_bytes());
            // A copy of the widget is read as the field's own is.
            let copy = *widget;
            let read = widget_toggle(field, &copy, source, Rules::Iso, &limits, &mut ());
            assert_eq!(read.unwrap().on_state(), names[index].as_bytes());
        }
        assert_eq!(wrong_opt(&told), 0, "{told:?}");
        let mut told: Vec<(Place, Fault)> = Vec::new();
        let group = field.group_with(source, Rules::Iso, &limits, &mut told);
        let group = group.unwrap().unwrap();
        assert_eq!(wrong_opt(&told), faults, "{told:?}");
        assert_eq!(group.group().has_opt, count.is_some());
    }
}

/// The on state a toggle's appearances go under is the name its widget's
/// normal appearance has, else its index where the field has an `/Opt`
/// array, whether or not it has an entry for the widget, else `Yes`, as
/// the field's group names it.
#[test]
fn on_states_are_the_widgets_own_else_its_index_else_yes() {
    let names = ["1", "Yes", "On", "spouse", "Choice1"];
    let mut fields = Vec::new();
    for (i, name) in names.iter().enumerate() {
        fields.push(widget(dict! {
            "T" => Object::text(&format!("own{i}")),
            "FT" => Object::name("Btn"),
            "Ff" => if i % 2 == 0 { 0 } else { 1 << 15 },
            "DA" => Object::text("/ZaDb 0 Tf 0 g"),
            "AS" => Object::name("Off"),
            "AP" => dict! { "N" => dict! { *name => id(50) } },
        }));
    }
    // A check box field of three kids and an `/Opt` of two: the third has
    // no entry, and is named by its index as the others are, as its group
    // names it and as PDFium names every widget of a field with `/Opt`.
    let kid = || widget(dict! { "Parent" => id(15), "MK" => dict! { "CA" => Object::text("4") } });
    fields.push(dict! {
        "T" => Object::text("opt"),
        "FT" => Object::name("Btn"),
        "Opt" => vec![Object::text("a"), Object::text("b")],
        "Kids" => vec![Object::from(id(40)), Object::from(id(41)), Object::from(id(42))],
    });
    fields.push(widget(
        dict! { "T" => Object::text("bare"), "FT" => Object::name("Btn") },
    ));
    let more = vec![
        (40, Object::from(kid())),
        (41, Object::from(kid())),
        (42, Object::from(kid())),
        (50, form_stream()),
    ];
    let (objects, catalog) = form_of(fields, more);
    let source = reading(&objects);
    let limits = Limits::DEFAULT;
    for rules in [Rules::Iso, Rules::Pdfium] {
        let form = Form::read(source, &catalog, rules, &limits)
            .unwrap()
            .unwrap();
        let mut seen = Vec::new();
        for field in form.fields() {
            let group = field.group(source, rules, &limits).unwrap().unwrap();
            for (index, widget) in field.widgets().iter().enumerate() {
                let read = widget_toggle(field, widget, source, rules, &limits, &mut ()).unwrap();
                let made = toggle_for_widget(&form, field, widget, source, rules, &limits, &mut ())
                    .unwrap();
                assert_eq!(made.on_state, read.on_state());
                assert_eq!(
                    group.group().on_state(index).as_deref(),
                    Some(read.on_state())
                );
                seen.push(String::from_utf8(made.on_state).unwrap());
            }
        }
        assert_eq!(
            seen,
            ["1", "Yes", "On", "spouse", "Choice1", "0", "1", "2", "Yes"]
        );
    }
}

/// The mark is drawn in the default resources' ZapfDingbats where `/DA`
/// names it as the standard font is written; else in ZapfDingbats by name,
/// a fallback: for one with an encoding of its own, another font, or a name
/// the resources lack (told).
#[test]
fn the_mark_is_drawn_in_zapfdingbats_as_its_written_by_name() {
    let toggle = |da: &str| {
        widget(dict! {
            "T" => Object::text(da),
            "FT" => Object::name("Btn"),
            "DA" => Object::text(da),
            "MK" => dict! { "CA" => Object::text("4") },
        })
    };
    let das = [
        "/ZaDb 0 Tf 0 g",
        "/ZaEnc 0 Tf 0 g",
        "/Helv 0 Tf 0 g",
        "/Gone 0 Tf 0 g",
        "0 g",
    ];
    let (objects, catalog) = form_of(das.iter().map(|d| toggle(d)).collect(), Vec::new());
    let source = reading(&objects);
    let limits = Limits::DEFAULT;
    let form = Form::read(source, &catalog, Rules::Iso, &limits)
        .unwrap()
        .unwrap();
    for (field, da) in form.fields().iter().zip(das) {
        let widget = &field.widgets()[0];
        let mut told: Vec<(Place, Fault)> = Vec::new();
        let made = toggle_for_widget(&form, field, widget, source, Rules::Iso, &limits, &mut told)
            .unwrap();
        let res = made.appearances.on.needs.0[0].res.clone();
        match da {
            "/ZaDb 0 Tf 0 g" => {
                assert!(!made.font.is_fallback());
                assert_eq!(res.body(), &ResBody::Written(id(1)));
            }
            _ => {
                assert!(made.font.is_fallback(), "{da}");
                assert_eq!(res, zapf().res().unwrap(), "{da}");
            }
        }
        assert!(text(&made.appearances.on.content).contains("(4) Tj"));
        let missing = told.iter().any(|(_, f)| *f == Fault::FontNotInResources);
        assert_eq!(missing, da.starts_with("/Gone"), "{da}: {told:?}");
        // No default appearance's font is wanted for a mark.
        assert!(
            !told
                .iter()
                .any(|(_, f)| *f == Fault::MissingDefaultAppearance)
        );
    }
}

/// A push button's widget: its captions, whether it has rollover and down
/// appearances, 12 points with no `/DA` (told where a caption is drawn in a
/// font none names), an icon refused where it's shown, and the field's kind
/// held.
#[test]
fn a_push_buttons_widget_is_read_and_an_icon_refused() {
    let push = |name: &str, more: Dict| {
        let mut field = widget(dict! {
            "T" => Object::text(name),
            "FT" => Object::name("Btn"),
            "Ff" => 1 << 16,
        });
        for (key, value) in more.iter() {
            field.set(key.to_vec(), value.clone());
        }
        field
    };
    let fields = vec![
        push(
            "states",
            dict! {
                "DA" => Object::text("/Helv 0 Tf 0 g"),
                "H" => Object::name("P"),
                "MK" => dict! {
                    "CA" => Object::text("Go"),
                    "RC" => Object::text("Over"),
                    "AC" => Object::text("Down"),
                },
            },
        ),
        push(
            "no da",
            dict! { "MK" => dict! { "CA" => Object::text("Go") } },
        ),
        push("no caption", Dict::new()),
        push(
            "icon alone, none",
            dict! {
                "DA" => Object::text("/Helv 0 Tf 0 g"),
                "MK" => dict! { "CA" => Object::text("Go"), "TP" => 1 },
            },
        ),
        push(
            "icon beside",
            dict! { "MK" => dict! { "CA" => Object::text("Go"), "I" => id(50), "TP" => 2 } },
        ),
        push(
            "icon, caption alone",
            dict! { "MK" => dict! { "CA" => Object::text("Go"), "I" => id(50) } },
        ),
        widget(dict! { "T" => Object::text("text"), "FT" => Object::name("Tx") }),
        widget(dict! { "T" => Object::text("check"), "FT" => Object::name("Btn") }),
    ];
    let (objects, catalog) = form_of(fields, vec![(50, form_stream())]);
    let source = reading(&objects);
    let limits = Limits::DEFAULT;
    let form = Form::read(source, &catalog, Rules::Iso, &limits)
        .unwrap()
        .unwrap();
    let read = |index: usize, told: &mut Vec<(Place, Fault)>| {
        let field = &form.fields()[index];
        widget_push(
            field,
            &field.widgets()[0],
            source,
            Rules::Iso,
            &limits,
            told,
        )
    };
    let mut told = Vec::new();

    let states = read(0, &mut told).unwrap();
    let button = states.button();
    assert_eq!(
        (
            button.caption,
            button.rollover_caption,
            button.down_caption,
            button.pressed_states
        ),
        ("Go", Some("Over"), Some("Down"), true)
    );
    assert_eq!(states.font_name(), Some(&b"Helv"[..]));
    assert!(told.is_empty());

    let no_da = read(1, &mut told).unwrap();
    assert_eq!(no_da.default_appearance().font_size, Some(12.0));
    assert_eq!(no_da.font_name(), None);
    assert!(!no_da.button().pressed_states);
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].1, Fault::MissingDefaultAppearance);
    told.clear();
    // Nothing drawn, nothing told.
    assert_eq!(read(2, &mut told).unwrap().button().caption, "");
    assert!(told.is_empty());
    assert_eq!(read(3, &mut told).unwrap().button().caption, "");
    assert_eq!(read(4, &mut told).unwrap_err(), Unmade::Icon);
    assert_eq!(read(5, &mut told).unwrap().button().caption, "Go");
    assert_eq!(read(6, &mut told).unwrap_err(), Unmade::NotButton);
    assert_eq!(read(7, &mut told).unwrap_err(), Unmade::NotButton);
    // And a toggle's reader refuses a push button and a text field.
    for index in [0, 6] {
        let field = &form.fields()[index];
        let toggle = widget_toggle(
            field,
            &field.widgets()[0],
            source,
            Rules::Iso,
            &limits,
            &mut (),
        );
        assert_eq!(toggle.unwrap_err(), Unmade::NotButton);
    }

    // The convenience: the field's font, its name, and what `make` makes.
    let field = &form.fields()[0];
    let widget = &field.widgets()[0];
    let made = push_for_widget(&form, field, widget, source, Rules::Iso, &limits, &mut ()).unwrap();
    assert!(!made.font.is_fallback());
    assert_eq!(made.font_name.as_deref(), Some(&b"Helv"[..]));
    let again = states
        .make(made.font.res().unwrap(), &made.font.layout(), &limits)
        .unwrap();
    assert_eq!(made.appearances, again);
    let need = &made.appearances.normal.needs.0[0];
    assert_eq!(need.res.body(), &ResBody::Written(id(2)));
    let field = &form.fields()[1];
    let widget = &field.widgets()[0];
    let made = push_for_widget(&form, field, widget, source, Rules::Iso, &limits, &mut ()).unwrap();
    assert!(made.font.is_fallback());
    assert_eq!(made.appearances.normal.font_size, 12.0);
}

/// The look and the default appearance a button's reader reads are those
/// `widget_text` reads of a text widget with the same entries.
#[test]
fn a_buttons_look_is_read_as_a_text_widgets() {
    let with = |name: &str, ft: &str, ff: i64| {
        widget(dict! {
            "T" => Object::text(name),
            "FT" => Object::name(ft),
            "Ff" => ff,
            "Rect" => vec![Object::from(5), Object::from(5), Object::from(65), Object::from(25)],
            "DA" => Object::text("/Helv 9 Tf 0 0 1 rg"),
            "MK" => dict! {
                "R" => 90,
                "BG" => vec![Object::from(0.9)],
                "BC" => vec![Object::from(1), Object::from(0), Object::from(0)],
            },
            "BS" => dict! {
                "W" => 2,
                "S" => Object::name("D"),
                "D" => vec![Object::from(2), Object::from(1)],
            },
        })
    };
    let fields = vec![
        with("text", "Tx", 0),
        with("check", "Btn", 0),
        with("push", "Btn", 1 << 16),
    ];
    let (objects, catalog) = form_of(fields, Vec::new());
    let source = reading(&objects);
    let limits = Limits::DEFAULT;
    let form = Form::read(source, &catalog, Rules::Pdfium, &limits)
        .unwrap()
        .unwrap();
    let [text_field, check, push] = form.fields() else {
        panic!("three fields")
    };
    let rules = Rules::Pdfium;
    let tw = &text_field.widgets()[0];
    let text = widget_text(text_field, tw, source, rules, None, &limits, &mut ()).unwrap();
    let cw = &check.widgets()[0];
    let toggle = widget_toggle(check, cw, source, rules, &limits, &mut ()).unwrap();
    let pw = &push.widgets()[0];
    let pushed = widget_push(push, pw, source, rules, &limits, &mut ()).unwrap();
    assert_eq!(toggle.look(), text.look());
    assert_eq!(pushed.look(), text.look());
    assert_eq!(toggle.default_appearance(), text.default_appearance());
    assert_eq!(pushed.default_appearance(), text.default_appearance());
}

/// Bound through `AnnotMut`'s setters, an appearance with no `Off` leaves
/// none in the widget's normal appearance.
#[test]
fn a_widget_bound_from_a_made_toggle_has_no_off_where_none_was_made() {
    let mut look = WidgetLook::new(12.0, 12.0);
    look.border = BoxBorder::solid(0.0);
    let made = toggled(false, Some("4"), &look);
    assert!(made.off.is_none());
    let mut objects = Objects::new();
    let mut add = |n: u32, appearance: &Appearance| {
        let stream = Stream::new(Dict::new(), appearance.content.clone());
        objects.insert(id(n), Object::from(stream));
        id(n)
    };
    let on = add(21, &made.on);
    let down_on = add(22, &made.down_on);
    let down_off = add(23, made.down_off.as_ref().unwrap());
    let mut normal = vec![(&b"Yes"[..], on)];
    normal.extend(made.off.as_ref().map(|_| (&b"Off"[..], on)));
    let mut dict = widget(dict! { "T" => Object::text("c"), "FT" => Object::name("Btn") });
    AnnotMut::new(&mut dict)
        .set_appearance_states(AppearanceMode::Normal, &normal)
        .set_appearance_states(
            AppearanceMode::Down,
            &[(b"Yes", down_on), (b"Off", down_off)],
        )
        .set_appearance_state(b"Off");
    objects.insert(id(10), Object::from(dict));
    let catalog = dict! { "AcroForm" => dict! { "Fields" => vec![Object::from(id(10))] } };
    let source = reading(&objects);
    let form = Form::read(source, &catalog, Rules::Iso, &Limits::DEFAULT)
        .unwrap()
        .unwrap();
    let states = form.fields()[0].widgets()[0]
        .states(source, Rules::Iso, &Limits::DEFAULT)
        .unwrap();
    assert_eq!(states.on_state.as_deref(), Some(&b"Yes"[..]));
    assert!(!states.has_off);
    assert_eq!(states.state.as_deref(), Some(&b"Off"[..]));
}

/// No `Debug` of what's read or made shows a caption, a state's name, or a
/// font's.
#[test]
fn debug_prints_no_caption_or_state() {
    let mut toggle = ToggleButton::new();
    toggle.caption = Some("Zq#7");
    let mut push = PushButton::new("Zq#7 one");
    push.rollover_caption = Some("Zq#7 two");
    push.down_caption = Some("Zq#7 three");
    let shown = format!("{toggle:?} {push:?}");
    assert!(!shown.contains("Zq#7"), "{shown}");
    // A mark is the glyph its caption names, by its code where it's none of
    // the six PDFium knows: the one character of the caption drawn, and
    // nothing more of it.
    let mark = Mark::from_caption(toggle.caption, Mark::Check);
    assert_eq!(format!("{mark:?}"), format!("Other({})", b'Z'));
    assert_eq!(
        format!("{:?}", Mark::from_caption(Some("4 Zq#7"), Mark::Circle)),
        "Check"
    );

    let fields = vec![
        widget(dict! {
            "T" => Object::text("c"),
            "FT" => Object::name("Btn"),
            "DA" => Object::text("/ZaDb 0 Tf 0 g"),
            "MK" => dict! { "CA" => Object::text("Zq#7") },
            "AP" => dict! { "N" => dict! { "Wq#9" => id(50) } },
        }),
        widget(dict! {
            "T" => Object::text("p"),
            "FT" => Object::name("Btn"),
            "Ff" => 1 << 16,
            "DA" => Object::text("/Helv 0 Tf 0 g"),
            "MK" => dict! { "CA" => Object::text("Zq#7 caption") },
        }),
    ];
    let (objects, catalog) = form_of(fields, vec![(50, form_stream())]);
    let source = reading(&objects);
    let limits = Limits::DEFAULT;
    let form = Form::read(source, &catalog, Rules::Iso, &limits)
        .unwrap()
        .unwrap();
    let [check, push] = form.fields() else {
        panic!("two fields")
    };
    let (cw, pw) = (&check.widgets()[0], &push.widgets()[0]);
    let read = widget_toggle(check, cw, source, Rules::Iso, &limits, &mut ()).unwrap();
    assert_eq!(read.on_state(), b"Wq#9");
    let made = toggle_for_widget(&form, check, cw, source, Rules::Iso, &limits, &mut ()).unwrap();
    let read_push = widget_push(push, pw, source, Rules::Iso, &limits, &mut ()).unwrap();
    let made_push = push_for_widget(&form, push, pw, source, Rules::Iso, &limits, &mut ()).unwrap();
    let shown = format!(
        "{read:?} {made:?} {read_push:?} {made_push:?} {:?}",
        made.appearances
    );
    for secret in ["Zq#7", "Wq#9", "Helv", "ZaDb"] {
        assert!(!shown.contains(secret), "{secret} in {shown}");
    }
}

#[test]
fn button_types_are_send_and_sync() {
    fn both<T: Send + Sync>() {}
    both::<Mark>();
    both::<ToggleButton<'_>>();
    both::<PushButton<'_>>();
    both::<ToggleAppearances>();
    both::<PushAppearances>();
    both::<WidgetToggle>();
    both::<WidgetPush>();
    both::<MadeToggle>();
    both::<MadePush>();
}
