//! Files' fonts as read, built in code: each kind's codes, widths and heights,
//! what's charged, what fails and what's told; and over a reading of a file, every
//! way a stream's data may come to a font.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

use pdf_objects::fault::{Place, Sink};
use pdf_objects::object::Stream;
use pdf_objects::{Dict, Object, ObjectId, Resolve, Streams, Unreadable, Why, WithStreams, dict};

use super::afm;
use crate::fuzz::digest;
use crate::*;

/// A page's budget, as the tests read within it.
const BUDGET: usize = 256 << 20;

type Dictionary = Dict;

/// A map of objects: a test's file, each stream's data as it is.
#[derive(Default)]
struct Pdf {
    objects: HashMap<ObjectId, Object>,
    last: u32,
}

impl Pdf {
    fn new_object_id(&mut self) -> ObjectId {
        self.last += 1;
        ObjectId::new(self.last, 0)
    }

    fn add_object(&mut self, object: impl Into<Object>) -> ObjectId {
        let id = self.new_object_id();
        self.objects.insert(id, object.into());
        id
    }
}

impl<'o> Resolve<'o> for &'o Pdf {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        let pdf: &'o Pdf = self;
        pdf.objects.get(&id)
    }
}

impl Streams for &Pdf {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        self.decoded_by(id, limit)
    }
}

/// `font` read over `pdf`'s objects, within a page's budget.
fn font_of(pdf: &Pdf, font: Dictionary) -> Font {
    let mut budget = BUDGET;
    Font::read(pdf, &font, &Limits::DEFAULT, &mut budget, &mut ()).expect("within the budget")
}

/// The codes `s` is in `font`, each with where it is.
fn codes(font: &Font, s: &[u8]) -> Vec<(std::ops::Range<usize>, u32)> {
    font.codes(s).collect()
}

/// What's guessed of a font that embeds no program, and is no Type 3
/// font, but whose metrics are all given: its glyphs' outlines.
const OUTLINES: FontApprox = FontApprox {
    codes: false,
    widths: false,
    heights: false,
    boxes: true,
};

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect { x0, y0, x1, y1 }
}

/// A simple font's widths: `/Widths` from `/FirstChar` to `/LastChar`,
/// whole, and `MissingWidth` for every other code; a code a byte.
#[test]
fn simple_widths_from_first_char_and_missing_width() {
    let mut pdf = Pdf::default();
    let descriptor = pdf.add_object(dict! {
        "Type" => "FontDescriptor", "MissingWidth" => 500, "Ascent" => 800, "Descent" => -200,
        "FontBBox" => vec![0.into(), (-200).into(), 1000.into(), 800.into()],
    });
    let truetype = |first: i64, last: i64| {
        dict! {
            "Type" => "Font", "Subtype" => "TrueType", "BaseFont" => "Foo",
            "FirstChar" => first, "LastChar" => last,
            "Widths" => vec![600.into(), Object::from(700.9)],
            "FontDescriptor" => descriptor,
        }
    };
    let font = font_of(&pdf, truetype(65, 66));
    assert_eq!(font.kind(), FontKind::TrueType);
    let widths = [64, 65, 66, 67, 200].map(|c| font.width(c));
    assert_eq!(widths, [500.0, 600.0, 700.0, 500.0, 500.0]);
    assert_eq!(codes(&font, b"AB"), [(0..1, 65), (1..2, 66)]);
    assert_eq!(font.cid(65), None);
    // Its glyphs' outlines only a program it doesn't embed would say.
    assert_eq!(font.approx(), OUTLINES);
    // `/LastChar` before `/Widths` ends; 0, or past it, reads to its end.
    let short = font_of(&pdf, truetype(65, 65));
    assert_eq!([65, 66].map(|c| short.width(c)), [600.0, 500.0]);
    let open = font_of(&pdf, truetype(65, 0));
    assert_eq!(open.width(66), 700.0);
    // A `/FirstChar` past a byte's codes gives no widths.
    let past = font_of(&pdf, truetype(300, 301));
    assert_eq!(past.width(44), 500.0);
    // Without `MissingWidth`, 0; and a font with no widths known is a
    // guess.
    let mut bare = truetype(65, 66);
    bare.remove(b"FontDescriptor");
    let bare = font_of(&pdf, bare);
    assert_eq!(bare.width(67), 0.0);
    assert!(!bare.approx().widths);
    let mut unknown = truetype(65, 66);
    unknown.remove(b"Widths");
    assert!(font_of(&pdf, unknown).approx().widths);
}

/// A standard 14 font without `/Widths`: each code's glyph by its
/// encoding, at Adobe's width, with Adobe's box; a code the font has no
/// glyph for as the space, and the Euro, which Adobe gives no box, each
/// guessed, with none; Symbol in its own encoding whatever it names; one
/// with its program embedded, a guess; one with `/Widths`, its own.
#[test]
fn standard_14_without_widths() {
    let mut pdf = Pdf::default();
    let type1 = |base: &str| {
        dict! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => base.to_string() }
    };
    let helvetica = font_of(&pdf, type1("Helvetica"));
    assert_eq!(
        [b'A', b' ', 0x01].map(|c| helvetica.width(c.into())),
        [667.0, 278.0, 278.0]
    );
    assert_eq!(
        helvetica.glyph_box(b'A'.into()),
        Some(rect(14.0, 0.0, 654.0, 718.0))
    );
    assert_eq!(helvetica.glyph_box(b' '.into()), Some(Rect::default()));
    assert_eq!(helvetica.glyph_box(0x01), None);
    // Those StandardEncoding names no glyph for, or one PDFium's face is
    // another width of, and only those.
    let guessed = |c: u32| match Encoding::Standard.glyph(c as u8) {
        Some(name) => Std14::Helvetica.drawn_otherwise(name),
        None => true,
    };
    assert!((0..256).all(|c| helvetica.width_guessed(c) == guessed(c)));
    assert!(guessed(0x01) && guessed(0xAB) && !guessed(b'A'.into()));
    // A string has a code whose width is guessed where a byte of it is.
    for s in [
        &b""[..],
        b"A",
        b"\x01",
        b"\x01AB",
        b"AB\x01",
        b"A\xABB",
        b"ABC",
    ] {
        let any = s.iter().any(|&b| guessed(b.into()));
        assert_eq!(helvetica.width_guessed_in(s), any, "{s:?}");
    }
    let metrics = helvetica.metrics();
    assert_eq!((metrics.ascent, metrics.descent), (718.0, -207.0));
    assert_eq!(metrics.bbox, Some(rect(-166.0, -225.0, 1000.0, 931.0)));
    // PDFium takes its heights, and its glyphs' outlines, from the face it
    // substitutes.
    let heights = FontApprox {
        heights: true,
        ..OUTLINES
    };
    assert_eq!(helvetica.approx(), heights);

    let mut encoded = type1("Helvetica");
    encoded.set(
        "Encoding",
        dict! {
            "BaseEncoding" => "WinAnsiEncoding",
            "Differences" => vec![1.into(), "A".into()],
        },
    );
    let encoded = font_of(&pdf, encoded);
    assert_eq!([1, 0x80].map(|c| encoded.width(c)), [667.0, 556.0]);
    // 0x81, which WinAnsiEncoding leaves empty, draws the bullet.
    assert_eq!(encoded.width(0x81), 350.0);
    assert_eq!(
        [1, 0x80, 0x81, b'A'.into()].map(|c| encoded.width_guessed(c)),
        [false, true, false, false]
    );
    assert_eq!(encoded.glyph_box(0x80), None);
    let mut given = type1("Helvetica");
    given.set("Encoding", "WinAnsiEncoding");
    given.set("FirstChar", 0x80);
    given.set("Widths", vec![600.into()]);
    let given = font_of(&pdf, given);
    assert_eq!(given.width(0x80), 600.0);
    assert!((0..256).all(|c| !given.width_guessed(c)));
    assert!(!given.width_guessed_in(b"\x80\x01"));
    assert!(Std14::Courier.glyph_width("A") == Some(600.0));
    let courier = font_of(&pdf, type1("Courier"));
    assert!((0..=255).all(|c| courier.width(c) == 600.0));
    let mut symbol = type1("Symbol");
    symbol.set("Encoding", "WinAnsiEncoding");
    assert_eq!(font_of(&pdf, symbol).width(b'a'.into()), 631.0);

    let program = pdf.add_object(Stream::new(dict! {}, crate::fixture::type1_program()));
    let descriptor = pdf.add_object(dict! { "FontFile" => program });
    let mut embedded = type1("ABCDEF+Helvetica");
    embedded.set("FontDescriptor", descriptor);
    let embedded = font_of(&pdf, embedded);
    assert_eq!(embedded.width(b'A'.into()), 667.0);
    assert!(embedded.approx().widths);
    assert!((0..256).all(|c| !embedded.width_guessed(c)));
    assert_eq!(embedded.glyph_box(b'A'.into()), None);
}

/// Each standard 14 font's heights and glyphs' boxes are its AFM's, every
/// glyph's within the font's box; Symbol and ZapfDingbats, which give no
/// `Ascender` or `Descender`, reach their box's top and bottom.
#[test]
fn standard_14_heights_and_boxes_are_the_afm_s() {
    for font in Std14::ALL {
        let metrics = font.metrics();
        let bbox = metrics.bbox.unwrap();
        assert!(metrics.descent < 0.0 && 0.0 < metrics.ascent, "{font:?}");
        assert!(
            bbox.y0 <= metrics.descent && metrics.ascent <= bbox.y1,
            "{font:?}"
        );
        for (name, width) in afm(font.afm_text()).0 {
            assert_eq!(font.glyph_width(name), Some(width), "{font:?} {name}");
            let b = font.glyph_box(name).unwrap();
            let within = bbox.x0 <= b.x0 && b.x1 <= bbox.x1 && bbox.y0 <= b.y0 && b.y1 <= bbox.y1;
            assert!(within, "{font:?} {name} {b:?}");
        }
    }
    let helvetica = Std14::Helvetica.metrics();
    assert_eq!((helvetica.ascent, helvetica.descent), (718.0, -207.0));
    let dingbats = Std14::ZapfDingbats.metrics();
    assert_eq!((dingbats.ascent, dingbats.descent), (820.0, -143.0));
    let a = Std14::TimesBold.glyph_box("A");
    assert_eq!(a, Some(rect(9.0, 0.0, 689.0, 690.0)));
    assert_eq!(Std14::Helvetica.glyph_box("nonesuch"), None);
}

/// A Type 0 font over `/Encoding`, with a CIDFont of `descendant`'s keys.
fn type0(pdf: &mut Pdf, encoding: Object, descendant: Dictionary) -> Dictionary {
    let cid_font = pdf.add_object(descendant);
    dict! {
        "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "Foo",
        "Encoding" => encoding, "DescendantFonts" => vec![cid_font.into()],
    }
}

/// Identity-H: two bytes a code, each its CID, widths from `/W` in both
/// its forms, the first range given for a CID winning, and `/DW` for the
/// rest.
#[test]
fn identity_h_widths_from_w_and_dw() {
    let mut pdf = Pdf::default();
    let descriptor = pdf.add_object(dict! {
        "FontBBox" => vec![0.into(), (-200).into(), 1000.into(), 900.into()],
        "Ascent" => 900, "Descent" => -200,
    });
    let w: Vec<Object> = vec![
        1.into(),
        vec![500.into(), 600.into()].into(),
        10.into(),
        20.into(),
        700.into(),
        15.into(),
        vec![900.into()].into(),
    ];
    let font = type0(
        &mut pdf,
        "Identity-H".into(),
        dict! {
            "Subtype" => "CIDFontType2", "W" => w, "DW" => 900, "FontDescriptor" => descriptor,
        },
    );
    let font = font_of(&pdf, font);
    assert_eq!(font.kind(), FontKind::Type0(Descendant::CidFontType2));
    let widths = [1, 2, 3, 10, 15, 20, 21].map(|c| font.width(c));
    assert_eq!(widths, [500.0, 600.0, 900.0, 700.0, 700.0, 700.0, 900.0]);
    // A code cut off at the end has a 0 for its second byte, as PDFium
    // reads it.
    let split = codes(&font, &[0, 1, 0, 2, 0, 10, 7]);
    assert_eq!(split, [(0..2, 1), (2..4, 2), (4..6, 10), (6..7, 0x700)]);
    assert_eq!(font.cid(0x1234), Some(0x1234));
    assert_eq!(font.vertical(1), None);
    let metrics = font.metrics();
    assert_eq!((metrics.ascent, metrics.descent), (900.0, -200.0));
    assert_eq!(font.approx(), OUTLINES);
    // No `/DW`: 1000.
    let bare = type0(
        &mut pdf,
        "Identity-H".into(),
        dict! { "Subtype" => "CIDFontType0" },
    );
    let bare = font_of(&pdf, bare);
    assert_eq!(bare.kind(), FontKind::Type0(Descendant::CidFontType0));
    assert_eq!(bare.width(5), 1000.0);
    // A predefined CMap the build hasn't splits by the `/ToUnicode` codespace, its
    // CIDs unknown: KSCpc-EUC-V, which Adobe's hasn't.
    let predefined = type0(&mut pdf, "KSCpc-EUC-V".into(), dict! { "DW" => 800 });
    let predefined = font_of(&pdf, predefined);
    assert!(predefined.approx().codes);
    assert_eq!(predefined.cid(0x41), None);
    assert_eq!(predefined.width(0x41), 800.0);
    assert!(predefined.vertical(0x41).is_some());
    // One PDFium doesn't know is two bytes each, each its CID, as PDFium reads it: a
    // name outside ISO's list; CNS-EUC-V, which PDFium's table hasn't.
    for name in ["Adobe-Korea1-V", "CNS-EUC-V"] {
        let predefined = type0(&mut pdf, name.into(), dict! { "DW" => 800 });
        let predefined = font_of(&pdf, predefined);
        assert!(!predefined.approx().codes, "{name}");
        assert_eq!(predefined.cid(0x41), Some(0x41));
        assert_eq!(predefined.width(0x41), 800.0);
        assert!(predefined.vertical(0x41).is_some());
    }
}

/// A predefined CMap the build has (hayro-cmap's bundle of Adobe's) and
/// PDFium knows: its codes split by PDFium's scheme for it, a byte, or
/// two where the first leads two (90ms-RKSJ-H), or two each
/// (UniJIS-UCS2-H), a code cut off at the end 0 for its second byte; each
/// code's CID Adobe's, by the code's length, 0 for a code it maps none
/// of; widths by those CIDs; `Tw` after a space of a byte; vertical where
/// its name ends in V. With no `/ToUnicode`, its text is as PDFium reads
/// it: a UCS2 CMap's codes, another's CIDs in Adobe-Japan1-UCS2, none for
/// the code 0.
#[cfg(feature = "predefined-cmaps")]
#[test]
fn predefined_cmaps_are_adobe_s() {
    let mut pdf = Pdf::default();
    let w = |pairs: &[(i64, i64)]| -> Object {
        let w = (pairs.iter()).flat_map(|&(cid, w)| [cid.into(), vec![w.into()].into()]);
        Object::from(w.collect::<Vec<Object>>())
    };
    let rksj = type0(
        &mut pdf,
        "90ms-RKSJ-H".into(),
        dict! { "W" => w(&[(264, 500), (843, 1000)]), "DW" => 700 },
    );
    let rksj = font_of(&pdf, rksj);
    assert!(!rksj.approx().codes);
    let split = codes(&rksj, b"A\x82\xa0\xb1\x88\x9f\x82");
    #[rustfmt::skip]
    assert_eq!(split, [(0..1, 0x41), (1..3, 0x82a0), (3..4, 0xb1), (4..6, 0x889f), (6..7, 0x8200)]);
    let cids = [0x41, 0x82a0, 0xb1, 0x889f, 0x8200, 0x20, 0x80].map(|c| rksj.cid(c));
    assert_eq!(cids, [264, 843, 343, 1125, 0, 231, 0].map(Some));
    let widths = [0x41, 0x82a0, 0xb1].map(|c| rksj.width(c));
    assert_eq!(widths, [500.0, 1000.0, 700.0]);
    assert!(rksj.word_spaced(32, 1));
    let text = rksj.text(b"A\x82\xa0\x88\x9f\xb1");
    assert_eq!(text.as_deref(), Some("Aあ亜ｱ"));
    assert_eq!(rksj.code_text(0x82a0).as_deref(), Some("あ"));
    assert!(!rksj.is_vertical());

    let ucs2 = type0(
        &mut pdf,
        "UniJIS-UCS2-H".into(),
        dict! { "W" => w(&[(34, 550)]) },
    );
    let ucs2 = font_of(&pdf, ucs2);
    assert!(!ucs2.approx().codes);
    let split = codes(&ucs2, b"\0A\x30\x42\x65");
    assert_eq!(split, [(0..2, 0x41), (2..4, 0x3042), (4..5, 0x6500)]);
    let cids = [0x41, 0x3042, 0x65e5, 0x20].map(|c| ucs2.cid(c));
    assert_eq!(cids, [34, 843, 3284, 1].map(Some));
    assert_eq!((ucs2.width(0x41), ucs2.width(0x3042)), (550.0, 1000.0));
    assert!(!ucs2.word_spaced(32, 2));
    let text = ucs2.text(b"\0A\x30\x42\x65\xe5");
    assert_eq!(text.as_deref(), Some("Aあ日"));
    assert_eq!(ucs2.text(b"\0\0"), None);

    let vertical = type0(&mut pdf, "90ms-RKSJ-V".into(), dict! {});
    let vertical = font_of(&pdf, vertical);
    assert!(vertical.is_vertical());
    let cids = (vertical.cid(0x8141), vertical.cid(0x41));
    assert_eq!(cids, (Some(7887), Some(264)));
}

/// A Type 0 font's text: its `/ToUnicode`'s where it has one, its codes
/// split by that's codespace; without one, as PDFium reads it, Identity's
/// CIDs in the UCS2 CMap of the collection its CIDFont's
/// `/CIDSystemInfo` orders, by name or string, its registry unread; none
/// for an Identity font of another ordering, nor for an embedded CMap's
/// CIDs.
#[cfg(feature = "predefined-cmaps")]
#[test]
fn type0_text_as_pdfium_reads_it() {
    let mut pdf = Pdf::default();
    let info = |ordering: Object| {
        dict! {
            "CIDSystemInfo" => dict! {
                "Registry" => Object::text("Other"), "Ordering" => ordering,
            },
        }
    };
    let text = |pdf: &mut Pdf, encoding: Object, descendant: Dictionary| {
        let font = type0(pdf, encoding, descendant);
        font_of(pdf, font).text(b"\x01\x08\x03\x4b")
    };
    let japan1 = info(Object::text("Japan1"));
    let named = info("Japan1".into());
    let identity = info(Object::text("Identity"));
    let ordered = text(&mut pdf, "Identity-H".into(), japan1.clone());
    assert_eq!(ordered.as_deref(), Some("Aあ"));
    let ordered = text(&mut pdf, "Identity-V".into(), named);
    assert_eq!(ordered.as_deref(), Some("Aあ"));
    assert_eq!(text(&mut pdf, "Identity-H".into(), identity), None);
    let cmap = pdf.add_object(Stream::new(
        dict! {},
        b"1 begincidrange <0000> <FFFF> 0 endcidrange".to_vec(),
    ));
    assert_eq!(text(&mut pdf, cmap.into(), japan1.clone()), None);
    // CID 0, `.notdef`, is Adobe's U+FFFD, as PDFium reads it, but a
    // string showing it isn't text: it's told from another by its bytes.
    let font = type0(&mut pdf, "Identity-H".into(), japan1.clone());
    let font = font_of(&pdf, font);
    assert_eq!(font.code_text(0).as_deref(), Some("\u{FFFD}"));
    assert_eq!(font.text(b"\x01\x08\x00\x00"), None);
    let font = type0(&mut pdf, "90ms-RKSJ-H".into(), japan1.clone());
    let font = font_of(&pdf, font);
    assert_eq!(font.cid(0x80), Some(0));
    assert_eq!(font.text(b"A"), Some("A".into()));
    assert_eq!(font.text(b"A\x80"), None);

    let to_unicode = pdf.add_object(Stream::new(
        dict! {},
        b"1 begincodespacerange <0000> <FFFF> endcodespacerange\n\
          2 beginbfchar <0108> <005A> <034B> <0059> endbfchar"
            .to_vec(),
    ));
    let mut font = type0(&mut pdf, "UniJIS-UCS2-H".into(), japan1);
    font.set("ToUnicode", to_unicode);
    let font = font_of(&pdf, font);
    assert_eq!(font.text(b"\x01\x08\x03\x4b").as_deref(), Some("ZY"));
    // A code it misses is the code's, as with no `/ToUnicode`.
    assert_eq!(font.text(b"\x30\x42").as_deref(), Some("あ"));
    assert_eq!(font.code_text(0x3042).as_deref(), Some("あ"));
}

/// A simple font over a `/ToUnicode` CMap of `mappings`.
fn over_to_unicode(pdf: &mut Pdf, mappings: &[u8]) -> Font {
    let to_unicode = pdf.add_object(Stream::new(dict! {}, mappings.to_vec()));
    let font = dict! {
        "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
        "ToUnicode" => to_unicode,
    };
    font_of(pdf, font)
}

/// What `mappings` make of `codes`, as a string each.
fn mapped(mappings: &[u8], codes: &[u32]) -> Vec<String> {
    let font = over_to_unicode(&mut Pdf::default(), mappings);
    (codes.iter())
        .map(|&code| font.code_text(code).unwrap_or_default().into_owned())
        .collect()
}

/// A `/ToUnicode`'s code is its last mapping's, whichever kind of mapping and however
/// the ranges lie: PDFium has a map of codes, and what a later range doesn't reach
/// stays the earlier's.
#[test]
fn a_to_unicode_s_later_mapping_is_over_an_earlier() {
    let codes = [0x41, 0x42, 0x43];
    let over =
        b"1 beginbfrange <00> <ff> <0400> endbfrange 1 beginbfrange <41> <41> <0041> endbfrange";
    assert_eq!(mapped(over, &codes), ["A", "\u{442}", "\u{443}"]);
    let over =
        b"1 beginbfrange <41> <43> <0410> endbfrange 1 beginbfrange <42> <42> <0041> endbfrange";
    assert_eq!(mapped(over, &codes), ["\u{410}", "A", "\u{412}"]);
    let over = b"1 beginbfrange <41> <43> <0410> endbfrange 1 beginbfchar <42> <0041> endbfchar";
    assert_eq!(mapped(over, &codes), ["\u{410}", "A", "\u{412}"]);
    let over = b"1 beginbfchar <42> <0041> endbfchar 1 beginbfrange <41> <43> <0410> endbfrange";
    assert_eq!(mapped(over, &codes), ["\u{410}", "\u{411}", "\u{412}"]);
    let over = b"1 beginbfchar <41> <0416> endbfchar 1 beginbfchar <41> <0417> endbfchar";
    assert_eq!(mapped(over, &[0x41]), ["\u{417}"]);
}

/// A `/ToUnicode`'s block is taken whole or not at all, as PDFium takes it: its count
/// what it holds, each code a code of 16 bits at most, each range going up from its
/// first by its last's low byte, an array as long as that. What's lost leaves the codes
/// to the encoding, and no other block.
#[test]
fn a_to_unicode_block_is_taken_whole_or_not_at_all() {
    let z = "\u{416}";
    for block in [
        &b"2 beginbfchar <41> <0416> endbfchar"[..],
        b"0 beginbfchar <41> <0416> endbfchar",
        b"beginbfchar <41> <0416> endbfchar",
        b"x beginbfchar <41> <0416> endbfchar",
        b"-1 beginbfchar <41> <0416> endbfchar",
        b"1 beginbfchar <42> <0417> x <41> <0416> endbfchar",
        b"<41> <0416> endbfchar",
        b"1 beginbfchar <41> <0416> x endbfchar",
        b"1 beginbfchar <41> <0416> <42> endbfchar",
        b"1 beginbfchar <41> <0416> <42> <0417> endbfchar",
        b"2 beginbfchar <41> <0416> <010042> <0417> endbfchar",
        b"2 beginbfchar <> <0417> <41> <0416> endbfchar",
        b"2 beginbfrange <41> <41> <0416> endbfrange",
        b"2 beginbfrange <> <41> <0417> <41> <41> <0416> endbfrange",
        b"1 beginbfrange <41> <41> <0416> <42> <42> <0417> endbfrange",
        b"2 beginbfrange <41> <40> <0417> <41> <41> <0416> endbfrange",
        b"2 beginbfrange <41> <41> <0416> <010042> <010042> <0417> endbfrange",
        b"1 beginbfrange <41> <42> [<0416>] endbfrange",
        b"1 beginbfrange <41> <41> [<0416> <0417>] endbfrange",
    ] {
        let name = String::from_utf8_lossy(block).into_owned();
        assert_eq!(mapped(block, &[0x41]), ["A"], "{name}");
    }
    // Another block still counts, beside one that's lost.
    let two = b"9 beginbfchar <42> <0417> endbfchar 1 beginbfchar <41> <0416> endbfchar";
    assert_eq!(mapped(two, &[0x41, 0x42]), [z, "B"]);
    // Of what looks wrong but is taken: a code of many leading zeros.
    let long = b"1 beginbfchar <0000000041> <0416> endbfchar";
    assert_eq!(mapped(long, &[0x41]), [z]);
    // A range past its low byte is read whole, where an array that's too short is by
    // that byte; and its low byte is what's read where it goes down, or past 16 bits.
    let past = b"1 beginbfrange <0041> <0142> <0416> endbfrange";
    assert_eq!(
        mapped(past, &[0x41, 0x142, 0x143]),
        ["\u{416}", "\u{517}", ""]
    );
    let whole = b"1 beginbfrange <0000> <FFFF> <0000> endbfrange";
    assert_eq!(mapped(whole, &[0x100, 0x3042]), ["\u{100}", "あ"]);
    let crossing = b"1 beginbfrange <00FF> <0100> [<0416> <0417>] endbfrange";
    assert_eq!(mapped(crossing, &[0xff, 0x100]), ["\u{416}", "\u{417}"]);
    let array = b"1 beginbfrange <0041> <0142> [<0416> <0417>] endbfrange";
    assert_eq!(
        mapped(array, &[0x41, 0x42, 0x142]),
        ["\u{416}", "\u{417}", ""]
    );
    let down = b"1 beginbfrange <0141> <0042> <0416> endbfrange";
    assert_eq!(
        mapped(down, &[0x141, 0x142, 0x143]),
        ["\u{416}", "\u{417}", ""]
    );
    let long = b"1 beginbfrange <0041> <010042> <0416> endbfrange";
    assert_eq!(
        mapped(long, &[0x41, 0x42, 0x43]),
        ["\u{416}", "\u{417}", "C"]
    );
}

/// A `/ToUnicode`'s values, PDFium's 16-bit units: U+FFFF and what's empty are no
/// text, so the encoding's; a lone surrogate, which no `str` holds, U+FFFD, and not
/// counted up from it; the last unit of several counted up, a surrogate pair's too.
#[test]
fn a_to_unicode_s_values_are_16_bit_units() {
    let codes = [0x41, 0x42, 0x43];
    let none = ["A", "B", "C"];
    assert_eq!(mapped(b"1 beginbfchar <41> <FFFF> endbfchar", &codes), none);
    assert_eq!(mapped(b"1 beginbfchar <41> <> endbfchar", &codes), none);
    // What's empty in a bfchar passes over; U+FFFF and an empty range map to no text.
    let kept = b"1 beginbfchar <41> <0416> endbfchar 1 beginbfchar <41> <> endbfchar";
    assert_eq!(mapped(kept, &[0x41]), ["\u{416}"]);
    let over = b"1 beginbfchar <41> <0416> endbfchar 1 beginbfchar <41> <FFFF> endbfchar";
    assert_eq!(mapped(over, &[0x41]), ["A"]);
    let over = b"1 beginbfchar <41> <0416> endbfchar 1 beginbfrange <41> <41> <> endbfrange";
    assert_eq!(mapped(over, &[0x41]), ["A"]);
    // Up through U+FFFF, 16 bits: no text, then the character 0.
    let past = b"1 beginbfrange <41> <43> <FFFE> endbfrange";
    assert_eq!(mapped(past, &codes), ["\u{fffe}", "B", "\0"]);
    let lone = b"1 beginbfrange <41> <43> <DD00> endbfrange";
    assert_eq!(mapped(lone, &codes), ["\u{fffd}"; 3]);
    let lone = b"1 beginbfchar <41> <DD00> endbfchar";
    assert_eq!(mapped(lone, &[0x41]), ["\u{fffd}"]);
    let several = b"1 beginbfrange <41> <42> <00660069> endbfrange";
    assert_eq!(mapped(several, &[0x41, 0x42]), ["fi", "fj"]);
    let pair = b"1 beginbfrange <41> <42> <D83DDE00> endbfrange";
    assert_eq!(mapped(pair, &[0x41, 0x42]), ["\u{1f600}", "\u{1f601}"]);
    let array = b"1 beginbfrange <41> <43> [<> /B <0416>] endbfrange";
    assert_eq!(mapped(array, &codes), ["A", "B", "\u{416}"]);
}

/// A Type 0 font's code its `/ToUnicode` misses is its CID's text, by code and in a
/// string; a `/ToUnicode` naming a collection's UCS2 CMap reads a simple font's so.
#[cfg(feature = "predefined-cmaps")]
#[test]
fn a_code_a_to_unicode_misses_is_its_cid_s_text() {
    let mut pdf = Pdf::default();
    let info = dict! {
        "CIDSystemInfo" => dict! {
            "Registry" => Object::text("Adobe"), "Ordering" => Object::text("Japan1"),
        },
    };
    let to_unicode = pdf.add_object(Stream::new(
        dict! {},
        b"1 beginbfchar <0108> <005A> endbfchar".to_vec(),
    ));
    let mut font = type0(&mut pdf, "Identity-H".into(), info);
    font.set("ToUnicode", to_unicode);
    let font = font_of(&pdf, font);
    assert_eq!(font.code_text(0x108).as_deref(), Some("Z"));
    assert_eq!(font.code_text(0x34b).as_deref(), Some("あ"));
    assert_eq!(font.text(b"\x01\x08\x03\x4b").as_deref(), Some("Zあ"));
    // CID 0 in a string has none, as with no `/ToUnicode`.
    assert_eq!(font.text(b"\x01\x08\x00\x00"), None);

    let named = b"/Adobe-Japan1-UCS2 usecmap 1 beginbfchar <41> <0416> endbfchar";
    let font = over_to_unicode(&mut pdf, named);
    assert_eq!(font.code_text(0x41).as_deref(), Some("\u{416}"));
    assert_eq!(font.code_text(0x42).as_deref(), Some("a"));
    let said = b"/CMapName /Adobe-Japan1-UCS2 def";
    assert_eq!(
        over_to_unicode(&mut pdf, said).code_text(0x42).as_deref(),
        Some("a")
    );
}

