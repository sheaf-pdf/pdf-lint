//! The strings content shows, built in code: each operator the walk reads, the forms
//! it follows and how far, the resources each looks fonts up in, and what isn't read,
//! each as PDFium (chromium/8125) reads it for its text.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use pdf_content_streams::{Concat, ResCx};
use pdf_objects::object::Stream;
use pdf_objects::{Object, ObjectId, Resolve, Streams, Unreadable, Why, dict};

use crate::*;

/// A page's budget, as the tests read within it.
const BUDGET: usize = 256 << 20;

/// A file's objects in a map, each stream's data as it is but one's, which doesn't
/// read for `why`.
#[derive(Default)]
struct File {
    objects: BTreeMap<ObjectId, Object>,
    failing: Option<(ObjectId, Why)>,
}

impl File {
    fn add(&mut self, object: impl Into<Object>) -> ObjectId {
        let id = ObjectId::new(self.objects.len() as u32 + 1, 0);
        self.objects.insert(id, object.into());
        id
    }

    /// A form whose content is `content`, with `resources` of its own where they're
    /// given.
    fn form(&mut self, content: &str, resources: Option<Object>) -> ObjectId {
        let mut form = dict! { "Type" => "XObject", "Subtype" => "Form" };
        if let Some(resources) = resources {
            form.set("Resources", resources);
        }
        self.add(Stream::new(form, content.as_bytes().to_vec()))
    }
}

impl<'o> Resolve<'o> for &'o File {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        let file: &'o File = self;
        file.objects.get(&id)
    }
}

impl Streams for &File {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        if let Some((failing, why)) = self.failing
            && failing == id
        {
            return Err(Unreadable::new(id, why, "it doesn't read"));
        }
        let Some(stream) = self.objects.get(&id).and_then(Object::as_stream) else {
            return Err(Unreadable::new(id, Why::Damaged, "no such stream"));
        };
        let data = stream.data.bytes().unwrap_or_default();
        match data.len() > limit {
            true => Err(Unreadable::new(id, Why::Limit, "past the limit")),
            false => Ok(Cow::Borrowed(data)),
        }
    }
}

/// Courier, each code its own glyph: what most tests show in.
fn courier() -> Object {
    dict! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier",
        "Encoding" => "WinAnsiEncoding",
    }
    .into()
}

/// Courier with `A` drawn as `glyph`, so which font shows a string can be told by its
/// text.
fn courier_as(glyph: &str) -> Object {
    dict! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier",
        "Encoding" => dict! { "Differences" => vec![Object::from(65), Object::from(glyph)] },
    }
    .into()
}

/// What `content`, drawn with `resources`, shows over `file`, under `limits` and
/// within `budget`, fonts read anew.
fn walk_with(
    file: &File,
    resources: &Object,
    content: &str,
    limits: &Limits,
    budget: &mut usize,
) -> Shown {
    let content = Concat::new(&[content.as_bytes()]);
    let fonts = Fonts::default();
    shown(
        file,
        &fonts,
        Some(resources),
        &content,
        &ResCx::none(),
        limits,
        budget,
    )
}

/// [`walk_with`] the default caps and a page's budget.
fn walk(file: &File, resources: &Object, content: &str) -> Shown {
    walk_with(
        file,
        resources,
        content,
        &Limits::DEFAULT,
        &mut BUDGET.clone(),
    )
}

/// Each string's text, and the form it's in.
fn texts(shown: &Shown) -> Vec<(String, Option<ObjectId>)> {
    let text = |s: &ShownString| s.text().unwrap_or_else(|| "?".into());
    shown.strings.iter().map(|s| (text(s), s.form)).collect()
}

/// Each operator that shows a string shows it, at its operand, and `TJ` each string
/// of its array, its numbers passed over; `BT` and `ET` change nothing, and a string
/// shown outside them is shown, as PDFium shows it.
#[test]
fn each_operator_shows_its_string() {
    let file = File::default();
    let resources = Object::from(dict! { "Font" => dict! { "F1" => courier() } });
    let content = "/F1 12 Tf (a) Tj BT [(b) -20 1.5 (c) /N true ()] TJ 14 TL (d) ' 1 2 (e) \" ET";
    let read = walk(&file, &resources, content);
    assert!(read.is_whole());
    assert_eq!(read.text(), "abcde");
    let at: Vec<_> = read
        .strings
        .iter()
        .map(|s| &content[s.at.clone()])
        .collect();
    assert_eq!(at, ["(a)", "(b)", "(c)", "(d)", "(e)"]);
    // Each is Courier, read once.
    let font = &read.strings[0].font;
    assert!(read.strings.iter().all(|s| Arc::ptr_eq(&s.font, font)));
}

