//! Unit, cap, and hygiene tests for the field's font and the appearances
//! made in it.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::BTreeMap;

use pdf_content_streams::{Color, Res};
use pdf_fonts::{Font, GlyphBox, Std14};
use pdf_gfx::{BoxBorder, BoxBorderStyle, DefaultAppearance, LayoutFont, Matrix, UnencodableChar};
use pdf_objects::fault::Place;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{
    Data, Dict, Object, ObjectId, Resolve, Stream, Streams, Unreadable, Value, Why, WithStreams,
    dict,
};

use super::{
    Appearance, FieldFont, FieldLayoutFont, TextField, Unmade, WidgetLook, for_widget, text,
    widget_text,
};
use crate::{Cap, Error, Fault, FieldFlags, Form, Limits};

type Objects = BTreeMap<ObjectId, Object>;

/// `objects` with each stream's data, held as bytes, decoded as it is.
fn reading(objects: &Objects) -> impl Resolve<'_> + Streams + Copy + '_ {
    WithStreams::new(objects, move |id: ObjectId, _: usize| {
        match objects.get(&id).map(Object::value) {
            Some(Value::Stream(stream)) => match &stream.data {
                Data::Bytes(bytes) => Ok(bytes.clone()),
                _ => Err(Unreadable::new(id, Why::Detached, "not held")),
            },
            _ => Err(Unreadable::new(
                id,
                Why::Damaged,
                format!("{id} R isn't a stream"),
            )),
        }
    })
}

fn id(n: u32) -> ObjectId {
    ObjectId::new(n, 0)
}

fn std_font(base: &str, encoding: Object) -> Dict {
    dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("Type1"),
        "BaseFont" => Object::name(base),
        "Encoding" => encoding,
    }
}

/// Acrobat's `/Helv`: Helvetica's built-in encoding with PDFDocEncoding's
/// punctuation laid over it.
fn differences() -> Object {
    let names = |list: &[&str]| list.iter().map(|&n| Object::name(n)).collect::<Vec<_>>();
    let mut entries = vec![Object::from(128)];
    entries.extend(names(&[
        "bullet",
        "dagger",
        "daggerdbl",
        "ellipsis",
        "emdash",
        "endash",
    ]));
    entries.push(Object::from(141));
    entries.extend(names(&["quotedblleft", "quotedblright"]));
    entries.push(Object::from(160));
    entries.extend(names(&["Euro"]));
    Object::from(dict! { "Type" => Object::name("Encoding"), "Differences" => entries })
}

/// A font that isn't one of the standard 14 and isn't embedded: `/Widths` of
/// 500 for codes 32 to 126 and a descriptor's heights.
fn described(ascent: i64, descent: i64, descriptor: ObjectId) -> (Dict, Dict) {
    let widths: Vec<Object> = (32..=126).map(|_| Object::from(500)).collect();
    let font = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("Type1"),
        "BaseFont" => Object::name("Foo"),
        "FirstChar" => 32,
        "LastChar" => 126,
        "Widths" => widths,
        "Encoding" => Object::name("WinAnsiEncoding"),
        "FontDescriptor" => descriptor,
    };
    let descriptor = dict! {
        "Type" => Object::name("FontDescriptor"),
        "FontName" => Object::name("Foo"),
        "Flags" => 32,
        "FontBBox" => vec![Object::from(0), Object::from(-200), Object::from(1000), Object::from(800)],
        "ItalicAngle" => 0,
        "Ascent" => ascent,
        "Descent" => descent,
        "CapHeight" => 700,
        "StemV" => 80,
    };
    (font, descriptor)
}

fn find_told(
    fonts: Option<(&Dict, Option<ObjectId>)>,
    name: &[u8],
    objects: &Objects,
    limits: &Limits,
) -> (Result<FieldFont, Error>, Vec<(Place, Fault)>) {
    let mut told = Vec::new();
    let found = FieldFont::find(fonts, name, reading(objects), limits, &mut told);
    (found, told)
}

/// The codes `layout` encodes `text` to, character by character, `None` for a
/// character it has none for.
fn codes(layout: &FieldLayoutFont<'_>, text: &str) -> Vec<Option<Vec<u8>>> {
    text.chars()
        .map(|ch| {
            let mut out = Vec::new();
            layout.encode_char(ch, &mut out).then_some(out)
        })
        .collect()
}

/// Each `/Encoding` a `/DR` font of the standard 14 comes in encodes text to
/// its own codes, by glyph name, which pdf-fonts reads back as the same text,
/// each measured by Adobe's metrics for its glyph.
#[test]
fn dr_fonts_encode_by_their_encodings_and_measure_by_adobes_metrics() {
    let text = "é € — “ ”";
    let glyphs = [
        "eacute",
        "space",
        "Euro",
        "space",
        "emdash",
        "space",
        "quotedblleft",
        "space",
        "quotedblright",
    ];
    let fonts = dict! {
        "Helv" => id(1),
        "HeMac" => id(2),
        "HeDif" => id(3),
    };
    let mut objects = Objects::new();
    objects.insert(
        id(1),
        Object::from(std_font("Helvetica", Object::name("WinAnsiEncoding"))),
    );
    objects.insert(
        id(2),
        Object::from(std_font("Helvetica", Object::name("MacRomanEncoding"))),
    );
    objects.insert(id(3), Object::from(std_font("Helvetica", differences())));

    // ISO 32000-2, D.2: each character's code in each encoding, or none.
    let expected: [(&[u8], [Option<u8>; 9]); 3] = [
        (
            b"Helv",
            [
                Some(0xE9),
                Some(32),
                Some(0x80),
                Some(32),
                Some(0x97),
                Some(32),
                Some(0x93),
                Some(32),
                Some(0x94),
            ],
        ),
        (
            b"HeMac",
            [
                Some(0x8E),
                Some(32),
                None,
                Some(32),
                Some(0xD1),
                Some(32),
                Some(0xD2),
                Some(32),
                Some(0xD3),
            ],
        ),
        // The built-in encoding has no `é`; `/Differences` gives the rest
        // codes of their own, the lowest of which is taken.
        (
            b"HeDif",
            [
                None,
                Some(32),
                Some(0xA0),
                Some(32),
                Some(0x84),
                Some(32),
                Some(0x8D),
                Some(32),
                Some(0x8E),
            ],
        ),
    ];
    for (name, want) in expected {
        let (found, told) = find_told(Some((&fonts, None)), name, &objects, &Limits::DEFAULT);
        let font = found.unwrap();
        assert!(told.is_empty(), "{told:?}");
        assert!(!font.is_fallback());
        let layout = font.layout();
        let got = codes(&layout, text);
        let want: Vec<Option<Vec<u8>>> = want.iter().map(|c| c.map(|c| vec![c])).collect();
        assert_eq!(got, want, "{}", String::from_utf8_lossy(name));
        for ((code, ch), glyph) in got.iter().zip(text.chars()).zip(glyphs) {
            let Some(code) = code else {
                assert_eq!(layout.char_advance(ch), None);
                continue;
            };
            assert_eq!(
                font.font().text(code).as_deref(),
                Some(ch.to_string().as_str())
            );
            let width = Std14::Helvetica.glyph_width(glyph).unwrap();
            assert_eq!(layout.char_advance(ch), Some(width), "{glyph}");
            assert_eq!(font.font().width(u32::from(code[0])), width);
        }
    }
    assert_eq!(Std14::Helvetica.glyph_width("emdash"), Some(1000.0));
    assert_eq!(Std14::Helvetica.glyph_width("quotedblleft"), Some(333.0));
}

/// A font that isn't standard and isn't embedded is measured by its
/// `/Widths` and its descriptor's heights.
#[test]
fn a_described_font_measures_by_its_widths_and_descriptor() {
    let (font, descriptor) = described(800, -200, id(2));
    let mut objects = Objects::new();
    objects.insert(id(1), Object::from(font));
    objects.insert(id(2), Object::from(descriptor));
    let fonts = dict! { "Desc" => id(1) };
    let (found, told) = find_told(Some((&fonts, None)), b"Desc", &objects, &Limits::DEFAULT);
    let font = found.unwrap();
    assert!(told.is_empty(), "{told:?}");
    let layout = font.layout();
    let metrics = layout.font_metrics();
    assert_eq!((metrics.ascent, metrics.descent), (800.0, -200.0));
    for ch in ['A', 'g', ' ', '~'] {
        assert_eq!(layout.char_advance(ch), Some(500.0));
    }
    assert_eq!(codes(&layout, "Ag"), [Some(vec![b'A']), Some(vec![b'g'])]);
    // Past `/LastChar`, the code has no width given.
    assert_eq!(codes(&layout, "é"), [Some(vec![0xE9])]);
    assert_eq!(layout.char_advance('é'), Some(0.0));
}

