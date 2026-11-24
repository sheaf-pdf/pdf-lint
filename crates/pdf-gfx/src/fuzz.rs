//! In-process fuzzing entrypoints exercised by unit tests and `fuzz/fuzz_targets/gfx.rs`.
//!
//! Verifies on arbitrary byte inputs:
//! 1. Zero panics across `interpret`, `interpret_with`, and `encode`.
//! 2. Sink invariance: `interpret` (`&mut ()`) and `interpret_with` (`Vec`)
//!    produce identical `Result<Summary, Error>`.
//! 3. `Wants` invariance: `Wants::NONE` produces the exact same
//!    `Result<Summary, Error>` as `Wants::ALL`.
//! 4. Re-encode round-trip invariance: whenever `encode` succeeds, re-interpreting
//!    the encoded page and Form XObject streams reproduces the exact same
//!    non-`Op` geometry, text, clip, form, image, shading, and mark events.
//! 5. `/DA` round-trip invariance: whenever `DefaultAppearance::parse` reads the
//!    page content, what `to_bytes` writes parses again and writes the same
//!    bytes again. `DefaultAppearance::parse_with` gives what `parse` gives
//!    whatever its sink, tells a sink that wants none nothing, and tells a
//!    `Vec` what it read past: an unreadable string's error last where it
//!    gives `None`, a missing font last where it gives one without, every
//!    offset within the string.
//! 6. Box borders: `Canvas::box_border` over a box, width, dash array, and
//!    colors taken from the input's raw bytes draws nothing for a width or box
//!    it can't use, and otherwise one balanced `q`..`Q` scope that interprets
//!    without an error or a fault into at most three painted paths;
//!    `Canvas::box_bands` draws the same bands, in a scope of their own, and
//!    nothing for a style without them; `Canvas::box_bands_in` in the shades
//!    `box_bands` chooses writes what it writes, and in shades taken from
//!    the input the same bands, drawn exactly where those are.
//! 7. Layout flags (`text-layout`): `layout_text` over the input's first bytes
//!    with `TextBoxStyle::line_end_spaces` and `font_heights_as_given` each on
//!    and off gives the same error or none, lines over the text in order with
//!    a code for each character, none wider than its box but one of a single
//!    character; with `line_end_spaces`, every character but a line break on
//!    a line; with `font_heights_as_given`, Helvetica, whose heights the
//!    floors don't move, laid out as without it.
//! 8. Fallback (`text-layout`): `layout_text` over the same text in Helvetica
//!    with Symbol after it (`WithFallback`), a character and a grapheme
//!    cluster at a time, in lines and in comb cells: laid out by clusters
//!    only where it is by characters; each line's runs its codes in order,
//!    a face each, the next face another, each run's units drawn in the face
//!    `face_for` gives them, each cell in its unit's; text whose every unit,
//!    drawn or past a comb's cells, is Helvetica's laid out as Helvetica
//!    alone lays it out, and other text refused by Helvetica alone at the
//!    first unit it lacks; text of one character a cluster the same either
//!    way; and what `Canvas::draw_layout_in` draws interprets to a glyph a
//!    code, each face under its resource.

use std::collections::BTreeMap;
use std::ops::ControlFlow;

use pdf_content_streams::{Color, Concat, Join, Reserved};
use pdf_fonts::Fonts;
use pdf_objects::fault::{Place, Sink};
use pdf_objects::parse::Parser;
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{
    Dict, Object, ObjectId, Resolve, Stream, Streams, Unreadable, Value, Why, WithStreams,
};
use pdf_semantics::pages::{self, Inheritable};

use crate::canvas::{BoxBorder, BoxBorderStyle, Canvas};
use crate::da::DefaultAppearance;
use crate::encode::{EncodedPage, encode, encode_with};
use crate::fault::Fault;
use crate::geom::{Matrix, PageGeom, Rect};
use crate::interp::{Event, Forms, Kind, Opts, Wants, interpret, interpret_with};
use crate::limits::Limits;
use crate::path::Seg;

