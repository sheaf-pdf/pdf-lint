//! The facade timed beside the very crate calls it makes, under the same
//! options, so that what it costs over them, its overhead alone, can be
//! judged before and after a change, by `cargo bench -p pdfs`:
//!
//! - `open`: a file of 2,000 pages opened from memory, by `pdfs::from_bytes`
//!   (`facade`) and by pdf-structure's `Reading::open` under the facade's
//!   defaults, PDFium's recovery, zlib-rs and no fault looked for (`direct`).
//! - `pages`: opened, then its page tree walked by PDFium's rules, by
//!   `Pdf::pages` (`facade`) and by pdf-semantics' `catalog` and
//!   `pages::pages` (`direct`).
//! - `content`: opened, walked, then each page's content joined and its
//!   color spaces counted, by `Page::content` (`facade`) and by
//!   pdf-content-streams' `Join::push_contents` and `ResCx::read` (`direct`).
//! - `text`: opened, walked, then each page's text read, its content joined
//!   and the strings it shows walked in its fonts, a form among them, by
//!   `Page::text` (`facade`) and by `Join::push_contents`, `ResCx::read` and
//!   pdf-fonts' `shown` over fonts read once a file (`direct`).
//! - `save`: a new document of 2,000 pages, each drawing a square and a line
//!   in Helvetica, the font written once, saved as `Document::save` saves one
//!   (`facade`), and by the very calls it makes, pdf-content-streams'
//!   `ContentWriter`, pdf-structure's `Deflate::encode` and document,
//!   pdf-semantics' `pages::write` and pdf-structure's `write::document`
//!   (`direct`): the same bytes, held so before they're timed.
//!
//! The file is written here by pdf-structure's writer: a catalog, a root over
//! 2,000 pages, each with a `/Contents` of two streams, one deflated, and
//! resources naming a color space of its own, the page tree's objects in
//! object streams under a cross-reference stream. `text` reads it with a font
//! in each page's resources, Helvetica with a `/ToUnicode`, and a form each
//! page draws, all shared. Throughput is in pages. Each side is held to what it
//! gives before it's timed: the same pages, each page's content the same
//! bytes, and its text the same. Nothing is read from disk, nothing is
//! encrypted and no cap is met, so the inputs are the same on every run.

use std::collections::HashMap;
use std::hint::black_box;

use criterion::{Criterion, Throughput};
use pdfs::pdf_content_streams::{
    ContentWriter, Join, Limits as ContentLimits, Paint, Res, ResCx, Reserved,
};
use pdfs::pdf_filters::{Backend, Prediction};
use pdfs::pdf_fonts::Fonts;
use pdfs::pdf_fonts::Std14;
use pdfs::pdf_objects::object::{Stream, Value};
use pdfs::pdf_objects::pdf_tokenizer::Rules;
use pdfs::pdf_objects::version::Version;
use pdfs::pdf_objects::{Object, dict};
use pdfs::pdf_semantics::Limits as SemanticsLimits;
use pdfs::pdf_semantics::pages::{self, Inheritable};
use pdfs::pdf_structure::read::{self, Faults, Reading, Recovery};
use pdfs::pdf_structure::write::{self, Deflate};
use pdfs::pdf_structure::{Document, XrefKind};
use pdfs::{Limits, Options};

/// How many pages the file has.
const PAGES: u32 = 2_000;

/// A page's two content streams: what it draws, and its text.
const DRAWN: &str = "q 0.5 0 0 0.5 72 72 cm /CS0 cs 0.1 0.2 0.3 0.4 scn 0 0 100 100 re f Q \
    q 1 0 0 RG 2 w 10 10 m 200 200 l S Q";
const TEXT: &str = "BT /F1 12 Tf 72 720 Td (A line of text on the page) Tj 0 -14 Td \
    [(Kerned) -120 (text)] TJ ET";

/// What each page's second stream draws in the file `text` reads: the form.
const FORM: &str = " /X Do";

