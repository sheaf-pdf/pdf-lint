//! TrueType programs opened for embedding, built by hand: the glyphs and codes text is
//! drawn in, widths and heights, what a licence allows, each refusal and what it stands
//! before, and the programs they're cut to, read again by skrifa.

use std::sync::{Arc, OnceLock};

use pdf_fixtures::font::{self, Glyph, TRUETYPE, TrueType};
use skrifa::MetadataProvider;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::types::{GlyphId, Tag};
use skrifa::raw::{FontRef, TableProvider};

use crate::embed::{Embedding, Error, Glyphs, Level, Outlines, Program, Unmapped};
use crate::{FontMetrics, Rect};

/// The sample program's glyphs: the character each draws, as `font::sample` lays them.
const SAMPLE: [(char, u16); 20] = [
    (' ', 1),
    ('A', 2),
    ('g', 3),
    ('H', 4),
    ('\u{C4}', 6),
    ('\u{1DE}', 8),
    ('\u{5E9}', 9),
    ('\u{5DC}', 10),
    ('\u{5D5}', 11),
    ('\u{5DD}', 12),
    ('\u{633}', 13),
    ('\u{644}', 14),
    ('\u{627}', 15),
    ('\u{645}', 16),
    ('\u{4E2D}', 300),
    ('\u{6587}', 301),
    ('\u{4E16}', 302),
    ('\u{754C}', 303),
    ('\u{1D11E}', 320),
    ('\u{A8}', 5),
];

/// The sample program at 1000 units an em, opened.
fn sample() -> Program {
    Program::open(font::sample(1000).build(), 0).unwrap()
}

/// What opening `program` refuses it for. Where it's opened, as with a check taken out,
/// it's cut to `drawn`, so that what the check stands before shows (a panic in the
/// subsetter, or a subset's size), and the test fails.
fn refused(program: Vec<u8>, drawn: impl IntoIterator<Item = u16>) -> Error {
    match Program::open(program, 0) {
        Err(error) => error,
        Ok(program) => {
            let mut glyphs = Glyphs::new();
            drawn.into_iter().for_each(|glyph| glyphs.insert(glyph, ""));
            let cut = program.subset(&glyphs).map(|subset| subset.program.len());
            panic!("opened, and cut to {cut:?} bytes");
        }
    }
}

/// `font`'s program with its table `tag` made `data`.
fn with(font: &TrueType, tag: &[u8; 4], data: Vec<u8>) -> Vec<u8> {
    let mut tables = font.tables();
    font::set(&mut tables, *tag, data);
    font::sfnt(TRUETYPE, &tables)
}

/// A program of 65,535 glyphs, each its own box, every glyph's advance in `hmtx`.
fn most_glyphs() -> &'static TrueType {
    static MOST: OnceLock<TrueType> = OnceLock::new();
    MOST.get_or_init(|| {
        let mut font = TrueType::new(1000);
        for i in 1..=65_534u16 {
            let x = (i % 500) as i16;
            font.glyph(Glyph::Box(x, 0, x + 100, 700), 600);
        }
        font
    })
}

/// Every character of the sample is drawn with the glyph `font::sample` gives it, from
/// format 12 where both subtables have it, and from format 4 where it's alone; one past
/// the BMP from format 12 alone.
#[test]
fn characters_are_their_glyphs() {
    let program = sample();
    for (c, glyph) in SAMPLE {
        assert_eq!(program.glyph(c), Some(glyph), "{c:?}");
    }
    let mut bmp = font::sample(1000);
    bmp.format_12 = false;
    let bmp = Program::open(bmp.build(), 0).unwrap();
    for (c, glyph) in SAMPLE {
        let expected = (u32::from(c) <= 0xFFFF).then_some(glyph);
        assert_eq!(bmp.glyph(c), expected, "{c:?}");
    }
    // None for a character it hasn't, a control character, and `.notdef`'s.
    for c in ['\n', 'Z', '\u{FE0F}', '\u{1F600}'] {
        assert_eq!(program.glyph(c), None, "{c:?}");
    }
    let mut zero = font::sample(1000);
    zero.map('Z', 0);
    // A glyph past the count is none too.
    zero.map('Y', 500);
    let zero = Program::open(zero.build(), 0).unwrap();
    assert_eq!((zero.glyph('Z'), zero.glyph('Y')), (None, None));
    assert_eq!(program.glyph_count(), 321);
}