/// Built-in seed inputs exercising paths, text operators, Form XObjects, inline
/// images, and marked content for unit tests and `just fuzz gfx`.
pub const SEEDS: &[&[u8]] = &[
    b"",
    b"q 10 20 30 40 re f Q",
    b"BT /F1 12 Tf 10 20 Td (Hello) Tj 0 -14 TD (World) ' ET\xff<< /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >>",
    b"0 0 50 50 re W n /Fm1 Do\xff<< /XObject << /Fm1 1 0 R >> >>\xff<< /Subtype /Form /BBox [0 0 40 40] >>\nstream\n0 0 20 20 re S",
    b"BI /W 1 /H 1 /BPC 8 /CS /G ID x EI",
    b"/Span << /MCID 0 /ActualText (Hi) >> BDC 1 w 1 J 5 5 m h S EMC",
    b"BI /W 1 /$\x00\"\x00\x00\x00\x00\x80\x00\x00x EI",
    b"BI /SMask /None/W 1 / W* BI /W 1 /W n /Fm1 C\x01\x00\x00\x00\x00\x00\x00\"] I /CS /] ID x E\x01\x00\x00\x00\x00\x00\x00\x05KC 8 /CS /] ID x D x EIH;1 /BPKC 8 /CS /] ID x EI",
    b"BI /F /Fl ID x EI\n\n",
    b"/Helv 0 Tf 0.2 0.4 0.6 rg 0.5 G 0 0 0 1 k 1.5 Tc -2 Tw 90 Tz 14 TL 2 Tr 3 Ts",
    // A box border's numbers, as `check_box_border` reads them: the box
    // 0 0 100 50, a width of 2, the dash array [3 1], a hint of 0.5 0.25 1.
    b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xc8\x42\x00\x00\x48\x42\x00\x00\x00\x40\x00\x00\x40\x40\x00\x00\x80\x3f\x00\x00\x00\x3f\x00\x00\x80\x3e\x00\x00\x80\x3f",
    // A `/DA` with each fault `parse_with` tells but the unreadable one, and
    // text whose spaces the layout flags keep or drop.
    b"Tc /Helv Tf 9 Tr 0 0 rg 1000000000000000000000000000000000000000000. Ts",
    b"  ab cd  ef-gh  \n\tij -  kl\r\nmn",
    // Text Helvetica and Symbol draw between them, and a base and a mark
    // neither draws as one cluster.
    b"Gr\xc3\xbc\xc3\x9fe \xce\xb1\xce\xb2\xce\xb3 \xce\xa9 x\xe2\x88\x80y\n\xce\x94 \xe2\x80\x94 caf\xc3\xa9",
    b"caf\xc3\xa9 e\xcc\x81 \xce\xb1",
    // Eight letters Helvetica draws in eight comb cells, and past them a
    // letter only Symbol draws.
    b"abcdefgh\xce\xb1",
    b"1 Tw 2 Tc /Fm1 Do 5 Tw 6 Tc /Fm1 Do\xff<< /XObject << /Fm1 1 0 R >> /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >>\xff<< /Subtype /Form /BBox [0 0 80 80] >>\nstream\nBT /F1 10 Tf 1e39 0 (Hi) \" ET",
];

