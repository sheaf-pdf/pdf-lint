//! Fonts written for embedding, of programs built by hand: each object a Type 0 font
//! over `/Identity-H` reaches, its keys and values; the font read back by this crate's
//! reader, its text, widths and glyphs' boxes, and shown on a page; its `/ToUnicode`,
//! its tag, `/CIDToGIDMap` and `/CIDSet`; what a licence allows; and the files PDFium's
//! cases and qpdf read.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use pdf_content_streams::{Concat, ResCx};
use pdf_fixtures::fixture::{self, Body, Xref};
use pdf_fixtures::font::{self, Glyph, TrueType};
use pdf_objects::object::Stream;
use pdf_objects::{Dict, Object, ObjectId, Unreadable, Value, Why, WithStreams, dict};
use skrifa::raw::{FontRef, TableProvider};

use crate::embed::{Embedding, Error, Glyphs, Options, Program, write};
use crate::{Font, Fonts, Limits, shown};

/// Texts read back, none with two characters one glyph of the sample draws: Latin, a
/// composite and a composite of composites among it; Chinese; Hebrew and Arabic, their
/// code points as given, unshaped; and one past the Basic Multilingual Plane.
const TEXTS: [&str; 5] = ["Ag HÄǞ", "中文世界", "שלום", "سلام", "𝄞A𝄞"];

/// The sample program at `units_per_em`, opened.
fn sample(units_per_em: u16) -> Program {
    Program::open(font::sample(units_per_em).build(), 0).unwrap()
}

/// `text` drawn in `program`, the font written under `options`: its objects, and the
/// codes `text` shows in it.
fn drawn(program: &Program, text: &str, options: &Options) -> (Vec<Object>, Vec<u8>) {
    let mut glyphs = Glyphs::new();
    let codes = glyphs.add(program, text).unwrap();
    (write(program, &glyphs, options).unwrap(), codes)
}

/// `objects`, as [`write()`] gives them, numbered from `first`: each reference `(i, 0)`
/// among them made `(first + i, 0)`.
fn placed(objects: &[Object], first: u32) -> BTreeMap<ObjectId, Object> {
    fn renumber(object: &mut Object, first: u32) {
        match object.value_mut() {
            Value::Ref(id) => *id = ObjectId::new(first + id.number, 0),
            Value::Array(items) => items.iter_mut().for_each(|o| renumber(o, first)),
            Value::Dict(dict) => dict.iter_mut().for_each(|(_, o)| renumber(o, first)),
            Value::Stream(stream) => stream.dict.iter_mut().for_each(|(_, o)| renumber(o, first)),
            _ => {}
        }
    }
    (first..)
        .zip(objects)
        .map(|(n, object)| {
            let mut object = object.clone();
            renumber(&mut object, first);
            (ObjectId::new(n, 0), object)
        })
        .collect()
}

/// The font object `font` of `objects` read, its program with `programs`, as a reading
/// of a file of them would: told no fault.
fn read(objects: &BTreeMap<ObjectId, Object>, font: u32) -> Font {
    let file = WithStreams::new(objects, |id, _| {
        let data = objects.get(&id).and_then(Object::as_stream);
        let data = data
            .and_then(|stream| stream.data.bytes())
            .map(<[u8]>::to_vec);
        data.ok_or_else(|| Unreadable::new(id, Why::Damaged, "no such stream"))
    });
    let dict = objects[&ObjectId::new(font, 0)].as_dict().unwrap();
    let mut budget = 1 << 24;
    let mut faults = Vec::new();
    let read = Font::read(file, dict, &Limits::DEFAULT, &mut budget, &mut faults).unwrap();
    assert!(faults.is_empty(), "{faults:?}");
    read
}

/// `objects[i]`'s dictionary, or its stream's.
fn dict(objects: &[Object], i: usize) -> &Dict {
    objects[i].as_dict().unwrap()
}

/// `objects[i]`'s stream's data.
fn data(objects: &[Object], i: usize) -> &[u8] {
    objects[i].as_stream().unwrap().data.bytes().unwrap()
}

/// The font's name, its `/BaseFont`.
fn base_font(objects: &[Object]) -> String {
    let name = dict(objects, 0).get("BaseFont").and_then(Object::as_name);
    String::from_utf8(name.unwrap().to_vec()).unwrap()
}

/// Each two bytes of `codes`, a code.
fn codes_of(codes: &[u8]) -> impl Iterator<Item = u16> + '_ {
    codes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|code| u16::from_be_bytes(*code))
}

