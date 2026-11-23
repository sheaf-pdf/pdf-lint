//! An annotation built in a font of its own (`BuiltAnnot::free_text_with_font`,
//! `redact_with_font`), added to a document: the font put in the form's
//! default resources (`/AcroForm /DR /Font`) under the name its `/DA` gives,
//! or the first free one after it, which `/DA` then names; on a file with no
//! form, with a form whose resources are held directly, by reference, the
//! form itself by reference, and all three by reference; read back by
//! pdf-forms by that name; a font given
//! as a resource of objects written once, its references among them made its
//! own objects'; nothing registered for a `/DA` that names no font, and
//! nothing changed for a document with no catalog. `qpdf --check` finds each
//! saved file clean when run via `just test-qpdf`.

#![cfg(feature = "text-layout")]

use std::collections::HashMap;
use std::path::Path;

use pdf_fixtures::qpdf::Qpdf;
use pdfs::forms::appearance::FieldFont;
use pdfs::pdf_content_streams::ResBody;
use pdfs::pdf_fonts::FontKind;
use pdfs::pdf_objects::version::Version;
use pdfs::pdf_objects::{Resolve as _, dict};
use pdfs::pdf_structure::write::Deflate;
use pdfs::prelude::*;

/// What a `/FreeText` built in an embedded font says: Chinese, which no
/// standard 14 font draws.
#[cfg(feature = "embed")]
const NOTE: &str = "\u{4E2D}\u{6587}";

/// A redaction's overlay text.
#[cfg(feature = "embed")]
const LABEL: &str = "Ag";

/// How the file holds its form before annotations are added to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    /// No form.
    None,
    /// A form in the catalog, its default resources and their fonts in it,
    /// `Helv` among them.
    Direct,
    /// A form in the catalog, its default resources an object of their own,
    /// their fonts in it, `Helv` and `Helv1` among them.
    Resources,
    /// The form an object of its own, its default resources and their fonts
    /// in it, `Helv` among them.
    Form,
    /// The form, its default resources and their fonts each an object of its
    /// own, `Helv` and `Helv2` among them and `Helv1` `null`.
    Objects,
}

impl Held {
    const ALL: [Held; 5] = [
        Held::None,
        Held::Direct,
        Held::Resources,
        Held::Form,
        Held::Objects,
    ];

    /// The name an annotation whose `/DA` names `Helv` is given.
    fn helv(self) -> &'static str {
        match self {
            Held::None => "Helv",
            Held::Direct | Held::Form | Held::Objects => "Helv1",
            Held::Resources => "Helv2",
        }
    }
}

/// A file of one page, its form held as `held` says: the form's one field a
/// text field that is its own widget, on the page, in Helvetica.
fn file(held: Held) -> Vec<u8> {
    let mut doc = Document::new();
    doc.add_page(Rect::LETTER);
    let page = doc.page_ids().unwrap()[0].unwrap();
    if held != Held::None {
        let s = doc.structure_mut();
        let helvetica = s.add(Std14::Helvetica.dict());
        let zapf = s.add(Std14::ZapfDingbats.dict());
        let rect = vec![
            Object::from(72),
            Object::from(600),
            Object::from(272),
            Object::from(620),
        ];
        let field = s.add(dict! {
            "Type" => Object::name("Annot"),
            "Subtype" => Object::name("Widget"),
            "FT" => Object::name("Tx"),
            "T" => Object::text("name"),
            "V" => Object::text("Ann"),
            "Rect" => rect,
            "P" => page,
            "F" => 4,
            "DA" => Object::text("/Helv 0 Tf 0 g"),
        });
        s.get_dict_mut(page)
            .unwrap()
            .set("Annots", vec![Object::from(field)]);
        let mut form = dict! {
            "Fields" => vec![Object::from(field)],
            "DA" => Object::text("/Helv 0 Tf 0 g"),
            "NeedAppearances" => true,
        };
        match held {
            Held::None => unreachable!(),
            Held::Direct | Held::Form => {
                form.set("DR", dict! { "Font" => dict! { "Helv" => helvetica } });
            }
            Held::Resources => {
                let fonts = dict! { "Helv" => helvetica, "Helv1" => zapf };
                form.set("DR", s.add(dict! { "Font" => fonts }));
            }
            Held::Objects => {
                let fonts = dict! {
                    "Helv" => helvetica, "Helv1" => Object::null(), "Helv2" => zapf,
                };
                let fonts = s.add(fonts);
                form.set("DR", s.add(dict! { "Font" => fonts }));
            }
        }
        let form = match held {
            Held::Form | Held::Objects => Object::from(s.add(form)),
            _ => Object::from(form),
        };
        s.catalog_mut().unwrap().set("AcroForm", form);
    }
    doc.to_bytes().unwrap()
}

