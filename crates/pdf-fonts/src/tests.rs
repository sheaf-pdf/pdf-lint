//! The fonts' data: widths that are Adobe's, the encodings' tables each equal to
//! its AFM, and Adobe's notices carried; and a Type 1 program's scan.

use std::collections::HashMap;

use super::afm::Afm;
use super::fixture;
use super::*;

mod fonts;

/// An AFM's widths and codes, read here as plainly as it can be: each
/// `C <code> ; WX <width> ; N <name>` line.
fn afm(text: &str) -> (HashMap<&str, f32>, HashMap<u8, &str>) {
    let mut widths = HashMap::new();
    let mut codes = HashMap::new();
    for line in text.lines().filter(|l| l.starts_with("C ")) {
        let field = |key: &str| {
            line.split(';')
                .find_map(|f| f.trim().strip_prefix(key))
                .map(str::trim)
        };
        let name = field("N ").unwrap();
        widths.insert(name, field("WX ").unwrap().parse().unwrap());
        if let Ok(code) = field("C ").unwrap().parse::<u8>() {
            codes.insert(code, name);
        }
    }
    (widths, codes)
}

/// Windows-1252, from its code chart: None for the five codes it leaves
/// empty.
fn cp1252_chart(code: u8) -> Option<char> {
    const HIGH: &str = "€\u{81}‚ƒ„…†‡ˆ‰Š‹Œ\u{8d}Ž\u{8f}\u{90}‘’“”•–—˜™š›œ\u{9d}žŸ";
    match code {
        0x81 | 0x8d | 0x8f | 0x90 | 0x9d => None,
        0x80..=0x9f => HIGH.chars().nth(code as usize - 0x80),
        _ => Some(code as char),
    }
}

/// Every code an encoding a font is drawn in maps has its glyph's width in
/// Adobe's AFM, and every glyph it names is in the AFM.
#[test]
fn widths_equal_the_afm() {
    for font in Std14::ALL {
        let (widths, codes) = afm(font.afm_text());
        let encoding = match font {
            Std14::Symbol | Std14::ZapfDingbats => font.own(),
            _ => Encoding::WinAnsi,
        };
        let mut mapped = 0;
        for code in 0..=255u8 {
            let Some(glyph) = encoding.glyph(code) else {
                continue;
            };
            let width = widths.get(glyph).copied();
            assert_eq!(font.glyph_width(glyph), width, "{font:?} {code:#x} {glyph}");
            assert!(width.is_some(), "{font:?} {code:#x} {glyph}");
            mapped += 1;
        }
        let expected = match font {
            // Printable ASCII, 0x7F's bullet, 0x80–0x9F with the bullet where it has
            // nothing else, and 0xA0–0xFF.
            Std14::Symbol | Std14::ZapfDingbats => codes.len(),
            _ => 95 + 1 + 32 + 96,
        };
        assert_eq!(mapped, expected, "{font:?}");
        assert_eq!(font.afm().len(), widths.len(), "{font:?}");
        for (name, width) in widths {
            assert_eq!(font.glyph_width(name), Some(width), "{font:?} {name}");
        }
        assert_eq!(font.glyph_width("no such glyph"), None);
    }
    assert_eq!(Std14::Helvetica.glyph_width("A"), Some(667.0));
    assert_eq!(Std14::Helvetica.glyph_width("Euro"), Some(556.0));
    assert_eq!(Std14::HelveticaBold.glyph_width("m"), Some(889.0));
    assert_eq!(Std14::Courier.glyph_width("OE"), Some(600.0));
    assert_eq!(Std14::TimesRoman.glyph_width("florin"), Some(500.0));
    assert_eq!(Std14::Symbol.glyph_width("alpha"), Some(631.0));
    assert_eq!(Std14::ZapfDingbats.glyph_width("a20"), Some(846.0));
}