/// A `/ToUnicode` CMap's entries, as [`write()`] writes them.
fn cmap(entries: &[&str]) -> String {
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    for block in entries.chunks(100) {
        cmap += &format!("{} beginbfchar\n", block.len());
        for entry in block {
            cmap += &format!("{entry}\n");
        }
        cmap += "endbfchar\n";
    }
    cmap + "endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n"
}

/// The objects a font of the sample is, each its keys and values in order: the name
/// shared, the program cut as it's cut, its `/Length1` its length, the CMap and the map
/// each glyph's, every stream unfiltered.
#[test]
fn the_font_is_its_objects() {
    let program = sample(1000);
    let mut glyphs = Glyphs::new();
    assert_eq!(glyphs.add(&program, "AgH").unwrap(), [0, 2, 0, 3, 0, 4]);
    let objects = write(&program, &glyphs, &Options::default()).unwrap();
    assert_eq!(objects.len(), 6);
    let name = Object::name(base_font(&objects));
    let type0 = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("Type0"),
        "BaseFont" => name.clone(),
        "Encoding" => Object::name("Identity-H"),
        "DescendantFonts" => vec![Object::reference(1)],
        "ToUnicode" => Object::reference(4),
    };
    assert_eq!(dict(&objects, 0), &type0);
    let widths = vec![2.into(), vec![600.into(), 500.into(), 700.into()].into()];
    let cid_font = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("CIDFontType2"),
        "BaseFont" => name.clone(),
        "CIDSystemInfo" => dict! {
            "Registry" => Object::text("Adobe"),
            "Ordering" => Object::text("Identity"),
            "Supplement" => 0,
        },
        "FontDescriptor" => Object::reference(2),
        "W" => Object::from(widths),
        "CIDToGIDMap" => Object::reference(5),
    };
    assert_eq!(dict(&objects, 1), &cid_font);
    let bbox: Vec<Object> = [10, -400, 960, 1100].map(Object::from).to_vec();
    let descriptor = dict! {
        "Type" => Object::name("FontDescriptor"),
        "FontName" => name,
        "Flags" => 4,
        "FontBBox" => bbox,
        "ItalicAngle" => 0,
        "Ascent" => 800,
        "Descent" => -200,
        "CapHeight" => 700,
        "XHeight" => 480,
        "StemV" => 95,
        "FontFile2" => Object::reference(3),
    };
    assert_eq!(dict(&objects, 2), &descriptor);

    let cut = program.subset(&glyphs).unwrap();
    assert_eq!(cut.glyphs, [0, 2, 3, 4]);
    let length = cut.program.len() as i64;
    let file = dict! { "Length1" => length, "Length" => length };
    assert_eq!(dict(&objects, 3), &file);
    assert_eq!(data(&objects, 3), cut.program);
    let to_unicode = cmap(&["<0002> <0041>", "<0003> <0067>", "<0004> <0048>"]);
    assert_eq!(String::from_utf8_lossy(data(&objects, 4)), to_unicode);
    assert_eq!(data(&objects, 5), [0, 0, 0, 0, 0, 1, 0, 2, 0, 3]);
    for i in 4..6 {
        let length = data(&objects, i).len() as i64;
        assert_eq!(dict(&objects, i), &dict! { "Length" => length }, "{i}");
    }
}

