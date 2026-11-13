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
//!
//! The file is written here by pdf-structure's writer: a catalog, a root over
//! 2,000 pages, each with a `/Contents` of two streams, one deflated, and
//! resources naming a color space of its own, the page tree's objects in
//! object streams under a cross-reference stream. Throughput is in pages.
//! Each side is held to what it gives before it's timed: the same pages, and
//! each page's content the same bytes. Nothing is read from disk, nothing is
//! encrypted and no cap is met, so the inputs are the same on every run.

use std::hint::black_box;

use criterion::{Criterion, Throughput};
use pdfs::Options;
use pdfs::pdf_content_streams::{Join, Limits as ContentLimits, ResCx};
use pdfs::pdf_filters::{Backend, Prediction};
use pdfs::pdf_objects::object::{Stream, Value};
use pdfs::pdf_objects::pdf_tokenizer::Rules;
use pdfs::pdf_objects::version::Version;
use pdfs::pdf_objects::{Object, dict};
use pdfs::pdf_semantics::Limits as SemanticsLimits;
use pdfs::pdf_semantics::pages::{self, Inheritable};
use pdfs::pdf_structure::read::{self, Faults, Reading, Recovery};
use pdfs::pdf_structure::write::{self, Deflate};
use pdfs::pdf_structure::{Document, XrefKind};

/// How many pages the file has.
const PAGES: u32 = 2_000;

/// A page's two content streams: what it draws, and its text.
const DRAWN: &str = "q 0.5 0 0 0.5 72 72 cm /CS0 cs 0.1 0.2 0.3 0.4 scn 0 0 100 100 re f Q \
    q 1 0 0 RG 2 w 10 10 m 200 200 l S Q";
const TEXT: &str = "BT /F1 12 Tf 72 720 Td (A line of text on the page) Tj 0 -14 Td \
    [(Kerned) -120 (text)] TJ ET";

/// The file the benches read.
fn file() -> Vec<u8> {
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
        let resources = dict! { "ColorSpace" => dict! { "CS0" => space } };
        let contents = vec![Object::reference(drawn), Object::reference(text)];
        let parent = Object::reference(2);
        let dict = dict! {
            "Type" => "Page", "Parent" => parent, "Contents" => contents, "Resources" => resources,
        };
        doc.insert(page, dict);
        doc.insert(drawn, stream(dict! {}, DRAWN.as_bytes().to_vec()));
        let mut deflated = Vec::new();
        encoder
            .encode(TEXT.as_bytes(), Prediction::None, &mut deflated)
            .unwrap();
        doc.insert(text, stream(dict! { "Filter" => "FlateDecode" }, deflated));
    }
    doc.insert(3, stream(dict! { "N" => 4 }, Vec::new()));
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

/// The facade's content: each page's bytes.
fn facade_content(file: &[u8]) -> Vec<Vec<u8>> {
    let pdf = pdfs::from_bytes(file).unwrap();
    let pages = pdf.pages().unwrap();
    (pages.iter())
        .map(|page| page.content().unwrap().bytes().to_vec())
        .collect()
}

fn benches(c: &mut Criterion) {
    let file = file();
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
}

/// The groups, as one benchmark binary: what `criterion_group!` and
/// `criterion_main!` would write, without the public function the first
/// makes.
fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    benches(&mut criterion);
    criterion.final_summary();
}