/// `Tj`'s operand as PDFium's parser reads a string of it: a name's bytes and `true`
/// and `false` as written; a number, `null`, an array or nothing shows nothing, nor
/// does an empty string. A `Tj` of two shows the last. A `TJ` of no array, or of an
/// array PDFium ends at an array inside it, shows nothing.
#[test]
fn what_an_operand_shows_is_pdfiums_string_of_it() {
    let file = File::default();
    let resources = Object::from(dict! { "Font" => dict! { "F1" => courier() } });
    let content = "/F1 12 Tf /Ab#43 Tj true Tj false Tj 12 Tj null Tj [(x)] Tj () Tj Tj \
                   (x) (y) Tj (z) TJ [(A) [(Z)] (Y)] TJ";
    let read = walk(&file, &resources, content);
    assert_eq!(read.text(), "AbCtruefalsey");
}

/// `Tf` by a name the resources' `/Font` hasn't, or whose value is no dictionary,
/// picks PDFium's stock Helvetica; a string shown before any `Tf` isn't shown, as
/// PDFium draws none; and `Tf`'s name may be a string, as PDFium reads it.
#[test]
fn a_font_the_resources_havent_is_pdfiums_stock_helvetica() {
    let mut file = File::default();
    let stream = file.add(Stream::new(courier().as_dict().unwrap().clone(), vec![]));
    let resources = Object::from(dict! {
        "Font" => dict! { "F1" => courier_as("B"), "S" => Object::from(stream), "N" => 5 },
    });
    let read = walk(
        &file,
        &resources,
        "(A) Tj /F9 12 Tf (A) Tj /S 9 Tf (A) Tj /N 9 Tf (A) Tj",
    );
    assert_eq!(read.text(), "AAA");
    let fonts = Fonts::default();
    for string in &read.strings {
        assert_eq!(string.font.id(), None);
        assert_eq!(string.font.kind(), fonts.stock().kind());
    }
    let read = walk(&file, &resources, "(F1) 12 Tf (A) Tj 12 12 Tf (A) Tj");
    assert_eq!(read.text(), "BA");
    assert!(read.is_whole());
}

/// `q` saves the font, `Q` restores it, and a `Q` with nothing saved is passed over;
/// `BT` keeps it.
#[test]
fn q_and_q_save_and_restore_the_font() {
    let file = File::default();
    let resources = Object::from(dict! {
        "Font" => dict! { "B" => courier_as("B"), "C" => courier_as("C") },
    });
    let read = walk(
        &file,
        &resources,
        "/B 9 Tf (A) Tj q /C 9 Tf (A) Tj Q (A) Tj BT (A) Tj ET",
    );
    assert_eq!(read.text(), "BCBB");
    let read = walk(&file, &resources, "q /B 9 Tf (A) Tj Q Q (A) Tj");
    assert_eq!(read.text(), "B", "the font before any was none");
    let read = walk(
        &file,
        &resources,
        "/B 9 Tf (A) Tj Q Q /C 9 Tf (A) Tj Q (A) Tj",
    );
    assert_eq!(read.text(), "BCC", "a Q with nothing saved is passed over");
}

/// A font PDFium doesn't load, a Type 0 font of no CIDFont, leaves the one before
/// it, or none.
#[test]
fn a_font_pdfium_doesnt_load_leaves_the_one_before() {
    let file = File::default();
    let unloaded = dict! { "Type" => "Font", "Subtype" => "Type0", "Encoding" => "Identity-H" };
    let resources = Object::from(dict! {
        "Font" => dict! { "B" => courier_as("B"), "T" => unloaded },
    });
    let read = walk(
        &file,
        &resources,
        "/T 9 Tf (A) Tj /B 9 Tf (A) Tj /T 9 Tf (A) Tj",
    );
    assert_eq!(read.text(), "BB");
    assert!(read.is_whole());
}