/// Runs `interpret`, `interpret_with`, `Wants::NONE`, and `encode` round-trip
/// checks over an arbitrary byte slice split into page content, resources, and
/// indirect objects/streams.
pub fn check(data: &[u8]) {
    let mut parts = data.split(|&b| b == 0xFF);
    let page_bytes = parts.next().unwrap_or_default();
    let res_bytes = parts.next().unwrap_or_default();

    check_default_appearance(page_bytes);
    check_box_border(data);

    let mut parse_limits = pdf_objects::Limits::DEFAULT;
    parse_limits.nesting = 16;
    parse_limits.values = 512;

    let resources = Parser::at(res_bytes, 0)
        .rules(Rules::Pdfium)
        .limits(parse_limits)
        .value(&mut ())
        .ok();

    let mut map: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut stream_bytes: BTreeMap<ObjectId, Vec<u8>> = BTreeMap::new();
    for (idx, chunk) in parts.take(16).enumerate() {
        let id = ObjectId::new((idx + 1) as u32, 0);
        if let Some((dict_part, raw_stream)) = split_once_bytes(chunk, b"\nstream\n") {
            if let Ok(obj) = Parser::at(dict_part, 0)
                .rules(Rules::Pdfium)
                .limits(parse_limits)
                .value(&mut ())
                && let Value::Dict(dict) = obj.into_value()
            {
                stream_bytes.insert(id, raw_stream.to_vec());
                let s = Stream::new(dict, raw_stream.to_vec());
                map.insert(id, Object::from(s));
            }
        } else if let Ok(obj) = Parser::at(chunk, 0)
            .rules(Rules::Pdfium)
            .limits(parse_limits)
            .value(&mut ())
        {
            map.insert(id, obj);
        }
    }

    let limits = Limits {
        form_depth: 6,
        states: 32,
        operators: 4_096,
        glyphs: 4_096,
        segments: 4_096,
        marked: 32,
        interpreted: 64 << 10,
        decompressed: 128 << 10,
        ..Limits::DEFAULT
    };

    let streams = WithStreams::new(&map, |id: ObjectId, cap: usize| {
        match stream_bytes.get(&id) {
            Some(bytes) if bytes.len() <= cap => Ok(bytes.clone()),
            Some(_) => Err(Unreadable::new(id, Why::Limit, "past budget")),
            None => Err(Unreadable::new(id, Why::Damaged, "missing stream")),
        }
    });
    let concat = Concat::new(&[page_bytes]);
    let fonts = Fonts::default();

    let mut events_before = Vec::new();
    let res_all = interpret(
        streams,
        &concat,
        resources.as_ref(),
        &limits,
        &fonts,
        Opts::default(),
        |ev| {
            if let Some(snap) = snapshot(&ev) {
                events_before.push(snap);
            }
            ControlFlow::Continue(())
        },
    );

    let mut faults = Vec::new();
    let res_sink = interpret_with(
        streams,
        &concat,
        resources.as_ref(),
        &limits,
        &fonts,
        Opts::default(),
        &mut faults,
        |_| ControlFlow::Continue(()),
    );
    assert_eq!(res_all, res_sink);
    for (_, fault) in &faults {
        let _ = format!("{fault:?}");
    }

    let res_none = interpret(
        streams,
        &concat,
        resources.as_ref(),
        &limits,
        &fonts,
        Opts {
            forms: Forms::Descend,
            wants: Wants::NONE,
        },
        |_| ControlFlow::Continue(()),
    );
    assert_eq!(res_all, res_none);

    #[cfg(feature = "text-layout")]
    fuzz_text_layout(streams, &concat, resources.as_ref(), &limits, &fonts, data);

    if res_all.is_ok() {
        let enc_plain = encode(streams, &concat, resources.as_ref(), &limits, &fonts);
        let mut enc_faults = Vec::new();
        let enc_sink = encode_with(
            streams,
            &concat,
            resources.as_ref(),
            &limits,
            &fonts,
            &mut enc_faults,
        );
        assert_eq!(enc_plain, enc_sink);
        for (_, fault) in &enc_faults {
            let _ = format!("{fault:?}");
        }

        if let Ok(EncodedPage {
            content: enc_page,
            forms: enc_forms,
        }) = enc_plain
        {
            let mut map2 = map.clone();
            let mut sb2 = stream_bytes.clone();
            for (fid, bytes) in enc_forms {
                let dict = map2
                    .get(&fid)
                    .and_then(Object::as_stream)
                    .map_or_else(Dict::new, |s| s.dict.clone());
                sb2.insert(fid, bytes.clone());
                map2.insert(fid, Object::from(Stream::new(dict, bytes)));
            }
            let streams2 = WithStreams::new(&map2, |id: ObjectId, cap: usize| match sb2.get(&id) {
                Some(bytes) if bytes.len() <= cap => Ok(bytes.clone()),
                Some(_) => Err(Unreadable::new(id, Why::Limit, "past budget")),
                None => Err(Unreadable::new(id, Why::Damaged, "missing stream")),
            });
            let concat2 = Concat::new(&[&enc_page]);
            let mut events_after = Vec::new();
            let again = interpret(
                streams2,
                &concat2,
                resources.as_ref(),
                &limits,
                &fonts,
                Opts::default(),
                |ev| {
                    if let Some(snap) = snapshot(&ev) {
                        events_after.push(snap);
                    }
                    ControlFlow::Continue(())
                },
            );
            if again.is_ok() {
                assert_eq!(events_before.len(), events_after.len());
                for (a, b) in events_before.iter().zip(&events_after) {
                    assert_eq!(a.tag, b.tag);
                    assert_eq!(a.nums.len(), b.nums.len());
                    for (&x, &y) in a.nums.iter().zip(&b.nums) {
                        assert!(x == y || (x.is_nan() && y.is_nan()));
                    }
                }
            }
        }
    }
}

/// A sink that wants no fault, and fails if it's told one.
struct Unwanted;

impl Sink<Fault> for Unwanted {
    fn fault(&mut self, _: Place, fault: Fault) {
        panic!("told {fault:?}, though it wants none");
    }

    fn wants(&self) -> bool {
        false
    }
}

