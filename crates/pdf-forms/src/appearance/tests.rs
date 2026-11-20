//! Unit, cap, and hygiene tests for the field's font.

use std::collections::BTreeMap;

use pdf_content_streams::Res;
use pdf_fonts::{Font, Std14};
use pdf_gfx::LayoutFont;
use pdf_objects::fault::Place;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{
    Data, Dict, Object, ObjectId, Resolve, Streams, Unreadable, Value, Why, WithStreams, dict,
};

use super::{FieldFont, FieldLayoutFont};
use crate::{Cap, Error, Fault, Form, Limits};

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
        assert_eq!(metrics.bbox, face.metrics().bbox);
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
        "exceeded cap on bytes of an appearance's text"
    );
}

#[test]
fn field_fonts_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<FieldFont>();
    check::<FieldLayoutFont<'static>>();
    check::<Font>();
}