/// Text drawn and the font written reads back by this crate's reader as it was given,
/// each code's width the program's, at 2048 units an em, and with `programs` each
/// code's glyph's box the box of the glyph it is in the program as given; shown on a
/// page, its strings are the texts.
#[test]
fn the_font_reads_back_as_drawn() {
    let fixture = font::sample(2048);
    let program = Program::open(fixture.build(), 0).unwrap();
    // No two characters on one glyph.
    let mut glyph_of = HashMap::new();
    for c in TEXTS.concat().chars() {
        let glyph = program.glyph(c).unwrap();
        assert_eq!(*glyph_of.entry(glyph).or_insert(c), c, "{c:?}");
    }
    let mut glyphs = Glyphs::new();
    let codes = TEXTS.map(|text| glyphs.add(&program, text).unwrap());
    let objects = write(&program, &glyphs, &Options::default()).unwrap();
    let file = placed(&objects, 10);
    let font = read(&file, 10);
    let original = whole_as_given(&fixture.build());
    for (text, codes) in TEXTS.iter().zip(&codes) {
        assert_eq!(font.text(codes).as_deref(), Some(*text));
        for code in codes_of(codes) {
            let width = font.width(u32::from(code));
            assert_eq!(width, program.width(code), "{text:?} {code}");
            assert_ne!(width, 0.0, "{text:?} {code}");
            if cfg!(feature = "programs") {
                let boxed = font.char_box(u32::from(code));
                assert!(boxed.is_some(), "{text:?} {code}");
                assert_eq!(boxed, original.char_box(u32::from(code)), "{text:?} {code}");
            }
        }
    }
    // A's glyph is wider than it's tall, and g's reaches below the line.
    if cfg!(feature = "programs") {
        let a = font.char_box(2).unwrap();
        let g = font.char_box(3).unwrap();
        assert!(a.x1 > 580.0 && a.y0 == 0.0 && g.y0 < -190.0, "{a:?} {g:?}");
    }

    let resources = Object::from(dict! { "Font" => dict! { "F1" => Object::reference(10) } });
    let content: String = (codes.iter())
        .map(|codes| {
            let hex: String = codes.iter().map(|b| format!("{b:02X}")).collect();
            format!("BT /F1 12 Tf <{hex}> Tj ET\n")
        })
        .collect();
    let content = Concat::new(&[content.as_bytes()]);
    let streams = WithStreams::new(&file, |id, _| {
        let data = file.get(&id).and_then(Object::as_stream);
        let data = data
            .and_then(|stream| stream.data.bytes())
            .map(<[u8]>::to_vec);
        data.ok_or_else(|| Unreadable::new(id, Why::Damaged, "no such stream"))
    });
    let mut budget = 1 << 24;
    let fonts = Fonts::default();
    let read = shown(
        streams,
        &fonts,
        Some(&resources),
        &content,
        &ResCx::none(),
        &Limits::DEFAULT,
        &mut budget,
    );
    assert!(read.is_whole(), "{read:?}");
    let strings: Vec<String> = read.strings.iter().map(|s| s.text().unwrap()).collect();
    assert_eq!(strings, TEXTS);
    assert_eq!(read.text(), TEXTS.concat());
}

/// `program`, embedded as it's given in a CIDFontType2 whose CIDs are its glyphs, read:
/// each glyph's box as the reader gives a program's.
fn whole_as_given(program: &[u8]) -> Font {
    let objects = BTreeMap::from([
        (
            ObjectId::new(1, 0),
            dict! {
                "Type" => Object::name("Font"), "Subtype" => Object::name("Type0"),
                "BaseFont" => Object::name("Given"), "Encoding" => Object::name("Identity-H"),
                "DescendantFonts" => vec![Object::reference(2)],
            }
            .into(),
        ),
        (
            ObjectId::new(2, 0),
            dict! {
                "Type" => Object::name("Font"), "Subtype" => Object::name("CIDFontType2"),
                "BaseFont" => Object::name("Given"),
                "CIDSystemInfo" => dict! {
                    "Registry" => Object::text("Adobe"), "Ordering" => Object::text("Identity"),
                    "Supplement" => 0,
                },
                "FontDescriptor" => Object::reference(3),
                "CIDToGIDMap" => Object::name("Identity"),
            }
            .into(),
        ),
        (
            ObjectId::new(3, 0),
            dict! {
                "Type" => Object::name("FontDescriptor"), "FontName" => Object::name("Given"),
                "Flags" => 4, "FontBBox" => [0, -400, 1000, 1100].map(Object::from).to_vec(),
                "ItalicAngle" => 0, "Ascent" => 800, "Descent" => -200, "CapHeight" => 700,
                "StemV" => 80, "FontFile2" => Object::reference(4),
            }
            .into(),
        ),
        (
            ObjectId::new(4, 0),
            Stream::new(dict! {}, program.to_vec()).into(),
        ),
    ]);
    read(&objects, 1)
}

/// Two characters the program draws with one glyph show one code, which reads back as
/// the smaller of their texts, whichever was drawn first.
#[test]
fn two_characters_on_one_glyph_read_back_as_the_smaller() {
    let mut fixture = font::sample(1000);
    fixture.map('B', 2);
    fixture.map('\u{391}', 2);
    let program = Program::open(fixture.build(), 0).unwrap();
    for (drawn, text) in [
        ("BA", "AA"),
        ("AB", "AA"),
        ("\u{391}B", "BB"),
        ("B\u{391}", "BB"),
    ] {
        let mut glyphs = Glyphs::new();
        let codes = glyphs.add(&program, drawn).unwrap();
        assert_eq!(codes, [0, 2, 0, 2]);
        let objects = write(&program, &glyphs, &Options::default()).unwrap();
        let font = read(&placed(&objects, 1), 1);
        assert_eq!(font.text(&codes).as_deref(), Some(text), "{drawn:?}");
    }
}