/// Identity-V: the vertical metrics `/W2` gives in both its forms, and
/// `/DW2`'s, centred on the width, for the rest.
#[test]
fn identity_v_metrics_from_w2_and_dw2() {
    let mut pdf = Pdf::default();
    let w2: Vec<Object> = vec![
        1.into(),
        vec![(-900).into(), 300.into(), 800.into()].into(),
        5.into(),
        9.into(),
        (-800).into(),
        250.into(),
        700.into(),
    ];
    let font = type0(
        &mut pdf,
        "Identity-V".into(),
        dict! {
            "Subtype" => "CIDFontType0", "W" => vec![3.into(), vec![600.into()].into()],
            "W2" => w2, "DW2" => vec![900.into(), (-1100).into()],
        },
    );
    let font = font_of(&pdf, font);
    let v = |w1, vx, vy| Some(VerticalMetrics { w1, vx, vy });
    assert_eq!(font.vertical(1), v(-900.0, 300.0, 800.0));
    assert_eq!(font.vertical(7), v(-800.0, 250.0, 700.0));
    assert_eq!(font.vertical(3), v(-1100.0, 300.0, 900.0));
    assert_eq!(font.vertical(4), v(-1100.0, 500.0, 900.0));
    let plain = type0(&mut pdf, "Identity-V".into(), dict! {});
    assert_eq!(font_of(&pdf, plain).vertical(4), v(-1000.0, 500.0, 880.0));
}

/// An embedded CMap: codes by its codespace, as PDFium splits them, a code
/// cut off at the end 0; CIDs by its `cidrange`s and `cidchar`s over
/// those of the CMap its `/UseCMap` names, `/WMode` from its program; a
/// CMap that uses itself read once; Identity used by `usecmap`; another
/// predefined CMap, by `usecmap` or `/UseCMap`, read for the codes nothing
/// else maps, by their length; each guessed, as PDFium reads none of them, as
/// is one the build hasn't.
#[test]
fn embedded_cmap_over_the_one_it_uses() {
    let mut pdf = Pdf::default();
    let base = pdf.add_object(Stream::new(
        dict! { "Type" => "CMap" },
        b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
          2 begincodespacerange <00> <7F> <8000> <FFFF> endcodespacerange\n\
          1 begincidrange <20> <7E> 1 endcidrange\n\
          endcmap CMapName currentdict /CMap defineresource pop end end"
            .to_vec(),
    ));
    let own = pdf.add_object(Stream::new(
        dict! { "Type" => "CMap", "UseCMap" => base },
        b"begincmap /WMode 1 def\n\
          1 begincidchar <41> 500 endcidchar\n\
          1 begincidrange <8140> <8142> 1000 endcidrange endcmap"
            .to_vec(),
    ));
    let font = type0(&mut pdf, own.into(), dict! { "Subtype" => "CIDFontType0" });
    let font = font_of(&pdf, font);
    let split = codes(&font, b"A B\x81\x41\x81");
    let expected = [
        (0..1, 0x41),
        (1..2, 0x20),
        (2..3, 0x42),
        (3..5, 0x8141),
        (5..6, 0),
    ];
    assert_eq!(split, expected);
    let cids = [0x41, 0x20, 0x42, 0x8141, 0x8142, 0x8000].map(|c| font.cid(c));
    assert_eq!(cids, [500, 1, 35, 1001, 1002, 0].map(Some));
    assert!(font.vertical(0x41).is_some());
    assert!(font.approx().codes);

    let mut looped = Stream::new(dict! {}, b"1 begincidchar <0005> 9 endcidchar".to_vec());
    let id = pdf.new_object_id();
    looped.dict.set("UseCMap", id);
    pdf.objects.insert(id, looped.into());
    let looped = type0(&mut pdf, id.into(), dict! {});
    let looped = font_of(&pdf, looped);
    assert_eq!(looped.cid(5), Some(9));
    assert_eq!(looped.vertical(5), None);

    let identity = pdf.add_object(Stream::new(
        dict! {},
        b"/Identity-H usecmap 1 begincidchar <0041> 7 endcidchar".to_vec(),
    ));
    let identity = type0(&mut pdf, identity.into(), dict! {});
    let identity = font_of(&pdf, identity);
    assert_eq!(codes(&identity, b"\0A\0B"), [(0..2, 0x41), (2..4, 0x42)]);
    assert_eq!(
        (identity.cid(0x41), identity.cid(0x42)),
        (Some(7), Some(0x42))
    );
    assert!(identity.approx().codes);
    // Its bundle, for the codes nothing else maps.
    #[cfg(feature = "predefined-cmaps")]
    {
        let other = pdf.add_object(Stream::new(
            dict! {},
            b"1 begincodespacerange <0000> <FFFF> endcodespacerange\n\
              /UniJIS-UCS2-H usecmap 1 begincidchar <0041> 7 endcidchar"
                .to_vec(),
        ));
        let other = type0(&mut pdf, other.into(), dict! {});
        let other = font_of(&pdf, other);
        let cids = [0x41, 0x3042, 0xd800].map(|c| other.cid(c));
        assert_eq!(cids, [7, 843, 0].map(Some));
        assert!(other.approx().codes);
        let named = pdf.add_object(Stream::new(
            dict! { "UseCMap" => "90ms-RKSJ-H" },
            b"1 begincodespacerange <00> <FF> endcodespacerange".to_vec(),
        ));
        let named = type0(&mut pdf, named.into(), dict! {});
        let named = font_of(&pdf, named);
        assert_eq!((named.cid(0x41), named.cid(0xb1)), (Some(264), Some(343)));
        assert!(named.approx().codes);
    }
    let unknown = pdf.add_object(Stream::new(dict! {}, b"/Adobe-Korea1-2 usecmap".to_vec()));
    let unknown = type0(&mut pdf, unknown.into(), dict! {});
    assert!(font_of(&pdf, unknown).approx().codes);
}

/// An embedded CMap's codes as PDFium splits them
/// (`CPDF_CMapParser::HandleCodeSpaceRange`, `CPDF_CMap::GetNextChar`):
/// by one range a byte each, but two where it's of two bytes, whatever
/// the bytes; by more, a byte at a time while the last range of more
/// bytes whose first byte holds theirs is longer, a code as long as that
/// range though its later bytes are past its bounds, and 0 for a byte no
/// range begins with.
#[test]
fn codes_split_as_pdfium_splits_them() {
    let mut pdf = Pdf::default();
    let mut cmap = |space: &str| {
        let program = format!("begincmap 2 begincodespacerange {space} endcodespacerange endcmap");
        let id = pdf.add_object(Stream::new(dict! {}, program.into_bytes()));
        let font = type0(&mut pdf, id.into(), dict! {});
        font_of(&pdf, font)
    };
    let four = cmap("<00000000> <FFFFFFFF>");
    assert_eq!(codes(&four, b"\x01\x02"), [(0..1, 1), (1..2, 2)]);
    let two = cmap("<8140> <9FFC>");
    assert_eq!(codes(&two, b"\x01\x02\x03"), [(0..2, 0x102), (2..3, 0x300)]);
    let mixed = cmap("<00> <7F> <8140> <817E>");
    let split = codes(&mixed, b"A\x81\x41\x81\x20\x90\x81");
    #[rustfmt::skip]
    assert_eq!(split, [(0..1, 0x41), (1..3, 0x8141), (3..5, 0x8120), (5..6, 0), (6..7, 0)]);
    assert!(!mixed.approx().codes);
}

/// What reading a font's streams decompresses to is charged to the budget
/// it's read within: its CharProcs, each stream once however many names
/// draw it, and its `/ToUnicode`; and an embedded CMap's chain. Past the
/// budget it fails, and the budget is as it was. A source's font, read
/// before, costs as much again ([`Fonts::get`]), so what a page costs is
/// the same whichever page read it first; within a scope, once by any
/// name, and a font written as a dictionary each time it's read.
#[test]
fn reading_a_font_is_charged_to_its_budget() {
    let mut pdf = Pdf::default();
    let proc = b"100 0 0 0 100 100 d1 0 0 100 100 re f".to_vec();
    let to_unicode = b"1 beginbfchar <61> <0041> endbfchar".to_vec();
    let cost = proc.len() + to_unicode.len();
    let proc = pdf.add_object(Stream::new(dict! {}, proc));
    let to_unicode = pdf.add_object(Stream::new(dict! {}, to_unicode));
    let type3 = dict! {
        "Type" => "Font", "Subtype" => "Type3",
        "FontMatrix" => vec![0.001.into(), 0.into(), 0.into(), 0.001.into(), 0.into(), 0.into()],
        "Encoding" => dict! { "Differences" => vec![97.into(), "a".into(), "b".into(), "c".into()] },
        "CharProcs" => dict! { "a" => proc, "b" => proc, "c" => proc },
        "ToUnicode" => to_unicode,
    };
    let lim = &Limits::DEFAULT;
    let mut budget = cost;
    let font = Font::read(&pdf, &type3, lim, &mut budget, &mut ()).unwrap();
    assert_eq!((font.width(98), budget), (100.0, 0));
    let mut budget = cost - 1;
    assert_eq!(
        Font::read(&pdf, &type3, lim, &mut budget, &mut ()).err(),
        Some(Error::TooLarge)
    );
    assert_eq!(budget, cost - 1);

    let by_ref = pdf.add_object(type3.clone());
    let resources: Object = dict! {
        "Font" => dict! { "R1" => by_ref, "R2" => by_ref, "D1" => type3.clone() },
    }
    .into();
    let fonts = Fonts::default();
    let mut budget = 10 * cost;
    fonts.get(&pdf, &by_ref.into(), lim, &mut budget).unwrap();
    fonts.get(&pdf, &by_ref.into(), lim, &mut budget).unwrap();
    assert_eq!(budget, 8 * cost);
    let mut budget = cost - 1;
    let again = fonts.get(&pdf, &by_ref.into(), lim, &mut budget);
    assert_eq!(again.err(), Some(Error::TooLarge));
    let mut scope = fonts.scope();
    let mut budget = 10 * cost;
    for name in [&b"R1"[..], b"R2", b"R1", b"D1", b"D1"] {
        let picked = scope.font(
            &pdf,
            ObjectId::new(1, 0),
            Some(&resources),
            name,
            lim,
            &mut budget,
        );
        assert!(picked.unwrap().is_some());
    }
    let picked = scope.font(
        &pdf,
        ObjectId::new(2, 0),
        Some(&resources),
        b"D1",
        lim,
        &mut budget,
    );
    assert!(picked.unwrap().is_some());
    // R1 and R2 once, and D1 once for each owner it's picked by.
    assert_eq!(budget, 7 * cost);

    // An embedded CMap's chain: the CMap, and the one it uses.
    let base_program = b"1 begincodespacerange <00> <FF> endcodespacerange".to_vec();
    let own_program = b"1 begincidrange <00> <FF> 0 endcidrange".to_vec();
    let chain = base_program.len() + own_program.len();
    let base = pdf.add_object(Stream::new(dict! {}, base_program));
    let own = pdf.add_object(Stream::new(dict! { "UseCMap" => base }, own_program));
    let font = type0(&mut pdf, own.into(), dict! {});
    let mut budget = chain;
    assert!(Font::read(&pdf, &font, lim, &mut budget, &mut ()).is_ok());
    assert_eq!(budget, 0);
    let mut budget = chain - 1;
    assert_eq!(
        Font::read(&pdf, &font, lim, &mut budget, &mut ()).err(),
        Some(Error::TooLarge)
    );
}

/// What an embedded CMap takes from one it uses, which PDFium doesn't read, is guessed:
/// a CID, its codespace, its `/WMode`, by a stream or Identity; but not where it takes
/// nothing, as PDFium reads it all the same, a code past its own ranges CID 0.
#[test]
fn what_a_cmap_takes_from_one_it_uses_is_guessed() {
    let mut pdf = Pdf::default();
    let top = b"1 begincidchar <0001> 2 endcidchar".to_vec();
    let mut over = |base: &[u8]| {
        let base = pdf.add_object(Stream::new(dict! {}, base.to_vec()));
        let own = pdf.add_object(Stream::new(dict! { "UseCMap" => base }, top.clone()));
        let font = type0(&mut pdf, own.into(), dict! {});
        font_of(&pdf, font)
    };
    let cid = over(b"1 begincidrange <0002> <0005> 1 endcidrange");
    assert_eq!(
        (cid.cid(1), cid.cid(2), cid.cid(6)),
        (Some(2), Some(1), Some(0))
    );
    assert!(cid.approx().codes);
    assert!(
        over(b"1 begincodespacerange <00> <ff> endcodespacerange")
            .approx()
            .codes
    );
    assert!(over(b"/WMode 1 def").approx().codes);
    assert!(!over(b"/WMode 0 def").approx().codes);
    let nothing = over(b"");
    assert_eq!((nothing.cid(1), nothing.cid(2)), (Some(2), Some(0)));
    assert!(!nothing.approx().codes);
    assert!(over(b"/Identity-H usecmap").approx().codes);
    let named = pdf.add_object(Stream::new(
        dict! { "UseCMap" => "Identity-H" },
        b"1 begincidchar <0001> 2 endcidchar".to_vec(),
    ));
    let named = type0(&mut pdf, named.into(), dict! {});
    assert!(font_of(&pdf, named).approx().codes);
}

/// An embedded CMap whose chain can't be read whole has its codes
/// guessed: a `/UseCMap` that's no stream, and a chain past
/// [`Limits::cmaps`].
#[test]
fn a_cmap_chain_read_in_part_is_guessed() {
    let mut pdf = Pdf::default();
    let program = || b"1 begincidrange <00> <FF> 0 endcidrange".to_vec();
    let nothing = pdf.add_object(Object::from(7));
    let broken = pdf.add_object(Stream::new(dict! { "UseCMap" => nothing }, program()));
    let broken = type0(&mut pdf, broken.into(), dict! {});
    assert!(font_of(&pdf, broken).approx().codes);
    // Those it uses take nothing here, so what's guessed is only where it's cut.
    let mut next = pdf.add_object(Stream::new(dict! {}, Vec::new()));
    next = pdf.add_object(Stream::new(dict! { "UseCMap" => next }, Vec::new()));
    next = pdf.add_object(Stream::new(dict! { "UseCMap" => next }, program()));
    let long = type0(&mut pdf, next.into(), dict! {});
    let mut lim = Limits::DEFAULT;
    lim.cmaps = 3;
    let read = |lim: &Limits| {
        let mut budget = BUDGET;
        Font::read(&pdf, &long, lim, &mut budget, &mut ())
            .unwrap()
            .approx()
            .codes
    };
    assert!(!read(&lim));
    lim.cmaps = 2;
    assert!(read(&lim));
}

/// What's told of a CMap's chain that can't be read whole: a chain back to a
/// CMap read, a loop, as the file's `UseCMap`, in the CMap it names again; and a
/// chain longer than [`Limits::cmaps`] as the cap's `TooManyCMaps`, in the first
/// CMap past it, and no `UseCMap`, the chain read up to it; but one as long as the
/// cap that ends in a reference to nothing as the file's `UseCMap`.
#[test]
fn a_cmap_chain_s_loop_and_its_cap_are_told_apart() {
    let mut pdf = Pdf::default();
    let program = || b"1 begincidrange <00> <FF> 0 endcidrange".to_vec();
    let first = pdf.new_object_id();
    let second = pdf.add_object(Stream::new(dict! { "UseCMap" => first }, Vec::new()));
    pdf.objects.insert(
        first,
        Stream::new(dict! { "UseCMap" => second }, program()).into(),
    );
    let looped = type0(
        &mut pdf,
        first.into(),
        dict! { "Subtype" => "CIDFontType2" },
    );
    assert_eq!(
        told(&pdf, &looped),
        [(Place::new(Some(first), None), Fault::UseCMap)]
    );

    // 513 CMaps, each using the one before: the first read is the last added.
    let mut next = pdf.add_object(Stream::new(dict! {}, Vec::new()));
    let past = next;
    for _ in 1..Limits::DEFAULT.cmaps {
        next = pdf.add_object(Stream::new(dict! { "UseCMap" => next }, Vec::new()));
    }
    next = pdf.add_object(Stream::new(dict! { "UseCMap" => next }, program()));
    let long = type0(&mut pdf, next.into(), dict! { "Subtype" => "CIDFontType2" });
    assert_eq!(
        told(&pdf, &long),
        [(Place::new(Some(past), None), Fault::TooManyCMaps)]
    );
    assert!(font_of(&pdf, long.clone()).approx().codes);
    assert_eq!(
        Fault::TooManyCMaps.to_string(),
        "a CMap's /UseCMap chain longer than is read"
    );
    // One fewer is read whole.
    let whole = (pdf.objects.get(&next).and_then(Object::as_stream))
        .and_then(|cmap| cmap.dict.get(b"UseCMap")?.as_reference())
        .unwrap();
    let whole = type0(
        &mut pdf,
        whole.into(),
        dict! { "Subtype" => "CIDFontType2" },
    );
    assert_eq!(told(&pdf, &whole), []);

    // 512 CMaps, the last using an object the file hasn't: no longer than the
    // cap, so the file's `UseCMap`, in what it names, and no `TooManyCMaps`.
    let nothing = ObjectId::new(99_999, 0);
    let mut next = pdf.add_object(Stream::new(dict! { "UseCMap" => nothing }, Vec::new()));
    for _ in 2..Limits::DEFAULT.cmaps {
        next = pdf.add_object(Stream::new(dict! { "UseCMap" => next }, Vec::new()));
    }
    next = pdf.add_object(Stream::new(dict! { "UseCMap" => next }, program()));
    let dangling = type0(&mut pdf, next.into(), dict! { "Subtype" => "CIDFontType2" });
    assert_eq!(
        told(&pdf, &dangling),
        [(Place::new(Some(nothing), None), Fault::UseCMap)]
    );
}

/// A `/ToUnicode` that's a stream whose data doesn't decode is told as one that's
/// no stream is, in words that say both, and its text read as with none.
#[test]
fn a_to_unicode_that_doesn_t_decode_is_told() {
    let mut pdf = Pdf::default();
    let map = b"1 beginbfchar <41> <0416> endbfchar".to_vec();
    let to_unicode = pdf.add_object(Stream::new(dict! {}, map));
    let font = dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica", "ToUnicode" => to_unicode };
    let undecoded = WithStreams::new(&pdf, |id, _| {
        Err(Unreadable::new(id, Why::Damaged, "it decodes to nothing"))
    });
    let mut told = Vec::new();
    let mut budget = BUDGET;
    let read = Font::read(undecoded, &font, &Limits::DEFAULT, &mut budget, &mut told).unwrap();
    assert_eq!(read.code_text(0x41).as_deref(), Some("A"));
    let at = font.get(b"ToUnicode").and_then(|value| value.at);
    assert_eq!(told, [(Place::new(None, at), Fault::ToUnicode)]);
    assert_eq!(
        Fault::ToUnicode.to_string(),
        "a /ToUnicode that's no stream, or doesn't decode"
    );
    assert_eq!(font_of(&pdf, font).code_text(0x41).as_deref(), Some("Ж"));
}

/// A Type 3 font: its widths and boxes through a `/FontMatrix` that isn't
/// a thousandth; a glyph's box from its CharProc's `d1`, else the
/// font's `/FontBBox`; a width `/Widths` doesn't give from its CharProc;
/// a code with no CharProc drawing nothing.
#[test]
fn type3_through_its_font_matrix() {
    let mut pdf = Pdf::default();
    let mut proc = |content: &[u8]| pdf.add_object(Stream::new(dict! {}, content.to_vec()));
    let (a, b, c) = (
        proc(b"50 0 10 0 60 80 d1 0 0 50 80 re f"),
        proc(b"40 0 d0 1 0 0 rg 0 0 40 40 re f"),
        proc(b"0 0 0 0 0 0 d1"),
    );
    let type3 = |bbox: Option<Vec<Object>>| {
        let mut font = dict! {
            "Type" => "Font", "Subtype" => "Type3",
            "FontMatrix" => vec![0.01.into(), 0.into(), 0.into(), 0.01.into(), 0.into(), 0.into()],
            "FirstChar" => 65, "Widths" => vec![50.into(), 0.into()],
            "Encoding" => dict! {
                "Differences" => vec![65.into(), "a".into(), "b".into(), "c".into()],
            },
            "CharProcs" => dict! { "a" => a, "b" => b, "c" => c },
        };
        if let Some(bbox) = bbox {
            font.set("FontBBox", bbox);
        }
        font
    };
    let bbox = vec![0.into(), (-10).into(), 100.into(), 90.into()];
    let font = font_of(&pdf, type3(Some(bbox)));
    assert_eq!(font.kind(), FontKind::Type3);
    assert_eq!(
        [65, 66, 67, 68].map(|c| font.width(c)),
        [500.0, 400.0, 0.0, 0.0]
    );
    // The box cut to whole thousandths as PDFium cuts it: -10 × 0.01 ×
    // 1000 is -99.99999 in an `f32`.
    let whole = rect(0.0, -99.0, 1000.0, 900.0);
    assert_eq!(font.glyph_box(65), Some(rect(100.0, 0.0, 600.0, 800.0)));
    assert_eq!(font.glyph_box(66), Some(whole));
    assert_eq!(font.glyph_box(67), Some(whole));
    assert_eq!(font.glyph_box(68), Some(Rect::default()));
    // `A`'s top, its `d1`'s, and the box's bottom, as there's no `g`.
    let metrics = font.metrics();
    assert_eq!((metrics.ascent, metrics.descent), (800.0, -99.0));
    assert_eq!(codes(&font, b"AB"), [(0..1, 65), (1..2, 66)]);
    assert!(!font.approx().any(), "{:?}", font.approx());
    // A matrix that turns glyph space over leaves the box's top below its
    // bottom, as PDFium keeps it, and the ascent, there being no `A`, the
    // box's top, held within it no longer; where `A`'s top is the box's
    // bottom, the heights are one, and a guess.
    let mut over = type3(Some(vec![0.into(), 0.into(), 1000.into(), 1000.into()]));
    let turned = [0.01f64, 0.0, 0.0, -0.01, 0.0, 0.0].map(Object::from);
    over.set("FontMatrix", turned.to_vec());
    let mut moved = over.clone();
    let differences = vec![66.into(), "a".into(), "b".into(), "c".into()];
    moved.set("Encoding", dict! { "Differences" => differences });
    let moved = font_of(&pdf, moved);
    let m = moved.metrics();
    assert_eq!(
        (m.ascent, m.descent, moved.approx().heights),
        (-10000.0, 0.0, false)
    );
    assert_eq!(moved.glyph_box(66), Some(rect(100.0, -800.0, 600.0, 0.0)));
    let over = font_of(&pdf, over);
    let m = over.metrics();
    assert_eq!(
        (m.ascent, m.descent, over.approx().heights),
        (0.0, 0.0, true)
    );
    // With no `/FontBBox`, a glyph `d1` gives no box for is a guess.
    let unbounded = font_of(&pdf, type3(None));
    assert_eq!(
        unbounded.glyph_box(65),
        Some(rect(100.0, 0.0, 600.0, 800.0))
    );
    assert_eq!(unbounded.glyph_box(66), None);
    assert!(unbounded.approx().boxes && unbounded.approx().heights);
}

/// Ascent and descent: the descriptor's within its `/FontBBox`, a
/// positive `Descent` read as below; else guessed, as PDFium takes them
/// from the face it draws in: the descriptor's, else the standard 14
/// font's AFM's, else the `/FontBBox`'s, else an em above and a quarter
/// below; and guessed where they're one, as PDFium then bounds a glyph by
/// its outline.
#[test]
fn heights_by_each_fallback() {
    let mut pdf = Pdf::default();
    let mut heights = |base: &str, descriptor: Option<Dictionary>| {
        let mut font =
            dict! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => base.to_string() };
        if let Some(d) = descriptor {
            font.set("FontDescriptor", pdf.add_object(d));
        }
        let font = font_of(&pdf, font);
        let m = font.metrics();
        (m.ascent, m.descent, font.approx().heights)
    };
    let bbox = |l: i64, b: i64, r: i64, t: i64| -> Object {
        vec![l.into(), b.into(), r.into(), t.into()].into()
    };
    let clamped =
        dict! { "Ascent" => 900, "Descent" => -300, "FontBBox" => bbox(0, -200, 1000, 800) };
    assert_eq!(heights("Foo", Some(clamped)), (800.0, -200.0, false));
    let positive = dict! {
        "Ascent" => 700, "Descent" => 250, "FontBBox" => bbox(0, -300, 1000, 900),
    };
    assert_eq!(heights("Foo", Some(positive)), (700.0, -250.0, false));
    let unboxed = dict! { "Ascent" => 700, "Descent" => -250 };
    assert_eq!(heights("Foo", Some(unboxed)), (700.0, -250.0, true));
    let boxed = dict! { "FontBBox" => bbox(-50, -150, 1000, 850) };
    assert_eq!(heights("Foo", Some(boxed)), (850.0, -150.0, true));
    let zeros = dict! { "Ascent" => 0, "Descent" => 0, "FontBBox" => bbox(0, 0, 0, 0) };
    assert_eq!(heights("Times-Roman", Some(zeros)), (683.0, -217.0, true));
    assert_eq!(heights("Symbol", None), (1010.0, -293.0, true));
    let short = dict! { "FontBBox" => bbox(0, -100, 1000, 600) };
    assert_eq!(heights("Helvetica", Some(short)), (600.0, -100.0, true));
    let tall = dict! { "Ascent" => 950, "Descent" => -100 };
    assert_eq!(heights("Helvetica", Some(tall)), (950.0, -100.0, true));
    assert_eq!(heights("Foo", None), (1000.0, -250.0, true));
    assert_eq!(heights("Foo", Some(dict! {})), (1000.0, -250.0, true));
    let one = dict! { "Ascent" => 5, "Descent" => 5, "FontBBox" => bbox(0, -200, 1000, 800) };
    assert_eq!(heights("Foo", Some(one)), (5.0, 5.0, true));
}

/// Arrays a font is read from never expand past their own length: a
/// `/W` range of 2³¹ CIDs is one range, a hundred thousand of them over
/// each other the first, and `/Widths` far longer than 256 codes, 256.
#[test]
fn hostile_metrics_stay_bounded() {
    let mut pdf = Pdf::default();
    let mut w: Vec<Object> = vec![0.into(), i64::from(i32::MAX).into(), 500.into()];
    for i in 0..100_000 {
        w.extend([0.into(), u32::MAX.into(), (i + 1).into()]);
    }
    let font = type0(&mut pdf, "Identity-H".into(), dict! { "W" => w, "DW" => 7 });
    let font = font_of(&pdf, font);
    assert_eq!(
        [0, 12345, i32::MAX as u32].map(|c| font.width(c)),
        [500.0; 3]
    );
    assert_eq!(font.width(u32::MAX), 7.0);
    let widths: Vec<Object> = (0..1_000_000).map(Object::from).collect();
    let simple = dict! { "Subtype" => "Type1", "FirstChar" => 0, "Widths" => widths };
    let simple = font_of(&pdf, simple);
    assert_eq!(simple.width(255), 255.0);
    let differences = vec![
        Object::from(1i64 << 40),
        "A".into(),
        Object::from(-5),
        "B".into(),
    ];
    let encoded = dict! {
        "Subtype" => "Type1", "BaseFont" => "Helvetica",
        "Encoding" => dict! { "Differences" => differences },
    };
    assert_eq!(font_of(&pdf, encoded).width(b'A'.into()), 667.0);
}

/// A source's fonts are read once each: an object's for every page, and
/// one written in the resources as a dictionary once an interpretation of content.
#[test]
fn fonts_are_read_once() {
    let mut pdf = Pdf::default();
    let helvetica =
        pdf.add_object(dict! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
    let resources: Object = dict! {
        "Font" => dict! {
            "F1" => dict! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier" },
            "F2" => helvetica,
        },
    }
    .into();
    let lim = &Limits::DEFAULT;
    let budget = &mut BUDGET.clone();
    let fonts = Fonts::default();
    let get = |font: &Object, budget: &mut usize| fonts.get(&pdf, font, lim, budget).unwrap();
    let by_ref = get(&helvetica.into(), budget).unwrap();
    assert!(Arc::ptr_eq(
        &by_ref,
        &get(&helvetica.into(), budget).unwrap()
    ));
    let (page, other) = (ObjectId::new(1, 0), ObjectId::new(2, 0));
    let mut pick = |scope: &mut FontScope<'_>, owner, name: &[u8]| {
        let picked = scope.font(&pdf, owner, Some(&resources), name, lim, budget);
        picked.unwrap()
    };
    let mut scope = fonts.scope();
    let f1 = pick(&mut scope, page, b"F1").unwrap();
    assert_eq!(scope.get(f1).width(b'A'.into()), 600.0);
    assert_eq!(pick(&mut scope, page, b"F1"), Some(f1));
    let other_f1 = pick(&mut scope, other, b"F1").unwrap();
    assert!(!std::ptr::eq(scope.get(f1), scope.get(other_f1)));
    let f2 = pick(&mut scope, page, b"F2").unwrap();
    assert!(std::ptr::eq(&*by_ref, scope.get(f2)));
    assert!(pick(&mut scope, page, b"F3").is_none());
    let mut again = fonts.scope();
    let f2 = pick(&mut again, other, b"F2").unwrap();
    assert!(std::ptr::eq(&*by_ref, again.get(f2)));
    assert!(get(&Object::from(1), budget).is_none());
}
/// A file's Arial draws at Helvetica's widths: the other half of
/// `standard_14_by_pdfium_s_names`.
#[test]
fn a_font_s_alternate_name_is_read_at_its_metrics() {
    let pdf = Pdf::default();
    let arial = font_of(
        &pdf,
        dict! { "Type" => "Font", "Subtype" => "TrueType", "BaseFont" => "Arial,Bold" },
    );
    assert_eq!(arial.width(b'A'.into()), 722.0);
    assert!(!arial.approx().widths);
}

