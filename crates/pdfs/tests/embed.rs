//! Text drawn in a TrueType program embedded in a document, saved and read
//! back: drawn by its codes and laid out in a box, in Latin, Chinese, Hebrew,
//! Arabic and past the Basic Multilingual Plane; saved again after more is
//! drawn, linearized, as an update and encrypted; the font's objects as
//! written; and what's refused, with nothing drawn.

#![cfg(feature = "embed")]

use pdf_fixtures::font::{TrueType, sample};
use pdfs::pdf_content_streams::ResBody;
use pdfs::pdf_fonts::Font;
use pdfs::pdf_fonts::embed::{self, Program};
use pdfs::pdf_objects::{Dict, Object, ObjectId, Resolve as _};
use pdfs::prelude::*;

/// What's drawn, each a script the sample program has, no two of whose
/// characters, in any of them, it draws with one glyph: two of its Latin
/// letters composites.
const TEXTS: [&str; 5] = [
    "Ag H\u{C4}\u{1DE}",
    "\u{4E2D}\u{6587}\u{4E16}\u{754C}",
    "\u{5E9}\u{5DC}\u{5D5}\u{5DD}",
    "\u{633}\u{644}\u{627}\u{645}",
    "\u{1D11E}",
];

/// The sample program, at `units_per_em`, its bytes.
fn program(units_per_em: u16) -> Vec<u8> {
    sample(units_per_em).build()
}

/// The number `font` is written at.
fn number(font: &EmbeddedFont) -> ObjectId {
    match font.res().body() {
        ResBody::Written(id) => *id,
        _ => panic!("an embedded font is a number reserved for it"),
    }
}

/// `texts` hold no two characters `program` draws with one glyph, so each
/// reads back as itself (text is kept a glyph, not a character).
fn on_glyphs_of_their_own(program: &Program, texts: &[&str]) {
    let mut seen = std::collections::BTreeMap::new();
    for c in texts.iter().flat_map(|text| text.chars()) {
        let glyph = program.glyph(c).expect("drawn");
        assert_eq!(
            *seen.entry(glyph).or_insert(c),
            c,
            "{c:?} on another's glyph"
        );
    }
}

/// `file`'s pages' text, each joined.
fn texts(file: &[u8]) -> Vec<String> {
    let pdf = pdfs::from_bytes(file).unwrap();
    let pages = pdf.pages().unwrap();
    let read = |page: pdfs::Page<'_, &[u8]>| {
        let text = page.text().unwrap();
        assert!(text.is_whole());
        text.text()
    };
    pages.iter().map(read).collect()
}

/// The font page `index` of `pdf` draws with, its one, as read.
fn page_font<S: pdfs::pdf_structure::read::ByteSource>(
    pdf: &Pdf<S>,
    index: usize,
) -> std::sync::Arc<Font> {
    let pages = pdf.pages().unwrap();
    let fonts = pages.get(index).unwrap().fonts().unwrap();
    assert_eq!(fonts.len(), 1);
    fonts[0].font.clone().unwrap().unwrap()
}

/// Each character of `texts`, read through `read`, `program`'s width.
fn widths_are_the_programs(read: &Font, program: &Program, texts: &[&str]) {
    for c in texts.iter().flat_map(|text| text.chars()) {
        let glyph = program.glyph(c).unwrap();
        let width = program.width(glyph);
        assert!(width > 0.0);
        assert_eq!(read.width(u32::from(glyph)), width, "{c:?}");
    }
}

/// A page of Letter's box drawn as `canvas` is, added to `doc`.
fn add(doc: &mut Document<'_>, canvas: Canvas) {
    *doc.add_page(Rect::LETTER) = canvas.into_writer();
}