/// A `/ToUnicode` CMap gives a character past the Basic Multilingual Plane as its
/// surrogate pair, one code's, never two codes' lone halves; its entries are in blocks
/// of 100 at most; a glyph's text is the smallest kept, whatever order they came in; and
/// a glyph kept with no text has no entry.
#[test]
fn the_to_unicode_cmap() {
    let program = sample(1000);
    let (objects, codes) = drawn(&program, "𝄞", &Options::default());
    assert_eq!(codes, [0x01, 0x40]);
    assert_eq!(
        String::from_utf8_lossy(data(&objects, 4)),
        cmap(&["<0140> <D834DD1E>"])
    );
    let font = read(&placed(&objects, 1), 1);
    assert_eq!(font.text(&codes).as_deref(), Some("𝄞"));

    let mut many = Glyphs::new();
    (1..=250).for_each(|glyph| many.insert(glyph, "x"));
    let objects = write(&program, &many, &Options::default()).unwrap();
    let to_unicode = String::from_utf8(data(&objects, 4).to_vec()).unwrap();
    let blocks: Vec<&str> = (to_unicode.lines())
        .filter(|line| line.ends_with(" beginbfchar"))
        .collect();
    assert_eq!(
        blocks,
        ["100 beginbfchar", "100 beginbfchar", "50 beginbfchar"]
    );
    let entries: Vec<String> = (1..=250).map(|g| format!("<{g:04X}> <0078>")).collect();
    let entries: Vec<&str> = entries.iter().map(String::as_str).collect();
    assert_eq!(to_unicode, cmap(&entries));
    let font = read(&placed(&objects, 1), 1);
    for glyph in [1u16, 100, 101, 200, 201, 250] {
        assert_eq!(
            font.text(&glyph.to_be_bytes()).as_deref(),
            Some("x"),
            "{glyph}"
        );
    }

    let mut one = Glyphs::new();
    one.insert(2, "B");
    one.add(&program, "A").unwrap();
    one.insert(3, "fi");
    one.insert(3, "f");
    let mut other = Glyphs::new();
    other.insert(3, "fi");
    other.add(&program, "A").unwrap();
    other.insert(2, "B");
    other.insert(3, "f");
    other.insert(4, "");
    let one = write(&program, &one, &Options::default()).unwrap();
    let other = write(&program, &other, &Options::default()).unwrap();
    assert_eq!(data(&one, 4), data(&other, 4));
    assert_eq!(
        String::from_utf8_lossy(data(&one, 4)),
        cmap(&["<0002> <0041>", "<0003> <0066>"])
    );
}

/// A subset's name is a tag of six capitals and `+` before the program's PostScript
/// name: the same for the same glyphs of the same program, in any order, and another
/// for other glyphs or another program.
#[test]
fn a_subset_is_tagged_by_its_glyphs() {
    let program = sample(1000);
    let name =
        |program: &Program, text: &str| base_font(&drawn(program, text, &Options::default()).0);
    let tagged = name(&program, "Ag");
    let (tag, rest) = tagged.split_at(6);
    assert!(tag.bytes().all(|b| b.is_ascii_uppercase()), "{tagged}");
    assert_eq!(rest, "+FixtureSans-Regular");
    assert_eq!(name(&program, "Ag"), tagged);
    assert_eq!(name(&program, "gAgA"), tagged);
    let others = [
        name(&program, "AgH"),
        name(&program, "A"),
        name(&program, "Ǟ"),
        name(&sample(2048), "Ag"),
    ];
    for other in &others {
        assert_ne!(other[..6], tagged[..6], "{other}");
        assert_eq!(&other[6..], rest);
    }
}

/// The same glyphs of the same program are written in the same objects, whatever order
/// they were drawn in and however often, and so in the same file.
#[test]
fn the_same_glyphs_write_the_same_bytes() {
    let program = sample(1000);
    let texts = ["Ag中𝄞שלום", "𝄞中gA", "םולש"];
    let mut written = Vec::new();
    for order in [[0, 1, 2], [2, 1, 0], [1, 2, 0], [0, 1, 2]] {
        let mut glyphs = Glyphs::new();
        for i in order {
            glyphs.add(&program, texts[i]).unwrap();
        }
        written.push(write(&program, &glyphs, &Options::default()).unwrap());
    }
    assert!(written.iter().all(|objects| *objects == written[0]));
    let codes = program.codes("Ag").unwrap();
    assert_eq!(
        case_file(&written[0], &codes),
        case_file(&written[3], &codes)
    );
}