/// The stock font, by name alone, and a font asked of nothing.
#[test]
fn the_stock_font_is_helvetica_in_win_ansi() {
    let fonts = Fonts::default();
    let stock = fonts.stock();
    assert_eq!(stock.width(b'A'.into()), 667.0);
    assert_eq!(stock.code_text(0x80).as_deref(), Some("€"));
    assert!(Arc::ptr_eq(&stock, &fonts.stock()));
    let mut scope = fonts.scope();
    let picked = scope.stock();
    assert_eq!(scope.stock(), picked);
    assert_eq!(scope.get(picked).width(b' '.into()), 278.0);
}

/// A font and what holds it cross threads, as a source's are shared.
#[test]
fn fonts_are_send_and_sync() {
    fn held<T: Send + Sync>() {}
    held::<Font>();
    held::<Arc<Font>>();
    held::<Fonts>();
    held::<FontScope<'_>>();
    held::<Codes<'_>>();
    held::<Error>();
}

/// Each kind of codes, walked over strings of bytes that lead and that don't, every
/// prefix and suffix of them: by `next` and by `fold` alike, each code a byte to four of
/// the string, from its start to its end in order; at every step its `size_hint` holds
/// what's left, exactly where each code takes as many bytes; and at the end, nothing,
/// again.
#[test]
fn codes_walk_alike_within_their_hints() {
    let mut pdf = Pdf::default();
    let embedded = |pdf: &mut Pdf, space: &str| {
        let program = format!("begincmap {space} endcmap");
        let id = pdf.add_object(Stream::new(dict! {}, program.into_bytes()));
        let font = type0(pdf, id.into(), dict! {});
        font_of(pdf, font)
    };
    let space = "3 begincodespacerange <00> <3F> <8000> <BFFF> <C0000000> <FFFFFFFF> \
                 endcodespacerange";
    let ranged = {
        let to_unicode = format!("begincmap {space} endcmap");
        let to_unicode = pdf.add_object(Stream::new(dict! {}, to_unicode.into_bytes()));
        let mut font = type0(&mut pdf, Object::from(7), dict! {});
        font.set("ToUnicode", to_unicode);
        font_of(&pdf, font)
    };
    let named = |pdf: &mut Pdf, name: &str| {
        let font = type0(pdf, name.into(), dict! {});
        font_of(pdf, font)
    };
    let not_a_name = type0(&mut pdf, Object::from(7), dict! {});
    let not_a_name = font_of(&pdf, not_a_name);
    // Each font, and whether each of its codes takes as many bytes, but the last.
    let fonts = [
        (
            "simple",
            font_of(&pdf, dict! { "Subtype" => "TrueType" }),
            true,
        ),
        ("identity", named(&mut pdf, "Identity-H"), true),
        (
            "one byte",
            embedded(
                &mut pdf,
                "1 begincodespacerange <00> <FF> endcodespacerange",
            ),
            true,
        ),
        (
            "two bytes",
            embedded(
                &mut pdf,
                "1 begincodespacerange <0000> <FFFF> endcodespacerange",
            ),
            true,
        ),
        ("mixed", embedded(&mut pdf, space), false),
        ("no codespace", embedded(&mut pdf, ""), true),
        // With the feature, codes by PDFium's scheme: a byte or two, and two bytes
        // each; without, the build hasn't them, and they're split as unknown.
        (
            "leading",
            named(&mut pdf, "90ms-RKSJ-H"),
            !cfg!(feature = "predefined-cmaps"),
        ),
        ("unicode", named(&mut pdf, "UniJIS-UCS2-H"), true),
        ("unknown", named(&mut pdf, "Adobe-Korea1-2"), true),
        ("not a name", not_a_name, true),
        ("by /ToUnicode", ranged, false),
    ];
    let bytes = b"\x41\x81\x40\xc3\x20\x00\xff\xe0\x7f\x82\x9f\xfc\x3f\xc0\x00\x00";
    let strings = (0..=bytes.len()).flat_map(|i| [&bytes[..i], &bytes[i..]]);
    for s in strings {
        for (kind, font, exact) in &fonts {
            let mut walk = font.codes(s);
            let mut by_next = Vec::new();
            loop {
                let left = walk.clone().count();
                let hint = walk.size_hint();
                assert!(
                    hint.0 <= left && hint.1.is_some_and(|most| left <= most),
                    "{kind}"
                );
                if *exact {
                    assert_eq!(hint, (left, Some(left)), "{kind}: {s:?}");
                }
                let Some(code) = walk.next() else {
                    break;
                };
                by_next.push(code);
            }
            assert_eq!(
                (walk.next(), walk.size_hint()),
                (None, (0, Some(0))),
                "{kind}"
            );
            let by_fold = font.codes(s).fold(Vec::new(), |mut codes, code| {
                codes.push(code);
                codes
            });
            assert_eq!(by_next, by_fold, "{kind}: {s:?}");
            let mut at = 0;
            for (range, _) in &by_next {
                assert!(
                    range.start == at && (1..=4).contains(&range.len()),
                    "{kind}: {s:?}"
                );
                at = range.end;
            }
            assert_eq!(at, s.len(), "{kind}: {s:?}");
        }
    }
}

/// A font's `Debug` says what it is and how much it holds, never a file's text: a
/// font whose `/ToUnicode` maps a code to a marker, and whose `/Differences` names a
/// glyph that's the marker's text, prints neither, nor the glyph's name; and its codes
/// print where they are, not the string.
#[test]
fn a_font_s_debug_holds_no_text_of_its_file() {
    let mut pdf = Pdf::default();
    let to_unicode = b"1 begincodespacerange <00> <FF> endcodespacerange \
        1 beginbfchar <42> <0051005A00580051> endbfchar";
    let to_unicode = pdf.add_object(Stream::new(dict! {}, to_unicode.to_vec()));
    let font = font_of(
        &pdf,
        dict! {
            "Subtype" => "TrueType", "BaseFont" => "Foo", "ToUnicode" => to_unicode,
            "Encoding" => dict! {
                "Differences" => vec![65.into(), Object::from("uni0051005A00580051")],
            },
        },
    );
    assert_eq!(font.code_text(0x42).as_deref(), Some("QZXQ"));
    assert_eq!(font.text(b"A").as_deref(), Some("QZXQ"));
    let printed = format!("{font:?}");
    // The marker as text, as its name's, and as the UTF-16 units a CMap holds it in.
    for marker in ["QZXQ", "0051", "uni0051", "81, 90, 88, 81"] {
        assert!(!printed.contains(marker), "{marker} in {printed}");
    }
    assert!(printed.contains("kind: TrueType") && printed.contains("to_unicode_ranges: 1"));
    assert_eq!(
        format!("{:?}", font.codes(b"QZXQ")),
        "Codes { at: 0, left: 4 }"
    );
    let fonts = Fonts::default();
    let id = pdf.add_object(dict! {
        "Subtype" => "TrueType", "BaseFont" => "Foo", "ToUnicode" => to_unicode,
    });
    let got = fonts.get(&pdf, &Object::from(id), &Limits::DEFAULT, &mut { BUDGET });
    assert!(got.unwrap().is_some());
    assert!(!format!("{fonts:?}").contains("QZXQ"));
}

/// `Tw` is added after a code 32 of a byte, as its CMap sizes it: never in Identity
/// or in a CMap of two bytes each, whatever its value and however many bytes it took
/// at a string's end; in one of a byte or mixed by its value; and by its length where
/// the CMap isn't known.
#[test]
fn a_code_is_word_spaced_by_how_its_cmap_sizes_it() {
    let mut pdf = Pdf::default();
    let embedded = |pdf: &mut Pdf, space: &str| {
        let program = format!("begincmap {space} endcmap");
        let id = pdf.add_object(Stream::new(dict! {}, program.into_bytes()));
        let font = type0(pdf, id.into(), dict! {});
        font_of(pdf, font)
    };
    let two = embedded(
        &mut pdf,
        "1 begincodespacerange <0000> <FFFF> endcodespacerange",
    );
    assert!(!two.word_spaced(32, 1) && !two.word_spaced(32, 2));
    let one = embedded(
        &mut pdf,
        "1 begincodespacerange <00> <FF> endcodespacerange",
    );
    assert!(one.word_spaced(32, 1) && one.word_spaced(32, 2) && !one.word_spaced(33, 1));
    let mixed = embedded(
        &mut pdf,
        "2 begincodespacerange <00> <7F> <8000> <FFFF> endcodespacerange",
    );
    assert!(mixed.word_spaced(32, 1) && mixed.word_spaced(32, 2));
    let none = embedded(&mut pdf, "");
    assert!(!none.word_spaced(32, 1) && !none.word_spaced(32, 2));

    let identity = type0(&mut pdf, "Identity-H".into(), dict! {});
    let identity = font_of(&pdf, identity);
    assert!(!identity.word_spaced(32, 1) && !identity.word_spaced(32, 2));
    let two_bytes = type0(&mut pdf, "Adobe-Korea1-2".into(), dict! {});
    let two_bytes = font_of(&pdf, two_bytes);
    assert!(!two_bytes.word_spaced(32, 1) && !two_bytes.word_spaced(32, 2));
    let unknown = type0(&mut pdf, Object::from(7), dict! {});
    let unknown = font_of(&pdf, unknown);
    assert!(unknown.word_spaced(32, 1) && !unknown.word_spaced(32, 2));
    assert!(!unknown.word_spaced(33, 1));
    let simple = font_of(&pdf, dict! { "Subtype" => "TrueType" });
    assert!(simple.word_spaced(32, 1) && !simple.word_spaced(33, 1));
}

/// Without the feature, a predefined CMap is one the build hasn't: its codes split
/// by the `/ToUnicode` codespace, or two bytes, its CIDs unknown, and said; and it
/// names its collection's text to no one.
#[cfg(not(feature = "predefined-cmaps"))]
#[test]
fn without_the_feature_a_predefined_cmap_is_guessed() {
    let mut pdf = Pdf::default();
    let font = type0(&mut pdf, "90ms-RKSJ-V".into(), dict! { "DW" => 800 });
    let font = font_of(&pdf, font);
    assert!(font.approx().codes && font.is_vertical());
    assert_eq!(font.cid(0x41), None);
    assert_eq!(font.width(0x41), 800.0);
    assert_eq!(
        codes(&font, b"\x41\x42\x43"),
        [(0..2, 0x4142), (2..3, 0x43)]
    );
    let identity = dict! {
        "CIDSystemInfo" => dict! { "Ordering" => Object::text("Japan1") },
    };
    let identity = type0(&mut pdf, "Identity-H".into(), identity);
    assert_eq!(font_of(&pdf, identity).text(b"\x01\x08"), None);
}

/// Each predefined CMap's name, the bundle's and each PDFium has a scheme for, as a
/// Type 0 font's `/Encoding`, by `usecmap` and by `/UseCMap`, and the font PDFium reads
/// as GBK: told as one the build hasn't exactly where a build with `predefined-cmaps`
/// reads it from its bundle, which tells nothing; and as one no build reads, in every
/// build, where neither does: a name PDFium knows that the bundle hasn't, and, as an
/// `/Encoding`, one of the bundle's PDFium doesn't know.
#[test]
fn a_predefined_cmap_the_build_hasn_t_is_the_build_s() {
    // PDFium knows these, which ISO 32000-2 doesn't predefine and the bundle hasn't.
    const UNBUNDLED: [&str; 5] = [
        "83pv-RKSJ-V",
        "90pv-RKSJ-V",
        "GBK2K-EUC-H",
        "GBK2K-EUC-V",
        "KSCpc-EUC-V",
    ];
    // The bundle has these, which PDFium doesn't know: ISO 32000-2's CNS-EUC, and
    // what maps Adobe's CIDs to their text.
    const UNKNOWN: [&str; 6] = [
        "Adobe-CNS1-UCS2",
        "Adobe-GB1-UCS2",
        "Adobe-Japan1-UCS2",
        "Adobe-Korea1-UCS2",
        "CNS-EUC-H",
        "CNS-EUC-V",
    ];
    let not_built = match cfg!(feature = "predefined-cmaps") {
        true => vec![],
        false => vec![Fault::PredefinedNotBuilt],
    };
    let faults = |pdf: &Pdf, font: &Dict| -> Vec<Fault> {
        told(pdf, font)
            .into_iter()
            .map(|(_, fault)| fault)
            .collect()
    };
    let names = crate::predefined::names();
    assert_eq!(names.len(), 70);
    let mut pdf = Pdf::default();
    for name in &names {
        let name = name.as_str();
        let identity = matches!(name, "Identity-H" | "Identity-V");
        let named = match name {
            _ if identity => vec![],
            _ if UNBUNDLED.contains(&name) || UNKNOWN.contains(&name) => {
                vec![Fault::PredefinedUnknown]
            }
            _ => not_built.clone(),
        };
        let font = type0(&mut pdf, name.into(), dict! { "Subtype" => "CIDFontType2" });
        assert_eq!(faults(&pdf, &font), named, "{name} as an /Encoding");
        let used = match name {
            _ if identity => vec![],
            _ if UNBUNDLED.contains(&name) => vec![Fault::PredefinedUnknown],
            _ => not_built.clone(),
        };
        let by_operator = format!("/{name} usecmap").into_bytes();
        let by_operator = pdf.add_object(Stream::new(dict! {}, by_operator));
        let by_key = pdf.add_object(Stream::new(dict! { "UseCMap" => name }, Vec::new()));
        for cmap in [by_operator, by_key] {
            let font = type0(&mut pdf, cmap.into(), dict! { "Subtype" => "CIDFontType2" });
            assert_eq!(faults(&pdf, &font), used, "{name} used by {cmap:?}");
        }
    }
    let gbk = pdf_objects::object::Value::Name(b"\xCB\xCE\xCC\xE5".to_vec());
    let gbk = dict! { "Subtype" => "TrueType", "BaseFont" => Object::new(gbk) };
    assert_eq!(faults(&pdf, &gbk), not_built, "GBK");
}

/// A resolver and streams of `pdf`'s that say `why` of `failing`.
#[derive(Clone, Copy)]
struct Failing<'o> {
    pdf: &'o Pdf,
    failing: ObjectId,
    why: Why,
}

impl<'o> Resolve<'o> for Failing<'o> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.try_resolve(id).ok().flatten()
    }

    fn try_resolve(&self, id: ObjectId) -> Result<Option<&'o Object>, Unreadable> {
        if id == self.failing {
            return Err(Unreadable::new(id, self.why, "it doesn't read"));
        }
        Ok(self.pdf.objects.get(&id))
    }
}

impl Streams for Failing<'_> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        self.pdf.decoded_by(id, limit)
    }
}