/// Each text drawn in its own line by its codes ([`Canvas::text_at`]).
fn drawn_at(font: &EmbeddedFont, texts: &[&str]) -> Canvas {
    let mut canvas = Canvas::new();
    for (i, text) in texts.iter().enumerate() {
        let at = Point::new(72.0, 700.0 - 30.0 * i as f32);
        canvas.text_at(font.res(), 24.0, at, &font.codes(text).unwrap());
    }
    canvas
}

/// Text in each script drawn in a font embedded in a new document, by its
/// codes and laid out in a box, a document each, reads back from the file
/// saved as drawn: the page's text, and the font's widths the program's.
#[test]
fn text_drawn_in_an_embedded_font_reads_back() {
    let checked = Program::open(program(1000), 0).unwrap();
    on_glyphs_of_their_own(&checked, &TEXTS);
    let mut doc = Document::new();
    let font = doc.embed_font(program(1000)).unwrap();
    add(&mut doc, drawn_at(&font, &TEXTS));
    let file = doc.to_bytes().unwrap();
    assert_eq!(texts(&file), [TEXTS.concat()]);
    let pdf = pdfs::from_bytes(&file[..]).unwrap();
    let read = page_font(&pdf, 0);
    assert_eq!(read.id(), Some(number(&font)));
    widths_are_the_programs(&read, font.program(), &TEXTS);

    // Laid out in a box, a paragraph a script: each line shown, and read
    // back joined; a document of its own, so only the layout records its
    // glyphs.
    #[cfg(feature = "text-layout")]
    {
        let mut doc = Document::new();
        let font = doc.embed_font(program(2048)).unwrap();
        let mut canvas = Canvas::new();
        let rect = Rect::new(72.0, 72.0, 540.0, 720.0);
        let style = TextBoxStyle::default();
        let laid = canvas.text_box(font.res(), &font, &TEXTS.join("\n"), rect, &style);
        assert_eq!(laid.unwrap().lines.len(), 5);
        add(&mut doc, canvas);
        let file = doc.to_bytes().unwrap();
        assert_eq!(texts(&file), [TEXTS.concat()]);
        let pdf = pdfs::from_bytes(&file[..]).unwrap();
        widths_are_the_programs(&page_font(&pdf, 0), font.program(), &TEXTS);
    }
}

/// A second save, after more is drawn on a page added, writes the font
/// again at the same numbers, its program cut to the glyphs both pages
/// draw: the first page still reads as drawn.
#[test]
fn a_second_save_writes_the_font_again_with_more() {
    let mut doc = Document::new();
    let font = doc.embed_font(program(1000)).unwrap();
    add(&mut doc, drawn_at(&font, &TEXTS[..1]));
    let first = doc.to_bytes().unwrap();
    add(&mut doc, drawn_at(&font, &TEXTS[1..]));
    let second = doc.to_bytes().unwrap();
    assert_eq!(texts(&first), [TEXTS[0]]);
    assert_eq!(texts(&second), [TEXTS[0].to_owned(), TEXTS[1..].concat()]);

    let (first, second) = (Walked::of(&first), Walked::of(&second));
    assert_eq!(first.numbers[0], number(&font));
    assert_eq!(second.numbers, first.numbers, "the same objects");
    // `.notdef`, the space, A, g, H, Ä and Ǟ and the two marks and Ä they're
    // made of; then the rest of the scripts' 13 glyphs.
    assert_eq!(first.program.glyph_count(), 9);
    assert_eq!(second.program.glyph_count(), 9 + 13);
    let pdf = pdfs::from_bytes(&second.file[..]).unwrap();
    for page in 0..2 {
        widths_are_the_programs(&page_font(&pdf, page), font.program(), &TEXTS);
    }
}

/// A font's objects, as a file holds them: walked from its Type 0 font to
/// its program and its `/ToUnicode`, as a page's resources reach them.
struct Walked {
    file: Vec<u8>,
    /// The Type 0 font's, its CIDFont's, its descriptor's, its program's,
    /// its `/ToUnicode`'s and its `/CIDToGIDMap`'s numbers.
    numbers: [ObjectId; 6],
    /// The program, decoded and opened.
    program: Program,
}

