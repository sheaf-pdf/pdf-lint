//! The content writer: what each call writes, and what that reads back
//! as; names bound at the end, clear of what the resources bind; and the
//! same bytes from the same calls on any thread.

use std::collections::BTreeMap;

use pdf_objects::{Data, Dict, Object, ObjectId, Span, Str, Stream, Value, dict};

use super::*;
use crate::fuzz::read_as_written as close;
use crate::{Limits, Operation, ResBody, ResCx, Tok, object, operations, tokens};

/// The text of what `w` wrote, with no resources reserved or given.
fn written(w: ContentWriter) -> (String, ResourceNeeds) {
    let (bytes, needs) = w.finish(&Reserved::default());
    (String::from_utf8(bytes).unwrap(), needs)
}

fn list(subtype: &str) -> Dict {
    dict! { "Type" => "Pagination", "Subtype" => subtype }
}

/// A font's resource, standing in for one a font gives.
fn font(base: &str) -> Res {
    Res::dict(dict! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => base })
}

/// A name `Sh` and six base32 characters, counted up by one.
fn next(name: &[u8]) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let n = name[2..].iter().fold(0u32, |n, c| {
        n * 32 + ALPHABET.iter().position(|a| a == c).unwrap() as u32
    });
    let n = (n + 1) & ((1 << 30) - 1);
    let mut out = b"Sh".to_vec();
    out.extend((0..6).rev().map(|i| ALPHABET[(n >> (5 * i)) as usize & 31]));
    out
}

/// The names `resources` binds, its references resolved in `objects`.
fn reserved(objects: &BTreeMap<ObjectId, Object>, resources: &Object) -> Reserved {
    Reserved::read(&|id: ObjectId| objects.get(&id), Some(resources))
}

/// `content`'s operations, every one read.
fn read(content: &[u8]) -> Vec<Operation<'_>> {
    operations(content, &ResCx::none(), &Limits::DEFAULT)
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn each_call_writes_its_operator() {
    let mut w = ContentWriter::new();
    w.save();
    w.transform([2.0, 0.0, 0.0, 1.0, -0.00001, 1.0 / 3.0]);
    w.fill_color(Color::Gray(0.5));
    w.stroke_color(Color::Rgb([1.0, 0.0, 0.25]));
    w.fill_color(Color::Cmyk([0.0, 0.1, 0.2, 1.0]));
    w.stroke_color(Color::rgb8([255, 0, 51]));
    w.line_width(0.75);
    w.dash(&[4.0, 3.0], 0.0);
    w.dash(&[], 0.0);
    w.line_cap(LineCap::Round);
    w.line_join(LineJoin::Bevel);
    w.move_to(1.0, 2.0);
    w.line_to(612.0, 1e7);
    w.curve_to(1.0, 2.0, 3.0, 4.0, 5.0, f32::NAN);
    w.rect(0.0, 0.0, 10.0, -0.0);
    w.close();
    for paint in [
        Paint::Fill,
        Paint::FillEvenOdd,
        Paint::Stroke,
        Paint::CloseStroke,
        Paint::FillStroke,
        Paint::FillStrokeEvenOdd,
        Paint::CloseFillStroke,
        Paint::CloseFillStrokeEvenOdd,
    ] {
        w.paint(paint);
    }
    w.clip(FillRule::NonZero);
    w.clip(FillRule::EvenOdd);
    w.paint(Paint::None);
    w.begin_text();
    w.text_mode(TextMode::Invisible);
    w.horizontal_scale(160.0);
    w.leading(12.5);
    w.text_move(72.0, 700.0);
    w.text_matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    w.next_line();
    w.end_text();
    w.restore();
    w.end_marked();
    let (text, needs) = written(w);
    assert_eq!(
        text,
        "q\n2 0 0 1 0 0.3333 cm\n0.5 g\n1 0 0.25 RG\n0 0.1 0.2 1 k\n1 0 0.2 RG\n\
         0.75 w\n[4 3] 0 d\n[] 0 d\n1 J\n2 j\n1 2 m\n612 10000000 l\n1 2 3 4 5 0 c\n\
         0 0 10 0 re\nh\nf\nf*\nS\ns\nB\nB*\nb\nb*\nW\nW*\nn\n\
         BT\n3 Tr\n160 Tz\n12.5 TL\n72 700 Td\n1 0 0 1 0 0 Tm\nT*\nET\nQ\nEMC\n"
    );
    assert_eq!(needs, ResourceNeeds::default());
}

/// Each mode, cap and join is written as its number, and a mode read back
/// from it.
#[test]
fn modes_caps_and_joins_are_their_numbers() {
    let modes = [
        TextMode::Fill,
        TextMode::Stroke,
        TextMode::FillStroke,
        TextMode::Invisible,
        TextMode::FillClip,
        TextMode::StrokeClip,
        TextMode::FillStrokeClip,
        TextMode::Clip,
    ];
    let mut w = ContentWriter::new();
    for (n, mode) in modes.into_iter().enumerate() {
        w.text_mode(mode);
        assert_eq!(TextMode::of(n as i32), Some(mode));
        assert_eq!(mode.clips(), n >= 4);
    }
    assert_eq!((TextMode::of(-1), TextMode::of(8)), (None, None));
    for cap in [LineCap::Butt, LineCap::Round, LineCap::Square] {
        w.line_cap(cap);
    }
    for join in [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel] {
        w.line_join(join);
    }
    let (text, _) = written(w);
    assert_eq!(
        text,
        "0 Tr\n1 Tr\n2 Tr\n3 Tr\n4 Tr\n5 Tr\n6 Tr\n7 Tr\n0 J\n1 J\n2 J\n0 j\n1 j\n2 j\n"
    );
}