/// A simple font's `/ToUnicode` gives its codes' text; a Type 0 font's codes are
/// split as it draws them, each its `/ToUnicode`'s text.
#[test]
fn a_strings_text_is_its_fonts() {
    let mut file = File::default();
    let to_unicode = file.add(Stream::new(
        dict! {},
        b"1 begincodespacerange <00> <FF> endcodespacerange 1 beginbfchar <41> <005A> endbfchar"
            .to_vec(),
    ));
    let mut simple = courier().as_dict().unwrap().clone();
    simple.set("ToUnicode", Object::from(to_unicode));
    let cmap = file.add(Stream::new(
        dict! {},
        b"1 begincodespacerange <0000> <FFFF> endcodespacerange \
          2 beginbfchar <0041> <0048> <0042> <00E9> endbfchar"
            .to_vec(),
    ));
    let descendant = file.add(dict! {
        "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "Foo",
        "CIDSystemInfo" => dict! {
            "Registry" => Object::text("Adobe"), "Ordering" => Object::text("Identity"),
            "Supplement" => 0,
        },
    });
    let type0 = dict! {
        "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "Foo", "Encoding" => "Identity-H",
        "DescendantFonts" => vec![Object::from(descendant)], "ToUnicode" => Object::from(cmap),
    };
    let resources = Object::from(dict! { "Font" => dict! { "S" => simple, "T" => type0 } });
    let read = walk(
        &file,
        &resources,
        "/S 9 Tf (AA) Tj /T 9 Tf <00410042> Tj [<0041> 5 <0042>] TJ",
    );
    assert_eq!(
        texts(&read),
        [
            ("ZZ".into(), None),
            ("Hé".into(), None),
            ("H".into(), None),
            ("é".into(), None)
        ]
    );
    assert_eq!(read.text(), "ZZHéHé");
    // A code its font says no text of is U+FFFD in the text joined.
    let read = walk(&file, &resources, "/T 9 Tf <00410043> Tj");
    assert_eq!(read.strings[0].text(), None);
    assert_eq!(read.text(), "\u{FFFD}\u{FFFD}");
}

/// A form shows its strings where it's drawn, in the font of what draws it, a form
/// two deep too, each string said to be in its form; what a form picks isn't kept
/// after it, and its `Q` can't restore what draws it.
#[test]
fn a_forms_strings_are_shown_where_its_drawn() {
    let mut file = File::default();
    let inner = file.form("(A) Tj /C 9 Tf (A) Tj", None);
    let outer = file.form(
        "Q Q (A) Tj /X Do (A) Tj",
        Some(dict! { "XObject" => dict! { "X" => Object::from(inner) } }.into()),
    );
    let resources = Object::from(dict! {
        "Font" => dict! { "B" => courier_as("B"), "C" => courier_as("C") },
        "XObject" => dict! { "X" => Object::from(outer) },
    });
    let read = walk(&file, &resources, "/B 9 Tf /X Do (A) Tj");
    let (outer, inner) = (Some(outer), Some(inner));
    let expected = [
        ("B", outer),
        ("B", inner),
        ("C", inner),
        ("B", outer),
        ("B", None),
    ];
    let expected: Vec<_> = expected.into_iter().map(|(t, f)| (t.into(), f)).collect();
    assert_eq!(texts(&read), expected);
    assert!(read.is_whole());
}