impl Walked {
    /// The objects of the font `file`'s first page draws in, its one,
    /// reached from its resources, each stream deflated, the program's
    /// `/Length1` its decoded length.
    fn of(file: &[u8]) -> Walked {
        let mut walked = Walked::of_reading(&pdfs::from_bytes(file).unwrap());
        walked.file = file.to_vec();
        walked
    }

    /// The font's objects in `pdf`'s file, as [`Walked::of`] walks them, but
    /// for the file's bytes.
    fn of_reading<S: pdfs::pdf_structure::read::ByteSource>(pdf: &Pdf<S>) -> Walked {
        let reading = pdf.reading();
        let object = |id: ObjectId| reading.resolve(id).unwrap();
        let named = |dict: &Dict, key: &str| dict.get(key).and_then(Object::as_reference).unwrap();
        let pages = pdf.pages().unwrap();
        let page = pages.get(0).unwrap().attributes();
        let resources = page
            .dict
            .unwrap()
            .get("Resources")
            .unwrap()
            .as_dict()
            .unwrap();
        let fonts = resources.get("Font").unwrap().as_dict().unwrap();
        assert_eq!(fonts.len(), 1);
        let type0 = fonts.iter().next().unwrap().1.as_reference().unwrap();
        let font = object(type0).as_dict().unwrap();
        assert_eq!(font.get("Subtype").unwrap().as_name(), Some(&b"Type0"[..]));
        let encoding = font.get("Encoding").unwrap().as_name();
        assert_eq!(encoding, Some(&b"Identity-H"[..]));
        let descendants = font.get("DescendantFonts").unwrap().as_array().unwrap();
        let cid = descendants[0].as_reference().unwrap();
        let cid_font = object(cid).as_dict().unwrap();
        let subtype = cid_font.get("Subtype").unwrap().as_name();
        assert_eq!(subtype, Some(&b"CIDFontType2"[..]));
        let descriptor = named(cid_font, "FontDescriptor");
        let file2 = named(object(descriptor).as_dict().unwrap(), "FontFile2");
        let to_unicode = named(font, "ToUnicode");
        let to_gid = named(cid_font, "CIDToGIDMap");
        for stream in [file2, to_unicode, to_gid] {
            let dict = &object(stream).as_stream().unwrap().dict;
            let filter = dict.get("Filter").and_then(Object::as_name);
            assert_eq!(filter, Some(&b"FlateDecode"[..]), "{stream:?} deflated");
        }
        let data = pdf.stream(file2).unwrap();
        let length1 = object(file2).as_stream().unwrap().dict.get("Length1");
        assert_eq!(
            length1.and_then(Object::as_integer),
            Some(data.len() as i64)
        );
        let program = Program::open(data.into_owned(), 0).unwrap();
        let cmap = pdf.stream(to_unicode).unwrap();
        assert!(cmap.starts_with(b"/CIDInit /ProcSet findresource begin"));
        let numbers = [type0, cid, descriptor, file2, to_unicode, to_gid];
        Walked {
            file: Vec::new(),
            numbers,
            program,
        }
    }
}

/// Saved whole, the font's objects are as a page's resources reach them,
/// each stream deflated and the program's `/Length1` its decoded length.
#[test]
fn the_font_is_written_its_streams_deflated() {
    let mut doc = Document::new();
    let font = doc.embed_font(program(1000)).unwrap();
    add(&mut doc, drawn_at(&font, &TEXTS));
    let walked = Walked::of(&doc.to_bytes().unwrap());
    assert_eq!(walked.numbers[0], number(&font));
    let numbers: std::collections::BTreeSet<_> = walked.numbers.into();
    assert_eq!(numbers.len(), 6);
    assert_eq!(walked.program.glyph_count(), 22);
}