/// Text's codes are two bytes a character, its glyph's number in the program high byte
/// first, a character past the BMP one glyph like any; the same each time, whatever was
/// drawn before.
#[test]
fn codes_are_the_programs_glyphs() {
    let program = sample();
    let text = "Ag 中文 שלום سلام 𝄞Ǟ";
    let codes = program.codes(text).unwrap();
    let expected: Vec<u8> = text
        .chars()
        .flat_map(|c| SAMPLE.iter().find(|s| s.0 == c).unwrap().1.to_be_bytes())
        .collect();
    assert_eq!(codes, expected);
    assert_eq!(codes.len(), 2 * text.chars().count());
    assert_eq!(program.codes("中𝄞").unwrap(), [0x01, 0x2C, 0x01, 0x40]);
    assert_eq!(program.codes("").unwrap(), b"");
    // Kept the same by a `Glyphs`, whatever it keeps already.
    let mut glyphs = Glyphs::new();
    glyphs.add(&program, "Hg").unwrap();
    assert_eq!(glyphs.add(&program, text).unwrap(), expected);
    assert_eq!(program.codes(text).unwrap(), expected);
}

/// A character the program has no glyph for is refused with where it is in bytes, and a
/// `Glyphs` keeps nothing of the text: a code point it hasn't, a variation selector it
/// doesn't map, a line feed.
#[test]
fn an_unmapped_character_is_refused() {
    let program = sample();
    let cases = [
        ("A中Z", 4, 'Z'),
        ("A\u{FE0F}", 1, '\u{FE0F}'),
        ("שלום\nA", 8, '\n'),
        ("𝄞\u{1F600}", 4, '\u{1F600}'),
    ];
    for (text, at, character) in cases {
        let refused = program.codes(text).unwrap_err();
        assert_eq!(refused, Unmapped { at, character }, "{text:?}");
        let mut glyphs = Glyphs::new();
        glyphs.add(&program, "g").unwrap();
        assert_eq!(glyphs.add(&program, text), Err(refused));
        assert_eq!(glyphs.len(), 1, "{text:?}");
        assert_eq!(program.subset(&glyphs).unwrap().glyphs, [0, 3]);
    }
    assert_eq!(
        program.codes("\u{1F600}").unwrap_err().to_string(),
        "U+1F600 at byte 0 has no glyph in the font program"
    );
}

/// A glyph's width is its advance a thousand times over the units per em, to the
/// nearest whole: at 1000 units and at 2048, half a thousandth up; a glyph past `hmtx`'s
/// advances takes the last's, and one past the count is 0.
#[test]
fn widths_are_thousandths_of_an_em() {
    let program = sample();
    let widths = [
        (0, 500.0),
        (1, 250.0),
        (2, 600.0),
        (9, 660.0),
        (300, 1000.0),
        (320, 580.0),
    ];
    for (glyph, width) in widths {
        assert_eq!(program.width(glyph), width, "{glyph}");
    }
    assert_eq!(program.width(321), 0.0);
    assert_eq!(program.width(u16::MAX), 0.0);

    let wide = font::sample(2048);
    let advances: Vec<u16> = wide.glyphs.iter().map(|g| g.1).collect();
    let wide = Program::open(wide.build(), 0).unwrap();
    // 600 at 2048 is 1229 units, and back 600.09765625, so 600.
    assert_eq!(advances[2], 1229);
    assert_eq!(wide.width(2), 600.0);
    for (glyph, &advance) in advances.iter().enumerate() {
        let width = (f64::from(advance) * 1000.0 / 2048.0).round() as f32;
        assert_eq!(wide.width(glyph as u16), width, "{glyph}");
    }
    // 1023 units is 499.51 thousandths, 1 is 0.49, 3 is 1.46 and 1025 is 500.49.
    let mut odd = TrueType::new(2048);
    for advance in [1023, 1, 3, 1025] {
        odd.glyph(Glyph::Empty, advance);
    }
    let odd = Program::open(odd.build(), 0).unwrap();
    assert_eq!([1, 2, 3, 4].map(|g| odd.width(g)), [500.0, 0.0, 1.0, 500.0]);

    let mut short = font::sample(1000);
    short.long_metrics = Some(3);
    let short = Program::open(short.build(), 0).unwrap();
    assert_eq!(
        [0, 1, 2, 3, 300].map(|g| short.width(g)),
        [500.0, 250.0, 600.0, 600.0, 600.0]
    );
}

/// Its heights are its `hhea`'s, or its `OS/2`'s typographic ones where it says to use
/// them, and its box its `head`'s, each in thousandths of an em.
#[test]
fn metrics_are_thousandths_of_an_em() {
    let program = sample();
    let bbox = Rect::new(10.0, -400.0, 960.0, 1100.0);
    assert_eq!(
        program.metrics(),
        FontMetrics::new(800.0, -200.0, Some(bbox))
    );

    let mut typo = font::sample(2048);
    let os2 = typo.os2.as_mut().unwrap();
    os2.fs_selection = 0x0080;
    os2.typo_ascender = 1700;
    os2.typo_descender = -512;
    let typo = Program::open(typo.build(), 0).unwrap();
    let metrics = typo.metrics();
    assert_eq!(
        (metrics.ascent, metrics.descent),
        (1700.0 * 1000.0 / 2048.0, -250.0)
    );
    let bbox = metrics.bbox.unwrap();
    // The sample's box at 2048 units, rounded to them: 20, -819, 1966 and 2253.
    assert_eq!(
        [bbox.x0, bbox.y0],
        [20.0 * 1000.0 / 2048.0, -819.0 * 1000.0 / 2048.0]
    );
    assert_eq!(
        [bbox.x1, bbox.y1],
        [1966.0 * 1000.0 / 2048.0, 2253.0 * 1000.0 / 2048.0]
    );
}