/// A `/CIDSet` is written where it's asked for, a bit a CID from 0, set for `.notdef`,
/// each glyph drawn and each a composite among them draws; and not by default.
#[test]
fn a_cid_set_is_written_where_asked() {
    let program = sample(1000);
    let (objects, _) = drawn(&program, "ǞH中", &Options::default());
    assert_eq!(objects.len(), 6);
    assert_eq!(dict(&objects, 2).get("CIDSet"), None);

    let mut options = Options {
        cid_set: true,
        ..Options::default()
    };
    let (objects, _) = drawn(&program, "ǞH中", &options);
    assert_eq!(objects.len(), 7);
    assert_eq!(dict(&objects, 2).get("CIDSet"), Some(&Object::reference(6)));
    // 0, 2, 4 to 8 and 300.
    let mut bits = vec![0; 38];
    bits[0] = 0b1010_1111;
    bits[1] = 0b1000_0000;
    bits[37] = 0b0000_1000;
    assert_eq!(data(&objects, 6), bits);
    let length = bits.len() as i64;
    assert_eq!(dict(&objects, 6), &dict! { "Length" => length });

    // Embedded whole, every glyph's CID, after the program's CMap.
    options.whole = true;
    let (objects, _) = drawn(&program, "A", &options);
    assert_eq!(objects.len(), 6);
    assert_eq!(dict(&objects, 2).get("CIDSet"), Some(&Object::reference(5)));
    let mut bits = vec![0xFF; 41];
    bits[40] = 0b1000_0000;
    assert_eq!(data(&objects, 5), bits);
}

/// A program embedded whole, as asked or as its licence asks, is every glyph in its own
/// order, under its own name, its CIDs its glyphs (`/Identity`), with no map; its text
/// reads back the same.
#[test]
fn a_program_is_embedded_whole_where_asked() {
    let whole = Options {
        whole: true,
        ..Options::default()
    };
    let mut licensed = font::sample(1000);
    licensed.os2.as_mut().unwrap().fs_type = 0x0100;
    let licensed = Program::open(licensed.build(), 0).unwrap();
    assert!(!licensed.embedding().subsetting);
    for (program, options) in [(sample(1000), whole), (licensed, Options::default())] {
        let (objects, codes) = drawn(&program, "Ag中", &options);
        assert_eq!(objects.len(), 5);
        assert_eq!(base_font(&objects), "FixtureSans-Regular");
        let identity = Object::name("Identity");
        assert_eq!(dict(&objects, 1).get("CIDToGIDMap"), Some(&identity));
        let cut = FontRef::new(data(&objects, 3)).unwrap();
        assert_eq!(cut.maxp().unwrap().num_glyphs(), 321);
        // Every glyph, but not every table: none to draw text in it by character.
        assert!(cut.cmap().is_err() && cut.os2().is_err());
        assert_eq!(data(&objects, 3), program.whole().unwrap().program);
        let font = read(&placed(&objects, 1), 1);
        assert_eq!(font.text(&codes).as_deref(), Some("Ag中"));
        if cfg!(feature = "programs") {
            let given = whole_as_given(&program.data);
            for code in codes_of(&codes).map(u32::from) {
                assert!(font.char_box(code).is_some(), "{code}");
                assert_eq!(font.char_box(code), given.char_box(code), "{code}");
            }
        }
    }
}

/// A program whose licence restricts it to its owner's leave, or to its bitmaps, is
/// refused unless the caller says it has the right; one that may be previewed, edited
/// or installed is written as any.
#[test]
fn a_restricted_licence_is_refused_unless_given() {
    let given = Options {
        licensed: true,
        ..Options::default()
    };
    for (fs_type, refused) in [
        (0x0002, true),
        (0x0200, true),
        (0x0202, true),
        (0x0004, false),
        (0x0008, false),
        (0x0000, false),
    ] {
        let mut fixture = font::sample(1000);
        fixture.os2.as_mut().unwrap().fs_type = fs_type;
        let program = Program::open(fixture.build(), 0).unwrap();
        let mut glyphs = Glyphs::new();
        glyphs.add(&program, "Ag").unwrap();
        let written = write(&program, &glyphs, &Options::default());
        match refused {
            true => assert_eq!(
                written.unwrap_err(),
                Error::Restricted(Embedding::of(fs_type)),
                "{fs_type:#06x}"
            ),
            false => assert_eq!(written.unwrap().len(), 6, "{fs_type:#06x}"),
        }
        let embedding = program.embedding();
        assert_eq!(embedding.allows(&Options::default()), !refused);
        assert!(embedding.allows(&given));
        assert_eq!(write(&program, &glyphs, &given).unwrap().len(), 6);
    }
    assert_eq!(
        Error::Restricted(Embedding::of(2)).to_string(),
        "the font program's licence doesn't let its outlines be embedded"
    );
}