/// Codes shown in a font, the font named: literal and escaped, or, with
/// a control code among them, in hex.
#[test]
fn text_is_shown_as_its_codes() {
    let (helvetica, wide) = (font("Helvetica"), font("Wide"));
    let mut w = ContentWriter::new();
    w.font(helvetica.clone(), 9.0);
    w.show(b"(a\\b) \xe9");
    w.save();
    w.font(wide.clone(), 10.0);
    w.show(b"\x00H\x00i");
    w.restore();
    w.show(b"\xe9");
    w.show(b"");
    w.show(b"a\rb");
    w.show(b"\x7f~");
    let (bytes, needs) = w.finish(&Reserved::default());
    let [h, g] = &needs.0[..] else {
        panic!("{needs:?}")
    };
    let mut want = format!("/{} 9 Tf\n", String::from_utf8_lossy(&h.name)).into_bytes();
    want.extend_from_slice(b"(\\(a\\\\b\\) \xe9) Tj\nq\n");
    want.extend(format!("/{} 10 Tf\n", String::from_utf8_lossy(&g.name)).bytes());
    want.extend_from_slice(b"<00480069> Tj\nQ\n(\xe9) Tj\n() Tj\n<610D62> Tj\n<7F7E> Tj\n");
    assert_eq!(bytes, want, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!((h.kind, &h.res), (ResKind::Font, &helvetica));
    assert_eq!((g.kind, &g.res), (ResKind::Font, &wide));
}

/// An operator content read gives is written as it is, its operands
/// before it, a line of its own, an inline image whole; it names no
/// resource, and what it does isn't followed.
#[test]
fn an_operator_read_is_written_as_it_is() {
    let mut w = ContentWriter::new();
    w.font(font("Helvetica"), 9.0);
    w.operation([&b"/F1"[..], b"12"], b"Tf");
    w.operation([&b"[(a) -250 <62>]"[..]], b"TJ");
    w.operation([], b"BI /W 1 /H 1 /CS /G /BPC 8 ID \xff EI");
    w.operation([], b"Q");
    w.show(b"\xe9");
    let (bytes, needs) = w.finish(&Reserved::default());
    let [helvetica] = &needs.0[..] else {
        panic!("{needs:?}")
    };
    let mut want = format!("/{} 9 Tf\n", String::from_utf8_lossy(&helvetica.name)).into_bytes();
    want.extend_from_slice(
        b"/F1 12 Tf\n[(a) -250 <62>] TJ\nBI /W 1 /H 1 /CS /G /BPC 8 ID \xff EI\n",
    );
    want.extend_from_slice(b"Q\n(\xe9) Tj\n");
    assert_eq!(bytes, want, "{}", String::from_utf8_lossy(&bytes));
}

/// Content read, each operation written again, reads as the same
/// operations: an inline image, a `BI` that begins none and the content
/// after it, nested arrays as PDFium ends them, and operands at the end,
/// which aren't written.
#[test]
fn content_written_again_reads_as_it_did() {
    let content: &[u8] = b"q 1 0 0 1 5 5 cm /F1 12 Tf [(a)-250(b)] TJ\n\
        BI /W 2 /H 1 /BPC 8 /CS /G ID xy EI Q\n\
        BI /A BMC EMC /W 1 [[1] 2] 3 d0 % a comment\n(x\\)y)Tj 7";
    let res = ResCx::none();
    let ops = read(content);
    let mut w = ContentWriter::new();
    for op in &ops {
        let operands = op.operands().map(|o| o.bytes());
        match op.inline_image() {
            Some(image) => w.operation(operands, &content[image.at()]),
            None => w.operation(operands, op.name()),
        }
    }
    let (again, needs) = w.finish(&Reserved::default());
    assert!(needs.0.is_empty());
    let shown = String::from_utf8_lossy(&again);
    /// `s`'s tokens but its inline images, whose data's ranges are its
    /// own.
    fn kept<'s>(s: &'s [u8], res: &ResCx) -> Vec<Tok<'s>> {
        tokens(s, res, &Limits::DEFAULT)
            .map(|t| t.unwrap().1)
            .filter(|t| !matches!(t, Tok::InlineImage(_)))
            .collect()
    }
    let mut before = kept(content, &res);
    assert_eq!(
        before.pop(),
        Some(Tok::Operand(b"7")),
        "operands at the end"
    );
    assert_eq!(kept(&again, &res), before, "{shown}");
    let names =
        |ops: &[Operation]| -> Vec<Vec<u8>> { ops.iter().map(|op| op.name().to_vec()).collect() };
    let read_again = read(&again);
    assert_eq!(names(&read_again), names(&ops), "{shown}");
    let image = |ops: &[Operation], s: &[u8]| {
        let image = ops[4].inline_image().unwrap();
        (s[image.at()].to_vec(), s[image.data()].to_vec())
    };
    assert_eq!(image(&read_again, &again), image(&ops, content));
    assert!(
        read_again[6].inline_image().is_none(),
        "a BI that begins none"
    );
}