impl Pdf {
    /// Stream `id`'s data, lent, as `Streams` gives it of a map: none of what's no
    /// stream, and too large past `limit`.
    fn decoded_by(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
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

/// A font of each kind, reaching an object of each kind a font reads: the objects,
/// the fonts, and each object it reaches.
fn fonts_that_reach() -> (Pdf, Vec<(Dict, Vec<ObjectId>)>) {
    let mut pdf = Pdf::default();
    let map = |pdf: &mut Pdf| {
        pdf.add_object(Stream::new(
            dict! {},
            b"1 beginbfchar <41> <0041> endbfchar".to_vec(),
        ))
    };
    // Its `/Descent` no number, by reference, so that what's at fault is read too.
    let at_fault = pdf.add_object(Object::text("a"));
    let descriptor = pdf.add_object(dict! { "Ascent" => 700, "Descent" => at_fault });
    let widths = pdf.add_object(vec![Object::from(600)]);
    let number = pdf.add_object(Object::from(65));
    let encoding = pdf.add_object(dict! {
        "Differences" => vec![Object::from(65), Object::from("A")],
    });
    let to_unicode = map(&mut pdf);
    let simple = dict! {
        "Subtype" => "TrueType", "BaseFont" => "Foo", "FontDescriptor" => descriptor,
        "Widths" => widths, "FirstChar" => number, "Encoding" => encoding,
        "ToUnicode" => to_unicode,
    };
    let reached = vec![at_fault, descriptor, widths, number, encoding, to_unicode];
    let mut fonts = vec![(simple, reached)];

    let proc = pdf.add_object(Stream::new(dict! {}, b"100 0 d0".to_vec()));
    let procs = pdf.add_object(dict! { "a" => proc });
    let type3 = dict! {
        "Subtype" => "Type3", "CharProcs" => procs,
        "Encoding" => dict! { "Differences" => vec![Object::from(65), Object::from("a")] },
    };
    fonts.push((type3, vec![proc, procs]));

    let w = pdf.add_object(vec![Object::from(1), vec![Object::from(600)].into()]);
    let info = pdf.add_object(dict! { "Ordering" => Object::text("Identity") });
    let descriptor = pdf.add_object(dict! { "Ascent" => 700 });
    let cid_font = pdf.add_object(dict! {
        "Subtype" => "CIDFontType2", "W" => w, "CIDSystemInfo" => info,
        "FontDescriptor" => descriptor,
    });
    let descendants = pdf.add_object(vec![Object::from(cid_font)]);
    let identity = dict! {
        "Subtype" => "Type0", "Encoding" => "Identity-V", "DescendantFonts" => descendants,
    };
    fonts.push((identity, vec![w, info, descriptor, cid_font, descendants]));

    let base = pdf.add_object(Stream::new(dict! {}, b"begincmap endcmap".to_vec()));
    let cmap = pdf.add_object(Stream::new(
        dict! { "UseCMap" => base },
        b"begincmap endcmap".to_vec(),
    ));
    let embedded = dict! { "Subtype" => "Type0", "Encoding" => cmap };
    fonts.push((embedded, vec![cmap, base]));
    (pdf, fonts)
}

/// A stream where a font is wanted is none, though its dictionary is there, as PDFium
/// has it (`ToDictionary`); and it's a font's descriptor or CIDFont by its dictionary,
/// as PDFium reads those (`GetDict`).
#[test]
fn a_stream_is_no_dictionary() {
    let mut pdf = Pdf::default();
    let cid_font = pdf.add_object(Stream::new(dict! { "DW" => 500 }, Vec::new()));
    let by_ref = Object::from(cid_font);
    let got = Fonts::default().get(&pdf, &by_ref, &Limits::DEFAULT, &mut 0);
    assert!(got.unwrap().is_none());

    let descriptor = pdf.add_object(Stream::new(
        dict! { "Ascent" => 700, "Descent" => -200, "FontBBox" => vec![Object::from(0), Object::from(-200), Object::from(900), Object::from(700)] },
        Vec::new(),
    ));
    let font = font_of(
        &pdf,
        dict! { "Subtype" => "Type1", "FontDescriptor" => descriptor },
    );
    assert_eq!(font.metrics().ascent, 700.0);
    let font = dict! {
        "Subtype" => "Type0", "Encoding" => "Identity-H", "DescendantFonts" => vec![Object::from(cid_font)],
    };
    assert_eq!(font_of(&pdf, font).width(1), 500.0);
}

/// What each object a font reaches failing does to the font: `Why::Damaged` is an
/// object that isn't there, read as with none, and anything else the `Err`, whole,
/// and the budget as it was; never a font with its widths made up.
#[test]
fn each_why_over_a_failing_resolver() {
    let (pdf, fonts) = fonts_that_reach();
    for (font, reached) in &fonts {
        for &failing in reached {
            for why in [Why::Capped, Why::Locked, Why::Source] {
                let objects = Failing {
                    pdf: &pdf,
                    failing,
                    why,
                };
                let mut budget = BUDGET;
                let read = Font::read(objects, font, &Limits::DEFAULT, &mut budget, &mut ());
                let Err(Error::Unreadable(unreadable)) = read else {
                    panic!("{failing:?} {why:?}: {read:?}");
                };
                assert_eq!((unreadable.id, unreadable.why), (failing, why));
                assert_eq!(budget, BUDGET, "{failing:?} {why:?}");
                assert_eq!(Error::from(unreadable).to_string(), "it doesn't read");
            }
            let objects = Failing {
                pdf: &pdf,
                failing,
                why: Why::Damaged,
            };
            let mut budget = BUDGET;
            let mut told = Vec::new();
            Font::read(objects, font, &Limits::DEFAULT, &mut budget, &mut told)
                .unwrap_or_else(|e| panic!("{failing:?}: {e}"));
            let at = told.iter().find_map(|(place, fault)| match fault {
                Fault::Damaged { id } => Some((*id, place.object)),
                _ => None,
            });
            assert_eq!(at, Some((failing, Some(failing))), "{told:?}");
        }
    }
}

/// `Fonts` and `FontScope` say what a resolver says too: of the font itself, the
/// resources it's looked up in, the `/Font` dictionary. `Fonts` keeps nothing of it.
#[test]
fn fonts_and_their_scope_say_why_a_font_doesn_t_read() {
    let mut pdf = Pdf::default();
    let font = pdf.add_object(dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica" });
    let named = pdf.add_object(dict! { "F1" => font });
    let held = pdf.add_object(dict! { "Font" => named });
    let resources = Object::from(held);
    let lim = &Limits::DEFAULT;
    let by_ref = Object::from(font);
    for why in [Why::Capped, Why::Locked, Why::Source] {
        for failing in [font, named, held] {
            let objects = Failing {
                pdf: &pdf,
                failing,
                why,
            };
            let fonts = Fonts::default();
            let mut budget = 100;
            let picked = fonts.scope().font(
                objects,
                ObjectId::new(1000, 0),
                Some(&resources),
                b"F1",
                lim,
                &mut budget,
            );
            let Err(Error::Unreadable(unreadable)) = picked else {
                panic!("{failing:?} {why:?}: {picked:?}");
            };
            assert_eq!((unreadable.id, unreadable.why), (failing, why));
        }
        let objects = Failing {
            pdf: &pdf,
            failing: font,
            why,
        };
        let fonts = Fonts::default();
        let got = fonts.get(objects, &by_ref, lim, &mut 100);
        assert!(matches!(got, Err(Error::Unreadable(_))), "{got:?}");
        // Asked again of what reads, it's read: the failure isn't kept.
        assert!(fonts.get(&pdf, &by_ref, lim, &mut 100).unwrap().is_some());
    }
    let objects = Failing {
        pdf: &pdf,
        failing: font,
        why: Why::Damaged,
    };
    let got = Fonts::default().get(objects, &by_ref, lim, &mut 100);
    assert!(got.unwrap().is_none());
}

/// A font that doesn't read for a reason that's the caller's is read once in a scope,
/// however often it's picked, by its name or another: each pick gives the same `Err`,
/// and its stream is asked for once. Another scope reads it again, as `Fonts` keeps no
/// font that didn't read. One past the budget isn't kept, and within more it's read.
#[test]
fn a_scope_reads_a_font_that_doesn_t_read_once() {
    let mut pdf = Pdf::default();
    let map = b"1 beginbfchar <41> <0041> endbfchar".to_vec();
    let to_unicode = pdf.add_object(Stream::new(dict! {}, map));
    let font = dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica", "ToUnicode" => to_unicode };
    let font = pdf.add_object(font);
    let named = pdf.add_object(dict! { "F1" => font, "F2" => font });
    let resources = Object::from(pdf.add_object(dict! { "Font" => named }));
    let (page, lim) = (ObjectId::new(1000, 0), &Limits::DEFAULT);
    let asked = Cell::new(0);
    let capped = WithStreams::new(&pdf, |id, _| {
        asked.set(asked.get() + 1);
        Err(Unreadable::new(id, Why::Capped, "no"))
    });
    let failed = Error::Unreadable(Unreadable::new(to_unicode, Why::Capped, "no"));
    let fonts = Fonts::default();
    let mut scope = fonts.scope();
    for name in [b"F1", b"F1", b"F2", b"F1", b"F2"] {
        let mut budget = BUDGET;
        let picked = scope.font(capped, page, Some(&resources), name, lim, &mut budget);
        assert_eq!((picked, budget), (Err(failed.clone()), BUDGET));
    }
    assert_eq!(asked.get(), 1);
    let mut budget = BUDGET;
    let picked = fonts
        .scope()
        .font(capped, page, Some(&resources), b"F1", lim, &mut budget);
    assert_eq!((picked, asked.get()), (Err(failed), 2));

    let mut scope = fonts.scope();
    let mut budget = 1;
    let picked = scope.font(&pdf, page, Some(&resources), b"F1", lim, &mut budget);
    assert_eq!((picked, budget), (Err(Error::TooLarge), 1));
    let mut budget = BUDGET;
    let picked = scope.font(&pdf, page, Some(&resources), b"F1", lim, &mut budget);
    assert!(matches!(picked, Ok(Some(_))), "{picked:?}");
}

/// A font trusts no `Streams`: what's given past what was asked is too large, the
/// budget as it was, and what's given within it is charged; what's past a cap of what
/// gives it (`Why::Capped`), takes a password or is of a source that failed is the
/// `Err`, the caller's to know, as an object past a cap is; and a stream that's
/// damaged, or through a filter the reader doesn't decode, is none, what the font makes
/// of none told and nothing of the stream's own.
#[test]
fn a_font_holds_a_streams_to_what_it_asked() {
    let mut pdf = Pdf::default();
    let to_unicode = pdf.add_object(Stream::new(dict! {}, Vec::new()));
    let font = dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica", "ToUnicode" => to_unicode };
    let lim = &Limits::DEFAULT;
    type Given<'g> = &'g dyn Fn(usize) -> Result<Vec<u8>, Unreadable>;
    let read = |given: Given, budget: &mut usize, told: &mut Vec<(Place, Fault)>| {
        let asked = Cell::new(None);
        let objects = WithStreams::new(&pdf, |id, limit| {
            assert_eq!(id, to_unicode);
            asked.set(Some(limit));
            given(limit)
        });
        let read = Font::read(objects, &font, lim, budget, told);
        (read, asked.get())
    };
    let many = |_: usize| Ok(vec![b' '; 100]);
    for over in [0, 1, 99] {
        let mut budget = over;
        let (font, asked) = read(&many, &mut budget, &mut Vec::new());
        assert_eq!(font.err(), Some(Error::TooLarge));
        assert_eq!((budget, asked), (over, Some(over)));
    }
    let mut budget = 100;
    let (font, asked) = read(&many, &mut budget, &mut Vec::new());
    assert_eq!(font.unwrap().cost(), 100);
    assert_eq!((budget, asked), (0, Some(100)));

    let given = |why: Why| move |_: usize| Err(Unreadable::new(to_unicode, why, "no"));
    let mut budget = 100;
    let (font, _) = read(&given(Why::Limit), &mut budget, &mut Vec::new());
    assert_eq!(font.err(), Some(Error::TooLarge));
    assert_eq!(budget, 100);
    for why in [Why::Capped, Why::Locked, Why::Source] {
        let mut budget = 100;
        let (font, _) = read(&given(why), &mut budget, &mut Vec::new());
        let unreadable = Unreadable::new(to_unicode, why, "no");
        assert_eq!(font.err(), Some(Error::Unreadable(unreadable)));
        assert_eq!(budget, 100);
    }
    for why in [Why::Damaged, Why::Filter] {
        let mut told = Vec::new();
        let (font, _) = read(&given(why), &mut budget, &mut told);
        assert_eq!(font.unwrap().cost(), 0);
        assert_eq!(budget, 100);
        let faults: Vec<Fault> = told.into_iter().map(|(_, fault)| fault).collect();
        assert_eq!(faults, [Fault::ToUnicode], "{why:?}");
    }
}

/// A stream past a cap of what gives it (`Why::Capped`) is what the reading comes to, as
/// a locked one is: the `Err`, naming it, and the budget as it was. The reading goes on
/// past it as if it weren't there, but decodes no stream after it and tells nothing
/// after it: here a `/ToUnicode`, then the CMap, never asked for, and a `/W` cut short,
/// none of them told, as what it would tell is of no font of the file's.
#[test]
fn a_font_reads_on_past_a_capped_stream() {
    let mut pdf = Pdf::default();
    let to_unicode = b"1 beginbfchar <41> <0041> endbfchar".to_vec();
    let to_unicode = pdf.add_object(Stream::new(dict! {}, to_unicode));
    let cmap = b"1 begincidrange <00> <FF> 0 endcidrange".to_vec();
    let cmap = pdf.add_object(Stream::new(dict! {}, cmap));
    let w = vec![
        Object::from(1),
        Object::from(2),
        vec![Object::from(600)].into(),
    ];
    let cid_font = pdf.add_object(dict! { "Subtype" => "CIDFontType2", "W" => w });
    let font = dict! {
        "Subtype" => "Type0", "BaseFont" => "Foo", "Encoding" => cmap,
        "DescendantFonts" => vec![Object::from(cid_font)], "ToUnicode" => to_unicode,
    };
    for why in [Why::Capped, Why::Locked] {
        let asked = RefCell::new(Vec::new());
        let objects = WithStreams::new(&pdf, |id, limit| {
            asked.borrow_mut().push(id);
            match id == to_unicode {
                true => Err(Unreadable::new(id, why, "no")),
                false => pdf.decoded_by(id, limit).map(Cow::into_owned),
            }
        });
        let mut budget = BUDGET;
        let mut told = Vec::new();
        let read = Font::read(objects, &font, &Limits::DEFAULT, &mut budget, &mut told);
        let failed = Unreadable::new(to_unicode, why, "no");
        assert_eq!(read.err(), Some(Error::Unreadable(failed)), "{why:?}");
        assert_eq!(budget, BUDGET);
        assert_eq!(asked.into_inner(), [to_unicode]);
        assert_eq!(told, [], "{why:?}");
    }
    // Read whole, the `/W` is told: of the font the failure hid.
    let (mut budget, mut told) = (BUDGET, Vec::new());
    Font::read(&pdf, &font, &Limits::DEFAULT, &mut budget, &mut told).unwrap();
    let told: Vec<(Option<ObjectId>, Fault)> = told
        .into_iter()
        .map(|(place, fault)| (place.object, fault))
        .collect();
    assert_eq!(told, [(Some(cid_font), Fault::W)]);
}

/// What a reading told before it failed is told, and nothing after: here an
/// `/Encoding` that's no encoding, before a `/ToUnicode` that doesn't read. Read whole,
/// the font tells its `/Widths` missing after it, and a `/ToUnicode` that doesn't
/// decode before that.
#[test]
fn a_failed_reading_tells_what_it_found_before_it() {
    let mut pdf = Pdf::default();
    let to_unicode = pdf.add_object(Stream::new(dict! {}, b"no CMap".to_vec()));
    let font = dict! {
        "Subtype" => "Type1", "BaseFont" => "Foo", "Encoding" => "Nope",
        "ToUnicode" => to_unicode,
    };
    let read = |why: Option<Why>| {
        let objects = WithStreams::new(&pdf, |id, limit| match why {
            Some(why) => Err(Unreadable::new(id, why, "no")),
            None => pdf.decoded_by(id, limit).map(Cow::into_owned),
        });
        let (mut budget, mut told) = (BUDGET, Vec::new());
        let read = Font::read(objects, &font, &Limits::DEFAULT, &mut budget, &mut told);
        let told: Vec<Fault> = told.into_iter().map(|(_, fault)| fault).collect();
        (read.err(), told)
    };
    let whole = vec![Fault::Encoding, Fault::NoWidths];
    assert_eq!(read(None), (None, whole));
    let damaged = vec![Fault::Encoding, Fault::ToUnicode, Fault::NoWidths];
    assert_eq!(read(Some(Why::Damaged)), (None, damaged));
    for why in [Why::Capped, Why::Locked, Why::Source, Why::Detached] {
        let (error, told) = read(Some(why));
        let failed = Unreadable::new(to_unicode, why, "no");
        assert_eq!(error, Some(Error::Unreadable(failed)));
        assert_eq!(told, [Fault::Encoding], "{why:?}");
    }
}

/// A font's program stream: one that doesn't decode is the file's, a program that
/// doesn't read (`ProgramMalformed`); one through filters the reader doesn't decode is
/// told so too, as the caller tells apart by its own `Streams`; and one, or its object,
/// that doesn't read for a reason that's the caller's is the `Err` alone, its object
/// and its data alike, nothing told. Without `programs`, or read for text, no program's
/// data is asked for, and only its object can fail the reading.
#[test]
fn a_program_that_doesn_t_read_is_told_where_it_s_the_file_s() {
    let mut pdf = Pdf::default();
    let file = pdf.add_object(Stream::new(dict! {}, b"no program".to_vec()));
    let descriptor = pdf.add_object(dict! { "Type" => "FontDescriptor", "FontFile2" => file });
    let font = dict! {
        "Subtype" => "TrueType", "BaseFont" => "Foo", "FirstChar" => 65,
        "Widths" => vec![Object::from(600)], "FontDescriptor" => descriptor,
    };
    let programs = cfg!(feature = "programs");
    for text in [false, true] {
        let read = |objects: Failing, data: Option<Why>| {
            let streams = WithStreams::new(objects, |id, limit| match data {
                Some(why) => Err(Unreadable::new(id, why, "no")),
                None => pdf.decoded_by(id, limit).map(Cow::into_owned),
            });
            let (lim, mut budget, mut told) = (&Limits::DEFAULT, BUDGET, Vec::new());
            let read = match text {
                true => Font::read_for_text(streams, &font, lim, &mut budget, &mut told),
                false => Font::read(streams, &font, lim, &mut budget, &mut told),
            };
            let told: Vec<(Option<ObjectId>, Fault)> = told
                .into_iter()
                .map(|(place, fault)| (place.object, fault))
                .collect();
            (read.map(|font| font.program()), budget, told)
        };
        let reads = programs && !text;
        let fine = Failing {
            pdf: &pdf,
            failing: ObjectId::new(1000, 0),
            why: Why::Damaged,
        };
        for why in [Why::Damaged, Why::Filter] {
            let (program, budget, told) = read(fine, Some(why));
            let malformed = (
                FontProgram::Malformed,
                vec![(Some(file), Fault::ProgramMalformed)],
            );
            let (expected, faults) = match reads {
                true => malformed,
                false => (FontProgram::None, Vec::new()),
            };
            assert_eq!(
                (program, budget, told),
                (Ok(expected), BUDGET, faults),
                "{why:?}"
            );
        }
        for why in [Why::Capped, Why::Locked, Why::Source] {
            let failed = Error::Unreadable(Unreadable::new(file, why, "no"));
            // Its data past a cap, and its object: alike, nothing told.
            let (program, budget, told) = read(fine, Some(why));
            match reads {
                true => assert_eq!((program, budget, &told), (Err(failed), BUDGET, &vec![])),
                false => assert_eq!((program, &told), (Ok(FontProgram::None), &vec![])),
            }
            let object = Failing {
                pdf: &pdf,
                failing: file,
                why,
            };
            let (program, budget, told) = read(object, None);
            let failed = Error::Unreadable(Unreadable::new(file, why, "it doesn't read"));
            assert_eq!(
                (program, budget, told),
                (Err(failed), BUDGET, vec![]),
                "{why:?}"
            );
        }
    }
}

/// A stream a font reads is asked for by its own number, as a reading says of a
/// reference that it's no stream: a `/ToUnicode`, a CMap and the one it uses, a
/// CharProc. As in PDFium, the value refers to the stream itself: a reference to a
/// reference to one is none.
#[test]
fn a_stream_is_asked_for_by_its_own_number() {
    let mut pdf = Pdf::default();
    let map = b"1 beginbfchar <41> <0416> endbfchar".to_vec();
    let to_unicode = pdf.add_object(Stream::new(dict! {}, map));
    let simple = dict! { "Subtype" => "Type1", "BaseFont" => "Foo", "ToUnicode" => to_unicode };
    assert_eq!(font_of(&pdf, simple).code_text(0x41).as_deref(), Some("Ж"));
    let once = pdf.add_object(Object::from(to_unicode));
    let simple = dict! { "Subtype" => "Type1", "BaseFont" => "Foo", "ToUnicode" => once };
    assert_eq!(font_of(&pdf, simple).code_text(0x41).as_deref(), Some("A"));

    let base = pdf.add_object(Stream::new(
        dict! {},
        b"1 begincidchar <0005> 9 endcidchar".to_vec(),
    ));
    let cmap = pdf.add_object(Stream::new(
        dict! { "UseCMap" => base },
        b"1 begincidchar <0006> 8 endcidchar".to_vec(),
    ));
    let font = type0(&mut pdf, cmap.into(), dict! {});
    let font = font_of(&pdf, font);
    assert_eq!((font.cid(5), font.cid(6)), (Some(9), Some(8)));
    assert!(font.approx().codes);
    // Where the one it uses is by a chain, what's read of it is all that's known.
    let once = pdf.add_object(Object::from(base));
    let cut = pdf.add_object(Stream::new(
        dict! { "UseCMap" => once },
        b"1 begincidchar <0006> 8 endcidchar".to_vec(),
    ));
    let font = type0(&mut pdf, cut.into(), dict! {});
    let font = font_of(&pdf, font);
    assert_eq!((font.cid(5), font.cid(6)), (Some(0), Some(8)));
    assert!(font.approx().codes);
    // A chain to the CMap itself: PDFium doesn't load such a font.
    let once = pdf.add_object(Object::from(cmap));
    let font = type0(&mut pdf, once.into(), dict! {});
    let mut told = Vec::new();
    let mut budget = BUDGET;
    let font = Font::read(&pdf, &font, &Limits::DEFAULT, &mut budget, &mut told).unwrap();
    assert!(font.approx().codes);
    assert!(told.iter().any(|(_, fault)| *fault == Fault::CMap));

    let proc = pdf.add_object(Stream::new(dict! {}, b"100 0 d0".to_vec()));
    let type3 = |proc: ObjectId| {
        dict! {
            "Subtype" => "Type3", "CharProcs" => dict! { "a" => proc },
            "Encoding" => dict! { "Differences" => vec![Object::from(65), Object::from("a")] },
        }
    };
    assert_eq!(font_of(&pdf, type3(proc)).width(65), 100_000.0);
    let once = pdf.add_object(Object::from(proc));
    assert_eq!(font_of(&pdf, type3(once)).width(65), 0.0);

    // A chain back to itself reads as no stream, and ends.
    let mut pdf = Pdf::default();
    let looped = pdf.new_object_id();
    pdf.objects.insert(looped, Object::from(looped));
    let font = dict! { "Subtype" => "Type1", "ToUnicode" => looped };
    let mut told = Vec::new();
    let mut budget = BUDGET;
    let font = Font::read(&pdf, &font, &Limits::DEFAULT, &mut budget, &mut told).unwrap();
    assert_eq!(font.code_text(0x41).as_deref(), Some("A"));
    assert!(told.iter().any(|(_, fault)| *fault == Fault::ToUnicode));
}

/// `width`, written as `value` in a simple font's `/Widths` at code 65.
fn simple_width(pdf: &Pdf, value: Object) -> f32 {
    let font = dict! {
        "Subtype" => "TrueType", "BaseFont" => "Foo", "FirstChar" => 65, "Widths" => vec![value],
    };
    font_of(pdf, font).width(65)
}

/// A number as PDFium's integer accessor reads it, and a simple font's width as the 16
/// bits it holds it in: a real cut from its 32-bit float, `true` 1, what's no number
/// and what's past 32 bits 0, past 31 bits a negative; a reference followed once.
#[test]
fn a_simple_font_s_widths_are_pdfium_s_integers_in_16_bits() {
    let mut pdf = Pdf::default();
    for (value, width) in [
        (Object::from(600), 600.0),
        (Object::from(600.9), 600.0),
        (Object::from(true), 1.0),
        (Object::from(false), 0.0),
        (Object::text("a"), 0.0),
        (Object::null(), 0.0),
        (Object::from("Name"), 0.0),
        (Object::from(vec![Object::from(700)]), 0.0),
        (Object::from(4294967296_i64), 0.0),
        (Object::from(2147483648_i64), 0.0),
        (Object::from(2147484148_i64), 500.0),
        (Object::from(-500.7), 65036.0),
        (Object::from(-500), 65036.0),
        (Object::from(70000), 4464.0),
        // What's held at 0xffff is what PDFium takes for none.
        (Object::from(65535), 0.0),
        (Object::from(-1), 0.0),
        (Object::from(16777217.0), 0.0),
    ] {
        assert_eq!(simple_width(&pdf, value.clone()), width, "{value:?}");
    }
    let number = pdf.add_object(Object::from(700));
    assert_eq!(simple_width(&pdf, number.into()), 700.0);
    let chain = pdf.add_object(Object::from(number));
    assert_eq!(simple_width(&pdf, chain.into()), 0.0);
    let yes = pdf.add_object(Object::from(true));
    assert_eq!(simple_width(&pdf, yes.into()), 1.0);
    assert_eq!(simple_width(&pdf, Object::reference(999)), 0.0);
}

/// A CID font's widths are PDFium's integers in 32 bits, `/DW` in 16.
#[test]
fn a_cid_font_s_widths_are_pdfium_s_integers() {
    let mut pdf = Pdf::default();
    let number = pdf.add_object(Object::from(700));
    let chain = pdf.add_object(Object::from(number));
    let w = vec![
        Object::from(1),
        vec![
            Object::from(true),
            Object::from(70000),
            Object::from(4294967296_i64),
            Object::from(2147483648_i64),
            Object::from(-500.7),
            Object::text("a"),
            number.into(),
            chain.into(),
        ]
        .into(),
    ];
    let font = type0(&mut pdf, "Identity-H".into(), dict! { "W" => w });
    let font = font_of(&pdf, font);
    assert_eq!(
        [1, 2, 3, 4, 5, 6, 7, 8].map(|c| font.width(c)),
        [1.0, 70000.0, 0.0, -2147483648.0, -500.0, 0.0, 700.0, 0.0]
    );
    for (dw, width) in [
        (Object::from(700.9), 700.0),
        (Object::from(true), 1.0),
        (Object::text("a"), 0.0),
        (Object::null(), 0.0),
        (Object::from(70000), 4464.0),
        (Object::from(32768), -32768.0),
        (number.into(), 700.0),
        (chain.into(), 0.0),
        (Object::reference(999), 0.0),
    ] {
        let font = type0(&mut pdf, "Identity-H".into(), dict! { "DW" => dw.clone() });
        assert_eq!(font_of(&pdf, font).width(1), width, "{dw:?}");
    }
    let font = type0(&mut pdf, "Identity-H".into(), dict! {});
    assert_eq!(font_of(&pdf, font).width(1), 1000.0);
}

/// `/W` as PDFium's `LoadMetricsArray` reads it, what's written by a chain included.
#[test]
fn a_cid_font_s_w_as_pdfium_reads_it() {
    let mut pdf = Pdf::default();
    let widths = |pdf: &mut Pdf, w: Vec<Object>, cids: [u32; 3]| {
        let font = type0(pdf, "Identity-H".into(), dict! { "W" => w });
        cids.map(|c| font_of(pdf, font.clone()).width(c))
    };
    let numbers = |ns: &[i32]| -> Object {
        Object::from(ns.iter().map(|&n| Object::from(n)).collect::<Vec<_>>())
    };
    // A first CID below 0 is none, and those after it the CIDs they are.
    let w = vec![(-2).into(), numbers(&[100, 200, 300, 400])];
    assert_eq!(widths(&mut pdf, w, [0, 1, 2]), [300.0, 400.0, 1000.0]);
    // `true` a CID.
    let w = vec![true.into(), numbers(&[100, 200])];
    assert_eq!(widths(&mut pdf, w, [0, 1, 2]), [1000.0, 100.0, 200.0]);
    // A CID past what an `int` holds with its values is passed over, and it goes on.
    let w = vec![
        i32::MAX.into(),
        numbers(&[100, 200]),
        1.into(),
        numbers(&[300]),
    ];
    assert_eq!(widths(&mut pdf, w, [0, 1, 2]), [1000.0, 300.0, 1000.0]);
    // A reference to nothing is passed over, and one to a reference is its number.
    let number = pdf.add_object(Object::from(1));
    let chain = pdf.add_object(Object::from(number));
    let w = vec![Object::reference(999), chain.into(), numbers(&[300])];
    assert_eq!(widths(&mut pdf, w, [0, 1, 2]), [1000.0, 300.0, 1000.0]);
    // An array by a reference to it; by a chain it's a number, 0, where a last goes.
    let list = pdf.add_object(numbers(&[300]));
    let w = vec![1.into(), list.into()];
    assert_eq!(widths(&mut pdf, w, [0, 1, 2]), [1000.0, 300.0, 1000.0]);
    let once = pdf.add_object(Object::from(list));
    let w = vec![1.into(), once.into(), 500.into()];
    assert_eq!(widths(&mut pdf, w, [0, 1, 2]), [1000.0, 1000.0, 1000.0]);
    // An array where the last CID goes ends it, and where a value goes.
    let w = vec![
        1.into(),
        2.into(),
        numbers(&[300]),
        1.into(),
        2.into(),
        400.into(),
    ];
    assert_eq!(widths(&mut pdf, w, [0, 1, 2]), [1000.0, 1000.0, 1000.0]);
    // A range last below 0 gives none; one first gives from 0.
    let w = vec![
        (-5).into(),
        (-1).into(),
        300.into(),
        (-5).into(),
        1.into(),
        400.into(),
    ];
    assert_eq!(widths(&mut pdf, w, [0, 1, 2]), [400.0, 400.0, 1000.0]);
    // By a reference it's read; by a chain, none is.
    let w = pdf.add_object(Object::from(vec![Object::from(1), numbers(&[300])]));
    let by = type0(&mut pdf, "Identity-H".into(), dict! { "W" => w });
    assert_eq!(font_of(&pdf, by).width(1), 300.0);
    let chain = pdf.add_object(Object::from(w));
    let by = type0(&mut pdf, "Identity-H".into(), dict! { "W" => chain });
    assert_eq!(font_of(&pdf, by).width(1), 1000.0);
    // A CMap's CID is 16 bits.
    let cmap = pdf.add_object(Stream::new(
        dict! {},
        b"1 begincodespacerange <00> <ff> endcodespacerange 1 begincidrange <41> <42> 65601 endcidrange"
            .to_vec(),
    ));
    let font = type0(&mut pdf, cmap.into(), dict! {});
    assert_eq!(font_of(&pdf, font).cid(0x42), Some(66));
}

/// A simple font's `/FirstChar` and `/LastChar`, as they go into PDFium's `size_t`; and
/// where it starts from, its `/MissingWidth` or a fixed-pitch standard font's own.
#[test]
fn a_simple_font_s_range_of_widths() {
    let mut pdf = Pdf::default();
    let of = |pdf: &Pdf, extra: Dict, code: u32| {
        let mut font = dict! {
            "Subtype" => "TrueType", "BaseFont" => "Foo",
            "Widths" => vec![Object::from(500), Object::from(600)],
        };
        for (key, value) in extra.iter() {
            font.set(key, value.clone());
        }
        font_of(pdf, font).width(code)
    };
    let first = |n: Object| dict! { "FirstChar" => n };
    assert_eq!(of(&pdf, first(65.9.into()), 65), 500.0);
    assert_eq!(of(&pdf, first(true.into()), 1), 500.0);
    assert_eq!(of(&pdf, first(Object::text("a")), 0), 500.0);
    assert_eq!(of(&pdf, first((-1).into()), 0), 0.0);
    assert_eq!(of(&pdf, first(300.into()), 44), 0.0);
    assert_eq!(of(&pdf, first(4294967361_i64.into()), 65), 0.0);
    let chain = {
        let number = pdf.add_object(Object::from(65));
        pdf.add_object(Object::from(number))
    };
    assert_eq!(of(&pdf, first(chain.into()), 0), 500.0);
    assert_eq!(of(&pdf, first(chain.into()), 65), 0.0);
    let last = |n: Object| dict! { "FirstChar" => 65, "LastChar" => n };
    assert_eq!(of(&pdf, last((-1).into()), 66), 600.0);
    assert_eq!(of(&pdf, last(true.into()), 65), 0.0);
    assert_eq!(of(&pdf, last(65.9.into()), 66), 0.0);
    assert_eq!(of(&pdf, last(60.into()), 65), 0.0);
    assert_eq!(of(&pdf, last(70.into()), 66), 600.0);

    // An empty `/Widths` from code 0 writes 0 over `/MissingWidth`.
    let descriptor = pdf.add_object(dict! { "MissingWidth" => 777 });
    let empty = |first: i32| {
        dict! {
            "Subtype" => "TrueType", "BaseFont" => "Foo", "FirstChar" => first,
            "FontDescriptor" => descriptor, "Widths" => Vec::<Object>::new(),
        }
    };
    assert_eq!(font_of(&pdf, empty(0)).width(65), 0.0);
    assert_eq!(font_of(&pdf, empty(65)).width(65), 777.0);
    for (missing, width) in [
        (Object::from(true), 1.0),
        (Object::text("a"), 0.0),
        (Object::from(-500), 65036.0),
        (Object::from(65535), 0.0),
    ] {
        let descriptor = pdf.add_object(dict! { "MissingWidth" => missing.clone() });
        let font = dict! {
            "Subtype" => "TrueType", "BaseFont" => "Foo", "FirstChar" => 65,
            "FontDescriptor" => descriptor, "Widths" => vec![Object::from(500)],
        };
        assert_eq!(font_of(&pdf, font).width(66), width, "{missing:?}");
    }

    // A fixed-pitch standard font read as Type 1 starts at its pitch; as TrueType, or
    // past a subset's tag, it doesn't.
    let courier = |subtype: &str, name: &str| {
        let font = dict! {
            "Subtype" => subtype.to_string(), "BaseFont" => name.to_string(),
            "FirstChar" => 65, "Widths" => vec![Object::from(500)],
        };
        font_of(&pdf, font).width(66)
    };
    assert_eq!(courier("Type1", "Courier"), 600.0);
    assert_eq!(courier("MMType1", "Courier"), 600.0);
    assert_eq!(courier("Type1", "CourierNew"), 600.0);
    assert_eq!(courier("TrueType", "Courier"), 0.0);
    assert_eq!(courier("Type1", "ABCDEF+Courier"), 0.0);
    assert_eq!(courier("Type1", "Helvetica"), 0.0);
}

/// A descriptor's `/Flags` of capitals only: a lower-case code takes its capital's
/// width, where that isn't 0, as PDFium draws its glyph.
#[test]
fn all_caps_lower_case_takes_its_capital_s_width() {
    let mut pdf = Pdf::default();
    let flagged = |pdf: &mut Pdf, flags: Object| pdf.add_object(dict! { "Flags" => flags });
    let all_caps = flagged(&mut pdf, (1 << 16).into());
    let fill: Vec<Object> = (0..34).map(|i| Object::from(300 + i)).collect();
    let font = |descriptor: ObjectId, widths: Vec<Object>| {
        dict! {
            "Subtype" => "TrueType", "BaseFont" => "Foo", "FirstChar" => 65,
            "FontDescriptor" => descriptor, "Widths" => widths,
        }
    };
    let read = font_of(&pdf, font(all_caps, fill.clone()));
    assert_eq!(read.width(u32::from(b'a')), 300.0);
    assert_eq!(read.width(u32::from(b'A')), 300.0);
    let mut zero = fill.clone();
    zero[0] = Object::from(0);
    let read = font_of(&pdf, font(all_caps, zero));
    assert_eq!(read.width(u32::from(b'a')), 332.0);
    let plain = flagged(&mut pdf, 32.into());
    let read = font_of(&pdf, font(plain, fill.clone()));
    assert_eq!(read.width(u32::from(b'a')), 332.0);
    // Those of its other ranges; not those between them.
    let read = dict! {
        "Subtype" => "TrueType", "BaseFont" => "Foo", "FirstChar" => 192,
        "FontDescriptor" => all_caps, "Widths" => vec![Object::from(500)],
    };
    let read = font_of(&pdf, read);
    assert_eq!(
        [0xe0, 0xc0, 0xf7, 0xd7].map(|c| read.width(c)),
        [500.0, 500.0, 0.0, 0.0]
    );
    // With no `/Widths`: its capital's, in a standard font, with its box.
    let helvetica = dict! {
        "Subtype" => "Type1", "BaseFont" => "Helvetica", "FontDescriptor" => all_caps,
    };
    let read = font_of(&pdf, helvetica);
    assert_eq!(read.width(u32::from(b'a')), 667.0);
    assert_eq!(
        read.glyph_box(u32::from(b'a')),
        read.glyph_box(u32::from(b'A'))
    );
    // One that embeds its program draws what that has.
    let program = pdf.add_object(Stream::new(dict! {}, crate::fixture::truetype_program()));
    let embeds = pdf.add_object(dict! { "Flags" => 1 << 16, "FontFile2" => program });
    let read = font_of(&pdf, font(embeds, fill));
    assert_eq!(read.width(u32::from(b'a')), 332.0);
    // A program's key that's no stream isn't one embedded.
    let no_program = pdf.add_object(dict! { "FontFile2" => Object::reference(999) });
    let helvetica = dict! {
        "Subtype" => "Type1", "BaseFont" => "Helvetica", "FontDescriptor" => no_program,
    };
    let read = font_of(&pdf, helvetica);
    assert!(!read.approx().widths && read.glyph_box(u32::from(b'A')).is_some());
    // By a chain, `/Flags` is 0.
    let number = pdf.add_object(Object::from(1 << 16));
    let chain = pdf.add_object(Object::from(number));
    let chained = flagged(&mut pdf, chain.into());
    let read = font_of(&pdf, font(chained, (0..34).map(Object::from).collect()));
    assert_eq!(read.width(u32::from(b'a')), 32.0);
}

/// A descriptor's numbers and box by PDFium's accessors, and what they come to.
#[test]
fn a_descriptor_s_numbers_as_pdfium_reads_them() {
    let mut pdf = Pdf::default();
    let number = pdf.add_object(Object::from(800));
    let chain = pdf.add_object(Object::from(number));
    let boxed = |ns: [i32; 4]| -> Object {
        ns.iter()
            .map(|&n| Object::from(n))
            .collect::<Vec<_>>()
            .into()
    };
    let read = |pdf: &mut Pdf, bbox: Object, ascent: Object, descent: Object| {
        let descriptor = pdf.add_object(dict! {
            "FontBBox" => bbox, "Ascent" => ascent, "Descent" => descent,
        });
        let font = dict! {
            "Subtype" => "TrueType", "BaseFont" => "Foo", "FontDescriptor" => descriptor,
        };
        let metrics = font_of(pdf, font).metrics();
        (metrics.ascent, metrics.descent)
    };
    let tall = boxed([0, -300, 1000, 900]);
    let heights = |pdf: &mut Pdf, a: Object, d: Object| read(pdf, tall.clone(), a, d);
    assert_eq!(
        heights(&mut pdf, 800.9.into(), (-200.9).into()),
        (800.0, -200.0)
    );
    assert_eq!(heights(&mut pdf, true.into(), true.into()), (1.0, 1.0));
    assert_eq!(heights(&mut pdf, true.into(), (-200).into()), (1.0, -200.0));
    assert_eq!(
        heights(&mut pdf, Object::text("a"), (-200).into()),
        (0.0, -200.0)
    );
    assert_eq!(
        heights(&mut pdf, number.into(), (-200).into()),
        (800.0, -200.0)
    );
    assert_eq!(
        heights(&mut pdf, chain.into(), (-200).into()),
        (0.0, -200.0)
    );
    assert_eq!(
        heights(&mut pdf, 4294967296_i64.into(), (-200).into()),
        (0.0, -200.0)
    );
    // A `/Descent` over 10 is below the baseline.
    assert_eq!(heights(&mut pdf, 800.into(), 200.into()), (800.0, -200.0));
    assert_eq!(heights(&mut pdf, 800.into(), 11.into()), (800.0, -11.0));
    assert_eq!(heights(&mut pdf, 800.into(), 10.into()), (800.0, 10.0));
    assert_eq!(
        heights(&mut pdf, 800.into(), 2147483648_i64.into()),
        (800.0, -300.0)
    );
    // They're held within the box where its top is above its bottom, and not else.
    let small = boxed([0, -100, 1000, 500]);
    assert_eq!(
        read(&mut pdf, small, 800.into(), (-200).into()),
        (500.0, -100.0)
    );
    let flipped = boxed([0, 900, 1000, -300]);
    assert_eq!(
        read(&mut pdf, flipped, 1000.into(), (-500).into()),
        (1000.0, -500.0)
    );
    let flat = boxed([5, 100, 6, 100]);
    assert_eq!(
        read(&mut pdf, flat, 800.into(), (-200).into()),
        (800.0, -200.0)
    );
    // A box of what's no number has it as 0, and past its end.
    let short: Object = vec![Object::from(0), Object::from(-300), Object::from(1000)].into();
    assert_eq!(
        read(&mut pdf, short, 800.into(), (-200).into()),
        (0.0, -200.0)
    );
    let yes: Object = vec![0.into(), (-300).into(), 1000.into(), true.into()].into();
    assert_eq!(
        read(&mut pdf, yes, 800.into(), (-200).into()),
        (1.0, -200.0)
    );
    let by: Object = vec![0.into(), (-100).into(), 1000.into(), chain.into()].into();
    assert_eq!(read(&mut pdf, by, 800.into(), (-200).into()), (0.0, -100.0));
    // By a reference it's read, by a chain none is, and so they're guessed.
    let reference = pdf.add_object(boxed([0, -100, 1000, 500]));
    assert_eq!(
        read(&mut pdf, reference.into(), 800.into(), (-200).into()),
        (500.0, -100.0)
    );
    let once = pdf.add_object(Object::from(reference));
    assert_eq!(
        read(&mut pdf, once.into(), 800.into(), (-200).into()),
        (800.0, -200.0)
    );
    let descriptor = pdf.add_object(dict! {
        "FontBBox" => once, "Ascent" => 800, "Descent" => -200,
    });
    let font =
        dict! { "Subtype" => "TrueType", "BaseFont" => "Foo", "FontDescriptor" => descriptor };
    assert!(font_of(&pdf, font).approx().heights);
}

/// Each accessor's references: `/FontDescriptor` two, its value's and one more; `/Widths`
/// one; `/CharProcs`, a dictionary, two; a CIDFont, an array's element, one.
#[test]
fn references_are_followed_as_each_accessor_follows_them() {
    let mut pdf = Pdf::default();
    let descriptor = pdf.add_object(dict! { "MissingWidth" => 777 });
    let refer = |pdf: &mut Pdf, id: ObjectId| pdf.add_object(Object::from(id));
    let once = refer(&mut pdf, descriptor);
    let twice = refer(&mut pdf, once);
    let simple = |pdf: &Pdf, descriptor: ObjectId| {
        let font = dict! {
            "Subtype" => "TrueType", "BaseFont" => "Foo", "FontDescriptor" => descriptor,
            "FirstChar" => 65, "Widths" => vec![Object::from(500)],
        };
        font_of(pdf, font).width(66)
    };
    assert_eq!(simple(&pdf, descriptor), 777.0);
    assert_eq!(simple(&pdf, once), 777.0);
    assert_eq!(simple(&pdf, twice), 0.0);

    let widths = pdf.add_object(Object::from(vec![Object::from(710), Object::from(720)]));
    let chain = refer(&mut pdf, widths);
    let helvetica = |pdf: &Pdf, widths: ObjectId| {
        let font = dict! {
            "Subtype" => "Type1", "BaseFont" => "Helvetica", "FirstChar" => 65, "Widths" => widths,
        };
        font_of(pdf, font).width(65)
    };
    assert_eq!(helvetica(&pdf, widths), 710.0);
    assert_eq!(helvetica(&pdf, chain), 667.0);

    let cid_font = pdf.add_object(dict! { "DW" => 500 });
    let once = refer(&mut pdf, cid_font);
    let fonts = pdf.add_object(Object::from(vec![Object::from(cid_font)]));
    let fonts_once = refer(&mut pdf, fonts);
    let cid_width = |pdf: &Pdf, fonts: Object| {
        let font = dict! {
            "Subtype" => "Type0", "Encoding" => "Identity-H", "DescendantFonts" => fonts,
        };
        font_of(pdf, font).width(1)
    };
    assert_eq!(cid_width(&pdf, vec![Object::from(cid_font)].into()), 500.0);
    assert_eq!(cid_width(&pdf, vec![Object::from(once)].into()), 1000.0);
    assert_eq!(cid_width(&pdf, fonts.into()), 500.0);
    assert_eq!(cid_width(&pdf, fonts_once.into()), 1000.0);

    // What's told of a reference to a reference where it's read as none.
    let font = dict! {
        "Subtype" => "Type1", "BaseFont" => "Helvetica", "FirstChar" => 65, "Widths" => chain,
    };
    let told = told(&pdf, &font);
    assert_eq!(told, [(Place::new(Some(widths), None), Fault::Chained)]);

    // A `/BaseFont` and a `/Subtype` as PDFium's byte string accessor reads them: a
    // string as a name, through one reference.
    let width_at_66 = |pdf: &Pdf, font: Dict| font_of(pdf, font).width(66);
    let by = |subtype: Object, base: Object| {
        dict! {
            "Subtype" => subtype, "BaseFont" => base,
            "FirstChar" => 65, "Widths" => vec![Object::from(500)],
        }
    };
    let courier = pdf.add_object(Object::from("Courier"));
    let chained = refer(&mut pdf, courier);
    assert_eq!(
        width_at_66(&pdf, by("Type1".into(), Object::text("Courier"))),
        600.0
    );
    assert_eq!(width_at_66(&pdf, by("Type1".into(), courier.into())), 600.0);
    assert_eq!(width_at_66(&pdf, by("Type1".into(), chained.into())), 0.0);
    assert_eq!(
        width_at_66(&pdf, by(Object::text("TrueType"), "Courier".into())),
        0.0
    );
    let true_type = pdf.add_object(Object::from("TrueType"));
    let chained = refer(&mut pdf, true_type);
    assert_eq!(
        width_at_66(&pdf, by(true_type.into(), "Courier".into())),
        0.0
    );
    // By a chain it's no `/Subtype`, which reads as Type 1.
    assert_eq!(
        width_at_66(&pdf, by(chained.into(), "Courier".into())),
        600.0
    );
}

/// `/Differences` as PDFium reads it, the one reader of it: each value after one
/// reference, a name at the code and the code on one, anything else the code by the
/// integer accessor, a reference to nothing passed over.
#[test]
fn differences_as_pdfium_reads_them() {
    let mut pdf = Pdf::default();
    let number = pdf.add_object(Object::from(66));
    let number_chain = pdf.add_object(Object::from(number));
    let name = pdf.add_object(Object::from("C"));
    let name_chain = pdf.add_object(Object::from(name));
    let text = |pdf: &Pdf, differences: Object, code: u32| {
        let font = dict! {
            "Subtype" => "Type1", "BaseFont" => "Helvetica",
            "Encoding" => dict! { "Differences" => differences },
        };
        font_of(pdf, font).code_text(code).map(|t| t.into_owned())
    };
    let c = || Some("C".to_string());
    assert_eq!(text(&pdf, vec![65.into(), "C".into()].into(), 65), c());
    // A real is cut; `true` is 1; what's no number is 0, a string, `null`, an array.
    assert_eq!(text(&pdf, vec![65.9.into(), "C".into()].into(), 65), c());
    assert_eq!(text(&pdf, vec![true.into(), "C".into()].into(), 1), c());
    for other in [
        Object::text("s"),
        Object::null(),
        vec![Object::from(1)].into(),
    ] {
        let list = vec![66.into(), other.clone(), "C".into()];
        assert_eq!(text(&pdf, list.into(), 0), c(), "{other:?}");
        let list = vec![66.into(), other.clone(), "C".into()];
        assert_eq!(
            text(&pdf, list.into(), 66).as_deref(),
            Some("B"),
            "{other:?}"
        );
    }
    // A reference once; a chain to a number by the integer accessor, to a name 0.
    assert_eq!(text(&pdf, vec![number.into(), "C".into()].into(), 66), c());
    assert_eq!(text(&pdf, vec![65.into(), name.into()].into(), 65), c());
    assert_eq!(
        text(&pdf, vec![number_chain.into(), "C".into()].into(), 66),
        c()
    );
    let list = vec![66.into(), name_chain.into(), "C".into()];
    assert_eq!(text(&pdf, list.into(), 0), c());
    // A reference to nothing changes nothing.
    let list = vec![66.into(), Object::reference(999), "C".into()];
    assert_eq!(text(&pdf, list.into(), 66), c());
    // Past 255, none; a negative comes back round to 0; past 32 bits is 0.
    let list = vec![255.into(), "B".into(), "C".into()];
    assert_eq!(text(&pdf, list.clone().into(), 0).as_deref(), None);
    assert_eq!(text(&pdf, list.into(), 255).as_deref(), Some("B"));
    let list = vec![(-1).into(), "B".into(), "C".into()];
    assert_eq!(text(&pdf, list.into(), 0), c());
    let list = vec![4294967361_i64.into(), "C".into()];
    assert_eq!(text(&pdf, list.into(), 0), c());
    // An empty name leaves the code to its base encoding, and moves on.
    let list = vec![65.into(), "".into(), "C".into()];
    assert_eq!(text(&pdf, list.clone().into(), 65).as_deref(), Some("A"));
    assert_eq!(text(&pdf, list.into(), 66), c());
    let list = vec![65.into(), "C".into(), 65.into(), "".into()];
    assert_eq!(text(&pdf, list.into(), 65).as_deref(), Some("A"));
    // By a reference it's read, by a chain none is, and it's the array's own that's
    // told of what's in it.
    let list = pdf.add_object(Object::from(vec![65.into(), "C".into()]));
    let once = pdf.add_object(Object::from(list));
    assert_eq!(text(&pdf, list.into(), 65), c());
    assert_eq!(text(&pdf, once.into(), 65).as_deref(), Some("A"));
    // `.notdef` is the space in what's read as Type 1 of no symbols.
    let notdef = vec![65.into(), ".notdef".into()];
    assert_eq!(text(&pdf, notdef.into(), 65).as_deref(), Some(" "));
}

/// A TrueType font named Symbol with no program, not said to be of no symbols, reads by
/// PDFium's table of codes, whose Greek letters aren't the glyph list's.
#[test]
fn a_symbol_font_with_no_program_reads_as_pdfium_s_table_in_truetype() {
    let mut pdf = Pdf::default();
    let descriptor = |flags: i64| dict! { "Flags" => flags };
    let text = |pdf: &Pdf, subtype: &str, flags: i64, encoding: Option<&str>, code: u32| {
        let mut font = dict! {
            "Subtype" => subtype, "BaseFont" => "Symbol", "FontDescriptor" => descriptor(flags),
        };
        if let Some(encoding) = encoding {
            font.set("Encoding", encoding);
        }
        font_of(pdf, font).code_text(code).map(|t| t.into_owned())
    };
    let is = |text: &str| Some(text.to_string());
    for (code, greek, listed) in [
        (0x44, "\u{394}", "\u{2206}"),
        (0x57, "\u{3a9}", "\u{2126}"),
        (0x6d, "\u{3bc}", "\u{b5}"),
    ] {
        assert_eq!(text(&pdf, "TrueType", 4, None, code), is(greek));
        assert_eq!(text(&pdf, "TrueType", 0, None, code), is(greek));
        let no_descriptor = dict! { "Subtype" => "TrueType", "BaseFont" => "Symbol" };
        let read = font_of(&pdf, no_descriptor);
        assert_eq!(read.code_text(code).as_deref(), Some(greek));
        assert_eq!(
            text(&pdf, "TrueType", 4, Some("WinAnsiEncoding"), code),
            is(greek)
        );
        // By names, in Type 1, or where it says it's of no symbols.
        assert_eq!(text(&pdf, "Type1", 4, None, code), is(listed));
        assert_eq!(text(&pdf, "TrueType", 32, None, code), is(listed));
    }
    // Where the two agree, as Alpha.
    assert_eq!(text(&pdf, "TrueType", 4, None, 0x41), is("\u{391}"));
    // With a program, its own charmap decides, not this table: its code itself, where
    // that's a Unicode one; read for text, a guess by name.
    let file = pdf.add_object(Stream::new(dict! {}, crate::fixture::truetype_program()));
    let font = Object::from(dict! {
        "Subtype" => "TrueType", "BaseFont" => "Symbol",
        "FontDescriptor" => dict! { "Flags" => 4, "FontFile2" => file },
    });
    let read = |fonts: Fonts| {
        let mut budget = BUDGET;
        let lim = &Limits::DEFAULT;
        let read = fonts.get(&pdf, &font, lim, &mut budget).unwrap().unwrap();
        read.code_text(0x44).map(|text| text.into_owned())
    };
    let own = if cfg!(feature = "programs") {
        "D"
    } else {
        "\u{2206}"
    };
    assert_eq!(read(Fonts::default()).as_deref(), Some(own));
    assert_eq!(read(Fonts::for_text()).as_deref(), Some("\u{2206}"));
}

/// A simple font's base encoding, as PDFium comes to it, by its `/Flags` and `/Encoding`.
#[test]
fn base_encodings_as_pdfium_comes_to_them() {
    let mut pdf = Pdf::default();
    let name = pdf.add_object(Object::from("WinAnsiEncoding"));
    let chain = pdf.add_object(Object::from(name));
    let quote = |pdf: &Pdf, font: Dict| font_of(pdf, font).code_text(0x27).map(|t| t.into_owned());
    let quoted = |text: &str| Some(text.to_string());
    let (standard, win_ansi) = (quoted("’"), quoted("'"));
    let with = |encoding: Object| {
        dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => encoding }
    };
    assert_eq!(quote(&pdf, with("WinAnsiEncoding".into())), win_ansi);
    assert_eq!(quote(&pdf, with("MacRomanEncoding".into())), win_ansi);
    assert_eq!(quote(&pdf, with("StandardEncoding".into())), standard);
    // By name, MacExpertEncoding is read as WinAnsiEncoding, as PDFium has it.
    assert_eq!(quote(&pdf, with("MacExpertEncoding".into())), win_ansi);
    assert_eq!(quote(&pdf, with("Nope".into())), standard);
    assert_eq!(quote(&pdf, with(name.into())), win_ansi);
    assert_eq!(quote(&pdf, with(chain.into())), standard);
    assert_eq!(quote(&pdf, with(Object::text("WinAnsiEncoding"))), standard);
    let base = |value: Object| with(dict! { "BaseEncoding" => value }.into());
    assert_eq!(quote(&pdf, base("WinAnsiEncoding".into())), win_ansi);
    assert_eq!(quote(&pdf, base(Object::text("WinAnsiEncoding"))), win_ansi);
    assert_eq!(quote(&pdf, base(name.into())), win_ansi);
    assert_eq!(quote(&pdf, base(chain.into())), standard);
    assert_eq!(quote(&pdf, base("MacExpertEncoding".into())), standard);
    // Symbol and ZapfDingbats keep their own whatever's named.
    let symbol = dict! {
        "Subtype" => "Type1", "BaseFont" => "Symbol", "Encoding" => "WinAnsiEncoding",
    };
    assert_eq!(font_of(&pdf, symbol).code_text(0x61).as_deref(), Some("α"));
    // A TrueType font is read in StandardEncoding but where its `/Flags` say it's
    // symbolic, and with no `/Encoding`, WinAnsiEncoding then.
    let true_type = dict! { "Subtype" => "TrueType", "BaseFont" => "Foo" };
    assert_eq!(quote(&pdf, true_type.clone()), standard);
    let symbolic = pdf.add_object(dict! { "Flags" => 4 });
    let mut flagged = true_type.clone();
    flagged.set("FontDescriptor", symbolic);
    assert_eq!(quote(&pdf, flagged), win_ansi);
    // Named Symbol, a TrueType font is drawn in Adobe's Symbol only where its `/Flags`
    // say it's symbolic, and else its widths are a face's, guessed.
    let named_symbol = dict! { "Subtype" => "TrueType", "BaseFont" => "Symbol" };
    let read = font_of(&pdf, named_symbol.clone());
    assert!(read.approx().widths);
    assert_eq!(read.width(u32::from(b'a')), 0.0);
    let mut flagged = named_symbol;
    flagged.set("FontDescriptor", symbolic);
    let read = font_of(&pdf, flagged);
    assert!(!read.approx().widths);
    assert_eq!(read.width(u32::from(b'a')), 631.0);
    // In a Type 3 font's dictionary with no base it's StandardEncoding.
    let type3 = |encoding: Option<Object>| {
        let mut font = dict! { "Subtype" => "Type3" };
        if let Some(encoding) = encoding {
            font.set("Encoding", encoding);
        }
        font
    };
    assert_eq!(quote(&pdf, type3(Some(dict! {}.into()))), standard);
    assert_eq!(quote(&pdf, type3(None)), None);
}

/// A Type 3 font's numbers, as PDFium's `Load` reads them.
#[test]
fn type3_numbers_as_pdfium_reads_them() {
    let mut pdf = Pdf::default();
    let proc = pdf.add_object(Stream::new(dict! {}, b"410 0 d0".to_vec()));
    let number = pdf.add_object(Object::from(500));
    let chain = pdf.add_object(Object::from(number));
    let matrix: Object = [0.001, 0.0, 0.0, 0.001, 0.0, 0.0]
        .into_iter()
        .map(Object::from)
        .collect::<Vec<_>>()
        .into();
    let font = |matrix: Option<Object>, first: Object, widths: Object| {
        let mut font = dict! {
            "Subtype" => "Type3", "FirstChar" => first, "Widths" => widths,
            "CharProcs" => dict! { "A" => proc },
            "Encoding" => dict! { "Differences" => vec![Object::from(65), Object::from("A")] },
        };
        if let Some(matrix) = matrix {
            font.set("FontMatrix", matrix);
        }
        font
    };
    let width = |pdf: &Pdf, matrix: Option<Object>, first: Object, widths: Object| {
        font_of(pdf, font(matrix, first, widths)).width(65)
    };
    let one = |w: Object| -> Object { vec![w].into() };
    let at_65 = |w: Object| width(&pdf, Some(matrix.clone()), 65.into(), one(w));
    // By the float accessor, rounded: `true` is 0, and so its CharProc's.
    assert_eq!(at_65(500.6.into()), 501.0);
    assert_eq!(at_65(500.5.into()), 501.0);
    assert_eq!(at_65(true.into()), 410.0);
    assert_eq!(at_65(Object::text("a")), 410.0);
    assert_eq!(at_65(number.into()), 500.0);
    assert_eq!(at_65(chain.into()), 410.0);
    assert_eq!(at_65(4294967296_i64.into()), 410.0);
    assert_eq!(at_65((-500).into()), -500.0);
    // `/FirstChar` by the integer accessor, `true` 1, so 65 has no `/Widths` entry.
    let at = |first: Object| width(&pdf, Some(matrix.clone()), first, one(500.into()));
    assert_eq!(at(65.9.into()), 500.0);
    assert_eq!(at(true.into()), 410.0);
    assert_eq!(at((-1).into()), 410.0);
    assert_eq!(at(300.into()), 410.0);
    // A matrix of six, else the identity, each by the float accessor.
    let of = |matrix: Vec<Object>| width(&pdf, Some(matrix.into()), 65.into(), one(500.into()));
    let n = |ns: &[f64]| ns.iter().map(|&n| Object::from(n)).collect::<Vec<_>>();
    assert_eq!(of(n(&[0.002, 0.0, 0.0, 0.001, 0.0, 0.0])), 1000.0);
    assert_eq!(of(n(&[0.002, 0.0, 0.0, 0.001, 0.0])), 500_000.0);
    assert_eq!(of(n(&[0.002, 0.0, 0.0, 0.001, 0.0, 0.0, 0.0])), 500_000.0);
    assert_eq!(width(&pdf, None, 65.into(), one(500.into())), 500_000.0);
    let mut yes = n(&[0.002, 0.0, 0.0, 0.001, 0.0, 0.0]);
    yes[0] = Object::from(true);
    // `a` 0 leaves no `/Widths` entry, and the CharProc's width through `b`.
    yes[1] = Object::from(0.001);
    assert_eq!(of(yes), 410.0);
    // A CharProc's width through the matrix's length along x.
    let skew = n(&[0.0006, 0.0008, 0.0, 0.001, 0.0, 0.0]);
    let read = width(
        &pdf,
        Some(skew.into()),
        65.into(),
        Vec::<Object>::new().into(),
    );
    assert_eq!(read, 410.0);

    // A box by the float accessor, past its end 0: cut, not rounded.
    let boxed = |pdf: &mut Pdf, bbox: Object| {
        let mut font = font(Some(matrix.clone()), 65.into(), one(500.into()));
        font.set("FontBBox", bbox);
        let _ = &pdf;
        let font = font_of(pdf, font);
        (font.metrics().ascent, font.metrics().descent)
    };
    let sides = |ns: &[f64]| -> Object { n(ns).into() };
    // `A` here has no `d1`, so its ascent's the box's.
    assert_eq!(
        boxed(&mut pdf, sides(&[0.0, -200.9, 1000.0, 800.9])),
        (800.0, -200.0)
    );
    assert_eq!(
        boxed(&mut pdf, sides(&[0.0, -200.0, 1000.0])),
        (0.0, -200.0)
    );
    let yes: Object = vec![0.into(), (-200).into(), 1000.into(), true.into()].into();
    assert_eq!(boxed(&mut pdf, yes), (0.0, -200.0));
}

/// A CharProc's numbers, as PDFium's parser takes them: wherever its `d0` or `d1` is,
/// the last of them, and what's no number or isn't there 0.
#[test]
fn a_charproc_s_numbers_as_pdfium_takes_them() {
    let mut pdf = Pdf::default();
    let matrix: Object = [0.001, 0.0, 0.0, 0.001, 0.0, 0.0]
        .into_iter()
        .map(Object::from)
        .collect::<Vec<_>>()
        .into();
    let mut read = |content: &[u8]| {
        let proc = pdf.add_object(Stream::new(dict! {}, content.to_vec()));
        let font = dict! {
            "Subtype" => "Type3", "FontMatrix" => matrix.clone(),
            "CharProcs" => dict! { "A" => proc },
            "Encoding" => dict! { "Differences" => vec![Object::from(65), Object::from("A")] },
        };
        let font = font_of(&pdf, font);
        (font.width(65), font.glyph_box(65))
    };
    let no_box = None;
    assert_eq!(read(b"410.6 0 d0"), (411.0, no_box));
    assert_eq!(read(b"/x 0 d0"), (0.0, no_box));
    assert_eq!(read(b"(x) 0 d0"), (0.0, no_box));
    assert_eq!(read(b"410 d0"), (0.0, no_box));
    assert_eq!(read(b"d0"), (0.0, no_box));
    assert_eq!(read(b"7 8 410 0 d0"), (410.0, no_box));
    assert_eq!(read(b"q 410 0 d0 Q"), (410.0, no_box));
    assert_eq!(read(b"100 0 d0 410 0 d0"), (410.0, no_box));
    assert_eq!(read(b"4294967296 0 d0"), (0.0, no_box));
    assert_eq!(read(b"q Q"), (0.0, no_box));
    assert_eq!(read(b""), (0.0, no_box));
    let boxed = Some(rect(0.0, 0.0, 100.0, 100.0));
    assert_eq!(read(b"410 0 0 0 100 100 d1"), (410.0, boxed));
    assert_eq!(read(b"7 410 0 0 0 100 100 d1"), (410.0, boxed));
    assert_eq!(read(b"100 0 d0 410 0 0 0 100 100 d1"), (410.0, boxed));
    // A `d0` after a `d1` leaves its box.
    assert_eq!(read(b"100 0 0 0 100 100 d1 410 0 d0"), (410.0, boxed));
    assert_eq!(read(b"/x 0 0 0 100 100 d1"), (0.0, boxed));
    // Short by one, each number's a place along, the width 0.
    assert_eq!(
        read(b"410 0 0 100 100 d1"),
        (0.0, Some(rect(0.0, 0.0, 100.0, 100.0)))
    );
}

/// A vertical font's numbers, in the 16 bits PDFium holds each in.
#[test]
fn a_vertical_font_s_numbers_are_16_bits() {
    let mut pdf = Pdf::default();
    let ns = |ns: &[i64]| -> Object {
        ns.iter()
            .map(|&n| Object::from(n))
            .collect::<Vec<_>>()
            .into()
    };
    let vertical = |pdf: &mut Pdf, keys: Dict| {
        let font = type0(pdf, "Identity-V".into(), keys);
        font_of(pdf, font).vertical(65).unwrap()
    };
    let places = |v: VerticalMetrics| (v.w1, v.vx, v.vy);
    let plain = vertical(&mut pdf, dict! {});
    assert_eq!(places(plain), (-1000.0, 500.0, 880.0));
    // `/DW2` of one number has the second 0, and of none both, where an absent one is
    // PDFium's defaults.
    let read = vertical(&mut pdf, dict! { "DW2" => ns(&[700]) });
    assert_eq!(places(read), (0.0, 500.0, 700.0));
    let read = vertical(&mut pdf, dict! { "DW2" => ns(&[]) });
    assert_eq!(places(read), (0.0, 500.0, 0.0));
    let read = vertical(&mut pdf, dict! { "DW2" => ns(&[70000, -70000]) });
    assert_eq!(places(read), (-4464.0, 500.0, 4464.0));
    let dw2: Object = vec![Object::from(true), Object::text("a")].into();
    let read = vertical(&mut pdf, dict! { "DW2" => dw2 });
    assert_eq!(places(read), (0.0, 500.0, 1.0));
    // Its centre is half its width, cut, and in 16 bits.
    let w = |pdf: &mut Pdf, n: i64| {
        let w: Object = vec![Object::from(65), ns(&[n])].into();
        vertical(pdf, dict! { "W" => w }).vx
    };
    assert_eq!(w(&mut pdf, 501), 250.0);
    assert_eq!(w(&mut pdf, -501), -250.0);
    assert_eq!(w(&mut pdf, 70000), -30536.0);
    // `/W2`'s three.
    let w2: Object = vec![Object::from(65), ns(&[40000, 70000, -40000])].into();
    let read = vertical(&mut pdf, dict! { "W2" => w2 });
    assert_eq!(places(read), (-25536.0, 4464.0, 25536.0));
    // An array short of three has 0 past its end; a range short of them is none.
    let w2: Object = vec![Object::from(65), ns(&[-800, 250])].into();
    let read = vertical(&mut pdf, dict! { "W2" => w2 });
    assert_eq!(places(read), (-800.0, 250.0, 0.0));
    let read = vertical(&mut pdf, dict! { "W2" => ns(&[65, 66, -800, 250]) });
    assert_eq!(places(read), (-1000.0, 500.0, 880.0));
}

/// What PDFium doesn't load is told: a `/DescendantFonts` it can't read one CIDFont
/// from, an `/Encoding` that's neither a name nor a stream, each by its accessor.
#[test]
fn a_font_pdfium_doesn_t_load_is_told() {
    let mut pdf = Pdf::default();
    let cid_font = pdf.add_object(dict! { "Subtype" => "CIDFontType2" });
    let once = pdf.add_object(Object::from(cid_font));
    let list = pdf.add_object(Object::from(vec![Object::from(cid_font)]));
    let list_once = pdf.add_object(Object::from(list));
    let name = pdf.add_object(Object::from("Identity-H"));
    let name_once = pdf.add_object(Object::from(name));
    let font = |fonts: Object, encoding: Object| {
        dict! { "Subtype" => "Type0", "DescendantFonts" => fonts, "Encoding" => encoding }
    };
    let good = || vec![Object::from(cid_font)].into();
    let faults = |pdf: &Pdf, font: Dict| -> Vec<Fault> {
        told(pdf, &font).into_iter().map(|(_, f)| f).collect()
    };
    assert_eq!(faults(&pdf, font(good(), "Identity-H".into())), []);
    assert_eq!(faults(&pdf, font(list.into(), "Identity-H".into())), []);
    assert_eq!(faults(&pdf, font(good(), name.into())), []);
    let elem: Object = vec![Object::from(once)].into();
    assert_eq!(
        faults(&pdf, font(elem, "Identity-H".into())),
        [Fault::DescendantFonts]
    );
    assert_eq!(
        faults(&pdf, font(list_once.into(), "Identity-H".into())),
        [Fault::Chained, Fault::DescendantFonts]
    );
    assert_eq!(
        faults(&pdf, font(good(), Object::text("Identity-H"))),
        [Fault::CMap]
    );
    assert_eq!(faults(&pdf, font(good(), name_once.into())), [Fault::CMap]);
    let no_encoding =
        dict! { "Subtype" => "Type0", "DescendantFonts" => vec![Object::from(cid_font)] };
    assert_eq!(faults(&pdf, no_encoding), [Fault::CMap]);
}

/// A TrueType font PDFium reads as GBK: by the first four bytes of its name, unless it
/// has a TrueType program by its key; its codes by GBK-EUC-H and its widths fixed.
#[cfg(feature = "predefined-cmaps")]
#[test]
fn a_truetype_font_read_as_gbk() {
    let mut pdf = Pdf::default();
    let named = |subtype: &str, descriptor: Option<ObjectId>| {
        let mut font = dict! {
            "Subtype" => subtype.to_string(), "BaseFont" => Object::new(pdf_objects::object::Value::Name(b"\xCB\xCE\xCC\xE5".to_vec())),
            "FirstChar" => 65, "Widths" => vec![Object::from(510), Object::from(610)],
        };
        if let Some(descriptor) = descriptor {
            font.set("FontDescriptor", descriptor);
        }
        font
    };
    let font = font_of(&pdf, named("TrueType", None));
    assert_eq!(font.kind(), FontKind::TrueType);
    assert_eq!(codes(&font, b"A\xB0\xA1"), [(0..1, 0x41), (1..3, 0xb0a1)]);
    assert_eq!(font.width(0x41), 500.0);
    assert_eq!(font.width(0x1f), 0.0);
    assert_eq!(font.width(0xb0a1), 1000.0);
    assert_eq!(font.code_text(0xb0a1).as_deref(), Some("啊"));
    assert!(font.byte_widths().is_none());
    assert!(!font.approx().codes);
    let descriptor = pdf.add_object(dict! { "Flags" => 32 });
    assert_eq!(
        font_of(&pdf, named("TrueType", Some(descriptor))).width(0x41),
        500.0
    );
    let program = pdf.add_object(dict! { "Flags" => 32, "FontFile2" => Object::reference(999) });
    assert_eq!(
        font_of(&pdf, named("TrueType", Some(program))).width(0x41),
        510.0
    );
    assert_eq!(font_of(&pdf, named("Type1", None)).width(0x41), 510.0);
}

/// A predefined CMap's codes below the space have CID 0, as PDFium's tables have no
/// `.notdef` ranges.
#[cfg(feature = "predefined-cmaps")]
#[test]
fn predefined_codes_below_the_space_have_cid_0() {
    let mut pdf = Pdf::default();
    let font = type0(&mut pdf, "GBK-EUC-H".into(), dict! {});
    let font = font_of(&pdf, font);
    assert_eq!(font.cid(1), Some(0));
    assert_ne!(font.cid(0x41), Some(0));
}

/// A resolver that counts what it's asked.
#[derive(Clone, Copy)]
struct Counting<'c, 'o> {
    pdf: &'o Pdf,
    asked: &'c Cell<usize>,
}

impl<'o> Resolve<'o> for Counting<'_, 'o> {
    fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
        self.asked.set(self.asked.get() + 1);
        self.pdf.objects.get(&id)
    }
}

impl Streams for Counting<'_, '_> {
    fn decoded(&self, id: ObjectId, limit: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
        self.pdf.decoded_by(id, limit)
    }
}

/// A sink that says it wants none, and fails if it's told any.
struct Unwanting;

impl Sink<Fault> for Unwanting {
    fn fault(&mut self, _: Place, fault: Fault) {
        panic!("told {fault:?}");
    }