/// The file the benches read; with `fonts`, each page's resources naming a font
/// and a form, both shared, and its text drawing the form.
fn file(fonts: bool) -> Vec<u8> {
    let mut doc = Document::new(Version::new(1, 7), dict! { "Root" => Object::reference(1) });
    doc.insert(
        1,
        dict! { "Type" => "Catalog", "Pages" => Object::reference(2) },
    );
    let kids: Vec<Object> = (0..PAGES).map(|i| Object::reference(10 + 3 * i)).collect();
    let media = vec![Object::from(0), 0.into(), 612.into(), 792.into()];
    let root = dict! {
        "Type" => "Pages", "Kids" => kids, "Count" => i64::from(PAGES), "MediaBox" => media,
    };
    doc.insert(2, root);
    let encoder = Deflate::DEFAULT;
    for i in 0..PAGES {
        let (page, drawn, text) = (10 + 3 * i, 11 + 3 * i, 12 + 3 * i);
        let space = vec![Object::from("ICCBased"), Object::reference(3)];
        let mut resources = dict! { "ColorSpace" => dict! { "CS0" => space } };
        if fonts {
            resources.set("Font", dict! { "F1" => Object::reference(4) });
            resources.set("XObject", dict! { "X" => Object::reference(6) });
        }
        let contents = vec![Object::reference(drawn), Object::reference(text)];
        let parent = Object::reference(2);
        let dict = dict! {
            "Type" => "Page", "Parent" => parent, "Contents" => contents, "Resources" => resources,
        };
        doc.insert(page, dict);
        doc.insert(drawn, stream(dict! {}, DRAWN.as_bytes().to_vec()));
        let mut deflated = Vec::new();
        let written = match fonts {
            true => format!("{TEXT}{FORM}"),
            false => TEXT.to_string(),
        };
        encoder
            .encode(written.as_bytes(), Prediction::None, &mut deflated)
            .unwrap();
        doc.insert(text, stream(dict! { "Filter" => "FlateDecode" }, deflated));
    }
    doc.insert(3, stream(dict! { "N" => 4 }, Vec::new()));
    if fonts {
        let font = dict! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding", "ToUnicode" => Object::reference(5),
        };
        doc.insert(4, font);
        let map = b"1 begincodespacerange <00> <FF> endcodespacerange \
            2 beginbfchar <41> <0061> <4B> <006B> endbfchar";
        doc.insert(5, stream(dict! {}, map.to_vec()));
        let form = b"q BT /F1 9 Tf 72 72 Td (A form's line) Tj ET Q".to_vec();
        doc.insert(
            6,
            stream(dict! { "Type" => "XObject", "Subtype" => "Form" }, form),
        );
    }
    let mut options = write::Options::DEFAULT;
    options.kind = XrefKind::Stream;
    options.pack = Some(write::Pack::DEFAULT);
    let mut out = Vec::new();
    write::document(&mut out, &doc, None, None, &options).unwrap();
    out
}

/// A stream of `dict` and `data`.
fn stream(dict: pdfs::pdf_objects::Dict, data: Vec<u8>) -> Object {
    Object::new(Value::Stream(Box::new(Stream::new(dict, data))))
}

/// The reading's options the facade opens with by default.
fn direct_options() -> read::Options<'static> {
    let mut options = read::Options::DEFAULT;
    options.recovery = Recovery::Pdfium;
    options.flate = Backend::ZlibRs;
    options.faults = Faults::Off;
    options
}

/// The direct side's pages: the catalog, and its tree walked by PDFium's
/// rules, as the facade walks it.
fn direct_pages(reading: &Reading<&[u8]>) -> usize {
    let catalog = pdfs::pdf_semantics::catalog(reading, reading.trailer().dict());
    let catalog = catalog.unwrap().unwrap();
    let limits = SemanticsLimits::DEFAULT;
    pages::pages(reading, catalog, Rules::Pdfium, &limits, &mut ())
        .unwrap()
        .len()
}

/// The direct side's content: each page's streams joined and its color
/// spaces counted, as the facade joins and counts them; each page's bytes.
fn direct_content(reading: &Reading<&[u8]>) -> Vec<Vec<u8>> {
    let catalog = pdfs::pdf_semantics::catalog(reading, reading.trailer().dict());
    let catalog = catalog.unwrap().unwrap();
    let limits = SemanticsLimits::DEFAULT;
    let table = pages::pages(reading, catalog, Rules::Pdfium, &limits, &mut ()).unwrap();
    let content = ContentLimits::DEFAULT;
    (table.iter())
        .map(|page| {
            let mut join = Join::new(&content);
            if let Some(contents) = page.dict.and_then(|dict| dict.get(b"Contents")) {
                join.push_contents(reading, contents).unwrap();
            }
            let resources = page.attribute(Inheritable::Resources).map(|a| a.written);
            let res = ResCx::read(reading, resources, &content).unwrap();
            black_box(res);
            join.finish().bytes().to_vec()
        })
        .collect()
}