/// Saved twice, nothing drawn between, a document drawn in an embedded font
/// writes the same bytes in the clear; and so does the same drawing in
/// another document.
#[test]
fn saved_twice_it_writes_the_same_bytes() {
    let saved = || {
        let mut doc = Document::new();
        let font = doc.embed_font(program(1000)).unwrap();
        add(&mut doc, drawn_at(&font, &TEXTS));
        let file = doc.to_bytes().unwrap();
        assert_eq!(doc.to_bytes().unwrap(), file);
        file
    };
    assert_eq!(saved(), saved());
}

/// Linearized, the font is written as a whole save writes it, numbered as
/// the layout numbers it, and the linearization holds.
#[test]
fn a_linearized_save_writes_the_font() {
    let mut doc = Document::new();
    let font = doc.embed_font(program(1000)).unwrap();
    add(&mut doc, drawn_at(&font, &TEXTS));
    let mut out = Vec::new();
    doc.save_linearized_to(&mut out, &doc.linearize_options())
        .unwrap();
    let verdict = pdfs::from_bytes(&out[..]).unwrap().linearized().unwrap();
    assert!(verdict.holds && verdict.judged_whole, "{verdict:?}");
    assert_eq!(texts(&out), [TEXTS.concat()]);
    // Laid out anew, at numbers of its own.
    assert_eq!(Walked::of(&out).program.glyph_count(), 22);
}

/// A font embedded in an edited file is written in the update that adds the
/// page drawn in it.
#[test]
fn an_update_writes_the_font() {
    let mut base = Document::new();
    let page = base.add_page(Rect::LETTER);
    page.begin_text();
    page.font(standard_font(Std14::Helvetica), 12.0);
    page.show(&Std14::Helvetica.codes("Before").unwrap());
    page.end_text();
    let base = base.to_bytes().unwrap();
    let pdf = pdfs::from_bytes(&base[..]).unwrap();
    let mut doc = pdf.edit().unwrap();
    let font = doc.embed_font(program(1000)).unwrap();
    add(&mut doc, drawn_at(&font, &TEXTS));
    let mut update = Vec::new();
    doc.save_update_to(&mut update).unwrap().unwrap();
    let updated = [&base[..], &update].concat();
    assert_eq!(texts(&updated), ["Before".to_owned(), TEXTS.concat()]);
    let id = number(&font);
    assert!(pdf.reading().resolve(id).is_none(), "not in the file read");
    let read = pdfs::from_bytes(&updated[..]).unwrap();
    assert_eq!(page_font(&read, 1).id(), Some(id));
    widths_are_the_programs(&page_font(&read, 1), font.program(), &TEXTS);
}

/// Encrypted (AES-256), the font is written encrypted as every stream is,
/// and read back by the password as drawn.
#[test]
fn an_encrypted_save_writes_the_font() {
    let mut doc = Document::new();
    let font = doc.embed_font(program(1000)).unwrap();
    add(&mut doc, drawn_at(&font, &TEXTS));
    let password = Password("secret").prepared().unwrap();
    doc.encrypt(&Protection::new(&password)).unwrap();
    let file = doc.to_bytes().unwrap();
    let mut options = pdfs::Options::DEFAULT;
    options.passwords = &[Password("secret")];
    let pdf = options.from_bytes(&file[..]).unwrap();
    assert!(matches!(pdf.encryption(), Encryption::Opened { .. }));
    let pages = pdf.pages().unwrap();
    assert_eq!(pages.get(0).unwrap().text().unwrap().text(), TEXTS.concat());
    widths_are_the_programs(&page_font(&pdf, 0), font.program(), &TEXTS);
    let walked = Walked::of_reading(&pdf);
    assert_eq!(walked.program.glyph_count(), 22);
    // With no password, it's locked.
    let locked = pdfs::from_bytes(&file[..]).unwrap();
    assert_eq!(*locked.encryption(), Encryption::Locked);
}