/// An inline image PDFium keeps no stream of may read otherwise written
/// again: an ASCIIHex image whose first `>` is in a comment after it, with
/// no `EI` after that `>`, ends at its first `EI`, and a rectangle after it
/// is filled; written again, the comment gone, its first `>` is the one
/// before a later `EI`, and it runs on to that, over the rectangle.
#[test]
fn an_image_read_as_missing_may_read_otherwise_written_again() {
    let content: &[u8] =
        b"BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 41 EI q % > (\n Q 0 0 9 9 re f\n> EI";
    let mut w = ContentWriter::new();
    let ops = read(content);
    for op in &ops {
        let operands = op.operands().map(|o| o.bytes());
        match op.inline_image() {
            Some(image) => w.operation(operands, &content[image.at()]),
            None => w.operation(operands, op.name()),
        }
    }
    let (again, _) = w.finish(&Reserved::default());
    let names =
        |ops: &[Operation]| -> Vec<Vec<u8>> { ops.iter().map(|op| op.name().to_vec()).collect() };
    let image = ops[0].inline_image().unwrap();
    assert_eq!(image.stream(), crate::ImageStream::Missing);
    assert_eq!(names(&ops), [&b"BI"[..], b"q", b"Q", b"re", b"f", b"EI"]);
    let read_again = read(&again);
    assert_eq!(names(&read_again), [b"BI"]);
    assert!(
        read_again[0]
            .inline_image()
            .is_some_and(|i| i.at() == (0..again.len() - 1))
    );
}

/// The content's last operation may read otherwise written again where its
/// reading looked past its token: Flate data that's a zlib header asking
/// for a preset dictionary, `x` and the space before `EI`, the four bytes
/// that name the dictionary past `EI`. Where they're there, zlib, and
/// PDFium on it, stops for the dictionary, having counted none of the
/// data, and PDFium keeps the data to `EI`; written again, a line's end
/// alone after it, they aren't, zlib reads on to the end, and PDFium keeps
/// no stream. An operation after the image, written again, writes enough
/// after it. The fuzz body's round trip passes over the first, and not a
/// last image that reads alike.
#[test]
fn flate_data_asking_for_a_dictionary_past_its_ei_may_read_otherwise_written_again() {
    use crate::ImageStream;
    use crate::fuzz::written_again;
    let again = |content: &[u8]| {
        let mut w = ContentWriter::new();
        for op in read(content) {
            let operands = op.operands().map(|o| o.bytes());
            match op.inline_image() {
                Some(image) => w.operation(operands, &content[image.at()]),
                None => w.operation(operands, op.name()),
            }
        }
        w.finish(&Reserved::default()).0
    };
    let stream = |s: &[u8]| read(s)[0].inline_image().unwrap().stream();
    let res = ResCx::none();
    let last: &[u8] = b"BI /F /Fl /L 6 ID x EI\n\n";
    let last_again = again(last);
    assert_eq!(last_again, b"BI /F /Fl /L 6 ID x EI\n");
    assert_eq!(stream(last), ImageStream::Data(18..19));
    assert_eq!(stream(&last_again), ImageStream::Missing);
    assert!(!written_again(last, &res, &Limits::DEFAULT));
    let followed: &[u8] = b"BI /F /Fl /L 6 ID x EI q Q";
    let followed_again = again(followed);
    assert_eq!(followed_again, b"BI /F /Fl /L 6 ID x EI\nq\nQ\n");
    for s in [followed, &followed_again] {
        assert_eq!(stream(s), ImageStream::Data(18..19));
    }
    assert!(written_again(followed, &res, &Limits::DEFAULT));
    let unfiltered: &[u8] = b"BI /W 2 /H 1 /BPC 8 /CS /G ID xy EI % a comment";
    assert!(written_again(unfiltered, &res, &Limits::DEFAULT));
}

/// Where a reading decodes CCITTFax data to find its end, as PDFium does,
/// the decoder reads on past the image's `EI`: fax data whose decoder fails
/// in what follows `EI`, its end the stand-in's, is kept as a stream;
/// written again, other white-space after it, it decodes through it to past
/// `EI`, and PDFium keeps none. By the stand-in's reading, it's the same.
#[cfg(feature = "image-ends")]
#[test]
fn fax_data_decoded_past_its_ei_may_read_otherwise_written_again() {
    use crate::{CodedEnds, ImageStream};
    let content: &[u8] =
        b"q BI /F /CCF /DP <</K -1 /Columns 16>> /W 16 /H 4 ID \x97\nEI Q\n\x8ev\xee\x08!\x80";
    let again: &[u8] =
        b"q\nBI /F /CCF /DP <</K -1 /Columns 16>> /W 16 /H 4 ID \x97\nEI\nQ\n\x8ev\xee\x08!\n";
    let mut w = ContentWriter::new();
    for op in read(content) {
        let operands = op.operands().map(|o| o.bytes());
        match op.inline_image() {
            Some(image) => w.operation(operands, &content[image.at()]),
            None => w.operation(operands, op.name()),
        }
    }
    assert_eq!(w.finish(&Reserved::default()).0, again);
    let stream = |s: &[u8], coded_ends| {
        let limits = Limits {
            coded_ends,
            ..Limits::DEFAULT
        };
        let ops: Vec<_> = operations(s, &ResCx::none(), &limits)
            .collect::<Result<_, _>>()
            .unwrap();
        let image = ops[1].inline_image().unwrap();
        (s[image.at()].to_vec(), image.stream())
    };
    let token = b"BI /F /CCF /DP <</K -1 /Columns 16>> /W 16 /H 4 ID \x97\nEI".to_vec();
    for s in [content, again] {
        assert_eq!(
            stream(s, CodedEnds::StandIn),
            (token.clone(), ImageStream::Data(53..54))
        );
    }
    assert_eq!(
        stream(content, CodedEnds::Decoded),
        (token.clone(), ImageStream::Data(53..54))
    );
    assert_eq!(
        stream(again, CodedEnds::Decoded),
        (token, ImageStream::Missing)
    );
}

