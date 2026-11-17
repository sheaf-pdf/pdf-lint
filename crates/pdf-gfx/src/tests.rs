//! Unit, round-trip, resource-cap, and hygiene tests for `pdf-gfx`.

use std::collections::BTreeMap;
use std::ops::ControlFlow;

use pdf_content_streams::{
    Blend, Color, Concat, FillRule, GState, Join, LineCap, LineJoin, Paint, Res, Reserved, TextMode,
};
use pdf_fonts::{Fonts, Std14};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Object, ObjectId, Stream, Unreadable, Why, WithStreams, dict};
use pdf_semantics::pages;

use crate::{
    ActiveColorSpace, BlendState, Canvas, Cap, Clip, ClipPart, ColorValues, DashPattern,
    EncodedPage, Error, Fault, FormUse, Forms, ImageDrawn, ImageOf, Kind, Limits, Mark, MarkProps,
    Marks, Matrix, Opts, PageGeom, PathDrawn, Point, Quad, Rect, Refused, RenderingIntent,
    Rotation, Seg, ShadingDrawn, SoftMask, SoftMaskKind, State, Summary, TextPiece, TextState,
    Unwritable, UseId, Wants, encode, encode_with, fuzz, interpret, interpret_with,
};

#[derive(Default)]
struct Store {
    objects: BTreeMap<ObjectId, Object>,
    streams: BTreeMap<ObjectId, Vec<u8>>,
    next: u32,
}

impl Store {
    fn alloc(&mut self) -> ObjectId {
        self.next += 1;
        ObjectId::new(self.next, 0)
    }

    fn put(&mut self, obj: impl Into<Object>) -> ObjectId {
        let id = self.alloc();
        self.objects.insert(id, obj.into());
        id
    }

    fn put_stream(&mut self, dict: Dict, data: Vec<u8>) -> ObjectId {
        let id = self.alloc();
        self.streams.insert(id, data.clone());
        self.objects
            .insert(id, Object::from(Stream::new(dict, data)));
        id
    }

    fn as_streams(&self) -> impl pdf_objects::Resolve<'_> + pdf_objects::Streams + Copy {
        WithStreams::new(&self.objects, move |id, cap| match self.streams.get(&id) {
            Some(bytes) if bytes.len() <= cap => Ok(bytes.clone()),
            Some(_) => Err(Unreadable::new(id, Why::Limit, "past budget")),
            None => Err(Unreadable::new(id, Why::Damaged, "no stream")),
        })
    }
}

fn helvetica_resources() -> Object {
    Object::from(dict! {
        "Font" => dict! {
            "F1" => Std14::Helvetica.dict(),
            "F2" => Std14::Courier.dict(),
        },
    })
}

fn assert_approx(a: f32, b: f32) {
    let diff = (a - b).abs();
    assert!(diff <= 1e-3, "expected {a} ≈ {b} (diff {diff})");
}

fn assert_rect_approx(a: Rect, b: Rect) {
    assert_approx(a.x0, b.x0);
    assert_approx(a.y0, b.y0);
    assert_approx(a.x1, b.x1);
    assert_approx(a.y1, b.y1);
}

#[test]
fn public_types_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Point>();
    check::<Rect>();
    check::<Quad>();
    check::<Matrix>();
    check::<Rotation>();
    check::<PageGeom>();
    check::<Limits>();
    check::<Cap>();
    check::<Error>();
    check::<Unwritable>();
    check::<Fault>();
    check::<State>();
    check::<TextState>();
    check::<ActiveColorSpace>();
    check::<ColorValues>();
    check::<DashPattern>();
    check::<RenderingIntent>();
    check::<BlendState>();
    check::<SoftMask>();
    check::<SoftMaskKind>();
    check::<Clip>();
    check::<ClipPart>();
    check::<Mark>();
    check::<MarkProps>();
    check::<Marks>();
    check::<PathDrawn<'static>>();
    check::<Seg>();
    check::<ImageDrawn>();
    check::<ShadingDrawn>();
    check::<FormUse<'static>>();
    check::<UseId>();
    check::<Refused>();
    check::<Summary>();
    check::<EncodedPage>();
    check::<Canvas>();
    #[cfg(feature = "text-layout")]
    {
        check::<crate::PageChar>();
        check::<crate::TextLine>();
        check::<crate::SearchOpts>();
        check::<crate::TextMatch>();
        check::<crate::TextPage>();
        check::<crate::TextPageOpts>();
        check::<crate::FontEncoder<'static>>();
        check::<crate::TextAlign>();
        check::<crate::VerticalAlign>();
        check::<crate::WrapMode>();
        check::<crate::TextBoxStyle>();
        check::<crate::UnencodableChar>();
        check::<crate::LaidOutLine>();
        check::<crate::LaidOutText>();
    }
}

#[test]
fn readme_first_example_and_links_match_invariants() {
    let readme = include_str!("../README.md");
    let lib = include_str!("lib.rs");

    assert!(
        readme.contains("as of chromium/8152"),
        "README.md must state the pinned PDFium build"
    );
    assert!(
        readme.contains("https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-APACHE"),
        "README.md must link LICENSE-APACHE"
    );
    assert!(
        readme.contains("https://github.com/sheaf-pdf/pdf-lint/blob/main/LICENSE-MIT"),
        "README.md must link LICENSE-MIT"
    );
    assert!(
        !readme.contains("](../"),
        "README.md must not contain relative parent links"
    );

    let mut rest = readme;
    while let Some(pos) = rest.find("](") {
        let after = &rest[pos + 2..];
        let end = after.find(')').expect("unclosed markdown link");
        let url = &after[..end];
        assert!(
            url.starts_with("https://"),
            "every README.md link must use https://, found {url:?}"
        );
        rest = &after[end + 1..];
    }

    let readme_block = readme
        .split_once("```rust\n")
        .and_then(|(_, tail)| tail.split_once("\n```"))
        .map(|(code, _)| code)
        .expect("README.md must have a ```rust block");

    let doc_lines: String = lib
        .lines()
        .take_while(|l| l.starts_with("//!") || l.is_empty())
        .filter_map(|l| l.strip_prefix("//! ").or_else(|| l.strip_prefix("//!")))
        .collect::<Vec<_>>()
        .join("\n");
    let lib_block = doc_lines
        .split_once("```\n")
        .and_then(|(_, tail)| tail.split_once("\n```"))
        .map(|(code, _)| code)
        .expect("src/lib.rs must have a ``` doc block");

    assert_eq!(
        readme_block, lib_block,
        "first ```rust block in README.md must match first ``` block in src/lib.rs line-for-line"
    );
}

