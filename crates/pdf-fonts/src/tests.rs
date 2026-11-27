//! The fonts' data: widths that are Adobe's, the encodings' tables each equal to
//! its AFM, and Adobe's notices carried; and a Type 1 program's scan.

use std::collections::HashMap;

use super::afm::Afm;
use super::fixture;
use super::type1::Refusal::{Capped, Malformed};
use super::*;

#[cfg(feature = "embed")]
mod embed;
mod fonts;
mod shown;
#[cfg(feature = "embed")]
mod written;

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

/// Each encoding's code for a character reads back as it, and every code whose text
/// is one character is found by it: that code, or a lower one of the same text. In
/// WinAnsiEncoding, a code Windows-1252 has a character at, so the bullet is 0x95.
#[test]
fn each_character_s_code_reads_back_as_it() {
    use Encoding::{MacRoman, Standard, Symbol, WinAnsi, ZapfDingbats};
    for encoding in [Standard, WinAnsi, MacRoman, Symbol, ZapfDingbats] {
        for code in 0..=255u8 {
            let Some(text) = encoding.text(code) else {
                continue;
            };
            let mut chars = text.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                continue;
            };
            let found = encoding.code(c);
            let found = found.unwrap_or_else(|| panic!("{encoding:?} {code:#x} {c:?}"));
            let windows = encoding != WinAnsi || cp1252(code).is_some();
            assert!(found <= code || !windows, "{encoding:?} {code:#x}");
            assert_eq!(encoding.text(found), Some(text), "{encoding:?} {code:#x}");
        }
        for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
            if let Some(code) = encoding.code(c) {
                assert_eq!(encoding.text(code), Some(String::from(c)), "{c:?}");
            }
        }
    }
    let drawn = |encoding: Encoding| {
        let chars = (0..=0x10_FFFF).filter_map(char::from_u32);
        chars.filter(|&c| encoding.code(c).is_some()).count()
    };
    // Windows-1252's 218 characters from the space up; Symbol's 189 glyphs and
    // ZapfDingbats' 202, each its own character.
    assert_eq!(
        [drawn(WinAnsi), drawn(Symbol), drawn(ZapfDingbats)],
        [218, 189, 202]
    );
    assert_eq!(WinAnsi.code('•'), Some(0x95));
    assert_eq!(WinAnsi.code('\u{a0}'), Some(0xa0));
    assert_eq!(WinAnsi.code(' '), Some(0x20));
    assert_eq!(MacRoman.code(' '), Some(0x20));
    // Symbol's Greek letters whose names the glyph list reads as signs.
    assert_eq!(Symbol.code('\u{394}'), None);
    assert_eq!(Symbol.code('\u{2206}'), Some(b'D'));
}

/// Every code of the encoding a standard 14 font's dictionary names reads, in the
/// font that dictionary is, as its encoding's text, and that text is written in it;
/// so text of every character it draws, alone and together, reads back as given.
#[test]
fn a_standard_14_font_s_codes_read_back_as_its_text() {
    for font in Std14::ALL {
        let dict = font.dict();
        let read = Font::read(
            crate::fonts::NoObjects,
            &dict,
            &Limits::DEFAULT,
            &mut 0,
            &mut (),
        );
        let read = read.unwrap();
        let encoding = font.written_in();
        let mut all = String::new();
        for code in 0..=255u8 {
            let text = encoding.text(code);
            assert_eq!(read.text(&[code]), text, "{font:?} {code:#x}");
            let Some(text) = text else {
                continue;
            };
            let codes = font.codes(&text).unwrap();
            assert_eq!(
                read.text(&codes).as_ref(),
                Some(&text),
                "{font:?} {code:#x}"
            );
            if codes == [code] {
                all.push_str(&text);
            }
        }
        let codes = font.codes(&all).unwrap();
        assert_eq!(codes.len(), all.chars().count());
        assert_eq!(read.text(&codes), Some(all), "{font:?}");
    }
}

