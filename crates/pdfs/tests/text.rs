//! Pages' fonts and text through the facade, over files written by pdf-fixtures:
//! the strings each page shows in its fonts, forms followed, each font read once a
//! file, a page written in place, what a call's caps and budget leave unread, and a
//! locked file.

use std::sync::Arc;

use pdf_fixtures::fixture::{self, Body, Xref};
use pdfs::pdf_content_streams::Pushed;
use pdfs::pdf_crypt::Password;
use pdfs::pdf_crypt::writer::{Kind, Settings};
use pdfs::pdf_fonts::{Error as FontError, Unread};
use pdfs::pdf_objects::{Dict, Object, ObjectId, Why, dict};
use pdfs::{Error, Limits, Options};

fn r(n: u32) -> Object {
    Object::reference(n)
}

fn content_stream(data: &str) -> Body {
    Body::stream(dict! {}, data.as_bytes())
}

/// A catalog, 1, a root, 2, over `pages`, each a page of the root's from 3, and
/// `more` beside them.
fn file_of(pages: &[Dict], more: &[(u32, Body)]) -> Vec<u8> {
    let kids: Vec<Object> = (0..pages.len() as u32).map(|i| r(3 + i)).collect();
    let mut objects = vec![
        (
            1,
            Body::from(dict! { "Type" => "Catalog", "Pages" => r(2) }),
        ),
        (
            2,
            Body::from(dict! { "Type" => "Pages", "Kids" => kids, "Count" => pages.len() as i64 }),
        ),
    ];
    for (i, page) in pages.iter().enumerate() {
        let mut page = page.clone();
        page.set("Type", "Page");
        page.set("Parent", r(2));
        objects.push((3 + i as u32, Body::from(page)));
    }
    objects.extend(more.iter().cloned());
    fixture::file("1.7", &objects, dict! { "Root" => r(1) }, Xref::Table)
}

/// Courier, as a font dictionary of its own: its `/ToUnicode`, where it's given,
/// object `to_unicode`.
fn courier(to_unicode: Option<u32>) -> Body {
    let mut font = dict! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier",
        "Encoding" => "WinAnsiEncoding",
    };
    if let Some(to_unicode) = to_unicode {
        font.set("ToUnicode", r(to_unicode));
    }
    Body::from(font)
}

/// A `/ToUnicode` reading `A` as `Z`.
const A_IS_Z: &str =
    "1 begincodespacerange <00> <FF> endcodespacerange 1 beginbfchar <41> <005A> endbfchar";

/// Two pages over the same fonts: F1, Courier with a `/ToUnicode` (20, 21), F2
/// Courier (22); the first page draws form 30, which draws form 31, each
/// showing a string.
fn two_pages() -> Vec<u8> {
    let resources = dict! {
        "Font" => dict! { "F1" => r(20), "F2" => r(22) },
        "XObject" => dict! { "X" => r(30) },
    };
    let pages = [
        dict! { "Contents" => r(10), "Resources" => resources.clone() },
        dict! { "Contents" => r(11), "Resources" => resources },
    ];
    let form = |content: &str, resources: Option<Dict>| {
        let mut dict = dict! { "Type" => "XObject", "Subtype" => "Form" };
        if let Some(resources) = resources {
            dict.set("Resources", resources);
        }
        Body::stream(dict, content.as_bytes())
    };
    let more = [
        (
            10,
            content_stream("BT /F1 12 Tf (AB) Tj /F2 12 Tf [(C) -20 (D)] TJ ET /X Do"),
        ),
        (11, content_stream("BT /F1 12 Tf (A) Tj ET")),
        (20, courier(Some(21))),
        (21, content_stream(A_IS_Z)),
        (22, courier(None)),
        (
            30,
            form(
                "(E) Tj /Y Do",
                Some(dict! { "XObject" => dict! { "Y" => r(31) } }),
            ),
        ),
        (31, form("/F1 9 Tf (A) Tj", None)),
    ];
    file_of(&pages, &more)
}

/// Each page's strings in order, in their fonts, the forms followed two deep;
/// each font read once a file, so both pages show in the same; and the fonts
/// their resources name, in order.
#[test]
fn a_pages_text_is_the_strings_it_shows() {
    let pdf = pdfs::from_bytes(two_pages()).unwrap();
    let pages = pdf.pages().unwrap();
    let first = pages.get(0).unwrap().text().unwrap();
    assert!(first.is_whole(), "{first:?}");
    assert_eq!(first.text(), "ZBCDEZ");
    let forms: Vec<_> = first.strings.iter().map(|s| s.form).collect();
    let (outer, inner) = (Some(ObjectId::new(30, 0)), Some(ObjectId::new(31, 0)));
    assert_eq!(forms, [None, None, None, outer, inner]);
    let second = pages.get(1).unwrap().text().unwrap();
    assert_eq!(second.text(), "Z");
    assert!(Arc::ptr_eq(&first.strings[0].font, &second.strings[0].font));

    let fonts = pages.get(0).unwrap().fonts().unwrap();
    let named: Vec<_> = (fonts.iter())
        .map(|f| {
            (
                f.name.clone(),
                f.font.as_ref().unwrap().as_ref().unwrap().id(),
            )
        })
        .collect();
    let id = |n| Some(ObjectId::new(n, 0));
    assert_eq!(named, [(b"F1".to_vec(), id(20)), (b"F2".to_vec(), id(22))]);
    let kept = fonts[0].font.as_ref().unwrap().as_ref().unwrap();
    assert!(Arc::ptr_eq(kept, &first.strings[0].font));
}