/// What a licence allows is its `OS/2`'s `fsType`: each usage bit its level, the least
/// restrictive where more are set, and the subsetting and bitmap bits apart; a program
/// without an `OS/2` says nothing against it.
#[test]
fn embedding_is_the_fs_type() {
    let cases = [
        (0x0000, Level::Installable, true, false),
        (0x0002, Level::Restricted, true, false),
        (0x0004, Level::Preview, true, false),
        (0x0008, Level::Editable, true, false),
        (0x0006, Level::Preview, true, false),
        (0x000A, Level::Editable, true, false),
        (0x000E, Level::Editable, true, false),
        // Bit 0 is reserved, and says nothing.
        (0x0001, Level::Installable, true, false),
        (0x0100, Level::Installable, false, false),
        (0x0200, Level::Installable, true, true),
        (0x0302, Level::Restricted, false, true),
    ];
    for (fs_type, level, subsetting, bitmaps_only) in cases {
        let mut font = font::sample(1000);
        font.os2.as_mut().unwrap().fs_type = fs_type;
        for version in [0, 1, 2, 5] {
            font.os2.as_mut().unwrap().version = version;
            let program = Program::open(font.build(), 0).unwrap();
            let embedding = program.embedding();
            assert_eq!(embedding.level, level, "{fs_type:#06x}");
            assert_eq!(embedding.subsetting, subsetting, "{fs_type:#06x}");
            assert_eq!(embedding.bitmaps_only, bitmaps_only, "{fs_type:#06x}");
        }
    }
    let mut none = font::sample(1000);
    none.os2 = None;
    let none = Program::open(none.build(), 0).unwrap();
    assert_eq!(none.embedding(), sample().embedding());
    assert_eq!(none.embedding().level, Level::Installable);
}

/// Bytes that aren't an SFNT, or a face of one with its table directory out of order,
/// are refused; so is a face other than one a program has.
#[test]
fn what_isnt_a_program_or_its_face_is_refused() {
    let program = font::sample(1000).build();
    for bytes in [
        &b""[..],
        b"%!PS-AdobeFont-1.0: Times",
        &program[..10],
        &[0; 64],
    ] {
        assert_eq!(
            Program::open(bytes.to_vec(), 0).unwrap_err(),
            Error::NotSfnt
        );
    }
    // Its tables in the order given: `cvt ` before `cmap`.
    let mut tables = font::sample(1000).tables();
    tables.swap(1, 2);
    let unsorted = font::sfnt(TRUETYPE, &tables);
    assert!(FontRef::new(&unsorted).unwrap().head().is_ok());
    assert_eq!(Program::open(unsorted, 0).unwrap_err(), Error::NotSfnt);
    // A directory counting more tables than it holds.
    let mut short = program.clone();
    short.truncate(12 + 16 * 3);
    assert_eq!(Program::open(short, 0).unwrap_err(), Error::NotSfnt);

    assert_eq!(
        Program::open(program.clone(), 1).unwrap_err(),
        Error::Index { index: 1, faces: 1 }
    );
    let other = font::sample(2048).build();
    let pair = font::collection(&[program, other]);
    // Told apart by their boxes' left: 10 thousandths at 1000 units, 20 units at 2048.
    let left = |program: Program| program.metrics().bbox.unwrap().x0;
    assert_eq!(left(Program::open(pair.clone(), 1).unwrap()), 9.765_625);
    assert_eq!(left(Program::open(pair.clone(), 0).unwrap()), 10.0);
    for index in [2, u32::MAX] {
        let refused = Program::open(pair.clone(), index).unwrap_err();
        assert_eq!(refused, Error::Index { index, faces: 2 });
    }
    assert_eq!(
        Error::Index { index: 2, faces: 2 }.to_string(),
        "face 2 asked of a font program of 2"
    );
}