/// A `/DA` font the default resources lack falls back by its name to a
/// standard 14 font, in the face its name says, and that is told.
#[test]
fn a_missing_font_falls_back_by_name_and_keeps_its_style() {
    let cases: &[(&str, Std14)] = &[
        // Names pdf-fonts knows.
        ("Helvetica", Std14::Helvetica),
        ("Arial", Std14::Helvetica),
        ("ArialMT", Std14::Helvetica),
        ("Arial,Bold", Std14::HelveticaBold),
        ("Times-Italic", Std14::TimesItalic),
        ("TimesNewRomanPSMT", Std14::TimesRoman),
        ("Courier", Std14::Courier),
        ("CourierNew,Bold", Std14::CourierBold),
        ("ZapfDingbats", Std14::ZapfDingbats),
        ("ABCDEF+Helvetica-Oblique", Std14::HelveticaOblique),
        // The abbreviations forms give, as written.
        ("Helv", Std14::Helvetica),
        ("HeBo", Std14::HelveticaBold),
        ("HeOb", Std14::HelveticaOblique),
        ("HeBO", Std14::HelveticaBoldOblique),
        ("Cour", Std14::Courier),
        ("CoBo", Std14::CourierBold),
        ("CoOb", Std14::CourierOblique),
        ("CoBO", Std14::CourierBoldOblique),
        ("TiRo", Std14::TimesRoman),
        ("TiBo", Std14::TimesBold),
        ("TiIt", Std14::TimesItalic),
        ("TiBI", Std14::TimesBoldItalic),
        ("Symb", Std14::Symbol),
        ("ZaDb", Std14::ZapfDingbats),
        // The family by the name's start, the face by its words.
        ("Times", Std14::TimesRoman),
        ("TimesLTStd-BoldItalic", Std14::TimesBoldItalic),
        ("Cour-Oblique", Std14::CourierOblique),
        ("courierstd-bold", Std14::CourierBold),
        ("HelveticaLTStd-Bold", Std14::HelveticaBold),
        ("HelveticaLTStd-Roman", Std14::Helvetica),
        ("helv", Std14::Helvetica),
        ("HEBO", Std14::Helvetica),
        ("ABCDEF+TimesLTStd-Roman", Std14::TimesRoman),
        ("Foo-BoldOblique", Std14::HelveticaBoldOblique),
        ("OCRAStd", Std14::Helvetica),
        ("", Std14::Helvetica),
    ];
    let objects = Objects::new();
    let dr_fonts = dict! { "Other" => id(9) };
    for &(name, face) in cases {
        for fonts in [None, Some((&dr_fonts, Some(id(4))))] {
            let (found, told) = find_told(fonts, name.as_bytes(), &objects, &Limits::DEFAULT);
            let font = found.unwrap();
            assert!(font.is_fallback(), "{name}");
            // The dictionary it will be written as names the face.
            assert_eq!(font.res(), Res::dict(face.dict()).ok(), "{name}");
            let place = Place::new(fonts.and_then(|(_, id)| id), None);
            assert_eq!(told, [(place, Fault::FontNotInResources)], "{name}");
            assert_eq!(
                font.layout().font_metrics(),
                FieldFont::standard(face).layout().font_metrics()
            );
        }
    }
    // Each fallback reads as the dictionary it will be written as: the same
    // codes' text and widths as that dictionary read on its own.
    for face in Std14::ALL {
        let fallback = FieldFont::standard(face);
        let written = face.dict();
        let limits = pdf_fonts::Limits::DEFAULT;
        let read = Font::read(reading(&objects), &written, &limits, &mut 0, &mut ()).unwrap();
        for code in 0..=255 {
            let (ours, theirs) = (fallback.font(), &read);
            assert_eq!(
                ours.code_text(code),
                theirs.code_text(code),
                "{face:?} {code}"
            );
            assert_eq!(ours.width(code), theirs.width(code), "{face:?} {code}");
        }
    }
}

/// An entry that isn't a font dictionary, or a reference to nothing, is a
/// font the resources lack.
#[test]
fn an_entry_that_isnt_a_font_dictionary_falls_back() {
    let mut objects = Objects::new();
    objects.insert(id(1), Object::from(3));
    let fonts = dict! { "Num" => 3, "Ref" => id(1), "Gone" => id(2), "Null" => Object::null() };
    for name in [&b"Num"[..], b"Ref", b"Gone", b"Null"] {
        let (found, told) = find_told(
            Some((&fonts, Some(id(7)))),
            name,
            &objects,
            &Limits::DEFAULT,
        );
        assert!(found.unwrap().is_fallback());
        assert_eq!(
            told,
            [(Place::new(Some(id(7)), None), Fault::FontNotInResources)]
        );
    }
}

/// The resource an appearance names its font by: the object, for a font held
/// by reference; the dictionary, for a direct one that holds no reference and
/// for a fallback; none for a direct one that holds a reference.
#[test]
fn res_names_what_can_be_written() {
    let (described, descriptor) = described(800, -200, id(2));
    let direct = std_font("Courier", Object::name("WinAnsiEncoding"));
    let mut objects = Objects::new();
    objects.insert(
        id(1),
        Object::from(std_font("Helvetica", Object::name("WinAnsiEncoding"))),
    );
    objects.insert(id(2), Object::from(descriptor));
    // A direct font's descriptor, by reference two levels in.
    let mut deep = direct.clone();
    deep.push(
        "Encoding",
        dict! { "Differences" => vec![Object::from(65), Object::from(id(1))] },
    );
    let fonts = dict! {
        "Ind" => id(1),
        "Direct" => direct.clone(),
        "DirectRef" => described,
        "Deep" => deep,
    };
    let find = |name: &[u8]| {
        FieldFont::find(
            Some((&fonts, None)),
            name,
            reading(&objects),
            &Limits::DEFAULT,
            &mut (),
        )
        .unwrap()
    };
    assert_eq!(find(b"Ind").res(), Some(Res::written(id(1))));
    assert_eq!(find(b"Direct").res(), Some(Res::dict(direct).unwrap()));
    assert_eq!(find(b"DirectRef").res(), None);
    assert_eq!(find(b"Deep").res(), None);
    assert!(!find(b"DirectRef").is_fallback());
    assert_eq!(
        find(b"Missing").res(),
        Some(Res::dict(Std14::Helvetica.dict()).unwrap())
    );
    assert_eq!(
        FieldFont::standard(Std14::Symbol).res(),
        Some(Res::dict(Std14::Symbol.dict()).unwrap())
    );
}

/// What a font's streams decompress to is held to `Limits::font_bytes`; a
/// stream that can't be read for the caller's reason is that error.
#[test]
fn a_font_past_font_bytes_is_a_cap() {
    let cmap =
        b"1 begincodespacerange <00> <FF> endcodespacerange 1 beginbfchar <41> <0042> endbfchar"
            .to_vec();
    let mut font = std_font("Helvetica", Object::name("WinAnsiEncoding"));
    font.push("ToUnicode", id(2));
    let mut objects = Objects::new();
    objects.insert(id(1), Object::from(font));
    objects.insert(
        id(2),
        Object::from(pdf_objects::Stream::new(Dict::new(), cmap.clone())),
    );
    let fonts = dict! { "F" => id(1) };

    let mut limits = Limits::DEFAULT;
    limits.font_bytes = cmap.len() - 1;
    let (found, _) = find_told(Some((&fonts, None)), b"F", &objects, &limits);
    assert_eq!(found.unwrap_err(), Error::Limit(Cap::FontBytes));
    assert_eq!(
        Error::Limit(Cap::FontBytes).to_string(),
        "exceeded cap on bytes a field's font decompresses to"
    );
    limits.font_bytes = cmap.len();
    let (found, _) = find_told(Some((&fonts, None)), b"F", &objects, &limits);
    let font = found.unwrap();
    assert_eq!(font.font().cost(), cmap.len());
    // The `/ToUnicode` gives `A` the text `B`.
    assert_eq!(codes(&font.layout(), "B"), [Some(vec![b'A'])]);

    let locked = WithStreams::new(&objects, |id: ObjectId, _: usize| {
        Err::<Vec<u8>, _>(Unreadable::new(id, Why::Locked, "locked"))
    });
    let err = FieldFont::find(
        Some((&fonts, None)),
        b"F",
        locked,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Unreadable(u) if u.why == Why::Locked));
}