/// A font embedded and never drawn in is written all the same, its program
/// `.notdef` alone and its `/W` empty; one drawn in only on a page refused
/// is written as never drawn in.
#[test]
fn a_font_never_drawn_in_is_written() {
    let mut doc = Document::new();
    let font = doc.embed_font(program(1000)).unwrap();
    add(&mut doc, Canvas::new());
    let file = doc.to_bytes().unwrap();
    let pdf = pdfs::from_bytes(&file[..]).unwrap();
    let written = pdf.reading().resolve(number(&font)).unwrap();
    let written = written.as_dict().unwrap();
    assert_eq!(
        written.get("Subtype").unwrap().as_name(),
        Some(&b"Type0"[..])
    );
    assert_eq!(widths(&file, &font), Vec::<Object>::new());
}

/// The pages a document's page ids place are placed without the font: it's
/// written at the save, with what's drawn after, which then reads back.
#[test]
fn page_ids_place_the_pages_but_not_the_font() {
    let mut doc = Document::new();
    let font = doc.embed_font(program(1000)).unwrap();
    add(&mut doc, drawn_at(&font, &TEXTS[..1]));
    assert_eq!(doc.page_ids().unwrap().len(), 1);
    assert!(
        doc.structure().resolve(number(&font)).is_none(),
        "not written"
    );
    add(&mut doc, drawn_at(&font, &TEXTS[1..]));
    let file = doc.to_bytes().unwrap();
    assert_eq!(texts(&file), [TEXTS[0].to_owned(), TEXTS[1..].concat()]);
}

/// `font`'s CIDFont's `/W` in `file`.
fn widths(file: &[u8], font: &EmbeddedFont) -> Vec<Object> {
    let pdf = pdfs::from_bytes(file).unwrap();
    let reading = pdf.reading();
    let type0 = reading.resolve(number(font)).unwrap().as_dict().unwrap();
    let descendants = type0.get("DescendantFonts").unwrap().as_array().unwrap();
    let cid = reading.resolve(descendants[0].as_reference().unwrap());
    let cid = cid.unwrap().as_dict().unwrap();
    cid.get("W").unwrap().as_array().unwrap().to_vec()
}

/// A program whose licence restricts it to its owner's leave, or to its
/// bitmaps, is refused, nothing reserved for it; said to be the caller's
/// right, it's embedded and drawn in.
#[test]
fn a_restricted_program_is_embedded_only_when_the_caller_says_so() {
    for fs_type in [0x0002, 0x0200] {
        let mut fixture = sample(1000);
        fixture.os2.as_mut().unwrap().fs_type = fs_type;
        let restricted = fixture.build();
        let mut doc = Document::new();
        let before = doc.structure().max_id();
        let refused = doc.embed_font(restricted.clone()).unwrap_err();
        assert!(
            matches!(&refused, Error::Embed(embed::Error::Restricted(_))),
            "{refused:?}"
        );
        assert_eq!(doc.structure().max_id(), before, "nothing reserved");
        let mut options = embed::Options::default();
        options.licensed = true;
        let font = doc.embed_font_with(restricted, 0, &options).unwrap();
        add(&mut doc, drawn_at(&font, &TEXTS));
        assert_eq!(texts(&doc.to_bytes().unwrap()), [TEXTS.concat()]);
    }
    // What isn't a program it opens is refused too.
    let refused = Document::new().embed_font(b"not a font".to_vec());
    assert!(matches!(refused, Err(Error::Embed(embed::Error::NotSfnt))));
}