/// A page written in place in its parent's `/Kids` has no object; its text is
/// read all the same.
#[test]
fn a_page_with_no_object_shows_its_text() {
    let page = dict! {
        "Type" => "Page", "Parent" => r(2), "Contents" => r(10),
        "Resources" => dict! { "Font" => dict! { "F1" => r(20) } },
    };
    let objects = [
        (
            1,
            Body::from(dict! { "Type" => "Catalog", "Pages" => r(2) }),
        ),
        (
            2,
            Body::from(
                dict! { "Type" => "Pages", "Kids" => vec![Object::from(page)], "Count" => 1 },
            ),
        ),
        (10, content_stream("BT /F1 9 Tf (A) Tj ET")),
        (20, courier(Some(21))),
        (21, content_stream(A_IS_Z)),
    ];
    let file = fixture::file("1.7", &objects, dict! { "Root" => r(1) }, Xref::Table);
    let pdf = pdfs::from_bytes(file).unwrap();
    let pages = pdf.pages().unwrap();
    let page = pages.get(0).unwrap();
    assert_eq!(page.attributes().id, None);
    assert_eq!(page.text().unwrap().text(), "Z");
    assert_eq!(page.fonts().unwrap().len(), 1);
}

/// The caps and the budget each call is given: a form past `fonts.forms` and
/// a font past the budget are told, and what's shown in them isn't; each call
/// has a budget of its own.
#[test]
fn whats_past_a_cap_or_the_budget_is_told() {
    let mut limits = Limits::DEFAULT;
    limits.fonts.forms = 1;
    let mut options = Options::DEFAULT;
    options.limits = limits;
    let pdf = options.from_bytes(two_pages()).unwrap();
    let text = pdf.pages().unwrap().get(0).unwrap().text().unwrap();
    assert_eq!(text.text(), "ZBCDE");
    assert_eq!(
        text.unread,
        [Unread::Deep {
            form: ObjectId::new(31, 0)
        }]
    );

    // A budget a byte short of the `/ToUnicode`: F1 is told, each page.
    let mut limits = Limits::DEFAULT;
    limits.font_budget = A_IS_Z.len() - 1;
    options.limits = limits;
    let pdf = options.from_bytes(two_pages()).unwrap();
    let pages = pdf.pages().unwrap();
    for (page, text) in [(0, "CDE"), (1, "")] {
        let shown = pages.get(page).unwrap().text().unwrap();
        assert_eq!(shown.text(), text);
        assert!(shown.unread.iter().all(|u| matches!(
            u,
            Unread::Font { name, error: FontError::TooLarge } if name == b"F1"
        )));
        assert!(!shown.is_whole());
    }
    let fonts = pages.get(0).unwrap().fonts().unwrap();
    assert!(matches!(fonts[0].font, Err(FontError::TooLarge)));
    assert!(fonts[1].font.as_ref().unwrap().is_some());
}

/// The settings of a file encrypted under AES-256 by `user`.
fn aes256() -> Settings<'static> {
    Settings {
        kind: Kind::Aes256,
        user: b"user",
        owner: b"owner",
        p: -4,
        id: b"0123456789abcdef",
        encrypt_metadata: true,
    }
}

/// A locked file's text is an error, its content's, never its ciphertext as
/// text; a font whose `/ToUnicode` is locked says so. Opened by its password, it
/// reads.
#[test]
fn a_locked_files_text_is_an_error() {
    let objects = [
        (
            1,
            Body::from(dict! { "Type" => "Catalog", "Pages" => r(2) }),
        ),
        (
            2,
            Body::from(dict! { "Type" => "Pages", "Kids" => vec![r(3)], "Count" => 1 }),
        ),
        (
            3,
            Body::from(dict! {
                "Type" => "Page", "Parent" => r(2), "Contents" => r(10),
                "Resources" => dict! { "Font" => dict! { "F1" => r(20) } },
            }),
        ),
        (10, content_stream("BT /F1 9 Tf (A) Tj ET")),
        (20, courier(Some(21))),
        (21, content_stream(A_IS_Z)),
    ];
    let file = fixture::encrypted(&aes256(), &objects, dict! { "Root" => r(1) });
    let locked = pdfs::from_bytes(&file[..]).unwrap();
    let pages = locked.pages().unwrap();
    let page = pages.get(0).unwrap();
    match page.text().unwrap_err() {
        Error::Content(Pushed::Decode(unreadable)) => assert_eq!(unreadable.why, Why::Locked),
        other => panic!("{other:?}"),
    }
    match &page.fonts().unwrap()[0].font {
        Err(FontError::Unreadable(unreadable)) => assert_eq!(unreadable.why, Why::Locked),
        other => panic!("{other:?}"),
    }

    let given = [Password("user")];
    let mut options = Options::DEFAULT;
    options.passwords = &given;
    let pdf = options.from_bytes(&file[..]).unwrap();
    let text = pdf.pages().unwrap().get(0).unwrap().text().unwrap();
    assert_eq!(text.text(), "Z");
}

/// No `Debug` of a page's fonts or its text holds a name or a string.
#[test]
fn no_debug_holds_a_name_or_a_string() {
    let page = dict! {
        "Contents" => r(10), "Resources" => dict! { "Font" => dict! { "MARKERFONT" => r(20) } },
    };
    let more = [
        (10, content_stream("BT /MARKERFONT 9 Tf (MARKER) Tj ET")),
        (20, courier(None)),
    ];
    let pdf = pdfs::from_bytes(file_of(&[page], &more)).unwrap();
    let pages = pdf.pages().unwrap();
    let page = pages.get(0).unwrap();
    let text = page.text().unwrap();
    assert_eq!(text.text(), "MARKER");
    let fonts = page.fonts().unwrap();
    for printed in [format!("{text:?}"), format!("{fonts:?}")] {
        assert!(!printed.contains("MARKER"), "{printed}");
    }
}