/// The heights of the face PDFium draws each standard 14 font in, where the
/// file embeds none and no descriptor gives them; a descriptor's where it
/// does.
#[test]
fn standard_fonts_take_the_heights_pdfium_draws_them_with() {
    let heights: [(Std14, f32, f32); 14] = [
        (Std14::Courier, 803.0, -248.0),
        (Std14::CourierBold, 811.0, -248.0),
        (Std14::CourierOblique, 803.0, -248.0),
        (Std14::CourierBoldOblique, 811.0, -248.0),
        (Std14::Helvetica, 945.0, -224.0),
        (Std14::HelveticaBold, 962.0, -227.0),
        (Std14::HelveticaOblique, 951.0, -224.0),
        (Std14::HelveticaBoldOblique, 962.0, -227.0),
        (Std14::TimesRoman, 878.0, -249.0),
        (Std14::TimesBold, 889.0, -249.0),
        (Std14::TimesItalic, 868.0, -249.0),
        (Std14::TimesBoldItalic, 869.0, -249.0),
        (Std14::Symbol, 1010.0, -293.0),
        (Std14::ZapfDingbats, 826.0, -163.0),
    ];
    for (face, ascent, descent) in heights {
        let standard = FieldFont::standard(face);
        let metrics = standard.layout().font_metrics();
        assert_eq!(
            (metrics.ascent, metrics.descent),
            (ascent, descent),
            "{face:?}"
        );
        // No box: the face is the viewer's.
        assert_eq!(metrics.bbox, None);
        // The font as read keeps Adobe's metrics; only the layout's differ.
        assert_eq!(standard.font().metrics(), face.metrics());

        // The same font in the default resources.
        let fonts = dict! { "F" => Object::from(face.dict()) };
        let found = FieldFont::find(
            Some((&fonts, None)),
            b"F",
            reading(&Objects::new()),
            &Limits::DEFAULT,
            &mut (),
        )
        .unwrap();
        assert_eq!(found.layout().font_metrics(), metrics, "{face:?}");
    }

    // A descriptor with a box and heights gives them; one without a box
    // leaves PDFium its face's, as does one whose heights are both 0.
    let descriptor = |bbox: bool, ascent: i64, descent: i64| {
        let mut d = dict! { "Type" => Object::name("FontDescriptor"), "Ascent" => ascent, "Descent" => descent };
        if bbox {
            d.push(
                "FontBBox",
                vec![
                    Object::from(-100),
                    Object::from(-300),
                    Object::from(1000),
                    Object::from(900),
                ],
            );
        }
        d
    };
    for (desc, want) in [
        (descriptor(true, 700, -300), (700.0, -300.0)),
        (descriptor(false, 700, -300), (878.0, -249.0)),
        (descriptor(true, 0, 0), (878.0, -249.0)),
    ] {
        let mut font = std_font("Times-Roman", Object::name("WinAnsiEncoding"));
        font.push("FontDescriptor", desc);
        let fonts = dict! { "F" => Object::from(font) };
        let found = FieldFont::find(
            Some((&fonts, None)),
            b"F",
            reading(&Objects::new()),
            &Limits::DEFAULT,
            &mut (),
        )
        .unwrap();
        let metrics = found.layout().font_metrics();
        assert_eq!((metrics.ascent, metrics.descent), want);
    }

    // A `/BaseFont` written by reference names the face all the same.
    let mut objects = Objects::new();
    objects.insert(id(1), Object::name("Courier-Bold"));
    let mut font = std_font("Courier-Bold", Object::name("WinAnsiEncoding"));
    font.set("BaseFont", id(1));
    let fonts = dict! { "F" => Object::from(font) };
    let found = FieldFont::find(
        Some((&fonts, None)),
        b"F",
        reading(&objects),
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap();
    let metrics = found.layout().font_metrics();
    assert_eq!((metrics.ascent, metrics.descent), (811.0, -248.0));

    // A Type 3 font draws in no face, whatever its name.
    let type3 = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("Type3"),
        "BaseFont" => Object::name("Helvetica"),
        "FontMatrix" => vec![
            Object::from(0.001),
            Object::from(0),
            Object::from(0),
            Object::from(0.001),
            Object::from(0),
            Object::from(0),
        ],
        "CharProcs" => Dict::new(),
        "Encoding" => dict! { "Differences" => vec![Object::from(32), Object::name("space")] },
        "FirstChar" => 32,
        "LastChar" => 32,
        "Widths" => vec![Object::from(250)],
    };
    let fonts = dict! { "F" => Object::from(type3) };
    let found = FieldFont::find(
        Some((&fonts, None)),
        b"F",
        reading(&Objects::new()),
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap();
    assert!(found.font().approx().heights);
    assert_eq!(found.layout().font_metrics(), found.font().metrics());
    assert_ne!(found.layout().font_metrics().ascent, 945.0);

    // A font that isn't standard keeps what pdf-fonts reads.
    let fonts = dict! { "F" => Object::from(std_font("Foo", Object::name("WinAnsiEncoding"))) };
    let found = FieldFont::find(
        Some((&fonts, None)),
        b"F",
        reading(&Objects::new()),
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap();
    assert_eq!(found.layout().font_metrics(), found.font().metrics());
}

/// The form's own default resources, through `Form::field_font`.
#[test]
fn a_forms_field_font_is_found_in_its_default_resources() {
    let mut objects = Objects::new();
    objects.insert(
        id(1),
        Object::from(std_font("Courier", Object::name("WinAnsiEncoding"))),
    );
    let catalog = dict! {
        "AcroForm" => dict! {
            "Fields" => Vec::<Object>::new(),
            "DR" => dict! { "Font" => dict! { "Cour" => id(1) } },
        },
    };
    let form = Form::read(&objects, &catalog, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap()
        .unwrap();
    let mut told: Vec<(Place, Fault)> = Vec::new();
    let found = form
        .field_font(reading(&objects), b"Cour", &Limits::DEFAULT, &mut told)
        .unwrap();
    assert_eq!(found.res(), Some(Res::written(id(1))));
    assert_eq!(found.layout().char_advance('i'), Some(600.0));
    assert!(told.is_empty());
    let missing = form
        .field_font(reading(&objects), b"TiRo", &Limits::DEFAULT, &mut told)
        .unwrap();
    assert_eq!(
        missing.res(),
        Some(Res::dict(Std14::TimesRoman.dict()).unwrap())
    );
    assert_eq!(told, [(Place::new(None, None), Fault::FontNotInResources)]);
}

/// A sink that wants no faults, and panics if told one.
struct Unwanted;

impl pdf_objects::fault::Sink<Fault> for Unwanted {
    fn fault(&mut self, _: Place, fault: Fault) {
        panic!("told {fault:?} though it wants none");
    }

    fn wants(&self) -> bool {
        false
    }
}

/// The font read is the same whatever the sink, and a sink that wants none
/// is told none; a `Vec` is told the font's departures, placed in the font's
/// object.
#[test]
fn the_same_font_whatever_the_sink() {
    let mut objects = Objects::new();
    // Not standard, and no `/Widths`.
    objects.insert(
        id(1),
        Object::from(std_font("Foo", Object::name("WinAnsiEncoding"))),
    );
    let (described, descriptor) = described(800, -200, id(3));
    objects.insert(id(2), Object::from(described));
    objects.insert(id(3), Object::from(descriptor));
    let fonts = dict! {
        "Bare" => id(1),
        "Desc" => id(2),
        "Std" => Object::from(Std14::TimesBold.dict()),
    };
    let text = "Hello é € — “ ” 中";
    for name in [&b"Bare"[..], b"Desc", b"Std", b"HeBo"] {
        let quiet = FieldFont::find(
            Some((&fonts, Some(id(9)))),
            name,
            reading(&objects),
            &Limits::DEFAULT,
            &mut (),
        )
        .unwrap();
        let unwanted = FieldFont::find(
            Some((&fonts, Some(id(9)))),
            name,
            reading(&objects),
            &Limits::DEFAULT,
            &mut Unwanted,
        )
        .unwrap();
        assert_eq!(format!("{unwanted:?}"), format!("{quiet:?}"));
        let (told, faults) = find_told(
            Some((&fonts, Some(id(9)))),
            name,
            &objects,
            &Limits::DEFAULT,
        );
        let told = told.unwrap();
        assert_eq!(format!("{quiet:?}"), format!("{told:?}"));
        assert_eq!(quiet.res(), told.res());
        assert_eq!(quiet.is_fallback(), told.is_fallback());
        let (quiet, told) = (quiet.layout(), told.layout());
        assert_eq!(quiet.font_metrics(), told.font_metrics());
        assert_eq!(codes(&quiet, text), codes(&told, text));
        for ch in text.chars() {
            assert_eq!(quiet.char_advance(ch), told.char_advance(ch));
        }
        match name {
            b"Bare" => assert_eq!(
                faults,
                [(
                    Place::new(Some(id(1)), None),
                    Fault::Font(pdf_fonts::Fault::NoWidths)
                )]
            ),
            b"HeBo" => assert_eq!(
                faults,
                [(Place::new(Some(id(9)), None), Fault::FontNotInResources)]
            ),
            _ => assert!(faults.is_empty(), "{faults:?}"),
        }
    }
}

/// `Debug` of a font, its layout, and what's told prints neither the font's
/// name nor the name a `/DA` string gave.
#[test]
fn debug_prints_no_name() {
    let mut objects = Objects::new();
    objects.insert(
        id(1),
        Object::from(std_font("SECRETFONT", Object::name("WinAnsiEncoding"))),
    );
    let fonts = dict! { "SECRETNAME" => id(1) };
    let (found, mut told) = find_told(
        Some((&fonts, None)),
        b"SECRETNAME",
        &objects,
        &Limits::DEFAULT,
    );
    let found = found.unwrap();
    let (fallback, more) = find_told(
        Some((&fonts, None)),
        b"SECRETBold",
        &objects,
        &Limits::DEFAULT,
    );
    let fallback = fallback.unwrap();
    told.extend(more);
    let shown = format!(
        "{found:?} {:?} {fallback:?} {:?} {told:?} {}",
        found.layout(),
        fallback.layout(),
        told.iter()
            .map(|(_, fault)| fault.to_string())
            .collect::<String>(),
    );
    assert!(!shown.contains("SECRET"), "{shown}");
    assert!(shown.contains("fallback: true"), "{shown}");

    // The variants this module and the makers tell, each as it reads.
    let said = |fault: Fault| (format!("{fault:?}"), fault.to_string());
    assert_eq!(
        said(Fault::from(pdf_gfx::Fault::DefaultAppearanceNoFont)),
        (
            "DefaultAppearance(DefaultAppearanceNoFont)".into(),
            "a default appearance string departs: DefaultAppearanceNoFont".into()
        )
    );
    assert_eq!(
        said(Fault::MissingDefaultAppearance),
        (
            "MissingDefaultAppearance".into(),
            "a text field has no default appearance with a font".into()
        )
    );
    assert_eq!(
        said(Fault::FontNotInResources),
        (
            "FontNotInResources".into(),
            "the default resources have no font by the name /DA gives".into()
        )
    );
    assert_eq!(
        said(Fault::from(pdf_fonts::Fault::NoWidths)),
        (
            "Font(NoWidths)".into(),
            "a simple font with no /Widths".into()
        )
    );
    assert_eq!(
        Error::Limit(Cap::Text).to_string(),
        "exceeded cap on bytes of an appearance's text, or a comb field's cells"
    );
}

#[test]
fn field_fonts_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<FieldFont>();
    check::<FieldLayoutFont<'static>>();
    check::<Font>();
}

/// A box of `width` by `height` with no border.
fn bare(width: f32, height: f32) -> WidgetLook<'static> {
    let mut look = WidgetLook::new(width, height);
    look.border = BoxBorder::solid(0.0);
    look
}

fn helvetica() -> FieldFont {
    FieldFont::standard(Std14::Helvetica)
}

/// `field` made in Helvetica in a 100 by 20 box with no border, at 10
/// points in black.
fn made(field: &TextField<'_>) -> Result<Appearance, Unmade> {
    let font = helvetica();
    text(
        field,
        &bare(100.0, 20.0),
        &DefaultAppearance::new("Helv", 10.0, Some(Color::Gray(0.0))),
        font.res().unwrap(),
        &font.layout(),
        &Limits::DEFAULT,
    )
}

/// The strings an appearance's content shows, each `(...)` as written.
fn strings(content: &[u8]) -> Vec<&[u8]> {
    let mut found = Vec::new();
    let mut rest = content;
    while let Some(open) = rest.iter().position(|&b| b == b'(') {
        let close = open + rest[open..].iter().position(|&b| b == b')').unwrap();
        found.push(&rest[open + 1..close]);
        rest = &rest[close + 1..];
    }
    found
}

/// A character the font has no code for is refused at its byte offset in
/// the text the caller gave, whatever was dropped before the layout saw
/// it: a single line's line breaks, a comb field's characters past its
/// cells. A multiline field keeps its line breaks.
#[test]
fn unencodable_is_told_at_its_offset_in_the_callers_text() {
    let refused = |field: &TextField<'_>| match made(field) {
        Err(Unmade::Unencodable(c)) => Some((c.at, c.character)),
        Ok(_) => None,
        Err(other) => panic!("{other:?}"),
    };
    let line = "ab\r\ncd\n中";
    assert_eq!(
        refused(&TextField::new(line)),
        Some((line.find('中').unwrap(), '中'))
    );
    let mut multiline = TextField::new("\n\r中");
    multiline.flags = FieldFlags::MULTILINE;
    assert_eq!(refused(&multiline), Some((2, '中')));

    let mut comb = TextField::new("1\r\n中");
    comb.flags = FieldFlags::COMB;
    comb.max_len = Some(5);
    assert_eq!(refused(&comb), Some((3, '中')));
    // Past the cells: cut before it's drawn, so not refused.
    comb.text = "12\r\n34567中";
    let ap = made(&comb).unwrap();
    assert!(ap.overflowed);
    assert!(strings(&ap.content).iter().all(|s| s.len() == 1));
    assert_eq!(strings(&ap.content).len(), 5);
    comb.text = "12345";
    assert!(!made(&comb).unwrap().overflowed);

    // The error says the same in each form.
    let err = made(&TextField::new("a€中")).unwrap_err();
    assert_eq!(
        err,
        Unmade::Unencodable(UnencodableChar {
            at: 4,
            character: '中'
        })
    );
    assert_eq!(
        err.to_string(),
        "the font has no code for '中', at byte 4 of the text"
    );
}