/// A form's resources are its own, else those of what draws it; a category its own
/// haven't got, or one that's no dictionary, is the page's; one they have, without
/// the name, is the stock font.
#[test]
fn a_forms_resources_are_its_own_else_its_callers_else_the_pages() {
    let mut file = File::default();
    let b = file.add(courier_as("B"));
    let c = file.add(courier_as("C"));
    let page_font = dict! { "F1" => Object::from(b) };
    let shows = file.form("/F1 9 Tf (A) Tj", None);
    let cases: [(Object, &str); 5] = [
        // Its own, with no `/Font`: the page's.
        (dict! { "ProcSet" => vec![Object::from("PDF")] }.into(), "B"),
        // Its own `/Font` no dictionary: the page's.
        (dict! { "Font" => 7 }.into(), "B"),
        // Its own `/Font`, without `F1`: the stock font.
        (
            dict! { "Font" => dict! { "F2" => Object::from(c) } }.into(),
            "A",
        ),
        // Its own `/Font` with `F1`.
        (
            dict! { "Font" => dict! { "F1" => Object::from(c) } }.into(),
            "C",
        ),
        // No dictionary: none of its own, so what draws it's, the page's.
        (Object::from(9), "B"),
    ];
    for (own, text) in cases {
        let form = file.form("/F1 9 Tf (A) Tj", Some(own.clone()));
        let resources = Object::from(dict! {
            "Font" => page_font.clone(), "XObject" => dict! { "X" => Object::from(form) },
        });
        assert_eq!(walk(&file, &resources, "/X Do").text(), text, "{own:?}");
    }
    // A form of no resources drawn by one of its own looks up in its caller's, but a
    // category they haven't got in the page's.
    let caller = file.form(
        "/Y Do",
        Some(
            dict! {
                "XObject" => dict! { "Y" => Object::from(shows) },
                "Font" => dict! { "F1" => Object::from(c) },
            }
            .into(),
        ),
    );
    let resources = Object::from(dict! {
        "Font" => page_font.clone(), "XObject" => dict! { "X" => Object::from(caller) },
    });
    assert_eq!(walk(&file, &resources, "/X Do").text(), "C");
    let caller = file.form(
        "/Y Do",
        Some(dict! { "XObject" => dict! { "Y" => Object::from(shows) } }.into()),
    );
    let resources = Object::from(dict! {
        "Font" => page_font, "XObject" => dict! { "X" => Object::from(caller) },
    });
    assert_eq!(walk(&file, &resources, "/X Do").text(), "B");
}

/// `Do` follows a form, by its `/Subtype`, as PDFium does: an image, another
/// subtype, none, a stream written in place and what's no stream show nothing, and
/// aren't told; nor is a form whose stream doesn't decode, which is the file's.
#[test]
fn only_a_form_is_followed() {
    let mut file = File::default();
    let content = b"/F1 9 Tf (A) Tj".to_vec();
    let with = |subtype: Option<&str>| {
        let mut dict = dict! { "Resources" => dict! { "Font" => dict! { "F1" => courier() } } };
        if let Some(subtype) = subtype {
            dict.set("Subtype", subtype);
        }
        Stream::new(dict, content.clone())
    };
    let image = file.add(with(Some("Image")));
    let ps = file.add(with(Some("PS")));
    let none = file.add(with(None));
    let not_a_stream = file.add(dict! { "Subtype" => "Form" });
    let damaged = file.form("(A) Tj", None);
    file.failing = Some((damaged, Why::Damaged));
    let resources = Object::from(dict! {
        "Font" => dict! { "F1" => courier() },
        "XObject" => dict! {
            "I" => Object::from(image), "P" => Object::from(ps), "N" => Object::from(none),
            "D" => Object::from(not_a_stream), "W" => Object::from(with(Some("Form"))),
            "Z" => Object::from(damaged), "R" => Object::reference(99),
        },
    });
    let read = walk(
        &file,
        &resources,
        "/F1 9 Tf /I Do /P Do /N Do /D Do /W Do /Z Do /R Do /Q Do",
    );
    assert!(read.strings.is_empty() && read.is_whole(), "{read:?}");
}

/// Forms are followed `forms` deep and no deeper, the next told as not followed; a
/// form within itself too, to that depth, as PDFium draws it.
#[test]
fn forms_are_followed_to_the_cap() {
    let mut file = File::default();
    let mut next = None;
    let mut ids = Vec::new();
    // Forms 1 to 41 deep, the page drawing the first, each showing its depth.
    for depth in (1..=41).rev() {
        let draws = next.map(|id| dict! { "XObject" => dict! { "X" => Object::from(id) } });
        let form = file.form(
            &format!("({}) Tj /X Do", depth % 10),
            draws.map(Object::from),
        );
        ids.push(form);
        next = Some(form);
    }
    let resources = Object::from(dict! {
        "Font" => dict! { "F1" => courier() },
        "XObject" => dict! { "X" => Object::from(next.unwrap()) },
    });
    let read = walk(&file, &resources, "/F1 9 Tf /X Do");
    assert_eq!(read.strings.len(), 40);
    assert_eq!(read.text(), "1234567890".repeat(4));
    assert_eq!(read.unread, [Unread::Deep { form: ids[0] }]);
    for forms in [0, 1, 39] {
        let mut limits = Limits::DEFAULT;
        limits.forms = forms;
        let read = walk_with(
            &file,
            &resources,
            "/F1 9 Tf /X Do",
            &limits,
            &mut BUDGET.clone(),
        );
        assert_eq!(read.strings.len(), forms);
        assert_eq!(
            read.unread,
            [Unread::Deep {
                form: ids[40 - forms]
            }]
        );
    }
    // Within itself: drawn 40 deep, the 41st told.
    let mut file = File::default();
    let font = dict! { "F1" => courier() };
    let itself = file.form("/F1 9 Tf (A) Tj /X Do", None);
    let own = dict! { "Font" => font.clone(), "XObject" => dict! { "X" => Object::from(itself) } };
    file.objects
        .get_mut(&itself)
        .unwrap()
        .as_stream_mut()
        .unwrap()
        .dict
        .set("Resources", own);
    let resources = Object::from(dict! { "XObject" => dict! { "X" => Object::from(itself) } });
    let read = walk(&file, &resources, "/X Do");
    assert_eq!(read.text(), "A".repeat(40));
    assert_eq!(read.unread, [Unread::Deep { form: itself }]);
}