/// Parses `da` as a `/DA` string, with and without a sink, and, where it
/// parses, holds the round trip: what `to_bytes` writes parses again, and
/// writes the same bytes again.
fn check_default_appearance(da: &[u8]) {
    // The default content limits both times: what's written holds no more
    // operands, nesting, or images than what was read, so no cap that took
    // `da` refuses it.
    let limits = pdf_content_streams::Limits::DEFAULT;
    let parsed = DefaultAppearance::parse(da, &limits);
    let mut faults = Vec::new();
    assert_eq!(
        DefaultAppearance::parse_with(da, &limits, &mut faults),
        parsed
    );
    assert_eq!(
        DefaultAppearance::parse_with(da, &limits, &mut Unwanted),
        parsed
    );
    for (i, (place, fault)) in faults.iter().enumerate() {
        let _ = format!("{fault:?}");
        assert_eq!(*place, Place::new(None, None));
        let last = i + 1 == faults.len();
        match fault {
            Fault::DefaultAppearanceUnreadable(error) => {
                assert!(last && parsed.is_none() && error.at <= da.len());
            }
            Fault::DefaultAppearanceOperand { at, .. }
            | Fault::DefaultAppearanceTextMode { at } => {
                assert!(*at < da.len());
            }
            Fault::DefaultAppearanceNoFont => {
                assert!(last && parsed.as_ref().is_some_and(|p| p.font_name.is_none()));
            }
            _ => panic!("{fault:?} is no /DA fault"),
        }
    }
    let told_last = |want: fn(&Fault) -> bool| faults.last().is_some_and(|(_, f)| want(f));
    let Some(parsed) = parsed else {
        assert!(told_last(|f| matches!(
            f,
            Fault::DefaultAppearanceUnreadable(_)
        )));
        return;
    };
    assert_eq!(
        parsed.font_name.is_none(),
        told_last(|f| matches!(f, Fault::DefaultAppearanceNoFont))
    );
    let _ = format!("{parsed:?}");
    let _ = parsed.size();
    #[cfg(feature = "text-layout")]
    let _ = parsed.text_box_style();

    let written = parsed.to_bytes();
    let again =
        DefaultAppearance::parse(&written, &limits).expect("a written /DA must parse again");
    // A content parser keeps the first 255 bytes of a word, so a font name
    // whose written form (its `/` and `#XX` escapes among it) is longer reads
    // back cut short. The bytes are held equal only where the name fits.
    let name_word = written
        .iter()
        .position(|&b| b == b' ')
        .unwrap_or(written.len());
    if parsed.font_name.is_none() || name_word <= 255 {
        assert_eq!(again.to_bytes(), written);
    }
}

/// Draws a border in each of the five styles from numbers read off `data`'s
/// raw bytes (any bit pattern: NaN, infinities, and subnormals among them) and
/// holds what `Canvas::box_border` promises of the stream.
fn check_box_border(data: &[u8]) {
    let mut numbers = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c));
    let mut next = || numbers.next().unwrap_or(0.0);
    // As given, not put in order: an inverted box is one too small to draw in.
    let rect = Rect {
        x0: next(),
        y0: next(),
        x1: next(),
        y1: next(),
    };
    let width = next();
    let dash = [next(), next()];
    let hint = Color::Rgb([next(), next(), next()]);
    let shades = [
        Color::Gray(next()),
        Color::Cmyk([next(), next(), next(), next()]),
    ];
    let usable = width > 0.0 && width.is_finite() && rect.is_finite();

    let store: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let streams = WithStreams::new(&store, |id: ObjectId, _cap: usize| {
        Err(Unreadable::new(id, Why::Damaged, "missing stream"))
    });
    for style in [
        BoxBorderStyle::Solid,
        BoxBorderStyle::Dashed,
        BoxBorderStyle::Beveled,
        BoxBorderStyle::Inset,
        BoxBorderStyle::Underline,
    ] {
        let border = BoxBorder::new(width, style).with_dash(&dash);
        let _ = format!("{border:?}");
        let inset = border.inset();
        assert!(inset >= 0.0);
        assert_eq!(inset == 0.0, !(width > 0.0 && width.is_finite()));

        let mut canvas = Canvas::new();
        canvas.box_border(rect, &border, Color::Gray(0.0), Some(hint));
        let (bytes, needs) = canvas.finish(&Reserved::default());
        assert!(needs.0.is_empty());

        // The bands alone: what follows the frame in the border's scope, in
        // a scope of their own.
        let mut bands = Canvas::new();
        bands.box_bands(rect, &border, Some(hint));
        let (bands, needs) = bands.finish(&Reserved::default());
        assert!(needs.0.is_empty());
        if bands.is_empty() {
            assert!(!bytes.windows(3).any(|w| w == b"\nf\n"), "bands not drawn");
        } else {
            let filled = bands
                .strip_prefix(b"q\n")
                .and_then(|b| b.strip_suffix(b"Q\n"))
                .expect("the bands are in a scope of their own");
            assert!(bytes.ends_with(&[filled, b"Q\n"].concat()));
            assert!(bytes.len() > bands.len());
        }
        check_bands_in(rect, &border, Some(hint), shades, &bands);

        if bytes.is_empty() {
            continue;
        }
        assert!(usable, "a border drawn for a width or box that can't be");
        let words = || bytes.split(u8::is_ascii_whitespace);
        assert!(bytes.starts_with(b"q\n") && bytes.ends_with(b"Q\n"));
        assert_eq!(words().filter(|w| w == b"q").count(), 1);
        assert_eq!(words().filter(|w| w == b"Q").count(), 1);

        let mut faults = Vec::new();
        let mut paths = 0usize;
        let summary = interpret_with(
            streams,
            &Concat::new(&[&bytes]),
            None,
            &Limits::DEFAULT,
            &Fonts::default(),
            Opts::default(),
            &mut faults,
            |ev| {
                if matches!(ev.kind, Kind::Path(_)) {
                    paths += 1;
                }
                ControlFlow::Continue(())
            },
        );
        assert!(summary.is_ok(), "a written border must interpret");
        assert!(faults.is_empty(), "a written border reads without a fault");
        assert!(paths <= 3);
    }
}

