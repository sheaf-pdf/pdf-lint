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
    ActiveColorSpace, BlendState, BoxBorder, BoxBorderStyle, Canvas, Cap, Clip, ClipPart,
    ColorValues, DashPattern, DefaultAppearance, EncodedPage, Error, Fault, FormUse, Forms,
    ImageDrawn, ImageOf, Kind, Limits, Mark, MarkProps, Marks, Matrix, Opts, PageGeom, PathDrawn,
    Point, Quad, Rect, Refused, RenderingIntent, Rotation, Seg, ShadingDrawn, SoftMask,
    SoftMaskKind, SoftMaskParams, State, Summary, TextPiece, TextState, TransparencyGroup,
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
    check::<SoftMaskParams>();
    check::<TransparencyGroup<'static>>();
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
    check::<BoxBorder<'static>>();
    check::<BoxBorderStyle>();
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
        check::<crate::LaidOutCell>();
        check::<crate::LaidOutLine>();
        check::<crate::LaidOutText>();
        check::<crate::LaidOutRun>();
        check::<crate::TextBoxError>();
        check::<crate::WithFallback<'static, pdf_fonts::Std14, pdf_fonts::Std14>>();
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

/// `Tj`, `'` and `"` show a name's bytes, its `#` escapes decoded, as they
/// show a string's, as PDFium's `GetString` takes them; a `TJ` array's name
/// is no string, and moves nothing.
#[test]
fn a_name_shown_by_tj_and_the_quotes_is_its_bytes() {
    let store = Store::default();
    let resources = helvetica_resources();
    let fonts = Fonts::default();
    let content = Concat::new(&[
        br#"BT /F1 10 Tf 14 TL 10 100 Td /AB Tj /CD ' 1 2 /EF " [(G) /HI (J)] TJ /K#4CM Tj 7 Tj ET"#,
    ]);
    let mut shown: Vec<(Vec<u8>, f32)> = Vec::new();
    interpret(
        store.as_streams(),
        &content,
        Some(&resources),
        &Limits::DEFAULT,
        &fonts,
        Opts::default(),
        |ev| {
            if let Kind::Text(show) = ev.kind {
                let x = show.glyphs().next().map_or(f32::NAN, |g| g.origin.x);
                shown.push((show.string.to_vec(), x));
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();
    let strings: Vec<&[u8]> = shown.iter().map(|(s, _)| s.as_slice()).collect();
    assert_eq!(
        strings,
        [&b"AB"[..], b"CD", b"EF", b"G", b"J", b"KLM"],
        "{shown:?}"
    );
    // `'` and `"` each begin a line at 10; `J` follows `G` with no adjustment.
    assert_approx(shown[1].1, 10.0);
    assert_approx(shown[2].1, 10.0);
    let g_width = 10.0 * 0.778;
    assert_approx(shown[4].1 - shown[3].1, g_width + 2.0);
}

/// `Tj`, `'`, `"`, `Tf`, `Do`, and marked-content tags read `true` and `false`
/// as their written bytes, as PDFium's `GetString` takes them (`pdf-fonts`'s
/// `shown` agrees); a `TJ` array's boolean is no string, and moves nothing.
#[test]
fn booleans_are_strings_where_pdfiums_get_string_reads_them() {
    let mut store = Store::default();
    let form_id = store.put_stream(
        dict! {
            "Type" => Object::name("XObject"),
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(100)],
        },
        b"BT (ok) Tj ET".to_vec(),
    );
    let resources = Object::from(dict! {
        "Font" => dict! {
            "true" => Std14::Helvetica.dict(),
        },
        "XObject" => dict! {
            "false" => form_id,
        },
    });
    let fonts = Fonts::default();
    let content = Concat::new(&[
        br#"true BMC BT true 10 Tf 14 TL 10 100 Td true Tj false ' 1 2 true " [(A) true (B) false] TJ ET false Do EMC"#,
    ]);
    let mut shown: Vec<Vec<u8>> = Vec::new();
    let mut tags: Vec<Vec<u8>> = Vec::new();
    interpret(
        store.as_streams(),
        &content,
        Some(&resources),
        &Limits::DEFAULT,
        &fonts,
        Opts::default(),
        |ev| {
            match ev.kind {
                Kind::Text(show) => shown.push(show.string.to_vec()),
                Kind::MarkedBegin(mark) => tags.push(mark.tag.to_vec()),
                _ => {}
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();
    let strings: Vec<&[u8]> = shown.iter().map(Vec::as_slice).collect();
    assert_eq!(
        strings,
        [&b"true"[..], b"false", b"true", b"A", b"B", b"ok"],
        "{shown:?}"
    );
    assert_eq!(tags, [b"true".to_vec()]);

    let mut budget = 1 << 20;
    let fonts_shown = pdf_fonts::shown(
        store.as_streams(),
        &fonts,
        Some(&resources),
        &content,
        &pdf_content_streams::ResCx::none(),
        &pdf_fonts::Limits::DEFAULT,
        &mut budget,
    );
    assert_eq!(fonts_shown.text(), "truefalsetrueABok");
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

/// A clip's path segments are counted as it's built, each part's once, so
/// that holding [`Limits::segments`] at every path operator never walks the
/// clip: the count is what its parts hold, through a merge, a text part and
/// the clip saved before them; 100,000 clips, each wider than the last so
/// that none merges, are interpreted as quickly as one; and the cap still
/// counts the clip's segments with those of the path being built.
#[test]
fn a_clip_s_segments_are_counted_as_it_is_built() {
    let walked = |clip: &Clip| -> usize {
        clip.parts()
            .map(|part| match part {
                ClipPart::Path { segments, .. } => segments.len(),
                ClipPart::Text { .. } => 0,
            })
            .sum()
    };
    let square = |r: f32| {
        [
            Seg::Move(Point::new(0.0, 0.0)),
            Seg::Line(Point::new(r, 0.0)),
            Seg::Line(Point::new(r, r)),
            Seg::Line(Point::new(0.0, r)),
            Seg::Close,
        ]
    };
    let unclipped = Clip::default();
    let outer = unclipped.with_path(
        &square(100.0),
        FillRule::NonZero,
        Rect::new(0.0, 0.0, 100.0, 100.0),
    );
    // Within the outer square, the inner one takes its place.
    let inner = outer.with_path(
        &square(50.0),
        FillRule::NonZero,
        Rect::new(0.0, 0.0, 50.0, 50.0),
    );
    // A triangle past the inner square cuts it.
    let triangle = [
        Seg::Move(Point::new(0.0, 0.0)),
        Seg::Line(Point::new(60.0, 0.0)),
        Seg::Line(Point::new(30.0, 60.0)),
        Seg::Close,
    ];
    let cut = inner.with_path(
        &triangle,
        FillRule::EvenOdd,
        Rect::new(0.0, 0.0, 60.0, 60.0),
    );
    let glyph = Quad::from_rect(Rect::new(1.0, 1.0, 5.0, 5.0));
    let text = cut.with_text(vec![glyph], 1);
    for (clip, parts, segments) in [
        (&unclipped, 0, 0),
        (&outer, 1, 5),
        (&inner, 1, 5),
        (&cut, 2, 9),
        (&text, 3, 9),
    ] {
        assert_eq!(clip.parts().count(), parts);
        assert_eq!(clip.segments(), segments);
        assert_eq!(clip.segments(), walked(clip));
    }

    let store = Store::default();
    let fonts = Fonts::default();
    let clips = |n: usize| -> Vec<u8> {
        (1..=n)
            .flat_map(|i| format!("0 0 {i} {i} re W n ").into_bytes())
            .collect()
    };
    let run = |content: &[u8], limits: &Limits| {
        let mut ops = 0;
        let read = interpret(
            store.as_streams(),
            &Concat::new(&[content]),
            None,
            limits,
            &fonts,
            Opts::default(),
            |ev| {
                ops += usize::from(matches!(ev.kind, Kind::Op));
                ControlFlow::Continue(())
            },
        );
        (read, ops)
    };
    let (read, _) = run(&clips(100_000), &Limits::DEFAULT);
    assert_eq!(read.unwrap().operators, 300_000);
    // 1,000 clips of 6 segments (`re` is `m`, four `l` and `h`), then a
    // rectangle more being built, which passes 6,005 at its `re`.
    let content = clips(1_001);
    let at = |segments: usize| Limits {
        segments,
        ..Limits::DEFAULT
    };
    let (read, ops) = run(&content, &at(6_005));
    assert_eq!(read.unwrap_err(), Error::Limit(Cap::Segments));
    assert_eq!(ops, 3_001);
    assert!(run(&content, &at(6_006)).0.is_ok());
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

/// A font whose codes are bytes is laid out without its codes split
/// (`text::bytes_end`) to the bit where its codes are laid out one at a time
/// (`text::laid_end`): with any widths, at any size, spaced or not, `Tc` and
/// `Tw` 0 of either sign, which the first leaves out, from where an
/// operator's text starts or on from a string before it.
#[test]
fn byte_codes_end_where_laid_out() {
    /// A deterministic generator of the cases.
    struct Cases(u64);
    impl Cases {
        fn next(&mut self) -> u32 {
            self.0 = (self.0)
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 32) as u32
        }
        fn within(&mut self, lo: f32, hi: f32) -> f32 {
            let unit = f64::from(self.next()) / 4_294_967_296.0;
            lo + (hi - lo) * unit as f32
        }
        fn spacing(&mut self) -> f32 {
            match self.next() % 3 {
                0 => 0.0,
                1 => -0.0,
                _ => self.within(-50.0, 50.0),
            }
        }
    }
    let mut cases = Cases(0x2545_F491_4F6C_DD1D);
    let store = Store::default();
    for case in 0..512 {
        let widths: Vec<Object> = (0..256)
            .map(|_| Object::from(i64::from(cases.next() % 5_000) - 2_000))
            .collect();
        let font = dict! {
            "Type" => Object::name("Font"),
            "Subtype" => Object::name("TrueType"),
            "BaseFont" => Object::name("ABCDEF+Some"),
            "FirstChar" => 0,
            "LastChar" => 255,
            "Widths" => widths,
        };
        let mut budget = 1 << 20;
        let font = pdf_fonts::Font::read(
            store.as_streams(),
            &font,
            &pdf_fonts::Limits::DEFAULT,
            &mut budget,
            &mut (),
        )
        .unwrap();
        let byte_widths = font.byte_widths().expect("a simple font's codes are bytes");
        assert!((0..256u32).all(|c| byte_widths[c as usize].to_bits() == font.width(c).to_bits()));
        let size = match cases.next() % 4 {
            0 => 0.0,
            1 => -0.0,
            2 => 3e38,
            _ => cases.within(-100.0, 100.0),
        };
        let text = TextState {
            size,
            word_space: cases.spacing(),
            char_space: cases.spacing(),
            ..TextState::default()
        };
        let from = match cases.next() % 2 {
            0 => 0.0,
            _ => cases.within(-1_000.0, 1_000.0),
        };
        let len = cases.next() as usize % 64;
        let s: Vec<u8> = (0..len)
            .map(|_| match cases.next() % 4 {
                0 => b' ',
                _ => (cases.next() % 256) as u8,
            })
            .collect();
        let fast = crate::text::bytes_end(byte_widths, &text, &s, from);
        let (laid, glyphs) = crate::text::laid_end(&font, &text, &s, from);
        assert!(
            fast.to_bits() == laid.to_bits() || fast.is_nan() && laid.is_nan(),
            "case {case}: {fast} {laid}"
        );
        assert_eq!(glyphs, s.len());
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
    assert!(comb_laid.lines[0].cells[0].origin.x > 36.0 + 80.0);

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

fn parse_da(da: &[u8]) -> DefaultAppearance {
    DefaultAppearance::parse(da, &pdf_content_streams::Limits::DEFAULT).expect("the /DA parses")
}

#[test]
fn default_appearance_reads_and_writes_font_color_and_each_scalar() {
    let da = parse_da(b"/Helv 12 Tf 0 g");
    assert_eq!(da.font_name.as_deref(), Some(&b"Helv"[..]));
    assert_eq!(da.font_size, Some(12.0));
    assert_eq!(da.color, Some(Color::Gray(0.0)));
    assert_eq!(
        (da.char_space, da.word_space, da.horizontal_scale),
        (None, None, None)
    );
    assert_eq!((da.leading, da.text_mode, da.rise), (None, None, None));
    assert_eq!(
        da,
        DefaultAppearance::new("Helv", 12.0, Some(Color::Gray(0.0)))
    );
    assert_eq!(da.to_bytes(), b"/Helv 12 Tf 0 g");

    // Each text-state scalar by itself: read into its own field, and written back.
    let none = DefaultAppearance::default();
    for (src, want) in [
        (
            &b"1.5 Tc"[..],
            DefaultAppearance {
                char_space: Some(1.5),
                ..none.clone()
            },
        ),
        (
            b"-2 Tw",
            DefaultAppearance {
                word_space: Some(-2.0),
                ..none.clone()
            },
        ),
        (
            b"90 Tz",
            DefaultAppearance {
                horizontal_scale: Some(90.0),
                ..none.clone()
            },
        ),
        (
            b"14 TL",
            DefaultAppearance {
                leading: Some(14.0),
                ..none.clone()
            },
        ),
        (
            b"3 Tr",
            DefaultAppearance {
                text_mode: Some(TextMode::Invisible),
                ..none.clone()
            },
        ),
        (
            b"-0.25 Ts",
            DefaultAppearance {
                rise: Some(-0.25),
                ..none.clone()
            },
        ),
    ] {
        let da = parse_da(src);
        assert_eq!(da, want, "{}", String::from_utf8_lossy(src));
        assert_eq!(da.to_bytes(), src);
    }

    // All of them, written in the one canonical order whatever order they were read in.
    let da = parse_da(b"3 Ts 2 Tr 14 TL 90 Tz -2 Tw 1.5 Tc 0.2 0.4 0.6 rg /F#201 9 Tf");
    assert_eq!(da.font_name.as_deref(), Some(&b"F 1"[..]));
    assert_eq!(da.color, Some(Color::Rgb([0.2, 0.4, 0.6])));
    assert_eq!(da.text_mode, Some(TextMode::FillStroke));
    assert_eq!(
        da.to_bytes(),
        b"/F#201 9 Tf 0.2 0.4 0.6 rg 1.5 Tc -2 Tw 90 Tz 14 TL 2 Tr 3 Ts"
    );

    // The default is empty, and writes nothing; a color alone has no leading space.
    assert_eq!(none.to_bytes(), b"");
    assert_eq!(parse_da(b""), none);
    assert_eq!(parse_da(b"0 0 1 0 k").to_bytes(), b"0 0 1 0 k");
}

#[test]
fn default_appearance_ignores_bad_operands_and_keeps_the_last_of_each() {
    // `Tr` outside 0..=7 sets no mode, and leaves an earlier one standing.
    assert_eq!(parse_da(b"8 Tr").text_mode, None);
    assert_eq!(parse_da(b"-1 Tr").text_mode, None);
    assert_eq!(parse_da(b"1 Tr 9 Tr").text_mode, Some(TextMode::Stroke));
    assert_eq!(parse_da(b"7 Tr").text_mode, Some(TextMode::Clip));
    // A real is cut to its whole part, as the interpreter reads `Tr`.
    assert_eq!(parse_da(b"2.9 Tr").text_mode, Some(TextMode::FillStroke));

    // A real past `f32`'s range reads as infinite: the operator is ignored.
    let huge = format!("1{}.", "0".repeat(45));
    for op in ["Tc", "Tw", "Tz", "TL", "Tr", "Ts"] {
        let da = parse_da(format!("{huge} {op}").as_bytes());
        assert_eq!(da, DefaultAppearance::default(), "{op}");
    }
    let da = parse_da(format!("4 Tc {huge} Tc /Helv {huge} Tf {huge} g 0 {huge} 0 rg").as_bytes());
    assert_eq!(da.char_space, Some(4.0));
    assert_eq!(da.font_name.as_deref(), Some(&b"Helv"[..]));
    assert_eq!(da.font_size, None);
    assert_eq!(da.color, None);
    // Written, a font without a size gets 0.
    assert_eq!(da.to_bytes(), b"/Helv 0 Tf 4 Tc");

    // An operator without the operands it needs is passed over, as is any other operator.
    let da = parse_da(b"Tc Tw 1 0 rg 0 0 1 k BT (x) Tj ET 5 w");
    assert_eq!(da, DefaultAppearance::default());
    // No `Tf` still parses: the caller decides whether that's a fault.
    assert_eq!(parse_da(b"0.5 g").font_name, None);

    // The last color of each kind wins, clamped to 0..=1: the nonstroking one is the text's,
    // and a stroking one never is, whichever comes last.
    assert_eq!(
        parse_da(b"0 g 1 0 0 rg").color,
        Some(Color::Rgb([1.0, 0.0, 0.0]))
    );
    let both = parse_da(b"1 0 0 rg 0.25 G");
    assert_eq!(both.color, Some(Color::Rgb([1.0, 0.0, 0.0])));
    assert_eq!(both.stroke_color, Some(Color::Gray(0.25)));
    let both = parse_da(b"0.1 0.2 0.3 RG 0.5 g 1 G 0 0.5 1 0.25 K");
    assert_eq!(both.color, Some(Color::Gray(0.5)));
    assert_eq!(both.stroke_color, Some(Color::Cmyk([0.0, 0.5, 1.0, 0.25])));
    // Written back in the one order: the text's color, then the stroking one.
    assert_eq!(both.to_bytes(), b"0.5 g 0 0.5 1 0.25 K");
    let stroke = parse_da(b"/Helv 8 Tf 2 -1 0.5 RG");
    assert_eq!(stroke.color, None);
    assert_eq!(stroke.stroke_color, Some(Color::Rgb([1.0, 0.0, 0.5])));
    assert_eq!(stroke.to_bytes(), b"/Helv 8 Tf 1 0 0.5 RG");
    assert_eq!(parse_da(&stroke.to_bytes()), stroke);
    // A stroking operator without its operands is passed over like any other.
    assert_eq!(parse_da(b"G 1 RG 0 0 1 K"), DefaultAppearance::default());
    assert_eq!(
        parse_da(b"2 -1 0.5 rg").color,
        Some(Color::Rgb([1.0, 0.0, 0.5]))
    );
    // The last `Tf` and the last scalar win too.
    let da = parse_da(b"/A 8 Tf 1 Tc /B 10 Tf 2 Tc");
    assert_eq!(da.font_name.as_deref(), Some(&b"B"[..]));
    assert_eq!((da.font_size, da.char_space), (Some(10.0), Some(2.0)));
}

#[test]
fn default_appearance_size_is_none_for_auto_sizing() {
    let da = parse_da(b"/Helv 0 Tf");
    assert_eq!(da.font_size, Some(0.0));
    assert_eq!(da.size(), None);
    assert_eq!(da.to_bytes(), b"/Helv 0 Tf");

    assert_eq!(parse_da(b"/Helv 9.5 Tf").size(), Some(9.5));
    assert_eq!(parse_da(b"/Helv -3 Tf").size(), None);
    assert_eq!(parse_da(b"/Helv -3 Tf").font_size, Some(-3.0));
    assert_eq!(parse_da(b"0 g").size(), None);
    assert_eq!(DefaultAppearance::new("F", f32::NAN, None).size(), None);
    assert_eq!(
        DefaultAppearance::new("F", f32::INFINITY, None).size(),
        None
    );
}

/// Symbol lays out the Greek letters and the composed angle brackets at the
/// codes of the signs that draw them, at those glyphs' widths; every other
/// character of every standard 14 font is laid out as its encoding's code for
/// it, and one the encoding hasn't is refused with nothing written.
#[cfg(feature = "text-layout")]
#[test]
fn symbol_lays_out_greek_letters_and_composed_angle_brackets() {
    use crate::pdf_fonts::Encoding;
    use crate::{LayoutFont, TextBoxStyle, layout_text};

    // What text has, the character Symbol's encoding has for it, its code.
    const GIVEN: [(char, char, u8); 5] = [
        ('\u{0394}', '\u{2206}', b'D'),
        ('\u{03A9}', '\u{2126}', b'W'),
        ('\u{03BC}', '\u{00B5}', b'm'),
        ('\u{3008}', '\u{2329}', 0xE1),
        ('\u{3009}', '\u{232A}', 0xF1),
    ];
    for (given, sign, code) in GIVEN {
        assert_eq!(
            Encoding::Symbol.code(sign),
            Some(code),
            "U+{:04X}",
            sign as u32
        );
        assert_eq!(Encoding::Symbol.code(given), None, "U+{:04X}", given as u32);
        for ch in [given, sign] {
            let mut out = vec![0xAA];
            assert!(
                Std14::Symbol.encode_char(ch, &mut out),
                "U+{:04X}",
                ch as u32
            );
            assert_eq!(out, [0xAA, code], "U+{:04X}", ch as u32);
            let glyph = Encoding::Symbol.glyph(code).unwrap();
            let width = Std14::Symbol.glyph_width(glyph);
            assert!(width.is_some_and(|w| w > 0.0), "{glyph}");
            assert_eq!(Std14::Symbol.char_advance(ch), width, "U+{:04X}", ch as u32);
        }
    }

    // Everything else, in every font: the encoding's code and its glyph's
    // width, or refused, `out` untouched.
    for font in Std14::ALL {
        let enc = match font {
            Std14::Symbol => Encoding::Symbol,
            Std14::ZapfDingbats => Encoding::ZapfDingbats,
            _ => Encoding::WinAnsi,
        };
        let symbol = font == Std14::Symbol;
        let mut out = Vec::new();
        // The Basic Multilingual Plane, which holds every character an
        // encoding has, and some past it.
        for ch in ('\0'..='\u{FFFF}').chain(['\u{1D11E}', '\u{E0041}', char::MAX]) {
            if symbol && GIVEN.iter().any(|&(given, ..)| given == ch) {
                continue;
            }
            out.clear();
            let code = enc.code(ch);
            assert_eq!(
                font.encode_char(ch, &mut out),
                code.is_some(),
                "{font:?} {ch:?}"
            );
            assert_eq!(out, Vec::from_iter(code), "{font:?} {ch:?}");
            let width = code.and_then(|c| font.glyph_width(enc.glyph(c)?));
            assert_eq!(font.char_advance(ch), width, "{font:?} {ch:?}");
        }
    }
    // The letters are Symbol's alone: Helvetica has the micro sign, no mu.
    assert_eq!(Std14::Helvetica.char_advance('\u{03BC}'), None);
    assert!(Std14::Helvetica.char_advance('\u{00B5}').is_some());

    // Laid out, composed text is the same codes as the signs.
    let rect = Rect::from_xywh(0.0, 0.0, 400.0, 40.0);
    let style = TextBoxStyle {
        size: Some(10.0),
        ..TextBoxStyle::default()
    };
    let letters = "\u{3008}\u{0394}\u{03A9}\u{03BC}\u{3009}";
    let signs = "\u{2329}\u{2206}\u{2126}\u{00B5}\u{232A}";
    let laid = layout_text(&Std14::Symbol, letters, rect, &style).unwrap();
    assert_eq!(laid.lines.len(), 1);
    assert_eq!(laid.lines[0].codes, [0xE1, b'D', b'W', b'm', 0xF1]);
    let as_signs = layout_text(&Std14::Symbol, signs, rect, &style).unwrap();
    assert_eq!(laid.lines[0].codes, as_signs.lines[0].codes);
    assert_eq!(laid.lines[0].width, as_signs.lines[0].width);
    let refused = layout_text(&Std14::Helvetica, letters, rect, &style).unwrap_err();
    assert_eq!((refused.at, refused.character), (0, '\u{3008}'));
}

#[cfg(feature = "text-layout")]
#[test]
fn default_appearance_text_box_style_maps_the_size_alone() {
    use crate::TextBoxStyle;

    let fixed = parse_da(b"/Helv 9 Tf 3 Tr 20 TL 2 Tc").text_box_style();
    assert_eq!(
        fixed,
        TextBoxStyle {
            size: Some(9.0),
            ..TextBoxStyle::default()
        }
    );
    // `3 Tr` and `TL` are kept for writing back, never applied to the style.
    assert_eq!(fixed.mode, TextMode::Fill);
    assert_eq!(fixed.line_height, None);

    // Exactly 0 asks for auto-sizing, a negative zero among its spellings.
    for auto in [&b"/Helv 0 Tf"[..], b"/Helv -0 Tf", b"/Helv 0.0 Tf"] {
        assert!(parse_da(auto).auto_sized(), "{auto:?}");
        assert_eq!(
            parse_da(auto).text_box_style(),
            TextBoxStyle {
                size: None,
                ..TextBoxStyle::default()
            }
        );
    }
    // No `Tf`, or a size that says nothing: the default style, at its 12.
    assert_eq!(TextBoxStyle::default().size, Some(12.0));
    for unsaid in [&b"/Helv -1 Tf"[..], b"0 g", b""] {
        assert!(!parse_da(unsaid).auto_sized(), "{unsaid:?}");
        assert_eq!(parse_da(unsaid).size(), None);
        assert_eq!(parse_da(unsaid).text_box_style(), TextBoxStyle::default());
    }
    for size in [f32::NAN, f32::INFINITY] {
        let da = DefaultAppearance::new("F", size, None);
        assert!(!da.auto_sized());
        assert_eq!(da.text_box_style(), TextBoxStyle::default());
    }
    assert!(!parse_da(b"/Helv 9 Tf").auto_sized());
}

/// Auto-sized text that fits at the ceiling is drawn at the ceiling itself,
/// not a search's step under it.
#[cfg(feature = "text-layout")]
#[test]
fn auto_size_takes_its_ceiling_where_the_text_fits() {
    use crate::{TextBoxStyle, layout_text};

    let style = TextBoxStyle {
        size: None,
        ..TextBoxStyle::default()
    };
    let roomy = Rect::new(0.0, 0.0, 194.0, 54.0);
    let laid = layout_text(&Std14::Helvetica, "Hi", roomy, &style).unwrap();
    assert!(!laid.overflowed);
    assert_eq!(laid.font_size, 12.0);
    let wider = TextBoxStyle {
        max_auto_size: 30.0,
        ..style
    };
    let laid = layout_text(&Std14::Helvetica, "Hi", roomy, &wider).unwrap();
    assert_eq!(laid.font_size, 30.0);

    // Text that doesn't fit at the ceiling is still searched for under it.
    let tight = Rect::new(0.0, 0.0, 60.0, 12.0);
    let laid = layout_text(&Std14::Helvetica, "Auto-fit title", tight, &style).unwrap();
    assert!(!laid.overflowed);
    assert!(
        laid.font_size > 4.0 && laid.font_size < 12.0,
        "{}",
        laid.font_size
    );
}

#[test]
fn default_appearance_round_trips_what_it_writes() {
    fn name_word(written: &[u8]) -> usize {
        written
            .iter()
            .position(|&b| b == b' ')
            .unwrap_or(written.len())
    }

    // What `to_bytes` writes parses again and writes the same bytes again.
    let again = |src: &[u8]| {
        let written = parse_da(src).to_bytes();
        assert_eq!(parse_da(&written).to_bytes(), written);
        written
    };
    assert_eq!(again(b"/Helv 12 Tf 0 g"), b"/Helv 12 Tf 0 g");
    // Numbers are rounded to four places once, then stay.
    assert_eq!(
        again(b"/F 10.123456 Tf 0.33333333 g 1234.56789 Tc"),
        b"/F 10.1235 Tf 0.3333 g 1234.5679 Tc"
    );
    // A whole number past 32 bits keeps its point, so it reads back as a real.
    assert_eq!(again(b"/F 12345678901. Tf"), b"/F 12345678848. Tf");
    assert_eq!(again(b"-0.00001 Ts"), b"0 Ts");
    // A name's delimiters, spaces, and `#` are escaped, and read back as they were.
    let escaped = again(b"/A#20B#23#28#ff 7 Tf");
    assert_eq!(escaped, b"/A#20B#23#28#FF 7 Tf");
    assert_eq!(
        parse_da(&escaped).font_name.as_deref(),
        Some(&b"A B#(\xff"[..])
    );
    // An empty name is a name.
    assert_eq!(again(b"/ 7 Tf"), b"/ 7 Tf");

    // A name holding a NUL can't be written: `/Helv` stands in, with nothing
    // half-written before it.
    let nul = parse_da(b"/A#00B 7 Tf 1 g");
    assert_eq!(nul.font_name.as_deref(), Some(&b"A\0B"[..]));
    assert_eq!(nul.to_bytes(), b"/Helv 7 Tf 1 g");
    assert_eq!(again(b"/A#00B 7 Tf 1 g"), b"/Helv 7 Tf 1 g");

    // A color set out of range by hand is written clamped, as it is read.
    let out_of_range = DefaultAppearance::new("Helv", 12.0, Some(Color::Gray(5.0)));
    assert_eq!(out_of_range.to_bytes(), b"/Helv 12 Tf 1 g");
    assert_eq!(again(&out_of_range.to_bytes()), b"/Helv 12 Tf 1 g");
    let rgb = DefaultAppearance::new("F", 1.0, Some(Color::Rgb([-1.0, 0.5, f32::NAN])));
    assert_eq!(rgb.to_bytes(), b"/F 1 Tf 0 0.5 0 rg");
    assert_eq!(again(&rgb.to_bytes()), b"/F 1 Tf 0 0.5 0 rg");
    // A size with no name isn't written: `Tf` takes both.
    let mut sized = DefaultAppearance::new("F", 9.0, None);
    sized.font_name = None;
    sized.rise = Some(1.0);
    assert_eq!(sized.to_bytes(), b"1 Ts");

    // A content parser keeps a word's first 255 bytes, so a name whose
    // written form is longer reads back cut short: the one case outside the
    // round trip, which `to_bytes` documents and the fuzz body leaves out.
    let long = DefaultAppearance::new(vec![b' '; 100], 7.0, None);
    let written = long.to_bytes();
    assert_eq!(name_word(&written), 301);
    let cut = parse_da(&written);
    let mut kept = vec![b' '; 84];
    kept.extend_from_slice(b"#2"); // the escape the cut fell in
    assert_eq!(cut.font_name, Some(kept));
    assert_eq!(cut.font_size, Some(7.0));
    // A name that fits, at the edge, holds.
    let fits = DefaultAppearance::new(vec![b'a'; 254], 7.0, None);
    assert_eq!(name_word(&fits.to_bytes()), 255);
    assert_eq!(parse_da(&fits.to_bytes()), fits);

    // Pseudo-random fragments of /DA operators, numbers, and names: the fuzz
    // body's invariant, held here over a fixed sample.
    let tokens = [
        &b"/Helv "[..],
        b"/F#201 ",
        b"/A#00 ",
        b"0 ",
        b"12 ",
        b"-3.14159 ",
        b"0.00004 ",
        b"99999.99995 ",
        b"4294967296 ",
        b"3000000000.5 ",
        b"--5 ",
        b"1.2.3 ",
        b"(s) ",
        b"[1 2] ",
        b"Tf ",
        b"g ",
        b"G ",
        b"rg ",
        b"RG ",
        b"k ",
        b"K ",
        b"Tc ",
        b"Tw ",
        b"Tz ",
        b"TL ",
        b"Tr ",
        b"Ts ",
        b"BT ",
    ];
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut parsed = 0;
    for _ in 0..512 {
        let mut buf = Vec::new();
        for _ in 0..12 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            buf.extend_from_slice(tokens[(state >> 32) as usize % tokens.len()]);
        }
        if DefaultAppearance::parse(&buf, &pdf_content_streams::Limits::DEFAULT).is_some() {
            parsed += 1;
            again(&buf);
        }
        fuzz::check(&buf);
    }
    assert!(parsed > 400, "{parsed} of 512 fragments parsed");
}

#[test]
fn default_appearance_debug_reports_the_names_length_not_the_name() {
    let da = parse_da(b"/SECRET_FONT 12 Tf 0 0 1 rg 2 Tc 1 Tw 90 Tz 14 TL 1 Tr 3 Ts");
    let dbg = format!("{da:?}");
    assert!(!dbg.contains("SECRET"), "{dbg}");
    assert!(!dbg.contains("83, 69, 67"), "{dbg}"); // nor as a list of bytes
    assert_eq!(
        dbg,
        "DefaultAppearance { font_name_len: Some(11), font_size: Some(12.0), \
         color: Some(Rgb([0.0, 0.0, 1.0])), stroke_color: None, char_space: Some(2.0), word_space: Some(1.0), \
         horizontal_scale: Some(90.0), leading: Some(14.0), text_mode: Some(Stroke), \
         rise: Some(3.0) }"
    );
    let pretty = format!("{da:#?}");
    assert!(!pretty.contains("SECRET"), "{pretty}");
    assert!(
        format!("{:?}", DefaultAppearance::default()).contains("font_name_len: None"),
        "an absent name has no length"
    );
}

#[test]
fn default_appearance_parse_is_refused_past_a_content_cap() {
    let src = b"/Helv 12 Tf 0.1 0.2 0.3 rg";
    assert!(DefaultAppearance::parse(src, &pdf_content_streams::Limits::DEFAULT).is_some());

    // `rg` has three operands: a cap of two refuses the string whole.
    let mut limits = pdf_content_streams::Limits::DEFAULT;
    limits.operands = 2;
    assert_eq!(DefaultAppearance::parse(src, &limits), None);
    // Under the same cap, what stays within it parses.
    assert!(DefaultAppearance::parse(b"/Helv 12 Tf 0 g", &limits).is_some());

    // What can't be lexed is refused too.
    assert_eq!(
        DefaultAppearance::parse(
            b"/Helv 12 Tf (unclosed",
            &pdf_content_streams::Limits::DEFAULT
        ),
        None
    );
}

#[test]
fn unreadable_or_damaged_group_bc_and_tr_do_not_disturb_non_rasterizing_callers() {
    use std::collections::BTreeMap;
    use std::ops::ControlFlow;

    use pdf_content_streams::Concat;
    use pdf_fonts::Fonts;
    use pdf_objects::{Object, ObjectId, Stream, Unreadable, Why, dict};

    struct SelectiveResolver {
        good: BTreeMap<ObjectId, Object>,
        damaged_id: ObjectId,
        capped_id: ObjectId,
    }

    impl<'a> pdf_objects::Resolve<'a> for &'a SelectiveResolver {
        fn resolve(&self, id: ObjectId) -> Option<&'a Object> {
            self.good.get(&id)
        }

        fn try_resolve(&self, id: ObjectId) -> Result<Option<&'a Object>, Unreadable> {
            if id == self.damaged_id {
                Err(Unreadable::new(id, Why::Damaged, "damaged object"))
            } else if id == self.capped_id {
                Err(Unreadable::new(id, Why::Capped, "capped object"))
            } else {
                Ok(self.good.get(&id))
            }
        }
    }

    let form_id = ObjectId::new(10, 0);
    let smask_form_id = ObjectId::new(11, 0);
    let damaged_id = ObjectId::new(90, 0);
    let capped_id = ObjectId::new(91, 0);

    let mut good = BTreeMap::new();
    let form_bytes = b"0 0 10 10 re f".to_vec();
    good.insert(
        form_id,
        Object::from(Stream::new(
            dict! {
                "Type" => Object::name("XObject"),
                "Subtype" => Object::name("Form"),
                "BBox" => vec![Object::from(0), Object::from(0), Object::from(10), Object::from(10)],
                "Group" => Object::from(capped_id),
            },
            form_bytes.clone(),
        )),
    );
    good.insert(
        smask_form_id,
        Object::from(Stream::new(
            dict! {
                "Type" => Object::name("XObject"),
                "Subtype" => Object::name("Form"),
                "BBox" => vec![Object::from(0), Object::from(0), Object::from(10), Object::from(10)],
                "Group" => dict! {
                    "S" => Object::name("Transparency"),
                    "I" => Object::from(damaged_id),
                    "K" => Object::from(capped_id),
                },
            },
            form_bytes.clone(),
        )),
    );

    let resolver = SelectiveResolver {
        good,
        damaged_id,
        capped_id,
    };
    let streams = pdf_objects::WithStreams::new(&resolver, |id, _| {
        if id == form_id || id == smask_form_id {
            Ok(form_bytes.clone())
        } else {
            Err(Unreadable::new(id, Why::Damaged, "missing stream"))
        }
    });

    let resources = Object::from(dict! {
        "XObject" => dict! {
            "Fm0" => Object::from(form_id),
            "Fm1" => Object::from(smask_form_id),
        },
        "ExtGState" => dict! {
            "GS0" => dict! {
                "SMask" => dict! {
                    "S" => Object::name("Alpha"),
                    "G" => Object::from(smask_form_id),
                    "BC" => Object::from(damaged_id),
                    "TR" => Object::from(capped_id),
                },
            },
            "GS1" => dict! {
                "SMask" => dict! {
                    "S" => Object::name("Luminosity"),
                    "G" => Object::from(smask_form_id),
                    "BC" => vec![Object::from(0.5), Object::from(capped_id)],
                },
            },
        },
    });

    let content = b"/GS0 gs /GS1 gs /Fm0 Do /Fm1 Do";
    let concat = Concat::new(&[&content[..]]);
    let fonts = Fonts::default();
    let mut faults = Vec::new();
    let mut forms_entered = 0_usize;
    let summary = crate::interpret_with(
        streams,
        &concat,
        Some(&resources),
        &crate::Limits::DEFAULT,
        &fonts,
        crate::Opts::default(),
        &mut faults,
        |ev| {
            if matches!(ev.kind, crate::Kind::FormBegin(_)) {
                forms_entered += 1;
            }
            ControlFlow::Continue(())
        },
    )
    .expect("auxiliary /Group, /BC, and /TR failures must not fail non-rasterizing callers");
    assert_eq!(forms_entered, 2);
    assert!(
        faults.is_empty(),
        "auxiliary /Group, /BC, and /TR must not add Fault::Damaged: {faults:?}"
    );
    assert!(summary.segments >= 2);
}

const BOX_BORDER_STYLES: [BoxBorderStyle; 5] = [
    BoxBorderStyle::Solid,
    BoxBorderStyle::Dashed,
    BoxBorderStyle::Beveled,
    BoxBorderStyle::Inset,
    BoxBorderStyle::Underline,
];

/// What `Canvas::box_border` writes for one border, as text.
fn box_border_text(
    rect: Rect,
    border: &BoxBorder<'_>,
    stroke: Color,
    fill_hint: Option<Color>,
) -> String {
    let mut canvas = Canvas::new();
    canvas.box_border(rect, border, stroke, fill_hint);
    let (bytes, needs) = canvas.finish(&Reserved::default());
    assert!(needs.0.is_empty(), "a border needs no resource");
    String::from_utf8(bytes).unwrap()
}

/// One painted path of an interpreted stream: how it was painted, the state it
/// was painted under, and the box its paint covers (a stroke's half width
/// added: on every side of a closed path's mitered corners, across a single
/// horizontal segment's butt-capped line only).
struct Painted {
    paint: Paint,
    state: State,
    points: Vec<Point>,
    covers: Rect,
}

/// Interprets `content` with no resources, holding that it reads without a
/// fault and that its `q` and `Q` balance, and returns its painted paths.
fn painted_paths(content: &[u8]) -> Vec<Painted> {
    let words = || content.split(u8::is_ascii_whitespace);
    assert_eq!(
        words().filter(|w| w == b"q").count(),
        words().filter(|w| w == b"Q").count(),
        "q and Q balance"
    );
    let store = Store::default();
    let mut faults = Vec::new();
    let mut out = Vec::new();
    let summary = interpret_with(
        store.as_streams(),
        &Concat::new(&[content]),
        None,
        &Limits::DEFAULT,
        &Fonts::default(),
        Opts::default(),
        &mut faults,
        |ev| {
            if let Kind::Path(path) = &ev.kind {
                let points: Vec<Point> = path.segments.iter().flat_map(Seg::points).collect();
                let mut covers = points.iter().fold(
                    Rect {
                        x0: f32::INFINITY,
                        y0: f32::INFINITY,
                        x1: f32::NEG_INFINITY,
                        y1: f32::NEG_INFINITY,
                    },
                    |r, p| Rect {
                        x0: r.x0.min(p.x),
                        y0: r.y0.min(p.y),
                        x1: r.x1.max(p.x),
                        y1: r.y1.max(p.y),
                    },
                );
                if matches!(path.paint, Paint::Stroke) {
                    let half = ev.state.line_width / 2.0;
                    covers.y0 -= half;
                    covers.y1 += half;
                    let one_level_segment = matches!(
                        path.segments,
                        [Seg::Move(a), Seg::Line(b)] if a.y == b.y
                    );
                    if !one_level_segment {
                        covers.x0 -= half;
                        covers.x1 += half;
                    }
                }
                out.push(Painted {
                    paint: path.paint,
                    state: ev.state.clone(),
                    points,
                    covers,
                });
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();
    assert!(faults.is_empty(), "no fault (an extra Q among them)");
    assert!(summary.deepest_q <= 1);
    out
}

#[test]
fn box_border_solid_and_dashed_stroke_the_inset_frame() {
    let rect = Rect::new(10.0, 20.0, 110.0, 70.0);
    let red = Color::Rgb([1.0, 0.0, 0.0]);

    // A solid border is `border_rect`'s drawing with its color set inside
    // the `q`.
    let mut plain = Canvas::new();
    plain.border_rect(rect, 2.0);
    let plain = String::from_utf8(plain.finish(&Reserved::default()).0).unwrap();
    assert_eq!(plain, "q\n2 w\n11 21 98 48 re\nS\nQ\n");
    let solid = box_border_text(rect, &BoxBorder::solid(2.0), red, None);
    assert_eq!(solid, "q\n1 0 0 RG\n2 w\n[] 0 d\n11 21 98 48 re\nS\nQ\n");
    assert_eq!(
        solid
            .replacen("1 0 0 RG\n", "", 1)
            .replacen("[] 0 d\n", "", 1),
        plain
    );
    // The fill hint and a dash array are a solid border's to ignore.
    assert_eq!(
        box_border_text(
            rect,
            &BoxBorder::solid(2.0).with_dash(&[1.0, 2.0]),
            red,
            Some(Color::Gray(0.3))
        ),
        solid
    );
    assert_eq!(
        BoxBorder::solid(2.0),
        BoxBorder::new(2.0, BoxBorderStyle::Solid)
    );
    assert_eq!(BoxBorderStyle::default(), BoxBorderStyle::Solid);
    // A box exactly as wide as the border still takes it; a narrower or
    // shorter one takes nothing.
    assert_eq!(
        box_border_text(
            Rect::new(0.0, 0.0, 2.0, 9.0),
            &BoxBorder::solid(2.0),
            red,
            None
        ),
        "q\n1 0 0 RG\n2 w\n[] 0 d\n1 1 0 7 re\nS\nQ\n"
    );
    assert_eq!(
        box_border_text(
            Rect::new(0.0, 0.0, 1.9, 9.0),
            &BoxBorder::solid(2.0),
            red,
            None
        ),
        ""
    );
    assert_eq!(
        box_border_text(
            Rect::new(0.0, 0.0, 9.0, 1.9),
            &BoxBorder::solid(2.0),
            red,
            None
        ),
        ""
    );

    // Dashed: the same frame under the array, phase 0.
    let dashed = BoxBorder::new(2.0, BoxBorderStyle::Dashed);
    assert_eq!(
        box_border_text(
            rect,
            &dashed.with_dash(&[4.0, 1.5]),
            Color::Gray(0.25),
            None
        ),
        "q\n0.25 G\n2 w\n[4 1.5] 0 d\n11 21 98 48 re\nS\nQ\n"
    );
    // A zero among positive numbers is a usable array.
    assert_eq!(
        box_border_text(
            rect,
            &dashed.with_dash(&[0.0, 2.0]),
            Color::Gray(0.25),
            None
        ),
        "q\n0.25 G\n2 w\n[0 2] 0 d\n11 21 98 48 re\nS\nQ\n"
    );
    // `[3]` where there is no array, or one a stroke can't take: a negative
    // or non-finite number, or all zeros.
    let fallback = "q\n0.25 G\n2 w\n[3] 0 d\n11 21 98 48 re\nS\nQ\n";
    for unusable in [
        &[][..],
        &[4.0, -1.0],
        &[f32::NAN, 2.0],
        &[2.0, f32::INFINITY],
        &[0.0],
        &[0.0, 0.0],
    ] {
        assert_eq!(
            box_border_text(rect, &dashed.with_dash(unusable), Color::Gray(0.25), None),
            fallback,
            "{unusable:?}"
        );
    }
    assert_eq!(
        box_border_text(Rect::new(0.0, 0.0, 1.9, 9.0), &dashed, red, None),
        ""
    );
}

/// Every style draws the same whatever dash the caller left set: only a
/// dashed border is dashed, and by its own array.
#[test]
fn box_border_sets_its_own_dash() {
    let rect = Rect::new(10.0, 20.0, 110.0, 70.0);
    let dash = [4.0, 1.0];
    for style in [
        BoxBorderStyle::Solid,
        BoxBorderStyle::Dashed,
        BoxBorderStyle::Beveled,
        BoxBorderStyle::Inset,
        BoxBorderStyle::Underline,
    ] {
        let border = BoxBorder::new(2.0, style).with_dash(&dash);
        let alone = box_border_text(rect, &border, Color::Gray(0.0), None);
        let mut after = Canvas::new();
        after.dash(&[2.0, 2.0], 1.0);
        after.box_border(rect, &border, Color::Gray(0.0), None);
        let after = String::from_utf8(after.finish(&Reserved::default()).0).unwrap();
        assert_eq!(after, format!("[2 2] 1 d\n{alone}"), "{style:?}");
        let expected = if style == BoxBorderStyle::Dashed {
            "[4 1] 0 d"
        } else {
            "[] 0 d"
        };
        assert_eq!(alone.matches(" d\n").count(), 1, "{style:?}");
        assert!(
            alone.contains(&format!("\n{expected}\n")),
            "{style:?}: {alone}"
        );
        // Under the caller's dash every stroke is still read with the border's own.
        for path in painted_paths(after.as_bytes()) {
            if matches!(path.paint, Paint::Stroke) {
                let own: &[f32] = if style == BoxBorderStyle::Dashed {
                    &dash
                } else {
                    &[]
                };
                assert_eq!(&path.state.dash.array[..], own, "{style:?}");
            }
        }
    }
}

#[test]
fn box_border_underline_is_one_segment_along_the_bottom() {
    let rect = Rect::new(10.0, 20.0, 110.0, 70.0);
    let underline = BoxBorder::new(3.0, BoxBorderStyle::Underline).with_dash(&[1.0]);
    assert_eq!(
        box_border_text(rect, &underline, Color::Cmyk([0.0, 1.0, 1.0, 0.0]), None),
        "q\n0 1 1 0 K\n3 w\n[] 0 d\n10 21.5 m\n110 21.5 l\nS\nQ\n"
    );
    // A box as tall as the line takes it; a shorter one, or one with no
    // width, takes nothing.
    assert_eq!(
        box_border_text(
            Rect::new(0.0, 0.0, 50.0, 3.0),
            &underline,
            Color::Gray(0.0),
            None
        ),
        "q\n0 G\n3 w\n[] 0 d\n0 1.5 m\n50 1.5 l\nS\nQ\n"
    );
    assert_eq!(
        box_border_text(
            Rect::new(0.0, 0.0, 50.0, 2.9),
            &underline,
            Color::Gray(0.0),
            None
        ),
        ""
    );
    assert_eq!(
        box_border_text(
            Rect::new(5.0, 0.0, 5.0, 30.0),
            &underline,
            Color::Gray(0.0),
            None
        ),
        ""
    );
}

#[test]
fn box_border_bevels_fill_two_mitered_bands_inside_the_frame() {
    let rect = Rect::new(10.0, 20.0, 110.0, 70.0);
    let blue = Color::Rgb([0.0, 0.0, 1.0]);
    // Width 2: the frame is stroked along 11..109 by 21..69; the bands lie
    // between the box inset by 2 (12..108 by 22..68) and by 4 (14..106 by
    // 24..66). The first runs up the left and along the top, the second along
    // the bottom and up the right; both start at the bottom left and miter at
    // the two shared corners.
    let frame = "q\n0 0 1 RG\n2 w\n[] 0 d\n11 21 98 48 re\nS\n";
    let top_left = "12 22 m\n12 68 l\n108 68 l\n106 66 l\n14 66 l\n14 24 l\nh\nf\n";
    let bottom_right = "12 22 m\n108 22 l\n108 68 l\n106 66 l\n106 24 l\n14 24 l\nh\nf\n";
    let bevel =
        |light: &str, dark: &str| format!("{frame}{light}\n{top_left}{dark}\n{bottom_right}Q\n");

    let beveled = BoxBorder::new(2.0, BoxBorderStyle::Beveled);
    // Beveled: white above and to the left; below and to the right the hint
    // at half its brightness, a middle gray without one.
    assert_eq!(
        box_border_text(rect, &beveled, blue, None),
        bevel("1 g", "0.5 g")
    );
    assert_eq!(
        box_border_text(rect, &beveled, blue, Some(Color::Gray(0.8))),
        bevel("1 g", "0.4 g")
    );
    assert_eq!(
        box_border_text(rect, &beveled, blue, Some(Color::Rgb([1.0, 0.5, 0.2]))),
        bevel("1 g", "0.5 0.25 0.1 rg")
    );
    // CMYK: the three inks halved and black moved halfway to full, which
    // halves the red, green and blue the device formula gives
    // (1 - 0.2 - 0.2 = 0.6 red becomes 1 - 0.1 - 0.6 = 0.3).
    assert_eq!(
        box_border_text(
            rect,
            &beveled,
            blue,
            Some(Color::Cmyk([0.2, 0.4, 0.0, 0.2]))
        ),
        bevel("1 g", "0.1 0.2 0 0.6 k")
    );
    // The dash array is a dashed border's alone.
    assert_eq!(
        box_border_text(rect, &beveled.with_dash(&[1.0]), blue, None),
        bevel("1 g", "0.5 g")
    );

    // Inset: a middle gray above and to the left, a light gray below and to
    // the right, whatever the hint.
    let inset = BoxBorder::new(2.0, BoxBorderStyle::Inset);
    assert_eq!(
        box_border_text(rect, &inset, blue, None),
        bevel("0.5 g", "0.75 g")
    );
    assert_eq!(
        box_border_text(rect, &inset, blue, Some(Color::Rgb([1.0, 0.5, 0.2]))),
        bevel("0.5 g", "0.75 g")
    );

    for style in [BoxBorderStyle::Beveled, BoxBorderStyle::Inset] {
        let border = BoxBorder::new(2.0, style);
        // A box of exactly four widths holds bands that meet in the middle.
        let text = box_border_text(Rect::new(0.0, 0.0, 8.0, 8.0), &border, blue, None);
        assert!(
            text.starts_with("q\n0 0 1 RG\n2 w\n[] 0 d\n1 1 6 6 re\nS\n"),
            "{text}"
        );
        assert!(
            text.contains("2 2 m\n2 6 l\n6 6 l\n4 4 l\n4 4 l\n4 4 l\nh\nf\n"),
            "{text}"
        );
        assert!(
            text.contains("2 2 m\n6 2 l\n6 6 l\n4 4 l\n4 4 l\n4 4 l\nh\nf\n"),
            "{text}"
        );
        // Between two and four widths, either way: the frame alone.
        for small in [
            Rect::new(0.0, 0.0, 7.9, 30.0),
            Rect::new(0.0, 0.0, 30.0, 7.9),
            Rect::new(0.0, 0.0, 4.0, 4.0),
        ] {
            let text = box_border_text(small, &border, blue, None);
            assert!(text.starts_with("q\n0 0 1 RG\n2 w\n[] 0 d\n1 1 "), "{text}");
            assert!(text.ends_with(" re\nS\nQ\n"), "{text}");
            assert!(!text.contains('f') && !text.contains('g'), "{text}");
        }
        // Under two widths, either way: nothing.
        for tiny in [
            Rect::new(0.0, 0.0, 3.9, 30.0),
            Rect::new(0.0, 0.0, 30.0, 3.9),
        ] {
            assert_eq!(box_border_text(tiny, &border, blue, None), "");
        }
    }
}

#[test]
fn box_border_draws_nothing_for_an_unusable_width_or_box() {
    let rect = Rect::new(10.0, 20.0, 110.0, 70.0);
    for style in BOX_BORDER_STYLES {
        for width in [0.0, -0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let border = BoxBorder::new(width, style).with_dash(&[2.0]);
            assert_eq!(
                box_border_text(rect, &border, Color::Gray(0.0), Some(Color::Gray(1.0))),
                "",
                "{style:?} at {width}"
            );
            // A border that draws nothing takes no room.
            assert_eq!(border.inset(), 0.0, "{style:?} at {width}");
        }
        let border = BoxBorder::new(1.0, style);
        for bad in [
            Rect {
                x0: f32::NAN,
                ..rect
            },
            Rect {
                y0: f32::NEG_INFINITY,
                ..rect
            },
            Rect {
                x1: f32::INFINITY,
                ..rect
            },
            Rect {
                y1: f32::NAN,
                ..rect
            },
            // An inverted box is smaller than any border.
            Rect {
                x0: 110.0,
                x1: 10.0,
                ..rect
            },
            Rect {
                y0: 70.0,
                y1: 20.0,
                ..rect
            },
        ] {
            assert_eq!(
                box_border_text(bad, &border, Color::Gray(0.0), None),
                "",
                "{style:?} in {bad:?}"
            );
        }
    }

    // What a border takes inside its box: its width, twice that with a bevel.
    assert_eq!(BoxBorder::solid(1.5).inset(), 1.5);
    assert_eq!(BoxBorder::new(1.5, BoxBorderStyle::Dashed).inset(), 1.5);
    assert_eq!(BoxBorder::new(1.5, BoxBorderStyle::Underline).inset(), 1.5);
    assert_eq!(BoxBorder::new(1.5, BoxBorderStyle::Beveled).inset(), 3.0);
    assert_eq!(BoxBorder::new(1.5, BoxBorderStyle::Inset).inset(), 3.0);

    // `Debug` is the numbers and the style.
    assert_eq!(
        format!(
            "{:?}",
            BoxBorder::new(2.0, BoxBorderStyle::Inset).with_dash(&[3.0, 1.0])
        ),
        "BoxBorder { width: 2.0, style: Inset, dash: [3.0, 1.0] }"
    );
}

#[test]
fn box_border_streams_read_back_balanced_and_inside_the_box() {
    let stroke = Color::Rgb([0.2, 0.4, 0.6]);
    let hint = Color::Cmyk([0.1, 0.2, 0.3, 0.4]);
    let boxes = [
        Rect::new(10.0, 20.0, 110.0, 70.0),
        Rect::new(-40.5, -30.25, -10.5, 12.75),
        Rect::new(0.0, 0.0, 12.0, 12.0),
        Rect::new(0.0, 0.0, 7.0, 300.0),
        Rect::new(100.0, 100.0, 103.0, 103.0),
    ];
    for style in BOX_BORDER_STYLES {
        for rect in boxes {
            for width in [0.5, 1.0, 3.0, 6.0] {
                let border = BoxBorder::new(width, style).with_dash(&[2.0, 1.0]);
                let mut canvas = Canvas::new();
                canvas.box_border(rect, &border, stroke, Some(hint));
                // A path of the caller's after the border, to see the state
                // the border left.
                canvas.stroke_rect(Rect::new(0.0, 0.0, 1.0, 1.0));
                let (bytes, _) = canvas.finish(&Reserved::default());
                let mut paths = painted_paths(&bytes);
                let what = format!("{style:?} of {width} in {rect:?}");

                let after = paths.pop().expect("the caller's own path");
                assert_eq!(after.state.line_width, 1.0, "{what}");
                assert!(after.state.dash.is_solid(), "{what}");
                assert_eq!(after.state.stroke_color.as_slice(), &[0.0], "{what}");
                assert_eq!(after.state.fill_color.as_slice(), &[0.0], "{what}");
                assert_eq!(after.state.stroke_color_space, ActiveColorSpace::DeviceGray);

                let (fits_frame, fits_bands) = match style {
                    BoxBorderStyle::Solid | BoxBorderStyle::Dashed => {
                        (rect.width() >= width && rect.height() >= width, false)
                    }
                    BoxBorderStyle::Underline => (rect.height() >= width, false),
                    _ => (
                        rect.width() >= 2.0 * width && rect.height() >= 2.0 * width,
                        rect.width() >= 4.0 * width && rect.height() >= 4.0 * width,
                    ),
                };
                let expected = match (fits_frame, fits_bands) {
                    (false, _) => 0,
                    (true, false) => 1,
                    (true, true) => 3,
                };
                assert_eq!(paths.len(), expected, "{what}");

                for (i, path) in paths.iter().enumerate() {
                    // Nothing is painted outside the box (a rounding's worth
                    // of slack on the written numbers).
                    assert!(
                        rect.expand(1e-3).contains_rect(path.covers),
                        "{what}: path {i} covers {:?}",
                        path.covers
                    );
                    if i == 0 {
                        assert_eq!(path.paint, Paint::Stroke, "{what}");
                        assert_eq!(path.state.line_width, width, "{what}");
                        assert_eq!(path.state.stroke_color_space, ActiveColorSpace::DeviceRgb);
                        assert_eq!(path.state.stroke_color.as_slice(), &[0.2, 0.4, 0.6]);
                        assert_eq!(
                            &path.state.dash.array[..],
                            if style == BoxBorderStyle::Dashed {
                                &[2.0, 1.0][..]
                            } else {
                                &[][..]
                            },
                            "{what}"
                        );
                    } else {
                        // A band: filled, six corners, a border's width inside
                        // the frame.
                        assert_eq!(path.paint, Paint::Fill, "{what}");
                        let band = Rect::new(
                            rect.x0 + width,
                            rect.y0 + width,
                            rect.x1 - width,
                            rect.y1 - width,
                        );
                        assert!(band.expand(1e-3).contains_rect(path.covers), "{what}");
                        let corners: Vec<Point> = path.points[..6].to_vec();
                        assert_eq!(corners[0], Point::new(band.x0, band.y0), "{what}");
                        assert_eq!(corners[2], Point::new(band.x1, band.y1), "{what}");
                    }
                }
            }
        }
    }
}

#[test]
fn box_border_fuzz_body_holds_over_extreme_numbers() {
    // The fuzz body's border check over numbers of every kind (a small
    // xorshift's raw bit patterns, and each special value in each place).
    let specials = [
        0.0_f32,
        -0.0,
        1.0,
        -1.0,
        0.5,
        3.0,
        1e-30,
        f32::MIN_POSITIVE / 2.0,
        1e30,
        f32::MAX,
        f32::MIN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
    ];
    let base = [0.0_f32, 0.0, 100.0, 50.0, 2.0, 3.0, 1.0, 0.5, 0.25, 1.0];
    let bytes_of =
        |numbers: &[f32]| -> Vec<u8> { numbers.iter().flat_map(|n| n.to_le_bytes()).collect() };
    fuzz::check(&bytes_of(&base));
    for place in 0..base.len() {
        for special in specials {
            let mut numbers = base;
            numbers[place] = special;
            fuzz::check(&bytes_of(&numbers));
        }
    }
    // A fuzzer's find: a box as tall as `f32` goes and a width equal to its
    // height, whose underline's stroke a reader can't bound.
    let found = [
        0x3530_2030_u32,
        0x2030_3520,
        0x7f72_2030,
        0x7f7f_7f7f,
        0x7f7f_7f7f,
    ];
    let mut numbers = base;
    for (number, bits) in numbers.iter_mut().zip(found) {
        *number = f32::from_bits(bits);
    }
    fuzz::check(&bytes_of(&numbers));
    // Boxes and widths near `f32`'s largest together, where a stroke's
    // extent overflows though every number written is finite.
    let large = [1e37_f32, 3e37, 1e38, 3e38, f32::MAX];
    for hi in large {
        for lo in [-hi, -hi / 2.0, 0.0, hi / 2.0] {
            for width in large.iter().flat_map(|w| [*w, w / 16.0]) {
                for (x0, y0, x1, y1) in
                    [(lo, lo, hi, hi), (lo, 0.0, hi, 50.0), (0.0, lo, 100.0, hi)]
                {
                    let mut numbers = base;
                    numbers[..5].copy_from_slice(&[x0, y0, x1, y1, width]);
                    fuzz::check(&bytes_of(&numbers));
                }
            }
        }
    }
    let mut x = 0x2545_f491_u32;
    for _ in 0..4096 {
        let mut data = Vec::with_capacity(40);
        for i in 0..10 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            // Half the numbers small and plain, so that borders are drawn.
            if (x >> 7) & 1 == 0 || i >= 7 {
                data.extend_from_slice(&x.to_le_bytes());
            } else {
                data.extend_from_slice(&((x % 4000) as f32 / 16.0).to_le_bytes());
            }
        }
        fuzz::check(&data);
    }
}

#[test]
fn user_segments_transparency_groups_and_soft_mask_params_track_state() {
    let mut store = Store::default();
    let tr_fn_id = store.put_stream(
        dict! {
            "FunctionType" => 2,
            "Domain" => vec![Object::from(0.0), Object::from(1.0)],
            "C0" => vec![Object::from(1.0)],
            "C1" => vec![Object::from(0.0)],
            "N" => 1.0,
        },
        Vec::new(),
    );
    let smask_group = store.put_stream(
        dict! {
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(50), Object::from(50)],
            "Group" => dict! {
                "S" => Object::name("Transparency"),
                "I" => true,
                "CS" => Object::name("DeviceGray"),
            },
        },
        b"0.5 g 0 0 50 50 re f".to_vec(),
    );
    let form_group_dict = store.put(Object::from(dict! {
        "S" => Object::name("Transparency"),
        "I" => true,
        "K" => true,
        "CS" => Object::name("SecretGroupCS"),
    }));
    let form_id = store.put_stream(
        dict! {
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(40), Object::from(40)],
            "Matrix" => vec![
                Object::from(1), Object::from(0), Object::from(0),
                Object::from(1), Object::from(5), Object::from(7),
            ],
            "Group" => Object::from(form_group_dict),
        },
        b"0 0 m 10 20 l 30 40 50 60 70 80 c h B".to_vec(),
    );

    // Direct `TransparencyGroup::from_dict` parsing (indirect `/Group`, non-transparency `/S`, Debug hygiene)
    let form_obj = store.objects.get(&form_id).unwrap().as_stream().unwrap();
    let parsed_group = TransparencyGroup::from_dict(&store.objects, &form_obj.dict).unwrap();
    assert!(parsed_group.isolated);
    assert!(parsed_group.knockout);
    assert_eq!(
        parsed_group.color_space.and_then(Object::as_name),
        Some(&b"SecretGroupCS"[..])
    );
    let dbg_group = format!("{parsed_group:?}");
    assert!(
        !dbg_group.contains("SecretGroupCS"),
        "Debug leaked /CS name: {dbg_group}"
    );
    let non_trans = dict! { "Group" => dict! { "S" => Object::name("Luminosity") } };
    assert!(TransparencyGroup::from_dict(&store.objects, &non_trans).is_none());

    let resources = Object::from(dict! {
        "ExtGState" => dict! {
            "GS1" => dict! {
                "SMask" => dict! {
                    "S" => Object::name("Luminosity"),
                    "G" => smask_group,
                    "BC" => vec![Object::from(0.25), Object::from(0.5), Object::from(0.75)],
                    "TR" => dict! { "SecretTrKey" => Object::text("SecretTransferPayload"), "Ref" => tr_fn_id },
                },
            },
            "GSId" => dict! {
                "SMask" => dict! {
                    "S" => Object::name("Alpha"),
                    "G" => smask_group,
                    "TR" => Object::name("Identity"),
                },
            },
            "GSOff" => dict! {
                "SMask" => Object::name("None"),
            },
        },
        "XObject" => dict! { "FmG" => form_id },
    });

    // Set CTM to `[2 0 0 3 10 20]`, apply `/GS1 gs` (must snapshot that CTM!),
    // then apply another `cm` (`[1 0 0 1 100 200]`): `soft_mask_params.ctm` MUST remain
    // `[2 0 0 3 10 20]` while `state.ctm` advances, and `user_segments` transformed by
    // `state.ctm` must equal `segments`!
    let content = Concat::new(&[br#"
        q
        2 0 0 3 10 20 cm
        /GS1 gs
        1 0 0 1 100 200 cm
        3 4 m 12 15 l 20 25 30 35 40 45 c h S
        /FmG Do
        /GSId gs
        0 0 5 5 re f
        /GSOff gs
        0 0 5 5 re f
        Q
    "#]);

    let fonts = Fonts::default();
    let mut path_states = Vec::new();
    let mut seen_form_group = None;
    interpret(
        store.as_streams(),
        &content,
        Some(&resources),
        &Limits::DEFAULT,
        &fonts,
        Opts::default(),
        |ev| {
            match ev.kind {
                Kind::Path(drawn) => {
                    assert_eq!(drawn.user_segments.len(), drawn.segments.len());
                    for (&u, &p) in drawn.user_segments.iter().zip(drawn.segments.iter()) {
                        let mapped = match u {
                            Seg::Move(pt) => Seg::Move(ev.state.ctm.apply(pt)),
                            Seg::Line(pt) => Seg::Line(ev.state.ctm.apply(pt)),
                            Seg::Curve(c1, c2, pt) => Seg::Curve(
                                ev.state.ctm.apply(c1),
                                ev.state.ctm.apply(c2),
                                ev.state.ctm.apply(pt),
                            ),
                            Seg::Close => Seg::Close,
                        };
                        assert_eq!(mapped, p);
                    }
                    path_states.push(ev.state.clone());
                }
                Kind::FormBegin(f) => {
                    seen_form_group = f
                        .group
                        .map(|g| (g.isolated, g.knockout, g.color_space.is_some()));
                }
                _ => {}
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();

    assert_eq!(seen_form_group, Some((true, true, true)));
    assert_eq!(path_states.len(), 4);

    // Path 0 (after `/GS1 gs` and subsequent `cm`):
    let p0 = path_states[0]
        .soft_mask_params
        .as_ref()
        .expect("GS1 params");
    assert_eq!(p0.backdrop.as_ref().unwrap().as_slice(), &[0.25, 0.5, 0.75]);
    assert!(p0.transfer.is_some());
    assert_eq!(p0.ctm, Matrix::new(2.0, 0.0, 0.0, 3.0, 10.0, 20.0));
    assert_ne!(path_states[0].ctm, p0.ctm);
    let dbg_state = format!("{:?}", path_states[0]);
    assert!(
        !dbg_state.contains("SecretTransferPayload") && !dbg_state.contains("SecretTrKey"),
        "{dbg_state}"
    );

    // Path 2 (after `/GSId gs`): `/TR /Identity` normalized to `None`, `backdrop` cleared to `None`,
    // and `ctm` updated to the CTM active when `/GSId gs` executed!
    let p2 = path_states[2]
        .soft_mask_params
        .as_ref()
        .expect("GSId params");
    assert!(p2.backdrop.is_none());
    assert!(p2.transfer.is_none());
    assert_eq!(p2.ctm, path_states[2].ctm);

    // Path 3 (after `/GSOff gs`): `/SMask /None` clears both `soft_mask` and `soft_mask_params`!
    assert_eq!(path_states[3].soft_mask, SoftMask::None);
    assert!(path_states[3].soft_mask_params.is_none());
}

/// A font for layout's numbers: every ASCII character 500 units wide but the
/// space, 250, and the heights it's given.
#[cfg(feature = "text-layout")]
struct Even {
    ascent: f32,
    descent: f32,
}

#[cfg(feature = "text-layout")]
impl crate::LayoutFont for Even {
    fn font_metrics(&self) -> pdf_fonts::FontMetrics {
        pdf_fonts::FontMetrics::new(self.ascent, self.descent, None)
    }

    fn char_advance(&self, ch: char) -> Option<f32> {
        ch.is_ascii()
            .then_some(if ch == ' ' { 250.0 } else { 500.0 })
    }

    fn encode_char(&self, ch: char, out: &mut Vec<u8>) -> bool {
        if !ch.is_ascii() {
            return false;
        }
        out.push(ch as u8);
        true
    }
}

/// `text` laid out at size 10 in [`Even`] (a character 5 wide, a space 2.5)
/// in a box `width` wide, `align`ed, keeping its spaces or not: each line's
/// text, `x` and width, its codes held equal to its text.
#[cfg(feature = "text-layout")]
fn even_lines(
    text: &str,
    width: f32,
    align: crate::TextAlign,
    keep: bool,
) -> Vec<(String, f32, f32)> {
    let style = crate::TextBoxStyle {
        size: Some(10.0),
        align,
        line_end_spaces: keep,
        ..crate::TextBoxStyle::default()
    };
    let font = Even {
        ascent: 800.0,
        descent: -200.0,
    };
    let laid = crate::layout_text(&font, text, Rect::new(0.0, 0.0, width, 100.0), &style).unwrap();
    assert!(!laid.overflowed, "{text:?} in {width}");
    laid.lines
        .iter()
        .map(|line| {
            let shown = &text[line.text_range.clone()];
            assert_eq!(line.codes, shown.as_bytes(), "{text:?}");
            (shown.to_string(), line.origin.x, line.width)
        })
        .collect()
}

#[cfg(feature = "text-layout")]
#[test]
fn line_end_spaces_keeps_the_spaces_a_line_breaks_at() {
    use crate::TextAlign::{Center, Justify, Left, Right};

    let lines = |text, width, keep| even_lines(text, width, Left, keep);
    let want = |of: &[(&str, f32)]| -> Vec<(String, f32, f32)> {
        of.iter().map(|&(s, w)| (s.to_string(), 0.0, w)).collect()
    };

    // Broken before a word: off, the spaces are trimmed from the line and
    // passed over at the next; on, they end the line, in its codes, its range
    // and its width.
    let text = "ab cd  ef";
    assert_eq!(
        lines(text, 30.0, false),
        want(&[("ab cd", 22.5), ("ef", 10.0)])
    );
    assert_eq!(
        lines(text, 30.0, true),
        want(&[("ab cd  ", 27.5), ("ef", 10.0)])
    );
    // The width the line is aligned by is the one with its spaces.
    for (align, off, on) in [(Right, 7.5, 2.5), (Center, 3.75, 1.25)] {
        assert_eq!(even_lines(text, 30.0, align, false)[0].1, off, "{align:?}");
        assert_eq!(even_lines(text, 30.0, align, true)[0].1, on, "{align:?}");
    }

    // A space that would pass the width: off, the line breaks at it and keeps
    // its last word; on, before that word, which begins the next line.
    let text = "ab cd ef";
    assert_eq!(
        lines(text, 22.5, false),
        want(&[("ab cd", 22.5), ("ef", 10.0)])
    );
    assert_eq!(
        lines(text, 22.5, true),
        want(&[("ab ", 12.5), ("cd ef", 22.5)])
    );
    // With no word begun on the line after its first character, before the
    // space, which begins the next line.
    let text = "abcd ef";
    assert_eq!(
        lines(text, 20.0, false),
        want(&[("abcd", 20.0), ("ef", 10.0)])
    );
    assert_eq!(
        lines(text, 20.0, true),
        want(&[("abcd", 20.0), (" ef", 12.5)])
    );

    // A hyphen joins the characters about it: on, no word begins after one,
    // so a line with no other word breaks before its next character.
    let text = "ab-cd ef";
    assert_eq!(
        lines(text, 25.0, false),
        want(&[("ab-cd", 25.0), ("ef", 10.0)])
    );
    assert_eq!(
        lines(text, 25.0, true),
        want(&[("ab-cd", 25.0), (" ef", 12.5)])
    );
    // Nor at a space or a hyphen after one.
    let text = "a--b c";
    assert_eq!(
        lines(text, 22.5, true),
        want(&[("a--b ", 22.5), ("c", 5.0)])
    );
    assert_eq!(
        lines("a- b c", 15.0, true),
        want(&[("a- ", 12.5), ("b c", 12.5)])
    );
    // A hyphenated word that fits a line moves to the next whole; one longer
    // than a line breaks before the character that passes it, hyphen or not.
    assert_eq!(
        lines("ab cd-ef", 30.0, true),
        want(&[("ab ", 12.5), ("cd-ef", 25.0)])
    );
    assert_eq!(
        lines("abc-def", 17.5, true),
        want(&[("abc", 15.0), ("-de", 15.0), ("f", 5.0)])
    );
    // Punctuation that doesn't join ends a word; an opening bracket and a
    // currency sign begin one, and inside a bracket just opened nothing
    // does.
    assert_eq!(
        lines("ab!cd", 20.0, true),
        want(&[("ab!", 15.0), ("cd", 10.0)])
    );
    assert_eq!(
        lines("ab(cd", 20.0, true),
        want(&[("ab", 10.0), ("(cd", 15.0)])
    );
    assert_eq!(
        lines("a( bc", 17.5, true),
        want(&[("a", 5.0), ("( bc", 17.5)])
    );
    assert_eq!(
        lines("ab$12", 20.0, true),
        want(&[("ab", 10.0), ("$12", 15.0)])
    );

    // A paragraph's last line keeps its spaces too; a paragraph's first ones
    // are kept either way.
    assert_eq!(lines("ab  ", 100.0, false), want(&[("ab", 10.0)]));
    assert_eq!(lines("ab  ", 100.0, true), want(&[("ab  ", 15.0)]));
    assert_eq!(
        lines("ab  \ncd", 100.0, false),
        want(&[("ab", 10.0), ("cd", 10.0)])
    );
    assert_eq!(
        lines("ab  \ncd", 100.0, true),
        want(&[("ab  ", 15.0), ("cd", 10.0)])
    );
    for keep in [false, true] {
        assert_eq!(lines("  ab", 100.0, keep), want(&[("  ab", 15.0)]));
    }

    // Justified, the words are spread to the edge by the spaces between them:
    // the `Tw` is the same, and the width, the spaces at the end widened too,
    // is the whole line's.
    let style = |keep| crate::TextBoxStyle {
        size: Some(10.0),
        align: Justify,
        line_end_spaces: keep,
        ..crate::TextBoxStyle::default()
    };
    let font = Even {
        ascent: 800.0,
        descent: -200.0,
    };
    let rect = Rect::new(0.0, 0.0, 30.0, 100.0);
    let off = crate::layout_text(&font, "ab cd  ef", rect, &style(false)).unwrap();
    let on = crate::layout_text(&font, "ab cd  ef", rect, &style(true)).unwrap();
    assert_eq!((off.lines[0].word_space, off.lines[0].width), (7.5, 30.0));
    assert_eq!((on.lines[0].word_space, on.lines[0].width), (7.5, 50.0));
    assert_eq!((on.lines[1].word_space, on.lines[1].width), (0.0, 10.0));
    // Drawn, the kept spaces are in the `Tj`.
    let mut canvas = Canvas::new();
    canvas.draw_layout(Res::dict(Std14::Helvetica.dict()).unwrap(), &on);
    let (bytes, _) = canvas.finish(&Reserved::default());
    let drawn = String::from_utf8(bytes).unwrap();
    assert!(
        drawn.contains("7.5 Tw\n1 0 0 1 0 92 Tm\n(ab cd  ) Tj\n"),
        "{drawn}"
    );
}

/// A line its glyphs fill exactly: keeping its spaces, it breaks where
/// PDFium's sum of the advances in `f32` passes the box's width; without
/// them, it fits by the allowance, as it always has.
#[cfg(feature = "text-layout")]
#[test]
fn line_end_spaces_breaks_a_line_its_glyphs_fill_where_pdfium_does() {
    let lines = |text: &str, size: f32, keep: bool| -> Vec<String> {
        let style = crate::TextBoxStyle {
            size: Some(size),
            line_end_spaces: keep,
            ..crate::TextBoxStyle::default()
        };
        // Ten of Courier's advances of 600 across.
        let rect = Rect::new(0.0, 0.0, 6.0 * size, 200.0);
        let laid = crate::layout_text(&Std14::Courier, text, rect, &style).unwrap();
        assert!(!laid.overflowed, "{text:?} at {size}");
        (laid.lines.iter())
            .map(|line| text[line.text_range.clone()].to_string())
            .collect()
    };
    let a = |n: usize| "A".repeat(n);
    // How many `A`s PDFium as of chromium/8152 sets on the first line at each
    // size (pdf-forms' `forms-ap` case of them): where ten advances sum past
    // the width in `f32` (`6.0000005` each at 10 points), nine.
    for (size, first) in [
        (4.0, 10),
        (5.0, 9),
        (6.0, 10),
        (7.0, 9),
        (8.0, 10),
        (9.0, 9),
        (10.0, 9),
        (11.0, 10),
        (12.0, 10),
        (14.0, 9),
    ] {
        assert_eq!(
            lines(&a(12), size, true),
            [a(first), a(12 - first)],
            "{size}"
        );
        assert_eq!(lines(&a(12), size, false), [a(10), a(2)], "{size}");
        // Broken before a word, the line's words are where they were.
        assert_eq!(
            lines("AAAAA AAAA AAAA", size, true),
            ["AAAAA ", "AAAA AAAA"],
            "{size}"
        );
    }
}

/// Where a line that keeps its spaces may break between two characters, by
/// the classes PDFium's line breaking reads them in.
#[cfg(feature = "text-layout")]
#[test]
fn line_end_spaces_divides_words_by_their_characters_classes() {
    use crate::wrap::divides;

    // Two letters or digits, Latin's past ASCII among them, never.
    for (a, b) in [
        ('a', 'b'),
        ('1', 'x'),
        ('x', '2'),
        ('é', 'ß'),
        ('Ａ', 'ｚ'),
        ('ẞ', 'a'),
    ] {
        assert!(!divides(a, b), "{a:?} {b:?}");
    }
    // Before a space or punctuation never; after one, at a character that
    // isn't, always.
    for b in [
        ' ', '\u{3000}', '!', ')', ',', '-', '.', ':', '?', ']', '}', '\u{2019}', '\u{3002}',
    ] {
        assert!(!divides('a', b), "{b:?}");
    }
    for a in [
        ' ', '\u{3000}', '!', '"', ')', ':', ';', '?', ']', '}', '\u{201D}', '\u{FF01}',
    ] {
        assert!(divides(a, 'b'), "{a:?}");
    }
    // Punctuation that joins: neither after it nor before it.
    for a in ['\'', ',', '-', '.', '/', '\u{6}'] {
        assert!(!divides(a, 'b'), "{a:?}");
        assert!(!divides(a, '中'), "{a:?}");
    }
    assert!(!divides('a', '\u{6}'));
    // A currency sign or the numero sign: before one, not after it.
    for sign in ['$', '€', '£', '¥', '\u{20BF}', '№', '\u{FF04}'] {
        assert!(divides('a', sign), "{sign:?}");
        assert!(!divides(sign, '1'), "{sign:?}");
        assert!(!divides(sign, '中'), "{sign:?}");
    }
    // Ideographs and syllables each a word, before and after.
    for cjk in ['中', 'あ', 'カ', '한', '\u{20000}', '\u{FF76}', '\u{3021}'] {
        assert!(divides('a', cjk), "{cjk:?}");
        assert!(divides(cjk, 'a'), "{cjk:?}");
        assert!(divides(cjk, '中'), "{cjk:?}");
    }
    // Other characters stay with what's beside them.
    for (a, b) in [
        ('a', '#'),
        ('#', 'a'),
        ('a', '+'),
        ('*', '1'),
        ('a', 'α'),
        ('α', 'β'),
    ] {
        assert!(!divides(a, b), "{a:?} {b:?}");
    }
}

#[cfg(feature = "text-layout")]
#[test]
fn font_heights_as_given_drops_the_floors() {
    use crate::{TextBoxStyle, VerticalAlign, layout_text};

    let rect = Rect::new(0.0, 0.0, 100.0, 50.0);
    let style = |given, vertical_align| TextBoxStyle {
        size: Some(10.0),
        vertical_align,
        font_heights_as_given: given,
        ..TextBoxStyle::default()
    };
    // Heights under the floors: 0.5 and -0.1 em, 0.6 between them.
    let short = Even {
        ascent: 500.0,
        descent: -100.0,
    };
    let baselines = |font: &Even, text, given, valign| {
        let laid = layout_text(font, text, rect, &style(given, valign)).unwrap();
        let ys: Vec<f32> = laid.lines.iter().map(|l| l.origin.y).collect();
        (ys, laid.line_height)
    };
    // Off: an ascent of 0.70, a descent of -0.15, a step of 0.85 times 1.2.
    let (ys, step) = baselines(&short, "ab\ncd", false, VerticalAlign::Top);
    assert_approx(ys[0], 43.0);
    assert_approx(ys[1], 32.8);
    assert_approx(step, 10.2);
    // On: the font's own, 0.5 and -0.1, a step of 0.6 times 1.2.
    let (ys, step) = baselines(&short, "ab\ncd", true, VerticalAlign::Top);
    assert_approx(ys[0], 45.0);
    assert_approx(ys[1], 37.8);
    assert_approx(step, 7.2);
    // A single line about the middle: `25 - (ascent + descent) / 2`.
    assert_approx(
        baselines(&short, "ab", false, VerticalAlign::Middle).0[0],
        22.25,
    );
    assert_approx(
        baselines(&short, "ab", true, VerticalAlign::Middle).0[0],
        23.0,
    );
    // With no leading the step is the height itself.
    let unled = TextBoxStyle {
        leading_factor: 1.0,
        ..style(true, VerticalAlign::Top)
    };
    assert_approx(
        layout_text(&short, "ab", rect, &unled).unwrap().line_height,
        6.0,
    );
    // Comb cells stand on the same baseline.
    let comb = TextBoxStyle {
        comb: Some(5),
        ..style(true, VerticalAlign::Middle)
    };
    let laid = layout_text(&short, "ab", rect, &comb).unwrap();
    assert_approx(laid.lines[0].cells[0].origin.y, 23.0);

    // Heights that give none: the floors all the same, so no step is 0. A
    // descent above the ascent (a flipped Type 3 font), heights not finite,
    // or an ascent above the descent by less than `f32` holds once divided
    // by 1000.
    for (ascent, descent) in [
        (0.0, 0.0),
        (-100.0, 500.0),
        (f32::NAN, -200.0),
        (800.0, f32::NEG_INFINITY),
        (f32::INFINITY, -200.0),
        (f32::from_bits(1), 0.0),
    ] {
        let font = Even { ascent, descent };
        for valign in [VerticalAlign::Top, VerticalAlign::Middle] {
            let on = layout_text(&font, "ab\ncd", rect, &style(true, valign)).unwrap();
            let off = layout_text(&font, "ab\ncd", rect, &style(false, valign)).unwrap();
            // As text, since an infinite height lays lines out where the
            // floors leave it, at infinite or NaN places, either way.
            assert_eq!(format!("{on:?}"), format!("{off:?}"), "{ascent} {descent}");
            assert!(on.line_height > 0.0);
        }
    }

    // Helvetica's heights are within the floors: the same either way.
    // Courier's ascent (629) isn't: on, it stands higher by 0.071 em.
    for valign in [VerticalAlign::Top, VerticalAlign::Middle] {
        let text = "Hello\nworld";
        let on = layout_text(&Std14::Helvetica, text, rect, &style(true, valign)).unwrap();
        let off = layout_text(&Std14::Helvetica, text, rect, &style(false, valign)).unwrap();
        assert_eq!(on, off);
    }
    let on = layout_text(
        &Std14::Courier,
        "Hi",
        rect,
        &style(true, VerticalAlign::Top),
    )
    .unwrap();
    let off = layout_text(
        &Std14::Courier,
        "Hi",
        rect,
        &style(false, VerticalAlign::Top),
    )
    .unwrap();
    assert_approx(on.lines[0].origin.y, 50.0 - 6.29);
    assert_approx(off.lines[0].origin.y, 50.0 - 7.0);
}

/// What `Canvas::box_bands` writes for one border, as text.
fn box_bands_text(rect: Rect, border: &BoxBorder<'_>, fill_hint: Option<Color>) -> String {
    let mut canvas = Canvas::new();
    canvas.box_bands(rect, border, fill_hint);
    let (bytes, needs) = canvas.finish(&Reserved::default());
    assert!(needs.0.is_empty(), "bands need no resource");
    String::from_utf8(bytes).unwrap()
}

#[test]
fn box_bands_draw_box_borders_bands_alone() {
    let rect = Rect::new(10.0, 20.0, 110.0, 70.0);
    let blue = Color::Rgb([0.0, 0.0, 1.0]);
    // What `box_border` writes in the bands' place, in a scope of their own.
    assert_eq!(
        box_bands_text(rect, &BoxBorder::new(2.0, BoxBorderStyle::Beveled), None),
        "q\n1 g\n12 22 m\n12 68 l\n108 68 l\n106 66 l\n14 66 l\n14 24 l\nh\nf\n\
         0.5 g\n12 22 m\n108 22 l\n108 68 l\n106 66 l\n106 24 l\n14 24 l\nh\nf\nQ\n"
    );
    // For every shade, `box_border` is its frame's scope and stroke, then
    // the same bands.
    let frame = "q\n0 0 1 RG\n2 w\n[] 0 d\n11 21 98 48 re\nS\n";
    for style in [BoxBorderStyle::Beveled, BoxBorderStyle::Inset] {
        for hint in [
            None,
            Some(Color::Gray(0.8)),
            Some(Color::Rgb([1.0, 0.5, 0.2])),
            Some(Color::Cmyk([0.2, 0.4, 0.0, 0.2])),
        ] {
            // The dash array is a dashed border's alone, here as there.
            let border = BoxBorder::new(2.0, style).with_dash(&[1.0]);
            let bands = box_bands_text(rect, &border, hint);
            let whole = box_border_text(rect, &border, blue, hint);
            assert_eq!(
                whole,
                format!("{frame}{}", bands.strip_prefix("q\n").unwrap()),
                "{style:?} {hint:?}"
            );
        }
    }

    // Nothing for the styles without bands, at any width.
    for style in [
        BoxBorderStyle::Solid,
        BoxBorderStyle::Dashed,
        BoxBorderStyle::Underline,
    ] {
        for width in [0.5, 2.0, 6.0] {
            let border = BoxBorder::new(width, style).with_dash(&[2.0]);
            assert_eq!(box_bands_text(rect, &border, Some(Color::Gray(0.5))), "");
        }
    }
    for style in [BoxBorderStyle::Beveled, BoxBorderStyle::Inset] {
        // Nothing where `box_border` would draw no bands: a width or box it
        // can't use, or a box under four widths (no empty scope either).
        for width in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                box_bands_text(rect, &BoxBorder::new(width, style), None),
                ""
            );
        }
        let border = BoxBorder::new(2.0, style);
        for bad in [
            Rect {
                x0: f32::NAN,
                ..rect
            },
            Rect {
                y1: f32::INFINITY,
                ..rect
            },
            Rect::new(0.0, 0.0, 7.9, 30.0),
            Rect::new(0.0, 0.0, 30.0, 7.9),
            Rect::new(0.0, 0.0, 3.0, 3.0),
        ] {
            assert_eq!(
                box_bands_text(bad, &border, None),
                "",
                "{style:?} in {bad:?}"
            );
        }
        let huge = BoxBorder::new(f32::MAX / 4.0, style);
        assert_eq!(
            box_bands_text(Rect::new(0.0, 0.0, f32::MAX, f32::MAX), &huge, None),
            ""
        );
        // A box of exactly four widths holds bands that meet in the middle.
        let met = box_bands_text(Rect::new(0.0, 0.0, 8.0, 8.0), &border, None);
        assert!(
            met.contains("2 2 m\n2 6 l\n6 6 l\n4 4 l\n4 4 l\n4 4 l\nh\nf\n"),
            "{met}"
        );

        // Read back: two fills a width inside the box, and the caller's
        // fill color as it was after them.
        let mut canvas = Canvas::new();
        canvas.box_bands(rect, &border, Some(Color::Gray(0.8)));
        canvas.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0));
        let (bytes, _) = canvas.finish(&Reserved::default());
        let mut paths = painted_paths(&bytes);
        let after = paths.pop().expect("the caller's own path");
        assert_eq!(after.state.fill_color.as_slice(), &[0.0]);
        assert_eq!(paths.len(), 2);
        let band = Rect::new(12.0, 22.0, 108.0, 68.0);
        for path in &paths {
            assert_eq!(path.paint, Paint::Fill);
            assert!(band.expand(1e-3).contains_rect(path.covers), "{style:?}");
        }
    }
}

/// `da` parsed with a sink, held to what `parse` gives: the value, and the
/// faults told, each at a place with no object and no span.
fn da_faults_under(
    da: &[u8],
    limits: &pdf_content_streams::Limits,
) -> (Option<DefaultAppearance>, Vec<Fault>) {
    let mut faults = Vec::new();
    let parsed = DefaultAppearance::parse_with(da, limits, &mut faults);
    assert_eq!(parsed, DefaultAppearance::parse(da, limits));
    assert_eq!(parsed, DefaultAppearance::parse_with(da, limits, &mut ()));
    let faults = faults
        .into_iter()
        .map(|(place, fault)| {
            assert_eq!(place, pdf_objects::fault::Place::new(None, None));
            fault
        })
        .collect();
    (parsed, faults)
}

fn da_faults(da: &[u8]) -> (Option<DefaultAppearance>, Vec<Fault>) {
    da_faults_under(da, &pdf_content_streams::Limits::DEFAULT)
}

#[test]
fn default_appearance_parse_with_tells_each_fault_at_its_offset() {
    use pdf_content_streams::{ErrorKind, Operator};

    let operand = |at, operator| Fault::DefaultAppearanceOperand { at, operator };
    let faults = |da: &[u8]| da_faults(da).1;

    // Nothing to tell.
    assert_eq!(faults(b"/Helv 12 Tf 0 g"), []);
    assert_eq!(
        faults(b"/Helv 0 Tf 0 0 1 rg 2 Tc 1 Tw 90 Tz 14 TL 1 Tr 3 Ts 0 0 0 1 K 0.5 G"),
        []
    );
    // Operands before those an operator takes are passed over, as PDFium
    // passes them over, and other operators too.
    assert_eq!(faults(b"/A 1 /Helv 12 Tf 1 2 0.5 g BT (x) Tj ET 5 w"), []);

    // `Tf` read in part: its name kept where its size isn't finite, its size
    // kept where its name is missing or not a name.
    let huge = format!("1{}.", "0".repeat(45));
    let src = format!("/Helv {huge} Tf");
    let (da, told) = da_faults(src.as_bytes());
    assert_eq!(told, [operand(src.len() - 2, Operator::Font)]);
    let da = da.unwrap();
    assert_eq!(
        (da.font_name.as_deref(), da.font_size),
        (Some(&b"Helv"[..]), None)
    );
    // Taken from the last, as PDFium takes them, a lone name is the size,
    // and reads as 0: no font is named.
    let (da, told) = da_faults(b"/Helv Tf");
    assert_eq!(
        told,
        [operand(6, Operator::Font), Fault::DefaultAppearanceNoFont]
    );
    let da = da.unwrap();
    assert_eq!((da.font_name, da.font_size), (None, Some(0.0)));
    let (da, told) = da_faults(b"12 12 Tf");
    assert_eq!(
        told,
        [operand(6, Operator::Font), Fault::DefaultAppearanceNoFont]
    );
    assert_eq!(da.unwrap().font_size, Some(12.0));
    assert_eq!(
        faults(b"12 Tf"),
        [operand(3, Operator::Font), Fault::DefaultAppearanceNoFont]
    );
    // A later `Tf` without a name keeps the earlier one's.
    assert_eq!(faults(b"/A 8 Tf 9 Tf"), [operand(10, Operator::Font)]);

    // A color short of components, or with one not finite, is ignored.
    assert_eq!(
        faults(b"/Helv 9 Tf 0 0 rg"),
        [operand(15, Operator::FillRgb)]
    );
    assert_eq!(
        faults(b"/F 1 Tf 1 0 K"),
        [operand(12, Operator::StrokeCmyk)]
    );
    assert_eq!(faults(b"/F 1 Tf G"), [operand(8, Operator::StrokeGray)]);
    let src = format!("/F 1 Tf 0 {huge} 0 RG");
    assert_eq!(
        faults(src.as_bytes()),
        [operand(src.len() - 2, Operator::StrokeRgb)]
    );
    // So is each text-state operator with no operand.
    assert_eq!(
        faults(b"/F 1 Tf Tc Tw Tz TL Ts Tr"),
        [
            operand(8, Operator::CharSpace),
            operand(11, Operator::WordSpace),
            operand(14, Operator::Scale),
            operand(17, Operator::Leading),
            operand(20, Operator::Rise),
            operand(23, Operator::RenderMode),
        ]
    );
    let src = format!("/F 1 Tf {huge} Tr");
    assert_eq!(
        faults(src.as_bytes()),
        [operand(src.len() - 2, Operator::RenderMode)]
    );

    // A mode outside 0..=7; a real cut to its whole part is none.
    let (da, told) = da_faults(b"/F 1 Tf 8 Tr -1 Tr 2.9 Tr");
    assert_eq!(
        told,
        [
            Fault::DefaultAppearanceTextMode { at: 10 },
            Fault::DefaultAppearanceTextMode { at: 16 },
        ]
    );
    assert_eq!(da.unwrap().text_mode, Some(TextMode::FillStroke));

    // Unreadable: the error, at its offset, last, and no appearance. What was
    // told before it stays told.
    let unreadable =
        |at, kind| Fault::DefaultAppearanceUnreadable(pdf_content_streams::Error { at, kind });
    assert_eq!(
        da_faults(b"/Helv 12 Tf (unclosed"),
        (None, vec![unreadable(12, ErrorKind::String)])
    );
    assert_eq!(
        da_faults(b"Tc /Helv 12 Tf <0"),
        (
            None,
            vec![
                operand(0, Operator::CharSpace),
                unreadable(15, ErrorKind::HexString)
            ]
        )
    );
    // A cap passed is told by its kind, at the first operand past it.
    let mut limits = pdf_content_streams::Limits::DEFAULT;
    limits.operands = 2;
    assert_eq!(
        da_faults_under(b"/Helv 12 Tf 0.1 0.2 0.3 rg", &limits),
        (None, vec![unreadable(20, ErrorKind::Operands)])
    );

    // No `Tf` names a font: told last, with no offset.
    for nameless in [&b""[..], b"0.5 g", b"BT (x) Tj ET 5 w", b"/Helv 12"] {
        assert_eq!(
            faults(nameless),
            [Fault::DefaultAppearanceNoFont],
            "{nameless:?}"
        );
    }
    // In the order read.
    assert_eq!(
        faults(b"Tc /Helv 12 Tf 9 Tr"),
        [
            operand(0, Operator::CharSpace),
            Fault::DefaultAppearanceTextMode { at: 17 }
        ]
    );
}

/// Counts what it's told; `wants` as it's made, and fails if it's told
/// anything while it wants nothing.
struct Counted {
    wants: bool,
    told: usize,
}

impl pdf_objects::fault::Sink<Fault> for Counted {
    fn fault(&mut self, _: pdf_objects::fault::Place, fault: Fault) {
        assert!(self.wants, "told {fault:?}, though it wants none");
        self.told += 1;
    }

    fn wants(&self) -> bool {
        self.wants
    }
}

#[test]
fn default_appearance_parse_with_gives_what_parse_gives_whatever_the_sink() {
    let huge = format!("1{}.", "0".repeat(45));
    let sources: Vec<Vec<u8>> = [
        &b"/Helv 12 Tf 0 g"[..],
        b"/Helv Tf",
        b"12 12 Tf",
        b"/F 1 Tf Tc Tw Tz TL Ts Tr 8 Tr 0 0 rg",
        b"Tc /Helv 12 Tf <0",
        b"/Helv 12 Tf (unclosed",
        b"",
        b"/SECRET_FONT 12 Tf 0 0 1 rg 2 Tc 1 Tw 90 Tz 14 TL 1 Tr 3 Ts",
    ]
    .iter()
    .map(|s| s.to_vec())
    .chain([format!("{huge} Tc /F {huge} Tf {huge} g 9 Tr").into_bytes()])
    .collect();
    for src in &sources {
        let limits = pdf_content_streams::Limits::DEFAULT;
        let (parsed, told) = da_faults(src);
        // A sink that wants nothing is told nothing, and the value is the
        // same; one that wants faults is told each, through `dyn` too.
        let mut quiet = Counted {
            wants: false,
            told: 0,
        };
        assert_eq!(
            DefaultAppearance::parse_with(src, &limits, &mut quiet),
            parsed
        );
        assert_eq!(quiet.told, 0);
        let mut counted = Counted {
            wants: true,
            told: 0,
        };
        let sink: &mut dyn pdf_objects::fault::Sink<Fault> = &mut counted;
        assert_eq!(DefaultAppearance::parse_with(src, &limits, sink), parsed);
        assert_eq!(counted.told, told.len());
        // The faults name no font and quote no operand.
        for fault in &told {
            let dbg = format!("{fault:?}");
            assert!(!dbg.contains("SECRET") && !dbg.contains("Helv"), "{dbg}");
        }
    }

    // Each fault's `Debug`: offsets, the operator, the error's kind.
    use pdf_content_streams::{ErrorKind, Operator};
    for (fault, dbg) in [
        (
            Fault::DefaultAppearanceUnreadable(pdf_content_streams::Error {
                at: 12,
                kind: ErrorKind::String,
            }),
            "DefaultAppearanceUnreadable(Error { at: 12, kind: String })",
        ),
        (
            Fault::DefaultAppearanceOperand {
                at: 3,
                operator: Operator::Font,
            },
            "DefaultAppearanceOperand { at: 3, operator: Font }",
        ),
        (
            Fault::DefaultAppearanceTextMode { at: 10 },
            "DefaultAppearanceTextMode { at: 10 }",
        ),
        (Fault::DefaultAppearanceNoFont, "DefaultAppearanceNoFont"),
    ] {
        assert_eq!(format!("{fault:?}"), dbg);
    }
}

/// A TrueType program embedded in a document, laid out in as itself and through
/// `ProgramFont`, which records the glyphs it encodes.
#[cfg(all(feature = "embed", feature = "text-layout"))]
mod program {
    use std::sync::{Arc, Mutex};

    use pdf_fixtures::font;
    use pdf_fonts::embed::{Glyphs, Program};

    use crate::{LayoutFont, ProgramFont, Rect, TextBoxStyle, UnencodableChar, layout_text};

    fn program() -> Program {
        let mut fixture = font::sample(1000);
        fixture.names = vec![(4, "MARKER Full".into()), (6, "MARKER-PostScript".into())];
        Program::open(fixture.build(), 0).unwrap()
    }

    fn style() -> TextBoxStyle {
        TextBoxStyle {
            size: Some(10.0),
            ..TextBoxStyle::default()
        }
    }

    const BOX: Rect = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: 300.0,
        y1: 40.0,
    };

    /// Its metrics, each character's advance its glyph's width and its code its
    /// glyph's number, two bytes, high first; a character its `cmap` doesn't map
    /// has neither, and nothing is written for it.
    #[test]
    fn a_program_is_laid_out_in_as_its_glyphs() {
        let program = program();
        assert_eq!(program.font_metrics(), program.metrics());
        let a = program.glyph('A').unwrap();
        assert_eq!(program.char_advance('A'), Some(program.width(a)));
        assert_eq!(program.char_advance('\u{4E2D}'), Some(1000.0));
        let mut out = vec![7];
        assert!(program.encode_char('\u{1D11E}', &mut out));
        assert_eq!(out, [7, 0x01, 0x40]);
        assert_eq!(program.char_advance('z'), None);
        assert!(!program.encode_char('z', &mut out));
        assert!(!program.encode_char('\n', &mut out));
        assert_eq!(out, [7, 0x01, 0x40]);

        // A layout in it is the program's codes, through `&dyn LayoutFont` too.
        let text = "Ag H\u{C4}\u{4E2D}";
        let laid = layout_text(&program, text, BOX, &style()).unwrap();
        let codes = program.codes(text).unwrap();
        assert_eq!(laid.lines.len(), 1);
        assert_eq!(laid.lines[0].codes, codes);
        let dynamic: &dyn LayoutFont = &program;
        let again = layout_text(dynamic, text, BOX, &style()).unwrap();
        assert_eq!(again.lines[0].codes, codes);
        let refused = layout_text(&program, "Az", BOX, &style()).unwrap_err();
        assert_eq!(
            refused,
            UnencodableChar {
                at: 1,
                character: 'z'
            }
        );
    }

    /// `ProgramFont` lays text out as its program does, and records each glyph it
    /// encodes once, with the character it was drawn for: a layout refused
    /// part-way has recorded the glyphs before the refusal, and glyphs recorded by
    /// two fonts sharing one set are one set.
    #[test]
    fn a_program_font_records_each_glyph_it_encodes() {
        let program = program();
        let glyphs = Mutex::new(Glyphs::new());
        let font = ProgramFont::new(&program, &glyphs);
        assert!(std::ptr::eq(font.program(), &program));
        assert_eq!(font.font_metrics(), program.metrics());
        assert_eq!(font.char_advance('g'), program.char_advance('g'));
        assert_eq!(font.char_advance('z'), None);
        assert!(glyphs.lock().unwrap().is_empty());

        let text = "AgA gH";
        let laid = layout_text(&font, text, BOX, &style()).unwrap();
        let plain = layout_text(&program, text, BOX, &style()).unwrap();
        assert_eq!(laid.lines[0].codes, plain.lines[0].codes);
        assert_eq!(laid.lines[0].origin, plain.lines[0].origin);
        let mut expected = Glyphs::new();
        expected.add(&program, "Ag H").unwrap();
        assert_eq!(
            glyphs.lock().unwrap().iter().collect::<Vec<_>>(),
            expected.iter().collect::<Vec<_>>()
        );

        let refused = layout_text(&font, "\u{4E2D}z\u{6587}", BOX, &style()).unwrap_err();
        assert_eq!(refused.character, 'z');
        let recorded = glyphs.lock().unwrap().clone();
        assert_eq!(recorded.len(), 5);
        assert!(recorded.iter().any(|(_, text)| text == "\u{4E2D}"));
        assert!(!recorded.iter().any(|(_, text)| text == "\u{6587}"));

        // Two threads, one set.
        let program = Arc::new(program);
        let shared = Arc::new(Mutex::new(Glyphs::new()));
        std::thread::scope(|scope| {
            for text in ["\u{5E9}\u{5DC}", "\u{633}\u{644}"] {
                let (program, shared) = (&program, &shared);
                scope.spawn(move || {
                    let font = ProgramFont::new(program, shared);
                    layout_text(&font, text, BOX, &style()).unwrap();
                });
            }
        });
        assert_eq!(shared.lock().unwrap().len(), 4);
    }

    /// A lock a thread panicked holding is taken as it is: what's recorded is
    /// whole at every step, and recording goes on.
    #[test]
    fn a_poisoned_lock_is_taken_as_it_is() {
        let program = program();
        let glyphs = Mutex::new(Glyphs::new());
        glyphs.lock().unwrap().add(&program, "A").unwrap();
        let poisoned = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _held = glyphs.lock().unwrap();
                    panic!("poisons the lock");
                })
                .join()
        });
        assert!(poisoned.is_err() && glyphs.is_poisoned());
        let font = ProgramFont::new(&program, &glyphs);
        let laid = layout_text(&font, "gH", BOX, &style()).unwrap();
        assert_eq!(laid.lines[0].codes, program.codes("gH").unwrap());
        let recorded = glyphs.lock().unwrap_or_else(|p| p.into_inner()).len();
        assert_eq!(recorded, 3);
        assert!(format!("{font:?}").contains("glyphs: Some(3)"));
    }

    /// `Debug` gives the program's counts and how many glyphs are recorded, never
    /// a name of the program or a text drawn, and doesn't wait for a lock the
    /// caller holds. The font is `Send` and `Sync`, for threads drawing pages.
    #[test]
    fn a_program_font_prints_counts_alone() {
        let program = program();
        let glyphs = Mutex::new(Glyphs::new());
        let font = ProgramFont::new(&program, &glyphs);
        layout_text(&font, "MARKER", BOX, &style()).unwrap_err();
        glyphs.lock().unwrap().insert(9, "MARKER");
        let printed = format!("{font:?}");
        assert!(!printed.contains("MARKER"), "{printed}");
        assert!(
            printed.contains("glyphs: 321") && printed.contains("glyphs: Some(1)"),
            "{printed}"
        );
        let held = glyphs.lock().unwrap();
        assert!(format!("{font:?}").contains("glyphs: None"));
        drop(held);

        fn shared<T: Send + Sync>() {}
        shared::<ProgramFont<'static>>();
    }
}