#[test]
fn debug_hygiene_never_leaks_plaintext_names_or_tags() {
    let mut store = Store::default();
    let cs_id = store.put(Object::from(vec![
        Object::name("CalRGB"),
        Object::from(dict! {}),
    ]));
    let prop_id = store.put(Object::from(dict! {
        "Type" => Object::name("OCG"),
        "MCID" => 7,
        "ActualText" => Object::text("SECRET_ACTUAL_TEXT"),
        "SecretKey" => Object::text("SECRET_PROP_VALUE"),
    }));
    let form_id = store.put_stream(
        dict! {
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(100)],
        },
        b"0 0 10 10 re f".to_vec(),
    );

    let resources = Object::from(dict! {
        "Font" => dict! { "SECRET_FONT" => Std14::Helvetica.dict() },
        "ColorSpace" => dict! { "SECRET_CS" => cs_id },
        "Properties" => dict! { "SECRET_PROP_NAME" => prop_id },
        "XObject" => dict! { "SECRET_FORM" => form_id },
    });

    let stream = br#"
        /SECRET_CS CS 0.1 0.2 0.3 /SECRET_PAT SCN
        /SECRET_TAG /SECRET_PROP_NAME BDC
        /SECRET_INLINE << /SecretInline (SECRET_INLINE_VAL) /ActualText (SECRET_ACTUAL_INLINE) >> BDC
        BT /SECRET_FONT 12 Tf 10 20 Td (SECRET_GLYPH_STR) Tj ET
        /SECRET_MISSING gs
        /SECRET_FORM Do
        EMC EMC
    "#;
    let concat = Concat::new(&[stream]);
    let fonts = Fonts::default();
    let mut faults = Vec::new();
    let mut debugs = Vec::new();

    interpret_with(
        store.as_streams(),
        &concat,
        Some(&resources),
        &Limits::DEFAULT,
        &fonts,
        Opts::default(),
        &mut faults,
        |ev| {
            debugs.push(format!("{:?}", ev.state));
            debugs.push(format!("{:?}", ev.marks));
            debugs.push(format!("{:?}", ev.kind));
            if let Kind::Text(show) = ev.kind {
                for g in show.glyphs() {
                    debugs.push(format!("{g:?}"));
                }
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();

    for (_, fault) in &faults {
        debugs.push(format!("{fault:?}"));
    }
    let encoded = encode(
        store.as_streams(),
        &concat,
        Some(&resources),
        &Limits::DEFAULT,
        &fonts,
    )
    .unwrap();
    debugs.push(format!("{encoded:?}"));
    debugs.push(format!("{:?}", TextPiece::Codes(b"SECRET_PIECE")));
    #[cfg(feature = "text-layout")]
    {
        let tp = crate::TextPage::extract(
            store.as_streams(),
            &concat,
            Some(&resources),
            &Limits::DEFAULT,
            &fonts,
            crate::TextPageOpts::default(),
        )
        .unwrap();
        debugs.push(format!("{tp:?}"));
        for ch in tp.chars() {
            debugs.push(format!("{ch:?}"));
        }
        let laid = crate::layout_text(
            &Std14::Helvetica,
            "SECRET_GLYPH_STR",
            Rect::new(0.0, 0.0, 200.0, 50.0),
            &crate::TextBoxStyle::default(),
        )
        .unwrap();
        debugs.push(format!("{laid:?}"));
    }

    let all = debugs.join("\n");
    for secret in [
        "SECRET_FONT",
        "SECRET_CS",
        "SECRET_PAT",
        "SECRET_PROP_NAME",
        "SECRET_PROP_VALUE",
        "SECRET_ACTUAL_TEXT",
        "SECRET_TAG",
        "SECRET_INLINE",
        "SECRET_INLINE_VAL",
        "SECRET_ACTUAL_INLINE",
        "SECRET_GLYPH_STR",
        "SECRET_MISSING",
        "SECRET_FORM",
        "SECRET_PIECE",
    ] {
        assert!(
            !all.contains(secret),
            "Debug output leaked {secret:?}:\n{all}"
        );
    }
}

#[cfg(feature = "tracing")]
#[test]
fn tracing_spans_never_contain_plaintext_or_names() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event as TraceEvent, Metadata, Subscriber};

    #[derive(Default, Clone)]
    struct FieldCollector {
        next_id: Arc<AtomicU64>,
        fields: Arc<Mutex<Vec<String>>>,
    }

    struct Visitor(Arc<Mutex<Vec<String>>>);
    impl Visit for Visitor {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0
                .lock()
                .unwrap()
                .push(format!("{}={value:?}", field.name()));
        }
    }

    impl Subscriber for FieldCollector {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, attrs: &Attributes<'_>) -> Id {
            let mut v = Visitor(Arc::clone(&self.fields));
            attrs.record(&mut v);
            Id::from_u64(self.next_id.fetch_add(1, Ordering::Relaxed) + 1)
        }

        fn record(&self, _: &Id, values: &Record<'_>) {
            let mut v = Visitor(Arc::clone(&self.fields));
            values.record(&mut v);
        }

        fn record_follows_from(&self, _: &Id, _: &Id) {}

        fn event(&self, event: &TraceEvent<'_>) {
            let mut v = Visitor(Arc::clone(&self.fields));
            event.record(&mut v);
        }

        fn enter(&self, _: &Id) {}

        fn exit(&self, _: &Id) {}
    }

    let collector = FieldCollector::default();
    let res = Object::from(dict! {
        "Font" => dict! { "TRACE_SECRET_FONT" => Std14::Helvetica.dict() },
    });
    let concat = Concat::new(&[b"BT /TRACE_SECRET_FONT 12 Tf 10 20 Td (TRACE_SECRET_BODY) Tj ET"]);
    let store = Store::default();
    let fonts = Fonts::default();

    tracing::subscriber::with_default(collector.clone(), || {
        let _ = encode(
            store.as_streams(),
            &concat,
            Some(&res),
            &Limits::DEFAULT,
            &fonts,
        )
        .unwrap();
    });

    let recorded = collector.fields.lock().unwrap().join("\n");
    assert!(
        !recorded.is_empty(),
        "expected tracing spans to be recorded"
    );
    assert!(!recorded.contains("TRACE_SECRET_FONT"), "{recorded}");
    assert!(!recorded.contains("TRACE_SECRET_BODY"), "{recorded}");
}