/// A character the program has no glyph for is refused, by its codes and by
/// a layout, with nothing drawn: its codes record nothing; a layout records
/// the characters' before it, which the font is then written with.
#[test]
fn a_character_the_program_has_no_glyph_for_is_refused() {
    let mut doc = Document::new();
    let font = doc.embed_font(program(1000)).unwrap();
    let refused = font.codes("A\u{1F600}").unwrap_err();
    assert_eq!((refused.at, refused.character), (1, '\u{1F600}'));
    let refused = font.codes("Ag\n").unwrap_err();
    assert_eq!((refused.at, refused.character), (2, '\n'));
    add(&mut doc, Canvas::new());
    assert_eq!(widths(&doc.to_bytes().unwrap(), &font), []);

    #[cfg(feature = "text-layout")]
    {
        let mut canvas = Canvas::new();
        let rect = Rect::new(72.0, 72.0, 540.0, 720.0);
        let refused = canvas
            .text_box(
                font.res(),
                &font,
                "Ag\u{1F600}",
                rect,
                &TextBoxStyle::default(),
            )
            .unwrap_err();
        assert_eq!((refused.at, refused.character), (2, '\u{1F600}'));
        let reserved = pdfs::pdf_content_streams::Reserved::default();
        let (content, _) = canvas.into_writer().named(&reserved);
        assert!(content.is_empty(), "nothing drawn");
        // A and g, CIDs 2 and 3, recorded, as the layout encoded them.
        let widths = widths(&doc.to_bytes().unwrap(), &font);
        assert_eq!(widths.len(), 2, "one run");
        assert_eq!(widths[0].as_integer(), Some(2));
        let run: Vec<_> = (widths[1].as_array().unwrap().iter())
            .map(Object::as_integer)
            .collect();
        assert_eq!(run, [Some(600), Some(500)]);
    }
}

/// Pages drawn on threads of their own draw in one font, which records
/// every glyph each draws.
#[test]
fn a_font_is_drawn_in_from_other_threads() {
    let mut doc = Document::new();
    let font = doc.embed_font(program(1000)).unwrap();
    let canvases: Vec<Canvas> = std::thread::scope(|scope| {
        let drawn: Vec<_> = (TEXTS.iter())
            .map(|text| {
                let font = font.clone();
                scope.spawn(move || drawn_at(&font, &[text]))
            })
            .collect();
        drawn
            .into_iter()
            .map(|drawn| drawn.join().unwrap())
            .collect()
    });
    for canvas in canvases {
        add(&mut doc, canvas);
    }
    let file = doc.to_bytes().unwrap();
    assert_eq!(texts(&file), TEXTS);
}

/// No `Debug` holds the text drawn in a font, nor its program's names.
#[test]
fn no_debug_holds_what_is_drawn() {
    let mut fixture: TrueType = sample(1000);
    for (c, glyph) in [('M', 17), ('R', 18), ('K', 19), ('E', 20)] {
        fixture.map(c, glyph);
    }
    fixture.names = vec![(6, "MARKERSans-Regular".to_owned())];
    let mut doc = Document::new();
    let font = doc.embed_font(fixture.build()).unwrap();
    let mut canvas = Canvas::new();
    canvas.text_at(
        font.res(),
        12.0,
        Point::new(72.0, 700.0),
        &font.codes("MARKER").unwrap(),
    );
    add(&mut doc, canvas);
    for shown in [format!("{font:?}"), format!("{doc:?}")] {
        assert!(!shown.contains("MARKER"), "{shown}");
    }
    assert!(format!("{font:?}").contains("glyphs: 5"), "{font:?}");
    doc.to_bytes().unwrap();
    assert!(!format!("{doc:?}").contains("MARKER"));
    let refused = font.codes("MARKER\u{1F600}").unwrap_err();
    assert!(!refused.to_string().contains("MARKER"));
}