/// Holds `Canvas::box_bands_in` to `bands`, what `Canvas::box_bands` drew
/// for `border` in `rect` with `hint`: in the shades `box_bands` chooses,
/// the same bytes; in `shades`, cut from the input, nothing exactly where
/// `bands` is nothing, and else the same paths in a scope of their own.
fn check_bands_in(
    rect: Rect,
    border: &BoxBorder<'_>,
    hint: Option<Color>,
    shades: [Color; 2],
    bands: &[u8],
) {
    let (top_left, bottom_right) = crate::canvas::shades(border.style, hint);
    let mut own = Canvas::new();
    own.box_bands_in(rect, border, top_left, bottom_right);
    let (own, needs) = own.finish(&Reserved::default());
    assert!(needs.0.is_empty());
    assert_eq!(
        own, bands,
        "box_bands is box_bands_in in the style's shades"
    );

    let mut given = Canvas::new();
    given.box_bands_in(rect, border, shades[0], shades[1]);
    let (given, needs) = given.finish(&Reserved::default());
    assert!(needs.0.is_empty());
    assert_eq!(
        given.is_empty(),
        bands.is_empty(),
        "bands drawn where box_bands draws them"
    );
    // Every line but the two that set a fill color the same.
    let paths = |bytes: &[u8]| -> Vec<Vec<u8>> {
        bytes
            .split(|&b| b == b'\n')
            .filter(|line| {
                ![&b" g"[..], b" rg", b" k"]
                    .iter()
                    .any(|op| line.ends_with(op))
            })
            .map(<[u8]>::to_vec)
            .collect()
    };
    assert_eq!(
        paths(&given),
        paths(bands),
        "the same bands in other shades"
    );
}

fn split_once_bytes<'a>(haystack: &'a [u8], needle: &[u8]) -> Option<(&'a [u8], &'a [u8])> {
    let pos = haystack.windows(needle.len()).position(|w| w == needle)?;
    Some((&haystack[..pos], &haystack[pos + needle.len()..]))
}

#[cfg(feature = "text-layout")]
fn fuzz_text_layout<'a, S>(
    streams: S,
    concat: &Concat<'_>,
    resources: Option<&'a Object>,
    limits: &Limits,
    fonts: &Fonts,
    raw_data: &[u8],
) where
    S: Resolve<'a> + Streams + Copy,
{
    use pdf_fonts::Std14;

    use crate::geom::Point;
    use crate::layout::{SearchOpts, TextPage, TextPageOpts};
    use crate::wrap::{TextAlign, TextBoxStyle, WrapMode, layout_text};

    let r1 = TextPage::extract(
        streams,
        concat,
        resources,
        limits,
        fonts,
        TextPageOpts::default(),
    );
    let mut faults = Vec::new();
    let r2 = TextPage::extract_with(
        streams,
        concat,
        resources,
        limits,
        fonts,
        TextPageOpts::default(),
        &mut faults,
    );
    assert_eq!(r1, r2);

    if let Ok(tp) = r1 {
        let txt = tp.text();
        for c in tp.chars() {
            assert_eq!(&txt[c.byte_range()], c.ch.encode_utf8(&mut [0u8; 4]));
        }
        let _ = tp.text_dehyphenated();
        let _ = tp.selection_quads(0..tp.len());
        let _ = tp.selection_rects(0..tp.len());
        let _ = tp.char_index_at(Point::new(50.0, 50.0), 8.0, 8.0);
        let _ = tp.text_in_rect(Rect::new(0.0, 0.0, 200.0, 200.0));
        let _ = tp.search("a", SearchOpts::default());
        let _ = format!("{tp:?}");
    }

    if let Ok(sample) = std::str::from_utf8(&raw_data[..raw_data.len().min(96)]) {
        let style = TextBoxStyle {
            size: None,
            align: TextAlign::Justify,
            wrap: WrapMode::Word,
            ..TextBoxStyle::default()
        };
        let _ = layout_text(
            &Std14::Helvetica,
            sample,
            Rect::new(0.0, 0.0, 120.0, 60.0),
            &style,
        );
        check_layout_flags(sample);
        check_fallback(sample);
    }
}