    fn wants(&self) -> bool {
        false
    }
}

/// Reading a font asks for the objects it reaches the same number of times whatever
/// the sink, `()` as few as what's read needs: telling costs no look-up.
#[test]
fn a_sink_costs_no_lookups() {
    let (pdf, fonts) = fonts_that_reach();
    // The simple font: each of what it reaches once, its `/Encoding` by its decoder
    // alone. The Type 3 font's: its `/CharProcs` and its one CharProc. Identity's:
    // each once. The embedded CMap: `/Encoding` by its decoder and by its codes, read,
    // and the one it uses.
    let expected = [6, 2, 5, 4];
    for ((font, _), expected) in fonts.iter().zip(expected) {
        let asked = Cell::new(0);
        let objects = Counting {
            pdf: &pdf,
            asked: &asked,
        };
        let mut budget = BUDGET;
        let none = Font::read(objects, font, &Limits::DEFAULT, &mut budget, &mut ()).unwrap();
        assert_eq!(asked.get(), expected, "{font:?}");

        asked.set(0);
        let mut told = Vec::new();
        let mut budget = BUDGET;
        let kept = Font::read(objects, font, &Limits::DEFAULT, &mut budget, &mut told).unwrap();
        assert_eq!(asked.get(), expected, "{font:?}");
        assert_eq!(digest(&kept), digest(&none));

        asked.set(0);
        let mut budget = BUDGET;
        let unwanted =
            Font::read(objects, font, &Limits::DEFAULT, &mut budget, &mut Unwanting).unwrap();
        assert_eq!(asked.get(), expected, "{font:?}");
        assert_eq!(digest(&unwanted), digest(&none));
    }
}

/// `dict` read with a `Vec`, what's told; the same whatever the sink, in what's read
/// and in what it costs.
fn told(pdf: &Pdf, dict: &Dict) -> Vec<(Place, Fault)> {
    let mut told = Vec::new();
    let (mut kept, mut none) = (BUDGET, BUDGET);
    let read = Font::read(pdf, dict, &Limits::DEFAULT, &mut kept, &mut told);
    let without = Font::read(pdf, dict, &Limits::DEFAULT, &mut none, &mut ());
    let mut borrowed = Vec::new();
    let mut dynamic: &mut dyn Sink<Fault> = &mut borrowed;
    let mut again = BUDGET;
    let by_dyn = Font::read(pdf, dict, &Limits::DEFAULT, &mut again, &mut dynamic);
    assert_eq!(digest(&read.unwrap()), digest(&without.unwrap()));
    assert_eq!(
        digest(&by_dyn.unwrap()),
        digest(&font_of(pdf, dict.clone()))
    );
    assert_eq!((kept, again), (none, none));
    assert_eq!(told, borrowed);
    told
}

/// Each fault a reading comes to is told, where it is, and what's well formed tells
/// none.
#[test]
fn faults_are_told_where_they_are() {
    let mut pdf = Pdf::default();
    let faults = |pdf: &Pdf, font: Dict| -> Vec<Fault> {
        told(pdf, &font)
            .into_iter()
            .map(|(_, fault)| fault)
            .collect()
    };
    let descriptor = pdf.add_object(dict! {
        "Ascent" => 700, "Descent" => -200, "MissingWidth" => 100,
        "FontBBox" => vec![Object::from(0), Object::from(-200), Object::from(900), Object::from(700)],
    });
    let good = dict! {
        "Subtype" => "TrueType", "BaseFont" => "Foo", "FontDescriptor" => descriptor,
        "FirstChar" => 65, "LastChar" => 66, "Widths" => vec![Object::from(600), Object::from(700)],
        "Encoding" => dict! {
            "BaseEncoding" => "MacExpertEncoding",
            "Differences" => vec![Object::from(65), Object::from("A"), Object::from(66.5)],
        },
    };
    assert_eq!(faults(&pdf, good.clone()), []);

    let with = |key: &str, value: Object| {
        let mut font = good.clone();
        font.set(key, value);
        font
    };
    let without = |key: &[u8]| {
        let mut font = good.clone();
        font.remove(key);
        font
    };
    assert_eq!(faults(&pdf, without(b"Subtype")), [Fault::Subtype]);
    assert_eq!(
        faults(&pdf, with("Subtype", "Nope".into())),
        [Fault::Subtype]
    );
    assert_eq!(faults(&pdf, without(b"Widths")), [Fault::NoWidths]);
    assert_eq!(
        faults(&pdf, with("LastChar", 67.into())),
        [Fault::WidthsLength]
    );
    assert_eq!(
        faults(&pdf, with("FirstChar", 300.into())),
        [Fault::FirstChar, Fault::WidthsLength]
    );
    assert_eq!(faults(&pdf, without(b"FirstChar")), [Fault::FirstChar]);
    assert_eq!(
        faults(&pdf, with("FirstChar", Object::text("a"))),
        [Fault::NotANumber, Fault::FirstChar]
    );
    assert_eq!(
        faults(
            &pdf,
            with("Widths", vec![Object::from(600), Object::text("a")].into())
        ),
        [Fault::NotANumber]
    );
    assert_eq!(
        faults(&pdf, with("Encoding", "Nope".into())),
        [Fault::Encoding]
    );
    assert_eq!(
        faults(&pdf, with("Encoding", Object::from(1))),
        [Fault::Encoding]
    );
    let base = dict! { "BaseEncoding" => "Nope" };
    assert_eq!(
        faults(&pdf, with("Encoding", base.into())),
        [Fault::Encoding]
    );
    let differences = |items: Vec<Object>| dict! { "Differences" => items };
    let past = differences(vec![Object::from(300), Object::from("A")]);
    assert_eq!(
        faults(&pdf, with("Encoding", past.into())),
        [Fault::Differences]
    );
    let string = differences(vec![Object::text("a")]);
    assert_eq!(
        faults(&pdf, with("Encoding", string.into())),
        [Fault::Differences]
    );
    assert_eq!(
        faults(&pdf, with("ToUnicode", 1.into())),
        [Fault::ToUnicode]
    );
    let number = pdf.add_object(Object::from(1));
    assert_eq!(
        faults(&pdf, with("ToUnicode", number.into())),
        [Fault::ToUnicode]
    );

    let once = pdf.add_object(Object::from(descriptor));
    let chained = with("FontDescriptor", once.into());
    let placed = told(&pdf, &chained);
    assert_eq!(
        placed,
        [(Place::new(Some(descriptor), None), Fault::Chained)]
    );

    let bad = pdf.add_object(dict! { "Ascent" => Object::text("a") });
    let mut font = dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica" };
    font.set("FontDescriptor", bad);
    assert_eq!(
        told(&pdf, &font),
        [(Place::new(Some(bad), None), Fault::NotANumber)]
    );

    // Type 0's.
    let type0_with =
        |pdf: &mut Pdf, encoding: Object, descendant: Dict| type0(pdf, encoding, descendant);
    let good = type0_with(
        &mut pdf,
        "Identity-H".into(),
        dict! { "Subtype" => "CIDFontType2" },
    );
    assert_eq!(faults(&pdf, good), []);
    let bare = dict! { "Subtype" => "Type0", "Encoding" => "Identity-H" };
    assert_eq!(faults(&pdf, bare), [Fault::DescendantFonts]);
    let cid_font = pdf.add_object(dict! { "Subtype" => "CIDFontType2" });
    let two = dict! {
        "Subtype" => "Type0", "Encoding" => "Identity-H",
        "DescendantFonts" => vec![Object::from(cid_font), Object::from(cid_font)],
    };
    assert_eq!(faults(&pdf, two), [Fault::DescendantFonts]);
    let other = type0(&mut pdf, "Identity-H".into(), dict! { "Subtype" => "Nope" });
    assert_eq!(faults(&pdf, other), [Fault::Subtype]);
    let none = type0(&mut pdf, 1.into(), dict! { "Subtype" => "CIDFontType2" });
    assert_eq!(faults(&pdf, none), [Fault::CMap]);
    let unknown = type0(
        &mut pdf,
        "Adobe-Korea1-2".into(),
        dict! { "Subtype" => "CIDFontType2" },
    );
    assert_eq!(faults(&pdf, unknown), [Fault::PredefinedUnknown]);
    let w = vec![
        Object::from(1),
        Object::from(2),
        vec![Object::from(600)].into(),
    ];
    let cut = dict! { "Subtype" => "CIDFontType2", "W" => w.clone(), "W2" => w };
    let cut = type0(&mut pdf, "Identity-V".into(), cut);
    assert_eq!(faults(&pdf, cut), [Fault::W, Fault::W2]);

    // A CMap's chain.
    let program = || b"1 begincidrange <00> <FF> 0 endcidrange".to_vec();
    let nothing = pdf.add_object(Object::from(7));
    let broken = pdf.add_object(Stream::new(dict! { "UseCMap" => nothing }, program()));
    let broken = type0(
        &mut pdf,
        broken.into(),
        dict! { "Subtype" => "CIDFontType2" },
    );
    let chain = told(&pdf, &broken);
    assert_eq!(chain, [(Place::new(Some(nothing), None), Fault::UseCMap)]);
    let looped = pdf.new_object_id();
    let looping = Stream::new(dict! { "UseCMap" => looped }, program());
    pdf.objects.insert(looped, looping.into());
    let looped = type0(
        &mut pdf,
        looped.into(),
        dict! { "Subtype" => "CIDFontType2" },
    );
    assert_eq!(faults(&pdf, looped), [Fault::UseCMap]);
    let usecmap = pdf.add_object(Stream::new(dict! {}, b"/Nope-H usecmap".to_vec()));
    let usecmap = type0(
        &mut pdf,
        usecmap.into(),
        dict! { "Subtype" => "CIDFontType2" },
    );
    assert_eq!(faults(&pdf, usecmap), [Fault::PredefinedUnknown]);

    // Each says what it is, the whole of it, and no source.
    let said = Fault::Damaged {
        id: ObjectId::new(7, 0),
    };
    assert_eq!(said.to_string(), "7 0 R doesn't read");
    assert!(std::error::Error::source(&said).is_none());
    let error = Error::Unreadable(Unreadable::new(
        ObjectId::new(7, 0),
        Why::Locked,
        "7 0 R is locked",
    ));
    assert_eq!(error.to_string(), "7 0 R is locked");
    assert!(std::error::Error::source(&error).is_none());
}

/// Read for text, a font reads as it does without, this version reading no program:
/// and it's charged alike.
#[test]
fn fonts_for_text_read_the_same() {
    let (pdf, fonts) = fonts_that_reach();
    for (font, _) in fonts {
        let font = Object::from(font);
        let lim = &Limits::DEFAULT;
        let (mut one, mut other) = (BUDGET, BUDGET);
        let text = Fonts::for_text().get(&pdf, &font, lim, &mut one).unwrap();
        let all = Fonts::default().get(&pdf, &font, lim, &mut other).unwrap();
        assert_eq!(digest(&text.unwrap()), digest(&all.unwrap()));
        assert_eq!(one, other);
    }
}

/// Fonts over the programs [`crate::fixture`] builds, each by its name: `PT`, TrueType;
/// `PC`, a bare CFF; `P1`, Type 1; `PI`, an Identity-H CIDFont over the TrueType; `PN`,
/// the TrueType with no `/FontBBox`; `PZ`, the TrueType with `Ascent` and `Descent` 0;
/// `PS`, the CFF with `/Widths` from code 1.
#[cfg(feature = "programs")]
fn program_fonts() -> (Pdf, HashMap<&'static str, ObjectId>) {
    use crate::fixture::{cff_program, truetype_program, type1_program};
    let mut pdf = Pdf::default();
    let truetype = pdf.add_object(Stream::new(dict! {}, truetype_program()));
    let cff = pdf.add_object(Stream::new(dict! { "Subtype" => "Type1C" }, cff_program()));
    let type1 = pdf.add_object(Stream::new(dict! {}, type1_program()));
    let widths = |pairs: &[(u8, i64)]| {
        let mut widths = vec![Object::from(0); 72];
        for &(code, width) in pairs {
            widths[usize::from(code - 32)] = Object::from(width);
        }
        widths
    };
    let tt_widths = widths(&[
        (b' ', 250),
        (b'A', 600),
        (b'B', 450),
        (b'C', 700),
        (b'D', 500),
        (b'g', 500),
    ]);
    let ps_widths = widths(&[(b' ', 250), (b'A', 600), (b'D', 500), (b'g', 500)]);
    let simple = |pdf: &mut Pdf, subtype: &str, widths: &[Object], descriptor: Dictionary| {
        let descriptor = pdf.add_object(descriptor);
        pdf.add_object(dict! {
            "Type" => "Font", "Subtype" => subtype, "BaseFont" => "Fix",
            "FirstChar" => 32, "LastChar" => 103, "Widths" => widths.to_vec(),
            "Encoding" => "WinAnsiEncoding", "FontDescriptor" => descriptor,
        })
    };
    let descriptor = |file: &str, program: ObjectId, bbox: [i64; 4], ascent: i64, descent: i64| {
        let mut descriptor = dict! {
            "Type" => "FontDescriptor", "FontName" => "Fix", "Flags" => 32,
            "FontBBox" => bbox.iter().map(|&n| Object::from(n)).collect::<Vec<_>>(),
            "Ascent" => ascent, "Descent" => descent, "CapHeight" => 750,
            "ItalicAngle" => 0, "StemV" => 80,
        };
        descriptor.set(file, program);
        descriptor
    };
    let pt = simple(
        &mut pdf,
        "TrueType",
        &tt_widths,
        descriptor("FontFile2", truetype, [0, -200, 700, 800], 800, -200),
    );
    let pc = simple(
        &mut pdf,
        "Type1",
        &ps_widths,
        descriptor("FontFile3", cff, [0, -200, 600, 750], 750, -200),
    );
    let p1 = simple(
        &mut pdf,
        "Type1",
        &ps_widths,
        descriptor("FontFile", type1, [0, -200, 600, 750], 750, -200),
    );
    let mut no_box = descriptor("FontFile2", truetype, [0, -200, 700, 800], 900, -100);
    no_box.remove(b"FontBBox");
    let pn = simple(&mut pdf, "TrueType", &tt_widths, no_box);
    let pz = simple(
        &mut pdf,
        "TrueType",
        &tt_widths,
        descriptor("FontFile2", truetype, [0, -300, 1000, 1000], 0, 0),
    );
    let mut short = vec![Object::from(0); 103];
    for (code, width) in [(1, 400), (b'A', 300), (b'g', 500)] {
        short[usize::from(code - 1)] = Object::from(width);
    }
    let short_descriptor =
        pdf.add_object(descriptor("FontFile3", cff, [0, -100, 600, 500], 500, -100));
    let ps = pdf.add_object(dict! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Fix",
        "FirstChar" => 1, "LastChar" => 103, "Widths" => short,
        "Encoding" => "WinAnsiEncoding", "FontDescriptor" => short_descriptor,
    });
    let cid_descriptor = pdf.add_object(descriptor(
        "FontFile2",
        truetype,
        [0, -200, 700, 800],
        800,
        -200,
    ));
    let descendant = pdf.add_object(dict! {
        "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "Fix",
        "FontDescriptor" => cid_descriptor, "CIDToGIDMap" => "Identity",
        "W" => vec![Object::from(1), [600, 500, 250, 450, 700].map(Object::from).to_vec().into()],
        "CIDSystemInfo" => dict! {
            "Registry" => Object::text("Adobe"),
            "Ordering" => Object::text("Identity"), "Supplement" => 0,
        },
    });
    let pi = pdf.add_object(dict! {
        "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "Fix",
        "Encoding" => "Identity-H", "DescendantFonts" => vec![Object::from(descendant)],
    });
    let names = HashMap::from([
        ("PT", pt),
        ("PC", pc),
        ("P1", p1),
        ("PI", pi),
        ("PN", pn),
        ("PZ", pz),
        ("PS", ps),
    ]);
    (pdf, names)
}

