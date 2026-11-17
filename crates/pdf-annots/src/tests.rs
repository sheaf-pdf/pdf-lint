//! Unit, invariant, resource-cap, and hygiene tests for `pdf-annots`.

use std::collections::BTreeMap;

use pdf_content_streams::Color;
use pdf_gfx::{Matrix, Point, Rect};
use pdf_objects::pdf_tokenizer::Rules;
use pdf_objects::{Dict, Form, Object, ObjectId, Str, Stream, Value, Why, dict};
use pdf_semantics::pages;

use crate::{
    Action, AdditionalActions, Annot, AnnotColor, AnnotFlags, AnnotMut, AnnotSubtype, AnnotThread,
    AppearanceChars, AppearanceEntry, AppearanceMode, Appearances, AttachmentIcon, Border,
    BorderStyle, BuiltAnnot, Cap, CaretAnnot, CaretSymbol, DefaultAppearance, Dest, DestFit,
    DestNameKind, DestTable, Error, ExplicitDest, Fault, FileAttachmentAnnot, FlattenOpts,
    FlattenPlan, FlattenTarget, FreeTextAnnot, FreeTextIntent, HighlightMode, InkAnnot, Limits,
    LineAnnot, LineEnding, LinkAnnot, MarkedState, MarkupInfo, NamedAction, OpenAction, PageTarget,
    PaintStyle, PolyAnnot, PolyKind, PopupAnnot, QuadPoints, Quadding, RedactAnnot, ReplyKind,
    ResolvedDest, ReviewState, ShapeAnnot, ShapeKind, StampAnnot, StampName, SynthesizedAp,
    TabOrder, TextAnnot, TextIcon, TextMarkupAnnot, TextMarkupKind, TypedAnnot, WidgetAnnot, fuzz,
    page_annots, page_annots_with, page_tabs, placement_matrix, plan_flatten,
    plan_flatten_filtered, push_page_annot, resolve_dest, resolve_threads, stream_placement_matrix,
    synth_annot,
};

#[derive(Default)]
struct Store {
    objects: BTreeMap<ObjectId, Object>,
    next: u32,
}

impl Store {
    fn put(&mut self, obj: impl Into<Object>) -> ObjectId {
        self.next += 1;
        let id = ObjectId::new(self.next, 0);
        self.objects.insert(id, obj.into());
        id
    }

    fn put_stream(&mut self, dict: Dict, bytes: Vec<u8>) -> ObjectId {
        self.put(Object::from(Stream::new(dict, bytes)))
    }
}

#[test]
fn public_types_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Limits>();
    check::<Cap>();
    check::<Error>();
    check::<Fault>();
    check::<AnnotFlags>();
    check::<AnnotColor>();
    check::<BorderStyle>();
    check::<Border>();
    check::<PaintStyle>();
    check::<LineEnding>();
    check::<Quadding>();
    check::<AppearanceChars>();
    check::<DefaultAppearance>();
    check::<AppearanceMode>();
    check::<AppearanceEntry>();
    check::<Appearances>();
    check::<PageTarget>();
    check::<DestFit>();
    check::<ExplicitDest>();
    check::<DestNameKind>();
    check::<Dest>();
    check::<ResolvedDest>();
    check::<DestTable>();
    check::<NamedAction>();
    check::<Action>();
    check::<AdditionalActions>();
    check::<AnnotSubtype>();
    check::<TextIcon>();
    check::<TextAnnot>();
    check::<HighlightMode>();
    check::<LinkAnnot>();
    check::<FreeTextIntent>();
    check::<FreeTextAnnot>();
    check::<TextMarkupKind>();
    check::<TextMarkupAnnot>();
    check::<LineAnnot>();
    check::<ShapeKind>();
    check::<ShapeAnnot>();
    check::<PolyKind>();
    check::<PolyAnnot>();
    check::<InkAnnot>();
    check::<StampName>();
    check::<StampAnnot>();
    check::<CaretSymbol>();
    check::<CaretAnnot>();
    check::<PopupAnnot>();
    check::<AttachmentIcon>();
    check::<FileAttachmentAnnot>();
    check::<RedactAnnot>();
    check::<WidgetAnnot>();
    check::<TypedAnnot>();
    check::<ReplyKind>();
    check::<MarkedState>();
    check::<ReviewState>();
    check::<AnnotThread>();
    check::<TabOrder>();
    check::<MarkupInfo>();
    check::<QuadPoints>();
    check::<Annot<'static>>();
    check::<OpenAction>();
    check::<SynthesizedAp>();
    check::<BuiltAnnot>();
    check::<FlattenTarget>();
    check::<FlattenOpts>();
    check::<FlattenPlan>();
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
fn debug_hygiene_never_leaks_plaintext_uris_or_names() {
    let mut store = Store::default();
    let fs_id = store.put(dict! {
        "Type" => Object::name("Filespec"),
        "UF" => Object::text("SECRET_ATTACHMENT_NAME.xml"),
    });
    let a1 = store.put(dict! {
        "Subtype" => Object::name("Link"),
        "Rect" => vec![Object::from(0), Object::from(0), Object::from(50), Object::from(20)],
        "Contents" => Object::text("SECRET_CONTENTS_BODY"),
        "NM" => Object::text("SECRET_UNIQUE_NM"),
        "A" => dict! {
            "S" => Object::name("URI"),
            "URI" => Object::text("https://secret.internal.example/token=XYZ123"),
        },
    });
    let a2 = store.put(dict! {
        "Subtype" => Object::name("Redact"),
        "Rect" => vec![Object::from(10), Object::from(10), Object::from(80), Object::from(30)],
        "T" => Object::text("SECRET_AUTHOR_NAME"),
        "Subj" => Object::text("SECRET_SUBJECT_LINE"),
        "RC" => Object::text("SECRET_RICH_TEXT"),
        "OverlayText" => Object::text("SECRET_OVERLAY_TEXT"),
        "DA" => Object::text("/SECRET_FONT_RES 11 Tf 0 g"),
    });
    let a3 = store.put(dict! {
        "Subtype" => Object::name("FileAttachment"),
        "Rect" => vec![Object::from(0), Object::from(0), Object::from(16), Object::from(16)],
        "FS" => fs_id,
        "Name" => Object::name("SECRET_CUSTOM_ICON"),
    });
    let a4 = store.put(dict! {
        "Subtype" => Object::name("SECRET_SUBTYPE_NAME"),
        "Rect" => vec![Object::from(0), Object::from(0), Object::from(10), Object::from(10)],
    });
    let a5 = store.put(dict! {
        "Subtype" => Object::name("Widget"),
        "Rect" => vec![Object::from(0), Object::from(0), Object::from(60), Object::from(20)],
        "MK" => dict! {
            "CA" => Object::text("SECRET_BUTTON_CAPTION"),
        },
        "A" => dict! {
            "S" => Object::name("Named"),
            "N" => Object::name("SECRET_NAMED_ACTION"),
        },
    });

    let a6 = store.put(dict! {
        "Subtype" => Object::name("Text"),
        "Rect" => vec![Object::from(0), Object::from(0), Object::from(16), Object::from(16)],
        "StateModel" => Object::text("SECRET_STATE_MODEL"),
        "State" => Object::text("SECRET_STATE_VALUE"),
    });

    let built = BuiltAnnot::text_note(
        Rect::new(0.0, 0.0, 20.0, 20.0),
        "SECRET_NOTE_BODY",
        TextIcon::Comment,
        AnnotColor::Rgb([1.0, 0.9, 0.2]),
    );
    let mut raw_mut_dict = dict! { "Contents" => Object::text("SECRET_MUT_TEXT") };
    let annot_mut = AnnotMut::new(&mut raw_mut_dict);

    let page = dict! {
        "Type" => Object::name("Page"),
        "Annots" => vec![
            Object::from(a1),
            Object::from(a2),
            Object::from(a3),
            Object::from(a4),
            Object::from(a5),
            Object::from(a6),
        ],
    };
    let mut faults = Vec::new();
    let annots = page_annots_with(
        &store.objects,
        &page,
        None,
        Rules::Pdfium,
        &Limits::DEFAULT,
        &mut faults,
    )
    .unwrap();
    let dbg = format!("{annots:?} {faults:?} {built:?} {annot_mut:?}");
    for secret in [
        "SECRET_ATTACHMENT_NAME",
        "SECRET_CONTENTS_BODY",
        "SECRET_UNIQUE_NM",
        "secret.internal.example",
        "XYZ123",
        "SECRET_AUTHOR_NAME",
        "SECRET_SUBJECT_LINE",
        "SECRET_RICH_TEXT",
        "SECRET_OVERLAY_TEXT",
        "SECRET_FONT_RES",
        "SECRET_CUSTOM_ICON",
        "SECRET_SUBTYPE_NAME",
        "SECRET_BUTTON_CAPTION",
        "SECRET_NAMED_ACTION",
        "SECRET_STATE_MODEL",
        "SECRET_STATE_VALUE",
        "SECRET_NOTE_BODY",
        "SECRET_MUT_TEXT",
    ] {
        assert!(
            !dbg.contains(secret),
            "Debug output leaked {secret:?}: {dbg}"
        );
    }
}