/// Each table of an encoding names what its fonts' AFM gives each code: the
/// twelve Latin fonts' StandardEncoding, Symbol's and ZapfDingbats' own.
#[test]
fn each_table_equals_its_afm() {
    for font in Std14::ALL {
        let (_, codes) = afm(font.afm_text());
        let own = font.own();
        for code in 0..=255u8 {
            let of_afm = codes.get(&code).copied();
            assert_eq!(own.glyph(code), of_afm, "{font:?} {code:#x}");
        }
    }
    // As each table is a font's own, none is another's.
    assert_eq!(Encoding::Standard.glyph(b'A'), Some("A"));
    assert_eq!(Encoding::Symbol.glyph(b'A'), Some("Alpha"));
    assert_eq!(Encoding::ZapfDingbats.glyph(b'A'), Some("a10"));
    assert_eq!(Encoding::Standard.glyph(0x27), Some("quoteright"));
    assert_eq!(Encoding::WinAnsi.glyph(0x27), Some("quotesingle"));
    assert_eq!(Encoding::MacRoman.glyph(0x80), Some("Adieresis"));
    assert_eq!(Encoding::MacRoman.glyph(0xf0), Some("apple"));
    assert_eq!(Encoding::MacRoman.text(0xf0).as_deref(), Some("\u{f8ff}"));
    // WinAnsi's bullet at 0x7f is its own, though MacRoman's low half is the same.
    assert_eq!(Encoding::WinAnsi.glyph(0x7f), Some("bullet"));
    assert_eq!(Encoding::MacRoman.glyph(0x7f), None);
}

/// A table's text is its glyphs': `A` in the Latin encodings, a Greek letter
/// and a dingbat in their own, read from the list a font of theirs reads.
#[test]
fn encodings_give_their_glyphs_text() {
    assert_eq!(Encoding::Standard.text(b'A').as_deref(), Some("A"));
    assert_eq!(Encoding::MacRoman.text(0x80).as_deref(), Some("Ä"));
    assert_eq!(Encoding::Symbol.text(b'a').as_deref(), Some("α"));
    assert_eq!(Encoding::ZapfDingbats.text(0x34).as_deref(), Some("✔"));
    assert_eq!(Encoding::ZapfDingbats.text(0x7f), None);
    assert_eq!(Encoding::named(b"WinAnsiEncoding"), Some(Encoding::WinAnsi));
    assert_eq!(
        Encoding::named(b"StandardEncoding"),
        Some(Encoding::Standard)
    );
    assert_eq!(
        Encoding::named(b"MacRomanEncoding"),
        Some(Encoding::MacRoman)
    );
    assert_eq!(Encoding::named(b"MacExpertEncoding"), None);
    assert_eq!(Encoding::named(b"Symbol"), None);
}

/// WinAnsiEncoding is Windows-1252 at all 256 codes: each character it draws is its
/// code's text, and a control code has none, but that the codes above the space it
/// leaves empty draw the bullet (ISO 32000-2, D.2).
#[test]
fn win_ansi_is_cp1252_both_ways() {
    for code in 0..=255u8 {
        let c = cp1252_chart(code);
        let drawn = c.filter(|c| !c.is_control());
        let unused = code > 0x20 && drawn.is_none();
        let text = match unused {
            true => Some("•".to_string()),
            false => drawn.map(String::from),
        };
        assert_eq!(Encoding::WinAnsi.text(code), text, "{code:#x}");
        assert_eq!(cp1252(code), drawn, "{code:#x}");
        assert_eq!(
            Encoding::WinAnsi.glyph(code) == Some("bullet"),
            unused || code == 0x95,
            "{code:#x}"
        );
    }
    // The second space and the soft hyphen are their own.
    assert_eq!(Encoding::WinAnsi.text(0xa0).as_deref(), Some("\u{a0}"));
    assert_eq!(Encoding::WinAnsi.text(0xad).as_deref(), Some("\u{ad}"));
}

/// Each name PDFium takes for a font, in any case, after a subset's tag, and no
/// other.
#[test]
fn standard_14_by_pdfium_s_names() {
    let named = |n: &str| Std14::named(n.as_bytes());
    assert_eq!(named("Helvetica"), Some(Std14::Helvetica));
    assert_eq!(named("Arial,Bold"), Some(Std14::HelveticaBold));
    assert_eq!(named("ABCDEF+ArialMT"), Some(Std14::Helvetica));
    assert_eq!(named("arialmt"), Some(Std14::Helvetica));
    assert_eq!(
        named("TimesNewRomanPS-BoldItalicMT"),
        Some(Std14::TimesBoldItalic)
    );
    assert_eq!(named("CourierNew,Italic"), Some(Std14::CourierOblique));
    assert_eq!(named("SymbolMT"), Some(Std14::Symbol));
    assert_eq!(named("ArialNarrow"), None);
    assert_eq!(named("ABC+Arial"), None);
    for font in Std14::ALL {
        assert_eq!(named(font.name()), Some(font));
    }
}