/// Font `name` of [`program_fonts`], read within a page's budget, its program read where
/// `programs` says.
#[cfg(feature = "programs")]
fn program_font(
    fonts: &(Pdf, HashMap<&'static str, ObjectId>),
    name: &str,
    programs: bool,
) -> Arc<Font> {
    let read = match programs {
        true => Fonts::default(),
        false => Fonts::for_text(),
    };
    let mut budget = BUDGET;
    let font = Object::reference(fonts.1[name].number);
    read.get(&fonts.0, &font, &Limits::DEFAULT, &mut budget)
        .unwrap()
        .unwrap()
}

/// Each glyph's box is its program's, as FreeType loads it unscaled and
/// PDFium normalizes it to thousandths, a side below 0 a unit up
/// (`NormalizeFontMetric`); a code PDFium maps to no glyph has `-1` all
/// round in a simple font; a CIDFont's top is raised by a 64th of itself,
/// and a CID past its glyphs is all 0. TrueType, CFF and Type 1 alike.
#[cfg(feature = "programs")]
#[test]
fn outline_boxes_are_the_program_s() {
    let f = program_fonts();
    let char_box = |font: &Font, code: u8| font.char_box(code.into());
    let truetype = program_font(&f, "PT", true);
    assert_eq!(truetype.program(), FontProgram::Read);
    assert!(!truetype.approx().boxes);
    assert_eq!(
        char_box(&truetype, b'A'),
        Some(rect(0.0, 0.0, 600.0, 750.0))
    );
    // Its off-curve point is a control point the box holds.
    assert_eq!(
        char_box(&truetype, b'g'),
        Some(rect(20.0, -199.0, 480.0, 560.0))
    );
    assert_eq!(char_box(&truetype, b' '), Some(Rect::default()));
    // Drawn from its side bearing, not its outline's left.
    assert_eq!(
        char_box(&truetype, b'B'),
        Some(rect(50.0, 0.0, 350.0, 300.0))
    );
    assert_eq!(
        char_box(&truetype, b'C'),
        Some(rect(100.0, 50.0, 700.0, 800.0))
    );
    // `D` has no glyph, and draws `.notdef`; so does a code of no name.
    let notdef = Some(rect(50.0, 0.0, 450.0, 700.0));
    assert_eq!(
        (char_box(&truetype, b'D'), char_box(&truetype, 1)),
        (notdef, notdef)
    );
    for name in ["PC", "P1"] {
        let font = program_font(&f, name, true);
        assert_eq!(font.program(), FontProgram::Read, "{name:?}");
        assert_eq!(char_box(&font, b'A'), Some(rect(0.0, 0.0, 600.0, 750.0)));
        // The curve's control points.
        assert_eq!(
            char_box(&font, b'g'),
            Some(rect(20.0, -199.0, 480.0, 560.0))
        );
        assert_eq!(char_box(&font, b' '), Some(Rect::default()));
        assert_eq!(char_box(&font, b'D'), Some(Rect::default()), "{name:?}");
        assert_eq!(char_box(&font, 1), Some(rect(-1.0, -1.0, -1.0, -1.0)));
    }
    let cid = program_font(&f, "PI", true);
    assert_eq!(cid.char_box(1), Some(rect(0.0, 0.0, 600.0, 761.0)));
    assert_eq!(cid.char_box(2), Some(rect(20.0, -199.0, 480.0, 568.0)));
    assert_eq!(cid.char_box(3), Some(Rect::default()));
    assert_eq!(cid.char_box(4), Some(rect(50.0, 0.0, 350.0, 304.0)));
    assert_eq!(cid.char_box(5), Some(rect(100.0, 50.0, 700.0, 812.0)));
    assert_eq!(cid.char_box(9), Some(Rect::default()));
    // Read for text, it has none.
    let text = program_font(&f, "PT", false);
    assert_eq!(
        (text.program(), char_box(&text, b'A')),
        (FontProgram::None, None)
    );
}

/// Where the descriptor has no `/FontBBox`, the heights are the program's
/// face's, within its box, as PDFium takes them (`CheckFontMetrics`); where
/// it has one but gives `Ascent` and `Descent` 0, `A`'s top and `g`'s
/// bottom. Read for text, both are guessed, as without a program.
#[cfg(feature = "programs")]
#[test]
fn heights_from_the_program_where_the_descriptor_lacks_them() {
    let f = program_fonts();
    let heights = |name: &str, programs: bool| {
        let font = program_font(&f, name, programs);
        let m = font.metrics();
        (m.ascent, m.descent, m.bbox, font.approx().heights)
    };
    let given = Some(rect(0.0, -200.0, 700.0, 800.0));
    assert_eq!(heights("PT", true), (800.0, -200.0, given, false));
    let face = Some(rect(0.0, -199.0, 700.0, 800.0));
    assert_eq!(heights("PN", true), (800.0, -199.0, face, false));
    let own = Some(rect(0.0, -300.0, 1000.0, 1000.0));
    assert_eq!(heights("PZ", true), (750.0, -199.0, own, false));
    assert!(heights("PN", false).3 && heights("PZ", false).3);
}

/// A program that doesn't read, cut short, garbage or of no format, is malformed, and a
/// Type 1 program whose reading would cost past `type1_work` is refused for the cap:
/// either way the font's boxes guessed, its heights as without one; never a panic.
#[cfg(feature = "programs")]
#[test]
fn malformed_programs_fall_back() {
    use crate::fixture::{cff_program, truetype_program, type1_lookups_program, type1_program};
    use FontProgram::{Capped, Malformed};
    let half = |p: Vec<u8>| p[..p.len() / 2].to_vec();
    let programs = [
        ("TrueType", "FontFile2", half(truetype_program()), Malformed),
        (
            "TrueType",
            "FontFile2",
            b"\0\x01\0\0garbage".to_vec(),
            Malformed,
        ),
        ("Type1", "FontFile3", half(cff_program()), Malformed),
        ("Type1", "FontFile", half(type1_program()), Malformed),
        ("Type1", "FontFile", type1_lookups_program(3000), Capped),
        (
            "Type1",
            "FontFile3",
            b"\xff\xd8\xff\xe0 a JPEG".to_vec(),
            Malformed,
        ),
    ];
    for (subtype, key, program, read) in programs {
        let mut pdf = Pdf::default();
        let program = pdf.add_object(Stream::new(dict! {}, program));
        let mut descriptor = dict! {
            "Type" => "FontDescriptor", "Flags" => 32, "Ascent" => 700, "Descent" => -200,
        };
        descriptor.set(key, program);
        let descriptor = pdf.add_object(descriptor);
        let font = font_of(
            &pdf,
            dict! {
                "Type" => "Font", "Subtype" => subtype, "BaseFont" => "Bad",
                "FirstChar" => 65, "LastChar" => 65, "Widths" => vec![Object::from(600)],
                "FontDescriptor" => descriptor,
            },
        );
        assert_eq!(font.program(), read, "{subtype} {key}");
        assert!(font.approx().boxes && font.approx().heights);
        assert_eq!(font.char_box(65), None);
        assert_eq!(
            (font.metrics().ascent, font.metrics().descent),
            (700.0, -200.0)
        );
    }
}

/// A CIDFont's program is read once, however many of its glyphs are
/// drawn: two thousand CIDs over a Type 1 program of a MiB, each drawn as
/// it's asked for, are drawn quickly.
#[cfg(feature = "programs")]
#[test]
fn a_cid_font_s_program_is_read_once() {
    let program = crate::fixture::type1_program();
    let at = program.windows(4).position(|w| w == b"/Pai").unwrap();
    let comment = [&b"%"[..], &vec![b'x'; 1 << 20], b"\n"].concat();
    let program = [&program[..at], &comment, &program[at..]].concat();
    let mut pdf = Pdf::default();
    let program = pdf.add_object(Stream::new(dict! {}, program));
    let descriptor = pdf.add_object(dict! {
        "Type" => "FontDescriptor", "Flags" => 32, "Ascent" => 750, "Descent" => -200,
        "FontFile" => program,
    });
    let font = type0(
        &mut pdf,
        "Identity-H".into(),
        dict! { "Subtype" => "CIDFontType0", "FontDescriptor" => descriptor },
    );
    let font = font_of(&pdf, font);
    assert_eq!(font.program(), FontProgram::Read);
    let started = std::time::Instant::now();
    assert_eq!(font.char_box(2), Some(rect(0.0, 0.0, 600.0, 761.0)));
    for cid in 0..2000 {
        let _ = font.char_box(cid);
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

/// An SFNT whose `loca` record is past its end, which FreeType reads
/// past, and so it's read here: its glyphs taken for none, it reads, over its CFF.
#[cfg(feature = "programs")]
#[test]
fn a_loca_record_past_the_end_is_read_past() {
    let mut pdf = Pdf::default();
    let program = crate::fixture::loca_past_end_program();
    let program = pdf.add_object(Stream::new(dict! { "Subtype" => "OpenType" }, program));
    let descriptor = pdf.add_object(dict! {
        "Type" => "FontDescriptor", "Flags" => 32, "Ascent" => 750, "Descent" => -200,
        "FontFile3" => program,
    });
    let font = font_of(
        &pdf,
        dict! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Fix",
            "FirstChar" => 65, "LastChar" => 65, "Widths" => vec![Object::from(600)],
            "Encoding" => "WinAnsiEncoding", "FontDescriptor" => descriptor,
        },
    );
    assert_eq!(font.program(), FontProgram::Read);
}

/// A program's work is bounded: a CFF glyph whose subroutines call each
/// other past its fuel, and a TrueType composite of composites past the
/// glyphs one may draw, are unknown, and found so quickly; one within its
/// fuel is drawn.
#[cfg(feature = "programs")]
#[test]
fn programs_past_their_work_are_unknown() {
    use crate::fixture::{composite_bomb_program, heavy_cff_program};
    let font = |subtype: &str, key: &str, program: Vec<u8>| {
        let mut pdf = Pdf::default();
        let program = pdf.add_object(Stream::new(dict! {}, program));
        let mut descriptor = dict! {
            "Type" => "FontDescriptor", "Flags" => 32, "Ascent" => 750, "Descent" => 0,
            "FontBBox" => [0, 0, 600, 750].map(Object::from).to_vec(),
        };
        descriptor.set(key, program);
        let descriptor = pdf.add_object(descriptor);
        font_of(
            &pdf,
            dict! {
                "Type" => "Font", "Subtype" => subtype, "BaseFont" => "Heavy",
                "FirstChar" => 65, "LastChar" => 65, "Widths" => vec![Object::from(600)],
                "Encoding" => "WinAnsiEncoding", "FontDescriptor" => descriptor,
            },
        )
    };
    let started = std::time::Instant::now();
    let light = font("Type1", "FontFile3", heavy_cff_program(3));
    assert_eq!(light.char_box(65), Some(rect(0.0, 0.0, 640.0, 640.0)));
    let heavy = font("Type1", "FontFile3", heavy_cff_program(10));
    assert_eq!(
        (heavy.program(), heavy.char_box(65)),
        (FontProgram::Read, None)
    );
    let mut pdf = Pdf::default();
    let bomb = pdf.add_object(Stream::new(dict! {}, composite_bomb_program()));
    let descriptor = pdf.add_object(dict! {
        "Type" => "FontDescriptor", "Flags" => 32, "Ascent" => 750, "Descent" => 0,
        "FontBBox" => [0, 0, 600, 750].map(Object::from).to_vec(), "FontFile2" => bomb,
    });
    let bomb = type0(
        &mut pdf,
        "Identity-H".into(),
        dict! { "Subtype" => "CIDFontType2", "FontDescriptor" => descriptor },
    );
    let bomb = font_of(&pdf, bomb);
    assert_eq!(bomb.char_box(0), None);
    assert_eq!(bomb.char_box(6), Some(rect(0.0, 0.0, 600.0, 761.0)));
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

/// skrifa draws 2,048 components of a composite and no more, nested or not, whatever
/// the limits say: so that's the limit by default, where FreeType has none and PDFium
/// draws one past it. Empty components it doesn't count, the limit does: they cost a
/// look-up each.
#[cfg(feature = "programs")]
#[test]
fn a_composite_s_components_end_at_skrifa_s_bound() {
    use crate::fixture::composite_of_program;
    let read = |(groups, leaves, empties), limits: Limits| {
        let mut pdf = Pdf::default();
        let program = composite_of_program(groups, leaves, empties);
        let file = pdf.add_object(Stream::new(dict! {}, program));
        let descriptor = pdf.add_object(dict! { "Flags" => 4, "FontFile2" => file });
        let font = type0(
            &mut pdf,
            "Identity-H".into(),
            dict! { "Subtype" => "CIDFontType2", "FontDescriptor" => descriptor },
        );
        let mut budget = BUDGET;
        Font::read(&pdf, &font, &limits, &mut budget, &mut ()).unwrap()
    };
    let boxed = |shape, limits| read(shape, limits).char_box(0);
    let triangle = Some(rect(0.0, 0.0, 600.0, 761.0));
    let default = Limits::DEFAULT;
    assert_eq!(default.composite_glyphs, 2049);
    let mut raised = default;
    raised.composite_glyphs = 1 << 12;
    for limits in [default, raised] {
        assert_eq!(boxed((40, 8, 0), limits), triangle);
        assert_eq!(boxed((0, 2048, 0), limits), triangle);
        assert_eq!(boxed((40, 9, 0), limits), None);
        assert_eq!(boxed((0, 2049, 0), limits), None);
    }
    assert_eq!(boxed((0, 1, 2047), default), triangle);
    assert_eq!(boxed((0, 1, 2048), default), None);
    assert_eq!(boxed((0, 1, 2048), raised), triangle);
    // Past the limit, what's charged is what's counted; past skrifa's bound alone, a
    // raised limit charges what it would have drawn.
    let charged = |limits| {
        let mut budget = BUDGET;
        let font = read((0, 2049, 0), limits);
        font.charge_outlines(&[0, 0], |_| true, &mut budget)
            .unwrap();
        BUDGET - budget
    };
    assert_eq!(charged(raised), charged(default) + 2050 + 3 * 2049);
}

/// read-fonts evaluates a glyph's charstring for a million operations, each number and
/// operator one, whatever its fuel: past that a box is unknown, where FreeType has no
/// such bound and PDFium draws it.
#[cfg(feature = "programs")]
#[test]
fn a_charstring_ends_at_read_fonts_operations() {
    use crate::fixture::{long_cff_program, moving_cff_program};
    let boxed = |program: Vec<u8>, limits: Limits| {
        let mut pdf = Pdf::default();
        let font = over_program(&mut pdf, "Type1", "FontFile3", program, 32, None);
        let mut budget = BUDGET;
        let read = Font::read(&pdf, &font, &limits, &mut budget, &mut ()).unwrap();
        (read.program(), read.char_box(65))
    };
    let mut fuelled = Limits::DEFAULT;
    fuelled.font_fuel = usize::MAX;
    fuelled.glyph_fuel = usize::MAX;
    for limits in [Limits::DEFAULT, fuelled] {
        for (program, known) in [
            (long_cff_program(499_999), true),
            (long_cff_program(500_000), false),
            (moving_cff_program(6), true),
            (moving_cff_program(7), false),
        ] {
            let (read, drawn) = boxed(program, limits);
            assert_eq!((read, drawn.is_some()), (FontProgram::Read, known));
        }
    }
}

/// What a program decompresses to is charged to the budget its font is
/// read within, a `/CIDToGIDMap` stream's too, and a simple font's what
/// drawing its glyphs costs: past it, the font is too large; read for
/// text, none of it is charged.
#[cfg(feature = "programs")]
#[test]
fn a_program_is_charged_to_the_budget() {
    let f = program_fonts();
    let font = |name: &str| -> Dictionary { f.0.objects[&f.1[name]].as_dict().unwrap().clone() };
    let lim = &Limits::DEFAULT;
    let program = crate::fixture::truetype_program().len();
    let pt = font("PT");
    let read = |budget: &mut usize| Font::read(&f.0, &pt, lim, budget, &mut ());
    let mut budget = usize::MAX;
    read(&mut budget).unwrap();
    let charged = usize::MAX - budget;
    assert!(
        charged > program,
        "{charged} charged, the program {program}"
    );
    let mut budget = charged;
    read(&mut budget).unwrap();
    assert_eq!(budget, 0);
    let mut budget = charged - 1;
    assert_eq!(read(&mut budget).err(), Some(Error::TooLarge));
    assert_eq!(budget, charged - 1);
    let mut budget = 0;
    let text = Fonts::for_text();
    let by_ref = Object::reference(f.1["PT"].number);
    assert!(text.get(&f.0, &by_ref, lim, &mut budget).unwrap().is_some());

    let (mut pdf, names) = program_fonts();
    let map = pdf.add_object(Stream::new(dict! {}, vec![0, 0, 0, 2, 0, 1]));
    let cid = pdf.objects[&names["PI"]].as_dict().unwrap().clone();
    let descendant = cid.get(b"DescendantFonts").unwrap().as_array().unwrap()[0]
        .as_reference()
        .unwrap();
    let mut dictionary = pdf.objects[&descendant].as_dict().unwrap().clone();
    dictionary.set("CIDToGIDMap", map);
    pdf.objects.insert(descendant, dictionary.into());
    let mut budget = program + 6;
    let mapped = Font::read(&pdf, &cid, lim, &mut budget, &mut ()).unwrap();
    assert_eq!(budget, 0);
    // CID 1 draws glyph 2, CID 2 glyph 1, and CID 3 is past the map.
    assert_eq!(mapped.char_box(1), Some(rect(20.0, -199.0, 480.0, 568.0)));
    assert_eq!(mapped.char_box(2), Some(rect(0.0, 0.0, 600.0, 761.0)));
    assert_eq!(mapped.char_box(3), Some(Rect::default()));
    let mut budget = program + 5;
    assert_eq!(
        Font::read(&pdf, &cid, lim, &mut budget, &mut ()).err(),
        Some(Error::TooLarge)
    );
    // `A` and `g`, for its heights, are drawn as it's read, and charged.
    let mut to_gid = vec![0; 0x68 * 2];
    to_gid[0x41 * 2 + 1] = 1;
    to_gid[0x67 * 2 + 1] = 2;
    let len = to_gid.len();
    let map = pdf.add_object(Stream::new(dict! {}, to_gid));
    let mut dictionary = pdf.objects[&descendant].as_dict().unwrap().clone();
    dictionary.set("CIDToGIDMap", map);
    pdf.objects.insert(descendant, dictionary.into());
    let mut budget = usize::MAX;
    Font::read(&pdf, &cid, lim, &mut budget, &mut ()).unwrap();
    let charged = usize::MAX - budget;
    assert!(charged > program + len, "{charged}");
    let mut budget = charged - 1;
    assert_eq!(
        Font::read(&pdf, &cid, lim, &mut budget, &mut ()).err(),
        Some(Error::TooLarge)
    );
}

/// A font over `program` under `key`, `/Flags` as given, showing what its `/Encoding`,
/// where it has one, names, with widths of 500.
#[cfg(feature = "programs")]
fn over_program(
    pdf: &mut Pdf,
    subtype: &str,
    key: &str,
    program: Vec<u8>,
    flags: i64,
    encoding: Option<Object>,
) -> Dictionary {
    let program = pdf.add_object(Stream::new(dict! {}, program));
    let mut descriptor = dict! {
        "Type" => "FontDescriptor", "Flags" => flags, "Ascent" => 750, "Descent" => -200,
        "FontBBox" => [0, -200, 600, 750].map(Object::from).to_vec(),
    };
    descriptor.set(key, program);
    let mut font = dict! {
        "Type" => "Font", "Subtype" => subtype, "BaseFont" => "Fix", "FirstChar" => 32,
        "Widths" => vec![Object::from(500); 224], "FontDescriptor" => descriptor,
    };
    if let Some(encoding) = encoding {
        font.set("Encoding", encoding);
    }
    font
}

/// Programs crafted where a parser's arithmetic can overflow read without a panic in a
/// build with overflow checks, as this one is: a Type 1 program whose `/FontBBox` has a
/// number with two signs, and one whose glyph rolls by the least shift a 32-bit number
/// holds. Each still reads.
#[cfg(feature = "programs")]
#[test]
fn programs_read_where_arithmetic_could_overflow() {
    use crate::fixture::{rolling_type1_program, type1_program};
    let bbox = b"/FontBBox {0 -200 600 750}";
    let program = type1_program();
    let at = program.windows(bbox.len()).position(|w| w == bbox).unwrap();
    let doubled = [
        &program[..at],
        b"/FontBBox {5 --32768 989 775}",
        &program[at + bbox.len()..],
    ];
    for (key, program) in [
        ("FontFile", doubled.concat()),
        ("FontFile", rolling_type1_program()),
    ] {
        let mut pdf = Pdf::default();
        let font = over_program(&mut pdf, "Type1", key, program, 32, None);
        let read = Font::read(&pdf, &font, &Limits::DEFAULT, &mut BUDGET.clone(), &mut ());
        assert_eq!(read.unwrap().program(), FontProgram::Read, "{key}");
    }
}

/// What PDFium's encoding leaves to a program is its text: a symbolic Type 1 font's code
/// is the name its program's own encoding gives its glyph; a TrueType font's, by its
/// charmaps, a Mac one's table or the code itself. Read for text, it's guessed by name.
#[cfg(feature = "programs")]
#[test]
fn a_program_says_the_text_its_encoding_leaves_to_it() {
    use crate::fixture::{truetype_program, type1_program};
    let type1 = type1_program();
    let at = type1.windows(9).position(|w| w == b"/Encoding").unwrap();
    let end = at + type1[at..].windows(4).position(|w| w == b"def\n").unwrap();
    let own = b"/Encoding 256 array 0 1 255 {1 index exch /.notdef put} for \
        dup 97 /A put dup 98 /g put readonly ";
    let custom = [&type1[..at], &own[..], &type1[end..]].concat();
    let mut pdf = Pdf::default();
    let symbolic = over_program(&mut pdf, "Type1", "FontFile", custom.clone(), 4, None);
    let read = font_of(&pdf, symbolic.clone());
    assert_eq!(read.code_text(97).as_deref(), Some("A"));
    assert_eq!(read.code_text(98).as_deref(), Some("g"));
    assert_eq!(read.code_text(99), None);
    assert_eq!(read.char_box(97), Some(rect(0.0, 0.0, 600.0, 750.0)));
    // A Dingbats name in a program is its text, whatever the font's name, and a name of
    // joined parts each part's.
    let of = |name: &str| {
        let program = crate::fixture::type1_of(name);
        let own = format!(
            "/Encoding 256 array 0 1 255 {{1 index exch /.notdef put}} for dup 98 /{name} put readonly "
        );
        [&program[..at], own.as_bytes(), &program[end..]].concat()
    };
    for (name, text) in [("a73", "\u{25a0}"), ("f_i", "fi")] {
        let font = over_program(&mut pdf, "Type1", "FontFile", of(name), 4, None);
        assert_eq!(font_of(&pdf, font).code_text(98).as_deref(), Some(text));
    }
    // Its base an encoding's, its names are that, not the program's.
    let win = over_program(
        &mut pdf,
        "Type1",
        "FontFile",
        custom,
        4,
        Some("WinAnsiEncoding".into()),
    );
    let win = font_of(&pdf, win);
    assert_eq!(win.code_text(97).as_deref(), Some("a"));
    let mut budget = BUDGET;
    let font = pdf.add_object(symbolic);
    let text = Fonts::for_text()
        .get(&pdf, &font.into(), &Limits::DEFAULT, &mut budget)
        .unwrap()
        .unwrap();
    assert_eq!(text.code_text(97).as_deref(), Some("a"), "the guess");

    // A TrueType program, by the charmap it has.
    let with_map = |platform: u16, encoding: u16| {
        let mut program = truetype_program();
        let count = usize::from(u16::from_be_bytes([program[4], program[5]]));
        let record = (0..count)
            .map(|i| 12 + 16 * i)
            .find(|&r| &program[r..r + 4] == b"cmap");
        let record = record.unwrap();
        let at = u32::from_be_bytes(program[record + 8..record + 12].try_into().unwrap()) as usize;
        program[at + 4..at + 6].copy_from_slice(&platform.to_be_bytes());
        program[at + 6..at + 8].copy_from_slice(&encoding.to_be_bytes());
        program
    };
    let mut pdf = Pdf::default();
    let mac = over_program(&mut pdf, "TrueType", "FontFile2", with_map(1, 0), 4, None);
    let mac = font_of(&pdf, mac);
    assert_eq!(mac.code_text(0x41).as_deref(), Some("A"));
    assert_eq!(mac.code_text(0x8a).as_deref(), Some("\u{e4}"));
    assert_eq!(mac.code_text(0xb0), None, "PDFium's table has none");
    assert_eq!(
        mac.code_text(0xc1).as_deref(),
        Some("\u{a1}"),
        "where it has the pound"
    );
    let unicode = over_program(&mut pdf, "TrueType", "FontFile2", with_map(0, 3), 4, None);
    let unicode = font_of(&pdf, unicode);
    assert_eq!(unicode.code_text(0xe4).as_deref(), Some("\u{e4}"));
    // Said to be WinAnsiEncoding, with only a Mac charmap, it's MacRomanEncoding.
    let moved = over_program(
        &mut pdf,
        "TrueType",
        "FontFile2",
        with_map(1, 0),
        4,
        Some("WinAnsiEncoding".into()),
    );
    let moved = font_of(&pdf, moved);
    assert_eq!(moved.code_text(0x8a).as_deref(), Some("\u{e4}"));
    assert_eq!(moved.char_box(0x41), Some(rect(0.0, 0.0, 600.0, 750.0)));
    // Symbol's by its charmap: nothing where it has no Mac one too.
    let symbol = over_program(&mut pdf, "TrueType", "FontFile2", with_map(3, 0), 4, None);
    let symbol = font_of(&pdf, symbol);
    assert_eq!(symbol.code_text(0x41), None);
    assert_eq!(symbol.char_box(0x41), Some(rect(0.0, 0.0, 600.0, 750.0)));
}

/// A CID-keyed CFF's glyph for a CID is its charset's: the lowest, of a CID it gives
/// twice, as FreeType takes it; one it doesn't give draws nothing.
#[cfg(feature = "programs")]
#[test]
fn a_cid_keyed_cff_s_glyphs_are_its_charset_s() {
    let mut pdf = Pdf::default();
    let program = pdf.add_object(Stream::new(dict! {}, crate::fixture::cid_cff_program()));
    let descriptor = pdf.add_object(dict! {
        "Flags" => 4, "Ascent" => 750, "Descent" => -200, "FontFile3" => program,
        "FontBBox" => [0, -200, 600, 750].map(Object::from).to_vec(),
    });
    let font = type0(
        &mut pdf,
        "Identity-H".into(),
        dict! { "Subtype" => "CIDFontType0", "FontDescriptor" => descriptor },
    );
    let font = font_of(&pdf, font);
    assert_eq!(font.program(), FontProgram::Read);
    assert_eq!(font.char_box(5), Some(rect(0.0, 0.0, 600.0, 761.0)));
    assert_eq!(font.char_box(7), Some(rect(0.0, 0.0, 600.0, 761.0)));
    assert_eq!(font.char_box(2), Some(Rect::default()));
    assert_eq!(font.char_box(0), Some(Rect::default()));
    assert_eq!(font.char_box(1000), Some(Rect::default()));
}

/// A TrueType font FreeType draws hinted for PDFium, by its name, has no box said of any
/// code, those with no glyph among them, and says so; its text is as any other's.
#[cfg(feature = "programs")]
#[test]
fn a_tricky_font_s_boxes_are_unknown() {
    let mut pdf = Pdf::default();
    let mut font = over_program(
        &mut pdf,
        "TrueType",
        "FontFile2",
        crate::fixture::truetype_program(),
        32,
        Some("WinAnsiEncoding".into()),
    );
    font.set("BaseFont", "ABCDEF+MingLiU");
    let read = font_of(&pdf, font);
    assert_eq!(read.program(), FontProgram::Read);
    assert!(read.outlined() && read.approx().boxes);
    assert!((0..=255).all(|code| read.char_box(code).is_none()));
    assert_eq!(read.code_text(0x41).as_deref(), Some("A"));
}

/// A TrueType program's units per em are FreeType's to refuse: 16 to 16384.
#[cfg(feature = "programs")]
#[test]
fn a_truetype_s_units_per_em_are_as_freetype_takes_them() {
    let with_upem = |units: u16| {
        let mut program = crate::fixture::truetype_program();
        let count = usize::from(u16::from_be_bytes([program[4], program[5]]));
        let record = (0..count)
            .map(|i| 12 + 16 * i)
            .find(|&r| &program[r..r + 4] == b"head");
        let record = record.unwrap();
        let at = u32::from_be_bytes(program[record + 8..record + 12].try_into().unwrap()) as usize;
        program[at + 18..at + 20].copy_from_slice(&units.to_be_bytes());
        program
    };
    for (units, program, top) in [
        (16, FontProgram::Read, Some(750_000.0 / 16.0_f32)),
        (2000, FontProgram::Read, Some(375.0)),
        (16384, FontProgram::Read, Some(46.0)),
        (15, FontProgram::Malformed, None),
        (16385, FontProgram::Malformed, None),
        (0, FontProgram::Malformed, None),
    ] {
        let mut pdf = Pdf::default();
        let font = over_program(
            &mut pdf,
            "TrueType",
            "FontFile2",
            with_upem(units),
            32,
            None,
        );
        let read = font_of(&pdf, font);
        assert_eq!(read.program(), program, "{units}");
        let got = read.char_box(0x41).map(|r| r.y1);
        assert_eq!(got.map(f32::trunc), top.map(f32::trunc), "{units}");
    }
}

/// A program FreeType doesn't load reads as PDFium reads a font with none, so a font
/// with no `/Encoding` isn't left to it, but WinAnsiEncoding; and it's told.
#[cfg(feature = "programs")]
#[test]
fn a_program_that_doesn_t_load_is_read_as_none() {
    let mut pdf = Pdf::default();
    let bad = over_program(
        &mut pdf,
        "TrueType",
        "FontFile2",
        b"garbage".to_vec(),
        4,
        None,
    );
    let mut budget = BUDGET;
    let mut faults = Vec::new();
    let read = Font::read(&pdf, &bad, &Limits::DEFAULT, &mut budget, &mut faults).unwrap();
    assert_eq!(read.program(), FontProgram::Malformed);
    assert!(faults.iter().any(|(_, f)| *f == Fault::ProgramMalformed));
    assert_eq!(read.code_text(0xe4).as_deref(), Some("\u{e4}"));
    assert!(!read.outlined());
    // Read for text it can't be told, and it's taken to be embedded.
    let font = pdf.add_object(bad);
    let mut budget = BUDGET;
    let text = Fonts::for_text()
        .get(&pdf, &font.into(), &Limits::DEFAULT, &mut budget)
        .unwrap()
        .unwrap();
    assert_eq!(text.program(), FontProgram::None);
    assert_eq!(text.code_text(0xe4), None);
}

/// A font of capitals only, where its program has no glyph for a lower case, draws that
/// as its capital and advances as far; one that has a glyph keeps it.
#[cfg(feature = "programs")]
#[test]
fn a_lower_case_with_no_glyph_takes_its_capital_s() {
    let mut pdf = Pdf::default();
    let program = crate::fixture::type1_program();
    let differences = dict! { "Differences" => vec![Object::from(97), Object::from(".notdef")] };
    let mut caps = over_program(
        &mut pdf,
        "Type1",
        "FontFile",
        program,
        32 | 1 << 16,
        Some(differences.into()),
    );
    let mut widths = vec![Object::from(300); 224];
    widths[65 - 32] = Object::from(700);
    caps.set("Widths", widths);
    let read = font_of(&pdf, caps);
    let a = Some(rect(0.0, 0.0, 600.0, 750.0));
    assert_eq!((read.char_box(65), read.char_box(97)), (a, a));
    assert_eq!((read.width(65), read.width(97)), (700.0, 700.0));
    assert_eq!(read.width(98), 300.0);
}

/// Each of a program's bounds is what's in `Limits`: under smaller ones, what's past them
/// is unknown; and a Type 1 program past `type1_work` is refused before read-fonts has
/// it.
#[cfg(feature = "programs")]
#[test]
fn a_program_s_bounds_are_the_limits() {
    use crate::fixture::{composite_bomb_program, heavy_cff_program, truetype_program};
    let read = |subtype: &str, key: &str, program: Vec<u8>, limits: Limits| {
        let mut pdf = Pdf::default();
        let font = over_program(
            &mut pdf,
            subtype,
            key,
            program,
            32,
            Some("WinAnsiEncoding".into()),
        );
        let mut budget = BUDGET;
        Font::read(&pdf, &font, &limits, &mut budget, &mut ()).unwrap()
    };
    let light = || heavy_cff_program(3);
    let mut limits = Limits::DEFAULT;
    assert!(
        read("Type1", "FontFile3", light(), limits)
            .char_box(65)
            .is_some()
    );
    limits.font_fuel = 0;
    limits.glyph_fuel = 16;
    assert_eq!(
        read("Type1", "FontFile3", light(), limits).char_box(65),
        None
    );
    // A font's fuel is what each glyph's share is of, whatever the least.
    let mut limits = Limits::DEFAULT;
    limits.glyph_fuel = 16;
    assert!(
        read("Type1", "FontFile3", light(), limits)
            .char_box(65)
            .is_some()
    );

    let composite = |limits: Limits, cid: u32| {
        let mut pdf = Pdf::default();
        let program = pdf.add_object(Stream::new(dict! {}, composite_bomb_program()));
        let descriptor = pdf.add_object(dict! { "Flags" => 4, "FontFile2" => program });
        let font = type0(
            &mut pdf,
            "Identity-H".into(),
            dict! { "Subtype" => "CIDFontType2", "FontDescriptor" => descriptor },
        );
        let mut budget = BUDGET;
        let read = Font::read(&pdf, &font, &limits, &mut budget, &mut ()).unwrap();
        read.char_box(cid)
    };
    // Glyph 5 draws 50 of glyph 6.
    assert!(composite(Limits::DEFAULT, 5).is_some());
    let mut limits = Limits::DEFAULT;
    limits.composite_glyphs = 50;
    assert_eq!(composite(limits, 5), None);
    limits.composite_glyphs = 51;
    assert!(composite(limits, 5).is_some());
    let mut limits = Limits::DEFAULT;
    limits.composite_depth = 0;
    assert_eq!(composite(limits, 5), None);
    limits.composite_depth = 1;
    assert!(composite(limits, 5).is_some());
    assert!(read("TrueType", "FontFile2", truetype_program(), limits).outlined());

    // An accented character's components are looked up among all the glyphs: charged, so
    // that drawing one in a font of many glyphs doesn't cost what the glyphs do.
    use crate::fixture::type1_accents_program as accents;
    let limits = Limits::DEFAULT;
    let boxed = |program| read("Type1", "FontFile", program, limits).char_box(65);
    assert!(boxed(accents(100, 64)).is_some());
    assert!(boxed(accents(30_000, 2)).is_some());
    assert_eq!(boxed(accents(30_000, 6)), None);

    let mut limits = Limits::DEFAULT;
    let lookups = crate::fixture::type1_lookups_program(100);
    assert_eq!(
        read("Type1", "FontFile", lookups.clone(), limits).program(),
        FontProgram::Read
    );
    limits.type1_work = 0;
    assert_eq!(
        read("Type1", "FontFile", lookups, limits).program(),
        FontProgram::Capped
    );
}

/// A Type 1 program past `type1_work` is refused for the cap, told so, never as a
/// program that doesn't read, and read as one that doesn't, but for what it says of
/// its program; one whose scan finds it broken before the cap's spent is malformed, and
/// what's no Type 1 program is malformed whatever the cap.
#[cfg(feature = "programs")]
#[test]
fn a_type1_program_past_its_cap_is_told_as_the_cap_s() {
    let read = |program: Vec<u8>, type1_work: usize| {
        let mut pdf = Pdf::default();
        let encoding = Some("WinAnsiEncoding".into());
        let font = over_program(&mut pdf, "Type1", "FontFile", program, 32, encoding);
        let limits = Limits {
            type1_work,
            ..Limits::DEFAULT
        };
        let (mut budget, mut told) = (BUDGET, Vec::new());
        let read = Font::read(&pdf, &font, &limits, &mut budget, &mut told).unwrap();
        let faults: Vec<Fault> = told.into_iter().map(|(_, fault)| fault).collect();
        (read, faults)
    };
    let lookups = crate::fixture::type1_lookups_program(100);
    let (font, faults) = read(lookups.clone(), Limits::DEFAULT.type1_work);
    assert_eq!((font.program(), faults), (FontProgram::Read, vec![]));
    let (capped, faults) = read(lookups, 0);
    assert_eq!(capped.program(), FontProgram::Capped);
    assert_eq!(faults, [Fault::ProgramCapped]);
    let no_eexec = b"%!PS-AdobeFont-1.0: Fix\n/FontName /Fix def\n".to_vec();
    let (malformed, faults) = read(no_eexec, Limits::DEFAULT.type1_work);
    assert_eq!(malformed.program(), FontProgram::Malformed);
    assert_eq!(faults, [Fault::ProgramMalformed]);
    let said = |font: &Font| {
        let codes = 0..=255;
        let boxes: Vec<_> = codes.clone().map(|code| font.char_box(code)).collect();
        let text: Vec<_> = codes
            .map(|code| font.code_text(code).map(Cow::into_owned))
            .collect();
        let metrics = font.metrics();
        (
            font.approx(),
            font.outlined(),
            metrics.ascent,
            metrics.descent,
            boxes,
            text,
        )
    };
    assert_eq!(said(&capped), said(&malformed));
    for type1_work in [0, Limits::DEFAULT.type1_work] {
        let (font, faults) = read(b"garbage".to_vec(), type1_work);
        assert_eq!(font.program(), FontProgram::Malformed, "{type1_work}");
        assert_eq!(faults, [Fault::ProgramMalformed], "{type1_work}");
    }
}

/// Drawing a CIDFont's glyphs is charged as what shows them says it's the first to.
#[cfg(feature = "programs")]
#[test]
fn a_cid_font_s_outlines_are_charged_as_they_re_shown() {
    let f = program_fonts();
    let cid = program_font(&f, "PI", true);
    let s = b"\x00\x01\x00\x02\x00\x01";
    let mut first = std::collections::HashSet::new();
    let mut budget = usize::MAX;
    cid.charge_outlines(s, |glyph| first.insert(glyph), &mut budget)
        .unwrap();
    assert_eq!(first.len(), 2);
    let spent = usize::MAX - budget;
    assert!(spent > 0);
    let mut budget = spent - 1;
    let mut first = std::collections::HashSet::new();
    let shown = cid.charge_outlines(s, |glyph| first.insert(glyph), &mut budget);
    assert_eq!(shown, Err(Error::TooLarge));
    let mut budget = 0;
    assert_eq!(cid.charge_outlines(s, |_| false, &mut budget), Ok(()));
    let pt = program_font(&f, "PT", true);
    let mut budget = 0;
    assert_eq!(pt.charge_outlines(b"Ag", |_| true, &mut budget), Ok(()));
}

/// A font read for text reads no program of any kind, and charges none; what it has of
/// its boxes and heights is as with none.
#[cfg(feature = "programs")]
#[test]
fn for_text_reads_no_program() {
    let f = program_fonts();
    for name in ["PT", "PC", "P1", "PI", "PN", "PZ", "PS"] {
        let mut budget = 0;
        let font = Object::reference(f.1[name].number);
        let read = Fonts::for_text()
            .get(&f.0, &font, &Limits::DEFAULT, &mut budget)
            .unwrap()
            .unwrap();
        assert_eq!(read.program(), FontProgram::None, "{name}");
        assert!(!read.outlined() && read.approx().boxes, "{name}");
        assert_eq!((read.cost(), read.char_box(65)), (0, None), "{name}");
    }
}

/// Without the feature a font embeds no program that's read, whoever asks: what reading
/// it comes to is what `Fonts::for_text` comes to, its program never charged.
#[cfg(not(feature = "programs"))]
#[test]
fn without_the_feature_a_font_reads_as_for_text() {
    use crate::fixture::{cff_program, truetype_program, type1_program};
    for (subtype, key, program) in [
        ("TrueType", "FontFile2", truetype_program()),
        ("Type1", "FontFile3", cff_program()),
        ("Type1", "FontFile", type1_program()),
    ] {
        let mut pdf = Pdf::default();
        let program = pdf.add_object(Stream::new(dict! {}, program));
        let mut descriptor = dict! { "Flags" => 4, "FontBBox" => vec![Object::from(0); 4] };
        descriptor.set(key, program);
        let font = pdf.add_object(dict! {
            "Subtype" => subtype, "BaseFont" => "Fix", "FontDescriptor" => descriptor,
            "FirstChar" => 65, "Widths" => vec![Object::from(500)],
        });
        let font = Object::from(font);
        let lim = &Limits::DEFAULT;
        let (mut one, mut other) = (0, 0);
        let text = Fonts::for_text()
            .get(&pdf, &font, lim, &mut one)
            .unwrap()
            .unwrap();
        let all = Fonts::default()
            .get(&pdf, &font, lim, &mut other)
            .unwrap()
            .unwrap();
        assert_eq!(digest(&text), digest(&all));
        assert_eq!(all.program(), FontProgram::None);
        assert!(!all.outlined() && all.approx().boxes && all.approx().heights);
    }
}

/// Where table `tag` of an SFNT is, and how long.
#[cfg(feature = "programs")]
fn sfnt_table(program: &[u8], tag: &[u8; 4]) -> (usize, usize) {
    let tables = u16::from_be_bytes([program[4], program[5]]) as usize;
    (0..tables)
        .map(|i| &program[12 + 16 * i..28 + 16 * i])
        .find(|record| &record[..4] == tag)
        .map(|record| {
            let word = |at: usize| u32::from_be_bytes(record[at..at + 4].try_into().unwrap());
            (word(8) as usize, word(12) as usize)
        })
        .expect("the table")
}

/// A TrueType `program` over `pdf`, as a Type 0 font in Identity, each CID its glyph.
#[cfg(feature = "programs")]
fn identity_truetype(pdf: &mut Pdf, program: Vec<u8>) -> Dict {
    let file = pdf.add_object(Stream::new(dict! {}, program));
    let descriptor = pdf.add_object(dict! { "Flags" => 4, "FontFile2" => file });
    type0(
        pdf,
        "Identity-H".into(),
        dict! { "Subtype" => "CIDFontType2", "FontDescriptor" => descriptor },
    )
}

/// A `loca` entry past its table charges what's read, none of it, however far it
/// claims: a font of a few hundred bytes is no more than it holds.
#[cfg(feature = "programs")]
#[test]
fn a_loca_entry_past_its_table_charges_nothing() {
    use crate::fixture::truetype_program;
    let read = |patch: Option<usize>, budget: &mut usize| {
        let mut program = truetype_program();
        let (loca, _) = sfnt_table(&program, b"loca");
        if let Some(entry) = patch {
            program[loca + 4 * entry] = 0x7f;
        }
        let mut pdf = Pdf::default();
        let file = pdf.add_object(Stream::new(dict! {}, program));
        let descriptor = dict! { "Flags" => 32, "FontFile2" => file };
        let font = dict! {
            "Subtype" => "TrueType", "BaseFont" => "Fix", "Encoding" => "WinAnsiEncoding",
            "FontDescriptor" => descriptor, "FirstChar" => 65, "Widths" => vec![Object::from(500)],
        };
        Font::read(&pdf, &font, &Limits::DEFAULT, budget, &mut ()).map(|font| font.cost())
    };
    let whole = read(None, &mut BUDGET.clone()).unwrap();
    for entry in 1..=6 {
        let mut budget = 1 << 16;
        let cost = read(Some(entry), &mut budget).expect("what it holds is within this");
        assert!(cost <= whole, "entry {entry}: {cost} past {whole}");
    }
}

/// A composite of itself ends at any depth a `Limits` can say, 255 among them: past it
/// its box is unknown.
#[cfg(feature = "programs")]
#[test]
fn a_composite_of_itself_ends_at_any_depth() {
    use crate::fixture::truetype_program;
    let mut program = truetype_program();
    let (glyf, len) = sfnt_table(&program, b"glyf");
    // Glyph 5, the program's composite, the last ten bytes but for its one component's
    // flags, is made a component of itself.
    let component = glyf + len - 8;
    assert_eq!(program[component..component + 2], [0, 1]);
    program[component..component + 2].copy_from_slice(&[0, 5]);
    let mut pdf = Pdf::default();
    let font = identity_truetype(&mut pdf, program);
    for depth in [0, 1, 32, 254, u8::MAX] {
        let limits = Limits {
            composite_depth: depth,
            ..Limits::DEFAULT
        };
        let mut budget = BUDGET;
        let read = Font::read(&pdf, &font, &limits, &mut budget, &mut ()).unwrap();
        assert_eq!(read.char_box(5), None, "at a depth of {depth}");
    }
}

/// A glyph past the program's fails as FreeType fails it, where nothing's a program's
/// empty glyph: PDFium's box of no glyph, for a code made a glyph where the program has
/// no charmap.
#[cfg(feature = "programs")]
#[test]
fn a_glyph_past_the_program_is_none() {
    use crate::fixture::truetype_program;
    let mut program = truetype_program();
    let (at, _) = (0..20)
        .map(|i| &program[12 + 16 * i..])
        .position(|record| record.starts_with(b"cmap"))
        .map(|i| (12 + 16 * i, ()))
        .unwrap();
    program[at + 3] = b'q';
    let mut pdf = Pdf::default();
    let file = pdf.add_object(Stream::new(dict! {}, program));
    let descriptor = dict! { "Flags" => 4, "FontFile2" => file };
    let font = dict! {
        "Subtype" => "TrueType", "BaseFont" => "ABCDEF+Fix", "FontDescriptor" => descriptor,
        "FirstChar" => 0, "Widths" => vec![Object::from(500); 256],
    };
    let read = font_of(&pdf, font);
    // Glyph 1 is the program's, and 65 is past its six.
    assert!(read.char_box(1).is_some_and(|b| b.y1 > 0.0));
    assert_eq!(read.char_box(65), Some(rect(-1.0, -1.0, -1.0, -1.0)));
}

/// What FreeType and skrifa read otherwise is said, so a box is unknown and not wrong: a
/// `loca` of a format that's neither, long metrics that don't fit `hmtx` or none, and a
/// glyph that doesn't parse.
#[cfg(feature = "programs")]
#[test]
fn what_skrifa_reads_otherwise_is_unknown() {
    use crate::fixture::truetype_program;
    let patched = |patch: &dyn Fn(&mut Vec<u8>)| {
        let mut program = truetype_program();
        patch(&mut program);
        let mut pdf = Pdf::default();
        let font = identity_truetype(&mut pdf, program);
        let read = font_of(&pdf, font);
        (read.approx().boxes, read.char_box(1), read.char_box(2))
    };
    let (exact, first, second) = patched(&|_| {});
    assert!(!exact && first.is_some() && second.is_some());
    let format = |value: i16| {
        move |p: &mut Vec<u8>| {
            let (head, _) = sfnt_table(p, b"head");
            p[head + 50..head + 52].copy_from_slice(&value.to_be_bytes());
        }
    };
    let metrics = |value: u16| {
        move |p: &mut Vec<u8>| {
            let (hhea, _) = sfnt_table(p, b"hhea");
            p[hhea + 34..hhea + 36].copy_from_slice(&value.to_be_bytes());
        }
    };
    assert!(patched(&format(0)).0 == exact || patched(&format(1)).0 == exact);
    for inexact in [patched(&format(2)), patched(&format(-1))] {
        assert_eq!(inexact, (true, None, None));
    }
    for k in [0, 7, u16::MAX] {
        assert_eq!(patched(&metrics(k)), (true, None, None), "{k}");
    }
    for k in [1, 6] {
        assert!(!patched(&metrics(k)).0, "{k}");
    }
    // A `cmap` whose records don't fit.
    let records = |p: &mut Vec<u8>| {
        let (cmap, _) = sfnt_table(p, b"cmap");
        p[cmap + 2..cmap + 4].copy_from_slice(&[1, 1]);
    };
    assert_eq!(patched(&records), (true, None, None));
    // A composite of an empty glyph draws nothing, as an empty glyph does, and isn't
    // unknown.
    let mut program = truetype_program();
    let (glyf, len) = sfnt_table(&program, b"glyf");
    program[glyf + len - 7] = 3;
    let mut pdf = Pdf::default();
    let font = identity_truetype(&mut pdf, program);
    assert_eq!(font_of(&pdf, font).char_box(5), Some(Rect::default()));
    // Glyph 1's contours past what it holds: still a font of boxes, this one unknown.
    let (exact, first, second) = patched(&|p| {
        let (glyf, _) = sfnt_table(p, b"glyf");
        p[glyf + 36] = 0x7f;
    });
    assert!(!exact && second.is_some());
    assert_eq!(first, None);
    // And a composite of it, which draws nothing and isn't a composite of empty glyphs.
    let mut program = truetype_program();
    let (glyf, _) = sfnt_table(&program, b"glyf");
    program[glyf + 36] = 0x7f;
    let mut pdf = Pdf::default();
    let font = identity_truetype(&mut pdf, program);
    assert_eq!(font_of(&pdf, font).char_box(5), None);
}

/// A CMap name PDFium doesn't know has no text of its CIDs, its CMap being one it never
/// loaded, whichever collection its CIDFont orders; Identity has.
#[test]
fn a_name_pdfium_does_not_know_has_no_cid_s_text() {
    let mut pdf = Pdf::default();
    let japan1 = dict! {
        "CIDSystemInfo" => dict! {
            "Registry" => Object::text("Adobe"), "Ordering" => Object::text("Japan1"),
        },
    };
    for name in ["Foo", "FooV"] {
        let font = type0(&mut pdf, name.into(), japan1.clone());
        let read = font_of(&pdf, font);
        assert_eq!(read.code_text(1), None);
        assert_eq!(read.cid(1), Some(1));
        assert_eq!(read.is_vertical(), name.ends_with('V'));
    }
    #[cfg(feature = "predefined-cmaps")]
    {
        let font = type0(&mut pdf, "Identity-H".into(), japan1);
        assert!(font_of(&pdf, font).code_text(1).is_some());
    }
}

/// A range ending past 16 bits maps only codes past them, by where it ends, as PDFium's
/// table holds only those within them; and within them the last given wins.
#[test]
fn a_cmap_s_ranges_past_16_bits_are_apart() {
    let read = |cmap: &str| {
        let mut pdf = Pdf::default();
        let cmap = pdf.add_object(Stream::new(dict! {}, cmap.as_bytes().to_vec()));
        let font = type0(&mut pdf, cmap.into(), dict! { "Subtype" => "CIDFontType2" });
        font_of(&pdf, font)
    };
    let mixed = "2 begincodespacerange <00> <ff> <00000000> <ffffffff> endcodespacerange ";
    let font = read(&format!(
        "{mixed} 1 begincidrange <00000042> <00010042> 1 endcidrange"
    ));
    assert_eq!(font.cid(0x42), Some(0));
    assert_eq!(font.cid(0x1_0042), Some(1));
    let font = read(&format!(
        "{mixed} 1 begincidchar <414141> 12 endcidchar \
         1 begincidrange <00414141> <00414142> 65536 endcidrange"
    ));
    assert_eq!(font.cid(0x41_4141), Some(12));
    assert_eq!(font.cid(0x41_4142), Some(1));
    let font = read(&format!(
        "{mixed} 2 begincidrange <0041> <0042> 7 <0042> <0043> 9 endcidrange"
    ));
    assert_eq!(font.cid(0x42), Some(9));
    assert_eq!(font.cid(0x41), Some(7));
    // Its first past its last holds only what's past it, nothing.
    let font = read(&format!(
        "{mixed} 1 begincidrange <00050000> <00040000> 3 endcidrange"
    ));
    assert_eq!(font.cid(0x4_8000), Some(0));
}

/// Whether PDFium loads a font is said wherever it's read, as what's told says, and
/// through `Fonts`, which tells nothing.
#[test]
fn whether_a_font_is_loaded_is_said() {
    let mut pdf = Pdf::default();
    let cid_font = pdf.add_object(dict! { "Subtype" => "CIDFontType2" });
    let type0 = |encoding: Option<Object>, fonts: Option<Object>| {
        let mut font = dict! { "Subtype" => "Type0", "BaseFont" => "Foo" };
        if let Some(encoding) = encoding {
            font.set("Encoding", encoding);
        }
        if let Some(fonts) = fonts {
            font.set("DescendantFonts", fonts);
        }
        font
    };
    let one = || Some(Object::from(vec![Object::from(cid_font)]));
    let identity = || Some(Object::from("Identity-H"));
    let cases = [
        (type0(identity(), one()), true),
        (type0(Some("Foo".into()), one()), true),
        (type0(Some(5.into()), one()), false),
        (type0(None, one()), false),
        (type0(identity(), None), false),
        (
            type0(identity(), Some(Object::from(Vec::<Object>::new()))),
            false,
        ),
        (
            dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica" },
            true,
        ),
        (dict! { "Subtype" => "TrueType", "BaseFont" => "Foo" }, true),
        (dict! { "Subtype" => "Type3" }, true),
    ];
    for (font, loaded) in cases {
        let refused = told(&pdf, &font)
            .iter()
            .any(|(_, fault)| matches!(fault, Fault::DescendantFonts | Fault::CMap));
        assert_eq!(refused, !loaded, "{font:?}");
        assert_eq!(font_of(&pdf, font.clone()).loaded(), loaded, "{font:?}");
        let written = Object::from(font);
        let read = Fonts::default().get(&pdf, &written, &Limits::DEFAULT, &mut BUDGET.clone());
        assert_eq!(read.unwrap().unwrap().loaded(), loaded);
    }
}

/// A font read for text reads no program, whatever the build's features, and tells what
/// else it comes to.
#[test]
fn a_font_read_for_text_reads_no_program() {
    use crate::fixture::truetype_program;
    let mut pdf = Pdf::default();
    let file = pdf.add_object(Stream::new(dict! {}, truetype_program()));
    let bad = pdf.add_object(Stream::new(dict! {}, vec![0u8; 8]));
    let lim = &Limits::DEFAULT;
    for program in [file, bad] {
        let descriptor = dict! { "Flags" => 4, "FontFile2" => program };
        let font = dict! {
            "Subtype" => "TrueType", "BaseFont" => "Fix", "FontDescriptor" => descriptor,
            "FirstChar" => "A", "Widths" => vec![Object::from(500)],
        };
        let (mut one, mut other) = (BUDGET, BUDGET);
        let mut faults = Vec::new();
        let text = Font::read_for_text(&pdf, &font, lim, &mut one, &mut faults).unwrap();
        let written = Object::from(font);
        let kept = Fonts::for_text()
            .get(&pdf, &written, lim, &mut other)
            .unwrap();
        assert_eq!(digest(&text), digest(&kept.unwrap()));
        assert_eq!((one, other), (BUDGET, BUDGET));
        assert_eq!(text.program(), FontProgram::None);
        assert!(!text.outlined() && text.approx().boxes);
        let faults: Vec<_> = faults.into_iter().map(|(_, fault)| fault).collect();
        assert!(faults.contains(&Fault::FirstChar) && !faults.contains(&Fault::ProgramMalformed));
    }
}

/// A fault in what's numbered says the object it's in: an encoding dictionary, its
/// `/Differences`, a `/Widths`, a Type 3 font's, a `/W`, a `/W2`, a `/DW2`, a box.
#[test]
fn a_fault_says_the_numbered_object_it_is_in() {
    let mut pdf = Pdf::default();
    let not_a_number: Object = "x".into();
    let widths = pdf.add_object(vec![not_a_number.clone()]);
    let differences = pdf.add_object(vec![Object::from(true)]);
    let encoding = pdf.add_object(dict! {
        "BaseEncoding" => "Foo", "Differences" => differences,
    });
    let bare = pdf.add_object(dict! { "Differences" => vec![Object::from(true)] });
    let named = pdf.add_object(Object::from("Foo"));
    let places = |pdf: &Pdf, font: Dict| -> Vec<(Fault, Option<ObjectId>)> {
        told(pdf, &font)
            .into_iter()
            .map(|(place, fault)| (fault, place.object))
            .collect()
    };
    let told_of = places(
        &pdf,
        dict! {
            "Subtype" => "Type1", "BaseFont" => "Foo", "Encoding" => encoding,
            "FirstChar" => 65, "LastChar" => 65, "Widths" => widths,
        },
    );
    assert!(
        told_of.contains(&(Fault::NotANumber, Some(widths))),
        "{told_of:?}"
    );
    assert!(
        told_of.contains(&(Fault::Encoding, Some(encoding))),
        "{told_of:?}"
    );
    assert!(
        told_of.contains(&(Fault::Differences, Some(differences))),
        "{told_of:?}"
    );
    let told_of = places(
        &pdf,
        dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => bare },
    );
    assert!(
        told_of.contains(&(Fault::Differences, Some(bare))),
        "{told_of:?}"
    );
    let told_of = places(
        &pdf,
        dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => named },
    );
    assert!(
        told_of.contains(&(Fault::Encoding, Some(named))),
        "{told_of:?}"
    );
    // In the font's own dictionary, a list written there names none.
    let told_of = places(
        &pdf,
        dict! {
            "Subtype" => "Type1", "BaseFont" => "Helvetica",
            "FirstChar" => 65, "Widths" => vec![not_a_number.clone()],
        },
    );
    assert!(told_of.contains(&(Fault::NotANumber, None)), "{told_of:?}");
    let told_of = places(
        &pdf,
        dict! {
            "Subtype" => "Type3", "FontMatrix" => widths, "FirstChar" => 65, "Widths" => widths,
        },
    );
    assert!(
        told_of.contains(&(Fault::NotANumber, Some(widths))),
        "{told_of:?}"
    );
    let matrix = pdf.add_object(vec![not_a_number.clone(); 6]);
    let told_of = places(&pdf, dict! { "Subtype" => "Type3", "FontMatrix" => matrix });
    assert!(
        told_of.contains(&(Fault::NotANumber, Some(matrix))),
        "{told_of:?}"
    );
    let w = pdf.add_object(vec![Object::from(1), not_a_number.clone(), Object::from(2)]);
    let dw2 = pdf.add_object(vec![not_a_number.clone(), Object::from(1)]);
    let cid_font = pdf.add_object(dict! {
        "Subtype" => "CIDFontType2", "W" => w, "W2" => w, "DW2" => dw2,
    });
    let told_of = places(
        &pdf,
        dict! {
            "Subtype" => "Type0", "Encoding" => "Identity-V",
            "DescendantFonts" => vec![Object::from(cid_font)],
        },
    );
    assert!(
        told_of.contains(&(Fault::NotANumber, Some(w))),
        "{told_of:?}"
    );
    assert!(
        told_of.contains(&(Fault::NotANumber, Some(dw2))),
        "{told_of:?}"
    );
}

/// What reads a file implements `Streams` on itself and `Resolve` on a reference to it,
/// as `Resolve` needs to give what it holds, and a font is read over the reference.
#[test]
fn streams_go_on_the_type_and_a_font_is_read_over_a_reference() {
    struct Source {
        objects: HashMap<ObjectId, Object>,
    }
    impl<'o> Resolve<'o> for &'o Source {
        fn resolve(&self, id: ObjectId) -> Option<&'o Object> {
            let source: &'o Source = self;
            source.objects.get(&id)
        }
    }
    impl Streams for Source {
        fn decoded(&self, id: ObjectId, _: usize) -> Result<Cow<'_, [u8]>, Unreadable> {
            Err(Unreadable::new(id, Why::Damaged, "no streams"))
        }
    }
    let source = Source {
        objects: HashMap::new(),
    };
    let font = dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica" };
    let mut budget = BUDGET;
    let read = Font::read(&source, &font, &Limits::DEFAULT, &mut budget, &mut ()).unwrap();
    assert_eq!(read.width(u32::from(b'A')), 667.0);
}