/// A character no code draws is refused where it is, never written as another; so
/// is a combining accent after its letter, which the letter composed isn't.
#[test]
fn a_character_with_no_code_is_refused_never_replaced() {
    let refused = |font: Std14, text: &str| font.codes(text).unwrap_err();
    let han = refused(Std14::Helvetica, "a中文b");
    assert_eq!(
        (han.at, han.character, han.font),
        (1, '中', Std14::Helvetica)
    );
    let words = "U+4E2D at byte 1 has no code in Helvetica's encoding";
    assert_eq!(han.to_string(), words);
    assert_eq!(refused(Std14::TimesRoman, "e\u{301}").at, 1);
    assert_eq!(Std14::TimesRoman.codes("é"), Ok(vec![0xe9]));
    assert_eq!(refused(Std14::Symbol, "αx").character, 'x');
    assert_eq!(Std14::ZapfDingbats.codes("✔✘"), Ok(b"48".to_vec()));
    assert_eq!(refused(Std14::ZapfDingbats, "✔x").at, 3);
    assert_eq!(refused(Std14::Courier, "a\nb").character, '\n');
    assert_eq!(Std14::Courier.codes(""), Ok(Vec::new()));
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

/// The two files whose terms ask that they go with what they cover: Adobe's, with
/// its notice as each AFM has it, and FreeType's, whole.
#[test]
fn the_notice_files_are_carried() {
    let must_read = include_str!("../assets/afm/MustRead.html");
    assert!(must_read.contains(Std14::NOTICE));
    let ftl = include_str!("../FTL.TXT");
    assert!(ftl.starts_with("                    The FreeType Project LICENSE"));
    assert!(ftl.ends_with("--- end of FTL.TXT ---\n"));
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
        Some(GlyphBox::new(-166.0, -225.0, 1000.0, 931.0))
    );
    let symbol = Std14::Symbol.metrics();
    assert_eq!((symbol.ascent, symbol.descent), (1010.0, -293.0));
    assert_eq!(
        Std14::Helvetica.glyph_box("A"),
        Some(GlyphBox::new(14.0, 0.0, 654.0, 718.0))
    );
    assert_eq!(Std14::Helvetica.glyph_box("no such glyph"), None);
    assert_eq!(
        GlyphBox::new(3.0, 4.0, 1.0, 2.0),
        GlyphBox::new(1.0, 2.0, 3.0, 4.0)
    );
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
    assert_eq!(
        Std14::Helvetica.glyph_box("Euro"),
        Some(GlyphBox::default())
    );
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

/// What the crate says it gives, by those names; and pdf-objects and pdf-content-streams,
/// re-exported whole, each the crate itself.
#[test]
fn the_crate_s_names_are_there() {
    #[allow(unused_imports)]
    use crate::{
        Codes, Descendant, Encoding, Error, Fault, Font, FontApprox, FontId, FontKind, FontMetrics,
        FontProgram, FontScope, Fonts, GlyphBox, Limits, Std14, VerticalMetrics, cp1252,
        glyph_text, name_code_point,
    };
    fn same<T>(_: fn(&T) -> &T) {}
    same::<pdf_objects::Dict>(|d: &crate::pdf_objects::Dict| d);
    same::<pdf_objects::Unreadable>(|d: &crate::pdf_objects::Unreadable| d);
    same::<pdf_content_streams::Limits>(|d: &crate::pdf_content_streams::Limits| d);
    fn resolves<'o, R: pdf_objects::Resolve<'o>>() {}
    fn also<'o, R: crate::pdf_objects::Resolve<'o>>() {
        resolves::<R>();
    }
    let _ = also::<
        pdf_objects::ResolveFn<fn(pdf_objects::ObjectId) -> Option<&'static pdf_objects::Object>>,
    >;
}

/// `Std14::ALL` is the 14 in the order of their discriminants, so each is there once,
/// at its own index.
#[test]
fn all_14_are_in_the_order_of_their_discriminants() {
    for (i, font) in Std14::ALL.into_iter().enumerate() {
        assert_eq!(font as usize, i, "{font:?}");
    }
}

/// What's `const` makes a constant: a rectangle and metrics, whether a font guesses, a
/// standard font's name, a Windows-1252 character and an encoding by its name.
#[test]
fn const_fns_make_constants() {
    const BOX: GlyphBox = GlyphBox::new(10.0, 5.0, -10.0, -5.0);
    const METRICS: FontMetrics = FontMetrics::new(800.0, -200.0, Some(BOX));
    const EXACT: FontApprox = FontApprox {
        codes: false,
        widths: false,
        heights: false,
        boxes: false,
    };
    const WIDTHS: FontApprox = FontApprox {
        widths: true,
        ..EXACT
    };
    const GUESSED: [bool; 2] = [WIDTHS.any(), EXACT.any()];
    const NAME: &str = Std14::TimesBoldItalic.name();
    const EURO: Option<char> = cp1252(0x80);
    const EMPTY: Option<char> = cp1252(0x81);
    const WIN_ANSI: Option<Encoding> = Encoding::named(b"WinAnsiEncoding");
    assert_eq!((BOX.x0, BOX.y0, BOX.x1, BOX.y1), (-10.0, -5.0, 10.0, 5.0));
    assert_eq!(METRICS.bbox, Some(BOX));
    assert_eq!(GUESSED, [true, false]);
    assert_eq!(NAME, "Times-BoldItalic");
    assert_eq!((EURO, EMPTY), (Some('€'), None));
    assert_eq!(WIN_ANSI, Some(Encoding::WinAnsi));
}

/// The README is shown on crates.io and docs.rs, where a path beside the crate
/// reaches nothing: its links to the licences, the root's and the FreeType Project
/// License, and to Adobe's notice for the metrics are the repository's URLs, the
/// manifest's `repository` and the file, each file the crate's own where it's in
/// the crate; and every link it has is a URL.
#[test]
fn the_readmes_licence_links_reach_the_files() {
    let readme = include_str!("../README.md");
    let repository = env!("CARGO_PKG_REPOSITORY");
    for file in ["LICENSE-APACHE", "LICENSE-MIT"] {
        let link = format!("[{file}]({repository}/blob/main/{file})");
        assert!(
            readme.contains(&link),
            "{file}'s link isn't the repository's URL"
        );
    }
    let crate_dir = format!("crates/{}", env!("CARGO_PKG_NAME"));
    for file in ["FTL.TXT", "assets/afm/MustRead.html"] {
        let link = format!("[`{file}`]({repository}/blob/main/{crate_dir}/{file})");
        assert!(
            readme.contains(&link),
            "{file}'s link isn't the repository's URL"
        );
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
        assert!(here.is_file(), "{file} isn't in the crate");
    }
    for (at, _) in readme.match_indices("](") {
        let target = &readme[at + 2..];
        let target = &target[..target.find(')').unwrap_or(target.len())];
        assert!(
            target.starts_with("https://"),
            "a link isn't a URL: {target}"
        );
    }
}

/// The crate doc's example is the README's, line for line, so that what docs.rs shows
/// and what crates.io shows are one example, and each runs as a doctest, in every build.
#[test]
fn the_crate_docs_example_is_the_readmes() {
    let readme: Vec<&str> = (include_str!("../README.md").lines())
        .skip_while(|line| *line != "```rust")
        .skip(1)
        .take_while(|line| *line != "```")
        .collect();
    let doc: Vec<&str> = (include_str!("lib.rs").lines())
        .map_while(|line| line.strip_prefix("//!"))
        .map(|line| line.strip_prefix(' ').unwrap_or(line))
        .skip_while(|line| *line != "```")
        .skip(1)
        .take_while(|line| *line != "```")
        .collect();
    assert!(readme.len() > 10, "the README's example: {readme:?}");
    assert_eq!(doc, readme);
}

/// [`fixture::type1_program`] with `clear` in its clear text, before `/PaintType`.
fn type1_holding(clear: &str) -> Vec<u8> {
    let program = fixture::type1_program();
    let at = program.windows(4).position(|w| w == b"/Pai").unwrap();
    [&program[..at], clear.as_bytes(), b"\n", &program[at..]].concat()
}

/// A Type 1 program's clear text read as read-fonts' parser reads it: by what each of
/// these holds, it finds `eexec` and the charstrings behind it, or doesn't. Each is what
/// a parser that reads a procedure by its tokens, or looks past an integer by its tokens,
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
    assert_eq!(refused(&glyph(" % a comment\n"), &limits), Some(Malformed));
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
    assert_eq!(
        refused(&fixture::type1_lookups_program(3000), &limits),
        Some(Capped)
    );
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
    assert_eq!(refused(&program(in_a_map(glyphs)), &limits), Some(Capped));
    // A map's codes and an array's names come to the same.
    let in_an_array = |n: usize| format!("/Encoding [{}] def", "/nope ".repeat(n));
    assert!(type1::scan(&program(in_an_array(256)), &limits).is_some());
    assert!(type1::scan(&program(in_an_array(glyphs)), &limits).is_none());
    assert_eq!(
        refused(&program(in_an_array(glyphs)), &limits),
        Some(Capped)
    );
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
    assert!(type1::scan(&program(twice.clone()), &limits).is_none());
    assert_eq!(refused(&program(twice), &limits), Some(Capped));
    // A glyph is a glyph, whatever its name: read-fonts goes through them all.
    let clear = in_an_array(glyphs);
    let once = fixture::type1_with(
        clear.as_bytes(),
        std::iter::repeat_n(("g", empty.clone()), glyphs),
    );
    assert!(type1::scan(&once, &limits).is_none());
    assert_eq!(refused(&once, &limits), Some(Capped));
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
    assert_eq!(refused(&fixture::type1_program(), &no_work), Some(Capped));
    assert!(type1::scan(b"", &limits).is_none());
    assert_eq!(refused(b"", &limits), Some(Malformed));
    let no_eexec = b"%!PS-AdobeFont-1.0: no eexec\n";
    assert!(type1::scan(no_eexec, &limits).is_none());
    assert_eq!(refused(no_eexec, &limits), Some(Malformed));
    // Whose fuel's spent before it's found broken, whatever it is: no work to spare.
    assert_eq!(refused(no_eexec, &no_work), Some(Capped));
    assert_eq!(refused(b"", &no_work), Some(Malformed));
}