/// The direct side's text: each page's content joined and its color spaces
/// counted, then the strings it shows walked, over fonts read once a file and a
/// font budget each page, as the facade reads it; each page's text.
fn direct_text(reading: &Reading<&[u8]>) -> Vec<String> {
    let catalog = pdfs::pdf_semantics::catalog(reading, reading.trailer().dict());
    let catalog = catalog.unwrap().unwrap();
    let limits = Limits::DEFAULT;
    let table = pages::pages(reading, catalog, Rules::Pdfium, &limits.semantics, &mut ()).unwrap();
    let fonts = Fonts::default();
    let mut fonts_limits = limits.fonts;
    fonts_limits.content = limits.content;
    (table.iter())
        .map(|page| {
            let mut join = Join::new(&limits.content);
            if let Some(contents) = page.dict.and_then(|dict| dict.get(b"Contents")) {
                join.push_contents(reading, contents).unwrap();
            }
            let resources = page.attribute(Inheritable::Resources).map(|a| a.written);
            let res = ResCx::read(reading, resources, &limits.content).unwrap();
            let content = join.finish();
            let mut budget = limits.font_budget;
            let shown = pdfs::pdf_fonts::shown(
                reading,
                &fonts,
                resources,
                &content,
                &res,
                &fonts_limits,
                &mut budget,
            );
            shown.text()
        })
        .collect()
}

/// The facade's text: each page's.
fn facade_text(file: &[u8]) -> Vec<String> {
    let pdf = pdfs::from_bytes(file).unwrap();
    let pages = pdf.pages().unwrap();
    (pages.iter())
        .map(|page| page.text().unwrap().text())
        .collect()
}

/// The facade's content: each page's bytes.
fn facade_content(file: &[u8]) -> Vec<Vec<u8>> {
    let pdf = pdfs::from_bytes(file).unwrap();
    let pages = pdf.pages().unwrap();
    (pages.iter())
        .map(|page| page.content().unwrap().bytes().to_vec())
        .collect()
}

fn benches(c: &mut Criterion) {
    let file = file(false);
    let file = &file[..];
    let options = direct_options();
    assert_eq!(Options::DEFAULT.flate, options.flate);

    // Each side held to what the other gives.
    let pdf = pdfs::from_bytes(file).unwrap();
    let reading = Reading::open(file, &options).unwrap();
    assert_eq!(pdf.reading().size(), reading.size());
    assert_eq!(pdf.pages().unwrap().len(), PAGES as usize);
    assert_eq!(direct_pages(&reading), PAGES as usize);
    let facade = facade_content(file);
    let direct = direct_content(&Reading::open(file, &options).unwrap());
    assert_eq!(facade, direct);
    let joined = format!("{DRAWN} {TEXT}");
    assert!(facade.iter().all(|page| page == joined.as_bytes()));

    let mut group = c.benchmark_group("open");
    group.throughput(Throughput::Elements(u64::from(PAGES)));
    group.bench_function("facade", |b| {
        b.iter(|| pdfs::from_bytes(black_box(file)).unwrap())
    });
    group.bench_function("direct", |b| {
        b.iter(|| Reading::open(black_box(file), &options).unwrap())
    });
    group.finish();

    let mut group = c.benchmark_group("pages");
    group.throughput(Throughput::Elements(u64::from(PAGES)));
    group.bench_function("facade", |b| {
        b.iter(|| {
            let pdf = pdfs::from_bytes(black_box(file)).unwrap();
            pdf.pages().unwrap().len()
        })
    });
    group.bench_function("direct", |b| {
        b.iter(|| direct_pages(&Reading::open(black_box(file), &options).unwrap()))
    });
    group.finish();

    let mut group = c.benchmark_group("content");
    group.throughput(Throughput::Elements(u64::from(PAGES)));
    group.bench_function("facade", |b| b.iter(|| facade_content(black_box(file))));
    group.bench_function("direct", |b| {
        b.iter(|| direct_content(&Reading::open(black_box(file), &options).unwrap()))
    });
    group.finish();

    let with_fonts = self::file(true);
    let with_fonts = &with_fonts[..];
    let facade = facade_text(with_fonts);
    let direct = direct_text(&Reading::open(with_fonts, &options).unwrap());
    assert_eq!(facade, direct);
    // The `/ToUnicode` reads `A` and `K` as `a` and `k`.
    let line = "a line of text on the page".to_string() + "kernedtext" + "a form's line";
    assert!(facade.iter().all(|page| *page == line), "{}", facade[0]);
    let mut group = c.benchmark_group("text");
    group.throughput(Throughput::Elements(u64::from(PAGES)));
    group.bench_function("facade", |b| b.iter(|| facade_text(black_box(with_fonts))));
    group.bench_function("direct", |b| {
        b.iter(|| direct_text(&Reading::open(black_box(with_fonts), &options).unwrap()))
    });
    group.finish();

    let font = pdfs::standard_font(Std14::Helvetica);
    let line = Std14::Helvetica
        .codes("A line of text on the page")
        .unwrap();
    let saved = facade_save(&font, &line);
    assert_eq!(saved, direct_save(&font, &line));
    assert_eq!(
        pdfs::from_bytes(&saved[..]).unwrap().pages().unwrap().len(),
        PAGES as usize
    );
    let mut group = c.benchmark_group("save");
    group.throughput(Throughput::Elements(u64::from(PAGES)));
    group.bench_function("facade", |b| {
        b.iter(|| facade_save(&font, black_box(&line)))
    });
    group.bench_function("direct", |b| {
        b.iter(|| direct_save(&font, black_box(&line)))
    });
    group.finish();
}