/// A collection of version 1.1, whose second list of faces read-fonts reads and the
/// subsetter doesn't: where the two lists put the face apart, it's refused, as what's
/// checked wouldn't be what's cut.
#[test]
fn a_face_two_lists_put_apart_is_refused() {
    let first = font::sample(1000).build();
    let second = font::sample(2048).build();
    let header = 12 + 4 + 4 + 4;
    let mut file = b"ttcf".to_vec();
    file.extend(0x0001_0001u32.to_be_bytes());
    file.extend(1u32.to_be_bytes());
    let at_first = header;
    let at_second = (header + first.len()).next_multiple_of(4);
    file.extend((at_first as u32).to_be_bytes());
    file.extend(1u32.to_be_bytes());
    file.extend((at_second as u32).to_be_bytes());
    file.extend(font::moved(&first, at_first));
    file.resize(at_second, 0);
    file.extend(font::moved(&second, at_second));
    // read-fonts takes the second list's face, at 2048 units an em.
    let read = FontRef::from_index(&file, 0).unwrap();
    assert_eq!(read.head().unwrap().units_per_em(), 2048);
    assert_eq!(Program::open(file.clone(), 0).unwrap_err(), Error::NotSfnt);
    // Where both lists put it in one place, it's taken.
    file[12..16].copy_from_slice(&(at_second as u32).to_be_bytes());
    let bbox = Program::open(file, 0).unwrap().metrics().bbox.unwrap();
    assert_eq!(bbox.x0, 9.765_625);
}

/// A program whose outlines aren't TrueType's is refused, saying whose they are; one
/// with none at all lacks its `glyf`.
#[test]
fn outlines_other_than_truetypes_are_refused() {
    let font = font::sample(1000);
    let cases: [(&[[u8; 4]], Error); 6] = [
        (&[*b"CFF "], Error::Outlines(Outlines::Cff)),
        (&[*b"CFF2"], Error::Outlines(Outlines::Cff2)),
        (&[*b"CBDT", *b"CBLC"], Error::Outlines(Outlines::Bitmaps)),
        (&[*b"EBDT", *b"EBLC"], Error::Outlines(Outlines::Bitmaps)),
        (&[*b"sbix"], Error::Outlines(Outlines::Bitmaps)),
        (&[], Error::Malformed { table: "glyf" }),
    ];
    for (added, error) in cases {
        let mut tables = font.tables();
        font::remove(&mut tables, *b"glyf");
        font::remove(&mut tables, *b"loca");
        for tag in added {
            font::set(&mut tables, *tag, vec![0, 1, 0, 0, 0, 0, 0, 0]);
        }
        let tag = if added.first() == Some(b"CFF ") {
            *b"OTTO"
        } else {
            TRUETYPE
        };
        let program = font::sfnt(tag, &tables);
        assert_eq!(Program::open(program, 0).unwrap_err(), error, "{added:?}");
    }
    // A `CFF ` beside a `glyf`: TrueType's, as the subsetter takes it.
    let both = with(&font, b"CFF ", vec![1, 0, 4, 4]);
    assert!(Program::open(both, 0).is_ok());
    assert_eq!(
        Error::Outlines(Outlines::Cff).to_string(),
        "the font program's outlines are CFF, not TrueType's"
    );
}

/// A table a program needs that's missing or doesn't read is refused, naming it.
#[test]
fn a_malformed_table_is_refused() {
    let font = font::sample(1000);
    let tables = font.tables();
    let table = |tag: &[u8; 4]| tables.iter().find(|t| &t.0 == tag).unwrap().1.clone();
    let cut = |tag: &[u8; 4], len: usize| {
        let mut data = table(tag);
        data.truncate(len);
        with(&font, tag, data)
    };
    let set = |tag: &[u8; 4], at: usize, value: u16| {
        let mut data = table(tag);
        data[at..at + 2].copy_from_slice(&value.to_be_bytes());
        with(&font, tag, data)
    };
    let missing = |tag: &[u8; 4]| {
        let mut tables = font.tables();
        font::remove(&mut tables, *tag);
        font::sfnt(TRUETYPE, &tables)
    };
    let cases = [
        (cut(b"head", 20), "head"),
        (set(b"head", 18, 0), "head"),
        (missing(b"head"), "head"),
        (cut(b"maxp", 4), "maxp"),
        (set(b"maxp", 4, 0), "maxp"),
        (cut(b"hhea", 10), "hhea"),
        (set(b"hhea", 34, 0), "hhea"),
        (missing(b"hhea"), "hhea"),
        (cut(b"hmtx", 4 * 320 + 2), "hmtx"),
        (missing(b"hmtx"), "hmtx"),
        (missing(b"loca"), "loca"),
    ];
    for (program, table) in cases {
        assert_eq!(
            Program::open(program, 0).unwrap_err(),
            Error::Malformed { table },
            "{table}"
        );
    }
    assert_eq!(
        Error::Malformed { table: "loca" }.to_string(),
        "the font program's `loca` table is missing or malformed"
    );
}