/// Each AFM carries Adobe's notice as it's written and its own copyright, on both
/// the lines Adobe put it, and its second line says what was changed: the terms
/// the data is taken under.
#[test]
fn each_afm_holds_its_notices() {
    for font in Std14::ALL {
        let lines: Vec<&str> = font.afm_text().lines().collect();
        let name = font.name();
        assert_eq!(lines[0], "StartFontMetrics 4.1", "{name}");
        assert!(
            lines[1].starts_with("Comment Modified: the kerning data")
                && lines[1].contains("MustRead.html"),
            "{name}"
        );
        assert_eq!(
            lines[2].strip_prefix("Comment "),
            Some(Std14::NOTICE),
            "{name}"
        );
        let line = |key: &str| {
            let copyright = lines.iter().find_map(|l| l.strip_prefix(key));
            copyright.unwrap_or_else(|| panic!("{name} has no {key:?} line"))
        };
        let comment = line("Comment Copyright (c) 19");
        let notice = line("Notice Copyright (c) 19");
        assert!(comment.contains("Adobe Systems Incorporated"), "{name}");
        assert!(notice.starts_with(comment), "{name}");
    }
}

/// An AFM as `Afm::parse` reads one: a name and a whole width make a glyph, a box
/// is optional, a code past a byte's or none doesn't stop it, and nothing past
/// `EndCharMetrics` is read.
#[test]
fn an_afm_s_lines_read_as_they_are() {
    let afm = Afm::parse(
        "Comment Ascender 1\nAscender 700\nDescender -200\nFontBBox -1 -2 3 4 \n\
         StartCharMetrics 5\n\
         C 65 ; WX 600 ; N A ; B 1 2 3 4 ;\n\
         C 300 ; WX 500 ; N past ; B 5 6 7 8 ;\n\
         C -1 ; WX 400 ; N unencoded ;\n\
         C 66 ; WX 1.5 ; N real ; B 0 0 0 0 ;\n\
         C 67 ; WX 100 ;\n\
         EndCharMetrics\n\
         C 68 ; WX 900 ; N after ;\n",
    );
    assert_eq!((afm.ascender, afm.descender), (Some(700), Some(-200)));
    assert_eq!(afm.bbox, [-1, -2, 3, 4]);
    assert_eq!(afm.width("A"), Some(600));
    assert_eq!(afm.glyph_box("A"), Some([1, 2, 3, 4]));
    assert_eq!(afm.width("past"), Some(500));
    assert_eq!(afm.width("unencoded"), Some(400));
    assert_eq!(afm.glyph_box("unencoded"), Some([0; 4]));
    assert_eq!(afm.width("real"), None);
    assert_eq!(afm.width("after"), None);
    assert_eq!(afm.len(), 3);
}

/// Heights as the AFM gives them, the box's where it gives none; boxes per glyph.
#[test]
fn metrics_are_the_afm_s() {
    let helvetica = Std14::Helvetica.metrics();
    assert_eq!((helvetica.ascent, helvetica.descent), (718.0, -207.0));
    assert_eq!(
        helvetica.bbox,
        Some(Rect::new(-166.0, -225.0, 1000.0, 931.0))
    );
    let symbol = Std14::Symbol.metrics();
    assert_eq!((symbol.ascent, symbol.descent), (1010.0, -293.0));
    assert_eq!(
        Std14::Helvetica.glyph_box("A"),
        Some(Rect::new(14.0, 0.0, 654.0, 718.0))
    );
    assert_eq!(Std14::Helvetica.glyph_box("no such glyph"), None);
    assert_eq!(Rect::new(3.0, 4.0, 1.0, 2.0), Rect::new(1.0, 2.0, 3.0, 4.0));
}