/// Lays `text` out in Helvetica with Symbol after it, a character and a
/// grapheme cluster at a time, in lines and in comb cells, and holds what
/// the runs promise; draws each layout and interprets it.
#[cfg(feature = "text-layout")]
fn check_fallback(text: &str) {
    use pdf_content_streams::Res;
    use pdf_fonts::Std14;
    use unicode_segmentation::UnicodeSegmentation;

    use crate::wrap::{
        FontSet, LaidOutText, TextBoxStyle, UnencodableChar, WithFallback, face_for, layout_text,
    };

    let helvetica = Res::dict(Std14::Helvetica.dict()).expect("a dictionary of names");
    let symbol = Res::dict(Std14::Symbol.dict()).expect("a dictionary of names");
    let set = WithFallback::new(&Std14::Helvetica, &Std14::Symbol, symbol.clone());
    assert_eq!(set.faces(), 2);
    let rect = Rect::new(0.0, 0.0, 120.0, 60.0);
    // Each paragraph's units, at their offsets in the text.
    let units = |clusters: bool| -> Vec<Vec<(usize, &str)>> {
        let mut start = 0;
        let mut paragraphs = Vec::new();
        for para in text.split_inclusive(['\r', '\n']) {
            let body = para.trim_end_matches(['\r', '\n']);
            let at = start;
            paragraphs.push(match clusters {
                true => body
                    .grapheme_indices(true)
                    .map(|(i, u)| (at + i, u))
                    .collect(),
                false => (body.char_indices())
                    .map(|(i, c)| (at + i, &body[i..i + c.len_utf8()]))
                    .collect(),
            });
            start += para.len();
        }
        paragraphs
    };
    let one_char_clusters = (units(true).iter().flatten()).all(|(_, u)| u.chars().nth(1).is_none());

    for comb in [None, Some(8)] {
        let mut laid: Vec<Option<LaidOutText>> = Vec::new();
        for clusters in [false, true] {
            let style = TextBoxStyle {
                size: Some(9.0),
                comb,
                clusters,
                ..TextBoxStyle::default()
            };
            let all = units(clusters);
            let faces: Option<Vec<usize>> = (all.iter().flatten())
                .map(|(_, unit)| face_for(&set, unit).ok())
                .collect();
            let result = layout_text(&set, text, rect, &style);
            // Refused exactly where a unit has no face.
            assert_eq!(result.is_ok(), faces.is_some(), "{clusters}");
            let Ok(out) = result else {
                laid.push(None);
                continue;
            };
            let faces = faces.unwrap_or_default();
            for line in &out.lines {
                assert_eq!(line.cell_faces.len(), line.cells.len());
                assert!((line.ascent >= 0.0 && line.descent <= 0.0) || !line.ascent.is_finite());
                if !line.cells.is_empty() {
                    // Each cell in its unit's face, the units in order.
                    assert_eq!(&line.cell_faces[..], &faces[..line.cells.len()]);
                    continue;
                }
                let mut end = 0;
                let mut last_face = None;
                for run in &line.runs {
                    assert_eq!(run.codes.start, end, "runs in order");
                    assert!(run.codes.end > run.codes.start && run.face < 2);
                    assert_ne!(Some(run.face), last_face, "a face a run");
                    assert!(run.x.is_finite());
                    end = run.codes.end;
                    last_face = Some(run.face);
                    let drawn = (all.iter().flatten())
                        .filter(|(at, _)| run.text.contains(at))
                        .map(|(_, unit)| face_for(&set, unit));
                    assert!(drawn.into_iter().all(|f| f == Ok(run.face)));
                }
                assert_eq!(end, line.codes.len(), "the runs are the codes");
            }
            // Helvetica alone: where it has every unit, those past a comb's
            // cells among them, its own layout; else refused at the first
            // unit it lacks, as `face_for` refuses it there.
            let alone = layout_text(&Std14::Helvetica, text, rect, &style);
            let other = (all.iter().flatten()).zip(&faces).find(|(_, f)| **f != 0);
            match other {
                None => assert_eq!(Ok(&out), alone.as_ref()),
                Some(((at, unit), _)) => {
                    let lacks = face_for(&Std14::Helvetica, unit).expect_err("Helvetica lacks it");
                    let at = at + lacks.at;
                    let character = lacks.character;
                    assert_eq!(alone, Err(UnencodableChar { at, character }));
                }
            }

            // Drawn, each face under its resource: a glyph a code.
            let mut canvas = Canvas::new();
            let drawn = canvas.draw_layout_in(&[helvetica.clone(), symbol.clone()], &out);
            assert!(drawn, "a layout drawn as made, a resource a face");
            let (bytes, needs) = canvas.finish(&Reserved::default());
            let mut fonts = Dict::new();
            for need in &needs.0 {
                let font = match need.res == helvetica {
                    true => Std14::Helvetica,
                    false => Std14::Symbol,
                };
                fonts.set(need.name.clone(), font.dict());
            }
            let resources = Object::from(pdf_objects::dict! { "Font" => fonts });
            let objects = BTreeMap::<ObjectId, Object>::new();
            let streams = WithStreams::new(&objects, |id, _| {
                Err(Unreadable::new(id, Why::Damaged, "no streams"))
            });
            let codes: usize = (out.lines.iter())
                .map(|l| l.codes.len() + l.cells.iter().map(|(_, c)| c.len()).sum::<usize>())
                .sum();
            let summary = interpret(
                streams,
                &Concat::new(&[&bytes]),
                Some(&resources),
                &Limits::DEFAULT,
                &Fonts::default(),
                Opts::default(),
                |_| ControlFlow::Continue(()),
            )
            .expect("a layout drawn interprets");
            assert_eq!(summary.glyphs, codes);
            laid.push(Some(out));
        }
        // By clusters only where by characters; the same where every
        // cluster is a character.
        assert!(laid[1].is_none() || laid[0].is_some());
        if one_char_clusters {
            assert_eq!(laid[0], laid[1]);
        }
    }
}