/// Why [`type1::read`] refuses `program` under `limits`: none where it reads it.
fn refused(program: &[u8], limits: &Limits) -> Option<type1::Refusal> {
    type1::read(program, limits).err()
}

#[test]
fn classify_style_strips_subset_tags_excludes_monotype_and_emits_no_faults() {
    use pdf_objects::{Object, ObjectId, Stream, Unreadable, Why, WithStreams, dict, resolve_fn};

    let mut objs = HashMap::<ObjectId, Object>::new();
    // Non-numeric `/FontWeight /Bold` and `/Flags /Bogus` on Type 0 / GBK descriptors must emit
    // zero new faults when `classify_style` inspects them!
    objs.insert(
        ObjectId::new(10, 0),
        Object::from(dict! {
            "Type" => Object::name("FontDescriptor"),
            "FontBBox" => vec![Object::from(0), Object::from(-200), Object::from(800), Object::from(800)],
            "Ascent" => 800,
            "Descent" => -200,
            "FontWeight" => Object::name("Bold"),
            "Flags" => Object::name("Bogus"),
        }),
    );
    objs.insert(
        ObjectId::new(11, 0),
        Object::from(dict! {
            "Type" => Object::name("Font"),
            "Subtype" => Object::name("CIDFontType2"),
            "BaseFont" => Object::name("ABBOLD+MicrosoftSansSerif"),
            "FontDescriptor" => Object::reference(10),
        }),
    );
    // Type 3 `/CharProcs` streams: `d0` (colored) vs `d1` (uncolored).
    objs.insert(
        ObjectId::new(20, 0),
        Object::from(Stream::new(dict! {}, b"500 0 d0 0 0 400 700 re f".to_vec())),
    );
    objs.insert(
        ObjectId::new(21, 0),
        Object::from(Stream::new(
            dict! {},
            b"500 0 0 0 400 700 d1 0 0 400 700 re f".to_vec(),
        )),
    );

    let streams = WithStreams::new(resolve_fn(|id| objs.get(&id)), |id, limit| {
        let Some(s) = objs.get(&id).and_then(Object::as_stream) else {
            return Err(Unreadable::new(id, Why::Damaged, "missing"));
        };
        let bytes = s.data.bytes().unwrap();
        if bytes.len() > limit {
            return Err(Unreadable::new(id, Why::Limit, "past limit"));
        }
        Ok(bytes.to_vec())
    });

    // 1. Type 0 with `ABBOLD+MicrosoftSansSerif` + `/FontWeight /Bold` + `/Flags /Bogus`:
    //    - `ABBOLD+` subset prefix stripped (`bold == false`);
    //    - `Sans` excluded from `Serif` (`serif == false`);
    //    - ZERO faults emitted!
    let t0_dict = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("Type0"),
        "BaseFont" => Object::name("ABBOLD+MicrosoftSansSerif"),
        "Encoding" => Object::name("Identity-H"),
        "DescendantFonts" => vec![Object::reference(11)],
    };
    let mut faults = Vec::new();
    let mut budget = 1 << 20;
    let t0 = Font::read(
        streams,
        &t0_dict,
        &Limits::DEFAULT,
        &mut budget,
        &mut faults,
    )
    .unwrap();
    assert!(faults.is_empty(), "unexpected faults: {faults:?}");
    assert!(!t0.style().bold, "subset tag ABBOLD+ must not trigger bold");
    assert!(
        !t0.style().serif,
        "MicrosoftSansSerif must be sans-serif, not serif"
    );
    assert!(!t0.style().fixed_pitch);

    // 2. `MonotypeCorsiva` is NOT fixed-pitch, whereas `DejaVuSansMono-Oblique` IS fixed-pitch + italic + sans!
    let monotype_dict = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("TrueType"),
        "BaseFont" => Object::name("ABCDEF+MonotypeCorsiva"),
        "FirstChar" => 32,
        "LastChar" => 32,
        "Widths" => vec![Object::from(500)],
    };
    let monotype = Font::read(
        streams,
        &monotype_dict,
        &Limits::DEFAULT,
        &mut budget,
        &mut (),
    )
    .unwrap();
    assert!(
        !monotype.style().fixed_pitch,
        "MonotypeCorsiva must not be classified as fixed_pitch"
    );

    let mono_dict = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("TrueType"),
        "BaseFont" => Object::name("XYZWTQ+DejaVuSansMono-BoldOblique"),
        "FirstChar" => 32,
        "LastChar" => 32,
        "Widths" => vec![Object::from(600)],
    };
    let mono = Font::read(streams, &mono_dict, &Limits::DEFAULT, &mut budget, &mut ()).unwrap();
    assert!(mono.style().fixed_pitch);
    assert!(!mono.style().serif);
    assert!(mono.style().bold);
    assert!(mono.style().italic);

    // 3. `Type3Glyph::uncolored` distinguishes `d0` (`false`) from `d1` (`true`).
    let t3_dict = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("Type3"),
        "FontBBox" => vec![Object::from(0), Object::from(0), Object::from(500), Object::from(700)],
        "FontMatrix" => vec![
            Object::from(0.001), Object::from(0), Object::from(0),
            Object::from(0.001), Object::from(0), Object::from(0),
        ],
        "Encoding" => dict! {
            "Differences" => vec![Object::from(65), Object::name("A"), Object::name("B")],
        },
        "CharProcs" => dict! {
            "A" => Object::reference(20),
            "B" => Object::reference(21),
        },
    };
    let t3 = Font::read(streams, &t3_dict, &Limits::DEFAULT, &mut budget, &mut ()).unwrap();
    let ga = t3.type3_glyph(65).unwrap();
    let gb = t3.type3_glyph(66).unwrap();
    assert!(!ga.uncolored);
    assert!(gb.uncolored);

    #[cfg(not(feature = "programs"))]
    assert!(FallbackProgram::open(fixture::truetype_program(), &Limits::DEFAULT).is_none());
}