/// A password field draws `*` for each character, so its value reaches
/// neither the font, the content, an error, nor any `Debug`.
#[test]
fn a_passwords_value_reaches_nothing() {
    let value = "hunter2中\u{1F600}";
    let mut field = TextField::new(value);
    field.flags = FieldFlags::PASSWORD;
    // The font has no code for two of its characters, and isn't asked.
    let ap = made(&field).unwrap();
    let shown = strings(&ap.content);
    assert_eq!(shown, [&b"*********"[..]]);
    assert!(!ap.content.windows(4).any(|w| w == b"hunt"));
    for hidden in [
        format!("{ap:?}"),
        format!("{field:?}"),
        format!("{:?}", made(&field)),
    ] {
        assert!(!hidden.contains("hunter"), "{hidden}");
    }

    // In a font with no `*`, `*` is refused, at the text's start, whatever
    // came before it.
    let dingbats = FieldFont::standard(Std14::ZapfDingbats);
    let mut after_a_break = field;
    after_a_break.text = "\r\nhunter2";
    let err = text(
        &after_a_break,
        &bare(100.0, 20.0),
        &DefaultAppearance::new("ZaDb", 10.0, None),
        dingbats.res().unwrap(),
        &dingbats.layout(),
        &Limits::DEFAULT,
    )
    .unwrap_err();
    assert_eq!(
        err,
        Unmade::Unencodable(UnencodableChar {
            at: 0,
            character: '*'
        })
    );
    assert!(!format!("{err:?} {err}").contains("hunter"));

    // A multiline password keeps its lines, each character starred.
    field.flags = FieldFlags::PASSWORD | FieldFlags::MULTILINE;
    field.text = "ab\ncd";
    let ap = made(&field).unwrap();
    assert_eq!(strings(&ap.content), [&b"**"[..], &b"**"[..]]);

    // Read through the convenience, a password field's value isn't in what's
    // read either.
    let mut objects = Objects::new();
    objects.insert(
        id(1),
        Object::from(dict! {
            "T" => Object::text("pin"),
            "FT" => Object::name("Tx"),
            "Ff" => 8192,
            "V" => Object::text(value),
            "DA" => Object::text("/Helv 0 Tf 0 g"),
            "Subtype" => Object::name("Widget"),
            "Rect" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(20)],
        }),
    );
    let catalog = dict! { "AcroForm" => dict! { "Fields" => vec![Object::from(id(1))] } };
    let form = Form::read(&objects, &catalog, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap()
        .unwrap();
    let field = &form.fields()[0];
    let read = widget_text(
        field,
        &field.widgets()[0],
        reading(&objects),
        Rules::Pdfium,
        None,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap();
    let made = for_widget(
        &form,
        field,
        &field.widgets()[0],
        reading(&objects),
        Rules::Pdfium,
        None,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap();
    assert_eq!(strings(&made.appearance.content), [&b"*********"[..]]);
    let shown = format!("{read:?} {made:?}");
    assert!(!shown.contains("hunter"), "{shown}");
}

/// Colors are written with the digits they're given: the navy forms use,
/// `0 0 0.502 rg`, and gray and CMYK, for the text, the background, and the
/// border.
#[test]
fn colors_are_written_with_their_digits() {
    let font = helvetica();
    let mut look = WidgetLook::new(100.0, 20.0);
    look.background = Some(Color::Cmyk([0.1, 0.2, 0.3, 0.4]));
    look.border_color = Some(Color::Gray(0.25));
    let ap = text(
        &TextField::new("Hello"),
        &look,
        &DefaultAppearance::new("Helv", 10.0, Some(Color::Rgb([0.0, 0.0, 0.502]))),
        font.res().unwrap(),
        &font.layout(),
        &Limits::DEFAULT,
    )
    .unwrap();
    let content = String::from_utf8_lossy(&ap.content);
    for written in ["0 0 0.502 rg", "0.1 0.2 0.3 0.4 k", "0.25 G"] {
        assert!(content.contains(written), "{written} in {content}");
    }
    // No color in `/DA`: black.
    let ap = text(
        &TextField::new("Hello"),
        &bare(100.0, 20.0),
        &DefaultAppearance::new("Helv", 10.0, None),
        font.res().unwrap(),
        &font.layout(),
        &Limits::DEFAULT,
    )
    .unwrap();
    let content = String::from_utf8_lossy(&ap.content);
    assert!(content.contains("n\n0 g\nBT"), "{content}");
}

/// An empty text writes `/Tx BMC EMC` with nothing between, and needs no
/// font.
#[test]
fn an_empty_text_is_an_empty_tx() {
    let ap = made(&TextField::new("")).unwrap();
    let content = String::from_utf8_lossy(&ap.content);
    assert!(content.ends_with("/Tx BMC\nEMC\n"), "{content}");
    assert!(ap.needs.0.is_empty());
    assert_eq!(ap.font_size, 10.0);
    // A single line of line breaks alone shows nothing too.
    let ap = made(&TextField::new("\r\n")).unwrap();
    assert!(String::from_utf8_lossy(&ap.content).ends_with("/Tx BMC\nEMC\n"));
}

/// A box with no width or no height, or one that isn't finite, is no box;
/// a number of the look or of the default appearance that isn't finite,
/// or a size or box at which the text's place isn't, or reaches past
/// `1e35`, is refused; text past `Limits::text` is a cap.
#[test]
fn unmade_for_no_box_numbers_not_finite_and_the_text_cap() {
    let font = helvetica();
    let aligned = |quadding, look: &WidgetLook<'_>, da: &DefaultAppearance, limits: &Limits| {
        let mut field = TextField::new("Hello");
        field.quadding = quadding;
        text(
            &field,
            look,
            da,
            font.res().unwrap(),
            &font.layout(),
            limits,
        )
    };
    let make = |look: &WidgetLook<'_>, da: &DefaultAppearance, limits: &Limits| {
        aligned(pdf_annots::Quadding::Right, look, da, limits)
    };
    let da = DefaultAppearance::new("Helv", 10.0, None);
    for (w, h) in [
        (0.0, 20.0),
        (100.0, 0.0),
        (-100.0, 20.0),
        (f32::NAN, 20.0),
        (100.0, f32::INFINITY),
    ] {
        assert_eq!(
            make(&WidgetLook::new(w, h), &da, &Limits::DEFAULT),
            Err(Unmade::NoBox),
            "{w} {h}"
        );
    }

    let mut looks = Vec::new();
    let mut look = WidgetLook::new(100.0, 20.0);
    look.border.width = f32::NAN;
    looks.push(look);
    let mut look = WidgetLook::new(100.0, 20.0);
    look.background = Some(Color::Gray(f32::NAN));
    looks.push(look);
    let mut look = WidgetLook::new(100.0, 20.0);
    look.border_color = Some(Color::Rgb([0.0, f32::INFINITY, 0.0]));
    looks.push(look);
    // Borders so much wider than the box that the box inside them is past
    // finite: twice the largest `f32`, and a beveled border's twice its width.
    let mut look = WidgetLook::new(100.0, 20.0);
    look.border = BoxBorder::solid(f32::MAX);
    looks.push(look);
    let mut look = WidgetLook::new(100.0, 20.0);
    look.border = BoxBorder::new(2.0e38, BoxBorderStyle::Beveled);
    looks.push(look);
    for look in &looks {
        assert_eq!(
            make(look, &da, &Limits::DEFAULT),
            Err(Unmade::NotFinite),
            "{look:?}"
        );
    }
    // One whose box inside it is huge but finite is made, its text centered
    // near the box's origin; right-aligned, `1e38` out, the text reaches too
    // far to be placed.
    let mut look = WidgetLook::new(100.0, 20.0);
    look.border = BoxBorder::solid(1.0e38);
    let centered = aligned(pdf_annots::Quadding::Center, &look, &da, &Limits::DEFAULT);
    assert!(centered.is_ok());
    assert_eq!(make(&look, &da, &Limits::DEFAULT), Err(Unmade::NotFinite));
    let look = WidgetLook::new(100.0, 20.0);
    let mut nan_size = da.clone();
    nan_size.font_size = Some(f32::NAN);
    let mut nan_color = da.clone();
    nan_color.color = Some(Color::Cmyk([0.0, 0.0, f32::NAN, 1.0]));
    // So large the line's width and its place aren't finite; and so large
    // that, every number written finite, its five characters reach past
    // `1e35`, as a size of the default appearance may be.
    let huge = DefaultAppearance::new("Helv", 3.0e38, None);
    let far = DefaultAppearance::new("Helv", 1.0e36, None);
    for da in [nan_size, nan_color, huge, far] {
        assert_eq!(
            make(&look, &da, &Limits::DEFAULT),
            Err(Unmade::NotFinite),
            "{da:?}"
        );
    }
    // One huge but within it is made.
    let near = DefaultAppearance::new("Helv", 1.0e33, None);
    assert!(make(&look, &near, &Limits::DEFAULT).is_ok());

    let mut limits = Limits::DEFAULT;
    limits.text = 4;
    assert_eq!(make(&look, &da, &limits), Err(Unmade::Limit(Cap::Text)));
    limits.text = 5;
    assert!(make(&look, &da, &limits).is_ok());

    for (unmade, said) in [
        (Unmade::NoBox, "the widget's box has no width or no height"),
        (
            Unmade::NotFinite,
            "a number of the widget's look or of its default appearance isn't finite, \
             or sets the text too far out to be placed",
        ),
        (
            Unmade::Limit(Cap::Text),
            "exceeded cap on bytes of an appearance's text, or a comb field's cells",
        ),
        (Unmade::NotText, "the field isn't a text field"),
        (
            Unmade::FontNotBindable,
            "the field's font can't be named by the appearance's resources",
        ),
    ] {
        assert_eq!(unmade.to_string(), said);
        assert!(std::error::Error::source(&unmade).is_none());
    }
    let unreadable = Unmade::from(Error::Unreadable(Unreadable::new(
        id(3),
        Why::Locked,
        "locked",
    )));
    assert!(std::error::Error::source(&unreadable).is_none());
    assert_eq!(
        Unmade::from(Error::Limit(Cap::FontBytes)),
        Unmade::Limit(Cap::FontBytes)
    );
}