/// Forms are drawn `form_uses` times a call and no more, a form a form draws among
/// them, each past it told as not followed; forms that each draw another twice are
/// drawn so far and stop.
#[test]
fn forms_are_drawn_as_many_times_as_the_cap() {
    let mut file = File::default();
    let inner = file.form("(B) Tj", None);
    let outer = file.form(
        "(A) Tj /Y Do",
        Some(dict! { "XObject" => dict! { "Y" => Object::from(inner) } }.into()),
    );
    let resources = Object::from(dict! {
        "Font" => dict! { "F1" => courier() },
        "XObject" => dict! { "X" => Object::from(outer) },
    });
    let mut limits = Limits::DEFAULT;
    limits.form_uses = 3;
    let read = walk_with(
        &file,
        &resources,
        "/F1 9 Tf /X Do /X Do /X Do",
        &limits,
        &mut BUDGET.clone(),
    );
    assert_eq!(read.text(), "ABA");
    let many = |form| Unread::Many { form };
    assert_eq!(read.unread, [many(inner), many(outer)]);
    // Twice in each, 40 deep: 2 to the 40 uses, cut at the cap's.
    let mut file = File::default();
    let twice = file.form("/F1 9 Tf (A) Tj /X Do /X Do", None);
    let own = dict! { "Font" => dict! { "F1" => courier() }, "XObject" => dict! { "X" => Object::from(twice) } };
    file.objects
        .get_mut(&twice)
        .unwrap()
        .as_stream_mut()
        .unwrap()
        .dict
        .set("Resources", own.clone());
    let read = walk(&file, &Object::from(own), "/X Do");
    assert_eq!(read.strings.len(), Limits::DEFAULT.form_uses);
    assert!(read.unread.contains(&Unread::Many { form: twice }));
}

/// A form whose stream doesn't read for a reason that's the caller's is told, with
/// why, never passed over; what's drawn after it is read.
#[test]
fn a_form_that_doesnt_read_is_told() {
    for why in [Why::Locked, Why::Capped, Why::Source] {
        let mut file = File::default();
        let form = file.form("(B) Tj", None);
        let other = file.form("(C) Tj", None);
        file.failing = Some((form, why));
        let resources = Object::from(dict! {
            "Font" => dict! { "F1" => courier() },
            "XObject" => dict! { "X" => Object::from(form), "Y" => Object::from(other) },
        });
        let read = walk(&file, &resources, "/F1 9 Tf (A) Tj /X Do /Y Do /X Do");
        assert_eq!(read.text(), "AC");
        let told = |unread: &Unread| match unread {
            Unread::Form {
                name,
                form: Some(id),
                error: Error::Unreadable(u),
            } => (name.clone(), *id, u.why),
            other => panic!("{other:?}"),
        };
        let told: Vec<_> = read.unread.iter().map(told).collect();
        assert_eq!(
            told,
            [(b"X".to_vec(), form, why), (b"X".to_vec(), form, why)]
        );
    }
}