/// Text in more than one font (`FontSet`, `WithFallback`), a character or a
/// grapheme cluster at a time.
#[cfg(feature = "text-layout")]
mod fallback {
    use pdf_content_streams::{Res, Reserved};
    use pdf_fonts::{FontMetrics, Std14};

    use super::assert_approx;
    use crate::{
        Canvas, FontSet, LaidOutText, LayoutFont, Rect, TextAlign, TextBoxError, TextBoxStyle,
        UnencodableChar, VerticalAlign, WithFallback, WrapMode, face_for, layout_text,
    };

    /// A font of the characters it's given, each `advance` wide, its code the
    /// character's place among them after `base`, with the heights given.
    struct Only {
        chars: &'static str,
        advance: f32,
        base: u8,
        ascent: f32,
        descent: f32,
    }

    impl LayoutFont for Only {
        fn font_metrics(&self) -> FontMetrics {
            FontMetrics::new(self.ascent, self.descent, None)
        }

        fn char_advance(&self, ch: char) -> Option<f32> {
            self.chars.contains(ch).then_some(self.advance)
        }

        fn encode_char(&self, ch: char, out: &mut Vec<u8>) -> bool {
            match self.chars.chars().position(|c| c == ch) {
                Some(i) => {
                    out.push(self.base + i as u8);
                    true
                }
                None => false,
            }
        }
    }

