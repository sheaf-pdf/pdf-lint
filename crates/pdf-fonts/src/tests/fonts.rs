//! Files' fonts as read, built in code: each kind's codes, widths and heights,
//! what's charged, what fails and what's told; and over a reading of a file, every
//! way a stream's data may come to a font.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;

use pdf_objects::dict;
use pdf_objects::object::Stream;

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
        (self.last, 0)
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
    fn decoded(&self, id: ObjectId, limit: usize) -> Decoded {
        let Some(stream) = self.objects.get(&id).and_then(Object::as_stream) else {
            return Decoded::None;
        };
        let data = stream.data.bytes().unwrap_or_default();
        match data.len() > limit {
            true => Decoded::TooLarge,
            false => Decoded::Data(data.to_vec()),
        }
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
    assert_eq!(
        [1, 0x80, 0x81, b'A'.into()].map(|c| encoded.width_guessed(c)),
        [false, true, true, false]
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

    let program = pdf.add_object(Stream::new(dict! {}, vec![0]));
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
    // A predefined CMap the build hasn't, or PDFium doesn't know, splits by
    // the `/ToUnicode` codespace, its CIDs unknown: a name outside ISO's list; CNS-EUC-V, which PDFium's table hasn't; KSCpc-EUC-V, which
    // Adobe's hasn't.
    for name in ["Adobe-Korea1-V", "CNS-EUC-V", "KSCpc-EUC-V"] {
        let predefined = type0(&mut pdf, name.into(), dict! { "DW" => 800 });
        let predefined = font_of(&pdf, predefined);
        assert!(predefined.approx().codes, "{name}");
        assert_eq!(predefined.cid(0x41), None);
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
                "Registry" => Object::string("Other"), "Ordering" => ordering,
            },
        }
    };
    let text = |pdf: &mut Pdf, encoding: Object, descendant: Dictionary| {
        let font = type0(pdf, encoding, descendant);
        font_of(pdf, font).text(b"\x01\x08\x03\x4b")
    };
    let japan1 = info(Object::string("Japan1"));
    let named = info("Japan1".into());
    let identity = info(Object::string("Identity"));
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
    assert_eq!(font.text(b"\x30\x42"), None);
    assert_eq!(font.code_text(0x3042), None);
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
/// CMap that uses itself read once; Identity used by `usecmap` known;
/// another predefined CMap, by `usecmap` or `/UseCMap`, read for the codes
/// nothing else maps, by their length, and guessed, as PDFium reads none,
/// as is one the build hasn't.
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
    assert!(!font.approx().codes);

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
    assert!(!identity.approx().codes);
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
        let picked = scope.font(&pdf, (1, 0), Some(&resources), name, lim, &mut budget);
        assert!(picked.unwrap().is_some());
    }
    let picked = scope.font(&pdf, (2, 0), Some(&resources), b"D1", lim, &mut budget);
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
    let mut next = pdf.add_object(Stream::new(dict! {}, program()));
    for _ in 0..2 {
        next = pdf.add_object(Stream::new(dict! { "UseCMap" => next }, program()));
    }
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
    let (page, other) = ((1, 0), (2, 0));
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
    held::<Error>();
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
    let unknown = type0(&mut pdf, "Adobe-Korea1-2".into(), dict! {});
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
        "CIDSystemInfo" => dict! { "Ordering" => Object::string("Japan1") },
    };
    let identity = type0(&mut pdf, "Identity-H".into(), identity);
    assert_eq!(font_of(&pdf, identity).text(b"\x01\x08"), None);
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
    fn decoded(&self, id: ObjectId, limit: usize) -> Decoded {
        self.pdf.decoded_by(id, limit)
    }
}