/// Lays `text` out with each of `TextBoxStyle`'s two flags on and off, in
/// Helvetica, whose heights the floors don't move, and Courier, whose ascent
/// they do, and holds what the flags promise.
#[cfg(feature = "text-layout")]
fn check_layout_flags(text: &str) {
    use pdf_fonts::Std14;

    use crate::wrap::{LaidOutText, TextAlign, TextBoxStyle, WrapMode, layout_text};

    let rect = Rect::new(0.0, 0.0, 120.0, 60.0);
    let unbroken: String = text.chars().filter(|c| !matches!(c, '\r' | '\n')).collect();
    let holds = |laid: &LaidOutText, style: &TextBoxStyle| {
        assert!(laid.font_size.is_finite() && laid.font_size > 0.0);
        assert!(laid.line_height.is_finite() && laid.line_height > 0.0);
        let mut after = 0;
        for line in &laid.lines {
            let shown = text
                .get(line.text_range.clone())
                .expect("a line's range is the text's");
            assert!(line.text_range.start >= after, "lines in order");
            after = line.text_range.end;
            assert!(!shown.contains(['\r', '\n']));
            assert_eq!(
                line.codes.len(),
                shown.chars().count(),
                "a code a character"
            );
            if style.align != TextAlign::Justify && shown.chars().nth(1).is_some() {
                assert!(
                    line.width <= laid.bounds.width() + 1e-3,
                    "a line fits its box"
                );
            }
        }
        if style.line_end_spaces {
            let kept: String = laid
                .lines
                .iter()
                .map(|l| &text[l.text_range.clone()])
                .collect();
            assert_eq!(kept, unbroken, "every character on a line");
        }
    };
    for font in [Std14::Helvetica, Std14::Courier] {
        for (wrap, align, size) in [
            (WrapMode::Word, TextAlign::Left, None),
            (WrapMode::Word, TextAlign::Justify, Some(9.0)),
            (WrapMode::Char, TextAlign::Right, Some(11.0)),
        ] {
            let mut laid = Vec::new();
            for keep in [false, true] {
                for given in [false, true] {
                    let style = TextBoxStyle {
                        size,
                        align,
                        wrap,
                        line_end_spaces: keep,
                        font_heights_as_given: given,
                        ..TextBoxStyle::default()
                    };
                    let result = layout_text(&font, text, rect, &style);
                    if let Ok(out) = &result {
                        holds(out, &style);
                    }
                    laid.push(result);
                }
            }
            // The flags move no character's code: the same error, or none.
            let errors: Vec<_> = laid.iter().map(|r| r.as_ref().err()).collect();
            assert!(errors.iter().all(|e| *e == errors[0]));
            if font == Std14::Helvetica {
                assert_eq!(laid[0], laid[1]);
                assert_eq!(laid[2], laid[3]);
            }
        }
    }
}

#[derive(Debug)]
struct Snap {
    tag: String,
    nums: Vec<f32>,
}

fn push_matrix(nums: &mut Vec<f32>, m: Matrix) {
    nums.extend(m.to_array());
}

fn push_rect(nums: &mut Vec<f32>, r: Rect) {
    nums.extend([r.x0, r.y0, r.x1, r.y1]);
}