/// The space and the glyphs PDFium's own faces draw otherwise: Latin, not
/// Courier's, and each of the few of its own.
#[test]
fn glyphs_drawn_otherwise() {
    assert!(Std14::Helvetica.drawn_otherwise("Abreve"));
    assert!(Std14::TimesItalic.drawn_otherwise("zdotaccent"));
    assert!(!Std14::Courier.drawn_otherwise("Abreve"));
    assert!(!Std14::Symbol.drawn_otherwise("lozenge"));
    assert!(Std14::Helvetica.drawn_otherwise("guillemotleft"));
    assert!(!Std14::HelveticaBold.drawn_otherwise("guillemotleft"));
    assert!(!Std14::Helvetica.drawn_otherwise("A"));
    // The Euro is told by its box, which Adobe's metrics leave empty.
    assert!(!Std14::Helvetica.drawn_otherwise("Euro"));
    assert_eq!(Std14::Helvetica.glyph_box("Euro"), Some(Rect::default()));
}

/// Glyph names read by the Adobe Glyph List's rules: a name's own text, the parts
/// an underscore joins, what follows a period passed over, `uni` and `u`, a
/// Dingbats name only in a font of them; and anything but ASCII after `uni` no
/// text, four bytes of it not being whole characters.
#[test]
fn glyph_names_read_as_text() {
    let text = |name, dingbats| glyph_text(name, dingbats);
    assert_eq!(text("A", false).as_deref(), Some("A"));
    assert_eq!(text("f_i", false).as_deref(), Some("fi"));
    assert_eq!(text("a.alt", false).as_deref(), Some("a"));
    assert_eq!(text("uni0416", false).as_deref(), Some("Ж"));
    assert_eq!(text("uni00410042", false).as_deref(), Some("AB"));
    assert_eq!(text("u1F600", false).as_deref(), Some("😀"));
    assert_eq!(text("a20", true).as_deref(), Some("✔"));
    assert_eq!(text("a20", false), None);
    assert_eq!(text("uniD800", false), None);
    assert_eq!(text("uni€€€€", false), None);
    assert_eq!(text("", false), None);
    assert_eq!(name_code_point(b"uni0416"), Some((0x416, false)));
    assert_eq!(name_code_point(b"uni0416.alt"), Some((0x416, true)));
    assert_eq!(name_code_point(b"u1F600"), Some((0x1f600, false)));
    assert_eq!(name_code_point(b"A"), Some((0x41, false)));
    assert_eq!(name_code_point(b"a.alt"), Some((0x61, true)));
    assert_eq!(name_code_point(b".notdef"), None);
    assert_eq!(name_code_point(b"f_i"), None);
    assert_eq!(name_code_point(b"\xff"), None);
}

/// What the crate says it gives, by those names; and pdf-objects' and pdf-content-streams'
/// own types, what it re-exports of them.
#[test]
fn the_crate_s_names_are_there() {
    #[allow(unused_imports)]
    use crate::{
        Decoded, Descendant, Encoding, Error, Fault, Font, FontApprox, FontId, FontKind,
        FontMetrics, FontProgram, FontScope, Fonts, Limits, Place, Rect, Sink, Std14, Streams,
        VerticalMetrics, With, cp1252, glyph_text, name_code_point,
        type1::{Charstrings, scan},
    };
    fn same<T>(_: fn(&T) -> &T) {}
    same::<pdf_objects::Dict>(|d: &crate::Dict| d);
    same::<pdf_objects::Object>(|d: &crate::Object| d);
    same::<pdf_objects::ObjectId>(|d: &crate::ObjectId| d);
    same::<pdf_objects::Span>(|d: &crate::Span| d);
    same::<pdf_objects::Unreadable>(|d: &crate::Unreadable| d);
    same::<pdf_objects::Why>(|d: &crate::Why| d);
    same::<pdf_content_streams::Limits>(|d: &crate::pdf_content_streams::Limits| d);
    fn resolves<'o, R: pdf_objects::Resolve<'o>>() {}
    fn also<'o, R: crate::Resolve<'o>>() {
        resolves::<R>();
    }
    let _ = also::<fn(crate::ObjectId) -> Option<&'static crate::Object>>;
}

/// [`fixture::type1_program`] with `clear` in its clear text, before `/PaintType`.
fn type1_holding(clear: &str) -> Vec<u8> {
    let program = fixture::type1_program();
    let at = program.windows(4).position(|w| w == b"/Pai").unwrap();
    [&program[..at], clear.as_bytes(), b"\n", &program[at..]].concat()
}