/// `value`, through the one reference it may be.
fn direct<'a>(pdf: &'a Pdf<&[u8]>, value: &'a Object) -> &'a Object {
    match value.as_reference() {
        Some(id) => pdf.reading().resolve(id).unwrap(),
        None => value,
    }
}

/// The form `pdf`'s catalog holds, its default resources and their fonts.
fn form<'a>(pdf: &'a Pdf<&[u8]>) -> (&'a Dict, &'a Dict, &'a Dict) {
    let catalog = pdf.catalog().unwrap();
    let form = direct(pdf, catalog.get("AcroForm").unwrap())
        .as_dict()
        .unwrap();
    let resources = direct(pdf, form.get("DR").unwrap()).as_dict().unwrap();
    let fonts = direct(pdf, resources.get("Font").unwrap())
        .as_dict()
        .unwrap();
    (form, resources, fonts)
}

/// The font pdf-forms finds by `name` in `pdf`'s form, found there.
fn found(pdf: &Pdf<&[u8]>, name: &str) -> FieldFont {
    let catalog = pdf.catalog().unwrap();
    let limits = pdfs::forms::Limits::DEFAULT;
    let font =
        FieldFont::find_in_catalog(pdf.reading(), catalog, name.as_bytes(), &limits, &mut ());
    let font = font.unwrap();
    assert!(!font.is_fallback(), "{name} in the form's fonts");
    font
}

/// The `/DA` of each annotation of `pdf`'s first page that isn't a widget.
fn das(pdf: &Pdf<&[u8]>) -> Vec<Vec<u8>> {
    let pages = pdf.pages().unwrap();
    let annots = pages.get(0).unwrap().annots().unwrap();
    annots
        .iter()
        .filter(|annot| annot.dict().get("Subtype").and_then(Object::as_name) != Some(b"Widget"))
        .map(|annot| annot.dict().get("DA").unwrap().as_bytes().unwrap().to_vec())
        .collect()
}

/// `file(held)` edited: a font embedded in it, and two annotations added in
/// it, a `/FreeText` with the `/DA` `/Helv 12 Tf 0 g` and a redaction whose
/// `/DA` is the default one, `/Helv 0 Tf 1 g`; and two `/FreeText`s in
/// Times, a resource of one object, `/TiRo` in `/DA`. Saved whole, with the
/// embedded font's number.
#[cfg(feature = "embed")]
fn edited(held: Held) -> (Vec<u8>, ObjectId) {
    let file = file(held);
    let pdf = pdfs::from_bytes(&file[..]).unwrap();
    let mut doc = pdf.edit().unwrap();
    let font = doc
        .embed_font(pdf_fixtures::font::sample(1000).build())
        .unwrap();
    let ResBody::Written(number) = *font.res().body() else {
        panic!("an embedded font is a number reserved for it");
    };
    let page = doc.page_ids().unwrap()[0].unwrap();
    let style = PaintStyle::default();
    let rect = Rect::new(72.0, 500.0, 372.0, 560.0);
    let da = DefaultAppearance::new("Helv", 12.0, Some(Color::Gray(0.0)));
    let note =
        BuiltAnnot::free_text_with_font(rect, NOTE, &da, Quadding::Left, &style, font.res(), &font);
    doc.add_annot(page, note.unwrap()).unwrap();
    let regions = [QuadPoints::from_rect(Rect::new(72.0, 700.0, 272.0, 720.0))];
    let mark = RedactMark::new()
        .with_quads(&regions)
        .with_interior_color(AnnotColor::Gray(0.0))
        .with_overlay_text(LABEL);
    let rect = mark.bounds().unwrap();
    let redaction = BuiltAnnot::redact_with_font(rect, &mark, font.res(), &font);
    doc.add_annot(page, redaction.unwrap()).unwrap();
    let times = DefaultAppearance::new("TiRo", 10.0, Some(Color::Gray(0.0)));
    for y in [400.0, 300.0] {
        let rect = Rect::new(72.0, y, 372.0, y + 40.0);
        let note = BuiltAnnot::free_text_with_font(
            rect,
            "In Times",
            &times,
            Quadding::Left,
            &style,
            standard_font(Std14::TimesRoman),
            &Std14::TimesRoman,
        );
        doc.add_annot(page, note.unwrap()).unwrap();
    }
    (doc.to_bytes().unwrap(), number)
}