/// Property lists are named, never inline; an ExtGState is a resource.
#[test]
fn marked_content_names_its_list() {
    let mut w = ContentWriter::new();
    w.begin_marked("Artifact", list("Header"));
    w.gstate(GState {
        fill_alpha: Some(0.5),
        stroke_alpha: Some(2.0),
        blend: Some(Blend::Multiply),
    });
    w.end_marked();
    w.begin_marked("A b/c", list("Header"));
    w.end_marked();
    w.begin_tagged("Span");
    w.end_marked();
    let (text, needs) = written(w);
    let [props, gs] = &needs.0[..] else {
        panic!("{needs:?}")
    };
    let (p, g) = (
        String::from_utf8_lossy(&props.name),
        String::from_utf8_lossy(&gs.name),
    );
    assert_eq!(
        text,
        format!("/Artifact /{p} BDC\n/{g} gs\nEMC\n/A#20b#2Fc /{p} BDC\nEMC\n/Span BMC\nEMC\n")
    );
    assert_eq!(
        (props.kind, &props.res),
        (ResKind::Properties, &Res::dict(list("Header")))
    );
    assert_eq!(
        (gs.kind, &gs.res),
        (
            ResKind::ExtGState,
            &Res::dict(dict! {
                "Type" => "ExtGState", "ca" => 0.5, "CA" => 1.0, "BM" => "Multiply",
            })
        )
    );
    assert!(!text.contains("<<"));
}

/// An alpha is clamped from 0 to 1, and one that isn't finite is 0, so
/// the ExtGState is one pdf-objects' writer writes; each blend mode is
/// its name.
#[test]
fn a_gstate_is_written_as_it_sets() {
    let gs = |state: GState| {
        let mut w = ContentWriter::new();
        w.gstate(state);
        written(w).1.0.remove(0).res
    };
    let alphas = |fill: f32, stroke: f32| GState {
        fill_alpha: Some(fill),
        stroke_alpha: Some(stroke),
        blend: None,
    };
    let dict = |ca: f64, stroke: f64| {
        Res::dict(dict! { "Type" => "ExtGState", "ca" => ca, "CA" => stroke })
    };
    for (state, want) in [
        (alphas(f32::NAN, f32::INFINITY), dict(0.0, 0.0)),
        (alphas(f32::NEG_INFINITY, -f32::NAN), dict(0.0, 0.0)),
        (alphas(-0.5, 1.5), dict(0.0, 1.0)),
        (alphas(0.3, 1.0), dict(0.3, 1.0)),
    ] {
        let res = gs(state);
        assert_eq!(res, want, "{state:?}");
        let ResBody::Objects(objects) = &res.body else {
            panic!("{res:?}")
        };
        let mut out = Vec::new();
        assert!(serialize::write(&mut out, &objects[0]).is_ok(), "{state:?}");
    }
    assert_eq!(
        gs(GState::default()),
        Res::dict(dict! { "Type" => "ExtGState" })
    );
    let blends = [
        (Blend::Normal, "Normal"),
        (Blend::Multiply, "Multiply"),
        (Blend::Screen, "Screen"),
        (Blend::Overlay, "Overlay"),
        (Blend::Darken, "Darken"),
        (Blend::Lighten, "Lighten"),
        (Blend::ColorDodge, "ColorDodge"),
        (Blend::ColorBurn, "ColorBurn"),
        (Blend::HardLight, "HardLight"),
        (Blend::SoftLight, "SoftLight"),
        (Blend::Difference, "Difference"),
        (Blend::Exclusion, "Exclusion"),
        (Blend::Hue, "Hue"),
        (Blend::Saturation, "Saturation"),
        (Blend::Color, "Color"),
        (Blend::Luminosity, "Luminosity"),
    ];
    for (blend, name) in blends {
        let state = GState {
            blend: Some(blend),
            ..GState::default()
        };
        let want = Res::dict(dict! { "Type" => "ExtGState", "BM" => name });
        assert_eq!(gs(state), want);
    }
}