impl Pdf {
    fn decoded_by(&self, id: ObjectId, limit: usize) -> Decoded {
        let pdf = self;
        Streams::decoded(&pdf, id, limit)
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
    let at_fault = pdf.add_object(Object::string("a"));
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
    let info = pdf.add_object(dict! { "Ordering" => Object::string("Identity") });
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

/// A stream where a dictionary is wanted is none, though its dictionary is there: a
/// font's descriptor, its CIDFont, a font itself.
#[test]
fn a_stream_is_no_dictionary() {
    let mut pdf = Pdf::default();
    let descriptor = pdf.add_object(Stream::new(
        dict! { "Ascent" => 700, "Descent" => -200, "FontBBox" => vec![Object::from(0), Object::from(-200), Object::from(900), Object::from(700)] },
        Vec::new(),
    ));
    let font = font_of(
        &pdf,
        dict! { "Subtype" => "Type1", "FontDescriptor" => descriptor },
    );
    assert_eq!(font.metrics().ascent, 1000.0);
    let cid_font = pdf.add_object(Stream::new(dict! { "DW" => 500 }, Vec::new()));
    let font = dict! {
        "Subtype" => "Type0", "Encoding" => "Identity-H", "DescendantFonts" => vec![Object::from(cid_font)],
    };
    assert_eq!(font_of(&pdf, font).width(1), 1000.0);
    let by_ref = Object::from(cid_font);
    let got = Fonts::default().get(&pdf, &by_ref, &Limits::DEFAULT, &mut 0);
    assert!(got.unwrap().is_none());
}

/// What each object a font reaches failing does to the font: `Why::Damaged` is an
/// object that isn't there, read as with none, and anything else the `Err`, whole,
/// and the budget as it was; never a font with its widths made up.
#[test]
fn each_why_over_a_failing_resolver() {
    let (pdf, fonts) = fonts_that_reach();
    for (font, reached) in &fonts {
        for &failing in reached {
            for why in [Why::Limit, Why::Locked, Why::Source] {
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

/// `Fonts` and `FontScope` say what a resolver says too, and keep nothing of it: the
/// font itself, the resources it's looked up in, the `/Font` dictionary.
#[test]
fn fonts_and_their_scope_say_why_a_font_doesn_t_read() {
    let mut pdf = Pdf::default();
    let font = pdf.add_object(dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica" });
    let named = pdf.add_object(dict! { "F1" => font });
    let held = pdf.add_object(dict! { "Font" => named });
    let resources = Object::from(held);
    let lim = &Limits::DEFAULT;
    let by_ref = Object::from(font);
    for why in [Why::Limit, Why::Locked, Why::Source] {
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
                (1000, 0),
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

/// A font trusts no `Streams`: what's given past what was asked is too large, the
/// budget as it was, and what's given within it is charged; too large and none are
/// what they say; and what a stream's source says of why is the `Err`, or for
/// `Why::Damaged` no stream.
#[test]
fn a_font_holds_a_streams_to_what_it_asked() {
    let mut pdf = Pdf::default();
    let to_unicode = pdf.add_object(Stream::new(dict! {}, Vec::new()));
    let font = dict! { "Subtype" => "Type1", "BaseFont" => "Helvetica", "ToUnicode" => to_unicode };
    let lim = &Limits::DEFAULT;
    let read = |given: &dyn Fn(usize) -> Decoded, budget: &mut usize| {
        let asked = Cell::new(None);
        let objects = With::new(&pdf, |id, limit| {
            assert_eq!(id, to_unicode);
            asked.set(Some(limit));
            given(limit)
        });
        let read = Font::read(objects, &font, lim, budget, &mut ());
        (read, asked.get())
    };
    let many = |_: usize| Decoded::Data(vec![b' '; 100]);
    for over in [0, 1, 99] {
        let mut budget = over;
        let (font, asked) = read(&many, &mut budget);
        assert_eq!(font.err(), Some(Error::TooLarge));
        assert_eq!((budget, asked), (over, Some(over)));
    }
    let mut budget = 100;
    let (font, asked) = read(&many, &mut budget);
    assert_eq!(font.unwrap().cost(), 100);
    assert_eq!((budget, asked), (0, Some(100)));

    let mut budget = 100;
    let (font, _) = read(&|_| Decoded::TooLarge, &mut budget);
    assert_eq!(font.err(), Some(Error::TooLarge));
    assert_eq!(budget, 100);
    let (font, _) = read(&|_| Decoded::None, &mut budget);
    assert_eq!(font.unwrap().cost(), 0);
    assert_eq!(budget, 100);

    for why in [Why::Limit, Why::Locked, Why::Source] {
        let unreadable = Unreadable::new(to_unicode, why, "no");
        let given = |_: usize| Decoded::Unreadable(unreadable.clone());
        let mut budget = 100;
        let (font, _) = read(&given, &mut budget);
        assert_eq!(font.err(), Some(Error::Unreadable(unreadable.clone())));
        assert_eq!(budget, 100);
    }
    let unreadable = Unreadable::new(to_unicode, Why::Damaged, "no");
    let given = |_: usize| Decoded::Unreadable(unreadable.clone());
    let (font, _) = read(&given, &mut budget);
    assert_eq!(font.unwrap().cost(), 0);
}

/// A stream a font reads is asked for by its own number, its chain of references
/// followed first, as a reading says of a reference that it's no stream: a
/// `/ToUnicode`, a CMap and the one it uses, a CharProc.
#[test]
fn a_stream_is_asked_for_by_its_own_number() {
    let mut pdf = Pdf::default();
    let chained = |pdf: &mut Pdf, stream: Stream| {
        let stream = pdf.add_object(stream);
        let once = pdf.add_object(Object::from(stream));
        pdf.add_object(Object::from(once))
    };
    let to_unicode = chained(
        &mut pdf,
        Stream::new(dict! {}, b"1 beginbfchar <41> <0041> endbfchar".to_vec()),
    );
    let simple = dict! { "Subtype" => "Type1", "BaseFont" => "Foo", "ToUnicode" => to_unicode };
    assert_eq!(font_of(&pdf, simple).code_text(0x41).as_deref(), Some("A"));

    let base = chained(
        &mut pdf,
        Stream::new(dict! {}, b"1 begincidchar <0005> 9 endcidchar".to_vec()),
    );
    let cmap = chained(
        &mut pdf,
        Stream::new(
            dict! { "UseCMap" => base },
            b"1 begincidchar <0006> 8 endcidchar".to_vec(),
        ),
    );
    let font = type0(&mut pdf, cmap.into(), dict! {});
    let font = font_of(&pdf, font);
    assert_eq!((font.cid(5), font.cid(6)), (Some(9), Some(8)));
    assert!(!font.approx().codes);

    let proc = chained(&mut pdf, Stream::new(dict! {}, b"100 0 d0".to_vec()));
    let type3 = dict! {
        "Subtype" => "Type3", "CharProcs" => dict! { "a" => proc },
        "Encoding" => dict! { "Differences" => vec![Object::from(65), Object::from("a")] },
    };
    assert_eq!(font_of(&pdf, type3).width(65), 100_000.0);

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
    fn decoded(&self, id: ObjectId, limit: usize) -> Decoded {
        self.pdf.decoded_by(id, limit)
    }
}

/// A sink that says it wants none, and fails if it's told any.
struct Unwanting;

impl Sink for Unwanting {
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
    let mut dynamic: &mut dyn Sink = &mut borrowed;
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
        faults(&pdf, with("FirstChar", Object::string("a"))),
        [Fault::NotANumber, Fault::FirstChar]
    );
    assert_eq!(
        faults(
            &pdf,
            with(
                "Widths",
                vec![Object::from(600), Object::string("a")].into()
            )
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
    let string = differences(vec![Object::string("a")]);
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

    let bad = pdf.add_object(dict! { "Ascent" => Object::string("a") });
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
    let said = Fault::Damaged { id: (7, 0) };
    assert_eq!(said.to_string(), "object 7 0 doesn't read");
    assert!(std::error::Error::source(&said).is_none());
    let error = Error::Unreadable(Unreadable::new((7, 0), Why::Locked, "7 0 R is locked"));
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

/// Every way a stream's data may come to a font from a reading of a file.
#[cfg(feature = "structure")]
mod reading {
    use std::borrow::Cow;
    use std::io;
    use std::ops::Range;
    use std::sync::atomic::{AtomicBool, Ordering};

    use pdf_structure::Password;
    use pdf_structure::bytes::{ByteSource, Options, Reading};
    use pdf_structure::fixture::{self, Body, Xref};
    use pdf_structure::pdf_crypt::writer::{Kind, Settings};

    use super::*;

    fn asked<S: ByteSource>(reading: &Reading<S>, id: ObjectId, limit: usize) -> Decoded {
        Streams::decoded(&reading, id, limit)
    }

    fn unreadable(decoded: Decoded) -> Why {
        match decoded {
            Decoded::Unreadable(unreadable) => unreadable.why,
            other => panic!("{other:?}"),
        }
    }

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

    /// The objects of a file with a font: its `/ToUnicode` object 2, and a stream of
    /// its data through each way, object 3 Flate's.
    fn objects() -> Vec<(u32, Body)> {
        vec![
            (
                1,
                dict! { "Subtype" => "Type1", "BaseFont" => "Foo", "ToUnicode" => Object::reference(2) }
                    .into(),
            ),
            (2, Body::stream(dict! {}, PLAIN)),
            (
                3,
                Body::stream(dict! { "Filter" => "FlateDecode" }, &fixture::zlib(PLAIN)),
            ),
        ]
    }

    fn font_of_reading<S: ByteSource>(
        reading: &Reading<S>,
        budget: &mut usize,
    ) -> Result<Font, Error> {
        let font = reading.get((1, 0)).unwrap().as_dict().unwrap();
        Font::read(reading, font, &Limits::DEFAULT, budget, &mut ())
    }

    /// A stream within its limit is its data; past it too large, each by a byte, with
    /// filters and without; and past the reading's own limit, whatever's asked.
    #[test]
    fn data_within_and_past_the_limit() {
        let file = fixture::file("1.7", &objects(), dict! {}, Xref::Table);
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        for id in [(2, 0), (3, 0)] {
            let at = PLAIN.len();
            assert_eq!(
                asked(&reading, id, at),
                Decoded::Data(PLAIN.to_vec()),
                "{id:?}"
            );
            assert_eq!(asked(&reading, id, at - 1), Decoded::TooLarge, "{id:?}");
            assert_eq!(asked(&reading, id, 0), Decoded::TooLarge, "{id:?}");
        }
        let mut options = Options::DEFAULT;
        options.limits.stream = 4;
        let small = Reading::open(&file[..], &options).unwrap();
        assert_eq!(asked(&small, (2, 0), 1000), Decoded::TooLarge);
        assert_eq!(asked(&small, (3, 0), 1000), Decoded::TooLarge);

        // And a font over it is charged what it decoded, and fails a byte short.
        let mut budget = PLAIN.len();
        let font = font_of_reading(&reading, &mut budget).unwrap();
        assert_eq!(budget, 0);
        assert_eq!(font.code_text(0x41).as_deref(), Some("A"));
        let mut budget = PLAIN.len() - 1;
        let font = font_of_reading(&reading, &mut budget);
        assert_eq!(font.err(), Some(Error::TooLarge));
        assert_eq!(budget, PLAIN.len() - 1);
    }

    /// A stream that decodes in part is what decoded, as PDFium takes it; none where
    /// nothing did; what the reader can't decode by its filters, or isn't a stream or
    /// there, is none; a stream listing more filters than the reader takes is past
    /// its cap.
    #[test]
    fn what_doesn_t_decode_whole() {
        let many: Vec<Object> = (0..26).map(|_| Object::from("ASCIIHexDecode")).collect();
        let objects = vec![
            (
                1,
                Body::stream(dict! { "Filter" => "ASCIIHexDecode" }, b"4142 zz"),
            ),
            (
                2,
                Body::stream(dict! { "Filter" => "ASCIIHexDecode" }, b"zz"),
            ),
            (
                3,
                Body::stream(dict! { "Filter" => "JBIG2Decode" }, b"data"),
            ),
            (4, dict! { "Not" => "a stream" }.into()),
            (5, Body::stream(dict! { "Filter" => many }, b"41>")),
            (6, Body::Object(Object::from(2))),
        ];
        let file = fixture::file("1.7", &objects, dict! {}, Xref::Table);
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        assert_eq!(asked(&reading, (1, 0), 100), Decoded::Data(b"AB".to_vec()));
        assert_eq!(asked(&reading, (2, 0), 100), Decoded::None);
        assert_eq!(asked(&reading, (3, 0), 100), Decoded::None);
        assert_eq!(asked(&reading, (4, 0), 100), Decoded::None);
        assert_eq!(asked(&reading, (99, 0), 100), Decoded::None);
        assert_eq!(asked(&reading, (6, 0), 100), Decoded::None);
        assert_eq!(unreadable(asked(&reading, (5, 0), 100)), Why::Limit);
    }

    /// An object that doesn't parse is none, as it isn't there; one past a cap the
    /// reader reads under says so.
    #[test]
    fn what_the_resolver_says_of_it_first() {
        let deep = [
            "[".repeat(pdf_structure::bytes::NESTING + 8),
            "]".repeat(pdf_structure::bytes::NESTING + 8),
        ]
        .concat();
        let objects: [(u32, &[u8]); 2] = [(1, b"<< /Unended ("), (2, deep.as_bytes())];
        let file = fixture::assembled("1.7", &objects, "<< /Size 3 >>");
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        assert!(matches!(
            Resolve::try_resolve(&&reading, (1, 0)),
            Err(Unreadable {
                why: Why::Damaged,
                ..
            })
        ));
        assert_eq!(asked(&reading, (1, 0), 100), Decoded::None);
        assert_eq!(unreadable(asked(&reading, (2, 0), 100)), Why::Limit);
    }

    /// A locked file's stream is locked, and a font that reads it isn't made up; opened,
    /// it's read. A stream that doesn't decrypt, in a file that's open, and one of a
    /// handler the reader can't open say so too.
    #[test]
    fn a_locked_file_s_streams() {
        let mut objects = objects();
        objects.push((
            4,
            Body::stream(
                dict! {
                    "Filter" => vec![Object::from("Crypt")],
                    "DecodeParms" => vec![Object::from(dict! { "Name" => "Undefined" })],
                },
                b"data",
            ),
        ));
        let file = fixture::encrypted(&settings(b"user"), &objects, dict! {});
        let locked = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        assert_eq!(unreadable(asked(&locked, (2, 0), 100)), Why::Locked);
        let mut budget = 100;
        let font = font_of_reading(&locked, &mut budget);
        let Err(Error::Unreadable(unreadable_font)) = font else {
            panic!("{font:?}");
        };
        assert_eq!(
            (unreadable_font.id, unreadable_font.why),
            ((2, 0), Why::Locked)
        );
        assert_eq!(budget, 100);

        let opened = Reading::open_with(&file[..], &[Password("user")], &Options::DEFAULT).unwrap();
        let mut budget = 100;
        let font = font_of_reading(&opened, &mut budget).unwrap();
        assert_eq!(font.code_text(0x41).as_deref(), Some("A"));
        assert!(matches!(
            opened.decoded_within((4, 0), 100),
            Err(pdf_structure::Error::Decrypt(_))
        ));
        assert_eq!(unreadable(asked(&opened, (4, 0), 100)), Why::Locked);

        let at = file.windows(9).position(|w| w == b"/Standard").unwrap();
        let other = [&file[..at], b"/Standarx", &file[at + 9..]].concat();
        let unopened = Reading::open(&other[..], &Options::DEFAULT).unwrap();
        assert!(matches!(
            unopened.decoded_within((2, 0), 100),
            Err(pdf_structure::Error::Unsupported)
        ));
        assert_eq!(unreadable(asked(&unopened, (2, 0), 100)), Why::Locked);
    }

    /// A stream the encryption dictionary refers to isn't handed out, as it may be
    /// ciphertext.
    #[test]
    fn a_stream_the_encryption_dictionary_refers_to() {
        let mut objects = objects();
        objects.push((7, Body::stream(dict! {}, b"abc")));
        let mut file = fixture::encrypted(&settings(b""), &objects, dict! {});
        // The encryption dictionary, the next object, written again in an update so
        // that it refers to the stream.
        let find = |file: &[u8], what: &[u8]| {
            let at = file.windows(what.len()).rposition(|w| w == what);
            at.unwrap_or_else(|| panic!("{}", what.escape_ascii()))
        };
        let start = find(&file, b"8 0 obj\n");
        let end = start + find(&file[start..], b"endobj");
        let own = String::from_utf8_lossy(&file[start..end]).into_owned();
        assert!(own.contains("/Length 256"), "{own}");
        let own = own.replacen("/Length 256", "/Length 7 0 R", 1);
        let prev = find(&file, b"xref\n0 ");
        let at = file.len();
        file.extend_from_slice(format!("{own}endobj\n").as_bytes());
        let xref = file.len();
        file.extend_from_slice(
            format!(
                "xref\n8 1\n{at:010} 00000 n \ntrailer\n<< /Size 9 /Encrypt 8 0 R \
                 /ID [(0123456789abcdef) (0123456789abcdef)] /Prev {prev} >>\n\
                 startxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        let reading = Reading::open(&file[..], &Options::DEFAULT).unwrap();
        let got = reading.decoded_within((7, 0), 100);
        assert!(
            matches!(got, Err(pdf_structure::Error::Encrypted)),
            "{got:?}"
        );
        assert_eq!(unreadable(asked(&reading, (7, 0), 100)), Why::Locked);
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

    /// A source that fails is `Why::Source`, whether it fails an object's read or its
    /// data's, and asked again once it gives them it's read.
    #[test]
    fn a_source_that_fails() {
        let bytes = fixture::file("1.7", &objects(), dict! {}, Xref::Table);
        let source = Failing {
            bytes,
            failing: AtomicBool::new(false),
        };
        let reading = Reading::open(&source, &Options::DEFAULT).unwrap();
        let fail = |on: bool| source.failing.store(on, Ordering::Relaxed);

        fail(true);
        assert_eq!(unreadable(asked(&reading, (2, 0), 100)), Why::Source);
        let got = Resolve::try_resolve(&&reading, (3, 0));
        assert!(matches!(
            got,
            Err(Unreadable {
                why: Why::Source,
                ..
            })
        ));
        fail(false);
        // The object read and kept, its data's read is what fails.
        assert!(reading.get((2, 0)).is_ok());
        assert!(reading.get((1, 0)).is_ok());
        fail(true);
        assert!(matches!(
            reading.decoded_within((2, 0), 100),
            Err(pdf_structure::Error::Io(_))
        ));
        assert_eq!(unreadable(asked(&reading, (2, 0), 100)), Why::Source);
        let mut budget = 100;
        let font = font_of_reading(&reading, &mut budget);
        assert!(
            matches!(&font, Err(Error::Unreadable(u)) if u.why == Why::Source),
            "{font:?}"
        );
        assert_eq!(budget, 100);
        fail(false);
        assert_eq!(asked(&reading, (2, 0), 100), Decoded::Data(PLAIN.to_vec()));
    }
}