#[test]
fn fuzz_seeds_pass_all_invariants() {
    for seed in fuzz::SEEDS {
        fuzz::check(seed);
    }
}

#[test]
fn subtypes_borders_da_and_encrypted_string_locked_error() {
    let mut store = Store::default();
    let ft = store.put(dict! {
        "Subtype" => Object::name("FreeText"),
        "Rect" => vec![Object::from(100), Object::from(200), Object::from(50), Object::from(120)],
        "DA" => Object::text("/Helv 14 Tf 0.25 0.5 0.75 rg"),
        "Q" => 2,
        "IT" => Object::name("FreeTextCallout"),
        "CL" => vec![
            Object::from(30), Object::from(110),
            Object::from(45), Object::from(140),
            Object::from(50), Object::from(140),
        ],
        "LE" => Object::name("ClosedArrow"),
        "BS" => dict! {
            "W" => 2.5,
            "S" => Object::name("D"),
            "D" => vec![Object::from(6), Object::from(3)],
        },
        "BE" => dict! {
            "S" => Object::name("C"),
            "I" => 1.5,
        },
    });
    let line = store.put(dict! {
        "Subtype" => Object::name("Line"),
        "Rect" => vec![Object::from(0), Object::from(0), Object::from(100), Object::from(100)],
        "L" => vec![Object::from(10), Object::from(20), Object::from(80), Object::from(90)],
        "LE" => vec![Object::name("Circle"), Object::name("OpenArrow")],
        "IC" => vec![Object::from(1.0), Object::from(0.0), Object::from(0.0)],
        "Cap" => true,
        "Border" => vec![Object::from(2), Object::from(4), Object::from(1.5)],
    });
    let page = dict! {
        "Tabs" => Object::name("S"),
        "Annots" => vec![Object::from(ft), Object::from(line)],
    };

    assert_eq!(
        page_tabs(
            &store.objects,
            &page,
            None,
            Rules::Iso,
            &Limits::DEFAULT,
            &mut ()
        )
        .unwrap(),
        Some(TabOrder::Structure)
    );
    let annots = page_annots(&store.objects, &page, None, Rules::Iso, &Limits::DEFAULT).unwrap();
    assert_eq!(annots.len(), 2);
    assert_eq!(annots[0].rect(), Rect::new(50.0, 120.0, 100.0, 200.0));
    assert_eq!(annots[0].raw_rect(), [100.0, 200.0, 50.0, 120.0]);
    assert_eq!(annots[0].border().width, 2.5);
    assert_eq!(annots[0].border().style, BorderStyle::Dashed);
    assert_eq!(annots[0].border().dash, vec![6.0, 3.0]);
    assert_eq!(annots[0].border().cloudy_intensity, Some(1.5));

    let TypedAnnot::FreeText(ft_view) = annots[0].typed() else {
        panic!("expected FreeText");
    };
    let da = ft_view.da.as_ref().unwrap();
    assert_eq!(da.font_name.as_deref(), Some(&b"Helv"[..]));
    assert_eq!(da.font_size, Some(14.0));
    assert_eq!(da.color, Some(Color::Rgb([0.25, 0.5, 0.75])));
    let reparsed = DefaultAppearance::parse(&da.to_bytes(), &Limits::DEFAULT.content).unwrap();
    assert_eq!(reparsed, *da);
    assert_eq!(ft_view.quadding, Quadding::Right);
    assert_eq!(ft_view.intent, FreeTextIntent::Callout);
    assert_eq!(ft_view.callout_line.len(), 3);
    assert_eq!(ft_view.line_ending, LineEnding::ClosedArrow);

    let TypedAnnot::Line(line_view) = annots[1].typed() else {
        panic!("expected Line");
    };
    assert_eq!(
        line_view.endpoints,
        Some((Point::new(10.0, 20.0), Point::new(80.0, 90.0)))
    );
    assert_eq!(
        line_view.endings,
        (LineEnding::Circle, LineEnding::OpenArrow)
    );
    assert!(line_view.caption);
    assert_eq!(annots[1].legacy_border(), Some([2.0, 4.0, 1.5]));

    // Encrypted string without password must fail with `Why::Locked`.
    let mut locked_dict = dict! {
        "Subtype" => Object::name("Text"),
        "Rect" => vec![Object::from(0), Object::from(0), Object::from(10), Object::from(10)],
    };
    locked_dict.set(
        "Contents",
        Object::from(Value::String(
            Str::Encrypted(vec![1, 2, 3].into()),
            Some(Form::Literal),
        )),
    );
    let err = Annot::from_dict(
        &store.objects,
        &locked_dict,
        Some(ObjectId::new(99, 0)),
        Rules::Pdfium,
        &Limits::DEFAULT,
    )
    .unwrap_err();
    assert!(matches!(err, Error::Unreadable(u) if u.why == Why::Locked));
}