/// A font that doesn't read for a reason that's the caller's is told, and what's
/// shown in it isn't, until another is picked or `Q` restores one.
#[test]
fn a_font_that_doesnt_read_is_told() {
    let mut file = File::default();
    let to_unicode = file.add(Stream::new(
        dict! {},
        b"1 beginbfchar <41> <0042> endbfchar".to_vec(),
    ));
    let mut locked = courier().as_dict().unwrap().clone();
    locked.set("ToUnicode", Object::from(to_unicode));
    file.failing = Some((to_unicode, Why::Locked));
    let resources = Object::from(dict! { "Font" => dict! { "F1" => courier(), "L" => locked } });
    let read = walk(
        &file,
        &resources,
        "/F1 9 Tf (A) Tj q /L 9 Tf (A) Tj Q (C) Tj /L 9 Tf (D) Tj",
    );
    assert_eq!(read.text(), "AC");
    let told: Vec<_> = (read.unread.iter())
        .map(|u| match u {
            Unread::Font {
                name,
                error: Error::Unreadable(u),
            } => (name.clone(), u.why),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        told,
        [(b"L".to_vec(), Why::Locked), (b"L".to_vec(), Why::Locked)]
    );
}

/// A font and a form are charged to the budget: a font once a call, a form each time
/// it's drawn. One past what's left is told, and what's shown in it isn't; what's
/// within it after is read.
#[test]
fn what_s_past_the_budget_is_told() {
    let mut file = File::default();
    let map =
        b"1 begincodespacerange <00> <FF> endcodespacerange 1 beginbfchar <41> <0042> endbfchar";
    let to_unicode = file.add(Stream::new(dict! {}, map.to_vec()));
    let mut mapped = courier().as_dict().unwrap().clone();
    mapped.set("ToUnicode", Object::from(to_unicode));
    let form = file.form("(C) Tj", None);
    let resources = Object::from(dict! {
        "Font" => dict! { "F1" => courier(), "M" => mapped },
        "XObject" => dict! { "X" => Object::from(form) },
    });
    let content = "/M 9 Tf (A) Tj /F1 9 Tf /X Do /X Do /M 9 Tf (A) Tj";
    let mut budget = BUDGET;
    let read = walk_with(&file, &resources, content, &Limits::DEFAULT, &mut budget);
    assert_eq!(read.text(), "BCCB");
    assert_eq!(BUDGET - budget, map.len() + 2 * "(C) Tj".len());
    // Too little for the font: it's told each time it's picked, the forms read.
    let mut budget = map.len() - 1;
    let read = walk_with(&file, &resources, content, &Limits::DEFAULT, &mut budget);
    assert_eq!(read.text(), "CC");
    let font = Unread::Font {
        name: b"M".to_vec(),
        error: Error::TooLarge,
    };
    assert_eq!(read.unread, [font.clone(), font]);
    assert_eq!(budget, map.len() - 1 - 12);
    // Enough for the font and one use of the form.
    let mut budget = map.len() + 6;
    let read = walk_with(&file, &resources, content, &Limits::DEFAULT, &mut budget);
    assert_eq!(read.text(), "BCB");
    let unread = Unread::Form {
        name: b"X".to_vec(),
        form: Some(form),
        error: Error::TooLarge,
    };
    assert_eq!(read.unread, [unread]);
    assert_eq!(budget, 0);
}

/// A second page's walk over the same fonts is charged for each it picks again, as
/// the first was: what each page costs is the same whichever read a font first.
#[test]
fn each_page_is_charged_for_its_fonts() {
    let mut file = File::default();
    let map =
        b"1 begincodespacerange <00> <FF> endcodespacerange 1 beginbfchar <41> <0042> endbfchar";
    let to_unicode = file.add(Stream::new(dict! {}, map.to_vec()));
    let font = file.add(dict! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier",
        "ToUnicode" => Object::from(to_unicode),
    });
    let resources = Object::from(dict! { "Font" => dict! { "F1" => Object::from(font) } });
    let content = Concat::new(&[b"/F1 9 Tf (A) Tj /F1 9 Tf (A) Tj"]);
    let fonts = Fonts::default();
    let mut budget = 2 * map.len();
    for _ in 0..2 {
        let read = shown(
            &file,
            &fonts,
            Some(&resources),
            &content,
            &ResCx::none(),
            &Limits::DEFAULT,
            &mut budget,
        );
        assert_eq!(read.text(), "BB");
        assert!(read.is_whole());
    }
    assert_eq!(budget, 0);
    let read = shown(
        &file,
        &fonts,
        Some(&resources),
        &content,
        &ResCx::none(),
        &Limits::DEFAULT,
        &mut budget,
    );
    assert_eq!(read.text(), "");
    let past = Unread::Font {
        name: b"F1".to_vec(),
        error: Error::TooLarge,
    };
    assert_eq!(read.unread, [past.clone(), past]);
}