/// `tests/pdfium/programs`, for `tests/pdfium/cases.py`, whose files are Python's and so
/// can't call the builders: each program by the name the file has.
fn programs_written() -> Vec<(&'static str, Vec<u8>)> {
    use crate::fixture::{
        OpenType, cff_program, cid_cff_program, cid_cff_with, composite_of_program,
        moving_cff_program, opentype_cid_program, opentype_program, truetype_program, type1_of,
        type1_program,
    };
    let of = |post_names, cmap| OpenType { post_names, cmap };
    let thousandth = ["0.001", "0", "0", "0.001", "0", "0"];
    let two_thousandths = ["0.002", "0", "0", "0.002", "0", "0"];
    let one = ["1", "0", "0", "1", "0", "0"];
    let sheared = ["0.001", "0", "0.0005", "0.001", "0", "0"];
    vec![
        ("truetype", truetype_program()),
        ("composite-2048", composite_of_program(40, 8, 0)),
        ("composite-2049", composite_of_program(40, 9, 0)),
        ("cff", cff_program()),
        ("cff-moving-6", moving_cff_program(6)),
        ("cff-moving-7", moving_cff_program(7)),
        ("type1", type1_program()),
        ("type1-dingbat", type1_of("a73")),
        ("cid-cff", cid_cff_program()),
        ("cid-cff-top", cid_cff_with(Some(two_thousandths), None)),
        ("cid-cff-font", cid_cff_with(None, Some(two_thousandths))),
        (
            "cid-cff-both-one",
            cid_cff_with(Some(thousandth), Some(one)),
        ),
        (
            "cid-cff-one-both",
            cid_cff_with(Some(one), Some(thousandth)),
        ),
        (
            "cid-cff-both-two",
            cid_cff_with(Some(thousandth), Some(two_thousandths)),
        ),
        (
            "cid-cff-both-sheared",
            cid_cff_with(Some(sheared), Some(two_thousandths)),
        ),
        ("otto", opentype_program(of(false, true))),
        ("otto-post-names", opentype_program(of(true, true))),
        ("otto-no-cmap", opentype_program(of(false, false))),
        ("otto-post-names-no-cmap", opentype_program(of(true, false))),
        ("cid-otto", opentype_cid_program(of(false, true))),
        ("cid-otto-post-names", opentype_cid_program(of(true, true))),
        ("cid-otto-no-cmap", opentype_cid_program(of(false, false))),
    ]
}