#[cfg(feature = "programs")]
#[test]
fn outline_bounds_match_char_box_with_shifted_matrices_and_fallback_advances_are_exact() {
    use pdf_objects::{Object, ObjectId, Stream, Unreadable, Why, WithStreams, dict, resolve_fn};

    fn verb_control_bounds(verbs: &[OutlineVerb]) -> Option<GlyphBox> {
        let mut bbox: Option<[f32; 4]> = None;
        let mut add = |x: f32, y: f32| {
            let b = bbox.get_or_insert([x, y, x, y]);
            *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        };
        for &v in verbs {
            match v {
                OutlineVerb::MoveTo(x, y) | OutlineVerb::LineTo(x, y) => add(x, y),
                OutlineVerb::QuadTo(cx, cy, x, y) => {
                    add(cx, cy);
                    add(x, y);
                }
                OutlineVerb::CurveTo(cx0, cy0, cx1, cy1, x, y) => {
                    add(cx0, cy0);
                    add(cx1, cy1);
                    add(x, y);
                }
                OutlineVerb::Close => {}
            }
        }
        bbox.map(|[x0, y0, x1, y1]| GlyphBox::new(x0, y0, x1, y1))
    }

    // Shift Type 1 `/FontMatrix` by `(dx = 0.1, dy = 0.05)` (`+100`, `+50` glyph units!)
    let mut shifted_t1 = fixture::type1_program();
    let old_mat = b"/FontMatrix [0.001 0 0 0.001 0 0]";
    let new_mat = b"/FontMatrix [0.001 0 0 0.001 0.1 0.05]";
    let pos = shifted_t1
        .windows(old_mat.len())
        .position(|w| w == old_mat)
        .unwrap();
    shifted_t1.splice(pos..pos + old_mat.len(), new_mat.iter().copied());

    // Shift CID-CFF `/FontMatrix` by `(dx = 0.12, dy = -0.04)` (`+120`, `-40` glyph units!)
    let shifted_cid_cff =
        fixture::cid_cff_with(Some(["0.001", "0", "0", "0.001", "0.12", "-0.04"]), None);

    let mut objs = HashMap::<ObjectId, Object>::new();
    objs.insert(
        ObjectId::new(1, 0),
        Object::from(Stream::new(dict! {}, fixture::truetype_program())),
    );
    objs.insert(
        ObjectId::new(2, 0),
        Object::from(Stream::new(dict! {}, shifted_t1)),
    );
    objs.insert(
        ObjectId::new(3, 0),
        Object::from(Stream::new(
            dict! { "Subtype" => Object::name("CIDFontType0C") },
            shifted_cid_cff,
        )),
    );
    objs.insert(
        ObjectId::new(4, 0),
        Object::from(dict! {
            "Type" => Object::name("FontDescriptor"),
            "FontFile2" => Object::reference(1),
        }),
    );
    objs.insert(
        ObjectId::new(5, 0),
        Object::from(dict! {
            "Type" => Object::name("FontDescriptor"),
            "FontFile" => Object::reference(2),
        }),
    );
    objs.insert(
        ObjectId::new(6, 0),
        Object::from(dict! {
            "Type" => Object::name("FontDescriptor"),
            "FontFile3" => Object::reference(3),
        }),
    );
    objs.insert(
        ObjectId::new(7, 0),
        Object::from(dict! {
            "Type" => Object::name("Font"),
            "Subtype" => Object::name("CIDFontType0"),
            "BaseFont" => Object::name("ShiftedCid"),
            "FontDescriptor" => Object::reference(6),
        }),
    );

    let streams = WithStreams::new(resolve_fn(|id| objs.get(&id)), |id, limit| {
        let Some(s) = objs.get(&id).and_then(Object::as_stream) else {
            return Err(Unreadable::new(id, Why::Damaged, "missing"));
        };
        let bytes = s.data.bytes().unwrap();
        if bytes.len() > limit {
            return Err(Unreadable::new(id, Why::Limit, "past limit"));
        }
        Ok(bytes.to_vec())
    });

    let mut budget = 16 << 20;

    // 1. TrueType (`glyf`) outline bounds == `char_box`, plus `/BaseEncoding /PDFDocEncoding`
    //    (`Gid::Unknown` in `char_box` still draws the outline!).
    let tt_dict = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("TrueType"),
        "BaseFont" => Object::name("FixTT"),
        "Encoding" => dict! { "BaseEncoding" => Object::name("PDFDocEncoding") },
        "FontDescriptor" => Object::reference(4),
        "FirstChar" => 32,
        "LastChar" => 122,
        "Widths" => vec![Object::from(600); 91],
    };
    let tt_font = Font::read(streams, &tt_dict, &Limits::DEFAULT, &mut budget, &mut ()).unwrap();
    // Under `PDFDocEncoding`, `char_box(b'A')` is `None` (`Gid::Unknown`), YET `outline` succeeds!
    assert_eq!(tt_font.char_box(u32::from(b'A')), None);
    let mut verbs = Vec::new();
    assert!(tt_font.outline(u32::from(b'A'), None, &mut verbs));
    assert_eq!(
        verb_control_bounds(&verbs),
        Some(GlyphBox::new(0.0, 0.0, 600.0, 750.0))
    );

    // Standard `WinAnsiEncoding` TrueType: `outline` control bounds equal `char_box`!
    let tt_win_dict = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("TrueType"),
        "BaseFont" => Object::name("FixTTWin"),
        "Encoding" => Object::name("WinAnsiEncoding"),
        "FontDescriptor" => Object::reference(4),
        "FirstChar" => 32,
        "LastChar" => 122,
        "Widths" => vec![Object::from(600); 91],
    };
    let tt_win = Font::read(
        streams,
        &tt_win_dict,
        &Limits::DEFAULT,
        &mut budget,
        &mut (),
    )
    .unwrap();
    for code in [
        u32::from(b'A'),
        u32::from(b'B'),
        u32::from(b'C'),
        u32::from(b'g'),
    ] {
        verbs.clear();
        assert!(tt_win.outline(code, None, &mut verbs));
        let got = verb_control_bounds(&verbs).unwrap();
        let exp = tt_win.char_box(code).unwrap();
        // `char_box` uses FreeType 26.6 fixed-point `FT_MulFix` scaling (`±1` unit vs exact `f32`).
        assert!(
            (got.x0 - exp.x0).abs() <= 1.0
                && (got.y0 - exp.y0).abs() <= 1.0
                && (got.x1 - exp.x1).abs() <= 1.0
                && (got.y1 - exp.y1).abs() <= 1.0,
            "code {code}: got {got:?}, exp {exp:?}"
        );
    }
    // Unmapped code (`> 255` on a simple font) leaves `verbs` unchanged!
    let sentinel = vec![OutlineVerb::MoveTo(7.0, 9.0)];
    verbs = sentinel.clone();
    assert!(!tt_win.outline(300, None, &mut verbs));
    assert_eq!(verbs, sentinel);

    // 2. Shifted Type 1 (`/FontMatrix [0.001 0 0 0.001 0.1 0.05]`): outline bounds == `char_box`!
    let t1_dict = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("Type1"),
        "BaseFont" => Object::name("FixT1"),
        "Encoding" => Object::name("WinAnsiEncoding"),
        "FontDescriptor" => Object::reference(5),
        "FirstChar" => 32,
        "LastChar" => 122,
        "Widths" => vec![Object::from(600); 91],
    };
    let t1_font = Font::read(streams, &t1_dict, &Limits::DEFAULT, &mut budget, &mut ()).unwrap();
    for code in [u32::from(b'A'), u32::from(b'g')] {
        verbs.clear();
        assert!(t1_font.outline(code, None, &mut verbs));
        let got = verb_control_bounds(&verbs).unwrap();
        let expected = t1_font.char_box(code).unwrap();
        assert!(
            (got.x0 - expected.x0).abs() <= 1.0
                && (got.y0 - expected.y0).abs() <= 1.0
                && (got.x1 - expected.x1).abs() <= 1.0
                && (got.y1 - expected.y1).abs() <= 1.0,
            "Type 1 shifted matrix mismatch for {code}: got {got:?}, expected {expected:?}"
        );
    }
    // Verify `A` was shifted to `100..700 x 50..800` (not `0..600 x 0..750`!).
    assert_eq!(
        t1_font.char_box(u32::from(b'A')),
        Some(GlyphBox::new(100.0, 50.0, 700.0, 800.0))
    );

    // 3. Shifted CID-CFF (`[0.001 0 0 0.001 0.12 -0.04]`): outline bounds match shifted glyph box
    //    (recall `cid_box` raises `y1` by `y1 / 64` for PDFium `GetCharBBox` parity, so `x0`, `y0`, `x1`
    //    match identically: `120..720`, `y0 = -40`, raw `y1 = 710`!).
    let cid_dict = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("Type0"),
        "BaseFont" => Object::name("ShiftedCid"),
        "Encoding" => Object::name("Identity-H"),
        "DescendantFonts" => vec![Object::reference(7)],
    };
    let cid_font = Font::read(streams, &cid_dict, &Limits::DEFAULT, &mut budget, &mut ()).unwrap();
    verbs.clear();
    assert!(cid_font.outline(7, None, &mut verbs));
    let cid_got = verb_control_bounds(&verbs).unwrap();
    let cid_box = cid_font.char_box(7).unwrap();
    assert!(
        (cid_got.x0 - cid_box.x0).abs() <= 1.0
            && (cid_got.y0 - cid_box.y0).abs() <= 1.0
            && (cid_got.x1 - cid_box.x1).abs() <= 1.0,
        "cid_got={cid_got:?}, cid_box={cid_box:?}"
    );
    assert_eq!(cid_got, GlyphBox::new(119.0, -40.0, 719.0, 710.0));
    // Out-of-fuel glyph leaves `verbs` unchanged!
    let mut zero_fuel_limits = Limits::DEFAULT;
    zero_fuel_limits.glyph_fuel = 2;
    zero_fuel_limits.font_fuel = 2;
    let heavy_id = ObjectId::new(30, 0);
    objs.insert(
        heavy_id,
        Object::from(Stream::new(
            dict! { "Subtype" => Object::name("Type1C") },
            fixture::heavy_cff_program(6),
        )),
    );
    objs.insert(
        ObjectId::new(31, 0),
        Object::from(dict! {
            "Type" => Object::name("FontDescriptor"),
            "FontFile3" => Object::from(heavy_id),
        }),
    );
    let streams2 = WithStreams::new(resolve_fn(|id| objs.get(&id)), |id, limit| {
        let Some(s) = objs.get(&id).and_then(Object::as_stream) else {
            return Err(Unreadable::new(id, Why::Damaged, "missing"));
        };
        let bytes = s.data.bytes().unwrap();
        if bytes.len() > limit {
            return Err(Unreadable::new(id, Why::Limit, "past limit"));
        }
        Ok(bytes.to_vec())
    });
    let heavy_dict = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("Type1"),
        "BaseFont" => Object::name("HeavyCFF"),
        "Encoding" => Object::name("WinAnsiEncoding"),
        "FontDescriptor" => Object::reference(31),
        "FirstChar" => 65,
        "LastChar" => 65,
        "Widths" => vec![Object::from(600)],
    };
    let heavy_font = Font::read(
        streams2,
        &heavy_dict,
        &zero_fuel_limits,
        &mut budget,
        &mut (),
    )
    .unwrap();
    verbs = sentinel.clone();
    assert!(!heavy_font.outline(u32::from(b'A'), None, &mut verbs));
    assert_eq!(verbs, sentinel);

    // 4. `FallbackProgram::outline_char` returns exact charstring/hmtx advances (not fixed 600.0),
    //    and returns `None` (leaving `out` untouched) when a bare CFF defines neither charstring
    //    width nor `defaultWidthX`!
    let fb_t1 = FallbackProgram::open(fixture::type1_program(), &Limits::DEFAULT).unwrap();
    verbs.clear();
    assert_eq!(fb_t1.outline_char('A', None, &mut verbs), Some(600.0));
    assert!(!verbs.is_empty());
    verbs.clear();
    assert_eq!(fb_t1.outline_char('g', None, &mut verbs), Some(500.0));
    verbs.clear();
    assert_eq!(fb_t1.outline_char(' ', None, &mut verbs), Some(250.0));
    assert!(verbs.is_empty());

    let fb_tt = FallbackProgram::open(fixture::truetype_program(), &Limits::DEFAULT).unwrap();
    verbs.clear();
    assert_eq!(fb_tt.outline_char('B', None, &mut verbs), Some(450.0));
    verbs.clear();
    assert_eq!(fb_tt.outline_char('C', None, &mut verbs), Some(700.0));

    let fb_cff_no_w = FallbackProgram::open(fixture::cff_program(), &Limits::DEFAULT).unwrap();
    verbs = sentinel.clone();
    assert_eq!(fb_cff_no_w.outline_char('A', None, &mut verbs), None);
    assert_eq!(verbs, sentinel);

    // 5. `Font::outline_status` distinguishes `Drawn`, `NoGlyph`, `NoProgram`, and `BudgetExceeded`!
    let unembedded_dict = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("Type1"),
        "BaseFont" => Object::name("Helvetica"),
    };
    let unembedded_font = Font::read(
        streams2,
        &unembedded_dict,
        &Limits::DEFAULT,
        &mut budget,
        &mut (),
    )
    .unwrap();
    verbs.clear();
    assert_eq!(
        unembedded_font.outline_status(u32::from(b'A'), None, &mut verbs),
        crate::OutlineOutcome::NoProgram
    );
    verbs.clear();
    assert_eq!(
        t1_font.outline_status(u32::from(b'A'), None, &mut verbs),
        crate::OutlineOutcome::Drawn
    );
    verbs.clear();
    assert_eq!(
        t1_font.outline_status(u32::from(b'Z'), None, &mut verbs),
        crate::OutlineOutcome::Drawn
    );
    assert!(
        verbs.is_empty(),
        "unmapped named code maps to empty .notdef"
    );
    verbs.clear();
    assert_eq!(
        t1_font.outline_status(300, None, &mut verbs),
        crate::OutlineOutcome::NoGlyph
    );
    verbs.clear();
    assert_eq!(
        cid_font.outline_status(99, None, &mut verbs),
        crate::OutlineOutcome::NoGlyph
    );
    verbs.clear();
    assert_eq!(
        heavy_font.outline_status(u32::from(b'A'), None, &mut verbs),
        crate::OutlineOutcome::BudgetExceeded
    );
}