/// A Type 1 program's clear text read as read-fonts' parser reads it: by what each of
/// these holds, it finds `eexec` and the charstrings behind it, or doesn't. Each is what a
/// parser that reads a procedure by its tokens, or looks past an integer by its tokens,
/// reads otherwise; `programs` holds it to read-fonts' own reading.
#[test]
fn type1_tokens_are_read_as_read_fonts_reads_them() {
    let cases: &[(&str, bool, &str)] = &[
        ("/N { { } } def", true, "spaces inside braces"),
        ("/N {{}} def", true, "no spaces"),
        ("/N { % {\n } def", true, "a brace in a comment"),
        ("/N { (}) } def", true, "a brace in a string"),
        ("/N { (\\)}) } def", true, "an escape in a string"),
        ("/N { ( } def", false, "a string unended in a procedure"),
        ("/N { { } def", false, "a procedure unended"),
        (
            "/N (a)/N (b)/O def",
            true,
            "a string followed at once by a name",
        ),
        (
            "/N (a)currentfile eexec",
            false,
            "`eexec` straight after a string, what follows it no program",
        ),
        ("% currentfile eexec\n/N def", true, "`eexec` in a comment"),
        ("/N (currentfile eexec) def", true, "`eexec` in a string"),
    ];
    let limits = Limits::DEFAULT;
    for (clear, read, what) in cases {
        let program = type1_holding(clear);
        assert_eq!(
            type1::scan(&program, &limits).is_some(),
            *read,
            "{what}: {clear}"
        );
        #[cfg(feature = "programs")]
        assert_eq!(
            skrifa::raw::ps::type1::Type1Font::new(&program).is_ok(),
            *read,
            "read-fonts, {what}: {clear}"
        );
    }
    // A string's `)` and what's straight after it: a key read as a key.
    let empty = fixture::type1_empty_glyph();
    let private = [
        b"/lenIV -1 def\n/X (a)/CharStrings 1 dict dup begin\n/.notdef ".to_vec(),
        format!("{} RD ", empty.len()).into_bytes(),
        empty,
        b" ND\nend\n".to_vec(),
    ]
    .concat();
    let program = fixture::type1_private(b"", &private);
    assert!(type1::scan(&program, &limits).is_some());
    #[cfg(feature = "programs")]
    assert!(skrifa::raw::ps::type1::Type1Font::new(&program).is_ok());
    // However deep its procedures nest, and however many integers run together, each is
    // read in time that grows as the program does, and it stays a program.
    for program in [
        type1_holding(&format!(
            "/N {}{}def",
            "{ ".repeat(1 << 16),
            "} ".repeat(1 << 16)
        )),
        fixture::type1_integers_program(1 << 16),
    ] {
        assert!(type1::scan(&program, &limits).is_some());
    }
}

/// An integer before `RD` or `-|` is a binary blob's length, only where nothing but
/// whitespace is between them, as read-fonts looks ahead.
#[test]
fn a_type1_blob_s_length_is_followed_only_by_whitespace() {
    let glyph = |between: &str| {
        let data = fixture::type1_empty_glyph();
        let private = [
            b"/lenIV -1 def\n/CharStrings 1 dict dup begin\n/.notdef ".to_vec(),
            format!("{}{between}RD ", data.len()).into_bytes(),
            data,
            b" ND\nend\n".to_vec(),
        ]
        .concat();
        fixture::type1_private(b"", &private)
    };
    let limits = Limits::DEFAULT;
    assert!(type1::scan(&glyph(" "), &limits).is_some());
    assert!(type1::scan(&glyph(" \t\r\n "), &limits).is_some());
    assert!(type1::scan(&glyph(" % a comment\n"), &limits).is_none());
}

/// A glyph's name that isn't UTF-8 is its bytes as Latin-1, as read-fonts keeps it, and
/// two names that come to one are a name given twice.
#[test]
fn a_type1_name_s_bytes_as_read_fonts_keeps_them() {
    let empty = fixture::type1_empty_glyph();
    let program = |names: &[&[u8]]| {
        let mut private = b"/lenIV -1 def\n/CharStrings 1 dict dup begin\n".to_vec();
        for name in names {
            private.extend([b"/", *name].concat());
            private.extend(format!(" {} RD ", empty.len()).bytes());
            private.extend(&empty);
            private.extend(b" ND\n");
        }
        private.extend(b"end\n");
        fixture::type1_private(b"", &private)
    };
    let limits = Limits::DEFAULT;
    let found = type1::scan(&program(&[b"caf\xe9", b"plain"]), &limits).unwrap();
    assert!(found.get("caf\u{e9}".as_bytes()).unwrap().is_some());
    assert!(!found.contains_key(&b"caf\xe9"[..]));
    assert!(found.get(&b"plain"[..]).unwrap().is_some());
    let found = type1::scan(&program(&[b"caf\xe9", "caf\u{e9}".as_bytes()]), &limits).unwrap();
    assert_eq!(found.get("caf\u{e9}".as_bytes()), Some(&None));
}