/// A standard 14 Type 1 font that embeds no program has no box of its own,
/// so comb text in it takes its size from PDFium's table; any other font
/// keeps its box.
#[test]
fn a_standard_font_has_no_box_unless_it_embeds_a_program() {
    assert_eq!(helvetica().layout().font_metrics().bbox, None);
    let found = |fonts: &Dict, objects: &Objects| {
        FieldFont::find(
            Some((fonts, None)),
            b"F",
            reading(objects),
            &Limits::DEFAULT,
            &mut (),
        )
        .unwrap()
        .layout()
        .font_metrics()
        .bbox
    };
    let mut objects = Objects::new();
    let fonts = dict! { "F" => Object::from(Std14::Courier.dict()) };
    assert_eq!(found(&fonts, &objects), None);

    // Named as a standard font but embedding a program (here one that can't
    // be read): its descriptor's box.
    objects.insert(
        id(2),
        Object::from(Stream::new(Dict::new(), b"not a program".to_vec())),
    );
    let descriptor = dict! {
        "Type" => Object::name("FontDescriptor"),
        "FontBBox" => vec![Object::from(0), Object::from(-100), Object::from(500), Object::from(900)],
        "Ascent" => 900,
        "Descent" => -100,
        "FontFile" => id(2),
    };
    let mut font = std_font("Helvetica", Object::name("WinAnsiEncoding"));
    font.push("FontDescriptor", descriptor);
    let fonts = dict! { "F" => Object::from(font) };
    assert_eq!(
        found(&fonts, &objects),
        Some(GlyphBox::new(0.0, -100.0, 500.0, 900.0))
    );

    // A TrueType font named Arial isn't a standard font to PDFium.
    let mut arial = std_font("Arial", Object::name("WinAnsiEncoding"));
    arial.set("Subtype", Object::name("TrueType"));
    let fonts = dict! { "F" => Object::from(arial) };
    assert!(found(&fonts, &objects).is_some());

    // A font that isn't standard: its descriptor's box.
    let (described, descriptor) = described(800, -200, id(4));
    objects.insert(id(4), Object::from(descriptor));
    let fonts = dict! { "F" => Object::from(described) };
    assert_eq!(
        found(&fonts, &objects),
        Some(GlyphBox::new(0.0, -200.0, 1000.0, 800.0))
    );
}