/// A new document's page `i`: a square filled, and a line in `font`.
fn draw(page: &mut ContentWriter, i: u32, font: &Res, line: &[u8]) {
    page.save();
    page.rect(72.0, 72.0, 144.0 + i as f32, 144.0);
    page.paint(Paint::Fill);
    page.restore();
    page.begin_text();
    page.font(font.clone(), 12.0);
    page.text_move(72.0, 700.0);
    page.show(line);
    page.end_text();
}

/// A new document of [`PAGES`] pages saved as the facade saves one.
fn facade_save(font: &Res, line: &[u8]) -> Vec<u8> {
    let mut doc = pdfs::Document::new();
    for i in 0..PAGES {
        draw(doc.add_page(pages::Rect::LETTER), i, font, line);
    }
    doc.to_bytes().unwrap()
}

/// The same document saved by the very calls the facade makes: each page's
/// content deflated as a stream of its own, the font written the first time
/// it's drawn with, the page's dictionary numbered, then a page tree over
/// them, each page its `/Parent`, and the document written.
fn direct_save(font: &Res, line: &[u8]) -> Vec<u8> {
    let trailer = dict! { "Root" => Object::reference(1) };
    let mut doc = Document::new(Version::new(1, 7), trailer);
    doc.insert(1, dict! { "Type" => "Catalog" });
    let mut deflate = Deflate::DEFAULT;
    deflate.backend = Backend::ZlibRs;
    let mut written = HashMap::new();
    let mut placed = Vec::new();
    for i in 0..PAGES {
        let mut page = ContentWriter::new();
        draw(&mut page, i, font, line);
        let (content, needs) = page.finish(&Reserved::default());
        let mut deflated = Vec::new();
        deflate
            .encode(&content, Prediction::None, &mut deflated)
            .unwrap();
        let mut stream = Stream::new(dict! {}, content);
        stream.dict.set("Filter", "FlateDecode");
        stream.set_bytes(deflated);
        let contents = doc.add(Object::new(Value::Stream(Box::new(stream))));
        let mut fonts = pdfs::pdf_objects::Dict::new();
        for need in &needs.0 {
            let id = *written
                .entry(need.res.clone())
                .or_insert_with(|| doc.add(Std14::Helvetica.dict()));
            fonts.set(&need.name, id);
        }
        let media = vec![Object::from(0), 0.into(), 612.into(), 792.into()];
        let dict = dict! {
            "Type" => "Page", "MediaBox" => media, "Resources" => dict! { "Font" => fonts },
            "Contents" => contents,
        };
        placed.push(doc.add(dict));
    }
    let limits = SemanticsLimits::DEFAULT;
    let tree = pages::write(&placed, pages::FANOUT, &limits, || doc.new_id()).unwrap();
    for (id, node) in tree.nodes {
        doc.set(id, node);
    }
    for (&page, &parent) in placed.iter().zip(&tree.parents) {
        doc.get_dict_mut(page).unwrap().set("Parent", parent);
    }
    doc.catalog_mut().unwrap().set("Pages", tree.root);
    let mut options = write::Options::DEFAULT;
    options.kind = XrefKind::Stream;
    options.pack = Some(write::Pack::DEFAULT);
    options.deflate = deflate;
    let mut out = Vec::new();
    write::document(&mut out, &doc, None, None, &options).unwrap();
    out
}

/// The groups, as one benchmark binary: what `criterion_group!` and
/// `criterion_main!` would write, without the public function the first
/// makes.
fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    benches(&mut criterion);
    criterion.final_summary();
}