/// The text the Form XObject `id` of `pdf` shows, read through its own
/// `/Resources`, and the fonts those name.
#[cfg(feature = "text-layout")]
fn form_text(pdf: &Pdf<&[u8]>, id: ObjectId) -> (String, Vec<ObjectId>) {
    use std::ops::ControlFlow;

    use pdfs::content_streams::Concat;
    use pdfs::gfx::{Kind, Limits, Opts, interpret};

    let reading = pdf.reading();
    let form = reading.resolve(id).unwrap().as_stream().unwrap();
    assert_eq!(
        form.dict.get("Subtype").unwrap().as_name(),
        Some(&b"Form"[..])
    );
    let resources = form.dict.get("Resources");
    let fonts = resources
        .and_then(|resources| resources.as_dict()?.get("Font")?.as_dict())
        .map_or(Vec::new(), |fonts| {
            let number = |(_, font): (_, &Object)| font.as_reference().unwrap();
            fonts.iter().map(number).collect()
        });
    let content = pdf.stream(id).unwrap();
    let slices = [&content[..]];
    let mut text = String::new();
    let summary = interpret(
        reading,
        &Concat::new(&slices),
        resources,
        &Limits::DEFAULT,
        &pdfs::fonts::Fonts::default(),
        Opts::default(),
        |event| {
            if let Kind::Text(show) = &event.kind {
                for glyph in show.glyphs() {
                    text.push_str(&glyph.text().expect("the font's /ToUnicode has it"));
                }
            }
            ControlFlow::Continue(())
        },
    );
    assert!(!summary.unwrap().stopped);
    (text, fonts)
}