/// Names come from the digest, bumped past what the resources bind to
/// something else.
#[test]
fn names_are_bound_at_the_end() {
    let draw = |w: &mut ContentWriter| {
        w.begin_marked("Artifact", list("Header"));
        w.end_marked();
        w.begin_marked("Artifact", list("Footer"));
        w.end_marked();
    };
    let mut w = ContentWriter::new();
    draw(&mut w);
    let (_, needs) = written(w);
    let [header, footer] = &needs.0[..] else {
        panic!("{needs:?}")
    };
    for name in [&header.name, &footer.name] {
        assert_eq!(name.len(), 8, "{name:?}");
        assert!(name.starts_with(b"Sh"));
        assert!(
            name[2..]
                .iter()
                .all(|c| b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567".contains(c))
        );
    }
    assert_ne!(header.name, footer.name);

    // The header's name, taken by something else in the page's
    // resources: the next name up. Bound to the header's own list, it
    // stays.
    let same = (1, 0);
    let objects = BTreeMap::from([
        (same, Object::from(list("Header"))),
        ((2, 0), Object::from(same)),
    ]);
    let taken = |bound: Object| {
        let resources = Object::from(dict! {
            "Properties" => dict! { header.name.clone() => bound },
        });
        reserved(&objects, &resources)
    };
    for (bound, bumped) in [
        (list("Other").into(), true),
        (Object::from(same), false),
        (list("Header").into(), false),
        (Object::from((99, 0)), true),
        // A reference to a reference, followed.
        (Object::from((2, 0)), false),
    ] {
        let mut w = ContentWriter::new();
        draw(&mut w);
        let (_, needs) = w.finish(&taken(bound.clone()));
        let want = if bumped {
            next(&header.name)
        } else {
            header.name.clone()
        };
        assert_eq!(needs.0[0].name, want, "{bound:?}");
        assert_eq!(needs.0[1].name, footer.name);
    }
    // A name in another category doesn't count.
    let fonts = Object::from(dict! {
        "Font" => dict! { header.name.clone() => list("Other") },
    });
    let mut w = ContentWriter::new();
    draw(&mut w);
    let (_, needs) = w.finish(&reserved(&objects, &fonts));
    assert_eq!(needs.0[0].name, header.name);

    // A name reserved is never made: the header's and the next one up,
    // both bound to something else, make it the one after.
    let resources = Object::from(dict! {
        "Properties" => dict! {
            header.name.clone() => list("Other"),
            next(&header.name) => list("Another"),
        },
    });
    let mut w = ContentWriter::new();
    draw(&mut w);
    let (_, needs) = w.finish(&reserved(&objects, &resources));
    assert_eq!(needs.0[0].name, next(&next(&header.name)));
}

/// A resources dictionary, and a category of it, given by reference are
/// followed, eight references in a row at most; a stream is read as its
/// dictionary.
#[test]
fn resources_by_reference_are_followed() {
    let mut w = ContentWriter::new();
    w.begin_marked("Artifact", list("Header"));
    let (_, needs) = w.clone().finish(&Reserved::default());
    let name = needs.0[0].name.clone();
    let category = dict! { name.clone() => list("Other") };
    // Objects 1 to 9 each refer to the next; 10 is the category, 11 the
    // resources, 12 a stream of the category.
    let mut objects: BTreeMap<ObjectId, Object> = (1..10)
        .map(|n| ((n, 0), Object::reference(n + 1)))
        .collect();
    objects.insert((10, 0), Object::from(category.clone()));
    objects.insert(
        (11, 0),
        Object::from(dict! { "Properties" => Object::reference(10) }),
    );
    let mut stream = Stream::default();
    stream.dict = category;
    objects.insert((12, 0), Object::from(stream));
    let bumped = |resources: Object| {
        let (_, needs) = w.named(&reserved(&objects, &resources));
        needs.0[0].name != name
    };
    let by = |category: Object| Object::from(dict! { "Properties" => category });
    assert!(bumped(Object::reference(11)), "the resources by reference");
    assert!(bumped(by(Object::reference(10))));
    assert!(bumped(by(Object::reference(3))), "eight in a row");
    assert!(!bumped(by(Object::reference(2))), "nine");
    assert!(!bumped(by(Object::reference(99))), "a reference to none");
    assert!(bumped(by(Object::reference(12))), "a stream's dictionary");
    assert!(!bumped(Object::null()));
}

/// A name bound to an object that holds a stream's data where a reader
/// found it, or none, is told by its reference alone, never by a digest,
/// which a place shares with another file's stream; one in memory is
/// told by its digest too.
#[test]
fn a_placed_stream_is_told_by_its_reference_alone() {
    let stream = |data: Data| {
        let mut s = Stream::default();
        s.dict = dict! { "Type" => "XObject", "Subtype" => "Form" };
        s.data = data;
        Object::from(s)
    };
    let name = |res: &Res, reserved: &Reserved| {
        let mut w = ContentWriter::new();
        w.draw(res.clone());
        w.finish(reserved).1.0.remove(0).name
    };
    for (object, by_digest) in [
        (stream(Data::At(Span::new(None, 10, 20))), false),
        (stream(Data::At(Span::new(Some((4, 0)), 0, 7))), false),
        (stream(Data::Unplaced), false),
        (stream(Data::Bytes(b"q\n".to_vec())), true),
        (Object::from(vec![stream(Data::Unplaced)]), false),
    ] {
        let objects = BTreeMap::from([((5, 0), object.clone())]);
        let bound = |res: &Res| {
            let named = name(res, &Reserved::default());
            let resources = Object::from(dict! {
                "XObject" => dict! { named.clone() => Object::reference(5) },
            });
            (named, reserved(&objects, &resources))
        };
        // The object itself, made a resource, digests as the page's does.
        let res = Res::of([object.clone()]);
        let (named, kept) = bound(&res);
        assert_eq!(name(&res, &kept) == named, by_digest, "{object:?}");
        // By its number, always.
        let res = Res::written((5, 0));
        let (named, kept) = bound(&res);
        assert_eq!(name(&res, &kept), named, "{object:?}");
    }
}

/// What the names a resources dictionary binds end at is read and
/// digested once, however many names bind it, so a dictionary of many
/// names binding one large object costs what it holds; and only the
/// writer's names are read for what they bind.
#[test]
fn each_object_names_bind_is_read_once() {
    use std::cell::Cell;
    let sh = |i: u32| {
        let mut name = b"Sh".to_vec();
        name.extend((0..6).rev().map(|k| BASE32[(i >> (5 * k)) as usize & 31]));
        name
    };
    let big = Object::from((0..10_000).map(Object::from).collect::<Vec<_>>());
    let mut objects = BTreeMap::from([((5, 0), big)]);
    let mut names = Dict::new();
    for i in 0..200u32 {
        // Half bound straight to object 5, half through one of their own.
        let to = match i % 2 {
            0 => 5,
            _ => {
                objects.insert((100 + i, 0), Object::reference(5));
                100 + i
            }
        };
        names.push(sh(i), Object::reference(to));
        names.push(format!("F{i}").as_bytes(), Object::reference(to));
    }
    let resources = Object::from(dict! { "XObject" => names });
    let five = Cell::new(0);
    let resolve = |id: ObjectId| {
        five.set(five.get() + usize::from(id == (5, 0)));
        objects.get(&id)
    };
    let kept = Reserved::read(&resolve, Some(&resources));
    assert_eq!(five.get(), 1, "object 5 read once");
    assert_eq!(kept.names().count(), 400);
    // Names the writer doesn't give are kept clear of, unread.
    let others: Dict = (0..200u32)
        .map(|i| (format!("F{i}").into_bytes(), Object::reference(5)))
        .collect();
    let resources = Object::from(dict! { "XObject" => others });
    five.set(0);
    let kept = Reserved::read(&resolve, Some(&resources));
    assert_eq!((five.get(), kept.names().count()), (0, 200));
    // A name of the writer's binding the very resource, through a
    // reference of its own, is used again.
    let mut w = ContentWriter::new();
    w.draw(Res::of([objects[&(5, 0)].clone()]));
    let (_, needs) = w.named(&Reserved::default());
    let mut bound = Dict::new();
    bound.push(&needs.0[0].name, Object::reference(101));
    let resources = Object::from(dict! { "XObject" => bound });
    let (_, again) = w.named(&reserved(&objects, &resources));
    assert_eq!(again.0[0].name, needs.0[0].name);
}

/// A resolver kept in a binding reads resources dictionaries made one
/// after another, each borrowed for its call alone.
#[test]
fn a_resolver_reads_resources_made_after_it() {
    let objects = BTreeMap::from([((1, 0), Object::from(list("Header")))]);
    let resolve = |id: ObjectId| objects.get(&id);
    let mut read = 0;
    for name in ["ShAAAAAA", "ShAAAAAB"] {
        let resources = Object::from(dict! {
            "Properties" => dict! { name => Object::reference(1) },
        });
        read += Reserved::read(&resolve, Some(&resources)).names().count();
    }
    assert_eq!(read, 2);
}

/// One resource drawn in two categories is a need in each, each named in
/// its own.
#[test]
fn a_resource_in_two_categories_is_a_need_in_each() {
    let form = Res::written((7, 0));
    let mut w = ContentWriter::new();
    w.draw(form.clone());
    w.begin_text();
    w.font(form.clone(), 12.0);
    w.end_text();
    w.draw(form.clone());
    let (text, needs) = written(w);
    let kinds: Vec<_> = needs.0.iter().map(|n| (n.kind, &n.res)).collect();
    assert_eq!(kinds, [(ResKind::XObject, &form), (ResKind::Font, &form)]);
    let name = |i: usize| String::from_utf8_lossy(&needs.0[i].name).into_owned();
    assert_eq!(
        text,
        format!("/{0} Do\nBT\n/{1} 12 Tf\nET\n/{0} Do\n", name(0), name(1))
    );
}

/// Two resources whose digests start with the same 30 bits are named
/// apart in one content, the second drawn the next name up.
#[test]
fn resources_whose_digests_start_alike_are_named_apart() {
    let list = |n: i64| dict! { "Type" => "Pagination", "N" => n };
    let (a, b) = (Res::dict(list(22_004)), Res::dict(list(27_618)));
    assert_ne!(a.id, b.id);
    assert_eq!(a.id.0 >> 98, b.id.0 >> 98, "the same first 30 bits");
    let mut w = ContentWriter::new();
    w.begin_marked("A", list(22_004));
    w.begin_marked("B", list(27_618));
    let (text, needs) = written(w);
    let [first, second] = &needs.0[..] else {
        panic!("{needs:?}")
    };
    assert_eq!(second.name, next(&first.name));
    let (f, s) = (
        String::from_utf8_lossy(&first.name),
        String::from_utf8_lossy(&second.name),
    );
    assert_eq!(text, format!("/A /{f} BDC\n/B /{s} BDC\n"));
}

/// A resource drawn again is one need, where it was first drawn; a form
/// written already is named by its number.
#[test]
fn needs_are_one_a_resource_in_first_use_order() {
    let (helvetica, zapf) = (font("Helvetica"), font("ZapfDingbats"));
    let mut w = ContentWriter::new();
    w.font(helvetica.clone(), 9.0);
    w.draw(Res::written((7, 0)));
    w.font(zapf.clone(), 9.0);
    w.font(helvetica.clone(), 10.0);
    w.draw(Res::written((7, 0)));
    w.draw(Res::written((8, 0)));
    let (text, needs) = written(w);
    let kinds: Vec<_> = needs.0.iter().map(|n| (n.kind, &n.res.body)).collect();
    assert_eq!(
        kinds,
        [
            (ResKind::Font, &helvetica.body),
            (ResKind::XObject, &ResBody::Written((7, 0))),
            (ResKind::Font, &zapf.body),
            (ResKind::XObject, &ResBody::Written((8, 0))),
        ]
    );
    let name = |i: usize| String::from_utf8_lossy(&needs.0[i].name).into_owned();
    assert_eq!(
        text,
        format!(
            "/{0} 9 Tf\n/{1} Do\n/{2} 9 Tf\n/{0} 10 Tf\n/{1} Do\n/{3} Do\n",
            name(0),
            name(1),
            name(2),
            name(3)
        )
    );
}

/// The names kept: those a resources dictionary binds, those content was
/// given, kept too, and one forgotten, free again.
#[test]
fn the_names_kept_are_taken_and_forgotten() {
    let mut w = ContentWriter::new();
    w.begin_marked("Artifact", list("Header"));
    w.draw(Res::written((3, 0)));
    let (_, needs) = w.named(&Reserved::default());
    let resources = Object::from(dict! { "Font" => dict! { "F1" => Object::reference(1) } });
    let mut kept = reserved(&BTreeMap::new(), &resources);
    kept.take(&needs);
    kept.take(&needs);
    let names: Vec<_> = kept.names().map(|(k, n)| (k, n.to_vec())).collect();
    let (list, form) = (&needs.0[0], &needs.0[1]);
    assert_eq!(
        names,
        [
            (ResKind::Font, b"F1".to_vec()),
            (ResKind::Properties, list.name.clone()),
            (ResKind::XObject, form.name.clone()),
        ]
    );
    // Taken, a name stays its resource's.
    assert_eq!(w.named(&kept).1, needs);
    kept.forget(ResKind::XObject, &form.name);
    kept.forget(ResKind::XObject, b"none");
    assert_eq!(kept.names().count(), 2);
    // Taken for another resource, a name is bumped; forgotten, it's free.
    let mut other = ContentWriter::new();
    other.draw(Res::written((4, 0)));
    let (_, own) = other.named(&Reserved::default());
    let mut taken = Reserved::default();
    taken.take(&ResourceNeeds(vec![Need {
        res: Res::written((5, 0)),
        ..own.0[0].clone()
    }]));
    assert_eq!(other.named(&taken).1.0[0].name, next(&own.0[0].name));
    taken.forget(ResKind::XObject, &own.0[0].name);
    assert_eq!(other.named(&taken).1, own, "forgotten, free again");
}

/// An image fills its target.
#[test]
fn an_image_fills_its_target() {
    let mut w = ContentWriter::new();
    w.image(Res::written((6, 0)), 1.0, 2.0, 3.0, 4.0);
    let (text, needs) = written(w);
    let name = String::from_utf8_lossy(&needs.0[0].name);
    assert_eq!(text, format!("q\n3 0 0 4 1 2 cm\n/{name} Do\nQ\n"));
}

/// The same calls give the same bytes and names, here and on other
/// threads.
#[test]
fn identical_calls_give_identical_bytes() {
    fn draw() -> (Vec<u8>, ResourceNeeds) {
        let mut w = ContentWriter::new();
        w.begin_marked("Artifact", list("Footer"));
        w.save();
        w.gstate(GState {
            fill_alpha: Some(0.3),
            ..GState::default()
        });
        w.begin_text();
        w.fill_color(Color::rgb8([200, 30, 30]));
        w.font(font("Helvetica-Bold"), 72.0);
        let turn = 45f32.to_radians();
        w.text_matrix([
            turn.cos(),
            turn.sin(),
            -turn.sin(),
            turn.cos(),
            123.456,
            789.012,
        ]);
        w.show(b"Caf\xe9 \x93x\x94 \x96 \x80");
        w.font(font("Wide"), 10.0);
        w.show(b"\x4e\x2d\x65\x87");
        w.end_text();
        w.restore();
        w.end_marked();
        w.finish(&Reserved::default())
    }
    let first = draw();
    assert_eq!(draw(), first);
    let threads: Vec<_> = (0..2).map(|_| std::thread::spawn(draw)).collect();
    for t in threads {
        assert_eq!(t.join().unwrap(), first);
    }
}

/// What each call writes reads back as written: each number as PDFium
/// reads it within its rounding, each name and string as written, and
/// each resource's name a need's, in its category.
#[test]
fn what_each_call_writes_reads_back_as_written() {
    for x in [
        0.0,
        -0.0,
        1.0 / 3.0,
        -2.5,
        0.00005,
        -0.00004,
        0.99995,
        1e7,
        123_456.79,
        16_777_216.0,
        2_147_483_520.0,
        -2_147_483_648.0,
        f32::MIN_POSITIVE,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ] {
        let mut w = ContentWriter::new();
        w.line_width(x);
        let (content, _) = w.finish(&Reserved::default());
        let read = read(&content)[0].arg(0).unwrap().number();
        assert!(close(read, x), "{x} written {content:?}, read {read}");
    }

    let (helvetica, header) = (font("Helvetica"), list("Header"));
    let state = GState {
        fill_alpha: Some(0.5),
        ..GState::default()
    };
    let mut w = ContentWriter::new();
    w.font(helvetica.clone(), 9.5);
    let shown: [&[u8]; 4] = [b"(a)\\ \xe9", b"\r\n", b"", b"\xff\x00"];
    for codes in shown {
        w.show(codes);
    }
    let tags = ["A b/c#(d)", "\u{7f}é", "Span"];
    w.begin_tagged(tags[0]);
    w.begin_tagged(tags[1]);
    w.begin_marked(tags[2], header.clone());
    w.gstate(state);
    w.draw(Res::written((9, 0)));
    w.dash(&[3.0, 1.0 / 3.0], 2.0);
    w.transform([1.5, -0.25, 0.0, 1.0, 1e6, -7.0]);
    let (content, needs) = w.finish(&Reserved::default());
    let ops = read(&content);
    let names: Vec<_> = ops.iter().map(Operation::name).collect();
    assert_eq!(
        names,
        [
            &b"Tf"[..],
            b"Tj",
            b"Tj",
            b"Tj",
            b"Tj",
            b"BMC",
            b"BMC",
            b"BDC",
            b"gs",
            b"Do",
            b"d",
            b"cm"
        ]
    );
    let number = |op: &Operation, i: usize| op.arg(i).unwrap().number();
    let name = |op: &Operation, i: usize| op.arg(i).unwrap().name().unwrap().into_owned();
    assert_eq!(number(&ops[0], 0), 9.5);
    for (op, codes) in ops[1..5].iter().zip(shown) {
        let read = object(op.arg(0).unwrap().bytes(), &Limits::DEFAULT).unwrap();
        assert!(
            matches!(read.value(), Value::String(Str::Bytes(b), _) if b == codes),
            "{read:?}"
        );
    }
    for (op, tag) in ops[5..8].iter().zip(tags) {
        let i = usize::from(op.name() == b"BDC");
        assert_eq!(name(op, i), tag.as_bytes());
    }
    let dash: Vec<f32> = ops[10]
        .arg(1)
        .unwrap()
        .elements()
        .map(|e| e.number())
        .collect();
    assert!(close(dash[0], 3.0) && close(dash[1], 1.0 / 3.0), "{dash:?}");
    assert_eq!(number(&ops[10], 0), 2.0);
    let cm: Vec<f32> = ops[11].operands().map(|o| o.number()).collect();
    assert_eq!(cm, [1.5, -0.25, 0.0, 1.0, 1e6, -7.0]);
    // Each resource is named as a need in its category, and each need is
    // named.
    let named: Vec<_> = ops
        .iter()
        .filter_map(|op| op.resource())
        .map(|(kind, name)| (kind, name.into_owned()))
        .collect();
    let given: Vec<_> = needs.0.iter().map(|n| (n.kind, n.name.clone())).collect();
    assert_eq!(named, given);
    let res: Vec<_> = needs.0.iter().map(|n| &n.res).collect();
    let written_9 = Res::written((9, 0));
    assert_eq!(
        res,
        [&helvetica, &Res::dict(header), &state.res(), &written_9]
    );
}

/// A whole number outside the signed 32-bit range keeps its point, so
/// PDFium reads it as itself, where without it, it reads 0; one inside
/// that range, `-2147483648` and `2147483520` (the `f32` below 2^31)
/// among them, is written bare, as is every other number but for its
/// decimals.
#[test]
fn numbers_past_32_bits_keep_their_point() {
    for (x, written) in [
        (2_147_483_648.0, &b"2147483648."[..]),
        (4_294_967_296.0, b"4294967296."),
        (1e10, b"10000000000."),
        (-2_147_483_904.0, b"-2147483904."),
        // Its exact decimal, as every number is written.
        (f32::MAX, b"340282346638528859811704183484516925440."),
        (f32::MIN, b"-340282346638528859811704183484516925440."),
        (-2_147_483_648.0, b"-2147483648"),
        (2_147_483_520.0, b"2147483520"),
        (16_777_216.0, b"16777216"),
        (-0.0, b"0"),
        (0.5, b"0.5"),
    ] {
        let mut out = Vec::new();
        write_number(&mut out, x);
        assert_eq!(out, written, "{x}");
        let mut w = ContentWriter::new();
        w.line_width(x);
        let (content, _) = w.finish(&Reserved::default());
        let as_written = read(&content)[0].arg(0).unwrap().number();
        assert_eq!(as_written, x, "{x} read from {content:?}");
        // Without its point, PDFium reads one past 32 bits as 0, or, below
        // 2^32 and unsigned, as itself.
        let bare = written.strip_suffix(b".").unwrap_or(written);
        let without = [bare, b" w"].concat();
        let bare_read = read(&without)[0].arg(0).unwrap().number();
        let unsigned = (0.0..4_294_967_296.0).contains(&x);
        let shown = String::from_utf8_lossy(bare);
        assert_eq!(
            bare_read == x,
            written == bare || unsigned,
            "{shown} read as {bare_read}"
        );
    }
}

/// A name past 255 bytes, its `/` among them, and a string past 32,767
/// bytes are written whole, and read as PDFium reads them: the name's
/// first 255 bytes, the string's first 32,767.
#[test]
fn a_long_name_and_string_are_written_whole() {
    let tag = "a".repeat(300);
    let codes = vec![b'x'; 40_000];
    let mut w = ContentWriter::new();
    w.begin_tagged(&tag);
    w.show(&codes);
    let (content, _) = w.finish(&Reserved::default());
    let mut want = [b"/", tag.as_bytes(), b" BMC\n("].concat();
    want.extend_from_slice(&codes);
    want.extend_from_slice(b") Tj\n");
    assert_eq!(content, want);
    let ops = read(&content);
    let tag_read = ops[0].arg(0).unwrap().name().unwrap();
    assert_eq!(&tag_read[..], &tag.as_bytes()[..254]);
    let string = object(ops[1].arg(0).unwrap().bytes(), &Limits::DEFAULT).unwrap();
    assert_eq!(string.as_bytes(), Some(&codes[..32_767]));
}