fn snapshot(ev: &Event<'_>) -> Option<Snap> {
    let mut nums = Vec::new();
    let tag = match &ev.kind {
        Kind::Op => return None,
        Kind::Text(show) => {
            push_matrix(&mut nums, show.start);
            nums.push(show.size);
            let mut count = 0usize;
            for g in show.glyphs() {
                count += 1;
                nums.extend([g.origin.x, g.origin.y, g.advance]);
                for p in g.quad.0 {
                    nums.extend([p.x, p.y]);
                }
            }
            format!(
                "text:{:?}:{}:{}:{count}",
                show.string, show.stock, show.mode as u8
            )
        }
        Kind::FormBegin(u) => {
            push_matrix(&mut nums, u.matrix);
            if let Some(b) = u.bbox {
                push_rect(&mut nums, b);
            }
            format!("form_begin:{}:{}", u.form, u.id.depth())
        }
        Kind::FormEnd(u) => format!("form_end:{}:{}", u.form, u.id.depth()),
        Kind::FormRefused { form, why } => format!("form_refused:{form}:{why:?}"),
        Kind::Path(p) => {
            push_rect(&mut nums, p.bounds);
            let mut kinds = String::new();
            for s in p.segments {
                match *s {
                    Seg::Move(pt) => {
                        kinds.push('m');
                        nums.extend([pt.x, pt.y]);
                    }
                    Seg::Line(pt) => {
                        kinds.push('l');
                        nums.extend([pt.x, pt.y]);
                    }
                    Seg::Curve(a, b, c) => {
                        kinds.push('c');
                        nums.extend([a.x, a.y, b.x, b.y, c.x, c.y]);
                    }
                    Seg::Close => kinds.push('h'),
                }
            }
            format!("path:{kinds}:{:?}:{:?}", p.paint, p.clip)
        }
        Kind::Image(img) => {
            for p in img.quad.0 {
                nums.extend([p.x, p.y]);
            }
            format!(
                "image:{:?}:{:?}:{:?}:{}",
                img.width, img.height, img.bits_per_component, img.stencil
            )
        }
        Kind::Shading(sh) => {
            if let Some(b) = sh.bounds {
                push_rect(&mut nums, b);
            }
            format!("shading:{:?}:{}", sh.shading, sh.bounds.is_some())
        }
        Kind::Ignored(id) => format!("ignored:{id}"),
        Kind::MarkedBegin(m) => format!("mb:{:?}:{:?}", m.tag, m.mcid),
        Kind::MarkedEnd(m) => format!("me:{:?}:{:?}", m.tag, m.mcid),
        Kind::MarkedPoint(m) => format!("mp:{:?}:{:?}", m.tag, m.mcid),
    };
    push_matrix(&mut nums, ev.state.ctm);
    if let Some(cb) = ev.state.clip_bounds() {
        push_rect(&mut nums, cb);
    }
    Some(Snap { tag, nums })
}

/// Runs `PageGeom::from_page`, `interpret`, `interpret_with`, and `encode` over
/// the first pages of a parsed PDF trailer and object/stream store.
pub fn over<'a, S>(streams: S, trailer: &'a Dict)
where
    S: Resolve<'a> + Streams + Copy,
{
    let Ok(Some(cat)) = pdf_semantics::catalog(streams, trailer) else {
        return;
    };
    let mut sem_limits = pdf_semantics::Limits::DEFAULT;
    sem_limits.tree_depth = 16;
    sem_limits.pages = 16;
    let Ok(table) = pages::pages(streams, cat, Rules::Pdfium, &sem_limits, &mut ()) else {
        return;
    };
    let limits = Limits {
        form_depth: 6,
        states: 32,
        operators: 4_096,
        glyphs: 4_096,
        segments: 4_096,
        marked: 32,
        interpreted: 64 << 10,
        decompressed: 128 << 10,
        ..Limits::DEFAULT
    };
    let fonts = Fonts::default();
    for page in table.iter().take(4) {
        let geom = PageGeom::from_page(page);
        let _ = geom.to_view();
        let _ = geom.from_view();
        let Some(page_dict) = page.dict else {
            continue;
        };
        let mut join = Join::new(&limits.content);
        if let Some(contents) = page_dict.get(b"Contents")
            && join.push_contents(streams, contents).is_err()
        {
            continue;
        }
        let content = join.finish();
        let resources = page.attribute(Inheritable::Resources).map(|r| r.written);
        let r1 = interpret(
            streams,
            &content,
            resources,
            &limits,
            &fonts,
            Opts::default(),
            |_| ControlFlow::Continue(()),
        );
        let mut faults = Vec::new();
        let r2 = interpret_with(
            streams,
            &content,
            resources,
            &limits,
            &fonts,
            Opts::default(),
            &mut faults,
            |_| ControlFlow::Continue(()),
        );
        assert_eq!(r1, r2);
        let e1 = encode(streams, &content, resources, &limits, &fonts);
        let mut efaults = Vec::new();
        let e2 = encode_with(streams, &content, resources, &limits, &fonts, &mut efaults);
        assert_eq!(e1, e2);
    }
}

#[cfg(test)]
mod tests {
    use super::SEEDS;

    /// `PDF_GFX_FUZZ_SEEDS` names a folder: writes the seed inputs there for
    /// `just fuzz gfx` to begin from.
    #[test]
    #[ignore = "needs PDF_GFX_FUZZ_SEEDS, a folder to write the seeds to"]
    fn the_seeds_are_written() {
        let dir =
            std::env::var_os("PDF_GFX_FUZZ_SEEDS").expect("PDF_GFX_FUZZ_SEEDS names a folder");
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (i, seed) in SEEDS.iter().enumerate() {
            std::fs::write(dir.join(format!("seed-{i}")), seed).unwrap();
        }
    }
}