/// A glyph kept past the program's count is refused, cut or whole.
#[test]
fn glyphs_past_the_count_are_refused() {
    let program = sample(1000);
    let mut glyphs = Glyphs::new();
    glyphs.add(&program, "A").unwrap();
    glyphs.insert(400, "x");
    let whole = Options {
        whole: true,
        ..Options::default()
    };
    for options in [Options::default(), whole] {
        assert_eq!(
            write(&program, &glyphs, &options).unwrap_err(),
            Error::Glyph {
                glyph: 400,
                count: 321
            }
        );
    }
}

/// `/W` holds each glyph drawn, a run of glyphs one after another their first CID and
/// their widths, whole thousandths; none where nothing's drawn.
#[test]
fn widths_are_runs_of_the_glyphs_drawn() {
    let program = sample(1000);
    let (objects, _) = drawn(&program, "שAg Hם中界𝄞", &Options::default());
    let w = dict(&objects, 1).get("W").unwrap();
    let run =
        |widths: &[i64]| Object::from(widths.iter().map(|&w| Object::from(w)).collect::<Vec<_>>());
    let expected = Object::from(vec![
        1.into(),
        run(&[250, 600, 500, 700]),
        9.into(),
        run(&[660]),
        12.into(),
        run(&[610]),
        300.into(),
        run(&[1000]),
        303.into(),
        run(&[1000]),
        320.into(),
        run(&[580]),
    ]);
    assert_eq!(w, &expected);
    let (objects, _) = drawn(&program, "", &Options::default());
    assert_eq!(dict(&objects, 1).get("W"), Some(&Object::from(Vec::new())));
    assert_eq!(program.subset(&Glyphs::new()).unwrap().glyphs, [0]);
    assert_eq!(data(&objects, 5), [0, 0]);
}

/// A descriptor's values are the program's: its flags fixed pitch, serif and italic as
/// its tables say; its box rounded out; its cap height its `OS/2`'s, else `H`'s top,
/// else its ascent; its x height where its `OS/2` gives one; its stem from its weight.
#[test]
fn the_descriptor_is_the_program_s() {
    let descriptor = |fixture: &TrueType| {
        let program = Program::open(fixture.build(), 0).unwrap();
        let (objects, _) = drawn(&program, "A", &Options::default());
        dict(&objects, 2).clone()
    };
    let number = |d: &Dict, key: &str| d.get(key).and_then(Object::as_number);

    let mut fixture = font::sample(1000);
    fixture.post.fixed_pitch = true;
    fixture.os2.as_mut().unwrap().family_class = 0x0105;
    fixture.mac_style = 0x0002;
    assert_eq!(number(&descriptor(&fixture), "Flags"), Some(71.0));
    fixture.mac_style = 0;
    fixture.post.italic_angle = -12 * 65536 - 32768;
    let leaning = descriptor(&fixture);
    assert_eq!(number(&leaning, "Flags"), Some(71.0));
    assert_eq!(leaning.get("ItalicAngle"), Some(&Object::from(-12.5f32)));
    for (class, flags) in [(0x0100, 6.0), (0x0700, 6.0), (0x0600, 4.0), (0x0800, 4.0)] {
        let mut fixture = font::sample(1000);
        fixture.os2.as_mut().unwrap().family_class = class;
        assert_eq!(
            number(&descriptor(&fixture), "Flags"),
            Some(flags),
            "{class:#06x}"
        );
    }

    // At 2048 units, the box is 20, -819, 1966, 2253: 9.77, -399.9, 959.96, 1100.1.
    let wide = descriptor(&font::sample(2048));
    let bbox: Vec<Object> = [9, -400, 960, 1101].map(Object::from).to_vec();
    assert_eq!(wide.get("FontBBox"), Some(&Object::from(bbox)));
    assert_eq!(number(&wide, "Ascent"), Some(800.0));
    assert_eq!(number(&wide, "CapHeight"), Some(700.0));

    let mut fixture = font::sample(1000);
    let os2 = fixture.os2.as_mut().unwrap();
    os2.cap_height = 0;
    os2.weight = 700;
    let bold = descriptor(&fixture);
    assert_eq!(number(&bold, "CapHeight"), Some(700.0));
    assert_eq!(number(&bold, "StemV"), Some(169.0));
    fixture.os2.as_mut().unwrap().version = 1;
    let old = descriptor(&fixture);
    assert_eq!(number(&old, "CapHeight"), Some(700.0));
    assert_eq!(old.get("XHeight"), None);
    fixture.os2 = None;
    assert_eq!(number(&descriptor(&fixture), "StemV"), Some(80.0));
    // With no `H`, its ascent.
    let mut plain = TrueType::new(1000);
    let a = plain.glyph(Glyph::Box(10, 0, 590, 650), 600);
    plain.map('A', a);
    plain.os2.as_mut().unwrap().version = 1;
    let plain = descriptor(&plain);
    assert_eq!(number(&plain, "CapHeight"), Some(800.0));
    assert_eq!(number(&plain, "StemV"), Some(95.0));
    let keys: Vec<&[u8]> = plain.iter().map(|(key, _)| key).collect();
    assert_eq!(
        keys,
        [
            &b"Type"[..],
            b"FontName",
            b"Flags",
            b"FontBBox",
            b"ItalicAngle",
            b"Ascent",
            b"Descent",
            b"CapHeight",
            b"StemV",
            b"FontFile2"
        ]
    );
}