/// A font embedded in a document and given to the two builders is put in
/// the form's fonts under the name `/DA` gives, else the first free one
/// after it, which every `/DA` drawn in it then names: on a file with no
/// form, a form made with no field; on one with a form, its fonts extended
/// where they're held, every entry before kept, the form's fields, `/DA`
/// and `/NeedAppearances` as they were. pdf-forms reads the font back by
/// that name; Times, a resource of one object drawn twice, is one object.
#[cfg(feature = "embed")]
#[test]
fn a_font_an_annotation_is_built_in_is_the_forms_by_its_da_name() {
    for held in Held::ALL {
        let before = file(held);
        let (saved, embedded) = edited(held);
        let old = pdfs::from_bytes(&before[..]).unwrap();
        let pdf = pdfs::from_bytes(&saved[..]).unwrap();
        let name = held.helv();

        // Each /DA names the font by the name it's under.
        let renamed = format!("/{name} 12 Tf 0 g");
        let overlay = format!("/{name} 0 Tf 1 g");
        assert_eq!(
            das(&pdf),
            [
                renamed.as_bytes(),
                overlay.as_bytes(),
                b"/TiRo 10 Tf 0 g",
                b"/TiRo 10 Tf 0 g"
            ],
            "{held:?}"
        );

        // The form's fonts: the embedded font and Times added, every entry
        // before as it was.
        let (form, resources, fonts) = self::form(&pdf);
        assert_eq!(
            fonts.get(name).and_then(Object::as_reference),
            Some(embedded),
            "{held:?}"
        );
        let times = fonts.get("TiRo").and_then(Object::as_reference).unwrap();
        let times = pdf.reading().resolve(times).unwrap().as_dict().unwrap();
        assert_eq!(
            times.get("BaseFont").and_then(Object::as_name),
            Some(&b"Times-Roman"[..])
        );
        let kept = match held {
            Held::None => {
                // A form made: an empty /Fields, and /DR.
                let keys: Vec<&[u8]> = form.iter().map(|(key, _)| key).collect();
                assert_eq!(keys, [&b"Fields"[..], b"DR"]);
                assert_eq!(form.get("Fields").unwrap().as_array(), Some(&[][..]));
                assert_eq!(resources.len(), 1);
                assert!(
                    pdf.catalog()
                        .unwrap()
                        .get("AcroForm")
                        .unwrap()
                        .as_dict()
                        .is_some()
                );
                0
            }
            _ => {
                let (old_form, old_resources, old_fonts) = self::form(&old);
                // Values compared as they are, not where they were written.
                let same = |a: Option<&Object>, b: &Object| a.is_some_and(|a| a.eq_value(b));
                for (key, value) in old_form.iter().filter(|(key, _)| *key != b"DR") {
                    assert!(same(form.get(key), value), "{held:?}");
                }
                assert_eq!(resources.len(), old_resources.len());
                for (key, value) in old_fonts.iter() {
                    if !value.is_null() {
                        assert!(same(fonts.get(key), value), "{held:?}");
                    }
                }
                // Each held where it was: the catalog's /AcroForm, the
                // form's /DR, and their /Font as references to the same
                // objects, or none.
                let catalog = (pdf.catalog().unwrap(), old.catalog().unwrap());
                let at = |a: Option<&Object>, b: Option<&Object>| {
                    assert_eq!(
                        a.and_then(Object::as_reference),
                        b.and_then(Object::as_reference),
                        "{held:?}"
                    );
                };
                at(catalog.0.get("AcroForm"), catalog.1.get("AcroForm"));
                at(form.get("DR"), old_form.get("DR"));
                at(resources.get("Font"), old_resources.get("Font"));
                let fields = pdfs::forms::Form::read(
                    pdf.reading(),
                    catalog.0,
                    Rules::Pdfium,
                    &pdfs::forms::Limits::DEFAULT,
                );
                assert_eq!(fields.unwrap().unwrap().fields().len(), 1);
                old_fonts
                    .iter()
                    .filter(|(_, value)| !value.is_null())
                    .count()
            }
        };
        assert_eq!(fonts.len(), kept + 2, "{held:?}");

        // pdf-forms finds each by its /DA's name: the embedded font, a Type 0
        // font, at its number, and Times.
        let font = found(&pdf, name);
        assert_eq!(font.res(), Some(Res::written(embedded)));
        assert!(matches!(font.font().kind(), FontKind::Type0(_)));
        let times = found(&pdf, "TiRo");
        assert!(matches!(times.font().kind(), FontKind::Type1));
    }
}