/// Units per em outside the 16 to 16,384 OpenType allows, of which FreeType, and so
/// PDFium, loads no program, are refused as it's opened, so that no font is written
/// whose glyphs a viewer draws in another; each end of the range is taken, its widths
/// a thousand times its advances over it.
#[test]
fn units_per_em_outside_opentypes_range_are_refused() {
    let font = font::sample(1000);
    let head = font
        .tables()
        .into_iter()
        .find(|t| &t.0 == b"head")
        .unwrap()
        .1;
    let at = |units: u16| {
        let mut head = head.clone();
        head[18..20].copy_from_slice(&units.to_be_bytes());
        with(&font, b"head", head)
    };
    for units in [0, 1, 15, 16_385, 65_136, u16::MAX] {
        let refused = Program::open(at(units), 0).unwrap_err();
        assert_eq!(refused, Error::Malformed { table: "head" }, "{units}");
    }
    for (units, width) in [(16, 37_500.0), (16_384, 37.0)] {
        let program = Program::open(at(units), 0).unwrap();
        assert_eq!(program.width(2), width, "{units}");
    }
}

/// A composite naming every glyph number to 65,535, in a program of 65,535 glyphs, is
/// refused as it's opened: cut, the subsetter would number past 65,535 glyphs.
#[test]
fn a_composite_past_the_glyphs_is_refused() {
    let mut font = TrueType::new(1000);
    let mut composite = vec![0xFF, 0xFF, 0, 0, 0, 0, 0, 100, 0, 100];
    for glyph in 0..=u16::MAX {
        // Its offsets bytes, x and y values; more after it but the last.
        let flags: u16 = if glyph < u16::MAX { 0x0022 } else { 0x0002 };
        composite.extend(flags.to_be_bytes());
        composite.extend(glyph.to_be_bytes());
        composite.extend([0, 0]);
    }
    font.glyph(Glyph::Raw(composite), 500);
    while font.glyphs.len() < 65_535 {
        font.glyph(Glyph::Empty, 500);
    }
    let program = font.build();
    let error = refused(program, 0..u16::MAX);
    assert_eq!(error, Error::Malformed { table: "glyf" });

    // One component past the count of a small program is refused alike; one at the
    // count's last glyph isn't.
    for (component, opens) in [(323, false), (322, true)] {
        let mut small = font::sample(1000);
        small.glyph(Glyph::Composite(vec![(2, 0, 0), (component, 0, 0)]), 500);
        small.glyph(Glyph::Box(0, 0, 100, 100), 500);
        let program = small.build();
        match opens {
            true => {
                let program = Program::open(program, 0).unwrap();
                let mut glyphs = Glyphs::new();
                glyphs.insert(321, "x");
                assert_eq!(program.subset(&glyphs).unwrap().glyphs, [0, 2, 321, 322]);
            }
            false => assert_eq!(refused(program, 0..323), Error::Malformed { table: "glyf" }),
        }
    }
}

/// A `loca` placing every even glyph on the same 3 KiB, every odd one backwards, is
/// refused as it's opened: cut to the even glyphs, it would write some 100 MB from a
/// program of 3 KiB's glyphs.
#[test]
fn a_loca_whose_glyphs_overlap_is_refused() {
    let mut font = TrueType::new(1000);
    while font.glyphs.len() < 65_535 {
        font.glyph(Glyph::Empty, 500);
    }
    font.long_loca = true;
    let points: Vec<(i16, i16, bool)> = (0..611).map(|i| (i, i % 7, true)).collect();
    let mut glyph = font::simple(&points);
    glyph.resize(3072, 0);
    let loca: Vec<u8> = (0..=65_535u32)
        .flat_map(|i| (if i % 2 == 0 { 0u32 } else { 3072 }).to_be_bytes())
        .collect();
    let mut tables = font.tables();
    font::set(&mut tables, *b"glyf", glyph);
    font::set(&mut tables, *b"loca", loca);
    let program = font::sfnt(TRUETYPE, &tables);
    assert!(program.len() < 600 << 10);
    let even = (0..u16::MAX).step_by(2);
    assert_eq!(refused(program, even), Error::Malformed { table: "loca" });

    // Glyphs one after another, `loca`'s last entry at `glyf`'s end, aren't.
    let mut exact = font::sample(1000);
    exact.long_loca = true;
    assert!(Program::open(exact.build(), 0).is_ok());
}

/// The sample program with a `name` table of `records` over `storage`.
fn named(records: &[[u16; 6]], storage: &[u8]) -> Vec<u8> {
    with(
        &font::sample(1000),
        b"name",
        font::name_table(records, storage),
    )
}

/// A record the subsetter keeps: Windows' Unicode, US English, name `id`.
fn kept(id: u16, length: u16, offset: u16) -> [u16; 6] {
    [3, 1, 0x409, id, length, offset]
}

/// 65,536 bytes of storage, no two runs of them alike.
fn storage() -> Vec<u8> {
    (0..=0xFFFFu32).map(|i| (i % 251) as u8).collect()
}