#[test]
fn appearance_states_and_section_12_5_5_placement_matrix() {
    let mut store = Store::default();
    let off_id = store.put_stream(
        dict! {
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(10), Object::from(10)],
        },
        b"0 g 0 0 10 10 re S".to_vec(),
    );
    let yes_id = store.put_stream(
        dict! {
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(20), Object::from(10)],
            "Matrix" => vec![
                Object::from(2), Object::from(0),
                Object::from(0), Object::from(2),
                Object::from(10), Object::from(20),
            ],
        },
        b"0 g 0 0 20 10 re f".to_vec(),
    );
    let widget = dict! {
        "Subtype" => Object::name("Widget"),
        "Rect" => vec![Object::from(100), Object::from(200), Object::from(180), Object::from(240)],
        "AS" => Object::name("Yes"),
        "AP" => dict! {
            "N" => dict! {
                "Off" => off_id,
                "Yes" => yes_id,
            },
        },
    };
    let annot =
        Annot::from_dict(&store.objects, &widget, None, Rules::Iso, &Limits::DEFAULT).unwrap();
    let ap = annot.appearances().unwrap();
    assert_eq!(ap.stream(AppearanceMode::Normal, true, false), Some(yes_id));
    // Rollover falls back to Normal when `fallback_to_normal = true`.
    assert_eq!(
        ap.stream(AppearanceMode::Rollover, true, true),
        Some(yes_id)
    );

    // Transformed BBox: `[0,0,20,10]` under `scale(2,2) + translate(10,20)` = `[10, 20, 50, 40]` (40x20).
    // Placed onto `annot_rect = [100, 200, 180, 240]` (80x40) -> `sx = 2, sy = 2`:
    // `tx = 100 - 10*2 = 80`, `ty = 200 - 20*2 = 160`!
    let (_, form_mat, place) =
        stream_placement_matrix(&store.objects, yes_id, annot.rect(), &Limits::DEFAULT)
            .unwrap()
            .unwrap();
    assert_eq!(
        place,
        Matrix {
            a: 2.0,
            b: 0.0,
            c: 0.0,
            d: 2.0,
            e: 80.0,
            f: 160.0,
        }
    );
    let combined = form_mat.then(place);
    assert_eq!(
        combined.apply_rect(Rect::new(0.0, 0.0, 20.0, 10.0)),
        annot.rect()
    );
    assert_eq!(
        placement_matrix(
            Rect::new(5.0, 5.0, 5.0, 10.0),
            Matrix::IDENTITY,
            annot.rect()
        ),
        None
    );
}

#[test]
fn dest_iso_vs_pdfium_action_cycle_and_reply_threads() {
    let mut store = Store::default();
    let p1 = ObjectId::new(10, 0);
    let p2 = ObjectId::new(11, 0);
    let pages_id = ObjectId::new(9, 0);
    store.objects.insert(
        pages_id,
        Object::from(dict! {
            "Type" => Object::name("Pages"),
            "Kids" => vec![Object::from(p1), Object::from(p2)],
            "Count" => 2,
            "MediaBox" => vec![Object::from(0), Object::from(0), Object::from(200), Object::from(200)],
        }),
    );
    store.objects.insert(
        p1,
        Object::from(dict! { "Type" => Object::name("Page"), "Parent" => pages_id }),
    );
    store.objects.insert(
        p2,
        Object::from(dict! { "Type" => Object::name("Page"), "Parent" => pages_id }),
    );

    let catalog = dict! {
        "Type" => Object::name("Catalog"),
        "Pages" => pages_id,
        "Names" => dict! {
            "Dests" => dict! {
                "Names" => vec![
                    Object::text("in_tree"),
                    Object::from(dict! { "D" => vec![Object::from(p2), Object::name("Fit")] }),
                ],
            },
        },
        "Dests" => dict! {
            "in_dict" => vec![Object::from(p1), Object::name("XYZ"), Object::from(12), Object::from(34), Object::null()],
        },
    };

    let table = pages::pages(
        &store.objects,
        &catalog,
        Rules::Pdfium,
        &pdf_semantics::Limits::DEFAULT,
        &mut (),
    )
    .unwrap();

    // Cross-boundary `/Name` in `/Names/Dests`: rejected under `Rules::Iso`, accepted under `Rules::Pdfium` with fault.
    let cross_dest = Dest::Named {
        name: b"in_tree".to_vec(),
        kind: DestNameKind::Name,
    };
    assert_eq!(
        resolve_dest(
            &store.objects,
            Some(&catalog),
            &table,
            &cross_dest,
            Rules::Iso,
            &pdf_semantics::Limits::DEFAULT,
            &mut ()
        )
        .unwrap(),
        None
    );
    let mut dfaults = Vec::new();
    let pdfium_res = resolve_dest(
        &store.objects,
        Some(&catalog),
        &table,
        &cross_dest,
        Rules::Pdfium,
        &pdf_semantics::Limits::DEFAULT,
        &mut dfaults,
    )
    .unwrap()
    .unwrap();
    assert_eq!(pdfium_res.page_index, 1);
    assert_eq!(pdfium_res.fit, DestFit::Fit);
    assert!(
        dfaults
            .iter()
            .any(|(_, f)| *f == pdf_semantics::Fault::DestCrossResolved)
    );

    // Cyclic `/Next` action chain terminates cleanly with `Fault::Semantics(ActionInvalid)`.
    let act1 = ObjectId::new(20, 0);
    let act2 = ObjectId::new(21, 0);
    store.objects.insert(
        act1,
        Object::from(dict! {
            "S" => Object::name("URI"),
            "URI" => Object::text("https://a.example"),
            "Next" => act2,
        }),
    );
    store.objects.insert(
        act2,
        Object::from(dict! {
            "S" => Object::name("Named"),
            "N" => Object::name("NextPage"),
            "Next" => act1,
        }),
    );
    let link_dict = dict! {
        "Subtype" => Object::name("Link"),
        "Rect" => vec![Object::from(0), Object::from(0), Object::from(20), Object::from(20)],
        "A" => act1,
    };
    let mut afaults = Vec::new();
    let link_annot = Annot::from_dict_with(
        &store.objects,
        &link_dict,
        None,
        Rules::Pdfium,
        &Limits::DEFAULT,
        &mut afaults,
    )
    .unwrap();
    let TypedAnnot::Link(lnk) = link_annot.typed() else {
        panic!("expected Link");
    };
    assert_eq!(lnk.actions.len(), 2);
    assert!(
        afaults
            .iter()
            .any(|(_, f)| *f == Fault::Semantics(pdf_semantics::Fault::ActionInvalid))
    );

    // Reply threads + review state updates + popup exclusion.
    let root_id = store.put(dict! {
        "Subtype" => Object::name("Highlight"),
        "Rect" => vec![Object::from(10), Object::from(10), Object::from(50), Object::from(20)],
        "QuadPoints" => vec![
            Object::from(10), Object::from(20), Object::from(50), Object::from(20),
            Object::from(10), Object::from(10), Object::from(50), Object::from(10),
        ],
    });
    let reply_id = store.put(dict! {
        "Subtype" => Object::name("Text"),
        "Rect" => vec![Object::from(10), Object::from(10), Object::from(30), Object::from(30)],
        "IRT" => root_id,
        "Contents" => Object::text("Looks good"),
    });
    let state_id = store.put(dict! {
        "Subtype" => Object::name("Text"),
        "Rect" => vec![Object::from(10), Object::from(10), Object::from(30), Object::from(30)],
        "IRT" => root_id,
        "StateModel" => Object::text("Review"),
        "State" => Object::text("Completed"),
    });
    let group_id = store.put(dict! {
        "Subtype" => Object::name("Line"),
        "Rect" => vec![Object::from(0), Object::from(0), Object::from(10), Object::from(10)],
        "L" => vec![Object::from(0), Object::from(0), Object::from(10), Object::from(10)],
        "IRT" => root_id,
        "RT" => Object::name("Group"),
    });
    let popup_id = store.put(dict! {
        "Subtype" => Object::name("Popup"),
        "Rect" => vec![Object::from(60), Object::from(10), Object::from(160), Object::from(90)],
        "Parent" => root_id,
        "Open" => true,
    });
    let page_dict = dict! {
        "Annots" => vec![
            Object::from(root_id),
            Object::from(reply_id),
            Object::from(state_id),
            Object::from(group_id),
            Object::from(popup_id),
        ],
    };
    let page_list = page_annots(
        &store.objects,
        &page_dict,
        None,
        Rules::Pdfium,
        &Limits::DEFAULT,
    )
    .unwrap();
    let threads = resolve_threads(&page_list, &Limits::DEFAULT, &mut ());
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].root_index, 0);
    assert_eq!(threads[0].replies, vec![1]);
    assert_eq!(threads[0].group_members, vec![3]);
    assert_eq!(threads[0].review, ReviewState::Completed);

    // Z-order QuadPoints uncrosses cleanly into a non-self-intersecting Quad,
    // and QuadPoints::from_quad / from_rect round-trip both axis-aligned and rotated quads.
    let zq = QuadPoints([10.0, 40.0, 50.0, 40.0, 10.0, 20.0, 50.0, 20.0]);
    assert_eq!(zq.bounds(), Rect::new(10.0, 20.0, 50.0, 40.0));
    let uq = zq.to_quad();
    assert_eq!(
        uq.0,
        [
            Point::new(10.0, 20.0),
            Point::new(50.0, 20.0),
            Point::new(50.0, 40.0),
            Point::new(10.0, 40.0),
        ]
    );
    assert_eq!(QuadPoints::from(uq), zq);
    assert_eq!(QuadPoints::from(Rect::new(10.0, 20.0, 50.0, 40.0)), zq);

    let rotated = pdf_gfx::Quad([
        Point::new(10.0, 10.0),
        Point::new(30.0, 20.0),
        Point::new(25.0, 30.0),
        Point::new(5.0, 20.0),
    ]);
    assert_eq!(QuadPoints::from_quad(rotated).to_quad(), rotated);

    // AnnotColor <-> pdf_content_streams::Color round-trip.
    for c in [
        pdf_content_streams::Color::Gray(0.4),
        pdf_content_streams::Color::rgb8([20, 40, 80]),
        pdf_content_streams::Color::Cmyk([0.1, 0.2, 0.3, 0.4]),
    ] {
        assert_eq!(AnnotColor::from(c).to_content_color(), Some(c));
    }
}