/// A Helvetica whose encoding is an object of its own, the resource's
/// second, which the first names as `(1, 0)`.
fn helvetica_of_objects() -> Res {
    Res::of(vec![
        Object::from(dict! {
            "Type" => Object::name("Font"),
            "Subtype" => Object::name("Type1"),
            "BaseFont" => Object::name("Helvetica"),
            "Encoding" => Object::reference(1),
        }),
        Object::from(dict! {
            "Type" => Object::name("Encoding"),
            "BaseEncoding" => Object::name("WinAnsiEncoding"),
        }),
    ])
    .unwrap()
}

/// A `/FreeText` in Helvetica with the `/DA` `da`, its font for the form
/// [`helvetica_of_objects`].
fn in_helvetica_of_objects(da: &DefaultAppearance) -> BuiltAnnot {
    let rect = Rect::new(72.0, 500.0, 372.0, 560.0);
    let style = PaintStyle::default();
    let mut built = BuiltAnnot::free_text(rect, "Helvetica", da, Quadding::Left, &style);
    built.da_font = Some(helvetica_of_objects());
    built
}

/// The `/DA` annotation `annot` of `doc` holds.
fn da_of(doc: &pdfs::pdf_structure::Document, annot: ObjectId) -> Vec<u8> {
    let dict = doc.get_dict(annot).unwrap();
    dict.get("DA").unwrap().as_bytes().unwrap().to_vec()
}

/// A new document of one page, two `/FreeText`s added to it by
/// `build::add_annot` in [`helvetica_of_objects`], `/Helv` in their `/DA`,
/// and the two annotations; saved.
fn of_objects() -> (Vec<u8>, pdfs::pdf_structure::Document, [ObjectId; 2]) {
    let mut doc = Document::new();
    doc.add_page(Rect::LETTER);
    let page = doc.page_ids().unwrap()[0].unwrap();
    let da = DefaultAppearance::new("Helv", 12.0, Some(Color::Gray(0.0)));
    let add = |doc: &mut Document<'_>| {
        let built = in_helvetica_of_objects(&da);
        pdfs::build::add_annot(doc.structure_mut(), page, built, Deflate::DEFAULT).unwrap()
    };
    let annots = [add(&mut doc), add(&mut doc)];
    let file = doc.to_bytes().unwrap();
    (file, doc.into_structure(), annots)
}