/// A font's name is its program's PostScript name's bytes that a name holds unescaped,
/// at most 120; its full name's where those are none; `Unnamed` where neither.
#[test]
fn a_font_is_named_by_its_program() {
    let named = |names: &[(u16, &str)]| {
        let mut fixture = font::sample(1000);
        fixture.names = names
            .iter()
            .map(|&(id, name)| (id, name.to_owned()))
            .collect();
        let program = Program::open(fixture.build(), 0).unwrap();
        let whole = Options {
            whole: true,
            ..Options::default()
        };
        base_font(&drawn(&program, "A", &whole).0)
    };
    assert_eq!(named(&[(6, "Fix(ture) Sans/#1%[é]")]), "FixtureSans1");
    assert_eq!(named(&[(4, "Fixture Sans Bold")]), "FixtureSansBold");
    assert_eq!(named(&[(4, "Fixture Bold"), (6, "()")]), "FixtureBold");
    assert_eq!(named(&[(1, "Fixture")]), "Unnamed");
    assert_eq!(named(&[]), "Unnamed");
    let long = "F".repeat(200);
    assert_eq!(named(&[(6, &long)]), "F".repeat(120));
    let program = {
        let mut fixture = font::sample(1000);
        fixture.names = vec![(6, long.clone())];
        Program::open(fixture.build(), 0).unwrap()
    };
    assert_eq!(
        base_font(&drawn(&program, "A", &Options::default()).0).len(),
        127
    );
}

/// Options are each off by default, and say so.
#[test]
fn options_are_off_by_default() {
    let options = Options::default();
    assert!(!options.cid_set && !options.whole && !options.licensed);
    assert_eq!(
        format!("{options:?}"),
        "Options { cid_set: false, whole: false, licensed: false }"
    );
}

/// A file of one page showing `codes` at 1000 points in the font `objects` are, from
/// object 10, as PDFium's cases are laid out: 1 the catalog, 2 the pages, 3 the page,
/// 4 its content.
fn case_file(objects: &[Object], codes: &[u8]) -> Vec<u8> {
    let hex: String = codes.iter().map(|b| format!("{b:02X}")).collect();
    let content = format!("BT /F0 1000 Tf 0 100 Td <{hex}> Tj ET");
    let media = [0, 0, 200_000, 20_000].map(Object::from).to_vec();
    let mut bodies: Vec<(u32, Body)> = vec![
        (
            1,
            dict! { "Type" => Object::name("Catalog"), "Pages" => Object::reference(2) }.into(),
        ),
        (
            2,
            dict! {
                "Type" => Object::name("Pages"), "Kids" => vec![Object::reference(3)],
                "Count" => 1,
            }
            .into(),
        ),
        (
            3,
            dict! {
                "Type" => Object::name("Page"), "Parent" => Object::reference(2),
                "MediaBox" => media, "Contents" => Object::reference(4),
                "Resources" => dict! { "Font" => dict! { "F0" => Object::reference(10) } },
            }
            .into(),
        ),
        (4, Body::stream(dict! {}, content.as_bytes())),
    ];
    for (id, object) in placed(objects, 10) {
        let body = match object.as_stream() {
            Some(stream) => Body::stream(stream.dict.clone(), stream.data.bytes().unwrap()),
            None => Body::Object(object),
        };
        bodies.push((id.number, body));
    }
    let trailer = dict! { "Root" => Object::reference(1) };
    fixture::file("1.7", &bodies, trailer, Xref::Table)
}