/// Each program refused as it's opened for its `name`, as cutting it would overflow;
/// each edge opened and cut, its `name` read again.
fn names_refused(overflows: &[Vec<u8>], edges: &[Vec<u8>]) {
    for program in overflows {
        let error = refused(program.clone(), 0..321);
        assert_eq!(error, Error::Malformed { table: "name" });
    }
    for program in edges {
        let program = Program::open(program.clone(), 0).unwrap();
        let cut = program.whole().unwrap();
        assert!(FontRef::new(&cut.program).unwrap().name().is_ok());
    }
}

/// A record the subsetter keeps reaching past 65,535 bytes, which it adds in 16 bits, is
/// refused, of each Unicode platform it keeps; one reaching 65,535 is taken.
#[test]
fn a_name_record_past_65535_bytes_is_refused() {
    let storage = storage();
    let overflows = [
        named(&[kept(1, 1, 0xFFFF)], &storage),
        named(&[[0, 3, 0, 6, 1, 0xFFFF]], &storage),
        named(&[[3, 10, 0x409, 14, 1, 0xFFFF]], &storage),
    ];
    names_refused(
        &overflows,
        &[named(&[kept(1, 1, 0xFFFE)], &storage[..0xFFFF])],
    );
}

/// More records the subsetter keeps than 65,535 bytes holds, with its header, which it
/// counts in 16 bits, are refused: 5,461; 5,460 are taken, and so are 5,461 it doesn't
/// keep.
#[test]
fn more_name_records_than_a_table_holds_are_refused() {
    let overflows = [named(&[kept(1, 2, 0)].repeat(5461), &[0, 1])];
    let edges = [
        named(&[kept(1, 2, 0)].repeat(5460), &[0, 1]),
        named(&[[1, 0, 0, 1, 2, 0]].repeat(5461), &[0, 1]),
    ];
    names_refused(&overflows, &edges);
}

/// Distinct strings the subsetter keeps past 65,535 bytes together, which it adds in 16
/// bits, are refused; 65,535 are taken, and a string kept twice is counted once.
#[test]
fn name_strings_past_65535_bytes_are_refused() {
    let storage = storage();
    let overflows = [named(
        &[kept(1, 40_000, 0), kept(4, 30_000, 35_535)],
        &storage,
    )];
    let edges = [
        named(
            &[kept(1, 40_000, 0), kept(4, 25_535, 40_000)],
            &storage[..0xFFFF],
        ),
        named(
            &[kept(1, 40_000, 0), kept(4, 40_000, 0)],
            &storage[..40_000],
        ),
    ];
    names_refused(&overflows, &edges);
}

/// A `name` table too short for its header or its records, or with a kept string past
/// it, is refused; records the subsetter doesn't keep aren't read, and a table of a
/// version but 0, which it keeps whole, isn't either.
#[test]
fn a_name_table_cut_short_is_refused() {
    let font = font::sample(1000);
    let short = [
        named(&[kept(1, 2, 0)], &[]),
        with(&font, b"name", vec![0, 0, 0, 3, 0, 42, 0, 3]),
        with(&font, b"name", vec![0]),
        with(&font, b"name", vec![0, 0, 0, 0, 0, 99]),
    ];
    for program in short {
        let error = Program::open(program, 0).unwrap_err();
        assert_eq!(error, Error::Malformed { table: "name" });
    }
    // Macintosh's record, and a name past 14, each past the table.
    let unkept = named(&[[1, 0, 0, 1, 1, 0xFFFF], kept(15, 1, 0xFFFF)], &[]);
    assert!(Program::open(unkept, 0).is_ok());
    assert!(Program::open(with(&font, b"name", vec![0, 1, 0xFF, 0xFF]), 0).is_ok());
}

/// A `Glyphs` holding a glyph past the program's count, as one of another program
/// would, is refused as the program is cut, also beside the 65,535 a program can have.
#[test]
fn glyphs_past_the_count_are_refused() {
    let program = sample();
    let mut glyphs = Glyphs::new();
    glyphs.add(&program, "Ag").unwrap();
    glyphs.insert(321, "x");
    let error = program.subset(&glyphs).unwrap_err();
    assert_eq!(
        error,
        Error::Glyph {
            glyph: 321,
            count: 321
        }
    );
    assert_eq!(
        error.to_string(),
        "glyph 321 is past the font program's 321 glyphs"
    );

    let most = Program::open(most_glyphs().build(), 0).unwrap();
    let mut every = Glyphs::new();
    (0..=u16::MAX).for_each(|glyph| every.insert(glyph, "x"));
    let error = most.subset(&every).unwrap_err();
    assert_eq!(
        error,
        Error::Glyph {
            glyph: 65_535,
            count: 65_535
        }
    );
}