#[test]
fn state_operators_colors_and_extgstate_rules() {
    let mut store = Store::default();
    let smask_group = store.put_stream(dict! { "Subtype" => Object::name("Form") }, Vec::new());
    let gs_op_fallback = store.put(Object::from(dict! {
        "LW" => 2.5,
        "LC" => 1,
        "LJ" => 2,
        "ML" => 6.0,
        "D" => vec![Object::from(vec![Object::from(4), Object::from(2)]), Object::from(1.5)],
        "RI" => Object::name("Perceptual"),
        "FL" => 3.0,
        "CA" => 0.4,
        "ca" => 0.8,
        "AIS" => true,
        "TK" => false,
        "OP" => true,
        "OPM" => 1,
        "BM" => vec![Object::name("NotABlend"), Object::name("Multiply")],
        "SMask" => dict! { "S" => Object::name("Luminosity"), "G" => smask_group },
    }));
    let gs_op_explicit = store.put(Object::from(dict! {
        "OP" => true,
        "op" => false,
        "SMask" => Object::name("None"),
        "Font" => vec![Object::from(2), Object::from(14)],
    }));
    let unloaded_font = store.put(Object::from(dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("Type0"),
    }));
    let gs_unloaded_font = store.put(Object::from(dict! {
        "Font" => vec![Object::name("BadFont"), Object::from(9)],
    }));
    let cs_sep = store.put(Object::from(vec![
        Object::name("Separation"),
        Object::name("Spot"),
        Object::name("DeviceGray"),
        Object::from(dict! {
            "FunctionType" => 2,
            "Domain" => vec![Object::from(0), Object::from(1)],
            "C0" => vec![Object::from(0)],
            "C1" => vec![Object::from(1)],
            "N" => 1,
        }),
    ]));

    let resources = Object::from(dict! {
        "Font" => dict! {
            "2" => Std14::Courier.dict(),
            "F1" => Std14::Helvetica.dict(),
            "BadFont" => unloaded_font,
        },
        "ExtGState" => dict! {
            "GS1" => gs_op_fallback,
            "GS2" => gs_op_explicit,
            "GSBad" => gs_unloaded_font,
        },
        "ColorSpace" => dict! {
            "CS1" => cs_sep,
        },
    });

    let content = Concat::new(&[br#"
        Q
        q
        2 0 0 2 10 20 cm
        /CS1 CS 0.75 SCN
        /Pattern cs 0.1 0.2 0.3 0.4 0.5 /Pat1 scn
        /GS1 gs
        0 0 10 10 re f
        /GS2 gs
        BT (Hi) Tj ET
        /BadFont 11 Tf
        BT (KeptCourier) Tj ET
        /GSBad gs
        BT (DroppedByGs) Tj ET
        Q
        0 0 5 5 re S
    "#]);

    let fonts = Fonts::default();
    let mut faults = Vec::new();
    let mut states = Vec::new();
    let mut shown_texts = Vec::new();

    interpret_with(
        store.as_streams(),
        &content,
        Some(&resources),
        &Limits::DEFAULT,
        &fonts,
        Opts::default(),
        &mut faults,
        |ev| {
            match ev.kind {
                Kind::Path(_) => states.push(ev.state.clone()),
                Kind::Text(t) => {
                    let s: String = t
                        .glyphs()
                        .filter_map(|g| g.text().map(String::from))
                        .collect();
                    shown_texts.push((s, ev.state.text.size));
                }
                _ => {}
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();

    assert!(faults.iter().any(|(_, f)| *f == Fault::ExtraRestore));
    assert_eq!(states.len(), 2);

    let s0 = &states[0];
    assert_eq!(s0.ctm, Matrix::new(2.0, 0.0, 0.0, 2.0, 10.0, 20.0));
    assert_eq!(s0.line_width, 2.5);
    assert_eq!(s0.line_cap, LineCap::Round);
    assert_eq!(s0.line_join, LineJoin::Bevel);
    assert_eq!(s0.miter_limit, 6.0);
    assert_eq!(s0.dash.array.as_ref(), &[4.0, 2.0]);
    assert_eq!(s0.dash.phase, 1.5);
    assert_eq!(s0.rendering_intent, RenderingIntent::Perceptual);
    assert_eq!(s0.flatness, 3.0);
    assert_approx(s0.stroke_alpha, 0.4);
    assert_approx(s0.fill_alpha, 0.8);
    assert!(s0.alpha_is_shape);
    assert!(!s0.text_knockout);
    assert!(s0.stroke_overprint);
    assert!(
        s0.fill_overprint,
        "/OP should also set /op when /op is absent"
    );
    assert_eq!(s0.overprint_mode, 1);
    assert_eq!(s0.blend_mode, BlendState::Mode(Blend::Multiply));
    assert_eq!(
        s0.soft_mask,
        SoftMask::Mask {
            kind: Some(SoftMaskKind::Luminosity),
            group: Some(smask_group),
        }
    );
    assert_eq!(s0.stroke_color_space.resource_name(), Some(&b"CS1"[..]));
    assert_eq!(s0.stroke_color.as_slice(), &[0.75]);
    assert_eq!(s0.fill_color_space, ActiveColorSpace::Pattern);
    assert_eq!(s0.fill_color.as_slice(), &[0.1, 0.2, 0.3, 0.4, 0.5]);
    assert_eq!(s0.fill_pattern.as_deref(), Some(&b"Pat1"[..]));

    // After Q, state returns to defaults.
    let s1 = &states[1];
    assert_eq!(s1.ctm, Matrix::IDENTITY);
    assert_eq!(s1.line_width, 1.0);
    assert_eq!(s1.soft_mask, SoftMask::None);

    // GS2 set /op false explicitly, and set /Font [2 14] via GetByteStringAt ("2" -> Courier 14).
    // Subsequent `/BadFont 11 Tf` kept Courier (with size updated to 11), while `/GSBad gs`
    // cleared `state.text.font` to None so `(DroppedByGs) Tj` emitted a fontless TextShow (0 glyphs).
    assert_eq!(
        shown_texts,
        vec![
            ("Hi".to_string(), 14.0),
            ("KeptCourier".to_string(), 11.0),
            (String::new(), 9.0),
        ]
    );
}

#[test]
fn text_layout_tj_empty_string_quirk_and_vertical_writing() {
    let mut store = Store::default();
    // Vertical CIDFont Type 0 with Identity-V and /DW2 [880 -1000]
    let cid_font = store.put(Object::from(dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("CIDFontType2"),
        "BaseFont" => Object::name("VertFont"),
        "CIDSystemInfo" => dict! {
            "Registry" => Object::text("Adobe"),
            "Ordering" => Object::text("Identity"),
            "Supplement" => 0,
        },
        "DW2" => vec![Object::from(880), Object::from(-1000)],
    }));
    let type0_v = store.put(Object::from(dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("Type0"),
        "BaseFont" => Object::name("VertFont"),
        "Encoding" => Object::name("Identity-V"),
        "DescendantFonts" => vec![Object::from(cid_font)],
    }));

    let resources = Object::from(dict! {
        "Font" => dict! {
            "F1" => Std14::Courier.dict(), // monospace 600 glyph width
            "FV" => type0_v,
        },
    });

    // In Courier at 10pt, 'A' advance is 6.0.
    // `[(A) 200 () 300 (B) 500 ()] TJ`:
    // - interior empty `()` merges 200 + 300 = 500 -> B starts at 6.0 - 5.0 = 1.0!
    // - trailing empty `()` resets B's trailing adjustment from 500 to 0.0, so a following `(C) Tj`
    //   starts at 1.0 + 6.0 = 7.0 (NOT 1.0 + 6.0 - 5.0 = 2.0)!
    let content = Concat::new(&[br#"
        BT
        /F1 10 Tf
        0 100 Td
        [(A) 200 () 300 (B) 500 ()] TJ
        (C) Tj
        ET
        BT
        /FV 10 Tf
        50 200 Td
        <00410042> Tj
        ET
    "#]);

    let fonts = Fonts::default();
    let mut horiz_xs = Vec::new();
    let mut vert_ys = Vec::new();

    interpret(
        store.as_streams(),
        &content,
        Some(&resources),
        &Limits::DEFAULT,
        &fonts,
        Opts::default(),
        |ev| {
            if let Kind::Text(show) = ev.kind {
                if show.font.is_some_and(|f| f.is_vertical()) {
                    for g in show.glyphs() {
                        vert_ys.push((g.cid, g.pen.x, g.pen.y, g.advance));
                    }
                } else {
                    for g in show.glyphs() {
                        horiz_xs.push((g.text().map(String::from).unwrap_or_default(), g.pen.x));
                    }
                }
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();

    assert_eq!(horiz_xs.len(), 3);
    assert_eq!(horiz_xs[0].0, "A");
    assert_approx(horiz_xs[0].1, 0.0);
    assert_eq!(horiz_xs[1].0, "B");
    assert_approx(horiz_xs[1].1, 1.0);
    assert_eq!(horiz_xs[2].0, "C");
    assert_approx(horiz_xs[2].1, 7.0);

    assert_eq!(vert_ys.len(), 2);
    assert_eq!(vert_ys[0].0, Some(0x41));
    assert_approx(vert_ys[0].1, 50.0);
    assert_approx(vert_ys[0].2, 200.0);
    assert_approx(vert_ys[0].3, -10.0);
    assert_eq!(vert_ys[1].0, Some(0x42));
    assert_approx(vert_ys[1].2, 190.0);
}

#[cfg(feature = "programs")]
#[test]
fn embedded_truetype_outline_expands_glyph_quad() {
    let sfnt_bytes = build_wide_truetype_a();
    let mut store = Store::default();
    let ff2 = store.put_stream(dict! { "Length1" => sfnt_bytes.len() as i64 }, sfnt_bytes);
    let desc = store.put(Object::from(dict! {
        "Type" => Object::name("FontDescriptor"),
        "FontName" => Object::name("WideA"),
        "Flags" => 32,
        "FontBBox" => vec![Object::from(-200), Object::from(-400), Object::from(1200), Object::from(1100)],
        "Ascent" => 800,
        "Descent" => -200,
        "FontFile2" => ff2,
    }));
    let font_dict = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("TrueType"),
        "BaseFont" => Object::name("WideA"),
        "FirstChar" => 65,
        "LastChar" => 65,
        "Widths" => vec![Object::from(600)],
        "Encoding" => Object::name("WinAnsiEncoding"),
        "FontDescriptor" => desc,
    };
    let resources = Object::from(dict! { "Font" => dict! { "F1" => font_dict } });
    // First at 10pt (outline union active: x spans -2..12, y spans -4..11),
    // then at tiny 0.005pt (< SIZE_EPSILON = 0.01: outline skipped, stays metric cell 0..0.003).
    let content = Concat::new(&[b"BT /F1 10 Tf 0 0 Td (A) Tj /F1 0.005 Tf 0 0 Td (A) Tj ET"]);
    let fonts = Fonts::default();
    let mut boxes = Vec::new();

    interpret(
        store.as_streams(),
        &content,
        Some(&resources),
        &Limits::DEFAULT,
        &fonts,
        Opts::default(),
        |ev| {
            if let Kind::Text(show) = ev.kind {
                for g in show.glyphs() {
                    boxes.push(g.quad.bounds());
                }
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();

    assert_eq!(boxes.len(), 2);
    assert!(boxes[0].x0 < -1.95 && boxes[0].x1 > 11.95, "{:?}", boxes[0]);
    assert!(boxes[0].y0 < -3.95 && boxes[0].y1 > 10.95, "{:?}", boxes[0]);
    assert_approx(boxes[1].x0, 0.0);
    assert_approx(boxes[1].x1, 0.003);
}

#[cfg(feature = "predefined-cmaps")]
#[test]
fn predefined_cjk_cmap_decodes_glyphs() {
    let mut store = Store::default();
    let cid = store.put(Object::from(dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("CIDFontType0"),
        "BaseFont" => Object::name("KozMinPro-Regular"),
        "CIDSystemInfo" => dict! {
            "Registry" => Object::text("Adobe"),
            "Ordering" => Object::text("Japan1"),
            "Supplement" => 6,
        },
        "DW" => 1000,
    }));
    let font = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("Type0"),
        "BaseFont" => Object::name("KozMinPro-Regular-UniJIS-UTF16-H"),
        "Encoding" => Object::name("UniJIS-UTF16-H"),
        "DescendantFonts" => vec![Object::from(cid)],
    };
    let resources = Object::from(dict! { "Font" => dict! { "F1" => font } });
    // U+65E5 U+672C ("日本") in UTF-16BE
    let content = Concat::new(&[b"BT /F1 10 Tf 10 20 Td <65E5672C> Tj ET"]);
    let fonts = Fonts::default();
    let mut text = String::new();
    let mut approx_codes = false;

    interpret(
        store.as_streams(),
        &content,
        Some(&resources),
        &Limits::DEFAULT,
        &fonts,
        Opts::default(),
        |ev| {
            if let Kind::Text(show) = ev.kind {
                for g in show.glyphs() {
                    approx_codes |= g.approx.codes;
                    if let Some(t) = g.text() {
                        text.push_str(&t);
                    }
                }
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();

    assert!(!approx_codes);
    assert_eq!(text, "日本");
}

#[test]
fn paths_stroked_bounds_fast_buffer_and_single_point_clips() {
    let store = Store::default();
    let fonts = Fonts::default();
    let content = Concat::new(&[br#"
        % 1. Closed single-point dot with round cap -> radius = 4 / 2 = 2
        q 4 w 1 J 50 50 m h S Q
        % 2. Closed single-point dot with butt cap -> empty stroke
        q 4 w 0 J 50 50 m h S Q
        % 3. Hairline 0 w -> treated as width 1.0 in user space -> half = 0.5
        q 0 w 0 J 10 20 m 30 20 l S Q
        % 4. ParsePathObject 6-float fast buffer after 2-arg `m`
        q 10 10 m 1 2 3 4 20 30 c f Q
        % 5. Single-point `m` clip -> empty clip rect at (0, 0)
        q 40 40 m W n 0 0 100 100 re f Q
        % 6. Two-point horizontal line clip -> zero-height rect [10, 20, 50, 20]
        q 10 20 m 50 20 l W n 0 0 100 100 re f Q
    "#]);

    let mut path_bounds = Vec::new();
    let mut path_seg_counts = Vec::new();
    let mut clips = Vec::new();
    interpret(
        store.as_streams(),
        &content,
        None,
        &Limits::DEFAULT,
        &fonts,
        Opts::default(),
        |ev| {
            if let Kind::Path(p) = ev.kind {
                path_bounds.push(p.bounds);
                path_seg_counts.push(p.segments.len());
                clips.push(ev.state.clip.bounds());
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();

    // Closed butt-cap dot (2) produces no Path event, matching PDFium!
    // Paths: 0 = round dot, 1 = hairline, 2 = fast-buffer curve, 3 = single-pt clip (`W n`),
    // 4 = rect under single-pt clip, 5 = line clip (`W n`), 6 = rect under line clip.
    assert_eq!(path_bounds.len(), 7);
    assert_rect_approx(path_bounds[0], Rect::new(48.0, 48.0, 52.0, 52.0));
    assert_rect_approx(path_bounds[1], Rect::new(9.5, 19.5, 30.5, 20.5));
    assert_eq!(path_seg_counts[2], 2);
    assert_eq!(clips[4], Some(Rect::default()));
    assert_eq!(clips[6], Some(Rect::new(10.0, 20.0, 50.0, 20.0)));
}

#[test]
fn clip_auto_merge_text_clips_and_deep_drop_safety() {
    let store = Store::default();
    let resources = helvetica_resources();
    let fonts = Fonts::default();

    // Inner rect [20, 30, 80, 80] is contained in outer [0, 0, 100, 100], so auto-merge replaces
    // the outer rect! Then a triangle clip adds a Path; then `4 Tr` text clip adds Text at `ET`.
    let content = Concat::new(&[br#"
        0 0 100 100 re W n
        20 30 60 50 re W n
        0 0 m 80 0 l 40 80 l h W* n
        BT /F1 12 Tf 4 Tr 25 35 Td (Clip) Tj ET
        0 0 10 10 re f
    "#]);

    let mut final_clip = Clip::default();
    interpret(
        store.as_streams(),
        &content,
        Some(&resources),
        &Limits::DEFAULT,
        &fonts,
        Opts::default(),
        |ev| {
            if matches!(ev.kind, Kind::Path(ref p) if p.paint != Paint::None) {
                final_clip = ev.state.clip.clone();
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();

    // `Clip::parts()` iterates innermost first: Text -> EvenOdd triangle Path -> merged [20, 30, 80, 80] Path!
    let parts: Vec<ClipPart> = final_clip.parts().cloned().collect();
    assert_eq!(parts.len(), 3);
    assert!(matches!(parts[0], ClipPart::Text { ref glyphs, .. } if glyphs.len() == 4));
    assert!(matches!(
        parts[1],
        ClipPart::Path {
            rule: FillRule::EvenOdd,
            ..
        }
    ));
    assert!(matches!(
        parts[2],
        ClipPart::Path { bounds, .. } if bounds == Rect::new(20.0, 30.0, 80.0, 80.0)
    ));

    // Deep Clip and Marks chains drop iteratively without overflowing the call stack.
    let mut deep_clip = Clip::default();
    let mut deep_marks = Marks::default();
    for i in 0..25_000usize {
        deep_clip = deep_clip.with_path(
            &[
                Seg::Move(Point::new(0.0, 0.0)),
                Seg::Line(Point::new(10.0, i as f32)),
                Seg::Line(Point::new(5.0, 10.0)),
            ],
            FillRule::NonZero,
            Rect::new(0.0, 0.0, 10.0, i as f32),
        );
        deep_marks.push(Mark::new(b"P".to_vec(), None, None, None, 0..1));
    }
    drop(deep_clip);
    drop(deep_marks);
}

#[test]
fn forms_resource_fallback_cycles_images_shadings_and_marks() {
    let mut store = Store::default();
    let ocg_id = store.put(Object::from(dict! { "Type" => Object::name("OCG") }));
    let smask_id = store.put_stream(dict! { "Subtype" => Object::name("Image") }, Vec::new());

    // Image XObject 200x100 drawn into a 144x72 pt box -> 200 / (144/72) = 100 DPI!
    let img_id = store.put_stream(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 200,
            "Height" => 100,
            "BitsPerComponent" => 8,
            "Interpolate" => true,
            "SMask" => smask_id,
            "OC" => ocg_id,
        },
        vec![0u8; 16],
    );

    // Self-recursive Form XObject to test Refused::Cycle
    let cyc_id = store.alloc();
    let cyc_stream = Stream::new(
        dict! {
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(50), Object::from(50)],
            "Resources" => dict! { "XObject" => dict! { "Self" => cyc_id } },
        },
        b"/Self Do".to_vec(),
    );
    store.streams.insert(cyc_id, b"/Self Do".to_vec());
    store.objects.insert(cyc_id, Object::from(cyc_stream));

    // Inner Form has /Resources with /XObject ONLY (no /Font!), so `/FPage 10 Tf` falls back
    // per-category to the page's `/Resources/Font` (`FindResourceHolder`)!
    // It also has an extra `EMC` (which must NOT close the page's open `/Outer` mark) and an
    // unclosed `/UnclosedInForm BMC` (which closes implicitly at Form EOF).
    let inner_bytes = br#"
        EMC
        /OC /PropOC BDC
        BT /FPage 10 Tf 5 5 Td (FromForm) Tj ET
        q 144 0 0 72 0 0 cm /Im1 Do Q
        /UnclosedInForm BMC
    "#;
    let form_id = store.put_stream(
        dict! {
            "Subtype" => Object::name("Form"),
            "Matrix" => vec![Object::from(1), Object::from(0), Object::from(0), Object::from(1), Object::from(10), Object::from(20)],
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(200), Object::from(200)],
            "OC" => ocg_id,
            "Resources" => dict! {
                "XObject" => dict! { "Im1" => img_id },
                "Properties" => dict! { "PropOC" => ocg_id },
            },
        },
        inner_bytes.to_vec(),
    );

    let shading_id = store.put(Object::from(dict! {
        "ShadingType" => 2,
        "ColorSpace" => Object::name("DeviceRGB"),
        "BBox" => vec![Object::from(0), Object::from(0), Object::from(80), Object::from(60)],
    }));

    let page_props = store.put(Object::from(dict! {
        "MCID" => 42,
        "ActualText" => Object::bytes(&[0xFE, 0xFF, 0x00, b'O', 0x00, b'K'][..]),
    }));
    let resources = Object::from(dict! {
        "Font" => dict! { "FPage" => Std14::Helvetica.dict() },
        "XObject" => dict! { "Fm1" => form_id, "Cyc" => cyc_id, "TopImg" => img_id },
        "Shading" => dict! { "Sh1" => shading_id },
        "Properties" => dict! { "P1" => page_props },
    });

    // Inline image with valid data AND truncated inline image (`ImageStream::Missing`) which PDFium skips!
    let page_bytes = br#"
        /Artifact BMC
        /Span /P1 BDC
        /MP1 MP
        /Fm1 Do
        /Cyc Do
        0 0 50 50 re W n /Sh1 sh
        BI /W 2 /H 2 /BPC 8 /CS /G ID abcd EI
        EMC
        EMC
    "#;
    let missing_stream_inline = b"BI /W 1 /H 1 /BPC 8 /CS /G /F /XYZ ID xy EI";

    let fonts = Fonts::default();
    let mut faults = Vec::new();
    let mut dpis = Vec::new();
    let mut inline_count = 0usize;
    let mut shadings = Vec::new();
    let mut refused = Vec::new();
    let mut form_text_marks = Vec::new();

    interpret_with(
        store.as_streams(),
        &Concat::new(&[page_bytes]),
        Some(&resources),
        &Limits::DEFAULT,
        &fonts,
        Opts::default(),
        &mut faults,
        |ev| {
            match ev.kind {
                Kind::Image(img) => match img.of {
                    ImageOf::XObject(id) => {
                        assert_eq!(id, img_id);
                        assert_eq!(img.smask, Some(smask_id));
                        assert_eq!(img.oc, Some(ocg_id));
                        assert!(img.interpolate);
                        dpis.push((img.dpi(), img.dpi_with_unit(2.0)));
                    }
                    ImageOf::Inline { .. } => inline_count += 1,
                },
                Kind::Shading(sh) => shadings.push(sh),
                Kind::FormRefused { why, .. } => refused.push(why),
                Kind::Text(show) => {
                    assert!(ev.marks.in_artifact());
                    assert_eq!(ev.marks.innermost_mcid(), Some(42));
                    let tags: Vec<Vec<u8>> = ev.marks.iter().map(|m| m.tag.clone()).collect();
                    let actual: Vec<Option<String>> =
                        ev.marks.iter().map(|m| m.actual_text.clone()).collect();
                    let ocs: Vec<Option<ObjectId>> = ev.marks.iter().map(|m| m.oc).collect();
                    let txt: String = show
                        .glyphs()
                        .filter_map(|g| g.text().map(String::from))
                        .collect();
                    form_text_marks.push((txt, tags, actual, ocs));
                }
                _ => {}
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();

    assert_eq!(dpis.len(), 1);
    let ((hx, hy), (ux, uy)) = (dpis[0].0.unwrap(), dpis[0].1.unwrap());
    assert_approx(hx, 100.0);
    assert_approx(hy, 100.0);
    assert_approx(ux, 50.0);
    assert_approx(uy, 50.0);
    assert_eq!(inline_count, 1);
    assert_eq!(shadings.len(), 1);
    assert_eq!(shadings[0].shading, Some(shading_id));
    assert_eq!(shadings[0].bounds, Some(Rect::new(0.0, 0.0, 50.0, 50.0)));
    assert_eq!(refused, vec![Refused::Cycle]);
    assert_eq!(form_text_marks.len(), 1);
    assert_eq!(form_text_marks[0].0, "FromForm");
    assert_eq!(
        form_text_marks[0].1,
        vec![b"OC".to_vec(), b"Span".to_vec(), b"Artifact".to_vec()]
    );
    assert_eq!(
        form_text_marks[0].2,
        vec![None, Some("OK".to_string()), None]
    );
    assert_eq!(form_text_marks[0].3, vec![Some(ocg_id), None, None]);
    assert!(faults.iter().any(|(_, f)| *f == Fault::ExtraEndMarked));
    assert!(
        faults
            .iter()
            .any(|(_, f)| *f == Fault::UnclosedMarked { count: 2 })
    );
    assert!(
        faults
            .iter()
            .any(|(_, f)| *f == Fault::FormCycle { form: cyc_id })
    );

    // Inline image with unknown filter (`ImageStream::Missing`) produces 0 Image events in `interpret`.
    let mut trunc_images = 0usize;
    interpret(
        store.as_streams(),
        &Concat::new(&[missing_stream_inline]),
        None,
        &Limits::DEFAULT,
        &fonts,
        Opts::default(),
        |ev| {
            if matches!(ev.kind, Kind::Image(_)) {
                trunc_images += 1;
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();
    assert_eq!(trunc_images, 0);

    // `Forms::Skip` skips entering `Fm1` while still reporting `TopImg` in the top-level stream!
    let mut skip_images = 0usize;
    let sum = interpret(
        store.as_streams(),
        &Concat::new(&[b"/Fm1 Do /TopImg Do"]),
        Some(&resources),
        &Limits::DEFAULT,
        &fonts,
        Opts {
            forms: Forms::Skip,
            wants: Wants::ALL,
        },
        |ev| {
            if matches!(ev.kind, Kind::Image(_)) {
                skip_images += 1;
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();
    assert_eq!(skip_images, 1);
    assert_eq!(sum.uses, 0);
}

#[test]
fn limits_caps_not_finite_unreadable_and_wants_invariance() {
    let mut store = Store::default();
    let f2 = store.put_stream(
        dict! { "Subtype" => Object::name("Form") },
        b"0 0 1 1 re f".to_vec(),
    );
    let f1 = store.put_stream(
        dict! {
            "Subtype" => Object::name("Form"),
            "Resources" => dict! { "XObject" => dict! { "F2" => f2 } },
        },
        b"/F2 Do".to_vec(),
    );
    let res = Object::from(dict! {
        "Font" => dict! { "F1" => Std14::Helvetica.dict() },
        "XObject" => dict! { "F1" => f1 },
    });
    let fonts = Fonts::default();

    // 1. States cap
    let err = interpret(
        store.as_streams(),
        &Concat::new(&[b"q q q Q Q Q"]),
        None,
        &Limits {
            states: 2,
            ..Limits::DEFAULT
        },
        &fonts,
        Opts::default(),
        |_| ControlFlow::Continue(()),
    )
    .unwrap_err();
    assert_eq!(err, Error::Limit(Cap::States));

    // 2. Operators cap
    let err = interpret(
        store.as_streams(),
        &Concat::new(&[b"q Q q Q"]),
        None,
        &Limits {
            operators: 3,
            ..Limits::DEFAULT
        },
        &fonts,
        Opts::default(),
        |_| ControlFlow::Continue(()),
    )
    .unwrap_err();
    assert_eq!(err, Error::Limit(Cap::Operators));

    // 3. Glyphs cap
    let err = interpret(
        store.as_streams(),
        &Concat::new(&[b"BT /F1 12 Tf (Hello) Tj ET"]),
        Some(&res),
        &Limits {
            glyphs: 4,
            ..Limits::DEFAULT
        },
        &fonts,
        Opts::default(),
        |_| ControlFlow::Continue(()),
    )
    .unwrap_err();
    assert_eq!(err, Error::Limit(Cap::Glyphs));

    // 4. Segments cap
    let err = interpret(
        store.as_streams(),
        &Concat::new(&[b"0 0 10 10 re f"]),
        None,
        &Limits {
            segments: 3,
            ..Limits::DEFAULT
        },
        &fonts,
        Opts::default(),
        |_| ControlFlow::Continue(()),
    )
    .unwrap_err();
    assert_eq!(err, Error::Limit(Cap::Segments));

    // 5. Marked cap
    let err = interpret(
        store.as_streams(),
        &Concat::new(&[b"/A BMC /B BMC /C BMC EMC EMC EMC"]),
        None,
        &Limits {
            marked: 2,
            ..Limits::DEFAULT
        },
        &fonts,
        Opts::default(),
        |_| ControlFlow::Continue(()),
    )
    .unwrap_err();
    assert_eq!(err, Error::Limit(Cap::Marked));

    // 6. Interpreted byte cap
    let err = interpret(
        store.as_streams(),
        &Concat::new(&[b"0 0 10 10 re f"]),
        None,
        &Limits {
            interpreted: 5,
            ..Limits::DEFAULT
        },
        &fonts,
        Opts::default(),
        |_| ControlFlow::Continue(()),
    )
    .unwrap_err();
    assert_eq!(err, Error::Limit(Cap::Interpreted));

    // 7. Decompressed byte cap on Form XObject
    let err = interpret(
        store.as_streams(),
        &Concat::new(&[b"/F1 Do"]),
        Some(&res),
        &Limits {
            decompressed: 10,
            ..Limits::DEFAULT
        },
        &fonts,
        Opts::default(),
        |_| ControlFlow::Continue(()),
    )
    .unwrap_err();
    assert_eq!(err, Error::Limit(Cap::Decompressed));

    // 8. FormDepth cap emits Refused::Depth + Fault::FormDepth
    let mut faults = Vec::new();
    let mut depth_refused = 0usize;
    interpret_with(
        store.as_streams(),
        &Concat::new(&[b"/F1 Do"]),
        Some(&res),
        &Limits {
            form_depth: 1,
            ..Limits::DEFAULT
        },
        &fonts,
        Opts::default(),
        &mut faults,
        |ev| {
            if matches!(
                ev.kind,
                Kind::FormRefused {
                    why: Refused::Depth,
                    ..
                }
            ) {
                depth_refused += 1;
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();
    assert_eq!(depth_refused, 1);
    assert!(
        faults
            .iter()
            .any(|(_, f)| *f == Fault::FormDepth { form: f2 })
    );

    // 9. NotFinite on CTM overflow (10^20 * 10^20 = 10^40 > f32::MAX)
    let err = interpret(
        store.as_streams(),
        &Concat::new(&[
            b"100000000000000000000.0 0 0 100000000000000000000.0 0 0 cm 100000000000000000000.0 0 0 100000000000000000000.0 0 0 cm 0 0 1 1 re f",
        ]),
        None,
        &Limits::DEFAULT,
        &fonts,
        Opts::default(),
        |_| ControlFlow::Continue(()),
    )
    .unwrap_err();
    assert_eq!(err, Error::NotFinite);

    // 10. Unreadable::Locked propagates immediately as Error::Unreadable
    let locked_streams = WithStreams::new(&store.objects, |id, _| {
        Err(Unreadable::new(id, Why::Locked, "encrypted"))
    });
    let err = interpret(
        locked_streams,
        &Concat::new(&[b"/F1 Do"]),
        Some(&res),
        &Limits::DEFAULT,
        &fonts,
        Opts::default(),
        |_| ControlFlow::Continue(()),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Unreadable(u) if u.why == Why::Locked));

    // 11. ControlFlow::Break stops cleanly
    let mut seen = 0usize;
    let sum = interpret(
        store.as_streams(),
        &Concat::new(&[b"0 0 1 1 re f 2 2 1 1 re f 4 4 1 1 re f"]),
        None,
        &Limits::DEFAULT,
        &fonts,
        Opts::default(),
        |ev| {
            if matches!(ev.kind, Kind::Path(_)) {
                seen += 1;
                return ControlFlow::Break(());
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();
    assert_eq!(seen, 1);
    assert!(sum.stopped);
}

#[test]
fn encode_rewrites_relative_text_moves_into_position_independent_runs() {
    let mut store = Store::default();
    let form_id = store.put_stream(
        dict! {
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(300), Object::from(300)],
        },
        br#"
            BT
            /F1 10 Tf
            14 TL
            10 200 Td (FormLine1) Tj
            0 -14 TD (FormLine2) '
            1.5 0.5 (FormLine3 a b) "
            ET
        "#
        .to_vec(),
    );
    let resources = Object::from(dict! {
        "Font" => dict! { "F1" => Std14::Helvetica.dict(), "F2" => Std14::Courier.dict() },
        "XObject" => dict! { "Fm1" => form_id },
    });

    let original = Concat::new(&[br#"
        q 1 0 0 1 5 10 cm
        BT
        /F1 12 Tf
        16 TL
        36 700 Td (Line 1) Tj
        T* (Line 2) Tj
        10 -20 TD (Line 3) '
        2 0.5 (Line 4 with words) "
        [(Ke) 120 (rn)] TJ
        ET
        /Fm1 Do
        Q
    "#]);

    let fonts = Fonts::default();
    let collect_glyphs = |st: &Store, content: &Concat<'_>| -> Vec<(Option<String>, [f32; 8])> {
        let mut out = Vec::new();
        interpret(
            st.as_streams(),
            content,
            Some(&resources),
            &Limits::DEFAULT,
            &fonts,
            Opts::default(),
            |ev| {
                if let Kind::Text(show) = ev.kind {
                    for g in show.glyphs() {
                        let pts = g.quad.0;
                        out.push((
                            g.text().map(String::from),
                            [
                                pts[0].x, pts[0].y, pts[1].x, pts[1].y, pts[2].x, pts[2].y,
                                pts[3].x, pts[3].y,
                            ],
                        ));
                    }
                }
                ControlFlow::Continue(())
            },
        )
        .unwrap();
        out
    };

    let before = collect_glyphs(&store, &original);
    let encoded = encode_with(
        store.as_streams(),
        &original,
        Some(&resources),
        &Limits::DEFAULT,
        &fonts,
        &mut (),
    )
    .unwrap();

    // Encoded page and Form stream must contain zero relative line-moving operators (`T*`, `TD`, `'`, `"`).
    for bytes in std::iter::once(&encoded.content).chain(encoded.forms.iter().map(|(_, b)| b)) {
        let res_cx = pdf_content_streams::ResCx::default();
        for op in pdf_content_streams::operations(bytes, &res_cx, &Limits::DEFAULT.content) {
            let op = op.unwrap();
            assert!(
                !matches!(
                    op.operator(),
                    pdf_content_streams::Operator::NextLine
                        | pdf_content_streams::Operator::MoveLineLeading
                        | pdf_content_streams::Operator::NextLineShow
                        | pdf_content_streams::Operator::NextLineShowSpaced
                ),
                "unexpected relative operator {:?} left in encoded stream",
                op.operator()
            );
        }
    }

    let mut store2 = store;
    let form_bytes = encoded
        .forms
        .iter()
        .find_map(|(id, b)| (*id == form_id).then(|| b.clone()))
        .expect("form must be encoded");
    store2.streams.insert(form_id, form_bytes.clone());
    store2.objects.insert(
        form_id,
        Object::from(Stream::new(
            dict! {
                "Subtype" => Object::name("Form"),
                "BBox" => vec![Object::from(0), Object::from(0), Object::from(300), Object::from(300)],
            },
            form_bytes,
        )),
    );
    let after = collect_glyphs(&store2, &Concat::new(&[&encoded.content]));
    assert_eq!(
        before, after,
        "encode must preserve every glyph quad bit-for-bit"
    );
}

#[test]
fn encode_rejects_unwritable_inline_images_and_undecoded_page_streams() {
    let store = Store::default();
    let fonts = Fonts::default();

    // 1. Unknown filter (`ImageStream::Missing`) -> Unwritable::InlineStream
    let err = encode(
        store.as_streams(),
        &Concat::new(&[b"BI /W 1 /H 1 /BPC 8 /CS /G /F /XYZ ID xy EI"]),
        None,
        &Limits::DEFAULT,
        &fonts,
    )
    .unwrap_err();
    assert!(matches!(
        err,
        Error::Unwritable {
            why: Unwritable::InlineStream,
            ..
        }
    ));

    // 2. Mismatched /L -> Unwritable::InlineLength
    let err = encode(
        store.as_streams(),
        &Concat::new(&[b"BI /W 1 /H 1 /BPC 8 /CS /G /L 99 ID a EI"]),
        None,
        &Limits::DEFAULT,
        &fonts,
    )
    .unwrap_err();
    assert!(matches!(
        err,
        Error::Unwritable {
            why: Unwritable::InlineLength,
            ..
        }
    ));

    // 3. Undecoded page stream -> Error::PageStream
    let mut bad_store = Store::default();
    let bad_id = bad_store.alloc();
    bad_store
        .objects
        .insert(bad_id, Object::from(Stream::new(dict! {}, Vec::new())));
    let mut join = Join::new(&Limits::DEFAULT.content);
    join.push_or_empty(&bad_store.as_streams(), bad_id).unwrap();
    let concat_with_undecoded = join.finish();
    let err = encode(
        bad_store.as_streams(),
        &concat_with_undecoded,
        None,
        &Limits::DEFAULT,
        &fonts,
    )
    .unwrap_err();
    assert!(matches!(err, Error::PageStream(_)));
}

#[test]
fn canvas_shapes_borders_xobjects_marks_and_page_geom_integration() {
    let mut store = Store::default();
    let img_obj = store.put_stream(
        dict! {
            "Subtype" => Object::name("Image"),
            "Width" => 300,
            "Height" => 150,
            "BitsPerComponent" => 8,
        },
        vec![0; 8],
    );
    let form_obj = store.put_stream(
        dict! {
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(50), Object::from(25)],
        },
        b"0 0 50 25 re f".to_vec(),
    );

    let font_res = Res::dict(Std14::Helvetica.dict()).unwrap();
    let img_res = Res::written(img_obj);
    let form_res = Res::written(form_obj);
    let props_res = Res::dict(dict! { "MCID" => 1 }).unwrap();

    let mut gs = GState::default();
    gs.fill_alpha = Some(0.75);

    let mut canvas = Canvas::new();
    canvas.with_state(|c| {
        c.gstate(gs);
        c.line_width(2.0);
        c.line_cap(LineCap::Round);
        c.line_join(LineJoin::Round);
        c.miter_limit(5.0);
        c.dash(&[3.0, 1.0], 0.0);
        c.clip_rect(Rect::from_xywh(0.0, 0.0, 612.0, 792.0));
        c.fill_color(Color::Gray(0.9));
        c.stroke_color(Color::rgb8([20, 40, 80]));
        c.fill_stroke_rect(Rect::from_xywh(10.0, 10.0, 100.0, 50.0));
        c.border_rect(Rect::from_xywh(120.0, 10.0, 80.0, 40.0), 4.0);
        c.rounded_rect(Rect::from_xywh(10.0, 80.0, 100.0, 40.0), 8.0, Paint::Fill);
        c.circle(Point::new(180.0, 100.0), 20.0, Paint::Stroke);
        c.ellipse(Rect::from_xywh(220.0, 80.0, 60.0, 30.0), Paint::FillEvenOdd);
        let curve = [
            Seg::Move(Point::new(300.0, 80.0)),
            Seg::Line(Point::new(340.0, 80.0)),
            Seg::Curve(
                Point::new(350.0, 90.0),
                Point::new(350.0, 100.0),
                Point::new(340.0, 110.0),
            ),
            Seg::Close,
        ];
        c.draw_segments(&curve, Paint::Stroke);
        c.clip_path(&curve, FillRule::EvenOdd);
    });

    assert!(canvas.place_image(
        img_res.clone(),
        Rect::from_xywh(36.0, 200.0, 150.0, 75.0),
        Rotation::R0
    ));
    assert!(canvas.place_form(
        form_res.clone(),
        Rect::from_xywh(0.0, 0.0, 50.0, 25.0),
        Rect::from_xywh(220.0, 200.0, 100.0, 50.0),
        Rotation::R90,
    ));
    canvas.with_tagged("Artifact", |c| {
        c.text_with_matrix(
            font_res.clone(),
            12.0,
            Matrix::translate(36.0, 400.0),
            b"Header",
        );
    });
    canvas.with_marked("P", props_res.clone(), |c| {
        c.text_kerned(
            font_res.clone(),
            14.0,
            Matrix::translate(36.0, 360.0),
            TextMode::Fill,
            &[
                TextPiece::Codes(b"AV"),
                TextPiece::Adjust(80.0),
                TextPiece::Codes(b"A"),
            ],
        );
    });

    let (bytes, needs) = canvas.finish(&Reserved::default());
    let mut fonts_dict = Dict::new();
    let mut gs_dict = Dict::new();
    let mut xo_dict = Dict::new();
    let mut prop_dict = Dict::new();
    for need in &needs.0 {
        if need.res == font_res {
            fonts_dict.set(need.name.clone(), Std14::Helvetica.dict());
        } else if need.res == img_res {
            xo_dict.set(need.name.clone(), img_obj);
        } else if need.res == form_res {
            xo_dict.set(need.name.clone(), form_obj);
        } else if need.res == props_res {
            prop_dict.set(need.name.clone(), dict! { "MCID" => 1 });
        } else {
            gs_dict.set(
                need.name.clone(),
                dict! { "Type" => Object::name("ExtGState"), "ca" => 0.75 },
            );
        }
    }
    let resources = Object::from(dict! {
        "Font" => fonts_dict,
        "ExtGState" => gs_dict,
        "XObject" => xo_dict,
        "Properties" => prop_dict,
    });

    let mut seen_images = 0usize;
    let mut seen_paths = 0usize;
    let summary = interpret(
        store.as_streams(),
        &Concat::new(&[&bytes]),
        Some(&resources),
        &Limits::DEFAULT,
        &Fonts::default(),
        Opts::default(),
        |ev| {
            match ev.kind {
                Kind::Image(_) => seen_images += 1,
                Kind::Path(_) => seen_paths += 1,
                _ => {}
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();
    assert_eq!(summary.uses, 1);
    assert_eq!(seen_images, 1);
    assert_eq!(summary.glyphs, 9);
    assert!(seen_paths >= 8);

    // Verify PageGeom::from_page against pdf-semantics's Page
    let page_id = store.put(Object::from(dict! {
        "Type" => Object::name("Page"),
        "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(612), Object::from(792)],
        "CropBox" => vec![Object::from(18), Object::from(18), Object::from(594), Object::from(774)],
        "TrimBox" => vec![Object::from(36), Object::from(36), Object::from(576), Object::from(756)],
        "Rotate" => 90,
        "UserUnit" => 2.0,
    }));
    let root_id = store.put(Object::from(dict! {
        "Type" => Object::name("Pages"),
        "Kids" => vec![Object::from(page_id)],
        "Count" => 1,
    }));
    if let Some(p) = store
        .objects
        .get_mut(&page_id)
        .and_then(Object::as_dict_mut)
    {
        p.set("Parent", root_id);
    }
    let catalog = dict! { "Pages" => root_id };
    let table = pages::pages(
        &store.objects,
        &catalog,
        Rules::Pdfium,
        &pdf_semantics::Limits::DEFAULT,
        &mut (),
    )
    .unwrap();
    let geom = PageGeom::from_page(table.get(0).unwrap());
    assert_eq!(geom.rotation, Rotation::R90);
    assert_eq!(geom.user_unit, 2.0);
    assert_eq!(geom.trim_box, Some(Rect::new(36.0, 36.0, 576.0, 756.0)));
    assert_eq!(geom.view_size().width, 756.0);
    assert_eq!(geom.view_size().height, 576.0);
}

#[test]
fn fuzz_harness_corpus_smoke_and_prng_invariants() {
    for seed in fuzz::SEEDS {
        fuzz::check(seed);
    }

    // Exercise 64 deterministic pseudo-random mutations around operator tokens.
    let tokens = [
        &b"q "[..],
        b"Q ",
        b"1 0 0 1 2 3 cm ",
        b"0 0 10 10 re ",
        b"f ",
        b"S ",
        b"W n ",
        b"BT ",
        b"ET ",
        b"/F1 10 Tf ",
        b"12 TL ",
        b"5 -10 Td ",
        b"0 -12 TD ",
        b"T* ",
        b"(ab) Tj ",
        b"[(a) 100 () -50 (b)] TJ ",
        b"(c) ' ",
        b"1 2 (d e) \" ",
        b"/Tag BMC ",
        b"EMC ",
        b"0.5 g ",
        b"2 w ",
        b"1 J ",
    ];
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    for _ in 0..64 {
        let mut buf = Vec::new();
        for _ in 0..24 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let idx = (state >> 32) as usize % tokens.len();
            buf.extend_from_slice(tokens[idx]);
        }
        buf.extend_from_slice(
            b"\xff<< /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Courier >> >> >>",
        );
        fuzz::check(&buf);
    }
}

#[cfg(feature = "programs")]
fn build_wide_truetype_a() -> Vec<u8> {
    fn be(values: &[i16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }
    fn simple_rect(x0: i16, y0: i16, x1: i16, y1: i16) -> Vec<u8> {
        let pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
        let mut g = be(&[1, x0, y0, x1, y1, 3, 0]);
        g.extend([1u8; 4]);
        let mut last = (0i16, 0i16);
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for (px, py) in pts {
            xs.push(px - last.0);
            ys.push(py - last.1);
            last = (px, py);
        }
        g.extend(be(&xs));
        g.extend(be(&ys));
        g
    }

    let glyphs = [
        simple_rect(0, 0, 500, 700),
        simple_rect(-200, -400, 1200, 1100),
    ];
    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    for glyph in &glyphs {
        loca.extend((glyf.len() as u32).to_be_bytes());
        glyf.extend(glyph);
        glyf.resize(glyf.len().next_multiple_of(4), 0);
    }
    loca.extend((glyf.len() as u32).to_be_bytes());

    let head = [
        &0x0001_0000u32.to_be_bytes()[..],
        &0x0001_0000u32.to_be_bytes(),
        &0u32.to_be_bytes(),
        &0x5F0F_3CF5u32.to_be_bytes(),
        &be(&[0, 1000]),
        &[0; 16],
        &be(&[-200, -400, 1200, 1100, 0, 8, 2, 1, 0]),
    ]
    .concat();
    let hhea = [
        &0x0001_0000u32.to_be_bytes()[..],
        &be(&[800, -200, 0, 1200, 0, 0, 1200, 1, 0, 0, 0, 0, 0, 0, 0, 2]),
    ]
    .concat();
    let hmtx = be(&[500, 0, 600, -200]);
    let maxp = [
        &0x0001_0000u32.to_be_bytes()[..],
        &be(&[2, 4, 1, 0, 0, 2, 0, 0, 0, 0, 0, 0, 1, 1]),
    ]
    .concat();
    let post = [&0x0003_0000u32.to_be_bytes()[..], &[0; 28]].concat();

    // Format 4 cmap mapping 0x0041 ('A') -> glyph 1
    let ends = [0x0041u16, 0xFFFF];
    let deltas = [1u16.wrapping_sub(0x0041), 1];
    let mut sub = Vec::new();
    for v in [4u16, 32, 0, 4, 0, 0, 0] {
        sub.extend(v.to_be_bytes());
    }
    for v in ends.iter().chain([&0u16]).chain(&ends).chain(&deltas) {
        sub.extend(v.to_be_bytes());
    }
    sub.extend([0u8; 4]);
    let cmap = [&[0, 0, 0, 1, 0, 3, 0, 1, 0, 0, 0, 12][..], &sub].concat();

    let mut tables = [
        (*b"cmap", cmap),
        (*b"glyf", glyf),
        (*b"head", head),
        (*b"hhea", hhea),
        (*b"hmtx", hmtx),
        (*b"loca", loca),
        (*b"maxp", maxp),
        (*b"post", post),
    ];
    tables.sort_by_key(|t| t.0);
    let mut out = [
        &0x0001_0000u32.to_be_bytes()[..],
        &be(&[tables.len() as i16, 0, 0, 0]),
    ]
    .concat();
    let mut offset = 12 + 16 * tables.len();
    let mut data = Vec::new();
    for (tag, table) in &tables {
        out.extend(tag);
        out.extend(0u32.to_be_bytes());
        out.extend((offset as u32).to_be_bytes());
        out.extend((table.len() as u32).to_be_bytes());
        data.extend(table);
        data.resize(data.len().next_multiple_of(4), 0);
        offset = 12 + 16 * tables.len() + data.len();
    }
    out.extend(data);
    out
}

#[cfg(feature = "text-layout")]
#[test]
fn text_page_dehyphenation_hit_testing_bounded_text_and_options() {
    use crate::{SearchOpts, TextPage, TextPageOpts};

    let mut store = Store::default();
    let form_id = store.put_stream(
        dict! {
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(200), Object::from(200)],
        },
        b"BT /F1 12 Tf 72 620 Td (FormNote) Tj ET".to_vec(),
    );
    let resources = Object::from(dict! {
        "Font" => dict! { "F1" => Std14::Helvetica.dict() },
        "XObject" => dict! { "Fm1" => form_id },
    });

    let stream = br#"
        BT
        /F1 12 Tf
        72 700 Td (inter-) Tj
        0 -16 Td (national trade) Tj
        3 Tr
        0 -16 Td (HiddenOCR) Tj
        0 Tr
        ET
        /Fm1 Do
    "#;
    let concat = Concat::new(&[stream]);
    let fonts = Fonts::default();

    let tp = TextPage::extract(
        store.as_streams(),
        &concat,
        Some(&resources),
        &Limits::DEFAULT,
        &fonts,
        TextPageOpts::default(),
    )
    .unwrap();

    assert_eq!(tp.text(), "inter-\nnational trade\nHiddenOCR\nFormNote");
    assert_eq!(
        tp.text_dehyphenated(),
        "international trade\nHiddenOCR\nFormNote"
    );
    assert_eq!(tp.lines().len(), 4);

    // Default SearchOpts (`ignore_soft_hyphens: true`) matches across the line wrap
    // and produces 2 selection quads (one on line 0, one on line 1)!
    let hits = tp.search("international", SearchOpts::default());
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].quads.len(), 2);
    assert!(tp.search("international", SearchOpts::PDFIUM).is_empty());

    // Hit-testing finds `'i'` near `(73, 704)` and returns `None` far away.
    let idx = tp
        .char_index_at(Point::new(73.0, 704.0), 2.0, 2.0)
        .expect("character hit at first glyph");
    assert_eq!(tp.chars()[idx].ch, 'i');
    assert_eq!(tp.char_index_at(Point::new(500.0, 500.0), 2.0, 2.0), None);

    // Bounded rectangle query extracts only the top two lines.
    assert_eq!(
        tp.text_in_rect(Rect::new(60.0, 680.0, 250.0, 720.0)),
        "inter-\nnational trade"
    );

    // `include_invisible: false` omits `3 Tr` OCR text, and `Forms::Skip` omits `/Fm1`.
    let visible_top = TextPage::extract(
        store.as_streams(),
        &concat,
        Some(&resources),
        &Limits::DEFAULT,
        &fonts,
        TextPageOpts {
            forms: Forms::Skip,
            include_invisible: false,
            ..TextPageOpts::default()
        },
    )
    .unwrap();
    assert_eq!(visible_top.text(), "inter-\nnational trade");
}

#[cfg(feature = "text-layout")]
#[test]
fn canvas_text_box_wrapping_alignment_autosize_comb_and_custom_fonts() {
    use crate::{
        FontEncoder, TextAlign, TextBoxStyle, TextPage, TextPageOpts, VerticalAlign, WrapMode,
        layout_text,
    };

    let font_res = Res::dict(Std14::Helvetica.dict()).unwrap();
    let box_rect = Rect::from_xywh(36.0, 600.0, 140.0, 100.0);

    // 1. Word-wrapped justified paragraph: wrapped lines carry `word_space > 0.0`,
    //    while the paragraph's final line stays left-aligned (`word_space == 0.0`).
    let para = "The quick brown fox jumps over the lazy dog.\nSecond short line.";
    let justified_style = TextBoxStyle {
        size: Some(11.0),
        align: TextAlign::Justify,
        vertical_align: VerticalAlign::Top,
        wrap: WrapMode::Word,
        padding: 4.0,
        ..TextBoxStyle::default()
    };
    let mut canvas = Canvas::new();
    let laid = canvas
        .text_box(
            font_res.clone(),
            &Std14::Helvetica,
            para,
            box_rect,
            &justified_style,
        )
        .unwrap();
    assert!(!laid.overflowed);
    assert!(laid.lines.len() >= 3);
    assert!(laid.lines[0].word_space > 0.0);
    assert_eq!(laid.lines.last().unwrap().word_space, 0.0);

    // 2. Auto-sized (`size: None`) centered single-line banner inside a tight box.
    let auto_style = TextBoxStyle {
        size: None,
        min_auto_size: 4.0,
        max_auto_size: 18.0,
        align: TextAlign::Center,
        vertical_align: VerticalAlign::Middle,
        wrap: WrapMode::NoWrap,
        ..TextBoxStyle::default()
    };
    let auto_laid = canvas
        .text_box(
            font_res.clone(),
            &Std14::Helvetica,
            "Auto-fit title",
            Rect::from_xywh(36.0, 540.0, 60.0, 20.0),
            &auto_style,
        )
        .unwrap();
    assert!(!auto_laid.overflowed);
    assert!(auto_laid.font_size >= 4.0 && auto_laid.font_size < 18.0);

    // 3. Right-aligned 6-cell comb field (`"42"` lands in cells 4 and 5).
    let comb_style = TextBoxStyle {
        size: Some(12.0),
        align: TextAlign::Right,
        vertical_align: VerticalAlign::Middle,
        comb: Some(6),
        ..TextBoxStyle::default()
    };
    let comb_rect = Rect::from_xywh(36.0, 500.0, 120.0, 24.0);
    let comb_laid = canvas
        .text_box(
            font_res.clone(),
            &Std14::Courier,
            "42",
            comb_rect,
            &comb_style,
        )
        .unwrap();
    assert!(!comb_laid.overflowed);
    assert_eq!(comb_laid.lines[0].cells.len(), 2);
    assert!(comb_laid.lines[0].cells[0].0.x > 36.0 + 80.0);

    // 4. Unencodable character returns Err without mutating the canvas.
    let (before_bytes, _) = canvas.named(&Reserved::default());
    let err = canvas
        .text_box(
            font_res.clone(),
            &Std14::Helvetica,
            "ok \u{65E5}",
            box_rect,
            &TextBoxStyle::default(),
        )
        .unwrap_err();
    assert_eq!(err.at, 3);
    assert_eq!(err.character, '\u{65E5}');
    let (after_bytes, needs) = canvas.finish(&Reserved::default());
    assert_eq!(before_bytes, after_bytes);

    // Round-trip the canvas back through `TextPage::extract`!
    let mut font_dict = Dict::new();
    for need in &needs.0 {
        font_dict.set(need.name.clone(), Std14::Helvetica.dict());
    }
    let resources = Object::from(dict! { "Font" => font_dict });
    let store = Store::default();
    let tp = TextPage::extract(
        store.as_streams(),
        &Concat::new(&[&after_bytes]),
        Some(&resources),
        &Limits::DEFAULT,
        &Fonts::default(),
        TextPageOpts::default(),
    )
    .unwrap();
    assert!(tp.text().contains("The quick brown fox"));
    assert!(tp.text().contains("Second short line."));
    assert!(tp.text().contains("Auto-fit title"));

    // 5. `FontEncoder` over a 2-byte Type 0 CMap font with `/ToUnicode`.
    let mut store2 = Store::default();
    let to_uni = store2.put_stream(
        dict! {},
        b"1 begincodespacerange <0000> <FFFF> endcodespacerange\n\
          2 beginbfchar\n<0001> <65E5>\n<0002> <672C>\nendbfchar"
            .to_vec(),
    );
    let cid_font = store2.put(Object::from(dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("CIDFontType2"),
        "BaseFont" => Object::name("TestJP"),
        "CIDSystemInfo" => dict! {
            "Registry" => Object::text("Adobe"),
            "Ordering" => Object::text("Identity"),
            "Supplement" => 0,
        },
        "DW" => 1000,
    }));
    let type0_dict = dict! {
        "Type" => Object::name("Font"),
        "Subtype" => Object::name("Type0"),
        "BaseFont" => Object::name("TestJP"),
        "Encoding" => Object::name("Identity-H"),
        "DescendantFonts" => vec![Object::from(cid_font)],
        "ToUnicode" => to_uni,
    };
    let mut budget = 1 << 20;
    let parsed_type0 = pdf_fonts::Font::read(
        store2.as_streams(),
        &type0_dict,
        &pdf_fonts::Limits::DEFAULT,
        &mut budget,
        &mut (),
    )
    .unwrap();
    let encoder = FontEncoder::new(&parsed_type0);
    let cjk_laid = layout_text(
        &encoder,
        "\u{65E5}\u{672C}",
        Rect::from_xywh(0.0, 0.0, 30.0, 40.0),
        &TextBoxStyle {
            size: Some(20.0),
            wrap: WrapMode::Char,
            ..TextBoxStyle::default()
        },
    )
    .unwrap();
    // At size 20 with DW 1000, each glyph is 20pt wide -> wraps into 2 lines inside 30pt!
    assert_eq!(cjk_laid.lines.len(), 2);
    assert_eq!(cjk_laid.lines[0].codes, vec![0x00, 0x01]);
    assert_eq!(cjk_laid.lines[1].codes, vec![0x00, 0x02]);
}

#[test]
fn encode_refuses_lone_bi_and_unstable_inline_image_ends() {
    let store = Store::default();
    let fonts = Fonts::default();

    // 1. Lone `BI` followed by `"` (`NextLineShowSpaced`) must be refused with
    //    `Unwritable::LoneBi` rather than turning into an inline image after `"`.
    for raw in [
        &b"BI /W 1 /$\x00\"\x00\x00\x00\x00\x80\x00\x00x EI"[..],
        &b"BI /SMask /None/W 1 / W* BI /W 1 /W n /Fm1 C\x01\x00\x00\x00\x00\x00\x00\"] I /CS /] ID x E\x01\x00\x00\x00\x00\x00\x00\x05KC 8 /CS /] ID x D x EIH;1 /BPKC 8 /CS /] ID x EI"[..],
    ] {
        crate::fuzz::check(raw);
        let err = encode(
            store.as_streams(),
            &Concat::new(&[raw]),
            None,
            &Limits::DEFAULT,
            &fonts,
        )
        .unwrap_err();
        assert_eq!(
            err,
            Error::Unwritable {
                form: None,
                at: 0,
                why: Unwritable::LoneBi,
            }
        );
    }

    // 2. Trailing Flate `FDICT` header (`x `) peeking 2 bytes past `EI`:
    //    without trailing operations after `EI`, writing only `\n` drops the
    //    stream on read-back -> refused with `Unwritable::InlineStream`.
    let trailing_fdict = b"BI /F /Fl ID x EI\n\n";
    crate::fuzz::check(trailing_fdict);
    let err = encode(
        store.as_streams(),
        &Concat::new(&[trailing_fdict]),
        None,
        &Limits::DEFAULT,
        &fonts,
    )
    .unwrap_err();
    assert_eq!(
        err,
        Error::Unwritable {
            form: None,
            at: 0,
            why: Unwritable::InlineStream,
        }
    );

    //    When followed by `q Q`, enough bytes remain past `EI` so `encode`
    //    succeeds and preserves the inline image!
    let followed_fdict = b"BI /F /Fl ID x EI q Q";
    crate::fuzz::check(followed_fdict);
    let enc = encode(
        store.as_streams(),
        &Concat::new(&[followed_fdict]),
        None,
        &Limits::DEFAULT,
        &fonts,
    )
    .unwrap();
    assert!(!enc.content.is_empty());

    // 3. Form XObject without `/Resources` containing an inline image sized by
    //    inherited `/CS1`: drawn first under 1-component `/DeviceGray` and then
    //    inside a wrapper form whose `/CS1` is 3-component `/DeviceRGB`.
    let mut store2 = Store::default();
    let inner_form = store2.put_stream(
        dict! {
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(20), Object::from(20)],
        },
        b"BI /W 2 /H 1 /BPC 8 /CS /CS1 ID ab EI 1234 EI".to_vec(),
    );
    let outer_form = store2.put_stream(
        dict! {
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(20), Object::from(20)],
            "Resources" => dict! {
                "XObject" => dict! { "Inner" => inner_form },
                "ColorSpace" => dict! { "CS1" => Object::name("DeviceRGB") },
            },
        },
        b"/Inner Do".to_vec(),
    );
    let page_res = Object::from(dict! {
        "XObject" => dict! {
            "Inner" => inner_form,
            "Outer" => outer_form,
        },
        "ColorSpace" => dict! { "CS1" => Object::name("DeviceGray") },
    });
    let err = encode(
        store2.as_streams(),
        &Concat::new(&[b"/Inner Do /Outer Do"]),
        Some(&page_res),
        &Limits::DEFAULT,
        &fonts,
    )
    .unwrap_err();
    assert_eq!(
        err,
        Error::Unwritable {
            form: Some(inner_form),
            at: 0,
            why: Unwritable::InlineStream,
        }
    );
}

#[test]
fn encode_preserves_inherited_spacing_on_reused_form_with_nonfinite_quote_op() {
    let mut store = Store::default();
    let form_id = store.put_stream(
        dict! {
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(200), Object::from(50)],
        },
        b"BT /F1 10 Tf 1e39 0 (A B) \" ET".to_vec(),
    );
    let page_res = Object::from(dict! {
        "XObject" => dict! { "Fm1" => form_id },
        "Font" => dict! {
            "F1" => dict! {
                "Type" => Object::name("Font"),
                "Subtype" => Object::name("Type1"),
                "BaseFont" => Object::name("Helvetica"),
            },
        },
    });
    let page_bytes = b"1 Tw 2 Tc /Fm1 Do 10 Tw 20 Tc /Fm1 Do";
    let fonts = Fonts::default();
    let concat = Concat::new(&[page_bytes]);

    let collect_origins = |s_store: &Store, c: &Concat<'_>| -> Vec<f32> {
        let mut xs = Vec::new();
        interpret(
            s_store.as_streams(),
            c,
            Some(&page_res),
            &Limits::DEFAULT,
            &fonts,
            Opts::default(),
            |ev| {
                if let Kind::Text(show) = &ev.kind {
                    xs.extend(show.glyphs().map(|g| g.origin.x));
                }
                ControlFlow::Continue(())
            },
        )
        .unwrap();
        xs
    };

    let before = collect_origins(&store, &concat);
    let encoded = encode(
        store.as_streams(),
        &concat,
        Some(&page_res),
        &Limits::DEFAULT,
        &fonts,
    )
    .unwrap();
    for (fid, bytes) in encoded.forms {
        store.streams.insert(fid, bytes.clone());
        store.objects.insert(
            fid,
            Object::from(Stream::new(
                dict! {
                    "Subtype" => Object::name("Form"),
                    "BBox" => vec![Object::from(0), Object::from(0), Object::from(200), Object::from(50)],
                },
                bytes,
            )),
        );
    }
    let after = collect_origins(&store, &Concat::new(&[&encoded.content]));
    assert_eq!(before, after);
}