fn programs_folder() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pdfium/programs")
}

/// An OpenType font's CFF names its glyphs, whatever its `post` says or names, and draws
/// them where its Unicode charmap has none of them (`cff_face_init`,
/// `cff_get_name_index`).
#[cfg(feature = "programs")]
#[test]
fn an_opentype_font_s_names_are_its_cff_s() {
    use crate::fixture::{OpenType, opentype_program};
    for post_names in [false, true] {
        let program = opentype_program(OpenType {
            post_names,
            cmap: true,
        });
        let mut pdf = Pdf::default();
        for flags in [32, 4] {
            let font = over_program(
                &mut pdf,
                "Type1",
                "FontFile3",
                program.clone(),
                flags,
                Some("WinAnsiEncoding".into()),
            );
            let read = font_of(&pdf, font);
            assert_eq!(read.program(), FontProgram::Read);
            let space = rect(0.0, 0.0, 0.0, 0.0);
            assert_eq!(read.char_box(b' '.into()), Some(space));
            let a = rect(0.0, 0.0, 600.0, 750.0);
            assert_eq!(read.char_box(b'A'.into()), Some(a));
            let g = rect(20.0, -199.0, 480.0, 560.0);
            assert_eq!(read.char_box(b'g'.into()), Some(g));
        }
    }
}

/// An OpenType font's Unicode charmap where it has no `cmap`: of its `post`'s names,
/// where they're the SFNT's to give, so a code draws what those name, or else of its
/// CFF's.
#[cfg(feature = "programs")]
#[test]
fn an_opentype_font_with_no_cmap_has_one_of_its_names() {
    use crate::fixture::{OpenType, opentype_program};
    let (a, g) = (
        rect(0.0, 0.0, 600.0, 750.0),
        rect(20.0, -199.0, 480.0, 560.0),
    );
    for (post_names, a, g) in [(false, a, g), (true, g, a)] {
        let program = opentype_program(OpenType {
            post_names,
            cmap: false,
        });
        let mut pdf = Pdf::default();
        let font = over_program(
            &mut pdf,
            "TrueType",
            "FontFile3",
            program,
            32,
            Some("WinAnsiEncoding".into()),
        );
        let read = font_of(&pdf, font);
        assert_eq!(read.char_box(b'A'.into()), Some(a));
        assert_eq!(read.char_box(b'g'.into()), Some(g));
    }
}

/// A CID-keyed CFF in an OpenType font: a CID is its glyph, as FreeType keeps no CIDs but
/// for a bare one's, where each is by its charset.
#[cfg(feature = "programs")]
#[test]
fn an_opentype_font_s_cids_are_its_glyphs() {
    use crate::fixture::{OpenType, cid_cff_program, opentype_cid_program};
    let program = opentype_cid_program(OpenType {
        post_names: false,
        cmap: true,
    });
    let boxes = |program: Vec<u8>| {
        let mut pdf = Pdf::default();
        let file = pdf.add_object(Stream::new(dict! {}, program));
        let descriptor = pdf.add_object(dict! { "Flags" => 4, "FontFile3" => file });
        let font = type0(
            &mut pdf,
            "Identity-H".into(),
            dict! { "Subtype" => "CIDFontType0", "FontDescriptor" => descriptor },
        );
        let read = font_of(&pdf, font);
        [1, 2, 5].map(|cid| read.char_box(cid).map(|b| b.y1))
    };
    assert_eq!(boxes(program), [761.0, 568.0, 0.0].map(Some));
    assert_eq!(boxes(cid_cff_program()), [0.0, 0.0, 761.0].map(Some));
}

/// An SFNT's format is its tag's, as FreeType's readers take them, whatever tables it
/// has: a CFF under TrueType's tags, or a TrueType under `OTTO`, reads as no program.
#[cfg(feature = "programs")]
#[test]
fn an_sfnt_s_format_is_its_tag_s() {
    use crate::fixture::{OpenType, opentype_program, truetype_program};
    let tagged = |program: Vec<u8>, tag: &[u8; 4]| {
        let mut program = program;
        program[..4].copy_from_slice(tag);
        let mut pdf = Pdf::default();
        let font = over_program(&mut pdf, "TrueType", "FontFile2", program, 32, None);
        font_of(&pdf, font).program()
    };
    let opentype = || {
        opentype_program(OpenType {
            post_names: false,
            cmap: true,
        })
    };
    assert_eq!(tagged(opentype(), b"OTTO"), FontProgram::Read);
    for tag in [b"\0\x01\0\0", b"\0\x02\0\0", b"true", b"typ1"] {
        assert_eq!(tagged(opentype(), tag), FontProgram::Malformed);
    }
    for tag in [b"\0\x01\0\0", b"\0\x02\0\0", b"true"] {
        assert_eq!(tagged(truetype_program(), tag), FontProgram::Read);
    }
    for tag in [b"OTTO", b"typ1"] {
        assert_eq!(tagged(truetype_program(), tag), FontProgram::Malformed);
    }
}

/// Writes `tests/pdfium/programs`: run it when a builder changes, then
/// `oracle/pdfium.py cases --write`.
#[test]
#[ignore = "writes into the source tree"]
fn the_programs_are_written() {
    std::fs::create_dir_all(programs_folder()).unwrap();
    for (name, program) in programs_written() {
        std::fs::write(programs_folder().join(name), program).unwrap();
    }
}

/// The oracle's programs are what the builders build. A package holds no such folder.
#[test]
fn the_programs_are_the_builders() {
    if !programs_folder().exists() {
        return;
    }
    for (name, program) in programs_written() {
        let written = std::fs::read(programs_folder().join(name)).unwrap();
        assert!(written == program, "{name}: run the_programs_are_written");
    }
}

/// A font over a reading of a file: what it's charged, and what it comes to where a
/// stream it reads doesn't read, doesn't decode or is past a cap, as pdf-structure's
/// `Streams` for a `Reading` says of each (whose own tests hold what it says).
mod reading {
    use std::borrow::Cow;
    use std::io;
    use std::ops::Range;
    use std::sync::atomic::{AtomicBool, Ordering};

    use pdf_fixtures::fixture::{self, Body, Xref};
    use pdf_structure::pdf_crypt::Password;
    use pdf_structure::pdf_crypt::writer::{Kind, Settings};
    use pdf_structure::read::{ByteSource, Options, Reading, WithReading};

    use super::*;

    const PLAIN: &[u8] = b"1 beginbfchar <41> <0041> endbfchar";

    fn settings(user: &'static [u8]) -> Settings<'static> {
        Settings {
            kind: Kind::Aes256,
            user,
            owner: b"owner",
            p: -4,
            id: b"0123456789abcdef",
            encrypt_metadata: true,
        }
    }

    /// The objects of a file with a font, object 1, and its `/ToUnicode`, object 2.
    fn objects() -> Vec<(u32, Body)> {
        vec![
            (
                1,
                dict! { "Subtype" => "Type1", "BaseFont" => "Foo", "ToUnicode" => Object::reference(2) }
                    .into(),
            ),
            (2, Body::stream(dict! {}, PLAIN)),
        ]
    }

    fn font_of_reading<S: ByteSource>(
        reading: &Reading<S>,
        budget: &mut usize,
        told: &mut Vec<(Place, Fault)>,
    ) -> Result<Font, Error> {
        let font = reading.get(ObjectId::new(1, 0)).unwrap().as_dict().unwrap();
        Font::read(reading, font, &Limits::DEFAULT, budget, told)
    }

    /// A font over a reading is charged what its streams decoded, and fails a byte
    /// short, its budget as it was; and so is one over the file read whole beside its
    /// reading, which is a resolver and a `Streams` as one.
    #[test]
    fn a_font_is_charged_what_it_decoded() {
        let file = fixture::file("1.7", &objects(), dict! {}, Xref::Table);
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        let mut budget = PLAIN.len();
        let font = font_of_reading(&reading, &mut budget, &mut Vec::new()).unwrap();
        assert_eq!(budget, 0);
        assert_eq!(font.code_text(0x41).as_deref(), Some("A"));
        let mut budget = PLAIN.len() - 1;
        let font = font_of_reading(&reading, &mut budget, &mut Vec::new());
        assert_eq!(font.err(), Some(Error::TooLarge));
        assert_eq!(budget, PLAIN.len() - 1);

        let with = WithReading::new(&file[..], &Options::DEFAULT).unwrap();
        let dict = (&with).resolve(ObjectId::new(1, 0)).unwrap();
        let dict = dict.as_dict().unwrap();
        let read =
            |budget: &mut usize| Font::read(&with, dict, &Limits::DEFAULT, budget, &mut Vec::new());
        let mut budget = PLAIN.len();
        let font = read(&mut budget).unwrap();
        assert_eq!(budget, 0);
        assert_eq!(font.code_text(0x41).as_deref(), Some("A"));
        let mut budget = PLAIN.len() - 1;
        assert_eq!(read(&mut budget).err(), Some(Error::TooLarge));
        assert_eq!(budget, PLAIN.len() - 1);
    }

    /// A `/ToUnicode` whose data decodes to nothing, or is through a filter the reader
    /// doesn't decode, is read as none, what's told of it the font's own; one listing
    /// more filters than the reader takes is past a cap, the caller's to know, its
    /// budget as it was and nothing told after it.
    #[test]
    fn what_doesn_t_decode_is_none_and_past_a_cap_the_caller_s() {
        let many: Vec<Object> = (0..26).map(|_| Object::from("ASCIIHexDecode")).collect();
        let to_unicode = [
            dict! { "Filter" => "ASCIIHexDecode" },
            dict! { "Filter" => "JBIG2Decode" },
            dict! { "Filter" => many },
        ];
        let read = |dict: Dict, data: &[u8]| {
            let mut objects = objects();
            objects[1] = (2, Body::stream(dict, data));
            let file = fixture::file("1.7", &objects, dict! {}, Xref::Table);
            let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
            let (mut budget, mut told) = (100, Vec::new());
            let font = font_of_reading(&reading, &mut budget, &mut told);
            let told: Vec<Fault> = told.into_iter().map(|(_, fault)| fault).collect();
            let text = font.map(|font| font.code_text(0x41).map(Cow::into_owned));
            (text, budget, told)
        };
        let [nothing, unfiltered, capped] = to_unicode;
        for (dict, data) in [(nothing, &b"zz"[..]), (unfiltered, b"data")] {
            let (text, budget, told) = read(dict, data);
            assert_eq!(text, Ok(Some("A".into())));
            // Its `/ToUnicode`'s, and its `/Widths`', which it hasn't.
            assert_eq!(
                (budget, told),
                (100, vec![Fault::ToUnicode, Fault::NoWidths])
            );
        }
        let (text, budget, told) = read(capped, b"41>");
        let Err(Error::Unreadable(unreadable)) = text else {
            panic!("{text:?}");
        };
        assert_eq!(
            (unreadable.id, unreadable.why),
            (ObjectId::new(2, 0), Why::Capped)
        );
        // Nothing after it: not what it makes of none, nor the `/Widths` it hasn't.
        assert_eq!((budget, told), (100, vec![]));
    }

    /// A locked file's font isn't made up, and its budget is as it was; opened, it's
    /// read.
    #[test]
    fn a_locked_file_s_font_isn_t_made_up() {
        let file = fixture::encrypted(&settings(b"user"), &objects(), dict! {});
        let locked = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        let mut budget = 100;
        let font = font_of_reading(&locked, &mut budget, &mut Vec::new());
        let Err(Error::Unreadable(unreadable_font)) = font else {
            panic!("{font:?}");
        };
        assert_eq!(
            (unreadable_font.id, unreadable_font.why),
            (ObjectId::new(2, 0), Why::Locked)
        );
        assert_eq!(budget, 100);

        let mut options = Options::DEFAULT;
        options.passwords = &[Password("user")];
        let opened = Reading::open(&file[..], &options).unwrap();
        let mut budget = 100;
        let font = font_of_reading(&opened, &mut budget, &mut Vec::new()).unwrap();
        assert_eq!(font.code_text(0x41).as_deref(), Some("A"));
    }

    /// A file read by position, whose source fails once it's told to.
    struct Failing {
        bytes: Vec<u8>,
        failing: AtomicBool,
    }

    impl ByteSource for Failing {
        fn len(&self) -> usize {
            self.bytes.len()
        }

        fn read_at(&self, range: Range<usize>) -> io::Result<Cow<'_, [u8]>> {
            match self.failing.load(Ordering::Relaxed) {
                true => Err(io::Error::other("no bytes")),
                false => Ok(Cow::Borrowed(&self.bytes[range])),
            }
        }

        fn first_window(&self) -> usize {
            64
        }
    }

    /// A font whose stream's data the source fails to give is the source's to know,
    /// its budget as it was; given them again, it's read.
    #[test]
    fn a_font_whose_source_fails() {
        let bytes = fixture::file("1.7", &objects(), dict! {}, Xref::Table);
        let source = Failing {
            bytes,
            failing: AtomicBool::new(false),
        };
        let reading = Reading::open(&source, &Options::DEFAULT).unwrap();
        let fail = |on: bool| source.failing.store(on, Ordering::Relaxed);
        // The objects read and kept, their data's read is what fails.
        assert!(reading.get(ObjectId::new(2, 0)).is_ok());
        assert!(reading.get(ObjectId::new(1, 0)).is_ok());
        fail(true);
        let mut budget = 100;
        let font = font_of_reading(&reading, &mut budget, &mut Vec::new());
        assert!(
            matches!(&font, Err(Error::Unreadable(u)) if u.why == Why::Source),
            "{font:?}"
        );
        assert_eq!(budget, 100);
        fail(false);
        let font = font_of_reading(&reading, &mut budget, &mut Vec::new()).unwrap();
        assert_eq!(font.code_text(0x41).as_deref(), Some("A"));
    }
}
