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
            // Printable ASCII, 27 of 0x80–0x9F, and 0xA0–0xFF.
            Std14::Symbol | Std14::ZapfDingbats => codes.len(),
            _ => 95 + 27 + 96,
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
    assert_eq!(Encoding::MacRoman.glyph(0xf0), None);
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

/// WinAnsiEncoding is Windows-1252 exactly, all 256 codes: each character it
/// draws is its code's text, and a control code or an empty one has none.
#[test]
fn win_ansi_is_cp1252_both_ways() {
    for code in 0..=255u8 {
        let c = cp1252_chart(code);
        let drawn = c.filter(|c| !c.is_control());
        assert_eq!(
            Encoding::WinAnsi.text(code),
            drawn.map(String::from),
            "{code:#x}"
        );
        assert_eq!(cp1252(code), drawn, "{code:#x}");
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

/// A Type 1 program with procedures `levels` deep.
fn type1_nested(levels: usize) -> Vec<u8> {
    let program = fixture::type1_program();
    let at = program.windows(4).position(|w| w == b"/Pai").unwrap();
    let nested = format!(
        "/Nested {}{}def\n",
        "{ ".repeat(levels),
        "} ".repeat(levels)
    );
    [&program[..at], nested.as_bytes(), &program[at..]].concat()
}

/// A Type 1 program whose procedures nest as deep as the scan allows is
/// read; one a level deeper isn't, and neither is one past a smaller cap.
#[test]
fn type1_procedures_nest_within_the_scan_s_depth() {
    let limits = Limits::DEFAULT;
    let depth = limits.proc_depth;
    assert!(type1::scan(&type1_nested(depth), &limits).is_some());
    assert!(type1::scan(&type1_nested(depth + 1), &limits).is_none());
    let smaller = Limits {
        proc_depth: 3,
        ..limits
    };
    assert!(type1::scan(&type1_nested(3), &smaller).is_some());
    assert!(type1::scan(&type1_nested(4), &smaller).is_none());
}

/// A Type 1 program whose integers run as long as the scan reads ahead of
/// is read; one an integer longer isn't, nor a run of a hundred thousand,
/// which the scan refuses within its stack.
#[test]
fn type1_integers_run_within_the_scan_s_lookahead() {
    use fixture::type1_integers_program as type1_integers;
    let limits = Limits::DEFAULT;
    let ahead = limits.ints_ahead;
    assert!(type1::scan(&type1_integers(ahead), &limits).is_some());
    assert!(type1::scan(&type1_integers(ahead + 1), &limits).is_none());
    assert!(type1::scan(&type1_integers(100_000), &limits).is_none());
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