/// The names of an `/Encoding` count, each among all the glyphs, as read-fonts looks for
/// each among them one by one, whichever form it has; a real one's are within what's
/// allowed, and past that it isn't read.
#[test]
fn a_type1_encoding_s_names_are_counted_among_its_glyphs() {
    let limits = Limits::DEFAULT;
    assert!(type1::scan(&fixture::type1_lookups_program(256), &limits).is_some());
    assert!(type1::scan(&fixture::type1_lookups_program(3000), &limits).is_none());
    let glyphs = 3000;
    let names: Vec<String> = (0..glyphs).map(|g| format!("g{g}")).collect();
    let empty = fixture::type1_empty_glyph();
    let program = |clear: String| {
        fixture::type1_with(
            clear.as_bytes(),
            (names.iter()).map(|name| (name.as_str(), empty.clone())),
        )
    };
    let in_a_map = |n: usize| {
        format!(
            "/Encoding 256 array\n{}readonly def",
            "dup 0 /nope put\n".repeat(n)
        )
    };
    assert!(type1::scan(&program(in_a_map(256)), &limits).is_some());
    assert!(type1::scan(&program(in_a_map(glyphs)), &limits).is_none());
    // A map's codes and an array's names come to the same.
    let in_an_array = |n: usize| format!("/Encoding [{}] def", "/nope ".repeat(n));
    assert!(type1::scan(&program(in_an_array(256)), &limits).is_some());
    assert!(type1::scan(&program(in_an_array(glyphs)), &limits).is_none());
    // What's no name ends an array, and `def` a map.
    let cut = format!("/Encoding [ 1 {}] def", "/nope ".repeat(glyphs));
    assert!(type1::scan(&program(cut), &limits).is_some());
    let ended = format!("/Encoding 256 array def {}", "1 /nope ".repeat(glyphs));
    assert!(type1::scan(&program(ended), &limits).is_some());
    // A predefined one looks nothing up, nor a name that follows no key.
    assert!(type1::scan(&program("/Encoding StandardEncoding def".into()), &limits).is_some());
    let unkeyed = format!("[{}]", "/nope ".repeat(glyphs));
    assert!(type1::scan(&program(unkeyed), &limits).is_some());
    // Each is counted: two of what one may have aren't read.
    assert!(type1::scan(&program(in_an_array(800)), &limits).is_some());
    let twice = format!("{0} {0}", in_an_array(800));
    assert!(type1::scan(&program(twice), &limits).is_none());
    // A glyph is a glyph, whatever its name: read-fonts goes through them all.
    let clear = in_an_array(glyphs);
    let once = fixture::type1_with(
        clear.as_bytes(),
        std::iter::repeat_n(("g", empty.clone()), glyphs),
    );
    assert!(type1::scan(&once, &limits).is_none());
}

/// What a scan finds: each charstring by name, decrypted and past `lenIV`'s bytes.
/// A program of no work to spare isn't read, and neither is what's no program.
#[test]
fn type1_charstrings_found() {
    let limits = Limits::DEFAULT;
    let found = type1::scan(&fixture::type1_program(), &limits).unwrap();
    let mut names: Vec<&[u8]> = found.keys().map(Vec::as_slice).collect();
    names.sort_unstable();
    assert_eq!(names, [&b".notdef"[..], b"A", b"g", b"space"]);
    // `0 600 hsbw`, first, and `endchar`, last.
    let a = found[&b"A"[..]].as_ref().unwrap();
    assert_eq!(a[..4], [139, 248, 236, 13]);
    assert_eq!(a.last(), Some(&14));
    let no_work = Limits {
        type1_work: 0,
        ..limits
    };
    assert!(type1::scan(&fixture::type1_program(), &no_work).is_none());
    assert!(type1::scan(b"", &limits).is_none());
    assert!(type1::scan(b"%!PS-AdobeFont-1.0: no eexec\n", &limits).is_none());
}