#[test]
fn builders_appearance_synthesis_and_page_flattening_roundtrip() {
    use pdf_objects::WithStreams;

    let mut store = Store::default();
    let mut page_dict = dict! {
        "Type" => Object::name("Page"),
        "Resources" => dict! {
            "XObject" => dict! {
                "Fm0" => ObjectId::new(999, 0),
            },
        },
    };

    let border = Border::default();
    let da = DefaultAppearance::new(b"Helv".to_vec(), 11.0, Some(Color::Rgb([0.1, 0.2, 0.8])));

    let built_items = [
        BuiltAnnot::link_dest(
            Rect::new(10.0, 10.0, 60.0, 25.0),
            &Dest::fit(ObjectId::new(3, 0)),
        ),
        BuiltAnnot::link_uri(
            Rect::new(10.0, 30.0, 60.0, 45.0),
            b"https://example.org".to_vec(),
        ),
        BuiltAnnot::text_markup(
            TextMarkupKind::Highlight,
            &[QuadPoints([20.0, 70.0, 80.0, 70.0, 20.0, 56.0, 80.0, 56.0])],
            AnnotColor::Rgb([1.0, 0.95, 0.2]),
            0.6,
        ),
        BuiltAnnot::text_markup(
            TextMarkupKind::Squiggly,
            &[QuadPoints([20.0, 54.0, 80.0, 54.0, 20.0, 42.0, 80.0, 42.0])],
            AnnotColor::Rgb([0.9, 0.1, 0.1]),
            1.0,
        ),
        BuiltAnnot::shape(
            ShapeKind::Circle,
            Rect::new(100.0, 100.0, 160.0, 140.0),
            &PaintStyle::new(
                Some(AnnotColor::Rgb([0.0, 0.5, 0.0])),
                Some(AnnotColor::Rgb([0.8, 1.0, 0.8])),
            )
            .with_opacity(0.9),
        ),
        BuiltAnnot::line(
            Point::new(50.0, 200.0),
            Point::new(120.0, 230.0),
            (LineEnding::Circle, LineEnding::OpenArrow),
            &PaintStyle::new(
                Some(AnnotColor::Rgb([0.1, 0.1, 0.1])),
                Some(AnnotColor::Rgb([1.0, 0.0, 0.0])),
            ),
        ),
        BuiltAnnot::poly(
            PolyKind::Polygon,
            &[
                Point::new(200.0, 200.0),
                Point::new(240.0, 200.0),
                Point::new(220.0, 240.0),
            ],
            (LineEnding::None, LineEnding::None),
            &PaintStyle::new(Some(AnnotColor::Gray(0.2)), Some(AnnotColor::Gray(0.8))),
        ),
        BuiltAnnot::ink(
            &[vec![
                Point::new(300.0, 300.0),
                Point::new(310.0, 315.0),
                Point::new(325.0, 305.0),
            ]],
            AnnotColor::Rgb([0.0, 0.0, 0.8]),
            &border,
            1.0,
        ),
        BuiltAnnot::stamp(
            Rect::new(72.0, 500.0, 220.0, 548.0),
            StampName::Approved,
            None,
            AnnotColor::Rgb([0.1, 0.6, 0.2]),
            1.0,
        ),
        BuiltAnnot::text_note(
            Rect::new(300.0, 500.0, 324.0, 524.0),
            "Review note",
            TextIcon::Note,
            AnnotColor::Rgb([1.0, 0.9, 0.3]),
        ),
        BuiltAnnot::free_text(
            Rect::new(72.0, 600.0, 240.0, 650.0),
            "Hello PDF Annotations",
            &da,
            Quadding::Center,
            &PaintStyle::new(
                Some(AnnotColor::Gray(0.3)),
                Some(AnnotColor::Rgb([1.0, 1.0, 0.95])),
            ),
        ),
        BuiltAnnot::redact(
            Rect::new(72.0, 680.0, 200.0, 700.0),
            &[],
            AnnotColor::Gray(0.0),
            Some("REDACTED"),
            false,
            Some(&da),
            Quadding::Center,
        ),
    ];

    for b in built_items {
        let aid = b.write_into(|obj| store.put(obj));
        assert!(push_page_annot(&mut page_dict, aid));
    }

    // Add a screen-only annotation (`F = 0`, no PRINT flag) with `/AP /N` and a `/Widget`.
    let screen_only_ap = store.put_stream(
        dict! {
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(10), Object::from(10)],
        },
        b"0 g 0 0 10 10 re f".to_vec(),
    );
    let screen_only_id = store.put(dict! {
        "Subtype" => Object::name("Square"),
        "Rect" => vec![Object::from(400), Object::from(400), Object::from(410), Object::from(410)],
        "F" => 0,
        "AP" => dict! { "N" => screen_only_ap },
    });
    assert!(push_page_annot(&mut page_dict, screen_only_id));

    let widget_ap = store.put_stream(
        dict! {
            "Subtype" => Object::name("Form"),
            "BBox" => vec![Object::from(0), Object::from(0), Object::from(40), Object::from(16)],
        },
        b"0.5 g 0 0 40 16 re f".to_vec(),
    );
    let widget_id = store.put(dict! {
        "Subtype" => Object::name("Widget"),
        "Rect" => vec![Object::from(400), Object::from(450), Object::from(440), Object::from(466)],
        "F" => AnnotFlags::PRINT.bits() as i64,
        "AP" => dict! { "N" => widget_ap },
    });
    assert!(push_page_annot(&mut page_dict, widget_id));

    let mut read_faults = Vec::new();
    let parsed = page_annots_with(
        &store.objects,
        &page_dict,
        None,
        Rules::Iso,
        &Limits::DEFAULT,
        &mut read_faults,
    )
    .unwrap();
    assert_eq!(read_faults, Vec::new());
    assert_eq!(parsed.len(), 14);
    assert!(synth_annot(&parsed[2]).is_some());

    // Flatten for Display excluding widgets: `Fm0` is already in `/Resources/XObject`, so none of
    // the new XObject bindings may collide with `Fm0`!
    let disp_opts = FlattenOpts::DEFAULT.with_widgets(false);
    let disp_plan = plan_flatten(
        &store.objects,
        &page_dict,
        None,
        page_dict.get(b"Resources"),
        disp_opts,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert!(disp_plan.changed());
    // 10 visual markups + 1 screen-only square = 11 drawn; 2 links have no `/AP` so removed without drawing; 1 widget kept!
    assert_eq!(disp_plan.drawn_count, 11);
    assert_eq!(disp_plan.removed_count, 13);
    assert_eq!(disp_plan.kept_annots, vec![Object::from(widget_id)]);
    assert!(
        disp_plan
            .xobject_bindings
            .iter()
            .all(|(name, _)| name.as_slice() != b"Fm0")
    );

    // Flatten for Print (including widgets): `screen_only_id` is NOT drawn in `Print` mode!
    let print_opts = FlattenOpts::for_target(FlattenTarget::Print);
    let print_plan = plan_flatten(
        &store.objects,
        &page_dict,
        None,
        page_dict.get(b"Resources"),
        print_opts,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(print_plan.drawn_count, 11); // 10 printable markups + 1 printable widget; screen_only skipped!
    assert!(print_plan.kept_annots.is_empty());

    // `plan_flatten_filtered`: keep everything except the `/Stamp`.
    let filtered = plan_flatten_filtered(
        &store.objects,
        &page_dict,
        None,
        page_dict.get(b"Resources"),
        FlattenOpts::default(),
        &Limits::DEFAULT,
        |a| *a.subtype() == AnnotSubtype::Stamp,
    )
    .unwrap();
    assert_eq!(filtered.drawn_count, 1);
    assert_eq!(filtered.kept_annots.len(), 13);

    // Verify `combine_with_page` + `pdf_gfx::interpret` executes every synthesized `/AP /N` stream
    // cleanly without error even when the original page stream leaves an unclosed `10 0 0 10 0 0 cm`!
    let mut xo_dict = Dict::new();
    for (name, id) in &disp_plan.xobject_bindings {
        xo_dict.set(name.clone(), *id);
    }
    let res_obj = Object::from(dict! { "XObject" => xo_dict });
    let combined = disp_plan.combine_with_page(b"10 0 0 10 0 0 cm");

    let streams = WithStreams::new(&store.objects, |id: ObjectId, _limit: usize| {
        let s = store
            .objects
            .get(&id)
            .and_then(Object::as_stream)
            .ok_or_else(|| pdf_objects::Unreadable::new(id, Why::Damaged, "missing"))?;
        match &s.data {
            pdf_objects::Data::Bytes(bytes) => Ok(bytes.clone()),
            _ => Err(pdf_objects::Unreadable::new(id, Why::Damaged, "unplaced")),
        }
    });
    let slices = [&combined[..]];
    let concat = pdf_content_streams::Concat::new(&slices);
    let mut events = 0usize;
    pdf_gfx::interpret(
        streams,
        &concat,
        Some(&res_obj),
        &pdf_gfx::Limits::DEFAULT,
        &pdf_fonts::Fonts::default(),
        pdf_gfx::Opts::default(),
        |_| {
            events += 1;
            std::ops::ControlFlow::Continue(())
        },
    )
    .unwrap();
    assert!(
        events > 10,
        "expected rendered path/text events, got {events}"
    );
}

#[test]
fn default_appearance_is_pdf_gfxs_type() {
    use std::any::TypeId;

    assert_eq!(
        TypeId::of::<crate::DefaultAppearance>(),
        TypeId::of::<pdf_gfx::DefaultAppearance>()
    );
    // One value passes for both names, with no conversion.
    let da: pdf_gfx::DefaultAppearance =
        crate::DefaultAppearance::new("Helv", 12.0, AnnotColor::Gray(0.0).to_content_color());
    let same: &crate::DefaultAppearance = &da;
    assert_eq!(same.to_bytes(), b"/Helv 12 Tf 0 g");
    // A transparent annotation color is no `/DA` color.
    assert_eq!(AnnotColor::Transparent.to_content_color(), None);
}

const BORDER_STYLES: [BorderStyle; 5] = [
    BorderStyle::Solid,
    BorderStyle::Dashed,
    BorderStyle::Beveled,
    BorderStyle::Inset,
    BorderStyle::Underline,
];

/// A red border of `width` in `style` over a light gray interior.
fn bordered(width: f32, style: BorderStyle) -> PaintStyle {
    PaintStyle::new(
        Some(AnnotColor::Rgb([1.0, 0.0, 0.0])),
        Some(AnnotColor::Gray(0.8)),
    )
    .with_border(Border {
        width,
        style,
        ..Border::default()
    })
}

fn content_text(ap: &SynthesizedAp) -> String {
    String::from_utf8(ap.content.clone()).unwrap()
}

/// What `pdf_gfx::Canvas::box_border` writes for `style`'s border around
/// `rect`.
fn border_text(rect: Rect, style: &PaintStyle) -> String {
    let mut canvas = pdf_gfx::Canvas::new();
    canvas.box_border(
        rect,
        &style.border.to_box_border(),
        style.stroke.and_then(AnnotColor::to_content_color).unwrap(),
        style.fill.and_then(AnnotColor::to_content_color),
    );
    String::from_utf8(canvas.finish(&pdf_content_streams::Reserved::default()).0).unwrap()
}

/// Interprets a synthesized appearance over its own resources, holding that
/// it reads without a fault, that its `q` and `Q` balance, and that every
/// painted path lies inside its `/BBox` (a stroke's half width added). Returns
/// the painted paths' count and the glyphs'.
fn read_back(ap: &SynthesizedAp) -> (usize, usize) {
    use std::ops::ControlFlow;

    use pdf_content_streams::Paint;
    use pdf_gfx::{Kind, Seg};

    let words = || ap.content.split(u8::is_ascii_whitespace);
    assert_eq!(
        words().filter(|w| w == b"q").count(),
        words().filter(|w| w == b"Q").count(),
        "q and Q balance"
    );
    let objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let streams = pdf_objects::WithStreams::new(&objects, |id: ObjectId, _limit: usize| {
        Err(pdf_objects::Unreadable::new(id, Why::Damaged, "missing"))
    });
    let resources = Object::from(ap.resources.clone());
    let slices = [&ap.content[..]];
    let mut faults = Vec::new();
    let mut painted = 0usize;
    let summary = pdf_gfx::interpret_with(
        streams,
        &pdf_content_streams::Concat::new(&slices),
        Some(&resources),
        &pdf_gfx::Limits::DEFAULT,
        &pdf_fonts::Fonts::default(),
        pdf_gfx::Opts::default(),
        &mut faults,
        |ev| {
            if let Kind::Path(path) = &ev.kind
                && !matches!(path.paint, Paint::None)
            {
                painted += 1;
                let half = if matches!(path.paint, Paint::Stroke) {
                    ev.state.line_width / 2.0
                } else {
                    0.0
                };
                // A single level segment is an underline: butt caps, so its
                // width reaches up and down only.
                let side = if matches!(path.segments, [Seg::Move(a), Seg::Line(b)] if a.y == b.y) {
                    0.0
                } else {
                    half
                };
                for p in path.segments.iter().flat_map(Seg::points) {
                    assert!(
                        p.x - side >= ap.bbox.x0 - 1e-3
                            && p.x + side <= ap.bbox.x1 + 1e-3
                            && p.y - half >= ap.bbox.y0 - 1e-3
                            && p.y + half <= ap.bbox.y1 + 1e-3,
                        "{p:?} with a half width of {half} is outside {:?}",
                        ap.bbox
                    );
                }
            }
            ControlFlow::Continue(())
        },
    )
    .unwrap();
    assert!(faults.is_empty(), "no fault (an extra Q among them)");
    (painted, summary.glyphs)
}

#[test]
fn free_text_and_square_borders_are_drawn_in_each_of_the_five_styles() {
    use pdf_gfx::{BoxBorder, BoxBorderStyle};

    // A border style is its drawing style, and a border carries its width
    // and dash array to the canvas.
    for (read, drawn) in [
        (BorderStyle::Solid, BoxBorderStyle::Solid),
        (BorderStyle::Dashed, BoxBorderStyle::Dashed),
        (BorderStyle::Beveled, BoxBorderStyle::Beveled),
        (BorderStyle::Inset, BoxBorderStyle::Inset),
        (BorderStyle::Underline, BoxBorderStyle::Underline),
    ] {
        assert_eq!(BoxBorderStyle::from(read), drawn);
        let border = Border {
            width: 1.5,
            style: read,
            dash: vec![4.0, 2.0],
            ..Border::default()
        };
        assert_eq!(
            border.to_box_border(),
            BoxBorder::new(1.5, drawn).with_dash(&[4.0, 2.0])
        );
    }

    let rect = Rect::new(100.0, 200.0, 200.0, 250.0);
    let bbox = Rect::new(0.0, 0.0, 100.0, 50.0);
    for style in BORDER_STYLES {
        let paint = bordered(2.0, style);
        // Painted paths of the border alone: its frame or line, and a
        // bevel's two bands.
        let border_paths = match style {
            BorderStyle::Beveled | BorderStyle::Inset => 3,
            _ => 1,
        };

        // FreeText: the background over the whole box, then the canvas's
        // border around it (the background its bevel's hint), then the text.
        let ft = crate::synth_free_text(rect, "Hi", None, Quadding::Left, &paint).unwrap();
        let text = content_text(&ft);
        let border = border_text(bbox, &paint);
        assert!(!border.is_empty());
        assert!(
            text.starts_with(&format!("q\n0.8 g\n0 0 100 50 re\nf\n{border}")),
            "{style:?}: {text}"
        );
        assert_eq!(read_back(&ft), (1 + border_paths, 2), "{style:?}");

        // Square: the interior over the box less `/RD`, then the border
        // inside that.
        let sq = crate::synth_shape(ShapeKind::Square, rect, &paint, Some([1.0, 2.0, 3.0, 4.0]))
            .unwrap();
        let inner = Rect::new(1.0, 2.0, 97.0, 46.0);
        assert_eq!(
            content_text(&sq),
            format!(
                "q\n0.8 g\n1 2 96 44 re\nf\n{}Q\n",
                border_text(inner, &paint)
            ),
            "{style:?}"
        );
        assert_eq!(read_back(&sq), (1 + border_paths, 0), "{style:?}");

        // With no interior color the border stands alone, a bevel's shade
        // the hintless gray.
        let mut hollow = paint.clone();
        hollow.fill = None;
        let sq = crate::synth_shape(ShapeKind::Square, rect, &hollow, None).unwrap();
        assert_eq!(
            content_text(&sq),
            format!("q\n{}Q\n", border_text(bbox, &hollow)),
            "{style:?}"
        );
        // With no border color, or no width, the interior alone.
        let mut plain = paint.clone();
        plain.stroke = None;
        let sq = crate::synth_shape(ShapeKind::Square, rect, &plain, None).unwrap();
        assert_eq!(content_text(&sq), "q\n0.8 g\n0 0 100 50 re\nf\nQ\n");
        let sq = crate::synth_shape(ShapeKind::Square, rect, &bordered(0.0, style), None).unwrap();
        assert_eq!(content_text(&sq), "q\n0.8 g\n0 0 100 50 re\nf\nQ\n");
        // With neither, nothing.
        let mut neither = hollow.clone();
        neither.stroke = None;
        let sq = crate::synth_shape(ShapeKind::Square, rect, &neither, None).unwrap();
        assert_eq!(content_text(&sq), "");
    }

    // The styles' own operators, once each, on a square of width 1.
    let square = |style| {
        content_text(
            &crate::synth_shape(ShapeKind::Square, rect, &bordered(1.0, style), None).unwrap(),
        )
    };
    let fill = "q\n0.8 g\n0 0 100 50 re\nf\n";
    let frame = "q\n1 0 0 RG\n1 w\n0.5 0.5 99 49 re\nS\n";
    assert_eq!(square(BorderStyle::Solid), format!("{fill}{frame}Q\nQ\n"));
    assert_eq!(
        square(BorderStyle::Dashed),
        format!("{fill}q\n1 0 0 RG\n1 w\n[3] 0 d\n0.5 0.5 99 49 re\nS\nQ\nQ\n")
    );
    assert_eq!(
        square(BorderStyle::Underline),
        format!("{fill}q\n1 0 0 RG\n1 w\n0 0.5 m\n100 0.5 l\nS\nQ\nQ\n")
    );
    let bands = |light: &str, dark: &str| {
        format!(
            "{fill}{frame}{light}\n1 1 m\n1 49 l\n99 49 l\n98 48 l\n2 48 l\n2 2 l\nh\nf\n\
             {dark}\n1 1 m\n99 1 l\n99 49 l\n98 48 l\n98 2 l\n2 2 l\nh\nf\nQ\nQ\n"
        )
    };
    // Beveled: the interior color (0.8 gray) at half its brightness below
    // and to the right.
    assert_eq!(square(BorderStyle::Beveled), bands("1 g", "0.4 g"));
    assert_eq!(square(BorderStyle::Inset), bands("0.5 g", "0.75 g"));

    // A dashed border's own array is drawn as given.
    let mut dashed = bordered(1.0, BorderStyle::Dashed);
    dashed.border.dash = vec![4.0, 2.0];
    let sq = crate::synth_shape(ShapeKind::Square, rect, &dashed, None).unwrap();
    assert!(content_text(&sq).contains("1 w\n[4 2] 0 d\n0.5 0.5 99 49 re\nS\n"));
    let ft = crate::synth_free_text(rect, "Hi", None, Quadding::Left, &dashed).unwrap();
    assert!(content_text(&ft).contains("1 w\n[4 2] 0 d\n0.5 0.5 99 49 re\nS\n"));

    // Under an opacity the fill and the border share one graphics state.
    let sq = crate::synth_shape(
        ShapeKind::Square,
        rect,
        &bordered(1.0, BorderStyle::Solid).with_opacity(0.5),
        None,
    )
    .unwrap();
    let text = content_text(&sq);
    assert!(
        text.starts_with("q\n/") && text.contains(" gs\n0.8 g\n0 0 100 50 re\nf\nq\n1 0 0 RG\n")
    );
    assert_eq!(read_back(&sq), (2, 0));
}

#[test]
fn free_text_text_clears_its_border_and_stays_put_for_a_solid_one() {
    let rect = Rect::new(100.0, 200.0, 200.0, 250.0);
    let free_text = |style: &PaintStyle| {
        content_text(&crate::synth_free_text(rect, "Hi", None, Quadding::Left, style).unwrap())
    };
    // The box the text is laid out and clipped in.
    let text_box = |style: &PaintStyle| {
        let text = free_text(style);
        let clip = text.find(" re\nW\nn\n").expect("the text's clip");
        let start = text[..clip].rfind('\n').unwrap() + 1;
        text[start..clip].to_owned()
    };

    // A solid, dashed or underline border: its width and 2 more, as before
    // the other styles were drawn; a width of 0 or below: 2.
    for style in [
        BorderStyle::Solid,
        BorderStyle::Dashed,
        BorderStyle::Underline,
    ] {
        assert_eq!(text_box(&bordered(1.0, style)), "3 3 94 44", "{style:?}");
        assert_eq!(
            text_box(&bordered(3.5, style)),
            "5.5 5.5 89 39",
            "{style:?}"
        );
    }
    for style in BORDER_STYLES {
        for width in [0.0, -1.0, f32::NAN] {
            assert_eq!(text_box(&bordered(width, style)), "2 2 96 46", "{style:?}");
            assert!(!free_text(&bordered(width, style)).contains("RG"));
        }
    }
    // A bevel takes twice its width (the frame, then the band), and the text
    // clears both.
    for style in [BorderStyle::Beveled, BorderStyle::Inset] {
        assert_eq!(text_box(&bordered(1.0, style)), "4 4 92 42", "{style:?}");
        assert_eq!(text_box(&bordered(3.0, style)), "8 8 84 34", "{style:?}");
    }
    // The padding counts the border whether or not a color draws it.
    let mut uncolored = bordered(3.0, BorderStyle::Solid);
    uncolored.stroke = None;
    assert_eq!(text_box(&uncolored), "5 5 90 40");
    assert!(!free_text(&uncolored).contains("RG"));

    // A solid border's whole stream, at widths 1, 0 and -1: what it was when
    // the border was `border_rect`'s under a color set outside its `q`, but
    // for where that color stands.
    #[cfg(feature = "text-layout")]
    let (text_1, text_0) = (
        "0 g\nq\n3 3 94 44 re\nW\nn\nBT\n/ShWMNLLO 12 Tf\n1 0 0 1 3 38.384 Tm\n(Hi) Tj\nET\nQ\nQ\n",
        "0 g\nq\n2 2 96 46 re\nW\nn\nBT\n/ShWMNLLO 12 Tf\n1 0 0 1 2 39.384 Tm\n(Hi) Tj\nET\nQ\nQ\n",
    );
    #[cfg(not(feature = "text-layout"))]
    let (text_1, text_0) = (
        "q\n3 3 94 44 re\nW\nn\n0 g\nBT\n/ShWMNLLO 12 Tf\n1 0 0 1 3 35 Tm\n(Hi) Tj\nET\nQ\nQ\n",
        "q\n2 2 96 46 re\nW\nn\n0 g\nBT\n/ShWMNLLO 12 Tf\n1 0 0 1 2 36 Tm\n(Hi) Tj\nET\nQ\nQ\n",
    );
    let background = "q\n0.8 g\n0 0 100 50 re\nf\n";
    let now = free_text(&bordered(1.0, BorderStyle::Solid));
    assert_eq!(
        now,
        format!("{background}q\n1 0 0 RG\n1 w\n0.5 0.5 99 49 re\nS\nQ\n{text_1}")
    );
    assert_eq!(
        now.replacen("q\n1 0 0 RG\n1 w\n", "1 0 0 RG\nq\n1 w\n", 1),
        format!("{background}1 0 0 RG\nq\n1 w\n0.5 0.5 99 49 re\nS\nQ\n{text_1}")
    );
    for width in [0.0, -1.0] {
        assert_eq!(
            free_text(&bordered(width, BorderStyle::Solid)),
            format!("{background}{text_0}"),
            "{width}"
        );
    }
}

#[test]
fn rounded_and_cloudy_squares_and_circles_keep_their_one_stroked_path() {
    let rect = Rect::new(100.0, 200.0, 200.0, 250.0);
    let shape = |kind, style: &PaintStyle| {
        let ap = crate::synth_shape(kind, rect, style, None).unwrap();
        assert_eq!(read_back(&ap), (1, 0));
        content_text(&ap)
    };
    let rounded = |style| {
        let mut paint = bordered(2.0, style);
        paint.border.horizontal_radius = 5.0;
        paint.border.vertical_radius = 5.0;
        paint
    };
    let cloudy = |style| {
        let mut paint = bordered(2.0, style);
        paint.border.cloudy_intensity = Some(1.0);
        paint
    };

    // One path along the border's center line, filled and stroked.
    let circle = shape(ShapeKind::Circle, &bordered(2.0, BorderStyle::Solid));
    assert_eq!(
        circle,
        "q\n1 0 0 RG\n2 w\n0.8 g\n99 25 m\n99 38.2548 77.062 49 50 49 c\n\
         22.938 49 1 38.2548 1 25 c\n1 11.7452 22.938 1 50 1 c\n\
         77.062 1 99 11.7452 99 25 c\nh\nB\nQ\n"
    );
    let round = shape(ShapeKind::Square, &rounded(BorderStyle::Solid));
    assert!(
        round.starts_with("q\n1 0 0 RG\n2 w\n0.8 g\n6 1 m\n94 1 l\n96.7614 1 99 3.2386 99 6 c\n"),
        "{round}"
    );
    assert!(round.ends_with("h\nB\nQ\n"));
    let cloud = shape(ShapeKind::Square, &cloudy(BorderStyle::Solid));
    assert_eq!(cloud, "q\n1 0 0 RG\n2 w\n0.8 g\n1 1 98 48 re\nB\nQ\n");

    // The bevels and the underline are a plain rectangle's: here they come
    // out solid.
    for style in [
        BorderStyle::Beveled,
        BorderStyle::Inset,
        BorderStyle::Underline,
    ] {
        assert_eq!(
            shape(ShapeKind::Circle, &bordered(2.0, style)),
            circle,
            "{style:?}"
        );
        assert_eq!(
            shape(ShapeKind::Square, &rounded(style)),
            round,
            "{style:?}"
        );
        assert_eq!(shape(ShapeKind::Square, &cloudy(style)), cloud, "{style:?}");
    }
    // A dashed one is the same path under its dash array.
    let dash = "q\n1 0 0 RG\n2 w\n[3] 0 d\n0.8 g\n";
    let solid = "q\n1 0 0 RG\n2 w\n0.8 g\n";
    assert_eq!(
        shape(ShapeKind::Circle, &bordered(2.0, BorderStyle::Dashed)),
        circle.replacen(solid, dash, 1)
    );
    assert_eq!(
        shape(ShapeKind::Square, &rounded(BorderStyle::Dashed)),
        round.replacen(solid, dash, 1)
    );
    assert_eq!(
        shape(ShapeKind::Square, &cloudy(BorderStyle::Dashed)),
        cloud.replacen(solid, dash, 1)
    );
}

#[test]
fn a_dashed_border_built_with_no_array_is_dashed_by_threes() {
    let rect = Rect::new(100.0, 200.0, 200.0, 250.0);
    let border = |dash: &[f32]| Border {
        width: 2.0,
        style: BorderStyle::Dashed,
        dash: dash.to_vec(),
        ..Border::default()
    };
    let black =
        |dash: &[f32]| PaintStyle::new(Some(AnnotColor::Gray(0.0)), None).with_border(border(dash));
    let pts = [
        Point::new(110.0, 210.0),
        Point::new(190.0, 240.0),
        Point::new(150.0, 245.0),
    ];
    let line = |style: &PaintStyle| {
        content_text(
            &crate::synth_line(
                rect,
                pts[0],
                pts[1],
                (LineEnding::None, LineEnding::None),
                style,
            )
            .unwrap(),
        )
    };

    // A line: Table 168's default array where the border has none (a border
    // read from a file already has it; one built by hand need not).
    assert_eq!(
        line(&black(&[])),
        "q\n0 G\n0 g\n2 w\n[3] 0 d\n2 w\n10 10 m\n90 40 l\nS\n[] 0 d\nQ\n"
    );
    assert_eq!(line(&black(&[])), line(&black(&[3.0])));
    assert!(line(&black(&[5.0, 1.0])).contains("2 w\n[5 1] 0 d\n2 w\n"));
    // A solid border sets no dash for its line.
    let solid = PaintStyle::new(Some(AnnotColor::Gray(0.0)), None);
    assert!(!line(&solid).contains("[3]"));

    // And everywhere else a border's path is stroked.
    for kind in [PolyKind::Polygon, PolyKind::PolyLine] {
        let poly = |style: &PaintStyle| {
            content_text(
                &crate::synth_poly(
                    kind,
                    rect,
                    &pts,
                    (LineEnding::None, LineEnding::None),
                    style,
                )
                .unwrap(),
            )
        };
        assert!(poly(&black(&[])).contains("[3] 0 d\n"), "{kind:?}");
        assert_eq!(poly(&black(&[])), poly(&black(&[3.0])), "{kind:?}");
    }
    let ink = |border: &Border| {
        content_text(
            &crate::synth_ink(rect, &[pts.to_vec()], AnnotColor::Gray(0.0), border, 1.0).unwrap(),
        )
    };
    assert!(ink(&border(&[])).contains("[3] 0 d\n"));
    assert_eq!(ink(&border(&[])), ink(&border(&[3.0])));
    let circle = |style: &PaintStyle| {
        content_text(&crate::synth_shape(ShapeKind::Circle, rect, style, None).unwrap())
    };
    assert!(circle(&black(&[])).contains("2 w\n[3] 0 d\n"));
    assert_eq!(circle(&black(&[])), circle(&black(&[3.0])));
}