/// A drawn glyph whose `loca` entries don't place it is refused as it's cut; a
/// composite cut short after its component's number the subsetter refuses, said in
/// this crate's words.
#[test]
fn a_glyph_that_cant_be_cut_is_refused() {
    let mut font = font::sample(1000);
    let short = font.glyph(
        Glyph::Raw(vec![0xFF, 0xFF, 0, 0, 0, 0, 0, 9, 0, 9, 0, 0x21, 0, 2]),
        500,
    );
    let program = Program::open(font.build(), 0).unwrap();
    let mut glyphs = Glyphs::new();
    glyphs.insert(short, "x");
    let error = program.subset(&glyphs).unwrap_err();
    assert_eq!(error, Error::Subset("a table it rewrites doesn't read"));
    assert_eq!(
        error.to_string(),
        "the font program couldn't be cut to the glyphs drawn: a table it rewrites doesn't read"
    );

    // A `loca` shorter than the glyphs: the last glyph has no end.
    let font = font::sample(1000);
    let tables = font.tables();
    let mut loca = tables.iter().find(|t| &t.0 == b"loca").unwrap().1.clone();
    loca.truncate(loca.len() - 2);
    let program = Program::open(with(&font, b"loca", loca), 0).unwrap();
    let mut clef = Glyphs::new();
    clef.add(&program, "A𝄞").unwrap();
    assert_eq!(
        program.subset(&clef).unwrap_err(),
        Error::Malformed { table: "loca" }
    );
    let mut a = Glyphs::new();
    a.add(&program, "A").unwrap();
    assert!(program.subset(&a).is_ok());
}

/// A pen that keeps each command an outline draws.
#[derive(Default)]
struct Commands(Vec<String>);

impl OutlinePen for Commands {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push(format!("M {x} {y}"));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push(format!("L {x} {y}"));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.push(format!("Q {cx0} {cy0} {x} {y}"));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.push(format!("C {cx0} {cy0} {cx1} {cy1} {x} {y}"));
    }
    fn close(&mut self) {
        self.0.push("Z".into());
    }
}

/// Glyph `glyph`'s outline in `font`, unscaled, as skrifa draws it.
fn outline(font: &FontRef<'_>, glyph: u16) -> Vec<String> {
    let mut pen = Commands::default();
    let settings = DrawSettings::unhinted(Size::unscaled(), LocationRef::default());
    let glyph = font
        .outline_glyphs()
        .get(GlyphId::new(u32::from(glyph)))
        .unwrap();
    glyph.draw(settings, &mut pen).unwrap();
    pen.0
}

/// A program cut to what's drawn holds exactly `.notdef`, the glyphs drawn and their
/// composites' components, two deep, in order of their numbers in the program; each
/// draws as it did, its advance as it was; it has no `cmap`, and the tables a CIDFont's
/// program needs, its hinting tables where the program has them.
#[test]
fn the_subset_is_the_glyphs_drawn() {
    for hinting in [true, false] {
        let mut font = font::sample(1000);
        font.hinting = hinting;
        let program = Program::open(font.build(), 0).unwrap();
        let mut glyphs = Glyphs::new();
        glyphs.add(&program, "\u{1DE}H中").unwrap();
        let subset = program.subset(&glyphs).unwrap();
        assert_eq!(subset.glyphs, [0, 2, 4, 5, 6, 7, 8, 300]);

        let original = FontRef::new(&program.data).unwrap();
        let cut = FontRef::new(&subset.program).unwrap();
        assert_eq!(cut.maxp().unwrap().num_glyphs(), 8);
        let advances = cut.glyph_metrics(Size::unscaled(), LocationRef::default());
        for (new, &old) in subset.glyphs.iter().enumerate() {
            assert_eq!(outline(&cut, new as u16), outline(&original, old), "{old}");
            let advance = advances.advance_width(GlyphId::new(new as u32));
            assert_eq!(advance, Some(program.width(old)), "{old}");
        }
        // Ǟ, now glyph 6, is A's box and two more, each drawn alike.
        assert_eq!(outline(&cut, 6).len(), 3 * outline(&cut, 1).len());
        let has = |tag: &[u8; 4]| cut.table_data(Tag::new(tag)).is_some();
        assert!(!has(b"cmap") && !has(b"OS/2"));
        for tag in [
            b"glyf", b"head", b"hhea", b"hmtx", b"loca", b"maxp", b"name", b"post",
        ] {
            assert!(has(tag), "{tag:?}");
        }
        for tag in [b"cvt ", b"fpgm", b"prep"] {
            assert_eq!(has(tag), hinting, "{tag:?}");
        }
    }
}