/// A form of text fields, each its own widget with `entries` and a box of
/// 100 by 20, whose default resources hold `/F1`, object 1.
fn text_fields(entries: Vec<Dict>, f1: Dict) -> (Objects, Dict) {
    let mut objects = Objects::new();
    objects.insert(id(1), Object::from(f1));
    let mut fields = Vec::new();
    for (i, mut field) in entries.into_iter().enumerate() {
        let n = 10 + u32::try_from(i).unwrap();
        field.set("T", Object::text(&format!("f{i}")));
        field.set("Subtype", Object::name("Widget"));
        field.set(
            "Rect",
            vec![
                Object::from(0),
                Object::from(0),
                Object::from(100),
                Object::from(20),
            ],
        );
        if field.get("FT").is_none() {
            field.set("FT", Object::name("Tx"));
        }
        objects.insert(id(n), Object::from(field));
        fields.push(Object::from(id(n)));
    }
    let catalog = dict! {
        "AcroForm" => dict! {
            "Fields" => fields,
            "DR" => dict! { "Font" => dict! { "F1" => id(1) } },
        },
    };
    (objects, catalog)
}

/// The convenience reads the widget's own `/DA` else the field's, parses it
/// once and tells its departures once, in the object it was read from; a
/// text stream's value is its text, any other value none, told; a field
/// that isn't a text field is refused.
#[test]
fn the_convenience_reads_its_widget_and_tells_once() {
    let f1 = std_font("Courier", Object::name("WinAnsiEncoding"));
    let mut objects = Objects::new();
    objects.insert(id(1), Object::from(f1));
    objects.insert(
        id(2),
        Object::from(Stream::new(Dict::new(), b"streamed".to_vec())),
    );
    objects.insert(
        id(3),
        Object::from(Stream::new(Dict::new(), vec![b'x'; 64])),
    );
    // A parent with two widgets, one with a `/DA` and `/Q` of its own that
    // passes over an operand; a field with no `/DA` anywhere; one whose
    // `/DA` has no `Tf`; values a stream, one past the cap, a name; a button.
    let rect = || {
        vec![
            Object::from(0),
            Object::from(0),
            Object::from(100),
            Object::from(20),
        ]
    };
    objects.insert(
        id(10),
        Object::from(dict! {
            "T" => Object::text("parent"),
            "FT" => Object::name("Tx"),
            "V" => Object::text("Hello"),
            "DA" => Object::text("/F1 10 Tf 0 g"),
            "Kids" => vec![Object::from(id(11)), Object::from(id(12))],
        }),
    );
    objects.insert(
        id(11),
        Object::from(
            dict! { "Parent" => id(10), "Subtype" => Object::name("Widget"), "Rect" => rect() },
        ),
    );
    objects.insert(
        id(12),
        Object::from(dict! {
            "Parent" => id(10),
            "Subtype" => Object::name("Widget"),
            "Rect" => rect(),
            "DA" => Object::text("/F1 12 Tf 9 Tr 1 0 0 rg"),
            "Q" => 2,
        }),
    );
    let widget = |n: u32, entries: Dict| {
        let mut field = entries;
        field.set("T", Object::text(&format!("f{n}")));
        field.set("Subtype", Object::name("Widget"));
        field.set("Rect", rect());
        field
    };
    objects.insert(
        id(13),
        Object::from(widget(
            13,
            dict! { "FT" => Object::name("Tx"), "V" => Object::text("x") },
        )),
    );
    objects.insert(
        id(14),
        Object::from(widget(14, dict! { "FT" => Object::name("Tx"), "V" => Object::text("x"), "DA" => Object::text("0 0 1 rg") })),
    );
    objects.insert(
        id(15),
        Object::from(widget(
            15,
            dict! { "FT" => Object::name("Tx"), "V" => id(2), "DA" => Object::text("/F1 0 Tf") },
        )),
    );
    objects.insert(
        id(16),
        Object::from(widget(
            16,
            dict! { "FT" => Object::name("Tx"), "V" => id(3), "DA" => Object::text("/F1 0 Tf") },
        )),
    );
    objects.insert(
        id(17),
        Object::from(widget(17, dict! { "FT" => Object::name("Tx"), "V" => Object::name("Off"), "DA" => Object::text("/F1 0 Tf") })),
    );
    objects.insert(
        id(18),
        Object::from(widget(18, dict! { "FT" => Object::name("Btn") })),
    );
    let fields: Vec<Object> = (10..=18)
        .filter(|n| ![11, 12].contains(n))
        .map(|n| Object::from(id(n)))
        .collect();
    let catalog = dict! {
        "AcroForm" => dict! {
            "Fields" => fields,
            "DR" => dict! { "Font" => dict! { "F1" => id(1) } },
        },
    };
    let form = Form::read(&objects, &catalog, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap()
        .unwrap();
    let mut limits = Limits::DEFAULT;
    limits.text = 32;
    let read = |at: usize, widget: usize, told: &mut Vec<(Place, Fault)>| {
        let field = &form.fields()[at];
        widget_text(
            field,
            &field.widgets()[widget],
            reading(&objects),
            Rules::Pdfium,
            None,
            &limits,
            told,
        )
    };

    // The parent's own widget takes the parent's `/DA` and `/Q`.
    let mut told = Vec::new();
    let plain = read(0, 0, &mut told).unwrap();
    assert!(told.is_empty(), "{told:?}");
    assert_eq!(plain.font_name(), Some(&b"F1"[..]));
    assert_eq!(plain.default_appearance().font_size, Some(10.0));
    assert_eq!(plain.field().quadding, pdf_annots::Quadding::Left);
    assert_eq!(plain.field().text, "Hello");
    // The other its own, its departure told once, in the widget's object.
    let own = read(0, 1, &mut told).unwrap();
    assert_eq!(own.default_appearance().font_size, Some(12.0));
    assert_eq!(
        own.default_appearance().color,
        Some(Color::Rgb([1.0, 0.0, 0.0]))
    );
    assert_eq!(own.field().quadding, pdf_annots::Quadding::Right);
    assert_eq!(
        told,
        [(
            Place::new(Some(id(12)), None),
            Fault::DefaultAppearance(pdf_gfx::Fault::DefaultAppearanceTextMode { at: 12 })
        )]
    );

    // No `/DA`, or one with no `Tf`: Helvetica, told once, at the field.
    for (at, color) in [(1, None), (2, Some(Color::Rgb([0.0, 0.0, 1.0])))] {
        let mut told = Vec::new();
        let read = read(at, 0, &mut told).unwrap();
        assert_eq!(read.font_name(), None);
        assert_eq!(read.default_appearance().color, color);
        let id = form.fields()[at].id();
        assert_eq!(
            told,
            [(Place::new(id, None), Fault::MissingDefaultAppearance)]
        );
        let mut told = Vec::new();
        let field = &form.fields()[at];
        let made = for_widget(
            &form,
            field,
            &field.widgets()[0],
            reading(&objects),
            Rules::Pdfium,
            None,
            &limits,
            &mut told,
        )
        .unwrap();
        assert!(made.font.is_fallback());
        assert_eq!(made.font.res(), helvetica().res());
        assert_eq!(made.appearance.font_size, 14.0);
        assert_eq!(
            told,
            [(Place::new(id, None), Fault::MissingDefaultAppearance)]
        );
    }

    // A text stream's value; one past `Limits::text`; a name.
    let mut told = Vec::new();
    assert_eq!(read(3, 0, &mut told).unwrap().field().text, "streamed");
    assert!(told.is_empty());
    assert_eq!(read(4, 0, &mut told).unwrap_err(), Unmade::Limit(Cap::Text));
    let named = read(5, 0, &mut told).unwrap();
    assert_eq!(named.field().text, "");
    assert_eq!(
        told,
        [(
            Place::new(Some(id(17)), None),
            Fault::WrongType { key: "V" }
        )]
    );
    // A button.
    assert_eq!(read(6, 0, &mut Vec::new()).unwrap_err(), Unmade::NotText);

    // What's read is what `text` makes from it.
    let font = form
        .field_font(reading(&objects), b"F1", &limits, &mut ())
        .unwrap();
    let field = &form.fields()[0];
    let by_hand = text(
        &own.field(),
        &own.look(),
        own.default_appearance(),
        font.res().unwrap(),
        &font.layout(),
        &limits,
    )
    .unwrap();
    let made = for_widget(
        &form,
        field,
        &field.widgets()[1],
        reading(&objects),
        Rules::Pdfium,
        None,
        &limits,
        &mut (),
    )
    .unwrap();
    assert_eq!(made.appearance, by_hand);
    assert_eq!(
        own.make(font.res().unwrap(), &font.layout(), &limits)
            .unwrap(),
        by_hand
    );
    // Shown otherwise than the value.
    let shown = widget_text(
        field,
        &field.widgets()[0],
        reading(&objects),
        Rules::Pdfium,
        Some("$1.00"),
        &limits,
        &mut (),
    )
    .unwrap();
    assert!(matches!(shown.field().text, "$1.00"));

    // A direct font of the default resources that holds a reference can't
    // be named by the appearance: nothing is made.
    let mut objects = Objects::new();
    objects.insert(
        id(2),
        Object::from(dict! { "Type" => Object::name("FontDescriptor") }),
    );
    let mut direct = std_font("Courier", Object::name("WinAnsiEncoding"));
    direct.set("FontDescriptor", id(2));
    objects.insert(
        id(10),
        Object::from(dict! {
            "T" => Object::text("f"),
            "FT" => Object::name("Tx"),
            "V" => Object::text("x"),
            "DA" => Object::text("/Direct 0 Tf"),
            "Subtype" => Object::name("Widget"),
            "Rect" => rect(),
        }),
    );
    let catalog = dict! {
        "AcroForm" => dict! {
            "Fields" => vec![Object::from(id(10))],
            "DR" => dict! { "Font" => dict! { "Direct" => Object::from(direct) } },
        },
    };
    let form = Form::read(&objects, &catalog, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap()
        .unwrap();
    let field = &form.fields()[0];
    let unmade = for_widget(
        &form,
        field,
        &field.widgets()[0],
        reading(&objects),
        Rules::Pdfium,
        None,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap_err();
    assert_eq!(unmade, Unmade::FontNotBindable);
}

/// `Debug` of what's made, what it's made from, and what's read prints
/// lengths and numbers, never the text.
#[test]
fn debug_prints_no_text() {
    let field = TextField::new("SECRET value");
    let ap = made(&field).unwrap();
    let mut objects = Objects::new();
    objects.insert(
        id(1),
        Object::from(dict! {
            "T" => Object::text("f"),
            "FT" => Object::name("Tx"),
            "V" => Object::text("SECRET value"),
            "DA" => Object::text("/SECRETFONT 0 Tf"),
            "Subtype" => Object::name("Widget"),
            "Rect" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(20)],
        }),
    );
    let catalog = dict! { "AcroForm" => dict! { "Fields" => vec![Object::from(id(1))] } };
    let form = Form::read(&objects, &catalog, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap()
        .unwrap();
    let form_field = &form.fields()[0];
    let widget = &form_field.widgets()[0];
    let read = widget_text(
        form_field,
        widget,
        reading(&objects),
        Rules::Pdfium,
        None,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap();
    let made = for_widget(
        &form,
        form_field,
        widget,
        reading(&objects),
        Rules::Pdfium,
        None,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap();
    let shown = format!("{field:?} {ap:?} {read:?} {made:?} {:?}", read.look());
    assert!(!shown.contains("SECRET"), "{shown}");
    assert!(shown.contains("text_len: 12"), "{shown}");
    assert!(
        shown.contains(&format!("content_len: {}", ap.content.len())),
        "{shown}"
    );
}

/// Objects whose font, object 1, is counted each time it's resolved.
#[derive(Clone, Copy)]
struct Counting<'a> {
    objects: &'a Objects,
    reads: &'a Cell<usize>,
}

impl<'a> Resolve<'a> for Counting<'a> {
    fn resolve(&self, at: ObjectId) -> Option<&'a Object> {
        if at == id(1) {
            self.reads.set(self.reads.get() + 1);
        }
        self.objects.get(&at)
    }
}

impl Streams for Counting<'_> {
    fn decoded(&self, at: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        reading(self.objects)
            .decoded(at, limit)
            .map(|d| Cow::Owned(d.into_owned()))
    }
}

/// A thousand widgets read with `widget_text` and made with `make` in one
/// `FieldFont`, read once, where `for_widget` reads the font for each.
#[test]
fn a_thousand_widgets_read_their_font_once() {
    let fields = (0..1000)
        .map(|i| dict! { "V" => Object::text(&format!("{i}")), "DA" => Object::text("/F1 0 Tf 0 g") })
        .collect();
    let (objects, catalog) = text_fields(
        fields,
        std_font("Times-Roman", Object::name("WinAnsiEncoding")),
    );
    let reads = Cell::new(0);
    let counting = Counting {
        objects: &objects,
        reads: &reads,
    };
    let form = Form::read(counting, &catalog, Rules::Pdfium, &Limits::DEFAULT)
        .unwrap()
        .unwrap();
    assert_eq!(reads.get(), 0);
    let font = form
        .field_font(counting, b"F1", &Limits::DEFAULT, &mut ())
        .unwrap();
    let layout = font.layout();
    let res = font.res().unwrap();
    let mut made = 0;
    for field in form.terminals().map(|(_, field)| field) {
        let read = widget_text(
            field,
            &field.widgets()[0],
            counting,
            Rules::Pdfium,
            None,
            &Limits::DEFAULT,
            &mut (),
        )
        .unwrap();
        assert_eq!(read.font_name(), Some(&b"F1"[..]));
        let ap = read.make(res.clone(), &layout, &Limits::DEFAULT).unwrap();
        assert_eq!(ap.needs.0.len(), 1);
        made += 1;
    }
    assert_eq!((made, reads.get()), (1000, 1));

    let field = &form.fields()[0];
    for_widget(
        &form,
        field,
        &field.widgets()[0],
        counting,
        Rules::Pdfium,
        None,
        &Limits::DEFAULT,
        &mut (),
    )
    .unwrap();
    assert_eq!(reads.get(), 2);
}

#[test]
fn appearances_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Appearance>();
    check::<Unmade>();
    check::<super::Made>();
    check::<super::WidgetText<'static>>();
}

