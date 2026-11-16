//! Unit, invariant, resource-cap, and hygiene tests for `pdf-annots`.

use std::collections::BTreeMap;

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
    assert_eq!(da.color, Some(AnnotColor::Rgb([0.25, 0.5, 0.75])));
    let reparsed = DefaultAppearance::parse(&da.to_bytes(), &Limits::DEFAULT).unwrap();
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

    // Z-order QuadPoints uncrosses cleanly into a non-self-intersecting Quad.
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
    let da = DefaultAppearance {
        font_name: Some(b"Helv".to_vec()),
        font_size: Some(11.0),
        color: Some(AnnotColor::Rgb([0.1, 0.2, 0.8])),
    };

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