/// The same glyphs cut to the same bytes whatever order they were drawn in, and
/// however often; `.notdef` alone where nothing is.
#[test]
fn the_subset_is_the_same_in_any_order() {
    let program = sample();
    let texts = ["Ag中𝄞שלום", "𝄞中gA", "םולש", "AAAA𝄞gggg中ש", "לוםש"];
    let mut cut = Vec::new();
    for order in [[0, 1, 2], [2, 1, 0], [1, 2, 0]] {
        let mut glyphs = Glyphs::new();
        for i in order {
            glyphs.add(&program, texts[i]).unwrap();
        }
        cut.push(program.subset(&glyphs).unwrap().program);
    }
    let mut glyphs = Glyphs::new();
    glyphs.add(&program, texts[3]).unwrap();
    glyphs.add(&program, texts[4]).unwrap();
    cut.push(program.subset(&glyphs).unwrap().program);
    assert!(cut.iter().all(|c| *c == cut[0]));

    let none = program.subset(&Glyphs::new()).unwrap();
    assert_eq!(none.glyphs, [0]);
    assert_eq!(
        FontRef::new(&none.program)
            .unwrap()
            .maxp()
            .unwrap()
            .num_glyphs(),
        1
    );
}

/// The whole program is every glyph in its own order, each drawn as it was; drawing
/// every glyph cuts the same.
#[test]
fn the_whole_program_is_every_glyph() {
    let program = sample();
    let whole = program.whole().unwrap();
    assert_eq!(whole.glyphs, (0..321).collect::<Vec<_>>());
    let original = FontRef::new(&program.data).unwrap();
    let cut = FontRef::new(&whole.program).unwrap();
    assert_eq!(cut.maxp().unwrap().num_glyphs(), 321);
    for glyph in [0, 2, 6, 8, 300, 320] {
        assert_eq!(outline(&cut, glyph), outline(&original, glyph));
    }
    let mut every = Glyphs::new();
    (0..321).for_each(|glyph| every.insert(glyph, "x"));
    assert_eq!(program.subset(&every).unwrap().program, whole.program);
}

/// A program of 65,535 glyphs, every one drawn, is cut to all of them, and what's
/// written is the program's size and a constant at most: its `hmtx` each glyph's
/// advance and its `loca` long, at most six bytes a glyph more than it had.
#[test]
fn every_glyph_of_the_most_is_cut_within_the_programs_size() {
    let bytes = most_glyphs().build();
    let program = Program::open(bytes.clone(), 0).unwrap();
    assert_eq!(program.glyph_count(), 65_535);
    let mut every = Glyphs::new();
    (0..u16::MAX).for_each(|glyph| every.insert(glyph, "x"));
    let subset = program.subset(&every).unwrap();
    assert_eq!(subset.glyphs.len(), 65_535);
    assert!(subset.program.len() <= bytes.len() + 6 * 65_536);
    let cut = FontRef::new(&subset.program).unwrap();
    assert_eq!(cut.maxp().unwrap().num_glyphs(), 65_535);
    let original = FontRef::new(&bytes).unwrap();
    for glyph in [1, 499, 500, 65_534] {
        assert_eq!(outline(&cut, glyph), outline(&original, glyph));
    }
}

/// A `Glyphs` keeps a glyph's smallest text, as `str` orders them, whatever order the
/// texts came in; text of any length by its number.
#[test]
fn glyphs_keep_the_smallest_text() {
    let program = sample();
    let mut glyphs = Glyphs::new();
    assert!(glyphs.is_empty());
    glyphs.insert(2, "B");
    glyphs.add(&program, "A").unwrap();
    glyphs.insert(2, "C");
    glyphs.insert(3, "fi");
    glyphs.insert(3, "f");
    assert_eq!(glyphs.len(), 2);
    assert_eq!(glyphs.texts.get(&2).map(String::as_str), Some("A"));
    assert_eq!(glyphs.texts.get(&3).map(String::as_str), Some("f"));
}

/// `Debug` says how much, never a name of the program, a byte of it or a text drawn.
#[test]
fn debug_holds_no_name_or_text() {
    let mut font = font::sample(1000);
    font.names = vec![(4, "MARKER Full".into()), (6, "MARKER-PostScript".into())];
    let program = Program::open(font.build(), 0).unwrap();
    let mut glyphs = Glyphs::new();
    glyphs.add(&program, "AgH").unwrap();
    glyphs.insert(7, "MARKER");
    let printed = format!("{program:?} {glyphs:?} {:?}", program.clone());
    assert!(!printed.contains("MARKER"), "{printed}");
    assert!(
        printed.contains("glyphs: 321") && printed.contains("len: 4"),
        "{printed}"
    );
    let embedding = Embedding::of(0);
    assert!(format!("{embedding:?}").contains("Installable"));
}

/// A program and the glyphs drawn in it may be shared by threads drawing pages.
#[test]
fn programs_and_glyphs_are_send_and_sync() {
    fn shared<T: Send + Sync>() {}
    shared::<Program>();
    shared::<Glyphs>();
    shared::<Error>();
    shared::<Arc<Program>>();
}