/// `field` made in Helvetica at 10 points in black in `look`: its content.
fn drawn(field: &TextField<'_>, look: &WidgetLook<'_>) -> String {
    let font = helvetica();
    let ap = text(
        field,
        look,
        &DefaultAppearance::new("Helv", 10.0, None),
        font.res().unwrap(),
        &font.layout(),
        &Limits::DEFAULT,
    )
    .unwrap();
    String::from_utf8(ap.content).unwrap()
}

/// A 100 by 20 box with a border 2 wide in `style` under `dash`, blue where
/// `colored`.
fn bordered(style: BoxBorderStyle, dash: &[f32], colored: bool) -> WidgetLook<'_> {
    let mut look = WidgetLook::new(100.0, 20.0);
    look.border = BoxBorder::new(2.0, style).with_dash(dash);
    look.border_color = colored.then_some(Color::Rgb([0.0, 0.0, 1.0]));
    look
}

/// What's drawn before the text: the content up to `/Tx BMC`.
fn under_the_text(content: &str) -> &str {
    &content[..content.find("/Tx BMC").unwrap()]
}

/// Each border style is drawn where PDFium draws it, in its order: a beveled
/// or inset border's bands first, with a border color or without one, then
/// the frame in that color; a dashed border as one open path from its
/// bottom left corner up, under its own dash array or `[3]`; an underline's
/// line half its width up; nothing for a width of `0`.
#[test]
fn each_border_style_is_drawn_as_pdfium_draws_it() {
    use BoxBorderStyle::{Beveled, Dashed, Inset, Solid, Underline};
    let hello = TextField::new("Hello");
    let border = |style, dash: &[f32], colored| {
        under_the_text(&drawn(&hello, &bordered(style, dash, colored))).to_string()
    };

    // Dashed: one open path, half the width inside the box, from the bottom
    // left corner up and round, under the dash array as given.
    let dashed = border(Dashed, &[3.0, 2.0], true);
    assert_eq!(
        dashed,
        "q\n0 0 1 RG\n2 w\n[3 2] 0 d\n1 1 m\n1 19 l\n99 19 l\n99 1 l\n1 1 l\nS\nQ\n"
    );
    // An array that can't be used, `[3]`.
    for unusable in [&[][..], &[0.0, 0.0], &[-1.0, 2.0], &[3.0, f32::INFINITY]] {
        assert!(
            border(Dashed, unusable, true).contains("[3] 0 d\n1 1 m\n"),
            "{unusable:?}"
        );
    }

    // Beveled: the light band, the dark one (white halved with no
    // background), then the frame.
    let beveled = border(Beveled, &[], true);
    assert_eq!(
        beveled,
        "q\n1 g\n2 2 m\n2 18 l\n98 18 l\n96 16 l\n4 16 l\n4 4 l\nh\nf\n\
         0.5 0.5 0.5 rg\n2 2 m\n98 2 l\n98 18 l\n96 16 l\n96 4 l\n4 4 l\nh\nf\nQ\n\
         q\n0 0 1 RG\n2 w\n[] 0 d\n1 1 98 18 re\nS\nQ\n"
    );
    // Over a background, the dark band is it halved.
    let mut look = bordered(Beveled, &[], true);
    look.background = Some(Color::Gray(0.8));
    let over = under_the_text(&drawn(&hello, &look)).to_string();
    assert!(
        over.starts_with("q\n0.8 g\n0 0 100 20 re\nf\nQ\nq\n1 g\n"),
        "{over}"
    );
    assert!(over.contains("f\n0.4 g\n2 2 m\n"), "{over}");
    // Inset: a middle gray band and a light one, then the frame.
    let inset = border(Inset, &[], true);
    assert!(inset.starts_with("q\n0.5 g\n2 2 m\n"), "{inset}");
    assert!(inset.contains("f\n0.75 g\n2 2 m\n"), "{inset}");
    assert!(inset.ends_with("Q\nq\n0 0 1 RG\n2 w\n[] 0 d\n1 1 98 18 re\nS\nQ\n"));
    // With no border color, the bands alone.
    for (style, banded) in [(Beveled, &beveled), (Inset, &inset)] {
        let bands = border(style, &[], false);
        assert!(banded.starts_with(&bands), "{style:?}: {bands}");
        assert!(
            !bands.contains("RG") && bands.ends_with("f\nQ\n"),
            "{bands}"
        );
    }

    // Solid: the frame; underline: the line along the bottom.
    assert_eq!(
        border(Solid, &[], true),
        "q\n0 0 1 RG\n2 w\n[] 0 d\n1 1 98 18 re\nS\nQ\n"
    );
    assert_eq!(
        border(Underline, &[], true),
        "q\n0 0 1 RG\n2 w\n[] 0 d\n0 1 m\n100 1 l\nS\nQ\n"
    );

    // No color, or no width: nothing for the other styles, nothing at all.
    for style in [Solid, Dashed, Underline] {
        assert_eq!(border(style, &[3.0], false), "", "{style:?}");
    }
    for style in [Solid, Dashed, Beveled, Inset, Underline] {
        for colored in [false, true] {
            let mut look = bordered(style, &[3.0], colored);
            look.border.width = 0.0;
            assert_eq!(under_the_text(&drawn(&hello, &look)), "", "{style:?}");
        }
    }
}