/// The files PDFium's cases read of fonts the writer embeds (`tests/pdfium/written`),
/// each a page showing a string at 1000 points, the font object 10: Latin, a composite
/// and a composite of composites among it, at 1000 units an em and at 2048; Chinese;
/// Hebrew and Arabic, unshaped, in the order given; a character past the Basic
/// Multilingual Plane; a program embedded whole; and one with a `/CIDSet`.
fn written_cases() -> Vec<(&'static str, Vec<u8>)> {
    let case = |program: &Program, text: &str, options: &Options| {
        let (objects, codes) = drawn(program, text, options);
        case_file(&objects, &codes)
    };
    let (thousand, wide) = (sample(1000), sample(2048));
    let default = Options::default();
    let whole = Options {
        whole: true,
        ..Options::default()
    };
    let cid_set = Options {
        cid_set: true,
        ..Options::default()
    };
    vec![
        ("latin", case(&thousand, "Ag HÄǞ", &default)),
        ("latin-2048", case(&wide, "Ag HÄǞ", &default)),
        ("cjk", case(&thousand, "中文世界", &default)),
        ("hebrew", case(&thousand, "שלום", &default)),
        ("arabic", case(&thousand, "سلام", &default)),
        ("clef", case(&thousand, "A𝄞g", &default)),
        ("whole", case(&thousand, "Ag中", &whole)),
        ("cid-set", case(&thousand, "Ag中", &cid_set)),
    ]
}

/// Where `tests/pdfium/written` is.
fn written_folder() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pdfium/written")
}

/// Writes `tests/pdfium/written`: run it when the writer or a case changes, then `just
/// pdfium-answers`.
#[test]
#[ignore = "writes into the source tree"]
fn the_written_cases_are_written() {
    let folder = written_folder();
    std::fs::create_dir_all(&folder).unwrap();
    for file in std::fs::read_dir(&folder).unwrap() {
        std::fs::remove_file(file.unwrap().path()).unwrap();
    }
    for (name, file) in written_cases() {
        std::fs::write(folder.join(format!("{name}.pdf")), file).unwrap();
    }
}

/// The oracle's written files are what the writer writes, and no others. A package holds
/// no such folder.
#[test]
fn the_written_cases_are_the_writer() {
    let folder = written_folder();
    if !folder.exists() {
        return;
    }
    let cases = written_cases();
    let mut names: Vec<String> = (std::fs::read_dir(&folder).unwrap())
        .map(|file| file.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut expected: Vec<String> = cases
        .iter()
        .map(|(name, _)| format!("{name}.pdf"))
        .collect();
    expected.sort();
    assert_eq!(names, expected, "run the_written_cases_are_written");
    for (name, file) in cases {
        let written = std::fs::read(folder.join(format!("{name}.pdf"))).unwrap();
        assert!(written == file, "{name}: run the_written_cases_are_written");
    }
}

/// `qpdf --check` finds nothing in each written file: what it says is printed, for the
/// record of `just test-qpdf`'s run. qpdf is pdf-fixtures': `PDF_LINT_QPDF`, else `qpdf`
/// on the `PATH`; ignored, so `cargo test` needs none, and its absence fails it.
#[test]
#[ignore = "needs qpdf: `just test-qpdf`"]
fn qpdf_checks_the_written_cases_clean() {
    use pdf_fixtures::qpdf::Qpdf;
    for (name, file) in written_cases() {
        let path = written_folder().join(format!("{name}.pdf"));
        let on_disk = std::fs::read(&path).unwrap();
        assert!(on_disk == file, "{name}: run the_written_cases_are_written");
        let qpdf = Qpdf::new(&path);
        let ran = qpdf
            .exec(&["--check".into(), path.display().to_string()])
            .unwrap();
        let (stdout, stderr) = (
            String::from_utf8_lossy(&ran.stdout),
            String::from_utf8_lossy(&ran.stderr),
        );
        println!("{name}: exit {:?}\n{stdout}{stderr}", ran.code);
        assert_eq!(ran.code, Some(0), "{name}: {stdout}{stderr}");
        assert!(
            stderr.is_empty() && !stdout.contains("WARNING"),
            "{name}: {stdout}{stderr}"
        );
        assert!(
            stdout.contains("No syntax or stream encoding errors"),
            "{name}: {stdout}"
        );
    }
}