/// A `/FreeText` and a `/Redact` over two regions, their text Chinese and
/// Hebrew, built in a font embedded in the document and added to a page
/// drawn in it too: saved and reopened, each appearance stream shows the
/// text given under the one font, written once with every glyph drawn; the
/// redaction's overlay is its rollover appearance too; and a character the
/// program has no glyph for builds nothing.
#[cfg(feature = "text-layout")]
#[test]
fn annotations_text_in_an_embedded_font_reads_back() {
    use pdfs::annots::{
        AnnotColor, AppearanceMode, BuiltAnnot, DefaultAppearance, PaintStyle, QuadPoints,
        Quadding, RedactMark, TypedAnnot,
    };
    use pdfs::content_streams::Color;
    use pdfs::gfx::UnencodableChar;

    const PAGE: &str = "Ag";
    const NOTE: &str = "\u{4E2D}\u{6587} \u{5E9}\u{5DC}\u{5D5}\u{5DD}";
    const LABEL: &str = "\u{4E16}\u{754C} \u{5E9}\u{5DC}";

    let mut doc = Document::new();
    let font = doc.embed_font(program(1000)).unwrap();
    on_glyphs_of_their_own(font.program(), &[PAGE, NOTE, LABEL]);
    add(&mut doc, drawn_at(&font, &[PAGE]));
    let page_id = doc.page_ids().unwrap()[0].unwrap();

    let da = DefaultAppearance::new("Helv", 12.0, Some(Color::Gray(0.0)));
    let style = PaintStyle::new(
        Some(AnnotColor::Gray(0.0)),
        Some(AnnotColor::Rgb([1.0, 1.0, 0.8])),
    );
    let note_rect = Rect::new(72.0, 500.0, 372.0, 560.0);
    let note = BuiltAnnot::free_text_with_font(
        note_rect,
        NOTE,
        &da,
        Quadding::Left,
        &style,
        font.res(),
        &font,
    )
    .unwrap();
    // Over the page's own text, and a region below it.
    let quads = [
        QuadPoints::from_rect(Rect::new(72.0, 696.0, 272.0, 724.0)),
        QuadPoints::from_rect(Rect::new(72.0, 656.0, 272.0, 684.0)),
    ];
    let mark = RedactMark::new()
        .with_quads(&quads)
        .with_interior_color(AnnotColor::Gray(0.0))
        .with_overlay_text(LABEL);
    let rect = mark.bounds().unwrap();
    let redaction = BuiltAnnot::redact_with_font(rect, &mark, font.res(), &font).unwrap();

    // A character the program has no glyph for: named, and nothing built.
    let refused = UnencodableChar {
        at: NOTE.len(),
        character: '\u{1F600}',
    };
    let bad = format!("{NOTE}\u{1F600}");
    let built = BuiltAnnot::free_text_with_font(
        note_rect,
        &bad,
        &da,
        Quadding::Left,
        &style,
        font.res(),
        &font,
    );
    assert_eq!(built.map(|built| built.dict), Err(refused));
    let bad_mark = mark.with_overlay_text(&bad);
    let built = BuiltAnnot::redact_with_font(rect, &bad_mark, font.res(), &font);
    assert_eq!(built.map(|built| built.dict), Err(refused));

    doc.add_annot(page_id, note).unwrap();
    doc.add_annot(page_id, redaction).unwrap();
    let file = doc.to_bytes().unwrap();

    let pdf = pdfs::from_bytes(&file[..]).unwrap();
    let pages = pdf.pages().unwrap();
    let page = pages.get(0).unwrap();
    assert_eq!(page.text().unwrap().text(), PAGE);
    let annots = page.annots().unwrap();
    assert_eq!(annots.len(), 2);
    let normal = |annot: &pdfs::annots::Annot<'_>| {
        let appearances = annot.appearances().expect("/AP");
        appearances
            .stream(AppearanceMode::Normal, false, false)
            .expect("/AP /N")
    };
    let embedded = number(&font);

    // The FreeText: its contents, and its appearance's text in the font.
    assert!(matches!(annots[0].typed(), TypedAnnot::FreeText(_)));
    assert_eq!(annots[0].rect(), note_rect);
    assert_eq!(annots[0].contents(), Some(NOTE));
    assert_eq!(
        form_text(&pdf, normal(&annots[0])),
        (NOTE.to_owned(), vec![embedded])
    );

    // The redaction: its mark draws no text; its overlay the label in each
    // region, in the font; and /AP /R is the overlay.
    let TypedAnnot::Redact(read) = annots[1].typed() else {
        panic!("a redaction");
    };
    assert_eq!(annots[1].rect(), rect);
    assert_eq!(read.quad_points, quads);
    assert_eq!(read.overlay_text.as_deref(), Some(LABEL));
    assert_eq!(
        form_text(&pdf, normal(&annots[1])),
        (String::new(), Vec::new())
    );
    let overlay = read.overlay_form.expect("/RO");
    assert_ne!(overlay, normal(&annots[1]));
    let appearances = annots[1].appearances().unwrap();
    assert_eq!(
        appearances.stream(AppearanceMode::Rollover, false, false),
        Some(overlay)
    );
    assert_eq!(
        form_text(&pdf, overlay),
        ([LABEL, LABEL].concat(), vec![embedded])
    );

    // One font in the file, the page's and the two forms', its program cut
    // to `.notdef` and the glyphs the three drew.
    let reading = pdf.reading();
    let type0s = reading.ids().filter(|&id| {
        let dict = reading.resolve(id).and_then(Object::as_dict);
        dict.is_some_and(|dict| {
            dict.get("Type").and_then(Object::as_name) == Some(&b"Font"[..])
                && dict.get("Subtype").and_then(Object::as_name) == Some(&b"Type0"[..])
        })
    });
    assert_eq!(type0s.collect::<Vec<_>>(), [embedded]);
    let programs = reading.ids().filter(|&id| {
        let dict = reading.resolve(id).and_then(Object::as_dict);
        dict.is_some_and(|dict| dict.get("FontFile2").is_some())
    });
    assert_eq!(programs.count(), 1);
    let walked = Walked::of(&file);
    assert_eq!(walked.numbers[0], embedded);
    let drawn: std::collections::BTreeSet<_> = [PAGE, NOTE, LABEL]
        .iter()
        .flat_map(|text| text.chars())
        .map(|c| font.program().glyph(c).unwrap())
        .collect();
    assert_eq!(usize::from(walked.program.glyph_count()), 1 + drawn.len());
    widths_are_the_programs(&page_font(&pdf, 0), font.program(), &[PAGE, NOTE, LABEL]);
}