/// A comb field whose border is solid or dashed and has a color has its
/// cells divided, at the cells' edges across the box less the border, with
/// or without text: solid lines with square caps, or dashed ones under the
/// border's dash array. No other border draws them, nor one with no color,
/// nor a field of one cell. A field with more cells than `Limits::text`
/// is past that cap.
#[test]
fn a_comb_fields_cells_are_divided_by_solid_and_dashed_borders() {
    use BoxBorderStyle::{Beveled, Dashed, Inset, Solid, Underline};
    let mut comb = TextField::new("123");
    comb.flags = FieldFlags::COMB;
    comb.max_len = Some(5);
    let lines = "21.2 2 m\n21.2 18 l\nS\n40.4 2 m\n40.4 18 l\nS\n\
                 59.6 2 m\n59.6 18 l\nS\n78.8 2 m\n78.8 18 l\nS\nQ\n";

    let solid = drawn(&comb, &bordered(Solid, &[], true));
    let frame = "q\n0 0 1 RG\n2 w\n[] 0 d\n1 1 98 18 re\nS\nQ\n";
    assert_eq!(
        under_the_text(&solid),
        format!("{frame}q\n2 w\n0 0 1 RG\n2 J\n{lines}")
    );
    let dashed = drawn(&comb, &bordered(Dashed, &[3.0, 2.0], true));
    assert!(
        under_the_text(&dashed).ends_with(&format!("q\n2 w\n0 0 1 RG\n[3 2] 0 d\n{lines}")),
        "{dashed}"
    );
    // With no text, the cells are divided all the same.
    let mut empty = comb;
    empty.text = "";
    let divided = drawn(&empty, &bordered(Solid, &[], true));
    assert!(
        divided.ends_with(&format!("2 J\n{lines}/Tx BMC\nEMC\n")),
        "{divided}"
    );

    // Not for the other styles, a border with no color, one cell, or no
    // comb.
    let count = |content: &str| under_the_text(content).matches(" m\n").count();
    for style in [Beveled, Inset, Underline] {
        let content = drawn(&comb, &bordered(style, &[], true));
        let alone = drawn(&TextField::new("123"), &bordered(style, &[], true));
        assert_eq!(
            under_the_text(&content),
            under_the_text(&alone),
            "{style:?}"
        );
    }
    for style in [Solid, Dashed] {
        assert_eq!(count(&drawn(&comb, &bordered(style, &[3.0], false))), 0);
    }
    let mut one = comb;
    one.max_len = Some(1);
    assert_eq!(count(&drawn(&one, &bordered(Solid, &[], true))), 0);
    let mut unset = comb;
    unset.max_len = None;
    assert_eq!(count(&drawn(&unset, &bordered(Solid, &[], true))), 0);

    // More cells than `Limits::text`: the cap.
    let font = helvetica();
    let mut limits = Limits::DEFAULT;
    limits.text = 4;
    let make = |field: &TextField<'_>| {
        text(
            field,
            &bordered(Solid, &[], true),
            &DefaultAppearance::new("Helv", 10.0, None),
            font.res().unwrap(),
            &font.layout(),
            &limits,
        )
    };
    assert_eq!(make(&comb), Err(Unmade::Limit(Cap::Text)));
    comb.max_len = Some(4);
    assert!(make(&comb).is_ok());
    comb.max_len = Some(u32::MAX);
    assert_eq!(make(&comb), Err(Unmade::Limit(Cap::Text)));
}

/// `/MK /R` turns the box: at a quarter turn or three its width and height
/// swap, the text is laid out and the border drawn in the turned box, and
/// the matrix turns it back onto the widget's rectangle. A negative turn is
/// clockwise; a number that isn't a multiple of 90 is no turn.
#[test]
fn rotation_turns_the_box_and_its_matrix_places_it() {
    let quarter = [
        Matrix::IDENTITY,
        Matrix::new(0.0, 1.0, -1.0, 0.0, 100.0, 0.0),
        Matrix::new(-1.0, 0.0, 0.0, -1.0, 100.0, 20.0),
        Matrix::new(0.0, -1.0, 1.0, 0.0, 0.0, 20.0),
    ];
    let font = helvetica();
    for (rotation, turns) in [
        (0, 0),
        (90, 1),
        (180, 2),
        (270, 3),
        (360, 0),
        (450, 1),
        (-90, 3),
        (-180, 2),
        (-270, 1),
        (-360, 0),
        (45, 0),
        (-45, 0),
        (89, 0),
        (i32::MAX, 0),
        (i32::MIN, 0),
    ] {
        let mut look = WidgetLook::new(100.0, 20.0);
        look.rotation = rotation;
        look.border_color = Some(Color::Gray(0.0));
        let ap = text(
            &TextField::new("Hello"),
            &look,
            &DefaultAppearance::new("Helv", 10.0, None),
            font.res().unwrap(),
            &font.layout(),
            &Limits::DEFAULT,
        )
        .unwrap();
        let turned = turns % 2 == 1;
        let (w, h) = if turned { (20.0, 100.0) } else { (100.0, 20.0) };
        assert_eq!(ap.bbox, pdf_gfx::Rect::new(0.0, 0.0, w, h), "{rotation}");
        assert_eq!(ap.matrix, quarter[turns], "{rotation}");
        // The matrix places the box on the widget's rectangle.
        let corners = ap.bbox.corners().map(|p| ap.matrix.apply(p));
        let placed = corners.iter().fold(
            pdf_gfx::Rect::new(corners[0].x, corners[0].y, corners[0].x, corners[0].y),
            |r, p| r.union(pdf_gfx::Rect::new(p.x, p.y, p.x, p.y)),
        );
        assert_eq!(
            placed,
            pdf_gfx::Rect::new(0.0, 0.0, 100.0, 20.0),
            "{rotation}"
        );
        // The frame and the clip are the turned box's.
        let content = String::from_utf8(ap.content).unwrap();
        let (frame, clip) = match turned {
            true => ("0.5 0.5 19 99 re\nS", "1 1 18 98 re\nW"),
            false => ("0.5 0.5 99 19 re\nS", "1 1 98 18 re\nW"),
        };
        assert!(
            content.contains(frame) && content.contains(clip),
            "{rotation}: {content}"
        );
    }
}