/// A font given as a resource of objects is written by `build::add_annot`
/// itself for each annotation, each of its objects at a number of its own
/// and each reference `(i, 0)` among them made the `i`th's, so the form's
/// entry reaches the font's own objects, never the file's; written again,
/// it's another object, so the second annotation's is `Helv1`. A `/DA`
/// that names no font registers nothing; and a document with no catalog is
/// refused where there's a font to register, nothing changed.
#[test]
fn a_font_of_objects_is_written_with_its_references_its_own() {
    let (file, doc, annots) = of_objects();
    let catalog = doc.catalog().unwrap();
    let form = catalog.get("AcroForm").unwrap().as_dict().unwrap();
    let fonts = form.get("DR").unwrap().as_dict().unwrap();
    let fonts = fonts.get("Font").unwrap().as_dict().unwrap();
    let mut encodings = Vec::new();
    for name in ["Helv", "Helv1"] {
        let font = fonts.get(name).and_then(Object::as_reference).unwrap();
        let encoding = doc.get_dict(font).unwrap().get("Encoding").unwrap();
        let encoding = encoding.as_reference().unwrap();
        assert_ne!(encoding, ObjectId::new(1, 0));
        assert_eq!(
            doc.get_dict(encoding).unwrap().get("BaseEncoding"),
            Some(&Object::name("WinAnsiEncoding"))
        );
        encodings.push((font, encoding));
    }
    assert_ne!(encodings[0], encodings[1]);
    assert_eq!(da_of(&doc, annots[0]), b"/Helv 12 Tf 0 g");
    assert_eq!(da_of(&doc, annots[1]), b"/Helv1 12 Tf 0 g");
    // Saved, pdf-forms finds each by its name: the object written.
    let pdf = pdfs::from_bytes(&file[..]).unwrap();
    for (name, (font, _)) in ["Helv", "Helv1"].into_iter().zip(encodings) {
        assert_eq!(found(&pdf, name).res(), Some(Res::written(font)));
    }

    // A /DA with no font: nothing registered, no form made.
    let mut bare = DefaultAppearance::default();
    bare.color = Some(Color::Gray(0.0));
    let mut plain = pdfs::pdf_structure::Document::new(
        Version::new(1, 7),
        dict! { "Root" => ObjectId::new(1, 0) },
    );
    plain.insert(1, dict! { "Type" => Object::name("Catalog") });
    let page = plain.add(dict! { "Type" => Object::name("Page") });
    let built = in_helvetica_of_objects(&bare);
    let annot = pdfs::build::add_annot(&mut plain, page, built, Deflate::DEFAULT).unwrap();
    assert_eq!(da_of(&plain, annot), b"0 g");
    assert!(!plain.catalog().unwrap().has("AcroForm"));

    // No catalog: refused where there's a font to register, nothing added;
    // added as before where there's none.
    let mut rootless = pdfs::pdf_structure::Document::new(Version::new(1, 7), dict! {});
    let page = rootless.add(dict! { "Type" => Object::name("Page") });
    let objects = rootless.objects().count();
    let da = DefaultAppearance::new("Helv", 12.0, Some(Color::Gray(0.0)));
    let refused = pdfs::build::add_annot(
        &mut rootless,
        page,
        in_helvetica_of_objects(&da),
        Deflate::DEFAULT,
    );
    assert!(matches!(refused, Err(pdfs::Error::NoCatalog)));
    assert_eq!(rootless.objects().count(), objects);
    assert!(!rootless.get_dict(page).unwrap().has("Annots"));
    let mut fontless = in_helvetica_of_objects(&da);
    fontless.da_font = None;
    assert!(pdfs::build::add_annot(&mut rootless, page, fontless, Deflate::DEFAULT).is_ok());
}

/// Each file the tests above save, written to `folder`, by name.
fn saved_files(folder: &Path) -> Vec<std::path::PathBuf> {
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    #[cfg(feature = "embed")]
    for held in Held::ALL {
        files.insert(format!("pdfs-annots-{held:?}.pdf"), edited(held).0);
    }
    for held in Held::ALL {
        files.insert(format!("pdfs-annots-{held:?}-before.pdf"), file(held));
    }
    files.insert("pdfs-annots-resource-of-objects.pdf".into(), of_objects().0);
    std::fs::create_dir_all(folder).unwrap();
    let mut paths = Vec::new();
    for (name, bytes) in files {
        let path = folder.join(name);
        std::fs::write(&path, bytes).unwrap();
        paths.push(path);
    }
    paths.sort();
    paths
}

/// `qpdf --check` finds every file the tests save clean, the forms they
/// extend and make among them.
#[test]
#[ignore = "needs qpdf: `just test-qpdf`"]
fn qpdf_checks_each_saved_file_clean() {
    let folder = Path::new(env!("CARGO_TARGET_TMPDIR")).join("pdfs-annots");
    for path in saved_files(&folder) {
        let qpdf = Qpdf::new(&path);
        if let Err(why) = qpdf.version() {
            panic!("this test needs qpdf ({why}): install it, or name it in PDF_LINT_QPDF");
        }
        let ran = qpdf
            .exec(&["--check".to_string(), path.display().to_string()])
            .expect("qpdf runs");
        let said = String::from_utf8_lossy(&ran.stdout).to_string()
            + &String::from_utf8_lossy(&ran.stderr);
        println!("{}: {said}", path.display());
        assert_eq!(ran.code, Some(0), "{}: {said}", path.display());
        assert!(!said.contains("WARNING"), "{}: {said}", path.display());
        std::fs::remove_file(&path).unwrap();
    }
}