    /// Latin letters, a space and a hyphen, without a mark.
    const LATIN: Only = Only {
        chars: "abcde -",
        advance: 500.0,
        base: 1,
        ascent: 800.0,
        descent: -200.0,
    };

    /// An `e`, a combining acute and an `é`, taller.
    const MARKS: Only = Only {
        chars: "e\u{301}\u{E9}",
        advance: 400.0,
        base: 101,
        ascent: 1000.0,
        descent: -400.0,
    };

    /// Two more letters, `x` among them, and a hyphen.
    const LAST: Only = Only {
        chars: "xyz-",
        advance: 600.0,
        base: 201,
        ascent: 700.0,
        descent: -300.0,
    };

    fn res(font: Std14) -> Res {
        Res::dict(font.dict()).unwrap()
    }

    fn named(base: u8) -> Res {
        Res::dict(pdf_objects::dict! { "Base" => i64::from(base) }).unwrap()
    }

    fn style() -> TextBoxStyle {
        TextBoxStyle {
            size: Some(10.0),
            font_heights_as_given: true,
            leading_factor: 1.0,
            ..TextBoxStyle::default()
        }
    }

    const BOX: Rect = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: 200.0,
        y1: 100.0,
    };

    /// What `draw` writes on a canvas of its own, as text, each resource
    /// named `F` and its place among the resources it names, which it gives.
    fn drawn(draw: impl FnOnce(&mut Canvas)) -> (String, Vec<Res>) {
        let mut canvas = Canvas::new();
        draw(&mut canvas);
        let (bytes, needs) = canvas.finish(&Reserved::default());
        let mut content = String::from_utf8_lossy(&bytes).into_owned();
        for (i, need) in needs.0.iter().enumerate() {
            let name = format!("/{} ", String::from_utf8_lossy(&need.name));
            content = content.replace(&name, &format!("/F{i} "));
        }
        let named = needs.0.into_iter().map(|need| need.res).collect();
        (content, named)
    }

    /// The first face with an advance and a code for every character of a
    /// unit draws it; one no face draws whole is refused at the character
    /// the first face lacks, as a layout in it alone would refuse it.
    #[test]
    fn the_first_face_with_the_whole_unit_draws_it() {
        let set = WithFallback::new(&LATIN, &MARKS, named(101));
        assert_eq!(set.faces(), 2);
        assert_eq!(face_for(&set, "a"), Ok(0));
        assert_eq!(face_for(&set, "e"), Ok(0));
        assert_eq!(face_for(&set, "\u{E9}"), Ok(1));
        assert_eq!(face_for(&set, "\u{301}"), Ok(1));
        // The first face has the `e` but not its mark: the second, whole.
        assert_eq!(face_for(&set, "e\u{301}"), Ok(1));
        // A tab is a space; an empty unit the first face's.
        assert_eq!(face_for(&set, "\t"), Ok(0));
        assert_eq!(face_for(&set, ""), Ok(0));
        // Neither has all of it: the character the first face lacks.
        let refused = |at, character| Err(UnencodableChar { at, character });
        assert_eq!(face_for(&set, "q"), refused(0, 'q'));
        assert_eq!(face_for(&set, "a\u{301}"), refused(1, '\u{301}'));
        assert_eq!(face_for(&set, "\u{301}q"), refused(0, '\u{301}'));
        // One font is a set of one face.
        assert_eq!(LATIN.faces(), 1);
        assert_eq!(face_for(&LATIN, "ab"), Ok(0));
        assert_eq!(face_for(&LATIN, "\u{301}"), refused(0, '\u{301}'));
        assert_eq!(LATIN.face_advance(1, 'a'), None);
        assert!(!LATIN.face_encode(1, 'a', &mut Vec::new()));
        assert_eq!(LATIN.face_resource(0), None);

        // A set of no faces draws nothing.
        struct NoFaces;
        impl FontSet for NoFaces {
            fn faces(&self) -> usize {
                0
            }
            fn face_metrics(&self, _: usize) -> FontMetrics {
                FontMetrics::new(0.0, 0.0, None)
            }
            fn face_advance(&self, _: usize, _: char) -> Option<f32> {
                Some(500.0)
            }
            fn face_encode(&self, _: usize, _: char, _: &mut Vec<u8>) -> bool {
                true
            }
        }
        assert_eq!(face_for(&NoFaces, "ab"), refused(0, 'a'));
        assert_eq!(face_for(&NoFaces, ""), Ok(0));

        // A layout asks the same: a character at a time, the mark falls back
        // alone; by clusters, `a` and its mark have no face, refused at the
        // mark, at its offset in the text.
        let text = "ab a\u{301}";
        let laid = layout_text(&set, text, BOX, &style()).unwrap();
        let faces: Vec<_> = laid.lines[0].runs.iter().map(|r| r.face).collect();
        assert_eq!(faces, [0, 1]);
        let clusters = TextBoxStyle {
            clusters: true,
            ..style()
        };
        assert_eq!(
            layout_text(&set, text, BOX, &clusters).unwrap_err(),
            UnencodableChar {
                at: 4,
                character: '\u{301}'
            }
        );
    }

    /// With clusters, a base and its mark are one comb cell in one face, its
    /// codes one string centred by their advances together, and the cells
    /// are counted by clusters; a comb line's heights start from the first
    /// face's and widen by its cells' faces.
    #[test]
    fn a_cluster_is_one_comb_cell() {
        let set = WithFallback::new(&LATIN, &MARKS, named(101));
        let comb = |cells, clusters| TextBoxStyle {
            comb: Some(cells),
            clusters,
            ..style()
        };
        let text = "ae\u{301}b";
        let laid = layout_text(&set, text, BOX, &comb(4, true)).unwrap();
        let line = &laid.lines[0];
        assert!(!laid.overflowed);
        let faces: Vec<usize> = line.cells.iter().map(|c| c.face).collect();
        assert_eq!(faces, [0, 1, 0]);
        let codes: Vec<&[u8]> = line.cells.iter().map(|c| c.codes.as_slice()).collect();
        assert_eq!(codes, [&[1][..], &[101, 102], &[2]]);
        let ranges: Vec<_> = line.cells.iter().map(|c| c.text.clone()).collect();
        assert_eq!(ranges, [0..1, 1..4, 4..5]);
        assert!(!format!("{laid:?}").contains("101"));
        assert!(line.runs.is_empty() && line.codes.is_empty());
        // Each cell is 50 wide: the cluster is 8 wide, 400 units twice.
        assert_approx(line.cells[0].origin.x, 22.5);
        assert_approx(line.cells[1].origin.x, 50.0 + 21.0);
        assert_approx(line.cells[2].origin.x, 100.0 + 22.5);
        // The second face is drawn: its heights, 10 and -4, widen the line.
        assert_approx(line.ascent, 10.0);
        assert_approx(line.descent, -4.0);

        // A character at a time, the mark is a cell of its own.
        let laid = layout_text(&set, text, BOX, &comb(4, false)).unwrap();
        let faces: Vec<usize> = laid.lines[0].cells.iter().map(|c| c.face).collect();
        assert_eq!(faces, [0, 0, 1, 0]);
        // Three clusters past two cells: two drawn, and the text overflows;
        // in three they fit.
        let laid = layout_text(&set, text, BOX, &comb(2, true)).unwrap();
        assert!(laid.overflowed);
        assert_eq!(laid.lines[0].cells.len(), 2);
        assert!(
            !layout_text(&set, text, BOX, &comb(3, true))
                .unwrap()
                .overflowed
        );

        // In the first face alone, its heights; in the second alone, still
        // from the first face's, widened: `LAST` is shorter above.
        let laid = layout_text(&set, "ab", BOX, &comb(4, true)).unwrap();
        assert_approx(laid.lines[0].ascent, 8.0);
        assert_approx(laid.lines[0].descent, -2.0);
        let set = WithFallback::new(&LATIN, &LAST, named(201));
        let laid = layout_text(&set, "z", BOX, &comb(4, true)).unwrap();
        let faces: Vec<usize> = laid.lines[0].cells.iter().map(|c| c.face).collect();
        assert_eq!(faces, [1]);
        assert_approx(laid.lines[0].ascent, 8.0);
        assert_approx(laid.lines[0].descent, -3.0);
    }

    /// A comb draws the units its cells hold, but takes a face for every
    /// unit: one past the cells that only the second face has is laid out,
    /// where the first font alone refuses it, and one no face has is refused.
    #[test]
    fn a_comb_takes_a_face_for_the_units_past_its_cells() {
        let set = WithFallback::new(&LATIN, &MARKS, named(101));
        let comb = TextBoxStyle {
            comb: Some(4),
            ..style()
        };
        let text = "abcd\u{E9}";
        let laid = layout_text(&set, text, BOX, &comb).unwrap();
        assert!(laid.overflowed);
        let faces: Vec<usize> = laid.lines[0].cells.iter().map(|c| c.face).collect();
        assert_eq!(faces, [0; 4]);
        // The `é` isn't drawn, so the line has the first face's heights alone.
        assert_approx(laid.lines[0].ascent, 8.0);
        assert_approx(laid.lines[0].descent, -2.0);
        let refused = |at, character| Err(UnencodableChar { at, character });
        assert_eq!(layout_text(&LATIN, text, BOX, &comb), refused(4, '\u{E9}'));
        assert_eq!(layout_text(&set, "abcdq", BOX, &comb), refused(4, 'q'));
    }

    /// Each run's place is the advances before it on the line summed, with
    /// the word spacing of the spaces among them; it's drawn there, under its
    /// face's resource, by a `Tf` where the face changes.
    #[test]
    fn runs_are_placed_by_the_advances_before_them() {
        let helvetica = res(Std14::Helvetica);
        let symbol = res(Std14::Symbol);
        let set = WithFallback::new(&Std14::Helvetica, &Std14::Symbol, symbol.clone());
        let text = "ab \u{3B1}\u{3B2} cd";
        let laid = layout_text(&set, text, BOX, &style()).unwrap();
        let line = &laid.lines[0];
        let summed = |text: &str, font: Std14| {
            (text.chars()).fold(0.0f32, |x, ch| x + font.char_advance(ch).unwrap() * 0.01)
        };
        let before_alpha = summed("ab ", Std14::Helvetica);
        let after_beta = before_alpha + summed("\u{3B1}\u{3B2}", Std14::Symbol);
        let runs: Vec<_> = (line.runs.iter())
            .map(|r| (r.face, r.codes.clone(), r.text.clone(), r.x))
            .collect();
        assert_eq!(
            runs,
            [
                (0, 0..3, 0..3, 0.0),
                (1, 3..5, 3..7, before_alpha),
                (0, 5..8, 7..10, after_beta),
            ]
        );
        assert_eq!(line.codes, b"ab ab cd");
        assert_approx(line.width, after_beta + summed(" cd", Std14::Helvetica));

        let (content, named) =
            drawn(|c| assert!(c.draw_layout_in(&[helvetica.clone(), symbol.clone()], &laid)));
        assert_eq!(
            content,
            "BT\n/F0 10 Tf\n1 0 0 1 0 89.9 Tm\n(ab ) Tj\n/F1 10 Tf\n1 0 0 1 13.9 89.9 Tm\n\
             (ab) Tj\n/F0 10 Tf\n1 0 0 1 25.7 89.9 Tm\n( cd) Tj\nET\n"
        );
        assert_eq!(named, [helvetica.clone(), symbol.clone()]);

        // Justified, a run past a space `Tw` widens stands that much further.
        let justified = TextBoxStyle {
            align: TextAlign::Justify,
            ..style()
        };
        let narrow = Rect::new(0.0, 0.0, 40.0, 100.0);
        let laid = layout_text(&set, "a \u{3B1} bbbbbbbbbb", narrow, &justified).unwrap();
        let line = &laid.lines[0];
        assert_eq!(
            &"a \u{3B1} bbbbbbbbbb"[line.text_range.clone()],
            "a \u{3B1}"
        );
        assert!(line.word_space > 0.0);
        let before = summed("a ", Std14::Helvetica) + line.word_space;
        assert_eq!(line.runs[1].x, before);
        assert_approx(line.width, 40.0);
    }

    /// A line's ascent and descent are the largest and smallest of the faces
    /// on it, from `0`, an empty line's the first face's; a step runs from one
    /// line's descent to the next one's ascent. Lines all in one face stand as
    /// in that face alone.
    #[test]
    fn heights_of_lines_in_one_face_the_other_and_both() {
        let set = WithFallback::new(&LATIN, &LAST, named(201));
        let lines = |text, style: &TextBoxStyle| {
            let laid = layout_text(&set, text, BOX, style).unwrap();
            (laid.lines.iter())
                .map(|l| (l.origin.y, l.ascent, l.descent))
                .collect::<Vec<_>>()
        };
        let near = |got: Vec<(f32, f32, f32)>, want: &[(f32, f32, f32)]| {
            assert_eq!(got.len(), want.len(), "{got:?}");
            for (g, w) in got.iter().zip(want) {
                assert_approx(g.0, w.0);
                assert_approx(g.1, w.1);
                assert_approx(g.2, w.2);
            }
        };
        // `LATIN` 8 and -2 at size 10, `LAST` 7 and -3; a line of both 8 and
        // -3; an empty line `LATIN`'s.
        near(
            lines("ab\nxy\naxb\n\nab", &style()),
            &[
                (92.0, 8.0, -2.0),
                (92.0 - 9.0, 7.0, -3.0),
                (83.0 - 11.0, 8.0, -3.0),
                (72.0 - 11.0, 8.0, -2.0),
                (61.0 - 10.0, 8.0, -2.0),
            ],
        );
        // A taller second face: its line starts higher below the first.
        let tall = WithFallback::new(&LATIN, &MARKS, named(101));
        let laid = layout_text(&tall, "ab\n\u{E9}\nab", BOX, &style()).unwrap();
        let ys: Vec<f32> = laid.lines.iter().map(|l| l.origin.y).collect();
        assert_approx(ys[0], 92.0);
        assert_approx(ys[1], 92.0 - 12.0);
        assert_approx(ys[2], 80.0 - 12.0);
        // With leading, the step is that times the distance.
        let led = TextBoxStyle {
            leading_factor: 1.5,
            ..style()
        };
        assert_approx(lines("ab\nxy", &led)[1].0, 92.0 - 13.5);
        // A step given is every step.
        let given = TextBoxStyle {
            line_height: Some(20.0),
            ..style()
        };
        assert_approx(lines("ab\nxy\nab", &given)[2].0, 52.0);
        // A single line of both about the middle, and lines from the bottom.
        let middle = TextBoxStyle {
            vertical_align: VerticalAlign::Middle,
            ..style()
        };
        near(lines("ax", &middle), &[(50.0 - 2.5, 8.0, -3.0)]);
        let bottom = TextBoxStyle {
            vertical_align: VerticalAlign::Bottom,
            ..style()
        };
        near(
            lines("ab\nxy", &bottom),
            &[(3.0 + 9.0, 8.0, -2.0), (3.0, 7.0, -3.0)],
        );
        // A line starts from 0: a face below the baseline whole adds no
        // ascent above it.
        let low = Only {
            chars: "q",
            advance: 500.0,
            base: 1,
            ascent: -100.0,
            descent: -500.0,
        };
        let sunk = WithFallback::new(&LATIN, &low, named(1));
        let laid = layout_text(&sunk, "ab\nq", BOX, &style()).unwrap();
        assert_approx(laid.lines[1].ascent, 0.0);
        assert_approx(laid.lines[1].descent, -5.0);
        // Lines too tall together overflow: these two are 8 + 9 + 3 high.
        let fits = |height| {
            let short = Rect::new(0.0, 0.0, 200.0, height);
            !layout_text(&set, "ab\nxy", short, &style())
                .unwrap()
                .overflowed
        };
        assert!(fits(20.0) && !fits(19.9));

        // All in one face, the second or the first: as that face alone lays
        // them out, the arithmetic and all (at 11 points and a leading of
        // 1.2, a step taken six times isn't six steps in `f32`).
        let many = ("xy\nyxz\nzz\nxx\nyy\nzy\nxz", &LAST as &dyn LayoutFont);
        for (text, alone) in [many, ("ab\nba\nab\nba\nab\nba", &LATIN)] {
            for valign in [
                VerticalAlign::Top,
                VerticalAlign::Middle,
                VerticalAlign::Bottom,
            ] {
                let style = TextBoxStyle {
                    size: Some(11.0),
                    vertical_align: valign,
                    leading_factor: 1.2,
                    ..style()
                };
                let laid = layout_text(&set, text, BOX, &style).unwrap();
                let one = layout_text(alone, text, BOX, &style).unwrap();
                assert_eq!(laid.line_height, one.line_height);
                for (l, o) in laid.lines.iter().zip(&one.lines) {
                    assert_eq!(
                        (l.origin, l.width, &l.codes, l.ascent, l.descent),
                        (o.origin, o.width, &o.codes, o.ascent, o.descent)
                    );
                }
            }
        }
        // An empty line among the second face's is the first face's.
        near(
            lines("xy\n\nxy", &style()),
            &[
                (93.0, 7.0, -3.0),
                (93.0 - 11.0, 8.0, -2.0),
                (82.0 - 9.0, 7.0, -3.0),
            ],
        );
    }

    /// A set of one face, and a set whose first face has all of the text, lay
    /// it out and draw it as that font does, byte for byte, naming no other
    /// resource.
    #[test]
    fn a_set_whose_first_face_has_the_text_is_that_font() {
        let helvetica = res(Std14::Helvetica);
        let symbol = res(Std14::Symbol);
        let set = WithFallback::new(&Std14::Helvetica, &Std14::Symbol, symbol.clone());
        let one: &dyn FontSet = &Std14::Helvetica;
        let texts = [
            "The quick brown fox jumps over the lazy dog.\nSecond line, caf\u{E9}.",
            "  lead  and\ttrail  \r\n\r\nx",
            "42",
            "",
        ];
        let styles = [
            TextBoxStyle::default(),
            TextBoxStyle {
                align: TextAlign::Justify,
                ..TextBoxStyle::default()
            },
            TextBoxStyle {
                size: None,
                align: TextAlign::Center,
                vertical_align: VerticalAlign::Middle,
                ..TextBoxStyle::default()
            },
            TextBoxStyle {
                wrap: WrapMode::Char,
                align: TextAlign::Right,
                vertical_align: VerticalAlign::Bottom,
                line_end_spaces: true,
                ..TextBoxStyle::default()
            },
            TextBoxStyle {
                comb: Some(6),
                align: TextAlign::Center,
                ..TextBoxStyle::default()
            },
            TextBoxStyle {
                clusters: true,
                font_heights_as_given: true,
                leading_factor: 1.0,
                ..TextBoxStyle::default()
            },
        ];
        let rect = Rect::new(10.0, 20.0, 130.0, 80.0);
        for text in texts {
            for style in &styles {
                let alone = layout_text(&Std14::Helvetica, text, rect, style).unwrap();
                assert_eq!(layout_text(&set, text, rect, style).unwrap(), alone);
                assert_eq!(layout_text(one, text, rect, style).unwrap(), alone);
                let (want, names) = drawn(|c| assert!(c.draw_layout(helvetica.clone(), &alone)));
                let fonts = [helvetica.clone(), symbol.clone()];
                assert_eq!(
                    drawn(|c| assert!(c.draw_layout_in(&fonts, &alone))),
                    (want.clone(), names)
                );
                let boxed = |font: &dyn FontSet| {
                    drawn(|c| {
                        c.text_box_in(helvetica.clone(), font, text, rect, style)
                            .unwrap();
                    })
                };
                let alone = drawn(|c| {
                    c.text_box(helvetica.clone(), &Std14::Helvetica, text, rect, style)
                        .unwrap();
                });
                assert_eq!(boxed(&set), alone);
                assert_eq!(boxed(&Std14::Helvetica), alone);
                assert!(!boxed(&set).1.contains(&symbol));
            }
        }
        // Clusters of a character each are characters.
        let text = "caf\u{E9} au lait";
        let clustered = TextBoxStyle {
            clusters: true,
            ..TextBoxStyle::default()
        };
        assert_eq!(
            layout_text(&set, text, rect, &clustered).unwrap(),
            layout_text(&set, text, rect, &TextBoxStyle::default()).unwrap()
        );
        assert!(!TextBoxStyle::default().clusters);
    }

    /// Sets nest: three fonts, each drawn by `text_box_in` under its own
    /// resource, the same whichever way they're nested.
    #[test]
    fn sets_nest() {
        let (a, b, c) = (named(1), named(101), named(201));
        let inner = WithFallback::new(&LATIN, &MARKS, b.clone());
        let left = WithFallback::new(&inner, &LAST, c.clone());
        let deeper = WithFallback::new(&MARKS, &LAST, c.clone());
        let right = WithFallback::new(&LATIN, &deeper, b.clone());
        let text = "ab \u{E9}z";
        for set in [&left as &dyn FontSet, &right] {
            assert_eq!(set.faces(), 3);
            let faces: Vec<_> = ["a", "\u{E9}", "z"]
                .map(|u| face_for(set, u).unwrap())
                .into();
            assert_eq!(faces, [0, 1, 2]);
            assert_eq!(
                [0, 1, 2, 3].map(|f| set.face_resource(f)),
                [None, Some(b.clone()), Some(c.clone()), None]
            );
        }
        let boxed = |set: &dyn FontSet| {
            drawn(|canvas| {
                canvas
                    .text_box_in(a.clone(), set, text, BOX, &style())
                    .unwrap();
            })
        };
        let (content, named) = boxed(&left);
        assert_eq!((content.clone(), named.clone()), boxed(&right));
        assert_eq!(named, [a.clone(), b.clone(), c.clone()]);
        assert_eq!(content.matches(" Tf\n").count(), 3);
        assert!(content.contains("<010206> Tj\n/F1 10 Tf\n"));
        assert!(content.contains("(g) Tj\n/F2 10 Tf\n"));
        assert!(content.ends_with("(\u{FFFD}) Tj\nET\nQ\n"), "{content}");
    }

    /// Two faces, the second with no resource: a caller's own set that
    /// leaves [`FontSet::face_resource`] at its default.
    struct Unnamed;

    impl FontSet for Unnamed {
        fn faces(&self) -> usize {
            2
        }

        fn face_metrics(&self, face: usize) -> FontMetrics {
            [Std14::Helvetica, Std14::Symbol][face.min(1)].metrics()
        }

        fn face_advance(&self, face: usize, ch: char) -> Option<f32> {
            [Std14::Helvetica, Std14::Symbol]
                .get(face)?
                .char_advance(ch)
        }

        fn face_encode(&self, face: usize, ch: char, out: &mut Vec<u8>) -> bool {
            ([Std14::Helvetica, Std14::Symbol].get(face)).is_some_and(|f| f.encode_char(ch, out))
        }
    }

    /// A face some of the text is drawn in that has no resource: `text_box_in`
    /// refuses it with the face, and `draw_layout` and `draw_layout_in` say
    /// they drew nothing, each leaving the canvas as it was, never a part of
    /// the text drawn or a panic. Text the faces with a resource draw whole
    /// is drawn.
    #[test]
    fn a_face_with_no_resource_draws_nothing() {
        let helvetica = res(Std14::Helvetica);
        let clipped = TextBoxStyle {
            clip: true,
            ..style()
        };
        for style in [style(), clipped] {
            let (content, named) = drawn(|c| {
                assert_eq!(
                    c.text_box_in(helvetica.clone(), &Unnamed, "a\u{3C0}b", BOX, &style),
                    Err(TextBoxError::NoResource { face: 1 })
                );
                assert_eq!(
                    c.text_box_in(helvetica.clone(), &Unnamed, "a\u{4E2D}", BOX, &style),
                    Err(TextBoxError::Unencodable(UnencodableChar {
                        at: 1,
                        character: '\u{4E2D}'
                    }))
                );
            });
            assert_eq!((content.as_str(), named.len()), ("", 0));
            // The first face's text is drawn as the font alone draws it.
            let boxed = drawn(|c| {
                (c.text_box_in(helvetica.clone(), &Unnamed, "ab", BOX, &style)).unwrap();
            });
            let alone = drawn(|c| {
                (c.text_box(helvetica.clone(), &Std14::Helvetica, "ab", BOX, &style)).unwrap();
            });
            assert_eq!(boxed, alone);
            assert!(boxed.0.contains("(ab) Tj"));
        }
        assert_eq!(
            TextBoxError::NoResource { face: 1 }.to_string(),
            "face 1 of the fonts has no resource to draw text under"
        );
        let err = UnencodableChar {
            at: 1,
            character: 'x',
        };
        assert_eq!(TextBoxError::from(err).to_string(), err.to_string());

        // A layout in two faces, drawn with one resource, in a line and in
        // comb cells; with both it's drawn.
        let symbol = res(Std14::Symbol);
        let fonts = WithFallback::new(&Std14::Helvetica, &Std14::Symbol, symbol.clone());
        let comb = TextBoxStyle {
            comb: Some(4),
            ..style()
        };
        for style in [style(), comb] {
            let laid = layout_text(&fonts, "a\u{3C0}b", BOX, &style).unwrap();
            let (content, named) = drawn(|c| {
                assert!(!c.draw_layout(helvetica.clone(), &laid));
                assert!(!c.draw_layout_in(std::slice::from_ref(&helvetica), &laid));
                assert!(!c.draw_layout_in(&[], &laid));
            });
            assert_eq!((content.as_str(), named.len()), ("", 0));
            let (content, named) =
                drawn(|c| assert!(c.draw_layout_in(&[helvetica.clone(), symbol.clone()], &laid)));
            assert_eq!(named.len(), 2);
            assert_eq!(content.matches(" Tj\n").count(), 3);
        }
        // Nothing to draw is drawn, with no font at all.
        let empty = layout_text(&fonts, "", BOX, &style()).unwrap();
        assert_eq!(drawn(|c| assert!(c.draw_layout_in(&[], &empty))).0, "");
    }

    /// A line all in one face is drawn as its codes whole, as they are when
    /// it's drawn; a line in several by its runs, and one whose run names
    /// bytes its codes don't have keeps the layout undrawn.
    #[test]
    fn a_line_is_drawn_as_its_codes_or_its_runs() {
        let helvetica = res(Std14::Helvetica);
        let symbol = res(Std14::Symbol);
        let mut laid = layout_text(&Std14::Helvetica, "ab", BOX, &style()).unwrap();
        laid.lines[0].codes = b"abcd".to_vec();
        let (content, _) = drawn(|c| assert!(c.draw_layout(helvetica.clone(), &laid)));
        assert!(content.contains("(abcd) Tj"), "{content}");
        laid.lines[0].codes = b"a".to_vec();
        let (content, _) = drawn(|c| assert!(c.draw_layout(helvetica.clone(), &laid)));
        assert!(content.contains("(a) Tj"), "{content}");
        laid.lines[0].runs.clear();
        let (content, _) = drawn(|c| assert!(c.draw_layout(helvetica.clone(), &laid)));
        assert!(content.contains("(a) Tj"), "{content}");

        // A line all in a set's second face is its codes whole too.
        let fonts = WithFallback::new(&Std14::Helvetica, &Std14::Symbol, symbol.clone());
        let both = [helvetica.clone(), symbol.clone()];
        let mut laid = layout_text(&fonts, "\u{3C0}", BOX, &style()).unwrap();
        assert_eq!(laid.lines[0].runs.len(), 1);
        laid.lines[0].codes.push(b'p');
        let (content, named) = drawn(|c| assert!(c.draw_layout_in(&both, &laid)));
        assert!(content.contains("(pp) Tj"), "{content}");
        assert_eq!(named.as_slice(), std::slice::from_ref(&symbol));

        // In two faces: the runs' bytes, and none where a run has none.
        let mut laid = layout_text(&fonts, "a\u{3C0}b", BOX, &style()).unwrap();
        laid.lines[0].codes.extend_from_slice(b"cd");
        let (content, _) = drawn(|c| assert!(c.draw_layout_in(&both, &laid)));
        assert!(content.contains("(b) Tj") && !content.contains("cd"));
        laid.lines[0].codes.truncate(2);
        let (content, named) = drawn(|c| assert!(!c.draw_layout_in(&both, &laid)));
        assert_eq!((content.as_str(), named.len()), ("", 0));
    }

    /// `layout_text` and `text_box` take a `&dyn LayoutFont`, and
    /// `layout_text` and `text_box_in` a `&dyn FontSet`, as they take the
    /// font itself.
    #[test]
    fn layout_text_and_text_box_take_a_dyn_layout_font() {
        let font: &dyn LayoutFont = &Std14::Helvetica;
        let text = "Hello, world";
        let laid = layout_text(font, text, BOX, &style()).unwrap();
        assert_eq!(
            laid,
            layout_text(&Std14::Helvetica, text, BOX, &style()).unwrap()
        );
        let helvetica = res(Std14::Helvetica);
        let mut canvas = Canvas::new();
        let boxed: LaidOutText = canvas
            .text_box(helvetica.clone(), font, text, BOX, &style())
            .unwrap();
        assert_eq!(boxed, laid);
        let set: &dyn FontSet = &font;
        assert_eq!(layout_text(set, text, BOX, &style()).unwrap(), laid);
        assert_eq!(
            canvas.text_box_in(helvetica, set, text, BOX, &style()),
            Ok(laid)
        );
    }

    /// With clusters, a line breaks between clusters alone, a word too long
    /// for its line among them; a character at a time, inside one.
    #[test]
    fn clusters_break_lines_between_clusters() {
        let marked = Only {
            chars: "e\u{301} ",
            advance: 500.0,
            base: 1,
            ascent: 800.0,
            descent: -200.0,
        };
        // Each character 5 wide, each cluster 10, in a box 15 wide.
        let narrow = Rect::new(0.0, 0.0, 15.0, 100.0);
        let text = "e\u{301}e\u{301}e\u{301}";
        let shown = |clusters, wrap| {
            let style = TextBoxStyle {
                clusters,
                wrap,
                ..style()
            };
            let laid = layout_text(&marked, text, narrow, &style).unwrap();
            (laid.lines.iter())
                .map(|l| text[l.text_range.clone()].to_string())
                .collect::<Vec<_>>()
        };
        for wrap in [WrapMode::Char, WrapMode::Word] {
            assert_eq!(shown(false, wrap), ["e\u{301}e", "\u{301}e\u{301}"]);
            assert_eq!(shown(true, wrap), ["e\u{301}", "e\u{301}", "e\u{301}"]);
        }
        // A space a mark is joined to is shown, never trimmed or passed over
        // where a line begins with it.
        let style = TextBoxStyle {
            clusters: true,
            ..style()
        };
        let laid = layout_text(&marked, "e \u{301}", narrow, &style).unwrap();
        assert_eq!(laid.lines.len(), 1);
        assert_eq!(laid.lines[0].codes, [1, 3, 2]);
        let ten = Rect::new(0.0, 0.0, 10.0, 100.0);
        let laid = layout_text(&marked, "ee \u{301}", ten, &style).unwrap();
        let codes: Vec<_> = laid.lines.iter().map(|l| l.codes.clone()).collect();
        assert_eq!(codes, [vec![1, 1], vec![3, 2]]);
        // Keeping spaces, a word begins after a cluster a space begins, as
        // after the space: the line breaks before it.
        let kept = TextBoxStyle {
            line_end_spaces: true,
            ..style
        };
        let text = "e \u{301}eee";
        let twenty = Rect::new(0.0, 0.0, 20.0, 100.0);
        let laid = layout_text(&marked, text, twenty, &kept).unwrap();
        let lines: Vec<_> = laid
            .lines
            .iter()
            .map(|l| &text[l.text_range.clone()])
            .collect();
        assert_eq!(lines, ["e \u{301}", "eee"]);
    }

    /// `Debug` gives counts and places, never a code or a resource.
    #[test]
    fn debug_gives_counts_alone() {
        let set = WithFallback::new(&LATIN, &MARKS, named(101));
        assert_eq!(format!("{set:?}"), "WithFallback { faces: 2 }");
        let laid = layout_text(&set, "ab\u{E9}", BOX, &style()).unwrap();
        let printed = format!("{:?}", laid.lines[0]);
        assert!(
            printed.contains("runs: [LaidOutRun { face: 0, codes: 0..2"),
            "{printed}"
        );
        assert!(!printed.contains("101"), "{printed}");
    }
}