/// Content that doesn't lex past a point is read to there, and that's told, for the
/// content given and for a form, whose caller reads on after it.
#[test]
fn content_read_as_far_as_it_lexes_is_told() {
    let mut file = File::default();
    let form = file.form("(B) Tj (C", None);
    let resources = Object::from(dict! {
        "Font" => dict! { "F1" => courier() },
        "XObject" => dict! { "X" => Object::from(form) },
    });
    let read = walk(&file, &resources, "/F1 9 Tf /X Do (A) Tj [(D) (E");
    assert_eq!(texts(&read), [("B".into(), Some(form)), ("A".into(), None)]);
    let kinds: Vec<_> = (read.unread.iter())
        .map(|u| match u {
            Unread::Content { form, error } => (*form, error.kind),
            other => panic!("{other:?}"),
        })
        .collect();
    use pdf_content_streams::ErrorKind;
    assert_eq!(
        kinds,
        [(Some(form), ErrorKind::String), (None, ErrorKind::String)]
    );
    // Past a cap of the content's.
    let mut limits = Limits::DEFAULT;
    limits.content.operands = 2;
    let read = walk_with(
        &file,
        &resources,
        "/F1 9 Tf (A) Tj 1 2 3 (B) Tj",
        &limits,
        &mut BUDGET.clone(),
    );
    assert_eq!(read.text(), "A");
    assert!(
        matches!(&read.unread[..], [Unread::Content { form: None, error }] if error.kind == pdf_content_streams::ErrorKind::Operands)
    );
}

/// A form's content is lexed with its own color spaces where its resources name
/// some, else the page's, so an inline image in it ends where PDFium ends it.
#[test]
fn a_forms_inline_images_end_by_its_own_color_spaces() {
    let mut file = File::default();
    // Four components of a byte each: the image's data runs past the content, where
    // with the page's, which haven't the space, it ends at its `EI`.
    let spaces = dict! {
        "ColorSpace" => dict! { "CS0" => vec![Object::from("Pattern"), Object::from("DeviceRGB")] },
    };
    let image = "BI /W 2 /H 1 /BPC 8 /CS /CS0 ID (A) Tj EI (B) Tj";
    let own = file.form(image, Some(spaces.into()));
    let none = file.form(image, Some(dict! { "ProcSet" => 1 }.into()));
    let resources = Object::from(dict! {
        "Font" => dict! { "F1" => courier() },
        "XObject" => dict! { "O" => Object::from(own), "N" => Object::from(none) },
    });
    let read = walk(&file, &resources, "/F1 9 Tf /O Do /N Do");
    assert_eq!(texts(&read), [("B".into(), Some(none))]);
    let kind = pdf_content_streams::ErrorKind::InlineImage;
    assert!(
        matches!(&read.unread[..], [Unread::Content { form, error }] if *form == Some(own) && error.kind == kind),
        "{read:?}"
    );
}

/// Nothing a `Debug` of what's shown prints is a string's bytes, its text or a
/// name's.
#[test]
fn no_debug_holds_a_string_or_a_name() {
    let mut file = File::default();
    let form = file.form("(MARKER) Tj", None);
    file.failing = Some((form, Why::Locked));
    let resources = Object::from(dict! {
        "Font" => dict! { "MARKERFONT" => courier() },
        "XObject" => dict! { "MARKERFORM" => Object::from(form) },
    });
    let read = walk(
        &file,
        &resources,
        "/MARKERFONT 9 Tf (MARKER) Tj /MARKERFORM Do",
    );
    assert_eq!(read.text(), "MARKER");
    for printed in [
        format!("{read:?}"),
        format!("{:?}", read.strings[0]),
        format!("{:?}", read.unread),
    ] {
        assert!(!printed.contains("MARKER"), "{printed}");
    }
}

/// What's shown can be kept and shared across threads.
#[test]
fn whats_shown_is_send_and_sync() {
    fn held<T: Send + Sync>() {}
    held::<Shown>();
    held::<ShownString>();
    held::<Unread>();
}