#[test]
fn transparency_group_resets_entry_state_and_user_segments_match_ctm() {
    let mut store = Store::default();
    let smask_group = store.put_stream(
        dict! {
            "Type" => Object::name("XObject"),
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(10), Object::from(10)],
        },
        b"0 0 10 10 re f".to_vec(),
    );
    let tr_fn = store.put(Object::from(dict! { "FunctionType" => 2, "N" => 1.0 }));
    let group_form = store.put_stream(
        dict! {
            "Type" => Object::name("XObject"),
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(50), Object::from(50)],
            "Group" => dict! {
                "S" => Object::name("Transparency"),
                "I" => true,
                "K" => true,
                "CS" => Object::name("DeviceRGB"),
            },
        },
        b"1 2 3 4 re B".to_vec(),
    );
    let plain_form = store.put_stream(
        dict! {
            "Type" => Object::name("XObject"),
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(50), Object::from(50)],
        },
        b"1 2 3 4 re f".to_vec(),
    );

    let bc_20: Vec<Object> = (0..20).map(|i| Object::from(f64::from(i) * 0.05)).collect();
    let resources = Object::from(dict! {
        "ExtGState" => dict! {
            "GS1" => dict! {
                "CA" => 0.3,
                "ca" => 0.6,
                "BM" => Object::name("Multiply"),
                "SMask" => dict! {
                    "S" => Object::name("Luminosity"),
                    "G" => smask_group,
                    "BC" => bc_20,
                    "TR" => tr_fn,
                },
            },
        },
        "XObject" => dict! {
            "Grp" => group_form,
            "Pln" => plain_form,
        },
    });

    let fonts = Fonts::default();
    let content = Concat::new(&[b"2 0 0 3 5 7 cm /GS1 gs /Grp Do /Pln Do 10 20 m 30 40 l S"]);

    let mut form_begin_alpha = None;
    let mut inside_group_state = None;
    let mut inside_plain_state = None;
    let mut outer_after_forms_state = None;
    let mut user_and_dev_matches = false;

    interpret(
        store.as_streams(),
        &content,
        Some(&resources),
        &Limits::DEFAULT,
        &fonts,
        Opts::default(),
        |ev| {
            match &ev.kind {
                Kind::FormBegin(f) if f.form == group_form => {
                    assert!(f.group.unwrap().isolated && f.group.unwrap().knockout);
                    form_begin_alpha = Some((ev.state.stroke_alpha, ev.state.fill_alpha));
                }
                Kind::Path(p) => match ev.at.stream.map(|u| u.depth()) {
                    Some(1) if inside_group_state.is_none() => {
                        inside_group_state = Some(ev.state.clone());
                    }
                    Some(1) => {
                        inside_plain_state = Some(ev.state.clone());
                    }
                    None => {
                        outer_after_forms_state = Some(ev.state.clone());
                        let mapped: Vec<Seg> = p
                            .user_segments
                            .iter()
                            .map(|s| match *s {
                                Seg::Move(pt) => Seg::Move(ev.state.ctm.apply(pt)),
                                Seg::Line(pt) => Seg::Line(ev.state.ctm.apply(pt)),
                                Seg::Curve(a, b, c) => Seg::Curve(
                                    ev.state.ctm.apply(a),
                                    ev.state.ctm.apply(b),
                                    ev.state.ctm.apply(c),
                                ),
                                Seg::Close => Seg::Close,
                            })
                            .collect();
                        user_and_dev_matches = mapped == p.segments;
                    }
                    _ => {}
                },
                _ => {}
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();

    // `FormBegin` sees outer `CA=0.3, ca=0.6`:
    let (ba_s, ba_f) = form_begin_alpha.unwrap();
    assert_approx(ba_s, 0.3);
    assert_approx(ba_f, 0.6);

    // Inside transparency group: `CA=1, ca=1, BM=Normal, SMask=None`:
    let ig = inside_group_state.unwrap();
    assert_approx(ig.stroke_alpha, 1.0);
    assert_approx(ig.fill_alpha, 1.0);
    assert!(ig.blend_mode.is_normal());
    assert_eq!(ig.soft_mask, SoftMask::None);
    assert!(ig.soft_mask_params.is_none());

    // Inside non-group Form XObject: inherits `CA=0.3, ca=0.6, BM=Multiply, SMask`:
    let ip = inside_plain_state.unwrap();
    assert_approx(ip.stroke_alpha, 0.3);
    assert_approx(ip.fill_alpha, 0.6);
    assert_eq!(ip.blend_mode, BlendState::Mode(Blend::Multiply));

    // After both Form XObjects exit: outer state still has `CA=0.3, ca=0.6`, 20-element `/BC`, and `/TR` ref!
    let outer = outer_after_forms_state.unwrap();
    assert_approx(outer.stroke_alpha, 0.3);
    assert_approx(outer.fill_alpha, 0.6);
    let smp = outer.soft_mask_params.as_ref().unwrap();
    assert_eq!(smp.backdrop.as_ref().unwrap().as_slice().len(), 20);
    assert_eq!(smp.transfer.as_deref(), Some(&Object::from(tr_fn)));
    assert!(user_and_dev_matches);
}
